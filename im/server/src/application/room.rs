//! Direct Room 用例：创建（幂等）、列表、读取、成员读取、邀请/重入
//! （幂等）、移除、主动退群与群主原子转让。
//!
//! 用例层负责规则编排与事务边界：校验幂等键、把 agent_id / room_id /
//! participant_id 规范化、生成 Room ID 与 Membership 周期 ID、生成创建
//! 时间，再把已就绪的事实交给 `ports::room::RoomStore` 一次性持久化。
//! bad-id / unknown / not-owned 在应用层合并为统一的 NotFound，不泄露
//! 其他 Human 的 Agent 是否存在；Agent/Owner 撤销状态、同对已有 Room、
//! 邀请资格、转让资格与周期窗口边界都在存储事务内完成，不留半成品。

use crate::application::unexpected_store_error;
use crate::domain::agent::IdempotencyKey;
use crate::domain::message::MessageId;
use crate::domain::participant::{Participant, ParticipantId};
use crate::domain::room::{AttentionLevel, MembershipId, ReadState, RoomId};
use crate::domain::time::UtcMillis;
use crate::ports::room::{
    AdvanceReadCursorOutcome as StoreAdvanceOutcome, CreateDirectRoomOutcome as StoreCreateOutcome,
    CreateRoomOutcome as StoreCreateRoomOutcome, InviteMemberOutcome as StoreInviteOutcome,
    MemberView, NewDirectRoom, NewGroup, NewHumanDirectRoom, RoomListItem, RoomStore,
    RoomStoreError, TransferOwnerOutcome as StoreTransferOutcome,
};
use crate::ports::storage::StorageError;

/// 创建结果：首次创建与幂等重放返回相同的响应模型，以
/// `idempotency_replayed` 区分（HTTP 201 / 200）。
#[derive(Debug)]
pub enum CreateDirectRoomOutcome {
    Created {
        room: crate::domain::room::Room,
        memberships: Vec<crate::domain::room::Membership>,
    },
    Replayed {
        room: crate::domain::room::Room,
        memberships: Vec<crate::domain::room::Membership>,
    },
}

#[derive(Debug)]
pub enum CreateDirectRoomError {
    /// key 格式不合法（长度、字符集、空白、逗号）。
    InvalidIdempotencyKey,
    /// bad-id、unknown Agent、当前 Human 从未拥有该 Agent。
    NotFound,
    /// 同 Human + 同 key 已被不同 Agent 使用。
    IdempotencyConflict,
    /// 当前 Human 曾经拥有，但 Agent 或 Owner 关系已 revoked。
    AgentNotActive,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for CreateDirectRoomError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdempotencyKey => write!(f, "invalid idempotency key"),
            Self::NotFound => write!(f, "agent not found"),
            Self::IdempotencyConflict => {
                write!(
                    f,
                    "idempotency key was already used with a different request"
                )
            }
            Self::AgentNotActive => write!(f, "agent or owner relationship is revoked"),
            Self::Store(e) => write!(f, "room store failure: {e}"),
        }
    }
}

impl std::error::Error for CreateDirectRoomError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_create_error(e: RoomStoreError) -> CreateDirectRoomError {
    match e {
        RoomStoreError::NotFound => CreateDirectRoomError::NotFound,
        RoomStoreError::IdempotencyConflict => CreateDirectRoomError::IdempotencyConflict,
        RoomStoreError::AgentNotActive => CreateDirectRoomError::AgentNotActive,
        RoomStoreError::Database(e) => CreateDirectRoomError::Store(e),
        other => CreateDirectRoomError::Store(unexpected_store_error("room store error", other)),
    }
}

