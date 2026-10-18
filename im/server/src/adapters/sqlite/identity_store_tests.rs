//! SQLite 身份 adapter 测试：真实临时数据库上的 bootstrap、补发、认证、
//! 撤销、并发唯一与 schema 兜底。

use std::sync::Arc;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::application::identity::{bootstrap_human, issue_human_session};
use crate::domain::human_session::TokenHash;
use crate::ports::identity::IdentityStore as _;

struct Fixture {
    _dir: tempfile::TempDir,
    db_path: std::path::PathBuf,
    store: MigrationStore,
    identity: SqliteIdentityStore,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("core.db");
    let store = MigrationStore::open(&db_path)
        .await
        .expect("open + migrate");
    let identity = SqliteIdentityStore::new(store.pool().clone());
    Fixture {
        _dir: dir,
        db_path,
        store,
        identity,
    }
}

async fn count_participants(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM participants")
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

async fn count_sessions(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM human_sessions")
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn bootstrap_creates_human_and_session_without_plaintext_token() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice")
        .await
        .expect("first bootstrap");

    assert_eq!(count_participants(&f).await, 1);
    assert_eq!(count_sessions(&f).await, 1);

    let (kind, status, display_name): (String, String, String) =
        sqlx::query_as("SELECT kind, status, display_name FROM participants")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(kind, "human");
    assert_eq!(status, "active");
    assert_eq!(display_name, "Alice");

    let (human_id, token_hash, revoked_at): (String, String, Option<i64>) =
        sqlx::query_as("SELECT human_id, token_hash, revoked_at FROM human_sessions")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(human_id, outcome.participant.id.as_str());
    assert_eq!(revoked_at, None);
    assert_eq!(token_hash.len(), 64, "fixed-length hash in DB");
    assert_ne!(
        token_hash,
        outcome.token.as_str(),
        "raw token must not be stored"
    );
    assert_eq!(
        TokenHash::parse_hex(&token_hash),
        Some(outcome.session.token_hash)
    );

    let raw = std::fs::read(&f.db_path).unwrap();
    assert!(
        !raw.windows(outcome.token.as_str().len())
            .any(|w| w == outcome.token.as_str().as_bytes()),
        "token bytes must not appear anywhere in the db file"
    );
}

#[tokio::test]
async fn duplicate_bootstrap_fails_and_adds_nothing() {
    let f = fixture().await;
    let first = bootstrap_human(&f.identity, "Alice")
        .await
        .expect("first bootstrap");

    let err = bootstrap_human(&f.identity, "Bob").await.unwrap_err();
    assert!(
        matches!(
            err,
            crate::application::identity::BootstrapError::AlreadyBootstrapped
        ),
        "expected explicit conflict, got {err}"
    );

    assert_eq!(count_participants(&f).await, 1);
    assert_eq!(count_sessions(&f).await, 1);

    let stored_hash: String = sqlx::query_scalar("SELECT token_hash FROM human_sessions")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert!(
        !stored_hash.contains(first.token.as_str()),
        "no token leakage from failed retry"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_bootstrap_exactly_one_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .unwrap();
    let identity = Arc::new(SqliteIdentityStore::new(store.pool().clone()));
    let f = Fixture {
        _dir: dir,
        db_path: std::path::PathBuf::new(),
        store,
        identity: identity.as_ref().clone(),
    };

    let mut tasks = Vec::new();
    for _ in 0..10 {
        let identity = identity.clone();
        tasks.push(tokio::spawn(async move {
            bootstrap_human(identity.as_ref(), "Alice").await
        }));
    }
    let mut successes = 0;
    let mut conflicts = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(_) => successes += 1,
            Err(crate::application::identity::BootstrapError::AlreadyBootstrapped) => {
                conflicts += 1
            }
            Err(e) => panic!("unexpected concurrent bootstrap error: {e}"),
        }
    }
    assert_eq!(successes, 1, "exactly one bootstrap must win");
    assert_eq!(conflicts, 9);
    assert_eq!(count_participants(&f).await, 1);
    assert_eq!(count_sessions(&f).await, 1);
}

#[tokio::test]
async fn issue_session_succeeds_for_active_human_and_leaves_original_intact() {
    let f = fixture().await;
    let first = bootstrap_human(&f.identity, "Alice").await.unwrap();

    let issued = issue_human_session(&f.identity, first.participant.id.as_str())
        .await
        .expect("active human must be issuable");
    assert_ne!(issued.token, first.token, "resend must create a new token");

    assert_eq!(count_sessions(&f).await, 2);
    assert_eq!(count_participants(&f).await, 1);

    let revoked: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM human_sessions WHERE revoked_at IS NOT NULL")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(revoked, 0, "issuing must not revoke other sessions");

    let ctx = crate::application::identity::authenticate(&f.identity, issued.token.as_str())
        .await
        .expect("new token must authenticate");
    assert_eq!(ctx.participant.id, first.participant.id);
}

