//! SQLite Direct Room adapter 测试：真实临时数据库上的创建、幂等、可见性、
//! Agent 撤销传播、deferred 复合外键完整性、终态守卫、并发唯一与升级路径。

use std::sync::Arc;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use uuid::Uuid;

use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::migration_store::{MIGRATOR, MigrationStore};
use crate::adapters::sqlite::room_store::SqliteRoomStore;
use crate::application::agent;
use crate::application::identity::bootstrap_human;
use crate::application::room as room_application;
use crate::domain::agent::IdempotencyKey;
use crate::domain::human_session::{HumanSession, SessionId, TokenHash};
use crate::domain::participant::{DisplayName, Participant, ParticipantId};
use crate::domain::room::{
    AttentionLevel, MembershipClosedBy, MembershipId, MembershipRole, MembershipStatus, PeerRead,
    RoomId,
};
use crate::domain::time::UtcMillis;
use crate::domain::token::OpaqueToken;
use crate::ports::identity::{IdentityStore as _, NewSession};
use crate::ports::migration::{MigrationProbe, MigrationStatus};
use crate::ports::room::{CreateDirectRoomOutcome, NewDirectRoom, RoomStore as _};

struct Fixture {
    _dir: tempfile::TempDir,
    store: MigrationStore,
    identity: SqliteIdentityStore,
    agent: SqliteAgentStore,
    room: SqliteRoomStore,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("open + migrate");
    let pool = store.pool().clone();
    Fixture {
        _dir: dir,
        identity: SqliteIdentityStore::new(pool.clone()),
        agent: SqliteAgentStore::new(pool.clone()),
        room: SqliteRoomStore::new(pool.clone()),
        store,
    }
}

/// 通过 schema 直接创建第二个 Human 并为其补发 Session。
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

/// 通过公开用例创建 Direct Room（无标题），返回 Room ID。
async fn create_room(
    f: &Fixture,
    human: &Participant,
    agent_id: &str,
    key: &str,
) -> (RoomId, Vec<crate::domain::room::Membership>) {
    let outcome = room_application::create_direct_room(&f.room, human, agent_id, key)
        .await
        .expect("create direct room");
    let room_application::CreateDirectRoomOutcome::Created { room, memberships } = outcome else {
        panic!("first room create must be Created");
    };
    (room.id, memberships)
}

/// 通过公开用例创建 group Room，返回 Room ID。
async fn create_group(f: &Fixture, owner: &Participant, name: &str, key: &str) -> RoomId {
    let outcome = room_application::create_group(&f.room, owner, name, key)
        .await
        .expect("create group");
    let room_application::CreateRoomOutcome::Created { room, .. } = outcome else {
        panic!("first group create must be Created");
    };
    room.id
}

async fn count_rooms(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM rooms")
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

async fn count_pairs(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM direct_room_pairs")
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

async fn count_memberships(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM room_memberships")
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

async fn count_room_idempotency(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM direct_room_idempotency")
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn create_persists_room_pair_memberships_and_idempotency_atomically() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;

    let (room_id, memberships) =
        create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    assert_eq!(count_rooms(&f).await, 1);
    assert_eq!(count_pairs(&f).await, 1);
    assert_eq!(count_memberships(&f).await, 2);
    assert_eq!(count_room_idempotency(&f).await, 1);

    let (kind, status): (String, String) =
        sqlx::query_as("SELECT kind, status FROM rooms WHERE id = ?")
            .bind(room_id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(kind, "direct");
    assert_eq!(status, "active");

    assert_eq!(memberships.len(), 2);
    assert_eq!(memberships[0].participant_id, alice.participant.id);
    assert_eq!(memberships[0].role.as_str(), "owner");
    assert_eq!(memberships[0].status, MembershipStatus::Active);
    assert_eq!(memberships[1].participant_id, agent_id);
    assert_eq!(memberships[1].role.as_str(), "member");
    assert_eq!(memberships[1].status, MembershipStatus::Active);

    let (human_id, pair_agent): (String, String) =
        sqlx::query_as("SELECT human_id, agent_id FROM direct_room_pairs")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(human_id, alice.participant.id.as_str());
    assert_eq!(pair_agent, agent_id.as_str());

    let (stored_key, stored_agent, stored_room): (String, String, String) =
        sqlx::query_as("SELECT key, agent_id, room_id FROM direct_room_idempotency")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(stored_key, "r-k");
    assert_eq!(stored_agent, agent_id.as_str());
    assert_eq!(stored_room, room_id.as_str());
}

#[tokio::test]
async fn creating_agent_alone_creates_no_room_or_membership() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    create_agent(&f, &alice.participant, "助手", "a-k").await;

    assert_eq!(count_rooms(&f).await, 0);
    assert_eq!(count_pairs(&f).await, 0);
    assert_eq!(count_memberships(&f).await, 0);
    assert_eq!(count_room_idempotency(&f).await, 0);

    let list = room_application::list_rooms(&f.room, &alice.participant.id)
        .await
        .expect("list");
    assert!(list.is_empty(), "ownership must not create membership");
}

#[tokio::test]
async fn replay_same_key_same_agent_returns_same_room_and_adds_nothing() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let (room_id, _) = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    let outcome =
        room_application::create_direct_room(&f.room, &alice.participant, agent_id.as_str(), "r-k")
            .await
            .expect("replay");
    let room_application::CreateDirectRoomOutcome::Replayed { room, memberships } = outcome else {
        panic!("replay must be Replayed");
    };
    assert_eq!(room.id, room_id, "replay must return the same room");
    assert_eq!(memberships.len(), 2);
    assert_eq!(memberships[0].participant_id, alice.participant.id);

    assert_eq!(count_rooms(&f).await, 1);
    assert_eq!(count_memberships(&f).await, 2);
    assert_eq!(count_room_idempotency(&f).await, 1);
}

#[tokio::test]
async fn same_key_different_agent_conflicts_and_leaves_no_side_effects() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_a = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let agent_b = create_agent(&f, &alice.participant, "乙", "a-k2").await;
    create_room(&f, &alice.participant, agent_a.as_str(), "r-k").await;

    let err =
        room_application::create_direct_room(&f.room, &alice.participant, agent_b.as_str(), "r-k")
            .await
            .unwrap_err();
    assert!(
        matches!(
            err,
            room_application::CreateDirectRoomError::IdempotencyConflict
        ),
        "expected conflict, got {err}"
    );

    assert_eq!(count_rooms(&f).await, 1);
    assert_eq!(count_memberships(&f).await, 2);
    assert_eq!(count_room_idempotency(&f).await, 1);

    let replay =
        room_application::create_direct_room(&f.room, &alice.participant, agent_a.as_str(), "r-k")
            .await
            .expect("original request must still replay");
    assert!(
        matches!(
            replay,
            room_application::CreateDirectRoomOutcome::Replayed { .. }
        ),
        "first facts must not be overwritten"
    );
}

#[tokio::test]
async fn unknown_not_owned_and_bad_agent_ids_are_uniformly_not_found() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let agent_a = create_agent(&f, &alice.participant, "甲", "a-k1").await;

    for (human, agent_id) in [
        (&bob, agent_a.as_str()),                                     // not owned
        (&alice.participant, "00000000-0000-0000-0000-000000000000"), // unknown
        (&alice.participant, "not-a-uuid"),                           // bad id
        (&alice.participant, ""),                                     // bad id
    ] {
        let err = room_application::create_direct_room(&f.room, human, agent_id, "fresh-key")
            .await
            .unwrap_err();
        assert!(
            matches!(err, room_application::CreateDirectRoomError::NotFound),
            "expected NotFound for {agent_id:?}, got {err}"
        );
    }
    assert_eq!(count_rooms(&f).await, 0);
    assert_eq!(count_room_idempotency(&f).await, 0);
}

#[tokio::test]
async fn agent_id_is_canonicalized_before_comparison() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    let upper = agent_id.as_str().to_uppercase();
    let outcome = room_application::create_direct_room(&f.room, &alice.participant, &upper, "r-k")
        .await
        .expect("uppercase agent_id must normalize and replay");
    assert!(matches!(
        outcome,
        room_application::CreateDirectRoomOutcome::Replayed { .. }
    ));
    assert_eq!(count_rooms(&f).await, 1);
}

#[tokio::test]
async fn owned_but_revoked_agent_or_owner_returns_agent_not_active() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "甲", "a-k1").await;

    agent::revoke_agent(&f.agent, &alice.participant.id, agent_id.as_str())
        .await
        .expect("revoke");

    let err = room_application::create_direct_room(
        &f.room,
        &alice.participant,
        agent_id.as_str(),
        "fresh",
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, room_application::CreateDirectRoomError::AgentNotActive),
        "expected agent_not_active, got {err}"
    );
    assert_eq!(count_rooms(&f).await, 0);
    assert_eq!(count_room_idempotency(&f).await, 0);
}

#[tokio::test]
async fn different_keys_same_pair_return_the_same_room() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "甲", "a-k1").await;

    // 批次二「一间房 = 一段关系」：同一 Human–Agent 对**只有一间私人房**
    //（每段 Agent Owner 关系一间）。第二个 key 再要一次 → **Replayed** 回原房。
    let (room_1, _) = create_room(&f, &alice.participant, agent_id.as_str(), "r-k1").await;
    let replay = room_application::create_direct_room(
        &f.room,
        &alice.participant,
        agent_id.as_str(),
        "r-k2",
    )
    .await
    .expect("second create for the same pair must replay");
    let room_application::CreateDirectRoomOutcome::Replayed { room, memberships } = replay else {
        panic!("second create for the same pair must be Replayed");
    };
    assert_eq!(room.id, room_1, "replay returns the original room");
    assert_eq!(memberships.len(), 2);

    assert_eq!(count_rooms(&f).await, 1, "只有一个房间");
    assert_eq!(count_pairs(&f).await, 1, "只有一条配对");
    assert_eq!(count_memberships(&f).await, 2);
    assert_eq!(
        count_room_idempotency(&f).await,
        1,
        "**重放不写**：第二个 key 不落幂等记录（store 对 Replayed 不 commit）——\
         幂等依据是关系唯一性本身，不是键的账"
    );

    // 配对只一条，见证那段 Agent Owner 关系。
    // ⚠ 2026-09-28：原查询还取 `relation` 列并断言 `"private"` —— 服务层退役后
    // `direct_room_pairs` 只剩一种关系（没有判别列可断言了）。
    let pairs: Vec<(String, String, String)> =
        sqlx::query_as("SELECT room_id, human_id, agent_id FROM direct_room_pairs")
            .fetch_all(f.store.pool())
            .await
            .unwrap();
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].0, room_1.as_str());
    assert_eq!(pairs[0].1, alice.participant.id.as_str());
    assert_eq!(pairs[0].2, agent_id.as_str());

    // list 只一间；get/members 正常。
    let list = room_application::list_rooms(&f.room, &alice.participant.id)
        .await
        .expect("list");
    assert_eq!(list.len(), 1);
    let got = room_application::get_room(&f.room, &alice.participant.id, room_1.as_str())
        .await
        .expect("get room");
    assert_eq!(got.room.id, room_1);
    assert_eq!(got.room.name, None, "direct 房没有标题");
    let members =
        room_application::get_room_members(&f.room, &alice.participant.id, room_1.as_str())
            .await
            .expect("members");
    assert_eq!(members.len(), 2);
}

#[tokio::test]
async fn same_pair_second_room_is_rejected_by_the_witness_unique() {
    // 批次二「一间房 = 一段关系」：**一段关系一间**。同一对开第二间房在存储层
    // 就不可能 —— 后者也由 `different_keys_same_pair_return_the_same_room` 从
    // 另一侧钉住（replay 回原房）。
    //
    // ⚠ **2026-09-28（服务层退役）**：本用例原名
    // `same_pair_different_relation_is_the_only_way_to_a_second_room` —— 那时
    // "第二间房"还有一条正当来路（服务房 = 另一段关系）。服务层退役后那条路没了，
    // 判据本身**没有变松**：唯一性从「部分唯一索引 (human, agent) WHERE
    // relation='private'」换成**普通 `UNIQUE(human_id, agent_id)`**，本用例钉的
    // 仍是同一件事（第二条配对必须被数据库拒绝）。
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let (room_1, _) = create_room(&f, &alice.participant, agent_id.as_str(), "k-1").await;

    let second_room = uuid::Uuid::new_v4().to_string();
    let err = sqlx::query(
        "INSERT INTO direct_room_pairs \
         (room_id, human_id, agent_id, human_membership_id, agent_membership_id, created_at) \
         VALUES (?, ?, ?, 'm-h', 'm-a', 1)",
    )
    .bind(&second_room)
    .bind(alice.participant.id.as_str())
    .bind(agent_id.as_str())
    .execute(f.store.pool())
    .await
    .unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("UNIQUE") || message.contains("direct room pair"),
        "第二条私人配对必须被拒（每段关系至多一间），got {message}"
    );
    assert_eq!(count_pairs(&f).await, 1, "拒绝之后仍然只有一条配对");
    assert_eq!(
        room_1.as_str(),
        sqlx::query_scalar::<_, String>("SELECT room_id FROM direct_room_pairs")
            .fetch_one(f.store.pool())
            .await
            .unwrap(),
        "那一条配对仍指向原房"
    );
}

