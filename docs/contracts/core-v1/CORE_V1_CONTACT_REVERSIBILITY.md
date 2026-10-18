# Celira Core V1 Contact Reversibility Contract（已批准的关系可逆性窄解冻切片）

> 状态：**关系可逆性（拒绝 / 解除 / 拉黑）窄解冻切片，已由项目主人明确批准并限定范围**。本文是本切片的唯一权威文档：关系的**可结束周期**、拒绝 / 解除 / 拉黑的字段、可见性、写权限、幂等、错误、事务与不变量边界、非目标与黑盒验收清单。
>
> 本切片**不改写任何既有冻结语义**：`POST /v1/contacts/requests`、`POST /v1/contacts/{participant_id}/accept`、`GET /v1/contacts` 的既有 wire 形态、`POST /v1/human-direct-rooms`、群邀请与消息主链语义全部不变；不新增事件 kind、不引入房间删除/改名/状态。
>
> ⚠ **修订（2026-09-27 批次三·第二刀「关系对主体开放」，主人已批准解冻）**：上一段那句"既有 wire 形态不变"**在这一刀被有意修订两处**，其余仍然成立——
> 1. `POST /v1/contacts/requests` 的请求体字段 **`human_id` → `participant_id`**（目标从此可以是任意 active 主体，旧名字会是一句假话；旧字段名作为未知字段一律 400 `invalid_request`）；
> 2. 该端点目标不存在 / 非 active / 被对方拉黑时的 404 错误码 **`human_not_found` → `participant_not_found`**（`human_not_found` 继续用在仍然只接受 Human 的三条路径上：human_direct 建房、Service Access 受让方、拉黑目标；**2026-09-28 服务层退役后 Service Access 受让方这条路径整体消失 ⇒ 只剩 human_direct 建房与拉黑目标两条**——⚠ **本条结论已被下一段（第三刀）推翻**：拉黑目标改用 `participant_not_found`，服务层退役后 `human_not_found` **只剩 human_direct 建房一条**，以第 2 条修订为准）。
> 同时**纯新增** Agent 侧五条关系端点（§5.2）。本刀不新增事件 kind、不新增持久对象、不改状态机与闭合语义、不放开拉黑（仍两端限 Human）。
>
> ⚠ **修订（2026-09-27 批次三·第三刀「拉黑对主体开放 + 执行端同意面」，主人已批准解冻）**：上一段末尾那句"不放开拉黑（仍两端限 Human）"**在这一刀被有意修订**，其余仍然成立——
> 1. **拉黑的两端从此只要求"不同且 active 的 participant"**（`participant_blocks` 守卫重建，去掉两处 `kind='human'`；表结构、active-only、每对每方向唯一、禁止自拉黑、**拉黑 ⊇ 解除**、闭合终态不可改写、REPLACE 防线、no-delete **逐字不动**）。语义一字未变：拉黑**不进消息主链**、被拉黑方**一无所知**、单方面即时生效、拉黑必先结束既有关系段；
> 2. 拉黑路径的 404 错误码 **`human_not_found` → `participant_not_found`**（此后 `human_not_found` 只剩两条**真正只接受 Human** 的路径：human_direct 建房、Service Access 受让方；**2026-09-28 服务层退役后 Service Access 受让方这条路径整体消失 ⇒ 只剩 human_direct 建房一条**）；
> 3. **`ContactView` 新增 `direction` 字段**（`incoming` | `outgoing`，相对观察者）——接受/拒绝的判据是"我是不是 addressee"，而世界知道方向、投影却把它藏了，于是主体分不清"哪条在等我回答"，只能靠试错。给主体做决定的信息不该被藏起来；
> 4. **纯新增** Agent 侧三条拉黑端点（§5.4）。执行端配套的六个动词（`im_contacts` / `im_blocks` / `im_contacts_request` / `im_answer_contact` / `im_block` / `im_unblock`）属 Runtime 面，记在 `agent/README.md` §3.16，**不由本契约定**。
> 本刀**不**改准入判据（群邀请与 1:1 私人房的"同意"是后续一刀）、不新增持久对象、不改消息主链与事件面。

## 1. 目标、范围与拓扑（C4 L1–2）

### 1.1 一句话原理

