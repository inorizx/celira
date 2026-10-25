# Celira Core V1 Agent History Contract（解冻切片 B2：翻记录）

> 状态：**已批准窄解冻**（2026-09-09 主人三切片连批，本切片 = B2「翻记录」；
> 授权范围 = 本文件边界内的改动，边界之外的 Core 冻结面仍然冻结）。
> 本文件是 Agent 历史读取面（agent 翻看自己作为成员的历史消息）的唯一权威
> 契约：新增 Backend 纯读端点 `GET /v1/backend/rooms/{room_id}/messages` +
> Runtime 侧 `celira_read_history` 工具（Retrieve 类免门，结果注入当轮视野
> 用完即弃）的字段、可见性、分页、错误、不变量与非目标。
>
> 本切片只在下列方向放宽 Core v1 冻结，且**不**修改任何既有冻结语义：
> Completion / Backend 四端点 / Event / ACK 语义零变动；不为历史读取新增任何
> SQLite 表、列、索引或持久对象（复用人类历史路径的 `messages` 既有一切）；
> 不产生第二个公共事实写者；不改变人类历史端点 `GET /v1/rooms/{room_id}/messages`
> 的任何 wire 行为（Store 层「Room 存在但 reader 非 active member」的区分只
> 在路由层多映射回 Human 路径的既有 404）。
>
> **术语对照（2026-09-11 双系统重命名；正文冻结不动）**：本契约正文中的
> `organs::core` 与 `CELIRA_CORE_BIN` 为历史名，本仓库已分别更名
> `extensions::im` 与 `CELIRA_IM_BIN`；正文保持冻结原文，历史细节归 Git。
>
> **v1.1 附加事实（2026-10-25，已获主人批准）**：本端点在 `messages` / `next_before` /
> `request_id` 之外**附加**一个顶层 `room` 头（房间 id / kind / name + 最新简介
> `profile`），来源是 `CORE_V1_ROOM_PROFILE.md` 的群简介版本链。它是**只读附加
> 字段**：分页、消息投影、准入与错误码**一字未变**；人类历史端点不返回它。
> 本切片仍不为历史读取新增任何表/列/索引/写入。
>
> ⚠ **2026-09-28（Completion / ACK 整体退役并删除）**：本文提到的 Completion 与 ACK（旧投递模型）**已连端点带表删除**（`completions` / `agent_delivery_positions`、`POST …/events/{event_id}/completion`、`POST …/events/{event_id}/ack`、`c1.be.ack` 哨兵与相关错误码；授权与原理见 `CORE_V1_AGENT_PROTOCOL.md` 文档头）。本文凡出现它们的句子都是**这一刀当时的范围声明**（"不改 X / 不产生 X"）——那句话本身仍然成立，只是里面的对照物已经不在了；按历史基线阅读。

## 1. 目标、范围与拓扑（C4 L1–2）

### 1.1 一句话原理

Agent 能说（say 全清醒开放）、能看见自己的世界（B1 通讯录），但不能翻聊天记录——
历史消息是系统装配时给多少算多少，Agent 没有「往上滑」的手。主人愿景（「微信记录」
体感）：人的记忆 = 对话记录的有损压缩，原文住服务器、需要时翻——「我记得」vs
「我查到」诚实区分。本切片给 Agent 那只手（机械分页读取，绝不做任何语义检索——宪法禁）。

### 1.2 C4 Level 1–2 拓扑

```text
Agent（Runtime 内）
  ├─ 既有 client：Backend 四端点 + SSE 门铃 + organs::core
  └─ 新增 read_history 工具（Retrieve 类免门，全清醒类型）── 直读 Core 历史端点
            │
            ▼
Celira Core
  └─ 新增 Backend 纯读端点 GET /v1/backend/rooms/{room_id}/messages
       （认证 BackendAuth；准入 = 目标 room 的 active member；分页与消息
         公共投影与人类历史路径完全同构）
```

- 呈现（Client）只读已授权投影；公共事实（Core）唯一裁决并持久化；执行/连接
  平面不进入本切片；
- C4 增量：**零新容器、一条新读取连接**（agent 历史读取入口，纯 GET）。

### 1.3 必须保持的边界（非目标）

- **装配基底不动**：基底瘦身（历史原文从装配基底搬出、改为 Agent 主动检索）是
  Context 换代的事，等真实长任务撞 ObservationFull 触发——本切片禁止触碰任何
  装配纪律；
- 不做任何语义检索（关键词 / 向量 / RAG——宪法禁）；`read_history` 只做机械
  分页读取；
- 不做 finish 派生处置 / 跨房寻址 / Grant 地址簿 / SelfProposed / 在场平面 /
  邮箱适配器（后续切片）；
- 不改变 Completion / 四端点 / Event / ACK / 事件泵语义；不新增 room kind、
  不新建房间、不追加任何写入口。

