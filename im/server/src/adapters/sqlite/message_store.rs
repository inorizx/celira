//! SQLite Message adapter：实现 `ports::message::MessageStore`。
//!
//! 本模块是 Message 事实唯一允许接触 SQLite 文件的地方。HTTP handler 和
//! 用例不得直接操作数据库，只能通过端口调用。
//!
//! 提交使用 `BEGIN IMMEDIATE` 显式单写事务完成先读后写：既有幂等事实
//! 裁决 → 新 key 时重新检查 active Room / Participant / Membership →
//! 校验 Mention 目标（非 group Room 的非空 mentions、目标不是该 Room 的
//! active 成员、目标 participant 非 active 一律 InvalidMention）→ 校验 reply target（同
//! Room、已提交历史 Message、对 actor 任一历史 Membership 窗口可见，
//! 否则 InvalidReplyTarget）→ 事务内按该 Room 当前最大 sequence 分配
//! 下一个正整数 → 插入 Message（reply_to_message_id 随同一行直接写入
//! messages 不可变列）与其 Mention 记录 → 提交。SQLite 单写者与
//! (room_id, sequence) 唯一约束共同裁决并发，不依赖进程内 mutex，也不
//! 在事务外读取 MAX(sequence)。
//!
//! 幂等比较针对规范化 Room ID、完整 body string、规范 Mention 集合与
//! reply target 精确比较：actor + key 已存在且四者完全一致 → 重放（返回
//! 原 Message 事实）；任一不同（包括只改变 Mention 集合或 reply target）
//! → 冲突。既有重放优先于当前 Room / Membership 状态；History 的读取
//! 授权始终按当前 active Membership 独立裁决。
//!
//! 写入内核（幂等裁决、Membership 检查、Mention 插入、sequence 分配与
//! Message 插入）已抽到 `message_kernel`，Completion adapter 与 Message
//! adapter 共用完全相同的正文、Membership、Room sequence、幂等与 SQLite
//! 约束裁决。

use std::collections::HashMap;

use sqlx::Connection;
use sqlx::sqlite::SqliteConnection;

use crate::adapters::sqlite::message_kernel::{
    SQL_BEGIN_IMMEDIATE, fetch_attachments_for_messages, fetch_existing,
    fetch_recalls_for_messages, insert_message, next_sequence, require_active_membership,
    validate_attachment_ids, validate_reply_target,
};
use crate::adapters::sqlite::{finish_rollback, rollback_after_primary};
use crate::domain::agent::IdempotencyKey;
use crate::domain::attachment::AttachmentMetadata;
use crate::domain::message::{Message, MessageBody, MessageId};
use crate::domain::participant::{DisplayName, ParticipantId, ParticipantKind, ParticipantStatus};
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::ports::message::{
    HistoryMessage, MAX_ATTACHMENTS_PER_MESSAGE, MAX_MENTIONS_PER_MESSAGE, MessagePage,
    MessageStoreError, NewMessage, ParticipantProfile, RecallOutcome, RecallStoreError,
    SendMessageOutcome,
};
use crate::ports::storage::{StorageError, StorageFailureKind};

type MessageRow = (
    String,
    String,
    i64,
    String,
    Option<String>,
    Option<String>,
    i64,
);
pub(crate) type HistoryRow = (
    String,
    String,
    i64,
    String,
    Option<String>,
    Option<String>,
    i64,
    String,
    String,
    String,
    String,
);

/// 基于同一 SqlitePool 的 Message 存储。
#[derive(Clone)]
pub struct SqliteMessageStore {
    pool: sqlx::sqlite::SqlitePool,
}

impl SqliteMessageStore {
    pub fn new(pool: sqlx::sqlite::SqlitePool) -> Self {
        Self { pool }
    }
}

fn db_error(e: sqlx::Error) -> MessageStoreError {
    MessageStoreError::Database(StorageError::from(e))
}

