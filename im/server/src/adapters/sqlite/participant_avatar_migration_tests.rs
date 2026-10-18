//! Participant Avatar migration 完整性测试（CORE_V1_AVATAR_P0.md §6）。
//!
//! 在真实临时 SQLite 上应用全部 migration（含本切片 202609110001），用直接
//! SQL 证明头像字段族的 schema 守卫：avatar 列追加存在、文本 Profile 版本行
//! 不受影响（avatar 全 NULL）、全部或全无触发器、content_type 白名单、
//! size_bytes 1..=2 MiB、sha256_hex 64 位小写 hex、storage_key 严格 UUID v4；
//! 既有版本链守卫（单调/不可变/身份 REPLACE 防线）对头像版本行同样生效。
//! 幂等表复用既有 participant_profile_put_idempotency，形状不变。

use crate::adapters::sqlite::migration_store::MigrationStore;
use crate::ports::migration::MigrationProbe;
use sqlx::SqlitePool;

const WORLD_HUMAN: &str = "11111111-1111-4111-8111-111111111111";
const WORLD_AGENT: &str = "22222222-2222-4222-8222-222222222222";
const OWNER_MEMBERSHIP: &str = "33333333-3333-4333-8333-333333333333";

async fn open() -> (tempfile::TempDir, MigrationStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = MigrationStore::open(&dir.path().join("core.db"))
        .await
        .expect("open + migrate on a fresh DB");
    (dir, store)
}

/// 最小闭合世界：active Human owner + active Agent + active Owner 关系
/// （头像写路径只对 active Agent Owner，DB 兜底由既有
/// participant_profile_versions_target_owned_agent 触发器裁决）。
async fn seed_world(pool: &SqlitePool) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO participants (id, kind, status, display_name, created_at) VALUES (?, 'human', 'active', 'Owner', 1)")
        .bind(WORLD_HUMAN)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO participants (id, kind, status, display_name, created_at) VALUES (?, 'agent', 'active', 'Agent', 1)")
        .bind(WORLD_AGENT)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO agent_owners (agent_id, human_id, status, created_at) VALUES (?, ?, 'active', 1)")
        .bind(WORLD_AGENT)
        .bind(WORLD_HUMAN)
        .execute(&mut *tx)
        .await
        .unwrap();
    let _ = OWNER_MEMBERSHIP;
    tx.commit().await.unwrap();
}

