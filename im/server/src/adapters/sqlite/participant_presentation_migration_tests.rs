//! Participant Presentation migration 完整性测试。
//!
//! 在真实临时 SQLite 上应用全部 migration（含本切片 202608200001），用直接
//! SQL 证明本切片的 schema 守卫（CORE_V1_PARTICIPANT_PRESENTATION.md §6）：
//! 不可变、单调版本、身份 REPLACE 防线、方向/资格触发器（含
//! `rooms.status='active'`）、`length(CAST(... AS BLOB))` 字节上限与 NUL 拒绝、
//! **空串在 SQL 层同样被拒**、幂等记录的 deferred 外键（**只证明引用一致性，
//! 不证明同事务**）与见证一致性守卫（publisher == actor、created_at ==
//! published_at、result_is_create == (version == 1)）。
//! 本阶段不实现 adapter，因此所有断言直接面向 migration 生成的 schema。

use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::ports::migration::MigrationProbe;
use sqlx::SqlitePool;

/// 与 migration 用例一致的最小 group 世界。
/// 四行成员周期以合法 v4 UUID 落库（见模块常量）。
struct World {
    human: &'static str,
    agent: &'static str,
    room: &'static str,
}

const OWNER_MEMBERSHIP: &str = "11111111-1111-4111-8111-111111111111";
const AGENT_MEMBERSHIP: &str = "22222222-2222-4222-8222-222222222222";

async fn open() -> (tempfile::TempDir, MigrationStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("open + migrate on a fresh DB");
    (dir, store)
}

