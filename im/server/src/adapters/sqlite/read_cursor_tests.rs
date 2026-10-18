//! SQLite read cursor / 未读 adapter 测试：真实临时数据库上的 cursor 初始
//! 化与冻结、单调推进 / no-op、窗口可见性裁决、未读投影、群主转让继承、
//! advancement → read.cursor.advanced fact/delivery 原子成立、重启保持、
//! commit 前取消回滚，以及数据库直接 INSERT/UPDATE/DELETE/REPLACE 的
//! fail-closed 负向守卫（契约 CORE_V1_READ_STATE.md 6–7）。

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use uuid::Uuid;

use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::message_store::SqliteMessageStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::room_store::SqliteRoomStore;
use crate::application::identity::bootstrap_human;
use crate::application::room as room_application;
use crate::domain::agent::IdempotencyKey;
use crate::domain::human_session::{HumanSession, SessionId, TokenHash};
use crate::domain::message::{MessageBody, MessageId};
use crate::domain::participant::{DisplayName, Participant, ParticipantId};
use crate::domain::room::{ReadState, RoomId};
use crate::domain::time::UtcMillis;
use crate::domain::token::OpaqueToken;
use crate::ports::identity::{IdentityStore as _, NewSession};
use crate::ports::message::{MessageStore as _, NewMessage};
use crate::ports::room::{AdvanceReadCursorOutcome, RoomStore as _};

struct Fixture {
    _dir: tempfile::TempDir,
    identity: SqliteIdentityStore,
    room: SqliteRoomStore,
    message: SqliteMessageStore,
    pool: sqlx::SqlitePool,
    db_path: std::path::PathBuf,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("core.db");
    let store = MigrationStore::open(&db_path)
        .await
        .expect("open + migrate");
    let pool = store.pool().clone();
    Fixture {
        _dir: dir,
        identity: SqliteIdentityStore::new(pool.clone()),
        room: SqliteRoomStore::new(pool.clone()),
        message: SqliteMessageStore::new(pool.clone()),
        pool,
        db_path,
    }
}

/// 通过 schema 直接创建第二个 Human 并为其补发 Session。
async fn add_human(
    pool: &sqlx::SqlitePool,
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

async fn create_group(f: &Fixture, owner: &Participant, name: &str, key: &str) -> RoomId {
    let outcome = room_application::create_group(&f.room, owner, name, key)
        .await
        .expect("create group");
    let room_application::CreateRoomOutcome::Created { room, .. } = outcome else {
        panic!("first group create must be Created");
    };
    room.id
}

async fn invite(f: &Fixture, owner: &Participant, room: &RoomId, target: &Participant, key: &str) {
    room_application::invite_group_member(&f.room, owner, room.as_str(), target.id.as_str(), key)
        .await
        .expect("invite member");
}

/// accepted 联系人 fixture（owner ↔ human，group 邀请前置事实）。
async fn accept_contact(f: &Fixture, owner: &Participant, human: &Participant) {
    let (a, b) = crate::domain::contact::canonical_pair(&owner.id, &human.id);
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
    .bind(Uuid::new_v4().to_string())
    .execute(f.pool())
    .await
    .expect("insert accepted contact fixture");
}

/// 发送一条普通 Message，返回 (Message ID, sequence)。
async fn send(
    f: &Fixture,
    room: &RoomId,
    actor: &Participant,
    body: &str,
    key: &str,
) -> (MessageId, i64) {
    let outcome = f
        .message
        .send_message(
            &NewMessage {
                id: MessageId::new_v4(),
                room_id: room.clone(),
                actor_id: actor.id.clone(),
                body: Some(MessageBody::parse(body).unwrap()),
                mentions: Vec::new(),
                attachments: Vec::new(),
                reply_to_message_id: None,
                created_at: UtcMillis::now(),
            },
            &IdempotencyKey::parse(key).unwrap(),
        )
        .await
        .expect("send message");
    let crate::ports::message::SendMessageOutcome::Created { message } = outcome else {
        panic!("first send must be Created");
    };
    (message.id, message.sequence)
}

async fn read_state_of(f: &Fixture, human: &ParticipantId, room: &RoomId) -> ReadState {
    let item = f
        .room
        .get_room(human, room)
        .await
        .expect("get room")
        .expect("room must be visible");
    item.read_state
}

async fn list_read_states(f: &Fixture, human: &ParticipantId) -> Vec<(RoomId, ReadState)> {
    f.room
        .list_rooms(human)
        .await
        .expect("list rooms")
        .into_iter()
        .map(|item| (item.room.id, item.read_state))
        .collect()
}

async fn advance(
    f: &Fixture,
    human: &Participant,
    room: &RoomId,
    message_id: &MessageId,
) -> Result<AdvanceReadCursorOutcome, crate::ports::room::RoomStoreError> {
    f.room
        .advance_read_cursor(&human.id, room, message_id)
        .await
}

async fn cursor_row(f: &Fixture, membership_id: &str) -> Option<(i64, Option<String>)> {
    sqlx::query_as(
        "SELECT read_sequence, last_read_message_id FROM read_cursors WHERE membership_id = ?",
    )
    .bind(membership_id)
    .fetch_optional(f.pool())
    .await
    .expect("read cursor row")
}

impl Fixture {
    fn pool(&self) -> &sqlx::SqlitePool {
        &self.pool
    }
}

/// 新成员 cursor：last_read_message_id = null、内部 read_sequence =
/// join_after_sequence；首次加入前消息不计未读；他人消息计未读。
#[tokio::test]
async fn new_membership_starts_null_cursor_and_counts_only_visible_other_messages() {
    let f = fixture().await;
    let a = bootstrap_human(&f.identity, "Alice")
        .await
        .expect("bootstrap")
        .participant;
    let b = add_human(f.pool(), &f.identity, "Bob").await;
    accept_contact(&f, &a, &b).await;
    let group = create_group(&f, &a, "群", "rc-g-1").await;

    // A 先发两条（B 尚未入群，B 视角不可见、不计未读）。
    let (m1, s1) = send(&f, &group, &a, "a1", "rc-a-1").await;
    let (_m2, s2) = send(&f, &group, &a, "a2", "rc-a-2").await;
    assert_eq!((s1, s2), (1, 2));

    // A 的初始 read state：null cursor；自己的消息不给自己计未读。
    let a_state = read_state_of(&f, &a.id, &group).await;
    assert_eq!(a_state.last_read_message_id, None);
    assert_eq!(a_state.unread_count, 0);

    // B 入群：cursor 内部边界 = 2（join_after），入群前消息不计未读。
    invite(&f, &a, &group, &b, "rc-inv-b-1").await;
    let b_state = read_state_of(&f, &b.id, &group).await;
    assert_eq!(b_state.last_read_message_id, None);
    assert_eq!(b_state.unread_count, 0);

    // 入群后他人消息：B 未读 +1；自己消息不计；Room list/get 一致。
    let (m3, s3) = send(&f, &group, &a, "a3", "rc-a-3").await;
    assert_eq!(s3, 3);
    let b_state = read_state_of(&f, &b.id, &group).await;
    assert_eq!(b_state.unread_count, 1);
    let b_send = send(&f, &group, &b, "b1", "rc-b-1").await;
    assert_eq!(b_send.1, 4);
    let b_state = read_state_of(&f, &b.id, &group).await;
    assert_eq!(b_state.unread_count, 1, "own message must not count");
    let (m5, _) = send(&f, &group, &a, "a4", "rc-a-4").await;
    let b_state = read_state_of(&f, &b.id, &group).await;
    assert_eq!(b_state.unread_count, 2);

    // list 投影与 get 一致。
    let listed = list_read_states(&f, &b.id).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].1, b_state);

    // 加入前消息（m1/m2）对 B 不可见：不能作为推进目标（统一拒绝，与
    // 未知消息同一语义），也不计未读。
    let err = advance(&f, &b, &group, &m1)
        .await
        .expect_err("pre-join message must be rejected as target");
    assert!(matches!(
        err,
        crate::ports::room::RoomStoreError::InvalidReadCursor
    ));

    // 推进到入群后第一条他人消息（m3）：cursor 前进，未读按位置减少。
    let outcome = advance(&f, &b, &group, &m3).await.expect("advance to m3");
    let AdvanceReadCursorOutcome::Advanced {
        read_state,
        advancement_id,
    } = outcome
    else {
        panic!("first advance must be Advanced");
    };
    assert_eq!(read_state.last_read_message_id.as_ref(), Some(&m3));
    assert_eq!(read_state.unread_count, 1, "m5 仍未读（m3 已读、m5 未读）");
    assert_eq!(advancement_id.len(), 36);

    let b_state = read_state_of(&f, &b.id, &group).await;
    assert_eq!(b_state, read_state);
    let b_list = list_read_states(&f, &b.id).await;
    assert_eq!(b_list[0].1, read_state);

    // 同目标重试与旧目标乱序：no-op，不产生新 advancement / Event。
    let again = advance(&f, &b, &group, &m3)
        .await
        .expect("retry same target");
    let AdvanceReadCursorOutcome::NoOp { read_state } = again else {
        panic!("same target must be NoOp");
    };
    assert_eq!(read_state.last_read_message_id.as_ref(), Some(&m3));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM read_cursor_advancements")
        .fetch_one(f.pool())
        .await
        .unwrap();
    assert_eq!(count, 1, "no-op must not create an advancement");

    // 自己发送的消息也可以作为推进目标（可见即可推进）。
    let (b1_id, _) = b_send;
    let outcome = advance(&f, &b, &group, &b1_id)
        .await
        .expect("advance to own message");
    let AdvanceReadCursorOutcome::Advanced { read_state, .. } = outcome else {
        panic!("own visible message must advance");
    };
    assert_eq!(read_state.last_read_message_id.as_ref(), Some(&b1_id));
    assert_eq!(read_state.unread_count, 1, "只剩 m5 未读");

    // 推进到 m5 后未读清零。
    let outcome = advance(&f, &b, &group, &m5).await.expect("advance to m5");
    let AdvanceReadCursorOutcome::Advanced { read_state, .. } = outcome else {
        panic!("m5 must advance");
    };
    assert_eq!(read_state.unread_count, 0);
    // 旧值 m3（低于当前、仍可见）no-op。
    let stale = advance(&f, &b, &group, &m3).await.expect("stale target");
    let AdvanceReadCursorOutcome::NoOp { read_state } = stale else {
        panic!("stale target must be NoOp");
    };
    assert_eq!(read_state.last_read_message_id.as_ref(), Some(&m5));
}

