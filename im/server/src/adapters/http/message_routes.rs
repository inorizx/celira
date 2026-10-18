//! Message HTTP handlers（契约 CORE_V1_MESSAGES.md 8.x +
//! CORE_V1_MESSAGE_RECALL.md §12）。
//!
//! handler 只调用 `application::message` 用例与端口，不直接操作 SQLite；
//! 所有非 2xx 走统一 JSON 错误信封，body 的 error.request_id 与
//! x-request-id 一致。Message 路由独立接受最多 128 KiB JSON，不沿用
//! Agent/Direct Room 的 4 KiB 上限；超出返回 413 payload_too_large。
//! 正文的 16384 UTF-8 bytes 边界在应用与持久层裁决，不属于路由上限。
//!
//! 撤回路由（两个入口只做认证适配、共用同一 application recall 用例与
//! 同一 MessageStore 事务）：Human 入口 `DELETE /v1/rooms/{room_id}/
//! messages/{message_id}` + CurrentAuth；Agent 入口 `DELETE /v1/backend/
//! rooms/{room_id}/messages/{message_id}` + BackendAuth。请求必须无 body、
//! 无 query、无 Idempotency-Key——非空 body、任意 query 或出现
//! Idempotency-Key 一律 400 invalid_request（不能静默忽略）；撤回路由使用
//! 小且独立的 body limit，不放宽其他路由。

use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, FailedToBufferBody, JsonRejection, QueryRejection};
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};

use crate::adapters::http::AppState;
use crate::adapters::http::agent_routes::{KeyError, idempotency_key_from_headers};
use crate::adapters::http::auth::CurrentAuth;
use crate::adapters::http::backend_auth::BackendAuth;
use crate::adapters::http::backend_stream_routes::wake_agents;
use crate::adapters::http::human_event_routes::wake_humans;
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::routes::{error_response, store_error_response};
use crate::application::message::{
    MessageQueryError, RecallError, RecallOutcome, SendMessageError, list_messages, recall_message,
    send_message,
};
use crate::domain::agent_event::AgentEventKind;
use crate::domain::human_event::{HumanEventKind, HumanEventSource};
use crate::domain::message::Message;
use crate::domain::participant::{DisplayName, Participant, ParticipantKind, ParticipantStatus};
use crate::ports::message::HistoryMessage;
use crate::protocol::error::codes;
use crate::protocol::message::{
    HistoryQuery, ListMessagesResponse, MessageActorView, MessageView, RecallMessageResponse,
    SendMessageRequest, SendMessageResponse,
};

/// Message 路由请求体上限：128 KiB，容纳合法正文的 JSON 转义表达。
pub const MAX_MESSAGE_ROUTE_BODY_BYTES: usize = 128 * 1024;

/// 撤回路由的独立小 body limit：撤回请求必须无 body，任何非空 body 一律
/// 400 invalid_request（不接受、不忽略）；小上限只为路由形状约束服务，
/// 不放宽 Message/Completion 的 128 KiB 上限。
pub const MAX_RECALL_BODY_BYTES: usize = 1024;

/// Message 切片路由。路由级上限按入口分开：撤回路由（必须无 body）在前、
/// 带小 body limit；发送/History 路由在后、带 128 KiB 上限。
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/rooms/{room_id}/messages/{message_id}",
            delete(recall_message_route),
        )
        .route(
            "/v1/backend/rooms/{room_id}/messages/{message_id}",
            delete(recall_backend_message_route),
        )
        .layer(axum::extract::DefaultBodyLimit::max(MAX_RECALL_BODY_BYTES))
        .route(
            "/v1/rooms/{room_id}/messages",
            get(list_messages_route).post(send_message_route),
        )
        .route(
            "/v1/backend/rooms/{room_id}/messages",
            get(list_backend_messages_route).post(send_backend_message_route),
        )
        .layer(axum::extract::DefaultBodyLimit::max(
            MAX_MESSAGE_ROUTE_BODY_BYTES,
        ))
}