/// 在一个 `BEGIN/COMMIT` 事务内建立合法闭合的 group 世界：
/// active Human owner H、active Agent A、active Owner 关系、
/// group Room + coverage + owner/member 两条 active 周期。
async fn seed_group_world(pool: &SqlitePool) -> World {
    let w = World {
        human: "human-owner-1",
        agent: "agent-member-1",
        room: "group-room-1",
    };
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO participants (id, kind, status, display_name, created_at) VALUES (?, 'human', 'active', 'Owner', 1)")
        .bind(w.human)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO participants (id, kind, status, display_name, created_at) VALUES (?, 'agent', 'active', 'Agent', 1)")
        .bind(w.agent)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO agent_owners (agent_id, human_id, status, created_at) VALUES (?, ?, 'active', 1)")
        .bind(w.agent)
        .bind(w.human)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO rooms (id, kind, status, created_at, name) VALUES (?, 'group', 'active', 1, 'g')")
        .bind(w.room)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO room_coverage (room_id, kind, created_at, group_owner_membership_id, group_owner_role, group_owner_active_marker) VALUES (?, 'group', 1, ?, 'owner', 1)")
        .bind(w.room)
        .bind(OWNER_MEMBERSHIP)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO room_memberships \
         (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
         VALUES (?, ?, ?, 'owner', 1, 0, 1)",
    )
    .bind(OWNER_MEMBERSHIP)
    .bind(w.room)
    .bind(w.human)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO room_memberships \
         (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
         VALUES (?, ?, ?, 'member', 1, 0, 1)",
    )
    .bind(AGENT_MEMBERSHIP)
    .bind(w.room)
    .bind(w.agent)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    w
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

#[tokio::test]
async fn migration_applies_on_fresh_db_and_creates_all_four_tables() {
    let (_dir, store) = open().await;
    let status = store.status().await.expect("probe");
    assert_eq!(
        status,
        crate::ports::migration::MigrationStatus {
            pending: 0,
            failed: 0
        }
    );
    for table in [
        "participant_profile_versions",
        "room_presentation_versions",
        "participant_profile_put_idempotency",
        "room_presentation_put_idempotency",
    ] {
        assert!(
            tables_exist(store.pool(), table).await,
            "missing table {table}"
        );
    }
}

#[tokio::test]
async fn profile_versions_insert_monotonically_with_an_owner_publisher() {
    let (_dir, store) = open().await;
    let w = seed_group_world(store.pool()).await;

    sqlx::query(
        "INSERT INTO participant_profile_versions \
         (participant_id, profile_version, short_description, public_description, published_by_id, published_at) \
         VALUES (?, 1, ?, NULL, ?, 10)",
    )
    .bind(w.agent)
    .bind("short v1")
    .bind(w.human)
    .execute(store.pool())
    .await
    .expect("version 1 written by the agent owner");
    sqlx::query(
        "INSERT INTO participant_profile_versions \
         (participant_id, profile_version, short_description, public_description, published_by_id, published_at) \
         VALUES (?, 2, NULL, 'public v2', ?, 20)",
    )
    .bind(w.agent)
    .bind(w.human)
    .execute(store.pool())
    .await
    .expect("version 2 must be exactly latest+1");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM participant_profile_versions")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(count, 2, "only two version rows may exist");
}

#[tokio::test]
async fn profile_versions_reject_jumps_gaps_duplicates_and_version_zero() {
    let (_dir, store) = open().await;
    let w = seed_group_world(store.pool()).await;

    // v1 合法。
    sqlx::query(
        "INSERT INTO participant_profile_versions (participant_id, profile_version, short_description, public_description, published_by_id, published_at) VALUES (?, 1, NULL, NULL, ?, 1)",
    )
    .bind(w.agent)
    .bind(w.human)
    .execute(store.pool())
    .await
    .unwrap();

    // 跳号：直接写 v3 拒绝（必须 = max+1 = 2）。
    let err = sqlx::query(
        "INSERT INTO participant_profile_versions (participant_id, profile_version, short_description, public_description, published_by_id, published_at) VALUES (?, 3, NULL, NULL, ?, 3)",
    )
    .bind(w.agent)
    .bind(w.human)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("exactly one more"), "{err}");

    // 版本 0 被拒绝（单调触发器或 CHECK 先裁决都成立，写法同 seed 后不推进）。
    let err = sqlx::query(
        "INSERT INTO participant_profile_versions (participant_id, profile_version, short_description, public_description, published_by_id, published_at) VALUES (?, 0, NULL, NULL, ?, 0)",
    )
    .bind(w.agent)
    .bind(w.human)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("CHECK") || err.to_string().contains("exactly one more"),
        "{err}"
    );

    // 重复 v1（普通重复与 REPLACE 都不可重建既有版本：identity guard 或单调
    // 触发器任一先裁决都证明 REPLACE 无法绕过不可变防线）。
    for stmt in [
        "INSERT INTO participant_profile_versions (participant_id, profile_version, short_description, public_description, published_by_id, published_at) VALUES (?, 1, 'other', NULL, ?, 1)",
        "INSERT OR REPLACE INTO participant_profile_versions (participant_id, profile_version, short_description, public_description, published_by_id, published_at) VALUES (?, 1, 'rewritten', NULL, ?, 1)",
    ] {
        let err = sqlx::query(stmt)
            .bind(w.agent)
            .bind(w.human)
            .execute(store.pool())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("identity already exists")
                || err.to_string().contains("exactly one more"),
            "{stmt}: {err}"
        );
    }
}

#[tokio::test]
async fn profile_versions_are_immutable_and_non_deletable() {
    let (_dir, store) = open().await;
    let w = seed_group_world(store.pool()).await;
    sqlx::query(
        "INSERT INTO participant_profile_versions (participant_id, profile_version, short_description, public_description, published_by_id, published_at) VALUES (?, 1, 'v1', NULL, ?, 1)",
    )
    .bind(w.agent)
    .bind(w.human)
    .execute(store.pool())
    .await
    .unwrap();

    let upd = sqlx::query(
        "UPDATE participant_profile_versions SET short_description = 'x' WHERE participant_id = ? AND profile_version = 1",
    )
    .bind(w.agent)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(upd.to_string().contains("immutable"), "{upd}");

    let del = sqlx::query("DELETE FROM participant_profile_versions WHERE participant_id = ?")
        .bind(w.agent)
        .execute(store.pool())
        .await
        .unwrap_err();
    assert!(del.to_string().contains("cannot be deleted"), "{del}");
}