/// primary 变体 → composite kind 裁决（CORE_V1_ERROR_HANDLING.md §7.2）：
/// `Database(StorageError)` 沿用嵌套 kind；业务变体（`NotFound` /
/// `IdempotencyConflict` / `InvalidMention`）归 `Unknown`——内部事务完成
/// 失败分类，对外 500 `internal_error`，绝不声称业务冲突已可靠回滚。
fn primary_storage_kind(e: &MessageStoreError) -> StorageFailureKind {
    match e {
        MessageStoreError::Database(storage) => storage.kind(),
        _ => StorageFailureKind::Unknown,
    }
}

/// 持久化行/枚举解析失败：归 `decode_or_corrupt`（对外 `internal_error`）。
fn corrupt(message: impl std::fmt::Display) -> MessageStoreError {
    MessageStoreError::Database(StorageError::from_message(
        StorageFailureKind::DecodeOrCorrupt,
        message.to_string(),
    ))
}

fn message_from_row(
    (id, room_id, sequence, actor_id, body, reply_target, created_at): MessageRow,
) -> Result<Message, MessageStoreError> {
    let body = match &body {
        None => None,
        Some(text) => Some(
            MessageBody::parse(text)
                .map_err(|e| corrupt(format!("stored message body invalid ({e}), id={id}")))?,
        ),
    };
    Ok(Message {
        id: MessageId(id),
        room_id: RoomId(room_id),
        sequence,
        actor_id: ParticipantId(actor_id),
        body,
        mentions: Vec::new(),
        attachments: Vec::new(),
        reply_to_message_id: reply_target.map(MessageId),
        created_at: UtcMillis::from_millis(created_at),
        recalled_at: None,
    })
}

/// Message + actor 当前公共资料投影（读取时的当前投影）→ HistoryMessage。
/// 与 agent_event 读取共用同一行形状与解析，避免产生第二套不一致的
/// Message DTO。`mentions` 是调用方按 message ID 预取的规范排序 Mention
/// 列表；`attachments` 是调用方按 message ID 预取的有序绑定附件元数据
/// （契约 CORE_V1_ATTACHMENTS.md）；`recalled_at` 是调用方按 message ID
/// 预取的撤回事实时点（None = 未撤回）；`reply_to_message_id` 直接来自
/// messages 列的不可变字段，不复制 target 正文、actor 资料、sequence 或
/// 可见性边界。
pub(crate) fn history_from_row(
    (
        id,
        room_id,
        sequence,
        actor_id,
        body,
        reply_target,
        created_at,
        p_id,
        p_kind,
        p_status,
        p_display_name,
    ): HistoryRow,
    mentions: Vec<ParticipantId>,
    attachments: Vec<AttachmentMetadata>,
    recalled_at: Option<UtcMillis>,
) -> Result<HistoryMessage, MessageStoreError> {
    let message = message_from_row((
        id,
        room_id,
        sequence,
        actor_id,
        body,
        reply_target,
        created_at,
    ))?;
    Ok(HistoryMessage {
        message: message
            .with_mentions(mentions)
            .with_attachments(attachments)
            .with_recalled_at(recalled_at),
        actor: ParticipantProfile {
            id: ParticipantId(p_id),
            kind: ParticipantKind::parse(&p_kind)
                .ok_or_else(|| corrupt(format!("unknown participant kind {p_kind:?}")))?,
            status: ParticipantStatus::parse(&p_status)
                .ok_or_else(|| corrupt(format!("unknown participant status {p_status:?}")))?,
            display_name: DisplayName::parse(&p_display_name)
                .map_err(|e| corrupt(format!("stored display name invalid: {e}")))?,
        },
    })
}

