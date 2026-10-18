//! Participant Avatar 领域事实：头像的 participant 窄通道（CORE_V1_AVATAR_P0.md）。
//!
//! 头像不是独立版本链：它是 Profile 版本链（participant_profile_versions）的
//! 一个字段族——写入/摘除 = 追加新 Profile 版本（profile_version +1），与文本
//! 字段共用同一条链、同一套 Idempotency-Key + base_version 裁决、同一份幂等
//! 记录表。本模块只定义头像自身的事实形状（白名单 Content-Type、字节上限、
//! 内部完整性引用）与写入/摘除的 canonical 请求摘要，不新增版本链机制。

use crate::domain::attachment::StorageKey;
use crate::domain::participant_presentation::{BaseVersion, RequestDigest};
use crate::domain::time::UtcMillis;

/// 单头像字节上限：2 MiB（契约 §3.3）。上传请求体上限 = 该值 + multipart
/// 框架开销（路由层常量）。头像与消息附件不同：是内联展示图，必须排除
/// SVG/HTML 等可执行类型（见 `AvatarContentType` 白名单）。
pub const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;

/// 头像 Content-Type 白名单校验失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvatarContentTypeError {
    /// 不在 image/png | image/jpeg | image/webp 白名单内。
    NotWhitelisted,
}

impl std::fmt::Display for AvatarContentTypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "avatar content type must be one of image/png, image/jpeg, image/webp"
        )
    }
}

impl std::error::Error for AvatarContentTypeError {}

/// 已通过白名单校验的 Content-Type。头像是内联展示图：只允许 PNG/JPEG/WebP，
/// SVG/HTML 等可执行/可解析类型一律拒绝（与消息附件的无白名单形状校验
/// 不同，契约 §3.3——头像写路径有 MIME 白名单）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvatarContentType {
    Png,
    Jpeg,
    Webp,
}

impl AvatarContentType {
    pub fn parse(raw: &str) -> Result<Self, AvatarContentTypeError> {
        match raw {
            "image/png" => Ok(Self::Png),
            "image/jpeg" => Ok(Self::Jpeg),
            "image/webp" => Ok(Self::Webp),
            _ => Err(AvatarContentTypeError::NotWhitelisted),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }
}

/// 头像内部完整事实（随 Profile 版本行持久化）。公共投影
/// `ProfileRead::avatar` 只输出 `AvatarMeta`（存在性 + content_type +
/// size_bytes），绝不下行 storage key / SHA-256 / 路径（契约 §3.3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Avatar {
    /// 内部 storage key（严格 UUID v4）：媒体目录字节文件名的唯一来源；
    /// 公共协议永不返回。
    pub storage_key: StorageKey,
    pub content_type: AvatarContentType,
    pub size_bytes: i64,
    /// 持久化字节的 SHA-256（64 位小写 hex）：下载完整性校验专用，公共
    /// 协议永不返回。
    pub sha256_hex: String,
    /// 头像最后设置/更新的 Core 时间（随版本行不可变）。
    pub updated_at: UtcMillis,
}

/// 头像公共投影元数据（存在性 + content_type + size_bytes；版本在所在
/// Profile 版本行的 profile_version 上寻址）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvatarMeta {
    pub content_type: AvatarContentType,
    pub size_bytes: i64,
}

impl From<&Avatar> for AvatarMeta {
    fn from(avatar: &Avatar) -> Self {
        Self {
            content_type: avatar.content_type,
            size_bytes: avatar.size_bytes,
        }
    }
}

/// 头像 PUT 请求的 canonical 输入：固定字段序（base_version + content_type +
/// 字节 SHA-256，不携带字节本身）。
#[derive(serde::Serialize)]
struct AvatarPutCanonicalRequest<'a> {
    base_version: u64,
    content_type: &'a str,
    sha256: &'a str,
}

/// 头像摘除请求的 canonical 输入：固定字段序（base_version + removed=true）。
#[derive(serde::Serialize)]
struct AvatarRemoveCanonicalRequest {
    base_version: u64,
    removed: bool,
}

