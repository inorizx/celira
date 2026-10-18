//! Agent 读游标 HTTP handler（契约 `CORE_V1_AGENT_READ_CURSOR.md`，窄解冻
//! 切片 M1「Agent 读游标」）。
//!
//! `PUT /v1/backend/rooms/{room_id}/read-cursor`：Agent 显式说"我读到这条
//! 消息了"。语义**逐条同 Human 侧** `PUT /v1/rooms/{room_id}/read-cursor`：
//!
//! - 认证 BackendAuth（401 语义与既有 Backend 端点完全一致）；
//! - requester 必须是该 Room 的 active 成员（未知 / bad-id / 非成员统一
//!   404 `room_not_found`，不泄露房间是否存在）；
//! - 目标必须是同 Room、且对该 Agent 的历史 Membership 窗口可见的真实
//!   Message（未知 / 他 Room / 不可见统一 400 `invalid_read_cursor`）；
//! - 目标不高于当前位置：成功 no-op（200 `advanced=false`），不产生任何事实；
//! - 目标更高：插入不可变 advancement 事实，其触发器链在同一事务内单调
//!   推进 cursor（无第二条写路径）；
//! - 无 Idempotency-Key：单调推进天然幂等。
//!
//! **与 Human 侧唯一的差别在事件面**：Agent 的推进**不产生任何事件**
//! （`human_event_facts_read_cursor_advanced` 只对 Human 的 advancement 造
//! fact，见 migration `202609270001`）。理由：`read.cursor.advanced` 按契约
//! 只投递**推进者本人**，而 Agent 只有一个执行端、位置由读面（通讯录的
//! `read_state`）直接读得到——事件对它没有收件人。因此本 handler 不做
//! post-commit 唤醒（没有 fact 可反查），也不产生"对方知道我读了"的语义。
//!
//! handler 只调用 `application::room::advance_read_cursor`（与 Human 同一
//! 用例）与端口，不直接操作 SQLite；所有非 2xx 走统一 JSON 错误信封，
//! body 的 request_id 与 x-request-id 一致。

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::put;
use axum::{Json, Router};

use crate::adapters::http::AppState;
use crate::adapters::http::backend_auth::BackendAuth;
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::room_routes::{MAX_READ_CURSOR_BODY_BYTES, read_state_view};
use crate::adapters::http::routes::{error_response, store_error_response};
use crate::application::room::{AdvanceReadCursorError, advance_read_cursor};
use crate::protocol::error::codes;
use crate::protocol::room::{PutReadCursorRequest, PutReadCursorResponse};

/// Agent 读游标路由：一条写端点（BackendAuth）。单独成子 router 只为把
/// route-local 请求体上限（与 Human 侧同一常量 1 KiB）限定在本端点上。
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/backend/rooms/{room_id}/read-cursor",
            put(put_read_cursor_route),
        )
        .layer(axum::extract::DefaultBodyLimit::max(
            MAX_READ_CURSOR_BODY_BYTES,
        ))
}

/// `PUT /v1/backend/rooms/{room_id}/read-cursor`：请求体只接受
/// `message_id`（`deny_unknown_fields`）；body 上限与 Human 侧同一常量
/// （1 KiB）。成功 200 `{read_state, advanced, request_id}`。
async fn put_read_cursor_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
    body: Result<Json<PutReadCursorRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    let Json(payload) = match body {
        Ok(body) => body,
        Err(axum::extract::rejection::JsonRejection::BytesRejection(
            axum::extract::rejection::BytesRejection::FailedToBufferBody(
                axum::extract::rejection::FailedToBufferBody::LengthLimitError(_),
            ),
        )) => {
            return error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                codes::PAYLOAD_TOO_LARGE,
                format!("request body exceeds {MAX_READ_CURSOR_BODY_BYTES} bytes"),
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

    match advance_read_cursor(
        state.room.as_ref(),
        &auth.view.agent.id,
        &room_id,
        &payload.message_id,
    )
    .await
    {
        Ok(crate::application::room::AdvanceReadCursorOutcome::Advanced { read_state, .. }) => (
            StatusCode::OK,
            Json(PutReadCursorResponse {
                read_state: read_state_view(&read_state),
                advanced: true,
                request_id,
            }),
        )
            .into_response(),
        Ok(crate::application::room::AdvanceReadCursorOutcome::NoOp { read_state }) => (
            StatusCode::OK,
            Json(PutReadCursorResponse {
                read_state: read_state_view(&read_state),
                advanced: false,
                request_id,
            }),
        )
            .into_response(),
        Err(AdvanceReadCursorError::RoomNotFound) => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            &request_id,
        ),
        Err(AdvanceReadCursorError::InvalidMessageId) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "message_id must be a UUID v4",
            &request_id,
        ),
        Err(AdvanceReadCursorError::InvalidReadCursor) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_READ_CURSOR,
            "the message cannot be the read cursor target",
            &request_id,
        ),
        Err(AdvanceReadCursorError::Store(e)) => {
            store_error_response("backend.read_cursor", &e, &request_id)
        }
    }
}