/// 一次性读取一组 Message ID 的 Mention 集合（规范 ID 排序），用于
/// History 与 Event 页面的公共视图投影。
pub(crate) async fn fetch_mentions_for_messages(
    conn: &mut SqliteConnection,
    message_ids: &[String],
) -> Result<HashMap<String, Vec<ParticipantId>>, MessageStoreError> {
    let mut map: HashMap<String, Vec<ParticipantId>> = HashMap::new();
    if message_ids.is_empty() {
        return Ok(map);
    }
    let placeholders = message_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let query = format!(
        "SELECT message_id, target_agent_id FROM mentions \
         WHERE message_id IN ({placeholders}) ORDER BY message_id, target_agent_id"
    );
    let mut q = sqlx::query_as::<_, (String, String)>(&query);
    for id in message_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(&mut *conn).await.map_err(db_error)?;
    for (message_id, target) in rows {
        map.entry(message_id)
            .or_default()
            .push(ParticipantId(target));
    }
    Ok(map)
}

/// 新 key 时的 Mention 校验（同一 `BEGIN IMMEDIATE` 事务内）：
/// - 非空 mentions 只允许指向 group Room；
/// - 每个目标必须是该 Room 的 active 成员（Human 或 Agent 均可），且
///   participant 当前 active。
///
/// 任何不满足统一 `InvalidMention`，整个事务回滚，不留 Message / Mention /
/// Event / sequence。
async fn validate_mentions(
    conn: &mut SqliteConnection,
    room_id: &str,
    mentions: &[ParticipantId],
) -> Result<(), MessageStoreError> {
    if mentions.is_empty() {
        return Ok(());
    }
    let room_kind: Option<String> = sqlx::query_scalar("SELECT kind FROM rooms WHERE id = ?")
        .bind(room_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_error)?;
    if room_kind.as_deref() != Some("group") {
        return Err(MessageStoreError::InvalidMention);
    }
    for target in mentions {
        let ok: Option<i64> = sqlx::query_scalar(
            "SELECT 1 \
             FROM participants p \
             JOIN room_memberships m \
               ON m.room_id = ? AND m.participant_id = p.id AND m.active_marker = 1 \
             WHERE p.id = ? AND p.status = 'active' \
             LIMIT 1",
        )
        .bind(room_id)
        .bind(target.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_error)?;
        if ok.is_none() {
            return Err(MessageStoreError::InvalidMention);
        }
    }
    Ok(())
}