#[tokio::test]
async fn replay_same_key_matches_agent() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_a = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let agent_b = create_agent(&f, &alice.participant, "乙", "a-k2").await;

    // 首次创建（direct 房**没有标题**——批次二，标题不再是 payload 的一部分）。
    let (room_id, _) = create_room(&f, &alice.participant, agent_a.as_str(), "r-k").await;

    // 同 key + 同 agent：重放原 Room，事实总数不增。
    let outcome =
        room_application::create_direct_room(&f.room, &alice.participant, agent_a.as_str(), "r-k")
            .await
            .expect("replay");
    let room_application::CreateDirectRoomOutcome::Replayed { room, .. } = outcome else {
        panic!("same key + same agent must replay");
    };
    assert_eq!(room.id, room_id, "replay returns the original room");
    assert_eq!(room.name, None, "direct 房没有标题");
    assert_eq!(count_rooms(&f).await, 1);
    assert_eq!(count_memberships(&f).await, 2);
    assert_eq!(count_room_idempotency(&f).await, 1);

    // 同 key 改 agent → 409，原事实不变。
    let err =
        room_application::create_direct_room(&f.room, &alice.participant, agent_b.as_str(), "r-k")
            .await
            .unwrap_err();
    assert!(
        matches!(
            err,
            room_application::CreateDirectRoomError::IdempotencyConflict
        ),
        "same key different agent must conflict, got {err}"
    );

    assert_eq!(count_rooms(&f).await, 1);
    assert_eq!(count_memberships(&f).await, 2);
    assert_eq!(count_room_idempotency(&f).await, 1);

    // 同一对（agent_a）**换个 key** 再要一次：也不是新开——回放同一间房
    //（「一间房 = 一段关系」；`Replayed` 不写，事务直接丢弃 ⇒ 幂等记录仍 1 条）。
    let outcome = room_application::create_direct_room(
        &f.room,
        &alice.participant,
        agent_a.as_str(),
        "r-k-another",
    )
    .await
    .expect("second key for the same pair replays");
    let room_application::CreateDirectRoomOutcome::Replayed { room, .. } = outcome else {
        panic!("another key for the same pair must replay the same room");
    };
    assert_eq!(room.id, room_id);
    assert_eq!(count_rooms(&f).await, 1);
    assert_eq!(count_room_idempotency(&f).await, 1);
}

#[tokio::test]
async fn different_humans_same_key_are_independent() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let agent_a = create_agent(&f, &alice.participant, "甲的", "a-k1").await;
    let agent_b = create_agent(&f, &bob, "乙的", "a-k2").await;

    let (room_a, _) = create_room(&f, &alice.participant, agent_a.as_str(), "shared-key").await;
    let (room_b, _) = create_room(&f, &bob, agent_b.as_str(), "shared-key").await;
    assert_ne!(room_a, room_b, "same key across humans must not conflict");

    assert_eq!(count_rooms(&f).await, 2);
    let alice_list = room_application::list_rooms(&f.room, &alice.participant.id)
        .await
        .expect("alice list");
    let bob_list = room_application::list_rooms(&f.room, &bob.id)
        .await
        .expect("bob list");
    assert_eq!(alice_list.len(), 1);
    assert_eq!(bob_list.len(), 1);
    assert_eq!(alice_list[0].room.id, room_a);
    assert_eq!(bob_list[0].room.id, room_b);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_create_same_key_creates_exactly_one_room() {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .unwrap();
    let pool = store.pool().clone();
    let identity = SqliteIdentityStore::new(pool.clone());
    let agent_store = Arc::new(SqliteAgentStore::new(pool.clone()));
    let room_store = Arc::new(SqliteRoomStore::new(pool.clone()));
    let alice = bootstrap_human(&identity, "Alice").await.unwrap();
    let participant = Arc::new(alice.participant);
    let agent_id = agent::create_agent(agent_store.as_ref(), &participant, "并发", "a-k")
        .await
        .expect("create agent");
    let agent::CreateAgentOutcome::Created { agent, .. } = agent_id else {
        panic!("agent must be Created");
    };
    let agent_id = Arc::new(agent.id);

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let room_store = room_store.clone();
        let participant = participant.clone();
        let agent_id = agent_id.clone();
        tasks.push(tokio::spawn(async move {
            room_application::create_direct_room(
                room_store.as_ref(),
                &participant,
                agent_id.as_str(),
                "concurrent-key",
            )
            .await
        }));
    }
    let mut created = 0;
    let mut replayed = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(room_application::CreateDirectRoomOutcome::Created { .. }) => created += 1,
            Ok(room_application::CreateDirectRoomOutcome::Replayed { .. }) => replayed += 1,
            Err(e) => panic!("unexpected concurrent create error: {e}"),
        }
    }
    assert_eq!(created, 1, "exactly one create must win");
    assert_eq!(replayed, 7);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM rooms")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM direct_room_idempotency")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM room_memberships")
            .fetch_one(&pool)
            .await
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn list_is_member_scoped_sorted_and_readable_after_revoke() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let agent_a = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let agent_b = create_agent(&f, &alice.participant, "乙", "a-k2").await;

    let (_room_a, _) = create_room(&f, &alice.participant, agent_a.as_str(), "r-k1").await;
    let (room_b, _) = create_room(&f, &alice.participant, agent_b.as_str(), "r-k2").await;

    agent::revoke_agent(&f.agent, &alice.participant.id, agent_b.as_str())
        .await
        .expect("revoke agent b");

    let alice_list = room_application::list_rooms(&f.room, &alice.participant.id)
        .await
        .expect("alice list");
    assert_eq!(alice_list.len(), 2, "revoked peer must stay listed");
    let mut expected = alice_list.clone();
    expected.sort_by(|a, b| {
        (a.room.created_at, a.room.id.as_str()).cmp(&(b.room.created_at, b.room.id.as_str()))
    });
    assert_eq!(
        alice_list, expected,
        "must be ordered by created_at then id"
    );

    let item_b = alice_list
        .iter()
        .find(|item| item.room.id == room_b)
        .expect("room b present");
    assert_eq!(item_b.peer.as_ref().unwrap().id, agent_b);
    assert_eq!(item_b.peer.as_ref().unwrap().status.as_str(), "revoked");
    assert_eq!(item_b.membership.participant_id, alice.participant.id);
    assert_eq!(item_b.membership.status, MembershipStatus::Active);

    let bob_list = room_application::list_rooms(&f.room, &bob.id)
        .await
        .expect("bob list");
    assert!(bob_list.is_empty(), "bob must not see alice's rooms");
}

#[tokio::test]
async fn get_and_members_are_member_scoped() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let agent_id = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let (room_id, _) = create_room(&f, &alice.participant, agent_id.as_str(), "r-k1").await;

    let own = room_application::get_room(&f.room, &alice.participant.id, room_id.as_str())
        .await
        .expect("own get");
    assert_eq!(own.room.id, room_id);
    assert_eq!(own.room.kind.as_str(), "direct");
    assert_eq!(own.room.status.as_str(), "active");
    assert_eq!(own.membership.participant_id, alice.participant.id);
    assert_eq!(own.membership.role.as_str(), "owner");
    assert_eq!(own.peer.as_ref().unwrap().id, agent_id);
    assert_eq!(own.peer.as_ref().unwrap().kind.as_str(), "agent");

    let members =
        room_application::get_room_members(&f.room, &alice.participant.id, room_id.as_str())
            .await
            .expect("own members");
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].membership.role.as_str(), "owner");
    assert_eq!(members[0].membership.participant_id, alice.participant.id);
    assert_eq!(members[0].participant.id, alice.participant.id);
    assert_eq!(members[1].membership.role.as_str(), "member");
    assert_eq!(members[1].membership.participant_id, agent_id);

    for (human, bad) in [
        (&bob, room_id.as_str()),                                     // not member
        (&alice.participant, "00000000-0000-0000-0000-000000000000"), // unknown
        (&alice.participant, "not-a-uuid"),                           // bad id
    ] {
        let err = room_application::get_room(&f.room, &human.id, bad)
            .await
            .unwrap_err();
        assert!(
            matches!(err, room_application::RoomQueryError::NotFound),
            "expected NotFound for {bad:?}, got {err}"
        );
        let err = room_application::get_room_members(&f.room, &human.id, bad)
            .await
            .unwrap_err();
        assert!(
            matches!(err, room_application::RoomQueryError::NotFound),
            "expected NotFound for {bad:?}, got {err}"
        );
    }
}

