#!/usr/bin/env python3
"""把一次 soak 的账本与轨迹折成成功率表。

口径分三层，不混：
  1. 执行成功率：被派发的动作，回执是 success / failure / unknown 各多少；
  2. 每个动作的分布：动作名取自轨迹里的 observed（每次真正执行一条）；
  3. 门禁：模型想调、但被拦下先看文档的次数——这不是失败。

用法：analyze.py <环境根> [...]
"""
import json
import os
import sys
from collections import Counter, defaultdict


def load(path):
    rows = []
    try:
        with open(path, encoding="utf-8") as handle:
            for line in handle:
                raw, _, _ = line.partition("\t")
                raw = raw.strip()
                if raw:
                    rows.append(json.loads(raw))
    except FileNotFoundError:
        pass
    return rows


def load_trace(root):
    """读轨迹：封存的部分在 trace.parts/，活的在 trace.ndjson。

    环境大了会封存成段；只读 trace.ndjson 会漏掉前面所有活动。
    """
    rows = []
    parts = os.path.join(root, "trace.parts")
    if os.path.isdir(parts):
        for name in sorted(os.listdir(parts)):
            if name.endswith(".ndjson"):
                rows.extend(load(os.path.join(parts, name)))
    rows.extend(load(os.path.join(root, "trace.ndjson")))
    rows.sort(key=lambda row: row.get("seq", 0))
    return rows


def main(roots):
    runs = {}
    for root in roots:
        facts = load(f"{root}/facts.ndjson")
        trace = load_trace(root)
        for row in facts:
            run = row.get("run")
            if not run:
                continue
            slot = runs.setdefault(f"{root}:{run}", {
                "ending": None, "prepared": 0, "results": Counter(),
                "observed": [], "gated": 0, "input": "", "last_content": "",
            })
            if row.get("fact") == "run-ended":
                slot["ending"] = row.get("subject")
            elif row.get("fact") == "action-prepared":
                slot["prepared"] += 1
            elif row.get("fact") == "action-result":
                slot["results"][row.get("state") or "missing"] += 1
        for row in trace:
            slot = runs.get(f"{root}:{row.get('run')}")
            if slot is None:
                continue
            kind = row.get("kind")
            if kind == "observed":
                slot["observed"].append(row)
            elif kind == "run-input":
                slot["input"] = row.get("text") or ""
            elif kind == "ending":
                slot["ending"] = row.get("subject")
            elif kind == "model-response":
                slot["last_content"] = row.get("content") or ""
            elif kind == "injection":
                for block in row.get("blocks") or []:
                    if block.get("label") == "admission" and "needs-inspection" in (block.get("text") or ""):
                        slot["gated"] += 1

    states = Counter()
    by_action = defaultdict(Counter)
    failures = Counter()
    endings = Counter()
    per_prompt = defaultdict(Counter)
    gated = 0
    prepared = 0
    answered = 0

    for slot in runs.values():
        endings[slot["ending"] or "未收尾"] += 1
        gated += slot["gated"]
        prepared += slot["prepared"]
        states.update(slot["results"])
        if slot["last_content"].strip():
            answered += 1
        for obs in slot["observed"]:
            state = obs.get("state") or "missing"
            by_action[obs.get("action") or "?"][state] += 1
            per_prompt[slot["input"] or "?"][state] += 1
            if state == "failure":
                failures[obs.get("failure") or "未标明"] += 1

    executed = states["success"] + states["failure"] + states["unknown"]
    print(f"活动 {len(runs)} 次 · 动作 {executed} 次 · "
          f"成功 {states['success']} · 失败 {states['failure']} · 未确认 {states['unknown']}")
    if executed:
        print(f"执行成功率 {states['success'] * 100.0 / executed:.1f}%")
    print(f"门禁拦下先看文档 {gated} 次（不算失败）· 派发 {prepared} 次")
    if states["missing"]:
        print(f"⚠ 派发后无回执 {states['missing']} 次")
    print("结局：" + " · ".join(f"{k}={v}" for k, v in endings.most_common()))
    print(f"最后留下正文 {answered} 次 · 没留正文 {len(runs) - answered} 次")

    print("\n按动作：")
    for action, counter in sorted(by_action.items(), key=lambda kv: -sum(kv[1].values())):
        n = sum(counter.values())
        ok = counter["success"]
        print(f"  {action:22s} 执行 {n:3d} · 成功 {ok:3d} ({ok * 100.0 / n:5.1f}%)"
              f" · 失败 {counter['failure']} · 未确认 {counter['unknown']}")

    if failures:
        print("\n失败原因：" + " · ".join(f"{k}={v}" for k, v in failures.most_common()))

    failing = [(text, c) for text, c in per_prompt.items() if c["failure"]]
    if failing:
        print("\n按请求（只列有失败的）：")
        for text, counter in sorted(failing, key=lambda kv: -kv[1]["failure"]):
            print(f"  {text[:34]:36s} 动作 {sum(counter.values()):3d} · 失败 {counter['failure']}")


if __name__ == "__main__":
    main(sys.argv[1:])