async fn columns(pool: &SqlitePool, table: &str) -> Vec<String> {
    sqlx::query_scalar::<_, String>(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .fetch_all(pool)
        .await
        .unwrap()
}

/// 合法头像版本行的 INSERT 语句骨架（全部字段 present）。
const AVATAR_INSERT: &str = "INSERT INTO participant_profile_versions \
     (participant_id, profile_version, short_description, public_description, \
      avatar_storage_key, avatar_content_type, avatar_size_bytes, avatar_sha256_hex, avatar_updated_at, \
      published_by_id, published_at) \
     VALUES (?, ?, NULL, NULL, ?, ?, ?, ?, ?, ?, ?)";

async fn insert_avatar_version(
    pool: &SqlitePool,
    version: i64,
    avatar: AvatarInsert,
) -> Result<(), sqlx::Error> {
    sqlx::query(AVATAR_INSERT)
        .bind(WORLD_AGENT)
        .bind(version)
        .bind(avatar.storage_key)
        .bind(avatar.content_type)
        .bind(avatar.size_bytes)
        .bind(avatar.sha256_hex)
        .bind(avatar.updated_at)
        .bind(WORLD_HUMAN)
        .bind(version * 10)
        .execute(pool)
        .await
        .map(|_| ())
}

struct AvatarInsert {
    storage_key: &'static str,
    content_type: &'static str,
    size_bytes: i64,
    sha256_hex: String,
    updated_at: i64,
}

fn valid_avatar() -> AvatarInsert {
    AvatarInsert {
        storage_key: "550e8400-e29b-41d4-a716-446655440000",
        content_type: "image/png",
        size_bytes: 1024,
        sha256_hex: "a".repeat(64),
        updated_at: 100,
    }
}

#[tokio::test]
async fn migration_applies_and_avatar_columns_are_additive() {
    let (_dir, store) = open().await;
    let status = store.status().await.expect("probe");
    assert_eq!(
        status,
        crate::ports::migration::MigrationStatus {
            pending: 0,
            failed: 0
        }
    );
    let cols = columns(store.pool(), "participant_profile_versions").await;
    for expected in [
        "avatar_storage_key",
        "avatar_content_type",
        "avatar_size_bytes",
        "avatar_sha256_hex",
        "avatar_updated_at",
    ] {
        assert!(
            cols.iter().any(|c| c == expected),
            "missing column {expected}"
        );
    }
    // 既有列原样保留（追加式）。
    for existing in [
        "participant_id",
        "profile_version",
        "short_description",
        "public_description",
        "published_by_id",
        "published_at",
    ] {
        assert!(
            cols.iter().any(|c| c == existing),
            "existing column {existing} must stay"
        );
    }
    // 幂等表形状不变（复用既有表族）。
    let idem_cols = columns(store.pool(), "participant_profile_put_idempotency").await;
    assert!(
        !idem_cols.iter().any(|c| c.contains("avatar")),
        "idempotency table must not gain avatar columns"
    );
}

#[tokio::test]
async fn text_only_profile_versions_still_insert_with_avatar_all_null() {
    let (_dir, store) = open().await;
    seed_world(store.pool()).await;
    sqlx::query(
        "INSERT INTO participant_profile_versions \
         (participant_id, profile_version, short_description, public_description, published_by_id, published_at) \
         VALUES (?, 1, 'short', NULL, ?, 5)",
    )
    .bind(WORLD_AGENT)
    .bind(WORLD_HUMAN)
    .execute(store.pool())
    .await
    .expect("text-only version 1 must insert with avatar all NULL");
    type AvatarCols = (
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<i64>,
    );
    let (key, ct, size, sha, at): AvatarCols = sqlx::query_as(
        "SELECT avatar_storage_key, avatar_content_type, avatar_size_bytes, avatar_sha256_hex, avatar_updated_at \
         FROM participant_profile_versions WHERE participant_id = ? AND profile_version = 1",
    )
    .bind(WORLD_AGENT)
    .fetch_one(store.pool())
    .await
    .unwrap();
    assert!(key.is_none() && ct.is_none() && size.is_none() && sha.is_none() && at.is_none());
}

#[tokio::test]
async fn avatar_versions_require_all_or_nothing_fields() {
    let (_dir, store) = open().await;
    seed_world(store.pool()).await;

    // 合法：全部 present。
    insert_avatar_version(store.pool(), 1, valid_avatar())
        .await
        .expect("all-present avatar version must insert");

    // 部分 present（storage_key 有、其余无）→ 全部或全无触发器拒绝。
    let err = sqlx::query(AVATAR_INSERT)
        .bind(WORLD_AGENT)
        .bind(2_i64)
        .bind(Some("550e8400-e29b-41d4-a716-446655440001"))
        .bind(Option::<&str>::None)
        .bind(Option::<i64>::None)
        .bind(Option::<String>::None)
        .bind(Option::<i64>::None)
        .bind(WORLD_HUMAN)
        .bind(20_i64)
        .execute(store.pool())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("all present or all absent"),
        "{err}"
    );

    // 合法头像 v2（推进版本链；单调触发器与头像字段一致）。
    insert_avatar_version(store.pool(), 2, valid_avatar())
        .await
        .expect("v2 must insert after failed partial attempt");
}

#[tokio::test]
async fn avatar_content_type_is_whitelisted() {
    let (_dir, store) = open().await;
    seed_world(store.pool()).await;
    for (v, ct) in [(1, "image/jpeg"), (2, "image/webp")] {
        let mut a = valid_avatar();
        a.content_type = ct;
        insert_avatar_version(store.pool(), v, a)
            .await
            .expect("whitelisted type passes");
    }
    // 拒绝不推进版本：全部用 v3（= 最新 2 + 1，让版本触发器放行、CHECK 先裁决）。
    for ct in [
        "image/svg+xml",
        "image/gif",
        "text/html",
        "application/octet-stream",
        "image/png;charset=utf-8",
    ] {
        let mut a = valid_avatar();
        a.content_type = ct;
        let err = insert_avatar_version(store.pool(), 3, a).await.unwrap_err();
        assert!(err.to_string().contains("CHECK"), "{ct}: {err}");
    }
}

