//! SQLite Agent adapter 测试：真实临时数据库上的创建、幂等、撤销、
//! 生命周期终态守卫、并发唯一、DB 约束、schema 边界与 v2→v3 升级路径。

use std::sync::Arc;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use uuid::Uuid;

use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::migration_store::{MIGRATOR, MigrationStore};
use crate::application::agent;
use crate::application::identity::bootstrap_human;
use crate::domain::human_session::{HumanSession, SessionId, TokenHash};
use crate::domain::participant::{DisplayName, Participant, ParticipantId};
use crate::domain::time::UtcMillis;
use crate::domain::token::OpaqueToken;
use crate::ports::agent::AgentStore as _;
use crate::ports::identity::{IdentityStore as _, NewSession};
use crate::ports::migration::{MigrationProbe, MigrationStatus};

struct Fixture {
    _dir: tempfile::TempDir,
    store: MigrationStore,
    identity: SqliteIdentityStore,
    agent: SqliteAgentStore,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("open + migrate");
    let identity = SqliteIdentityStore::new(store.pool().clone());
    let agent = SqliteAgentStore::new(store.pool().clone());
    Fixture {
        _dir: dir,
        store,
        identity,
        agent,
    }
}

/// 通过 schema 直接创建第二个 Human 并为其补发 Session。
///
/// bootstrap guard 是应用层「首位 Human」门禁，只允许一次 bootstrap；
/// participants 表本身允许并存多个 human 行（存储事实），这里用最接近
/// 生产数据的路径构造第二个 Human，供跨 Human 隔离场景使用。
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

