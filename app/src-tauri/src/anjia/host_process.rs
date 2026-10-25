//! 安家 · **起它自己的进程**（承施工单 §一 §8.5 第 5 步；口径照 `scripts/home/up.js`）。
//!
//! 本模块只做三件事，**只做这三件**：
//!
//! 1. **参数**（`buildHostArgs` 的直译，顺序是语义）：
//!    `--root <环境根> --agent <id> --materials <资料根> --max-turns 300
//!     [--terminal full（**有终端臂的平台**——Windows 与 Linux）] --im-endpoint <url>
//!     [--model-sources <清单>] [--identity-agent <id> --identity-name <名字>]`；
//! 2. **环境**（`modelEnvironment` 的直译）：绑定在场 ⇒ **一个模型 env 都不给**
//!    （连手边本来就有的那两条也摘掉——文件与进程侧打架时宿主会拒绝启动）；
//!    绑定不在场 ⇒ 给 `DEEPSEEK_API_KEY`（取值来源见 `super::user_env` 的口径）；
//!    `CELIRA_IM_BACKEND_AUTH` 只经环境、不进 argv；
//! 3. **等报到**：读日志里的 `[listening]` / `[exit]` 两行（`up.js` 的 20 秒预算），
//!    **如实分三态**：报到了 / 自己说了原因退了 / 没等到（不替它下结论）。
//!
//! ## 本模块**不做**的事（护栏）
//!
//! - **不做进程管理**：不重启（§8.6-2）、不认领 PID（§8.7）、不写 pid 文件、
//!   不做托盘与退出语义（§8.2-8.3）。进程起完就交给它自己。
//! - 日志只 rotate（旧的一份留档为 `.1`）后新建，不清别的、不动别的家的。

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::world_channel as world;

/// 平台缝：**这台机器上，主体有没有一只「终端」的手**。
///
/// 判据照 `scripts/home/up.js`：① 这台机器有没有终端臂（`isWindows || isLinux`；Linux 自
/// 「第五刀」起有 bash 那只手）；② **这一支有没有显式声明 `terminal=full`**（2026-10-14 主人裁：
/// **缺省不给手**，支级 ⇒ 家级 ⇒ 不给；见 [`terminal_tier`]）。
/// 其余平台（macOS 等）判据里没有臂，**声明了也不给**。三支都能被机械判错（见用例）：
/// 本机是 Linux，Windows 支靠显式传参守住。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPlatform {
    Windows,
    Linux,
    /// 其余平台：判据里没有终端臂。
    Other,
}

impl HostPlatform {
    /// 本机是哪一支（**唯一**读 `cfg!` 的地方）。
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }

    /// 有终端臂的平台（`up.js`：`isWindows || isLinux`）。
    fn has_terminal_arm(self) -> bool {
        matches!(self, Self::Windows | Self::Linux)
    }

    /// 有沙箱 provider 的平台（`up.js`：今天只有 Linux；provider 是否真可用由 runtime 探针回答）。
    fn has_sandbox_arm(self) -> bool {
        matches!(self, Self::Linux)
    }
}

/// 一支的**终端档**（`terminal=`）：`none` / `sandbox` / `full`。
///
/// ⚠ 判据的**权威**是 `scripts/home/residents.js` 的 `residentHands` ＋ `resolveTerminal`
/// （两边必须一起动）：
///   · 支级 `agent.conf` 写了 ⇒ 用它；
///   · 没写 ⇒ 家级 `home.conf` 写了 ⇒ 用它；
///   · 都没写 ⇒ **不给手**（2026-10-14 主人裁：平台默认是不给；要手只能显式写 `full` / `sandbox`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandTier {
    Sandbox,
    Full,
    None,
}

/// 认取值：只有 `none` / `sandbox` / `full` 算数，别的**当没写**（与 JS 的 `known()` 一致：不猜、
/// 不就近映射，认不出就落到"不给"那一档）。
fn tier_of(raw: Option<String>) -> Option<HandTier> {
    match raw?.trim().to_ascii_lowercase().as_str() {
        "none" => Some(HandTier::None),
        "sandbox" => Some(HandTier::Sandbox),
        "full" => Some(HandTier::Full),
        _ => None,
    }
}

/// 这一支最终是哪个档（支级 ⇒ 家级 ⇒ `None`＝不给）。
fn terminal_tier(spec: &HostSpec) -> Option<HandTier> {
    if let Some(tier) = tier_of(crate::registration::read_agent_terminal(&spec.env_root)) {
        return Some(tier);
    }
    tier_of(crate::home_facts::read_conf(&spec.home_dir).terminal)
}

