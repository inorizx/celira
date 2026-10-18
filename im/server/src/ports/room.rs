//! Room、Membership 周期与房间幂等窄端口。
//!
//! 本端口只声明 Room 切片的持久化边界，不依赖具体数据库或 HTTP 框架。
//! SQLite adapter 实现本端口；应用用例只通过端口消费存储能力。
//! Agent revoke 对 Agent Membership 周期的关闭由数据库事务触发器在同一
//! SQLite 事务内完成，不属于本端口。
//!
//! Room kind 语义（CORE_V1_HUMAN_ROOMS_AND_MENTIONS.md 4.3）：
//! - `direct`：Human–Agent Direct Room（Human owner + Agent member）；
//! - `human_direct`：两个 accepted Human 的私聊（创建者 owner + 另一端
//!   member，role 不产生全局好友之外的权限）；
//! - `group`：多 Human/Agent 群聊（创建者 owner，其他加入者为 member；
//!   owner 直接邀请 accepted Human 或自己拥有的 active Agent，直接生成
//!   active Membership 周期，不造邀请状态机；owner 可移除非 owner 成员、
//!   成员可主动退群、owner 可原子转让）。
//!
//! Membership 周期模型（migration 004）：每行一条不可变周期，由
//! membership_id 标识；active 周期 left 三元组为空；关闭后
//! left_at/left_before_sequence/closed_by 固化、整行不可变；同
//! Room+Participant 至多一个 active 周期；重入 = 新周期。invite 必须携带
//! Idempotency-Key 并先查 key 再查权限（同 key 重试只重放历史结果）；
//! transfer 在一个 BEGIN IMMEDIATE 事务内完成关闭/打开/插入不可变
//! `group_owner_transitions` 事实/coverage 见证/幂等记录，任何一步失败
//! 整体回滚；新 owner 周期 INSERT 守卫只接受「该 Room 尚无任何
//! Membership」或「同一事务内精确匹配的 transition」两种来路。

use crate::domain::agent::IdempotencyKey;
use crate::domain::message::MessageId;
use crate::domain::participant::{DisplayName, ParticipantId, ParticipantKind, ParticipantStatus};
use crate::domain::room::{Membership, MembershipId, ReadState, Room, RoomId};
use crate::domain::time::UtcMillis;
use crate::ports::storage::StorageError;

/// 待持久化的新 Direct Room：Room、配对与两条奠基 Membership 周期的数据
/// 来源（membership_id 由应用层生成，供配对行见证）。
///
/// **没有 name**（批次二，2026-09-27 主人裁决）：direct 房**没有标题**——
/// 房的身份是它见证的那段关系，不是它叫什么。`rooms.name` 这一列保留，但只
/// 供 `group` 用（direct 房恒 NULL，由 database 守卫钉住）。
/// 见证的关系就是**一段 Agent Owner 关系**（`direct_room_pairs` 上
/// `UNIQUE(human_id, agent_id)` ⇒ 每段关系至多一间房）。
/// ⚠ 2026-09-28：原来还有第二种来路（服务房 / Service Access），已随服务层
/// 退役删除。
#[derive(Debug, Clone)]
pub struct NewDirectRoom {
    pub room_id: RoomId,
    pub human_id: ParticipantId,
    pub agent_id: ParticipantId,
    pub human_membership_id: MembershipId,
    pub agent_membership_id: MembershipId,
    pub created_at: UtcMillis,
}

/// 待持久化的新 Human direct Room：Room、两端 Human 与两条奠基 Membership
/// 周期的数据来源（创建者 role=owner，另一端 role=member）。
#[derive(Debug, Clone)]
pub struct NewHumanDirectRoom {
    pub room_id: RoomId,
    pub creator_id: ParticipantId,
    pub other_id: ParticipantId,
    pub creator_membership_id: MembershipId,
    pub other_membership_id: MembershipId,
    pub created_at: UtcMillis,
}

