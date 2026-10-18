//! SQLite Home adapter 测试：真实临时数据库上的 Home 初始化、Grant 签发/
//! 兑换/撤销、Session Metadata 1:1、Session 管理隔离与 schema 兜底。

use std::time::Duration;

use tokio::sync::oneshot;

use crate::adapters::sqlite::home_store::SqliteHomeStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::application::home;
use crate::application::identity;
use crate::domain::grant_token::GrantToken;
use crate::domain::home::{GrantId, GrantStatus};
use crate::domain::human_session::{SessionId, TokenHash};
use crate::domain::time::UtcMillis;
use crate::ports::home::{HomeStore as _, StoreError};

struct Fixture {
    _dir: tempfile::TempDir,
    db_path: std::path::PathBuf,
    store: MigrationStore,
    home: SqliteHomeStore,
    identity: SqliteIdentityStore,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("core.db");
    let store = MigrationStore::open(&db_path)
        .await
        .expect("open + migrate");
    let home = SqliteHomeStore::new(store.pool().clone());
    let identity = SqliteIdentityStore::new(store.pool().clone());
    Fixture {
        _dir: dir,
        db_path,
        store,
        home,
        identity,
    }
}

async fn count_rows(f: &Fixture, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

async fn bootstrap(f: &Fixture, name: &str) -> (String, String) {
    home::init_home(&f.home, None)
        .await
        .expect("initialize home");
    let outcome = identity::bootstrap_human(&f.identity, name)
        .await
        .expect("bootstrap");
    (
        outcome.participant.id.as_str().to_owned(),
        outcome.token.as_str().to_owned(),
    )
}

async fn create_human(f: &Fixture, name: &str) -> (String, String) {
    let outcome = identity::create_human(&f.identity, name)
        .await
        .expect("create human");
    (
        outcome.participant.id.as_str().to_owned(),
        outcome.token.as_str().to_owned(),
    )
}

/// 通过真实应用用例签发 Grant，返回 (grant_id, raw token)。
async fn issue(f: &Fixture, human_id: &str, label: Option<&str>) -> (String, String) {
    let issued = home::issue_grant(
        &f.home,
        &crate::domain::participant::ParticipantId(human_id.to_owned()),
        label,
        crate::domain::home::GrantKind::SelfDevice,
        15,
    )
    .await
    .expect("issue grant");
    (
        issued.grant.grant_id.as_str().to_owned(),
        issued.token.as_str().to_owned(),
    )
}

/// 通过真实应用用例兑换 Grant（device label 可选），返回 ExchangeResult。
async fn exchange(
    f: &Fixture,
    token: &str,
    label: Option<&str>,
) -> Result<home::ExchangeResult, home::ExchangeError> {
    exchange_store(&f.home, token, label).await
}

async fn exchange_store(
    store: &SqliteHomeStore,
    token: &str,
    label: Option<&str>,
) -> Result<home::ExchangeResult, home::ExchangeError> {
    home::exchange_grant(store, token, label, None).await
}

/// owned 变体：供跨 task 借用的并发/回滚测试（store 与 token 都是
/// Clone/owned，避免 'static 借用问题）。
async fn exchange_store_owned(
    store: SqliteHomeStore,
    token: String,
) -> Result<home::ExchangeResult, home::ExchangeError> {
    home::exchange_grant(&store, &token, None, None).await
}

// ---------- Home Identity ----------

#[tokio::test]
async fn home_init_once_then_immutable_and_readable() {
    let f = fixture().await;
    assert_eq!(count_rows(&f, "home").await, 0);
    let created = home::init_home(&f.home, Some("My Home"))
        .await
        .expect("init");
    assert!(created, "first init creates the home");
    assert_eq!(count_rows(&f, "home").await, 1);

    // 幂等：再次初始化成功但不再创建。
    let again = home::init_home(&f.home, None).await.expect("idempotent");
    assert!(!again, "second init must be a no-op");
    assert_eq!(count_rows(&f, "home").await, 1);

    let stored = f.home.get_home().await.expect("get").expect("home exists");
    assert_eq!(stored.display_name.as_ref().unwrap().as_str(), "My Home");
    // UUID v4 形状。
    let uuid = uuid::Uuid::parse_str(stored.home_id.as_str()).unwrap();
    assert_eq!(uuid.get_version(), Some(uuid::Version::Random));

    // 第二次 init 不改变既有身份（immutable）。
    let stored_again = f.home.get_home().await.expect("get").expect("home");
    assert_eq!(stored_again.home_id, stored.home_id);
}

#[tokio::test]
async fn home_row_rejects_update_delete_and_replace_direct_sql() {
    let f = fixture().await;
    home::init_home(&f.home, None).await.expect("init");
    let stored = f.home.get_home().await.expect("get").expect("home");
    let pool = f.store.pool();

    let update = sqlx::query("UPDATE home SET home_id = ? WHERE id = 1")
        .bind("11111111-2222-4333-8444-555555555555")
        .execute(pool)
        .await;
    assert!(update.is_err(), "home row must be immutable");

    let delete = sqlx::query("DELETE FROM home WHERE id = 1")
        .execute(pool)
        .await;
    assert!(delete.is_err(), "home row must not be deletable");

    let replace = sqlx::query("INSERT OR REPLACE INTO home (id, home_id, display_name, created_at) VALUES (1, ?, NULL, 0)")
        .bind("11111111-2222-4333-8444-555555555555")
        .execute(pool)
        .await;
    assert!(
        replace.is_err(),
        "REPLACE must not bypass the immutability guard"
    );

    let still = f.home.get_home().await.expect("get").expect("home");
    assert_eq!(still.home_id, stored.home_id, "identity must not change");
}

#[tokio::test]
async fn home_init_failure_leaves_no_half_row() {
    let f = fixture().await;
    // guard 占用后直接插入 home 行：模拟「guard 写入成功、home 写入失败」
    // 的事务半途状态不可能出现；这里验证 guard 冲突路径不留下行。
    let _ = home::init_home(&f.home, None).await;
    assert_eq!(count_rows(&f, "home").await, 1);
    let err = f
        .home
        .init_home(&crate::ports::home::NewHome {
            home_id: crate::domain::home::HomeId::new_v4(),
            display_name: None,
            created_at: UtcMillis::now(),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::AlreadyInitialized));
    assert_eq!(count_rows(&f, "home").await, 1, "no second row");
}

#[tokio::test]
async fn home_init_validates_display_name() {
    let f = fixture().await;
    let err = home::init_home(&f.home, Some("\u{0}")).await.unwrap_err();
    assert!(
        matches!(err, home::InitHomeError::DisplayName(_)),
        "NUL display name must fail: {err}"
    );
    assert_eq!(count_rows(&f, "home").await, 0);
}

// ---------- Grant 签发 ----------

#[tokio::test]
async fn issue_stores_only_sha256_digest_and_token_never_in_db() {
    let f = fixture().await;
    let (human_id, _) = bootstrap(&f, "Alice").await;
    let (_grant_id, token) = issue(&f, &human_id, Some("phone")).await;

    let (stored_hash, issued_by, device_label): (String, String, Option<String>) =
        sqlx::query_as("SELECT token_hash, issued_by, device_label FROM pairing_grants")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(stored_hash.len(), 64, "SHA-256 hex");
    assert_ne!(stored_hash, token, "raw token must not be stored");
    assert_eq!(
        stored_hash,
        TokenHash::from_token_str(token.as_str()).hex(),
        "digest must match the raw token"
    );
    assert_eq!(issued_by, human_id);
    assert_eq!(device_label.as_deref(), Some("phone"));

    let raw = std::fs::read(&f.db_path).unwrap();
    assert!(
        !raw.windows(token.len()).any(|w| w == token.as_bytes()),
        "grant token bytes must not appear in the db file"
    );
}

#[tokio::test]
async fn issue_requires_active_human() {
    let f = fixture().await;
    let unknown = home::issue_grant(
        &f.home,
        &crate::domain::participant::ParticipantId(uuid::Uuid::new_v4().to_string()),
        None,
        crate::domain::home::GrantKind::SelfDevice,
        15,
    )
    .await
    .unwrap_err();
    assert!(matches!(unknown, home::IssueGrantError::UnknownHuman));
    assert_eq!(count_rows(&f, "pairing_grants").await, 0);

    let (human_id, _) = bootstrap(&f, "Alice").await;
    let bad_label = home::issue_grant(
        &f.home,
        &crate::domain::participant::ParticipantId(human_id.clone()),
        Some("\u{0}"),
        crate::domain::home::GrantKind::SelfDevice,
        15,
    )
    .await
    .unwrap_err();
    assert!(matches!(bad_label, home::IssueGrantError::DeviceLabel(_)));
    assert_eq!(count_rows(&f, "pairing_grants").await, 0);
}

#[tokio::test]
async fn grant_token_entropy_and_format_hygiene() {
    // 每个 token ≥256 bit 熵、稳定前缀、固定长度；格式解析严格。
    let tokens: Vec<GrantToken> = (0..8).map(|_| GrantToken::generate().unwrap()).collect();
    for t in &tokens {
        let s = t.as_str();
        assert!(s.starts_with("clr_pg_"), "stable grant prefix");
        assert_eq!(s.len(), 7 + 43, "prefix + 256-bit base64url");
        let rest = &s[7..];
        assert!(
            rest.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "base64url alphabet"
        );
        assert_eq!(GrantToken::parse_candidate(s), Some(t.clone()));
        assert!(format!("{t:?}").contains("[REDACTED]"), "Debug must redact");
    }
    // 互异性（熵证明）。
    let unique: std::collections::HashSet<String> =
        tokens.iter().map(|t| t.as_str().to_owned()).collect();
    assert_eq!(unique.len(), tokens.len(), "no collisions among samples");

    // 格式拒绝：错误前缀、错误长度、非法字符。
    assert!(GrantToken::parse_candidate("clr_pg_AAAA").is_none());
    assert!(GrantToken::parse_candidate("clr_hs_xxxx").is_none());
    assert!(GrantToken::parse_candidate("pg_").is_none());
    let mut bad = tokens[0].as_str().to_owned();
    bad.push('x');
    assert!(GrantToken::parse_candidate(&bad).is_none());
    let mut bad2 = tokens[0].as_str().to_owned();
    bad2.pop();
    assert!(GrantToken::parse_candidate(&bad2).is_none());
}

// ---------- 兑换 ----------

#[tokio::test]
async fn exchange_creates_independent_session_bound_to_issuer_with_metadata() {
    let f = fixture().await;
    let (human_id, _) = bootstrap(&f, "Alice").await;
    let (grant_id, token) = issue(&f, &human_id, None).await;

    let sessions_before = count_rows(&f, "human_sessions").await;
    let result = exchange(&f, &token, Some("work-laptop"))
        .await
        .expect("exchange");
    let outcome = result.outcome;

    assert_eq!(
        outcome.human_id.as_str(),
        human_id,
        "session human must equal grant issued_by"
    );
    assert_eq!(sessions_before + 1, count_rows(&f, "human_sessions").await);
    assert_eq!(count_rows(&f, "session_metadata").await, 1);

    // 新 Session 的 token 是全新随机 token（不是 grant token 复用）。
    assert_ne!(result.session_token.as_str(), token);
    assert!(result.session_token.as_str().starts_with("clr_hs_"));
    assert_eq!(
        result.session_token.as_str().len(),
        7 + 43,
        "session token shape"
    );

    // metadata 1:1：device label + source grant；无 token/hash。
    let (session_id, device_label, source_grant_id, token_hash): (
        String,
        Option<String>,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT m.session_id, m.device_label, m.source_grant_id, s.token_hash \
             FROM session_metadata m JOIN human_sessions s ON s.id = m.session_id",
    )
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(session_id, outcome.session_id.as_str());
    assert_eq!(device_label.as_deref(), Some("work-laptop"));
    assert_eq!(source_grant_id, grant_id);
    assert_eq!(
        token_hash,
        TokenHash::from_token_str(result.session_token.as_str()).hex()
    );
    assert_eq!(token_hash.len(), 64);
    assert!(
        !token_hash.contains(result.session_token.as_str()),
        "no token text in hash column"
    );

    // grant 已消费；同 token 重放统一 grant_invalid，绝不新建/重发 Session。
    let (status, consumed_at): (String, Option<i64>) =
        sqlx::query_as("SELECT status, consumed_at FROM pairing_grants WHERE grant_id = ?")
            .bind(&grant_id)
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(status, "consumed");
    assert!(consumed_at.is_some());
    let replay = exchange(&f, &token, None).await.unwrap_err();
    assert!(matches!(replay, home::ExchangeError::GrantInvalid));
    assert_eq!(
        count_rows(&f, "human_sessions").await,
        sessions_before + 1,
        "replay must not create a second session"
    );

    // 新 Session 真实可用：认证通过。
    let auth = identity::authenticate(&f.identity, result.session_token.as_str())
        .await
        .expect("authenticate");
    assert_eq!(auth.participant.id.as_str(), human_id);
}

#[tokio::test]
async fn exchange_rejects_all_invalid_shapes_uniformly() {
    let f = fixture().await;
    let (human_id, _) = bootstrap(&f, "Alice").await;
    let (_, _token) = issue(&f, &human_id, None).await;

    // 未知（随机但格式合法）。
    let unknown = GrantToken::generate().unwrap();
    let err = exchange(&f, unknown.as_str(), None).await.unwrap_err();
    assert!(matches!(err, home::ExchangeError::GrantInvalid));
    // 格式错误（短、错前缀、乱字符）。
    for bad in [
        "clr_pg_ab",
        "clr_pg_",
        "oops",
        "",
        "clr_hs_ABCDEFGHIJKLMNOPQRSTUVWXYZ234567",
    ] {
        let err = exchange(&f, bad, None).await.unwrap_err();
        assert!(
            matches!(err, home::ExchangeError::GrantInvalid),
            "bad format {bad:?} must be grant_invalid"
        );
    }
    // Human Session token 走 exchange 一律 grant_invalid（不能以 Session 认证）。
    let (_, human_token) = create_human(&f, "Bob").await;
    let sessions_before_invalid_attempts = count_rows(&f, "human_sessions").await;
    let err = exchange(&f, &human_token, None).await.unwrap_err();
    assert!(matches!(err, home::ExchangeError::GrantInvalid));

    // 撤销后不可兑换。
    let (grant_id, token2) = issue(&f, &human_id, None).await;
    home::revoke_grant(
        &f.home,
        &crate::domain::participant::ParticipantId(human_id.clone()),
        &GrantId(grant_id),
    )
    .await
    .expect("revoke");
    let err = exchange(&f, &token2, None).await.unwrap_err();
    assert!(matches!(err, home::ExchangeError::GrantInvalid));

    // 过期不可兑换（直接落库一个已过期 grant，走真实查询路径）。
    let now = UtcMillis::now();
    let past_token = GrantToken::generate().unwrap();
    sqlx::query(
        "INSERT INTO pairing_grants (grant_id, token_hash, issued_by, status, issued_at, expires_at) \
         VALUES (?, ?, ?, 'active', ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(TokenHash::from_token_str(past_token.as_str()).hex())
    .bind(&human_id)
    .bind(now.millis() - 60_000)
    .bind(now.millis() - 1)
    .execute(f.store.pool())
    .await
    .unwrap();
    let err = exchange(&f, past_token.as_str(), None).await.unwrap_err();
    assert!(matches!(err, home::ExchangeError::GrantInvalid));
    assert_eq!(
        count_rows(&f, "human_sessions").await,
        sessions_before_invalid_attempts,
        "no session created by any invalid shape"
    );
}

#[tokio::test]
async fn exchange_rolls_back_entirely_when_aborted_mid_transaction() {
    let f = fixture().await;
    let (human_id, _) = bootstrap(&f, "Alice").await;
    let (_, token) = issue(&f, &human_id, None).await;
    let sessions_before = count_rows(&f, "human_sessions").await;

    // arm 兑换事务的 commit-checkpoint：事务已写入、commit 前暂停。
    let (reached_tx, reached_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    f.home.arm_commit_checkpoint(reached_tx, release_rx);

    let exchange_task = tokio::spawn(exchange_store_owned(f.home.clone(), token.clone()));
    reached_rx
        .await
        .expect("exchange transaction reached commit pause");
    // 在事务仍打开时 abort future：Transaction Drop 必须整体回滚。
    exchange_task.abort();
    let _ = release_tx.send(());

    // 等待回滚完成并验证 grant 仍可兑换、无残留行。
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        count_rows(&f, "human_sessions").await,
        sessions_before,
        "aborted exchange must not leave a session row"
    );
    assert_eq!(count_rows(&f, "session_metadata").await, 0);
    let (status,): (String,) = sqlx::query_as("SELECT status FROM pairing_grants")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(status, "active", "grant must stay usable after rollback");

    // 重试成功只产生一个 Session（原子性 + 可重试）。
    let retry = exchange(&f, &token, None)
        .await
        .expect("retry after rollback");
    assert_eq!(count_rows(&f, "human_sessions").await, sessions_before + 1);
    let (status,): (String,) = sqlx::query_as("SELECT status FROM pairing_grants")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(status, "consumed");
    let (revoked_ok,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM human_sessions WHERE token_hash = ?")
            .bind(TokenHash::from_token_str(retry.session_token.as_str()).hex())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(
        revoked_ok, 1,
        "retried session is the only one with that token"
    );
}

// ---------- Grant 列表 / 撤销 ----------

#[tokio::test]
async fn list_grants_shows_own_unconsumed_with_derived_status_and_revoke_is_idempotent() {
    let f = fixture().await;
    let (alice, _) = bootstrap(&f, "Alice").await;
    let (_, token1) = issue(&f, &alice, Some("laptop")).await;
    let (grant2, token2) = issue(&f, &alice, None).await;

    // 消费 token1 后它不再出现在列表；token2 仍在（active）。
    let result = exchange(&f, &token1, None).await.expect("exchange");
    assert!(matches!(result.outcome.grant_id.as_str().len(), 36));
    let listed = home::list_grants(
        &f.home,
        &crate::domain::participant::ParticipantId(alice.clone()),
    )
    .await
    .expect("list");
    assert_eq!(listed.len(), 1, "consumed grant must disappear");
    assert_eq!(listed[0].grant_id.as_str(), grant2);
    assert_eq!(listed[0].status_at(UtcMillis::now()), GrantStatus::Active);
    assert!(listed[0].device_label.is_none());

    // 撤销 grant2：幂等（两次都成功）。
    let gid = GrantId(grant2.clone());
    home::revoke_grant(
        &f.home,
        &crate::domain::participant::ParticipantId(alice.clone()),
        &gid,
    )
    .await
    .expect("first revoke");
    home::revoke_grant(
        &f.home,
        &crate::domain::participant::ParticipantId(alice.clone()),
        &gid,
    )
    .await
    .expect("repeat revoke must be idempotent");

    let listed = home::list_grants(
        &f.home,
        &crate::domain::participant::ParticipantId(alice.clone()),
    )
    .await
    .expect("list");
    assert_eq!(listed.len(), 1, "revoked grant still visible (unconsumed)");
    assert_eq!(listed[0].status_at(UtcMillis::now()), GrantStatus::Revoked);
    let (status, revoked_at): (String, Option<i64>) =
        sqlx::query_as("SELECT status, revoked_at FROM pairing_grants WHERE grant_id = ?")
            .bind(&grant2)
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(status, "revoked");
    assert!(revoked_at.is_some());

    // 已撤销不可兑换。
    let err = exchange(&f, &token2, None).await.unwrap_err();
    assert!(matches!(err, home::ExchangeError::GrantInvalid));

    // 过期 grant 列表显示派生 expired。
    let now = UtcMillis::now();
    let grant3 = uuid::Uuid::new_v4().to_string();
    let token3 = GrantToken::generate().unwrap();
    sqlx::query(
        "INSERT INTO pairing_grants \
         (grant_id, token_hash, issued_by, status, issued_at, expires_at) \
         VALUES (?, ?, ?, 'active', ?, ?)",
    )
    .bind(&grant3)
    .bind(TokenHash::from_token_str(token3.as_str()).hex())
    .bind(&alice)
    .bind(now.millis() - 60_000)
    .bind(now.millis() - 1)
    .execute(f.store.pool())
    .await
    .expect("insert expired grant state");
    let listed = home::list_grants(&f.home, &crate::domain::participant::ParticipantId(alice))
        .await
        .expect("list");
    assert!(
        listed
            .iter()
            .any(|g| g.grant_id.as_str() == grant3 && g.status_at(now) == GrantStatus::Expired)
    );
    let err = exchange(&f, token3.as_str(), None).await.unwrap_err();
    assert!(matches!(err, home::ExchangeError::GrantInvalid));
}

#[tokio::test]
async fn revoke_grant_owner_scoped_and_consumed_invisible() {
    let f = fixture().await;
    let (alice, _) = bootstrap(&f, "Alice").await;
    let (bob, _) = create_human(&f, "Bob").await;
    let (alice_grant, token) = issue(&f, &alice, None).await;
    let (_, token2) = issue(&f, &bob, None).await;

    // 非本人撤销 → NotFound。
    let err = home::revoke_grant(
        &f.home,
        &crate::domain::participant::ParticipantId(bob.clone()),
        &GrantId(alice_grant.clone()),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, home::RevokeGrantError::NotFound));
    // 未知 grant → NotFound。
    let err = home::revoke_grant(
        &f.home,
        &crate::domain::participant::ParticipantId(alice.clone()),
        &GrantId(uuid::Uuid::new_v4().to_string()),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, home::RevokeGrantError::NotFound));

    // 已消费的 grant：不可见、不可撤销（与未知同一错误面）。
    let result = exchange(&f, &token, None)
        .await
        .expect("consume alice grant");
    let err = home::revoke_grant(
        &f.home,
        &crate::domain::participant::ParticipantId(alice),
        &result.outcome.grant_id.clone(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, home::RevokeGrantError::NotFound));

    // 撤销不影响他人 grant。
    let listed = home::list_grants(&f.home, &crate::domain::participant::ParticipantId(bob))
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    let _ = token2;
}

// ---------- Session 管理 ----------

#[tokio::test]
async fn sessions_owner_scoped_list_and_independent_revoke() {
    let f = fixture().await;
    let (alice, alice_token) = bootstrap(&f, "Alice").await;
    let (bob, bob_token) = create_human(&f, "Bob").await;
    let (_, grant_token) = issue(&f, &alice, None).await;
    let result = exchange(&f, &grant_token, Some("device-a"))
        .await
        .expect("exchange");
    let device_session_id = result.outcome.session_id.clone();

    // Alice 有 2 个 Session（bootstrap + 兑换）；Bob 有 1 个。
    let alice_sessions = home::list_sessions(
        &f.home,
        &crate::domain::participant::ParticipantId(alice.clone()),
    )
    .await
    .expect("list");
    assert_eq!(alice_sessions.len(), 2);
    let bob_sessions = home::list_sessions(
        &f.home,
        &crate::domain::participant::ParticipantId(bob.clone()),
    )
    .await
    .expect("list");
    assert_eq!(bob_sessions.len(), 1);

    // 列表只含非秘密元数据：找到设备 Session 并核对 metadata；无 token/hash。
    let device = alice_sessions
        .iter()
        .find(|s| s.session_id == device_session_id)
        .expect("device session listed");
    assert_eq!(device.device_label.as_ref().unwrap().as_str(), "device-a");
    assert_eq!(
        device.source_grant_id,
        Some(result.outcome.grant_id.clone())
    );
    assert!(device.revoked_at.is_none());

    // 撤销设备 Session：Alice 的 bootstrap Session 不受影响。
    home::revoke_owned_session(
        &f.home,
        &crate::domain::participant::ParticipantId(alice.clone()),
        &device_session_id,
    )
    .await
    .expect("revoke device session");
    // 幂等重复撤销。
    home::revoke_owned_session(
        &f.home,
        &crate::domain::participant::ParticipantId(alice.clone()),
        &device_session_id,
    )
    .await
    .expect("repeat revoke idempotent");

    let alice_sessions = home::list_sessions(
        &f.home,
        &crate::domain::participant::ParticipantId(alice.clone()),
    )
    .await
    .expect("list");
    let device = alice_sessions
        .iter()
        .find(|s| s.session_id == device_session_id)
        .expect("revoked session still listed with revoked_at");
    assert!(device.revoked_at.is_some());

    // 设备 Session token 认证失败（已撤销），Alice 原 Session 仍有效。
    let auth = identity::authenticate(&f.identity, result.session_token.as_str())
        .await
        .expect_err("revoked session must fail");
    assert!(matches!(auth, identity::AuthenticateError::UnknownSession));
    let ok = identity::authenticate(&f.identity, &alice_token)
        .await
        .expect("alice bootstrap session still works");
    assert_eq!(ok.participant.id.as_str(), alice);
    let _ = bob_token;
}

#[tokio::test]
async fn revoke_session_owner_scoped_not_found_for_unknown_and_foreign() {
    let f = fixture().await;
    let (alice, _) = bootstrap(&f, "Alice").await;
    let (bob, _) = create_human(&f, "Bob").await;

    // Bob 撤销 Alice 的 bootstrap Session → SessionNotFound。
    let alice_session_id = {
        let outcome = identity::authenticate(
            &f.identity,
            identity::issue_human_session(&f.identity, &alice)
                .await
                .expect("issue")
                .token
                .as_str(),
        )
        .await
        .expect("auth");
        outcome.session.id.clone()
    };
    let err = home::revoke_owned_session(
        &f.home,
        &crate::domain::participant::ParticipantId(bob.clone()),
        &alice_session_id,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, home::RevokeOwnedSessionError::NotFound));

    // 未知 session → NotFound（同一错误面）。
    let err = home::revoke_owned_session(
        &f.home,
        &crate::domain::participant::ParticipantId(bob),
        &SessionId(uuid::Uuid::new_v4().to_string()),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, home::RevokeOwnedSessionError::NotFound));

    // Alice 自己的 session 不受影响。
    let alice_sessions =
        home::list_sessions(&f.home, &crate::domain::participant::ParticipantId(alice))
            .await
            .expect("list");
    assert_eq!(alice_sessions.len(), 2);
    assert!(alice_sessions[0].revoked_at.is_none());
}

