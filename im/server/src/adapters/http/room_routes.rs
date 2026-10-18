//! Room HTTP handlers（契约 CORE_V1_DIRECT_ROOMS.md 7.x +
//! CORE_V1_HUMAN_ROOMS_AND_MENTIONS.md 5.3/5.4）。
//!
//! handler 只调用 `application::room` 用例与端口，不直接操作 SQLite；
//! 所有非 2xx 走统一 JSON 错误信封，body 的 error.request_id 与
//! x-request-id 一致。list / get / members 三个端点共用同一可见性裁决
//! （`RoomStore` 的同一查询路径，要求当前 active Membership 周期），不由
//! 各 handler 各自推断。

use axum::extract::rejection::{BytesRejection, FailedToBufferBody, JsonRejection};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};

use crate::adapters::http::AppState;
use crate::adapters::http::agent_routes::{KeyError, idempotency_key_from_headers};
use crate::adapters::http::auth::CurrentAuth;
use crate::adapters::http::human_event_routes::wake_humans;
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::routes::{
    MAX_AGENT_CREATE_BODY_BYTES, error_response, store_error_response,
};
use crate::application::room::{
    AdvanceReadCursorError, CreateDirectRoomError, CreateRoomError, InviteMemberError,
    LeaveMembershipError, RemoveMemberError, RoomQueryError, TransferOwnerError,
    advance_read_cursor, create_direct_room, create_group, create_human_direct_room, get_room,
    get_room_members, invite_group_member, leave_membership, list_rooms, remove_group_member,
    transfer_group_owner,
};
use crate::domain::human_event::{HumanEventKind, HumanEventSource};
use crate::domain::participant::Participant;
use crate::domain::room::{Membership, PeerRead, ReadState, Room};
use crate::ports::room::{MemberView, ParticipantProfile, RoomListItem};
use crate::ports::storage::StorageError;
use crate::protocol::error::codes;
use crate::protocol::room::{
    CreateDirectRoomRequest, CreateDirectRoomResponse, CreateGroupRequest,
    CreateHumanDirectRoomRequest, CreateRoomResponse, GetRoomMembersResponse, GetRoomResponse,
    InviteMemberRequest, InviteMemberResponse, ListRoomItem, ListRoomsResponse, MembersItem,
    MembershipView, ParticipantMinView, PutReadCursorRequest, PutReadCursorResponse, ReadStateView,
    RoomView, TransferOwnerRequest, TransferOwnerResponse,
};

/// read cursor PUT 的 route-local 请求体上限：小而固定（消息体只有
/// message_id），不放宽其他路由的既有上限。
pub const MAX_READ_CURSOR_BODY_BYTES: usize = 1024;

/// Room 切片路由。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/direct-rooms", axum::routing::post(create_room))
        .route(
            "/v1/human-direct-rooms",
            axum::routing::post(create_human_direct_room_route),
        )
        .route("/v1/groups", axum::routing::post(create_group_route))
        .route("/v1/rooms", get(list_rooms_route))
        .route("/v1/rooms/{room_id}", get(get_room_route))
        .route(
            "/v1/rooms/{room_id}/members",
            get(get_members_route).post(invite_member_route),
        )
        .route(
            "/v1/rooms/{room_id}/members/{participant_id}",
            delete(remove_member_route),
        )
        .route(
            "/v1/rooms/{room_id}/leave",
            axum::routing::post(leave_route),
        )
        .route(
            "/v1/rooms/{room_id}/owner-transfer",
            axum::routing::post(transfer_owner_route),
        )
        .merge(read_cursor_router())
}

/// read cursor 路由单独成子 router：其 route-local 请求体上限（1 KiB）只
/// 作用于本端点，不影响 Room 切片其他路由的既有 4 KiB 上限。
fn read_cursor_router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/rooms/{room_id}/read-cursor",
            axum::routing::put(put_read_cursor_route),
        )
        .layer(axum::extract::DefaultBodyLimit::max(
            MAX_READ_CURSOR_BODY_BYTES,
        ))
}