async fn count_agents(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM participants WHERE kind = 'agent'")
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

async fn count_owners(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM agent_owners")
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

async fn count_idempotency(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM agent_create_idempotency")
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn create_persists_agent_owner_and_idempotency_atomically() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let created = agent::create_agent(&f.agent, &outcome.participant, "助手", "key-1")
        .await
        .expect("create");

    let agent::CreateAgentOutcome::Created { agent, ownership } = created else {
        panic!("first create must be Created");
    };
    assert_ne!(
        agent.id, outcome.participant.id,
        "agent must differ from human"
    );
    assert_eq!(agent.kind.as_str(), "agent");
    assert_eq!(agent.status.as_str(), "active");
    assert_eq!(agent.revoked_at, None);
    assert_eq!(ownership.agent_id, agent.id);
    assert_eq!(ownership.human_id, outcome.participant.id);
    assert_eq!(ownership.status.as_str(), "active");

    assert_eq!(count_agents(&f).await, 1);
    assert_eq!(count_owners(&f).await, 1);
    assert_eq!(count_idempotency(&f).await, 1);

    let (kind, status, revoked_at): (String, String, Option<i64>) =
        sqlx::query_as("SELECT kind, status, revoked_at FROM participants WHERE id = ?")
            .bind(agent.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(kind, "agent");
    assert_eq!(status, "active");
    assert_eq!(revoked_at, None);

    let (human_id, owner_status): (String, String) =
        sqlx::query_as("SELECT human_id, status FROM agent_owners WHERE agent_id = ?")
            .bind(agent.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(human_id, outcome.participant.id.as_str());
    assert_eq!(owner_status, "active");

    let (stored_key, digest, idem_agent): (String, String, String) = sqlx::query_as(
        "SELECT key, request_digest, agent_id FROM agent_create_idempotency WHERE human_id = ?",
    )
    .bind(outcome.participant.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(stored_key, "key-1");
    assert_eq!(digest.len(), 64);
    assert_eq!(idem_agent, agent.id.as_str());
}

#[tokio::test]
async fn replay_same_key_and_digest_returns_same_agent_and_adds_nothing() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();

    let first = agent::create_agent(&f.agent, &outcome.participant, "助手", "key-1")
        .await
        .expect("first create");
    let agent::CreateAgentOutcome::Created {
        agent: first_agent, ..
    } = first
    else {
        panic!("first create must be Created");
    };

    let second = agent::create_agent(&f.agent, &outcome.participant, "助手", "key-1")
        .await
        .expect("replay must not conflict");
    let agent::CreateAgentOutcome::Replayed { agent, ownership } = second else {
        panic!("replay must be Replayed");
    };
    assert_eq!(agent, first_agent);
    assert_eq!(ownership.agent_id, first_agent.id);

    assert_eq!(count_agents(&f).await, 1);
    assert_eq!(count_owners(&f).await, 1);
    assert_eq!(count_idempotency(&f).await, 1);
}

#[tokio::test]
async fn same_key_different_digest_conflicts_and_leaves_no_side_effects() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();

    let first = agent::create_agent(&f.agent, &outcome.participant, "助手", "key-1")
        .await
        .expect("first create");
    let agent::CreateAgentOutcome::Created {
        agent: first_agent, ..
    } = first
    else {
        panic!("first create must be Created");
    };

    let err = agent::create_agent(&f.agent, &outcome.participant, "不同助手", "key-1")
        .await
        .unwrap_err();
    assert!(
        matches!(err, agent::CreateAgentError::IdempotencyConflict),
        "expected conflict, got {err}"
    );

    assert_eq!(count_agents(&f).await, 1);
    assert_eq!(count_owners(&f).await, 1);
    assert_eq!(count_idempotency(&f).await, 1);

    let replay = agent::create_agent(&f.agent, &outcome.participant, "助手", "key-1")
        .await
        .expect("original payload must still replay");
    let agent::CreateAgentOutcome::Replayed { agent, .. } = replay else {
        panic!("expected replay");
    };
    assert_eq!(agent, first_agent, "first facts must not be overwritten");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_create_same_request_creates_exactly_one_agent() {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .unwrap();
    let identity = SqliteIdentityStore::new(store.pool().clone());
    let agent_store = Arc::new(SqliteAgentStore::new(store.pool().clone()));
    let outcome = bootstrap_human(&identity, "Alice").await.unwrap();
    let participant = Arc::new(outcome.participant);

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let agent_store = agent_store.clone();
        let participant = participant.clone();
        tasks.push(tokio::spawn(async move {
            agent::create_agent(agent_store.as_ref(), &participant, "并发", "concurrent-key").await
        }));
    }
    let mut created = 0;
    let mut replayed = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(agent::CreateAgentOutcome::Created { .. }) => created += 1,
            Ok(agent::CreateAgentOutcome::Replayed { .. }) => replayed += 1,
            Err(e) => panic!("unexpected concurrent create error: {e}"),
        }
    }
    assert_eq!(created, 1, "exactly one create must win");
    assert_eq!(replayed, 7);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM participants WHERE kind = 'agent'")
            .fetch_one(store.pool())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM agent_create_idempotency")
            .fetch_one(store.pool())
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn different_humans_same_key_each_succeed() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;

    let a = agent::create_agent(&f.agent, &alice.participant, "A 的助手", "shared-key")
        .await
        .expect("alice create");
    let agent::CreateAgentOutcome::Created { agent: agent_a, .. } = a else {
        panic!("alice first create");
    };

    let b = agent::create_agent(&f.agent, &bob, "B 的助手", "shared-key")
        .await
        .expect("bob create must not conflict with alice");
    let agent::CreateAgentOutcome::Created { agent: agent_b, .. } = b else {
        panic!("bob first create");
    };
    assert_ne!(agent_a.id, agent_b.id);

    let alice_list = agent::list_agents(&f.agent, &alice.participant.id)
        .await
        .expect("alice list");
    let bob_list = agent::list_agents(&f.agent, &bob.id)
        .await
        .expect("bob list");
    assert_eq!(alice_list.len(), 1);
    assert_eq!(alice_list[0].agent.id, agent_a.id);
    assert_eq!(bob_list.len(), 1);
    assert_eq!(bob_list[0].agent.id, agent_b.id);
}

#[tokio::test]
async fn list_is_scoped_to_owner_and_sorted_by_created_at_then_id() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;

    for (name, key) in [("一", "k-1"), ("二", "k-2"), ("三", "k-3")] {
        agent::create_agent(&f.agent, &alice.participant, name, key)
            .await
            .expect("alice create");
    }
    agent::create_agent(&f.agent, &bob, "B 的", "k-b")
        .await
        .expect("bob create");

    let alice_list = agent::list_agents(&f.agent, &alice.participant.id)
        .await
        .expect("alice list");
    assert_eq!(alice_list.len(), 3);

    let mut expected = alice_list.clone();
    expected.sort_by(|a, b| {
        (a.agent.created_at, a.agent.id.as_str()).cmp(&(b.agent.created_at, b.agent.id.as_str()))
    });
    assert_eq!(
        alice_list, expected,
        "must be ordered by created_at then id"
    );

    let bob_list = agent::list_agents(&f.agent, &bob.id)
        .await
        .expect("bob list");
    assert_eq!(bob_list.len(), 1, "bob must not see alice's agents");
    assert_eq!(bob_list[0].ownership.human_id, bob.id);
}

#[tokio::test]
async fn get_returns_own_and_none_for_unknown_or_not_owned() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let created = agent::create_agent(&f.agent, &alice.participant, "助手", "k")
        .await
        .expect("create");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("created");
    };

    let own = agent::get_agent(&f.agent, &alice.participant.id, agent.id.as_str())
        .await
        .expect("own get");
    assert_eq!(own.agent.id, agent.id);
    assert_eq!(own.ownership.human_id, alice.participant.id);

    for bad in [
        "00000000-0000-0000-0000-000000000000",
        "not-a-uuid",
        agent.id.as_str(),
    ] {
        let err = agent::get_agent(&f.agent, &bob.id, bad).await.unwrap_err();
        assert!(
            matches!(err, agent::AgentQueryError::NotFound),
            "expected NotFound for {bad:?}, got {err}"
        );
    }
}

