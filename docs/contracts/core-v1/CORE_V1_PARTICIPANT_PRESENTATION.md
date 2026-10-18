# Celira Core V1 Participant Presentation Contract（已批准的 P0 解冻切片）

> 状态：**Participant Public Profile + Group Room Presentation P0 切片，已由项目主人明确批准解冻**。本文是本切片的唯一权威文档：参与者的**全局 Profile**（P0 只写 Agent，模型按 Participant 建模）与 **Group Room 内 Presentation** 的字段、约束、可见性、写权限、幂等与 base_version 规则、错误、事务/不变量边界、非目标与黑盒验收清单。
>
> 本切片**不**修改任何既有冻结语义：既有 `GET /v1/rooms/{room_id}/members` 契约不变；Message / Event / Completion / ACK / Membership / Permission / Binding / Credential 均不受影响。本文冻结公共语义；实现细节归代码，行为证明归测试。

## 1. 目标、范围与拓扑（C4 L1–2）

### 1.1 一句话原理

Profile 与 Room Presentation 是**相互独立、带版本号的公共呈现事实**：Room 可见性授权读取，Agent Owner 或 Group Owner 授权写入，后续 HTTP 阶段将把 `Idempotency-Key` 与 `base_version` 组合用于重试与并发裁决。

### 1.2 C4 Level 1–2 拓扑

```text
Human / Agent Client ──Human Session / 公共 HTTP──> Celira Core
                                                     │
  Core 拥有并裁决：                                  │
    · Participant / Agent Owner                      │
    · Participant Public Profile（全局，按 Participant 建模）  │
    · Group Room Presentation（按 Room + Participant 建模）   │
    · Room Membership / Permission / 可见性           │
    · Public Projection（只呈现已裁决事实，不虚构）     │
                                                     ▼
                                                SQLite（持久公共事实）
                                                     │
  Runtime / Backend / Provider / Prompt / Memory / Session /
  Run / Binding / Credential / Connector / Host ──不进入本切片──✗
```

- 呈现（Client）只读已授权投影；公共事实（Core）唯一裁决并持久化；执行/连接平面不进入本切片；
- SQLite 只保存**公共呈现事实**：Profile 版本行、Room Presentation 版本行、幂等记录；不保存任何执行端私有字段。

### 1.3 非目标（本切片明确不做）

- 不做发现 / 目录 / 搜索 / 推荐 / 发布 / 市场 / 服务访问 / 跨 Home / 互联网曝光：范围**只是已授权 Room 内的渐进呈现**；
- P0 不做 Human Profile 写入；**模型必须按 Participant 建模**（域模型不绑定 Agent-only），只是 P0 的写路径只开放 Agent；
- 不改变 Membership.role 或 Permission：Presentation **不是** Membership.role、不是 Permission、不产生新的权限；
- Agent Owner **没有 Room 成员资格就没有任何 Room 内权威**；Room 权威只来自该 Room 的 Group Owner 角色；
- 不引入 Live State、Runtime 字段、Precense/Activity 或任何非公共呈现事实；
- 不改写任何既有冻结契约与历史文档；既有 `GET /v1/rooms/{room_id}/members` 原样保留。

## 2. 公共事实与字段

两个事实族各自是**不可变、单调递增版本**的序列：无任何行 = 版本 0（Profile 视为 null 空投影；Room Presentation 视为 null 空投影）。每次写入**追加一个新版本行**，绝不 UPDATE / DELETE / 重建既有行。

### 2.1 Participant Profile（全局，P0：Agent）

| 字段 | 类型 | 约束 |
|---|---|---|
| `participant_id` | ParticipantId (UUID v4) | Profile 所属参与者；P0 只允许 Agent，模型按 Participant 建模 |
| `profile_version` | u64 | `> 0`；同 participant 严格单调 +1（无记录 => 版本 0） |
| `short_description` | 可选文本 | 规范化（空串 => null）；UTF-8 字节数 `<= 512`；禁 U+0000；原样保存 |
| `public_description` | 可选文本 | 规范化（空串 => null）；UTF-8 字节数 `<= 16384`；禁 U+0000；原样保存 |
| `published_by_id` | ParticipantId | 发布者 Human（P0：该 Agent 的 active Owner） |
| `published_at` | UtcMillis | Core 生成，不可由客户端指定 |

