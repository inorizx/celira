//! SQLite adapter：migration、查询和事务实现。

use crate::ports::storage::{StorageError, StorageFailureKind};

/// 把底层 `sqlx::Error` 归入稳定内部类别（CORE_V1_ERROR_HANDLING.md §7.1）。
///
/// 分类优先使用 SQLx/SQLite 的错误类型与错误码，不依赖 Display 文本：
/// - `DatabaseError::kind()` 直接裁决 UNIQUE / FOREIGN KEY / NOT NULL /
///   CHECK 约束（对应 SQLITE_CONSTRAINT 扩展码）；
/// - 其余按 SQLite primary result code 裁决：5/6 busy/locked、10/13/14
///   IO/打开/满、26 NOTADB（非数据库文件，持久事实不可用，归
///   `decode_or_corrupt` 而非连接/IO）、11 数据损坏、19 未映射的 constraint
///   扩展码；
/// - `sqlx::Error` 非 Database 变体按来源归 IO/池/解码/迁移等类别；
/// - 未分类一律 `Unknown`（对外 `internal_error`），绝不猜测业务语义。
impl From<sqlx::Error> for StorageError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::Database(ref dberr) => match dberr.kind() {
                sqlx::error::ErrorKind::UniqueViolation
                | sqlx::error::ErrorKind::ForeignKeyViolation
                | sqlx::error::ErrorKind::NotNullViolation
                | sqlx::error::ErrorKind::CheckViolation => {
                    StorageError::new(StorageFailureKind::Constraint, e)
                }
                sqlx::error::ErrorKind::Other => match sqlite_primary_code(&**dberr) {
                    Some(5 | 6) => StorageError::new(StorageFailureKind::BusyOrLocked, e),
                    Some(10 | 13 | 14) => StorageError::new(StorageFailureKind::ConnectionOrIo, e),
                    // NOTADB（26）：文件存在但根本不是 SQLite 数据库——持久
                    // 事实不可解析，归 decode_or_corrupt（对外 500），不是
                    // 暂时不可用的连接/IO（不得可重试 503）。
                    Some(26) => StorageError::new(StorageFailureKind::DecodeOrCorrupt, e),
                    Some(11) => StorageError::new(StorageFailureKind::DecodeOrCorrupt, e),
                    Some(19) => StorageError::new(StorageFailureKind::Constraint, e),
                    _ => StorageError::new(StorageFailureKind::Unknown, e),
                },
                _ => StorageError::new(StorageFailureKind::Unknown, e),
            },
            sqlx::Error::Io(_) | sqlx::Error::Tls(_) => {
                StorageError::new(StorageFailureKind::ConnectionOrIo, e)
            }
            sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed | sqlx::Error::WorkerCrashed => {
                StorageError::new(StorageFailureKind::ConnectionOrIo, e)
            }
            sqlx::Error::Decode(_)
            | sqlx::Error::ColumnDecode { .. }
            | sqlx::Error::ColumnNotFound(_)
            | sqlx::Error::TypeNotFound { .. }
            | sqlx::Error::ColumnIndexOutOfBounds { .. }
            | sqlx::Error::RowNotFound
            | sqlx::Error::Protocol(_)
            | sqlx::Error::Migrate(_) => StorageError::new(StorageFailureKind::DecodeOrCorrupt, e),
            sqlx::Error::BeginFailed => StorageError::new(StorageFailureKind::ConnectionOrIo, e),
            sqlx::Error::Configuration(_)
            | sqlx::Error::InvalidArgument(_)
            | sqlx::Error::Encode(_)
            | sqlx::Error::AnyDriverError(_)
            | sqlx::Error::InvalidSavePointStatement
            | _ => StorageError::new(StorageFailureKind::Unknown, e),
        }
    }
}

/// SQLite primary result code（去掉扩展码高位）。字符串无法解析时返回 None。
fn sqlite_primary_code(dberr: &dyn sqlx::error::DatabaseError) -> Option<i64> {
    let code = dberr.code()?;
    code.parse::<i64>().ok().map(|c| c & 0xff)
}