#[tokio::test]
async fn profile_versions_reject_wrong_publisher_or_non_owned_target() {
    let (_dir, store) = open().await;
    let w = seed_group_world(store.pool()).await;
    // 无关系的第三方 Human。
    sqlx::query("INSERT INTO participants (id, kind, status, display_name, created_at) VALUES ('human-stranger', 'human', 'active', 'Stranger', 1)")
        .execute(store.pool())
        .await
        .unwrap();

    // 第三方 Human 非 Owner：拒绝。
    let err = sqlx::query(
        "INSERT INTO participant_profile_versions (participant_id, profile_version, short_description, public_description, published_by_id, published_at) VALUES (?, 1, NULL, NULL, 'human-stranger', 1)",
    )
    .bind(w.agent)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("owned by the publisher"), "{err}");

    // 目标是不存在/未知参与者：触发器与外键都拒绝。
    let err = sqlx::query(
        "INSERT INTO participant_profile_versions (participant_id, profile_version, short_description, public_description, published_by_id, published_at) VALUES ('ghost-agent', 1, NULL, NULL, ?, 1)",
    )
    .bind(w.human)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("owned by the publisher")
            || err.to_string().contains("FOREIGN KEY"),
        "{err}"
    );

    // 目标必须是 Agent：给 Human 写 Profile 拒绝（P0 只写 Agent，DB 兜底）。
    let err = sqlx::query(
        "INSERT INTO participant_profile_versions (participant_id, profile_version, short_description, public_description, published_by_id, published_at) VALUES ('human-stranger', 1, NULL, NULL, ?, 1)",
    )
    .bind(w.human)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("owned by the publisher"), "{err}");
}

#[tokio::test]
async fn profile_text_byte_limits_use_blob_length_and_reject_nul() {
    let (_dir, store) = open().await;
    let w = seed_group_world(store.pool()).await;

    // 512 ASCII 字节通过。
    let s512 = "a".repeat(512);
    sqlx::query(
        "INSERT INTO participant_profile_versions (participant_id, profile_version, short_description, public_description, published_by_id, published_at) VALUES (?, 1, ?, NULL, ?, 1)",
    )
    .bind(w.agent)
    .bind(&s512)
    .bind(w.human)
    .execute(store.pool())
    .await
    .expect("512-byte short_description must pass");

    // 513 ASCII 字节拒绝（版本写 2，但失败不推进；随后继续用 2 即可）。
    let s513 = "a".repeat(513);
    let err = sqlx::query(
        "INSERT INTO participant_profile_versions (participant_id, profile_version, short_description, public_description, published_by_id, published_at) VALUES (?, 2, ?, NULL, ?, 2)",
    )
    .bind(w.agent)
    .bind(&s513)
    .bind(w.human)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("CHECK"), "{err}");
    assert!(
        tables_exist(store.pool(), "participant_profile_versions").await,
        "sanity: table still readable after rejected insert"
    );

    // 171 个 3 字节 CJK 字符 = 513 字节：字符数 171 <= 512（字符数不超）但
    // 字节数超限，必须拒绝——证明上限按 BLOB 字节数而非 length(text) 字符数
    // 裁决（若用 length(text)，171 <= 512 会误放行）。
    let cjk = "界".repeat(171);
    let err = sqlx::query(
        "INSERT INTO participant_profile_versions (participant_id, profile_version, short_description, public_description, published_by_id, published_at) VALUES (?, 2, ?, NULL, ?, 3)",
    )
    .bind(w.agent)
    .bind(&cjk)
    .bind(w.human)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("CHECK"), "{err}");

    // NUL 拒绝。
    let err = sqlx::query(
        "INSERT INTO participant_profile_versions (participant_id, profile_version, short_description, public_description, published_by_id, published_at) VALUES (?, 2, ?, NULL, ?, 4)",
    )
    .bind(w.agent)
    .bind("has\u{0}nul")
    .bind(w.human)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("CHECK"), "{err}");

    // 空串（应用归一化为 NULL 前的原始值）在 SQL 层必须拒绝：不能成为与 NULL
    // 并列的第二个持久事实。之前的失败插入都不推进版本，因此下一合法版本号
    // 仍然是 2，但想直写 '' 的尝试必须被 CHECK 拦下。
    let err = sqlx::query(
        "INSERT INTO participant_profile_versions (participant_id, profile_version, short_description, public_description, published_by_id, published_at) VALUES (?, 2, '', NULL, ?, 5)",
    )
    .bind(w.agent)
    .bind(w.human)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("CHECK"),
        "raw empty string must be rejected by SQL ({err})"
    );

    // 由空白组成的非空字符串仍合法且不 trim——版本 2 以 '  ' 原样写入。
    sqlx::query(
        "INSERT INTO participant_profile_versions (participant_id, profile_version, short_description, public_description, published_by_id, published_at) VALUES (?, 2, '  ', NULL, ?, 6)",
    )
    .bind(w.agent)
    .bind(w.human)
    .execute(store.pool())
    .await
    .expect("whitespace-only non-empty text must remain valid (no trim)");
}

