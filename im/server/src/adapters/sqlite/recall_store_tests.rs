//! SQLite Message Recall adapter 测试：真实临时数据库上的撤回事实、窗口
//! 与时间唯一事实源、幂等/并发线性化、原子收敛（Human/Agent Recall Event
//! 与 created 快照变形）、失败回滚与连接复用、重启保持、数据库反向写入
//! 拒绝矩阵、delivery/target 继承与 reply/read-state 相邻契约。
//!
//! 边界时间通过受控 fixture 构造 created_at 相对 SQLite statement-time
//! （直接 SQL 插入 messages 行时用 `CAST(unixepoch('subsec')*1000 AS
//! INTEGER)` 表达同一时基），不在生产代码加入测试后门。

use std::sync::Arc;
use std::time::Duration;

use sqlx::{Connection, SqlitePool};
use uuid::Uuid;

use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::event_store::SqliteEventStore;
use crate::adapters::sqlite::human_event_store::SqliteHumanEventStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::message_store::SqliteMessageStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::room_store::SqliteRoomStore;
use crate::application::agent;
use crate::application::identity::bootstrap_human;
use crate::application::room as room_application;
use crate::domain::agent::IdempotencyKey;
use crate::domain::human_event::HumanEventKind;
use crate::domain::message::{MessageBody, MessageId};
use crate::domain::participant::{DisplayName, Participant, ParticipantId};
use crate::domain::room::{MembershipId, RoomId};
use crate::domain::time::UtcMillis;
use crate::ports::event::EventStore as _;
use crate::ports::human_event::HumanEventStore as _;
use crate::ports::message::{
    MessageStore as _, MessageStoreError, NewMessage, RecallOutcome, RecallStoreError,
    SendMessageOutcome,
};
use crate::ports::room::RoomStore as _;

struct Fixture {
    _dir: tempfile::TempDir,
    store: MigrationStore,
    pool: SqlitePool,
    identity: SqliteIdentityStore,
    agent: SqliteAgentStore,
    room: SqliteRoomStore,
    message: SqliteMessageStore,
    human_events: SqliteHumanEventStore,
    events: SqliteEventStore,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("open + migrate");
    let pool = store.pool().clone();
    Fixture {
        _dir: dir,
        pool: pool.clone(),
        identity: SqliteIdentityStore::new(pool.clone()),
        agent: SqliteAgentStore::new(pool.clone()),
        room: SqliteRoomStore::new(pool.clone()),
        message: SqliteMessageStore::new(pool.clone()),
        human_events: SqliteHumanEventStore::new(pool.clone()),
        events: SqliteEventStore::new(pool.clone()),
        store,
    }
}

/// 通过 schema 直接创建第二个 Human（非成员/后来加入者用）。
async fn add_human(pool: &SqlitePool, display_name: &str) -> Participant {
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

async fn create_room(f: &Fixture, human: &Participant, agent_id: &str, key: &str) -> RoomId {
    let outcome = room_application::create_direct_room(&f.room, human, agent_id, key)
        .await
        .expect("create direct room");
    let room_application::CreateDirectRoomOutcome::Created { room, .. } = outcome else {
        panic!("first room create must be Created");
    };
    room.id
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

async fn invite(
    f: &Fixture,
    owner: &Participant,
    group_id: &str,
    member: &ParticipantId,
    key: &str,
) {
    let outcome =
        room_application::invite_group_member(&f.room, owner, group_id, member.as_str(), key)
            .await
            .expect("invite member");
    let room_application::InviteMemberOutcome::Created { .. } = outcome else {
        panic!("invite must be Created");
    };
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

/// group 多 Mention 发送（规范排序后的 Mention 目标列表由存储层裁决）。
async fn send_mention(
    store: &SqliteMessageStore,
    actor_id: &ParticipantId,
    room_id: &RoomId,
    body: &str,
    key: &str,
    targets: &[&ParticipantId],
) -> SendMessageOutcome {
    let mut mentions: Vec<ParticipantId> = targets.iter().map(|t| (*t).clone()).collect();
    mentions.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    store
        .send_message(
            &NewMessage {
                id: MessageId::new_v4(),
                room_id: room_id.clone(),
                actor_id: actor_id.clone(),
                body: Some(MessageBody::parse(body).expect("test body valid")),
                mentions,
                attachments: Vec::new(),
                reply_to_message_id: None,
                created_at: UtcMillis::now(),
            },
            &IdempotencyKey::parse(key).expect("test key valid"),
        )
        .await
        .expect("send mention message")
}

fn message_of(outcome: &SendMessageOutcome) -> crate::domain::message::Message {
    match outcome {
        SendMessageOutcome::Created { message } | SendMessageOutcome::Replayed { message } => {
            message.clone()
        }
    }
}

async fn recall(
    store: &SqliteMessageStore,
    actor_id: &ParticipantId,
    room_id: &RoomId,
    message_id: &MessageId,
) -> Result<RecallOutcome, RecallStoreError> {
    store.recall_message(actor_id, room_id, message_id).await
}

/// 受控 fixture：直接 SQL 构建一个 group Room + owner Membership 周期，
/// joined_at 相对 SQLite statement-time 回拨 `base_offset_ms`，使后续以
/// 旧时点 created_at 插入的 Message 的 Human Event 投递守卫（joined_at <=
/// created_at）成立。同一事务内完成 room + coverage + membership 闭合
/// （coverage 的 deferred 复合外键在 COMMIT 时解析）。返回 Room ID。
async fn backdated_group_room(
    f: &Fixture,
    owner_id: &ParticipantId,
    base_offset_ms: i64,
) -> RoomId {
    let room_id = RoomId::new_v4();
    let membership_id = MembershipId::new_v4();
    let mut conn = f.store.pool().acquire().await.unwrap();
    let mut tx = conn.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO rooms (id, kind, status, created_at, name) \
         VALUES (?, 'group', 'active', ?, 'fixture')",
    )
    .bind(room_id.as_str())
    .bind(base_offset_ms)
    .execute(&mut *tx)
    .await
    .expect("insert fixture room");
    sqlx::query(
        "INSERT INTO room_coverage (room_id, kind, created_at, group_owner_membership_id) \
         VALUES (?, 'group', ?, ?)",
    )
    .bind(room_id.as_str())
    .bind(base_offset_ms)
    .bind(membership_id.as_str())
    .execute(&mut *tx)
    .await
    .expect("insert fixture coverage");
    sqlx::query(
        "INSERT INTO room_memberships \
         (membership_id, room_id, participant_id, role, joined_at, join_after_sequence) \
         VALUES (?, ?, ?, 'owner', ?, 0)",
    )
    .bind(membership_id.as_str())
    .bind(room_id.as_str())
    .bind(owner_id.as_str())
    .bind(base_offset_ms)
    .execute(&mut *tx)
    .await
    .expect("insert fixture membership");
    tx.commit().await.expect("commit fixture room");
    room_id
}

/// 直接 SQL 插入一条 created_at = SQLite statement-time + offset 的
/// Message fixture（窗口测试用：应用路径无法伪造时间，fixture 直接用同一
/// 时基表达 created_at）。满足 messages 全部守卫（active room/actor/
/// membership、唯一 key），触发器照常生成 Event/事实；backdated 房间保证
/// 投递守卫成立。返回 Message ID。
async fn insert_message_at_offset(
    f: &Fixture,
    room_id: &RoomId,
    actor_id: &ParticipantId,
    body: &str,
    key: &str,
    created_at_offset_ms: i64,
) -> String {
    let id = Uuid::new_v4().to_string();
    sqlx::query(&format!(
        "INSERT INTO messages \
         (id, room_id, sequence, actor_id, body, reply_to_message_id, idempotency_key, created_at) \
         VALUES (?, ?, (SELECT COALESCE(MAX(sequence), 0) + 1 FROM messages WHERE room_id = ?), \
                 ?, ?, NULL, ?, \
                 CAST(unixepoch('subsec') * 1000 AS INTEGER) + {created_at_offset_ms})"
    ))
    .bind(&id)
    .bind(room_id.as_str())
    .bind(room_id.as_str())
    .bind(actor_id.as_str())
    .bind(body)
    .bind(key)
    .execute(f.store.pool())
    .await
    .expect("insert raw message fixture");
    id
}

fn now_ms() -> i64 {
    UtcMillis::now().millis()
}

/// 读取 message_recalls 表全部行。
async fn recall_rows(f: &Fixture) -> Vec<(String, String, i64)> {
    sqlx::query_as(
        "SELECT message_id, recalled_by_participant_id, recalled_at \
                    FROM message_recalls ORDER BY message_id",
    )
    .fetch_all(f.store.pool())
    .await
    .unwrap()
}

/// 读取一条 Message 的 created 快照（原始 JSON 文本）。
async fn created_snapshot_of(f: &Fixture, message_id: &str) -> String {
    sqlx::query_scalar(
        "SELECT snapshot FROM human_event_facts \
         WHERE kind = 'message.created' AND source_id = ?",
    )
    .bind(message_id)
    .fetch_one(f.store.pool())
    .await
    .unwrap()
}

// ─────────────────────────── 撤回事实与幂等 ───────────────────────────

#[tokio::test]
async fn recall_first_time_creates_single_fact_with_statement_time() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &room_id, "hello", "m-1").await);

    let before = now_ms();
    let outcome = recall(&f.message, &alice.participant.id, &room_id, &msg.id)
        .await
        .expect("recall within window");
    let after = now_ms();

    assert!(!outcome.replayed, "first recall must be Created");
    assert!(
        outcome.recalled_at.millis() >= before && outcome.recalled_at.millis() <= after,
        "recalled_at must be SQLite statement-time within the request window: {} !in [{before}, {after}]",
        outcome.recalled_at.millis()
    );

    let rows = recall_rows(&f).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, msg.id.as_str());
    assert_eq!(rows[0].1, alice.participant.id.as_str());
    assert_eq!(rows[0].2, outcome.recalled_at.millis());

    // messages 行不改不删：body/sequence 保留（内部事实）。
    let (body, sequence): (String, i64) =
        sqlx::query_as("SELECT body, sequence FROM messages WHERE id = ?")
            .bind(msg.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(body, "hello");
    assert_eq!(sequence, 1);
}

