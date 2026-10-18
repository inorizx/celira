//! Avatar HTTP handlers（契约 CORE_V1_AVATAR_P0.md §3–§4）。
//!
//! handler 只调用 `application::participant_avatar` 用例与窄端口，不直接
//! 操作 SQLite / 文件系统。写路径（PUT/DELETE）只对 active Agent Owner
//! Human Session 开放，授权在存储事务内先于重放裁决；读路径（GET 字节）与
//! 成员详情同一 Room 可见性裁决。公共响应绝不返回 storage key、SHA-256 或
//! 文件路径；下载响应带 nosniff + 按版本寻址的缓存头。
//!
//! PUT 是严格形状 multipart：恰好一个 `base_version` 字段 part + 恰好一个
//! `name="file"` 文件 part（Content-Disposition 必须带 filename、part
//! Content-Type 必须存在且过白名单）；文件字节 ≤ 2 MiB；路由级请求体上限 =
//! 2 MiB + 64 KiB（multipart 框架开销），超出 413。

use axum::body::Body;
use axum::extract::multipart::{Multipart, MultipartError};
use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, X_CONTENT_TYPE_OPTIONS};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};

use crate::adapters::http::AppState;
use crate::adapters::http::agent_routes::{KeyError, idempotency_key_from_headers};
use crate::adapters::http::auth::CurrentAuth;
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::participant_presentation_routes::{
    presentation_query_error_response, profile_view,
};
use crate::adapters::http::routes::{error_response, store_error_response};
use crate::application::participant_avatar::{
    AvatarWriteError, get_member_avatar_bytes, put_owner_avatar, remove_owner_avatar,
};
use crate::domain::participant::Participant;
use crate::domain::participant_avatar::MAX_AVATAR_BYTES;
use crate::ports::participant_presentation::LatestAvatar;
use crate::protocol::error::codes;
use crate::protocol::participant_avatar::{AvatarWriteResponse, DeleteAvatarRequest};

/// 上传路由级请求体上限：2 MiB 文件字节 + 64 KiB multipart 框架开销。
pub const MAX_AVATAR_REQUEST_BYTES: usize = MAX_AVATAR_BYTES + 64 * 1024;

/// Avatar 切片路由。PUT/DELETE 共享 Owner 写路径；GET 是 Room 成员读路径。
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/agents/{agent_id}/profile/avatar",
            put(put_avatar_route).delete(delete_avatar_route),
        )
        .route(
            "/v1/rooms/{room_id}/members/{participant_id}/avatar",
            get(get_avatar_route),
        )
        .layer(axum::extract::DefaultBodyLimit::max(
            MAX_AVATAR_REQUEST_BYTES,
        ))
}

/// multipart 解析错误 → 413（路由级字节超限）或 400 avatar_invalid（形状错）。
fn multipart_error_response(e: MultipartError, request_id: &str) -> Response {
    if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
        error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            codes::PAYLOAD_TOO_LARGE,
            format!("request body exceeds {MAX_AVATAR_REQUEST_BYTES} bytes"),
            request_id,
        )
    } else {
        error_response(
            StatusCode::BAD_REQUEST,
            codes::AVATAR_INVALID,
            "invalid multipart avatar request",
            request_id,
        )
    }
}

struct AvatarPutParts {
    base_version: String,
    content_type: String,
    bytes: axum::body::Bytes,
}