#[tokio::test]
async fn issue_session_fails_for_unknown_human() {
    let f = fixture().await;
    bootstrap_human(&f.identity, "Alice").await.unwrap();

    for bad in ["does-not-exist", "not-a-uuid"] {
        let err = issue_human_session(&f.identity, bad).await.unwrap_err();
        assert!(
            matches!(
                err,
                crate::application::identity::IssueSessionError::UnknownHuman
            ),
            "expected UnknownHuman for {bad:?}, got {err}"
        );
    }
    assert_eq!(count_sessions(&f).await, 1);
    assert_eq!(count_participants(&f).await, 1);
}

#[tokio::test]
async fn authenticate_returns_context_for_valid_token_and_none_for_unknown_hash() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();

    let ctx = crate::application::identity::authenticate(&f.identity, outcome.token.as_str())
        .await
        .expect("valid token");
    assert_eq!(ctx.participant.id, outcome.participant.id);
    assert_eq!(ctx.participant.kind.as_str(), "human");
    assert_eq!(ctx.participant.status.as_str(), "active");
    assert_eq!(ctx.session.id, outcome.session.id);
    assert_eq!(ctx.session.revoked_at, None);

    let random_hash = TokenHash([7u8; 32]);
    assert!(
        f.identity
            .authenticate(&random_hash)
            .await
            .unwrap()
            .is_none(),
        "unknown hash must not authenticate"
    );
}

#[tokio::test]
async fn revoked_session_stops_authenticating_but_human_and_other_sessions_survive() {
    let f = fixture().await;
    let first = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let second = issue_human_session(&f.identity, first.participant.id.as_str())
        .await
        .unwrap();

    crate::application::identity::revoke_session(&f.identity, &first.session.id)
        .await
        .expect("revoke must persist");

    let err = crate::application::identity::authenticate(&f.identity, first.token.as_str())
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            crate::application::identity::AuthenticateError::UnknownSession
        ),
        "revoked token must fail closed"
    );
    let ctx = crate::application::identity::authenticate(&f.identity, second.token.as_str())
        .await
        .expect("other session must remain valid");
    assert_eq!(ctx.participant.id, first.participant.id);

    assert_eq!(
        count_participants(&f).await,
        1,
        "revoke must not delete the human"
    );
    assert_eq!(count_sessions(&f).await, 2);

    let revoked: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM human_sessions WHERE revoked_at IS NOT NULL")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(revoked, 1);
}

#[tokio::test]
async fn revoke_is_idempotent() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    for _ in 0..2 {
        crate::application::identity::revoke_session(&f.identity, &outcome.session.id)
            .await
            .expect("revoke must be idempotent");
    }
    assert_eq!(count_participants(&f).await, 1);
}

#[tokio::test]
async fn db_checks_reject_invalid_display_names_directly() {
    let f = fixture().await;
    for display_name in ["   ", "al\u{0}ice", &"a".repeat(81)] {
        let result = sqlx::query(
            "INSERT INTO participants (id, kind, status, display_name, created_at) \
                                  VALUES ('x', 'human', 'active', ?, 0)",
        )
        .bind(display_name)
        .execute(f.store.pool())
        .await;
        assert!(
            result.is_err(),
            "expected DB CHECK rejection for {display_name:?}"
        );
    }
    assert_eq!(count_participants(&f).await, 0);
}

#[tokio::test]
async fn db_enforces_fixed_length_unique_token_hash() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();

    let bad_length = sqlx::query(
        "INSERT INTO human_sessions (id, human_id, token_hash, created_at) \
         VALUES ('s-bad-len', ?, 'short', 0)",
    )
    .bind(outcome.participant.id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(bad_length.is_err(), "fixed-length hash CHECK must fire");

    let duplicate = sqlx::query(
        "INSERT INTO human_sessions (id, human_id, token_hash, created_at) \
         VALUES ('s-dup', ?, ?, 0)",
    )
    .bind(outcome.participant.id.as_str())
    .bind(outcome.session.token_hash.hex())
    .execute(f.store.pool())
    .await;
    assert!(duplicate.is_err(), "token hash UNIQUE must fire");
}

