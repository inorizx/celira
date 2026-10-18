//! Participant Presentation 领域事实：全局 Profile 与 Group Room Presentation。
//!
//! 契约（CORE_V1_PARTICIPANT_PRESENTATION.md §2–§5）：Profile 与 Room
//! Presentation 是相互独立、带版本号的公共呈现事实；文本可选、空串规范化
//! 为 NULL、按 UTF-8 字节上限与 NUL 校验、原样保存；版本 > 0 且单调 +1；
//! base_version >= 0；幂等使用请求摘要（SHA-256 64 位小写 hex，来自**固定字段
//! 序的 canonical JSON**：字段含 base_version 与全部可空编辑字段、空串归一为
//! null、null 显式输出）。投影侧另提供 UTF-8 安全的 preview 辅助（可置于
//! domain：纯函数，不引入 Runtime 字段，也不依赖任何展示框架）。幂等重放
//! 语义（同 key 同摘要恒重放、即使版本已被后续写入推进）由
//! `decide_idempotency` 编码。

use sha2::{Digest, Sha256};

use crate::domain::participant::ParticipantId;
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;

/// 各文本字段的 UTF-8 字节上限（与 migration 的 `length(CAST(value AS BLOB))`
/// CHECK 一致，domain 提前在同一数字上校验）。
pub mod presentation_limits {
    /// Profile 短描述最大 UTF-8 字节数。
    pub const PROFILE_SHORT_DESCRIPTION_MAX_BYTES: usize = 512;
    /// Profile 公开描述最大 UTF-8 字节数。
    pub const PROFILE_PUBLIC_DESCRIPTION_MAX_BYTES: usize = 16_384;
    /// Room Presentation 房间显示名最大 UTF-8 字节数。
    pub const ROOM_DISPLAY_NAME_MAX_BYTES: usize = 80;
    /// Room Presentation 角色名最大 UTF-8 字节数。
    pub const ROOM_ROLE_NAME_MAX_BYTES: usize = 128;
    /// Room Presentation 角色描述最大 UTF-8 字节数。
    pub const ROOM_ROLE_DESCRIPTION_MAX_BYTES: usize = 4_096;
}

/// 呈现文本校验失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationTextError {
    /// 包含 U+0000。
    ContainsNul,
    /// 超过 UTF-8 字节上限（按字节，不是 scalar value 数）。
    TooLong { bytes: usize, max_bytes: usize },
}

impl std::fmt::Display for PresentationTextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ContainsNul => write!(f, "presentation text must not contain U+0000"),
            Self::TooLong { bytes, max_bytes } => write!(
                f,
                "presentation text exceeds {max_bytes} UTF-8 bytes (got {bytes})"
            ),
        }
    }
}

impl std::error::Error for PresentationTextError {}

/// 已校验的可选呈现文本：按调用者提交的正文原样保存，不做 Unicode 归一化、
/// 不做首尾空白裁剪；空串由 `parse_optional` 规范化为 `None`。
/// `MAX_BYTES` 是该字段的 UTF-8 字节上限（SQLite 内用 BLOB length 同样强制）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationText<const MAX_BYTES: usize>(String);

impl<const MAX_BYTES: usize> PresentationText<MAX_BYTES> {
    /// 非空文本校验：禁 U+0000、UTF-8 字节数 <= MAX_BYTES。
    pub fn parse(raw: &str) -> Result<Self, PresentationTextError> {
        if raw.chars().any(|c| c == '\u{0}') {
            return Err(PresentationTextError::ContainsNul);
        }
        let bytes = raw.len();
        if bytes > MAX_BYTES {
            return Err(PresentationTextError::TooLong {
                bytes,
                max_bytes: MAX_BYTES,
            });
        }
        Ok(Self(raw.to_owned()))
    }

    /// 可选文本入口：空字符串规范化为 `None`（契约：空串 == NULL），
    /// 非空文本照常校验。
    pub fn parse_optional(raw: &str) -> Result<Option<Self>, PresentationTextError> {
        if raw.is_empty() {
            return Ok(None);
        }
        Self::parse(raw).map(Some)
    }

    /// 从 `Option<&str>` 归一化：`None` 与空串都是未提供，非空照常校验。
    pub fn parse_opt(raw: Option<&str>) -> Result<Option<Self>, PresentationTextError> {
        match raw {
            None => Ok(None),
            Some(s) => Self::parse_optional(s),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 常见文本别名（字节上限与契约文档一致）。
pub type ShortDescription =
    PresentationText<{ presentation_limits::PROFILE_SHORT_DESCRIPTION_MAX_BYTES }>;
pub type PublicDescription =
    PresentationText<{ presentation_limits::PROFILE_PUBLIC_DESCRIPTION_MAX_BYTES }>;
pub type RoomDisplayName = PresentationText<{ presentation_limits::ROOM_DISPLAY_NAME_MAX_BYTES }>;
pub type RoomRoleName = PresentationText<{ presentation_limits::ROOM_ROLE_NAME_MAX_BYTES }>;
pub type RoomRoleDescription =
    PresentationText<{ presentation_limits::ROOM_ROLE_DESCRIPTION_MAX_BYTES }>;

/// base_version 校验失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseVersionError {
    /// base_version 小于 0（语法/输入错）。
    Negative(i64),
}

impl std::fmt::Display for BaseVersionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Negative(v) => write!(f, "base_version must be >= 0 (got {v})"),
        }
    }
}

