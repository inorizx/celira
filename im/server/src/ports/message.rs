//! Message 与 History 窄端口。
//!
//! 本端口只声明 Message 切片的持久化边界，不依赖具体数据库或 HTTP 框架。
//! SQLite adapter 实现本端口；应用用例只通过端口消费存储能力。端口签名
//! 使用通用 Participant actor（`actor_id` / `reader_id`），不命名或约束为
//! Human：Human-only 只属于当前公开 HTTP 认证入口。

use crate::domain::agent::IdempotencyKey;
use crate::domain::attachment::AttachmentId;
use crate::domain::message::{Message, MessageId};
use crate::domain::participant::ParticipantId;
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::ports::storage::StorageError;

pub use crate::ports::room::ParticipantProfile;

/// 单条 Message 的 Mention 目标上限。64 足够正常群聊明确点名多个 Agent，
/// 同时把最坏事务工作量锁定：防止 128 KiB 请求被放大为数千次逐目标校验、
/// Mention INSERT 与 Agent Event 触发，阻塞单节点全部写者。
///
/// 防线分三层，同一 `invalid_mention` 语义：
/// 1. application 在 UUID 解析 / 容量分配和进入 Store 前拒绝
///    `raw.len() > MAX_MENTIONS_PER_MESSAGE`；
/// 2. SQLite Message adapter 对绕过 application 的调用在任何逐目标 SQL
///    循环前拒绝 `mentions.len() > MAX_MENTIONS_PER_MESSAGE`；
/// 3. migration 005 的 mentions BEFORE INSERT 触发器在数据库层兜底：同一
///    message 已有 64 条时拒绝第 65 条，直接 SQL 也不能越界。
pub const MAX_MENTIONS_PER_MESSAGE: usize = 64;

/// 单条 Message 的附件绑定上限：8（契约 CORE_V1_ATTACHMENTS.md 5.4）。
/// 防线与 Mention 上限一致：application 先拒、SQLite adapter 在任何逐附件
/// SQL 循环前拒、migration 012 的 message_attachments position CHECK
/// （1..=8）与身份触发器在数据库层兜底（直接 SQL 也不能越界）。
pub const MAX_ATTACHMENTS_PER_MESSAGE: usize = 8;

/// 待持久化的新 Message。id、room、actor、body、mentions 与 created_at
/// 由应用层就绪（mentions 必须是规范 ID 排序、去重后的目标列表，最多
/// `MAX_MENTIONS_PER_MESSAGE` 个；非 group Room 必须为空数组；
/// `attachments` 是有序绑定附件 ID 列表，最多 `MAX_ATTACHMENTS_PER_MESSAGE`
/// 个，body 与 attachments 至少一个）；`reply_to_message_id` 由应用层解析
/// 为规范 UUID v4 的 Option（None 表示普通消息），目标合法性（同 Room、
/// 已提交历史 Message、对 actor 任一历史 Membership 窗口可见）由存储层在
/// 提交事务内裁决；sequence 由存储层在事务内按该 Room 裁决分配，不由
/// 调用方指定。
#[derive(Debug, Clone)]
pub struct NewMessage {
    pub id: crate::domain::message::MessageId,
    pub room_id: RoomId,
    pub actor_id: ParticipantId,
    pub body: Option<crate::domain::message::MessageBody>,
    /// 规范排序后的 Mention 目标 Participant ID 列表。
    pub mentions: Vec<ParticipantId>,
    /// 有序绑定附件 ID 列表（position 1..n 按请求顺序；契约
    /// CORE_V1_ATTACHMENTS.md 5.4，参与幂等重放裁决）。
    pub attachments: Vec<AttachmentId>,
    /// 回复/引用的目标 Message ID；None 表示普通消息（Completion 写回
    /// 恒为 None，Core 不替主体自动猜测回复对象）。
    pub reply_to_message_id: Option<crate::domain::message::MessageId>,
    pub created_at: UtcMillis,
}

/// 提交结果：首次创建与幂等重放返回相同的 Message 事实，以
/// `idempotency_replayed` 区分（HTTP 201 / 200）。
#[derive(Debug, Clone)]
pub enum SendMessageOutcome {
    Created { message: Message },
    Replayed { message: Message },
}

/// History 列表项：Message + actor 当前公共资料投影（读取时的当前投影，
/// 不是消息提交时的资料快照；Message 自身只固化 actor ID）。
#[derive(Debug, Clone)]
pub struct HistoryMessage {
    pub message: Message,
    pub actor: ParticipantProfile,
}

/// 有界历史页：messages 始终按 sequence 升序；next_before 语义严格按
/// 契约（有更旧消息时为当前页第一条 Message 的 sequence）。
#[derive(Debug, Clone)]
pub struct MessagePage {
    pub messages: Vec<HistoryMessage>,
    pub next_before: Option<i64>,
}

