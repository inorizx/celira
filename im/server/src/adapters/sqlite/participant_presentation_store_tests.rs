//! Participant Presentation SQLite adapter 契约测试。
//!
//! 在真实临时 SQLite + 全部 migration 上证明窄 adapter 的单个 `BEGIN
//! IMMEDIATE` 事务语义（CORE_V1_PARTICIPANT_PRESENTATION.md §5–§6）：授权先于
//! 重放、同 key 同摘要恒重放（即使 head 已推进）、同 key 不同摘要冲突、
//! 新 key stale、写后读回、失败回滚不留半成品、`published_by_kind` 派生、
//! 摘要列表拼装。种子世界用直接 SQL（与 migration 测试同一闭合 group 世界），
//! 只通过端口调用 adapter 做被测操作。

use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::participant_presentation_store::SqliteParticipantPresentationStore;
use crate::domain::agent::IdempotencyKey;
use crate::domain::participant::{ParticipantId, ParticipantKind};
use crate::domain::participant_presentation::{
    BaseVersion, ParticipantProfileVersion, PresentationVersion, ProfileVersion, RequestDigest,
    RoomDisplayName, RoomPresentationVersion, RoomRoleDescription, RoomRoleName, ShortDescription,
    profile_put_digest, room_presentation_put_digest,
};
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::ports::participant_presentation::{ParticipantPresentationStore as _, StoreError};
use crate::ports::storage::StorageFailureKind;
use sqlx::SqlitePool;

const WORLD_HUMAN: &str = "11111111-1111-4111-8111-111111111111";
const WORLD_AGENT: &str = "22222222-2222-4222-8222-222222222222";
const WORLD_ROOM: &str = "33333333-3333-4333-8333-333333333333";
const OWNER_MEMBERSHIP: &str = "44444444-4444-4444-8444-444444444444";
const AGENT_MEMBERSHIP: &str = "55555555-5555-4555-8555-555555555555";

struct Fixture {
    _dir: tempfile::TempDir,
    pool: SqlitePool,
    store: SqliteParticipantPresentationStore,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("open + migrate");
    let pool = store.pool().clone();
    let presentation = SqliteParticipantPresentationStore::new(pool.clone());
    seed_group_world(&pool).await;
    let _ = store;
    Fixture {
        _dir: dir,
        pool,
        store: presentation,
    }
}

