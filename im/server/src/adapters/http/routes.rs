//! Resource API 路由与 handler。
//!
//! handler 不直接操作 SQLite：`/ready` 通过 `application::readiness` 用例和
//! `ports::MigrationProbe` 端口完成检查；受保护端点通过
//! `adapters::http::auth::CurrentAuth` 提取器与 `ports::IdentityStore` 端口认证。

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};

use crate::adapters::http::AppState;
use crate::adapters::http::agent_routes;
use crate::adapters::http::attachment_routes;
use crate::adapters::http::attention_routes;
use crate::adapters::http::auth::CurrentAuth;
use crate::adapters::http::backend_block_routes;
use crate::adapters::http::backend_contact_routes;
use crate::adapters::http::backend_directory_routes;
use crate::adapters::http::backend_group_routes;
use crate::adapters::http::backend_leave_routes;
use crate::adapters::http::backend_profile_routes;
use crate::adapters::http::backend_read_cursor_routes;
use crate::adapters::http::backend_read_routes;
use crate::adapters::http::backend_routes;
use crate::adapters::http::backend_stream_routes;
use crate::adapters::http::call_routes;
use crate::adapters::http::contact_routes;
use crate::adapters::http::event_routes;
use crate::adapters::http::home_routes;
use crate::adapters::http::human_event_routes;
use crate::adapters::http::human_profile_routes;
use crate::adapters::http::live_routes;
use crate::adapters::http::message_routes;
use crate::adapters::http::middleware::{REQUEST_ID_HEADER, RequestId};
use crate::adapters::http::participant_avatar_routes;
use crate::adapters::http::participant_presentation_routes;
use crate::adapters::http::room_profile_routes;
use crate::adapters::http::room_routes;
use crate::application::identity;
use crate::application::readiness::{ReadyError, check_ready};
use crate::ports::storage::{StorageError, StorageFailureKind};
use crate::protocol::error::{ErrorBody, StatusOk, codes};
use crate::protocol::me::{MeResponse, ParticipantView, SessionView};

/// Agent 创建请求体的固定上限（4 KiB）。
pub const MAX_AGENT_CREATE_BODY_BYTES: usize = 4096;

