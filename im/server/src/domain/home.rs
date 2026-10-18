//! Home Identity、Pairing Grant 与 Session Metadata 领域事实。
//!
//! A 包窄解冻切片（CORE_V1_IM_CONTRACT.md §3.1）：只新增本模块事实与
//! 端点，不改变任何既有冻结语义。
//!
//! - Home Identity：每 Core 恰一条、非秘密、不可变；不等于任何
//!   Human/Device/Session，不能作匿名发现（读取恒需认证）。
//! - Pairing Grant：短期、一次性、可撤销配对凭据；原始 token 只输出
//!   一次，数据库只存 SHA-256 摘要；兑换只产生与 `issued_by` 相同的
//!   Human 的独立 Session。
//! - Session Metadata：1:1 引用既有 Session 的新事实（非秘密设备标签 +
//!   来源 grant），绝不包含 token/hash，不改写 human_sessions 语义。

use crate::domain::human_session::TokenHash;
use crate::domain::participant::{DisplayName, ParticipantId};
use crate::domain::time::UtcMillis;

/// 稳定 Home ID（UUID v4 字符串）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HomeId(pub String);

impl HomeId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 生成新的 UUID v4 Home ID。
    pub fn new_v4() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl std::fmt::Display for HomeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Home Identity：初始化一次、不可变的非秘密标识。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    pub home_id: HomeId,
    pub display_name: Option<DisplayName>,
    pub created_at: UtcMillis,
}

/// 稳定 Grant ID（UUID v4 字符串）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GrantId(pub String);

impl GrantId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 生成新的 UUID v4 Grant ID。
    pub fn new_v4() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// 解析 Core 可识别的稳定 ID：必须是 UUID v4 字符串；格式不合法
    /// 返回 None（用于对外统一「bad-id」与「不存在」）。
    pub fn parse_stable(raw: &str) -> Option<Self> {
        let uuid = uuid::Uuid::parse_str(raw).ok()?;
        (uuid.get_version() == Some(uuid::Version::Random)).then(|| Self(raw.to_owned()))
    }
}

impl std::fmt::Display for GrantId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Grant 生命周期状态。`expired` 是派生状态：存储状态为 active 且
/// `expires_at` 已过时对外呈现 expired（TTL 内未用自动失效，不落库改写）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantStatus {
    Active,
    Consumed,
    Revoked,
    Expired,
}

impl GrantStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Consumed => "consumed",
            Self::Revoked => "revoked",
            Self::Expired => "expired",
        }
    }

    /// 数据库枚举值 → 领域类型；未知值返回 None。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "consumed" => Some(Self::Consumed),
            "revoked" => Some(Self::Revoked),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }
}

/// Grant 的用途（**封闭词表**）：决定兑换产出**谁的**会话。
///
/// - `SelfDevice`：新设备兑换为**签发者本人**的会话（既有语义，一字不动）；
/// - `NewHuman`：兑换产出**一个新 Human** + 该 Human 的独立会话——「邀请一个
///   人进这个世界」。display_name 由**兑换者自己**给（被邀请者自己命名自己，
///   不由签发者代填）。
///
/// ⚠ **不做匿名自注册**：一个世界的入口只有两条——建一个世界，或被请进一个
/// 世界（`PRODUCT_VISION.md` 当前阶段；邀请码是"家钥匙"在人身上的对称物）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantKind {
    SelfDevice,
    NewHuman,
}

impl GrantKind {
    /// 持久列 `grants_human` 的编码（1 = 邀请一个人）。
    pub fn grants_human(self) -> bool {
        match self {
            Self::SelfDevice => false,
            Self::NewHuman => true,
        }
    }

    /// 持久列 → 领域值。列是 0/1（由 CHECK 保证），未知形态视为损坏。
    pub fn from_grants_human(grants_human: bool) -> Self {
        if grants_human {
            Self::NewHuman
        } else {
            Self::SelfDevice
        }
    }

    /// 对外 wire 值（`POST /v1/home/pairing-grants` 的 `grants` 字段）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SelfDevice => "self_device",
            Self::NewHuman => "new_human",
        }
    }

    /// wire 值 → 领域值；**未知取值返回 None**（封闭表，调用方必须拒绝）。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "self_device" => Some(Self::SelfDevice),
            "new_human" => Some(Self::NewHuman),
            _ => None,
        }
    }
}

/// Pairing Grant 记录（不含原始 token）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingGrant {
    pub grant_id: GrantId,
    pub issued_by: ParticipantId,
    pub device_label: Option<DisplayName>,
    /// 用途（`grants_human` 列的领域读法）。
    pub kind: GrantKind,
    pub status: GrantStatus,
    pub issued_at: UtcMillis,
    pub expires_at: UtcMillis,
    pub consumed_at: Option<UtcMillis>,
    pub revoked_at: Option<UtcMillis>,
}

impl PairingGrant {
    /// 以「存储状态 + 当前时间」呈现对外状态：active 且已过 expires_at
    /// 视为 expired（派生，不落库）；其余状态原样。
    pub fn status_at(&self, now: UtcMillis) -> GrantStatus {
        match self.status {
            GrantStatus::Active if self.expires_at <= now => GrantStatus::Expired,
            other => other,
        }
    }

    /// 是否可兑换：存储状态 active 且当前时间未过期。
    pub fn is_exchangeable_at(&self, now: UtcMillis) -> bool {
        self.status == GrantStatus::Active && now < self.expires_at
    }
}

/// 待持久化的新 Grant（签发侧领域事实：摘要 + 签发者 + 时间窗）。
#[derive(Debug, Clone)]
pub struct NewGrant {
    pub grant_id: GrantId,
    pub issued_by: ParticipantId,
    pub device_label: Option<DisplayName>,
    /// 用途：`SelfDevice`（既有）或 `NewHuman`（邀请一个人进来）。
    pub kind: GrantKind,
    pub token_hash: TokenHash,
    pub issued_at: UtcMillis,
    pub expires_at: UtcMillis,
}
