//! Participant Presentation 存储窄端口（Profile + Group Room Presentation）。
//!
//! 契约（CORE_V1_PARTICIPANT_PRESENTATION.md §4–§6）：只声明 P0 写路径与最新
//! 读取所需的持久化边界，不依赖具体数据库/HTTP 框架；不引入 generic
//! repository。裁决由 storage adapter 在事务内执行，顺序为：**认证 →
//! 授权 → 幂等重放/冲突 → base_version 裁决**；同 key + 同请求摘要恒重放
//! 既有成功结果（即使用例中最新版本已被其他写入推进，也不重新裁决
//! base_version 而变 stale），授权先于重放——已撤销/未授权 actor 一律不得重放。
//! 存储层只表达「不可变版本 + 幂等记录」的能力与 typed 错误；`put_*` 在单个
//! `BEGIN IMMEDIATE` 事务内完成，数据库守卫只证明引用/身份一致性（不单独证明
//! 同事务），见 §6。

use crate::domain::agent::IdempotencyKey;
use crate::domain::participant::{ParticipantId, ParticipantKind};
use crate::domain::participant_avatar::{Avatar, AvatarMeta};
use crate::domain::participant_presentation::{
    BaseVersion, ParticipantProfileVersion, PresentationVersion, ProfileVersion, PublicDescription,
    RequestDigest, RoomDisplayName, RoomPresentationVersion, RoomRoleDescription, RoomRoleName,
    ShortDescription,
};
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::ports::room::ParticipantProfile;
use crate::ports::storage::StorageError;

/// Profile 幂等 PUT 的结果：`created=false` 表示同 key 同请求的重放，返回首次
/// 成功记录的结果（绝不追加第二个版本）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfilePutOutcome {
    /// 本次调用是否产生了新的版本行；false = 幂等重放。
    pub created: bool,
    /// 结果版本（新写入或重放的既有结果）。
    pub version: ProfileVersion,
    /// 该请求首次是否把 `participant_id` 从无事实推进到版本 1。
    pub result_is_create: bool,
    /// 可读的结果行（重放时是首个成功写入的行）。
    pub result: ParticipantProfileVersion,
}

/// Room Presentation 幂等 PUT 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomPresentationPutOutcome {
    pub created: bool,
    pub version: PresentationVersion,
    pub result_is_create: bool,
    pub result: RoomPresentationVersion,
}

/// 读取投影：最新 Profile 版本行 + 发布者 kind（`published_by_kind` 是派生
/// 投影，来源为 `published_by_id -> participants.kind`，**绝不**在版本行冗余
/// 存一份公共事实；读时以 JOIN 还原）+ avatar 公共投影（存在性 +
/// content_type + size_bytes；绝不含 storage key / SHA-256 / 路径）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRead {
    pub participant_id: ParticipantId,
    pub profile_version: ProfileVersion,
    pub short_description: Option<ShortDescription>,
    pub public_description: Option<PublicDescription>,
    pub avatar: Option<AvatarMeta>,
    pub published_by_id: ParticipantId,
    pub published_by_kind: ParticipantKind,
    pub published_at: UtcMillis,
}

/// 读取投影：最新 Room Presentation 版本行 + 发布者 kind（派生，同上）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomPresentationRead {
    pub room_id: RoomId,
    pub participant_id: ParticipantId,
    pub presentation_version: PresentationVersion,
    pub room_display_name: Option<RoomDisplayName>,
    pub room_role_name: Option<RoomRoleName>,
    pub room_role_description: Option<RoomRoleDescription>,
    pub published_by_id: ParticipantId,
    pub published_by_kind: ParticipantKind,
    pub published_at: UtcMillis,
}

/// Member Presentation 摘要单项：某 Room 当前 active 成员（Human 与 Agent）
/// 的最小公共资料 + 其全局 Profile 的 short_description + avatar 存在性
/// （无头像恒为 None）+ 其在该 Room 的最新 Presentation 文本字段
/// （Human 无 Presentation，字段恒为 None）。
/// `room_role_description` 返回**完整**存储值，`GET
/// /v1/rooms/{room_id}/member-presentations` 在读取时刻以
/// `domain::participant_presentation::preview` 生成 512 字节 preview，绝不
/// 在摘要响应里落 full description / public_description。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberPresentationSummary {
    pub participant: ParticipantProfile,
    pub short_description: Option<ShortDescription>,
    pub avatar: Option<AvatarMeta>,
    pub room_display_name: Option<RoomDisplayName>,
    pub room_role_name: Option<RoomRoleName>,
    pub room_role_description: Option<RoomRoleDescription>,
}