#[tokio::test]
async fn recall_replay_returns_same_recalled_at_and_adds_nothing() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &room_id, "hello", "m-1").await);

    let first = recall(&f.message, &alice.participant.id, &room_id, &msg.id)
        .await
        .expect("first recall");
    assert!(!first.replayed);
    // 重复撤回：幂等成功、recalled_at 不刷新、无新事实。
    let second = recall(&f.message, &alice.participant.id, &room_id, &msg.id)
        .await
        .expect("replay must succeed");
    assert!(second.replayed);
    assert_eq!(second.recalled_at, first.recalled_at);
    assert_eq!(recall_rows(&f).await.len(), 1);
    let facts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM human_event_facts")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    // created + member.joined x2 + recall（重复撤回不新增）。
    assert_eq!(facts, 4, "replay must not create new facts");
}

#[tokio::test]
async fn recall_rejects_unknown_cross_room_and_foreign_actor() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(&f.pool, "Bob").await;
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_a = create_room(&f, &alice.participant, agent_id.as_str(), "r-a").await;
    let agent_b = create_agent(&f, &alice.participant, "二号", "b-k").await;
    let room_b = create_room(&f, &alice.participant, agent_b.as_str(), "r-b").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &room_a, "hello", "m-1").await);

    // unknown message。
    assert!(matches!(
        recall(
            &f.message,
            &alice.participant.id,
            &room_a,
            &MessageId::new_v4()
        )
        .await,
        Err(RecallStoreError::MessageNotFound)
    ));
    // 跨 room：消息在 room_a，从 room_b 撤回。
    assert!(matches!(
        recall(&f.message, &alice.participant.id, &room_b, &msg.id).await,
        Err(RecallStoreError::MessageNotFound)
    ));
    // 非成员（Bob 不在 room_a）→ RoomNotFound 先于消息裁决。
    assert!(matches!(
        recall(&f.message, &bob.id, &room_a, &msg.id).await,
        Err(RecallStoreError::RoomNotFound)
    ));
    // 同一 group 中非作者的 active 成员 → MessageNotFound（统一，不泄露
    // 归属）。
    accept_contact(&f, &alice.participant, &bob).await;
    let group = create_group(&f, &alice.participant, "群", "g-k").await;
    invite(&f, &alice.participant, group.as_str(), &bob.id, "inv-b").await;
    let group_msg =
        message_of(&send(&f.message, &alice.participant.id, &group, "hello", "g-1").await);
    assert!(matches!(
        recall(&f.message, &bob.id, &group, &group_msg.id).await,
        Err(RecallStoreError::MessageNotFound)
    ));
    // 无副作用：没有产生任何撤回事实。
    assert!(recall_rows(&f).await.is_empty());
}

#[tokio::test]
async fn recall_requires_active_membership_before_message_adjudication() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(&f.pool, "Bob").await;
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &room_id, "hello", "m-1").await);

    // 非成员 → RoomNotFound（先于消息裁决，与发送/读取同一 404 面）。
    assert!(matches!(
        recall(&f.message, &bob.id, &room_id, &msg.id).await,
        Err(RecallStoreError::RoomNotFound)
    ));
    // bad room id → RoomNotFound。
    assert!(matches!(
        recall(&f.message, &bob.id, &RoomId::new_v4(), &msg.id).await,
        Err(RecallStoreError::RoomNotFound)
    ));
}

#[tokio::test]
async fn recall_window_is_judged_against_sqlite_statement_time() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    // 受控 fixture：房间与成员周期回拨 400s，使旧时点消息的事实投递守卫成立。
    let room_id = backdated_group_room(&f, &alice.participant.id, -400_000).await;

    // 窗口内（created_at = now - 60s）：成立。
    let fresh_id = insert_message_at_offset(
        &f,
        &room_id,
        &alice.participant.id,
        "fresh",
        "w-fresh",
        -60_000,
    )
    .await;
    let outcome = recall(
        &f.message,
        &alice.participant.id,
        &room_id,
        &MessageId(fresh_id.clone()),
    )
    .await
    .expect("within-window recall must succeed");
    assert!(!outcome.replayed);

    // 迟到（created_at = now - 130s）：首次撤回拒绝，无任何新事实。
    let stale_id = insert_message_at_offset(
        &f,
        &room_id,
        &alice.participant.id,
        "stale",
        "w-stale",
        -130_000,
    )
    .await;
    assert!(matches!(
        recall(
            &f.message,
            &alice.participant.id,
            &room_id,
            &MessageId(stale_id.clone())
        )
        .await,
        Err(RecallStoreError::WindowExpired)
    ));
    let facts_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM human_event_facts")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(
        recall_rows(&f).await.len(),
        1,
        "expired recall adds no fact"
    );
    let facts_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM human_event_facts")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(
        facts_before, facts_after,
        "expired recall must not converge anything"
    );

    // 未来 created_at（now + 60s）：首次撤回同样拒绝（now < created_at）。
    let future_id = insert_message_at_offset(
        &f,
        &room_id,
        &alice.participant.id,
        "future",
        "w-future",
        60_000,
    )
    .await;
    assert!(matches!(
        recall(
            &f.message,
            &alice.participant.id,
            &room_id,
            &MessageId(future_id)
        )
        .await,
        Err(RecallStoreError::WindowExpired)
    ));

    // 已撤回的重复撤回不受窗口限制：边界消息 M1 恰等撤回后，等待窗口
    // 过去再重放仍幂等成功（先读既有事实，不重算、不重跑窗口裁决）。
    let boundary_room = backdated_group_room(&f, &alice.participant.id, -130_000).await;
    let m1 = Uuid::new_v4().to_string();
    let mut conn = f.store.pool().acquire().await.expect("conn");
    sqlx::query(
        "CREATE TEMP TRIGGER recall_boundary_fixture \
         AFTER INSERT ON messages \
         BEGIN \
             INSERT INTO message_recalls (message_id, recalled_by_participant_id, recalled_at) \
             SELECT NEW.id, NEW.actor_id, CAST(unixepoch('subsec') * 1000 AS INTEGER) \
             FROM messages m \
             WHERE m.id = NEW.id \
               AND NEW.created_at = CAST(unixepoch('subsec') * 1000 AS INTEGER) - 120000; \
         END",
    )
    .execute(&mut *conn)
    .await
    .expect("create boundary fixture trigger");
    sqlx::query(
        "INSERT INTO messages \
         (id, room_id, sequence, actor_id, body, reply_to_message_id, idempotency_key, created_at) \
         VALUES (?, ?, (SELECT COALESCE(MAX(sequence), 0) + 1 FROM messages WHERE room_id = ?), \
                 ?, 'boundary', NULL, 'w-boundary', \
                 CAST(unixepoch('subsec') * 1000 AS INTEGER) - 120000)",
    )
    .bind(&m1)
    .bind(boundary_room.as_str())
    .bind(boundary_room.as_str())
    .bind(alice.participant.id.as_str())
    .execute(&mut *conn)
    .await
    .expect("insert boundary message");
    sqlx::query("DROP TRIGGER recall_boundary_fixture")
        .execute(&mut *conn)
        .await
        .expect("drop fixture trigger");
    drop(conn);
    assert_eq!(
        recall_rows(&f).await.len(),
        2,
        "boundary recall at exact equality accepted"
    );

    // 等待窗口过去（> 1ms），再重放：已撤回的重复撤回不受窗口限制。
    tokio::time::sleep(Duration::from_millis(50)).await;
    let replay = recall(
        &f.message,
        &alice.participant.id,
        &boundary_room,
        &MessageId(m1),
    )
    .await
    .expect("repeat recall outside window must replay");
    assert!(replay.replayed);
    // 对照：同一时点（窗口已过）对另一条未撤回消息的**首次**撤回仍被拒。
    let m2 = insert_message_at_offset(
        &f,
        &boundary_room,
        &alice.participant.id,
        "expired-now",
        "w-expired",
        -120_000,
    )
    .await;
    assert!(matches!(
        recall(
            &f.message,
            &alice.participant.id,
            &boundary_room,
            &MessageId(m2)
        )
        .await,
        Err(RecallStoreError::WindowExpired)
    ));
}

