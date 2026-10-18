//! 本地文件系统 Blob 存储 adapter：实现 `ports::blob::BlobStorage`。
//!
//! 布局：`<media_root>/attachments/<storage_key>`，storage_key 必须是严格
//! UUID v4（内部事实；用户输入绝不进入路径）。写入原子且 no-clobber：
//! 临时文件用 create_new（O_EXCL）创建、权限 0600，绝不覆盖或 follow
//! 预置的 `<key>.tmp` symlink（同名残留时改用内部随机临时名）；最终
//! 安装用硬链接 temp→final 的 no-clobber 语义（final 已存在 → 稳定错误，
//! 绝不覆盖），随后删除 temp。读取禁止 follow symlink（先 symlink_metadata，
//! 符号链接/非普通文件一律拒绝），且先按 metadata 长度预检、再 take(limit+1)
//! 流式读取：单附件超过 16 MiB 在分配前即返回稳定 DecodeOrCorrupt，绝不把
//! 任意大小文件全部读入内存。读取/删除只接受严格 UUID v4 key；orphan
//! sweep 只处理受控命名空间内、超过 grace period 的普通文件：未被 SQLite
//! 引用的 final 与严格匹配 Core 自有格式的 temp 残留；禁止 follow symlink、
//! 禁止越过媒体根目录、禁止按用户输入
//! 路径删除（契约 CORE_V1_ATTACHMENTS.md 10）。

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::domain::attachment::StorageKey;
use crate::ports::blob::BlobStorage;
use crate::ports::storage::{StorageError, StorageFailureKind};

/// orphan sweep 的生产 grace period：文件 mtime 必须早于该值才会被清理。
/// 与 pending 绑定窗口同量级（24 小时），保证上传「写字节 → 插行」之间
/// 崩溃留下的孤儿文件不会与在途上传竞态。
pub const ORPHAN_SWEEP_GRACE: Duration = Duration::from_secs(86_400);

/// 临时文件后缀：写入先落到 `<key>.tmp`（create_new），成功后以 no-clobber
/// 安装为 `<key>`；崩溃残留的 `.tmp` 不属于 UUID 命名空间，sweep 不处理，
/// 下次同名写入用内部随机临时名避开（绝不覆盖残留）。
const TMP_SUFFIX: &str = ".tmp";

/// 随机临时名分配上限：同名残留/预置 symlink 的极端冲突下尝试次数。
const TMP_RENAME_ATTEMPTS: usize = 64;

fn io_error(e: std::io::Error) -> StorageError {
    StorageError::new(StorageFailureKind::ConnectionOrIo, e)
}

/// sweep 只承认 Core 自己会生成的两类名字：最终 UUID，或
/// `<UUID>.tmp[.<UUID>]` 临时名。其余名称永不触碰。
fn sweep_name(name: &str) -> Option<(StorageKey, bool)> {
    if let Some(key) = StorageKey::parse_canonical(name) {
        return Some((key, false));
    }
    if let Some(base) = name.strip_suffix(TMP_SUFFIX) {
        return StorageKey::parse_canonical(base).map(|key| (key, true));
    }
    let (base, nonce) = name.split_once(&format!("{TMP_SUFFIX}."))?;
    let key = StorageKey::parse_canonical(base)?;
    uuid::Uuid::parse_str(nonce)
        .ok()
        .filter(|uuid| uuid.get_version_num() == 4 && uuid.to_string() == nonce)
        .map(|_| (key, true))
}

/// 基于本地目录的 Blob 存储。`dir` 必须是 Core 持有的附件目录
/// （`<media_root>/attachments`），由 composition root 在启动时确保存在。
#[derive(Debug, Clone)]
pub struct LocalBlobStorage {
    dir: PathBuf,
}

impl LocalBlobStorage {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path_for(&self, key: &StorageKey) -> PathBuf {
        self.dir.join(key.as_str())
    }

    fn tmp_path_for(&self, key: &StorageKey) -> PathBuf {
        self.dir.join(format!("{}{TMP_SUFFIX}", key.as_str()))
    }

