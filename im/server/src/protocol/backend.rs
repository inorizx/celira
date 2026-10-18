//! Backend Binding / Credential Resource API 与 Backend 认证 API 的请求与
//! 响应模型（契约 CORE_V1_BACKEND_BINDINGS.md 5.x / 6.x）。
//!
//! 模型只包含公开管理视图：Binding / Credential 元数据、幂等标记和
//! request_id。绝不携带原始 token、token hash、幂等键、Room、Runtime 或
//! Agent 私有状态；原始 token 只在新 Credential 首次成功创建响应中返回
//! 一次，幂等重放恒为 null。

use serde::de::{Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use std::fmt;

/// POST /v1/agents/{agent_id}/backend-bindings 请求体。未知字段一律拒绝。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBindingRequest {
    pub label: String,
}

/// Binding 公共管理视图。
#[derive(Debug, Clone, Serialize)]
pub struct BindingView {
    pub id: String,
    pub agent_id: String,
    pub created_by_human_id: String,
    pub label: String,
    pub status: String,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

/// POST .../backend-bindings 响应：201 首次创建 / 200 幂等重放，以
/// `idempotency_replayed` 区分。
#[derive(Debug, Clone, Serialize)]
pub struct CreateBindingResponse {
    pub binding: BindingView,
    pub idempotency_replayed: bool,
    pub request_id: String,
}

/// GET .../backend-bindings 响应（active + revoked 历史，无 Credential 信息）。
#[derive(Debug, Clone, Serialize)]
pub struct ListBindingsResponse {
    pub bindings: Vec<BindingView>,
    pub request_id: String,
}

/// GET .../backend-bindings/{binding_id} 响应。
#[derive(Debug, Clone, Serialize)]
pub struct GetBindingResponse {
    pub binding: BindingView,
    pub request_id: String,
}

/// 反序列化的 replaces 字段值外壳。
///
/// serde 对 `Option<T>` 字段缺省时隐式取 None，无法区分「字段缺失」与
/// 「显式 null」；契约要求字段必须存在，因此用自定义类型保证缺失字段
/// 触发反序列化错误（HTTP 400 invalid_request）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplacesCredentialField {
    Null,
    Id(String),
}

impl<'de> Deserialize<'de> for ReplacesCredentialField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Option::<String>::deserialize(deserializer)? {
            Some(id) => Ok(Self::Id(id)),
            None => Ok(Self::Null),
        }
    }
}

/// POST .../credentials 请求体。字段必须存在；非 null 必须是 UUID v4。
#[derive(Debug, Clone)]
pub struct IssueCredentialRequest {
    pub replaces_credential_id: ReplacesCredentialField,
}

impl<'de> Deserialize<'de> for IssueCredentialRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct IssueCredentialRequestVisitor;

        impl<'de> Visitor<'de> for IssueCredentialRequestVisitor {
            type Value = IssueCredentialRequest;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object with replaces_credential_id")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut replaces = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key != "replaces_credential_id" {
                        return Err(serde::de::Error::unknown_field(
                            &key,
                            &["replaces_credential_id"],
                        ));
                    }
                    if replaces.is_some() {
                        return Err(serde::de::Error::duplicate_field("replaces_credential_id"));
                    }
                    replaces = Some(map.next_value::<ReplacesCredentialField>()?);
                }
                let replaces_credential_id = replaces
                    .ok_or_else(|| serde::de::Error::missing_field("replaces_credential_id"))?;
                Ok(IssueCredentialRequest {
                    replaces_credential_id,
                })
            }
        }

        deserializer.deserialize_map(IssueCredentialRequestVisitor)
    }
}

/// Credential 公共管理视图（不含 token、hash、replaces 与幂等键）。
#[derive(Debug, Clone, Serialize)]
pub struct CredentialView {
    pub id: String,
    pub binding_id: String,
    pub status: String,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

/// POST .../credentials 响应：201 首次成功签发携带 `token`（原始 secret
/// 唯一出口）；200 幂等重放 `token` 恒为 null。Debug 永远遮蔽 token。
#[derive(Clone, Serialize)]
pub struct IssueCredentialResponse {
    pub credential: CredentialView,
    pub token: Option<String>,
    pub idempotency_replayed: bool,
    pub request_id: String,
}

impl std::fmt::Debug for IssueCredentialResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssueCredentialResponse")
            .field("credential", &self.credential)
            .field("token", &self.token.as_ref().map(|_| "[REDACTED]"))
            .field("idempotency_replayed", &self.idempotency_replayed)
            .field("request_id", &self.request_id)
            .finish()
    }
}

/// GET .../credentials 响应（active + revoked 历史）。
#[derive(Debug, Clone, Serialize)]
pub struct ListCredentialsResponse {
    pub credentials: Vec<CredentialView>,
    pub request_id: String,
}

/// GET /v1/backend/me 响应中的被代表 Agent 最小视图。
#[derive(Debug, Clone, Serialize)]
pub struct BackendAgentView {
    pub id: String,
    pub kind: String,
    pub display_name: String,
    pub status: String,
}

/// GET /v1/backend/me 响应中的 Binding 视图。
#[derive(Debug, Clone, Serialize)]
pub struct BackendBindingView {
    pub id: String,
    pub label: String,
    pub status: String,
}

/// GET /v1/backend/me 响应中的 Credential 视图。
#[derive(Debug, Clone, Serialize)]
pub struct BackendCredentialView {
    pub id: String,
    pub status: String,
    pub created_at: String,
}

/// GET /v1/backend/me 成功响应：只返回被代表的 Agent 公共资料、Binding 与
/// Credential 元数据。不返回 Owner、Room、Membership、Permission、Event、
/// token/hash、Runtime 配置或 Agent 私有状态。
#[derive(Debug, Clone, Serialize)]
pub struct BackendMeResponse {
    pub agent: BackendAgentView,
    pub binding: BackendBindingView,
    pub credential: BackendCredentialView,
    pub request_id: String,
}