impl std::error::Error for BaseVersionError {}

/// 客户端提交的 base_version：客户端认为的当前最新版本，>= 0（无记录时为 0）。
/// 写入裁决「base_version == 当前最新版本」属 application 层，此处只做纯规则
/// 校验与数字访问。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BaseVersion(u64);

impl BaseVersion {
    #[cfg(test)]
    pub const ZERO: BaseVersion = BaseVersion(0);

    pub fn parse(v: i64) -> Result<Self, BaseVersionError> {
        if v < 0 {
            return Err(BaseVersionError::Negative(v));
        }
        Ok(Self(v as u64))
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

/// 版本校验失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionError {
    /// 版本必须大于 0。
    Zero,
}

impl std::fmt::Display for VersionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "presentation version must be > 0")
    }
}

impl std::error::Error for VersionError {}

macro_rules! positive_version {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u64);

        impl $name {
            /// 从数据库 i64 解析；<= 0 视为非法（版本从 1 起）。
            pub fn parse(v: i64) -> Result<Self, VersionError> {
                if v <= 0 {
                    return Err(VersionError::Zero);
                }
                Ok(Self(v as u64))
            }

            /// 从 u64 解析；0 视为非法。
            pub fn from_u64(v: u64) -> Result<Self, VersionError> {
                if v == 0 {
                    return Err(VersionError::Zero);
                }
                Ok(Self(v))
            }

            pub fn as_u64(&self) -> u64 {
                self.0
            }
        }
    };
}

positive_version! {
    /// 版本 > 0 的 Profile 版本号（无行 = 版本 0，由 `latest_profile` 返回 None 表达）。
    ProfileVersion
}

positive_version! {
    /// 版本 > 0 的 Room Presentation 版本号（无行 = 版本 0）。
    PresentationVersion
}

/// 请求摘要：SHA-256 64 位小写十六进制。存于幂等记录，绝不携带正文/预览。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestDigest(String);

impl RequestDigest {
    /// 从规范化字节计算 SHA-256 摘要。
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(hex::encode(Sha256::digest(bytes)))
    }

    /// 从既有 64 位小写 hex 还原（领域规则测试辅助）。
    #[cfg(test)]
    pub fn parse_hex(raw: &str) -> Option<Self> {
        if raw.len() != 64 {
            return None;
        }
        let ok = raw.bytes().all(|b| (b as char).is_ascii_hexdigit())
            && raw.bytes().all(|b| !b.is_ascii_uppercase());
        if !ok {
            return None;
        }
        Some(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 规范化可选值：空串视为未提供（与存储的 NULL 归一化一致，保证 `""` 与
/// `null` 是同一有效请求、同一摘要）。
fn normalized_opt(raw: Option<&str>) -> Option<&str> {
    // `Option::filter` 就是这一件事（clippy `manual_filter` 指的也是它）；判据不变：
    // **空串与"没给"是同一个有效请求**（归一化之后进摘要，两处逐字节同形）。
    raw.filter(|s| !s.is_empty())
}

/// Profile PUT 请求的 canonical 输入：固定字段序、空串已归一为 None。
/// `Option::None` 在 serde_json 中**显式输出 null**（不省略字段）。
#[derive(serde::Serialize)]
struct ProfilePutCanonicalRequest<'a> {
    base_version: u64,
    short_description: Option<&'a str>,
    public_description: Option<&'a str>,
}

/// Room Presentation PUT 请求的 canonical 输入（固定字段序，同上）。
#[derive(serde::Serialize)]
struct RoomPresentationPutCanonicalRequest<'a> {
    base_version: u64,
    room_display_name: Option<&'a str>,
    room_role_name: Option<&'a str>,
    room_role_description: Option<&'a str>,
}

/// Agent Profile 写入的请求摘要。canonical JSON 为固定字段序的
/// `{"base_version":…,"short_description":…,"public_description":…}`
/// （`null` 显式输出，空串归一为 `null`），SHA-256 64 位小写 hex。
/// `base_version` 参与摘要，使「同 key + 不同版本/请求」成为冲突而非重放。
pub fn profile_put_digest(
    base_version: BaseVersion,
    short_description: Option<&str>,
    public_description: Option<&str>,
) -> RequestDigest {
    let canonical = ProfilePutCanonicalRequest {
        base_version: base_version.as_u64(),
        short_description: normalized_opt(short_description),
        public_description: normalized_opt(public_description),
    };
    let bytes = serde_json::to_vec(&canonical)
        .expect("canonical profile request serialization is infallible");
    RequestDigest::from_bytes(&bytes)
}

