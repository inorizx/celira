//! SQLite Attachment adapter：实现 `ports::attachment::AttachmentStore`。
//!
//! 本模块是附件元数据/状态/绑定读取唯一允许接触 SQLite 文件的地方（附件
//! 绑定写入发生在 Message/Completion 提交事务内，见 message_kernel，不走
//! 本模块）。HTTP handler 和用例不得直接操作数据库，只能通过端口调用。
//!
//! 本模块只提供：
//! - `create_pending`：上传成功后建立 pending 附件行（Room / Participant /
//!   Membership 必须 active，与 Message 写入同一谓词）；
//! - `authorized_attachment`：下载授权裁决（绑定存在、Message 同 Room 且
//!   uploader==Message actor、未撤回、读取者当前 active 成员且消息在其历史
//!   Membership 周期窗口内——与 History 同一可见性谓词）；其余一律 None，
//!   调用方统一 404 attachment_not_found；
//! - `purge_expired_pending`：内部受控清理——`BEGIN IMMEDIATE` 单写事务内
//!   原子裁决并删除「过期（uploaded_at + 24h <= now）、state=pending、无
//!   message_attachments 绑定行」的附件行（pending 是私有暂存租约，到期
//!   即可回收；bound 与公共事实绝不删除），返回被删除行的内部 storage
//!   key 供调用方随后删除对应字节；
//! - `referenced_storage_keys`：启动 orphan sweep 的引用集合（purge 之后
//!   仍存在的行；被引用的文件绝不清理）。
//!
//! Blob 字节存储由 `adapters::blob` 提供，本模块不接触文件系统。

use std::collections::HashSet;

use sqlx::Connection;
use sqlx::sqlite::SqlitePool;

use crate::domain::attachment::{
    Attachment, AttachmentId, AttachmentState, NewAttachment, PENDING_BINDING_WINDOW_MS, StorageKey,
};
use crate::domain::participant::ParticipantId;
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::ports::attachment::{AttachmentStore, AttachmentStoreError};
use crate::ports::storage::{StorageError, StorageFailureKind};

use super::rollback_after_primary;

/// primary 变体 → composite kind 裁决（CORE_V1_ERROR_HANDLING.md §7.2）：
/// `Database(StorageError)` 沿用嵌套 kind；`RoomNotFound` 业务变体归
/// `Unknown`——内部事务完成失败分类，对外 500 `internal_error`。
fn primary_storage_kind(e: &AttachmentStoreError) -> StorageFailureKind {
    match e {
        AttachmentStoreError::Database(storage) => storage.kind(),
        AttachmentStoreError::RoomNotFound => StorageFailureKind::Unknown,
    }
}

type AttachmentRow = (
    String,
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
    String,
    i64,
);

/// 基于同一 SqlitePool 的附件元数据存储。
#[derive(Clone)]
pub struct SqliteAttachmentStore {
    pool: SqlitePool,
}

impl SqliteAttachmentStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn db_error(e: sqlx::Error) -> AttachmentStoreError {
    AttachmentStoreError::Database(StorageError::from(e))
}

/// 持久化行/枚举解析失败：归 `decode_or_corrupt`（对外 `internal_error`）。
fn corrupt(message: impl std::fmt::Display) -> AttachmentStoreError {
    AttachmentStoreError::Database(StorageError::from_message(
        StorageFailureKind::DecodeOrCorrupt,
        message.to_string(),
    ))
}

