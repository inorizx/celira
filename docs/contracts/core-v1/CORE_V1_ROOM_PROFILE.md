# Celira Core v1.1 附加切片：Group Room Profile（群简介）

> 状态：**v1.1 附加切片 + 2026-10-26 接入切片，已获主人批准**。授权范围 =
> 本文件边界内的改动；边界之外的 Core v1 冻结面（身份、房间、成员、权限、
> 消息、Event、附件、撤回、read state、投递位置）仍然冻结。本文件是「群简介」
> 的唯一权威契约：持久形状、写读 wire、错误码、不变量与非目标。
>
> ✅ **三层已接入（2026-10-26）**：Core（`im/server`）提供读写；客户端
> （`client/`）的群信息抽屉可读、群主可编辑；Native Runtime（`agent/`）以
> `im_write_room_profile` 写入，并以 `im_read_history` 的房间头读取。本文仍是
> 群简介的唯一权威契约；客户端与 Runtime 的实现细节见各自 README 与验收记录。

## 1. 目标与拓扑（C4 Level 1–2）

### 1.1 一句话原理

**房间简介是 Core 的公共事实、Runtime 的读时上下文**：只有该房**当前 active
owner**（Human 或 Agent——owner 是房内的治理角色，不是参与者 kind）能版本化
写入；读取随单房历史回执到达；它**不进消息 / 投递链**，也不产生未读或通知。

### 1.2 拓扑

```text
Client（已接入）── Human Session ──> Core HTTP
  ├─ GET /v1/rooms/{room_id}                 读简介（member-scoped 管理视图）
  └─ PUT /v1/rooms/{room_id}/profile         群主写（Human Session + Idempotency-Key）

Native Runtime（已接入）── BackendAuth ──> Core HTTP
  ├─ PUT /v1/backend/rooms/{room_id}/profile 群主写（Agent 身份 + Idempotency-Key）
  └─ GET /v1/backend/rooms/{room_id}/messages
       历史回执带房间头（id / kind / name / profile）

Core SQLite
  ├─ rooms                          不动（房名与 kind 仍由既有事实持有）
  ├─ room_profile_versions          新增：简介的不可变版本链
  └─ room_profile_put_idempotency   新增：写请求幂等见证（actor 不限 kind）
```

- **零新容器、零新连接拓扑**：客户端与执行端仍然只经既有 HTTP 面说话；
  本切片没有新增进程、端口、队列或存储。
- 消息、投递、已读游标、SSE 门铃、Human Event / Agent Event **全部不动**；
  简介不是消息，也不参与任何未读计数。

## 2. 归属：简介是房间的公共资料

| 事实 | 归属 | 写者 | 读面 |
|---|---|---|---|
| 群名 `rooms.name` | 既有 Room 事实 | 建房时固定（本切片不改可变性） | RoomView |
| 群简介 | **本切片新增的版本化公共资料** | 该群当前 active owner（**Human 或 Agent**） | `GET /v1/rooms/{id}` 与单房历史回执 |
| 成员在群里的呈现 | 既有 Group Room Presentation | 该房 owner 或成员自己 | 既有 member-presentations 面 |

判据一句话：**简介说的是"这间房是什么"，不是"我在这间房里是谁"**——所以它
不挂在任何 Participant 上，也不复制 `display_name` / `status` / `kind`。

## 3. 写面：两条认证面，同一条业务路径

```text
PUT /v1/rooms/{room_id}/profile           Human Session（CurrentAuth）
PUT /v1/backend/rooms/{room_id}/profile   Agent BackendAuth
```

两条端点调用**同一个 application 用例**、共用同一段请求体 / 幂等键解析与响应
映射（适配器只换认证面与 actor 的来源），语义、状态码、错误码与成功形状逐条
相同。Human 面在 2026-10-26 之前的线上行为**一字未变**。

### 3.1 请求

```text
PUT /v1/rooms/{room_id}/profile
Authorization: Bearer <HumanSessionToken>
Idempotency-Key: <opaque key>
Content-Type: application/json

{ "base_version": 0, "description": "这个群在做什么…" }
```

```text
PUT /v1/backend/rooms/{room_id}/profile
Authorization: Bearer <clr_bc_...>        （BackendAuth）
Idempotency-Key: <opaque key>
Content-Type: application/json

{ "base_version": 0, "description": "这个群在做什么…" }
```

- 认证：Human 面用 Human Session（`CurrentAuth`）；Backend 面用 BackendAuth
  （active Credential + active Binding + active Agent）。身份只来自认证——
  路径里只有房间，没有"以谁的名义写"。两支凭据互不通用：Backend token 打
  Human 面 → 401 `invalid_session`；Human token 打 Backend 面 → 401
  `invalid_credential`。
