//! SQLite Agent Event adapter 测试：真实临时数据库上的原子生成（无 Binding
//! 也生成）、Message+Event 同事务回滚、每 Agent sequence 从 1 单调唯一、
//! 幂等/失败无副作用、读取分页与 Agent 隔离、数据库反向写入拒绝矩阵、
//! 撤销不改写 Event 与 schema 精确集合。（旧库升级保数据那条承诺已退役：
//! 迁移治理只认全新数据库与重复启动，见下方删除注记。）

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Connection, SqlitePool};
use uuid::Uuid;

use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::backend_store::SqliteBackendStore;
use crate::adapters::sqlite::event_store::SqliteEventStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::message_store::SqliteMessageStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::room_store::SqliteRoomStore;
use crate::application::agent;
use crate::application::identity::bootstrap_human;
use crate::application::room as room_application;
use crate::domain::agent::IdempotencyKey;
use crate::domain::backend::{BackendToken, BindingId, CredentialId};
use crate::domain::message::{MessageBody, MessageId};
use crate::domain::participant::{Participant, ParticipantId};
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::ports::backend::{BackendStore as _, NewBinding, NewCredential};
use crate::ports::event::{EventPage, EventStore as _};
use crate::ports::message::{MessageStore as _, NewMessage, SendMessageOutcome};

struct Fixture {
    _dir: tempfile::TempDir,
    store: MigrationStore,
    identity: SqliteIdentityStore,
    agent: SqliteAgentStore,
    room: SqliteRoomStore,
    message: SqliteMessageStore,
    event: SqliteEventStore,
    backend: SqliteBackendStore,
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
        event: SqliteEventStore::new(pool.clone()),
        backend: SqliteBackendStore::new(pool.clone()),
        store,
    }
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

/// 通过公开用例创建 Direct Room，返回 Room ID。
async fn create_room(f: &Fixture, human: &Participant, agent_id: &str, key: &str) -> RoomId {
    let outcome = room_application::create_direct_room(&f.room, human, agent_id, key)
        .await
        .expect("create direct room");
    let room_application::CreateDirectRoomOutcome::Created { room, .. } = outcome else {
        panic!("first room create must be Created");
    };
    room.id
}

/// 通过 Message 端口提交一条 Human 消息，返回提交事实。
async fn send(
    store: &SqliteMessageStore,
    actor_id: &ParticipantId,
    room_id: &RoomId,
    body: &str,
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
                reply_to_message_id: None,
                created_at: UtcMillis::now(),
            },
            &IdempotencyKey::parse(key).expect("test key valid"),
        )
        .await
}

fn message_of(outcome: &SendMessageOutcome) -> &crate::domain::message::Message {
    match outcome {
        SendMessageOutcome::Created { message } | SendMessageOutcome::Replayed { message } => {
            message
        }
    }
}

async fn count_events(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM agent_events")
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

async fn count_events_for(f: &Fixture, agent_id: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM agent_events WHERE target_agent_id = ?")
        .bind(agent_id)
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

async fn event_sequences_for(f: &Fixture, agent_id: &str) -> Vec<i64> {
    sqlx::query_scalar(
        "SELECT sequence FROM agent_events WHERE target_agent_id = ? ORDER BY sequence ASC",
    )
    .bind(agent_id)
    .fetch_all(f.store.pool())
    .await
    .unwrap()
}

async fn list_events(f: &Fixture, agent_id: &str, after: i64, limit: u32) -> EventPage {
    f.event
        .list_events(&ParticipantId(agent_id.to_owned()), after, limit)
        .await
        .expect("list events")
}

/// 直接向 agent_events 写入（反向写入探针），返回数据库错误原文。
async fn insert_event_raw(
    pool: &SqlitePool,
    id: &str,
    target_agent_id: &str,
    sequence: i64,
    message_id: &str,
    created_at: i64,
) -> Result<(), String> {
    match sqlx::query(
        "INSERT INTO agent_events (id, target_agent_id, sequence, message_id, created_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(target_agent_id)
    .bind(sequence)
    .bind(message_id)
    .bind(created_at)
    .execute(pool)
    .await
    {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(e)) => Err(e.message().to_owned()),
        Err(e) => Err(format!("{e}")),
    }
}

#[tokio::test]
async fn new_human_message_generates_exactly_one_event_without_binding() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    let sent = send(&f.message, &alice.participant.id, &room_id, "hello", "m-1")
        .await
        .expect("send");
    let message = message_of(&sent);
    assert_eq!(message.sequence, 1);

    assert_eq!(
        count_events(&f).await,
        1,
        "exactly one event without binding"
    );
    let (id, target, sequence, msg_id, created_at): (String, String, i64, String, i64) =
        sqlx::query_as(
            "SELECT id, target_agent_id, sequence, message_id, created_at FROM agent_events",
        )
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(target, agent_id.as_str());
    assert_eq!(msg_id, message.id.as_str());
    assert_eq!(sequence, 1);
    assert_eq!(
        created_at,
        message.created_at.millis(),
        "created_at must equal source message"
    );
    let uuid = Uuid::parse_str(&id).expect("event id must be a UUID");
    assert_eq!(
        uuid.get_version(),
        Some(uuid::Version::Random),
        "event id must be v4"
    );
    assert_eq!(id, id.to_lowercase());
}

#[tokio::test]
async fn idempotent_replay_does_not_generate_second_event() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    let first = send(&f.message, &alice.participant.id, &room_id, "hello", "m-1")
        .await
        .expect("first send");
    let replay = send(&f.message, &alice.participant.id, &room_id, "hello", "m-1")
        .await
        .expect("replay");
    assert!(matches!(replay, SendMessageOutcome::Replayed { .. }));

    assert_eq!(count_events(&f).await, 1, "replay must not add an event");
    let (id, message_id, sequence): (String, String, i64) =
        sqlx::query_as("SELECT id, message_id, sequence FROM agent_events")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(sequence, 1);
    assert_eq!(message_id, message_of(&first).id.as_str());
    assert_eq!(
        Uuid::parse_str(&id).unwrap().get_version(),
        Some(uuid::Version::Random)
    );
}

