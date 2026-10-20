//! **本机世界**（完整 app 自带的那一半）——「这台机器上的世界在不在、地址是什么、
//! 我（主人）在不在里面、怎么拿到一支会话」。
//!
//! ## 为什么有这一层（第一性原理）
//!
//! 桌面 app 的默认形态是**成品**（`PROJECT_SHAPE.md` §9.1：Windows＝"完整 app：壳 + 本机世界
//! \+ 本机执行端"），不是"一个等着别人来 serve 页面的空壳"。而**页面在壳里没有"页面来源"
//! 可解析**（`client/src/api/shell.ts` 的 `resolveShellUrl`：相对地址在壳里响亮失败）——
//! 于是**本机地址与本机世界的主人是机器事实，必须由壳在运行时解析并交给页面**
//! （§8.4 推论 2："本机地址由 app 解析"；§9.3 一条路："app 里家条目统一为地址 + 钥匙，
//! 本机那条只是地址为 loopback 的一条"）。让用户手敲 `http://127.0.0.1:8080` 是把
//! **我们**的实现细节摊派给他 —— 那不是"让用户自己处理"。
//!
//! ## 这一层做的四件事，与它刻意不做的
//!
//! 1. **报读数**（[`readout`]）：家在哪、世界在不在、主人是谁 —— **只读，不动作**；
//! 2. **起世界**（[`ensure_world`]）：探活优先（**已经在跑就不动它**，`up.js` 同款），
//!    不在才起：无参数、`cwd` = 家、三个 `CELIRA_IM_*` 环境（`up.js` 的 `imEnv`）、
//!    日志 `logs/im.out`、就绪判据 `GET /health` 回 200（30 秒预算，与 `up.js` 同一份）；
//! 3. **首次布置**（[`ensure_human`]）：家里还没有主人 ⇒ `bootstrap-human`（显示名
//!    取**这台机器的用户名**，取不到用占位 `未命名` —— 与 `provision.js` 的
//!    `defaultHumanDisplayName` 逐字同款；**不是**关系词，见那条函数的判据）并把 `human_id` 记进
//!    `home.conf`（**唯一允许的写**，见 [`conf_set_text`] 与 `home_facts` 的护栏）；
//!    已经有主人 ⇒ 直接补一支会话（`issue-human-session`，与 `provision.js` 同款）；
//! 4. **交一支会话**（[`local_session`]）：token **只在内存里交给页面一次**，壳不落盘、不打印。
//!
//! **不做**：不建 Agent（§8.5："不存在默认 Agent"——身份来自一次显式创建）、不做托盘与
//! 退出语义（§8.2-8.3）、不做 PID 认领（§8.7）、崩了不自动重启（§8.6-2）、不代理、
//! 不做远程条目（§8.4 那张表的后半）。**起本机的执行端住户**是紧随其后的一刀。
//!
//! ## 与 `home.js` 的关系（判据对齐，不共用代码）
//!
//! 同一个家**同一时刻只有一只布置的手**：有壳时是这里的 [`ensure_world`] / [`ensure_human`]，
//! 无壳时是 `node home.js up`（Linux/服务器形态，§9.1 明确"不要壳"）。
//! 两边**写同一份文件、同一套键、同一条就绪判据** —— 这是 `anjia` 与 `adopt.js` 那条先例
//! 的同款处置（照抄判据、不共用代码）。
//!
//! ## 一条诚实：账本照写
//!
//! 壳起的这个世界**也写 `logs/im.pid`**（与 `up.js` 同一份文件、同一个格式：一个 pid 数字）。
//! 为什么：`node home.js status/down` 按这本账认进程 —— 壳起了却不记账，CLI 会如实报
//! "没在跑"，而它明明在跑（**假读数**）。这不是"PID 认领"（§8.7 撤掉的是**认领别人的**
//! 进程），这是"谁起的谁记账"。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use serde::Serialize;
use tauri::State;

use crate::home_facts;
use crate::world_channel;
use crate::world_channel::WorldHttp;

/// 首位 Human 的显示名**由人给**（2026-10-20 主人裁决："必须要填"）。
///
/// ## 为什么不再取这台机器的用户名
///
/// 用户名是**机器事实**，而显示名是**公共事实**（那个世界的 Participant）—— 把机器
/// 事实写进公共世界就是拿它冒充公共事实，而且它常常不是你（`Administrator` / `user` /
/// `DESKTOP-8K2LQ`），同机两个人也分不开。判据与 2026-10-12 给 Agent 定的那条是**同一条**：
/// 显示名是公共呈现，**系统不替谁认定身份**（判据 9）。
///
/// ## 三条规则（与 Core 同一套，别在这里发明第二种）
///
/// 权威在 `im/server/src/domain/participant.rs` 的 `DisplayName::parse`（契约 4.1）：
/// 禁 U+0000、至多 **80 个 Unicode scalar value**、trim 后至少一个非空白字符。
/// 壳这一道**不是**重复劳动：页面与 CLI 都能绕过对方，而壳是唯一同时看得见两者的那一层。
/// 返回的是**trim 之后**要提交给世界的那份正文（与兑换邀请那条路一致，`invite.ts` 同款）。
pub fn normalize_human_name(raw: &str) -> Result<String, HumanNameError> {
    if raw.contains('\u{0}') {
        return Err(HumanNameError::ContainsNul);
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(HumanNameError::Empty);
    }
    // `chars().count()` 就是 Unicode scalar value 计数（与 Core 那一侧逐字同款）。
    if trimmed.chars().count() > 80 {
        return Err(HumanNameError::TooLong);
    }
    Ok(trimmed.to_string())
}

/// 名字不合法的三种（封闭表；文案是给页面的一句话，不是给机器判读的词）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HumanNameError {
    /// trim 之后一个字都不剩。
    Empty,
    /// 含 U+0000。
    ContainsNul,
    /// 超过 80 个 Unicode scalar value。
    TooLong,
}

impl HumanNameError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Empty => "名字不能是空的 —— 这个世界里别人看到的就是它",
            Self::ContainsNul => "名字里不能有控制字符（U+0000）",
            Self::TooLong => "名字最多 80 个字（按字符数，不按字节）",
        }
    }
}