- `Idempotency-Key` **必带**，形状规则沿用既有写端点（1–128 字节、
  无首尾空白、无逗号、可打印 ASCII；缺 → `400 idempotency_key_required`，
  形状错 → `400 invalid_idempotency_key`）。
- `base_version`：`>= 0` 的整数。首次写 `0`；此后必须等于当前最新版本，
  否则 `409 profile_version_conflict`。
- `description`：**必填**（缺字段 / `null` → `400 invalid_request`）。
  校验：1..2000 **UTF-8 字节**（按 BLOB 长度）、禁 U+0000、`trim` 后非空；
  允许换行、回车、Tab、Emoji 与正常 Unicode。违反 → `400 invalid_profile_description`。
  **原样保存**：不 trim、不做 Unicode 归一化；`trim` 只用于"是否为空"的裁决。
- 请求体上限 16 KiB（JSON 转义后仍容纳 2000 字节正文）；超出 → `413 payload_too_large`。
- 未知字段一律 `400 invalid_request`。

### 3.2 权限（顺序即裁决顺序）

1. Room 不存在 / bad-id / actor 不是该房 **active 成员** → `404 room_not_found`
   （不泄露存在性）；
2. Room 不是 `group`（`direct` / `human_direct`）→ `400 invalid_room_kind`；
3. actor 是该房 active 成员但不是 **owner** → `403 not_room_owner`
   （明确 ACL：身份已由认证证明，群主资格是可观察公共事实）；
4. actor 是该房 active owner 但**不是 active 主体**（已 revoked）→ 同样
   `403 not_room_owner`。

**判据只有"当前 active owner"，不看参与者 kind**：owner 自 2026-09-27 起是
房内的治理角色（`role='owner'`），不是参与者种类（`kind`）——一支 Agent 可以
建房并持有 owner，因此它也能写自己群的简介（经 BackendAuth 面）；Human owner
走 Human 面——Backend 面代表的是那支 Agent 自己，不是它的 Owner（两支凭据是
不同主体，不能互相代替）。

授权读的是**当前 active owner 周期**：群主转让后，新 owner 立刻可写，旧 owner
立刻 `403`（不靠任何身份缓存）。转让**跨 kind** 同样即时生效（Human → Agent、
Agent → Human 都已验证）。

### 3.3 成功响应（统一 `200`）

```json
{
  "profile": {
    "description": "这个群在做什么…",
    "version": 1,
    "updated_at": "2026-10-25T12:34:56.789+00:00"
  },
  "idempotency_replayed": false,
  "request_id": "…"
}
```

两条认证面共享同一套语义：

- **每次成功写恰好产生一个新版本**（`version = 当前最新 + 1`；首版 = 1）。
- 首次写与后续更新都是 `200`；不设 `201`（"首个版本"不是资源创建语义，
  重放与首次同形）。
- 同 key + 同摘要重放：`200`、`idempotency_replayed=true`、返回**首次**结果，
  **不追加版本**、**不重新裁决 `base_version`**（即使 head 已被推进）。
- 同 key + 不同摘要：`409 idempotency_conflict`，首次事实不变。

## 4. 读面

### 4.1 `GET /v1/rooms/{room_id}` 增加 `profile`

```json
{
  "room": { "id": "…", "kind": "group", "status": "active", "name": "研发组", "created_at": "…" },
  "membership": { … },
  "peer": null,
  "read_state": { … },
  "peer_read": null,
  "profile": { "description": "…", "version": 2, "updated_at": "…" },
  "request_id": "…"
}
```

- `profile` 在**没有简介**或**非 group** 时为 `null`；其余字段与今天逐字段相同。
- 可见性沿用既有 `GET /v1/rooms/{id}` 裁决（unknown / bad-id / 非成员统一 404）。
- **`GET /v1/rooms` 列表不带这一格，`RoomView` 也不加字段**——简介只在单房
  读面出现，列表仍是既有形状。

### 4.2 `GET /v1/backend/rooms/{room_id}/messages` 增加顶层 `room`

```json
{
  "messages": [ … ],
  "next_before": null,
  "room": {
    "id": "…",
    "kind": "group",
    "name": "研发组",
    "profile": { "description": "…", "version": 2 }
  },
  "request_id": "…"
}
```

- `room` 是**附加字段**：`messages` / `next_before` / `request_id` 的形状、
  顺序语义与分页规则**一字未动**；每一页都带同一个房间头。