#[tokio::test]
async fn revoke_propagates_to_agent_membership_with_same_revoked_at() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let (room_id, _) = create_room(&f, &alice.participant, agent_id.as_str(), "r-k1").await;

    agent::revoke_agent(&f.agent, &alice.participant.id, agent_id.as_str())
        .await
        .expect("revoke");

    let (agent_status, agent_revoked_at): (String, Option<i64>) =
        sqlx::query_as("SELECT status, revoked_at FROM participants WHERE id = ?")
            .bind(agent_id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(agent_status, "revoked");
    let revoked_at = agent_revoked_at.expect("agent must carry revoked_at");

    let (owner_status, owner_revoked_at): (String, Option<i64>) =
        sqlx::query_as("SELECT status, revoked_at FROM agent_owners WHERE agent_id = ?")
            .bind(agent_id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(owner_status, "revoked");
    assert_eq!(
        owner_revoked_at,
        Some(revoked_at),
        "owner must share revoked_at"
    );

    let (m_active, m_left_at, m_closed_by): (i64, Option<i64>, Option<String>) = sqlx::query_as(
        "SELECT active_marker, left_at, closed_by FROM room_memberships \
         WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(agent_id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(m_active, 0, "agent membership must be closed");
    assert_eq!(m_closed_by.as_deref(), Some("agent_revoked"));
    assert_eq!(
        m_left_at,
        Some(revoked_at),
        "membership must share revoked_at"
    );

    let (human_m_active, human_m_left_at): (i64, Option<i64>) = sqlx::query_as(
        "SELECT active_marker, left_at FROM room_memberships \
         WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(alice.participant.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(human_m_active, 1);
    assert_eq!(human_m_left_at, None);

    let (room_status,): (String,) = sqlx::query_as("SELECT status FROM rooms WHERE id = ?")
        .bind(room_id.as_str())
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(room_status, "active", "room must stay active after revoke");

    agent::revoke_agent(&f.agent, &alice.participant.id, agent_id.as_str())
        .await
        .expect("repeat revoke");
    let (_, revoked_at_again): (String, Option<i64>) =
        sqlx::query_as("SELECT status, revoked_at FROM participants WHERE id = ?")
            .bind(agent_id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(
        revoked_at_again,
        Some(revoked_at),
        "repeat revoke must not change revoked_at"
    );

    let members =
        room_application::get_room_members(&f.room, &alice.participant.id, room_id.as_str())
            .await
            .expect("members after revoke");
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].membership.status, MembershipStatus::Active);
    assert_eq!(members[0].participant.id, alice.participant.id);
    assert_eq!(members[1].membership.status, MembershipStatus::Closed);
    assert_eq!(members[1].participant.status.as_str(), "revoked");

    let replay = room_application::create_direct_room(
        &f.room,
        &alice.participant,
        agent_id.as_str(),
        "r-k1",
    )
    .await
    .expect("old key replay must still work after revoke");
    let room_application::CreateDirectRoomOutcome::Replayed { room, memberships } = replay else {
        panic!("replay expected");
    };
    assert_eq!(
        room.id, room_id,
        "replay returns the same room after revoke"
    );
    let agent_m = memberships
        .iter()
        .find(|m| m.participant_id == agent_id)
        .expect("agent membership present");
    assert_eq!(
        agent_m.status,
        MembershipStatus::Closed,
        "replay reflects current state"
    );
    assert_eq!(agent_m.closed_by, Some(MembershipClosedBy::AgentRevoked));

    let err = room_application::create_direct_room(
        &f.room,
        &alice.participant,
        agent_id.as_str(),
        "new-key",
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, room_application::CreateDirectRoomError::AgentNotActive),
        "new key after revoke must be agent_not_active, got {err}"
    );
    assert_eq!(count_rooms(&f).await, 1);
    assert_eq!(count_memberships(&f).await, 2);
    assert_eq!(count_room_idempotency(&f).await, 1);
}

#[tokio::test]
async fn revoke_closes_agent_memberships_in_all_rooms_of_the_pair() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    // 同 Human–Agent 的**两个房间**（批次二后同一对不可能有两间私人房）：
    // 一间私人房（一段 Owner 关系）+ 一个两者同在的群（房本身就是关系）。
    // 撤销 Agent 时，它在**这两个房间**里的周期都必须被关闭。
    let (room_1, _) = create_room(&f, &alice.participant, agent_id.as_str(), "r-k1").await;
    let room_2 = create_group(&f, &alice.participant, "群", "g-k1").await;
    room_application::invite_group_member(
        &f.room,
        &alice.participant,
        room_2.as_str(),
        agent_id.as_str(),
        "i-k1",
    )
    .await
    .expect("invite agent into the group");

    agent::revoke_agent(&f.agent, &alice.participant.id, agent_id.as_str())
        .await
        .expect("revoke");

    // 两个房间的 Agent Membership 都被同一事务以 agent_revoked 关闭，
    // Human Membership 与 Room 都保持 active。
    for (label, room) in [("room1", &room_1), ("room2", &room_2)] {
        let rows: Vec<(String, i64, Option<i64>, Option<String>)> = sqlx::query_as(
            "SELECT m.participant_id, m.active_marker, m.left_at, m.closed_by \
             FROM room_memberships m WHERE m.room_id = ? ORDER BY m.participant_id",
        )
        .bind(room.as_str())
        .fetch_all(f.store.pool())
        .await
        .unwrap();
        assert_eq!(rows.len(), 2, "{label}: two memberships");
        let human_row = rows
            .iter()
            .find(|(p, _, _, _)| p == alice.participant.id.as_str())
            .expect("human membership");
        assert_eq!(human_row.1, 1, "{label}: human membership stays active");
        assert_eq!(human_row.2, None);
        let agent_row = rows
            .iter()
            .find(|(p, _, _, _)| p == agent_id.as_str())
            .expect("agent membership");
        assert_eq!(agent_row.1, 0, "{label}: agent membership closed");
        assert_eq!(agent_row.3.as_deref(), Some("agent_revoked"));
        assert!(agent_row.2.is_some(), "{label}: left_at set");
        let (room_status,): (String,) = sqlx::query_as("SELECT status FROM rooms WHERE id = ?")
            .bind(room.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
        assert_eq!(room_status, "active", "{label}: room stays active");
    }

    // 成员视图按**房 kind 各自的口径**仍可读：
    // · 私人房（direct）：回**两条周期**——Human active + Agent closed（周期保留）；
    // · 群（group）：成员名单是「现在谁在群里」⇒ 撤销后只剩 Human 一条。
    let direct_members =
        room_application::get_room_members(&f.room, &alice.participant.id, room_1.as_str())
            .await
            .expect("direct members after revoke");
    assert_eq!(direct_members.len(), 2);
    assert_eq!(
        direct_members[0].membership.status,
        MembershipStatus::Active
    );
    assert_eq!(
        direct_members[1].membership.status,
        MembershipStatus::Closed
    );
    assert_eq!(
        direct_members[1].membership.closed_by,
        Some(MembershipClosedBy::AgentRevoked)
    );

    let group_members =
        room_application::get_room_members(&f.room, &alice.participant.id, room_2.as_str())
            .await
            .expect("group members after revoke");
    assert_eq!(
        group_members.len(),
        1,
        "群成员名单是「现在谁在群里」：被撤销的 Agent 不再列出"
    );
    assert_eq!(
        group_members[0].membership.participant_id,
        alice.participant.id
    );
    assert_eq!(
        group_members[0].membership.status,
        MembershipStatus::Active,
        "Human 仍是群主，周期 active"
    );
    // 而**周期事实**仍在（上面第一个循环已逐房核过 `closed_by = agent_revoked`）。
}

#[tokio::test]
async fn db_deferred_fk_rejects_empty_and_half_rooms_at_commit() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let charlie = add_human(f.store.pool(), &f.identity, "Charlie").await;
    let agent_a = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let agent_b = create_agent(&f, &alice.participant, "乙", "a-k2").await;
    let (room_id, _) = create_room(&f, &alice.participant, agent_a.as_str(), "r-k1").await;

    let alice_id = alice.participant.id.as_str();
    let (a_ab, b_ab) = crate::domain::contact::canonical_pair(&alice.participant.id, &bob.id);
    let (a_ac, b_ac) = crate::domain::contact::canonical_pair(&alice.participant.id, &charlie.id);
    // alice 是创建者：owner 跟随 alice 的位置（canonical 两端 role 互异）。
    let (role_ac_a, role_ac_b) = if a_ac.as_str() == alice_id {
        ("owner", "member")
    } else {
        ("member", "owner")
    };
    // alice–charlie 的 accepted 关系段（human_direct 配对前置事实）。
    let period_ac = "30000000-0000-4000-8000-0000000000c1";
    let contact_ac = format!(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at, idempotency_key) \
         VALUES ('{period_ac}', '{a_ac}', '{b_ac}', '{a_ac}', '{b_ac}', 'accepted', 1, 1, 'k-ac')"
    );

    // 每个场景在事务内构造中间态：全部 INSERT 必须成功，只有 COMMIT 被
    // deferred 复合外键拒绝——证明闭合来自 COMMIT 期外键而非提前的
    // INSERT 失败。
    // 每个场景用固定合法 UUID 作为 membership_id（表级 CHECK 只接受
    // 标准 v4 小写连字符），配对行见证奠基周期身份。
    let mid_a = "10000000-0000-4000-8000-00000000000a";
    let mid_b = "10000000-0000-4000-8000-00000000000b";
    let scenarios: Vec<(&str, String)> = vec![
        (
            "direct: rooms+coverage without pair",
            format!(
                "INSERT INTO rooms (id, kind, status, created_at, name) \
                 VALUES ('d-empty', 'direct', 'active', 10, NULL); \
                 INSERT INTO room_coverage \
                 (room_id, kind, created_at, direct_human_id, direct_agent_id) \
                 VALUES ('d-empty', 'direct', 10, '{alice_id}', '{}')",
                agent_a.as_str()
            ),
        ),
        (
            "human_direct: rooms+coverage without pair",
            format!(
                "INSERT INTO rooms (id, kind, status, created_at, name) \
                 VALUES ('hd-empty', 'human_direct', 'active', 11, NULL); \
                 INSERT INTO room_coverage \
                 (room_id, kind, created_at, human_direct_a_id, human_direct_b_id) \
                 VALUES ('hd-empty', 'human_direct', 11, '{a_ab}', '{b_ab}')"
            ),
        ),
        (
            "group: rooms+coverage without owner membership",
            format!(
                "INSERT INTO rooms (id, kind, status, created_at, name) \
                 VALUES ('g-empty', 'group', 'active', 12, 'g'); \
                 INSERT INTO room_coverage \
                 (room_id, kind, created_at, group_owner_membership_id, \
                  group_owner_role, group_owner_active_marker) \
                 VALUES ('g-empty', 'group', 12, '{mid_a}', 'owner', 1)"
            ),
        ),
        (
            "direct: pair without memberships",
            format!(
                "INSERT INTO rooms (id, kind, status, created_at, name) \
                 VALUES ('d-nomembers', 'direct', 'active', 13, NULL); \
                 INSERT INTO room_coverage \
                 (room_id, kind, created_at, direct_human_id, direct_agent_id) \
                 VALUES ('d-nomembers', 'direct', 13, '{alice_id}', '{}'); \
                 INSERT INTO direct_room_pairs \
                 (room_id, human_id, agent_id, human_membership_id, agent_membership_id, created_at) \
                 VALUES ('d-nomembers', '{alice_id}', '{}', '{mid_a}', '{mid_b}', 13)",
                agent_b.as_str(),
                agent_b.as_str()
            ),
        ),
        (
            "direct: pair with only the human membership",
            format!(
                "INSERT INTO rooms (id, kind, status, created_at, name) \
                 VALUES ('d-half-human', 'direct', 'active', 14, NULL); \
                 INSERT INTO room_coverage \
                 (room_id, kind, created_at, direct_human_id, direct_agent_id) \
                 VALUES ('d-half-human', 'direct', 14, '{alice_id}', '{}'); \
                 INSERT INTO direct_room_pairs \
                 (room_id, human_id, agent_id, human_membership_id, agent_membership_id, created_at) \
                 VALUES ('d-half-human', '{alice_id}', '{}', '{mid_a}', '{mid_b}', 14); \
                 INSERT INTO room_memberships \
                 (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
                 VALUES ('{mid_a}', 'd-half-human', '{alice_id}', 'owner', 14, 0, 1)",
                agent_b.as_str(),
                agent_b.as_str()
            ),
        ),
        (
            "direct: pair with only the agent membership",
            format!(
                "INSERT INTO rooms (id, kind, status, created_at, name) \
                 VALUES ('d-half-agent', 'direct', 'active', 15, NULL); \
                 INSERT INTO room_coverage \
                 (room_id, kind, created_at, direct_human_id, direct_agent_id) \
                 VALUES ('d-half-agent', 'direct', 15, '{alice_id}', '{}'); \
                 INSERT INTO direct_room_pairs \
                 (room_id, human_id, agent_id, human_membership_id, agent_membership_id, created_at) \
                 VALUES ('d-half-agent', '{alice_id}', '{}', '{mid_a}', '{mid_b}', 15); \
                 INSERT INTO room_memberships \
                 (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
                 VALUES ('{mid_b}', 'd-half-agent', '{}', 'member', 15, 0, 1)",
                agent_b.as_str(),
                agent_b.as_str(),
                agent_b.as_str()
            ),
        ),
        (
            "direct: coverage witness (alice, agent_a) but pair is (alice, agent_b)",
            format!(
                "INSERT INTO rooms (id, kind, status, created_at, name) \
                 VALUES ('d-wrongpair', 'direct', 'active', 16, NULL); \
                 INSERT INTO room_coverage \
                 (room_id, kind, created_at, direct_human_id, direct_agent_id) \
                 VALUES ('d-wrongpair', 'direct', 16, '{alice_id}', '{}'); \
                 INSERT INTO direct_room_pairs \
                 (room_id, human_id, agent_id, human_membership_id, agent_membership_id, created_at) \
                 VALUES ('d-wrongpair', '{alice_id}', '{}', '{mid_a}', '{mid_b}', 16); \
                 INSERT INTO room_memberships \
                 (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
                 VALUES ('{mid_a}', 'd-wrongpair', '{alice_id}', 'owner', 16, 0, 1), \
                        ('{mid_b}', 'd-wrongpair', '{}', 'member', 16, 0, 1)",
                agent_a.as_str(),
                agent_b.as_str(),
                agent_b.as_str()
            ),
        ),
        (
            "human_direct: coverage witness (alice, bob) but pair is (alice, charlie)",
            format!(
                "{contact_ac}; \
                 INSERT INTO rooms (id, kind, status, created_at, name) \
                 VALUES ('hd-wrongpair', 'human_direct', 'active', 17, NULL); \
                 INSERT INTO room_coverage \
                 (room_id, kind, created_at, human_direct_a_id, human_direct_b_id) \
                 VALUES ('hd-wrongpair', 'human_direct', 17, '{a_ab}', '{b_ab}'); \
                 INSERT INTO human_direct_pairs \
                 (room_id, human_a_id, human_b_id, contact_period_id, human_a_membership_id, human_b_membership_id, \
                  role_a, role_b, created_at) \
                 VALUES ('hd-wrongpair', '{a_ac}', '{b_ac}', '{period_ac}', '{mid_a}', '{mid_b}', \
                         '{role_ac_a}', '{role_ac_b}', 17); \
                 INSERT INTO room_memberships \
                 (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
                 VALUES ('{mid_a}', 'hd-wrongpair', '{a_ac}', '{role_ac_a}', 17, 0, 1), \
                        ('{mid_b}', 'hd-wrongpair', '{b_ac}', '{role_ac_b}', 17, 0, 1)"
            ),
        ),
        (
            "group: coverage owner witness alice but owner membership is bob",
            format!(
                "INSERT INTO rooms (id, kind, status, created_at, name) \
                 VALUES ('g-wrongowner', 'group', 'active', 18, 'g'); \
                 INSERT INTO room_coverage \
                 (room_id, kind, created_at, group_owner_membership_id, \
                  group_owner_role, group_owner_active_marker) \
                 VALUES ('g-wrongowner', 'group', 18, '{mid_a}', 'owner', 1); \
                 INSERT INTO room_memberships \
                 (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
                 VALUES ('{mid_b}', 'g-wrongowner', '{}', 'owner', 18, 0, 1)",
                bob.id.as_str()
            ),
        ),
        (
            "group: transition without closing/opening the four periods cannot commit",
            format!(
                "{contact_ac}; \
                 INSERT INTO rooms (id, kind, status, created_at, name) \
                 VALUES ('g-transitionless', 'group', 'active', 19, 'g'); \
                 INSERT INTO room_coverage \
                 (room_id, kind, created_at, group_owner_membership_id, \
                  group_owner_role, group_owner_active_marker) \
                 VALUES ('g-transitionless', 'group', 19, '{mid_a}', 'owner', 1); \
                 INSERT INTO room_memberships \
                 (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
                 VALUES ('{mid_a}', 'g-transitionless', '{alice_id}', 'owner', 19, 0, 1), \
                        ('{mid_b}', 'g-transitionless', '{}', 'member', 19, 0, 1); \
                 INSERT INTO group_owner_transitions \
                 (transition_id, room_id, old_owner_participant_id, target_participant_id, \
                  old_owner_membership_id, target_member_membership_id, new_owner_membership_id, \
                  old_owner_new_member_membership_id, occurred_at, boundary_sequence) \
                 VALUES ('20000000-0000-4000-8000-0000000000c1', 'g-transitionless', \
                         '{alice_id}', '{}', '{mid_a}', '{mid_b}', \
                         '20000000-0000-4000-8000-0000000000c2', \
                         '20000000-0000-4000-8000-0000000000c3', 19, 0)",
                charlie.id.as_str(),
                charlie.id.as_str(),
            ),
        ),
    ];

    for (label, setup) in &scenarios {
        let mut conn = f.store.pool().acquire().await.unwrap();
        sqlx::query("BEGIN").execute(&mut *conn).await.unwrap();
        let run = sqlx::query(setup).execute(&mut *conn).await;
        assert!(
            run.is_ok(),
            "{label}: every intermediate INSERT must succeed, got {run:?}"
        );
        let commit = sqlx::query("COMMIT").execute(&mut *conn).await;
        assert!(
            commit.is_err(),
            "{label}: commit must fail via the deferred closure FK"
        );
        let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
    }

    // 直接 SQL 把 group coverage 见证改写为 member 周期（非 owner / 非
    // active）必须在 UPDATE 阶段被拒绝（覆盖行只能见证同 room 的 active
    // owner 周期）。
    let bad_witness =
        sqlx::query("UPDATE room_coverage SET group_owner_membership_id = ? WHERE room_id = ?")
            .bind(mid_b)
            .bind(room_id.as_str())
            .execute(f.store.pool())
            .await;
    assert!(
        bad_witness.is_err(),
        "coverage must not re-witness to a non-owner membership period"
    );

    assert_eq!(count_rooms(&f).await, 1, "no unclosed room may persist");
    assert_eq!(count_pairs(&f).await, 1);
    assert_eq!(count_memberships(&f).await, 2);
    let _ = room_id;
}

