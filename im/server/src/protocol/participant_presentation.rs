//! Participant Presentation Resource API 请求与响应模型
//! （CORE_V1_PARTICIPANT_PRESENTATION.md）。
//!
//! 模型只输出已授权公共呈现事实：Profile 版本行、Room Presentation 版本行、
//! 幂等标记与 request_id。`published_by_kind` 是派生投影（来源
//! `published_by_id -> participants.kind`），在响应中输出但绝非第二个持久
//! 身份事实。不携带 Owner secret / Backend / Runtime / Member / Event 数据。

use serde::{Deserialize, Deserializer, Serialize};

use crate::protocol::agent::{AgentView, OwnershipView};
use crate::protocol::participant_avatar::AvatarView;
use crate::protocol::room::ParticipantMinView;

/// 可空编辑字段的 deserialize 辅助：要求字段在 JSON 中**显式出现**
/// （值为字符串或 `null` 都算出现）。serde 对 `Option<Option<String>>`
/// 遇到 `null` 会直接短路为外层 `None`，无法区分「缺字段」与「显式 null」；
/// 本辅助把呈现的值反序列化为内层 `Option<String>` 并包装为 `Some(...)`：
/// - 显式 `null` → `Some(None)`（= 空投影，与空串同为 NULL 事实）；
/// - 字符串值 → `Some(Some(s))`；
/// - 字段缺失（不含在 JSON 中）→ 由 `#[serde(default)]` 填外层 `None`，
///   路由据此 400 `invalid_request`（必填但可 null）。
fn required_optional<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

/// PUT /v1/agents/{agent_id}/profile 请求体。全部字段必填：
/// `base_version` 为整数（>= 0），两个描述字段可显式 `null`（= 空投影）。
/// 缺失字段（含 `deny_unknown_fields` 之外的一切未知字段）以 400 拒绝；
/// 显式 null 与空串都规范化为 NULL 事实。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutProfileRequest {
    pub base_version: i64,
    #[serde(default, deserialize_with = "required_optional")]
    pub short_description: Option<Option<String>>,
    #[serde(default, deserialize_with = "required_optional")]
    pub public_description: Option<Option<String>>,
}

/// PUT /v1/rooms/{room_id}/members/{participant_id}/presentation 请求体。
/// 全部字段必填；三个可空字段可显式 `null`。缺失字段与未知字段一律 400。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutPresentationRequest {
    pub base_version: i64,
    #[serde(default, deserialize_with = "required_optional")]
    pub room_display_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "required_optional")]
    pub room_role_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "required_optional")]
    pub room_role_description: Option<Option<String>>,
}

/// Profile 版本视图。`published_by_kind` 派生自 `published_by_id` 的
/// Participant kind（读取时 JOIN participants 或写时来自认证 Human），
/// 绝不冗余存储。`avatar` 是头像窄通道的公共投影（存在性 + content_type +
/// size_bytes；版本即本视图的 profile_version），无头像为 null。
#[derive(Debug, Clone, Serialize)]
pub struct ProfileView {
    pub participant_id: String,
    pub profile_version: u64,
    pub short_description: Option<String>,
    pub public_description: Option<String>,
    pub avatar: Option<AvatarView>,
    pub published_by_id: String,
    pub published_by_kind: String,
    pub published_at: String,
}

/// GET /v1/agents/{agent_id}/profile 响应：Agent 管理视图 + Owner 关系 +
/// 最新 Profile（无版本为 null）。
#[derive(Debug, Clone, Serialize)]
pub struct GetProfileResponse {
    pub agent: AgentView,
    pub ownership: OwnershipView,
    pub profile: Option<ProfileView>,
    pub request_id: String,
}

/// GET /v1/backend/profile 响应（CORE_V1_AGENT_READ_SURFACE.md §2.2）：Agent
/// 经 BackendAuth 读**自己**的最新公共 Profile（无版本为 null）。
/// 不接受路径中的 agent_id；不含 Owner / Binding / Credential / token。
#[derive(Debug, Clone, Serialize)]
pub struct GetSelfProfileResponse {
    pub profile: Option<ProfileView>,
    pub request_id: String,
}

/// PUT /v1/agents/{agent_id}/profile 响应：首次创建（版本 0 → 1）201、
/// 后续更新（追加版本）200、同 key 同摘要重放返回首次成功时的状态；
/// `idempotency_replayed` 区分真实重放。
#[derive(Debug, Clone, Serialize)]
pub struct PutProfileResponse {
    pub profile: ProfileView,
    pub idempotency_replayed: bool,
    pub request_id: String,
}

/// Room Presentation 版本视图。
#[derive(Debug, Clone, Serialize)]
pub struct PresentationView {
    pub room_id: String,
    pub participant_id: String,
    pub presentation_version: u64,
    pub room_display_name: Option<String>,
    pub room_role_name: Option<String>,
    pub room_role_description: Option<String>,
    pub published_by_id: String,
    pub published_by_kind: String,
    pub published_at: String,
}

/// GET /v1/rooms/{room_id}/members/{participant_id}/presentation 响应：
/// participant + 当前 Presentation（direct / human_direct 恒为 null）。
#[derive(Debug, Clone, Serialize)]
pub struct GetPresentationResponse {
    pub participant: ParticipantMinView,
    pub presentation: Option<PresentationView>,
    pub request_id: String,
}

/// PUT /v1/rooms/{room_id}/members/{participant_id}/presentation 响应：
/// 首次创建 201、后续更新 200、同 key 同摘要重放返回首次成功时的状态；
/// `idempotency_replayed` 区分真实重放。
#[derive(Debug, Clone, Serialize)]
pub struct PutPresentationResponse {
    pub presentation: PresentationView,
    pub idempotency_replayed: bool,
    pub request_id: String,
}

/// 成员详情（GET /v1/rooms/{room_id}/members/{participant_id}/profile）：
/// participant + Profile（无版本为 null）+ Presentation（direct 为 null）。
#[derive(Debug, Clone, Serialize)]
pub struct MemberProfileResponse {
    pub participant: ParticipantMinView,
    pub profile: Option<ProfileView>,
    pub presentation: Option<PresentationView>,
    pub request_id: String,
}

/// member-presentations 摘要单项：participant 最小资料 + Profile
/// short_description + avatar 存在性 + Presentation 的 room_display_name /
/// room_role_name / room_role_description_preview。绝不含 public_description /
/// full role description；preview 由读取时刻从 full stored room_role_description 生成
/// （max 512 UTF-8 bytes）。
#[derive(Debug, Clone, Serialize)]
pub struct MemberPresentationSummaryItem {
    pub participant: ParticipantMinView,
    pub short_description: Option<String>,
    pub avatar: Option<AvatarView>,
    pub room_display_name: Option<String>,
    pub room_role_name: Option<String>,
    pub room_role_description_preview: Option<String>,
}

/// GET /v1/rooms/{room_id}/member-presentations 响应。
#[derive(Debug, Clone, Serialize)]
pub struct GetMemberPresentationsResponse {
    pub members: Vec<MemberPresentationSummaryItem>,
    pub request_id: String,
}
