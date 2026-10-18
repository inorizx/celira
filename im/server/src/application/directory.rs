//! Agent 通讯录（Directory）读取用例（切片 B1，CORE_V1_AGENT_DIRECTORY.md）。
//!
//! 用例层只做参数裁决与端口编排：把 `since` 解析为 unix ms 基线（负数 /
//! 非数字 → `InvalidRequest`，HTTP 400），再调用 `RoomStore` 的纯读投影。
//! 零写语义：本用例不产生任何持久对象、不落幂等事实、不生成 Event。

use crate::domain::participant::ParticipantId;
use crate::ports::room::{BackendDirectoryRoom, RoomStore, RoomStoreError};
use crate::ports::storage::StorageError;

/// 通讯录查询失败。
#[derive(Debug)]
pub enum DirectoryQueryError {
    /// `since` 非法（非数字 / 负数 / 溢出）：对外 400 `invalid_request`。
    InvalidRequest,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for DirectoryQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest => write!(f, "invalid since parameter"),
            Self::Store(e) => write!(f, "directory store failure: {e}"),
        }
    }
}

impl std::error::Error for DirectoryQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// 通讯录查询只在 active member 集合上做纯读投影：业务变体（NotFound 等）
/// 理论不可达；逃逸即内部不变量破坏 → Unknown 存储失败（对外 500）。
fn map_store_error(e: RoomStoreError) -> DirectoryQueryError {
    DirectoryQueryError::Store(crate::application::unexpected_store_error(
        "room directory store error",
        e,
    ))
}

/// 读取该 agent 的通讯录目录（BackendAuth 已保证 agent 身份与 active
/// 状态）：`since` 为 unix ms 增量基线（`None` = 无基线，`since_count`
/// 恒为 0）；负数 / 溢出 / 非数字 → `InvalidRequest`（HTTP 400）。
pub async fn list_backend_rooms(
    store: &dyn RoomStore,
    agent_id: &ParticipantId,
    since: Option<i64>,
) -> Result<Vec<BackendDirectoryRoom>, DirectoryQueryError> {
    let since = match since {
        None => None,
        // 负数/越界时间戳一律 400（unix ms 必为非负）。
        Some(ms) if ms < 0 => return Err(DirectoryQueryError::InvalidRequest),
        Some(ms) => Some(ms),
    };
    store
        .list_backend_rooms(agent_id, since)
        .await
        .map_err(map_store_error)
}
