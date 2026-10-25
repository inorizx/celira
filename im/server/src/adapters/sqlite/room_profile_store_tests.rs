//! Group Room Profile SQLite adapter 契约测试。
//!
//! 在真实临时 SQLite + 全部 migration 上证明窄 adapter 的单个
//! `BEGIN IMMEDIATE` 事务语义（CORE_V1_ROOM_PROFILE.md §5–§6）：授权先于
//! 重放、同 key 同摘要恒重放（即使 head 已推进）、同 key 不同摘要冲突、
//! 新 key stale、写后读回、失败回滚不留半成品、group-only、owner 转让后
//! 新 owner 可写、房间头投影。种子世界用直接 SQL（与 migration 测试同一
//! 闭合 group 世界），只通过端口调用 adapter 做被测操作。

use sqlx::SqlitePool;

use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::adapters::sqlite::participant_presentation_store::SqliteParticipantPresentationStore;
use crate::adapters::sqlite::test_relations::seed_accepted_relation;
use crate::domain::agent::IdempotencyKey;
use crate::domain::participant::ParticipantId;
use crate::domain::participant_presentation::BaseVersion;
use crate::domain::room::{RoomId, RoomKind};
use crate::domain::room_profile::{
    RoomDescription, RoomProfileVersion, RoomProfileVersionRow, room_profile_put_digest,
};
use crate::domain::time::UtcMillis;
use crate::ports::participant_presentation::{ParticipantPresentationStore as _, StoreError};

const OWNER: &str = "11111111-1111-4111-8111-111111111111";
const MEMBER: &str = "22222222-2222-4222-8222-222222222222";
const OUTSIDER: &str = "33333333-3333-4333-8333-333333333333";
const AGENT: &str = "44444444-4444-4444-8444-444444444444";
const GROUP_ROOM: &str = "55555555-5555-4555-8555-555555555555";
const OWNER_MEMBERSHIP: &str = "66666666-6666-4666-8666-666666666666";
const MEMBER_MEMBERSHIP: &str = "77777777-7777-4777-8777-777777777777";
const AGENT_MEMBERSHIP: &str = "88888888-8888-4888-8888-888888888888";

struct Fixture {
    _dir: tempfile::TempDir,
    pool: SqlitePool,
    store: SqliteParticipantPresentationStore,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let migrated = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("open + migrate");
    let pool = migrated.pool().clone();
    let store = SqliteParticipantPresentationStore::new(pool.clone());
    seed_world(&pool).await;
    Fixture {
        _dir: dir,
        pool,
        store,
    }
}

