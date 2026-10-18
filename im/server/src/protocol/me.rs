//! GET /v1/me 响应模型（契约 6.2）。
//!
//! 本模型只包含公开上下文：participant + current session + request_id。
//! 不得携带 token、token hash 或任何其他 Session 秘密。

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ParticipantView {
    pub id: String,
    pub kind: String,
    pub display_name: String,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionView {
    pub id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MeResponse {
    pub participant: ParticipantView,
    pub session: SessionView,
    pub request_id: String,
}
