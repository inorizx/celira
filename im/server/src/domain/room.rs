//! Room、Membership 周期与房间内 Role：Core 公共事实的第三张领域切片。
//!
//! 契约（CORE_V1_DIRECT_ROOMS.md 5.x + CORE_V1_HUMAN_ROOMS_AND_MENTIONS.md
//! 4.3）：Room 是统一公共容器，最小 kind 为 direct（Human–Agent Direct
//! Room）、human_direct（两个 accepted Human 的私聊）与 group（多
//! Human/Agent 的群聊容器）；Membership 是显式、可撤销、可重入、可保留
//! 历次周期的公共关系，每行是一条不可变周期（join_after_sequence /
//! left_before_sequence 决定该周期内可见的 Room sequence 窗口）；
//! direct 中 Human 固定 role=owner、Agent 固定 role=member；human_direct
//! 中创建者固定 role=owner、另一端固定 role=member；group 创建者固定
//! role=owner、其他加入者为 member。Role 与状态作为纯规则在此强制。

use crate::domain::message::MessageId;
use crate::domain::participant::ParticipantId;
use crate::domain::time::UtcMillis;

/// 稳定、不可复用的 Room ID（UUID v4 字符串，统一小写连字符）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RoomId(pub String);

impl RoomId {
    /// 新 Room ID：UUID v4 的标准小写连字符格式。
    pub fn new_v4() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// 解析并规范化为小写连字符的 UUID v4；其他格式返回 None（用于对外
    /// 统一「bad-id」与「不存在」）。
    pub fn parse_canonical(raw: &str) -> Option<Self> {
        let uuid = uuid::Uuid::parse_str(raw).ok()?;
        (uuid.get_version() == Some(uuid::Version::Random))
            .then(|| Self(uuid.hyphenated().to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RoomId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 稳定、不可复用的 Membership 周期 ID（UUID v4 字符串，统一小写连字符）。
/// 每个周期（active 或 closed）都有独立 ID，是公开 Membership 视图的一部分。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MembershipId(pub String);

impl MembershipId {
    /// 新周期 ID：UUID v4 的标准小写连字符格式。
    pub fn new_v4() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MembershipId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Room 种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomKind {
    /// 既有 Human–Agent Direct Room，语义不变。
    Direct,
    /// 两个 accepted Human 的私聊。
    HumanDirect,
    /// 多 Human/Agent Participant 的群聊容器。
    Group,
}

impl RoomKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::HumanDirect => "human_direct",
            Self::Group => "group",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "direct" => Some(Self::Direct),
            "human_direct" => Some(Self::HumanDirect),
            "group" => Some(Self::Group),
            _ => None,
        }
    }
}

// **一间房见证的那段关系**（批次二，2026-09-27 主人裁决）。
//
// 房的身份是它见证的关系，不是它叫什么。`direct` 房见证一段 **Agent Owner
// 关系**（`agent_owners` 的那一行）——每对 Human–Agent **至多一间**。
// `human_direct` 见证 contact 周期、`group` 房本身就是关系（多方），都不在
// 这个词表里。
//
// ⚠ **2026-09-28（服务层退役）**：这里原有一个 `RoomRelation` 词表，取值
// `private` / `service`，对应持久列 `direct_room_pairs.relation`。服务层
// （Publication / Access / 服务房）整体退役后，`direct` 房**只剩一种来路**，
// 于是那个词表、那一列与那条分支一并退场：判据落成 `direct_room_pairs` 上的
// **`UNIQUE(human_id, agent_id)`**（每段关系至多一间，一字不改），不再需要
// 一个"是哪段关系"的判别列。**"一间房 = 一段关系"这条裁决本身没有被收回。**

/// Room 容器状态。当前只有 active，不随任一 Participant 撤销改变。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomStatus {
    Active,
}

impl RoomStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            _ => None,
        }
    }
}

/// Room 公共视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room {
    pub id: RoomId,
    pub kind: RoomKind,
    pub status: RoomStatus,
    /// 可选非空标题：只有 `direct` 与 `group` 允许（均非 NULL 时按显示名
    /// 规则校验）；`human_direct` 恒为 None。
    pub name: Option<String>,
    pub created_at: UtcMillis,
}

