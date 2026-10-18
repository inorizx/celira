#!/usr/bin/env python3
"""trace_view.py 的 stdlib unittest：用临时 fixture 定住它对「她那次活动」的判据。

为什么值得测：这支工具的**唯一价值**就是把同一件事的三个落点摆在一起——
经历侧（trace）、事实侧（facts）、宿主侧（host.out）。三个落点里的任何一个
接错字段（例如把 `failure` 读成不存在，或漏掉"这一轮没有模型响应"的标记），
工具就会把「失败的分类只在事实侧」这个结论说错。所以判据要机械。

**两组布局**（2026-10-14 住户化那一刀）：

  `LegacyLayoutTest`    老形状：`<家>/env/` ＋ `<家>/logs/host.out`
  `ResidentsLayoutTest` 今天的形状：`<家>/agents/<支>/` ＋ `<家>/logs/agents/<支>/host.out`，
                        且一次活动**可能同时**在当前段与封口段里（去重是真机事实）

玩法：`python tools/tests/test_trace_view.py`（或 `python -m unittest discover tools/tests`）。
不碰真实的家、不联网。**只有跨语言对照那一条会起一次 node**（拿 `scripts/home/paths.js`
的真值来比本文件算出来的布局名）——这台机器上没有 node 时如实跳过，不假装通过。
"""

from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TOOL = Path(__file__).resolve().parent.parent / "trace_view.py"
PATHS_JS = Path(__file__).resolve().parents[2] / "scripts" / "home" / "paths.js"

RUN = "host-1-2-3"
# 第二个活动：只有注入、没有模型响应的那一轮 —— 用来定住"缺哪一环"的标记。
TURNS = (1, 2)

# 住户化那组用两支住户、两个活动。
AGENT_A = "agent-aaaa"
AGENT_B = "agent-bbbb"
RUN_A = "host-11-22-1"
RUN_B = "host-33-44-1"


def load_tool():
    spec = importlib.util.spec_from_file_location("trace_view", TOOL)
    module = importlib.util.module_from_spec(spec)
    sys.modules["trace_view"] = module
    spec.loader.exec_module(module)
    return module


