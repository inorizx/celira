//! 普通文本 Message：Core 公共事实的第四张领域切片。
//!
//! 契约（CORE_V1_MESSAGES.md 4/6）：Message 是 Room 内不可变、有序的
//! 公共事实，由 Core 独立裁决；Message ID 稳定（UUID v4），正文是原样
//! 保存的 UTF-8 文本，只校验字节边界与 NUL，不 trim、不归一化、不转换
//! 成第二种正文格式。序列号与创建时间由 Core 分配，客户端不可指定。

use crate::domain::attachment::AttachmentMetadata;
use crate::domain::participant::ParticipantId;
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;

/// 稳定、不可复用的 Message ID（UUID v4 字符串，统一小写连字符）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageId(pub String);

impl MessageId {
    /// 新 Message ID：UUID v4 的标准小写连字符格式。
    pub fn new_v4() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// 解析并规范化为小写连字符的 UUID v4；其他格式返回 None（用于对外
    /// 统一「bad-id」与「不存在」；read cursor PUT 的目标 message_id 解析
    /// 失败按 400 invalid_request 拒绝）。
    pub fn parse_canonical(raw: &str) -> Option<Self> {
        let uuid = uuid::Uuid::parse_str(raw).ok()?;
        (uuid.get_version() == Some(uuid::Version::Random))
            .then(|| Self(uuid.hyphenated().to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MessageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 正文校验失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageBodyError {
    /// 空 string。
    Empty,
    /// 包含 U+0000。
    ContainsNul,
    /// UTF-8 字节数超过 16384。
    TooLong(usize),
}

impl std::fmt::Display for MessageBodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "message body must not be empty"),
            Self::ContainsNul => write!(f, "message body must not contain U+0000"),
            Self::TooLong(len) => write!(f, "message body exceeds 16384 UTF-8 bytes (got {len})"),
        }
    }
}

impl std::error::Error for MessageBodyError {}

/// 已通过校验的普通文本正文：按调用者提交的正文原样保存（`Some`）；
/// 附件-only Message 的正文为 `None`（契约 CORE_V1_ATTACHMENTS.md 5.4：
/// body 与 attachments 至少一个）。
///
/// 规则（契约 6）：UTF-8 编码后长度为 1-16384 bytes；禁止 U+0000；
/// 允许换行、回车、Tab、Emoji 与正常 Unicode；不做 trim、不做 Unicode
/// normalization、不静默删除、不截断。纯空白非空正文合法。
/// 幂等比较使用解析后、未规范化的完整 string 精确比较。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageBody(String);