/// POST /v1/direct-rooms：创建 Human–Agent Direct Room（Human owner +
/// Agent member 两份显式 Membership 周期）。请求体 `{"agent_id": ...,
/// "name": ...}`，`name` 可选（省略或 null 都规范化为无标题）。首次创建
/// 201；同 Human + 同 key + 同 Agent + 同规范化标题重放 200（即使
/// Agent/Owner 已 revoked，也返回同一 Room 与当前 Membership 状态）；
/// 同 key 改 Agent 或标题 409 idempotency_conflict；同一 Human–Agent 对
/// 用不同 key 可创建多个 Room。
async fn create_room(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<CreateDirectRoomRequest>, JsonRejection>,
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
                format!("request body exceeds {MAX_AGENT_CREATE_BODY_BYTES} bytes"),
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

    match create_direct_room(
        state.room.as_ref(),
        &auth.participant,
        &payload.agent_id,
        &key,
    )
    .await
    {
        Ok(crate::application::room::CreateDirectRoomOutcome::Created { room, memberships }) => {
            wake_room_memberships(&state, &memberships, &request_id).await;
            (
                StatusCode::CREATED,
                Json(CreateDirectRoomResponse {
                    room: room_view(&room),
                    memberships: memberships.iter().map(membership_view).collect(),
                    idempotency_replayed: false,
                    request_id,
                }),
            )
                .into_response()
        }
        Ok(crate::application::room::CreateDirectRoomOutcome::Replayed { room, memberships }) => (
            StatusCode::OK,
            Json(CreateDirectRoomResponse {
                room: room_view(&room),
                memberships: memberships.iter().map(membership_view).collect(),
                idempotency_replayed: true,
                request_id,
            }),
        )
            .into_response(),
        Err(CreateDirectRoomError::InvalidIdempotencyKey) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_IDEMPOTENCY_KEY,
            "invalid Idempotency-Key",
            &request_id,
        ),
        Err(CreateDirectRoomError::NotFound) => error_response(
            StatusCode::NOT_FOUND,
            codes::AGENT_NOT_FOUND,
            "agent not found",
            &request_id,
        ),
        Err(CreateDirectRoomError::IdempotencyConflict) => error_response(
            StatusCode::CONFLICT,
            codes::IDEMPOTENCY_CONFLICT,
            "idempotency key was already used with a different request",
            &request_id,
        ),
        Err(CreateDirectRoomError::AgentNotActive) => error_response(
            StatusCode::CONFLICT,
            codes::AGENT_NOT_ACTIVE,
            "agent is not active",
            &request_id,
        ),
        Err(CreateDirectRoomError::Store(e)) => {
            store_error_response("room.create_direct", &e, &request_id)
        }
    }
}

/// 请求体解析与幂等键提取的共用前置（human-direct / group / invite /
/// transfer）。
/// 解析 JSON body 与必填 Idempotency-Key：Human 路由与 Backend 路由**共用**
/// （批次三第一刀），语义逐条同源：body 超限 413、未知字段/坏 JSON 400、
/// 缺 key 400 `idempotency_key_required`、非法 key 400 `invalid_idempotency_key`。
// axum 的 `Response` **天生大**（几个头 + body 的枚举），而这里是"把一次请求解析失败
// 直接翻成一个响应"的窄助手 ⇒ 错误路径罕见、**不为它把每个调用点都加一层 `Box` 解引用**。
// 判据一句话：**这一条是样式 lint，不是正确性判据**（`-D warnings` 下必须显式表态）。
#[allow(clippy::result_large_err)]
pub(crate) async fn parse_body_and_key_shared<B>(
    body: Result<Json<B>, JsonRejection>,
    headers: &HeaderMap,
    request_id: &str,
) -> Result<(Json<B>, String), Response>
where
    B: serde::de::DeserializeOwned,
{
    let Json(payload) = parse_body_and_key(body, request_id).await?;
    let key = match require_key(headers, request_id) {
        Ok(key) => key,
        Err(response) => return Err(*response),
    };
    Ok((Json(payload), key))
}

// axum 的 `Response` **天生大**（几个头 + body 的枚举），而这里是"把一次请求解析失败
// 直接翻成一个响应"的窄助手 ⇒ 错误路径罕见、**不为它把每个调用点都加一层 `Box` 解引用**。
// 判据一句话：**这一条是样式 lint，不是正确性判据**（`-D warnings` 下必须显式表态）。
#[allow(clippy::result_large_err)]
async fn parse_body_and_key<B>(
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
    Ok(Json(payload))
}

