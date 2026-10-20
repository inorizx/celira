//! SQLite Backend adapter 测试：真实临时数据库上的 Binding/Credential 创建、
//! 幂等、CAS 轮换、撤销传播、并发裁决、取消安全、hash 脱敏、schema 边界
//! 与 v5→v6 升级路径。

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Connection, SqlitePool};
use uuid::Uuid;

use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::backend_store::SqliteBackendStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::migration_store::{MIGRATOR, MigrationStore};
use crate::application::agent;
use crate::application::backend;
use crate::application::identity::bootstrap_human;
use crate::domain::agent::IdempotencyKey;
use crate::domain::backend::{BackendBinding, BackendCredential, BackendToken};
use crate::domain::human_session::{HumanSession, SessionId, TokenHash};
use crate::domain::participant::{DisplayName, Participant, ParticipantId};
use crate::domain::time::UtcMillis;
use crate::domain::token::OpaqueToken;
use crate::ports::backend::{BackendStore as _, NewCredential};
use crate::ports::identity::{IdentityStore as _, NewSession};
use crate::ports::migration::{MigrationProbe as _, MigrationStatus};

struct Fixture {
    _dir: tempfile::TempDir,
    store: MigrationStore,
    identity: SqliteIdentityStore,
    agent: SqliteAgentStore,
    backend: SqliteBackendStore,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("open + migrate");
    let identity = SqliteIdentityStore::new(store.pool().clone());
    let agent = SqliteAgentStore::new(store.pool().clone());
    let backend = SqliteBackendStore::new(store.pool().clone());
    Fixture {
        _dir: dir,
        store,
        identity,
        agent,
        backend,
    }
}

/// 通过 schema 直接创建第二个 Human 并为其补发 Session（跨 Human 隔离场景）。
async fn add_human(
    pool: &SqlitePool,
    identity: &SqliteIdentityStore,
    display_name: &str,
) -> Participant {
    let id = ParticipantId(Uuid::new_v4().to_string());
    sqlx::query(
        "INSERT INTO participants (id, kind, status, display_name, created_at, revoked_at) \
         VALUES (?, 'human', 'active', ?, 0, NULL)",
    )
    .bind(id.as_str())
    .bind(display_name)
    .execute(pool)
    .await
    .expect("insert second human");
    let token = OpaqueToken::generate().expect("token");
    let session = HumanSession::new(
        SessionId(Uuid::new_v4().to_string()),
        id.clone(),
        TokenHash::from_token(&token),
        UtcMillis::now(),
    );
    identity
        .issue_human_session(&NewSession {
            id: session.id.clone(),
            human_id: session.human_id.clone(),
            token_hash: session.token_hash,
            created_at: session.created_at,
        })
        .await
        .expect("issue session for second human");
    Participant::new_human(
        id,
        DisplayName::parse(display_name).unwrap(),
        UtcMillis::now(),
    )
}

/// 通过公开用例创建 Agent，返回其 ID。
async fn create_agent(f: &Fixture, human: &Participant, name: &str, key: &str) -> ParticipantId {
    let created = agent::create_agent(&f.agent, human, name, key)
        .await
        .expect("create agent");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("agent create must be Created");
    };
    agent.id
}

/// 通过公开用例创建 Binding，返回事实。
async fn create_binding(
    f: &Fixture,
    human: &Participant,
    agent_id: &str,
    label: &str,
    key: &str,
) -> BackendBinding {
    let outcome = backend::create_binding(&f.backend, human, agent_id, label, key)
        .await
        .expect("create binding");
    let backend::CreateBindingOutcome::Created { binding } = outcome else {
        panic!("first binding create must be Created");
    };
    binding
}

/// 通过公开用例签发 Credential（首次或轮换），返回事实与一次性 token。
async fn issue_credential(
    f: &Fixture,
    human: &Participant,
    agent_id: &str,
    binding_id: &str,
    replaces: Option<&str>,
    key: &str,
) -> (BackendCredential, BackendToken) {
    let outcome = backend::issue_credential(&f.backend, human, agent_id, binding_id, replaces, key)
        .await
        .expect("issue credential");
    let backend::IssueCredentialOutcome::Created { credential, token } = outcome else {
        panic!("first issue must be Created");
    };
    (credential, token)
}

