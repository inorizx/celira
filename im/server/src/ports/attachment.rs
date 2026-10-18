//! Attachment 元数据窄端口（SQLite 侧）。
//!
//! 本端口只声明附件切片的持久化边界，不依赖具体数据库、HTTP 框架或文件
//! 系统。SQLite adapter 实现本端口；应用用例只通过端口消费存储能力。
//!
//! 附件绑定（Message POST / Completion reply 的同一事务绑定）不经过本
//! 端口：绑定发生在 `ports::message` / `ports::completion` 的提交事务内
//! （message_kernel 的附件校验与绑定插入）。本端口只提供：
//! - `create_pending`：上传成功后建立 pending 附件行（Room / Participant /
//!   Membership 必须 active）；
//! - `authorized_attachment`：下载授权裁决（返回附件事实或 None = 统一
//!   404 attachment_not_found）；
//! - `purge_expired_pending`：内部受控清理——回收「过期（uploaded_at +
//!   24h <= now）、state=pending、无绑定行」的私有暂存租约行，返回被删除
//!   行的内部 storage key（调用方随后删除对应字节）；
//! - `referenced_storage_keys`：统一附件维护链的 orphan sweep 用（purge 后仍存在的
//!   全部 pending/bound 行的 storage key 集合，sweep 只清理不被引用的
//!   孤儿文件）。
//!
//! Blob 字节存储由独立窄端口 `ports::blob::BlobStorage` 提供；本端口不
//! 接触文件系统，BlobStorage 不接触 SQLite。

use std::collections::HashSet;

use crate::domain::attachment::{Attachment, AttachmentId, NewAttachment, StorageKey};
use crate::domain::participant::ParticipantId;
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::ports::storage::StorageError;

/// 存储层错误。
#[derive(Debug)]
pub enum AttachmentStoreError {
    /// bad room、Room 非 active、uploader Participant 非 active 或该 Room
    /// 无 active Membership（上传路径）：对外统一 404 room_not_found。
    RoomNotFound,
    /// 数据库或其他持久化失败（typed cause + 稳定分类）；协议错误与存储
    /// 错误一律不泄露 SQL、路径、storage key、hash 或其他附件事实。
    Database(StorageError),
}

impl std::fmt::Display for AttachmentStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RoomNotFound => write!(f, "no such active room membership"),
            Self::Database(e) => write!(f, "attachment store failure: {e}"),
        }
    }
}

impl std::error::Error for AttachmentStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(e) => Some(e),
            _ => None,
        }
    }
}

/// 附件存储端口（窄接口，不扩成万能 repository）。
#[async_trait::async_trait]
pub trait AttachmentStore: Send + Sync {
    /// 建立一条 pending 附件行。uploader 必须在该 Room 有 active Membership
    /// 周期（与 Message 写入同一谓词）；失败返回 `RoomNotFound`。字节写入
    /// 由调用方先经 BlobStorage 完成（行插入失败时调用方负责 best-effort
    /// 清理字节）。id / storage_key 由调用方生成（Core 分配）。
    async fn create_pending(&self, attachment: &NewAttachment) -> Result<(), AttachmentStoreError>;

    /// 下载授权裁决：返回 `Some(附件事实)` 当且仅当——附件存在且 state =
    /// bound、绑定 Message 与该附件同 Room 且 uploader == Message actor、
    /// Message 未撤回、读取者当前有该 Room 的 active Membership 周期且该
    /// Message 落在读取者任一历史 Membership 周期窗口内（与 History 同一
    /// 可见性谓词）。其余（pending / unknown / 跨 Room / 跨 actor /
    /// 撤回 / 非成员）一律返回 None，调用方统一 404 attachment_not_found，
    /// 不泄露存在性或原因。
    async fn authorized_attachment(
        &self,
        reader_id: &ParticipantId,
        room_id: &RoomId,
        attachment_id: &AttachmentId,
    ) -> Result<Option<Attachment>, AttachmentStoreError>;

    /// 内部受控清理（pending 私有暂存租约到期回收）：在一个
    /// `BEGIN IMMEDIATE` 单写事务内原子裁决并删除全部满足「state =
    /// pending、uploaded_at + 24h <= now、且不存在任何 message_attachments
    /// 绑定行」的附件行，返回被删除行的内部 storage key（调用方随后
    /// best-effort 删除对应字节，绝不向公共协议暴露）。未过期 pending、
    /// bound 或存在绑定行的附件一律不删除（数据库 DELETE 守卫只放行该
    /// 条件）；绑定事务与清理事务并发时由 SQLite 写锁线性化——先绑定则
    /// 清理不可见（已 bound/有绑定），先清理则绑定整体 404 回滚，绝不
    /// 误删；任何失败整体回滚。
    async fn purge_expired_pending(
        &self,
        now: UtcMillis,
    ) -> Result<Vec<StorageKey>, AttachmentStoreError>;

    /// 全部剩余 SQLite pending/bound 附件行的 storage key 集合（统一附件
    /// 维护链的 orphan sweep 引用集合；调用方应先执行 `purge_expired_pending`
    /// 再读取本集合，被引用的文件绝不清理，过期 pending 绝不被保护到
    /// 永远）。
    async fn referenced_storage_keys(&self) -> Result<HashSet<String>, AttachmentStoreError>;
}