/// 单条有界历史查询：keyset pagination（`sequence < before`），取
/// limit + 1 条判断是否还有更旧消息，再反转回升序。
///
/// 可见性：先由调用方要求 reader 当前 active Membership 周期，再按 reader
/// 历次 Membership 周期窗口过滤——`join_after_sequence < message.sequence`
/// 且（`left_before_sequence` 为空或 `message.sequence <=
/// left_before_sequence`）。before/next_before 只基于过滤后的可见行：
/// 不泄露 gap 内消息，也不分页漏重。
async fn fetch_page(
    conn: &mut SqliteConnection,
    reader_id: &str,
    room_id: &str,
    limit: u32,
    before: Option<i64>,
) -> Result<MessagePage, MessageStoreError> {
    let take = i64::from(limit) + 1;
    let rows: Vec<HistoryRow> = match before {
        Some(bound) => sqlx::query_as(
            "SELECT m.id, m.room_id, m.sequence, m.actor_id, m.body, \
                    m.reply_to_message_id, m.created_at, \
                    p.id, p.kind, p.status, p.display_name \
             FROM messages m \
             JOIN participants p ON p.id = m.actor_id \
             WHERE m.room_id = ? AND m.sequence < ? \
               AND EXISTS ( \
                   SELECT 1 FROM room_memberships mm \
                   WHERE mm.room_id = m.room_id AND mm.participant_id = ? \
                     AND mm.join_after_sequence < m.sequence \
                     AND (mm.left_before_sequence IS NULL OR m.sequence <= mm.left_before_sequence) \
               ) \
             ORDER BY m.sequence DESC \
             LIMIT ?",
        )
        .bind(room_id)
        .bind(bound)
        .bind(reader_id)
        .bind(take)
        .fetch_all(&mut *conn)
        .await
        .map_err(db_error)?,
        None => sqlx::query_as(
            "SELECT m.id, m.room_id, m.sequence, m.actor_id, m.body, \
                    m.reply_to_message_id, m.created_at, \
                    p.id, p.kind, p.status, p.display_name \
             FROM messages m \
             JOIN participants p ON p.id = m.actor_id \
             WHERE m.room_id = ? \
               AND EXISTS ( \
                   SELECT 1 FROM room_memberships mm \
                   WHERE mm.room_id = m.room_id AND mm.participant_id = ? \
                     AND mm.join_after_sequence < m.sequence \
                     AND (mm.left_before_sequence IS NULL OR m.sequence <= mm.left_before_sequence) \
               ) \
             ORDER BY m.sequence DESC \
             LIMIT ?",
        )
        .bind(room_id)
        .bind(reader_id)
        .bind(take)
        .fetch_all(&mut *conn)
        .await
        .map_err(db_error)?,
    };

    let message_ids: Vec<String> = rows.iter().map(|row| row.0.clone()).collect();
    let mentions_by_message = fetch_mentions_for_messages(conn, &message_ids).await?;
    let recalls_by_message = fetch_recalls_for_messages(conn, &message_ids).await?;
    let attachments_by_message = fetch_attachments_for_messages(conn, &message_ids).await?;

    let has_more = rows.len() as i64 == take;
    let page_rows: Vec<HistoryRow> = if has_more {
        rows[..rows.len() - 1].to_vec()
    } else {
        rows
    };
    // next_before = 当前返回页按升序展示时的第一条（最旧一条）的 sequence；
    // 下一页传回该值读取 sequence < next_before，无重复、无跳过。
    let next_before = if has_more {
        page_rows.last().map(|row| row.2)
    } else {
        None
    };

    let mut messages = Vec::with_capacity(page_rows.len());
    for row in page_rows.into_iter().rev() {
        let mentions = mentions_by_message.get(&row.0).cloned().unwrap_or_default();
        let recalled_at = recalls_by_message.get(&row.0).copied();
        let attachments = attachments_by_message
            .get(&row.0)
            .cloned()
            .unwrap_or_default();
        messages.push(history_from_row(row, mentions, attachments, recalled_at)?);
    }
    Ok(MessagePage {
        messages,
        next_before,
    })
}

/// Service Direct Room 新消息准入**已随 Agent 服务层退役删除**（2026-09-28）。
///
/// 原来这里有一道 `admit_service_room_message`：Room 若存在 service binding，
/// 新 `message.created` 必须证明其 Access 仍 active 且最新 Publication 仍
/// `accepting`。服务层（Publication / Access / 服务房）整体退役之后，不存在
/// "服务房"这种 Room，也就不存在需要额外准入的那一类消息——**准入判据回到
/// 一条**：房间成员资格（`require_active_membership`，见 `send_message`）。
/// 失败词 `ServiceNotAccepting` 同时退场（`ports::message` / `application::message`
/// 与 `protocol::error` 的服务错误码一并删除）。

