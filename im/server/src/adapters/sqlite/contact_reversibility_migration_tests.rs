//! Contact Reversibility migration 完整性测试（CORE_V1_CONTACT_REVERSIBILITY.md）。
//!
//! 在真实临时 SQLite 上应用全部 migration（含本切片 202609250001），用直接
//! SQL 证明本切片的 schema 守卫：联系段周期表（active-only INSERT、状态机、
//! 闭合原因耦合、闭合终态不可改写、不可删、REPLACE 身份防线、每对至多一个
//! active 段）、黑名单段（禁止自拉黑、两端 active Human、每方向至多一个
//! active 段、闭合终态、REPLACE 防线）、human_direct 配对改为见证「一段关系」
//! （一段关系至多一房、必须 active accepted 段）、段结束关闭双方 human_direct
//! 成员周期（closed_by=contact_ended + 各自 member.left）与 human 关闭守卫的
//! 同事务精确见证。
//! 本阶段不实现 adapter 之外的新协议，因此所有断言直接面向 migration 生成的
//! schema。

use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::ports::migration::MigrationProbe;
use sqlx::SqlitePool;
use uuid::Uuid;

const H_A: &str = "human-a";
const H_B: &str = "human-b";
const H_C: &str = "human-c";
const PERIOD_AB: &str = "11111111-1111-4111-8111-aaaaaaaaaaaa";
const ROOM_HD: &str = "human-direct-room-1";
const MEMBERSHIP_A: &str = "11111111-1111-4111-8111-bbbbbbbbbbbb";
const MEMBERSHIP_B: &str = "11111111-1111-4111-8111-cccccccccccc";

async fn open() -> (tempfile::TempDir, MigrationStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("open + migrate on a fresh DB");
    (dir, store)
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

/// 三个 active Human（A/B/C）。A<B<C 的顺序由 id 字面量决定（'human-a' <
/// 'human-b' < 'human-c'），恰好满足规范序 CHECK。
async fn seed_humans(pool: &SqlitePool) {
    for (id, name) in [(H_A, "Alice"), (H_B, "Bob"), (H_C, "Carol")] {
        sqlx::query(
            "INSERT INTO participants (id, kind, status, display_name, created_at) \
             VALUES (?, 'human', 'active', ?, 1)",
        )
        .bind(id)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    }
}

/// 建立 A–B 的 active accepted 关系段（用固定 period_id，便于后续被配对引用）。
async fn seed_accepted_period(pool: &SqlitePool) {
    sqlx::query(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, \
          accepted_at, closed_at, closed_by, active_marker, idempotency_key) \
         VALUES (?, ?, ?, ?, ?, 'accepted', 1, 2, NULL, NULL, 1, 'k-ab')",
    )
    .bind(PERIOD_AB)
    .bind(H_A)
    .bind(H_B)
    .bind(H_A)
    .bind(H_B)
    .execute(pool)
    .await
    .unwrap();
}

/// 建立 A–B 的 human_direct 房间：rooms + coverage + 配对（见证 PERIOD_AB）
/// + 两条 active 成员周期。全部在一个事务内提交。
async fn seed_human_direct_room(pool: &SqlitePool) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO rooms (id, kind, status, created_at, name) \
         VALUES (?, 'human_direct', 'active', 3, NULL)",
    )
    .bind(ROOM_HD)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO room_coverage \
         (room_id, kind, created_at, human_direct_a_id, human_direct_b_id) \
         VALUES (?, 'human_direct', 3, ?, ?)",
    )
    .bind(ROOM_HD)
    .bind(H_A)
    .bind(H_B)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO human_direct_pairs \
         (room_id, human_a_id, human_b_id, contact_period_id, human_a_membership_id, \
          human_b_membership_id, role_a, role_b, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, 'owner', 'member', 3)",
    )
    .bind(ROOM_HD)
    .bind(H_A)
    .bind(H_B)
    .bind(PERIOD_AB)
    .bind(MEMBERSHIP_A)
    .bind(MEMBERSHIP_B)
    .execute(&mut *tx)
    .await
    .unwrap();
    for (mid, participant, role) in [(MEMBERSHIP_A, H_A, "owner"), (MEMBERSHIP_B, H_B, "member")] {
        sqlx::query(
            "INSERT INTO room_memberships \
             (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
             VALUES (?, ?, ?, ?, 3, 0, 1)",
        )
        .bind(mid)
        .bind(ROOM_HD)
        .bind(participant)
        .bind(role)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
}

/// 闭合 A–B 段（closed_by='ended'），触发器应同事务关闭双方成员周期。
async fn end_period(pool: &SqlitePool) {
    sqlx::query(
        "UPDATE contact_periods SET active_marker = 0, closed_at = 30, closed_by = 'ended' \
         WHERE period_id = ?",
    )
    .bind(PERIOD_AB)
    .execute(pool)
    .await
    .expect("ending an active accepted period must succeed");
}