/// 待持久化的新 group Room：Room、群名与 owner Membership 周期的数据来源。
#[derive(Debug, Clone)]
pub struct NewGroup {
    pub room_id: RoomId,
    pub owner_id: ParticipantId,
    pub owner_membership_id: MembershipId,
    pub name: String,
    pub created_at: UtcMillis,
}

/// 对端 Participant 的最小公共资料（id、kind、display name、status）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantProfile {
    pub id: ParticipantId,
    pub kind: ParticipantKind,
    pub display_name: DisplayName,
    pub status: ParticipantStatus,
}

/// 列表项：Room + 当前 Human 的 Membership 周期 + 对端 Participant 最小
/// 资料 + 当前 Human 的 read state（契约 CORE_V1_READ_STATE.md 4.1）。
/// `direct` 与 `human_direct` 的对端是配对中的另一位 Participant；group
/// 没有单一对端，恒为 None（群名在 Room 公共属性中）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomListItem {
    pub room: Room,
    pub membership: Membership,
    pub peer: Option<ParticipantProfile>,
    pub read_state: ReadState,
    /// 「对方读没读我最后那条」（契约 `CORE_V1_READ_RECEIPT.md` §2）：
    /// **只在问题成立时出现**——`None` ＝ 房不是 `direct`、对方当前不是这间房的
    /// active 成员、或我在这间房还没说过话。**投影，不是新事实**（零新写者）。
    pub peer_read: Option<crate::domain::room::PeerRead>,
}

/// 成员项：Membership 周期 + 对应 Participant 最小资料。
#[derive(Debug, Clone)]
pub struct MemberView {
    pub membership: Membership,
    pub participant: ParticipantProfile,
}

/// Agent 通讯录（切片 B1，CORE_V1_AGENT_DIRECTORY.md）单房目录投影：
/// 从既有 Room / Membership / Message / Participant 公共事实推导，不新增
/// 持久对象。`since` 增量计数口径与既有 unread_count 一致（消息落在该
/// agent 任一 Membership 周期窗口内、author ≠ 该 agent）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendDirectoryRoom {
    pub room_id: RoomId,
    pub kind: crate::domain::room::RoomKind,
    /// 机械显示名：`COALESCE(房间标题, 对端 display_name)`（group 恒为群名；
    /// direct 有可选标题时用标题；human_direct 用对端 Human 名）。
    pub display_name: String,
    /// 房间创建时间（机械排序回退基准；无消息房按此排序）。
    pub room_created_at: UtcMillis,
    /// 该房对该 agent 可见的最近一条公共 Message 的 created_at（无消息 =
    /// None；排序回退房间 created_at）。
    pub last_activity_at: Option<UtcMillis>,
    /// 该最近一条 Message 的正文公共投影（撤回 tombstone / 附件-only =
    /// None；存储层不截断）。
    pub preview: Option<String>,
    /// 自 `since` 以来该房可见且 author ≠ 该 agent 的 Message 增量计数。
    pub since_count: i64,
    /// 该 agent 在这个房间的 read state（契约
    /// CORE_V1_AGENT_READ_CURSOR.md §3）：与 Human 侧 `read_state` **同一
    /// 投影**——当前 active 周期的 cursor 位置与未读数；cursor 缺失时
    /// 防御性回退到周期 join boundary。
    pub read_state: crate::domain::room::ReadState,
    /// 「对方读没读我最后那条」（契约 `CORE_V1_READ_RECEIPT.md` §2）：
    /// **与 Human 侧同一格、同一判据**（人机同形）；`None` ＝ 问题不成立
    /// （房不是 `direct`、对端当前不是这间房的 active 成员、或该 agent 在这间房
    /// 还没说过话）。**投影，不是新事实**。
    pub peer_read: Option<crate::domain::room::PeerRead>,
}

