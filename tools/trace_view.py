#!/usr/bin/env python3
"""Celira 经历查看器：只读地看「她在每一拍里经历了什么」。

为什么需要它 —— 三个落点的信息量本来不一样，肉眼对不出来：

  <环境根>/trace.ndjson   她的经历（事件本体：注入 / 模型响应 / 行动 / 观察 / 收场）
  <环境根>/facts.ndjson   耐久事实（run-started / action-* / run-ended ...）
  logs/agents/<支>/host.out   宿主的 [run] 行（每段在场一行：结局 + 失败分类）

**失败分类的不对称已经修了**（2026-09-19）：经历侧的 `ending` 现在也带 `failure`
（`store.rs` 的 `ending_payload`），与事实侧 `run-ended` 的 `failure` 同口径。
**旧记录**（修之前写的）仍然只有 `system = model-failed`，读出来就是缺字段 ——
本工具把两边都摆出来，缺的就是缺的，不替任何一侧遮掩。

## 住户化布局（2026-10-14 这一刀）

一个家可以住**多支** Agent：每支有自己的环境根 `<家>/agents/<支>/` 与自己的宿主日志
`<家>/logs/agents/<支>/host.out`。从前这支工具按老的单支形状找（`<家>/env/…`、
`<家>/logs/host.out`），于是住户化之后它**对今天的家全部报缺**（真机：`where` 里三项 `[缺]`）
—— 看日志这件事本身过期了，而 `agent/README.md` 还把它当作"看清某段怎么断的"的替代读法。

现在：**两种形状都认，住户化优先**；老形状照读（它是看历史用的，与 `[wake]` / `[run]` 两种
行形状同一条纪律）。一个家里两种形状同时在（搬家搬到一半）也照实各列一节。

## 分段：一次活动可能**同时**在两个地方

活动收场时那一段被**封口**挪进 `<环境根>/trace.parts/<序号>.ndjson`（事实侧同款
`facts.parts/`），当前那一段留在 `trace.ndjson`。⇒ 封口那一刻**在飞**的那一次会同时留在
两处（真机读数：dev 家 140 个活动里有 1 个两边都在，小欣 41 个里 1 个）——
所以读的时候按 `(run, kind|fact, seq)` **去重**。那不是防御性代码，是一条真机事实。

## 行格式：正文 ＋ 行尾校验和（2026-10-06 起）

今天每一条记录的一行是 **`<JSON 正文>\t<8 位十六进制>`**——校验和写在行尾、制表符之后
（权威读法见 `agent/src/store/format.rs` 的 `line_sum`：按**最后一个**制表符切；正文里不可能
出现裸制表符，JSON 会把它转义成 `\t` 两个字符）。加校验和之前写下的**旧记录没有那一段**，
两种都要读得出来。

⇒ 从前这支工具直接对整行 `json.loads` ⇒ **对今天的账本一行都解不出来**（真机：小欣那本
3353 条记录**全部**落进"解不出来"），而它自己不会说这件事。现在按权威规则切开；
制表符之后**不是**十六进制的那种行，权威读法是**明确失败**（fail-closed），
本工具照实标 `[读不出来]`——**不静默当成旧记录**（"没有校验和"与"校验和读不出来"是两件事）。

⚠ 本工具**不核校验和链**（`chain_i = crc32(chain_{i-1}, body_i)` 那条链）：那是经历存储的
写入者/读者的事，诊断用 `environment_diagnose`。这里只把那一列剥掉再解 JSON。

用法（HOME 取自 --home 或环境变量 CELIRA_HOME_DIR，默认 D:/celira-home）：

  python tools/trace_view.py where                  # 有哪些日志、多大、多新（逐支一节）
  python tools/trace_view.py list [--n 20]          # 最近的活动（结局 + 失败分类）
  python tools/trace_view.py list --failures        # 只看没走完的
  python tools/trace_view.py show <run-id>          # 逐条经历 + 三落点结局对比
  python tools/trace_view.py facts <run-id>         # 该活动的耐久事实
  python tools/trace_view.py show <run-id> --full   # 不截断正文

⚠ `--home` 与 `--agent` 都是**全局选项**：写在子命令**之前**（`--agent <支> list`）。

只读：本工具不写任何文件、不连接任何服务、不起进程。
零第三方依赖，只用标准库。全部文本按 UTF-8 读与写（Windows 终端默认码页会毁中文）。
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import sys
from pathlib import Path

# 正文默认截断宽度（--full 关闭）。
TEXT_LIMIT = 1200

# ---------------------------------------------------------------------------
# 布局：**读者视角的一份判据，两个形状**
# ---------------------------------------------------------------------------
#
# ⚠ 这些名字的**权威不在本文件**：家侧在 `scripts/home/paths.js`（`AGENTS_DIR` /
# `agentEnvRoot` / `agentLogDir` / `agentHostLog` / `legacyEnvRoot`），经历与事实那几个
# 文件名在 Agent 侧（`agent/src/store/` 与写经历那一层）。本文件是**读者**，不是第二份判据
# —— 所以 `tools/tests/test_trace_view.py` 里有一条**跨语言对照**：拿 node 问一次
# `paths.js` 的真值，把家侧那几个名字与这里算出来的比一遍。哪天布局改了而这里没跟上，
# 那条用例就红；**这一刀之前，它就是悄悄过期的那种坏法**（没人看得见）。
AGENTS_DIR = "agents"
LEGACY_ENV_DIR = "env"
TRACE_FILE = "trace.ndjson"
FACTS_FILE = "facts.ndjson"
TRACE_PARTS_DIR = "trace.parts"
FACTS_PARTS_DIR = "facts.parts"
HOST_LOG = "host.out"
IM_LOG = "im.out"


def out(text: str = "") -> None:
    sys.stdout.write(text + "\n")


def stamp(seconds: float) -> str:
    return datetime.datetime.fromtimestamp(seconds).strftime("%Y-%m-%d %H:%M:%S")


def read_lines(path: Path) -> list[str]:
    """整个文件按 UTF-8 读成行；缺文件就如实说，不假装空。"""
    if not path.exists():
        return []
    with open(path, encoding="utf-8", errors="replace") as handle:
        return [line for line in handle.read().splitlines() if line.strip()]


def segments(current: Path, parts_dir: Path) -> list[Path]:
    """一个落点的**全部段**：封口段（按序号，即时间序）在前，当前段在后。

    两处都可能有同一次活动（见文件头「分段」）⇒ 调用方必须去重，见 `read_jsonl`。
    """
    found: list[Path] = []
    if parts_dir.is_dir():
        found.extend(sorted(entry for entry in parts_dir.iterdir() if entry.suffix == ".ndjson"))
    if current.exists():
        found.append(current)
    return found


def split_checksum(line: str) -> tuple[str, str]:
    """按权威规则切开「正文 ＋ 行尾校验和」。

    返回 `(正文, "present" | "absent" | "unreadable")`——**三态，不压成两态**
    （与 `agent/src/store/format.rs` 的 `LineSum` 一一对应）：

    - 没有制表符 ⇒ `absent`：加校验和（2026-10-06）之前写下的旧记录，正文就是整行；
    - 制表符之后是十六进制 ⇒ `present`：正文是制表符之前那一段；
    - 制表符之后**不是**十六进制 ⇒ `unreadable`：尾巴被截断、或这一行被改过——
      权威读法在这里**明确失败**（fail-closed），因此本工具也**不把它当旧记录**。
    """
    index = line.rfind("\t")
    if index < 0:
        return line, "absent"
    tail = line[index + 1 :]
    if tail and all(character in "0123456789abcdefABCDEF" for character in tail):
        return line[:index], "present"
    return line, "unreadable"


def parse_line(line: str) -> dict:
    body, shape = split_checksum(line)
    if shape == "unreadable":
        # 读不出来就说读不出来：不静默降级成"旧记录"，也不假装它不存在。
        return {"__unreadable__": line[:200]}
    try:
        return json.loads(body)
    except json.JSONDecodeError:
        # 半行（正在追加）是正常现象，跳过但不隐藏计数。
        return {"__unparsable__": line[:200]}


# 读不出来的两种行（各自都要能数出来：**跳过 ≠ 没有**）。
BROKEN_KEYS = ("__unparsable__", "__unreadable__")


def broken_records(records: list[dict]) -> list[dict]:
    return [record for record in records if any(key in record for key in BROKEN_KEYS)]


def broken_note(records: list[dict]) -> str:
    """一句读数：这本账里有多少行读不出来、各是哪种（读不出来不许静默）。"""
    if not records:
        return ""
    half = sum(1 for record in records if "__unparsable__" in record)
    tail = sum(1 for record in records if "__unreadable__" in record)
    parts = []
    if half:
        parts.append(f"半行/解不出来 {half} 行")
    if tail:
        parts.append(f"行尾校验列读不出来 {tail} 行")
    return (
        f"  ⚠ 这本账里有 {len(records)} 行读不出来（{' · '.join(parts)}）："
        "它们不在这份清单里——别把「缺」当成「没有」"
    )


def record_key(record: dict) -> tuple | None:
    """去重键：`(run, kind|fact, seq)`；读不出这三格（半行、旧形状）⇒ `None`（不去重）。"""
    run = record.get("run")
    if not run:
        return None
    return (run, record.get("kind") or record.get("fact"), record.get("seq"))


def read_jsonl(paths: list[Path]) -> list[dict]:
    """把若干段读成一份记录表，**按 `(run, kind|fact, seq)` 去重**。

    去重的理由是真机事实（封口段与当前段会同时含一次活动），不是防御性编程；
    读不出键的行（半行、没有活动标识的旧记录）一律**原样保留**，不合并。
    """
    seen: set = set()
    records: list[dict] = []
    for path in paths:
        for line in read_lines(path):
            record = parse_line(line)
            key = record_key(record)
            if key is not None:
                if key in seen:
                    continue
                seen.add(key)
            records.append(record)
    return records


class Resident:
    """一支住户的三个落点。老家的单支环境根也用它（只是地址与标签不同）。"""

    def __init__(self, agent_id: str, env_root: Path, log_dir: Path, legacy: bool = False) -> None:
        self.agent_id = agent_id
        self.env_root = env_root
        self.log_dir = log_dir
        self.legacy = legacy

    @property
    def label(self) -> str:
        return f"（老家单支）{self.agent_id}" if self.legacy else self.agent_id

    def trace_files(self) -> list[Path]:
        return segments(self.env_root / TRACE_FILE, self.env_root / TRACE_PARTS_DIR)

    def facts_files(self) -> list[Path]:
        return segments(self.env_root / FACTS_FILE, self.env_root / FACTS_PARTS_DIR)

    def host_log(self) -> Path:
        return self.log_dir / HOST_LOG


class Home:
    """一个家：住户化的 `<家>/agents/<支>/` ＋ 老形状的 `<家>/env/`（在场就都认）。"""

    def __init__(self, root: Path) -> None:
        self.root = root
        self.residents = self._residents()
        self.legacy = self._legacy()

    def _residents(self) -> list[Resident]:
        base = self.root / AGENTS_DIR
        if not base.is_dir():
            return []
        return [
            Resident(entry.name, entry, self.root / "logs" / AGENTS_DIR / entry.name)
            for entry in sorted(base.iterdir())
            if entry.is_dir()
        ]

    def _legacy(self) -> Resident | None:
        env = self.root / LEGACY_ENV_DIR
        if not env.is_dir():
            return None
        return Resident(LEGACY_ENV_DIR, env, self.root / "logs", legacy=True)

    def all(self) -> list[Resident]:
        found = list(self.residents)
        if self.legacy is not None:
            found.append(self.legacy)
        return found

    def layout(self) -> str:
        notes = []
        if self.residents:
            notes.append(f"住户化（{len(self.residents)} 支）")
        if self.legacy is not None:
            notes.append("老形状 env/ 还在")
        return " ＋ ".join(notes) if notes else "两种形状都没有（这个家还没布置过？）"


def describe(label: str, paths: list[Path], kept: str) -> None:
    """一个落点的一节读数：**在就是几段多大、缺就是缺**（不假装空）。"""
    present = [path for path in paths if path.exists()]
    if not present:
        where = paths[0] if paths else Path("（没有任何段）")
        out(f"  [缺] {label:<8} {where}")
        out(f"           保留策略：{kept}")
        return
    total = sum(path.stat().st_size for path in present)
    newest = max(present, key=lambda path: path.stat().st_mtime)
    out(
        f"  {label:<8} {len(present)} 段 · {total} 字节 · 最后写入 "
        f"{stamp(newest.stat().st_mtime)}"
    )
    out(f"           {' / '.join(path.name for path in present)}")
    out(f"           保留策略：{kept}")


def cmd_where(home: Home) -> int:
    out(f"home = {home.root}")
    out(f"布局 = {home.layout()}")
    out()
    residents = home.all()
    if not residents:
        out(f"  （{home.root} 下既没有 {AGENTS_DIR}/ 也没有 {LEGACY_ENV_DIR}/：没有可看的落点）")
    for resident in residents:
        out(f"支 {resident.label}")
        out(f"  环境根   {resident.env_root}")
        out(f"  日志目录 {resident.log_dir}")
        describe("她的经历", resident.trace_files(), "追加累积（跨开机；封口段进 trace.parts/）")
        describe("耐久事实", resident.facts_files(), "追加累积（跨开机；封口段进 facts.parts/）")
        describe("宿主日志", [resident.host_log()], "每次开机被清空（up.js 主动 truncate）")
    describe("im 日志", [home.root / "logs" / IM_LOG], "每次开机被清空")
    out()
    out("所以：想查「很久以前那次活动怎么断的」，宿主那侧的读数多半已经不在了；")
    out("她的经历与耐久事实还在（模型失败的分类也活在 run-ended 的 failure 字段里）。")
    return 0


def wake_rows(path: Path) -> list[str]:
    """宿主 stdout 里的 `[run]` 行 —— 每段在场一行，含结局与失败分类（若有）。

    ⚠ 2026-10-10 在场那一刀：那一行的前缀从 `[wake]` 改成 `[run]`，内容行从 `wake run=…`
    变成 `run=…`（一段在场会吸收后来的每一声音铃，一次唤醒不再是"一段"）。**旧日志里的
    `wake run=` 照读**——本工具是看历史用的，两边都认。
    """
    rows = []
    for line in read_lines(path):
        if line.startswith("run=") or line.startswith("wake run="):
            rows.append(line)
    return rows


def parse_wake(line: str) -> dict:
    """`run=<id> drives=N ending=... [model-failure=...] notification=...`

    （旧日志那一行是 `wake run=…`：**同一批字段**，只有多一个前导词。）
    """
    fields: dict[str, str] = {}
    for token in line.split():
        if "=" in token:
            key, _, value = token.partition("=")
            fields[key] = value
    return fields


def cmd_list(home: Home, limit: int, failures_only: bool, agent: str | None) -> int:
    residents = [r for r in home.all() if agent is None or r.agent_id == agent]
    out(
        "来源：各支的 <家>/logs/agents/<支>/host.out 的 [run] 行 —— 只覆盖**本次开机以来**"
        "（每次 up 会清空这个文件）。"
    )
    out(f"布局 = {home.layout()}")
    if agent is not None:
        out(f"（只看这一支：{agent}）")
    if not residents:
        out(f"没有可看的住户：{home.root}")
        return 0
    out()
    for resident in residents:
        rows = [parse_wake(line) for line in wake_rows(resident.host_log())]
        shown = rows
        if failures_only:
            shown = [row for row in rows if not row.get("ending", "").startswith("subject:")]
        runs_in_trace = {
            record.get("run") for record in read_jsonl(resident.trace_files()) if record.get("run")
        }
        out(
            f"支 {resident.label}：本次开机 {len(rows)} 段在场"
            f"（显示最后 {min(len(shown), limit)} 段）；她的经历里 {len(runs_in_trace)} 次活动"
            "（跨开机累积）"
        )
        if not shown:
            out("    （这一段里没有要显示的 [run] 行）")
            out()
            continue
        out(f"    {'activity id':<34} {'ending':<26} failure")
        for row in shown[-limit:]:
            out(
                f"    {row.get('run', '?'):<34} "
                f"{row.get('ending', '?'):<26} "
                f"{row.get('model-failure', '-')}"
            )
        out()
    return 0


def render_block(block: dict, full: bool) -> str:
    text = block.get("text", "")
    if not full and len(text) > TEXT_LIMIT:
        text = text[:TEXT_LIMIT] + f"\n… （截断，共 {len(block.get('text', ''))} 字，用 --full 看全文）"
    return text


def cmd_show(home: Home, run_id: str, full: bool, agent: str | None) -> int:
    for resident in home.all():
        if agent is not None and resident.agent_id != agent:
            continue
        everything = read_jsonl(resident.trace_files())
        records = [record for record in everything if record.get("run") == run_id]
        if not records:
            continue
        records.sort(key=lambda record: record.get("seq", 0))

        facts = [
            record for record in read_jsonl(resident.facts_files()) if record.get("run") == run_id
        ]
        ending_trace = next((r for r in records if r.get("kind") == "ending"), None)
        ending_fact = next((r for r in facts if r.get("fact") == "run-ended"), None)
        wake = next(
            (parse_wake(row) for row in wake_rows(resident.host_log()) if run_id in row), {}
        )

        out(f"活动 {run_id}")
        out(f"  支 {resident.label}")
        note = broken_note(broken_records(everything))
        if note:
            out(note)
        out(f"  经历 {len(records)} 条（seq {records[0].get('seq')}–{records[-1].get('seq')}）")
        out(f"  事实 {len(facts)} 条")
        out()
        out("结局对比（同一件事的三个落点）")
        if ending_trace:
            # 新记录带 `failure`；修之前写的旧记录只有 `system` —— 缺就是缺，写成 `-`，不编造。
            trace_failure = ending_trace.get("failure") or "-"
            out(
                f"  经历侧 ending     : system={ending_trace.get('system')} "
                f"failure={trace_failure}"
            )
        else:
            out("  经历侧 ending     : （没有）")
        if ending_fact:
            detail = ending_fact.get("failure") or "-"
            out(f"  事实侧 run-ended  : system={ending_fact.get('system')} failure={detail}")
        else:
            out("  事实侧 run-ended  : （没有）")
        out(
            f"  宿主侧 [run]      : {wake.get('ending', '（没有）')} "
            f"model-failure={wake.get('model-failure', '-')}"
        )
        # 三处说法不一致时**说出来**，而不是挑一个看起来顺眼的。旧记录会落在这一档。
        observed = {
            (ending_trace.get("failure") if ending_trace else None),
            (ending_fact.get("failure") if ending_fact else None),
            wake.get("model-failure"),
        }
        if len(observed) > 1:
            out("  （三处说法不一致：上面每行都是它自己那侧的原文，别替任何一侧圆场）")
        out()

        turns = sorted({r.get("turn") for r in records if r.get("turn") is not None})
        if turns:
            out("每轮的记录种类（缺哪一环一眼可见）")
            for turn in turns:
                kinds = [r.get("kind") for r in records if r.get("turn") == turn]
                mark = "  <-- 这一轮没有模型响应" if "model-response" not in kinds else ""
                out(f"  turn {turn}: {' / '.join(kinds)}{mark}")
            out()

        out("逐条")
        for record in records:
            print_record(record, full)
        return 0
    out(f"经历里没有这个活动：{run_id}")
    return 1


def print_record(record: dict, full: bool) -> None:
    seq = record.get("seq")
    kind = record.get("kind")
    action = record.get("action") or ""
    turn = record.get("turn")
    head = f"  #{seq} {kind}"
    if turn is not None:
        head += f" turn={turn}"
    if action:
        head += f" action={action}"
    if kind == "ending":
        head += f" system={record.get('system')}"
        if record.get("failure"):
            head += f" failure={record['failure']}"
        elif record.get("subject"):
            head += f" subject={record['subject']}"
    out(head)
    if record.get("stage"):
        out(f"      stage={record['stage']}")
    for block in record.get("blocks", []) or []:
        label = block.get("label", "?")
        body = render_block(block, full)
        out(f"      [{label}]")
        for line in body.splitlines() or [""]:
            out(f"        {line}")
    content = record.get("content")
    if isinstance(content, str) and content:
        body = content if full or len(content) <= TEXT_LIMIT else content[:TEXT_LIMIT] + "…"
        out("      [content]")
        for line in body.splitlines():
            out(f"        {line}")
    text = record.get("text")
    if isinstance(text, str) and text:
        body = text if full or len(text) <= TEXT_LIMIT else text[:TEXT_LIMIT] + "…"
        out("      [text]")
        for line in body.splitlines():
            out(f"        {line}")
    for param in record.get("params", []) or []:
        label = param.get("label", "?")
        value = param.get("text", "")
        if not full and len(value) > TEXT_LIMIT:
            value = value[:TEXT_LIMIT] + "…"
        out(f"      param {label}:")
        for line in (value.splitlines() or [""])[:40]:
            out(f"        {line}")


def cmd_facts(home: Home, run_id: str, agent: str | None) -> int:
    for resident in home.all():
        if agent is not None and resident.agent_id != agent:
            continue
        everything = read_jsonl(resident.facts_files())
        facts = [record for record in everything if record.get("run") == run_id]
        if not facts:
            continue
        facts.sort(key=lambda record: record.get("seq", 0))
        out(f"支 {resident.label}")
        note = broken_note(broken_records(everything))
        if note:
            out(note)
        for fact in facts:
            out(json.dumps(fact, ensure_ascii=False, sort_keys=True))
        return 0
    out(f"事实里没有这个活动：{run_id}")
    return 1


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        prog="trace_view.py",
        description="只读地看她的经历 / 事实 / 宿主 wake 行（住户化与老形状都认）。",
    )
    parser.add_argument(
        "--home",
        default=os.environ.get("CELIRA_HOME_DIR", "D:/celira-home"),
        help="家的根目录（默认取 CELIRA_HOME_DIR，否则 D:/celira-home）",
    )
    parser.add_argument(
        "--agent",
        default=None,
        help="只看这一支（一个家住多支时用；不给就是全部住户）。"
        "⚠ 与 --home 同款：**写在子命令之前**（`--agent <支> list`）",
    )
    sub = parser.add_subparsers(dest="command")

    sub.add_parser("where", help="有哪些日志、多大、多新（逐支一节）")

    p_list = sub.add_parser("list", help="最近的活动（含失败分类）")
    p_list.add_argument("--n", type=int, default=20, help="每支显示多少拍（默认 20）")
    p_list.add_argument("--failures", action="store_true", help="只看非 subject 结局的")

    p_show = sub.add_parser("show", help="一次活动的逐条经历")
    p_show.add_argument("run", help="活动 id（host-<pid>-<ms>-<n>）")
    p_show.add_argument("--full", action="store_true", help="不截断正文")

    p_facts = sub.add_parser("facts", help="一次活动的耐久事实")
    p_facts.add_argument("run", help="活动 id")

    args = parser.parse_args(argv)
    home = Home(Path(args.home))

    if args.command in (None, "where"):
        return cmd_where(home)
    if args.command == "list":
        return cmd_list(home, args.n, args.failures, args.agent)
    if args.command == "show":
        return cmd_show(home, args.run, args.full, args.agent)
    if args.command == "facts":
        return cmd_facts(home, args.run, args.agent)
    parser.print_help()
    return 2


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8")
    raise SystemExit(main(sys.argv[1:]))