/// 就绪预算：30 秒（`up.js` 的 `waitHttp(..., 30)`）。
const READY_BUDGET_MS: u64 = 30_000;
/// 探活间隔（`up.js` 的 `waitHttp` 步长 500ms）。
const READY_STEP_MS: u64 = 500;

// ---------------------------------------------------------------------------
// 家事实：装配一次，后面都用它
// ---------------------------------------------------------------------------

/// 本机家的一次快照（家在哪、配置、布局、可执行件）。
#[derive(Debug, Clone)]
pub struct LocalFacts {
    pub home_dir: PathBuf,
    pub conf: home_facts::HomeConf,
    pub layout: home_facts::Layout,
    /// 仓库根/解压根（找不到 ⇒ `None`：读数是"缺可执行件"，**不猜路径**）。
    pub repo_dir: Option<PathBuf>,
}

impl LocalFacts {
    /// 世界地址（`home.conf` 的 `listen_addr`，缺省 `127.0.0.1:8080`）。
    pub fn listen(&self) -> String {
        home_facts::listen_addr(&self.conf)
    }

    /// 世界的 base URL（`http://<listen>`）。
    pub fn world_base(&self) -> String {
        format!("http://{}", self.listen())
    }

    /// 装载（`home_dir` / `repo_dir` 都是**显式覆盖 → 环境变量 → 默认**，与安家同一套解析）。
    pub fn load(explicit_home: Option<&Path>, explicit_repo: Option<&Path>) -> Self {
        let home_dir = home_facts::resolve_home_dir(explicit_home);
        let conf = home_facts::read_conf(&home_dir);
        let repo_dir = home_facts::resolve_repo_dir(explicit_repo);
        let layout = match &repo_dir {
            Some(repo) => home_facts::detect_layout(repo),
            // 找不到根 ⇒ 给一个"源码形态 + 默认家"的空布局：读数是「缺可执行件」，
            // 路径由 `layout.im_exe` 如实报出来（`paths.js` 的 `exeIn` 同款意图）。
            None => home_facts::detect_layout(&home_dir),
        };
        Self {
            home_dir,
            conf,
            layout,
            repo_dir,
        }
    }

    /// 世界的 pid 账本（`logs/im.pid`，与 `up.js` 同一处）。
    pub fn im_pid_file(&self) -> PathBuf {
        self.home_dir.join("logs").join("im.pid")
    }

    /// 世界的日志（`logs/im.out`）。
    pub fn im_log_file(&self) -> PathBuf {
        self.home_dir.join("logs").join("im.out")
    }
}

// ---------------------------------------------------------------------------
// 读数（壳↔前端的形状；稳定词表，人话归页面）
// ---------------------------------------------------------------------------

/// 本机世界的**状态**（封闭词表；页面只认它，人话归页面 —— 与 `anjia` 的 `kind()` 同一条纪律）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldState {
    /// 答得上：地址上有世界（`GET /health` 回 200）。
    Running,
    /// 没在跑（可起）。
    NotRunning,
    /// 缺可执行件（这台机器上还没有 world 二进制）。
    NoCore,
}

impl WorldState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::NotRunning => "not-running",
            Self::NoCore => "no-core",
        }
    }
}

/// 一支本机住户在读数里的样子（**只报机器事实**：它是谁、账本说它在不在跑）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResidentReadout {
    pub agent_id: String,
    pub label: String,
    /// `running` / `not-running` / `not-ours` / `unverifiable`（封闭词表；人话归页面）。
    pub state: String,
    /// 账本里那个 pid（没有 ⇒ `null`）——读数用，页面不据此做任何事。
    pub pid: Option<u32>,
}

/// 一次读数的形状（给页面；**不含任何凭据**）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalWorldReadout {
    /// 世界状态（`running` / `not-running` / `no-core`）。
    pub state: String,
    /// 世界的地址（`http://127.0.0.1:<port>`）——页面拿它当**世界通道**的 base。
    pub address: String,
    /// 家的位置（读数用；页面不据此拼路径）。
    pub home_dir: String,
    /// 这位主人是谁（`home.conf` 的 `human_id`；还没铸人 ⇒ `null`）。
    pub human_id: Option<String>,
    /// 家的门两个端面（loopback 基址，页面拿它当**本机家条目**的地址）。
    pub status_base: String,
    pub config_base: String,
    /// 这台机器要不要跑一个世界：`null` = **还没表态**（§8.6-1 该问一次）。
    pub run_local_world: Option<bool>,
    /// 世界的可执行件（缺件时这条读数说明"该有什么"）。
    pub im_exe: String,
    /// 这个家住着哪几支、各自在不在跑（**账本 + 身份核对**，不扫进程表）。
    pub residents: Vec<ResidentReadout>,
    /// 一条如实读数（原样；只在"起失败了"这类场合带原因，其余时候是 `null`）。
    pub detail: Option<String>,
}

/// 端口的默认值（`paths.js` 的 `DEFAULT_STATUS_PORT` / `DEFAULT_CONFIG_PORT`）。
const DEFAULT_STATUS_PORT: &str = "5190";
const DEFAULT_CONFIG_PORT: &str = "5191";

/// 从 `home.conf` 取一个端口；没有 / 读不懂 ⇒ 默认值（与 `paths.js` 同一条纪律：
/// 新字段一律带默认值，用户不填就按默认）。
fn port_or(value: Option<&String>, fallback: &str) -> String {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()))
        .unwrap_or_else(|| fallback.to_string())
}

/// 组一份读数（**纯函数**：状态、表态、家事实进，形状出 ⇒ 可机械判错）。
pub fn readout(
    facts: &LocalFacts,
    state: WorldState,
    run_local_world: Option<bool>,
) -> LocalWorldReadout {
    let listen = facts.listen();
    LocalWorldReadout {
        state: state.as_str().to_string(),
        address: format!("http://{listen}"),
        home_dir: facts.home_dir.display().to_string(),
        human_id: facts.conf.human_id.clone(),
        status_base: format!(
            "http://127.0.0.1:{}",
            port_or(facts.conf.status_port.as_ref(), DEFAULT_STATUS_PORT)
        ),
        config_base: format!(
            "http://127.0.0.1:{}",
            port_or(facts.conf.config_port.as_ref(), DEFAULT_CONFIG_PORT)
        ),
        run_local_world,
        im_exe: facts.layout.im_exe.display().to_string(),
        residents: resident_readouts(facts),
        detail: None,
    }
}