def write_jsonl(path: Path, records: list[dict], signed: bool = False) -> None:
    """写一份账本。

    `signed=True` 写的是**今天那一行的形状**：`<JSON 正文>\\t<8 位十六进制>`（2026-10-06 起
    带校验和，见 `agent/src/store/format.rs` 的 `signed_line` / `line_sum`）。
    ⚠ 这里那 8 位是**假值**（本工具不核链，只把那一列剥掉）——真链是 CRC-32 累进，
    核它归经历存储与 `environment_diagnose`。
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    lines = []
    for index, record in enumerate(records):
        body = json.dumps(record, ensure_ascii=False)
        lines.append(f"{body}\t{index:08x}" if signed else body)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def run_ending(run: str, failure: str = "transport-failure") -> dict:
    return {
        "kind": "ending",
        "run": run,
        "seq": 8,
        "system": "model-failed",
        "failure": failure,
    }


class TraceViewTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.mod = load_tool()

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        (self.root / "env").mkdir()
        (self.root / "logs").mkdir()
        self._write_fixture()

    def tearDown(self):
        self._tmp.cleanup()

    def _write_fixture(self):
        trace = [
            {"kind": "run-start", "run": RUN, "seq": 1},
            {"kind": "run-input", "run": RUN, "seq": 2, "text": ""},
            {
                "kind": "injection",
                "run": RUN,
                "seq": 3,
                "stage": "turn",
                "turn": 1,
                "blocks": [{"label": "status-bar", "text": "turn = 1/8"}],
            },
            {"kind": "model-response", "run": RUN, "seq": 4, "turn": 1, "action": "im_rooms"},
            {"kind": "action-selected", "run": RUN, "seq": 5, "turn": 1, "action": "im_rooms"},
            {
                "kind": "observed",
                "run": RUN,
                "seq": 6,
                "turn": 1,
                "action": "im_rooms",
                "blocks": [{"label": "im_rooms", "text": "room = 值班室"}],
            },
            {
                "kind": "injection",
                "run": RUN,
                "seq": 7,
                "stage": "turn",
                "turn": 2,
                "blocks": [{"label": "status-bar", "text": "turn = 2/8"}],
            },
            # 新格式：经历侧的 ending 带失败分类（2026-09-19 起）。
            run_ending(RUN),
            {"kind": "sealed", "run": RUN, "seq": 9},
        ]
        facts = [
            {"fact": "run-started", "run": RUN, "seq": 1},
            {
                "fact": "run-ended",
                "run": RUN,
                "seq": 2,
                "system": "model-failed",
                "failure": "transport-failure",
            },
        ]
        host = (
            "celira-agent-host v1\n"
            "[run]\n"
            f"run={RUN} drives=1 ending=system:model-failed "
            "model-failure=transport-failure notification=read\n"
        )
        write_jsonl(self.root / "env" / "trace.ndjson", trace)
        write_jsonl(self.root / "env" / "facts.ndjson", facts)
        (self.root / "logs" / "host.out").write_text(host, encoding="utf-8")

    def run_tool(self, *argv):
        """跑一次工具，返回 (退出码, 全部 stdout)。--home 必须排在子命令前。"""
        buffer = io.StringIO()
        with contextlib.redirect_stdout(buffer):
            code = self.mod.main(["--home", str(self.root), *argv])
        return code, buffer.getvalue()

    def test_show_puts_three_landings_side_by_side(self):
        """三个落点必须同屏，且新记录的**经历侧也带分类**（这一刀修的就是它）。"""
        code, text = self.run_tool("show", RUN)
        self.assertEqual(code, 0)
        self.assertIn("经历侧 ending     : system=model-failed failure=transport-failure", text)
        self.assertIn("事实侧 run-ended  : system=model-failed failure=transport-failure", text)
        self.assertIn("宿主侧 [run]      : system:model-failed model-failure=transport-failure", text)
        # 三处一致 ⇒ 不该出现"说法不一致"的提示。
        self.assertNotIn("三处说法不一致", text)

    def test_show_reports_when_the_three_landings_disagree(self):
        """旧记录（经历侧没有 failure）⇒ 如实说"不一致"，不替任何一侧圆场。"""
        path = self.root / "env" / "trace.ndjson"
        lines = [
            json.loads(line)
            for line in path.read_text(encoding="utf-8").splitlines()
            if line.strip()
        ]
        for record in lines:
            if record.get("kind") == "ending":
                record.pop("failure", None)
        write_jsonl(path, lines)

        code, text = self.run_tool("show", RUN)
        self.assertEqual(code, 0)
        self.assertIn("经历侧 ending     : system=model-failed failure=-", text)
        self.assertIn("三处说法不一致", text)

    def test_show_marks_the_turn_without_model_response(self):
        """缺哪一环要机械标出来，不能靠读的人自己发现。"""
        _, text = self.run_tool("show", RUN)
        self.assertIn("turn 1: injection / model-response / action-selected / observed", text)
        self.assertIn("turn 2: injection  <-- 这一轮没有模型响应", text)

    def test_show_prints_run_input_text_when_present(self):
        """活动开场输入是正文，不该被静默省略（这里为空串，故只断言不炸且字段被访问）。"""
        _, text = self.run_tool("show", RUN)
        self.assertIn("#2 run-input", text)

    def test_show_unknown_activity_fails_closed(self):
        code, text = self.run_tool("show", "host-nope")
        self.assertEqual(code, 1)
        self.assertIn("经历里没有这个活动", text)

    def test_list_parses_host_wake_rows(self):
        code, text = self.run_tool("list")
        self.assertEqual(code, 0)
        self.assertIn(RUN, text)
        self.assertIn("transport-failure", text)

    def test_list_still_reads_the_old_wake_shaped_rows(self):
        """**旧日志照读**（2026-10-10 在场那一刀把 `[wake]` 那一块改成了 `[run]`）。

        本工具是看历史用的：她昨天那段账里的行是 `wake run=…`，今天的是 `run=…`——
        两个形状都必须读得出来，否则"很久以前那次活动怎么断的"就查不到了。
        """
        (self.root / "logs" / "host.out").write_text(
            "celira-agent-host v1\n"
            "[wake]\n"
            f"wake run={RUN} drives=1 ending=subject:held notification=read\n",
            encoding="utf-8",
        )
        code, text = self.run_tool("list")
        self.assertEqual(code, 0)
        self.assertIn(RUN, text)
        self.assertIn("subject:held", text)

    def test_list_reports_host_log_is_per_boot_only(self):
        """宿主日志只覆盖本次开机 —— 这个限制必须写在读数旁边，不能让人误以为它累积。"""
        _, text = self.run_tool("list")
        self.assertIn("本次开机", text)

    def test_where_is_honest_when_a_log_is_missing(self):
        (self.root / "logs" / "im.out").unlink(missing_ok=True)
        code, text = self.run_tool("where")
        self.assertEqual(code, 0)
        self.assertIn("[缺]", text)

    def test_facts_unknown_activity_fails_closed(self):
        code, text = self.run_tool("facts", "host-nope")
        self.assertEqual(code, 1)
        self.assertIn("事实里没有这个活动", text)


class ResidentsLayoutTest(unittest.TestCase):
    """住户化布局：一个家住多支，每支有自己的环境根与宿主日志；经历分当前段与封口段。"""

    @classmethod
    def setUpClass(cls):
        cls.mod = load_tool()

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self._write_fixture()

    def tearDown(self):
        self._tmp.cleanup()

    def _write_fixture(self):
        a = self.root / "agents" / AGENT_A
        b = self.root / "agents" / AGENT_B
        # 甲支：活动**跨两段**（前半在封口段里），且 seq 3 两处都在（真机那种重叠）。
        head = [
            {"kind": "run-start", "run": RUN_A, "seq": 1},
            {"kind": "run-input", "run": RUN_A, "seq": 2, "text": ""},
            {
                "kind": "injection",
                "run": RUN_A,
                "seq": 3,
                "stage": "turn",
                "turn": 1,
                "blocks": [{"label": "status-bar", "text": "turn = 1/8"}],
            },
        ]
        tail = [
            {"kind": "injection", "run": RUN_A, "seq": 3, "stage": "turn", "turn": 1, "blocks": []},
            {"kind": "model-response", "run": RUN_A, "seq": 4, "turn": 1, "action": "im_rooms"},
            {"kind": "sealed", "run": RUN_A, "seq": 5},
        ]
        write_jsonl(a / "trace.parts" / "000000000000.ndjson", head, signed=True)
        write_jsonl(a / "trace.ndjson", tail, signed=True)
        write_jsonl(
            a / "facts.ndjson",
            [
                {"fact": "run-started", "run": RUN_A, "seq": 1},
                {
                    "fact": "run-ended",
                    "run": RUN_A,
                    "seq": 2,
                    "system": "model-failed",
                    "failure": "transport-failure",
                },
            ],
            signed=True,
        )
        # 乙支：一个正常收场的活动。
        write_jsonl(
            b / "trace.ndjson",
            [
                {"kind": "run-start", "run": RUN_B, "seq": 1},
                run_ending(RUN_B, failure=""),
            ],
        )
        write_jsonl(
            b / "facts.ndjson",
            [{"fact": "run-started", "run": RUN_B, "seq": 1}],
        )
        (self.root / "logs" / "agents" / AGENT_A).mkdir(parents=True)
        (self.root / "logs" / "agents" / AGENT_B).mkdir(parents=True)
        (self.root / "logs" / "agents" / AGENT_A / "host.out").write_text(
            "celira-agent-host v1\n"
            "[run]\n"
            f"run={RUN_A} drives=1 ending=system:model-failed "
            "model-failure=transport-failure notification=read\n",
            encoding="utf-8",
        )
        (self.root / "logs" / "agents" / AGENT_B / "host.out").write_text(
            "celira-agent-host v1\n"
            "[run]\n"
            f"run={RUN_B} drives=1 ending=subject:ended notification=read\n",
            encoding="utf-8",
        )

    def run_tool(self, *argv):
        buffer = io.StringIO()
        with contextlib.redirect_stdout(buffer):
            code = self.mod.main(["--home", str(self.root), *argv])
        return code, buffer.getvalue()

    def test_where_names_each_resident_and_the_layout(self):
        code, text = self.run_tool("where")
        self.assertEqual(code, 0)
        self.assertIn("住户化（2 支）", text)
        self.assertIn(AGENT_A, text)
        self.assertIn(AGENT_B, text)
        # 每一支的**日志目录**要印全（操作者照它去看宿主那侧的原件）。
        self.assertIn(str(self.root / "logs" / "agents" / AGENT_A), text)

    def test_where_says_so_when_the_home_has_neither_shape(self):
        empty = Path(self._tmp.name) / "nope"
        empty.mkdir()
        buffer = io.StringIO()
        with contextlib.redirect_stdout(buffer):
            code = self.mod.main(["--home", str(empty), "where"])
        self.assertEqual(code, 0)
        self.assertIn("两种形状都没有", buffer.getvalue())

    def test_show_joins_the_current_segment_with_the_sealed_ones(self):
        """一次活动跨段（前半封口、后半还在当前段）时，读数必须是**两段的并集**。"""
        code, text = self.run_tool("show", RUN_A)
        self.assertEqual(code, 0)
        self.assertIn("#1 run-start", text)
        self.assertIn("#5 sealed", text)
        self.assertIn(f"支 {AGENT_A}", text)

    def test_show_reports_the_deduplicated_count(self):
        """两边都在的那一条**只算一次**：两段合起来 6 条、去重后 5 条（seq 1–5）。"""
        _, text = self.run_tool("show", RUN_A)
        self.assertIn("经历 5 条（seq 1–5）", text)

    def test_signed_lines_are_read_and_the_checksum_column_is_not_part_of_the_body(self):
        """**今天那一行的形状**（行尾 `\\t<8 位十六进制>`）必须读得出来。

        这一刀之前，旧读法对今天的账本**一行都解不出来**（真机：小欣那本 3353 条记录全部
        落进"解不出来"），而工具自己不会说这件事。
        """
        signed = (self.root / "agents" / AGENT_A / "trace.ndjson").read_text(encoding="utf-8")
        self.assertIn("\t", signed, "fixture 本身必须带那一列，否则这条用例没验到东西")
        code, text = self.run_tool("show", RUN_A)
        self.assertEqual(code, 0)
        self.assertIn("#1 run-start", text)
        self.assertNotIn("读不出来", text)

    def test_a_line_whose_checksum_column_is_unreadable_is_reported_not_swallowed(self):
        """行尾那一列读不出来 ⇒ 权威读法 fail-closed；本工具**照实标出来**，不当旧记录。"""
        path = self.root / "agents" / AGENT_A / "trace.ndjson"
        path.write_text(
            path.read_text(encoding="utf-8") + '{"kind":"sealed","run":"x","seq":9}\tZZZ\n',
            encoding="utf-8",
        )
        code, text = self.run_tool("show", RUN_A)
        self.assertEqual(code, 0)
        self.assertIn("行读不出来", text)
        self.assertIn("行尾校验列读不出来 1 行", text)

    def test_unsigned_old_lines_still_read(self):
        """乙支那份是**加校验和之前**的形状（没有那一列）：旧账照读。"""
        unsigned = (self.root / "agents" / AGENT_B / "trace.ndjson").read_text(encoding="utf-8")
        self.assertNotIn("\t", unsigned)
        code, text = self.run_tool("show", RUN_B)
        self.assertEqual(code, 0)
        self.assertIn("#1 run-start", text)

    def test_list_reads_every_residents_host_log(self):
        code, text = self.run_tool("list")
        self.assertEqual(code, 0)
        self.assertIn(RUN_A, text)
        self.assertIn(RUN_B, text)
        self.assertIn("subject:ended", text)

    def test_agent_filter_narrows_to_one_resident(self):
        # `--agent` 是全局选项：写在子命令**之前**（与 --home 同款）。
        _, text = self.run_tool("--agent", AGENT_B, "list")
        self.assertIn(RUN_B, text)
        self.assertNotIn(RUN_A, text)

        code, text = self.run_tool("--agent", AGENT_B, "show", RUN_A)
        self.assertEqual(code, 1)
        self.assertIn("经历里没有这个活动", text)


class LayoutNamesMatchTheHomeSideAuthorityTest(unittest.TestCase):
    """本文件算出来的家侧布局名，必须与 `scripts/home/paths.js` 一致。

    为什么值得起一次 node：这一刀之前，`trace_view.py` 里那份布局是**第二份判据**，
    而它**悄悄过期**了——住户化之后它对今天的家全部报缺（`where` 里三项 `[缺]`），
    没人看得见。跨语言对照把"第二份"变成"被核对的那一份"。
    """

    @classmethod
    def setUpClass(cls):
        cls.mod = load_tool()

    def test_layout_names_match_the_home_side_authority(self):
        node = shutil.which("node")
        if node is None:
            self.skipTest("这台机器上没有 node：跨语言对照跳过（家侧脚本本来就要 node）")
        script = (
            "const P=require(process.argv[1]);"
            "const h='H', a='A';"
            "console.log(P.agentsRoot(h));"
            "console.log(P.agentEnvRoot(h,a));"
            "console.log(P.agentLogDir(h,a));"
            "console.log(P.agentHostLog(h,a));"
            "console.log(P.legacyEnvRoot(h));"
        )
        try:
            done = subprocess.run(
                [node, "-e", script, str(PATHS_JS)],
                capture_output=True,
                text=True,
                timeout=60,
            )
        except (OSError, subprocess.SubprocessError) as error:  # pragma: no cover - 环境相关
            self.skipTest(f"node 起不来（{error}）：跨语言对照跳过")
        self.assertEqual(done.returncode, 0, done.stderr)
        got = done.stdout.strip().splitlines()
        self.assertEqual(len(got), 5, done.stdout)
        want = [
            Path("H") / self.mod.AGENTS_DIR,
            Path("H") / self.mod.AGENTS_DIR / "A",
            Path("H") / "logs" / self.mod.AGENTS_DIR / "A",
            Path("H") / "logs" / self.mod.AGENTS_DIR / "A" / self.mod.HOST_LOG,
            Path("H") / self.mod.LEGACY_ENV_DIR,
        ]
        for name, (from_js, from_tool) in zip(
            ["agentsRoot", "agentEnvRoot", "agentLogDir", "agentHostLog", "legacyEnvRoot"],
            zip(got, want),
        ):
            self.assertEqual(
                Path(from_js).parts,
                from_tool.parts,
                f"布局名对不上：paths.js 的 {name} 说 {from_js}，本工具算 {from_tool}",
            )


if __name__ == "__main__":
    unittest.main(verbosity=2)
