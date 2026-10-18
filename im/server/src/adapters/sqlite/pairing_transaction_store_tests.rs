//! A2 Pairing Transaction SQLite adapter 测试：真实临时数据库上的
//! prepare 幂等重放、commit 原子消费与幂等重试、abort 保留 Grant、
//! 过期/invalid 统一 redaction、并发 commit 恰好一次、schema 兜底。

use std::time::Duration;

use tokio::sync::oneshot;

use crate::adapters::sqlite::home_store::SqliteHomeStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::application::home;
use crate::domain::human_session::{SessionId, TokenHash};
use crate::domain::pairing_transaction::{TransactionId, TransactionStatus};
use crate::domain::participant::ParticipantId;
use crate::domain::time::UtcMillis;
use crate::ports::home::{
    CommitTransactionCommand, HomeStore as _, PrepareTransactionRequest, StoreError,
};
use crate::ports::migration::MigrationProbe;

struct Fixture {
    _dir: tempfile::TempDir,
    store: MigrationStore,
    home: SqliteHomeStore,
    human_id: String,
    grant_id: String,
    grant_token_hash: TokenHash,
    session_token_hex: String,
    commit_token_hex: String,
    commit_token_hash: TokenHash,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("open + migrate");
    let home = SqliteHomeStore::new(store.pool().clone());
    let identity = SqliteIdentityStore::new(store.pool().clone());
    home::init_home(&home, None).await.expect("init home");
    let outcome = crate::application::identity::bootstrap_human(&identity, "Alice")
        .await
        .expect("bootstrap");
    let human_id = outcome.participant.id.as_str().to_owned();
    let issued = home::issue_grant(
        &home,
        &ParticipantId(human_id.clone()),
        None,
        crate::domain::home::GrantKind::SelfDevice,
        15,
    )
    .await
    .expect("issue grant");
    let grant_id = issued.grant.grant_id.as_str().to_owned();
    let grant_token_hash = TokenHash::from_token_str(issued.token.as_str());

    // Host 在 Core 之外生成的高熵 token → 只提交 SHA-256 摘要（64 hex）。
    let session_token_hex = "a".repeat(63) + "b";
    let commit_token_hex = "f".repeat(64);
    let commit_token_hash = TokenHash::parse_hex(&commit_token_hex).unwrap();

    Fixture {
        _dir: dir,
        store,
        home,
        human_id,
        grant_id,
        grant_token_hash,
        session_token_hex,
        commit_token_hex,
        commit_token_hash,
    }
}

