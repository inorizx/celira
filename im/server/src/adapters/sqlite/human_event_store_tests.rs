//! SQLite Human Event adapter 测试：真实临时数据库上的两层事实（shared
//! fact + per-Human delivery）原子生成、五类事件与 targeting 矩阵、每
//! Human sequence 从 1 单调唯一、共享快照非 N 份、发生时快照语义、读取时
//! Membership 授权过滤（唯一例外是本人 member.left）、post-commit 唤醒
//! 目标反查、数据库反向写入拒绝矩阵与 schema 精确集合。

use sqlx::{Connection, SqlitePool};
use uuid::Uuid;

use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::human_event_store::SqliteHumanEventStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::message_store::SqliteMessageStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::room_store::SqliteRoomStore;
use crate::application::agent;
use crate::application::identity::bootstrap_human;
use crate::application::room as room_application;
use crate::domain::agent::IdempotencyKey;
use crate::domain::human_event::{HumanEventKind, HumanEventSource};
use crate::domain::human_session::{HumanSession, SessionId, TokenHash};
use crate::domain::message::{MessageBody, MessageId};
use crate::domain::participant::{DisplayName, Participant, ParticipantId};
use crate::domain::room::{MembershipId, RoomId};
use crate::domain::time::UtcMillis;
use crate::domain::token::OpaqueToken;
use crate::ports::human_event::{HumanEventStore as _, HumanEventWithFact};
use crate::ports::identity::{IdentityStore as _, NewSession};
use crate::ports::message::{MessageStore as _, NewMessage, SendMessageOutcome};
use crate::ports::migration::MigrationProbe;

struct Fixture {
    _dir: tempfile::TempDir,
    store: MigrationStore,
    identity: SqliteIdentityStore,
    agent: SqliteAgentStore,
    room: SqliteRoomStore,
    message: SqliteMessageStore,
    human_events: SqliteHumanEventStore,
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
        message: SqliteMessageStore::new(pool.clone()),
        human_events: SqliteHumanEventStore::new(pool.clone()),
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

async fn create_agent(f: &Fixture, human: &Participant, name: &str, key: &str) -> ParticipantId {
    let created = agent::create_agent(&f.agent, human, name, key)
        .await
        .expect("create agent");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("agent create must be Created");
    };
    agent.id
}

async fn create_direct_room(
    f: &Fixture,
    human: &Participant,
    agent_id: &ParticipantId,
    key: &str,
) -> (RoomId, Vec<crate::domain::room::Membership>) {
    let outcome = room_application::create_direct_room(&f.room, human, agent_id.as_str(), key)
        .await
        .expect("create direct room");
    let room_application::CreateDirectRoomOutcome::Created { room, memberships } = outcome else {
        panic!("first room create must be Created");
    };
    (room.id, memberships)
}

async fn create_group(f: &Fixture, owner: &Participant, name: &str, key: &str) -> RoomId {
    let outcome = room_application::create_group(&f.room, owner, name, key)
        .await
        .expect("create group");
    let room_application::CreateRoomOutcome::Created { room, .. } = outcome else {
        panic!("first group create must be Created");
    };
    room.id
}

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

/// 带 reply target 提交的发送辅助（CORE_V1_REPLIES_AND_QUOTES.md）。
async fn send_reply(
    store: &SqliteMessageStore,
    actor_id: &ParticipantId,
    room_id: &RoomId,
    body: &str,
    reply_to: &MessageId,
    key: &str,
) -> Result<SendMessageOutcome, crate::ports::message::MessageStoreError> {
    store
        .send_message(
            &NewMessage {
                id: MessageId::new_v4(),
                room_id: room_id.clone(),
                actor_id: actor_id.clone(),
                body: Some(MessageBody::parse(body).expect("test body valid")),
                mentions: Vec::new(),
                attachments: Vec::new(),
                reply_to_message_id: Some(reply_to.clone()),
                created_at: UtcMillis::now(),
            },
            &IdempotencyKey::parse(key).expect("test key valid"),
        )
        .await
}

async fn invite(
    f: &Fixture,
    owner: &Participant,
    group_id: &str,
    member: &ParticipantId,
    key: &str,
) -> crate::domain::room::Membership {
    let outcome =
        room_application::invite_group_member(&f.room, owner, group_id, member.as_str(), key)
            .await
            .expect("invite member");
    let room_application::InviteMemberOutcome::Created { membership } = outcome else {
        panic!("invite must be Created");
    };
    membership
}

async fn send(
    store: &SqliteMessageStore,
    actor_id: &ParticipantId,
    room_id: &RoomId,
    body: &str,
    key: &str,
) -> SendMessageOutcome {
    store
        .send_message(
            &NewMessage {
                id: MessageId::new_v4(),
                room_id: room_id.clone(),
                actor_id: actor_id.clone(),
                body: Some(MessageBody::parse(body).expect("test body valid")),
                mentions: Vec::new(),
                attachments: Vec::new(),
                reply_to_message_id: None,
                created_at: UtcMillis::now(),
            },
            &IdempotencyKey::parse(key).expect("test key valid"),
        )
        .await
        .expect("send message")
}

fn message_of(outcome: &SendMessageOutcome) -> &crate::domain::message::Message {
    match outcome {
        SendMessageOutcome::Created { message } | SendMessageOutcome::Replayed { message } => {
            message
        }
    }
}