    /// 以 create_new（O_EXCL）打开一个全新临时文件：优先确定性名
    /// `<key>.tmp`；已被占用（崩溃残留、预置 symlink 等，绝不 follow/
    /// 覆盖）时改用内部随机名 `<key>.tmp.<uuid>` 重试。所有名字都在受控
    /// 命名空间内，且任何一次 O_EXCL 失败都只换名重试，绝不触碰既有文件。
    async fn open_fresh_tmp(
        &self,
        key: &StorageKey,
    ) -> Result<(PathBuf, tokio::fs::File), StorageError> {
        let mut opts = tokio::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        opts.mode(0o600);
        let primary = self.tmp_path_for(key);
        match opts.open(&primary).await {
            Ok(file) => return Ok((primary, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io_error(e)),
        }
        for _ in 0..TMP_RENAME_ATTEMPTS {
            let name = format!("{}{TMP_SUFFIX}.{}", key.as_str(), uuid::Uuid::new_v4());
            let path = self.dir.join(&name);
            match opts.open(&path).await {
                Ok(file) => return Ok((path, file)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(io_error(e)),
            }
        }
        Err(StorageError::from_message(
            StorageFailureKind::ConnectionOrIo,
            "blob write could not allocate a fresh temporary file",
        ))
    }

    /// 固化附件目录中的目录项变更。Linux/Unix 上目录 fd 的 sync_all
    /// 使 final 安装和 temp 删除进入文件系统的持久化边界；其他平台没有
    /// 可移植的目录 fsync 语义，只能依赖文件同步和平台自身的目录保证。
    async fn sync_directory(&self) -> Result<(), StorageError> {
        #[cfg(unix)]
        {
            tokio::fs::File::open(&self.dir)
                .await
                .map_err(io_error)?
                .sync_all()
                .await
                .map_err(io_error)?;
        }
        #[cfg(not(unix))]
        {
            // Windows 没有与 Unix 目录 fd fsync 对等的可移植 tokio API。
        }
        Ok(())
    }

    async fn remove_owned_tmp(&self, tmp: &std::path::Path) {
        let _ = tokio::fs::remove_file(tmp).await;
    }
}

#[async_trait::async_trait]
impl BlobStorage for LocalBlobStorage {
    async fn write(&self, key: &StorageKey, bytes: &[u8]) -> Result<(), StorageError> {
        // 防御：只接受严格 UUID v4 key（用户输入绝不进入路径）。
        let _ = StorageKey::parse_canonical(key.as_str()).ok_or_else(|| {
            StorageError::from_message(
                StorageFailureKind::Unknown,
                "blob write rejected non-canonical storage key",
            )
        })?;
        tokio::fs::create_dir_all(&self.dir)
            .await
            .map_err(io_error)?;
        let final_path = self.path_for(key);
        // 临时文件必须 create_new（O_EXCL）：绝不覆盖既有 `<key>.tmp`、
        // 绝不 follow 预置 symlink（O_EXCL 对已存在的 symlink 本身返回
        // EEXIST）；确定性临时名被占用（崩溃残留/攻击）时改用内部随机
        // 名（同样 O_EXCL，绝不覆盖任何既有名字）。
        let (tmp, mut file) = self.open_fresh_tmp(key).await?;
        if let Err(error) = file.write_all(bytes).await {
            drop(file);
            self.remove_owned_tmp(&tmp).await;
            return Err(io_error(error));
        }
        if let Err(error) = file.sync_all().await {
            drop(file);
            self.remove_owned_tmp(&tmp).await;
            return Err(io_error(error));
        }
        drop(file);
        // final 安装 no-clobber：硬链接 temp→final（final 已存在，含预置
        // symlink，返回 EEXIST → 稳定错误，绝不覆盖/替换），成功后删除
        // temp；temp 与 final 同目录同文件系统，链接原子成立。
        match tokio::fs::hard_link(&tmp, &final_path).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                self.remove_owned_tmp(&tmp).await;
                return Err(StorageError::from_message(
                    StorageFailureKind::Unknown,
                    "blob write refused: destination storage key already exists",
                ));
            }
            Err(e) => {
                self.remove_owned_tmp(&tmp).await;
                return Err(io_error(e));
            }
        }
        // final 的目录项先单独进入持久化边界；这一步成功后，附件字节即使
        // temp 清理失败也已可靠提交，不能再向调用方翻转成失败。
        if let Err(error) = self.sync_directory().await {
            // 尚未证明 final 目录项耐久，尽力回滚当前可见的两个名字；回滚
            // 自身失败仍保留原始 sync 错误，残留由 orphan 维护链兜底。
            let _ = tokio::fs::remove_file(&final_path).await;
            self.remove_owned_tmp(&tmp).await;
            let _ = self.sync_directory().await;
            return Err(error);
        }
        // temp 只是安装过程私有名字。final 已耐久后，其删除/删除目录项同步
        // 失败只留下无害残留，不能把成功写入误报为失败；未来写入会改用
        // 内部随机 temp 名，残留不覆盖也不阻塞该附件事实。
        if tokio::fs::remove_file(&tmp).await.is_ok() {
            let _ = self.sync_directory().await;
        }
        Ok(())
    }