/// 幂等重放成功分支的无 primary 显式回滚（CORE_V1_ERROR_HANDLING.md §7.2）：
/// 只回滚、不携带任何业务 primary。rollback 成功返回 `Ok(())`；rollback
/// 失败返回普通 rollback `StorageError`（无复合，语义与既有重放分支一致）。
pub(crate) async fn finish_rollback(
    tx: sqlx::sqlite::SqliteTransaction<'_>,
) -> Result<(), StorageError> {
    match tx.rollback().await {
        Ok(()) => Ok(()),
        Err(e) => Err(StorageError::from(e)),
    }
}

/// 业务失败分支的收尾 helper（CORE_V1_ERROR_HANDLING.md §7.2）：先显式
/// rollback，再把 primary 原样交还；rollback 失败以复合 `StorageError`
/// 保留双因果，任何业务写入失败都是 primary，绝不被 rollback 覆盖。
///
/// - rollback 成功：返回原 `primary`（`E` 原样——`IdempotencyConflict` /
///   `AuthorizationRejected` / `NotFound` / `SourceInactive` 等业务变体与
///   `Database(StorageError)` 的类型/变体完全不变）；
/// - rollback 失败：返回 `wrap(composite)`——composite 的 `source()` 主链
///   持有原 primary（`E`，可 `downcast_ref` 还原原 StoreError 业务变体），
///   rollback 的分类/cause 经 `with_rollback` 旁链独立保留
///   （`rollback_kind()` / `rollback_cause()` / `has_rollback_failure()`）；
///   composite 的 `kind()` 按 `primary_kind` 裁决：
///   - primary 是 `Database(StorageError)`：沿用嵌套 `StorageError.kind()`，
///     对外映射与原有语义完全一致；
///   - primary 是业务变体：`Unknown`（内部事务完成失败分类，对外 500
///     `internal_error`），绝不声称业务冲突已可靠回滚。
///
/// `primary_kind` 由调用方按 primary 变体裁决（Database → 嵌套 kind，业务
/// 变体 → `Unknown`）；`wrap` 由各 Store 把复合 StorageError 包回自己的
/// `Database` 变体。
///
/// 不改变 commit 成功、业务裁决、幂等重放语义与 Transaction Drop 的取消
/// 回滚路径（commit 失败后 SQLx 由 Drop 启动回滚，见规范 §7.2）。
pub(crate) async fn rollback_after_primary<E>(
    tx: sqlx::sqlite::SqliteTransaction<'_>,
    primary: E,
    primary_kind: StorageFailureKind,
    wrap: impl FnOnce(StorageError) -> E,
) -> E
where
    E: std::error::Error + Send + Sync + 'static,
{
    match tx.rollback().await {
        Ok(()) => primary,
        Err(e) => wrap(compose_rollback_primary(
            primary,
            primary_kind,
            StorageError::from(e),
        )),
    }
}

