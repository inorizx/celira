//! **机器侧的家事实**（共享层）——「这个家在哪儿、口开在哪儿、缺件在哪儿、
//! 这支 Agent 在不在这里」。
//!
//! ⚠ **这一片不属于任何一个功能**（2026-10-03 从 `anjia/home.rs` 提升到这里）：**安家**
//! （把一支 Agent 接到这台机器）与**本机世界**（完整 app 自带的那一半：起世界、首布置、
//! 正门）都要用它。留在 `anjia/` 里，app 的默认路径就得反过来依赖一个功能模块 —— 依赖方向错。
//!
//! `PROJECT_SHAPE.md` §8.4：**地址与凭据归 app 管**（本机条目 = 约定地址 + 免钥匙）；
//! 完整「家的管理模型」（远程条目、钥匙、多条目）**不在这里**——这里没有远程、没有钥匙。
//!
//! 口径一律**照 `scripts/home/` 现做法**（那是同一批家事实的唯一既有实现）：
//!
//! - 家的默认位置按平台（`paths.js`）：Windows `D:/celira-home`，别的平台 `~/celira-home`；
//!   `CELIRA_HOME_DIR` 可换（**只读环境，不写任何地方**）；
//! - 世界地址 = `home.conf` 的 `listen_addr`（缺省 `127.0.0.1:8080`）——**loopback、免钥匙**；
//! - 家的布局按形态判据（`paths.js` 的 `detectLayout`）：发布形态 `bin/`，源码形态
//!   `target/release` + `agent/target/release`；
//! - Agent 的后端凭据存储（`credentials.js`）：Windows `%LOCALAPPDATA%\Celira\credentials`，
//!   别的平台 `$XDG_CONFIG_HOME/celira/credentials`（缺省 `~/.config/celira/credentials`）；
//!   `CELIRA_CREDENTIALS_DIR` 可覆盖（测试与 CI 用）。**一个身份一个文件**（0600）。
//!
//! ## 读 vs 写（护栏）
//!
//! - `home.conf` **读为主**；**唯一允许的写**是 `human_id` 一个键，且只在一种场合：
//!   本机世界的**首次布置由壳完成**（建了首位 Human ⇒ 这个 id 必须记下来，否则
//!   `home.js` 下次跑会以为还没铸人、再去 `bootstrap-human` 一次）。写法逐字照
//!   `conf.js` 的 `confSet`（同键老行删掉、新行追加在末尾、统一 LF、其余行含 `mcp_` 原样保留）
//!   —— 见 [`crate::local_world`] 的 `conf_set_text`。**同一个家同一时刻只有一只布置的手**
//!   （有壳时是壳，无壳时是 `home.js`），两者写的是同一份文件、同一套键。
//!   其余键（`listen_addr` / 端口 / `home_id`）壳**一律不写**。
//! - 凭据存储**写**（安家六步里「签后端凭据」的落盘），口径与 `credentials.js` 逐字段对齐：
//!   一份事实一个权威位置，两个写者就是抢格子。
//! - 壳不存 Human token、不存家钥匙（护栏）：token 只在内存里交给页面一次。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 世界默认地址（`paths.js` 的 `DEFAULT_LISTEN`，一字不差）。
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8080";

// ---------------------------------------------------------------------------
// 家在哪
// ---------------------------------------------------------------------------

/// 家的默认位置按平台（`paths.js` `DEFAULT_HOME` 同一口径）。
pub fn default_home_dir() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from("D:/celira-home")
    } else {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("~"));
        home.join("celira-home")
    }
}

/// 相对路径按当前目录展开（JS 侧 `path.resolve` 的等价物；`std::path::absolute`
/// 要 1.79，而本 crate 的 `rust-version` 是 1.77.2，所以自己来这一下）。
fn abspath(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// **家的位置**：显式覆盖（测试/将来条目）→ `CELIRA_HOME_DIR` → 平台默认。
pub fn resolve_home_dir(explicit: Option<&Path>) -> PathBuf {
    if let Some(path) = explicit {
        return abspath(path);
    }
    if let Some(value) = std::env::var_os("CELIRA_HOME_DIR") {
        if !value.is_empty() {
            return abspath(Path::new(&value));
        }
    }
    default_home_dir()
}

// ---------------------------------------------------------------------------
// home.conf（只读）
// ---------------------------------------------------------------------------

/// `home.conf` 里壳用得到的那几格。**只读**。
///
/// `status_port` / `config_port`（2026-10-03 补）是**家的门**两个端面所在的口：
/// 页面要按它们组"本机家条目"的地址（§9.3：app 里家条目统一为"地址 + 钥匙"，
/// 本机那条只是地址为 loopback 的一条）。缺省值在 `paths.js`（5190 / 5191），
/// 这里只如实交出文件里写了什么 —— 兜默认是调用方的事。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HomeConf {
    pub listen_addr: Option<String>,
    pub human_id: Option<String>,
    pub agent_id: Option<String>,
    pub room_id: Option<String>,
    /// **家的 id**（住户化那一步铸的）：有它 = 新形状的家（键按家取）；
    /// 没有它而 `agent_id` 还在 = **更老的那一档**（世界级键，换形状会在世界里另建一支）。
    pub home_id: Option<String>,
    pub status_port: Option<String>,
    pub config_port: Option<String>,
    /// **家级的终端档默认**（`terminal=full|none`；2026-10-14 主人裁那一格）。
    ///
    /// 壳起 Agent 时要用它：档位是**支级事实**（`agents/<支>/agent.conf` 优先），家级这一行
    /// 只是默认；**两处都没写 ⇒ 不给手**。判据的权威是 `scripts/home/residents.js` 的
    /// `residentHands` ＋ `resolveTerminal` —— 这里只如实交出文件里写了什么。
    pub terminal: Option<String>,
    /// **外部工具的授权面**：`mcp_<名>=<transport>:<命令>` 那些行，按**文件里的顺序**交出
    /// **剥掉 `mcp_` 前缀**之后的声明（`<名>=<transport>:<命令>`），与 `conf.js` 的
    /// `mcpDeclarations` 逐条同形。
    ///
    /// ⚠ **不在这里解析、也不校验**：等号右边原样交给宿主，解析只有一处权威（宿主入口）。
    /// 裸的 `mcp_`（前缀后面什么都没有）那一行跳过；`mcp_=` 这类畸形行照旧交出 `=`，
    /// 由宿主入口去报——本层不许"顺手修好"，那会让配置的值与运行的值悄悄不一致。
    pub mcp: Vec<String>,
}