/// 头像读取（字节下载路径专用）：最新 Profile 版本行的内部完整头像事实
/// （含 storage_key 与 sha256——公共投影 `ProfileRead` 不含这些）。
/// `profile_version` 是该头像所在版本行的版本（缓存头按版本寻址）。只认
/// 最新版本行：摘除后（最新行 avatar=NULL）返回 None，历史版本行的头像
/// 不可恢复为当前头像。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestAvatar {
    pub profile_version: ProfileVersion,
    pub avatar: Avatar,
}

/// 存储层错误（内部 typed；application/HTTP 阶段按
/// CORE_V1_PARTICIPANT_PRESENTATION.md §7 映射到公共 code/状态）。
#[derive(Debug)]
pub enum StoreError {
    /// 目标参与者不存在 / 无 active Owner 关系（隐藏存在性）。
    /// 公共映射：404 `agent_not_found`。
    UnknownParticipant,
    /// 目标参与者不是 active（revoked 等）。
    /// 公共映射：409 `agent_not_active`。
    ParticipantNotActive,
    /// 目标 Room 不存在，或 actor 不是该 Room 的 active owner（隐藏存在性）。
    /// 公共映射：404 `room_not_found`。
    UnknownRoom,
    /// 目标 Room 不是 group（不支持 Room Presentation）。
    /// 公共映射：400 `room_presentation_not_supported`。
    NotGroupRoom,
    /// (room, participant) 不是该 group Room 的 active Agent 成员（隐藏存在性）。
    /// 公共映射：404 `member_not_found`。
    NotActiveAgentMember,
    /// actor 不是该 group Room 的 active owner（Human）。
    /// 公共映射：404 `room_not_found`（隐藏存在性）。
    NotGroupOwner,
    /// actor 不是目标 Agent 的 active Owner。
    /// 公共映射：404 `agent_not_found`（隐藏存在性）。
    NotAgentOwner,
    /// 同 key 但请求摘要不同：幂等冲突，首次事实不变。
    /// 公共映射：409 `idempotency_conflict`。
    IdempotencyConflict,
    /// 新 key 且 `base_version` 已过期（latest 已被其他写入推进）。
    /// 公共映射：409 `profile_stale`（Profile）/ `room_presentation_stale`
    /// （Room Presentation）。
    ConcurrentVersionChanged,
    /// 数据库或其他持久化失败（typed cause + 稳定分类）。
    Database(StorageError),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownParticipant => write!(f, "no such participant"),
            Self::ParticipantNotActive => write!(f, "participant is not active"),
            Self::UnknownRoom => write!(f, "no such room"),
            Self::NotGroupRoom => write!(f, "room is not a group room"),
            Self::NotActiveAgentMember => {
                write!(f, "participant is not an active agent member of the room")
            }
            Self::NotGroupOwner => write!(f, "publisher is not the active group owner"),
            Self::NotAgentOwner => write!(f, "publisher is not an active owner of the agent"),
            Self::IdempotencyConflict => {
                write!(f, "idempotency key already used with a different request")
            }
            Self::ConcurrentVersionChanged => write!(f, "base_version is stale"),
            Self::Database(e) => write!(f, "presentation store failure: {e}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(e) => Some(e),
            _ => None,
        }
    }
}

