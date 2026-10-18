//! Durable Agent Event 窄端口。
//!
//! 本端口只声明 Event 切片的持久化边界，不依赖具体数据库或 HTTP 框架。
//! SQLite adapter 实现本端口；应用用例只通过端口消费存储能力。
//!
//! Event 生成不经过本端口：Message → target Agent Event 的原子生成属于
//! SQLite Message 事务/触发器事实链（migration 007 的 Message AFTER
//! INSERT 触发器），HTTP/application 不并行维护第二套 Event 写入流程。
//! 本端口只提供「当前 Agent 自己的 sequence > after 的稳定有界观察」。

use std::sync::Arc;

use crate::domain::agent_event::{AgentEvent, AgentEventId};
use crate::domain::message::{Message, MessageId};
use crate::domain::participant::ParticipantId;
use crate::ports::room::ParticipantProfile;
use crate::ports::storage::StorageError;

/// Backend Event 唤醒流的 volatile notifier 端口（窄解冻切片，
/// CORE_V1_AGENT_PROTOCOL.md §8）：语义与 Human Event notifier 同构
/// （按 target Agent 订阅、每连接唤醒通道有界容量 1、弱引用、重复提示
/// 合并），但实例独立——Agent Event 变化只唤醒 Backend SSE 门铃
/// （`events.available`），绝不进入 Human Event 表或 Human WS 语义。
/// notifier 只负责及时唤醒，不负责恢复正确性：耐久 REST Event 才是
/// 恢复来源，唤醒丢失/慢消费/重启均不影响耐久事实。
pub trait BackendEventNotifier: Send + Sync {
    /// 订阅该 Agent 的唤醒通道；notifier 只持有弱引用，连接的强引用由
    /// 订阅方保持——连接断开时订阅被惰性剪除。
    fn subscribe(&self, agent_id: &ParticipantId, wake: Arc<tokio::sync::mpsc::Sender<()>>);
    /// 定向唤醒（post-commit 由 HTTP 成功路径从已提交 agent_events 事实
    /// 反查 target Agent 后调用；不在本端口复制 targeting 规则）。
    fn notify(&self, agent_ids: &[ParticipantId]);
}

/// 一条可观察的 Event：Event 事实 + source Message 公共事实 + actor 读取
/// 时当前公共资料投影（不是消息提交时的资料快照）。
#[derive(Debug, Clone)]
pub struct EventWithMessage {
    pub event: AgentEvent,
    pub message: Message,
    pub actor: ParticipantProfile,
}
/// 有界事件页：events 严格按 Event sequence 升序。
#[derive(Debug, Clone)]
pub struct EventPage {
    pub events: Vec<EventWithMessage>,
}

/// 存储层错误。
#[derive(Debug)]
pub enum EventStoreError {
    /// 数据库或其他持久化失败（typed cause + 稳定分类）；协议错误与存储
    /// 错误一律不泄露 SQL、路径、token、hash 或其他 Agent 的事实。
    Database(StorageError),
}

impl std::fmt::Display for EventStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(e) => write!(f, "event store failure: {e}"),
        }
    }
}

impl std::error::Error for EventStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(e) => Some(e),
        }
    }
}

/// Agent Event 存储端口（窄接口，不扩成万能 repository）。
#[async_trait::async_trait]
pub trait EventStore: Send + Sync {
    /// 返回当前 target Agent 下 `sequence > after` 的 Event，严格按
    /// sequence 升序，最多 `limit` 条；不使用 OFFSET。纯读取，无副作用：
    /// 不建立/改写 Event、attempt、Completion、cursor、Lease 或 Binding。
    async fn list_events(
        &self,
        target_agent_id: &ParticipantId,
        after: i64,
        limit: u32,
    ) -> Result<EventPage, EventStoreError>;

    /// 把 Backend Event 公共 cursor 解析回内部位置：按 target Agent 的
    /// 作用域裁决（cursor 引用的 Event 必须属于该 Agent）。未知、跨 Agent
    /// 的 cursor 一律返回 None（对外统一 400 invalid_request，不泄露
    /// Event 是否存在或属于哪个 Agent）。cursor 编码/解码只存在于
    /// protocol/application 边界，本端口只接收稳定 Event ID 并返回内部
    /// i64 sequence。
    async fn resolve_event_after(
        &self,
        target_agent_id: &ParticipantId,
        cursor_event_id: &AgentEventId,
    ) -> Result<Option<i64>, EventStoreError>;

    /// post-commit 唤醒目标反查（窄解冻切片 §8）：按已提交 agent_events
    /// 事实反查 `(kind, source message)` 的全部 target Agent，供 HTTP
    /// 成功路径定向唤醒 Backend SSE 门铃。纯读取、不复制 targeting 规则
    /// （Event 由 SQLite 触发器写入，本端口只反查已提交事实）。
    async fn event_targets(
        &self,
        kind: &crate::domain::agent_event::AgentEventKind,
        message_id: &MessageId,
    ) -> Result<Vec<ParticipantId>, EventStoreError>;
}
