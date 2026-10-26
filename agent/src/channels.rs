//! 外部输入入口（每 Agent 一支）：把一个外部系统的 `POST` 变成一条**私有输入**。
//!
//! ## 边界（先说清楚它不是什么）
//!
//! - 这里落下的东西是**私有输入**，**不是公共事实**：事件写进 `<环境根>/channels/inbox/`，
//!   不写 Core、不写 im、不写任何一份可被别的主体读到的账。它进的是**这一支 Agent 自己的环境**。
//! - **默认关闭**：环境根下没有 `channels/routes.json` 就**没有入口**（[`ChannelServer::start`]
//!   返回 `None`），不建目录、不开端口、不占任何资源。哪一个外部系统能说话、说什么，全由操作者
//!   写在那份文件里。
//! - **本刀尚未接入宿主唤醒**：入口只把一条 `sync_channel(1)` 的 `Receiver` 交出去
//!   （[`ChannelServer::take_wake_receiver`]），宿主那一刀才把它接进 `WakeHub`。因此今天收到事件
//!   **不会**叫醒主体——事实先落盘，接线是下一刀。
//! - 它不认识 Core、OS、任何 App，也不是 App（不实现 `AppPort`）：它是**宿主的入口**，与门铃同层
//!   （门铃是宿主侧那条唤醒长连接，本件是宿主侧那条入站短连接）。
//!
//! ## 一次请求经过什么
//!
//! ```text
//! POST /channels/{route}
//!   -> 路由解析（未知 / disabled 都是 404）
//!   -> Authorization: Bearer <该路由的 secret>（常量时间比较；缺/错 401）
//!   -> 每路由固定 60 秒窗口的条数（内存态；429 带 Retry-After）
//!   -> 正文：全局硬上限（axum 层）-> 每路由上限（413）
//!   -> 媒体类型只认 text/* 与 application/json，且字节必须是 UTF-8（415）
//!   -> 幂等键（缺就现生成）-> 文件名 <route>--<id>.json，create_new 抢（已存在 = 200 重复）
//!   -> 写文件 + sync_all（任何失败删半成品、503）
//!   -> 剪一次保留期（1000 条 / 30 天，最旧先删）
//!   -> 唤醒信号 try_send(())（满了就丢：它说的是"有输入"，不是"来了几条"）
//! ```
//!
//! ## 认证 v1 为什么只有 Bearer（不做 HMAC / 签名）
//!
//! 这一格回答的问题只有一句：**这条请求是不是拿着这条路由的 secret 发来的**。HMAC 要解决的是
//! 另一件事——"报文在途中被改过没有、这条是不是重放"。它需要双方共享一把签名钥匙、一个时钟
//! 假设与一个重放窗口，而本刀**没有第二个消费者**：今天接进来的外部系统只会带一个 Bearer。
//! 因此这里**不引入摘要依赖、不假装自己防重放**：幂等键只保证"同一个键只落一条"，
//! 它**不证明**"这条请求是新的"——同键异体也算重复，是这一条的直接推论（见下）。
//!
//! **没有密钥就没有签名**，所以本刀也不写"预留 HMAC 的钩子"：那会是一条没人验收的空壳。
//! 要签名的那天，改变的是 `Authorization` 这一个头与它的校验函数，落盘形状与状态码都不变。
//!
//! ## 幂等：键是身份，正文不是
//!
//! 优先 `Idempotency-Key`，其次 `X-GitHub-Delivery`，再次 `X-Request-Id`；值只允许
//! `[A-Za-z0-9._-]`、1..=64 字节（**形状不对就是 400**，不静默丢弃、不换用下一个头）。
//! 三个头都没有就现生成一个随机 id。文件名是 `<route>--<id>.json`，用 `create_new` 抢：
//! 已存在即重复（200），**不覆盖、不追加**。
//!
//! ⚠ **同键异体也算重复**：键标识的是"这一次投递"，不是"这一份正文"。第二次带着同一个键、
//! 不同正文来，入口只回 200 duplicate，且**第一个文件一个字节都不改**。要新的一条，换键。
//!
//! ## 限流、保留期、热加载：三条机械限制，各自说清射程
//!
//! - **限流**是**每路由**固定 60 秒窗口（不是滑动窗口、不是令牌桶），计数在内存里：
//!   `rate_per_minute = 0` 表示不限；**重启清零**——它是保护入口的机械闸，不是账。
//! - **保留期**写死为**最多 1000 条或 30 天**，启动与每次接收后按**最旧先删**（`mtime` 排序，
//!   同刻按文件名）：收件箱是"待处理的输入"，不是主体的记忆，因此它有上界。
//! - **热加载**只看 `routes.json` 的 `mtime` ＋ 长度（2 秒轮询）：解析失败**保留上一张表**
//!   并打一行日志；`bind` 变更**要求重启**（监听套接字不热换，这一条写在
//!   [`ChannelServer::reload_now`] 上）。
//!
//! ## 头只存三样
//!
//! `content_type`、`event`（`X-GitHub-Event` / `X-Gitlab-Event` 取一个）、`user_agent`。
//! **绝不存 `Authorization`、`Cookie`**，也不存别的头——事件文件里没有它们的字段，
//! 因此"顺手把整个请求头 dump 下来"这种泄漏在这份形状里没有位置。三样各按 128 / 256 / 256
//! 字节按字符边界截断（头可以很长，事件文件不该跟着长）。
//!
//! ## 权限（Unix 0600；Windows 尽力）
//!
//! `routes.json` 里是**凭据**，因此本模块写它时在 Unix 上建 0600 的文件（先写临时文件、
//! 设权限、再改名）。Windows 上**尽力**：没有等价的 mode 位，文件继承目录 ACL——这一条是
//! 如实登记的边界，不假装两个平台一样。secret **不派生 `Debug`**、不进日志、错误响应里
//! 一个字节都不回显。

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::{Deserialize, Serialize};