/// 本切片的全部路由。
///
/// 路由级请求体上限按入口分开：Agent 与 Direct Room 保持 4 KiB（layer
/// 只作用于其前已注册的路由）；Message 路由在层之后合并，由自身持有
/// 128 KiB 上限。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/v1/me", get(me))
        .route("/v1/sessions/current", delete(revoke_current))
        .merge(home_routes::router())
        .merge(agent_routes::router())
        .merge(room_routes::router())
        // 提醒档（窄解冻切片 1a，契约 `CORE_V1_IM_CONTRACT.md` §9.1）：Human 与
        // Backend 两条**同形**写端点 + 列表里那一格。route-local 1 KiB 请求体
        // 上限在子 router 里，故在下面那层 4 KiB 默认上限之前合并。
        .merge(attention_routes::router())
        .merge(contact_routes::router())
        // 批次三·第二刀「关系对主体开放」：Agent 侧的关系面（看见 / 发起 / 应答 /
        // 解除）。关系是两个主体之间的事实，事实必须够得着。
        .merge(backend_contact_routes::router())
        // 批次三·第三刀 第①步「拉黑对主体开放」：关系的对称面（能加就能拒绝、
        // 能解除就能拉黑）。不进消息主链、被拉黑方一无所知。
        .merge(backend_block_routes::router())
        // 批次四·表达权第一刀：Agent 写**自己**的资料（身份只来自认证）。
        .merge(backend_profile_routes::router())
        // 表达权第四刀（2026-10-17）：**人写自己的**名字与头像（身份只来自认证，路径里没有"写谁的"）。
        .merge(human_profile_routes::router())
        .merge(backend_routes::router())
        .merge(backend_directory_routes::router())
        // Agent 读取面对齐（窄解冻切片 CORE_V1_AGENT_READ_SURFACE.md）：6 条
        // 纯读端点，复用 Human 侧同一用例与同一投影，零写语义零持久新增。
        // （原 7 条：`GET /v1/backend/publication` 随 Agent 服务层退役删除。）
        .merge(backend_read_routes::router())
        .merge(backend_read_cursor_routes::router())
        .merge(backend_leave_routes::router())
        // 批次三·第一刀「owner = 创建者」：Agent 侧的房治理面（建房 / 邀请 / 移除 /
        // 转让）。owner 是房内的治理角色（`role`），不是参与者种类（`kind`）——
        // 这四条让"一支 Agent 也能当 owner"成为**可达且可退出**的状态。
        .merge(backend_group_routes::router())
        .merge(event_routes::router())
        .merge(backend_stream_routes::router())
        .merge(human_event_routes::router())
        .layer(axum::extract::DefaultBodyLimit::max(
            MAX_AGENT_CREATE_BODY_BYTES,
        ))
        .merge(live_routes::router())
        .merge(message_routes::router())
        .merge(attachment_routes::router())
        // Participant Presentation 切片：在 4 KiB 默认上限之后注册，自带独立
        // 较大的 PUT body 上限（Profile/Presentation 正文字段可达 16 KiB）。
        // Room 可见性与 Agent Owner 授权复用既有 Room/Agent 查询，路由只
        // 组合用例，不重复弱化检查。
        .merge(participant_presentation_routes::router())
        // Group Room Profile（v1.1 附加切片，契约 CORE_V1_ROOM_PROFILE.md）：
        // 群简介写端点（该房 active owner 且为 active Human；授权在存储事务内
        // 先于重放裁决）。自带 16 KiB 请求体上限，故在 4 KiB 默认层之后合并。
        .merge(room_profile_routes::router())
        // Participant Avatar 切片（窄解冻切片）：在 4 KiB 默认上限之后注册，
        // PUT 自带 2 MiB + 开销的独立上限（multipart）。写路径只对 active
        // Agent Owner，读路径与成员详情同一 Room 可见性裁决。
        .merge(participant_avatar_routes::router())
        // Call 切片：在 4 KiB 默认上限之后注册，信令 POST 自带独立上限。
        .merge(call_routes::router())
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
}

/// GET /health：进程存活即可返回 200（匿名）。
async fn health(request_id: RequestId) -> Response {
    Json(StatusOk {
        status: StatusOk::STATUS,
        request_id: request_id.0,
    })
    .into_response()
}

/// GET /ready：SQLite 可连接且迁移已全部应用才返回 200，否则非 2xx（匿名）。
///
/// 失败响应只使用固定安全文案：迁移计数与探针原因只进入结构化日志，绝不
/// 拼入公开 message（CORE_V1_ERROR_HANDLING.md §5.3）。
async fn ready(State(state): State<AppState>, request_id: RequestId) -> Response {
    let request_id = request_id.0;
    match check_ready(state.probe.as_ref()).await {
        Ok(()) => Json(StatusOk {
            status: StatusOk::STATUS,
            request_id,
        })
        .into_response(),
        Err(ReadyError::NotReady { pending, failed }) => {
            tracing::warn!(
                operation = "readiness.check",
                error_kind = "not_ready",
                pending,
                failed,
                request_id = %request_id,
                "service not ready: migrations pending"
            );
            error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                codes::NOT_READY,
                "service not ready",
                &request_id,
            )
        }
        Err(ReadyError::ProbeFailed(_)) => {
            tracing::error!(
                operation = "readiness.check",
                error_kind = "storage_unavailable",
                request_id = %request_id,
                "storage probe failed"
            );
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                codes::INTERNAL_ERROR,
                "storage probe failed",
                &request_id,
            )
        }
    }
}

/// GET /v1/me：返回当前认证上下文（participant + session + request_id）。
/// 不返回 token、token hash 或其他 Session 信息。
async fn me(auth: CurrentAuth, request_id: RequestId) -> Response {
    let participant = &auth.participant;
    Json(MeResponse {
        participant: ParticipantView {
            id: participant.id.as_str().to_owned(),
            kind: participant.kind.as_str().to_owned(),
            display_name: participant.display_name.as_str().to_owned(),
            status: participant.status.as_str().to_owned(),
            created_at: participant.created_at.to_rfc3339(),
        },
        session: SessionView {
            id: auth.session.id.as_str().to_owned(),
            created_at: auth.session.created_at.to_rfc3339(),
        },
        request_id: request_id.0,
    })
    .into_response()
}