/// 起一支本地执行端需要的全部材料（宿主声明的那些）。
#[derive(Debug, Clone)]
pub struct HostSpec {
    /// 家的位置（`up.js` 的 `homeDir`）——子进程的**工作目录**照它设，
    /// 资料根 = `<家>/materials`（同 `up.js` 的 `path.join(homeDir, 'materials')`）。
    pub home_dir: PathBuf,
    pub env_root: PathBuf,
    pub agent_id: String,
    pub listen: String,
    /// 「是谁」（`GET /v1/backend/me` 同一份回执的两个值；缺 ⇒ 不带那两行）。
    pub identity: Option<(String, String)>,
    /// 模型**清单**（机器级那份）——只在文件档（环境根有绑定）时给。
    pub model_sources_file: Option<PathBuf>,
    /// **搜索源清单**的路径（2026-10-20）：与模型那一格同款——给清单，用哪一条看环境根的绑定。
    /// None = 没声明 ⇒ 宿主走环境变量那一条老路（CLI / 无头形态的配置方式）。
    pub web_sources_file: Option<PathBuf>,
    /// 平台缝（见 `HostPlatform`）。
    pub platform: HostPlatform,
}

/// 起进程的输入（凭据已就位）。
pub struct HostLaunch<'a> {
    pub exe: &'a Path,
    pub spec: &'a HostSpec,
    /// 后端凭据（`CELIRA_IM_BACKEND_AUTH`）——只经环境。
    pub backend_credential: &'a str,
    /// 模型钥匙（`DEEPSEEK_API_KEY`，绑定不在场时给；绑定在场时这里给 `None`）。
    pub model_api_key: Option<&'a str>,
    pub log_file: &'a Path,
}

// ---------------------------------------------------------------------------
// 模型那一半的计划（**唯一口径**：安家那只手与"打开 app 起它"都走这里）
// ---------------------------------------------------------------------------
//
// 与 `up.js` 的早失败口径同款：环境根里有**绑定** ⇒ 走文件档（给宿主清单路径、
// **不给**模型 env）；没有绑定 ⇒ 要这台机器的 `DEEPSEEK_API_KEY`，没有就**早失败**
// （不是起起来再看着它认证不过）。绑定读不懂 ⇒ 如实拒绝（fail-closed）。

/// 模型那一半的两种走法。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelPlan {
    /// 文件档：这一支环境根里有绑定 ⇒ 给宿主**清单文件**的路径。
    File { sources_file: PathBuf },
    /// 环境档：没有绑定 ⇒ 给宿主这台机器上的那把钥匙（只在进程环境里过一道）。
    Env { api_key: String },
}

impl ModelPlan {
    pub fn sources_file(&self) -> Option<PathBuf> {
        match self {
            Self::File { sources_file } => Some(sources_file.clone()),
            Self::Env { .. } => None,
        }
    }

    pub fn env_key(&self) -> Option<&str> {
        match self {
            Self::File { .. } => None,
            Self::Env { api_key } => Some(api_key.as_str()),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::File { .. } => "file",
            Self::Env { .. } => "env",
        }
    }
}

/// 计划不出来的两种原因（都要如实说，不静默降级）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelPlanError {
    /// 环境根里的绑定读不懂（畸形 / 读不出来）——先修它。
    BindingUnreadable(String),
    /// **没有绑定**：app 形态不继承模型来源，所以这就是"还没配"。
    MissingBinding,
}