#[tokio::test]
async fn conflict_and_failed_sends_produce_no_events() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    send(&f.message, &alice.participant.id, &room_id, "hello", "m-1")
        .await
        .expect("first send");

    // 同 key 换 body → 冲突，无新增 Event。
    let err = send(
        &f.message,
        &alice.participant.id,
        &room_id,
        "changed",
        "m-1",
    )
    .await
    .expect_err("conflict");
    assert!(matches!(
        err,
        crate::ports::message::MessageStoreError::IdempotencyConflict
    ));

    // 非法正文在应用层拒绝；存储层用空/超长正文直接证明无副作用。
    for (i, bad) in ["", &"a".repeat(16385)].iter().enumerate() {
        let result = sqlx::query(
            "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, 0)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(room_id.as_str())
        .bind((i + 2) as i64)
        .bind(alice.participant.id.as_str())
        .bind(*bad)
        .bind(format!("bad-{i}"))
        .execute(f.store.pool())
        .await;
        assert!(result.is_err(), "DB must reject the bad body");
    }

    // 未知 Room → 404，无 Message 无 Event。
    let ghost = RoomId::new_v4();
    let err = send(&f.message, &alice.participant.id, &ghost, "x", "ghost-k")
        .await
        .expect_err("unknown room");
    assert!(matches!(
        err,
        crate::ports::message::MessageStoreError::NotFound
    ));

    assert_eq!(count_messages(&f).await, 1);
    assert_eq!(count_events(&f).await, 1, "failed attempts leave no event");
}

async fn count_messages(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn agent_actor_messages_do_not_generate_events() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    // Agent 走同一提交内核：消息成功但不生成 Event（actor 不是配对 Human）。
    let sent = send(&f.message, &agent_id, &room_id, "agent memo", "g-k")
        .await
        .expect("agent actor send");
    assert!(matches!(sent, SendMessageOutcome::Created { .. }));
    assert_eq!(
        count_events(&f).await,
        0,
        "agent actor must not generate an event"
    );

    // Human 随后正常生成自己的 Event（sequence 从 1）。
    send(
        &f.message,
        &alice.participant.id,
        &room_id,
        "human msg",
        "h-k",
    )
    .await
    .expect("human send");
    assert_eq!(event_sequences_for(&f, agent_id.as_str()).await, vec![1]);
}

#[tokio::test]
async fn revoked_target_does_not_generate_event_but_message_commits() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    send(&f.message, &alice.participant.id, &room_id, "before", "m-1")
        .await
        .expect("send before revoke");
    agent::revoke_agent(&f.agent, &alice.participant.id, agent_id.as_str())
        .await
        .expect("revoke agent");

    // Agent revoked：Human 消息仍提交（既有契约），但不生成 Event。
    let after = send(&f.message, &alice.participant.id, &room_id, "after", "m-2")
        .await
        .expect("human send after agent revoke");
    assert_eq!(message_of(&after).sequence, 2);
    assert_eq!(
        count_events(&f).await,
        1,
        "revoked target must not generate an event"
    );

    // revoked Agent 自己也不能再写消息。
    let err = send(&f.message, &agent_id, &room_id, "zombie", "z-k")
        .await
        .expect_err("revoked agent write");
    assert!(matches!(
        err,
        crate::ports::message::MessageStoreError::NotFound
    ));
}

#[tokio::test]
async fn per_agent_sequence_starts_at_one_and_is_monotonic() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_a = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let agent_b = create_agent(&f, &alice.participant, "乙", "a-k2").await;
    let room_a = create_room(&f, &alice.participant, agent_a.as_str(), "r-k1").await;
    let room_b = create_room(&f, &alice.participant, agent_b.as_str(), "r-k2").await;

    for i in 0..3 {
        send(
            &f.message,
            &alice.participant.id,
            &room_a,
            &format!("a {i}"),
            &format!("a-{i}"),
        )
        .await
        .expect("send a");
        send(
            &f.message,
            &alice.participant.id,
            &room_b,
            &format!("b {i}"),
            &format!("b-{i}"),
        )
        .await
        .expect("send b");
    }

    assert_eq!(
        event_sequences_for(&f, agent_a.as_str()).await,
        vec![1, 2, 3]
    );
    assert_eq!(
        event_sequences_for(&f, agent_b.as_str()).await,
        vec![1, 2, 3]
    );
    assert_eq!(count_events(&f).await, 6);
}

