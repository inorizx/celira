//! Group Room Profile（群简介）用例：群主写、成员读、单房历史回执带房间头。
//!
//! 契约：CORE_V1_ROOM_PROFILE.md §3–§5。用例层只做规则编排：解析/规范化
//! Room ID 与幂等键、以 domain 校验 `base_version` 与正文（1..2000 UTF-8
//! 字节、禁 NUL、`trim` 后非空）、生成请求摘要，再调用窄存储端口在单个
//! `BEGIN IMMEDIATE` 事务内完成授权（该房当前 active owner 且为 active
//! Human，**先于重放裁决**）→ 幂等/并发裁决 → 写不可变版本行 + 幂等见证 →
//! commit。
//!
//! 三个读入口：
//! - `get_room_detail`：`GET /v1/rooms/{room_id}` 的 member-scoped 视图 +
//!   简介（无简介 / 非 group 为 None）；
//! - `latest_room_profile`：供回执组装读取；
//! - `room_header`：`GET /v1/backend/rooms/{room_id}/messages` 的顶层房间头
//!   （身份三格 + 简介）。它**不做**可见性裁决——调用方必须先经既有成员准入。
//!
//! 简介不进消息 / 投递链，不产生未读或通知。路由层绝不重复弱化检查。

use crate::application::unexpected_store_error;
use crate::domain::agent::IdempotencyKey;
use crate::domain::participant::{Participant, ParticipantId};
use crate::domain::participant_presentation::BaseVersion;
use crate::domain::room::RoomId;
use crate::domain::room_profile::{
    RoomDescription, RoomProfileVersion, RoomProfileVersionRow, room_profile_put_digest,
};
use crate::domain::time::UtcMillis;
use crate::ports::participant_presentation::{
    ParticipantPresentationStore, RoomProfileHeader, RoomProfileRead, StoreError,
};
use crate::ports::room::{RoomListItem, RoomStore};
use crate::ports::storage::StorageError;

use crate::application::participant_presentation::map_store_query_error;
use crate::application::room::{RoomQueryError, get_room};

// ============================================================
// 写：PUT /v1/rooms/{room_id}/profile
// ============================================================

/// 幂等 PUT 结果：`created=false` 为同 key 同摘要重放（HTTP 200 +
/// `idempotency_replayed=true`，不追加版本）。
#[derive(Debug)]
pub struct PutRoomProfileOutcome {
    pub created: bool,
    pub result: RoomProfileRead,
}

/// 群简介 PUT 失败（内部 typed；HTTP 阶段映射到公共 code/状态）。
#[derive(Debug)]
pub enum RoomProfilePutError {
    /// key 格式不合法：400 `invalid_idempotency_key`。
    InvalidIdempotencyKey,
    /// 请求体形状不合法（`description` 缺失、`base_version` 负数或非法）：
    /// 400 `invalid_request`。
    InvalidRequest,
    /// 简介正文不合法（空、`trim` 后为空、含 NUL、超过 2000 字节）：
    /// 400 `invalid_profile_description`。
    InvalidDescription,
    /// Room unknown / bad-id / actor 不是该房当前 active 成员：
    /// 统一 404 `room_not_found`（不泄露存在性）。
    RoomNotFound,
    /// actor 是该房当前 active 成员但不是 owner：403 `not_room_owner`。
    NotRoomOwner,
    /// 目标 Room 不是 group（direct / human_direct）：400 `invalid_room_kind`。
    InvalidRoomKind,
    /// 新 key 但 `base_version` 已过期：409 `profile_version_conflict`。
    VersionConflict,
    /// 同 key 已被不同请求使用：409 `idempotency_conflict`。
    IdempotencyConflict,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for RoomProfilePutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdempotencyKey => write!(f, "invalid idempotency key"),
            Self::InvalidRequest => write!(f, "invalid room profile request"),
            Self::InvalidDescription => write!(f, "invalid room profile description"),
            Self::RoomNotFound => write!(f, "room not found"),
            Self::NotRoomOwner => write!(f, "actor is not the active room owner"),
            Self::InvalidRoomKind => write!(f, "room kind does not support a profile"),
            Self::VersionConflict => write!(f, "room profile base_version is stale"),
            Self::IdempotencyConflict => {
                write!(
                    f,
                    "idempotency key was already used with a different request"
                )
            }
            Self::Store(e) => write!(f, "presentation store failure: {e}"),
        }
    }
}

impl std::error::Error for RoomProfilePutError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_put_room_profile_store_error(e: StoreError) -> RoomProfilePutError {
    match e {
        StoreError::UnknownRoom | StoreError::NotRoomMember => RoomProfilePutError::RoomNotFound,
        StoreError::NotRoomOwner => RoomProfilePutError::NotRoomOwner,
        StoreError::NotGroupRoom => RoomProfilePutError::InvalidRoomKind,
        StoreError::IdempotencyConflict => RoomProfilePutError::IdempotencyConflict,
        StoreError::ConcurrentVersionChanged => RoomProfilePutError::VersionConflict,
        StoreError::Database(e) => RoomProfilePutError::Store(e),
        other => RoomProfilePutError::Store(unexpected_store_error(
            "presentation store error for room profile put",
            other,
        )),
    }
}