/// 撤回（MessageRecall，契约 CORE_V1_MESSAGE_RECALL.md）结果：首次成立与
/// 幂等重放返回同一 recalled_at（首次成立时点，重放不重算、不刷新）。
#[derive(Debug, Clone)]
pub struct RecallOutcome {
    /// 首次撤回成立时 SQLite 在同一 INSERT statement 内生成的毫秒
    /// statement-time（重复撤回时是既有事实的 recalled_at）。
    pub recalled_at: UtcMillis,
    /// true = 重复同一撤回（HTTP 200 idempotency_replayed=true）。
    pub replayed: bool,
}

/// 撤回存储层错误。
#[derive(Debug)]
pub enum RecallStoreError {
    /// bad room、Room 非 active、actor Participant 非 active 或该 Room 无
    /// active Membership：对外统一 404 room_not_found（先于消息裁决）。
    RoomNotFound,
    /// 消息 unknown、跨 Room、非当前 actor 的 Message 或对 actor 不可见：
    /// 对外统一 404 message_not_found（不泄露存在性或归属）。
    MessageNotFound,
    /// 未撤回且超过 120 秒窗口的首次撤回：409 message_recall_window_expired
    /// （同一 statement 裁决；已撤回的重复撤回不经过本错误，先读既有事实
    /// 幂等成功）。
    WindowExpired,
    /// 数据库或其他持久化失败（typed cause + 稳定分类）。
    Database(StorageError),
}

impl std::fmt::Display for RecallStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RoomNotFound => write!(f, "no such active room membership"),
            Self::MessageNotFound => write!(f, "no such message for the actor"),
            Self::WindowExpired => write!(f, "message recall window expired"),
            Self::Database(e) => write!(f, "message store failure: {e}"),
        }
    }
}

impl std::error::Error for RecallStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(e) => Some(e),
            _ => None,
        }
    }
}

/// 存储层错误。
#[derive(Debug)]
pub enum MessageStoreError {
    /// bad-id、unknown Room、Room 非 active、actor/reader Participant 非
    /// active：对外统一 404，不泄露 Room 或成员是否存在。
    NotFound,
    /// Room 与 actor Participant 均存在且 active，但该 actor 不是该校验
    /// 时点的 active member（active_marker=1 周期不存在）。Human 路径对外
    /// 仍统一 404（不泄露存在性）；Backend agent 发消息路径（CORE_V1_
    /// AGENT_SPEAK.md）对外 403 not_room_member（成员资格是已证明身份下的
    /// 明确 ACL 拒绝）。
    NotAMember,
    /// 同 actor + 同 key 已被不同 Room、不同 body、不同 Mention 集合、
    /// 不同 reply target 或不同有序附件集合使用：409。
    IdempotencyConflict,
    /// Mention 非法：非 group Room 携带非空 mentions、目标不是该 Room 的
    /// active 成员（Human 或 Agent 均可）、目标 participant 非 active：
    /// 对外统一 400 invalid_mention，且不得留下
    /// Message/Mention/Event/sequence。
    InvalidMention,
    /// reply target 非法：target 未知、跨 Room、对 source actor 任一历史
    /// Membership 窗口不可见（统一 400 invalid_reply_target，不区分具体
    /// 原因），且不得留下 Message/Event/sequence。
    InvalidReplyTarget,
    /// 附件集合超过 `MAX_ATTACHMENTS_PER_MESSAGE` 上限或包含重复附件 ID：
    /// 400 invalid_attachment（请求形状错误，先于任何事实）。
    InvalidAttachment,
    /// 附件 unknown / 非当前 actor 上传 / 跨 Room / 非 pending / 已过期：
    /// 统一 404 attachment_not_found（不泄露存在性或原因）。
    AttachmentNotFound,
    /// 数据库或其他持久化失败（typed cause + 稳定分类）。
    Database(StorageError),
}

impl std::fmt::Display for MessageStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no such active room"),
            Self::NotAMember => write!(f, "actor is not an active member of the room"),
            Self::IdempotencyConflict => write!(
                f,
                "idempotency key was already used with a different request"
            ),
            Self::InvalidMention => write!(
                f,
                "invalid mention: targets must be distinct active members of a group room"
            ),
            Self::InvalidReplyTarget => {
                write!(
                    f,
                    "reply target must be a same-room historical message visible to the actor"
                )
            }
            Self::InvalidAttachment => write!(f, "invalid attachment set"),
            Self::AttachmentNotFound => write!(f, "attachment not found"),
            Self::Database(e) => write!(f, "message store failure: {e}"),
        }
    }
}

impl std::error::Error for MessageStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(e) => Some(e),
            _ => None,
        }
    }
}