/// 撤回请求形状守卫（契约 §12.1）：请求必须无 body、无 query、无
/// Idempotency-Key；非空 body（含超过小 body limit 的 body）、任意 query
/// 或出现 Idempotency-Key 一律 400 invalid_request，不得被接受或静默忽略。
fn recall_shape_error(
    body: Result<Bytes, BytesRejection>,
    query: Option<&str>,
    request_id: &str,
) -> Result<(), Box<Response>> {
    let body_non_empty = match &body {
        Ok(bytes) => !bytes.is_empty(),
        // 超过小 body limit 同样是「非空 body」：统一 400，不泄露具体大小。
        Err(_) => true,
    };
    if body_non_empty || query.is_some() {
        return Err(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "recall request must have no body and no query parameters",
            request_id,
        )));
    }
    Ok(())
}

/// 检查 Idempotency-Key 头是否出现（任何出现都拒绝，不校验格式）。
fn recall_idempotency_key_present(headers: &HeaderMap) -> bool {
    headers.contains_key("idempotency-key")
}

/// 成功响应（首次 201 / 幂等重放 200 共用同一响应形状，以
/// `idempotency_replayed` 区分）。
fn recall_response(
    outcome: &RecallOutcome,
    message_id: &str,
    room_id: &str,
    request_id: String,
) -> Response {
    let status = if outcome.idempotency_replayed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    (
        status,
        Json(RecallMessageResponse {
            message_id: message_id.to_owned(),
            room_id: room_id.to_owned(),
            recalled_at: outcome.recalled_at.to_rfc3339(),
            idempotency_replayed: outcome.idempotency_replayed,
            request_id,
        }),
    )
        .into_response()
}

/// 撤回错误映射（契约 §12.2）：bad message UUID 400 invalid_request；bad
/// room / 无 active membership 404 room_not_found（先于消息裁决）；
/// unknown / cross-room / not-author / 对 actor 不可见统一 404
/// message_not_found；首次超窗 409 message_recall_window_expired；存储按
/// 既有安全错误边界。
fn recall_error_response(e: RecallError, request_id: &str) -> Response {
    match e {
        RecallError::InvalidRequest => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "message_id must be a canonical UUID v4",
            request_id,
        ),
        RecallError::RoomNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            request_id,
        ),
        RecallError::MessageNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::MESSAGE_NOT_FOUND,
            "message not found",
            request_id,
        ),
        RecallError::WindowExpired => error_response(
            StatusCode::CONFLICT,
            codes::MESSAGE_RECALL_WINDOW_EXPIRED,
            "message recall window expired",
            request_id,
        ),
        RecallError::Store(e) => store_error_response("message.recall", &e, request_id),
    }
}

/// 撤回公共逻辑（Human 与 Backend 两个入口共用）：认证上下文解析出 actor
/// 后调用同一 application recall 用例；post-commit 只在首次撤回成功后按
/// (message.recalled, message_id) 反查已提交 delivery 唤醒 Human（重放不
/// 唤醒、失败不反转提交）；另外（首次 201 与幂等 200 都执行，便于 repair）
/// 经共享 `notify_live` 即时清理依赖该 source Message 的临时 Activity 并
/// 定向唤醒原 Room 当前 Human——撤回是已提交公共事实，临时草稿立即失效；
/// 清理/唤醒失败只记录脱敏 warn，绝不翻转撤回响应（TTL 最终兜底）。
async fn recall_common(
    state: &AppState,
    actor_id: &crate::domain::participant::ParticipantId,
    room_id: &str,
    message_id: &str,
    request_id: &str,
) -> Response {
    let outcome = match recall_message(state.message.as_ref(), actor_id, room_id, message_id).await
    {
        Ok(outcome) => outcome,
        Err(e) => return recall_error_response(e, request_id),
    };
    let idempotency_replayed = outcome.idempotency_replayed;
    let response = recall_response(&outcome, message_id, room_id, request_id.to_owned());
    if !idempotency_replayed {
        wake_humans(
            state,
            &[HumanEventSource::new(
                HumanEventKind::MessageRecalled,
                message_id.to_owned(),
            )],
            request_id,
        )
        .await;
        // Backend SSE 门铃（窄解冻切片 §8）：message.recalled Event 的
        // target Agent 同样定向唤醒（撤回是已提交公共事实）。
        wake_agents(state, AgentEventKind::Recalled, message_id, request_id).await;
    }
    // 主体进行态：撤回者如果正在这间房写，撤回动作本身取代那条进行态。
    // ⚠ 2026-09-28：原先这里还有一段「清理挂在 source Message 上的临时
    // Activity 并唤醒 Live WS」——Activity 与 Live WS 已随旧 Live State
    // 整体删除（执行过程不进 IM），只剩这一条与主体有关的收尾。
    if let Some(room_id) = crate::domain::room::RoomId::parse_canonical(room_id) {
        crate::application::live::clear_own_typing(state.live.as_ref(), &room_id, actor_id);
    }
    response
}