/// 非法目标统一拒绝：未知 / 他 Room / 加入前不可见消息 → InvalidReadCursor，
/// cursor 与 advancement 均不变；无 active Membership / unknown Room →
/// NotFound（404）。
#[tokio::test]
async fn invalid_targets_and_non_members_are_uniformly_rejected() {
    let f = fixture().await;
    let a = bootstrap_human(&f.identity, "Alice")
        .await
        .expect("bootstrap")
        .participant;
    let b = add_human(f.pool(), &f.identity, "Bob").await;
    let c = add_human(f.pool(), &f.identity, "Carol").await;
    accept_contact(&f, &a, &b).await;
    accept_contact(&f, &a, &c).await;
    let group1 = create_group(&f, &a, "群1", "rc-g-1").await;
    let group2 = create_group(&f, &a, "群2", "rc-g-2").await;
    invite(&f, &a, &group1, &b, "rc-inv-b").await;
    invite(&f, &a, &group2, &b, "rc-inv-b-2").await;
    let (_m1, _) = send(&f, &group1, &a, "a1", "rc-a-1").await;
    let (other_room_msg, _) = send(&f, &group2, &a, "g2-1", "rc-a-2").await;

    // 他 Room 消息：统一 invalid_read_cursor。
    let err = advance(&f, &b, &group1, &other_room_msg)
        .await
        .expect_err("other-room message must be rejected");
    assert!(
        matches!(err, crate::ports::room::RoomStoreError::InvalidReadCursor),
        "{err:?}"
    );

    // 未知 UUID（合法 v4 但不存在）：统一 invalid_read_cursor。
    let ghost = MessageId::new_v4();
    let err = advance(&f, &b, &group1, &ghost)
        .await
        .expect_err("unknown message must be rejected");
    assert!(matches!(
        err,
        crate::ports::room::RoomStoreError::InvalidReadCursor
    ));

    // 加入前消息不可见：C 从未入群，其成员周期不存在 → 404 语义。
    let err = advance(&f, &c, &group1, &_m1)
        .await
        .expect_err("non-member must be rejected");
    assert!(matches!(err, crate::ports::room::RoomStoreError::NotFound));

    // 未入群 Human 推进他人 Room：NotFound。
    let err = advance(&f, &c, &group2, &other_room_msg)
        .await
        .expect_err("non-member in group2 must be rejected");
    assert!(matches!(err, crate::ports::room::RoomStoreError::NotFound));

    // 全部拒绝后无任何 advancement / cursor 变化。
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM read_cursor_advancements")
        .fetch_one(f.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    let b_state = read_state_of(&f, &b.id, &group1).await;
    assert_eq!(b_state.last_read_message_id, None);
    assert_eq!(b_state.unread_count, 1, "m1 仍未读");
}

/// 并发推进：两个目标并发最终收敛到最高合法位置，无倒退、无重复推进事实。
#[tokio::test]
async fn concurrent_advances_converge_to_highest_legal_position() {
    let f = fixture().await;
    let a = bootstrap_human(&f.identity, "Alice")
        .await
        .expect("bootstrap")
        .participant;
    let b = add_human(f.pool(), &f.identity, "Bob").await;
    accept_contact(&f, &a, &b).await;
    let group = create_group(&f, &a, "群", "rc-g-1").await;
    invite(&f, &a, &group, &b, "rc-inv-b").await;
    let (_m1, _) = send(&f, &group, &a, "a1", "rc-a-1").await;
    let (m2, _) = send(&f, &group, &a, "a2", "rc-a-2").await;
    let (m3, _) = send(&f, &group, &a, "a3", "rc-a-3").await;

    let f1 = &f;
    let b1 = &b;
    let g1 = &group;
    let (r1, r2) = tokio::join!(advance(f1, b1, g1, &m2), advance(f1, b1, g1, &m3),);
    r1.expect("concurrent advance m2");
    r2.expect("concurrent advance m3");

    let state = read_state_of(&f, &b.id, &group).await;
    assert_eq!(state.last_read_message_id.as_ref(), Some(&m3));
    assert_eq!(state.unread_count, 0);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM read_cursor_advancements")
        .fetch_one(f.pool())
        .await
        .unwrap();
    // m2/m3 都高于初始 null 位置：两个推进都可能真实发生；若 m3 先赢则
    // m2 变 no-op。只允许 1 或 2 条，且必须严格单调（无倒退/重复事实）。
    assert!(count == 1 || count == 2, "advancements: {count}");
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT from_sequence, to_sequence FROM read_cursor_advancements \
         ORDER BY to_sequence",
    )
    .fetch_all(f.pool())
    .await
    .unwrap();
    assert!(
        rows.windows(2).all(|w| w[0].1 < w[1].1),
        "strictly increasing"
    );
    assert_eq!(rows.last().map(|r| r.1), Some(3));
}

