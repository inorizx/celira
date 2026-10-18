//! 附件用例：上传（两步式第一步）与下载（授权读取）。
//!
//! 用例层负责规则编排与事务边界：解析 Room、校验 file_name/content_type
//! 形状、检查 16 MiB 字节上限，生成 Core 分配的附件 ID、storage key 与
//! 时间，先经 `ports::blob::BlobStorage` 原子写入字节、再经
//! `ports::attachment::AttachmentStore` 建立 pending 行（行插入失败时
//! best-effort 删除刚写入的字节）；下载先经 AttachmentStore 授权裁决
//! （None → 统一 404 attachment_not_found），再经 BlobStorage 读取字节。
//! 公共结果只含 id/file_name/content_type/size_bytes/state/uploaded_at，
//! 绝不返回 storage key、SHA-256 或文件路径（契约 CORE_V1_ATTACHMENTS.md
//! 5.1/5.3/6）。

use sha2::Digest as _;

use crate::domain::attachment::{
    Attachment, AttachmentId, ContentType, FileName, MAX_ATTACHMENT_BYTES, NewAttachment,
    StorageKey,
};
use crate::domain::participant::ParticipantId;
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::ports::attachment::{AttachmentStore, AttachmentStoreError};
use crate::ports::blob::BlobStorage;
use crate::ports::storage::StorageError;

/// 上传结果：pending 附件公共事实。
#[derive(Debug)]
pub struct UploadAttachmentOutcome {
    pub attachment: Attachment,
}

#[derive(Debug)]
pub enum UploadAttachmentError {
    /// bad room、Room 非 active、actor 非 active 或该 Room 无 active
    /// Membership：对外统一 404 room_not_found。
    RoomNotFound,
    /// file_name / content_type 缺失或形状非法（400
    /// invalid_attachment_metadata）。
    InvalidAttachmentMetadata(String),
    /// 文件字节超过 16 MiB（413 payload_too_large）。
    PayloadTooLarge,
    /// 存储不可用（Blob 或 SQLite）。
    Store(StorageError),
}

impl std::fmt::Display for UploadAttachmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RoomNotFound => write!(f, "room not found"),
            Self::InvalidAttachmentMetadata(e) => write!(f, "invalid attachment metadata: {e}"),
            Self::PayloadTooLarge => write!(f, "attachment exceeds the 16 MiB byte bound"),
            Self::Store(e) => write!(f, "attachment store failure: {e}"),
        }
    }
}

