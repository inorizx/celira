//! SQLite 普通 Message 提交内核：人类路径与 Agent 的 say 路径共用。
//!
//! Message 事实唯一允许的插入路径。各写入口（人类消息 / Agent say /
//! 撤回 tombstone）
//! 复用完全相同的正文、Membership、Room sequence、幂等与 SQLite 约束
//! 裁决，不产生第二套 Message 插入逻辑。所有函数都必须在
//! `BEGIN IMMEDIATE` 或等价单写事务内调用（事务由调用方持有，本模块不
//! 泄漏 SQLx 事务类型到 domain/application/HTTP 层）。
//!
//! 附件绑定（契约 CORE_V1_ATTACHMENTS.md 9.1）：`insert_message` 先插入
//! message_attachments 绑定行（position 1..n）、再插入 Message 行（messages
//! 的 AFTER INSERT 触发器把附件 state 置为 bound）、最后逐条插入 Mention
//! 行（migration 007 的 Message AFTER INSERT 触发器只处理 direct 配对路径，
//! group 路径由 Mention AFTER INSERT 触发器逐条生成 Event）；绑定行的
//! message_id 外键 DEFERRABLE INITIALLY DEFERRED，孤儿绑定（无 Message）在
//! COMMIT 被拒绝。`validate_attachment_ids` 在进入事务写入前校验每个附件
//! 必须是当前 actor 上传、同 Room、pending 且未过 24 小时绑定窗口；Mention
//! 与附件的任何守卫失败都会使整个 Message 事务回滚，不得留下 Message、
//! 绑定行、Mention、Event 或 sequence。幂等重放读取既有 Message 及其
//! Mention 集合与有序附件集合（`fetch_existing`），由调用方做
//! Room/body/Mention/回复/附件的精确比较。

use sqlx::sqlite::SqliteConnection;

use std::collections::HashMap;

use crate::domain::attachment::{
    AttachmentId, AttachmentMetadata, ContentType, FileName, PENDING_BINDING_WINDOW_MS,
};
use crate::domain::message::{Message, MessageBody, MessageId};
use crate::domain::participant::ParticipantId;
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::ports::message::{
    MAX_ATTACHMENTS_PER_MESSAGE, MAX_MENTIONS_PER_MESSAGE, MessageStoreError, NewMessage,
};
use crate::ports::storage::{StorageError, StorageFailureKind};

/// SQLite 单写事务：所有先读后写必须使用 BEGIN IMMEDIATE，不得使用默认
/// deferred transaction 承担先读后写。
pub(crate) const SQL_BEGIN_IMMEDIATE: &str = "BEGIN IMMEDIATE";

fn db_error(e: sqlx::Error) -> MessageStoreError {
    MessageStoreError::Database(StorageError::from(e))
}

/// 持久化行解析失败：归 `decode_or_corrupt`（对外 `internal_error`）。
fn corrupt(message: impl std::fmt::Display) -> MessageStoreError {
    MessageStoreError::Database(StorageError::from_message(
        StorageFailureKind::DecodeOrCorrupt,
        message.to_string(),
    ))
}

/// 读取一条 Message 的全部 Mention 目标（规范 ID 排序）。事务内调用。
pub(crate) async fn fetch_mentions(
    conn: &mut SqliteConnection,
    message_id: &str,
) -> Result<Vec<ParticipantId>, MessageStoreError> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT target_agent_id FROM mentions WHERE message_id = ? ORDER BY target_agent_id",
    )
    .bind(message_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_error)?;
    Ok(rows.into_iter().map(ParticipantId).collect())
}

/// 幂等事实行（reply target 直接来自 messages 列的不可变字段）。
type ExistingRow = (
    String,
    String,
    i64,
    String,
    Option<String>,
    Option<String>,
    i64,
);