/// DELETE /v1/rooms/{room_id}/messages/{message_id}（Human Session）：
/// 作者本人 + 120 秒窗口内一次性撤回；首次 201、重复同一撤回 200 幂等
/// 重放。请求必须无 body、无 query、无 Idempotency-Key。
async fn recall_message_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path((room_id, message_id)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
    request_id: RequestId,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let request_id = request_id.0;
    if recall_idempotency_key_present(&headers) {
        return error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "recall request must not carry an Idempotency-Key header",
            &request_id,
        );
    }
    if let Err(response) = recall_shape_error(body, raw_query.as_deref(), &request_id) {
        return *response;
    }
    recall_common(
        &state,
        &auth.participant.id,
        &room_id,
        &message_id,
        &request_id,
    )
    .await
}

/// DELETE /v1/backend/rooms/{room_id}/messages/{message_id}（Backend
/// Credential）：Agent 本人撤回自己的 Message（如 Completion 写回的 reply
/// Message）；同一 participant-actor recall 用例与同一 MessageStore 事务，
/// 不存在第二套撤回业务逻辑。请求约束与错误语义与 Human 入口一致。
async fn recall_backend_message_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path((room_id, message_id)): Path<(String, String)>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
    request_id: RequestId,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let request_id = request_id.0;
    if recall_idempotency_key_present(&headers) {
        return error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "recall request must not carry an Idempotency-Key header",
            &request_id,
        );
    }
    if let Err(response) = recall_shape_error(body, raw_query.as_deref(), &request_id) {
        return *response;
    }
    recall_common(
        &state,
        &auth.view.agent.id,
        &room_id,
        &message_id,
        &request_id,
    )
    .await
}