/// DELETE /v1/sessions/current：认证当前 Session 后幂等撤销。
/// 持久化提交后返回 204（携带 x-request-id）；不删除 Human，不撤销其他
/// Session。已撤销 token 无法再次认证，之后的重复请求返回 401。Store 失败
/// 复用统一 StorageFailureKind 分类：`busy_or_locked` / `connection_or_io`
/// → 503 storage_unavailable；其余（`decode_or_corrupt` / `constraint` /
/// `unknown`）→ 500 internal_error；错误日志带 canonical request_id。
async fn revoke_current(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    match identity::revoke_session(state.identity.as_ref(), &auth.session.id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(identity::RevokeError::Store(e)) => {
            store_error_response("identity.session.revoke", &e, &request_id)
        }
    }
}

/// 未知路径。
async fn not_found(request_id: RequestId) -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        codes::NOT_FOUND,
        "no such endpoint",
        &request_id.0,
    )
}

/// 路径存在但方法不被允许（如 POST /health）。
async fn method_not_allowed(request_id: RequestId) -> Response {
    error_response(
        StatusCode::METHOD_NOT_ALLOWED,
        codes::METHOD_NOT_ALLOWED,
        "method not allowed for this endpoint",
        &request_id.0,
    )
}

/// 统一错误响应：稳定 error code + request id。
pub(crate) fn error_response(
    status: StatusCode,
    code: &'static str,
    message: impl Into<String>,
    request_id: &str,
) -> Response {
    (status, Json(ErrorBody::new(code, message, request_id))).into_response()
}

/// Store 基础设施失败 → 稳定公开响应（CORE_V1_ERROR_HANDLING.md §5.2/§7.1）：
/// `decode_or_corrupt`、`unknown` 与 `constraint` 是确定性内部错误（500
/// `internal_error`，禁止可重试 503；`constraint` 只有已被应用层吸收为既有
/// 业务冲突才走业务 code，逃逸到这里的 constraint 一律视为内部不变量
/// 破坏）；`busy_or_locked` / `connection_or_io` 是运行时存储暂时不可用
/// （503 `storage_unavailable`）。只读稳定 kind，绝不把 cause 文本写入
/// 日志或公开响应；cause 链保留在错误对象内供诊断。日志显式携带
/// canonical request_id 与 rollback 复合标记。
pub(crate) fn store_error_response(
    operation: &'static str,
    e: &StorageError,
    request_id: &str,
) -> Response {
    let (status, code, message, error_kind) = match e.kind() {
        StorageFailureKind::DecodeOrCorrupt => (
            StatusCode::INTERNAL_SERVER_ERROR,
            codes::INTERNAL_ERROR,
            "internal error",
            "decode_or_corrupt",
        ),
        StorageFailureKind::Unknown => (
            StatusCode::INTERNAL_SERVER_ERROR,
            codes::INTERNAL_ERROR,
            "internal error",
            "internal_invariant",
        ),
        StorageFailureKind::Constraint => (
            StatusCode::INTERNAL_SERVER_ERROR,
            codes::INTERNAL_ERROR,
            "internal error",
            "constraint",
        ),
        StorageFailureKind::BusyOrLocked | StorageFailureKind::ConnectionOrIo => (
            StatusCode::SERVICE_UNAVAILABLE,
            codes::STORAGE_UNAVAILABLE,
            "storage temporarily unavailable",
            "storage_unavailable",
        ),
    };
    tracing::error!(
        operation,
        error_kind,
        request_id = %request_id,
        rollback_failed = e.has_rollback_failure(),
        rollback_error_kind = e.rollback_kind().map(|k| k.as_str()),
        "store failure"
    );
    error_response(status, code, message, request_id)
}

/// 便于测试读取响应体中的 request id（中间件回写响应头）。
#[allow(dead_code)]
pub(crate) fn response_request_id(response: &Response) -> Option<String> {
    response
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}
