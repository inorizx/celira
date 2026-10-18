//! 本机 Operator CLI 命令：bootstrap-human / create-human /
//! issue-human-session / issue-pairing-grant。
//!
//! CLI 是部署控制面，不是公共远程管理协议。成功时只向 stdout 输出一行
//! 机器可解析 JSON（participant_id、session_id、token、grant_id、
//! expires_at 等），日志与 stderr 绝不输出原始 token；失败以非零码退出。
//!
//! `create-human` 与 `bootstrap-human` 共用同一 Human 创建与一次性
//! Session 签发用例（`application::identity` 的 provision_human），区别
//! 只在 bootstrap 额外占用全局 guard；HTTP 不提供匿名注册。
//!
//! 全部 CLI 命令在打开数据库后都幂等初始化 Home Identity（与服务器启动
//! 同一 `application::home::init_home` 用例与 guard）：任何 Core 进程
//! （常驻或一次性 CLI）都是 Home 事实的初始化入口，保证「每个新 Core
//! 数据库有且只有一个 Home Identity」，且初始配对 CLI 在尚无服务器启动
//! 的库上也能产出可兑换的 Grant。

use serde_json::json;

use crate::adapters::sqlite::home_store::SqliteHomeStore;
use crate::adapters::sqlite::identity_store::SqliteIdentityStore;
use crate::adapters::sqlite::migration_store::{DbError, MigrationStore};
use crate::application::home;
use crate::application::identity;
use crate::application::identity::{BootstrapError, CreateHumanError, IssueSessionError};
use crate::composition::Config;

/// CLI 失败（打印到 stderr 后以非零码退出）。
///
/// Identity/Home 应用错误以 typed 变体保留（不再退化为 String）：token
/// 摘要、typed cause 链经 `Error::source()` 可下钻，绝不在 Display 或
/// 日志中泄露原始 token。
#[derive(Debug)]
pub enum CliError {
    Database(DbError),
    Bootstrap(BootstrapError),
    CreateHuman(CreateHumanError),
    IssueSession(IssueSessionError),
    Home(home::InitHomeError),
    IssueGrant(home::IssueGrantError),
}

impl CliError {
    /// 稳定内部分类（结构化日志 error_kind 专用）。
    pub fn error_kind(&self) -> &'static str {
        match self {
            CliError::Database(_) => "database",
            CliError::Bootstrap(_) => "identity",
            CliError::CreateHuman(_) => "identity",
            CliError::IssueSession(_) => "identity",
            CliError::Home(_) => "home",
            CliError::IssueGrant(_) => "home",
        }
    }
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(e) => write!(f, "database failure: {e}"),
            Self::Bootstrap(e) => write!(f, "identity failure: {e}"),
            Self::CreateHuman(e) => write!(f, "identity failure: {e}"),
            Self::IssueSession(e) => write!(f, "identity failure: {e}"),
            Self::Home(e) => write!(f, "home failure: {e}"),
            Self::IssueGrant(e) => write!(f, "home failure: {e}"),
        }
    }
}

impl std::error::Error for CliError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(e) => Some(e),
            Self::Bootstrap(e) => Some(e),
            Self::CreateHuman(e) => Some(e),
            Self::IssueSession(e) => Some(e),
            Self::Home(e) => Some(e),
            Self::IssueGrant(e) => Some(e),
        }
    }
}

async fn open_identity(
    config: &Config,
) -> Result<(MigrationStore, SqliteIdentityStore, SqliteHomeStore), CliError> {
    let store = MigrationStore::open(&config.db_path)
        .await
        .map_err(CliError::Database)?;
    let identity = SqliteIdentityStore::new(store.pool().clone());
    let home_store = SqliteHomeStore::new(store.pool().clone());
    // 任何 CLI 命令都幂等初始化 Home Identity（同一 guard；已初始化跳过）。
    home::init_home(&home_store, config.home_display_name.as_deref())
        .await
        .map_err(CliError::Home)?;
    Ok((store, identity, home_store))
}

/// `celira-im bootstrap-human --display-name <NAME>`
///
/// 打开同一 `CELIRA_DB_PATH` 并执行 migration，在 guard + 首位 Human +
/// 初始 Session 的事务内创建身份，然后仅向 stdout 输出机器可解析 JSON。
pub async fn bootstrap_human(config: &Config, display_name: &str) -> Result<(), CliError> {
    let (_store, identity, _home) = open_identity(config).await?;
    match identity::bootstrap_human(&identity, display_name).await {
        Ok(outcome) => {
            print_outcome(&outcome);
            Ok(())
        }
        Err(e) => Err(CliError::Bootstrap(e)),
    }
}