/// 这个家住着哪几支、各自在不在跑（**只读**：账本 + 身份核对，不动作）。
pub fn resident_readouts(facts: &LocalFacts) -> Vec<ResidentReadout> {
    crate::local_residents::residents(&facts.home_dir)
        .into_iter()
        .map(|resident| {
            let state = crate::local_residents::state_of(&resident, &facts.layout.host_exe);
            ResidentReadout {
                label: resident.label(),
                agent_id: resident.agent_id,
                pid: crate::local_residents::ledger_pid(&resident.pid_file),
                state: state.kind().to_string(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 探活 / 起世界
// ---------------------------------------------------------------------------

/// 答得上吗（`GET /health` 回 200）。
fn probe(base: &str) -> bool {
    match WorldHttp::new(base) {
        Ok(http) => http.health_ok(),
        Err(_) => false,
    }
}

/// 等它答得上（预算 [`READY_BUDGET_MS`]；`up.js` 同一份步长）。
fn wait_ready(base: &str) -> bool {
    let mut waited = 0;
    loop {
        if probe(base) {
            return true;
        }
        if waited >= READY_BUDGET_MS {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(READY_STEP_MS));
        waited += READY_STEP_MS;
    }
}

/// 起世界的结果（如实三态：已经在跑 / 这一次起来了 / 起不来）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureOutcome {
    /// 它已经在跑（**不是我们起的**也照样用 —— 同一个家只该有一个世界）。
    AlreadyRunning,
    /// 这一次起来了（`pid` = 我们记下的那个）。
    Started { pid: u32 },
    /// 缺可执行件。
    NoCore,
    /// 起不来（30 秒没回应 / spawn 失败）：读数原样带出来。
    Failed { detail: String },
}

/// **确保这个世界在跑**（探活优先 ⇒ 不在才起 ⇒ 起了等就绪）。
///
/// 三条判据照 `up.js`：
/// 1. **已经在跑就不动它**（同一个家只该有一个服务端；起第二个会撞端口）；
/// 2. 起来之后**必须等它自己答**（`GET /health` 回 200），不信"进程起来了"；
/// 3. 起来之后**记 pid 账本**（见本文件头那条"账本照写"）。
pub fn ensure_world(facts: &LocalFacts) -> EnsureOutcome {
    let base = facts.world_base();
    if probe(&base) {
        return EnsureOutcome::AlreadyRunning;
    }
    if !facts.layout.im_exe.is_file() {
        return EnsureOutcome::NoCore;
    }
    if let Some(parent) = facts.im_log_file().parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            return EnsureOutcome::Failed {
                detail: format!("建日志目录失败（{}）：{error}", parent.display()),
            };
        }
    }
    let mut command = Command::new(&facts.layout.im_exe);
    // **无参数**（`up.js` 起服务端就是这一句），家事实全经环境与 cwd 进。
    command
        .current_dir(&facts.home_dir)
        .envs(world_channel::im_env(&facts.listen(), &facts.home_dir))
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .stdin(Stdio::null());
    // **不给它控制台窗口**（2026-10-08 真机读数：修之前 `conhost` 的父进程正是这一支 ——
    // 打开 app 就多一个黑框）。日志照旧写文件，一个字不少。
    crate::console_window::suppress(&mut command);
    // 日志：与 `up.js` 同款——stdout 与 stderr 收在同一个 fd（不丢半句）。
    match append_log(&facts.im_log_file()) {
        Ok(log) => {
            let stderr = match log.try_clone() {
                Ok(handle) => handle,
                Err(error) => {
                    return EnsureOutcome::Failed {
                        detail: format!("日志文件句柄复制失败：{error}"),
                    }
                }
            };
            command.stdout(Stdio::from(log)).stderr(Stdio::from(stderr));
        }
        Err(detail) => return EnsureOutcome::Failed { detail },
    }

    let child: Child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return EnsureOutcome::Failed {
                detail: format!("起不来（{}）：{error}", facts.layout.im_exe.display()),
            }
        }
    };
    let pid = child.id();
    // 账本照写（见文件头"一条诚实"）：CLI 那边（`home.js status/down`）按它认进程。
    write_pid(&facts.im_pid_file(), pid);

    if wait_ready(&base) {
        EnsureOutcome::Started { pid }
    } else {
        EnsureOutcome::Failed {
            detail: format!(
                "起不来（{} 秒没回应）—— 日志尾巴：\n{}",
                READY_BUDGET_MS / 1000,
                world_channel::tail_text(&read_text(&facts.im_log_file()), 5)
            ),
        }
    }
}

/// 追加式打开日志（**不 rotate**：世界那一份是常驻件的日志，通路与 `up.js` 的
/// `spawnDetached(logFile)` 一致 —— 它也是追加，不把上一次的读数丢掉）。
fn append_log(path: &Path) -> Result<fs::File, String> {
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("日志文件开不了（{}）：{error}", path.display()))
}

fn read_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn write_pid(path: &Path, pid: u32) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, format!("{pid}\n"));
}

// ---------------------------------------------------------------------------
// 首次布置 / 会话
// ---------------------------------------------------------------------------

/// 取一支本机会话的结果（如实三态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionOutcome {
    /// 拿到会话（`human_id` 是它代表的主人；`minted_human` = 这一次刚铸的人）。
    Ok {
        human_id: String,
        token: String,
        minted_human: bool,
    },
    /// **这个世界还没有你**（`home.conf` 没有 `human_id`）⇒ 名字得由人给，
    /// 铸人只走 [`bootstrap_first_human`]。这一支**不**等于失败：它是一个待用户回答的问题。
    NoHuman,
    /// 世界没在跑（可起 —— 调用方先 [`ensure_world`]）。
    WorldDown,
    /// 缺可执行件。
    NoCore,
    /// 失败（原样原因）。
    Failed { detail: String },
}

