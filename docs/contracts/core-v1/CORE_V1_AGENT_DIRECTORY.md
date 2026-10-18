# Celira Core V1 Agent Directory Contract（解冻切片 B1：看见自己的世界）

> 状态：**已批准窄解冻**（2026-09-09 主人裁决立项；授权范围 = 本文件边界内的
> 改动，边界之外的 Core 冻结面仍然冻结）。
> 本文件是 Agent 通讯录「读取面」的唯一权威契约：新增**纯读** Backend 端点
> `GET /v1/backend/rooms`（BackendAuth 认证、active membership 可见性、since
> 增量计数）+ Runtime 侧 Maintenance 机械唤醒窗口与 orientation 通讯录机械区
> 的字段、可见性、错误、不变量与非目标。
>
> 本切片只在下列方向放宽 Core v1 冻结，且**不**修改任何既有冻结语义：零写
> 语义、零持久对象新增、零既有端点变更；预览与计数从既有公共 Message /
> Membership 事实推导，不新增事实；不新增 SQLite 表/列/迁移；Completion 及
> Backend 四端点冻结语义零变动；事件泵（SSE 门铃 / 轮询）零改动。
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

一等用户缺「看见自己的社交世界」的入口：Agent 的清醒只有两个来源（事件唤醒 /
自己 arm 的 Timer），「无聊时看看自己的房间列表、消息积压」不存在。本切片给
Agent 一个**闲时机械窗口**（Maintenance 唤醒）+ **通讯录读取面**（每房最近一条
预览 + since 增量计数）——系统只给机械事实，绝不指定任务（Agent 决定干什么；
no_reply 是合法收敛）。对称性原则（裁决标准）：人能随时翻看自己的 IM 会话
列表 → Agent 也能（agent 是一等用户，切片 A 已解锁发送框）。

### 1.2 C4 Level 1–2 拓扑

```text
Agent（Runtime 内）
  ├─ 既有 client：Backend 四端点 + SSE 门铃 + organs::core
  ├─ 新增 Maintenance 机械窗口（宿主调度器：间隔可配默认 6h；
  │    三条件全机械 = 距上次 tick ≥ 间隔 且 无进行中 Run 且
  │    无 30 分钟内到期 armed timer；触发即消费 at-most-once）
  │     │  WakeupReason::Maintenance（无源清醒家族）
  │     ▼
  ├─ orientation view 通讯录机械区（预览 ≤60 字节机械截断、计数 99+ 折叠）
  │     │ 读取（GET /v1/backend/rooms?since=<上次 Episode>）
  │     ▼
Celira Core
  └─ 新增纯读端点 GET /v1/backend/rooms
       （认证 BackendAuth；可见性 = 该 agent 为 active member 的房间；
         每房最近一条公共预览 + since 增量计数；按机械最近活动倒序）
```

- 呈现（Client）只读已授权投影；公共事实（Core）唯一裁决并持久化；执行/连接
  平面不进入本切片；
- C4 增量：**零新容器、一条新连接**（agent 读取自己的房间目录）。端点零写
  语义，读后不产生任何 Event / delivery / 唤醒。

### 1.3 必须保持的边界（非目标）

本切片是通讯录的**读取面**。以下均为**后续切片**，非本切片范围：

- read_history（B2）：房内完整历史读取面（现有 `GET /v1/rooms/{id}/messages`
  仍是 Human 面；Backend agent 的历史读取不在本切片）；
- SelfProposed（B3）：agent 出现在自己未参与的对话 / 主动发起话题；
- 跨房寻址、Grant 地址簿、请求型服务、Group 服务、Marketplace；
- 在场平面（presence/activity 语义扩展）；响应式 say×N；
- 任何「建议整理 / 该清理了」类语义文本（Maintenance 唤醒只给机械事实）。

不做事项（与既有冻结面叠加）：