/// POST /v1/rooms/{room_id}/messages：提交普通文本 Message（可选
/// reply_to_message_id 表达回复/引用，契约 CORE_V1_REPLIES_AND_QUOTES.md；
/// 可选有序 attachments 表达附件绑定，契约 CORE_V1_ATTACHMENTS.md 5.4——
/// body 与 attachments 至少一个，附件-only Message 的 body 为 null）。
/// 首次创建 201；同 actor + 同 key + 同 Room + 相同 body/mentions/reply
/// target/有序附件集合重放 200；同 key 但任一字段不同 409。bad-id /
/// unknown / 非成员 / 非 active Membership 统一 404 room_not_found；reply
/// target 格式非法 400 invalid_request，目标未知/跨 Room/不可见 400
/// invalid_reply_target；附件超上限/重复 400 invalid_attachment，附件
/// unknown/跨 Room/跨 actor/非 pending/过期统一 404 attachment_not_found。
async fn send_message_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<SendMessageRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;

    let Json(payload) = match body {
        Ok(body) => body,
        Err(JsonRejection::BytesRejection(BytesRejection::FailedToBufferBody(
            FailedToBufferBody::LengthLimitError(_),
        ))) => {
            return error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                codes::PAYLOAD_TOO_LARGE,
                format!("request body exceeds {MAX_MESSAGE_ROUTE_BODY_BYTES} bytes"),
                &request_id,
            );
        }
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "invalid JSON request body",
                &request_id,
            );
        }
    };

    let key = match idempotency_key_from_headers(&headers) {
        Ok(key) => key,
        Err(KeyError::Missing) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::IDEMPOTENCY_KEY_REQUIRED,
                "missing Idempotency-Key header",
                &request_id,
            );
        }
        Err(KeyError::Invalid) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_IDEMPOTENCY_KEY,
                "invalid Idempotency-Key header",
                &request_id,
            );
        }
    };

    match send_message(
        state.message.as_ref(),
        &auth.participant,
        &room_id,
        payload.body.as_deref(),
        &payload.mentions,
        payload.reply_to_message_id.as_deref(),
        &payload.attachments,
        &key,
    )
    .await
    {
        Ok(crate::application::message::SendMessageOutcome::Created { message }) => {
            // post-commit 唤醒：从已提交 delivery/fact 事实反查目标 Human，
            // best-effort，失败只记录脱敏 warn，不影响已提交响应。
            wake_humans(
                &state,
                &[HumanEventSource::new(
                    HumanEventKind::MessageCreated,
                    message.id.as_str().to_owned(),
                )],
                &request_id,
            )
            .await;
            // Backend SSE 门铃（窄解冻切片 §8）：按已提交 agent_events 事实
            // 反查 target Agent 定向唤醒，同一 best-effort 纪律。
            wake_agents(
                &state,
                AgentEventKind::Created,
                message.id.as_str(),
                &request_id,
            )
            .await;
            // 主体进行态：这条消息**取代**说话人自己在这间房的「正在写」。
            // 易失、best-effort：房间 id 解析失败只跳过（进行态会按 TTL 自己
            // 过期），绝不影响已提交的消息。
            if let Some(room_id) = crate::domain::room::RoomId::parse_canonical(&room_id) {
                crate::application::live::clear_own_typing(
                    state.live.as_ref(),
                    &room_id,
                    &auth.participant.id,
                );
            }
            (
                StatusCode::CREATED,
                Json(SendMessageResponse {
                    message: message_view(&message, &auth.participant),
                    idempotency_replayed: false,
                    request_id,
                }),
            )
                .into_response()
        }
        Ok(crate::application::message::SendMessageOutcome::Replayed { message }) => (
            StatusCode::OK,
            Json(SendMessageResponse {
                message: message_view(&message, &auth.participant),
                idempotency_replayed: true,
                request_id,
            }),
        )
            .into_response(),
        Err(SendMessageError::InvalidIdempotencyKey) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_IDEMPOTENCY_KEY,
            "invalid Idempotency-Key",
            &request_id,
        ),
        Err(SendMessageError::InvalidMessageBody(e)) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_MESSAGE_BODY,
            e.to_string(),
            &request_id,
        ),
        Err(SendMessageError::InvalidRequest) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "invalid request body",
            &request_id,
        ),
        Err(SendMessageError::NotFound) => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            &request_id,
        ),
        Err(SendMessageError::NotAMember) => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            &request_id,
        ),
        Err(SendMessageError::InvalidMention) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_MENTION,
            "invalid mention: targets must be distinct active members of a group room",
            &request_id,
        ),
        Err(SendMessageError::InvalidReplyTarget) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REPLY_TARGET,
            "reply target must be a same-room historical message visible to the actor",
            &request_id,
        ),
        Err(SendMessageError::InvalidAttachment) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_ATTACHMENT,
            "a message can have at most 8 distinct attachments",
            &request_id,
        ),
        Err(SendMessageError::AttachmentNotFound) => error_response(
            StatusCode::NOT_FOUND,
            codes::ATTACHMENT_NOT_FOUND,
            "attachment not found",
            &request_id,
        ),
        Err(SendMessageError::IdempotencyConflict) => error_response(
            StatusCode::CONFLICT,
            codes::IDEMPOTENCY_CONFLICT,
            "idempotency key was already used with a different request",
            &request_id,
        ),
        Err(SendMessageError::Store(e)) => store_error_response("message.send", &e, &request_id),
    }
}