/// 这一支的模型计划（`up.js` / `anjia` / 起住户 三处同一条口径）。
///
/// ⚠ **2026-10-07 主人裁：app 形态不继承模型来源。**
///
/// 从前这一格是"没有绑定 ⇒ 读这台机器的 `DEEPSEEK_API_KEY` 递给它"。那条路的问题不在机制，
/// 在**它替用户做了决定**：环境里那把钥匙是**很久以前某个人为别的工具设的**，机器级、无期限、
/// 产品看不见 —— 它可能**已经过期**（主体第一次被叫醒就失败，而失败在产品里没有归属），也可能
/// **用户根本不想用**（比如按量计费很贵，产品却悄悄按那把计费）。判据：**"这台机器上恰好有这么
/// 一条"不该等于"这一支被配置过"。**
///
/// 所以 app 里模型来源**只从页面来**（选供应商 → 贴 key → 保存 → 绑定给这一支）；没绑定就是
/// **没配**，如实早失败，并指向那一页。**CLI / 无头 / CI 形态一字不变**（`scripts/home/up.js`
/// 的 `modelEnvironment`）：环境变量是**部署者**的配置方式，真机门禁也靠它。
pub fn model_plan(home_dir: &Path, env_root: &Path) -> Result<ModelPlan, ModelPlanError> {
    match crate::home_facts::read_model_binding(env_root) {
        crate::home_facts::ModelBinding::Bound { .. } => Ok(ModelPlan::File {
            sources_file: crate::home_facts::model_sources_list_path(home_dir),
        }),
        crate::home_facts::ModelBinding::Unreadable { reason } => {
            Err(ModelPlanError::BindingUnreadable(reason))
        }
        // **不继承**：这一支没有绑定 ⇒ 还没配（页面是 app 里唯一的来路）。
        crate::home_facts::ModelBinding::Absent => Err(ModelPlanError::MissingBinding),
    }
}

/// `buildHostArgs` 的直译。**顺序是语义**（与 bash 版逐字节对齐）。
pub fn build_host_args(spec: &HostSpec) -> Vec<String> {
    // **支级资源上界**（2026-10-05）：`agent.conf` 的 `max_turns`；缺省 = 内置 300。
    // 值**原样**交给宿主 —— 校验归它自己的旗标解析（非整数、0 都被拒，且有明确失败文本）。
    // 在这一层"顺手兜底成 8"会让**配置的值**与**运行的值**悄悄不一致（假绿）。
    let max_turns =
        crate::registration::read_agent_budget(&spec.env_root).unwrap_or_else(|| "300".to_string());
    let mut args: Vec<String> = vec![
        "--root".into(),
        spec.env_root.display().to_string(),
        "--agent".into(),
        spec.agent_id.clone(),
        "--materials".into(),
        spec.home_dir.join("materials").display().to_string(),
        "--max-turns".into(),
        max_turns,
    ];
    // ⚠ **终端档是支级事实，而且缺省不给**（2026-10-14 主人裁）：只有**显式声明**才给这只手。
    //   判据与 `scripts/home/up.js` 的 `residents.resolveTerminal` 同一条（那边是权威）：
    //   · `full`  → 有终端臂的平台（Windows / Linux）；
    //   · `sandbox` → 有沙箱 provider 的平台（今天只有 Linux；探针由 runtime 负责，不可用则
    //     宿主拒绝启动，不在这里把它换成 full 或 none）；
    //   · `none` / 没写 / 平台不支持 → **不带旗标**（宿主缺省就是 none）。
    //   ⚠ 与 `up.js` 不一致就是**两个行为**（哪个平台有没有臂、谁声明了，权威都在那边）。
    //   档位就是授权本体：给了 ⇒ 这台机器随主体用（agent 侧按档位门控装配）。
    let terminal = terminal_tier(spec);
    if terminal == Some(HandTier::Full) && spec.platform.has_terminal_arm() {
        args.push("--terminal".into());
        args.push("full".into());
    } else if terminal == Some(HandTier::Sandbox) && spec.platform.has_sandbox_arm() {
        args.push("--terminal".into());
        args.push("sandbox".into());
    }
    // **外部工具的授权面**（`home.conf` 的 `mcp_*` 行）：剥掉 `mcp_` 前缀，等号右边**原样**
    // 交给宿主（`conf.js` 的 `mcpDeclarations` 同一条）。⇒ 装了哪几台由**家**声明一次，
    // 每一支住户都看得见；**不在这里校验**——解析与拒绝只有一处权威（宿主入口）。
    for declaration in crate::home_facts::read_conf(&spec.home_dir).mcp {
        args.push("--mcp".into());
        args.push(declaration);
    }
    args.push("--im-endpoint".into());
    args.push(format!("http://{}", spec.listen));
    if let Some(list) = &spec.model_sources_file {
        args.push("--model-sources".into());
        args.push(list.display().to_string());
    }
    if let Some(list) = &spec.web_sources_file {
        args.push("--web-sources".into());
        args.push(list.display().to_string());
    }
    if let Some((id, name)) = &spec.identity {
        args.push("--identity-agent".into());
        args.push(id.clone());
        args.push("--identity-name".into());
        args.push(name.clone());
    }
    args
}

/// 起进程的结果（进程活着 + 日志在哪儿）。
#[derive(Debug, Clone)]
pub struct Spawned {
    pub pid: u32,
    pub log_file: PathBuf,
}