/// 创建 Direct Room：校验 key → 规范化 agent_id → 生成
/// Room ID 与两条 Membership 周期 ID → 原子持久化 Room + 配对 + 两份
/// Membership + 幂等记录（幂等事实包含规范化标题）。
///
/// 裁决顺序（契约 7.1）：认证/请求体与 agent_id 校验 → 标题校验 → 既有
/// 幂等事实（同 key 重放 → 同一间房）→ valid-but-unknown/not-owned 404 →
/// Agent/Owner revoked 409 → 201。
///
/// **一间房 = 一段关系**（批次二，2026-09-27 主人裁决）：该 (human, agent) 的
/// 私人房**至多一间**（每段 Agent Owner 关系一间）。再要一次**不报错、不新开**：
/// 返回那间**已存在**的私人房（`Replayed`）——IM 里"再点一次同一个联系人"就是
/// 回到你们那间房，不是再开一间。服务房是**另一段关系**，走另一条端点。
///
/// **没有 `name`**：direct 房没有标题（`rooms.name` 只供 group）。
pub async fn create_direct_room(
    store: &dyn RoomStore,
    human: &Participant,
    agent_id_raw: &str,
    idempotency_key_raw: &str,
) -> Result<CreateDirectRoomOutcome, CreateDirectRoomError> {
    let key = IdempotencyKey::parse(idempotency_key_raw)
        .map_err(|_| CreateDirectRoomError::InvalidIdempotencyKey)?;
    let agent_id =
        ParticipantId::parse_canonical(agent_id_raw).ok_or(CreateDirectRoomError::NotFound)?;
    let created_at = UtcMillis::now();
    let room_id = RoomId::new_v4();

    match store
        .create_direct_room(
            &human.id,
            &NewDirectRoom {
                room_id,
                human_id: human.id.clone(),
                agent_id,
                human_membership_id: MembershipId::new_v4(),
                agent_membership_id: MembershipId::new_v4(),
                created_at,
            },
            &key,
        )
        .await
    {
        Ok(StoreCreateOutcome::Created { room, memberships }) => {
            Ok(CreateDirectRoomOutcome::Created { room, memberships })
        }
        Ok(StoreCreateOutcome::Replayed { room, memberships }) => {
            Ok(CreateDirectRoomOutcome::Replayed { room, memberships })
        }
        Err(e) => Err(map_create_error(e)),
    }
}

/// 查询类失败：unknown / bad-id / 非成员统一为 NotFound。
#[derive(Debug)]
pub enum RoomQueryError {
    NotFound,
    Store(StorageError),
}

impl std::fmt::Display for RoomQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "room not found"),
            Self::Store(e) => write!(f, "room store failure: {e}"),
        }
    }
}

impl std::error::Error for RoomQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_query_error(e: RoomStoreError) -> RoomQueryError {
    match e {
        RoomStoreError::Database(e) => RoomQueryError::Store(e),
        other => RoomQueryError::Store(unexpected_store_error("room store error", other)),
    }
}

/// 创建 human_direct / group 的结果：首次创建与幂等重放返回相同的响应
/// 模型，以 `idempotency_replayed` 区分（HTTP 201 / 200）。
#[derive(Debug)]
pub enum CreateRoomOutcome {
    Created {
        room: crate::domain::room::Room,
        memberships: Vec<crate::domain::room::Membership>,
    },
    Replayed {
        room: crate::domain::room::Room,
        memberships: Vec<crate::domain::room::Membership>,
    },
}

/// 创建 human_direct / group 失败。
#[derive(Debug)]
pub enum CreateRoomError {
    /// key 格式不合法（长度、字符集、空白、逗号）。
    InvalidIdempotencyKey,
    /// 目标 Human 不存在、不是 active Human 或已 revoked：404。
    HumanNotFound,
    /// 目标 Human 存在但当前 Human 与它没有 accepted 联系人关系：409。
    ContactNotAccepted,
    /// 同一对 Human 已由不同 idempotency key 创建：409。
    HumanDirectRoomExists,
    /// 同 Human + 同 key 已被不同请求使用：409。
    IdempotencyConflict,
    /// 群名不合法（空、超 80 个 scalar、含 NUL 或纯空白）：400。
    InvalidGroupName,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for CreateRoomError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdempotencyKey => write!(f, "invalid idempotency key"),
            Self::HumanNotFound => write!(f, "human not found"),
            Self::ContactNotAccepted => {
                write!(
                    f,
                    "the contact relationship with this human is not accepted"
                )
            }
            Self::HumanDirectRoomExists => {
                write!(f, "a human direct room already exists for this human pair")
            }
            Self::IdempotencyConflict => {
                write!(
                    f,
                    "idempotency key was already used with a different request"
                )
            }
            Self::InvalidGroupName => write!(f, "invalid group name"),
            Self::Store(e) => write!(f, "room store failure: {e}"),
        }
    }
}