#[tokio::test]
async fn migration_swaps_contacts_table_and_creates_reversibility_tables() {
    let (_dir, store) = open().await;
    let status = store.status().await.expect("probe");
    assert_eq!(
        status,
        crate::ports::migration::MigrationStatus {
            pending: 0,
            failed: 0
        }
    );
    for table in ["contact_periods", "participant_blocks"] {
        assert!(
            tables_exist(store.pool(), table).await,
            "missing table {table}"
        );
    }
    assert!(
        !tables_exist(store.pool(), "contacts").await,
        "legacy contacts table must be gone (replaced by contact_periods)"
    );
}

#[tokio::test]
async fn contact_periods_enforce_active_only_and_state_machine() {
    let (_dir, store) = open().await;
    seed_humans(store.pool()).await;

    // 直接插入闭合段：拒绝（closed 只能由 UPDATE 关闭路径产生）。
    let closed = sqlx::query(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, \
          accepted_at, closed_at, closed_by, active_marker, idempotency_key) \
         VALUES (?, ?, ?, ?, ?, 'accepted', 1, 2, 3, 'ended', 0, 'k-x')",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(H_A)
    .bind(H_B)
    .bind(H_A)
    .bind(H_B)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(closed.to_string().contains("must be active"), "{closed}");

    seed_accepted_period(store.pool()).await;

    // accepted → pending 被状态机拒绝；accepted_at 不可改写。
    let revert = sqlx::query(
        "UPDATE contact_periods SET state = 'pending', accepted_at = NULL WHERE period_id = ?",
    )
    .bind(PERIOD_AB)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        revert.to_string().contains("can only move once"),
        "{revert}"
    );

    // 闭合原因与状态耦合：accepted 段不能被 declined 关闭。
    let declined = sqlx::query(
        "UPDATE contact_periods SET active_marker = 0, closed_at = 9, closed_by = 'declined' \
         WHERE period_id = ?",
    )
    .bind(PERIOD_AB)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        declined
            .to_string()
            .contains("declined closes a pending period"),
        "{declined}"
    );

    // 闭合必须一次写满（缺 closed_by）。
    let half = sqlx::query(
        "UPDATE contact_periods SET active_marker = 0, closed_at = 9 WHERE period_id = ?",
    )
    .bind(PERIOD_AB)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        half.to_string().contains("closed_at and a valid closed_by"),
        "{half}"
    );

    // closed_by 词表封闭：未知原因被 CHECK / 关闭原因耦合守卫拒绝。
    let unknown = sqlx::query(
        "UPDATE contact_periods SET active_marker = 0, closed_at = 9, closed_by = 'ghost' \
         WHERE period_id = ?",
    )
    .bind(PERIOD_AB)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        unknown.to_string().contains("CHECK")
            || unknown
                .to_string()
                .contains("closed_at and a valid closed_by")
            || unknown
                .to_string()
                .contains("ended closes an active period"),
        "{unknown}"
    );

    // 合法：ended 关闭 accepted 段。
    end_period(store.pool()).await;

    // 闭合段是终态：不可复活、不可改写任何列。
    for stmt in [
        "UPDATE contact_periods SET active_marker = 1, closed_at = NULL, closed_by = NULL WHERE period_id = ?",
        "UPDATE contact_periods SET accepted_at = 99 WHERE period_id = ?",
    ] {
        let err = sqlx::query(stmt)
            .bind(PERIOD_AB)
            .execute(store.pool())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("immutable"), "{stmt}: {err}");
    }

    // 不可硬删除。
    let del = sqlx::query("DELETE FROM contact_periods WHERE period_id = ?")
        .bind(PERIOD_AB)
        .execute(store.pool())
        .await
        .unwrap_err();
    assert!(del.to_string().contains("cannot be deleted"), "{del}");
}