impl std::error::Error for UploadAttachmentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// 上传：生成 Core 分配的附件身份 → BlobStorage 原子写入字节 → SQLite
/// pending 行（同一请求内完成；行插入失败 best-effort 删除刚写入的字节，
/// 崩溃残留由启动与运行期共用的附件维护链清理）。`bytes.len() > 16 MiB` 在进入
/// BlobStorage 前拒绝（413）。SHA-256 由 Core 对上传字节计算并只作内部
/// 完整性事实。
pub async fn upload_attachment(
    store: &dyn AttachmentStore,
    blob: &dyn BlobStorage,
    actor_id: &ParticipantId,
    room_id_raw: &str,
    file_name_raw: &str,
    content_type_raw: &str,
    bytes: &[u8],
) -> Result<UploadAttachmentOutcome, UploadAttachmentError> {
    // 请求形状先于一切存储副作用：元数据与 16 MiB 上限先校验，Room 与
    // Membership 由存储层裁决（bad/unknown/非成员统一 404）。
    let file_name = FileName::parse(file_name_raw)
        .map_err(|e| UploadAttachmentError::InvalidAttachmentMetadata(e.to_string()))?;
    let content_type = ContentType::parse(content_type_raw)
        .map_err(|e| UploadAttachmentError::InvalidAttachmentMetadata(e.to_string()))?;
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(UploadAttachmentError::PayloadTooLarge);
    }
    let room_id =
        RoomId::parse_canonical(room_id_raw).ok_or(UploadAttachmentError::RoomNotFound)?;
    let id = AttachmentId::new_v4();
    let storage_key = StorageKey::new_v4();
    let uploaded_at = UtcMillis::now();
    let sha256_hex: String = hex::encode(sha2::Sha256::digest(bytes));

    // 1. 先写字节（原子 + 文件/目录持久化）：崩溃于此处 → 孤儿文件，
    //    由统一附件维护链按 grace 回收。
    blob.write(&storage_key, bytes)
        .await
        .map_err(UploadAttachmentError::Store)?;

    // 2. 再插 pending 行（同一请求内）。
    let new_attachment = NewAttachment {
        id: id.clone(),
        uploader_participant_id: actor_id.clone(),
        room_id: room_id.clone(),
        file_name: file_name.clone(),
        content_type: content_type.clone(),
        size_bytes: bytes.len() as i64,
        storage_key: storage_key.clone(),
        sha256_hex: sha256_hex.clone(),
        uploaded_at,
    };
    match store.create_pending(&new_attachment).await {
        Ok(()) => {}
        Err(AttachmentStoreError::RoomNotFound) => {
            // 行未落库：best-effort 清理字节；清理失败只记录脱敏 warning
            // （孤儿由统一附件维护链兜底），不掩盖业务失败。
            if let Err(cleanup_error) = blob.delete(&storage_key).await {
                let _ = cleanup_error;
                tracing::warn!(
                    operation = "attachment.upload.cleanup",
                    error_kind = "connection_or_io",
                    "failed to remove blob bytes after pending row insert failed"
                );
            }
            return Err(UploadAttachmentError::RoomNotFound);
        }
        Err(AttachmentStoreError::Database(e)) => {
            if let Err(cleanup_error) = blob.delete(&storage_key).await {
                let _ = cleanup_error;
                tracing::warn!(
                    operation = "attachment.upload.cleanup",
                    error_kind = "connection_or_io",
                    "failed to remove blob bytes after pending row insert failed"
                );
            }
            return Err(UploadAttachmentError::Store(e));
        }
    }

    Ok(UploadAttachmentOutcome {
        attachment: Attachment {
            id,
            uploader_participant_id: actor_id.clone(),
            room_id: room_id.clone(),
            file_name,
            content_type,
            size_bytes: bytes.len() as i64,
            state: crate::domain::attachment::AttachmentState::Pending,
            storage_key,
            sha256_hex,
            uploaded_at,
        },
    })
}

#[derive(Debug)]
pub enum DownloadAttachmentError {
    /// room_id / attachment_id 不是规范 UUID v4（400 invalid_request）。
    InvalidRequest,
    /// pending / unknown / 跨 Room / 跨 actor / 过期 / 撤回 / 非成员：
    /// 统一 404 attachment_not_found（不泄露存在性或原因）。
    AttachmentNotFound,
    /// 存储不可用。
    Store(StorageError),
}

impl std::fmt::Display for DownloadAttachmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest => write!(f, "invalid attachment id"),
            Self::AttachmentNotFound => write!(f, "attachment not found"),
            Self::Store(e) => write!(f, "attachment store failure: {e}"),
        }
    }
}

impl std::error::Error for DownloadAttachmentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