/// 严格形状 multipart 解析：恰好一个 `base_version` 字段 part + 恰好一个
/// `name="file"` 文件 part（filename 与 Content-Type 必填）；额外/缺失/
/// 重复 part 一律 400 avatar_invalid。文件字节超 2 MiB → 413（multipart
/// 框架在字段级裁决）。
// axum 的 `Response` **天生大**（几个头 + body 的枚举），而这里是"把一次请求解析失败
// 直接翻成一个响应"的窄助手 ⇒ 错误路径罕见、**不为它把每个调用点都加一层 `Box` 解引用**。
// 判据一句话：**这一条是样式 lint，不是正确性判据**（`-D warnings` 下必须显式表态）。
#[allow(clippy::result_large_err)]
async fn parse_avatar_parts(
    mut multipart: Multipart,
    request_id: &str,
) -> Result<AvatarPutParts, Response> {
    let mut base_version: Option<String> = None;
    let mut file: Option<(String, axum::body::Bytes)> = None;
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                if field.name() == Some("base_version") {
                    if base_version.is_some() {
                        return Err(error_response(
                            StatusCode::BAD_REQUEST,
                            codes::AVATAR_INVALID,
                            "avatar request must contain exactly one base_version field",
                            request_id,
                        ));
                    }
                    let text = match field.text().await {
                        Ok(text) => text,
                        Err(e) => return Err(multipart_error_response(e, request_id)),
                    };
                    base_version = Some(text);
                } else if field.name() == Some("file") {
                    if file.is_some() {
                        return Err(error_response(
                            StatusCode::BAD_REQUEST,
                            codes::AVATAR_INVALID,
                            "avatar request must contain exactly one file part",
                            request_id,
                        ));
                    }
                    if field.file_name().is_none() {
                        return Err(error_response(
                            StatusCode::BAD_REQUEST,
                            codes::AVATAR_INVALID,
                            "file part must carry a file name",
                            request_id,
                        ));
                    }
                    let Some(content_type) = field.content_type().map(str::to_owned) else {
                        return Err(error_response(
                            StatusCode::BAD_REQUEST,
                            codes::AVATAR_INVALID,
                            "file part must carry a content type",
                            request_id,
                        ));
                    };
                    let bytes = match field.bytes().await {
                        Ok(bytes) => bytes,
                        Err(e) => return Err(multipart_error_response(e, request_id)),
                    };
                    if bytes.len() > MAX_AVATAR_BYTES {
                        return Err(error_response(
                            StatusCode::PAYLOAD_TOO_LARGE,
                            codes::PAYLOAD_TOO_LARGE,
                            format!("avatar exceeds {MAX_AVATAR_BYTES} bytes"),
                            request_id,
                        ));
                    }
                    file = Some((content_type, bytes));
                } else {
                    return Err(error_response(
                        StatusCode::BAD_REQUEST,
                        codes::AVATAR_INVALID,
                        "avatar request must contain only base_version and file parts",
                        request_id,
                    ));
                }
            }
            Ok(None) => break,
            Err(e) => return Err(multipart_error_response(e, request_id)),
        }
    }
    let Some(base_version) = base_version else {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            codes::AVATAR_INVALID,
            "avatar request must contain a base_version field",
            request_id,
        ));
    };
    let Some((content_type, bytes)) = file else {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            codes::AVATAR_INVALID,
            "avatar request must contain a file part",
            request_id,
        ));
    };
    Ok(AvatarPutParts {
        base_version,
        content_type,
        bytes,
    })
}

/// 严格读取 Idempotency-Key：缺失 400 idempotency_key_required；重复 header /
/// 逗号合并 400 invalid_idempotency_key；其余格式交给域层。
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

/// PUT /v1/agents/{agent_id}/profile/avatar：active Agent Owner 唯一写路径。
/// 首次创建（版本 0 → 1）201、后续更新 200、同 key 同摘要重放返回首次成功
/// 时的状态；`idempotency_replayed` 区分真实重放。
async fn put_avatar_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
    multipart: Multipart,
) -> Response {
    put_avatar_response(
        &state,
        &auth.participant,
        &agent_id,
        &request_id.0,
        &headers,
        multipart,
    )
    .await
}

/// 头像写入的**共用实现**（批次四表达权第二刀）：Human 侧（Owner 写它名下 Agent 的
/// 头像）与 Agent 侧（`PUT /v1/backend/me/avatar`，**写自己的**）走同一段——同一个
/// application 用例、同一段响应构造、同一套错误码，只在适配器换认证面。
///
/// `target_id` 是要写谁的头像：Human 侧来自路径，Agent 侧来自**认证本身**。
pub(crate) async fn put_avatar_response(
    state: &AppState,
    actor: &Participant,
    target_id: &str,
    request_id: &str,
    headers: &HeaderMap,
    multipart: Multipart,
) -> Response {
    let request_id = request_id.to_owned();
    let key = match require_key(headers, &request_id) {
        Ok(key) => key,
        Err(response) => return *response,
    };
    let parts = match parse_avatar_parts(multipart, &request_id).await {
        Ok(parts) => parts,
        Err(response) => return response,
    };
    let base_version: i64 = match parts.base_version.parse::<i64>() {
        Ok(v) => v,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::AVATAR_INVALID,
                "base_version must be an integer",
                &request_id,
            );
        }
    };
    match put_owner_avatar(
        state.presentation.as_ref(),
        state.blob.as_ref(),
        actor,
        target_id,
        &key,
        base_version,
        &parts.content_type,
        &parts.bytes,
    )
    .await
    {
        Ok(outcome) => {
            let status = if outcome.result_is_create {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (
                status,
                Json(AvatarWriteResponse {
                    profile: profile_view(&outcome.result),
                    idempotency_replayed: !outcome.created,
                    request_id,
                }),
            )
                .into_response()
        }
        Err(e) => avatar_write_error_response(e, &request_id),
    }
}

/// DELETE /v1/agents/{agent_id}/profile/avatar：摘除 = 新版本 avatar=null。
/// 一律 200（即使从无头像状态推进版本 0 → 1）；同 key 同摘要恒重放。
async fn delete_avatar_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
    body: Result<Json<DeleteAvatarRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    delete_avatar_response(
        &state,
        &auth.participant,
        &agent_id,
        &request_id.0,
        &headers,
        body,
    )
    .await
}