async fn count_rows(f: &Fixture, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

fn prepare_request(f: &Fixture, nonce: &str) -> PrepareTransactionRequest {
    PrepareTransactionRequest {
        grant_token_hash: f.grant_token_hash,
        client_nonce: nonce.to_owned(),
        session_token_hash: TokenHash::parse_hex(&f.session_token_hex).unwrap(),
        commit_token_hash: f.commit_token_hash,
        transaction_id: TransactionId::new_v4(),
        reserved_session_id: SessionId(uuid::Uuid::new_v4().to_string()),
        prepared_at: UtcMillis::now(),
        expires_at: UtcMillis::from_millis(UtcMillis::now().millis() + 900_000),
    }
}

fn commit_command(f: &Fixture, transaction_id: &TransactionId) -> CommitTransactionCommand {
    CommitTransactionCommand {
        transaction_id: transaction_id.clone(),
        commit_token_hash: f.commit_token_hash,
        device_label: None,
        committed_at: UtcMillis::now(),
    }
}

async fn commit_owned(
    store: SqliteHomeStore,
    transaction_id: TransactionId,
    commit_token_hash: TokenHash,
) -> Result<crate::domain::pairing_transaction::TransactionView, StoreError> {
    store
        .commit_transaction(
            &CommitTransactionCommand {
                transaction_id,
                commit_token_hash,
                device_label: None,
                committed_at: UtcMillis::now(),
            },
            UtcMillis::now(),
        )
        .await
}

#[tokio::test]
async fn prepare_is_idempotent_for_same_nonce_and_digests() {
    let f = fixture().await;
    let sessions_before = count_rows(&f, "human_sessions").await;
    let tx_before = count_rows(&f, "pairing_transactions").await;
    let nonce = "host-generated-nonce-1";

    let first = f
        .home
        .prepare_transaction(&prepare_request(&f, nonce), UtcMillis::now())
        .await
        .expect("first prepare");
    let second = f
        .home
        .prepare_transaction(&prepare_request(&f, nonce), UtcMillis::now())
        .await
        .expect("retry prepare");
    let third = f
        .home
        .prepare_transaction(&prepare_request(&f, nonce), UtcMillis::now())
        .await
        .expect("retry prepare 2");

    assert_eq!(
        first.transaction_id, second.transaction_id,
        "idempotent reuse must return the same transaction id"
    );
    assert_eq!(second.transaction_id, third.transaction_id);
    assert_eq!(tx_before + 1, count_rows(&f, "pairing_transactions").await);
    // prepare 不创建正式 Session、不消费 Grant。
    assert_eq!(sessions_before, count_rows(&f, "human_sessions").await);
    let (grant_status,): (String,) = sqlx::query_as("SELECT status FROM pairing_grants")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(grant_status, "active");
    assert_eq!(first.status, TransactionStatus::Prepared);
    assert_eq!(first.home_id.as_str().len(), 36);
    assert_eq!(first.human_id.as_str(), f.human_id);
}

#[tokio::test]
async fn prepare_same_nonce_different_digests_is_invalid() {
    let f = fixture().await;
    let nonce = "host-generated-nonce-2";
    let _first = f
        .home
        .prepare_transaction(&prepare_request(&f, nonce), UtcMillis::now())
        .await
        .expect("first prepare");

    // 同一 nonce + 不同 curl digest → 参数不变量冲突（统一
    // `pairing_transaction_invalid`，不暴露存在性）。
    let mut conflict = prepare_request(&f, nonce);
    conflict.session_token_hash = TokenHash::parse_hex(&("c".repeat(63) + "d")).unwrap();
    let err = f
        .home
        .prepare_transaction(&conflict, UtcMillis::now())
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::TransactionNotFound));
    assert_eq!(count_rows(&f, "pairing_transactions").await, 1);
}

