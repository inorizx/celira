//! Agent 侧的表达权面（批次四）：**一支 Agent 写自己的资料与头像**。
//!
//! - `PUT /v1/backend/me/profile`（表达权第一刀）：写自己的介绍；
//! - `PUT|DELETE /v1/backend/me/avatar`（表达权第二刀）：写 / 摘除自己的头像。
//!
//! **为什么必须有它**：在此之前，一支 Agent 对自己的介绍**只能由 Owner 代笔**
//! （契约原文：「actor 必须是目标 Agent 的 active Agent Owner……Agent 自己、
//! Backend/Connector/Runtime 都不得写」）。表达权归主体：自己的资料与头像只有自己
//! （或替它担责的 Owner）能写——ownership 是责任关系，不是代笔权。
//!
//! **形状与批次三第③步同一条**：**并集**（主人裁决「像飞书 QQ 那种，人类用户把它
//! 带进来」）——Owner 的写权限**一字不动**，只是多了"它自己"这一支。
//!
//! **身份只来自认证**：路径里**没有** agent_id（与读取面同一条纪律）——一支 Agent
//! 只能写自己的东西，不能借路径去写别人的。
//!
//! **语义与 Human 侧逐条相同**（同一个 application 用例、同一段响应构造、同一套
//! 错误码）：必带 `Idempotency-Key` 与 `base_version`（并发裁决在授权之后）；
//! 每次写入追加新版本，绝不改写既有行；头像 PNG/JPEG/WebP ≤2 MiB、服务端不裁切。
//!
//! **本刀不做**：房呈现（Group Room Presentation，房治理面，actor 仍限该房 active
//! owner）。（原文还列了「Publication / Service 层」——那一层已于 2026-09-28
//! 整体退役。）

use axum::Json;
use axum::Router;
use axum::extract::Multipart;
use axum::extract::Path;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::put;

use crate::adapters::http::AppState;
use crate::adapters::http::backend_auth::{BackendAuth, backend_agent_actor};
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::participant_avatar_routes::{
    delete_avatar_response, put_avatar_response,
};
use crate::adapters::http::participant_presentation_routes::{
    put_profile_response, put_room_presentation_response,
};
use crate::protocol::participant_avatar::DeleteAvatarRequest;
use crate::protocol::participant_presentation::{PutPresentationRequest, PutProfileRequest};

/// Agent 侧表达权路由（四条，全部 BackendAuth）。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/backend/me/profile", put(put_own_profile_route))
        .route(
            "/v1/backend/me/avatar",
            put(put_own_avatar_route).delete(delete_own_avatar_route),
        )
        // 房呈现：**写自己在某一间群房里的那一份**（身份只来自认证）。
        .route(
            "/v1/backend/rooms/{room_id}/my-presentation",
            put(put_my_presentation_route),
        )
}

/// `PUT /v1/backend/me/profile`：写**我自己**的资料。
async fn put_own_profile_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<PutProfileRequest>, JsonRejection>,
) -> Response {
    let actor = backend_agent_actor(&auth.view.agent);
    let target = auth.view.agent.id.as_str().to_owned();
    put_profile_response(&state, &actor, &target, &request_id.0, &headers, body).await
}

/// `PUT /v1/backend/me/avatar`：写**我自己**的头像（multipart，单步）。
async fn put_own_avatar_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    request_id: RequestId,
    headers: HeaderMap,
    multipart: Multipart,
) -> Response {
    let actor = backend_agent_actor(&auth.view.agent);
    let target = auth.view.agent.id.as_str().to_owned();
    put_avatar_response(&state, &actor, &target, &request_id.0, &headers, multipart).await
}

/// `DELETE /v1/backend/me/avatar`：摘除**我自己**的头像（新版本 avatar = null）。
async fn delete_own_avatar_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<DeleteAvatarRequest>, JsonRejection>,
) -> Response {
    let actor = backend_agent_actor(&auth.view.agent);
    let target = auth.view.agent.id.as_str().to_owned();
    delete_avatar_response(&state, &actor, &target, &request_id.0, &headers, body).await
}

/// `PUT /v1/backend/rooms/{room_id}/my-presentation`：写**我自己**在这间群房里的呈现。
///
/// **身份只来自认证**：路径里只有房间，没有"写谁的"——写的永远是发起者自己
/// （它必须是该房的 active Agent 成员；房 owner 的写权限一字不动，见
/// `put_room_presentation_response`）。
async fn put_my_presentation_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    request_id: RequestId,
    headers: HeaderMap,
    Path(room_id): Path<String>,
    body: Result<Json<PutPresentationRequest>, JsonRejection>,
) -> Response {
    let actor = backend_agent_actor(&auth.view.agent);
    let target = auth.view.agent.id.as_str().to_owned();
    put_room_presentation_response(
        &state,
        &actor,
        &room_id,
        &target,
        &request_id.0,
        &headers,
        body,
    )
    .await
}