/// 离群冻结与重入：closed cursor 不可推进；重入得到新 null cursor，离群
/// 间隙消息不计未读、不可见。
#[tokio::test]
async fn leave_freezes_cursor_and_rejoin_starts_fresh_boundary() {
    let f = fixture().await;
    let a = bootstrap_human(&f.identity, "Alice")
        .await
        .expect("bootstrap")
        .participant;
    let b = add_human(f.pool(), &f.identity, "Bob").await;
    accept_contact(&f, &a, &b).await;
    let group = create_group(&f, &a, "群", "rc-g-1").await;
    invite(&f, &a, &group, &b, "rc-inv-b").await;
    let (_m1, _) = send(&f, &group, &a, "a1", "rc-a-1").await;
    let (m2, _) = send(&f, &group, &a, "a2", "rc-a-2").await;
    advance(&f, &b, &group, &m2).await.expect("advance to m2");

    // B 退群：closed 周期 cursor 冻结（行保留，read_sequence 不变）。
    let left = room_application::leave_membership(&f.room, &b.id, group.as_str())
        .await
        .expect("leave");
    let (frozen_sequence, frozen_message) = cursor_row(&f, left.as_str())
        .await
        .expect("cursor row kept");
    assert_eq!(frozen_sequence, 2);
    assert_eq!(frozen_message.as_deref(), Some(m2.as_str()));

    // 离群期间 A 发消息：不计入 B；B 推进 → NotFound（无 active 周期）。
    let (m3, _) = send(&f, &group, &a, "a3", "rc-a-3").await;
    let err = advance(&f, &b, &group, &m3)
        .await
        .expect_err("closed membership must not advance");
    assert!(matches!(err, crate::ports::room::RoomStoreError::NotFound));

    // B 重入：新周期新 cursor（null），间隙消息不计未读；重入后 A 消息计
    // 未读。
    invite(&f, &a, &group, &b, "rc-inv-b-2").await;
    let b_state = read_state_of(&f, &b.id, &group).await;
    assert_eq!(b_state.last_read_message_id, None);
    assert_eq!(b_state.unread_count, 0, "gap m3 must not count");
    let (m4, _) = send(&f, &group, &a, "a4", "rc-a-4").await;
    let b_state = read_state_of(&f, &b.id, &group).await;
    assert_eq!(b_state.unread_count, 1, "only post-rejoin m4 counts");
    advance(&f, &b, &group, &m4).await.expect("advance to m4");
    let b_state = read_state_of(&f, &b.id, &group).await;
    assert_eq!(b_state.unread_count, 0);
}

/// schema 范围：所有提及 "cursor" 的 SQL 对象只属于本切片的
/// read_cursors / read_cursor_advancements 事实与 human_event_facts 对
/// read.cursor.advanced kind 的守卫扩展（read cursor 是已封板公共事实；
/// 不应出现服务端 ACK、attempt、Lease、Runtime 等任何延期对象）。
#[tokio::test]
async fn schema_cursor_mentions_are_only_the_sanctioned_read_cursor_objects() {
    let f = fixture().await;
    let objects: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, sql FROM sqlite_master \
         WHERE sql LIKE '%cursor%' OR name LIKE '%cursor%'",
    )
    .fetch_all(f.pool())
    .await
    .unwrap();
    for (name, sql) in &objects {
        let sanctioned = name.contains("read_cursor") || sql.contains("read.cursor.advanced");
        assert!(
            sanctioned,
            "cursor mention must belong to the sanctioned read-cursor chain: {name:?}"
        );
        for forbidden in [
            "ack", "attempt", "lease", "fencing", "runtime", "provider", "session", "token",
        ] {
            assert!(
                !name.to_ascii_lowercase().contains(forbidden),
                "deferred object {forbidden:?} in cursor schema name {name:?}"
            );
        }
    }
    let cursor_tables: usize = objects
        .iter()
        .filter(|(name, _)| name.starts_with("read_cursor"))
        .count();
    assert!(
        cursor_tables >= 2,
        "read_cursors + read_cursor_advancements must exist: {objects:?}"
    );
}

/// 群主转让：两位 Human 的新周期分别继承前序 cursor，未读连续——不清零、
/// 不增加、不倒退；转让前后未读一致。
#[tokio::test]
async fn owner_transfer_inherits_predecessor_cursors_without_unread_change() {
    let f = fixture().await;
    let a = bootstrap_human(&f.identity, "Alice")
        .await
        .expect("bootstrap")
        .participant;
    let b = add_human(f.pool(), &f.identity, "Bob").await;
    accept_contact(&f, &a, &b).await;
    let group = create_group(&f, &a, "群", "rc-g-1").await;
    invite(&f, &a, &group, &b, "rc-inv-b").await;
    let (m1, _) = send(&f, &group, &a, "a1", "rc-a-1").await;
    let (m2, _) = send(&f, &group, &b, "b1", "rc-b-1").await;
    let (_m3, _) = send(&f, &group, &a, "a2", "rc-a-2").await;
    // A 读到 m1，B 读到 m2；A 未读 {m2}, B 未读 {m3}。
    advance(&f, &a, &group, &m1).await.expect("A advance m1");
    advance(&f, &b, &group, &m2).await.expect("B advance m2");
    let a_before = read_state_of(&f, &a.id, &group).await;
    let b_before = read_state_of(&f, &b.id, &group).await;
    assert_eq!(a_before.unread_count, 1, "m2 by B");
    assert_eq!(b_before.unread_count, 1, "m3 by A");

    // 转让：B 成为 owner，A 成为 member；四条周期切换不产生 joined/left
    // 噪音，cursor 由同事务 transition 精确见证继承。
    let outcome = room_application::transfer_group_owner(
        &f.room,
        &a,
        group.as_str(),
        b.id.as_str(),
        "rc-t-1",
    )
    .await
    .expect("transfer owner");
    let room_application::TransferOwnerOutcome::Transferred { membership, .. } = outcome else {
        panic!("transfer must be Transferred");
    };
    assert_eq!(membership.participant_id, b.id);

    let a_after = read_state_of(&f, &a.id, &group).await;
    let b_after = read_state_of(&f, &b.id, &group).await;
    assert_eq!(a_after, a_before, "A cursor continuity");
    assert_eq!(b_after, b_before, "B cursor continuity");
    assert_eq!(a_after.last_read_message_id.as_ref(), Some(&m1));
    assert_eq!(b_after.last_read_message_id.as_ref(), Some(&m2));

    // 转让后新消息照常计未读；推进不受角色影响。
    let (m4, _) = send(&f, &group, &a, "a3", "rc-a-3").await;
    assert_eq!(
        read_state_of(&f, &b.id, &group).await.unread_count,
        2,
        "m3+m4"
    );
    assert_eq!(read_state_of(&f, &a.id, &group).await.unread_count, 1, "m2");
    advance(&f, &b, &group, &m4).await.expect("B advance m4");
    assert_eq!(read_state_of(&f, &b.id, &group).await.unread_count, 0);

    // 继承的 cursor 与时间无关：不能对转让后新周期插 join boundary cursor
    //（负向由 read_cursors_insert_guard 覆盖，见 DB 守卫测试）。
}