#[tokio::test]
async fn revoke_is_atomic_and_repeat_keeps_original_revoked_at() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let created = agent::create_agent(&f.agent, &alice.participant, "助手", "k")
        .await
        .expect("create");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("created");
    };

    agent::revoke_agent(&f.agent, &alice.participant.id, agent.id.as_str())
        .await
        .expect("revoke");

    let (kind, status, revoked_at): (String, String, Option<i64>) =
        sqlx::query_as("SELECT kind, status, revoked_at FROM participants WHERE id = ?")
            .bind(agent.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(kind, "agent");
    assert_eq!(status, "revoked");
    let first_revoked_at = revoked_at.expect("revoked agent must have revoked_at");

    let (owner_status, owner_revoked_at): (String, Option<i64>) =
        sqlx::query_as("SELECT status, revoked_at FROM agent_owners WHERE agent_id = ?")
            .bind(agent.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(owner_status, "revoked");
    assert_eq!(owner_revoked_at, Some(first_revoked_at));

    let (human_status, human_revoked_at): (String, Option<i64>) =
        sqlx::query_as("SELECT status, revoked_at FROM participants WHERE id = ?")
            .bind(alice.participant.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(human_status, "active", "revoke must not touch the human");
    assert_eq!(human_revoked_at, None);

    agent::revoke_agent(&f.agent, &alice.participant.id, agent.id.as_str())
        .await
        .expect("repeat revoke must succeed");
    let (_, _, revoked_at_again): (String, String, Option<i64>) =
        sqlx::query_as("SELECT kind, status, revoked_at FROM participants WHERE id = ?")
            .bind(agent.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(
        revoked_at_again,
        Some(first_revoked_at),
        "repeat revoke must not change original revoked_at"
    );

    let viewed = agent::get_agent(&f.agent, &alice.participant.id, agent.id.as_str())
        .await
        .expect("revoked agent must stay readable");
    assert_eq!(viewed.agent.status.as_str(), "revoked");
    assert_eq!(viewed.ownership.status.as_str(), "revoked");
}

#[tokio::test]
async fn revoke_unknown_or_not_owned_returns_not_found() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let created = agent::create_agent(&f.agent, &alice.participant, "助手", "k")
        .await
        .expect("create");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("created");
    };

    for (human, id) in [
        (&bob.id, agent.id.as_str()),
        (
            &alice.participant.id,
            "00000000-0000-0000-0000-000000000000",
        ),
        (&alice.participant.id, "not-a-uuid"),
    ] {
        let err = agent::revoke_agent(&f.agent, human, id).await.unwrap_err();
        assert!(
            matches!(err, agent::AgentQueryError::NotFound),
            "expected NotFound for {id:?}, got {err}"
        );
    }
    assert_eq!(count_agents(&f).await, 1, "failed revokes must not mutate");
    assert_eq!(count_owners(&f).await, 1);
}

#[tokio::test]
async fn db_triggers_reject_participant_kind_mutation() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let created = agent::create_agent(&f.agent, &alice.participant, "助手", "k")
        .await
        .expect("create");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("created");
    };

    let human_to_agent = sqlx::query("UPDATE participants SET kind = 'agent' WHERE id = ?")
        .bind(alice.participant.id.as_str())
        .execute(f.store.pool())
        .await;
    assert!(
        human_to_agent.is_err(),
        "human kind must not be rewritten to agent"
    );

    let agent_to_human = sqlx::query("UPDATE participants SET kind = 'human' WHERE id = ?")
        .bind(agent.id.as_str())
        .execute(f.store.pool())
        .await;
    assert!(
        agent_to_human.is_err(),
        "agent kind must not be rewritten to human"
    );

    let same_kind = sqlx::query("UPDATE participants SET kind = 'agent' WHERE id = ?")
        .bind(agent.id.as_str())
        .execute(f.store.pool())
        .await;
    assert!(same_kind.is_ok(), "writing back the same kind is a no-op");

    let (human_kind, agent_kind): (String, String) = sqlx::query_as(
        "SELECT kind, (SELECT kind FROM participants WHERE id = ?) FROM participants WHERE id = ?",
    )
    .bind(agent.id.as_str())
    .bind(alice.participant.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(human_kind, "human");
    assert_eq!(agent_kind, "agent");
}

