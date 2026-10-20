//! SQLite Message adapter 测试：真实临时数据库上的提交、幂等重放/冲突、
//! 并发唯一、无副作用失败、History keyset 分页、数据库反向约束、UTF-8
//! 字节边界、不可变/不可删、升级与 schema 精确集合。

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Connection, SqlitePool};
use uuid::Uuid;

use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::message_store::SqliteMessageStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::room_store::SqliteRoomStore;
use crate::application::agent;
use crate::application::identity::bootstrap_human;
use crate::application::room as room_application;
use crate::domain::agent::IdempotencyKey;
use crate::domain::message::{MessageBody, MessageId};
use crate::domain::participant::{DisplayName, Participant, ParticipantId};
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::ports::message::{
    MAX_MENTIONS_PER_MESSAGE, MessagePage, MessageStore, NewMessage, SendMessageOutcome,
};
use crate::ports::migration::{MigrationProbe, MigrationStatus};

struct Fixture {
    _dir: tempfile::TempDir,
    store: MigrationStore,
    pool: SqlitePool,
    identity: SqliteIdentityStore,
    agent: SqliteAgentStore,
    room: SqliteRoomStore,
    message: SqliteMessageStore,
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
        store,
    }
}

/// 通过 schema 直接创建第二个 Human（非成员用）。
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

/// 通过 Message 端口提交：actor 是通用 Participant（不限于 Human）。
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

async fn list(
    store: &SqliteMessageStore,
    reader_id: &ParticipantId,
    room_id: &RoomId,
    limit: u32,
    before: Option<i64>,
) -> Result<Option<MessagePage>, crate::ports::message::MessageStoreError> {
    store.list_messages(reader_id, room_id, limit, before).await
}

async fn count_messages(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(f.store.pool())
        .await
        .unwrap()
}

async fn sequences_of(f: &Fixture, room_id: &str) -> Vec<i64> {
    sqlx::query_scalar("SELECT sequence FROM messages WHERE room_id = ? ORDER BY sequence ASC")
        .bind(room_id)
        .fetch_all(f.store.pool())
        .await
        .unwrap()
}

fn message_of(outcome: &SendMessageOutcome) -> &crate::domain::message::Message {
    match outcome {
        SendMessageOutcome::Created { message } | SendMessageOutcome::Replayed { message } => {
            message
        }
    }
}

#[tokio::test]
async fn serial_send_assigns_increasing_unique_sequences() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    for (i, key) in ["m-1", "m-2", "m-3"].iter().enumerate() {
        let outcome = send(
            &f.message,
            &alice.participant.id,
            &room_id,
            &format!("msg {i}"),
            key,
        )
        .await
        .expect("send");
        let SendMessageOutcome::Created { message } = outcome else {
            panic!("first send must be Created");
        };
        assert_eq!(message.sequence, (i + 1) as i64);
        assert_eq!(message.actor_id, alice.participant.id);
        assert_eq!(message.body.as_ref().unwrap().as_str(), format!("msg {i}"));
        assert_eq!(message.room_id, room_id);
    }
    assert_eq!(sequences_of(&f, room_id.as_str()).await, vec![1, 2, 3]);
}

#[tokio::test]
async fn replay_same_key_same_request_returns_same_message_without_duplicate() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    let first = send(&f.message, &alice.participant.id, &room_id, "hello", "m-1")
        .await
        .expect("first send");
    let replay = send(&f.message, &alice.participant.id, &room_id, "hello", "m-1")
        .await
        .expect("replay must succeed");
    let SendMessageOutcome::Replayed { message } = replay else {
        panic!("replay must be Replayed");
    };
    assert_eq!(message_of(&first).id, message.id);
    assert_eq!(message_of(&first).sequence, message.sequence);
    assert_eq!(message_of(&first).created_at, message.created_at);
    assert_eq!(count_messages(&f).await, 1, "replay must not add a message");
}

#[tokio::test]
async fn replay_same_key_different_body_conflicts_without_rewriting() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    let first = send(&f.message, &alice.participant.id, &room_id, "hello", "m-1")
        .await
        .expect("first send");
    let err = send(&f.message, &alice.participant.id, &room_id, "hello2", "m-1")
        .await
        .expect_err("different body must conflict");
    assert!(
        matches!(
            err,
            crate::ports::message::MessageStoreError::IdempotencyConflict
        ),
        "expected conflict, got {err}"
    );
    let after = list(&f.message, &alice.participant.id, &room_id, 50, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.messages.len(), 1);
    assert_eq!(after.messages[0].message.id, message_of(&first).id);
    assert_eq!(
        after.messages[0].message.body.as_ref().unwrap().as_str(),
        "hello"
    );
}

#[tokio::test]
async fn replay_same_key_different_room_conflicts_without_side_effect() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_1 = create_agent(&f, &alice.participant, "甲", "a-k1").await;
    let agent_2 = create_agent(&f, &alice.participant, "乙", "a-k2").await;
    let room_1 = create_room(&f, &alice.participant, agent_1.as_str(), "r-k1").await;
    let room_2 = create_room(&f, &alice.participant, agent_2.as_str(), "r-k2").await;

    send(&f.message, &alice.participant.id, &room_1, "hello", "m-1")
        .await
        .expect("send in room 1");
    let err = send(&f.message, &alice.participant.id, &room_2, "hello", "m-1")
        .await
        .expect_err("same key in another room must conflict");
    assert!(matches!(
        err,
        crate::ports::message::MessageStoreError::IdempotencyConflict
    ));
    assert_eq!(sequences_of(&f, room_2.as_str()).await, Vec::<i64>::new());
    assert_eq!(count_messages(&f).await, 1);
}

#[tokio::test]
async fn idempotency_comparison_uses_normalized_room_and_parsed_body() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    let first = send(
        &f.message,
        &alice.participant.id,
        &room_id,
        "h\u{e9}llo",
        "m-1",
    )
    .await
    .expect("first send");
    // 相同解析后正文（转义表达不同但 string 相同）→ 重放而非冲突。
    let replay = send(&f.message, &alice.participant.id, &room_id, "héllo", "m-1")
        .await
        .expect("same parsed body must replay");
    let SendMessageOutcome::Replayed { message } = replay else {
        panic!("must be Replayed");
    };
    assert_eq!(message.id, message_of(&first).id);
    assert_eq!(count_messages(&f).await, 1);
}

#[tokio::test]
async fn different_actor_same_key_is_independent() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    // Agent 走同一提交内核（通用 Participant actor），与 Human 使用相同
    // key 彼此独立。
    send(&f.message, &agent_id, &room_id, "from agent", "shared-key")
        .await
        .expect("agent send");
    let human_msg = send(
        &f.message,
        &alice.participant.id,
        &room_id,
        "from human",
        "shared-key",
    )
    .await
    .expect("human send with same key must be independent");
    let SendMessageOutcome::Created { message } = human_msg else {
        panic!("human key must be a fresh fact");
    };
    assert_eq!(message.sequence, 2);

    let agent_replay = send(&f.message, &agent_id, &room_id, "from agent", "shared-key")
        .await
        .expect("agent replay");
    let SendMessageOutcome::Replayed { message } = agent_replay else {
        panic!("agent key must replay its own message");
    };
    assert_eq!(message.body.as_ref().unwrap().as_str(), "from agent");
    assert_eq!(count_messages(&f).await, 2);
}

#[tokio::test]
async fn non_member_send_is_not_found_without_any_side_effect() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(&f.pool, "Bob").await;
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    let err = send(&f.message, &bob.id, &room_id, "intrude", "bob-k")
        .await
        .expect_err("non-member must be rejected");
    // 拆分后的存储契约：Room 与 actor 均存在且 active、但无 active
    // Membership → NotAMember（HTTP 层：Human 路径 404、Backend agent 发消息
    // 路径 403 not_room_member）；unknown/bad-id Room 仍 NotFound（404）。
    assert!(matches!(
        err,
        crate::ports::message::MessageStoreError::NotAMember
    ));
    assert_eq!(count_messages(&f).await, 0, "no message may be created");
    assert_eq!(sequences_of(&f, room_id.as_str()).await, Vec::<i64>::new());

    // 失败的尝试不得占用幂等事实：Alice 随后可用相同 key 正常发送。
    send(
        &f.message,
        &alice.participant.id,
        &room_id,
        "legit",
        "bob-k",
    )
    .await
    .expect("failed attempt must not consume the key");
}

#[tokio::test]
async fn unknown_room_send_is_not_found_without_side_effect() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let ghost = RoomId::new_v4();
    let err = send(&f.message, &alice.participant.id, &ghost, "hello", "m-1")
        .await
        .expect_err("unknown room must be rejected");
    assert!(matches!(
        err,
        crate::ports::message::MessageStoreError::NotFound
    ));
    assert_eq!(count_messages(&f).await, 0);
}

#[tokio::test]
async fn cancelled_immediate_transaction_rolls_back_before_connection_reuse() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    // 单连接池保证 abort 后的查询和正式 send 必须复用同一物理连接；如果
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

    // acquire 会等待唯一连接归池，并触发 SQLx 在 Transaction Drop 时排队的
    // rollback；未提交事实必须消失。
    let uncommitted_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE id = ?")
        .bind(uncommitted_id.as_str())
        .fetch_one(&cancel_pool)
        .await
        .expect("same pooled connection must recover after cancellation");
    assert_eq!(uncommitted_count, 0);

    let outcome = send(
        &cancel_store,
        &alice.participant.id,
        &room_id,
        "after cancellation",
        "after-cancel-key",
    )
    .await
    .expect("same pool must accept the next Message transaction");
    let SendMessageOutcome::Created { message } = outcome else {
        panic!("first committed Message after cancellation must be created");
    };
    assert_eq!(message.sequence, 1);
}

#[tokio::test]
async fn concurrent_different_keys_produce_unique_monotonic_sequences() {
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
            let key = format!("concurrent-key-{i}");
            send(&store, &actor, &room, "boom", &key).await
        }));
    }
    let mut sequences = Vec::new();
    for handle in handles {
        let outcome = handle.await.unwrap().expect("concurrent send");
        let SendMessageOutcome::Created { message } = outcome else {
            panic!("distinct keys must all create");
        };
        sequences.push(message.sequence);
    }
    sequences.sort_unstable();
    assert_eq!(sequences, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(count_messages(&f).await, 8);
}

