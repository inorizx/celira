//! A 包 HTTP 路由（CORE_V1_IM_CONTRACT.md §3.1）：Home 身份、Pairing
//! Grant 与 Session 管理。
//!
//! - `GET /v1/home`：认证 Human 读取 Home Identity（非匿名发现）；
//! - `POST /v1/home/pairing-grants`：认证 Human 签发（token 只输出一次）；
//! - `GET /v1/home/pairing-grants`：认证 Human 列出本人未消费 Grant；
//! - `DELETE /v1/home/pairing-grants/{id}`：认证 Human 幂等撤销；
//! - `POST /v1/home/pairing-grants/exchange`：**匿名**，只接受 Grant
//!   Bearer（Human Session token 一律 `grant_invalid`），原子兑换独立
//!   Human Session；
//! - `GET /v1/sessions` / `DELETE /v1/sessions/{id}`：认证 Human 管理
//!   自己的 Session（只返回非秘密元数据；独立撤销）。
//!
//! A2（HOME_ACCESS.md §4.3 A2，窄解冻切片）：Pairing Transaction 可靠
//! 提交协议。全部 A2 端点**匿名**，认证一律经 token 摘要寻址，不暴露
//! 存在性：
//! - `POST /v1/home/pairing-grants/prepare`：凭 Grant Bearer + Host 生成
//!   的 `client_nonce`、`session_token_hash`、`commit_token_hash`（均为
//!   固定长度 SHA-256 摘要）；创建或幂等重用 `pairing_transaction`，返回
//!   transaction_id / Home Identity / issued_by 非秘密身份 / reserved
//!   session id / expires_at。/ 原始 Session / commit token 从不进入 Core。
//! - `GET /v1/home/pairing-transactions/{id}`：凭 commit token Bearer 查询
//!   非秘密状态（prepared/committed/expired/aborted）；仅供 Host 恢复，
//!   不给浏览器直连。
//! - `POST /v1/home/pairing-transactions/{id}/commit`：凭 commit token
//!   Bearer 在一个事务内完成正式 Human Session 创建 + session_metadata +
//!   标记 committed + 消费 Grant；重试幂等，绝不产生第二 Session。
//! - `DELETE /v1/home/pairing-transactions/{id}`：凭 commit token Bearer
//!   主动放弃 prepared 事务；不影响 Grant 的 active 状态。

use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, FailedToBufferBody, JsonRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::adapters::http::AppState;
use crate::adapters::http::auth::{BearerParse, CurrentAuth, parse_authorization};
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::routes::{error_response, store_error_response};
use crate::application::home;
use crate::domain::home::GrantId;
use crate::domain::human_session::SessionId;
use crate::domain::pairing_transaction::{TransactionId, TransactionView};
use crate::domain::time::UtcMillis;
use crate::protocol::error::codes;
use crate::protocol::home::{
    CommitTransactionBody, ExchangeHomeView, ExchangeResponse, ExchangeSessionView,
    GrantIssueResponse, GrantListResponse, GrantView, HomeResponse, PrepareTransactionRequest,
    SessionListResponse, SessionView, TransactionResponse, transaction_response,
};

/// 本切片的全部路由。请求体极小，注册在 4 KiB DefaultBodyLimit 层之前。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/home", get(get_home))
        .route(
            "/v1/home/pairing-grants",
            post(issue_grant).get(list_grants),
        )
        .route("/v1/home/pairing-grants/{grant_id}", delete(revoke_grant))
        .route("/v1/home/pairing-grants/exchange", post(exchange_grant))
        .route("/v1/home/pairing-grants/prepare", post(prepare_transaction))
        .route(
            "/v1/home/pairing-transactions/{transaction_id}",
            get(get_transaction_status).delete(abort_transaction),
        )
        .route(
            "/v1/home/pairing-transactions/{transaction_id}/commit",
            post(commit_transaction),
        )
        .route("/v1/sessions", get(list_sessions))
        .route("/v1/sessions/{session_id}", delete(revoke_session))
}

