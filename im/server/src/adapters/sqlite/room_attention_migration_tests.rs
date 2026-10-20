//! 提醒档（attention）migration 守卫测试（契约 `CORE_V1_IM_CONTRACT.md` §9.1，
//! 窄解冻切片 1a）：在真实临时 SQLite 上应用全部 migration，再用**直接 SQL**
//! 证明本切片那两道守卫 fail closed——
//!
//! 1. `room_attention_insert_guard`：只 group 房 + 本人是该房 active 成员；
//! 2. `room_attention_update_guard`：行身份 `(room_id, participant_id)` 不可
//!    改写（**只能写本人那一行**）+ 改值仍须 group 房 + active 成员。
//!
//! 房间本身用**公开用例**建（group / direct 都走 `application::room`），
//! 守卫则用直接 SQL 去打——这正是要证明的那件事：**绕过应用层也改不动**
//! （应用层"身份只来自认证"那一半由 `adapters/http/attention_routes_tests.rs`
//! 证明；本文件不重复判 HTTP 形状）。
//!
//! ⚠ 本切片**不改消息主链**，因此这里连一条 Message 都不种：表与守卫不需要它。

use sqlx::SqlitePool;
use uuid::Uuid;

use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::room_store::SqliteRoomStore;
use crate::application::agent;
use crate::application::identity::bootstrap_human;
use crate::application::room as room_application;
use crate::domain::participant::{DisplayName, Participant, ParticipantId};
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;

struct Fixture {
    _dir: tempfile::TempDir,
    pool: SqlitePool,
    identity: SqliteIdentityStore,
    agent: SqliteAgentStore,
    room: SqliteRoomStore,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("open + migrate on a fresh DB");
    let pool = store.pool().clone();
    Fixture {
        _dir: dir,
        identity: SqliteIdentityStore::new(pool.clone()),
        agent: SqliteAgentStore::new(pool.clone()),
        room: SqliteRoomStore::new(pool.clone()),
        pool,
    }
}

/// 通过 schema 直接创建第二个 Human（同 `room_store_tests` 的夹具手法）。
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

async fn create_group(f: &Fixture, owner: &Participant, name: &str, key: &str) -> RoomId {
    let outcome = room_application::create_group(&f.room, owner, name, key)
        .await
        .expect("create group");
    let room_application::CreateRoomOutcome::Created { room, .. } = outcome else {
        panic!("first group create must be Created");
    };
    room.id
}

async fn create_direct(f: &Fixture, human: &Participant, agent_id: &str, key: &str) -> RoomId {
    let outcome = room_application::create_direct_room(&f.room, human, agent_id, key)
        .await
        .expect("create direct room");
    let room_application::CreateDirectRoomOutcome::Created { room, .. } = outcome else {
        panic!("first room create must be Created");
    };
    room.id
}

