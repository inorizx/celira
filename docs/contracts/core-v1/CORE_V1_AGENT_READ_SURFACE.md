# Celira Core V1 Agent Read Surface（窄解冻切片）

> 状态：**已由项目主人批准解冻**（2026-09-25，依据 `outputs/CORE_ALIGNMENT_DECISION_DRAFT_2026-09-25.md` v5 §1.5 与 `outputs/CORE_ALIGN_KNIFE_01_AGENT_READ_SURFACE_EXECUTION_PROMPT.md`）。本切片属 Core v1 封板门禁下的窄解冻：**只新增只读公共端点**。
>
> 本切片**不修改**任何既有冻结语义：既有 Human 侧端点、Message、Event、Membership、Permission、Binding、Credential、Presentation 写入路径、Avatar 写入路径全部不受影响。**不新增任何 SQLite 表/列/migration**，**不新增任何持久事实**。
> ⚠ **2026-09-28 服务层退役**：原第 7 条端点 `GET /v1/backend/publication`（读自己的 Service Publication）已随 Agent Service 层整体删除，本切片现为 **6 条**；其余六条语义与响应形状一字未动。
> 本文冻结本切片的公共语义；实现细节归代码，行为证明归测试。

## 1. 目标与范围

Agent 是房间成员，此前却看不见同房有谁、也看不见自己的公共呈现（Human 早就有这些读取面）。本切片补齐这条对称：**Agent 经 Backend 协议读取「同一个房间里的人」与「自己」**。

这是批次三（「被拉进空间需要它自己同意」）的前置：**看不见成员的 Agent 无从同意**。

**原理声明**：既有的可见性谓词本来就是 participant-kind 无关的（`room_store.rs` 的成员裁决只查 active membership；`application/participant_presentation.rs` 的读函数参数类型已是 `ParticipantId`），所以本切片是**开门，不是造门**——全部端点复用 Human 侧**同一**用例与同一投影，不另立一套可见性。

## 2. 端点（6 条，全部 BackendAuth）

### 2.1 看见房间里的人（5 条）

| 方法 | 路径 | 语义 |
|---|---|---|
| `GET` | `/v1/backend/rooms/{room_id}/members` | 该房成员列表，投影与 Human 侧 `GET /v1/rooms/{room_id}/members` **逐字段相同** |
| `GET` | `/v1/backend/rooms/{room_id}/member-presentations` | active 成员 Presentation 摘要列表（preview 上限 512 UTF-8 bytes，同 Human 侧） |
| `GET` | `/v1/backend/rooms/{room_id}/members/{participant_id}/profile` | 某 active 成员的公共 Profile + 当前 Room Presentation |
| `GET` | `/v1/backend/rooms/{room_id}/members/{participant_id}/presentation` | 某 active 成员在该 Room 的 Presentation（direct / human_direct 恒 null） |
| `GET` | `/v1/backend/rooms/{room_id}/members/{participant_id}/avatar` | 某 Room 成员头像字节（响应头同 Human 侧：存储 content_type + `nosniff` + ETag + `private, max-age=86400`） |

### 2.2 看见自己（1 条）

| 方法 | 路径 | 语义 |
|---|---|---|
| `GET` | `/v1/backend/profile` | **自己**的最新公共 Profile（无版本为 `null`） |

⚠ **2026-09-28 服务层退役**：本条原为 2 条——第二条 `GET /v1/backend/publication`（**自己**的最新 Service Publication）已随 Agent Service 层整体退役删除（端点、协议类型与错误码一并退场）。本切片因此从 7 条端点变为 **6 条**。

**身份只来自 BackendAuth**：这一条**不接受**路径中的 agent_id——从形状上排除「读别人的」。响应不含 Owner、Binding、Credential、token/hash 或任何 Runtime 私有状态。

## 3. 可见性裁决（唯一裁决，不复制）