/// 解析请求体；Payload 超限 → 413，其余 JSON 形状错误 → 400。
/// Err 变体 Box 化：axum `Response` 体积大，按值返回触发 clippy::result_large_err。
fn parse_body<T>(body: Result<Json<T>, JsonRejection>, request_id: &str) -> Result<T, Box<Response>>
where
    T: serde::de::DeserializeOwned,
{
    match body {
        Ok(Json(payload)) => Ok(payload),
        Err(JsonRejection::BytesRejection(BytesRejection::FailedToBufferBody(
            FailedToBufferBody::LengthLimitError(_),
        ))) => Err(Box::new(error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            codes::PAYLOAD_TOO_LARGE,
            "request body too large",
            request_id,
        ))),
        Err(_) => Err(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "invalid JSON request body",
            request_id,
        ))),
    }
}

/// GET /v1/home：认证后返回 Home Identity，供绑定核验与展示；未认证
/// 401（Home 不是匿名发现对象）。Home 行缺失是内部不变量破坏（启动与
/// CLI 都幂等初始化），归 500 internal_error。
async fn get_home(
    State(state): State<AppState>,
    _auth: CurrentAuth,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    match home::get_home(state.home.as_ref()).await {
        Ok(Some(home_identity)) => Json(HomeResponse {
            home_id: home_identity.home_id.as_str().to_owned(),
            display_name: home_identity
                .display_name
                .as_ref()
                .map(|n| n.as_str().to_owned()),
            created_at: home_identity.created_at.to_rfc3339(),
            request_id,
        })
        .into_response(),
        Ok(None) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            codes::INTERNAL_ERROR,
            "internal error",
            &request_id,
        ),
        Err(home::HomeError::Store(e)) => store_error_response("home.get", &e, &request_id),
    }
}

/// POST /v1/home/pairing-grants 请求体：可选 device_label 与 grants
/// （未知字段拒绝）。
///
/// `grants` ∈ {`self_device`, `new_human`}，**缺省 `self_device`**——老客户端
/// 一字不改地继续工作。`new_human` = 邀请一个人进这个世界（TTL 明显更长，
/// 仍一次性、仍可撤销）；**不做匿名自注册**：一个世界的入口只有"建一个世界"
/// 与"被请进一个世界"两条。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IssueGrantBody {
    #[serde(default)]
    device_label: Option<String>,
    #[serde(default)]
    grants: Option<String>,
}

/// POST /v1/home/pairing-grants：认证 Human 签发 Grant；成功响应返回
/// grant 元数据与一次性 raw token（此后不可恢复）。
async fn issue_grant(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
    body: Result<Json<IssueGrantBody>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    let body = match parse_body(body, &request_id) {
        Ok(payload) => payload,
        Err(response) => return *response,
    };
    let device_label = body.device_label.as_deref();
    // 封闭词表：未知取值一律 400（不猜、不静默降级成设备配对）。
    let kind = match body.grants.as_deref() {
        None => crate::domain::home::GrantKind::SelfDevice,
        Some(raw) => match crate::domain::home::GrantKind::parse(raw) {
            Some(kind) => kind,
            None => {
                return error_response(
                    StatusCode::BAD_REQUEST,
                    codes::INVALID_REQUEST,
                    "unknown grants value",
                    &request_id,
                );
            }
        },
    };
    let ttl_minutes = state.grant_ttl_minutes;
    match home::issue_grant(
        state.home.as_ref(),
        &auth.participant.id,
        device_label,
        kind,
        ttl_minutes,
    )
    .await
    {
        Ok(issued) => {
            let grant = &issued.grant;
            (
                StatusCode::CREATED,
                Json(GrantIssueResponse {
                    grant_id: grant.grant_id.as_str().to_owned(),
                    status: grant.status.as_str().to_owned(),
                    grants: grant.kind.as_str().to_owned(),
                    device_label: grant.device_label.as_ref().map(|n| n.as_str().to_owned()),
                    issued_at: grant.issued_at.to_rfc3339(),
                    expires_at: grant.expires_at.to_rfc3339(),
                    token: issued.token.as_str().to_owned(),
                    request_id,
                }),
            )
                .into_response()
        }
        // CurrentAuth 保证当前 Human 存在且 active：未知/非 active 在
        // 认证路径上不可达，到达即内部不变量破坏（500）。
        Err(home::IssueGrantError::UnknownHuman | home::IssueGrantError::HumanNotActive) => {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                codes::INTERNAL_ERROR,
                "internal error",
                &request_id,
            )
        }
        Err(home::IssueGrantError::DeviceLabel(_)) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "invalid device_label",
            &request_id,
        ),
        Err(home::IssueGrantError::Token(_)) => {
            tracing::error!(
                operation = "home.grant.issue",
                error_kind = "secure_random_unavailable",
                request_id = %request_id,
                "secure random token generation failed"
            );
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                codes::INTERNAL_ERROR,
                "internal error",
                &request_id,
            )
        }
        Err(home::IssueGrantError::Store(e)) => {
            store_error_response("home.grant.issue", &e, &request_id)
        }
    }
}