/// 纯组合函数（独立出来以便 synthetic 测试直接覆盖 rollback 失败分支）：
/// primary 作为 composite 的 typed cause 保留在 `source()` 主链（可
/// downcast 还原原 StoreError 业务变体），rollback 分类/cause 经
/// `with_rollback` 旁链保留；`kind()` 按 `primary_kind` 裁决。
pub(crate) fn compose_rollback_primary<E>(
    primary: E,
    primary_kind: StorageFailureKind,
    rollback: StorageError,
) -> StorageError
where
    E: std::error::Error + Send + Sync + 'static,
{
    StorageError::new(primary_kind, primary).with_rollback(rollback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::message::MessageStoreError;
    use sqlx::Connection;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::error::Error;
    use std::path::Path;
    use std::time::Duration;

    /// 单连接、busy_timeout=0 的真实 SQLite 池（锁竞争立即返回 SQLITE_BUSY）。
    async fn pool(path: &Path) -> sqlx::SqlitePool {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .busy_timeout(Duration::ZERO);
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .expect("open sqlite for classification test")
    }

    /// 真实 SQLITE_CONSTRAINT（UNIQUE）→ `constraint`，且 cause 链保留
    /// 原始 `sqlx::Error`（类型身份与文本都可识别）。
    #[tokio::test]
    async fn unique_violation_classifies_as_constraint_with_typed_cause() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool(&dir.path().join("constraint.db")).await;
        sqlx::query("CREATE TABLE t (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO t (id, name) VALUES ('a', 'x')")
            .execute(&pool)
            .await
            .unwrap();
        let err = sqlx::query("INSERT INTO t (id, name) VALUES ('a', 'y')")
            .execute(&pool)
            .await
            .unwrap_err();

        let storage = StorageError::from(err);
        assert_eq!(storage.kind(), StorageFailureKind::Constraint);

        // 链可下钻：StorageError → sqlx::Error → 数据库原始错误；原始
        // constraint 文本在 cause 链中可识别，而 StorageError Display 只
        // 有稳定分类。
        let sqlx_err = storage.source().expect("sqlx::Error cause present");
        assert!(
            sqlx_err
                .to_string()
                .contains("error returned from database"),
            "sqlx::Error Display must be reachable through the chain: {sqlx_err}"
        );
        let db_err = sqlx_err.source().expect("database error below sqlx::Error");
        assert!(
            db_err.to_string().contains("UNIQUE constraint failed"),
            "original SQLite constraint cause must be identifiable: {db_err}"
        );
        assert!(
            !storage.to_string().contains("UNIQUE constraint failed"),
            "StorageError Display must stay classification-only: {}",
            storage
        );
    }

    /// 真实 SQLITE_CONSTRAINT_FOREIGNKEY → `constraint`。
    #[tokio::test]
    async fn foreign_key_violation_classifies_as_constraint() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool(&dir.path().join("fk.db")).await;
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE parent (id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE child (id TEXT PRIMARY KEY, parent_id TEXT NOT NULL REFERENCES parent(id))")
            .execute(&pool)
            .await
            .unwrap();
        let err = sqlx::query("INSERT INTO child (id, parent_id) VALUES ('c', 'missing')")
            .execute(&pool)
            .await
            .unwrap_err();
        assert_eq!(
            StorageError::from(err).kind(),
            StorageFailureKind::Constraint
        );
    }

    /// 真实 SQLITE_BUSY（第二连接 BEGIN IMMEDIATE 撞第一连接写锁）→
    /// `busy_or_locked`。
    #[tokio::test]
    async fn busy_lock_conflict_classifies_as_busy_or_locked() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("busy.db");
        let pool_a = pool(&db_path).await;
        let pool_b = pool(&db_path).await;
        sqlx::query("CREATE TABLE t (id TEXT PRIMARY KEY)")
            .execute(&pool_a)
            .await
            .unwrap();

        let mut conn_a = pool_a.acquire().await.unwrap();
        let _tx_a = conn_a
            .begin_with("BEGIN IMMEDIATE")
            .await
            .expect("first writer holds the write lock");
        let mut conn_b = pool_b.acquire().await.unwrap();
        let err = conn_b
            .begin_with("BEGIN IMMEDIATE")
            .await
            .expect_err("second writer must hit SQLITE_BUSY with zero busy timeout");

        assert_eq!(
            StorageError::from(err).kind(),
            StorageFailureKind::BusyOrLocked
        );
    }

    /// 真实 SQLx 解码失败（列类型与目标类型不匹配）→ `decode_or_corrupt`。
    #[tokio::test]
    async fn column_type_mismatch_classifies_as_decode_or_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool(&dir.path().join("decode.db")).await;
        sqlx::query("CREATE TABLE t (id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO t (id) VALUES ('not-an-int')")
            .execute(&pool)
            .await
            .unwrap();
        let err = sqlx::query_as::<_, (i64,)>("SELECT id FROM t")
            .fetch_one(&pool)
            .await
            .unwrap_err();
        assert_eq!(
            StorageError::from(err).kind(),
            StorageFailureKind::DecodeOrCorrupt
        );
    }

    /// 非 Database 变体按来源归类；未分类保持 `unknown`（不猜测业务语义）。
    #[test]
    fn non_database_variants_classify_by_source() {
        let io = StorageError::from(sqlx::Error::Io(std::io::Error::other("disk gone")));
        assert_eq!(io.kind(), StorageFailureKind::ConnectionOrIo);

        let closed = StorageError::from(sqlx::Error::PoolClosed);
        assert_eq!(closed.kind(), StorageFailureKind::ConnectionOrIo);

        let row = StorageError::from(sqlx::Error::RowNotFound);
        assert_eq!(row.kind(), StorageFailureKind::DecodeOrCorrupt);

        let protocol = StorageError::from(sqlx::Error::Protocol("bad frame".into()));
        assert_eq!(protocol.kind(), StorageFailureKind::DecodeOrCorrupt);

        let config = StorageError::from(sqlx::Error::InvalidArgument("bad".into()));
        assert_eq!(config.kind(), StorageFailureKind::Unknown);
    }

    /// 真实 SQLITE_NOTADB（26：文件存在但不是 SQLite 数据库）→
    /// `decode_or_corrupt`（对外 500 internal_error），不是 `connection_or_io`
    /// （不得可重试 503）。
    #[tokio::test]
    async fn not_a_database_file_classifies_as_decode_or_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("not-a-db.db");
        std::fs::write(&path, b"this is definitely not a sqlite database file").unwrap();
        let pool = pool(&path).await;
        let err = sqlx::query("SELECT count(*) FROM sqlite_master")
            .fetch_one(&pool)
            .await
            .err()
            .expect("reading a non-database file must fail");

        let storage = StorageError::from(err);
        assert_eq!(
            storage.kind(),
            StorageFailureKind::DecodeOrCorrupt,
            "NOTADB must classify as decode_or_corrupt, not connection_or_io"
        );
        let sqlx_err = storage.source().expect("sqlx::Error cause present");
        let db_err = sqlx_err.source().expect("database error below sqlx::Error");
        assert!(
            db_err.to_string().contains("not a database"),
            "original NOTADB cause must be identifiable: {db_err}"
        );
        assert!(
            !storage.to_string().contains("not a database"),
            "StorageError Display must stay classification-only: {}",
            storage
        );
    }

    /// 业务 primary（`MessageStoreError::IdempotencyConflict`）+ synthetic
    /// rollback 失败：composite `source()` 主链仍可 downcast 还原业务变体，
    /// rollback cause 独立可识别，Display 无注入路径/SQL。
    #[test]
    fn message_business_primary_survives_rollback_failure_in_source_chain() {
        let primary = MessageStoreError::IdempotencyConflict;
        let rollback = StorageError::from_message(
            StorageFailureKind::ConnectionOrIo,
            "rollback failed: disk gone, SELECT * FROM secrets WHERE token='abc' path=/tmp/core.db",
        );

        let composite = compose_rollback_primary(primary, StorageFailureKind::Unknown, rollback);

        assert_eq!(
            composite.kind(),
            StorageFailureKind::Unknown,
            "business primary composite must classify as Unknown (对外 500 internal_error)"
        );
        let source = composite
            .source()
            .expect("primary source chain must survive");
        let primary_back = source
            .downcast_ref::<MessageStoreError>()
            .expect("composite source must be the original store error");
        assert!(
            matches!(primary_back, MessageStoreError::IdempotencyConflict),
            "business variant must survive the rollback failure: {primary_back:?}"
        );

        assert!(composite.has_rollback_failure());
        assert_eq!(
            composite.rollback_kind(),
            Some(StorageFailureKind::ConnectionOrIo)
        );
        let rollback_cause = composite
            .rollback_cause()
            .expect("rollback cause must be separately identifiable");
        assert!(rollback_cause.to_string().contains("disk gone"));

        let display = composite.to_string();
        for forbidden in [
            "SELECT", "secrets", "token", "abc", "/tmp", "core.db", "disk",
        ] {
            assert!(
                !display.contains(forbidden),
                "Display must not leak primary/rollback cause text: {forbidden:?}: {display:?}"
            );
        }
        assert_eq!(display, "storage failure: unknown");
    }

    /// 业务 primary（`BackendStoreError::AgentNotActive`）+
    /// synthetic rollback 失败：同类证明——业务变体不被 rollback Database
    /// 错误替换，rollback cause 独立保留。
    /// ⚠ 2026-09-28：原用的是 `CompletionStoreError::AuthorizationRejected`
    /// ——那个 store 已随 Completion/ACK 整体退役删除，换一个仍在场的业务
    /// 变体，证的是同一条性质。
    #[test]
    fn business_primary_survives_rollback_failure_in_source_chain() {
        let primary = crate::ports::backend::BackendStoreError::AgentNotActive;
        let rollback = StorageError::from_message(
            StorageFailureKind::BusyOrLocked,
            "rollback: database is locked at /tmp/secret/core.db",
        );

        let composite = compose_rollback_primary(primary, StorageFailureKind::Unknown, rollback);

        assert_eq!(composite.kind(), StorageFailureKind::Unknown);
        let source = composite
            .source()
            .expect("primary source chain must survive");
        let primary_back = source
            .downcast_ref::<crate::ports::backend::BackendStoreError>()
            .expect("composite source must be the original store error");
        assert!(
            matches!(
                primary_back,
                crate::ports::backend::BackendStoreError::AgentNotActive
            ),
            "business variant must survive the rollback failure: {primary_back:?}"
        );
        assert_eq!(
            composite.rollback_kind(),
            Some(StorageFailureKind::BusyOrLocked)
        );
        assert!(
            composite
                .rollback_cause()
                .expect("rollback cause")
                .to_string()
                .contains("locked")
        );
        let display = composite.to_string();
        for forbidden in ["locked", "/tmp", "secret", "core.db"] {
            assert!(
                !display.contains(forbidden),
                "Display must not leak rollback cause text: {forbidden:?}: {display:?}"
            );
        }
        assert_eq!(display, "storage failure: unknown");
    }

    /// Database primary + synthetic rollback 失败：composite `kind()` 沿用
    /// 嵌套 StorageError kind，`source()` 主链仍是原 `Database` 变体（可
    /// 下钻到嵌套 StorageError 与底层 cause），rollback cause 旁链独立。
    #[test]
    fn database_primary_rollback_failure_keeps_kind_and_source() {
        let db_primary = StorageError::from_message(
            StorageFailureKind::Constraint,
            "primary: UNIQUE constraint failed: secret_token",
        );
        let rollback = StorageError::from_message(
            StorageFailureKind::ConnectionOrIo,
            "rollback: disk gone at /tmp/secret/core.db",
        );

        let composite = compose_rollback_primary(
            MessageStoreError::Database(db_primary.clone()),
            db_primary.kind(),
            rollback,
        );

        assert_eq!(composite.kind(), StorageFailureKind::Constraint);
        let source = composite.source().expect("primary source chain");
        let primary_back = source
            .downcast_ref::<MessageStoreError>()
            .expect("composite source must be the original store error");
        let MessageStoreError::Database(nested) = primary_back else {
            panic!("Database primary must stay a Database variant: {primary_back:?}");
        };
        assert_eq!(nested.kind(), StorageFailureKind::Constraint);
        assert_eq!(
            nested.source().expect("nested typed cause").to_string(),
            "primary: UNIQUE constraint failed: secret_token"
        );
        assert!(composite.has_rollback_failure());
        assert_eq!(
            composite.to_string(),
            "storage failure: constraint",
            "Display keeps the primary stable classification"
        );
    }

    /// 真实事务上的 Database primary + 成功 rollback：`rollback_after_primary`
    /// 原样返回 primary，类型/变体与 cause 链完全不变（约束冲突在真实
    /// SQLite 上触发 primary，随后显式 rollback 成功）。
    #[tokio::test]
    async fn rollback_after_primary_success_returns_database_primary_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool(&dir.path().join("rollback.db")).await;
        sqlx::query("CREATE TABLE t (id TEXT PRIMARY KEY, name TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO t (id, name) VALUES ('a', 'x')")
            .execute(&pool)
            .await
            .unwrap();

        let mut conn = pool.acquire().await.unwrap();
        let mut tx = conn
            .begin_with("BEGIN IMMEDIATE")
            .await
            .expect("begin immediate");
        let primary = sqlx::query("INSERT INTO t (id, name) VALUES ('a', 'y')")
            .execute(&mut *tx)
            .await
            .expect_err("duplicate primary key must fail inside the transaction");
        let primary = MessageStoreError::Database(StorageError::from(primary));
        let kind_before = match &primary {
            MessageStoreError::Database(storage) => storage.kind(),
            _ => unreachable!(),
        };

        let returned =
            rollback_after_primary(tx, primary, kind_before, MessageStoreError::Database).await;
        let MessageStoreError::Database(returned) = returned else {
            panic!("Database primary must be returned after successful rollback");
        };
        assert_eq!(returned.kind(), kind_before);
        assert_eq!(returned.kind(), StorageFailureKind::Constraint);
        assert!(!returned.has_rollback_failure());
        let sqlx_err = returned.source().expect("typed sqlx cause must survive");
        assert!(
            sqlx_err
                .to_string()
                .contains("error returned from database"),
            "sqlx::Error must survive the rollback boundary: {sqlx_err}"
        );
        let db_err = sqlx_err.source().expect("database error below sqlx::Error");
        assert!(
            db_err.to_string().contains("UNIQUE constraint failed"),
            "original constraint cause must be identifiable: {db_err}"
        );
    }

    /// 真实事务上的业务 primary + 成功 rollback：`rollback_after_primary`
    /// 原样返回业务变体（`IdempotencyConflict`），不被 Database 包装或
    /// rollback 结果替换。
    #[tokio::test]
    async fn rollback_after_primary_success_returns_business_variant_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool(&dir.path().join("rollback-business.db")).await;
        sqlx::query("CREATE TABLE t (id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();

        let mut conn = pool.acquire().await.unwrap();
        let tx = conn
            .begin_with("BEGIN IMMEDIATE")
            .await
            .expect("begin immediate");
        let primary = MessageStoreError::IdempotencyConflict;

        let returned = rollback_after_primary(
            tx,
            primary,
            StorageFailureKind::Unknown,
            MessageStoreError::Database,
        )
        .await;
        assert!(
            matches!(returned, MessageStoreError::IdempotencyConflict),
            "business variant must survive real rollback: {returned:?}"
        );
    }
}