#[tokio::test]
async fn bootstrap_violates_nothing_on_bad_display_name() {
    let f = fixture().await;
    let err = bootstrap_human(&f.identity, "   ").await.unwrap_err();
    assert!(
        matches!(
            err,
            crate::application::identity::BootstrapError::DisplayName(_)
        ),
        "expected display name error, got {err}"
    );
    assert_eq!(count_participants(&f).await, 0);
    assert_eq!(count_sessions(&f).await, 0);
    assert!(
        f.identity
            .authenticate(&TokenHash([1u8; 32]))
            .await
            .unwrap()
            .is_none(),
        "unknown hash must not authenticate"
    );
}

#[tokio::test]
async fn create_human_creates_independent_human_and_session_without_guard() {
    let f = fixture().await;
    // 先 bootstrap 首位 Human（占 guard）。
    let first = bootstrap_human(&f.identity, "Alice")
        .await
        .expect("bootstrap");

    // create-human：不占 guard，可重复创建多个独立 Human。
    let second = crate::application::identity::create_human(&f.identity, "Bob")
        .await
        .expect("create human");
    let third = crate::application::identity::create_human(&f.identity, "Carol")
        .await
        .expect("create second human");

    assert_ne!(first.participant.id, second.participant.id);
    assert_ne!(second.participant.id, third.participant.id);
    assert_eq!(count_participants(&f).await, 3);
    assert_eq!(count_sessions(&f).await, 3);
    assert_ne!(second.session.human_id, third.session.human_id);

    // 各自 Session 独立：token 只能认证自己的 Human。
    let second_token = crate::domain::token::OpaqueToken::parse_candidate(second.token.as_str())
        .expect("valid token format");
    let ctx = f
        .identity
        .authenticate(&crate::domain::human_session::TokenHash::from_token(
            &second_token,
        ))
        .await
        .expect("auth");
    let ctx = ctx.expect("session resolves");
    assert_eq!(ctx.participant.id, second.participant.id);
    assert_eq!(ctx.participant.display_name.as_str(), "Bob");

    // 显示名非法 → 明确失败且不创建 Human。
    let err = crate::application::identity::create_human(&f.identity, "   ")
        .await
        .expect_err("blank display name must fail");
    assert!(matches!(
        err,
        crate::application::identity::CreateHumanError::DisplayName(_)
    ));
    assert_eq!(count_participants(&f).await, 3);
}

#[tokio::test]
async fn cancelled_bootstrap_rolls_back_before_connection_reuse() {
    let f = fixture().await;
    // 单连接池保证 abort 后的查询和下一次 bootstrap 必须复用同一物理
    // 连接；如果 Transaction Drop 没有在归池前回滚，后续写会撞上遗留的
    // BEGIN IMMEDIATE 写锁或嵌套事务。
    let options = SqliteConnectOptions::new()
        .filename(f._dir.path().join("core.db"))
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5));
    let cancel_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open single-connection cancellation pool");
    let cancel_identity = SqliteIdentityStore::new(cancel_pool.clone());
    let next_identity = cancel_identity.clone();

    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
    let (released_tx, released_rx) = tokio::sync::oneshot::channel();
    cancel_identity.arm_commit_checkpoint(reached_tx, released_rx);

    // bootstrap 写事务在「guard + participant + session 已写入、commit 前」
    // 挂起，等待测试 abort。
    let handle = tokio::spawn(async move { bootstrap_human(&cancel_identity, "Alice").await });

    tokio::time::timeout(std::time::Duration::from_secs(10), reached_rx)
        .await
        .expect("bootstrap must reach the commit checkpoint in time")
        .expect("bootstrap must not fail before the checkpoint");
    handle.abort();
    let join_error = handle.await.expect_err("task must be cancelled");
    assert!(join_error.is_cancelled());
    drop(released_tx);

    // acquire 会等待唯一连接归池，并触发 SQLx 在 Transaction Drop 时排队
    // 的 rollback；未提交事实必须消失。
    let participants: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM participants")
        .fetch_one(&cancel_pool)
        .await
        .expect("same pooled connection must recover after cancellation");
    assert_eq!(participants, 0, "cancelled bootstrap must leave no human");
    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM human_sessions")
        .fetch_one(&cancel_pool)
        .await
        .expect("same pooled connection must recover after cancellation");
    assert_eq!(sessions, 0, "cancelled bootstrap must leave no session");

    bootstrap_human(&next_identity, "Alice")
        .await
        .expect("next bootstrap on the same single connection must succeed");
    let participants: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM participants")
        .fetch_one(&cancel_pool)
        .await
        .expect("same pooled connection must stay usable");
    assert_eq!(participants, 1);
}