impl MessageBody {
    pub fn parse(raw: &str) -> Result<Self, MessageBodyError> {
        let len = raw.len();
        if len == 0 {
            return Err(MessageBodyError::Empty);
        }
        if len > 16384 {
            return Err(MessageBodyError::TooLong(len));
        }
        if raw.contains('\u{0}') {
            return Err(MessageBodyError::ContainsNul);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 一条已提交 Message 的公共事实。
///
/// 不含 idempotency key：key 是 messages 事实行的内部协调列，永不进入
/// 公共视图或 History。`mentions` 是显式结构化唤醒事实（group 消息专用，
/// 规范 ID 排序，每个目标至多一次；非 group 消息恒为空数组）：Mention
/// 记录与 source Message 在同一 SQLite 事务提交，并参与 Message 的幂等
/// 重放裁决（同 actor + key + Room + body + Mention 集合才是重放）。
/// `reply_to_message_id` 是 source Message 的回复/引用事实（契约
/// CORE_V1_REPLIES_AND_QUOTES.md）：同 Room、对 source actor 任一历史
/// Membership 窗口可见的已提交历史 Message；`None` 表示普通消息。该事实
/// 直接固化在 messages 行的不可变列上，本字段不复制 target 正文、actor
/// 资料、sequence 或可见性快照，并参与幂等重放裁决。
/// `recalled_at` 是 MessageRecall 事实（契约 CORE_V1_MESSAGE_RECALL.md）的
/// 只读投影：`None` 表示未撤回；`Some(首次撤回时点)` 表示已撤回。它不
/// 改写任何 messages 事实列——撤回只由独立 message_recalls 行成立，本字段
/// 由存储层在同一查询中读取该事实填充。公共视图层据此把 body 投影为
/// null、mentions 投影为空数组（内部比较与审计仍保留原 body/mentions）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: MessageId,
    pub room_id: RoomId,
    /// Room 内正整数 sequence；由写入事务按该 Room 当前最大 sequence
    /// 分配，消费者不得依赖其永久无间隙。
    pub sequence: i64,
    pub actor_id: ParticipantId,
    /// 正文：普通 Message 为 `Some`；附件-only Message 为 `None`（契约
    /// CORE_V1_ATTACHMENTS.md：body 与 attachments 至少一个）。
    pub body: Option<MessageBody>,
    /// 规范排序后的 Mention 目标 Participant ID 列表；无 Mention 为空数组。
    pub mentions: Vec<ParticipantId>,
    /// 绑定附件公共元数据（按 position 升序，契约
    /// CORE_V1_ATTACHMENTS.md 4）：参与幂等重放裁决的有序集合；Message
    /// 撤回后公共投影由视图层清空，本事实字段保留。
    pub attachments: Vec<AttachmentMetadata>,
    /// 回复/引用的目标 Message ID；普通消息为 None。
    pub reply_to_message_id: Option<MessageId>,
    pub created_at: UtcMillis,
    /// 撤回事实投影：None = 未撤回；Some(recalled_at) = 已撤回。
    pub recalled_at: Option<UtcMillis>,
}

impl Message {
    /// 新 Message：id 与 created_at 由 Core 分配，sequence 由存储事务分配。
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(
        id: MessageId,
        room_id: RoomId,
        actor_id: ParticipantId,
        body: MessageBody,
        created_at: UtcMillis,
    ) -> Self {
        Self {
            id,
            room_id,
            sequence: 0,
            actor_id,
            body: Some(body),
            mentions: Vec::new(),
            attachments: Vec::new(),
            reply_to_message_id: None,
            created_at,
            recalled_at: None,
        }
    }

    /// 新附件-only Message：body 为 None（附件随绑定元数据进入
    /// `attachments`）。
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new_attachment_only(
        id: MessageId,
        room_id: RoomId,
        actor_id: ParticipantId,
        created_at: UtcMillis,
    ) -> Self {
        Self {
            id,
            room_id,
            sequence: 0,
            actor_id,
            body: None,
            mentions: Vec::new(),
            attachments: Vec::new(),
            reply_to_message_id: None,
            created_at,
            recalled_at: None,
        }
    }

    /// 写入后由存储层回填真实 sequence（事务内裁决的结果）。
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_sequence(mut self, sequence: i64) -> Self {
        self.sequence = sequence;
        self
    }

    /// 回填规范排序后的 Mention 目标列表（存储层读取/写入时使用）。
    pub fn with_mentions(mut self, mentions: Vec<ParticipantId>) -> Self {
        self.mentions = mentions;
        self
    }

    /// 回填绑定附件公共元数据（存储层读取时使用；按 position 升序）。
    pub fn with_attachments(mut self, attachments: Vec<AttachmentMetadata>) -> Self {
        self.attachments = attachments;
        self
    }

    /// 回填撤回事实投影（存储层读取时使用；None = 未撤回）。
    pub fn with_recalled_at(mut self, recalled_at: Option<UtcMillis>) -> Self {
        self.recalled_at = recalled_at;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_accepts_normal_unicode_and_whitespace_only() {
        for text in [
            "hello",
            "你好，世界",
            "line1\nline2\r\nline3\ttab",
            "😀 emoji",
            " ",
            "\t\n\r ",
            "trailing space ",
        ] {
            let body = MessageBody::parse(text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
            assert_eq!(body.as_str(), text, "body must round-trip verbatim");
        }
    }

    #[test]
    fn body_rejects_empty_nul_and_too_long() {
        assert_eq!(MessageBody::parse(""), Err(MessageBodyError::Empty));
        assert_eq!(
            MessageBody::parse("a\u{0}b"),
            Err(MessageBodyError::ContainsNul)
        );
        assert_eq!(
            MessageBody::parse(&"a".repeat(16385)),
            Err(MessageBodyError::TooLong(16385))
        );
    }

    #[test]
    fn body_byte_boundary_is_utf8_bytes_not_scalars() {
        // 16384 个中文字符共 49152 UTF-8 bytes：必须按字节拒绝。
        let too_long = "中".repeat(16384);
        assert_eq!(
            MessageBody::parse(&too_long),
            Err(MessageBodyError::TooLong(49152))
        );
        // 正好 16384 个 ASCII 字节：合法。
        assert!(MessageBody::parse(&"a".repeat(16384)).is_ok());
    }

    #[test]
    fn message_public_fields_are_stable_and_displayable() {
        let id = MessageId::new_v4();
        let room = RoomId::new_v4();
        let actor = ParticipantId("actor-id".into());
        let created_at = UtcMillis::from_millis(1);
        let message = Message::new(
            id.clone(),
            room.clone(),
            actor.clone(),
            MessageBody::parse("hello").unwrap(),
            created_at,
        );
        assert_eq!(message.id, id);
        assert_eq!(message.room_id, room);
        assert_eq!(message.actor_id, actor);
        assert_eq!(message.sequence, 0);
        assert_eq!(message.body.as_ref().unwrap().as_str(), "hello");
        assert_eq!(message.attachments, Vec::new());
        assert_eq!(message.created_at, created_at);
        assert_eq!(id.to_string(), id.as_str());
    }
}