/// `home.conf` 的 `human_id`（有 ⇒ 直接用；没有 ⇒ 还没铸人）。
pub fn human_id_of(facts: &LocalFacts) -> Option<String> {
    facts
        .conf
        .human_id
        .clone()
        .filter(|value| !value.trim().is_empty())
}

/// **取一支本机会话**（只读：世界还没铸人时它**不替谁起名**）。
///
/// 顺序：
/// 1. 缺可执行件 ⇒ [`SessionOutcome::NoCore`]；
/// 2. 世界答不上 ⇒ [`SessionOutcome::WorldDown`]（**不猜、不动**）；
/// 3. `home.conf` 没有 `human_id` ⇒ [`SessionOutcome::NoHuman`]（**名字得由人给**，
///    铸人只走 [`bootstrap_first_human`] —— 2026-10-20 之前这里会顺手铸一个，
///    而那时的名字是从机器用户名来的）；
/// 4. 有 ⇒ `issue-human-session`（补一支，旧的那支仍然有效）。
pub fn local_session(facts: &LocalFacts) -> SessionOutcome {
    if !facts.layout.im_exe.is_file() {
        return SessionOutcome::NoCore;
    }
    if !probe(&facts.world_base()) {
        return SessionOutcome::WorldDown;
    }
    match human_id_of(facts) {
        Some(human_id) => issue_session_for(facts, human_id),
        None => SessionOutcome::NoHuman,
    }
}

/// 补一支会话（已有主人时那两条路共用）。
fn issue_session_for(facts: &LocalFacts, human_id: String) -> SessionOutcome {
    match world_channel::issue_human_session(
        &facts.layout.im_exe,
        &facts.listen(),
        &facts.home_dir,
        &human_id,
    ) {
        Ok(token) => SessionOutcome::Ok {
            human_id,
            token,
            minted_human: false,
        },
        Err(detail) => SessionOutcome::Failed { detail },
    }
}

/// **铸首位 Human**（`bootstrap-human`）—— 名字由人给，**这是唯一的铸人入口**。
///
/// 顺序与 [`local_session`] 同一条：缺件 ⇒ `NoCore`；世界没跑 ⇒ `WorldDown`；
/// 已经有主人 ⇒ 补一支会话（**不铸第二个**，幂等）；名字不合法 ⇒ `Failed`（原因给人看）。
pub fn bootstrap_first_human(facts: &LocalFacts, raw_name: &str) -> SessionOutcome {
    if !facts.layout.im_exe.is_file() {
        return SessionOutcome::NoCore;
    }
    if !probe(&facts.world_base()) {
        return SessionOutcome::WorldDown;
    }
    if let Some(human_id) = human_id_of(facts) {
        return issue_session_for(facts, human_id);
    }
    let name = match normalize_human_name(raw_name) {
        Ok(name) => name,
        Err(error) => {
            return SessionOutcome::Failed {
                detail: error.message().to_string(),
            }
        }
    };
    match world_channel::bootstrap_human(
        &facts.layout.im_exe,
        &facts.listen(),
        &facts.home_dir,
        &name,
    ) {
        Ok((human_id, token)) => {
            // 记下来（否则 `home.js` 下次跑会以为还没铸人、再 bootstrap 一次）。
            if let Err(detail) = remember_human_id(facts, &human_id) {
                // 会话已经拿到，但这份事实没落盘 ⇒ **如实报失败**（下次会重复铸人那一步，
                // 而服务端的 guard 会挡住 —— 那是要用户看见的事，不是吞掉的事）。
                return SessionOutcome::Failed { detail };
            }
            SessionOutcome::Ok {
                human_id,
                token,
                minted_human: true,
            }
        }
        Err(detail) => SessionOutcome::Failed { detail },
    }
}

/// 把 `human_id` 记进 `home.conf`（**唯一允许的写**；空值与"已经有同一个"都不写盘）。
fn remember_human_id(facts: &LocalFacts, human_id: &str) -> Result<(), String> {
    if human_id.trim().is_empty() {
        return Err("铸人回执里的 human id 是空的 —— 不写 home.conf".to_string());
    }
    if human_id_of(facts).as_deref() == Some(human_id) {
        return Ok(());
    }
    let path = facts.home_dir.join("home.conf");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let next = conf_set_text(&existing, "human_id", human_id);
    fs::write(&path, next)
        .map_err(|error| format!("写 home.conf 失败（{}）：{error}", path.display()))
}

