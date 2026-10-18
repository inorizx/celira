//! SQLite Attachment adapter 测试（真实临时 SQLite + 真实 migration）：
//! create_pending 的成员资格守卫、下载授权矩阵（绑定/未绑定/跨 Room/
//! 跨 actor/撤回/非成员/窗口外）、purge_expired_pending（未过期保留、
//! 过期无绑定回收、过期有绑定/已 bound 拒绝、并发清理与绑定线性化、
//! 返回 key 的字节清理、DELETE 守卫只放行清理条件）、
//! referenced_storage_keys，以及 Message 绑定链路的同事务原子性与数据库
//! 守卫（body-or-attachments、绑定一致性、状态机、不可变、无孤儿绑定）。

use std::path::PathBuf;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::attachment_store::SqliteAttachmentStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::message_store::SqliteMessageStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::room_store::SqliteRoomStore;
use crate::application::{agent, identity, room as room_application};
use crate::domain::agent::IdempotencyKey;
use crate::domain::attachment::{AttachmentId, ContentType, FileName, NewAttachment, StorageKey};
use crate::domain::message::{MessageBody, MessageId};
use crate::domain::participant::{Participant, ParticipantId};
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::ports::attachment::AttachmentStore;
use crate::ports::blob::BlobStorage;
use crate::ports::message::{MessageStore, NewMessage, SendMessageOutcome};

/// 打开迁移完备的临时数据库（真实 SQLite：全部 migration 应用 + busy
/// timeout 与生产一致）。
async fn open_pool(dir: &tempfile::TempDir) -> SqlitePool {
    open_pool_max(dir, 4).await
}

/// 同上，但允许指定连接池上限（并发清理/绑定线性化测试需要 >4 并发）。
/// 迁移仍由 `MigrationStore` 应用（含 4 连接池），返回的独立池复用同一
/// 数据库文件（与既有 migration 幂等测试同一模式）。
async fn open_pool_max(dir: &tempfile::TempDir, max_connections: u32) -> SqlitePool {
    MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("migrate test database");
    let options = SqliteConnectOptions::new()
        .filename(dir.path().join("core.db"))
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(options)
        .await
        .expect("open pool with custom connection cap")
}

/// 最小 fixture：通过公开用例建立 Human owner + Agent + Direct Room
/// （fixture 允许触碰数据库，成功/拒绝结果由存储层 API 或约束观察）。
struct Fixture {
    _dir: tempfile::TempDir,
    pool: SqlitePool,
    human: Participant,
    agent_id: String,
    room_id: String,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_pool(&dir).await;
    fixture_with_pool(dir, pool).await
}

async fn fixture_with_pool(dir: tempfile::TempDir, pool: SqlitePool) -> Fixture {
    let identity = SqliteIdentityStore::new(pool.clone());
    let human = identity::bootstrap_human(&identity, "Owner")
        .await
        .expect("bootstrap")
        .participant;
    let agent_store = SqliteAgentStore::new(pool.clone());
    let created = agent::create_agent(&agent_store, &human, "A", "att-a-k")
        .await
        .expect("create agent");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("agent create must be Created");
    };
    let room_store = SqliteRoomStore::new(pool.clone());
    let created_room =
        room_application::create_direct_room(&room_store, &human, agent.id.as_str(), "att-r-k")
            .await
            .expect("create direct room");
    let room_application::CreateDirectRoomOutcome::Created { room, .. } = created_room else {
        panic!("room create must be Created");
    };
    Fixture {
        _dir: dir,
        pool,
        human,
        agent_id: agent.id.as_str().to_owned(),
        room_id: room.id.as_str().to_owned(),
    }
}

fn new_attachment_at(fx: &Fixture, actor: &str, uploaded_at: UtcMillis) -> NewAttachment {
    NewAttachment {
        id: AttachmentId(uuid::Uuid::new_v4().to_string()),
        uploader_participant_id: ParticipantId(actor.into()),
        room_id: RoomId(fx.room_id.clone()),
        file_name: FileName::parse("a.txt").unwrap(),
        content_type: ContentType::parse("text/plain").unwrap(),
        size_bytes: 5,
        storage_key: StorageKey::new_v4(),
        sha256_hex: "a".repeat(64),
        uploaded_at,
    }
}

fn new_attachment(fx: &Fixture, id: &str, actor: &str) -> NewAttachment {
    let mut att = new_attachment_at(fx, actor, UtcMillis::now());
    att.id = AttachmentId(id.into());
    att
}