async fn active_credential_count(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM backend_credentials WHERE status = 'active'")
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

async fn binding_count(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM backend_bindings")
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn create_binding_persists_and_replays() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;

    let created = create_binding(&f, human, agent_id.as_str(), "local runtime", "b-1").await;
    assert_eq!(created.agent_id, agent_id);
    assert_eq!(created.created_by_human_id, human.id);
    assert_eq!(created.label.as_str(), "local runtime");
    assert_eq!(created.status.as_str(), "active");
    assert!(created.revoked_at.is_none());
    assert_eq!(binding_count(&f).await, 1);

    // 幂等协调列随事实行原子保存。
    let stored: (String,) =
        sqlx::query_as("SELECT idempotency_key FROM backend_bindings WHERE id = ?")
            .bind(created.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(stored.0, "b-1");

    // 同 Human + 同 key + 同 Agent + 同 label → 重放，不新增第二行。
    let replayed =
        backend::create_binding(&f.backend, human, agent_id.as_str(), "local runtime", "b-1")
            .await
            .expect("replay");
    let backend::CreateBindingOutcome::Replayed { binding } = replayed else {
        panic!("same key same request must be Replayed");
    };
    assert_eq!(binding.id, created.id);
    assert_eq!(binding_count(&f).await, 1);

    // 同 key 不同 label → 409。
    let conflict = backend::create_binding(&f.backend, human, agent_id.as_str(), "other", "b-1")
        .await
        .expect_err("different label must conflict");
    assert!(matches!(
        conflict,
        backend::CreateBindingError::IdempotencyConflict
    ));
    assert_eq!(binding_count(&f).await, 1);
}

#[tokio::test]
async fn create_binding_idempotency_key_is_global_across_agents() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_a = create_agent(&f, human, "A", "k-a").await;
    let agent_b = create_agent(&f, human, "B", "k-b").await;
    create_binding(&f, human, agent_a.as_str(), "la", "same-key").await;

    let conflict = backend::create_binding(&f.backend, human, agent_b.as_str(), "lb", "same-key")
        .await
        .expect_err("same key different agent must conflict");
    assert!(matches!(
        conflict,
        backend::CreateBindingError::IdempotencyConflict
    ));
}

#[tokio::test]
async fn create_binding_requires_active_owned_agent() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;

    // 未知 Agent → 404。
    let unknown = Uuid::new_v4().to_string();
    let err = backend::create_binding(&f.backend, human, &unknown, "l", "b-1")
        .await
        .expect_err("unknown agent");
    assert!(matches!(err, backend::CreateBindingError::AgentNotFound));

    // 非 Owner 的第二个 Human 创建同一个 Agent 的 Binding → 404（不可区分）。
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let err = backend::create_binding(&f.backend, &bob, agent_id.as_str(), "l", "b-2")
        .await
        .expect_err("not-owned agent");
    assert!(matches!(err, backend::CreateBindingError::AgentNotFound));

    // revoked Agent → 409 agent_not_active。
    agent::revoke_agent(&f.agent, &human.id, agent_id.as_str())
        .await
        .expect("revoke agent");
    let err = backend::create_binding(&f.backend, human, agent_id.as_str(), "l", "b-3")
        .await
        .expect_err("revoked agent");
    assert!(matches!(err, backend::CreateBindingError::AgentNotActive));
}

#[tokio::test]
async fn one_active_binding_per_agent() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;

    create_binding(&f, human, agent_id.as_str(), "first", "b-1").await;
    let err = backend::create_binding(&f.backend, human, agent_id.as_str(), "second", "b-2")
        .await
        .expect_err("second active binding");
    assert!(matches!(err, backend::CreateBindingError::BindingExists));

    // 撤销后可以再创建。
    let first = backend::list_bindings(&f.backend, &human.id, agent_id.as_str())
        .await
        .unwrap()
        .pop()
        .unwrap();
    backend::revoke_binding(&f.backend, &human.id, agent_id.as_str(), first.id.as_str())
        .await
        .expect("revoke binding");
    let outcome = backend::create_binding(&f.backend, human, agent_id.as_str(), "second", "b-2")
        .await
        .expect("create after revoke");
    assert!(matches!(
        outcome,
        backend::CreateBindingOutcome::Created { .. }
    ));
}

#[tokio::test]
async fn list_and_get_binding_are_owner_scoped() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;
    let b1 = create_binding(&f, human, agent_id.as_str(), "one", "b-1").await;
    backend::revoke_binding(&f.backend, &human.id, agent_id.as_str(), b1.id.as_str())
        .await
        .expect("revoke");
    let b2 = create_binding(&f, human, agent_id.as_str(), "two", "b-2").await;

    // 列表包含 active + revoked 历史，按 created_at 升序。
    let list = backend::list_bindings(&f.backend, &human.id, agent_id.as_str())
        .await
        .expect("list");
    let ids: Vec<&str> = list.iter().map(|b| b.id.as_str()).collect();
    assert_eq!(ids, vec![b1.id.as_str(), b2.id.as_str()]);

    let got = backend::get_binding(&f.backend, &human.id, agent_id.as_str(), b2.id.as_str())
        .await
        .expect("get");
    assert_eq!(got.id, b2.id);
    assert_eq!(got.status.as_str(), "active");

    // unknown/bad binding id 在 owned Agent 下 → BindingNotFound。
    let ghost = Uuid::new_v4().to_string();
    let err = backend::get_binding(&f.backend, &human.id, agent_id.as_str(), &ghost)
        .await
        .expect_err("ghost binding");
    assert!(matches!(err, backend::BackendQueryError::BindingNotFound));
    let err = backend::get_binding(&f.backend, &human.id, agent_id.as_str(), "not-a-uuid")
        .await
        .expect_err("bad binding id");
    assert!(matches!(err, backend::BackendQueryError::BindingNotFound));

    // 未知 Agent → AgentNotFound；非 Owner → AgentNotFound。
    let err = backend::list_bindings(&f.backend, &human.id, &Uuid::new_v4().to_string())
        .await
        .expect_err("unknown agent list");
    assert!(matches!(err, backend::BackendQueryError::AgentNotFound));
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let err = backend::list_bindings(&f.backend, &bob.id, agent_id.as_str())
        .await
        .expect_err("not-owned list");
    assert!(matches!(err, backend::BackendQueryError::AgentNotFound));
}

#[tokio::test]
async fn issue_replay_and_conflict() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;
    let binding = create_binding(&f, human, agent_id.as_str(), "local runtime", "b-1").await;

    let (created, token) = issue_credential(
        &f,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        None,
        "c-1",
    )
    .await;
    assert_eq!(created.status.as_str(), "active");
    assert!(token.as_str().starts_with("clr_bc_"));
    assert_eq!(active_credential_count(&f).await, 1);

    // 同 key 同请求重放 → Replayed 同 ID，不新增、不恢复。
    let replayed = backend::issue_credential(
        &f.backend,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        None,
        "c-1",
    )
    .await
    .expect("replay");
    let backend::IssueCredentialOutcome::Replayed { credential } = replayed else {
        panic!("same key same request must be Replayed");
    };
    assert_eq!(credential.id, created.id);
    assert_eq!(active_credential_count(&f).await, 1);
    // 重放不产生新 secret：DB 中仍只有原 hash。
    let hashes: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM backend_credentials WHERE token_hash = ?")
            .bind(token.sha256_hex())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(hashes, 1);

    // 同 key 不同 replaces → IdempotencyConflict。
    let err = backend::issue_credential(
        &f.backend,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        Some(created.id.as_str()),
        "c-1",
    )
    .await
    .expect_err("same key different replaces");
    assert!(matches!(
        err,
        backend::IssueCredentialError::IdempotencyConflict
    ));
}

