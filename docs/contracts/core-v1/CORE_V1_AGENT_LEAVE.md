# Core v1 解冻切片 · Agent 退房（M2）

> **状态**：2026-09-27 主人批准并交付。窄解冻切片，登记在 `CORE_V1_ACCEPTANCE.md` §6.1 与根 `AGENTS.md`「已批准的解冻切片」。
> **口径（主人裁决原话）**：**「能退，规则统一」**——任何房里的成员都能退，一处规则。
> 本文件是本切片公共语义的**唯一权威位置**；实现细节归代码，行为证明归用例。

## 1. 症状 / 目标

`closed_by='leave'`（成员主动退出）本来就在词表里、**Human 早就能用**（`POST /v1/rooms/{room_id}/leave`），而 Agent 用不了：`room_memberships_agent_close_guard` 只放行 `agent_revoked`（撤销传播）与群里的 `removed` / `transfer`。

按本项目的判据（**不对称只许来自来历或处境**），这条限制没有来历或处境上的理由——它是**债**：Agent 被单方面放进房间（`POST /v1/direct-rooms`，无同意步骤），却只能被撤销，「在房里」这件事对 Agent 是单向的。

## 2. 规则（一条 + 一处例外）

**规则**：**非 owner 的成员周期，可以由它自己以 `closed_by='leave'` 关闭**——不分房 kind、不分参与者 kind。

**例外（一处）**：`human_direct` 房的出口归**关系模型**（`contact_ended` + 精确见证，见 `CORE_V1_CONTACT_REVERSIBILITY.md`）——那间房的成员周期是**一段关系的投影**，不是「我在一个房里」。因此 `human_direct` 一律 404。

**owner 一律先转让**（既有规则，本切片不动）：`409 owner_transfer_required`。在 `direct` 房里 Human **恒为 owner**（`direct_room_pairs.role_owner` 恒 `owner`），因此 Human 的 1:1 出口仍被这条既有规则挡住。

## 3. 端点

```
POST /v1/backend/rooms/{room_id}/leave
Authorization: Bearer <Backend credential>
（无请求体、无 query、无 Idempotency-Key）
```

**成功**：`204 No Content`，无响应体。该 Agent 自己那条 active 成员周期进入 closed 终态（`closed_by='leave'`、`left_at=now`、`left_before_sequence=该房当前最大 message sequence`）。

**与 Human 侧 `POST /v1/rooms/{room_id}/leave` 逐条同形**：同一条 application 用例（`leave_membership`）、同一个状态码、同一套错误码，只换认证面（BackendAuth）。这是本仓库既有的分叉纪律：**领域层与应用层 kind 无关，只在适配器分叉**。

### 错误

| 状态 | code | 何时 |
|---|---|---|
| 401 | `authentication_required` / `invalid_credential` | 缺 Authorization 头 / 凭据不被接受（含 **Human token 打本端点**） |
| 404 | `room_not_found` | unknown / bad-id / **不是该房 active 成员** / **`human_direct` 房** / 周期已关闭（含重复调用）——统一，不泄露存在性 |
| 409 | `owner_transfer_required` | 调用者是该房 owner（Agent 在群里成为 owner 时同理） |
| 5xx | `storage_unavailable` 等 | 存储不可用 |

**重复调用**：第二次 `404`（周期已关闭，不再是成员）——与 Human 侧同。**无幂等键**：本动作的幂等由「成员资格」承担，不由键承担。

## 4. 不变量

