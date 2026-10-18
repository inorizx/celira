//! SQLite adapter 测试：真实临时数据库上的迁移与探测。

use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::ports::migration::MigrationProbe;
use sqlx::migrate::Migrate;
use std::error::Error;

#[tokio::test]
async fn migrations_apply_idempotently_and_metadata_is_verifiable() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("core.db");

    let first = MigrationStore::open(&db_path).await.expect("first open");
    let status = first.status().await.expect("probe must succeed");
    assert_eq!(
        status,
        crate::ports::migration::MigrationStatus {
            pending: 0,
            failed: 0
        }
    );

    drop(first);

    let second = MigrationStore::open(&db_path)
        .await
        .expect("reopen must be idempotent");
    let status = second.status().await.expect("probe must succeed");
    assert_eq!(
        status,
        crate::ports::migration::MigrationStatus {
            pending: 0,
            failed: 0
        }
    );

    let mut conn = second.pool().acquire().await.unwrap();
    conn.ensure_migrations_table().await.unwrap();
    let applied = conn.list_applied_migrations().await.unwrap();
    assert_eq!(
        applied.len(),
        crate::adapters::sqlite::migration_store::MIGRATOR
            .migrations
            .len()
    );
    assert!(conn.dirty_version().await.unwrap().is_none());
    let failed: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 0")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(failed.0, 0);
}

#[tokio::test]
async fn real_pool_connections_have_foreign_keys_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("open");

    let mut conn = store.pool().acquire().await.expect("acquire from pool");
    let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut *conn)
        .await
        .expect("read PRAGMA foreign_keys from a real pooled connection");
    assert_eq!(
        fk, 1,
        "Room 等公共事实的 deferred FK 闭合依赖连接级 PRAGMA foreign_keys=ON，真实池连接必须生效"
    );
}

#[tokio::test]
async fn open_creates_parent_directories() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("nested/deeper/core.db");
    MigrationStore::open(&db_path)
        .await
        .expect("parent dirs must be created");
    assert!(db_path.exists());
}

#[tokio::test]
async fn invalid_db_path_fails_observably() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"not a directory").unwrap();
    let db_path = blocker.join("core.db");

    let err = MigrationStore::open(&db_path).await.unwrap_err();
    assert!(
        matches!(
            err,
            crate::adapters::sqlite::migration_store::DbError::CreateParentDir(_)
        ),
        "expected parent dir creation failure, got {err}"
    );
}

#[tokio::test]
async fn failed_migration_is_counted_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("open");
    {
        let mut conn = store.pool().acquire().await.unwrap();
        conn.ensure_migrations_table().await.unwrap();
        let latest: i64 = sqlx::query_scalar(
            "SELECT version FROM _sqlx_migrations ORDER BY version DESC LIMIT 1",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        sqlx::query("UPDATE _sqlx_migrations SET success = 0 WHERE version = ?")
            .bind(latest)
            .execute(&mut *conn)
            .await
            .unwrap();
    }

    let status = store.status().await.expect("probe must succeed");
    assert_eq!(
        status.failed, 1,
        "one failed migration must be counted exactly once"
    );
}

#[tokio::test]
async fn checksum_mismatch_fails_probe() {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("open");
    {
        let mut conn = store.pool().acquire().await.unwrap();
        conn.ensure_migrations_table().await.unwrap();
        sqlx::query(
            "UPDATE _sqlx_migrations SET checksum = X'00' \
             WHERE version = (SELECT MIN(version) FROM _sqlx_migrations)",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    let err = store.status().await.unwrap_err();
    assert_eq!(
        err.kind(),
        crate::ports::storage::StorageFailureKind::DecodeOrCorrupt,
        "checksum mismatch is a structure inconsistency"
    );
    let cause = err.source().expect("typed cause must be present");
    assert!(
        cause.to_string().contains("checksum"),
        "checksum mismatch detail must stay reachable in the cause chain, got {cause}"
    );
    assert!(
        !err.to_string().contains("checksum"),
        "StorageError Display must stay classification-only: {err}"
    );
}

#[tokio::test]
async fn applied_version_outside_compiled_set_fails_probe() {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("open");
    {
        let mut conn = store.pool().acquire().await.unwrap();
        conn.ensure_migrations_table().await.unwrap();
        sqlx::query(
            "INSERT INTO _sqlx_migrations \
             (version, description, installed_on, success, checksum, execution_time) \
             VALUES (42424242, 'foreign migration', CURRENT_TIMESTAMP, 1, X'00', 0)",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
    }

    let err = store.status().await.unwrap_err();
    assert_eq!(
        err.kind(),
        crate::ports::storage::StorageFailureKind::DecodeOrCorrupt,
        "foreign applied version is a structure inconsistency"
    );
    let cause = err.source().expect("typed cause must be present");
    assert!(
        cause.to_string().contains("42424242"),
        "unknown-version detail must stay reachable in the cause chain, got {cause}"
    );
}

#[tokio::test]
#[cfg(unix)]
async fn status_fails_when_db_file_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("core.db");
    let store = MigrationStore::open(&db_path).await.expect("open");
    std::fs::remove_file(&db_path).unwrap();

    let err = store.status().await.unwrap_err();
    assert_eq!(
        err.kind(),
        crate::ports::storage::StorageFailureKind::ConnectionOrIo,
        "missing db file is a connection/io condition"
    );
    let cause = err.source().expect("typed cause must be present");
    assert!(
        cause.to_string().contains("missing"),
        "missing-file detail must stay reachable in the cause chain, got {cause}"
    );
}
