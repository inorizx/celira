//! Participant Presentation 用例：Agent 全局 Profile 与 Group Room Presentation
//! 的读取与幂等写入。
//!
//! 用例层负责规则编排：解析/规范化 ID 与幂等键、以 domain 校验 base_version
//! 与正文（空串归一为 NULL、字节/NUL 边界）、生成请求摘要，再调用窄存储端口
//! 在单个 `BEGIN IMMEDIATE` 事务内完成授权（active Agent Owner / active group
//! owner + active Agent member）→ 幂等/并发裁决 → 写不可变版本行 + 幂等见证
//! → commit。Room 可见性（当前 active Membership）复用 `RoomStore` 的既有
//! 查询；Profile/Presentation 写路径的授权由存储端口在事务内、任何幂等裁决
//! 之前完成（授权先于重放，被撤销/未授权 actor 一律不得经重放访问结果）。
//! 路由层绝不重复弱化检查。
//!
//! 契约：CORE_V1_PARTICIPANT_PRESENTATION.md §3–§7。

use crate::application::unexpected_store_error;
use crate::domain::agent::IdempotencyKey;
use crate::domain::participant::{Participant, ParticipantId};
use crate::domain::participant_presentation::{
    BaseVersion, ParticipantProfileVersion, PresentationVersion, ProfileVersion, PublicDescription,
    RoomDisplayName, RoomPresentationVersion, RoomRoleDescription, RoomRoleName, ShortDescription,
};
use crate::domain::room::{MembershipStatus, RoomId};
use crate::domain::time::UtcMillis;
use crate::ports::agent::AgentStoreError;
use crate::ports::participant_presentation::{
    MemberPresentationSummary, ParticipantPresentationStore, ProfileRead, RoomPresentationRead,
    StoreError,
};
use crate::ports::room::{ParticipantProfile, RoomStore, RoomStoreError};
use crate::ports::storage::StorageError;

/// 幂等 PUT 结果（Profile / Presentation 统一形状）：`created=false` 为同
/// key 同摘要重放，`result_is_create` 决定 HTTP 201/200（首次把版本 0 推进
/// 到 1）。
#[derive(Debug)]
pub struct IdempotentPutOutcome<T> {
    pub created: bool,
    pub result_is_create: bool,
    pub result: T,
}

/// 只读错误映射（Presentation Store 只读路径只返回 Database 变体；其余为
/// 内部不变量破坏，统一归 Unknown）。
pub(crate) fn map_store_query_error(e: StoreError) -> StorageError {
    match e {
        StoreError::Database(e) => e,
        other => unexpected_store_error("presentation store read error", other),
    }
}

// ============================================================
// Owner-scoped Agent Profile
// ============================================================

/// Owner-scoped Agent Profile GET 结果。
#[derive(Debug)]
pub struct GetOwnerProfileOutcome {
    pub owned: crate::domain::agent::OwnedAgent,
    pub profile: Option<ProfileRead>,
}

/// Owner-scoped Agent Profile GET 失败。
#[derive(Debug)]
pub enum ProfileGetError {
    /// unknown / bad-id / 无 active Owner 关系：统一 404 `agent_not_found`
    /// （隐藏存在性）。
    NotFound,
    /// 目标 Agent 或 Owner 关系已 revoked：409 `agent_not_active`。
    NotActive,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for ProfileGetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "agent not found"),
            Self::NotActive => write!(f, "agent not active"),
            Self::Store(e) => write!(f, "presentation store failure: {e}"),
        }
    }
}

impl std::error::Error for ProfileGetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_agent_store_error(e: AgentStoreError) -> ProfileGetError {
    match e {
        AgentStoreError::Database(e) => ProfileGetError::Store(e),
        other => ProfileGetError::Store(unexpected_store_error("agent store error", other)),
    }
}

/// Owner-scoped Agent Profile GET：目标必须是当前 active Human 且对该 Agent
/// 存在 active Owner 关系（P0 只读 Agent Profile）。unknown / bad-id /
/// 从未拥有的统一 NotFound；曾经拥有但已 revoked 的统一 NotActive。
pub async fn get_owner_profile(
    agent_store: &dyn crate::ports::agent::AgentStore,
    presentation_store: &dyn ParticipantPresentationStore,
    human_id: &ParticipantId,
    agent_id_raw: &str,
) -> Result<GetOwnerProfileOutcome, ProfileGetError> {
    let agent_id = ParticipantId::parse_canonical(agent_id_raw).ok_or(ProfileGetError::NotFound)?;
    let owned = agent_store
        .get_owned_agent(human_id, &agent_id)
        .await
        .map_err(map_agent_store_error)?
        .ok_or(ProfileGetError::NotFound)?;
    if owned.ownership.status == crate::domain::participant::ParticipantStatus::Revoked
        || owned.agent.status == crate::domain::participant::ParticipantStatus::Revoked
    {
        return Err(ProfileGetError::NotActive);
    }
    let profile = presentation_store
        .latest_profile(&agent_id)
        .await
        .map_err(|e| ProfileGetError::Store(map_store_query_error(e)))?;
    Ok(GetOwnerProfileOutcome { owned, profile })
}

