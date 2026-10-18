//! Call HTTP handler（契约 CORE_V1_CALL.md）。
//!
//! Human Session 侧：
//! - `POST /v1/calls`：Human 发起点到点通话（`{room_id, callee_id, media?}`；
//!   201 新建 / 200 复用同 (room, caller, callee) 既有 active Call）；
//! - `GET /v1/calls/{call_id}`：读取 Call（参与者才可，非参与者 404）；
//! - `POST /v1/calls/{call_id}/accept|decline|hangup`：状态机动作；
//! - `POST /v1/calls/{call_id}/signal`：中继信令（offer/answer/ice）；
//! - `GET /v1/call/stream`：Human Call-stream WS（首帧 authenticate + 只发
//!   与本人相关的投影：call.ring / call.signal / call.state）。
//!
//! Backend Credential 侧（Agent 执行端，代表被叫或被叫 Agent）：
//! - `POST /v1/backend/calls`：Agent 作为发起方；
//! - `POST /v1/backend/calls/{call_id}/accept|decline|hangup|signal`；
//! - `GET /v1/backend/call/stream`：Backend Call-stream WS。
//!
//! 发起方与应答方都既可以是 Human 也可以是 Agent：任意两个 active
//! Participant 都可互相呼叫（H↔H / H↔A / A↔A）。
//!
//! 媒体（音频/视频）由两端客户端 WebRTC P2P 直连，Core 只协调状态并中继
//! 纯信令控制消息；一切临时投影绝不进入 `/v1/human/events`、Human Event
//! 表或 `events.available` 语义，绝不写 SQLite。handler 只调用
//! `application::call` 用例与窄端口。

use std::sync::Arc;
use std::time::Duration;

use axum::extract::rejection::JsonRejection;
use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::adapters::call::CALL_STREAM_CAPACITY;
use crate::adapters::http::AppState;
use crate::adapters::http::auth::CurrentAuth;
use crate::adapters::http::backend_auth::BackendAuth;
use crate::adapters::http::human_event_routes::auth_fail_and_close;
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::routes::{error_response, store_error_response};
use crate::application::identity;
use crate::domain::call::CallId;
use crate::domain::participant::ParticipantId;
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::protocol::call::{
    CallActionResponse, CallResponse, CallView, InitiateRequest, InitiateResponse, SignalRequest,
    build_signal_body, call_stream_frame, parse_signal_kind,
};
use crate::protocol::error::codes;
use crate::protocol::human_event::{WS_AUTHENTICATE, WS_AUTHENTICATED, WsAuthFrame};

/// WS 首帧认证超时（复用既有 5 秒语义）。
const WS_AUTH_TIMEOUT: Duration = Duration::from_secs(5);

/// WS 认证帧体上限（与既有一致）。
const WS_MAX_FRAME_BYTES: usize = 4096;

/// Call 切片路由：Human 发起面 + Backend（被叫/发起 Agent）面。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/calls", post(initiate_route))
        .route("/v1/calls/{call_id}", axum::routing::get(get_route))
        .route("/v1/calls/{call_id}/accept", post(accept_route))
        .route("/v1/calls/{call_id}/decline", post(decline_route))
        .route("/v1/calls/{call_id}/hangup", post(hangup_route))
        .route("/v1/calls/{call_id}/signal", post(signal_route))
        .route("/v1/call/stream", get(human_stream_route))
        .route("/v1/backend/calls", post(backend_initiate_route))
        .route(
            "/v1/backend/calls/{call_id}/accept",
            post(backend_accept_route),
        )
        .route(
            "/v1/backend/calls/{call_id}/decline",
            post(backend_decline_route),
        )
        .route(
            "/v1/backend/calls/{call_id}/hangup",
            post(backend_hangup_route),
        )
        .route(
            "/v1/backend/calls/{call_id}/signal",
            post(backend_signal_route),
        )
        .route("/v1/backend/call/stream", get(backend_stream_route))
}

fn parse_call_id(raw: &str) -> Option<CallId> {
    CallId::parse_canonical(raw)
}

