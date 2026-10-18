//! SQLite schema 回归测试：BEFORE INSERT identity guard 拒绝 INSERT OR
//! REPLACE / REPLACE 对不可变公共事实的删除并重建。
//!
//! SQLite 的 REPLACE 冲突解决会先隐式删除冲突行；默认 recursive_triggers=
//! OFF 下这些隐式删除不触发 no-delete 触发器，可绕过 immutable / no-delete
//! 守卫改写既有事实（accepted 联系人改回 pending、revoked Membership
//! 复活、coverage / 幂等映射改写、Message / Mention / Event 替换）。每个
//! 不可变事实表都带 BEFORE INSERT identity guard，在插入阶段直接拒绝任一
//! 身份已占用的写入。本文件在真实临时数据库上通过公开用例建立事实，再对
//! 每个 UNIQUE 身份路径（不只主键）证明普通重复 INSERT、INSERT OR
//! REPLACE、REPLACE 全部被拒且原始事实不变。

use sqlx::Connection;
use sqlx::sqlite::SqlitePool;
use uuid::Uuid;

use crate::adapters::sqlite::agent_store::SqliteAgentStore;
use crate::adapters::sqlite::contact_store::SqliteContactStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::message_store::SqliteMessageStore;
use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::room_store::SqliteRoomStore;
use crate::application::agent;
use crate::application::contact as contact_application;
use crate::application::identity::bootstrap_human;
use crate::application::room as room_application;
use crate::domain::agent::IdempotencyKey;
use crate::domain::message::{Message, MessageBody, MessageId};
use crate::domain::participant::{DisplayName, Participant, ParticipantId};
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::ports::message::{MessageStore as _, NewMessage, SendMessageOutcome};

struct Fixture {
    _dir: tempfile::TempDir,
    pool: SqlitePool,
    identity: SqliteIdentityStore,
    agent: SqliteAgentStore,
    room: SqliteRoomStore,
    contact: SqliteContactStore,
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
        contact: SqliteContactStore::new(pool.clone()),
        message: SqliteMessageStore::new(pool.clone()),
    }
}

/// 通过 schema 直接创建第二个 Human（联系人/群成员用）。
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
    .expect("insert human");
    Participant::new_human(
        id,
        DisplayName::parse(display_name).unwrap(),
        UtcMillis::now(),
    )
}

/// 联系人 / human_direct 配对的规范顺序 (a_id < b_id)。
fn canonical_ids(a: &Participant, b: &Participant) -> (String, String) {
    if a.id.as_str() < b.id.as_str() {
        (a.id.as_str().to_owned(), b.id.as_str().to_owned())
    } else {
        (b.id.as_str().to_owned(), a.id.as_str().to_owned())
    }
}

/// 通过公开用例创建 Agent（附带 active Agent Owner 关系），返回其 ID。
async fn create_agent(f: &Fixture, human: &Participant, name: &str, key: &str) -> ParticipantId {
    let created = agent::create_agent(&f.agent, human, name, key)
        .await
        .expect("create agent");
    let agent::CreateAgentOutcome::Created { agent, .. } = created else {
        panic!("agent create must be Created");
    };
    agent.id
}

/// 通过公开用例创建 Human–Agent Direct Room，返回 Room ID。
async fn create_direct_room(f: &Fixture, human: &Participant, agent_id: &str, key: &str) -> RoomId {
    let outcome = room_application::create_direct_room(&f.room, human, agent_id, key)
        .await
        .expect("create direct room");
    let room_application::CreateDirectRoomOutcome::Created { room, .. } = outcome else {
        panic!("room create must be Created");
    };
    room.id
}

/// 建立 alice → 目标 Human 的 accepted 联系人关系。
async fn accept_contact_with(
    f: &Fixture,
    requester: &Participant,
    addressee: &Participant,
    key: &str,
) {
    let outcome = contact_application::send_contact_request(
        &f.contact,
        requester,
        addressee.id.as_str(),
        key,
    )
    .await
    .expect("contact request must succeed");
    let contact_application::SendContactRequestOutcome::Created { contact, .. } = outcome else {
        panic!("contact request must be Created");
    };
    assert_eq!(
        contact.status,
        crate::domain::contact::ContactStatus::Pending
    );
    contact_application::accept_contact(&f.contact, &addressee.id, requester.id.as_str())
        .await
        .expect("contact accept must succeed");
}

/// 通过公开用例创建 Human–Human Direct Room，返回 Room ID。
async fn create_human_direct_room(
    f: &Fixture,
    creator: &Participant,
    other: &Participant,
    key: &str,
) -> RoomId {
    let outcome =
        room_application::create_human_direct_room(&f.room, creator, other.id.as_str(), key)
            .await
            .expect("create human direct room");
    let room_application::CreateRoomOutcome::Created { room, .. } = outcome else {
        panic!("human direct room create must be Created");
    };
    room.id
}

/// 通过公开用例创建 group Room，返回 Room ID。
async fn create_group(f: &Fixture, owner: &Participant, name: &str, key: &str) -> RoomId {
    let outcome = room_application::create_group(&f.room, owner, name, key)
        .await
        .expect("create group");
    let room_application::CreateRoomOutcome::Created { room, .. } = outcome else {
        panic!("group create must be Created");
    };
    room.id
}

/// 通过公开用例邀请群成员（invite 必带 Idempotency-Key；这里按目标派生
/// 稳定 key，保证同目标重复调用可幂等重放）。
async fn invite(f: &Fixture, owner: &Participant, room_id: &RoomId, participant: &ParticipantId) {
    let key = format!("inv-{}", participant.as_str());
    room_application::invite_group_member(
        &f.room,
        owner,
        room_id.as_str(),
        participant.as_str(),
        &key,
    )
    .await
    .expect("invite must succeed");
}

/// 通过 Message 端口提交一条带 Mention 的群消息，返回提交事实。
async fn send_mention(
    store: &SqliteMessageStore,
    actor_id: &ParticipantId,
    room_id: &RoomId,
    body: &str,
    key: &str,
    target: &ParticipantId,
) -> Message {
    let outcome = store
        .send_message(
            &NewMessage {
                id: MessageId::new_v4(),
                room_id: room_id.clone(),
                actor_id: actor_id.clone(),
                body: Some(MessageBody::parse(body).expect("test body valid")),
                mentions: vec![target.clone()],
                attachments: Vec::new(),
                reply_to_message_id: None,
                created_at: UtcMillis::now(),
            },
            &IdempotencyKey::parse(key).expect("test key valid"),
        )
        .await
        .expect("send mention message");
    match outcome {
        SendMessageOutcome::Created { message } | SendMessageOutcome::Replayed { message } => {
            message
        }
    }
}

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

/// 三种改写形式（普通重复 INSERT、INSERT OR REPLACE、REPLACE）都必须被拒
/// 绝，且错误命中任一允许原文。
async fn assert_all_forms_rejected(
    pool: &SqlitePool,
    sql_plain: &str,
    needles: &[&str],
    ctx: &str,
) {
    for sql in [
        sql_plain,
        &sql_plain.replacen("INSERT INTO", "INSERT OR REPLACE INTO", 1),
        &sql_plain.replacen("INSERT INTO", "REPLACE INTO", 1),
    ] {
        let err = rejected(pool, sql).await;
        assert!(
            needles.iter().any(|needle| err.contains(needle)),
            "{ctx}: unexpected error: {err}"
        );
    }
}

/// 构造一次完整合法直接 SQL 转让的全部语句（顺序：插入 transition（旧
/// owner 与目标 member 仍 active，INSERT guard 通过；新周期 FK deferred）
/// → 关闭旧 owner 与目标 member（transfer 关闭守卫找到刚插入的匹配
/// transition）→ 打开新 owner 与旧 owner member → 更新 coverage → 写幂等
/// 记录）。测试可替换其中任一条来注入错配/缺步，验证写入期守卫与 COMMIT
/// 期 deferred 复合外键裁决。参数即 group_owner_transitions 的固定列集，
/// 不另行造类型。
#[allow(clippy::too_many_arguments)]
fn transfer_tx_sql(
    room_id: &str,
    old_owner_id: &str,
    target_id: &str,
    old_owner_mid: &str,
    target_mid: &str,
    new_owner_mid: &str,
    old_owner_member_mid: &str,
    transition_id: &str,
    occurred_at: i64,
    boundary: i64,
    key: &str,
) -> Vec<String> {
    vec![
        format!(
            "INSERT INTO group_owner_transitions \
             (transition_id, room_id, old_owner_participant_id, target_participant_id, \
              old_owner_membership_id, target_member_membership_id, new_owner_membership_id, \
              old_owner_new_member_membership_id, occurred_at, boundary_sequence) \
             VALUES ('{transition_id}', '{room_id}', '{old_owner_id}', '{target_id}', \
                     '{old_owner_mid}', '{target_mid}', '{new_owner_mid}', \
                     '{old_owner_member_mid}', {occurred_at}, {boundary})"
        ),
        format!(
            "UPDATE room_memberships \
             SET active_marker = 0, left_at = {occurred_at}, left_before_sequence = {boundary}, \
                 closed_by = 'transfer' \
             WHERE membership_id = '{old_owner_mid}'"
        ),
        format!(
            "UPDATE room_memberships \
             SET active_marker = 0, left_at = {occurred_at}, left_before_sequence = {boundary}, \
                 closed_by = 'transfer' \
             WHERE membership_id = '{target_mid}'"
        ),
        format!(
            "INSERT INTO room_memberships \
             (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, \
              active_marker) \
             VALUES ('{new_owner_mid}', '{room_id}', '{target_id}', 'owner', \
                     {occurred_at}, {boundary}, 1)"
        ),
        format!(
            "INSERT INTO room_memberships \
             (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, \
              active_marker) \
             VALUES ('{old_owner_member_mid}', '{room_id}', '{old_owner_id}', 'member', \
                     {occurred_at}, {boundary}, 1)"
        ),
        format!(
            "UPDATE room_coverage SET group_owner_membership_id = '{new_owner_mid}' \
             WHERE room_id = '{room_id}'"
        ),
        format!(
            "INSERT INTO group_owner_transfer_idempotency \
             (human_id, key, transition_id, created_at) \
             VALUES ('{old_owner_id}', '{key}', '{transition_id}', {occurred_at})"
        ),
    ]
}

/// 在单个事务内顺序执行语句并 COMMIT；任何语句失败或 COMMIT 失败自动
/// ROLLBACK 并返回数据库错误原文；成功返回 Ok。
async fn commit_tx(pool: &SqlitePool, statements: &[String]) -> Result<(), String> {
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("BEGIN").execute(&mut *conn).await.unwrap();
    for sql in statements {
        if let Err(e) = sqlx::query(sql).execute(&mut *conn).await {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            drop(conn);
            return Err(match e {
                sqlx::Error::Database(d) => d.message().to_owned(),
                other => format!("{other}"),
            });
        }
    }
    let commit = sqlx::query("COMMIT").execute(&mut *conn).await;
    if commit.is_err() {
        // deferred 外键失败后事务仍打开：必须显式回滚，否则连接回到池中
        // 仍持有写锁，后续 BEGIN IMMEDIATE 会撞 database is locked。
        let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
    }
    drop(conn);
    match commit {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(d)) => Err(d.message().to_owned()),
        Err(other) => Err(format!("{other}")),
    }
}

#[tokio::test]
async fn contacts_replace_cannot_revert_accepted_or_rewrite_request_key() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    let charlie = add_human(&f.pool, "Charlie").await;
    accept_contact_with(&f, &alice, &bob, "k-ab").await;

    let (a_ab, b_ab) = canonical_ids(&alice, &bob);
    // 同对 active 段路径：把 accepted 改回 pending 并换掉 accepted_at。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO contact_periods \
             (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at, idempotency_key) \
             VALUES ('00000000-0000-4000-8000-00000000c001', '{a_ab}', '{b_ab}', '{}', '{}', 'pending', 10, NULL, 'k-new')",
            alice.id.as_str(),
            bob.id.as_str(),
        ),
        &["contact period identity already exists"],
        "contact period active pair path",
    )
    .await;

    let (a_ac, b_ac) = canonical_ids(&alice, &charlie);
    // (requester_id, idempotency_key) 唯一路径：同 requester + key 改写为
    // 另一对 Human。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO contact_periods \
             (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at, idempotency_key) \
             VALUES ('00000000-0000-4000-8000-00000000c002', '{a_ac}', '{b_ac}', '{}', '{}', 'pending', 11, NULL, 'k-ab')",
            alice.id.as_str(),
            charlie.id.as_str(),
        ),
        &["contact period identity already exists"],
        "contact period request key path",
    )
    .await;
    let (status, accepted_at, requester, addressee, key): (
        String,
        Option<i64>,
        String,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT state, accepted_at, requester_id, addressee_id, idempotency_key \
             FROM contact_periods WHERE a_id = ? AND b_id = ?",
    )
    .bind(a_ab.as_str())
    .bind(b_ab.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(status, "accepted");
    assert!(accepted_at.is_some_and(|v| v > 0));
    assert_eq!(requester, alice.id.as_str());
    assert_eq!(addressee, bob.id.as_str());
    assert_eq!(key, "k-ab");
}