impl std::error::Error for CreateRoomError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_create_room_error(e: RoomStoreError) -> CreateRoomError {
    match e {
        RoomStoreError::HumanNotFound => CreateRoomError::HumanNotFound,
        RoomStoreError::ContactNotAccepted => CreateRoomError::ContactNotAccepted,
        RoomStoreError::HumanDirectRoomExists => CreateRoomError::HumanDirectRoomExists,
        RoomStoreError::IdempotencyConflict => CreateRoomError::IdempotencyConflict,
        RoomStoreError::Database(e) => CreateRoomError::Store(e),
        other => CreateRoomError::Store(unexpected_store_error("room store error", other)),
    }
}

/// 为 accepted 联系人创建 Human direct Room：校验 key → 规范化目标 →
/// 生成 Room ID 与两条 Membership 周期 ID → 原子持久化 Room + 覆盖 +
/// 配对 + 两份 Membership + 幂等记录。裁决顺序：认证/请求体与 key 校验 →
/// 既有幂等事实（同目标重放、不同目标 409）→ 目标 Human 存在且 active
/// （404）→ accepted 联系人（409）→ 同对已有 Room（409）→ 201。
pub async fn create_human_direct_room(
    store: &dyn RoomStore,
    human: &Participant,
    other_human_raw: &str,
    idempotency_key_raw: &str,
) -> Result<CreateRoomOutcome, CreateRoomError> {
    let key = IdempotencyKey::parse(idempotency_key_raw)
        .map_err(|_| CreateRoomError::InvalidIdempotencyKey)?;
    let other_id =
        ParticipantId::parse_canonical(other_human_raw).ok_or(CreateRoomError::HumanNotFound)?;
    let created_at = UtcMillis::now();
    let room_id = RoomId::new_v4();

    match store
        .create_human_direct_room(
            &human.id,
            &NewHumanDirectRoom {
                room_id,
                creator_id: human.id.clone(),
                other_id,
                creator_membership_id: MembershipId::new_v4(),
                other_membership_id: MembershipId::new_v4(),
                created_at,
            },
            &key,
        )
        .await
    {
        Ok(StoreCreateRoomOutcome::Created { room, memberships }) => {
            Ok(CreateRoomOutcome::Created { room, memberships })
        }
        Ok(StoreCreateRoomOutcome::Replayed { room, memberships }) => {
            Ok(CreateRoomOutcome::Replayed { room, memberships })
        }
        Err(e) => Err(map_create_room_error(e)),
    }
}

/// 创建 group Room：校验 key → 校验群名（与显示名同规则）→ 生成 Room ID
/// 与 owner Membership 周期 ID → 原子持久化 Room + 覆盖 + owner
/// Membership + 幂等记录；创建者自动成为 owner。同 Human + key 同群名
/// 重放、异群名 409。
pub async fn create_group(
    store: &dyn RoomStore,
    human: &Participant,
    name_raw: &str,
    idempotency_key_raw: &str,
) -> Result<CreateRoomOutcome, CreateRoomError> {
    let key = IdempotencyKey::parse(idempotency_key_raw)
        .map_err(|_| CreateRoomError::InvalidIdempotencyKey)?;
    let name = crate::domain::participant::DisplayName::parse(name_raw)
        .map_err(|_| CreateRoomError::InvalidGroupName)?
        .as_str()
        .to_owned();
    let created_at = UtcMillis::now();
    let room_id = RoomId::new_v4();

    match store
        .create_group(
            &human.id,
            &NewGroup {
                room_id,
                owner_id: human.id.clone(),
                owner_membership_id: MembershipId::new_v4(),
                name,
                created_at,
            },
            &key,
        )
        .await
    {
        Ok(StoreCreateRoomOutcome::Created { room, memberships }) => {
            Ok(CreateRoomOutcome::Created { room, memberships })
        }
        Ok(StoreCreateRoomOutcome::Replayed { room, memberships }) => {
            Ok(CreateRoomOutcome::Replayed { room, memberships })
        }
        Err(e) => Err(map_create_room_error(e)),
    }
}

/// group 邀请/重入结果：首次创建或重入 201 / 同 key 重放 200（replayed）/
/// 新 key 但目标已有 active 周期 200（不重复打开周期）。
#[derive(Debug)]
pub enum InviteMemberOutcome {
    Created {
        membership: crate::domain::room::Membership,
    },
    Replayed {
        membership: crate::domain::room::Membership,
    },
    AlreadyActive {
        membership: crate::domain::room::Membership,
    },
}

