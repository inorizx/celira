#!/usr/bin/env bash
# 夜间巡检：门禁 + 动作面反复跑（找概率性报错）+ 环境体检。
#
# 为什么要有它：单测全绿只说明"这一次是绿的"。动作调用出问题往往是**概率性**的
# （并发、超时、端口、临时文件），一次跑过去看不出来。这里把动作面反复跑几轮，
# 每轮记下失败的用例名：既失败又通过的，就是"偶发"；每轮都失败的，是红的。
#
# 用法：
#   scripts/nightly-soak.sh              # 默认 3 轮
#   SOAK_ROUNDS=5 scripts/nightly-soak.sh
#
# 出来的是一份给人看的短报告（stdout）。它**不改仓库、不提交、不清理构建产物**。

set -uo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
AGENT="$ROOT/agent"
ROUNDS="${SOAK_ROUNDS:-3}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

say() { printf '%s\n' "$*"; }
line() { say "----------------------------------------"; }

cd "$AGENT" || { say "找不到 agent 目录：$AGENT"; exit 1; }

# ---- 环境体检 ---------------------------------------------------------------
disk_pct=$(df --output=pcent /data 2>/dev/null | tail -1 | tr -dc '0-9')
mem_pct=$(free | awk '/Mem:/ {printf "%d", $3 * 100 / $2}')
userns=$(cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns 2>/dev/null || echo n/a)
landlock=$(awk '/landlock/ {found=1} END {print found ? "available" : "absent"}' /proc/kallsyms 2>/dev/null || echo n/a)

say "【夜间巡检 $(date '+%Y-%m-%d %H:%M')】"

# 磁盘太满就先说话：夜里把盘写满，第二天的构建会以完全无关的报错炸掉。
if [ "${disk_pct:-0}" -ge 90 ]; then
    say "环境：/data 已用 ${disk_pct}%（低于 10% 余量）——**先清盘再跑**，本轮跳过门禁与压测。"
    exit 0
fi

# ---- 门禁（一次） -----------------------------------------------------------
line
gate_test=$(timeout 1200 cargo test 2>&1 | tee "$WORK/gate.log" | grep -E '^test result:' || true)
gate_pass=$(printf '%s\n' "$gate_test" | grep -oE 'ok\. [0-9]+ passed' | awk '{s+=$2} END {print s+0}')
gate_fail=$(printf '%s\n' "$gate_test" | grep -c 'FAILED' || true)
gate_clippy=$(timeout 600 cargo clippy --all-targets -- -D warnings 2>&1 | grep -cE '^(warning|error)' || true)
gate_fmt="ok"
timeout 120 cargo fmt --check >/dev/null 2>&1 || gate_fmt="**失败**"

say "门禁：test ${gate_pass} 通过 / ${gate_fail} 组失败 · clippy ${gate_clippy} 条 · fmt ${gate_fmt}"
if [ "${gate_fail:-0}" -ne 0 ]; then
    say "  ⚠ 失败用例："
    awk '/^failures:$/ {buf=""; flag=1; next} /^test result:/ {flag=0; printf "%s", buf} flag && /^    [^ ]/ {buf = buf "  - " $1 "\n"}' "$WORK/gate.log" | sort -u | head -20
fi

# ---- 动作面反复跑（重点：群聊与终端） --------------------------------------
# 这些是把"调用动作"整条链路跑通的用例：准入 → 准备 → 派发 → 回执 → 恢复。
# 群聊那套（im_app）覆盖：说话、翻记录、撤回、成员、读位置。
line
soak_list="im_app terminal_app file_app multi_action_turn provider_wire"

# 汇总表：<suite>|<test>|<失败次数>
: > "$WORK/failures.tsv"
: > "$WORK/totals.tsv"

for suite in $soak_list; do
    total=0
    for round in $(seq 1 "$ROUNDS"); do
        out="$WORK/${suite}-${round}.log"
        timeout 900 cargo test --test "$suite" >"$out" 2>&1
        passed=$(grep -oE 'test result: ok\. [0-9]+ passed' "$out" | awk '{s+=$4} END {print s+0}')
        failed=$(grep -c 'test result: FAILED' "$out" || true)
        total=$((total + passed))
        if [ "${failed:-0}" -ne 0 ]; then
            # ⚠ 只为**失败用例名**这一块取值：cargo 会打两次 `failures:`——第一次是
            # `---- 用例 stdout ----` 的正文（panic 原文），第二次才是名字表。取最后一次。
            awk '/^failures:$/ {buf=""; flag=1; next} /^test result:/ {flag=0; printf "%s", buf} flag && /^    [^ ]/ {print $1}' "$out" \
                | while read -r name; do
                    [ -n "$name" ] && printf '%s\t%s\n' "$suite" "$name" >> "$WORK/failures.tsv"
                done
        fi
    done
    printf '%s\t%s\n' "$suite" "$total" >> "$WORK/totals.tsv"
done

# 并发多跑一轮群聊：串行过、并发挂的，是并发敏感，不是逻辑错。
timeout 900 cargo test --test im_app -- --test-threads=8 >"$WORK/im_app-concurrent.log" 2>&1
conc_failed=$(grep -c 'test result: FAILED' "$WORK/im_app-concurrent.log" || true)
if [ "${conc_failed:-0}" -ne 0 ]; then
    awk '/^failures:$/ {buf=""; flag=1; next} /^test result:/ {flag=0; printf "%s", buf} flag && /^    [^ ]/ {print "im_app(并发)" "\t" $1}' "$WORK/im_app-concurrent.log" \
        >> "$WORK/failures.tsv"
fi

say "动作面：${ROUNDS} 轮 × [${soak_list}] + 群聊并发一轮"
while IFS=$'\t' read -r suite total; do
    say "  ${suite}：${total} 次通过"
done < "$WORK/totals.tsv"

if [ -s "$WORK/failures.tsv" ]; then
    say "  ⚠ 有失败的用例（这就是要找的\"概率性报错\"）："
    sort "$WORK/failures.tsv" | awk -F'\t' '{count[$1"::"$2]++} END {for (k in count) printf "  - %s：%d 次\n", k, count[k]}' | sort
    say "  复现：cd agent && cargo test --test <套件> <用例名> -- --nocapture"
else
    say "  ✓ 没有偶发，也没有红。"
fi

# ---- 环境读数 ---------------------------------------------------------------
line
say "环境：/data 已用 ${disk_pct}% · 内存 ${mem_pct}% · 无特权 userns 限制=${userns} · landlock=${landlock}"
say "（全绿时这就是全部；红了才展开上面那几行。）"