#[tokio::test]
async fn direct_room_facts_cannot_be_replaced() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let agent_id = create_agent(&f, &alice, "助手", "a-k").await;
    let other_agent = create_agent(&f, &alice, "乙", "a-k2").await;
    let room_id = create_direct_room(&f, &alice, agent_id.as_str(), "r-k").await;
    let (original_room_created_at,): (i64,) =
        sqlx::query_as("SELECT created_at FROM rooms WHERE id = ?")
            .bind(room_id.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    let (original_coverage_created_at,): (i64,) =
        sqlx::query_as("SELECT created_at FROM room_coverage WHERE room_id = ?")
            .bind(room_id.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();

    // rooms 主键路径：同 id 改写 kind / created_at / name。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO rooms (id, kind, status, created_at, name) \
             VALUES ('{}', 'group', 'active', 999, 'x')",
            room_id.as_str(),
        ),
        &["room identity already exists"],
        "rooms primary key path",
    )
    .await;

    // room_coverage 主键路径：改写覆盖 created_at（结构见证身份必须与
    // 既有覆盖行一致，保证拒绝来自身份守卫而非 CHECK）。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO room_coverage \
             (room_id, kind, created_at, direct_human_id, direct_agent_id) \
             VALUES ('{}', 'direct', 999, '{}', '{}')",
            room_id.as_str(),
            alice.id.as_str(),
            agent_id.as_str(),
        ),
        &["room coverage identity already exists"],
        "room coverage primary key path",
    )
    .await;

    // direct_room_pairs 主键 (room_id) 路径：把配对改挂到另一 Agent。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO direct_room_pairs (room_id, human_id, agent_id, created_at) \
             VALUES ('{}', '{}', '{}', 999)",
            room_id.as_str(),
            alice.id.as_str(),
            other_agent.as_str(),
        ),
        &["direct room pair identity already exists"],
        "direct pair room id path",
    )
    .await;

    // 一间房 = 一段关系（批次二）：同一对 Human–Agent 再要一次**回到原房**，
    // 不再新建第二间（"同一对可开多间"这条已被本切片收回）。
    let replayed = room_application::create_direct_room(&f.room, &alice, agent_id.as_str(), "r-k2")
        .await
        .expect("replay of the same pair must succeed");
    let room_application::CreateDirectRoomOutcome::Replayed {
        room: replayed_room,
        ..
    } = replayed
    else {
        panic!("the same pair must be replayed into its one room");
    };
    assert_eq!(
        replayed_room.id, room_id,
        "a second create for the same pair must return the room of that relation"
    );
    // 该房已有自己的配对：任何 REPLACE/INSERT 把配对改挂到已占用的 room_id 都被
    // room_id 守卫拒绝。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO direct_room_pairs (room_id, human_id, agent_id, created_at) \
             VALUES ('{}', '{}', '{}', 999)",
            room_id.as_str(),
            alice.id.as_str(),
            agent_id.as_str(),
        ),
        &["direct room pair identity already exists"],
        "direct pair room id path (occupied room)",
    )
    .await;

    // direct_room_idempotency 主键 (human_id, key) 路径：改写 Room 映射。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO direct_room_idempotency (human_id, key, agent_id, room_id, created_at) \
             VALUES ('{}', 'r-k', '{}', '{}', 999)",
            alice.id.as_str(),
            agent_id.as_str(),
            room_id.as_str(),
        ),
        &["direct room idempotency identity already exists"],
        "direct room idempotency path",
    )
    .await;

    // room_memberships 身份路径：改写 active Membership 的 role / joined_at
    // （仅身份冲突，其余守卫都通过）。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO room_memberships \
             (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
             VALUES ('{}', '{}', '{}', 'member', 1, 0, 1)",
            Uuid::new_v4(),
            room_id.as_str(),
            alice.id.as_str(),
        ),
        &["room membership identity already exists"],
        "direct membership identity path",
    )
    .await;

    // Agent 撤销后其 Membership 周期 closed：REPLACE 不能复活为 active。
    agent::revoke_agent(&f.agent, &alice.id, agent_id.as_str())
        .await
        .expect("revoke agent");
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO room_memberships \
             (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
             VALUES ('{}', '{}', '{}', 'member', 1, 0, 1)",
            Uuid::new_v4(),
            room_id.as_str(),
            agent_id.as_str(),
        ),
        &[
            "room membership identity already exists",
            "active membership requires active participant",
        ],
        "revoked direct membership resurrection",
    )
    .await;

    // 全部事实保持原状：**只剩这一间**（同一对的第二次创建回到了它，没有第二间）。
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM rooms")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        1,
        "one relation, one room: the replay created nothing"
    );
    let (kind, created_at): (String, i64) =
        sqlx::query_as("SELECT kind, created_at FROM rooms WHERE id = ?")
            .bind(room_id.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(kind, "direct");
    assert_eq!(
        created_at, original_room_created_at,
        "room created_at must be the original"
    );
    let (kind, coverage_created_at): (String, i64) =
        sqlx::query_as("SELECT kind, created_at FROM room_coverage WHERE room_id = ?")
            .bind(room_id.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(kind, "direct");
    assert_eq!(
        coverage_created_at, original_coverage_created_at,
        "coverage created_at must be the original"
    );
    // 配对仍钉在自己的 Room 上（同一对只有这一条配对）。
    let (pair_room_id, pair_agent): (String, String) =
        sqlx::query_as("SELECT room_id, agent_id FROM direct_room_pairs WHERE room_id = ?")
            .bind(room_id.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(pair_room_id, room_id.as_str(), "pair must keep its room");
    assert_eq!(pair_agent, agent_id.as_str());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM direct_room_pairs")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        1,
        "one relation, one pair"
    );
    let (mapped_room_id,): (String,) = sqlx::query_as(
        "SELECT room_id FROM direct_room_idempotency WHERE human_id = ? AND key = 'r-k'",
    )
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        mapped_room_id,
        room_id.as_str(),
        "idempotency mapping must not change"
    );
    let (active, left_at): (i64, Option<i64>) = sqlx::query_as(
        "SELECT active_marker, left_at FROM room_memberships \
             WHERE room_id = ? AND participant_id = ?",
    )
    .bind(room_id.as_str())
    .bind(agent_id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(active, 0);
    assert!(left_at.is_some_and(|v| v > 0));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM room_memberships")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        2,
        "one fully closed direct room, two memberships"
    );
}

#[tokio::test]
async fn human_direct_facts_cannot_be_replaced() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    let charlie = add_human(&f.pool, "Charlie").await;
    accept_contact_with(&f, &alice, &bob, "k-ab").await;
    accept_contact_with(&f, &alice, &charlie, "k-ac").await;
    let (a_ab, b_ab) = canonical_ids(&alice, &bob);
    let (a_ac, b_ac) = canonical_ids(&alice, &charlie);
    let room_id = create_human_direct_room(&f, &alice, &bob, "hdr-k").await;

    // human_direct_pairs 主键 (room_id) 路径：把配对换成另一对 Human。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO human_direct_pairs \
             (room_id, human_a_id, human_b_id, role_a, role_b, created_at) \
             VALUES ('{}', '{a_ac}', '{b_ac}', 'owner', 'member', 999)",
            room_id.as_str(),
        ),
        &["human direct pair identity already exists"],
        "human direct pair room id path",
    )
    .await;

    // human_direct_pairs 的 UNIQUE (human_a_id, human_b_id) 路径：把同一
    // 对 Human 改挂到另一 Room。该 Room 必须真实存在且结构闭合（rooms →
    // coverage → 配对 → 两份 Membership 才能提交），因此用公开用例创建
    // 第二个完整 Human direct Room（alice–charlie 已 accepted）。
    let second_room = create_human_direct_room(&f, &alice, &charlie, "hdr-k2").await;
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO human_direct_pairs \
             (room_id, human_a_id, human_b_id, role_a, role_b, created_at) \
             VALUES ('{second_room}', '{a_ab}', '{b_ab}', 'owner', 'member', 999)"
        ),
        &["human direct pair identity already exists"],
        "human direct pair human pair path",
    )
    .await;

    // human_direct_room_idempotency 主键 (human_id, key) 路径：改写
    // 对端 Human / Room 映射。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO human_direct_room_idempotency \
             (human_id, key, peer_human_id, canonical_a, canonical_b, room_id, created_at) \
             VALUES ('{}', 'hdr-k', '{}', '{a_ac}', '{b_ac}', '{}', 999)",
            alice.id.as_str(),
            charlie.id.as_str(),
            room_id.as_str(),
        ),
        &["human direct room idempotency identity already exists"],
        "human direct idempotency path",
    )
    .await;
    // 事实保持：配对仍挂在原 Room（canonical 顺序，human_a 不一定是
    // 创建者），幂等记录仍映射原对端与原 Room。
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM human_direct_pairs")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        2,
        "two fully closed rooms, each with its own pair"
    );
    let (pair_room, a, b): (String, String, String) = sqlx::query_as(
        "SELECT room_id, human_a_id, human_b_id FROM human_direct_pairs \
         WHERE room_id = ?",
    )
    .bind(room_id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(pair_room, room_id.as_str());
    assert_eq!(a, a_ab.as_str());
    assert_eq!(b, b_ab.as_str());
    let (peer, mapped_room): (String, String) = sqlx::query_as(
        "SELECT peer_human_id, room_id FROM human_direct_room_idempotency \
         WHERE human_id = ? AND key = 'hdr-k'",
    )
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(peer, bob.id.as_str());
    assert_eq!(mapped_room, room_id.as_str());
}

