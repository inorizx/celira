//! Backend Binding / Credential HTTP handlers（契约 CORE_V1_BACKEND_BINDINGS.md 5.x/6.x）。
//!
//! handler 只调用 `application::backend` 用例与端口，不直接操作 SQLite；
//! 所有非 2xx 走统一 JSON 错误信封，body 的 request_id 与 x-request-id
//! 一致。管理端点只接受 Human Bearer Session；`GET /v1/backend/me` 只接受
//! Backend Credential，两通道互斥。

use axum::extract::rejection::{BytesRejection, FailedToBufferBody, JsonRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};

use crate::adapters::http::AppState;
use crate::adapters::http::agent_routes::{KeyError, idempotency_key_from_headers};
use crate::adapters::http::auth::CurrentAuth;
use crate::adapters::http::backend_auth::BackendAuth;
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::routes::{
    MAX_AGENT_CREATE_BODY_BYTES, error_response, store_error_response,
};
use crate::application::backend::{
    self as backend_application, BackendQueryError, CreateBindingError, CreateBindingOutcome,
    CredentialQueryError, IssueCredentialError, IssueCredentialOutcome,
};
use crate::domain::backend::{BackendBinding, BackendCredential};
use crate::protocol::backend::{
    BackendAgentView, BackendBindingView, BackendCredentialView, BackendMeResponse, BindingView,
    CreateBindingRequest, CreateBindingResponse, CredentialView, GetBindingResponse,
    IssueCredentialRequest, IssueCredentialResponse, ListBindingsResponse, ListCredentialsResponse,
};
use crate::protocol::error::codes;

/// Backend 切片路由：管理端点挂在 Agent 资源下；认证探针独立入口。
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/agents/{agent_id}/backend-bindings",
            get(list_bindings).post(create_binding),
        )
        .route(
            "/v1/agents/{agent_id}/backend-bindings/{binding_id}",
            get(get_binding).delete(delete_binding),
        )
        .route(
            "/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials",
            get(list_credentials).post(issue_credential),
        )
        .route(
            "/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials/{credential_id}",
            delete(delete_credential),
        )
        .route("/v1/backend/me", get(backend_me))
}

/// 提取 Idempotency-Key；HTTP 映射留给 handler，避免辅助函数把大型
/// `Response` 放进 `Result::Err`。
fn idempotency_key_or_error(headers: &axum::http::HeaderMap) -> Result<String, KeyError> {
    idempotency_key_from_headers(headers)
}

fn key_error_response(error: KeyError, request_id: &str) -> Response {
    match error {
        KeyError::Missing => error_response(
            StatusCode::BAD_REQUEST,
            codes::IDEMPOTENCY_KEY_REQUIRED,
            "missing Idempotency-Key header",
            request_id,
        ),
        KeyError::Invalid => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_IDEMPOTENCY_KEY,
            "invalid Idempotency-Key header",
            request_id,
        ),
    }
}

#[derive(Debug, Clone, Copy)]
enum JsonBodyError {
    TooLarge,
    Invalid,
}

/// 解析 JSON 请求体：超限 → 413，其余解析失败 → 400。
fn json_body<T>(body: Result<Json<T>, JsonRejection>) -> Result<Json<T>, JsonBodyError>
where
    T: serde::de::DeserializeOwned,
{
    match body {
        Ok(body) => Ok(body),
        Err(JsonRejection::BytesRejection(BytesRejection::FailedToBufferBody(
            FailedToBufferBody::LengthLimitError(_),
        ))) => Err(JsonBodyError::TooLarge),
        Err(_) => Err(JsonBodyError::Invalid),
    }
}

fn json_body_error_response(error: JsonBodyError, request_id: &str) -> Response {
    match error {
        JsonBodyError::TooLarge => error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            codes::PAYLOAD_TOO_LARGE,
            format!("request body exceeds {MAX_AGENT_CREATE_BODY_BYTES} bytes"),
            request_id,
        ),
        JsonBodyError::Invalid => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "invalid JSON request body",
            request_id,
        ),
    }
}