/// `conf.js` 的 `confSet` 直译（**纯函数**，可机械判错）：同键老行删掉、新行追加在末尾、
/// 统一 LF、**其余行原样保留**（含全部 `mcp_` 行）；值里可以含 `=`，只按第一个等号切。
pub fn conf_set_text(existing: &str, key: &str, value: &str) -> String {
    let prefix = format!("{key}=");
    let mut lines: Vec<String> = existing
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        // 结尾换行会多出一个空串：去掉它，避免写回时越写越多空行（`readLines` 同款）。
        .collect();
    if lines.last().map(|line| line.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    let kept: Vec<String> = lines
        .into_iter()
        .filter(|line| !line.starts_with(&prefix))
        .collect();
    let mut out = kept;
    out.push(format!("{key}={value}"));
    format!("{}\n", out.join("\n"))
}

/// `conf.js` 的 `confDelete` 直译（**纯函数**）：删掉所有以 `<键>=` 开头的行，其余原样，
/// 统一 LF、**结尾一个换行**。返回 `(新正文, 有没有删掉东西)` —— 第二个值让调用方知道
/// "这次真的改了没有"（照 JS 的 `return false`：没改就不写盘）。
pub fn conf_delete_text(existing: &str, key: &str) -> (String, bool) {
    let prefix = format!("{key}=");
    let lines: Vec<String> = existing
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();
    let before = if lines.last().map(|line| line.is_empty()).unwrap_or(false) {
        lines.len() - 1
    } else {
        lines.len()
    };
    let kept: Vec<String> = lines[..before]
        .iter()
        .filter(|line| !line.starts_with(&prefix))
        .cloned()
        .collect();
    let changed = kept.len() != before;
    // ⚠ 与 JS 逐字对齐：**总是**以 `\n` 结尾（全删光时那份文件里是一个空行，不是一个空文件）——
    // 两边读回来都等于"没有这个键"，但字节形状照抄权威那一份，省得将来对不上。
    let text = format!("{}\n", kept.join("\n"));
    (text, changed)
}

// ---------------------------------------------------------------------------
// 命令（壳↔前端的最窄面）
// ---------------------------------------------------------------------------

/// 只读读数：**不动作**（不起世界、不铸人、不签会话）。
#[tauri::command]
pub async fn local_world_readout(
    config: State<'_, crate::MachineConfigDir>,
) -> Result<LocalWorldReadout, String> {
    let config_dir = config.0.clone();
    let view =
        tauri::async_runtime::spawn_blocking(move || -> Result<LocalWorldReadout, String> {
            let facts = LocalFacts::load(None, None);
            let state = world_state(&facts);
            let decided = crate::machine_settings::read_run_local_world(&config_dir);
            Ok(readout(&facts, state, decided))
        })
        .await
        .map_err(|error| format!("壳内部错误：本机世界读数没能回来（{error}）"))??;
    Ok(view)
}

/// 表态「在这台机器上跑一个世界」：写下表态，然后**确保它在跑**（探活优先），
/// **并确保这个家的住户都在跑**（§8.1：本地进程是本地 app 的孩子 —— "打开就进、它就在"）。
///
/// 返回值是**起完之后**的读数（世界在不在、主人是谁、住户各在不在跑），**不含 token** ——
/// token 只从 [`local_world_session`] 出来，一次一支。
#[tauri::command]
pub async fn local_world_enable(
    config: State<'_, crate::MachineConfigDir>,
) -> Result<LocalWorldReadout, String> {
    let config_dir = config.0.clone();
    let view =
        tauri::async_runtime::spawn_blocking(move || -> Result<LocalWorldReadout, String> {
            crate::machine_settings::write_run_local_world(&config_dir, true)?;
            let facts = LocalFacts::load(None, None);
            // 起世界那一步的失败**不吞**：读数里如实带上（世界没起来 ⇒ 调用方不该往下走）。
            let outcome = ensure_world(&facts);
            let state = match outcome {
                EnsureOutcome::AlreadyRunning | EnsureOutcome::Started { .. } => {
                    WorldState::Running
                }
                EnsureOutcome::NoCore => WorldState::NoCore,
                EnsureOutcome::Failed { detail } => {
                    let mut view = readout(&facts, WorldState::NotRunning, Some(true));
                    // 失败原因原样交给页面（它是读数的一部分，不是异常）。
                    view.detail = Some(detail);
                    return Ok(view);
                }
            };
            // 世界在跑 ⇒ 把本机住户也带起来（它本来在跑 ⇒ 一个字都不动）。
            // ⚠ 起住户**不等报到**：不为了报一句读数把正门堵二十秒（状态由下一次读数给出）。
            // ⚠ 主人会话是**可选的**：拿不到就跳过"世界说它在不在线"那一问（账本仍然照判），
            //    但**绝不**因为拿不到会话就不起它 —— 那会让"打开 app 它就在"这条承诺失效。
            let token = match local_session(&facts) {
                SessionOutcome::Ok { token, .. } => Some(token),
                _ => None,
            };
            let reports = crate::local_residents::ensure_residents(&facts, token.as_deref());
            // **家的两道门**（页面那三格读数与机器页三间的来路 ＋ 模型配置页的读；探活优先）。
            // 起不来不拦正门（页面会把读不到如实说成"状态未知"），但**要记在读数里**。
            let mut door_failures: Vec<String> = Vec::new();
            for (what, outcome) in [
                ("状态口", crate::home_doors::ensure_status_door(&facts)),
                ("配置门", crate::home_doors::ensure_config_door(&facts)),
            ] {
                if let crate::home_doors::DoorOutcome::Failed { detail } = outcome {
                    door_failures.push(format!("家的{what}没起来：{detail}"));
                }
            }
            let mut view = readout(&facts, state, Some(true));
            view.detail = summarize_residents(&reports);
            for failure in door_failures {
                view.detail = Some(match view.detail {
                    Some(existing) => format!("{existing}；{failure}"),
                    None => failure,
                });
            }
            Ok(view)
        })
        .await
        .map_err(|error| format!("壳内部错误：起本机世界没能回来（{error}）"))??;
    Ok(view)
}

/// 把"这一轮起了谁"收成一句如实读数（一个都没起 ⇒ `None`，不编一句"一切正常"）。
fn summarize_residents(reports: &[crate::local_residents::ResidentReport]) -> Option<String> {
    use crate::local_residents::StartOutcome;
    let mut lines: Vec<String> = Vec::new();
    for report in reports {
        let Some(outcome) = &report.outcome else {
            continue;
        };
        let line = match outcome {
            StartOutcome::Started { pid } => format!("{}：起来了（pid {pid}）", report.label),
            StartOutcome::AlreadyRunning { pid } => {
                format!("{}：本来就在跑（pid {pid}）", report.label)
            }
            StartOutcome::RefusedNoCore { exe } => {
                format!("{}：缺宿主可执行件（{}）", report.label, exe.display())
            }
            StartOutcome::RefusedMissingModel => format!(
                "{}：模型那一半没人在（没有文件档绑定，也没有 DEEPSEEK_API_KEY）—— 没起这支 Agent",
                report.label
            ),
            StartOutcome::RefusedMissingCredential { detail } => {
                format!("{}：{detail}", report.label)
            }
            StartOutcome::Failed { detail } => {
                format!("{}：起这支 Agent 没成（{detail}）", report.label)
            }
        };
        lines.push(line);
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("；"))
    }
}