/// 配置所在的子目录（环境根之下）。
const CONFIG_DIRECTORY: &str = "channels";
/// 路由表文件名。
const CONFIG_FILE: &str = "routes.json";
/// 收件箱子目录（环境根之下）。
const INBOX_DIRECTORY: &str = "inbox";

/// 每路由正文上限的缺省值。
pub const DEFAULT_MAX_BODY_BYTES: usize = 32 * 1024;
/// 正文上限的**硬**上界：再大的请求在 axum 那一层就进不来。
pub const MAX_BODY_BYTES: usize = 256 * 1024;
/// 每路由每分钟条数的缺省值。
pub const DEFAULT_RATE_PER_MINUTE: u32 = 60;
/// 限流窗口：固定 60 秒（不滑动）。
const RATE_WINDOW: Duration = Duration::from_secs(60);
/// 路由表轮询间隔。
const RELOAD_INTERVAL: Duration = Duration::from_secs(2);
/// 幂等键 / 事件 id 的形状上界。
const MAX_EVENT_ID_BYTES: usize = 64;
/// 一个 secret 的形状上界（只防写坏，不规定生成方式）。
const MAX_SECRET_BYTES: usize = 256;
/// 三样请求头各自的截断上界（按字符边界）。
const MAX_EVENT_HEADER_BYTES: usize = 128;
const MAX_CONTENT_TYPE_BYTES: usize = 256;
const MAX_USER_AGENT_BYTES: usize = 256;

// ---------------------------------------------------------------------------
// 配置
// ---------------------------------------------------------------------------

/// 一份配置读不出来 / 形状不对。
///
/// 两个变体刻意分开：`Io` 是"文件这一层的事"（不存在、读不动、写不下），`Shape` 是"文件里的
/// 内容不合约"。两者的下一步不同——前者去看盘，后者去改字。载荷是**给操作者的细节**，
/// 里面没有 secret（本模块从不把 secret 放进任何错误文本）。
#[derive(Debug)]
pub enum ConfigError {
    Io(String),
    Shape(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(detail) => write!(formatter, "IO: {detail}"),
            ConfigError::Shape(detail) => write!(formatter, "形状: {detail}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// 一条路由的声明。**secret 是私有字段**：它只经 [`RouteConfig::secret`] 交给承载请求的那一处，
/// 本类型**不派生 `Debug`**，因此不会被顺手打进一行日志。
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteConfig {
    id: String,
    label: String,
    secret: String,
    #[serde(default = "default_enabled")]
    enabled: bool,
    #[serde(default = "default_max_body")]
    max_body_bytes: usize,
    #[serde(default = "default_rate")]
    rate_per_minute: u32,
}

impl RouteConfig {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// 这份凭据只给"装请求头"的那一处用：不打日志、不进任何读数行。
    pub fn secret(&self) -> &str {
        &self.secret
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn max_body_bytes(&self) -> usize {
        self.max_body_bytes
    }

    pub fn rate_per_minute(&self) -> u32 {
        self.rate_per_minute
    }
}

/// `channels/routes.json` 的全部内容。
///
/// `bind` 是 `ip:port`（数字地址，不吃主机名：入口不替操作者做 DNS）。端口 `0` 留给测试——
/// 它绑一个系统分配的端口，[`ChannelServer::local_addr`] 说的是**实际**绑到的那个。
///
/// 形状是**严格**的：多一个认不得的键、少一个必填键、取值不在范围内，**整份拒绝**——路由表是
/// 凭据表，读不懂就不该猜（也不会"跳过坏的那一条、留下好的"）。
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelConfig {
    bind: String,
    routes: Vec<RouteConfig>,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8787".to_string(),
            routes: Vec::new(),
        }
    }
}

impl ChannelConfig {
    /// 读环境根下那一份。**没有文件 ⇒ `Ok(None)`**（这是"默认关闭"的判据，不是错误）。
    pub fn load(root: &Path) -> Result<Option<Self>, ConfigError> {
        let path = routes_path(root);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(ConfigError::Io(format!("{}: {error}", path.display())));
            }
        };
        let config: ChannelConfig = serde_json::from_str(&text)
            .map_err(|error| ConfigError::Shape(format!("{}: {error}", path.display())))?;
        config.validate()?;
        Ok(Some(config))
    }

