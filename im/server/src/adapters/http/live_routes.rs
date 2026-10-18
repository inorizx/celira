//! 主体进行态 HTTP handler（契约 `CORE_V1_AGENT_PROTOCOL.md` §6）。
//!
//! 只有四条路由，人机同形：
//! - 读（同一投影，两个认证面）：
//!   - `GET /v1/rooms/{room_id}/live`（Human Session）
//!   - `GET /v1/backend/rooms/{room_id}/live`（BackendAuth；身份只来自认证）
//!
//!   返回该房**全部 active 成员**的 `{participant_id, display_name, kind,
//!   online, typing}`；未知 / bad-id / 非成员统一 `404 room_not_found`。
//! - 写（无参数，`204`）：
//!   - `POST /v1/rooms/{room_id}/typing`（Human Session）
//!   - `POST /v1/backend/rooms/{room_id}/typing`（BackendAuth）
//!
//!   只说「我正在这间房写」——没有正文、没有阶段、没有 stream/revision。
//!
//! 这里**没有**执行面：旧的心跳（`/v1/backend/presence`）、Activity 上报
//! （`/v1/backend/events/{event_id}/activity`）与独立 Live WS
//! （`/v1/human/live/stream`）已于 2026-09-28 整体删除——在线由**连接**
//! 给出（Human WS 订阅表 / Agent 门铃流 / 最近被看见），不需要任何一方
//! 上报身体态；进行态走**已有**的两条连接，不再造第三条线。
//!
//! 一切进行态绝不进入 `/v1/human/events`、Human Event 表或
//! `events.available` 语义。handler 只调用 `application::live` 用例与窄
//! 端口，不直接操作 SQLite。

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::adapters::http::AppState;
use crate::adapters::http::auth::CurrentAuth;
use crate::adapters::http::backend_auth::BackendAuth;
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::routes::error_response;
use crate::application::live::{LiveTypingError, mark_typing, room_live_snapshot};
use crate::application::room::RoomQueryError;
use crate::domain::time::UtcMillis;
use crate::protocol::error::codes;
use crate::protocol::live::{MemberLiveView, RoomLiveSnapshotResponse};

/// 进行态切片路由。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/rooms/{room_id}/live", get(room_live_route))
        // 读取面对齐（V9，批次四；2026-09-28 重做为主体形状）：Agent 读
        // **同一间房**的进行态快照——同一个 application 用例、同一段投影，
        // 只在适配器换认证面。
        .route(
            "/v1/backend/rooms/{room_id}/live",
            get(backend_room_live_route),
        )
        .route("/v1/rooms/{room_id}/typing", post(typing_route))
        .route(
            "/v1/backend/rooms/{room_id}/typing",
            post(backend_typing_route),
        )
}

/// GET /v1/rooms/{room_id}/live：Human Session 读该房进行态快照。
async fn room_live_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    match room_live_snapshot(
        state.room.as_ref(),
        state.live.as_ref(),
        state.human_connections.as_ref(),
        state.backend_connections.as_ref(),
        &auth.participant.id,
        &room_id,
        UtcMillis::now(),
    )
    .await
    {
        Ok(snapshot) => snapshot_response(snapshot, request_id),
        Err(e) => query_error_response(e, &request_id),
    }
}

/// GET /v1/backend/rooms/{room_id}/live：BackendAuth 读**同一段投影**。
async fn backend_room_live_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    match room_live_snapshot(
        state.room.as_ref(),
        state.live.as_ref(),
        state.human_connections.as_ref(),
        state.backend_connections.as_ref(),
        &auth.view.agent.id,
        &room_id,
        UtcMillis::now(),
    )
    .await
    {
        Ok(snapshot) => snapshot_response(snapshot, request_id),
        Err(e) => query_error_response(e, &request_id),
    }
}

/// POST /v1/rooms/{room_id}/typing：Human 声明「我正在这间房写」。
async fn typing_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let request_id = request_id.0;
    if let Err(response) = require_empty_body(body, &request_id) {
        return *response;
    }
    finish_typing(
        mark_typing(
            state.room.as_ref(),
            state.live.as_ref(),
            &auth.participant.id,
            &room_id,
            UtcMillis::now(),
        )
        .await,
        request_id,
    )
}

/// POST /v1/backend/rooms/{room_id}/typing：Agent 声明「我正在这间房写」
/// （与 Human 侧同一条用例、同一判据）。
async fn backend_typing_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let request_id = request_id.0;
    if let Err(response) = require_empty_body(body, &request_id) {
        return *response;
    }
    finish_typing(
        mark_typing(
            state.room.as_ref(),
            state.live.as_ref(),
            &auth.view.agent.id,
            &room_id,
            UtcMillis::now(),
        )
        .await,
        request_id,
    )
}

/// typing 没有参数：带 body 一律 `400 invalid_request`（不静默忽略——
/// 静默忽略会让"我传的字段生效了吗"变成只能靠试）。
///
/// 失败走 `Box<Response>`：`Response` 是个大结构（clippy `result_large_err`），
/// 而这条函数几乎总是返回 `Ok(())` —— 不该让每次成功调用都压一个大结构在栈上。
fn require_empty_body(
    body: Result<Bytes, BytesRejection>,
    request_id: &str,
) -> Result<(), Box<Response>> {
    let empty = match body {
        Ok(bytes) => bytes.is_empty(),
        Err(_) => false,
    };
    if empty {
        return Ok(());
    }
    Err(Box::new(error_response(
        StatusCode::BAD_REQUEST,
        codes::INVALID_REQUEST,
        "typing request must not carry a body",
        request_id,
    )))
}

/// typing 成功即 `204`（无响应体：声明本身不是事实，没有可回的东西）。
fn finish_typing(result: Result<(), LiveTypingError>, request_id: String) -> Response {
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(LiveTypingError::NotFound) => not_found_response(&request_id),
        Err(LiveTypingError::Store(e)) => query_error_response(e, &request_id),
    }
}

/// 进行态快照 → 线格式（人机同一段构造，避免两处漂移）。
fn snapshot_response(
    snapshot: crate::application::live::RoomLiveSnapshot,
    request_id: String,
) -> Response {
    Json(RoomLiveSnapshotResponse {
        room_id: snapshot.room_id.as_str().to_owned(),
        members: snapshot
            .members
            .into_iter()
            .map(|member| MemberLiveView {
                participant_id: member.participant_id.as_str().to_owned(),
                display_name: member.display_name.as_str().to_owned(),
                kind: member.kind.as_str().to_owned(),
                online: member.online,
                typing: member.typing,
            })
            .collect(),
        request_id,
    })
    .into_response()
}

/// 统一 404（不泄露房间/他人是否存在）。
fn not_found_response(request_id: &str) -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        codes::ROOM_NOT_FOUND,
        "room not found",
        request_id,
    )
}

/// 成员读取用例错误 → 统一 404 / 脱敏 500。
fn query_error_response(error: RoomQueryError, request_id: &str) -> Response {
    match error {
        RoomQueryError::NotFound => not_found_response(request_id),
        RoomQueryError::Store(e) => {
            tracing::error!(
                operation = "live.snapshot",
                request_id = %request_id,
                error_kind = ?e.kind(),
                "room store failure during live snapshot"
            );
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                codes::INTERNAL_ERROR,
                "internal error",
                request_id,
            )
        }
    }
}
