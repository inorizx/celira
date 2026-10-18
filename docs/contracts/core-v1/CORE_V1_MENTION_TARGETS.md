# Celira Core V1 Mention Targets Contract（已批准的 mention 目标放开窄切片）

> 状态：**mention 目标放开（group 房间内任意 active 成员）窄解冻切片，已由项目主人明确批准并限定范围**。本文是本切片的唯一权威文档：Mention 的**目标范围**、准入与可见性、事件语义边界、错误、不变量与非目标。`CORE_V1_IM_CONTRACT.md` §6.4 只保留指向本文的指针，不复制语义。
>
> 本切片**只放开「谁能被 @」这一个判据**：群房间里 @ 一个 active Human 成员从此是合法事实。除此之外不改写任何既有冻结语义——房间类型边界、事件语义、错误码、幂等、撤回、History 投影、Agent 侧协议全部不变。行为证明归测试，数据库守卫由 schema 与测试证明，本文只冻结公共语义。

## 1. 目标与范围

### 1.1 一句话原理

群房间的 Mention 目标从「active **Agent** 成员」放宽为「该群房间的 **active 成员**（Human 或 Agent 均可）」；@ 人成为合法事实。**房间类型边界不放开**（`direct` / `human_direct` 仍然要求 mentions 为空），**事件语义不放开**（只有 Agent 目标才产生 Agent Event）。

### 1.2 非目标（本切片明确不做）

- **不放开 `direct` / `human_direct`**：这两种房间仍然要求 mentions 为空，携带非空 mentions 一律 400 `invalid_mention`。两人房 @ 无信息量，放开将牵动 direct 的事件语义，本切片不碰；
- 不新增、不删除任何事件 kind；`@` Human **不产生**任何 Human durable event / WS 唤醒（不引入通知面）；
- 不改变发言准入：准入仍**只由房间成员资格一个裁决者**决定，`@` 不授予任何额外读取权或可见性；
- 不改 History 投影（可见性仍按读取者的成员周期窗口，被 @ 的人不会因此看到窗口外的消息）；
- 不改 Mention 的提交形态：仍与 source Message 同一事务、仍不可改不可删、仍返回规范排序后的列表；
- 不改幂等语义（Mention 集合仍参与幂等比较）；不改撤回、回复、附件任何东西；
- 不动 Agent 侧协议（`CORE_V1_AGENT_PROTOCOL.md`）、不动客户端、不动 `agent/` crate；
- 不新增错误码，不改 400 `invalid_mention` 之外的错误面。

## 2. 目标范围（权威表述）

一条 Message 的 `mentions` 是**显式唤醒意图的公共事实**，不由 Core 根据昵称或正文 `@` 猜测。每个目标必须同时满足（在提交事务内裁决）：

| 判据 | 要求 |
|---|---|
| 房间类型 | source Message 所属 Room 必须是 `group`；`direct` / `human_direct` 必须空 mentions |
| 成员资格 | 目标在该 Room 有 active Membership 周期（`active_marker = 1`）；提交事务内仍 active |
| Participant 状态 | 目标 participant 当前 `status = 'active'` |
| Participant 种类 | **Human 或 Agent 均可**（本切片放开的唯一判据） |
| 形状 | 规范小写连字符 UUID v4；目标互不相同；每条 Message 至多 64 个目标 |

- 公共视图（发送响应与 History）返回**规范排序去重后**的 mentions 列表，原样回显提交的目标集合；
- 持久化列 `mentions.target_agent_id` 是历史形状，**语义为「mention 目标 participant」**，可存 Human 或 Agent Participant ID；列名本切片不改。

## 3. 准入与可见性

- `@` 不影响任何准入裁决：能发这条消息的人仍是「该 Room 的 active 成员」这一个判据；
- `@` 不扩展可见性：被 @ 的 Human 读取 History 时，仍只看到其历次成员周期窗口内的消息；
- `@` 不产生通知：本切片不引入任何 Human 侧事件或 WS 唤醒，被 @ 的人只能通过既有的 History 读取 / 事件机制感知；
- 目标在提交事务内失效（成员周期被关闭或 participant 非 active）时，整条 Message 事务回滚。

## 4. 事件语义（不放开）

- **目标为 active Agent**：每条合法 Mention 为该 Agent 生成恰好一条 `message.created` Agent Event（与既有语义逐字一致）；
- **目标为 Human**：Message 与 Mention 照常提交，**不生成任何 Event**（既不生成 Agent Event，也不生成 Human Event）；
- group 中无 Mention 的消息不产生 Agent Event；Agent 回复不携带 Mention、不自触发。

## 5. 错误

统一 400 `invalid_mention`，不区分具体原因（不泄露目标是否存在、是否成员、是否 active）：

- 非 `group` Room 携带非空 mentions；
- 目标不是该 Room 的 active 成员；
- 目标 participant 非 active；
- 重复目标、目标不是规范 UUID v4；
- 目标数量超过 64。

**原子性**：任何一项不满足都 fail closed，整个事务回滚，不留下 Message / Mention / Event / sequence / 幂等事实；后续合法消息的 sequence 不被失败尝试占用。

## 6. 不变量

- Mention 与 source Message 同一事务提交（Message 先、Mention 后）；任何守卫失败整体回滚；
- Mention 一经提交不可 UPDATE / 不可 DELETE；`INSERT OR REPLACE` / `REPLACE` 不能绕过；
- 每条 Message 至多 64 个 Mention 目标，目标集合去重后规范排序；
- 幂等比较纳入规范 Mention 集合：同 actor + 同 key + 同 Room + 同 body + 不同 Mention 集合 → 409 `idempotency_conflict`；
- 目标集合放宽不改变「Mention 是显式结构事实」这一性质：Core 永不从正文推断 mention。

## 7. 黑盒验收清单

1. group 中 @ 一个 active **Human** 成员 → 201，响应与 History 的 `mentions` 如实回显该 Human ID，且新增 Agent Event 数为 **0**；
2. group 中 @ 非成员（Human 或 Agent）→ 400 `invalid_mention`，且无 Message / Mention / Event / sequence 副作用；
3. `direct` 与 `human_direct` 携带非空 mentions → 400 `invalid_mention`（不放开）；
4. group 中 @ active Agent 成员 → 201 且生成恰好一条 Agent Event（既有语义回归）；
5. 重复目标、非 UUID v4、超过 64 个目标 → 400 `invalid_mention`；
6. 只改 Mention 集合的同 key 重放 → 409 `idempotency_conflict`。