/// group 邀请/重入失败。
#[derive(Debug)]
pub enum InviteMemberError {
    /// key 格式不合法（长度、字符集、空白、逗号）。
    InvalidIdempotencyKey,
    /// unknown / bad-id / 非 owner / 非 group Room / 调用者无权限：统一
    /// 404 room_not_found，不泄露 Room 是否存在。
    RoomNotFound,
    /// 邀请目标不是可邀请对象（未知、非 active、非 accepted 联系人、
    /// 非自己拥有的 active Agent）：统一 404。
    MemberNotFound,
    /// 同 Human + 同 key 已被不同请求使用：409。
    IdempotencyConflict,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for InviteMemberError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdempotencyKey => write!(f, "invalid idempotency key"),
            Self::RoomNotFound => write!(f, "room not found"),
            Self::MemberNotFound => write!(f, "member not found"),
            Self::IdempotencyConflict => {
                write!(
                    f,
                    "idempotency key was already used with a different request"
                )
            }
            Self::Store(e) => write!(f, "room store failure: {e}"),
        }
    }
}

impl std::error::Error for InviteMemberError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_invite_error(e: RoomStoreError) -> InviteMemberError {
    match e {
        RoomStoreError::NotFound => InviteMemberError::RoomNotFound,
        RoomStoreError::MemberNotFound => InviteMemberError::MemberNotFound,
        RoomStoreError::IdempotencyConflict => InviteMemberError::IdempotencyConflict,
        RoomStoreError::Database(e) => InviteMemberError::Store(e),
        other => InviteMemberError::Store(unexpected_store_error("room store error", other)),
    }
}

/// group owner 邀请 accepted Human 或自己拥有且 active 的 Agent（必须
/// 携带 Idempotency-Key，先查 key 再查权限）：直接生成 active Membership
/// 周期，不造邀请状态机；同 key 重放返回历史结果；目标已有 active 周期
/// 时用新 key 返回 200 并记录；历史 closed 周期可用新 key 重入。
pub async fn invite_group_member(
    store: &dyn RoomStore,
    owner: &Participant,
    room_id_raw: &str,
    participant_raw: &str,
    idempotency_key_raw: &str,
) -> Result<InviteMemberOutcome, InviteMemberError> {
    let key = IdempotencyKey::parse(idempotency_key_raw)
        .map_err(|_| InviteMemberError::InvalidIdempotencyKey)?;
    let room_id = RoomId::parse_canonical(room_id_raw).ok_or(InviteMemberError::RoomNotFound)?;
    let participant_id =
        ParticipantId::parse_canonical(participant_raw).ok_or(InviteMemberError::MemberNotFound)?;
    match store
        .invite_group_member(&owner.id, &room_id, &participant_id, &key)
        .await
    {
        Ok(StoreInviteOutcome::Created { membership }) => {
            Ok(InviteMemberOutcome::Created { membership })
        }
        Ok(StoreInviteOutcome::Replayed { membership }) => {
            Ok(InviteMemberOutcome::Replayed { membership })
        }
        Ok(StoreInviteOutcome::AlreadyActive { membership }) => {
            Ok(InviteMemberOutcome::AlreadyActive { membership })
        }
        Err(e) => Err(map_invite_error(e)),
    }
}

/// 移除 group 成员的失败。
#[derive(Debug)]
pub enum RemoveMemberError {
    /// unknown / bad-id / 非 owner / 非 group Room：统一 404。
    RoomNotFound,
    /// 目标不是该 Room 的 active 成员：统一 404。
    MemberNotFound,
    /// 尝试移除 owner 自己。
    OwnerNotRemovable,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for RemoveMemberError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RoomNotFound => write!(f, "room not found"),
            Self::MemberNotFound => write!(f, "member not found"),
            Self::OwnerNotRemovable => write!(f, "the group owner cannot be removed"),
            Self::Store(e) => write!(f, "room store failure: {e}"),
        }
    }
}

impl std::error::Error for RemoveMemberError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_remove_error(e: RoomStoreError) -> RemoveMemberError {
    match e {
        RoomStoreError::NotFound => RemoveMemberError::RoomNotFound,
        RoomStoreError::MemberNotFound => RemoveMemberError::MemberNotFound,
        RoomStoreError::OwnerNotRemovable => RemoveMemberError::OwnerNotRemovable,
        RoomStoreError::Database(e) => RemoveMemberError::Store(e),
        other => RemoveMemberError::Store(unexpected_store_error("room store error", other)),
    }
}