## 2. 准入（可见性充分必要条件）

agent 可经本端点读取目标 room 的历史，当且仅当**全部**成立：

1. 认证：请求携带有效 BackendAuth（同四端点同语义：合法 Backend Credential、
   其绑定 Agent 为 active Agent）；认证失败语义与四端点完全一致（401，见 §5）；
2. 目标 room 存在（canonical UUID v4；bad-id 或未知 → 404 `room_not_found`，
   不泄露存在性——与人类路径同 404 语义）；
3. **agent 是目标 room 的 active member**（存在且 Membership 周期 active）。
   Room 存在但 agent 非 active member → **403 `not_room_member`**——对齐见
   say 端点先例（agent 已通过凭据证明身份，成员资格是明确 ACL）。

准入不检查 Service Access / Binding / Publication（普通成员读取，与 say 同律）。
⚠ **2026-09-28 服务层退役**：Agent Service 已整体删除，Service Access / Publication
这两个对照物不复存在；**规则本身一字未变**——历史读取不检查任何服务事实。
`read_history` 是 Retrieve 原子能力，**Runtime 侧不经 ActionGate**（读自己的
房间历史与 record.read 同族——免门直达；能力门是写/动作面的边界，不是读取面）。

## 3. 新增端点

### 3.1 路由

```text
GET /v1/backend/rooms/{room_id}/messages?limit=...&before=...
```

- 认证：`Authorization: Bearer <BackendAuth BackendToken>`（同四端点 BackendAuth
  语义；401 语义一致，见 §5）；
- 查询参数与人类历史端点同一 `HistoryQuery`：`limit` 可选（默认 50、范围
  1–100）、`before` 可选 opaque cursor（引用一条该 Room 内可见 Message；
  unknown / 非法 / 跨端点类型 / 跨 Room / 不可见 cursor → 400 `invalid_request`，
  不泄露存在性）；
- 响应：与人类历史端点同一消息形状——`messages` 按 sequence 升序、
  `next_before` 为 opaque 分页游标（有更旧消息时为当前页第一条 Message 的
  cursor）或 null。**同一公共投影**（MessageView：id/room_id/actor/body/
  mentions/attachments/reply_to_message_id/created_at/recalled_at）；
  **v1.1 附加**：顶层多一个 `room` 头（`{id, kind, name, profile}`，
  `profile = {description, version} | null`），每一页同值，见
  `CORE_V1_ROOM_PROFILE.md` §4.2；`messages` / `next_before` / `request_id`
  与人类端点逐字段同形；
- GET 只读：无 Idempotency-Key 要求（与人类历史端点一致）。

### 3.2 分页语义（opaque cursor）

- 初次不传 `before` → 返回最新一页（limit 条，sequence 升序）；有更旧消息时
  `next_before` 非空；
- 翻更早页：回传 `before=<next_before>` → 返回 `sequence < cursor_sequence`
  的一页；**无重复、无跳过**；
- 游标是 opaque 字符串（资源类型 `m` + UUID），客户端不做任何解析（读取/游标
  管理细节沉 Runtime 工具背面）；unknown / 跨 Room / 不可见 cursor → 400，
  不泄露存在性。

## 4. 实现归属（与人类路径同构，零新增持久面）

- 复用同一 `application::message::list_messages` 用例与同一 `MessageStore`
  分页/投影逻辑；Store 层只新增一处区分：读取授权裁决时，Room 存在但 reader
  非 active member → 类型化 `NotAMember`（后端路由出 403；Human 路由在
  `query_error_response` 把 `NotAMember` 收敛回 404 `room_not_found`——
  人类 wire 与该区分引入前逐字节一致）；
- 零 SQLite 表/列/索引新增；零写语义；不产生 Event / Completion / ACK / Live
  通知。⚠ v1.1 的 `room` 头读取 `room_profile_versions`（该表由
  `CORE_V1_ROOM_PROFILE.md` 的群简介切片拥有与写入）；历史端点本身仍不新增
  持久对象、不写任何一行。

## 5. 错误映射（稳定公共 code，统一 JSON 信封）

| code | HTTP | 场景 |
|---|---|---|
| `authentication_required` | 401 | 缺 Authorization（与四端点同） |
| `invalid_credential` | 401 | Credential 无效/已撤销（与四端点同） |
| `invalid_request` | 400 | limit 非法（0 / >100 / 非数字）、before 非法/跨端点/跨 Room/不可见 |
| `room_not_found` | 404 | bad-id / 未知 / 非 active Room（不泄露存在性，与人类路径同） |
| `not_room_member` | **403** | **Room 存在但该 agent 非其 active member**（明确 ACL，对齐 say 端点） |
| store failure | 500/503 | 既有安全错误边界（与人类路径同） |

> 403 的引入动机与 say 端点一致：agent 已通过凭据证明身份（≠ 匿名访问），
> 成员资格是明确 ACL；「非成员」与「房间不存在」必须区分（Agent 可推断该房间存在
> 但自己不在其中）。

