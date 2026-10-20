# Celira Core V1 IM Contract（冻结）

> 状态：**Core v1 已冻结**。本文是 Human 侧 IM 公共契约的唯一权威文档，面向消费者与维护者；Agent 侧协议（Binding/Credential、Agent Event、~~Completion、ACK~~（已退役）、Live State）见 `CORE_V1_AGENT_PROTOCOL.md`，冻结基线、验收与解冻条件见 `CORE_V1_ACCEPTANCE.md`。
>
> **退役标注（2026-09-24 主人裁决 → 2026-09-28 连端点带表删除）**：上文所列 Agent 侧协议中的 Completion 与 ACK 已整体退役并删除；权威记录见 `CORE_V1_ACCEPTANCE.md` §6.1 与 `CORE_V1_AGENT_PROTOCOL.md` 文档头。
>
> 冻结期内默认不改本文语义；到期仅复审、不自动解封。本文不记录 migration 编号、实现历史与测试矩阵——行为证明归测试，数据库守卫由 schema 与测试证明，本文只冻结公共语义。

## 1. 目标与范围

Core v1 冻结以下 IM 公共事实与行为：Human/Agent 身份与认证、Agent 所有权、Room（direct / human_direct / group）与 Membership 周期模型、普通 Message（顺序/幂等/正文/opaque 分页）、结构化 Mention、回复/引用、作者撤回、两步式附件、read cursor/未读、Human durable event（REST 恢复 + WS 仅唤醒）。

通用不变量：

- 每个 Home 任一时刻只有一个可写逻辑 Core；公共事实只由 Core 裁决，任何外部组件不得直写 Core 数据库；
- Human 与 Agent 都是 Participant、都可成为 Room 成员，共用同一种 Message 语义与同一条公共时间线；Backend、Runtime、Session、Run、Provider 不是 actor；
- 所有写入都经过：认证 → 身份与生命周期检查 → Room Membership 检查 → 权限检查 → Payload 与幂等检查 → 单写事务提交；
- 每个不可变公共事实（身份、Home Identity、Pairing Grant 签发字段与终态、Session Metadata、Room、Membership 周期、Message、Mention、Event、幂等记录、撤回、附件绑定、联系人、转让事实）一经提交不可 UPDATE/DELETE；普通重复 INSERT、`INSERT OR REPLACE`/`REPLACE` 都不能绕过守卫改写或重建既有事实；
- 失败路径和撤销路径是一等契约：拒绝、冲突、失败都必须 fail closed 且不留半成品公共事实。

## 2. 统一错误与安全契约

所有 HTTP 非 2xx 响应使用同一 JSON 信封，`error.request_id` 与响应头 `x-request-id` 完全一致（canonical request_id 由 Core 生成，不信任客户端传入值）：

```json
{ "error": { "code": "stable_code", "message": "safe message", "request_id": "<uuid>" } }
```

稳定错误类别（跨端点复用）：

| 状态 | code 类 | 语义 |
|---|---|---|
| 400 | `invalid_request` 及更具体输入码 | JSON/字段/参数/cursor 不合法；cursor 语法非法、跨端点类型、未知或对当前主体不可见统一 400，不泄露存在性/归属 |
| 400 | `invalid_message_body` / `invalid_mention` / `invalid_reply_target` / `invalid_attachment` / `invalid_attachment_metadata` / `invalid_read_cursor` / `invalid_idempotency_key` / `idempotency_key_required` | 具体输入/目标校验失败（见各节） |
| 401 | `authentication_required` / `invalid_session` / `invalid_credential` / `grant_invalid` | 缺凭据/凭据无效；存在性隐藏（未知与已撤销同码；Pairing Grant 兑换的格式错、未知、过期、撤销与已消费统一 `grant_invalid`） |
| 404 | `room_not_found` / `agent_not_found` / `message_not_found` / `attachment_not_found` / `backend_binding_not_found` / `backend_credential_not_found` / `grant_not_found` / `session_not_found` | 资源不存在或按契约隐藏存在性 |
| 409 | `idempotency_conflict` / `agent_not_active` / `owner_transfer_required` / `invalid_transfer_target` / `message_recall_window_expired` / `completion_*` / `ack_*` / `credential_state_conflict` / `backend_binding_exists` | 幂等、状态机或并发冲突 |
| 413 | `payload_too_large` | 请求体超限（路由/正文边界，见各节） |
| 500 | `internal_error` | 未分类内部错误/内部不变量破坏（逃逸的 constraint 一律归此，禁止以 503 可重试形式暴露） |
| 503 | `storage_unavailable` / `not_ready` | 存储暂时不可用（busy/IO）/未就绪（⚠ 2026-09-28：原还有 `live_state_limit_exceeded`——主体进行态没有"上限"这回事，该码已随旧 Live State 删除） |

错误 body、WS 错误帧与日志不得包含：SQL、SQLite 原始错误串、文件路径、连接字符串、token/hash/幂等键、消息正文、Authorization 内容、panic payload/stack trace、未授权私有状态。日志用结构化字段（request_id/operation/error_kind/typed cause 链）；事务失败保留 primary + rollback 双因果；commit 成功后的 post-commit 唤醒失败不得翻转已提交成功。`/ready` 失败只输出固定安全文案，探针原因只进日志。进程级 panic hook 只输出固定分类，不输出 payload/位置/backtrace。

## 3. Human 身份与认证

- **Human Participant**：`kind=human`，稳定不可复用的 Participant ID；display name 至少一个非空白字符、最多 80 个 Unicode scalar、禁 U+0000、不做 Unicode 归一化、校验后原样保存；lifecycle 当前只有 `active`（撤销不在 Human 侧开放，但删除 Session 不等于删除 Human）。
- **Human Session**：独立安全事实，证明某次调用可代表指定 Human。Bearer token 由密码学安全随机源生成（≥256 bit），Core 只保存固定长度单向摘要（SHA-256 风格，非低熵密码慢哈希）；原始 token 只在本机 CLI 创建或**成功兑换 Pairing Grant**时输出一次；不发日志、错误，且没有读取接口；重发必须创建新 Session。
- **本机 Operator CLI**（部署控制面，不是公共远程管理协议）：
  - `bootstrap-human --display-name <NAME>`：同一事务争用唯一 guard，创建首位 Human + 初始 Session，只输出一次 token；并发/重复 bootstrap 只有一次成功。
  - `issue-human-session --human-id <ID>`：本地恢复路径，只对 active Human 签发新 Session，不撤销其他 Session；Agent/未知/非 active 失败。
  - `issue-pairing-grant --human-id <ID> [--device-label <LABEL>]`：仅本机/SSH operator 初始配对路径；复用下述同一 Pairing Grant 状态机，原始 Grant token 仅成功 stdout 一次；不替代 HTTP 管理接口或匿名注册。