/// 解析 `home.conf` 的正文（与 `conf.js` 同三条纪律：去行尾 CR、只认行首 `键=`、
/// 值里可以含 `=` 只按第一个等号切）。空值按「没写」处理（`conf.js` 会把空串
/// 交出去，但那一格在两边都是读不出东西的坏形状——这里明确当缺席）。
pub fn parse_conf(text: &str) -> HomeConf {
    let mut conf = HomeConf::default();
    let mut set = |key: &str, value: String| {
        if value.is_empty() {
            return;
        }
        match key {
            "listen_addr" => conf.listen_addr = Some(value),
            "human_id" => conf.human_id = Some(value),
            "agent_id" => conf.agent_id = Some(value),
            "room_id" => conf.room_id = Some(value),
            "home_id" => conf.home_id = Some(value),
            "status_port" => conf.status_port = Some(value),
            "config_port" => conf.config_port = Some(value),
            "terminal" => conf.terminal = Some(value),
            key if key.starts_with("mcp_") => {
                let name = &key["mcp_".len()..];
                // 裸的 `mcp_` 走掉；名字与值都非空才算一条声明（值与 `conf.js` 同形）。
                if !name.is_empty() {
                    conf.mcp.push(format!("{name}={value}"));
                }
            }
            _ => {}
        }
    };
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let Some(at) = line.find('=') else { continue };
        let (key, rest) = line.split_at(at);
        set(key, rest[1..].to_string());
    }
    conf
}

/// 读一个家的 `home.conf`；文件不在/读不出来 ⇒ 全空（调用方按「家还没布置过」处置）。
pub fn read_conf(home_dir: &Path) -> HomeConf {
    match fs::read_to_string(home_dir.join("home.conf")) {
        Ok(text) => parse_conf(&text),
        Err(_) => HomeConf::default(),
    }
}

/// **环境根记的是哪一支**（`identity.json` 的 `{agent: <id>}`；`readEnvIdentity` 的直译）。
///
/// 严格照 JS：文件不在 / 读不懂 / `agent` 不是非空字符串 ⇒ `None`（**不猜**）。
/// 两个消费者共用这一份：状态口那一格（`home_doors`）与家的形状迁移（`home_shape`）。
pub fn read_env_identity(env_root: &Path) -> Option<String> {
    let text = fs::read_to_string(env_root.join("identity.json")).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&text).ok()?;
    let agent = parsed.get("agent")?.as_str()?.trim().to_string();
    if agent.is_empty() {
        None
    } else {
        Some(agent)
    }
}

/// 世界地址：`home.conf` 的 `listen_addr`，缺省 `127.0.0.1:8080`。
pub fn listen_addr(conf: &HomeConf) -> String {
    conf.listen_addr
        .clone()
        .unwrap_or_else(|| DEFAULT_LISTEN.to_string())
}

// ---------------------------------------------------------------------------
// 家的布局与可执行件（`paths.js` `detectLayout` 的直译）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutKind {
    /// 发布形态（解压 zip）：可执行件从 `bin/` 来（判据 = `bin/agent_host` 在）。
    Release,
    /// 源码形态（clone）：`target/release` + `agent/target/release`。
    Source,
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub kind: LayoutKind,
    pub repo: PathBuf,
    pub im_exe: PathBuf,
    pub host_exe: PathBuf,
    pub run_exe: PathBuf,
    /// **只读处境的那一次 CLI**（`agent_host_status`；`paths.js` 的 `statusExe`）——
    /// 状态口把"处境"**委派**给它（同一个权威，见 [`crate::home_doors`]）。
    pub status_exe: PathBuf,
}

fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

fn is_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_file())
        .unwrap_or(false)
}

/// `paths.js` 的 `exeIn`：`<base>.exe` 优先，其次无后缀名；两样都没有时返回
/// **本平台**该有的那个名字（`paths.js` 2026-10-01 起同款——错误信息要说的是
/// 「**这台机器上**它该叫什么」；从前一律返回 `.exe`，于是 Linux 上会报出一个
/// 这台机器上永远不会有的名字，把「包里没有 core」和「少了一个文件」混成一件事）。
fn exe_in(dir: &Path, base: &str) -> PathBuf {
    let win = dir.join(format!("{base}.exe"));
    if is_file(&win) {
        return win;
    }
    let plain = dir.join(base);
    if is_file(&plain) {
        return plain;
    }
    dir.join(exe_name(base))
}

