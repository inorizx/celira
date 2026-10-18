//! 运行配置：全部来自环境变量，带安全默认值。
//!
//! 环境变量非法时返回 `ConfigError`，进程必须明确失败，不得静默回退。

use std::net::SocketAddr;
use std::path::PathBuf;

use crate::domain::participant::DisplayName;

/// 监听地址环境变量。
pub const ENV_LISTEN_ADDR: &str = "CELIRA_LISTEN_ADDR";
/// SQLite 路径环境变量。
pub const ENV_DB_PATH: &str = "CELIRA_DB_PATH";
/// 媒体根目录环境变量（附件字节存储；契约 CORE_V1_ATTACHMENTS.md 10）。
pub const ENV_MEDIA_ROOT: &str = "CELIRA_MEDIA_ROOT";
/// Pairing Grant TTL（分钟）环境变量：1..=60，默认 15。
pub const ENV_GRANT_TTL_MINUTES: &str = "CELIRA_GRANT_TTL_MINUTES";
/// Home Identity 可选显示名环境变量（仅首次初始化生效；已初始化后不可变）。
pub const ENV_HOME_DISPLAY_NAME: &str = "CELIRA_HOME_DISPLAY_NAME";

/// 安全默认值：仅监听本机回环。
pub const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:8080";
/// 安全默认值：相对运行目录的 data/core.db。
pub const DEFAULT_DB_PATH: &str = "data/core.db";
/// 安全默认值：相对运行目录的 data/media（附件字节存储根目录）。
pub const DEFAULT_MEDIA_ROOT: &str = "data/media";
/// Grant TTL 默认值：15 分钟（HOME_ACCESS.md §9 已确认默认）。
pub const DEFAULT_GRANT_TTL_MINUTES: u32 = 15;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub listen_addr: SocketAddr,
    pub db_path: PathBuf,
    pub media_root: PathBuf,
    /// Pairing Grant TTL（分钟），已校验 1..=60。
    pub grant_ttl_minutes: u32,
    /// Home Identity 可选显示名（原始字符串；由初始化用例按显示名规则
    /// 完整校验；已初始化的库忽略）。
    pub home_display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    InvalidListenAddr(String),
    EmptyDbPath,
    EmptyMediaRoot,
    InvalidGrantTtl(String),
    InvalidHomeDisplayName(String),
}

impl ConfigError {
    /// 稳定内部分类（结构化日志 error_kind 专用）。
    pub fn error_kind(&self) -> &'static str {
        match self {
            ConfigError::InvalidListenAddr(_) => "invalid_listen_addr",
            ConfigError::EmptyDbPath => "empty_db_path",
            ConfigError::EmptyMediaRoot => "empty_media_root",
            ConfigError::InvalidGrantTtl(_) => "invalid_grant_ttl",
            ConfigError::InvalidHomeDisplayName(_) => "invalid_home_display_name",
        }
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::InvalidListenAddr(v) => {
                write!(f, "{ENV_LISTEN_ADDR}={v:?} is not a valid SocketAddr")
            }
            ConfigError::EmptyDbPath => write!(f, "{ENV_DB_PATH} must not be empty"),
            ConfigError::EmptyMediaRoot => write!(f, "{ENV_MEDIA_ROOT} must not be empty"),
            ConfigError::InvalidGrantTtl(v) => write!(
                f,
                "{ENV_GRANT_TTL_MINUTES}={v:?} is not an integer in 1..=60"
            ),
            ConfigError::InvalidHomeDisplayName(v) => write!(
                f,
                "{ENV_HOME_DISPLAY_NAME}={v:?} is not a valid home display name"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// 从环境变量加载配置；缺省时使用安全默认值。
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::parse(
            std::env::var(ENV_LISTEN_ADDR).ok().as_deref(),
            std::env::var(ENV_DB_PATH).ok().as_deref(),
            std::env::var(ENV_MEDIA_ROOT).ok().as_deref(),
            std::env::var(ENV_GRANT_TTL_MINUTES).ok().as_deref(),
            std::env::var(ENV_HOME_DISPLAY_NAME).ok().as_deref(),
        )
    }

    /// 纯函数解析，便于测试（不读写进程环境）。
    pub fn parse(
        listen_addr: Option<&str>,
        db_path: Option<&str>,
        media_root: Option<&str>,
        grant_ttl_minutes: Option<&str>,
        home_display_name: Option<&str>,
    ) -> Result<Self, ConfigError> {
        let listen_addr: SocketAddr =
            listen_addr
                .unwrap_or(DEFAULT_LISTEN_ADDR)
                .parse()
                .map_err(|_| {
                    ConfigError::InvalidListenAddr(
                        listen_addr.unwrap_or(DEFAULT_LISTEN_ADDR).to_owned(),
                    )
                })?;

        let db_path = db_path.unwrap_or(DEFAULT_DB_PATH);
        if db_path.trim().is_empty() {
            return Err(ConfigError::EmptyDbPath);
        }

        let media_root = media_root.unwrap_or(DEFAULT_MEDIA_ROOT);
        if media_root.trim().is_empty() {
            return Err(ConfigError::EmptyMediaRoot);
        }

        let grant_ttl_minutes = match grant_ttl_minutes {
            None => DEFAULT_GRANT_TTL_MINUTES,
            Some(raw) => crate::application::home::parse_grant_ttl_minutes(raw)
                .ok_or_else(|| ConfigError::InvalidGrantTtl(raw.to_owned()))?,
        };

        let home_display_name = match home_display_name {
            None | Some("") => None,
            Some(raw) => {
                // 完整规则（80 scalar、无 NUL、非纯空白）在此先行校验，
                // 初始化用例会再次校验（同一规则，双保险）。
                DisplayName::parse(raw)
                    .map_err(|_| ConfigError::InvalidHomeDisplayName(raw.to_owned()))?;
                Some(raw.to_owned())
            }
        };

        Ok(Self {
            listen_addr,
            db_path: PathBuf::from(db_path),
            media_root: PathBuf::from(media_root),
            grant_ttl_minutes,
            home_display_name,
        })
    }
}