/// group owner 移除 active 成员（Membership 周期进入 closed 终态，
/// closed_by=removed；返回被关闭的 membership_id，供 post-commit 唤醒
/// 目标查询）；被移除者不能读取或发消息；重入需新 key 打开新周期。
pub async fn remove_group_member(
    store: &dyn RoomStore,
    owner: &Participant,
    room_id_raw: &str,
    participant_raw: &str,
) -> Result<MembershipId, RemoveMemberError> {
    let room_id = RoomId::parse_canonical(room_id_raw).ok_or(RemoveMemberError::RoomNotFound)?;
    let participant_id =
        ParticipantId::parse_canonical(participant_raw).ok_or(RemoveMemberError::MemberNotFound)?;
    store
        .remove_group_member(&owner.id, &room_id, &participant_id)
        .await
        .map_err(map_remove_error)
}

/// 主动退群的失败。
#[derive(Debug)]
pub enum LeaveMembershipError {
    /// unknown / bad-id / 非 group Room / 当前 Human 无 active 周期：统一
    /// 404。
    RoomNotFound,
    /// group owner 主动退群：必须先转让（409 owner_transfer_required）。
    OwnerTransferRequired,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for LeaveMembershipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RoomNotFound => write!(f, "room not found"),
            Self::OwnerTransferRequired => {
                write!(f, "the group owner must transfer ownership before leaving")
            }
            Self::Store(e) => write!(f, "room store failure: {e}"),
        }
    }
}

impl std::error::Error for LeaveMembershipError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_leave_error(e: RoomStoreError) -> LeaveMembershipError {
    match e {
        RoomStoreError::NotFound => LeaveMembershipError::RoomNotFound,
        RoomStoreError::OwnerTransferRequired => LeaveMembershipError::OwnerTransferRequired,
        RoomStoreError::Database(e) => LeaveMembershipError::Store(e),
        other => LeaveMembershipError::Store(unexpected_store_error("room store error", other)),
    }
}

/// 成员主动退出一间房：自己的 Membership 周期进入 closed 终态
/// （closed_by=leave；返回被关闭的 membership_id，供 post-commit 唤醒
/// 目标查询），此后对 Room 的所有读写 404；owner 必须先转让。
///
/// **一条规则**（2026-09-27 M2 切片）：非 owner 的成员周期，可以由**它自己**
/// 以 `closed_by='leave'` 关闭——不分房 kind、不分参与者 kind。
/// 例外一处：`human_direct` 房的出口归关系模型（`contact_ended` + 精确见证），
/// 不走这里。owner 一律先转让（`OwnerTransferRequired`）。
///
/// 入参是**参与者身份**而不是 `Participant`：退出只需要「我是谁」，不需要
/// 显示名/状态/创建时刻（与 `advance_read_cursor` 同一形状——一条用例，
/// 两个适配器共用）。
pub async fn leave_membership(
    store: &dyn RoomStore,
    participant_id: &ParticipantId,
    room_id_raw: &str,
) -> Result<MembershipId, LeaveMembershipError> {
    let room_id = RoomId::parse_canonical(room_id_raw).ok_or(LeaveMembershipError::RoomNotFound)?;
    store
        .leave_membership(participant_id, &room_id)
        .await
        .map_err(map_leave_error)
}

/// 群主转让结果：`Transferred` / `Replayed`（同 key 重放，200，以
/// `idempotency_replayed` 区分）。
#[derive(Debug)]
pub enum TransferOwnerOutcome {
    Transferred {
        membership: crate::domain::room::Membership,
        /// 不可变 transition 事实 ID（post-commit 唤醒目标查询的 source ID）。
        transition_id: String,
    },
    Replayed {
        membership: crate::domain::room::Membership,
    },
}

/// 群主转让失败。
#[derive(Debug)]
pub enum TransferOwnerError {
    /// key 格式不合法（长度、字符集、空白、逗号）。
    InvalidIdempotencyKey,
    /// unknown / bad-id / 非 group Room / 调用者不是 active owner：统一
    /// 404 room_not_found。
    RoomNotFound,
    /// 目标不是该 Room 当前 active 的成员（未知、已 closed、revoked、不在本房）：
    /// 统一 404 member_not_found。**不分参与者 kind**——Agent 与 Human 同一条判据
    /// （2026-09-27 批次三第一刀）。
    MemberNotFound,
    /// 目标就是转让者自己：400 invalid_transfer_target。
    InvalidTransferTarget,
    /// 同 Human + 同 key 已被不同请求使用：409。
    IdempotencyConflict,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for TransferOwnerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdempotencyKey => write!(f, "invalid idempotency key"),
            Self::RoomNotFound => write!(f, "room not found"),
            Self::MemberNotFound => write!(f, "member not found"),
            Self::InvalidTransferTarget => {
                write!(f, "the transfer target must be a different active member")
            }
            Self::IdempotencyConflict => {
                write!(
                    f,
                    "idempotency key was already used with a different request"
                )
            }
            Self::Store(e) => write!(f, "room store failure: {e}"),
        }
    }
}