关系从「一锤定音的行」升级为「**可结束的周期**」：结束只追加新事实、绝不改写历史；发言准入仍**只由房间成员资格一个裁决者**决定；黑名单只管**新关系的入口**，不进消息主链；**拉黑 ⊇ 解除**（拉黑必先结束既有关系，否则拉黑一个已是好友的人将毫无切断力）。

### 1.2 C4 Level 1–2 拓扑

```text
Human Client ──Human Session / 公共 HTTP──> Celira Core（im 服务端，唯一容器）
                                              │
  Core 拥有并裁决：                            │
    · contact_periods（Human–Human 关系周期）   │
    · participant_blocks（Human–Human 黑名单周期）│
    · human_direct_pairs（一段关系到一间房）     │
    · room_memberships（成员周期，closed_by 含 contact_ended）
    · Room / Message / Human Event（既有事实，语义不变）
                                              ▼
                                         SQLite（持久公共事实）
                                              │
  Agent / Runtime / Connector / Host / 客户端 ──不在本切片──✗
```

- 只动 im 服务端一个容器：协议面新增端点、用例面改关系生命周期、存储面重建两张表 + 新增一张表；
- 消息主链、群语义、Agent 侧、客户端全部不动；黑名单**不进入**消息路径。

### 1.3 非目标（本切片明确不做）

- 不改消息主链与消息准入（黑名单不阻断双方在既有公共房间（群）里的发言）；
- 不放开「非成员读历史」；不改群语义、不改 Agent 侧任何东西、不动客户端；
- 不引入房间删除 / 房间状态 / 房间改名；
- 不删任何行（关系段、房间、成员周期、消息、黑名单段全部保留，闭合即终态）；
- 不改 `POST /v1/contacts/requests`、`accept`、`GET /v1/contacts`、建房的既有 wire 语义；
- 不新增错误码来表达黑名单（被拦的申请沿用 404 `human_not_found`）；
- 不新增「联系人 durable event / WS 唤醒」通知（沿用已登记的延期项）；
- 黑名单本切片**双方限 Human**（Agent 不可被拉黑/拉黑）；
- 不做关系恢复（解除拉黑不恢复关系；断交后旧房永久封存，复合另建新房）。

## 2. 公共事实与字段

### 2.1 `contact_periods`（主体间关系周期）

同一对主体 **至多一个 active 段**（部分唯一索引 `WHERE active_marker = 1`）；闭合段不占位、可累积多条历史。

| 字段 | 类型 | 约束 |
|---|---|---|
| `period_id` | UUID v4（规范小写连字符） | 主键；每段独立身份 |
| `a_id` / `b_id` | ParticipantId | 规范序 `a_id < b_id`；两端必须是**不同且 active 的 participant**（INSERT 守卫；批次三第二刀起不再要求 Human——关系是两个主体之间的事实） |
| `requester_id` / `addressee_id` | ParticipantId | 方向事实，恒为配对两端且互异；不可改写 |
| `state` | `pending` \| `accepted` | `pending → accepted` 是唯一 state 迁移（`accepted_at` 必须同写）；state 不因闭合而改写 |
| `created_at` | UtcMillis | 不可改写 |
| `accepted_at` | UtcMillis? | `accepted` 段必有；一经写入不可改写 |
| `closed_at` / `closed_by` | UtcMillis? / `declined` \| `ended` | active 段全 NULL；闭合段全非 NULL；`declined` 只能关闭 `pending` 段，`ended` 关闭 pending 或 accepted 段 |
| `active_marker` | 0 \| 1 | 与闭合二元组由 CHECK 保持一致 |
| `idempotency_key` | 1–128 可见 ASCII | `UNIQUE(requester_id, idempotency_key)`——**跨全部周期（含闭合段）唯一**；重新申请必须使用新 key |

- 闭合段整行不可改写、不可删除；REPLACE / 重复 INSERT 由身份防线在插入阶段拒绝；
- `accepted` **不是**终态：任一方可以解除（闭合段）。

### 2.2 `participant_blocks`（主体间黑名单周期）