/// 窗口边界「恰等成立」：用同 statement 内建的临时触发器把 created_at 与
/// recalled_at 对齐到同一 SQLite statement-time（created_at = now - 120000，
/// recall 恰在 now = created_at + 120000），证明 guard 的 `<=` 是包含的。
#[tokio::test]
async fn recall_window_exact_boundary_equality_is_accepted() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    // 受控 fixture：房间与成员周期回拨 130s，使边界消息（created_at =
    // now - 120s）的事实投递守卫成立。
    let room_id = backdated_group_room(&f, &alice.participant.id, -130_000).await;
    let msg_id = Uuid::new_v4().to_string();

    // 受控 fixture：messages AFTER INSERT 触发器在**同一 statement**内，
    // 以同一 statement-time 插入恰等边界的 message_recalls。临时触发器
    // 必须与 message INSERT 在同一连接上创建（temp schema 按连接隔离）。
    let mut conn = f.store.pool().acquire().await.expect("conn");
    sqlx::query(
        "CREATE TEMP TRIGGER recall_boundary_fixture \
         AFTER INSERT ON messages \
         BEGIN \
             INSERT INTO message_recalls (message_id, recalled_by_participant_id, recalled_at) \
             SELECT NEW.id, NEW.actor_id, CAST(unixepoch('subsec') * 1000 AS INTEGER) \
             FROM messages m \
             WHERE m.id = NEW.id \
               AND NEW.created_at = CAST(unixepoch('subsec') * 1000 AS INTEGER) - 120000; \
         END",
    )
    .execute(&mut *conn)
    .await
    .expect("create boundary fixture trigger");

    sqlx::query(
        "INSERT INTO messages \
         (id, room_id, sequence, actor_id, body, reply_to_message_id, idempotency_key, created_at) \
         VALUES (?, ?, (SELECT COALESCE(MAX(sequence), 0) + 1 FROM messages WHERE room_id = ?), \
                 ?, 'boundary', NULL, 'w-boundary', \
                 CAST(unixepoch('subsec') * 1000 AS INTEGER) - 120000)",
    )
    .bind(&msg_id)
    .bind(room_id.as_str())
    .bind(room_id.as_str())
    .bind(alice.participant.id.as_str())
    .execute(&mut *conn)
    .await
    .expect("insert boundary message");

    sqlx::query("DROP TRIGGER recall_boundary_fixture")
        .execute(&mut *conn)
        .await
        .expect("drop fixture trigger");
    drop(conn);

    let rows = recall_rows(&f).await;
    assert_eq!(
        rows.len(),
        1,
        "boundary recall at now = created_at + 120000 must be accepted"
    );
    let (m_id, _, recalled_at) = &rows[0];
    assert_eq!(m_id, &msg_id);
    let created_at: i64 = sqlx::query_scalar("SELECT created_at FROM messages WHERE id = ?")
        .bind(&msg_id)
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(*recalled_at, created_at + 120000, "exact boundary equality");
}

#[tokio::test]
async fn concurrent_recalls_linearize_to_exactly_one_fact() {
    let f = fixture().await;
    let f = Arc::new(f);
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &room_id, "hello", "m-1").await);

    let mut handles = Vec::new();
    for _ in 0..8 {
        let f = f.clone();
        let actor = alice.participant.id.clone();
        let room = room_id.clone();
        let mid = msg.id.clone();
        handles.push(tokio::spawn(async move {
            recall(&f.message, &actor, &room, &mid).await
        }));
    }
    let mut outcomes = Vec::new();
    for handle in handles {
        outcomes.push(
            handle
                .await
                .expect("recall task joined")
                .expect("recall ok"),
        );
    }
    let rows = recall_rows(&f).await;
    assert_eq!(rows.len(), 1, "exactly one MessageRecall fact");
    let (_, _, recalled_at) = rows[0];
    for outcome in &outcomes {
        assert_eq!(
            outcome.recalled_at.millis(),
            recalled_at,
            "all responses must converge to the single recalled_at"
        );
    }
    let created_count = outcomes.iter().filter(|o| !o.replayed).count();
    assert_eq!(created_count, 1, "exactly one caller becomes the creator");
}

#[tokio::test]
async fn cancelled_recall_rolls_back_and_connection_stays_reusable() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &room_id, "hello", "m-1").await);

    // 取消路径：spawn 一个可能被立即 abort 的撤回任务。BEGIN IMMEDIATE 由
    // SQLx Transaction 持有生命周期，任务取消自动回滚，不能把未结束事务
    // 带回连接池。无论 abort 落在事务中（回滚、无事实）还是任务已先完成
    // （事实已提交），最终都收敛为恰好一条事实且同一 pool 可复用。
    let f_arc = Arc::new(f);
    let actor = alice.participant.id.clone();
    let room = room_id.clone();
    let mid = msg.id.clone();
    let f_inner = f_arc.clone();
    let cancel_handle = tokio::spawn(async move {
        let handle =
            tokio::spawn(async move { recall(&f_inner.message, &actor, &room, &mid).await });
        // 立即取消：最大化命中「事务已启动、尚未提交」的窗口。
        handle.abort();
        let _ = handle.await;
    });
    cancel_handle.await.unwrap();

    // 同一 pool 继续完成撤回：已取消（回滚）则首次成立，未取消则幂等
    // 重放——两者都收敛到恰好一条事实。
    let outcome = recall(&f_arc.message, &alice.participant.id, &room_id, &msg.id)
        .await
        .expect("pool still usable after cancellation");
    assert!(outcome.recalled_at.millis() > 0);
    assert_eq!(recall_rows(&f_arc).await.len(), 1);
}

#[tokio::test]
async fn recall_persists_across_restart_with_converged_facts() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("core.db");
    let first = MigrationStore::open(&db_path).await.expect("first open");
    let pool = first.pool().clone();
    let identity = SqliteIdentityStore::new(pool.clone());
    let agent_store = SqliteAgentStore::new(pool.clone());
    let room_store = SqliteRoomStore::new(pool.clone());
    let message_store = SqliteMessageStore::new(pool.clone());
    let alice = bootstrap_human(&identity, "Alice").await.unwrap();
    let agent_id = agent::create_agent(&agent_store, &alice.participant, "助手", "a-k")
        .await
        .expect("create agent");
    let agent::CreateAgentOutcome::Created { agent, .. } = agent_id else {
        panic!("created")
    };
    let room = room_application::create_direct_room(
        &room_store,
        &alice.participant,
        agent.id.as_str(),
        "r-k",
    )
    .await
    .expect("create room");
    let room_application::CreateDirectRoomOutcome::Created { room, .. } = room else {
        panic!("created")
    };
    let msg = message_of(
        &send(
            &message_store,
            &alice.participant.id,
            &room.id,
            "hello",
            "m-1",
        )
        .await,
    );
    let outcome = recall(&message_store, &alice.participant.id, &room.id, &msg.id)
        .await
        .expect("recall");
    assert!(!outcome.replayed);
    drop(first);

    // 重启：同一数据库文件重新打开，事实与收敛保持。
    let second = MigrationStore::open(&db_path).await.expect("reopen");
    let pool = second.pool().clone();
    let message_store = SqliteMessageStore::new(pool.clone());
    let (recalled_at,): (i64,) =
        sqlx::query_as("SELECT recalled_at FROM message_recalls WHERE message_id = ?")
            .bind(msg.id.as_str())
            .fetch_one(&pool)
            .await
            .expect("recall fact survives restart");
    assert_eq!(recalled_at, outcome.recalled_at.millis());
    let replay = recall(&message_store, &alice.participant.id, &room.id, &msg.id)
        .await
        .expect("replay after restart");
    assert!(replay.replayed);
    assert_eq!(replay.recalled_at.millis(), recalled_at);

    // 收敛事实保持：created 快照变形 + Human/Agent recall 事实。
    let snapshot: String = sqlx::query_scalar(
        "SELECT snapshot FROM human_event_facts WHERE kind = 'message.created' AND source_id = ?",
    )
    .bind(msg.id.as_str())
    .fetch_one(&pool)
    .await
    .expect("created snapshot survives restart");
    let snap: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
    assert!(snap["body"].is_null());
    assert_eq!(snap["recalled_at"].as_i64(), Some(recalled_at));
    let recall_facts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM human_event_facts WHERE kind = 'message.recalled'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(recall_facts, 1);
    let agent_kinds: Vec<String> =
        sqlx::query_scalar("SELECT kind FROM agent_events WHERE message_id = ? ORDER BY sequence")
            .bind(msg.id.as_str())
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(agent_kinds, vec!["message.created", "message.recalled"]);
}

#[tokio::test]
async fn recall_keeps_foreign_key_check_clean() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &room_id, "hello", "m-1").await);
    recall(&f.message, &alice.participant.id, &room_id, &msg.id)
        .await
        .expect("recall");
    let violations: Vec<(String, String, String)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(f.store.pool())
        .await
        .unwrap();
    assert!(
        violations.is_empty(),
        "foreign_key_check must be clean after recall: {violations:?}"
    );
}

// ─────────────────────── 原子收敛（Human/Agent/快照）─────────────────────

