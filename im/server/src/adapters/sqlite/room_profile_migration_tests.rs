//! Group Room Profile migration 完整性测试。
//!
//! 在真实临时 SQLite 上应用全部 migration（含本切片 202610250001），用直接
//! SQL 证明本切片的 schema 守卫（CORE_V1_ROOM_PROFILE.md §6）：
//! 不可变、同 room 单调 +1、身份 REPLACE 防线、group-only + active Human
//! owner 写入资格、`length(CAST(... AS BLOB))` 2000 字节上限、NUL 拒绝、
//! trim 后非空、幂等记录的 deferred 外键（**只证明引用一致性，不证明同
//! 事务**）与见证一致性守卫（writer == actor、created_at == updated_at）。
//! 本阶段所有断言直接面向 migration 生成的 schema。

use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::test_relations::seed_accepted_relation;
use crate::ports::migration::MigrationProbe;
use sqlx::SqlitePool;

const OWNER_HUMAN: &str = "11111111-1111-4111-8111-111111111111";
const MEMBER_HUMAN: &str = "22222222-2222-4222-8222-222222222222";
const AGENT: &str = "33333333-3333-4333-8333-333333333333";
const GROUP_ROOM: &str = "44444444-4444-4444-8444-444444444444";
const SECOND_GROUP_ROOM: &str = "45454545-4545-4545-8545-454545454545";
const DIRECT_ROOM: &str = "55555555-5555-4555-8555-555555555555";
const OWNER_MEMBERSHIP: &str = "66666666-6666-4666-8666-666666666666";
const SECOND_OWNER_MEMBERSHIP: &str = "69696969-6969-4969-8969-696969696969";
const MEMBER_HUMAN_MEMBERSHIP: &str = "77777777-7777-4777-8777-777777777777";
const AGENT_MEMBERSHIP: &str = "88888888-8888-4888-8888-888888888888";
const DIRECT_HUMAN_MEMBERSHIP: &str = "89898989-8989-4989-8989-898989898989";
const DIRECT_AGENT_MEMBERSHIP: &str = "90909090-9090-4090-8090-909090909090";

async fn open() -> (tempfile::TempDir, MigrationStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("open + migrate on a fresh DB");
    (dir, store)
}