/// accepted 联系人 fixture（group 邀请前置事实）。
async fn accept_contact(f: &Fixture, a: &Participant, b: &Participant) {
    let (canonical_a, canonical_b) = crate::domain::contact::canonical_pair(&a.id, &b.id);
    sqlx::query(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at, idempotency_key) \
         VALUES (?, ?, ?, ?, ?, 'accepted', 1, 1, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(canonical_a.as_str())
    .bind(canonical_b.as_str())
    .bind(canonical_a.as_str())
    .bind(canonical_b.as_str())
    .bind(Uuid::new_v4().to_string())
    .execute(&f.pool)
    .await
    .expect("insert accepted contact fixture");
}

async fn invite(f: &Fixture, owner: &Participant, room: &RoomId, target: &Participant, key: &str) {
    room_application::invite_group_member(&f.room, owner, room.as_str(), target.id.as_str(), key)
        .await
        .expect("invite member");
}

/// 直接 SQL 写一行提醒档。
async fn raw_insert(
    pool: &SqlitePool,
    room: &RoomId,
    participant: &ParticipantId,
    level: &str,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO room_attention (room_id, participant_id, level, updated_at) VALUES (?, ?, ?, 1)",
    )
    .bind(room.as_str())
    .bind(participant.as_str())
    .bind(level)
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// 直接 SQL 改一行提醒档（三个参数都可改，正因为可改才需要守卫）。
async fn raw_update(
    pool: &SqlitePool,
    room: &RoomId,
    participant: &ParticipantId,
    next_room: &RoomId,
    next_participant: &ParticipantId,
    level: &str,
) -> Result<(), String> {
    sqlx::query(
        "UPDATE room_attention SET room_id = ?, participant_id = ?, level = ?, updated_at = 2 \
         WHERE room_id = ? AND participant_id = ?",
    )
    .bind(next_room.as_str())
    .bind(next_participant.as_str())
    .bind(level)
    .bind(room.as_str())
    .bind(participant.as_str())
    .execute(pool)
    .await
    .map(|_| ())
    .map_err(|e| e.to_string())
}

async fn attention_rows(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM room_attention")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn attention_level(pool: &SqlitePool, room: &RoomId, participant: &ParticipantId) -> String {
    sqlx::query_scalar("SELECT level FROM room_attention WHERE room_id = ? AND participant_id = ?")
        .bind(room.as_str())
        .bind(participant.as_str())
        .fetch_one(pool)
        .await
        .unwrap()
}

/// 守卫 1：**只 group 房 + 本人是该房 active 成员**（两个失败同形、都 fail closed）。
#[tokio::test]
async fn attention_insert_guard_requires_a_group_room_and_an_active_member() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.expect("alice");
    let bob = add_human(&f.pool, "Bob").await;
    let agent_id = create_agent(&f, &alice.participant, "Agent A", "mig-at-a-1").await;
    accept_contact(&f, &alice.participant, &bob).await;

    let group = create_group(&f, &alice.participant, "有提醒档的群", "mig-at-g-1").await;
    invite(&f, &alice.participant, &group, &bob, "mig-at-g-1-b").await;
    let direct = create_direct(&f, &alice.participant, agent_id.as_str(), "mig-at-d-1").await;

    // (a) group 房 + active 成员 ⇒ 写得进来（正例：两道守卫都没有误伤）。
    raw_insert(&f.pool, &group, &alice.participant.id, "mentions")
        .await
        .expect("an active member of a group room can write its own row");
    assert_eq!(
        attention_level(&f.pool, &group, &alice.participant.id).await,
        "mentions"
    );
    assert_eq!(attention_rows(&f.pool).await, 1);

    // (b) `direct` 房：即便我确实是这间房的 active 成员，两人房里也没有
    //     「群消息」这一档 ⇒ 守卫 1 当场拒绝（不是"存下来但没人读"）。
    let denied = raw_insert(&f.pool, &direct, &alice.participant.id, "all")
        .await
        .expect_err("a direct room has no attention level");
    assert!(
        denied.contains("group room and an active membership"),
        "rejected by the room-attention guard, not by an unrelated constraint: {denied}"
    );
    assert_eq!(attention_rows(&f.pool).await, 1, "被拒的写入不留行");

    // (c) 群房，但写入者不是该房成员（Agent A 不在那个群）⇒ 同一个拒绝。
    let agent_participant = ParticipantId(agent_id.as_str().to_owned());
    let denied = raw_insert(&f.pool, &group, &agent_participant, "all")
        .await
        .expect_err("a non-member cannot write an attention row");
    assert!(
        denied.contains("group room and an active membership"),
        "{denied}"
    );
    assert_eq!(attention_rows(&f.pool).await, 1);

    // (d) 取值闭集由表级 CHECK 兜住（Bob 此时仍是 active 成员 ⇒ 守卫放行，
    //     轮到 CHECK 说话）。这一条证明"闭集"不依赖任何一个写路径自己记得校验。
    let denied = raw_insert(&f.pool, &group, &bob.id, "none")
        .await
        .expect_err("level is a closed set");
    assert!(denied.contains("CHECK"), "{denied}");
    assert_eq!(attention_rows(&f.pool).await, 1);

    // (e) 离群之后：Bob 的周期变 closed ⇒ 他在这间房**再也写不进来**
    //     （"成员 active"是写入时的判据，不是历史判据）。
    room_application::leave_membership(&f.room, &bob.id, group.as_str())
        .await
        .expect("bob leaves the group");
    let denied = raw_insert(&f.pool, &group, &bob.id, "all")
        .await
        .expect_err("a departed member cannot write an attention row");
    assert!(
        denied.contains("group room and an active membership"),
        "{denied}"
    );
    assert_eq!(attention_rows(&f.pool).await, 1);
}

