//! Room Resource API 请求与响应模型（契约 CORE_V1_DIRECT_ROOMS.md 7.x +
//! CORE_V1_HUMAN_ROOMS_AND_MENTIONS.md 5.3/5.4）。
//!
//! 模型只包含公开管理视图：Room 公共属性（group 与可命名 direct 含 name）、当前 Human 的
//! Membership 周期、对端 Participant 最小资料（direct / human_direct 有
//! 对端，group 为 null）与房间 Membership 周期列表。Membership 对外视图
//! 只暴露 membership_id、participant_id、role、joined_at、left_at、
//! closed_by；内部 sequence 边界（join_after_sequence /
//! left_before_sequence）不暴露。不携带 Message、Backend、Credential、
//! Agent Owner 或私有 Runtime 信息。

use serde::{Deserialize, Serialize};

/// POST /v1/direct-rooms 请求体。未知字段一律拒绝。
///
/// **没有 `name`**（批次二，2026-09-27 主人裁决）：direct 房没有标题——房的身份
/// 是它见证的那段关系。该 Agent 已经有一间私人房时，本请求**返回那间房**
/// （replay），不报错、不新开。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDirectRoomRequest {
    pub agent_id: String,
}

/// POST /v1/human-direct-rooms 请求体。未知字段一律拒绝。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateHumanDirectRoomRequest {
    pub human_id: String,
}

/// POST /v1/groups 请求体。未知字段一律拒绝。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateGroupRequest {
    pub name: String,
}

/// POST /v1/rooms/{room_id}/members 请求体。未知字段一律拒绝。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InviteMemberRequest {
    pub participant_id: String,
}

/// POST /v1/rooms/{room_id}/owner-transfer 请求体。未知字段一律拒绝。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferOwnerRequest {
    pub participant_id: String,
}

/// PUT /v1/rooms/{room_id}/read-cursor 与
/// PUT /v1/backend/rooms/{room_id}/read-cursor 的请求体（Human 与 Agent
/// **同一形状**，见 CORE_V1_AGENT_READ_CURSOR.md §2）。只接受 `message_id`；
/// `deny_unknown_fields` 拒绝 sequence、human_id、membership_id、
/// unread_count、updated_at 或设备字段（契约 CORE_V1_IM_CONTRACT.md §9）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutReadCursorRequest {
    pub message_id: String,
}

/// PUT /v1/rooms/{room_id}/attention 与
/// PUT /v1/backend/rooms/{room_id}/attention 的请求体（Human 与 Agent
/// **同一形状**，契约 `CORE_V1_IM_CONTRACT.md` §9.1）。**只有 `level`**：
/// `deny_unknown_fields` 拒绝 participant_id、room_id、membership_id、
/// updated_at 或任何其他字段——"写谁的"由认证上下文给出，请求体里没有主语。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutAttentionRequest {
    pub level: String,
}

/// 提醒档写响应（契约 §9.1）：只回写后的权威取值。
///
/// 没有 `idempotency_replayed`——这一档是**状态写**（无幂等键），同值重写与
/// 改值都是同一个 200，响应形状不因"是不是重写"而变。
#[derive(Debug, Clone, Serialize)]
pub struct PutAttentionResponse {
    /// `all` / `mentions`。
    pub attention: String,
    pub request_id: String,
}

/// 对外 read state 视图：只暴露 `last_read_message_id`（可 null）与
/// `unread_count`；不返回内部初始 boundary / cursor sequence。
#[derive(Debug, Clone, Serialize)]
pub struct ReadStateView {
    pub last_read_message_id: Option<String>,
    pub unread_count: i64,
}

/// Room 公共视图。group 与可命名 direct 携带 name，human_direct 恒为 null。
#[derive(Debug, Clone, Serialize)]
pub struct RoomView {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub name: Option<String>,
    pub created_at: String,
}

/// Membership 周期管理视图：只暴露公开周期身份与时间展示（joined_at /
/// left_at），不暴露内部 sequence 边界。active 周期 left_at/closed_by 为
/// null；closed 周期（leave / removed / transfer / agent_revoked）三者
/// 固化。group 成员列表只列 active 周期；direct / human_direct 保持配对
/// 成员展示（Agent revoke 后 closed 的 Agent 周期仍可见）。
#[derive(Debug, Clone, Serialize)]
pub struct MembershipView {
    pub membership_id: String,
    pub participant_id: String,
    pub role: String,
    pub joined_at: String,
    pub left_at: Option<String>,
    pub closed_by: Option<String>,
}

/// Participant 的最小公共资料：id、kind、display name、status。
#[derive(Debug, Clone, Serialize)]
pub struct ParticipantMinView {
    pub id: String,
    pub kind: String,
    pub display_name: String,
    pub status: String,
}

