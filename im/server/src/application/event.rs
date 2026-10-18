//! Agent Event 用例：当前获授权 Backend 的有界无副作用观察。
//!
//! 用例层只编排读取：目标 Agent 恒来自 Backend 认证上下文（Credential →
//! Binding → Agent 已由 `application::backend::authenticate_backend` 解析，
//! 请求不能携带或覆盖 agent_id），把已解析的 `after`/`limit` 交给
//! `ports::event::EventStore` 拉取，再按契约计算 `next_after`（opaque
//! cursor 字符串；`after` 缺省表示起点，cursor 按当前 Agent 作用域解析回
//! 内部 sequence）。
//!
//! Event 生成不经过本模块：Message → target Agent Event 的原子生成属于
//! SQLite Message 事务/触发器事实链，HTTP handler 与用例不得双写 Message
//! 与 Event。

use crate::domain::agent_event::AgentEventId;
use crate::domain::participant::ParticipantId;
use crate::ports::event::{EventStore, EventStoreError, EventWithMessage};
use crate::ports::storage::StorageError;
use crate::protocol::cursor::{Cursor, CursorPayload, CursorResource};

/// 有界观察结果：events 严格升序；next_after 为 opaque cursor 字符串
/// （非空页为最后一条 Event 的 cursor、空页为实际采用的 after cursor、
/// 缺省为起点 sentinel）。
#[derive(Debug)]
pub struct ListAgentEventsOutcome {
    pub events: Vec<EventWithMessage>,
    pub next_after: String,
}

#[derive(Debug)]
pub enum EventQueryError {
    /// after cursor / limit 非法（防御性复检，HTTP 层已做严格语法解析）；
    /// cursor 跨端点类型、未知或不属于当前认证 Agent 统一归此（400
    /// invalid_request，不泄露 Event 存在性或归属）。
    InvalidRequest,
    /// 存储不可用；不泄露 SQL、路径或其他 Agent 事实。
    Store(StorageError),
}

impl std::fmt::Display for EventQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest => write!(f, "invalid event paging parameters"),
            Self::Store(e) => write!(f, "event store failure: {e}"),
        }
    }
}

impl std::error::Error for EventQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// 解析 Backend Event `after` cursor：None（省略）为起点；否则必须是
/// `be` 资源类型（`start` 哨兵或规范 UUID v4）。返回（内部起始位置,
/// 请求游标）；未知/跨 Agent 的 UUID 由存储层按当前 Agent 作用域裁决。
/// ⚠ 2026-09-28：原还接受 `ack` 哨兵（从服务端投递位置之后读取）——
/// 随 Completion/ACK 整体退役删除。
async fn parse_after_cursor(
    store: &dyn EventStore,
    target_agent_id: &ParticipantId,
    after_raw: Option<&str>,
) -> Result<(i64, Cursor), EventQueryError> {
    match after_raw {
        None => Ok((0, Cursor::backend_event_start())),
        Some(raw) => {
            let cursor = Cursor::parse(raw).ok_or(EventQueryError::InvalidRequest)?;
            if cursor.resource != CursorResource::BackendEvent {
                return Err(EventQueryError::InvalidRequest);
            }
            match cursor.payload {
                CursorPayload::Start => Ok((0, cursor)),
                CursorPayload::Id(ref id) => {
                    let resolved = store
                        .resolve_event_after(target_agent_id, &AgentEventId(id.clone()))
                        .await
                        .map_err(|e| match e {
                            EventStoreError::Database(e) => EventQueryError::Store(e),
                        })?;
                    resolved
                        .map(|sequence| (sequence, cursor))
                        .ok_or(EventQueryError::InvalidRequest)
                }
            }
        }
    }
}