/// Participant Presentation 存储端口。
///
/// 事务契约：每个 `put_*` 在**单个 `BEGIN IMMEDIATE` 事务**内完成「读最新版本
/// → 裁决 base_version/幂等 → 插入不可变版本行 → 插入/复用幂等记录 → commit」；
/// 任何失败整体回滚；幂等表绝不携带正文/描述/预览。裁决**先认证与授权**，
/// 再处理重放：同 key + 同请求摘要恒重放既有成功结果（必要时读回记录的
/// `result_version`/`result_is_create`），**不因 base_version 过期变 stale**；
/// 同 key 不同摘要 → `IdempotencyConflict`；新 key 才要求
/// `base_version == 最新版本`。数据库守卫（deferred FK / 见证触发器）只证明
/// 引用与身份一致性，同事务原子性由本端口要求的单个 `BEGIN IMMEDIATE` 保证。
#[async_trait::async_trait]
pub trait ParticipantPresentationStore: Send + Sync {
    /// 幂等写入 Agent（P0；模型按 Participant 建模）全局 Profile 新版本。
    /// `record.published_by_id` 既是被授权并认证的 Human，也是幂等记录的 actor
    /// 与首次写入的发布者；授权（active Owner 关系）先于任何重放裁决——被撤销
    /// 的 actor 不得经重放访问结果。裁决规则：同 key + 同摘要 → 重放（读回既有
    /// `result`，不裁决 base_version）；同 key 不同摘要 → `IdempotencyConflict`；
    /// 新 key 要求 `base_version` == 当前最新（无行视为 0）且
    /// `record.profile_version` == 其 + 1。
    async fn put_profile_version(
        &self,
        key: &IdempotencyKey,
        request_digest: &RequestDigest,
        base_version: &BaseVersion,
        record: &ParticipantProfileVersion,
        now: crate::domain::time::UtcMillis,
    ) -> Result<ProfilePutOutcome, StoreError>;

    /// 幂等写入 Group Room Presentation 新版本。`record.published_by_id` 必须是
    /// 该 active group Room 的 active owner（Human），`record.participant_id`
    /// 必须是该 active group Room 的 active Agent 成员；裁决规则同 Profile
    /// （同 key 同摘要恒重放、同 key 不同摘要冲突、新 key 才要求
    /// `base_version` == 当前最新，(room, participant) 无行视为 0）。
    async fn put_room_presentation_version(
        &self,
        key: &IdempotencyKey,
        request_digest: &RequestDigest,
        base_version: &BaseVersion,
        record: &RoomPresentationVersion,
        now: crate::domain::time::UtcMillis,
    ) -> Result<RoomPresentationPutOutcome, StoreError>;

    /// 读取某 Participant 最新 Profile（含发布者 kind 派生投影）；无记录返回
    /// None（= 版本 0 / null 空投影）。
    async fn latest_profile(
        &self,
        participant_id: &ParticipantId,
    ) -> Result<Option<ProfileRead>, StoreError>;

    /// 读取某 (room, participant) 最新 Room Presentation（含发布者 kind 派生
    /// 投影）；无记录返回 None。
    async fn latest_room_presentation(
        &self,
        room_id: &RoomId,
        participant_id: &ParticipantId,
    ) -> Result<Option<RoomPresentationRead>, StoreError>;

    /// 某 Room 全部当前 active 成员（Human 与 Agent）的 Presentation 摘要
    /// （Participant 最小资料 + Profile short_description + avatar 存在性 +
    /// 最新 Presentation 文本字段；Human 成员无 Presentation，字段恒为 None）。
    /// 只做事实拼装，**不做可见性裁决**：调用方必须先经 RoomStore 确认当前
    /// actor 是该 Room 的 active member 后才调用，避免绕过既有成员可见性。
    /// 非 group Room 无 Presentation，字段一律为 None（direct 成员仍返回其
    /// 全局 Profile short_description 与 avatar 存在性）。
    async fn member_presentation_summaries(
        &self,
        room_id: &RoomId,
    ) -> Result<Vec<MemberPresentationSummary>, StoreError>;

    /// 读取某 Participant 最新 Profile 版本行中的头像完整事实（含内部
    /// storage_key 与 sha256，字节下载路径专用）。只认最新版本行：无版本行
    /// 或最新行 avatar=NULL（含摘除后）返回 None。
    async fn latest_avatar(
        &self,
        participant_id: &ParticipantId,
    ) -> Result<Option<LatestAvatar>, StoreError>;

    /// 全部 Profile 版本行引用的头像 storage key 集合（含历史版本——版本行
    /// 不可变，其字节必须被 orphan sweep 引用集合保护，绝不作为孤儿删除；
    /// 只回收从未被任何版本行引用的崩溃残留）。
    async fn avatar_storage_keys(&self) -> Result<std::collections::HashSet<String>, StoreError>;
}