/// PUT /v1/rooms/{room_id}/profile：该 group 房当前 active owner（Human）唯一
/// 写路径。每条成功写入追加一个不可变简介版本行（+1）与幂等见证；同 key
/// 同摘要恒重放既有结果（`created=false`），同 key 不同摘要
/// `idempotency_conflict`，新 key 要求 `base_version == 当前最新`。
///
/// 授权（该房 active owner 且为 active Human）由存储端口在 `BEGIN IMMEDIATE`
/// 事务内先于任何幂等裁决完成：owner 转让后旧 owner 一律不得经重放访问结果。
pub async fn put_room_profile(
    presentation_store: &dyn ParticipantPresentationStore,
    actor: &Participant,
    room_id_raw: &str,
    idempotency_key_raw: &str,
    base_version_raw: i64,
    description_raw: Option<&str>,
) -> Result<PutRoomProfileOutcome, RoomProfilePutError> {
    let key = IdempotencyKey::parse(idempotency_key_raw)
        .map_err(|_| RoomProfilePutError::InvalidIdempotencyKey)?;
    let room_id = RoomId::parse_canonical(room_id_raw).ok_or(RoomProfilePutError::RoomNotFound)?;
    let base_version =
        BaseVersion::parse(base_version_raw).map_err(|_| RoomProfilePutError::InvalidRequest)?;
    let description_raw = description_raw.ok_or(RoomProfilePutError::InvalidRequest)?;
    let description = RoomDescription::parse(description_raw)
        .map_err(|_| RoomProfilePutError::InvalidDescription)?;
    let request_digest = room_profile_put_digest(base_version, description.as_str());
    let now = UtcMillis::now();
    // 新 key 且 base_version == 最新时，新版本 = base_version + 1；存储端口
    // 在事务内以真实最新 + 1 覆盖，重放路径不裁决版本。
    let proposed_version = RoomProfileVersion::from_u64(base_version.as_u64() + 1)
        .map_err(|_| RoomProfilePutError::InvalidRequest)?;
    let record = RoomProfileVersionRow {
        room_id,
        version: proposed_version,
        description,
        updated_by: actor.id.clone(),
        updated_at: now,
    };
    let outcome = presentation_store
        .put_room_profile_version(&key, &request_digest, &base_version, &record, now)
        .await
        .map_err(map_put_room_profile_store_error)?;
    Ok(PutRoomProfileOutcome {
        created: outcome.created,
        result: outcome.result,
    })
}

// ============================================================
// 读：GET /v1/rooms/{room_id}（member-scoped 视图 + 简介）
// ============================================================

/// 单房管理视图 + 简介。`profile=None` 表示这间房还没有简介（或不是 group）。
#[derive(Debug)]
pub struct RoomDetail {
    pub room: RoomListItem,
    pub profile: Option<RoomProfileRead>,
}

/// GET /v1/rooms/{room_id}：先走既有 Room 可见性用例（unknown / bad-id /
/// 非成员统一 404），再读该房最新简介。**不**改 `GET /v1/rooms` 列表，也
/// **不**把简介塞进 `RoomView`。
pub async fn get_room_detail(
    room_store: &dyn RoomStore,
    presentation_store: &dyn ParticipantPresentationStore,
    participant_id: &ParticipantId,
    room_id_raw: &str,
) -> Result<RoomDetail, RoomQueryError> {
    let room = get_room(room_store, participant_id, room_id_raw).await?;
    let profile = presentation_store
        .latest_room_profile(&room.room.id)
        .await
        .map_err(|e| RoomQueryError::Store(map_store_query_error(e)))?;
    Ok(RoomDetail { room, profile })
}

// ============================================================
// 读：单房历史回执的房间头
// ============================================================

/// 读取单房历史回执的房间头（身份三格 + 最新简介）。Room unknown / bad-id
/// 返回 None。
///
/// ⚠ **不做可见性裁决**：调用方（Backend 单房历史路由）必须**先**成功走完
/// 既有 `list_messages` 成员准入，再调用本函数；这样非成员仍是既有的
/// 403 `not_room_member`，本读面无第二套准入判据。
pub async fn room_header(
    presentation_store: &dyn ParticipantPresentationStore,
    room_id_raw: &str,
) -> Result<Option<RoomProfileHeader>, StorageError> {
    let Some(room_id) = RoomId::parse_canonical(room_id_raw) else {
        return Ok(None);
    };
    presentation_store
        .room_profile_header(&room_id)
        .await
        .map_err(map_store_query_error)
}
