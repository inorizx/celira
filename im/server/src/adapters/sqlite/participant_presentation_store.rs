//! SQLite Participant Presentation adapter：实现
//! `ports::participant_presentation::ParticipantPresentationStore`。
//! 版本链事实三类：Participant Profile、Group Room Presentation 与（v1.1
//! 附加切片）Group Room Profile 群简介；三者共用同一套「不可变版本 + 幂等
//! 见证 + 单事务授权先于重放」的形状。
//!
//! 本模块是 Profile / Group Room Presentation / Group Room Profile 公共事实
//! 唯一允许接触 SQLite 文件的地方，独立于 generic repository 之外的窄
//! adapter。每一次 `put_*` 都在**单个 `BEGIN IMMEDIATE` 事务**内完成：授权
//! （active Owner / active group owner + active Agent member / 该房当前
//! active owner（Human 或 Agent），**先于任何重放裁决**）→ 读 scope + key 幂等记录
//! 与 digest → 同 key 同摘要恒重放既有结果（必要时读回历史版本行，即使当前
//! 最新版本已被其他写入推进也不重新裁决 base_version）→ 同 key 不同摘要
//! `IdempotencyConflict` → 新 key 要求 `base_version == 当前最新` → 插入
//! 不可变版本行与幂等见证 → commit；任何失败整体回滚，不留半个版本或半条
//! 幂等记录。幂等表只存 SHA-256 请求摘要与结果版本，绝不携带正文/描述/预览。
//!
//! 契约：CORE_V1_PARTICIPANT_PRESENTATION.md §4–§6、
//! CORE_V1_ROOM_PROFILE.md §3–§6。数据库触发器/CHECK 只证明引用/身份一致性
//! 与世界卫生，不见证同事务原子性；同事务原子性由本 adapter 的单个
//! `BEGIN IMMEDIATE` 保证。

use sqlx::Connection;
use sqlx::sqlite::{SqliteConnection, SqlitePool};

use crate::adapters::sqlite::{finish_rollback, rollback_after_primary};
use crate::domain::agent::IdempotencyKey;
use crate::domain::attachment::StorageKey;
use crate::domain::participant::{ParticipantId, ParticipantKind};
use crate::domain::participant_avatar::{Avatar, AvatarContentType, AvatarMeta};
use crate::domain::participant_presentation::{
    BaseVersion, ParticipantProfileVersion, PresentationVersion, ProfileVersion, PublicDescription,
    RequestDigest, RoomDisplayName, RoomPresentationVersion, RoomRoleDescription, RoomRoleName,
    ShortDescription,
};
use crate::domain::room::RoomId;
use crate::domain::room_profile::{RoomDescription, RoomProfileVersion, RoomProfileVersionRow};
use crate::domain::time::UtcMillis;
use crate::ports::participant_presentation::{
    LatestAvatar, MemberPresentationSummary, ParticipantPresentationStore, ProfilePutOutcome,
    ProfileRead, RoomPresentationPutOutcome, RoomPresentationRead, RoomProfileHeader,
    RoomProfilePutOutcome, RoomProfileRead, StoreError,
};
use crate::ports::room::ParticipantProfile;
use crate::ports::storage::{StorageError, StorageFailureKind};

const SQL_BEGIN_IMMEDIATE: &str = "BEGIN IMMEDIATE";

/// 基于同一 SqlitePool 的 Presentation 存储。
#[derive(Clone)]
pub struct SqliteParticipantPresentationStore {
    pool: SqlitePool,
}

impl SqliteParticipantPresentationStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn db_error(e: sqlx::Error) -> StoreError {
    StoreError::Database(StorageError::from(e))
}

/// primary 变体 → composite kind 裁决（CORE_V1_ERROR_HANDLING.md §7.2）：
/// `Database(StorageError)` 沿用嵌套 kind；业务变体归 `Unknown`。
fn primary_storage_kind(e: &StoreError) -> StorageFailureKind {
    match e {
        StoreError::Database(storage) => storage.kind(),
        _ => StorageFailureKind::Unknown,
    }
}

/// 持久化行/枚举解析失败：归 `decode_or_corrupt`（对外 `internal_error`）。
fn corrupt(message: impl std::fmt::Display) -> StoreError {
    StoreError::Database(StorageError::from_message(
        StorageFailureKind::DecodeOrCorrupt,
        message.to_string(),
    ))
}

/// 空串归一化为 `None`：`NULL` 与 `""` 是同一「未设置」事实，`""` 绝不落库
/// （DB CHECK `<> ''` 同样拒绝）；纯空白非空串原样保留、不 trim。
/// application 层的 `parse_opt` 已在来源归一化，此处在 adapter 内再兜底一次，
/// 使直连 adapter 的调用方也不能绕过（CORE_V1_PARTICIPANT_PRESENTATION.md §5.3/§6.5）。
fn normalize_empty<T: Clone>(value: &Option<T>, as_str: fn(&T) -> &str) -> Option<T> {
    value.as_ref().filter(|v| !as_str(v).is_empty()).cloned()
}

/// 从版本行原始列解析 ProfileRead（含发布者 kind 派生投影与 avatar 公共
/// 投影——存在性 + content_type + size_bytes；绝不含 storage key / hash）。
type ProfileReadRow = (
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    String,
    String,
    i64,
);

