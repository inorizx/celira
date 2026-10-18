//! Contact HTTP handlers（契约 CORE_V1_CONTACT_REVERSIBILITY.md +
//! 批次三第二刀「关系对主体开放」）。
//!
//! `POST /v1/contacts/requests` 使用 Idempotency-Key；公开响应不返回
//! token/hash/幂等键。接受动作只允许 addressee；unknown / 非 addressee
//! 统一 404 contact_not_found，不泄露关系是否存在。拒绝（decline）、解除
//! （DELETE /v1/contacts/{participant_id}）与拉黑（/v1/blocks/*）为纯新增端点。
//! 被拉黑拦截的申请返回与「目标不存在」同形的 404（不泄露）。
//!
//! **本切片的分叉纪律**（与第一刀同法）：五个动作的响应构造在这里是
//! `pub(crate)` 共享函数，Human 路由（`CurrentAuth`）与 Agent 路由
//! （`BackendAuth`，`backend_contact_routes.rs`）**共用同一段代码**——同一个
//! application 用例、同一套错误码、同一份投影，只在适配器换认证面与 actor 来源。
//!
//! handler 只调用 `application::contact` 用例与端口，不直接操作 SQLite；
//! 所有非 2xx 走统一 JSON 错误信封，body 的 error.request_id 与
//! x-request-id 一致。

use axum::extract::rejection::{BytesRejection, FailedToBufferBody, JsonRejection};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use crate::adapters::http::AppState;
use crate::adapters::http::agent_routes::{KeyError, idempotency_key_from_headers};
use crate::adapters::http::auth::CurrentAuth;
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::routes::{
    MAX_AGENT_CREATE_BODY_BYTES, error_response, store_error_response,
};
use crate::application::contact::{
    BlockContactError, ContactQueryError, SendContactRequestError, accept_contact,
    block_participant, decline_contact, end_contact, list_blocks, list_contacts,
    send_contact_request, unblock_participant,
};
use crate::domain::contact::Contact;
use crate::domain::participant::{Participant, ParticipantId};
use crate::ports::contact::BlockedHuman;
use crate::ports::room::ParticipantProfile;
use crate::protocol::contact::{
    AcceptContactResponse, BlockView, ContactView, CreateContactRequest, CreateContactResponse,
    ListBlocksResponse, ListContactsResponse,
};
use crate::protocol::error::codes;
use crate::protocol::room::ParticipantMinView;

/// Contact 切片路由。请求体上限与 Agent/Direct Room 一致（4 KiB）。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/contacts/requests", axum::routing::post(create_contact))
        .route("/v1/contacts", get(list_contacts_route))
        .route(
            "/v1/contacts/{participant_id}/accept",
            axum::routing::post(accept_contact_route),
        )
        .route(
            "/v1/contacts/{participant_id}/decline",
            axum::routing::post(decline_contact_route),
        )
        .route(
            "/v1/contacts/{participant_id}",
            axum::routing::delete(end_contact_route),
        )
        .route("/v1/blocks", get(list_blocks_route))
        .route(
            "/v1/blocks/{participant_id}",
            axum::routing::post(block_participant_route).delete(unblock_participant_route),
        )
}