- `room.profile` 只有 `description` + `version`（回执是上下文，不是管理视图，
  不带 `updated_at`）；无简介或非 group 为 `null`。
- 准入**仍是既有的**：未知房 / bad-id → `404 room_not_found`；Room 存在但
  agent 非 active 成员 → `403 not_room_member`。房间头在 `list_messages`
  成功之后才读取，**不引入第二套可见性判据**。
- 人类历史端点 `GET /v1/rooms/{room_id}/messages` **不变**，不返回 `room`。

## 5. 错误码

| code | HTTP | 场景 |
|---|---|---|
| `authentication_required` | 401 | 缺 Authorization（任一写面） |
| `invalid_session` | 401 | Session 无效 / 已撤销 / 指向非 active Human（Human 面） |
| `invalid_credential` | 401 | 凭据格式错 / Human token / 未知 / 任一级 revoked（Backend 面） |
| `idempotency_key_required` | 400 | 缺 `Idempotency-Key` |
| `invalid_idempotency_key` | 400 | key 形状不合法 / 重复 header |
| `invalid_request` | 400 | 请求体不是 JSON、缺字段、`base_version` 负数或非整数、未知字段 |
| `invalid_profile_description` | 400 | 正文为空、`trim` 后为空、含 U+0000、超过 2000 UTF-8 字节 |
| `invalid_room_kind` | 400 | 目标 Room 不是 `group` |
| `not_room_owner` | 403 | actor 是该房 active 成员但不是当前 active owner（不限 kind） |
| `room_not_found` | 404 | bad-id / 未知房 / actor 不是该房 active 成员（不泄露存在性） |
| `profile_version_conflict` | 409 | 新 key 但 `base_version` != 当前最新 |
| `idempotency_conflict` | 409 | 同 key 已被不同请求（不同摘要）使用 |
| `payload_too_large` | 413 | 请求体超过 16 KiB |
| `storage_unavailable` / `internal_error` | 503 / 500 | 既有安全错误边界 |

错误信封与 `request_id` 规则沿用既有统一形状；错误正文不含简介原文、幂等键、
SQL、路径或内部版本边界。

## 6. 持久形状与不变量

### 6.1 Migration

两个 schema-only 迁移（**不搬行、不改任何既有迁移**）：

`im/server/migrations/202610250001_room_profile.sql`（v1.1 切片）：

- `room_profile_versions(room_id, version, description, updated_by, updated_at)`，
  PK `(room_id, version)`；
- `room_profile_put_idempotency(actor_human_id, room_id, key, request_digest,
  result_version, created_at)`，PK `(actor_human_id, room_id, key)`，
  deferred 复合外键 `(room_id, result_version)` 指向版本链，只证明引用一致性；
- 触发器：版本不可 UPDATE / DELETE / REPLACE 改写；同 room 版本严格 +1；
  写入者必须是 active group 的 active Human owner；幂等记录不可改写 / 不可删 /
  身份防重 / actor 必须是 active Human / 见证一致性（writer == actor、
  `updated_at == created_at`）；正文 CHECK 按 BLOB 长度 1..2000、禁 NUL、
  trim 后非空。

`im/server/migrations/202610260001_room_profile_any_owner.sql`（2026-10-26，
owner 不限 kind）：

- `ALTER TABLE room_profile_put_idempotency RENAME COLUMN actor_human_id TO
  actor_id`——列名与事实对齐（actor 可以是 Human 或 Agent）。SQLite 的
  `RENAME COLUMN` 同步改写 PK、外键与触发器正文里的引用；
- 重建 `room_profile_versions_insert_guards`：**去掉 `p.kind='human'`**，仍要求
  Room 是 active group、写入者是 active 主体、且是该房当前 active owner；
- 重建幂等 actor 守卫并改名 `room_profile_put_idempotency_actor_active`：actor
  必须是 active 主体（不限 kind），错误文案同步为
  `room profile idempotency actor must be an active participant`。

其他表 / 列 / 触发器（不可变、单调 +1、身份防重、见证一致性、正文 CHECK）逐字
不动；已提交的 `202610250001` 不重写。

### 6.2 不变量

- **I1 房间资料，不是消息**：简介不进 `messages`、投递、已读游标、SSE、
  Human Event / Agent Event；不产生未读、不产生通知、不改任何分页。
- **I2 只有群主能写**：非成员 404（不泄露）、成员非 owner 403、非 group 400；
  授权读当前 active owner 周期，**不限参与者 kind**（Human 与 Agent owner
  同一条判据，两个认证面各自证明身份），转让即时生效（含跨 kind）；
  DB 触发器兜同一判据。
