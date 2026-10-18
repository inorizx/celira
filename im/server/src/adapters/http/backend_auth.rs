//! Backend Bearer 认证提取器：独立于 Human Session 的认证上下文。
//!
//! 契约（CORE_V1_BACKEND_BINDINGS.md 6.x）：BackendAuth = active Credential
//! + active Binding + active Agent。它不是 Human `CurrentAuth`，不能调用
//!
//! `/v1/me`、Agent 管理、Room 管理或 Human Message API；Human Session token
//! 也不能通过 BackendAuth。Bearer 解析继续要求一个且仅一个 Authorization
//! header、Bearer scheme（大小写不敏感）、非空 token、不接受逗号合并或
//! 重复 header，候选 token 必须严格符合 `clr_bc_` 固定格式。

use axum::Json;
use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use uuid::Uuid;

use crate::adapters::http::AppState;
use crate::adapters::http::auth::{BearerParse, parse_authorization};
use crate::adapters::http::middleware::RequestId;
use crate::application::backend;
use crate::domain::participant::{DisplayName, Participant, ParticipantKind, ParticipantStatus};
use crate::ports::backend::{AgentAuthView, BackendAuthView};
use crate::ports::storage::StorageFailureKind;
use crate::protocol::error::{ErrorBody, codes};

/// 认证失败响应。缺 header 返回 `authentication_required`；其余一切失败
/// （格式错误、Human token、未知 token、任一级 revoked）统一返回
/// `invalid_credential`，同一错误文案，不形成存在性 oracle。存储失败：
/// `decode_or_corrupt` / `unknown` / `constraint` → 500 `internal_error`
/// （constraint 逃逸到接入层即内部不变量破坏，禁止可重试 503）；
/// `busy_or_locked` / `connection_or_io` → 503 `storage_unavailable`。
#[derive(Debug)]
pub enum BackendAuthError {
    MissingHeader {
        request_id: String,
    },
    InvalidCredential {
        request_id: String,
    },
    Internal {
        request_id: String,
        kind: StorageFailureKind,
    },
}

impl IntoResponse for BackendAuthError {
    fn into_response(self) -> Response {
        let (status, code, message, request_id) = match self {
            Self::MissingHeader { request_id } => (
                StatusCode::UNAUTHORIZED,
                codes::AUTHENTICATION_REQUIRED,
                "missing Authorization: Bearer header",
                request_id,
            ),
            Self::InvalidCredential { request_id } => (
                StatusCode::UNAUTHORIZED,
                codes::INVALID_CREDENTIAL,
                "invalid backend credential",
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

fn request_id_from(parts: &Parts) -> String {
    parts
        .extensions
        .get::<RequestId>()
        .map(|r| r.0.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

/// 把认证上下文里的 Agent **物化成领域 actor**（`Participant`）——Backend 侧凡是
/// 需要"以该 Agent 的身份"调用 application 用例的地方都用这一个构造点。
///
/// 不变量（认证已保证）：`kind=agent`、`status=active`；`display_name` 在创建时
/// 已过 `DisplayName` 校验，此处按恒真不变量构造，绝不可能走到 parse 失败分支。
/// 2026-09-27 批次三第一刀：原来只有 Agent 发言那一处这么构造，现在房治理面
/// （建房/邀请/移除/转让）也要同一个 actor ⇒ 收成一处权威实现，不复制那段不变量。
pub(crate) fn backend_agent_actor(view: &AgentAuthView) -> Participant {
    Participant {
        id: view.id.clone(),
        kind: ParticipantKind::Agent,
        display_name: DisplayName::parse(&view.display_name)
            .expect("authenticated agent display_name was validated at creation (invariant)"),
        status: ParticipantStatus::Active,
        created_at: view.created_at,
    }
}

/// 已认证的 Backend 上下文：Credential → Binding → Agent 解析出的被代表
/// Agent 与所属 Binding/Credential 元数据。
#[derive(Debug, Clone)]
pub struct BackendAuth {
    pub view: BackendAuthView,
}

impl FromRequestParts<AppState> for BackendAuth {
    type Rejection = BackendAuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let request_id = request_id_from(parts);
        let token = match parse_authorization(&parts.headers) {
            BearerParse::Ok(token) => token,
            BearerParse::Missing => return Err(BackendAuthError::MissingHeader { request_id }),
            BearerParse::Invalid => return Err(BackendAuthError::InvalidCredential { request_id }),
        };
        authenticate_token(state, &token, &request_id).await
    }
}

/// 用原始 token 完成 BackendAuth 认证：extractor 与 SSE 唤醒流等无法走
/// extractor 的入口共用；失败映射与 extractor 完全一致（同一错误分类、
/// 同一日志纪律；token 绝不进入日志）。
pub(crate) async fn authenticate_token(
    state: &AppState,
    token: &str,
    request_id: &str,
) -> Result<BackendAuth, BackendAuthError> {
    match backend::authenticate_backend(state.backend.as_ref(), token).await {
        Ok(view) => {
            // 主体进行态：**任何一次成功的 BackendAuth 都算"被看见"**。
            // 这不是心跳上报，而是认证这条既有路径的副产品——它让不挂
            // 门铃的拉取式 Backend 也有一个"最近在线"的兜底（挂门铃的
            // 由连接本身给出在线，这一笔只是重复确认）。
            crate::application::live::mark_seen(
                state.live.as_ref(),
                &view.agent.id,
                crate::domain::time::UtcMillis::now(),
            );
            Ok(BackendAuth { view })
        }
        Err(backend::AuthenticateBackendError::InvalidCredential) => {
            Err(BackendAuthError::InvalidCredential {
                request_id: request_id.to_owned(),
            })
        }
        Err(backend::AuthenticateBackendError::Store(e)) => {
            let error_kind = match e.kind() {
                StorageFailureKind::DecodeOrCorrupt => "decode_or_corrupt",
                StorageFailureKind::Unknown => "internal_invariant",
                StorageFailureKind::Constraint => "constraint",
                _ => "storage_unavailable",
            };
            tracing::error!(
                operation = "backend.authenticate",
                error_kind,
                request_id = %request_id,
                rollback_failed = e.has_rollback_failure(),
                "backend store failure during authentication"
            );
            Err(BackendAuthError::Internal {
                request_id: request_id.to_owned(),
                kind: e.kind(),
            })
        }
    }
}