impl std::error::Error for TransferOwnerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_transfer_error(e: RoomStoreError) -> TransferOwnerError {
    match e {
        RoomStoreError::NotFound => TransferOwnerError::RoomNotFound,
        RoomStoreError::MemberNotFound => TransferOwnerError::MemberNotFound,
        RoomStoreError::InvalidTransferTarget => TransferOwnerError::InvalidTransferTarget,
        RoomStoreError::IdempotencyConflict => TransferOwnerError::IdempotencyConflict,
        RoomStoreError::Database(e) => TransferOwnerError::Store(e),
        other => TransferOwnerError::Store(unexpected_store_error("room store error", other)),
    }
}

/// 群主原子转让（必须携带 Idempotency-Key，先查 key 再查权限）：目标
/// 必须是当前 active 的 Human member 且不能是自己；存储事务内完成关闭/
/// 打开/coverage 见证/幂等记录，任何一步失败整体回滚。
pub async fn transfer_group_owner(
    store: &dyn RoomStore,
    owner: &Participant,
    room_id_raw: &str,
    target_raw: &str,
    idempotency_key_raw: &str,
) -> Result<TransferOwnerOutcome, TransferOwnerError> {
    let key = IdempotencyKey::parse(idempotency_key_raw)
        .map_err(|_| TransferOwnerError::InvalidIdempotencyKey)?;
    let room_id = RoomId::parse_canonical(room_id_raw).ok_or(TransferOwnerError::RoomNotFound)?;
    let target_id =
        ParticipantId::parse_canonical(target_raw).ok_or(TransferOwnerError::MemberNotFound)?;
    match store
        .transfer_group_owner(&owner.id, &room_id, &target_id, &key)
        .await
    {
        Ok(StoreTransferOutcome::Transferred {
            membership,
            transition_id,
        }) => Ok(TransferOwnerOutcome::Transferred {
            membership,
            transition_id,
        }),
        Ok(StoreTransferOutcome::Replayed { membership }) => {
            Ok(TransferOwnerOutcome::Replayed { membership })
        }
        Err(e) => Err(map_transfer_error(e)),
    }
}

/// 列出当前 Human 存在 active Membership 周期的 Room，按创建时间升序、
/// 再按 Room ID 排序；Agent revoked 后仍可读取（Human 周期保持 active）。
pub async fn list_rooms(
    store: &dyn RoomStore,
    human_id: &ParticipantId,
) -> Result<Vec<RoomListItem>, RoomQueryError> {
    store.list_rooms(human_id).await.map_err(map_query_error)
}

/// 读取当前 Human 存在 active Membership 周期的单个 Room；unknown、
/// bad-id、not-member 统一 NotFound（HTTP 404 room_not_found）。
pub async fn get_room(
    store: &dyn RoomStore,
    human_id: &ParticipantId,
    room_id_raw: &str,
) -> Result<RoomListItem, RoomQueryError> {
    let room_id = RoomId::parse_canonical(room_id_raw).ok_or(RoomQueryError::NotFound)?;
    store
        .get_room(human_id, &room_id)
        .await
        .map_err(map_query_error)?
        .ok_or(RoomQueryError::NotFound)
}

/// 读取当前 Human 存在 active Membership 周期的 Room 的全部公开
/// Membership 周期；unknown、bad-id、not-member 统一 NotFound。
pub async fn get_room_members(
    store: &dyn RoomStore,
    human_id: &ParticipantId,
    room_id_raw: &str,
) -> Result<Vec<MemberView>, RoomQueryError> {
    let room_id = RoomId::parse_canonical(room_id_raw).ok_or(RoomQueryError::NotFound)?;
    store
        .get_room_members(human_id, &room_id)
        .await
        .map_err(map_query_error)?
        .ok_or(RoomQueryError::NotFound)
}

