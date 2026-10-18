//! 基础附件：Core 公共事实的附件领域切片。
//!
//! 契约（CORE_V1_ATTACHMENTS.md）：附件是「两步式」公共事实——multipart
//! 上传产生 pending attachment（Core 本地媒体目录持有字节、SQLite 持有
//! 元数据与状态），随后 Message / Completion 在同一 SQLite 事务内绑定；
//! 未绑定 pending 零公共投影且不可下载；绑定一次后不可解绑。公共协议
//! 只暴露 id/file_name/content_type/size_bytes（与 state/uploaded_at），
//! 永不暴露 storage key、SHA-256 或文件路径。

use crate::domain::participant::ParticipantId;
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;

/// 单附件字节上限：16 MiB。上传请求体上限 = 该值 + multipart 框架开销
/// （16 MiB + 64 KiB，路由层常量）。
pub const MAX_ATTACHMENT_BYTES: usize = 16 * 1024 * 1024;

/// pending 附件的绑定窗口：上传后 24 小时内可由上传者在本 Room 绑定一次。
/// pending 是私有暂存租约：过期后不可绑定、不可下载，且只由内部受控清理
/// （`AttachmentStore::purge_expired_pending`）连同其字节一起回收；bound
/// 附件与 Message/Event 公共事实绝不删除。
pub const PENDING_BINDING_WINDOW_MS: i64 = 86_400_000;

/// file_name 上限（UTF-8 字节）。用户输入只进入元数据，绝不进入路径。
pub const MAX_FILE_NAME_BYTES: usize = 255;

/// content_type 上限（ASCII 可见字符）。不设 MIME 白名单，只校验形状。
pub const MAX_CONTENT_TYPE_BYTES: usize = 255;

/// 稳定、不可复用的附件 ID（UUID v4 字符串，统一小写连字符）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AttachmentId(pub String);

impl AttachmentId {
    /// 新附件 ID：UUID v4 的标准小写连字符格式。
    pub fn new_v4() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// 解析并规范化为小写连字符的严格 UUID v4（版本位 4 + variant nibble
    /// 8/9/a/b，与数据库 CHECK 同一谓词）；其他格式返回 None（对外统一
    /// 「bad-id」与「不存在」）。
    pub fn parse_canonical(raw: &str) -> Option<Self> {
        let uuid = uuid::Uuid::parse_str(raw).ok()?;
        // variant nibble 必须为 8/9/a/b（RFC 4122），与数据库 CHECK 同一
        // 谓词：Microsoft(6/c/d) 与 Future(e/f) variant 一律拒绝。
        let variant_ok = uuid.get_variant() == uuid::Variant::RFC4122;
        (uuid.get_version() == Some(uuid::Version::Random) && variant_ok)
            .then(|| Self(uuid.hyphenated().to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AttachmentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 内部 storage key：Core 生成的随机 UUID v4，是媒体目录中字节文件名的
/// 唯一来源。公共协议永不返回；用户输入（file_name 等）绝不进入路径。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StorageKey(pub String);

impl StorageKey {
    pub fn new_v4() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// 解析并规范化为小写连字符的 UUID v4（版本位 4 + variant nibble
    /// 8/9/a/b，与 `AttachmentId` 及数据库 CHECK 同一谓词）；其他格式
    /// 返回 None。BlobStorage 与 orphan sweep 只接受严格 UUID v4 命名空间
    /// 内的 key。
    pub fn parse_canonical(raw: &str) -> Option<Self> {
        let uuid = uuid::Uuid::parse_str(raw).ok()?;
        // variant nibble 必须为 8/9/a/b（RFC 4122），与 AttachmentId 同一
        // 谓词：Microsoft(6/c/d) 与 Future(e/f) variant 一律拒绝。
        let variant_ok = uuid.get_variant() == uuid::Variant::RFC4122;
        (uuid.get_version() == Some(uuid::Version::Random) && variant_ok)
            .then(|| Self(uuid.hyphenated().to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for StorageKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// file_name 校验失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileNameError {
    /// 空 string。
    Empty,
    /// 包含 U+0000。
    ContainsNul,
    /// UTF-8 字节数超过 255。
    TooLong(usize),
}

impl std::fmt::Display for FileNameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "file name must not be empty"),
            Self::ContainsNul => write!(f, "file name must not contain U+0000"),
            Self::TooLong(len) => write!(f, "file name exceeds 255 UTF-8 bytes (got {len})"),
        }
    }
}

impl std::error::Error for FileNameError {}

/// 已通过校验的 file_name：UTF-8 1-255 bytes、禁 NUL、不 trim 不归一化。
/// 只进入公共元数据，绝不进入文件路径或响应头。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileName(String);

impl FileName {
    pub fn parse(raw: &str) -> Result<Self, FileNameError> {
        let len = raw.len();
        if len == 0 {
            return Err(FileNameError::Empty);
        }
        if len > MAX_FILE_NAME_BYTES {
            return Err(FileNameError::TooLong(len));
        }
        if raw.contains('\u{0}') {
            return Err(FileNameError::ContainsNul);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// content_type 校验失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentTypeError {
    /// 空 string。
    Empty,
    /// 超过 255 bytes。
    TooLong(usize),
    /// 包含非 ASCII 可见字符或空白（允许范围 0x21-0x7E，不含空格、Tab、
    /// CR/LF、DEL 与 Unicode）。
    NotAsciiVisible,
}

impl std::fmt::Display for ContentTypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "content type must not be empty"),
            Self::TooLong(len) => write!(f, "content type exceeds 255 bytes (got {len})"),
            Self::NotAsciiVisible => {
                write!(
                    f,
                    "content type must consist of visible ASCII characters only"
                )
            }
        }
    }
}

impl std::error::Error for ContentTypeError {}

/// 已通过校验的 content_type：非空、1-255 bytes、全部为 ASCII 可见
/// 非空白字符（0x21-0x7E；空格、Tab、CR/LF、DEL 与 Unicode 一律拒绝）。
/// 不设 MIME 白名单，只校验形状。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentType(String);

impl ContentType {
    pub fn parse(raw: &str) -> Result<Self, ContentTypeError> {
        let len = raw.len();
        if len == 0 {
            return Err(ContentTypeError::Empty);
        }
        if len > MAX_CONTENT_TYPE_BYTES {
            return Err(ContentTypeError::TooLong(len));
        }
        if !raw.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
            return Err(ContentTypeError::NotAsciiVisible);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 附件状态：pending（已上传未绑定）或 bound（已随 Message 绑定，终态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentState {
    Pending,
    Bound,
}

impl AttachmentState {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "bound" => Some(Self::Bound),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Bound => "bound",
        }
    }
}

/// 附件公共元数据（Message 视图 / 下载校验使用）：不包含 storage key、
/// hash 或路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentMetadata {
    pub id: AttachmentId,
    pub file_name: FileName,
    pub content_type: ContentType,
    pub size_bytes: i64,
}