/// 形态判据（与 `paths.js` 的 `detectLayout` 同一条，**两问，不再是一问**，2026-09-30）：
///
/// ① `packaged`：我在一个**发布包**里吗？判据 = `bin/` 里有 **Agent 的可执行件**
///    （`agent_host[.exe]`）—— 那是「可执行件从哪儿来」这个问题的**直接**答案；
/// ② core（`celira-im`）在不在**不是**形态判据：它只决定**编排**（本机全量 vs 接远端世界）。
///    从前把两问合成一问（「`bin/celira-im` 在不在」），于是 **runtime 子包**（按设计不含
///    core）会把自己判成源码形态，反过来要用户去 `cargo build`。
pub fn detect_layout(repo: &Path) -> Layout {
    let bin = repo.join("bin");
    let packaged = is_file(&bin.join("agent_host.exe")) || is_file(&bin.join("agent_host"));
    let agent_dir = if packaged {
        bin.clone()
    } else {
        repo.join("agent").join("target").join("release")
    };
    let im_dir = if packaged {
        bin
    } else {
        repo.join("target").join("release")
    };
    Layout {
        kind: if packaged {
            LayoutKind::Release
        } else {
            LayoutKind::Source
        },
        repo: repo.to_path_buf(),
        im_exe: exe_in(&im_dir, "celira-im"),
        host_exe: exe_in(&agent_dir, "agent_host"),
        run_exe: exe_in(&agent_dir, "agent_run"),
        status_exe: exe_in(&agent_dir, "agent_host_status"),
    }
}

/// 这个目录像不像一个「家/仓库根」？（布局里至少有一个可执行件对得上）
fn looks_like_root(dir: &Path) -> bool {
    let layout = detect_layout(dir);
    is_file(&layout.im_exe) || is_file(&layout.host_exe) || is_file(&layout.run_exe)
}

/// **仓库根/解压根**的位置：显式覆盖（测试）→ `CELIRA_REPO_DIR` → 从当前可执行件
/// 往上找（开发形态：`app/src-tauri/target/…` 的若干层之上就是仓库根）。
///
/// ⚠ 找不到 ⇒ `None`，调用方如实报「缺可执行件」——**不猜、不编路径**
/// （壳的目标形态是 Windows 桌面，装好的壳里这一找必然落空；那一格怎么合流
/// 属后续刀，本刀不发明发布布局）。
pub fn resolve_repo_dir(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return Some(abspath(path));
    }
    if let Some(value) = std::env::var_os("CELIRA_REPO_DIR") {
        if !value.is_empty() {
            return Some(abspath(Path::new(&value)));
        }
    }
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?.to_path_buf();
    for _ in 0..8 {
        if looks_like_root(&dir) {
            return Some(dir);
        }
        dir = dir.parent()?.to_path_buf();
    }
    None
}

// ---------------------------------------------------------------------------
// 后端凭据的本地存储（`credentials.js` 的直译）
// ---------------------------------------------------------------------------

/// 凭据目录的**纯函数**版本（只看传进来的输入，便于判错）。
///
/// 与 `credentials.js` 逐句对齐：覆盖目录 → Windows `LOCALAPPDATA` →
/// `XDG_CONFIG_HOME`/`~/.config`（Windows 上 `LOCALAPPDATA` 缺席时同样落到
/// `~/.config` 那一支——JS 的行为就是这样，注释与代码打架时**以代码为准**）。
pub fn credentials_dir_from(
    override_dir: Option<&str>,
    local_app_data: Option<&str>,
    xdg_config_home: Option<&str>,
    user_home: &Path,
    is_windows: bool,
) -> PathBuf {
    let override_trimmed = override_dir.unwrap_or("").trim();
    if !override_trimmed.is_empty() {
        return abspath(Path::new(override_trimmed));
    }
    if is_windows {
        let local = local_app_data.unwrap_or("").trim();
        if !local.is_empty() {
            return Path::new(local).join("Celira").join("credentials");
        }
    }
    let xdg = xdg_config_home.unwrap_or("").trim();
    let base = if !xdg.is_empty() {
        PathBuf::from(xdg)
    } else {
        user_home.join(".config")
    };
    base.join("celira").join("credentials")
}

/// 读当前环境的凭据目录。
pub fn credentials_dir() -> PathBuf {
    let override_dir = std::env::var("CELIRA_CREDENTIALS_DIR").ok();
    let local_app_data = std::env::var("LOCALAPPDATA").ok();
    let xdg = std::env::var("XDG_CONFIG_HOME").ok();
    let user_home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("~"));
    credentials_dir_from(
        override_dir.as_deref(),
        local_app_data.as_deref(),
        xdg.as_deref(),
        &user_home,
        cfg!(windows),
    )
}

// ---------------------------------------------------------------------------
// 凭据存储的**形状**（`credentials.js` 是权威，2026-10-03 对齐）
// ---------------------------------------------------------------------------
//
// ⚠ 从前这一片只有**老形状**（`<目录>/<身份>.token`），而 JS 那边早已改成
// **按「世界 × 身份」分条**（`<目录>/<世界摘要>/<身份>.token`）——壳于是成了"第二只写歪的手"：
// 单世界今天不炸，多世界就会串。这里逐条对齐 JS：
//
// | 事实 | 形状 | 权威 |
// |---|---|---|
// | 世界那一维 | `sha256(normalizeWorld(世界地址))` 的前 16 位十六进制 | `credentials.js` 的 `worldKey` |
// | 新形状（**唯一写路径**） | `<目录>/<世界摘要>/<身份>.token` | `credentialFile({world, agentId})` |
// | 老形状（**只读回落**） | `<目录>/<身份>.token` | `legacyCredentialFile`（迁移桥） |

