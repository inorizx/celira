//! Human Participant：长期公共身份。
//!
//! 契约（CORE_V1_IDENTITY_AUTH.md 4.1）：Human 是 Participant 的一种，
//! 不是 Session、设备或 Credential；显示名规则在本模块作为纯规则强制。

use crate::domain::time::UtcMillis;

/// 稳定、不可复用的 Participant ID（UUID v4 字符串）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ParticipantId(pub String);

impl ParticipantId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 解析 Core 可识别的稳定 ID：必须是 UUID v4 字符串。
    /// 格式不合法返回 None（用于对外统一「bad-id」与「不存在」）。
    pub fn parse_stable(raw: &str) -> Option<Self> {
        let uuid = uuid::Uuid::parse_str(raw).ok()?;
        (uuid.get_version() == Some(uuid::Version::Random)).then(|| Self(raw.to_owned()))
    }

    /// 解析并规范化为小写连字符的 UUID v4；格式不合法返回 None。
    /// 契约（CORE_V1_DIRECT_ROOMS.md 7.1）：合法 agent_id 在查库与幂等
    /// 比较前统一为标准小写连字符格式。
    pub fn parse_canonical(raw: &str) -> Option<Self> {
        let uuid = uuid::Uuid::parse_str(raw).ok()?;
        (uuid.get_version() == Some(uuid::Version::Random))
            .then(|| Self(uuid.hyphenated().to_string()))
    }
}

impl std::fmt::Display for ParticipantId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Participant 种类。当前切片包含 human 与 agent。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantKind {
    Human,
    Agent,
}

impl ParticipantKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
        }
    }

    /// 数据库枚举值 → 领域类型；未知值返回 None。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "human" => Some(Self::Human),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }
}

/// Participant 生命周期状态。Human 只存在 active；Agent 支持
/// active/revoked 两个终态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParticipantStatus {
    Active,
    Revoked,
}

impl ParticipantStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Revoked => "revoked",
        }
    }

    /// 数据库枚举值 → 领域类型；未知值返回 None。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "revoked" => Some(Self::Revoked),
            _ => None,
        }
    }
}

/// 显示名校验失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayNameError {
    /// 没有非空白字符。
    NoNonWhitespace,
    /// 包含 U+0000。
    ContainsNul,
    /// 超过 80 个 Unicode scalar values。
    TooLong(usize),
}

impl std::fmt::Display for DisplayNameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoNonWhitespace => {
                write!(
                    f,
                    "display name must contain at least one non-whitespace character"
                )
            }
            Self::ContainsNul => write!(f, "display name must not contain U+0000"),
            Self::TooLong(len) => write!(
                f,
                "display name exceeds 80 Unicode scalar values (got {len})"
            ),
        }
    }
}

impl std::error::Error for DisplayNameError {}

/// 已通过校验的显示名：按调用者提交的正文保存，不做 Unicode 归一化。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayName(String);

impl DisplayName {
    /// 规则（契约 4.1）：
    /// - 至少一个非空白字符；
    /// - 最多 80 个 Unicode scalar values（`chars().count()` 与 scalar value
    ///   计数一致：Rust `&str` 恒为合法 UTF-8，迭代器逐个产出 scalar value）；
    /// - 禁止 U+0000；
    /// - 不做兼容归一化，不静默删除或替换字符。
    pub fn parse(raw: &str) -> Result<Self, DisplayNameError> {
        if raw.chars().any(|c| c == '\u{0}') {
            return Err(DisplayNameError::ContainsNul);
        }
        let scalar_count = raw.chars().count();
        if scalar_count > 80 {
            return Err(DisplayNameError::TooLong(scalar_count));
        }
        if !raw.chars().any(|c| !c.is_whitespace()) {
            return Err(DisplayNameError::NoNonWhitespace);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Human Participant 公共视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Participant {
    pub id: ParticipantId,
    pub kind: ParticipantKind,
    pub display_name: DisplayName,
    pub status: ParticipantStatus,
    pub created_at: UtcMillis,
}

impl Participant {
    pub fn new_human(id: ParticipantId, display_name: DisplayName, created_at: UtcMillis) -> Self {
        Self {
            id,
            kind: ParticipantKind::Human,
            display_name,
            status: ParticipantStatus::Active,
            created_at,
        }
    }
}