#[tokio::test]
async fn first_issue_requires_no_active_credential() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;
    let binding = create_binding(&f, human, agent_id.as_str(), "local runtime", "b-1").await;

    issue_credential(
        &f,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        None,
        "c-1",
    )
    .await;
    let err = backend::issue_credential(
        &f.backend,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        None,
        "c-2",
    )
    .await
    .expect_err("null replaces while active exists");
    assert!(matches!(
        err,
        backend::IssueCredentialError::CredentialStateConflict
    ));
    // 无副作用：仍只有一个 active，没有第二行。
    assert_eq!(active_credential_count(&f).await, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM backend_credentials")
            .fetch_one(f.store.pool())
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn cas_rotation_handover_is_atomic() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;
    let binding = create_binding(&f, human, agent_id.as_str(), "local runtime", "b-1").await;

    let (first, first_token) = issue_credential(
        &f,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        None,
        "c-1",
    )
    .await;

    // CAS 轮换：指定当前 active ID。
    let (second, second_token) = issue_credential(
        &f,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        Some(first.id.as_str()),
        "c-2",
    )
    .await;
    assert_ne!(second.id, first.id);
    assert_eq!(active_credential_count(&f).await, 1);

    // 旧 token 认证失败，新 token 认证成功。
    assert!(
        f.backend
            .authenticate(&first_token.sha256_hex())
            .await
            .unwrap()
            .is_none()
    );
    let view = f
        .backend
        .authenticate(&second_token.sha256_hex())
        .await
        .unwrap()
        .expect("new token authenticates");
    assert_eq!(view.agent.id, agent_id);
    assert_eq!(view.binding.id, binding.id);
    assert_eq!(view.credential.id, second.id);

    // 轮换链：第三次轮换，只有第二个是 active。
    let (third, third_token) = issue_credential(
        &f,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        Some(second.id.as_str()),
        "c-3",
    )
    .await;
    assert!(
        f.backend
            .authenticate(&second_token.sha256_hex())
            .await
            .unwrap()
            .is_none(),
        "rotated-away token must fail"
    );
    assert!(
        f.backend
            .authenticate(&third_token.sha256_hex())
            .await
            .unwrap()
            .is_some()
    );
    // 历史保留：三次 Credential 都在，只有一个 active。
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backend_credentials")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(total, 3);
    assert_eq!(active_credential_count(&f).await, 1);
    // 无明文 token。
    let plaintext: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM backend_credentials WHERE instr(token_hash, 'clr_bc_') > 0",
    )
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(plaintext, 0);
    let _ = (third_token, third);
}

#[tokio::test]
async fn stale_replaces_id_conflicts_without_side_effects() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;
    let binding = create_binding(&f, human, agent_id.as_str(), "local runtime", "b-1").await;

    let (first, first_token) = issue_credential(
        &f,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        None,
        "c-1",
    )
    .await;
    issue_credential(
        &f,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        Some(first.id.as_str()),
        "c-2",
    )
    .await;

    // stale replaces（已轮换掉的 ID）→ 409，不撤销当前、不生成 token。
    let err = backend::issue_credential(
        &f.backend,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        Some(first.id.as_str()),
        "c-3",
    )
    .await
    .expect_err("stale replaces");
    assert!(matches!(
        err,
        backend::IssueCredentialError::CredentialStateConflict
    ));
    assert_eq!(active_credential_count(&f).await, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM backend_credentials")
            .fetch_one(f.store.pool())
            .await
            .unwrap(),
        2
    );

    // 不属于该 Binding 的 UUID → 409 且无副作用。
    let foreign = Uuid::new_v4().to_string();
    let err = backend::issue_credential(
        &f.backend,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        Some(&foreign),
        "c-4",
    )
    .await
    .expect_err("foreign replaces");
    assert!(matches!(
        err,
        backend::IssueCredentialError::CredentialStateConflict
    ));
    assert_eq!(active_credential_count(&f).await, 1);
    let _ = first_token;
}

#[tokio::test]
async fn lost_response_recovery_uses_list_then_rotate() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;
    let binding = create_binding(&f, human, agent_id.as_str(), "local runtime", "b-1").await;
    issue_credential(
        &f,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        None,
        "c-1",
    )
    .await;

    // 客户端丢了 201 响应：list 解析当前 active，以新 key + 该 ID 轮换。
    let list = backend::list_credentials(
        &f.backend,
        &human.id,
        agent_id.as_str(),
        binding.id.as_str(),
    )
    .await
    .expect("list");
    let active: Vec<&BackendCredential> = list
        .iter()
        .filter(|c| c.status.as_str() == "active")
        .collect();
    assert_eq!(active.len(), 1);
    let (rotated, _new_token) = issue_credential(
        &f,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        Some(active[0].id.as_str()),
        "c-2",
    )
    .await;
    assert_ne!(rotated.id, active[0].id);
    // 旧 secret 不可找回：list 不含 token/hash 字段（由端口模型保证），
    // 重放原签发 key 也不返回 token（应用层语义，见 HTTP 测试）。
    let list = backend::list_credentials(
        &f.backend,
        &human.id,
        agent_id.as_str(),
        binding.id.as_str(),
    )
    .await
    .expect("list after rotate");
    assert_eq!(list.len(), 2);
}