/// 解析关系请求体与必填 Idempotency-Key：Human 路由与 Agent 路由**共用**
/// （批次三第二刀）。语义逐条同源：body 超限 413、坏 JSON / 未知字段 400、
/// 缺 key 400 `idempotency_key_required`、非法 key 400 `invalid_idempotency_key`。
// axum 的 `Response` **天生大**（几个头 + body 的枚举），而这里是"把一次请求解析失败
// 直接翻成一个响应"的窄助手 ⇒ 错误路径罕见、**不为它把每个调用点都加一层 `Box` 解引用**。
// 判据一句话：**这一条是样式 lint，不是正确性判据**（`-D warnings` 下必须显式表态）。
#[allow(clippy::result_large_err)]
pub(crate) async fn parse_contact_body_and_key(
    body: Result<Json<CreateContactRequest>, JsonRejection>,
    headers: &HeaderMap,
    request_id: &str,
) -> Result<(Json<CreateContactRequest>, String), Response> {
    let Json(payload) = match body {
        Ok(body) => body,
        Err(JsonRejection::BytesRejection(BytesRejection::FailedToBufferBody(
            FailedToBufferBody::LengthLimitError(_),
        ))) => {
            return Err(error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                codes::PAYLOAD_TOO_LARGE,
                format!("request body exceeds {MAX_AGENT_CREATE_BODY_BYTES} bytes"),
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
    let key = match idempotency_key_from_headers(headers) {
        Ok(key) => key,
        Err(KeyError::Missing) => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                codes::IDEMPOTENCY_KEY_REQUIRED,
                "missing Idempotency-Key header",
                request_id,
            ));
        }
        Err(KeyError::Invalid) => {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_IDEMPOTENCY_KEY,
                "invalid Idempotency-Key header",
                request_id,
            ));
        }
    };
    Ok((Json(payload), key))
}

/// POST /v1/contacts/requests：向另一个 active **主体**发起联系人请求
/// （Human 或 Agent 都可以——批次三第二刀）。首次创建 201；重复请求 /
/// 反向请求（A→B 已有 pending 时 B→A）/ 幂等重放 200 且
/// idempotency_replayed=true；同 requester + key 换目标 409。
/// 目标未知 / 非 active / 双方存在任一方向 active 拉黑段，一律与
/// 「目标不存在」同形 404 `participant_not_found`（不泄露）。
async fn create_contact(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<CreateContactRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    let (Json(payload), key) = match parse_contact_body_and_key(body, &headers, &request_id).await {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    create_contact_response(
        &state,
        &auth.participant,
        &payload.participant_id,
        &key,
        &request_id,
    )
    .await
}

/// 发起关系申请的**响应构造**：Human 路由与 Agent 路由共用（批次三第二刀）。
pub(crate) async fn create_contact_response(
    state: &AppState,
    actor: &Participant,
    target_id: &str,
    key: &str,
    request_id: &str,
) -> Response {
    match send_contact_request(state.contact.as_ref(), actor, target_id, key).await {
        Ok(crate::application::contact::SendContactRequestOutcome::Created { contact, peer }) => (
            StatusCode::CREATED,
            Json(CreateContactResponse {
                contact: contact_view(&contact, &peer, &actor.id),
                idempotency_replayed: false,
                request_id: request_id.to_owned(),
            }),
        )
            .into_response(),
        Ok(crate::application::contact::SendContactRequestOutcome::Existing { contact, peer }) => (
            StatusCode::OK,
            Json(CreateContactResponse {
                contact: contact_view(&contact, &peer, &actor.id),
                idempotency_replayed: true,
                request_id: request_id.to_owned(),
            }),
        )
            .into_response(),
        Err(SendContactRequestError::InvalidIdempotencyKey) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_IDEMPOTENCY_KEY,
            "invalid Idempotency-Key",
            request_id,
        ),
        Err(SendContactRequestError::InvalidSelfRequest) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "cannot send a contact request to yourself",
            request_id,
        ),
        Err(SendContactRequestError::NotFound) => error_response(
            StatusCode::NOT_FOUND,
            codes::PARTICIPANT_NOT_FOUND,
            "participant not found",
            request_id,
        ),
        Err(SendContactRequestError::IdempotencyConflict) => error_response(
            StatusCode::CONFLICT,
            codes::IDEMPOTENCY_CONFLICT,
            "idempotency key was already used with a different request",
            request_id,
        ),
        Err(SendContactRequestError::Store(e)) => {
            store_error_response("contact.request", &e, request_id)
        }
    }
}

/// GET /v1/contacts：当前主体作为 requester 或 addressee 的全部联系人
/// 关系及对端最小资料（对端可以是 Human 或 Agent）；pending 与 accepted
/// 原样公开。
async fn list_contacts_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
) -> Response {
    list_contacts_response(&state, &auth.participant.id, &request_id.0).await
}

