//! 窄基础设施错误值对象：稳定分类 + typed cause。
//!
//! 本模块只表达「存储失败」这一基础设施事实的稳定分类与因果链，不依赖
//! HTTP/Axum、SQLx 或任何具体数据库类型（分类职责在 SQLite adapter，
//! 见 `adapters::sqlite`）。它不是万能 Repository 错误：各 Store 只在自己
//! 的 `Database` 变体中持有它，业务变体保持不变。
//!
//! 安全边界：`Display` 只输出稳定分类名称，永不包含 cause 文本；完整的
//! typed cause 链通过 `Error::source()` 与 `cause()` 保留给结构化日志与
//! 诊断，测试可沿链下钻识别原始底层错误。
//!
//! 显式事务的双因果：primary（业务写入/commit 失败）在 `Error::source()`
//! 主链上保持原样；rollback 失败以独立只读 accessor（`rollback_kind()` /
//! `rollback_cause()` / `has_rollback_failure()`）保留，绝不覆盖 primary，
//! 也绝不进入 `Display`（对外映射只按 `kind()` 裁决，HTTP 因此天然安全）。

use std::error::Error;
use std::fmt;
use std::sync::Arc;

/// 稳定基础设施失败类别（CORE_V1_ERROR_HANDLING.md §7.1）。
///
/// 对外公开映射规则：`DecodeOrCorrupt`、`Unknown` 与 `Constraint` →
/// `internal_error`（500，确定性内部不变量，禁止可重试 503）；只有
/// `BusyOrLocked` / `ConnectionOrIo` → 运行时存储暂时不可用（503
/// `storage_unavailable`）。`Constraint` 只有在存储层已确认对应既有业务
/// 冲突（如 `AlreadyBootstrapped`）被应用层吸收时才映射为业务 code。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StorageFailureKind {
    /// 唯一键、外键、CHECK 等约束裁决失败。
    Constraint,
    /// SQLite 锁竞争或忙超时。
    BusyOrLocked,
    /// 连接、文件系统或读写不可用。
    ConnectionOrIo,
    /// 持久化行无法解析：枚举/状态/JSON 解析失败、结构不一致或迁移
    /// checksum 不匹配。
    DecodeOrCorrupt,
    /// 未分类数据库错误。
    Unknown,
}

impl StorageFailureKind {
    /// 稳定内部名称（进入结构化日志与 Display，不进入公开协议文案）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Constraint => "constraint",
            Self::BusyOrLocked => "busy_or_locked",
            Self::ConnectionOrIo => "connection_or_io",
            Self::DecodeOrCorrupt => "decode_or_corrupt",
            Self::Unknown => "unknown",
        }
    }
}

/// 携带 typed cause 的窄基础设施错误。
///
/// - `kind()`：安全分类访问，HTTP adapter 只按它决定 `storage_unavailable`
///   与 `internal_error` 的取舍；
/// - `Display`：只含稳定分类，绝不含 SQL、路径、绑定值或 cause 文本；
/// - `Error::source()`：暴露 primary cause，供结构化日志与测试沿因果链下钻
///   （`downcast_ref` 可还原 sqlx::Error 等底层类型）；
/// - 显式事务的 rollback 失败由 `with_rollback` 复合：primary 链不被覆盖，
///   rollback 分类/cause 通过只读 accessor 单独识别，Display 与 HTTP 映射
///   仍只看 primary `kind()`（CORE_V1_ERROR_HANDLING.md §7.2）。
#[derive(Debug, Clone)]
pub struct StorageError {
    kind: StorageFailureKind,
    cause: Arc<dyn Error + Send + Sync>,
    rollback: Option<RollbackFailure>,
}