#[tokio::test]
async fn group_create_idempotency_cannot_remap_room() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    accept_contact_with(&f, &alice, &bob, "k-ab").await;
    let group_id = create_group(&f, &alice, "项目群", "g-k").await;
    let other_room = create_human_direct_room(&f, &alice, &bob, "hdr-k").await;
    let (original_group_created_at,): (i64,) = sqlx::query_as(
        "SELECT created_at FROM group_create_idempotency WHERE human_id = ? AND key = 'g-k'",
    )
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();

    // group_create_idempotency 主键 (human_id, key) 路径：把同 key 重映射
    // 到另一 Room。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO group_create_idempotency (human_id, key, room_id, name, created_at) \
             VALUES ('{}', 'g-k', '{}', '项目群', 999)",
            alice.id.as_str(),
            other_room.as_str(),
        ),
        &["group create idempotency identity already exists"],
        "group create idempotency remap path",
    )
    .await;

    // 事实保持：幂等记录仍映射原群 Room，rooms 总数不增加。
    let (mapped_room, name, created_at): (String, String, i64) = sqlx::query_as(
        "SELECT room_id, name, created_at FROM group_create_idempotency \
         WHERE human_id = ? AND key = 'g-k'",
    )
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(mapped_room, group_id.as_str());
    assert_eq!(name, "项目群");
    assert_eq!(
        created_at, original_group_created_at,
        "created_at must be the original"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM rooms")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn closed_membership_periods_cannot_be_rewritten_or_duplicated() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    accept_contact_with(&f, &alice, &bob, "k-ab").await;
    let group_id = create_group(&f, &alice, "群", "g-k").await;
    invite(&f, &alice, &group_id, &bob.id).await;
    let (closed_membership_id,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ?",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    room_application::remove_group_member(&f.room, &alice, group_id.as_str(), bob.id.as_str())
        .await
        .expect("remove bob");

    // closed 周期身份（membership_id）路径：REPLACE 不能改写 closed 行。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO room_memberships \
             (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
             VALUES ('{closed_membership_id}', '{}', '{}', 'member', 1, 0, 1)",
            group_id.as_str(),
            bob.id.as_str(),
        ),
        &["room membership identity already exists"],
        "closed membership id path",
    )
    .await;

    // (room_id, participant_id, active_marker) 身份路径：不允许再造一条
    // 同 Room+Participant 的 closed 周期（active_marker=0 已占用）。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO room_memberships \
             (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
             VALUES ('{}', '{}', '{}', 'member', 1, 0, 0)",
            Uuid::new_v4(),
            group_id.as_str(),
            bob.id.as_str(),
        ),
        &[
            "room membership identity already exists",
            "new room memberships must be active periods",
        ],
        "closed period duplicate path",
    )
    .await;

    // closed 周期不可改写（不允许 UPDATE 恢复 / 改 left 三元组）。
    let restore = sqlx::query(
        "UPDATE room_memberships \
         SET active_marker = 1, left_at = NULL, left_before_sequence = NULL, closed_by = NULL \
         WHERE membership_id = ?",
    )
    .bind(&closed_membership_id)
    .execute(&f.pool)
    .await;
    assert!(restore.is_err(), "closed period must not be restored");

    let (active, left_at, left_before, closed_by): (i64, Option<i64>, Option<i64>, Option<String>) =
        sqlx::query_as(
            "SELECT active_marker, left_at, left_before_sequence, closed_by \
             FROM room_memberships WHERE membership_id = ?",
        )
        .bind(&closed_membership_id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(active, 0);
    assert!(left_at.is_some_and(|v| v > 0));
    assert!(left_before.is_some());
    assert_eq!(closed_by.as_deref(), Some("removed"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM room_memberships")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        2,
        "owner + closed bob, no extra rows"
    );
}

#[tokio::test]
async fn messages_cannot_be_replaced_and_mentions_and_events_preserved() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let agent_id = create_agent(&f, &alice, "助手", "a-k").await;
    let group_id = create_group(&f, &alice, "群", "g-k").await;
    invite(&f, &alice, &group_id, &agent_id).await;
    let original = send_mention(&f.message, &alice.id, &group_id, "hi", "gm-1", &agent_id).await;
    let (mention_count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM mentions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(mention_count, 1);

    // 主键 (id) 路径：同 id 替换正文。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
             VALUES ('{}', '{}', 2, '{}', 'evil', 'k-evil', 999)",
            original.id.as_str(),
            group_id.as_str(),
            alice.id.as_str(),
        ),
        &["message identity already exists"],
        "messages primary key path",
    )
    .await;

    // UNIQUE (actor_id, idempotency_key) 路径：同 actor + key 换 id / body。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
             VALUES ('{}', '{}', 2, '{}', 'evil', 'gm-1', 999)",
            Uuid::new_v4(),
            group_id.as_str(),
            alice.id.as_str(),
        ),
        &["message identity already exists"],
        "messages actor+key path",
    )
    .await;

    // UNIQUE (room_id, sequence) 路径：同 Room + sequence 换 id / key。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO messages (id, room_id, sequence, actor_id, body, idempotency_key, created_at) \
             VALUES ('{}', '{}', 1, '{}', 'evil', 'k-evil2', 999)",
            Uuid::new_v4(),
            group_id.as_str(),
            alice.id.as_str(),
        ),
        &["message identity already exists"],
        "messages room+sequence path",
    )
    .await;

    // mentions 主键 (message_id, target_agent_id) 路径。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO mentions (message_id, target_agent_id) \
             VALUES ('{}', '{}')",
            original.id.as_str(),
            agent_id.as_str(),
        ),
        &["mention identity already exists"],
        "mentions primary key path",
    )
    .await;

    // 原始事实保持：Message 未被替换，关联 Mention 与 Event 仍指向原
    // Message ID，序列未被消耗。
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        1
    );
    let (body, sequence, key, created_at): (String, i64, String, i64) = sqlx::query_as(
        "SELECT body, sequence, idempotency_key, created_at FROM messages WHERE id = ?",
    )
    .bind(original.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(body, "hi");
    assert_eq!(sequence, 1);
    assert_eq!(key, "gm-1");
    assert_eq!(created_at, original.created_at.millis());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM mentions")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        1
    );
    let (event_id, event_seq, event_message): (String, i64, String) = sqlx::query_as(
        "SELECT id, sequence, message_id FROM agent_events \
         WHERE target_agent_id = ?",
    )
    .bind(agent_id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        event_message,
        original.id.as_str(),
        "event must keep the message"
    );
    assert_eq!(event_seq, 1);
    let _ = event_id;
    let room_sequences: Vec<i64> =
        sqlx::query_scalar("SELECT sequence FROM messages WHERE room_id = ? ORDER BY sequence")
            .bind(group_id.as_str())
            .fetch_all(&f.pool)
            .await
            .unwrap();
    assert_eq!(
        room_sequences,
        vec![1],
        "failed replaces must not consume sequence"
    );
}

#[tokio::test]
async fn messages_reply_column_rejects_illegal_targets_and_rewrites() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let agent_1 = create_agent(&f, &alice, "甲", "a-k1").await;
    let agent_2 = create_agent(&f, &alice, "乙", "a-k2").await;
    let room_a = create_direct_room(&f, &alice, agent_1.as_str(), "r-a").await;
    let room_b = create_direct_room(&f, &alice, agent_2.as_str(), "r-b").await;

    let m_a1 = send(&f.message, &alice.id, &room_a, "A-1", "a-m-1").await;
    let m_a2 = send(&f.message, &alice.id, &room_a, "A-2", "a-m-2").await;
    send(&f.message, &alice.id, &room_b, "B-1", "b-m-1").await;

    // 合法直接 SQL：同 Room 的已提交历史 Message 作 target，reply 与
    // source 同一行提交成功。
    let direct_id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO messages \
         (id, room_id, sequence, actor_id, body, reply_to_message_id, idempotency_key, created_at) \
         VALUES (?, ?, 3, ?, 'A-3', ?, 'd-m-1', ?)",
    )
    .bind(&direct_id)
    .bind(room_a.as_str())
    .bind(alice.id.as_str())
    .bind(m_a1.id.as_str())
    .bind(UtcMillis::now().millis())
    .execute(&f.pool)
    .await
    .expect("same-room committed target must be accepted directly");

    // 跨 Room：source 在 B、target 在 A → 插入守卫拒绝（三种插入形式）。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO messages \
             (id, room_id, sequence, actor_id, body, reply_to_message_id, idempotency_key, created_at) \
             VALUES ('{}', '{}', 2, '{}', 'x', '{}', 'd-m-x', 1)",
            Uuid::new_v4(),
            room_b.as_str(),
            alice.id.as_str(),
            m_a1.id.as_str(),
        ),
        &["reply target must be a same-room historical message visible to the actor"],
        "messages cross-room reply path",
    )
    .await;

    // 未知 target（规范 UUID 但不存在）→ 守卫拒绝。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO messages \
             (id, room_id, sequence, actor_id, body, reply_to_message_id, idempotency_key, created_at) \
             VALUES ('{}', '{}', 4, '{}', 'x', '{}', 'd-m-y', 1)",
            Uuid::new_v4(),
            room_a.as_str(),
            alice.id.as_str(),
            Uuid::new_v4(),
        ),
        &["reply target must be a same-room historical message visible to the actor"],
        "messages unknown reply target path",
    )
    .await;

    // 自指：reply_to_message_id == 同一行的 id（直接 SQL 才能构造）→
    // 守卫拒绝（messages 表 CHECK reply_to_message_id <> id 独立兜底）。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO messages \
             (id, room_id, sequence, actor_id, body, reply_to_message_id, idempotency_key, created_at) \
             VALUES ('self-ref', '{}', 5, '{}', 'x', 'self-ref', 'd-m-z', 1)",
            room_a.as_str(),
            alice.id.as_str(),
        ),
        &["reply target must be a same-room historical message visible to the actor"],
        "messages self-reference reply path",
    )
    .await;

    // 非法形状 fail closed：reply_to_message_id 非 TEXT 存储类先被 BEFORE
    // INSERT guard 拒绝（BEFORE 触发器先于 CHECK 求值）。
    let now = UtcMillis::now().millis();
    let err = rejected(
        &f.pool,
        &format!(
            "INSERT INTO messages \
             (id, room_id, sequence, actor_id, body, reply_to_message_id, idempotency_key, created_at) \
             VALUES ('shape-1', '{}', 6, '{}', 'x', X'DEADBEEF', 'd-m-w', {now})",
            room_a.as_str(),
            alice.id.as_str(),
        ),
    )
    .await;
    assert!(
        err.contains("reply target must be a same-room historical message visible to the actor"),
        "unexpected error: {err}"
    );

    // CHECK 兜底独立验证：移除 guard 后，非 TEXT 形状与自指仍被表级 CHECK
    // 拒绝（typeof / reply_to_message_id <> id）。
    sqlx::query("DROP TRIGGER messages_reply_target_guard")
        .execute(&f.pool)
        .await
        .expect("drop reply guard for CHECK backstop proof");
    let err = rejected(
        &f.pool,
        &format!(
            "INSERT INTO messages \
             (id, room_id, sequence, actor_id, body, reply_to_message_id, idempotency_key, created_at) \
             VALUES ('shape-1', '{}', 6, '{}', 'x', X'DEADBEEF', 'd-m-w', {now})",
            room_a.as_str(),
            alice.id.as_str(),
        ),
    )
    .await;
    assert!(err.contains("CHECK constraint failed"), "{err}");
    let err = rejected(
        &f.pool,
        &format!(
            "INSERT INTO messages \
             (id, room_id, sequence, actor_id, body, reply_to_message_id, idempotency_key, created_at) \
             VALUES ('self-ref', '{}', 5, '{}', 'x', 'self-ref', 'd-m-z', {now})",
            room_a.as_str(),
            alice.id.as_str(),
        ),
    )
    .await;
    assert!(err.contains("CHECK constraint failed"), "{err}");

    // reply 字段不可改写：UPDATE 被 messages_immutable 拒绝。
    let err = rejected(
        &f.pool,
        &format!(
            "UPDATE messages SET reply_to_message_id = '{}' WHERE id = '{}'",
            m_a2.id.as_str(),
            direct_id,
        ),
    )
    .await;
    assert!(err.contains("messages are immutable"), "{err}");

    // 身份替换防线：INSERT OR REPLACE / REPLACE 以主键 (id) 冲突时先隐式
    // 删除冲突行，可绕过 immutable 守卫改写 reply 字段；messages_insert_
    // identity_guard 在插入阶段直接拒绝，旧 Message 与 Event 不变。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO messages \
             (id, room_id, sequence, actor_id, body, reply_to_message_id, idempotency_key, created_at) \
             VALUES ('{direct_id}', '{room_a}', 3, '{alice}', 'REWRITTEN', '{m_a2}', 'd-m-1', 1)",
            direct_id = direct_id,
            room_a = room_a.as_str(),
            alice = alice.id.as_str(),
            m_a2 = m_a2.id.as_str(),
        ),
        &["message identity already exists"],
        "messages REPLACE identity path",
    )
    .await;

    // 全部失败后既有事实不变：直接行仍是 body='A-3'、target 仍指向 m_a1，
    // 全部 Agent Event 仍恰好 4 条（A-1/A-2/A-3 → agent_1，B-1 → agent_2）。
    let (body, target, sequence): (String, Option<String>, i64) =
        sqlx::query_as("SELECT body, reply_to_message_id, sequence FROM messages WHERE id = ?")
            .bind(&direct_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(body, "A-3");
    assert_eq!(target.as_deref(), Some(m_a1.id.as_str()));
    assert_eq!(sequence, 3);
    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_events")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(events, 4, "failed rewrites must not change events");
}