    async fn read(&self, key: &StorageKey) -> Result<Vec<u8>, StorageError> {
        let _ = StorageKey::parse_canonical(key.as_str()).ok_or_else(|| {
            StorageError::from_message(
                StorageFailureKind::Unknown,
                "blob read rejected non-canonical storage key",
            )
        })?;
        let path = self.path_for(key);
        // 下载禁止 follow symlink：先 symlink_metadata（不 follow）检查，
        // 预置/被替换为符号链接或非普通文件的 key 一律拒绝——绝不读取
        // 受控附件目录之外的内容。读取/检查之间的 TOCTOU 不能由这次预检
        // 消除；当前边界依赖附件目录仅由 Core 写入。若未来允许不可信本地
        // 写者，必须改用 O_NOFOLLOW / fd-based 安全打开。
        let meta = tokio::fs::symlink_metadata(&path).await.map_err(io_error)?;
        if meta.file_type().is_symlink() || !meta.file_type().is_file() {
            return Err(StorageError::from_message(
                StorageFailureKind::DecodeOrCorrupt,
                "stored blob is not a regular file",
            ));
        }
        // 长度预检：先于任何分配拒绝超限文件（单附件 ≤ 16 MiB），
        // 绝不把任意大小文件全部读入内存。
        if meta.len() > crate::domain::attachment::MAX_ATTACHMENT_BYTES as u64 {
            return Err(StorageError::from_message(
                StorageFailureKind::DecodeOrCorrupt,
                "stored blob exceeds the attachment size bound",
            ));
        }
        let file = tokio::fs::File::open(&path).await.map_err(io_error)?;
        // 流式读取并保留硬上限：take(limit+1) 保证即使文件在预检后被
        // 替换/增长，也绝不分配超过上限的内存；读到第 limit+1 字节即
        // 判定超限。capacity 只按预检长度申请，不超过上限。
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        file.take(crate::domain::attachment::MAX_ATTACHMENT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(io_error)?;
        if bytes.len() as u64 > crate::domain::attachment::MAX_ATTACHMENT_BYTES as u64 {
            return Err(StorageError::from_message(
                StorageFailureKind::DecodeOrCorrupt,
                "stored blob exceeds the attachment size bound",
            ));
        }
        Ok(bytes)
    }

    async fn delete(&self, key: &StorageKey) -> Result<(), StorageError> {
        let _ = StorageKey::parse_canonical(key.as_str()).ok_or_else(|| {
            StorageError::from_message(
                StorageFailureKind::Unknown,
                "blob delete rejected non-canonical storage key",
            )
        })?;
        match tokio::fs::remove_file(self.path_for(key)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_error(e)),
        }
    }

