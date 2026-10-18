//! 人写自己的资料（表达权第四刀，2026-10-17 主人批准）。
//!
//! 四条端点，**全部 HumanAuth**，身份只来自认证（路径里没有"写谁的"——从形状上
//! 排除"改别人"）：
//!
//! | 端点 | 语义 |
//! |---|---|
//! | `GET /v1/me/profile` | 读自己：名字（身份事实）＋ 呈现（头像引用 ＋ 版本 ＋ 署名） |
//! | `PUT /v1/me/profile` | 写自己的名字 —— **状态写**（无版本行、无幂等键） |
//! | `PUT /v1/me/profile/avatar` | 写自己的头像（multipart 单步，版本链） |
//! | `DELETE /v1/me/profile/avatar` | 摘除自己的头像（新版本 `avatar=null`） |
//!
//! ## 为什么名字与头像不是同一种写
//!
//! 名字是**既有 Participant 身份事实**（`participants.display_name`；呈现切片明文
//! "不复制 `display_name`/`status`/`kind`"）⇒ 它不在版本链上，**状态写**就够
//! （同值重写天然幂等，照 `PUT /v1/backend/rooms/{id}/read-cursor` 的先例）。
//! 头像是**呈现版本链**上的一格 ⇒ 走 `Idempotency-Key` ＋ `base_version` 那一套，
//! 与 Agent 侧逐条相同（复用同一段响应构造与同一套错误码）。
//!
//! ## 归属（三问定的，见 `outputs/HUMAN_PROFILE_RULING_DRAFT_2026-10-17.md`）
//!
//! 换成微信/QQ 还成立吗 ⇒ 归**世界**；别人要不要看见 ⇒ 世界必须持有一份权威；
//! 是不是"谁说的" ⇒ 归**主体**且必须署名。⇒ 客户端只放凭据与缓存，
//! **换世界不迁移**（那是另一条契约，本刀不碰）。

use axum::Json;
use axum::Router;
use axum::extract::Multipart;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use crate::adapters::http::AppState;
use crate::adapters::http::auth::CurrentAuth;
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::participant_avatar_routes::{
    delete_avatar_response, put_avatar_response,
};
use crate::adapters::http::participant_presentation_routes::profile_view;
use crate::adapters::http::routes::{error_response, store_error_response};
use crate::application::identity::{RenameError, rename_human};
use crate::application::participant_presentation::get_self_profile;
use crate::protocol::error::codes;
use crate::protocol::human_profile::{
    GetHumanProfileResponse, PutHumanProfileRequest, PutHumanProfileResponse,
};

/// Human 侧的表达权路由（四条，全部 HumanAuth）。
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/v1/me/profile",
            get(get_own_profile_route).put(put_own_profile_route),
        )
        .route(
            "/v1/me/profile/avatar",
            axum::routing::put(put_own_avatar_route).delete(delete_own_avatar_route),
        )
}

/// `GET /v1/me/profile`：读**我自己**的名字与呈现。
///
/// 名字来自认证上下文（会话指向的那一行），呈现来自版本链 ⇒ 两件事**并排**给出来，
/// 不做"合起来的 profile 对象"（它们是两种事实，见文件头）。
async fn get_own_profile_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    match get_self_profile(state.presentation.as_ref(), &auth.participant.id).await {
        Ok(profile) => Json(GetHumanProfileResponse {
            display_name: auth.participant.display_name.as_str().to_owned(),
            profile: profile.as_ref().map(profile_view),
            request_id,
        })
        .into_response(),
        Err(e) => store_error_response("me.profile.read", &e, &request_id),
    }
}

/// `PUT /v1/me/profile`：改**我自己**的名字。
///
/// **状态写**：只动 `participants.display_name` 一列 —— 没有版本行、没有幂等记录
/// （同值重写天然幂等）。校验在用例里（`DisplayName::parse`，与兑换时同一套）。
async fn put_own_profile_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
    body: Result<Json<PutHumanProfileRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    let Json(payload) = match body {
        Ok(body) => body,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "invalid JSON request body (only `display_name` is accepted)",
                &request_id,
            );
        }
    };
    match rename_human(
        state.identity.as_ref(),
        &auth.participant.id,
        &payload.display_name,
    )
    .await
    {
        Ok(display_name) => Json(PutHumanProfileResponse {
            display_name: display_name.as_str().to_owned(),
            request_id,
        })
        .into_response(),
        Err(RenameError::InvalidName(_)) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "display_name must be 1..=80 characters, non-blank, and free of NUL",
            &request_id,
        ),
        // 自己写自己时这只可能是"我已被撤销"（认证已过）—— 与既有非 active 口径一致。
        Err(RenameError::NotActiveHuman) => error_response(
            StatusCode::FORBIDDEN,
            codes::INVALID_SESSION,
            "human participant is not active",
            &request_id,
        ),
        Err(RenameError::Store(e)) => store_error_response("me.profile.rename", &e, &request_id),
    }
}

/// `PUT /v1/me/profile/avatar`：写**我自己**的头像（multipart，单步）。
///
/// 与 Agent 侧**复用同一段实现**（`put_avatar_response`）：白名单 PNG/JPEG/WebP、
/// ≤2 MiB、字节与 key 与 base_version 同一事务、Core 不裁切不转码。
async fn put_own_avatar_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
    headers: HeaderMap,
    multipart: Multipart,
) -> Response {
    let target = auth.participant.id.as_str().to_owned();
    put_avatar_response(
        &state,
        &auth.participant,
        &target,
        &request_id.0,
        &headers,
        multipart,
    )
    .await
}

/// `DELETE /v1/me/profile/avatar`：摘除**我自己**的头像（新版本 `avatar=null`）。
async fn delete_own_avatar_route(
    State(state): State<AppState>,
    auth: CurrentAuth,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<crate::protocol::participant_avatar::DeleteAvatarRequest>, JsonRejection>,
) -> Response {
    let target = auth.participant.id.as_str().to_owned();
    delete_avatar_response(
        &state,
        &auth.participant,
        &target,
        &request_id.0,
        &headers,
        body,
    )
    .await
}