    pub fn bind(&self) -> &str {
        &self.bind
    }

    pub fn routes(&self) -> &[RouteConfig] {
        &self.routes
    }

    /// 形状校验。**整份拒绝**而不是"跳过坏的那一条"：路由表是凭据表，读不懂就不该猜。
    fn validate(&self) -> Result<(), ConfigError> {
        self.bind.parse::<SocketAddr>().map_err(|_| {
            ConfigError::Shape(format!("bind 必须是 ip:port（数字地址）: {}", self.bind))
        })?;
        let mut seen: HashSet<&str> = HashSet::new();
        for route in &self.routes {
            if !route_id_is_valid(&route.id) {
                return Err(ConfigError::Shape(format!(
                    "route id 只允许 [a-z0-9][a-z0-9_-]{{0,63}}: {}",
                    route.id
                )));
            }
            if !seen.insert(route.id.as_str()) {
                return Err(ConfigError::Shape(format!("route id 重复: {}", route.id)));
            }
            if route.label.trim().is_empty() {
                return Err(ConfigError::Shape(format!(
                    "route 没有 label: {}",
                    route.id
                )));
            }
            if route.secret.is_empty() || route.secret.len() > MAX_SECRET_BYTES {
                return Err(ConfigError::Shape(format!(
                    "route 的 secret 长度必须在 1..={MAX_SECRET_BYTES} 字节: {}",
                    route.id
                )));
            }
            if route.max_body_bytes == 0 || route.max_body_bytes > MAX_BODY_BYTES {
                return Err(ConfigError::Shape(format!(
                    "route 的 max_body_bytes 必须在 1..={MAX_BODY_BYTES}: {}",
                    route.id
                )));
            }
        }
        Ok(())
    }
}

fn default_enabled() -> bool {
    true
}

fn default_max_body() -> usize {
    DEFAULT_MAX_BODY_BYTES
}

fn default_rate() -> u32 {
    DEFAULT_RATE_PER_MINUTE
}

fn routes_path(root: &Path) -> PathBuf {
    root.join(CONFIG_DIRECTORY).join(CONFIG_FILE)
}

fn inbox_path(root: &Path) -> PathBuf {
    root.join(CONFIG_DIRECTORY).join(INBOX_DIRECTORY)
}

/// 一条能被安全用作文件名前缀的 route id。
///
/// 只认小写字母、数字、`-`、`_`，首字符必须是字母或数字，长度 1..=64。**这个集合里没有 `.`
/// 与路径分隔符**，因此 `../` 一类的形状在这一步就出不去；小写也避免了"大小写不敏感的文件系统
/// 上两条 id 撞进同一个文件"。
fn route_id_is_valid(id: &str) -> bool {
    let bytes = id.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 {
        return false;
    }
    if !bytes[0].is_ascii_alphanumeric() {
        return false;
    }
    bytes.iter().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-' || *byte == b'_'
    })
}