/// 头像写入的请求摘要。canonical JSON 为固定字段序的
/// `{"base_version":…,"content_type":"image/png","sha256":"<64 hex>"}`
/// （不存字节本身），SHA-256 64 位小写 hex。与文本 Profile PUT / 摘除的
/// 摘要形状互不相同（同一幂等表内同 key 异摘要 → 409）。
pub fn avatar_put_digest(
    base_version: BaseVersion,
    content_type: AvatarContentType,
    sha256_hex: &str,
) -> RequestDigest {
    let canonical = AvatarPutCanonicalRequest {
        base_version: base_version.as_u64(),
        content_type: content_type.as_str(),
        sha256: sha256_hex,
    };
    let bytes = serde_json::to_vec(&canonical)
        .expect("canonical avatar request serialization is infallible");
    RequestDigest::from_bytes(&bytes)
}

/// 头像摘除的请求摘要：`{"base_version":…,"removed":true}`。
pub fn avatar_remove_digest(base_version: BaseVersion) -> RequestDigest {
    let canonical = AvatarRemoveCanonicalRequest {
        base_version: base_version.as_u64(),
        removed: true,
    };
    let bytes = serde_json::to_vec(&canonical)
        .expect("canonical avatar remove serialization is infallible");
    RequestDigest::from_bytes(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_whitelist_round_trips_and_rejects_executable_types() {
        for (raw, expected) in [
            ("image/png", AvatarContentType::Png),
            ("image/jpeg", AvatarContentType::Jpeg),
            ("image/webp", AvatarContentType::Webp),
        ] {
            assert_eq!(AvatarContentType::parse(raw), Ok(expected));
            assert_eq!(expected.as_str(), raw);
        }
        for bad in [
            "image/svg+xml",
            "image/gif",
            "text/html",
            "application/octet-stream",
            "image/png;charset=utf-8",
            "",
            "IMAGE/PNG",
        ] {
            assert_eq!(
                AvatarContentType::parse(bad),
                Err(AvatarContentTypeError::NotWhitelisted),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn avatar_put_digest_is_stable_and_includes_all_fields() {
        let bv = BaseVersion::parse(3).unwrap();
        let d1 = avatar_put_digest(bv, AvatarContentType::Png, "a".repeat(64).as_str());
        let d2 = avatar_put_digest(bv, AvatarContentType::Png, "a".repeat(64).as_str());
        assert_eq!(d1, d2, "same inputs must produce the same digest");
        assert_eq!(d1.as_str().len(), 64);

        let different_bytes =
            avatar_put_digest(bv, AvatarContentType::Png, "b".repeat(64).as_str());
        assert_ne!(
            d1, different_bytes,
            "different sha256 must change the digest"
        );
        let different_type =
            avatar_put_digest(bv, AvatarContentType::Jpeg, "a".repeat(64).as_str());
        assert_ne!(
            d1, different_type,
            "different content type must change the digest"
        );
        let different_version = avatar_put_digest(
            BaseVersion::parse(4).unwrap(),
            AvatarContentType::Png,
            "a".repeat(64).as_str(),
        );
        assert_ne!(
            d1, different_version,
            "base_version participates in the digest"
        );
    }

    #[test]
    fn avatar_remove_digest_is_stable_and_distinct_from_put() {
        let bv = BaseVersion::parse(2).unwrap();
        let r1 = avatar_remove_digest(bv);
        let r2 = avatar_remove_digest(bv);
        assert_eq!(r1, r2);
        let r_other_version = avatar_remove_digest(BaseVersion::parse(3).unwrap());
        assert_ne!(r1, r_other_version);
        let put = avatar_put_digest(
            BaseVersion::parse(2).unwrap(),
            AvatarContentType::Png,
            "a".repeat(64).as_str(),
        );
        assert_ne!(
            r1, put,
            "remove digest shape must differ from put digest shape"
        );
        // 与文本 Profile 摘要形状亦不同（同表同 key 空间内可区分）。
        let profile = crate::domain::participant_presentation::profile_put_digest(
            BaseVersion::parse(2).unwrap(),
            None,
            None,
        );
        assert_ne!(r1, profile);
    }
}