#[tokio::test]
async fn concurrent_same_key_same_request_converges_to_single_message() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    let mut handles = Vec::new();
    for _ in 0..8 {
        let store = f.message.clone();
        let actor = alice.participant.id.clone();
        let room = room_id.clone();
        handles.push(tokio::spawn(async move {
            send(&store, &actor, &room, "same request", "same-key").await
        }));
    }
    let mut ids = Vec::new();
    let mut created = 0;
    for handle in handles {
        let outcome = handle.await.unwrap().expect("concurrent send");
        match &outcome {
            SendMessageOutcome::Created { .. } => created += 1,
            SendMessageOutcome::Replayed { .. } => {}
        }
        ids.push(message_of(&outcome).id.clone());
    }
    assert_eq!(created, 1, "exactly one request may create");
    assert!(
        ids.iter().all(|id| id == &ids[0]),
        "all successful responses must converge to the same message id"
    );
    assert_eq!(count_messages(&f).await, 1);
}

#[tokio::test]
async fn concurrent_same_key_different_requests_have_at_most_one_winner() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    let mut handles = Vec::new();
    for i in 0..8u32 {
        let store = f.message.clone();
        let actor = alice.participant.id.clone();
        let room = room_id.clone();
        let body = if i % 2 == 0 { "request A" } else { "request B" };
        handles.push(tokio::spawn(async move {
            send(&store, &actor, &room, body, "same-key").await
        }));
    }
    let mut created = 0;
    let mut replayed = 0;
    let mut conflicts = 0;
    for handle in handles {
        match handle.await.unwrap() {
            Ok(SendMessageOutcome::Created { .. }) => created += 1,
            Ok(SendMessageOutcome::Replayed { .. }) => replayed += 1,
            Err(crate::ports::message::MessageStoreError::IdempotencyConflict) => conflicts += 1,
            Err(other) => panic!("unexpected outcome: {other}"),
        }
    }
    assert_eq!(created, 1, "at most one request may create");
    assert_eq!(replayed, 3, "requests identical to the winner must replay");
    assert_eq!(
        conflicts, 4,
        "requests different from the winner must conflict"
    );
    assert_eq!(count_messages(&f).await, 1, "no double write");
}

#[tokio::test]
async fn history_latest_page_and_keyset_pagination_are_complete() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    for i in 1..=7 {
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

    // 首屏：最新一页，messages 升序。
    let first = list(&f.message, &alice.participant.id, &room_id, 3, None)
        .await
        .unwrap()
        .unwrap();
    let seqs: Vec<i64> = first.messages.iter().map(|m| m.message.sequence).collect();
    assert_eq!(seqs, vec![5, 6, 7]);
    assert_eq!(
        first.next_before,
        Some(5),
        "next_before = first sequence of page"
    );

    // 翻页：无重复、无跳过地读完全部历史。
    let second = list(
        &f.message,
        &alice.participant.id,
        &room_id,
        3,
        first.next_before,
    )
    .await
    .unwrap()
    .unwrap();
    let seqs: Vec<i64> = second.messages.iter().map(|m| m.message.sequence).collect();
    assert_eq!(seqs, vec![2, 3, 4]);
    assert_eq!(second.next_before, Some(2));

    let third = list(
        &f.message,
        &alice.participant.id,
        &room_id,
        3,
        second.next_before,
    )
    .await
    .unwrap()
    .unwrap();
    let seqs: Vec<i64> = third.messages.iter().map(|m| m.message.sequence).collect();
    assert_eq!(seqs, vec![1]);
    assert_eq!(third.next_before, None, "no older messages");

    // 拼接检查：无重复、无跳过、覆盖 1..=7。
    let all: Vec<i64> = [&first, &second, &third]
        .iter()
        .flat_map(|p| p.messages.iter().map(|m| m.message.sequence))
        .collect();
    assert_eq!(all, vec![5, 6, 7, 2, 3, 4, 1]);
}

#[tokio::test]
async fn history_limit_respects_bound_and_before_is_exclusive() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    for i in 1..=10 {
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
    // limit 超出现有消息数：全部返回，next_before=null。
    let page = list(&f.message, &alice.participant.id, &room_id, 100, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.messages.len(), 10);
    assert_eq!(page.next_before, None);
    // before 是严格上界：before=5 返回 1..=4。
    let page = list(&f.message, &alice.participant.id, &room_id, 50, Some(5))
        .await
        .unwrap()
        .unwrap();
    let seqs: Vec<i64> = page.messages.iter().map(|m| m.message.sequence).collect();
    assert_eq!(seqs, vec![1, 2, 3, 4]);
    assert_eq!(page.next_before, None);
}

#[tokio::test]
async fn empty_room_history_returns_empty_page() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    let page = list(&f.message, &alice.participant.id, &room_id, 50, None)
        .await
        .unwrap()
        .unwrap();
    assert!(page.messages.is_empty());
    assert_eq!(page.next_before, None);
}

#[tokio::test]
async fn history_read_requires_current_active_membership() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(&f.pool, "Bob").await;
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    send(
        &f.message,
        &alice.participant.id,
        &room_id,
        "private",
        "m-1",
    )
    .await
    .expect("send");

    let result = list(&f.message, &bob.id, &room_id, 50, None)
        .await
        .expect_err("non-member must be a distinct typed error");
    // 切片 B2：Room 存在但 reader 非 active member → 明确 Err(NotAMember)
    // （后端历史端点据以出 403；Human 路径在路由层映射回 404，wire 不变）。
    assert!(matches!(
        result,
        crate::ports::message::MessageStoreError::NotAMember
    ));

    let ghost = RoomId::new_v4();
    let result = list(&f.message, &alice.participant.id, &ghost, 50, None)
        .await
        .unwrap();
    assert!(
        result.is_none(),
        "unknown room must be None（404，不泄露存在性）"
    );
}

#[tokio::test]
async fn history_actor_is_current_participant_projection() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    // Agent 与 Human 各发一条：History 的 actor 投影来自当前 participants
    // 行，Message 自身只固化 actor ID。
    send(
        &f.message,
        &alice.participant.id,
        &room_id,
        "human msg",
        "h-k",
    )
    .await
    .expect("human send");
    send(&f.message, &agent_id, &room_id, "agent msg", "g-k")
        .await
        .expect("agent send");

    let page = list(&f.message, &alice.participant.id, &room_id, 50, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.messages.len(), 2);
    let (human_item, agent_item) = (&page.messages[0], &page.messages[1]);
    assert_eq!(human_item.message.actor_id, alice.participant.id);
    assert_eq!(human_item.actor.kind.as_str(), "human");
    assert_eq!(human_item.actor.display_name.as_str(), "Alice");
    assert_eq!(agent_item.message.actor_id, agent_id);
    assert_eq!(agent_item.actor.kind.as_str(), "agent");
    assert_eq!(agent_item.actor.display_name.as_str(), "助手");
    for item in &page.messages {
        assert_eq!(item.actor.id, item.message.actor_id);
        assert_eq!(item.actor.status.as_str(), "active");
    }
}

#[tokio::test]
async fn agent_revoke_keeps_history_readable_and_human_write_working() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let first = send(
        &f.message,
        &alice.participant.id,
        &room_id,
        "before revoke",
        "m-1",
    )
    .await
    .expect("send");

    agent::revoke_agent(&f.agent, &alice.participant.id, agent_id.as_str())
        .await
        .expect("revoke agent");

    // Human 仍可读取既有历史；Message actor ID 与正文不变。
    let page = list(&f.message, &alice.participant.id, &room_id, 50, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.messages.len(), 1);
    assert_eq!(page.messages[0].message.id, message_of(&first).id);
    assert_eq!(
        page.messages[0].message.body.as_ref().unwrap().as_str(),
        "before revoke"
    );
    assert_eq!(
        page.messages[0].message.actor_id, alice.participant.id,
        "message actor id must not change"
    );

    // Human Membership 仍 active：新 Human Message 只依据 actor、Room 与
    // actor Membership 裁决，仍可发送。
    let after = send(
        &f.message,
        &alice.participant.id,
        &room_id,
        "after revoke",
        "m-2",
    )
    .await
    .expect("human send after agent revoke");
    assert_eq!(message_of(&after).sequence, 2);

    // 被撤销 Agent 的 Membership 已 revoked：不能再以 Agent 身份写入。
    let err = send(&f.message, &agent_id, &room_id, "zombie", "z-k")
        .await
        .expect_err("revoked agent must not write");
    assert!(matches!(
        err,
        crate::ports::message::MessageStoreError::NotFound
    ));
}

#[tokio::test]
async fn replay_precedes_current_membership_state() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let first = send(&f.message, &agent_id, &room_id, "agent memo", "agent-key")
        .await
        .expect("agent send");

    agent::revoke_agent(&f.agent, &alice.participant.id, agent_id.as_str())
        .await
        .expect("revoke agent");

    // 既有幂等事实的重放优先于当前 Room/Membership 状态。
    let replay = send(&f.message, &agent_id, &room_id, "agent memo", "agent-key")
        .await
        .expect("replay must precede membership state");
    let SendMessageOutcome::Replayed { message } = replay else {
        panic!("must be Replayed");
    };
    assert_eq!(message.id, message_of(&first).id);
    assert_eq!(count_messages(&f).await, 1, "replay creates no new fact");
}