#[tokio::test]
async fn profile_idempotency_records_witness_real_versions_and_resist_replay() {
    let (_dir, store) = open().await;
    let w = seed_group_world(store.pool()).await;

    // 版本行 + 幂等记录同一事务（deferred FK 见证 result_profile_version）。
    {
        let mut tx = store.pool().begin().await.unwrap();
        sqlx::query(
            "INSERT INTO participant_profile_versions (participant_id, profile_version, short_description, public_description, published_by_id, published_at) VALUES (?, 1, NULL, NULL, ?, 1)",
        )
        .bind(w.agent)
        .bind(w.human)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO participant_profile_put_idempotency (actor_human_id, participant_id, key, request_digest, result_profile_version, result_is_create, created_at) VALUES (?, ?, ?, ?, 1, 1, 1)",
        )
        .bind(w.human)
        .bind(w.agent)
        .bind("key-1")
        .bind("a".repeat(64))
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    // 同 key 重放（普通重复与 REPLACE）被 identity guard 拒绝。
    for stmt in [
        "INSERT INTO participant_profile_put_idempotency (actor_human_id, participant_id, key, request_digest, result_profile_version, result_is_create, created_at) VALUES (?, ?, ?, ?, 1, 1, 1)",
        "INSERT OR REPLACE INTO participant_profile_put_idempotency (actor_human_id, participant_id, key, request_digest, result_profile_version, result_is_create, created_at) VALUES (?, ?, ?, ?, 1, 1, 1)",
    ] {
        let err = sqlx::query(stmt)
            .bind(w.human)
            .bind(w.agent)
            .bind("key-1")
            .bind("a".repeat(64))
            .execute(store.pool())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("identity already exists"),
            "{stmt}: {err}"
        );
    }

    // 幂等记录引用不存在的版本（deferred FK 无见证）→ COMMIT 期失败。
    // result_is_create=0 与 v9(≠1) 一致，因此确实是外键失败而非 identity CHECK。
    let err = sqlx::query(
        "INSERT INTO participant_profile_put_idempotency (actor_human_id, participant_id, key, request_digest, result_profile_version, result_is_create, created_at) VALUES (?, ?, 'key-2', ?, 9, 0, 2)",
    )
    .bind(w.human)
    .bind(w.agent)
    .bind("b".repeat(64))
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("FOREIGN KEY") || err.to_string().contains("constraint"),
        "{err}"
    );

    // 幂等表不携带正文/描述/预览——schema 里根本没有这些列。
    let profile_cols: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM pragma_table_info('participant_profile_put_idempotency')",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    for forbidden in ["short_description", "public_description", "preview"] {
        assert!(
            !profile_cols.iter().any(|c| c == forbidden),
            "{forbidden} must not be stored"
        );
    }
}