/// POST /v1/direct-rooms 响应：201 首次创建 / 200 幂等重放，以
/// `idempotency_replayed` 区分。
#[derive(Debug, Clone, Serialize)]
pub struct CreateDirectRoomResponse {
    pub room: RoomView,
    pub memberships: Vec<MembershipView>,
    pub idempotency_replayed: bool,
    pub request_id: String,
}

/// POST /v1/human-direct-rooms / POST /v1/groups 响应（同一形状）。
#[derive(Debug, Clone, Serialize)]
pub struct CreateRoomResponse {
    pub room: RoomView,
    pub memberships: Vec<MembershipView>,
    pub idempotency_replayed: bool,
    pub request_id: String,
}

/// 列表中的单项：Room + 当前 Human 的 Membership 周期 + 对端 Participant
/// 最小资料（direct / human_direct 有对端，group 为 null）+ 当前 Human 的
/// read state + 「**对方读没读我最后那条**」（`peer_read`，契约
/// `CORE_V1_READ_RECEIPT.md` §2）+ **提醒档**（`attention`，契约
/// `CORE_V1_IM_CONTRACT.md` §9.1）。
#[derive(Debug, Clone, Serialize)]
pub struct ListRoomItem {
    pub room: RoomView,
    pub membership: MembershipView,
    pub peer: Option<ParticipantMinView>,
    pub read_state: ReadStateView,
    /// `"read"` / `"unread"`；**不出现**（`null`）＝ 问题不成立：房不是 `direct`、
    /// 对方当前不是这间房的 active 成员、或我在这间房还没说过话。
    pub peer_read: Option<String>,
    /// `"all"` / `"mentions"`：这间房的动静要不要提醒我。**缺行投影成 `all`**；
    /// 两人房里没有这一档，恒为 `all`。
    pub attention: String,
}

/// GET /v1/rooms 响应。
#[derive(Debug, Clone, Serialize)]
pub struct ListRoomsResponse {
    pub rooms: Vec<ListRoomItem>,
    pub request_id: String,
}

/// GET /v1/rooms/{room_id} 响应（不内嵌完整成员列表）。
///
/// `profile` 是 v1.1 附加切片的群简介（CORE_V1_ROOM_PROFILE.md §4.1）：
/// 无简介或非 group 恒为 null。列表端点 `GET /v1/rooms` 与 `RoomView`
/// **不带**这一格。
#[derive(Debug, Clone, Serialize)]
pub struct GetRoomResponse {
    pub room: RoomView,
    pub membership: MembershipView,
    pub peer: Option<ParticipantMinView>,
    pub read_state: ReadStateView,
    pub peer_read: Option<String>,
    pub profile: Option<crate::protocol::room_profile::RoomProfileView>,
    pub request_id: String,
}

/// 成员列表中的单项：Membership 周期 + 对应 Participant 最小资料。
#[derive(Debug, Clone, Serialize)]
pub struct MembersItem {
    pub membership: MembershipView,
    pub participant: ParticipantMinView,
}

/// GET /v1/rooms/{room_id}/members 响应：owner 在前，其余按 joined_at
/// 升序、再按 Participant ID 排序。
#[derive(Debug, Clone, Serialize)]
pub struct GetRoomMembersResponse {
    pub members: Vec<MembersItem>,
    pub request_id: String,
}

/// POST /v1/rooms/{room_id}/members 响应：201 创建/重入 / 200 同 key 重放
/// 或新 key 但目标已有 active 周期，以 `idempotency_replayed` 区分（同
/// key 重放为 true）。
#[derive(Debug, Clone, Serialize)]
pub struct InviteMemberResponse {
    pub membership: MembershipView,
    pub idempotency_replayed: bool,
    pub request_id: String,
}

/// POST /v1/rooms/{room_id}/owner-transfer 响应：200 转让完成或同 key
/// 重放，以 `idempotency_replayed` 区分；返回新 owner 的 Membership 周期
/// 视图。
#[derive(Debug, Clone, Serialize)]
pub struct TransferOwnerResponse {
    pub membership: MembershipView,
    pub idempotency_replayed: bool,
    pub request_id: String,
}

/// PUT /v1/rooms/{room_id}/read-cursor 响应：成功统一 200；`advanced=true`
/// 为真实推进（产生一条 durable read.cursor.advanced Human Event），
/// `advanced=false` 为目标不高于当前 cursor 的幂等 no-op；`read_state` 为
/// 当前权威 read state。
#[derive(Debug, Clone, Serialize)]
pub struct PutReadCursorResponse {
    pub read_state: ReadStateView,
    pub advanced: bool,
    pub request_id: String,
}