#[tokio::test]
async fn contact_periods_reject_replace_and_one_active_period_per_pair() {
    let (_dir, store) = open().await;
    seed_humans(store.pool()).await;
    seed_accepted_period(store.pool()).await;

    // 普通重复 INSERT 与 REPLACE 都被身份防线拒绝（同对已有 active 段）。
    for stmt in [
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, \
          accepted_at, closed_at, closed_by, active_marker, idempotency_key) \
         VALUES ('22222222-2222-4222-8222-222222222222', 'human-a', 'human-b', 'human-a', 'human-b', \
                 'pending', 10, NULL, NULL, NULL, 1, 'k-new')",
        "INSERT OR REPLACE INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, \
          accepted_at, closed_at, closed_by, active_marker, idempotency_key) \
         VALUES ('22222222-2222-4222-8222-222222222222', 'human-a', 'human-b', 'human-a', 'human-b', \
                 'pending', 10, NULL, NULL, NULL, 1, 'k-new')",
    ] {
        let err = sqlx::query(stmt).execute(store.pool()).await.unwrap_err();
        assert!(
            err.to_string().contains("identity already exists"),
            "{stmt}: {err}"
        );
    }

    // 幂等键按 requester 唯一（跨周期）：同 requester + key 换另一对也被拒。
    let err = sqlx::query(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, \
          accepted_at, closed_at, closed_by, active_marker, idempotency_key) \
         VALUES (?, 'human-a', 'human-c', 'human-a', 'human-c', 'pending', 11, NULL, NULL, NULL, 1, 'k-ab')",
    )
    .bind(Uuid::new_v4().to_string())
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("identity already exists"), "{err}");

    // 段闭合后可再申请新 key（闭合段不占位，每对至多一个 active 段）。
    end_period(store.pool()).await;
    sqlx::query(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, \
          accepted_at, closed_at, closed_by, active_marker, idempotency_key) \
         VALUES (?, 'human-a', 'human-b', 'human-a', 'human-b', 'pending', 40, NULL, NULL, NULL, 1, 'k-ab-2')",
    )
    .bind(Uuid::new_v4().to_string())
    .execute(store.pool())
    .await
    .expect("a new period after closure must be allowed with a new key");

    // 无关的另一对可以正常插入（闭合段不占位不影响其它配对）。
    sqlx::query(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, \
          accepted_at, closed_at, closed_by, active_marker, idempotency_key) \
         VALUES (?, 'human-a', 'human-c', 'human-a', 'human-c', 'pending', 41, NULL, NULL, NULL, 1, 'k-ac-2')",
    )
    .bind(Uuid::new_v4().to_string())
    .execute(store.pool())
    .await
    .expect("unrelated pair must insert");

    // 两端必须是**不同且 active 的 participant**：批次三第二刀之后 Agent 是合法
    // 端点（形状由 `relations_accept_any_active_participant_pair` 证明），这里只
    // 证明**非 active** 的一端仍被守卫拒绝，且原文已换成说得通的那句。
    sqlx::query(
        "INSERT INTO participants (id, kind, status, display_name, created_at, revoked_at) \
         VALUES ('zagent-dead', 'agent', 'revoked', 'A', 1, 9)",
    )
    .execute(store.pool())
    .await
    .unwrap();
    let err = sqlx::query(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, \
          accepted_at, closed_at, closed_by, active_marker, idempotency_key) \
         VALUES (?, 'human-c', 'zagent-dead', 'human-c', 'zagent-dead', 'pending', 42, NULL, NULL, NULL, 1, 'k-cdead')",
    )
    .bind(Uuid::new_v4().to_string())
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("two different active participants"),
        "{err}"
    );
}

#[tokio::test]
async fn human_direct_pair_witnesses_one_active_accepted_period() {
    let (_dir, store) = open().await;
    seed_humans(store.pool()).await;
    seed_accepted_period(store.pool()).await;
    seed_human_direct_room(store.pool()).await;

    // 同一段关系至多一房：另建房间 + 复用同 period_id → 身份防线拒绝。
    let err = sqlx::query(
        "INSERT INTO human_direct_pairs \
         (room_id, human_a_id, human_b_id, contact_period_id, human_a_membership_id, \
          human_b_membership_id, role_a, role_b, created_at) \
         VALUES ('other-room', ?, ?, ?, ?, ?, 'owner', 'member', 4)",
    )
    .bind(H_A)
    .bind(H_B)
    .bind(PERIOD_AB)
    .bind(Uuid::new_v4().to_string())
    .bind(Uuid::new_v4().to_string())
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("identity already exists")
            || err
                .to_string()
                .contains("requires an active accepted contact period"),
        "{err}"
    );

    // 段闭合后配对不再满足「active accepted 段」：为另一对（A–C）建 pending
    // 段并尝试为其建配对 → 守卫拒绝（pending 段不能建 Human direct Room）。
    end_period(store.pool()).await;
    let pending_period_ac = "33333333-3333-4333-8333-bbbbbbbbbbbb";
    sqlx::query(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, \
          accepted_at, closed_at, closed_by, active_marker, idempotency_key) \
         VALUES (?, 'human-a', 'human-c', 'human-a', 'human-c', 'pending', 5, NULL, NULL, NULL, 1, 'k-ac')",
    )
    .bind(pending_period_ac)
    .execute(store.pool())
    .await
    .unwrap();
    let err = sqlx::query(
        "INSERT INTO human_direct_pairs \
         (room_id, human_a_id, human_b_id, contact_period_id, human_a_membership_id, \
          human_b_membership_id, role_a, role_b, created_at) \
         VALUES ('second-room', 'human-a', 'human-c', ?, ?, ?, 'owner', 'member', 5)",
    )
    .bind(pending_period_ac)
    .bind(Uuid::new_v4().to_string())
    .bind(Uuid::new_v4().to_string())
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("requires an active accepted contact period"),
        "{err}"
    );
}