/// 一条已持久化的附件公共事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub id: AttachmentId,
    pub uploader_participant_id: ParticipantId,
    pub room_id: RoomId,
    pub file_name: FileName,
    pub content_type: ContentType,
    pub size_bytes: i64,
    pub state: AttachmentState,
    /// 内部 storage key（严格 UUID v4）：媒体目录字节文件名的唯一来源；
    /// 公共协议永不返回。
    pub storage_key: StorageKey,
    /// 持久化字节的 SHA-256（64 字符小写 hex）：只用于下载完整性校验，
    /// 公共 metadata / HTTP 协议永不返回。
    pub sha256_hex: String,
    pub uploaded_at: UtcMillis,
}

impl Attachment {
    pub fn metadata(&self) -> AttachmentMetadata {
        AttachmentMetadata {
            id: self.id.clone(),
            file_name: self.file_name.clone(),
            content_type: self.content_type.clone(),
            size_bytes: self.size_bytes,
        }
    }
}

/// 待持久化的新 pending 附件。id、storage_key、sha256_hex 与 uploaded_at
/// 由 Core 分配（应用层），file_name / content_type / size_bytes 来自上传
/// 请求；state 由存储层按 'pending' 落库。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAttachment {
    pub id: AttachmentId,
    pub uploader_participant_id: ParticipantId,
    pub room_id: RoomId,
    pub file_name: FileName,
    pub content_type: ContentType,
    pub size_bytes: i64,
    pub storage_key: StorageKey,
    /// 上传字节的 SHA-256 hex（内部完整性事实，公共协议永不返回）。
    pub sha256_hex: String,
    pub uploaded_at: UtcMillis,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_id_parses_strict_canonical_uuid_v4() {
        let ok = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(
            AttachmentId::parse_canonical(ok).unwrap(),
            AttachmentId(ok.into())
        );
        // 大写与无连字符（32 hex）输入都被规范化（parse_canonical 的输出
        // 恒为小写连字符；数据库守卫另行拒绝大写/畸形形式的直接写入）。
        for raw in [
            "550E8400-E29B-41D4-A716-446655440000",
            "550e8400e29b41d4a716446655440000",
        ] {
            assert_eq!(
                AttachmentId::parse_canonical(raw).unwrap(),
                AttachmentId("550e8400-e29b-41d4-a716-446655440000".into())
            );
        }
        for bad in [
            "",
            "not-a-uuid",
            "550e8400-e29b-31d4-a716-446655440000",
            "550e8400-e29b-41d4-6716-446655440000",
            "550e8400-e29b-41d4-a716-44665544000",
        ] {
            assert!(AttachmentId::parse_canonical(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn storage_key_is_canonical_uuid_v4_only() {
        let key = StorageKey::new_v4();
        assert_eq!(StorageKey::parse_canonical(key.as_str()).unwrap(), key);
        for bad in ["../etc/passwd", "a/b", "", "not-a-uuid", "KEY-1"] {
            assert!(StorageKey::parse_canonical(bad).is_none(), "{bad:?}");
        }
        // 与 AttachmentId 同一谓词：版本位非 4 / variant 非 RFC4122
        // （6/c/d Microsoft、e/f Future）一律拒绝。
        for bad in [
            "550e8400-e29b-31d4-a716-446655440000",
            "550e8400-e29b-41d4-6716-446655440000",
            "550e8400-e29b-41d4-c716-446655440000",
            "550e8400-e29b-41d4-e716-446655440000",
        ] {
            assert!(StorageKey::parse_canonical(bad).is_none(), "{bad:?}");
        }
        // RFC4122 variant 8/9/a/b 均接受。
        for ok in [
            "550e8400-e29b-41d4-8716-446655440000",
            "550e8400-e29b-41d4-9716-446655440000",
            "550e8400-e29b-41d4-a716-446655440000",
            "550e8400-e29b-41d4-b716-446655440000",
        ] {
            assert_eq!(
                StorageKey::parse_canonical(ok).unwrap().as_str(),
                ok,
                "{ok:?}"
            );
        }
    }

    #[test]
    fn file_name_accepts_unicode_and_rejects_nul_empty_and_too_long() {
        assert_eq!(FileName::parse("报告.pdf").unwrap().as_str(), "报告.pdf");
        assert_eq!(FileName::parse("a\u{0}b"), Err(FileNameError::ContainsNul));
        assert_eq!(FileName::parse(""), Err(FileNameError::Empty));
        assert_eq!(
            FileName::parse(&"a".repeat(256)),
            Err(FileNameError::TooLong(256))
        );
        assert!(FileName::parse(&"a".repeat(255)).is_ok());
        // 255 个中文字符 = 765 UTF-8 bytes：必须按字节拒绝。
        assert_eq!(
            FileName::parse(&"中".repeat(255)),
            Err(FileNameError::TooLong(765))
        );
    }

    #[test]
    fn content_type_requires_visible_ascii_within_cap() {
        assert_eq!(
            ContentType::parse("application/pdf").unwrap().as_str(),
            "application/pdf"
        );
        assert_eq!(
            ContentType::parse("text/plain;charset=utf-8")
                .unwrap()
                .as_str(),
            "text/plain;charset=utf-8"
        );
        assert_eq!(ContentType::parse(""), Err(ContentTypeError::Empty));
        for bad in [
            "application/pdf\n",
            "application/pdf\x7f",
            "text/中文",
            "a\tb",
            "a\u{0}b",
            "application/pdf ",
            "a b",
            " application/pdf",
        ] {
            assert_eq!(
                ContentType::parse(bad),
                Err(ContentTypeError::NotAsciiVisible),
                "{bad:?}"
            );
        }
        assert_eq!(
            ContentType::parse(&"a".repeat(256)),
            Err(ContentTypeError::TooLong(256))
        );
        assert!(ContentType::parse(&"a".repeat(255)).is_ok());
    }

    #[test]
    fn attachment_state_round_trips() {
        assert_eq!(
            AttachmentState::parse("pending"),
            Some(AttachmentState::Pending)
        );
        assert_eq!(
            AttachmentState::parse("bound"),
            Some(AttachmentState::Bound)
        );
        assert_eq!(AttachmentState::parse("other"), None);
        assert_eq!(AttachmentState::Pending.as_str(), "pending");
        assert_eq!(AttachmentState::Bound.as_str(), "bound");
    }
}
