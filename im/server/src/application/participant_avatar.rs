//! Avatar 用例（CORE_V1_AVATAR_P0.md）：Owner 写路径（PUT/DELETE）与 Room
//! 成员读路径（GET 字节）。
//!
//! 写路径复用 Presentation 的同一版本链 / Idempotency-Key + base_version 裁决
//! / 同一份幂等记录表：头像写入/摘除 = 追加新 Profile 版本（profile_version
//! +1），授权（active Agent Owner）由存储端口在 `BEGIN IMMEDIATE` 内先于任何
//! 幂等裁决完成。字节纪律与消息附件逐项对齐：字节先经 `BlobStorage` 原子
//! 安装（临时文件 + fsync + no-clobber），随后版本行 + 幂等见证同一事务
//! 追加；行写入失败 / 幂等重放分支 best-effort 删除刚写入的未被引用字节，
//! 崩溃残留由附件同一条维护链的 orphan sweep 兜底。读路径与成员详情同一
//! 授权裁决（调用者与目标都是该 Room 当前 active 成员），读取时校验
//! size + SHA-256，不一致按内部损坏失败，绝不返回字节。

use sha2::Digest as _;

use crate::application::participant_presentation::{
    IdempotentPutOutcome, PresentationQueryError, map_store_query_error, profile_read_from_version,
    resolve_active_members,
};
use crate::application::unexpected_store_error;
use crate::domain::agent::IdempotencyKey;
use crate::domain::attachment::StorageKey;
use crate::domain::participant::{Participant, ParticipantId};
use crate::domain::participant_avatar::{
    Avatar, AvatarContentType, MAX_AVATAR_BYTES, avatar_put_digest, avatar_remove_digest,
};
use crate::domain::participant_presentation::{
    BaseVersion, ParticipantProfileVersion, ProfileVersion,
};
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::ports::blob::BlobStorage;
use crate::ports::participant_presentation::{
    LatestAvatar, ParticipantPresentationStore, ProfileRead, StoreError,
};
use crate::ports::room::RoomStore;
use crate::ports::storage::{StorageError, StorageFailureKind};

/// 头像写路径（PUT 设置 / DELETE 摘除统一）失败。
#[derive(Debug)]
pub enum AvatarWriteError {
    /// key 格式不合法。
    InvalidIdempotencyKey,
    /// base_version 非法（负值/非整数已由路由层 400）、空字节或非白名单
    /// Content-Type：400 `avatar_invalid`。
    InvalidBody,
    /// 字节超过 2 MiB：413 `payload_too_large`。
    PayloadTooLarge,
    /// unknown / bad-id / 无 active Owner 关系：统一 404 `agent_not_found`。
    NotFound,
    /// 目标 Agent 或 Owner 关系已 revoked：409 `agent_not_active`。
    NotActive,
    /// 同 key 已被不同请求使用：409 `idempotency_conflict`。
    IdempotencyConflict,
    /// 新 key 但 base_version 已过期：409 `profile_stale`。
    Stale,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for AvatarWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdempotencyKey => write!(f, "invalid idempotency key"),
            Self::InvalidBody => write!(f, "invalid avatar body"),
            Self::PayloadTooLarge => write!(f, "avatar exceeds the 2 MiB byte bound"),
            Self::NotFound => write!(f, "agent not found"),
            Self::NotActive => write!(f, "agent not active"),
            Self::IdempotencyConflict => {
                write!(
                    f,
                    "idempotency key was already used with a different request"
                )
            }
            Self::Stale => write!(f, "profile base_version is stale"),
            Self::Store(e) => write!(f, "avatar store failure: {e}"),
        }
    }
}

impl std::error::Error for AvatarWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_avatar_write_store_error(e: StoreError) -> AvatarWriteError {
    match e {
        StoreError::UnknownParticipant | StoreError::NotAgentOwner => AvatarWriteError::NotFound,
        StoreError::ParticipantNotActive => AvatarWriteError::NotActive,
        StoreError::IdempotencyConflict => AvatarWriteError::IdempotencyConflict,
        StoreError::ConcurrentVersionChanged => AvatarWriteError::Stale,
        StoreError::Database(e) => AvatarWriteError::Store(e),
        other => AvatarWriteError::Store(unexpected_store_error("avatar write store error", other)),
    }
}

