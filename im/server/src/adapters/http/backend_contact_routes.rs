//! Agent 侧的关系面（批次三·第二刀「关系对主体开放」，2026-09-27 主人批准）：
//! `GET /v1/backend/contacts`、`POST /v1/backend/contacts/requests`、
//! `POST /v1/backend/contacts/{participant_id}/accept|decline`、
//! `DELETE /v1/backend/contacts/{participant_id}`。
//!
//! **为什么五条要一起存在**：关系是两个主体之间的事实（第一刀把 owner 从 kind
//! 里解出来之后，这一刀把关系也解出来），但事实若够不着就等于没有——一支 Agent
//! 必须能**看见**谁在申请、能**自己**应答、能**主动**发起、能**解除**。这四条
//! 加上"看见"就是它的全部关系能力。
//!
//! **与 Human 侧逐条同形**：同一个 application 用例、同一段响应构造
//! （`contact_routes::*_response`）、同一套错误码与幂等语义、同一份投影；只换认证面
//! （`BackendAuth`）与 actor 来源（`backend_agent_actor`）。这是本仓库既有的分叉
//! 纪律：**领域层与应用层 kind 无关，只在适配器分叉**。
//!
//! **同意归被申请的那一方**（2026-09-27 主人裁决）：`accept` / `decline` 的判据本来
//! 就是"我是不是 addressee"，换成 Agent 认证面后自动成立——**不新增"谁有权代答"
//! 的规则**。Owner 不代答；它保留的是 revoke 生死权（撤销会按既有传播关掉该 Agent
//! 名下的成员周期）。
//!
//! **不做的**：Agent 侧拉黑（`participant_blocks` 仍两端限 Human，主人 2026-09-25
//! 明文裁决）；关系动作不产生任何事件（与 Human 侧一致，Agent 在清醒期拉列表看到
//! 申请）。
//!
//! **不新增任何持久事实**：五条端点读写的全部是既有表（`contact_periods`）。

use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{delete, get, post};

use crate::adapters::http::AppState;
use crate::adapters::http::backend_auth::{BackendAuth, backend_agent_actor};
use crate::adapters::http::contact_routes::{
    accept_contact_response, create_contact_response, decline_contact_response,
    end_contact_response, list_contacts_response, parse_contact_body_and_key,
};
use crate::adapters::http::middleware::RequestId;
use crate::protocol::contact::CreateContactRequest;

/// Agent 侧关系路由（五条，全部 BackendAuth）。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/backend/contacts", get(list_contacts_route))
        .route("/v1/backend/contacts/requests", post(create_contact_route))
        .route(
            "/v1/backend/contacts/{participant_id}/accept",
            post(accept_contact_route),
        )
        .route(
            "/v1/backend/contacts/{participant_id}/decline",
            post(decline_contact_route),
        )
        .route(
            "/v1/backend/contacts/{participant_id}",
            delete(end_contact_route),
        )
}

/// `GET /v1/backend/contacts`：我作为 requester 或 addressee 的全部关系
/// （pending 与 accepted 原样公开，peer 相对我，可以是 Human 或 Agent）。
async fn list_contacts_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    request_id: RequestId,
) -> Response {
    list_contacts_response(&state, &auth.view.agent.id, &request_id.0).await
}

/// `POST /v1/backend/contacts/requests`：我发起关系申请（必带
/// Idempotency-Key；目标可以是任意 active 主体）。
async fn create_contact_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<CreateContactRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    let (Json(payload), key) = match parse_contact_body_and_key(body, &headers, &request_id).await {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    let actor = backend_agent_actor(&auth.view.agent);
    create_contact_response(&state, &actor, &payload.participant_id, &key, &request_id).await
}

/// `POST /v1/backend/contacts/{participant_id}/accept`：**我**接受给我的申请
/// （非 addressee 一律 404 `contact_not_found`）。
async fn accept_contact_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(participant_id): Path<String>,
    request_id: RequestId,
) -> Response {
    accept_contact_response(&state, &auth.view.agent.id, &participant_id, &request_id.0).await
}

/// `POST /v1/backend/contacts/{participant_id}/decline`：**我**拒绝给我的申请
/// （非 addressee / 段非 pending 一律 404 `contact_not_found`）。
async fn decline_contact_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(participant_id): Path<String>,
    request_id: RequestId,
) -> Response {
    decline_contact_response(&state, &auth.view.agent.id, &participant_id, &request_id.0).await
}

/// `DELETE /v1/backend/contacts/{participant_id}`：我单方面解除（无 active 段
/// 是成功 no-op，幂等 204）。
async fn end_contact_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(participant_id): Path<String>,
    request_id: RequestId,
) -> Response {
    end_contact_response(&state, &auth.view.agent.id, &participant_id, &request_id.0).await
}