/// 对外 read state（契约 CORE_V1_READ_STATE.md 4.1）：仅
/// `last_read_message_id`（可 null）与 `unread_count`；内部初始
/// boundary / cursor sequence（join_after_sequence / read_sequence）不暴露。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadState {
    /// 最近一次客户端成功推进到的可见 Message ID；尚未推进时为 None。
    pub last_read_message_id: Option<MessageId>,
    /// 非负未读数：当前 cursor 之后、落在当前 Human 任一真实 Membership
    /// 周期窗口内、且 actor 不是当前 Human 的 Message 数量（读取时从唯一
    /// Message + Membership + cursor 事实投影，不建立第二张计数器表）。
    pub unread_count: i64,
}

/// **提醒档**（契约 `CORE_V1_IM_CONTRACT.md` §9.1）：这间房的动静要不要提醒我。
///
/// 它是**个人事实**（每 Room × Participant 至多一行，只有本人能写、只有本人
/// 看得见），与 §9 的读位置同一档。⚠ 它**不改消息主链**——消息照常提交、未读
/// 照常计算、历史照常可读；它只决定那条**易失**通知发不发。⇒ **任何一档都不
/// 丢消息**。
///
/// **两态**，而且**缺行就是 `all`**（默认档，不预建行）：两态里没有第三格
/// "没设过"——那是 `Default`，不是一个可以出现在公开取值里的第三种词。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AttentionLevel {
    /// 这间房的动静都提醒我（**默认**，也是缺行时投影出来的取值）。
    #[default]
    All,
    /// 只有**结构化 mention 到我**才提醒。
    Mentions,
}

impl AttentionLevel {
    /// 对外稳定字符串（契约里的取值就是这两个词）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Mentions => "mentions",
        }
    }

    /// 从公开取值还原；认不出的取值**不猜**（调用方按请求非法或内部不变量
    /// 破坏处理——写面是 400，读面是 corrupt）。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "all" => Some(Self::All),
            "mentions" => Some(Self::Mentions),
            _ => None,
        }
    }
}

/// 「**对方读没读我最后那条**」（契约 `CORE_V1_READ_RECEIPT.md` §2）。
///
/// **两态**，而且这一格**只在问题成立时出现**——`Option::None` 表示问题不成立，
/// 三种情形一视同仁：① 房不是 `direct`；② 对方当前不是这间房的 active 成员；
/// ③ 我在这间房里还没说过话。判据一句话：**一格只在它能被回答的时候出现**
/// （"没有答案"与"答案是 unread"必须分得开——缺省不当零）。
///
/// ⚠ 它说的是**读位置**，不是"对方看到了"：读到是位置，看到是判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerRead {
    /// 对方的读位置 ≥ 我在这间房发出的最后一条 Message。
    Read,
    /// 对方还没读到那儿。
    Unread,
}

impl PeerRead {
    /// 对外稳定字符串（契约里的取值就是这两个词）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Unread => "unread",
        }
    }

    /// 从存储投影还原；认不出的取值**不猜**（调用方按内部不变量破坏处理）。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "read" => Some(Self::Read),
            "unread" => Some(Self::Unread),
            _ => None,
        }
    }
}

impl Room {
    /// 新 Direct Room：kind=direct、status=active、可选非空标题
    /// （`None` 表示无标题，合法）。
    pub fn new_direct(id: RoomId, name: Option<String>, created_at: UtcMillis) -> Self {
        Self {
            id,
            kind: RoomKind::Direct,
            status: RoomStatus::Active,
            name,
            created_at,
        }
    }

    /// 新 Human direct Room：kind=human_direct、status=active、恒无标题。
    pub fn new_human_direct(id: RoomId, created_at: UtcMillis) -> Self {
        Self {
            id,
            kind: RoomKind::HumanDirect,
            status: RoomStatus::Active,
            name: None,
            created_at,
        }
    }

    /// 新 group Room：kind=group、status=active、携带创建时校验后的群名。
    pub fn new_group(id: RoomId, name: String, created_at: UtcMillis) -> Self {
        Self {
            id,
            kind: RoomKind::Group,
            status: RoomStatus::Active,
            name: Some(name),
            created_at,
        }
    }
}