## 6. 不变量

- I1 只增不改：Completion / 四端点 / Event / SQLite 既有表列语义零变动；人类
  历史端点的所有 wire 行为与该区分引入前一致（404 语义不因内部类型化而改变）；
- I2 读取授权是房间成员资格：本端点返回的任何消息均属于该 agent 为其 active
  member 的房间；非成员绝不可读（403）、未知房间绝不可探（404）；
- I3 分页完整性：同一游标序列翻页无重复、无跳过（每页恰好 limit 条，末页可
  < limit；前后页消息 id 零重叠）；重放任意页产生确定性同集；
- I4 同构：与本端点同级的人类历史端点返回同一消息集合（同 Room 同 reader
  同授权窗口）——同一投影、同一分页规则、同一消息视图；**唯一差异是
  v1.1 的顶层 `room` 头只出现在 Backend 面**（人类端点不返回它，消息集合
  本身仍逐条相同）；
- I5 只读：GET 无任何持久副作用、不产生 Event/通知；重复读取幂等（恒同集）。

## 7. Runtime 侧 `celira_read_history` 工具契约（D2 窄面厚背）

> 本段描述 Runtime 对工具的支持面，作为与 Core 端点的协作契约；Core 契约在
> 第 3–6 节定死，Runtime 是本端点的**唯一已批准消费方**。

- 工具名 `celira_read_history`；参数 = 目标房间 `room` + 条数 `limit`（1–50，
  超限拒绝不截断——工具面比路由上限更严，先于任何 Core 往返）+ 可选 opaque
  `before` 游标（翻更早页；缺省/null = 最新一页）；
- **Retrieve 类直达（免门）**：读自己的房间历史是 Retrieve 原子能力，不走
  ActionGate（与 `record.read` 同族）；任何清醒类型（Timer / System /
  Maintenance / 响应式 Message 唤醒）统一注入（白名单 16）；
- 读取细节（重试、分页游标管理）全部沉背面；工具结果 = 当轮 Observation：
  条数 + 逐条正文（作者 kind/display_name、时间）+ `next_before`（供 Agent 翻更早
  页），**用完即弃**（不持久进装配基底、不进线头）；
- 错误分类干净呈现（稳定词、零内容、绝不虚构正文）：非成员 → denied（403）、
  房间未知 → not found（404）、凭据失效 → unavailable（401）、分页被拒 →
  rejected（400）、存储/传输 → unavailable；
- 机械校验先于任何 Core 往返：room 非空、≤64 bytes（**标识那一族的上限**，
  `im_channel::MAX_ID_BYTES`）——超限**本地拒绝**，不走到任何往返；
  limit 1–50——超限拒绝不截断（与 ReadMessage 同裁决先例）。
  ⚠ 2026-10-13 更正两处：本行原写 **≤2048**（与 `CORE_V1_AGENT_SPEAK.md` 那句的 ≤64 说法
  不一，而两份切片描述的是**同一条**机械校验 ⇒ 2048 是抄错的数字，不是放宽的界）；
  且当时**这一条读路径没有落实这个检查**（同族的另外 14 处都有），已同刀补齐。

## 8. 测试证据与验证口径

- 行为证明来自**公开协议 / 真实进程黑盒**（真实 Core HTTP、loopback fake
  provider、禁真实 LLM/API key），不直读 SQLite 冒充证据；
- 断言面：公共 HTTP 响应（状态码/code 信封/消息视图）、opaque 分页游标序列、
  前后页零重叠、403/404/401 区分；
- 验收场景（A1–A4）：真实 Core e2e——多消息历史（升序/作者投影）、limit=2
  首页 + before 翻页零重叠、非成员 403、401 语义；
- 回归纪律：既有全部测试零回归（人类历史端点 wire 不变的直接证据）；门禁
  四件套（`cargo fmt --check` / `cargo check --all-targets` / `cargo clippy` /
  `cargo test --all-targets`）全绿；
- opt-in 真实 Core e2e 按 `CELIRA_CORE_BIN` 模式运行（loopback fixture；未注入
  CELIRA_CORE_BIN 时该测试组静默跳过，不算失败）。

## 9. 非目标（重述边界）

- 装配基底/装配纪律零改动（基底瘦身归 Context 换代，等真实长任务撞
  ObservationFull 触发）；
- 不做任何语义检索（关键词/向量/主题——宪法禁）；只做机械分页；
- 不做 finish 派生处置 / 跨房寻址 / Grant 地址簿 / SelfProposed / 在场平面 /
  邮箱适配器；
- 不新建房间、不新增 room kind、不做 Service 准入扩展、不追加任何写入口；
- Runtime `read_history` 不落入任何 Core Event 之外的本体；Retrieve 读取面无
  能力门——能力门始终只守写/动作面。