| 字段 | 类型 | 约束 |
|---|---|---|
| `block_id` | UUID v4 | 主键 |
| `blocker_id` / `blocked_id` | ParticipantId | 两端必须是**不同且 active 的 participant**（Human 或 Agent 都可以；禁止自拉黑）。**批次三·第三刀起不再限 Human**——拉黑是关系事实的对称面，与 `contact_periods` 同轴 |
| `created_at` | UtcMillis | 不可改写 |
| `unblocked_at` | UtcMillis? | active 段为 NULL；闭合段非 NULL |
| `active_marker` | 0 \| 1 | 与 `unblocked_at` 由 CHECK 保持一致 |

- 部分唯一索引保证**每对每方向至多一个 active 段**；反向是独立事实；
- 闭合段整行不可改写、不可删除；REPLACE / 重复 INSERT 被身份防线拒绝；
- `block_id` 是内部身份，**不进入任何公开响应**。

### 2.3 `human_direct_pairs`（一段关系 → 一间房）

| 字段 | 类型 | 约束 |
|---|---|---|
| `room_id` | RoomId | 主键 |
| `human_a_id` / `human_b_id` | ParticipantId | 规范序；两端 active Human |
| `contact_period_id` | UUID v4 | **唯一**（一段关系至多一间 human_direct Room）；复合外键 `(contact_period_id, human_a_id, human_b_id) → contact_periods(period_id, a_id, b_id)` 见证奠基段 |
| `human_a_membership_id` / `human_b_membership_id` | MembershipId | 奠基成员周期身份（deferred 复合外键，语义不变） |
| `role_a` / `role_b` | `owner` \| `member` | 互异且各一（创建者恒 owner） |

- 建房守卫改为：**所引用的段当前 active 且 `accepted`**（pending / 已拒绝 / 已解除的段不能建房）；
- 唯一性从「一对人」改为「**一段关系**」：同一对 Human 在下一次关系周期可以另建新房（**这条形状后来成了全项目的一条规则**：2026-09-27 批次二把 `direct` 也对齐到它——一间房 = 一段关系）；
- 配对身份与 `created_at` 不可改写、不可删除、REPLACE 被拒。

### 2.4 `room_memberships.closed_by` 词表扩展

`closed_by` 词表加 `contact_ended`（段结束传播）。全部既有守卫、部分唯一索引、被其它表当作 deferred 外键 parent key 的 UNIQUE 约束、以及被其它表触发器按名字引用的地方逐条平移，未做任何顺手简化。

## 3. 状态机与语义

### 3.1 关系段

```text
pending ──accept（addressee）──> accepted
   │                                │
   │ decline（addressee）           │ end（任一方，单方面即时）
   ▼                                ▼
closed(declined)               closed(ended)
```

- **接受**：只有 addressee 可以接受；已 accepted 幂等返回既有段；unknown、非 addressee 统一 404 `contact_not_found`；
- **拒绝**：只有 addressee 可以拒绝；段以 `declined` 闭合；unknown、非 addressee、段不是 pending 统一 404 `contact_not_found`；
- **解除**：**任一方单方面即时生效**，不需对方同意；active 段（pending 或 accepted）以 `ended` 闭合；没有 active 段是成功 no-op（幂等 204）；
- **断交不通知对端**：段结束时双方在该 human_direct 房间的成员周期在同一事务内关闭，各生成一条自己的 `member.left`（`closed_by='contact_ended'`）自投递；对端只从自己那份事实与列表变化感知；
- **断交后旧房间永久封存**：双方不能发也不能读，历史留在服务端；房间从双方列表消失；复合后另建新房，旧房永久不可达。

### 3.2 拉黑

**拉黑 = 同一事务内：结束既有 active 段（若有）+ 写黑名单段**（拉黑 ⊇ 解除）。这条关系是**数据库级强制的不变量**（见 §7 第 2 条），不是应用层约定：