/// 每次真实推进：advancement 事实 + read.cursor.advanced fact/delivery 同
/// 事务原子成立；只投递推进者本人；no-op 不生成。
#[tokio::test]
async fn advancement_fact_and_human_event_delivery_are_atomic_and_personal() {
    let f = fixture().await;
    let a = bootstrap_human(&f.identity, "Alice")
        .await
        .expect("bootstrap")
        .participant;
    let b = add_human(f.pool(), &f.identity, "Bob").await;
    accept_contact(&f, &a, &b).await;
    let group = create_group(&f, &a, "群", "rc-g-1").await;
    invite(&f, &a, &group, &b, "rc-inv-b").await;
    let (m1, s1) = send(&f, &group, &a, "a1", "rc-a-1").await;
    assert_eq!(s1, 1);

    let AdvanceReadCursorOutcome::Advanced {
        read_state: _,
        advancement_id,
    } = advance(&f, &b, &group, &m1).await.expect("advance")
    else {
        panic!("must be Advanced");
    };

    // advancement 事实字段一致。
    let row: (String, String, i64, i64, String, i64) = sqlx::query_as(
        "SELECT participant_id, room_id, from_sequence, to_sequence, last_read_message_id, created_at \
         FROM read_cursor_advancements WHERE advancement_id = ?",
    )
    .bind(&advancement_id)
    .fetch_one(f.pool())
    .await
    .expect("advancement row");
    assert_eq!(row.0, b.id.as_str());
    assert_eq!(row.1, group.as_str());
    assert_eq!(row.2, 0);
    assert_eq!(row.3, 1);
    assert_eq!(row.4, m1.as_str());
    assert!(row.5 > 0);

    // fact + snapshot：只含 read-state 必要稳定字段（room_id /
    // last_read_message_id；不含 advancement id——advancement_id 只存在于
    // 内部 source_id 列，绝不进入公共快照），无正文 / sequence
    // boundary / membership / session。
    let fact: (String, String, String, String) = sqlx::query_as(
        "SELECT f.kind, f.source_id, f.room_id, f.snapshot \
         FROM human_event_facts f WHERE f.kind = 'read.cursor.advanced' AND f.source_id = ?",
    )
    .bind(&advancement_id)
    .fetch_one(f.pool())
    .await
    .expect("fact row");
    assert_eq!(fact.1, advancement_id);
    assert_eq!(fact.2, group.as_str());
    let snapshot: serde_json::Value = serde_json::from_str(&fact.3).expect("snapshot json");
    let keys: Vec<String> = snapshot
        .as_object()
        .expect("snapshot object")
        .keys()
        .cloned()
        .collect();
    let mut keys = keys.clone();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["last_read_message_id".to_owned(), "room_id".to_owned()],
        "read.cursor.advanced snapshot must contain only room_id and last_read_message_id: {snapshot}"
    );
    assert!(
        snapshot.get("id").is_none(),
        "no advancement id in snapshot"
    );
    assert_eq!(snapshot["room_id"], group.as_str());
    assert_eq!(snapshot["last_read_message_id"], m1.as_str());
    assert!(snapshot.get("body").is_none());
    assert!(snapshot.get("sequence").is_none());
    assert!(snapshot.get("join_after_sequence").is_none());
    assert!(snapshot.get("session_id").is_none());

    // delivery 只投递推进者本人（B 有、A 没有）。
    let b_deliveries: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM human_event_deliveries d \
         JOIN human_event_facts f ON f.fact_id = d.fact_id \
         WHERE d.human_id = ? AND f.kind = 'read.cursor.advanced'",
    )
    .bind(b.id.as_str())
    .fetch_one(f.pool())
    .await
    .unwrap();
    let a_deliveries: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM human_event_deliveries d \
         JOIN human_event_facts f ON f.fact_id = d.fact_id \
         WHERE d.human_id = ? AND f.kind = 'read.cursor.advanced'",
    )
    .bind(a.id.as_str())
    .fetch_one(f.pool())
    .await
    .unwrap();
    assert_eq!(b_deliveries, 1);
    assert_eq!(
        a_deliveries, 0,
        "advancement must only be delivered to the advancing human"
    );

    // no-op 不产生新 fact / delivery。
    advance(&f, &b, &group, &m1).await.expect("retry noop");
    let facts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM human_event_facts WHERE kind = 'read.cursor.advanced'",
    )
    .fetch_one(f.pool())
    .await
    .unwrap();
    assert_eq!(facts, 1);
}

/// 重启保持：cursor、advancement 与未读投影在重新打开同一数据库后不变。
#[tokio::test]
async fn restart_keeps_cursor_advancement_and_unread() {
    let f = fixture().await;
    let a = bootstrap_human(&f.identity, "Alice")
        .await
        .expect("bootstrap")
        .participant;
    let b = add_human(f.pool(), &f.identity, "Bob").await;
    accept_contact(&f, &a, &b).await;
    let group = create_group(&f, &a, "群", "rc-g-1").await;
    invite(&f, &a, &group, &b, "rc-inv-b").await;
    let (m1, _) = send(&f, &group, &a, "a1", "rc-a-1").await;
    let (m2, _) = send(&f, &group, &a, "a2", "rc-a-2").await;
    advance(&f, &b, &group, &m1).await.expect("advance");
    let before = read_state_of(&f, &b.id, &group).await;
    assert_eq!(before.last_read_message_id.as_ref(), Some(&m1));
    assert_eq!(before.unread_count, 1);

    // 重新打开同一数据库（fresh store 实例 = 进程重启语义）。
    let reopened = MigrationStore::open(&f.db_path).await.expect("reopen");
    let room2 = SqliteRoomStore::new(reopened.pool().clone());
    let item = room2
        .get_room(&b.id, &group)
        .await
        .expect("get room")
        .expect("visible after restart");
    assert_eq!(item.read_state, before);
    let b2 = b.clone();
    let outcome = room2
        .advance_read_cursor(&b2.id, &group, &m2)
        .await
        .expect("advance after restart");
    let AdvanceReadCursorOutcome::Advanced { read_state, .. } = outcome else {
        panic!("must advance after restart");
    };
    assert_eq!(read_state.unread_count, 0);
}

/// commit 前取消：cursor / advancement / fact / delivery 全部回滚，连接可
/// 复用，下一次推进正常。
#[tokio::test]
async fn cancelled_advance_rolls_back_fully_and_reuses_connection() {
    let f = fixture().await;
    let a = bootstrap_human(&f.identity, "Alice")
        .await
        .expect("bootstrap")
        .participant;
    let b = add_human(f.pool(), &f.identity, "Bob").await;
    accept_contact(&f, &a, &b).await;
    let group = create_group(&f, &a, "群", "rc-g-1").await;
    invite(&f, &a, &group, &b, "rc-inv-b").await;
    let (m1, _) = send(&f, &group, &a, "a1", "rc-a-1").await;
    let (m2, _) = send(&f, &group, &a, "a2", "rc-a-2").await;

    // 单连接池：abort 后的查询与下一次推进必须复用同一物理连接。
    let options = SqliteConnectOptions::new()
        .filename(&f.db_path)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5));
    let cancel_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open single-connection cancellation pool");
    let cancel_room = SqliteRoomStore::new(cancel_pool.clone());

    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
    let (released_tx, released_rx) = tokio::sync::oneshot::channel();
    cancel_room.arm_commit_checkpoint(reached_tx, released_rx);

    let b2 = b.clone();
    let g2 = group.clone();
    let m2b = m2.clone();
    let task_room = cancel_room.clone();
    let handle =
        tokio::spawn(async move { task_room.advance_read_cursor(&b2.id, &g2, &m2b).await });

    tokio::time::timeout(std::time::Duration::from_secs(10), reached_rx)
        .await
        .expect("advance must reach the commit checkpoint in time")
        .expect("advance must not fail before the checkpoint");
    handle.abort();
    let join_error = handle.await.expect_err("task must be cancelled");
    assert!(join_error.is_cancelled());
    drop(released_tx);

    for (label, sql) in [
        (
            "advancements",
            "SELECT COUNT(*) FROM read_cursor_advancements",
        ),
        (
            "facts",
            "SELECT COUNT(*) FROM human_event_facts WHERE kind = 'read.cursor.advanced'",
        ),
        (
            "deliveries",
            "SELECT COUNT(*) FROM human_event_deliveries d \
             JOIN human_event_facts f ON f.fact_id = d.fact_id \
             WHERE f.kind = 'read.cursor.advanced'",
        ),
    ] {
        let count: i64 = sqlx::query_scalar(sql)
            .fetch_one(&cancel_pool)
            .await
            .expect("same pooled connection must recover after cancellation");
        assert_eq!(count, 0, "cancelled advance must leave no {label}");
    }
    let state: (i64, Option<String>) = sqlx::query_as(
        "SELECT c.read_sequence, c.last_read_message_id FROM read_cursors c \
         JOIN room_memberships m ON m.membership_id = c.membership_id \
         WHERE c.room_id = ? AND c.participant_id = ? AND m.active_marker = 1",
    )
    .bind(group.as_str())
    .bind(b.id.as_str())
    .fetch_one(&cancel_pool)
    .await
    .expect("cursor row recoverable");
    assert_eq!(state.0, 0, "cursor must stay at join boundary");
    assert_eq!(state.1, None);

    // 同一物理连接下一次推进成功（连接未被污染）。
    let outcome = cancel_room
        .advance_read_cursor(&b.id, &group, &m1)
        .await
        .expect("advance after cancellation");
    let AdvanceReadCursorOutcome::Advanced { .. } = outcome else {
        panic!("must advance after cancellation");
    };
}