1. **只关自己那条周期**：不动别人的周期、不动房、不动消息、不动配对行、**不删任何行**；
2. **退出之后，该房对这位 Agent 彻底消失**：通讯录不再列它，历史/发言/读游标一律按「非成员」处理；
3. **事件面与 Human 退群同形状**：`member.left` 事实由既有触发器 `human_event_facts_member_left` 生成（它本来就与参与者 kind 无关，snapshot 里带 `participant.kind`），投递给关闭后仍在房内的 **Human**；主体自己（Agent）**没有** Human Event 收件箱，也不产生任何 Agent Event；
4. **退出不是删房、也不是结束关系**：房与历史都留着，Human（owner）照旧读得到成员名单（那位 Agent 的周期 `closed_by='leave'` 如实呈现）；对面可以再建一间新房——**「像 IM 那样删好友」是另一件事**（Human–Agent 的关系生命周期，见 §6）；
5. **owner 不可退**（先转让）；
6. **Human 侧行为不变**，除一处显式声明的变化：`POST /v1/rooms/{direct_id}/leave`（Human 恒为 owner）从 `404` 变为 `409 owner_transfer_required`——同一处 owner 规则的自然结果，不再是房 kind 特判；实质不变（Human 依然退不了 direct 房）。

## 5. 落地物

| 层 | 落地物 |
|---|---|
| migration | `202609270002_agent_leave_membership.sql`：重建 `room_memberships_agent_close_guard`，`NOT(...)` 里加 `leave` 分支（`role='member'` 且房 kind ≠ `human_direct`），其余两个分支逐字保留 |
| 存储 | `RoomStore::leave_membership`（名字不再撒谎：从 `leave_group_member` 改名）；房 kind 判据从「必须是 group」改为「**不是 human_direct**」 |
| 应用 | `application::room::leave_membership(store, &ParticipantId, room_id)`——一条用例，两个适配器共用（照 M1 对 `advance_read_cursor` 的先例） |
| HTTP | `adapters/http/backend_leave_routes.rs`（`POST /v1/backend/rooms/{room_id}/leave`） |
| 事件 | **零改动**（触发器 kind 无关 + `member.left` 白名单本来就含 `leave`） |

## 6. 非目标（明确不做）

- **不做「删好友」**：Agent 退出一间 direct 房**不等于结束这段关系**（对面可立刻再建）。Human–Agent 的关系生命周期（拒绝 / 解除 / 拉黑那一族）是**批次三**的内容：今天 Agent 既不能同意也不能拒绝，被人放进去就出不来——本切片只给出**最小出口**；
- **不放开 owner 退房**（「没有 owner 的房怎么办」是独立产品裁决）；
- **不引入拉黑**（`participant_blocks` 双方限 Human，不动）；
- **不给 Agent 同意 / 拒绝 / 被邀请的语义**；
- **不新增任何持久对象**（无新表 / 新列 / 新幂等记录 / 新事件 kind）；
- **不改 `agent_revoked` 传播路径、成员加入路径与消息主链**；
- **不改既有 Backend 消息 / 历史端点的非成员口径**：它们今天回 `403 not_room_member`，与本切片及 Human 侧的 `404` 不一致（审计编号 **C2**，2026-09-27 记账已裁：记为已知偏斜（照顾），不当违规）——本切片如实钉住现状，不顺手改；
- **不动 Runtime**：执行端的 `im_leave_room` 动词是另一刀。

## 7. 验收

1. 群：Agent 退房 204 → 通讯录不再列该房 → 历史按非成员处理（今天 403，见 C2）→ 第二次 404；
2. direct 房：Agent 退房 204 → 自己出局、**Human 留在房里**（成员名单里那位 Agent 的周期 `closed_by='leave'`）；
3. `human_direct`：一律 404（即使调用者是那间房 member 周期的持有人）；绕过存储层直接写同样的关闭也被守卫拒绝；
4. 非成员 / 未知房 / 形状非法：一律 404；
5. 认证：BackendAuth 专属（缺头 / 未知 token / Human token 一律 401）；
6. Human 侧：group member 204、group owner 409、human_direct 404 照旧；
7. **`member.left` 事实与投递**：存储层用例逐条数（fact 一条、投递给房内 Human、Agent 无 Human 收件箱）；
8. 门禁四件套全绿 + 独立进程黑盒。