/// 表态「这台机器上不跑世界」（第一屏回登录页，去连别的世界）。
///
/// ⚠ **只改表态，不停已经在跑的世界** —— "停"是退出语义那一格（§8.2-8.3），不在这里偷做。
#[tauri::command]
pub async fn local_world_disable(
    config: State<'_, crate::MachineConfigDir>,
) -> Result<LocalWorldReadout, String> {
    let config_dir = config.0.clone();
    let view =
        tauri::async_runtime::spawn_blocking(move || -> Result<LocalWorldReadout, String> {
            crate::machine_settings::write_run_local_world(&config_dir, false)?;
            let facts = LocalFacts::load(None, None);
            let state = world_state(&facts);
            Ok(readout(&facts, state, Some(false)))
        })
        .await
        .map_err(|error| format!("壳内部错误：写本机世界表态没能回来（{error}）"))??;
    Ok(view)
}

/// 交一支**本机会话**（页面用它进自己的世界）。
///
/// 三态如实：拿到 / 世界没在跑 / **这个世界还没有你**（`no-human`）/ 失败（原因原样）。
/// **token 只在这一次调用里出现**，壳不落盘、不打日志；页面负责把它放到自己的持久面
/// （`prefs`，唯一权威）。
///
/// ⚠ 它**不铸人**（2026-10-20）：没有主人时回 `no-human`，由页面问出名字再叫
/// [`local_world_bootstrap`]。从前这里会顺手用一个机器用户名铸一个。
#[tauri::command]
pub async fn local_world_session() -> Result<LocalSessionView, String> {
    let view = tauri::async_runtime::spawn_blocking(move || -> LocalSessionView {
        let facts = LocalFacts::load(None, None);
        session_view(&facts, local_session(&facts))
    })
    .await
    .map_err(|error| format!("壳内部错误：本机会话没能回来（{error}）"))?;
    Ok(view)
}

/// **给这个世界里第一位 Human 起名并铸人**（名字由人给 —— 2026-10-20 主人裁决）。
///
/// 这是唯一的铸人入口；已经有主人时它只是补一支会话（幂等，**不会**改掉已有的名字）。
/// 名字不合法 ⇒ `Err`（一句话给人看）：页面已经先按同一条判据拦过一次，这里是第二道。
#[tauri::command]
pub async fn local_world_bootstrap(display_name: String) -> Result<LocalSessionView, String> {
    let view = tauri::async_runtime::spawn_blocking(move || -> LocalSessionView {
        let facts = LocalFacts::load(None, None);
        session_view(&facts, bootstrap_first_human(&facts, &display_name))
    })
    .await
    .map_err(|error| format!("壳内部错误：铸人那一步没能回来（{error}）"))?;
    Ok(view)
}

/// 一次会话结果 ⇒ 页面读数（**唯一**的映射处；两个命令共用）。
fn session_view(facts: &LocalFacts, outcome: SessionOutcome) -> LocalSessionView {
    match outcome {
        SessionOutcome::Ok {
            human_id,
            token,
            minted_human,
        } => LocalSessionView {
            state: "ok".to_string(),
            address: facts.world_base(),
            human_id: Some(human_id),
            token: Some(token),
            minted_human,
            detail: None,
        },
        // **这不是失败**：世界在跑、只是还没有你 ⇒ 页面的下一步是问名字。
        SessionOutcome::NoHuman => LocalSessionView {
            state: "no-human".to_string(),
            address: facts.world_base(),
            human_id: None,
            token: None,
            minted_human: false,
            detail: None,
        },
        SessionOutcome::WorldDown => LocalSessionView {
            state: "world-down".to_string(),
            address: facts.world_base(),
            human_id: human_id_of(facts),
            token: None,
            minted_human: false,
            detail: None,
        },
        SessionOutcome::NoCore => LocalSessionView {
            state: "no-core".to_string(),
            address: facts.world_base(),
            human_id: human_id_of(facts),
            token: None,
            minted_human: false,
            detail: Some(facts.layout.im_exe.display().to_string()),
        },
        SessionOutcome::Failed { detail } => LocalSessionView {
            state: "failed".to_string(),
            address: facts.world_base(),
            human_id: human_id_of(facts),
            token: None,
            minted_human: false,
            detail: Some(detail),
        },
    }
}

/// 一次会话的回报（**唯一带 token 的形状**；页面用完即存自己的持久面）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSessionView {
    /// 封闭词表：`ok` / `no-human` / `world-down` / `no-core` / `failed`。
    /// 页面必须**逐值**判：不认识的值一律不当 `ok`（fail-closed）。
    pub state: String,
    pub address: String,
    pub human_id: Option<String>,
    /// 会话原文（只有 `state == "ok"` 时在）。
    pub token: Option<String>,
    /// 这一次是不是刚铸了首位 Human（读数用；页面据此说"你是这个世界的第一位主人"）。
    pub minted_human: bool,
    /// 失败原因（原样）。
    pub detail: Option<String>,
}

/// 当前世界状态（`running` / `not-running` / `no-core`）。
pub fn world_state(facts: &LocalFacts) -> WorldState {
    if probe(&facts.world_base()) {
        return WorldState::Running;
    }
    if facts.layout.im_exe.is_file() {
        WorldState::NotRunning
    } else {
        WorldState::NoCore
    }
}