async fn count_facts(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM human_event_facts")
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

async fn count_deliveries_for(f: &Fixture, human_id: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM human_event_deliveries WHERE human_id = ?")
        .bind(human_id)
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

async fn list(f: &Fixture, human_id: &ParticipantId, after: i64) -> Vec<HumanEventWithFact> {
    f.human_events
        .list_events(human_id, after, 100)
        .await
        .expect("list human events")
}

async fn snapshot_of(f: &Fixture, human_id: &str, sequence: i64) -> serde_json::Value {
    let snap: String = sqlx::query_scalar(
        "SELECT f.snapshot FROM human_event_facts f \
         JOIN human_event_deliveries d ON d.fact_id = f.fact_id \
         WHERE d.human_id = ? AND d.sequence = ?",
    )
    .bind(human_id)
    .bind(sequence)
    .fetch_one(f.store.pool())
    .await
    .expect("snapshot row");
    serde_json::from_str(&snap).expect("snapshot is valid json")
}

/// migration 009 的 JSON1 前提由真实迁移测试证明：应用 009 后 json_object /
/// json_valid / json_extract 可用，CHECK 会拒绝非 JSON snapshot。
#[tokio::test]
async fn migration_009_proves_json1_and_snapshot_check() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("core.db");
    let store = MigrationStore::open(&db_path)
        .await
        .expect("open + migrate");
    let status = store.status().await.unwrap();
    assert_eq!(status.pending, 0, "all migrations applied");
    let pool = store.pool();

    // JSON1 函数真实可用。
    let value: String = sqlx::query_scalar("SELECT json_object('a', 1, 'b', json_array(2, 3))")
        .fetch_one(pool)
        .await
        .expect("json_object available");
    assert_eq!(value, r#"{"a":1,"b":[2,3]}"#);
    let valid: i64 = sqlx::query_scalar("SELECT json_valid('[1,2]')")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(valid, 1);
    let extracted: String = sqlx::query_scalar(
        "SELECT json_extract(json_object('participant', json_object('id','h1')), '$.participant.id')",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(extracted, "h1");

    // human_event_facts.snapshot 的 json_valid CHECK 拒绝非 JSON 文本。
    let id = ParticipantId(Uuid::new_v4().to_string());
    sqlx::query(
        "INSERT INTO participants (id, kind, status, display_name, created_at, revoked_at) \
         VALUES (?, 'human', 'active', 'X', 0, NULL)",
    )
    .bind(id.as_str())
    .execute(pool)
    .await
    .unwrap();
    let room = RoomId::new_v4();
    let membership_id = MembershipId::new_v4();
    // rooms → room_coverage 是 deferred 复合外键，必须在同一事务内完成
    // room + coverage（group 见证必须指向 owner 周期）+ membership 闭合。
    let mut conn = pool.acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    sqlx::query("INSERT INTO rooms (id, kind, status, created_at, name) VALUES (?, 'group', 'active', 1, 'g')")
        .bind(room.as_str())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO room_coverage \
         (room_id, kind, created_at, group_owner_membership_id) \
         VALUES (?, 'group', 1, ?)",
    )
    .bind(room.as_str())
    .bind(membership_id.as_str())
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO room_memberships \
         (membership_id, room_id, participant_id, role, joined_at, join_after_sequence) \
         VALUES (?, ?, ?, 'owner', 1, 0)",
    )
    .bind(membership_id.as_str())
    .bind(room.as_str())
    .bind(id.as_str())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let bad = sqlx::query(
        "INSERT INTO human_event_facts (fact_id, kind, source_id, room_id, created_at, snapshot) \
         VALUES (?, 'member.joined', ?, ?, 1, 'not json')",
    )
    .bind("00000000-0000-4000-8000-000000000001")
    .bind(membership_id.as_str())
    .bind(room.as_str())
    .execute(pool)
    .await;
    assert!(bad.is_err(), "non-json snapshot must be rejected");
}

/// message.created：Human 消息与 Completion 写回的 Agent 消息都进入 Human
/// Event；目标 = 发生时 Room 授权可见的 Human（含发送者本人）；共享快照
/// 一份、per-Human delivery 按人数、不复制正文。
#[tokio::test]
async fn message_created_targets_active_humans_with_single_shared_fact() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let agent_id = create_agent(&f, &alice.participant, "助手", "k-1").await;

    // human_direct：两位 Human 都是目标（含发送者本人）。
    accept_contact(&f, &alice.participant, &bob).await;
    let hd_outcome = room_application::create_human_direct_room(
        &f.room,
        &alice.participant,
        bob.id.as_str(),
        "hd-1",
    )
    .await
    .expect("human direct create");
    let room_application::CreateRoomOutcome::Created {
        room: hd_room,
        memberships: hd_memberships,
    } = hd_outcome
    else {
        panic!("must be created");
    };
    assert_eq!(hd_memberships.len(), 2);
    let sent = send(
        &f.message,
        &alice.participant.id,
        &hd_room.id,
        "hi bob",
        "m-1",
    )
    .await;
    let msg = message_of(&sent);
    // 共享事实恰好一份（不按收件人数复制正文）。
    assert_eq!(count_facts(&f).await, 3, "2 joined + 1 message.created");
    assert_eq!(
        count_deliveries_for(&f, alice.participant.id.as_str()).await,
        3
    );
    assert_eq!(count_deliveries_for(&f, bob.id.as_str()).await, 2);

    // 快照：message.created 含公开字段，不含 mentions/幂等键。
    let snap = snapshot_of(&f, alice.participant.id.as_str(), 3).await;
    assert_eq!(snap["kind_of"], serde_json::Value::Null);
    let alice_events = list(&f, &alice.participant.id, 0).await;
    let created = alice_events
        .iter()
        .find(|e| e.fact.kind.as_str() == "message.created")
        .expect("alice has the message.created event");
    assert_eq!(created.sequence, 3);
    assert_eq!(created.fact.source_id, msg.id.as_str());
    assert_eq!(created.fact.room_id, hd_room.id);
    let snap: serde_json::Value = serde_json::from_str(&created.fact.snapshot).unwrap();
    assert_eq!(snap["id"], msg.id.as_str());
    assert_eq!(snap["room_id"], hd_room.id.as_str());
    assert!(
        snap.get("sequence").is_none(),
        "message.created snapshot must not contain the internal message sequence"
    );
    assert_eq!(snap["actor"]["id"], alice.participant.id.as_str());
    assert_eq!(snap["actor"]["kind"], "human");
    assert_eq!(snap["body"], "hi bob");
    assert!(snap.get("idempotency_key").is_none());
    assert!(snap.get("mentions").is_none());

    // Bob 也能看到同一条事实（同一 fact_id），sequence 独立。
    let bob_events = list(&f, &bob.id, 0).await;
    let bob_created = bob_events
        .iter()
        .find(|e| e.fact.kind.as_str() == "message.created")
        .expect("bob has the message.created event");
    assert_eq!(
        bob_created.fact.id, created.fact.id,
        "shared fact not copied"
    );
    assert_eq!(bob_created.fact.snapshot, created.fact.snapshot);
    assert_eq!(bob_created.sequence, 2, "bob: own joined + message");

    // Agent 自己说的话（say）也进入 Human Event。
    // ⚠ 2026-09-28：原走 Completion reply 写回；Completion/ACK 整体退役后
    // 改走 say——Human Event 的语义与来源无关（普通 Message 都进）。
    let direct = create_direct_room(&f, &alice.participant, &agent_id, "d-1").await;
    let sent2 = send(
        &f.message,
        &alice.participant.id,
        &direct.0,
        "hello agent",
        "m-2",
    )
    .await;
    let _ = message_of(&sent2);
    let said = send(
        &f.message,
        &agent_id,
        &direct.0,
        "echo: hello agent",
        "reply-key-1",
    )
    .await;
    let reply_message_id = message_of(&said).id.clone();
    let alice_events = list(&f, &alice.participant.id, 0).await;
    let reply_created = alice_events
        .iter()
        .find(|e| e.fact.source_id == reply_message_id.as_str())
        .expect("agent reply message.created event");
    assert_eq!(reply_created.fact.kind.as_str(), "message.created");
    let snap: serde_json::Value = serde_json::from_str(&reply_created.fact.snapshot).unwrap();
    assert_eq!(snap["actor"]["id"], agent_id.as_str());
    assert_eq!(snap["actor"]["kind"], "agent");
    assert_eq!(snap["body"], "echo: hello agent");
}
/// member.joined：新周期（含奠基周期与群邀请/重入）给加入后的 Human 集合；
/// Agent 主体也产生事件；human_direct 第二位 Human 能由 member.joined 发现
/// Room。
#[tokio::test]
async fn member_joined_targets_post_join_humans() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let agent_id = create_agent(&f, &alice.participant, "助手", "k-1").await;
    let (direct, direct_ms) = create_direct_room(&f, &alice.participant, &agent_id, "d-1").await;
    // direct 奠基：Human owner + Agent member 各一条 joined；Human 收到两条。
    let alice_events = list(&f, &alice.participant.id, 0).await;
    assert_eq!(alice_events.len(), 2);
    assert!(
        alice_events
            .iter()
            .all(|e| e.fact.kind.as_str() == "member.joined")
    );
    let join_snaps: Vec<&HumanEventWithFact> = alice_events.iter().collect();
    let snap: serde_json::Value = serde_json::from_str(&join_snaps[0].fact.snapshot).unwrap();
    assert_eq!(snap["room_id"], direct.as_str());
    assert_eq!(snap["participant"]["kind"], "human");
    assert_eq!(snap["role"], "owner");
    assert_eq!(
        join_snaps[0].fact.source_id,
        direct_ms[0].membership_id.as_str()
    );

    // human_direct：Bob 的 joined 同时投给两端；Alice 的奠基周期先于 Bob
    // 的周期插入，只投给当时 active 的 Alice（加入后集合语义按插入时序）。
    accept_contact(&f, &alice.participant, &bob).await;
    let hd_outcome = room_application::create_human_direct_room(
        &f.room,
        &alice.participant,
        bob.id.as_str(),
        "hd-1",
    )
    .await
    .expect("human direct create");
    let room_application::CreateRoomOutcome::Created { room: hd, .. } = hd_outcome else {
        panic!("must be created");
    };
    let bob_events = list(&f, &bob.id, 0).await;
    assert_eq!(bob_events.len(), 1, "bob sees his own hd joined");
    assert_eq!(bob_events[0].fact.kind.as_str(), "member.joined");
    let snap: serde_json::Value = serde_json::from_str(&bob_events[0].fact.snapshot).unwrap();
    assert_eq!(snap["room_id"], hd.id.as_str());
    assert_eq!(snap["participant"]["id"], bob.id.as_str());
    assert_eq!(snap["role"], "member");

    // group：邀请 Agent 主体 → owner 收到一条 joined。
    let group = create_group(&f, &alice.participant, "群", "g-1").await;
    let _ = invite(&f, &alice.participant, group.as_str(), &agent_id, "i-ag-1").await;
    let _ = invite(&f, &alice.participant, group.as_str(), &bob.id, "i-bob-1").await;
    let alice_events = list(&f, &alice.participant.id, 0).await;
    let agent_join =
        alice_events
            .iter()
            .filter(|e| e.fact.kind.as_str() == "member.joined" && e.fact.room_id == group)
            .filter(|e| {
                serde_json::from_str::<serde_json::Value>(&e.fact.snapshot).unwrap()["participant"]
                    ["id"]
                    == agent_id.as_str()
            })
            .count();
    assert_eq!(
        agent_join, 1,
        "agent subject joined event for alice in group"
    );

    // 重入：新 key 打开新周期 → 又一条 joined。
    room_application::leave_membership(&f.room, &bob.id, group.as_str())
        .await
        .expect("bob leaves");
    let _ = invite(&f, &alice.participant, group.as_str(), &bob.id, "i-bob-2").await;
    let bob_events = list(&f, &bob.id, 0).await;
    let bob_joins: Vec<&HumanEventWithFact> = bob_events
        .iter()
        .filter(|e| e.fact.kind.as_str() == "member.joined" && e.fact.room_id == group)
        .collect();
    assert_eq!(bob_joins.len(), 2, "1 group join + 1 rejoin");
}