/// 当前认证 Agent 的稳定有界观察：`sequence > after`、严格升序、最多
/// `limit` 条；纯读取，不推进任何服务端位置（无服务端位置可推进）。
pub async fn list_agent_events(
    store: &dyn EventStore,
    target_agent_id: &ParticipantId,
    after_raw: Option<&str>,
    limit: u32,
) -> Result<ListAgentEventsOutcome, EventQueryError> {
    if !(1..=100).contains(&limit) {
        return Err(EventQueryError::InvalidRequest);
    }
    let (after, requested) = parse_after_cursor(store, target_agent_id, after_raw).await?;
    let page = store
        .list_events(target_agent_id, after, limit)
        .await
        .map_err(|e| match e {
            EventStoreError::Database(e) => EventQueryError::Store(e),
        })?;
    let next_after = page
        .events
        .last()
        .map(|item| Cursor::backend_event(&item.event.id).to_string())
        .unwrap_or_else(|| requested.to_string());
    Ok(ListAgentEventsOutcome {
        events: page.events,
        next_after,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::participant::ParticipantId;
    use crate::ports::event::{EventPage, EventStore, EventStoreError};

    struct FixedStore {
        events: Vec<EventWithMessage>,
        resolved: Vec<(String, String)>,
    }

    #[async_trait::async_trait]
    impl EventStore for FixedStore {
        async fn list_events(
            &self,
            target_agent_id: &ParticipantId,
            after: i64,
            limit: u32,
        ) -> Result<EventPage, EventStoreError> {
            assert!(after >= 0);
            assert!((1..=100).contains(&limit));
            let _ = target_agent_id;
            Ok(EventPage {
                events: self
                    .events
                    .iter()
                    .filter(|item| item.event.sequence > after)
                    .take(limit as usize)
                    .cloned()
                    .collect(),
            })
        }

        async fn resolve_event_after(
            &self,
            _target_agent_id: &ParticipantId,
            cursor_event_id: &AgentEventId,
        ) -> Result<Option<i64>, EventStoreError> {
            Ok(self
                .resolved
                .iter()
                .find(|(id, _)| id == cursor_event_id.as_str())
                .map(|(_, sequence)| sequence.parse::<i64>().expect("fixture sequence")))
        }

        async fn event_targets(
            &self,
            _kind: &crate::domain::agent_event::AgentEventKind,
            _message_id: &crate::domain::message::MessageId,
        ) -> Result<Vec<ParticipantId>, EventStoreError> {
            // 唤醒目标反查不属于本 fixture 的既有用例；空集即「无唤醒」。
            Ok(Vec::new())
        }
    }

    fn sample_event(sequence: i64) -> EventWithMessage {
        use crate::domain::agent_event::{AgentEvent, AgentEventId};
        use crate::domain::message::{Message, MessageBody, MessageId};
        use crate::domain::participant::{
            DisplayName, ParticipantId, ParticipantKind, ParticipantStatus,
        };
        use crate::domain::room::RoomId;
        use crate::domain::time::UtcMillis;
        use crate::ports::room::ParticipantProfile;

        EventWithMessage {
            event: AgentEvent {
                id: AgentEventId(format!("00000000-0000-4000-8000-{sequence:012}")),
                kind: crate::domain::agent_event::AgentEventKind::Created,
                target_agent_id: ParticipantId("agent-1".into()),
                sequence,
                message_id: MessageId("00000000-0000-4000-8000-000000000009".into()),
                created_at: UtcMillis::from_millis(1),
            },
            message: Message::new(
                MessageId("00000000-0000-4000-8000-000000000009".into()),
                RoomId::new_v4(),
                ParticipantId("human-1".into()),
                MessageBody::parse("hello").unwrap(),
                UtcMillis::from_millis(1),
            )
            .with_sequence(1),
            actor: ParticipantProfile {
                id: ParticipantId("human-1".into()),
                kind: ParticipantKind::Human,
                display_name: DisplayName::parse("Owner").unwrap(),
                status: ParticipantStatus::Active,
            },
        }
    }

    #[tokio::test]
    async fn next_after_is_last_event_cursor_when_non_empty_and_requested_cursor_when_empty() {
        let events = vec![sample_event(1), sample_event(2), sample_event(7)];
        let store = FixedStore {
            events,
            resolved: Vec::new(),
        };
        let outcome = list_agent_events(&store, &ParticipantId("agent-1".into()), None, 50)
            .await
            .unwrap();
        assert_eq!(
            outcome.next_after,
            "c1.be.00000000-0000-4000-8000-000000000007"
        );

        let empty = FixedStore {
            events: Vec::new(),
            resolved: Vec::new(),
        };
        // 缺省 after：空页返回起点 sentinel。
        let outcome = list_agent_events(&empty, &ParticipantId("agent-1".into()), None, 50)
            .await
            .unwrap();
        assert_eq!(
            outcome.next_after, "c1.be.start",
            "empty page keeps the start cursor"
        );
        // 显式 cursor：空页保持实际请求 cursor（回传格式不变）。
        let cursor = "c1.be.00000000-0000-4000-8000-000000000042";
        let store = FixedStore {
            events: Vec::new(),
            resolved: vec![(
                "00000000-0000-4000-8000-000000000042".to_owned(),
                "42".to_owned(),
            )],
        };
        let outcome = list_agent_events(&store, &ParticipantId("agent-1".into()), Some(cursor), 50)
            .await
            .unwrap();
        assert_eq!(outcome.next_after, cursor);
        assert!(outcome.events.is_empty());
    }

    #[tokio::test]
    async fn out_of_range_params_and_unknown_or_wrong_type_cursors_are_rejected() {
        let store = FixedStore {
            events: Vec::new(),
            resolved: Vec::new(),
        };
        for limit in [0u32, 101u32] {
            assert!(matches!(
                list_agent_events(&store, &ParticipantId("agent-1".into()), None, limit).await,
                Err(EventQueryError::InvalidRequest)
            ));
        }
        for bad in [
            "c1.m.550e8400-e29b-41d4-a716-446655440000",
            "c1.he.550e8400-e29b-41d4-a716-446655440000",
            "c1.he.ack",
            "c1.be.not-a-uuid",
            "c1.be.550e8400-e29b-41d4-a716-446655440000",
            "c2.be.550e8400-e29b-41d4-a716-446655440000",
            "c1.be.ackx",
            "",
        ] {
            let outcome =
                list_agent_events(&store, &ParticipantId("agent-1".into()), Some(bad), 50).await;
            assert!(
                matches!(outcome, Err(EventQueryError::InvalidRequest)),
                "must reject {bad:?}"
            );
        }
    }
}