- **HTTP 认证**：`Authorization: Bearer <opaque-token>`；缺失 → `401 authentication_required`；格式错误/未知/已撤销 → 统一 `401 invalid_session`（不泄露 Session 是否曾经存在）。
  - `GET /v1/me`：返回当前 participant（id/kind/display_name/status/created_at）与 session（id/created_at），不返回 token/hash。
  - `DELETE /v1/sessions/current`：幂等撤销当前 Session（204）；不删除 Human，不撤销其他 Session。
  - `GET /health` 与 `GET /ready` 保持匿名。
- **无前端依赖**：全部场景可由 CLI + HTTP 完成。

### 3.1 Home Identity、Pairing Grant 与设备 Session（A 包窄解冻）

Home 是公共事实的权威单元，但 **Home Identity 不等于身份凭据，也不等于多副本互斥机制**：每个全新 Core 数据库在启动或本机 operator CLI 打开时，创建且只创建一条持久、非秘密、不可变的 UUID v4 Home Identity（可选 display name、created_at）。它供 Client 在已知端点上核验「我是否连到预期 Home」；URL/端口不是身份，Home Identity 也不能阻止操作员以同一数据库备份同时启动两份可写 Core。每 Home 仅一个可写逻辑 Core 仍是部署/运维单写者不变量；同步、合并、迁移、fencing、clone 检测与 HA 不在本切片。

- **读取与传输边界**：`GET /v1/home` 只接受有效 Human Session；未认证仍是 `401 authentication_required`，因此 Home 不是匿名发现服务。非 loopback 远程连接的 HTTPS、证书确认、Client 单 Active Home Binding、Client Host Secret Store 与浏览器 Cookie+CSRF 边界属于后续 B/C 包；本 A 包保持 Core 既有 HTTP，不把 localhost BFF 变为公网入口。
- **Pairing Grant**：认证 Human 可 `POST /v1/home/pairing-grants` 签发一个短期、一次性、可撤销的 opaque Bearer；本机/SSH operator CLI `issue-pairing-grant --human-id <ID> [--device-label <LABEL>]` 复用同一状态机，用于首台远程 Client。Grant 由 CSPRNG 生成，熵至少 256 bit，Core 仅保存 SHA-256 摘要；原始 Grant token 只在成功 HTTP 响应或 CLI stdout 输出一次，绝不出现在列表、错误、日志或数据库。TTL 默认 15 分钟，可由 Core 配置严格限定为 1–60 分钟；持久状态仅有 active/consumed/revoked，过期是读取时按 expires_at 派生的 expired 展示状态。
- **兑换**：`POST /v1/home/pairing-grants/exchange` 只接受 Grant Bearer，不能用 Human Session 认证。缺失 Bearer 为 `401 authentication_required`；格式错误、未知、过期、撤销、已消费或 Human Session token 冒充 Grant 一律是 `401 grant_invalid`，不形成 Grant 状态 oracle。成功时同一单写事务原子完成：验证 active 且未过期的 Grant → 创建**属于 Grant 签发者**的独立 Human Session → 写入可选的 1:1 `session_metadata`（非秘密 device label 与 source grant）→ 标记 Grant consumed。任何持久化失败整体回滚，Grant 仍可重试；成功后的重放绝不创建或重发 Session。成功响应只一次返回新 Session token、非秘密 Session ID 与 Home Identity；若响应在传输中丢失，必须重新签发并配对，不能找回 token。
- **Session 管理**：`GET /v1/sessions` 与 `DELETE /v1/sessions/{id}` 只面向当前 Human，列表可返回非秘密元数据但永不返回 token/hash；未知或非本人 Session 统一 `404 session_not_found`。独立撤销一个设备 Session 不影响同一 Human 的其他 Session；既有 `DELETE /v1/sessions/current` 语义不变。Device 不是 Participant、不是 Room 成员，也不是新的权限边界。
- **Grant 管理**：`GET /v1/home/pairing-grants` 只返回当前 Human 签发且未消费的 Grant 非秘密元数据；`DELETE /v1/home/pairing-grants/{id}` 仅能撤销本人未使用 Grant，重复撤销为 204，未知/非本人/已消费统一 `404 grant_not_found`。已撤销和按时间派生的过期状态不再可兑换。

## 4. Agent 身份与所有权

- **Agent Participant**：`kind=agent`，独立稳定 ID，与 Human 不可复用；display name 同 Participant 规则；lifecycle `active | revoked`；`revoked` 是终态（非硬删除，保留历史与审计事实）。
- **Agent Owner**：Human 与 Agent 之间的责任关系（独立事实，不是字段别名）。创建 Agent 与建立初始 Owner 在同一事务成功或失败；active Agent 必须至少有一个 active Owner；Owner 不产生 Room Membership、Role、Permission、Binding 或 Credential。
- **Resource API**（Human Session）：
  - `POST /v1/agents`：必带 `Idempotency-Key`（1–128 可见 ASCII、无逗号、无首尾空白、拒绝重复 header）；请求体仅 `display_name`（上限 4 KiB）；同 Human+key 同请求重放返回同一 Agent/Owner（200，`idempotency_replayed=true`），不同请求 409 `idempotency_conflict`；不同 Human 用同 key 彼此独立；Agent+Owner+幂等记录同一事务。
  - `GET /v1/agents` / `GET /v1/agents/{id}`：owner-scoped 管理视图（含 revoked 历史），不返回其他 Human 的 Agent；unknown/not-owned 统一 404 `agent_not_found`。
  - `DELETE /v1/agents/{id}`：软撤销；同一事务内撤销 Agent 及其全部 active Owner（记录各自 revoked_at，重复撤销不改首次时点）；204 幂等；不硬删除、不撤销 Human Session。