/// **Agent 退群**（M2 窄解冻切片，2026-09-27）：走**同一条** member.left 路径——
/// 事实由既有触发器 `human_event_facts_member_left` 生成（它本来就与参与者
/// kind 无关），投递给关闭后仍在 Room 的 Human；主体自己（Agent）**没有**
/// Human Event 收件箱（它不是 Human）。
#[tokio::test]
async fn agent_leave_produces_the_same_member_left_fact_as_a_human_leave() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let agent_id = create_agent(&f, &alice.participant, "助手", "k-ag").await;
    let group = create_group(&f, &alice.participant, "群", "g-ag").await;
    accept_contact(&f, &alice.participant, &bob).await;
    let _ = invite(&f, &alice.participant, group.as_str(), &bob.id, "i-bob-ag").await;
    let _ = invite(&f, &alice.participant, group.as_str(), &agent_id, "i-ag-ag").await;

    let closed = room_application::leave_membership(&f.room, &agent_id, group.as_str())
        .await
        .expect("agent leaves the group");

    // 周期真的关了：理由是 leave，left_before_sequence 落定。
    let (active, closed_by, left_before): (i64, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT active_marker, closed_by, left_before_sequence FROM room_memberships \
         WHERE membership_id = ?",
    )
    .bind(closed.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(active, 0);
    assert_eq!(closed_by.as_deref(), Some("leave"));
    assert!(left_before.is_some(), "leave must fix left_before_sequence");

    // 事实：一条 member.left，source = 被关闭的周期，snapshot 里 kind=agent。
    let snapshot: String = sqlx::query_scalar(
        "SELECT snapshot FROM human_event_facts \
         WHERE kind = 'member.left' AND source_id = ? AND room_id = ?",
    )
    .bind(closed.as_str())
    .bind(group.as_str())
    .fetch_one(f.store.pool())
    .await
    .expect("agent leave produces a member.left fact");
    let snapshot: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
    assert_eq!(snapshot["participant"]["kind"], "agent");
    assert_eq!(snapshot["participant"]["id"], agent_id.as_str());
    assert_eq!(snapshot["closed_by"], "leave");
    assert_eq!(snapshot["room_id"], group.as_str());

    // 投递：房内剩下的两位 Human 各收到一条；Agent 自己没有 Human 收件箱。
    for human in [&alice.participant.id, &bob.id] {
        let events = list(&f, human, 0).await;
        let left = events
            .iter()
            .filter(|e| e.fact.kind.as_str() == "member.left")
            .filter(|e| e.fact.source_id == closed.as_str())
            .count();
        assert_eq!(left, 1, "房内 Human {human} 应收到那条 member.left");
    }
    assert!(
        list(&f, &agent_id, 0).await.is_empty(),
        "Agent 不是 Human：它没有 Human Event 收件箱"
    );
}

/// **`human_direct` 的出口归关系模型**，不是 `leave`：即使调用者正是那间房
/// member 周期的持有人，也一律 `NotFound`（与 Human 侧同一形状）。
/// 负对照：同一对人在**群**里能退 ⇒ 拒绝来自房 kind，不是调用者。
#[tokio::test]
async fn leave_is_not_the_exit_of_a_human_direct_room() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    accept_contact(&f, &alice.participant, &bob).await;

    let outcome = room_application::create_human_direct_room(
        &f.room,
        &alice.participant,
        bob.id.as_str(),
        "hd-leave",
    )
    .await
    .expect("human direct create");
    let room_application::CreateRoomOutcome::Created {
        room: hd_room,
        memberships,
    } = outcome
    else {
        panic!("first human_direct create must be Created");
    };
    // 那间房的 member 周期持有人（owner 那一位另有转让规则管）。
    let member = memberships
        .iter()
        .find(|m| m.role == crate::domain::room::MembershipRole::Member)
        .expect("human_direct has one member period");

    let err =
        room_application::leave_membership(&f.room, &member.participant_id, hd_room.id.as_str())
            .await
            .unwrap_err();
    assert!(
        matches!(err, room_application::LeaveMembershipError::RoomNotFound),
        "human_direct 的出口不是 leave（归关系模型），got {err}"
    );

    // 守卫兜底：绕过存储层直接写同样的关闭，也必须被拒绝。
    let guard = sqlx::query(
        "UPDATE room_memberships SET active_marker = 0, left_at = 1, \
         left_before_sequence = 0, closed_by = 'leave' WHERE membership_id = ?",
    )
    .bind(member.membership_id.as_str())
    .execute(f.store.pool())
    .await;
    assert!(
        guard.is_err(),
        "human_direct 的成员周期不能被 closed_by='leave' 关闭（守卫兜底）"
    );

    // 负对照：同一对人在群里能退。
    let group = create_group(&f, &alice.participant, "群", "g-hd").await;
    let _ = invite(&f, &alice.participant, group.as_str(), &bob.id, "i-bob-hd").await;
    room_application::leave_membership(&f.room, &bob.id, group.as_str())
        .await
        .expect("同一对人在群里能退");
}

/// member.left：leave / removed / agent_revoked 各产生一条；目标是关闭后
/// 仍在 Room 的 Human；主体是 Human 时本人还收到自己的 member.left。
#[tokio::test]
async fn member_left_targets_remaining_humans_and_subject_self() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let carol = add_human(f.store.pool(), &f.identity, "Carol").await;
    let agent_id = create_agent(&f, &alice.participant, "助手", "k-1").await;
    let group = create_group(&f, &alice.participant, "群", "g-1").await;
    accept_contact(&f, &alice.participant, &bob).await;
    accept_contact(&f, &alice.participant, &carol).await;
    let bob_ms = invite(&f, &alice.participant, group.as_str(), &bob.id, "i-bob-1").await;
    let _ = invite(
        &f,
        &alice.participant,
        group.as_str(),
        &carol.id,
        "i-carol-1",
    )
    .await;
    let _ = invite(&f, &alice.participant, group.as_str(), &agent_id, "i-ag-1").await;

    // Bob 主动退群：Alice / Carol 收到 member.left；Bob 本人也收到。
    let before_bob = count_deliveries_for(&f, bob.id.as_str()).await;
    room_application::leave_membership(&f.room, &bob.id, group.as_str())
        .await
        .expect("bob leaves");
    let bob_events = list(&f, &bob.id, 0).await;
    let own_left = bob_events
        .iter()
        .filter(|e| e.fact.kind.as_str() == "member.left" && e.fact.room_id == group)
        .collect::<Vec<_>>();
    assert_eq!(own_left.len(), 1, "bob gets his own member.left");
    let snap: serde_json::Value = serde_json::from_str(&own_left[0].fact.snapshot).unwrap();
    assert_eq!(snap["participant"]["id"], bob.id.as_str());
    assert_eq!(snap["closed_by"], "leave");
    assert_eq!(snap["role"], "member");
    assert_eq!(own_left[0].fact.source_id, bob_ms.membership_id.as_str());
    assert_eq!(
        count_deliveries_for(&f, bob.id.as_str()).await,
        before_bob + 1
    );
    // 仍在 Room 的 Human 各收到一条。
    let alice_left = list(&f, &alice.participant.id, 0)
        .await
        .iter()
        .filter(|e| e.fact.kind.as_str() == "member.left" && e.fact.room_id == group)
        .count();
    let carol_left = list(&f, &carol.id, 0)
        .await
        .iter()
        .filter(|e| e.fact.kind.as_str() == "member.left" && e.fact.room_id == group)
        .count();
    assert_eq!(alice_left, 1);
    assert_eq!(carol_left, 1);

    // owner 移除 Carol（removed）：Carold 收到自己的 left；Alice/Bob(已离群，
    // 但此时在群的是 alice) 收到。
    let before_carol = count_deliveries_for(&f, carol.id.as_str()).await;
    room_application::remove_group_member(
        &f.room,
        &alice.participant,
        group.as_str(),
        carol.id.as_str(),
    )
    .await
    .expect("remove carol");
    let carol_events = list(&f, &carol.id, 0).await;
    let own_left = carol_events
        .iter()
        .filter(|e| e.fact.kind.as_str() == "member.left" && e.fact.room_id == group)
        .collect::<Vec<_>>();
    assert_eq!(own_left.len(), 1, "removed subject gets own member.left");
    let snap: serde_json::Value = serde_json::from_str(&own_left[0].fact.snapshot).unwrap();
    assert_eq!(snap["closed_by"], "removed");
    assert_eq!(
        count_deliveries_for(&f, carol.id.as_str()).await,
        before_carol + 1
    );

    // Agent revoke：主体是 Agent（无自投递），仍在 Room 的 Human 收到。
    let before_alice_left = list(&f, &alice.participant.id, 0)
        .await
        .iter()
        .filter(|e| e.fact.kind.as_str() == "member.left")
        .count();
    agent::revoke_agent(&f.agent, &alice.participant.id, agent_id.as_str())
        .await
        .expect("revoke agent");
    let alice_left_after = list(&f, &alice.participant.id, 0)
        .await
        .iter()
        .filter(|e| e.fact.kind.as_str() == "member.left")
        .count();
    assert_eq!(
        alice_left_after,
        before_alice_left + 1,
        "agent revoke left event"
    );
    let alice_events_now = list(&f, &alice.participant.id, 0).await;
    let agent_left =
        alice_events_now
            .iter()
            .filter(|e| e.fact.kind.as_str() == "member.left")
            .find(|e| {
                serde_json::from_str::<serde_json::Value>(&e.fact.snapshot).unwrap()["participant"]
                    ["id"]
                    == agent_id.as_str()
            })
            .expect("agent subject left event");
    let snap: serde_json::Value = serde_json::from_str(&agent_left.fact.snapshot).unwrap();
    assert_eq!(snap["closed_by"], "agent_revoked");
}

