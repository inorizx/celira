//! Agent 侧的**房治理面**（批次三·第一刀「owner = 创建者」，2026-09-27 主人批准）：
//! `POST /v1/backend/groups`、`POST /v1/backend/rooms/{room_id}/members`、
//! `DELETE /v1/backend/rooms/{room_id}/members/{participant_id}`、
//! `POST /v1/backend/rooms/{room_id}/owner-transfer`。
//!
//! **为什么这四条要一起存在**：owner 是**房内的治理角色**（`role='owner'`），不是参与者
//! 种类（`kind`）——所以一支 Agent 也能建房、也能持有 owner。但"owner 可以是一支 Agent"
//! 若只有数据库里那一行事实，它就既管不了成员、也**永远退不出自己的房**（M2 的规则：
//! owner 退房必须先转让）。四条端点让这个角色**可达**且**可退出**。
//!
//! **与 Human 侧逐条同形**：同一个 application 用例（`create_group` /
//! `invite_group_member` / `remove_group_member` / `transfer_group_owner`）、同一段响应
//! 构造与唤醒（`room_routes::*_response`）、同一套参数解析（
//! `room_routes::parse_body_and_key_shared`：body 上限 / 未知字段 / 幂等键同码）、同一套
//! 错误码与状态码；**只换认证面**（`BackendAuth`）与 actor 的来源
//! （`backend_agent_actor`）。这是本仓库既有的分叉纪律：**领域层与应用层 kind 无关，
//! 只在适配器分叉**（见 `AGENTS.md`、`CORE_V1_AGENT_LEAVE.md`）。
//!
//! **邀请资格是一条判据**（不分"邀请者是不是 Human"）：目标要与邀请者有 accepted 联系人
//! 段，或该主体被邀请者以 active Agent Owner 拥有。今天一支 Agent owner 两条都取不到
//! ——因为**它名下没有那两类关系事实**（联系人只存在于 Human–Human、ownership 只存在于
//! Human→Agent），不是因为另立了一条禁令；本批第二刀把关系泛化之后，同一个判据自动生效。
//!
//! **不新增任何持久事实**：四条端点写入的全部是既有表（rooms / room_coverage /
//! room_memberships / group_*_idempotency / group_owner_transitions）。

use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{delete, post};

use crate::adapters::http::AppState;
use crate::adapters::http::backend_auth::{BackendAuth, backend_agent_actor};
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::room_routes::{
    create_group_response, invite_member_response, parse_body_and_key_shared,
    remove_member_response, transfer_owner_response,
};
use crate::protocol::room::{CreateGroupRequest, InviteMemberRequest, TransferOwnerRequest};

/// Agent 侧的房治理路由（四条写端点，全部 BackendAuth）。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/backend/groups", post(create_group_route))
        .route(
            "/v1/backend/rooms/{room_id}/members",
            post(invite_member_route),
        )
        .route(
            "/v1/backend/rooms/{room_id}/members/{participant_id}",
            delete(remove_member_route),
        )
        .route(
            "/v1/backend/rooms/{room_id}/owner-transfer",
            post(transfer_owner_route),
        )
}

/// `POST /v1/backend/groups`：Agent 建群，**它自己成为 owner**（首次 201；同
/// Agent+key+同群名重放 200；异群名 409 `idempotency_conflict`）。
async fn create_group_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<CreateGroupRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    let (Json(payload), key) = match parse_body_and_key_shared(body, &headers, &request_id).await {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    let actor = backend_agent_actor(&auth.view.agent);
    create_group_response(&state, &actor, &payload.name, &key, &request_id).await
}

/// `POST /v1/backend/rooms/{room_id}/members`：owner 邀请成员（必带
/// Idempotency-Key，先查 key 再查权限）。资格判据与 Human 侧**同一条**。
async fn invite_member_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<InviteMemberRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    let (Json(payload), key) = match parse_body_and_key_shared(body, &headers, &request_id).await {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    let actor = backend_agent_actor(&auth.view.agent);
    invite_member_response(
        &state,
        &actor,
        &room_id,
        &payload.participant_id,
        &key,
        &request_id,
    )
    .await
}

/// `DELETE /v1/backend/rooms/{room_id}/members/{participant_id}`：owner 移除
/// active 成员（204）；owner 自身不可移除（409 `owner_not_removable`）。
/// **不分参与者 kind**：Agent owner 也能移除 Human 成员（2026-09-27 主人裁决）。
async fn remove_member_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path((room_id, participant_id)): Path<(String, String)>,
    request_id: RequestId,
) -> Response {
    let actor = backend_agent_actor(&auth.view.agent);
    remove_member_response(&state, &actor, &room_id, &participant_id, &request_id.0).await
}

/// `POST /v1/backend/rooms/{room_id}/owner-transfer`：owner 原子转让给**任意
/// active 成员**（含 Agent）；200 返回新 owner 的周期视图，同 key 重放
/// `idempotency_replayed=true`。目标不是本房 active member ⇒ 404；目标是转让者
/// 自己 ⇒ 400 `invalid_transfer_target`。
async fn transfer_owner_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
    headers: HeaderMap,
    body: Result<Json<TransferOwnerRequest>, JsonRejection>,
) -> Response {
    let request_id = request_id.0;
    let (Json(payload), key) = match parse_body_and_key_shared(body, &headers, &request_id).await {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    let actor = backend_agent_actor(&auth.view.agent);
    transfer_owner_response(
        &state,
        &actor,
        &room_id,
        &payload.participant_id,
        &key,
        &request_id,
    )
    .await
}