#[tokio::test]
async fn profile_idempotency_witness_guards_require_consistent_publisher_and_time() {
    let (_dir, store) = open().await;
    let w = seed_group_world(store.pool()).await;

    // 先插入版本行 v1（发布者 human-owner-1，published_at=10），提交。
    sqlx::query(
        "INSERT INTO participant_profile_versions (participant_id, profile_version, short_description, public_description, published_by_id, published_at) VALUES (?, 1, NULL, NULL, ?, 10)",
    )
    .bind(w.agent)
    .bind(w.human)
    .execute(store.pool())
    .await
    .expect("version 1 must insert");

    // 第二个 active Human（不是发布者）。
    sqlx::query("INSERT INTO participants (id, kind, status, display_name, created_at) VALUES ('human-other', 'human', 'active', 'Other', 1)")
        .execute(store.pool())
        .await
        .unwrap();

    // actor 不是被引用版本的发布者 → 见证触发器拒绝。
    let err = sqlx::query(
        "INSERT INTO participant_profile_put_idempotency (actor_human_id, participant_id, key, request_digest, result_profile_version, result_is_create, created_at) VALUES ('human-other', ?, 'key-pub', ?, 1, 1, 10)",
    )
    .bind(w.agent)
    .bind("a".repeat(64))
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("witness must match"),
        "publisher mismatch must be rejected ({err})"
    );

    // created_at != 被引用版本的 published_at(10) → 见证触发器拒绝。
    let err = sqlx::query(
        "INSERT INTO participant_profile_put_idempotency (actor_human_id, participant_id, key, request_digest, result_profile_version, result_is_create, created_at) VALUES (?, ?, 'key-time', ?, 1, 1, 11)",
    )
    .bind(w.human)
    .bind(w.agent)
    .bind("b".repeat(64))
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("witness must match"), "{err}");

    // result_is_create 与版本不符（version 1 必须 is_create=1）→ CHECK 拒绝。
    let err = sqlx::query(
        "INSERT INTO participant_profile_put_idempotency (actor_human_id, participant_id, key, request_digest, result_profile_version, result_is_create, created_at) VALUES (?, ?, 'key-ic', ?, 1, 0, 10)",
    )
    .bind(w.human)
    .bind(w.agent)
    .bind("c".repeat(64))
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("CHECK"), "{err}");

    // 一致的记录（actor=发布者、created_at=published_at、is_create 匹配）通过。
    sqlx::query(
        "INSERT INTO participant_profile_put_idempotency (actor_human_id, participant_id, key, request_digest, result_profile_version, result_is_create, created_at) VALUES (?, ?, 'key-ok', ?, 1, 1, 10)",
    )
    .bind(w.human)
    .bind(w.agent)
    .bind("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789")
    .execute(store.pool())
    .await
    .expect("consistent witness must pass");
}

#[tokio::test]
async fn idempotency_deferred_fk_is_referential_not_same_transaction() {
    let (_dir, store) = open().await;
    let w = seed_group_world(store.pool()).await;

    // 事务 A：只插入版本行 v1（发布者 human，published_at=5）并提交。
    {
        let mut tx = store.pool().begin().await.unwrap();
        sqlx::query(
            "INSERT INTO participant_profile_versions (participant_id, profile_version, short_description, public_description, published_by_id, published_at) VALUES (?, 1, NULL, NULL, ?, 5)",
        )
        .bind(w.agent)
        .bind(w.human)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    // 独立事务 B：幂等记录引用已提交的 v1——引用/身份一致即可成功。
    // 证明 deferred 外键只是引用一致性守卫，**不是**同事务原子性证明
    //（同事务原子性由 adapter 的单个 `BEGIN IMMEDIATE` 保证，见 §6）。
    sqlx::query(
        "INSERT INTO participant_profile_put_idempotency (actor_human_id, participant_id, key, request_digest, result_profile_version, result_is_create, created_at) VALUES (?, ?, 'key-x', ?, 1, 1, 5)",
    )
    .bind(w.human)
    .bind(w.agent)
    .bind("d".repeat(64))
    .execute(store.pool())
    .await
    .expect("deferred FK allows referencing an already-committed version row (referential consistency only)");
}

#[tokio::test]
async fn room_presentation_requires_group_owner_publisher_and_active_agent_member() {
    let (_dir, store) = open().await;
    let w = seed_group_world(store.pool()).await;
    sqlx::query("INSERT INTO participants (id, kind, status, display_name, created_at) VALUES ('human-stranger', 'human', 'active', 'Stranger', 1)")
        .execute(store.pool())
        .await
        .unwrap();

    // 合法：active group owner 为 active agent member 写 v1。
    sqlx::query(
        "INSERT INTO room_presentation_versions (room_id, participant_id, presentation_version, room_display_name, room_role_name, room_role_description, published_by_id, published_at) VALUES (?, ?, 1, 'Display', 'Role', NULL, ?, 1)",
    )
    .bind(w.room)
    .bind(w.agent)
    .bind(w.human)
    .execute(store.pool())
    .await
    .expect("active group owner writing for an active agent member must pass");

    // 非 owner 的 Human（第三者）拒绝：不是该 group Room 的 active owner。
    let err = sqlx::query(
        "INSERT INTO room_presentation_versions (room_id, participant_id, presentation_version, room_display_name, room_role_name, room_role_description, published_by_id, published_at) VALUES (?, ?, 2, NULL, NULL, NULL, 'human-stranger', 2)",
    )
    .bind(w.room)
    .bind(w.agent)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("active group owner or the participant itself"),
        "{err}"
    );

    // owner 身份虽在，但目标不是 active agent member（用第三方 Human 当目标）拒绝。
    let err = sqlx::query(
        "INSERT INTO room_presentation_versions (room_id, participant_id, presentation_version, room_display_name, room_role_name, room_role_description, published_by_id, published_at) VALUES (?, 'human-stranger', 2, NULL, NULL, NULL, ?, 3)",
    )
    .bind(w.room)
    .bind(w.human)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("active agent member"), "{err}");

    // 不存在的 Room：group-kind 守卫（触发器先于外键执行）拒绝。
    let err = sqlx::query(
        "INSERT INTO room_presentation_versions (room_id, participant_id, presentation_version, room_display_name, room_role_name, room_role_description, published_by_id, published_at) VALUES ('no-such-room', ?, 2, NULL, NULL, NULL, ?, 4)",
    )
    .bind(w.agent)
    .bind(w.human)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("requires a group room")
            || err.to_string().contains("FOREIGN KEY"),
        "{err}"
    );
}