/// 世界摘要的长度（`credentials.js` 的 `WORLD_KEY_LENGTH`）。
pub const WORLD_KEY_LENGTH: usize = 16;

/// 世界地址的**规范化形状**（`normalizeWorld` 直译）：`协议//主机[:端口][路径]`，
/// 小写主机、去尾斜杠。读不懂的取值退回"去尾斜杠的原样"（仍然稳定，只是不享规范化红利）。
pub fn normalize_world(base: &str) -> String {
    let raw = base.trim();
    if raw.is_empty() {
        return String::new();
    }
    match url::Url::parse(raw) {
        Ok(url) => {
            let host = url.host_str().unwrap_or("").to_lowercase();
            let port = match url.port() {
                Some(port) => format!(":{port}"),
                None => String::new(),
            };
            let path = url.path().trim_end_matches('/');
            format!("{}://{host}{port}{path}", url.scheme())
        }
        Err(_) => raw.trim_end_matches('/').to_string(),
    }
}

/// 世界那一维的**稳定摘要**（`worldKey` 直译）。世界地址缺席 ⇒ `None`
/// （调用方按"这一维缺了"处置，**不**静默退回只按身份 —— 静默退回就是让两支撞车）。
pub fn world_key(world: &str) -> Option<String> {
    use sha2::{Digest, Sha256};
    let normalized = normalize_world(world);
    if normalized.is_empty() {
        return None;
    }
    let digest = Sha256::digest(normalized.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Some(hex[..WORLD_KEY_LENGTH].to_string())
}

/// 身份 id 只允许安全字符（它会被拼进文件名；`safeAgentId` 直译）。
fn safe_agent_id(agent_id: &str) -> Option<String> {
    let id = agent_id.trim();
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return None;
    }
    Some(id.to_string())
}

/// **新形状**（唯一写路径）：`<目录>/<世界摘要>/<身份>.token`。
/// 世界那一维缺了 / 身份 id 不合法 ⇒ `None`（**不**退回老形状）。
pub fn credential_file(dir: &Path, world: &str, agent_id: &str) -> Option<PathBuf> {
    let id = safe_agent_id(agent_id)?;
    let key = world_key(world)?;
    Some(dir.join(key).join(format!("{id}.token")))
}

/// **老形状**（只读回落）：`<目录>/<身份>.token`（`legacyCredentialFile` 直译）。
pub fn legacy_credential_file(dir: &Path, agent_id: &str) -> Option<PathBuf> {
    let id = safe_agent_id(agent_id)?;
    Some(dir.join(format!("{id}.token")))
}

/// 一个身份凭据的读取三态（「没有」与「读不出来」分开——`credentials.js` 同款）。
#[derive(Debug, Clone)]
pub enum CredentialRead {
    Present(String),
    Missing,
    Unreadable(String),
}

/// 读一份文件的三态（`readAt` 直译）。
fn read_at(file: &Path) -> CredentialRead {
    match fs::read_to_string(file) {
        Ok(text) => {
            let token = text.trim();
            if token.is_empty() {
                CredentialRead::Unreadable("empty".to_string())
            } else {
                CredentialRead::Present(token.to_string())
            }
        }
        Err(error) => {
            if error.kind() == std::io::ErrorKind::NotFound {
                CredentialRead::Missing
            } else {
                CredentialRead::Unreadable("read-failed".to_string())
            }
        }
    }
}

/// 读一支身份的后端凭据：**新形状优先，老形状回落**（`resolve` 的次序）。
pub fn read_credential(dir: &Path, world: &str, agent_id: &str) -> CredentialRead {
    if let Some(file) = credential_file(dir, world, agent_id) {
        match read_at(&file) {
            CredentialRead::Missing => {}
            found => return found,
        }
    }
    match legacy_credential_file(dir, agent_id) {
        Some(file) => read_at(&file),
        None => CredentialRead::Missing,
    }
}

/// 老形状那份文件在哪（**迁移用**：把它搬到新形状；调用方决定什么时候搬）。
pub fn legacy_credential_path(dir: &Path, agent_id: &str) -> Option<PathBuf> {
    legacy_credential_file(dir, agent_id)
}

/// 写一个身份的凭据（**原子**：同目录临时文件 + rename；0600 是 POSIX 上的意思，
/// Windows 上权限随用户目录 ACL）。**已经存在但读不出来时拒绝覆盖**（照
/// `credentials.js`：那多半是别人的文件/坏文件，覆盖它等于替主人做决定）。
///
/// ⚠ 写路径**只认新形状**（按「世界 × 身份」分条）：世界那一维缺了 ⇒ 拒绝 ——
/// 与 `credentials.js` 的"静默退回就是让两支撞车"同一条判据。
pub fn write_credential(
    dir: &Path,
    world: &str,
    agent_id: &str,
    token: &str,
) -> Result<PathBuf, String> {
    let value = token.trim();
    if value.is_empty() {
        return Err("凭据是空的 —— 不写（空值会让\"缺凭据\"变成\"静默坏掉\"）".to_string());
    }
    let Some(file) = credential_file(dir, world, agent_id) else {
        return Err(format!(
            "凭据的落点算不出来（世界「{world}」或身份「{agent_id}」读不懂）—— 不写"
        ));
    };
    if let CredentialRead::Unreadable(reason) = read_at(&file) {
        return Err(format!(
            "凭据文件读不出来（{}：{reason}）—— 先看清它再重试",
            file.display()
        ));
    }
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("建凭据目录失败：{error}"))?;
    }
    let temp = file.with_file_name(format!(
        ".{}.{}.tmp",
        file.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "credential".to_string()),
        std::process::id()
    ));
    let write_result = (|| -> Result<(), String> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut handle = options
            .open(&temp)
            .map_err(|error| format!("写临时凭据文件失败：{error}"))?;
        handle
            .write_all(format!("{value}\n").as_bytes())
            .map_err(|error| format!("写临时凭据文件失败：{error}"))?;
        Ok(())
    })();
    if let Err(message) = write_result {
        let _ = fs::remove_file(&temp);
        return Err(message);
    }
    if let Err(error) = fs::rename(&temp, &file) {
        let _ = fs::remove_file(&temp);
        return Err(format!("凭据落盘失败：{error}"));
    }
    Ok(file)
}

