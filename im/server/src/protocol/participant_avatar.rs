//! Avatar Resource API 请求与响应模型（CORE_V1_AVATAR_P0.md）。
//!
//! 模型只输出已授权公共呈现事实：头像公共投影（存在性 + content_type +
//! size_bytes；版本在 ProfileView.profile_version 上寻址）、幂等标记与
//! request_id。绝不携带 storage key、SHA-256、路径或任何 Runtime 私有数据。

use serde::{Deserialize, Serialize};

use crate::protocol::participant_presentation::ProfileView;

/// 头像公共投影：存在性 + content_type + size_bytes。版本（缓存/一致性
/// 寻址）由所在 Profile 版本行的 `profile_version` 表达；storage key /
/// SHA-256 / 路径绝不下行。
#[derive(Debug, Clone, Serialize)]
pub struct AvatarView {
    pub content_type: String,
    pub size_bytes: i64,
}

/// PUT/DELETE /v1/agents/{agent_id}/profile/avatar 响应：返回最新 Profile
/// 视图（含 avatar 投影——写入后 Some、摘除后 null）+ 幂等标记。PUT 首次
/// 创建 201、后续更新 200；DELETE 一律 200。
#[derive(Debug, Clone, Serialize)]
pub struct AvatarWriteResponse {
    pub profile: ProfileView,
    pub idempotency_replayed: bool,
    pub request_id: String,
}

/// DELETE /v1/agents/{agent_id}/profile/avatar 请求体：`base_version` 必填
/// （>= 0 整数），未知字段 400。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteAvatarRequest {
    pub base_version: i64,
}