/// 关系列表的**响应构造**：Human 路由与 Agent 路由共用（批次三第二刀）。
pub(crate) async fn list_contacts_response(
    state: &AppState,
    viewer_id: &ParticipantId,
    request_id: &str,
) -> Response {
    match list_contacts(state.contact.as_ref(), viewer_id).await {
        Ok(items) => Json(ListContactsResponse {
            contacts: items
                .iter()
                .map(|item| contact_view(&item.contact, &item.peer, viewer_id))
                .collect(),
            request_id: request_id.to_owned(),
        })
        .into_response(),
        Err(e) => query_error_response(e, request_id),
    }
}

/// POST /v1/contacts/{participant_id}/accept：由 addressee 接受 pending 请求。
/// 成功 200（pending → accepted 或已 accepted 幂等）；不存在、当前主体
/// 不是 addressee、bad-id 统一 404 contact_not_found。
async fn accept_contact_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(participant_id): Path<String>,
    request_id: RequestId,
) -> Response {
    accept_contact_response(&state, &auth.participant.id, &participant_id, &request_id.0).await
}

/// 接受的**响应构造**：Human 路由与 Agent 路由共用（批次三第二刀）。
pub(crate) async fn accept_contact_response(
    state: &AppState,
    addressee_id: &ParticipantId,
    requester_id: &str,
    request_id: &str,
) -> Response {
    match accept_contact(state.contact.as_ref(), addressee_id, requester_id).await {
        Ok((contact, peer)) => Json(AcceptContactResponse {
            contact: contact_view(&contact, &peer, addressee_id),
            request_id: request_id.to_owned(),
        })
        .into_response(),
        Err(e) => query_error_response(e, request_id),
    }
}

/// POST /v1/contacts/{participant_id}/decline：由 addressee 拒绝 pending 请求
/// （段以 declined 关闭，204）。不存在、当前主体不是 addressee、段不是
/// pending、bad-id 统一 404 contact_not_found，不泄露关系是否存在。
async fn decline_contact_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(participant_id): Path<String>,
    request_id: RequestId,
) -> Response {
    decline_contact_response(&state, &auth.participant.id, &participant_id, &request_id.0).await
}

/// 拒绝的**响应构造**：Human 路由与 Agent 路由共用（批次三第二刀）。
pub(crate) async fn decline_contact_response(
    state: &AppState,
    addressee_id: &ParticipantId,
    requester_id: &str,
    request_id: &str,
) -> Response {
    match decline_contact(state.contact.as_ref(), addressee_id, requester_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => query_error_response(e, request_id),
    }
}

/// DELETE /v1/contacts/{participant_id}：任一方单方面解除关系（active 段以
/// ended 关闭并关闭双方在该 human_direct 房间的成员周期，204）。没有 active
/// 段是成功 no-op（幂等 204）；bad-id 统一 404 contact_not_found。
async fn end_contact_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(participant_id): Path<String>,
    request_id: RequestId,
) -> Response {
    end_contact_response(&state, &auth.participant.id, &participant_id, &request_id.0).await
}

/// 解除的**响应构造**：Human 路由与 Agent 路由共用（批次三第二刀）。
pub(crate) async fn end_contact_response(
    state: &AppState,
    actor_id: &ParticipantId,
    peer_id: &str,
    request_id: &str,
) -> Response {
    match end_contact(state.contact.as_ref(), actor_id, peer_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => query_error_response(e, request_id),
    }
}

/// GET /v1/blocks：当前主体作为 blocker 的全部 active 拉黑段及对端最小资料
/// （对端可以是 Human 或 Agent——批次三第三刀第①步）。
async fn list_blocks_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
) -> Response {
    list_blocks_response(&state, &auth.participant.id, &request_id.0).await
}

/// 拉黑列表的**响应构造**：Human 路由与 Agent 路由共用（批次三第三刀）。
pub(crate) async fn list_blocks_response(
    state: &AppState,
    blocker_id: &ParticipantId,
    request_id: &str,
) -> Response {
    match list_blocks(state.contact.as_ref(), blocker_id).await {
        Ok(items) => Json(ListBlocksResponse {
            blocks: items.iter().map(block_view).collect(),
            request_id: request_id.to_owned(),
        })
        .into_response(),
        Err(e) => query_error_response(e, request_id),
    }
}

