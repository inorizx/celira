//! Participant Presentation HTTP handlers
//! （CORE_V1_PARTICIPANT_PRESENTATION.md §3–§7）。
//!
//! handler 只调用 `application::participant_presentation` 用例与窄端口，不
//! 直接操作 SQLite；所有非 2xx 走统一 JSON 错误信封。Room 可见性复用
//! `RoomStore`（active Membership 同一裁决），Agent Owner 授权在 Profile/
//! Presentation 写路径由存储端口在 `BEGIN IMMEDIATE` 事务内先于重放裁决
//! ——路由层绝不重复弱化检查、不引入第二个写者。
//!
//! PUT Profile / Presentation 独立接受较大的请求体（public_description /
//! room_role_description 上限接近 16 KiB / 4 KiB，JSON 转义后仍有余量）；
//! 超出返回 413 payload_too_large。正文的字段级字节 / NUL 边界在域层裁决
//! （400 profile_invalid / room_presentation_invalid），不属于路由上限。

use axum::extract::rejection::{BytesRejection, FailedToBufferBody, JsonRejection};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use crate::adapters::http::AppState;
use crate::adapters::http::agent_routes::{
    KeyError, agent_view, idempotency_key_from_headers, ownership_view,
};
use crate::adapters::http::auth::CurrentAuth;
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::routes::{error_response, store_error_response};
use crate::application::participant_presentation::{
    PresentationPutError, PresentationQueryError, ProfileGetError, ProfilePutError,
    get_member_presentation, get_member_presentations, get_member_profile, get_owner_profile,
    put_owner_profile, put_room_presentation,
};
use crate::domain::participant::Participant;
use crate::domain::participant_presentation::preview;
use crate::ports::participant_presentation::{
    MemberPresentationSummary, ProfileRead, RoomPresentationRead,
};
use crate::ports::room::ParticipantProfile;
use crate::protocol::error::codes;
use crate::protocol::participant_avatar::AvatarView;
use crate::protocol::participant_presentation::{
    GetMemberPresentationsResponse, GetPresentationResponse, GetProfileResponse,
    MemberPresentationSummaryItem, MemberProfileResponse, PresentationView, ProfileView,
    PutPresentationRequest, PutPresentationResponse, PutProfileRequest, PutProfileResponse,
};
use crate::protocol::room::ParticipantMinView;

/// Presentation 切片 PUT 请求体上限：64 KiB，容纳合法 public_description
/// （<= 16384 字节）与 room_role_description（<= 4096 字节）的 JSON 转义
/// 表达；GET 无 body，不受影响。
pub const MAX_PRESENTATION_PUT_BODY_BYTES: usize = 64 * 1024;

/// member-presentations 摘要 preview 的固定字节预算（契约 §2.2 投影规则）。
pub const MEMBER_PRESENTATION_PREVIEW_MAX_BYTES: usize = 512;

/// Presentation 切片路由。
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/agents/{agent_id}/profile",
            get(get_owner_profile_route).put(put_owner_profile_route),
        )
        .route(
            "/v1/rooms/{room_id}/member-presentations",
            get(get_member_presentations_route),
        )
        .route(
            "/v1/rooms/{room_id}/members/{participant_id}/profile",
            get(get_member_profile_route),
        )
        .route(
            "/v1/rooms/{room_id}/members/{participant_id}/presentation",
            get(get_member_presentation_route).put(put_room_presentation_route),
        )
        .layer(axum::extract::DefaultBodyLimit::max(
            MAX_PRESENTATION_PUT_BODY_BYTES,
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
                format!("request body exceeds {MAX_PRESENTATION_PUT_BODY_BYTES} bytes"),
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

/// GET /v1/agents/{agent_id}/profile：active Agent Owner Human Session 只读
/// 最新 Profile；无版本返回 profile=null。unknown / bad-id / 从未拥有统一
/// 404 `agent_not_found`；released 目标统一 409 `agent_not_active`。
async fn get_owner_profile_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(agent_id): Path<String>,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    match get_owner_profile(
        state.agent.as_ref(),
        state.presentation.as_ref(),
        &auth.participant.id,
        &agent_id,
    )
    .await
    {
        Ok(outcome) => Json(GetProfileResponse {
            agent: agent_view(&outcome.owned.agent),
            ownership: ownership_view(&outcome.owned.ownership),
            profile: outcome.profile.as_ref().map(profile_view),
            request_id,
        })
        .into_response(),
        Err(e) => profile_get_error_response(e, &request_id),
    }
}

/// PUT /v1/agents/{agent_id}/profile：active Agent Owner Human Session 唯一
/// 写路径（授权在存储事务内先于重放）。首次创建 201、后续更新 200、同 key
/// 同摘要重放返回首次成功时的状态；`idempotency_replayed` 区分真实重放。
async fn put_owner_profile_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
    body: Result<Json<PutProfileRequest>, JsonRejection>,
) -> Response {
    put_profile_response(
        &state,
        &auth.participant,
        &agent_id,
        &request_id.0,
        &headers,
        body,
    )
    .await
}