- **I3 版本只增不改**：无行 = 没有简介；每次成功写恰好 +1；UPDATE / DELETE /
  REPLACE 一律被拒；不同 Room 的版本互不影响。
- **I4 幂等**：同 key 同摘要恒重放（不追加版本、不重新裁决 base_version）；
  同 key 不同摘要 409；新 key 才要求 `base_version == 当前最新`；失败整体回滚，
  不留半个版本或半条幂等记录。
- **I5 读面不新增准入**：`GET /v1/rooms/{id}` 复用既有成员裁决；单房历史先
  成功 `list_messages` 再补房间头；非成员仍 403、未知房仍 404。
- **I6 列表与 RoomView 不动**：`GET /v1/rooms` 不带 `profile`，`RoomView` 无新字段。
- **I7 正文原样**：存的是提交原文（含首尾空白、多行、Emoji）；`trim` 只裁决
  非空；2000 字节按 UTF-8 字节数（SQLite 用 `length(CAST(... AS BLOB))`）。
- **I8 附件/头像/成员呈现零耦合**：简介不是附件，也不是 Participant Profile
  或 Room Presentation 的一个字段。

## 7. 测试证据

- migration 完整性：`cargo test -p celira-im --bin celira-im room_profile_migration_tests`
  ——表/触发器存在、`actor_id` 正名（含触发器正文同步改写）、旧的 kind 限定
  actor 守卫退场、单调、不可变、REPLACE 防线、group-only、**active owner
  （Human 与 Agent）**、Human 成员不可代写、拒绝非成员 / direct 房、正文边界
  （空/纯空白/NUL/2001 字节/多行 Emoji）、幂等 deferred FK 与见证。
- 存储层：`cargo test -p celira-im --bin celira-im room_profile_store_tests`
  ——写读、版本单调、重放抵御 head 推进、冲突/过期、**Agent owner 可写**、
  Human/Agent 成员 403、非成员 404、owner 转让（Human → Agent 与
  Agent → Human）后权限即时切换、direct 房、房间头、失败零残留。
- HTTP 契约：`cargo test -p celira-im --bin celira-im room_profile_routes_tests`
  ——完整错误矩阵、列表/RoomView 不变、历史房间头与分页零重叠、owner 转让、
  Human owner 的 human route 行为不变、**Agent owner 经 BackendAuth 写自己的
  群**（401 / 403 / 404 / 400 / 409 / 重放）、Human 成员不可代写。
- 领域规则：`cargo test -p celira-im --bin celira-im room_profile_tests`。
- 真实进程黑盒：`cargo test -p celira-im --test room_profile_blackbox`
  ——CLI 建世界 → 群主写 → 成员读 → 错误矩阵 → owner 转让 → 历史房间头 →
  **Agent 经 backend 建房并写简介 → Human 成员读 → 跨 kind 转让 → 重启后
  仍在**。
- 回归：`cargo test -p celira-im --all-targets` 全绿；`cargo fmt --check`
  干净；`cargo clippy -p celira-im --all-targets -- -D warnings` 干净。
- 客户端接入（2026-10-26）：`npm test` 958/958、`npm run build`、`npm run lint`
  全过；浏览器冒烟：群信息抽屉的空态 → 编辑 → 保存 → 显示，以及人类群主把群
  转让给 Agent 成员的入口（Agent 行姓名仍可读）。
- Runtime 接入（2026-10-26）：`cargo test -p celira-agent --all-targets
  --no-fail-fast` 全绿（1082 passed；`im_app` 116 passed），新增
  `im_write_room_profile` 的形状、参数网关、403/404/409 分类、2xx 读数留白与
  房间头复核用例。

行为证明来自公开协议 / 真实进程黑盒；SQLite 只在 migration 与 store 层证明
持久守卫，不以直读数据库冒充 HTTP 成功证据。

## 8. 非目标（一个字都不碰）

- 公告 / 置顶 / 未读 / 通知 / SSE / 事件；
- `POST /v1/groups` 或 Backend 建房端点的创建体（建房仍只有 `name`）；
- `scripts/home/**`；
- `direct` / `human_direct` 的简介（它们没有这一格）；
- 群名可变性（`rooms.name` 仍按既有规则，创建后不改）；
- 简介的历史版本读取 / 回滚 / 富文本 / 搜索 / 审核 / 封面图；
- 任何消息、投递、已读、通知语义；
- 房间头像、成员简介（Participant Profile / Room Presentation 见各自契约）。