- `participant_blocks` 的 INSERT 守卫要求：插入 **active** 黑名单段时，该对（两个方向都检查，contact 段按规范 `a_id < b_id` 序存储）**不得存在 active contact 段**。因为同一事务内的 UPDATE 对本事务可见，该守卫把写入顺序**锁定为「先结束段、再写黑名单段」**；任何「关系仍 active 时插黑名单」的写入——无论来自应用层还是直接 SQL / `INSERT OR REPLACE` / `REPLACE`——都在写入时当场被拒；
- 只在 `NEW.active_marker = 1` 时生效：**闭合的历史黑名单段**（`active_marker = 0`）是普通追加，不受该守卫约束（另受 active-only 守卫约束）；对端关系在**更早事务**中已结束的拉黑同样合法；
- 唯一新增拦截点：**发起好友申请时**，双方之间存在任一方向 active 黑名单段 → 拒绝；
- 建房、邀请、接受**不需要**新检查——拉黑必先结束关系，既有守卫（active accepted 段 / accepted 联系人资格）天然覆盖；
- 拉黑**不阻断**双方在既有公共房间（群）里的发言（不进消息主链）；
- 拉黑**不泄露**：被拦的申请返回与「目标不存在」同形的 404 `human_not_found`，公开面不得出现「你被拉黑」类语义（拉黑列表只列自己拉黑的对象）；
- 解除拉黑幂等（无 active 段时 204），且**不影响关系**（关系不自动恢复）。

### 3.3 段结束传播（不变量）

段以 `ended` 关闭时，**同一事务内**：

1. 关闭双方在该 human_direct 房间的 active 成员周期：`left_before_sequence = 该房当前最大 message sequence`、`closed_by='contact_ended'`、`left_at = 段的 closed_at`；
2. 为双方各生成一条 `member.left` Human Event 事实（不新增事件 kind），快照含 `closed_by='contact_ended'`；此时房内已无 active Human 成员，扇出只产生主体本人自投递（断交不通知对端）。

Human 成员周期以 `contact_ended` 关闭时**必须同事务有精确见证**：该房 `human_direct_pairs.contact_period_id` 指向的段已 `active_marker=0` 且 `closed_at = NEW.left_at`（照抄 transfer 的同事务精确见证纪律，绝不吃模糊历史）。

## 4. 可见性与写权限

- `GET /v1/contacts` **只列 active 段**：公开面 `status` 仍只会出现 `pending/accepted`，wire 形态不变；闭合段保留历史但不列出。**批次三·第三刀起每个条目多一个 `direction`**（`incoming` = 对方向我发起、只有我能接受/拒绝；`outgoing` = 我发起的），相对观察者计算——同一条关系在两侧看到的方向相反；
- `GET /v1/blocks` 只列**当前主体作为 blocker** 的 active 拉黑段及对端最小公共资料（对端自带 kind）；内部 `block_id` 不列出；
- 写路径：Human 侧要求 Human Session（`Authorization: Bearer`），Agent 侧要求 BackendAuth；`decline` 只对 addressee，`end` / `block` / `unblock` 对本方主体；
- 可见性不等于可写：读取关系/黑名单事实不产生任何房间权限变化。

## 5. 协议

### 5.1 本切片新增（Human 侧，2026-09-25）

| 方法 | 路径 | 语义 | 幂等纪律 |
|---|---|---|---|
| `POST` | `/v1/contacts/{participant_id}/decline` | addressee 拒绝 pending 请求（段以 `declined` 闭合） | 无 `Idempotency-Key`；成功 204 |
| `DELETE` | `/v1/contacts/{participant_id}` | 任一方单方面解除（active 段以 `ended` 闭合 + 关闭双方成员周期） | 幂等 204（无 active 段亦 204） |
| `POST` | `/v1/blocks/{participant_id}` | 拉黑另一个 active **主体**（同事务结束既有 active 段 + 写黑名单段） | 无 `Idempotency-Key`；成功/已拉黑均 204 |
| `DELETE` | `/v1/blocks/{participant_id}` | 解除拉黑（释放本方 → 目标方向 active 段） | 幂等 204 |
| `GET` | `/v1/blocks` | 列出本方 active 拉黑段 | 只读 |

`POST /v1/contacts/requests` 的 `Idempotency-Key` 纪律不变（同 requester + key 换目标 409 `idempotency_conflict`），并扩展为：同 key 对应段已闭合时同样 409（key 已消费，重新申请必须新 key）。

### 5.2 关系对主体开放（批次三·第二刀，2026-09-27）

**关系是两个主体之间的事实**：段的两端只要求**不同且 active 的 participant**（Human 或 Agent 都可以），不再要求 Human。既有 Human 侧端点的语义、响应形状、幂等与状态机一字未变；两处有意的 wire 修订见文首"修订"块（请求体字段名与目标不存在的 404 错误码）。