/// 下载：AttachmentStore 授权裁决（None → 统一 404）→ BlobStorage 读取
/// 字节。返回公共元数据（不含 storage key / hash）与字节。
pub async fn download_attachment(
    store: &dyn AttachmentStore,
    blob: &dyn BlobStorage,
    reader_id: &ParticipantId,
    room_id_raw: &str,
    attachment_id_raw: &str,
) -> Result<(crate::domain::attachment::AttachmentMetadata, Vec<u8>), DownloadAttachmentError> {
    let room_id =
        RoomId::parse_canonical(room_id_raw).ok_or(DownloadAttachmentError::InvalidRequest)?;
    let attachment_id = AttachmentId::parse_canonical(attachment_id_raw)
        .ok_or(DownloadAttachmentError::InvalidRequest)?;
    let attachment = match store
        .authorized_attachment(reader_id, &room_id, &attachment_id)
        .await
    {
        Ok(Some(attachment)) => attachment,
        Ok(None) => return Err(DownloadAttachmentError::AttachmentNotFound),
        Err(AttachmentStoreError::Database(e)) => return Err(DownloadAttachmentError::Store(e)),
        Err(AttachmentStoreError::RoomNotFound) => {
            return Err(DownloadAttachmentError::AttachmentNotFound);
        }
    };
    let bytes = blob
        .read(&attachment.storage_key)
        .await
        .map_err(DownloadAttachmentError::Store)?;
    let size_matches =
        i64::try_from(bytes.len()).is_ok_and(|actual_size| actual_size == attachment.size_bytes);
    let actual_sha256 = hex::encode(sha2::Sha256::digest(&bytes));
    if !size_matches || actual_sha256 != attachment.sha256_hex {
        return Err(DownloadAttachmentError::Store(StorageError::from_message(
            crate::ports::storage::StorageFailureKind::DecodeOrCorrupt,
            "stored attachment bytes do not match durable metadata",
        )));
    }
    Ok((attachment.metadata(), bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::storage::StorageFailureKind;

    struct RecordingBlob {
        written: std::sync::Mutex<Vec<(String, Vec<u8>)>>,
        deleted: std::sync::Mutex<Vec<String>>,
        fail_write: bool,
        fail_read: bool,
    }

    #[async_trait::async_trait]
    impl BlobStorage for RecordingBlob {
        async fn write(&self, key: &StorageKey, bytes: &[u8]) -> Result<(), StorageError> {
            if self.fail_write {
                return Err(StorageError::from_message(
                    StorageFailureKind::ConnectionOrIo,
                    "injected write failure",
                ));
            }
            self.written
                .lock()
                .unwrap()
                .push((key.as_str().to_owned(), bytes.to_vec()));
            Ok(())
        }
        async fn read(&self, key: &StorageKey) -> Result<Vec<u8>, StorageError> {
            if self.fail_read {
                return Err(StorageError::from_message(
                    StorageFailureKind::ConnectionOrIo,
                    "injected read failure",
                ));
            }
            Ok(self
                .written
                .lock()
                .unwrap()
                .iter()
                .find(|(k, _)| k == key.as_str())
                .map(|(_, b)| b.clone())
                .unwrap_or_default())
        }
        async fn delete(&self, key: &StorageKey) -> Result<(), StorageError> {
            self.deleted.lock().unwrap().push(key.as_str().to_owned());
            Ok(())
        }
        async fn sweep_orphans(
            &self,
            _referenced: &std::collections::HashSet<String>,
            _grace: std::time::Duration,
        ) -> Result<(), StorageError> {
            Ok(())
        }
    }

    struct RecordingStore {
        pending: std::sync::Mutex<Vec<NewAttachment>>,
        authorized: std::sync::Mutex<Vec<(String, String, String)>>,
        fail_pending: Option<AttachmentStoreError>,
        authorized_result: Option<Attachment>,
    }

    #[async_trait::async_trait]
    impl AttachmentStore for RecordingStore {
        async fn create_pending(
            &self,
            attachment: &NewAttachment,
        ) -> Result<(), AttachmentStoreError> {
            self.pending.lock().unwrap().push(attachment.clone());
            match &self.fail_pending {
                Some(e) => Err(match e {
                    AttachmentStoreError::RoomNotFound => AttachmentStoreError::RoomNotFound,
                    AttachmentStoreError::Database(d) => AttachmentStoreError::Database(d.clone()),
                }),
                None => Ok(()),
            }
        }
        async fn authorized_attachment(
            &self,
            reader_id: &ParticipantId,
            room_id: &RoomId,
            attachment_id: &AttachmentId,
        ) -> Result<Option<Attachment>, AttachmentStoreError> {
            self.authorized.lock().unwrap().push((
                reader_id.as_str().to_owned(),
                room_id.as_str().to_owned(),
                attachment_id.as_str().to_owned(),
            ));
            Ok(self.authorized_result.clone())
        }
        async fn referenced_storage_keys(
            &self,
        ) -> Result<std::collections::HashSet<String>, AttachmentStoreError> {
            Ok(std::collections::HashSet::new())
        }
        async fn purge_expired_pending(
            &self,
            _now: crate::domain::time::UtcMillis,
        ) -> Result<Vec<crate::domain::attachment::StorageKey>, AttachmentStoreError> {
            Ok(Vec::new())
        }
    }

    fn sample_attachment() -> Attachment {
        Attachment {
            id: AttachmentId("00000000-0000-4000-8000-000000000001".into()),
            uploader_participant_id: ParticipantId("h-1".into()),
            room_id: RoomId("00000000-0000-4000-8000-000000000002".into()),
            file_name: FileName::parse("a.txt").unwrap(),
            content_type: ContentType::parse("text/plain").unwrap(),
            size_bytes: 5,
            state: crate::domain::attachment::AttachmentState::Bound,
            storage_key: StorageKey("00000000-0000-4000-8000-000000000003".into()),
            sha256_hex: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824".into(),
            uploaded_at: UtcMillis::from_millis(1),
        }
    }

    #[tokio::test]
    async fn upload_validates_metadata_and_limits_before_touching_stores() {
        let blob = RecordingBlob {
            written: std::sync::Mutex::new(Vec::new()),
            deleted: std::sync::Mutex::new(Vec::new()),
            fail_write: false,
            fail_read: false,
        };
        let store = RecordingStore {
            pending: std::sync::Mutex::new(Vec::new()),
            authorized: std::sync::Mutex::new(Vec::new()),
            fail_pending: None,
            authorized_result: None,
        };
        let actor = ParticipantId("h-1".into());
        // (file_name, content_type, bytes, room_raw, 期望变体判定)
        type UploadCase = (
            &'static str,
            &'static str,
            Vec<u8>,
            &'static str,
            fn(&UploadAttachmentError) -> bool,
        );
        let cases: Vec<UploadCase> = vec![
            (
                "",
                "text/plain",
                b"x".to_vec(),
                "00000000-0000-4000-8000-000000000002",
                |e| matches!(e, UploadAttachmentError::InvalidAttachmentMetadata(_)),
            ),
            (
                "a\u{0}b",
                "text/plain",
                b"x".to_vec(),
                "00000000-0000-4000-8000-000000000002",
                |e| matches!(e, UploadAttachmentError::InvalidAttachmentMetadata(_)),
            ),
            (
                "a.txt",
                "",
                b"x".to_vec(),
                "00000000-0000-4000-8000-000000000002",
                |e| matches!(e, UploadAttachmentError::InvalidAttachmentMetadata(_)),
            ),
            (
                "a.txt",
                "text/\u{7f}",
                b"x".to_vec(),
                "00000000-0000-4000-8000-000000000002",
                |e| matches!(e, UploadAttachmentError::InvalidAttachmentMetadata(_)),
            ),
            (
                "a.txt",
                "text/plain",
                vec![0u8; 16 * 1024 * 1024 + 1],
                "00000000-0000-4000-8000-000000000002",
                |e| matches!(e, UploadAttachmentError::PayloadTooLarge),
            ),
            ("a.txt", "text/plain", b"x".to_vec(), "not-a-room", |e| {
                matches!(e, UploadAttachmentError::RoomNotFound)
            }),
        ];
        for (name, ctype, bytes, room, is_expected) in cases {
            let result = upload_attachment(&store, &blob, &actor, room, name, ctype, &bytes).await;
            assert!(
                is_expected(&result.unwrap_err()),
                "{name:?} {ctype:?} must fail with the expected variant"
            );
        }
        assert!(
            store.pending.lock().unwrap().is_empty(),
            "no row before validation"
        );
        assert!(
            blob.written.lock().unwrap().is_empty(),
            "no bytes written before validation"
        );
    }

    #[tokio::test]
    async fn upload_writes_blob_then_row_and_cleans_up_on_row_failure() {
        let blob = RecordingBlob {
            written: std::sync::Mutex::new(Vec::new()),
            deleted: std::sync::Mutex::new(Vec::new()),
            fail_write: false,
            fail_read: false,
        };
        let store = RecordingStore {
            pending: std::sync::Mutex::new(Vec::new()),
            authorized: std::sync::Mutex::new(Vec::new()),
            fail_pending: Some(AttachmentStoreError::RoomNotFound),
            authorized_result: None,
        };
        let actor = ParticipantId("h-1".into());
        let room = "00000000-0000-4000-8000-000000000002";
        let result =
            upload_attachment(&store, &blob, &actor, room, "a.txt", "text/plain", b"hello").await;
        assert!(matches!(result, Err(UploadAttachmentError::RoomNotFound)));
        assert_eq!(blob.written.lock().unwrap().len(), 1, "blob written first");
        assert_eq!(
            blob.deleted.lock().unwrap().len(),
            1,
            "blob removed after row failure"
        );
        let written_key = &blob.written.lock().unwrap()[0].0;
        assert_eq!(&blob.deleted.lock().unwrap()[0], written_key);
        assert_eq!(
            store.pending.lock().unwrap().len(),
            1,
            "pending row insert was attempted once and failed"
        );
    }

    #[tokio::test]
    async fn upload_success_returns_pending_public_facts_only() {
        let blob = RecordingBlob {
            written: std::sync::Mutex::new(Vec::new()),
            deleted: std::sync::Mutex::new(Vec::new()),
            fail_write: false,
            fail_read: false,
        };
        let store = RecordingStore {
            pending: std::sync::Mutex::new(Vec::new()),
            authorized: std::sync::Mutex::new(Vec::new()),
            fail_pending: None,
            authorized_result: None,
        };
        let actor = ParticipantId("h-1".into());
        let room = "00000000-0000-4000-8000-000000000002";
        let outcome =
            upload_attachment(&store, &blob, &actor, room, "a.txt", "text/plain", b"hello")
                .await
                .unwrap();
        assert_eq!(
            outcome.attachment.state,
            crate::domain::attachment::AttachmentState::Pending
        );
        assert_eq!(outcome.attachment.size_bytes, 5);
        assert_eq!(outcome.attachment.file_name.as_str(), "a.txt");
        let row = store.pending.lock().unwrap()[0].clone();
        assert_eq!(row.id, outcome.attachment.id);
        assert_eq!(row.sha256_hex.len(), 64, "sha256 is internal and stored");
        assert_eq!(row.storage_key, outcome.attachment.storage_key);
    }

    #[tokio::test]
    async fn download_maps_none_to_unified_not_found_and_verifies_bytes() {
        let blob = RecordingBlob {
            written: std::sync::Mutex::new(Vec::new()),
            deleted: std::sync::Mutex::new(Vec::new()),
            fail_write: false,
            fail_read: false,
        };
        let store = RecordingStore {
            pending: std::sync::Mutex::new(Vec::new()),
            authorized: std::sync::Mutex::new(Vec::new()),
            fail_pending: None,
            authorized_result: None,
        };
        let actor = ParticipantId("h-1".into());
        let room = "00000000-0000-4000-8000-000000000002";
        let bad_id = "00000000-0000-4000-8000-0000000000ZZ";
        let result = download_attachment(&store, &blob, &actor, room, bad_id).await;
        assert!(matches!(
            result,
            Err(DownloadAttachmentError::InvalidRequest)
        ));
        let result = download_attachment(
            &store,
            &blob,
            &actor,
            room,
            "00000000-0000-4000-8000-000000000001",
        )
        .await;
        assert!(matches!(
            result,
            Err(DownloadAttachmentError::AttachmentNotFound)
        ));

        let blob = RecordingBlob {
            written: std::sync::Mutex::new(vec![(
                "00000000-0000-4000-8000-000000000003".to_owned(),
                b"hello".to_vec(),
            )]),
            deleted: std::sync::Mutex::new(Vec::new()),
            fail_write: false,
            fail_read: false,
        };
        let store = RecordingStore {
            pending: std::sync::Mutex::new(Vec::new()),
            authorized: std::sync::Mutex::new(Vec::new()),
            fail_pending: None,
            authorized_result: Some(sample_attachment()),
        };
        let (meta, bytes) = download_attachment(
            &store,
            &blob,
            &actor,
            room,
            "00000000-0000-4000-8000-000000000001",
        )
        .await
        .unwrap();
        assert_eq!(bytes, b"hello");
        assert_eq!(meta.size_bytes, 5);
        assert_eq!(meta.file_name.as_str(), "a.txt");

        for corrupted in [b"jello".to_vec(), b"hello!".to_vec()] {
            let blob = RecordingBlob {
                written: std::sync::Mutex::new(vec![(
                    "00000000-0000-4000-8000-000000000003".to_owned(),
                    corrupted,
                )]),
                deleted: std::sync::Mutex::new(Vec::new()),
                fail_write: false,
                fail_read: false,
            };
            let store = RecordingStore {
                pending: std::sync::Mutex::new(Vec::new()),
                authorized: std::sync::Mutex::new(Vec::new()),
                fail_pending: None,
                authorized_result: Some(sample_attachment()),
            };
            let err = download_attachment(
                &store,
                &blob,
                &actor,
                room,
                "00000000-0000-4000-8000-000000000001",
            )
            .await
            .expect_err("corrupt attachment bytes must never be returned");
            let DownloadAttachmentError::Store(storage) = err else {
                panic!("corruption must remain an internal store failure");
            };
            assert_eq!(storage.kind(), StorageFailureKind::DecodeOrCorrupt);
        }
    }
}
