# Celira Core V1 Agent Speak Contract（解冻切片 A：房内主动说话）

> 状态：**已批准窄解冻**（2026-09-09 主人明确裁决「解冻」「出吧」；
> 授权范围 = 本文件边界内的改动，边界之外的 Core 冻结面仍然冻结）。
> 本文件是 Agent 发送框「主动说话」面的唯一权威契约：新增 Backend 端点
> （agent 以成员身份向已属房间主动发起公共 Message）+ Runtime 侧 `say` 工具
> （窄面厚背；2026-09-09 第二刀前奏：响应式开放后**全部清醒类型注入**）的字段、
> 可见性、写权限、幂等、错误、不变量与非目标。
>
> 本切片只在下列方向放宽 Core v1 冻结，且**不**修改任何既有冻结语义：
> Completion 及 Backend 四端点冻结语义零变动；响应式 Run（事件驱动）路径零
> 改动；不新增 SQLite 表/列（幂等持久化面与人类 message.create 路径现状
> 完全对齐，见 §6）；Message / Human Event / Agent Event / Completion /
> ACK / Membership / Permission / Binding / Credential 均不被本切片新增产生
> 或改写——新端点只产生与人类 `POST /v1/rooms/{room_id}/messages` 完全同构
> 的一行公共 Message（author = 发起该消息的 agent）。
>
> 实现细节归代码、行为证明归测试；本文冻结公共语义。历史过程归 Git；未来无
> 真实消费者的提案不进入本文件。
>
> **术语对照（2026-09-11 双系统重命名；正文冻结不动）**：本契约正文中的
> `organs::core` 与 `CELIRA_CORE_BIN` 为历史名，本仓库已分别更名
> `extensions::im` 与 `CELIRA_IM_BIN`；正文保持冻结原文，历史细节归 Git。
>
> ⚠ **2026-09-28（Completion / ACK 整体退役并删除）**：本文提到的 Completion 与 ACK（旧投递模型）**已连端点带表删除**（`completions` / `agent_delivery_positions`、`POST …/events/{event_id}/completion`、`POST …/events/{event_id}/ack`、`c1.be.ack` 哨兵与相关错误码；授权与原理见 `CORE_V1_AGENT_PROTOCOL.md` 文档头）。本文凡出现它们的句子都是**这一刀当时的范围声明**（"不改 X / 不产生 X"）——那句话本身仍然成立，只是里面的对照物已经不在了；按历史基线阅读。

## 1. 目标、范围与拓扑（C4 L1–2）

### 1.1 一句话原理

一等用户缺发送框：agent 已被授予 client（Backend 四端点 + SSE 门铃 + Runtime
`organs::core`），但发送框被 Completion 冻结焊死——只能应答唤醒事件（恰好一条），
不能主动说话。本切片解「闹钟悖论」：Timer 唤醒 Agent → Agent 想在 Direct Room 提醒主人 →
当前不可能（无说话通道）。对称性原则（裁决标准）：人能做的 Agent 能做、不多不少——人在
IM 里能随时在群里发言 → Agent 在自己是成员的房间里应能主动发言。

### 1.2 C4 Level 1–2 拓扑

```text
Agent（Runtime 内）
  ├─ 既有 client：Backend 四端点 + SSE 门铃（events.available）+ organs::core
  └─ 新增 say 工具（全部清醒类型注入）── ActionGate celira.message.say Grant
            │
            ▼
Celira Core
  └─ 新增 Backend 端点 POST /v1/backend/rooms/{room_id}/messages
       （认证 BackendAuth；准入 = 目标 room 的 active member；
         落公共 Message，author = 该 agent）
            │
            ▼ post-commit 全复用既有管道（零新增路由）
  ├─ 人类侧：既有 WS push（Human Event message.created + Live notify）
  └─ 房内其他 agent：既有 message.created Agent Event 路由（SSE 门铃/轮询）
```