/// 直接 SQL 插入附件行（绕过应用层；测试构造攻击/边界事实用）。state 与
/// uploaded_at 由调用方给定——插入路径不受附件触发守卫约束。
async fn insert_attachment_row_raw(
    pool: &SqlitePool,
    fx: &Fixture,
    id: &str,
    storage_key: &StorageKey,
    actor: &str,
    state: &str,
    uploaded_at: UtcMillis,
) {
    sqlx::query(
        "INSERT INTO attachments \
         (id, uploader_participant_id, room_id, file_name, content_type, size_bytes, \
          state, storage_key, sha256_hex, uploaded_at) \
         VALUES (?, ?, ?, 'a.txt', 'text/plain', 5, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(actor)
    .bind(&fx.room_id)
    .bind(state)
    .bind(storage_key.as_str())
    .bind("a".repeat(64))
    .bind(uploaded_at.millis())
    .execute(pool)
    .await
    .expect("raw attachment row insert");
}

async fn create_pending_row(fx: &Fixture, id: &str, actor: &str) {
    let store = SqliteAttachmentStore::new(fx.pool.clone());
    store
        .create_pending(&new_attachment(fx, id, actor))
        .await
        .expect("pending row must be created");
}

async fn send_with_attachments(
    fx: &Fixture,
    actor: &str,
    key: &str,
    body: Option<&str>,
    attachments: &[&str],
) -> Result<SendMessageOutcome, crate::ports::message::MessageStoreError> {
    let store = SqliteMessageStore::new(fx.pool.clone());
    store
        .send_message(
            &NewMessage {
                id: MessageId::new_v4(),
                room_id: RoomId(fx.room_id.clone()),
                actor_id: ParticipantId(actor.into()),
                body: body.map(|b| MessageBody::parse(b).unwrap()),
                mentions: Vec::new(),
                attachments: attachments
                    .iter()
                    .map(|id| AttachmentId((*id).into()))
                    .collect(),
                reply_to_message_id: None,
                created_at: UtcMillis::now(),
            },
            &IdempotencyKey::parse(key).unwrap(),
        )
        .await
}

#[tokio::test]
async fn create_pending_requires_active_membership() {
    let fx = fixture().await;
    let store = SqliteAttachmentStore::new(fx.pool.clone());
    // 成员正常上传成功。
    store
        .create_pending(&new_attachment(
            &fx,
            "00000000-0000-4000-8000-0000000000b1",
            fx.human.id.as_str(),
        ))
        .await
        .expect("member upload must succeed");
    // 非成员 / unknown Room 统一 RoomNotFound。
    let outsider = ParticipantId(uuid::Uuid::new_v4().to_string());
    sqlx::query("INSERT INTO participants (id, kind, status, display_name, created_at, revoked_at) VALUES (?, 'human', 'active', 'O', ?, NULL)")
        .bind(outsider.as_str()).bind(UtcMillis::now().millis()).execute(&fx.pool).await.unwrap();
    let mut bad = new_attachment(
        &fx,
        "00000000-0000-4000-8000-0000000000b3",
        outsider.as_str(),
    );
    let err = store.create_pending(&bad).await.unwrap_err();
    assert!(matches!(
        err,
        crate::ports::attachment::AttachmentStoreError::RoomNotFound
    ));
    bad.room_id = RoomId("00000000-0000-4000-8000-000000000099".into());
    let err = store.create_pending(&bad).await.unwrap_err();
    assert!(matches!(
        err,
        crate::ports::attachment::AttachmentStoreError::RoomNotFound
    ));
}

#[tokio::test]
async fn pending_attachment_is_not_downloadable_until_bound() {
    let fx = fixture().await;
    create_pending_row(
        &fx,
        "00000000-0000-4000-8000-0000000000c1",
        fx.human.id.as_str(),
    )
    .await;
    let store = SqliteAttachmentStore::new(fx.pool.clone());
    let result = store
        .authorized_attachment(
            &fx.human.id,
            &RoomId(fx.room_id.clone()),
            &AttachmentId("00000000-0000-4000-8000-0000000000c1".into()),
        )
        .await
        .unwrap();
    assert!(result.is_none(), "pending must never be downloadable");

    // 绑定后 uploader 可下载。
    send_with_attachments(
        &fx,
        fx.human.id.as_str(),
        "att-key-1",
        Some("hello"),
        &["00000000-0000-4000-8000-0000000000c1"],
    )
    .await
    .expect("bind must succeed");
    let result = store
        .authorized_attachment(
            &fx.human.id,
            &RoomId(fx.room_id.clone()),
            &AttachmentId("00000000-0000-4000-8000-0000000000c1".into()),
        )
        .await
        .unwrap();
    let attachment = result.expect("bound attachment must be downloadable by the uploader");
    assert_eq!(attachment.sha256_hex, "a".repeat(64));
}

#[tokio::test]
async fn download_authorization_matrix_is_fail_closed() {
    let fx = fixture().await;
    let store = SqliteAttachmentStore::new(fx.pool.clone());
    let att_id = "00000000-0000-4000-8000-0000000000d1";
    create_pending_row(&fx, att_id, fx.human.id.as_str()).await;
    send_with_attachments(
        &fx,
        fx.human.id.as_str(),
        "att-key-2",
        Some("hello"),
        &[att_id],
    )
    .await
    .expect("bind must succeed");
    let att = AttachmentId(att_id.into());
    let room = RoomId(fx.room_id.clone());

    // 成员（Human / Agent）都可下载。
    for reader in [fx.human.id.as_str(), fx.agent_id.as_str()] {
        let result = store
            .authorized_attachment(&ParticipantId(reader.to_owned()), &room, &att)
            .await
            .unwrap();
        assert!(result.is_some(), "member {reader} must be able to download");
    }
    // 非成员统一 None。
    let outsider = ParticipantId(uuid::Uuid::new_v4().to_string());
    sqlx::query("INSERT INTO participants (id, kind, status, display_name, created_at, revoked_at) VALUES (?, 'human', 'active', 'O', ?, NULL)")
        .bind(outsider.as_str()).bind(UtcMillis::now().millis()).execute(&fx.pool).await.unwrap();
    let result = store
        .authorized_attachment(&outsider, &room, &att)
        .await
        .unwrap();
    assert!(result.is_none(), "non-member must not download");
    // 跨 Room：room 参数不匹配统一 None（不泄露存在性）。
    let other_room = RoomId("00000000-0000-4000-8000-0000000000d3".into());
    let result = store
        .authorized_attachment(&fx.human.id, &other_room, &att)
        .await
        .unwrap();
    assert!(result.is_none(), "cross-room download must be denied");
    // unknown attachment 统一 None。
    let ghost = AttachmentId("00000000-0000-4000-8000-0000000000d4".into());
    let result = store
        .authorized_attachment(&fx.human.id, &room, &ghost)
        .await
        .unwrap();
    assert!(result.is_none(), "unknown attachment must be denied");
    // 撤回后不可下载。
    sqlx::query(
        "INSERT INTO message_recalls (message_id, recalled_by_participant_id, recalled_at) \
         SELECT id, actor_id, CAST(unixepoch('subsec') * 1000 AS INTEGER) \
         FROM messages WHERE room_id = ? ORDER BY sequence LIMIT 1",
    )
    .bind(&fx.room_id)
    .execute(&fx.pool)
    .await
    .unwrap();
    let result = store
        .authorized_attachment(&fx.human.id, &room, &att)
        .await
        .unwrap();
    assert!(
        result.is_none(),
        "recalled message attachments must not be downloadable"
    );
}

#[tokio::test]
async fn message_binding_is_atomic_and_guards_reject_violations() {
    let fx = fixture().await;
    let att_id = "00000000-0000-4000-8000-0000000000e1";
    create_pending_row(&fx, att_id, fx.human.id.as_str()).await;

    // 绑定成功：Message 成立，附件 bound，position 有序。
    let outcome = send_with_attachments(
        &fx,
        fx.human.id.as_str(),
        "att-key-3",
        Some("hello"),
        &[att_id],
    )
    .await
    .expect("bind must succeed");
    let message_id = match &outcome {
        SendMessageOutcome::Created { message } => message.id.as_str().to_owned(),
        _ => panic!("must be created"),
    };
    let state: String = sqlx::query_scalar("SELECT state FROM attachments WHERE id = ?")
        .bind(att_id)
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(state, "bound");
    let position: i64 =
        sqlx::query_scalar("SELECT position FROM message_attachments WHERE attachment_id = ?")
            .bind(att_id)
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert_eq!(position, 1);

    // 同附件二次绑定（换 key 换 Message）→ AttachmentNotFound（附件已 bound）。
    let err = send_with_attachments(
        &fx,
        fx.human.id.as_str(),
        "att-key-4",
        Some("again"),
        &[att_id],
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err,
        crate::ports::message::MessageStoreError::AttachmentNotFound
    ));

    // 跨 actor 绑定他人 pending：Agent（同 Room 成员）上传，Human 尝试
    // 绑定 → AttachmentNotFound；Human（非 uploader）作为 actor 绑定
    // 必须失败且不占用该 pending。
    create_pending_row(
        &fx,
        "00000000-0000-4000-8000-0000000000e3",
        fx.agent_id.as_str(),
    )
    .await;
    let err = send_with_attachments(
        &fx,
        fx.human.id.as_str(),
        "att-key-5",
        Some("x"),
        &["00000000-0000-4000-8000-0000000000e3"],
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err,
        crate::ports::message::MessageStoreError::AttachmentNotFound
    ));
    // 该 pending 未被占用（跨 actor 失败不留任何绑定/状态变化）。
    let state: String = sqlx::query_scalar(
        "SELECT state FROM attachments WHERE id = '00000000-0000-4000-8000-0000000000e3'",
    )
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert_eq!(state, "pending");

    // 附件-only Message 成功；body NULL + 无绑定被数据库守卫拒绝。
    create_pending_row(
        &fx,
        "00000000-0000-4000-8000-0000000000e4",
        fx.human.id.as_str(),
    )
    .await;
    send_with_attachments(
        &fx,
        fx.human.id.as_str(),
        "att-key-6",
        None,
        &["00000000-0000-4000-8000-0000000000e4"],
    )
    .await
    .expect("attachment-only message must succeed");
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    let result = sqlx::query(
        "INSERT INTO messages (id, room_id, sequence, actor_id, body, reply_to_message_id, idempotency_key, created_at) \
         VALUES (?, ?, ?, ?, NULL, NULL, ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&fx.room_id)
    .bind(before + 1)
    .bind(fx.human.id.as_str())
    .bind("direct-sql-body-null")
    .bind(UtcMillis::now().millis())
    .execute(&fx.pool)
    .await;
    assert!(
        result.is_err(),
        "body NULL without bindings must be rejected by the DB"
    );
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(after, before, "rejected insert must leave no message");

    // 幂等重放：同 key 同 body 同附件 → 同 Message。
    let replay = send_with_attachments(
        &fx,
        fx.human.id.as_str(),
        "att-key-6",
        None,
        &["00000000-0000-4000-8000-0000000000e4"],
    )
    .await
    .unwrap();
    assert!(matches!(replay, SendMessageOutcome::Replayed { .. }));
    // 同 key 同请求（带附件）→ 重放。
    let replay2 = send_with_attachments(
        &fx,
        fx.human.id.as_str(),
        "att-key-3",
        Some("hello"),
        &[att_id],
    )
    .await
    .unwrap();
    assert!(matches!(replay2, SendMessageOutcome::Replayed { .. }));

    // 附件事实不可 UPDATE / DELETE；绑定行不可 DELETE（直接 SQL 守卫）。
    let att_uuid = "00000000-0000-4000-8000-0000000000e1";
    assert!(
        sqlx::query("UPDATE attachments SET size_bytes = 1 WHERE id = ?")
            .bind(att_uuid)
            .execute(&fx.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM attachments WHERE id = ?")
            .bind(att_uuid)
            .execute(&fx.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM message_attachments WHERE attachment_id = ?")
            .bind(att_uuid)
            .execute(&fx.pool)
            .await
            .is_err()
    );
    // state 反向（bound → pending）被守卫拒绝。
    assert!(
        sqlx::query("UPDATE attachments SET state = 'pending' WHERE id = ?")
            .bind(att_uuid)
            .execute(&fx.pool)
            .await
            .is_err()
    );
    // 孤儿绑定（无 Message）被 deferred FK 拒绝。
    let orphan = sqlx::query("INSERT INTO message_attachments (message_id, attachment_id, position) VALUES ('00000000-0000-4000-8000-000000000099', '00000000-0000-4000-8000-0000000000e3', 1)")
        .execute(&fx.pool)
        .await;
    assert!(orphan.is_err(), "orphan binding must be rejected at commit");
    // 绑定行身份防改写：INSERT OR REPLACE 同类写入被 identity guard 拒绝。
    assert!(sqlx::query("INSERT OR REPLACE INTO message_attachments (message_id, attachment_id, position) VALUES (?, ?, 1)")
        .bind(&message_id)
        .bind(att_uuid)
        .execute(&fx.pool)
        .await
        .is_err());
}

#[tokio::test]
async fn referenced_storage_keys_covers_all_rows() {
    let fx = fixture().await;
    let store = SqliteAttachmentStore::new(fx.pool.clone());
    let first = new_attachment(
        &fx,
        "00000000-0000-4000-8000-0000000000f1",
        fx.human.id.as_str(),
    );
    let second = new_attachment(
        &fx,
        "00000000-0000-4000-8000-0000000000f2",
        fx.human.id.as_str(),
    );
    store.create_pending(&first).await.unwrap();
    store.create_pending(&second).await.unwrap();
    let keys = store.referenced_storage_keys().await.unwrap();
    assert_eq!(keys.len(), 2);
    assert!(keys.contains(first.storage_key.as_str()));
    assert!(keys.contains(second.storage_key.as_str()));
}

/// purge 语义一：未过期 pending 保留（私有暂存租约未到期不回收）、过期
/// 且无绑定的 pending 行被原子删除并返回其 storage key；过期但不满足
/// 条件的（有绑定行 / bound）一律不回收。
#[tokio::test]
async fn purge_keeps_fresh_and_recycles_only_expired_unbound_pending() {
    let fx = fixture().await;
    let store = SqliteAttachmentStore::new(fx.pool.clone());
    let now = UtcMillis::now();
    let h = fx.human.id.as_str();

    // 未过期 pending（1 小时前上传）。
    let fresh = new_attachment_at(&fx, h, UtcMillis::from_millis(now.millis() - 3_600_000));
    store.create_pending(&fresh).await.unwrap();
    // 过期无绑定 pending（25 小时前上传，Human 与 Agent 各一条）。
    let expired_h = new_attachment_at(&fx, h, UtcMillis::from_millis(now.millis() - 90_000_000));
    let expired_a = new_attachment_at(
        &fx,
        fx.agent_id.as_str(),
        UtcMillis::from_millis(now.millis() - 90_000_000),
    );
    store.create_pending(&expired_h).await.unwrap();
    store.create_pending(&expired_a).await.unwrap();

    // 过期但有绑定行：直接 SQL 在既有 Message 上绑定过期 pending 行
    // （绑定守卫只要求 pending 状态，不检查过期；purge 必须因有绑定拒绝）。
    let bound_msg = send_with_attachments(&fx, h, "purge-key-1", Some("m"), &[])
        .await
        .expect("plain message");
    let bound_msg_id = match &bound_msg {
        SendMessageOutcome::Created { message } => message.id.as_str().to_owned(),
        _ => panic!("must be created"),
    };
    let expired_bound =
        new_attachment_at(&fx, h, UtcMillis::from_millis(now.millis() - 90_000_000));
    sqlx::query(
        "INSERT INTO attachments \
         (id, uploader_participant_id, room_id, file_name, content_type, size_bytes, \
          state, storage_key, sha256_hex, uploaded_at) \
         VALUES (?, ?, ?, 'a.txt', 'text/plain', 5, 'pending', ?, ?, ?)",
    )
    .bind(expired_bound.id.as_str())
    .bind(h)
    .bind(&fx.room_id)
    .bind(expired_bound.storage_key.as_str())
    .bind("a".repeat(64))
    .bind(now.millis() - 90_000_000)
    .execute(&fx.pool)
    .await
    .expect("raw expired pending row");
    sqlx::query(
        "INSERT INTO message_attachments (message_id, attachment_id, position) VALUES (?, ?, 1)",
    )
    .bind(&bound_msg_id)
    .bind(expired_bound.id.as_str())
    .execute(&fx.pool)
    .await
    .expect("raw binding for expired pending");

    // 过期已 bound（直接 SQL 构造 bound 行；插入路径不受附件触发守卫约束）。
    let expired_bound_row =
        new_attachment_at(&fx, h, UtcMillis::from_millis(now.millis() - 90_000_000));
    insert_attachment_row_raw(
        &fx.pool,
        &fx,
        expired_bound_row.id.as_str(),
        &expired_bound_row.storage_key,
        h,
        "bound",
        UtcMillis::from_millis(now.millis() - 90_000_000),
    )
    .await;

    let keys = store
        .purge_expired_pending(now)
        .await
        .expect("purge must succeed");
    let mut returned: Vec<String> = keys.iter().map(|k| k.as_str().to_owned()).collect();
    returned.sort();
    let mut expected = vec![
        expired_h.storage_key.as_str().to_owned(),
        expired_a.storage_key.as_str().to_owned(),
    ];
    expected.sort();
    assert_eq!(
        returned, expected,
        "only expired unbound pendings are purged"
    );

    // 行存在性：fresh / 有绑定 / bound 全保留；两条过期无绑定行已删。
    for id in [
        fresh.id.as_str(),
        expired_bound.id.as_str(),
        expired_bound_row.id.as_str(),
    ] {
        let exists: bool =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM attachments WHERE id = ?")
                .bind(id)
                .fetch_one(&fx.pool)
                .await
                .unwrap()
                > 0;
        assert!(exists, "{id} must be kept");
    }
    for id in [expired_h.id.as_str(), expired_a.id.as_str()] {
        let exists: bool =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM attachments WHERE id = ?")
                .bind(id)
                .fetch_one(&fx.pool)
                .await
                .unwrap()
                > 0;
        assert!(!exists, "{id} must be purged");
    }
    // 有绑定行的附件行未删：绑定关系仍在。
    let binding: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM message_attachments WHERE attachment_id = ?")
            .bind(expired_bound.id.as_str())
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert_eq!(
        binding, 1,
        "binding of an expired pending must survive purge"
    );

    // 空清理幂等：再跑一次返回空集合。
    let again = store.purge_expired_pending(now).await.unwrap();
    assert!(again.is_empty(), "second purge must be a no-op");
}

/// purge 语义二：返回的 storage key 是内部事实，启动层据此删除对应字节
/// （best-effort）；未返回 key 的字节（未过期 pending / bound）绝不动。
#[tokio::test]
async fn purge_returns_keys_whose_blobs_are_cleaned_like_startup() {
    let fx = fixture().await;
    let store = SqliteAttachmentStore::new(fx.pool.clone());
    let now = UtcMillis::now();
    let h = fx.human.id.as_str();

    let fresh = new_attachment_at(&fx, h, UtcMillis::from_millis(now.millis() - 3_600_000));
    let expired = new_attachment_at(&fx, h, UtcMillis::from_millis(now.millis() - 90_000_000));
    let bound = new_attachment_at(&fx, h, UtcMillis::from_millis(now.millis() - 3_600_000));
    store.create_pending(&fresh).await.unwrap();
    store.create_pending(&expired).await.unwrap();
    store.create_pending(&bound).await.unwrap();
    send_with_attachments(&fx, h, "purge-key-2", Some("b"), &[bound.id.as_str()])
        .await
        .expect("bind must succeed");

    // 真实媒体目录 + LocalBlobStorage（与 production 同一写入路径）。
    let media = tempfile::tempdir().unwrap();
    let blob = crate::adapters::blob::LocalBlobStorage::new(media.path().join("attachments"));
    blob.write(&fresh.storage_key, b"fresh").await.unwrap();
    blob.write(&expired.storage_key, b"expired").await.unwrap();
    blob.write(&bound.storage_key, b"bound").await.unwrap();

    let keys = store.purge_expired_pending(now).await.unwrap();
    assert_eq!(
        keys,
        vec![expired.storage_key.clone()],
        "exactly the expired unbound key is returned"
    );
    // 启动清理语义：只删除返回 key 的字节。
    let blob_dir = media.path().join("attachments");
    for key in &keys {
        blob.delete(key).await.unwrap();
    }
    assert!(
        !blob_dir.join(expired.storage_key.as_str()).exists(),
        "expired blob removed"
    );
    assert!(
        blob_dir.join(fresh.storage_key.as_str()).exists(),
        "fresh blob kept"
    );
    assert!(
        blob_dir.join(bound.storage_key.as_str()).exists(),
        "bound blob kept"
    );
    // 行事实同步：fresh 仍 pending、bound 仍 bound、expired 已回收。
    let fresh_state: String = sqlx::query_scalar("SELECT state FROM attachments WHERE id = ?")
        .bind(fresh.id.as_str())
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(fresh_state, "pending");
    let bound_state: String = sqlx::query_scalar("SELECT state FROM attachments WHERE id = ?")
        .bind(bound.id.as_str())
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(bound_state, "bound");
}

/// purge 语义三（DELETE 守卫）：数据库只放行「过期 + pending + 无绑定行」
/// 一种删除条件——未过期 pending、bound、有绑定行一律被触发器拒绝；
/// purge 以墙钟为兜底，即使调用方传入未来 now 也无法删除未过期附件。
#[tokio::test]
async fn delete_guard_only_allows_expired_pending_without_bindings() {
    let fx = fixture().await;
    let store = SqliteAttachmentStore::new(fx.pool.clone());
    let now = UtcMillis::now();
    let h = fx.human.id.as_str();

    // 三条过期无绑定 pending + 一条未过期 pending（DELETE 守卫矩阵）。
    let expired_1 = new_attachment_at(&fx, h, UtcMillis::from_millis(now.millis() - 90_000_000));
    let expired_2 = new_attachment_at(&fx, h, UtcMillis::from_millis(now.millis() - 90_000_000));
    let fresh = new_attachment_at(&fx, h, UtcMillis::from_millis(now.millis() - 3_600_000));
    store.create_pending(&expired_1).await.unwrap();
    store.create_pending(&expired_2).await.unwrap();
    store.create_pending(&fresh).await.unwrap();

    // 过期 + 有绑定行。
    let msg = send_with_attachments(&fx, h, "purge-key-3", Some("m"), &[])
        .await
        .expect("plain message");
    let SendMessageOutcome::Created { message: msg } = msg else {
        panic!("must be created");
    };
    let expired_bound =
        new_attachment_at(&fx, h, UtcMillis::from_millis(now.millis() - 90_000_000));
    insert_attachment_row_raw(
        &fx.pool,
        &fx,
        expired_bound.id.as_str(),
        &expired_bound.storage_key,
        h,
        "pending",
        UtcMillis::from_millis(now.millis() - 90_000_000),
    )
    .await;
    sqlx::query(
        "INSERT INTO message_attachments (message_id, attachment_id, position) VALUES (?, ?, 1)",
    )
    .bind(msg.id.as_str())
    .bind(expired_bound.id.as_str())
    .execute(&fx.pool)
    .await
    .expect("raw binding");
    // 已 bound。
    let bound_row = new_attachment_at(&fx, h, UtcMillis::from_millis(now.millis() - 90_000_000));
    insert_attachment_row_raw(
        &fx.pool,
        &fx,
        bound_row.id.as_str(),
        &bound_row.storage_key,
        h,
        "bound",
        UtcMillis::from_millis(now.millis() - 90_000_000),
    )
    .await;

    // 直接 SQL DELETE 矩阵：未过期 / bound / 过期有绑定 → 拒绝。
    for id in [
        fresh.id.as_str(),
        bound_row.id.as_str(),
        expired_bound.id.as_str(),
    ] {
        assert!(
            sqlx::query("DELETE FROM attachments WHERE id = ?")
                .bind(id)
                .execute(&fx.pool)
                .await
                .is_err(),
            "DELETE of {id} must be rejected by the guard"
        );
    }
    // 过期 + pending + 无绑定 → 守卫放行（内部清理的合法条件）。
    assert!(
        sqlx::query("DELETE FROM attachments WHERE id = ?")
            .bind(expired_1.id.as_str())
            .execute(&fx.pool)
            .await
            .is_ok(),
        "expired unbound pending DELETE is the only allowed condition"
    );

    // 墙钟兜底：purge 传入未来 now（已过期）也删不掉未过期附件——DELETE
    // 守卫按墙钟独立裁决，整体回滚（Database 错误、行保留）。
    let future = UtcMillis::from_millis(now.millis() + 90_000_000);
    let err = store.purge_expired_pending(future).await.unwrap_err();
    assert!(
        matches!(
            err,
            crate::ports::attachment::AttachmentStoreError::Database(_)
        ),
        "wall-clock backstop must abort the DELETE: {err}"
    );
    let exists: bool =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM attachments WHERE id = ?")
            .bind(expired_2.id.as_str())
            .fetch_one(&fx.pool)
            .await
            .unwrap()
            > 0;
    assert!(
        exists,
        "fresh pending must survive a purge with a future now"
    );
}

/// purge 语义四（并发线性化）：清理事务与绑定事务同为 BEGIN IMMEDIATE，
/// 由 SQLite 写锁线性化。过期附件上并发 6 个 purge + 6 个绑定：恰好一个
/// purge 删除该行并返回其 key，其余 purge 空、全部绑定 404 整体回滚，
/// 不留 Message / 绑定行 / 半态。反向顺序（先绑定后清理）由
/// purge_keeps_fresh_and_recycles_only_expired_unbound_pending 证明。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_purge_and_bind_linearize_without_partial_state() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_pool_max(&dir, 8).await;
    let fx = fixture_with_pool(dir, pool).await;
    let now = UtcMillis::now();
    let h = fx.human.id.as_str();

    // 过期 pending（墙钟与 purge now 一致地过期）。
    let expired = new_attachment_at(&fx, h, UtcMillis::from_millis(now.millis() - 90_000_000));
    let store = SqliteAttachmentStore::new(fx.pool.clone());
    store.create_pending(&expired).await.unwrap();
    let att_id = expired.id.clone();
    let room_id = fx.room_id.clone();
    let actor_id = fx.human.id.clone();

    let mut purge_tasks = Vec::new();
    for _ in 0..6 {
        let store = store.clone();
        purge_tasks.push(tokio::spawn(async move {
            store.purge_expired_pending(UtcMillis::now()).await
        }));
    }
    let mut bind_tasks = Vec::new();
    for i in 0..6 {
        let pool = fx.pool.clone();
        let att_id = att_id.clone();
        let room_id = room_id.clone();
        let actor_id = actor_id.clone();
        bind_tasks.push(tokio::spawn(async move {
            let store = SqliteMessageStore::new(pool);
            store
                .send_message(
                    &NewMessage {
                        id: MessageId::new_v4(),
                        room_id: RoomId(room_id),
                        actor_id,
                        body: Some(MessageBody::parse("bind").unwrap()),
                        mentions: Vec::new(),
                        attachments: vec![att_id],
                        reply_to_message_id: None,
                        created_at: UtcMillis::now(),
                    },
                    &IdempotencyKey::parse(&format!("purge-conc-{i}")).unwrap(),
                )
                .await
        }));
    }

    let mut purged_keys: usize = 0;
    let mut purge_errors: usize = 0;
    for task in purge_tasks {
        match task.await.expect("purge task must not panic") {
            Ok(keys) => purged_keys += keys.len(),
            Err(e) => {
                assert!(
                    matches!(
                        e,
                        crate::ports::attachment::AttachmentStoreError::Database(_)
                    ),
                    "purge must only fail with a storage error: {e}"
                );
                purge_errors += 1;
            }
        }
    }
    for task in bind_tasks {
        let result = task
            .await
            .expect("bind task must not panic")
            .expect_err("binding an expired pending must always be rejected");
        assert!(
            matches!(
                result,
                crate::ports::message::MessageStoreError::AttachmentNotFound
            ),
            "concurrent bind must fail uniformly with AttachmentNotFound: {result}"
        );
    }

    // 线性化结果：该过期行恰被删除一次（其余 purge 空），无任何半态。
    assert_eq!(
        purged_keys, 1,
        "exactly one purge must win the delete (purge_errors={purge_errors})"
    );
    let att_exists: i64 =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM attachments WHERE id = ?")
            .bind(expired.id.as_str())
            .fetch_one(&fx.pool)
            .await
            .unwrap();
    assert_eq!(att_exists, 0, "the purged row must be gone");
    let messages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(messages, 0, "failed binds must leave no messages");
    let bindings: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM message_attachments")
        .fetch_one(&fx.pool)
        .await
        .unwrap();
    assert_eq!(bindings, 0, "failed binds must leave no binding rows");
}

/// 迁移 012 在全新库重复启动幂等（MigrationStore 重开同一文件）。
#[tokio::test]
async fn migrations_apply_idempotently_with_attachments_schema() {
    let dir = tempfile::tempdir().unwrap();
    let db_path: PathBuf = dir.path().join("core.db");
    MigrationStore::open(&db_path).await.expect("first open");
    MigrationStore::open(&db_path)
        .await
        .expect("second open must be idempotent");
    let options = SqliteConnectOptions::new()
        .filename(&db_path)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name IN ('attachments', 'message_attachments') ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        tables,
        vec!["attachments".to_owned(), "message_attachments".to_owned()]
    );
    let clean: i64 = sqlx::query_scalar("PRAGMA foreign_key_check")
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
    assert_eq!(clean, 0, "foreign_key_check must be clean");
}