/// 由 label 造一个 id。
///
/// **带路径分隔符或 `..` 的 label 直接拒绝**：label 不是路径，静默把它"洗成"一个安全 id 会把
/// 操作者的错误藏起来。别的字符（空格、中文、标点）折成 `-`，连续折叠、首尾去掉。
fn slug_from_label(label: &str) -> Result<String, ConfigError> {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return Err(ConfigError::Shape("label 不能为空".to_string()));
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains("..") {
        return Err(ConfigError::Shape(format!(
            "label 不是路径，不能含 / \\ 或 ..: {label}"
        )));
    }
    let mut slug = String::with_capacity(trimmed.len());
    for character in trimmed.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        return Err(ConfigError::Shape(format!(
            "label 折不出一个可用的 route id: {label}"
        )));
    }
    let slug: String = slug.chars().take(48).collect();
    let slug = slug.trim_end_matches('-').to_string();
    if !route_id_is_valid(&slug) {
        return Err(ConfigError::Shape(format!(
            "label 折出的 route id 形状不对: {label}"
        )));
    }
    Ok(slug)
}

fn unique_route_id(config: &ChannelConfig, slug: &str) -> Result<String, ConfigError> {
    if !config.routes.iter().any(|route| route.id == slug) {
        return Ok(slug.to_string());
    }
    for suffix in 2..=999u32 {
        let candidate = format!("{slug}-{suffix}");
        if !config.routes.iter().any(|route| route.id == candidate) {
            return Ok(candidate);
        }
    }
    Err(ConfigError::Shape(format!("{slug} 的编号已经用满")))
}

/// 新生成的一份 secret：`cc_` ＋ 32 字节随机数的十六进制。
fn new_secret() -> Result<String, ConfigError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| ConfigError::Io(format!("随机数取不到: {error}")))?;
    let mut text = String::with_capacity(3 + 64);
    text.push_str("cc_");
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    Ok(text)
}

/// 新生成的事件 id：16 字节随机数的十六进制。
fn new_event_id() -> Option<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).ok()?;
    let mut text = String::with_capacity(32);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    Some(text)
}

/// 写回 `routes.json`：先写临时文件、设 0600（Unix）、再改名。
///
/// 临时文件用 `create_new`：上一轮被打断留下的半成品不会被接着写。
fn store_config(root: &Path, config: &ChannelConfig) -> Result<(), ConfigError> {
    let directory = root.join(CONFIG_DIRECTORY);
    fs::create_dir_all(&directory)
        .map_err(|error| ConfigError::Io(format!("建不了 {}: {error}", directory.display())))?;
    let path = directory.join(CONFIG_FILE);
    let temporary = directory.join(format!(".{CONFIG_FILE}.tmp"));
    let _ = fs::remove_file(&temporary);
    let mut text = serde_json::to_string_pretty(config)
        .map_err(|error| ConfigError::Shape(format!("配置写不出来: {error}")))?;
    text.push('\n');

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|error| ConfigError::Io(format!("建不了 {}: {error}", temporary.display())))?;
    if let Err(error) = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
    {
        drop(file);
        let _ = fs::remove_file(&temporary);
        return Err(ConfigError::Io(format!(
            "写不进 {}: {error}",
            temporary.display()
        )));
    }
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)).map_err(|error| {
            ConfigError::Io(format!("设不了 {} 的权限: {error}", temporary.display()))
        })?;
    }
    fs::rename(&temporary, &path)
        .map_err(|error| ConfigError::Io(format!("改名 {} 失败: {error}", path.display())))?;
    Ok(())
}

/// 新加的一条路由（`secret` 只该被打印**一次**）。
pub struct AddedRoute {
    pub id: String,
    pub endpoint: String,
    pub secret: String,
}

/// 加一条路由并写回配置。
///
/// `max_body_bytes` / `rate_per_minute` 缺省取 [`DEFAULT_MAX_BODY_BYTES`] /
/// [`DEFAULT_RATE_PER_MINUTE`]；`bind` 缺省沿用配置里已有的那一个。label 造 id，冲突加 `-2`、`-3`。
pub fn add_route(
    root: &Path,
    label: &str,
    max_body_bytes: Option<usize>,
    rate_per_minute: Option<u32>,
    bind: Option<&str>,
) -> Result<AddedRoute, ConfigError> {
    let mut config = ChannelConfig::load(root)?.unwrap_or_default();
    let slug = slug_from_label(label)?;
    if let Some(bind) = bind {
        bind.parse::<SocketAddr>()
            .map_err(|_| ConfigError::Shape(format!("bind 必须是 ip:port（数字地址）: {bind}")))?;
        config.bind = bind.to_string();
    }
    let max_body_bytes = max_body_bytes.unwrap_or(DEFAULT_MAX_BODY_BYTES);
    if max_body_bytes == 0 || max_body_bytes > MAX_BODY_BYTES {
        return Err(ConfigError::Shape(format!(
            "max_body_bytes 必须在 1..={MAX_BODY_BYTES}: {max_body_bytes}"
        )));
    }
    let rate_per_minute = rate_per_minute.unwrap_or(DEFAULT_RATE_PER_MINUTE);
    let id = unique_route_id(&config, &slug)?;
    let secret = new_secret()?;
    let endpoint = format!("http://{}/channels/{id}", config.bind);
    config.routes.push(RouteConfig {
        id: id.clone(),
        label: label.trim().to_string(),
        secret: secret.clone(),
        enabled: true,
        max_body_bytes,
        rate_per_minute,
    });
    config.validate()?;
    store_config(root, &config)?;
    Ok(AddedRoute {
        id,
        endpoint,
        secret,
    })
}