/// best-effort 删除未被任何版本行引用的刚写入字节（行写入失败 / 幂等重放
/// 分支）：删除失败只记录脱敏 warning，残留由附件同一条维护链的 orphan
/// sweep 兜底，不掩盖业务失败。
async fn cleanup_unreferenced_avatar(blob: &dyn BlobStorage, storage_key: &StorageKey) {
    if blob.delete(storage_key).await.is_err() {
        tracing::warn!(
            operation = "avatar.write.cleanup",
            error_kind = "connection_or_io",
            "failed to remove unreferenced avatar bytes; orphan sweep covers it"
        );
    }
}

/// Owner-scoped Agent Avatar PUT：active Agent Owner Human Session 唯一写
/// 路径。字节先原子安装（临时文件 + fsync + no-clobber），随后存储端口在
/// 单个 `BEGIN IMMEDIATE` 内完成授权 → 幂等/base_version 裁决 → 追加不可变
/// Profile 版本行（avatar 字段族）+ 幂等见证。同 key 同摘要恒重放既有结果
/// （不追加版本；应用层删除刚写入、未被任何行引用的字节），同 key 异摘要
/// 409，新 key 要求 base_version == 当前最新。
#[allow(clippy::too_many_arguments)]
pub async fn put_owner_avatar(
    presentation_store: &dyn ParticipantPresentationStore,
    blob: &dyn BlobStorage,
    human: &Participant,
    agent_id_raw: &str,
    idempotency_key_raw: &str,
    base_version_raw: i64,
    content_type_raw: &str,
    bytes: &[u8],
) -> Result<IdempotentPutOutcome<ProfileRead>, AvatarWriteError> {
    let key = IdempotencyKey::parse(idempotency_key_raw)
        .map_err(|_| AvatarWriteError::InvalidIdempotencyKey)?;
    let agent_id =
        ParticipantId::parse_canonical(agent_id_raw).ok_or(AvatarWriteError::NotFound)?;
    let base_version =
        BaseVersion::parse(base_version_raw).map_err(|_| AvatarWriteError::InvalidBody)?;
    let content_type =
        AvatarContentType::parse(content_type_raw).map_err(|_| AvatarWriteError::InvalidBody)?;
    if bytes.is_empty() {
        return Err(AvatarWriteError::InvalidBody);
    }
    if bytes.len() > MAX_AVATAR_BYTES {
        return Err(AvatarWriteError::PayloadTooLarge);
    }
    let sha256_hex = hex::encode(sha2::Sha256::digest(bytes));
    let request_digest = avatar_put_digest(base_version, content_type, &sha256_hex);
    let now = UtcMillis::now();
    // 新 key 且 base_version == 最新时，新版本 = base_version + 1；存储端口
    // 在事务内以真实最新 + 1 覆盖，重放路径不裁决版本。
    let proposed_version = ProfileVersion::from_u64(base_version.as_u64() + 1)
        .map_err(|_| AvatarWriteError::InvalidBody)?;
    let storage_key = StorageKey::new_v4();
    let record = ParticipantProfileVersion {
        participant_id: agent_id,
        profile_version: proposed_version,
        short_description: None,
        public_description: None,
        avatar: Some(Avatar {
            storage_key: storage_key.clone(),
            content_type,
            size_bytes: bytes.len() as i64,
            sha256_hex: sha256_hex.clone(),
            updated_at: now,
        }),
        published_by_id: human.id.clone(),
        published_at: now,
    };

    // 1. 先写字节（原子 + fsync 安装；失败由 blob adapter 清理临时文件）。
    blob.write(&storage_key, bytes)
        .await
        .map_err(AvatarWriteError::Store)?;
    // 2. 版本行 + 幂等见证同一事务追加（授权/幂等/版本裁决在事务内）。
    match presentation_store
        .put_profile_version(&key, &request_digest, &base_version, &record, now)
        .await
    {
        Ok(outcome) if outcome.created => Ok(IdempotentPutOutcome {
            created: true,
            result_is_create: outcome.result_is_create,
            result: profile_read_from_version(human, outcome.result),
        }),
        Ok(outcome) => {
            // 幂等重放：不追加版本，刚写入的字节未被任何行引用 → 清理。
            cleanup_unreferenced_avatar(blob, &storage_key).await;
            Ok(IdempotentPutOutcome {
                created: false,
                result_is_create: outcome.result_is_create,
                result: profile_read_from_version(human, outcome.result),
            })
        }
        Err(primary) => {
            cleanup_unreferenced_avatar(blob, &storage_key).await;
            Err(map_avatar_write_store_error(primary))
        }
    }
}