#[async_trait::async_trait]
impl crate::ports::message::MessageStore for SqliteMessageStore {
    async fn send_message(
        &self,
        message: &NewMessage,
        idempotency_key: &IdempotencyKey,
    ) -> Result<SendMessageOutcome, MessageStoreError> {
        // 对绕过 application 的调用：在任何逐目标 SQL 循环（validate_mentions /
        // insert_message）之前拒绝超过上限的 Mention 或附件集合。该守卫是
        // 请求形状校验，先于幂等重放裁决，与 application 层顺序一致（正文
        // 校验同样先于幂等查找）；超限请求不产生任何 Message / Mention /
        // 附件绑定 / Event / sequence，甚至不进入事务。
        if message.mentions.len() > MAX_MENTIONS_PER_MESSAGE {
            return Err(MessageStoreError::InvalidMention);
        }
        if message.attachments.len() > MAX_ATTACHMENTS_PER_MESSAGE {
            return Err(MessageStoreError::InvalidAttachment);
        }
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        // 仍使用 BEGIN IMMEDIATE 保证单写者裁决，但由 SQLx Transaction
        // 持有生命周期：future 被取消或提前 drop 时自动启动回滚，不能把
        // 未结束事务带回连接池。
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;

        let outcome: Result<SendMessageOutcome, MessageStoreError> = async {
            if let Some(existing) =
                fetch_existing(&mut tx, message.actor_id.as_str(), idempotency_key.as_str()).await?
            {
                if existing.room_id != message.room_id
                    || existing.body != message.body
                    || existing.mentions != message.mentions
                    || existing
                        .attachments
                        .iter()
                        .map(|meta| meta.id.clone())
                        .collect::<Vec<_>>()
                        != message.attachments
                    || existing.reply_to_message_id != message.reply_to_message_id
                {
                    return Err(MessageStoreError::IdempotencyConflict);
                }
                return Ok(SendMessageOutcome::Replayed { message: existing });
            }

            require_active_membership(&mut tx, message.room_id.as_str(), message.actor_id.as_str())
                .await?;
            validate_mentions(&mut tx, message.room_id.as_str(), &message.mentions).await?;
            if let Some(target) = &message.reply_to_message_id {
                validate_reply_target(
                    &mut tx,
                    message.actor_id.as_str(),
                    message.room_id.as_str(),
                    target.as_str(),
                )
                .await?;
            }
            validate_attachment_ids(
                &mut tx,
                message.actor_id.as_str(),
                message.room_id.as_str(),
                &message.attachments,
                message.created_at,
            )
            .await?;
            let sequence = next_sequence(&mut tx, message.room_id.as_str()).await?;
            insert_message(&mut tx, message, sequence, idempotency_key.as_str()).await?;

            // 回读绑定附件元数据（同一事务内可见）：Created 响应与幂等重放
            // 返回同一 Message 事实形状。
            let attachments = fetch_attachments_for_messages(
                &mut tx,
                std::slice::from_ref(&message.id.as_str().to_owned()),
            )
            .await?
            .remove(message.id.as_str())
            .unwrap_or_default();

            Ok(SendMessageOutcome::Created {
                message: Message {
                    id: message.id.clone(),
                    room_id: message.room_id.clone(),
                    sequence,
                    actor_id: message.actor_id.clone(),
                    body: message.body.clone(),
                    mentions: message.mentions.clone(),
                    attachments,
                    reply_to_message_id: message.reply_to_message_id.clone(),
                    created_at: message.created_at,
                    recalled_at: None,
                },
            })
        }
        .await;

        match outcome {
            Ok(SendMessageOutcome::Created { message }) => {
                tx.commit().await.map_err(db_error)?;
                Ok(SendMessageOutcome::Created { message })
            }
            Ok(replayed @ SendMessageOutcome::Replayed { .. }) => {
                finish_rollback(tx)
                    .await
                    .map_err(MessageStoreError::Database)?;
                Ok(replayed)
            }
            Err(e) => {
                let primary_kind = primary_storage_kind(&e);
                Err(rollback_after_primary(tx, e, primary_kind, MessageStoreError::Database).await)
            }
        }
    }