#[tokio::test]
async fn db_rejects_empty_nul_and_overlong_bodies() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let room = room_id.as_str();

    for (i, bad) in ["", "a\u{0}b", &"a".repeat(16385)].iter().enumerate() {
        let result = sqlx::query(
            "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, 0)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(room)
        .bind(1i64)
        .bind(alice.participant.id.as_str())
        .bind(*bad)
        .bind(format!("bad-{i}"))
        .execute(f.store.pool())
        .await;
        assert!(result.is_err(), "expected DB rejection for body len/NUL");
    }
    // 合法边界：纯空白、16384 ASCII bytes、16383 多字节 UTF-8。
    // （migration 009 起消息事实时点必须被 Membership 周期覆盖，直接写库
    // 的合法消息使用读取周期内的时间。）
    let now = UtcMillis::now().millis() + 1;
    for (i, ok_body) in [" ", &"a".repeat(16384), &"中".repeat(5461)]
        .iter()
        .enumerate()
    {
        sqlx::query(
            "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(room)
        .bind((i + 2) as i64)
        .bind(alice.participant.id.as_str())
        .bind(*ok_body)
        .bind(format!("ok-{i}"))
        .bind(now)
        .execute(f.store.pool())
        .await
        .expect("body within UTF-8 byte bounds must be accepted");
    }
    // 多字节越过 16384 字节边界：必须拒绝（按字节而非字符）。
    let result = sqlx::query(
        "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, 0)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(room)
    .bind(1i64)
    .bind(alice.participant.id.as_str())
    .bind("中".repeat(5462))
    .bind("too-long")
    .execute(f.store.pool())
    .await;
    assert!(
        result.is_err(),
        "5462*3=16386 utf8 bytes must be rejected (byte boundary, not char count)"
    );
}

#[tokio::test]
async fn db_rejects_blob_storage_class_for_body() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let room = room_id.as_str();

    // 每次插入使用不冲突的 sequence/key/id，排除唯一约束制造假拒绝：
    // 断言必须来自 CHECK（SQLITE_CONSTRAINT_CHECK），而非 UNIQUE。
    let mut sequence = 1i64;
    let reject_as_check = |result: Result<(), sqlx::Error>, label: &str| match result {
        Err(sqlx::Error::Database(e)) => {
            assert!(
                e.message().contains("CHECK constraint failed"),
                "{label}: expected CHECK rejection, got database error: {}",
                e.message()
            );
            assert_eq!(
                e.code().as_deref(),
                Some("275"),
                "{label}: expected SQLITE_CONSTRAINT_CHECK (275), got {:?}",
                e.code()
            );
        }
        other => panic!("{label}: expected CHECK rejection, got {other:?}"),
    };

    // BLOB 存储类旁路：X'FFFE'（非法 UTF-8）与多字节非法序列。
    for (i, blob) in [vec![0xFF, 0xFE], vec![0xFF, 0xFE, 0xFD]]
        .iter()
        .enumerate()
    {
        let result = sqlx::query(
            "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, 0)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(room)
        .bind(sequence)
        .bind(alice.participant.id.as_str())
        .bind(blob.clone())
        .bind(format!("blob-{i}"))
        .execute(f.store.pool())
        .await;
        sequence += 1;
        reject_as_check(result.map(|_| ()), &format!("X'FFFE' blob #{i}"));
    }

    // 含 NUL 的 BLOB（X'610062' = 'a' NUL 'b'）。
    let result = sqlx::query(
        "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, 0)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(room)
    .bind(sequence)
    .bind(alice.participant.id.as_str())
    .bind(b"a\x00b".to_vec())
    .bind("blob-nul")
    .execute(f.store.pool())
    .await;
    sequence += 1;
    reject_as_check(result.map(|_| ()), "NUL blob");

    // CAST(? AS BLOB) 变体：字符串经 CAST 进入 BLOB 存储类同样被拒。
    let result = sqlx::query(
        "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
         VALUES (?, ?, ?, ?, CAST(? AS BLOB), ?, 0)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(room)
    .bind(sequence)
    .bind(alice.participant.id.as_str())
    .bind("x")
    .bind("blob-cast")
    .execute(f.store.pool())
    .await;
    sequence += 1;
    reject_as_check(result.map(|_| ()), "CAST(? AS BLOB)");

    // 合法 TEXT（含 Unicode 与纯空白）仍被接受（时点落在 Membership
    // 周期内，migration 009 交付守卫要求）。
    let now = UtcMillis::now().millis() + 1;
    for (i, text) in ["hello", "你好，世界 😀", " \t\n"].iter().enumerate() {
        sqlx::query(
            "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(room)
        .bind(sequence)
        .bind(alice.participant.id.as_str())
        .bind(*text)
        .bind(format!("text-{i}"))
        .bind(now)
        .execute(f.store.pool())
        .await
        .expect("legal TEXT body must be accepted");
        sequence += 1;
    }

    // 失败不留下额外 Message：只有 3 条合法 TEXT 行。
    assert_eq!(
        count_messages(&f).await,
        3,
        "rejected blobs must leave no rows"
    );
}

#[tokio::test]
async fn db_rejects_non_v4_message_ids() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let room = room_id.as_str();
    let now = UtcMillis::now().millis() + 1;

    // 其他前置事实（active Room/actor/Membership、sequence、body、
    // idempotency key、created_at）全部合法，仅 Message ID 非规范/非 v4：
    // 直接 INSERT 必须被拒绝。每个用例使用不冲突的 sequence/key，
    // 排除 UNIQUE/identity guard 制造假拒绝；断言只要求拒绝，不依赖
    // 多个 CHECK/trigger 的具体错误顺序。
    let v4 = Uuid::new_v4().to_string();
    let mut bad_ids: Vec<(String, &str)> = Vec::new();
    bad_ids.push((v4.to_uppercase(), "uppercase"));
    let mut v1 = v4.clone().into_bytes();
    v1[14] = b'1';
    bad_ids.push((String::from_utf8(v1).unwrap(), "v1 version nibble"));
    let mut v5 = v4.clone().into_bytes();
    v5[14] = b'5';
    bad_ids.push((String::from_utf8(v5).unwrap(), "v5 version nibble"));
    let mut bad_variant = v4.clone().into_bytes();
    bad_variant[19] = b'0';
    bad_ids.push((
        String::from_utf8(bad_variant).unwrap(),
        "wrong variant nibble",
    ));
    let mut extra_hyphen = v4.clone().into_bytes();
    extra_hyphen[0] = b'-';
    bad_ids.push((String::from_utf8(extra_hyphen).unwrap(), "extra hyphen"));
    bad_ids.push(("not-a-uuid".to_owned(), "wrong length"));
    bad_ids.push((format!("{v4}\0tail"), "NUL suffixed"));

    for (i, (id, label)) in bad_ids.into_iter().enumerate() {
        let result = sqlx::query(
            "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
             VALUES (?, ?, ?, ?, 'x', ?, ?)",
        )
        .bind(id)
        .bind(room)
        .bind((i + 2) as i64)
        .bind(alice.participant.id.as_str())
        .bind(format!("bad-id-{i}"))
        .bind(now)
        .execute(f.store.pool())
        .await;
        assert!(
            result.is_err(),
            "{label}: non-v4 message id must be rejected"
        );
    }

    // BLOB 存储类旁路同样被 typeof(id) CHECK 拒绝。
    let result = sqlx::query(
        "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
         VALUES (?, ?, ?, ?, 'x', ?, ?)",
    )
    .bind(v4.clone().into_bytes())
    .bind(room)
    .bind(10i64)
    .bind(alice.participant.id.as_str())
    .bind("bad-id-blob")
    .bind(now)
    .execute(f.store.pool())
    .await;
    assert!(result.is_err(), "BLOB message id must be rejected");

    // 对照：同一前置事实下规范 v4 文本直接 INSERT 仍被接受。
    sqlx::query(
        "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
         VALUES (?, ?, ?, ?, 'x', ?, ?)",
    )
    .bind(v4)
    .bind(room)
    .bind(1i64)
    .bind(alice.participant.id.as_str())
    .bind("legal-id")
    .bind(now)
    .execute(f.store.pool())
    .await
    .expect("canonical v4 message id must be accepted");

    // 失败不留下额外 Message：只有 1 条合法行。
    assert_eq!(
        count_messages(&f).await,
        1,
        "rejected ids must leave no rows"
    );
}

#[tokio::test]
async fn db_rejects_duplicate_sequence_and_duplicate_actor_key() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let room = room_id.as_str();
    let now = UtcMillis::now().millis() + 1;

    sqlx::query(
        "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
         VALUES (?, ?, ?, ?, 'x', ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(room)
    .bind(1i64)
    .bind(alice.participant.id.as_str())
    .bind("k-1")
    .bind(now)
    .execute(f.store.pool())
    .await
    .unwrap();
    let duplicate_sequence = sqlx::query(
        "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
         VALUES (?, ?, ?, ?, 'x', ?, 0)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(room)
    .bind(1i64)
    .bind(alice.participant.id.as_str())
    .bind("k-2")
    .execute(f.store.pool())
    .await;
    assert!(
        duplicate_sequence.is_err(),
        "duplicate (room_id, sequence) must be rejected"
    );
    let duplicate_key = sqlx::query(
        "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
         VALUES (?, ?, ?, ?, 'x', ?, 0)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(room)
    .bind(2i64)
    .bind(alice.participant.id.as_str())
    .bind("k-1")
    .execute(f.store.pool())
    .await;
    assert!(
        duplicate_key.is_err(),
        "duplicate (actor_id, idempotency_key) must be rejected"
    );
    assert_eq!(count_messages(&f).await, 1);
}

#[tokio::test]
async fn db_rejects_non_member_and_revoked_membership_inserts() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(&f.pool, "Bob").await;
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    sqlx::query(
        "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
         VALUES (?, ?, 1, ?, 'x', ?, 0)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(room_id.as_str())
    .bind(bob.id.as_str())
    .bind("bob-k")
    .execute(f.store.pool())
    .await
    .expect_err("non-member insert must be rejected");
    // active 成员（含 Agent，证明不写死 Human）直接写库成功（时点落在
    // Membership 周期内）。
    let now = UtcMillis::now().millis() + 1;
    sqlx::query(
        "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
         VALUES (?, ?, 1, ?, 'x', ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(room_id.as_str())
    .bind(agent_id.as_str())
    .bind("agent-k")
    .bind(now)
    .execute(f.store.pool())
    .await
    .expect("active member agent insert must succeed");

    // 撤销后：Agent Membership 变为 revoked，其直接写库被拒绝。
    agent::revoke_agent(&f.agent, &alice.participant.id, agent_id.as_str())
        .await
        .expect("revoke agent");
    sqlx::query(
        "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
         VALUES (?, ?, 1, ?, 'x', ?, 0)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(room_id.as_str())
    .bind(agent_id.as_str())
    .bind("agent-k2")
    .execute(f.store.pool())
    .await
    .expect_err("revoked membership insert must be rejected");

    assert_eq!(count_messages(&f).await, 1);
}

#[tokio::test]
async fn db_enforces_immutable_and_no_delete_on_messages() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;
    let first = send(&f.message, &alice.participant.id, &room_id, "frozen", "m-1")
        .await
        .expect("send");
    let id = message_of(&first).id.as_str();

    for statement in [
        format!("UPDATE messages SET body = 'tampered' WHERE id = '{id}'"),
        format!("UPDATE messages SET sequence = 99 WHERE id = '{id}'"),
        format!("UPDATE messages SET actor_id = 'someone-else' WHERE id = '{id}'"),
        format!("DELETE FROM messages WHERE id = '{id}'"),
    ] {
        let result = sqlx::query(&statement).execute(f.store.pool()).await;
        assert!(
            result.is_err(),
            "statement must be rejected by the database: {statement}"
        );
    }
    // 等值自赋值不触发不可变守卫（与既有表一致）。
    let noop = format!("UPDATE messages SET body = body WHERE id = '{id}'");
    sqlx::query(&noop)
        .execute(f.store.pool())
        .await
        .expect("no-op self assignment must pass");

    let (body, sequence): (String, i64) =
        sqlx::query_as("SELECT body, sequence FROM messages WHERE id = ?")
            .bind(id)
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(body, "frozen");
    assert_eq!(sequence, 1);
}

#[tokio::test]
async fn unicode_and_whitespace_bodies_round_trip_verbatim() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    let bodies = [
        "line1\nline2\r\nline3\ttab",
        "😀 emoji 你好，世界",
        " \t\n\r ",
        "trailing space ",
        "混合 Mixed 123",
    ];
    for (i, body) in bodies.iter().enumerate() {
        let outcome = send(
            &f.message,
            &alice.participant.id,
            &room_id,
            body,
            &format!("u-{i}"),
        )
        .await
        .expect("send");
        assert_eq!(
            message_of(&outcome).body.as_ref().unwrap().as_str(),
            *body,
            "must be verbatim"
        );
    }
    let page = list(&f.message, &alice.participant.id, &room_id, 50, None)
        .await
        .unwrap()
        .unwrap();
    let round_trip: Vec<&str> = page
        .messages
        .iter()
        .map(|m| m.message.body.as_ref().unwrap().as_str())
        .collect();
    assert_eq!(round_trip, bodies);
}

#[tokio::test]
async fn restart_preserves_messages_and_replay_facts() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("core.db");
    let store = MigrationStore::open(&db_path).await.expect("open");
    let pool = store.pool().clone();
    let identity = SqliteIdentityStore::new(pool.clone());
    let agent = SqliteAgentStore::new(pool.clone());
    let room = SqliteRoomStore::new(pool.clone());
    let message = SqliteMessageStore::new(pool.clone());
    let alice = bootstrap_human(&identity, "Alice").await.unwrap();
    let created = agent::create_agent(&agent, &alice.participant, "助手", "a-k")
        .await
        .expect("create agent");
    let agent::CreateAgentOutcome::Created {
        agent: agent_created,
        ..
    } = created
    else {
        panic!("must be Created");
    };
    let created_room = room_application::create_direct_room(
        &room,
        &alice.participant,
        agent_created.id.as_str(),
        "r-k",
    )
    .await
    .expect("create room");
    let room_application::CreateDirectRoomOutcome::Created { room, .. } = created_room else {
        panic!("must be Created");
    };
    let first = send(
        &message,
        &alice.participant.id,
        &room.id,
        "persist me",
        "m-1",
    )
    .await
    .expect("send");
    drop(store);

    let reopened = MigrationStore::open(&db_path).await.expect("reopen");
    let status = reopened.status().await.expect("probe");
    assert_eq!(
        status,
        MigrationStatus {
            pending: 0,
            failed: 0
        }
    );
    let message2 = SqliteMessageStore::new(reopened.pool().clone());

    let replay = send(
        &message2,
        &alice.participant.id,
        &room.id,
        "persist me",
        "m-1",
    )
    .await
    .expect("replay after restart");
    let SendMessageOutcome::Replayed { message } = replay else {
        panic!("must replay after restart");
    };
    assert_eq!(message.id, message_of(&first).id);
    assert_eq!(message.sequence, 1);
    let page = list(&message2, &alice.participant.id, &room.id, 50, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(page.messages.len(), 1);
    assert_eq!(
        page.messages[0].message.body.as_ref().unwrap().as_str(),
        "persist me"
    );
}

// （批次二删除）`upgrade_from_v4_preserves_data_and_messages_work_after` 断言「版本升级后旧事实保留」——那是**已退役的承诺**：
// CORE_V1_IM_CONTRACT.md:262 的迁移治理写明「旧开发数据一律整体丢弃重建，不提供
// 旧库升级桥、兼容双读或回填；验收只认**全新数据库与重复启动**」。本刀是 schema-only
// 重建（真家走「删 core.db 重新奠基」），所以那条路径不存在了。
// 「重复启动」那一半由 `migration_store_tests` 覆盖（同一库重复打开、迁移计数不重复）。

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
        "pairing_transactions",
    ];
    for table in &tables {
        assert!(
            expected.contains(&table.as_str()),
            "unexpected table {table:?} in schema"
        );
    }
    assert_eq!(tables.len(), expected.len());

    // 回复/引用唯一事实是 messages 上的可空不可变列：schema 精确集合不得
    // 出现第二张回复关系表（断言上方 expected 已排除），messages 必须
    // 持有 reply_to_message_id 列。
    let message_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('messages') ORDER BY cid")
            .fetch_all(f.store.pool())
            .await
            .unwrap();
    assert!(
        message_columns
            .iter()
            .any(|c| c.as_str() == "reply_to_message_id"),
        "messages must carry the reply_to_message_id column: {message_columns:?}"
    );

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

// ===== Mentions：group 消息显式唤醒事实 =====

/// 通过公开用例创建 group 并邀请成员（owner + 成员）。
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

/// 带 mentions 提交的发送辅助。
async fn send_with_mentions(
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

#[tokio::test]
async fn group_mentions_commit_with_message_and_round_trip_in_history() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    // accepted contact A↔B（group 邀请 Human 成员需要）。
    let (ca, cb) = crate::domain::contact::canonical_pair(&alice.id, &bob.id);
    sqlx::query(
        "INSERT INTO contact_periods (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at, idempotency_key) \
         VALUES ('00000000-0000-4000-8000-00000000d001', ?, ?, ?, ?, 'accepted', 1, 2, 'c-k')",
    )
    .bind(ca.as_str())
    .bind(cb.as_str())
    .bind(alice.id.as_str())
    .bind(bob.id.as_str())
    .execute(&f.pool)
    .await
    .expect("accepted contact");
    let agent_1 = create_agent(&f, &alice, "甲", "a-k1").await;
    let agent_2 = create_agent(&f, &alice, "乙", "a-k2").await;
    let group = create_group_with_members(
        &f,
        &alice,
        "项目群",
        "g-k",
        &[bob.id.clone(), agent_1.clone(), agent_2.clone()],
    )
    .await;

    // 普通群消息（无 mention）：成功且 History mentions 为空数组。
    let outcome = send(&f.message, &alice.id, &group, "普通消息", "m-1")
        .await
        .expect("ordinary group message");
    let SendMessageOutcome::Created { message } = outcome else {
        panic!("must be created");
    };
    assert_eq!(message.sequence, 1);
    assert!(message.mentions.is_empty());

    // @agent_1：mentions 规范排序返回。
    let outcome = send_with_mentions(
        &f.message,
        &alice.id,
        &group,
        "@甲 回答",
        vec![agent_1.clone()],
        "m-2",
    )
    .await
    .expect("mention one agent");
    let SendMessageOutcome::Created { message } = outcome else {
        panic!("must be created");
    };
    assert_eq!(message.sequence, 2);
    assert_eq!(message.mentions, vec![agent_1.clone()]);

    // 双 mention：存储层要求输入已规范排序（应用层负责排序/去重），
    // 这里显式模拟应用层结果：按 ID 升序。
    let mut sorted = vec![agent_1.clone(), agent_2.clone()];
    sorted.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    let outcome = send_with_mentions(
        &f.message,
        &alice.id,
        &group,
        "@两位 回答",
        sorted.clone(),
        "m-3",
    )
    .await
    .expect("mention two agents");
    let SendMessageOutcome::Created { message } = outcome else {
        panic!("must be created");
    };
    assert_eq!(
        message.mentions, sorted,
        "mentions round-trip canonically sorted"
    );

    // History 返回 mentions。
    let page = list(&f.message, &alice.id, &group, 100, None)
        .await
        .expect("history")
        .expect("visible");
    assert_eq!(page.messages.len(), 3);
    assert!(page.messages[0].message.mentions.is_empty());
    assert_eq!(page.messages[1].message.mentions, vec![agent_1.clone()]);
    assert_eq!(page.messages[2].message.mentions, sorted);

    // 数据库事实：mentions 行与 Message 同事务存在。
    let mentions_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mentions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(mentions_count, 3);
}

#[tokio::test]
async fn mention_idempotency_includes_mention_set() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    let (ca, cb) = crate::domain::contact::canonical_pair(&alice.id, &bob.id);
    sqlx::query(
        "INSERT INTO contact_periods (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at, idempotency_key) \
         VALUES ('00000000-0000-4000-8000-00000000d001', ?, ?, ?, ?, 'accepted', 1, 2, 'c-k')",
    )
    .bind(ca.as_str())
    .bind(cb.as_str())
    .bind(alice.id.as_str())
    .bind(bob.id.as_str())
    .execute(&f.pool)
    .await
    .expect("accepted contact");
    let agent_1 = create_agent(&f, &alice, "甲", "a-k1").await;
    let agent_2 = create_agent(&f, &alice, "乙", "a-k2").await;
    let group = create_group_with_members(
        &f,
        &alice,
        "群",
        "g-k",
        &[bob.id.clone(), agent_1.clone(), agent_2.clone()],
    )
    .await;

    // 同 key + 同 body + 同 mentions → 重放 200 语义（Replayed）。
    let first = send_with_mentions(
        &f.message,
        &alice.id,
        &group,
        "body",
        vec![agent_1.clone()],
        "m-key",
    )
    .await
    .expect("first");
    assert!(matches!(first, SendMessageOutcome::Created { .. }));
    let replay = send_with_mentions(
        &f.message,
        &alice.id,
        &group,
        "body",
        vec![agent_1.clone()],
        "m-key",
    )
    .await
    .expect("replay");
    assert!(matches!(replay, SendMessageOutcome::Replayed { .. }));

    // 同 key + 同 body + 不同 mention 集合 → 409 idempotency_conflict。
    let err = send_with_mentions(
        &f.message,
        &alice.id,
        &group,
        "body",
        vec![agent_2.clone()],
        "m-key",
    )
    .await
    .expect_err("changed mention set must conflict");
    assert!(
        matches!(
            err,
            crate::ports::message::MessageStoreError::IdempotencyConflict
        ),
        "changed mention set must be IdempotencyConflict, got {err:?}"
    );
    assert!(matches!(
        err,
        crate::ports::message::MessageStoreError::IdempotencyConflict
    ));

    // 同 key + 同 body + 空 mentions（原消息有 mention）→ 冲突。
    let err = send(&f.message, &alice.id, &group, "body", "m-key")
        .await
        .expect_err("dropped mention must conflict");
    assert!(matches!(
        err,
        crate::ports::message::MessageStoreError::IdempotencyConflict
    ));

    // 失败不留 Message / Mention / sequence。
    assert_eq!(count_messages(&f).await, 1);
    let mentions_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mentions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(mentions_count, 1);
}

#[tokio::test]
async fn invalid_mentions_are_rejected_atomically() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    let (ca, cb) = crate::domain::contact::canonical_pair(&alice.id, &bob.id);
    sqlx::query(
        "INSERT INTO contact_periods (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at, idempotency_key) \
         VALUES ('00000000-0000-4000-8000-00000000d001', ?, ?, ?, ?, 'accepted', 1, 2, 'c-k')",
    )
    .bind(ca.as_str())
    .bind(cb.as_str())
    .bind(alice.id.as_str())
    .bind(bob.id.as_str())
    .execute(&f.pool)
    .await
    .expect("accepted contact");
    let agent_1 = create_agent(&f, &alice, "甲", "a-k1").await;
    let agent_2 = create_agent(&f, &alice, "乙", "a-k2").await;

    // direct Room 不接受 mentions。
    let direct = create_room(&f, &alice, agent_1.as_str(), "r-k1").await;
    let err = send_with_mentions(
        &f.message,
        &alice.id,
        &direct,
        "hi",
        vec![agent_1.clone()],
        "dm-1",
    )
    .await
    .expect_err("direct room must reject mentions");
    assert!(matches!(
        err,
        crate::ports::message::MessageStoreError::InvalidMention
    ));

    // group 中 mention 非成员 Agent → InvalidMention。（Human 成员已不再是
    // 非法目标：见本函数末尾「active Human 成员是合法 mention 目标」一段。）
    let group =
        create_group_with_members(&f, &alice, "群", "g-k", &[bob.id.clone(), agent_1.clone()])
            .await;
    for (label, target) in [("non-member-agent", agent_2.clone())] {
        let err = send_with_mentions(
            &f.message,
            &alice.id,
            &group,
            "hi",
            vec![target],
            &format!("m-{label}"),
        )
        .await
        .expect_err("invalid mention target");
        assert!(
            matches!(
                err,
                crate::ports::message::MessageStoreError::InvalidMention
            ),
            "{label}"
        );
    }

    // 非法 mention 不得留下 Message/Mention/sequence：下一条合法消息
    // sequence 从 1 开始。
    let outcome = send(&f.message, &alice.id, &group, "合法", "m-ok")
        .await
        .expect("valid message after failures");
    let SendMessageOutcome::Created { message } = outcome else {
        panic!("must be created");
    };
    assert_eq!(
        message.sequence, 1,
        "no sequence consumed by failed mentions"
    );
    assert_eq!(count_messages(&f).await, 1);
    let mentions_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mentions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(mentions_count, 0);

    // Mention 目标放开窄切片（CORE_V1_MENTION_TARGETS.md）：该 group 的
    // active Human 成员 bob 是合法目标——成功提交、mentions 如实回显，且
    // 不生成任何 agent_event（只有 agent 目标才生成 Event）。
    let outcome = send_with_mentions(
        &f.message,
        &alice.id,
        &group,
        "@Bob 看这里",
        vec![bob.id.clone()],
        "m-human-member",
    )
    .await
    .expect("active human member is a legal mention target");
    let SendMessageOutcome::Created { message } = outcome else {
        panic!("must be created");
    };
    assert_eq!(
        message.sequence, 2,
        "the rejected mention consumed no sequence"
    );
    assert_eq!(message.mentions, vec![bob.id.clone()]);
    let mentions_after_human: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mentions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(mentions_after_human, 1, "human mention persisted");
    let events_after_human: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_events")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        events_after_human, 0,
        "human mention must not generate any agent event"
    );
}

/// 建立 64 个由 alice 拥有的 active Agent 并全部邀请进一个 group，返回
/// group Room ID 与 64 个 Agent ID（已按规范 ID 排序，直接可作为合法
/// Mention 目标集合）。
async fn group_with_64_agents(f: &Fixture, alice: &Participant) -> (RoomId, Vec<ParticipantId>) {
    let mut agents = Vec::with_capacity(MAX_MENTIONS_PER_MESSAGE);
    for i in 0..MAX_MENTIONS_PER_MESSAGE {
        let agent = create_agent(f, alice, &format!("甲{i}"), &format!("a-k-{i}")).await;
        agents.push(agent);
    }
    let group = create_group_with_members(f, alice, "大群", "g-k-64", &agents).await;
    let mut sorted = agents.clone();
    sorted.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    (group, sorted)
}

#[tokio::test]
async fn store_rejects_mentions_over_cap_without_side_effects() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let agent_id = create_agent(&f, &alice, "助手", "a-k").await;
    let group =
        create_group_with_members(&f, &alice, "群", "g-k", std::slice::from_ref(&agent_id)).await;

    let over_cap = vec![agent_id.clone(); MAX_MENTIONS_PER_MESSAGE + 1];
    let err = send_with_mentions(&f.message, &alice.id, &group, "超量", over_cap, "m-1")
        .await
        .expect_err("mentions over the cap must be rejected");
    assert!(
        matches!(
            err,
            crate::ports::message::MessageStoreError::InvalidMention
        ),
        "expected InvalidMention, got {err}"
    );
    // 无 Message / Mention / Event / sequence 副作用：下一条合法消息
    // sequence 从 1 开始。
    let outcome = send(&f.message, &alice.id, &group, "合法", "m-ok")
        .await
        .expect("valid message after over-cap rejection");
    let SendMessageOutcome::Created { message } = outcome else {
        panic!("must be created");
    };
    assert_eq!(message.sequence, 1, "no sequence consumed");
    assert_eq!(count_messages(&f).await, 1);
    let mentions_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mentions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(mentions_count, 0);
    let events_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_events")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(events_count, 0);
}

#[tokio::test]
async fn store_mention_cap_is_request_shape_check_preceding_idempotency() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let agent_id = create_agent(&f, &alice, "助手", "a-k").await;
    let group =
        create_group_with_members(&f, &alice, "群", "g-k", std::slice::from_ref(&agent_id)).await;

    // 先建立一条既有幂等事实。
    let outcome = send(&f.message, &alice.id, &group, "hello", "m-key")
        .await
        .expect("first message");
    assert!(matches!(outcome, SendMessageOutcome::Created { .. }));

    // 同 key + 超量 Mention：请求形状校验先于幂等重放裁决，返回
    // InvalidMention 而不是 Replayed/IdempotencyConflict（与 application
    // 层正文校验先于幂等查找的顺序一致；超限请求不可能存在合法历史，
    // 因此不影响既有重放契约）。
    let over_cap = vec![agent_id.clone(); MAX_MENTIONS_PER_MESSAGE + 1];
    let err = send_with_mentions(&f.message, &alice.id, &group, "hello", over_cap, "m-key")
        .await
        .expect_err("over-cap shape must win over replay");
    assert!(
        matches!(
            err,
            crate::ports::message::MessageStoreError::InvalidMention
        ),
        "expected InvalidMention, got {err}"
    );
    assert_eq!(count_messages(&f).await, 1, "no new facts");
}

#[tokio::test]
async fn exactly_64_mentions_succeed_with_64_mentions_and_64_events() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let (group, sorted_agents) = group_with_64_agents(&f, &alice).await;

    let outcome = send_with_mentions(
        &f.message,
        &alice.id,
        &group,
        "@全员 回答",
        sorted_agents.clone(),
        "m-64",
    )
    .await
    .expect("exactly 64 mentions must succeed");
    let SendMessageOutcome::Created { message } = outcome else {
        panic!("must be created");
    };
    assert_eq!(message.sequence, 1);
    assert_eq!(message.mentions, sorted_agents);

    let mentions_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mentions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(mentions_count as usize, MAX_MENTIONS_PER_MESSAGE);
    // 每条合法 Mention 生成恰好一条 Agent Event。
    let events_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_events")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(events_count as usize, MAX_MENTIONS_PER_MESSAGE);
    let distinct_targets: i64 =
        sqlx::query_scalar("SELECT COUNT(DISTINCT target_agent_id) FROM agent_events")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(distinct_targets as usize, MAX_MENTIONS_PER_MESSAGE);

    // 同集合幂等重放仍成立，不新增事实。
    let replay = send_with_mentions(
        &f.message,
        &alice.id,
        &group,
        "@全员 回答",
        sorted_agents,
        "m-64",
    )
    .await
    .expect("replay of exactly-64 mention set");
    assert!(matches!(replay, SendMessageOutcome::Replayed { .. }));
    let mentions_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mentions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(mentions_after as usize, MAX_MENTIONS_PER_MESSAGE);
}

#[tokio::test]
async fn database_mention_cap_matches_shared_rust_contract() {
    let f = fixture().await;
    let trigger_sql: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'mentions_insert_max_guard'",
    )
    .fetch_one(&f.pool)
    .await
    .expect("mention cap trigger must exist");
    let normalized = trigger_sql.split_whitespace().collect::<Vec<_>>().join(" ");
    let expected_threshold =
        format!("WHERE message_id = NEW.message_id) >= {MAX_MENTIONS_PER_MESSAGE}");
    assert!(
        normalized.contains(&expected_threshold),
        "SQLite mention cap must match Rust MAX_MENTIONS_PER_MESSAGE={MAX_MENTIONS_PER_MESSAGE}; trigger={normalized}"
    );
}

