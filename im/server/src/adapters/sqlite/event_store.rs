//! SQLite Agent Event adapter：实现 `ports::event::EventStore`。
//!
//! 本模块是 Agent Event 观察唯一允许接触 SQLite 文件的地方。HTTP handler
//! 和用例不得直接操作数据库，只能通过端口调用。
//!
//! Event 生成不经过本模块：Message → target Agent Event 的原子生成由
//! migration 007 的 Message AFTER INSERT 触发器在同一 SQLite 事务内完成，
//! 本模块只提供纯读取（`sequence > after`、严格升序、最多 limit 条，
//! 不使用 OFFSET），不建立或改写任何服务端状态。
//!
//! source Message 的 actor 投影按「读取时当前 Participant 公共资料」
//! 实时 JOIN participants，与 Message History 读取共用同一行形状
//! （`message_store::HistoryRow` / `history_from_row`）。

use sqlx::sqlite::SqlitePool;

use crate::adapters::sqlite::message_kernel::{
    fetch_attachments_for_messages, fetch_recalls_for_messages,
};
use crate::adapters::sqlite::message_store::{
    HistoryRow, fetch_mentions_for_messages, history_from_row,
};
use crate::domain::agent_event::{AgentEvent, AgentEventId, AgentEventKind};
use crate::domain::message::MessageId;
use crate::domain::participant::ParticipantId;
use crate::domain::time::UtcMillis;
use crate::ports::event::{EventPage, EventStore, EventStoreError, EventWithMessage};
use crate::ports::storage::{StorageError, StorageFailureKind};

/// Event 事实行：id、kind、target_agent_id、sequence、message_id、created_at。
type EventRow = (String, String, String, i64, String, i64);

/// 基于同一 SqlitePool 的 Agent Event 存储。
#[derive(Clone)]
pub struct SqliteEventStore {
    pool: SqlitePool,
}

impl SqliteEventStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn db_error(e: sqlx::Error) -> EventStoreError {
    EventStoreError::Database(StorageError::from(e))
}

/// 持久化行/枚举解析失败：归 `decode_or_corrupt`（对外 `internal_error`）。
fn corrupt(message: impl std::fmt::Display) -> EventStoreError {
    EventStoreError::Database(StorageError::from_message(
        StorageFailureKind::DecodeOrCorrupt,
        message.to_string(),
    ))
}

fn event_from_row(
    (id, kind, target_agent_id, sequence, message_id, created_at): EventRow,
    history: crate::ports::message::HistoryMessage,
) -> Result<EventWithMessage, EventStoreError> {
    let kind = AgentEventKind::parse(&kind)
        .ok_or_else(|| corrupt(format!("unknown agent event kind {kind:?}")))?;
    let event = AgentEvent {
        id: AgentEventId(id),
        kind,
        target_agent_id: ParticipantId(target_agent_id),
        sequence,
        message_id: MessageId(message_id),
        created_at: UtcMillis::from_millis(created_at),
    };
    Ok(EventWithMessage {
        event,
        message: history.message,
        actor: history.actor,
    })
}

/// 一次性读取一组 Message ID 的 HistoryRow（与 Message History 共用同一
/// 行形状，避免第二套不一致的 Message DTO）。
async fn fetch_message_rows(
    conn: &mut sqlx::sqlite::SqliteConnection,
    message_ids: &[String],
) -> Result<Vec<(String, HistoryRow)>, EventStoreError> {
    if message_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = message_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let query = format!(
        "SELECT m.id, m.room_id, m.sequence, m.actor_id, m.body, \
                m.reply_to_message_id, m.created_at, \
                p.id, p.kind, p.status, p.display_name \
         FROM messages m \
         JOIN participants p ON p.id = m.actor_id \
         WHERE m.id IN ({placeholders})"
    );
    let mut q = sqlx::query_as::<_, HistoryRow>(&query);
    for id in message_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(&mut *conn).await.map_err(db_error)?;
    Ok(rows.into_iter().map(|row| (row.0.clone(), row)).collect())
}