#[tokio::test]
async fn concurrent_messages_assign_unique_events_per_agent() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    let mut handles = Vec::new();
    for i in 0..8u32 {
        let store = f.message.clone();
        let actor = alice.participant.id.clone();
        let room = room_id.clone();
        handles.push(tokio::spawn(async move {
            send(
                &store,
                &actor,
                &room,
                "boom",
                &format!("concurrent-key-{i}"),
            )
            .await
        }));
    }
    let mut sequences = Vec::new();
    for handle in handles {
        let outcome = handle.await.unwrap().expect("concurrent send");
        sequences.push(message_of(&outcome).sequence);
    }
    sequences.sort_unstable();
    assert_eq!(sequences, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(
        event_sequences_for(&f, agent_id.as_str()).await,
        vec![1, 2, 3, 4, 5, 6, 7, 8],
        "each committed message must carry exactly one event with the next agent sequence"
    );
}

#[tokio::test]
async fn cancelled_transaction_rolls_back_message_and_event_and_frees_sequence() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    // 单连接池：abort 后的查询与正式 send 必须复用同一物理连接；如果
    // Transaction Drop 没有回滚，后续写会遇到 nested transaction/锁污染。
    let options = SqliteConnectOptions::new()
        .filename(f._dir.path().join("core.db"))
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5));
    let cancel_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open single-connection cancellation pool");
    let cancel_store = SqliteMessageStore::new(cancel_pool.clone());

    let uncommitted_id = MessageId::new_v4();
    let task_message_id = uncommitted_id.as_str().to_owned();
    let task_room_id = room_id.as_str().to_owned();
    let task_actor_id = alice.participant.id.as_str().to_owned();
    let task_pool = cancel_pool.clone();
    let (inserted_tx, inserted_rx) = tokio::sync::oneshot::channel();
    let (_keep_open_tx, keep_open_rx) = tokio::sync::oneshot::channel::<()>();

    let handle = tokio::spawn(async move {
        let mut conn = task_pool.acquire().await?;
        let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
        // 直接 INSERT 同样走 messages AFTER INSERT 触发器：未提交事务内
        // Message 与 Event 一起成立，abort 后必须一起消失。
        sqlx::query(
            "INSERT INTO messages \
             (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
             VALUES (?, ?, 1, ?, 'uncommitted', 'cancelled-key', ?)",
        )
        .bind(task_message_id)
        .bind(task_room_id)
        .bind(task_actor_id)
        .bind(UtcMillis::now().millis())
        .execute(&mut *tx)
        .await?;
        inserted_tx
            .send(())
            .expect("cancellation test receiver must remain alive");
        let _ = keep_open_rx.await;
        tx.commit().await
    });

    inserted_rx
        .await
        .expect("BEGIN IMMEDIATE and INSERT must complete before abort");
    handle.abort();
    let join_error = handle.await.expect_err("task must be cancelled");
    assert!(join_error.is_cancelled());

    // 同一连接恢复后：未提交 Message 与 Event 都必须消失。
    let uncommitted_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE id = ?")
        .bind(uncommitted_id.as_str())
        .fetch_one(&cancel_pool)
        .await
        .expect("same pooled connection must recover after cancellation");
    assert_eq!(uncommitted_count, 0);
    let uncommitted_events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_events")
        .fetch_one(&cancel_pool)
        .await
        .expect("event rows must be rolled back with the message");
    assert_eq!(uncommitted_events, 0);

    // 下一次正式提交：Message sequence 与 Event sequence 都不被占用。
    let outcome = send(
        &cancel_store,
        &alice.participant.id,
        &room_id,
        "after cancellation",
        "after-cancel-key",
    )
    .await
    .expect("same pool must accept the next Message transaction");
    assert_eq!(message_of(&outcome).sequence, 1);
    assert_eq!(
        event_sequences_for(&f, agent_id.as_str()).await,
        vec![1],
        "cancelled event must not consume the next agent sequence"
    );
}

#[tokio::test]
async fn list_events_is_bounded_ordered_and_agent_scoped() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_a = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let agent_b = create_agent(&f, &alice.participant, "乙", "a-k2").await;
    let room_a = create_room(&f, &alice.participant, agent_a.as_str(), "r-k1").await;
    let room_b = create_room(&f, &alice.participant, agent_b.as_str(), "r-k2").await;

    for i in 0..5 {
        send(
            &f.message,
            &alice.participant.id,
            &room_a,
            &format!("a {i}"),
            &format!("a-{i}"),
        )
        .await
        .expect("send a");
    }
    send(&f.message, &alice.participant.id, &room_b, "b only", "b-k")
        .await
        .expect("send b");

    // Agent A 只能看到自己的 1..=5，严格升序。
    let first = list_events(&f, agent_a.as_str(), 0, 2).await;
    let seqs: Vec<i64> = first.events.iter().map(|e| e.event.sequence).collect();
    assert_eq!(seqs, vec![1, 2]);
    assert_eq!(first.events.len(), 2);

    let second = list_events(&f, agent_a.as_str(), 2, 2).await;
    let seqs: Vec<i64> = second.events.iter().map(|e| e.event.sequence).collect();
    assert_eq!(seqs, vec![3, 4]);
    let third = list_events(&f, agent_a.as_str(), 4, 2).await;
    let seqs: Vec<i64> = third.events.iter().map(|e| e.event.sequence).collect();
    assert_eq!(seqs, vec![5]);

    // 空页：after 超出范围返回空。
    let empty = list_events(&f, agent_a.as_str(), 5, 2).await;
    assert!(empty.events.is_empty());
    let empty = list_events(&f, agent_a.as_str(), i64::MAX, 2).await;
    assert!(empty.events.is_empty());

    // Agent B 隔离：只有自己的 1，看不到 A 的任何 Event。
    let b_page = list_events(&f, agent_b.as_str(), 0, 100).await;
    let seqs: Vec<i64> = b_page.events.iter().map(|e| e.event.sequence).collect();
    assert_eq!(seqs, vec![1]);
    for item in &b_page.events {
        assert_eq!(item.event.target_agent_id.as_str(), agent_b.as_str());
    }

    // Event 的 source Message 事实与 actor 当前投影。
    let item = &first.events[0];
    assert_eq!(item.message.room_id, room_a);
    assert_eq!(item.message.body.as_ref().unwrap().as_str(), "a 0");
    assert_eq!(item.actor.id, alice.participant.id);
    assert_eq!(item.actor.kind.as_str(), "human");
    assert_eq!(item.actor.display_name.as_str(), "Alice");
    assert_eq!(item.actor.status.as_str(), "active");
    assert_eq!(
        item.event.created_at, item.message.created_at,
        "event created_at must equal source message created_at"
    );
}