#[tokio::test]
async fn db_deferred_fk_rejects_lone_room_at_commit() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let (room_id, _) = create_room(&f, &alice.participant, agent_id.as_str(), "r-k1").await;

    let mut conn = f.store.pool().acquire().await.unwrap();
    sqlx::query("BEGIN").execute(&mut *conn).await.unwrap();
    let run = sqlx::query(
        "INSERT INTO rooms (id, kind, status, created_at) \
         VALUES ('lone-1', 'direct', 'active', 50)",
    )
    .execute(&mut *conn)
    .await;
    assert!(
        run.is_ok(),
        "lone room insert must succeed inside the transaction"
    );
    let commit = sqlx::query("COMMIT").execute(&mut *conn).await;
    assert!(
        commit.is_err(),
        "commit of a lone rooms row must fail via the deferred rooms -> room_coverage FK"
    );
    let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;

    assert_eq!(count_rooms(&f).await, 1, "no lone room may persist");
    let _ = room_id;
}

/// 攻击式 DB 守卫（`rooms_insert_kind_name_guard`）：绕过应用层直接向
/// rooms 插入 `kind='human_direct'` 且 name 非 NULL 的行，必须被触发器在
/// INSERT 阶段拒绝（Human–Human 私聊没有标题事实源）。用合法 Room ID 与
/// 合法标题形状，确保身份守卫（id 已存在才触发）与表级 name CHECK 都不
/// 先于 kind/name 触发器成为拒绝原因；错误文字命中触发器消息。
#[tokio::test]
async fn db_guard_rejects_human_direct_room_with_non_null_name() {
    let f = fixture().await;

    let result = sqlx::query(
        "INSERT INTO rooms (id, kind, status, created_at, name) \
         VALUES (?, 'human_direct', 'active', 1, ?)",
    )
    .bind(RoomId::new_v4().as_str())
    .bind("标题")
    .execute(f.store.pool())
    .await;
    let err = result.expect_err("human_direct with non-null name must be rejected at insert");
    assert!(
        err.to_string()
            .contains("human_direct rooms must have a null name"),
        "rejection must come from the kind/name trigger, got: {err}"
    );

    // 对照：human_direct + NULL name 在事务内通过触发器（INSERT 成功，
    // 不 commit 以避免与守卫无关的 deferred 闭环约束）：拒绝原因确实是
    // name 非 NULL，而不是 human_direct 行本身非法。
    {
        let mut conn = f.store.pool().acquire().await.unwrap();
        sqlx::query("BEGIN").execute(&mut *conn).await.unwrap();
        let legal = sqlx::query(
            "INSERT INTO rooms (id, kind, status, created_at) \
             VALUES (?, 'human_direct', 'active', 2)",
        )
        .bind(RoomId::new_v4().as_str())
        .execute(&mut *conn)
        .await;
        assert!(
            legal.is_ok(),
            "human_direct with null name must still pass the trigger"
        );
        let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
    }

    // 被拒行绝不留库（INSERT 失败即整体无副作用）。
    assert_eq!(count_rooms(&f).await, 0);
}