    async fn sweep_orphans(
        &self,
        referenced: &HashSet<String>,
        grace: Duration,
    ) -> Result<(), StorageError> {
        // 目录不存在视为空（首次启动），其余读取失败返回稳定分类（调用方
        // 只记录脱敏 warning，不阻塞启动）。
        let mut entries = match tokio::fs::read_dir(&self.dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(io_error(e)),
        };
        let cutoff = std::time::SystemTime::now()
            .checked_sub(grace)
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        while let Some(entry) = entries.next_entry().await.map_err(io_error)? {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            // 只处理受控命名空间：严格 UUID v4 final，或 Core 自己生成的
            // `<UUID>.tmp[.<UUID>]` 临时名；其他文件一律跳过。
            let Some((key, is_tmp)) = sweep_name(name) else {
                continue;
            };
            // final 被 SQLite pending/bound 行引用时绝不清理。temp 只是安装
            // 过程的第二个名字，不是公共/持久事实；grace 保护新鲜写入。
            if !is_tmp && referenced.contains(name) {
                continue;
            }
            // symlink_metadata 不 follow symlink：符号链接一律跳过（禁止
            // follow、禁止越过根目录删除目标）。
            let meta = match tokio::fs::symlink_metadata(entry.path()).await {
                Ok(meta) => meta,
                Err(_) => {
                    tracing::warn!(
                        operation = "blob.sweep.stat",
                        error_kind = "connection_or_io",
                        storage_key = %key,
                        "orphan sweep could not stat attachment file; skipped"
                    );
                    continue;
                }
            };
            if meta.file_type().is_symlink() || !meta.file_type().is_file() {
                continue;
            }
            let modified = match meta.modified() {
                Ok(modified) => modified,
                Err(_) => continue,
            };
            if modified > cutoff {
                continue;
            }
            match tokio::fs::remove_file(entry.path()).await {
                Ok(()) => {
                    tracing::info!(
                        operation = "blob.sweep.remove",
                        storage_key = %key,
                        temporary = is_tmp,
                        "orphan attachment file removed"
                    );
                }
                Err(_) => {
                    // 清理失败只记录脱敏 warning（只含 storage key 与稳定
                    // 分类），不阻塞启动。
                    tracing::warn!(
                        operation = "blob.sweep.remove",
                        error_kind = "connection_or_io",
                        storage_key = %key,
                        temporary = is_tmp,
                        "orphan sweep failed to remove attachment file; maintenance continues"
                    );
                }
            }
        }
        Ok(())
    }
}