#[async_trait::async_trait]
impl EventStore for SqliteEventStore {
    async fn list_events(
        &self,
        target_agent_id: &ParticipantId,
        after: i64,
        limit: u32,
    ) -> Result<EventPage, EventStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let rows: Vec<EventRow> = sqlx::query_as(
            "SELECT e.id, e.kind, e.target_agent_id, e.sequence, e.message_id, e.created_at \
             FROM agent_events e \
             WHERE e.target_agent_id = ? AND e.sequence > ? \
             ORDER BY e.sequence ASC \
             LIMIT ?",
        )
        .bind(target_agent_id.as_str())
        .bind(after)
        .bind(i64::from(limit))
        .fetch_all(&mut *conn)
        .await
        .map_err(db_error)?;

        let message_ids: Vec<String> = rows.iter().map(|row| row.4.clone()).collect();
        let mentions_by_message = fetch_mentions_for_messages(&mut conn, &message_ids)
            .await
            .map_err(|e| match e {
                crate::ports::message::MessageStoreError::Database(storage) => {
                    EventStoreError::Database(storage)
                }
                other => corrupt(format!("mention projection failed unexpectedly: {other}")),
            })?;
        let recalls_by_message = fetch_recalls_for_messages(&mut conn, &message_ids)
            .await
            .map_err(|e| match e {
                crate::ports::message::MessageStoreError::Database(storage) => {
                    EventStoreError::Database(storage)
                }
                other => corrupt(format!("recall projection failed unexpectedly: {other}")),
            })?;
        let attachments_by_message = fetch_attachments_for_messages(&mut conn, &message_ids)
            .await
            .map_err(|e| match e {
                crate::ports::message::MessageStoreError::Database(storage) => {
                    EventStoreError::Database(storage)
                }
                other => corrupt(format!(
                    "attachment projection failed unexpectedly: {other}"
                )),
            })?;
        let message_rows = fetch_message_rows(&mut conn, &message_ids).await?;

        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            let message_id = row.4.clone();
            let mentions = mentions_by_message
                .get(&message_id)
                .cloned()
                .unwrap_or_default();
            let recalled_at = recalls_by_message.get(&message_id).copied();
            let attachments = attachments_by_message
                .get(&message_id)
                .cloned()
                .unwrap_or_default();
            let history_row = message_rows
                .iter()
                .find(|(id, _)| id == &message_id)
                .map(|(_, row)| row.clone())
                .ok_or_else(|| corrupt(format!("event source message missing {message_id}")))?;
            let history = history_from_row(history_row, mentions, attachments, recalled_at)
                .map_err(|e| match e {
                    crate::ports::message::MessageStoreError::Database(storage) => {
                        EventStoreError::Database(storage)
                    }
                    other => corrupt(format!("event projection failed unexpectedly: {other}")),
                })?;
            events.push(event_from_row(row, history)?);
        }
        Ok(EventPage { events })
    }

    async fn resolve_event_after(
        &self,
        target_agent_id: &ParticipantId,
        cursor_event_id: &AgentEventId,
    ) -> Result<Option<i64>, EventStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        // 按 target Agent 作用域裁决：cursor 引用的 Event 必须属于该
        // Agent。未知 / 跨 Agent 统一 None（对外 400 invalid_request，
        // 不泄露 Event 是否存在或属于哪个 Agent）。
        let sequence: Option<i64> = sqlx::query_scalar(
            "SELECT sequence FROM agent_events \
             WHERE target_agent_id = ? AND id = ? \
             LIMIT 1",
        )
        .bind(target_agent_id.as_str())
        .bind(cursor_event_id.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_error)?;
        Ok(sequence)
    }

    async fn event_targets(
        &self,
        kind: &AgentEventKind,
        message_id: &MessageId,
    ) -> Result<Vec<ParticipantId>, EventStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        // post-commit 反查：只读已提交 agent_events 事实（Event 由触发器
        // 写入，这里不复制 targeting 规则）；DISTINCT 去重 + 稳定排序。
        let targets: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT e.target_agent_id \
             FROM agent_events e \
             WHERE e.kind = ? AND e.message_id = ? \
             ORDER BY e.target_agent_id",
        )
        .bind(kind.as_str())
        .bind(message_id.as_str())
        .fetch_all(&mut *conn)
        .await
        .map_err(db_error)?;
        Ok(targets.into_iter().map(ParticipantId).collect())
    }
}