/// 群主转让：四周期切换不产生 joined/left 噪音，只发一条 owner.transferred；
/// 目标 = 转让时点 Room 的 Human 成员。
#[tokio::test]
async fn owner_transfer_emits_single_transferred_without_join_left_noise() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let group = create_group(&f, &alice.participant, "群", "g-1").await;
    accept_contact(&f, &alice.participant, &bob).await;
    let _ = invite(&f, &alice.participant, group.as_str(), &bob.id, "i-bob-1").await;

    let before = count_facts(&f).await;
    // 转让前的奠基 joined 数（按人）：Alice 有 2（自己 + Bob），Bob 有 1
    // （自己）——转让不得新增任何 joined/left。
    let joined_before_alice = list(&f, &alice.participant.id, 0)
        .await
        .iter()
        .filter(|e| e.fact.room_id == group && e.fact.kind.as_str() == "member.joined")
        .count();
    let joined_before_bob = list(&f, &bob.id, 0)
        .await
        .iter()
        .filter(|e| e.fact.room_id == group && e.fact.kind.as_str() == "member.joined")
        .count();
    let outcome = room_application::transfer_group_owner(
        &f.room,
        &alice.participant,
        group.as_str(),
        bob.id.as_str(),
        "t-1",
    )
    .await
    .expect("transfer");
    let room_application::TransferOwnerOutcome::Transferred {
        membership: new_owner,
        transition_id,
    } = outcome
    else {
        panic!("must be transferred");
    };
    assert_eq!(new_owner.participant_id, bob.id);

    let after = count_facts(&f).await;
    assert_eq!(after, before + 1, "only the owner.transferred fact");

    for human in [&alice.participant.id, &bob.id] {
        let events = list(&f, human, 0).await;
        let group_events: Vec<&HumanEventWithFact> =
            events.iter().filter(|e| e.fact.room_id == group).collect();
        // 转让四周期切换不产生 joined/left 噪音：每个 Human 只有奠基周期
        // 的 member.joined + 恰好一条 owner.transferred，零 member.left。
        let joined = group_events
            .iter()
            .filter(|e| e.fact.kind.as_str() == "member.joined")
            .count();
        let left = group_events
            .iter()
            .filter(|e| e.fact.kind.as_str() == "member.left")
            .count();
        let transferred: Vec<&&HumanEventWithFact> = group_events
            .iter()
            .filter(|e| e.fact.kind.as_str() == "owner.transferred")
            .collect();
        let expected_joined = if *human == alice.participant.id {
            joined_before_alice
        } else {
            joined_before_bob
        };
        assert_eq!(
            joined, expected_joined,
            "no joined noise from the four-period switch"
        );
        assert_eq!(left, 0, "no member.left noise from the four-period switch");
        assert_eq!(transferred.len(), 1, "exactly one owner.transferred");
        assert_eq!(transferred[0].fact.source_id, transition_id);
        let snap: serde_json::Value = serde_json::from_str(&transferred[0].fact.snapshot).unwrap();
        assert_eq!(snap["old_owner"]["id"], alice.participant.id.as_str());
        assert_eq!(snap["new_owner"]["id"], bob.id.as_str());
    }
}

/// 读取时授权过滤：离群后看不到 Room 的其他事件（消息/他人 left），只能
/// 看自己的 member.left；重入后 member.joined 可见并恢复 History；全局
/// after cursor 无越权也不永久漏掉权威 History。
#[tokio::test]
async fn read_time_filtering_fail_closed_except_own_member_left() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let carol = add_human(f.store.pool(), &f.identity, "Carol").await;
    let group = create_group(&f, &alice.participant, "群", "g-1").await;
    accept_contact(&f, &alice.participant, &bob).await;
    accept_contact(&f, &alice.participant, &carol).await;
    let _ = invite(&f, &alice.participant, group.as_str(), &bob.id, "i-bob-1").await;
    let _ = invite(
        &f,
        &alice.participant,
        group.as_str(),
        &carol.id,
        "i-carol-1",
    )
    .await;

    // Bob 在群期间发送 1-2；Alice 发送 3。
    let b1 = message_of(&send(&f.message, &bob.id, &group, "b1", "b-m-1").await)
        .id
        .clone();
    let b2 = message_of(&send(&f.message, &bob.id, &group, "b2", "b-m-2").await)
        .id
        .clone();
    let a1 = message_of(&send(&f.message, &alice.participant.id, &group, "a1", "a-m-1").await)
        .id
        .clone();

    // Bob 退群。
    room_application::leave_membership(&f.room, &bob.id, group.as_str())
        .await
        .expect("bob leaves");
    // 群内继续产生消息（Bob 不在）。
    let a2 = message_of(&send(&f.message, &alice.participant.id, &group, "a2", "a-m-2").await)
        .id
        .clone();
    let a3 = message_of(&send(&f.message, &alice.participant.id, &group, "a3", "a-m-3").await)
        .id
        .clone();
    // Carol 退群 → Bob 离群期间也收不到 Carol 的 left。
    room_application::leave_membership(&f.room, &carol.id, group.as_str())
        .await
        .expect("carol leaves");

    // Bob 拉取：离群后只可见自己的 member.left；群消息、他人 left 全部
    // fail closed（读取时按当前 Membership 过滤的唯一例外是本人
    // member.left）。
    let bob_events = list(&f, &bob.id, 0).await;
    assert_eq!(bob_events.len(), 1, "out-of-group events fail closed");
    assert_eq!(bob_events[0].fact.kind.as_str(), "member.left");
    let snap: serde_json::Value = serde_json::from_str(&bob_events[0].fact.snapshot).unwrap();
    assert_eq!(snap["participant"]["id"], bob.id.as_str());
    assert_eq!(snap["closed_by"], "leave");

    // 重入：member.joined 可见并恢复 History；离群间隙的事实（a2/a3 与
    // Carol 的 left）从未投递给 Bob，重入后依然不可见。
    let _ = invite(&f, &alice.participant, group.as_str(), &bob.id, "i-bob-2").await;
    let b4 = message_of(&send(&f.message, &bob.id, &group, "b4", "b-m-4").await)
        .id
        .clone();
    let b5 = message_of(&send(&f.message, &bob.id, &group, "b5", "b-m-5").await)
        .id
        .clone();
    let bob_events = list(&f, &bob.id, 0).await;
    let joined_count = bob_events
        .iter()
        .filter(|e| e.fact.kind.as_str() == "member.joined")
        .count();
    assert_eq!(joined_count, 3, "own + carol + rejoin joined events");
    // Bob 可见：3 次 joined + 1..3/6..7 五条消息 + 本人 left = 9；a2/a3
    // （序列 4-5）与 Carol 的 left 不存在。
    assert_eq!(bob_events.len(), 9, "delivered facts + rejoin visible");
    let own_left = bob_events
        .iter()
        .filter(|e| e.fact.kind.as_str() == "member.left")
        .collect::<Vec<_>>();
    assert_eq!(own_left.len(), 1, "own member.left survives rejoin");
    let created_ids: Vec<String> = bob_events
        .iter()
        .filter(|e| e.fact.kind.as_str() == "message.created")
        .map(|e| e.fact.source_id.clone())
        .collect();
    assert_eq!(
        created_ids,
        vec![
            b1.as_str(),
            b2.as_str(),
            a1.as_str(),
            b4.as_str(),
            b5.as_str()
        ],
        "gap messages (a2/a3) never visible"
    );
    let a2_id = a2.as_str().to_owned();
    let a3_id = a3.as_str().to_owned();
    assert!(
        !created_ids.contains(&a2_id) && !created_ids.contains(&a3_id),
        "a2/a3 message ids must not appear among visible created events"
    );
    for e in bob_events
        .iter()
        .filter(|e| e.fact.kind.as_str() == "message.created")
    {
        let snap: serde_json::Value = serde_json::from_str(&e.fact.snapshot).unwrap();
        assert!(
            snap.get("sequence").is_none(),
            "message.created snapshot must not contain the internal message sequence"
        );
    }
    // 历史窗口：Room History 按 Membership 周期窗口恢复（1-3 与重入后的
    // 6-7），a2/a3（4-5）属于离群间隙仍不可见。
    let history = f
        .message
        .list_messages(&bob.id, &group, 100, None)
        .await
        .expect("history")
        .expect("member can read");
    let seqs: Vec<i64> = history
        .messages
        .iter()
        .map(|m| m.message.sequence)
        .collect();
    assert_eq!(seqs, vec![1, 2, 3, 6, 7], "history windows restored");
    // 全局 after cursor 不产生越权：a2/a3 的 fact 从未投递给 Bob，也不可读。
    let a2_fact: String = sqlx::query_scalar(
        "SELECT f.fact_id FROM human_event_facts f WHERE f.kind='message.created' \
         AND f.source_id = ?",
    )
    .bind(a2.as_str())
    .fetch_one(f.store.pool())
    .await
    .expect("a2 fact");
    let leaked: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM human_event_deliveries WHERE human_id = ? AND fact_id = ?",
    )
    .bind(bob.id.as_str())
    .bind(&a2_fact)
    .fetch_optional(f.store.pool())
    .await
    .expect("probe");
    assert!(leaked.is_none(), "gap messages never delivered to bob");
}