#[tokio::test]
async fn database_rejects_65th_mention_without_harming_existing_facts() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let mut agents = Vec::with_capacity(MAX_MENTIONS_PER_MESSAGE + 1);
    for i in 0..=MAX_MENTIONS_PER_MESSAGE {
        let agent = create_agent(&f, &alice, &format!("甲{i}"), &format!("a-k-{i}")).await;
        agents.push(agent);
    }
    // 第 65 个 Agent 是合法成员但不在首批 64 个 Mention 集合内。
    let sixty_fifth = agents[MAX_MENTIONS_PER_MESSAGE].clone();
    let sixty_four: Vec<ParticipantId> = agents[..MAX_MENTIONS_PER_MESSAGE].to_vec();
    let group = create_group_with_members(&f, &alice, "大群", "g-k", &agents).await;

    // 先经 Store 提交恰好 64 条 Mention 的合法 Message。
    let outcome = send_with_mentions(
        &f.message,
        &alice.id,
        &group,
        "@第一批 回答",
        sixty_four.clone(),
        "m-64",
    )
    .await
    .expect("legal 64-mention message");
    let SendMessageOutcome::Created { message } = outcome else {
        panic!("must be created");
    };
    assert_eq!(message.sequence, 1);

    // 直接 SQL 插入第 65 条 Mention（目标合法、非重复）：被数据库兜底拒绝，
    // 既有 64 条与 source Message 不受误伤。
    let err = sqlx::query("INSERT INTO mentions (message_id, target_agent_id) VALUES (?, ?)")
        .bind(message.id.as_str())
        .bind(sixty_fifth.as_str())
        .execute(&f.pool)
        .await
        .expect_err("65th mention must be rejected by the database");
    assert!(
        err.to_string().contains("at most 64 mentions"),
        "unexpected database error: {err}"
    );
    let mentions_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mentions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(mentions_count as usize, MAX_MENTIONS_PER_MESSAGE);
    assert_eq!(count_messages(&f).await, 1);
    let events_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_events")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(events_count as usize, MAX_MENTIONS_PER_MESSAGE);

    // 同事务插入 65 条 Mention（Message 先插入）：第 65 条失败使整个事务
    // 回滚，不留下 Message、Mention、Event 或 sequence。
    let overflow_tx_result = async {
        let mut conn = f.pool.acquire().await.unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *conn)
            .await
            .unwrap();
        let new_message_id = uuid::Uuid::new_v4().to_string();
        let result: Result<(), ()> = async {
            sqlx::query(
                "INSERT INTO messages \
                 (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
                 VALUES (?, ?, 2, ?, 'overflow', 'm-65', 1)",
            )
            .bind(&new_message_id)
            .bind(group.as_str())
            .bind(alice.id.as_str())
            .execute(&mut *conn)
            .await
            .map_err(|_| ())?;
            for target in sixty_four.iter().chain(std::iter::once(&sixty_fifth)) {
                sqlx::query("INSERT INTO mentions (message_id, target_agent_id) VALUES (?, ?)")
                    .bind(&new_message_id)
                    .bind(target.as_str())
                    .execute(&mut *conn)
                    .await
                    .map_err(|_| ())?;
            }
            Ok(())
        }
        .await;
        sqlx::query("ROLLBACK").execute(&mut *conn).await.unwrap();
        result
    }
    .await;
    assert!(
        overflow_tx_result.is_err(),
        "65-mention transaction must fail"
    );
    assert_eq!(count_messages(&f).await, 1, "overflow message rolled back");
    let mentions_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mentions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(mentions_after as usize, MAX_MENTIONS_PER_MESSAGE);
    let events_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_events")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(events_after as usize, MAX_MENTIONS_PER_MESSAGE);
    // sequence 未被占用：下一条合法 Message 从 2 开始。
    let outcome = send(&f.message, &alice.id, &group, "合法", "m-ok")
        .await
        .expect("valid message after rolled-back overflow");
    let SendMessageOutcome::Created { message } = outcome else {
        panic!("must be created");
    };
    assert_eq!(message.sequence, 2, "no sequence consumed by rollback");
}