#[tokio::test]
async fn recall_atomically_converges_events_and_snapshot_in_direct_room() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &room_id, "hello", "m-1").await);

    let created_snapshot_before = created_snapshot_of(&f, msg.id.as_str()).await;
    let created_snapshot_before: serde_json::Value =
        serde_json::from_str(&created_snapshot_before).unwrap();
    assert_eq!(created_snapshot_before["body"], "hello");
    assert!(
        created_snapshot_before["recalled_at"].is_null(),
        "created snapshot must固化 recalled_at: null"
    );
    assert!(
        created_snapshot_before.get("mentions").is_none(),
        "created snapshot 恒不含 mentions"
    );
    assert!(
        created_snapshot_before.get("sequence").is_none(),
        "created snapshot 不含 sequence"
    );

    let outcome = recall(&f.message, &alice.participant.id, &room_id, &msg.id)
        .await
        .expect("recall");
    let recalled_at = outcome.recalled_at.millis();

    // Human fact message.recalled：最小快照 message_id/room_id/recalled_at。
    let (fact_id, created_at, room, snapshot): (String, i64, String, String) = sqlx::query_as(
        "SELECT fact_id, created_at, room_id, snapshot FROM human_event_facts \
         WHERE kind = 'message.recalled' AND source_id = ?",
    )
    .bind(msg.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .expect("recalled fact exists");
    assert_eq!(
        created_at, recalled_at,
        "recalled fact created_at = recalled_at"
    );
    assert_eq!(room, msg.room_id.as_str());
    let snap: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
    assert_eq!(
        snap,
        serde_json::json!({
            "message_id": msg.id.as_str(),
            "room_id": msg.room_id.as_str(),
            "recalled_at": recalled_at,
        })
    );

    // delivery：投递给 message.created 既有集合（direct room 的 Human
    // 成员——发送者本人）。
    let deliveries: Vec<String> =
        sqlx::query_scalar("SELECT d.human_id FROM human_event_deliveries d WHERE d.fact_id = ?")
            .bind(&fact_id)
            .fetch_all(f.store.pool())
            .await
            .unwrap();
    assert_eq!(deliveries, vec![alice.participant.id.as_str().to_owned()]);

    // created 快照单向变形：body=null、recalled_at=首次时点、其余键保留。
    let deformed: serde_json::Value =
        serde_json::from_str(&created_snapshot_of(&f, msg.id.as_str()).await).unwrap();
    assert!(deformed["body"].is_null());
    assert_eq!(deformed["recalled_at"].as_i64(), Some(recalled_at));
    assert_eq!(deformed["id"], created_snapshot_before["id"]);
    assert_eq!(deformed["room_id"], created_snapshot_before["room_id"]);
    assert_eq!(deformed["actor"], created_snapshot_before["actor"]);
    assert_eq!(
        deformed["reply_to_message_id"],
        created_snapshot_before["reply_to_message_id"]
    );
    assert_eq!(
        deformed["created_at"],
        created_snapshot_before["created_at"]
    );

    // Agent event message.recalled：target = created 同一集合，kind 列，
    // per-Agent sequence 跨 kind 共享（created=1, recalled=2）。
    let events: Vec<(String, String, String, i64, i64)> = sqlx::query_as(
        "SELECT id, kind, target_agent_id, sequence, created_at FROM agent_events \
         WHERE message_id = ? ORDER BY sequence",
    )
    .bind(msg.id.as_str())
    .fetch_all(f.store.pool())
    .await
    .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].1, "message.created");
    assert_eq!(events[0].2, agent_id.as_str());
    assert_eq!(events[0].3, 1);
    assert_eq!(events[0].4, msg.created_at.millis());
    assert_eq!(events[1].1, "message.recalled");
    assert_eq!(events[1].2, agent_id.as_str());
    assert_eq!(events[1].3, 2, "sequence shared across kinds");
    assert_eq!(
        events[1].4, recalled_at,
        "recalled event created_at = recalled_at"
    );
}

#[tokio::test]
async fn agent_recall_of_own_reply_generates_no_agent_recall_event() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &room_id, "hello", "m-1").await);
    let _ = &msg;
    // Agent 自己以成员身份 say 一条（原走 Completion reply 写回；
    // ⚠ 2026-09-28：Completion/ACK 整体退役后改走 say）。
    let said = send(&f.message, &agent_id, &room_id, "echo: hello", "reply-k-1").await;
    let reply_message_id = message_of(&said).id.clone();
    let agent_events_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_events")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(
        agent_events_before, 1,
        "agent reply generates no created event"
    );

    // Agent 经 application 用例撤回自己的 reply Message：201 语义。
    let outcome = recall(&f.message, &agent_id, &room_id, &reply_message_id)
        .await
        .expect("agent recalls own reply");
    assert!(!outcome.replayed);
    let rows = recall_rows(&f).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].1,
        agent_id.as_str(),
        "recalled_by = the agent author"
    );

    // 不凭空生成 Agent Recall Event（reply 没有 created target）。
    let agent_events_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_events")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(agent_events_before, agent_events_after);

    // Human 侧收敛照常：reply 的 created 快照变形 + message.recalled 事实。
    let snap: serde_json::Value =
        serde_json::from_str(&created_snapshot_of(&f, reply_message_id.as_str()).await).unwrap();
    assert!(snap["body"].is_null());
    let recalled_facts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM human_event_facts WHERE kind = 'message.recalled'",
    )
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(recalled_facts, 1);

    // ⚠ 2026-09-28：原还断言「既有 Completion 终态保留（撤回不级联）」
    // ——`completions` 表已随 Completion/ACK 整体退役删除。
}

#[tokio::test]
async fn agent_cannot_recall_human_message_and_vice_versa() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &room_id, "hello", "m-1").await);

    // Agent 撤回 Human 消息：author 不匹配 → MessageNotFound。
    assert!(matches!(
        recall(&f.message, &agent_id, &room_id, &msg.id).await,
        Err(RecallStoreError::MessageNotFound)
    ));
    assert!(recall_rows(&f).await.is_empty());

    // Human 撤回 Agent 说的话（say）：author 不匹配 → MessageNotFound。
    // ⚠ 2026-09-28：原走 Completion reply 写回；退役后改走 say。
    let said = send(&f.message, &agent_id, &room_id, "echo: hello", "reply-k-1").await;
    let reply_message_id = message_of(&said).id.clone();
    assert!(matches!(
        recall(
            &f.message,
            &alice.participant.id,
            &room_id,
            &reply_message_id
        )
        .await,
        Err(RecallStoreError::MessageNotFound)
    ));
    assert_eq!(recall_rows(&f).await.len(), 0);
}

#[tokio::test]
async fn failed_recall_leaves_no_partial_facts() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let room_id = backdated_group_room(&f, &alice.participant.id, -400_000).await;
    let stale_id = insert_message_at_offset(
        &f,
        &room_id,
        &alice.participant.id,
        "stale",
        "w-stale",
        -130_000,
    )
    .await;

    let snapshot_before = created_snapshot_of(&f, &stale_id).await;
    let facts_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM human_event_facts")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    let events_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_events")
        .fetch_one(f.store.pool())
        .await
        .unwrap();

    assert!(matches!(
        recall(
            &f.message,
            &alice.participant.id,
            &room_id,
            &MessageId(stale_id.clone())
        )
        .await,
        Err(RecallStoreError::WindowExpired)
    ));

    assert_eq!(recall_rows(&f).await.len(), 0, "no recall fact on failure");
    let facts_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM human_event_facts")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    let events_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_events")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(facts_before, facts_after, "failed recall must not converge");
    assert_eq!(events_before, events_after);
    assert_eq!(
        created_snapshot_of(&f, &stale_id).await,
        snapshot_before,
        "snapshot unchanged"
    );
}

// ──────────────────────── 数据库反向写入（fail closed）────────────────────

/// 执行预期被数据库拒绝的写入，返回数据库错误原文；意外成功则 panic。
async fn rejected(pool: &SqlitePool, sql: &str) -> String {
    let err = sqlx::query(sql).execute(pool).await.expect_err(&format!(
        "write must be rejected by the database, got Ok for: {sql}"
    ));
    match err {
        sqlx::Error::Database(e) => e.message().to_owned(),
        other => format!("{other}"),
    }
}