/// 删一条路由。**先校验 id 形状**：`../` 一类的值在碰文件之前就被拒。
///
/// 返回 `false` = 配置里本来就没有这一条（不写文件）。
pub fn remove_route(root: &Path, id: &str) -> Result<bool, ConfigError> {
    if !route_id_is_valid(id) {
        return Err(ConfigError::Shape(format!("route id 形状不对: {id}")));
    }
    let Some(mut config) = ChannelConfig::load(root)? else {
        return Ok(false);
    };
    let before = config.routes.len();
    config.routes.retain(|route| route.id != id);
    if config.routes.len() == before {
        return Ok(false);
    }
    store_config(root, &config)?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

/// 收件箱的保留期。缺省＝写死的那两条（1000 条 / 30 天）；测试可以给更小的值，
/// 否则"30 天"这一条要等三十天才能验。
///
/// `max_events` 至少按 1 条算：刚写下的那一条不能被自己这一剪剪掉。
#[derive(Clone, Copy)]
pub struct Retention {
    pub max_events: usize,
    pub max_age: Duration,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            max_events: 1000,
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
        }
    }
}

/// 一条入站事件在盘上的形状。
#[derive(Serialize)]
struct InboxEvent<'a> {
    id: &'a str,
    route: &'a str,
    label: &'a str,
    received_at: String,
    content_type: String,
    event: Option<String>,
    user_agent: Option<String>,
    body: &'a str,
    body_bytes: usize,
}

/// 一次接收的结果（决定 202 / 200）。
enum Recorded {
    Accepted,
    Duplicate,
}

struct Window {
    started: Instant,
    seen: u32,
}

struct ServerState {
    root: PathBuf,
    config_path: PathBuf,
    inbox: PathBuf,
    bind: String,
    routes: RwLock<HashMap<String, Arc<RouteConfig>>>,
    limiter: Mutex<HashMap<String, Window>>,
    retention: Retention,
    wake: SyncSender<()>,
    stamp: Mutex<Option<(SystemTime, u64)>>,
}

impl ServerState {
    fn route(&self, id: &str) -> Option<Arc<RouteConfig>> {
        let table = self
            .routes
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let route = table.get(id)?;
        if !route.enabled {
            return None;
        }
        Some(Arc::clone(route))
    }

    /// 固定窗口：返回 `Err(还要等几秒)`。
    fn admit(&self, route: &RouteConfig) -> Result<(), u64> {
        if route.rate_per_minute == 0 {
            return Ok(());
        }
        let now = Instant::now();
        let mut windows = self
            .limiter
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let window = windows.entry(route.id.clone()).or_insert(Window {
            started: now,
            seen: 0,
        });
        if now.duration_since(window.started) >= RATE_WINDOW {
            window.started = now;
            window.seen = 0;
        }
        if window.seen >= route.rate_per_minute {
            let elapsed = now.duration_since(window.started).as_secs();
            let remaining = RATE_WINDOW.as_secs().saturating_sub(elapsed).max(1);
            return Err(remaining);
        }
        window.seen += 1;
        Ok(())
    }