/// 数据库直接 INSERT/UPDATE/DELETE/REPLACE 负向守卫：非 Human cursor、
/// 跳读、转让不继承、倒退、closed 推进、他 Room / 未来 Message、advancement
/// 伪造、UPDATE/DELETE/REPLACE 全部 fail closed。
#[tokio::test]
async fn direct_sql_writes_are_fail_closed() {
    let f = fixture().await;
    let a = bootstrap_human(&f.identity, "Alice")
        .await
        .expect("bootstrap")
        .participant;
    let b = add_human(f.pool(), &f.identity, "Bob").await;
    accept_contact(&f, &a, &b).await;
    let group = create_group(&f, &a, "群", "rc-g-1").await;
    invite(&f, &a, &group, &b, "rc-inv-b").await;
    let (m1, _) = send(&f, &group, &a, "a1", "rc-a-1").await;
    let (_m2, _) = send(&f, &group, &a, "a2", "rc-a-2").await;
    let b_membership: String = sqlx::query_scalar(
        "SELECT m.membership_id FROM room_memberships m \
         WHERE m.room_id = ? AND m.participant_id = ? AND m.active_marker = 1",
    )
    .bind(group.as_str())
    .bind(b.id.as_str())
    .fetch_one(f.pool())
    .await
    .unwrap();
    let a_membership: String = sqlx::query_scalar(
        "SELECT m.membership_id FROM room_memberships m \
         WHERE m.room_id = ? AND m.participant_id = ? AND m.active_marker = 1",
    )
    .bind(group.as_str())
    .bind(a.id.as_str())
    .fetch_one(f.pool())
    .await
    .unwrap();

    // Agent 周期同样有一条 cursor（切片 M1，migration 202609270001）：
    // 由**同一个**触发器建行，boundary = join_after_sequence、last_read 为
    // null——与 Human 同形。⚠ 本条原为反向断言（"Agent 周期不能有 cursor"），
    // 2026-09-27 随窄解冻切片 M1 改为正向：Agent 与 Human 的差异只允许来自
    // 来历与处境，读游标两者都不是。
    let agent = crate::application::agent::create_agent(
        &crate::adapters::sqlite::agent_store::SqliteAgentStore::new(f.pool().clone()),
        &a,
        "甲",
        "rc-agent-1",
    )
    .await
    .expect("create agent");
    let crate::application::agent::CreateAgentOutcome::Created { agent, .. } = agent else {
        panic!("agent must be Created");
    };
    let agent_membership = {
        let outcome = room_application::invite_group_member(
            &f.room,
            &a,
            group.as_str(),
            agent.id.as_str(),
            "rc-inv-agent-1",
        )
        .await
        .expect("invite agent");
        let room_application::InviteMemberOutcome::Created { membership } = outcome else {
            panic!("agent invite must be Created");
        };
        membership.membership_id
    };
    let agent_cursor = cursor_row(&f, agent_membership.as_str()).await;
    let (agent_sequence, agent_last_read) = agent_cursor.expect("agent memberships get cursors");
    assert_eq!(
        agent_last_read, None,
        "fresh agent join boundary has no last_read_message_id"
    );
    let agent_join_after: i64 = sqlx::query_scalar(
        "SELECT join_after_sequence FROM room_memberships WHERE membership_id = ?",
    )
    .bind(agent_membership.as_str())
    .fetch_one(f.pool())
    .await
    .unwrap();
    assert_eq!(
        agent_sequence, agent_join_after,
        "agent cursor boundary = join_after_sequence"
    );

    // 负对照（守卫没松）：直接 SQL 为**已存在**周期的身份重复建行仍被拒
    // ——放开的只有 kind，不是"cursor 身份 = 周期的 participant"这条判据。
    let err = sqlx::query(
        "INSERT INTO read_cursors (membership_id, participant_id, room_id, read_sequence, last_read_message_id, created_at) \
         VALUES (?, ?, ?, 0, NULL, 0)",
    )
    .bind(agent_membership.as_str())
    .bind(agent.id.as_str())
    .bind(group.as_str())
    .execute(f.pool())
    .await
    .expect_err("duplicate agent cursor insert must be rejected");
    assert!(
        err.to_string()
            .contains("read cursor identity already exists")
    );

    // 跳读：cursor 已由触发器创建，任何重复 INSERT（含跳读形状）先被身份
    // 守卫拒绝（fail closed：已存在的 cursor 行不可重建、不可覆盖）。
    let err = sqlx::query(
        "INSERT INTO read_cursors (membership_id, participant_id, room_id, read_sequence, last_read_message_id, created_at) \
         VALUES (?, ?, ?, 5, NULL, 0)",
    )
    .bind(&b_membership)
    .bind(b.id.as_str())
    .bind(group.as_str())
    .execute(f.pool())
    .await
    .expect_err("jump insert must be rejected");
    assert!(
        err.to_string()
            .contains("read cursor identity already exists")
    );

    // 重复身份（普通重复 INSERT 与 REPLACE）→ 拒绝，不重建。
    let now = UtcMillis::now().millis();
    let err = sqlx::query(
        "INSERT INTO read_cursors (membership_id, participant_id, room_id, read_sequence, last_read_message_id, created_at) \
         VALUES (?, ?, ?, 0, NULL, ?)",
    )
    .bind(&b_membership)
    .bind(b.id.as_str())
    .bind(group.as_str())
    .bind(now)
    .execute(f.pool())
    .await
    .expect_err("duplicate cursor insert must be rejected");
    assert!(
        err.to_string()
            .contains("read cursor identity already exists")
    );
    let err = sqlx::query(
        "INSERT OR REPLACE INTO read_cursors (membership_id, participant_id, room_id, read_sequence, last_read_message_id, created_at) \
         VALUES (?, ?, ?, 9, NULL, ?)",
    )
    .bind(&b_membership)
    .bind(b.id.as_str())
    .bind(group.as_str())
    .bind(now)
    .execute(f.pool())
    .await
    .expect_err("REPLACE cursor must be rejected");
    assert!(
        err.to_string()
            .contains("read cursor identity already exists")
    );

    // 倒退推进 / 未来 / 他 Room / closed 周期推进：UPDATE 守卫。

    let err = sqlx::query(
        "UPDATE read_cursors SET read_sequence = 100, last_read_message_id = ? WHERE membership_id = ?",
    )
    .bind(m1.as_str())
    .bind(&b_membership)
    .execute(f.pool())
    .await
    .expect_err("future sequence update must be rejected");
    assert!(err.to_string().contains("monotonically"));
    let ghost = MessageId::new_v4();
    let err = sqlx::query(
        "UPDATE read_cursors SET read_sequence = 2, last_read_message_id = ? WHERE membership_id = ?",
    )
    .bind(ghost.as_str())
    .bind(&b_membership)
    .execute(f.pool())
    .await
    .expect_err("unknown message update must be rejected");
    assert!(err.to_string().contains("monotonically"));
    let err = sqlx::query(
        "UPDATE read_cursors SET read_sequence = 2, last_read_message_id = ? WHERE membership_id = ?",
    )
    .bind(m1.as_str())
    .bind(&b_membership)
    .execute(f.pool())
    .await
    .expect_err("sequence/message mismatch update must be rejected");
    assert!(err.to_string().contains("monotonically"));
    let err = sqlx::query(
        "UPDATE read_cursors SET read_sequence = -1, last_read_message_id = NULL WHERE membership_id = ?",
    )
    .bind(&b_membership)
    .execute(f.pool())
    .await
    .expect_err("backward update must be rejected");
    assert!(err.to_string().contains("monotonically"));
    let err = sqlx::query("UPDATE read_cursors SET membership_id = ? WHERE membership_id = ?")
        .bind(&a_membership)
        .bind(&b_membership)
        .execute(f.pool())
        .await
        .expect_err("identity update must be rejected");
    assert!(
        err.to_string()
            .contains("read cursor identity is immutable")
    );
    let err = sqlx::query("DELETE FROM read_cursors WHERE membership_id = ?")
        .bind(&b_membership)
        .execute(f.pool())
        .await
        .expect_err("cursor delete must be rejected");
    assert!(err.to_string().contains("cannot be deleted"));

    // advancement 伪造：from != 当前 cursor；Message 他 Room；UPDATE /
    // DELETE / REPLACE。
    let err = sqlx::query(
        "INSERT INTO read_cursor_advancements \
         (advancement_id, participant_id, room_id, membership_id, from_sequence, to_sequence, last_read_message_id, created_at) \
         VALUES (?, ?, ?, ?, 5, 6, ?, 0)",
    )
    .bind(MessageId::new_v4().as_str())
    .bind(b.id.as_str())
    .bind(group.as_str())
    .bind(&b_membership)
    .bind(m1.as_str())
    .execute(f.pool())
    .await
    .expect_err("forged from_sequence must be rejected");
    assert!(
        err.to_string()
            .contains("must move from the current cursor")
    );
    let err = sqlx::query(
        "INSERT INTO read_cursor_advancements \
         (advancement_id, participant_id, room_id, membership_id, from_sequence, to_sequence, last_read_message_id, created_at) \
         VALUES (?, ?, ?, ?, 0, 1, ?, 0)",
    )
    .bind(MessageId::new_v4().as_str())
    .bind(b.id.as_str())
    .bind(group.as_str())
    .bind(&b_membership)
    .bind(ghost.as_str())
    .execute(f.pool())
    .await
    .expect_err("unknown target message must be rejected");
    assert!(
        err.to_string()
            .contains("must move from the current cursor")
    );

    // 真实推进后：advancement 不可 UPDATE / DELETE / REPLACE。
    advance(&f, &b, &group, &m1).await.expect("legit advance");
    let aid: String = sqlx::query_scalar(
        "SELECT advancement_id FROM read_cursor_advancements WHERE membership_id = ?",
    )
    .bind(&b_membership)
    .fetch_one(f.pool())
    .await
    .unwrap();
    let err =
        sqlx::query("UPDATE read_cursor_advancements SET to_sequence = 2 WHERE advancement_id = ?")
            .bind(&aid)
            .execute(f.pool())
            .await
            .expect_err("advancement update must be rejected");
    assert!(err.to_string().contains("immutable"));
    let err = sqlx::query("DELETE FROM read_cursor_advancements WHERE advancement_id = ?")
        .bind(&aid)
        .execute(f.pool())
        .await
        .expect_err("advancement delete must be rejected");
    assert!(err.to_string().contains("cannot be deleted"));
    let err = sqlx::query(
        "INSERT OR REPLACE INTO read_cursor_advancements \
         (advancement_id, participant_id, room_id, membership_id, from_sequence, to_sequence, last_read_message_id, created_at) \
         VALUES (?, ?, ?, ?, 0, 9, ?, 0)",
    )
    .bind(&aid)
    .bind(b.id.as_str())
    .bind(group.as_str())
    .bind(&b_membership)
    .bind(m1.as_str())
    .execute(f.pool())
    .await
    .expect_err("REPLACE advancement must be rejected");
    assert!(err.to_string().contains("identity already exists"));

    // 伪造 fact：kind=read.cursor.advanced 但 source 不是真实 advancement
    // → migration 009 的 fact guard 拒绝。
    let err = sqlx::query(
        "INSERT INTO human_event_facts (fact_id, kind, source_id, room_id, created_at, snapshot) \
         VALUES (?, 'read.cursor.advanced', ?, ?, 0, '{\"id\":\"x\"}')",
    )
    .bind(MessageId::new_v4().as_str())
    .bind(MessageId::new_v4().as_str())
    .bind(group.as_str())
    .execute(f.pool())
    .await
    .expect_err("forged fact must be rejected");
    assert!(err.to_string().contains("consistent source fact"));

    // closed 周期 cursor 不能推进（UPDATE 守卫的 active 检查）：B 退群后
    // 直接 UPDATE 其 cursor → 拒绝。
    room_application::leave_membership(&f.room, &b.id, group.as_str())
        .await
        .expect("leave");
    let err = sqlx::query(
        "UPDATE read_cursors SET read_sequence = 2, last_read_message_id = ? WHERE membership_id = ?",
    )
    .bind(m1.as_str())
    .bind(&b_membership)
    .execute(f.pool())
    .await
    .expect_err("closed cursor update must be rejected");
    assert!(err.to_string().contains("monotonically"));
}

