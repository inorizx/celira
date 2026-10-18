//! Agent 读取面 HTTP handler（契约 CORE_V1_AGENT_READ_SURFACE.md，窄解冻切片
//! 「Agent 读取面对齐」）。
//!
//! Agent 是房间成员，此前却看不见同房有谁、也看不见自己的公共呈现。本模块
//! 补齐这条对称：**看见房间里的人**（5 条）与**看见自己**（1 条），全部
//! BackendAuth 认证。
//!
//! **唯一裁决，不复制**：全部房间范围端点复用 Human 侧同一用例
//! （`application::room::get_room_members`、
//! `application::participant_presentation::get_member_*`、
//! `application::participant_avatar::get_member_avatar_bytes`），投影与响应头
//! 复用 Human 侧同一投影函数——**不新增任何 Agent 专用可见性判据**。
//!
//! **身份只来自认证**：`/v1/backend/profile`
//! 不接受路径中的 agent_id，从形状上排除「读别人的」。
//!
//! 零写语义、零持久对象新增、零既有端点变更：读取不写 cursor、不生成
//! Event、不推进投递位置。

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use crate::adapters::http::AppState;
use crate::adapters::http::backend_auth::BackendAuth;
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::participant_avatar_routes::avatar_bytes_response;
use crate::adapters::http::participant_presentation_routes::{
    min_view, presentation_query_error_response, presentation_view, profile_view, summary_item_view,
};
use crate::adapters::http::room_routes::{member_item_view, query_error_response};
use crate::adapters::http::routes::{error_response, store_error_response};
use crate::application::participant_avatar::get_member_avatar_bytes;
use crate::application::participant_presentation::{
    get_member_presentation, get_member_presentations, get_member_profile, get_self_profile,
};
use crate::application::room::get_room_members;
use crate::protocol::error::codes;
use crate::protocol::participant_presentation::{
    GetMemberPresentationsResponse, GetPresentationResponse, GetSelfProfileResponse,
    MemberProfileResponse,
};
use crate::protocol::room::GetRoomMembersResponse;

/// Agent 读取面路由：6 条纯读端点（契约 §2；原 7 条，`/v1/backend/publication`
/// 随 Agent 服务层退役删除，2026-09-28）。
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/backend/rooms/{room_id}/members",
            get(list_members_route),
        )
        .route(
            "/v1/backend/rooms/{room_id}/member-presentations",
            get(list_member_presentations_route),
        )
        .route(
            "/v1/backend/rooms/{room_id}/members/{participant_id}/profile",
            get(get_member_profile_route),
        )
        .route(
            "/v1/backend/rooms/{room_id}/members/{participant_id}/presentation",
            get(get_member_presentation_route),
        )
        .route(
            "/v1/backend/rooms/{room_id}/members/{participant_id}/avatar",
            get(get_member_avatar_route),
        )
        .route("/v1/backend/profile", get(get_self_profile_route))
}

/// GET /v1/backend/rooms/{room_id}/members：requester（该 Agent）必须对该
/// Room 存在 active Membership 周期——复用 `RoomStore::get_room_members` 的
/// 同一可见性裁决；投影与 Human 侧 `GET /v1/rooms/{room_id}/members` 逐字段
/// 相同。未知 / bad-id / 非 active 成员统一 404 `room_not_found`。
async fn list_members_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    match get_room_members(state.room.as_ref(), &auth.view.agent.id, &room_id).await {
        Ok(members) => Json(GetRoomMembersResponse {
            members: members.iter().map(member_item_view).collect(),
            request_id,
        })
        .into_response(),
        Err(e) => query_error_response(e, &request_id),
    }
}

/// GET /v1/backend/rooms/{room_id}/member-presentations：active 成员
/// Presentation 摘要列表（preview 上限 512 UTF-8 bytes，同 Human 侧）。
async fn list_member_presentations_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    match get_member_presentations(
        state.room.as_ref(),
        state.presentation.as_ref(),
        &auth.view.agent.id,
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

/// GET /v1/backend/rooms/{room_id}/members/{participant_id}/profile：requester
/// 与 target 都必须当前 active 成员；返回 target 的最小公共资料、全局
/// Profile（无版本为 null）与当前 Room Presentation。
async fn get_member_profile_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path((room_id, participant_id)): Path<(String, String)>,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    match get_member_profile(
        state.room.as_ref(),
        state.presentation.as_ref(),
        &auth.view.agent.id,
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

/// GET /v1/backend/rooms/{room_id}/members/{participant_id}/presentation：
/// requester 与 target 都必须当前 active 成员；direct / human_direct 无
/// Presentation（返回 null）。
async fn get_member_presentation_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path((room_id, participant_id)): Path<(String, String)>,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    match get_member_presentation(
        state.room.as_ref(),
        state.presentation.as_ref(),
        &auth.view.agent.id,
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

/// GET /v1/backend/rooms/{room_id}/members/{participant_id}/avatar：requester
/// 与 target 都必须当前 active 成员（与成员详情同一裁决）；无头像统一 404
/// `avatar_not_found`。响应头与 Human 侧共用同一构造（content_type +
/// nosniff + ETag + private 缓存）。
async fn get_member_avatar_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path((room_id, participant_id)): Path<(String, String)>,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    match get_member_avatar_bytes(
        state.room.as_ref(),
        state.presentation.as_ref(),
        state.blob.as_ref(),
        &auth.view.agent.id,
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

/// GET /v1/backend/profile：Agent 读**自己**的最新公共 Profile（无版本为
/// null）。身份只来自 BackendAuth；无路径参数，从形状上排除读别人的。
async fn get_self_profile_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    match get_self_profile(state.presentation.as_ref(), &auth.view.agent.id).await {
        Ok(profile) => Json(GetSelfProfileResponse {
            profile: profile.as_ref().map(profile_view),
            request_id,
        })
        .into_response(),
        Err(e) => store_error_response("agent.read_surface.self_profile", &e, &request_id),
    }
}