- 不复制 `display_name` / `status` / `kind`：那些是既有 Participant 身份事实，本切片不建冗余副本；
- 只允许可选短描述与公开描述两个文本事实；不做任何 Unicode 归一化，空值一律规范化为 `NULL`。

### 2.2 Group Room Presentation（按 Room + Participant）

| 字段 | 类型 | 约束 |
|---|---|---|
| `room_id` | RoomId | 必须是 `kind='group'` 的 Room |
| `participant_id` | ParticipantId | 该 Room 的 active Agent 成员 |
| `presentation_version` | u64 | `> 0`；同 (room, participant) 严格单调 +1（无记录 => 版本 0） |
| `room_display_name` | 可选文本 | 规范化（空串 => null）；UTF-8 字节数 `<= 80`；禁 U+0000 |
| `room_role_name` | 可选文本 | 规范化（空串 => null）；UTF-8 字节数 `<= 128`；禁 U+0000 |
| `room_role_description` | 可选文本 | 规范化（空串 => null）；UTF-8 字节数 `<= 4096`；禁 U+0000 |
| `published_by_id` | ParticipantId | 该 Group Room 的 active owner（Human） |
| `published_at` | UtcMillis | Core 生成，不可由客户端指定 |

- 每 (room_id, participant_id) 一条版本链；同一 Agent 在不同 Group Room 各自独立；
- 只支持 **group + active Agent member**；direct / human_direct / 非成员 Agent 都不得写。

### 2.3 SQLite 字节长度语义

`length(text)` 在 SQLite 里返回**字符数**（scalar value 数），不是字节数；因此所有文本长度约束在 SQL 内必须用 `length(CAST(value AS BLOB))` 按 UTF-8 字节数裁决；U+0000 用 `instr(value, char(0)) = 0` 拒绝。空串由**应用归一化**为 `NULL`；SQL 层**同样拒绝 `''`**（每个可空文本列的 `<> ''` CHECK），使空串不可能成为与 `NULL` 并列的第二个持久事实；由空白组成的非空字符串（如 `'  '`）仍合法且**不 trim** 原样保存。这些都在 migration 内以一列多 CHECK 实现，行为由 migration 完整性测试证明。

### 2.3b 人自己的名字（2026-10-17 表达权第四刀）

⚠ **名字不在这条版本链上**：§2.1 明文「不复制 `display_name` / `status` / `kind`：那些是既有 Participant 身份事实」——所以人的名字住在 `participants.display_name`，**不是** `participant_profile_versions` 的一格。

- **写路径**：`PUT /v1/me/profile`（HumanAuth，身份只来自认证，路径里没有"写谁的"）；
- **它是状态写**：没有版本行、没有幂等记录 —— 同值重写天然幂等（照 `PUT /v1/backend/rooms/{room_id}/read-cursor` 的先例）；
- **约束与兑换时同一套**（`DisplayName::parse`：禁 NUL、≤80 个 Unicode scalar value、trim 后非空；`participants` 表上也有同样三条 CHECK）；
- **只有本人能写**：Human 没有 Owner 概念 ⇒ 不带"替别人写"的分支（Agent 那一支的并集一字不动）；
- **投影语义**：名字是**投影不是快照** ⇒ 改名之后历史消息里显示的作者名跟着变（2026-10-17 主人裁决，见 `outputs/HUMAN_PROFILE_RULING_DRAFT_2026-10-17.md` §6）。

人的**头像**走本切片的版本链那一套（`PUT|DELETE /v1/me/profile/avatar`），细节在 `CORE_V1_AVATAR_P0.md`。

### 2.4 字段来源：`published_by_kind` 是 API 投影，不是持久事实

API 投影里的 `published_by_kind`（发布者 Human 的种类）是**派生投影**：来源为 `published_by_id -> participants.kind`，**不**在 `participant_profile_versions` / `room_presentation_versions` 里冗余存一份 `published_by_kind` 列。后续 API 阶段必须输出该投影，但它**永不成为第二个持久身份事实**——一旦参与者的 `kind` 或 `published_by_id` 变化，投影随之变化，绝不写入版本行。migration 只存 `published_by_id`（外键 → participants），来源单一。

## 3. 可见性（当前 active Membership 读取投影）