/// 直接 UPDATE cursor 的唯一合法来路是精确 advancement 见证：对 active
/// cursor 选一条真实、同 Room、当前可见且 sequence 更高的 Message 直接
/// UPDATE——即便 Message/sequence 形状完全合法（旧守卫全部通过），没有
/// 精确见证也必须被 DB 拒绝；cursor / advancement / fact / delivery 均
/// 不变。
#[tokio::test]
async fn direct_cursor_update_without_advancement_witness_is_rejected() {
    let f = fixture().await;
    let a = bootstrap_human(&f.identity, "Alice")
        .await
        .expect("bootstrap")
        .participant;
    let b = add_human(f.pool(), &f.identity, "Bob").await;
    accept_contact(&f, &a, &b).await;
    let group = create_group(&f, &a, "群", "rc-g-1").await;
    invite(&f, &a, &group, &b, "rc-inv-b").await;
    let (m1, _) = send(&f, &group, &a, "a1", "rc-a-1").await;
    let b_membership: String = sqlx::query_scalar(
        "SELECT m.membership_id FROM room_memberships m \
         WHERE m.room_id = ? AND m.participant_id = ? AND m.active_marker = 1",
    )
    .bind(group.as_str())
    .bind(b.id.as_str())
    .fetch_one(f.pool())
    .await
    .unwrap();

    // 目标 m1 是真实、同 Room、对 B 历史可见、sequence 高于当前 cursor(0)
    // 的 Message；active / 单调 / Message 合法性防线全部满足，唯独缺少
    // 精确 advancement 见证——必须被拒绝。
    let err = sqlx::query(
        "UPDATE read_cursors SET read_sequence = 1, last_read_message_id = ? WHERE membership_id = ?",
    )
    .bind(m1.as_str())
    .bind(&b_membership)
    .execute(f.pool())
    .await
    .expect_err("witnessless legal-shaped cursor update must be rejected");
    assert!(err.to_string().contains("advancement witness"));

    // cursor / advancement / fact / delivery 均不变。
    assert_eq!(cursor_row(&f, &b_membership).await, Some((0, None)));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM read_cursor_advancements")
        .fetch_one(f.pool())
        .await
        .unwrap();
    assert_eq!(count, 0, "no advancement may exist without a witness");
    let facts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM human_event_facts WHERE kind = 'read.cursor.advanced'",
    )
    .fetch_one(f.pool())
    .await
    .unwrap();
    assert_eq!(facts, 0, "no fact may exist without an advancement");
    let deliveries: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM human_event_deliveries d \
         JOIN human_event_facts f ON f.fact_id = d.fact_id \
         WHERE f.kind = 'read.cursor.advanced'",
    )
    .fetch_one(f.pool())
    .await
    .unwrap();
    assert_eq!(
        deliveries, 0,
        "no delivery may exist without an advancement"
    );

    // 同一合法形状经真实路径（INSERT advancement 触发链）成功——证明拒绝
    // 原因是「缺少见证」而非目标不合法。
    advance(&f, &b, &group, &m1).await.expect("legit advance");
}