/// 日志 rotate：旧的一份留档为 `.1`（`up.js` 的 `proc.rotate` 同款意图：
/// 读的那一侧不会被上一次那一行骗出一道假绿，而旧读数回头还查得到）。
fn rotate_log(log_file: &Path) {
    if !log_file.exists() {
        return;
    }
    let backup = PathBuf::from(format!("{}.1", log_file.display()));
    let _ = fs::remove_file(&backup);
    let _ = fs::rename(log_file, &backup);
}

/// **代理绕过是"加两条"，不是"换一份"**（与 `up.js` 的 `appendNoProxy` 同一口径）。
///
/// 宿主必须能**不经代理**够到本机的世界与两个口 ⇒ `127.0.0.1` / `localhost` 一定要在
/// `NO_PROXY` 里。但**直接覆盖成一个常量**会把操作者自己加的绕过（例如给模型那家加一条）
/// **静默吞掉**：他改了环境变量也不生效，而症状是"主体的模型请求读体中断 / 超时"。
///
/// ⚠ **这一段是 CLI 先改、壳落后的那一份**（`up.js` 2026-10-09 真机实测后改成追加；本壳
/// 2026-10-10 才跟上）——两份实现各写一遍 argv/env 契约，正是登记在案的那条漂移风险。
///
/// 所以：**保留原有取值，把本机那两条补进去**（去重、去空）。
fn append_no_proxy(existing: Option<&str>) -> String {
    let mut parts: Vec<String> = existing
        .unwrap_or("")
        .split(',')
        .map(|each| each.trim().to_string())
        .filter(|each| !each.is_empty())
        .collect();
    for host in ["127.0.0.1", "localhost"] {
        if !parts.iter().any(|each| each == host) {
            parts.push(host.to_string());
        }
    }
    parts.join(",")
}

/// 起 `agent_host`。**stdout 与 stderr 收在同一个 fd**（同 `up.js`：`logs/host.out`
/// 把两边收在一起，不丢半句）。返回进程句柄（**不等待**——它是要常驻的）。
pub fn spawn_agent_host(launch: &HostLaunch) -> Result<(Spawned, Child), String> {
    if let Some(parent) = launch.log_file.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("建日志目录失败：{error}"))?;
    }
    rotate_log(launch.log_file);
    let stdout = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(launch.log_file)
        .map_err(|error| format!("日志文件开不了（{}）：{error}", launch.log_file.display()))?;
    let stderr = stdout
        .try_clone()
        .map_err(|error| format!("日志文件句柄复制失败：{error}"))?;

    // 与 `up.js` 同款：loopback 流量别被机器级代理拐走——但**保留**操作者自己那几条
    // （覆盖会把他给模型那家加的绕过静默吞掉，见 [`append_no_proxy`]）。
    let no_proxy = append_no_proxy(
        std::env::var("NO_PROXY")
            .or_else(|_| std::env::var("no_proxy"))
            .ok()
            .as_deref(),
    );
    let mut command = Command::new(launch.exe);
    command
        .args(build_host_args(launch.spec))
        .env("CELIRA_IM_BACKEND_AUTH", launch.backend_credential)
        .env("NO_PROXY", &no_proxy)
        .env("no_proxy", &no_proxy)
        // 子进程的 cwd = 家（`up.js` 的 `cwd: homeDir`）。
        .current_dir(&launch.spec.home_dir)
        // 同一份库配置导给子进程（`up.js` 的 `imEnv`：服务端与它的 CLI 子命令看同一个库）。
        .envs(world::im_env(&launch.spec.listen, &launch.spec.home_dir))
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    if let Some(key) = launch.model_api_key {
        command.env("DEEPSEEK_API_KEY", key);
    } else {
        // 文件档：手边本来就有的那两条也摘掉（宿主侧纪律：两个来源同时说话 = 拒绝启动）。
        command.env_remove("DEEPSEEK_API_KEY");
        command.env_remove("CELIRA_MODEL_API_KEY");
    }

    // **不给它控制台窗口**（2026-10-08）：宿主是控制台程序，而壳没有控制台 ⇒ 少了这一行
    // 就是"安家之后一直挂着一个黑框"。它的两条日志流照旧写文件（上面那两行）。
    crate::console_window::suppress(&mut command);

    let child = command
        .spawn()
        .map_err(|error| format!("起不来（{}）：{error}", launch.exe.display()))?;
    let pid = child.id();
    Ok((
        Spawned {
            pid,
            log_file: launch.log_file.to_path_buf(),
        },
        child,
    ))
}

