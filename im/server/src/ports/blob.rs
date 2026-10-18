//! Blob 字节存储窄端口。
//!
//! 本端口只声明「附件字节的写入/读取/删除/孤儿清理」边界，不依赖具体
//! 文件系统布局、SQLite 或任何外部供应商；SQLite 侧的附件元数据由
//! `ports::attachment::AttachmentStore` 负责，本端口不接触数据库。
//!
//! 安全边界（契约 CORE_V1_ATTACHMENTS.md 10）：
//! - key 必须是严格 UUID v4（内部 storage key），实现必须拒绝其他任何
//!   名称——用户输入（file_name 等）绝不进入路径；
//! - 写入必须原子且 no-clobber：临时文件用 create_new（O_EXCL）创建
//!   （绝不 follow 预置 symlink、绝不覆盖既有残留；同名残留时改用内部
//!   随机临时名），最终安装以 no-clobber 语义落位（final 已存在 → 稳定
//!   错误，绝不覆盖），权限最小化；
//! - orphan sweep 只处理受控命名空间内、超过 grace period 的普通文件：
//!   未被 `referenced` 的 final UUID，以及严格匹配 Core 自有格式的 temp 残留；
//!   禁止 follow symlink、禁止越过根目录、
//!   禁止按用户输入路径删除；失败只记录脱敏 warning，不阻塞启动。

use std::collections::HashSet;
use std::time::Duration;

use crate::domain::attachment::StorageKey;
use crate::ports::storage::StorageError;

/// Blob 字节存储端口（窄接口，不扩成万能 repository）。
#[async_trait::async_trait]
pub trait BlobStorage: Send + Sync {
    /// 原子写入 `key` 的字节：实现必须先在受控目录内以 create_new 写临时
    /// 文件（绝不覆盖或 follow 预置 `<key>.tmp`；同名残留时改用内部随机
    /// 临时名），再以 no-clobber 语义安装到 `<key>`——目标已存在（含被
    /// 预置的 symlink）必须返回稳定错误而不是覆盖。写入失败不得留下可被
    /// 后续调用误用的半成品；key 非严格 UUID v4 一律拒绝。
    async fn write(&self, key: &StorageKey, bytes: &[u8]) -> Result<(), StorageError>;

    /// 读取 `key` 的字节（单附件 ≤ 16 MiB，可整体读取）；key 不存在或
    /// 非法返回 `ConnectionOrIo` 分类错误。
    async fn read(&self, key: &StorageKey) -> Result<Vec<u8>, StorageError>;

    /// 删除 `key` 的字节（上传失败后的 best-effort 清理；不存在视为成功）。
    async fn delete(&self, key: &StorageKey) -> Result<(), StorageError>;

    /// 统一维护链 orphan sweep（best-effort）：删除受控命名空间内超过
    /// `grace` 的普通文件——名称严格匹配 UUID v4 且不在 `referenced`
    /// 集合中的 final，或严格匹配 `<UUID>.tmp[.<UUID>]` 的 Core temp 残留。
    /// 符号链接与命名空间外文件一律跳过（不 follow、不删除）。逐文件失败
    /// 只记录脱敏 warning 并继续；灾难性失败（无法读取目录，目录不存在
    /// 视为空）返回 Err，调用方只记录脱敏 warning，启动/运行期维护均继续。
    async fn sweep_orphans(
        &self,
        referenced: &HashSet<String>,
        grace: Duration,
    ) -> Result<(), StorageError>;
}