// ---------- 数据库守卫（直接 SQL 也不能绕过） ----------

#[tokio::test]
async fn schema_guards_reject_direct_rewrites_of_pairing_grants() {
    let f = fixture().await;
    let (human_id, _) = bootstrap(&f, "Alice").await;
    let (grant_id, _) = issue(&f, &human_id, None).await;
    let pool = f.store.pool();

    // 签发事实不可改写。
    let t1 = sqlx::query("UPDATE pairing_grants SET token_hash = ? WHERE grant_id = ?")
        .bind("a".repeat(64))
        .bind(&grant_id)
        .execute(pool)
        .await;
    assert!(t1.is_err(), "token_hash immutable");
    let t2 = sqlx::query("UPDATE pairing_grants SET issued_by = ? WHERE grant_id = ?")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&grant_id)
        .execute(pool)
        .await;
    assert!(t2.is_err(), "issued_by immutable");
    let t3 = sqlx::query("UPDATE pairing_grants SET expires_at = ? WHERE grant_id = ?")
        .bind(UtcMillis::now().millis() + 999_999_999)
        .bind(&grant_id)
        .execute(pool)
        .await;
    assert!(t3.is_err(), "expires_at immutable");

    // 未消费 grant 同样是不可删除的公共审计事实，不能借 REPLACE/DELETE
    // 抹去已签发但未使用的配对能力。
    let t4 = sqlx::query("DELETE FROM pairing_grants WHERE grant_id = ?")
        .bind(&grant_id)
        .execute(pool)
        .await;
    assert!(t4.is_err(), "active grant must not be deletable");
    let t5 = sqlx::query(
        "UPDATE pairing_grants SET status = 'consumed', consumed_at = ? WHERE grant_id = ?",
    )
    .bind(UtcMillis::now().millis())
    .bind(&grant_id)
    .execute(pool)
    .await;
    assert!(
        t5.is_err(),
        "consumption requires matching session metadata"
    );
    let t6 = sqlx::query("UPDATE pairing_grants SET status = 'expired' WHERE grant_id = ?")
        .bind(&grant_id)
        .execute(pool)
        .await;
    assert!(t6.is_err(), "expired is derived, not persisted");

    // 消费后终态不可变：不能再转换、不能删除。
    let (_, token) = issue(&f, &human_id, None).await;
    let r = exchange(&f, &token, None).await.expect("exchange");
    let gid = r.outcome.grant_id.as_str().to_owned();
    let t7 = sqlx::query(
        "UPDATE pairing_grants SET status = 'active', consumed_at = NULL WHERE grant_id = ?",
    )
    .bind(&gid)
    .execute(pool)
    .await;
    assert!(t7.is_err(), "consumed is terminal");
    let t8 = sqlx::query("DELETE FROM pairing_grants WHERE grant_id = ?")
        .bind(&gid)
        .execute(pool)
        .await;
    assert!(t8.is_err(), "consumed grant row must not be deletable");

    // session_metadata 不可变。
    let t9 = sqlx::query("UPDATE session_metadata SET device_label = ?")
        .bind("hacked")
        .execute(pool)
        .await;
    assert!(t9.is_err(), "session metadata immutable");
    let t10 = sqlx::query("DELETE FROM session_metadata")
        .execute(pool)
        .await;
    assert!(t10.is_err(), "session metadata not deletable");
}