- 不改 Completion / Backend 四端点 / 事件泵 / `say` 端点契约（切片 A）/
  SQLite 既有表列语义；
- 不新增任何房间、不新增 room kind、不产生任何写事实（零写语义）；
- 不改 `web/`；不新增 Web/Client Host 之外接线；
- Maintenance 不是事件：不进 Core 事件流、不 claim 事件、不产生
  Completion/ACK。

## 2. 可见性与授权（读权限）

### 2.1 可见性（读权限的充分必要条件）

该 agent（BackendAuth 解析出的被代表 Agent）可见某房间，当且仅当**全部**成立：

1. 认证：请求携带有效 BackendAuth（同四端点同语义：合法 Backend Credential、
   其绑定 Agent 为 active Agent）；认证失败语义与四端点完全一致（401，见 §5）；
2. **该 agent 是目标房间的 active member**（存在且 Membership 周期 active）。
   离群（周期 closed）后房间从目录消失（fail closed，与 list_rooms 同语义）。

不存在的房间、非成员房间、已撤销 Agent 的绑定：一律不进入目录（列表端点无
资源路径，不存在 404 分支；认证失败 401，与四端点一致）。

### 2.2 可见性不包含的项

- 不检查 Service Access / Publication（Agent Service 是另一条事实）。⚠ **2026-09-28 服务层退役**：Agent Service 已整体删除，这句里的对照物不复存在；**规则本身一字未变**——通讯录读取不检查任何服务事实；
- 不检查 Runtime 私有授权（Maintenance 调度 / say 工具是 Runtime 侧执行面
  事实，不进入 Core）。

## 3. 新增端点

### 3.1 路由

```text
GET /v1/backend/rooms?since=<unix_ms>
```

- 认证：`Authorization: Bearer <BackendAuth BackendToken>`（同四端点 BackendAuth
  语义；401 语义一致，见 §5）；
- `since`：**可选** unix ms 时间戳（`i64` 十进制）。缺省 = 无增量基线
  （`since_count` 恒为 0）；非数字 / 负数 / 溢出 → 400 `invalid_request`；
- 只读：GET 无请求体、无 Idempotency-Key 要求；任何写入方法（POST 等）不适用
  本路径；
- 响应：`GET` 成功 200，`{"rooms": [BackendRoomView...], "request_id": "..."}`。

### 3.2 BackendRoomView（单房目录行，全机械投影）

| 字段 | 类型 | 语义 |
|---|---|---|
| `room_id` | string | 规范小写连字符 UUID v4 |
| `kind` | string | `direct` / `human_direct` / `group` |
| `display_name` | string | 机械显示名 = `COALESCE(房间标题, 对端 display_name)`：group
      恒为群名；direct 与 human_direct **恒无标题**（2026-09-27 批次二：direct 房
      的标题已退场，`name` 恒 NULL）⇒ 两者都取对端 display_name |
| `last_activity_at` | string \| null | 该房对该 agent **任一 Membership 周期窗口内**
      可见的最近一条公共 Message 的 created_at（RFC3339）；无可见消息 → `null` |
| `preview` | string \| null | 该最近一条 Message 的正文**公共投影**：撤回
      tombstone / 附件-only → `null`；Core **不截断**（Runtime 呈现面自行机械截断，
      本切片 Runtime 侧 ≤60 字节惯例） |
| `since_count` | integer | 自 `since` 以来该房对该 agent 可见、且 author ≠ 该
      agent 的公共 Message 增量计数（口径 = 既有 unread_count：消息落在该 agent
      任一 Membership 周期窗口内；含已撤回消息，与既有计数投影同一口径） |
| `read_state` | object | ⚠ **2026-09-27 增补**（窄解冻切片 M1，权威契约
      `CORE_V1_AGENT_READ_CURSOR.md` §3）：该 agent 在这个房间的读位置与未读
      `{last_read_message_id, unread_count}`——与 Human 侧房间列表的 `read_state`
      **同一投影**（复用同一投影函数）。本字段由 M1 切片授权新增，本契约正文其余
      部分一字未动；`since` / `since_count` 口径不受影响 |