/// `celira-im create-human --display-name <NAME>`
///
/// 创建新的 active Human 并签发一次性 Session；与 bootstrap-human 共用
/// 同一 Human 创建与 Session 签发用例，但不占用 bootstrap guard。
pub async fn create_human(config: &Config, display_name: &str) -> Result<(), CliError> {
    let (_store, identity, _home) = open_identity(config).await?;
    match identity::create_human(&identity, display_name).await {
        Ok(outcome) => {
            print_outcome(&outcome);
            Ok(())
        }
        Err(e) => Err(CliError::CreateHuman(e)),
    }
}

fn print_outcome(outcome: &identity::BootstrapOutcome) {
    println!(
        "{}",
        json!({
            "participant_id": outcome.participant.id.as_str(),
            "session_id": outcome.session.id.as_str(),
            "token": outcome.token.as_str(),
        })
    );
}

/// `celira-im issue-human-session --human-id <PARTICIPANT_ID>`
///
/// 只对存在且 active 的 Human 创建新 Session；新 token 只输出一次。
pub async fn issue_human_session(config: &Config, human_id: &str) -> Result<(), CliError> {
    let (_store, identity, _home) = open_identity(config).await?;
    match identity::issue_human_session(&identity, human_id).await {
        Ok(issued) => {
            println!(
                "{}",
                json!({
                    "participant_id": human_id,
                    "session_id": issued.session.id.as_str(),
                    "token": issued.token.as_str(),
                })
            );
            Ok(())
        }
        Err(e) => Err(CliError::IssueSession(e)),
    }
}

/// `celira-im issue-pairing-grant --human-id <PARTICIPANT_ID> [--device-label <LABEL>]`
///
/// 本机/SSH operator CLI 的初始配对路径：在既有 active Human 下签发与
/// HTTP `POST /v1/home/pairing-grants` 同一公共 pairing grant 事实/状态机
/// 的 Grant（`application::home::issue_grant`），一次性向 stdout 输出原始
/// token，用于完成第一台远程 Client 的初始配对。TTL 取自配置
/// （`CELIRA_GRANT_TTL_MINUTES`，默认 15 分钟）。
pub async fn issue_pairing_grant(
    config: &Config,
    human_id: &str,
    device_label: Option<&str>,
) -> Result<(), CliError> {
    let (_store, _identity, home_store) = open_identity(config).await?;
    match home::issue_grant(
        &home_store,
        &crate::domain::participant::ParticipantId(human_id.to_owned()),
        device_label,
        // operator CLI 只签发**设备配对**那一类：邀请一个人是产品面的事
        // （认证 Human 在客户端里签发），不从命令行代发。
        crate::domain::home::GrantKind::SelfDevice,
        config.grant_ttl_minutes,
    )
    .await
    {
        Ok(issued) => {
            println!(
                "{}",
                json!({
                    "grant_id": issued.grant.grant_id.as_str(),
                    "status": issued.grant.status.as_str(),
                    "device_label": issued.grant.device_label.as_ref().map(|n| n.as_str()),
                    "issued_at": issued.grant.issued_at.to_rfc3339(),
                    "expires_at": issued.grant.expires_at.to_rfc3339(),
                    "token": issued.token.as_str(),
                })
            );
            Ok(())
        }
        Err(e) => Err(CliError::IssueGrant(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::storage::{StorageError, StorageFailureKind};
    use std::error::Error;

    /// CliError 以 typed 变体保留 identity 应用错误：source() 可沿
    /// BootstrapError → StorageError 下钻；Display 链绝不泄露 token。
    #[test]
    fn cli_error_keeps_typed_source_and_never_leaks_token() {
        let storage = StorageError::from_message(
            StorageFailureKind::DecodeOrCorrupt,
            "token=clr_hs_ABCDEFGHIJKLMNOPQRSTUVWXYZ234567 body=secret",
        );
        let err = CliError::Bootstrap(BootstrapError::Store(storage));
        assert_eq!(err.error_kind(), "identity");

        let display = err.to_string();
        for forbidden in ["clr_hs_", "ABCDEFGH", "token", "secret"] {
            assert!(
                !display.contains(forbidden),
                "CliError Display must not leak {forbidden:?}: {display:?}"
            );
        }

        let bootstrap = err
            .source()
            .expect("typed BootstrapError must be preserved");
        assert!(
            bootstrap.to_string().contains("identity store failure"),
            "typed identity error reachable via source: {bootstrap}"
        );
        let storage_err = bootstrap
            .source()
            .expect("StorageError below BootstrapError");
        assert!(
            storage_err.to_string().contains("decode_or_corrupt"),
            "stable classification reachable down the chain: {storage_err}"
        );
    }
}