#[tokio::test]
async fn db_triggers_make_revoked_agent_terminal() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let created = agent::create_agent(&f.agent, &alice.participant, "助手", "k")
        .await
        .expect("create");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("created");
    };

    agent::revoke_agent(&f.agent, &alice.participant.id, agent.id.as_str())
        .await
        .expect("first revoke must still pass lifecycle triggers");

    let (_, status, revoked_at): (String, String, Option<i64>) =
        sqlx::query_as("SELECT kind, status, revoked_at FROM participants WHERE id = ?")
            .bind(agent.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(status, "revoked");
    let original_revoked_at = revoked_at.expect("revoked agent must carry revoked_at");

    let reactivate =
        sqlx::query("UPDATE participants SET status = 'active', revoked_at = NULL WHERE id = ?")
            .bind(agent.id.as_str())
            .execute(f.store.pool())
            .await;
    assert!(
        reactivate.is_err(),
        "revoked agent must not be restored to active with revoked_at cleared"
    );

    let rewrite = sqlx::query("UPDATE participants SET revoked_at = ? WHERE id = ?")
        .bind(original_revoked_at + 1)
        .bind(agent.id.as_str())
        .execute(f.store.pool())
        .await;
    assert!(
        rewrite.is_err(),
        "revoked_at of revoked agent must not be rewritten"
    );

    let clear = sqlx::query("UPDATE participants SET revoked_at = NULL WHERE id = ?")
        .bind(agent.id.as_str())
        .execute(f.store.pool())
        .await;
    assert!(clear.is_err(), "revoked_at must not be cleared");

    let idempotent_reset = sqlx::query("UPDATE participants SET revoked_at = ? WHERE id = ?")
        .bind(original_revoked_at)
        .bind(agent.id.as_str())
        .execute(f.store.pool())
        .await;
    assert!(
        idempotent_reset.is_ok(),
        "rewriting the unchanged revoked_at is a no-op and stays allowed"
    );

    let (_, status_after, revoked_at_after): (String, String, Option<i64>) =
        sqlx::query_as("SELECT kind, status, revoked_at FROM participants WHERE id = ?")
            .bind(agent.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(status_after, "revoked");
    assert_eq!(revoked_at_after, Some(original_revoked_at));
}

#[tokio::test]
async fn db_triggers_make_revoked_owner_terminal() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let created = agent::create_agent(&f.agent, &alice.participant, "助手", "k")
        .await
        .expect("create");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("created");
    };

    // 合法 INSERT 历史 revoked Owner（Agent 仍 active）：终态守卫只作用于
    // UPDATE，不得阻止该插入，且可插入的合法历史记录先于任何改写测试存在。
    let first_revoked_at = UtcMillis::now().millis();
    sqlx::query(
        "INSERT INTO agent_owners (agent_id, human_id, status, created_at, revoked_at) \
         VALUES (?, ?, 'revoked', 0, ?)",
    )
    .bind(agent.id.as_str())
    .bind(bob.id.as_str())
    .bind(first_revoked_at)
    .execute(f.store.pool())
    .await
    .expect("historical revoked owner insert must stay legal");

    // Agent 仍 active：若没有 revoked-terminal 守卫，这个 Owner 可以被直接
    // 改回 active（active-agent 检查此时不会触发）。
    let reactivate_owner = sqlx::query(
        "UPDATE agent_owners SET status = 'active', revoked_at = NULL \
         WHERE agent_id = ? AND human_id = ?",
    )
    .bind(agent.id.as_str())
    .bind(bob.id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        reactivate_owner.is_err(),
        "revoked owner must not be restored to active even under an active agent"
    );

    let rewrite_owner =
        sqlx::query("UPDATE agent_owners SET revoked_at = ? WHERE agent_id = ? AND human_id = ?")
            .bind(first_revoked_at + 1)
            .bind(agent.id.as_str())
            .bind(bob.id.as_str())
            .execute(f.store.pool())
            .await;
    assert!(
        rewrite_owner.is_err(),
        "revoked_at of revoked owner must not be rewritten"
    );

    let (owner_status, owner_revoked_at): (String, Option<i64>) = sqlx::query_as(
        "SELECT status, revoked_at FROM agent_owners WHERE agent_id = ? AND human_id = ?",
    )
    .bind(agent.id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(owner_status, "revoked");
    assert_eq!(owner_revoked_at, Some(first_revoked_at));

    // 同一 Agent 的 active Owner 首次撤销（store 公共路径）仍必须通过全部守卫。
    agent::revoke_agent(&f.agent, &alice.participant.id, agent.id.as_str())
        .await
        .expect("first owner revoke must pass new triggers");

    let (owner_status, owner_revoked_at): (String, Option<i64>) = sqlx::query_as(
        "SELECT status, revoked_at FROM agent_owners WHERE agent_id = ? AND human_id = ?",
    )
    .bind(agent.id.as_str())
    .bind(alice.participant.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(owner_status, "revoked");
    let store_revoked_at = owner_revoked_at.expect("revoked owner must carry revoked_at");

    let rewrite_store_revoked =
        sqlx::query("UPDATE agent_owners SET revoked_at = ? WHERE agent_id = ? AND human_id = ?")
            .bind(store_revoked_at + 1)
            .bind(agent.id.as_str())
            .bind(alice.participant.id.as_str())
            .execute(f.store.pool())
            .await;
    assert!(
        rewrite_store_revoked.is_err(),
        "revoked_at rewrite must be rejected for store-revoked owner too"
    );
}

#[tokio::test]
async fn db_triggers_keep_first_and_repeat_public_revoke_legal() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let created = agent::create_agent(&f.agent, &alice.participant, "助手", "k")
        .await
        .expect("create");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("created");
    };

    // 首次撤销（裸 SQL 复现 store 的两步顺序：先 Agent revoked 再 Owner revoked）。
    let revoked_at = UtcMillis::now().millis();
    sqlx::query("UPDATE participants SET status = 'revoked', revoked_at = ? WHERE id = ?")
        .bind(revoked_at)
        .bind(agent.id.as_str())
        .execute(f.store.pool())
        .await
        .expect("first agent revoke must pass lifecycle triggers");
    sqlx::query("UPDATE agent_owners SET status = 'revoked', revoked_at = ? WHERE agent_id = ?")
        .bind(revoked_at)
        .bind(agent.id.as_str())
        .execute(f.store.pool())
        .await
        .expect("first owner revoke must pass lifecycle triggers");

    // 重复公开撤销（store 路径）必须不退化：仍成功，且不改变原 revoked_at。
    agent::revoke_agent(&f.agent, &alice.participant.id, agent.id.as_str())
        .await
        .expect("repeat public revoke must stay allowed");

    let (_, _, agent_revoked_at): (String, String, Option<i64>) =
        sqlx::query_as("SELECT kind, status, revoked_at FROM participants WHERE id = ?")
            .bind(agent.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(agent_revoked_at, Some(revoked_at));

    let (owner_status, owner_revoked_at): (String, Option<i64>) =
        sqlx::query_as("SELECT status, revoked_at FROM agent_owners WHERE agent_id = ?")
            .bind(agent.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(owner_status, "revoked");
    assert_eq!(owner_revoked_at, Some(revoked_at));

    // 终态守卫在重复撤销后仍然生效。
    let reactivate =
        sqlx::query("UPDATE participants SET status = 'active', revoked_at = NULL WHERE id = ?")
            .bind(agent.id.as_str())
            .execute(f.store.pool())
            .await;
    assert!(
        reactivate.is_err(),
        "revoked agent must remain terminal after repeat revoke"
    );
}

#[tokio::test]
async fn db_checks_reject_inconsistent_participant_rows() {
    let f = fixture().await;
    for (kind, status, revoked_at) in [
        ("robot", "active", None),
        ("human", "revoked", Some(1)),
        ("agent", "suspended", None),
        ("agent", "active", Some(1)),
        ("agent", "revoked", None),
        ("human", "active", Some(1)),
    ] {
        let result = sqlx::query(
            "INSERT INTO participants (id, kind, status, display_name, created_at, revoked_at) \
             VALUES ('x', ?, ?, 'ok', 0, ?)",
        )
        .bind(kind)
        .bind(status)
        .bind(revoked_at)
        .execute(f.store.pool())
        .await;
        assert!(
            result.is_err(),
            "expected DB rejection for ({kind}, {status}, {revoked_at:?})"
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM participants")
            .fetch_one(f.store.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn db_checks_reject_invalid_idempotency_rows() {
    let f = fixture().await;
    let outcome = bootstrap_human(&f.identity, "Alice").await.unwrap();
    for key in ["", &"a".repeat(129), "a,b", "a b", " a", "a\u{0}b"] {
        let result = sqlx::query(
            "INSERT INTO agent_create_idempotency (human_id, key, request_digest, agent_id, created_at) \
             VALUES (?, ?, ?, ?, 0)",
        )
        .bind(outcome.participant.id.as_str())
        .bind(key)
        .bind("0".repeat(64))
        .bind(outcome.participant.id.as_str())
        .execute(f.store.pool())
        .await;
        assert!(
            result.is_err(),
            "expected DB rejection for idempotency key {key:?}"
        );
    }
    let bad_digest = sqlx::query(
        "INSERT INTO agent_create_idempotency (human_id, key, request_digest, agent_id, created_at) \
         VALUES (?, 'ok-key', 'short', ?, 0)",
    )
    .bind(outcome.participant.id.as_str())
    .bind(outcome.participant.id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(bad_digest.is_err(), "digest length CHECK must fire");
    assert_eq!(count_idempotency(&f).await, 0);
}

#[tokio::test]
async fn db_trigger_enforces_human_to_agent_direction() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let created = agent::create_agent(&f.agent, &alice.participant, "助手", "k")
        .await
        .expect("create");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("created");
    };

    for (human, owned) in [
        (alice.participant.id.as_str(), bob.id.as_str()),
        (alice.participant.id.as_str(), alice.participant.id.as_str()),
        (agent.id.as_str(), agent.id.as_str()),
    ] {
        let result = sqlx::query(
            "INSERT INTO agent_owners (agent_id, human_id, status, created_at, revoked_at) \
             VALUES (?, ?, 'active', 0, NULL)",
        )
        .bind(owned)
        .bind(human)
        .execute(f.store.pool())
        .await;
        assert!(
            result.is_err(),
            "direction trigger must reject human={human:?} agent={owned:?}"
        );
    }

    let duplicate = sqlx::query(
        "INSERT INTO agent_owners (agent_id, human_id, status, created_at, revoked_at) \
         VALUES (?, ?, 'active', 0, NULL)",
    )
    .bind(agent.id.as_str())
    .bind(alice.participant.id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        duplicate.is_err(),
        "PK must forbid a second owner row per pair"
    );
}

#[tokio::test]
async fn db_triggers_reject_owner_direction_flips_via_update() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let created = agent::create_agent(&f.agent, &alice.participant, "助手", "k")
        .await
        .expect("create");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("created");
    };

    let flip_human =
        sqlx::query("UPDATE agent_owners SET human_id = ? WHERE agent_id = ? AND human_id = ?")
            .bind(agent.id.as_str())
            .bind(agent.id.as_str())
            .bind(alice.participant.id.as_str())
            .execute(f.store.pool())
            .await;
    assert!(
        flip_human.is_err(),
        "UPDATE must not point human_id at an agent"
    );

    let flip_agent =
        sqlx::query("UPDATE agent_owners SET agent_id = ? WHERE agent_id = ? AND human_id = ?")
            .bind(bob.id.as_str())
            .bind(agent.id.as_str())
            .bind(alice.participant.id.as_str())
            .execute(f.store.pool())
            .await;
    assert!(
        flip_agent.is_err(),
        "UPDATE must not point agent_id at a human"
    );

    assert_eq!(count_owners(&f).await, 1, "failed updates must not mutate");
    let (human_id, agent_id): (String, String) =
        sqlx::query_as("SELECT human_id, agent_id FROM agent_owners")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(human_id, alice.participant.id.as_str());
    assert_eq!(agent_id, agent.id.as_str());
}

#[tokio::test]
async fn db_trigger_rejects_active_owner_for_revoked_agent() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let created = agent::create_agent(&f.agent, &alice.participant, "助手", "k")
        .await
        .expect("create");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("created");
    };

    agent::revoke_agent(&f.agent, &alice.participant.id, agent.id.as_str())
        .await
        .expect("revoke");

    let active_for_revoked = sqlx::query(
        "INSERT INTO agent_owners (agent_id, human_id, status, created_at, revoked_at) \
         VALUES (?, ?, 'active', 0, NULL)",
    )
    .bind(agent.id.as_str())
    .bind(bob.id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        active_for_revoked.is_err(),
        "active owner must require active agent"
    );

    let reactivate = sqlx::query(
        "UPDATE agent_owners SET status = 'active', revoked_at = NULL WHERE agent_id = ?",
    )
    .bind(agent.id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        reactivate.is_err(),
        "owner must not be reactivated under a revoked agent"
    );

    let historical = sqlx::query(
        "INSERT INTO agent_owners (agent_id, human_id, status, created_at, revoked_at) \
         VALUES (?, ?, 'revoked', 0, ?)",
    )
    .bind(agent.id.as_str())
    .bind(bob.id.as_str())
    .bind(1)
    .execute(f.store.pool())
    .await;
    assert!(
        historical.is_ok(),
        "historical revoked owner may point at revoked agent"
    );
    assert_eq!(count_owners(&f).await, 2);
}