#[tokio::test]
async fn revoke_credential_fails_closed_immediately() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;
    let binding = create_binding(&f, human, agent_id.as_str(), "local runtime", "b-1").await;
    let (credential, token) = issue_credential(
        &f,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        None,
        "c-1",
    )
    .await;

    backend::revoke_credential(
        &f.backend,
        &human.id,
        agent_id.as_str(),
        binding.id.as_str(),
        credential.id.as_str(),
    )
    .await
    .expect("revoke credential");
    assert!(
        f.backend
            .authenticate(&token.sha256_hex())
            .await
            .unwrap()
            .is_none(),
        "revoked credential must fail authentication"
    );
    // Binding 与 Agent 保持 active。
    let binding_view = backend::get_binding(
        &f.backend,
        &human.id,
        agent_id.as_str(),
        binding.id.as_str(),
    )
    .await
    .expect("binding still readable");
    assert_eq!(binding_view.status.as_str(), "active");
    assert!(binding_view.revoked_at.is_none());

    // 重复撤销 → Ok 且原 revoked_at 不变。
    backend::revoke_credential(
        &f.backend,
        &human.id,
        agent_id.as_str(),
        binding.id.as_str(),
        credential.id.as_str(),
    )
    .await
    .expect("repeat revoke");
    let revoked_at: i64 =
        sqlx::query_scalar("SELECT revoked_at FROM backend_credentials WHERE id = ?")
            .bind(credential.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert!(revoked_at > 0);

    // unknown/bad/not-under-binding → CredentialNotFound。
    let ghost = Uuid::new_v4().to_string();
    let err = backend::revoke_credential(
        &f.backend,
        &human.id,
        agent_id.as_str(),
        binding.id.as_str(),
        &ghost,
    )
    .await
    .expect_err("ghost credential");
    assert!(matches!(
        err,
        backend::CredentialQueryError::CredentialNotFound
    ));
}

#[tokio::test]
async fn revoke_binding_cascades_credential_in_same_transaction() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;
    let binding = create_binding(&f, human, agent_id.as_str(), "local runtime", "b-1").await;
    let (_credential, token) = issue_credential(
        &f,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        None,
        "c-1",
    )
    .await;

    backend::revoke_binding(
        &f.backend,
        &human.id,
        agent_id.as_str(),
        binding.id.as_str(),
    )
    .await
    .expect("revoke binding");
    // 同事务级联：Credential 与 Binding 都 revoked。
    assert!(
        f.backend
            .authenticate(&token.sha256_hex())
            .await
            .unwrap()
            .is_none()
    );
    let (cred_status, binding_status, binding_revoked_at): (String, String, i64) = sqlx::query_as(
        "SELECT c.status, b.status, b.revoked_at \
         FROM backend_bindings b JOIN backend_credentials c ON c.binding_id = b.id",
    )
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(cred_status, "revoked");
    assert_eq!(binding_status, "revoked");
    assert!(binding_revoked_at > 0);

    // 已撤销 Binding 不能签发新 Credential。
    let err = backend::issue_credential(
        &f.backend,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        None,
        "c-2",
    )
    .await
    .expect_err("issue on revoked binding");
    assert!(matches!(
        err,
        backend::IssueCredentialError::CredentialStateConflict
    ));

    // 重复撤销 → Ok 且原 revoked_at 不变。
    backend::revoke_binding(
        &f.backend,
        &human.id,
        agent_id.as_str(),
        binding.id.as_str(),
    )
    .await
    .expect("repeat revoke");
    let revoked_at_after: i64 =
        sqlx::query_scalar("SELECT revoked_at FROM backend_bindings WHERE id = ?")
            .bind(binding.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(
        revoked_at_after, binding_revoked_at,
        "revoked_at must not change"
    );
}

#[tokio::test]
async fn agent_revoke_propagates_to_binding_and_credential() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;
    let binding = create_binding(&f, human, agent_id.as_str(), "local runtime", "b-1").await;
    let (_credential, token) = issue_credential(
        &f,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        None,
        "c-1",
    )
    .await;

    // 复用既有 Agent revoke API：migration 006 触发器在同一事务内级联撤销
    // Credential 与 Binding。
    agent::revoke_agent(&f.agent, &human.id, agent_id.as_str())
        .await
        .expect("revoke agent");
    assert!(
        f.backend
            .authenticate(&token.sha256_hex())
            .await
            .unwrap()
            .is_none(),
        "agent revoke must fail closed"
    );
    let (binding_status, cred_status): (String, String) = sqlx::query_as(
        "SELECT b.status, c.status \
         FROM backend_bindings b JOIN backend_credentials c ON c.binding_id = b.id",
    )
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(binding_status, "revoked");
    assert_eq!(cred_status, "revoked");

    // 历史仍在（不硬删除），认证永不恢复。
    assert_eq!(binding_count(&f).await, 1);
}