/// 闭合世界：active Human owner + active Human member（accepted 关系）+
/// active Agent（被 owner 拥有）+ 一间 group 房 + 一间 direct 房。
async fn seed_world(pool: &SqlitePool) {
    let mut tx = pool.begin().await.unwrap();
    for (id, kind, status, name) in [
        (OWNER_HUMAN, "human", "active", "Owner"),
        (MEMBER_HUMAN, "human", "active", "Member"),
        (AGENT, "agent", "active", "Agent"),
    ] {
        sqlx::query(
            "INSERT INTO participants (id, kind, status, display_name, created_at) \
             VALUES (?, ?, ?, ?, 1)",
        )
        .bind(id)
        .bind(kind)
        .bind(status)
        .bind(name)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO agent_owners (agent_id, human_id, status, created_at) \
         VALUES (?, ?, 'active', 1)",
    )
    .bind(AGENT)
    .bind(OWNER_HUMAN)
    .execute(&mut *tx)
    .await
    .unwrap();
    for (room, kind) in [
        (GROUP_ROOM, "group"),
        (SECOND_GROUP_ROOM, "group"),
        (DIRECT_ROOM, "direct"),
    ] {
        sqlx::query(
            "INSERT INTO rooms (id, kind, status, created_at, name) VALUES (?, ?, 'active', 1, NULL)",
        )
        .bind(room)
        .bind(kind)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    for (room, membership) in [
        (GROUP_ROOM, OWNER_MEMBERSHIP),
        (SECOND_GROUP_ROOM, SECOND_OWNER_MEMBERSHIP),
    ] {
        sqlx::query(
            "INSERT INTO room_coverage \
             (room_id, kind, created_at, group_owner_membership_id, group_owner_role, group_owner_active_marker) \
             VALUES (?, 'group', 1, ?, 'owner', 1)",
        )
        .bind(room)
        .bind(membership)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO room_memberships \
             (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
             VALUES (?, ?, ?, 'owner', 1, 0, 1)",
        )
        .bind(membership)
        .bind(room)
        .bind(OWNER_HUMAN)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    // direct 房同样要自证结构闭合（rooms 有一个 FK 指向 room_coverage）。
    sqlx::query(
        "INSERT INTO room_coverage (room_id, kind, created_at, direct_human_id, direct_agent_id) \
         VALUES (?, 'direct', 1, ?, ?)",
    )
    .bind(DIRECT_ROOM)
    .bind(OWNER_HUMAN)
    .bind(AGENT)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO direct_room_pairs \
         (room_id, human_id, agent_id, human_membership_id, agent_membership_id, created_at) \
         VALUES (?, ?, ?, ?, ?, 1)",
    )
    .bind(DIRECT_ROOM)
    .bind(OWNER_HUMAN)
    .bind(AGENT)
    .bind(DIRECT_HUMAN_MEMBERSHIP)
    .bind(DIRECT_AGENT_MEMBERSHIP)
    .execute(&mut *tx)
    .await
    .unwrap();
    for (membership, participant, role) in [
        (DIRECT_HUMAN_MEMBERSHIP, OWNER_HUMAN, "owner"),
        (DIRECT_AGENT_MEMBERSHIP, AGENT, "member"),
    ] {
        sqlx::query(
            "INSERT INTO room_memberships \
             (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
             VALUES (?, ?, ?, ?, 1, 0, 1)",
        )
        .bind(membership)
        .bind(DIRECT_ROOM)
        .bind(participant)
        .bind(role)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();

    // 关系与成员周期必须在不同语句里种：成员守卫要看 active accepted 关系段。
    seed_accepted_relation(pool, OWNER_HUMAN, MEMBER_HUMAN).await;
    for (membership, participant) in [
        (AGENT_MEMBERSHIP, AGENT),
        (MEMBER_HUMAN_MEMBERSHIP, MEMBER_HUMAN),
    ] {
        sqlx::query(
            "INSERT INTO room_memberships \
             (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
             VALUES (?, ?, ?, 'member', 2, 0, 1)",
        )
        .bind(membership)
        .bind(GROUP_ROOM)
        .bind(participant)
        .execute(pool)
        .await
        .unwrap();
    }
}

async fn insert_version(
    pool: &SqlitePool,
    room: &str,
    version: i64,
    description: &str,
    updated_by: &str,
    updated_at: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO room_profile_versions (room_id, version, description, updated_by, updated_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(room)
    .bind(version)
    .bind(description)
    .bind(updated_by)
    .bind(updated_at)
    .execute(pool)
    .await
    .map(|_| ())
}

async fn insert_idempotency(
    pool: &SqlitePool,
    actor: &str,
    room: &str,
    key: &str,
    digest: &str,
    result_version: i64,
    created_at: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO room_profile_put_idempotency \
         (actor_human_id, room_id, key, request_digest, result_version, created_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(actor)
    .bind(room)
    .bind(key)
    .bind(digest)
    .bind(result_version)
    .bind(created_at)
    .execute(pool)
    .await
    .map(|_| ())
}

fn digest(ch: char) -> String {
    std::iter::repeat_n(ch, 64).collect()
}

async fn tables_exist(pool: &SqlitePool, name: &str) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
        > 0
}

async fn object_exists(pool: &SqlitePool, kind: &str, name: &str) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sqlite_master WHERE type = ? AND name = ?")
        .bind(kind)
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
        > 0
}

#[tokio::test]
async fn migration_applies_and_creates_tables_and_guards() {
    let (_dir, store) = open().await;
    let status = store.status().await.expect("probe");
    assert_eq!(
        status,
        crate::ports::migration::MigrationStatus {
            pending: 0,
            failed: 0
        }
    );
    for table in ["room_profile_versions", "room_profile_put_idempotency"] {
        assert!(
            tables_exist(store.pool(), table).await,
            "missing table {table}"
        );
    }
    for trigger in [
        "room_profile_versions_immutable",
        "room_profile_versions_no_delete",
        "room_profile_versions_insert_identity_guard",
        "room_profile_versions_strictly_increment",
        "room_profile_versions_insert_guards",
        "room_profile_put_idempotency_actor_human",
        "room_profile_put_idempotency_immutable",
        "room_profile_put_idempotency_no_delete",
        "room_profile_put_idempotency_insert_identity_guard",
        "room_profile_put_idempotency_witness_consistency",
    ] {
        assert!(
            object_exists(store.pool(), "trigger", trigger).await,
            "missing trigger {trigger}"
        );
    }
}

#[tokio::test]
async fn versions_are_monotonic_and_immutable() {
    let (_dir, store) = open().await;
    let pool = store.pool();
    seed_world(pool).await;

    insert_version(pool, GROUP_ROOM, 1, "第一版", OWNER_HUMAN, 100)
        .await
        .expect("version 1");
    insert_version(pool, GROUP_ROOM, 2, "第二版\n多行 🙂", OWNER_HUMAN, 200)
        .await
        .expect("version 2");

    // 跳号 / 重复 / 回退一律拒绝。
    for (version, why) in [(4, "gap"), (2, "duplicate"), (1, "rewind")] {
        let err = insert_version(pool, GROUP_ROOM, version, "x", OWNER_HUMAN, 300)
            .await
            .expect_err(why);
        assert!(
            format!("{err}").contains("room profile version"),
            "{why}: {err}"
        );
    }

    // REPLACE 也不能改写既有版本身份。
    let err = sqlx::query(
        "INSERT OR REPLACE INTO room_profile_versions \
         (room_id, version, description, updated_by, updated_at) VALUES (?, 1, '篡改', ?, 400)",
    )
    .bind(GROUP_ROOM)
    .bind(OWNER_HUMAN)
    .execute(pool)
    .await
    .expect_err("REPLACE must be rejected");
    // REPLACE 的隐式删除发生在触发器之前：身份守卫或单调守卫任一先拒都算
    // （两条都在守同一件事——既有版本不能被重建）。
    assert!(format!("{err}").contains("room profile version"), "{err}");

    let err = sqlx::query("UPDATE room_profile_versions SET description = 'x' WHERE room_id = ?")
        .bind(GROUP_ROOM)
        .execute(pool)
        .await
        .expect_err("update must be rejected");
    assert!(format!("{err}").contains("immutable"), "{err}");

    let err = sqlx::query("DELETE FROM room_profile_versions WHERE room_id = ?")
        .bind(GROUP_ROOM)
        .execute(pool)
        .await
        .expect_err("delete must be rejected");
    assert!(format!("{err}").contains("cannot be deleted"), "{err}");
}

#[tokio::test]
async fn version_scope_is_per_room() {
    let (_dir, store) = open().await;
    let pool = store.pool();
    seed_world(pool).await;
    insert_version(pool, GROUP_ROOM, 1, "A 房", OWNER_HUMAN, 100)
        .await
        .expect("A room version 1");
    insert_version(pool, SECOND_GROUP_ROOM, 1, "B 房", OWNER_HUMAN, 100)
        .await
        .expect("B room version 1 是独立事实");
    insert_version(pool, GROUP_ROOM, 2, "A 房 v2", OWNER_HUMAN, 200)
        .await
        .expect("A room version 2");
    let b_latest: i64 =
        sqlx::query_scalar("SELECT MAX(version) FROM room_profile_versions WHERE room_id = ?")
            .bind(SECOND_GROUP_ROOM)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(b_latest, 1, "版本作用域不跨房");
}

#[tokio::test]
async fn description_enforces_bytes_nul_and_trim_non_empty() {
    let (_dir, store) = open().await;
    let pool = store.pool();
    seed_world(pool).await;

    for (description, why) in [
        (String::new(), "empty"),
        ("   ".to_string(), "spaces"),
        ("\n\t\r ".to_string(), "whitespace-only"),
        ("abc\u{0}def".to_string(), "nul"),
        ("a".repeat(2_001), "over 2000 bytes"),
    ] {
        let err = insert_version(pool, GROUP_ROOM, 1, &description, OWNER_HUMAN, 100)
            .await
            .expect_err(why);
        assert!(
            format!("{err}").to_lowercase().contains("check"),
            "{why}: {err}"
        );
    }

    let exact = "é".repeat(1_000);
    assert_eq!(exact.len(), 2_000);
    insert_version(pool, GROUP_ROOM, 1, &exact, OWNER_HUMAN, 100)
        .await
        .expect("exactly 2000 bytes");
    let text = "第一行\n第二行\r\n\t缩进 🙂";
    insert_version(pool, GROUP_ROOM, 2, text, OWNER_HUMAN, 200)
        .await
        .expect("multiline emoji");
    let stored: String = sqlx::query_scalar(
        "SELECT description FROM room_profile_versions WHERE room_id = ? AND version = 2",
    )
    .bind(GROUP_ROOM)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(stored, text, "原文保存，不 trim、不归一化");
}

#[tokio::test]
async fn only_group_rooms_with_active_human_owner_can_be_written() {
    let (_dir, store) = open().await;
    let pool = store.pool();
    seed_world(pool).await;

    let err = insert_version(pool, DIRECT_ROOM, 1, "私聊简介", OWNER_HUMAN, 100)
        .await
        .expect_err("direct room must be rejected");
    assert!(format!("{err}").contains("active group room"), "{err}");

    let err = insert_version(pool, GROUP_ROOM, 1, "成员写的", MEMBER_HUMAN, 100)
        .await
        .expect_err("active member but not owner");
    assert!(format!("{err}").contains("active human owner"), "{err}");

    let err = insert_version(pool, GROUP_ROOM, 1, "Agent 写的", AGENT, 100)
        .await
        .expect_err("agent cannot write");
    assert!(format!("{err}").contains("active human owner"), "{err}");

    let err = insert_version(pool, GROUP_ROOM, 1, "陌生人写的", "nobody", 100)
        .await
        .expect_err("unknown participant cannot write");
    assert!(format!("{err}").contains("active human owner"), "{err}");

    insert_version(pool, GROUP_ROOM, 1, "群主写的", OWNER_HUMAN, 100)
        .await
        .expect("active human owner can write");
}

#[tokio::test]
async fn idempotency_records_are_immutable_witnessed_and_keyed() {
    let (_dir, store) = open().await;
    let pool = store.pool();
    seed_world(pool).await;
    insert_version(pool, GROUP_ROOM, 1, "第一版", OWNER_HUMAN, 100)
        .await
        .expect("version 1");

    let d = digest('a');
    insert_idempotency(pool, OWNER_HUMAN, GROUP_ROOM, "k-1", &d, 1, 100)
        .await
        .expect("first idempotency record");

    // 同身份重复 / REPLACE 一律拒绝。
    let err = insert_idempotency(pool, OWNER_HUMAN, GROUP_ROOM, "k-1", &d, 1, 100)
        .await
        .expect_err("duplicate identity");
    assert!(
        format!("{err}").contains("identity already exists"),
        "{err}"
    );
    let err = sqlx::query(
        "INSERT OR REPLACE INTO room_profile_put_idempotency \
         (actor_human_id, room_id, key, request_digest, result_version, created_at) \
         VALUES (?, ?, 'k-1', ?, 1, 100)",
    )
    .bind(OWNER_HUMAN)
    .bind(GROUP_ROOM)
    .bind(digest('b'))
    .execute(pool)
    .await
    .expect_err("replace identity");
    assert!(
        format!("{err}").contains("identity already exists"),
        "{err}"
    );

    // 见证一致性：actor / created_at 必须与被引用版本行一致。
    let d2 = digest('b');
    let err = insert_idempotency(pool, OWNER_HUMAN, GROUP_ROOM, "k-2", &d2, 1, 999)
        .await
        .expect_err("wrong created_at");
    assert!(format!("{err}").contains("witness"), "{err}");

    // 非 active Human 的 actor 一律拒绝（这里是 Agent）。
    let err = insert_idempotency(pool, AGENT, GROUP_ROOM, "k-3", &d2, 1, 100)
        .await
        .expect_err("agent actor");
    // actor 守卫与见证守卫都会看到"它不是那条版本行的 active Human 写入者"；
    // 两条都拒即符合契约（这里不断言是哪一条先触发）。
    assert!(
        format!("{err}").contains("active human") || format!("{err}").contains("witness"),
        "{err}"
    );

    // 摘要 / key 形状。
    for (digest_value, key, why) in [
        (digest('A'), "k-4", "uppercase digest"),
        ("abc".to_string(), "k-5", "short digest"),
        (digest('c'), "bad key", "space in key"),
        (digest('c'), "a,b", "comma in key"),
    ] {
        let err = insert_idempotency(pool, OWNER_HUMAN, GROUP_ROOM, key, &digest_value, 1, 100)
            .await
            .expect_err(why);
        assert!(
            format!("{err}").to_lowercase().contains("check"),
            "{why}: {err}"
        );
    }

    let err = sqlx::query("UPDATE room_profile_put_idempotency SET created_at = 1")
        .execute(pool)
        .await
        .expect_err("update");
    assert!(format!("{err}").contains("immutable"), "{err}");
    let err = sqlx::query("DELETE FROM room_profile_put_idempotency")
        .execute(pool)
        .await
        .expect_err("delete");
    assert!(format!("{err}").contains("cannot be deleted"), "{err}");
}

#[tokio::test]
async fn idempotency_reference_is_deferred_and_checked_at_commit() {
    let (_dir, store) = open().await;
    let pool = store.pool();
    seed_world(pool).await;
    insert_version(pool, GROUP_ROOM, 1, "第一版", OWNER_HUMAN, 100)
        .await
        .expect("version 1");

    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO room_profile_put_idempotency \
         (actor_human_id, room_id, key, request_digest, result_version, created_at) \
         VALUES (?, ?, 'k-missing', ?, 2, 100)",
    )
    .bind(OWNER_HUMAN)
    .bind(GROUP_ROOM)
    .bind(digest('d'))
    .execute(&mut *tx)
    .await
    .expect("deferred FK lets the row in");
    let err = tx
        .commit()
        .await
        .expect_err("missing version must fail at commit");
    assert!(
        format!("{err}").to_lowercase().contains("foreign key"),
        "{err}"
    );
}
