//! Human Session：Core 的安全事实，证明某次 HTTP 调用可以代表指定 Human。
//!
//! 契约（CORE_V1_IDENTITY_AUTH.md 4.2/4.3）：数据库只保存固定长度单向摘要，
//! 原始 token 永不落盘、不进入日志或错误响应。

use sha2::{Digest, Sha256};

use crate::domain::participant::ParticipantId;
use crate::domain::time::UtcMillis;
use crate::domain::token::OpaqueToken;

/// 稳定 Session ID（UUID v4 字符串）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 解析 Core 可识别的稳定 ID：必须是 UUID v4 字符串。
    /// 格式不合法返回 None（用于对外统一「bad-id」与「不存在」）。
    pub fn parse_stable(raw: &str) -> Option<Self> {
        let uuid = uuid::Uuid::parse_str(raw).ok()?;
        (uuid.get_version() == Some(uuid::Version::Random)).then(|| Self(raw.to_owned()))
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 固定长度单向摘要（SHA-256，32 字节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenHash(pub [u8; 32]);

impl TokenHash {
    /// 摘要字节数：32 字节 = 256 bit。
    #[allow(dead_code)]
    pub const LEN: usize = 32;

    /// 对完整 opaque token 计算 SHA-256。随机 Bearer token 的摘要用于精确
    /// 查找，不使用面向低熵密码的慢哈希替代品。
    pub fn from_token(token: &OpaqueToken) -> Self {
        Self::from_token_str(token.as_str())
    }

    /// 对任意高熵 opaque token 文本（Human Session 或 Pairing Grant）计算
    /// SHA-256 摘要；与 `from_token` 同一哈希，纯字符串入口供 Grant token
    /// 复用同一摘要卫生。
    pub fn from_token_str(raw: &str) -> Self {
        Self(Sha256::digest(raw.as_bytes()).into())
    }

    pub fn hex(&self) -> String {
        hex::encode(self.0)
    }

    /// 从 64 字符十六进制还原；长度或字符集不合法返回 None。
    /// （当前由测试直接构造已知摘要使用；未来导出/审计路径复用。）
    #[allow(dead_code)]
    pub fn parse_hex(raw: &str) -> Option<Self> {
        let bytes = hex::decode(raw).ok()?;
        let array: [u8; 32] = bytes.try_into().ok()?;
        Some(Self(array))
    }
}

impl std::fmt::Display for TokenHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.hex())
    }
}

/// Human Session 记录（不含原始 token）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanSession {
    pub id: SessionId,
    pub human_id: ParticipantId,
    pub token_hash: TokenHash,
    pub created_at: UtcMillis,
    pub revoked_at: Option<UtcMillis>,
}

impl HumanSession {
    pub fn new(
        id: SessionId,
        human_id: ParticipantId,
        token_hash: TokenHash,
        created_at: UtcMillis,
    ) -> Self {
        Self {
            id,
            human_id,
            token_hash,
            created_at,
            revoked_at: None,
        }
    }
}