fn attachment_from_row(
    (
        id,
        uploader,
        room_id,
        file_name,
        content_type,
        size_bytes,
        state,
        storage_key,
        sha256_hex,
        uploaded_at,
    ): AttachmentRow,
) -> Result<Attachment, AttachmentStoreError> {
    use crate::domain::attachment::{ContentType, FileName, StorageKey};
    Ok(Attachment {
        id: AttachmentId(id),
        uploader_participant_id: ParticipantId(uploader),
        room_id: RoomId(room_id),
        file_name: FileName::parse(&file_name)
            .map_err(|e| corrupt(format!("stored attachment file name invalid ({e})")))?,
        content_type: ContentType::parse(&content_type)
            .map_err(|e| corrupt(format!("stored attachment content type invalid ({e})")))?,
        size_bytes,
        state: AttachmentState::parse(&state)
            .ok_or_else(|| corrupt(format!("unknown attachment state {state:?}")))?,
        storage_key: StorageKey::parse_canonical(&storage_key)
            .ok_or_else(|| corrupt(format!("stored storage key invalid {storage_key:?}")))?,
        sha256_hex: is_sha256_hex(&sha256_hex)
            .then_some(sha256_hex)
            .ok_or_else(|| corrupt("stored attachment SHA-256 invalid"))?,
        uploaded_at: UtcMillis::from_millis(uploaded_at),
    })
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[async_trait::async_trait]
impl AttachmentStore for SqliteAttachmentStore {
    async fn create_pending(&self, attachment: &NewAttachment) -> Result<(), AttachmentStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        // 上传授权：Room active、uploader Participant active、该 Room 有
        // active Membership 周期（与 Message 写入同一谓词）。守卫不写死
        // participant kind：Human 与 Agent 对称（当前公开入口决定 kind）。
        let visible: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM rooms r \
             JOIN room_memberships m ON m.room_id = r.id AND m.participant_id = ? \
             JOIN participants p ON p.id = m.participant_id \
             WHERE r.id = ? AND r.status = 'active' AND m.active_marker = 1 AND p.status = 'active' \
             LIMIT 1",
        )
        .bind(attachment.uploader_participant_id.as_str())
        .bind(attachment.room_id.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_error)?;
        if visible.is_none() {
            return Err(AttachmentStoreError::RoomNotFound);
        }
        sqlx::query(
            "INSERT INTO attachments \
             (id, uploader_participant_id, room_id, file_name, content_type, size_bytes, \
              state, storage_key, sha256_hex, uploaded_at) \
             VALUES (?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?)",
        )
        .bind(attachment.id.as_str())
        .bind(attachment.uploader_participant_id.as_str())
        .bind(attachment.room_id.as_str())
        .bind(attachment.file_name.as_str())
        .bind(attachment.content_type.as_str())
        .bind(attachment.size_bytes)
        .bind(attachment.storage_key.as_str())
        .bind(&attachment.sha256_hex)
        .bind(attachment.uploaded_at.millis())
        .execute(&mut *conn)
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn authorized_attachment(
        &self,
        reader_id: &ParticipantId,
        room_id: &RoomId,
        attachment_id: &AttachmentId,
    ) -> Result<Option<Attachment>, AttachmentStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        // 下载授权（契约 CORE_V1_ATTACHMENTS.md 5.3/7）：附件必须 bound、
        // 绑定 Message 与该附件同 Room 且 uploader == Message actor、
        // Message 未撤回、读取者当前有该 Room 的 active Membership 周期且
        // 该 Message 落在读取者任一历史 Membership 周期窗口内（与 History
        // 同一可见性谓词）。其余（pending/unknown/跨 Room/跨 actor/撤回/
        // 非成员）一律 None → 404 attachment_not_found，不泄露存在性。
        let row: Option<AttachmentRow> = sqlx::query_as(
            "SELECT a.id, a.uploader_participant_id, a.room_id, a.file_name, \
                    a.content_type, a.size_bytes, a.state, a.storage_key, a.sha256_hex, a.uploaded_at \
             FROM attachments a \
             JOIN message_attachments ma ON ma.attachment_id = a.id \
             JOIN messages m ON m.id = ma.message_id \
             WHERE a.id = ? AND a.room_id = ? AND a.state = 'bound' \
               AND a.uploader_participant_id = m.actor_id \
               AND NOT EXISTS (SELECT 1 FROM message_recalls r WHERE r.message_id = m.id) \
               AND EXISTS ( \
                   SELECT 1 FROM room_memberships mm \
                   WHERE mm.room_id = m.room_id AND mm.participant_id = ? \
                     AND mm.join_after_sequence < m.sequence \
                     AND (mm.left_before_sequence IS NULL OR m.sequence <= mm.left_before_sequence) \
               ) \
               AND EXISTS ( \
                   SELECT 1 FROM room_memberships mm2 \
                   WHERE mm2.room_id = m.room_id AND mm2.participant_id = ? AND mm2.active_marker = 1 \
               ) \
             LIMIT 1",
        )
        .bind(attachment_id.as_str())
        .bind(room_id.as_str())
        .bind(reader_id.as_str())
        .bind(reader_id.as_str())
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_error)?;
        row.map(attachment_from_row).transpose()
    }

    async fn purge_expired_pending(
        &self,
        now: UtcMillis,
    ) -> Result<Vec<StorageKey>, AttachmentStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        // BEGIN IMMEDIATE 单写事务：与绑定事务（Message/Completion 提交
        // 同为 BEGIN IMMEDIATE）写锁线性化——先绑定则清理不可见（已
        // bound/有绑定），先清理则绑定整体 404 回滚，绝不误删；任何失败
        // 整体回滚。事务由 SQLx Transaction 持有生命周期，future 被取消
        // 或提前 drop 时自动启动回滚。
        let mut tx = conn
            .begin_with(crate::adapters::sqlite::message_kernel::SQL_BEGIN_IMMEDIATE)
            .await
            .map_err(db_error)?;
        let outcome: Result<Vec<String>, AttachmentStoreError> = async {
            // 同一事务内先选后删：条件与数据库 DELETE 守卫（过期 + pending
            // + 无绑定行）完全一致；已 bound、有绑定、未过期一律不选不删。
            let keys: Vec<String> = sqlx::query_scalar(
                "SELECT storage_key FROM attachments \
                 WHERE state = 'pending' AND uploaded_at + ? <= ? \
                   AND NOT EXISTS (SELECT 1 FROM message_attachments ma \
                                   WHERE ma.attachment_id = attachments.id) \
                 ORDER BY storage_key",
            )
            .bind(PENDING_BINDING_WINDOW_MS)
            .bind(now.millis())
            .fetch_all(&mut *tx)
            .await
            .map_err(db_error)?;
            if !keys.is_empty() {
                sqlx::query(
                    "DELETE FROM attachments \
                     WHERE state = 'pending' AND uploaded_at + ? <= ? \
                       AND NOT EXISTS (SELECT 1 FROM message_attachments ma \
                                       WHERE ma.attachment_id = attachments.id)",
                )
                .bind(PENDING_BINDING_WINDOW_MS)
                .bind(now.millis())
                .execute(&mut *tx)
                .await
                .map_err(db_error)?;
            }
            Ok(keys)
        }
        .await;
        match outcome {
            Ok(keys) => {
                tx.commit().await.map_err(db_error)?;
                Ok(keys.into_iter().map(StorageKey).collect())
            }
            Err(primary) => {
                let primary_kind = primary_storage_kind(&primary);
                Err(rollback_after_primary(
                    tx,
                    primary,
                    primary_kind,
                    AttachmentStoreError::Database,
                )
                .await)
            }
        }
    }

    async fn referenced_storage_keys(&self) -> Result<HashSet<String>, AttachmentStoreError> {
        let mut conn = self.pool.acquire().await.map_err(db_error)?;
        let keys: Vec<String> =
            sqlx::query_scalar("SELECT storage_key FROM attachments ORDER BY storage_key")
                .fetch_all(&mut *conn)
                .await
                .map_err(db_error)?;
        Ok(keys.into_iter().collect())
    }
}

#[cfg(test)]
mod hash_tests {
    use super::*;

    #[test]
    fn persisted_sha256_shape_is_strict_lowercase_hex_and_corrupt_rows_fail_closed() {
        assert!(is_sha256_hex(&"a".repeat(64)));
        assert!(is_sha256_hex(&"0".repeat(64)));
        assert!(!is_sha256_hex(&"A".repeat(64)));
        assert!(!is_sha256_hex(&"g".repeat(64)));
        assert!(!is_sha256_hex(&"a".repeat(63)));
        assert!(!is_sha256_hex(&"a".repeat(65)));

        let row: AttachmentRow = (
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
            "a.txt".to_owned(),
            "text/plain".to_owned(),
            1,
            "bound".to_owned(),
            uuid::Uuid::new_v4().to_string(),
            "A".repeat(64),
            1,
        );
        let err = attachment_from_row(row).expect_err("corrupt stored hash must fail closed");
        let AttachmentStoreError::Database(storage) = err else {
            panic!("corrupt row must be classified as a database decode failure");
        };
        assert_eq!(storage.kind(), StorageFailureKind::DecodeOrCorrupt);
    }
}