/// 周期窗口验收（CORE_V1_HUMAN_ROOMS_AND_MENTIONS 群 Membership 周期）：
/// B 建群并邀请 A；A 在群期间发送 1-5；A 退群；群内继续产生 6-10；A 用
/// 新 key 重入后产生 11-15；A 的 History 只含 1-5 与 11-15；分页跨 gap
/// 无重复、无跳过、无 gap 消息泄露；B 全程可见 1-15；离群期间 A 的读写
/// 全部 404；旧 key 重放只返回历史 closed 周期。
#[tokio::test]
async fn history_windows_follow_membership_periods_without_gap_leakage() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;

    // B 与 A 的 accepted 联系人（直接 fixture 准备，公开 invite 路径要求）。
    let (a, b) = crate::domain::contact::canonical_pair(&alice.id, &bob.id);
    sqlx::query(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at, idempotency_key) \
         VALUES ('00000000-0000-4000-8000-00000000d003', ?, ?, ?, ?, 'accepted', 1, 1, 'k-ab')",
    )
    .bind(a.as_str())
    .bind(b.as_str())
    .bind(a.as_str())
    .bind(b.as_str())
    .execute(&f.pool)
    .await
    .expect("insert accepted contact fixture");

    // B 建群并邀请 A（invite 必带 key）。
    let group_id = room_application::create_group(&f.room, &bob, "窗口群", "g-k")
        .await
        .expect("create group")
        .room_id();
    let invited = room_application::invite_group_member(
        &f.room,
        &bob,
        group_id.as_str(),
        alice.id.as_str(),
        "inv-a-1",
    )
    .await
    .expect("invite A");
    let crate::application::room::InviteMemberOutcome::Created {
        membership: first_period,
    } = invited
    else {
        panic!("first invite must be Created");
    };
    assert_eq!(first_period.join_after_sequence, 0);

    // A 在群期间发送 1-5。
    for i in 1..=5 {
        let outcome = send(
            &f.message,
            &alice.id,
            &group_id,
            &format!("a{i}"),
            &format!("a-m-{i}"),
        )
        .await
        .expect("A sends while member");
        let SendMessageOutcome::Created { message } = outcome else {
            panic!("must be created");
        };
        assert_eq!(message.sequence, i);
    }

    // A 主动退群：离群后对 Room 的所有读写 404。
    room_application::leave_membership(&f.room, &alice.id, group_id.as_str())
        .await
        .expect("A leaves");
    assert!(
        crate::ports::room::RoomStore::get_room(&f.room, &alice.id, &group_id)
            .await
            .expect("get")
            .is_none(),
        "A must lose room access after leaving"
    );
    assert!(
        matches!(
            list(&f.message, &alice.id, &group_id, 50, None)
                .await
                .expect_err("A must not read history while out"),
            // 切片 B2：Room 存在但 reader 非 active member → 明确 NotAMember
            // （HTTP 层：Human 路径 404、Backend agent 历史端点 403）。
            crate::ports::message::MessageStoreError::NotAMember
        ),
        "A must not read history while out"
    );
    assert!(
        send(&f.message, &alice.id, &group_id, "out", "a-m-out")
            .await
            .is_err(),
        "A must not send while out"
    );

    // 群内继续产生 6-10（B 发送）。
    for i in 6..=10 {
        let outcome = send(
            &f.message,
            &bob.id,
            &group_id,
            &format!("b{i}"),
            &format!("b-m-{i}"),
        )
        .await
        .expect("B sends");
        let SendMessageOutcome::Created { message } = outcome else {
            panic!("must be created");
        };
        assert_eq!(message.sequence, i);
    }

    // 旧 key 重放只返回历史 closed 周期，绝不误开新周期。
    let replayed = room_application::invite_group_member(
        &f.room,
        &bob,
        group_id.as_str(),
        alice.id.as_str(),
        "inv-a-1",
    )
    .await
    .expect("old key replay");
    let crate::application::room::InviteMemberOutcome::Replayed { membership } = replayed else {
        panic!("old key must replay the historical period");
    };
    assert_eq!(membership.membership_id, first_period.membership_id);
    assert_eq!(
        membership.closed_by,
        Some(crate::domain::room::MembershipClosedBy::Leave)
    );

    // A 用新 key 重入（join_after_sequence=10，只对之后的消息可见）。
    let rejoined = room_application::invite_group_member(
        &f.room,
        &bob,
        group_id.as_str(),
        alice.id.as_str(),
        "inv-a-2",
    )
    .await
    .expect("rejoin with new key");
    let crate::application::room::InviteMemberOutcome::Created {
        membership: second_period,
    } = rejoined
    else {
        panic!("rejoin must be Created");
    };
    assert_eq!(second_period.join_after_sequence, 10);

    // 重入后 A 与 B 产生 11-15。
    for i in 11..=15 {
        let outcome = send(
            &f.message,
            &alice.id,
            &group_id,
            &format!("a{i}"),
            &format!("a-m-{i}"),
        )
        .await
        .expect("A sends after rejoin");
        let SendMessageOutcome::Created { message } = outcome else {
            panic!("must be created");
        };
        assert_eq!(message.sequence, i);
    }

    // A 的 History 只含 1-5 与 11-15；分页跨 gap 无重复、无跳过、无泄露。
    let mut visible: Vec<i64> = Vec::new();
    let mut next_before: Option<i64> = None;
    loop {
        let page = list(&f.message, &alice.id, &group_id, 3, next_before)
            .await
            .expect("A page")
            .expect("A must have an active period");
        for message in &page.messages {
            visible.push(message.message.sequence);
        }
        match page.next_before {
            Some(next) => next_before = Some(next),
            None => break,
        }
    }
    visible.sort_unstable();
    assert_eq!(
        visible,
        vec![1, 2, 3, 4, 5, 11, 12, 13, 14, 15],
        "A sees only its own membership windows, gap never leaks"
    );
    assert_eq!(
        visible.iter().filter(|&&s| (6..=10).contains(&s)).count(),
        0,
        "no out-of-membership message may leak into A's history"
    );
    // 首屏 = 最新一页（13,14,15），next_before=13。
    let first_page = list(&f.message, &alice.id, &group_id, 3, None)
        .await
        .expect("first page")
        .expect("active");
    let first_seqs: Vec<i64> = first_page
        .messages
        .iter()
        .map(|m| m.message.sequence)
        .collect();
    assert_eq!(first_seqs, vec![13, 14, 15]);
    assert_eq!(first_page.next_before, Some(13));

    // B（owner，全程 active）可见 1-15。
    let b_page = list(&f.message, &bob.id, &group_id, 100, None)
        .await
        .expect("B page")
        .expect("B active");
    let b_seqs: Vec<i64> = b_page.messages.iter().map(|m| m.message.sequence).collect();
    assert_eq!(
        b_seqs,
        (1..=15).collect::<Vec<i64>>(),
        "B sees the full timeline"
    );

    // 同 Room+Participant 至多一个 active 周期：A 现在只有一条 active。
    let active_periods: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(active_periods, 1);
}