#[tokio::test]
async fn db_guard_rejects_forged_recall_writes() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(&f.pool, "Bob").await;
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &room_id, "hello", "m-1").await);
    let msg_id = msg.id.as_str().to_owned();

    // 代撤：recalled_by != actor。
    let err = rejected(
        &f.pool,
        &format!(
            "INSERT INTO message_recalls (message_id, recalled_by_participant_id, recalled_at) \
             VALUES ('{msg_id}', '{}', CAST(unixepoch('subsec') * 1000 AS INTEGER))",
            bob.id.as_str()
        ),
    )
    .await;
    assert!(
        err.contains("message recall requires the author"),
        "proxy recall must be rejected: {err}"
    );

    // 显式伪造 recalled_at（旧值 / 未来值 / 任意值）一律拒绝。
    for forged in [now_ms() - 999999, now_ms() + 999999, 42] {
        let err = rejected(
            &f.pool,
            &format!(
                "INSERT INTO message_recalls (message_id, recalled_by_participant_id, recalled_at) \
                 VALUES ('{msg_id}', '{}', {forged})",
                alice.participant.id.as_str()
            ),
        )
        .await;
        assert!(
            err.contains("message recall requires the author"),
            "forged recalled_at {forged} must be rejected: {err}"
        );
    }

    // 迟到：显式 statement-time 表达但窗口外（created_at = now - 130s）。
    let stale_room = backdated_group_room(&f, &alice.participant.id, -400_000).await;
    let stale_id = insert_message_at_offset(
        &f,
        &stale_room,
        &alice.participant.id,
        "stale",
        "w-stale",
        -130_000,
    )
    .await;
    let err = rejected(
        &f.pool,
        &format!(
            "INSERT INTO message_recalls (message_id, recalled_by_participant_id, recalled_at) \
             SELECT '{stale_id}', '{}', CAST(unixepoch('subsec') * 1000 AS INTEGER)",
            alice.participant.id.as_str()
        ),
    )
    .await;
    assert!(
        err.contains("message recall requires the author"),
        "late direct-SQL recall must be rejected: {err}"
    );

    // 合法直接 SQL（完全满足同一不变量：作者 + active + 窗口 + statement-time）
    // 可以成立——guard 不依赖应用层，只依赖不变量。
    sqlx::query(&format!(
        "INSERT INTO message_recalls (message_id, recalled_by_participant_id, recalled_at) \
         SELECT '{msg_id}', '{}', CAST(unixepoch('subsec') * 1000 AS INTEGER) \
         WHERE '{msg_id}' NOT IN (SELECT message_id FROM message_recalls)",
        alice.participant.id.as_str()
    ))
    .execute(f.store.pool())
    .await
    .expect("invariant-satisfying direct SQL must be accepted");
    assert_eq!(recall_rows(&f).await.len(), 1);

    // 重复普通 INSERT / INSERT OR REPLACE / REPLACE / UPDATE / DELETE。
    for sql in [
        format!(
            "INSERT INTO message_recalls (message_id, recalled_by_participant_id, recalled_at) \
             VALUES ('{msg_id}', '{}', CAST(unixepoch('subsec') * 1000 AS INTEGER))",
            alice.participant.id.as_str()
        ),
        format!(
            "INSERT OR REPLACE INTO message_recalls (message_id, recalled_by_participant_id, recalled_at) \
             VALUES ('{msg_id}', '{}', CAST(unixepoch('subsec') * 1000 AS INTEGER))",
            alice.participant.id.as_str()
        ),
        format!(
            "REPLACE INTO message_recalls (message_id, recalled_by_participant_id, recalled_at) \
             VALUES ('{msg_id}', '{}', CAST(unixepoch('subsec') * 1000 AS INTEGER))",
            alice.participant.id.as_str()
        ),
        format!(
            "UPDATE message_recalls SET recalled_at = recalled_at + 1 WHERE message_id = '{msg_id}'"
        ),
        format!(
            "UPDATE message_recalls SET recalled_by_participant_id = '{}' WHERE message_id = '{msg_id}'",
            bob.id.as_str()
        ),
        format!("DELETE FROM message_recalls WHERE message_id = '{msg_id}'"),
    ] {
        let err = rejected(&f.pool, &sql).await;
        assert!(
            err.contains("message recall identity already exists")
                || err.contains("message recalls are immutable")
                || err.contains("message recalls cannot be deleted")
                || err.contains("message recall requires the author"),
            "write must be rejected: {err}"
        );
    }
    // 原事实不变。
    let rows = recall_rows(&f).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1, alice.participant.id.as_str());
}

#[tokio::test]
async fn db_guard_rejects_recalled_agent_event_forgery() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &room_id, "hello", "m-1").await);
    let msg_id = msg.id.as_str().to_owned();
    let now_expr = "CAST(unixepoch('subsec') * 1000 AS INTEGER)";

    // 无 message_recalls 见证的 message.recalled 直接 INSERT → 拒绝。
    let err = rejected(
        &f.pool,
        &format!(
            "INSERT INTO agent_events (id, kind, target_agent_id, sequence, message_id, created_at) \
             VALUES ('{}', 'message.recalled', '{}', 2, '{msg_id}', {now_expr})",
            Uuid::new_v4(),
            agent_id.as_str()
        ),
    )
    .await;
    assert!(
        err.contains("agent event requires a paired human message"),
        "recalled event without witness must be rejected: {err}"
    );

    // 有 recall 见证但 target 不在 created 集合（另一个 Agent）→ 拒绝。
    let other = create_agent(&f, &alice.participant, "另个", "o-k").await;
    recall(&f.message, &alice.participant.id, &room_id, &msg.id)
        .await
        .expect("recall");
    let recalled_at: i64 =
        sqlx::query_scalar("SELECT recalled_at FROM message_recalls WHERE message_id = ?")
            .bind(&msg_id)
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    let err = rejected(
        &f.pool,
        &format!(
            "INSERT INTO agent_events (id, kind, target_agent_id, sequence, message_id, created_at) \
             VALUES ('{}', 'message.recalled', '{}', 1, '{msg_id}', {recalled_at})",
            Uuid::new_v4(),
            other.as_str()
        ),
    )
    .await;
    assert!(
        err.contains("agent event requires a paired human message"),
        "recalled event for non-created target must be rejected: {err}"
    );

    // created_at != recalled_at 的 message.recalled → 拒绝。
    let err = rejected(
        &f.pool,
        &format!(
            "INSERT INTO agent_events (id, kind, target_agent_id, sequence, message_id, created_at) \
             VALUES ('{}', 'message.recalled', '{}', 3, '{msg_id}', {recalled_at} + 1)",
            Uuid::new_v4(),
            agent_id.as_str()
        ),
    )
    .await;
    assert!(
        err.contains("agent event requires a paired human message"),
        "recalled event with wrong created_at must be rejected: {err}"
    );
}

#[tokio::test]
async fn db_guard_rejects_recalled_fact_forgery_and_wrong_room() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &room_id, "hello", "m-1").await);
    let msg_id = msg.id.as_str().to_owned();
    let now_expr = "CAST(unixepoch('subsec') * 1000 AS INTEGER)";

    // 无 message_recalls 见证的 message.recalled fact 直接 INSERT → 拒绝。
    let err = rejected(
        &f.pool,
        &format!(
            "INSERT INTO human_event_facts (fact_id, kind, source_id, room_id, created_at, snapshot) \
             VALUES ('{}', 'message.recalled', '{msg_id}', '{}', {now_expr}, \
                     json_object('message_id', '{msg_id}', 'recalled_at', {now_expr}))",
            Uuid::new_v4(),
            room_id.as_str()
        ),
    )
    .await;
    assert!(
        err.contains("human event fact must reference a consistent source fact"),
        "recalled fact without witness must be rejected: {err}"
    );

    // 有见证但 room 不匹配 → 拒绝。
    recall(&f.message, &alice.participant.id, &room_id, &msg.id)
        .await
        .expect("recall");
    let recalled_at: i64 =
        sqlx::query_scalar("SELECT recalled_at FROM message_recalls WHERE message_id = ?")
            .bind(&msg_id)
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    let err = rejected(
        &f.pool,
        &format!(
            "INSERT INTO human_event_facts (fact_id, kind, source_id, room_id, created_at, snapshot) \
             VALUES ('{}', 'message.recalled', '{msg_id}', '{}', {recalled_at}, \
                     json_object('message_id', '{msg_id}', 'recalled_at', {recalled_at}))",
            Uuid::new_v4(),
            RoomId::new_v4().as_str()
        ),
    )
    .await;
    assert!(
        err.contains("human event fact must reference a consistent source fact"),
        "recalled fact with wrong room must be rejected: {err}"
    );
}