/// INSERT read_cursor_advancements 是真实推进的唯一写路径：直接合法 INSERT
/// 必须自动推进 cursor 并原子产生恰好一个 fact/delivery（无独立 UPDATE、
/// 无第二事务）；无见证的直接 UPDATE 仍被拒。
#[tokio::test]
async fn direct_advancement_insert_applies_cursor_and_events_atomically() {
    let f = fixture().await;
    let a = bootstrap_human(&f.identity, "Alice")
        .await
        .expect("bootstrap")
        .participant;
    let b = add_human(f.pool(), &f.identity, "Bob").await;
    accept_contact(&f, &a, &b).await;
    let group = create_group(&f, &a, "群", "rc-g-1").await;
    invite(&f, &a, &group, &b, "rc-inv-b").await;
    let (m1, _) = send(&f, &group, &a, "a1", "rc-a-1").await;
    let (m2, _) = send(&f, &group, &a, "a2", "rc-a-2").await;
    let b_membership: String = sqlx::query_scalar(
        "SELECT m.membership_id FROM room_memberships m \
         WHERE m.room_id = ? AND m.participant_id = ? AND m.active_marker = 1",
    )
    .bind(group.as_str())
    .bind(b.id.as_str())
    .fetch_one(f.pool())
    .await
    .unwrap();

    // 直接合法 INSERT advancement（from = 当前 cursor 0、to = m1 的
    // sequence 1、last_read_message_id = m1）：AFTER 触发器链在同一语句内
    // 自动推进 cursor 并原子生成 fact/delivery。
    let aid = Uuid::new_v4().to_string();
    let now = UtcMillis::now().millis();
    sqlx::query(
        "INSERT INTO read_cursor_advancements \
         (advancement_id, participant_id, room_id, membership_id, from_sequence, to_sequence, last_read_message_id, created_at) \
         VALUES (?, ?, ?, ?, 0, 1, ?, ?)",
    )
    .bind(&aid)
    .bind(b.id.as_str())
    .bind(group.as_str())
    .bind(&b_membership)
    .bind(m1.as_str())
    .bind(now)
    .execute(f.pool())
    .await
    .expect("direct legal advancement insert must apply atomically");

    // cursor 已自动推进到 (1, m1)，无需任何独立 UPDATE。
    assert_eq!(
        cursor_row(&f, &b_membership).await,
        Some((1, Some(m1.as_str().to_string())))
    );
    let adv_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM read_cursor_advancements WHERE advancement_id = ?",
    )
    .bind(&aid)
    .fetch_one(f.pool())
    .await
    .unwrap();
    assert_eq!(adv_count, 1, "exactly one advancement row");
    let fact_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM human_event_facts \
         WHERE kind = 'read.cursor.advanced' AND source_id = ?",
    )
    .bind(&aid)
    .fetch_one(f.pool())
    .await
    .unwrap();
    assert_eq!(fact_count, 1, "exactly one fact per advancement");
    let delivery_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM human_event_deliveries d \
         JOIN human_event_facts f ON f.fact_id = d.fact_id \
         WHERE f.kind = 'read.cursor.advanced' AND f.source_id = ?",
    )
    .bind(&aid)
    .fetch_one(f.pool())
    .await
    .unwrap();
    assert_eq!(
        delivery_count, 1,
        "exactly one delivery to the advancing human"
    );

    // 直接 UPDATE cursor 到 m2（真实、同 Room、可见、sequence 更高，但无
    // 见证）仍被拒；真实路径推进 m2 成功——只有 INSERT advancement 能移动
    // cursor，不存在并行第二写路径。
    let err = sqlx::query(
        "UPDATE read_cursors SET read_sequence = 2, last_read_message_id = ? WHERE membership_id = ?",
    )
    .bind(m2.as_str())
    .bind(&b_membership)
    .execute(f.pool())
    .await
    .expect_err("witnessless cursor update must still be rejected");
    assert!(err.to_string().contains("advancement witness"));
    advance(&f, &b, &group, &m2).await.expect("legit advance");
}

/// 转让打开的周期不能直接插 join boundary cursor（不继承即拒绝）；
/// 只能由同事务 transition 见证的合法流产生。
#[tokio::test]
async fn transfer_opened_period_cursor_requires_exact_inheritance() {
    let f = fixture().await;
    let a = bootstrap_human(&f.identity, "Alice")
        .await
        .expect("bootstrap")
        .participant;
    let b = add_human(f.pool(), &f.identity, "Bob").await;
    accept_contact(&f, &a, &b).await;
    let group = create_group(&f, &a, "群", "rc-g-1").await;
    invite(&f, &a, &group, &b, "rc-inv-b").await;
    let (m1, _) = send(&f, &group, &a, "a1", "rc-a-1").await;
    advance(&f, &b, &group, &m1).await.expect("B advance");

    let outcome = room_application::transfer_group_owner(
        &f.room,
        &a,
        group.as_str(),
        b.id.as_str(),
        "rc-t-1",
    )
    .await
    .expect("transfer");
    let room_application::TransferOwnerOutcome::Transferred { membership, .. } = outcome else {
        panic!("must be Transferred");
    };
    // 新 owner 周期（B）的 cursor 继承前序（值为 m1 推进后的状态）。
    let (seq, msg): (i64, Option<String>) = sqlx::query_as(
        "SELECT read_sequence, last_read_message_id FROM read_cursors WHERE membership_id = ?",
    )
    .bind(membership.membership_id.as_str())
    .fetch_one(f.pool())
    .await
    .expect("inherited cursor");
    assert_eq!(seq, 1);
    assert_eq!(msg.as_deref(), Some(m1.as_str()));

    // 直接 SQL 给转让周期插 join boundary cursor → 被身份守卫拒绝（cursor
    // 已由触发器以继承值创建，不可覆盖重建；未继承的 join boundary 形状
    // 无法写入）。
    let err = sqlx::query(
        "INSERT INTO read_cursors (membership_id, participant_id, room_id, read_sequence, last_read_message_id, created_at) \
         VALUES (?, ?, ?, 0, NULL, 0)",
    )
    .bind(membership.membership_id.as_str())
    .bind(b.id.as_str())
    .bind(group.as_str())
    .execute(f.pool())
    .await
    .expect_err("non-inherited cursor for transfer period must be rejected");
    // ⚠ 2026-09-27（切片 M1）：本行原来只认"身份已存在"那一句。本迁移重建了
    // `read_cursors_insert_guard`（放开 kind 判据），于是**两道 BEFORE INSERT
    // 守卫谁先报**变了：同一行既是重复身份、又是非法形状时，先报的是形状那道。
    // 两道守卫谁先触发**不是契约**（SQLite 不承诺 BEFORE 触发器的相对顺序），
    // 契约是"这一行写不进去"。真正的不变量在上面已经判过：继承值已在行里。
    let message = err.to_string();
    assert!(
        message.contains("read cursor identity already exists")
            || message.contains("read cursor must be a fresh join boundary"),
        "transfer period cursor insert must be rejected by a read-cursor insert guard: {err}"
    );
}