/// 房间内 Role。创建 Direct Room 时：Human 固定 `owner`，Agent 固定
/// `member`；owner 只在该 Room 内有意义，不等于 Agent Owner。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipRole {
    Owner,
    Member,
}

impl MembershipRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Member => "member",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "owner" => Some(Self::Owner),
            "member" => Some(Self::Member),
            _ => None,
        }
    }
}

/// Membership 周期生命周期状态：active 表示当前在群/房期间，closed 表示
/// 周期已关闭（left 三元组与关闭原因随之固化）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipStatus {
    Active,
    Closed,
}

/// Membership 周期关闭原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipClosedBy {
    /// 成员主动退群。
    Leave,
    /// group owner 移除成员。
    Removed,
    /// 群主原子转让（关闭旧 owner 与目标 member 周期）。
    Transfer,
    /// Agent 撤销传播。
    AgentRevoked,
    /// 关系段结束传播（human_direct 房间双方成员周期随 contact_periods
    /// 段以 ended 关闭而在同一事务内关闭；只属于 human_direct）。
    ContactEnded,
}

impl MembershipClosedBy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Leave => "leave",
            Self::Removed => "removed",
            Self::Transfer => "transfer",
            Self::AgentRevoked => "agent_revoked",
            Self::ContactEnded => "contact_ended",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "leave" => Some(Self::Leave),
            "removed" => Some(Self::Removed),
            "transfer" => Some(Self::Transfer),
            "agent_revoked" => Some(Self::AgentRevoked),
            "contact_ended" => Some(Self::ContactEnded),
            _ => None,
        }
    }
}

/// 显式 Membership 周期：某 Participant 在某段时间内属于某 Room 的独立
/// 公共关系。
///
/// 每行是一条不可变周期：`membership_id` 是公开周期身份；active 周期
/// `left_at`/`left_before_sequence`/`closed_by` 恒为 None；关闭后三者固化、
/// 整行不可变，重入只能打开新周期。周期窗口由 Room 内不可变 sequence
/// 裁决：`join_after_sequence < message.sequence` 且（`left_before_sequence`
/// 为空或 `message.sequence <= left_before_sequence`）；时间只作展示与
/// 审计，不参与授权。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Membership {
    pub membership_id: MembershipId,
    pub room_id: RoomId,
    pub participant_id: ParticipantId,
    pub role: MembershipRole,
    pub status: MembershipStatus,
    pub joined_at: UtcMillis,
    /// 周期内可见的 Room sequence 下限：只包含 `sequence > join_after_sequence`
    /// 的消息。内部 sequence 边界不对外暴露。
    pub join_after_sequence: i64,
    pub left_at: Option<UtcMillis>,
    /// 周期内可见的 Room sequence 上限（含）：只包含
    /// `sequence <= left_before_sequence` 的消息。内部 sequence 边界不对
    /// 外暴露。
    pub left_before_sequence: Option<i64>,
    pub closed_by: Option<MembershipClosedBy>,
}

impl Membership {
    /// 新 active 周期：无关闭字段。`join_after_sequence` 是加入时刻该 Room
    /// 当前最大 message sequence（新周期只对之后的消息可见）。
    pub fn new(
        membership_id: MembershipId,
        room_id: RoomId,
        participant_id: ParticipantId,
        role: MembershipRole,
        joined_at: UtcMillis,
        join_after_sequence: i64,
    ) -> Self {
        Self {
            membership_id,
            room_id,
            participant_id,
            role,
            status: MembershipStatus::Active,
            joined_at,
            join_after_sequence,
            left_at: None,
            left_before_sequence: None,
            closed_by: None,
        }
    }
}