enum KeyParse {
    Ok(String),
    Missing,
    Invalid,
}

fn parse_key(headers: &HeaderMap) -> KeyParse {
    match idempotency_key_from_headers(headers) {
        Ok(key) => KeyParse::Ok(key),
        Err(KeyError::Missing) => KeyParse::Missing,
        Err(KeyError::Invalid) => KeyParse::Invalid,
    }
}

/// 提取必填 Idempotency-Key（invite / transfer 用），缺失或非法返回错误
/// 响应（box 化避免 large-err）。
fn require_key(headers: &HeaderMap, request_id: &str) -> Result<String, Box<Response>> {
    match parse_key(headers) {
        KeyParse::Ok(key) => Ok(key),
        KeyParse::Missing => Err(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            codes::IDEMPOTENCY_KEY_REQUIRED,
            "missing Idempotency-Key header",
            request_id,
        ))),
        KeyParse::Invalid => Err(Box::new(error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_IDEMPOTENCY_KEY,
            "invalid Idempotency-Key header",
            request_id,
        ))),
    }
}

/// POST /v1/human-direct-rooms：为 accepted 联系人创建 Human direct Room。
/// 首次创建 201；同 Human + 同 key + 同目标重放 200；未接受 409
/// contact_not_accepted；同对已有 Room 409 human_direct_room_exists。
async fn create_human_direct_room_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<CreateHumanDirectRoomRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    let Json(payload) = match parse_body_and_key(body, &request_id).await {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let key = match parse_key(&headers) {
        KeyParse::Ok(key) => key,
        KeyParse::Missing => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::IDEMPOTENCY_KEY_REQUIRED,
                "missing Idempotency-Key header",
                &request_id,
            );
        }
        KeyParse::Invalid => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_IDEMPOTENCY_KEY,
                "invalid Idempotency-Key header",
                &request_id,
            );
        }
    };

    match create_human_direct_room(
        state.room.as_ref(),
        &auth.participant,
        &payload.human_id,
        &key,
    )
    .await
    {
        Ok(crate::application::room::CreateRoomOutcome::Created { room, memberships }) => {
            wake_room_memberships(&state, &memberships, &request_id).await;
            (
                StatusCode::CREATED,
                Json(CreateRoomResponse {
                    room: room_view(&room),
                    memberships: memberships.iter().map(membership_view).collect(),
                    idempotency_replayed: false,
                    request_id,
                }),
            )
                .into_response()
        }
        Ok(crate::application::room::CreateRoomOutcome::Replayed { room, memberships }) => (
            StatusCode::OK,
            Json(CreateRoomResponse {
                room: room_view(&room),
                memberships: memberships.iter().map(membership_view).collect(),
                idempotency_replayed: true,
                request_id,
            }),
        )
            .into_response(),
        Err(e) => create_room_error_response(e, &request_id),
    }
}

/// POST /v1/groups：创建 group Room，当前 Human 自动成为 owner。首次创建
/// 201；同 Human + 同 key + 同群名重放 200；异群名 409。
async fn create_group_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<CreateGroupRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    let Json(payload) = match parse_body_and_key(body, &request_id).await {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let key = match parse_key(&headers) {
        KeyParse::Ok(key) => key,
        KeyParse::Missing => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::IDEMPOTENCY_KEY_REQUIRED,
                "missing Idempotency-Key header",
                &request_id,
            );
        }
        KeyParse::Invalid => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_IDEMPOTENCY_KEY,
                "invalid Idempotency-Key header",
                &request_id,
            );
        }
    };

    create_group_response(&state, &auth.participant, &payload.name, &key, &request_id).await
}