/// 切片 M1（`CORE_V1_AGENT_READ_CURSOR.md`）：Agent 的 cursor 与 Human 同形
/// ——能推进、未读跟着变、no-op 不产生事实；**而 Agent 的推进不产生任何
/// Human Event**。同一场景里 Human 的推进照旧产生 fact + delivery 作为
/// **负对照**（证明"零事件"是 Agent 专属，不是事件面坏了）。
#[tokio::test]
async fn agent_cursor_advances_without_any_human_event() {
    let f = fixture().await;
    let a = bootstrap_human(&f.identity, "Alice")
        .await
        .expect("bootstrap")
        .participant;
    let group = create_group(&f, &a, "群", "rc-ag-g-1").await;

    // 建一支 Agent 并邀进群；入群后 A 发两条（对 Agent 都是"别人发的"）。
    let created = crate::application::agent::create_agent(
        &crate::adapters::sqlite::agent_store::SqliteAgentStore::new(f.pool().clone()),
        &a,
        "甲",
        "rc-ag-agent-1",
    )
    .await
    .expect("create agent");
    let crate::application::agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("agent must be Created");
    };
    room_application::invite_group_member(
        &f.room,
        &a,
        group.as_str(),
        agent.id.as_str(),
        "rc-ag-inv-1",
    )
    .await
    .expect("invite agent");
    let (m1, _) = send(&f, &group, &a, "a1", "rc-ag-a-1").await;
    let (m2, _) = send(&f, &group, &a, "a2", "rc-ag-a-2").await;

    // 1. 入群那一刻的 boundary：位置为空、两条都是未读。
    let before = read_state_of(&f, &agent.id, &group).await;
    assert_eq!(before.last_read_message_id, None);
    assert_eq!(before.unread_count, 2);

    // 2. 推进到第一条：位置 = m1、未读 1（cursor 与 advancement 同一事务落地）。
    let outcome = f
        .room
        .advance_read_cursor(&agent.id, &group, &m1)
        .await
        .expect("agent advance");
    let AdvanceReadCursorOutcome::Advanced { read_state, .. } = outcome else {
        panic!("agent advance must be Advanced");
    };
    assert_eq!(read_state.last_read_message_id.as_ref(), Some(&m1));
    assert_eq!(read_state.unread_count, 1);

    // 3. **零事件**：read.cursor.advanced 的 fact 与 delivery 一条都没有
    //    （同房间的 message.created 等事实不在本条判据范围内）。
    assert_eq!(
        read_cursor_fact_count(&f).await,
        0,
        "agent advance must not create a read.cursor.advanced fact"
    );
    assert_eq!(
        read_cursor_delivery_count(&f).await,
        0,
        "agent advance must not deliver any read.cursor.advanced event"
    );

    // 4. 再推进到第二条：未读 0；回退到 m1 → NoOp（不产生新 advancement）。
    let outcome = f
        .room
        .advance_read_cursor(&agent.id, &group, &m2)
        .await
        .expect("second agent advance");
    let AdvanceReadCursorOutcome::Advanced { read_state, .. } = outcome else {
        panic!("second agent advance must be Advanced");
    };
    assert_eq!(read_state.unread_count, 0);
    let outcome = f
        .room
        .advance_read_cursor(&agent.id, &group, &m1)
        .await
        .expect("older target");
    assert!(
        matches!(outcome, AdvanceReadCursorOutcome::NoOp { .. }),
        "an older target is a successful no-op"
    );
    assert_eq!(read_cursor_fact_count(&f).await, 0);

    // 5. 负对照：同一房间、同一条写路径，**Human 的推进照旧**产生 fact +
    //    delivery（只投推进者本人）。
    let outcome = f
        .room
        .advance_read_cursor(&a.id, &group, &m2)
        .await
        .expect("human advance");
    assert!(matches!(outcome, AdvanceReadCursorOutcome::Advanced { .. }));
    assert_eq!(
        read_cursor_fact_count(&f).await,
        1,
        "human advance still creates the fact"
    );
    assert_eq!(
        read_cursor_delivery_count(&f).await,
        1,
        "human advance still delivers to the advancing human"
    );
}

/// read.cursor.advanced 的 fact 数（只数该 kind，不数同房间其他事件）。
async fn read_cursor_fact_count(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM human_event_facts WHERE kind = 'read.cursor.advanced'")
        .fetch_one(f.pool())
        .await
        .expect("count read cursor facts")
}

/// read.cursor.advanced 的 delivery 数（fact 与 delivery 两层事实各自成立）。
async fn read_cursor_delivery_count(f: &Fixture) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM human_event_deliveries d \
         JOIN human_event_facts fct ON fct.fact_id = d.fact_id \
         WHERE fct.kind = 'read.cursor.advanced'",
    )
    .fetch_one(f.pool())
    .await
    .expect("count read cursor deliveries")
}

/// **既有周期缺 cursor 行 ⇒ 推不动；补建之后能推**（2026-09-27 真机验出的缺陷）。
///
/// 真机形态：`read_cursors_membership_insert` 只在**建周期**时建行，因此**迁移之前**
/// 就存在的周期没有行；而推进的见证守卫要求「存在一行且 `read_sequence = from`」
/// ⇒ 那些周期一条也推不动（真家 5 条活跃 Agent 周期全中）。
///
/// 本用例**复现那条路径**：先摘掉建行触发器再邀 Agent 入群（= 当年那一刻的历史），
/// 于是它有一条**没有 cursor 行**的 active 周期——单测/黑盒此前从没走过这一格，
/// 因为夹具的周期都是迁移之后建的。
#[tokio::test]
async fn a_period_without_a_cursor_row_cannot_advance_until_the_backfill_runs() {
    let f = fixture().await;
    let a = crate::application::identity::bootstrap_human(&f.identity, "Alice")
        .await
        .expect("bootstrap human");
    let group = create_group(&f, &a.participant, "群", "rc-bf-g-1").await;
    let created = crate::application::agent::create_agent(
        &crate::adapters::sqlite::agent_store::SqliteAgentStore::new(f.pool().clone()),
        &a.participant,
        "甲",
        "rc-bf-ag-1",
    )
    .await
    .expect("create agent");
    let crate::application::agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("agent must be Created");
    };

    // 模拟"迁移之前"：那一刻还没有建行触发器。
    sqlx::query("DROP TRIGGER read_cursors_membership_insert")
        .execute(f.pool())
        .await
        .expect("drop the row-creating trigger (simulating history)");
    room_application::invite_group_member(
        &f.room,
        &a.participant,
        group.as_str(),
        agent.id.as_str(),
        "rc-bf-inv-1",
    )
    .await
    .expect("invite agent");
    let (m1, _) = send(&f, &group, &a.participant, "a1", "rc-bf-a-1").await;

    // 历史形态：active 周期**没有** cursor 行。
    let membership_id: String = sqlx::query_scalar(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group.as_str())
    .bind(agent.id.as_str())
    .fetch_one(f.pool())
    .await
    .expect("active agent membership");
    assert_eq!(
        cursor_row(&f, &membership_id).await,
        None,
        "这一刻的历史形态：周期在、cursor 行不在"
    );

    // ① 缺陷本体：推进被守卫拒绝（真机上就是这一条把 M1 变成空转）。
    let refused = f.room.advance_read_cursor(&agent.id, &group, &m1).await;
    assert!(
        refused.is_err(),
        "缺 cursor 行的周期推不动——这正是真机验出的缺陷，got {refused:?}"
    );

    // ② 补建（**用迁移里那段真 SQL**，不抄一份）：行按 join boundary 建出来。
    sqlx::query(include_str!(
        "../../../migrations/202609270003_backfill_read_cursors.sql"
    ))
    .execute(f.pool())
    .await
    .expect("backfill must run");
    let row = cursor_row(&f, &membership_id)
        .await
        .expect("补建之后必须有一行");
    assert_eq!(row.1, None, "补建出来的位置是空的（还没读过）");

    // ③ 补建之后同一次推进成功，且未读按 boundary 算。
    let outcome = f
        .room
        .advance_read_cursor(&agent.id, &group, &m1)
        .await
        .expect("补建之后必须推得动");
    let AdvanceReadCursorOutcome::Advanced { read_state, .. } = outcome else {
        panic!("must be Advanced, got {outcome:?}");
    };
    assert_eq!(read_state.last_read_message_id.as_ref(), Some(&m1));
    assert_eq!(read_state.unread_count, 0);

    // ④ 补建是**幂等**的（再来一次不改变任何东西）。
    sqlx::query(include_str!(
        "../../../migrations/202609270003_backfill_read_cursors.sql"
    ))
    .execute(f.pool())
    .await
    .expect("backfill must be re-runnable");
    assert_eq!(
        cursor_row(&f, &membership_id).await.expect("still one row"),
        (
            // 已推进到 m1 的 sequence（不再回到 boundary）。
            sqlx::query_scalar::<_, i64>("SELECT sequence FROM messages WHERE id = ?")
                .bind(m1.as_str())
                .fetch_one(f.pool())
                .await
                .expect("message sequence"),
            Some(m1.as_str().to_owned())
        )
    );
}