// ---------------------------------------------------------------------------
// 「这支 Agent 是否已经安家」（criterion 3，承 PROJECT_SHAPE §8.0）
// ---------------------------------------------------------------------------

/// 本机「已安家」的三种来路（三态不许压成两态——报告要说清是哪儿）。
#[derive(Debug, Clone)]
pub enum HomedAt {
    /// 壳这一路的环境根：`<家>/agents/<id>/`（一支一个）。
    ShellRoot(PathBuf),
    /// `home.js` 那位住户的环境根：`<家>/env/`。
    HomeEnv(PathBuf),
    /// 家的住户登记：`home.conf` 的 `agent_id`（布置过、可能环境还没建完）。
    HomeConf,
}

#[derive(Debug, Clone)]
pub enum Plan {
    /// 可以安家（把六步往下走）。
    Adopt { agent_id: String },
    /// 这台机器上已经有它的家——**什么都不动**（判据 3：不是「这个家有没有主」，
    /// 是「这支有没有家」）。
    Already { agent_id: String, at: HomedAt },
    /// 形状不对的 id（空 / 带空格 / 超长——照 `adopt.js` 的 `refuse-bad-id`）。
    RefuseBadId,
}

/// 身份文件是不是记录着这支 Agent（`{"agent":"<id>"}` 那一格）。
fn identity_records_at(file: &Path, agent_id: &str) -> bool {
    let Ok(text) = fs::read_to_string(file) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    value.get("agent").and_then(|v| v.as_str()) == Some(agent_id)
}

/// **安家的判据 3**：这支 Agent 在不在这台机器上已经有家？
///
/// 三条来路逐一检查（全部**只读**）：
/// 1. 壳布局 `<家>/agents/<id>/identity.json` 在 ⇒ 就是它；
/// 2. `home.js` 布局 `<家>/env/identity.json` 记的是它 ⇒ 就是它（别的 id 不算）；
/// 3. `home.conf` 的 `agent_id` 是它 ⇒ 它是这个家的住户（`home.js` 的地盘，
///    壳不去造第二个身体）。
pub fn plan_anjia(home_dir: &Path, conf: &HomeConf, requested: &str) -> Plan {
    let want = requested.trim().to_string();
    if want.is_empty() || want.contains(' ') || want.chars().count() > 200 {
        return Plan::RefuseBadId;
    }
    let shell_root = home_dir.join("agents").join(&want);
    if is_file(&shell_root.join("identity.json")) {
        return Plan::Already {
            agent_id: want.clone(),
            at: HomedAt::ShellRoot(shell_root),
        };
    }
    let home_env = home_dir.join("env");
    if identity_records_at(&home_env.join("identity.json"), &want) {
        return Plan::Already {
            agent_id: want.clone(),
            at: HomedAt::HomeEnv(home_env),
        };
    }
    if conf.agent_id.as_deref() == Some(want.as_str()) {
        return Plan::Already {
            agent_id: want.clone(),
            at: HomedAt::HomeConf,
        };
    }
    Plan::Adopt { agent_id: want }
}

/// 一支 Agent 在壳布局里的环境根（一支一个：`<家>/agents/<id>/`）。
pub fn agent_env_root(home_dir: &Path, agent_id: &str) -> PathBuf {
    home_dir.join("agents").join(agent_id.trim())
}

/// 资料根：`<家>/materials/`（宿主 `--materials` 给的就是它；与 `up.js` 的 `materialsRoot` 同一格）。
pub fn materials_root(home_dir: &Path) -> PathBuf {
    home_dir.join("materials")
}

/// 技能区：`<家>/materials/skills/`（**建家时连空目录一起建**）。
///
/// 判据与 CLI 那边同一条（`scripts/home/paths.js` 的 `skillsRoot`）：Agent 侧
/// `skills_desktop` 把 `None`（没有技能区 ⇒ 整块不出场）与 `Some(0)`（有、但空 ⇒
/// `skills: 0 项（技能区为空）`）当成两件事。不建它，"这具身体没有技能区"与"技能区空着"
/// 在主体眼里合成一片空白——真机上主体据此答成"没有可安装的技能包"。
pub fn skills_dir(home_dir: &Path) -> PathBuf {
    materials_root(home_dir).join("skills")
}

// ---------------------------------------------------------------------------
// 模型那一半的两条路径（`model-sources.js` 的读口径，直译）
// ---------------------------------------------------------------------------

/// 模型**清单**（机器级那份）的路径（`model-sources.js` 的 `listPath`）。
pub fn model_sources_list_path(home_dir: &Path) -> PathBuf {
    home_dir.join("model_sources.json")
}

/// **搜索源清单**（机器级那份）的路径（`web-sources.js` 的 `listPath`，2026-10-20）。
///
/// 与模型那一份同级同款：宿主只拿到**清单**的路径，"这一支用哪一条"由宿主自己去环境根读绑定。
pub fn web_sources_list_path(home_dir: &Path) -> PathBuf {
    home_dir.join("web_sources.json")
}

