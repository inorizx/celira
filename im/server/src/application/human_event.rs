//! Durable Human Event 用例：当前 Human 的有界无副作用恢复读取。
//!
//! 用例层只编排读取：Human 恒来自 Human Session 认证上下文（HTTP
//! CurrentAuth / WS 首帧认证），请求不能携带或覆盖 human_id；把已解析的
//! `after`/`limit` 交给 `ports::human_event::HumanEventStore` 拉取，再按
//! 契约计算 `next_after`（opaque cursor 字符串；`after` 缺省表示起点，
//! cursor 按当前 Human 作用域解析回内部 sequence）。
//!
//! Human Event 生成不经过本模块：source 事实 → 共享 fact → per-Human
//! delivery 的原子成立属于 SQLite 触发器事实链（migration 009），HTTP
//! handler 与用例不得双写。

use crate::domain::human_event::HumanEventFactId;
use crate::domain::participant::ParticipantId;
use crate::ports::human_event::{HumanEventStore, HumanEventStoreError, HumanEventWithFact};
use crate::ports::storage::StorageError;
use crate::protocol::cursor::{Cursor, CursorPayload, CursorResource};

/// 有界恢复结果：events 严格按该 Human 的 sequence 升序；next_after 为
/// opaque cursor 字符串（非空页为最后一条 Event 的 cursor、空页为实际
/// 采用的 after cursor、缺省为起点 sentinel；保持请求位置，不可见事件只
/// 造成空洞，不推动空页前进）。
#[derive(Debug)]
pub struct ListHumanEventsOutcome {
    pub events: Vec<HumanEventWithFact>,
    pub next_after: String,
}

#[derive(Debug)]
pub enum HumanEventQueryError {
    /// after cursor / limit 非法（防御性复检，HTTP 层已做严格语法解析）；
    /// cursor 跨端点类型、未知或从未投递给当前 Human 的 fact 统一归此
    /// （400 invalid_request，不泄露 fact 存在性或归属）。
    InvalidRequest,
    /// 存储不可用；不泄露 SQL、路径或其他 Human 事实。
    Store(StorageError),
}

impl std::fmt::Display for HumanEventQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest => write!(f, "invalid human event paging parameters"),
            Self::Store(e) => write!(f, "human event store failure: {e}"),
        }
    }
}

impl std::error::Error for HumanEventQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// 解析 Human Event `after` cursor：None（省略）为起点；否则必须是 `he`
/// 资源类型（`start` 哨兵或规范 UUID v4）。返回（内部起始位置, 请求
/// 游标）；未知/跨 Human 的 fact 由存储层按当前 Human 作用域裁决。
async fn parse_after_cursor(
    store: &dyn HumanEventStore,
    human_id: &ParticipantId,
    after_raw: Option<&str>,
) -> Result<(i64, Cursor), HumanEventQueryError> {
    match after_raw {
        None => Ok((0, Cursor::human_event_start())),
        Some(raw) => {
            let cursor = Cursor::parse(raw).ok_or(HumanEventQueryError::InvalidRequest)?;
            if cursor.resource != CursorResource::HumanEvent {
                return Err(HumanEventQueryError::InvalidRequest);
            }
            match cursor.payload {
                CursorPayload::Start => Ok((0, cursor)),
                CursorPayload::Id(ref id) => {
                    let resolved = store
                        .resolve_event_after(human_id, &HumanEventFactId(id.clone()))
                        .await
                        .map_err(|e| match e {
                            HumanEventStoreError::Database(e) => HumanEventQueryError::Store(e),
                        })?;
                    resolved
                        .map(|sequence| (sequence, cursor))
                        .ok_or(HumanEventQueryError::InvalidRequest)
                }
            }
        }
    }
}

