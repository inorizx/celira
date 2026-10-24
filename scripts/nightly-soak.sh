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
free_gb=$(df --output=avail -BG /data 2>/dev/null | tail -1 | tr -dc '0-9')
mem_pct=$(free | awk '/Mem:/ {printf "%d", $3 * 100 / $2}')
userns=$(cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns 2>/dev/null || echo n/a)
landlock=$(awk '/landlock/ {found=1} END {print found ? "available" : "absent"}' /proc/kallsyms 2>/dev/null || echo n/a)

# 一次全量编译实测要 ~12G：曾经 68% 起步、编到一半写满，所有套件变成
# "0 个用例"，而旧版只认 FAILED ⇒ 报成绿。所以按**剩余空间**预判，不按百分比。
MIN_FREE_GB="${SOAK_MIN_FREE_GB:-14}"

say "【夜间巡检 $(date '+%Y-%m-%d %H:%M')】"

# 空间不够就先腾：agent/target 是纯构建产物，删掉只会让本轮重编一次。
if [ "${free_gb:-0}" -lt "$MIN_FREE_GB" ]; then
    say "环境：/data 仅剩 ${free_gb}G（< ${MIN_FREE_GB}G）——先清 agent/target 再跑。"
    cargo clean >/dev/null 2>&1 || true
    free_gb=$(df --output=avail -BG /data 2>/dev/null | tail -1 | tr -dc '0-9')
    disk_pct=$(df --output=pcent /data 2>/dev/null | tail -1 | tr -dc '0-9')
    say "  清理后剩余 ${free_gb}G。"
fi
if [ "${free_gb:-0}" -lt 6 ]; then
    say "环境：清理后仍只有 ${free_gb}G，放不下一次编译——本轮**红**（不是通过），先清盘。"
    exit 1
fi

# ---- 门禁（一次） -----------------------------------------------------------
line
red=0
gate_log="$WORK/gate.log"
timeout 1200 cargo test >"$gate_log" 2>&1
gate_rc=$?
gate_test=$(grep -E '^test result:' "$gate_log" || true)
gate_pass=$(printf '%s\n' "$gate_test" | grep -oE 'ok\. [0-9]+ passed' | awk '{s+=$2} END {print s+0}')
gate_fail=$(printf '%s\n' "$gate_test" | grep -c 'FAILED' || true)
gate_clippy=$(timeout 600 cargo clippy --all-targets -- -D warnings 2>&1 | grep -cE '^(warning|error)' || true)
gate_fmt="ok"
timeout 120 cargo fmt --check >/dev/null 2>&1 || gate_fmt="**失败**"

# 编译失败 / 一个用例都没跑起来，都是**空跑**，不是绿。
gate_build="ok"
if [ "$gate_rc" -ne 0 ] && [ -z "$gate_test" ]; then gate_build="**编译失败**"; fi
if [ "${gate_pass:-0}" -eq 0 ] && [ "$gate_build" = "ok" ]; then gate_build="**0 个用例**"; fi

say "门禁：test ${gate_pass} 通过 / ${gate_fail} 组失败 · clippy ${gate_clippy} 条 · fmt ${gate_fmt} · 构建 ${gate_build}"
if [ "$gate_build" != "ok" ]; then
    red=1
    say "  ⚠ 门禁没跑起来（passed=0 是空跑，不是通过）："
    grep -m5 -E '^error' "$gate_log" | sed 's/^/  - /'
fi
[ "${gate_clippy:-0}" -ne 0 ] && red=1
[ "$gate_fmt" != "ok" ] && red=1
if [ "${gate_fail:-0}" -ne 0 ]; then
    red=1
    say "  ⚠ 失败用例："
    awk '/^failures:$/ {buf=""; flag=1; next} /^test result:/ {flag=0; printf "%s", buf} flag && /^    [^ ]/ {buf = buf "  - " $1 "\n"}' "$gate_log" | sort -u | head -20
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
        # 0 个用例 = 没跑起来（编译失败/环境失败），不是"没有失败"。
        if [ "$passed" -eq 0 ] && [ "${failed:-0}" -eq 0 ]; then
            printf '%s\t%s\n' "$suite" "<没跑起来：0 个用例>" >> "$WORK/failures.tsv"
            red=1
        fi
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
elif [ "$red" -eq 0 ]; then
    say "  ✓ 没有偶发，也没有红。"
fi

# ---- 环境读数 ---------------------------------------------------------------
# 重新取一次：上面的值停在"清理之后、编译之前"，会把"编译又吃掉 12G"藏起来。
disk_pct=$(df --output=pcent /data 2>/dev/null | tail -1 | tr -dc '0-9')
free_gb=$(df --output=avail -BG /data 2>/dev/null | tail -1 | tr -dc '0-9')
line
say "环境：/data 已用 ${disk_pct}%（剩 ${free_gb}G）· 内存 ${mem_pct}% · 无特权 userns 限制=${userns} · landlock=${landlock}"
say "（全绿时这就是全部；红了才展开上面那几行。）"

if [ "$red" -ne 0 ]; then
    say "结论：**红** —— 上面有失败项，本轮不算通过。"
    exit 1
fi