/// 建群的**响应构造**：Human 路由（`CurrentAuth`）与 Backend 路由（`BackendAuth`）
/// **共用这一段**——2026-09-27 批次三第一刀「owner = 创建者」：owner 是房内的治理
/// 角色（`role`），不是参与者种类（`kind`），所以一支 Agent 也能建房。两条认证面
/// 只在**参数来源**上分叉，不复制执行链、不各写一套裁决（本仓库既有的分叉纪律）。
pub(crate) async fn create_group_response(
    state: &AppState,
    actor: &Participant,
    name_raw: &str,
    key: &str,
    request_id: &str,
) -> Response {
    match create_group(state.room.as_ref(), actor, name_raw, key).await {
        Ok(crate::application::room::CreateRoomOutcome::Created { room, memberships }) => {
            wake_room_memberships(state, &memberships, request_id).await;
            (
                StatusCode::CREATED,
                Json(CreateRoomResponse {
                    room: room_view(&room),
                    memberships: memberships.iter().map(membership_view).collect(),
                    idempotency_replayed: false,
                    request_id: request_id.to_owned(),
                }),
            )
                .into_response()
        }
        Ok(crate::application::room::CreateRoomOutcome::Replayed { room, memberships }) => (
            StatusCode::OK,
            Json(CreateRoomResponse {
                room: room_view(&room),
                memberships: memberships.iter().map(membership_view).collect(),
                idempotency_replayed: true,
                request_id: request_id.to_owned(),
            }),
        )
            .into_response(),
        Err(e) => create_room_error_response(e, request_id),
    }
}

/// POST /v1/rooms/{room_id}/members：group owner 邀请 accepted Human 或
/// 自己拥有的 active Agent（必带 Idempotency-Key，先查 key 再查权限）。
/// 首次邀请/重入 201；同 key 重放 200（replayed=true）；新 key 但目标已有
/// active 周期 200（replayed=false）。
async fn invite_member_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<InviteMemberRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    let Json(payload) = match parse_body_and_key(body, &request_id).await {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let key = match require_key(&headers, &request_id) {
        Ok(key) => key,
        Err(response) => return *response,
    };

    invite_member_response(
        &state,
        &auth.participant,
        &room_id,
        &payload.participant_id,
        &key,
        &request_id,
    )
    .await
}

/// 邀请成员的**响应构造**：Human 路由与 Backend 路由共用（批次三第一刀）。
/// 邀请**资格**判据一条：与 owner 有 accepted 联系人段，或该主体被 owner 以
/// active Agent Owner 拥有——两条分支都由应用层/数据库按**关系事实**裁决，
/// 不按"邀请者是不是 Human"分叉。
pub(crate) async fn invite_member_response(
    state: &AppState,
    actor: &Participant,
    room_id: &str,
    participant_id: &str,
    key: &str,
    request_id: &str,
) -> Response {
    match invite_group_member(state.room.as_ref(), actor, room_id, participant_id, key).await {
        Ok(crate::application::room::InviteMemberOutcome::Created { membership }) => {
            // post-commit 唤醒：新周期（首次邀请或重入）才会产生 member.joined
            // fact；Replayed / AlreadyActive 不唤醒。
            wake_humans(
                state,
                &[HumanEventSource::new(
                    HumanEventKind::MemberJoined,
                    membership.membership_id.as_str().to_owned(),
                )],
                request_id,
            )
            .await;
            (
                StatusCode::CREATED,
                Json(InviteMemberResponse {
                    membership: membership_view(&membership),
                    idempotency_replayed: false,
                    request_id: request_id.to_owned(),
                }),
            )
                .into_response()
        }
        Ok(crate::application::room::InviteMemberOutcome::Replayed { membership }) => (
            StatusCode::OK,
            Json(InviteMemberResponse {
                membership: membership_view(&membership),
                idempotency_replayed: true,
                request_id: request_id.to_owned(),
            }),
        )
            .into_response(),
        Ok(crate::application::room::InviteMemberOutcome::AlreadyActive { membership }) => (
            StatusCode::OK,
            Json(InviteMemberResponse {
                membership: membership_view(&membership),
                idempotency_replayed: false,
                request_id: request_id.to_owned(),
            }),
        )
            .into_response(),
        Err(e) => member_error_response(e.into(), request_id),
    }
}

/// DELETE /v1/rooms/{room_id}/members/{participant_id}：group owner 移除
/// active 成员（204）；owner 自身不可移除。
async fn remove_member_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path((room_id, participant_id)): Path<(String, String)>,
    request_id: RequestId,
) -> Response {
    remove_member_response(
        &state,
        &auth.participant,
        &room_id,
        &participant_id,
        &request_id.0,
    )
    .await
}