#[tokio::test]
async fn db_membership_lifecycle_rejects_direct_revokes_and_keeps_legal_propagation() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let (room_id, _) = create_room(&f, &alice.participant, agent_id.as_str(), "r-k1").await;

    // Human owner Membership：active → closed 直接关闭必须被拒绝。
    let human_revoke = sqlx::query(
        "UPDATE room_memberships \
         SET active_marker = 0, left_at = 60, left_before_sequence = 1, closed_by = 'removed' \
         WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(alice.participant.id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        human_revoke.is_err(),
        "human owner membership must never be closed directly"
    );

    // active Agent Membership：Agent 仍 active 时直接关闭必须被拒绝。
    let agent_revoke_while_active = sqlx::query(
        "UPDATE room_memberships \
         SET active_marker = 0, left_at = 60, left_before_sequence = 1, closed_by = 'removed' \
         WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(agent_id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        agent_revoke_while_active.is_err(),
        "active agent membership must not be closed while the agent is active"
    );

    // 合法 Agent 撤销仍原子传播：Agent / Owner / Agent Membership 同
    // revoked_at，Room 与 Human Membership 保持 active。
    agent::revoke_agent(&f.agent, &alice.participant.id, agent_id.as_str())
        .await
        .expect("revoke");
    let (agent_status, agent_revoked_at): (String, Option<i64>) =
        sqlx::query_as("SELECT status, revoked_at FROM participants WHERE id = ?")
            .bind(agent_id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(agent_status, "revoked");
    let revoked_at = agent_revoked_at.expect("agent must carry revoked_at");

    let (owner_status, owner_revoked_at): (String, Option<i64>) =
        sqlx::query_as("SELECT status, revoked_at FROM agent_owners WHERE agent_id = ?")
            .bind(agent_id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(owner_status, "revoked");
    assert_eq!(owner_revoked_at, Some(revoked_at));

    let (m_active, m_left_at): (i64, Option<i64>) = sqlx::query_as(
        "SELECT active_marker, left_at FROM room_memberships \
         WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(agent_id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(m_active, 0);
    assert_eq!(m_left_at, Some(revoked_at));

    let (room_status,): (String,) = sqlx::query_as("SELECT status FROM rooms WHERE id = ?")
        .bind(room_id.as_str())
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(room_status, "active");

    let (human_m_active, human_m_left_at): (i64, Option<i64>) = sqlx::query_as(
        "SELECT active_marker, left_at FROM room_memberships \
         WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(alice.participant.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(human_m_active, 1);
    assert_eq!(human_m_left_at, None);

    // 已关闭的 Agent Membership 周期终态仍不可改写（恢复/改时间）。
    let restore = sqlx::query(
        "UPDATE room_memberships \
         SET active_marker = 1, left_at = NULL, left_before_sequence = NULL, closed_by = NULL \
         WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(agent_id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(restore.is_err(), "closed membership must not be restored");
    let rewrite = sqlx::query(
        "UPDATE room_memberships SET left_at = ? WHERE room_id = ? AND participant_id = ?",
    )
    .bind(revoked_at + 1)
    .bind(room_id.as_str())
    .bind(agent_id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        rewrite.is_err(),
        "left_at of a closed membership must not be rewritten"
    );

    // 重复撤销不改变首次 revoked_at。
    agent::revoke_agent(&f.agent, &alice.participant.id, agent_id.as_str())
        .await
        .expect("repeat revoke");
    let (_, revoked_at_again): (String, Option<i64>) =
        sqlx::query_as("SELECT status, revoked_at FROM participants WHERE id = ?")
            .bind(agent_id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(revoked_at_again, Some(revoked_at));

    assert_eq!(count_rooms(&f).await, 1);
    assert_eq!(count_memberships(&f).await, 2);
}

#[tokio::test]
async fn db_insert_guard_rejects_revoked_membership_rows() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "甲", "a-k1").await;

    // 已实证的绕过路径：新行直接以 revoked + revoked_at 插入，可与另一份
    // active Membership 一起提交。INSERT 守卫必须在写入时拒绝，Human
    // owner 与 Agent member 两条路径都要覆盖；角色/配对触发器先放行，
    // 恰好证明拒绝来自 active-only INSERT 守卫而非其他约束。
    for (label, participant, role) in [
        (
            "revoked human owner membership",
            alice.participant.id.as_str(),
            "owner",
        ),
        (
            "revoked agent member membership",
            agent_id.as_str(),
            "member",
        ),
    ] {
        let mut conn = f.store.pool().acquire().await.unwrap();
        sqlx::query("BEGIN").execute(&mut *conn).await.unwrap();
        sqlx::query(
            "INSERT INTO rooms (id, kind, status, created_at) \
             VALUES ('guard-room', 'direct', 'active', 30)",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO room_coverage \
             (room_id, kind, created_at, direct_human_id, direct_agent_id) \
             VALUES ('guard-room', 'direct', 30, ?, ?)",
        )
        .bind(alice.participant.id.as_str())
        .bind(agent_id.as_str())
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO direct_room_pairs \
             (room_id, human_id, agent_id, human_membership_id, agent_membership_id, created_at) \
             VALUES ('guard-room', ?, ?, '00000000-0000-4000-8000-000000000001', \
                     '00000000-0000-4000-8000-000000000002', 30)",
        )
        .bind(alice.participant.id.as_str())
        .bind(agent_id.as_str())
        .execute(&mut *conn)
        .await
        .unwrap();
        let result = sqlx::query(
            "INSERT INTO room_memberships \
             (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
             VALUES ('00000000-0000-4000-8000-000000000003', 'guard-room', ?, ?, 30, 0, 0)",
        )
        .bind(participant)
        .bind(role)
        .execute(&mut *conn)
        .await;
        assert!(
            result.is_err(),
            "INSERT of {label} must be rejected at insert time"
        );
        let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
    }
    assert_eq!(count_rooms(&f).await, 0, "no guarded row may persist");

    // 合法创建路径不受守卫影响：两份 active Membership 仍可原子提交。
    let (room_id, memberships) =
        create_room(&f, &alice.participant, agent_id.as_str(), "r-k1").await;
    assert_eq!(count_rooms(&f).await, 1);
    assert_eq!(count_memberships(&f).await, 2);
    assert!(
        memberships
            .iter()
            .all(|m| m.status == MembershipStatus::Active)
    );
    let _ = room_id;

    // Agent revoke 传播不受 INSERT 守卫影响：仍通过 UPDATE 路径变 revoked。
    agent::revoke_agent(&f.agent, &alice.participant.id, agent_id.as_str())
        .await
        .expect("revoke propagation must keep working");
    let (m_active, m_left_at, m_closed_by): (i64, Option<i64>, Option<String>) = sqlx::query_as(
        "SELECT active_marker, left_at, closed_by FROM room_memberships \
         WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(agent_id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(m_active, 0);
    assert_eq!(m_closed_by.as_deref(), Some("agent_revoked"));
    assert!(m_left_at.is_some(), "propagated revoke must carry left_at");
}

#[tokio::test]
async fn revoked_human_membership_fixture_fails_closed_for_list_get_and_members() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let (room_id, _) = create_room(&f, &alice.participant, agent_id.as_str(), "r-k1").await;

    // 构造异常库状态：临时移除 Human 永不撤销守卫后直接撤销 Human
    // owner Membership，验证查询 fail closed。
    sqlx::query("DROP TRIGGER room_memberships_human_close_guard")
        .execute(f.store.pool())
        .await
        .unwrap();
    let abnormal = sqlx::query(
        "UPDATE room_memberships \
         SET active_marker = 0, left_at = 70, left_before_sequence = 0, closed_by = 'removed' \
         WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(alice.participant.id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        abnormal.is_ok(),
        "abnormal closed human membership must be constructible after dropping the guard"
    );
    let (m_active,): (i64,) = sqlx::query_as(
        "SELECT active_marker FROM room_memberships WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(alice.participant.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(m_active, 0, "fixture must be abnormal as intended");

    let list = room_application::list_rooms(&f.room, &alice.participant.id)
        .await
        .expect("list");
    assert!(
        list.is_empty(),
        "revoked human membership must hide the room from list"
    );

    let get = f
        .room
        .get_room(&alice.participant.id, &room_id)
        .await
        .expect("get");
    assert!(get.is_none(), "get must fail closed to None");
    let members = f
        .room
        .get_room_members(&alice.participant.id, &room_id)
        .await
        .expect("members");
    assert!(members.is_none(), "members must fail closed to None");

    let get = room_application::get_room(&f.room, &alice.participant.id, room_id.as_str())
        .await
        .unwrap_err();
    assert!(
        matches!(get, room_application::RoomQueryError::NotFound),
        "application get must map to NotFound"
    );
    let members =
        room_application::get_room_members(&f.room, &alice.participant.id, room_id.as_str())
            .await
            .unwrap_err();
    assert!(
        matches!(members, room_application::RoomQueryError::NotFound),
        "application members must map to NotFound"
    );
}

#[tokio::test]
async fn db_rejects_third_member_wrong_role_duplicate_and_identity_mutation() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let agent_id = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let (room_id, _) = create_room(&f, &alice.participant, agent_id.as_str(), "r-k1").await;

    for (label, participant, role) in [
        ("third member", bob.id.as_str(), "member"),
        ("human as member", alice.participant.id.as_str(), "member"),
        ("agent as owner", agent_id.as_str(), "owner"),
        (
            "duplicate membership",
            alice.participant.id.as_str(),
            "owner",
        ),
    ] {
        let result = sqlx::query(
            "INSERT INTO room_memberships \
             (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
             VALUES (?, ?, ?, ?, 20, 0, 1)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(room_id.as_str())
        .bind(participant)
        .bind(role)
        .execute(f.store.pool())
        .await;
        assert!(result.is_err(), "DB must reject {label}");
    }

    let flip_role = sqlx::query(
        "UPDATE room_memberships SET role = 'member' WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(alice.participant.id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(flip_role.is_err(), "membership role must be immutable");

    let move_member = sqlx::query(
        "UPDATE room_memberships SET room_id = 'other' WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(agent_id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(move_member.is_err(), "membership room must be immutable");

    let move_pair = sqlx::query("UPDATE direct_room_pairs SET agent_id = ? WHERE room_id = ?")
        .bind(bob.id.as_str())
        .bind(room_id.as_str())
        .execute(f.store.pool())
        .await;
    assert!(move_pair.is_err(), "pair identity must be immutable");

    let archive = sqlx::query("UPDATE rooms SET status = 'archived' WHERE id = ?")
        .bind(room_id.as_str())
        .execute(f.store.pool())
        .await;
    assert!(archive.is_err(), "room status must be immutable");

    let delete_membership = sqlx::query("DELETE FROM room_memberships WHERE room_id = ?")
        .bind(room_id.as_str())
        .execute(f.store.pool())
        .await;
    assert!(
        delete_membership.is_err(),
        "memberships must not be deletable"
    );

    assert_eq!(count_memberships(&f).await, 2);
    assert_eq!(count_pairs(&f).await, 1);
    assert_eq!(count_rooms(&f).await, 1);
}

#[tokio::test]
async fn db_rejects_cross_pair_idempotency_and_preserves_legal_records() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let agent_a = create_agent(&f, &alice.participant, "甲的", "a-k1").await;
    let agent_b = create_agent(&f, &bob, "乙的", "a-k2").await;
    let (room_a, _) = create_room(&f, &alice.participant, agent_a.as_str(), "r-k1").await;
    let (room_b, _) = create_room(&f, &bob, agent_b.as_str(), "r-k2").await;

    // alice 的 key 指向 bob 的 agent：room_b 的配对是 (bob, agent_b)，跨 pair 必须拒绝。
    let cross = sqlx::query(
        "INSERT INTO direct_room_idempotency (human_id, key, agent_id, room_id, created_at) \
         VALUES (?, 'k-cross', ?, ?, 30)",
    )
    .bind(alice.participant.id.as_str())
    .bind(agent_b.as_str())
    .bind(room_b.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        cross.is_err(),
        "composite FK must reject idempotency record whose pair does not match the room"
    );

    // 合法记录仍然可插入：alice 的 key 指向 (alice, agent_a, room_a)。
    let legal = sqlx::query(
        "INSERT INTO direct_room_idempotency (human_id, key, agent_id, room_id, created_at) \
         VALUES (?, 'k-legal', ?, ?, 30)",
    )
    .bind(alice.participant.id.as_str())
    .bind(agent_a.as_str())
    .bind(room_a.as_str())
    .execute(f.store.pool())
    .await;
    assert!(legal.is_ok(), "legal same-pair record must stay legal");

    let illegal = sqlx::query(
        "INSERT INTO direct_room_idempotency (human_id, key, agent_id, room_id, created_at) \
         VALUES (?, 'k-bad-room', ?, ?, 30)",
    )
    .bind(alice.participant.id.as_str())
    .bind(agent_a.as_str())
    .bind(room_b.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        illegal.is_err(),
        "record for pair (alice, agent_a) must not reference room_b"
    );

    assert_eq!(
        count_room_idempotency(&f).await,
        3,
        "room creates + legal record"
    );
}

// （批次二删除）`db_witnesses_idempotency_title_and_rejects_rewrite` 的主语是
// 「幂等记录的标题必须等于 rooms.name」——标题已退出身份，该见证不存在了。
// 它仍有价值的那半（幂等记录**不可改写**、主键 (human_id, key) 身份守卫）由
// `identity_guard_tests::direct_room_facts_cannot_be_replaced` 覆盖；
// 身份见证现在是那条复合外键（room + human + agent 指向真实配对），由 schema 承担。

#[tokio::test]
async fn db_rejects_rewriting_created_at_and_idempotency_facts() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let (room_id, _) = create_room(&f, &alice.participant, agent_id.as_str(), "r-k1").await;

    let (room_created,): (i64,) = sqlx::query_as("SELECT created_at FROM rooms WHERE id = ?")
        .bind(room_id.as_str())
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    let (pair_created,): (i64,) =
        sqlx::query_as("SELECT created_at FROM direct_room_pairs WHERE room_id = ?")
            .bind(room_id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    let (m_created,): (i64,) = sqlx::query_as(
        "SELECT joined_at FROM room_memberships WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(alice.participant.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    let (idem_created,): (i64,) =
        sqlx::query_as("SELECT created_at FROM direct_room_idempotency WHERE key = 'r-k1'")
            .fetch_one(f.store.pool())
            .await
            .unwrap();

    let rewrite_room = sqlx::query("UPDATE rooms SET created_at = ? WHERE id = ?")
        .bind(room_created + 1)
        .bind(room_id.as_str())
        .execute(f.store.pool())
        .await;
    assert!(rewrite_room.is_err(), "rooms.created_at must be immutable");

    let rewrite_pair = sqlx::query("UPDATE direct_room_pairs SET created_at = ? WHERE room_id = ?")
        .bind(pair_created + 1)
        .bind(room_id.as_str())
        .execute(f.store.pool())
        .await;
    assert!(
        rewrite_pair.is_err(),
        "direct_room_pairs.created_at must be immutable"
    );

    let rewrite_membership = sqlx::query(
        "UPDATE room_memberships SET joined_at = ? WHERE room_id = ? AND participant_id = ?",
    )
    .bind(m_created + 1)
    .bind(room_id.as_str())
    .bind(alice.participant.id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        rewrite_membership.is_err(),
        "room_memberships.joined_at must be immutable"
    );

    let rewrite_key =
        sqlx::query("UPDATE direct_room_idempotency SET key = 'rewritten' WHERE key = 'r-k1'")
            .execute(f.store.pool())
            .await;
    assert!(
        rewrite_key.is_err(),
        "direct_room_idempotency.key must be immutable"
    );

    let rewrite_idem_created =
        sqlx::query("UPDATE direct_room_idempotency SET created_at = ? WHERE key = 'r-k1'")
            .bind(idem_created + 1)
            .execute(f.store.pool())
            .await;
    assert!(
        rewrite_idem_created.is_err(),
        "direct_room_idempotency.created_at must be immutable"
    );

    let (room_created_after,): (i64,) = sqlx::query_as("SELECT created_at FROM rooms WHERE id = ?")
        .bind(room_id.as_str())
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(room_created_after, room_created);
    let (pair_created_after,): (i64,) =
        sqlx::query_as("SELECT created_at FROM direct_room_pairs WHERE room_id = ?")
            .bind(room_id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(pair_created_after, pair_created);
    let (m_created_after,): (i64,) = sqlx::query_as(
        "SELECT joined_at FROM room_memberships WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(alice.participant.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(m_created_after, m_created);
    let (idem_created_after,): (i64,) =
        sqlx::query_as("SELECT created_at FROM direct_room_idempotency WHERE key = 'r-k1'")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(idem_created_after, idem_created);

    agent::revoke_agent(&f.agent, &alice.participant.id, agent_id.as_str())
        .await
        .expect("revoke must keep propagating");
    let (m_active, m_left_at): (i64, Option<i64>) = sqlx::query_as(
        "SELECT active_marker, left_at FROM room_memberships \
         WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(agent_id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(m_active, 0);
    assert!(m_left_at.is_some(), "revoke propagation must set left_at");

    let replay = room_application::create_direct_room(
        &f.room,
        &alice.participant,
        agent_id.as_str(),
        "r-k1",
    )
    .await
    .expect("idempotent replay must still work");
    assert!(matches!(
        replay,
        room_application::CreateDirectRoomOutcome::Replayed { .. }
    ));
    assert_eq!(count_rooms(&f).await, 1);
    assert_eq!(count_memberships(&f).await, 2);
    assert_eq!(count_room_idempotency(&f).await, 1);
}

#[tokio::test]
async fn db_rejects_revoked_membership_restore_and_rewrite() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let (room_id, _) = create_room(&f, &alice.participant, agent_id.as_str(), "r-k1").await;

    agent::revoke_agent(&f.agent, &alice.participant.id, agent_id.as_str())
        .await
        .expect("revoke");

    let (_, left_at): (i64, Option<i64>) = sqlx::query_as(
        "SELECT active_marker, left_at FROM room_memberships WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(agent_id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    let original = left_at.expect("closed membership must carry left_at");

    let restore = sqlx::query(
        "UPDATE room_memberships \
         SET active_marker = 1, left_at = NULL, left_before_sequence = NULL, closed_by = NULL \
         WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(agent_id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(restore.is_err(), "closed membership must not be restored");

    let rewrite = sqlx::query(
        "UPDATE room_memberships SET left_at = ? WHERE room_id = ? AND participant_id = ?",
    )
    .bind(original + 1)
    .bind(room_id.as_str())
    .bind(agent_id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        rewrite.is_err(),
        "left_at of a closed membership must not be rewritten"
    );

    let (_, left_at_after): (i64, Option<i64>) = sqlx::query_as(
        "SELECT active_marker, left_at FROM room_memberships WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(agent_id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(left_at_after, Some(original));
}

#[tokio::test]
async fn db_guards_reject_pair_for_revoked_agent_or_missing_owner() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let agent_id = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let (room_id, _) = create_room(&f, &alice.participant, agent_id.as_str(), "r-k1").await;

    agent::revoke_agent(&f.agent, &alice.participant.id, agent_id.as_str())
        .await
        .expect("revoke");

    let revoked_pair = sqlx::query(
        "INSERT INTO direct_room_pairs (room_id, human_id, agent_id, created_at) \
         VALUES ('revoked-pair', ?, ?, 40)",
    )
    .bind(alice.participant.id.as_str())
    .bind(agent_id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        revoked_pair.is_err(),
        "pair insert must require active owner (revoked here)"
    );

    let no_owner = sqlx::query(
        "INSERT INTO direct_room_pairs (room_id, human_id, agent_id, created_at) \
         VALUES ('no-owner', ?, ?, 40)",
    )
    .bind(bob.id.as_str())
    .bind(agent_id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        no_owner.is_err(),
        "pair insert must require an existing owner"
    );

    let wrong_direction = sqlx::query(
        "INSERT INTO direct_room_pairs (room_id, human_id, agent_id, created_at) \
         VALUES ('wrong-dir', ?, ?, 40)",
    )
    .bind(agent_id.as_str())
    .bind(bob.id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        wrong_direction.is_err(),
        "pair direction must be human -> agent"
    );

    assert_eq!(count_pairs(&f).await, 1);
    let _ = room_id;
}

#[tokio::test]
async fn schema_contains_only_expected_tables_and_no_deferred_objects() {
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
            .filter(|word| !word.is_empty())
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
async fn upgrade_from_v3_preserves_data_and_rooms_work_after() {
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
        sqlx::query("BEGIN").execute(&mut *conn).await.unwrap();
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
        sqlx::raw_sql(include_str!(
            "../../../migrations/202608110003_agent_identity.sql"
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
        for version in [202608110001i64, 202608110002, 202608110003] {
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
        sqlx::query("COMMIT").execute(&mut *conn).await.unwrap();
    }

    let identity_v3 = SqliteIdentityStore::new(pool.clone());
    let outcome = bootstrap_human(&identity_v3, "Alice")
        .await
        .expect("bootstrap on v3 schema");
    let agent_v3 = SqliteAgentStore::new(pool.clone());
    let created = agent::create_agent(&agent_v3, &outcome.participant, "升级前", "a-k")
        .await
        .expect("create agent on v3");
    let agent::CreateAgentOutcome::Created {
        agent: agent_v3_created,
        ..
    } = created
    else {
        panic!("must be Created");
    };

    let store = MigrationStore::open(&db_path)
        .await
        .expect("upgrade must apply v4");
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
            .bind(agent_v3_created.id.as_str())
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(kind, "agent", "existing agent must survive the upgrade");
    assert_eq!(status_row, "active");

    let room = SqliteRoomStore::new(store.pool().clone());
    let created_room = room_application::create_direct_room(
        &room,
        &outcome.participant,
        agent_v3_created.id.as_str(),
        "post-upgrade-key",
    )
    .await
    .expect("direct room creation after upgrade");
    assert!(matches!(
        created_room,
        room_application::CreateDirectRoomOutcome::Created { .. }
    ));

    // 升级后的库同样必须执行 rooms → room_coverage 的 deferred 闭环：单独插入
    // rooms 行不能提交。
    {
        let mut conn = store.pool().acquire().await.unwrap();
        sqlx::query("BEGIN").execute(&mut *conn).await.unwrap();
        sqlx::query(
            "INSERT INTO rooms (id, kind, status, created_at) \
             VALUES ('lone-upgrade', 'direct', 'active', 1)",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        let commit = sqlx::query("COMMIT").execute(&mut *conn).await;
        assert!(
            commit.is_err(),
            "lone rooms row must not commit on the upgraded database"
        );
        let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
    }
}

#[tokio::test]
async fn store_rejects_invalid_idempotency_keys_at_db_level() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let (room_id, _) = create_room(&f, &alice.participant, agent_id.as_str(), "r-k1").await;

    for key in ["", &"a".repeat(129), "a,b", "a b", " a", "a\u{0}b"] {
        let result = sqlx::query(
            "INSERT INTO direct_room_idempotency (human_id, key, agent_id, room_id, created_at) \
             VALUES (?, ?, ?, ?, 0)",
        )
        .bind(alice.participant.id.as_str())
        .bind(key)
        .bind(agent_id.as_str())
        .bind(room_id.as_str())
        .execute(f.store.pool())
        .await;
        assert!(
            result.is_err(),
            "expected DB rejection for idempotency key {key:?}"
        );
    }
    assert_eq!(count_room_idempotency(&f).await, 1);
}

#[tokio::test]
async fn store_outcome_matches_application_semantics() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let created_at = UtcMillis::now();
    let room_id = RoomId::new_v4();
    let key = IdempotencyKey::parse("store-key").unwrap();
    let outcome = f
        .room
        .create_direct_room(
            &alice.participant.id,
            &NewDirectRoom {
                room_id: room_id.clone(),
                human_id: alice.participant.id.clone(),
                agent_id: agent_id.clone(),
                human_membership_id: MembershipId::new_v4(),
                agent_membership_id: MembershipId::new_v4(),
                created_at,
            },
            &key,
        )
        .await
        .expect("store create");
    let CreateDirectRoomOutcome::Created { room, memberships } = outcome else {
        panic!("must be Created");
    };
    assert_eq!(room.id, room_id);
    assert_eq!(memberships.len(), 2);

    let replay = f
        .room
        .create_direct_room(
            &alice.participant.id,
            &NewDirectRoom {
                room_id: RoomId::new_v4(),
                human_id: alice.participant.id.clone(),
                agent_id: agent_id.clone(),
                human_membership_id: MembershipId::new_v4(),
                agent_membership_id: MembershipId::new_v4(),
                created_at,
            },
            &key,
        )
        .await
        .expect("store replay");
    let CreateDirectRoomOutcome::Replayed { room, .. } = replay else {
        panic!("must be Replayed");
    };
    assert_eq!(
        room.id, room_id,
        "replay returns the stored room, not the new id"
    );
    assert_eq!(count_rooms(&f).await, 1);
}

#[tokio::test]
async fn cancelled_create_direct_room_rolls_back_before_connection_reuse() {
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
    let cancel_room = SqliteRoomStore::new(cancel_pool.clone());
    let next_room = cancel_room.clone();
    let alice = bootstrap_human(&cancel_identity, "Alice")
        .await
        .unwrap()
        .participant;
    let alice_next = alice.clone();
    let created = agent::create_agent(&cancel_agent, &alice, "甲", "cancel-a-k")
        .await
        .expect("seed agent");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("seed agent create must be Created");
    };
    let agent_id = agent.id.as_str().to_owned();

    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
    let (released_tx, released_rx) = tokio::sync::oneshot::channel();
    cancel_room.arm_commit_checkpoint(reached_tx, released_rx);

    // create_direct_room 写事务在「Room + 覆盖行 + 配对 + 两份 Membership +
    // 幂等记录已写入、commit 前」挂起，等待测试 abort。
    let task_agent_id = agent_id.clone();
    let handle = tokio::spawn(async move {
        room_application::create_direct_room(
            &cancel_room,
            &alice,
            &task_agent_id,
            "cancel-room-key",
        )
        .await
    });

    tokio::time::timeout(std::time::Duration::from_secs(10), reached_rx)
        .await
        .expect("room create must reach the commit checkpoint in time")
        .expect("room create must not fail before the checkpoint");
    handle.abort();
    let join_error = handle.await.expect_err("task must be cancelled");
    assert!(join_error.is_cancelled());
    drop(released_tx);

    for (label, sql) in [
        ("rooms", "SELECT COUNT(*) FROM rooms"),
        ("pairs", "SELECT COUNT(*) FROM direct_room_pairs"),
        ("coverage", "SELECT COUNT(*) FROM room_coverage"),
        ("memberships", "SELECT COUNT(*) FROM room_memberships"),
        (
            "idempotency",
            "SELECT COUNT(*) FROM direct_room_idempotency",
        ),
    ] {
        let count: i64 = sqlx::query_scalar(sql)
            .fetch_one(&cancel_pool)
            .await
            .expect("same pooled connection must recover after cancellation");
        assert_eq!(count, 0, "cancelled room create must leave no {label}");
    }

    let outcome =
        room_application::create_direct_room(&next_room, &alice_next, &agent_id, "cancel-room-key")
            .await
            .expect("next room create on the same single connection must succeed");
    let room_application::CreateDirectRoomOutcome::Created { .. } = outcome else {
        panic!("room create after cancellation must be Created, not Replayed");
    };
    assert_eq!(count_rooms(&f).await, 1);
    assert_eq!(count_pairs(&f).await, 1);
    assert_eq!(count_memberships(&f).await, 2);
}

/// 群邀请幂等验收：invite 必带 Idempotency-Key 且先查 key 再查权限；
/// 同 key 重试即使周期后来关闭也只重放历史结果，绝不误开新周期；历史
/// closed 周期用新 key 重入；不同 key 并发同目标最终只有一个 active 周期，
/// 后续请求各自记录到该 active 周期并返回 AlreadyActive（200）。
#[tokio::test]
async fn group_invite_idempotency_replays_closed_periods_and_rejoins_with_new_key() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let charlie = add_human(f.store.pool(), &f.identity, "Charlie").await;
    accept_contact(&f, &alice, &bob).await;
    accept_contact(&f, &alice, &charlie).await;
    let group_id = create_group(&f, &alice, "群", "g-k").await;

    // 新 key 首次邀请 → Created。
    let outcome = room_application::invite_group_member(
        &f.room,
        &alice,
        group_id.as_str(),
        bob.id.as_str(),
        "inv-b-1",
    )
    .await
    .expect("invite bob");
    let room_application::InviteMemberOutcome::Created { membership: first } = outcome else {
        panic!("first invite must be Created");
    };

    // 同 key 重放 → Replayed 同一周期，不重开。
    let outcome = room_application::invite_group_member(
        &f.room,
        &alice,
        group_id.as_str(),
        bob.id.as_str(),
        "inv-b-1",
    )
    .await
    .expect("replay invite");
    let room_application::InviteMemberOutcome::Replayed { membership } = outcome else {
        panic!("same key must replay");
    };
    assert_eq!(membership.membership_id, first.membership_id);

    // 新 key 但目标已有 active 周期 → AlreadyActive（同一周期），key 被记录。
    let outcome = room_application::invite_group_member(
        &f.room,
        &alice,
        group_id.as_str(),
        bob.id.as_str(),
        "inv-b-2",
    )
    .await
    .expect("new key on active member");
    let room_application::InviteMemberOutcome::AlreadyActive { membership } = outcome else {
        panic!("new key on active member must be AlreadyActive");
    };
    assert_eq!(membership.membership_id, first.membership_id);
    let outcome = room_application::invite_group_member(
        &f.room,
        &alice,
        group_id.as_str(),
        bob.id.as_str(),
        "inv-b-3",
    )
    .await
    .expect("another new key on active member");
    let room_application::InviteMemberOutcome::AlreadyActive { membership } = outcome else {
        panic!("second new key must also be AlreadyActive");
    };
    assert_eq!(membership.membership_id, first.membership_id);
    // 最终只有一个 active 周期。
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(
        active, 1,
        "different keys must converge to one active period"
    );
    let invite_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM group_member_invite_idempotency WHERE human_id = ?",
    )
    .bind(alice.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(invite_rows, 3, "each successful key is recorded");

    // 同 key 换目标 → IdempotencyConflict。
    let err = room_application::invite_group_member(
        &f.room,
        &alice,
        group_id.as_str(),
        charlie.id.as_str(),
        "inv-b-1",
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            err,
            room_application::InviteMemberError::IdempotencyConflict
        ),
        "same key different target must conflict, got {err}"
    );

    // 移除 bob：同 key 重放仍返回历史 closed 周期（removed），不重开；
    // 新 key 重入 → Created 新周期。
    room_application::remove_group_member(&f.room, &alice, group_id.as_str(), bob.id.as_str())
        .await
        .expect("remove bob");
    let outcome = room_application::invite_group_member(
        &f.room,
        &alice,
        group_id.as_str(),
        bob.id.as_str(),
        "inv-b-1",
    )
    .await
    .expect("old key replay after close");
    let room_application::InviteMemberOutcome::Replayed { membership } = outcome else {
        panic!("old key after close must replay the historical period");
    };
    assert_eq!(membership.membership_id, first.membership_id);
    assert_eq!(
        membership.closed_by,
        Some(MembershipClosedBy::Removed),
        "replay reflects the historical closed period"
    );
    let outcome = room_application::invite_group_member(
        &f.room,
        &alice,
        group_id.as_str(),
        bob.id.as_str(),
        "inv-b-rejoin",
    )
    .await
    .expect("rejoin with new key");
    let room_application::InviteMemberOutcome::Created {
        membership: rejoined,
    } = outcome
    else {
        panic!("rejoin must be Created");
    };
    assert_ne!(rejoined.membership_id, first.membership_id, "new period id");
    let active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(active, 1, "rejoin opens exactly one new active period");
    let periods: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships WHERE room_id = ? AND participant_id = ?",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(
        periods, 2,
        "closed history period preserved alongside the active one"
    );
}

/// accepted 联系人 fixture（alice ↔ human）。
async fn accept_contact(f: &Fixture, alice: &Participant, human: &Participant) {
    let (a, b) = crate::domain::contact::canonical_pair(&alice.id, &human.id);
    sqlx::query(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at, idempotency_key) \
         VALUES (?, ?, ?, ?, ?, 'accepted', 1, 1, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(a.as_str())
    .bind(b.as_str())
    .bind(a.as_str())
    .bind(b.as_str())
    .bind(format!("k-{}", human.id.as_str()))
    .execute(f.store.pool())
    .await
    .expect("insert accepted contact fixture");
}

/// 通过公开用例创建 group 并邀请 bob，返回 group_id。
async fn create_group_with_member(
    f: &Fixture,
    owner: &Participant,
    member: &Participant,
    name: &str,
    gkey: &str,
    ikey: &str,
) -> RoomId {
    let group_id = create_group(f, owner, name, gkey).await;
    room_application::invite_group_member(
        &f.room,
        owner,
        group_id.as_str(),
        member.id.as_str(),
        ikey,
    )
    .await
    .expect("invite member");
    group_id
}

/// 主动退群验收：成员 leave 关闭自己的周期，离群后无房间访问；重复 leave
/// 404；owner leave 返回 OwnerTransferRequired。
#[tokio::test]
async fn member_leave_closes_period_and_owner_leave_requires_transfer() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    accept_contact(&f, &alice, &bob).await;
    let group_id = create_group_with_member(&f, &alice, &bob, "群", "g-k", "inv-b-1").await;

    // bob 主动退群 → Ok；再退 → NotFound；Room 读写 404。
    room_application::leave_membership(&f.room, &bob.id, group_id.as_str())
        .await
        .expect("bob leaves");
    let err = room_application::leave_membership(&f.room, &bob.id, group_id.as_str())
        .await
        .unwrap_err();
    assert!(
        matches!(err, room_application::LeaveMembershipError::RoomNotFound),
        "second leave must be NotFound, got {err}"
    );
    assert!(
        crate::ports::room::RoomStore::get_room(&f.room, &bob.id, &group_id)
            .await
            .expect("get")
            .is_none(),
        "left member must lose room access"
    );
    let (active, left_before, closed_by): (i64, Option<i64>, Option<String>) = sqlx::query_as(
        "SELECT active_marker, left_before_sequence, closed_by FROM room_memberships \
         WHERE room_id = ? AND participant_id = ?",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(active, 0);
    assert_eq!(closed_by.as_deref(), Some("leave"));
    assert!(left_before.is_some(), "leave must fix left_before_sequence");

    // owner leave → OwnerTransferRequired。
    let err = room_application::leave_membership(&f.room, &alice.id, group_id.as_str())
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            room_application::LeaveMembershipError::OwnerTransferRequired
        ),
        "owner leave must require transfer, got {err}"
    );

    // Human 在 direct 房：他**恒为 owner**（`direct_room_pairs.role_owner` 恒
    // 'owner'）⇒ 走**同一条 owner 规则**（先转让），不再是「非 group ⇒ 404」。
    //
    // ⚠ 这是 M2 切片（2026-09-27）**显式声明的可观察变化**：Human 依然退不了
    // direct 房，但理由码从 404 变成 409——同一处规则的自然结果，不再是房 kind
    // 特判。放弃那个 404 是为了不把 kind 判据写进领域层。
    let agent_id = create_agent(&f, &alice, "甲", "a-k").await;
    let (direct_id, _) = create_room(&f, &alice, agent_id.as_str(), "r-k").await;
    let err = room_application::leave_membership(&f.room, &alice.id, direct_id.as_str())
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            room_application::LeaveMembershipError::OwnerTransferRequired
        ),
        "Human 在 direct 房恒为 owner ⇒ 先转让，got {err}"
    );

    // **Agent 在 direct 房是 member ⇒ 可以退**（M2 新增的能力）：周期关闭、
    // 理由是 leave，配对所指向的周期仍然在（房与历史都留着，Human 照旧可读）。
    let closed = room_application::leave_membership(&f.room, &agent_id, direct_id.as_str())
        .await
        .expect("agent leaves its direct room");
    let (active, closed_by): (i64, Option<String>) = sqlx::query_as(
        "SELECT active_marker, closed_by FROM room_memberships WHERE membership_id = ?",
    )
    .bind(closed.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(active, 0);
    assert_eq!(closed_by.as_deref(), Some("leave"));
    assert_eq!(
        count_pairs(&f).await,
        1,
        "配对行不删：房还在，只是这位成员不在里面了"
    );
    let (human_active, agent_active): (i64, i64) = sqlx::query_as(
        "SELECT \
           (SELECT COUNT(*) FROM room_memberships WHERE room_id = ? AND participant_id = ? \
              AND active_marker = 1), \
           (SELECT COUNT(*) FROM room_memberships WHERE room_id = ? AND participant_id = ? \
              AND active_marker = 1)",
    )
    .bind(direct_id.as_str())
    .bind(alice.id.as_str())
    .bind(direct_id.as_str())
    .bind(agent_id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!((human_active, agent_active), (1, 0));

    // 未知 Room → NotFound。
    let err = room_application::leave_membership(&f.room, &bob.id, &RoomId::new_v4().to_string())
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        room_application::LeaveMembershipError::RoomNotFound
    ));
}

/// 群主转让验收：BEGIN IMMEDIATE 事务内读取一次 X=当前最大 sequence；
/// 关闭旧 owner 与目标 member 周期、打开目标 owner 与旧 owner member
/// 周期、更新 coverage 见证、写幂等记录；任何一步失败整体回滚；同 key
/// 重放返回同一新 owner 周期；转让后旧 owner 成为 member 可退群；历史
/// 窗口不产生空洞。
#[tokio::test]
async fn transfer_swaps_owner_updates_witness_and_replays() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    accept_contact(&f, &alice, &bob).await;
    let group_id = create_group_with_member(&f, &alice, &bob, "群", "g-k", "inv-b-1").await;

    // 事务内产生的最大 sequence 为 X（转让边界读取一次）。
    let x: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM messages WHERE room_id = ?")
            .bind(group_id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(x, 0, "no messages yet, boundary is 0");

    // alice 转让给 bob。
    let outcome = room_application::transfer_group_owner(
        &f.room,
        &alice,
        group_id.as_str(),
        bob.id.as_str(),
        "t-1",
    )
    .await
    .expect("transfer");
    let room_application::TransferOwnerOutcome::Transferred { membership, .. } = outcome else {
        panic!("transfer must be Transferred");
    };
    assert_eq!(membership.participant_id, bob.id);
    assert_eq!(membership.role.as_str(), "owner");
    assert_eq!(membership.join_after_sequence, x, "join boundary is X");

    // coverage 见证切换到 bob 的 active owner 周期。
    let (witness, active, role): (String, i64, String) = sqlx::query_as(
        "SELECT c.group_owner_membership_id, m.active_marker, m.role \
         FROM room_coverage c JOIN room_memberships m ON m.membership_id = c.group_owner_membership_id \
         WHERE c.room_id = ?",
    )
    .bind(group_id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(witness, membership.membership_id.as_str());
    assert_eq!(active, 1);
    assert_eq!(role, "owner");

    // 恰好一个 active Human owner；旧 owner 成为 active member。
    let owners: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships \
         WHERE room_id = ? AND role = 'owner' AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(owners, 1, "exactly one active owner after transfer");
    let (alice_role, alice_active, alice_join_after, alice_left_before): (
        String,
        i64,
        i64,
        Option<i64>,
    ) = sqlx::query_as(
        "SELECT role, active_marker, join_after_sequence, left_before_sequence \
             FROM room_memberships WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(alice.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(alice_role, "member");
    assert_eq!(alice_active, 1);
    assert_eq!(
        alice_join_after, x,
        "old owner rejoins at the same boundary X"
    );
    assert_eq!(alice_left_before, None);
    let (bob_old_left_before,): (Option<i64>,) = sqlx::query_as(
        "SELECT left_before_sequence FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 0 AND closed_by = 'transfer'",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(
        bob_old_left_before,
        Some(x),
        "closed member period fixes left_before_sequence = X"
    );

    // 同 key 重放 → Replayed 同一新 owner 周期。
    let outcome = room_application::transfer_group_owner(
        &f.room,
        &alice,
        group_id.as_str(),
        bob.id.as_str(),
        "t-1",
    )
    .await
    .expect("transfer replay");
    let room_application::TransferOwnerOutcome::Replayed {
        membership: replayed,
    } = outcome
    else {
        panic!("same key must replay");
    };
    assert_eq!(replayed.membership_id, membership.membership_id);

    // 转让后旧 owner（alice 现在是 member）可以退群。
    room_application::leave_membership(&f.room, &alice.id, group_id.as_str())
        .await
        .expect("old owner leaves as member");

    // 新 owner 直接 leave → OwnerTransferRequired。
    let err = room_application::leave_membership(&f.room, &bob.id, group_id.as_str())
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            room_application::LeaveMembershipError::OwnerTransferRequired
        ),
        "new owner leave must require transfer"
    );
}

/// 转让目标校验：Agent、自己、非成员返回稳定业务错误（不落 DB 500）。
#[tokio::test]
async fn transfer_rejects_agent_self_and_non_member_targets() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let charlie = add_human(f.store.pool(), &f.identity, "Charlie").await;
    accept_contact(&f, &alice, &bob).await;
    accept_contact(&f, &alice, &charlie).await;
    let agent_id = create_agent(&f, &alice, "甲", "a-k").await;
    let group_id = create_group_with_member(&f, &alice, &bob, "群", "g-k", "inv-b-1").await;
    room_application::invite_group_member(
        &f.room,
        &alice,
        group_id.as_str(),
        agent_id.as_str(),
        "inv-ag-1",
    )
    .await
    .expect("invite agent");

    // 本房**成员**之外的目标（含 Agent）⇒ MemberNotFound（不是本房成员这件事
    // 与参与者 kind 无关；2026-09-27 批次三第一刀）。
    let outsider_agent = create_agent(&f, &alice, "乙", "a-k2").await;
    let err = room_application::transfer_group_owner(
        &f.room,
        &alice,
        group_id.as_str(),
        outsider_agent.as_str(),
        "t-outsider-agent",
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, room_application::TransferOwnerError::MemberNotFound),
        "an Agent outside the room is not a member, got {err}"
    );
    let err = room_application::transfer_group_owner(
        &f.room,
        &alice,
        group_id.as_str(),
        alice.id.as_str(),
        "t-self",
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            err,
            room_application::TransferOwnerError::InvalidTransferTarget
        ),
        "self target must be invalid_transfer_target, got {err}"
    );
    let err = room_application::transfer_group_owner(
        &f.room,
        &alice,
        group_id.as_str(),
        charlie.id.as_str(),
        "t-stranger",
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, room_application::TransferOwnerError::MemberNotFound),
        "non-member target must be member_not_found, got {err}"
    );
    // 非 owner 调用 → RoomNotFound。
    let err = room_application::transfer_group_owner(
        &f.room,
        &bob,
        group_id.as_str(),
        charlie.id.as_str(),
        "t-bob",
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, room_application::TransferOwnerError::RoomNotFound),
        "non-owner actor must be room_not_found, got {err}"
    );
    // 未知 Room / bad-id → RoomNotFound。
    let err = room_application::transfer_group_owner(
        &f.room,
        &alice,
        &RoomId::new_v4().to_string(),
        bob.id.as_str(),
        "t-ghost",
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err,
        room_application::TransferOwnerError::RoomNotFound
    ));

    // 事实不变：仍是一个 active owner。
    let owners: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships \
         WHERE room_id = ? AND role = 'owner' AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(owners, 1);

    // 正例（批次三第一刀）：把 owner 转让给**本房的 Agent 成员**——owner 是房内的
    // 治理角色，不是参与者种类；旧 owner 同事务降级为 member（"入群资格"管的是
    // 入群，不是换角色）。
    let outcome = room_application::transfer_group_owner(
        &f.room,
        &alice,
        group_id.as_str(),
        agent_id.as_str(),
        "t-agent",
    )
    .await
    .expect("an Agent member must be a valid transfer target");
    let room_application::TransferOwnerOutcome::Transferred { membership, .. } = outcome else {
        panic!("transfer to an Agent member must be Transferred");
    };
    assert_eq!(membership.participant_id.as_str(), agent_id.as_str());
    assert_eq!(membership.role, MembershipRole::Owner);
    let active: Vec<(String, String)> = sqlx::query_as(
        "SELECT participant_id, role FROM room_memberships \
         WHERE room_id = ? AND active_marker = 1 ORDER BY role, participant_id",
    )
    .bind(group_id.as_str())
    .fetch_all(f.store.pool())
    .await
    .unwrap();
    // 一个 owner（Agent A）+ 两个 member（Alice 被降级、Bob 原样）。
    assert_eq!(active.len(), 3, "one owner + two members: {active:?}");
    let owner_rows: Vec<&(String, String)> =
        active.iter().filter(|(_, role)| role == "owner").collect();
    assert_eq!(owner_rows.len(), 1, "exactly one owner: {active:?}");
    assert_eq!(
        owner_rows[0].0,
        agent_id.as_str(),
        "the Agent is the owner now: {active:?}"
    );
    let alice_row = active
        .iter()
        .find(|(id, _)| id == alice.id.as_str())
        .expect("the old owner is still a member");
    assert_eq!(alice_row.1, "member", "the Human was demoted: {active:?}");
}