- **Room Presentation**：P0 公开投影读取必须遵守**当前 active Room Membership**（`room_memberships.active_marker = 1` 的现行周期，与既有 members 契约同一裁决；**不是** Membership 历史）：只有该 Room 的当前 active member/观察者，能把该 Room+Participant 的呈现投影给相应 Agent 成员；非成员不看到任何呈现事实，不形成存在性 oracle；
- **Profile（P0：Agent）**：通过已授权 Room 内的成员详情端点主动读取完整 Profile；不提供跨 Room 或全局 Profile 读取入口。
- 呈现事实本身不改变房间权限：可见性不等于可写。

## 4. 写权限（P0 授权路径）

写入路径统一为「认证 Human Session → 权限裁决 → payload/base_version/幂等校验 → 单写事务提交」。

- **Agent 全局 Profile 写入**（⚠ **2026-09-28 批次四表达权第一刀修订**）：actor 必须是目标 Agent 的 **active Agent Owner**（本身为 active Human）**或该 Agent 自己**（BackendAuth）——**并集**，形状与批次三第③步同一条（主人裁决「像飞书 QQ 那种，人类用户把它带进来」）。在此之前这里写的是"**Agent 自己**、Backend/Connector/Runtime 都不得写"：**表达权归主体**——自己的资料只有自己（或替它担责的 Owner）能写，ownership 是责任关系，不是代笔权。没有 active Owner 关系的 Human、与该 Agent 无关的第三方、Connector/Runtime 仍不得写。**Agent 自己写自己**走 `PUT /v1/backend/me/profile`（BackendAuth；**身份只来自认证，路径里没有 agent_id**——与读取面同一条纪律）；`published_by_id` 如实记下**是谁发布的**（视图里另有 `published_by_kind`：是它自己还是 Owner——**不是禁止代笔，是让代笔有署名**）。
- **Group Room Presentation 写入**（⚠ **2026-09-28 批次四表达权第三刀修订**）：Room 本身必须是 **active** 且 `kind='group'`；actor 必须是该 Group Room 的 **active owner**（**不再限 kind**——批次三第一刀已把 owner 判据换成"创建者"）**或被呈现的那位主体自己**（`published_by_id = participant_id`，BackendAuth 走 `PUT /v1/backend/rooms/{room_id}/my-presentation`，**路径里只有房间、没有"写谁的"**）；`participant_id` 必须是该 Room 的 **active Agent member**。仅 Agent Owner 而**没有**该 Room 成员资格的 Human 对该 Room Presentation 无任何权威；三条件缺一不可；
- 写入口只接受 Human Session 认证；Backend Credential 不能写呈现事实（呈现是公共事实，Backend 不是 Participant）；
- Migration 在数据库层用触发器兜底上述方向与资格（见 §6 不变量）。

## 5. Idempotency-Key 与 base_version（精确规则）

### 5.1 基本语义

写入请求必须携带 `Idempotency-Key`（1–128 个可见 ASCII、无逗号、无首尾空白——与既有 Agent/Room 创建同一规则）与 `base_version`（整数 `>= 0`，表示客户端认为的当前最新版本）。裁决在**当前认证与当前授权通过之后**执行——授权先于重放：被撤销 / 未获授权的 actor 一律不得经重放访问结果。裁决规则：

- **同 key、同请求摘要（digest 相同）→ 恒重放**：返回首次成功记录的结果（幂等重放，`idempotency_replayed`），**绝不追加第二个版本**；**即使当前最新版本已被另一写入推进，也不重新裁决 base_version、不变 stale**——重放只读回记录的 `result_version` 与 `result_is_create`；
- **同 key、不同请求摘要（digest 不同）→ 409 `idempotency_conflict`**：首次事实不变，与 base_version 是否匹配无关（head 记录即权威）；
- **新 key、`base_version` 与当前最新不一致 → 409 `profile_stale` / `room_presentation_stale`**：不追加版本；
- **新 key、`base_version` 与当前最新一致 → 写入新版本**。

### 5.2 版本推进与 concurrency

- 无记录 => 最新版本 `V = 0`；
- 成功写入产生新版本 `V' = V + 1`；
- **只有「新 key」路径要求 `base_version == V`**（否则 409 stale）；同 key 同摘要的重放路径不裁决 base_version；
- 每个 key 至多写出一行版本行；不同 key 各自推进版本（并发不同 key 也以 base_version 裁决先后，任一时刻只有一个版本被接受，最终单调无缺口——由 migration 单调递增触发器强制）。

### 5.3 请求摘要（request_digest = SHA-256 canonical JSON）