/// 头像摘除的**共用实现**（批次四表达权第二刀）：与 [`put_avatar_response`] 同一条分叉。
pub(crate) async fn delete_avatar_response(
    state: &AppState,
    actor: &Participant,
    target_id: &str,
    request_id: &str,
    headers: &HeaderMap,
    body: Result<Json<DeleteAvatarRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let request_id = request_id.to_owned();
    let key = match require_key(headers, &request_id) {
        Ok(key) => key,
        Err(response) => return *response,
    };
    let Json(payload) = match body {
        Ok(payload) => payload,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "invalid JSON request body",
                &request_id,
            );
        }
    };
    match remove_owner_avatar(
        state.presentation.as_ref(),
        actor,
        target_id,
        &key,
        payload.base_version,
    )
    .await
    {
        Ok(outcome) => (
            StatusCode::OK,
            Json(AvatarWriteResponse {
                profile: profile_view(&outcome.result),
                idempotency_replayed: !outcome.created,
                request_id,
            }),
        )
            .into_response(),
        Err(e) => avatar_write_error_response(e, &request_id),
    }
}

/// GET /v1/rooms/{room_id}/members/{participant_id}/avatar：requester 与
/// target 都必须当前 active 成员（与成员详情同一裁决）；无头像统一 404
/// `avatar_not_found`。下载响应带存储 content_type、nosniff 与按版本寻址
/// 的缓存头（ETag = 头像所在 Profile 版本行版本；文本字段单独更新会推进
/// 版本造成缓存失效，是保守正确方向——头像字节本身随版本行不可变）。
async fn get_avatar_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path((room_id, participant_id)): Path<(String, String)>,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    match get_member_avatar_bytes(
        state.room.as_ref(),
        state.presentation.as_ref(),
        state.blob.as_ref(),
        &auth.participant.id,
        &room_id,
        &participant_id,
    )
    .await
    {
        Ok(Some((latest, bytes))) => avatar_bytes_response(&latest, bytes),
        Ok(None) => error_response(
            StatusCode::NOT_FOUND,
            codes::AVATAR_NOT_FOUND,
            "avatar not found",
            &request_id,
        ),
        Err(e) => presentation_query_error_response(e, &request_id),
    }
}

/// 头像字节 → 下载响应：存储 content_type + `nosniff` + ETag（按 Profile
/// 版本寻址）+ `private, max-age=86400`。Human 侧与 Agent 侧读路径**共用**
/// 本函数，保证响应头逐字段一致（CORE_V1_AGENT_READ_SURFACE.md §2.1）。
pub(crate) fn avatar_bytes_response(latest: &LatestAvatar, bytes: Vec<u8>) -> Response {
    let mut response = Response::new(Body::from(bytes));
    let content_type = HeaderValue::from_str(latest.avatar.content_type.as_str())
        .expect("stored avatar content type must be a valid header value");
    response.headers_mut().insert(CONTENT_TYPE, content_type);
    response
        .headers_mut()
        .insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    let etag = HeaderValue::from_str(&format!("\"v{}\"", latest.profile_version.as_u64()))
        .expect("etag derived from u64 is a valid header value");
    response.headers_mut().insert(ETAG, etag);
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400"),
    );
    response
}

/// 写路径错误映射（契约 §4）：非法输入 400 avatar_invalid、超限 413、
/// 非 Owner 404 agent_not_found、revoked 409 agent_not_active、幂等/版本
/// 冲突复用 P0 家族（idempotency_conflict / profile_stale）。
fn avatar_write_error_response(e: AvatarWriteError, request_id: &str) -> Response {
    match e {
        AvatarWriteError::InvalidIdempotencyKey => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_IDEMPOTENCY_KEY,
            "invalid Idempotency-Key",
            request_id,
        ),
        AvatarWriteError::InvalidBody => error_response(
            StatusCode::BAD_REQUEST,
            codes::AVATAR_INVALID,
            "invalid avatar body",
            request_id,
        ),
        AvatarWriteError::PayloadTooLarge => error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            codes::PAYLOAD_TOO_LARGE,
            format!("avatar exceeds {MAX_AVATAR_BYTES} bytes"),
            request_id,
        ),
        AvatarWriteError::NotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::AGENT_NOT_FOUND,
            "agent not found",
            request_id,
        ),
        AvatarWriteError::NotActive => error_response(
            StatusCode::CONFLICT,
            codes::AGENT_NOT_ACTIVE,
            "agent is not active",
            request_id,
        ),
        AvatarWriteError::IdempotencyConflict => error_response(
            StatusCode::CONFLICT,
            codes::IDEMPOTENCY_CONFLICT,
            "idempotency key was already used with a different request",
            request_id,
        ),
        AvatarWriteError::Stale => error_response(
            StatusCode::CONFLICT,
            codes::PROFILE_STALE,
            "profile base_version is stale",
            request_id,
        ),
        AvatarWriteError::Store(e) => {
            store_error_response("participant_avatar.write", &e, request_id)
        }
    }
}