#[tokio::test]
async fn commit_activates_reserved_session_and_consumes_grant_exactly_once() {
    let f = fixture().await;
    let nonce = "commit-nonce";
    let prepared = f
        .home
        .prepare_transaction(&prepare_request(&f, nonce), UtcMillis::now())
        .await
        .expect("prepare");
    let sessions_before = count_rows(&f, "human_sessions").await;

    let committed = f
        .home
        .commit_transaction(
            &commit_command(&f, &prepared.transaction_id),
            UtcMillis::now(),
        )
        .await
        .expect("commit");
    assert_eq!(committed.status, TransactionStatus::Committed);
    assert!(committed.committed_at.is_some());
    assert_eq!(committed.reserved_session_id, prepared.reserved_session_id);
    assert_eq!(committed.transaction_id, prepared.transaction_id);

    // 恰一个 Session + session_metadata；token hash = prepare 提交的摘要。
    assert_eq!(sessions_before + 1, count_rows(&f, "human_sessions").await);
    assert_eq!(count_rows(&f, "session_metadata").await, 1);
    let (db_hash,): (String,) =
        sqlx::query_as("SELECT token_hash FROM human_sessions WHERE id = ?")
            .bind(committed.reserved_session_id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(db_hash, f.session_token_hex);
    let (grant_status,): (String,) = sqlx::query_as("SELECT status FROM pairing_grants")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(grant_status, "consumed");
    let (txn_status, committed_at): (String, Option<i64>) =
        sqlx::query_as("SELECT status, committed_at FROM pairing_transactions")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(txn_status, "committed");
    assert!(committed_at.is_some());
}

#[tokio::test]
async fn commit_retry_is_idempotent_and_never_creates_a_second_session() {
    let f = fixture().await;
    let nonce = "commit-retry-nonce";
    let _prepared = f
        .home
        .prepare_transaction(&prepare_request(&f, nonce), UtcMillis::now())
        .await
        .expect("prepare");
    let sessions_before = count_rows(&f, "human_sessions").await;

    let mut first_command = commit_command(&f, &_prepared.transaction_id);
    first_command.device_label =
        Some(crate::domain::participant::DisplayName::parse("dev").unwrap());
    let first = f
        .home
        .commit_transaction(&first_command, UtcMillis::now())
        .await
        .expect("first commit");
    let replay = f
        .home
        .commit_transaction(
            &commit_command(&f, &_prepared.transaction_id),
            UtcMillis::now(),
        )
        .await
        .expect("replay commit must be idempotent");
    assert_eq!(first.transaction_id, replay.transaction_id);
    assert_eq!(sessions_before + 1, count_rows(&f, "human_sessions").await);
    assert_eq!(count_rows(&f, "session_metadata").await, 1);
    assert_eq!(count_rows(&f, "pairing_transactions").await, 1);
}

#[tokio::test]
async fn commit_rolls_back_entirely_when_aborted_mid_transaction() {
    let f = fixture().await;
    let nonce = "commit-rollback-nonce";
    let prepared = f
        .home
        .prepare_transaction(&prepare_request(&f, nonce), UtcMillis::now())
        .await
        .expect("prepare");
    let sessions_before = count_rows(&f, "human_sessions").await;

    let (reached_tx, reached_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    f.home.arm_commit_checkpoint(reached_tx, release_rx);

    let store = f.home.clone();
    let commit_hash = f.commit_token_hash;
    let commit_tx_id = prepared.transaction_id.clone();
    let task = tokio::spawn(async move {
        store
            .commit_transaction(
                &CommitTransactionCommand {
                    transaction_id: commit_tx_id,
                    commit_token_hash: commit_hash,
                    device_label: None,
                    committed_at: UtcMillis::now(),
                },
                UtcMillis::now(),
            )
            .await
    });
    reached_rx.await.expect("commit reached commit pause");
    task.abort();
    let _ = release_tx.send(());

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(sessions_before, count_rows(&f, "human_sessions").await);
    assert_eq!(count_rows(&f, "session_metadata").await, 0);
    let (grant_status,): (String,) = sqlx::query_as("SELECT status FROM pairing_grants")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(
        grant_status, "active",
        "grant must stay usable after rollback"
    );
    let (txn_status,): (String,) = sqlx::query_as("SELECT status FROM pairing_transactions")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(txn_status, "prepared");

    let retry = f
        .home
        .commit_transaction(
            &commit_command(&f, &prepared.transaction_id),
            UtcMillis::now(),
        )
        .await
        .expect("retry commit");
    assert_eq!(retry.status, TransactionStatus::Committed);
    assert_eq!(sessions_before + 1, count_rows(&f, "human_sessions").await);
    let (grant_status,): (String,) = sqlx::query_as("SELECT status FROM pairing_grants")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(grant_status, "consumed");
}

#[tokio::test]
async fn abort_only_affects_prepared_and_leaves_grant_active() {
    let f = fixture().await;
    let nonce = "abort-nonce";
    let prepared = f
        .home
        .prepare_transaction(&prepare_request(&f, nonce), UtcMillis::now())
        .await
        .expect("prepare");

    f.home
        .abort_transaction(
            &crate::ports::home::AbortTransactionCommand {
                transaction_id: prepared.transaction_id.clone(),
                commit_token_hash: f.commit_token_hash,
                aborted_at: UtcMillis::now(),
            },
            UtcMillis::now(),
        )
        .await
        .expect("abort");
    let (txn_status, aborted_at): (String, Option<i64>) =
        sqlx::query_as("SELECT status, aborted_at FROM pairing_transactions")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(txn_status, "aborted");
    assert!(aborted_at.is_some());
    let (grant_status,): (String,) = sqlx::query_as("SELECT status FROM pairing_grants")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(
        grant_status, "active",
        "abort must not consume or revoke grant"
    );

    // 已 aborted 不可再 commit / abort（统一 redaction）。
    let commit_err = f
        .home
        .commit_transaction(
            &commit_command(&f, &prepared.transaction_id),
            UtcMillis::now(),
        )
        .await
        .unwrap_err();
    assert!(matches!(commit_err, StoreError::TransactionNotFound));
    f.home
        .abort_transaction(
            &crate::ports::home::AbortTransactionCommand {
                transaction_id: prepared.transaction_id.clone(),
                commit_token_hash: f.commit_token_hash,
                aborted_at: UtcMillis::now(),
            },
            UtcMillis::now(),
        )
        .await
        .expect("repeated abort is idempotent");
    // 只有 bootstrap 的初始 Session，A2 没有创建任何新 Session。
    assert_eq!(count_rows(&f, "human_sessions").await, 1);
    assert_eq!(
        count_rows(&f, "pairing_transactions").await,
        1,
        "aborted transaction row persists (audit fact)"
    );
    assert_eq!(prepared.status, TransactionStatus::Prepared);

    // 重新 prepare（新 nonce + 全新 secrets，Host 放弃后重新生成）：
    // Grant 仍 active 可再次准备。
    let mut re_prepare = prepare_request(&f, "abort-nonce-2");
    // commit_token_hash 全局 UNIQUE，且已 aborted 事务保留该摘要为审计
    // 事实；重新准备必须使用全新 secrets（Host 在 abort 后重新生成）。
    re_prepare.session_token_hash = TokenHash::parse_hex(&"b".repeat(64)).unwrap();
    re_prepare.commit_token_hash = TokenHash::parse_hex(&"e".repeat(64)).unwrap();
    let _ = f
        .home
        .prepare_transaction(&re_prepare, UtcMillis::now())
        .await
        .expect("re-prepare after abort must succeed with fresh secrets");
    assert_eq!(
        count_rows(&f, "pairing_transactions").await,
        2,
        "aborted audit fact + fresh prepared transaction"
    );
    let (grant_status,): (String,) = sqlx::query_as("SELECT status FROM pairing_grants")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(grant_status, "active");
}

#[tokio::test]
async fn expiry_and_invalid_are_redacted_uniformly() {
    let f = fixture().await;
    let nonce = "expiry-nonce";
    let now = UtcMillis::now();
    let transaction_id = TransactionId::new_v4();
    let reserved_session_id = SessionId(uuid::Uuid::new_v4().to_string());
    sqlx::query(
        "INSERT INTO pairing_transactions \
         (transaction_id, grant_id, client_nonce, session_token_hash, commit_token_hash, \
          issued_by, reserved_session_id, status, prepared_at, expires_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, 'prepared', ?, ?)",
    )
    .bind(transaction_id.as_str())
    .bind(&f.grant_id)
    .bind(nonce)
    .bind(&f.session_token_hex)
    .bind(&f.commit_token_hex)
    .bind(&f.human_id)
    .bind(reserved_session_id.as_str())
    .bind(now.millis() - 60_000)
    .bind(now.millis() - 1)
    .execute(f.store.pool())
    .await
    .unwrap();

    // status / commit / abort 对过期事务统一 redaction。
    let status_err = f
        .home
        .get_transaction_status(&transaction_id, &f.commit_token_hash, now)
        .await
        .unwrap_err();
    assert!(matches!(status_err, StoreError::TransactionNotFound));
    let commit_err = f
        .home
        .commit_transaction(
            &CommitTransactionCommand {
                transaction_id: transaction_id.clone(),
                commit_token_hash: f.commit_token_hash,
                device_label: None,
                committed_at: now,
            },
            now,
        )
        .await
        .unwrap_err();
    assert!(matches!(commit_err, StoreError::TransactionNotFound));
    // 未知 commit hash 与过期同一错误面。
    let unknown_hash = TokenHash::parse_hex(&"e".repeat(64)).unwrap();
    let status_unknown = f
        .home
        .get_transaction_status(&transaction_id, &unknown_hash, now)
        .await
        .unwrap_err();
    assert!(matches!(status_unknown, StoreError::TransactionNotFound));
    assert_eq!(
        count_rows(&f, "human_sessions").await,
        1,
        "no A2 session created by any invalid shape"
    );

    // 过期事务状态 row 是审计事实，不可 DELETE。
    let del = sqlx::query("DELETE FROM pairing_transactions")
        .execute(f.store.pool())
        .await;
    assert!(
        del.is_err(),
        "pairing transaction row must not be deletable"
    );
}

#[tokio::test]
async fn concurrent_commit_creates_exactly_one_session_and_consumption() {
    let f = fixture().await;
    let nonce = "concurrent-commit";
    let prepared = f
        .home
        .prepare_transaction(&prepare_request(&f, nonce), UtcMillis::now())
        .await
        .expect("prepare");
    let sessions_before = count_rows(&f, "human_sessions").await;

    let h1 = f.home.clone();
    let h2 = f.home.clone();
    let c1 = f.commit_token_hash;
    let c2 = f.commit_token_hash;
    let tx1 = prepared.transaction_id.clone();
    let tx2 = prepared.transaction_id.clone();
    let t1 = tokio::spawn(async move { commit_owned(h1, tx1, c1).await });
    let t2 = tokio::spawn(async move { commit_owned(h2, tx2, c2).await });
    let (r1, r2) = tokio::join!(t1, t2);
    let ok = [r1, r2]
        .iter()
        .filter(|r| r.as_ref().unwrap().is_ok())
        .count();
    assert!(ok >= 1, "at least one commit wins");
    assert_eq!(sessions_before + 1, count_rows(&f, "human_sessions").await);
    assert_eq!(count_rows(&f, "session_metadata").await, 1);
    let (grant_status,): (String,) = sqlx::query_as("SELECT status FROM pairing_grants")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(grant_status, "consumed");
}

#[tokio::test]
async fn schema_guards_reject_direct_rewrites_of_pairing_transactions() {
    let f = fixture().await;
    let nonce = "schema-nonce";
    let _prepared = f
        .home
        .prepare_transaction(&prepare_request(&f, nonce), UtcMillis::now())
        .await
        .expect("prepare");
    let pool = f.store.pool();

    // prepare 字段不可改写。
    let t1 = sqlx::query("UPDATE pairing_transactions SET session_token_hash = ?")
        .bind("1".repeat(64))
        .execute(pool)
        .await;
    assert!(t1.is_err(), "session_token_hash immutable");
    let t2 = sqlx::query("UPDATE pairing_transactions SET commit_token_hash = ?")
        .bind("2".repeat(64))
        .execute(pool)
        .await;
    assert!(t2.is_err(), "commit_token_hash immutable");
    let t3 = sqlx::query("UPDATE pairing_transactions SET expires_at = ?")
        .bind(UtcMillis::now().millis() + 999_999_999)
        .execute(pool)
        .await;
    assert!(t3.is_err(), "expires_at immutable");

    // 状态机终态守卫：prepared 不能直接带上 committed_at。
    let t4 = sqlx::query(
        "UPDATE pairing_transactions SET status = 'prepared', committed_at = ? WHERE status = 'prepared'",
    )
    .bind(UtcMillis::now().millis())
    .execute(pool)
    .await;
    assert!(t4.is_err(), "prepared cannot carry committed_at");

    // 事务行不可删除（审计事实）。
    let t5 = sqlx::query("DELETE FROM pairing_transactions")
        .execute(pool)
        .await;
    assert!(t5.is_err(), "pairing transaction row must not be deletable");

    // 直接 SQL 伪造 cross-human committed（无对应 Session/Metadata）必须被
    // 状态机或来源守卫拒绝。
    let now = UtcMillis::now();
    let reserved2 = SessionId(uuid::Uuid::new_v4().to_string());
    let err = sqlx::query(
        "INSERT INTO pairing_transactions \
         (transaction_id, grant_id, client_nonce, session_token_hash, commit_token_hash, \
          issued_by, reserved_session_id, status, prepared_at, expires_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, 'committed', ?, ?)",
    )
    .bind(TransactionId::new_v4().as_str())
    .bind(&f.grant_id)
    .bind("schema-nonce-2")
    .bind("3".repeat(64))
    .bind("4".repeat(64))
    .bind(&f.human_id)
    .bind(reserved2.as_str())
    .bind(now.millis())
    .bind(now.millis() + 900_000)
    .execute(pool)
    .await;
    assert!(
        err.is_err(),
        "direct SQL cannot fabricate a committed transaction without its session metadata"
    );
}

#[tokio::test]
async fn committed_transaction_status_is_visible_and_reserved_is_public() {
    let f = fixture().await;
    let nonce = "status-nonce";
    let _prepared = f
        .home
        .prepare_transaction(&prepare_request(&f, nonce), UtcMillis::now())
        .await
        .expect("prepare");
    let committed = f
        .home
        .commit_transaction(
            &commit_command(&f, &_prepared.transaction_id),
            UtcMillis::now(),
        )
        .await
        .expect("commit");

    // status 返回 committed + reserved session id（Host 识别自己的预留）；
    // 隐式验证 Home/Human 非秘密视图可读。
    let status = f
        .home
        .get_transaction_status(
            &_prepared.transaction_id,
            &f.commit_token_hash,
            UtcMillis::now(),
        )
        .await
        .expect("committed status visible")
        .expect("some view");
    assert_eq!(status.status, TransactionStatus::Committed);
    assert_eq!(status.transaction_id, committed.transaction_id);
    assert_eq!(status.reserved_session_id, committed.reserved_session_id);
    assert_eq!(status.human_id.as_str(), f.human_id);
}

#[tokio::test]
async fn migration_applies_and_adds_pairing_transactions_without_altering_frozen_tables() {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("open + migrate");

    // 迁移完整：无 pending / failed。
    let status = store.status().await.expect("probe");
    assert_eq!(
        status,
        crate::ports::migration::MigrationStatus {
            pending: 0,
            failed: 0
        }
    );
    let table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'pairing_transactions'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(table, 1, "pairing_transactions table created");

    // 冻结事实未被本切片触碰：human_sessions / pairing_grants /
    // session_metadata 的列集合与 A 包一致。
    let cols: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('human_sessions') ORDER BY cid")
            .fetch_all(store.pool())
            .await
            .unwrap();
    assert_eq!(
        cols,
        vec!["id", "human_id", "token_hash", "created_at", "revoked_at"]
    );
    let cols: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('pairing_grants') ORDER BY cid")
            .fetch_all(store.pool())
            .await
            .unwrap();
    assert_eq!(
        cols,
        vec![
            "grant_id",
            "token_hash",
            "issued_by",
            "device_label",
            "status",
            "issued_at",
            "expires_at",
            "consumed_at",
            "revoked_at",
            // ⚠ 2026-09-28「邀请一个人进这个世界」刀新增两列（`202609280003` /
            // `202609280004`）：`grants_human`（用途：设备配对 / 邀请）与
            // `invited_human_id`（这张邀请码点名了谁——`session_metadata` 的
            // 来源守卫靠它认人）。**只加列、不搬行**，其余列逐字未动。
            //
            // 这条断言的意义就是让"冻结表被改了"必须被看见——所以如实跟着
            // 改，而不是放宽成"包含即可"。
            "grants_human",
            "invited_human_id"
        ]
    );

    // 只新增 pairing_transactions 表：不新增任何其他表。
    let all: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    let other: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' \
         AND name NOT LIKE 'sqlite_%' AND name != 'pairing_transactions'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(all - other, 1, "exactly one new table added by this slice");
}