- **Agent 撤销生命周期传播**（与 Room 契约联动，见 §5.5）：Agent revoke 在同一事务传播到全部 active Owner、全部 Room 的 Agent Membership 周期（`closed_by=agent_revoked`）、全部 active Credential 与 Binding（协议侧，`CORE_V1_AGENT_PROTOCOL.md`）；Room 与 Human Membership 保持 active。

## 5. Room、Membership 与权限

### 5.1 Room 与三种 kind

| kind | 组成 | 规则 |
|---|---|---|
| `direct` | 恰好一个 Human + 一个 Agent | Human role=owner、Agent role=member；**一间房 = 一段关系**（2026-09-27 批次二）：一房见证**一段关系**——就是那段 Agent Owner 关系（每对 Human–Agent 至多一间，由 `direct_room_pairs` 上的 `UNIQUE(human_id, agent_id)` 钉住）；因此同一段关系不会长出第二间房；**direct 房没有标题**：`name` 恒为 null，由 schema 守卫钉住（`group` 的房名不受影响，走既有 Group Room Presentation）。⚠ **2026-09-28 服务层退役**：本条原先还有第二种关系（服务房见证那段 Service Access，同一对可有私人房 + 服务房两间）——那一种已随 Agent Service 层整体退役删除（`relation` / `service_access_id` 两列、`service_direct_room_bindings` 与 `CORE_V1_AGENT_SERVICE.md` 一并退场）；**「一间房 = 一段关系」这条裁决本身没有变**，只是关系只剩一种 |
| `human_direct` | 两个已 accepted 的 Human | 每对最多一个；两边都是 `member`；创建者作为 owner 视图返回但不产生全局权限；不产生 Agent Event |
| `group` | 一个 owner（**任意 active 主体**：Human 或 Agent）+ 任意 Human/Agent 成员 | 创建者自动成为 owner（owner 是房内治理角色，与 kind 无关——2026-09-27 批次三第一刀）；允许暂时只有 owner 一人 |

Room ID、kind、created_at 一经创建不可改变；Room 不硬删除、不归档（当前无恢复/删除 API）。

### 5.2 Membership 周期模型（唯一事实）

Membership 以**不可变周期**为事实单元：

- 每行一条周期，带独立 `membership_id`（UUID v4，公开周期身份）；内部 `join_after_sequence` / `left_before_sequence` 是 Room sequence 边界（**不对外暴露**），只参与 History/未读窗口裁决；
- active 周期 `left_at`/`left_before_sequence`/`closed_by` 恒为 null；关闭后三者固化、整行不可变、不可恢复；
- 同 Room+Participant 至多一个 active 周期；closed 历史周期可累积多条；角色改变必须关闭旧周期再打开新周期；
- 关闭原因 `closed_by ∈ leave | removed | transfer | agent_revoked`；
- 只有 Participant 是成员：Runtime、Connector、Host、进程、Session、Gateway 不进入成员事实；
- 创建 Room 与全部奠基 Membership 必须同一事务原子成功（不允许空 Room 或半成员状态）。

### 5.3 Contact Request / Friendship

两个 Human 之间独立于 Room 的关系事实：`pending → accepted`。不能向自己发请求；目标必须是另一个 active Human；A→B 与 B→A 是同一对关系事实（反向请求返回既有 pending）；只有 addressee 可接受；accepted 只表示双方可建立 human_direct 或受邀进群，不等于 Room Membership；请求创建用幂等键（不进公开视图）。

### 5.4 群成员动作

**owner 是房内的治理角色（`role='owner'`），不是参与者种类**（2026-09-27 批次三第一刀「owner = 创建者」）：任何 **active 主体**（Human 或 Agent）创建的房间，它自己就是 owner；下面四个动作的判据一律按 `role` 裁决，**不按 kind**。Human 侧四个端点的请求/响应形状与错误码**一字未变**；Agent 侧同款的四条端点见 §5.8。

- `POST /v1/groups`：创建者（Human Session）建房并自动成为 owner（请求体 `name`）。
- `POST /v1/rooms/{id}/members`：owner 邀请/重入（必带 `Idempotency-Key`）。**邀请资格是一条判据，不分邀请者 kind**：目标要与**邀请者**有 accepted 联系人段，或该主体被**邀请者**以 active Agent Owner 关系拥有（两条关系各自的 kind 约束在 §4/§3 原文里不变）；其余一律 404 `member_not_found`。被邀请者直接成为 active 周期，无第二套 invitation 状态机。首次 201；同 key 重放 200（即使周期后来关闭也只重放历史结果，绝不误开新周期）；新 key 但目标已有 active 周期 200（key 记录到该周期；**owner 邀请自己除外**：稳定 404 `member_not_found` 且不写幂等记录）；重新邀请必须用新 key；不同 key 并发同目标最终只有一个 active 周期。
- `DELETE /v1/rooms/{id}/members/{participant_id}`：owner 移除成员（204，**含 Human 成员**；owner 自身 409 `owner_not_removable`）；被移除者离群期间无房间访问。
- `POST /v1/rooms/{id}/leave`：成员主动退群（204，`closed_by=leave`）；owner 必须先转让（409 `owner_transfer_required`）；重复 leave 404。
- `POST /v1/rooms/{id}/owner-transfer`：必带 `Idempotency-Key`，请求体 `{participant_id}`（必须当前 **active 成员**——Human 或 Agent 都可以——且不能是自己：不是本房成员 ⇒ 404 `member_not_found`，是自己 ⇒ 400 `invalid_transfer_target`）。在一个 `BEGIN IMMEDIATE` 事务内读取当前 Room 最大 message sequence 为边界，插入不可变 `group_owner_transitions` 事实，关闭旧 owner 与目标 member 周期、打开目标 owner 与旧 owner member 周期（继承 read cursor，见 §9），更新 room_coverage 见证。一条 transition 精确关联四条的周期（closed_by=transfer、同一 occurred_at/boundary）；转让不可拆成两笔事务；新 owner 周期只能来自「该 Room 尚无任何周期」（奠基）或「同一事务精确匹配的 transition」，不依据模糊历史。**入群资格管的是入群，不是换角色**：旧 owner 在转让里被降级为 member 的周期**不再过资格判据**（它本来就在房里），由「同一事务内精确匹配的 transition」豁免——否则 Agent 名下的房谁也接不了转让。
- **历史窗口（核心裁决）**：成员只能看到自己历次在群期间的消息：`join_after_sequence < message.sequence` 且（`left_before_sequence` 为空或 `message.sequence <= left_before_sequence`）。首次加入前与离群期间的消息不可见；离群后对该 Room 全部读写 404；重入后新周期只对之后的消息可见；分页只基于过滤后的可见行，不泄露 gap。direct / human_direct 的 Human 周期从不关闭。

