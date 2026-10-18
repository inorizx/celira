//! readiness 用例：验证数据库可连接且迁移已应用。

use crate::ports::migration::MigrationProbe;
use crate::ports::storage::StorageError;

/// readiness 检查结果。
#[derive(Debug)]
pub enum ReadyError {
    /// 迁移未全部应用或存在失败记录。
    NotReady { pending: u64, failed: u64 },
    /// 数据库不可连接或元数据不可查询（typed 基础设施错误；HTTP 层只使用
    /// 固定安全文案，详细原因保留在 cause 链与结构化日志）。
    ProbeFailed(StorageError),
}

impl std::fmt::Display for ReadyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotReady { pending, failed } => {
                write!(f, "service not ready: {pending} pending, {failed} failed")
            }
            Self::ProbeFailed(e) => write!(f, "storage probe failed: {e}"),
        }
    }
}

impl std::error::Error for ReadyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ProbeFailed(e) => Some(e),
            _ => None,
        }
    }
}

/// 就绪检查：只有 SQLite 可连接、无 pending 迁移、无失败迁移才返回 `Ok`。
///
/// HTTP adapter 通过 `MigrationProbe` 端口调用本用例，不直接操作 SQLite。
pub async fn check_ready(probe: &dyn MigrationProbe) -> Result<(), ReadyError> {
    let status = probe.status().await.map_err(ReadyError::ProbeFailed)?;
    if status.is_clean() {
        Ok(())
    } else {
        Err(ReadyError::NotReady {
            pending: status.pending,
            failed: status.failed,
        })
    }
}
