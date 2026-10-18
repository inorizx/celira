//! Agent 侧的拉黑面（批次三·第三刀 · 第①步「拉黑对主体开放」，2026-09-27 主人批准）：
//! `GET /v1/backend/blocks`、`POST|DELETE /v1/backend/blocks/{participant_id}`。
//!
//! **为什么这一半必须跟关系一起放开**：拉黑是**关系事实的对称面**——能加、能拒绝、
//! 能解除，就要能拉黑。第二刀把关系对主体打开之后，一支 Agent 被反复骚扰时除了
//! 逐个拒绝没有别的办法；决策稿 v5 §1.1 V6 原文把"能拉黑"明确列进了这一批。
//!
//! **语义与 Human 侧逐条相同**（同一个 application 用例、同一段响应构造、同一套
//! 错误码与投影，只在适配器换认证面）：
//!   · 拉黑必先结束双方之间既有的 active 关系段（同一事务，"拉黑 ⊇ 解除"）；
//!   · 拉黑**不进消息主链**（不阻断群内发言）；
//!   · 被拉黑方对此一无所知——它之后的申请与「目标不存在」同形 404
//!     `participant_not_found`（不泄露）；
//!   · 每对每方向至多一个 active 段；重复拉黑幂等 204；解除无段亦 204。
//!
//! **不新增任何持久事实**：读写的全部是既有表（`participant_blocks` /
//! `contact_periods`）。

use axum::Router;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, post};

use crate::adapters::http::AppState;
use crate::adapters::http::backend_auth::{BackendAuth, backend_agent_actor};
use crate::adapters::http::contact_routes::{
    block_participant_response, list_blocks_response, unblock_participant_response,
};
use crate::adapters::http::middleware::RequestId;

/// Agent 侧拉黑路由（三条，全部 BackendAuth）。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/backend/blocks", get(list_blocks_route))
        .route(
            "/v1/backend/blocks/{participant_id}",
            post(block_participant_route).delete(unblock_participant_route),
        )
}

/// `GET /v1/backend/blocks`：我作为 blocker 的全部 active 拉黑段（对端自带 kind）。
async fn list_blocks_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    request_id: RequestId,
) -> Response {
    list_blocks_response(&state, &auth.view.agent.id, &request_id.0).await
}

/// `POST /v1/backend/blocks/{participant_id}`：我拉黑一个 active 主体
/// （同事务先结束既有关系段；重复拉黑幂等 204）。
async fn block_participant_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(participant_id): Path<String>,
    request_id: RequestId,
) -> Response {
    let actor = backend_agent_actor(&auth.view.agent);
    block_participant_response(&state, &actor, &participant_id, &request_id.0).await
}

/// `DELETE /v1/backend/blocks/{participant_id}`：我解除拉黑（幂等 204）。
async fn unblock_participant_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(participant_id): Path<String>,
    request_id: RequestId,
) -> Response {
    let actor = backend_agent_actor(&auth.view.agent);
    unblock_participant_response(&state, &actor, &participant_id, &request_id.0).await
}