/// Self-scoped Agent Profile GET（CORE_V1_AGENT_READ_SURFACE.md §2.2）：
/// Agent 经 BackendAuth 读**自己**的最新公共 Profile。身份已由 BackendAuth
/// 裁决（active Credential + active Binding + active Agent），此处**不再**
/// 重复身份或所有权检查，也不接受调用方传入 agent_id。只读；无版本返回
/// None；唯一失败是存储不可用。
pub async fn get_self_profile(
    presentation_store: &dyn ParticipantPresentationStore,
    agent_id: &ParticipantId,
) -> Result<Option<ProfileRead>, StorageError> {
    presentation_store
        .latest_profile(agent_id)
        .await
        .map_err(map_store_query_error)
}

/// Agent Profile PUT 失败。
#[derive(Debug)]
pub enum ProfilePutError {
    /// key 格式不合法。
    InvalidIdempotencyKey,
    /// base_version 或正文不合法（负 base_version、NUL、超字节上限）：
    /// 400 `profile_invalid`。
    InvalidBody,
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

impl std::fmt::Display for ProfilePutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdempotencyKey => write!(f, "invalid idempotency key"),
            Self::InvalidBody => write!(f, "invalid profile body"),
            Self::NotFound => write!(f, "agent not found"),
            Self::NotActive => write!(f, "agent not active"),
            Self::IdempotencyConflict => {
                write!(
                    f,
                    "idempotency key was already used with a different request"
                )
            }
            Self::Stale => write!(f, "profile base_version is stale"),
            Self::Store(e) => write!(f, "presentation store failure: {e}"),
        }
    }
}

impl std::error::Error for ProfilePutError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_put_profile_store_error(e: StoreError) -> ProfilePutError {
    match e {
        StoreError::UnknownParticipant | StoreError::NotAgentOwner => ProfilePutError::NotFound,
        StoreError::ParticipantNotActive => ProfilePutError::NotActive,
        StoreError::IdempotencyConflict => ProfilePutError::IdempotencyConflict,
        StoreError::ConcurrentVersionChanged => ProfilePutError::Stale,
        StoreError::Database(e) => ProfilePutError::Store(e),
        other => ProfilePutError::Store(unexpected_store_error(
            "presentation store error for profile put",
            other,
        )),
    }
}

/// Owner-scoped Agent Profile PUT：active Agent Owner Human Session 唯一写
/// 路径。每条成功写入追加一个不可变 Profile 版本行（+1）与幂等见证；同 key
/// 同摘要恒重放既有结果（`created=false`），同 key 不同摘要
/// `idempotency_conflict`，新 key 要求 `base_version == 当前最新`。
///
/// 授权（active Owner 关系）由存储端口在 `BEGIN IMMEDIATE` 事务内先于任何
/// 幂等裁决完成：被撤销/未授权 actor 一律不得经重放访问结果。
pub async fn put_owner_profile(
    presentation_store: &dyn ParticipantPresentationStore,
    human: &Participant,
    agent_id_raw: &str,
    idempotency_key_raw: &str,
    base_version_raw: i64,
    short_description: Option<&str>,
    public_description: Option<&str>,
) -> Result<IdempotentPutOutcome<ProfileRead>, ProfilePutError> {
    let key = IdempotencyKey::parse(idempotency_key_raw)
        .map_err(|_| ProfilePutError::InvalidIdempotencyKey)?;
    let agent_id = ParticipantId::parse_canonical(agent_id_raw).ok_or(ProfilePutError::NotFound)?;
    let base_version =
        BaseVersion::parse(base_version_raw).map_err(|_| ProfilePutError::InvalidBody)?;
    let short =
        ShortDescription::parse_opt(short_description).map_err(|_| ProfilePutError::InvalidBody)?;
    let public = PublicDescription::parse_opt(public_description)
        .map_err(|_| ProfilePutError::InvalidBody)?;
    let request_digest = crate::domain::participant_presentation::profile_put_digest(
        base_version,
        short.as_ref().map(|s| s.as_str()),
        public.as_ref().map(|s| s.as_str()),
    );
    let now = UtcMillis::now();
    // 新 key 且 base_version == 最新时，新版本 = base_version + 1；存储端口
    // 在事务内以真实最新 + 1 覆盖，重放路径不裁决版本。
    let proposed_version = ProfileVersion::from_u64(base_version.as_u64() + 1)
        .map_err(|_| ProfilePutError::InvalidBody)?;
    let record = ParticipantProfileVersion {
        participant_id: agent_id,
        profile_version: proposed_version,
        short_description: short,
        public_description: public,
        avatar: None,
        published_by_id: human.id.clone(),
        published_at: now,
    };
    let outcome = presentation_store
        .put_profile_version(&key, &request_digest, &base_version, &record, now)
        .await
        .map_err(map_put_profile_store_error)?;
    Ok(IdempotentPutOutcome {
        created: outcome.created,
        result_is_create: outcome.result_is_create,
        result: profile_read_from_version(human, outcome.result),
    })
}