#[tokio::test]
async fn room_presentation_monotonic_immutable_and_no_replace() {
    let (_dir, store) = open().await;
    let w = seed_group_world(store.pool()).await;

    // v1 -> v2 单调。
    sqlx::query(
        "INSERT INTO room_presentation_versions (room_id, participant_id, presentation_version, room_display_name, room_role_name, room_role_description, published_by_id, published_at) VALUES (?, ?, 1, 'n1', NULL, NULL, ?, 1)",
    )
    .bind(w.room)
    .bind(w.agent)
    .bind(w.human)
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO room_presentation_versions (room_id, participant_id, presentation_version, room_display_name, room_role_name, room_role_description, published_by_id, published_at) VALUES (?, ?, 2, 'n2', NULL, NULL, ?, 2)",
    )
    .bind(w.room)
    .bind(w.agent)
    .bind(w.human)
    .execute(store.pool())
    .await
    .unwrap();

    // 跳号 v4 拒绝。
    let err = sqlx::query(
        "INSERT INTO room_presentation_versions (room_id, participant_id, presentation_version, room_display_name, room_role_name, room_role_description, published_by_id, published_at) VALUES (?, ?, 4, NULL, NULL, NULL, ?, 4)",
    )
    .bind(w.room)
    .bind(w.agent)
    .bind(w.human)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("exactly one more"), "{err}");

    // REPLACE 不能重建 v1（identity guard 或单调触发器任一先裁决都证明 REPLACE
    // 无法绕过不可变防线改写/重建既有版本）。
    let err = sqlx::query(
        "INSERT OR REPLACE INTO room_presentation_versions (room_id, participant_id, presentation_version, room_display_name, room_role_name, room_role_description, published_by_id, published_at) VALUES (?, ?, 1, 'rewritten', NULL, NULL, ?, 9)",
    )
    .bind(w.room)
    .bind(w.agent)
    .bind(w.human)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("identity already exists")
            || err.to_string().contains("exactly one more"),
        "{err}"
    );

    // 不可变 + 不可删。
    let upd = sqlx::query("UPDATE room_presentation_versions SET room_display_name = 'x' WHERE room_id = ? AND participant_id = ? AND presentation_version = 1")
        .bind(w.room)
        .bind(w.agent)
        .execute(store.pool())
        .await
        .unwrap_err();
    assert!(upd.to_string().contains("immutable"), "{upd}");
    let del = sqlx::query(
        "DELETE FROM room_presentation_versions WHERE room_id = ? AND participant_id = ?",
    )
    .bind(w.room)
    .bind(w.agent)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(del.to_string().contains("cannot be deleted"), "{del}");
}