/// 显式 rollback 失败（仅内部诊断）：与 primary 分开保留，永不进入
/// `Display` 或公开协议。`cause` 只被测试 accessor 使用——生产日志与协议
/// 边界只按 `rollback_kind()` 记录安全分类，不输出底层文本。
#[derive(Debug, Clone)]
pub struct RollbackFailure {
    kind: StorageFailureKind,
    #[cfg_attr(not(test), allow(dead_code))]
    cause: Arc<dyn Error + Send + Sync>,
}

impl StorageError {
    /// 以 typed cause 构造：cause 必须实现 `Error`（如 `sqlx::Error`），
    /// 原始类型通过 `downcast_ref` 可恢复。
    pub fn new(kind: StorageFailureKind, cause: impl Error + Send + Sync + 'static) -> Self {
        Self {
            kind,
            cause: Arc::new(cause),
            rollback: None,
        }
    }

    /// 只持有内部诊断消息的 cause（行解析/枚举失败等没有独立底层错误时
    /// 使用）；消息仅作内部诊断，永不进入 `Display` 或公开协议。
    pub fn from_message(kind: StorageFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            cause: Arc::new(StorageMessage(message.into())),
            rollback: None,
        }
    }

    /// 稳定分类访问（HTTP 映射与结构化日志专用）。
    pub fn kind(&self) -> StorageFailureKind {
        self.kind
    }

    /// 复合显式 rollback 失败：primary 主链保持不变，rollback 的分类与
    /// cause 独立保留；`Display` 仍只输出 primary 稳定分类。
    pub fn with_rollback(mut self, rollback: StorageError) -> Self {
        self.rollback = Some(RollbackFailure {
            kind: rollback.kind,
            cause: rollback.cause,
        });
        self
    }

    /// 是否携带 rollback 复合失败（结构化日志 `rollback_failed=true` 专用）。
    pub fn has_rollback_failure(&self) -> bool {
        self.rollback.is_some()
    }

    /// rollback 失败的稳定分类（与 primary 分开；无则 None）。
    pub fn rollback_kind(&self) -> Option<StorageFailureKind> {
        self.rollback.as_ref().map(|r| r.kind)
    }

    /// rollback 失败的 typed cause（与 primary source 链分开；测试可沿此
    /// 链下钻识别原始 rollback 底层错误）。生产日志/协议边界不使用它，
    /// 只按 `rollback_kind()` 记录安全分类。
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rollback_cause(&self) -> Option<&(dyn Error + Send + Sync)> {
        self.rollback.as_ref().map(|r| &*r.cause)
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "storage failure: {}", self.kind.as_str())
    }
}

impl Error for StorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.cause)
    }
}

/// 仅内部诊断用的消息 cause。
#[derive(Debug, Clone)]
struct StorageMessage(String);