#[tokio::test]
async fn db_rejects_reverse_writes_matrix() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_a = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let agent_b = create_agent(&f, &alice.participant, "乙", "a-k2").await;
    let room_a = create_room(&f, &alice.participant, agent_a.as_str(), "r-k1").await;
    let room_b = create_room(&f, &alice.participant, agent_b.as_str(), "r-k2").await;
    let m1 = message_of(
        &send(&f.message, &alice.participant.id, &room_a, "m1", "m-1")
            .await
            .expect("m1"),
    )
    .clone();
    let m2 = message_of(
        &send(&f.message, &alice.participant.id, &room_a, "m2", "m-2")
            .await
            .expect("m2"),
    )
    .clone();
    let mb = message_of(
        &send(&f.message, &alice.participant.id, &room_b, "mb", "mb-k")
            .await
            .expect("mb"),
    )
    .clone();
    let pool = f.store.pool().clone();
    let created_at = m1.created_at.millis();
    let fresh_id = || Uuid::new_v4().to_string();

    // UUID 文本形状：即使长度、version 与 variant 位置都像 v4，固定连字符
    // 错位、额外连字符或 BLOB 存储类也必须由表级 CHECK 拒绝。使用 next
    // sequence 和一条已有 source Message，让失败不能被“跳号”守卫掩盖。
    // 注意：这些写入复用了既有 (message_id, target_agent_id) 身份，也会同时
    // 触发 identity guard（同一写入违反多个约束时先返回哪条不属于契约）。
    let mut malformed_id = fresh_id().into_bytes();
    malformed_id[12] = b'-';
    malformed_id[13] = b'a';
    let malformed_id = String::from_utf8(malformed_id).unwrap();
    let err = insert_event_raw(
        &pool,
        &malformed_id,
        agent_a.as_str(),
        3,
        m1.id.as_str(),
        created_at,
    )
    .await
    .expect_err("malformed UUID shape must be rejected");
    assert!(
        err.contains("substr(id, 14, 1) = '-'")
            || err.contains("agent event identity already exists"),
        "malformed UUID rejection message: {err}"
    );

    let mut extra_hyphen_id = fresh_id().into_bytes();
    extra_hyphen_id[0] = b'-';
    let extra_hyphen_id = String::from_utf8(extra_hyphen_id).unwrap();
    let err = insert_event_raw(
        &pool,
        &extra_hyphen_id,
        agent_a.as_str(),
        3,
        m1.id.as_str(),
        created_at,
    )
    .await
    .expect_err("extra UUID hyphen must be rejected");
    assert!(
        err.contains("length(replace(id, '-', '')) = 32")
            || err.contains("agent event identity already exists"),
        "extra UUID hyphen rejection message: {err}"
    );

    let err = sqlx::query(
        "INSERT INTO agent_events (id, target_agent_id, sequence, message_id, created_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(fresh_id().into_bytes())
    .bind(agent_a.as_str())
    .bind(3_i64)
    .bind(m1.id.as_str())
    .bind(created_at)
    .execute(&pool)
    .await
    .expect_err("BLOB UUID must be rejected");
    let err = match err {
        sqlx::Error::Database(e) => e.message().to_owned(),
        other => format!("{other}"),
    };
    assert!(
        err.contains("typeof(id) = 'text'") || err.contains("agent event identity already exists"),
        "BLOB UUID rejection message: {err}"
    );

    let nul_suffixed_id = format!("{}\0tail", fresh_id());
    let err = insert_event_raw(
        &pool,
        &nul_suffixed_id,
        agent_a.as_str(),
        3,
        m1.id.as_str(),
        created_at,
    )
    .await
    .expect_err("NUL-suffixed UUID must be rejected");
    assert!(
        err.contains("instr(id, char(0)) = 0")
            || err.contains("agent event identity already exists"),
        "NUL UUID rejection message: {err}"
    );

    // 跳号：A 当前最大 sequence=2，直接写 5 被 sequence 守卫拒绝；该写入
    // 同时复用既有 (message_id, target_agent_id) 身份，identity guard 也
    // 可能先返回。
    let err = insert_event_raw(
        &pool,
        &fresh_id(),
        agent_a.as_str(),
        5,
        m1.id.as_str(),
        created_at,
    )
    .await
    .expect_err("skip must be rejected");
    assert!(
        err.contains("sequence must be the next")
            || err.contains("agent event identity already exists"),
        "skip rejection message: {err}"
    );

    // 重复 sequence：已有 seq=2，再写 2 同时触发 sequence 守卫与 identity
    // guard（(target, sequence) 已存在），先返回哪条不属于契约。
    let err = insert_event_raw(
        &pool,
        &fresh_id(),
        agent_a.as_str(),
        2,
        m1.id.as_str(),
        created_at,
    )
    .await
    .expect_err("duplicate sequence must be rejected");
    assert!(
        err.contains("sequence must be the next")
            || err.contains("agent event identity already exists"),
        "duplicate sequence rejection message: {err}"
    );

    // 重复 message+target：next=3 且其他条件全对，仍被拒绝（identity
    // guard 或 UNIQUE 约束）。
    let err = insert_event_raw(
        &pool,
        &fresh_id(),
        agent_a.as_str(),
        3,
        m1.id.as_str(),
        created_at,
    )
    .await
    .expect_err("duplicate message+target must be rejected");
    assert!(
        err.contains("UNIQUE constraint failed")
            || err.contains("agent event identity already exists"),
        "duplicate message+target rejection message: {err}"
    );

    // 错误 created_at：必须等于 source Message created_at；该写入同时复用
    // 既有 (message_id, target_agent_id) 身份，identity guard 也可能先返回。
    let err = insert_event_raw(
        &pool,
        &fresh_id(),
        agent_a.as_str(),
        3,
        m1.id.as_str(),
        created_at + 1,
    )
    .await
    .expect_err("wrong created_at must be rejected");
    assert!(
        err.contains("paired human message") || err.contains("agent event identity already exists"),
        "created_at rejection message: {err}"
    );

    // 错误 target：message 属于 room_b 的配对（agent_b），target 写成 agent_a 被拒。
    let err = insert_event_raw(
        &pool,
        &fresh_id(),
        agent_a.as_str(),
        3,
        mb.id.as_str(),
        mb.created_at.millis(),
    )
    .await
    .expect_err("wrong target must be rejected");
    assert!(
        err.contains("paired human message"),
        "wrong target rejection message: {err}"
    );

    // target 是 actor 自身（Human）：不是配对 Agent，被拒。
    let err = insert_event_raw(
        &pool,
        &fresh_id(),
        alice.participant.id.as_str(),
        3,
        m1.id.as_str(),
        created_at,
    )
    .await
    .expect_err("actor itself must be rejected");
    assert!(
        err.contains("paired human message"),
        "actor-as-target rejection message: {err}"
    );

    // 非 active target：revoke 后 Participant 与 Membership 都非 active，
    // 被拒；该写入同时复用既有 (message_id, target_agent_id) 身份，identity
    // guard 也可能先返回。
    agent::revoke_agent(&f.agent, &alice.participant.id, agent_a.as_str())
        .await
        .expect("revoke agent a");
    let err = insert_event_raw(
        &pool,
        &fresh_id(),
        agent_a.as_str(),
        3,
        m1.id.as_str(),
        created_at,
    )
    .await
    .expect_err("revoked target must be rejected");
    assert!(
        err.contains("paired human message") || err.contains("agent event identity already exists"),
        "revoked target rejection message: {err}"
    );

    // UPDATE / DELETE 一律拒绝。
    let err = sqlx::query("UPDATE agent_events SET sequence = 99 WHERE message_id = ?")
        .bind(m2.id.as_str())
        .execute(&pool)
        .await;
    let err = err.expect_err("update must be rejected");
    let err = match err {
        sqlx::Error::Database(e) => e.message().to_owned(),
        other => format!("{other}"),
    };
    assert!(
        err.contains("agent events are immutable"),
        "update message: {err}"
    );
    let err = sqlx::query("DELETE FROM agent_events").execute(&pool).await;
    let err = err.expect_err("delete must be rejected");
    let err = match err {
        sqlx::Error::Database(e) => e.message().to_owned(),
        other => format!("{other}"),
    };
    assert!(err.contains("cannot be deleted"), "delete message: {err}");

    // 全部拒绝后：A 仍只有 2 条 Event（B 1 条），Message 不变。
    assert_eq!(count_events_for(&f, agent_a.as_str()).await, 2);
    assert_eq!(count_events_for(&f, agent_b.as_str()).await, 1);
    assert_eq!(count_events(&f).await, 3);
    assert_eq!(count_messages(&f).await, 3);
    assert_eq!(event_sequences_for(&f, agent_a.as_str()).await, vec![1, 2]);
}