#[tokio::test]
async fn db_triggers_reject_idempotency_direction_violations() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let created = agent::create_agent(&f.agent, &alice.participant, "助手", "k")
        .await
        .expect("create");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("created");
    };
    let digest = "0".repeat(64);

    let agent_as_human = sqlx::query(
        "INSERT INTO agent_create_idempotency (human_id, key, request_digest, agent_id, created_at) \
         VALUES (?, 'k-agent-human', ?, ?, 0)",
    )
    .bind(agent.id.as_str())
    .bind(&digest)
    .bind(agent.id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        agent_as_human.is_err(),
        "human_id must not reference an agent"
    );

    let human_as_agent = sqlx::query(
        "INSERT INTO agent_create_idempotency (human_id, key, request_digest, agent_id, created_at) \
         VALUES (?, 'k-human-agent', ?, ?, 0)",
    )
    .bind(alice.participant.id.as_str())
    .bind(&digest)
    .bind(alice.participant.id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        human_as_agent.is_err(),
        "agent_id must not reference a human"
    );

    let update_human =
        sqlx::query("UPDATE agent_create_idempotency SET human_id = ? WHERE key = ?")
            .bind(agent.id.as_str())
            .bind("k")
            .execute(f.store.pool())
            .await;
    assert!(
        update_human.is_err(),
        "UPDATE must not point human_id at an agent"
    );

    let update_agent =
        sqlx::query("UPDATE agent_create_idempotency SET agent_id = ? WHERE key = ?")
            .bind(alice.participant.id.as_str())
            .bind("k")
            .execute(f.store.pool())
            .await;
    assert!(
        update_agent.is_err(),
        "UPDATE must not point agent_id at a human"
    );

    assert_eq!(
        count_idempotency(&f).await,
        1,
        "failed inserts and updates must not mutate"
    );
    let (human_id, agent_id): (String, String) =
        sqlx::query_as("SELECT human_id, agent_id FROM agent_create_idempotency WHERE key = 'k'")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(human_id, alice.participant.id.as_str());
    assert_eq!(agent_id, agent.id.as_str());
}