/// Profile 写入的**共用实现**（批次四表达权第一刀）：Human 侧（Owner 写它名下 Agent
/// 的资料）与 Agent 侧（`PUT /v1/backend/me/profile`，**写自己的**）走同一段——
/// 同一个 application 用例、同一段响应构造、同一套错误码，只在适配器换认证面。
///
/// `target_id` 是要写谁的资料：Human 侧来自路径，Agent 侧来自**认证本身**
/// （读取面与表达面同一条纪律：路径里没有 agent_id，身份只来自凭据）。
pub(crate) async fn put_profile_response(
    state: &AppState,
    actor: &Participant,
    target_id: &str,
    request_id: &str,
    headers: &HeaderMap,
    body: Result<Json<PutProfileRequest>, JsonRejection>,
) -> Response {
    let Json(payload) = match parse_put_body(body, request_id) {
        Ok(p) => p,
        Err(response) => return response,
    };
    let key = match require_key(headers, request_id) {
        Ok(key) => key,
        Err(response) => return *response,
    };
    let short_description = match payload.short_description {
        Some(v) => v,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "short_description is required (may be null)",
                request_id,
            );
        }
    };
    let public_description = match payload.public_description {
        Some(v) => v,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "public_description is required (may be null)",
                request_id,
            );
        }
    };
    match put_owner_profile(
        state.presentation.as_ref(),
        actor,
        target_id,
        &key,
        payload.base_version,
        short_description.as_deref(),
        public_description.as_deref(),
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
                Json(PutProfileResponse {
                    profile: profile_view(&outcome.result),
                    idempotency_replayed: !outcome.created,
                    request_id: request_id.to_owned(),
                }),
            )
                .into_response()
        }
        Err(e) => profile_put_error_response(e, request_id),
    }
}

/// GET /v1/rooms/{room_id}/member-presentations：当前 active Room member 的
/// 成员 Presentation 摘要列表（active Human 与 Agent 成员都在内；Human 成员
/// 无 Presentation，字段为 null）；summary 绝不含
/// public_description / full role description，preview 由读取时刻从 full
/// stored room_role_description 生成（max 512 UTF-8 bytes）。
async fn get_member_presentations_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    match get_member_presentations(
        state.room.as_ref(),
        state.presentation.as_ref(),
        &auth.participant.id,
        &room_id,
    )
    .await
    {
        Ok(members) => Json(GetMemberPresentationsResponse {
            members: members.iter().map(summary_item_view).collect(),
            request_id,
        })
        .into_response(),
        Err(e) => presentation_query_error_response(e, &request_id),
    }
}

/// GET /v1/rooms/{room_id}/members/{participant_id}/profile：requester 与
/// target 都必须当前 active 成员；返回 participant + 全局 Profile（无版本为
/// null）+ 当前 Presentation（direct / human_direct 恒为 null）。无 Secrets/
/// 私有 Runtime 数据。
async fn get_member_profile_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path((room_id, participant_id)): Path<(String, String)>,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    match get_member_profile(
        state.room.as_ref(),
        state.presentation.as_ref(),
        &auth.participant.id,
        &room_id,
        &participant_id,
    )
    .await
    {
        Ok(detail) => Json(MemberProfileResponse {
            participant: min_view(&detail.participant),
            profile: detail.profile.as_ref().map(profile_view),
            presentation: detail.presentation.as_ref().map(presentation_view),
            request_id,
        })
        .into_response(),
        Err(e) => presentation_query_error_response(e, &request_id),
    }
}

/// GET /v1/rooms/{room_id}/members/{participant_id}/presentation：requester
/// 与 target 都必须当前 active 成员；direct / human_direct 无 Presentation
/// （返回 null）。
async fn get_member_presentation_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    Path((room_id, participant_id)): Path<(String, String)>,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    match get_member_presentation(
        state.room.as_ref(),
        state.presentation.as_ref(),
        &auth.participant.id,
        &room_id,
        &participant_id,
    )
    .await
    {
        Ok(p) => Json(GetPresentationResponse {
            participant: min_view(&p.participant),
            presentation: p.presentation.as_ref().map(presentation_view),
            request_id,
        })
        .into_response(),
        Err(e) => presentation_query_error_response(e, &request_id),
    }
}

