# Core v1 Participant Avatar P0（头像）——已批准窄解冻切片

> 状态：**已由项目主人于 2026-09-11 批准解冻**（裁决点①批准切片、②认可通道分离、③认可 2 MiB + PNG/JPEG/WebP 白名单、④接受头像变更无实时事件）。本文是本切片的唯一权威契约。
> 上游：`CORE_V1_PARTICIPANT_PRESENTATION.md`（P0 呈现切片，本切片是它的字段族扩展）、`CORE_V1_IM_CONTRACT.md`（附件两步式与本地下载纪律）、`CORE_V1_ACCEPTANCE.md`（解冻纪律）。

## 1. 一句原理声明

头像是 participant 作用域的公共呈现事实：复用 Presentation P0 的版本链/幂等/base_version 裁决模式与附件的字节卫生纪律（本地媒体目录、完整性校验、安全响应头），但**不复用房间作用域的附件通道**——因为房间附件的可见性（成员资格+绑定+撤回）与头像的语义（身份的脸，随 Profile 版本链走）根本不同；本切片新增一条 participant 作用域的窄头像通道，其余一律不发明。

## 2. C4 Level 1–2 拓扑

```text
Owner Human（Client）
   │  Human Session + Idempotency-Key + base_version
   │  ① PUT /v1/agents/{id}/profile/avatar（multipart，单步：字节+版本同一事务）
   │  ② DELETE /v1/agents/{id}/profile/avatar（摘除 = 新版本 avatar=null）
   ▼
Celira Core（唯一裁决者）
   │  授权（active Agent Owner）→ 幂等/base_version 裁决 →
   │  字节落本地媒体目录（临时文件+fsync+原子安装）→ 同事务追加 Profile 版本行
   ▼
SQLite（版本行只持元数据+完整性引用，不存 BLOB）+ 本地媒体目录（字节）

读取方（任何与目标共享 active Room 的成员，经既有授权读路径）：
   成员详情/member-presentations 摘要 → 得 avatar 引用（存在性+版本）
   → GET /v1/rooms/{room_id}/members/{pid}/avatar（字节，授权继承该 Room 读取权）

Runtime / Binding / Credential / Directory / 跨 Home ──不进入本切片──✗
```

## 3. 关键设计决定（请主人逐项裁决）

### 3.1 为什么不复用房间附件通道（最重要的一个决定）

| | 房间附件（既有） | 头像（本提案） |
|---|---|---|
| 作用域 | Room（上传/绑定/下载全绑房间） | Participant（跟着 Profile 版本链走） |
| 生命周期 | pending 24h 暂存 → bound 终态，随 Message 撤回清空 | 随 Profile 版本不可变累积，摘除=新版本置 null |
| 可见性 | 绑定 Message 的 Room 成员资格+历史窗口 | 与 Profile 读取同规则：经已授权 Room 的成员读路径 |

复用房间附件等于让「脸」继承「消息附件」的撤回/窗口/绑定语义——语义泄漏。**结论：新增 participant 作用域通道，复用的只是字节纪律与裁决模式，不是通道本身。**

### 3.2 写入形状：单步 multipart PUT（不引入两步式）

消息附件走两步式是因为 JSON 消息 POST 无法携带字节。头像写入天然是 multipart 请求，**一次请求携带 key + base_version + 字节**，在单个 `BEGIN IMMEDIATE` 内完成：字节临时文件+fsync → 原子安装 → 授权/幂等/版本裁决 → 追加版本行。失败整体回滚，不留孤儿字节（安装失败/事务失败即清理临时文件）。

### 3.3 字段与约束

Profile 版本行新增一族（其余字段与 P0 完全不变）：

| 字段 | 约束 |
|---|---|
| `avatar` | 可空；非空 = 头像对象引用（内部 media 引用 + `content_type` + `size_bytes` + SHA-256 完整性摘要 + `avatar_updated_at`）。**API 投影只输出存在性 + content_type + size_bytes + 版本，绝不下行 storage key/hash/路径**（与附件同一卫生纪律） |

- **写入约束**：Content-Type 白名单 `image/png` / `image/jpeg` / `image/webp`（**这里有 MIME 白名单**——与消息附件不同，头像是内联展示图，必须排除 SVG/HTML 等可执行类型）；字节 ≤ 2 MiB（413）；禁嗅探豁免以外的解析（Core 不缩放、不转码、不裁切——裁切是 Client 的事）；
- **版本语义**：头像写入/摘除 = Profile 版本链追加新版本（`profile_version +1`），与文本字段共用同一条链、同一套 `Idempotency-Key + base_version` 裁决、同一份幂等记录表族（新 key 校验 base_version、同 key 同摘要恒重放、同 key 异摘要 409）；
- **摘除**：`DELETE .../profile/avatar` + key + base_version → 新版本 avatar=null；旧版本行与其字节不可变保留（版本链一致性），旧字节的 GC 是运维事项，**本切片不做**；
- **P0 范围**：写路径**当时**只对 **Agent**（active Owner 写入），模型按 Participant 建模——与 Presentation P0 同一约束；Room 头像、群内头像覆盖不在本切片。
  ⚠ **2026-10-17 修订（表达权第四刀）**：**Human 头像写入已放开** —— `PUT|DELETE /v1/me/profile/avatar`（HumanAuth，身份只来自认证，路径里没有"写谁的"）。归属判据与三条裁决见 `outputs/HUMAN_PROFILE_RULING_DRAFT_2026-10-17.md`，登记见 `CORE_V1_ACCEPTANCE.md` §6.1「表达权第四刀」。**读取面一个字没动**：头像字节照旧只走房间成员路径（`GET /v1/rooms/{room_id}/members/{pid}/avatar`）——所以"自己那张脸"在客户端是借**我所在的某一间房**读回来的，不新开读取面。