#[tokio::test]
async fn avatar_size_sha256_and_updated_at_are_bounded() {
    let (_dir, store) = open().await;
    seed_world(store.pool()).await;

    // size 上限 2 MiB：2 MiB 通过、2 MiB + 1 拒绝、0 拒绝。
    let mut a = valid_avatar();
    a.size_bytes = 2 * 1024 * 1024;
    insert_avatar_version(store.pool(), 1, a)
        .await
        .expect("2 MiB passes");
    let mut a = valid_avatar();
    a.size_bytes = 2 * 1024 * 1024 + 1;
    let err = insert_avatar_version(store.pool(), 2, a).await.unwrap_err();
    assert!(err.to_string().contains("CHECK"), "size over 2 MiB: {err}");
    let mut a = valid_avatar();
    a.size_bytes = 0;
    let err = insert_avatar_version(store.pool(), 2, a).await.unwrap_err();
    assert!(err.to_string().contains("CHECK"), "zero size: {err}");

    // sha256 必须 64 位小写 hex。
    let mut a = valid_avatar();
    a.sha256_hex = "short".to_owned();
    let err = insert_avatar_version(store.pool(), 2, a).await.unwrap_err();
    assert!(err.to_string().contains("CHECK"), "short sha256: {err}");
    let mut a = valid_avatar();
    a.sha256_hex = "A".repeat(64);
    let err = insert_avatar_version(store.pool(), 2, a).await.unwrap_err();
    assert!(err.to_string().contains("CHECK"), "uppercase sha256: {err}");

    // updated_at 必须 > 0。
    let mut a = valid_avatar();
    a.updated_at = 0;
    let err = insert_avatar_version(store.pool(), 2, a).await.unwrap_err();
    assert!(err.to_string().contains("CHECK"), "zero updated_at: {err}");
}

#[tokio::test]
async fn avatar_storage_key_must_be_strict_uuid_v4() {
    let (_dir, store) = open().await;
    seed_world(store.pool()).await;
    for key in [
        "not-a-uuid",
        "../etc/passwd",
        "550e8400-e29b-31d4-a716-446655440000",
        "550e8400-e29b-41d4-6716-446655440000",
        "550e8400e29b41d4a716446655440000",
    ] {
        let mut a = valid_avatar();
        a.storage_key = key;
        // 拒绝不推进版本：全部用 v1。
        let err = insert_avatar_version(store.pool(), 1, a).await.unwrap_err();
        assert!(err.to_string().contains("CHECK"), "{key}: {err}");
    }
    // 合法 UUID v4 通过。
    let mut a = valid_avatar();
    a.storage_key = "550e8400-e29b-41d4-b716-446655440000";
    insert_avatar_version(store.pool(), 1, a)
        .await
        .expect("canonical v4 passes");
}

#[tokio::test]
async fn avatar_versions_share_existing_chain_guards() {
    let (_dir, store) = open().await;
    seed_world(store.pool()).await;
    insert_avatar_version(store.pool(), 1, valid_avatar())
        .await
        .expect("v1");

    // 跳号（直接 v3）拒绝：单调触发器对头像版本行同样生效。
    let mut a = valid_avatar();
    a.storage_key = "550e8400-e29b-41d4-a716-446655440002";
    let err = insert_avatar_version(store.pool(), 3, a).await.unwrap_err();
    assert!(err.to_string().contains("exactly one more"), "{err}");

    // 不可变：头像字段一旦提交不可 UPDATE。
    let upd = sqlx::query(
        "UPDATE participant_profile_versions SET avatar_content_type = 'image/jpeg' \
         WHERE participant_id = ? AND profile_version = 1",
    )
    .bind(WORLD_AGENT)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(upd.to_string().contains("immutable"), "{upd}");

    // REPLACE 不能重建头像版本行（identity guard 或单调触发器任一先裁决）。
    let err = sqlx::query(
        "INSERT OR REPLACE INTO participant_profile_versions \
         (participant_id, profile_version, short_description, public_description, \
          avatar_storage_key, avatar_content_type, avatar_size_bytes, avatar_sha256_hex, avatar_updated_at, \
          published_by_id, published_at) \
         VALUES (?, 1, NULL, NULL, ?, 'image/png', 1024, ?, 999, ?, 1)",
    )
    .bind(WORLD_AGENT)
    .bind("550e8400-e29b-41d4-a716-446655440003")
    .bind("b".repeat(64))
    .bind(WORLD_HUMAN)
    .execute(store.pool())
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("identity already exists")
            || err.to_string().contains("exactly one more"),
        "{err}"
    );

    // 非 Owner 发布者（无关系第三方）拒绝：既有方向触发器对头像版本行生效。
    sqlx::query("INSERT INTO participants (id, kind, status, display_name, created_at) VALUES ('human-stranger', 'human', 'active', 'S', 1)")
        .execute(store.pool())
        .await
        .unwrap();
    let err = sqlx::query(AVATAR_INSERT)
        .bind(WORLD_AGENT)
        .bind(2_i64)
        .bind("550e8400-e29b-41d4-a716-446655440004")
        .bind("image/png")
        .bind(1024_i64)
        .bind("a".repeat(64))
        .bind(200_i64)
        .bind("human-stranger")
        .bind(20_i64)
        .execute(store.pool())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("owned by the publisher"), "{err}");
}