/// 发起公共前置：解析 InitiateRequest 的 room/callee/media，构造 400 或
/// 调用 application::initiate（caller 为认证方 participant id）。
async fn initiate_common(
    state: &AppState,
    caller_id: &ParticipantId,
    request_id: String,
    body: Result<Json<InitiateRequest>, JsonRejection>,
) -> Response {
    let Json(payload) = match body {
        Ok(body) => body,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "invalid JSON request body",
                &request_id,
            );
        }
    };
    let Some(room_id) = RoomId::parse_canonical(&payload.room_id) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "room_id must be a canonical UUID v4",
            &request_id,
        );
    };
    let Some(callee_id) = ParticipantId::parse_canonical(&payload.callee_id) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "callee_id must be a canonical UUID v4",
            &request_id,
        );
    };
    let Some(media) = crate::protocol::call::resolve_media(payload.media.as_deref()) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "media must be one of: audio, video",
            &request_id,
        );
    };

    match crate::application::call::initiate(
        state.call.as_ref(),
        state.room.as_ref(),
        caller_id,
        &room_id,
        &callee_id,
        media,
        UtcMillis::now(),
    )
    .await
    {
        Ok(result) => {
            let status = if result.created {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (
                status,
                Json(InitiateResponse {
                    call: CallView::from(&result.call),
                    created: result.created,
                    request_id,
                }),
            )
                .into_response()
        }
        Err(e) => call_error_response(e, "call.initiate", &request_id),
    }
}

/// POST /v1/calls：Human 发起。callee 可为 Human 或 Agent（同一共享
/// Room 内）。201 新建 / 200 复用。
async fn initiate_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
    body: Result<Json<InitiateRequest>, JsonRejection>,
) -> Response {
    initiate_common(&state, &auth.participant.id, request_id.0, body).await
}

/// GET /v1/calls/{call_id}：参与者才可读取；非参与者/未知统一 404。
async fn get_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(call_id): Path<String>,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    let Some(call_id) = parse_call_id(&call_id) else {
        return error_response(
            StatusCode::NOT_FOUND,
            codes::CALL_NOT_FOUND,
            "call not found",
            &request_id,
        );
    };
    let Some(call) = crate::application::call::get(state.call.as_ref(), &call_id) else {
        return error_response(
            StatusCode::NOT_FOUND,
            codes::CALL_NOT_FOUND,
            "call not found",
            &request_id,
        );
    };
    if call.caller_id != auth.participant.id && call.callee_id != auth.participant.id {
        return error_response(
            StatusCode::NOT_FOUND,
            codes::CALL_NOT_FOUND,
            "call not found",
            &request_id,
        );
    }
    Json(CallResponse {
        call: CallView::from(&call),
        request_id,
    })
    .into_response()
}

/// POST /v1/calls/{call_id}/accept：仅 callee。
async fn accept_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(call_id): Path<String>,
    request_id: RequestId,
) -> Response {
    action_route(
        &state,
        &call_id,
        &auth.participant.id,
        Action::Accept,
        request_id.0,
    )
    .await
}

/// POST /v1/calls/{call_id}/decline：仅 callee。
async fn decline_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(call_id): Path<String>,
    request_id: RequestId,
) -> Response {
    action_route(
        &state,
        &call_id,
        &auth.participant.id,
        Action::Decline,
        request_id.0,
    )
    .await
}

/// POST /v1/calls/{call_id}/hangup：caller 或 callee。
async fn hangup_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(call_id): Path<String>,
    request_id: RequestId,
) -> Response {
    action_route(
        &state,
        &call_id,
        &auth.participant.id,
        Action::Hangup,
        request_id.0,
    )
    .await
}

#[derive(Clone, Copy)]
enum Action {
    Accept,
    Decline,
    Hangup,
}

async fn action_route(
    state: &AppState,
    raw_call_id: &str,
    actor_id: &ParticipantId,
    action: Action,
    request_id: String,
) -> Response {
    let Some(call_id) = parse_call_id(raw_call_id) else {
        return error_response(
            StatusCode::NOT_FOUND,
            codes::CALL_NOT_FOUND,
            "call not found",
            &request_id,
        );
    };
    let now = UtcMillis::now();
    let result = match action {
        Action::Accept => {
            crate::application::call::accept(state.call.as_ref(), &call_id, actor_id, now)
        }
        Action::Decline => {
            crate::application::call::decline(state.call.as_ref(), &call_id, actor_id, now)
        }
        Action::Hangup => {
            crate::application::call::hangup(state.call.as_ref(), &call_id, actor_id, now)
        }
    };
    match result {
        Ok(call) => Json(CallActionResponse {
            call: CallView::from(&call),
            request_id,
        })
        .into_response(),
        Err(e) => call_error_response(e, "call.action", &request_id),
    }
}

