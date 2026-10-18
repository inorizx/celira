//! Agent Event 观察协议模型（契约 CORE_V1_AGENT_EVENTS.md 8.x +
//! CORE_V1_MESSAGE_RECALL.md §10）。
//!
//! 响应复用既有 `MessageView` 形状作为 source Message 公共视图，不建立
//! 第二套不一致的 Message DTO；Event 响应不包含 target Agent ID（由
//! BackendAuth 上下文唯一确定）、Binding/Credential/Owner、幂等键或任何
//! 私有事实。`kind` 有两种判别值：`message.created`（处理请求）与
//! `message.recalled`（更正通知，不接受 Completion）。
//!
//! 公共协议不暴露任何 sequence：`after`/`next_after` 是 `protocol::cursor`
//! 定义的 opaque cursor（缺省 `after` 表示起点），`BackendEventView` 与其
//! source `MessageView` 都不返回 sequence。

use serde::Serialize;

use crate::protocol::message::MessageView;

/// Event 判别值：`message.created`（处理请求）。
pub const EVENT_KIND_MESSAGE_CREATED: &str = "message.created";

/// Event 判别值：`message.recalled`（更正通知，不接受 Completion）。
pub const EVENT_KIND_MESSAGE_RECALLED: &str = "message.recalled";

/// 一条可观察的 Agent Event：Event 事实 + source Message 公共视图 +
/// actor 读取时当前公共资料投影。不返回任何 sequence。
#[derive(Debug, Clone, Serialize)]
pub struct BackendEventView {
    pub id: String,
    /// message.created / message.recalled（source Message 已撤回时两种
    /// kind 的 message 视图都是 tombstone）。
    pub kind: String,
    pub message: MessageView,
    pub created_at: String,
}

/// GET /v1/backend/events 成功响应：events 严格升序；next_after 为 opaque
/// cursor 字符串（非空页为最后一条 Event 的 cursor、空页为实际采用的
/// after cursor、缺省为起点 sentinel）；next_after 只属于调用方本地分页
/// 状态，Core 不持久化，也不表示 Completion 或 ACK。
#[derive(Debug, Clone, Serialize)]
pub struct ListBackendEventsResponse {
    pub events: Vec<BackendEventView>,
    pub next_after: String,
    pub request_id: String,
}
