//! Backend Binding 与 Backend Credential：Core 公共事实的第五张领域切片。
//!
//! 契约（CORE_V1_BACKEND_BINDINGS.md 4.x）：Binding 是 Agent 与获准驱动它
//! 的外部 Backend 之间的长期授权连接，不是 Participant、Runtime 配置或
//! 在线状态；Credential 是 Binding 下的一次可撤销秘密能力，只证明外部
//! Backend 被 Human Owner 授权代表指定 Agent，不改变 Agent 身份，也不
//! 产生 Room Membership 或消息权限。原始 token 只显示一次，数据库只保存
//! SHA-256 摘要；token/hash 不进入日志、错误响应或 Debug 输出。

use base64::Engine;
use sha2::{Digest, Sha256};

use crate::domain::participant::ParticipantId;
use crate::domain::time::UtcMillis;
use crate::domain::token::TokenGenError;

/// Backend token 稳定前缀：可识别但不承载 Agent、Binding、Credential、
/// Owner 或部署信息。
pub const BACKEND_TOKEN_PREFIX: &str = "clr_bc_";
/// 随机部分字节数：256 bit。
const RANDOM_BYTES: usize = 32;
/// 随机部分 base64url（无填充）编码长度：ceil(32/3)*4 - 1 = 43。
const RANDOM_ENCODED_LEN: usize = 43;

/// 稳定 Binding ID（UUID v4 字符串）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BindingId(pub String);

impl BindingId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BindingId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 稳定 Credential ID（UUID v4 字符串）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CredentialId(pub String);

impl CredentialId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CredentialId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Binding / Credential 生命周期状态。revoked 是终态：不恢复、不硬删除、
/// 不改写原 revoked_at。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendStatus {
    Active,
    Revoked,
}

impl BackendStatus {
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

/// Backend label 校验失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendLabelError {
    /// 没有非空白字符。
    NoNonWhitespace,
    /// 包含 U+0000。
    ContainsNul,
    /// 超过 80 个 Unicode scalar values。
    TooLong(usize),
}

impl std::fmt::Display for BackendLabelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoNonWhitespace => {
                write!(
                    f,
                    "backend label must contain at least one non-whitespace character"
                )
            }
            Self::ContainsNul => write!(f, "backend label must not contain U+0000"),
            Self::TooLong(len) => write!(
                f,
                "backend label exceeds 80 Unicode scalar values (got {len})"
            ),
        }
    }
}

impl std::error::Error for BackendLabelError {}

/// 已通过校验的 Backend label：仅用于 Owner 管理和审计，不是 Backend 类型
/// 注册表。校验后按提交原文保存，不做 Unicode normalization。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendLabel(String);

impl BackendLabel {
    /// 规则（契约 4.1）：至少一个非空白字符、最多 80 个 Unicode scalar
    /// values、禁止 U+0000；不做归一化。
    pub fn parse(raw: &str) -> Result<Self, BackendLabelError> {
        if raw.chars().any(|c| c == '\u{0}') {
            return Err(BackendLabelError::ContainsNul);
        }
        let scalar_count = raw.chars().count();
        if scalar_count > 80 {
            return Err(BackendLabelError::TooLong(scalar_count));
        }
        if !raw.chars().any(|c| !c.is_whitespace()) {
            return Err(BackendLabelError::NoNonWhitespace);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 一次性输出的 Backend token。Debug 输出被遮蔽，杜绝进入日志。
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct BackendToken(String);

impl BackendToken {
    /// 生成至少 256 bit 密码学安全随机 token，格式为 `clr_bc_` + 43 字符
    /// base64url 无填充随机串。
    pub fn generate() -> Result<Self, TokenGenError> {
        let mut bytes = [0u8; RANDOM_BYTES];
        getrandom::getrandom(&mut bytes).map_err(|e| TokenGenError(e.to_string()))?;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        debug_assert_eq!(encoded.len(), RANDOM_ENCODED_LEN);
        Ok(Self(format!("{BACKEND_TOKEN_PREFIX}{encoded}")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 严格校验候选 token 格式（前缀 + 固定长度 base64url 随机部分）。
    ///
    /// 格式不合法返回 None；Backend 认证入口对一切格式错误一律按
    /// `invalid_credential` 处理，不泄露 Credential 是否曾经存在。
    pub fn parse_candidate(input: &str) -> Option<Self> {
        let rest = input.strip_prefix(BACKEND_TOKEN_PREFIX)?;
        if rest.len() != RANDOM_ENCODED_LEN {
            return None;
        }
        if !rest
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return None;
        }
        Some(Self(input.to_owned()))
    }

    /// 完整 token 的 SHA-256 摘要（64 字符小写 hex）。原始 token 永不落盘，
    /// 数据库只保存该摘要；hash 只用于高熵随机 token 的精确查找。
    pub fn sha256_hex(&self) -> String {
        hex::encode(Sha256::digest(self.0.as_bytes()))
    }
}

impl std::fmt::Debug for BackendToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("BackendToken").field(&"[REDACTED]").finish()
    }
}

/// Backend Binding 公共管理事实（不含幂等协调列）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendBinding {
    pub id: BindingId,
    pub agent_id: ParticipantId,
    pub created_by_human_id: ParticipantId,
    pub label: BackendLabel,
    pub status: BackendStatus,
    pub created_at: UtcMillis,
    pub revoked_at: Option<UtcMillis>,
}

/// Backend Credential 公共管理事实（不含 token hash、replaces ID 与幂等
/// 协调列）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendCredential {
    pub id: CredentialId,
    pub binding_id: BindingId,
    pub issued_by_human_id: ParticipantId,
    pub status: BackendStatus,
    pub created_at: UtcMillis,
    pub revoked_at: Option<UtcMillis>,
}