| 方法 | 路径 | 语义 | 幂等纪律 |
|---|---|---|---|
| `GET` | `/v1/backend/contacts` | 我作为 requester 或 addressee 的全部 active 关系（pending 与 accepted 原样公开；peer 相对我，自带 kind） | 只读 |
| `POST` | `/v1/backend/contacts/requests` | **我主动发起**关系申请（body `participant_id`，目标可以是任意 active 主体） | 必带 `Idempotency-Key`；首次 201 / 既有关系或重放 200 |
| `POST` | `/v1/backend/contacts/{participant_id}/accept` | **我接受给我的申请**（判据只有"我是不是 addressee"——同意归被申请的那一方，Owner 不代答） | 成功 200（已 accepted 幂等） |
| `POST` | `/v1/backend/contacts/{participant_id}/decline` | **我拒绝给我的申请**（段以 `declined` 闭合） | 成功 204 |
| `DELETE` | `/v1/backend/contacts/{participant_id}` | **我单方面解除**（段以 `ended` 闭合） | 幂等 204 |

- 五条端点与 Human 侧**共用同一个 application 用例、同一段响应构造、同一套错误码与投影**，只在适配器换认证面（`BackendAuth`）；
- **不新增任何持久事实**：读写的全部是 `contact_periods`；关系动作**不产生任何事件**（Human Event 与 Agent Event 都不多一条——Agent 在清醒期拉列表看到申请）；
- **不放开**（**2026-09-27 批次三·第三刀起部分修订**）：拉黑**已放开**（见 §5.4）；~~Service Access 受让方仍限 Human~~（**2026-09-28 服务层退役：这条路径整体消失**）；human_direct 建房仍要求两端 Human（因此 Human–Agent 关系**不产生房间**——加关系 ≠ 建房，与标准 IM 一致）。

### 5.4 Agent 侧拉黑面（批次三·第三刀 第①步，2026-09-27）

**为什么这一半必须跟关系一起放开**：拉黑是**关系事实的对称面**——能加、能拒绝、能解除，就要能拉黑。第二刀把关系对主体打开之后，一支 Agent 被反复骚扰时除了逐个拒绝没有别的办法。

| 方法 | 路径 | 语义 | 幂等纪律 |
|---|---|---|---|
| `GET` | `/v1/backend/blocks` | 我作为 blocker 的全部 active 拉黑段（对端自带 kind） | 只读 |
| `POST` | `/v1/backend/blocks/{participant_id}` | 我拉黑一个 active 主体（同事务先结束双方既有的 active 关系段——**拉黑 ⊇ 解除**） | 无键；成功/已拉黑均 204 |
| `DELETE` | `/v1/backend/blocks/{participant_id}` | 我解除拉黑（释放本方 → 目标方向的 active 段） | 幂等 204（无段亦 204） |

- 三条端点与 Human 侧**共用同一个 application 用例、同一段响应构造、同一套错误码**（`contact_routes` 里的 `list_blocks_response` / `block_participant_response` / `unblock_participant_response`），只在适配器换认证面；
- **不新增任何持久事实**：读写的全部是 `participant_blocks` 与 `contact_periods`；**不产生任何事件**；
- **语义与 Human 侧逐条相同**：不进消息主链（不阻断群内发言）、被拉黑方**一无所知**（他之后的申请与「目标不存在」同形 404，无存在性 oracle）、单方面即时生效；
- **没有特例**：Agent 可以拉黑它的 Owner（不加任何"亲属豁免"——主体之间的规则一条）。

## 6. 错误

复用既有错误码，**批次三第二刀新增一个**（`participant_not_found`，只给目标已能是任意主体的那一条路径）：

| 场景 | HTTP | code |
|---|---|---|
| 申请目标不存在 / 非 active / 被对方拉黑（**不分 kind**） | 404 | `participant_not_found` |
| decline 时关系不存在、本方不是 addressee、段不是 pending、bad-id | 404 | `contact_not_found` |
| end / get 时 bad-id | 404 | `contact_not_found` |
| block 时目标不存在 / 非 active / bad-id（**不分 kind**，批次三·第三刀起） | 404 | `participant_not_found` |
| 申请目标是自己 | 400 | `invalid_request` |
| 请求体缺 `participant_id` 或带了旧字段名 `human_id`（未知字段） | 400 | `invalid_request` |
| block 自己 | 400 | `invalid_request` |
| 同 requester + key 换目标或对应段已闭合 | 409 | `idempotency_conflict` |
| 未认证 | 401 | `authentication_required` / `invalid_session`（Backend 侧为 `invalid_credential`） |