#[tokio::test]
async fn cancelled_agent_revoke_transaction_rolls_back_all_cascades_before_connection_reuse() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;
    let binding = create_binding(&f, human, agent_id.as_str(), "local runtime", "b-1").await;
    let (credential, token) = issue_credential(
        &f,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        None,
        "c-1",
    )
    .await;

    // 单连接池保证 abort 后所有查询与正式 revoke 复用同一物理连接；如果
    // Transaction Drop 没有回滚，后续会看到部分 revoked 事实或脏事务。
    let options = SqliteConnectOptions::new()
        .filename(f._dir.path().join("core.db"))
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5));
    let cancel_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open single-connection cancellation pool");

    let task_pool = cancel_pool.clone();
    let task_agent_id = agent_id.as_str().to_owned();
    let (updated_tx, updated_rx) = tokio::sync::oneshot::channel();
    let (_keep_open_tx, keep_open_rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        let mut conn = task_pool.acquire().await?;
        let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "UPDATE participants SET status = 'revoked', revoked_at = 1 \
             WHERE id = ? AND status = 'active'",
        )
        .bind(task_agent_id)
        .execute(&mut *tx)
        .await?;
        updated_tx
            .send(())
            .expect("cancellation test receiver must remain alive");
        let _ = keep_open_rx.await;
        tx.commit().await
    });

    updated_rx
        .await
        .expect("Agent and trigger cascades must execute before abort");
    handle.abort();
    let join_error = handle.await.expect_err("task must be cancelled");
    assert!(join_error.is_cancelled());

    let statuses: (String, String, String, String) = sqlx::query_as(
        "SELECT p.status, o.status, b.status, c.status \
         FROM participants p \
         JOIN agent_owners o ON o.agent_id = p.id \
         JOIN backend_bindings b ON b.agent_id = p.id \
         JOIN backend_credentials c ON c.binding_id = b.id \
         WHERE p.id = ? AND b.id = ? AND c.id = ?",
    )
    .bind(agent_id.as_str())
    .bind(binding.id.as_str())
    .bind(credential.id.as_str())
    .fetch_one(&cancel_pool)
    .await
    .expect("same pooled connection must recover and expose rolled-back facts");
    assert_eq!(
        statuses,
        (
            "active".into(),
            "active".into(),
            "active".into(),
            "active".into()
        )
    );

    let cancel_backend = SqliteBackendStore::new(cancel_pool.clone());
    assert!(
        cancel_backend
            .authenticate(&token.sha256_hex())
            .await
            .unwrap()
            .is_some(),
        "cancelled revoke must not invalidate the credential"
    );

    // 同一连接池随后执行正式 Store 撤销，证明连接可复用且完整传播最终提交。
    let cancel_agent = SqliteAgentStore::new(cancel_pool.clone());
    agent::revoke_agent(&cancel_agent, &human.id, agent_id.as_str())
        .await
        .expect("official revoke must succeed after cancelled transaction");
    assert!(
        cancel_backend
            .authenticate(&token.sha256_hex())
            .await
            .unwrap()
            .is_none(),
        "committed Agent revoke must fail closed"
    );
}