/// POST /v1/calls/{call_id}/signal：中继信令至对方。
async fn signal_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(call_id): Path<String>,
    request_id: RequestId,
    body: Result<Json<SignalRequest>, JsonRejection>,
) -> Response {
    signal_common(
        &state,
        &call_id,
        &auth.participant.id,
        request_id.0,
        body,
        "call.signal",
    )
    .await
}

/// Backend 侧动作（Agent 执行端；只接受代表该 Call 中 Agent 方的
/// Credential）。
async fn backend_accept_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(call_id): Path<String>,
    request_id: RequestId,
) -> Response {
    backend_action_route(
        &state,
        &call_id,
        &auth.view.agent.id,
        Action::Accept,
        request_id.0,
    )
    .await
}

/// POST /v1/backend/calls：Agent（经 Backend Credential）作为发起方在共享
/// Room 内呼叫另一 Participant（Human 或 Agent）。Callee 可为 Human 或
/// Agent。201 新建 / 200 复用。
async fn backend_initiate_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    request_id: RequestId,
    body: Result<Json<InitiateRequest>, JsonRejection>,
) -> Response {
    initiate_common(&state, &auth.view.agent.id, request_id.0, body).await
}

async fn backend_decline_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(call_id): Path<String>,
    request_id: RequestId,
) -> Response {
    backend_action_route(
        &state,
        &call_id,
        &auth.view.agent.id,
        Action::Decline,
        request_id.0,
    )
    .await
}

async fn backend_hangup_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(call_id): Path<String>,
    request_id: RequestId,
) -> Response {
    backend_action_route(
        &state,
        &call_id,
        &auth.view.agent.id,
        Action::Hangup,
        request_id.0,
    )
    .await
}

async fn backend_action_route(
    state: &AppState,
    raw_call_id: &str,
    actor_id: &ParticipantId,
    action: Action,
    request_id: String,
) -> Response {
    action_route(state, raw_call_id, actor_id, action, request_id).await
}

async fn backend_signal_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(call_id): Path<String>,
    request_id: RequestId,
    body: Result<Json<SignalRequest>, JsonRejection>,
) -> Response {
    signal_common(
        &state,
        &call_id,
        &auth.view.agent.id,
        request_id.0,
        body,
        "call.backend.signal",
    )
    .await
}

async fn signal_common(
    state: &AppState,
    raw_call_id: &str,
    from_id: &ParticipantId,
    request_id: String,
    body: Result<Json<SignalRequest>, JsonRejection>,
    operation: &'static str,
) -> Response {
    let Json(payload) = match body {
        Ok(body) => body,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "invalid JSON request body",
                &request_id,
            );
        }
    };
    let Some(call_id) = parse_call_id(raw_call_id) else {
        return error_response(
            StatusCode::NOT_FOUND,
            codes::CALL_NOT_FOUND,
            "call not found",
            &request_id,
        );
    };
    let Some(kind) = parse_signal_kind(&payload.kind) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            codes::CALL_PAYLOAD_INVALID,
            "signal kind must be one of: offer, answer, ice",
            &request_id,
        );
    };
    let Some(body) = build_signal_body(kind, &payload) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            codes::CALL_PAYLOAD_INVALID,
            "invalid signaling payload (shape/bytes/NUL/quantity)",
            &request_id,
        );
    };
    match crate::application::call::signal(
        state.call.as_ref(),
        &call_id,
        from_id,
        &body,
        UtcMillis::now(),
    ) {
        Ok(_delivered) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => call_error_response(e, operation, &request_id),
    }
}

/// Human Call-stream WS：首帧 authenticate → 订阅本人送达通道 → 只发与
/// 本人相关的 call.ring/call.signal/call.state。
async fn human_stream_route(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
    request_id: RequestId,
) -> Response {
    ws.max_message_size(WS_MAX_FRAME_BYTES)
        .on_upgrade(move |socket| call_stream_ws(socket, state, request_id.0, StreamRole::Human))
}

/// Backend Call-stream WS：首帧以 Backend Credential authenticate → 订阅
/// 代表 Agent 的送达通道。
async fn backend_stream_route(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
    request_id: RequestId,
) -> Response {
    ws.max_message_size(WS_MAX_FRAME_BYTES)
        .on_upgrade(move |socket| call_stream_ws(socket, state, request_id.0, StreamRole::Backend))
}