#[tokio::test]
async fn grant_issued_by_must_be_human_by_schema() {
    let f = fixture().await;
    let (human_id, _) = bootstrap(&f, "Alice").await;
    // Agent participant（直接插入，绕过应用层）：签发 grant 必须被
    // 数据库方向触发器拒绝。
    let agent_id = uuid::Uuid::new_v4().to_string();
    let now = UtcMillis::now().millis();
    sqlx::query(
        "INSERT INTO participants (id, kind, status, display_name, created_at) \
         VALUES (?, 'agent', 'active', 'Bot', ?)",
    )
    .bind(&agent_id)
    .bind(now)
    .execute(f.store.pool())
    .await
    .unwrap();
    let err = sqlx::query(
        "INSERT INTO pairing_grants (grant_id, token_hash, issued_by, status, issued_at, expires_at) \
         VALUES (?, ?, ?, 'active', ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind("a".repeat(64))
    .bind(&agent_id)
    .bind(now)
    .bind(now + 60_000)
    .execute(f.store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("issued_by"),
        "direction trigger: {err}"
    );
    assert_eq!(count_rows(&f, "pairing_grants").await, 0);
    let _ = human_id;
}

#[tokio::test]
async fn session_metadata_source_grant_must_belong_to_session_human_by_schema() {
    let f = fixture().await;
    let (alice, _) = bootstrap(&f, "Alice").await;
    let (bob, _) = create_human(&f, "Bob").await;
    let (alice_grant, _token) = issue(&f, &alice, None).await;

    // 绕过应用层直接为 Bob 的既有 Session 伪造 Alice Grant 来源，必须被
    // SQLite 的跨表归属守卫拒绝。
    let bob_session_id: String = sqlx::query_scalar(
        "SELECT id FROM human_sessions WHERE human_id = ? ORDER BY created_at, id LIMIT 1",
    )
    .bind(&bob)
    .fetch_one(f.store.pool())
    .await
    .expect("Bob session");
    let err = sqlx::query(
        "INSERT INTO session_metadata (session_id, device_label, source_grant_id) VALUES (?, ?, ?)",
    )
    .bind(&bob_session_id)
    .bind("forged-cross-human-source")
    .bind(&alice_grant)
    .execute(f.store.pool())
    .await
    .expect_err("cross-human grant source must be rejected");
    assert!(
        err.to_string().contains("source grant must belong"),
        "source ownership trigger: {err}"
    );
    assert_eq!(count_rows(&f, "session_metadata").await, 0);
}

// ---------- 并发 ----------

#[tokio::test]
async fn concurrent_exchange_of_same_grant_creates_exactly_one_session() {
    let f = fixture().await;
    let (human_id, _) = bootstrap(&f, "Alice").await;
    let (_, token) = issue(&f, &human_id, None).await;
    let sessions_before = count_rows(&f, "human_sessions").await;
    // 单连接池（max 1）+ BEGIN IMMEDIATE：两次并发兑换串行化，
    // 第二次必然看到 consumed → GrantInvalid。
    let pool = f.store.pool();
    let _ = pool;
    let t1 = token.clone();
    let t2 = token;
    let h1 = tokio::spawn(exchange_store_owned(f.home.clone(), t1));
    let h2 = tokio::spawn(exchange_store_owned(f.home.clone(), t2));
    let (r1, r2) = tokio::join!(h1, h2);
    let ok_count = [r1, r2]
        .iter()
        .filter(|r| r.as_ref().unwrap().is_ok())
        .count();
    assert_eq!(ok_count, 1, "exactly one exchange may win");
    assert_eq!(
        count_rows(&f, "human_sessions").await,
        sessions_before + 1,
        "exactly one session created"
    );
}

// ---------- 邀请一个人进这个世界（GrantKind::NewHuman） ----------

/// 邀请类签发：`NewHuman` + 明显更长的 TTL（72 小时），其余形状与设备配对同。
async fn issue_invite(f: &Fixture, human_id: &str) -> (String, String) {
    let issued = home::issue_grant(
        &f.home,
        &crate::domain::participant::ParticipantId(human_id.to_owned()),
        None,
        crate::domain::home::GrantKind::NewHuman,
        15,
    )
    .await
    .expect("issue invite grant");
    (
        issued.grant.grant_id.as_str().to_owned(),
        issued.token.as_str().to_owned(),
    )
}

/// 邀请类兑换：**被邀请者自己给的名字**。
async fn exchange_invite(
    f: &Fixture,
    token: &str,
    name: &str,
) -> Result<home::ExchangeResult, home::ExchangeError> {
    home::exchange_grant(&f.home, token, None, Some(name)).await
}

/// 邀请码兑换出一个**新的 active Human**，会话归它自己——而不是签发者。
#[tokio::test]
async fn invite_grant_creates_a_new_active_human_with_its_own_session() {
    let f = fixture().await;
    let (inviter_id, _) = bootstrap(&f, "Alice").await;
    let participants_before = count_rows(&f, "participants").await;

    let (grant_id, invite_token) = issue_invite(&f, &inviter_id).await;
    // 签发时：TTL 是邀请那一档（72 小时），不是设备配对的 15 分钟。
    let (issued_at, expires_at): (i64, i64) =
        sqlx::query_as("SELECT issued_at, expires_at FROM pairing_grants WHERE grant_id = ?")
            .bind(&grant_id)
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(expires_at - issued_at, 72 * 60 * 60 * 1000);

    let result = exchange_invite(&f, &invite_token, "阿元")
        .await
        .expect("invite exchange");
    let newcomer = result.outcome.human_id.as_str().to_owned();
    assert_ne!(newcomer, inviter_id, "邀请产出的是**新**主体，不是签发者");
    assert!(result.outcome.invited_new_human);
    assert_eq!(result.outcome.issued_by.as_str(), inviter_id);

    // 世界真的多了一个人，而且它是 active Human、名字由它自己给。
    assert_eq!(
        count_rows(&f, "participants").await,
        participants_before + 1
    );
    let (kind, status, display_name): (String, String, String) =
        sqlx::query_as("SELECT kind, status, display_name FROM participants WHERE id = ?")
            .bind(&newcomer)
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(
        (kind.as_str(), status.as_str(), display_name.as_str()),
        ("human", "active", "阿元")
    );

    // 会话归新人：摘要可查、metadata 指向这张 Grant、Grant 已消费。
    let session_token = result.session_token.as_str();
    let bound_human: String =
        sqlx::query_scalar("SELECT human_id FROM human_sessions WHERE token_hash = ?")
            .bind(TokenHash::from_token_str(session_token).hex())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(bound_human, newcomer);
    let source: String =
        sqlx::query_scalar("SELECT source_grant_id FROM session_metadata WHERE session_id = ?")
            .bind(result.outcome.session_id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(source, grant_id);
    let status: String = sqlx::query_scalar("SELECT status FROM pairing_grants WHERE grant_id = ?")
        .bind(&grant_id)
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(status, "consumed");

    // 重放：同一个码再也不行，也**不再造人**。
    let replay = exchange_invite(&f, &invite_token, "另一个人")
        .await
        .unwrap_err();
    assert!(matches!(replay, home::ExchangeError::GrantInvalid));
    assert_eq!(
        count_rows(&f, "participants").await,
        participants_before + 1
    );
}

/// 邀请码必须由**被邀请者自己**命名：没给名字就拒绝，且**不消费**这张码。
#[tokio::test]
async fn invite_requires_the_invitee_to_name_themselves_and_does_not_consume() {
    let f = fixture().await;
    let (inviter_id, _) = bootstrap(&f, "Alice").await;
    let (_, invite_token) = issue_invite(&f, &inviter_id).await;
    let participants_before = count_rows(&f, "participants").await;

    let refused = home::exchange_grant(&f.home, &invite_token, None, None)
        .await
        .unwrap_err();
    assert!(matches!(refused, home::ExchangeError::InvalidBody(_)));
    // 失败零残留：没有人、没有会话；码还是 active（补上名字就能用）。
    assert_eq!(count_rows(&f, "participants").await, participants_before);
    assert_eq!(count_rows(&f, "human_sessions").await, 1); // 只有 bootstrap 那一支

    // 名字非法（全空白）同样拒绝。
    let blank = exchange_invite(&f, &invite_token, "   ").await.unwrap_err();
    assert!(matches!(blank, home::ExchangeError::InvalidBody(_)));
    assert_eq!(count_rows(&f, "participants").await, participants_before);

    // 补上合法名字 ⇒ 成功（证明前两次失败没有消费掉这张码）。
    let ok = exchange_invite(&f, &invite_token, "阿元")
        .await
        .expect("retry works");
    assert!(ok.outcome.invited_new_human);
}

/// 设备配对那一类**不接受**名字（用途与请求体必须对上），失败也不消费。
#[tokio::test]
async fn device_grant_rejects_a_display_name() {
    let f = fixture().await;
    let (human_id, _) = bootstrap(&f, "Alice").await;
    let (_, device_token) = issue(&f, &human_id, Some("我的笔记本")).await;
    let participants_before = count_rows(&f, "participants").await;

    let refused = home::exchange_grant(&f.home, &device_token, None, Some("阿元"))
        .await
        .unwrap_err();
    assert!(matches!(refused, home::ExchangeError::InvalidBody(_)));
    assert_eq!(count_rows(&f, "participants").await, participants_before);

    // 不带名字仍然是既有语义：会话归签发者本人。
    let ok = exchange(&f, &device_token, Some("我的笔记本"))
        .await
        .expect("device pairing still works");
    assert!(!ok.outcome.invited_new_human);
    assert_eq!(ok.outcome.human_id.as_str(), human_id);
    assert_eq!(count_rows(&f, "participants").await, participants_before);
}

/// 撤销过的邀请码造不出人；未知码同样——两条路都零残留。
#[tokio::test]
async fn revoked_or_unknown_invite_never_creates_a_human() {
    let f = fixture().await;
    let (inviter_id, _) = bootstrap(&f, "Alice").await;
    let (grant_id, invite_token) = issue_invite(&f, &inviter_id).await;
    let participants_before = count_rows(&f, "participants").await;

    f.home
        .revoke_grant(
            &crate::domain::participant::ParticipantId(inviter_id.clone()),
            &crate::domain::home::GrantId(grant_id),
            crate::domain::time::UtcMillis::now(),
        )
        .await
        .expect("revoke");

    let revoked = exchange_invite(&f, &invite_token, "阿元")
        .await
        .unwrap_err();
    assert!(matches!(revoked, home::ExchangeError::GrantInvalid));
    let unknown = exchange_invite(&f, "clr_pg_unknown_invite_token_value", "阿元")
        .await
        .unwrap_err();
    assert!(matches!(unknown, home::ExchangeError::GrantInvalid));
    assert_eq!(count_rows(&f, "participants").await, participants_before);
}