/// PUT /v1/rooms/{room_id}/members/{participant_id}/presentation：current
/// Group active owner 为 current active Agent member 写 Presentation；仅
/// Agent Owner 而无该 Room 成员资格的 Human 无 Room 权威（存储事务内裁决）；
/// direct / human_direct → 400 `room_presentation_not_supported`。首次创建
/// 201、后续更新 200、同 key 同摘要重放返回首次成功时的状态。
async fn put_room_presentation_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
    headers: HeaderMap,
    Path((room_id, participant_id)): Path<(String, String)>,
    body: Result<Json<PutPresentationRequest>, JsonRejection>,
) -> Response {
    put_room_presentation_response(
        &state,
        &auth.participant,
        &room_id,
        &participant_id,
        &request_id.0,
        &headers,
        body,
    )
    .await
}

/// 房呈现写入的**共用实现**（批次四表达权第三刀）：Human 侧（房 owner 为成员写）与
/// Agent 侧（`PUT /v1/backend/rooms/{room_id}/my-presentation`，**写自己的那一份**）
/// 走同一段——同一个 application 用例、同一段响应构造、同一套错误码。
///
/// `target_id` 是被呈现者：Human 侧来自路径，Agent 侧来自**认证本身**。
pub(crate) async fn put_room_presentation_response(
    state: &AppState,
    actor: &Participant,
    room_id: &str,
    target_id: &str,
    request_id: &str,
    headers: &HeaderMap,
    body: Result<Json<PutPresentationRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.to_owned();
    let Json(payload) = match parse_put_body(body, &request_id) {
        Ok(p) => p,
        Err(response) => return response,
    };
    let key = match require_key(headers, &request_id) {
        Ok(key) => key,
        Err(response) => return *response,
    };
    let room_display_name = match payload.room_display_name {
        Some(v) => v,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "room_display_name is required (may be null)",
                &request_id,
            );
        }
    };
    let room_role_name = match payload.room_role_name {
        Some(v) => v,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "room_role_name is required (may be null)",
                &request_id,
            );
        }
    };
    let room_role_description = match payload.room_role_description {
        Some(v) => v,
        None => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "room_role_description is required (may be null)",
                &request_id,
            );
        }
    };
    match put_room_presentation(
        state.presentation.as_ref(),
        actor,
        room_id,
        target_id,
        &key,
        payload.base_version,
        room_display_name.as_deref(),
        room_role_name.as_deref(),
        room_role_description.as_deref(),
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
                Json(PutPresentationResponse {
                    presentation: presentation_view(&outcome.result),
                    idempotency_replayed: !outcome.created,
                    request_id,
                }),
            )
                .into_response()
        }
        Err(e) => presentation_put_error_response(e, &request_id),
    }
}

// ---------- 错误映射 ----------

fn profile_get_error_response(e: ProfileGetError, request_id: &str) -> Response {
    match e {
        ProfileGetError::NotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::AGENT_NOT_FOUND,
            "agent not found",
            request_id,
        ),
        ProfileGetError::NotActive => error_response(
            StatusCode::CONFLICT,
            codes::AGENT_NOT_ACTIVE,
            "agent is not active",
            request_id,
        ),
        ProfileGetError::Store(e) => {
            store_error_response("participant_presentation.get_owner_profile", &e, request_id)
        }
    }
}

fn profile_put_error_response(e: ProfilePutError, request_id: &str) -> Response {
    match e {
        ProfilePutError::InvalidIdempotencyKey => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_IDEMPOTENCY_KEY,
            "invalid Idempotency-Key",
            request_id,
        ),
        ProfilePutError::InvalidBody => error_response(
            StatusCode::BAD_REQUEST,
            codes::PROFILE_INVALID,
            "invalid profile body",
            request_id,
        ),
        ProfilePutError::NotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::AGENT_NOT_FOUND,
            "agent not found",
            request_id,
        ),
        ProfilePutError::NotActive => error_response(
            StatusCode::CONFLICT,
            codes::AGENT_NOT_ACTIVE,
            "agent is not active",
            request_id,
        ),
        ProfilePutError::IdempotencyConflict => error_response(
            StatusCode::CONFLICT,
            codes::IDEMPOTENCY_CONFLICT,
            "idempotency key was already used with a different request",
            request_id,
        ),
        ProfilePutError::Stale => error_response(
            StatusCode::CONFLICT,
            codes::PROFILE_STALE,
            "profile base_version is stale",
            request_id,
        ),
        ProfilePutError::Store(e) => {
            store_error_response("participant_presentation.put_owner_profile", &e, request_id)
        }
    }
}

