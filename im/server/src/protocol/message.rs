//! Message Resource API 请求与响应模型（契约 CORE_V1_MESSAGES.md 8.x +
//! CORE_V1_HUMAN_ROOMS_AND_MENTIONS.md 4.4/5.5 +
//! CORE_V1_REPLIES_AND_QUOTES.md 6.x + CORE_V1_ATTACHMENTS.md 5.4）。
//!
//! 模型只包含公开 Message 事实与 actor 当前公共资料投影；idempotency
//! key 永不进入任何响应模型或 History。请求体在保留 `body` 的基础上允许
//! 可选 `mentions`（缺省等价于空数组）、可选 `reply_to_message_id`
//! （缺省/null = 普通消息）与可选 `attachments`（有序绑定附件 ID 集合，
//! 缺省为空数组，至多 8 个；body 与 attachments 至少一个——附件-only
//! Message 的 body 为 null）；未知字段一律拒绝。Message 公共视图与
//! History 返回规范排序后的 mentions Participant ID 列表、有序附件元数据
//! 数组（无附件为空数组），并统一返回 `reply_to_message_id`（无回复目标
//! 时为 null，字段恒存在）；不返回 target 正文、target actor、sequence
//! 或 target Room。
//!
//! 公共协议不暴露任何 sequence（`before`/`next_before` 是
//! `protocol::cursor` 定义的 opaque cursor，`MessageView` 不返回
//! sequence）：客户端只依赖稳定资源 ID 与不透明游标，SQLite/单机内部
//! 排序实现不被冻结为永久公共协议。

use serde::{Deserialize, Serialize};

/// POST /v1/rooms/{room_id}/messages 请求体。未知字段一律拒绝；
/// `mentions` 缺省等价于空数组；`reply_to_message_id` 缺省或显式 null
/// 表示普通消息；`body` 缺省或显式 null 且 `attachments` 为空时拒绝
/// （Message 必须有正文或至少一个附件）；`attachments` 是有序附件 ID
/// 集合（顺序参与幂等重放裁决）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendMessageRequest {
    pub body: Option<String>,
    #[serde(default)]
    pub mentions: Vec<String>,
    #[serde(default)]
    pub reply_to_message_id: Option<String>,
    #[serde(default)]
    pub attachments: Vec<String>,
}

/// Message 公共视图：actor 来自 Core Participant 公共资料投影，客户端
/// 不能通过请求体覆盖 actor；mentions 是规范排序后的 Mention 目标
/// Participant ID 列表（无 Mention 为空数组）；reply_to_message_id 是
/// 回复/引用目标（普通消息为 null，字段恒存在），不嵌入 target 正文、
/// actor、sequence 或 Room。attachments 是有序绑定附件元数据数组（无
/// 附件为空数组，契约 CORE_V1_ATTACHMENTS.md 5.4），元素只含
/// id/file_name/content_type/size_bytes。视图不返回任何 sequence。
///
/// MessageRecall 扩展（契约 CORE_V1_MESSAGE_RECALL.md §7.1）：字段集恒
/// 存在——active 时 `body` 为原值 string（附件-only Message 为 null）、
/// `mentions` 为原集合、`attachments` 为绑定元数据、`recalled_at` 为 null；
/// recalled 时 `body` 为 null、`mentions` 为空数组、`attachments` 为空数组、
/// `recalled_at` 为首次撤回时点的 RFC3339。id/room_id/actor/
/// reply_to_message_id/created_at 两种状态都保留原值。原正文绝不返回任何
/// 公共恢复面。
#[derive(Debug, Clone, Serialize)]
pub struct MessageView {
    pub id: String,
    pub room_id: String,
    pub actor: MessageActorView,
    /// active 时为原值（附件-only Message 为 null）；recalled 时为 null
    /// （字段恒存在）。
    pub body: Option<String>,
    /// active 时为原 Mention 集合；recalled 时为空数组（字段恒存在）。
    pub mentions: Vec<String>,
    /// active 时为绑定附件元数据（position 升序）；recalled 时为空数组
    /// （字段恒存在；契约 CORE_V1_ATTACHMENTS.md 9.3）。
    pub attachments: Vec<crate::protocol::attachment::AttachmentMetadataView>,
    pub reply_to_message_id: Option<String>,
    pub created_at: String,
    /// 未撤回为 null；已撤回为首次撤回时点（字段恒存在）。
    pub recalled_at: Option<String>,
}

/// Message 的 actor 当前公共资料投影（读取时的当前投影，不是消息提交时
/// 的资料快照）。
#[derive(Debug, Clone, Serialize)]
pub struct MessageActorView {
    pub id: String,
    pub kind: String,
    pub display_name: String,
    pub status: String,
}

/// POST /v1/rooms/{room_id}/messages 响应：201 首次创建 / 200 幂等重放，
/// 以 `idempotency_replayed` 区分。
#[derive(Debug, Clone, Serialize)]
pub struct SendMessageResponse {
    pub message: MessageView,
    pub idempotency_replayed: bool,
    pub request_id: String,
}

/// GET /v1/rooms/{room_id}/messages 分页查询参数。limit 默认 50、范围
/// 1-100；before 为可选 opaque cursor（引用一条该 Room 内可见 Message）；
/// 未知、重复、非法、越界、跨端点类型、跨 Room 或不可见 cursor 在应用层
/// 统一 400 invalid_request。
#[derive(Debug, Clone, Deserialize)]
pub struct HistoryQuery {
    pub limit: Option<u32>,
    pub before: Option<String>,
}

/// GET /v1/rooms/{room_id}/messages 响应：messages 按 sequence 升序；
/// next_before 为 opaque cursor 字符串（有更旧消息时为当前页第一条
/// Message 的 cursor）或 null（没有更旧消息）。
#[derive(Debug, Clone, Serialize)]
pub struct ListMessagesResponse {
    pub messages: Vec<MessageView>,
    pub next_before: Option<String>,
    pub request_id: String,
}

/// GET /v1/backend/rooms/{room_id}/messages 响应：在人类历史响应形状之上
/// **附加**一个顶层 `room` 头（CORE_V1_ROOM_PROFILE.md §4.2）。`messages` /
/// `next_before` / `request_id` 与人类历史逐字段同形，分页语义一字未动。
#[derive(Debug, Clone, Serialize)]
pub struct BackendListMessagesResponse {
    pub messages: Vec<MessageView>,
    pub next_before: Option<String>,
    pub room: crate::protocol::room_profile::RoomHeaderView,
    pub request_id: String,
}

/// DELETE /v1/rooms/{room_id}/messages/{message_id} 与
/// DELETE /v1/backend/rooms/{room_id}/messages/{message_id} 成功响应
/// （契约 CORE_V1_MESSAGE_RECALL.md §12.1）：首次撤回 201、幂等重放 200，
/// 以 `idempotency_replayed` 区分；`recalled_at` 来自同一 message_recalls
/// INSERT 的 `RETURNING recalled_at`（SQLite 毫秒 statement-time，客户端
/// 不能提交或覆盖）。
#[derive(Debug, Clone, Serialize)]
pub struct RecallMessageResponse {
    pub message_id: String,
    pub room_id: String,
    pub recalled_at: String,
    pub idempotency_replayed: bool,
    pub request_id: String,
}