/// 每 Human sequence 从 1 单调唯一；不同 Human 互不隔离泄露；post-commit
/// delivery_targets 按 source 反查去重。
#[tokio::test]
async fn per_human_sequence_is_monotonic_and_delivery_targets_are_precise() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let group = create_group(&f, &alice.participant, "群", "g-1").await;
    accept_contact(&f, &alice.participant, &bob).await;
    let _ = invite(&f, &alice.participant, group.as_str(), &bob.id, "i-bob-1").await;

    let mut seqs_alice: Vec<i64> = Vec::new();
    let mut seqs_bob: Vec<i64> = Vec::new();
    for i in 1..=5 {
        let sent = send(
            &f.message,
            &alice.participant.id,
            &group,
            &format!("m{i}"),
            &format!("k-{i}"),
        )
        .await;
        let msg = message_of(&sent);
        // post-commit 唤醒目标：这条消息的 fact 恰投给 Alice 与 Bob。
        let targets = f
            .human_events
            .delivery_targets(&[HumanEventSource::new(
                HumanEventKind::MessageCreated,
                msg.id.as_str().to_owned(),
            )])
            .await
            .expect("delivery targets");
        let ids: Vec<&str> = targets.iter().map(|p| p.as_str()).collect();
        assert_eq!(ids.len(), 2, "both humans targeted, deduped");
        assert!(ids.contains(&alice.participant.id.as_str()));
        assert!(ids.contains(&bob.id.as_str()));
        seqs_alice.push(msg.sequence);
        seqs_bob.push(msg.sequence);
    }
    // 未知 source：空。
    let empty = f
        .human_events
        .delivery_targets(&[HumanEventSource::new(
            HumanEventKind::MessageCreated,
            "00000000-0000-4000-8000-000000000000",
        )])
        .await
        .expect("delivery targets");
    assert!(empty.is_empty());

    let alice_events = list(&f, &alice.participant.id, 0).await;
    let bob_events = list(&f, &bob.id, 0).await;
    let alice_seqs: Vec<i64> = alice_events.iter().map(|e| e.sequence).collect();
    let bob_seqs: Vec<i64> = bob_events.iter().map(|e| e.sequence).collect();
    assert_eq!(
        alice_seqs,
        (1..=alice_seqs.len() as i64).collect::<Vec<i64>>(),
        "alice sequence is 1..N monotonic"
    );
    assert_eq!(
        bob_seqs,
        (1..=bob_seqs.len() as i64).collect::<Vec<i64>>(),
        "bob sequence is 1..N monotonic"
    );
    // 分页：after 语义无重复无跳过。
    let mut seen: Vec<i64> = Vec::new();
    let mut after = 0i64;
    loop {
        let page = f
            .human_events
            .list_events(&alice.participant.id, after, 3)
            .await
            .expect("page");
        if page.is_empty() {
            break;
        }
        let page_seqs: Vec<i64> = page.iter().map(|e| e.sequence).collect();
        assert!(
            page_seqs.windows(2).all(|w| w[0] < w[1]),
            "strictly ascending"
        );
        seen.extend(page_seqs.clone());
        after = *page_seqs.last().unwrap();
    }
    assert_eq!(seen, alice_seqs, "keyset paging no dup/skip");
    // 同一 after 重拉稳定（无副作用）。
    let again = f
        .human_events
        .list_events(&alice.participant.id, 0, 3)
        .await
        .expect("repull");
    assert_eq!(again[0].sequence, alice_events[0].sequence);
}

/// 跨 kind 同 source_id：fact 唯一身份是 (kind, source_id)，post-commit
/// 唤醒目标反查必须同时匹配 kind 与 source_id，不能只按 source_id——
/// 否则会错误唤醒另一条拥有同值 source_id 字符串的 fact 的 Human。
#[tokio::test]
async fn delivery_targets_match_kind_and_source_id_together() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let carol = add_human(f.store.pool(), &f.identity, "Carol").await;
    let group = create_group(&f, &alice.participant, "群", "g-1").await;
    accept_contact(&f, &alice.participant, &bob).await;
    let _ = invite(&f, &alice.participant, group.as_str(), &bob.id, "i-bob-1").await;

    // 第一条 message.created：source_id = msg id。
    let sent = send(&f.message, &alice.participant.id, &group, "m1", "k-1").await;
    let msg = message_of(&sent);
    let msg_id = msg.id.as_str().to_owned();

    // 直接建第二个 group Room，并把 Carol 的 membership_id 故意取成与
    // 上一条消息相同的 UUID 字符串：member.joined fact 的 source_id 与
    // message.created fact 的 source_id 同值，但 kind 不同——按 migration
    // 009 的 `(kind, source_id)` 唯一身份这是两个不同的 fact，投递目标也
    // 不同（g-1：Alice+Bob；room2：Alice+Carol）。
    accept_contact(&f, &alice.participant, &carol).await;
    let room2 = RoomId::new_v4();
    let membership_alice = MembershipId::new_v4();
    let membership_carol = MembershipId(msg_id.clone());
    let mut conn = f.store.pool().acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    // rooms → room_coverage → memberships（与 migration_009 相同的
    // deferred 复合外键事务顺序）。
    sqlx::query(
        "INSERT INTO rooms (id, kind, status, created_at, name) \
         VALUES (?, 'group', 'active', 1, 'g2')",
    )
    .bind(room2.as_str())
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO room_coverage \
         (room_id, kind, created_at, group_owner_membership_id) \
         VALUES (?, 'group', 1, ?)",
    )
    .bind(room2.as_str())
    .bind(membership_alice.as_str())
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO room_memberships \
         (membership_id, room_id, participant_id, role, joined_at, join_after_sequence) \
         VALUES (?, ?, ?, 'owner', 1, 0)",
    )
    .bind(membership_alice.as_str())
    .bind(room2.as_str())
    .bind(alice.participant.id.as_str())
    .execute(&mut *tx)
    .await
    .unwrap();
    // Carol 的周期：membership_id 与 msg_id 同值（跨 kind 同 source_id）；
    // 该插入会经触发器生成 member.joined fact（source_id = msg_id）。
    sqlx::query(
        "INSERT INTO room_memberships \
         (membership_id, room_id, participant_id, role, joined_at, join_after_sequence) \
         VALUES (?, ?, ?, 'member', 1, 0)",
    )
    .bind(membership_carol.as_str())
    .bind(room2.as_str())
    .bind(carol.id.as_str())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // 两个 fact 真实存在且 source_id 相同、kind 不同。
    let kinds: Vec<String> =
        sqlx::query_scalar("SELECT kind FROM human_event_facts WHERE source_id = ? ORDER BY kind")
            .bind(&msg_id)
            .fetch_all(f.store.pool())
            .await
            .unwrap();
    assert_eq!(
        kinds,
        vec!["member.joined".to_owned(), "message.created".to_owned()]
    );

    // 按完整 source 身份反查：message.created 只唤醒 g-1 的 Alice+Bob，
    // 不会把 Carol（只拥有同值 source_id 的 member.joined delivery）算进
    // 来；member.joined 只唤醒 room2 的 Alice+Carol。
    let wake_message = f
        .human_events
        .delivery_targets(&[HumanEventSource::new(
            HumanEventKind::MessageCreated,
            msg_id.clone(),
        )])
        .await
        .expect("message targets");
    let wake_message_ids: Vec<&str> = wake_message.iter().map(|p| p.as_str()).collect();
    assert_eq!(wake_message.len(), 2, "only the g-1 humans");
    assert!(wake_message_ids.contains(&alice.participant.id.as_str()));
    assert!(wake_message_ids.contains(&bob.id.as_str()));
    assert!(
        !wake_message_ids.contains(&carol.id.as_str()),
        "carol must not be woken by message.created source"
    );

    let wake_join = f
        .human_events
        .delivery_targets(&[HumanEventSource::new(HumanEventKind::MemberJoined, msg_id)])
        .await
        .expect("join targets");
    let wake_join_ids: Vec<&str> = wake_join.iter().map(|p| p.as_str()).collect();
    assert_eq!(wake_join.len(), 2, "only the room2 humans");
    assert!(wake_join_ids.contains(&alice.participant.id.as_str()));
    assert!(wake_join_ids.contains(&carol.id.as_str()));
    assert!(
        !wake_join_ids.contains(&bob.id.as_str()),
        "bob must not be woken by the member.joined source"
    );
}