#[tokio::test]
async fn credential_binding_and_agent_revocation_do_not_alter_events() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    send(&f.message, &alice.participant.id, &room_id, "hello", "m-1")
        .await
        .expect("send");
    let binding_id = f
        .backend
        .create_binding(
            &alice.participant.id,
            &NewBinding {
                id: BindingId(Uuid::new_v4().to_string()),
                agent_id: agent_id.clone(),
                created_by_human_id: alice.participant.id.clone(),
                label: crate::domain::backend::BackendLabel::parse("local runtime").unwrap(),
                idempotency_key: IdempotencyKey::parse("b-k").unwrap(),
                created_at: UtcMillis::now(),
            },
        )
        .await
        .expect("create binding");
    let crate::ports::backend::CreateBindingOutcome::Created { binding } = binding_id else {
        panic!("must be created");
    };
    let token = BackendToken::generate().unwrap();
    let credential = NewCredential {
        id: CredentialId(Uuid::new_v4().to_string()),
        binding_id: binding.id.clone(),
        issued_by_human_id: alice.participant.id.clone(),
        token_hash: token.sha256_hex(),
        replaces_credential_id: None,
        idempotency_key: IdempotencyKey::parse("c-k").unwrap(),
        created_at: UtcMillis::now(),
    };
    f.backend
        .issue_credential(
            &alice.participant.id,
            &agent_id,
            binding.id.as_str(),
            &credential,
        )
        .await
        .expect("issue credential");

    let before: Vec<(String, i64, String, i64)> = sqlx::query_as(
        "SELECT id, sequence, message_id, created_at FROM agent_events ORDER BY sequence",
    )
    .fetch_all(f.store.pool())
    .await
    .unwrap();

    // 撤销 Credential → Event 行不变。
    f.backend
        .revoke_credential(
            &alice.participant.id,
            &agent_id,
            binding.id.as_str(),
            credential.id.as_str(),
        )
        .await
        .expect("revoke credential");
    let after_credential: Vec<(String, i64, String, i64)> = sqlx::query_as(
        "SELECT id, sequence, message_id, created_at FROM agent_events ORDER BY sequence",
    )
    .fetch_all(f.store.pool())
    .await
    .unwrap();
    assert_eq!(
        after_credential, before,
        "credential revoke must not touch events"
    );

    // 撤销 Binding → Event 行不变。
    f.backend
        .revoke_binding(&alice.participant.id, &agent_id, binding.id.as_str())
        .await
        .expect("revoke binding");
    let after_binding: Vec<(String, i64, String, i64)> = sqlx::query_as(
        "SELECT id, sequence, message_id, created_at FROM agent_events ORDER BY sequence",
    )
    .fetch_all(f.store.pool())
    .await
    .unwrap();
    assert_eq!(
        after_binding, before,
        "binding revoke must not touch events"
    );

    // 撤销 Agent → Event 行保留。
    agent::revoke_agent(&f.agent, &alice.participant.id, agent_id.as_str())
        .await
        .expect("revoke agent");
    let after_agent: Vec<(String, i64, String, i64)> = sqlx::query_as(
        "SELECT id, sequence, message_id, created_at FROM agent_events ORDER BY sequence",
    )
    .fetch_all(f.store.pool())
    .await
    .unwrap();
    assert_eq!(after_agent, before, "agent revoke must keep event rows");
    assert_eq!(count_events(&f).await, 1);
}