- **认证**：全部端点要求 active Credential + active Binding + active Agent（`BackendAuth`）。缺失 header → `401 authentication_required`；格式错误 / Human token / 未知 / 任一级 revoked → `401 invalid_credential`（同一文案，不形成存在性 oracle）。**Human Session token 一律不能通过这些端点。**
- **房间范围**：requester（该 Agent）必须对该 Room 存在 **active Membership 周期**；复用 `RoomStore::get_room_members` 的同一可见性查询，**不新增任何 Agent 专用可见性判据**。
- **成员范围**：target 必须与该 requester 同属该 Room 的 **active 成员**。
- **自己范围**：`/v1/backend/profile` 只读认证上下文所代表的 Agent 自身。

## 4. 错误面

| 情况 | 响应 |
|---|---|
| 缺 Authorization / 无效凭据 | `401 authentication_required` / `401 invalid_credential` |
| Room 未知、bad-id、requester 非该房 active 成员 | `404 room_not_found`（与 Human 侧同一判据） |
| target 未知、bad-id、非该房 active 成员 | `404 member_not_found` |
| 无头像 | `404 avatar_not_found` |
| 存储不可用 | 按既有安全错误边界（`503 storage_unavailable` / `500 internal_error`） |

**开放项（本切片不动，如实登记）**：Backend 侧的既有消息/历史端点对「非成员」返回 `403 not_room_member`（`CORE_V1_AGENT_HISTORY.md` / `CORE_V1_AGENT_SPEAK.md`），与本切片及 Human 侧的 `404` 不一致。该分叉（审计编号 **C2**）**已于 2026-09-27 批次一记账裁决：记为已知偏斜（照顾），不当违规**——两侧口径各自在权威契约里写清（Backend 消息/历史 = `403 not_room_member`；房间范围读取面与新写端点 = `404`），**不统一、不改既有端点**。**本切片取隐私保守的一侧（404，与 Human 侧一致）**。

## 5. 不变量

1. 本切片**不新增任何持久事实**：无新表、无新列、无 migration、无幂等记录、无事件 kind；
2. 全部投影**逐字段复用** Human 侧既有投影函数——不复制、不弱化、不新增字段；
3. 全部可见性裁决复用 Human 侧既有用例——**不存在第二套可见性规则**；
4. 读取**不产生**任何副作用：不写 cursor、不生成 Event、不推进投递位置；
5. Human 侧既有端点的语义与响应形状**逐字节不变**；
6. 呈现（Profile / Presentation / Avatar）的**写路径**本切片完全不碰——仍只对 active Owner（那是另一刀的裁决范围）。

## 6. 非目标

- 不加任何写路径（表达权归属属批次四）；
- ~~不给 Agent 房间 live 读取面~~（⚠ 2026-09-28 已由 V9 补上，并同日按主体形状重做：`GET /v1/backend/rooms/{room_id}/live` 与 Human 侧同一投影——见 `CORE_V1_AGENT_PROTOCOL.md` §6.3）；
- 不改 `agent_events`、不改通讯录 `since_count` 口径；
- 不做成员生命周期 Event（契约已登记为延期项）；
- 不改 `client/`、`agent/`、`im/connectors/` 任何代码。

## 7. 黑盒验收清单

在独立 Core 进程 + 真实 Mock Backend 上，经公开协议验证：

1. 六条端点在合法前提下返回与 Human 侧**同形**的投影；
2. Human Session token 打六条端点 → `401 invalid_credential`；
3. 无 Authorization → `401 authentication_required`；
4. 非成员 Agent 打房间范围端点 → `404 room_not_found`；
5. 目标非该房 active 成员 → `404 member_not_found`；
6. 无头像 → `404 avatar_not_found`；
7. revoked Agent 的 Credential → 全部端点 `401 invalid_credential`（fail closed）；
8. 读取前后 SQLite 行数不变（无副作用证明）；
9. `/v1/backend/profile` 不接受路径 id（形状约束）。

⚠ 清单里的条数是 **2026-09-28 服务层退役后**的现值（原 7 条 ⇒ 6 条：`GET /v1/backend/publication` 已删除）。