    async fn list_messages(
        &self,
        reader_id: &ParticipantId,
        room_id: &RoomId,
        limit: u32,
        before: Option<i64>,
    ) -> Result<Option<MessagePage>, MessageStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        // 读取授权按 reader 当前 active Membership 周期独立裁决。两步区分
        // （切片 B2 后端历史端点）：Room 标的存在/活性先裁决——不存在 →
        // Ok(None)（对外 404 room_not_found，不泄露存在性）；Room 存在但
        // reader 非 active member → Err(NotAMember)（对外 403
        // not_room_member，明确 ACL——agent 已通过 BackendAuth 证明身份）。
        let room_active: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM rooms r WHERE r.id = ? AND r.status = 'active' LIMIT 1",
        )
        .bind(room_id.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_error)?;
        if room_active.is_none() {
            return Ok(None);
        }
        let member: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM room_memberships m \
             JOIN participants p ON p.id = m.participant_id \
             WHERE m.room_id = ? AND m.participant_id = ? \
               AND m.active_marker = 1 AND p.status = 'active' \
             LIMIT 1",
        )
        .bind(room_id.as_str())
        .bind(reader_id.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_error)?;
        if member.is_none() {
            return Err(MessageStoreError::NotAMember);
        }
        fetch_page(
            &mut conn,
            reader_id.as_str(),
            room_id.as_str(),
            limit,
            before,
        )
        .await
        .map(Some)
    }

    async fn resolve_message_before(
        &self,
        reader_id: &ParticipantId,
        room_id: &RoomId,
        cursor_message_id: &MessageId,
    ) -> Result<Option<i64>, MessageStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        // 作用域裁决与 History 的周期窗口过滤使用同一谓词：cursor 引用的
        // Message 必须同属该 Room 且被 reader 任一历史 Membership 周期
        // 覆盖。未知 / 跨 Room / 不可见统一 None（对外 400 invalid_request，
        // 不泄露存在性、Room 或边界）。
        let sequence: Option<i64> = sqlx::query_scalar(
            "SELECT m.sequence FROM messages m \
             WHERE m.id = ? AND m.room_id = ? \
               AND EXISTS ( \
                   SELECT 1 FROM room_memberships mm \
                   WHERE mm.room_id = m.room_id AND mm.participant_id = ? \
                     AND mm.join_after_sequence < m.sequence \
                     AND (mm.left_before_sequence IS NULL OR m.sequence <= mm.left_before_sequence) \
               ) \
             LIMIT 1",
        )
        .bind(cursor_message_id.as_str())
        .bind(room_id.as_str())
        .bind(reader_id.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_error)?;
        Ok(sequence)
    }

    async fn recall_message(
        &self,
        actor_id: &ParticipantId,
        room_id: &RoomId,
        message_id: &MessageId,
    ) -> Result<RecallOutcome, RecallStoreError> {
        fn recall_db_error(e: sqlx::Error) -> RecallStoreError {
            RecallStoreError::Database(StorageError::from(e))
        }

        let mut conn = self.pool.acquire().await.map_err(recall_db_error)?;
        // BEGIN IMMEDIATE 由 SQLx Transaction 持有生命周期：future 被取消
        // 或提前 drop 时自动回滚，不能把未结束事务带回连接池；单写者裁决
        // 并发同 Message 的撤回（后到事务先读到既有事实 → 幂等重放）。
        let mut tx = conn
            .begin_with(SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(recall_db_error)?;

        let outcome: Result<RecallOutcome, RecallStoreError> = async {
            // 1. 先按 actor + Room 裁决 active Room / Participant /
            //    Membership（对外 404 room_not_found，先于消息裁决）。
            require_active_membership(&mut tx, room_id.as_str(), actor_id.as_str())
                .await
                .map_err(|e| match e {
                    MessageStoreError::NotFound => RecallStoreError::RoomNotFound,
                    MessageStoreError::NotAMember => RecallStoreError::RoomNotFound,
                    MessageStoreError::Database(storage) => RecallStoreError::Database(storage),
                    other => RecallStoreError::Database(StorageError::from_message(
                        StorageFailureKind::Unknown,
                        format!("recall pre-check failed unexpectedly: {other}"),
                    )),
                })?;

            // 2. 消息必须存在、属于该 Room 且 actor 等于 Message.actor；
            //    统一 MessageNotFound，不泄露存在性或归属。
            let message: Option<(String, String)> =
                sqlx::query_as("SELECT room_id, actor_id FROM messages WHERE id = ?")
                    .bind(message_id.as_str())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(recall_db_error)?;
            match message {
                None => return Err(RecallStoreError::MessageNotFound),
                Some((m_room, _)) if m_room != room_id.as_str() => {
                    return Err(RecallStoreError::MessageNotFound);
                }
                Some((_, m_actor)) if m_actor != actor_id.as_str() => {
                    return Err(RecallStoreError::MessageNotFound);
                }
                Some(_) => {}
            }

            // 3. 先读既有撤回事实：已撤回 → 幂等重放（不受窗口限制、不
            //    重算 recalled_at）。
            let existing: Option<i64> =
                sqlx::query_scalar("SELECT recalled_at FROM message_recalls WHERE message_id = ?")
                    .bind(message_id.as_str())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(recall_db_error)?;
            if let Some(recalled_at) = existing {
                return Ok(RecallOutcome {
                    recalled_at: UtcMillis::from_millis(recalled_at),
                    replayed: true,
                });
            }

            // 4. 首次撤回：单一 INSERT——recalled_at 由同一 statement 内的
            //    SQLite 毫秒 statement-time 生成并在 WHERE 中做同一窗口
            //    裁决（created_at <= now <= created_at + 120000）；窗口不
            //    满足 → 0 行 → WindowExpired（guard 对直接 SQL 独立兜底，
            //    应用路径恒通过）。RETURNING 取回 recalled_at。
            let inserted: Option<i64> = match sqlx::query_scalar::<_, i64>(
                "INSERT INTO message_recalls \
                     (message_id, recalled_by_participant_id, recalled_at) \
                     SELECT ?, ?, CAST(unixepoch('subsec') * 1000 AS INTEGER) \
                     WHERE CAST(unixepoch('subsec') * 1000 AS INTEGER) >= \
                           (SELECT created_at FROM messages WHERE id = ?) \
                       AND CAST(unixepoch('subsec') * 1000 AS INTEGER) <= \
                           (SELECT created_at FROM messages WHERE id = ?) + 120000 \
                     RETURNING recalled_at",
            )
            .bind(message_id.as_str())
            .bind(actor_id.as_str())
            .bind(message_id.as_str())
            .bind(message_id.as_str())
            .fetch_optional(&mut *tx)
            .await
            {
                Ok(value) => value,
                Err(e) => {
                    // 防御性兜底：BEGIN IMMEDIATE 下单写者使「先读后写」
                    // 线性化，正常不会触发；若并发/重放窗口仍撞上身份
                    // 守卫，则回读既有事实按幂等重放收敛，其余数据库
                    // 错误照常上抛。
                    if matches!(e, sqlx::Error::Database(_)) {
                        let existing: Option<i64> = sqlx::query_scalar(
                            "SELECT recalled_at FROM message_recalls WHERE message_id = ?",
                        )
                        .bind(message_id.as_str())
                        .fetch_optional(&mut *tx)
                        .await
                        .map_err(recall_db_error)?;
                        if let Some(recalled_at) = existing {
                            return Ok(RecallOutcome {
                                recalled_at: UtcMillis::from_millis(recalled_at),
                                replayed: true,
                            });
                        }
                    }
                    return Err(RecallStoreError::Database(StorageError::from(e)));
                }
            };
            match inserted {
                Some(recalled_at) => Ok(RecallOutcome {
                    recalled_at: UtcMillis::from_millis(recalled_at),
                    replayed: false,
                }),
                None => Err(RecallStoreError::WindowExpired),
            }
        }
        .await;

        match outcome {
            Ok(
                created @ RecallOutcome {
                    replayed: false, ..
                },
            ) => {
                tx.commit().await.map_err(recall_db_error)?;
                Ok(created)
            }
            Ok(replayed @ RecallOutcome { replayed: true, .. }) => {
                finish_rollback(tx)
                    .await
                    .map_err(RecallStoreError::Database)?;
                Ok(replayed)
            }
            Err(e) => {
                let primary_kind = match &e {
                    RecallStoreError::Database(storage) => storage.kind(),
                    _ => StorageFailureKind::Unknown,
                };
                Err(rollback_after_primary(tx, e, primary_kind, RecallStoreError::Database).await)
            }
        }
    }
}