// （批次二删除）`upgrade_from_v6_keeps_facts_and_does_not_backfill_events` 断言「版本升级后旧事实保留」——那是**已退役的承诺**：
// CORE_V1_IM_CONTRACT.md:262 的迁移治理写明「旧开发数据一律整体丢弃重建，不提供
// 旧库升级桥、兼容双读或回填；验收只认**全新数据库与重复启动**」。本刀是 schema-only
// 重建（真家走「删 core.db 重新奠基」），所以那条路径不存在了。
// 「重复启动」那一半由 `migration_store_tests` 覆盖（同一库重复打开、迁移计数不重复）。

#[tokio::test]
async fn schema_has_agent_events_and_no_deferred_objects() {
    let f = fixture().await;
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table'")
            .fetch_all(f.store.pool())
            .await
            .unwrap();
    assert!(
        tables.iter().any(|name| name == "agent_events"),
        "agent_events table must exist"
    );

    // agent_events 列与索引：无 Completion/ACK/cursor/attempt/Lease 等延期
    // 对象。kind 列（message.created / message.recalled）是撤回扩展
    // （CORE_V1_MESSAGE_RECALL.md §10）冻结的身份列。
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('agent_events') ORDER BY cid")
            .fetch_all(f.store.pool())
            .await
            .unwrap();
    assert_eq!(
        columns,
        vec![
            "id",
            "kind",
            "target_agent_id",
            "sequence",
            "message_id",
            "created_at"
        ]
    );

    let forbidden = [
        "ack", "attempt", "lease", "fencing", "runtime", "provider", "memory", "skill", "tool",
        "mcp", "run",
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
async fn reading_events_has_no_side_effects() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    for i in 0..3 {
        send(
            &f.message,
            &alice.participant.id,
            &room_id,
            &format!("msg {i}"),
            &format!("m-{i}"),
        )
        .await
        .expect("send");
    }

    let first = list_events(&f, agent_id.as_str(), 0, 2).await;
    let second = list_events(&f, agent_id.as_str(), 0, 2).await;
    assert_eq!(first.events.len(), second.events.len());
    assert_eq!(
        first.events[0].event.id, second.events[0].event.id,
        "re-pull must return the same events"
    );
    assert_eq!(
        first.events[0].event.sequence,
        second.events[0].event.sequence
    );

    // 读取不建/改 Event、attempt、Completion、cursor、Lease 或 Binding。
    assert_eq!(count_events(&f).await, 3);
    let bindings: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backend_bindings")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(bindings, 0);
}

// ===== group Mention → Agent Event：同一 Event 表双目标源 =====

/// 带 mentions 提交的发送辅助。
async fn send_mentions(
    store: &SqliteMessageStore,
    actor_id: &ParticipantId,
    room_id: &RoomId,
    body: &str,
    mentions: Vec<ParticipantId>,
    key: &str,
) -> Result<SendMessageOutcome, crate::ports::message::MessageStoreError> {
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
}

/// 创建 group 并邀请 owner 拥有的两个 Agent 与 accepted Human。
async fn create_group_with_members(
    f: &Fixture,
    owner: &Participant,
    name: &str,
    key: &str,
    members: &[ParticipantId],
) -> RoomId {
    let outcome = room_application::create_group(&f.room, owner, name, key)
        .await
        .expect("create group");
    let room_application::CreateRoomOutcome::Created { room, .. } = outcome else {
        panic!("first group create must be Created");
    };
    for (idx, member) in members.iter().enumerate() {
        let invite_key = format!("{key}-m{idx}");
        room_application::invite_group_member(
            &f.room,
            owner,
            room.id.as_str(),
            member.as_str(),
            &invite_key,
        )
        .await
        .expect("invite member");
    }
    room.id
}

#[tokio::test]
async fn group_mention_generates_events_and_ordinary_messages_do_not() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let agent_1 = create_agent(&f, &alice, "甲", "a-k1").await;
    let agent_2 = create_agent(&f, &alice, "乙", "a-k2").await;
    let group =
        create_group_with_members(&f, &alice, "群", "g-k", &[agent_1.clone(), agent_2.clone()])
            .await;

    // 普通群消息不生成 Event。
    send(&f.message, &alice.id, &group, "普通", "m-1")
        .await
        .expect("ordinary");
    assert_eq!(
        count_events(&f).await,
        0,
        "ordinary group message must not wake agents"
    );

    // @agent_1 → agent_1 一条 Event（sequence 从 1 开始）。
    send_mentions(
        &f.message,
        &alice.id,
        &group,
        "@甲 你好",
        vec![agent_1.clone()],
        "m-2",
    )
    .await
    .expect("mention one");
    let events = fetch_events(&f).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, agent_1.as_str());
    assert_eq!(events[0].1, 1);

    // @两个 Agent → 各自独立 Event，sequence 各自推进。
    let mut sorted = vec![agent_1.clone(), agent_2.clone()];
    sorted.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    send_mentions(
        &f.message,
        &alice.id,
        &group,
        "@两位 回答",
        sorted.clone(),
        "m-3",
    )
    .await
    .expect("mention two");
    let events = fetch_events(&f).await;
    assert_eq!(events.len(), 3);
    let by_agent: std::collections::HashMap<&str, Vec<i64>> = {
        let mut m: std::collections::HashMap<&str, Vec<i64>> = Default::default();
        for (agent, seq) in &events {
            m.entry(agent).or_default().push(*seq);
        }
        m
    };
    assert_eq!(by_agent.get(agent_1.as_str()).unwrap(), &vec![1, 2]);
    assert_eq!(by_agent.get(agent_2.as_str()).unwrap(), &vec![1]);

    // 幂等重放不生成第二条 Event。
    send_mentions(
        &f.message,
        &alice.id,
        &group,
        "@甲 你好",
        vec![agent_1.clone()],
        "m-2",
    )
    .await
    .expect("replay");
    assert_eq!(count_events(&f).await, 3, "replay must not create events");

    // Agent 回复（agent actor，无 mention）不生成 Event、不自触发。
    send(&f.message, &agent_1, &group, "echo: @甲 你好", "m-reply")
        .await
        .expect("agent reply");
    assert_eq!(
        count_events(&f).await,
        3,
        "agent reply must not self-trigger"
    );

    // direct 旧路径仍工作：direct 消息生成配对 Agent Event（同一张表）。
    let direct = create_room(&f, &alice, agent_1.as_str(), "r-k1").await;
    send(&f.message, &alice.id, &direct, "hi", "m-d1")
        .await
        .expect("direct message");
    let events = fetch_events(&f).await;
    assert_eq!(events.len(), 4);
    let agent_1_seq: Vec<i64> = events
        .iter()
        .filter(|(a, _)| *a == agent_1.as_str())
        .map(|(_, s)| *s)
        .collect();
    assert_eq!(
        agent_1_seq,
        vec![1, 2, 3],
        "direct and mention events share one sequence"
    );
}