/// POST /v1/backend/rooms/{room_id}/messages（Backend Credential）：
/// agent 以成员身份向已属房间主动发起公共 Message（CORE_V1_AGENT_SPEAK.md，
/// 发送框解冻切片 A——房内主动说话）。认证 = BackendAuth（同四端点语义）；
/// 准入 = 该 agent 是目标 room 的 active member（Room 不存在/bad-id/非
/// active → 404 room_not_found 同人类路径；**Room 存在但非 active member
/// → 403 not_room_member**——agent 已通过凭据证明身份，成员资格是明确 ACL）。
/// 幂等持久化面与人类 message.create 完全共用（同一 application 用例 + 同一
/// MessageStore 事务：`(actor_id, idempotency_key)` 唯一约束、同摘要重放
/// 200、异摘要 409、失败不留任何事实）；请求体形状与校验（body/mentions/
/// reply/attachments）与人类路径一致。落公共 Message（author = 该 agent）后
/// post-commit 走既有人类 WS push 与 agent Event 门铃/轮询（wake_humans +
/// wake_agents），不新增路由逻辑。
async fn send_backend_message_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<SendMessageRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;

    let Json(payload) = match body {
        Ok(body) => body,
        Err(JsonRejection::BytesRejection(BytesRejection::FailedToBufferBody(
            FailedToBufferBody::LengthLimitError(_),
        ))) => {
            return error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                codes::PAYLOAD_TOO_LARGE,
                format!("request body exceeds {MAX_MESSAGE_ROUTE_BODY_BYTES} bytes"),
                &request_id,
            );
        }
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "invalid JSON request body",
                &request_id,
            );
        }
    };

    let key = match idempotency_key_from_headers(&headers) {
        Ok(key) => key,
        Err(KeyError::Missing) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::IDEMPOTENCY_KEY_REQUIRED,
                "missing Idempotency-Key header",
                &request_id,
            );
        }
        Err(KeyError::Invalid) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_IDEMPOTENCY_KEY,
                "invalid Idempotency-Key header",
                &request_id,
            );
        }
    };

    // actor = 被代表的 Agent 的 Participant 公共事实（认证已保证 kind=agent、
    // status=active；display_name 在创建时已过 DisplayName 校验，此处按
    // 恒真不变量构造——绝不可能走到 parse 失败分支）。
    let view = &auth.view.agent;
    let actor = Participant {
        id: view.id.clone(),
        kind: ParticipantKind::Agent,
        display_name: DisplayName::parse(&view.display_name)
            .expect("authenticated agent display_name was validated at creation (invariant)"),
        status: ParticipantStatus::Active,
        created_at: view.created_at,
    };
    let actor_view = || MessageActorView {
        id: view.id.as_str().to_owned(),
        kind: "agent".to_owned(),
        display_name: view.display_name.clone(),
        status: view.status.clone(),
    };

    match send_message(
        state.message.as_ref(),
        &actor,
        &room_id,
        payload.body.as_deref(),
        &payload.mentions,
        payload.reply_to_message_id.as_deref(),
        &payload.attachments,
        &key,
    )
    .await
    {
        Ok(crate::application::message::SendMessageOutcome::Created { message }) => {
            // post-commit 唤醒：与人类路径同一 best-effort 纪律（从已提交
            // delivery/fact 反查目标，失败只记录脱敏 warn，不影响已提交响应）。
            wake_humans(
                &state,
                &[HumanEventSource::new(
                    HumanEventKind::MessageCreated,
                    message.id.as_str().to_owned(),
                )],
                &request_id,
            )
            .await;
            wake_agents(
                &state,
                AgentEventKind::Created,
                message.id.as_str(),
                &request_id,
            )
            .await;
            // 主体进行态：这条消息**取代**说话人自己在这间房的「正在写」。
            // 易失、best-effort：房间 id 解析失败只跳过（进行态会按 TTL 自己
            // 过期），绝不影响已提交的消息。
            if let Some(room_id) = crate::domain::room::RoomId::parse_canonical(&room_id) {
                crate::application::live::clear_own_typing(
                    state.live.as_ref(),
                    &room_id,
                    &actor.id,
                );
            }
            (
                StatusCode::CREATED,
                Json(SendMessageResponse {
                    message: project_message(&message, actor_view()),
                    idempotency_replayed: false,
                    request_id,
                }),
            )
                .into_response()
        }
        Ok(crate::application::message::SendMessageOutcome::Replayed { message }) => (
            StatusCode::OK,
            Json(SendMessageResponse {
                message: project_message(&message, actor_view()),
                idempotency_replayed: true,
                request_id,
            }),
        )
            .into_response(),
        Err(SendMessageError::InvalidIdempotencyKey) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_IDEMPOTENCY_KEY,
            "invalid Idempotency-Key",
            &request_id,
        ),
        Err(SendMessageError::InvalidMessageBody(e)) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_MESSAGE_BODY,
            e.to_string(),
            &request_id,
        ),
        Err(SendMessageError::InvalidRequest) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "invalid request body",
            &request_id,
        ),
        Err(SendMessageError::NotFound) => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            &request_id,
        ),
        Err(SendMessageError::NotAMember) => error_response(
            StatusCode::FORBIDDEN,
            codes::NOT_ROOM_MEMBER,
            "agent is not an active member of the room",
            &request_id,
        ),
        Err(SendMessageError::InvalidMention) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_MENTION,
            "invalid mention: targets must be distinct active members of a group room",
            &request_id,
        ),
        Err(SendMessageError::InvalidReplyTarget) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REPLY_TARGET,
            "reply target must be a same-room historical message visible to the actor",
            &request_id,
        ),
        Err(SendMessageError::InvalidAttachment) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_ATTACHMENT,
            "a message can have at most 8 distinct attachments",
            &request_id,
        ),
        Err(SendMessageError::AttachmentNotFound) => error_response(
            StatusCode::NOT_FOUND,
            codes::ATTACHMENT_NOT_FOUND,
            "attachment not found",
            &request_id,
        ),
        Err(SendMessageError::IdempotencyConflict) => error_response(
            StatusCode::CONFLICT,
            codes::IDEMPOTENCY_CONFLICT,
            "idempotency key was already used with a different request",
            &request_id,
        ),
        Err(SendMessageError::Store(e)) => store_error_response("message.send", &e, &request_id),
    }
}