/// 守卫 2：**只能写本人那一行**——行身份不可改写；改值仍须 group + active 成员。
#[tokio::test]
async fn attention_update_guard_only_writes_the_own_row() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice").await.expect("alice");
    let bob = add_human(&f.pool, "Bob").await;
    accept_contact(&f, &alice.participant, &bob).await;

    let group = create_group(&f, &alice.participant, "有提醒档的群", "mig-at2-g-1").await;
    invite(&f, &alice.participant, &group, &bob, "mig-at2-g-1-b").await;
    let other_group = create_group(&f, &alice.participant, "另一间群", "mig-at2-g-2").await;

    raw_insert(&f.pool, &group, &alice.participant.id, "mentions")
        .await
        .expect("seed alice's own row");

    // (a) 改**取值**（身份不变、仍 group + active 成员）⇒ 允许。
    raw_update(
        &f.pool,
        &group,
        &alice.participant.id,
        &group,
        &alice.participant.id,
        "all",
    )
    .await
    .expect("the owner of the row can change its level");
    assert_eq!(
        attention_level(&f.pool, &group, &alice.participant.id).await,
        "all"
    );

    // (b) 把这一行**转投给别人**（同一间房、另一个人）⇒ 拒绝：那是别人那一行。
    //     ⚠ Oracle 的锋利处：这一刻 Alice 仍是该群 active 成员、房仍是 group，
    //     所以只有"身份改写了"这一半可能触发——不需要从一句话里分辨是谁拒绝的。
    let denied = raw_update(
        &f.pool,
        &group,
        &alice.participant.id,
        &group,
        &bob.id,
        "all",
    )
    .await
    .expect_err("a row cannot be re-pointed at another participant");
    assert!(
        denied.contains("rewritten in place"),
        "rejected by the identity guard: {denied}"
    );
    assert_eq!(
        attention_level(&f.pool, &group, &alice.participant.id).await,
        "all",
        "被拒的改写不改原行"
    );

    // (c) 把这一行**搬去另一间房** ⇒ 拒绝（身份同样是身份）。
    let denied = raw_update(
        &f.pool,
        &group,
        &alice.participant.id,
        &other_group,
        &alice.participant.id,
        "all",
    )
    .await
    .expect_err("a row cannot be moved to another room");
    assert!(denied.contains("rewritten in place"), "{denied}");
    assert_eq!(attention_rows(&f.pool).await, 1, "行没有多出一份");

    // (d) 取值闭集对 UPDATE 同样生效（表级 CHECK 不分 INSERT/UPDATE）。
    let denied = raw_update(
        &f.pool,
        &group,
        &alice.participant.id,
        &group,
        &alice.participant.id,
        "none",
    )
    .await
    .expect_err("level is a closed set on update too");
    assert!(denied.contains("CHECK"), "{denied}");
    assert_eq!(
        attention_level(&f.pool, &group, &alice.participant.id).await,
        "all"
    );

    // (e) 离群之后我这一档冻结：改不动（同一道守卫的"active 成员"那半）。
    //     ⚠ Oracle 的锋利处：这一刻行身份**原样未动**、房仍是 group，所以只有
    //     "成员不再 active"这一半可能触发。
    raw_insert(&f.pool, &group, &bob.id, "mentions")
        .await
        .expect("seed bob's own row while he is still a member");
    room_application::leave_membership(&f.room, &bob.id, group.as_str())
        .await
        .expect("bob leaves the group");
    let denied = raw_update(&f.pool, &group, &bob.id, &group, &bob.id, "all")
        .await
        .expect_err("a departed member cannot change the level");
    assert!(denied.contains("active member of a group room"), "{denied}");
    assert_eq!(
        attention_level(&f.pool, &group, &bob.id).await,
        "mentions",
        "离群前的取值原样留着（不改消息主链，也不清行）"
    );
}