/// 创建结果：首次创建与幂等重放返回相同的数据，以 `idempotency_replayed`
/// 区分（HTTP 201 / 200）。memberships 固定 owner 在前、member 在后。
#[derive(Debug, Clone)]
pub enum CreateDirectRoomOutcome {
    Created {
        room: Room,
        memberships: Vec<Membership>,
    },
    Replayed {
        room: Room,
        memberships: Vec<Membership>,
    },
}

/// human_direct / group 创建结果（复用同一响应形状）。
#[derive(Debug, Clone)]
pub enum CreateRoomOutcome {
    Created {
        room: Room,
        memberships: Vec<Membership>,
    },
    Replayed {
        room: Room,
        memberships: Vec<Membership>,
    },
}

/// 邀请/重入结果：
/// - `Created`：新 key 且目标没有 active 周期（首次邀请或历史周期关闭后
///   重入），打开新周期，HTTP 201；
/// - `Replayed`：同 Human + 同 key 重放，返回该 key 记录的 membership
///   周期（即使该周期后来关闭也只重放历史结果），HTTP 200；
/// - `AlreadyActive`：新 key 但目标已有 active 周期（如并发不同 key），
///   记录该 key 到既有 active 周期并返回 200（不重复打开周期）。
#[derive(Debug, Clone)]
pub enum InviteMemberOutcome {
    Created { membership: Membership },
    Replayed { membership: Membership },
    AlreadyActive { membership: Membership },
}

/// 群主转让结果：`Transferred` 为新 key 完成的原子转让（携带
/// transition_id：不可变 `group_owner_transitions` 事实 ID，post-commit
/// 唤醒目标查询的 source ID）；`Replayed` 为同 Human + 同 key 重放（重放
/// JOIN 不可变 transition 事实读取该 key 记录的新 owner 周期，即使该周期
/// 后来已被再次转让关闭）。
#[derive(Debug, Clone)]
pub enum TransferOwnerOutcome {
    Transferred {
        membership: Membership,
        transition_id: String,
    },
    Replayed {
        membership: Membership,
    },
}

/// read cursor 推进结果：`Advanced` 为真实推进（携带 advancement_id：
/// 不可变 `read_cursor_advancements` 事实 ID，post-commit 唤醒目标查询的
/// source ID）；`NoOp` 为目标不高于当前 cursor（成功幂等，不产生
/// advancement / Human Event / 唤醒）。
#[derive(Debug, Clone)]
pub enum AdvanceReadCursorOutcome {
    Advanced {
        read_state: ReadState,
        advancement_id: String,
    },
    NoOp {
        read_state: ReadState,
    },
}

/// 存储层错误。
#[derive(Debug)]
pub enum RoomStoreError {
    /// bad-id、unknown Room、非 group、调用者无 active owner 权限：对外
    /// 统一 404 room_not_found，不泄露 Room 是否存在。
    NotFound,
    /// 同 Human + 同 key 已被不同请求使用：409。
    IdempotencyConflict,
    /// 当前 Human 曾经拥有，但 Agent 或 Owner 关系已 revoked：409。
    AgentNotActive,
    /// human_direct 目标不存在、不是 active Human 或已 revoked：404。
    HumanNotFound,
    /// 目标 Human 存在但当前 Human 与它没有 accepted 联系人关系：409。
    ContactNotAccepted,
    /// 同一对 Human 已由不同 idempotency key 创建 human_direct Room：409。
    HumanDirectRoomExists,
    /// 邀请目标不是可邀请对象（未知、非 active、非 accepted 联系人、
    /// 非自己拥有的 active Agent）：对外统一 404。
    MemberNotFound,
    /// 尝试移除 group owner 自己：409。
    OwnerNotRemovable,
    /// group owner 尝试主动退群：必须先转让（409 owner_transfer_required）。
    OwnerTransferRequired,
    /// 转让目标不合法：就是转让者自己（400 invalid_transfer_target）。
    /// 「不是本房 active member」走上面那条 `MemberNotFound`（不分参与者 kind，
    /// 2026-09-27 批次三第一刀：owner 是房内治理角色，不是参与者种类）。
    InvalidTransferTarget,
    /// read cursor 推进目标不合法：Message 未知、属于其他 Room 或对当前
    /// Human 的历史 Membership 窗口不可见（对外统一 400
    /// invalid_read_cursor，不泄露具体原因）。
    InvalidReadCursor,
    /// 数据库或其他持久化失败（typed cause + 稳定分类）。
    Database(StorageError),
}