/// 移除成员的**响应构造**：Human 路由与 Backend 路由共用（批次三第一刀）。
/// 判据是 `role='owner'`（不是 kind）：Agent owner 与 Human owner 同规则，
/// 能移除**任何 active 非 owner 成员**（含 Human）。
pub(crate) async fn remove_member_response(
    state: &AppState,
    actor: &Participant,
    room_id: &str,
    participant_id: &str,
    request_id: &str,
) -> Response {
    match remove_group_member(state.room.as_ref(), actor, room_id, participant_id).await {
        Ok(closed_membership_id) => {
            // post-commit 唤醒：member.left fact 的完整 source 身份
            // （kind + membership_id）。
            wake_humans(
                state,
                &[HumanEventSource::new(
                    HumanEventKind::MemberLeft,
                    closed_membership_id.as_str().to_owned(),
                )],
                request_id,
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => member_error_response(e.into(), request_id),
    }
}

/// POST /v1/rooms/{room_id}/leave：成员主动退群（204）；离群后对该 Room
/// 的所有读写 404；group owner 必须先转让（409 owner_transfer_required）。
async fn leave_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
) -> Response {
    match leave_membership(state.room.as_ref(), &auth.participant.id, &room_id).await {
        Ok(closed_membership_id) => {
            // post-commit 唤醒：member.left fact 的完整 source 身份
            // （含本人自投递）。
            wake_humans(
                &state,
                &[HumanEventSource::new(
                    HumanEventKind::MemberLeft,
                    closed_membership_id.as_str().to_owned(),
                )],
                &request_id.0,
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(LeaveMembershipError::RoomNotFound) => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            &request_id.0,
        ),
        Err(LeaveMembershipError::OwnerTransferRequired) => error_response(
            StatusCode::CONFLICT,
            codes::OWNER_TRANSFER_REQUIRED,
            "the group owner must transfer ownership before leaving",
            &request_id.0,
        ),
        Err(LeaveMembershipError::Store(e)) => {
            store_error_response("room.leave", &e, &request_id.0)
        }
    }
}

/// POST /v1/rooms/{room_id}/owner-transfer：群主原子转让（必带
/// Idempotency-Key，先查 key 再查权限）。200 返回新 owner 的 Membership
/// 周期视图；同 key 重放 replayed=true。
async fn transfer_owner_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<TransferOwnerRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    let Json(payload) = match parse_body_and_key(body, &request_id).await {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let key = match require_key(&headers, &request_id) {
        Ok(key) => key,
        Err(response) => return *response,
    };

    transfer_owner_response(
        &state,
        &auth.participant,
        &room_id,
        &payload.participant_id,
        &key,
        &request_id,
    )
    .await
}

/// 转让群主的**响应构造**：Human 路由与 Backend 路由共用（批次三第一刀）。
/// 目标可以是**任意 active 成员**（含 Agent）：owner 是治理角色，不是参与者种类。
pub(crate) async fn transfer_owner_response(
    state: &AppState,
    actor: &Participant,
    room_id: &str,
    target_id: &str,
    key: &str,
    request_id: &str,
) -> Response {
    match transfer_group_owner(state.room.as_ref(), actor, room_id, target_id, key).await {
        Ok(crate::application::room::TransferOwnerOutcome::Transferred {
            membership,
            transition_id,
        }) => {
            // post-commit 唤醒：owner.transferred fact 的完整 source 身份
            // （不可变 transition 事实）。
            wake_humans(
                state,
                &[HumanEventSource::new(
                    HumanEventKind::OwnerTransferred,
                    transition_id.as_str().to_owned(),
                )],
                request_id,
            )
            .await;
            (
                StatusCode::OK,
                Json(TransferOwnerResponse {
                    membership: membership_view(&membership),
                    idempotency_replayed: false,
                    request_id: request_id.to_owned(),
                }),
            )
                .into_response()
        }
        Ok(crate::application::room::TransferOwnerOutcome::Replayed { membership }) => (
            StatusCode::OK,
            Json(TransferOwnerResponse {
                membership: membership_view(&membership),
                idempotency_replayed: true,
                request_id: request_id.to_owned(),
            }),
        )
            .into_response(),
        Err(TransferOwnerError::InvalidIdempotencyKey) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_IDEMPOTENCY_KEY,
            "invalid Idempotency-Key",
            request_id,
        ),
        Err(TransferOwnerError::RoomNotFound) => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            request_id,
        ),
        Err(TransferOwnerError::MemberNotFound) => error_response(
            StatusCode::NOT_FOUND,
            codes::MEMBER_NOT_FOUND,
            "member not found",
            request_id,
        ),
        Err(TransferOwnerError::InvalidTransferTarget) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_TRANSFER_TARGET,
            "the transfer target must be a different active member",
            request_id,
        ),
        Err(TransferOwnerError::IdempotencyConflict) => error_response(
            StatusCode::CONFLICT,
            codes::IDEMPOTENCY_CONFLICT,
            "idempotency key was already used with a different request",
            request_id,
        ),
        Err(TransferOwnerError::Store(e)) => {
            store_error_response("room.owner_transfer", &e, request_id)
        }
    }
}