#[tokio::test]
async fn db_guard_rejects_snapshot_deformation_violations() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &room_id, "hello", "m-1").await);
    let msg_id = msg.id.as_str().to_owned();

    // 无 recall 见证的变形 → 拒绝。
    let err = rejected(
        &f.pool,
        &format!(
            "UPDATE human_event_facts SET snapshot = json_set(snapshot, '$.body', NULL) \
             WHERE kind = 'message.created' AND source_id = '{msg_id}'"
        ),
    )
    .await;
    assert!(
        err.contains("human event facts are immutable"),
        "unwitnessed deformation must be rejected: {err}"
    );

    recall(&f.message, &alice.participant.id, &room_id, &msg.id)
        .await
        .expect("recall");
    let recalled_at: i64 =
        sqlx::query_scalar("SELECT recalled_at FROM message_recalls WHERE message_id = ?")
            .bind(&msg_id)
            .fetch_one(f.store.pool())
            .await
            .unwrap();

    // 反向恢复（tombstone → active）→ 拒绝。
    let err = rejected(
        &f.pool,
        &format!(
            "UPDATE human_event_facts SET snapshot = json_set(snapshot, '$.body', 'restored') \
             WHERE kind = 'message.created' AND source_id = '{msg_id}'"
        ),
    )
    .await;
    assert!(
        err.contains("human event facts are immutable"),
        "reverse restore must be rejected: {err}"
    );

    // 二次变形（换成不同的 recalled_at，真实改变值）→ 拒绝（OLD.body 已
    // 是 null；等值自赋值的同值 UPDATE 属既有「等值自赋值」例外，见下文）。
    let err = rejected(
        &f.pool,
        &format!(
            "UPDATE human_event_facts SET snapshot = json_set(snapshot, '$.body', NULL, '$.recalled_at', {}) \
             WHERE kind = 'message.created' AND source_id = '{msg_id}'",
            recalled_at + 1
        ),
    )
    .await;
    assert!(
        err.contains("human event facts are immutable"),
        "second deformation must be rejected: {err}"
    );

    // 改动其他键（body=null 但 id 被改）→ 拒绝。
    let err = rejected(
        &f.pool,
        &format!(
            "UPDATE human_event_facts SET snapshot = json_set(snapshot, '$.body', NULL, '$.id', 'forged') \
             WHERE kind = 'message.created' AND source_id = '{msg_id}'"
        ),
    )
    .await;
    assert!(
        err.contains("human event facts are immutable"),
        "key tampering during deformation must be rejected: {err}"
    );

    // 错误的 recalled_at（不等于 witness）→ 拒绝。
    let err = rejected(
        &f.pool,
        &format!(
            "UPDATE human_event_facts SET snapshot = json_set(snapshot, '$.body', NULL, '$.recalled_at', {}) \
             WHERE kind = 'message.created' AND source_id = '{msg_id}'",
            recalled_at + 1
        ),
    )
    .await;
    assert!(
        err.contains("human event facts are immutable"),
        "wrong recalled_at must be rejected: {err}"
    );

    // 其他 fact（member.joined 等）与已变形的 created 快照：UPDATE 一律拒绝。
    let err = rejected(
        &f.pool,
        "UPDATE human_event_facts SET snapshot = json_set(snapshot, '$.body', NULL) \
         WHERE kind = 'member.joined'",
    )
    .await;
    assert!(
        err.contains("human event facts are immutable"),
        "other kind snapshot update must be rejected: {err}"
    );
    let err = rejected(&f.pool, "DELETE FROM human_event_facts").await;
    assert!(
        err.contains("human event facts cannot be deleted"),
        "fact delete must be rejected: {err}"
    );

    // 等值自赋值 UPDATE 不触发 immutable 守卫（既有规则接受）。
    sqlx::query(&format!(
        "UPDATE human_event_facts SET snapshot = snapshot WHERE kind = 'message.created' AND source_id = '{msg_id}'"
    ))
    .execute(f.store.pool())
    .await
    .expect("self-assignment update must be accepted");
}

/// 已证实守卫缺陷回归（"extra key 绕过"）：human_event_facts_immutable 曾只
/// 逐键比较 5 个固定键 + recalled_at 见证，允许携带 witness 的 active 快照被
/// 一次性改写为 body=null、recalled_at=witness 同时**增加多余键/重排 shape**
/// 的变造 tombstone（如 json_set(..., '$.forged_extra', 'survives')）。唯一
/// 允许的新快照必须与 OLD 经唯一变换
///   json_set(OLD.snapshot, '$.body', NULL, '$.recalled_at', witness)
/// 的完整 JSON 精确相等。
///
/// 守卫直击构造：真实迁移里 message_recalls INSERT 与 created 快照变形在同
/// 一收敛触发器内原子完成（witness ∧ active 快照不可兼得），等价于独立最小
/// 实验的前提是"撤销收敛"——本测试仅临时 DROP 收敛触发器后以合法直接 SQL
/// 建立 witness，使守卫面对最不利输入（witness 在、OLD 仍 active）；被测的
/// human_event_facts_immutable 与其余全部触发器保持真实迁移原样。
#[tokio::test]
async fn db_guard_rejects_forged_tombstone_shapes_even_with_witness_present() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &room_id, "hello", "m-1").await);
    let msg_id = msg.id.as_str().to_owned();
    let active_snapshot = created_snapshot_of(&f, &msg_id).await;
    let active: serde_json::Value = serde_json::from_str(&active_snapshot).unwrap();
    assert!(
        active["body"].is_string(),
        "fixture snapshot must be active"
    );

    // 撤销收敛（仅本测试的守卫直击构造）：此后 message_recalls INSERT 不再
    // 触发快照变形，witness 与 active 快照可并存。
    sqlx::query("DROP TRIGGER message_recalls_after_insert_converges")
        .execute(f.store.pool())
        .await
        .expect("drop convergence trigger");

    // 合法直接 SQL 撤回（作者 + active 周期 + 窗口 + statement-time 全满足）
    // → 产生 message_recalls witness。
    sqlx::query(&format!(
        "INSERT INTO message_recalls (message_id, recalled_by_participant_id, recalled_at) \
         SELECT '{msg_id}', '{}', CAST(unixepoch('subsec') * 1000 AS INTEGER)",
        alice.participant.id.as_str()
    ))
    .execute(f.store.pool())
    .await
    .expect("legit direct recall must establish the witness");
    let recalled_at: i64 =
        sqlx::query_scalar("SELECT recalled_at FROM message_recalls WHERE message_id = ?")
            .bind(&msg_id)
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(
        created_snapshot_of(&f, &msg_id).await,
        active_snapshot,
        "without convergence the created snapshot must stay active beside the witness"
    );

    // 1) 已证实绕过向量：变形同时增加多余键（$.forged_extra）→ 拒绝。
    let err = rejected(
        &f.pool,
        &format!(
            "UPDATE human_event_facts \
             SET snapshot = json_set(snapshot, '$.body', NULL, '$.recalled_at', {recalled_at}, '$.forged_extra', 'survives') \
             WHERE kind = 'message.created' AND source_id = '{msg_id}'"
        ),
    )
    .await;
    assert!(
        err.contains("human event facts are immutable"),
        "extra key alongside the deformation must be rejected: {err}"
    );

    // 2) 删键：变形同时删除其他键 → 拒绝。
    let err = rejected(
        &f.pool,
        &format!(
            "UPDATE human_event_facts \
             SET snapshot = json_set(json_remove(snapshot, '$.created_at'), '$.body', NULL, '$.recalled_at', {recalled_at}) \
             WHERE kind = 'message.created' AND source_id = '{msg_id}'"
        ),
    )
    .await;
    assert!(
        err.contains("human event facts are immutable"),
        "deleted key must be rejected: {err}"
    );

    // 3) actor 嵌套篡改：嵌套对象内部值改动 → 拒绝。
    let err = rejected(
        &f.pool,
        &format!(
            "UPDATE human_event_facts \
             SET snapshot = json_set(snapshot, '$.actor.display_name', 'HACKED', '$.body', NULL, '$.recalled_at', {recalled_at}) \
             WHERE kind = 'message.created' AND source_id = '{msg_id}'"
        ),
    )
    .await;
    assert!(
        err.contains("human event facts are immutable"),
        "actor nested tampering must be rejected: {err}"
    );

    // 4) 键重排成不同 shape（逐键值全等）→ 拒绝。
    let err = rejected(
        &f.pool,
        &format!(
            "UPDATE human_event_facts \
             SET snapshot = json_object('recalled_at', {recalled_at}, 'body', NULL, \
                 'id', json_extract(snapshot, '$.id'), \
                 'room_id', json_extract(snapshot, '$.room_id'), \
                 'actor', json_extract(snapshot, '$.actor'), \
                 'reply_to_message_id', json_extract(snapshot, '$.reply_to_message_id'), \
                 'created_at', json_extract(snapshot, '$.created_at')) \
             WHERE kind = 'message.created' AND source_id = '{msg_id}'"
        ),
    )
    .await;
    assert!(
        err.contains("human event facts are immutable"),
        "key reordering into a different shape must be rejected: {err}"
    );

    // 5) 正控制：唯一合法的完整 JSON 变换仍被接受，且结果逐字等于
    //    json_set(OLD, '$.body', NULL, '$.recalled_at', witness)。
    sqlx::query(&format!(
        "UPDATE human_event_facts \
         SET snapshot = json_set(snapshot, '$.body', NULL, '$.recalled_at', {recalled_at}) \
         WHERE kind = 'message.created' AND source_id = '{msg_id}'"
    ))
    .execute(f.store.pool())
    .await
    .expect("the unique exact deformation must remain accepted");
    let deformed: String = created_snapshot_of(&f, &msg_id).await;
    let expected: String =
        sqlx::query_scalar("SELECT json_set(?, '$.body', NULL, '$.recalled_at', ?)")
            .bind(&active_snapshot)
            .bind(recalled_at)
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(
        deformed, expected,
        "the accepted deformation must equal the unique json_set transformation byte-for-byte"
    );
}

#[tokio::test]
async fn db_guard_rejects_recalled_delivery_deviation() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(&f.pool, "Bob").await;
    let charlie = add_human(&f.pool, "Charlie").await;
    accept_contact(&f, &alice.participant, &bob).await;
    accept_contact(&f, &alice.participant, &charlie).await;
    let group = create_group(&f, &alice.participant, "群", "g-k").await;
    invite(&f, &alice.participant, group.as_str(), &bob.id, "inv-b").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &group, "hi", "g-1").await);
    let msg_id = msg.id.as_str().to_owned();
    // 撤回前 Charlie 加入（后来加入者）。
    invite(&f, &alice.participant, group.as_str(), &charlie.id, "inv-c").await;

    recall(&f.message, &alice.participant.id, &group, &msg.id)
        .await
        .expect("recall");
    let (recalled_fact_id,): (String,) = sqlx::query_as(
        "SELECT fact_id FROM human_event_facts WHERE kind = 'message.recalled' AND source_id = ?",
    )
    .bind(&msg_id)
    .fetch_one(f.store.pool())
    .await
    .unwrap();

    // 后来加入者 Charlie：给 recalled fact 投递 → 拒绝（不在 created 集合）。
    let err = rejected(
        &f.pool,
        &format!(
            "INSERT INTO human_event_deliveries (human_id, sequence, fact_id) \
             VALUES ('{}', (SELECT COALESCE(MAX(sequence), 0) + 1 FROM human_event_deliveries d WHERE d.human_id = '{}'), '{recalled_fact_id}')",
            charlie.id.as_str(),
            charlie.id.as_str()
        ),
    )
    .await;
    assert!(
        err.contains("human event delivery requires a membership period")
            || err.contains("human event delivery identity already exists"),
        "deviation delivery to later joiner must be rejected: {err}"
    );
}