/// GET /v1/rooms/{room_id}/messages：读取有界历史。初次不传 before 返回
/// 最新一页；messages 按 sequence 升序；有更旧消息时 next_before 为当前
/// 页第一条 Message 的 opaque cursor。非法 / 重复 / 越界的 limit、非法或
/// 不可解析的 before cursor 返回 400 invalid_request；unknown / bad-id /
/// 非成员统一 404。
async fn list_messages_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(room_id): Path<String>,
    query: Result<Query<HistoryQuery>, QueryRejection>,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    let Query(query) = match query {
        Ok(query) => query,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "invalid history paging parameters",
                &request_id,
            );
        }
    };

    match list_messages(
        state.message.as_ref(),
        &auth.participant.id,
        &room_id,
        query.limit,
        query.before.as_deref(),
    )
    .await
    {
        Ok(page) => Json(ListMessagesResponse {
            messages: page.messages.iter().map(history_message_view).collect(),
            next_before: page.next_before,
            request_id,
        })
        .into_response(),
        Err(e) => query_error_response(e, &request_id),
    }
}

/// GET /v1/backend/rooms/{room_id}/messages：**agent 历史读取面（切片 B2
/// 翻记录）**。认证 = BackendAuth（同四端点语义）；准入 = 该 agent 是目标
/// room 的 active member（Room 不存在/bad-id → 404 room_not_found 同人类
/// 路径；**Room 存在但非 active member → 403 not_room_member**——对齐 say
/// 端点先例，agent 已通过凭据证明身份，成员资格是明确 ACL）。分页与消息
/// 公共投影与人类历史路径完全同构（同一 `HistoryQuery`/`ListMessagesResponse`/
/// opaque `before` 游标/`next_before`）；零写语义、零持久新增、不改变任何
/// 既有端点。
async fn list_backend_messages_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(room_id): Path<String>,
    query: Result<Query<HistoryQuery>, QueryRejection>,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    let Query(query) = match query {
        Ok(query) => query,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "invalid history paging parameters",
                &request_id,
            );
        }
    };

    match list_messages(
        state.message.as_ref(),
        &auth.view.agent.id,
        &room_id,
        query.limit,
        query.before.as_deref(),
    )
    .await
    {
        Ok(page) => Json(ListMessagesResponse {
            messages: page.messages.iter().map(history_message_view).collect(),
            next_before: page.next_before,
            request_id,
        })
        .into_response(),
        Err(e) => backend_query_error_response(e, &request_id),
    }
}

