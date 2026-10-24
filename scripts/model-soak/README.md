# 真模型 soak

量一件事：**主体自己决定调用工具时，动作的成功率是多少**。

它不是功能测试，也不是回归门禁——功能测试用替身模型、可复算；这里用真模型，
同一句话每次都可能走不同的路。所以样本要重复，结论看比率，不看单次。

## 口径

三件事分开数，不混：

1. **执行成功率** = `success / (success + failure + unknown)`，只算真正被派发的动作；
2. **按动作**：每个动词执行多少次、成功多少次；
3. **门禁**：模型想调、但被拦下先看文档（`needs-inspection`）的次数——这不是失败，
   而且下一轮会重试。

另外单列一行「最后留下正文」，用来区分"活干了但没回话"和"动作失败"。

## 跑法

```bash
SOAK_ENDPOINT=https://.../v1/chat/completions \
CELIRA_MODEL_API_KEY=... \
bash scripts/model-soak.sh
```

端点要求额外会话头、而 Agent 适配层不带时，先起本地垫片（只补头，不改请求体）：

```bash
GATEWAY_UPSTREAM=https://.../v1/chat/completions \
python3 scripts/model-soak/gateway.py &

SOAK_ENDPOINT=http://127.0.0.1:18081/v1/chat/completions \
CELIRA_MODEL_API_KEY=... \
bash scripts/model-soak.sh
```

## 参数

| 变量 | 缺省 | 说明 |
|---|---|---|
| `SOAK_ENDPOINT` | 必填 | OpenAI 兼容的 chat completions 完整地址 |
| `CELIRA_MODEL_API_KEY` | 必填 | 模型凭据；只进请求头，不落盘 |
| `SOAK_MODEL` | `deepseek-v4.1-flash` | 模型名 |
| `SOAK_REPEAT` | `1` | 请求清单重复几轮；每轮一个独立环境与材料 |
| `SOAK_MAX_TURNS` | `8` | 每次活动的轮数上限 |
| `SOAK_CONTEXT_WINDOW` | `1000000` | 声明的上下文窗口 |
| `SOAK_ROOT` | `$TMPDIR/celira-model-soak` | 账本与材料落在哪 |
| `SOAK_BIN` | `agent/target/debug/agent_run` | 不指定时自动构建 |

## 成本

15 条请求 × 1 轮 ≈ 15 次活动、9 万 token 上下；3 轮 ≈ 27 万。脚本**不会**自己跑，
也不挂夜间——要重复跑就显式给 `SOAK_REPEAT`。

## 边界

- 不接 IM：不发任何外部消息；
- 只在临时根里跑：材料、环境、账本都不进仓库；
- 不碰 `scripts/home/*`，不读生产数据；
- 凭据只在环境变量里，脚本不写、不打印、不落盘。