/// 当前认证 Human 的稳定有界恢复：`sequence > after`、严格升序、最多
/// `limit` 条；纯读取，不推进任何服务端位置，不建立 cursor/ACK。
pub async fn list_human_events(
    store: &dyn HumanEventStore,
    human_id: &ParticipantId,
    after_raw: Option<&str>,
    limit: u32,
) -> Result<ListHumanEventsOutcome, HumanEventQueryError> {
    if !(1..=100).contains(&limit) {
        return Err(HumanEventQueryError::InvalidRequest);
    }
    let (after, requested) = parse_after_cursor(store, human_id, after_raw).await?;
    let events = store
        .list_events(human_id, after, limit)
        .await
        .map_err(|e| match e {
            HumanEventStoreError::Database(e) => HumanEventQueryError::Store(e),
        })?;
    let next_after = events
        .last()
        .map(|item| Cursor::human_event(&item.fact.id).to_string())
        .unwrap_or_else(|| requested.to_string());
    Ok(ListHumanEventsOutcome { events, next_after })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::human_event::{HumanEventFact, HumanEventFactId, HumanEventKind};
    use crate::domain::participant::ParticipantId;
    use crate::domain::room::RoomId;
    use crate::domain::time::UtcMillis;

    struct FixedStore {
        events: Vec<HumanEventWithFact>,
        resolved: Vec<(String, i64)>,
    }

    #[async_trait::async_trait]
    impl HumanEventStore for FixedStore {
        async fn list_events(
            &self,
            _human_id: &ParticipantId,
            after: i64,
            limit: u32,
        ) -> Result<Vec<HumanEventWithFact>, HumanEventStoreError> {
            assert!(after >= 0);
            assert!((1..=100).contains(&limit));
            Ok(self
                .events
                .iter()
                .filter(|item| item.sequence > after)
                .take(limit as usize)
                .cloned()
                .collect())
        }

        async fn resolve_event_after(
            &self,
            _human_id: &ParticipantId,
            cursor_fact_id: &HumanEventFactId,
        ) -> Result<Option<i64>, HumanEventStoreError> {
            Ok(self
                .resolved
                .iter()
                .find(|(id, _)| id == cursor_fact_id.as_str())
                .map(|(_, sequence)| *sequence))
        }

        async fn delivery_targets(
            &self,
            _sources: &[crate::domain::human_event::HumanEventSource],
        ) -> Result<Vec<ParticipantId>, HumanEventStoreError> {
            Ok(Vec::new())
        }
    }

    fn sample_event(sequence: i64) -> HumanEventWithFact {
        HumanEventWithFact {
            fact: HumanEventFact {
                id: HumanEventFactId(format!(
                    "00000000-0000-4000-8000-{sequence:012}"
                )),
                kind: HumanEventKind::MessageCreated,
                source_id: format!("msg-{sequence}"),
                room_id: RoomId::new_v4(),
                created_at: UtcMillis::from_millis(1),
                snapshot: r#"{"id":"msg-1","room_id":"r","actor":{"id":"h","kind":"human","display_name":"H","status":"active"},"body":"hi","created_at":1}"#.into(),
            },
            sequence,
        }
    }

    #[tokio::test]
    async fn next_after_is_last_fact_cursor_when_non_empty_and_requested_cursor_when_empty() {
        let events = vec![sample_event(1), sample_event(2), sample_event(7)];
        let store = FixedStore {
            events,
            resolved: Vec::new(),
        };
        let outcome = list_human_events(&store, &ParticipantId("human-1".into()), None, 50)
            .await
            .unwrap();
        assert_eq!(
            outcome.next_after,
            "c1.he.00000000-0000-4000-8000-000000000007"
        );

        let empty = FixedStore {
            events: Vec::new(),
            resolved: Vec::new(),
        };
        // 缺省 after：空页返回起点 sentinel。
        let outcome = list_human_events(&empty, &ParticipantId("human-1".into()), None, 50)
            .await
            .unwrap();
        assert_eq!(
            outcome.next_after, "c1.he.start",
            "empty page keeps the start cursor"
        );
        // 显式 cursor：空页保持实际请求 cursor（回传格式不变）。
        let cursor = "c1.he.00000000-0000-4000-8000-000000000042";
        let store = FixedStore {
            events: Vec::new(),
            resolved: vec![("00000000-0000-4000-8000-000000000042".to_owned(), 42)],
        };
        let outcome = list_human_events(&store, &ParticipantId("human-1".into()), Some(cursor), 50)
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
                list_human_events(&store, &ParticipantId("human-1".into()), None, limit).await,
                Err(HumanEventQueryError::InvalidRequest)
            ));
        }
        for bad in [
            "c1.m.550e8400-e29b-41d4-a716-446655440000",
            "c1.be.550e8400-e29b-41d4-a716-446655440000",
            "c1.he.not-a-uuid",
            "c1.he.550e8400-e29b-41d4-a716-446655440000",
            "c2.he.550e8400-e29b-41d4-a716-446655440000",
            "",
        ] {
            let outcome =
                list_human_events(&store, &ParticipantId("human-1".into()), Some(bad), 50).await;
            assert!(
                matches!(outcome, Err(HumanEventQueryError::InvalidRequest)),
                "must reject {bad:?}"
            );
        }
    }
}