    fn record(
        &self,
        route: &RouteConfig,
        id: &str,
        content_type: &str,
        event: Option<&str>,
        user_agent: Option<&str>,
        body: &str,
    ) -> std::io::Result<Recorded> {
        fs::create_dir_all(&self.inbox)?;
        let path = self.inbox.join(format!("{}--{id}.json", route.id));
        let payload = InboxEvent {
            id,
            route: route.id.as_str(),
            label: route.label.as_str(),
            received_at: rfc3339_utc(SystemTime::now()),
            content_type: content_type.to_string(),
            event: event.map(str::to_string),
            user_agent: user_agent.map(str::to_string),
            body,
            body_bytes: body.len(),
        };
        let bytes = serde_json::to_vec(&payload)
            .map_err(|error| std::io::Error::other(format!("事件编不出来: {error}")))?;

        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Ok(Recorded::Duplicate);
            }
            Err(error) => return Err(error),
        };
        if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        Ok(Recorded::Accepted)
    }

    /// 收件箱上界：最多 `max_events` 条、最多 `max_age`。最旧先删（`mtime`，同刻按文件名）。
    fn prune(&self) {
        let entries = match fs::read_dir(&self.inbox) {
            Ok(entries) => entries,
            Err(_) => return,
        };
        let now = SystemTime::now();
        let mut files: Vec<(SystemTime, String, PathBuf)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            files.push((modified, name, path));
        }
        files.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        let keep = self.retention.max_events.max(1);
        let mut excess = files.len().saturating_sub(keep);
        for (modified, _, path) in &files {
            let expired = now
                .duration_since(*modified)
                .map(|age| age > self.retention.max_age)
                .unwrap_or(false);
            if !expired && excess == 0 {
                continue;
            }
            if fs::remove_file(path).is_ok() && excess > 0 {
                excess -= 1;
            }
        }
    }

    /// 换了就重读；没换就什么都不做（2 秒一次）。
    fn reload_if_changed(&self) {
        let current = config_stamp(&self.config_path);
        {
            let mut stamp = self.stamp.lock().unwrap_or_else(|error| error.into_inner());
            if *stamp == current {
                return;
            }
            *stamp = current;
        }
        self.reload();
    }

    fn reload(&self) -> bool {
        match ChannelConfig::load(&self.root) {
            Ok(Some(config)) => {
                if config.bind != self.bind {
                    report(&format!(
                        "routes.json 的 bind 变了（{} -> {}）：监听套接字不热换，重启后生效",
                        self.bind, config.bind
                    ));
                }
                let table = config
                    .routes
                    .into_iter()
                    .map(|route| (route.id.clone(), Arc::new(route)))
                    .collect();
                let mut routes = self
                    .routes
                    .write()
                    .unwrap_or_else(|error| error.into_inner());
                *routes = table;
                true
            }
            Ok(None) => {
                report("routes.json 不在了：保留上一张路由表");
                false
            }
            Err(error) => {
                report(&format!(
                    "routes.json 读不出来（{error}）：保留上一张路由表"
                ));
                false
            }
        }
    }
}

fn config_stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let metadata = fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

fn report(message: &str) {
    eprintln!("[channels] {message}");
}

/// 入口本身。**没有 `routes.json` 就没有它**（[`ChannelServer::start`] 返回 `None`）。
pub struct ChannelServer {
    addr: SocketAddr,
    state: Arc<ServerState>,
    wake: Mutex<Option<Receiver<()>>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl ChannelServer {
    /// 按环境根启动。没有 `routes.json` ⇒ `None`（**默认关闭**，一个目录都不建）。
    ///
    /// 配置存在但读不出来、绑不上端口、建不了收件箱 ⇒ 也返回 `None`，并往 stderr 打一行
    /// `[channels] …` 说明是哪一步（这一行不含 secret）。
    pub fn start(root: &Path) -> Option<ChannelServer> {
        Self::start_with_retention(root, Retention::default())
    }

    /// 同 [`ChannelServer::start`]，保留期由调用方给（测试用；生产用缺省那两条）。
    pub fn start_with_retention(root: &Path, retention: Retention) -> Option<ChannelServer> {
        let config = match ChannelConfig::load(root) {
            Ok(Some(config)) => config,
            Ok(None) => return None,
            Err(error) => {
                report(&format!("routes.json 读不出来（{error}）：不启动入口"));
                return None;
            }
        };
        let bind = config.bind.clone();
        let address: SocketAddr = match bind.parse() {
            Ok(address) => address,
            Err(_) => {
                report(&format!("bind 不是 ip:port: {bind}"));
                return None;
            }
        };
        let inbox = inbox_path(root);
        if let Err(error) = fs::create_dir_all(&inbox) {
            report(&format!("建不了 {}: {error}", inbox.display()));
            return None;
        }

        let (wake_tx, wake_rx) = mpsc::sync_channel::<()>(1);
        let table = config
            .routes
            .into_iter()
            .map(|route| (route.id.clone(), Arc::new(route)))
            .collect();
        let state = Arc::new(ServerState {
            root: root.to_path_buf(),
            config_path: routes_path(root),
            inbox,
            bind: bind.clone(),
            routes: RwLock::new(table),
            limiter: Mutex::new(HashMap::new()),
            retention,
            wake: wake_tx,
            stamp: Mutex::new(config_stamp(&routes_path(root))),
        });
        state.prune();

        let (address_tx, address_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let serving = Arc::clone(&state);
        let thread = std::thread::Builder::new()
            .name("celira-channels".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = address_tx.send(Err(format!("runtime 起不来: {error}")));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let listener = match tokio::net::TcpListener::bind(address).await {
                        Ok(listener) => listener,
                        Err(error) => {
                            let _ = address_tx.send(Err(format!("绑不上 {address}: {error}")));
                            return;
                        }
                    };
                    let bound = match listener.local_addr() {
                        Ok(bound) => bound,
                        Err(error) => {
                            let _ = address_tx.send(Err(format!("读不到本地地址: {error}")));
                            return;
                        }
                    };
                    let _ = address_tx.send(Ok(bound));

                    let watcher_state = Arc::clone(&serving);
                    tokio::spawn(async move {
                        loop {
                            tokio::time::sleep(RELOAD_INTERVAL).await;
                            watcher_state.reload_if_changed();
                        }
                    });

                    let app = Router::new()
                        .route("/channels/{route}", post(ingress))
                        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
                        .with_state(Arc::clone(&serving));
                    let _ = axum::serve(listener, app)
                        .with_graceful_shutdown(async move {
                            let _ = stop_rx.await;
                        })
                        .await;
                });
            })
            .ok()?;

