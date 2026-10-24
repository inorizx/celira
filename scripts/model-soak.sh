#!/usr/bin/env bash
# 真模型 soak：用自然对话压动作面，产出一张可复算的成功率表。
#
# 边界（刻意写死在这里）：
#   - 只在自己的临时根里跑：材料、环境、账本都不进仓库；
#   - 不接 IM：不发任何外部消息；
#   - 不碰 scripts/home/*，也不读生产数据。
#
# 需要的环境变量：
#   SOAK_ENDPOINT         OpenAI 兼容的 chat completions 地址（必填）
#   CELIRA_MODEL_API_KEY  模型凭据（必填；只进请求头，不落盘）
# 可选：
#   SOAK_MODEL            模型名（缺省 deepseek-v4.1-flash）
#   SOAK_REPEAT           请求清单重复几轮（缺省 1；每轮一个独立环境）
#   SOAK_MAX_TURNS        每次活动的轮数上限（缺省 8）
#   SOAK_CONTEXT_WINDOW   声明的上下文窗口（缺省 1000000）
#   SOAK_ROOT             跑在哪个根下（缺省 $TMPDIR/celira-model-soak）
#   SOAK_BIN              agent_run 可执行文件（缺省构建 debug 版）
set -u

here=$(cd "$(dirname "$0")" && pwd)
root=${SOAK_ROOT:-${TMPDIR:-/tmp}/celira-model-soak}
repeat=${SOAK_REPEAT:-1}
model=${SOAK_MODEL:-deepseek-v4.1-flash}
window=${SOAK_CONTEXT_WINDOW:-1000000}
turns=${SOAK_MAX_TURNS:-8}
endpoint=${SOAK_ENDPOINT:?需要 SOAK_ENDPOINT：OpenAI 兼容的 chat completions 地址}
bin=${SOAK_BIN:-$here/../agent/target/debug/agent_run}

: "${CELIRA_MODEL_API_KEY:?需要 CELIRA_MODEL_API_KEY：模型凭据（只进请求头，不落盘）}"

if [ ! -x "$bin" ]; then
  (cd "$here/../agent" && cargo build -q --bin agent_run) || exit 1
fi

seed() {
  local mat=$1
  mkdir -p "$mat/work"
  printf '# Celira\n\nHuman 与 Agent 平等共处的聊天工作台。\n\n本周目标：Core IM v1 封板。\n' > "$mat/README.md"
  printf '标题：季度计划\n\n第一版：只有大纲。\n- 把地基打稳\n' > "$mat/draft.md"
  printf '2026-10-23 周会\n\n1. 琴音负责 Core IM 封板。\n2. 主人负责最终验收。\n3. 周五前给出 v2 大纲。\n4. 发布日暂定 11 月 15 日。\n' > "$mat/会议记录.md"
  printf '项目,金额\n服务器,1200\n模型调用,800\n设计,500\n杂项,300\n' > "$mat/预算.csv"
}

mkdir -p "$root"
total=0
round=1
while [ "$round" -le "$repeat" ]; do
  work="$root/round$round"
  mat="$work/materials"
  envroot="$work/env"
  rm -rf "$work"
  mkdir -p "$mat"
  seed "$mat"

  "$bin" init --root "$envroot" --agent model-soak >/dev/null || exit 1

  while IFS= read -r input; do
    case "$input" in ''|'#'*) continue ;; esac
    total=$((total + 1))
    "$bin" run --root "$envroot" --agent model-soak --materials "$mat" \
      --run "r$total" --max-turns "$turns" --create-dir work --input "$input" \
      --model "$model" --model-endpoint "$endpoint" --context-window "$window" \
      >/dev/null 2>&1
    printf '.'
  done < "$here/model-soak/requests.txt"
  printf ' 轮 %s/%s 完成\n' "$round" "$repeat"
  round=$((round + 1))
done

echo
echo "活动 $total 次；账本在 $root/round*/env"
python3 "$here/model-soak/analyze.py" "$root"/round*/env
