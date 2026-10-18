//! Agent lifecycle HTTP handlers（契约 CORE_V1_AGENT_OWNERSHIP.md 5.x）。
//!
//! handler 只调用 `application::agent` 用例与端口，不直接操作 SQLite；
//! 所有非 2xx 走统一 JSON 错误信封，body 的 request_id 与 x-request-id
//! 一致，不允许 Axum 默认纯文本错误绕开公共错误信封。

use axum::extract::rejection::{BytesRejection, FailedToBufferBody, JsonRejection};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use crate::adapters::http::AppState;
use crate::adapters::http::auth::CurrentAuth;
use crate::adapters::http::human_event_routes::wake_humans;
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::routes::{
    MAX_AGENT_CREATE_BODY_BYTES, error_response, store_error_response,
};
use crate::application::agent::AgentQueryError;
use crate::application::agent::{self as agent_application, CreateAgentError, CreateAgentOutcome};
use crate::domain::human_event::{HumanEventKind, HumanEventSource};
use crate::protocol::agent::{
    AgentView, CreateAgentRequest, CreateAgentResponse, GetAgentResponse, ListAgentsResponse,
    OwnedAgentView, OwnershipView,
};
use crate::protocol::error::codes;

/// Idempotency-Key 请求头名。
pub const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";

/// Agent 切片路由。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/agents", get(list_agents).post(create_agent))
        .route("/v1/agents/{agent_id}", get(get_agent).delete(delete_agent))
}

/// POST /v1/agents：创建 Agent 并建立创建者 Human → Agent 的初始 Owner。
/// 首次创建 201；同 Human + 同 key + 同请求重放 200。
async fn create_agent(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<CreateAgentRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;

    let Json(payload) = match body {
        Ok(body) => body,
        Err(JsonRejection::BytesRejection(BytesRejection::FailedToBufferBody(
            FailedToBufferBody::LengthLimitError(_),
        ))) => {
            return error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                codes::PAYLOAD_TOO_LARGE,
                format!("request body exceeds {MAX_AGENT_CREATE_BODY_BYTES} bytes"),
                &request_id,
            );
        }
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "invalid JSON request body",
                &request_id,
            );
        }
    };

    let key = match idempotency_key_from_headers(&headers) {
        Ok(key) => key,
        Err(KeyError::Missing) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::IDEMPOTENCY_KEY_REQUIRED,
                "missing Idempotency-Key header",
                &request_id,
            );
        }
        Err(KeyError::Invalid) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_IDEMPOTENCY_KEY,
                "invalid Idempotency-Key header",
                &request_id,
            );
        }
    };

    match agent_application::create_agent(
        state.agent.as_ref(),
        &auth.participant,
        &payload.display_name,
        &key,
    )
    .await
    {
        Ok(CreateAgentOutcome::Created { agent, ownership }) => (
            StatusCode::CREATED,
            Json(CreateAgentResponse {
                agent: agent_view(&agent),
                ownership: ownership_view(&ownership),
                idempotency_replayed: false,
                request_id,
            }),
        )
            .into_response(),
        Ok(CreateAgentOutcome::Replayed { agent, ownership }) => (
            StatusCode::OK,
            Json(CreateAgentResponse {
                agent: agent_view(&agent),
                ownership: ownership_view(&ownership),
                idempotency_replayed: true,
                request_id,
            }),
        )
            .into_response(),
        Err(CreateAgentError::InvalidIdempotencyKey) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_IDEMPOTENCY_KEY,
            "invalid Idempotency-Key",
            &request_id,
        ),
        Err(CreateAgentError::DisplayName(err)) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            err.to_string(),
            &request_id,
        ),
        Err(CreateAgentError::IdempotencyConflict) => error_response(
            StatusCode::CONFLICT,
            codes::IDEMPOTENCY_CONFLICT,
            "idempotency key was already used with a different request",
            &request_id,
        ),
        Err(CreateAgentError::Store(e)) => store_error_response("agent.create", &e, &request_id),
    }
}

/// GET /v1/agents：列出当前 Human 曾经拥有的 Agent（active + revoked），
/// 按创建时间升序、再按稳定 ID 排序；不泄露其他 Human 的 Agent。
async fn list_agents(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
) -> Response {
    match agent_application::list_agents(state.agent.as_ref(), &auth.participant.id).await {
        Ok(agents) => Json(ListAgentsResponse {
            agents: agents.iter().map(owned_view).collect(),
            request_id: request_id.0,
        })
        .into_response(),
        Err(e) => query_error_response(e, &request_id.0),
    }
}