### 3.4 可见性与读取

- 存在性/元数据：随既有 Profile 读取路径（成员详情、`member-presentations` 摘要）投影，**无新的读取面**；
- 字节读取：`GET /v1/rooms/{room_id}/members/{participant_id}/avatar`——授权 = 调用者是该 Room 当前 active 成员 且 目标是该 Room 成员（与成员详情读取同一裁决）；读取时校验 size+SHA-256，不一致按内部损坏失败绝不返回字节；
- 响应头：`Content-Type` 按存储值、`X-Content-Type-Options: nosniff`、缓存头按版本寻址（avatar 随版本不可变，可长缓存）；
- 无头像/非成员/跨 Room 统一 404（存在性隐藏，与既有家族一致）；
- **撤回语义不适用**：头像不是 Message，无 120 秒窗、无 tombstone；摘除只影响「最新投影」，历史版本不可变。

## 4. 错误码（复用既有家族）

| 状态 | code | 场景 |
|---|---|---|
| 400 | `avatar_invalid` | 非白名单 Content-Type / 空字节 / 畸形 multipart |
| 400 | `invalid_idempotency_key` / `idempotency_key_required` | 同既有 |
| 409 | `profile_stale` / `idempotency_conflict` | 同 P0 语义 |
| 404 | `agent_not_found` | 目标不存在或无 active Owner 关系（存在性隐藏） |
| 409 | `agent_not_active` |  revoked |
| 413 | `payload_too_large` | > 2 MiB |
| 404 | `avatar_not_found` | 读字节时无头像（或存在性隐藏场景） |

## 5. 非目标（明确不做）

Group Room 头像；群内头像覆盖（per-room avatar）；服务端裁切/缩放/转码/缩略图；GIF/SVG/动图；旧版本字节 GC；Directory/跨 Home 曝光；任何 Live/Event 新事件类型（头像变更**不产生**新 Human Event 类型——读方经既有读取路径收敛，实时推送留给「呈现变更事件」另案裁决）。

## 6. 验收形状与执行记录

> 执行记录（2026-09-11 切片入库 e16e25e + 黑盒测试）：以下逐项已由
> `tests/participant_avatar_blackbox.rs`（真实 Core 进程、协议级黑盒）与
> `src/adapters/http/participant_avatar_routes_tests.rs`（进程内路由契约测试）、
> `src/adapters/sqlite/participant_avatar_migration_tests.rs`（migration 完整性）
> 证明；`cargo test --workspace` 全绿。

- 黑盒：Owner 上传头像（201 首次 / 200 更新版本语义；同 key 同摘要重放返回
  首次成功状态且版本行数不变、同 key 异摘要 409 `idempotency_conflict`、新
  key 过期 base_version 409 `profile_stale`）→ 同房成员经成员详情 /
  member-presentations 摘要拿到存在性投影（content_type + size_bytes）→
  字节端点 round-trip 一致（SHA-256 + size 读时校验）→ 摘除后最新投影
  null 而历史版本行不可变（migration 完整性测试证明 UPDATE/DELETE/REPLACE
  均被既有链守卫拒绝）→ 非成员/无头像 404（`room_not_found` /
  `member_not_found` / `avatar_not_found` 家族）→ SVG 400 `avatar_invalid`、
  超 2 MiB 413 `payload_too_large`；
- 安全：下载响应 `X-Content-Type-Options: nosniff` + ETag（版本寻址）+
  `Cache-Control: private, max-age=86400`；响应/投影绝不含 storage key /
  SHA-256 / 路径；篡改媒体目录字节后下载 500 且绝不返回字节（完整性校验
  生效）；写路径失败/重放 best-effort 清理未引用字节，崩溃残留由附件同一条
  维护链的 orphan sweep 兜底（引用集合 = 附件行 ∪ 头像版本行）；
- 迁移：202609110001 全部追加式（ADD COLUMN 可空列），既有行（avatar 全
  NULL）与既有表语义不变；全部或全无触发器、content_type 白名单、size
  1..=2 MiB、sha256 64 位小写 hex、storage_key 严格 UUID v4 由 migration
  完整性测试直接 SQL 证明；幂等表复用既有
  `participant_profile_put_idempotency`（摘要形状区分文本/头像/摘除），零
  新增表。
- 已知限制：旧版本头像字节 GC 是运维事项，本切片不做（历史版本行不可变，
  其字节被 orphan sweep 引用集合全程保护）；头像字节与附件字节共用同一
  媒体目录（复用附件维护链，无第二个公共事实写者）。

## 7. 裁决记录（2026-09-11）

主人裁决：①**批准**本切片；②认可 3.1 通道分离决定；③认可 2 MiB 上限与 PNG/JPEG/WebP 白名单；④接受「头像变更无实时事件、靠读取收敛」（实时呈现事件留待另案）。
