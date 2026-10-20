//! 提醒档（attention）HTTP handler（契约 `CORE_V1_IM_CONTRACT.md` §9.1，
//! 2026-10-19 窄解冻切片 1a）。
//!
//! **两条路由、逐条同形**：
//! - `PUT /v1/rooms/{room_id}/attention`（Human，`CurrentAuth`）；
//! - `PUT /v1/backend/rooms/{room_id}/attention`（BackendAuth）。
//!
//! 两者**只在认证面分叉**（actor 的来源与 401 语义），语义/参数/错误码/响应
//! 形状共用同一段响应构造（`put_attention_response`）——本仓库既有的分叉
//! 纪律：领域层与应用层 kind 无关，只在适配器分叉。
//!
//! 语义一句话：**这间房的动静要不要提醒我**是个人事实，只有本人能写、
//! 只有本人看得见（与 §9 的读位置同一档）：
//!
//! - 请求体**只有 `{level}`**（`deny_unknown_fields`）；`level ∈ {all, mentions}`，
//!   认不出的词是 400 `invalid_request`；
//! - **不要求 Idempotency-Key**：状态写，同值重写天然幂等（照 §9 读位置的先例）；
//! - 本人不是该房 active 成员、或该房不是 `group`（`direct` / `human_direct`
//!   两人房里没有「群消息」这一档）、或房不存在 / 坏 id → 统一 404
//!   `room_not_found`（与非成员**同形**，不泄露房间是否存在）；
//! - 成功 200 `{"attention": "...", "request_id": "..."}`——只回写后的权威取值，
//!   没有 `idempotency_replayed`（没有幂等键，形状不因"是不是重写"而变）。
//!
//! ⚠ **它不改消息主链**：消息照常提交、未读照常计算、历史照常可读；这一档
//! 只决定那条**易失**通知发不发 ⇒ 零事件、零唤醒、零持久事实之外的东西。
//! 「按档决定发不发那条通知」是另一刀（1b），不在本文件。
//!
//! 请求体上限 route-local 1 KiB（与读游标同一常量级），不放宽其他路由既有上限。

use axum::extract::rejection::{BytesRejection, FailedToBufferBody, JsonRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::put;
use axum::{Json, Router};

use crate::adapters::http::AppState;
use crate::adapters::http::auth::CurrentAuth;
use crate::adapters::http::backend_auth::{BackendAuth, backend_agent_actor};
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::routes::{error_response, store_error_response};
use crate::application::room::{SetAttentionError, set_room_attention};
use crate::domain::participant::ParticipantId;
use crate::protocol::error::codes;
use crate::protocol::room::{PutAttentionRequest, PutAttentionResponse};

/// 提醒档 PUT 的 route-local 请求体上限：小而固定（请求体只有 `level`）。
pub const MAX_ATTENTION_BODY_BYTES: usize = 1024;

/// 提醒档路由：Human 与 Backend 两条同形写端点，共用本层请求体上限。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/rooms/{room_id}/attention", put(put_attention_route))
        .route(
            "/v1/backend/rooms/{room_id}/attention",
            put(put_backend_attention_route),
        )
        .layer(axum::extract::DefaultBodyLimit::max(
            MAX_ATTENTION_BODY_BYTES,
        ))
}

/// `PUT /v1/rooms/{room_id}/attention`：当前 Human 写**自己**在这间房的提醒档。
async fn put_attention_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
    body: Result<Json<PutAttentionRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    put_attention_response(&state, &auth.participant.id, &room_id, body, &request_id).await
}

/// `PUT /v1/backend/rooms/{room_id}/attention`：该 Agent 写**自己**在这间房的
/// 提醒档。身份只来自 BackendAuth——请求体里没有主语。
async fn put_backend_attention_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
    body: Result<Json<PutAttentionRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    // actor 的物化点只有一处（`backend_agent_actor`），这里取它的 id 就够：
    // 提醒档没有"署名"，只有"是谁的那一行"。
    let actor = backend_agent_actor(&auth.view.agent);
    put_attention_response(&state, &actor.id, &room_id, body, &request_id).await
}

/// 两条认证面**共用**的响应构造（人机同形：同一用例、同一错误映射、同一形状）。
async fn put_attention_response(
    state: &AppState,
    participant_id: &ParticipantId,
    room_id: &str,
    body: Result<Json<PutAttentionRequest>, JsonRejection>,
    request_id: &str,
) -> Response {
    let Json(payload) = match body {
        Ok(body) => body,
        Err(JsonRejection::BytesRejection(BytesRejection::FailedToBufferBody(
            FailedToBufferBody::LengthLimitError(_),
        ))) => {
            return error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                codes::PAYLOAD_TOO_LARGE,
                format!("request body exceeds {MAX_ATTENTION_BODY_BYTES} bytes"),
                request_id,
            );
        }
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "invalid JSON request body",
                request_id,
            );
        }
    };

    match set_room_attention(state.room.as_ref(), participant_id, room_id, &payload.level).await {
        Ok(level) => (
            StatusCode::OK,
            Json(PutAttentionResponse {
                attention: level.as_str().to_owned(),
                request_id: request_id.to_owned(),
            }),
        )
            .into_response(),
        // 坏 level 与坏 body 同一出口（400 invalid_request）：这一档只认两个词。
        Err(SetAttentionError::InvalidLevel) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "level must be all or mentions",
            request_id,
        ),
        // 未知 / 坏 id / 不是群 / 非成员 同一个 404（不泄露房间是否存在）。
        Err(SetAttentionError::RoomNotFound) => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            request_id,
        ),
        Err(SetAttentionError::Store(e)) => store_error_response("room.attention", &e, request_id),
    }
}