/// 写路径的发布者必然是当前认证的 active Human；`published_by_kind` 据
/// `published_by_id`（= 该 Human）的 Participant kind 派生，绝不冗余存储。
/// avatar 从完整内部事实降为公共投影（存在性 + content_type + size_bytes）。
pub(crate) fn profile_read_from_version(
    human: &Participant,
    version: ParticipantProfileVersion,
) -> ProfileRead {
    ProfileRead {
        participant_id: version.participant_id,
        profile_version: version.profile_version,
        short_description: version.short_description,
        public_description: version.public_description,
        avatar: version
            .avatar
            .as_ref()
            .map(crate::domain::participant_avatar::AvatarMeta::from),
        published_by_id: version.published_by_id,
        published_by_kind: human.kind,
        published_at: version.published_at,
    }
}

// ============================================================
// Room member presentation 读取
// ============================================================

/// Room 可见性查询失败（summary / member detail / member presentation）。
#[derive(Debug)]
pub enum PresentationQueryError {
    /// unknown / bad-id / actor 无 active Membership：统一 404
    /// `room_not_found`，不泄露 Room 是否存在。
    RoomNotFound,
    /// 目标不是该 Room 的当前 active 成员：统一 404 `member_not_found`。
    MemberNotFound,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for PresentationQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RoomNotFound => write!(f, "room not found"),
            Self::MemberNotFound => write!(f, "member not found"),
            Self::Store(e) => write!(f, "presentation store failure: {e}"),
        }
    }
}

impl std::error::Error for PresentationQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_room_query_error(e: RoomStoreError) -> PresentationQueryError {
    match e {
        RoomStoreError::Database(e) => PresentationQueryError::Store(e),
        other => PresentationQueryError::Store(unexpected_store_error(
            "room store error for presentation query",
            other,
        )),
    }
}

/// 解析并验证 actor 是该 Room 的当前 active 成员；返回 active 成员视图。
/// 复用 `RoomStore::get_room_members` 的同一裁决（direct / human_direct 的
/// 配对 closed 周期在此被过滤），绝不重新弱化。avatar 读路径复用同一裁决。
pub(crate) async fn resolve_active_members(
    room_store: &dyn RoomStore,
    human_id: &ParticipantId,
    room_id: &RoomId,
) -> Result<Vec<crate::ports::room::MemberView>, PresentationQueryError> {
    let members = room_store
        .get_room_members(human_id, room_id)
        .await
        .map_err(map_room_query_error)?
        .ok_or(PresentationQueryError::RoomNotFound)?;
    Ok(members
        .into_iter()
        .filter(|m| m.membership.status == MembershipStatus::Active)
        .collect())
}

/// GET /v1/rooms/{room_id}/member-presentations：当前 active Room member 的
/// Presentation 摘要列表。本用例只裁决「requester 必须当前 active 成员」
/// （复用 RoomStore 同一可见性查询），随后由存储读取该 Room 全部 active
/// 成员（Human 与 Agent）的 Profile short_description；Presentation 文本字段
/// 只对 active Agent 成员存在（Human 恒为 null）。
pub async fn get_member_presentations(
    room_store: &dyn RoomStore,
    presentation_store: &dyn ParticipantPresentationStore,
    human_id: &ParticipantId,
    room_id_raw: &str,
) -> Result<Vec<MemberPresentationSummary>, PresentationQueryError> {
    let room_id =
        RoomId::parse_canonical(room_id_raw).ok_or(PresentationQueryError::RoomNotFound)?;
    resolve_active_members(room_store, human_id, &room_id).await?;
    presentation_store
        .member_presentation_summaries(&room_id)
        .await
        .map_err(|e| PresentationQueryError::Store(map_store_query_error(e)))
}