/// 读取既有幂等事实（actor + key）、其 Mention 集合、有序附件集合与 reply
/// target；没有时返回 None。`recalled_at` 一并从 message_recalls 读取
/// （撤回后幂等重放仍按原 body/mentions/attachments/reply_to 内部事实比较，
/// 成功视图由公共投影层按 recalled_at 呈现 tombstone、不得泄原内容）。
pub(crate) async fn fetch_existing(
    conn: &mut SqliteConnection,
    actor_id: &str,
    key: &str,
) -> Result<Option<Message>, MessageStoreError> {
    let row: Option<ExistingRow> = sqlx::query_as(
        "SELECT id, room_id, sequence, actor_id, body, reply_to_message_id, created_at \
         FROM messages WHERE actor_id = ? AND idempotency_key = ?",
    )
    .bind(actor_id)
    .bind(key)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_error)?;
    let Some((id, room_id, sequence, actor_id, body, reply_target, created_at)) = row else {
        return Ok(None);
    };
    let body = match &body {
        None => None,
        Some(text) => Some(
            MessageBody::parse(text)
                .map_err(|e| corrupt(format!("stored message body invalid ({e}), id={id}")))?,
        ),
    };
    let mentions = fetch_mentions(conn, &id).await?;
    let attachments = fetch_attachments_for_messages(conn, std::slice::from_ref(&id))
        .await?
        .remove(&id)
        .unwrap_or_default();
    let recalled_at = fetch_recalled_at(conn, &id).await?;
    Ok(Some(Message {
        id: MessageId(id),
        room_id: RoomId(room_id),
        sequence,
        actor_id: ParticipantId(actor_id),
        body,
        mentions,
        attachments,
        reply_to_message_id: reply_target.map(MessageId),
        created_at: UtcMillis::from_millis(created_at),
        recalled_at,
    }))
}

/// 一次性读取一组 Message ID 的绑定附件公共元数据（position 升序），用于
/// History / Event / 幂等重放的事实投影。事务内调用。
pub(crate) async fn fetch_attachments_for_messages(
    conn: &mut SqliteConnection,
    message_ids: &[String],
) -> Result<HashMap<String, Vec<AttachmentMetadata>>, MessageStoreError> {
    let mut map: HashMap<String, Vec<AttachmentMetadata>> = HashMap::new();
    if message_ids.is_empty() {
        return Ok(map);
    }
    let placeholders = message_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let query = format!(
        "SELECT ma.message_id, a.id, a.file_name, a.content_type, a.size_bytes \
         FROM message_attachments ma \
         JOIN attachments a ON a.id = ma.attachment_id \
         WHERE ma.message_id IN ({placeholders}) \
         ORDER BY ma.message_id, ma.position"
    );
    let mut q = sqlx::query_as::<_, (String, String, String, String, i64)>(&query);
    for id in message_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(&mut *conn).await.map_err(db_error)?;
    for (message_id, id, file_name, content_type, size_bytes) in rows {
        let metadata = AttachmentMetadata {
            id: AttachmentId(id),
            file_name: FileName::parse(&file_name)
                .map_err(|e| corrupt(format!("stored attachment file name invalid ({e})")))?,
            content_type: ContentType::parse(&content_type)
                .map_err(|e| corrupt(format!("stored attachment content type invalid ({e})")))?,
            size_bytes,
        };
        map.entry(message_id).or_default().push(metadata);
    }
    Ok(map)
}

/// 新 key 时的附件校验（同一 `BEGIN IMMEDIATE` 事务内，契约
/// CORE_V1_ATTACHMENTS.md 5.4/9.1）：每个附件必须存在、uploader == actor、
/// 同 Room、state = pending、且未超过 24 小时绑定窗口；任何一个不满足
/// 统一 `AttachmentNotFound`（不泄露存在性或原因）。重复附件 ID 与超过
/// 上限属于 `InvalidAttachment`（请求形状错误，先于本校验）。
pub(crate) async fn validate_attachment_ids(
    conn: &mut SqliteConnection,
    actor_id: &str,
    room_id: &str,
    attachment_ids: &[AttachmentId],
    now: UtcMillis,
) -> Result<(), MessageStoreError> {
    if attachment_ids.len() > MAX_ATTACHMENTS_PER_MESSAGE {
        return Err(MessageStoreError::InvalidAttachment);
    }
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for id in attachment_ids {
        if !seen.insert(id.as_str().to_owned()) {
            return Err(MessageStoreError::InvalidAttachment);
        }
    }
    if attachment_ids.is_empty() {
        return Ok(());
    }
    let placeholders = attachment_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let query = format!(
        "SELECT id, uploader_participant_id, room_id, state, uploaded_at \
         FROM attachments WHERE id IN ({placeholders})"
    );
    let mut q = sqlx::query_as::<_, (String, String, String, String, i64)>(&query);
    for id in attachment_ids {
        q = q.bind(id.as_str());
    }
    let rows = q.fetch_all(&mut *conn).await.map_err(db_error)?;
    let by_id: HashMap<String, (String, String, String, i64)> = rows
        .into_iter()
        .map(|(id, uploader, room, state, uploaded_at)| (id, (uploader, room, state, uploaded_at)))
        .collect();
    for id in attachment_ids {
        let Some((uploader, room, state, uploaded_at)) = by_id.get(id.as_str()) else {
            return Err(MessageStoreError::AttachmentNotFound);
        };
        if uploader != actor_id
            || room != room_id
            || state != "pending"
            || uploaded_at + PENDING_BINDING_WINDOW_MS < now.millis()
        {
            return Err(MessageStoreError::AttachmentNotFound);
        }
    }
    Ok(())
}