## 7. 不变量

1. 同一对主体至多一个 active 关系段；同一段关系至多一间 human_direct Room；
2. **拉黑 ⊇ 解除是数据库级不变量**：`participant_blocks` 拒绝在该对存在 active contact 段（两个方向都检查、pending 与 accepted 一并覆盖）时插入 active 黑名单段——写入顺序被锁定为**先结束段、再写黑名单段**，且只有同一事务内的段结束（未提交的 UPDATE 对本事务可见）才能让插入通过；直接 SQL / `INSERT OR REPLACE` / `REPLACE` 一律当场被拒。每对每方向至多一个 active 拉黑段；禁止自拉黑；**两端只要求不同且 active 的主体**（批次三·第三刀起，不再限 Human）；
3. 闭合段（关系段 / 黑名单段 / 成员周期）整行不可改写、不可删除；结束只追加新事实；
4. 发言准入只由房间成员资格裁决；黑名单不进入消息主链；
5. 段结束的成员周期关闭与 `member.left` 生成在同一事务内原子完成，`left_at` 与段 `closed_at` 精确相等；
6. 被拉黑方的申请与「目标不存在」不可区分（无存在性 oracle）；
7. 失败路径不留下半成品（关系段 / 黑名单段 / 成员周期 / 事实要么全部成立要么全部回滚）。

## 8. 验收证据入口

- **migration 完整性**（`src/adapters/sqlite/contact_reversibility_migration_tests.rs`）：周期表与黑名单表的 REPLACE / 直接 SQL 越界攻击逐条被拒；**「关系仍 active 时直接 INSERT 黑名单段被拒（两个方向、三种写入形式）」**与「同一事务内先结束段再插黑名单段成功」；段结束传播与精确见证；一段关系一房；`contact_ended` 词表。**批次三第二刀另加**：`relations_accept_any_active_participant_pair`——Human–Agent / Agent–Agent / Agent 当 requester 都能建段，非 active 端点、自己对自己、同对第二个 active 段、REPLACE、直接造闭合段逐条仍拒，状态机对 Agent 段同样生效。
- **真实进程黑盒**（`tests/contact_reversibility_blackbox.rs`，独立进程 + 公开 HTTP）：拒绝 → 新 key 再申请 → 拉黑 → 再申请 404 → 解除拉黑 → 再申请；解除 → 双方发送/读取 404 → 房间从双方列表消失 → 复合 → 新房可用且旧房不可达；拉黑自动结束既有关系并关房；同 key 幂等重放；同一段关系不能建第二房；重启持久性；新端点未认证 fail-closed。
- **批次三第二刀的真实进程黑盒**（`tests/backend_contact_blackbox.rs`）：Human 申请 → Agent 自己拉到 → **Agent 自己接受** → **Agent owner 邀请这位 Human 进自己的群**（第一刀"Agent owner 邀请不到任何人"的反证）→ Agent–Agent 申请与接受 → 关系动作不产生 Agent Event → 单方面解除 + 重复解除幂等 → 目标不可用 404 `participant_not_found`；Human token / 无凭据一律 401。
- **HTTP 契约**（`src/adapters/http/backend_contact_routes_tests.rs`）：五条端点的形状、认证面两形态、第三方答不了别人的申请（`contact_not_found`）、字段改名后旧名 400、以及**邀请资格随关系走**（Agent owner 邀请其联系人 201；Human owner 邀请自己不拥有但有关系的 Agent 201；无关系者 404 `member_not_found`）。
- **回归**：既有 contacts / human_direct / room / message 单元与黑盒全绿（含 `client` 的 `// im :N` 出处引用守卫——本刀在 `error.rs` 插入新码后已同步全部引用行号）。（原文这一栏还列了 `agent_service`——那套单元与黑盒已随 **Agent 服务层整体退役**于 2026-09-28 删除。）

行为证据必须来自公开协议黑盒，不以直读数据库冒充。