/// 成员详情（member profile GET 用）：主动过滤到当前 active 成员后再取
/// Profile / Presentation。
#[derive(Debug)]
pub struct MemberDetail {
    pub participant: ParticipantProfile,
    pub profile: Option<ProfileRead>,
    pub presentation: Option<RoomPresentationRead>,
}

/// GET /v1/rooms/{room_id}/members/{participant_id}/profile：requester 与
/// target 都必须当前 active 成员；返回 target 的最小公共资料、全局 Profile
/// （无版本为 null）与当前 Room Presentation（direct / human_direct 无
/// Presentation，恒为 null）。
pub async fn get_member_profile(
    room_store: &dyn RoomStore,
    presentation_store: &dyn ParticipantPresentationStore,
    human_id: &ParticipantId,
    room_id_raw: &str,
    participant_id_raw: &str,
) -> Result<MemberDetail, PresentationQueryError> {
    let room_id =
        RoomId::parse_canonical(room_id_raw).ok_or(PresentationQueryError::RoomNotFound)?;
    let participant_id = ParticipantId::parse_canonical(participant_id_raw)
        .ok_or(PresentationQueryError::MemberNotFound)?;
    let active = resolve_active_members(room_store, human_id, &room_id).await?;
    let target = active
        .iter()
        .find(|m| m.participant.id == participant_id)
        .ok_or(PresentationQueryError::MemberNotFound)?;
    let profile = presentation_store
        .latest_profile(&participant_id)
        .await
        .map_err(|e| PresentationQueryError::Store(map_store_query_error(e)))?;
    let presentation = presentation_store
        .latest_room_presentation(&room_id, &participant_id)
        .await
        .map_err(|e| PresentationQueryError::Store(map_store_query_error(e)))?;
    Ok(MemberDetail {
        participant: target.participant.clone(),
        profile,
        presentation,
    })
}

/// Member presentation GET 结果。
#[derive(Debug)]
pub struct MemberPresentation {
    pub participant: ParticipantProfile,
    pub presentation: Option<RoomPresentationRead>,
}

/// GET /v1/rooms/{room_id}/members/{participant_id}/presentation：requester
/// 与 target 都必须当前 active 成员；direct / human_direct 无 Presentation
/// （返回 null）。
pub async fn get_member_presentation(
    room_store: &dyn RoomStore,
    presentation_store: &dyn ParticipantPresentationStore,
    human_id: &ParticipantId,
    room_id_raw: &str,
    participant_id_raw: &str,
) -> Result<MemberPresentation, PresentationQueryError> {
    let room_id =
        RoomId::parse_canonical(room_id_raw).ok_or(PresentationQueryError::RoomNotFound)?;
    let participant_id = ParticipantId::parse_canonical(participant_id_raw)
        .ok_or(PresentationQueryError::MemberNotFound)?;
    let active = resolve_active_members(room_store, human_id, &room_id).await?;
    let target = active
        .iter()
        .find(|m| m.participant.id == participant_id)
        .ok_or(PresentationQueryError::MemberNotFound)?;
    let presentation = presentation_store
        .latest_room_presentation(&room_id, &participant_id)
        .await
        .map_err(|e| PresentationQueryError::Store(map_store_query_error(e)))?;
    Ok(MemberPresentation {
        participant: target.participant.clone(),
        presentation,
    })
}

// ============================================================
// Group Room Presentation 写入
// ============================================================

/// Group Room Presentation PUT 失败。
#[derive(Debug)]
pub enum PresentationPutError {
    /// key 格式不合法。
    InvalidIdempotencyKey,
    /// base_version 或正文不合法（负 base_version、NUL、超字节上限）：
    /// 400 `room_presentation_invalid`。
    InvalidBody,
    /// Room unknown / bad-id / actor 不是该 group 的 active owner：
    /// 统一 404 `room_not_found`。
    RoomNotFound,
    /// 目标不是该 group 的 active Agent 成员：统一 404 `member_not_found`。
    MemberNotFound,
    /// 目标 Room 不是 group（direct / human_direct）：400
    /// `room_presentation_not_supported`。
    NotSupported,
    /// 同 key 已被不同请求使用：409 `idempotency_conflict`。
    IdempotencyConflict,
    /// 新 key 但 base_version 已过期：409 `room_presentation_stale`。
    Stale,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for PresentationPutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdempotencyKey => write!(f, "invalid idempotency key"),
            Self::InvalidBody => write!(f, "invalid room presentation body"),
            Self::RoomNotFound => write!(f, "room not found"),
            Self::MemberNotFound => write!(f, "member not found"),
            Self::NotSupported => {
                write!(f, "room presentation is not supported for this room")
            }
            Self::IdempotencyConflict => {
                write!(
                    f,
                    "idempotency key was already used with a different request"
                )
            }
            Self::Stale => write!(f, "room presentation base_version is stale"),
            Self::Store(e) => write!(f, "presentation store failure: {e}"),
        }
    }
}