/// GET /v1/home/pairing-grants：认证 Human 列出本人未消费 Grant
/// （含已撤销与派生过期；不含已消费；绝不包含 token/hash）。
async fn list_grants(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    let now = UtcMillis::now();
    match home::list_grants(state.home.as_ref(), &auth.participant.id).await {
        Ok(grants) => Json(GrantListResponse {
            grants: grants
                .into_iter()
                .map(|grant| {
                    let status = grant.status_at(now);
                    GrantView {
                        grant_id: grant.grant_id.as_str().to_owned(),
                        status: status.as_str().to_owned(),
                        grants: grant.kind.as_str().to_owned(),
                        device_label: grant.device_label.as_ref().map(|n| n.as_str().to_owned()),
                        issued_at: grant.issued_at.to_rfc3339(),
                        expires_at: grant.expires_at.to_rfc3339(),
                        consumed_at: grant.consumed_at.map(|t| t.to_rfc3339()),
                        revoked_at: grant.revoked_at.map(|t| t.to_rfc3339()),
                    }
                })
                .collect(),
            request_id,
        })
        .into_response(),
        Err(home::ListGrantsError::Store(e)) => {
            store_error_response("home.grant.list", &e, &request_id)
        }
    }
}

/// DELETE /v1/home/pairing-grants/{grant_id}：认证 Human 撤销本人未使用
/// Grant；幂等（204）；未知 / 非本人 / 已消费统一 404 `grant_not_found`
/// （bad-id 与不存在同一错误面，不泄露存在性）。
async fn revoke_grant(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
    Path(grant_id): Path<String>,
) -> Response {
    let request_id = request_id.0;
    let Some(grant_id) = GrantId::parse_stable(&grant_id) else {
        return error_response(
            StatusCode::NOT_FOUND,
            codes::GRANT_NOT_FOUND,
            "grant not found",
            &request_id,
        );
    };
    match home::revoke_grant(state.home.as_ref(), &auth.participant.id, &grant_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(home::RevokeGrantError::NotFound) => error_response(
            StatusCode::NOT_FOUND,
            codes::GRANT_NOT_FOUND,
            "grant not found",
            &request_id,
        ),
        Err(home::RevokeGrantError::Store(e)) => {
            store_error_response("home.grant.revoke", &e, &request_id)
        }
    }
}

/// POST /v1/home/pairing-grants/exchange 请求体：可选 device_label 与
/// display_name（未知字段拒绝）。
///
/// - `self_device`（设备配对）：只认 device_label；带 display_name 会被拒
///   （400，**不消费 Grant**）；
/// - `new_human`（邀请）：**必须**带 display_name——**被邀请者自己命名
///   自己**，不由签发者代填；缺名字 400 且**不消费 Grant**（补上即可重试）。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExchangeBody {
    #[serde(default)]
    device_label: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
}