impl std::fmt::Display for RoomStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no such owned agent"),
            Self::IdempotencyConflict => write!(
                f,
                "idempotency key was already used with a different request"
            ),
            Self::AgentNotActive => write!(f, "agent or owner relationship is revoked"),
            Self::HumanNotFound => write!(f, "no such active human"),
            Self::ContactNotAccepted => write!(
                f,
                "the contact relationship with this human is not accepted"
            ),
            Self::HumanDirectRoomExists => {
                write!(f, "a human direct room already exists for this human pair")
            }
            Self::MemberNotFound => {
                write!(f, "no invitable member with this participant id")
            }
            Self::OwnerNotRemovable => write!(f, "the group owner cannot be removed"),
            Self::OwnerTransferRequired => {
                write!(f, "the group owner must transfer ownership before leaving")
            }
            Self::InvalidTransferTarget => write!(
                f,
                "the transfer target must be a different active human member"
            ),
            Self::InvalidReadCursor => write!(f, "the message cannot be the read cursor target"),
            Self::Database(e) => write!(f, "room store failure: {e}"),
        }
    }
}

impl std::error::Error for RoomStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(e) => Some(e),
            _ => None,
        }
    }
}

/// Room 存储端口（窄接口，不扩成万能 repository）。
#[async_trait::async_trait]
pub trait RoomStore: Send + Sync {
    /// 原子创建 Direct Room、Human–Agent 配对、两条奠基 Membership 周期与
    /// 幂等记录；同一 Human–Agent 对允许创建多个 Direct Room，因此不存在
    /// 「同对唯一」冲突。同 Human + key 已存在时：同 Agent 且同规范化标题
    /// （`name`，null 与非 null 是不同事实）返回 `Replayed`（不再创建任何
    /// 行），同 key 改 Agent 或标题返回 `IdempotencyConflict` 且不留下半
    /// 成品。不同 key 创建同 Human–Agent 对（标题相同或不同）都返回
    /// `Created`。
    ///
    /// 新 key 的裁决顺序：bad-id/unknown/not-owned 统一 `NotFound` →
    /// 历史 owned 但 Agent/Owner revoked 为 `AgentNotActive` → 201。
    async fn create_direct_room(
        &self,
        human_id: &ParticipantId,
        room: &NewDirectRoom,
        idempotency_key: &IdempotencyKey,
    ) -> Result<CreateDirectRoomOutcome, RoomStoreError>;

    /// 原子创建 human_direct Room、两端配对、两条奠基 Membership 周期与
    /// 幂等记录；目标必须是存在且 active 的 Human，且当前 Human 与它存在
    /// accepted 联系人关系（同一对 Human 最多一个 Room）。
    async fn create_human_direct_room(
        &self,
        human_id: &ParticipantId,
        room: &NewHumanDirectRoom,
        idempotency_key: &IdempotencyKey,
    ) -> Result<CreateRoomOutcome, RoomStoreError>;

    /// 原子创建 group Room、覆盖行、owner Membership 周期与幂等记录；
    /// 创建者自动成为 owner；同 Human + key 已存在时同群名返回
    /// `Replayed`，不同群名返回 `IdempotencyConflict`。
    async fn create_group(
        &self,
        human_id: &ParticipantId,
        room: &NewGroup,
        idempotency_key: &IdempotencyKey,
    ) -> Result<CreateRoomOutcome, RoomStoreError>;

