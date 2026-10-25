//! Group Room Profile（群简介）领域事实：房间自己的版本化公共资料。
//!
//! 契约（CORE_V1_ROOM_PROFILE.md §3–§5）：简介是 **group Room 的公共事实**，
//! 只有该房当前 active owner（Human 或 Agent，owner 是房内治理角色而非
//! 参与者 kind）能写；正文 1..2000 UTF-8 字节、禁
//! U+0000、`trim` 后非空，原样保存（不做 Unicode 归一化、不 trim）；版本
//! > 0 且同 room 严格 +1；没有版本行 = 没有简介（读面 `profile: null`）。
//!
//! 幂等与 Profile / Room Presentation 同一形状：`base_version` 参与 canonical
//! JSON 摘要（固定字段序、`null` 显式输出），同 key 同摘要恒重放、同 key 不同
//! 摘要冲突、新 key 才要求 `base_version == 当前最新`。`BaseVersion` 与
//! `RequestDigest` 是既有公共概念，直接复用，不复制第二份。
//!
//! 简介**不进消息 / 投递链**，不产生未读或通知；它只是房间的读时上下文。

use crate::domain::participant::ParticipantId;
use crate::domain::participant_presentation::{BaseVersion, RequestDigest};
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;

/// 简介正文的 UTF-8 字节上限（与 migration 的
/// `length(CAST(description AS BLOB))` CHECK 一致，domain 提前在同一数字上校验）。
pub mod room_profile_limits {
    /// 房间简介最大 UTF-8 字节数。
    pub const ROOM_DESCRIPTION_MAX_BYTES: usize = 2_000;
}

/// 简介正文校验失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomDescriptionError {
    /// `trim` 之后为空（空串或纯空白）：简介必须有一句可读的话。
    Empty,
    /// 包含 U+0000（公共文本永不携带 NUL）。
    ContainsNul,
    /// 超过 UTF-8 字节上限（按字节，不是 scalar value 数）。
    TooLong { bytes: usize, max_bytes: usize },
}

impl std::fmt::Display for RoomDescriptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "room description must not be blank"),
            Self::ContainsNul => write!(f, "room description must not contain U+0000"),
            Self::TooLong { bytes, max_bytes } => write!(
                f,
                "room description exceeds {max_bytes} UTF-8 bytes (got {bytes})"
            ),
        }
    }
}

impl std::error::Error for RoomDescriptionError {}

/// 已校验的群简介正文：按提交原文保存（允许换行 / 回车 / Tab / Emoji 与正常
/// Unicode）；`trim` 只用于「是否为空」的裁决，不改写存储值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomDescription(String);

impl RoomDescription {
    /// 非空文本校验：禁 U+0000、`trim` 后非空、UTF-8 字节数 <= 2000。
    pub fn parse(raw: &str) -> Result<Self, RoomDescriptionError> {
        if raw.chars().any(|c| c == '\u{0}') {
            return Err(RoomDescriptionError::ContainsNul);
        }
        if raw.trim().is_empty() {
            return Err(RoomDescriptionError::Empty);
        }
        let bytes = raw.len();
        if bytes > room_profile_limits::ROOM_DESCRIPTION_MAX_BYTES {
            return Err(RoomDescriptionError::TooLong {
                bytes,
                max_bytes: room_profile_limits::ROOM_DESCRIPTION_MAX_BYTES,
            });
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 版本校验失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomProfileVersionError {
    /// 版本必须大于 0。
    Zero,
}

impl std::fmt::Display for RoomProfileVersionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "room profile version must be > 0")
    }
}

impl std::error::Error for RoomProfileVersionError {}

/// 版本 > 0 的房间简介版本号（无行 = 版本 0，由 `latest_room_profile` 返回 None 表达）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RoomProfileVersion(u64);

impl RoomProfileVersion {
    /// 从数据库 i64 解析；<= 0 视为非法（版本从 1 起）。
    pub fn parse(v: i64) -> Result<Self, RoomProfileVersionError> {
        if v <= 0 {
            return Err(RoomProfileVersionError::Zero);
        }
        Ok(Self(v as u64))
    }

    /// 从 u64 解析；0 视为非法。
    pub fn from_u64(v: u64) -> Result<Self, RoomProfileVersionError> {
        if v == 0 {
            return Err(RoomProfileVersionError::Zero);
        }
        Ok(Self(v))
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

/// 群简介 PUT 请求的 canonical 输入：固定字段序。
/// `description` 已在入口按原文校验（空串 / 纯空白被拒绝，不存在 NULL 形态）。
#[derive(serde::Serialize)]
struct RoomProfilePutCanonicalRequest<'a> {
    base_version: u64,
    description: &'a str,
}

/// 群简介写入的请求摘要：canonical JSON
/// `{"base_version":…,"description":"…"}` 的 SHA-256 64 位小写 hex。
/// `base_version` 参与摘要，使「同 key + 不同版本 / 正文」成为冲突而非重放。
pub fn room_profile_put_digest(base_version: BaseVersion, description: &str) -> RequestDigest {
    let canonical = RoomProfilePutCanonicalRequest {
        base_version: base_version.as_u64(),
        description,
    };
    let bytes = serde_json::to_vec(&canonical)
        .expect("canonical room profile request serialization is infallible");
    RequestDigest::from_bytes(&bytes)
}

/// 一次待提交的群简介版本行（完整内部事实，含写入者与时间）。
/// adapter 在同一事务内把它写成不可变版本行 + 幂等见证。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomProfileVersionRow {
    pub room_id: RoomId,
    pub version: RoomProfileVersion,
    pub description: RoomDescription,
    pub updated_by: ParticipantId,
    pub updated_at: UtcMillis,
}