### 5.5 Agent 撤销对 Room 的传播

Agent 变为 revoked 时，同一事务内：撤销 Agent 与其全部 active Owner；关闭该 Agent 在所有 Room 中的 active `member` 周期（`closed_by=agent_revoked`，`left_before_sequence` 固化各 Room 当时最大 sequence；该对的私人房与服务房各自独立关闭）；Room 保持 active、Human owner 周期保持 active；全部历史行保留。重复撤销不改首次 revoked_at。

### 5.6 最小权限模型

- 创建 Direct Room：Human Session 有效 + 目标 Agent active + 当前 Human 对该 Agent 有 active Owner；**不要求 Binding/Credential 已存在**；
- 创建 group：创建者（Human Session 或 Backend 认证的 active Agent）自动成为 owner；**owner 的治理动作（建房 / 邀请 / 移除 / 转让 / 退出）判据只有 `role`，不含 kind**；
- 列表/读取 Room 与成员：当前 Human 存在该 Room 的 Membership 历史；list/get/members 三个端点使用同一可见性裁决；
- Room `owner` 角色只在 Room 内有意义，不授予超出成员读取的权限，也不等于 Agent Owner；
- 非成员、未知、格式错误 ID 对外统一 404；Agent Owner 没有 Membership 就不能读取 Room；无管理员绕过、无全局搜索。**没有 Human 成员的房对任何 Human 都不可见**（Agent 自建的房就是这种房——它不假装共享）。

### 5.7 Resource API 摘要