#[tokio::test]
async fn db_fk_forbids_idempotency_records_crossing_owners() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let created = agent::create_agent(&f.agent, &alice.participant, "A 助手", "k-a")
        .await
        .expect("alice create");
    let agent::CreateAgentOutcome::Created { agent: agent_a, .. } = created else {
        panic!("alice create must be Created");
    };
    let created = agent::create_agent(&f.agent, &bob, "B 助手", "k-b")
        .await
        .expect("bob create");
    let agent::CreateAgentOutcome::Created { agent: agent_b, .. } = created else {
        panic!("bob create must be Created");
    };
    let digest = "0".repeat(64);

    // A 的幂等 key 不能指向 B 的 Agent：agent_owners 中不存在 (B 的 Agent, A) 对。
    let cross = sqlx::query(
        "INSERT INTO agent_create_idempotency (human_id, key, request_digest, agent_id, created_at) \
         VALUES (?, 'k-cross', ?, ?, 0)",
    )
    .bind(alice.participant.id.as_str())
    .bind(&digest)
    .bind(agent_b.id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        cross.is_err(),
        "composite FK must reject idempotency record for an agent the human does not own"
    );

    // UPDATE 也不能跨 Owner：agent_id 换到 B 的 Agent、human_id 换到 B 都要拒绝。
    let update_agent =
        sqlx::query("UPDATE agent_create_idempotency SET agent_id = ? WHERE key = 'k-a'")
            .bind(agent_b.id.as_str())
            .execute(f.store.pool())
            .await;
    assert!(
        update_agent.is_err(),
        "UPDATE must not move agent_id to an agent the human does not own"
    );

    let update_human =
        sqlx::query("UPDATE agent_create_idempotency SET human_id = ? WHERE key = 'k-a'")
            .bind(bob.id.as_str())
            .execute(f.store.pool())
            .await;
    assert!(
        update_human.is_err(),
        "UPDATE must not move human_id to a human that does not own the agent"
    );

    // 合法 own-agent 记录仍然可插入，且重放不退化。
    let own = sqlx::query(
        "INSERT INTO agent_create_idempotency (human_id, key, request_digest, agent_id, created_at) \
         VALUES (?, 'k-own', ?, ?, 0)",
    )
    .bind(alice.participant.id.as_str())
    .bind(&digest)
    .bind(agent_a.id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(own.is_ok(), "own-agent idempotency record must stay legal");

    let replay = agent::create_agent(&f.agent, &alice.participant, "A 助手", "k-a")
        .await
        .expect("own replay must not degrade");
    assert!(matches!(replay, agent::CreateAgentOutcome::Replayed { .. }));

    assert_eq!(
        count_idempotency(&f).await,
        3,
        "only legal records may persist"
    );
}