/// 让 CreateRoomOutcome 可读 Room ID 的辅助。
impl crate::application::room::CreateRoomOutcome {
    fn room_id(&self) -> RoomId {
        match self {
            crate::application::room::CreateRoomOutcome::Created { room, .. }
            | crate::application::room::CreateRoomOutcome::Replayed { room, .. } => room.id.clone(),
        }
    }
}

// ===== Reply / Quote 切片（CORE_V1_REPLIES_AND_QUOTES.md）=====

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
async fn reply_round_trips_through_send_history_and_column() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let agent_id = create_agent(&f, &alice, "助手", "a-k").await;
    let room_id = create_room(&f, &alice, agent_id.as_str(), "r-k").await;

    // 目标 Message（普通消息，reply_to_message_id = None）。
    let target = send(&f.message, &alice.id, &room_id, "第一条", "m-1")
        .await
        .expect("target send");
    let SendMessageOutcome::Created { message: target } = target else {
        panic!("target must be created");
    };
    assert_eq!(target.reply_to_message_id, None);
    assert_eq!(target.sequence, 1);

    // 合法回复：写入成功，事实携带 target ID。
    let reply = send_reply(
        &f.message,
        &alice.id,
        &room_id,
        "回复第一条",
        &target.id,
        "m-2",
    )
    .await
    .expect("valid reply must succeed");
    let SendMessageOutcome::Created { message: reply_msg } = reply else {
        panic!("reply must be created");
    };
    assert_eq!(reply_msg.sequence, 2);
    assert_eq!(
        reply_msg.reply_to_message_id,
        Some(target.id.clone()),
        "response must carry the reply target"
    );

    // reply_to_message_id 直接固化在 messages 行的不可变列：同一行插入，
    // 内核读取路径与 History 读同一列，无第二张关系表。
    let column_target: String =
        sqlx::query_scalar("SELECT reply_to_message_id FROM messages WHERE id = ?")
            .bind(reply_msg.id.as_str())
            .fetch_one(f.store.pool())
            .await
            .expect("reply message must store the target in its own row");
    assert_eq!(column_target, target.id.as_str());

    // History 只返回 target ID，不嵌入 target 正文。
    let page = list(&f.message, &alice.id, &room_id, 50, None)
        .await
        .expect("history")
        .expect("visible");
    let target_view = page
        .messages
        .iter()
        .find(|h| h.message.id == target.id)
        .expect("target row");
    let reply_view = page
        .messages
        .iter()
        .find(|h| h.message.id == reply_msg.id)
        .expect("reply row");
    assert_eq!(target_view.message.reply_to_message_id, None);
    assert_eq!(
        reply_view.message.reply_to_message_id,
        Some(target.id.clone())
    );
    assert_eq!(
        reply_view.message.body.as_ref().unwrap().as_str(),
        "回复第一条"
    );

    // 同 key 同完整请求 → 重放：同一 Message ID / sequence / reply target。
    let replayed = send_reply(
        &f.message,
        &alice.id,
        &room_id,
        "回复第一条",
        &target.id,
        "m-2",
    )
    .await
    .expect("same request replay");
    let SendMessageOutcome::Replayed { message: replayed } = replayed else {
        panic!("must replay");
    };
    assert_eq!(replayed.id, reply_msg.id);
    assert_eq!(replayed.sequence, 2);
    assert_eq!(replayed.reply_to_message_id, Some(target.id.clone()));
    assert_eq!(count_messages(&f).await, 2, "replay must not add facts");

    // 同 key 改 reply target → 冲突，既有事实不变。
    let other = send(&f.message, &alice.id, &room_id, "第三条", "m-3")
        .await
        .expect("third message");
    let SendMessageOutcome::Created { message: other_msg } = other else {
        panic!("must be created");
    };
    let conflict = send_reply(
        &f.message,
        &alice.id,
        &room_id,
        "回复第一条",
        &other_msg.id,
        "m-2",
    )
    .await
    .expect_err("same key with different reply target must conflict");
    assert!(
        matches!(
            conflict,
            crate::ports::message::MessageStoreError::IdempotencyConflict
        ),
        "must be idempotency conflict: {conflict}"
    );
    let still: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE reply_to_message_id IS NOT NULL")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(still, 1, "conflict must not change the stored reply target");
    assert_eq!(count_messages(&f).await, 3);
}

