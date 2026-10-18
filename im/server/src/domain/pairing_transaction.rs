//! A2 配对事务领域事实（HOME_ACCESS.md §4.3 A2，窄解冻切片）。
//!
//! 只新增本模块事实：Host 预生成高熵 provisional session token、commit
//! token 与 client_nonce，Core 只接收 SHA-256 摘要；prepare 创建或幂等
//! 重用 `pairing_transaction`，先于正式 Human Session 的创建且不消费
//! Grant；commit 在一个事务内激活 reserved session 并原子消费 Grant。
//! 原始 token 从不进入 Core 响应，Core 只存摘要。

use crate::domain::participant::ParticipantId;
use crate::domain::time::UtcMillis;

/// 稳定 Transaction ID（UUID v4 字符串）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TransactionId(pub String);

impl TransactionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 生成新的 UUID v4 Transaction ID。
    pub fn new_v4() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// 解析 Core 可识别的稳定 ID：必须是 UUID v4 v4 版本位且 variant 为
    /// RFC4122（8/9/a/b nibble，与既有 GrantId/SessionId 同一谓词）。
    /// 格式不合法返回 None（用于对外统一「bad-id」与「不存在」）。
    pub fn parse_stable(raw: &str) -> Option<Self> {
        let uuid = uuid::Uuid::parse_str(raw).ok()?;
        (uuid.get_version() == Some(uuid::Version::Random)
            && uuid.get_variant() == uuid::Variant::RFC4122)
            .then(|| Self(raw.to_owned()))
    }
}

impl std::fmt::Display for TransactionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 事务状态机（对外展示）。`expired` 是 derived 状态：存储状态为 prepared
/// 且 `expires_at` 已过时对外呈现 expired（不落库改写）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionStatus {
    Prepared,
    Committed,
    Aborted,
    Expired,
}

impl TransactionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Committed => "committed",
            Self::Aborted => "aborted",
            Self::Expired => "expired",
        }
    }

    /// 数据库枚举值 → 领域类型；未知值返回 None。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "prepared" => Some(Self::Prepared),
            "committed" => Some(Self::Committed),
            "aborted" => Some(Self::Aborted),
            _ => None,
        }
    }
}

/// Pairing Transaction 记录（不含任何原始 token；commit 后 session 摘要
/// 仍在，供 Host 校验预留）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingTransaction {
    pub transaction_id: TransactionId,
    pub grant_id: crate::domain::home::GrantId,
    /// prepare 提交的预留 Session ID：commit 用同一 ID 落正式 Human Session；
    /// 展示时用于让 Host 识别自己的预留（绝不用于认证）。
    pub reserved_session_id: crate::domain::human_session::SessionId,
    pub issued_by: ParticipantId,
    pub status: TransactionStatus,
    pub prepared_at: UtcMillis,
    pub expires_at: UtcMillis,
    pub committed_at: Option<UtcMillis>,
    pub aborted_at: Option<UtcMillis>,
}

impl PairingTransaction {
    /// 以「存储状态 + 当前时间」呈现对外状态：prepared 且已过 expires_at
    /// 视为 expired（派生，不落库）；其余状态原样。
    pub fn status_at(&self, now: UtcMillis) -> TransactionStatus {
        match self.status {
            TransactionStatus::Prepared if self.expires_at <= now => TransactionStatus::Expired,
            other => other,
        }
    }

    /// 事务是否可提交：存储状态 prepared 且当前时间未过期。
    pub fn is_committable_at(&self, now: UtcMillis) -> bool {
        self.status == TransactionStatus::Prepared && now < self.expires_at
    }
}

/// 非秘密事务视图（commit/status 成功路径共用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionView {
    pub transaction_id: TransactionId,
    pub status: TransactionStatus,
    pub home_id: crate::domain::home::HomeId,
    pub human_id: ParticipantId,
    pub reserved_session_id: crate::domain::human_session::SessionId,
    pub prepared_at: UtcMillis,
    pub expires_at: UtcMillis,
    pub committed_at: Option<UtcMillis>,
    pub aborted_at: Option<UtcMillis>,
}
