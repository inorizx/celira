//! 用例、授权编排和事务边界。

use crate::ports::storage::{StorageError, StorageFailureKind};

/// 未预期的 Store 业务变体 → `Unknown` 基础设施错误（HTTP 500
/// `internal_error`）；变体的稳定 Display 只作为内部诊断消息，不进入公开
/// 文案，typed cause 链仍可通过 `StorageError::cause()` 下钻。
pub(crate) fn unexpected_store_error(
    context: &'static str,
    other: impl std::fmt::Display,
) -> StorageError {
    StorageError::from_message(
        StorageFailureKind::Unknown,
        format!("unexpected {context}: {other}"),
    )
}

pub mod agent;
pub mod attachment;
pub mod backend;
pub mod call;
pub mod contact;
pub mod directory;
pub mod event;
pub mod home;
pub mod human_event;
pub mod identity;
pub mod live;
pub mod message;
pub mod participant_avatar;
pub mod participant_presentation;
pub mod readiness;
pub mod room;