/// 数据库反向写入拒绝矩阵：伪造 fact（错误 source/时间）、跳号、重复
/// delivery、错误 target、UPDATE/DELETE/REPLACE 全部拒绝。
#[tokio::test]
async fn database_reverse_writes_are_rejected() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let carol = add_human(f.store.pool(), &f.identity, "Carol").await;
    let group = create_group(&f, &alice.participant, "群", "g-1").await;
    accept_contact(&f, &alice.participant, &bob).await;
    let _ = invite(&f, &alice.participant, group.as_str(), &bob.id, "i-bob-1").await;
    let sent = send(&f.message, &alice.participant.id, &group, "m1", "k-1").await;
    let msg = message_of(&sent);
    let sent2 = send(&f.message, &alice.participant.id, &group, "m2", "k-2").await;
    let msg2 = message_of(&sent2);

    let pool = f.store.pool();
    let (alice_seq_max, bob_seq_max): (i64, i64) = sqlx::query_as(
        "SELECT \
            (SELECT COALESCE(MAX(sequence),0) FROM human_event_deliveries WHERE human_id = a.id), \
            (SELECT COALESCE(MAX(sequence),0) FROM human_event_deliveries WHERE human_id = b.id) \
         FROM participants a, participants b WHERE a.id = ? AND b.id = ?",
    )
    .bind(alice.participant.id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(pool)
    .await
    .unwrap();

    // 1) 伪造 fact：错误 kind/source 组合、错误 created_at、未知 source。
    for (kind, source, created_at) in [
        ("message.created", bob.id.as_str(), 0i64),
        ("member.joined", msg.id.as_str(), 0i64),
        ("member.left", msg.id.as_str(), 0i64),
        ("owner.transferred", msg.id.as_str(), 0i64),
    ] {
        let bad = sqlx::query(
            "INSERT INTO human_event_facts (fact_id, kind, source_id, room_id, created_at, snapshot) \
             VALUES (?, ?, ?, ?, ?, '{}')",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(kind)
        .bind(source)
        .bind(group.as_str())
        .bind(created_at)
        .execute(pool)
        .await;
        assert!(bad.is_err(), "forged fact {kind}/{source} must be rejected");
    }
    // 2) 跳号 delivery：直接给 Alice 插 sequence=MAX+2。
    let bad_seq = sqlx::query(
        "INSERT INTO human_event_deliveries (human_id, sequence, fact_id) \
         VALUES (?, ?, (SELECT fact_id FROM human_event_facts LIMIT 1))",
    )
    .bind(alice.participant.id.as_str())
    .bind(alice_seq_max + 2)
    .execute(pool)
    .await;
    assert!(bad_seq.is_err(), "gap sequence must be rejected");
    // 3) 重复 delivery（同 fact 同 human）：第二次插入被 UNIQUE 拒绝。
    let dup = sqlx::query(
        "INSERT INTO human_event_deliveries (human_id, sequence, fact_id) \
         SELECT d.human_id, d.sequence, d.fact_id FROM human_event_deliveries d LIMIT 1",
    )
    .execute(pool)
    .await;
    assert!(dup.is_err(), "duplicate delivery identity must be rejected");
    // 4) 错误 target：Carol 从未在该群有任何周期，任何 fact 都不可投递。
    let bad_target = sqlx::query(
        "INSERT INTO human_event_deliveries (human_id, sequence, fact_id) \
         VALUES (?, 1, (SELECT fact_id FROM human_event_facts LIMIT 1))",
    )
    .bind(carol.id.as_str())
    .execute(pool)
    .await;
    assert!(
        bad_target.is_err(),
        "wrong target delivery must be rejected"
    );
    // 5) Bob 已离群（无周期覆盖）后，把新 fact 投给他被拒。
    room_application::leave_membership(&f.room, &bob.id, group.as_str())
        .await
        .expect("bob leaves");
    let later_fact: String = sqlx::query_scalar(
        "SELECT fact_id FROM human_event_facts WHERE kind = 'message.created' \
         AND source_id = ?",
    )
    .bind(msg2.id.as_str())
    .fetch_one(pool)
    .await
    .unwrap();
    let late_delivery = sqlx::query(
        "INSERT INTO human_event_deliveries (human_id, sequence, fact_id) VALUES (?, ?, ?)",
    )
    .bind(bob.id.as_str())
    .bind(bob_seq_max + 1)
    .bind(&later_fact)
    .execute(pool)
    .await;
    assert!(
        late_delivery.is_err(),
        "late delivery to a non-covering human must be rejected"
    );
    // 6) UPDATE/DELETE fact 与 delivery 全部拒绝。
    for table in ["human_event_facts", "human_event_deliveries"] {
        let upd = sqlx::query(&format!("UPDATE {table} SET fact_id = fact_id"))
            .execute(pool)
            .await;
        assert!(upd.is_ok(), "equal-value self update tolerated");
        let upd = sqlx::query(&format!("UPDATE {table} SET created_at = created_at + 1"))
            .execute(pool)
            .await;
        assert!(upd.is_err(), "update on {table} must be rejected");
        let del = sqlx::query(&format!("DELETE FROM {table}"))
            .execute(pool)
            .await;
        assert!(del.is_err(), "delete on {table} must be rejected");
    }
    // 7) REPLACE 不能重建既有 fact / delivery。
    let (fact_id, kind, source_id, room_id, created_at, snapshot): (String, String, String, String, i64, String) =
        sqlx::query_as("SELECT fact_id, kind, source_id, room_id, created_at, snapshot FROM human_event_facts LIMIT 1")
            .fetch_one(pool).await.unwrap();
    let replace = sqlx::query(
        "INSERT OR REPLACE INTO human_event_facts \
         (fact_id, kind, source_id, room_id, created_at, snapshot) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&fact_id)
    .bind(&kind)
    .bind(&source_id)
    .bind(&room_id)
    .bind(created_at)
    .bind(&snapshot)
    .execute(pool)
    .await;
    assert!(replace.is_err(), "REPLACE on facts must be rejected");
    let (human_id, seq): (String, i64) =
        sqlx::query_as("SELECT human_id, sequence FROM human_event_deliveries LIMIT 1")
            .fetch_one(pool)
            .await
            .unwrap();
    let replace = sqlx::query(
        "INSERT OR REPLACE INTO human_event_deliveries (human_id, sequence, fact_id) \
         VALUES (?, ?, (SELECT fact_id FROM human_event_facts LIMIT 1))",
    )
    .bind(&human_id)
    .bind(seq)
    .execute(pool)
    .await;
    assert!(replace.is_err(), "REPLACE on deliveries must be rejected");
}

/// 时间戳守卫稳定性（migration 009 target guard）：
/// 服务端时钟进程内单调（`UtcMillis::now()` 绝不回拨）后，合法写入的
/// created_at 恒 >= 先前记录的 joined_at，守卫不会反向 abort 合法写入。
/// 本测试锁定两种形状：
/// - 冻结时钟等值（created_at == 目标 joined_at，系统时钟回拨后单调时钟
///   返回的形状）→ 消息合法成立并投递给全部覆盖成员；
/// - 回拨形状（created_at = joined_at - 1，非单调时钟才会产生的值）→
///   守卫仍拒绝，证明授权窗口未被削弱、未引入容差。
#[tokio::test]
async fn target_guard_accepts_frozen_clock_equality_and_rejects_backward_timestamps() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let group = create_group(&f, &alice.participant, "群", "g-1").await;
    accept_contact(&f, &alice.participant, &bob).await;
    let bob_membership = invite(&f, &alice.participant, group.as_str(), &bob.id, "i-bob-1").await;
    let bob_joined_at = bob_membership.joined_at;

    // 冻结时钟形状：消息 created_at == Bob 周期 joined_at（回拨后单调
    // 时钟返回的等值）→ Created，delivery 覆盖 Alice 与 Bob。
    let sent = f
        .message
        .send_message(
            &NewMessage {
                id: MessageId::new_v4(),
                room_id: group.clone(),
                actor_id: alice.participant.id.clone(),
                body: Some(MessageBody::parse("frozen").unwrap()),
                mentions: Vec::new(),
                attachments: Vec::new(),
                reply_to_message_id: None,
                created_at: bob_joined_at,
            },
            &IdempotencyKey::parse("f-1").unwrap(),
        )
        .await
        .expect("frozen-clock message must not be rejected by the target guard");
    let SendMessageOutcome::Created { message } = sent else {
        panic!("must be created");
    };
    let targets = f
        .human_events
        .delivery_targets(&[HumanEventSource::new(
            HumanEventKind::MessageCreated,
            message.id.as_str().to_owned(),
        )])
        .await
        .expect("targets");
    let ids: Vec<&str> = targets.iter().map(|p| p.as_str()).collect();
    assert_eq!(
        targets.len(),
        2,
        "both humans covered at the frozen instant"
    );
    assert!(ids.contains(&alice.participant.id.as_str()));
    assert!(ids.contains(&bob.id.as_str()));

    // 回拨形状：created_at = joined_at - 1（非单调时钟才会产生的值）→
    // 消息整体被守卫拒绝（Bob 的周期不覆盖该时点）。
    let backdated = f
        .message
        .send_message(
            &NewMessage {
                id: MessageId::new_v4(),
                room_id: group.clone(),
                actor_id: alice.participant.id.clone(),
                body: Some(MessageBody::parse("backdated").unwrap()),
                mentions: Vec::new(),
                attachments: Vec::new(),
                reply_to_message_id: None,
                created_at: UtcMillis::from_millis(bob_joined_at.millis() - 1),
            },
            &IdempotencyKey::parse("f-2").unwrap(),
        )
        .await;
    assert!(
        backdated.is_err(),
        "backward timestamp must still be rejected (authorization window preserved)"
    );
}

/// migration 009 后 `PRAGMA foreign_key_check` 干净：全新库真实迁移 +
/// 触发链真实产生 facts/deliveries 后逐条检查外键（delivery.fact_id →
/// human_event_facts.fact_id 主键、facts.room_id → rooms.id、
/// delivery.human_id → participants.id 等）。
#[tokio::test]
async fn migration_009_leaves_foreign_key_check_clean() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let group = create_group(&f, &alice.participant, "群", "g-1").await;
    accept_contact(&f, &alice.participant, &bob).await;
    let _ = invite(&f, &alice.participant, group.as_str(), &bob.id, "i-bob-1").await;
    let _ = send(&f.message, &alice.participant.id, &group, "m1", "fk-1").await;
    let _ = send(&f.message, &alice.participant.id, &group, "m2", "fk-2").await;
    let violations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_foreign_key_check")
        .fetch_one(f.store.pool())
        .await
        .expect("foreign_key_check must run");
    assert_eq!(
        violations, 0,
        "foreign key violations after real migration + trigger flow"
    );
}