存储在幂等记录里的是 **SHA-256 64 位小写十六进制摘要**，绝不存储描述/预览文本（幂等表永不复制正文）。摘要输入是**固定字段序的 canonical JSON**（`serde_json` 序列化私有请求 struct）：字段含 `base_version` 与全部可空编辑字段，**空串先归一为 `null`、`null` 显式输出**——因此 `""` 与 `null` 是同一有效请求并产生同一摘要（与存入数据库的规范化事实一致）。字段序固定于 struct 声明，不随调用方传参顺序变化；canonical 表示与 golden 值由 domain 层 digest 函数与单元测试统一定义并固守（`src/domain/participant_presentation.rs`）。

- Profile 摘要 = `profile_put_digest(base_version, short_description, public_description)` → canonical `{"base_version":…,"short_description":…,"public_description":…}`；
- Room Presentation 摘要 = `room_presentation_put_digest(base_version, room_display_name, room_role_name, room_role_description)` → canonical `{"base_version":…,"room_display_name":…,"room_role_name":…,"room_role_description":…}`。

### 5.4 幂等记录字段

| 表 | 复合主键 | 结果字段 |
|---|---|---|
| `participant_profile_put_idempotency` | `(actor_human_id, participant_id, key)` | `request_digest`、`result_profile_version`、`result_is_create`、`created_at` |
| `room_presentation_put_idempotency` | `(actor_human_id, room_id, participant_id, key)` | `request_digest`、`result_presentation_version`、`result_is_create`、`created_at` |

`result_is_create` **必须恰好等于 `result_version == 1`**（数据库 CHECK 强制，identity 见证），表示该请求首次把对应参与者 / Room+参与者从**无事实（版本 0）推进到版本 1**（true）还是**在既有版本上追加**（false）；重放时返回记录的 `result_version` 与 `result_is_create`，不重新裁决。授权先于重放：认证与授权（active Agent Owner / active group owner）在任何幂等读之前执行。

## 6. 事务与数据库不变量边界

每个写入（Profile 或 Room Presentation）由后续 adapter 在**单个 `BEGIN IMMEDIATE` 事务**内完成：读最新版本 → 认证 / 授权裁决 → 幂等 / base_version 裁决 →（新 key 时）插入不可变版本行 → 插入幂等记录 → commit；任一步失败整体回滚，不留半个版本或半条幂等记录。**数据库守卫只证明引用 / 身份一致性，不单独证明两行在同一事务内插入**：`BEGIN IMMEDIATE` 的同事务原子性是 adapter 的契约责任，deferred 复合外键与见证触发器都不能替代它。数据库层强制以下不变量（migration 触发器 + CHECK，由 migration 完整性测试证明）：

1. **不可变**：版本行与幂等记录一经插入不可 UPDATE / DELETE；`INSERT OR REPLACE` / `REPLACE` 的隐式删除阶段以 BEFORE INSERT identity guard 直接拒绝既有身份，绕过不可变守卫即失败；
2. **单调版本**：新版本必须等于「同 participant / 同 (room, participant) 当前最大版本 + 1」；版本 `> 0`；缺号/跳号/重复一律拒绝；
3. **方向与资格**：Profile 的 `published_by_id` 必须是 active Human 且是该 Agent 的 active Owner、target 必须是 active Agent；Room Presentation 的 `published_by_id` 必须是该 **active** group Room 的 active owner（Human）、`participant_id` 必须是该 active group Room 的 active Agent member、Room 必须 `kind='group' AND status='active'`；
4. **幂等见证（引用/身份一致性）**：幂等记录以 deferred 复合外键指向版本行（`(participant_id, result_profile_version)` → 对应 Profile 版本行；`(room_id, participant_id, result_presentation_version)` → 对应 Presentation 版本行），无法引用真实版本行的幂等记录在 COMMIT 期失败；身份一致性由 CHECK + BEFORE INSERT 见证触发器强制：`result_is_create == (result_version == 1)`、被引用版本行的 `published_by_id == actor_human_id`、`created_at` == 被引用版本的 `published_at`（同一 Core 操作时间）——这些守卫证明记录与版本行的引用/身份一致，**不**证明同事务原子性；
5. **正文卫生**：所有文本 NUL 拒绝、UTF-8 字节上限（BLOB length）；空串由应用归一化为 `NULL` 且 SQL 层同样拒绝 `''`（纯空白的非空字符串合法、不 trim）；幂等表绝不携带正文/描述/预览副本。

