//! Group Room Profile（群简介）HTTP handlers
//! （CORE_V1_ROOM_PROFILE.md §3–§5）。
//!
//! 只有一条写端点 `PUT /v1/rooms/{room_id}/profile`（Human Session）。handler
//! 只调用 `application::room_profile` 用例与窄端口，不直接操作 SQLite；所有
//! 非 2xx 走统一 JSON 错误信封。授权（该房当前 active owner 且为 active
//! Human）由存储端口在 `BEGIN IMMEDIATE` 事务内先于重放裁决——路由层绝不
//! 重复弱化检查、不引入第二个写者。
//!
//! 请求体上限 16 KiB：简介正文上限 2000 字节，JSON 转义（换行 / Emoji 等）
//! 后仍有余量；超出返回 413 payload_too_large。正文字段级边界在域层裁决
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
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::routes::{error_response, store_error_response};
use crate::application::room_profile::{RoomProfilePutError, put_room_profile};
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

/// PUT /v1/rooms/{room_id}/profile：current Group active owner（Human）为
/// 整间房写简介。**统一 200**：首次写入产生版本 N，同 key 同摘要重放返回
/// 首次结果（`idempotency_replayed=true`）且不追加版本；同 key 不同摘要
/// 409 `idempotency_conflict`；新 key 但 `base_version` 过期 409
/// `profile_version_conflict`。非成员 / 未知房 404 `room_not_found`；
/// active 成员但非 owner 403 `not_room_owner`；非 group 400 `invalid_room_kind`。
async fn put_room_profile_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<PutRoomProfileRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    let Json(payload) = match parse_put_body(body, &request_id) {
        Ok(p) => p,
        Err(response) => return response,
    };
    let key = match require_key(&headers, &request_id) {
        Ok(key) => key,
        Err(response) => return *response,
    };
    match put_room_profile(
        state.presentation.as_ref(),
        &auth.participant,
        &room_id,
        &key,
        payload.base_version,
        payload.description.as_deref(),
    )
    .await
    {
        Ok(outcome) => Json(PutRoomProfileResponse {
            profile: room_profile_view(&outcome.result),
            idempotency_replayed: !outcome.created,
            request_id,
        })
        .into_response(),
        Err(e) => room_profile_put_error_response(e, &request_id),
    }
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