/// GET /v1/rooms：只返回当前 Human 存在 active Membership 周期的 Room，
/// 按创建时间升序、再按 Room ID 排序；Room 恒 active，对端 Agent 可能已
/// revoked。
async fn list_rooms_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
) -> Response {
    match list_rooms(state.room.as_ref(), &auth.participant.id).await {
        Ok(rooms) => Json(ListRoomsResponse {
            rooms: rooms.iter().map(list_item_view).collect(),
            request_id: request_id.0,
        })
        .into_response(),
        Err(e) => query_error_response(e, &request_id.0),
    }
}

/// GET /v1/rooms/{room_id}：member-scoped 管理视图；unknown、bad-id、
/// not-member 统一 404 `room_not_found`。
async fn get_room_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
) -> Response {
    match get_room(state.room.as_ref(), &auth.participant.id, &room_id).await {
        Ok(item) => Json(GetRoomResponse {
            room: room_view(&item.room),
            membership: membership_view(&item.membership),
            peer: item.peer.as_ref().map(peer_view),
            read_state: read_state_view(&item.read_state),
            peer_read: peer_read_view(item.peer_read),
            request_id: request_id.0,
        })
        .into_response(),
        Err(e) => query_error_response(e, &request_id.0),
    }
}

/// GET /v1/rooms/{room_id}/members：member-scoped；group 只列 active 周期
/// （owner 在前，其余按 joined_at 升序、再按 Participant ID 排序）；
/// direct / human_direct 保持配对成员展示（含 Agent revoke 后 closed 的
/// Agent 周期）。
async fn get_members_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
) -> Response {
    match get_room_members(state.room.as_ref(), &auth.participant.id, &room_id).await {
        Ok(members) => Json(GetRoomMembersResponse {
            members: members.iter().map(member_item_view).collect(),
            request_id: request_id.0,
        })
        .into_response(),
        Err(e) => query_error_response(e, &request_id.0),
    }
}

/// PUT /v1/rooms/{room_id}/read-cursor：当前 Human 以一条自己真实可见的
/// Message 表达「已读到这里」；目标高于当前 cursor 原子推进（200
/// advanced=true，同事务生成 read.cursor.advanced Durable Human Event），
/// 同/旧目标成功 no-op（200 advanced=false）。请求体只接受 `message_id`；
/// message_id 非 UUID v4 / 未知字段 / 非法 JSON 为 400 invalid_request；
/// Message 未知、他 Room、不可见统一 400 invalid_read_cursor；unknown /
/// bad-id / 无 active Membership 统一 404 room_not_found；失败不改变
/// cursor、不生成 advancement/Event。
async fn put_read_cursor_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
    body: Result<Json<PutReadCursorRequest>, JsonRejection>,
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
                format!("request body exceeds {MAX_READ_CURSOR_BODY_BYTES} bytes"),
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

    match advance_read_cursor(
        state.room.as_ref(),
        &auth.participant.id,
        &room_id,
        &payload.message_id,
    )
    .await
    {
        Ok(crate::application::room::AdvanceReadCursorOutcome::Advanced {
            read_state,
            advancement_id,
        }) => {
            // post-commit 定向唤醒：从已提交 delivery/fact 按完整 source
            // 身份（kind + advancement_id）反查目标 Human（只推进者本人），
            // best-effort，失败只记录脱敏 warn，不影响已提交响应。
            wake_humans(
                &state,
                &[HumanEventSource::new(
                    HumanEventKind::ReadCursorAdvanced,
                    advancement_id,
                )],
                &request_id,
            )
            .await;
            (
                StatusCode::OK,
                Json(PutReadCursorResponse {
                    read_state: read_state_view(&read_state),
                    advanced: true,
                    request_id,
                }),
            )
                .into_response()
        }
        Ok(crate::application::room::AdvanceReadCursorOutcome::NoOp { read_state }) => (
            StatusCode::OK,
            Json(PutReadCursorResponse {
                read_state: read_state_view(&read_state),
                advanced: false,
                request_id,
            }),
        )
            .into_response(),
        Err(AdvanceReadCursorError::RoomNotFound) => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            &request_id,
        ),
        Err(AdvanceReadCursorError::InvalidMessageId) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "message_id must be a UUID v4",
            &request_id,
        ),
        Err(AdvanceReadCursorError::InvalidReadCursor) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_READ_CURSOR,
            "the message cannot be the read cursor target",
            &request_id,
        ),
        Err(AdvanceReadCursorError::Store(e)) => {
            store_error_response("room.read_cursor", &e, &request_id)
        }
    }
}

