//! Agent Participant 与 Agent Owner：Core 公共事实的第二张领域切片。
//!
//! 契约（CORE_V1_AGENT_OWNERSHIP.md 4.x）：Agent 是与 Human 不同的
//! Participant 变体；Owner 是 Human 与 Agent 之间的独立责任关系实体，
//! 不是 Agent 字段别名。幂等键格式与请求摘要在此作为纯规则强制。

use sha2::{Digest, Sha256};

use crate::domain::participant::{DisplayName, ParticipantId, ParticipantKind, ParticipantStatus};
use crate::domain::time::UtcMillis;

/// Agent Participant 公共视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub id: ParticipantId,
    pub kind: ParticipantKind,
    pub display_name: DisplayName,
    pub status: ParticipantStatus,
    pub created_at: UtcMillis,
    pub revoked_at: Option<UtcMillis>,
}

impl Agent {
    /// 新 Agent：kind=agent、status=active、无撤销时间。
    pub fn new(id: ParticipantId, display_name: DisplayName, created_at: UtcMillis) -> Self {
        Self {
            id,
            kind: ParticipantKind::Agent,
            display_name,
            status: ParticipantStatus::Active,
            created_at,
            revoked_at: None,
        }
    }
}

/// Agent Owner：Human 对 Agent 承担责任的独立关系实体。
///
/// 不产生 Room Membership、Role、Permission、Backend Binding 或 Credential；
/// 本切片只建立「创建者 Human → 新 Agent」的初始 active 关系。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOwner {
    pub agent_id: ParticipantId,
    pub human_id: ParticipantId,
    pub status: ParticipantStatus,
    pub created_at: UtcMillis,
    pub revoked_at: Option<UtcMillis>,
}

impl AgentOwner {
    /// 新 Owner 关系：status=active、无撤销时间。
    pub fn new(agent_id: ParticipantId, human_id: ParticipantId, created_at: UtcMillis) -> Self {
        Self {
            agent_id,
            human_id,
            status: ParticipantStatus::Active,
            created_at,
            revoked_at: None,
        }
    }
}

/// 管理视图最小单元：已拥有（或曾拥有）的 Agent 及其 Owner 关系。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedAgent {
    pub agent: Agent,
    pub ownership: AgentOwner,
}

/// 幂等键校验失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdempotencyKeyError {
    /// 空值。HTTP 层把「header 完全缺失」单独映射为
    /// `idempotency_key_required`；空值视为格式非法。
    Empty,
    /// 超过 128 个字符（可见 ASCII 下字节数即字符数）。
    TooLong(usize),
    /// 首尾空白。
    LeadingOrTrailingWhitespace,
    /// 包含逗号、控制字符或任何非可见 ASCII 字符。
    InvalidCharacter,
}

impl std::fmt::Display for IdempotencyKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "idempotency key must not be empty"),
            Self::TooLong(len) => {
                write!(
                    f,
                    "idempotency key exceeds 128 visible ASCII characters (got {len})"
                )
            }
            Self::LeadingOrTrailingWhitespace => {
                write!(
                    f,
                    "idempotency key must not have leading or trailing whitespace"
                )
            }
            Self::InvalidCharacter => write!(
                f,
                "idempotency key may only contain visible ASCII characters and no comma"
            ),
        }
    }
}

impl std::error::Error for IdempotencyKeyError {}

/// 幂等键：1–128 个可见 ASCII 字符（0x21–0x7E），禁止首尾空白、
/// 控制字符和逗号；不做任何归一化，按提交原文比较。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey(pub String);

impl IdempotencyKey {
    pub fn parse(raw: &str) -> Result<Self, IdempotencyKeyError> {
        if raw.is_empty() {
            return Err(IdempotencyKeyError::Empty);
        }
        let len = raw.len();
        if len > 128 {
            return Err(IdempotencyKeyError::TooLong(len));
        }
        let first = raw.as_bytes()[0];
        let last = raw.as_bytes()[len - 1];
        if first.is_ascii_whitespace() || last.is_ascii_whitespace() {
            return Err(IdempotencyKeyError::LeadingOrTrailingWhitespace);
        }
        for byte in raw.bytes() {
            if byte == b',' || !(0x21..=0x7E).contains(&byte) {
                return Err(IdempotencyKeyError::InvalidCharacter);
            }
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 创建请求的语义摘要：规范化字段名 + 未经 Unicode 归一化的 display name。
///
/// 用于判定「同一 Human + 同 key + 同请求」：同摘要视为重放，不同摘要
/// 视为冲突。字段名固定，值按调用者提交的正文参与计算。
pub fn create_request_digest(display_name: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"display_name=");
    hasher.update(display_name.as_bytes());
    hex::encode(hasher.finalize())
}
