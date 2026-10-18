//! SQLite Human Event adapter：实现 `ports::human_event::HumanEventStore`。
//!
//! 本模块是 Human Event 恢复读取与 post-commit 唤醒目标查询唯一允许接触
//! SQLite 文件的地方。HTTP handler 和用例不得直接操作数据库，只能通过
//! 端口调用。
//!
//! Human Event 生成不经过本模块：source 事实 → 共享 fact → per-Human
//! delivery 的原子成立由 migration 009 的触发器在同一 SQLite 事务内完成，
//! 本模块只提供：
//! - `list_events`：当前 Human 的 `sequence > after` 纯读取（带读取时
//!   Membership 授权过滤，唯一例外是读取者自己的 member.left）；
//! - `delivery_targets`：post-commit 按完整 source 身份（kind +
//!   source_id）反查被投递的 Human（从**已提交**的 delivery/fact 事实
//!   查询，不在 Rust 层复制 targeting 规则）。
//!
//! 快照是触发器固化的 JSON 文本，读取时原样透传给协议层投影；kind /
//! 快照解析失败视为持久事实无法解析（500），不泄露 SQL。

use sqlx::sqlite::SqlitePool;

use crate::domain::human_event::{
    HumanEventFact, HumanEventFactId, HumanEventKind, HumanEventSource,
};
use crate::domain::participant::ParticipantId;
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::ports::human_event::{HumanEventStore, HumanEventStoreError, HumanEventWithFact};
use crate::ports::storage::{StorageError, StorageFailureKind};

/// 扁平行：fact 事实 + delivery sequence。
type HumanEventRow = (String, String, String, String, i64, String, i64);

/// 基于同一 SqlitePool 的 Human Event 存储。
#[derive(Clone)]
pub struct SqliteHumanEventStore {
    pool: SqlitePool,
}

impl SqliteHumanEventStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn db_error(e: sqlx::Error) -> HumanEventStoreError {
    HumanEventStoreError::Database(StorageError::from(e))
}

/// 持久化行/枚举解析失败：归 `decode_or_corrupt`（对外 `internal_error`）。
fn corrupt(message: impl std::fmt::Display) -> HumanEventStoreError {
    HumanEventStoreError::Database(StorageError::from_message(
        StorageFailureKind::DecodeOrCorrupt,
        message.to_string(),
    ))
}

fn event_from_row(
    (fact_id, kind, source_id, room_id, created_at, snapshot, sequence): HumanEventRow,
) -> Result<HumanEventWithFact, HumanEventStoreError> {
    let kind = HumanEventKind::parse(&kind)
        .ok_or_else(|| corrupt(format!("unknown human event kind {kind:?}")))?;
    let fact = HumanEventFact {
        id: HumanEventFactId(fact_id),
        kind,
        source_id,
        room_id: RoomId(room_id),
        created_at: UtcMillis::from_millis(created_at),
        snapshot,
    };
    Ok(HumanEventWithFact { fact, sequence })
}

#[async_trait::async_trait]
impl HumanEventStore for SqliteHumanEventStore {
    async fn list_events(
        &self,
        human_id: &ParticipantId,
        after: i64,
        limit: u32,
    ) -> Result<Vec<HumanEventWithFact>, HumanEventStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let rows: Vec<HumanEventRow> = sqlx::query_as(
            "SELECT f.fact_id, f.kind, f.source_id, f.room_id, f.created_at, f.snapshot, d.sequence \
             FROM human_event_deliveries d \
             JOIN human_event_facts f ON f.fact_id = d.fact_id \
             WHERE d.human_id = ? AND d.sequence > ? \
               AND ( \
                   EXISTS (SELECT 1 FROM room_memberships mm \
                           WHERE mm.room_id = f.room_id AND mm.participant_id = ? \
                             AND mm.active_marker = 1) \
                   OR (f.kind = 'member.left' \
                       AND json_extract(f.snapshot, '$.participant.id') = ?) \
               ) \
             ORDER BY d.sequence ASC \
             LIMIT ?",
        )
        .bind(human_id.as_str())
        .bind(after)
        .bind(human_id.as_str())
        .bind(human_id.as_str())
        .bind(i64::from(limit))
        .fetch_all(&mut *conn)
        .await
        .map_err(db_error)?;
        rows.into_iter().map(event_from_row).collect()
    }

    async fn resolve_event_after(
        &self,
        human_id: &ParticipantId,
        cursor_fact_id: &HumanEventFactId,
    ) -> Result<Option<i64>, HumanEventStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        // 按当前 Human 作用域裁决：cursor 引用的 fact 必须已被该 Human
        // 投递（delivery 是永久事实，投递即授权）。未知 / 跨 Human /
        // 从未投递统一 None（对外 400 invalid_request，不泄露 fact 是否
        // 存在或属于谁）。读取期 Membership 过滤由 list_events 负责，
        // 游标只表达该 Human 自己序列上的位置。
        let sequence: Option<i64> = sqlx::query_scalar(
            "SELECT sequence FROM human_event_deliveries \
             WHERE human_id = ? AND fact_id = ? \
             LIMIT 1",
        )
        .bind(human_id.as_str())
        .bind(cursor_fact_id.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_error)?;
        Ok(sequence)
    }

    async fn delivery_targets(
        &self,
        sources: &[HumanEventSource],
    ) -> Result<Vec<ParticipantId>, HumanEventStoreError> {
        if sources.is_empty() {
            return Ok(Vec::new());
        }
        // fact 唯一身份是 (kind, source_id)：WHERE 必须同时匹配两者，否则
        // 同一 source_id 字符串的其他 kind fact 会错误唤醒无关 Human。
        let mut query = String::from(
            "SELECT DISTINCT d.human_id \
             FROM human_event_deliveries d \
             JOIN human_event_facts f ON f.fact_id = d.fact_id \
             WHERE ",
        );
        for (i, _source) in sources.iter().enumerate() {
            if i > 0 {
                query.push_str(" OR ");
            }
            query.push_str("(f.kind = ? AND f.source_id = ?)");
        }
        query.push_str(" ORDER BY d.human_id");
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let mut q = sqlx::query_scalar::<_, String>(&query);
        for source in sources {
            q = q.bind(source.kind().as_str()).bind(source.source_id());
        }
        let rows: Vec<String> = q.fetch_all(&mut *conn).await.map_err(db_error)?;
        Ok(rows.into_iter().map(ParticipantId).collect())
    }
}
