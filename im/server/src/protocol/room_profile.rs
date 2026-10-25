//! Group Room Profile（群简介）的公开请求 / 响应模型。
//!
//! 契约：CORE_V1_ROOM_PROFILE.md §3–§4。简介只暴露三格
//! `description` / `version` / `updated_at`；**不**暴露写入者、内部 sequence
//! 或任何历史版本。单房历史回执的房间头另有一个只带 `description` +
//! `version` 的窄投影（`room.profile`）。

use serde::{Deserialize, Serialize};

/// `PUT /v1/rooms/{room_id}/profile` 请求体。未知字段一律拒绝。
///
/// `description` 用 `Option` 区分「缺字段」与「合法正文」：缺字段 / `null`
/// 是请求形状错误（400 `invalid_request`）；存在但为空、纯空白、含 NUL、
/// 超过 2000 字节是正文错误（400 `invalid_profile_description`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutRoomProfileRequest {
    pub base_version: i64,
    pub description: Option<String>,
}

/// 群简介公共视图：正文 + 版本 + 最后更新时间（RFC3339）。
#[derive(Debug, Clone, Serialize)]
pub struct RoomProfileView {
    pub description: String,
    pub version: u64,
    pub updated_at: String,
}

/// `PUT /v1/rooms/{room_id}/profile` 成功响应：**统一 200**（首次与重放同
/// 形状），`idempotency_replayed` 区分同 key 同摘要重放。
#[derive(Debug, Clone, Serialize)]
pub struct PutRoomProfileResponse {
    pub profile: RoomProfileView,
    pub idempotency_replayed: bool,
    pub request_id: String,
}

/// 单房历史回执房间头里的简介窄投影：只有正文与版本（没有 `updated_at`）。
#[derive(Debug, Clone, Serialize)]
pub struct RoomProfileHeadView {
    pub description: String,
    pub version: u64,
}

/// 单房历史回执的顶层 `room` 头：房间身份三格 + 最新简介（无简介 / 非 group
/// 为 null）。它是**附加字段**：`messages` / `next_before` / `request_id`
/// 一字未动。
#[derive(Debug, Clone, Serialize)]
pub struct RoomHeaderView {
    pub id: String,
    pub kind: String,
    pub name: Option<String>,
    pub profile: Option<RoomProfileHeadView>,
}