impl fmt::Display for StorageMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for StorageMessage {}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用 typed cause 标记。
    #[derive(Debug)]
    struct Marker(String);
    impl fmt::Display for Marker {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.0)
        }
    }
    impl Error for Marker {}

    /// 注入 SQL / 路径 / token / body 的 cause：Display 必须完全不含这些
    /// 值，但 source 链仍能下钻到原始 cause 并识别其内容。
    #[test]
    fn display_is_safe_while_source_chain_keeps_the_cause() {
        let injected =
            "SELECT * FROM users WHERE token='abc' body='secret' path=/tmp/secret/core.db";
        let err = StorageError::from_message(StorageFailureKind::DecodeOrCorrupt, injected);

        let display = err.to_string();
        for forbidden in [
            "SELECT", "users", "abc", "secret", "/tmp", "core.db", "token", "body",
        ] {
            assert!(
                !display.contains(forbidden),
                "Display must not leak injected cause value {forbidden:?}: {display:?}"
            );
        }
        assert_eq!(display, "storage failure: decode_or_corrupt");

        let source = err
            .source()
            .expect("typed cause must be reachable via source()");
        assert_eq!(source.to_string(), injected);
    }

    #[test]
    fn kind_is_exposed_safely() {
        assert_eq!(
            StorageError::from_message(StorageFailureKind::Constraint, "x").kind(),
            StorageFailureKind::Constraint
        );
        assert_eq!(
            StorageError::from_message(StorageFailureKind::BusyOrLocked, "x").kind(),
            StorageFailureKind::BusyOrLocked
        );
        assert_eq!(
            StorageError::from_message(StorageFailureKind::Unknown, "x").kind(),
            StorageFailureKind::Unknown
        );
        assert_eq!(StorageFailureKind::Constraint.as_str(), "constraint");
        assert_eq!(StorageFailureKind::BusyOrLocked.as_str(), "busy_or_locked");
        assert_eq!(
            StorageFailureKind::ConnectionOrIo.as_str(),
            "connection_or_io"
        );
        assert_eq!(
            StorageFailureKind::DecodeOrCorrupt.as_str(),
            "decode_or_corrupt"
        );
        assert_eq!(StorageFailureKind::Unknown.as_str(), "unknown");
    }

    /// typed cause 可沿 `Error::source()` 链下钻；原始 cause 的内容与
    /// 结构通过链可识别（Display 永不包含它）。
    #[test]
    fn typed_cause_can_be_identified_down_the_source_chain() {
        let err = StorageError::new(
            StorageFailureKind::ConnectionOrIo,
            Marker("disk gone: /dev/null".into()),
        );
        let cause = err.source().expect("cause present");
        assert_eq!(cause.to_string(), "disk gone: /dev/null");
        // 嵌套 cause 链：上层错误 → StorageError → 原始 cause。
        assert!(cause.source().is_none());
        assert!(!err.to_string().contains("/dev/null"));
    }

    /// 双因果复合：`with_rollback` 后 primary 主链（kind + source）完全不变，
    /// rollback 的分类与 cause 可单独识别；`Display` 仍只输出 primary 稳定
    /// 分类，绝不包含两条底层文本。
    #[test]
    fn rollback_composite_keeps_primary_chain_and_isolates_rollback() {
        let primary = StorageError::new(
            StorageFailureKind::Constraint,
            Marker("primary: INSERT INTO t VALUES ('secret-token')".into()),
        );
        let rollback = StorageError::from_message(
            StorageFailureKind::ConnectionOrIo,
            "rollback: database is locked at /tmp/secret/core.db",
        );
        let composite = primary.clone().with_rollback(rollback);

        // primary 主链完全不变：类型/变体与 source 都保持原样。
        assert_eq!(composite.kind(), StorageFailureKind::Constraint);
        assert_eq!(composite.kind(), primary.kind());
        assert_eq!(
            composite.source().expect("primary source").to_string(),
            "primary: INSERT INTO t VALUES ('secret-token')"
        );

        // rollback 单独识别。
        assert!(composite.has_rollback_failure());
        assert_eq!(
            composite.rollback_kind(),
            Some(StorageFailureKind::ConnectionOrIo)
        );
        assert_eq!(
            composite
                .rollback_cause()
                .expect("rollback cause present")
                .to_string(),
            "rollback: database is locked at /tmp/secret/core.db"
        );

        // Display 只输出 primary 稳定分类；两条 cause 文本都不泄漏。
        assert_eq!(composite.to_string(), "storage failure: constraint");
        for forbidden in ["INSERT", "secret-token", "locked", "/tmp", "core.db"] {
            assert!(
                !composite.to_string().contains(forbidden),
                "Display must not leak any underlying text: {forbidden:?}"
            );
        }
    }

    /// 无 rollback 复合时 accessor 全部为空（重放成功分支的普通 rollback
    /// StorageError 就是这种形状）。
    #[test]
    fn plain_storage_error_has_no_rollback_failure() {
        let err = StorageError::from_message(StorageFailureKind::BusyOrLocked, "x");
        assert!(!err.has_rollback_failure());
        assert_eq!(err.rollback_kind(), None);
        assert!(err.rollback_cause().is_none());
    }
}
