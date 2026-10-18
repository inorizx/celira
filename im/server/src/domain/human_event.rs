//! Durable Human Event：账号级活动事实的领域切片。
//!
//! 契约（CORE_V1_HUMAN_EVENTS.md 4.x）：Human Event 是 Core 基于一条已
//! 提交公共事实（普通 Message / Membership 周期变化 / 群主转让 / read
//! cursor 推进）为账号级 Human 建立的可恢复活动事实。它不是第二套 History、
//! 不是 ACK、不是 WebSocket 连接状态；事件恢复正确性由耐久 REST 拉取承担，
//! WebSocket 只负责及时唤醒。
//!
//! 事实是两层规范化结构：shared fact（每 source occurrence 一份）与
//! per-Human delivery（human_id + 自己范围内单调 sequence + fact 引用），
//! 不允许按 recipient 复制正文/快照。领域实体只描述已持久化事实的
//! 读取形状；快照是发生时已授权的公开 JSON，由 SQLite 触发器在同一事务
//! 内固化，HTTP adapter 原样投影。

use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;

/// 稳定、不可复用的 Human Event fact ID（UUID v4 字符串，统一小写连字符）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HumanEventFactId(pub String);

impl HumanEventFactId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for HumanEventFactId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 当前 Human Event 的判别值（协议与持久层共用同一组字符串）。
pub mod kind {
    pub const MESSAGE_CREATED: &str = "message.created";
    pub const MESSAGE_RECALLED: &str = "message.recalled";
    pub const MEMBER_JOINED: &str = "member.joined";
    pub const MEMBER_LEFT: &str = "member.left";
    pub const OWNER_TRANSFERRED: &str = "owner.transferred";
    pub const READ_CURSOR_ADVANCED: &str = "read.cursor.advanced";
}

/// 已持久化的 Human Event kind（数据库 CHECK 限定六值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HumanEventKind {
    MessageCreated,
    MessageRecalled,
    MemberJoined,
    MemberLeft,
    OwnerTransferred,
    ReadCursorAdvanced,
}

impl HumanEventKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            kind::MESSAGE_CREATED => Some(Self::MessageCreated),
            kind::MESSAGE_RECALLED => Some(Self::MessageRecalled),
            kind::MEMBER_JOINED => Some(Self::MemberJoined),
            kind::MEMBER_LEFT => Some(Self::MemberLeft),
            kind::OWNER_TRANSFERRED => Some(Self::OwnerTransferred),
            kind::READ_CURSOR_ADVANCED => Some(Self::ReadCursorAdvanced),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MessageCreated => kind::MESSAGE_CREATED,
            Self::MessageRecalled => kind::MESSAGE_RECALLED,
            Self::MemberJoined => kind::MEMBER_JOINED,
            Self::MemberLeft => kind::MEMBER_LEFT,
            Self::OwnerTransferred => kind::OWNER_TRANSFERRED,
            Self::ReadCursorAdvanced => kind::READ_CURSOR_ADVANCED,
        }
    }
}

/// 一条已持久化的共享 Human Event fact（每 source occurrence 一份）。
///
/// fact 列只持有 kind/source/room/created_at 与发生时公开快照，不复制
/// Message 正文之外的任何私有事实；snapshot 是发生时已授权的公开 JSON
/// （不含 token/hash/幂等键/Owner 私有授权/Binding/Credential/内部边界
/// 字段），由 SQLite 触发器固化，读取时原样返回。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanEventFact {
    pub id: HumanEventFactId,
    pub kind: HumanEventKind,
    /// 稳定 source ID：messages.id / room_memberships.membership_id /
    /// group_owner_transitions.transition_id /
    /// read_cursor_advancements.advancement_id。
    pub source_id: String,
    pub room_id: RoomId,
    /// 必须等于 source occurrence 的时点（message.created_at /
    /// joined_at / left_at / occurred_at）。
    pub created_at: UtcMillis,
    /// 发生时已授权的公开快照（JSON 文本，触发器生成；解析属于 adapter
    /// 投影职责）。
    pub snapshot: String,
}

/// 最小窄 source 引用：post-commit 唤醒目标反查的完整 source 身份。
///
/// migration 009 中 fact 唯一身份是 `(kind, source_id)`——同一 source_id
/// 字符串在不同 kind 下是不同 fact（不同 source 表各自生成 ID，可能
/// 碰撞）；任何按 source 反查（delivery_targets / wake_humans）都必须
/// 同时携带 kind 与 source_id，不能只按 source_id。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HumanEventSource {
    kind: HumanEventKind,
    source_id: String,
}

impl HumanEventSource {
    /// 构造完整 source 引用。
    pub fn new(kind: HumanEventKind, source_id: impl Into<String>) -> Self {
        Self {
            kind,
            source_id: source_id.into(),
        }
    }

    pub fn kind(&self) -> HumanEventKind {
        self.kind
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }
}

impl std::fmt::Display for HumanEventSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.kind.as_str(), self.source_id)
    }
}