/// POST /v1/home/pairing-grants/exchange：**匿名兑换**，只接受 Grant
/// Bearer（Human Session token 的 `clr_hs_` 前缀格式不合格 → 统一
/// `grant_invalid`，绝不以 Human Session 认证本端点）。缺凭据 → 401
/// `authentication_required`；其余一切不可兑换形态（格式错、未知、已用、
/// 过期、撤销）统一 401 `grant_invalid`。成功响应携带 raw Human Session
/// token（一次）、session 非秘密 id 与 Home Identity。
async fn exchange_grant(
    State(state): State<AppState>,
    request_id: RequestId,
    headers: axum::http::HeaderMap,
    body: Result<Json<ExchangeBody>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    let candidate = match parse_authorization(&headers) {
        BearerParse::Ok(token) => token,
        BearerParse::Missing => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                codes::AUTHENTICATION_REQUIRED,
                "missing Authorization: Bearer header",
                &request_id,
            );
        }
        BearerParse::Invalid => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                codes::GRANT_INVALID,
                "invalid pairing grant",
                &request_id,
            );
        }
    };
    let body = match parse_body(body, &request_id) {
        Ok(payload) => payload,
        Err(response) => return *response,
    };
    let device_label = body.device_label.as_deref();
    let display_name = body.display_name.as_deref();
    match home::exchange_grant(state.home.as_ref(), &candidate, device_label, display_name).await {
        Ok(result) => {
            // 邀请进来的新人，**自动收到一条来自邀请者的待答关系申请**：
            // 一进来就看到"某某想加你"，自己决定接受/拒绝——**事后同意**，
            // 不是"预先同意"（服务层退役时正是这条理由）。
            //
            // 它是**best-effort**：邀请本身（Human + Session）已经成立，
            // 关系申请失败只记录脱敏 warn，绝不翻转已经发生的兑换。
            if result.outcome.invited_new_human {
                let key = crate::domain::agent::IdempotencyKey::parse(&format!(
                    "invite-{}",
                    result.outcome.grant_id.as_str()
                ));
                match key {
                    Ok(key) => {
                        if let Err(_e) = state
                            .contact
                            .send_contact_request(
                                &result.outcome.issued_by,
                                &result.outcome.human_id,
                                &key,
                            )
                            .await
                        {
                            tracing::warn!(
                                operation = "home.invite.contact_request",
                                request_id = %request_id,
                                error_kind = "contact_store_refused",
                                "invitee joined but the courtesy contact request failed; \
                                 the invite itself is unaffected"
                            );
                        }
                    }
                    Err(_) => tracing::warn!(
                        operation = "home.invite.contact_request",
                        request_id = %request_id,
                        error_kind = "invalid_derived_key",
                        "invitee joined but the courtesy contact request key was invalid"
                    ),
                }
            }
            let outcome = &result.outcome;
            let home_identity = &outcome.home;
            (
                StatusCode::CREATED,
                Json(ExchangeResponse {
                    session: ExchangeSessionView {
                        id: outcome.session_id.as_str().to_owned(),
                        human_id: outcome.human_id.as_str().to_owned(),
                        created_at: outcome.session_created_at.to_rfc3339(),
                    },
                    token: result.session_token.as_str().to_owned(),
                    home: ExchangeHomeView {
                        home_id: home_identity.home_id.as_str().to_owned(),
                        display_name: home_identity
                            .display_name
                            .as_ref()
                            .map(|n| n.as_str().to_owned()),
                        created_at: home_identity.created_at.to_rfc3339(),
                    },
                    request_id,
                }),
            )
                .into_response()
        }
        Err(home::ExchangeError::InvalidBody(_)) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "invalid device_label",
            &request_id,
        ),
        Err(home::ExchangeError::GrantInvalid) => error_response(
            StatusCode::UNAUTHORIZED,
            codes::GRANT_INVALID,
            "invalid pairing grant",
            &request_id,
        ),
        Err(home::ExchangeError::Token(_)) => {
            tracing::error!(
                operation = "home.grant.exchange",
                error_kind = "secure_random_unavailable",
                request_id = %request_id,
                "secure random session token generation failed"
            );
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                codes::INTERNAL_ERROR,
                "internal error",
                &request_id,
            )
        }
        Err(home::ExchangeError::Store(e)) => {
            store_error_response("home.grant.exchange", &e, &request_id)
        }
    }
}

/// GET /v1/sessions：认证 Human 列出自己的全部 Session（含已撤销）的
/// 非秘密元数据与可选 Session Metadata；绝不返回 token/hash。
async fn list_sessions(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    match home::list_sessions(state.home.as_ref(), &auth.participant.id).await {
        Ok(sessions) => Json(SessionListResponse {
            sessions: sessions
                .into_iter()
                .map(|s| SessionView {
                    id: s.session_id.as_str().to_owned(),
                    created_at: s.created_at.to_rfc3339(),
                    revoked_at: s.revoked_at.map(|t| t.to_rfc3339()),
                    device_label: s.device_label.as_ref().map(|n| n.as_str().to_owned()),
                    source_grant_id: s.source_grant_id.as_ref().map(|g| g.as_str().to_owned()),
                })
                .collect(),
            request_id,
        })
        .into_response(),
        Err(home::ListSessionsError::Store(e)) => {
            store_error_response("home.session.list", &e, &request_id)
        }
    }
}

