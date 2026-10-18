//! Agent Resource API 请求与响应模型（契约 CORE_V1_AGENT_OWNERSHIP.md 5.x）。
//!
//! 模型只包含公开管理视图：Agent 公共属性、Owner 关系、幂等标记和
//! request_id。不携带 Backend、Credential、Room、Message 或私有 Runtime
//! 信息。

use serde::{Deserialize, Serialize};

/// POST /v1/agents 请求体。未知字段一律拒绝。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAgentRequest {
    pub display_name: String,
}

/// Agent 公共管理视图。
#[derive(Debug, Clone, Serialize)]
pub struct AgentView {
    pub id: String,
    pub kind: String,
    pub display_name: String,
    pub status: String,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

/// 当前 Human 对 Agent 的 Owner 关系视图。
#[derive(Debug, Clone, Serialize)]
pub struct OwnershipView {
    pub agent_id: String,
    pub human_id: String,
    pub status: String,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

/// POST /v1/agents 响应：201 首次创建 / 200 幂等重放，以
/// `idempotency_replayed` 区分。
#[derive(Debug, Clone, Serialize)]
pub struct CreateAgentResponse {
    pub agent: AgentView,
    pub ownership: OwnershipView,
    pub idempotency_replayed: bool,
    pub request_id: String,
}

/// 列表中的单项：Agent 及其 Owner 关系。
#[derive(Debug, Clone, Serialize)]
pub struct OwnedAgentView {
    pub agent: AgentView,
    pub ownership: OwnershipView,
}

/// GET /v1/agents 响应。
#[derive(Debug, Clone, Serialize)]
pub struct ListAgentsResponse {
    pub agents: Vec<OwnedAgentView>,
    pub request_id: String,
}

/// GET /v1/agents/{agent_id} 响应。
#[derive(Debug, Clone, Serialize)]
pub struct GetAgentResponse {
    pub agent: AgentView,
    pub ownership: OwnershipView,
    pub request_id: String,
}