/// 闭合 group 世界：active Human owner、active Agent、active Owner 关系、
/// group Room + coverage + owner/member 两条 active 周期。
async fn seed_group_world(pool: &SqlitePool) {
    let human: &str = WORLD_HUMAN;
    let agent: &str = WORLD_AGENT;
    let room: &str = WORLD_ROOM;
    let mut tx = pool.begin().await.unwrap();
    for (id, kind, status, name) in [
        (human, "human", "active", "Owner"),
        (agent, "agent", "active", "Agent"),
    ] {
        sqlx::query("INSERT INTO participants (id, kind, status, display_name, created_at) VALUES (?, ?, ?, ?, 1)")
            .bind(id)
            .bind(kind)
            .bind(status)
            .bind(name)
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO agent_owners (agent_id, human_id, status, created_at) VALUES (?, ?, 'active', 1)")
        .bind(agent)
        .bind(human)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO rooms (id, kind, status, created_at, name) VALUES (?, 'group', 'active', 1, 'g')")
        .bind(room)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO room_coverage (room_id, kind, created_at, group_owner_membership_id, group_owner_role, group_owner_active_marker) VALUES (?, 'group', 1, ?, 'owner', 1)")
        .bind(room)
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
    .bind(room)
    .bind(human)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO room_memberships \
         (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
         VALUES (?, ?, ?, 'member', 1, 0, 1)",
    )
    .bind(AGENT_MEMBERSHIP)
    .bind(room)
    .bind(agent)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

fn profile_record(
    participant_id: ParticipantId,
    short: Option<&str>,
    public: Option<&str>,
) -> ParticipantProfileVersion {
    ParticipantProfileVersion {
        participant_id,
        profile_version: ProfileVersion::from_u64(1).unwrap(),
        short_description: short.map(|s| ShortDescription::parse(s).unwrap()),
        public_description: public
            .map(|s| crate::domain::participant_presentation::PublicDescription::parse(s).unwrap()),
        avatar: None,
        published_by_id: ParticipantId(WORLD_HUMAN.to_owned()),
        published_at: UtcMillis::from_millis(10),
    }
}

fn presentation_record(
    room_id: RoomId,
    participant_id: ParticipantId,
    display: Option<&str>,
    role_name: Option<&str>,
    role_desc: Option<&str>,
) -> RoomPresentationVersion {
    RoomPresentationVersion {
        room_id,
        participant_id,
        presentation_version: PresentationVersion::from_u64(1).unwrap(),
        room_display_name: display.map(|s| RoomDisplayName::parse(s).unwrap()),
        room_role_name: role_name.map(|s| RoomRoleName::parse(s).unwrap()),
        room_role_description: role_desc.map(|s| RoomRoleDescription::parse(s).unwrap()),
        published_by_id: ParticipantId(WORLD_HUMAN.to_owned()),
        published_at: UtcMillis::from_millis(20),
    }
}

fn key(s: &str) -> IdempotencyKey {
    IdempotencyKey::parse(s).unwrap()
}

fn digest_hex(h: &str) -> RequestDigest {
    RequestDigest::parse_hex(h).unwrap()
}

async fn row_counts(pool: &SqlitePool) -> (i64, i64) {
    let profiles: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM participant_profile_versions")
        .fetch_one(pool)
        .await
        .unwrap();
    let witnesses: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM participant_profile_put_idempotency")
            .fetch_one(pool)
            .await
            .unwrap();
    (profiles, witnesses)
}

#[tokio::test]
async fn profile_put_creates_version_one_with_witness_and_is_derivable() {
    let f = fixture().await;
    let agent_id = ParticipantId(WORLD_AGENT.to_owned());
    let digest = profile_put_digest(BaseVersion::ZERO, Some("hi"), Some("world"));
    let outcome = f
        .store
        .put_profile_version(
            &key("profile-key-1"),
            &digest,
            &BaseVersion::ZERO,
            &profile_record(agent_id.clone(), Some("hi"), Some("world")),
            UtcMillis::from_millis(10),
        )
        .await
        .expect("owner writes v1");
    assert!(outcome.created);
    assert_eq!(outcome.version.as_u64(), 1);
    assert!(outcome.result_is_create);
    assert_eq!(
        outcome.result.short_description.as_ref().unwrap().as_str(),
        "hi"
    );

    let read = f
        .store
        .latest_profile(&agent_id)
        .await
        .expect("read back")
        .expect("profile exists");
    assert_eq!(read.profile_version.as_u64(), 1);
    assert_eq!(read.published_by_kind, ParticipantKind::Human);
    assert_eq!(read.published_at.millis(), 10);
}

#[tokio::test]
async fn profile_replay_same_key_same_digest_returns_historical_even_after_head_advance() {
    let f = fixture().await;
    let agent_id = ParticipantId(WORLD_AGENT.to_owned());
    let d1 = profile_put_digest(BaseVersion::ZERO, Some("hi"), Some("world"));
    let first = f
        .store
        .put_profile_version(
            &key("k-replay"),
            &d1,
            &BaseVersion::ZERO,
            &profile_record(agent_id.clone(), Some("hi"), Some("world")),
            UtcMillis::from_millis(10),
        )
        .await
        .expect("first create");
    assert!(first.created);
    assert_eq!(first.version.as_u64(), 1);

    // 另一 key 把 head 推进到 v2。
    let d2 = profile_put_digest(BaseVersion::parse(1).unwrap(), Some("updated"), None);
    let second = f
        .store
        .put_profile_version(
            &key("k-advance"),
            &d2,
            &BaseVersion::parse(1).unwrap(),
            &profile_record(agent_id.clone(), Some("updated"), None),
            UtcMillis::from_millis(20),
        )
        .await
        .expect("advance head to v2");
    assert!(second.created);
    assert_eq!(second.version.as_u64(), 2);

    // 同 key 同摘要重放：即使 head 已推进到 2、base_version=0，也返回首次结果
    // v1，绝不 stale、绝不追加版本。
    let replay = f
        .store
        .put_profile_version(
            &key("k-replay"),
            &d1,
            &BaseVersion::ZERO,
            &profile_record(agent_id.clone(), Some("hi"), Some("world")),
            UtcMillis::from_millis(30),
        )
        .await
        .expect("replay succeeds");
    assert!(!replay.created);
    assert_eq!(replay.version.as_u64(), 1);
    assert!(replay.result_is_create);
    assert_eq!(
        replay.result.short_description.as_ref().unwrap().as_str(),
        "hi",
        "replay returns the historical row"
    );

    let (profiles, witnesses) = row_counts(&f.pool).await;
    assert_eq!(profiles, 2, "replay must not append a version");
    assert_eq!(witnesses, 2, "replay must not add a witness");
}

#[tokio::test]
async fn profile_same_key_different_digest_is_conflict_and_stale_new_key_rejected() {
    let f = fixture().await;
    let agent_id = ParticipantId(WORLD_AGENT.to_owned());
    let d1 = profile_put_digest(BaseVersion::ZERO, Some("hi"), None);
    f.store
        .put_profile_version(
            &key("k-conflict"),
            &d1,
            &BaseVersion::ZERO,
            &profile_record(agent_id.clone(), Some("hi"), None),
            UtcMillis::from_millis(10),
        )
        .await
        .expect("first create");

    let d2 = profile_put_digest(BaseVersion::ZERO, Some("different"), None);
    let err = f
        .store
        .put_profile_version(
            &key("k-conflict"),
            &d2,
            &BaseVersion::ZERO,
            &profile_record(agent_id.clone(), Some("different"), None),
            UtcMillis::from_millis(20),
        )
        .await
        .expect_err("different digest must conflict");
    assert!(matches!(err, StoreError::IdempotencyConflict), "{err:?}");

    // 新 key 但 base_version=0（head 已 1）→ stale。
    let d3 = profile_put_digest(BaseVersion::ZERO, Some("x"), None);
    let err = f
        .store
        .put_profile_version(
            &key("k-stale"),
            &d3,
            &BaseVersion::ZERO,
            &profile_record(agent_id.clone(), Some("x"), None),
            UtcMillis::from_millis(30),
        )
        .await
        .expect_err("stale base_version must be rejected");
    assert!(
        matches!(err, StoreError::ConcurrentVersionChanged),
        "{err:?}"
    );

    let (profiles, witnesses) = row_counts(&f.pool).await;
    assert_eq!(profiles, 1, "conflict and stale must not append versions");
    assert_eq!(witnesses, 1, "conflict and stale must not add witnesses");
}

#[tokio::test]
async fn profile_authorization_precedes_replay_and_revoked_owner_fails_closed() {
    let f = fixture().await;
    let agent_id = ParticipantId(WORLD_AGENT.to_owned());
    let d1 = profile_put_digest(BaseVersion::ZERO, Some("hi"), None);
    f.store
        .put_profile_version(
            &key("k-auth"),
            &d1,
            &BaseVersion::ZERO,
            &profile_record(agent_id.clone(), Some("hi"), None),
            UtcMillis::from_millis(10),
        )
        .await
        .expect("first create");

    // 撤销 Owner 关系后，同 key 同摘要的重放也必须在授权裁决阶段 fail closed
    //（授权先于重放：被撤销 actor 不得经重放访问结果）。
    sqlx::query(
        "UPDATE agent_owners SET status = 'revoked', revoked_at = 99 WHERE agent_id = ? AND human_id = ?",
    )
    .bind(WORLD_AGENT)
    .bind(WORLD_HUMAN)
    .execute(&f.pool)
    .await
    .unwrap();
    let err = f
        .store
        .put_profile_version(
            &key("k-auth"),
            &d1,
            &BaseVersion::ZERO,
            &profile_record(agent_id, Some("hi"), None),
            UtcMillis::from_millis(40),
        )
        .await
        .expect_err("revoked owner must not replay");
    assert!(matches!(err, StoreError::ParticipantNotActive), "{err:?}");

    let (profiles, witnesses) = row_counts(&f.pool).await;
    assert_eq!(profiles, 1);
    assert_eq!(witnesses, 1);
}

#[tokio::test]
async fn profile_non_owner_and_unknown_target_are_hidden() {
    let f = fixture().await;
    sqlx::query("INSERT INTO participants (id, kind, status, display_name, created_at) VALUES ('human2-id', 'human', 'active', 'Stranger', 1)")
        .execute(&f.pool)
        .await
        .unwrap();
    // Stranger 对该 agent 无 Owner 关系 → NotAgentOwner。
    let record = ParticipantProfileVersion {
        participant_id: ParticipantId(WORLD_AGENT.to_owned()),
        profile_version: ProfileVersion::from_u64(1).unwrap(),
        short_description: None,
        public_description: None,
        avatar: None,
        published_by_id: ParticipantId("human2-id".to_owned()),
        published_at: UtcMillis::from_millis(1),
    };
    let err = f
        .store
        .put_profile_version(
            &key("k-stranger"),
            &digest_hex(&"a".repeat(64)),
            &BaseVersion::ZERO,
            &record,
            UtcMillis::from_millis(1),
        )
        .await
        .expect_err("stranger must not write");
    assert!(matches!(err, StoreError::NotAgentOwner), "{err:?}");

    // 未知目标 → UnknownParticipant。
    let record = ParticipantProfileVersion {
        participant_id: ParticipantId("ghost-id".to_owned()),
        profile_version: ProfileVersion::from_u64(1).unwrap(),
        short_description: None,
        public_description: None,
        avatar: None,
        published_by_id: ParticipantId(WORLD_HUMAN.to_owned()),
        published_at: UtcMillis::from_millis(1),
    };
    let err = f
        .store
        .put_profile_version(
            &key("k-ghost"),
            &digest_hex(&"b".repeat(64)),
            &BaseVersion::ZERO,
            &record,
            UtcMillis::from_millis(1),
        )
        .await
        .expect_err("ghost target must be hidden");
    assert!(matches!(err, StoreError::UnknownParticipant), "{err:?}");
}

#[tokio::test]
async fn presentation_put_create_update_conflict_replay_and_stale() {
    let f = fixture().await;
    let room = RoomId(WORLD_ROOM.to_owned());
    let agent = ParticipantId(WORLD_AGENT.to_owned());

    let d1 = room_presentation_put_digest(BaseVersion::ZERO, Some("Display"), Some("Role"), None);
    let first = f
        .store
        .put_room_presentation_version(
            &key("pres-1"),
            &d1,
            &BaseVersion::ZERO,
            &presentation_record(
                room.clone(),
                agent.clone(),
                Some("Display"),
                Some("Role"),
                None,
            ),
            UtcMillis::from_millis(20),
        )
        .await
        .expect("group owner creates v1");
    assert!(first.created);
    assert_eq!(first.version.as_u64(), 1);
    assert!(first.result_is_create);

    let d2 = room_presentation_put_digest(BaseVersion::parse(1).unwrap(), Some("D2"), None, None);
    let second = f
        .store
        .put_room_presentation_version(
            &key("pres-2"),
            &d2,
            &BaseVersion::parse(1).unwrap(),
            &presentation_record(room.clone(), agent.clone(), Some("D2"), None, None),
            UtcMillis::from_millis(30),
        )
        .await
        .expect("update to v2");
    assert!(second.created);
    assert_eq!(second.version.as_u64(), 2);
    assert!(!second.result_is_create);

    // 同 key 同摘要重放 v1（head 已 2）。
    let replay = f
        .store
        .put_room_presentation_version(
            &key("pres-1"),
            &d1,
            &BaseVersion::ZERO,
            &presentation_record(
                room.clone(),
                agent.clone(),
                Some("Display"),
                Some("Role"),
                None,
            ),
            UtcMillis::from_millis(40),
        )
        .await
        .expect("replay v1");
    assert!(!replay.created);
    assert_eq!(replay.version.as_u64(), 1);
    assert!(replay.result_is_create);
    assert_eq!(
        replay.result.room_display_name.as_ref().unwrap().as_str(),
        "Display"
    );

    // 同 key 不同摘要 → 冲突。
    let d3 = room_presentation_put_digest(BaseVersion::parse(1).unwrap(), Some("X"), None, None);
    let err = f
        .store
        .put_room_presentation_version(
            &key("pres-2"),
            &d3,
            &BaseVersion::parse(1).unwrap(),
            &presentation_record(room.clone(), agent.clone(), Some("X"), None, None),
            UtcMillis::from_millis(50),
        )
        .await
        .expect_err("conflict");
    assert!(matches!(err, StoreError::IdempotencyConflict), "{err:?}");

    // 新 key stale。
    let d4 = room_presentation_put_digest(BaseVersion::ZERO, Some("Y"), None, None);
    let err = f
        .store
        .put_room_presentation_version(
            &key("pres-stale"),
            &d4,
            &BaseVersion::ZERO,
            &presentation_record(room.clone(), agent.clone(), Some("Y"), None, None),
            UtcMillis::from_millis(60),
        )
        .await
        .expect_err("stale");
    assert!(
        matches!(err, StoreError::ConcurrentVersionChanged),
        "{err:?}"
    );

    let presentations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM room_presentation_versions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let witnesses: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM room_presentation_put_idempotency")
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(presentations, 2);
    assert_eq!(witnesses, 2);
}

#[tokio::test]
async fn presentation_rejects_non_group_non_owner_and_non_member_target() {
    let f = fixture().await;
    let room = RoomId(WORLD_ROOM.to_owned());
    let agent = ParticipantId(WORLD_AGENT.to_owned());
    // 非 owner 的第三个 Human。
    sqlx::query("INSERT INTO participants (id, kind, status, display_name, created_at) VALUES ('human-str', 'human', 'active', 'S', 1)")
        .execute(&f.pool)
        .await
        .unwrap();

    let d = room_presentation_put_digest(BaseVersion::ZERO, None, None, None);
    let wrong_pub = presentation_record(room.clone(), agent.clone(), None, None, None);
    let record = RoomPresentationVersion {
        published_by_id: ParticipantId("human-str".to_owned()),
        ..wrong_pub
    };
    let err = f
        .store
        .put_room_presentation_version(
            &key("p-x"),
            &d,
            &BaseVersion::ZERO,
            &record,
            UtcMillis::from_millis(1),
        )
        .await
        .expect_err("non-owner must fail");

    assert!(matches!(err, StoreError::NotGroupOwner), "{err:?}");

    // 目标不是 active Agent member → member not found（owner 自己当目标不算）。
    let record = presentation_record(
        room.clone(),
        ParticipantId(WORLD_HUMAN.to_owned()),
        None,
        None,
        None,
    );
    let err = f
        .store
        .put_room_presentation_version(
            &key("p-y"),
            &d,
            &BaseVersion::ZERO,
            &record,
            UtcMillis::from_millis(1),
        )
        .await
        .expect_err("human target must fail");
    assert!(matches!(err, StoreError::NotActiveAgentMember), "{err:?}");
}

#[tokio::test]
async fn presentation_unsupported_for_direct_room() {
    let f = fixture().await;
    // 新建一个 direct Room（owner human + member agent）同一事务。
    let agent = ParticipantId(WORLD_AGENT.to_owned());
    let direct_room = RoomId("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_owned());
    let direct_owner_membership = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    let direct_agent_membership = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    {
        let mut tx = f.pool.begin().await.unwrap();
        sqlx::query("INSERT INTO rooms (id, kind, status, created_at, name) VALUES (?, 'direct', 'active', 1, NULL)")
            .bind(direct_room.as_str())
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO room_coverage (room_id, kind, created_at, direct_human_id, direct_agent_id) VALUES (?, 'direct', 1, ?, ?)")
            .bind(direct_room.as_str())
            .bind(WORLD_HUMAN)
            .bind(WORLD_AGENT)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO direct_room_pairs (room_id, human_id, agent_id, human_membership_id, agent_membership_id, created_at) VALUES (?, ?, ?, ?, ?, 1)")
            .bind(direct_room.as_str())
            .bind(WORLD_HUMAN)
            .bind(WORLD_AGENT)
            .bind(direct_owner_membership)
            .bind(direct_agent_membership)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO room_memberships (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) VALUES (?, ?, ?, 'owner', 1, 0, 1)",
        )
        .bind(direct_owner_membership)
        .bind(direct_room.as_str())
        .bind(WORLD_HUMAN)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO room_memberships (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) VALUES (?, ?, ?, 'member', 1, 0, 1)",
        )
        .bind(direct_agent_membership)
        .bind(direct_room.as_str())
        .bind(WORLD_AGENT)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    let d = room_presentation_put_digest(BaseVersion::ZERO, None, None, None);
    // 该 room 不存在 presentation；owner 对直接 room 的演示写被 DB 守卫拒绝，
    // 但 adapter 应先裁决 kind → NotGroupRoom（在写事务内，不留半成品）。
    let record = presentation_record(direct_room.clone(), agent, None, None, None);
    let err = f
        .store
        .put_room_presentation_version(
            &key("p-direct"),
            &d,
            &BaseVersion::ZERO,
            &record,
            UtcMillis::from_millis(1),
        )
        .await
        .expect_err("direct room must be unsupported");
    assert!(matches!(err, StoreError::NotGroupRoom), "{err:?}");

    // 未写任何 presentation 行。
    let presentations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM room_presentation_versions")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(presentations, 0);
}

#[tokio::test]
async fn member_presentation_summaries_join_profile_and_presentation_without_full_descriptions_flag()
 {
    let f = fixture().await;
    let room = RoomId(WORLD_ROOM.to_owned());
    let agent = ParticipantId(WORLD_AGENT.to_owned());
    // 写 profile 与 presentation。
    let dp = profile_put_digest(
        BaseVersion::ZERO,
        Some("short bio"),
        Some("very long public"),
    );
    f.store
        .put_profile_version(
            &key("summary-p"),
            &dp,
            &BaseVersion::ZERO,
            &profile_record(agent.clone(), Some("short bio"), Some("very long public")),
            UtcMillis::from_millis(10),
        )
        .await
        .expect("profile");
    let dr = room_presentation_put_digest(
        BaseVersion::ZERO,
        Some("RoomDisp"),
        Some("RoleN"),
        Some("very long role description that must show only as preview"),
    );
    f.store
        .put_room_presentation_version(
            &key("summary-r"),
            &dr,
            &BaseVersion::ZERO,
            &presentation_record(
                room.clone(),
                agent.clone(),
                Some("RoomDisp"),
                Some("RoleN"),
                Some("very long role description that must show only as preview"),
            ),
            UtcMillis::from_millis(20),
        )
        .await
        .expect("presentation");

    let summaries = f
        .store
        .member_presentation_summaries(&room)
        .await
        .expect("summaries");
    // 摘要按 Participant 建模：active Human（owner）与 Agent 成员都在内；
    // 只给 Agent 写了 profile，只有 Agent 有 presentation 字段。
    assert_eq!(summaries.len(), 2, "{summaries:?}");
    let agent_s = summaries
        .iter()
        .find(|s| s.participant.id == agent)
        .expect("agent is a member");
    assert_eq!(agent_s.participant.kind, ParticipantKind::Agent);
    assert_eq!(
        agent_s.short_description.as_ref().unwrap().as_str(),
        "short bio"
    );
    assert_eq!(
        agent_s.room_display_name.as_ref().unwrap().as_str(),
        "RoomDisp"
    );
    assert_eq!(agent_s.room_role_name.as_ref().unwrap().as_str(), "RoleN");
    assert_eq!(
        agent_s.room_role_description.as_ref().unwrap().as_str(),
        "very long role description that must show only as preview"
    );
    let human_s = summaries
        .iter()
        .find(|s| s.participant.id == ParticipantId(WORLD_HUMAN.to_owned()))
        .expect("human owner is a member");
    assert_eq!(human_s.participant.kind, ParticipantKind::Human);
    assert!(human_s.short_description.is_none(), "{human_s:?}");
    assert!(human_s.room_display_name.is_none(), "{human_s:?}");
    assert!(human_s.room_role_name.is_none(), "{human_s:?}");
    assert!(human_s.room_role_description.is_none(), "{human_s:?}");
}

#[tokio::test]
async fn empty_strings_normalize_to_null_and_whitespace_preserved() {
    let f = fixture().await;
    let agent = ParticipantId(WORLD_AGENT.to_owned());
    // 空串归一为 NULL（与 null 是同一事实）。
    let d = profile_put_digest(BaseVersion::ZERO, Some(""), Some("x"));
    let outcome = f
        .store
        .put_profile_version(
            &key("k-empty"),
            &d,
            &BaseVersion::ZERO,
            &profile_record(agent.clone(), Some(""), Some("x")),
            UtcMillis::from_millis(10),
        )
        .await
        .expect("empty short normalizes to null");
    assert!(outcome.result.short_description.is_none());
    assert_eq!(
        outcome.result.public_description.as_ref().unwrap().as_str(),
        "x"
    );

    // 纯空白非空串原样保存不 trim（版本 2）。
    let d2 = profile_put_digest(BaseVersion::parse(1).unwrap(), Some("  "), None);
    let outcome = f
        .store
        .put_profile_version(
            &key("k-ws"),
            &d2,
            &BaseVersion::parse(1).unwrap(),
            &profile_record(agent, Some("  "), None),
            UtcMillis::from_millis(20),
        )
        .await
        .expect("whitespace preserved");
    assert_eq!(
        outcome.result.short_description.as_ref().unwrap().as_str(),
        "  "
    );
}

#[tokio::test]
async fn database_failure_maps_to_store_database_variant() {
    let f = fixture().await;
    // 关闭底层连接池让所有操作失败（约束/unavailable 都可接受，只要落入
    // StoreError::Database 并保持 kind 分类）。
    f.pool.close().await;
    let agent = ParticipantId(WORLD_AGENT.to_owned());
    let d = profile_put_digest(BaseVersion::ZERO, None, None);
    let err = f
        .store
        .put_profile_version(
            &key("k-db"),
            &d,
            &BaseVersion::ZERO,
            &profile_record(agent, None, None),
            UtcMillis::from_millis(1),
        )
        .await
        .expect_err("closed pool must fail");
    match err {
        StoreError::Database(storage) => {
            assert_ne!(storage.kind(), StorageFailureKind::Unknown);
        }
        other => panic!("expected Database variant, got {other:?}"),
    }
}