/// DELETE /v1/sessions/{session_id}：认证 Human 撤销自己的 Session；
/// 幂等（204）；未知 / 非本人统一 404 `session_not_found`；独立撤销
/// 不影响同 Human 其他 Session（既有 DELETE /v1/sessions/current 语义
/// 不变）。bad-id 与不存在统一 404，不泄露存在性。
async fn revoke_session(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
    Path(session_id): Path<String>,
) -> Response {
    let request_id = request_id.0;
    let Some(session_id) = SessionId::parse_stable(&session_id) else {
        return error_response(
            StatusCode::NOT_FOUND,
            codes::SESSION_NOT_FOUND,
            "session not found",
            &request_id,
        );
    };
    match home::revoke_owned_session(state.home.as_ref(), &auth.participant.id, &session_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(home::RevokeOwnedSessionError::NotFound) => error_response(
            StatusCode::NOT_FOUND,
            codes::SESSION_NOT_FOUND,
            "session not found",
            &request_id,
        ),
        Err(home::RevokeOwnedSessionError::Store(e)) => {
            store_error_response("home.session.revoke", &e, &request_id)
        }
    }
}

// ===== A2 Pairing Transaction（HOME_ACCESS.md §4.3 A2）=====

/// POST /v1/home/pairing-transactions/{id}/commit 请求体（声明于 protocol）。
type CommitBody = CommitTransactionBody;

/// 事务视图成功响应：把领域视图 + Home Identity + issued_by 非秘密身份
/// 组装为公开 `TransactionResponse`（绝不包含 token / token hash）。
fn view_response(
    view: TransactionView,
    home: crate::domain::home::Home,
    request_id: &str,
) -> Json<TransactionResponse> {
    Json(transaction_response(
        &view,
        &home,
        None,
        request_id.to_owned(),
    ))
}

/// POST /v1/home/pairing-grants/prepare：匿名端点，只接受 Grant Bearer +
/// Host 生成的固定长度摘要。校验通过后创建或幂等重用 `pairing_transaction`，
/// **不创建正式 Human Session、不消费 Grant**；返回 transaction_id /
/// Home Identity / issued_by 非秘密身份 / reserved session id / expires_at。
/// 原始 Session / commit token 从不进入 Core 响应。同一 client_nonce +
/// 摘要重试返回同一准备事务。
async fn prepare_transaction(
    State(state): State<AppState>,
    request_id: RequestId,
    headers: axum::http::HeaderMap,
    body: Result<Json<PrepareTransactionRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    let grant = match parse_authorization(&headers) {
        BearerParse::Ok(token) => token,
        BearerParse::Missing => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                codes::AUTHENTICATION_REQUIRED,
                "missing Authorization: Bearer header",
                &request_id,
            );
        }
        BearerParse::Invalid => {
            return error_response(
                StatusCode::UNAUTHORIZED,
                codes::GRANT_INVALID,
                "invalid pairing grant",
                &request_id,
            );
        }
    };
    let body = match parse_body(body, &request_id) {
        Ok(payload) => payload,
        Err(response) => return *response,
    };
    match home::prepare_transaction(
        state.home.as_ref(),
        &grant,
        &body.client_nonce,
        &body.session_token_hash,
        &body.commit_token_hash,
    )
    .await
    {
        Ok(view) => {
            let home_identity = match home::get_home(state.home.as_ref()).await {
                Ok(Some(h)) => h,
                Ok(None) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        codes::INTERNAL_ERROR,
                        "internal error",
                        &request_id,
                    );
                }
                Err(home::HomeError::Store(e)) => {
                    return store_error_response("home.get", &e, &request_id);
                }
            };
            (
                StatusCode::CREATED,
                view_response(view, home_identity, &request_id),
            )
                .into_response()
        }
        Err(home::PrepareTransactionError::InvalidBody(_)) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "invalid prepare request",
            &request_id,
        ),
        Err(home::PrepareTransactionError::GrantInvalid) => error_response(
            StatusCode::UNAUTHORIZED,
            codes::GRANT_INVALID,
            "invalid pairing grant",
            &request_id,
        ),
        Err(home::PrepareTransactionError::TransactionInvalid) => error_response(
            StatusCode::BAD_REQUEST,
            codes::PAIRING_TRANSACTION_INVALID,
            "invalid pairing transaction",
            &request_id,
        ),
        Err(home::PrepareTransactionError::Store(e)) => {
            store_error_response("home.transaction.prepare", &e, &request_id)
        }
    }
}