#[tokio::test]
async fn db_guards_keep_revocation_path_and_historical_records_legal() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let created = agent::create_agent(&f.agent, &alice.participant, "助手", "k")
        .await
        .expect("create");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("created");
    };

    // 撤销顺序：先 Agent revoked，再 Owner revoked。用裸 SQL 复现该顺序，
    // 证明触发器不阻止现有撤销路径。
    let revoked_at = UtcMillis::now().millis();
    sqlx::query("UPDATE participants SET status = 'revoked', revoked_at = ? WHERE id = ?")
        .bind(revoked_at)
        .bind(agent.id.as_str())
        .execute(f.store.pool())
        .await
        .expect("agent revoke step must pass triggers");
    sqlx::query("UPDATE agent_owners SET status = 'revoked', revoked_at = ? WHERE agent_id = ?")
        .bind(revoked_at)
        .bind(agent.id.as_str())
        .execute(f.store.pool())
        .await
        .expect("owner revoke step must pass triggers");

    // 历史数据：revoked Owner 与引用 revoked Agent 的幂等记录仍然合法。
    sqlx::query(
        "INSERT INTO agent_owners (agent_id, human_id, status, created_at, revoked_at) \
         VALUES (?, ?, 'revoked', 0, ?)",
    )
    .bind(agent.id.as_str())
    .bind(bob.id.as_str())
    .bind(revoked_at)
    .execute(f.store.pool())
    .await
    .expect("historical revoked owner must stay legal");

    sqlx::query(
        "INSERT INTO agent_create_idempotency (human_id, key, request_digest, agent_id, created_at) \
         VALUES (?, 'hist-key', ?, ?, 0)",
    )
    .bind(alice.participant.id.as_str())
    .bind("0".repeat(64))
    .bind(agent.id.as_str())
    .execute(f.store.pool())
    .await
    .expect("idempotency record for revoked agent must stay legal");

    assert_eq!(count_owners(&f).await, 2);
    assert_eq!(count_idempotency(&f).await, 2);
}

#[tokio::test]
async fn storage_allows_multiple_owners_for_same_agent() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let created = agent::create_agent(&f.agent, &alice.participant, "助手", "k")
        .await
        .expect("create");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("created");
    };

    let second = sqlx::query(
        "INSERT INTO agent_owners (agent_id, human_id, status, created_at, revoked_at) \
         VALUES (?, ?, 'active', 0, NULL)",
    )
    .bind(agent.id.as_str())
    .bind(bob.id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        second.is_ok(),
        "different humans may own the same agent in storage"
    );
    assert_eq!(count_owners(&f).await, 2);
}

#[tokio::test]
async fn schema_contains_no_deferred_objects() {
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
        "room_profile_versions",
        "room_profile_put_idempotency",
        "pairing_transactions",
    ];
    for table in &tables {
        assert!(
            expected.contains(&table.as_str()),
            "unexpected table {table:?} in schema"
        );
    }
    assert_eq!(tables.len(), expected.len());

    // Room/Membership/Message 契约已生效，rooms/room_memberships/messages
    // 属于当前 schema；Backend Binding/Credential 契约已生效（migration
    // 006），backend_bindings/backend_credentials 属于当前 schema；Agent
    // Event 契约已生效（migration 007），agent_events 属于当前 schema；仍
    // 禁止任何延期对象（Completion、ACK、Lease、Replay、Runtime 等）。
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
async fn upgrade_from_v2_preserves_human_data_and_stays_ready() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("core.db");

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
        sqlx::raw_sql(include_str!(
            "../../../migrations/202608110001_core_baseline.sql"
        ))
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/202608110002_human_identity.sql"
        ))
        .execute(&mut *conn)
        .await
        .unwrap();
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
        for version in [202608110001i64, 202608110002] {
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

    let identity_v2 = SqliteIdentityStore::new(pool.clone());
    let outcome = bootstrap_human(&identity_v2, "Alice")
        .await
        .expect("bootstrap on v2 schema");

    let store = MigrationStore::open(&db_path)
        .await
        .expect("upgrade must apply v3");
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
        sqlx::query_as("SELECT kind, status FROM participants")
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(kind, "human", "existing human must survive the rebuild");
    assert_eq!(status_row, "active");

    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM human_sessions")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(sessions, 1, "existing session must survive the rebuild");

    let ctx = crate::application::identity::authenticate(&identity_v2, outcome.token.as_str())
        .await
        .expect("identity must keep working after upgrade");
    assert_eq!(ctx.participant.id, outcome.participant.id);

    let agent_store = SqliteAgentStore::new(store.pool().clone());
    let created = agent::create_agent(&agent_store, &ctx.participant, "升级后", "upgrade-key")
        .await
        .expect("agent create after upgrade");
    assert!(matches!(created, agent::CreateAgentOutcome::Created { .. }));
}