/// POST /v1/agents/{agent_id}/backend-bindings：创建 active Binding。
/// 首次创建 201；同 Human + 同 key + 同 Agent + 同 label 重放 200。
async fn create_binding(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(agent_id): Path<String>,
    request_id: RequestId,
    headers: axum::http::HeaderMap,
    body: Result<Json<CreateBindingRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    let Json(payload) = match json_body::<CreateBindingRequest>(body) {
        Ok(body) => body,
        Err(error) => return json_body_error_response(error, &request_id),
    };
    let key = match idempotency_key_or_error(&headers) {
        Ok(key) => key,
        Err(error) => return key_error_response(error, &request_id),
    };

    match backend_application::create_binding(
        state.backend.as_ref(),
        &auth.participant,
        &agent_id,
        &payload.label,
        &key,
    )
    .await
    {
        Ok(CreateBindingOutcome::Created { binding }) => (
            StatusCode::CREATED,
            Json(CreateBindingResponse {
                binding: binding_view(&binding),
                idempotency_replayed: false,
                request_id,
            }),
        )
            .into_response(),
        Ok(CreateBindingOutcome::Replayed { binding }) => (
            StatusCode::OK,
            Json(CreateBindingResponse {
                binding: binding_view(&binding),
                idempotency_replayed: true,
                request_id,
            }),
        )
            .into_response(),
        Err(CreateBindingError::InvalidIdempotencyKey) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_IDEMPOTENCY_KEY,
            "invalid Idempotency-Key",
            &request_id,
        ),
        Err(CreateBindingError::Label(err)) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            err.to_string(),
            &request_id,
        ),
        Err(CreateBindingError::AgentNotFound) => error_response(
            StatusCode::NOT_FOUND,
            codes::AGENT_NOT_FOUND,
            "agent not found",
            &request_id,
        ),
        Err(CreateBindingError::AgentNotActive) => error_response(
            StatusCode::CONFLICT,
            codes::AGENT_NOT_ACTIVE,
            "agent or owner relationship is revoked",
            &request_id,
        ),
        Err(CreateBindingError::BindingExists) => error_response(
            StatusCode::CONFLICT,
            codes::BACKEND_BINDING_EXISTS,
            "agent already has an active backend binding",
            &request_id,
        ),
        Err(CreateBindingError::IdempotencyConflict) => error_response(
            StatusCode::CONFLICT,
            codes::IDEMPOTENCY_CONFLICT,
            "idempotency key was already used with a different request",
            &request_id,
        ),
        Err(CreateBindingError::Store(e)) => {
            store_error_response("backend.binding.create", &e, &request_id)
        }
    }
}

/// GET /v1/agents/{agent_id}/backend-bindings：列出当前 Human 曾经授权给该
/// Agent 的全部 Binding（active + revoked）。
async fn list_bindings(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(agent_id): Path<String>,
    request_id: RequestId,
) -> Response {
    match backend_application::list_bindings(
        state.backend.as_ref(),
        &auth.participant.id,
        &agent_id,
    )
    .await
    {
        Ok(bindings) => Json(ListBindingsResponse {
            bindings: bindings.iter().map(binding_view).collect(),
            request_id: request_id.0,
        })
        .into_response(),
        Err(e) => query_error_response(e, &request_id.0),
    }
}

/// GET /v1/agents/{agent_id}/backend-bindings/{binding_id}：读取 Binding 管理
/// 视图；unknown/bad/not-under-agent 统一 404 backend_binding_not_found。
async fn get_binding(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path((agent_id, binding_id)): Path<(String, String)>,
    request_id: RequestId,
) -> Response {
    match backend_application::get_binding(
        state.backend.as_ref(),
        &auth.participant.id,
        &agent_id,
        &binding_id,
    )
    .await
    {
        Ok(binding) => Json(GetBindingResponse {
            binding: binding_view(&binding),
            request_id: request_id.0,
        })
        .into_response(),
        Err(e) => query_error_response(e, &request_id.0),
    }
}