/// GET /v1/home/pairing-transactions/{id}：凭 commit token Bearer 查询
/// 非秘密状态（prepared/committed/expired/aborted）。未知、错误 token、
/// 其他 Human 或过期事务统一 `pairing_transaction_invalid`，不提供
/// 存在性探测；仅供 Host 恢复，不给浏览器直连。
async fn get_transaction_status(
    State(state): State<AppState>,
    request_id: RequestId,
    headers: axum::http::HeaderMap,
    Path(transaction_id): Path<String>,
) -> Response {
    let request_id = request_id.0;
    let transaction_id = match TransactionId::parse_stable(&transaction_id) {
        Some(id) => id,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::PAIRING_TRANSACTION_INVALID,
                "invalid pairing transaction",
                &request_id,
            );
        }
    };
    let commit_token = match parse_bearer_commit_token(&headers, &request_id) {
        Ok(token) => token,
        Err(response) => return response,
    };
    let Ok(hash) = home::commit_token_hash_of(&commit_token) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            codes::PAIRING_TRANSACTION_INVALID,
            "invalid pairing transaction",
            &request_id,
        );
    };
    match home::get_transaction_status(state.home.as_ref(), &transaction_id, &hash).await {
        Ok(Some(view)) => {
            let home_identity = match home::get_home(state.home.as_ref()).await {
                Ok(Some(h)) => h,
                Ok(None) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        codes::INTERNAL_ERROR,
                        "internal error",
                        &request_id,
                    );
                }
                Err(home::HomeError::Store(e)) => {
                    return store_error_response("home.get", &e, &request_id);
                }
            };
            view_response(view, home_identity, &request_id).into_response()
        }
        Ok(None) => error_response(
            StatusCode::BAD_REQUEST,
            codes::PAIRING_TRANSACTION_INVALID,
            "invalid pairing transaction",
            &request_id,
        ),
        Err(home::TransactionStatusError::NotFound) => error_response(
            StatusCode::BAD_REQUEST,
            codes::PAIRING_TRANSACTION_INVALID,
            "invalid pairing transaction",
            &request_id,
        ),
        Err(home::TransactionStatusError::Store(e)) => {
            store_error_response("home.transaction.status", &e, &request_id)
        }
    }
}

/// POST /v1/home/pairing-transactions/{id}/commit：凭 commit token Bearer；
/// Core 在一个事务内完成「以 prepare 阶段提交的 session_token_hash 创建
/// 正式 Human Session + 写入 session_metadata + 标记 pairing_transaction
/// committed + 消费 Pairing Grant」。首次成功与重复提交返回同一非秘密
/// 结果，绝不产生第二 Session。
/// commit body 只有可选 device_label：真正空 body 或 `{}` 允许， malformed JSON
/// 与未知字段必须诚实返回 `invalid_request`，不能被当成恢复空请求。
fn parse_commit_body(
    body: Result<Bytes, BytesRejection>,
    request_id: &str,
) -> Result<Option<String>, Box<Response>> {
    let bytes = match body {
        Ok(bytes) => bytes,
        Err(BytesRejection::FailedToBufferBody(FailedToBufferBody::LengthLimitError(_))) => {
            return Err(Box::new(error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                codes::PAYLOAD_TOO_LARGE,
                "request body too large",
                request_id,
            )));
        }
        Err(_) => {
            return Err(Box::new(error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "invalid request body",
                request_id,
            )));
        }
    };
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    serde_json::from_slice::<CommitBody>(&bytes)
        .map(|body| body.device_label)
        .map_err(|_| {
            Box::new(error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "invalid JSON request body",
                request_id,
            ))
        })
}