- 呈现（Client）只读已授权投影；公共事实（Core）唯一裁决并持久化；执行/连接平面
  不进入本切片；
- C4 增量：**零新容器、一条新连接**（agent 主动写入口）。消息落房间后全部复用
  既有推管道与 Event 投递。

### 1.3 必须保持的边界（非目标，D5 过渡态声明）

本切片是发送框的「主动说话」面。以下均为**后续切片**，非本切片范围：

- finish 派生处置（finish 结果向房间广播/汇报的派生规则；多条 say/回复间的
  自动编排策略）；
- 跨房寻址、Grant 地址簿（按 Grant 反查可写的房间集合）；
- SelfProposed（agent 出现在自己未参与的对话）；
- 在场平面（presence/activity 语义扩展）；
- 邮箱适配器及其他外部渠道桥接。

不做事项（与既有冻结面叠加）：

- 不改 Completion / Backend 四端点 / 既有 Event 语义与 SQLite 既有表列语义；
- 不做任何跨房自动广播、转发、代发；agent 只能向自己已是 active member
  （Membership 周期 active）的房间发言；
- 不改 `web/`；不新增任何 Web/Client Host/Runtime 之外的接线；
- 不新增 room kind；不新建任何房间——`say` 只能写入**已存在且该 agent 是
  active member** 的房间。

## 2. 写权限与准入（ACL）

### 2.1 准入（写权限的充分必要条件）

agent 可向目标 room 主动发言，当且仅当**全部**成立：

1. 认证：请求携带有效 BackendAuth（同四端点同语义：合法 Backend Credential、
   其绑定 Agent 为 active Agent）；认证失败语义与四端点完全一致（401，见 §8）；
2. 目标 room 存在（canonical UUID v4；bad-id 或未知 → 404，不泄露存在性）；
3. **agent 是目标 room 的 active member**（存在且 Membership 周期 active）。
   Room 存在但 agent 非 active member → **403 not_room_member**——agent 已通过
   凭据证明身份，成员资格是明确 ACL（本切片不对不存在的房间与已有的非成员身份
   施加额外的存在性保护，与人类路径的 404 语义区分开）；
4. 消息体/提及/回复/附件形状合法（与人类 `message.create` 同一校验与错误语义）。

### 2.2 准入不包含的项

- 不检查 Service Access / Binding / Publication：本切片是普通成员发言，不经过
  Agent Service 准入（Agent Service P0 的 Service Direct Room 准入是另一条事实，
  两者互不替代）。⚠ **2026-09-28 服务层退役**：Agent Service 那一支已整体删除
  （Publication / Access / 服务房），这句里的对照物不复存在；**本句要说的那条规则
  一字未变**——本切片的发言准入不检查任何服务事实；
- 不检查 Runtime 私有授权：`celira.message.say` Grant 是 **Runtime 侧** ActionGate
  执行面的事实，不进入 Core——Core 端点以「认证的 agent + 房间 active member」
  为唯一写权限，不感知也不依赖 Runtime 能力线。

## 3. 新增端点

### 3.1 路由

```text
POST /v1/backend/rooms/{room_id}/messages
```

- 认证：`Authorization: Bearer <BackendAuth BackendToken>`（同四端点 BackendAuth
  语义；401 语义一致，见 §8）；
- 请求体：与人类 `POST /v1/rooms/{room_id}/messages` 同一 `SendMessageRequest`
  形状（`body` 可空 / `mentions` / `reply_to_message_id` / `attachments` 可选；
  body 与 attachments 至少一个）；
- 请求体上限：与人类 Message 路由一致（128 KiB 路由上限；正文 16384 UTF-8
  bytes 边界由应用层裁决）；
- 响应：与人类发送响应同一 `SendMessageResponse` 形状（含 `message` 公共视图、
  `idempotency_replayed`、`request_id`）；首次创建 **201**、同 key 幂等重放
  **200**。

### 3.2 幂等（Idempotency-Key）

