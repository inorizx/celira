//! Group Room Profile（群简介）HTTP handlers
//! （CORE_V1_ROOM_PROFILE.md §3–§5）。
//!
//! 两条写端点，**同一条业务路径**：
//! - `PUT /v1/rooms/{room_id}/profile`（Human Session，`CurrentAuth`）——逐字节
//!   沿用 v1.1 切片的行为；
//! - `PUT /v1/backend/rooms/{room_id}/profile`（Agent BackendAuth，2026-10-26）
//!   ——owner 是房内的治理角色而非参与者 kind：Agent 建了自己的群，就必须能
//!   写自己群的简介。
//!
//! 两个 adapter 只换认证面与 actor 的来源（`backend_agent_actor`），请求体 /
//! 幂等键解析、`application::room_profile::put_room_profile` 调用与响应映射
//! 全部走 [`put_room_profile_response`] 这一处，不复制第二套业务逻辑。
//!
//! handler 只调用用例与窄端口，不直接操作 SQLite；所有非 2xx 走统一 JSON
//! 错误信封。授权（该房当前 active owner，**不限参与者 kind**）由存储端口在
//! `BEGIN IMMEDIATE` 事务内先于重放裁决——路由层绝不重复弱化检查、不引入
//! 第二个写者。
//!
//! 请求体上限 16 KiB：简介正文上限 2000 字节，JSON 转义（换行 / Emoji 等）后
//! 仍有余量；超出返回 413 payload_too_large。正文字段级边界在域层裁决
//! （400 invalid_profile_description），不属于路由上限。

use axum::extract::rejection::{BytesRejection, FailedToBufferBody, JsonRejection};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::put;
use axum::{Json, Router};

use crate::adapters::http::AppState;
use crate::adapters::http::agent_routes::{KeyError, idempotency_key_from_headers};
use crate::adapters::http::auth::CurrentAuth;
use crate::adapters::http::backend_auth::{BackendAuth, backend_agent_actor};
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::routes::{error_response, store_error_response};
use crate::application::room_profile::{RoomProfilePutError, put_room_profile};
use crate::domain::participant::Participant;
use crate::ports::participant_presentation::{RoomProfileHeader, RoomProfileRead};
use crate::protocol::error::codes;
use crate::protocol::room_profile::{
    PutRoomProfileRequest, PutRoomProfileResponse, RoomHeaderView, RoomProfileHeadView,
    RoomProfileView,
};

/// 群简介 PUT 请求体上限：16 KiB，容纳合法 description（<= 2000 字节）的
/// JSON 转义表达。GET 无 body，不受影响。
pub const MAX_ROOM_PROFILE_BODY_BYTES: usize = 16 * 1024;

/// 群简介切片路由（在 4 KiB 默认上限之后注册，自带独立上限）。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/rooms/{room_id}/profile", put(put_room_profile_route))
        .route(
            "/v1/backend/rooms/{room_id}/profile",
            put(put_backend_room_profile_route),
        )
        .layer(axum::extract::DefaultBodyLimit::max(
            MAX_ROOM_PROFILE_BODY_BYTES,
        ))
}

/// 请求体解析：超过路由上限 → 413 payload_too_large；其余形状/语法错误 →
/// 400 invalid_request（统一信封）。
#[allow(clippy::result_large_err)]
fn parse_put_body<B>(
    body: Result<Json<B>, JsonRejection>,
    request_id: &str,
) -> Result<Json<B>, Response>
where
    B: serde::de::DeserializeOwned,
{
    let Json(payload) = match body {
        Ok(body) => body,
        Err(JsonRejection::BytesRejection(BytesRejection::FailedToBufferBody(
            FailedToBufferBody::LengthLimitError(_),
        ))) => {
            return Err(error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                codes::PAYLOAD_TOO_LARGE,
                format!("request body exceeds {MAX_ROOM_PROFILE_BODY_BYTES} bytes"),
                request_id,
            ));
        }
        Err(_) => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "invalid JSON request body",
                request_id,
            ));
        }
    };
    Ok(Json(payload))
}

fn require_key(headers: &HeaderMap, request_id: &str) -> Result<String, Box<Response>> {
    match idempotency_key_from_headers(headers) {
        Ok(key) => Ok(key),
        Err(KeyError::Missing) => Err(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            codes::IDEMPOTENCY_KEY_REQUIRED,
            "missing Idempotency-Key header",
            request_id,
        ))),
        Err(KeyError::Invalid) => Err(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_IDEMPOTENCY_KEY,
            "invalid Idempotency-Key header",
            request_id,
        ))),
    }
}