async fn fetch_events(f: &Fixture) -> Vec<(String, i64)> {
    sqlx::query_as("SELECT target_agent_id, sequence FROM agent_events ORDER BY sequence")
        .fetch_all(f.store.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn mention_target_validation_is_atomic_with_no_event_side_effects() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let agent_1 = create_agent(&f, &alice, "甲", "a-k1").await;
    let agent_2 = create_agent(&f, &alice, "乙", "a-k2").await;
    let group =
        create_group_with_members(&f, &alice, "群", "g-k", std::slice::from_ref(&agent_1)).await;

    // 非成员 Agent mention → 400，无 Message/Mention/Event/sequence。
    let err = send_mentions(
        &f.message,
        &alice.id,
        &group,
        "hi",
        vec![agent_2.clone()],
        "m-bad",
    )
    .await
    .expect_err("non-member mention must fail");
    assert!(matches!(
        err,
        crate::ports::message::MessageStoreError::InvalidMention
    ));
    assert_eq!(count_events(&f).await, 0);
    let message_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(message_count, 0);

    // 合法消息 sequence 从 1 开始（无副作用残留）。
    send_mentions(
        &f.message,
        &alice.id,
        &group,
        "@甲 ok",
        vec![agent_1.clone()],
        "m-ok",
    )
    .await
    .expect("valid mention");
    let events = fetch_events(&f).await;
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].1, 1,
        "agent event sequence starts from 1 after atomic failure"
    );
}