/// 幂等重放与失败路径不产生新 fact/delivery；Agent revoke 的 post-commit
/// 唤醒目标 = 本次关闭的 membership。
#[tokio::test]
async fn replays_and_failures_produce_no_events_and_revoke_targets_are_exact() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "k-1").await;
    let (direct, _ms) = create_direct_room(&f, &alice.participant, &agent_id, "d-1").await;

    let sent = send(&f.message, &alice.participant.id, &direct, "hi", "m-1").await;
    let _msg = message_of(&sent);
    let facts_before = count_facts(&f).await;
    // 同 key 重放：不新增。
    let replayed = send(&f.message, &alice.participant.id, &direct, "hi", "m-1").await;
    assert!(matches!(replayed, SendMessageOutcome::Replayed { .. }));
    assert_eq!(
        count_facts(&f).await,
        facts_before,
        "replay must not create facts"
    );
    // 失败（非成员）：不新增。
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let err = f
        .message
        .send_message(
            &NewMessage {
                id: MessageId::new_v4(),
                room_id: direct.clone(),
                actor_id: bob.id.clone(),
                body: Some(MessageBody::parse("x").unwrap()),
                mentions: Vec::new(),
                attachments: Vec::new(),
                reply_to_message_id: None,
                created_at: UtcMillis::now(),
            },
            &IdempotencyKey::parse("b-1").unwrap(),
        )
        .await;
    assert!(err.is_err());
    assert_eq!(count_facts(&f).await, facts_before, "failed send no facts");

    // Agent revoke：post-commit 目标 = 关闭周期涉及 Room 的 Human。
    let targets = agent::revoke_agent(&f.agent, &alice.participant.id, agent_id.as_str())
        .await
        .expect("revoke");
    assert_eq!(targets.len(), 1, "one membership closed in the direct room");
    let wake = f
        .human_events
        .delivery_targets(&[HumanEventSource::new(
            HumanEventKind::MemberLeft,
            targets[0].as_str().to_owned(),
        )])
        .await
        .expect("targets");
    assert_eq!(wake.len(), 1);
    assert_eq!(wake[0], alice.participant.id);
    // 重复撤销：空列表，无新 fact。
    let facts_before = count_facts(&f).await;
    let targets2 = agent::revoke_agent(&f.agent, &alice.participant.id, agent_id.as_str())
        .await
        .expect("repeat revoke");
    assert!(targets2.is_empty());
    assert_eq!(
        count_facts(&f).await,
        facts_before,
        "repeat revoke no facts"
    );
}

/// 无副作用读取与重启保持：重启后事实、sequence 与重拉结果不变。
#[tokio::test]
async fn restart_keeps_facts_sequences_and_replay() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("core.db");
    let (alice_id, bob_id, group, msg_ids) = {
        let store = MigrationStore::open(&db_path).await.expect("open");
        let pool = store.pool().clone();
        let identity = SqliteIdentityStore::new(pool.clone());
        let room = SqliteRoomStore::new(pool.clone());
        let message = SqliteMessageStore::new(pool.clone());
        let alice = bootstrap_human(&identity, "Alice").await.unwrap();
        let bob = add_human(&pool, &identity, "Bob").await;
        let group = {
            let outcome = room_application::create_group(&room, &alice.participant, "群", "g-1")
                .await
                .unwrap();
            let room_application::CreateRoomOutcome::Created { room, .. } = outcome else {
                panic!("created");
            };
            room.id
        };
        accept_contact_db(&pool, alice.participant.id.as_str(), bob.id.as_str()).await;
        let outcome = room_application::invite_group_member(
            &room,
            &alice.participant,
            group.as_str(),
            bob.id.as_str(),
            "i-1",
        )
        .await
        .unwrap();
        let _ = outcome;
        let mut ids = Vec::new();
        for i in 1..=3 {
            let sent = message
                .send_message(
                    &NewMessage {
                        id: MessageId::new_v4(),
                        room_id: group.clone(),
                        actor_id: alice.participant.id.clone(),
                        body: Some(MessageBody::parse(&format!("m{i}")).unwrap()),
                        mentions: Vec::new(),
                        attachments: Vec::new(),
                        reply_to_message_id: None,
                        created_at: UtcMillis::now(),
                    },
                    &IdempotencyKey::parse(&format!("k-{i}")).unwrap(),
                )
                .await
                .unwrap();
            let SendMessageOutcome::Created { message } = sent else {
                panic!("created");
            };
            ids.push(message.id.as_str().to_owned());
        }
        (alice.participant.id, bob.id, group, ids)
    };

    // 重启后：事实、sequence 与重拉保持。
    let store = MigrationStore::open(&db_path).await.expect("reopen");
    let human_events = SqliteHumanEventStore::new(store.pool().clone());
    let events = human_events
        .list_events(&alice_id, 0, 100)
        .await
        .expect("list after restart");
    let created: Vec<&HumanEventWithFact> = events
        .iter()
        .filter(|e| e.fact.kind.as_str() == "message.created")
        .collect();
    assert_eq!(created.len(), 3, "restart keeps all message facts");
    let sources: Vec<&str> = created.iter().map(|e| e.fact.source_id.as_str()).collect();
    assert_eq!(
        sources,
        msg_ids.iter().map(|s| s.as_str()).collect::<Vec<&str>>(),
        "restart keeps stable source ids"
    );
    let events_bob = human_events
        .list_events(&bob_id, 0, 100)
        .await
        .expect("bob after restart");
    assert_eq!(
        events_bob
            .iter()
            .filter(|e| e.fact.kind.as_str() == "message.created")
            .count(),
        3
    );
    // 同一 after 重拉稳定。
    let again = human_events
        .list_events(&alice_id, 0, 100)
        .await
        .expect("repull after restart");
    assert_eq!(again.len(), events.len());
    assert_eq!(again[0].sequence, events[0].sequence);
    assert_eq!(again[0].fact.id, events[0].fact.id);
    let _ = group;
    let _ = msg_ids;
}