// ─────────────────────── Human delivery/target 继承 ─────────────────────

#[tokio::test]
async fn recalled_deliveries_copy_created_set_including_left_members() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(&f.pool, "Bob").await;
    let charlie = add_human(&f.pool, "Charlie").await;
    accept_contact(&f, &alice.participant, &bob).await;
    accept_contact(&f, &alice.participant, &charlie).await;
    let group = create_group(&f, &alice.participant, "群", "g-k").await;
    invite(&f, &alice.participant, group.as_str(), &bob.id, "inv-b").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &group, "hi", "g-1").await);
    let msg_id = msg.id.as_str().to_owned();

    // 撤回时点：Bob 已离群（created 收件人集合包含 Bob，且 he has delivery）。
    room_application::leave_membership(&f.room, &bob.id, group.as_str())
        .await
        .expect("bob leaves");
    // Charlie 在撤回前加入（created 集合之外）。
    invite(&f, &alice.participant, group.as_str(), &charlie.id, "inv-c").await;

    recall(&f.message, &alice.participant.id, &group, &msg.id)
        .await
        .expect("recall");

    // created 既有 delivery 集合 = {alice, bob}（按 human_id 排序）。
    let mut expected_created: Vec<String> = vec![
        alice.participant.id.as_str().to_owned(),
        bob.id.as_str().to_owned(),
    ];
    expected_created.sort();
    let created_deliveries: Vec<String> = sqlx::query_scalar(
        "SELECT d.human_id FROM human_event_facts f \
         JOIN human_event_deliveries d ON d.fact_id = f.fact_id \
         WHERE f.kind = 'message.created' AND f.source_id = ? ORDER BY d.human_id",
    )
    .bind(&msg_id)
    .fetch_all(f.store.pool())
    .await
    .unwrap();
    assert_eq!(created_deliveries, expected_created);

    // recalled delivery 严格等于 created 集合：Bob（已离群）仍被投递、
    // Charlie（后来加入者）不被投递。
    let recalled_deliveries: Vec<String> = sqlx::query_scalar(
        "SELECT d.human_id FROM human_event_facts f \
         JOIN human_event_deliveries d ON d.fact_id = f.fact_id \
         WHERE f.kind = 'message.recalled' AND f.source_id = ? ORDER BY d.human_id",
    )
    .bind(&msg_id)
    .fetch_all(f.store.pool())
    .await
    .unwrap();
    assert_eq!(
        recalled_deliveries, created_deliveries,
        "recalled deliveries must copy the created set exactly (left member included, later joiner excluded)"
    );

    // 读取期过滤：Bob 离群期间读不到任何事件；Alice（active）读得到
    // message.recalled。
    let alice_events = list_for(&f, &alice.participant.id, 0).await;
    assert!(
        alice_events.iter().any(|e| {
            e.fact.kind == HumanEventKind::MessageRecalled && e.fact.source_id == msg_id
        }),
        "active human must read the recalled event"
    );
    let bob_events = list_for(&f, &bob.id, 0).await;
    assert!(
        !bob_events
            .iter()
            .any(|e| e.fact.kind == HumanEventKind::MessageRecalled && e.fact.source_id == msg_id),
        "left member must not read while out (delivery exists but filtered)"
    );
    assert!(
        !bob_events
            .iter()
            .any(|e| e.fact.kind == HumanEventKind::MessageCreated),
        "left member also cannot read the old created event while out"
    );
}

#[tokio::test]
async fn left_member_reads_back_delivered_recall_after_rejoin() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(&f.pool, "Bob").await;
    accept_contact(&f, &alice.participant, &bob).await;
    let group = create_group(&f, &alice.participant, "群", "g-k").await;
    invite(&f, &alice.participant, group.as_str(), &bob.id, "inv-b").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &group, "hi", "g-1").await);
    let msg_id = msg.id.as_str().to_owned();
    room_application::leave_membership(&f.room, &bob.id, group.as_str())
        .await
        .expect("bob leaves");
    recall(&f.message, &alice.participant.id, &group, &msg.id)
        .await
        .expect("recall");

    // Bob 重入：对既有旧消息按原窗口可见（join_after_sequence 之后的消息
    // 可读回），已投递的 recall 恢复可读。
    invite(&f, &alice.participant, group.as_str(), &bob.id, "inv-b2").await;
    let bob_events = list_for(&f, &bob.id, 0).await;
    let recalled = bob_events
        .iter()
        .find(|e| e.fact.kind == HumanEventKind::MessageRecalled && e.fact.source_id == msg_id)
        .expect("rejoined member must read back the delivered recall");
    assert_eq!(recalled.fact.created_at.millis(), {
        let (recalled_at,): (i64,) =
            sqlx::query_as("SELECT recalled_at FROM message_recalls WHERE message_id = ?")
                .bind(&msg_id)
                .fetch_one(f.store.pool())
                .await
                .unwrap();
        recalled_at
    });
}

async fn list_for(
    f: &Fixture,
    human_id: &ParticipantId,
    after: i64,
) -> Vec<crate::ports::human_event::HumanEventWithFact> {
    f.human_events
        .list_events(human_id, after, 100)
        .await
        .expect("list human events")
}

// ─────────────────────────── Agent target 复制 ─────────────────────────

#[tokio::test]
async fn group_multimention_recall_copies_created_targets_per_agent_sequences() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_a = create_agent(&f, &alice.participant, "A", "a-k").await;
    let agent_b = create_agent(&f, &alice.participant, "B", "b-k").await;
    let group = create_group(&f, &alice.participant, "群", "g-k").await;
    invite(&f, &alice.participant, group.as_str(), &agent_a, "inv-a").await;
    invite(&f, &alice.participant, group.as_str(), &agent_b, "inv-b").await;
    let msg = message_of(
        &send_mention(
            &f.message,
            &alice.participant.id,
            &group,
            "ping",
            "g-1",
            &[&agent_a, &agent_b],
        )
        .await,
    );
    let msg_id = msg.id.as_str().to_owned();

    // 撤回前每个 Agent 的 sequence 都是 1（各自只有一条 created）。
    let seqs: Vec<(String, i64)> = sqlx::query_as(
        "SELECT target_agent_id, sequence FROM agent_events \
         WHERE message_id = ? ORDER BY target_agent_id",
    )
    .bind(&msg_id)
    .fetch_all(f.store.pool())
    .await
    .unwrap();
    assert_eq!(seqs.len(), 2);

    recall(&f.message, &alice.participant.id, &group, &msg.id)
        .await
        .expect("recall");

    // 每条 created target 各生成一条 recalled，per-Agent sequence 跨 kind
    // 共享、严格 next：各自 {created=1, recalled=2}。
    for agent in [&agent_a, &agent_b] {
        let kinds: Vec<(String, i64)> = sqlx::query_as(
            "SELECT kind, sequence FROM agent_events \
             WHERE message_id = ? AND target_agent_id = ? ORDER BY sequence",
        )
        .bind(&msg_id)
        .bind(agent.as_str())
        .fetch_all(f.store.pool())
        .await
        .unwrap();
        assert_eq!(
            kinds,
            vec![
                ("message.created".to_owned(), 1),
                ("message.recalled".to_owned(), 2),
            ],
            "per-agent sequence shared across kinds for agent {}",
            agent.as_str()
        );
    }

    // cursor 无重复无跳过：sequence > 1 只返回 recalled。
    let page = f
        .events
        .list_events(&agent_a, 1, 100)
        .await
        .expect("list events")
        .events;
    assert_eq!(page.len(), 1);
    assert_eq!(
        page[0].event.kind,
        crate::domain::agent_event::AgentEventKind::Recalled
    );
    assert_eq!(page[0].event.message_id.as_str(), msg_id);
}

#[tokio::test]
async fn agent_created_late_pull_keeps_tombstone_marker_and_original_never_regenerated() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &room_id, "secret", "m-1").await);

    // 撤回发生在 Agent 首次拉取之前。
    recall(&f.message, &alice.participant.id, &room_id, &msg.id)
        .await
        .expect("recall");

    // 迟拉：created 事件仍存在（不可变事实），但 Message 带 recalled_at
    // tombstone 标记——公共视图层据此投影 body=null，绝不回放原正文。
    let page = f
        .events
        .list_events(&agent_id, 0, 100)
        .await
        .expect("list events")
        .events;
    assert_eq!(page.len(), 2, "created + recalled both remain");
    assert_eq!(
        page[0].event.kind,
        crate::domain::agent_event::AgentEventKind::Created
    );
    assert!(
        page[0].message.recalled_at.is_some(),
        "created view must carry the tombstone marker"
    );
    assert_eq!(page[0].event.created_at, msg.created_at);
    assert_eq!(
        page[1].event.kind,
        crate::domain::agent_event::AgentEventKind::Recalled
    );
    assert_eq!(
        page[1].message.recalled_at, page[0].message.recalled_at,
        "both kinds project the same tombstone"
    );
}