/// 后端历史查询失败映射：**Room 存在但非 active member → 403
/// not_room_member**（对齐 say 端点）；Room unknown/bad-id → 404
/// room_not_found（与人类路径同 404 语义）；分页参数非法 → 400。
fn backend_query_error_response(e: MessageQueryError, request_id: &str) -> Response {
    match e {
        MessageQueryError::NotAMember => error_response(
            StatusCode::FORBIDDEN,
            codes::NOT_ROOM_MEMBER,
            "agent is not an active member of the room",
            request_id,
        ),
        MessageQueryError::NotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            request_id,
        ),
        MessageQueryError::InvalidRequest => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "invalid history paging parameters",
            request_id,
        ),
        MessageQueryError::Store(e) => store_error_response("message.list", &e, request_id),
    }
}

/// 查询失败：unknown / bad-id / 非成员统一 404 room_not_found（Human 路径的
/// 存在性保护不变——Store 层的 NotAMember 区分在此收敛回 404，wire 与 B2 前
/// 完全一致）；分页参数非法为 400 invalid_request。
fn query_error_response(e: MessageQueryError, request_id: &str) -> Response {
    match e {
        MessageQueryError::NotFound | MessageQueryError::NotAMember => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            request_id,
        ),
        MessageQueryError::InvalidRequest => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "invalid history paging parameters",
            request_id,
        ),
        MessageQueryError::Store(e) => store_error_response("message.list", &e, request_id),
    }
}

/// 公共 Message 视图投影（发送响应 / History / Agent Event 共用同一投影，
/// 契约 CORE_V1_MESSAGE_RECALL.md §7.1 + CORE_V1_ATTACHMENTS.md 5.4/9.3）：
/// 字段集恒存在——active 时 body 为原值（附件-only Message 为 null）、
/// mentions 为原集合、attachments 为绑定附件元数据、recalled_at 为 null；
/// recalled 时 body=null、mentions=[]、attachments=[]、recalled_at=首次撤回
/// 时点（RFC3339）；id/room_id/actor/reply_to_message_id/created_at 两种
/// 状态都保留原值。原正文与原 Mention 集合绝不进入任何公共恢复面。
pub(crate) fn project_message(message: &Message, actor: MessageActorView) -> MessageView {
    let recalled = message.recalled_at.is_some();
    MessageView {
        id: message.id.as_str().to_owned(),
        room_id: message.room_id.as_str().to_owned(),
        actor,
        body: if recalled {
            None
        } else {
            message.body.as_ref().map(|body| body.as_str().to_owned())
        },
        mentions: if recalled {
            Vec::new()
        } else {
            message
                .mentions
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect()
        },
        attachments: if recalled {
            Vec::new()
        } else {
            message
                .attachments
                .iter()
                .map(|meta| crate::protocol::attachment::AttachmentMetadataView {
                    id: meta.id.as_str().to_owned(),
                    file_name: meta.file_name.as_str().to_owned(),
                    content_type: meta.content_type.as_str().to_owned(),
                    size_bytes: meta.size_bytes,
                })
                .collect()
        },
        reply_to_message_id: message
            .reply_to_message_id
            .as_ref()
            .map(|id| id.as_str().to_owned()),
        created_at: message.created_at.to_rfc3339(),
        recalled_at: message.recalled_at.map(|t| t.to_rfc3339()),
    }
}

/// 发送响应中的 Message 视图：actor 来自认证上下文的 Participant 公共
/// 资料（key 按 actor 全局作用域，重放时的 actor 恒为当前认证 Human）；
/// tombstone 投影与 History / Agent Event 共用（§7.1）。视图不返回 sequence。
fn message_view(message: &Message, actor: &Participant) -> MessageView {
    project_message(
        message,
        MessageActorView {
            id: actor.id.as_str().to_owned(),
            kind: actor.kind.as_str().to_owned(),
            display_name: actor.display_name.as_str().to_owned(),
            status: actor.status.as_str().to_owned(),
        },
    )
}

/// History 中的 Message 视图：actor 是读取时的当前 Participant 公共投影。
/// 视图不返回 sequence。
fn history_message_view(item: &HistoryMessage) -> MessageView {
    let message = &item.message;
    let actor = &item.actor;
    project_message(
        message,
        MessageActorView {
            id: actor.id.as_str().to_owned(),
            kind: actor.kind.as_str().to_owned(),
            display_name: actor.display_name.as_str().to_owned(),
            status: actor.status.as_str().to_owned(),
        },
    )
}
