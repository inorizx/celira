//! Pairing Grant 的高熵 opaque Bearer token：生成、格式校验与摘要。
//!
//! 契约（CORE_V1_IM_CONTRACT.md §3.1）：与 Human Session 同标准——
//! 密码学安全随机源、至少 256 bit、稳定可识别但不承载身份信息的前缀；
//! 原始 token 只在成功签发响应 / CLI stdout 出现一次，绝不进入日志、
//! 错误响应或数据库（数据库只存 SHA-256 摘要）。

use base64::Engine;

/// 稳定、可识别但不承载身份信息的前缀（区别于 Human Session 的 `clr_hs_`）。
pub const GRANT_TOKEN_PREFIX: &str = "clr_pg_";
/// 随机部分字节数：256 bit。
const RANDOM_BYTES: usize = 32;
/// 随机部分 base64url（无填充）编码长度：ceil(32/3)*4 - 1 = 43。
const RANDOM_ENCODED_LEN: usize = 43;

/// 一次性输出的 opaque Grant token。Debug 输出被遮蔽，杜绝进入日志。
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct GrantToken(String);

impl GrantToken {
    /// 生成至少 256 bit 密码学安全随机 token。
    pub fn generate() -> Result<Self, TokenGenError> {
        let mut bytes = [0u8; RANDOM_BYTES];
        getrandom::getrandom(&mut bytes).map_err(|e| TokenGenError(e.to_string()))?;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        debug_assert_eq!(encoded.len(), RANDOM_ENCODED_LEN);
        Ok(Self(format!("{GRANT_TOKEN_PREFIX}{encoded}")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 严格校验候选 token 格式（前缀 + 固定长度 base64url 随机部分）。
    ///
    /// 格式不合法返回 None；exchange 入口对一切格式错误统一按
    /// `grant_invalid` 处理，不泄露 Grant 是否曾经存在。
    pub fn parse_candidate(input: &str) -> Option<Self> {
        let rest = input.strip_prefix(GRANT_TOKEN_PREFIX)?;
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
}

impl std::fmt::Debug for GrantToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("GrantToken").field(&"[REDACTED]").finish()
    }
}

/// 密码学随机源失败。
#[derive(Debug)]
pub struct TokenGenError(pub String);

impl std::fmt::Display for TokenGenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "secure random source failed: {}", self.0)
    }
}

impl std::error::Error for TokenGenError {}