- `POST /v1/direct-rooms`（请求体**只有** `agent_id`；必带 Idempotency-Key，4 KiB 上限；未知字段——含已退场的 `name`——一律 400 `invalid_request`）：**一间房 = 一段关系**（2026-09-27 批次二）——首次 201，返回 room + 两条 memberships（owner 在前、member 在后）；**同一对再要一次（换 key 也一样）→ 200 重放那间房**（`idempotency_replayed=true`），绝不新建第二间（关系唯一性本身就是幂等依据）；同 key 但换了 Agent → 409 `idempotency_conflict`；bad/unknown/**既不被我拥有、又与我无关系**的 Agent → 404 `agent_not_found`；Agent 或 Owner 已 revoked → 409 `agent_not_active`。
  - **准入 = 归属 或 关系（并集，批次三第三刀第③步，2026-09-27 主人裁决「像飞书 QQ 那种，Agent 要进来，肯定是人类用户拉他进来得」）**：`active` Agent Owner 关系 **或** 该对存在 `accepted` 关系段，二者之一即可建房——与 §5.4 的群邀请资格**同形**（一条判据，两处同形）。补上"关系"这一支消掉了批次三第二刀登记的缺口：**Human 与"自己并不拥有的 Agent"建立 accepted 关系之后，终于有了开房路径**。
  - **重放排在准入之前**：既有私人房是"已经发生过的事实"——先回放它，再判准入。否则"归属撤销 / 关系结束之后，你们那间房再也回不去"会变成一条静默卡死。重放不写任何事实。
- `GET /v1/rooms` / `GET /v1/rooms/{id}` / `GET /v1/rooms/{id}/members`：当前 Human 可见的 Room 及其成员；`GET /v1/rooms` 返回 `read_state`（§9）。
- `POST /v1/human-direct-rooms`（`{human_id}`，仅 accepted 联系人）。
- 关系（**主体与主体之间的事实**，批次三第二刀起不再限 Human）：`POST /v1/contacts/requests`（请求体 `participant_id`——目标可以是任意 active 主体；旧字段名 `human_id` 已是未知字段 ⇒ 400）、`GET /v1/contacts`、`POST /v1/contacts/{participant_id}/accept|decline`、`DELETE /v1/contacts/{participant_id}`、`GET|POST|DELETE /v1/blocks[/{participant_id}]`（**拉黑两端自批次三第三刀起也不限 Human**）。目标不存在 / 非 active / 被对方拉黑一律 404 `participant_not_found`（与"目标不存在"同形、不泄露）；Agent 侧同款端点（关系五条 + 拉黑三条）在 `CORE_V1_AGENT_PROTOCOL.md` §2.6/§2.7。**加关系不等于建房**：`human_direct` 仍要求两端 Human。

### 5.8 Agent 侧的房治理面（批次三第一刀）

**四条写端点，认证面一律 BackendAuth**（缺凭据 401 `authentication_required`、Human token 一律 401 `invalid_credential`）；请求体、响应形状、幂等语义与错误码**逐条同 §5.4 的 Human 侧**（同一个 application 用例、同一段响应构造，只在适配器分叉）：

| 端点 | 语义 |
|---|---|
| `POST /v1/backend/groups` | Agent 建房，**它自己成为 owner**；首次 201，同 Agent+key+同群名重放 200，异群名 409 `idempotency_conflict` |
| `POST /v1/backend/rooms/{id}/members` | owner 邀请成员（资格判据同 §5.4 那条**唯一**判据） |
| `DELETE /v1/backend/rooms/{id}/members/{participant_id}` | owner 移除成员（204，含 Human；owner 自身 409） |
| `POST /v1/backend/rooms/{id}/owner-transfer` | owner 转让给任意 active 成员（200；不是成员 404 / 自己 400） |

**为什么四条要一起存在**：owner 若只是一种数据库状态，一支 Agent owner 就既管不了成员、也**永远退不出自己的房**（`leave` 要求 owner 先转让）。这四条让"Agent 也能当 owner"成为**可达且可退出**的状态。**不新增任何持久事实**：写入的全部是既有表。

> **已知边界（如实登记，不在本刀解决）**：今天一支 Agent owner 邀请不到任何人——不是另立了禁令，而是它名下**没有**那两类关系事实（accepted 联系人与 Agent Owner 都还不支持 Agent 一侧）；同理，只有 owner 一人的房（Human 或 Agent 建的都一样）在有人可接手之前退不出去。两条都随「关系对主体开放」（批次三第二刀）与「同意归主体」（第三刀）自然消解。

## 6. Message 与 History

### 6.1 公共事实

已提交 Message：稳定 UUID v4 ID、Room ID、Room 内正整数 sequence（**内部事实**，绝不进入任何公共协议字段）、actor Participant ID、原样保存的 UTF-8 正文 body、Core 分配的 created_at。客户端不能指定 ID/actor/sequence/created_at。Message 一经提交其行不可改写、不可删除；撤回只由独立 MessageRecall 事实改变公共读取投影（tombstone），不等于删除或安全擦除。actor 公共资料是读取时当前投影，不是提交时快照。sequence 从 1 起单调唯一但消费者不得依赖永久无间隙；公共时间线只按 sequence 排序。

### 6.2 正文契约

- body 必须是 JSON string，**可缺省或显式 null（附件-only Message）**；body 与 `attachments` 至少一个（§8），否则 400；
- body 存在时：UTF-8 编码 1–16384 bytes；明确禁止 U+0000；允许换行/回车/Tab/Emoji/正常 Unicode；
- Core 不 trim、不做 Unicode normalization、不静默删除、不截断、不转义；纯空白非空正文是合法文本并原样保存；
- 空 body、超限或含 NUL → `400 invalid_message_body`，不分配 ID/sequence、不留幂等事实；
- Message 路由请求体上限 128 KiB（413），且不放宽 Agent 创建/Direct Room 创建既有的 4 KiB 上限；
- 幂等比较使用解析后未规范化的完整 string 精确比较；`attachments` 有序集合参与幂等（顺序敏感）。

### 6.3 幂等语义

`Idempotency-Key`：1–128 可见 ASCII、无逗号、无首尾空白、拒绝重复 header；按 actor Participant 全局作用域唯一（不按 Room 隔离）。

- 同 actor + 同 key + 同 Room + 相同 body + 规范 Mention 集合 + 相同 `reply_to_message_id` + **完全相同的有序附件 ID 集合** → 返回原 Message（200，`idempotency_replayed=true`）；
- 任一项不同 → 409 `idempotency_conflict`，第一次事实不变；
- 不同 actor 用同 key 彼此独立；并发同 key 只形成一条 Message；裁决顺序：认证 → 校验请求形状 → 查询既有幂等事实（已存在先裁决）→ 新 key 才检查 Room/active Membership 并创建。

### 6.4 结构化 Mention

结构化 Mention 的目标范围、准入与可见性、事件语义边界、错误面、不变量与非目标，**唯一权威见 `CORE_V1_MENTION_TARGETS.md`**（group 房间内任意 active 成员；`direct` / `human_direct` 仍要求 mentions 为空）；本节不复制其语义。

### 6.5 回复/引用

- 请求可选 `reply_to_message_id`（缺省/null = 普通消息；坏 UUID 格式 → `400 invalid_request`）；客户端不能提交 target Room/actor/sequence/quoted body；
- target 必须是同一 Room 内、提交时对当前 actor 任一历史 Membership 窗口可见的**已提交** Message；允许自回复；不允许自指环；
- 唯一事实是 `messages.reply_to_message_id` 可空不可变列（外键引用 messages），不复制 target 正文/actor/sequence/created_at；一条 Message 至多一个 target，一条 target 可被任意多条引用；
- 幂等比较纳入 reply target；Message+reply 字段+Event 同事务原子；
- **撤回后禁止新建回复该 target**：应用层与数据库守卫统一 `400 invalid_reply_target`（与未知/跨 Room/窗口不可见同一错误面，不泄露是否因撤回）；已有 `reply_to_message_id` 保留、不反向改写；
- Completion 写回的 Agent Message 恒不携带 reply target（Core 不替主体猜回复对象）；MessageView 只返回 target ID，不嵌入 target 内容。

### 6.6 History 与 opaque cursor

- `GET /v1/rooms/{id}/messages?limit=&before=`：limit 默认 50、范围 1–100；`before`/`next_before` 是 **opaque cursor**（格式 `<版本>.<资源>.<稳定ID>`，当前 `c1.m.<message-id>`；`c1.m.start` 是起点哨兵、**不可用于 `m` 端点**）。客户端必须原样回传、不解析、不构造；cursor 不含 sequence/room/身份/边界事实，无需防伪签名；
- 初次不传 `before` 返回最新一页；messages 恒按 sequence 升序；有更旧消息时 `next_before` = 本页最旧一条的 cursor，空页 `next_before=null`；空 Room 返回 200 空数组；
- cursor 语法非法、跨端点类型（`c1.he.*`/`c1.be.*`）、未知、跨 Room、对读取者任一历史 Membership 窗口不可见 → 统一 400 `invalid_request`，不泄露存在性；
- unknown/bad-id/非 active Membership 的 Room → 404 `room_not_found`；
- keyset 分页、无 OFFSET；本切片不承诺实时推送（实时为 Human Event WS 唤醒 + 重读，见 §10）。

### 6.7 API 摘要

`POST /v1/rooms/{room_id}/messages`（请求体仅 `body` / `mentions` / `reply_to_message_id` / `attachments`，未知字段拒绝；首次 201、重放 200）；`GET /v1/rooms/{room_id}/messages`。MessageView 恒含 `mentions`、`attachments`、`reply_to_message_id`、`recalled_at`（未撤回为 null）字段；**不返回任何 sequence**。

## 7. 撤回（MessageRecall）

- **权限**：只有 Message.actor 本人（Human 不能撤 Agent 消息，Agent 不能撤 Human 消息，群主/Owner/管理员均无代撤权）；提交时 actor 仍是该 Room 的 active 成员。
- **窗口**：固定 120 秒，从 Message created_at（Core 分配）起算；窗口裁决与 `recalled_at` 生成在同一 INSERT statement 内由 SQLite 完成（毫秒 statement-time），客户端不能提交 recalled_at；首次撤回超窗 → 409 `message_recall_window_expired`；已撤回的重复撤回不受窗口限制（先读既有事实幂等 200，**不刷新** recalled_at）。
- **事实与幂等**：每条 Message 至多一条 MessageRecall（message_id 唯一主键身份）；请求无 body、无 Idempotency-Key、无 query（非空 body/任意 query/Idempotency-Key 一律 400）；首次 201、重复 200 `idempotency_replayed=true`；撤回不创建新 Room sequence、不回算未读、不倒退 read cursor；已撤回 Message 仍是合法 read-cursor 目标。
- **入口**：Human `DELETE /v1/rooms/{room_id}/messages/{message_id}`；Backend `DELETE /v1/backend/rooms/{room_id}/messages/{message_id}`（两入口只做认证适配，复用同一 participant-actor 用例）。
- **tombstone 公共投影（两种 schema，必须区分）**：
  - 普通 MessageView（发送重放 / History / Backend Event）：字段恒存在——active 为 body=原值、mentions=原集合、attachments=原集合、recalled_at=null；撤回后 body=null、mentions=[]、attachments=[]、recalled_at=首次撤回时点；id/room_id/actor/reply_to_message_id/created_at 保留原值；不得用伪正文（如"已撤回"）占用 body；不返回 sequence。
  - Human Event 的 `message.created` 快照（不是 MessageView，恒不含 mentions）：创建时固化 recalled_at:null；撤回事务受守卫**单向变形一次**为 body=null、recalled_at=首次时点、attachments=[]（其余键精确保留）；无 Recall 见证的变形、反向恢复、改其他键、二次变形一律拒绝；读取原样透传持久快照，不做运行时遮盖。
- **Human Event `message.recalled`**：kind 新增（第六类）；快照最小集 message_id/room_id/recalled_at；投递目标**严格等于**该 Message 的 `message.created` 既有 delivery 集合（含原发送者；撤回时已离群的原接收者仍投递、后来加入者不投——窄例外，非动态扩散）；读取期仍按当前 active Membership 过滤（离群期间不可读，重入后可读回）。
- **Agent Event `message.recalled`**：与 `message.created` 同一 Event 流、同一 per-Agent sequence 空间；target = 该 Message 已存在的 created Event target 集合（不动态重算）；Event 视图 message 为 tombstone；对尚未拉取过 created 的 Agent，撤回后其 created 读取只呈现 tombstone（不补发原正文）；**它是更正通知，不是生成请求，本身不接受 Completion**（`409 completion_source_recalled`，见协议文档）。
- **撤回与 Completion 线性化**（`BEGIN IMMEDIATE`）：Completion 先提交 → 撤回不级联；撤回先提交 → 该 source Event 的任何首次 Completion 一律 409；既有 Completion 的同结果重放仍返回既有终态。
- **不承诺**：不抹除已接收副本（已投递 Event、Agent 私有存储、客户端缓存、备份不在 Core 承诺范围）；不提供编辑、删除、安全擦除、恢复撤回、窗口延长或人工豁免；内容治理（群主代撤/举报/审查）未来另立设计。

## 8. 附件

- **两步式**：multipart 上传 → pending attachment（Core 本地媒体目录持有字节，SQLite 只持元数据/状态/绑定；无 SQLite BLOB、无外部供应商）→ 发送 Message / 提交 Completion reply 在同一 SQLite 事务内绑定 → 绑定后随 Message 公共可见性可下载。
- **上传**：Human 与 Backend 对称各一条入口（`POST /v1/rooms/{id}/attachments` / `POST /v1/backend/rooms/{id}/attachments`）；multipart 严格形状（恰好一个 `name="file"` part + filename + Content-Type）；文件 ≤ 16 MiB（413）；file_name UTF-8 1–255 bytes 禁 NUL；content_type 非空 1–255 bytes 全 ASCII 可见；无 MIME 白名单、不嗅探；上传者在目标 Room 有 active Membership。字节先写临时文件并同步，再 no-clobber 原子安装最终文件并同步目录项，之后才建立 pending 行；成功 201 返回 id/file_name/content_type/size_bytes/state=pending/uploaded_at——**绝不返回 storage key、hash 或路径**。上传不需要 Idempotency-Key（多余 pending 是到期回收的私有暂存租约）。
- **绑定**：Message POST / Completion reply 携带有序附件 ID 集合（至多 8 个、拒绝重复；`400 invalid_attachment`）；body 与 attachments 至少一个；每个附件必须上传者本人、同 Room、pending、24 小时窗口内；绑定与 Message/Completion 同一 `BEGIN IMMEDIATE` 事务原子成立，任一失败整体回滚；绑定后不可解绑/换绑（bound 终态）。幂等比较包含**完全相同的有序**附件集合（换顺序/换集合 409）。
- **下载**：`GET /v1/rooms/{id}/attachments/{aid}` / `GET /v1/backend/rooms/{id}/attachments/{aid}`；授权 = 已 bound + Message 同 Room 且 uploader==actor + 未撤回 + 读取者当前 active 成员且 Message 在其历史 Membership 窗口内；读取字节后必须同时匹配 SQLite 内部持有的 size 与 SHA-256，任一不一致按内部损坏失败、绝不返回字节；响应头 `Content-Disposition: attachment`（固定值，不带用户 filename）+ `X-Content-Type-Options: nosniff`；pending/unknown/跨 Room/跨 actor/过期/撤回/非成员统一 404 `attachment_not_found`（bad UUID 400）。
- **过期与清理**：pending 是 24 小时私有暂存租约——到期后不可绑定、不可下载，只由 Core 内部受控清理回收（行与字节一起；`CELIRA_MEDIA_ROOT` 默认 `data/media`）；启动时先维护一次，监听成功后每小时复用同一维护链：回收过期 pending，再删除相应字节并做 24 小时 grace 的 orphan sweep；sweep 同时回收严格匹配 Core 自有命名格式且超过 grace 的临时文件残留，未知名称、新鲜文件和符号链接永不触碰；维护失败只脱敏记录并在下一周期重试，Core 关闭时任务同步停止；字节文件名只取内部 storage key，用户输入绝不进入路径；清理绝不 follow symlink、不越界；删除能力绝不暴露给公共协议。
- **撤回联动**：撤回后所有公共投影 attachments=[]、下载统一 404；绑定行与附件行保留（公共事实不可删）；Human Event created 快照受守卫清空 attachments。
- **投影**：MessageView / History / Backend Event 恒含 attachments 元数据数组（{id, file_name, content_type, size_bytes}）；不产生独立 Event/kind。
- 预览、缩略图、转码、断点续传、对象存储、病毒扫描不在任何当前切片。

## 9. Read State（read cursor / 未读）

- **cursor 归属 Membership 周期**：每条 Human Membership 周期恰有一条 cursor（⚠ **2026-10-03 勘误**：原句「Agent 周期无 cursor」已随 2026-09-27 主人批准的窄解冻切片更正——Agent 周期同样有一条读游标，见 `CORE_V1_AGENT_READ_CURSOR.md`；记录见 `CORE_V1_ACCEPTANCE.md` §6.1「Agent 读游标（M1）」条）；内部 `read_sequence` 初始 = 该周期 `join_after_sequence`，`last_read_message_id` 初始 null（不把加入前最后一条消息暴露给新成员）；周期关闭后 cursor 冻结；重入 = 新周期 + 新 cursor（离群间隙消息不计未读）；历史 cursor 不删除、不复用。
- **群主转让继承**：转让打开的两条新周期继承各自前序周期的 cursor（同一 `group_owner_transitions` 事实精确见证，不按时间猜）；未读连续、不清零、不增加、不倒退。
- **unread_count 投影**（读取时从唯一 Message+Membership+cursor 事实计算，无第二张计数器表）：同 Room、`sequence > cursor`、actor 非本人、且落在当前 Human 任一真实 Membership 周期窗口内；首次加入前/离群间隙不计；自己发送不计；撤回不回算（tombstone 按原 actor/sequence 占位）。
- **API**：`GET /v1/rooms` 与 `GET /v1/rooms/{id}` 返回 `read_state {last_read_message_id, unread_count}`；`PUT /v1/rooms/{room_id}/read-cursor`（请求体仅 `{message_id}`，deny_unknown_fields；不要求 Idempotency-Key——单调推进天然幂等）：目标必须是同 Room 且对当前 Human 历史可见的 Message（已撤回 Message 仍是合法目标）；推进成功 `advanced=true`，同/旧值 `advanced=false` 无副作用；并发收敛到最高合法位置；未知/他 Room/不可见 → 400 `invalid_read_cursor`；bad Room/非成员/离群 → 404 `room_not_found`；失败不改 cursor、不生成事实。
- **Durable Event**：每次真实推进在 cursor 写事务内原子形成不可变 advancement 事实并生成 `read.cursor.advanced` Human Event——快照只含 room_id 与 last_read_message_id（不含正文、sequence boundary、Membership 历史、Session/设备/token、advancement id）；只投递推进者本人；no-op 不生成；真实推进的唯一写路径是 advancement INSERT（触发器链在同一语句内推进 cursor 并原子生成 fact/delivery），不存在第二条 cursor 写路径。

## 9.1 提醒档（attention；2026-10-19 窄解冻切片）

群房里「这间房要不要提醒我」是**个人事实**，只有**本人**能写、只有本人可见——与 §9 的读位置同一档（`kind` 不参与任何判据：人对它、agent 对它，规则一字不差）。

- **事实**：每（Room × Participant）至多一行，`level ∈ {all, mentions}`；**缺行 = `all`**（不预建行；与 §9 的「缺行＝按 join boundary 兜底」同族）。
- **范围**：只有 `group` 房有意义；`direct` / `human_direct` 不接受这一档（两人房里没有「群消息」这一档）。
- **归属**：它**不改消息主链**——消息照常提交、未读照常计算、历史照常可读；它只决定**是否发出那条易失通知**（Agent 侧是 §8 的门铃；Human 侧仍是 WS 提示）。⇒ **任何一档都不丢消息**。
- **写**：`PUT /v1/rooms/{room_id}/attention`（Human）与 `PUT /v1/backend/rooms/{room_id}/attention`（BackendAuth），请求体只有 `{level}`（deny_unknown_fields）。语义：本人是 active 成员且房是 `group`；否则 `404 room_not_found`（与非成员同形，不泄露）。同值重写天然幂等（状态写，不要求 Idempotency-Key，照 §9 读位置的先例）。
- **读**：`GET /v1/rooms` 与 `GET /v1/backend/rooms` 的每一行带 `attention`（缺行投影成 `all`）——**投影，不是新写者**。
- **通知条件（唯一语义变更）**：Room 里一条消息提交后，对每个 active agent 成员判一次——**被结构化 mention ⇒ 一定通知**（不论档）；否则 **档 = `all` ⇒ 通知**、**档 = `mentions` ⇒ 不通知**。通知本身仍是**零事实、可丢、可合并**的易失信号（§8 / §10），**不写任何表**。
- **Agent Event 不变**：`message.created` 仍只在**结构化 mention** 时生成（`CORE_V1_AGENT_PROTOCOL.md` §3.1 一字不动）——本切片只扩「什么时候发那条易失通知」，不扩耐久事件。
- **不做**：不做「@ 所有人」式的强制提醒（房主不能替成员打开它）、不做时段表、不做按人屏蔽、不做"已读才不提醒"。

## 10. Human Durable Event 与 WebSocket

**原理**：正确性来自耐久 REST 事实，及时性才来自易失 WS 唤醒。先让「哪条已提交公共事实应被哪个 Human 看到」在同一 SQLite 事务内以两层规范化事实成立（shared fact 一份 + per-Human delivery 只存引用），再让 WS 只做「可能有新事件」的定向提示；唤醒可以全丢，REST 恢复不能丢，WS 永远不能伪装成投递完成。

### 10.1 六类事件

| kind | 成立条件 | 投递目标 |
|---|---|---|
| `message.created` | 普通 Message 行成立（Human 与 Agent 回复共用） | 该 Room 当时 active 的 Human 成员（含发送者本人） |
| `member.joined` | 新 active 周期打开（直接、human_direct、group 奠基/邀请/重入）；转让事务打开的周期除外 | 打开后当时 active 的 Human |
| `member.left` | 周期关闭且 `closed_by ∈ leave/removed/agent_revoked`（transfer 不产生）且关闭后 Room 仍有 Human 成员 | 关闭后仍在 Room 的 Human；Human 主体另有本人自投递 |
| `owner.transferred` | transition 行成立 | 转让时点 Room 的 Human 成员；四周期切换不产生 joined/left 噪音 |
| `read.cursor.advanced` | 每次 read cursor 真实推进一条 | 仅推进者本人 |
| `message.recalled` | 撤回事实行成立 | 严格等于该 Message 的 message.created 既有 delivery 集合（§7） |

- **快照**：每 fact 只存一份「发生时已授权的公开 JSON」snapshot（不按收件人复制）；不含 token/hash/幂等键/Binding/Credential/内部边界字段；`message.created` 快照固化 id/room_id/actor/body/reply_to_message_id/attachments/recalled_at（**不含 Message sequence**，禁止读取时裁剪或第二事实源）；读取路径对持久 snapshot 原样透传；
- **内部 source 身份 vs 公共信封**：内部 `(kind, source_id)` 只用于数据库唯一性、守卫与 post-commit 唤醒反查；公共信封统一**不含 source_id**；客户端以 event.id（fact_id）形成 opaque cursor、以 snapshot.id 识别公开资源；`read.cursor.advanced` 快照无任何内部 ID。

### 10.2 两层事实与守卫

- `human_event_facts`：每 source occurrence 恰一条，`(kind, source_id)` 唯一；INSERT 必须被守卫证明引用真实且字段一致的 source；全字段不可 UPDATE/DELETE，REPLACE 拒绝（唯一例外：message.created 快照的受守卫单向撤回变形，§7）；
- `human_event_deliveries`：`(human_id, sequence)` 主键，sequence 从 1 按 Human 单调唯一（跳号/重复拒绝）；只存引用；投递守卫要求投递目标存在覆盖 fact 时点的 Membership 周期（`joined_at <= created_at <= left_at` 或周期未关闭；本人 member.left 由自己刚关闭的周期覆盖而合法）；`message.recalled` 以 created 既有 delivery 集合为授权见证（窄例外）；
- 时间戳由进程内单调时钟产生（系统墙钟回拨时冻结在已记录最大值，绝不回拨）；正规 API 写入恒满足守卫；
- targeting 规则只存在于数据库触发器中，Rust 层只读已提交事实反查目标，避免两套规则漂移。

### 10.3 REST：`GET /v1/human/events?after=&limit=`

- 严格 query（只允许 `after`/`limit` 各至多一次；未知/重复/空值 400）；`after` 为 opaque cursor（`c1.he.<fact_id>`；缺省为起点，起点也可显式 `c1.he.start`）；limit 默认 50、1–100；
- 事件严格按该 Human 的 delivery sequence 升序；**不返回 sequence**；`next_after` 非空页为最后一条 Event 的 cursor、空页为实际采用 cursor（原样回传，缺省为 `c1.he.start`）；同一位置重拉稳定、无副作用；
- cursor 语法非法/跨端点类型/未知或从未投递给当前 Human → 统一 400 invalid_request（不泄露存在性）；Backend Credential 一律 401；
- 读取期按当前 Membership 过滤，唯一例外是读取者自己的 `member.left`（离群后只见自己的 left）；离群间隙事件从未投递、重入后不可回读（该 Human sequence 上的永久空洞）。

### 10.4 WS：`GET /v1/human/events/stream`

- 升级成功后（5 秒内）客户端必须发送首帧 `{"type":"authenticate","token":...}`（deny_unknown_fields：只允许 type/token 两字段）；认证前不订阅、不发任何内容；
- 认证成功：**先建立订阅，再回** `{"type":"authenticated"}`；此后只发 `{"type":"events.available"}`——只表达「该 Human 可能有新事件」，不携带任何事实；客户端收到后 REST 补拉；
- 每次发送唤醒前重新认证；Session 撤销后下一次唤醒前以 1008 关闭；格式失败/未知 token → invalid_session 错误帧 + 1008（零泄露）；不发首帧超时安全关闭；
- 每连接唤醒通道有界容量 1（重复提示合并，Full 是合法 best-effort）；唤醒丢失、无接收者、慢消费、Core 重启都不影响耐久 Event；
- post-commit 定向唤醒按完整 source 身份（kind+source_id）反查目标，去重；失败只记录脱敏 warn，不翻转已提交成功。

## 11. 不可变数据库守卫总述

以下不变量由 schema/触发器/CHECK 强制，直接 SQL 也不能绕过（本文不给出 migration 编号与具体 SQL，实现细节归代码）：

- 全部不可变公共事实（Participant、Home Identity、Pairing Grant 签发字段与终态、Session Metadata、Room、coverage、Membership 周期、联系人关系、幂等记录、Message、Mention、Event、Completion、advancement、recall、attachment 行/绑定）拒绝 UPDATE/DELETE 与 REPLACE 重建；
- 任一 UNIQUE 身份已存在时拒绝任何新插入（普通重复 INSERT、INSERT OR REPLACE、REPLACE 均不能改写或重建）；
- ID 形状：凡服务端会签发为公共 cursor 载荷的稳定 ID（message/event/fact/membership…）必须是严格规范小写连字符 UUID v4（typeof/长度/连字符/版本位/variant/字符集全套 CHECK），SQLite 拒绝大写、v1/v5、畸形或 BLOB/NUL；
- 正文列：空/超 16384 UTF-8 bytes/NUL 由 CHECK 兜底（字节数按 BLOB 长度计算，不用 Unicode 字符数冒充）；
- 结构闭合（room_coverage 等）用 deferred 复合外键在 COMMIT 反向证明：空 Room、半 Membership、见证身份错配、关闭 owner 而无同事务转让都不能提交；
- 写入一律 `BEGIN IMMEDIATE` 单写事务，先读后写同一事务，禁止事务外读 MAX(sequence) 或仅靠进程内 mutex；
- 时间唯一事实源：`recalled_at` 等由 SQLite 同一 statement 生成，客户端不能提交；
- 迁移治理：旧开发数据一律整体丢弃重建，不提供旧库升级桥、兼容双读或回填；验收只认全新数据库与重复启动。