#[tokio::test]
async fn reply_to_own_previous_message_is_allowed() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let agent_id = create_agent(&f, &alice, "助手", "a-k").await;
    let room_id = create_room(&f, &alice, agent_id.as_str(), "r-k").await;

    let first = send(&f.message, &alice.id, &room_id, "自述", "m-1")
        .await
        .expect("first");
    let SendMessageOutcome::Created { message: first } = first else {
        panic!("must be created");
    };
    let second = send_reply(&f.message, &alice.id, &room_id, "自回复", &first.id, "m-2")
        .await
        .expect("self reply must be allowed in v1");
    let SendMessageOutcome::Created { message: second } = second else {
        panic!("must be created");
    };
    assert_eq!(second.reply_to_message_id, Some(first.id.clone()));
    let replies: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE reply_to_message_id IS NOT NULL")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(replies, 1, "exactly one reply, no body copy");
}

#[tokio::test]
async fn reply_to_unknown_or_cross_room_target_is_invalid_reply_target() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let agent_1 = create_agent(&f, &alice, "甲", "a-k1").await;
    let agent_2 = create_agent(&f, &alice, "乙", "a-k2").await;
    let room_a = create_room(&f, &alice, agent_1.as_str(), "r-a").await;
    let room_b = create_room(&f, &alice, agent_2.as_str(), "r-b").await;

    let target = send(&f.message, &alice.id, &room_a, "A 里的话", "m-a")
        .await
        .expect("target");
    let SendMessageOutcome::Created { message: target } = target else {
        panic!("must be created");
    };

    // 未知 target：统一 InvalidReplyTarget，不留任何事实。
    let unknown = MessageId::new_v4();
    let err = send_reply(&f.message, &alice.id, &room_a, "回", &unknown, "m-x")
        .await
        .expect_err("unknown target must be rejected");
    assert!(
        matches!(
            err,
            crate::ports::message::MessageStoreError::InvalidReplyTarget
        ),
        "must be invalid reply target: {err}"
    );

    // 跨 Room target：同一 actor 在 B Room 引用 A Room 的消息。
    let err = send_reply(&f.message, &alice.id, &room_b, "回", &target.id, "m-y")
        .await
        .expect_err("cross-room target must be rejected");
    assert!(
        matches!(
            err,
            crate::ports::message::MessageStoreError::InvalidReplyTarget
        ),
        "must be invalid reply target: {err}"
    );

    // 无副作用：失败不留下 Message、reply target、sequence gap。
    assert_eq!(count_messages(&f).await, 1);
    let replies: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE reply_to_message_id IS NOT NULL")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(replies, 0);
    let next = send(&f.message, &alice.id, &room_b, "B 里的消息", "m-b")
        .await
        .expect("next send");
    let SendMessageOutcome::Created { message: next } = next else {
        panic!("must be created");
    };
    assert_eq!(
        next.sequence, 1,
        "failed reply must not consume the sequence"
    );
}

#[tokio::test]
async fn reply_target_must_be_within_actor_membership_window() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    let (ca, cb) = crate::domain::contact::canonical_pair(&alice.id, &bob.id);
    sqlx::query(
        "INSERT INTO contact_periods (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at, idempotency_key) \
         VALUES ('00000000-0000-4000-8000-00000000d002', ?, ?, ?, ?, 'accepted', 1, 1, 'k-ab')",
    )
    .bind(ca.as_str())
    .bind(cb.as_str())
    .bind(alice.id.as_str())
    .bind(bob.id.as_str())
    .execute(&f.pool)
    .await
    .expect("accepted contact");

    // Bob 建群并先发一条消息，之后才邀请 Alice：seq 1 落在 Alice 加入前。
    let group = room_application::create_group(&f.room, &bob, "窗口群", "g-k")
        .await
        .expect("create group")
        .room_id();
    let before_join = send(&f.message, &bob.id, &group, "加入前", "b-1")
        .await
        .expect("pre-join message");
    let SendMessageOutcome::Created {
        message: before_join,
    } = before_join
    else {
        panic!("must be created");
    };
    room_application::invite_group_member(&f.room, &bob, group.as_str(), alice.id.as_str(), "i-a")
        .await
        .expect("invite A");

    // 回复首次加入前的消息 → InvalidReplyTarget，无副作用。
    let err = send_reply(&f.message, &alice.id, &group, "回", &before_join.id, "a-1")
        .await
        .expect_err("pre-join target must be rejected");
    assert!(
        matches!(
            err,
            crate::ports::message::MessageStoreError::InvalidReplyTarget
        ),
        "must be invalid reply target: {err}"
    );

    // 加入后的消息可见 → 合法。
    let after_join = send(&f.message, &bob.id, &group, "加入后", "b-2")
        .await
        .expect("post-join message");
    let SendMessageOutcome::Created {
        message: after_join,
    } = after_join
    else {
        panic!("must be created");
    };
    let ok = send_reply(&f.message, &alice.id, &group, "回", &after_join.id, "a-2")
        .await
        .expect("post-join target must be visible");
    let SendMessageOutcome::Created { .. } = ok else {
        panic!("must be created");
    };

    // Alice 离群，群内产生新消息，Alice 重入后再回复离群期间消息 →
    // InvalidReplyTarget；仍可回复自己历史窗口内的消息。
    room_application::leave_membership(&f.room, &alice.id, group.as_str())
        .await
        .expect("A leaves");
    let during_absence = send(&f.message, &bob.id, &group, "离群期间", "b-3")
        .await
        .expect("message during absence");
    let SendMessageOutcome::Created {
        message: during_absence,
    } = during_absence
    else {
        panic!("must be created");
    };
    room_application::invite_group_member(
        &f.room,
        &bob,
        group.as_str(),
        alice.id.as_str(),
        "i-a-2",
    )
    .await
    .expect("A rejoins");
    let err = send_reply(
        &f.message,
        &alice.id,
        &group,
        "回",
        &during_absence.id,
        "a-3",
    )
    .await
    .expect_err("message from the absence period must be rejected");
    assert!(
        matches!(
            err,
            crate::ports::message::MessageStoreError::InvalidReplyTarget
        ),
        "must be invalid reply target: {err}"
    );
    let ok = send_reply(&f.message, &alice.id, &group, "回旧", &after_join.id, "a-4")
        .await
        .expect("own historical window target must stay visible after rejoin");
    let SendMessageOutcome::Created { .. } = ok else {
        panic!("must be created");
    };
}