/// GET /v1/agents/{agent_id}：读取当前 Human 曾经拥有的 Agent 管理视图。
async fn get_agent(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(agent_id): Path<String>,
    request_id: RequestId,
) -> Response {
    match agent_application::get_agent(state.agent.as_ref(), &auth.participant.id, &agent_id).await
    {
        Ok(owned) => Json(GetAgentResponse {
            agent: agent_view(&owned.agent),
            ownership: ownership_view(&owned.ownership),
            request_id: request_id.0,
        })
        .into_response(),
        Err(e) => query_error_response(e, &request_id.0),
    }
}

/// DELETE /v1/agents/{agent_id}：软撤销 Agent 与其全部 active Owner。
/// 首次与重复撤销都返回 204；重复撤销不改原 revoked_at。
async fn delete_agent(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(agent_id): Path<String>,
    request_id: RequestId,
) -> Response {
    match agent_application::revoke_agent(state.agent.as_ref(), &auth.participant.id, &agent_id)
        .await
    {
        Ok(closed_membership_ids) => {
            // post-commit 唤醒：Agent revoke 传播关闭的 Membership 周期
            // （member.left fact 的完整 source 身份）；best-effort。
            let sources: Vec<HumanEventSource> = closed_membership_ids
                .iter()
                .map(|id| HumanEventSource::new(HumanEventKind::MemberLeft, id.as_str().to_owned()))
                .collect();
            wake_humans(&state, &sources, &request_id.0).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => query_error_response(e, &request_id.0),
    }
}

/// 查询失败：unknown / bad-id / not-owned 统一 404 `agent_not_found`。
fn query_error_response(e: AgentQueryError, request_id: &str) -> Response {
    match e {
        AgentQueryError::NotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::AGENT_NOT_FOUND,
            "agent not found",
            request_id,
        ),
        AgentQueryError::Store(e) => store_error_response("agent.list_or_get", &e, request_id),
    }
}

pub(crate) fn agent_view(agent: &crate::domain::agent::Agent) -> AgentView {
    AgentView {
        id: agent.id.as_str().to_owned(),
        kind: agent.kind.as_str().to_owned(),
        display_name: agent.display_name.as_str().to_owned(),
        status: agent.status.as_str().to_owned(),
        created_at: agent.created_at.to_rfc3339(),
        revoked_at: agent.revoked_at.map(|t| t.to_rfc3339()),
    }
}

pub(crate) fn ownership_view(ownership: &crate::domain::agent::AgentOwner) -> OwnershipView {
    OwnershipView {
        agent_id: ownership.agent_id.as_str().to_owned(),
        human_id: ownership.human_id.as_str().to_owned(),
        status: ownership.status.as_str().to_owned(),
        created_at: ownership.created_at.to_rfc3339(),
        revoked_at: ownership.revoked_at.map(|t| t.to_rfc3339()),
    }
}

fn owned_view(owned: &crate::domain::agent::OwnedAgent) -> OwnedAgentView {
    OwnedAgentView {
        agent: agent_view(&owned.agent),
        ownership: ownership_view(&owned.ownership),
    }
}

/// 严格读取 Idempotency-Key：
/// - 缺失 header → `Missing`（400 idempotency_key_required）；
/// - 多个 header 值或值内逗号（HTTP 列表合并痕迹）→ `Invalid`；
/// - 其余格式校验交给领域层（空值、超长、字符集等统一 400
///   invalid_idempotency_key）。
pub(crate) enum KeyError {
    Missing,
    Invalid,
}

pub(crate) fn idempotency_key_from_headers(headers: &HeaderMap) -> Result<String, KeyError> {
    let mut values = headers.get_all(IDEMPOTENCY_KEY_HEADER).iter();
    let Some(first) = values.next() else {
        return Err(KeyError::Missing);
    };
    if values.next().is_some() {
        return Err(KeyError::Invalid);
    }
    let Ok(value) = first.to_str() else {
        return Err(KeyError::Invalid);
    };
    if value.split(',').count() > 1 {
        return Err(KeyError::Invalid);
    }
    Ok(value.to_owned())
}