pub mod agent_store;
pub mod attachment_store;
pub mod backend_store;
pub mod contact_store;
pub mod event_store;
pub mod home_store;
pub mod human_event_store;
pub mod identity_store;
pub mod message_kernel;
pub mod message_store;
pub mod migration_store;
pub mod participant_presentation_store;
pub mod room_store;
/// 测试夹具专用（关系种子）：第③步把准入判据换成"双方确认过的关系"之后，
/// 所有"建房 / 拉群"夹具都要先有一段 accepted 关系。
#[cfg(test)]
pub(crate) mod test_relations;

#[cfg(test)]
mod agent_store_tests;
#[cfg(test)]
mod attachment_store_tests;
#[cfg(test)]
mod backend_store_tests;
#[cfg(test)]
#[cfg(test)]
mod contact_reversibility_migration_tests;
#[cfg(test)]
mod contact_store_tests;
#[cfg(test)]
#[cfg(test)]
mod event_store_tests;
#[cfg(test)]
mod home_store_tests;
#[cfg(test)]
mod human_event_store_tests;
#[cfg(test)]
mod identity_guard_tests;
#[cfg(test)]
mod identity_store_tests;
#[cfg(test)]
mod message_store_tests;
#[cfg(test)]
mod migration_store_tests;
#[cfg(test)]
mod pairing_transaction_store_tests;
#[cfg(test)]
mod participant_avatar_migration_tests;
#[cfg(test)]
mod participant_presentation_migration_tests;
#[cfg(test)]
mod participant_presentation_store_tests;
#[cfg(test)]
mod read_cursor_tests;
#[cfg(test)]
mod recall_store_tests;
#[cfg(test)]
mod room_attention_migration_tests;
#[cfg(test)]
mod room_profile_migration_tests;
#[cfg(test)]
mod room_profile_store_tests;
#[cfg(test)]
mod room_store_tests;