- 请求**必须**携带 `Idempotency-Key` 头：缺失 → 400 `idempotency_key_required`；
  格式非法 → 400 `invalid_idempotency_key`（与人类路径同解析规则）；
- 幂等键作用域 = `(actor_id, idempotency_key)`，actor_id = 认证 agent 的
  Participant ID；
- 同 actor + 同 key 重放，若请求摘要与首次一致（同 room、同 body、同
  mentions、同 reply target、同有序附件集合）→ 返回首次创建的同一 Message
  （200 `idempotency_replayed: true`），**不重复落账**；
- 同 actor + 同 key、不同摘要 → 409 `idempotency_conflict`，**不产生任何新事实、
  不修改首次事实**；
- 不同 actor 使用同一 key 互不干扰（key 以 actor 为全局作用域，与人类路径一致）。

## 4. 幂等持久化面（与人类路径现状对齐，绝不超出）

**核实的人类 message.create 路径现状（本切片如实承接，不新增持久化面）：**

- 幂等记录**不单独建表**：公共消息行 `messages` 自身携带 `idempotency_key` 列，
  数据库唯一约束为 `(actor_id, idempotency_key)`——同一 actor 同一 key 至多
  一行公共消息，行即幂等事实（消息行自身的内容即写后不可变摘要）；
- 首次提交与幂等判定在**同一个数据库事务**内完成：读取既有事实 → 无则插入
  新消息行（含 mentions/attachments/reply 绑定），有则按摘要比较返回重放或
  冲突；失败时不留下任何部分事实；
- 撤回不影响幂等事实：撤回后（公共投影呈 tombstone）同 key 同摘要重放仍按
  原内部事实比较与返回（不泄原正文）。

**新端点幂等持久化面 = 与人类路径逐字相同**：复用同一 application 用例
（`application::message::send_message`）+ 同一 MessageStore 事务 + `messages`
表既有列；不新增 SQLite 表、列、索引或任何独立幂等记录对象。幂等纪元（epoch）
等未来提案不在本切片。

## 5. 可见性（消息的公共事实）

`say` 落成的公共 Message 与人类 `message.create` 的公共 Message 完全同构：

- **author** = 认证 agent 的 Participant 公共事实（id = agent id，kind=`agent`，
  display_name/status 取其当前公共投影）；
- 房内**所有成员**（Human 与 Agent 同构）经既有读取面可见：`GET
  /v1/rooms/{room_id}/messages` 历史、消息引用/回复、撤回语义均与人类路径
  无差异；
- **人类侧**：post-commit 经既有 Human Event `message.created` + WS push 与
  Live notify 推送到房内 Human（与人类发送路径同一条 best-effort 唤醒纪律：
  失败只记录脱敏 warn，不影响已提交响应）；
- **房内其他 agent**：post-commit 经既有 Agent Event `message.created` 路由——
  Backend SSE 门铃（`events.available`）与轮询均可观察到，并可被唤醒（A4）。
  `view` 语义、静默规则与既有 Agent Event 一致；
- 对**非成员**：不可读、不可见（既有 rooms/messages 权限面不变）；
- 撤回：agent 本人可经既有 Backend 撤回路由撤回自己的 `say` 消息（契约
  `CORE_V1_IM_CONTRACT.md` §7，`DELETE /v1/backend/rooms/{room_id}/
  messages/{message_id}`），不新增撤回语义。

## 6. 错误映射（稳定公共 code，统一 JSON 信封）