/// `<环境根>/model_binding.json` 的三态（`model-sources.js` 的 `readBinding` 直译）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelBinding {
    /// 绑定不在场（这台环境根走进程侧 env 那一档）。
    Absent,
    /// 绑定在场：这一支用清单里的哪一条 ＋（可选）**支级覆盖那两格**。
    ///
    /// ⚠ **2026-10-14 加宽**：`model` / `reasoning_effort` 从前读侧看不见，而运行时**一直**认
    /// 它们（`agent/src/model_source.rs` 的 `apply_binding_overrides`：绑定的型号/档位**赢过**
    /// 条目里的）⇒ 页面上只能说"用哪一条来源"，说不出"**具体哪个型号**"。现在如实带出来
    /// （`None` = 没覆盖）。
    Bound {
        source: String,
        model: Option<String>,
        reasoning_effort: Option<String>,
    },
    /// 文件在场但读不懂（**不是**缺席——宿主会据此拒绝启动，壳照实报）。
    Unreadable { reason: String },
}

/// **支级覆盖那两格的形状预检**（照运行时读侧那份口径，`model-sources.js` 同款）。
///
/// 型号：**非空字符串**（允许空格与 `/` `:` 这类字符——"meta-llama/Llama-3-70b"、":free"
/// 都是真的型号名），禁控制字符，长度 ≤200 字节。
fn model_shape_ok(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && value.len() <= crate::model_sources::MODEL_MAX_BYTES
        && !value.chars().any(|each| each.is_control())
}