#[derive(Clone, Copy)]
enum StreamRole {
    Human,
    Backend,
}

/// 连接任务：等待首帧认证（5 秒）→ 订阅 Call-stream 送达通道 → ack → 唤醒
/// 循环（只发与本人相关的临时投影）。
async fn call_stream_ws(
    mut socket: WebSocket,
    state: AppState,
    request_id: String,
    role: StreamRole,
) {
    let frame = match timeout(WS_AUTH_TIMEOUT, socket.recv()).await {
        Ok(Some(Ok(Message::Text(text)))) => text,
        _ => {
            auth_fail_and_close(&mut socket, &request_id).await;
            return;
        }
    };
    let token = match serde_json::from_str::<WsAuthFrame>(frame.as_str()) {
        Ok(frame) if frame.kind == WS_AUTHENTICATE && !frame.token.is_empty() => frame.token,
        _ => {
            auth_fail_and_close(&mut socket, &request_id).await;
            return;
        }
    };

    // 首帧认证：Human Session 或 Backend Credential。
    let participant_id = match role {
        StreamRole::Human => match identity::authenticate(state.identity.as_ref(), &token).await {
            Ok(context) => context.participant.id,
            Err(identity::AuthenticateError::Store(_)) => {
                tracing::error!(
                    operation = "call.ws.authenticate",
                    error_kind = "storage_unavailable",
                    request_id = %request_id,
                    "identity store failure during call-stream human session authentication"
                );
                auth_fail_and_close(&mut socket, &request_id).await;
                return;
            }
            Err(_) => {
                auth_fail_and_close(&mut socket, &request_id).await;
                return;
            }
        },
        StreamRole::Backend => {
            match crate::application::backend::authenticate_backend(state.backend.as_ref(), &token)
                .await
            {
                Ok(view) => view.agent.id,
                Err(_) => {
                    auth_fail_and_close(&mut socket, &request_id).await;
                    return;
                }
            }
        }
    };

    // 先建立订阅再回 authenticated ack：订阅前发生的变化由 REST 重同步
    // 覆盖。每连接送达通道有界容量（复用 adapter 常量）。
    let (tx, mut rx) = mpsc::channel::<crate::ports::call::CallStreamEvent>(CALL_STREAM_CAPACITY);
    let sender = Arc::new(tx);
    state.call.subscribe(&participant_id, sender.clone());
    let ack = format!("{{\"type\":\"{WS_AUTHENTICATED}\"}}");
    if socket
        .send(Message::Text(Utf8Bytes::from(ack)))
        .await
        .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                // 只发与本人相关、仍在 TTL 内的投影。
                let frame_text = call_stream_frame(&event);
                if socket
                    .send(Message::Text(Utf8Bytes::from(frame_text)))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            frame = socket.recv() => match frame {
                None | Some(Err(_)) => return,
                Some(Ok(Message::Close(_))) => return,
                // 认证后的其他帧无契约语义，忽略（不泄露任何事实）。
                Some(Ok(_)) => {}
            },
        }
    }
}

/// CallError → HTTP 映射：400 invalid_request / call_payload_invalid /
/// 404 room_not_found / call_not_found / 409 call_state_conflict /
/// call_party_busy / 503 call_state_limit_exceeded / 存储错误沿用既有
/// 500/503 分类。
fn call_error_response(
    e: crate::application::call::CallError,
    operation: &'static str,
    request_id: &str,
) -> Response {
    match e {
        crate::application::call::CallError::InvalidRequest(message) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            message,
            request_id,
        ),
        crate::application::call::CallError::RoomNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            request_id,
        ),
        crate::application::call::CallError::CallNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::CALL_NOT_FOUND,
            "call not found",
            request_id,
        ),
        crate::application::call::CallError::StateConflict => error_response(
            StatusCode::CONFLICT,
            codes::CALL_STATE_CONFLICT,
            "call state conflict",
            request_id,
        ),
        crate::application::call::CallError::PartyBusy => error_response(
            StatusCode::CONFLICT,
            codes::CALL_PARTY_BUSY,
            "call party is already in an active call",
            request_id,
        ),
        crate::application::call::CallError::LimitReached => error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            codes::CALL_STATE_LIMIT_EXCEEDED,
            "call state store at capacity; retry after the active call ends",
            request_id,
        ),
        crate::application::call::CallError::Store(storage) => {
            store_error_response(operation, &storage, request_id)
        }
    }
}