    /// group owner 邀请成员（必须携带 Idempotency-Key，先查 key 再查
    /// 权限）：目标必须是 accepted Human 或自己拥有且 active 的 Agent；
    /// 邀请在同一事务内完成 owner 权限检查、目标资格检查和 Membership
    /// 周期创建，直接生成 active 周期，不造邀请状态机。owner 不能邀请
    /// 自己：在 active lookup 前显式识别并返回 `MemberNotFound`（不写幂等
    /// 记录，「目标已有 active 周期返回 200」不包括 owner 自己）。目标已
    /// 有 active 周期时返回 `AlreadyActive`（不重复打开周期）；目标有历史
    /// closed 周期且资格仍满足时用新 key 重入（`Created`）；同 key 重试
    /// 返回 `Replayed`（重放该 key 记录的周期，即使已关闭也不误开新周期）。
    async fn invite_group_member(
        &self,
        owner_id: &ParticipantId,
        room_id: &RoomId,
        participant_id: &ParticipantId,
        idempotency_key: &IdempotencyKey,
    ) -> Result<InviteMemberOutcome, RoomStoreError>;

    /// group owner 移除 active 成员：其 Membership 周期进入 closed 终态
    /// （closed_by=removed）；返回被关闭的 membership_id（post-commit
    /// 唤醒目标查询的 source ID）；owner 自身不可移除（`OwnerNotRemovable`）；
    /// unknown / bad-id / 非 owner / 非 group Room 统一 `NotFound`。
    async fn remove_group_member(
        &self,
        owner_id: &ParticipantId,
        room_id: &RoomId,
        participant_id: &ParticipantId,
    ) -> Result<MembershipId, RoomStoreError>;

    /// 成员主动退出一间房：自己的 Membership 周期进入 closed 终态
    /// （closed_by=leave），此后对 Room 的所有读写 404；返回被关闭的
    /// membership_id（post-commit 唤醒目标查询的 source ID）；owner
    /// 退出返回 `OwnerTransferRequired`；unknown / bad-id / `human_direct`
    /// 房 / 无 active 周期统一 `NotFound`。
    ///
    /// **一条规则**（2026-09-27 M2 切片）：非 owner 的成员周期可以由它自己
    /// 关闭，**不分房 kind、不分参与者 kind**；`human_direct` 的出口归关系
    /// 模型（`contact_ended` + 精确见证），因此那一档走 `NotFound`。
    async fn leave_membership(
        &self,
        participant_id: &ParticipantId,
        room_id: &RoomId,
    ) -> Result<MembershipId, RoomStoreError>;

    /// 群主原子转让（必须携带 Idempotency-Key，先查 key 再查权限）：
    /// 在 BEGIN IMMEDIATE 事务中读取一次 X=当前 Room 最大 message
    /// sequence 与时间 T；生成 transition_id 与两条新周期 ID；关闭旧
    /// owner 与目标 member 周期（closed_by=transfer，
    /// left_before_sequence=X）；插入不可变 `group_owner_transitions`
    /// 事实（其新周期 FK deferred，先于新周期插入）；打开目标 owner 与
    /// 旧 owner member 周期（join_after_sequence=X）；更新 coverage 见证
    /// 到新 owner 周期；写幂等记录 (human_id, key) → transition_id。
    /// 任何一步失败整体回滚。目标必须是当前 active 的 Human member 且
    /// 不能是自己；Agent、revoked、非成员返回稳定业务错误；同 key 重放
    /// JOIN 不可变 transition 读取该 key 记录的新 owner 周期。
    async fn transfer_group_owner(
        &self,
        owner_id: &ParticipantId,
        room_id: &RoomId,
        target_id: &ParticipantId,
        idempotency_key: &IdempotencyKey,
    ) -> Result<TransferOwnerOutcome, RoomStoreError>;

    /// 列出当前 Human 存在 active Membership 周期的 Room（离群后不再
    /// 可见），按 Room 创建时间升序、再按 Room ID 排序；Room 恒 active，
    /// 对端 Agent 可能已 revoked。
    async fn list_rooms(
        &self,
        human_id: &ParticipantId,
    ) -> Result<Vec<RoomListItem>, RoomStoreError>;