/// 读绑定那一份文件（`model-sources.js` 的 `readBinding` **逐条同形**）。
///
/// 那两格的 fail-closed 口径与运行时**必须一样**：型号不是非空字符串、档位不在封闭表里
/// ⇒ 整份读成 `Unreadable`（**不是**"没覆盖"）—— 那种文件宿主会拒绝启动，壳说"没覆盖"
/// 就是把"配置坏了"说成"没配"。
pub fn read_model_binding(env_root: &Path) -> ModelBinding {
    let file = env_root.join("model_binding.json");
    let Ok(text) = fs::read_to_string(&file) else {
        return ModelBinding::Absent;
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) else {
        return ModelBinding::Unreadable {
            reason: "not_json".to_string(),
        };
    };
    let source = parsed
        .get("source")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(source) = source else {
        return ModelBinding::Unreadable {
            reason: "missing_source".to_string(),
        };
    };
    // `null` 与"没写"同义（写者清掉覆盖时就是这么写的）。
    let model = match parsed.get("model") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(value)) if model_shape_ok(value) => {
            Some(value.trim().to_string())
        }
        Some(_) => {
            return ModelBinding::Unreadable {
                reason: "bad_model".to_string(),
            }
        }
    };
    let reasoning_effort = match parsed.get("reasoning_effort") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(value))
            if crate::model_sources::REASONING_EFFORTS.contains(&value.trim()) =>
        {
            Some(value.trim().to_string())
        }
        Some(_) => {
            return ModelBinding::Unreadable {
                reason: "bad_reasoning_effort".to_string(),
            }
        }
    };
    ModelBinding::Bound {
        source: source.to_string(),
        model,
        reasoning_effort,
    }
}

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "celira-anjia-test-{}-{}-{}",
            std::process::id(),
            tag,
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn conf_parsing_takes_first_equals_and_tolerates_crlf() {
        let text = "listen_addr=127.0.0.1:8090\r\nhuman_id=hum-1\r\nagent_id=a-1\r\n\
                    mcp_x=stdio:cmd --flag=1\r\nroom_id=\r\n";
        let conf = parse_conf(text);
        assert_eq!(conf.listen_addr.as_deref(), Some("127.0.0.1:8090"));
        assert_eq!(conf.human_id.as_deref(), Some("hum-1"));
        assert_eq!(conf.agent_id.as_deref(), Some("a-1"));
        // 空值按「没写」处理（两边都读不出东西的坏形状）。
        assert_eq!(conf.room_id, None);
        // 外部工具的声明：剥掉 `mcp_` 前缀、等号右边原样（与 `conf.js` 同形）。
        assert_eq!(conf.mcp, vec!["x=stdio:cmd --flag=1".to_string()]);
    }

    /// **`mcp_` 那些行**：按文件顺序交出、只剥前缀不解析；裸的 `mcp_` 跳过。
    #[test]
    fn mcp_declarations_keep_file_order_and_skip_the_bare_prefix() {
        let text = "mcp_browser=stdio:npx @playwright/mcp\n\
                    mcp_=oops\n\
                    mcp_db=stdio:node db.js,read-only,timeout=30\n\
                    terminal=full\n";
        let conf = parse_conf(text);
        assert_eq!(
            conf.mcp,
            [
                "browser=stdio:npx @playwright/mcp".to_string(),
                "db=stdio:node db.js,read-only,timeout=30".to_string(),
            ],
            "顺序是文件顺序；裸 `mcp_` 不进；值一个字不动"
        );
        assert_eq!(conf.terminal.as_deref(), Some("full"), "别的键照旧");
    }

    #[test]
    fn missing_conf_yields_defaults() {
        let home = temp_dir("missing-conf");
        let conf = read_conf(&home);
        assert_eq!(conf, HomeConf::default());
        assert_eq!(listen_addr(&conf), DEFAULT_LISTEN);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn credentials_dir_follows_scripts_home_precedence() {
        // ⚠ **平台无关的写法**（2026-10-03）：这几个输入都必须是**本平台意义上的绝对路径**
        // —— `abspath` 对"不绝对"的路径按当前目录展开，于是 `/tmp/c` 在 Windows 上会被展开成
        // `<盘>:\tmp\c`（`/tmp/c` 在 Windows 上**不是**绝对路径：有根、没有盘符）。
        // 从前这里写死 `/tmp/c` / `/xdg`，于是这条用例只在 Linux 上绿（Windows 上必红）。
        let override_dir = temp_dir("creds-override");
        let xdg_dir = temp_dir("creds-xdg");
        let user_home = temp_dir("creds-home");

        // 覆盖目录最优先（原样用）。
        assert_eq!(
            credentials_dir_from(
                Some(&override_dir.display().to_string()),
                Some("C:/local"),
                Some(&xdg_dir.display().to_string()),
                &user_home,
                false
            ),
            override_dir
        );
        // Windows 的 LOCALAPPDATA。
        assert_eq!(
            credentials_dir_from(
                None,
                Some("C:/Users/u/AppData/Local"),
                None,
                &user_home,
                true
            ),
            PathBuf::from("C:/Users/u/AppData/Local/Celira/credentials")
        );
        // Windows 上 LOCALAPPDATA 缺席 ⇒ 落到 xdg/~.config 一支（JS 代码如此）。
        assert_eq!(
            credentials_dir_from(None, None, None, &user_home, true),
            user_home.join(".config/celira/credentials")
        );
        // XDG 优先于 ~/.config。
        assert_eq!(
            credentials_dir_from(
                None,
                None,
                Some(&xdg_dir.display().to_string()),
                &user_home,
                false
            ),
            xdg_dir.join("celira/credentials")
        );
        assert_eq!(
            credentials_dir_from(None, None, None, &user_home, false),
            user_home.join(".config/celira/credentials")
        );

        let _ = fs::remove_dir_all(&override_dir);
        let _ = fs::remove_dir_all(&xdg_dir);
    }

    /// 世界摘要：**对着 `credentials.js` 算出来的真值钉住**（不是自己算一遍再对答案）——
    /// 三个输入与它们的摘要都是从权威实现那次调用里抄下来的（2026-10-03）。
    #[test]
    fn world_key_matches_the_authority() {
        assert_eq!(
            normalize_world("http://127.0.0.1:8080"),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            world_key("http://127.0.0.1:8080").as_deref(),
            Some("d30a576c03187167")
        );
        // 尾斜杠 / 大写协议 ⇒ 同一个世界（规范化红利）。
        assert_eq!(
            world_key("http://127.0.0.1:8080/").as_deref(),
            Some("d30a576c03187167")
        );
        assert_eq!(
            world_key("HTTP://127.0.0.1:8080").as_deref(),
            Some("d30a576c03187167")
        );
        // 主机名不同 ⇒ 另一个世界（小写是规范化，不等于把主机名改掉）。
        assert_eq!(
            world_key("http://LOCALHOST:8080").as_deref(),
            Some("a76d8c3e94eba61f")
        );
        // 没有协议 ⇒ 按"去尾斜杠的原样"算（与 JS 的 catch 那支同款）。
        assert_eq!(
            world_key("127.0.0.1:8080").as_deref(),
            Some("b678fa77dde442f6")
        );
        // 世界地址缺席 ⇒ 这一维缺了（**不**静默退回只按身份）。
        assert_eq!(world_key(""), None);
        assert_eq!(world_key("   "), None);
    }

    #[test]
    fn credential_file_is_world_scoped_and_legacy_is_read_only() {
        let dir = PathBuf::from("/tmp/creds");
        let world = "http://127.0.0.1:8080";
        // 新形状：`<目录>/<世界摘要>/<身份>.token`。
        assert_eq!(
            credential_file(&dir, world, "1c2f8a10-3f5e-4a01-9a6d-2f4d0a1b2c3d"),
            Some(
                dir.join("d30a576c03187167")
                    .join("1c2f8a10-3f5e-4a01-9a6d-2f4d0a1b2c3d.token")
            )
        );
        // 身份 id 不能当文件名 ⇒ 拒绝（两条形状都拒）。
        assert!(credential_file(&dir, world, "").is_none());
        assert!(credential_file(&dir, world, "a/b").is_none());
        assert!(credential_file(&dir, world, "a b").is_none());
        assert!(legacy_credential_file(&dir, "a/b").is_none());
        // **世界那一维缺了 ⇒ 新形状算不出来**（写路径据此拒绝；老形状只管读）。
        assert!(credential_file(&dir, "", "agent-x").is_none());
        assert_eq!(
            legacy_credential_file(&dir, "agent-x"),
            Some(dir.join("agent-x.token"))
        );
    }

    #[test]
    fn credential_round_trip_and_refuse_over_unreadable() {
        let dir = temp_dir("cred-roundtrip");
        let world = "http://127.0.0.1:8080";
        let id = "agent-x";
        assert!(matches!(
            read_credential(&dir, world, id),
            CredentialRead::Missing
        ));
        // 写路径**只认新形状**。
        let file = write_credential(&dir, world, id, "tok-1").unwrap();
        assert_eq!(
            file,
            dir.join("d30a576c03187167").join("agent-x.token"),
            "凭据落在按世界分条的那一格"
        );
        assert!(matches!(
            read_credential(&dir, world, id),
            CredentialRead::Present(t) if t == "tok-1"
        ));
        // 空凭据拒绝；世界那一维缺了也拒绝（**不**静默退回老形状）。
        assert!(write_credential(&dir, world, id, "  ").is_err());
        assert!(write_credential(&dir, "", id, "tok-1").is_err());
        // 存在但读不出来（空文件）⇒ 拒绝覆盖。
        fs::write(&file, "").unwrap();
        assert!(matches!(
            read_credential(&dir, world, id),
            CredentialRead::Unreadable(_)
        ));
        assert!(write_credential(&dir, world, id, "tok-2").is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::write(&file, "tok-3\n").unwrap();
            let _ = fs::set_permissions(&file, fs::Permissions::from_mode(0o600));
            let mode = fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "凭据文件应当 0600");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// 老形状（扁平那份）**读得到**（迁移桥），而且新形状在时**新形状优先**。
    #[test]
    fn legacy_flat_credential_is_read_but_never_written() {
        let dir = temp_dir("cred-legacy");
        let world = "http://127.0.0.1:8080";
        let id = "agent-x";
        let legacy = legacy_credential_file(&dir, id).unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(&legacy, "tok-legacy\n").unwrap();
        assert!(matches!(
            read_credential(&dir, world, id),
            CredentialRead::Present(t) if t == "tok-legacy"
        ));
        // 新形状那份一出现 ⇒ 以它为准（同一个身份只有一支有效凭据，新旧同时在场时新的是权威）。
        write_credential(&dir, world, id, "tok-new").unwrap();
        assert!(matches!(
            read_credential(&dir, world, id),
            CredentialRead::Present(t) if t == "tok-new"
        ));
        assert_eq!(
            legacy_credential_path(&dir, id),
            Some(legacy),
            "老形状那份的位置仍然算得出来（迁移要用）"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn plan_refuses_malformed_ids() {
        let home = temp_dir("plan-bad-id");
        let conf = HomeConf::default();
        assert!(matches!(plan_anjia(&home, &conf, ""), Plan::RefuseBadId));
        assert!(matches!(plan_anjia(&home, &conf, "  "), Plan::RefuseBadId));
        assert!(matches!(plan_anjia(&home, &conf, "a b"), Plan::RefuseBadId));
        assert!(matches!(
            plan_anjia(&home, &conf, &"x".repeat(201)),
            Plan::RefuseBadId
        ));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn plan_detects_all_three_homed_sources() {
        let home = temp_dir("plan-homed");
        let conf = HomeConf::default();

        // ① 壳布局。
        let shell_root = home.join("agents").join("agent-1");
        fs::create_dir_all(&shell_root).unwrap();
        fs::write(
            shell_root.join("identity.json"),
            "{\"agent\":\"agent-1\"}\n",
        )
        .unwrap();
        match plan_anjia(&home, &conf, "agent-1") {
            Plan::Already {
                at: HomedAt::ShellRoot(_),
                ..
            } => {}
            other => panic!("壳布局没认出来：{other:?}"),
        }

        // ② home.js 的环境根（只认记录的 id）。
        fs::create_dir_all(home.join("env")).unwrap();
        fs::write(
            home.join("env").join("identity.json"),
            "{\"agent\":\"resident\"}\n",
        )
        .unwrap();
        match plan_anjia(&home, &conf, "resident") {
            Plan::Already {
                at: HomedAt::HomeEnv(_),
                ..
            } => {}
            other => panic!("home.js 环境根没认出来：{other:?}"),
        }
        assert!(matches!(
            plan_anjia(&home, &conf, "other-id"),
            Plan::Adopt { .. }
        ));

        // ③ home.conf 的住户登记。
        let conf = HomeConf {
            agent_id: Some("resident".to_string()),
            ..HomeConf::default()
        };
        match plan_anjia(&home, &conf, "resident") {
            Plan::Already { .. } => {}
            other => panic!("住户登记没认出来：{other:?}"),
        }

        // 未安家的 id ⇒ Adopt。
        match plan_anjia(&home, &conf, "fresh") {
            Plan::Adopt { agent_id } => assert_eq!(agent_id, "fresh"),
            other => panic!("新 id 不该被挡住：{other:?}"),
        }
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn layout_resolution_matches_paths_js_shape() {
        let repo = temp_dir("layout");
        // 源码形态（无 bin/）：可执行件按 target/release 与 agent/target/release 找。
        let layout = detect_layout(&repo);
        assert_eq!(layout.kind, LayoutKind::Source);
        assert_eq!(
            layout.im_exe,
            repo.join("target/release").join(exe_name("celira-im"))
        );
        assert_eq!(
            layout.host_exe,
            repo.join("agent/target/release")
                .join(exe_name("agent_host"))
        );
        // **判据不锚在 core 成员上**（`paths.js` 2026-09-30 起两问分开）：bin/ 里只有
        // `celira-im` ⇒ 仍是源码形态（那种形状里"可执行件从哪儿来"并没有答案）。
        fs::create_dir_all(repo.join("bin")).unwrap();
        fs::write(repo.join("bin").join(exe_name("celira-im")), "stub").unwrap();
        assert_eq!(detect_layout(&repo).kind, LayoutKind::Source);
        // 发布形态：bin/ 里有 **Agent 的可执行件**（`agent_host`）⇒ 都去 bin/ ——
        // **runtime 子包**（有臂、不带 core）也算发布形态，不再是"要你 cargo build"。
        fs::write(repo.join("bin").join(exe_name("agent_host")), "stub").unwrap();
        let layout = detect_layout(&repo);
        assert_eq!(layout.kind, LayoutKind::Release);
        assert_eq!(layout.im_exe, repo.join("bin").join(exe_name("celira-im")));
        assert_eq!(layout.run_exe, repo.join("bin").join(exe_name("agent_run")));
        let _ = fs::remove_dir_all(&repo);
    }
}
