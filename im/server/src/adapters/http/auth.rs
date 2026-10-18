//! Bearer 认证提取器：严格解析 `Authorization` 并通过身份用例认证。
//!
//! 契约（CORE_V1_IDENTITY_AUTH.md 6.1）：缺失 header → `authentication_required`；
//! 非 Bearer、空 token、重复 header、格式错误 → `invalid_session`；未知、已
//! 撤销或指向非 active Human 的 token → `invalid_session`。所有失败响应携带
//! 稳定 `error.code`、`error.request_id` 与 `x-request-id`。

use axum::Json;
use axum::extract::FromRequestParts;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::adapters::http::AppState;
use crate::adapters::http::middleware::RequestId;
use crate::application::identity;
use crate::domain::human_session::HumanSession;
use crate::domain::participant::Participant;
use crate::ports::storage::StorageFailureKind;
use crate::protocol::error::{ErrorBody, codes};

/// 认证失败响应。
#[derive(Debug)]
pub enum AuthError {
    /// 缺少 Authorization header：401 `authentication_required`。
    MissingHeader { request_id: String },
    /// 格式错误或 Session 无效：401 `invalid_session`。
    InvalidSession { request_id: String },
    /// 身份存储失败：`decode_or_corrupt` / `unknown` / `constraint` → 500
    /// `internal_error`（constraint 逃逸到接入层即内部不变量破坏，禁止
    /// 可重试 503）；`busy_or_locked` / `connection_or_io` → 503
    /// `storage_unavailable`。
    Internal {
        request_id: String,
        kind: StorageFailureKind,
    },
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, code, message, request_id) = match self {
            Self::MissingHeader { request_id } => (
                StatusCode::UNAUTHORIZED,
                codes::AUTHENTICATION_REQUIRED,
                "missing Authorization: Bearer header",
                request_id,
            ),
            Self::InvalidSession { request_id } => (
                StatusCode::UNAUTHORIZED,
                codes::INVALID_SESSION,
                "invalid session",
                request_id,
            ),
            Self::Internal { request_id, kind } => match kind {
                StorageFailureKind::DecodeOrCorrupt
                | StorageFailureKind::Unknown
                | StorageFailureKind::Constraint => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    codes::INTERNAL_ERROR,
                    "internal error",
                    request_id,
                ),
                StorageFailureKind::BusyOrLocked | StorageFailureKind::ConnectionOrIo => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    codes::STORAGE_UNAVAILABLE,
                    "storage temporarily unavailable",
                    request_id,
                ),
            },
        };
        (status, Json(ErrorBody::new(code, message, request_id))).into_response()
    }
}

pub(crate) enum BearerParse {
    Ok(String),
    Missing,
    Invalid,
}

/// 严格解析 Authorization 头。规则：
/// - 一个且仅一个 header 值（`get_all` 与逗号合并都视为重复）；
/// - scheme 为 Bearer（大小写不敏感）；
/// - 必须有一个非空 token；token 正文不做修剪，留给领域层格式校验。
pub(crate) fn parse_authorization(headers: &HeaderMap) -> BearerParse {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let Some(first) = values.next() else {
        return BearerParse::Missing;
    };
    if values.next().is_some() {
        return BearerParse::Invalid;
    }
    let Ok(value) = first.to_str() else {
        return BearerParse::Invalid;
    };
    if value.split(',').count() > 1 {
        return BearerParse::Invalid;
    }
    let Some((scheme, token)) = value.split_once(' ') else {
        return BearerParse::Invalid;
    };
    if !scheme.eq_ignore_ascii_case("bearer") {
        return BearerParse::Invalid;
    }
    if token.is_empty() {
        return BearerParse::Invalid;
    }
    BearerParse::Ok(token.to_owned())
}

fn request_id_from(parts: &Parts) -> String {
    parts
        .extensions
        .get::<RequestId>()
        .map(|r| r.0.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

/// 已认证上下文：Bearer token 通过身份用例校验后注入 handler。
#[derive(Debug, Clone)]
pub struct CurrentAuth {
    pub participant: Participant,
    pub session: HumanSession,
}

impl FromRequestParts<AppState> for CurrentAuth {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let request_id = request_id_from(parts);
        let token = match parse_authorization(&parts.headers) {
            BearerParse::Ok(token) => token,
            BearerParse::Missing => return Err(AuthError::MissingHeader { request_id }),
            BearerParse::Invalid => return Err(AuthError::InvalidSession { request_id }),
        };

        match identity::authenticate(state.identity.as_ref(), &token).await {
            Ok(context) => Ok(Self {
                participant: context.participant,
                session: context.session,
            }),
            Err(
                identity::AuthenticateError::InvalidToken
                | identity::AuthenticateError::UnknownSession,
            ) => Err(AuthError::InvalidSession { request_id }),
            Err(identity::AuthenticateError::Store(e)) => {
                let error_kind = match e.kind() {
                    StorageFailureKind::DecodeOrCorrupt => "decode_or_corrupt",
                    StorageFailureKind::Unknown => "internal_invariant",
                    StorageFailureKind::Constraint => "constraint",
                    _ => "storage_unavailable",
                };
                tracing::error!(
                    operation = "identity.authenticate",
                    error_kind,
                    request_id = %request_id,
                    rollback_failed = e.has_rollback_failure(),
                    "identity store failure during authentication"
                );
                Err(AuthError::Internal {
                    request_id,
                    kind: e.kind(),
                })
            }
        }
    }
}