/// 冒烟测试（真实文件系统）：原子写入/读取/删除 round-trip、key 校验、
/// sweep 的引用保护 / grace 保护 / symlink 跳过 / 命名空间外跳过 /
/// 路径穿越（../）拒绝。
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn storage() -> (tempfile::TempDir, LocalBlobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let blob = LocalBlobStorage::new(dir.path().join("attachments"));
        (dir, blob)
    }

    #[tokio::test]
    async fn write_read_delete_round_trip_with_atomic_rename_and_0600() {
        let (_dir, blob) = storage();
        let key = StorageKey::new_v4();
        let payload = b"hello attachment bytes";
        blob.write(&key, payload).await.unwrap();
        assert_eq!(blob.read(&key).await.unwrap(), payload.to_vec());
        // 无 .tmp 残留（原子 rename 完成）。
        let tmp = blob.tmp_path_for(&key);
        assert!(!tmp.exists(), "tmp file must be renamed away");
        // 权限 0600（unix）。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(blob.path_for(&key)).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        }
        blob.delete(&key).await.unwrap();
        assert!(!blob.path_for(&key).exists());
        // 幂等删除。
        blob.delete(&key).await.unwrap();
    }

    #[tokio::test]
    async fn non_canonical_keys_are_rejected_for_every_operation() {
        let (_dir, blob) = storage();
        for bad in ["../evil", "a/b", "not-a-uuid", "KEY", ""] {
            assert!(StorageKey::parse_canonical(bad).is_none());
        }
        let bad = StorageKey("not-a-uuid".into());
        assert!(blob.write(&bad, b"x").await.is_err());
        assert!(blob.read(&bad).await.is_err());
        assert!(blob.delete(&bad).await.is_err());
    }

    #[tokio::test]
    async fn sweep_removes_only_unreferenced_aged_orphans_within_namespace() {
        let (dir, blob) = storage();
        let aged = StorageKey::new_v4();
        let referenced = StorageKey::new_v4();
        let fresh = StorageKey::new_v4();
        let tmp_name = format!("{}.tmp", StorageKey::new_v4().as_str());
        let random_tmp_name = format!(
            "{}.tmp.{}",
            StorageKey::new_v4().as_str(),
            uuid::Uuid::new_v4()
        );
        let fresh_tmp_name = format!("{}.tmp", StorageKey::new_v4().as_str());
        let malformed_tmp_name = format!("{}.tmp.not-a-v4", StorageKey::new_v4().as_str());
        let out_of_namespace = "operator-note.txt";

        blob.write(&aged, b"orphan").await.unwrap();
        blob.write(&referenced, b"kept").await.unwrap();
        blob.write(&fresh, b"kept-fresh").await.unwrap();
        std::fs::write(blob.dir.join(&tmp_name), b"tmp").unwrap();
        std::fs::write(blob.dir.join(&random_tmp_name), b"tmp-random").unwrap();
        std::fs::write(blob.dir.join(&fresh_tmp_name), b"tmp-fresh").unwrap();
        std::fs::write(blob.dir.join(&malformed_tmp_name), b"tmp-malformed").unwrap();
        std::fs::write(blob.dir.join(out_of_namespace), b"other").unwrap();

        // 把 aged 文件的 mtime 老化到 grace 之前；其他文件保持新鲜。
        let past = std::time::SystemTime::now() - Duration::from_secs(3600);
        let past_secs = past
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let file = std::fs::File::options()
            .write(true)
            .open(blob.path_for(&aged))
            .unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(past))
            .unwrap();
        drop(file);
        for name in [&tmp_name, &random_tmp_name] {
            std::fs::File::options()
                .write(true)
                .open(blob.dir.join(name))
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(past))
                .unwrap();
        }
        let _ = past_secs;

        let referenced_set: HashSet<String> = HashSet::from([referenced.as_str().to_owned()]);
        blob.sweep_orphans(&referenced_set, Duration::from_secs(1800))
            .await
            .unwrap();

        assert!(
            !blob.path_for(&aged).exists(),
            "aged unreferenced orphan must be swept"
        );
        assert!(
            blob.path_for(&referenced).exists(),
            "referenced file must be kept"
        );
        assert!(
            blob.path_for(&fresh).exists(),
            "fresh file within grace must be kept"
        );
        assert!(
            !blob.dir.join(&tmp_name).exists(),
            "aged deterministic Core temp must be swept"
        );
        assert!(
            !blob.dir.join(&random_tmp_name).exists(),
            "aged randomized Core temp must be swept"
        );
        assert!(
            blob.dir.join(&fresh_tmp_name).exists(),
            "fresh Core temp must remain protected by grace"
        );
        assert!(
            blob.dir.join(&malformed_tmp_name).exists(),
            "malformed temp-like names must stay outside the controlled namespace"
        );
        assert!(
            blob.dir.join(out_of_namespace).exists(),
            "files outside the strict namespace must be kept"
        );
        let _ = dir;
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn sweep_never_follows_symlinks() {
        let (dir, blob) = storage();
        // 符号链接指向目录外的敏感文件：sweep 必须跳过（不 follow、不
        // 删除链接、不触碰目标）。
        let outside = dir.path().join("outside-secret.txt");
        std::fs::write(&outside, b"secret").unwrap();
        std::fs::create_dir_all(&blob.dir).unwrap();
        // 名称本身严格伪装成 Core temp；即便能通过命名空间解析，类型守卫
        // 仍必须跳过 symlink，不能只靠“未知名称”规避测试。
        let link_name = format!("{}.tmp", StorageKey::new_v4().as_str());
        std::os::unix::fs::symlink(&outside, blob.dir.join(&link_name)).unwrap();

        blob.sweep_orphans(&HashSet::new(), Duration::ZERO)
            .await
            .unwrap();
        assert!(
            blob.dir.join(&link_name).exists(),
            "symlinks must never be removed"
        );
        assert!(
            outside.exists(),
            "sweep must never follow a symlink to delete its target"
        );
    }

    #[tokio::test]
    async fn sweep_treats_missing_directory_as_empty() {
        let (dir, blob) = storage();
        // 目录不存在（未创建）→ 空。
        assert!(!blob.dir.exists());
        blob.sweep_orphans(&HashSet::new(), Duration::from_secs(1))
            .await
            .unwrap();
        let _ = dir;
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn write_never_follows_preseeded_tmp_symlink_and_uses_fresh_temp_name() {
        let (dir, blob) = storage();
        let key = StorageKey::new_v4();
        // 预置 `<key>.tmp` symlink 指向目录外敏感文件：写入必须不 follow、
        // 不覆盖、不删除该链接，改用内部随机临时名完成落位。
        let outside = dir.path().join("outside-secret.txt");
        std::fs::write(&outside, b"secret").unwrap();
        std::fs::create_dir_all(&blob.dir).unwrap();
        std::os::unix::fs::symlink(&outside, blob.tmp_path_for(&key)).unwrap();

        blob.write(&key, b"payload").await.unwrap();

        assert_eq!(
            std::fs::read(&outside).unwrap(),
            b"secret",
            "sensitive file outside the media dir must never be touched"
        );
        assert_eq!(
            blob.read(&key).await.unwrap(),
            b"payload",
            "final blob must contain the written bytes"
        );
        let meta = std::fs::symlink_metadata(blob.tmp_path_for(&key)).unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "preseeded tmp symlink must stay untouched"
        );
        // 目录内除 final 与预置 symlink 外无任何 .tmp 残留（随机临时名也已
        // 在安装后移除）。
        let names: Vec<String> = std::fs::read_dir(&blob.dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        for name in names {
            assert!(
                name == key.as_str() || name == format!("{}{TMP_SUFFIX}", key.as_str()),
                "unexpected leftover entry {name:?}"
            );
        }
    }

    #[tokio::test]
    async fn write_skips_leftover_tmp_file_without_touching_it() {
        let (_dir, blob) = storage();
        let key = StorageKey::new_v4();
        // 崩溃残留的 `<key>.tmp` 普通文件：写入必须避开（不覆盖、不删除），
        // 使用随机临时名完成。
        std::fs::create_dir_all(&blob.dir).unwrap();
        std::fs::write(blob.tmp_path_for(&key), b"leftover").unwrap();

        blob.write(&key, b"payload").await.unwrap();

        assert_eq!(
            std::fs::read(blob.tmp_path_for(&key)).unwrap(),
            b"leftover",
            "leftover tmp file must never be overwritten or removed"
        );
        assert_eq!(blob.read(&key).await.unwrap(), b"payload");
    }

    #[tokio::test]
    async fn write_never_overwrites_existing_final() {
        let (_dir, blob) = storage();
        let key = StorageKey::new_v4();
        blob.write(&key, b"first").await.unwrap();
        // 第二次写同一 key：final 已存在 → 稳定错误（kind + 固定消息），
        // 内容不变、无 .tmp 残留。
        let err = blob.write(&key, b"second").await.unwrap_err();
        assert_eq!(
            err.kind(),
            StorageFailureKind::Unknown,
            "no-clobber refusal must be a stable, deterministic error"
        );
        assert_eq!(blob.read(&key).await.unwrap(), b"first");
        assert!(
            !blob.tmp_path_for(&key).exists(),
            "failed no-clobber write must not leave a temp file behind"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn write_never_replaces_preseeded_final_symlink() {
        let (dir, blob) = storage();
        let key = StorageKey::new_v4();
        // final 位置被预置 symlink（指向目录外敏感文件）：write 必须稳定
        // 拒绝，绝不覆盖链接、绝不触碰目标。
        let outside = dir.path().join("outside-secret-2.txt");
        std::fs::write(&outside, b"secret").unwrap();
        std::fs::create_dir_all(&blob.dir).unwrap();
        std::os::unix::fs::symlink(&outside, blob.path_for(&key)).unwrap();

        let err = blob.write(&key, b"payload").await.unwrap_err();
        assert_eq!(err.kind(), StorageFailureKind::Unknown);
        assert_eq!(std::fs::read(&outside).unwrap(), b"secret");
        assert!(
            std::fs::symlink_metadata(blob.path_for(&key))
                .unwrap()
                .file_type()
                .is_symlink(),
            "preseeded final symlink must not be replaced"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn read_never_follows_preseeded_symlink_to_outside_sensitive_file() {
        let (dir, blob) = storage();
        let key = StorageKey::new_v4();
        // 最终位置被预置 symlink（指向目录外的敏感文件）：read 必须拒绝，
        // 绝不 follow 读取敏感内容、绝不触碰链接与目标。
        let outside = dir.path().join("outside-secret-3.txt");
        std::fs::write(&outside, b"top secret payload").unwrap();
        std::fs::create_dir_all(&blob.dir).unwrap();
        std::os::unix::fs::symlink(&outside, blob.path_for(&key)).unwrap();

        let err = blob.read(&key).await.unwrap_err();
        assert_eq!(
            err.kind(),
            StorageFailureKind::DecodeOrCorrupt,
            "symlink must be rejected as a stable internal invariant"
        );
        assert_eq!(
            std::fs::read(&outside).unwrap(),
            b"top secret payload",
            "sensitive file outside the media dir must never be read"
        );
        assert!(
            std::fs::symlink_metadata(blob.path_for(&key))
                .unwrap()
                .file_type()
                .is_symlink(),
            "preseeded symlink must stay untouched"
        );
    }

    #[tokio::test]
    async fn read_rejects_oversized_blob_without_loading_full_content() {
        let (dir, blob) = storage();
        let key = StorageKey::new_v4();
        // 预置超过 16 MiB 上限的普通文件（稀疏文件：只记录长度，不实际
        // 占用磁盘块）。read 必须失败，且不得把完整内容载入内存（长度
        // 预检 + take(limit+1) 硬上限）。
        std::fs::create_dir_all(&blob.dir).unwrap();
        let file = std::fs::File::create(blob.path_for(&key)).unwrap();
        file.set_len(crate::domain::attachment::MAX_ATTACHMENT_BYTES as u64 + 1)
            .unwrap();
        drop(file);

        let err = blob.read(&key).await.unwrap_err();
        assert_eq!(
            err.kind(),
            StorageFailureKind::DecodeOrCorrupt,
            "oversized blob must fail with the stable size-bound error"
        );
        assert_eq!(
            std::fs::metadata(blob.path_for(&key)).unwrap().len(),
            crate::domain::attachment::MAX_ATTACHMENT_BYTES as u64 + 1,
            "oversized file must not be modified or truncated"
        );
        let _ = dir;
    }

    #[test]
    fn file_name_never_enters_paths() {
        // 路径构造只由 storage key 决定：即使 file_name 是穿越字符串，
        // 磁盘路径也不包含它（路径构造由 adapter 内部完成，用户输入只
        // 留在 SQLite 元数据）。
        let (_dir, blob) = storage();
        let key = StorageKey::new_v4();
        let path = blob.path_for(&key);
        let p = path.to_string_lossy();
        assert!(!p.contains("..") && !p.contains("secret"));
        assert!(path.ends_with(key.as_str()));
        let _ = std::io::stdout().write_all(b"");
    }
}