/// 读取一条 Message 的撤回事实时点（无撤回为 None）。事务内调用。
pub(crate) async fn fetch_recalled_at(
    conn: &mut SqliteConnection,
    message_id: &str,
) -> Result<Option<UtcMillis>, MessageStoreError> {
    let recalled_at: Option<i64> =
        sqlx::query_scalar("SELECT recalled_at FROM message_recalls WHERE message_id = ?")
            .bind(message_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(db_error)?;
    Ok(recalled_at.map(UtcMillis::from_millis))
}

/// 一次性读取一组 Message ID 的撤回事实时点（无撤回不在 map 中），用于
/// History 与 Event 页面的 tombstone 公共视图投影。事务内调用。
pub(crate) async fn fetch_recalls_for_messages(
    conn: &mut SqliteConnection,
    message_ids: &[String],
) -> Result<HashMap<String, UtcMillis>, MessageStoreError> {
    let mut map: HashMap<String, UtcMillis> = HashMap::new();
    if message_ids.is_empty() {
        return Ok(map);
    }
    let placeholders = message_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let query = format!(
        "SELECT message_id, recalled_at FROM message_recalls \
         WHERE message_id IN ({placeholders}) ORDER BY message_id"
    );
    let mut q = sqlx::query_as::<_, (String, i64)>(&query);
    for id in message_ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(&mut *conn).await.map_err(db_error)?;
    for (message_id, recalled_at) in rows {
        map.insert(message_id, UtcMillis::from_millis(recalled_at));
    }
    Ok(map)
}

/// 按该 Room 当前最大 sequence 分配下一个正整数（事务内调用）。
pub(crate) async fn next_sequence(
    conn: &mut SqliteConnection,
    room_id: &str,
) -> Result<i64, MessageStoreError> {
    let max: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM messages WHERE room_id = ?")
            .bind(room_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(db_error)?;
    Ok(max + 1)
}

/// 新 key 的写入前置检查（两步）：Room active + actor Participant active
/// 同时成立，则 actor 在该 Room 的 Membership 周期必须 active（active_marker=1）。
/// 第一类失败（bad-id / unknown / 非 active Room / 非 active Participant）
/// 返回 `NotFound`（对外 404，不泄露存在性）；第二类失败（Room 与 actor 均
/// 存在且 active、但无 active Membership 周期）返回 `NotAMember`——Human
/// 路径仍映射 404，Backend agent 主动发消息路径（CORE_V1_AGENT_SPEAK.md）
/// 映射 403。两类失败都不留任何 Message、sequence 或幂等事实。发消息只允许
/// 当前 active 周期。
pub(crate) async fn require_active_membership(
    conn: &mut SqliteConnection,
    room_id: &str,
    actor_id: &str,
) -> Result<(), MessageStoreError> {
    let room_and_participant: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM rooms r JOIN participants p ON p.id = ? \
         WHERE r.id = ? AND r.status = 'active' AND p.status = 'active' \
         LIMIT 1",
    )
    .bind(actor_id)
    .bind(room_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_error)?;
    if room_and_participant.is_none() {
        return Err(MessageStoreError::NotFound);
    }
    let membership: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND active_marker = 1 \
         LIMIT 1",
    )
    .bind(room_id)
    .bind(actor_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_error)?;
    if membership.is_none() {
        return Err(MessageStoreError::NotAMember);
    }
    Ok(())
}

