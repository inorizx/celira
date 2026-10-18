//! 存储就绪状态探测端口。

use crate::ports::storage::StorageError;

/// SQLite 迁移状态快照。
///
/// 由存储 adapter 提供；应用层只消费这个结果，不接触数据库实现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrationStatus {
    /// 尚未应用的迁移数量。
    pub pending: u64,
    /// 元数据表中标记失败的迁移数量（`_sqlx_migrations.success = 0`）。
    pub failed: u64,
}

impl MigrationStatus {
    /// 全部迁移已应用且无失败记录。
    pub fn is_clean(&self) -> bool {
        self.pending == 0 && self.failed == 0
    }
}

/// 迁移与数据库连通性探测接口。
///
/// 探测必须证明两件事：(1) SQLite 可连接可查询；(2) migration 元数据可验证
/// （无 pending、无 failed 记录）。失败返回 typed 基础设施错误：连接/IO
/// 类归 `ConnectionOrIo`，checksum/结构不一致归 `DecodeOrCorrupt`，未分类
/// 归 `Unknown`；adapter 负责分类，应用层与 HTTP 不接触具体数据库类型。
#[async_trait::async_trait]
pub trait MigrationProbe: Send + Sync {
    async fn status(&self) -> Result<MigrationStatus, StorageError>;
}