#[tokio::test]
async fn messages_reply_guard_rejects_invisible_target_in_transaction() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    let (ca, cb) = canonical_ids(&alice, &bob);
    sqlx::query(
        "INSERT INTO contact_periods (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, accepted_at, idempotency_key) \
         VALUES ('00000000-0000-4000-8000-00000000c003', ?, ?, ?, ?, 'accepted', 1, 1, 'k-ab')",
    )
    .bind(ca)
    .bind(cb)
    .bind(alice.id.as_str())
    .bind(bob.id.as_str())
    .execute(&f.pool)
    .await
    .expect("accepted contact");
    // Bob 建群、先发消息，再邀请 Alice：seq 1 对 Alice 的历史窗口不可见。
    let group_id = create_group(&f, &bob, "窗口群", "g-k").await;
    let pre_join = send(&f.message, &bob.id, &group_id, "加入前", "b-1").await;
    invite(&f, &bob, &group_id, &alice.id).await;
    // 邀请后 Bob 再发一条：seq 2 落在 Alice 的加入窗口内（可见）。
    let after_join = send(&f.message, &bob.id, &group_id, "加入后", "b-2").await;
    assert_eq!(after_join.sequence, 2);

    let messages_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
        .fetch_one(&f.pool)
        .await
        .unwrap();

    // 同一 BEGIN IMMEDIATE 事务内：source Message 行直接携带指向不可见
    // target 的 reply 字段 → 插入守卫失败 → 整体回滚，不留 Message、reply
    // 字段或 sequence。
    let mut conn = f.pool.acquire().await.unwrap();
    let mut tx = conn
        .begin_with("BEGIN IMMEDIATE")
        .await
        .expect("begin immediate");
    let new_source = Uuid::new_v4().to_string();
    let err = sqlx::query(
        "INSERT INTO messages \
         (id, room_id, sequence, actor_id, body, reply_to_message_id, idempotency_key, created_at) \
         VALUES (?, ?, 3, ?, 'source', ?, 'direct-sql', ?)",
    )
    .bind(&new_source)
    .bind(group_id.as_str())
    .bind(alice.id.as_str())
    .bind(pre_join.id.as_str())
    .bind(UtcMillis::now().millis())
    .execute(&mut *tx)
    .await
    .expect_err("reply to an invisible target must abort the insert");
    assert!(
        err.to_string()
            .contains("reply target must be a same-room historical message"),
        "unexpected error: {err}"
    );
    tx.rollback().await.expect("rollback");

    // 回滚后无任何残留，连接可复用：同一事务形状下指向可见 target 的合法
    // 直接 SQL 可提交。
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        messages_before,
        "canceled transaction must not leave the source message"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM messages WHERE reply_to_message_id IS NOT NULL"
        )
        .fetch_one(&f.pool)
        .await
        .unwrap(),
        0,
        "canceled transaction must not leave a reply target"
    );
    let sequences: Vec<i64> =
        sqlx::query_scalar("SELECT sequence FROM messages WHERE room_id = ? ORDER BY sequence")
            .bind(group_id.as_str())
            .fetch_all(&f.pool)
            .await
            .unwrap();
    assert_eq!(
        sequences,
        vec![1, 2],
        "canceled transaction must not consume sequence"
    );

    let mut conn = f.pool.acquire().await.unwrap();
    let mut tx = conn
        .begin_with("BEGIN IMMEDIATE")
        .await
        .expect("begin immediate");
    let ok_source = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO messages \
         (id, room_id, sequence, actor_id, body, reply_to_message_id, idempotency_key, created_at) \
         VALUES (?, ?, 3, ?, 'visible reply', ?, 'direct-ok', ?)",
    )
    .bind(&ok_source)
    .bind(group_id.as_str())
    .bind(alice.id.as_str())
    .bind(after_join.id.as_str())
    .bind(UtcMillis::now().millis())
    .execute(&mut *tx)
    .await
    .expect("visible in-window target must be accepted in a transaction");
    tx.commit().await.expect("commit");
    let (target,): (String,) =
        sqlx::query_as("SELECT reply_to_message_id FROM messages WHERE id = ?")
            .bind(&ok_source)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(target, after_join.id.as_str());
}

/// 通过 Message 端口提交一条普通消息（无 Mention），返回 Message 事实。
async fn send(
    store: &crate::adapters::sqlite::message_store::SqliteMessageStore,
    actor_id: &ParticipantId,
    room_id: &RoomId,
    body: &str,
    key: &str,
) -> crate::domain::message::Message {
    let outcome = store
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
        .expect("send message");
    match outcome {
        SendMessageOutcome::Created { message } | SendMessageOutcome::Replayed { message } => {
            message
        }
    }
}

#[tokio::test]
async fn agent_events_cannot_be_replaced() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let agent_id = create_agent(&f, &alice, "助手", "a-k").await;
    let group_id = create_group(&f, &alice, "群", "g-k").await;
    invite(&f, &alice, &group_id, &agent_id).await;
    let message = send_mention(&f.message, &alice.id, &group_id, "hi", "gm-1", &agent_id).await;
    let event_id: String = sqlx::query_scalar(
        "SELECT id FROM agent_events WHERE target_agent_id = ? ORDER BY sequence LIMIT 1",
    )
    .bind(agent_id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();

    // 主键 (id) 路径：同 id 换 sequence / message / created_at。同一写入
    // 同时违反插入守卫与身份守卫时先返回哪条不属契约（migration 011 重建
    // 插入守卫后其创建顺序变化，两个候选错误都合法）。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO agent_events (id, target_agent_id, sequence, message_id, created_at) \
             VALUES ('{event_id}', '{}', 2, '{}', 999)",
            agent_id.as_str(),
            message.id.as_str(),
        ),
        &[
            "agent event identity already exists",
            "agent event requires a paired human message",
        ],
        "agent events primary key path",
    )
    .await;

    // UNIQUE (target_agent_id, sequence) 路径：同 target + sequence 换 id
    //（该 sequence 冲突时 sequence-must-be-next 守卫也同时触发，先返回哪
    // 条不属于契约）。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO agent_events (id, target_agent_id, sequence, message_id, created_at) \
             VALUES ('{}', '{}', 1, '{}', 999)",
            Uuid::new_v4(),
            agent_id.as_str(),
            message.id.as_str(),
        ),
        &[
            "agent event identity already exists",
            "agent event sequence must be the next available",
            "agent event requires a paired human message",
        ],
        "agent events target+sequence path",
    )
    .await;

    // UNIQUE (kind, message_id, target_agent_id) 路径：同 source Message +
    // target 换 id / sequence（2 正是下一条可用 sequence，只触发身份守卫
    // 或插入守卫——created_at 与 source 不一致时插入守卫先触发）。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO agent_events (id, target_agent_id, sequence, message_id, created_at) \
             VALUES ('{}', '{}', 2, '{}', 999)",
            Uuid::new_v4(),
            agent_id.as_str(),
            message.id.as_str(),
        ),
        &[
            "agent event identity already exists",
            "agent event requires a paired human message",
        ],
        "agent events message+target path",
    )
    .await;

    // 原始事实保持：唯一 Event 未被替换，id / sequence / message /
    // created_at 不变。
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM agent_events")
            .fetch_one(&f.pool)
            .await
            .unwrap(),
        1
    );
    let (seq, event_message, created_at): (i64, String, i64) =
        sqlx::query_as("SELECT sequence, message_id, created_at FROM agent_events WHERE id = ?")
            .bind(&event_id)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(seq, 1);
    assert_eq!(event_message, message.id.as_str());
    assert_eq!(created_at, message.created_at.millis());
}

/// 数据库反向实验：同 Room+Participant 至多一个 active 周期（包括
/// REPLACE 形式也不能换掉既有 active 周期）；每 Room 至多一个 active
/// owner；closed 周期不可改（identity_guard + partial index 双防线）。
#[tokio::test]
async fn db_rejects_second_active_period_and_second_active_owner() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    let charlie = add_human(&f.pool, "Charlie").await;
    accept_contact_with(&f, &alice, &bob, "k-ab").await;
    accept_contact_with(&f, &alice, &charlie, "k-ac").await;
    let group_id = create_group(&f, &alice, "群", "g-k").await;
    invite(&f, &alice, &group_id, &bob.id).await;
    let (bob_active_membership_id,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();

    // 同 Room+Participant 第二个 active 周期：普通 INSERT / REPLACE 全拒。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO room_memberships \
             (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
             VALUES ('{}', '{}', '{}', 'member', 2, 0, 1)",
            Uuid::new_v4(),
            group_id.as_str(),
            bob.id.as_str(),
        ),
        &[
            "room membership identity already exists",
            "UNIQUE constraint failed",
        ],
        "second active period for the same room+participant",
    )
    .await;

    // 用 REPLACE 换掉既有 active 周期（同 room+participant 新 id）也拒绝，
    // 原周期身份不变。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO room_memberships \
             (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
             VALUES ('{}', '{}', '{}', 'owner', 2, 0, 1)",
            Uuid::new_v4(),
            group_id.as_str(),
            bob.id.as_str(),
        ),
        &[
            "room membership identity already exists",
            // ⚠ BEFORE 触发器之间的点火顺序不受承诺：这一行同时违反"同房+同参与者
            // 只能一个 active 周期"与"owner 唯一/成员资格"，两条守卫都会拒绝，
            // 谁的消息先到取决于 sqlite_master 里的顺序（2026-09-27 批次三第一刀
            // 重建资格守卫时顺序变了，于是这条断言必须两种都接受）。
            "group membership requires an active owner",
        ],
        "replace the active period identity",
    )
    .await;

    // 每 Room 至多一个 active owner：charlie 作为 owner 插入被拒。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO room_memberships \
             (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
             VALUES ('{}', '{}', '{}', 'owner', 2, 0, 1)",
            Uuid::new_v4(),
            group_id.as_str(),
            charlie.id.as_str(),
        ),
        &[
            "a group already has an active owner",
            "group membership requires an active owner",
            "UNIQUE constraint failed",
        ],
        "second active owner for the same group",
    )
    .await;

    // 事实不变：bob 的 active 周期仍在、owner 仍唯一。
    let (count_active,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM room_memberships WHERE room_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(count_active, 2, "owner + bob active, no extra rows");
    let (membership_id,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(membership_id, bob_active_membership_id);
}

/// 群邀请/转让幂等记录不可被 REPLACE 改写（同 key 重映射另一周期/
/// Room 被拒）；closed 周期与 active 周期的序列边界不可经 REPLACE 重建。
#[tokio::test]
async fn invite_and_transfer_idempotency_records_cannot_be_replaced() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    let charlie = add_human(&f.pool, "Charlie").await;
    accept_contact_with(&f, &alice, &bob, "k-ab").await;
    accept_contact_with(&f, &alice, &charlie, "k-ac").await;
    let group_id = create_group(&f, &alice, "群", "g-k").await;
    invite(&f, &alice, &group_id, &bob.id).await;
    invite(&f, &alice, &group_id, &charlie.id).await;

    // invite 幂等记录：(human_id, key) 主键不可改写/重映射。
    let (bob_membership,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let (charlie_membership,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(charlie.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let inv_key = format!("inv-{}", bob.id.as_str());
    // 同 key 重映射到 charlie 的周期。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO group_member_invite_idempotency \
             (human_id, key, room_id, participant_id, membership_id, created_at) \
             VALUES ('{}', '{inv_key}', '{}', '{}', '{charlie_membership}', 9)",
            alice.id.as_str(),
            group_id.as_str(),
            charlie.id.as_str(),
        ),
        &["group member invite idempotency identity already exists"],
        "invite idempotency remap",
    )
    .await;

    // 转让：完成后幂等记录不可改写；同 key 重放不重复执行。
    room_application::transfer_group_owner(
        &f.room,
        &alice,
        group_id.as_str(),
        bob.id.as_str(),
        "t-k",
    )
    .await
    .expect("transfer");
    let (transition_id,): (String,) = sqlx::query_as(
        "SELECT transition_id FROM group_owner_transfer_idempotency \
         WHERE human_id = ? AND key = 't-k'",
    )
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    // 同 key 重映射到另一 transition 被拒（identity guard）。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO group_owner_transfer_idempotency \
             (human_id, key, transition_id, created_at) \
             VALUES ('{}', 't-k', '{}', 9)",
            alice.id.as_str(),
            Uuid::new_v4(),
        ),
        &["group owner transfer idempotency identity already exists"],
        "transfer idempotency remap",
    )
    .await;

    // 记录保持原样。
    let (room, participant, membership): (String, String, String) = sqlx::query_as(
        "SELECT room_id, participant_id, membership_id FROM group_member_invite_idempotency \
         WHERE human_id = ? AND key = ?",
    )
    .bind(alice.id.as_str())
    .bind(inv_key.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(room, group_id.as_str());
    assert_eq!(participant, bob.id.as_str());
    assert_eq!(membership, bob_membership);
    let (t_room, t_target, t_transition): (String, String, String) = sqlx::query_as(
        "SELECT t.room_id, t.target_participant_id, t.transition_id \
         FROM group_owner_transfer_idempotency i \
         JOIN group_owner_transitions t ON t.transition_id = i.transition_id \
         WHERE i.human_id = ? AND i.key = 't-k'",
    )
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(t_room, group_id.as_str());
    assert_eq!(t_target, bob.id.as_str());
    assert_eq!(t_transition, transition_id);
}