        match address_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(addr)) => Some(ChannelServer {
                addr,
                state,
                wake: Mutex::new(Some(wake_rx)),
                stop: Some(stop_tx),
                thread: Some(thread),
            }),
            Ok(Err(detail)) => {
                report(&detail);
                let _ = thread.join();
                None
            }
            Err(_) => {
                report("等不到监听套接字");
                let _ = thread.join();
                None
            }
        }
    }

    /// **实际**绑到的地址（配置写 `127.0.0.1:0` 时，这里给系统分配的那个端口）。
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// 立刻重读 `routes.json`；返回"新表生效了没有"。解析失败保留旧表并打一行。
    ///
    /// ⚠ **`bind` 不在这里生效**：换监听地址要重启（端口是进程启动那一刻定下的）。
    pub fn reload_now(&self) -> bool {
        self.state.reload()
    }

    /// 取走唤醒通道的接收端。**只能取一次**（`Receiver` 不是 `Clone`；取走之后第二次给 `None`）。
    ///
    /// 语义是**电平**：通道容量 1，`try_send` 满了就丢——它说的是"有新的输入"，不是"来了几条"。
    /// 本刀只交出去，宿主那一刀才把它接进 `WakeHub`。
    pub fn take_wake_receiver(&self) -> Option<Receiver<()>> {
        self.wake
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
    }

    /// 收摊：停止接受新请求、等当前请求结束、等线程退出。重复调用安全。
    pub fn shutdown(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ChannelServer {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

async fn ingress(
    State(state): State<Arc<ServerState>>,
    UrlPath(route_id): UrlPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(route) = state.route(&route_id) else {
        return error_response(StatusCode::NOT_FOUND, "unknown-route");
    };
    if !authorized(&headers, route.secret()) {
        return error_response(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    if let Err(seconds) = state.admit(&route) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, seconds.to_string())],
            axum::Json(serde_json::json!({ "error": "rate-limited" })),
        )
            .into_response();
    }

    let Some(content_type) = header_text(&headers, header::CONTENT_TYPE, MAX_CONTENT_TYPE_BYTES)
    else {
        return error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported-content-type",
        );
    };
    if !supported_content_type(&content_type) {
        return error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported-content-type",
        );
    }
    if body.len() > route.max_body_bytes {
        return error_response(StatusCode::PAYLOAD_TOO_LARGE, "payload-too-large");
    }
    let Ok(text) = std::str::from_utf8(&body) else {
        return error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported-content-type",
        );
    };

    let supplied = match supplied_event_id(&headers) {
        Ok(supplied) => supplied,
        Err(()) => return error_response(StatusCode::BAD_REQUEST, "invalid-idempotency-key"),
    };
    let event_id = match supplied {
        Some(id) => id,
        None => match new_event_id() {
            Some(id) => id,
            None => return error_response(StatusCode::SERVICE_UNAVAILABLE, "storage-failed"),
        },
    };
    let event = header_text(&headers, "x-github-event", MAX_EVENT_HEADER_BYTES)
        .or_else(|| header_text(&headers, "x-gitlab-event", MAX_EVENT_HEADER_BYTES));
    let user_agent = header_text(&headers, header::USER_AGENT, MAX_USER_AGENT_BYTES);

    match state.record(
        &route,
        &event_id,
        &content_type,
        event.as_deref(),
        user_agent.as_deref(),
        text,
    ) {
        Ok(Recorded::Accepted) => {
            state.prune();
            let _ = state.wake.try_send(());
            (
                StatusCode::ACCEPTED,
                axum::Json(serde_json::json!({ "status": "accepted", "id": event_id })),
            )
                .into_response()
        }
        Ok(Recorded::Duplicate) => (
            StatusCode::OK,
            axum::Json(serde_json::json!({ "status": "duplicate", "id": event_id })),
        )
            .into_response(),
        Err(error) => {
            report(&format!("写不进 {}: {error}", state.inbox.display()));
            error_response(StatusCode::SERVICE_UNAVAILABLE, "storage-failed")
        }
    }
}