#[tokio::test]
async fn reply_target_failure_rolls_back_message_and_sequence() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let agent_1 = create_agent(&f, &alice, "甲", "a-k1").await;
    let agent_2 = create_agent(&f, &alice, "乙", "a-k2").await;
    let room_a = create_room(&f, &alice, agent_1.as_str(), "r-a").await;
    let room_b = create_room(&f, &alice, agent_2.as_str(), "r-b").await;

    let target = send(&f.message, &alice.id, &room_a, "目标", "m-a")
        .await
        .expect("target");
    let SendMessageOutcome::Created { message: target } = target else {
        panic!("must be created");
    };

    // 跨 Room 回复失败：Message、reply target、sequence 全部回滚。
    let err = send_reply(&f.message, &alice.id, &room_b, "回", &target.id, "m-bad")
        .await
        .expect_err("cross-room reply must fail");
    assert!(matches!(
        err,
        crate::ports::message::MessageStoreError::InvalidReplyTarget
    ));
    assert_eq!(count_messages(&f).await, 1, "no orphan message");
    let replies: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE reply_to_message_id IS NOT NULL")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(replies, 0, "no orphan reply target");

    // 失败的回复不占用 sequence：Room B 的下一条仍从 1 开始。
    let next = send(&f.message, &alice.id, &room_b, "正常", "m-b-ok")
        .await
        .expect("next");
    let SendMessageOutcome::Created { message: next } = next else {
        panic!("must be created");
    };
    assert_eq!(next.sequence, 1);
    // 事务取消后连接可复用：失败请求的 key 未留下任何幂等事实，同 key 的
    // 新请求正常创建新 Message。
    let again = send(&f.message, &alice.id, &room_a, "后续", "m-bad")
        .await
        .expect("connection stays usable");
    let SendMessageOutcome::Created { message: again } = again else {
        panic!("failed key must not have recorded an idempotency fact");
    };
    assert_eq!(again.sequence, 2);
}

#[tokio::test]
async fn group_reply_with_mentions_commits_reply_field_and_events() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    let (ca, cb) = crate::domain::contact::canonical_pair(&alice.id, &bob.id);
    sqlx::query(
        "INSERT INTO contact_periods (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at, idempotency_key) \
         VALUES ('00000000-0000-4000-8000-00000000d001', ?, ?, ?, ?, 'accepted', 1, 2, 'c-k')",
    )
    .bind(ca.as_str())
    .bind(cb.as_str())
    .bind(alice.id.as_str())
    .bind(bob.id.as_str())
    .execute(&f.pool)
    .await
    .expect("accepted contact");
    let agent_1 = create_agent(&f, &alice, "甲", "a-k1").await;
    let group = create_group_with_members(
        &f,
        &alice,
        "项目群",
        "g-k",
        &[bob.id.clone(), agent_1.clone()],
    )
    .await;

    let target = send(&f.message, &alice.id, &group, "问题", "m-1")
        .await
        .expect("target");
    let SendMessageOutcome::Created { message: target } = target else {
        panic!("must be created");
    };
    let outcome = f
        .message
        .send_message(
            &NewMessage {
                id: MessageId::new_v4(),
                room_id: group.clone(),
                actor_id: alice.id.clone(),
                body: Some(MessageBody::parse("回复并@").unwrap()),
                mentions: vec![agent_1.clone()],
                attachments: Vec::new(),
                reply_to_message_id: Some(target.id.clone()),
                created_at: UtcMillis::now(),
            },
            &IdempotencyKey::parse("m-2").unwrap(),
        )
        .await
        .expect("group reply with mention");
    let SendMessageOutcome::Created { message: reply } = outcome else {
        panic!("must be created");
    };
    assert_eq!(reply.sequence, 2);
    assert_eq!(reply.reply_to_message_id, Some(target.id.clone()));
    assert_eq!(reply.mentions, vec![agent_1.clone()]);

    // reply 字段 + Mention + Event 同事务成立：恰好 1 条带 reply 的消息、
    // 1 条 Mention、1 条指向 agent_1 的 Event（message.created 之外 group
    // 路径不重复）。
    let replies: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE reply_to_message_id IS NOT NULL")
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(replies, 1);
    let mention_events: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_events WHERE target_agent_id = ?")
            .bind(agent_1.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(
        mention_events, 1,
        "mention generates exactly one agent event"
    );
    let (mention_message,): (String,) =
        sqlx::query_as("SELECT message_id FROM mentions WHERE target_agent_id = ?")
            .bind(agent_1.as_str())
            .fetch_one(f.store.pool())
            .await
            .unwrap();
    assert_eq!(mention_message, reply.id.as_str());
}

// ===== Message cursor 解析（resolve_message_before）=====

#[tokio::test]
async fn resolve_message_cursor_is_room_and_membership_scoped() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(&f.pool, "Bob").await;
    let agent_id = create_agent(&f, &alice.participant, "助手", "a-k").await;
    let room_id = create_room(&f, &alice.participant, agent_id.as_str(), "r-k").await;

    // 发送 1..=3；跨 Room 的第二条消息 msg-other。
    let mut ids = Vec::new();
    for i in 1..=3 {
        let outcome = send(
            &f.message,
            &alice.participant.id,
            &room_id,
            &format!("msg {i}"),
            &format!("m-{i}"),
        )
        .await
        .expect("send");
        ids.push(message_of(&outcome).id.clone());
    }
    let other_agent = create_agent(&f, &alice.participant, "乙", "a-k2").await;
    let other_room = create_room(&f, &alice.participant, other_agent.as_str(), "r-k2").await;
    let other_outcome = send(
        &f.message,
        &alice.participant.id,
        &other_room,
        "other room",
        "m-other",
    )
    .await
    .expect("send other room");
    let other_id = message_of(&other_outcome).id.clone();

    // 同 Room 可见 cursor：解析回该 Message 的 sequence。
    for (i, id) in ids.iter().enumerate() {
        let seq = f
            .message
            .resolve_message_before(&alice.participant.id, &room_id, id)
            .await
            .expect("resolve")
            .expect("visible");
        assert_eq!(seq, i as i64 + 1, "cursor {id}");
    }

    // 跨 Room cursor：同一 Message 在另一个 Room 上不可解析（None）。
    assert_eq!(
        f.message
            .resolve_message_before(&alice.participant.id, &other_room, &ids[0])
            .await
            .expect("resolve"),
        None,
        "cross-room cursor must not resolve"
    );
    assert_eq!(
        f.message
            .resolve_message_before(&alice.participant.id, &room_id, &other_id)
            .await
            .expect("resolve"),
        None,
        "other-room message must not resolve in this room"
    );

    // 未知 Message ID：None（与不可见形状一致，不泄露存在性）。
    assert_eq!(
        f.message
            .resolve_message_before(&alice.participant.id, &room_id, &MessageId::new_v4())
            .await
            .expect("resolve"),
        None
    );

    // 非成员 reader：即使 Message 存在也不可解析（None）。
    assert_eq!(
        f.message
            .resolve_message_before(&bob.id, &room_id, &ids[0])
            .await
            .expect("resolve"),
        None,
        "non-member cursor must not resolve"
    );
}

#[tokio::test]
async fn resolve_message_cursor_respects_historical_membership_windows() {
    // 群场景：A 在群期间的消息、离群间隙的消息、重入后的消息；
    // 对 A 而言，间隙消息（从未可见）的 cursor 不可解析。
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.unwrap();
    let bob = add_human(&f.pool, "Bob").await;
    let alice_p = &alice.participant;

    // Bob 与 Alice 的 accepted 联系人 fixture。
    let (a, b) = crate::domain::contact::canonical_pair(&alice_p.id, &bob.id);
    sqlx::query(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at, idempotency_key) \
         VALUES ('00000000-0000-4000-8000-00000000d003', ?, ?, ?, ?, 'accepted', 1, 1, 'k-ab')",
    )
    .bind(a.as_str())
    .bind(b.as_str())
    .bind(a.as_str())
    .bind(b.as_str())
    .execute(&f.pool)
    .await
    .expect("insert accepted contact fixture");

    // Bob 建群并邀请 Alice。
    let group_id = room_application::create_group(&f.room, &bob, "窗口群", "g-k")
        .await
        .expect("create group")
        .room_id();
    let _ = room_application::invite_group_member(
        &f.room,
        &bob,
        group_id.as_str(),
        alice_p.id.as_str(),
        "inv-a-1",
    )
    .await
    .expect("invite A");

    let mut in_period = Vec::new();
    let mut gap = Vec::new();
    let mut after_rejoin = Vec::new();
    for i in 1..=3 {
        let outcome = send(
            &f.message,
            &alice_p.id,
            &group_id,
            &format!("a{i}"),
            &format!("a-m-{i}"),
        )
        .await
        .expect("A sends");
        in_period.push(message_of(&outcome).id.clone());
    }
    room_application::leave_membership(&f.room, &alice_p.id, group_id.as_str())
        .await
        .expect("A leaves");
    for i in 4..=6 {
        let outcome = send(
            &f.message,
            &bob.id,
            &group_id,
            &format!("b{i}"),
            &format!("b-m-{i}"),
        )
        .await
        .expect("B sends during A's gap");
        gap.push(message_of(&outcome).id.clone());
    }
    let _ = room_application::invite_group_member(
        &f.room,
        &bob,
        group_id.as_str(),
        alice_p.id.as_str(),
        "inv-a-2",
    )
    .await
    .expect("A rejoins");
    for i in 7..=8 {
        let outcome = send(
            &f.message,
            &alice_p.id,
            &group_id,
            &format!("a{i}"),
            &format!("a-m-{i}"),
        )
        .await
        .expect("A sends after rejoin");
        after_rejoin.push(message_of(&outcome).id.clone());
    }

    for id in in_period.iter().chain(after_rejoin.iter()) {
        assert!(
            f.message
                .resolve_message_before(&alice_p.id, &group_id, id)
                .await
                .expect("resolve")
                .is_some(),
            "message inside a membership window must resolve"
        );
    }
    for id in &gap {
        assert_eq!(
            f.message
                .resolve_message_before(&alice_p.id, &group_id, id)
                .await
                .expect("resolve"),
            None,
            "gap message must not resolve (never visible to A)"
        );
    }
    // 首个周期与重入周期都可见：解析结果 = 各自内部 sequence（1/2/3 与 7/8）。
    assert_eq!(
        f.message
            .resolve_message_before(&alice_p.id, &group_id, &in_period[0])
            .await
            .expect("resolve"),
        Some(1)
    );
    assert_eq!(
        f.message
            .resolve_message_before(&alice_p.id, &group_id, &after_rejoin[1])
            .await
            .expect("resolve"),
        Some(8)
    );
}