/// POST /v1/blocks/{participant_id}：拉黑另一个 active **主体**（同一事务内先
/// 结束既有 active 段再写拉黑段，204）。目标 unknown / 非 active 统一 404
/// `participant_not_found`；向自己拉黑 400 invalid_request；已有同方向 active
/// 拉黑段是幂等 204。被拉黑方对此一无所知（公开面不出现黑名单语义）。
async fn block_participant_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(participant_id): Path<String>,
    request_id: RequestId,
) -> Response {
    block_participant_response(&state, &auth.participant, &participant_id, &request_id.0).await
}

/// 拉黑的**响应构造**：Human 路由与 Agent 路由共用（批次三第三刀）。
pub(crate) async fn block_participant_response(
    state: &AppState,
    blocker: &Participant,
    blocked_id: &str,
    request_id: &str,
) -> Response {
    match block_participant(state.contact.as_ref(), blocker, blocked_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => block_error_response(e, request_id),
    }
}

/// DELETE /v1/blocks/{participant_id}：解除拉黑（释放当前主体 → 目标方向
/// active 段，204）；没有 active 段是成功 no-op（幂等 204）。
async fn unblock_participant_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(participant_id): Path<String>,
    request_id: RequestId,
) -> Response {
    unblock_participant_response(&state, &auth.participant, &participant_id, &request_id.0).await
}

/// 解除拉黑的**响应构造**：Human 路由与 Agent 路由共用（批次三第三刀）。
pub(crate) async fn unblock_participant_response(
    state: &AppState,
    blocker: &Participant,
    blocked_id: &str,
    request_id: &str,
) -> Response {
    match unblock_participant(state.contact.as_ref(), blocker, blocked_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => block_error_response(e, request_id),
    }
}

/// 拉黑失败映射：只用既有错误码（不新增表达黑名单）。
fn block_error_response(e: BlockContactError, request_id: &str) -> Response {
    match e {
        BlockContactError::NotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::PARTICIPANT_NOT_FOUND,
            "participant not found",
            request_id,
        ),
        BlockContactError::InvalidSelfBlock => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "cannot block yourself",
            request_id,
        ),
        BlockContactError::Store(e) => store_error_response("contact.block", &e, request_id),
    }
}

/// 查询失败：unknown / bad-id / 非 addressee 统一 404 contact_not_found。
fn query_error_response(e: ContactQueryError, request_id: &str) -> Response {
    match e {
        ContactQueryError::NotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::CONTACT_NOT_FOUND,
            "contact not found",
            request_id,
        ),
        ContactQueryError::Store(e) => {
            store_error_response("contact.list_or_accept", &e, request_id)
        }
    }
}

fn contact_view(
    contact: &Contact,
    peer: &ParticipantProfile,
    viewer_id: &ParticipantId,
) -> ContactView {
    ContactView {
        peer: ParticipantMinView {
            id: peer.id.as_str().to_owned(),
            kind: peer.kind.as_str().to_owned(),
            display_name: peer.display_name.as_str().to_owned(),
            status: peer.status.as_str().to_owned(),
        },
        status: contact.status.as_str().to_owned(),
        // 方向相对**观察者**：我是 addressee ⇒ 这条等我回答（incoming）。
        direction: if contact.addressee_id == *viewer_id {
            "incoming".to_owned()
        } else {
            "outgoing".to_owned()
        },
        created_at: contact.created_at.to_rfc3339(),
        accepted_at: contact.accepted_at.map(|t| t.to_rfc3339()),
    }
}

fn block_view(blocked: &BlockedHuman) -> BlockView {
    BlockView {
        peer: ParticipantMinView {
            id: blocked.peer.id.as_str().to_owned(),
            kind: blocked.peer.kind.as_str().to_owned(),
            display_name: blocked.peer.display_name.as_str().to_owned(),
            status: blocked.peer.status.as_str().to_owned(),
        },
        created_at: blocked.created_at.to_rfc3339(),
    }
}