/// Message 存储端口（窄接口，不扩成万能 repository）。
#[async_trait::async_trait]
pub trait MessageStore: Send + Sync {
    /// 提交或重放一条 Message。
    ///
    /// 必须在 `BEGIN IMMEDIATE` 或等价单写事务中完成先读后写：
    /// 1. 查询 actor + key 既有幂等事实；
    /// 2. 已存在时按规范化 Room ID、完整 body string、规范 Mention 集合、
    ///    reply_to_message_id 与**有序附件集合**做精确重放或冲突裁决
    ///    （不重新检查 Room/Membership；只改变 Mention 集合、reply target
    ///    或附件集合/顺序也返回 `IdempotencyConflict`）；
    /// 3. 新 key 时才检查 active Room、Participant 与 Membership，并在同一
    ///    事务内校验 Mention 目标（非 group Room 的非空 mentions、目标
    ///    不是该 Room 的 active 成员、目标 participant 非 active 一律
    ///    `InvalidMention`）、
    ///    校验 reply target（目标必须同 Room、已提交、对 actor 任一历史
    ///    Membership 窗口可见，否则 `InvalidReplyTarget`）、校验附件
    ///    （超过 `MAX_ATTACHMENTS_PER_MESSAGE` 或重复 → `InvalidAttachment`；
    ///    每个附件必须是当前 actor 上传、同 Room、pending、未过 24 小时
    ///    绑定窗口，否则 `AttachmentNotFound`）、按该 Room 当前最大
    ///    sequence 分配下一个正整数；
    /// 4. 先插入附件绑定行（position 1..n）、再插入 Message（body 与
    ///    attachments 至少一个，由数据库守卫兜底）、最后插入 Mention 记录
    ///    （同一事务，任何失败都整体回滚）后提交；附件 state 在同一事务
    ///    内经 messages AFTER INSERT 触发器置为 bound。
    ///
    /// 并发由 SQLite 单写者与 (room_id, sequence) 唯一约束共同裁决，
    /// 不依赖进程内 mutex。
    async fn send_message(
        &self,
        message: &NewMessage,
        idempotency_key: &IdempotencyKey,
    ) -> Result<SendMessageOutcome, MessageStoreError>;

    /// 读取有界历史页：keyset pagination，不使用 OFFSET。
    ///
    /// - `limit` 1-100；每页先取 `limit + 1` 判断是否还有更旧消息；
    /// - `before` 为 None 时返回最新一页，否则只读 `sequence < before`；
    /// - 返回 messages 按 sequence 升序；有更旧消息时 next_before 为当前
    ///   页第一条 Message 的 sequence，否则 None；
    /// - 读取授权始终按 reader 当前 active Membership 独立裁决；
    ///   unknown/bad-id/非 active 返回 None（对外统一 404）。
    async fn list_messages(
        &self,
        reader_id: &ParticipantId,
        room_id: &RoomId,
        limit: u32,
        before: Option<i64>,
    ) -> Result<Option<MessagePage>, MessageStoreError>;

    /// 把 Message 公共 cursor 解析回内部位置：按 reader 在指定 Room 的
    /// 作用域裁决——cursor 引用的 Message 必须同属该 Room，且被 reader
    /// 任一历史 Membership 周期覆盖（`join_after_sequence < sequence` 且
    /// 未 left 或 `sequence <= left_before_sequence`）。未知、跨 Room 或
    /// 对 reader 不可见的 cursor 一律返回 None（对外统一 400
    /// invalid_request，不泄露 Message 是否存在、所属 Room 或 Membership
    /// 边界）。cursor 编码/解码只存在于 protocol/application 边界，本端口
    /// 只接收稳定 Message ID 并返回内部 i64 sequence。
    async fn resolve_message_before(
        &self,
        reader_id: &ParticipantId,
        room_id: &RoomId,
        cursor_message_id: &MessageId,
    ) -> Result<Option<i64>, MessageStoreError>;

    /// 撤回一条 Message（契约 CORE_V1_MESSAGE_RECALL.md）。
    ///
    /// 必须在 `BEGIN IMMEDIATE` 或等价单写事务中完成先读后写：
    /// 1. 先按 actor + Room 裁决 active Room / Participant / Membership
    ///    （失败 `RoomNotFound`，先于消息裁决）；
    /// 2. 消息必须存在、属于该 Room 且 actor 等于 Message.actor（否则
    ///    `MessageNotFound`，不泄露存在性或归属）；
    /// 3. 先读既有 message_recalls 事实：已存在 → `RecallOutcome{replayed:
    ///    true}`（不重算 recalled_at，不受窗口限制）；
    /// 4. 首次撤回才尝试单一 INSERT：`recalled_at` 由同一 statement 内的
    ///    SQLite 毫秒 statement-time 生成并裁决 `created_at <= now <=
    ///    created_at + 120000`（`INSERT ... SELECT ... WHERE ... RETURNING
    ///    recalled_at`），窗口不满足返回 `WindowExpired`；该 INSERT 同事务
    ///    触发原子收敛（Human/Agent Recall Event、created 快照变形）；
    /// 5. 提交后才返回 `Created`。
    ///
    /// 并发同 Message 的撤回由 SQLite 单写者线性化：后到事务先读到既有
    /// 事实，收敛为幂等重放。取消或失败整体回滚，不留半事实。
    async fn recall_message(
        &self,
        actor_id: &ParticipantId,
        room_id: &RoomId,
        message_id: &MessageId,
    ) -> Result<RecallOutcome, RecallStoreError>;
}