#[tokio::test]
async fn room_presentation_idempotency_witnesses_version_and_rejects_bad_digest_shape() {
    let (_dir, store) = open().await;
    let w = seed_group_world(store.pool()).await;

    // 版本行 + 幂等记录同一事务。
    {
        let mut tx = store.pool().begin().await.unwrap();
        sqlx::query(
            "INSERT INTO room_presentation_versions (room_id, participant_id, presentation_version, room_display_name, room_role_name, room_role_description, published_by_id, published_at) VALUES (?, ?, 1, 'n', NULL, NULL, ?, 1)",
        )
        .bind(w.room)
        .bind(w.agent)
        .bind(w.human)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO room_presentation_put_idempotency (actor_human_id, room_id, participant_id, key, request_digest, result_presentation_version, result_is_create, created_at) VALUES (?, ?, ?, 'rkey-1', ?, 1, 1, 1)",
        )
        .bind(w.human)
        .bind(w.room)
        .bind(w.agent)
        .bind("c".repeat(64))
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    // 同 key 重放被 identity guard 拒绝（REPLACE 变体使用与首次记录一致的
    // 见证字段，因此失败源是身份守卫而非见证守卫）。
    let err = sqlx::query(
        "INSERT OR REPLACE INTO room_presentation_put_idempotency (actor_human_id, room_id, participant_id, key, request_digest, result_presentation_version, result_is_create, created_at) VALUES (?, ?, ?, 'rkey-1', ?, 1, 1, 1)",
    )
    .bind(w.human)
    .bind(w.room)
    .bind(w.agent)
    .bind("c".repeat(64))
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("identity already exists"), "{err}");

    // digest 必须 64 位：非 64 长度的请求摘要被 CHECK 拒绝（created_at 与 v1
    // 的 published_at 一致，避免见证守卫先于 CHECK 触发）。
    let err = sqlx::query(
        "INSERT INTO room_presentation_put_idempotency (actor_human_id, room_id, participant_id, key, request_digest, result_presentation_version, result_is_create, created_at) VALUES (?, ?, ?, 'rkey-2', 'short', 1, 1, 1)",
    )
    .bind(w.human)
    .bind(w.room)
    .bind(w.agent)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("CHECK"), "{err}");

    // 结果版本引用不存在的版本行：COMMIT 期 deferred FK 拒绝。
    // result_is_create=0 与 v99(≠1) 一致，因此确实是外键失败而非 identity CHECK。
    let err = sqlx::query(
        "INSERT INTO room_presentation_put_idempotency (actor_human_id, room_id, participant_id, key, request_digest, result_presentation_version, result_is_create, created_at) VALUES (?, ?, ?, 'rkey-3', ?, 99, 0, 3)",
    )
    .bind(w.human)
    .bind(w.room)
    .bind(w.agent)
    .bind("d".repeat(64))
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("FOREIGN KEY") || err.to_string().contains("constraint"),
        "{err}"
    );
}

#[tokio::test]
async fn room_presentation_text_empty_string_rejected_but_whitespace_ok() {
    let (_dir, store) = open().await;
    let w = seed_group_world(store.pool()).await;

    // v1：三个文本列都用纯空白非空串（'  '）——合法、不 trim 原样保存。
    sqlx::query(
        "INSERT INTO room_presentation_versions (room_id, participant_id, presentation_version, room_display_name, room_role_name, room_role_description, published_by_id, published_at) VALUES (?, ?, 1, '  ', '  ', '  ', ?, 1)",
    )
    .bind(w.room)
    .bind(w.agent)
    .bind(w.human)
    .execute(store.pool())
    .await
    .expect("whitespace-only non-empty text must remain valid (no trim)");

    // 失败插入不推进版本，后续尝试都用版本 2。三列各自直写 '' 都须被 CHECK 拒。
    let insert_v2 = "INSERT INTO room_presentation_versions \
         (room_id, participant_id, presentation_version, room_display_name, room_role_name, room_role_description, published_by_id, published_at) \
         VALUES (?, ?, 2, ?, ?, ?, ?, 2)";
    let none: Option<&str> = None;
    // room_display_name=''，其余 NULL。
    let err = sqlx::query(insert_v2)
        .bind(w.room)
        .bind(w.agent)
        .bind("")
        .bind(none)
        .bind(none)
        .bind(w.human)
        .execute(store.pool())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("CHECK"),
        "room_display_name raw empty string must be rejected ({err})"
    );
    // room_role_name=''，其余 NULL。
    let err = sqlx::query(insert_v2)
        .bind(w.room)
        .bind(w.agent)
        .bind(none)
        .bind("")
        .bind(none)
        .bind(w.human)
        .execute(store.pool())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("CHECK"),
        "room_role_name raw empty string must be rejected ({err})"
    );
    // room_role_description=''，其余 NULL。
    let err = sqlx::query(insert_v2)
        .bind(w.room)
        .bind(w.agent)
        .bind(none)
        .bind(none)
        .bind("")
        .bind(w.human)
        .execute(store.pool())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("CHECK"),
        "room_role_description raw empty string must be rejected ({err})"
    );

    // 最终以全非空文本补写 v2（显示版本单调仍然成立）。
    sqlx::query(
        "INSERT INTO room_presentation_versions (room_id, participant_id, presentation_version, room_display_name, room_role_name, room_role_description, published_by_id, published_at) VALUES (?, ?, 2, 'D', 'R', 'RD', ?, 5)",
    )
    .bind(w.room)
    .bind(w.agent)
    .bind(w.human)
    .execute(store.pool())
    .await
    .expect("v2 with real text must insert after failed empty-string attempts");
}