/// read cursor 推进结果：`Advanced`（真实推进，携带 advancement_id 供
/// post-commit 定向唤醒）/ `NoOp`（不高于当前，成功幂等）。
#[derive(Debug)]
pub enum AdvanceReadCursorOutcome {
    Advanced {
        read_state: ReadState,
        advancement_id: String,
    },
    NoOp {
        read_state: ReadState,
    },
}

/// read cursor 推进失败。
#[derive(Debug)]
pub enum AdvanceReadCursorError {
    /// bad-id / unknown Room / 当前 Human 无 active Membership：统一 404
    /// room_not_found，不泄露 Room 是否存在。
    RoomNotFound,
    /// 目标 message_id 不是 UUID v4：400 invalid_request。
    InvalidMessageId,
    /// Message 未知、属于其他 Room 或对当前 Human 的历史 Membership
    /// 窗口不可见：统一 400 invalid_read_cursor，不泄露具体原因。
    InvalidReadCursor,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for AdvanceReadCursorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RoomNotFound => write!(f, "room not found"),
            Self::InvalidMessageId => write!(f, "message id must be a UUID v4"),
            Self::InvalidReadCursor => write!(f, "the message cannot be the read cursor target"),
            Self::Store(e) => write!(f, "room store failure: {e}"),
        }
    }
}

impl std::error::Error for AdvanceReadCursorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_advance_error(e: RoomStoreError) -> AdvanceReadCursorError {
    match e {
        RoomStoreError::NotFound => AdvanceReadCursorError::RoomNotFound,
        RoomStoreError::InvalidReadCursor => AdvanceReadCursorError::InvalidReadCursor,
        RoomStoreError::Database(e) => AdvanceReadCursorError::Store(e),
        other => AdvanceReadCursorError::Store(unexpected_store_error("room store error", other)),
    }
}

/// 推进该主体在该 Room 的 read cursor（契约 `CORE_V1_IM_CONTRACT.md` §9；
/// Agent 侧见 `CORE_V1_AGENT_READ_CURSOR.md`）：解析并规范化 room_id 与
/// message_id（message_id 非 UUID v4 为 400 invalid_request）后交给存储层
/// 原子裁决；目标不高于当前 cursor 返回 `NoOp`（不产生 advancement /
/// 事件），更高时返回 `Advanced`（携带 advancement_id，Human 侧
/// post-commit 唤醒用）。失败不改变 cursor，不生成 advancement / 事件。
///
/// Human 与 Agent 共用本函数（身份只来自调用方的认证上下文）；两者唯一的
/// 差别在存储层的事件面：Agent 的推进不生成 Human Event。
pub async fn advance_read_cursor(
    store: &dyn RoomStore,
    participant_id: &ParticipantId,
    room_id_raw: &str,
    message_id_raw: &str,
) -> Result<AdvanceReadCursorOutcome, AdvanceReadCursorError> {
    let room_id =
        RoomId::parse_canonical(room_id_raw).ok_or(AdvanceReadCursorError::RoomNotFound)?;
    let message_id = MessageId::parse_canonical(message_id_raw)
        .ok_or(AdvanceReadCursorError::InvalidMessageId)?;
    match store
        .advance_read_cursor(participant_id, &room_id, &message_id)
        .await
    {
        Ok(StoreAdvanceOutcome::Advanced {
            read_state,
            advancement_id,
        }) => Ok(AdvanceReadCursorOutcome::Advanced {
            read_state,
            advancement_id,
        }),
        Ok(StoreAdvanceOutcome::NoOp { read_state }) => {
            Ok(AdvanceReadCursorOutcome::NoOp { read_state })
        }
        Err(e) => Err(map_advance_error(e)),
    }
}

/// 提醒档写失败（契约 `CORE_V1_IM_CONTRACT.md` §9.1）。
#[derive(Debug)]
pub enum SetAttentionError {
    /// bad-id / unknown Room / 非 group / 本人不是该 Room 的 active 成员：
    /// 统一 404 room_not_found，与非成员同形，不泄露 Room 是否存在。
    RoomNotFound,
    /// `level` 不在闭集 `{all, mentions}`：400 invalid_request。
    InvalidLevel,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for SetAttentionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RoomNotFound => write!(f, "room not found"),
            Self::InvalidLevel => write!(f, "level must be all or mentions"),
            Self::Store(e) => write!(f, "room store failure: {e}"),
        }
    }
}