| code | HTTP | 场景（本端点 vs 人类路径） |
|---|---|---|
| `invalid_request` | 400 | 非法 JSON / 非法房间 ID 形状（与人类路径同 400） |
| `payload_too_large` | 413 | 请求体超 128 KiB（与人类路径同） |
| `idempotency_key_required` | 400 | 缺 Idempotency-Key（与人类路径同） |
| `invalid_idempotency_key` | 400 | Idempotency-Key 格式非法（与人类路径同） |
| `invalid_message_body` | 400 | 正文非法 / 超 16384 UTF-8 bytes / 禁 NUL（与人类路径同） |
| `invalid_mention` | 400 | 提及非法（与人类路径同） |
| `invalid_reply_target` | 400 | 回复目标非法（与人类路径同） |
| `invalid_attachment` / `attachment_not_found` | 400/404 | 附件非法/未知（与人类路径同） |
| `service_not_accepting` | 409 | 目标房间为 Service Direct Room 且服务准入拒绝（**只可能在
      这一区发生**：`application::message` 对 Service Room 的既有准入未变） |
| `room_not_found` | 404 | 房间 bad-id / 未知 / 无 active Membership 周期外形（不泄露存在性；
      **与人类路径同语义**） |
| `not_room_member` | **403** | **本端点新增区分**：房间存在但该 agent 不是其 active member（明确
      ACL 拒绝）；对齐既有 403 类语义（如 `forbidden` 类别的稳定信封） |
| `idempotency_conflict` | 409 | 同 key 异摘要（与人类路径同） |
| store failure | 500/503 | 既有安全错误边界（与人类路径同） |

> 403 的引入动机：agent 已通过凭据证明自身身份（≠ 匿名访问），成员资格是明确
> ACL；把「非成员」与「房间不存在」合并为 404 会剥夺 agent 的排错能力（Agent
> 可推断该房间存在但自己不在其中）。人类路径保持既有 404 语义不变。

## 7. 不变量

- I1 只增不改：Completion / 四端点 / Event / SQLite 既有表列语义零变动；响应式
  Run 路径零改动；
- I2 写权限是房间成员资格：agent 发表的每条 `say` 消息其 author 恒为该认证
  agent，且该 agent 在写入时是该 room 的 active member；不存在非成员代发/越权；
- I3 幂等：同一 `(actor_id, idempotency_key)` 至多一条公共消息，同 key 重放
  恒返回同一 Message；同 key 异摘要恒 409 且零新事实（重放 ≠ 重新生成，绝不因
  重放重复公共消息）；
- I4 一致性：`say` 消息与人类 `message.create` 消息在房间内完全同构（顺序、
  可见性、提及、回复、附件、撤回、read state 语义一致）；
- I5 路由复用：post-commit 的 Human push 与 agent Event 投递走既有管道，不新增
  第二条路由逻辑；
- I6 本体正交：`say` 是普通公共 Message，不是 Event、Completion、Service 消息
  或 Runtime 私有状态；不产生第二个公共事实写者。

## 8. Runtime 侧 `say` 工具契约（D4 窄面厚背）

> 本段描述 Runtime 对工具的支持面，作为与 Core 端点的协作契约；Core 契约在第
> 3–7 节定死，Runtime 是本端点的**唯一已批准消费方**。

工具名与能力线：

- 能力线：`celira.message.say`（ActionGate Grant 精确匹配，对齐
  navigate/timer/unbind 先例）；未授权 → 受控拒绝（Denied，零写入零调用）；
- 工具名 `celira_say`；参数 = 目标房间 + 正文；**幂等键与投递细节全部沉背面**：
  - 幂等键：真实工具调用 → `ToolCall.id`（Runtime 注入 request envelope，
    Core 端点 `(actor_id, key)` 唯一）；脚本决策（无来源）→ run 指针合成稳定键；
  - 工具结果只回「已提交」类状态：Success=已提交、Failure=目标房间拒绝接收、
    Unknown=提交结果未确认（不会自动重发；同 key 幂等兜底）；
- 注入面：**全部清醒类型统一注入 15 工具白名单**（含 `celira_say`）——Timer /
  本地 System / Maintenance 机械窗口 / **响应式（Message）唤醒**均注入（2026-09-09
  第二刀前奏：响应式开放、表达自由统一——不再存在「无 say 的清醒」；执行面不再
  按清醒类型分流）；