async fn accept_contact_db(pool: &SqlitePool, a: &str, b: &str) {
    let (ca, cb) = if a < b { (a, b) } else { (b, a) };
    sqlx::query(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at, idempotency_key) \
         VALUES (?, ?, ?, ?, ?, 'accepted', 1, 1, 'k')",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(ca)
    .bind(cb)
    .bind(ca)
    .bind(cb)
    .execute(pool)
    .await
    .expect("insert accepted contact");
}

/// message.created 快照在发生时固化 reply_to_message_id：迁移 009 的
/// messages AFTER INSERT 触发器从同一 messages 行直接读取该不可变列写入
/// json_object（无 target 为 JSON null），持久 snapshot 本身就含该字段，
/// 读取路径原样透传、不做任何动态投影；非 message.created 快照不受
/// 影响；不复制 target 正文/actor/边界。
#[tokio::test]
async fn message_created_snapshot_carries_reply_target_id_only() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    accept_contact(&f, &alice.participant, &bob).await;
    let hd_outcome = room_application::create_human_direct_room(
        &f.room,
        &alice.participant,
        bob.id.as_str(),
        "hd-1",
    )
    .await
    .expect("human direct create");
    let room_application::CreateRoomOutcome::Created { room: hd_room, .. } = hd_outcome else {
        panic!("must be created");
    };

    let target = send(
        &f.message,
        &alice.participant.id,
        &hd_room.id,
        "第一",
        "m-1",
    )
    .await;
    let target_msg = message_of(&target);
    let reply = send_reply(
        &f.message,
        &alice.participant.id,
        &hd_room.id,
        "回复第一",
        &target_msg.id,
        "m-2",
    )
    .await
    .expect("reply");
    let reply_msg = message_of(&reply);

    // 持久固化快照（不经读取路径）：target 消息固化为 JSON null，reply
    // 消息固化为 target ID——发生时即固化，不是读取时补投影。
    let raw_target: serde_json::Value =
        serde_json::from_str(&raw_created_snapshot(f.store.pool(), target_msg.id.as_str()).await)
            .unwrap();
    assert_eq!(
        raw_target["reply_to_message_id"],
        serde_json::Value::Null,
        "stored snapshot itself freezes a null reply target at occurrence time"
    );
    let raw_reply: serde_json::Value =
        serde_json::from_str(&raw_created_snapshot(f.store.pool(), reply_msg.id.as_str()).await)
            .unwrap();
    assert_eq!(
        raw_reply["reply_to_message_id"],
        serde_json::json!(target_msg.id.as_str()),
        "stored snapshot itself freezes the reply target id at occurrence time"
    );
    assert!(
        raw_reply.get("quoted_body").is_none()
            && raw_reply.get("quoted_actor").is_none()
            && raw_reply.get("target_sequence").is_none(),
        "stored snapshot must not embed target content"
    );

    // 读取路径原样透传：REST 看到的快照与持久快照一致（无 target 字段仍
    // 存在且为 null；有 target 时只带 ID）。
    let events = list(&f, &alice.participant.id, 0).await;
    let created_of = |message_id: &str| {
        events
            .iter()
            .find(|e| {
                e.fact.kind.as_str() == "message.created"
                    && serde_json::from_str::<serde_json::Value>(&e.fact.snapshot)
                        .expect("snapshot json")["id"]
                        == message_id
            })
            .expect("message.created event")
    };
    let target_event = created_of(target_msg.id.as_str());
    let reply_event = created_of(reply_msg.id.as_str());
    let target_snap: serde_json::Value = serde_json::from_str(&target_event.fact.snapshot).unwrap();
    assert_eq!(
        target_snap["reply_to_message_id"],
        serde_json::Value::Null,
        "plain message keeps a null reply target field"
    );
    let reply_snap: serde_json::Value = serde_json::from_str(&reply_event.fact.snapshot).unwrap();
    assert_eq!(
        reply_snap["reply_to_message_id"],
        serde_json::json!(target_msg.id.as_str()),
        "reply message exposes only the target id"
    );
    assert_eq!(
        raw_target, target_snap,
        "read path passes snapshot verbatim"
    );
    assert_eq!(raw_reply, reply_snap, "read path passes snapshot verbatim");

    // 成员事件（非 message.created）不携带该字段。
    let joined = events
        .iter()
        .find(|e| e.fact.kind.as_str() == "member.joined")
        .expect("joined event");
    let joined_snap: serde_json::Value = serde_json::from_str(&joined.fact.snapshot).unwrap();
    assert!(joined_snap.get("reply_to_message_id").is_none());

    // 唯一事实是 messages 列：恰好一条 Message 携带非空 reply target。
    let replies: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE reply_to_message_id IS NOT NULL")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(replies, 1);
}

/// 直接读取持久化的 message.created 快照文本（不经 Human Event 读取路径）。
async fn raw_created_snapshot(pool: &SqlitePool, message_id: &str) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT f.snapshot FROM human_event_facts f \
         WHERE f.source_id = ? AND f.kind = 'message.created'",
    )
    .bind(message_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

// ===== Human Event cursor 解析（resolve_event_after）=====

#[tokio::test]
async fn resolve_human_event_cursor_is_delivery_scoped_per_human() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(f.store.pool(), &f.identity, "Bob").await;
    let agent_id = create_agent(&f, &alice.participant, "助手", "k-a").await;

    // 群：Alice 建群并邀请 Bob；Alice 发两条消息（Alice 与 Bob 都会收到
    // delivery；Bob 的 joined + 两条消息 = 3 条，Alice = 2 条消息）。
    accept_contact(&f, &alice.participant, &bob).await;
    let group_id = create_group(&f, &alice.participant, "群", "g-k").await;
    invite(&f, &alice.participant, group_id.as_str(), &bob.id, "inv-b").await;
    invite(
        &f,
        &alice.participant,
        group_id.as_str(),
        &agent_id,
        "inv-a",
    )
    .await;
    send(&f.message, &alice.participant.id, &group_id, "m1", "m-1").await;
    send(&f.message, &alice.participant.id, &group_id, "m2", "m-2").await;

    let alice_events = f
        .human_events
        .list_events(&alice.participant.id, 0, 100)
        .await
        .expect("list alice");
    let bob_events = f
        .human_events
        .list_events(&bob.id, 0, 100)
        .await
        .expect("list bob");
    let alice_messages: Vec<_> = alice_events
        .iter()
        .filter(|e| e.fact.kind == HumanEventKind::MessageCreated)
        .collect();
    let bob_messages: Vec<_> = bob_events
        .iter()
        .filter(|e| e.fact.kind == HumanEventKind::MessageCreated)
        .collect();
    assert_eq!(alice_messages.len(), 2, "alice: 2 message.created");
    assert_eq!(bob_messages.len(), 2, "bob: joined + 2 message.created");

    // 同 Human cursor：解析回该 Human 自己的 delivery sequence（与
    // list_events 返回的内部 position 完全一致）。
    for item in alice_messages.iter() {
        let seq = f
            .human_events
            .resolve_event_after(&alice.participant.id, &item.fact.id)
            .await
            .expect("resolve")
            .expect("own delivery");
        assert_eq!(
            seq, item.sequence,
            "cursor resolves to the delivery position"
        );
    }

    // 跨 Human cursor：Alice 的 fact 在 Bob 的作用域下不可解析——但 Bob
    // 也收到了同一条 fact（room 内扇出），因此 Bob 的解析结果是 Bob 自己
    // 的 delivery sequence（作用域内合法位置）。
    let shared_fact = &alice_messages[0].fact;
    let bob_seq = f
        .human_events
        .resolve_event_after(&bob.id, &shared_fact.id)
        .await
        .expect("resolve")
        .expect("bob also received the same shared fact");
    assert!(bob_seq >= 1, "bob resolves the fact within his own scope");

    // 从未投递给该 Human 的 fact（例如另一个群只有 Carol 收到）→ None。
    let carol = add_human(f.store.pool(), &f.identity, "Carol").await;
    accept_contact(&f, &alice.participant, &carol).await;
    let group_2 = create_group(&f, &alice.participant, "群二", "g-k2").await;
    invite(&f, &alice.participant, group_2.as_str(), &carol.id, "inv-c").await;
    send(&f.message, &alice.participant.id, &group_2, "c-only", "m-c").await;
    let carol_events = f
        .human_events
        .list_events(&carol.id, 0, 100)
        .await
        .expect("list carol");
    assert_eq!(carol_events.len(), 2, "carol: joined + message");
    assert_eq!(
        f.human_events
            .resolve_event_after(&bob.id, &carol_events[1].fact.id)
            .await
            .expect("resolve"),
        None,
        "fact never delivered to bob must not resolve"
    );

    // 未知 fact ID：None（与从未投递形状一致，不泄露存在性）。
    let unknown = crate::domain::human_event::HumanEventFactId(Uuid::new_v4().to_string());
    assert_eq!(
        f.human_events
            .resolve_event_after(&alice.participant.id, &unknown)
            .await
            .expect("resolve"),
        None
    );
}