// ───────────────────────── Completion 两种先后 ─────────────────────────
// ──────────────────────── reply 相邻契约 ─────────────────────────

#[tokio::test]
async fn reply_to_recalled_target_rejected_and_existing_reply_preserved() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let target =
        message_of(&send(&f.message, &alice.participant.id, &room_id, "target", "m-1").await);

    // 撤回前：合法 reply 成立（已有 reply 引用保留的前提）。
    let reply = f
        .message
        .send_message(
            &NewMessage {
                id: MessageId::new_v4(),
                room_id: room_id.clone(),
                actor_id: alice.participant.id.clone(),
                body: Some(MessageBody::parse("reply").unwrap()),
                mentions: Vec::new(),
                attachments: Vec::new(),
                reply_to_message_id: Some(target.id.clone()),
                created_at: UtcMillis::now(),
            },
            &IdempotencyKey::parse("m-2").expect("key"),
        )
        .await
        .expect("reply before recall");
    let reply = message_of(&reply);

    recall(&f.message, &alice.participant.id, &room_id, &target.id)
        .await
        .expect("recall target");

    // 撤回后新建回复该 target：应用层同一 invalid_reply_target，不泄露
    // 是否因撤回；无副作用。
    let err = f
        .message
        .send_message(
            &NewMessage {
                id: MessageId::new_v4(),
                room_id: room_id.clone(),
                actor_id: alice.participant.id.clone(),
                body: Some(MessageBody::parse("late reply").unwrap()),
                mentions: Vec::new(),
                attachments: Vec::new(),
                reply_to_message_id: Some(target.id.clone()),
                created_at: UtcMillis::now(),
            },
            &IdempotencyKey::parse("m-3").expect("key"),
        )
        .await
        .expect_err("reply to recalled target must be rejected");
    assert!(
        matches!(err, MessageStoreError::InvalidReplyTarget),
        "uniform invalid_reply_target"
    );

    // 数据库独立兜底：直接 SQL INSERT 携带已撤回 target → 拒绝。
    let err = rejected(
        &f.pool,
        &format!(
            "INSERT INTO messages \
             (id, room_id, sequence, actor_id, body, reply_to_message_id, idempotency_key, created_at) \
             VALUES ('{}', '{}', (SELECT COALESCE(MAX(sequence), 0) + 1 FROM messages WHERE room_id = '{}'), \
                     '{}', 'db-reply', '{}', 'db-k', 99)",
            Uuid::new_v4(),
            room_id.as_str(),
            room_id.as_str(),
            alice.participant.id.as_str(),
            target.id.as_str()
        ),
    )
    .await;
    assert!(
        err.contains("reply target must be a same-room historical message"),
        "db guard must reject reply to recalled target: {err}"
    );

    // 已有 reply 保留：reply 的 reply_to_message_id 仍是 target ID。
    let (reply_target,): (Option<String>,) =
        sqlx::query_as("SELECT reply_to_message_id FROM messages WHERE id = ?")
            .bind(reply.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(reply_target.as_deref(), Some(target.id.as_str()));

    // History 投影：target 的 recalled_at 标记存在（视图层投影 tombstone）。
    let page = f
        .message
        .list_messages(&alice.participant.id, &room_id, 100, None)
        .await
        .expect("list")
        .expect("visible");
    let target_view = page
        .messages
        .iter()
        .find(|m| m.message.id == target.id)
        .expect("target in history");
    assert!(
        target_view.message.recalled_at.is_some(),
        "tombstone marker in history"
    );
    let reply_view = page
        .messages
        .iter()
        .find(|m| m.message.id == reply.id)
        .expect("reply in history");
    assert_eq!(
        reply_view.message.reply_to_message_id,
        Some(target.id.clone()),
        "existing reply keeps its target"
    );
    assert!(
        reply_view.message.recalled_at.is_none(),
        "reply itself unrecalled"
    );
}

#[tokio::test]
async fn send_replay_of_recalled_message_compares_internal_facts_and_keeps_tombstone_marker() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &room_id, "secret", "m-1").await);
    recall(&f.message, &alice.participant.id, &room_id, &msg.id)
        .await
        .expect("recall");

    // 撤回后幂等重放仍按原 body 内部事实比较（同 key 同请求 → 重放成功），
    // 但 Message 事实带 recalled_at tombstone 标记（视图层不得泄原正文）。
    let replay = f
        .message
        .send_message(
            &NewMessage {
                id: msg.id.clone(),
                room_id: room_id.clone(),
                actor_id: alice.participant.id.clone(),
                body: msg.body.clone(),
                mentions: msg.mentions.clone(),
                attachments: Vec::new(),
                reply_to_message_id: msg.reply_to_message_id.clone(),
                created_at: msg.created_at,
            },
            &IdempotencyKey::parse("m-1").expect("key"),
        )
        .await
        .expect("replay after recall");
    assert!(matches!(replay, SendMessageOutcome::Replayed { .. }));
    let replayed = message_of(&replay);
    assert_eq!(
        replayed.body, msg.body,
        "internal fact comparison uses original body"
    );
    assert!(
        replayed.recalled_at.is_some(),
        "replay view carries tombstone marker"
    );
    assert_eq!(recall_rows(&f).await.len(), 1, "replay adds no recall fact");
}

#[tokio::test]
async fn read_cursor_can_advance_to_tombstone_and_unread_is_not_recomputed() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(&f.pool, "Bob").await;
    accept_contact(&f, &alice.participant, &bob).await;
    let group = create_group(&f, &alice.participant, "群", "g-k").await;
    invite(&f, &alice.participant, group.as_str(), &bob.id, "inv-b").await;
    let msg = message_of(&send(&f.message, &alice.participant.id, &group, "hi", "g-1").await);

    // Bob 的未读 = 1（Alice 的消息）；撤回前 cursor 尚未推进。
    let (unread,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM messages m \
         WHERE m.room_id = ? AND m.sequence > \
             (SELECT c.read_sequence FROM read_cursors c \
              JOIN room_memberships mm ON mm.membership_id = c.membership_id \
              WHERE c.room_id = ? AND c.participant_id = ? AND mm.active_marker = 1) \
           AND EXISTS (SELECT 1 FROM room_memberships w \
                       WHERE w.room_id = m.room_id AND w.participant_id = ? \
                         AND w.join_after_sequence < m.sequence \
                         AND (w.left_before_sequence IS NULL OR m.sequence <= w.left_before_sequence)) \
           AND m.actor_id <> ?",
    )
    .bind(group.as_str())
    .bind(group.as_str())
    .bind(bob.id.as_str())
    .bind(bob.id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(unread, 1);

    recall(&f.message, &alice.participant.id, &group, &msg.id)
        .await
        .expect("recall");
    // 撤回不回算未读：tombstone 仍按原 actor/sequence 占位计数。
    let (unread_after,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM messages m \
         WHERE m.room_id = ? AND m.sequence > \
             (SELECT c.read_sequence FROM read_cursors c \
              JOIN room_memberships mm ON mm.membership_id = c.membership_id \
              WHERE c.room_id = ? AND c.participant_id = ? AND mm.active_marker = 1) \
           AND EXISTS (SELECT 1 FROM room_memberships w \
                       WHERE w.room_id = m.room_id AND w.participant_id = ? \
                         AND w.join_after_sequence < m.sequence \
                         AND (w.left_before_sequence IS NULL OR m.sequence <= w.left_before_sequence)) \
           AND m.actor_id <> ?",
    )
    .bind(group.as_str())
    .bind(group.as_str())
    .bind(bob.id.as_str())
    .bind(bob.id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(unread_after, 1, "recall must not recompute unread");

    // read cursor 仍可推进到 tombstone。
    let outcome = f
        .room
        .advance_read_cursor(&bob.id, &group, &msg.id)
        .await
        .expect("advance cursor onto tombstone");
    let crate::ports::room::AdvanceReadCursorOutcome::Advanced { .. } = outcome else {
        panic!("cursor must advance");
    };
    // 推进后未读为 0（不回算不倒退，正常语义）。
    let (unread_final,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM messages m \
         WHERE m.room_id = ? AND m.sequence > \
             (SELECT c.read_sequence FROM read_cursors c \
              JOIN room_memberships mm ON mm.membership_id = c.membership_id \
              WHERE c.room_id = ? AND c.participant_id = ? AND mm.active_marker = 1) \
           AND EXISTS (SELECT 1 FROM room_memberships w \
                       WHERE w.room_id = m.room_id AND w.participant_id = ? \
                         AND w.join_after_sequence < m.sequence \
                         AND (w.left_before_sequence IS NULL OR m.sequence <= w.left_before_sequence)) \
           AND m.actor_id <> ?",
    )
    .bind(group.as_str())
    .bind(group.as_str())
    .bind(bob.id.as_str())
    .bind(bob.id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(f.store.pool())
    .await
    .unwrap();
    assert_eq!(unread_final, 0);
}