/// 房间创建成功路径的 post-commit 唤醒：奠基 Membership 周期（member.joined
/// fact 的完整 source 身份）交给统一唤醒助手。
async fn wake_room_memberships(state: &AppState, memberships: &[Membership], request_id: &str) {
    let sources: Vec<HumanEventSource> = memberships
        .iter()
        .map(|m| {
            HumanEventSource::new(
                HumanEventKind::MemberJoined,
                m.membership_id.as_str().to_owned(),
            )
        })
        .collect();
    wake_humans(state, &sources, request_id).await;
}

/// 查询失败：unknown / bad-id / 非成员统一 404 `room_not_found`。
pub(crate) fn query_error_response(e: RoomQueryError, request_id: &str) -> Response {
    match e {
        RoomQueryError::NotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            request_id,
        ),
        RoomQueryError::Store(e) => store_error_response("room.list_or_get", &e, request_id),
    }
}

/// human-direct / group 创建错误映射。
fn create_room_error_response(e: CreateRoomError, request_id: &str) -> Response {
    match e {
        CreateRoomError::InvalidIdempotencyKey => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_IDEMPOTENCY_KEY,
            "invalid Idempotency-Key",
            request_id,
        ),
        CreateRoomError::HumanNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::HUMAN_NOT_FOUND,
            "human not found",
            request_id,
        ),
        CreateRoomError::ContactNotAccepted => error_response(
            StatusCode::CONFLICT,
            codes::CONTACT_NOT_ACCEPTED,
            "the contact relationship with this human is not accepted",
            request_id,
        ),
        CreateRoomError::HumanDirectRoomExists => error_response(
            StatusCode::CONFLICT,
            codes::HUMAN_DIRECT_ROOM_EXISTS,
            "a human direct room already exists for this human pair",
            request_id,
        ),
        CreateRoomError::IdempotencyConflict => error_response(
            StatusCode::CONFLICT,
            codes::IDEMPOTENCY_CONFLICT,
            "idempotency key was already used with a different request",
            request_id,
        ),
        CreateRoomError::InvalidGroupName => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "invalid group name",
            request_id,
        ),
        CreateRoomError::Store(e) => store_error_response("room.create", &e, request_id),
    }
}

/// 邀请 / 移除错误映射。
fn member_error_response(e: MemberError, request_id: &str) -> Response {
    match e {
        MemberError::InvalidIdempotencyKey => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_IDEMPOTENCY_KEY,
            "invalid Idempotency-Key",
            request_id,
        ),
        MemberError::RoomNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            request_id,
        ),
        MemberError::MemberNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::MEMBER_NOT_FOUND,
            "member not found",
            request_id,
        ),
        MemberError::IdempotencyConflict => error_response(
            StatusCode::CONFLICT,
            codes::IDEMPOTENCY_CONFLICT,
            "idempotency key was already used with a different request",
            request_id,
        ),
        MemberError::OwnerNotRemovable => error_response(
            StatusCode::CONFLICT,
            codes::OWNER_NOT_REMOVABLE,
            "the group owner cannot be removed",
            request_id,
        ),
        MemberError::Store(e) => store_error_response("room.member_change", &e, request_id),
    }
}