#[tokio::test]
async fn ending_a_period_closes_both_human_direct_memberships_and_emits_left_facts() {
    let (_dir, store) = open().await;
    seed_humans(store.pool()).await;
    seed_accepted_period(store.pool()).await;
    seed_human_direct_room(store.pool()).await;

    end_period(store.pool()).await;

    // 双方成员周期都被关闭：closed_by=contact_ended、left_at=段的 closed_at。
    let rows: Vec<(String, i64, Option<i64>, Option<String>)> = sqlx::query_as(
        "SELECT participant_id, active_marker, left_at, closed_by FROM room_memberships \
         WHERE room_id = ? ORDER BY participant_id",
    )
    .bind(ROOM_HD)
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    for (participant, active, left_at, closed_by) in rows {
        assert_eq!(active, 0, "{participant} must be closed");
        assert_eq!(
            left_at,
            Some(30),
            "{participant} left_at = contact closed_at"
        );
        assert_eq!(closed_by.as_deref(), Some("contact_ended"), "{participant}");
    }

    // 双方各有一条 member.left 事实（closed_by=contact_ended），且只投递本人。
    let facts: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT source_id, created_at, snapshot FROM human_event_facts \
         WHERE kind = 'member.left' ORDER BY source_id",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert_eq!(facts.len(), 2, "one member.left per human");
    for (source, created_at, snapshot) in &facts {
        assert_eq!(created_at, &30);
        let snap: serde_json::Value = serde_json::from_str(snapshot).unwrap();
        assert_eq!(snap["closed_by"], "contact_ended");
        assert_eq!(snap["room_id"], ROOM_HD);
        assert!(source == MEMBERSHIP_A || source == MEMBERSHIP_B, "{source}");
    }
    // member.left 的 delivery 只投递主体本人（断交不通知对端）：每条 left
    // fact 恰好一条 delivery，且 human_id 正是该 fact 的主体。
    let left_deliveries: Vec<(String, String)> = sqlx::query_as(
        "SELECT d.human_id, f.source_id FROM human_event_deliveries d \
         JOIN human_event_facts f ON f.fact_id = d.fact_id \
         WHERE f.kind = 'member.left'",
    )
    .fetch_all(store.pool())
    .await
    .unwrap();
    assert_eq!(
        left_deliveries.len(),
        2,
        "one delivery per member.left fact"
    );
    for (human, source) in left_deliveries {
        let expected_membership = if human == H_A {
            MEMBERSHIP_A
        } else {
            MEMBERSHIP_B
        };
        assert_eq!(
            source, expected_membership,
            "{human} must only receive its own member.left"
        );
    }
}

#[tokio::test]
async fn human_close_guard_requires_precise_contact_end_witness_for_contact_ended() {
    let (_dir, store) = open().await;
    seed_humans(store.pool()).await;
    seed_accepted_period(store.pool()).await;
    seed_human_direct_room(store.pool()).await;

    // 段仍 active：直接以 contact_ended 关闭成员周期没有精确见证 → 拒绝
    //（照抄 transfer 的同事务精确见证纪律，绝不吃模糊历史）。
    let err = sqlx::query(
        "UPDATE room_memberships SET active_marker = 0, left_at = 30, left_before_sequence = 0, \
         closed_by = 'contact_ended' WHERE membership_id = ?",
    )
    .bind(MEMBERSHIP_A)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("precise contact end witness"),
        "{err}"
    );

    // 段以 ended 关闭后，触发器用精确见证（left_at = 段 closed_at = 30）完成
    // 同一事务内的双方关闭——守卫放行的正是这条路径。
    end_period(store.pool()).await;
    let (left_at, closed_by): (Option<i64>, Option<String>) =
        sqlx::query_as("SELECT left_at, closed_by FROM room_memberships WHERE membership_id = ?")
            .bind(MEMBERSHIP_A)
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(left_at, Some(30));
    assert_eq!(closed_by.as_deref(), Some("contact_ended"));

    // 闭合段与闭合成员周期都是终态：任何后续改写都被拒（不存在「改写为
    // 不匹配时点」的可达窗口——段的闭合触发器与守卫在同一事务内原子收口）。
    let err = sqlx::query("UPDATE room_memberships SET left_at = 31 WHERE membership_id = ?")
        .bind(MEMBERSHIP_A)
        .execute(store.pool())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("immutable"), "{err}");
}