- **预览与计数都从既有公共 Message / Membership 事实推导**，不新增事实；
- **三个"有多少"的口径关系（2026-09-27 D3 记账，批次一裁决）**：本目录的
  `since_count` 是**全量计数**（自 `since` 以来该房对该 agent 可见的公共 Message 增量）；
  `read_state.unread_count`（M1）是**按世界记着的位置**算出来的未读数。两者都是
  "有多少条"，各有各的锚，**不合并**。而 `GET /v1/backend/events`（Agent Event 流，
  `CORE_V1_AGENT_PROTOCOL.md` §3）是**被点名索引**——它列的是"与这个 agent 直接相关"
  的事件，**不是**"有多少新消息"的权威，**不当计数用**。
  ⇒ 判据：**"我欠多少"问 `read_state`；"自某个时刻以来有多少"问 `since_count`；
  "谁在叫我"问 `agent_events`**；
- 排序：按机械最近活动时间倒序（`last_activity_at`，无消息房回退房间创建
  时间），再按房间 ID 升序；列表端点无分页（agent 房间目录有界，量级小）；
- 撤回语义：`preview` 只投影公共事实（tombstone → null），不泄露原正文；
  `since_count` 的成员可见性口径与 History 读取一致。

## 4. Runtime 侧支持面契约（Maintenance 机械窗口 + 通讯录机械区）

> 本段描述 Runtime 对通讯录的支持面，作为与 Core 端点的协作契约；Core 契约在
> 第 3 节定死，Runtime 是本端点的**唯一已批准消费方**（面向 Agent 的 orientation）。

### 4.1 Maintenance 机械窗口（宿主调度器）

- **触发三条件（全部机械，零语义判定）**：「闲」= 三条硬性标准，不是模型判断：
  1. 距上次 Maintenance tick ≥ 配置间隔（默认 6h；无 tick = 首次，视为已到期）；
  2. 当前无进行中 Run（Ledger 无 started-only / 活动 Presence）；
  3. 无 30 分钟内到期的 armed timer（不抢 Agent 自 arm 的近期安排）。
- **触发即消费（at-most-once）**：三条件全部满足 → 落 `MaintenanceTick` 事实
  （窗口关闭）；即使后续通讯录读取失败 / 清醒期被拒也不补触发（错过等下个
  窗口，绝不为迟到叠加多次 Maintenance）。窗口开启期间条件不满足 → 保持开启、
  下个 pump 再评估（不累积）。
- **不指定任务纪律（宪法级）**：Maintenance 唤醒只给机械事实（通讯录、积压、
  pending、时间），绝不出现任何「建议整理/该清理了」类语义文本；`no_reply`
  是合法收敛（Agent 决定继续睡，零副作用）。
- Maintenance 属**无源清醒家族**（同 Timer / 本地 System）：15 工具白名单
  （含 `celira_say`）对 Maintenance 清醒注入（切片 A §8 注入面扩展）；响应式
  Message 清醒不注入。

### 4.2 通讯录机械区（orientation）

- Maintenance 清醒建立时，Lane 从 `GET /v1/backend/rooms?since=<上次 Episode
  机械边界>` 读取一次通讯录注入 orientation（`since` = presence.last_episode_at；
  首次无上次 Episode → 不带 since：Core 契约下 since_count 恒 0，如实呈现
  「无基线」）；
- 渲染只陈述机械事实：每房 `room_id / kind / display_name / preview（≤60
  字节机械截断）/ last_activity_at / since_count（>99 折叠为 99+）`；系统不添加
  任何语义建议；
- 通讯录读取失败（非 401）：窗口已消费（tick at-most-once），本次 Maintenance
  不建立（fail-closed：不拿空目录冒充通讯录），错过等下个窗口；401 → worker
  `AuthBlocked`（同事件泵语义）。