/// 「报到」的三种读数（`up.js` 的判据：`[listening]` / `[exit]` 两行，容错 CRLF）。
#[derive(Debug, Clone)]
pub enum HostReport {
    /// 报到了（`[listening]` 一行在）。
    Listening,
    /// 它自己说了原因退的（`[exit]` 起头那一行到文件尾，原样带出来）。
    Exited { lines: Vec<String> },
    /// 预算内什么都没读到——**不是失败，也不是成功**（如实的第三态）。
    NotYet,
}

fn read_text(file: &Path) -> String {
    let mut text = String::new();
    if let Ok(mut handle) = fs::File::open(file) {
        let _ = handle.read_to_string(&mut text);
    }
    text
}

fn log_has_line(text: &str, tag: &str) -> bool {
    text.split('\n')
        .any(|line| line.trim_end_matches('\r').starts_with(tag))
}

/// 从第一行 `[state]`（或 `[exit]`）到文件尾——它自己说了原因，原样交出来。
fn from_state_line(text: &str) -> Vec<String> {
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect();
    let at = lines
        .iter()
        .position(|line| line.starts_with("[state]") || line.starts_with("[exit]"));
    match at {
        Some(index) => lines[index..].to_vec(),
        None => lines.iter().rev().take(5).rev().cloned().collect(),
    }
}

/// 等报到：每 500ms 看一次日志，共 40 次（`up.js` 同一预算）。
pub fn wait_for_report(log_file: &Path, attempts: u32, sleep_ms: u64) -> HostReport {
    for _ in 0..attempts {
        let text = read_text(log_file);
        if log_has_line(&text, "[exit]") {
            return HostReport::Exited {
                lines: from_state_line(&text),
            };
        }
        if log_has_line(&text, "[listening]") {
            return HostReport::Listening;
        }
        std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
    }
    HostReport::NotYet
}