#[tokio::test]
async fn participant_blocks_guards_and_replace_defence() {
    let (_dir, store) = open().await;
    seed_humans(store.pool()).await;

    let block_a_b = "44444444-4444-4444-8444-aaaaaaaaaaaa";

    // 合法拉黑。
    sqlx::query(
        "INSERT INTO participant_blocks (block_id, blocker_id, blocked_id, created_at, unblocked_at, active_marker) \
         VALUES (?, ?, ?, 1, NULL, 1)",
    )
    .bind(block_a_b)
    .bind(H_A)
    .bind(H_B)
    .execute(store.pool())
    .await
    .expect("valid block must insert");

    // 禁止自拉黑。
    let err = sqlx::query(
        "INSERT INTO participant_blocks (block_id, blocker_id, blocked_id, created_at, unblocked_at, active_marker) \
         VALUES (?, ?, ?, 1, NULL, 1)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(H_A)
    .bind(H_A)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("CHECK")
            || err
                .to_string()
                .contains("two different active participants"),
        "{err}"
    );

    // 同方向 active 段唯一：重复与 REPLACE 都被身份防线拒绝。
    for stmt in [
        "INSERT INTO participant_blocks (block_id, blocker_id, blocked_id, created_at, unblocked_at, active_marker) \
         VALUES (?, 'human-a', 'human-b', 2, NULL, 1)",
        "INSERT OR REPLACE INTO participant_blocks (block_id, blocker_id, blocked_id, created_at, unblocked_at, active_marker) \
         VALUES (?, 'human-a', 'human-b', 2, NULL, 1)",
    ] {
        let err = sqlx::query(stmt)
            .bind(Uuid::new_v4().to_string())
            .execute(store.pool())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("identity already exists"),
            "{stmt}: {err}"
        );
    }

    // 反方向是独立事实（A→B 与 B→A 互不影响）。
    sqlx::query(
        "INSERT INTO participant_blocks (block_id, blocker_id, blocked_id, created_at, unblocked_at, active_marker) \
         VALUES (?, ?, ?, 3, NULL, 1)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(H_B)
    .bind(H_A)
    .execute(store.pool())
    .await
    .expect("reverse direction is an independent fact");

    // 直接插入已闭合段被拒。
    let err = sqlx::query(
        "INSERT INTO participant_blocks (block_id, blocker_id, blocked_id, created_at, unblocked_at, active_marker) \
         VALUES (?, 'human-a', 'human-c', 4, 5, 0)",
    )
    .bind(Uuid::new_v4().to_string())
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("must be active"), "{err}");

    // **Agent 能被拉黑了**（批次三第三刀 第①步：拉黑是关系事实的对称面）——
    // 这一段从"必须被拒"翻成"必须成功"；两端仍必须是不同且 active 的 participant。
    sqlx::query(
        "INSERT INTO participants (id, kind, status, display_name, created_at) \
         VALUES ('agent-1', 'agent', 'active', 'A', 1)",
    )
    .execute(store.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO participant_blocks (block_id, blocker_id, blocked_id, created_at, unblocked_at, active_marker) \
         VALUES (?, 'human-a', 'agent-1', 6, NULL, 1)",
    )
    .bind(Uuid::new_v4().to_string())
    .execute(store.pool())
    .await
    .expect("a human must be able to block an agent now");

    // 非 active 的一端仍被拒（用 revoked Agent 验，因为表 CHECK 规定 Human 只能是
    // active），且原文已换成说得通的那句。
    sqlx::query(
        "INSERT INTO participants (id, kind, status, display_name, created_at, revoked_at) \
         VALUES ('zagent-dead', 'agent', 'revoked', 'D', 1, 9)",
    )
    .execute(store.pool())
    .await
    .unwrap();
    let err = sqlx::query(
        "INSERT INTO participant_blocks (block_id, blocker_id, blocked_id, created_at, unblocked_at, active_marker) \
         VALUES (?, 'human-a', 'zagent-dead', 7, NULL, 1)",
    )
    .bind(Uuid::new_v4().to_string())
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("two different active participants"),
        "非 active 的一端必须被拒（新原文），got: {err}"
    );
    assert!(
        !err.to_string().contains("active humans"),
        "旧原文（active humans）从此是假话，不该再出现：{err}"
    );

    // 闭合必须写满时点；闭合段是终态；不可删。
    let err = sqlx::query("UPDATE participant_blocks SET active_marker = 0 WHERE block_id = ?")
        .bind(block_a_b)
        .execute(store.pool())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("requires unblocked_at"), "{err}");

    sqlx::query(
        "UPDATE participant_blocks SET active_marker = 0, unblocked_at = 70 WHERE block_id = ?",
    )
    .bind(block_a_b)
    .execute(store.pool())
    .await
    .expect("unblock must succeed");

    let err = sqlx::query(
        "UPDATE participant_blocks SET active_marker = 1, unblocked_at = NULL WHERE block_id = ?",
    )
    .bind(block_a_b)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(err.to_string().contains("immutable"), "{err}");

    let err = sqlx::query("DELETE FROM participant_blocks WHERE block_id = ?")
        .bind(block_a_b)
        .execute(store.pool())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("cannot be deleted"), "{err}");

    // 解除后可再次拉黑（新段）。
    sqlx::query(
        "INSERT INTO participant_blocks (block_id, blocker_id, blocked_id, created_at, unblocked_at, active_marker) \
         VALUES (?, 'human-a', 'human-b', 80, NULL, 1)",
    )
    .bind(Uuid::new_v4().to_string())
    .execute(store.pool())
    .await
    .expect("re-blocking after unblock must be allowed");
}