async fn commit_transaction(
    State(state): State<AppState>,
    request_id: RequestId,
    headers: axum::http::HeaderMap,
    Path(transaction_id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let request_id = request_id.0;
    let transaction_id = match TransactionId::parse_stable(&transaction_id) {
        Some(id) => id,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::PAIRING_TRANSACTION_INVALID,
                "invalid pairing transaction",
                &request_id,
            );
        }
    };
    let commit_token = match parse_bearer_commit_token(&headers, &request_id) {
        Ok(token) => token,
        Err(response) => return response,
    };
    let device_label = match parse_commit_body(body, &request_id) {
        Ok(device_label) => device_label,
        Err(response) => return *response,
    };
    match home::commit_transaction(
        state.home.as_ref(),
        &transaction_id,
        &commit_token,
        device_label.as_deref(),
    )
    .await
    {
        Ok(view) => {
            let home_identity = match home::get_home(state.home.as_ref()).await {
                Ok(Some(h)) => h,
                Ok(None) => {
                    return error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        codes::INTERNAL_ERROR,
                        "internal error",
                        &request_id,
                    );
                }
                Err(home::HomeError::Store(e)) => {
                    return store_error_response("home.get", &e, &request_id);
                }
            };
            (
                StatusCode::OK,
                view_response(view, home_identity, &request_id),
            )
                .into_response()
        }
        Err(home::CommitTransactionError::InvalidBody(_)) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "invalid device_label",
            &request_id,
        ),
        Err(home::CommitTransactionError::NotFound) => error_response(
            StatusCode::BAD_REQUEST,
            codes::PAIRING_TRANSACTION_INVALID,
            "invalid pairing transaction",
            &request_id,
        ),
        Err(home::CommitTransactionError::Store(e)) => {
            store_error_response("home.transaction.commit", &e, &request_id)
        }
    }
}

/// DELETE /v1/home/pairing-transactions/{id}：凭 commit token Bearer 主动
/// 放弃 prepared 事务；幂等；不影响 Grant 的 active 状态。已 committed /
/// 已 aborted / 过期一律 `pairing_transaction_invalid`。
async fn abort_transaction(
    State(state): State<AppState>,
    request_id: RequestId,
    headers: axum::http::HeaderMap,
    Path(transaction_id): Path<String>,
) -> Response {
    let request_id = request_id.0;
    let transaction_id = match TransactionId::parse_stable(&transaction_id) {
        Some(id) => id,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::PAIRING_TRANSACTION_INVALID,
                "invalid pairing transaction",
                &request_id,
            );
        }
    };
    let commit_token = match parse_bearer_commit_token(&headers, &request_id) {
        Ok(token) => token,
        Err(response) => return response,
    };
    match home::abort_transaction(state.home.as_ref(), &transaction_id, &commit_token).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(home::AbortTransactionError::NotFound) => error_response(
            StatusCode::BAD_REQUEST,
            codes::PAIRING_TRANSACTION_INVALID,
            "invalid pairing transaction",
            &request_id,
        ),
        Err(home::AbortTransactionError::Store(e)) => {
            store_error_response("home.transaction.abort", &e, &request_id)
        }
    }
}

/// A2 端点统一的 commit token Bearer 解析：缺凭据 → 401
/// `authentication_required`；格式非法 → `pairing_transaction_invalid`
/// （对外统一，不泄露存在性）。原始 token 只用于计算 SHA-256 摘要。
/// 错误响应复用调用方（middleware 生成的 canonical request_id）。
#[allow(clippy::result_large_err)]
fn parse_bearer_commit_token(
    headers: &axum::http::HeaderMap,
    request_id: &str,
) -> Result<String, Response> {
    match parse_authorization(headers) {
        BearerParse::Ok(token) => Ok(token),
        BearerParse::Missing => Err(error_response(
            StatusCode::UNAUTHORIZED,
            codes::AUTHENTICATION_REQUIRED,
            "missing Authorization: Bearer header",
            request_id,
        )),
        BearerParse::Invalid => Err(error_response(
            StatusCode::BAD_REQUEST,
            codes::PAIRING_TRANSACTION_INVALID,
            "invalid pairing transaction",
            request_id,
        )),
    }
}