/// 末尾几行（没等到时的读数尾巴）。
pub fn log_tail(log_file: &Path, lines: usize) -> Vec<String> {
    let text = read_text(log_file);
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..]
        .iter()
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(platform: HostPlatform, identity: bool, model_sources: bool) -> HostSpec {
        HostSpec {
            home_dir: PathBuf::from("/home/u/celira-home"),
            env_root: PathBuf::from("/home/u/celira-home/agents/a-1"),
            agent_id: "a-1".to_string(),
            listen: "127.0.0.1:8080".to_string(),
            identity: if identity {
                Some(("a-1".to_string(), "小满".to_string()))
            } else {
                None
            },
            model_sources_file: if model_sources {
                Some(PathBuf::from("/home/u/celira-home/model_sources.json"))
            } else {
                None
            },
            web_sources_file: None,
            platform,
        }
    }

    /// 平台无关的写法**（2026-10-03）：`--materials` 是 `home_dir.join("materials")` 出来的 ——
    /// 在 Windows 上它会用 `\` 连接。从前这里写死 `/home/u/celira-home/materials`，
    /// 于是这条用例只在 Linux 上绿。
    fn expected_materials() -> String {
        PathBuf::from("/home/u/celira-home")
            .join("materials")
            .display()
            .to_string()
    }

    /// **代理绕过是"加两条"，不是"换一份"**（2026-10-10）：操作者自己那几条必须**留着**——
    /// 覆盖会把他给模型那家加的绕过静默吞掉，症状是"模型请求读体中断 / 超时"（CLI 侧 2026-10-09
    /// 真机实测换来的口径，本壳同日落后于它；这一条钉住两份实现不许再漂）。
    #[test]
    fn no_proxy_keeps_the_operators_own_bypass_entries() {
        // 没有取值：补上本机那两条。
        assert_eq!(append_no_proxy(None), "127.0.0.1,localhost");
        // 空串 / 只有空白：同上（不许变成两条空条目）。
        assert_eq!(append_no_proxy(Some("")), "127.0.0.1,localhost");
        assert_eq!(append_no_proxy(Some(" , , ")), "127.0.0.1,localhost");
        // 操作者自己的条目**原样保留**（顺序也不动），已有的本机条目不重复补。
        assert_eq!(
            append_no_proxy(Some("localhost,127.0.0.1,::1,[::1]")),
            "localhost,127.0.0.1,::1,[::1]"
        );
        // 这就是今晚那一格：模型那家 + 本机两条。
        assert_eq!(
            append_no_proxy(Some("::1,opencode.ai")),
            "::1,opencode.ai,127.0.0.1,localhost"
        );
    }

    /// **支级资源上界从 `agent.conf` 来**（2026-10-05）：文件里写了 `max_turns=12` ⇒ argv 里
    /// 就是 `12`；**写了一个非法值就原样交出去**（不在这里兜底成 300 —— 校验的权威是宿主自己的
    /// 旗标解析，这里改写成另一个值就是"配置的值 ≠ 运行的值"的假绿）；文件不在 ⇒ 内置 `300`。
    #[test]
    fn host_args_take_max_turns_from_agent_conf_and_never_rewrite_it() {
        let dir = std::env::temp_dir().join(format!("celira-budget-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let at = |args: &[String]| {
            let index = args.iter().position(|arg| arg == "--max-turns").unwrap();
            args[index + 1].clone()
        };

        // 文件不在 ⇒ 内置 300。
        let mut s = spec(HostPlatform::Linux, false, false);
        s.env_root = dir.clone();
        assert_eq!(
            at(&build_host_args(&s)),
            "300",
            "没有 agent.conf ⇒ 内置 300"
        );

        // 文件里写了 12 ⇒ 12。
        fs::write(dir.join("agent.conf"), "agent_id=a-1\nmax_turns=12\n").unwrap();
        assert_eq!(
            at(&build_host_args(&s)),
            "12",
            "agent.conf 的 max_turns 必须进 argv"
        );

        // 非法值原样交出去（拒绝的权威在宿主）。
        fs::write(dir.join("agent.conf"), "max_turns=零\n").unwrap();
        assert_eq!(
            at(&build_host_args(&s)),
            "零",
            "非法值不许在这一层被改写成缺省（配置的值与运行的值必须一致）"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// 一个**真的临时家**：终端档那一格读的是**文件**（`agents/<支>/agent.conf` ⇒ `home.conf`），
    /// 所以这一格的用例必须落在真目录上，不能拿假路径糊过去。
    fn temp_spec(
        label: &str,
        platform: HostPlatform,
        home_lines: &str,
        agent_lines: &str,
    ) -> (PathBuf, HostSpec) {
        let dir =
            std::env::temp_dir().join(format!("celira-tier-{}-{}", label, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let env_root = dir.join("agents").join("a-1");
        fs::create_dir_all(&env_root).unwrap();
        if !home_lines.is_empty() {
            fs::write(dir.join("home.conf"), home_lines).unwrap();
        }
        if !agent_lines.is_empty() {
            fs::write(env_root.join("agent.conf"), agent_lines).unwrap();
        }
        let spec = HostSpec {
            home_dir: dir.clone(),
            env_root,
            agent_id: "a-1".to_string(),
            listen: "127.0.0.1:8080".to_string(),
            identity: None,
            model_sources_file: None,
            web_sources_file: None,
            platform,
        };
        (dir, spec)
    }

    /// 平台缝的**正例**：显式声明了 `terminal=full` 的支，Windows 与 Linux 给**同一份**参数
    /// （终端档都在；逐字节相同 —— 两支的差别在下游 agent 侧怎么用它，不在 argv 里）。
    /// 留档判据：`up.js` 的 `residentHands`（支级 ⇒ 家级 ⇒ 平台默认）＋ `resolveTerminal`。
    #[test]
    fn host_args_carry_the_terminal_tier_only_when_it_is_declared() {
        let (dir, spec) = temp_spec("declared", HostPlatform::Windows, "", "terminal=full\n");
        let materials = spec.home_dir.join("materials").display().to_string();
        let env_root = spec.env_root.display().to_string();
        let expected = vec![
            "--root",
            env_root.as_str(),
            "--agent",
            "a-1",
            "--materials",
            materials.as_str(),
            "--max-turns",
            "300",
            "--terminal",
            "full",
            "--im-endpoint",
            "http://127.0.0.1:8080",
        ];
        for platform in [HostPlatform::Windows, HostPlatform::Linux] {
            let declared = HostSpec {
                platform,
                ..spec.clone()
            };
            let args = build_host_args(&declared);
            assert_eq!(
                args.iter().map(String::as_str).collect::<Vec<_>>(),
                expected,
                "{platform:?} 这一支的参数不对"
            );
        }
        // 没有臂的平台：**声明了也不给**（有意的 fail-closed）。
        let other = HostSpec {
            platform: HostPlatform::Other,
            ..spec
        };
        assert!(!build_host_args(&other)
            .iter()
            .any(|arg| arg == "--terminal"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// **sandbox 档**：Linux 上原样进 argv；Windows / 别的平台上不带（也绝不换成 full）。
    #[test]
    fn host_args_carry_sandbox_only_on_linux() {
        let (dir, spec) = temp_spec("sandbox", HostPlatform::Linux, "", "terminal=sandbox\n");
        let linux = build_host_args(&spec);
        let at = linux
            .iter()
            .position(|arg| arg == "--terminal")
            .expect("Linux 上要带");
        assert_eq!(linux[at + 1], "sandbox");
        assert!(
            !linux.contains(&"full".to_string()),
            "sandbox 不许被换成 full"
        );

        let windows = HostSpec {
            platform: HostPlatform::Windows,
            ..spec
        };
        assert!(
            !build_host_args(&windows)
                .iter()
                .any(|arg| arg == "--terminal"),
            "Windows 上没有沙箱 provider ⇒ 不带，也不落成 full"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// **外部工具的授权面**：`home.conf` 里每一条 `mcp_<名>=…` 进一份 `--mcp <名>=…`，
    /// **按文件里的顺序**、等号右边原样交给宿主；裸的 `mcp_` 跳过。
    #[test]
    fn host_args_carry_every_declared_external_tool_in_file_order() {
        let (dir, spec) = temp_spec(
            "mcp",
            HostPlatform::Windows,
            "terminal=full\n\
             mcp_browser=stdio:npx @playwright/mcp --browser chrome\n\
             mcp_=oops\n\
             mcp_db=stdio:node db.js,read-only,timeout=30\n",
            "",
        );
        let args = build_host_args(&spec);
        let declared: Vec<&str> = args
            .iter()
            .enumerate()
            .filter(|(index, _)| *index > 0 && args[*index - 1] == "--mcp")
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(
            declared,
            [
                "browser=stdio:npx @playwright/mcp --browser chrome",
                "db=stdio:node db.js,read-only,timeout=30",
            ],
            "每一条都在、按文件顺序、原样交出；裸的 `mcp_` 跳过"
        );
        // 位置：`--terminal` 之后、`--im-endpoint` 之前（与 `up.js` 拼参数的次序一致）。
        let terminal_at = args.iter().position(|arg| arg == "--terminal").unwrap();
        let mcp_at = args.iter().position(|arg| arg == "--mcp").unwrap();
        let endpoint_at = args.iter().position(|arg| arg == "--im-endpoint").unwrap();
        assert!(terminal_at < mcp_at && mcp_at < endpoint_at);
        let _ = fs::remove_dir_all(&dir);
    }

    /// **没声明就不带**：家是干净的 ⇒ 一个 `--mcp` 都不出现（默认面是"一台都没有"）。
    #[test]
    fn host_args_without_any_declaration_carry_no_external_tool() {
        let (dir, spec) = temp_spec(
            "mcp-none",
            HostPlatform::Windows,
            "listen_addr=127.0.0.1:8080\n",
            "",
        );
        assert!(!build_host_args(&spec).iter().any(|arg| arg == "--mcp"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// **缺省是不给手**（2026-10-14 主人裁）：谁都没声明 ⇒ 有臂的平台也**不给** `--terminal`。
    /// 这一条就是本次裁决的钉子 —— 从前这里是"有臂就给 full"。
    #[test]
    fn host_args_without_any_declaration_never_carry_the_terminal_tier() {
        let (dir, spec) = temp_spec("undeclared", HostPlatform::Windows, "human_id=h-1\n", "");
        for platform in [HostPlatform::Windows, HostPlatform::Linux] {
            let bare = HostSpec {
                platform,
                ..spec.clone()
            };
            let args = build_host_args(&bare);
            assert!(
                !args.iter().any(|arg| arg == "--terminal"),
                "{platform:?} 上没人声明过 ⇒ 不给手（缺省＝自己区可写、出不去）"
            );
            assert!(args.contains(&"--im-endpoint".to_string()), "别的参数照旧");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    /// 三条规则连起来：**支级覆盖家级**；支级说 `none` ⇒ 家级给了也不给；
    /// 支级没写而家级给了 ⇒ 给；认不出的取值**当没写**（不猜、不就近映射）。
    #[test]
    fn the_tier_follows_the_resident_then_the_home_and_never_guesses() {
        let declared = |home: &str, agent: &str| {
            let (dir, spec) = temp_spec("rules", HostPlatform::Windows, home, agent);
            let tier = terminal_tier(&spec);
            let _ = fs::remove_dir_all(&dir);
            tier
        };
        assert_eq!(declared("", "terminal=full\n"), Some(HandTier::Full));
        assert_eq!(
            declared("", "terminal=sandbox\n"),
            Some(HandTier::Sandbox),
            "sandbox 要被认出来，不能当成没写"
        );
        assert_eq!(
            declared("terminal=none\n", "terminal=sandbox\n"),
            Some(HandTier::Sandbox),
            "支级 sandbox 覆盖家级 none"
        );
        assert_eq!(
            declared("", "terminal=NONE\n"),
            Some(HandTier::None),
            "大小写归一"
        );
        assert_eq!(
            declared("terminal=full\n", "terminal=none\n"),
            Some(HandTier::None),
            "支级覆盖家级：这一支明确不要，家级给了也不给"
        );
        assert_eq!(
            declared("terminal=full\n", ""),
            Some(HandTier::Full),
            "支级没写 ⇒ 家级是默认"
        );
        assert_eq!(declared("", ""), None, "都没写 ⇒ 不给");
        assert_eq!(
            declared("", "terminal=也许吧\n"),
            None,
            "认不出的取值当没写（与 JS 的 known() 同一条）"
        );
        assert_eq!(
            declared("terminal=FULL\n", ""),
            Some(HandTier::Full),
            "大小写归一"
        );
    }

    /// 平台缝的**反例**：其余平台（判据里没有终端臂）**一支都不给**；
    /// 其余参数照旧整齐 —— 这条「零个」就是回归判据。
    #[test]
    fn host_args_other_platforms_never_carry_the_terminal_tier() {
        let args = build_host_args(&spec(HostPlatform::Other, false, false));
        assert!(!args.iter().any(|arg| arg == "--terminal"));
        assert!(args.contains(&"--im-endpoint".to_string()));
        assert!(args.contains(&"http://127.0.0.1:8080".to_string()));
        let materials = expected_materials();
        assert_eq!(
            args.iter().map(String::as_str).collect::<Vec<_>>(),
            vec![
                "--root",
                "/home/u/celira-home/agents/a-1",
                "--agent",
                "a-1",
                "--materials",
                materials.as_str(),
                "--max-turns",
                "300",
                "--im-endpoint",
                "http://127.0.0.1:8080",
            ]
        );
    }

    /// 「是谁」两值**成对**给 + 清单：与平台无关（这里拿 Linux 这一支走一遍）。
    #[test]
    fn host_args_carry_identity_pair_and_model_list() {
        let (dir, spec) = temp_spec("identity", HostPlatform::Linux, "", "terminal=full\n");
        let spec = HostSpec {
            identity: Some(("a-1".to_string(), "小满".to_string())),
            model_sources_file: Some(spec.home_dir.join("model_sources.json")),
            web_sources_file: Some(spec.home_dir.join("web_sources.json")),
            ..spec
        };
        let args = build_host_args(&spec);
        let terminal_at = args.iter().position(|arg| arg == "--terminal").unwrap();
        assert_eq!(args[terminal_at + 1], "full");
        let identity_at = args
            .iter()
            .position(|arg| arg == "--identity-agent")
            .unwrap();
        assert_eq!(args[identity_at + 1], "a-1");
        assert_eq!(args[identity_at + 2], "--identity-name");
        assert_eq!(args[identity_at + 3], "小满");
        assert!(args.contains(&"--model-sources".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn report_marker_reads_three_states() {
        let dir = std::env::temp_dir().join(format!("celira-anjia-host-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("host.out");

        fs::write(
            &log,
            "celira-agent-host v1\n[listening]\ndoorbell=im-events\n",
        )
        .unwrap();
        assert!(matches!(wait_for_report(&log, 3, 1), HostReport::Listening));

        fs::write(
            &log,
            "banner\n[state]\nadmission=ok\n[exit]\nreason=environment-unusable\n",
        )
        .unwrap();
        match wait_for_report(&log, 3, 1) {
            HostReport::Exited { lines } => {
                assert!(lines
                    .iter()
                    .any(|line| line.contains("reason=environment-unusable")));
            }
            other => panic!("应当读到退场原因：{other:?}"),
        }

        fs::write(&log, "banner only\n").unwrap();
        assert!(matches!(wait_for_report(&log, 2, 1), HostReport::NotYet));

        let _ = fs::remove_dir_all(&dir);
    }
}