/// DELETE /v1/agents/{agent_id}/backend-bindings/{binding_id}：幂等软撤销
/// Binding（同一事务内先撤销其 active Credential）；成功与重复撤销均 204。
async fn delete_binding(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path((agent_id, binding_id)): Path<(String, String)>,
    request_id: RequestId,
) -> Response {
    match backend_application::revoke_binding(
        state.backend.as_ref(),
        &auth.participant.id,
        &agent_id,
        &binding_id,
    )
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => query_error_response(e, &request_id.0),
    }
}

/// POST /v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials：
/// 首次签发或 CAS 轮换。首次成功 201 且唯一一次返回原始 token；幂等重放
/// 200 且 token=null。
async fn issue_credential(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path((agent_id, binding_id)): Path<(String, String)>,
    request_id: RequestId,
    headers: axum::http::HeaderMap,
    body: Result<Json<IssueCredentialRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    let Json(payload) = match json_body::<IssueCredentialRequest>(body) {
        Ok(body) => body,
        Err(error) => return json_body_error_response(error, &request_id),
    };
    let key = match idempotency_key_or_error(&headers) {
        Ok(key) => key,
        Err(error) => return key_error_response(error, &request_id),
    };

    let replaces = match payload.replaces_credential_id {
        crate::protocol::backend::ReplacesCredentialField::Null => None,
        crate::protocol::backend::ReplacesCredentialField::Id(id) => Some(id),
    };
    match backend_application::issue_credential(
        state.backend.as_ref(),
        &auth.participant,
        &agent_id,
        &binding_id,
        replaces.as_deref(),
        &key,
    )
    .await
    {
        Ok(IssueCredentialOutcome::Created { credential, token }) => (
            StatusCode::CREATED,
            Json(IssueCredentialResponse {
                credential: credential_view(&credential),
                token: Some(token.as_str().to_owned()),
                idempotency_replayed: false,
                request_id,
            }),
        )
            .into_response(),
        Ok(IssueCredentialOutcome::Replayed { credential }) => (
            StatusCode::OK,
            Json(IssueCredentialResponse {
                credential: credential_view(&credential),
                token: None,
                idempotency_replayed: true,
                request_id,
            }),
        )
            .into_response(),
        Err(IssueCredentialError::InvalidIdempotencyKey) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_IDEMPOTENCY_KEY,
            "invalid Idempotency-Key",
            &request_id,
        ),
        Err(IssueCredentialError::InvalidReplacesId) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "replaces_credential_id must be a UUID v4",
            &request_id,
        ),
        Err(IssueCredentialError::AgentNotFound) => error_response(
            StatusCode::NOT_FOUND,
            codes::AGENT_NOT_FOUND,
            "agent not found",
            &request_id,
        ),
        Err(IssueCredentialError::AgentNotActive) => error_response(
            StatusCode::CONFLICT,
            codes::AGENT_NOT_ACTIVE,
            "agent or owner relationship is revoked",
            &request_id,
        ),
        Err(IssueCredentialError::BindingNotFound) => error_response(
            StatusCode::NOT_FOUND,
            codes::BACKEND_BINDING_NOT_FOUND,
            "backend binding not found",
            &request_id,
        ),
        Err(IssueCredentialError::CredentialStateConflict) => error_response(
            StatusCode::CONFLICT,
            codes::CREDENTIAL_STATE_CONFLICT,
            "credential issue or rotation does not match current state",
            &request_id,
        ),
        Err(IssueCredentialError::IdempotencyConflict) => error_response(
            StatusCode::CONFLICT,
            codes::IDEMPOTENCY_CONFLICT,
            "idempotency key was already used with a different request",
            &request_id,
        ),
        Err(IssueCredentialError::Store(e)) => {
            store_error_response("backend.credential.issue", &e, &request_id)
        }
    }
}

/// GET /v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials：列出
/// 该 Binding 的 Credential 元数据历史（active + revoked）。
async fn list_credentials(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path((agent_id, binding_id)): Path<(String, String)>,
    request_id: RequestId,
) -> Response {
    match backend_application::list_credentials(
        state.backend.as_ref(),
        &auth.participant.id,
        &agent_id,
        &binding_id,
    )
    .await
    {
        Ok(credentials) => Json(ListCredentialsResponse {
            credentials: credentials.iter().map(credential_view).collect(),
            request_id: request_id.0,
        })
        .into_response(),
        Err(e) => credential_query_error_response(e, &request_id.0),
    }
}