impl std::error::Error for SetAttentionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn map_attention_error(e: RoomStoreError) -> SetAttentionError {
    match e {
        RoomStoreError::NotFound => SetAttentionError::RoomNotFound,
        RoomStoreError::Database(e) => SetAttentionError::Store(e),
        other => SetAttentionError::Store(unexpected_store_error("room store error", other)),
    }
}

/// 写**本人**在这间房的提醒档（契约 `CORE_V1_IM_CONTRACT.md` §9.1）。
///
/// 三条裁决都在**用例这一层定死**，两条认证面（Human / Backend）逐条同形：
/// - `level` 必须落在闭集 `{all, mentions}`（否则 400 invalid_request）——
///   闭集在写入口就判，不把"认不出的词"交给数据库 CHECK 变成一个 500；
/// - `room_id` 必须是规范 UUID v4，且**本人是该房 active 成员 + 该房是
///   `group`**（两者同一出口：404 room_not_found，与非成员同形，不泄露）；
/// - **没有 `participant_id` 入参的来路**：身份只来自认证上下文 ⇒
///   "只有本人能写"由调用形状保证，不由参数校验保证。
///
/// 状态写、无幂等键、零事件、零唤醒；同值重写天然幂等。
pub async fn set_room_attention(
    store: &dyn RoomStore,
    participant_id: &ParticipantId,
    room_id_raw: &str,
    level_raw: &str,
) -> Result<AttentionLevel, SetAttentionError> {
    let level = AttentionLevel::parse(level_raw).ok_or(SetAttentionError::InvalidLevel)?;
    let room_id = RoomId::parse_canonical(room_id_raw).ok_or(SetAttentionError::RoomNotFound)?;
    store
        .set_room_attention(participant_id, &room_id, level)
        .await
        .map_err(map_attention_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::storage::StorageFailureKind;
    use std::error::Error;
    /// 应用 source 链：`RoomStoreError::Database(StorageError)` 经应用映射后，
    /// 从应用错误沿 `Error::source()` 下钻到 `StorageError`，再到底层 cause；
    /// 应用错误 Display 只含稳定文案，不含 cause 文本。
    #[test]
    fn application_error_chain_reaches_storage_error_and_original_cause() {
        let storage = StorageError::from_message(
            StorageFailureKind::BusyOrLocked,
            "database is locked: injected path=/tmp/secret/core.db",
        );
        let app_err = map_create_error(RoomStoreError::Database(storage.clone()));

        let CreateDirectRoomError::Store(inner) = app_err else {
            panic!("Database must map to Store");
        };
        assert_eq!(inner.kind(), StorageFailureKind::BusyOrLocked);
        let display = inner.to_string();
        assert!(
            !display.contains("/tmp") && !display.contains("core.db"),
            "application Display must not leak cause text: {display:?}"
        );

        // 应用错误 → StorageError：RoomStoreError 已消费，链从应用错误开始。
        let storage_from_app = inner;
        let cause = storage_from_app
            .source()
            .expect("StorageError must expose its cause");
        assert_eq!(
            cause.to_string(),
            "database is locked: injected path=/tmp/secret/core.db"
        );
    }

    /// 未预期的业务变体映射为 `Unknown`（HTTP 500 internal_error），cause
    /// 链保留原变体的稳定 Display 作为诊断消息。
    #[test]
    fn unexpected_business_variant_maps_to_unknown_kind_with_cause() {
        let app_err = map_create_error(RoomStoreError::HumanNotFound);
        let CreateDirectRoomError::Store(inner) = app_err else {
            panic!("unexpected variant must map to Store");
        };
        assert_eq!(inner.kind(), StorageFailureKind::Unknown);
        let cause = inner.source().expect("diagnostic cause present");
        assert!(
            cause.to_string().contains("unexpected room store error"),
            "cause must identify the unexpected variant: {cause}"
        );
        assert!(
            !inner.to_string().contains("unexpected room store error"),
            "Display must stay stable even for the unexpected path: {}",
            inner
        );
    }

    /// 端口错误本身也实现 `Error::source()`：Database 变体可下钻到
    /// StorageError。
    #[test]
    fn port_error_source_chain_reaches_storage_error() {
        let storage = StorageError::from_message(StorageFailureKind::Constraint, "fk violated");
        let port_err = RoomStoreError::Database(storage);
        let source = port_err
            .source()
            .expect("port Database variant must expose the StorageError");
        assert_eq!(
            source.to_string(),
            "storage failure: constraint",
            "port Display stays stable while source keeps the typed error"
        );
    }
}