/// 带 reply target 提交的发送辅助。
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
#[tokio::test]
async fn event_projection_carries_reply_target_without_target_content() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    // target 消息 → 产生 Event 1；回复 target 的消息 → 产生 Event 2。
    let target = send(&f.message, &alice.participant.id, &room_id, "被回复", "m-1")
        .await
        .expect("target");
    let target_msg = message_of(&target);
    let reply = send_reply(
        &f.message,
        &alice.participant.id,
        &room_id,
        "回复",
        &target_msg.id,
        "m-2",
    )
    .await
    .expect("reply");
    let reply_msg = message_of(&reply);

    let page = f
        .event
        .list_events(&agent_id, 0, 50)
        .await
        .expect("list events");
    assert_eq!(page.events.len(), 2);
    let target_event = page
        .events
        .iter()
        .find(|e| e.message.id == target_msg.id)
        .expect("target event");
    let reply_event = page
        .events
        .iter()
        .find(|e| e.message.id == reply_msg.id)
        .expect("reply event");
    assert_eq!(target_event.message.reply_to_message_id, None);
    assert_eq!(
        reply_event.message.reply_to_message_id,
        Some(target_msg.id.clone()),
        "agent event must expose the reply target id"
    );
    assert_eq!(
        reply_event.message.body.as_ref().unwrap().as_str(),
        "回复",
        "projection carries only the source body, never the target body"
    );
}

#[tokio::test]
async fn agent_say_message_has_null_reply_target_in_event_projection() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    // Human 先问一条（生成 Event），再由 **Agent 自己**以成员身份 say 回话：
    // 事件投影的 source Message 必须是 null，而不是某个猜测的 target。
    // ⚠ 2026-09-28：原走 Completion 写回（reply 意图）；Completion/ACK 整体
    // 退役后改走 say——拉取模型下主体自己说，同样不携带 reply target。
    let sent = send(&f.message, &alice.participant.id, &room_id, "问", "m-1")
        .await
        .expect("human message");
    let said = send(&f.message, &agent_id, &room_id, "答", "m-2")
        .await
        .expect("agent say");

    // Agent Message 不生成新 Event（不自我触发）：事件页仍只有 Human 的
    // "问" 一条，其 source Message 的 reply target 为 null。
    let page = f
        .event
        .list_events(&agent_id, 0, 50)
        .await
        .expect("list events");
    assert_eq!(page.events.len(), 1, "agent say must not create an event");
    assert_eq!(page.events[0].message.body.as_ref().unwrap().as_str(), "问");
    assert_eq!(
        page.events[0].message.reply_to_message_id, None,
        "plain source message keeps a null reply target"
    );

    // 说出的 "答" 消息存在且 messages 行 reply_to_message_id 为 NULL
    //（say 恒不携带 reply target，不猜测回复对象）。
    let reply_id = message_of(&said).id.clone();
    let (body,): (String,) = sqlx::query_as("SELECT body FROM messages WHERE id = ?")
        .bind(reply_id.as_str())
        .fetch_one(f.store.pool())
        .await
        .unwrap();
    assert_eq!(body, "答");
    let (reply_target,): (Option<String>,) =
        sqlx::query_as("SELECT reply_to_message_id FROM messages WHERE id = ?")
            .bind(reply_id.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(reply_target, None, "say never stores a reply target");
    let _ = sent;
}

// ===== Backend Event cursor 解析（resolve_event_after）=====

#[tokio::test]
async fn resolve_event_cursor_is_agent_scoped() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_a = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let agent_b = create_agent(&f, &alice.participant, "乙", "a-k2").await;
    let room_a = create_room(&f, &alice.participant, agent_a.as_str(), "r-1").await;
    let room_b = create_room(&f, &alice.participant, agent_b.as_str(), "r-2").await;
    for i in 1..=3 {
        send(
            &f.message,
            &alice.participant.id,
            &room_a,
            &format!("a{i}"),
            &format!("a-{i}"),
        )
        .await
        .expect("send A");
    }
    send(&f.message, &alice.participant.id, &room_b, "b only", "b-1")
        .await
        .expect("send B");

    let events_a = f
        .event
        .list_events(&agent_a, 0, 100)
        .await
        .expect("list A")
        .events;
    let events_b = f
        .event
        .list_events(&agent_b, 0, 100)
        .await
        .expect("list B")
        .events;
    assert_eq!(events_a.len(), 3);
    assert_eq!(events_b.len(), 1);

    // 同 Agent cursor：解析回该 Event 的 sequence（1..=3）。
    for (i, item) in events_a.iter().enumerate() {
        let seq = f
            .event
            .resolve_event_after(&agent_a, &item.event.id)
            .await
            .expect("resolve")
            .expect("own event");
        assert_eq!(seq, i as i64 + 1);
    }

    // 跨 Agent cursor：A 的 Event 在 B 的作用域下不可解析（None）。
    assert_eq!(
        f.event
            .resolve_event_after(&agent_b, &events_a[0].event.id)
            .await
            .expect("resolve"),
        None,
        "cross-agent cursor must not resolve"
    );

    // 未知 Event ID：None（与跨 Agent 形状一致，不泄露存在性）。
    let unknown = crate::domain::agent_event::AgentEventId(Uuid::new_v4().to_string());
    assert_eq!(
        f.event
            .resolve_event_after(&agent_a, &unknown)
            .await
            .expect("resolve"),
        None
    );
}

// ----------------------------------------------------------------------
// Agent Live State 切片：activity_source 只读裁决（CORE_V1_AGENT_PROTOCOL.md §6）
// ----------------------------------------------------------------------
