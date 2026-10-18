//! `POST /v1/backend/rooms/{room_id}/leave`：Agent 主动退出一个房间
//! （M2 窄解冻切片，2026-09-27 主人批准；口径「能退，规则统一」）。
//!
//! **与 Human 侧 `POST /v1/rooms/{room_id}/leave` 逐条同形**：同一条
//! application 用例（`leave_membership`）、同一个状态码（204）、同一套错误面
//! （404 `room_not_found` / 409 `owner_transfer_required`），只换认证面
//! （BackendAuth）。这是本仓库既有的分叉纪律：**领域层与应用层 kind 无关，
//! 只在适配器分叉**（见 `AGENTS.md` 与 `CORE_V1_AGENT_LEAVE.md`）。
//!
//! **一条规则**：非 owner 的成员周期可以由**它自己**以 `closed_by='leave'`
//! 关闭——不分房 kind、不分参与者 kind。例外一处：`human_direct` 房的出口归
//! 关系模型（`contact_ended` + 精确见证），因此那里一律 404。
//!
//! **事件面零改动**：`member.left` 事实由既有触发器
//! `human_event_facts_member_left` 生成——它本来就与参与者 kind 无关
//! （snapshot 里带 `p.kind`），白名单里也本来就有 `leave`。这里只照 Human 侧
//! 同一 best-effort 纪律做 post-commit 唤醒（失败只记录脱敏 warn，不影响
//! 已提交响应）。**不唤醒 Agent**：成员变更不是 Agent Event。

use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;

use crate::adapters::http::AppState;
use crate::adapters::http::backend_auth::BackendAuth;
use crate::adapters::http::human_event_routes::wake_humans;
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::routes::{error_response, store_error_response};
use crate::application::room::{LeaveMembershipError, leave_membership};
use crate::domain::human_event::{HumanEventKind, HumanEventSource};
use crate::protocol::error::codes;

/// Agent 退房路由：一条写端点（BackendAuth）。无请求体、无 query、**无幂等键**
/// （与 Human 侧同：重复退出第二次是 404——周期已关闭，不再是成员）。
pub fn router() -> Router<AppState> {
    Router::new().route("/v1/backend/rooms/{room_id}/leave", post(leave_route))
}

/// `POST /v1/backend/rooms/{room_id}/leave`：该 Agent 自己的 active 成员周期
/// 进入 closed 终态（`closed_by='leave'`），成功 204 无体；此后对该房的一切
/// 读写 404。owner 必须先转让（409）；unknown / bad-id / `human_direct` /
/// 无 active 周期统一 404。
async fn leave_route(
    State(state): State<AppState>,
    auth: BackendAuth,
    Path(room_id): Path<String>,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    match leave_membership(state.room.as_ref(), &auth.view.agent.id, &room_id).await {
        Ok(closed_membership_id) => {
            // post-commit 唤醒：member.left fact 的完整 source 身份。
            wake_humans(
                &state,
                &[HumanEventSource::new(
                    HumanEventKind::MemberLeft,
                    closed_membership_id.as_str().to_owned(),
                )],
                &request_id,
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(LeaveMembershipError::RoomNotFound) => error_response(
            StatusCode::NOT_FOUND,
            codes::ROOM_NOT_FOUND,
            "room not found",
            &request_id,
        ),
        Err(LeaveMembershipError::OwnerTransferRequired) => error_response(
            StatusCode::CONFLICT,
            codes::OWNER_TRANSFER_REQUIRED,
            "the group owner must transfer ownership before leaving",
            &request_id,
        ),
        Err(LeaveMembershipError::Store(e)) => {
            store_error_response("backend.leave", &e, &request_id)
        }
    }
}