/// 新 key 时的 reply target 校验（同一 `BEGIN IMMEDIATE` 事务内，契约
/// CORE_V1_REPLIES_AND_QUOTES.md 3.1/4 + CORE_V1_MESSAGE_RECALL.md §8）：
/// target 必须是已提交的历史 Message，与 source 同一 Room，且
/// target.sequence 落在 source actor 的任一真实 Membership 周期窗口
/// （`join_after_sequence < sequence` 且 `left_before_sequence` 为空或
/// `sequence <= left_before_sequence`）；已撤回的 target（存在
/// message_recalls 事实）同样拒绝——撤回后禁止新建回复该 target，与未知/
/// 跨 Room/不可见统一 `InvalidReplyTarget`（对外 400 invalid_reply_target，
/// 不区分具体原因，不泄露 target 是否存在、是否因撤回）。目标未知、跨
/// Room、不可见、已撤回统一失败，且不留任何 Message、Event 或 sequence。
/// 允许回复自己之前已提交的消息；不允许自指（target 必须是已提交行，
/// source 尚未提交，天然排除）；migration 005 的 messages_reply_target_guard
/// 由迁移 011 重建为同一谓词（排除已撤回 target）对直接 SQL 独立兜底。
pub(crate) async fn validate_reply_target(
    conn: &mut SqliteConnection,
    actor_id: &str,
    room_id: &str,
    target_message_id: &str,
) -> Result<(), MessageStoreError> {
    let ok: Option<i64> = sqlx::query_scalar(
        "SELECT 1 \
         FROM messages t \
         WHERE t.id = ? AND t.room_id = ? \
           AND NOT EXISTS (SELECT 1 FROM message_recalls r WHERE r.message_id = t.id) \
           AND EXISTS ( \
               SELECT 1 FROM room_memberships mm \
               WHERE mm.room_id = t.room_id AND mm.participant_id = ? \
                 AND mm.join_after_sequence < t.sequence \
                 AND (mm.left_before_sequence IS NULL OR t.sequence <= mm.left_before_sequence) \
           ) \
         LIMIT 1",
    )
    .bind(target_message_id)
    .bind(room_id)
    .bind(actor_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_error)?;
    if ok.is_none() {
        return Err(MessageStoreError::InvalidReplyTarget);
    }
    Ok(())
}

/// 事务内插入一条普通 Message（含附件绑定与 Mention 记录；(room_id,
/// sequence) 与 (actor_id, idempotency_key) 唯一约束以及 messages/mentions/
/// attachments/message_attachments 的 CHECK 与触发器守卫；Agent 回复的
/// actor 是 Agent 且不携带 Mention 与 reply target，因此不会生成 Agent
/// Event、不会自触发）。id、room、actor、body、mentions、attachments、
/// reply target 与 created_at 复用 `NewMessage` 事实，reply_to_message_id
/// 随同一行 messages INSERT 直接写入不可变列，sequence 由调用方在事务内
/// 分配。
///
/// 附件绑定顺序（契约 CORE_V1_ATTACHMENTS.md 9.1）：先插入
/// message_attachments 绑定行（position 1..n，message_id 外键
/// DEFERRABLE INITIALLY DEFERRED——同一事务内先于 Message 行合法），再
/// 插入 Message 行（messages_attachments_guard 在 INSERT 时统一裁决
/// body-or-attachments 与绑定一致性，messages_attachments_bound_after_insert
/// 同事务把附件 state 置为 bound），最后逐条插入 Mention 记录；任何失败
/// 整体回滚，不得留下 Message、绑定行、Mention、Event、sequence 或附件
/// 状态变化。
pub(crate) async fn insert_message(
    conn: &mut SqliteConnection,
    message: &NewMessage,
    sequence: i64,
    idempotency_key: &str,
) -> Result<(), MessageStoreError> {
    // 兜底守卫：任何调用方在逐条 Mention / 附件绑定 INSERT 循环前必须
    // 满足上限；
    // Mention，正常路径不会触发。
    if message.mentions.len() > MAX_MENTIONS_PER_MESSAGE {
        return Err(MessageStoreError::InvalidMention);
    }
    if message.attachments.len() > MAX_ATTACHMENTS_PER_MESSAGE {
        return Err(MessageStoreError::InvalidAttachment);
    }
    for (index, attachment_id) in message.attachments.iter().enumerate() {
        sqlx::query(
            "INSERT INTO message_attachments (message_id, attachment_id, position) \
             VALUES (?, ?, ?)",
        )
        .bind(message.id.as_str())
        .bind(attachment_id.as_str())
        .bind(i64::try_from(index + 1).expect("position fits i64"))
        .execute(&mut *conn)
        .await
        .map_err(db_error)?;
    }
    sqlx::query(
        "INSERT INTO messages \
         (id, room_id, sequence, actor_id, body, reply_to_message_id, idempotency_key, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(message.id.as_str())
    .bind(message.room_id.as_str())
    .bind(sequence)
    .bind(message.actor_id.as_str())
    .bind(message.body.as_ref().map(|body| body.as_str()))
    .bind(message.reply_to_message_id.as_ref().map(|id| id.as_str()))
    .bind(idempotency_key)
    .bind(message.created_at.millis())
    .execute(&mut *conn)
    .await
    .map_err(db_error)?;
    for target in &message.mentions {
        sqlx::query("INSERT INTO mentions (message_id, target_agent_id) VALUES (?, ?)")
            .bind(message.id.as_str())
            .bind(target.as_str())
            .execute(&mut *conn)
            .await
            .map_err(db_error)?;
    }
    Ok(())
}