/// 群简介写的**唯一** HTTP 执行路径：Human `CurrentAuth` 与 BackendAuth 两个
/// 适配器都调用本函数——解析顺序（body → Idempotency-Key → 用例）、错误映射
/// 与成功响应逐字节相同，只在调用方换 actor 的来源。
///
/// **统一 200**：首次写入产生版本 N，同 key 同摘要重放返回首次结果
/// （`idempotency_replayed=true`）且不追加版本；同 key 不同摘要
/// 409 `idempotency_conflict`；新 key 但 `base_version` 过期 409
/// `profile_version_conflict`。非成员 / 未知房 404 `room_not_found`；
/// active 成员但非 owner 403 `not_room_owner`；非 group 400 `invalid_room_kind`。
pub(crate) async fn put_room_profile_response(
    state: &AppState,
    actor: &Participant,
    room_id: &str,
    request_id: &str,
    headers: &HeaderMap,
    body: Result<Json<PutRoomProfileRequest>, JsonRejection>,
) -> Response {
    let Json(payload) = match parse_put_body(body, request_id) {
        Ok(p) => p,
        Err(response) => return response,
    };
    let key = match require_key(headers, request_id) {
        Ok(key) => key,
        Err(response) => return *response,
    };
    match put_room_profile(
        state.presentation.as_ref(),
        actor,
        room_id,
        &key,
        payload.base_version,
        payload.description.as_deref(),
    )
    .await
    {
        Ok(outcome) => Json(PutRoomProfileResponse {
            profile: room_profile_view(&outcome.result),
            idempotency_replayed: !outcome.created,
            request_id: request_id.to_owned(),
        })
        .into_response(),
        Err(e) => room_profile_put_error_response(e, request_id),
    }
}

/// `PUT /v1/rooms/{room_id}/profile`（Human Session）：行为与 v1.1 切片逐字节
/// 相同——认证面、actor 与响应映射都没有换。
async fn put_room_profile_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<PutRoomProfileRequest>, JsonRejection>,
) -> Response {
    put_room_profile_response(
        &state,
        &auth.participant,
        &room_id,
        &request_id.0,
        &headers,
        body,
    )
    .await
}

/// `PUT /v1/backend/rooms/{room_id}/profile`（BackendAuth）：Agent 以自己的
/// 身份写它所拥有的群的简介。身份只来自认证——路径里只有房间，没有"以谁
/// 的名义"；凡不是该房当前 active owner 的 Agent 一律 403 `not_room_owner`，
/// 非成员仍 404（不泄露存在性）。
async fn put_backend_room_profile_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<PutRoomProfileRequest>, JsonRejection>,
) -> Response {
    let actor = backend_agent_actor(&auth.view.agent);
    put_room_profile_response(&state, &actor, &room_id, &request_id.0, &headers, body).await
}

fn room_profile_put_error_response(e: RoomProfilePutError, request_id: &str) -> Response {
    match e {
        RoomProfilePutError::InvalidIdempotencyKey => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_IDEMPOTENCY_KEY,
            "invalid Idempotency-Key",
            request_id,
        ),
        RoomProfilePutError::InvalidRequest => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "invalid room profile request body",
            request_id,
        ),
        RoomProfilePutError::InvalidDescription => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_PROFILE_DESCRIPTION,
            "room profile description must be 1..2000 UTF-8 bytes, non-blank and without U+0000",
            request_id,
        ),
        RoomProfilePutError::RoomNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            request_id,
        ),
        RoomProfilePutError::NotRoomOwner => error_response(
            StatusCode::FORBIDDEN,
            codes::NOT_ROOM_OWNER,
            "only the active room owner can write the room profile",
            request_id,
        ),
        RoomProfilePutError::InvalidRoomKind => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_ROOM_KIND,
            "room profile is only available for group rooms",
            request_id,
        ),
        RoomProfilePutError::VersionConflict => error_response(
            StatusCode::CONFLICT,
            codes::PROFILE_VERSION_CONFLICT,
            "room profile base_version is stale",
            request_id,
        ),
        RoomProfilePutError::IdempotencyConflict => error_response(
            StatusCode::CONFLICT,
            codes::IDEMPOTENCY_CONFLICT,
            "idempotency key was already used with a different request",
            request_id,
        ),
        RoomProfilePutError::Store(e) => store_error_response("room_profile.put", &e, request_id),
    }
}

// ---------- 视图 ----------

/// 群简介公共视图（description / version / updated_at）。
pub(crate) fn room_profile_view(p: &RoomProfileRead) -> RoomProfileView {
    RoomProfileView {
        description: p.description.as_str().to_owned(),
        version: p.version.as_u64(),
        updated_at: p.updated_at.to_rfc3339(),
    }
}

/// 单房历史回执的顶层房间头（身份三格 + 简介窄投影）。
pub(crate) fn room_header_view(h: &RoomProfileHeader) -> RoomHeaderView {
    RoomHeaderView {
        id: h.room_id.as_str().to_owned(),
        kind: h.kind.as_str().to_owned(),
        name: h.name.clone(),
        profile: h.profile.as_ref().map(|p| RoomProfileHeadView {
            description: p.description.as_str().to_owned(),
            version: p.version.as_u64(),
        }),
    }
}