## 7. 错误（HTTP 层按此映射）

| 状态 | 稳定 code | 语义 |
|---|---|---|
| 401 | `authentication_required` / `invalid_session` | 无凭据 / 无效 Session |
| 400 | `invalid_idempotency_key` / `idempotency_key_required` | key 格式非法或缺失（与既有契约同一校验收敛） |
| 400 | `profile_invalid` | Agent Profile PUT 输入不合法：`base_version` 非整数或 `< 0`，或正文含 U+0000 / 超过对应字节上限 |
| 409 | `profile_stale` | Profile PUT：新 key 但 `base_version` ≠ 当前最新（并发/过期） |
| 400 | `room_presentation_invalid` | Room Presentation PUT 输入不合法：`base_version` 非整数或 `< 0`，或正文含 U+0000 / 超过对应字节上限 |
| 409 | `room_presentation_stale` | Room Presentation PUT：新 key 但 `base_version` ≠ 当前最新 |
| 400 | `room_presentation_not_supported` | 目标 Room 不是 group（direct / human_direct 等不支持的 Room 类型没有 Group Room Presentation） |
| 404 | `agent_not_found` | 目标 Agent 不存在，或无 active Owner 关系的写 Profile 请求（隐藏存在性，不留存在性 oracle） |
| 409 | `agent_not_active` | 目标 Agent 或 Owner 关系已 revoked（状态机冲突） |
| 404 | `room_not_found` | 目标 Room 不存在，或 actor 不是该 group Room 的 active owner（隐藏存在性） |
| 404 | `member_not_found` | 目标不是该 group Room 的 active Agent 成员（隐藏存在性） |
| 409 | `idempotency_conflict` | 同 key 不同请求摘要，首次事实不变 |
| 500 | `internal_error` | 未分类内部错误（逃逸 constraint 一律归此，不以 503 可重试暴露） |
| 503 | `storage_unavailable` | 存储暂时不可用 |

错误 body 与日志遵循既有统一 JSON 信封与脱敏纪律（不泄露 key、摘要、正文、SQL、路径）。

## 8. 黑盒验收清单（真实外部流程，已由 application/HTTP 与黑盒测试证明）

主路径：

1. 初始化 Human + 认证 Session；创建 Agent + active Owner 关系；
2. **Agent Profile 写入**：active Agent Owner Human Session + `Idempotency-Key` + `base_version=0` 提交 Profile（short/public description），产生版本 1（`result_is_create=true`）；再次同 key 同摘要重放返回同一结果且行数不变（`idempotency_replayed`）；
3. **并发/冲突**：同 key 不同摘要 → 409 `idempotency_conflict` 且首次事实不变；新 key 但 `base_version=0`（最新已变 1）→ 409 `profile_stale`；**重放不 stale**：同 key 同摘要、即使 `base_version` 已过期（最新已被推进到 2），仍返回首次成功结果（版本 1），绝不判 `profile_stale`、绝无第二个版本行；
4. **Group Room Presentation 写入**：创建 group Room（active Owner Human，`rooms.status='active'`）并加入 active Agent member；owner 以 agent 成员为目标写 Presentation（版本 1）；同 key 重放行数不变；
5. **无 Room 权威**：仅 Agent Owner、无该 group Room 成员资格的 Human 对 Room Presentation 写入 → `room_not_found`（404，隐藏存在性），不产生事实；非 group / human_direct / direct Room → `room_presentation_not_supported`（400）；非 active Agent member 目标 → `member_not_found`（404，隐藏）；
6. **失败路径**：NUL、超字节上限、以及**直写 SQL 的空串**都在公共提交前失败且不留任何事实；应用把空串归一化为 `NULL` 后写入成功（纯空白 `'  '` 之类非空串原样保留、不 trim）；合法 Unicode/换行/Tab/Emoji round-trip；
7. **只读投影**：读取侧不泄露非成员 Room 的呈现事实（无存在性 oracle）；
8. **不变量**：任何引入第二个写者、双写、改语义、改既有 members 契约、写 Live/Runtime 字段的组合都应在新测试中 fail closed。

每次验收至少保留：Core 启动与 readiness 输出、协议客户端请求摘要/状态、可关联的公共对象 ID、权限/幂等失败 code、可重复验证的行数与不可变证明。
