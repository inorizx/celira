//! Durable Agent Event：Core 公共事实的第六张领域切片。
//!
//! 契约（CORE_V1_AGENT_EVENTS.md 4.1 + CORE_V1_MESSAGE_RECALL.md §10）：
//! Agent Event 是 Core 基于一条已提交 Message 为一个目标 Agent 建立的
//! 可恢复处理记录，表达「Core 已裁决该 Message 应交给该 Agent 处理」。
//! 它不是 Message 副本、不是 Backend/Binding/Credential/连接状态，也不是
//! 任何处理完成证明。
//!
//! 一条 Event 的最小耐久身份：Core 生成的稳定 Event ID（UUID v4）、kind
//! （message.created / message.recalled）、target Agent ID、该 target Agent
//! 自己范围内从 1 开始单调唯一的正整数 sequence（跨 kind 共享，不因 kind
//! 分列）、source Message ID、Core 创建时间。`kind` 是协议判别值；
//! message.recalled 是撤回契约（CORE_V1_MESSAGE_RECALL.md）新增的更正
//! 通知——不是生成请求，本身不接受 Completion。

use crate::domain::message::MessageId;
use crate::domain::participant::ParticipantId;
use crate::domain::time::UtcMillis;

/// Agent Event 的协议判别值（与持久层/协议共用同一组字符串）。
pub mod kind {
    pub const MESSAGE_CREATED: &str = "message.created";
    pub const MESSAGE_RECALLED: &str = "message.recalled";
}

/// 已持久化的 Agent Event kind（数据库 CHECK 限定两值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentEventKind {
    Created,
    Recalled,
}

impl AgentEventKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            kind::MESSAGE_CREATED => Some(Self::Created),
            kind::MESSAGE_RECALLED => Some(Self::Recalled),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Created => kind::MESSAGE_CREATED,
            Self::Recalled => kind::MESSAGE_RECALLED,
        }
    }
}

/// 稳定、不可复用的 Agent Event ID（UUID v4 字符串，统一小写连字符）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentEventId(pub String);

impl AgentEventId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AgentEventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 一条已持久化的 Agent Event 公共事实。
///
/// 事件表只持有 kind、目标、source Message 引用、sequence 与时间，不复制
/// Room、actor、body 或 Message sequence；公开观察响应由不可变 Message
/// 事实和 actor 当前公共资料投影组成（HTTP adapter 职责）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEvent {
    pub id: AgentEventId,
    /// 协议判别值：message.created（处理请求）或 message.recalled（更正
    /// 通知，不接受 Completion）。
    pub kind: AgentEventKind,
    /// 目标 Agent（Participant）；Event 目标与存在不依赖 Binding/Credential。
    pub target_agent_id: ParticipantId,
    /// 该 target Agent 自己范围内从 1 开始、单调唯一的正整数 sequence
    /// （跨 kind 共享，不因 kind 分列）。
    pub sequence: i64,
    /// source Message ID：同一 source Message 对同一目标 Agent 每个 kind
    /// 至多一条。
    pub message_id: MessageId,
    /// message.created 时等于 source Message 的 created_at；message.recalled
    /// 时等于 recalled_at（SQLite statement-time）。
    pub created_at: UtcMillis,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_identity_fields_round_trip() {
        let id = AgentEventId("00000000-0000-4000-8000-000000000001".into());
        let event = AgentEvent {
            id: id.clone(),
            kind: AgentEventKind::Created,
            target_agent_id: ParticipantId("agent-1".into()),
            sequence: 7,
            message_id: MessageId("00000000-0000-4000-8000-000000000002".into()),
            created_at: UtcMillis::from_millis(42),
        };
        assert_eq!(event.id, id);
        assert_eq!(event.sequence, 7);
        assert_eq!(event.created_at, UtcMillis::from_millis(42));
        assert_eq!(id.to_string(), id.as_str());
        assert_eq!(event.target_agent_id.as_str(), "agent-1");
        assert_eq!(event.kind, AgentEventKind::Created);
        assert_eq!(
            AgentEventKind::parse("message.created"),
            Some(AgentEventKind::Created)
        );
        assert_eq!(
            AgentEventKind::parse("message.recalled"),
            Some(AgentEventKind::Recalled)
        );
        assert_eq!(AgentEventKind::parse("other"), None);
        assert_eq!(kind::MESSAGE_RECALLED, "message.recalled");
    }
}