/// 连续转让验收（A→B→A，API）：每次转让恰一 active owner、coverage 正确
/// 见证当前 owner、transition 事实逐条累积；旧 key（B→A 后再重放 A→B 的
/// key）仍返回历史新 owner 周期；两次转让之间发送的消息对所有历史窗口
/// 可见——旧 owner 历次周期窗口连续无洞。
#[tokio::test]
async fn consecutive_transfers_keep_single_owner_and_replay_history() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    accept_contact(&f, &alice, &bob).await;
    let group_id = create_group_with_member(&f, &alice, &bob, "群", "g-k", "inv-b-1").await;
    let message =
        crate::adapters::sqlite::message_store::SqliteMessageStore::new(f.store.pool().clone());

    // A → B（t1，边界 X1=0）。
    let t1 = room_application::transfer_group_owner(
        &f.room,
        &alice,
        group_id.as_str(),
        bob.id.as_str(),
        "t-1",
    )
    .await
    .expect("transfer 1");
    let room_application::TransferOwnerOutcome::Transferred {
        membership: t1_owner,
        ..
    } = t1
    else {
        panic!("t1 must be Transferred");
    };
    let x1 = t1_owner.join_after_sequence;
    assert_eq!(x1, 0, "first transfer boundary is 0");

    // 两次转让之间 alice（现在是 member）发一条消息：X2 前进到 1。
    let outcome = crate::ports::message::MessageStore::send_message(
        &message,
        &crate::ports::message::NewMessage {
            id: crate::domain::message::MessageId::new_v4(),
            room_id: group_id.clone(),
            actor_id: alice.id.clone(),
            body: Some(crate::domain::message::MessageBody::parse("between").expect("body")),
            mentions: vec![],
            attachments: Vec::new(),
            reply_to_message_id: None,
            created_at: UtcMillis::now(),
        },
        &IdempotencyKey::parse("m-between").expect("key"),
    )
    .await
    .expect("message between transfers");

    // B → A（t2，边界 X2=1）。
    let t2 = room_application::transfer_group_owner(
        &f.room,
        &bob,
        group_id.as_str(),
        alice.id.as_str(),
        "t-2",
    )
    .await
    .expect("transfer 2");
    let room_application::TransferOwnerOutcome::Transferred {
        membership: t2_owner,
        ..
    } = t2
    else {
        panic!("t2 must be Transferred");
    };
    let x2 = t2_owner.join_after_sequence;
    assert_eq!(x2, 1, "second transfer boundary advanced to 1");

    // 恰一 active owner，且是 alice 的 t2 周期；coverage 见证该周期。
    let owners: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships \
         WHERE room_id = ? AND role = 'owner' AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(owners, 1, "exactly one active owner after two transfers");
    let (active_owner_participant,): (String,) = sqlx::query_as(
        "SELECT participant_id FROM room_memberships \
         WHERE room_id = ? AND role = 'owner' AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(active_owner_participant, alice.id.as_str());
    let witness: String =
        sqlx::query_scalar("SELECT group_owner_membership_id FROM room_coverage WHERE room_id = ?")
            .bind(group_id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(witness, t2_owner.membership_id.as_str());
    let (witness_active, witness_role): (i64, String) =
        sqlx::query_as("SELECT active_marker, role FROM room_memberships WHERE membership_id = ?")
            .bind(witness.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(witness_active, 1);
    assert_eq!(witness_role, "owner");

    // t1 的新 owner 周期（bob）已被 t2 关闭，但旧 key t-1 仍可重放历史
    // 结果（alice 是 t2 后的当前 owner，仍可用 t-1 发起请求）。
    let replay = room_application::transfer_group_owner(
        &f.room,
        &alice,
        group_id.as_str(),
        bob.id.as_str(),
        "t-1",
    )
    .await
    .expect("old key must replay");
    let room_application::TransferOwnerOutcome::Replayed {
        membership: replayed,
    } = replay
    else {
        panic!("old key must replay");
    };
    assert_eq!(replayed.membership_id, t1_owner.membership_id);
    assert_eq!(replayed.participant_id, bob.id);
    assert_eq!(replayed.role.as_str(), "owner");
    let (closed,): (i64,) =
        sqlx::query_as("SELECT active_marker FROM room_memberships WHERE membership_id = ?")
            .bind(replayed.membership_id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(closed, 0, "replayed period is closed but still replayable");

    // 两条 transition 事实累积，每条精确关联四条周期。
    let transitions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM group_owner_transitions WHERE room_id = ?")
            .bind(group_id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(transitions, 2, "two immutable transitions accumulated");

    // alice 周期链连续无洞（按角色定位，避免同边界歧义）：
    // founding owner [0, X1] → member [X1, X2] → re-owner [X2, ∞)。
    let (founding_left,): (Option<i64>,) = sqlx::query_as(
        "SELECT left_before_sequence FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND role = 'owner' AND active_marker = 0",
    )
    .bind(group_id.as_str())
    .bind(alice.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(founding_left, Some(x1), "founding owner closes at X1");
    let (member_join, member_left): (i64, Option<i64>) = sqlx::query_as(
        "SELECT join_after_sequence, left_before_sequence FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND role = 'member'",
    )
    .bind(group_id.as_str())
    .bind(alice.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(member_join, x1, "alice member joins at X1");
    assert_eq!(member_left, Some(x2), "alice member closes at X2");
    let (reowner_join, reowner_left): (i64, Option<i64>) = sqlx::query_as(
        "SELECT join_after_sequence, left_before_sequence FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND role = 'owner' AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(alice.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(reowner_join, x2, "alice re-owner joins at X2");
    assert_eq!(reowner_left, None, "alice re-owner period is open");

    // bob 周期链连续：member [0, X1] → owner [X1, X2] → member [X2, ∞)。
    let (bob_member1_left,): (Option<i64>,) = sqlx::query_as(
        "SELECT left_before_sequence FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND role = 'member' AND active_marker = 0 \
         ORDER BY left_before_sequence LIMIT 1",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(bob_member1_left, Some(x1));
    let (bob_owner_join, bob_owner_left): (i64, Option<i64>) = sqlx::query_as(
        "SELECT join_after_sequence, left_before_sequence FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND role = 'owner' AND active_marker = 0",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(bob_owner_join, x1, "bob owner joins at X1");
    assert_eq!(bob_owner_left, Some(x2), "bob owner closes at X2");
    let (bob_member2_join,): (i64,) = sqlx::query_as(
        "SELECT join_after_sequence FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND role = 'member' AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(bob_member2_join, x2, "bob member joins at X2");

    // 窗口无洞的事实证据：夹在两次转让之间的消息对 alice（member 窗口）
    // 与 bob（owner 窗口）都可见，且历史页不遗漏。
    let _ = outcome;
    let page_alice = crate::ports::message::MessageStore::list_messages(
        &message, &alice.id, &group_id, 100, None,
    )
    .await
    .expect("alice history")
    .expect("alice is active member");
    let seqs_alice: Vec<i64> = page_alice
        .messages
        .iter()
        .map(|m| m.message.sequence)
        .collect();
    assert_eq!(
        seqs_alice,
        vec![1],
        "alice sees the between-transfer message"
    );
    let page_bob =
        crate::ports::message::MessageStore::list_messages(&message, &bob.id, &group_id, 100, None)
            .await
            .expect("bob history")
            .expect("bob is active member");
    let seqs_bob: Vec<i64> = page_bob
        .messages
        .iter()
        .map(|m| m.message.sequence)
        .collect();
    assert_eq!(seqs_bob, vec![1], "bob sees the between-transfer message");
}

/// **「对方读没读我最后那条」**（契约 `CORE_V1_READ_RECEIPT.md` §2/§6）。
///
/// 判据逐条（这一格是**投影**，不是新事实——零新写者、零新事件）：
///
/// 1. `direct` 房、我说过话、对方还没读到 ⇒ `"unread"`；
/// 2. 对方推进到它 ⇒ `"read"`（**同一格，单调**）；
/// 3. **我又发一条** ⇒ 回到 `"unread"`（"我最后那条"换了）；
/// 4. **我还没说过话**（只有对方在说）⇒ 这一格**不出现**（不是 `"unread"`）；
/// 5. **群房** ⇒ 不出现（这一格只在 `direct` 房回答）；
/// 6. **对方不是这间房的 active 成员了**（退房）⇒ 不出现（读位置无从谈起）。
#[tokio::test]
async fn peer_read_tells_whether_the_other_side_read_my_last_message() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let agent = create_agent(&f, &alice, "小满", "a-k-peer-read").await;
    let (room, _) = create_room(&f, &alice, agent.as_str(), "k-peer-read").await;
    let messages =
        crate::adapters::sqlite::message_store::SqliteMessageStore::new(f.store.pool().clone());

    /// 往这间房发一条（谁发的由 `actor` 定）。
    async fn say(
        messages: &crate::adapters::sqlite::message_store::SqliteMessageStore,
        room: &RoomId,
        actor: ParticipantId,
        body: &str,
        key: &str,
    ) -> crate::ports::message::SendMessageOutcome {
        crate::ports::message::MessageStore::send_message(
            messages,
            &crate::ports::message::NewMessage {
                id: crate::domain::message::MessageId::new_v4(),
                room_id: room.clone(),
                actor_id: actor,
                body: Some(crate::domain::message::MessageBody::parse(body).expect("body")),
                mentions: vec![],
                attachments: Vec::new(),
                reply_to_message_id: None,
                created_at: UtcMillis::now(),
            },
            &IdempotencyKey::parse(key).expect("key"),
        )
        .await
        .expect("send message")
    }

    // ④ 我还没说过话（对方先说）⇒ 这一格不出现（问题不成立，不是"没读"）。
    say(&messages, &room, agent.clone(), "我先说", "m-agent-1").await;
    let list = room_application::list_rooms(&f.room, &alice.id)
        .await
        .expect("list");
    assert_eq!(
        list[0].peer_read, None,
        "我还没说过话 ⇒ 这一格不出现（不是 unread）"
    );

    // ① 我说一条 ⇒ 对方还没读到 ⇒ unread。
    let sent = say(&messages, &room, alice.id.clone(), "你在吗", "m-human-1").await;
    let my_message_id = match sent {
        crate::ports::message::SendMessageOutcome::Created { message } => message.id,
        other => panic!("expected Created, got {other:?}"),
    };
    let list = room_application::list_rooms(&f.room, &alice.id)
        .await
        .expect("list");
    assert_eq!(
        list[0].peer_read,
        Some(PeerRead::Unread),
        "对方还没读到 ⇒ unread"
    );

    // ② 对方推进到它 ⇒ read（同一格，单调）。
    room_application::advance_read_cursor(&f.room, &agent, room.as_str(), my_message_id.as_str())
        .await
        .expect("agent advances its read cursor");
    let list = room_application::list_rooms(&f.room, &alice.id)
        .await
        .expect("list");
    assert_eq!(list[0].peer_read, Some(PeerRead::Read), "对方读到了 ⇒ read");

    // ③ 我又发一条 ⇒ 回到 unread（旧那条仍被读过，但"我最后那条"换了）。
    say(&messages, &room, alice.id.clone(), "在吗？", "m-human-2").await;
    let list = room_application::list_rooms(&f.room, &alice.id)
        .await
        .expect("list");
    assert_eq!(
        list[0].peer_read,
        Some(PeerRead::Unread),
        "我最后那条换了 ⇒ 回到 unread"
    );

    // ⑤ 群房 ⇒ 这一格不出现（这一格只在 direct 房回答）。
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    accept_contact(&f, &alice, &bob).await;
    let group_id = create_group_with_member(&f, &alice, &bob, "群", "g-peer", "inv-peer").await;
    let group = room_application::list_rooms(&f.room, &alice.id)
        .await
        .expect("list");
    let group_row = group
        .iter()
        .find(|item| item.room.id == group_id)
        .expect("group row");
    assert_eq!(
        group_row.peer_read, None,
        "群房不回答这个问题 ⇒ 这一格不出现"
    );

    // ⑥ 对方退房（不再是 active 成员）⇒ 这一格不出现。
    room_application::leave_membership(&f.room, &agent, room.as_str())
        .await
        .expect("agent leaves");
    let list = room_application::list_rooms(&f.room, &alice.id)
        .await
        .expect("list");
    let direct_row = list
        .iter()
        .find(|item| item.room.id == room)
        .expect("direct row");
    assert_eq!(
        direct_row.peer_read, None,
        "对方不是 active 成员 ⇒ 这一格不出现"
    );

    // ⑦ **全新一间房，第一条就是我说的话** ⇒ 对方没读过 ⇒ `unread`
    //    （不是"问题不成立"、更不是 `read`——2026-10-10 真机探针在这一格上没读到东西，
    //    这条用例把那个形状钉在服务端：房刚建、消息是第一条、对方从没醒过）。
    let agent_two = create_agent(&f, &alice, "小满二号", "a-k-peer-read-2").await;
    let (fresh_room, _) = create_room(&f, &alice, agent_two.as_str(), "k-peer-read-2").await;
    say(
        &messages,
        &fresh_room,
        alice.id.clone(),
        "第一条",
        "m-human-fresh",
    )
    .await;
    let list = room_application::list_rooms(&f.room, &alice.id)
        .await
        .expect("list");
    let fresh_row = list
        .iter()
        .find(|item| item.room.id == fresh_room)
        .expect("fresh direct row");
    assert_eq!(
        fresh_row.peer_read,
        Some(PeerRead::Unread),
        "新房第一条就是我说的话 ⇒ 对方没读过 ⇒ unread"
    );
}
/// **提醒档的唤醒目标**（契约 `CORE_V1_IM_CONTRACT.md` §9.1 的「通知条件」，
/// 窄解冻切片 1b）：只列该房 **active Agent 成员**里档为 `all` 的那些——
/// **缺行也算 `all`**（不预建行）；`mentions` 档的**不在**；Human 永不在
/// （Human 侧的提醒不经过服务端 gate）；非成员不在。纯读：调用前后零写、
/// 零事件、零唤醒。
#[tokio::test]
async fn attention_wake_targets_default_to_all_and_skip_mentions() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let agent_a = create_agent(&f, &alice, "Agent A", "attn-a").await;
    let agent_b = create_agent(&f, &alice, "Agent B", "attn-b").await;
    let agent_c = create_agent(&f, &alice, "Agent C", "attn-c").await;

    let group = create_group(&f, &alice, "提醒档群", "attn-g").await;
    for (n, agent) in [("1", &agent_a), ("2", &agent_b)] {
        room_application::invite_group_member(
            &f.room,
            &alice,
            group.as_str(),
            agent.as_str(),
            &format!("attn-inv-{n}"),
        )
        .await
        .expect("invite agent");
    }

    let mut targets = f
        .room
        .attention_wake_targets(&group)
        .await
        .expect("wake targets");
    targets.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    // 缺行 = all ⇒ 两位 Agent 都在；Human owner 与未入群的 Agent C 都不在。
    let mut expected = vec![agent_a.clone(), agent_b.clone()];
    expected.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    assert_eq!(
        targets, expected,
        "缺行必须投影成 all，且只列 active Agent 成员"
    );
    assert!(
        !targets.contains(&agent_c),
        "没入群的那支 Agent 不在唤醒目标里"
    );
    assert!(
        !targets.contains(&alice.id),
        "Human 不在 Wake 目标里（Human 侧的提醒不经过服务端 gate）"
    );

    // A 自己写 mentions ⇒ 它不在唤醒目标里，B 仍在。
    f.room
        .set_room_attention(&agent_a, &group, AttentionLevel::Mentions)
        .await
        .expect("agent A mutes itself");
    let targets = f
        .room
        .attention_wake_targets(&group)
        .await
        .expect("wake targets");
    assert_eq!(
        targets,
        vec![agent_b.clone()],
        "档 = mentions 的 Agent 不该被那条易失通知叫醒"
    );

    // A 写回 all ⇒ 又回到两位。
    f.room
        .set_room_attention(&agent_a, &group, AttentionLevel::All)
        .await
        .expect("agent A unmutes itself");
    let targets = f
        .room
        .attention_wake_targets(&group)
        .await
        .expect("wake targets");
    assert_eq!(targets.len(), 2);

    // 纯读：本方法一行都没写过（两条档行是上面显式写的，Agent C 始终没有行）。
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM room_attention")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(rows, 1, "只有 A 那一行；B 缺行仍然算 all");
}