#[tokio::test]
async fn list_includes_revoked_and_keeps_revoked_visible() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let created = agent::create_agent(&f.agent, &alice.participant, "助手", "k")
        .await
        .expect("create");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("created");
    };

    agent::revoke_agent(&f.agent, &alice.participant.id, agent.id.as_str())
        .await
        .expect("revoke");

    let list = agent::list_agents(&f.agent, &alice.participant.id)
        .await
        .expect("list after revoke");
    assert_eq!(list.len(), 1, "revoked agent must stay listed");
    assert_eq!(list[0].agent.status.as_str(), "revoked");
    assert_eq!(list[0].ownership.status.as_str(), "revoked");
    assert!(list[0].agent.revoked_at.is_some());
}

#[tokio::test]
async fn invalid_agent_ids_and_non_existent_keys_rejected_at_application() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();

    let err = agent::create_agent(&f.agent, &alice.participant, "助手", "")
        .await
        .unwrap_err();
    assert!(
        matches!(err, agent::CreateAgentError::InvalidIdempotencyKey),
        "empty key must be invalid, got {err}"
    );

    let err = agent::create_agent(&f.agent, &alice.participant, "助手", "a,b")
        .await
        .unwrap_err();
    assert!(
        matches!(err, agent::CreateAgentError::InvalidIdempotencyKey),
        "comma key must be invalid, got {err}"
    );

    let err = agent::create_agent(&f.agent, &alice.participant, "   ", "ok-key")
        .await
        .unwrap_err();
    assert!(
        matches!(err, agent::CreateAgentError::DisplayName(_)),
        "blank display name must be rejected, got {err}"
    );

    assert_eq!(count_agents(&f).await, 0);
    assert_eq!(count_idempotency(&f).await, 0);

    let unknown = ParticipantId("00000000-0000-0000-0000-000000000000".into());
    assert!(
        f.agent
            .get_owned_agent(&alice.participant.id, &unknown)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        f.agent
            .list_owned_agents(&unknown)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn cancelled_create_agent_rolls_back_before_connection_reuse() {
    let f = fixture().await;
    // 单连接池保证 abort 后的查询和下一次 create 必须复用同一物理连接；
    // 如果 Transaction Drop 没有在归池前回滚，后续写会撞上遗留的
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
    let cancel_agent = SqliteAgentStore::new(cancel_pool.clone());
    let next_agent = cancel_agent.clone();
    let alice = bootstrap_human(&cancel_identity, "Alice")
        .await
        .unwrap()
        .participant;
    let alice_next = alice.clone();

    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
    let (released_tx, released_rx) = tokio::sync::oneshot::channel();
    cancel_agent.arm_commit_checkpoint(reached_tx, released_rx);

    // create_agent 写事务在「agent + owner + 幂等记录已写入、commit 前」
    // 挂起，等待测试 abort。
    let handle = tokio::spawn(async move {
        agent::create_agent(&cancel_agent, &alice, "助手", "cancel-key").await
    });

    tokio::time::timeout(std::time::Duration::from_secs(10), reached_rx)
        .await
        .expect("agent create must reach the commit checkpoint in time")
        .expect("agent create must not fail before the checkpoint");
    handle.abort();
    let join_error = handle.await.expect_err("task must be cancelled");
    assert!(join_error.is_cancelled());
    drop(released_tx);

    let agents: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM participants WHERE kind = 'agent'")
        .fetch_one(&cancel_pool)
        .await
        .expect("same pooled connection must recover after cancellation");
    assert_eq!(agents, 0, "cancelled create must leave no agent");
    let owners: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_owners")
        .fetch_one(&cancel_pool)
        .await
        .expect("same pooled connection must recover after cancellation");
    assert_eq!(owners, 0, "cancelled create must leave no owner");
    let idempotency: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_create_idempotency")
        .fetch_one(&cancel_pool)
        .await
        .expect("same pooled connection must recover after cancellation");
    assert_eq!(
        idempotency, 0,
        "cancelled create must leave no idempotency record"
    );

    let created = agent::create_agent(&next_agent, &alice_next, "助手", "cancel-key")
        .await
        .expect("next agent create on the same single connection must succeed");
    let agent::CreateAgentOutcome::Created { .. } = created else {
        panic!("agent create after cancellation must be Created, not Replayed");
    };
    assert_eq!(count_agents(&f).await, 1);
    assert_eq!(count_owners(&f).await, 1);
    assert_eq!(count_idempotency(&f).await, 1);
}