fn profile_read_from_row(
    participant_id: ParticipantId,
    (version, short, public, avatar_ct, avatar_size, by, by_kind, at): ProfileReadRow,
) -> Result<ProfileRead, StoreError> {
    Ok(ProfileRead {
        participant_id,
        profile_version: ProfileVersion::parse(version)
            .map_err(|e| corrupt(format!("stored profile version invalid: {e}")))?,
        short_description: short
            .map(|s| {
                ShortDescription::parse(&s)
                    .map_err(|e| corrupt(format!("stored short description invalid: {e}")))
            })
            .transpose()?,
        public_description: public
            .map(|s| {
                PublicDescription::parse(&s)
                    .map_err(|e| corrupt(format!("stored public description invalid: {e}")))
            })
            .transpose()?,
        avatar: parse_avatar_meta(avatar_ct, avatar_size)?,
        published_by_id: ParticipantId(by),
        published_by_kind: ParticipantKind::parse(&by_kind)
            .ok_or_else(|| corrupt(format!("unknown publisher kind {by_kind:?}")))?,
        published_at: UtcMillis::from_millis(at),
    })
}

/// avatar 公共投影解析：全部或全无（migration 触发器保证持久层一致；此处
/// 对不一致按内部损坏 fail closed，绝不猜测）。content_type 必须过白名单。
fn parse_avatar_meta(
    content_type: Option<String>,
    size_bytes: Option<i64>,
) -> Result<Option<AvatarMeta>, StoreError> {
    match (content_type, size_bytes) {
        (Some(ct), Some(size)) => Ok(Some(AvatarMeta {
            content_type: AvatarContentType::parse(&ct)
                .map_err(|e| corrupt(format!("stored avatar content type invalid: {e}")))?,
            size_bytes: size,
        })),
        (None, None) => Ok(None),
        _ => Err(corrupt(
            "stored avatar fields must be all present or all absent",
        )),
    }
}

/// avatar 内部完整事实解析（版本行原始列，下载路径用）。全部或全无。
#[allow(clippy::type_complexity)]
fn parse_avatar_full(
    storage_key: Option<String>,
    content_type: Option<String>,
    size_bytes: Option<i64>,
    sha256_hex: Option<String>,
    updated_at: Option<i64>,
) -> Result<Option<Avatar>, StoreError> {
    match (
        storage_key,
        content_type,
        size_bytes,
        sha256_hex,
        updated_at,
    ) {
        (Some(key), Some(ct), Some(size), Some(sha256), Some(at)) => Ok(Some(Avatar {
            storage_key: StorageKey(key),
            content_type: AvatarContentType::parse(&ct)
                .map_err(|e| corrupt(format!("stored avatar content type invalid: {e}")))?,
            size_bytes: size,
            sha256_hex: sha256,
            updated_at: UtcMillis::from_millis(at),
        })),
        (None, None, None, None, None) => Ok(None),
        _ => Err(corrupt(
            "stored avatar fields must be all present or all absent",
        )),
    }
}

/// 从版本行原始列解析 RoomPresentationRead。
type PresentationReadRow = (
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
    i64,
);

fn presentation_read_from_row(
    room_id: RoomId,
    participant_id: ParticipantId,
    (version, display, role_name, role_desc, by, by_kind, at): PresentationReadRow,
) -> Result<RoomPresentationRead, StoreError> {
    Ok(RoomPresentationRead {
        room_id,
        participant_id,
        presentation_version: PresentationVersion::parse(version)
            .map_err(|e| corrupt(format!("stored presentation version invalid: {e}")))?,
        room_display_name: display
            .map(|s| {
                RoomDisplayName::parse(&s)
                    .map_err(|e| corrupt(format!("stored room display name invalid: {e}")))
            })
            .transpose()?,
        room_role_name: role_name
            .map(|s| {
                RoomRoleName::parse(&s)
                    .map_err(|e| corrupt(format!("stored room role name invalid: {e}")))
            })
            .transpose()?,
        room_role_description: role_desc
            .map(|s| {
                RoomRoleDescription::parse(&s)
                    .map_err(|e| corrupt(format!("stored room role description invalid: {e}")))
            })
            .transpose()?,
        published_by_id: ParticipantId(by),
        published_by_kind: ParticipantKind::parse(&by_kind)
            .ok_or_else(|| corrupt(format!("unknown publisher kind {by_kind:?}")))?,
        published_at: UtcMillis::from_millis(at),
    })
}

async fn fetch_latest_profile(
    conn: &mut SqliteConnection,
    participant_id: &ParticipantId,
) -> Result<Option<ProfileRead>, StoreError> {
    let row: Option<ProfileReadRow> = sqlx::query_as(
        "SELECT ppv.profile_version, ppv.short_description, ppv.public_description, \
                ppv.avatar_content_type, ppv.avatar_size_bytes, \
                ppv.published_by_id, p.kind, ppv.published_at \
         FROM participant_profile_versions ppv \
         JOIN participants p ON p.id = ppv.published_by_id \
         WHERE ppv.participant_id = ? \
         ORDER BY ppv.profile_version DESC LIMIT 1",
    )
    .bind(participant_id.as_str())
    .fetch_optional(conn)
    .await
    .map_err(db_error)?;
    row.map(|r| profile_read_from_row(participant_id.clone(), r))
        .transpose()
}