/// 创建 Direct Room 请求的语义由 agent_id 完整表达：同一 Human + key
/// 只能对应一个 Agent（重放裁决只使用 agent_id），本切片不保存请求摘要。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_id_parse_canonical_normalizes_case_and_format() {
        let raw = "01234567-89AB-4CDE-0123-456789ABCDEF";
        let id = RoomId::parse_canonical(raw).expect("valid v4");
        assert_eq!(id.as_str(), "01234567-89ab-4cde-0123-456789abcdef");
        assert_eq!(
            RoomId::parse_canonical(&id.as_str().to_uppercase()).unwrap(),
            id
        );
    }

    #[test]
    fn room_id_rejects_bad_ids() {
        for bad in [
            "",
            "not-a-uuid",
            "00000000-0000-0000-0000-000000000000", // v0，非 v4
            "01234567-89ab-cdef-0123-456789abcdef"
                .replace('e', "zz")
                .as_str(),
        ] {
            assert!(
                RoomId::parse_canonical(bad).is_none(),
                "expected rejection for {bad:?}"
            );
        }
    }

    #[test]
    fn room_kinds_have_fixed_kind_and_status() {
        let direct = Room::new_direct(RoomId::new_v4(), None, UtcMillis::from_millis(1));
        assert_eq!(direct.kind, RoomKind::Direct);
        assert_eq!(direct.status, RoomStatus::Active);
        assert_eq!(direct.name, None);
        let direct_named = Room::new_direct(
            RoomId::new_v4(),
            Some("话题".into()),
            UtcMillis::from_millis(1),
        );
        assert_eq!(direct_named.name.as_deref(), Some("话题"));
        let human_direct = Room::new_human_direct(RoomId::new_v4(), UtcMillis::from_millis(1));
        assert_eq!(human_direct.kind, RoomKind::HumanDirect);
        assert_eq!(human_direct.status, RoomStatus::Active);
        assert_eq!(human_direct.name, None);
        let group = Room::new_group(RoomId::new_v4(), "项目群".into(), UtcMillis::from_millis(1));
        assert_eq!(group.kind, RoomKind::Group);
        assert_eq!(group.status, RoomStatus::Active);
        assert_eq!(group.name.as_deref(), Some("项目群"));
        assert_eq!(RoomKind::HumanDirect.as_str(), "human_direct");
        assert_eq!(RoomKind::Group.as_str(), "group");
        assert_eq!(RoomKind::parse("human_direct"), Some(RoomKind::HumanDirect));
        assert_eq!(RoomKind::parse("group"), Some(RoomKind::Group));
        assert_eq!(RoomKind::parse("chat"), None);
    }

    #[test]
    fn membership_id_generates_v4_uuid() {
        let id = MembershipId::new_v4();
        let parsed = uuid::Uuid::parse_str(id.as_str()).expect("valid v4");
        assert_eq!(parsed.get_version(), Some(uuid::Version::Random));
        assert_eq!(id.as_str(), id.as_str().to_ascii_lowercase());
    }

    #[test]
    fn new_membership_is_active_period_without_closure() {
        let membership = Membership::new(
            MembershipId::new_v4(),
            RoomId::new_v4(),
            ParticipantId("p".into()),
            MembershipRole::Owner,
            UtcMillis::from_millis(1),
            7,
        );
        assert_eq!(membership.status, MembershipStatus::Active);
        assert_eq!(membership.join_after_sequence, 7);
        assert_eq!(membership.left_at, None);
        assert_eq!(membership.left_before_sequence, None);
        assert_eq!(membership.closed_by, None);
        assert_eq!(membership.role.as_str(), "owner");
        assert_eq!(MembershipRole::Member.as_str(), "member");
        assert_eq!(MembershipClosedBy::Leave.as_str(), "leave");
        assert_eq!(MembershipClosedBy::Removed.as_str(), "removed");
        assert_eq!(MembershipClosedBy::Transfer.as_str(), "transfer");
        assert_eq!(MembershipClosedBy::AgentRevoked.as_str(), "agent_revoked");
        assert_eq!(MembershipClosedBy::ContactEnded.as_str(), "contact_ended");
        assert_eq!(
            MembershipClosedBy::parse("removed"),
            Some(MembershipClosedBy::Removed)
        );
        assert_eq!(
            MembershipClosedBy::parse("contact_ended"),
            Some(MembershipClosedBy::ContactEnded)
        );
        assert_eq!(MembershipClosedBy::parse("revoked"), None);
    }
}