impl std::error::Error for PresentationPutError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_put_presentation_store_error(e: StoreError) -> PresentationPutError {
    match e {
        StoreError::UnknownRoom | StoreError::NotGroupOwner => PresentationPutError::RoomNotFound,
        StoreError::NotGroupRoom => PresentationPutError::NotSupported,
        StoreError::NotActiveAgentMember | StoreError::UnknownParticipant => {
            PresentationPutError::MemberNotFound
        }
        StoreError::IdempotencyConflict => PresentationPutError::IdempotencyConflict,
        StoreError::ConcurrentVersionChanged => PresentationPutError::Stale,
        StoreError::Database(e) => PresentationPutError::Store(e),
        other => PresentationPutError::Store(unexpected_store_error(
            "presentation store error for room presentation put",
            other,
        )),
    }
}

/// Group Room Presentation PUT：current Group active owner + 目标 current
/// active Agent member。仅 Agent Owner 而无该 Room 成员资格的 Human 无 Room
/// 权威（存储事务内裁决）；direct / human_direct → `NotSupported`。
#[allow(clippy::too_many_arguments)]
pub async fn put_room_presentation(
    presentation_store: &dyn ParticipantPresentationStore,
    human: &Participant,
    room_id_raw: &str,
    participant_id_raw: &str,
    idempotency_key_raw: &str,
    base_version_raw: i64,
    room_display_name: Option<&str>,
    room_role_name: Option<&str>,
    room_role_description: Option<&str>,
) -> Result<IdempotentPutOutcome<RoomPresentationRead>, PresentationPutError> {
    let key = IdempotencyKey::parse(idempotency_key_raw)
        .map_err(|_| PresentationPutError::InvalidIdempotencyKey)?;
    let room_id = RoomId::parse_canonical(room_id_raw).ok_or(PresentationPutError::RoomNotFound)?;
    let participant_id = ParticipantId::parse_canonical(participant_id_raw)
        .ok_or(PresentationPutError::MemberNotFound)?;
    let base_version =
        BaseVersion::parse(base_version_raw).map_err(|_| PresentationPutError::InvalidBody)?;
    let display = RoomDisplayName::parse_opt(room_display_name)
        .map_err(|_| PresentationPutError::InvalidBody)?;
    let role_name =
        RoomRoleName::parse_opt(room_role_name).map_err(|_| PresentationPutError::InvalidBody)?;
    let role_desc = RoomRoleDescription::parse_opt(room_role_description)
        .map_err(|_| PresentationPutError::InvalidBody)?;
    let request_digest = crate::domain::participant_presentation::room_presentation_put_digest(
        base_version,
        display.as_ref().map(|s| s.as_str()),
        role_name.as_ref().map(|s| s.as_str()),
        role_desc.as_ref().map(|s| s.as_str()),
    );
    let now = UtcMillis::now();
    let proposed_version = PresentationVersion::from_u64(base_version.as_u64() + 1)
        .map_err(|_| PresentationPutError::InvalidBody)?;
    let record = RoomPresentationVersion {
        room_id,
        participant_id,
        presentation_version: proposed_version,
        room_display_name: display,
        room_role_name: role_name,
        room_role_description: role_desc,
        published_by_id: human.id.clone(),
        published_at: now,
    };
    let outcome = presentation_store
        .put_room_presentation_version(&key, &request_digest, &base_version, &record, now)
        .await
        .map_err(map_put_presentation_store_error)?;
    Ok(IdempotentPutOutcome {
        created: outcome.created,
        result_is_create: outcome.result_is_create,
        result: presentation_read_from_version(human, outcome.result),
    })
}

/// 写路径的发布者必然是当前认证的 active Human（发布者即 active group
/// owner）；`published_by_kind` 派生，绝不冗余存储。
fn presentation_read_from_version(
    human: &Participant,
    version: RoomPresentationVersion,
) -> RoomPresentationRead {
    RoomPresentationRead {
        room_id: version.room_id,
        participant_id: version.participant_id,
        presentation_version: version.presentation_version,
        room_display_name: version.room_display_name,
        room_role_name: version.room_role_name,
        room_role_description: version.room_role_description,
        published_by_id: version.published_by_id,
        published_by_kind: human.kind,
        published_at: version.published_at,
    }
}