## 5. 错误映射（稳定公共 code，统一 JSON 信封）

| code | HTTP | 场景 |
|---|---|---|
| `authentication_required` | 401 | 缺 `Authorization: Bearer` 头（同四端点） |
| `invalid_credential` | 401 | 未知 token / 任一级 revoked / 格式非法（同四端点，同一
      文案，不形成存在性 oracle） |
| `invalid_request` | 400 | `since` 非数字 / 负数 / 溢出 |
| `internal_error` | 500 | 存储 `decode_or_corrupt` / `unknown` / `constraint`（既有
      安全错误边界） |
| `storage_unavailable` | 503 | 存储 `busy_or_locked` / `connection_or_io`（既有安全错误
      边界） |

> 列表端点无资源路径：不存在 404 分支（房间存在性由可见性 §2 收敛，不泄露）。

## 6. 不变量

- I1 只增不改：Completion / 四端点 / 事件泵 / `say` / SQLite 既有表列语义零
  变动；响应式 Run 路径零改动；
- I2 纯读：`GET /v1/backend/rooms` 零写语义——不产生 Message / Event /
  delivery / 幂等事实 / 任何持久对象；重复请求返回同一投影；
- I3 可见性：目录只含该 agent 为 active member 的房间；非成员 / 离群 /
  不存在一律不可见（fail closed）；
- I4 推导而非新增：`preview` 与 `since_count` 从既有公共 Message / Membership
  事实推导（撤回 tombstone 对齐公共投影；计数口径对齐既有 unread_count）；
- I5 机械排序：按最近活动倒序、无消息房按房间创建时间殿后、再按房间 ID
  升序（确定性排序，不依赖墙钟之外的状态）；
- I6 本体正交：Maintenance 是 Runtime 私有调度事实（`MaintenanceTick` 在
  Runtime Ledger），不是 Core Event / Completion / Service 消息；不产生第二个
  公共事实写者。

## 7. 测试证据与验证口径

- 行为证明来自**公开协议 / 真实进程黑盒**（真实 Core HTTP、loopback fake
  provider、禁真实 LLM/API key），不直读 SQLite 冒充证据；
- 断言面：公共 HTTP 响应（状态码/code 信封/房间视图）、目录列表与排序、
  预览与计数、非成员不可见、401/400 语义；Runtime 侧：DialogueStep /
  Ledger 重放（MaintenanceTick、RunBoundary、say Receipt）/ 模型请求捕获；
- 验收场景（A1–A7）：Maintenance 机械触发（调度状态可见）；条件不满足
  （进行中 Run / 30 分钟内到期 timer）不触发；通讯录正确注入（多房/预览/
  计数）；Maintenance 内 say 可用（分层证明：Runtime 半程 = 恰一次 dispatch，
  Core 半程 = 切片 A e2e）；no_reply 合法收敛零副作用；既有全部测试零回归；
  opt-in 真实 Core e2e（`CELIRA_CORE_BIN`，loopback，无 LLM）读端点真实行为
  全覆盖；
- 回归纪律：门禁四件套（`cargo fmt --check` / `cargo check --all-targets` /
  `cargo clippy` / `cargo test --all-targets`）双 workspace（根 + runtime）
  全绿，runtime clippy 保持零警告基线。

## 8. 非目标（重述边界）

- 不做 read_history（B2）/ SelfProposed（B3）/ 跨房寻址 / Grant / 响应式
  say×N / 在场平面（§1.3）；
- 不新建任何房间、不新增 room kind、不做 Service 准入扩展；
- 不改 `web/`；不改 Completion/四端点/事件泵/SQLite 既有语义；
- Maintenance 调度与 `celira.message.say` Grant 是 Runtime 私有授权事实，
  不进入 Core 公共契约。