fn error_response(status: StatusCode, kind: &'static str) -> Response {
    (status, axum::Json(serde_json::json!({ "error": kind }))).into_response()
}

/// 取一个可读的请求头；超长按字符边界截断。认不出来（非 ASCII）就当没有。
fn header_text(
    headers: &HeaderMap,
    name: impl axum::http::header::AsHeaderName,
    limit: usize,
) -> Option<String> {
    let value = headers.get(name)?.to_str().ok()?;
    Some(truncate_on_char_boundary(value, limit))
}

fn truncate_on_char_boundary(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn supported_content_type(value: &str) -> bool {
    let media = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    media.starts_with("text/") || media == "application/json"
}

fn authorized(headers: &HeaderMap, secret: &str) -> bool {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Some((scheme, presented)) = value.split_once(' ') else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("bearer") {
        return false;
    }
    same_secret(presented.as_bytes(), secret.as_bytes())
}

/// 常量时间比较（长度先比：**长度不是秘密**，secret 的长度由生成方式固定）。
fn same_secret(presented: &[u8], expected: &[u8]) -> bool {
    if presented.len() != expected.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in presented.iter().zip(expected) {
        difference |= left ^ right;
    }
    difference == 0
}

/// 幂等键：优先 `Idempotency-Key`，其次 `X-GitHub-Delivery`，再次 `X-Request-Id`。
///
/// **第一个出现的头说了算**：它形状不对就是 400，不往后找（否则"换一个头重发"会静默变成
/// 另一条事件）。
fn supplied_event_id(headers: &HeaderMap) -> Result<Option<String>, ()> {
    for name in ["idempotency-key", "x-github-delivery", "x-request-id"] {
        let Some(value) = headers.get(name) else {
            continue;
        };
        let text = value.to_str().map_err(|_| ())?;
        if !event_id_is_valid(text) {
            return Err(());
        }
        return Ok(Some(text.to_string()));
    }
    Ok(None)
}

/// 事件 id 的形状：`[A-Za-z0-9._-]`、1..=64 字节。
///
/// 它要进文件名，所以**不能含路径分隔符**；`.` 单独一个也是合法的值（`route--..json` 仍是
/// 一个普通文件名，不是路径）。
fn event_id_is_valid(id: &str) -> bool {
    let bytes = id.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_EVENT_ID_BYTES {
        return false;
    }
    bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'.' || *byte == b'_' || *byte == b'-')
}

/// RFC 3339（UTC，秒精度）。
fn rfc3339_utc(now: SystemTime) -> String {
    let seconds = now
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// 天数（1970-01-01 起）→ 年月日（Howard Hinnant 的 `civil_from_days`）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_ids_never_carry_a_path() {
        assert!(route_id_is_valid("github"));
        assert!(route_id_is_valid("github-2"));
        assert!(!route_id_is_valid("../evil"));
        assert!(!route_id_is_valid("a/b"));
        assert!(!route_id_is_valid(""));
        assert!(!route_id_is_valid("Upper"));
    }

    #[test]
    fn labels_become_file_safe_ids() {
        assert_eq!(slug_from_label("GitHub 通知").unwrap(), "github");
        assert!(slug_from_label("  Push / PR  ").is_err());
        assert!(slug_from_label("../..").is_err());
        assert!(slug_from_label("!!!").is_err());
    }

    #[test]
    fn timestamps_are_rfc3339() {
        assert_eq!(rfc3339_utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            rfc3339_utc(UNIX_EPOCH + Duration::from_secs(1_711_584_000)),
            "2024-03-28T00:00:00Z"
        );
    }
}