/// DELETE .../credentials/{credential_id}：幂等软撤销一个 Credential；成功
/// 与重复撤销均 204；被撤销 Credential 立即不能通过 Backend 认证。
async fn delete_credential(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path((agent_id, binding_id, credential_id)): Path<(String, String, String)>,
    request_id: RequestId,
) -> Response {
    match backend_application::revoke_credential(
        state.backend.as_ref(),
        &auth.participant.id,
        &agent_id,
        &binding_id,
        &credential_id,
    )
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => credential_query_error_response(e, &request_id.0),
    }
}

/// GET /v1/backend/me：本切片唯一 Backend 认证探针。成功只返回被代表的
/// Agent 公共资料与 Binding/Credential 元数据；任何失败统一 401
/// invalid_credential（缺 header 为 authentication_required）。
async fn backend_me(auth: BackendAuth, request_id: RequestId) -> Response {
    let view = &auth.view;
    Json(BackendMeResponse {
        agent: BackendAgentView {
            id: view.agent.id.as_str().to_owned(),
            kind: "agent".to_owned(),
            display_name: view.agent.display_name.clone(),
            status: view.agent.status.clone(),
        },
        binding: BackendBindingView {
            id: view.binding.id.as_str().to_owned(),
            label: view.binding.label.clone(),
            status: view.binding.status.clone(),
        },
        credential: BackendCredentialView {
            id: view.credential.id.as_str().to_owned(),
            status: view.credential.status.clone(),
            created_at: view.credential.created_at.to_rfc3339(),
        },
        request_id: request_id.0,
    })
    .into_response()
}

/// 查询失败：Agent unknown/bad/not-owned 统一 404 agent_not_found；owned
/// Agent 下 unknown/bad/not-under-agent Binding 统一 404
/// backend_binding_not_found。
fn query_error_response(e: BackendQueryError, request_id: &str) -> Response {
    match e {
        BackendQueryError::AgentNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::AGENT_NOT_FOUND,
            "agent not found",
            request_id,
        ),
        BackendQueryError::BindingNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::BACKEND_BINDING_NOT_FOUND,
            "backend binding not found",
            request_id,
        ),
        BackendQueryError::Store(e) => {
            store_error_response("backend.binding.list_or_get", &e, request_id)
        }
    }
}

/// Credential 查询失败：在查询失败之上增加 404 backend_credential_not_found。
fn credential_query_error_response(e: CredentialQueryError, request_id: &str) -> Response {
    match e {
        CredentialQueryError::AgentNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::AGENT_NOT_FOUND,
            "agent not found",
            request_id,
        ),
        CredentialQueryError::BindingNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::BACKEND_BINDING_NOT_FOUND,
            "backend binding not found",
            request_id,
        ),
        CredentialQueryError::CredentialNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::BACKEND_CREDENTIAL_NOT_FOUND,
            "backend credential not found",
            request_id,
        ),
        CredentialQueryError::Store(e) => {
            store_error_response("backend.credential.list_or_get", &e, request_id)
        }
    }
}

fn binding_view(binding: &BackendBinding) -> BindingView {
    BindingView {
        id: binding.id.as_str().to_owned(),
        agent_id: binding.agent_id.as_str().to_owned(),
        created_by_human_id: binding.created_by_human_id.as_str().to_owned(),
        label: binding.label.as_str().to_owned(),
        status: binding.status.as_str().to_owned(),
        created_at: binding.created_at.to_rfc3339(),
        revoked_at: binding.revoked_at.map(|t| t.to_rfc3339()),
    }
}

fn credential_view(credential: &BackendCredential) -> CredentialView {
    CredentialView {
        id: credential.id.as_str().to_owned(),
        binding_id: credential.binding_id.as_str().to_owned(),
        status: credential.status.as_str().to_owned(),
        created_at: credential.created_at.to_rfc3339(),
        revoked_at: credential.revoked_at.map(|t| t.to_rfc3339()),
    }
}