#[tokio::test]
async fn room_memberships_accept_contact_ended_in_closed_by_vocabulary() {
    let (_dir, store) = open().await;
    seed_humans(store.pool()).await;
    seed_accepted_period(store.pool()).await;
    seed_human_direct_room(store.pool()).await;
    end_period(store.pool()).await;

    // closed_by 词表含 contact_ended（表级 CHECK 放行，见 seed 的关闭结果）。
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships WHERE closed_by = 'contact_ended'",
    )
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert_eq!(count, 2);

    // 未知 closed_by 仍被 CHECK 拒绝。
    let err = sqlx::query(
        "UPDATE room_memberships SET active_marker = 0, left_at = 9, left_before_sequence = 0, \
         closed_by = 'ghost' WHERE membership_id = ?",
    )
    .bind(MEMBERSHIP_A)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("closed membership periods are immutable")
            || err.to_string().contains("valid closed_by"),
        "{err}"
    );
}

#[tokio::test]
async fn active_block_requires_the_contact_period_to_be_ended_first() {
    let (_dir, store) = open().await;
    seed_humans(store.pool()).await;
    // A–B accepted（PERIOD_AB）；A–C pending（两个方向的 active 段都被覆盖）。
    seed_accepted_period(store.pool()).await;
    let pending_ac = "55555555-5555-4555-8555-aaaaaaaaaaaa";
    sqlx::query(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, \
          accepted_at, closed_at, closed_by, active_marker, idempotency_key) \
         VALUES (?, 'human-a', 'human-c', 'human-a', 'human-c', 'pending', 5, NULL, NULL, NULL, 1, 'k-ac')",
    )
    .bind(pending_ac)
    .execute(store.pool())
    .await
    .unwrap();

    // 三个写入形式（普通 INSERT / INSERT OR REPLACE / REPLACE）在两个方向上
    // 都被拒：关系仍 active 时拉黑，无论方向，都必须先结束段。
    for (label, blocker, blocked) in [("forward", H_A, H_B), ("reverse", H_B, H_A)] {
        for sql in [
            "INSERT INTO participant_blocks (block_id, blocker_id, blocked_id, created_at, unblocked_at, active_marker) \
             VALUES (?, ?, ?, 10, NULL, 1)",
            "INSERT OR REPLACE INTO participant_blocks (block_id, blocker_id, blocked_id, created_at, unblocked_at, active_marker) \
             VALUES (?, ?, ?, 10, NULL, 1)",
            "REPLACE INTO participant_blocks (block_id, blocker_id, blocked_id, created_at, unblocked_at, active_marker) \
             VALUES (?, ?, ?, 10, NULL, 1)",
        ] {
            let err = sqlx::query(sql)
                .bind(Uuid::new_v4().to_string())
                .bind(blocker)
                .bind(blocked)
                .execute(store.pool())
                .await
                .unwrap_err();
            assert!(
                err.to_string()
                    .contains("requires the contact period to be ended first"),
                "{label}: {sql}: {err}"
            );
        }
    }

    // pending 段（active_marker = 1）同样阻止拉黑：guard 检查的是 active 段，
    // 不区分 pending / accepted。
    let err = sqlx::query(
        "INSERT INTO participant_blocks (block_id, blocker_id, blocked_id, created_at, unblocked_at, active_marker) \
         VALUES (?, 'human-a', 'human-c', 11, NULL, 1)",
    )
    .bind(Uuid::new_v4().to_string())
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("requires the contact period to be ended first"),
        "{err}"
    );

    // 本守卫只作用于 active 拉黑段：直接插入 **闭合** 黑名单段时，即便该对
    // 关系仍 active，拒绝也来自 active-only 守卫（而非本守卫）——证明新守卫
    // 没有把「历史段追加」误判。
    let err = sqlx::query(
        "INSERT INTO participant_blocks (block_id, blocker_id, blocked_id, created_at, unblocked_at, active_marker) \
         VALUES (?, 'human-a', 'human-b', 12, 13, 0)",
    )
    .bind(Uuid::new_v4().to_string())
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("must be active"),
        "closed block insert must be rejected by the active-only guard, not the new one: {err}"
    );

    // 同一事务内：先结束段、再插黑名单段 → 提交成功（A–C pending 段）。
    let period_ac_block = "66666666-6666-4666-8666-aaaaaaaaaaaa";
    {
        let mut tx = store.pool().begin().await.unwrap();
        // 同一事务内：先结束段、再插黑名单段 → 提交成功。
        sqlx::query(
            "UPDATE contact_periods SET active_marker = 0, closed_at = 20, closed_by = 'declined' \
             WHERE period_id = ?",
        )
        .bind(pending_ac)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO participant_blocks (block_id, blocker_id, blocked_id, created_at, unblocked_at, active_marker) \
             VALUES (?, 'human-a', 'human-c', 20, NULL, 1)",
        )
        .bind(period_ac_block)
        .execute(&mut *tx)
        .await
        .expect("ending the period first must let the block insert pass in the same transaction");
        tx.commit().await.unwrap();
    }

    // 段早已在更早的事务里结束（A–B 仍是 active）：先结束该段再拉黑，同一
    // 事务内成功；此后反向拉黑（段已闭合，普通 INSERT）也合法。
    let block_a_b = "77777777-7777-4777-8777-aaaaaaaaaaaa";
    {
        let mut tx = store.pool().begin().await.unwrap();
        sqlx::query(
            "UPDATE contact_periods SET active_marker = 0, closed_at = 30, closed_by = 'ended' \
             WHERE period_id = ?",
        )
        .bind(PERIOD_AB)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO participant_blocks (block_id, blocker_id, blocked_id, created_at, unblocked_at, active_marker) \
             VALUES (?, 'human-a', 'human-b', 30, NULL, 1)",
        )
        .bind(block_a_b)
        .execute(&mut *tx)
        .await
        .expect("ending the period first must let the block insert pass");
        tx.commit().await.unwrap();
    }
    sqlx::query(
        "INSERT INTO participant_blocks (block_id, blocker_id, blocked_id, created_at, unblocked_at, active_marker) \
         VALUES (?, 'human-b', 'human-a', 31, NULL, 1)",
    )
    .bind(Uuid::new_v4().to_string())
    .execute(store.pool())
    .await
    .expect("blocking a pair whose period already ended in an earlier transaction must pass");

    // 落地事实复核：恰好三条 active 拉黑段（A→C、A→B、B→A）。
    let active: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM participant_blocks WHERE active_marker = 1")
            .fetch_one(store.pool())
            .await
            .unwrap();
    assert_eq!(active, 3);
}