    /// 读取当前 Human 存在 active Membership 周期的单个 Room 管理视图
    /// （Room + 当前 Human Membership 周期 + 对端 Participant 最小资料）；
    /// unknown / bad-id / 非成员返回 None。
    async fn get_room(
        &self,
        human_id: &ParticipantId,
        room_id: &RoomId,
    ) -> Result<Option<RoomListItem>, RoomStoreError>;

    /// 读取当前 Human 存在 active Membership 周期的 Room 的全部公开
    /// Membership 周期（owner 在前，其余按 joined_at 升序、再按
    /// Participant ID 排序，含 Participant 最小资料）。group 只列 active
    /// 周期；direct / human_direct 保持配对成员展示（Agent revoke 后
    /// closed 的 Agent 周期仍可见）。unknown / bad-id / 非成员返回 None。
    async fn get_room_members(
        &self,
        human_id: &ParticipantId,
        room_id: &RoomId,
    ) -> Result<Option<Vec<MemberView>>, RoomStoreError>;

    /// 推进该主体在该 Room 的 read cursor（契约 `CORE_V1_IM_CONTRACT.md` §9；
    /// Agent 侧见 `CORE_V1_AGENT_READ_CURSOR.md`）：必须在
    /// `BEGIN IMMEDIATE` 或等价单写事务中完成——
    /// 1. 该主体必须存在该 Room 的 active Membership 周期（否则
    ///    `NotFound`，不泄露 Room 是否存在）；
    /// 2. 目标 Message 必须属于该 Room 且对该主体的历史 Membership
    ///    窗口可见（否则统一 `InvalidReadCursor`）；
    /// 3. 目标 sequence 不高于当前 cursor：成功 no-op（`NoOp`，不产生
    ///    advancement / Human Event）；
    /// 4. 目标更高：插入不可变 `read_cursor_advancements` 事实——其
    ///    AFTER INSERT 触发器链在同一语句内自动、精确地把 cursor 单调
    ///    推进到该 advancement（UPDATE 守卫要求与 advancement 全等的精确
    ///    见证，Rust 不存在独立 UPDATE 路径），并原子形成
    ///    read.cursor.advanced fact/delivery，commit 后返回 `Advanced`
    ///    （携带 advancement_id 供 post-commit 定向唤醒）。
    ///
    /// 并发由 SQLite 单写者与约束裁决，最终收敛到最高合法目标。
    ///
    /// Human 与 Agent 走**同一条**写路径、同一套可见性判据（迁移
    /// `202609270001` 把身份列 `human_id` 泛化为 `participant_id`，并放开
    /// 建行守卫的 kind 判据）；两者唯一的差别在事件面：Human 的推进生成
    /// `read.cursor.advanced` Human Event，**Agent 的推进不生成任何事件**
    /// （该 kind 按契约只投递推进者本人，而 Agent 的位置由读面直接读得到）。
    async fn advance_read_cursor(
        &self,
        participant_id: &ParticipantId,
        room_id: &RoomId,
        message_id: &MessageId,
    ) -> Result<AdvanceReadCursorOutcome, RoomStoreError>;

    /// Agent 通讯录纯读投影（切片 B1，CORE_V1_AGENT_DIRECTORY.md §4）：
    /// 该 agent 为 active member 的全部 Room 的机械目录行，按最近活动时间
    /// 倒序（无消息房回退房间创建时间）、再按房间 ID 升序。零写语义、零
    /// 持久对象新增；预览/计数从既有公共 Message 事实推导。`since` 为
    /// unix ms 增量基线（`None` = 无基线，`since_count` 恒为 0）。
    async fn list_backend_rooms(
        &self,
        agent_id: &ParticipantId,
        since: Option<i64>,
    ) -> Result<Vec<BackendDirectoryRoom>, RoomStoreError>;
}