async fn seed_world(pool: &SqlitePool) {
    let mut tx = pool.begin().await.unwrap();
    for (id, kind, name) in [
        (OWNER, "human", "Owner"),
        (MEMBER, "human", "Member"),
        (OUTSIDER, "human", "Outsider"),
        (AGENT, "agent", "Agent"),
    ] {
        sqlx::query(
            "INSERT INTO participants (id, kind, status, display_name, created_at) \
             VALUES (?, ?, 'active', ?, 1)",
        )
        .bind(id)
        .bind(kind)
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
    .bind(OWNER)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO rooms (id, kind, status, created_at, name) VALUES (?, 'group', 'active', 1, 'g')")
        .bind(GROUP_ROOM)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO room_coverage \
         (room_id, kind, created_at, group_owner_membership_id, group_owner_role, group_owner_active_marker) \
         VALUES (?, 'group', 1, ?, 'owner', 1)",
    )
    .bind(GROUP_ROOM)
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
    .bind(GROUP_ROOM)
    .bind(OWNER)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    seed_accepted_relation(pool, OWNER, MEMBER).await;
    for (membership, participant) in [(MEMBER_MEMBERSHIP, MEMBER), (AGENT_MEMBERSHIP, AGENT)] {
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

fn key(raw: &str) -> IdempotencyKey {
    IdempotencyKey::parse(raw).unwrap()
}

fn record(
    room: &RoomId,
    version: u64,
    description: &str,
    by: &str,
    at: i64,
) -> RoomProfileVersionRow {
    RoomProfileVersionRow {
        room_id: room.clone(),
        version: RoomProfileVersion::from_u64(version).unwrap(),
        description: RoomDescription::parse(description).unwrap(),
        updated_by: ParticipantId(by.to_owned()),
        updated_at: UtcMillis::from_millis(at),
    }
}

fn base(v: i64) -> BaseVersion {
    BaseVersion::parse(v).unwrap()
}

fn digest(
    b: BaseVersion,
    description: &str,
) -> crate::domain::participant_presentation::RequestDigest {
    room_profile_put_digest(b, description)
}

async fn put(
    f: &Fixture,
    k: &str,
    b: BaseVersion,
    description: &str,
    at: i64,
) -> Result<crate::ports::participant_presentation::RoomProfilePutOutcome, StoreError> {
    let room = RoomId(GROUP_ROOM.to_owned());
    let rec = record(&room, b.as_u64() + 1, description, OWNER, at);
    f.store
        .put_room_profile_version(
            &key(k),
            &digest(b, description),
            &b,
            &rec,
            UtcMillis::from_millis(at),
        )
        .await
}

async fn version_count(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM room_profile_versions")
        .fetch_one(&f.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn put_and_read_roundtrip_with_header() {
    let f = fixture().await;
    let room = RoomId(GROUP_ROOM.to_owned());

    let outcome = put(&f, "k1", base(0), "第一行\n第二行 🙂", 100)
        .await
        .expect("first write");
    assert!(outcome.created);
    assert_eq!(outcome.result.version.as_u64(), 1);
    assert_eq!(outcome.result.description.as_str(), "第一行\n第二行 🙂");
    assert_eq!(outcome.result.updated_at.millis(), 100);

    let latest = f.store.latest_room_profile(&room).await.unwrap().unwrap();
    assert_eq!(latest.version.as_u64(), 1);
    assert_eq!(latest.description.as_str(), "第一行\n第二行 🙂");

    let header = f.store.room_profile_header(&room).await.unwrap().unwrap();
    assert_eq!(header.room_id, room);
    assert_eq!(header.kind, RoomKind::Group);
    assert_eq!(header.name.as_deref(), Some("g"));
    assert_eq!(
        header.profile.unwrap().description.as_str(),
        "第一行\n第二行 🙂"
    );
}

#[tokio::test]
async fn versions_increment_and_replay_survives_an_advanced_head() {
    let f = fixture().await;
    let room = RoomId(GROUP_ROOM.to_owned());

    let first = put(&f, "k1", base(0), "v1", 100).await.expect("v1");
    assert!(first.created);
    let second = put(&f, "k2", base(1), "v2", 200).await.expect("v2");
    assert_eq!(second.result.version.as_u64(), 2);

    // 同 key 同摘要重放：返回首次结果（v1），不因 head 已是 v2 变 stale，
    // 也不追加版本。
    let replay = put(&f, "k1", base(0), "v1", 300).await.expect("replay");
    assert!(!replay.created);
    assert_eq!(replay.result.version.as_u64(), 1);
    assert_eq!(replay.result.description.as_str(), "v1");
    assert_eq!(version_count(&f).await, 2);

    let latest = f.store.latest_room_profile(&room).await.unwrap().unwrap();
    assert_eq!(latest.version.as_u64(), 2);
}

#[tokio::test]
async fn same_key_with_different_request_conflicts() {
    let f = fixture().await;
    put(&f, "k1", base(0), "v1", 100).await.expect("v1");
    let err = put(&f, "k1", base(0), "v1 改过", 200)
        .await
        .expect_err("different digest");
    assert!(matches!(err, StoreError::IdempotencyConflict), "{err:?}");
    assert_eq!(version_count(&f).await, 1);
}

#[tokio::test]
async fn stale_base_version_conflicts_and_leaves_no_rows() {
    let f = fixture().await;
    put(&f, "k1", base(0), "v1", 100).await.expect("v1");
    let err = put(&f, "k2", base(0), "v2", 200).await.expect_err("stale");
    assert!(
        matches!(err, StoreError::ConcurrentVersionChanged),
        "{err:?}"
    );
    assert_eq!(version_count(&f).await, 1);
    let idempotency: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM room_profile_put_idempotency")
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!(idempotency, 1, "失败的新 key 不留半条幂等记录");
}

#[tokio::test]
async fn only_the_active_human_owner_can_write() {
    let f = fixture().await;
    let room = RoomId(GROUP_ROOM.to_owned());
    let b = base(0);

    for (actor, expected) in [
        (MEMBER, StoreError::NotRoomOwner),
        (OUTSIDER, StoreError::NotRoomMember),
        (AGENT, StoreError::NotRoomOwner),
    ] {
        let rec = record(&room, 1, "写的", actor, 100);
        let err = f
            .store
            .put_room_profile_version(
                &key("k-any"),
                &digest(b, "写的"),
                &b,
                &rec,
                UtcMillis::from_millis(100),
            )
            .await
            .expect_err("unauthorized actor");
        assert!(
            std::mem::discriminant(&err) == std::mem::discriminant(&expected),
            "actor {actor}: {err:?} (want {expected:?})"
        );
    }

    let unknown_room = RoomId("99999999-9999-4999-8999-999999999999".to_owned());
    let rec = record(&unknown_room, 1, "写的", OWNER, 100);
    let err = f
        .store
        .put_room_profile_version(
            &key("k-unknown"),
            &digest(b, "写的"),
            &b,
            &rec,
            UtcMillis::from_millis(100),
        )
        .await
        .expect_err("unknown room");
    assert!(matches!(err, StoreError::UnknownRoom), "{err:?}");

    assert_eq!(version_count(&f).await, 0, "失败路径零落库");
}

#[tokio::test]
async fn direct_room_is_not_supported_and_non_members_stay_hidden() {
    let f = fixture().await;
    // 一间 direct 房（owner = OWNER_HUMAN，member = AGENT）；用最小闭合结构。
    let direct_room = RoomId("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_owned());
    let human_membership = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    let agent_membership = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
    {
        let mut tx = f.pool.begin().await.unwrap();
        sqlx::query("INSERT INTO rooms (id, kind, status, created_at, name) VALUES (?, 'direct', 'active', 1, NULL)")
            .bind(direct_room.as_str())
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO room_coverage (room_id, kind, created_at, direct_human_id, direct_agent_id) VALUES (?, 'direct', 1, ?, ?)")
            .bind(direct_room.as_str())
            .bind(OWNER)
            .bind(AGENT)
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("INSERT INTO direct_room_pairs (room_id, human_id, agent_id, human_membership_id, agent_membership_id, created_at) VALUES (?, ?, ?, ?, ?, 1)")
            .bind(direct_room.as_str())
            .bind(OWNER)
            .bind(AGENT)
            .bind(human_membership)
            .bind(agent_membership)
            .execute(&mut *tx)
            .await
            .unwrap();
        for (membership, participant, role) in [
            (human_membership, OWNER, "owner"),
            (agent_membership, AGENT, "member"),
        ] {
            sqlx::query(
                "INSERT INTO room_memberships \
                 (membership_id, room_id, participant_id, role, joined_at, join_after_sequence, active_marker) \
                 VALUES (?, ?, ?, ?, 1, 0, 1)",
            )
            .bind(membership)
            .bind(direct_room.as_str())
            .bind(participant)
            .bind(role)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
    }

    let b = base(0);
    let rec = record(&direct_room, 1, "私聊简介", OWNER, 100);
    let err = f
        .store
        .put_room_profile_version(
            &key("k-direct"),
            &digest(b, "私聊简介"),
            &b,
            &rec,
            UtcMillis::from_millis(100),
        )
        .await
        .expect_err("direct room");
    assert!(matches!(err, StoreError::NotGroupRoom), "{err:?}");

    // 非成员对同一间 direct 房：必须先撞「不是成员」（404），不能靠 400 探存在性。
    let rec = record(&direct_room, 1, "陌生人", OUTSIDER, 100);
    let err = f
        .store
        .put_room_profile_version(
            &key("k-outsider"),
            &digest(b, "陌生人"),
            &b,
            &rec,
            UtcMillis::from_millis(100),
        )
        .await
        .expect_err("outsider");
    assert!(matches!(err, StoreError::NotRoomMember), "{err:?}");

    assert_eq!(version_count(&f).await, 0);
    let header = f
        .store
        .room_profile_header(&direct_room)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(header.kind, RoomKind::Direct);
    assert!(header.profile.is_none(), "direct 房永不带简介");
}

#[tokio::test]
async fn header_is_null_without_profile_and_for_unknown_room() {
    let f = fixture().await;
    let room = RoomId(GROUP_ROOM.to_owned());
    let header = f.store.room_profile_header(&room).await.unwrap().unwrap();
    assert_eq!(header.kind, RoomKind::Group);
    assert_eq!(header.name.as_deref(), Some("g"));
    assert!(header.profile.is_none());
    assert!(f.store.latest_room_profile(&room).await.unwrap().is_none());

    let unknown = RoomId("99999999-9999-4999-8999-999999999999".to_owned());
    assert!(
        f.store
            .room_profile_header(&unknown)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        f.store
            .latest_room_profile(&unknown)
            .await
            .unwrap()
            .is_none()
    );
}