/// 批次三·第二刀（migration 202609270006）：**关系是两个主体之间的事实**——
/// 段的两端不再要求 Human，只要求 active participant。
///
/// 这一条只证明**放开的那一处**与**没有放开的所有处**：Agent–Agent、
/// Human–Agent 都能建段；非 active 的一端、自己对自己、同对第二个 active 段、
/// REPLACE 改写、闭合段直接插入，逐条仍被拒（表 CHECK、身份守卫、部分唯一
/// 索引、状态机、闭合耦合全部原样生效——本迁移只重建了一道触发器）。
///
/// ⚠ 断言只认「被拒绝 + 命中任一合法原文」：BEFORE 触发器之间的点火顺序不受
/// 承诺，同一写入被多道守卫拒绝时谁的消息先到会变（第一刀已踩过一次）。
#[tokio::test]
async fn relations_accept_any_active_participant_pair() {
    let (_dir, store) = open().await;
    seed_humans(store.pool()).await;
    // 参与者 id 的规范序由字面量决定（'human-*' < 'zagent-*'），恰好满足
    // 表的 CHECK a_id < b_id。
    for (id, kind, status, revoked_at) in [
        ("zagent-x", "agent", "active", None),
        ("zagent-y", "agent", "active", None),
        ("zagent-dead", "agent", "revoked", Some(9_i64)),
    ] {
        sqlx::query(
            "INSERT INTO participants (id, kind, status, display_name, created_at, revoked_at) \
             VALUES (?, ?, ?, ?, 1, ?)",
        )
        .bind(id)
        .bind(kind)
        .bind(status)
        .bind(id)
        .bind(revoked_at)
        .execute(store.pool())
        .await
        .expect("seed participant");
    }

    // 插入一段（调用方给出规范序的两个端点）。
    let insert_period = |a: &'static str, b: &'static str, key: &'static str| {
        let pool = store.pool().clone();
        async move {
            sqlx::query(
                "INSERT INTO contact_periods \
                 (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, \
                  accepted_at, closed_at, closed_by, active_marker, idempotency_key) \
                 VALUES (?, ?, ?, ?, ?, 'pending', 1, NULL, NULL, NULL, 1, ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(a)
            .bind(b)
            .bind(a)
            .bind(b)
            .bind(key)
            .execute(&pool)
            .await
        }
    };

    // ── 放开的（本刀的目的） ────────────────────────────────────────────────
    insert_period("human-a", "zagent-x", "k-hx")
        .await
        .expect("Human ↔ Agent 的段必须能建");
    insert_period("zagent-x", "zagent-y", "k-xy")
        .await
        .expect("Agent ↔ Agent 的段必须能建");
    // 回归：Human ↔ Human 的既有形状一字未变。
    insert_period("human-a", "human-b", "k-ab")
        .await
        .expect("Human ↔ Human 的段仍然能建");

    // 方向不限 kind：Agent 当 requester（换个 key 换一对端点）。
    sqlx::query(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, \
          accepted_at, closed_at, closed_by, active_marker, idempotency_key) \
         VALUES (?, 'human-c', 'zagent-y', 'zagent-y', 'human-c', 'pending', 1, NULL, NULL, NULL, 1, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind("k-cy")
    .execute(store.pool())
    .await
    .expect("Agent 当 requester 必须能建");

    // ── 没有放开的（逐条仍是拒绝） ──────────────────────────────────────────
    let revoked = insert_period("human-a", "zagent-dead", "k-hdead")
        .await
        .unwrap_err();
    assert!(
        revoked
            .to_string()
            .contains("two different active participants"),
        "非 active 的一端必须被拒（新原文），got: {revoked}"
    );
    assert!(
        !revoked.to_string().contains("active humans"),
        "旧原文（active humans）从此是假话，不该再出现：{revoked}"
    );

    let ghost = insert_period("human-a", "zagent-ghost", "k-hghost")
        .await
        .unwrap_err();
    assert!(
        ghost
            .to_string()
            .contains("two different active participants"),
        "不存在的端点必须被拒，got: {ghost}"
    );

    let same = insert_period("human-a", "human-a", "k-self")
        .await
        .unwrap_err();
    assert!(
        same.to_string().contains("CHECK constraint failed"),
        "自己对自己必须被 CHECK 拒绝，got: {same}"
    );

    // 同一对第二个 active 段 / REPLACE 改写既有段：身份守卫或部分唯一索引，
    // 谁先到不受承诺 ⇒ 两种原文都接受。
    let duplicate = insert_period("human-a", "zagent-x", "k-hx-2")
        .await
        .map(|_| ())
        .unwrap_err()
        .to_string();
    assert!(
        duplicate.contains("identity already exists")
            || duplicate.contains("UNIQUE constraint failed"),
        "同一对第二个 active 段必须被拒，got: {duplicate}"
    );
    let replaced = sqlx::query(
        "INSERT OR REPLACE INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, \
          accepted_at, closed_at, closed_by, active_marker, idempotency_key) \
         VALUES (?, 'human-a', 'zagent-x', 'human-a', 'zagent-x', 'pending', 1, NULL, NULL, NULL, 1, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind("k-hx-3")
    .execute(store.pool())
    .await
    .map(|_| ())
    .unwrap_err()
    .to_string();
    assert!(
        replaced.contains("identity already exists")
            || replaced.contains("UNIQUE constraint failed"),
        "REPLACE 改写既有 active 段必须被拒，got: {replaced}"
    );

    // 闭合段不能直接插入（active-only 守卫）。
    let closed = sqlx::query(
        "INSERT INTO contact_periods \
         (period_id, a_id, b_id, requester_id, addressee_id, state, created_at, \
          accepted_at, closed_at, closed_by, active_marker, idempotency_key) \
         VALUES (?, 'zagent-x', 'zagent-y', 'zagent-x', 'zagent-y', 'pending', 1, NULL, 2, 'ended', 0, 'k-xy-2')",
    )
    .bind(Uuid::new_v4().to_string())
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        closed.to_string().contains("must be active"),
        "闭合段不能直接插入，got: {closed}"
    );

    // 状态机对 Agent 段同样生效：pending → accepted 合法，回退被拒。
    let agent_period: String =
        sqlx::query_scalar("SELECT period_id FROM contact_periods WHERE idempotency_key = 'k-xy'")
            .fetch_one(store.pool())
            .await
            .unwrap();
    sqlx::query(
        "UPDATE contact_periods SET state = 'accepted', accepted_at = 5 WHERE period_id = ?",
    )
    .bind(&agent_period)
    .execute(store.pool())
    .await
    .expect("Agent 段也必须能 pending → accepted");
    let revert = sqlx::query(
        "UPDATE contact_periods SET state = 'pending', accepted_at = NULL WHERE period_id = ?",
    )
    .bind(&agent_period)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        revert.to_string().contains("can only move once"),
        "Agent 段的状态机回退必须被拒，got: {revert}"
    );
}