// ---------------------------------------------------------------------------
// 用例（纯逻辑 + 不碰真家的边界）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::home_facts::{HomeConf, Layout, LayoutKind};

    fn facts(home: &str, conf: HomeConf, im_exe: PathBuf) -> LocalFacts {
        LocalFacts {
            home_dir: PathBuf::from(home),
            conf,
            layout: Layout {
                kind: LayoutKind::Source,
                repo: PathBuf::from("/repo"),
                im_exe,
                host_exe: PathBuf::from("/repo/agent/target/release/agent_host"),
                run_exe: PathBuf::from("/repo/agent/target/release/agent_run"),
                status_exe: PathBuf::from("/repo/agent/target/release/agent_host_status"),
            },
            repo_dir: Some(PathBuf::from("/repo")),
        }
    }

    fn conf_with(pairs: &[(&str, &str)]) -> HomeConf {
        let text: String = pairs
            .iter()
            .map(|(key, value)| format!("{key}={value}\n"))
            .collect();
        home_facts::parse_conf(&text)
    }

    /// `confSet` 的四条判据（`conf.js`）：同键老行删掉、新行在末尾、其余行原样、LF 收尾。
    #[test]
    fn conf_set_matches_conf_js() {
        let before = "listen_addr=127.0.0.1:8080\r\nmcp_browser=stdio:npx x\r\nhuman_id=old\n";
        let after = conf_set_text(before, "human_id", "new-id");
        assert_eq!(
            after,
            "listen_addr=127.0.0.1:8080\nmcp_browser=stdio:npx x\nhuman_id=new-id\n"
        );
        // 值里含 `=`（`mcp_` 那一族）：只按第一个等号切 ⇒ 原样保留。
        let kept = conf_set_text("mcp_a=stdio:cmd --x=1\n", "human_id", "h-1");
        assert_eq!(kept, "mcp_a=stdio:cmd --x=1\nhuman_id=h-1\n");
        // 空文件 ⇒ 只有这一行（不留空行、不留前导换行）。
        assert_eq!(conf_set_text("", "human_id", "h-1"), "human_id=h-1\n");
        // 幂等：同一个键写两次，文件不长出第二行。
        let twice = conf_set_text(&after, "human_id", "new-id");
        assert_eq!(twice, after);
    }

    /// `confDelete` 的判据（`conf.js`）：删掉所有同键行、其余原样、LF 收尾；
    /// 第二个返回值 = **真的改了没有**（没改 ⇒ 调用方不写盘）。
    #[test]
    fn conf_delete_matches_conf_js() {
        let before = "listen_addr=127.0.0.1:8080\r\nagent_id=a-1\nroom_id=r-1\nmcp_a=stdio:x\n";
        let (after, changed) = conf_delete_text(before, "agent_id");
        assert_eq!(
            after,
            "listen_addr=127.0.0.1:8080\nroom_id=r-1\nmcp_a=stdio:x\n"
        );
        assert!(changed);
        // 同键**多行**全删（`confDelete` 是 filter，不是"删第一行"）。
        let (both, _) = conf_delete_text("agent_id=a\nagent_id=b\nkeep=1\n", "agent_id");
        assert_eq!(both, "keep=1\n");
        // 没有这个键 ⇒ 一个字不改，且如实说"没改"。
        let (same, touched) = conf_delete_text("human_id=h-1\n", "agent_id");
        assert_eq!(same, "human_id=h-1\n");
        assert!(!touched);
        // 全删光 ⇒ 那份文件里是一个空行（与 JS 的 `${kept.join('\n')}\n` 逐字对齐）。
        let (empty, _) = conf_delete_text("agent_id=a\n", "agent_id");
        assert_eq!(empty, "\n");
    }

    /// 读数：地址、端口的默认与覆盖、表态三态如实带上 —— 且**绝不含凭据**。
    #[test]
    fn readout_carries_address_ports_and_never_a_credential() {
        let basic = readout(
            &facts(
                "/home/u/celira-home",
                conf_with(&[("listen_addr", "127.0.0.1:9001")]),
                PathBuf::from("/repo/target/release/celira-im"),
            ),
            WorldState::Running,
            None,
        );
        assert_eq!(basic.state, "running");
        assert_eq!(basic.address, "http://127.0.0.1:9001");
        assert_eq!(basic.status_base, "http://127.0.0.1:5190");
        assert_eq!(basic.config_base, "http://127.0.0.1:5191");
        assert_eq!(
            basic.run_local_world, None,
            "还没表态 ⇒ null（界面该问一次）"
        );

        // 端口覆盖 + 表态 true。
        let overridden = readout(
            &facts(
                "/home/u/celira-home",
                conf_with(&[
                    ("listen_addr", "127.0.0.1:9001"),
                    ("status_port", "6190"),
                    ("config_port", "6191"),
                    ("human_id", "h-1"),
                ]),
                PathBuf::from("/repo/target/release/celira-im"),
            ),
            WorldState::NotRunning,
            Some(true),
        );
        assert_eq!(overridden.status_base, "http://127.0.0.1:6190");
        assert_eq!(overridden.config_base, "http://127.0.0.1:6191");
        assert_eq!(overridden.human_id.as_deref(), Some("h-1"));
        assert_eq!(overridden.run_local_world, Some(true));

        // 端口写得不成样子 ⇒ 回默认值（不把一个读不懂的值当端口用）。
        let broken = readout(
            &facts(
                "/home/u/celira-home",
                conf_with(&[("status_port", "not-a-port")]),
                PathBuf::from("/repo/target/release/celira-im"),
            ),
            WorldState::NoCore,
            Some(false),
        );
        assert_eq!(broken.status_base, "http://127.0.0.1:5190");
        assert_eq!(broken.state, "no-core");
    }

    /// 世界的地址来自 `home.conf`；缺省是 `127.0.0.1:8080`（与 `paths.js` 一字不差）。
    #[test]
    fn world_base_follows_home_conf_with_the_same_default() {
        let with_conf = facts(
            "/home/u/celira-home",
            conf_with(&[("listen_addr", "127.0.0.1:9100")]),
            PathBuf::from("/repo/target/release/celira-im"),
        );
        assert_eq!(with_conf.world_base(), "http://127.0.0.1:9100");
        let without = facts(
            "/home/u/celira-home",
            conf_with(&[]),
            PathBuf::from("/repo/target/release/celira-im"),
        );
        assert_eq!(without.world_base(), "http://127.0.0.1:8080");
    }

    /// 缺可执行件 ⇒ `no-core`（**不是**"没在跑"：两件事，两个读数）。
    ///
    /// ⚠ 用**一个没人听的端口**，否则探活会打到本机真跑着的那个世界上去（那正是这条用例
    /// 要区分的东西：状态先看世界答不答得上，再分"缺件"与"没在跑"）。
    #[test]
    fn missing_core_is_its_own_state() {
        let missing = facts(
            "/home/u/celira-home",
            conf_with(&[("listen_addr", "127.0.0.1:59233")]),
            PathBuf::from("/definitely/not/here/celira-im"),
        );
        assert_eq!(world_state(&missing), WorldState::NoCore);
        assert_eq!(WorldState::NoCore.as_str(), "no-core");
        assert_eq!(WorldState::NotRunning.as_str(), "not-running");
        assert_eq!(WorldState::Running.as_str(), "running");
    }

    /// 可执行件在、世界答不上 ⇒ `not-running`（可起；**不是**缺件）。
    #[test]
    fn a_silent_port_with_the_binary_present_is_merely_not_running() {
        let quiet = facts(
            "/home/u/celira-home",
            conf_with(&[("listen_addr", "127.0.0.1:59234")]),
            // 用**本进程自己的可执行件**当"存在但不是世界"的那个文件（平台无关）。
            std::env::current_exe().expect("current_exe"),
        );
        assert_eq!(world_state(&quiet), WorldState::NotRunning);
    }

    /// 世界答不上 ⇒ 会话那一步**停在"世界没在跑"**，什么都不做
    /// （判据 2 的形状：问不到就不动）。可执行件在（拿本进程自己当那个文件）。
    #[test]
    fn session_stops_when_the_world_is_down() {
        let down = facts(
            "/home/u/celira-home",
            conf_with(&[("listen_addr", "127.0.0.1:59231")]),
            std::env::current_exe().expect("current_exe"),
        );
        assert!(matches!(local_session(&down), SessionOutcome::WorldDown));
    }

    /// 缺可执行件 ⇒ 会话那一步报 `NoCore`（不试着连、不猜）。
    #[test]
    fn session_reports_missing_core_before_anything_else() {
        let missing = facts(
            "/home/u/celira-home",
            conf_with(&[("listen_addr", "127.0.0.1:59232")]),
            PathBuf::from("/definitely/not/here/celira-im"),
        );
        assert!(matches!(local_session(&missing), SessionOutcome::NoCore));
    }

    /// 首位 Human 的显示名**由人给**：三条规则与 Core 同一套（禁 NUL / ≤80 个 scalar /
    /// trim 后非空），返回的是 trim 之后要提交的正文。**机器用户名不再参与**（2026-10-20）。
    #[test]
    fn human_name_is_whatever_the_person_types_nothing_from_the_machine() {
        assert_eq!(normalize_human_name("测试名字").as_deref(), Ok("测试名字"));
        assert_eq!(
            normalize_human_name("  测试名字  ").as_deref(),
            Ok("测试名字"),
            "两头空白由壳削掉（与兑换邀请那条路同款）"
        );
        assert_eq!(normalize_human_name(""), Err(HumanNameError::Empty));
        assert_eq!(normalize_human_name("   "), Err(HumanNameError::Empty));
        assert_eq!(
            normalize_human_name("a\u{0}b"),
            Err(HumanNameError::ContainsNul)
        );
        // 80 个 scalar 可以，81 个不行 —— 按**字符**数，不按字节（与 `DisplayName::parse` 同款）。
        let eighty = "字".repeat(80);
        assert!(normalize_human_name(&eighty).is_ok());
        assert_eq!(
            normalize_human_name(&"字".repeat(81)),
            Err(HumanNameError::TooLong)
        );
        // emoji 是单个 scalar value（JS 那边 `[...s].length` 与它对齐，别写成 `.length`）。
        assert!(normalize_human_name(&"🐱".repeat(80)).is_ok());
        assert_eq!(
            normalize_human_name(&"🐱".repeat(81)),
            Err(HumanNameError::TooLong)
        );
        // 反向判据（这一刀的全部理由）：壳里**不许**再出现"从机器环境造名字"的路
        // （判据盯的是那两次 `env::var`，不是这两个词出现 —— 注释里提它们不算违规）。
        let source = include_str!("local_world.rs");
        assert!(
            !source.contains("env::var(\"USERNAME\")") && !source.contains("env::var(\"USER\")"),
            "壳不许再读机器用户名去当显示名"
        );
    }

    /// 没有主人 ⇒ 会话那一格回 **`no-human`**（不是失败、也不是 `ok`）：
    /// 页面据此问名字；壳不替它铸一个。
    #[test]
    fn a_world_without_a_human_says_so_and_mints_nobody() {
        let home = "/home/u/celira-home";
        let empty = facts(
            home,
            conf_with(&[]),
            PathBuf::from("/repo/target/release/celira-im"),
        );
        assert_eq!(human_id_of(&empty), None);
        // 世界没在跑 ⇒ 先撞 WorldDown（探活那一关），这一条钉的是**顺序**：
        // 没有主人时也绝不悄悄铸人 —— 铸人只走 `bootstrap_first_human`。
        let down = facts(
            home,
            conf_with(&[("listen_addr", "127.0.0.1:59233")]),
            std::env::current_exe().expect("current_exe"),
        );
        assert!(matches!(local_session(&down), SessionOutcome::WorldDown));
    }

    /// 会话结果 ⇒ 页面读数：`NoHuman` 必须落成 `no-human`（封闭词表那一格），
    /// 而且**不带 token、不谎报主人**。
    #[test]
    fn no_human_maps_to_the_no_human_word() {
        let f = facts(
            "/home/u/celira-home",
            conf_with(&[]),
            PathBuf::from("/repo/target/release/celira-im"),
        );
        let view = session_view(&f, SessionOutcome::NoHuman);
        assert_eq!(view.state, "no-human");
        assert!(view.token.is_none());
        assert!(view.human_id.is_none());
        assert!(!view.minted_human);
    }

    /// `human_id` 只在**非空**时算数（空值按"还没铸人"，与 `conf.js` 的空串处置对齐）。
    #[test]
    fn human_id_is_absent_when_blank() {
        let blank = facts(
            "/home/u/celira-home",
            conf_with(&[("human_id", "")]),
            PathBuf::from("/repo/target/release/celira-im"),
        );
        assert_eq!(human_id_of(&blank), None);
        let present = facts(
            "/home/u/celira-home",
            conf_with(&[("human_id", "h-1")]),
            PathBuf::from("/repo/target/release/celira-im"),
        );
        assert_eq!(human_id_of(&present).as_deref(), Some("h-1"));
    }
}