/// P1 拆分防护：transfer 关闭必须与完整转让同事务。T1 单独以
/// closed_by='transfer' 关闭旧 owner 或目标 member（把转让拆成两笔，T2
/// 再做其余步骤）在写入时立即失败——当前事务中没有匹配 transition；
/// 失败后状态不变（回滚），合法完整转让仍可正常完成并保持单 owner 与
/// 正确的 coverage 见证。
#[tokio::test]
async fn closing_owner_or_target_without_transfer_is_rejected_at_write_time() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    accept_contact_with(&f, &alice, &bob, "k-ab").await;
    let group_id = create_group(&f, &alice, "群", "g-k").await;
    invite(&f, &alice, &group_id, &bob.id).await;
    let (owner_membership,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND role = 'owner' AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let (member_membership,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();

    // T1：单独以 closed_by='transfer' 关闭目标 member 周期（left_at=T、
    // left_before_sequence=X），不插 transition——写入时立即失败。
    for (label, membership) in [
        ("target member", member_membership.as_str()),
        ("old owner", owner_membership.as_str()),
    ] {
        let err = rejected(
            &f.pool,
            &format!(
                "UPDATE room_memberships \
                 SET active_marker = 0, left_at = 9, left_before_sequence = 0, closed_by = 'transfer' \
                 WHERE membership_id = '{membership}'"
            ),
        )
        .await;
        assert!(
            err.contains("a transfer close requires a same-transaction matching owner transition"),
            "{label} transfer close alone must be rejected at write time, got: {err}"
        );
    }

    // 失败不落库：owner 与目标 member 周期仍 active。
    for membership in [owner_membership.as_str(), member_membership.as_str()] {
        let (active,): (i64,) =
            sqlx::query_as("SELECT active_marker FROM room_memberships WHERE membership_id = ?")
                .bind(membership)
                .fetch_one(&f.pool)
                .await
                .unwrap();
        assert_eq!(active, 1, "rejected transfer close must not persist");
    }
    let transitions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM group_owner_transitions WHERE room_id = ?")
            .bind(group_id.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(transitions, 0, "no transition may be created");

    // 合法完整转让仍成功：恰一个 active owner，coverage 见证新 owner。
    room_application::transfer_group_owner(
        &f.room,
        &alice,
        group_id.as_str(),
        bob.id.as_str(),
        "t-k",
    )
    .await
    .expect("transfer after rejected split");
    let owners: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships \
         WHERE room_id = ? AND role = 'owner' AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(owners, 1, "exactly one active owner after transfer");
    let witness: String =
        sqlx::query_scalar("SELECT group_owner_membership_id FROM room_coverage WHERE room_id = ?")
            .bind(group_id.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    let (witness_active, witness_role): (i64, String) =
        sqlx::query_as("SELECT active_marker, role FROM room_memberships WHERE membership_id = ?")
            .bind(witness.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(witness_active, 1);
    assert_eq!(witness_role, "owner");
}

/// P1 拆分防护（transition INSERT guard 与行不变量）：
/// - transition 写入要求 Room 是 group，旧 owner 与目标 member 周期当时
///   active、同 room、participant/role 精确匹配（错 room / 错 participant /
///   错 role / 不存在周期都在写入时被拒）；
/// - 已关闭的旧 owner / 目标周期不能被任何 transition 消费；
/// - transfer 关闭的时间 / 边界必须与同事务 transition 完全一致（错配被
///   写入期守卫拒绝）；
/// - 完整合法直接 SQL 转让（先插 transition 再关闭）仍可 COMMIT；
/// - transition 行不可 UPDATE / DELETE / REPLACE。
#[tokio::test]
async fn owner_transition_insert_guard_requires_active_periods_and_row_is_immutable() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    let charlie = add_human(&f.pool, "Charlie").await;
    accept_contact_with(&f, &alice, &bob, "k-ab").await;
    accept_contact_with(&f, &alice, &charlie, "k-ac").await;
    let group_id = create_group(&f, &alice, "群", "g-k").await;
    invite(&f, &alice, &group_id, &bob.id).await;
    invite(&f, &alice, &group_id, &charlie.id).await;
    let agent_id = create_agent(&f, &alice, "甲", "a-k").await;
    let direct_id = create_direct_room(&f, &alice, agent_id.as_str(), "d-k").await;

    let (owner_membership,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND role = 'owner' AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let (bob_membership,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let (charlie_membership,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(charlie.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();

    // 1. 写入期错配：非 group Room / 错 participant / 错 role / 不存在的
    //    周期，全部被 INSERT guard 当场拒绝（不等 COMMIT）。
    let ghost_mid = Uuid::new_v4().to_string();
    for (label, room, old_owner_id, target_id, old_owner_mid, target_mid) in [
        (
            "non-group room",
            direct_id.as_str(),
            alice.id.as_str(),
            bob.id.as_str(),
            owner_membership.as_str(),
            bob_membership.as_str(),
        ),
        (
            "wrong old owner participant",
            group_id.as_str(),
            alice.id.as_str(),
            bob.id.as_str(),
            bob_membership.as_str(),
            bob_membership.as_str(),
        ),
        (
            "wrong old owner role (member period as owner)",
            group_id.as_str(),
            charlie.id.as_str(),
            bob.id.as_str(),
            charlie_membership.as_str(),
            bob_membership.as_str(),
        ),
        (
            "unknown old owner membership",
            group_id.as_str(),
            alice.id.as_str(),
            bob.id.as_str(),
            ghost_mid.as_str(),
            bob_membership.as_str(),
        ),
    ] {
        let err = rejected(
            &f.pool,
            &format!(
                "INSERT INTO group_owner_transitions \
                 (transition_id, room_id, old_owner_participant_id, target_participant_id, \
                  old_owner_membership_id, target_member_membership_id, new_owner_membership_id, \
                  old_owner_new_member_membership_id, occurred_at, boundary_sequence) \
                 VALUES ('{}', '{room}', '{old_owner_id}', '{target_id}', \
                         '{old_owner_mid}', '{target_mid}', \
                         '{}', '{}', 9, 0)",
                Uuid::new_v4(),
                Uuid::new_v4(),
                Uuid::new_v4(),
            ),
        )
        .await;
        assert!(
            err.contains("group owner transition requires an active old owner"),
            "{label} must be rejected by the transition insert guard, got: {err}"
        );
    }

    // 2. 完整合法直接 SQL 转让（先插 transition 再关闭）仍可 COMMIT。
    //    （migration 009 起事实时点必须被真实周期覆盖，转让时点取当前
    //    时间。）
    let t1_time = UtcMillis::now().millis() + 1;
    let t1_new_owner_mid = Uuid::new_v4().to_string();
    let t1_old_owner_member_mid = Uuid::new_v4().to_string();
    let t1_transition = Uuid::new_v4().to_string();
    let t1 = transfer_tx_sql(
        group_id.as_str(),
        alice.id.as_str(),
        bob.id.as_str(),
        &owner_membership,
        &bob_membership,
        &t1_new_owner_mid,
        &t1_old_owner_member_mid,
        &t1_transition,
        t1_time,
        0,
        "g-t1",
    );
    assert!(
        commit_tx(&f.pool, &t1).await.is_ok(),
        "legal direct-sql transfer (transition first) must commit"
    );

    // 3. 已关闭的旧 owner / 目标周期不能再被任何 transition 消费。
    for (label, consumed, col) in [
        (
            "closed old owner period",
            owner_membership.as_str(),
            "old_owner_membership_id",
        ),
        (
            "closed target member period",
            bob_membership.as_str(),
            "target_member_membership_id",
        ),
    ] {
        let fresh_a = Uuid::new_v4().to_string();
        let fresh_b = Uuid::new_v4().to_string();
        let (old_owner_mid, target_mid) = match col {
            "old_owner_membership_id" => (consumed, fresh_a.as_str()),
            _ => (fresh_a.as_str(), consumed),
        };
        let err = rejected(
            &f.pool,
            &format!(
                "INSERT INTO group_owner_transitions \
                 (transition_id, room_id, old_owner_participant_id, target_participant_id, \
                  old_owner_membership_id, target_member_membership_id, new_owner_membership_id, \
                  old_owner_new_member_membership_id, occurred_at, boundary_sequence) \
                 VALUES ('{}', '{group_id}', '{}', '{}', \
                         '{old_owner_mid}', '{target_mid}', \
                         '{fresh_b}', '{}', 9, 0)",
                Uuid::new_v4(),
                bob.id.as_str(),
                alice.id.as_str(),
                Uuid::new_v4(),
            ),
        )
        .await;
        assert!(
            err.contains("group owner transition requires an active old owner"),
            "{label} must be rejected by the transition insert guard, got: {err}"
        );
    }

    // 4. transfer 关闭的时间 / 边界必须与同事务 transition 完全一致：
    //    错配在写入时被 transfer 关闭守卫拒绝（bob → alice 方向，bob 是
    //    当前 owner、alice 是 active member）。
    for (label, old, new) in [
        (
            "wrong left_at",
            format!("left_at = {t1_time}"),
            format!("left_at = {}", t1_time + 1),
        ),
        (
            "wrong boundary",
            "left_before_sequence = 0".to_owned(),
            "left_before_sequence = 1".to_owned(),
        ),
    ] {
        let mut sql = transfer_tx_sql(
            group_id.as_str(),
            bob.id.as_str(),
            alice.id.as_str(),
            &t1_new_owner_mid,
            &t1_old_owner_member_mid,
            &Uuid::new_v4().to_string(),
            &Uuid::new_v4().to_string(),
            &Uuid::new_v4().to_string(),
            t1_time,
            0,
            &format!("g-{label}"),
        );
        sql[1] = sql[1].replace(&old, &new);
        sql[2] = sql[2].replace(&old, &new);
        let err = commit_tx(&f.pool, &sql).await.unwrap_err();
        assert!(
            err.contains("a transfer close requires a same-transaction matching owner transition"),
            "{label} close must be rejected by the transfer close guard, got: {err}"
        );
    }

    // 5. transition 行不可改写 / 不可删除 / 不可 REPLACE。
    let err = rejected(
        &f.pool,
        &format!(
            "UPDATE group_owner_transitions SET occurred_at = 10 WHERE transition_id = '{t1_transition}'"
        ),
    )
    .await;
    assert!(
        err.contains("group owner transitions are immutable"),
        "transition update must be rejected, got: {err}"
    );
    let err = rejected(
        &f.pool,
        &format!("DELETE FROM group_owner_transitions WHERE transition_id = '{t1_transition}'"),
    )
    .await;
    assert!(
        err.contains("group owner transitions cannot be deleted"),
        "transition delete must be rejected, got: {err}"
    );
    // REPLACE 以同 transition_id 重建：用仍 active 的当前 owner/目标周期
    // 组成合法形状，让写入期 INSERT guard 通过、由身份守卫拒绝（任何
    // INSERT 形式都不能删除并重建既有转让事实）。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO group_owner_transitions \
             (transition_id, room_id, old_owner_participant_id, target_participant_id, \
              old_owner_membership_id, target_member_membership_id, new_owner_membership_id, \
              old_owner_new_member_membership_id, occurred_at, boundary_sequence) \
             VALUES ('{t1_transition}', '{group_id}', '{}', '{}', \
                     '{t1_new_owner_mid}', '{t1_old_owner_member_mid}', \
                     '{}', '{}', 9, 0)",
            bob.id.as_str(),
            alice.id.as_str(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        ),
        &["group owner transition identity already exists"],
        "transition REPLACE",
    )
    .await;

    // 6. 事实保持：恰一条合法 transition，coverage 见证 bob 的 active
    //    owner 周期，alice 是 active member。
    let transitions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM group_owner_transitions WHERE room_id = ?")
            .bind(group_id.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(transitions, 1, "exactly one legal transition");
    let witness: String =
        sqlx::query_scalar("SELECT group_owner_membership_id FROM room_coverage WHERE room_id = ?")
            .bind(group_id.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(witness, t1_new_owner_mid);
    let (witness_active,): (i64,) =
        sqlx::query_as("SELECT active_marker FROM room_memberships WHERE membership_id = ?")
            .bind(witness.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(witness_active, 1);
}

/// 群 owner 周期关闭只能经合法 transfer：直接 SQL 把 owner 周期
/// closed_by 改为 leave / removed / agent_revoked 都在写入时被拒；普通
/// member 的 leave 仍合法（COMMIT 成功）；转让形状的替换（无 transition
/// 的 transfer 关闭、或同事务 transition + 关闭后把从未入群的 Human
/// 直接插成 owner）都在写入时被拒并整体回滚，群状态不变。
#[tokio::test]
async fn owner_close_requires_legal_transfer_and_member_leave_stays_legal() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    let charlie = add_human(&f.pool, "Charlie").await;
    accept_contact_with(&f, &alice, &bob, "k-ab").await;
    accept_contact_with(&f, &alice, &charlie, "k-ac").await;
    let group_id = create_group(&f, &alice, "群", "g-k").await;
    invite(&f, &alice, &group_id, &bob.id).await;

    let (owner_membership,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND role = 'owner' AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let (member_membership,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();

    // 1. 非法 owner 关闭：leave / removed / agent_revoked 全部被拒。
    for reason in ["leave", "removed", "agent_revoked"] {
        let err = rejected(
            &f.pool,
            &format!(
                "UPDATE room_memberships \
                 SET active_marker = 0, left_at = 9, left_before_sequence = 0, closed_by = '{reason}' \
                 WHERE membership_id = '{owner_membership}'"
            ),
        )
        .await;
        assert!(
            err.contains(
                "human membership can only close as a group member leave/removal or via owner transfer"
            ),
            "owner close with {reason} must be rejected, got: {err}"
        );
    }

    // 2. 普通 member 合法 leave：事务内直接 SQL 关闭 member 周期，
    //    COMMIT 成功（与 owner 的不对称证明）。（migration 009 起事实时点
    //    必须被周期覆盖：关闭时点取当前时间，同时产生该 Human 可见的
    //    member.left 事实。）
    let leave_time = UtcMillis::now().millis() + 1;
    let mut conn = f.pool.acquire().await.unwrap();
    sqlx::query("BEGIN").execute(&mut *conn).await.unwrap();
    sqlx::query(
        "UPDATE room_memberships \
         SET active_marker = 0, left_at = ?, left_before_sequence = 0, closed_by = 'leave' \
         WHERE membership_id = ?",
    )
    .bind(leave_time)
    .bind(member_membership.as_str())
    .execute(&mut *conn)
    .await
    .unwrap();
    let commit = sqlx::query("COMMIT").execute(&mut *conn).await;
    assert!(commit.is_ok(), "member leave must commit, got {commit:?}");
    drop(conn);

    // bob 用新 key 重入成为 active member（转让形状的 target 必须是
    // active member；旧的 closed 周期不能被 transition 消费）。
    room_application::invite_group_member(
        &f.room,
        &alice,
        group_id.as_str(),
        bob.id.as_str(),
        "inv-bob-2",
    )
    .await
    .expect("bob re-enters as member");
    let (member_membership,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();

    // 3a. 转让形状替换：transfer 关闭旧 owner 而不插 transition——写入时
    //    立即被 transfer 关闭守卫拒绝（拆分防护），不再等到 COMMIT。
    let err = rejected(
        &f.pool,
        &format!(
            "UPDATE room_memberships \
             SET active_marker = 0, left_at = 9, left_before_sequence = 0, closed_by = 'transfer' \
             WHERE membership_id = '{owner_membership}'"
        ),
    )
    .await;
    assert!(
        err.contains("a transfer close requires a same-transaction matching owner transition"),
        "transfer close without transition must be rejected at write time, got: {err}"
    );

    // 3b. 合法形状（同事务 transition + 关闭）但把从未入群的 charlie 直接
    //    插成 owner——owner 周期必须来自「该 Room 尚无任何 Membership」或
    //    「同一事务内精确匹配的 transition 事实」，写入时被拒（不看模糊
    //    历史）。
    let t3b_time = UtcMillis::now().millis() + 1;
    let charlie_owner = Uuid::new_v4().to_string();
    let old_owner_member_mid = Uuid::new_v4().to_string();
    let err = commit_tx(
        &f.pool,
        &[
            format!(
                "INSERT INTO group_owner_transitions \
                 (transition_id, room_id, old_owner_participant_id, target_participant_id, \
                  old_owner_membership_id, target_member_membership_id, new_owner_membership_id, \
                  old_owner_new_member_membership_id, occurred_at, boundary_sequence) \
                 VALUES ('{}', '{group_id}', '{}', '{}', \
                         '{owner_membership}', '{member_membership}', \
                         '{charlie_owner}', '{old_owner_member_mid}', {t3b_time}, 0)",
                Uuid::new_v4(),
                alice.id.as_str(),
                bob.id.as_str(),
            ),
            format!(
                "UPDATE room_memberships \
                 SET active_marker = 0, left_at = {t3b_time}, left_before_sequence = 0, closed_by = 'transfer' \
                 WHERE membership_id = '{owner_membership}'"
            ),
            format!(
                "UPDATE room_memberships \
                 SET active_marker = 0, left_at = {t3b_time}, left_before_sequence = 0, closed_by = 'transfer' \
                 WHERE membership_id = '{member_membership}'"
            ),
            format!(
                "INSERT INTO room_memberships \
                 (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, \
                  active_marker) \
                 VALUES ('{charlie_owner}', '{group_id}', '{}', 'owner', {t3b_time}, 0, 1)",
                charlie.id.as_str(),
            ),
        ],
    )
    .await
    .unwrap_err();
    assert!(
        err.contains(
            "group owner must be the founding owner or a same-transaction transfer target"
        ),
        "a never-joined human must not be inserted as group owner, got: {err}"
    );

    // 4. 回滚后群状态不变：owner 仍 active、coverage 仍见证 alice、
    //    charlie 没有任何周期。
    let (active,): (i64,) =
        sqlx::query_as("SELECT active_marker FROM room_memberships WHERE membership_id = ?")
            .bind(owner_membership.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(
        active, 1,
        "rolled-back transfer-shaped replacement must not persist"
    );
    let witness: String =
        sqlx::query_scalar("SELECT group_owner_membership_id FROM room_coverage WHERE room_id = ?")
            .bind(group_id.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(witness, owner_membership);
    let charlie_periods: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships WHERE room_id = ? AND participant_id = ?",
    )
    .bind(group_id.as_str())
    .bind(charlie.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        charlie_periods, 0,
        "rejected owner insert must leave no trace"
    );
}

/// 群邀请/转让幂等记录的 witness 必须指向同一条真实事实：invite witness
/// 以复合外键对应同一条真实 member 周期行（错误 participant/membership
/// 错配、owner 周期冒充 member witness 被拒）；transfer 幂等记录收敛为
/// (human_id, key) → transition_id，以复合外键证明 human_id 正是
/// transition.old_owner_participant_id（未知 transition、human 与旧 owner
/// 错配都被拒）；完整合法 transfer（直接 SQL，含 transition 事实）可
/// COMMIT；新 owner 再次转让后旧记录与旧 transition 保持原样、其见证
/// 周期关闭仍可历史重放。
#[tokio::test]
async fn invite_and_transfer_idempotency_witnesses_must_match_the_same_membership() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    let charlie = add_human(&f.pool, "Charlie").await;
    accept_contact_with(&f, &alice, &bob, "k-ab").await;
    accept_contact_with(&f, &alice, &charlie, "k-ac").await;
    let group_id = create_group(&f, &alice, "群", "g-k").await;
    invite(&f, &alice, &group_id, &bob.id).await;
    invite(&f, &alice, &group_id, &charlie.id).await;

    let (owner_membership,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND role = 'owner' AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let (bob_membership,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let (charlie_membership,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(charlie.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();

    // 1. invite witness 必须是同 room+participant 的真实 member 周期。
    for (label, sql) in [
        (
            "participant/membership mismatch",
            format!(
                "INSERT INTO group_member_invite_idempotency \
                 (human_id, key, room_id, participant_id, membership_id, created_at) \
                 VALUES ('{}', 'sql-inv-mismatch', '{}', '{}', '{charlie_membership}', 9)",
                alice.id.as_str(),
                group_id.as_str(),
                bob.id.as_str(),
            ),
        ),
        (
            "owner period as member witness",
            format!(
                "INSERT INTO group_member_invite_idempotency \
                 (human_id, key, room_id, participant_id, membership_id, created_at) \
                 VALUES ('{}', 'sql-inv-owner', '{}', '{}', '{owner_membership}', 9)",
                alice.id.as_str(),
                group_id.as_str(),
                bob.id.as_str(),
            ),
        ),
    ] {
        let err = rejected(&f.pool, &sql).await;
        assert!(
            err.contains(
                "invite idempotency must witness a same-row group member membership period"
            ),
            "{label} must be rejected, got: {err}"
        );
    }
    // 合法 invite witness（bob 的 member 周期）仍可写入。
    sqlx::query(
        "INSERT INTO group_member_invite_idempotency \
         (human_id, key, room_id, participant_id, membership_id, created_at) \
         VALUES (?, 'sql-inv-legal', ?, ?, ?, 9)",
    )
    .bind(alice.id.as_str())
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .bind(bob_membership.as_str())
    .execute(&f.pool)
    .await
    .unwrap();

    // 2. 先做一次合法直接 SQL 转让（t1），为幂等 FK witness 提供真实
    //    transition。（migration 009 起事实时点必须被真实周期覆盖，转让
    //    时点取当前时间。）
    let t1_time = UtcMillis::now().millis() + 1;
    let new_owner_mid = Uuid::new_v4().to_string();
    let old_owner_member_mid = Uuid::new_v4().to_string();
    let t1_transition = Uuid::new_v4().to_string();
    let t1 = transfer_tx_sql(
        group_id.as_str(),
        alice.id.as_str(),
        bob.id.as_str(),
        &owner_membership,
        &bob_membership,
        &new_owner_mid,
        &old_owner_member_mid,
        &t1_transition,
        t1_time,
        0,
        "sql-t-1",
    );
    let commit = commit_tx(&f.pool, &t1).await;
    assert!(
        commit.is_ok(),
        "legal direct-sql transfer with transition must commit, got {commit:?}"
    );

    // 3. transfer 幂等 witness：(human_id, transition_id) 必须对应同一条
    //    真实 transition 且 human_id 正是其 old_owner。
    for (label, sql) in [
        (
            "unknown transition",
            format!(
                "INSERT INTO group_owner_transfer_idempotency \
                 (human_id, key, transition_id, created_at) \
                 VALUES ('{}', 'sql-t-ghost', '{}', 9)",
                alice.id.as_str(),
                Uuid::new_v4(),
            ),
        ),
        (
            "human is not the transition old owner",
            format!(
                "INSERT INTO group_owner_transfer_idempotency \
                 (human_id, key, transition_id, created_at) \
                 VALUES ('{}', 'sql-t-wrong-human', '{t1_transition}', 9)",
                bob.id.as_str(),
            ),
        ),
    ] {
        let err = rejected(&f.pool, &sql).await;
        assert!(
            err.contains("FOREIGN KEY constraint failed"),
            "transfer idempotency {label} must be rejected by the deferred FK, got: {err}"
        );
    }
    // 合法 witness（human=旧 owner + 真实 transition）仍可写入。
    sqlx::query(
        "INSERT INTO group_owner_transfer_idempotency \
         (human_id, key, transition_id, created_at) VALUES (?, 'sql-t-1-dup', ?, 9)",
    )
    .bind(alice.id.as_str())
    .bind(&t1_transition)
    .execute(&f.pool)
    .await
    .unwrap();

    // 4. t1 的 transition 事实与四条周期精确关联。
    let (t1_room, t1_old, t1_target, t1_occurred, t1_boundary): (String, String, String, i64, i64) =
        sqlx::query_as(
            "SELECT room_id, old_owner_participant_id, target_participant_id, \
                occurred_at, boundary_sequence \
         FROM group_owner_transitions WHERE transition_id = ?",
        )
        .bind(&t1_transition)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(t1_room, group_id.as_str());
    assert_eq!(t1_old, alice.id.as_str());
    assert_eq!(t1_target, bob.id.as_str());
    assert_eq!(t1_occurred, t1_time);
    assert_eq!(t1_boundary, 0);
    let closed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships \
         WHERE room_id = ? AND left_at = ? AND left_before_sequence = 0 \
           AND closed_by = 'transfer' AND active_marker = 0",
    )
    .bind(group_id.as_str())
    .bind(t1_time)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        closed, 2,
        "old owner + target member closed with exact fields"
    );

    // 5. 新 owner 再次转让（bob → alice，直接 SQL 第二条 transition）：旧
    //    记录与旧 transition 保持原样，其见证周期（bob 的旧 owner 周期）
    //    关闭后历史 key 仍可合法重放。
    let t2_time = t1_time + 1;
    let alice_reowner_mid = Uuid::new_v4().to_string();
    let bob_member2_mid = Uuid::new_v4().to_string();
    let t2_transition = Uuid::new_v4().to_string();
    let t2 = transfer_tx_sql(
        group_id.as_str(),
        bob.id.as_str(),
        alice.id.as_str(),
        &new_owner_mid,
        &old_owner_member_mid,
        &alice_reowner_mid,
        &bob_member2_mid,
        &t2_transition,
        t2_time,
        0,
        "sql-t-2",
    );
    let commit = commit_tx(&f.pool, &t2).await;
    assert!(
        commit.is_ok(),
        "second transfer must commit, got {commit:?}"
    );

    let (t1_key, t1_transition_after): (String, String) = sqlx::query_as(
        "SELECT key, transition_id FROM group_owner_transfer_idempotency \
         WHERE human_id = ? AND key = 'sql-t-1'",
    )
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(t1_key, "sql-t-1");
    assert_eq!(
        t1_transition_after, t1_transition,
        "old transfer idempotency record must stay intact"
    );
    let (t1_new_owner_after,): (String,) = sqlx::query_as(
        "SELECT new_owner_membership_id FROM group_owner_transitions \
         WHERE transition_id = ?",
    )
    .bind(&t1_transition)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        t1_new_owner_after, new_owner_mid,
        "old transition stays intact"
    );
    let (witness_active,): (i64,) =
        sqlx::query_as("SELECT active_marker FROM room_memberships WHERE membership_id = ?")
            .bind(new_owner_mid.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(
        witness_active, 0,
        "old record witness period is closed but must remain replayable history"
    );
    // 旧 key（sql-t-1，alice 发起、目标 bob）在 t2 后仍可历史重放。
    let outcome = room_application::transfer_group_owner(
        &f.room,
        &alice,
        group_id.as_str(),
        bob.id.as_str(),
        "sql-t-1",
    )
    .await
    .expect("old key must replay");
    let room_application::TransferOwnerOutcome::Replayed {
        membership: replayed,
    } = outcome
    else {
        panic!("old key must replay the historical transfer");
    };
    assert_eq!(replayed.membership_id.as_str(), new_owner_mid);
    let owners: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships \
         WHERE room_id = ? AND role = 'owner' AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(owners, 1, "exactly one active owner after two transfers");
    let witness: String =
        sqlx::query_scalar("SELECT group_owner_membership_id FROM room_coverage WHERE room_id = ?")
            .bind(group_id.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(witness, alice_reowner_mid);
}

/// 群邀请/转让幂等记录的 room/participant/membership 关系不能经
/// UPDATE / DELETE / REPLACE 绕过：UPDATE 任一 witness 列、DELETE 整行、
/// REPLACE 同 key 重映射都被拒，原始记录保持原样。
#[tokio::test]
async fn invite_and_transfer_idempotency_rows_cannot_be_updated_deleted_or_replaced() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    let charlie = add_human(&f.pool, "Charlie").await;
    accept_contact_with(&f, &alice, &bob, "k-ab").await;
    accept_contact_with(&f, &alice, &charlie, "k-ac").await;
    let group_id = create_group(&f, &alice, "群", "g-k").await;
    invite(&f, &alice, &group_id, &bob.id).await;

    let (owner_membership,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND role = 'owner' AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let (bob_membership,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let inv_key = format!("inv-{}", bob.id.as_str());

    // 1. invite 记录：UPDATE 任一 witness 列 / DELETE 都被拒。
    for (label, sql, needle) in [
        (
            "update membership",
            format!(
                "UPDATE group_member_invite_idempotency SET membership_id = '{owner_membership}' \
                 WHERE human_id = '{}' AND key = '{inv_key}'",
                alice.id.as_str(),
            ),
            "group member invite idempotency records are immutable",
        ),
        (
            "update participant",
            format!(
                "UPDATE group_member_invite_idempotency SET participant_id = '{}' \
                 WHERE human_id = '{}' AND key = '{inv_key}'",
                charlie.id.as_str(),
                alice.id.as_str(),
            ),
            "group member invite idempotency records are immutable",
        ),
        (
            "update room",
            format!(
                "UPDATE group_member_invite_idempotency SET room_id = 'other-room' \
                 WHERE human_id = '{}' AND key = '{inv_key}'",
                alice.id.as_str(),
            ),
            "group member invite idempotency records are immutable",
        ),
        (
            "delete",
            format!(
                "DELETE FROM group_member_invite_idempotency \
                 WHERE human_id = '{}' AND key = '{inv_key}'",
                alice.id.as_str(),
            ),
            "group member invite idempotency records cannot be deleted",
        ),
    ] {
        let err = rejected(&f.pool, &sql).await;
        assert!(
            err.contains(needle),
            "invite record {label} must be rejected, got: {err}"
        );
    }
    // REPLACE 同 key 重映射另一周期同样被拒（identity guard）。
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO group_member_invite_idempotency \
             (human_id, key, room_id, participant_id, membership_id, created_at) \
             VALUES ('{}', '{inv_key}', '{}', '{}', '{owner_membership}', 9)",
            alice.id.as_str(),
            group_id.as_str(),
            bob.id.as_str(),
        ),
        &["group member invite idempotency identity already exists"],
        "invite idempotency remap",
    )
    .await;

    // 2. 转让记录（先完成一次合法转让）：UPDATE / DELETE / REPLACE 都被拒。
    room_application::transfer_group_owner(
        &f.room,
        &alice,
        group_id.as_str(),
        bob.id.as_str(),
        "t-k",
    )
    .await
    .expect("transfer");
    let (transition_id,): (String,) = sqlx::query_as(
        "SELECT transition_id FROM group_owner_transfer_idempotency \
         WHERE human_id = ? AND key = 't-k'",
    )
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    for (label, sql, needle) in [
        (
            "update transition",
            format!(
                "UPDATE group_owner_transfer_idempotency SET transition_id = '{}' \
                 WHERE human_id = '{}' AND key = 't-k'",
                Uuid::new_v4(),
                alice.id.as_str(),
            ),
            "group owner transfer idempotency records are immutable",
        ),
        (
            "update key",
            format!(
                "UPDATE group_owner_transfer_idempotency SET key = 't-other' \
                 WHERE human_id = '{}' AND key = 't-k'",
                alice.id.as_str(),
            ),
            "group owner transfer idempotency records are immutable",
        ),
        (
            "delete",
            format!(
                "DELETE FROM group_owner_transfer_idempotency \
                 WHERE human_id = '{}' AND key = 't-k'",
                alice.id.as_str(),
            ),
            "group owner transfer idempotency records cannot be deleted",
        ),
    ] {
        let err = rejected(&f.pool, &sql).await;
        assert!(
            err.contains(needle),
            "transfer record {label} must be rejected, got: {err}"
        );
    }
    assert_all_forms_rejected(
        &f.pool,
        &format!(
            "INSERT INTO group_owner_transfer_idempotency \
             (human_id, key, transition_id, created_at) \
             VALUES ('{}', 't-k', '{}', 9)",
            alice.id.as_str(),
            Uuid::new_v4(),
        ),
        &["group owner transfer idempotency identity already exists"],
        "transfer idempotency remap",
    )
    .await;

    // 3. 两条记录保持原样。
    let (room, participant, membership): (String, String, String) = sqlx::query_as(
        "SELECT room_id, participant_id, membership_id FROM group_member_invite_idempotency \
         WHERE human_id = ? AND key = ?",
    )
    .bind(alice.id.as_str())
    .bind(inv_key.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(room, group_id.as_str());
    assert_eq!(participant, bob.id.as_str());
    assert_eq!(membership, bob_membership);
    let (t_room, t_target, t_transition): (String, String, String) = sqlx::query_as(
        "SELECT t.room_id, t.target_participant_id, t.transition_id \
         FROM group_owner_transfer_idempotency i \
         JOIN group_owner_transitions t ON t.transition_id = i.transition_id \
         WHERE i.human_id = ? AND i.key = 't-k'",
    )
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(t_room, group_id.as_str());
    assert_eq!(t_target, bob.id.as_str());
    assert_eq!(t_transition, transition_id);
}

/// group_owner_transitions 必须精确见证四条同 Room Membership 周期：
/// 缺任一步、四条周期任一 room/participant/role/time/boundary/closed_by
/// 错配都无法完成（transition INSERT guard 在写入时要求旧 owner 与目标
/// member 当时 active；transfer 关闭守卫要求同事务匹配 transition；
/// 其余缺步/错配由 COMMIT 期 deferred 复合外键裁决）；完整合法直接 SQL
/// transition 可以 COMMIT；同一周期在同一角色列不能被第二次消费（已关闭
/// 周期由 INSERT guard 拒绝，仍 active 的周期由角色列 UNIQUE 拒绝；跨
/// 角色列合法）。
#[tokio::test]
async fn owner_transition_must_precisely_witness_four_periods() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    let charlie = add_human(&f.pool, "Charlie").await;
    accept_contact_with(&f, &alice, &bob, "k-ab").await;
    accept_contact_with(&f, &alice, &charlie, "k-ac").await;
    let group_id = create_group(&f, &alice, "群", "g-k").await;
    invite(&f, &alice, &group_id, &bob.id).await;
    invite(&f, &alice, &group_id, &charlie.id).await;

    let (owner_membership,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND role = 'owner' AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let (bob_membership,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let (charlie_membership,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(charlie.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();

    // 1. 完整合法直接 SQL transition 可以 COMMIT（alice → bob）。
    //    （migration 009 起事实时点必须被真实周期覆盖，转让时点取当前
    //    时间。）
    let w_time = UtcMillis::now().millis() + 1;
    let t1_new_owner_mid = Uuid::new_v4().to_string();
    let t1_old_owner_member_mid = Uuid::new_v4().to_string();
    let legal = transfer_tx_sql(
        group_id.as_str(),
        alice.id.as_str(),
        bob.id.as_str(),
        &owner_membership,
        &bob_membership,
        &t1_new_owner_mid,
        &t1_old_owner_member_mid,
        &Uuid::new_v4().to_string(),
        w_time,
        0,
        "w-legal",
    );
    assert!(
        commit_tx(&f.pool, &legal).await.is_ok(),
        "legal direct-sql transition must commit"
    );

    // 2. 缺步变体（bob → alice 方向，t1 之后 bob 是 owner、alice 是
    //    member）：删除 transition / 新 owner / 旧 owner member 任一步都
    //    无法完成（写入期守卫或 COMMIT 期 deferred 外键）。
    let t2_new_owner_mid = Uuid::new_v4().to_string();
    let t2_old_owner_member_mid = Uuid::new_v4().to_string();
    for name in [
        "missing-transition",
        "missing-new-owner",
        "missing-old-owner-member",
    ] {
        let mut sql = transfer_tx_sql(
            group_id.as_str(),
            bob.id.as_str(),
            alice.id.as_str(),
            &t1_new_owner_mid,
            &t1_old_owner_member_mid,
            &t2_new_owner_mid,
            &t2_old_owner_member_mid,
            &Uuid::new_v4().to_string(),
            9,
            0,
            &format!("w-{name}"),
        );
        match name {
            "missing-transition" => {
                sql.remove(0);
            }
            "missing-new-owner" => {
                sql.remove(3);
            }
            "missing-old-owner-member" => {
                sql.remove(4);
            }
            _ => unreachable!(),
        }
        let err = commit_tx(&f.pool, &sql).await;
        assert!(
            err.is_err(),
            "{name} must not commit (owner transition guard / deferred FK)"
        );
    }

    // 3. 用 API 完成 bob → alice 合法转让（w-back），再注入四条周期的
    //    role/time/boundary/closed_by/participant 错配变体（alice → bob
    //    方向）：每条都无法 COMMIT。
    room_application::transfer_group_owner(
        &f.room,
        &bob,
        group_id.as_str(),
        alice.id.as_str(),
        "w-back",
    )
    .await
    .expect("transfer back");
    let (owner_after,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND role = 'owner' AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let (target_after,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let new_owner_mid2 = Uuid::new_v4().to_string();
    let old_owner_member_mid2 = Uuid::new_v4().to_string();
    for name in [
        "new-owner-role-member",
        "new-owner-time",
        "new-owner-boundary",
        "target-closed-by-removed",
        "target-participant-charlie",
    ] {
        let mut sql = transfer_tx_sql(
            group_id.as_str(),
            alice.id.as_str(),
            bob.id.as_str(),
            &owner_after,
            &target_after,
            &new_owner_mid2,
            &old_owner_member_mid2,
            &Uuid::new_v4().to_string(),
            9,
            0,
            &format!("w2-{name}"),
        );
        match name {
            "new-owner-role-member" => {
                sql[3] = sql[3].replace("'owner',", "'member',");
            }
            "new-owner-time" => {
                sql[3] = sql[3].replace("'owner', 9, 0, 1)", "'owner', 10, 0, 1)");
            }
            "new-owner-boundary" => {
                sql[3] = sql[3].replace("'owner', 9, 0, 1)", "'owner', 9, 1, 1)");
            }
            "target-closed-by-removed" => {
                sql[2] = sql[2].replace("'transfer'", "'removed'");
            }
            "target-participant-charlie" => {
                sql[2] = sql[2].replace(
                    &format!("WHERE membership_id = '{target_after}'"),
                    &format!("WHERE membership_id = '{charlie_membership}'"),
                );
            }
            _ => unreachable!(),
        }
        let err = commit_tx(&f.pool, &sql).await;
        assert!(
            err.is_err(),
            "{name} must not commit (transition FK precisely witnesses the four periods)"
        );
    }

    // 4. 同一周期在同一角色列不能被第二次消费：t1 的 new_owner 周期（bob）
    //    已作为 w-back 的 old_owner、t1 的 old_owner_new_member 周期（alice）
    //    已作为 w-back 的 target_member——任何把已消费周期放进角色列的
    //    transition 写入都被拒：已关闭的旧 owner / 目标周期在写入时被
    //    INSERT guard 拒绝（不 active），仍 active 的周期被角色列 UNIQUE
    //    拒绝。
    // 4a. 已关闭的 old_owner 周期重复消费 → INSERT guard（不 active）。
    let err = rejected(
        &f.pool,
        &format!(
            "INSERT INTO group_owner_transitions \
             (transition_id, room_id, old_owner_participant_id, target_participant_id, \
              old_owner_membership_id, target_member_membership_id, new_owner_membership_id, \
              old_owner_new_member_membership_id, occurred_at, boundary_sequence) \
             VALUES ('{}', '{}', '{}', '{}', \
                     '{t1_new_owner_mid}', '{t1_old_owner_member_mid}', \
                     '{new_owner_mid2}', '{old_owner_member_mid2}', 9, 0)",
            Uuid::new_v4(),
            group_id.as_str(),
            bob.id.as_str(),
            alice.id.as_str(),
        ),
    )
    .await;
    assert!(
        err.contains("group owner transition requires an active old owner"),
        "closed old owner period must be rejected by the transition insert guard, got: {err}"
    );
    // 4b. 已关闭的 target member 周期重复消费 → INSERT guard（不 active）。
    let err = rejected(
        &f.pool,
        &format!(
            "INSERT INTO group_owner_transitions \
             (transition_id, room_id, old_owner_participant_id, target_participant_id, \
              old_owner_membership_id, target_member_membership_id, new_owner_membership_id, \
              old_owner_new_member_membership_id, occurred_at, boundary_sequence) \
             VALUES ('{}', '{}', '{}', '{}', \
                     '{owner_after}', '{t1_old_owner_member_mid}', \
                     '{new_owner_mid2}', '{old_owner_member_mid2}', 9, 0)",
            Uuid::new_v4(),
            group_id.as_str(),
            alice.id.as_str(),
            bob.id.as_str(),
        ),
    )
    .await;
    assert!(
        err.contains("group owner transition requires an active old owner"),
        "closed target member period must be rejected by the transition insert guard, got: {err}"
    );
    // 4c. 仍 active 的周期在同一角色列重复消费（t1 的 new_owner 周期作为
    //    下一次的 new_owner）→ 角色列 UNIQUE。
    let err = rejected(
        &f.pool,
        &format!(
            "INSERT INTO group_owner_transitions \
             (transition_id, room_id, old_owner_participant_id, target_participant_id, \
              old_owner_membership_id, target_member_membership_id, new_owner_membership_id, \
              old_owner_new_member_membership_id, occurred_at, boundary_sequence) \
             VALUES ('{}', '{}', '{}', '{}', \
                     '{owner_after}', '{target_after}', \
                     '{t1_new_owner_mid}', '{old_owner_member_mid2}', 9, 0)",
            Uuid::new_v4(),
            group_id.as_str(),
            alice.id.as_str(),
            bob.id.as_str(),
        ),
    )
    .await;
    assert!(
        err.contains("UNIQUE constraint failed: group_owner_transitions"),
        "re-consuming an active period in the same role column must be rejected by the unique, got: {err}"
    );
    // 跨角色列合法：t1 的 new_owner 周期（bob）作为下一次转让的 old_owner
    // 已在 w-back 中证明；恰有两条合法 transition。
    let transitions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM group_owner_transitions WHERE room_id = ?")
            .bind(group_id.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(
        transitions, 2,
        "exactly the two legal transitions exist (w-legal + w-back)"
    );
}

/// P1 回归：某 Human 历史上有 transfer 周期、后来 leave 无 active 周期，
/// 直接 SQL 关闭当前 owner（transfer，无同事务 transition）在写入时被拒
/// 且状态不变；其历史 closed 周期也不能在任何 transition 角色列被消费
/// （INSERT guard：周期必须当时 active）。
#[tokio::test]
async fn historical_transfer_cannot_reuse_periods_after_leave() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    accept_contact_with(&f, &alice, &bob, "k-ab").await;
    let group_id = create_group(&f, &alice, "群", "g-k").await;
    invite(&f, &alice, &group_id, &bob.id).await;

    // A → B 合法转让（t1 事实），然后 A 以 member 身份退群：A 没有任何
    // active 周期，但历史上存在 closed_by='transfer' 的周期（旧 owner
    // 周期）。
    room_application::transfer_group_owner(
        &f.room,
        &alice,
        group_id.as_str(),
        bob.id.as_str(),
        "p1-t1",
    )
    .await
    .expect("transfer");
    room_application::leave_membership(&f.room, &alice.id, group_id.as_str())
        .await
        .expect("alice leaves as member");
    let (alice_owner_history,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND closed_by = 'transfer'",
    )
    .bind(group_id.as_str())
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        alice_owner_history, 1,
        "alice has a historical transfer period"
    );
    let alice_active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(alice_active, 0, "alice has no active membership");

    // 直接 SQL 复用历史 transfer：
    // - 关闭 bob 的 owner 周期（closed_by=transfer，无同事务 transition）
    //   在写入时立即被 transfer 关闭守卫拒绝；
    // - 把 alice 的历史 closed 周期放进 transition 角色列被 INSERT guard
    //   拒绝（不 active）。
    let (bob_owner_mid,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND role = 'owner' AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let err = rejected(
        &f.pool,
        &format!(
            "UPDATE room_memberships \
             SET active_marker = 0, left_at = 9, left_before_sequence = 0, closed_by = 'transfer' \
             WHERE membership_id = '{bob_owner_mid}'"
        ),
    )
    .await;
    assert!(
        err.contains("a transfer close requires a same-transaction matching owner transition"),
        "transfer close without same-transaction transition must be rejected, got: {err}"
    );
    let (alice_old_owner_mid,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND role = 'owner' AND active_marker = 0",
    )
    .bind(group_id.as_str())
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let fresh_target = Uuid::new_v4().to_string();
    let err = rejected(
        &f.pool,
        &format!(
            "INSERT INTO group_owner_transitions \
             (transition_id, room_id, old_owner_participant_id, target_participant_id, \
              old_owner_membership_id, target_member_membership_id, new_owner_membership_id, \
              old_owner_new_member_membership_id, occurred_at, boundary_sequence) \
             VALUES ('{}', '{group_id}', '{}', '{}', \
                     '{alice_old_owner_mid}', '{fresh_target}', \
                     '{}', '{}', 9, 0)",
            Uuid::new_v4(),
            alice.id.as_str(),
            bob.id.as_str(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        ),
    )
    .await;
    assert!(
        err.contains("group owner transition requires an active old owner"),
        "a closed historical owner period must not be consumed by a transition, got: {err}"
    );

    // 状态不变：bob 仍是 active owner、coverage 仍见证 bob 的周期、
    // alice 没有任何 active 周期、没有新增 transition。
    let (bob_active,): (i64,) =
        sqlx::query_as("SELECT active_marker FROM room_memberships WHERE membership_id = ?")
            .bind(bob_owner_mid.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(bob_active, 1, "rolled-back reuse must not persist");
    let witness: String =
        sqlx::query_scalar("SELECT group_owner_membership_id FROM room_coverage WHERE room_id = ?")
            .bind(group_id.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(witness, bob_owner_mid);
    let alice_active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(alice_active, 0);
    let transitions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM group_owner_transitions WHERE room_id = ?")
            .bind(group_id.as_str())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(transitions, 1, "no new transition may be created");

    // 复用 alice 的历史周期放进新 transition 的角色列也被拒（同一周期
    // 在同一角色列只消费一次：alice 的旧 owner 周期已是 t1 的
    // old_owner_membership_id，其 member 周期已是 t1 的
    // old_owner_new_member_membership_id）。
    let (alice_old_owner_mid,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND role = 'owner' AND active_marker = 0",
    )
    .bind(group_id.as_str())
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let (alice_old_member_mid,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND role = 'member' AND active_marker = 0",
    )
    .bind(group_id.as_str())
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let (bob_old_member_mid,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND role = 'member' AND active_marker = 0",
    )
    .bind(group_id.as_str())
    .bind(bob.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    for (label, consumed, col) in [
        (
            "reuse old owner period as old owner",
            alice_old_owner_mid.as_str(),
            "old_owner_membership_id",
        ),
        (
            "reuse target member period as target member",
            bob_old_member_mid.as_str(),
            "target_member_membership_id",
        ),
        (
            "alice's old member period cannot serve as target member (participant mismatch)",
            alice_old_member_mid.as_str(),
            "target_member_membership_id",
        ),
    ] {
        let fresh_a = Uuid::new_v4().to_string();
        let fresh_b = Uuid::new_v4().to_string();
        let fresh_c = Uuid::new_v4().to_string();
        let (old_owner_mid, target_mid) = match col {
            "old_owner_membership_id" => (consumed, fresh_a.as_str()),
            "target_member_membership_id" => (fresh_a.as_str(), consumed),
            _ => unreachable!(),
        };
        let err = rejected(
            &f.pool,
            &format!(
                "INSERT INTO group_owner_transitions \
                 (transition_id, room_id, old_owner_participant_id, target_participant_id, \
                  old_owner_membership_id, target_member_membership_id, new_owner_membership_id, \
                  old_owner_new_member_membership_id, occurred_at, boundary_sequence) \
                 VALUES ('{}', '{}', '{}', '{}', \
                         '{old_owner_mid}', '{target_mid}', \
                         '{fresh_b}', '{fresh_c}', 9, 0)",
                Uuid::new_v4(),
                group_id.as_str(),
                bob.id.as_str(),
                alice.id.as_str(),
            ),
        )
        .await;
        assert!(
            err.contains("group owner transition requires an active old owner"),
            "{label} must be rejected by the transition insert guard (not active), got: {err}"
        );
    }
}

/// owner 邀请自己：在 active lookup 前显式识别，返回既有 MemberNotFound，
/// 不写幂等记录、不产生任何周期；普通 active member 用不同 key 仍 200。
#[tokio::test]
async fn owner_self_invite_returns_member_not_found_without_idempotency_record() {
    let f = fixture().await;
    let alice = bootstrap_human(&f.identity, "Alice")
        .await
        .unwrap()
        .participant;
    let bob = add_human(&f.pool, "Bob").await;
    accept_contact_with(&f, &alice, &bob, "k-ab").await;
    let group_id = create_group(&f, &alice, "群", "g-k").await;

    let err = room_application::invite_group_member(
        &f.room,
        &alice,
        group_id.as_str(),
        alice.id.as_str(),
        "k-self-1",
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, room_application::InviteMemberError::MemberNotFound),
        "owner self invite must be MemberNotFound, got {err}"
    );
    // 同 key 重试仍然稳定 404 语义，且不产生幂等记录。
    let err = room_application::invite_group_member(
        &f.room,
        &alice,
        group_id.as_str(),
        alice.id.as_str(),
        "k-self-1",
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, room_application::InviteMemberError::MemberNotFound),
        "owner self invite retry must stay MemberNotFound, got {err}"
    );
    let records: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM group_member_invite_idempotency WHERE human_id = ? AND key = 'k-self-1'",
    )
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        records, 0,
        "self invite must not write an idempotency record"
    );
    let (owner_periods,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1",
    )
    .bind(group_id.as_str())
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(owner_periods, 1, "owner keeps exactly one active period");

    // 普通 active member 用不同 key 仍 200（AlreadyActive）且记录该 key。
    invite(&f, &alice, &group_id, &bob.id).await;
    let outcome = room_application::invite_group_member(
        &f.room,
        &alice,
        group_id.as_str(),
        bob.id.as_str(),
        "k-bob-2",
    )
    .await
    .expect("second key for active member");
    assert!(
        matches!(
            outcome,
            room_application::InviteMemberOutcome::AlreadyActive { .. }
        ),
        "active member with a new key must be AlreadyActive"
    );
    let records: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM group_member_invite_idempotency WHERE human_id = ? AND key = 'k-bob-2'",
    )
    .bind(alice.id.as_str())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(records, 1, "active-member key is recorded");
}