/// Room Presentation 写入的请求摘要（同理：canonical JSON + base_version）。
pub fn room_presentation_put_digest(
    base_version: BaseVersion,
    room_display_name: Option<&str>,
    room_role_name: Option<&str>,
    room_role_description: Option<&str>,
) -> RequestDigest {
    let canonical = RoomPresentationPutCanonicalRequest {
        base_version: base_version.as_u64(),
        room_display_name: normalized_opt(room_display_name),
        room_role_name: normalized_opt(room_role_name),
        room_role_description: normalized_opt(room_role_description),
    };
    let bytes = serde_json::to_vec(&canonical)
        .expect("canonical room presentation request serialization is infallible");
    RequestDigest::from_bytes(&bytes)
}

/// UTF-8 安全的 preview：返回不超过 `max_bytes` 字节、落在**完整字符边界**
/// 上的前缀（永不截断多字节字符；不做归一化）。文本整体在预算内时原样返回。
pub fn preview(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut idx = max_bytes;
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    &text[..idx]
}

/// 同 scope + 同 key 的既有幂等记录的可见信息（存储 adapter 从幂等表读出）。
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(test)]
pub struct ExistingIdempotency {
    pub request_digest: RequestDigest,
    pub result_version: u64,
    pub result_is_create: bool,
}

/// 幂等/并发裁决后的处置（CORE_V1_PARTICIPANT_PRESENTATION.md §5.1）。
///
/// 裁决顺序：**当前认证与当前授权通过之后**才进入本函数；授权（如 active
/// Owner 关系被撤销）先于重放，被撤销/未授权的 actor 一律不得经重放访问结果。
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(test)]
pub enum IdempotencyDisposition {
    /// 同 key 同请求摘要：重放首次成功记录的结果（`result_version` +
    /// `result_is_create`），**不重新裁决 base_version**，绝不追加版本——
    /// 即使当前最新版本已由其他写入推进。
    Replay {
        result_version: u64,
        result_is_create: bool,
    },
    /// 同 key 不同请求摘要：409 幂等冲突，首次事实不变。
    IdempotencyConflict,
    /// 新 key 且 `base_version !=` 当前最新：409 stale
    /// （`profile_stale` / `room_presentation_stale`），不追加版本。
    Stale { latest_version: u64 },
    /// 新 key 且 `base_version ==` 当前最新：允许写入新版本（= latest + 1）。
    Write,
}

/// 裁决一条写入请求（调用方已通过认证与授权）。`existing` 为同 scope + 同
/// key 的既有幂等记录；`latest_version` 为当前最新版本（无行视为 0）。
#[cfg(test)]
pub fn decide_idempotency(
    existing: Option<&ExistingIdempotency>,
    request_digest: &RequestDigest,
    base_version: u64,
    latest_version: u64,
) -> IdempotencyDisposition {
    if let Some(existing) = existing {
        if existing.request_digest == *request_digest {
            return IdempotencyDisposition::Replay {
                result_version: existing.result_version,
                result_is_create: existing.result_is_create,
            };
        }
        return IdempotencyDisposition::IdempotencyConflict;
    }
    if base_version == latest_version {
        IdempotencyDisposition::Write
    } else {
        IdempotencyDisposition::Stale { latest_version }
    }
}

/// 已提交的 Participant（P0：Agent）全局 Profile 版本行。
///
/// `avatar` 是头像窄通道字段族（CORE_V1_AVATAR_P0.md）：`Some` 表示该版本
/// 行设置了头像（写入），`None` 表示该版本行无头像（文本 Profile 写或摘除
/// 后的版本）；摘除 = 追加新版本且 avatar=None，旧版本行与其字节不可变保留。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantProfileVersion {
    pub participant_id: ParticipantId,
    pub profile_version: ProfileVersion,
    pub short_description: Option<ShortDescription>,
    pub public_description: Option<PublicDescription>,
    pub avatar: Option<crate::domain::participant_avatar::Avatar>,
    pub published_by_id: ParticipantId,
    pub published_at: UtcMillis,
}

/// 已提交的 Group Room Presentation 版本行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomPresentationVersion {
    pub room_id: RoomId,
    pub participant_id: ParticipantId,
    pub presentation_version: PresentationVersion,
    pub room_display_name: Option<RoomDisplayName>,
    pub room_role_name: Option<RoomRoleName>,
    pub room_role_description: Option<RoomRoleDescription>,
    pub published_by_id: ParticipantId,
    pub published_at: UtcMillis,
}
