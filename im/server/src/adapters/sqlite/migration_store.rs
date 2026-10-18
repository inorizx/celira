//! SQLite 连接与迁移。
//!
//! 本模块是 Core 唯一允许接触 SQLite 文件的地方。HTTP handler 和业务用例
//! 不得直接操作数据库，只能通过 `ports::MigrationProbe` 探测状态。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::ports::migration::{MigrationProbe, MigrationStatus};
use crate::ports::storage::{StorageError, StorageFailureKind};

/// 编译期内嵌的 migration 集合（来自 `migrations/` 目录）。
///
/// SQLx Migrator 维护 `_sqlx_migrations` 元数据表（version、description、
/// checksum、success），重复执行是幂等的；checksum 不一致会直接报错。
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// 数据库打开与迁移失败。
#[derive(Debug)]
pub enum DbError {
    CreateParentDir(std::io::Error),
    Connect(sqlx::Error),
    Migrate(sqlx::migrate::MigrateError),
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbError::CreateParentDir(e) => {
                write!(f, "failed to create database parent directory: {e}")
            }
            DbError::Connect(e) => write!(f, "failed to connect to SQLite: {e}"),
            DbError::Migrate(e) => write!(f, "failed to apply migrations: {e}"),
        }
    }
}

impl std::error::Error for DbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DbError::CreateParentDir(e) => Some(e),
            DbError::Connect(e) => Some(e),
            DbError::Migrate(e) => Some(e),
        }
    }
}

/// 已迁移的 SQLite 连接池。
#[derive(Clone, Debug)]
pub struct MigrationStore {
    pool: SqlitePool,
    db_path: PathBuf,
}

impl MigrationStore {
    /// 打开（必要时创建）数据库并幂等应用全部 migration。
    ///
    /// 任何一步失败都以 `Err` 返回，调用方（composition root）负责以非零
    /// 退出码终止进程。
    pub async fn open(db_path: &Path) -> Result<Self, DbError> {
        if let Some(parent) = db_path.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(DbError::CreateParentDir)?;
        }

        let options = SqliteConnectOptions::new()
            .filename(db_path)
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .map_err(DbError::Connect)?;

        MIGRATOR.run(&pool).await.map_err(DbError::Migrate)?;

        Ok(Self {
            pool,
            db_path: db_path.to_path_buf(),
        })
    }

    /// 供就绪探测与同 crate 身份 adapter 复用连接池。
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

#[async_trait::async_trait]
impl MigrationProbe for MigrationStore {
    /// 返回 pending / failed 迁移数量。
    ///
    /// 探测路径必须证明：(1) 数据库文件真实存在；(2) SQLite 可连接可查询
    /// （acquire + 查询）；(3) migration 元数据可验证（`_sqlx_migrations` 表可
    /// 创建可读取，已应用版本的集合与编译期内嵌集合一致、checksum 一致，
    /// 无 success=0 失败记录）。
    ///
    /// 失败按稳定类别返回 typed 基础设施错误（CORE_V1_ERROR_HANDLING.md
    /// §7.1）：文件/连接/IO 类归 `ConnectionOrIo`，checksum/版本结构不一致
    /// 归 `DecodeOrCorrupt`；SQLx 底层错误由 adapter 统一分类。原因只进
    /// typed cause 链与脱敏日志，不进入公开响应。
    async fn status(&self) -> Result<MigrationStatus, StorageError> {
        use sqlx::migrate::Migrate;

        // 连接池持有的文件句柄在数据库文件被 unlink 后仍可能正常工作，
        // 因此存在性必须以文件系统为准，而不是查询是否成功。
        if !tokio::fs::metadata(&self.db_path)
            .await
            .is_ok_and(|meta| meta.is_file())
        {
            return Err(StorageError::from_message(
                StorageFailureKind::ConnectionOrIo,
                format!("database file missing at {}", self.db_path.display()),
            ));
        }

        let mut conn = self.pool.acquire().await.map_err(StorageError::from)?;
        conn.ensure_migrations_table()
            .await
            .map_err(|e| StorageError::new(StorageFailureKind::DecodeOrCorrupt, e))?;

        let applied = conn
            .list_applied_migrations()
            .await
            .map_err(|e| StorageError::new(StorageFailureKind::DecodeOrCorrupt, e))?;
        let expected: HashSet<i64> = MIGRATOR.migrations.iter().map(|m| m.version).collect();

        // 逐条校验：已应用版本必须存在于编译期内嵌集合，且 checksum 必须一致
        // （与 SQLx 迁移时的 checksum 校验同一语义）。
        for row in &applied {
            match MIGRATOR
                .migrations
                .iter()
                .find(|m| m.version == row.version)
            {
                Some(compiled) => {
                    if compiled.checksum != row.checksum {
                        return Err(StorageError::from_message(
                            StorageFailureKind::DecodeOrCorrupt,
                            format!("checksum mismatch for migration {}", row.version),
                        ));
                    }
                }
                None => {
                    return Err(StorageError::from_message(
                        StorageFailureKind::DecodeOrCorrupt,
                        format!(
                            "applied migration {} not present in compiled set",
                            row.version
                        ),
                    ));
                }
            }
        }

        // pending 以版本集合差为准，不能只比较数量：数量相等但版本不同
        // 同样是未就绪。
        let applied_versions: HashSet<i64> = applied.iter().map(|m| m.version).collect();
        let pending = expected.difference(&applied_versions).count() as u64;

        // success=0 行与 dirty_version 指向同一批失败记录：SQLx 的
        // dirty_version 就是 `WHERE success = false` 的第一行。对同一失败
        // 只计一次。
        let failed_rows: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 0")
                .fetch_one(&mut *conn)
                .await
                .map_err(StorageError::from)?;
        let dirty = conn
            .dirty_version()
            .await
            .map_err(|e| StorageError::new(StorageFailureKind::DecodeOrCorrupt, e))?
            .is_some();
        let failed = match (failed_rows.0 > 0, dirty) {
            (true, _) => failed_rows.0.max(0) as u64,
            (false, true) => 1,
            (false, false) => 0,
        };

        Ok(MigrationStatus { pending, failed })
    }
}
