//! 请求上下文：request id 生成、日志关联、响应头回写与统一错误信封门禁。
//!
//! 门禁规则（CORE_V1_ERROR_HANDLING.md §5）：任何非 JSON 的非 2xx 响应
//! （Axum 提取器 rejection、WS 升级拒绝、未知的 plain-text 失败等）都在这里
//! 改写成同一 JSON 错误信封，canonical `request_id` 只由本中间件生成，客户端
//! 传入的 `x-request-id` 从不被读取或信任。

use std::time::Instant;

use axum::Json;
use axum::extract::{FromRequestParts, Request};
use axum::http::StatusCode;
use axum::http::header::{CONTENT_TYPE, HeaderValue};
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tracing::{Instrument, info, info_span, warn};
use uuid::Uuid;

use crate::protocol::error::{ErrorBody, codes};

/// 响应头中回写的 request id。
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// 每个请求一个的关联 id（由中间件注入）。
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

impl<S> FromRequestParts<S> for RequestId
where
    S: Send + Sync,
{
    type Rejection = ();

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<RequestId>().cloned().ok_or(())
    }
}

/// 统一错误信封：非 2xx 且不是 JSON 的响应（未知路由/方法不匹配之外的
/// 提取器 rejection、WS 升级拒绝、意外 plain-text 失败）改写成
/// `{error:{code,message,request_id}}`，保留原状态码。
///
/// handler 已封装的业务错误（application/json）原样通过；成功响应（含 101
/// 升级、204）不受影响。改写后由本中间件统一回写 `x-request-id`，body 与
/// header 使用同一个 server-owned ID。
fn enforce_error_envelope(request_id: &str, response: Response) -> Response {
    let status = response.status();
    if !(status.is_client_error() || status.is_server_error()) {
        return response;
    }
    let is_json = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if is_json {
        return response;
    }
    let (code, message) = match status {
        StatusCode::BAD_REQUEST => (codes::INVALID_REQUEST, "invalid request"),
        StatusCode::UNAUTHORIZED => (codes::INVALID_SESSION, "invalid session"),
        StatusCode::NOT_FOUND => (codes::NOT_FOUND, "no such endpoint"),
        StatusCode::METHOD_NOT_ALLOWED => (
            codes::METHOD_NOT_ALLOWED,
            "method not allowed for this endpoint",
        ),
        StatusCode::PAYLOAD_TOO_LARGE => (codes::PAYLOAD_TOO_LARGE, "request body too large"),
        StatusCode::UPGRADE_REQUIRED => {
            (codes::INVALID_REQUEST, "websocket upgrade headers required")
        }
        _ => (codes::INTERNAL_ERROR, "internal error"),
    };
    warn!(
        operation = "http.error_gate",
        error_kind = "unclassified_rejection",
        request_id = %request_id,
        status = status.as_u16(),
        code = %code,
        "unclassified non-JSON error response replaced with safe error envelope"
    );
    (status, Json(ErrorBody::new(code, message, request_id))).into_response()
}

/// 生成 request id、写入日志 span、回写 `x-request-id` 响应头，并保证所有
/// 非 2xx 响应都是统一 JSON 错误信封。
pub async fn request_context(mut req: Request, next: Next) -> Response {
    // canonical request id 只由 Core 生成：客户端传入的 x-request-id 不读取、
    // 不信任、不覆盖。
    let request_id = Uuid::new_v4().to_string();
    req.extensions_mut().insert(RequestId(request_id.clone()));

    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let span = info_span!(
        "http_request",
        request_id = %request_id,
        method = %method,
        path = %path
    );

    let start = Instant::now();
    let mut response = enforce_error_envelope(&request_id, next.run(req).instrument(span).await);
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    info!(
        request_id = %request_id,
        status = response.status().as_u16(),
        latency_ms = start.elapsed().as_millis() as u64,
        "request completed"
    );
    response
}
