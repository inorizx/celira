//! Agent 通讯录（Directory）HTTP handler（契约 CORE_V1_AGENT_DIRECTORY.md，
//! 切片 B1「看见自己的世界」）。
//!
//! `GET /v1/backend/rooms`：**纯读**端点——认证同四端点 BackendAuth
//! （`Authorization: Bearer <BackendToken>`；401 语义与四端点完全一致）；
//! 可见性 = 该 agent 为 active member 的房间；按机械最近活动时间倒序；
//! 可选 `since` 参数（unix ms）返回每房自该时间以来的消息增量计数。
//! 零写语义、零持久对象新增、零既有端点变更；预览与计数从既有公共
//! Message 事实推导。GET 只读、无 Idempotency-Key 要求。
//!
//! 切片 M1（`CORE_V1_AGENT_READ_CURSOR.md`）给每行加了 `read_state`：该
//! agent 在这个房间的读位置与未读——**同一个读面加一个字段**，不是新端点；
//! 投影复用 Human 侧 `read_state_view`，不新增判据。

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use crate::adapters::http::AppState;
use crate::adapters::http::backend_auth::BackendAuth;
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::room_routes::read_state_view;
use crate::adapters::http::routes::error_response;
use crate::application::directory::{self, DirectoryQueryError};
use crate::protocol::directory::{BackendRoomView, BackendRoomsResponse, DirectoryQuery};
use crate::protocol::error::codes;

/// 通讯录路由：纯读列表端点。
pub fn router() -> Router<AppState> {
    Router::new().route("/v1/backend/rooms", get(list_rooms))
}

/// GET /v1/backend/rooms：返回该 agent（BackendAuth 认证）为 active member
/// 的全部房间目录。`since` 可选 unix ms 基线；非法（非数字 / 负数 / 溢出）
/// → 400 `invalid_request`；认证失败 → 401（同四端点）。列表端点无资源
/// 路径，不存在 404 分支；存储失败按既有安全错误边界。
async fn list_rooms(
    State(state): State<AppState>,
    auth: BackendAuth,
    query: Result<Query<DirectoryQuery>, axum::extract::rejection::QueryRejection>,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    let Query(query) = match query {
        Ok(query) => query,
        Err(_) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "invalid since parameter",
                &request_id,
            );
        }
    };
    match directory::list_backend_rooms(state.room.as_ref(), &auth.view.agent.id, query.since).await
    {
        Ok(rooms) => Json(BackendRoomsResponse {
            rooms: rooms.iter().map(directory_view).collect(),
            request_id,
        })
        .into_response(),
        Err(e) => query_error_response(e, &request_id),
    }
}

/// 单房目录行 → 公共视图投影（最近活动时间 RFC3339；预览/计数原样；
/// read state 复用 Human 侧同一投影函数）。
fn directory_view(room: &crate::ports::room::BackendDirectoryRoom) -> BackendRoomView {
    BackendRoomView {
        room_id: room.room_id.as_str().to_owned(),
        kind: room.kind.as_str().to_owned(),
        display_name: room.display_name.clone(),
        last_activity_at: room.last_activity_at.map(|t| t.to_rfc3339()),
        preview: room.preview.clone(),
        // COUNT(*) 投影恒非负；负值（理论不可达）防御性收敛为 0，绝不
        // 把 i64 负数直接转 u64（回绕成天文数字）。
        since_count: room.since_count.max(0) as u64,
        read_state: read_state_view(&room.read_state),
        // 与 Human 侧**同一个投影函数**（人机同形，契约 CORE_V1_READ_RECEIPT.md §2）。
        peer_read: crate::adapters::http::room_routes::peer_read_view(room.peer_read),
        // 提醒档：与 Human 侧房间列表**同一格、同一投影**
        // （契约 `CORE_V1_IM_CONTRACT.md` §9.1 / 本文档 §3.2）。
        attention: room.attention.as_str().to_owned(),
    }
}

/// 查询失败映射：`since` 非法 → 400；存储按既有安全错误边界。
fn query_error_response(e: DirectoryQueryError, request_id: &str) -> Response {
    match e {
        DirectoryQueryError::InvalidRequest => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "invalid since parameter",
            request_id,
        ),
        DirectoryQueryError::Store(e) => crate::adapters::http::routes::store_error_response(
            "room.directory.list",
            &e,
            request_id,
        ),
    }
}