#[async_trait::async_trait]
impl ParticipantPresentationStore for SqliteParticipantPresentationStore {
    async fn put_profile_version(
        &self,
        key: &IdempotencyKey,
        request_digest: &RequestDigest,
        base_version: &BaseVersion,
        record: &ParticipantProfileVersion,
        now: UtcMillis,
    ) -> Result<ProfilePutOutcome, StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<ProfilePutOutcome, StoreError> = async {
            let short_description = normalize_empty(&record.short_description, ShortDescription::as_str);
            let public_description =
                normalize_empty(&record.public_description, PublicDescription::as_str);
            let target: Option<(String, String)> = sqlx::query_as(
                "SELECT kind, status FROM participants WHERE id = ?",
            )
            .bind(record.participant_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            let Some(target) = target else {
                return Err(StoreError::UnknownParticipant);
            };
            // 授权 = **归属 或 自己**（批次四表达权第一刀，2026-09-28）：自己的资料
            // 只有自己或替它担责的 Owner 能写——ownership 是责任关系，不是代笔权。
            // 形状与批次三第③步同一条：并集（Owner 的写权限一字不动，只多了"它自己"）。
            let owner_status: Option<String> = sqlx::query_scalar(
                "SELECT status FROM agent_owners \
                 WHERE agent_id = ? AND human_id = ? \
                 UNION ALL \
                 SELECT status FROM participants \
                 WHERE id = ? AND id = ? \
                 LIMIT 1",
            )
            .bind(record.participant_id.as_str())
            .bind(record.published_by_id.as_str())
            .bind(record.published_by_id.as_str())
            .bind(record.participant_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            let Some(owner_status) = owner_status else {
                return Err(StoreError::NotAgentOwner);
            };
            // 表达权第四刀（2026-10-17，主人批准）：**人也能写自己的头像** ——
            // 判据从"目标是不是一支 Agent"换成"这是不是**你自己的**资料"。
            // Agent 那一支照旧（自己或替它担责的 Owner，逐字不动）；Human 只允许自己
            // （人没有 Owner 概念 ⇒ 不带任何"替别人写"的分支）。
            // ⚠ 名字不走这里：`display_name` 是既有 Participant 身份事实，不在版本链上。
            let self_write = record.published_by_id == record.participant_id;
            let target_kind_ok = target.0 == "agent" || (target.0 == "human" && self_write);
            if owner_status == "revoked" || target.1 == "revoked" || !target_kind_ok {
                return Err(StoreError::ParticipantNotActive);
            }
            let existing: Option<(String, i64, bool)> = sqlx::query_as(
                "SELECT request_digest, result_profile_version, result_is_create \
                 FROM participant_profile_put_idempotency \
                 WHERE actor_human_id = ? AND participant_id = ? AND key = ?",
            )
            .bind(record.published_by_id.as_str())
            .bind(record.participant_id.as_str())
            .bind(key.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            let (existing_digest, result_version, result_is_create) = match existing {
                Some(e) => e,
                None => {
                    let latest: i64 = sqlx::query_scalar(
                        "SELECT COALESCE(MAX(profile_version), 0) \
                         FROM participant_profile_versions WHERE participant_id = ?",
                    )
                    .bind(record.participant_id.as_str())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(db_error)?;
                    if base_version.as_u64() != latest as u64 {
                        return Err(StoreError::ConcurrentVersionChanged);
                    }
                    let new_version = latest + 1;
                    let avatar = record.avatar.as_ref();
                    sqlx::query(
                        "INSERT INTO participant_profile_versions \
                         (participant_id, profile_version, short_description, public_description, \
                          avatar_storage_key, avatar_content_type, avatar_size_bytes, avatar_sha256_hex, avatar_updated_at, \
                          published_by_id, published_at) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(record.participant_id.as_str())
                    .bind(new_version)
                    .bind(short_description.as_ref().map(|s| s.as_str()))
                    .bind(public_description.as_ref().map(|s| s.as_str()))
                    .bind(avatar.map(|a| a.storage_key.as_str()))
                    .bind(avatar.map(|a| a.content_type.as_str()))
                    .bind(avatar.map(|a| a.size_bytes))
                    .bind(avatar.map(|a| a.sha256_hex.as_str()))
                    .bind(avatar.map(|a| a.updated_at.millis()))
                    .bind(record.published_by_id.as_str())
                    .bind(now.millis())
                    .execute(&mut *tx)
                    .await
                    .map_err(db_error)?;
                    sqlx::query(
                        "INSERT INTO participant_profile_put_idempotency \
                         (actor_human_id, participant_id, key, request_digest, result_profile_version, result_is_create, created_at) \
                         VALUES (?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(record.published_by_id.as_str())
                    .bind(record.participant_id.as_str())
                    .bind(key.as_str())
                    .bind(request_digest.as_str())
                    .bind(new_version)
                    .bind(new_version == 1)
                    .bind(now.millis())
                    .execute(&mut *tx)
                    .await
                    .map_err(db_error)?;
                    return Ok(ProfilePutOutcome {
                        created: true,
                        version: ProfileVersion::parse(new_version)
                            .map_err(|e| corrupt(format!("stored profile version invalid: {e}")))?,
                        result_is_create: new_version == 1,
                        result: ParticipantProfileVersion {
                            participant_id: record.participant_id.clone(),
                            profile_version: ProfileVersion::parse(new_version)
                                .map_err(|e| corrupt(format!("stored profile version invalid: {e}")))?,
                            short_description,
                            public_description,
                            avatar: record.avatar.clone(),
                            published_by_id: record.published_by_id.clone(),
                            published_at: now,
                        },
                    });
                }
            };
            if existing_digest != request_digest.as_str() {
                return Err(StoreError::IdempotencyConflict);
            }
            let Some(historical) = fetch_latest_version_row(&mut tx, &record.participant_id, result_version).await? else {
                return Err(corrupt("idempotency record points to missing profile version"));
            };
            let version = ProfileVersion::parse(result_version)
                .map_err(|e| corrupt(format!("stored profile version invalid: {e}")))?;
            Ok(ProfilePutOutcome {
                created: false,
                version,
                result_is_create,
                result: historical,
            })
        }
        .await;

        match outcome {
            Ok(outcome @ ProfilePutOutcome { created: true, .. }) => {
                tx.commit().await.map_err(db_error)?;
                Ok(outcome)
            }
            Ok(replayed) => {
                finish_rollback(tx).await.map_err(StoreError::Database)?;
                Ok(replayed)
            }
            Err(primary) => {
                let primary_kind = primary_storage_kind(&primary);
                Err(rollback_after_primary(tx, primary, primary_kind, StoreError::Database).await)
            }
        }
    }

    async fn put_room_presentation_version(
        &self,
        key: &IdempotencyKey,
        request_digest: &RequestDigest,
        base_version: &BaseVersion,
        record: &RoomPresentationVersion,
        now: UtcMillis,
    ) -> Result<RoomPresentationPutOutcome, StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<RoomPresentationPutOutcome, StoreError> = async {
            let room_display_name = normalize_empty(&record.room_display_name, RoomDisplayName::as_str);
            let room_role_name = normalize_empty(&record.room_role_name, RoomRoleName::as_str);
            let room_role_description =
                normalize_empty(&record.room_role_description, RoomRoleDescription::as_str);
            let room_kind: Option<String> = sqlx::query_scalar(
                "SELECT kind FROM rooms WHERE id = ?",
            )
            .bind(record.room_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            let Some(room_kind) = room_kind else {
                return Err(StoreError::UnknownRoom);
            };
            if room_kind != "group" {
                return Err(StoreError::NotGroupRoom);
            }
            // 授权 = **该房 active owner 或主体自己**（批次四表达权第三刀，2026-09-28）：
            // 房呈现是"主体在这一间房里的样子"——它自己那一份归它自己；owner 的写权限
            // 一字不动（且**不再限 kind**：批次三第一刀已把 owner 判据换成"创建者"，
            // 这里那处 `kind='human'` 是当时的遗留）。形状与前三刀同一条：**并集**。
            let is_owner: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM room_memberships o \
                 JOIN participants p ON p.id = o.participant_id \
                 WHERE o.room_id = ? AND o.role = 'owner' AND o.active_marker = 1 \
                   AND o.participant_id = ? AND p.status = 'active' \
                 UNION ALL \
                 SELECT 1 FROM participants p \
                 WHERE p.id = ? AND p.id = ? AND p.status = 'active' \
                 LIMIT 1",
            )
            .bind(record.room_id.as_str())
            .bind(record.published_by_id.as_str())
            .bind(record.published_by_id.as_str())
            .bind(record.participant_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            if is_owner.is_none() {
                return Err(StoreError::NotGroupOwner);
            }
            let is_target: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM room_memberships m \
                 JOIN participants p ON p.id = m.participant_id \
                 WHERE m.room_id = ? AND m.participant_id = ? AND m.role = 'member' \
                   AND m.active_marker = 1 AND p.kind = 'agent' AND p.status = 'active'",
            )
            .bind(record.room_id.as_str())
            .bind(record.participant_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            if is_target.is_none() {
                return Err(StoreError::NotActiveAgentMember);
            }
            let existing: Option<(String, i64, bool)> = sqlx::query_as(
                "SELECT request_digest, result_presentation_version, result_is_create \
                 FROM room_presentation_put_idempotency \
                 WHERE actor_human_id = ? AND room_id = ? AND participant_id = ? AND key = ?",
            )
            .bind(record.published_by_id.as_str())
            .bind(record.room_id.as_str())
            .bind(record.participant_id.as_str())
            .bind(key.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            let (existing_digest, result_version, result_is_create) = match existing {
                Some(e) => e,
                None => {
                    let latest: i64 = sqlx::query_scalar(
                        "SELECT COALESCE(MAX(presentation_version), 0) \
                         FROM room_presentation_versions WHERE room_id = ? AND participant_id = ?",
                    )
                    .bind(record.room_id.as_str())
                    .bind(record.participant_id.as_str())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(db_error)?;
                    if base_version.as_u64() != latest as u64 {
                        return Err(StoreError::ConcurrentVersionChanged);
                    }
                    let new_version = latest + 1;
                    sqlx::query(
                        "INSERT INTO room_presentation_versions \
                         (room_id, participant_id, presentation_version, room_display_name, room_role_name, room_role_description, published_by_id, published_at) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(record.room_id.as_str())
                    .bind(record.participant_id.as_str())
                    .bind(new_version)
                    .bind(room_display_name.as_ref().map(|s| s.as_str()))
                    .bind(room_role_name.as_ref().map(|s| s.as_str()))
                    .bind(room_role_description.as_ref().map(|s| s.as_str()))
                    .bind(record.published_by_id.as_str())
                    .bind(now.millis())
                    .execute(&mut *tx)
                    .await
                    .map_err(db_error)?;
                    sqlx::query(
                        "INSERT INTO room_presentation_put_idempotency \
                         (actor_human_id, room_id, participant_id, key, request_digest, result_presentation_version, result_is_create, created_at) \
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(record.published_by_id.as_str())
                    .bind(record.room_id.as_str())
                    .bind(record.participant_id.as_str())
                    .bind(key.as_str())
                    .bind(request_digest.as_str())
                    .bind(new_version)
                    .bind(new_version == 1)
                    .bind(now.millis())
                    .execute(&mut *tx)
                    .await
                    .map_err(db_error)?;
                    let version = PresentationVersion::parse(new_version)
                        .map_err(|e| corrupt(format!("stored presentation version invalid: {e}")))?;
                    return Ok(RoomPresentationPutOutcome {
                        created: true,
                        version,
                        result_is_create: new_version == 1,
                        result: RoomPresentationVersion {
                            room_id: record.room_id.clone(),
                            participant_id: record.participant_id.clone(),
                            presentation_version: version,
                            room_display_name,
                            room_role_name,
                            room_role_description,
                            published_by_id: record.published_by_id.clone(),
                            published_at: now,
                        },
                    });
                }
            };
            if existing_digest != request_digest.as_str() {
                return Err(StoreError::IdempotencyConflict);
            }
            let Some(historical) = fetch_latest_room_version_row(&mut tx, &record.room_id, &record.participant_id, result_version).await? else {
                return Err(corrupt("idempotency record points to missing presentation version"));
            };
            let version = PresentationVersion::parse(result_version)
                .map_err(|e| corrupt(format!("stored presentation version invalid: {e}")))?;
            Ok(RoomPresentationPutOutcome {
                created: false,
                version,
                result_is_create,
                result: historical,
            })
        }
        .await;

        match outcome {
            Ok(outcome @ RoomPresentationPutOutcome { created: true, .. }) => {
                tx.commit().await.map_err(db_error)?;
                Ok(outcome)
            }
            Ok(replayed) => {
                finish_rollback(tx).await.map_err(StoreError::Database)?;
                Ok(replayed)
            }
            Err(primary) => {
                let primary_kind = primary_storage_kind(&primary);
                Err(rollback_after_primary(tx, primary, primary_kind, StoreError::Database).await)
            }
        }
    }

    async fn put_room_profile_version(
        &self,
        key: &IdempotencyKey,
        request_digest: &RequestDigest,
        base_version: &BaseVersion,
        record: &RoomProfileVersionRow,
        now: UtcMillis,
    ) -> Result<RoomProfilePutOutcome, StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<RoomProfilePutOutcome, StoreError> = async {
            // ① Room 存在且 active；不存在 / 非 active 一律 UnknownRoom（404，
            // 不泄露存在性）。
            let room: Option<(String, String)> =
                sqlx::query_as("SELECT kind, status FROM rooms WHERE id = ?")
                    .bind(record.room_id.as_str())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db_error)?;
            let Some((room_kind, room_status)) = room else {
                return Err(StoreError::UnknownRoom);
            };
            if room_status != "active" {
                return Err(StoreError::UnknownRoom);
            }
            // ② actor 必须是该房当前 active 成员；不是成员 / 从未来过 / 已离开
            // 一律 NotRoomMember（路由出 404，与未知房同形）。这条必须排在
            // kind 之前：否则非成员能靠 400 探出一间 direct 房存在。
            let member: Option<(String, String)> = sqlx::query_as(
                "SELECT o.role, p.status \
                 FROM room_memberships o \
                 JOIN participants p ON p.id = o.participant_id \
                 WHERE o.room_id = ? AND o.participant_id = ? AND o.active_marker = 1",
            )
            .bind(record.room_id.as_str())
            .bind(record.updated_by.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            let Some((role, actor_status)) = member else {
                return Err(StoreError::NotRoomMember);
            };
            // ③ 只有 group 房有群简介（direct / human_direct → 400）。
            if room_kind != "group" {
                return Err(StoreError::NotGroupRoom);
            }
            // ④ 只有该房当前 active owner 可写。owner 是**房内的治理角色**，
            // 不是参与者 kind（2026-09-27 起 Agent 也能建房并持有 owner）：
            // 判据只看 role + active，不限 kind。仍要求写入者 active——
            // revoked 主体不得经重放或新写落库（DB 触发器同一判据兜底）。
            if role != "owner" || actor_status != "active" {
                return Err(StoreError::NotRoomOwner);
            }
            // ⑤ 幂等重放 / 冲突（授权已通过，重放不得绕过授权）。
            let existing: Option<(String, i64)> = sqlx::query_as(
                "SELECT request_digest, result_version FROM room_profile_put_idempotency \
                 WHERE actor_id = ? AND room_id = ? AND key = ?",
            )
            .bind(record.updated_by.as_str())
            .bind(record.room_id.as_str())
            .bind(key.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            let result_version = match existing {
                Some((existing_digest, result_version)) => {
                    if existing_digest != request_digest.as_str() {
                        return Err(StoreError::IdempotencyConflict);
                    }
                    result_version
                }
                None => {
                    // ⑥ 新 key 才裁决 base_version：必须等于当前最新（无行 = 0）。
                    let latest: i64 = sqlx::query_scalar(
                        "SELECT COALESCE(MAX(version), 0) FROM room_profile_versions \
                         WHERE room_id = ?",
                    )
                    .bind(record.room_id.as_str())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(db_error)?;
                    if base_version.as_u64() != latest as u64 {
                        return Err(StoreError::ConcurrentVersionChanged);
                    }
                    let new_version = latest + 1;
                    sqlx::query(
                        "INSERT INTO room_profile_versions \
                         (room_id, version, description, updated_by, updated_at) \
                         VALUES (?, ?, ?, ?, ?)",
                    )
                    .bind(record.room_id.as_str())
                    .bind(new_version)
                    .bind(record.description.as_str())
                    .bind(record.updated_by.as_str())
                    .bind(now.millis())
                    .execute(&mut *tx)
                    .await
                    .map_err(db_error)?;
                    sqlx::query(
                        "INSERT INTO room_profile_put_idempotency \
                         (actor_id, room_id, key, request_digest, result_version, created_at) \
                         VALUES (?, ?, ?, ?, ?, ?)",
                    )
                    .bind(record.updated_by.as_str())
                    .bind(record.room_id.as_str())
                    .bind(key.as_str())
                    .bind(request_digest.as_str())
                    .bind(new_version)
                    .bind(now.millis())
                    .execute(&mut *tx)
                    .await
                    .map_err(db_error)?;
                    let version = RoomProfileVersion::parse(new_version).map_err(|e| {
                        corrupt(format!("stored room profile version invalid: {e}"))
                    })?;
                    return Ok(RoomProfilePutOutcome {
                        created: true,
                        result: RoomProfileRead {
                            room_id: record.room_id.clone(),
                            version,
                            description: record.description.clone(),
                            updated_at: now,
                        },
                    });
                }
            };
            // ⑦ 重放：读回首次成功的版本行（不追加版本、不重新裁决 base_version）。
            let Some(historical) =
                fetch_room_profile_row(&mut tx, &record.room_id, result_version).await?
            else {
                return Err(corrupt(
                    "room profile idempotency record points to missing version",
                ));
            };
            Ok(RoomProfilePutOutcome {
                created: false,
                result: historical,
            })
        }
        .await;

        match outcome {
            Ok(outcome @ RoomProfilePutOutcome { created: true, .. }) => {
                tx.commit().await.map_err(db_error)?;
                Ok(outcome)
            }
            Ok(replayed) => {
                finish_rollback(tx).await.map_err(StoreError::Database)?;
                Ok(replayed)
            }
            Err(primary) => {
                let primary_kind = primary_storage_kind(&primary);
                Err(rollback_after_primary(tx, primary, primary_kind, StoreError::Database).await)
            }
        }
    }

    async fn latest_room_profile(
        &self,
        room_id: &RoomId,
    ) -> Result<Option<RoomProfileRead>, StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let row: Option<(i64, String, i64)> = sqlx::query_as(
            "SELECT version, description, updated_at FROM room_profile_versions \
             WHERE room_id = ? ORDER BY version DESC LIMIT 1",
        )
        .bind(room_id.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_error)?;
        row.map(|r| room_profile_read_from_row(room_id.clone(), r))
            .transpose()
    }

    async fn room_profile_header(
        &self,
        room_id: &RoomId,
    ) -> Result<Option<RoomProfileHeader>, StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        type HeaderRow = (
            String,
            Option<String>,
            Option<i64>,
            Option<String>,
            Option<i64>,
        );
        let row: Option<HeaderRow> = sqlx::query_as(
            "SELECT r.kind, r.name, v.version, v.description, v.updated_at \
             FROM rooms r \
             LEFT JOIN room_profile_versions v \
                ON v.room_id = r.id \
               AND v.version = (SELECT MAX(version) FROM room_profile_versions \
                                WHERE room_id = r.id) \
             WHERE r.id = ?",
        )
        .bind(room_id.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_error)?;
        let Some((kind, name, version, description, updated_at)) = row else {
            return Ok(None);
        };
        let kind = crate::domain::room::RoomKind::parse(&kind)
            .ok_or_else(|| corrupt(format!("unknown room kind {kind:?}")))?;
        let profile = match (version, description, updated_at) {
            (Some(version), Some(description), Some(updated_at)) => Some(
                room_profile_read_from_row(room_id.clone(), (version, description, updated_at))?,
            ),
            (None, None, None) => None,
            _ => {
                return Err(corrupt(
                    "stored room profile header has incomplete version fields",
                ));
            }
        };
        Ok(Some(RoomProfileHeader {
            room_id: room_id.clone(),
            kind,
            name,
            profile,
        }))
    }

    async fn latest_profile(
        &self,
        participant_id: &ParticipantId,
    ) -> Result<Option<ProfileRead>, StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        fetch_latest_profile(&mut conn, participant_id).await
    }

    async fn latest_room_presentation(
        &self,
        room_id: &RoomId,
        participant_id: &ParticipantId,
    ) -> Result<Option<RoomPresentationRead>, StoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let row: Option<PresentationReadRow> = sqlx::query_as(
            "SELECT rpv.presentation_version, rpv.room_display_name, rpv.room_role_name, \
                    rpv.room_role_description, rpv.published_by_id, p.kind, rpv.published_at \
             FROM room_presentation_versions rpv \
             JOIN participants p ON p.id = rpv.published_by_id \
             WHERE rpv.room_id = ? AND rpv.participant_id = ? \
             ORDER BY rpv.presentation_version DESC LIMIT 1",
        )
        .bind(room_id.as_str())
        .bind(participant_id.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_error)?;
        row.map(|r| presentation_read_from_row(room_id.clone(), participant_id.clone(), r))
            .transpose()
    }

    async fn member_presentation_summaries(
        &self,
        room_id: &RoomId,
    ) -> Result<Vec<MemberPresentationSummary>, StoreError> {
        type SummaryRow = (
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<i64>,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        let rows: Vec<SummaryRow> = sqlx::query_as(
            "SELECT p.id, p.kind, p.status, p.display_name, \
                    pf.short_description, \
                    pf.avatar_content_type, pf.avatar_size_bytes, \
                    rp.room_display_name, rp.room_role_name, rp.room_role_description \
             FROM room_memberships m \
             JOIN participants p ON p.id = m.participant_id \
             LEFT JOIN participant_profile_versions pf \
                ON pf.participant_id = p.id \
               AND pf.profile_version = (SELECT MAX(profile_version) FROM participant_profile_versions WHERE participant_id = p.id) \
             LEFT JOIN room_presentation_versions rp \
                ON rp.room_id = m.room_id AND rp.participant_id = m.participant_id \
               AND rp.presentation_version = (SELECT MAX(presentation_version) FROM room_presentation_versions \
                                              WHERE room_id = m.room_id AND participant_id = m.participant_id) \
             WHERE m.room_id = ? AND m.active_marker = 1 \
             ORDER BY p.id ASC",
        )
        .bind(room_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        let mut out = Vec::with_capacity(rows.len());
        for (
            id,
            kind,
            status,
            display_name,
            short,
            avatar_ct,
            avatar_size,
            room_display,
            role_name,
            role_desc,
        ) in rows
        {
            out.push(MemberPresentationSummary {
                participant: ParticipantProfile {
                    id: ParticipantId(id.clone()),
                    kind: ParticipantKind::parse(&kind)
                        .ok_or_else(|| corrupt(format!("unknown member kind {kind:?}")))?,
                    display_name: crate::domain::participant::DisplayName::parse(&display_name)
                        .map_err(|e| corrupt(format!("stored display name invalid: {e}")))?,
                    status: crate::domain::participant::ParticipantStatus::parse(&status)
                        .ok_or_else(|| corrupt(format!("unknown member status {status:?}")))?,
                },
                short_description: short
                    .map(|s| {
                        ShortDescription::parse(&s)
                            .map_err(|e| corrupt(format!("stored short description invalid: {e}")))
                    })
                    .transpose()?,
                avatar: parse_avatar_meta(avatar_ct, avatar_size)?,
                room_display_name: room_display
                    .map(|s| {
                        RoomDisplayName::parse(&s)
                            .map_err(|e| corrupt(format!("stored room display name invalid: {e}")))
                    })
                    .transpose()?,
                room_role_name: role_name
                    .map(|s| {
                        RoomRoleName::parse(&s)
                            .map_err(|e| corrupt(format!("stored room role name invalid: {e}")))
                    })
                    .transpose()?,
                room_role_description: role_desc
                    .map(|s| {
                        RoomRoleDescription::parse(&s).map_err(|e| {
                            corrupt(format!("stored room role description invalid: {e}"))
                        })
                    })
                    .transpose()?,
            });
        }
        Ok(out)
    }

    async fn latest_avatar(
        &self,
        participant_id: &ParticipantId,
    ) -> Result<Option<LatestAvatar>, StoreError> {
        type Row = (
            i64,
            Option<String>,
            Option<String>,
            Option<i64>,
            Option<String>,
            Option<i64>,
        );
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let row: Option<Row> = sqlx::query_as(
            "SELECT ppv.profile_version, ppv.avatar_storage_key, ppv.avatar_content_type, \
                    ppv.avatar_size_bytes, ppv.avatar_sha256_hex, ppv.avatar_updated_at \
             FROM participant_profile_versions ppv \
             WHERE ppv.participant_id = ? \
             ORDER BY ppv.profile_version DESC LIMIT 1",
        )
        .bind(participant_id.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_error)?;
        let Some((version, avatar_key, avatar_ct, avatar_size, avatar_sha256, avatar_at)) = row
        else {
            return Ok(None);
        };
        let version = ProfileVersion::parse(version)
            .map_err(|e| corrupt(format!("stored profile version invalid: {e}")))?;
        let Some(avatar) =
            parse_avatar_full(avatar_key, avatar_ct, avatar_size, avatar_sha256, avatar_at)?
        else {
            return Ok(None);
        };
        Ok(Some(LatestAvatar {
            profile_version: version,
            avatar,
        }))
    }

    async fn avatar_storage_keys(&self) -> Result<std::collections::HashSet<String>, StoreError> {
        let keys: Vec<String> = sqlx::query_scalar(
            "SELECT avatar_storage_key FROM participant_profile_versions \
             WHERE avatar_storage_key IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        Ok(keys.into_iter().collect())
    }
}

/// 幂等记录指向的历史 Profile 版本行原始列。
type HistoricalProfileRow = (
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<i64>,
    String,
    i64,
);

/// 幂等记录指向的 Profile 版本行（重放时读回历史结果；不分版本新旧）。
async fn fetch_latest_version_row(
    conn: &mut SqliteConnection,
    participant_id: &ParticipantId,
    version: i64,
) -> Result<Option<ParticipantProfileVersion>, StoreError> {
    let row: Option<HistoricalProfileRow> = sqlx::query_as(
        "SELECT profile_version, short_description, public_description, \
                avatar_storage_key, avatar_content_type, avatar_size_bytes, \
                avatar_sha256_hex, avatar_updated_at, published_by_id, published_at \
         FROM participant_profile_versions \
         WHERE participant_id = ? AND profile_version = ?",
    )
    .bind(participant_id.as_str())
    .bind(version)
    .fetch_optional(conn)
    .await
    .map_err(db_error)?;
    row.map(
        |(
            v,
            short,
            public,
            avatar_key,
            avatar_ct,
            avatar_size,
            avatar_sha256,
            avatar_at,
            by,
            at,
        )| {
            Ok(ParticipantProfileVersion {
                participant_id: participant_id.clone(),
                profile_version: ProfileVersion::parse(v)
                    .map_err(|e| corrupt(format!("stored profile version invalid: {e}")))?,
                short_description: short
                    .map(|s| {
                        ShortDescription::parse(&s)
                            .map_err(|e| corrupt(format!("stored short description invalid: {e}")))
                    })
                    .transpose()?,
                public_description: public
                    .map(|s| {
                        PublicDescription::parse(&s)
                            .map_err(|e| corrupt(format!("stored public description invalid: {e}")))
                    })
                    .transpose()?,
                avatar: parse_avatar_full(
                    avatar_key,
                    avatar_ct,
                    avatar_size,
                    avatar_sha256,
                    avatar_at,
                )?,
                published_by_id: ParticipantId(by),
                published_at: UtcMillis::from_millis(at),
            })
        },
    )
    .transpose()
}

/// 从版本行原始列解析 RoomProfileRead。
fn room_profile_read_from_row(
    room_id: RoomId,
    (version, description, updated_at): (i64, String, i64),
) -> Result<RoomProfileRead, StoreError> {
    Ok(RoomProfileRead {
        room_id,
        version: RoomProfileVersion::parse(version)
            .map_err(|e| corrupt(format!("stored room profile version invalid: {e}")))?,
        description: RoomDescription::parse(&description)
            .map_err(|e| corrupt(format!("stored room description invalid: {e}")))?,
        updated_at: UtcMillis::from_millis(updated_at),
    })
}

/// 幂等记录指向的 Room Profile 版本行（重放时读回历史结果；不分版本新旧）。
async fn fetch_room_profile_row(
    conn: &mut SqliteConnection,
    room_id: &RoomId,
    version: i64,
) -> Result<Option<RoomProfileRead>, StoreError> {
    let row: Option<(i64, String, i64)> = sqlx::query_as(
        "SELECT version, description, updated_at FROM room_profile_versions \
         WHERE room_id = ? AND version = ?",
    )
    .bind(room_id.as_str())
    .bind(version)
    .fetch_optional(conn)
    .await
    .map_err(db_error)?;
    row.map(|r| room_profile_read_from_row(room_id.clone(), r))
        .transpose()
}

/// 幂等记录指向的历史 Presentation 版本行原始列。
type HistoricalPresentationRow = (
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    i64,
);

/// 幂等记录指向的 Presentation 版本行（重放时读回历史结果）。
async fn fetch_latest_room_version_row(
    conn: &mut SqliteConnection,
    room_id: &RoomId,
    participant_id: &ParticipantId,
    version: i64,
) -> Result<Option<RoomPresentationVersion>, StoreError> {
    let row: Option<HistoricalPresentationRow> = sqlx::query_as(
        "SELECT presentation_version, room_display_name, room_role_name, room_role_description, published_by_id, published_at \
         FROM room_presentation_versions \
         WHERE room_id = ? AND participant_id = ? AND presentation_version = ?",
    )
    .bind(room_id.as_str())
    .bind(participant_id.as_str())
    .bind(version)
    .fetch_optional(conn)
    .await
    .map_err(db_error)?;
    row.map(|(v, display, role_name, role_desc, by, at)| {
        Ok(RoomPresentationVersion {
            room_id: room_id.clone(),
            participant_id: participant_id.clone(),
            presentation_version: PresentationVersion::parse(v)
                .map_err(|e| corrupt(format!("stored presentation version invalid: {e}")))?,
            room_display_name: display
                .map(|s| {
                    RoomDisplayName::parse(&s)
                        .map_err(|e| corrupt(format!("stored room display name invalid: {e}")))
                })
                .transpose()?,
            room_role_name: role_name
                .map(|s| {
                    RoomRoleName::parse(&s)
                        .map_err(|e| corrupt(format!("stored room role name invalid: {e}")))
                })
                .transpose()?,
            room_role_description: role_desc
                .map(|s| {
                    RoomRoleDescription::parse(&s)
                        .map_err(|e| corrupt(format!("stored room role description invalid: {e}")))
                })
                .transpose()?,
            published_by_id: ParticipantId(by),
            published_at: UtcMillis::from_millis(at),
        })
    })
    .transpose()
}