pub(crate) fn presentation_query_error_response(
    e: PresentationQueryError,
    request_id: &str,
) -> Response {
    match e {
        PresentationQueryError::RoomNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            request_id,
        ),
        PresentationQueryError::MemberNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::MEMBER_NOT_FOUND,
            "member not found",
            request_id,
        ),
        PresentationQueryError::Store(e) => {
            store_error_response("participant_presentation.room_query", &e, request_id)
        }
    }
}

fn presentation_put_error_response(e: PresentationPutError, request_id: &str) -> Response {
    match e {
        PresentationPutError::InvalidIdempotencyKey => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_IDEMPOTENCY_KEY,
            "invalid Idempotency-Key",
            request_id,
        ),
        PresentationPutError::InvalidBody => error_response(
            StatusCode::BAD_REQUEST,
            codes::ROOM_PRESENTATION_INVALID,
            "invalid room presentation body",
            request_id,
        ),
        PresentationPutError::RoomNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            request_id,
        ),
        PresentationPutError::MemberNotFound => error_response(
            StatusCode::NOT_FOUND,
            codes::MEMBER_NOT_FOUND,
            "member not found",
            request_id,
        ),
        PresentationPutError::NotSupported => error_response(
            StatusCode::BAD_REQUEST,
            codes::ROOM_PRESENTATION_NOT_SUPPORTED,
            "room presentation is not supported for this room",
            request_id,
        ),
        PresentationPutError::IdempotencyConflict => error_response(
            StatusCode::CONFLICT,
            codes::IDEMPOTENCY_CONFLICT,
            "idempotency key was already used with a different request",
            request_id,
        ),
        PresentationPutError::Stale => error_response(
            StatusCode::CONFLICT,
            codes::ROOM_PRESENTATION_STALE,
            "room presentation base_version is stale",
            request_id,
        ),
        PresentationPutError::Store(e) => store_error_response(
            "participant_presentation.put_room_presentation",
            &e,
            request_id,
        ),
    }
}

// ---------- 视图 ----------

pub(crate) fn profile_view(p: &ProfileRead) -> ProfileView {
    ProfileView {
        participant_id: p.participant_id.as_str().to_owned(),
        profile_version: p.profile_version.as_u64(),
        short_description: p.short_description.as_ref().map(|s| s.as_str().to_owned()),
        public_description: p.public_description.as_ref().map(|s| s.as_str().to_owned()),
        avatar: p.avatar.as_ref().map(|a| AvatarView {
            content_type: a.content_type.as_str().to_owned(),
            size_bytes: a.size_bytes,
        }),
        published_by_id: p.published_by_id.as_str().to_owned(),
        published_by_kind: p.published_by_kind.as_str().to_owned(),
        published_at: p.published_at.to_rfc3339(),
    }
}

pub(crate) fn presentation_view(p: &RoomPresentationRead) -> PresentationView {
    PresentationView {
        room_id: p.room_id.as_str().to_owned(),
        participant_id: p.participant_id.as_str().to_owned(),
        presentation_version: p.presentation_version.as_u64(),
        room_display_name: p.room_display_name.as_ref().map(|s| s.as_str().to_owned()),
        room_role_name: p.room_role_name.as_ref().map(|s| s.as_str().to_owned()),
        room_role_description: p
            .room_role_description
            .as_ref()
            .map(|s| s.as_str().to_owned()),
        published_by_id: p.published_by_id.as_str().to_owned(),
        published_by_kind: p.published_by_kind.as_str().to_owned(),
        published_at: p.published_at.to_rfc3339(),
    }
}

pub(crate) fn min_view(p: &ParticipantProfile) -> ParticipantMinView {
    ParticipantMinView {
        id: p.id.as_str().to_owned(),
        kind: p.kind.as_str().to_owned(),
        display_name: p.display_name.as_str().to_owned(),
        status: p.status.as_str().to_owned(),
    }
}

pub(crate) fn summary_item_view(s: &MemberPresentationSummary) -> MemberPresentationSummaryItem {
    MemberPresentationSummaryItem {
        participant: min_view(&s.participant),
        short_description: s.short_description.as_ref().map(|t| t.as_str().to_owned()),
        avatar: s.avatar.as_ref().map(|a| AvatarView {
            content_type: a.content_type.as_str().to_owned(),
            size_bytes: a.size_bytes,
        }),
        room_display_name: s.room_display_name.as_ref().map(|t| t.as_str().to_owned()),
        room_role_name: s.room_role_name.as_ref().map(|t| t.as_str().to_owned()),
        room_role_description_preview: s
            .room_role_description
            .as_ref()
            .map(|t| preview(t.as_str(), MEMBER_PRESENTATION_PREVIEW_MAX_BYTES).to_owned()),
    }
}