/// 邀请与移除错误的统一内部类型（映射自 application 错误）。
enum MemberError {
    InvalidIdempotencyKey,
    RoomNotFound,
    MemberNotFound,
    IdempotencyConflict,
    OwnerNotRemovable,
    Store(StorageError),
}

impl From<InviteMemberError> for MemberError {
    fn from(e: InviteMemberError) -> Self {
        match e {
            InviteMemberError::InvalidIdempotencyKey => Self::InvalidIdempotencyKey,
            InviteMemberError::RoomNotFound => Self::RoomNotFound,
            InviteMemberError::MemberNotFound => Self::MemberNotFound,
            InviteMemberError::IdempotencyConflict => Self::IdempotencyConflict,
            InviteMemberError::Store(e) => Self::Store(e),
        }
    }
}

impl From<RemoveMemberError> for MemberError {
    fn from(e: RemoveMemberError) -> Self {
        match e {
            RemoveMemberError::RoomNotFound => Self::RoomNotFound,
            RemoveMemberError::MemberNotFound => Self::MemberNotFound,
            RemoveMemberError::OwnerNotRemovable => Self::OwnerNotRemovable,
            RemoveMemberError::Store(e) => Self::Store(e),
        }
    }
}

pub(crate) fn room_view(room: &Room) -> RoomView {
    RoomView {
        id: room.id.as_str().to_owned(),
        kind: room.kind.as_str().to_owned(),
        status: room.status.as_str().to_owned(),
        name: room.name.clone(),
        created_at: room.created_at.to_rfc3339(),
    }
}

pub(crate) fn membership_view(membership: &Membership) -> MembershipView {
    MembershipView {
        membership_id: membership.membership_id.as_str().to_owned(),
        participant_id: membership.participant_id.as_str().to_owned(),
        role: membership.role.as_str().to_owned(),
        joined_at: membership.joined_at.to_rfc3339(),
        left_at: membership.left_at.map(|t| t.to_rfc3339()),
        closed_by: membership.closed_by.map(|c| c.as_str().to_owned()),
    }
}

fn peer_view(peer: &ParticipantProfile) -> ParticipantMinView {
    ParticipantMinView {
        id: peer.id.as_str().to_owned(),
        kind: peer.kind.as_str().to_owned(),
        display_name: peer.display_name.as_str().to_owned(),
        status: peer.status.as_str().to_owned(),
    }
}

fn list_item_view(item: &RoomListItem) -> ListRoomItem {
    ListRoomItem {
        room: room_view(&item.room),
        membership: membership_view(&item.membership),
        peer: item.peer.as_ref().map(peer_view),
        read_state: read_state_view(&item.read_state),
        peer_read: peer_read_view(item.peer_read),
    }
}

/// 「对方读没读我最后那条」的对外取值（契约 `CORE_V1_READ_RECEIPT.md` §2）：
/// **两态**，而 `None` ＝ **问题不成立**（那一格不出现）——不是"没读"。
/// ⚠ 它是**读位置**，不是"对方看到了"：契约与呈现都不许把它写成后者。
pub(crate) fn peer_read_view(value: Option<PeerRead>) -> Option<String> {
    value.map(|read| read.as_str().to_owned())
}

/// 对外 read state 视图：只暴露 last_read_message_id（可 null）与
/// unread_count；内部初始 boundary / cursor sequence 不进入公开响应。
/// Human 侧（房间列表 / 读游标响应）与 Agent 侧（Backend 通讯录 /
/// Backend 读游标响应）**共用本函数**——同一份投影，不各写一遍
/// （CORE_V1_AGENT_READ_CURSOR.md §3）。
pub(crate) fn read_state_view(state: &ReadState) -> ReadStateView {
    ReadStateView {
        last_read_message_id: state
            .last_read_message_id
            .as_ref()
            .map(|id| id.as_str().to_owned()),
        unread_count: state.unread_count,
    }
}

pub(crate) fn member_item_view(item: &MemberView) -> MembersItem {
    MembersItem {
        membership: membership_view(&item.membership),
        participant: peer_view(&item.participant),
    }
}