/// Owner-scoped Agent Avatar DELETE：摘除 = 追加新版本 avatar=null；旧版本行
/// 与其字节不可变保留（版本链一致性，旧字节 GC 是运维事项，本切片不做）。
/// 同 key 同摘要恒重放、同 key 异摘要 409、新 key 要求 base_version == 当前
/// 最新（与 PUT 同一套裁决）。
pub async fn remove_owner_avatar(
    presentation_store: &dyn ParticipantPresentationStore,
    human: &Participant,
    agent_id_raw: &str,
    idempotency_key_raw: &str,
    base_version_raw: i64,
) -> Result<IdempotentPutOutcome<ProfileRead>, AvatarWriteError> {
    let key = IdempotencyKey::parse(idempotency_key_raw)
        .map_err(|_| AvatarWriteError::InvalidIdempotencyKey)?;
    let agent_id =
        ParticipantId::parse_canonical(agent_id_raw).ok_or(AvatarWriteError::NotFound)?;
    let base_version =
        BaseVersion::parse(base_version_raw).map_err(|_| AvatarWriteError::InvalidBody)?;
    let request_digest = avatar_remove_digest(base_version);
    let now = UtcMillis::now();
    let proposed_version = ProfileVersion::from_u64(base_version.as_u64() + 1)
        .map_err(|_| AvatarWriteError::InvalidBody)?;
    let record = ParticipantProfileVersion {
        participant_id: agent_id,
        profile_version: proposed_version,
        short_description: None,
        public_description: None,
        avatar: None,
        published_by_id: human.id.clone(),
        published_at: now,
    };
    let outcome = presentation_store
        .put_profile_version(&key, &request_digest, &base_version, &record, now)
        .await
        .map_err(map_avatar_write_store_error)?;
    Ok(IdempotentPutOutcome {
        created: outcome.created,
        result_is_create: outcome.result_is_create,
        result: profile_read_from_version(human, outcome.result),
    })
}

/// GET /v1/rooms/{room_id}/members/{participant_id}/avatar（字节）：授权 =
/// 调用者与目标都是该 Room 的当前 active 成员（与成员详情同一裁决）；
/// 无头像返回 `Ok(None)`（路由统一 404 `avatar_not_found`）。读取时校验
/// size + SHA-256，不一致按内部损坏失败，绝不返回字节。
pub async fn get_member_avatar_bytes(
    room_store: &dyn RoomStore,
    presentation_store: &dyn ParticipantPresentationStore,
    blob: &dyn BlobStorage,
    human_id: &ParticipantId,
    room_id_raw: &str,
    participant_id_raw: &str,
) -> Result<Option<(LatestAvatar, Vec<u8>)>, PresentationQueryError> {
    let room_id =
        RoomId::parse_canonical(room_id_raw).ok_or(PresentationQueryError::RoomNotFound)?;
    let participant_id = ParticipantId::parse_canonical(participant_id_raw)
        .ok_or(PresentationQueryError::MemberNotFound)?;
    let active = resolve_active_members(room_store, human_id, &room_id).await?;
    if !active.iter().any(|m| m.participant.id == participant_id) {
        return Err(PresentationQueryError::MemberNotFound);
    }
    let Some(latest) = presentation_store
        .latest_avatar(&participant_id)
        .await
        .map_err(|e| PresentationQueryError::Store(map_store_query_error(e)))?
    else {
        return Ok(None);
    };
    let bytes = blob
        .read(&latest.avatar.storage_key)
        .await
        .map_err(PresentationQueryError::Store)?;
    let size_matches =
        i64::try_from(bytes.len()).is_ok_and(|actual_size| actual_size == latest.avatar.size_bytes);
    let actual_sha256 = hex::encode(sha2::Sha256::digest(&bytes));
    if !size_matches || actual_sha256 != latest.avatar.sha256_hex {
        return Err(PresentationQueryError::Store(StorageError::from_message(
            StorageFailureKind::DecodeOrCorrupt,
            "stored avatar bytes do not match durable metadata",
        )));
    }
    Ok(Some((latest, bytes)))
}