#[tokio::test]
async fn room_presentation_idempotency_witness_guards_require_consistent_publisher_and_time() {
    let (_dir, store) = open().await;
    let w = seed_group_world(store.pool()).await;

    // 版本行 v1（发布者 human-owner-1，published_at=20），提交。
    sqlx::query(
        "INSERT INTO room_presentation_versions (room_id, participant_id, presentation_version, room_display_name, room_role_name, room_role_description, published_by_id, published_at) VALUES (?, ?, 1, 'D', NULL, NULL, ?, 20)",
    )
    .bind(w.room)
    .bind(w.agent)
    .bind(w.human)
    .execute(store.pool())
    .await
    .expect("v1 must insert");
    sqlx::query("INSERT INTO participants (id, kind, status, display_name, created_at) VALUES ('human-other', 'human', 'active', 'Other', 1)")
        .execute(store.pool())
        .await
        .unwrap();

    // actor != 被引用版本发布者 → 拒绝。
    let err = sqlx::query(
        "INSERT INTO room_presentation_put_idempotency (actor_human_id, room_id, participant_id, key, request_digest, result_presentation_version, result_is_create, created_at) VALUES ('human-other', ?, ?, 'rkey-pub', ?, 1, 1, 20)",
    )
    .bind(w.room)
    .bind(w.agent)
    .bind("a".repeat(64))
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("witness must match"), "{err}");

    // created_at != published_at(20) → 拒绝。
    let err = sqlx::query(
        "INSERT INTO room_presentation_put_idempotency (actor_human_id, room_id, participant_id, key, request_digest, result_presentation_version, result_is_create, created_at) VALUES (?, ?, ?, 'rkey-time', ?, 1, 1, 21)",
    )
    .bind(w.human)
    .bind(w.room)
    .bind(w.agent)
    .bind("b".repeat(64))
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("witness must match"), "{err}");

    // result_is_create 与版本不符（version 1 必须 is_create=1）→ CHECK 拒绝。
    let err = sqlx::query(
        "INSERT INTO room_presentation_put_idempotency (actor_human_id, room_id, participant_id, key, request_digest, result_presentation_version, result_is_create, created_at) VALUES (?, ?, ?, 'rkey-ic', ?, 1, 0, 20)",
    )
    .bind(w.human)
    .bind(w.room)
    .bind(w.agent)
    .bind("c".repeat(64))
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("CHECK"), "{err}");

    // 一致的记录通过。
    sqlx::query(
        "INSERT INTO room_presentation_put_idempotency (actor_human_id, room_id, participant_id, key, request_digest, result_presentation_version, result_is_create, created_at) VALUES (?, ?, ?, 'rkey-ok', ?, 1, 1, 20)",
    )
    .bind(w.human)
    .bind(w.room)
    .bind(w.agent)
    .bind("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789")
    .execute(store.pool())
    .await
    .expect("consistent room witness must pass");
}