#[tokio::test]
async fn authenticate_rejects_unknown_and_format_invalid_hashes() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;
    let binding = create_binding(&f, human, agent_id.as_str(), "local runtime", "b-1").await;
    let (_credential, _token) = issue_credential(
        &f,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        None,
        "c-1",
    )
    .await;

    // 未知 hash → None。
    assert!(
        f.backend
            .authenticate(&"0".repeat(64))
            .await
            .unwrap()
            .is_none()
    );
    // 非 64 hex 的 hash 查询不命中（调用方在应用层先做格式校验）。
    assert!(
        f.backend
            .authenticate("clr_bc_plaintext")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn database_never_contains_plaintext_token_or_invalid_hash() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;
    let binding = create_binding(&f, human, agent_id.as_str(), "local runtime", "b-1").await;
    let (_c1, t1) = issue_credential(
        &f,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        None,
        "c-1",
    )
    .await;
    let c2 = backend::list_credentials(
        &f.backend,
        &human.id,
        agent_id.as_str(),
        binding.id.as_str(),
    )
    .await
    .unwrap()
    .pop()
    .unwrap();
    issue_credential(
        &f,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        Some(c2.id.as_str()),
        "c-2",
    )
    .await;

    // 全部 token_hash 都是 64 字符小写 hex；全库无 clr_bc_ 明文。
    let hashes: Vec<String> = sqlx::query_scalar("SELECT token_hash FROM backend_credentials")
        .fetch_all(f.store.pool())
        .await
        .unwrap();
    assert_eq!(hashes.len(), 2);
    for hash in &hashes {
        assert_eq!(hash.len(), 64, "hash must be fixed length");
        assert!(
            hash.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "hash must be lowercase hex"
        );
        assert!(!hash.contains("clr_bc_"));
    }
    let plaintext_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM backend_credentials WHERE token_hash = ?")
            .bind(t1.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(plaintext_rows, 0, "raw token must never be stored");

    // 扫描全部用户表的全部列，而非只检查 token_hash：任何事实、幂等协调
    // 字段或未来新增列都不得保存一次性 Credential 原文。
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master \
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(f.store.pool())
    .await
    .unwrap();
    for table in tables {
        let quoted_table = format!("\"{}\"", table.replace('"', "\"\""));
        let columns: Vec<(i64, String, String, i64, Option<String>, i64)> =
            sqlx::query_as(&format!("PRAGMA table_info({quoted_table})"))
                .fetch_all(f.store.pool())
                .await
                .unwrap();
        for (_, column, _, _, _, _) in columns {
            let quoted_column = format!("\"{}\"", column.replace('"', "\"\""));
            let count: i64 = sqlx::query_scalar(&format!(
                "SELECT COUNT(*) FROM {quoted_table} \
                 WHERE instr(CAST({quoted_column} AS TEXT), 'clr_bc_') > 0"
            ))
            .fetch_one(f.store.pool())
            .await
            .unwrap();
            assert_eq!(
                count, 0,
                "raw credential prefix persisted in {table}.{column}"
            );
        }
    }
}

#[test]
fn new_credential_debug_redacts_token_hash() {
    let fake_hash = "a".repeat(64);
    let command = NewCredential {
        id: crate::domain::backend::CredentialId("00000000-0000-4000-8000-000000000001".into()),
        binding_id: crate::domain::backend::BindingId(
            "00000000-0000-4000-8000-000000000002".into(),
        ),
        issued_by_human_id: ParticipantId("00000000-0000-4000-8000-000000000003".into()),
        token_hash: fake_hash.clone(),
        replaces_credential_id: None,
        idempotency_key: IdempotencyKey::parse("credential-debug-test").unwrap(),
        created_at: UtcMillis::from_millis(1),
    };
    let debug = format!("{command:?}");
    assert!(!debug.contains(&fake_hash));
    assert!(debug.contains("[REDACTED]"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_binding_creation_one_wins() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;

    let store = f.backend.clone();
    let human1 = human.clone();
    let human2 = human.clone();
    let agent1 = agent_id.clone();
    let agent2 = agent_id.clone();
    let (r1, r2) = tokio::join!(
        backend::create_binding(&store, &human1, agent1.as_str(), "one", "b-1"),
        backend::create_binding(&store, &human2, agent2.as_str(), "two", "b-2"),
    );
    let created = match (r1, r2) {
        (Ok(backend::CreateBindingOutcome::Created { .. }), Err(_)) => 1,
        (Err(_), Ok(backend::CreateBindingOutcome::Created { .. })) => 1,
        _ => panic!("exactly one concurrent binding create must win"),
    };
    assert_eq!(created, 1);
    assert_eq!(binding_count(&f).await, 1);
    let statuses: Vec<String> = sqlx::query_scalar("SELECT status FROM backend_bindings")
        .fetch_all(f.store.pool())
        .await
        .unwrap();
    assert_eq!(statuses, vec!["active".to_owned()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_issue_and_rotation_one_wins() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;
    let binding = create_binding(&f, human, agent_id.as_str(), "local runtime", "b-1").await;

    // 并发首次签发（不同 key，replaces=null）：最多一个成功。
    let store = f.backend.clone();
    let human1 = human.clone();
    let human2 = human.clone();
    let agent1 = agent_id.clone();
    let agent2 = agent_id.clone();
    let (r1, r2) = tokio::join!(
        backend::issue_credential(
            &store,
            &human1,
            agent1.as_str(),
            binding.id.as_str(),
            None,
            "c-1"
        ),
        backend::issue_credential(
            &store,
            &human2,
            agent2.as_str(),
            binding.id.as_str(),
            None,
            "c-2"
        ),
    );
    match (r1, r2) {
        (Ok(backend::IssueCredentialOutcome::Created { .. }), Err(e)) => assert!(matches!(
            e,
            backend::IssueCredentialError::CredentialStateConflict
        )),
        (Err(e), Ok(backend::IssueCredentialOutcome::Created { .. })) => assert!(matches!(
            e,
            backend::IssueCredentialError::CredentialStateConflict
        )),
        other => panic!("exactly one first issue must win, got {other:?}"),
    }
    assert_eq!(active_credential_count(&f).await, 1);

    // 并发轮换同一 active ID：最多一个成功，不会出现两个 active。
    let active_id: String =
        sqlx::query_scalar("SELECT id FROM backend_credentials WHERE status = 'active'")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    let human3 = human.clone();
    let human4 = human.clone();
    let agent3 = agent_id.clone();
    let agent4 = agent_id.clone();
    let (r3, r4) = tokio::join!(
        backend::issue_credential(
            &store,
            &human3,
            agent3.as_str(),
            binding.id.as_str(),
            Some(&active_id),
            "c-3",
        ),
        backend::issue_credential(
            &store,
            &human4,
            agent4.as_str(),
            binding.id.as_str(),
            Some(&active_id),
            "c-4",
        ),
    );
    match (r3, r4) {
        (Ok(backend::IssueCredentialOutcome::Created { .. }), Err(e)) => assert!(matches!(
            e,
            backend::IssueCredentialError::CredentialStateConflict
        )),
        (Err(e), Ok(backend::IssueCredentialOutcome::Created { .. })) => assert!(matches!(
            e,
            backend::IssueCredentialError::CredentialStateConflict
        )),
        other => panic!("exactly one rotation must win, got {other:?}"),
    }
    assert_eq!(active_credential_count(&f).await, 1);
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backend_credentials")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(total, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_issue_leaves_no_partial_state_and_pool_usable() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;
    let binding = create_binding(&f, human, agent_id.as_str(), "local runtime", "b-1").await;

    // 中途取消签发 future：SQLx Transaction RAII 必须回滚未提交事务。
    let store = f.backend.clone();
    let human1 = human.clone();
    let agent1 = agent_id.clone();
    let b1 = binding.id.clone();
    let handle = tokio::spawn(async move {
        let _ = backend::issue_credential(
            &store,
            &human1,
            agent1.as_str(),
            b1.as_str(),
            None,
            "c-cancel",
        )
        .await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    handle.abort();
    let _ = handle.await;

    // 无论取消发生在提交前还是提交后，数据库都必须一致且连接池可用：
    // 取消在提交前 → 无 Credential，可首次签发；取消在提交后 → 恰一个
    // active，可轮换。
    assert!(
        (0..=1).contains(&active_credential_count(&f).await),
        "cancel must not leave partial credential state"
    );
    match backend::issue_credential(
        &f.backend,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        None,
        "c-after-cancel",
    )
    .await
    {
        Ok(backend::IssueCredentialOutcome::Created { token, .. }) => {
            assert!(
                f.backend
                    .authenticate(&token.sha256_hex())
                    .await
                    .unwrap()
                    .is_some()
            );
        }
        Err(backend::IssueCredentialError::CredentialStateConflict) => {
            let active: String =
                sqlx::query_scalar("SELECT id FROM backend_credentials WHERE status = 'active'")
                    .fetch_one(f.store.pool())
                    .await
                    .unwrap();
            let rotated = backend::issue_credential(
                &f.backend,
                human,
                agent_id.as_str(),
                binding.id.as_str(),
                Some(&active),
                "c-after-cancel-rotate",
            )
            .await
            .expect("rotate after cancel");
            let backend::IssueCredentialOutcome::Created { token, .. } = rotated else {
                panic!("rotation must be Created");
            };
            assert!(
                f.backend
                    .authenticate(&token.sha256_hex())
                    .await
                    .unwrap()
                    .is_some()
            );
        }
        other => panic!("unexpected issue outcome after cancel: {other:?}"),
    }
}

#[tokio::test]
async fn schema_has_only_binding_and_credential_backend_objects() {
    let f = fixture().await;
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table'")
            .fetch_all(f.store.pool())
            .await
            .unwrap();
    let expected = [
        "_sqlx_migrations",
        "bootstrap_guard",
        "participants",
        "human_sessions",
        "agent_owners",
        "agent_create_idempotency",
        "rooms",
        "room_coverage",
        "direct_room_pairs",
        "human_direct_pairs",
        "room_memberships",
        "group_owner_transitions",
        "direct_room_idempotency",
        "human_direct_room_idempotency",
        "group_create_idempotency",
        "group_member_invite_idempotency",
        "group_owner_transfer_idempotency",
        "contact_periods",
        "participant_blocks",
        "messages",
        "mentions",
        "backend_bindings",
        "backend_credentials",
        "agent_events",
        "human_event_facts",
        "human_event_deliveries",
        "read_cursors",
        "read_cursor_advancements",
        "room_attention",
        "message_recalls",
        "attachments",
        "message_attachments",
        "home_guard",
        "home",
        "pairing_grants",
        "session_metadata",
        "participant_profile_versions",
        "room_presentation_versions",
        "participant_profile_put_idempotency",
        "room_presentation_put_idempotency",
        "pairing_transactions",
    ];
    assert_eq!(tables.len(), expected.len(), "tables: {tables:?}");
    for table in &tables {
        assert!(
            expected.contains(&table.as_str()),
            "unexpected table {table:?}"
        );
    }
    // 006/007 只新增两张事实表：任何 Completion/ACK/Lease/Runtime 等延期
    // 对象都不得出现在 schema 中。
    let forbidden = [
        "ack",
        "lease",
        "replay",
        "prompt",
        "memory",
        "skill",
        "tool",
        "mcp",
        "provider",
        "runtime",
        "workspace",
        "session_run",
        "hermes",
        "websocket",
    ];
    let all: Vec<String> =
        sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE sql IS NOT NULL")
            .fetch_all(f.store.pool())
            .await
            .unwrap();
    for sql in &all {
        let words: Vec<&str> = sql
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        for word in forbidden {
            assert!(
                !words.contains(&word),
                "schema must not mention deferred object {word:?}"
            );
        }
    }
}

#[tokio::test]
async fn upgrade_from_v5_preserves_data_and_backend_available() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("core.db");

    // 手工构造 v5 数据库（001-005 原始 SQL + 元数据行）。
    let options = SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap();
    {
        let mut conn = pool.acquire().await.unwrap();
        for version in [
            202608110001i64,
            202608110002,
            202608110003,
            202608110004,
            202608110005,
        ] {
            let sql = match version {
                202608110001 => include_str!("../../../migrations/202608110001_core_baseline.sql"),
                202608110002 => include_str!("../../../migrations/202608110002_human_identity.sql"),
                202608110003 => include_str!("../../../migrations/202608110003_agent_identity.sql"),
                202608110004 => include_str!("../../../migrations/202608110004_direct_rooms.sql"),
                202608110005 => include_str!("../../../migrations/202608110005_messages.sql"),
                _ => unreachable!(),
            };
            sqlx::raw_sql(sql).execute(&mut *conn).await.unwrap();
        }
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS _sqlx_migrations (\
                 version BIGINT PRIMARY KEY,\
                 description TEXT NOT NULL,\
                 installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,\
                 success BOOLEAN NOT NULL,\
                 checksum BLOB NOT NULL,\
                 execution_time BIGINT NOT NULL\
             )",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        for version in [
            202608110001i64,
            202608110002,
            202608110003,
            202608110004,
            202608110005,
        ] {
            let migration = MIGRATOR
                .migrations
                .iter()
                .find(|m| m.version == version)
                .expect("compiled migration");
            sqlx::query(
                "INSERT INTO _sqlx_migrations \
                 (version, description, installed_on, success, checksum, execution_time) \
                 VALUES (?, ?, CURRENT_TIMESTAMP, 1, ?, 0)",
            )
            .bind(version)
            .bind(migration.description.as_ref())
            .bind(&migration.checksum[..])
            .execute(&mut *conn)
            .await
            .unwrap();
        }
    }

    // v5 上完成 Human + Agent 事实。
    let identity_v5 = SqliteIdentityStore::new(pool.clone());
    let agent_v5 = SqliteAgentStore::new(pool.clone());
    let outcome = bootstrap_human(&identity_v5, "Alice")
        .await
        .expect("bootstrap on v5 schema");
    let agent_id = {
        let created = agent::create_agent(&agent_v5, &outcome.participant, "助手", "k-1")
            .await
            .expect("create agent on v5");
        let agent::CreateAgentOutcome::Created { agent, .. } = created else {
            panic!("created");
        };
        agent.id
    };

    // 升级到 006：readiness 通过，既有事实保持。
    let store = MigrationStore::open(&db_path).await.expect("upgrade to v6");
    let status = store.status().await.expect("probe");
    assert_eq!(
        status,
        MigrationStatus {
            pending: 0,
            failed: 0
        },
        "upgraded database must be ready"
    );
    let (kind, status_row): (String, String) =
        sqlx::query_as("SELECT kind, status FROM participants WHERE id = ?")
            .bind(agent_id.as_str())
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(kind, "agent");
    assert_eq!(status_row, "active");
    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM human_sessions")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(sessions, 1);

    // 006 后 Backend API 可用。
    let backend = SqliteBackendStore::new(store.pool().clone());
    let human = &outcome.participant;
    let binding =
        backend::create_binding(&backend, human, agent_id.as_str(), "local runtime", "b-1")
            .await
            .expect("create binding after upgrade");
    let backend::CreateBindingOutcome::Created { binding } = binding else {
        panic!("binding create must be Created");
    };
    let issue = backend::issue_credential(
        &backend,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        None,
        "c-1",
    )
    .await
    .expect("issue after upgrade");
    let backend::IssueCredentialOutcome::Created { token, .. } = issue else {
        panic!("issue must be Created");
    };
    assert!(
        backend
            .authenticate(&token.sha256_hex())
            .await
            .unwrap()
            .is_some()
    );

    // 重复启动（同一数据库再开一次）幂等，readiness 保持。
    let store_again = MigrationStore::open(&db_path).await.expect("reopen");
    let status = store_again.status().await.expect("probe again");
    assert_eq!(
        status,
        MigrationStatus {
            pending: 0,
            failed: 0
        },
        "repeated startup must stay ready"
    );
}

#[tokio::test]
async fn database_rejects_unicode_whitespace_and_non_text_binding_labels() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;

    for (index, label) in ["\u{3000}", "\u{0085}", "\u{2007}\u{202f}"]
        .into_iter()
        .enumerate()
    {
        let result = sqlx::query(
            "INSERT INTO backend_bindings \
             (id, agent_id, created_by_human_id, label, status, created_at, revoked_at, idempotency_key) \
             VALUES (?, ?, ?, ?, 'active', 1, NULL, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(agent_id.as_str())
        .bind(human.id.as_str())
        .bind(label)
        .bind(format!("unicode-whitespace-{index}"))
        .execute(f.store.pool())
        .await;
        assert!(result.is_err(), "Unicode whitespace-only label must fail");
    }

    let blob_result = sqlx::query(
        "INSERT INTO backend_bindings \
         (id, agent_id, created_by_human_id, label, status, created_at, revoked_at, idempotency_key) \
         VALUES (?, ?, ?, ?, 'active', 1, NULL, 'blob-label')",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(agent_id.as_str())
    .bind(human.id.as_str())
    .bind(Vec::from("local runtime".as_bytes()))
    .execute(f.store.pool())
    .await;
    assert!(blob_result.is_err(), "BLOB label must fail TEXT constraint");
    assert_eq!(binding_count(&f).await, 0);
}

#[tokio::test]
async fn database_enforces_rotation_cas_and_rolls_back_failed_handover() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let human = &outcome.participant;
    let agent_id = create_agent(&f, human, "助手", "k-1").await;
    let binding = create_binding(&f, human, agent_id.as_str(), "local runtime", "b-1").await;
    let (first, _first_token) = issue_credential(
        &f,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        None,
        "c-1",
    )
    .await;
    let (second, _second_token) = issue_credential(
        &f,
        human,
        agent_id.as_str(),
        binding.id.as_str(),
        Some(first.id.as_str()),
        "c-2",
    )
    .await;

    struct RawCredential<'a> {
        id: String,
        hash: String,
        replaces: &'a str,
        key: &'a str,
        created_at: i64,
    }

    async fn raw_insert(
        f: &Fixture,
        human: &Participant,
        binding: &BackendBinding,
        raw: RawCredential<'_>,
    ) -> Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
        sqlx::query(
            "INSERT INTO backend_credentials \
             (id, binding_id, issued_by_human_id, token_hash, replaces_credential_id, \
              status, created_at, revoked_at, idempotency_key) \
             VALUES (?, ?, ?, ?, ?, 'active', ?, NULL, ?)",
        )
        .bind(raw.id)
        .bind(binding.id.as_str())
        .bind(human.id.as_str())
        .bind(raw.hash)
        .bind(raw.replaces)
        .bind(raw.created_at)
        .bind(raw.key)
        .execute(f.store.pool())
        .await
    }

    let stale = raw_insert(
        &f,
        human,
        &binding,
        RawCredential {
            id: Uuid::new_v4().to_string(),
            hash: "b".repeat(64),
            replaces: first.id.as_str(),
            key: "raw-stale",
            created_at: 10_001,
        },
    )
    .await;
    assert!(stale.is_err(), "stale replaces must fail in SQLite");
    assert_eq!(active_credential_count(&f).await, 1);

    let second_hash: String =
        sqlx::query_scalar("SELECT token_hash FROM backend_credentials WHERE id = ?")
            .bind(second.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    let failed = raw_insert(
        &f,
        human,
        &binding,
        RawCredential {
            id: Uuid::new_v4().to_string(),
            hash: second_hash,
            replaces: second.id.as_str(),
            key: "raw-failed-after-revoke",
            created_at: 10_002,
        },
    )
    .await;
    assert!(
        failed.is_err(),
        "failed replacement insert must fail atomically"
    );
    let second_state: (String, Option<i64>) =
        sqlx::query_as("SELECT status, revoked_at FROM backend_credentials WHERE id = ?")
            .bind(second.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(second_state, ("active".to_owned(), None));

    let third_id = Uuid::new_v4().to_string();
    raw_insert(
        &f,
        human,
        &binding,
        RawCredential {
            id: third_id.clone(),
            hash: "c".repeat(64),
            replaces: second.id.as_str(),
            key: "raw-valid-cas",
            created_at: 10_003,
        },
    )
    .await
    .expect("database-level CAS rotation");
    let second_state: (String, Option<i64>) =
        sqlx::query_as("SELECT status, revoked_at FROM backend_credentials WHERE id = ?")
            .bind(second.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(second_state, ("revoked".to_owned(), Some(10_003)));
    let current: String = sqlx::query_scalar(
        "SELECT id FROM backend_credentials WHERE binding_id = ? AND status = 'active'",
    )
    .bind(binding.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(current, third_id);
    assert_eq!(active_credential_count(&f).await, 1);
}