- 机械校验先于任何 Core 往返：room 非空、≤64 bytes（**标识那一族的上限**——
  权威在 agent 侧的 `im_channel::MAX_ID_BYTES`，房间/参与者/消息目标共用）；
  body 非空、≤16384 UTF-8 bytes、禁 U+0000——超限拒绝不截断（与 Core MessageBody 同一裁决）。
  ⚠ 2026-10-13：`CORE_V1_AGENT_HISTORY.md` 那一行原写 ≤2048，与本节说法不一，
  已按本节对齐——两份切片描述的是**同一条**机械校验；同刀还补齐了历史那条读路径上
  **漏掉的这个检查**（本节这一条本来就对）。
- **处置派生（第二刀，2026-09-09 表达自由辩论定案）**：本清醒期已通过
  `celira_say` 发出公共消息（账本 append 序确定性判定）时，`finish` 不接受
  `expression`——受控拒绝（零收敛、零新事实，为模型注入指引后继续回合；
  以 `finish{}`/`no_reply` 收敛）。事件收尾走 no_reply 形状 Completion；公共
  足迹 = say 消息。结构不变量：有 say 的 Run 公共足迹只能是 say，无 say 的 Run
  公共回复只能是 reply——双轨在结构上不可能。
- **表达三通道最终形状（静默收敛，2026-09-10 六评审全票）**：工具 = 公共（表达
  工具 `core_say` / `email_send` 与显式 `finish{expression}` 落公共足迹）、
  心声 = 私有（零工具纯文本输出随 frame 的 assistant_content 完整入账、绝不落
  房）、quiescent = 自然停机（零工具完整回合收敛为 no_reply 形状，复用
  `completed(None)` 派生路径）。

## 9. 测试证据与验证口径

- 行为证明来自**公开协议 / 真实进程黑盒**（真实 Core HTTP/WS、loopback fake
  provider、禁真实 LLM/API key），不直读 SQLite 冒充证据，不引用内部 store
  抽样替代公共行为断言；
- 断言面：公共 HTTP 响应（状态码/code 信封/消息视图）、历史读取、WS 推送与
  SSE 门铃事件、幂等重放与冲突、403/404 区分；
- 验收场景（A1–A4）：Timer 唤醒 → Run 内 say 到 Direct Room → 人类侧经真实
  Core HTTP/WS 读到该条 Message（author = Agent）；同 tool-call-id 重试 → 恰好
  一条；非成员房间 say → 403 拒绝；双 agent 房间 A 的 Timer Run 内 say → B 经
  门铃/轮询收到 `message.created` 并可被唤醒；
- 回归纪律：既有全部测试零回归（响应式路径零改动的直接证据）；门禁四件套
  （`cargo fmt --check` / `cargo check --all-targets` / `cargo clippy` /
  `cargo test --all-targets`）全绿；
- opt-in 真实 Core e2e 按 `CELIRA_CORE_BIN` 模式运行（loopback fixture；未注入
  CELIRA_CORE_BIN 时该测试组静默跳过，不算失败）；
- 协议历史核对：人类 `message.create` 幂等现状（§4）以本次实现核验为准；若未来
  人类路径改变幂等持久化面，本节契约随人类路径同步变更（不构成对本切片的单方
  改动）。

## 10. 非目标（重述边界）

- 不做 finish 派生处置（finish 结果向房间广播/汇报的派生规则；多条 say/回复间
  的自动编排策略）/ 跨房寻址 / Grant 地址簿 / SelfProposed / 在场平面 / 邮箱
  适配器（§1.3）；
- 不新建任何房间、不新增 room kind、不做 Service 准入扩展；
- 不改 `web/`；不改 Completion/四端点/Event/SQLite 既有语义；
- Runtime `say` 不落入任何 Core Event 之外的本体；`celira.message.say` Grant
  是 Runtime 私有授权事实，不进入 Core 公共契约。