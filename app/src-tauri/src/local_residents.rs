//! **本机住户**——「这台机器上住着谁、主体该在跑吗、把它起起来」。
//!
//! ## 这一片解决什么
//!
//! §8.1：**本地进程是本地 app 的孩子**。世界的生命周期已经归壳（[`crate::local_world`]），
//! 但住户那半从前没有主人：`home.js up` 起过它、安家那只手起过它，**app 打开时不会起它**
//! ⇒ 用户看到的是"世界在跑、它却不在线、也不回话"，而他没有办法（除了去命令行）。
//!
//! ## 判据（三条，都不发明）
//!
//! 1. **谁是住户**：`<家>/agents/<支>/` 且（有 `agent.conf` 或 `identity.json`）
//!    —— 与 `residents.js` 的 `residents()` 同款，形状权威在 [`crate::registration`]。
//! 2. **主体在这台机器上在不在跑**：**账本**（`logs/agents/<支>/host.pid`，谁起的谁记账）
//!    ＋ **身份核对**（那个 pid 现在还是我们的宿主吗 —— [`crate::process_facts`]，照
//!    `proc.js` 的"杀之前先验身份"）。**不扫进程表**（§8.7 撤掉的正是"认领别人的进程"）。
//! 3. **该不该起**：**账本 ＋ 身份核对**决定动作（判据表与用例见 [`action_for`]）——
//!    在跑 ⇒ 不动；账本不在 / 陈旧 / 指向别人 / **核对不了** ⇒ **起它**（接管，`up.js` 同款）。
//!    ⚠ **presence 只当读数、不当闸**：世界说主体在线（`GET /v1/rooms/<房>/live`）会照问照报，
//!    但那是一个有 `SEEN_TTL` 滞后的投影 —— 拿它当硬闸会让**刚停过的住户再也起不来**
//!    （2026-10-03 真机栽过：陈旧账本指向一个打不开的 pid ⇒ `unverifiable` ⇒ 被自己的闸挡死）。
//!
//! ## 边界（如实登记）
//!
//! - **不起世界**（那是 [`crate::local_world::ensure_world`] 的事）；世界没在跑 ⇒ 这一片只报读数；
//! - **不做 PID 认领**：账本陈旧 ⇒ 当"没在跑"处理并**重起**（不杀、不认领任何进程）；
//! - **不轮换凭据**：起的时候**读**本地存储里那份（按「世界 × 身份」分条）；读不到 ⇒ 如实拒绝
//!   （轮换是安家那只手的事，见 [`crate::anjia`]）；
//! - **不承诺生命周期**：崩了不自动重启（§8.6-2）；停它归托盘/退出语义那一刀（§8.2-8.3）。

use std::path::{Path, PathBuf};

use crate::anjia::host_process;
use crate::home_facts;
use crate::local_world::LocalFacts;
use crate::process_facts::{self, Ownership};
use crate::registration::{self, AgentConf};

/// 一支本机住户（机器侧事实：它在哪、它的账本在哪）。
#[derive(Debug, Clone)]
pub struct Resident {
    pub agent_id: String,
    pub env_root: PathBuf,
    pub conf: AgentConf,
    pub pid_file: PathBuf,
    pub log_file: PathBuf,
}

impl Resident {
    /// 显示名（登记里那行；空 ⇒ 报 id 的短形式，不拿别处顶替）。
    pub fn label(&self) -> String {
        if self.conf.display_name.is_empty() {
            self.agent_id.chars().take(8).collect()
        } else {
            self.conf.display_name.clone()
        }
    }
}

/// 这个家住着哪几支（**只认新布局**，与 `residents.js` 同款）。
pub fn residents(home_dir: &Path) -> Vec<Resident> {
    registration::resident_env_roots(home_dir)
        .into_iter()
        .filter_map(|env_root| {
            let agent_id = env_root.file_name()?.to_string_lossy().to_string();
            Some(Resident {
                conf: registration::read_agent_conf(&env_root),
                pid_file: registration::host_pid_file(home_dir, &agent_id),
                log_file: registration::host_log_file(home_dir, &agent_id),
                env_root,
                agent_id,
            })
        })
        .collect()
}

/// 它在**这台机器上**在不在跑（三态 + 一档"核对不了"）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResidentState {
    /// 在跑：账本里的 pid 活着，而且确实是我们的宿主。
    Running { pid: u32 },
    /// 没在跑：账本不在，或账本里那个 pid 上什么都没有（陈旧账本 ⇒ 如实说）。
    NotRunning { stale_ledger: Option<u32> },
    /// 账本里那个 pid 上是**别的程序**（pid 撞车）——不认领、不杀，如实说。
    NotOurs { pid: u32, exe: PathBuf },
    /// 核对不了（平台问不出路径 / 查询失败）——**不许据此动手**。
    Unverifiable { pid: u32, detail: String },
}

impl ResidentState {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Running { .. } => "running",
            Self::NotRunning { .. } => "not-running",
            Self::NotOurs { .. } => "not-ours",
            Self::Unverifiable { .. } => "unverifiable",
        }
    }
}

/// 读账本里的 pid（文件不在 / 读不懂 ⇒ `None`）。
pub fn ledger_pid(pid_file: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(pid_file).ok()?;
    text.trim().parse::<u32>().ok().filter(|pid| *pid > 0)
}

/// 它的现状：**账本 + 身份核对**（不扫进程表）。
pub fn state_of(resident: &Resident, host_exe: &Path) -> ResidentState {
    let Some(pid) = ledger_pid(&resident.pid_file) else {
        return ResidentState::NotRunning { stale_ledger: None };
    };
    match process_facts::ownership(pid, host_exe) {
        Ownership::Ours { .. } => ResidentState::Running { pid },
        Ownership::Gone => ResidentState::NotRunning {
            stale_ledger: Some(pid),
        },
        Ownership::NotOurs { pid, exe } => ResidentState::NotOurs { pid, exe },
        Ownership::Unknown(detail) => ResidentState::Unverifiable { pid, detail },
    }
}

// ---------------------------------------------------------------------------
// 起它
// ---------------------------------------------------------------------------

/// 一次"起它"的结局（每一态都有名字，给读数用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartOutcome {
    /// 它本来就在跑（账本指向我们的宿主）—— 一个字都没动。
    AlreadyRunning { pid: u32 },
    /// 起来了（pid 已记账本）。
    Started { pid: u32 },

    /// 停手：缺可执行件（这台机器上还没有宿主二进制）。
    RefusedNoCore { exe: PathBuf },
    /// 停手：模型那一半没人在（且没有文件档绑定）—— 与 `up.js` 的早失败口径同款。
    RefusedMissingModel,
    /// 停手：本地存储里没有它的后端凭据（或读不出来）—— 起不了（宿主认证不过）。
    RefusedMissingCredential { detail: String },
    /// 失败：起进程那一步没成（原因原样）。
    Failed { detail: String },
}

impl StartOutcome {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::AlreadyRunning { .. } => "already-running",
            Self::Started { .. } => "started",
            Self::RefusedNoCore { .. } => "refused-no-core",
            Self::RefusedMissingModel => "refused-missing-model",
            Self::RefusedMissingCredential { .. } => "refused-missing-credential",
            Self::Failed { .. } => "failed",
        }
    }
}

/// 起一支住户（**唯一一处"怎么起本机住户"**；安家那只手与"打开 app"都走这里）。
///
/// `wait_for_report`：安家那条路要等报到（它的成功判据就是报到）；"打开 app"那条路不等
/// （不为了报一句读数把正门堵二十秒）—— 状态由下一次读数/探测给出。
pub fn start_resident(
    facts: &LocalFacts,
    resident: &Resident,
    wait_for_report: bool,
) -> StartOutcome {
    let host_exe = &facts.layout.host_exe;
    if !host_exe.is_file() {
        return StartOutcome::RefusedNoCore {
            exe: host_exe.clone(),
        };
    }
    if let ResidentState::Running { pid } = state_of(resident, host_exe) {
        return StartOutcome::AlreadyRunning { pid };
    }
    // 模型那一半：**只认绑定**（2026-10-07 主人裁：app 形态不继承环境变量那两条 —— 见
    // `host_process::model_plan` 的判据）。没绑定 ⇒ 这一支还没配，如实拒绝起它。
    let plan = match host_process::model_plan(&facts.home_dir, &resident.env_root) {
        Ok(plan) => plan,
        Err(host_process::ModelPlanError::BindingUnreadable(_)) => {
            return StartOutcome::RefusedMissingModel
        }
        Err(host_process::ModelPlanError::MissingBinding) => {
            return StartOutcome::RefusedMissingModel
        }
    };
    // 它的后端凭据：**读**（不轮换）本地存储里按「世界 × 身份」分条那份。
    let credential_dir = home_facts::credentials_dir();
    let world = facts.world_base();
    let backend = match home_facts::read_credential(&credential_dir, &world, &resident.agent_id) {
        home_facts::CredentialRead::Present(token) => token,
        home_facts::CredentialRead::Missing => {
            return StartOutcome::RefusedMissingCredential {
                detail: format!(
                "本地存储里没有这支 Agent（{}）在这个世界（{world}）的后端凭据 —— 先在 app 里给它安一次家",
                resident.agent_id
            ),
            }
        }
        home_facts::CredentialRead::Unreadable(reason) => {
            return StartOutcome::RefusedMissingCredential {
                detail: format!("这支 Agent 的后端凭据读不出来（{reason}）—— 先看清它再重试"),
            }
        }
    };
    let identity = Some((
        resident.agent_id.clone(),
        if resident.conf.display_name.is_empty() {
            resident.label()
        } else {
            resident.conf.display_name.clone()
        },
    ));
    let spec = host_process::HostSpec {
        home_dir: facts.home_dir.clone(),
        env_root: resident.env_root.clone(),
        agent_id: resident.agent_id.clone(),
        listen: facts.listen(),
        identity,
        model_sources_file: plan.sources_file(),
        web_sources_file: Some(facts.home_dir.join("web_sources.json")),
        platform: host_process::HostPlatform::current(),
    };
    let launch = host_process::HostLaunch {
        exe: host_exe,
        spec: &spec,
        backend_credential: &backend,
        model_api_key: plan.env_key(),
        log_file: &resident.log_file,
    };
    let spawned = match host_process::spawn_agent_host(&launch) {
        Ok((spawned, _child)) => spawned,
        Err(detail) => return StartOutcome::Failed { detail },
    };
    // **谁起的谁记账**（账本写不上 ⇒ 如实报失败：那会让下一次"打开 app"又起一个）。
    if let Err(detail) = registration::write_pid(&resident.pid_file, spawned.pid) {
        return StartOutcome::Failed {
            detail: format!("宿主起来了（pid {}），但账本没记上：{detail}", spawned.pid),
        };
    }
    if wait_for_report {
        match host_process::wait_for_report(&spawned.log_file, 40, 500) {
            host_process::HostReport::Listening => {}
            host_process::HostReport::Exited { lines } => {
                return StartOutcome::Failed {
                    detail: format!("它起来后自己退了：{}", lines.join(" / ")),
                }
            }
            // **第三态**（`up.js` 同款）：预算内什么都没读到 —— 不是失败也不是成功。
            // 账本已经记了、进程在跑，如实说"还没报到"，不当成失败（那会让调用方重起一个）。
            host_process::HostReport::NotYet => {}
        }
    }
    StartOutcome::Started { pid: spawned.pid }
}

// ---------------------------------------------------------------------------
// 「打开 app ⇒ 它就在」
// ---------------------------------------------------------------------------

/// 一次"确保本机住户都在跑"的读数（每支一行）。
#[derive(Debug, Clone)]
pub struct ResidentReport {
    pub agent_id: String,
    pub label: String,
    pub state: ResidentState,
    /// 世界此刻说它在不在线（`None` = 没问 / 问不到）—— **读数**，不是闸。
    pub presence_online: Option<bool>,
    pub outcome: Option<StartOutcome>,
}

/// 确保这个家的住户都在跑（**幂等**；世界没在跑 ⇒ 只报读数，一个都不起）。
///
/// ## 判据：**账本 + 身份核对**决定动作，presence 只当读数（2026-10-03 真机修的）
///
/// 从前 presence 是一道**硬闸**（"世界说它在线 ⇒ 不替你起"）。真机上它把产品自己的承诺挡死了：
/// 它的账本指向一个**打不开的 pid**（旧 pid 被复用、`OpenProcess` 拒绝访问 ⇒ `unverifiable`），
/// 于是"核对不上 ⇒ 不动"，它**永远起不来**（日志读数就是 `住户 小月：unverifiable`）。
///
/// 现在照 `up.js` 的做法（*"要修账本就 `node home.js up`（会接管）"*）：
///
/// | 账本 | 身份核对 | 动作 |
/// |---|---|---|
/// | 有 | 是我们的、活着 | **不动**（它本来就在跑） |
/// | 有 | 是别的程序 / pid 上什么都没有 / **核对不了** | **起它**（账本陈旧或核不出 ⇒ 接管） |
/// | 没有 | — | **起它** |
///
/// ⚠ 为什么 presence **不能**当硬闸：它是**世界侧的投影**，按契约有 `SEEN_TTL` 的滞后
/// （`CORE_V1_AGENT_PROTOCOL.md` §6.1：*"拉取式 Backend 停止活动后最多 SEEN_TTL 内仍可能被判为
/// 在线——这是'最近被看见'的固有代价"*）。拿一个会滞后的投影去否决"这台机器该不该有宿主"，
/// 就会让**刚停过的住户再也起不来**。它仍然**照问照报**（`presence_online` 那一格），
/// 只是不替用户做决定 —— 这也与"行动只信账本"那条纪律一致。
/// 对一支住户该**动不动手**（纯函数：判据表就是它的用例）。
///
/// | 账本 | 身份核对 | 世界在跑 | 动作 |
/// |---|---|---|---|
/// | 有 | 是我们的、活着 | — | **不动**（它本来就在跑） |
/// | 有 | 别的程序 / pid 上什么都没有 / 核对不了 | 在跑 | **起它**（账本陈旧或核不出 ⇒ 接管） |
/// | 没有 | — | 在跑 | **起它** |
/// | 任意 | 任意 | **没在跑** | **不动**（它起来也连不上谁） |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidentAction {
    /// 一个字都不动。
    Leave,
    /// 起它（`start_resident` 内部还会再核一次状态）。
    Start,
}

/// 判据表（**唯一一处**；`ensure_residents` 与用例都走它）。
pub fn action_for(state: &ResidentState, world_up: bool) -> ResidentAction {
    if !world_up {
        return ResidentAction::Leave;
    }
    match state {
        ResidentState::Running { .. } => ResidentAction::Leave,
        ResidentState::NotRunning { .. }
        | ResidentState::NotOurs { .. }
        | ResidentState::Unverifiable { .. } => ResidentAction::Start,
    }
}
pub fn ensure_residents(facts: &LocalFacts, human_token: Option<&str>) -> Vec<ResidentReport> {
    let list = residents(&facts.home_dir);
    if list.is_empty() {
        return Vec::new();
    }
    let world_up = crate::world_channel::WorldHttp::new(&facts.world_base())
        .map(|http| http.health_ok())
        .unwrap_or(false);
    list.into_iter()
        .map(|resident| {
            let state = state_of(&resident, &facts.layout.host_exe);
            let presence = if world_up {
                presence_of(facts, &resident, human_token)
            } else {
                None
            };
            // 动不动手**只有一处判据**（`action_for`；世界没在跑 ⇒ 一个都不起）。
            let outcome = match action_for(&state, world_up) {
                ResidentAction::Leave => None,
                ResidentAction::Start => Some(start_resident(facts, &resident, false)),
            };
            ResidentReport {
                label: resident.label(),
                agent_id: resident.agent_id,
                state,
                presence_online: presence,
                outcome,
            }
        })
        .collect()
}

/// 世界此刻说它在不在线（`GET /v1/rooms/<房>/live` —— 主体进行态的唯一裁决面）。
///
/// 三态：`Some(true/false)` 问到了 / `None` 问不到（没有会话、它那间房还不知道、请求失败）。
/// **这是读数**：调用方把它摆出来，不据此动手（理由见 `ensure_residents` 的判据表）。
fn presence_of(facts: &LocalFacts, resident: &Resident, human_token: Option<&str>) -> Option<bool> {
    let token = human_token?;
    let room_id = resident.conf.room_id.as_str();
    if room_id.is_empty() {
        return None;
    }
    let http = crate::world_channel::WorldHttp::new(&facts.world_base()).ok()?;
    http.room_live_online(token, room_id, &resident.agent_id)
        .ok()
        .flatten()
}
// ---------------------------------------------------------------------------
// 用例（纯逻辑：住户清单、账本、状态；**不起任何进程**）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("celira-res-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn seed_resident(home: &Path, id: &str, slot: u32, name: &str, room: &str) -> PathBuf {
        let env_root = home.join("agents").join(id);
        fs::create_dir_all(&env_root).unwrap();
        fs::write(
            env_root.join("identity.json"),
            format!("{{\"agent\":\"{id}\"}}"),
        )
        .unwrap();
        registration::write_agent_conf(
            &env_root,
            &AgentConf {
                agent_id: id.to_string(),
                room_id: room.to_string(),
                slot: Some(slot),
                display_name: name.to_string(),
            },
        )
        .unwrap();
        env_root
    }

    #[test]
    fn residents_come_from_the_registration_not_from_a_scan() {
        let home = temp_home("list");
        seed_resident(&home, "a-1", 1, "小月", "r-1");
        // 半成品目录（既无 agent.conf 也无 identity.json）不算住户。
        fs::create_dir_all(home.join("agents").join("junk")).unwrap();
        let list = residents(&home);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].agent_id, "a-1");
        assert_eq!(list[0].label(), "小月");
        // 账本与日志落在 layout 那条路上（`paths.js` 的 agentPidFile / agentHostLog）。
        assert_eq!(
            list[0].pid_file,
            home.join("logs")
                .join("agents")
                .join("a-1")
                .join("host.pid")
        );
        assert_eq!(
            list[0].log_file,
            home.join("logs")
                .join("agents")
                .join("a-1")
                .join("host.out")
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn label_falls_back_to_a_short_id_never_to_something_else() {
        let home = temp_home("label");
        seed_resident(&home, "e4e4410d-75e0-436c-a35b-273eb4dfe3f8", 1, "", "");
        let list = residents(&home);
        assert_eq!(list[0].label(), "e4e4410d", "没有显示名 ⇒ 报 id 的短形式");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn ledger_pid_is_read_strictly() {
        let home = temp_home("ledger");
        let file = home.join("host.pid");
        assert_eq!(ledger_pid(&file), None, "文件不在 ⇒ 没有账本");
        fs::write(&file, "4242\n").unwrap();
        assert_eq!(ledger_pid(&file), Some(4242));
        fs::write(&file, " 4242 ").unwrap();
        assert_eq!(ledger_pid(&file), Some(4242), "两侧空白不算数");
        for bad in ["", "abc", "0", "-3", "12.5"] {
            fs::write(&file, bad).unwrap();
            assert_eq!(
                ledger_pid(&file),
                None,
                "读不懂的账本按「没有」处理：{bad:?}"
            );
        }
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn state_distinguishes_missing_ledger_stale_ledger_and_our_own_process() {
        let home = temp_home("state");
        let exe = std::env::current_exe().unwrap();
        let env_root = seed_resident(&home, "a-1", 1, "小月", "r-1");
        let resident = Resident {
            agent_id: "a-1".to_string(),
            env_root,
            conf: registration::read_agent_conf(&home.join("agents").join("a-1")),
            pid_file: registration::host_pid_file(&home, "a-1"),
            log_file: registration::host_log_file(&home, "a-1"),
        };
        // 账本不在 ⇒ 没在跑（而且**不假装**有陈旧账本）。
        assert_eq!(
            state_of(&resident, &exe),
            ResidentState::NotRunning { stale_ledger: None }
        );
        // 账本指向一个**不存在的** pid ⇒ "陈旧账本"（如实带出来，供读数说清）。
        // ⚠ 用一个大到不会撞上的 pid：0 与负数由 `ledger_pid` 直接当"没有账本"（那不是账本）。
        let ghost = 4_000_000u32;
        fs::create_dir_all(resident.pid_file.parent().unwrap()).unwrap();
        registration::write_pid(&resident.pid_file, ghost).unwrap();
        assert_eq!(
            state_of(&resident, &exe),
            ResidentState::NotRunning {
                stale_ledger: Some(ghost)
            }
        );
        // 账本指向**本进程**，而期望的可执行件就是本进程 ⇒ 在跑（身份核对通过）。
        registration::write_pid(&resident.pid_file, std::process::id()).unwrap();
        match state_of(&resident, &exe) {
            ResidentState::Running { pid } => assert_eq!(pid, std::process::id()),
            #[cfg(not(any(windows, target_os = "linux")))]
            ResidentState::Unverifiable { .. } => {}
            other => panic!("本进程该被认成在跑的宿主：{other:?}"),
        }
        // 期望的可执行件换成别的程序 ⇒ 账本指向的不是"我们的宿主"（**不认领**）。
        let other = state_of(&resident, Path::new(r"D:\definitely\not\us\agent_host.exe"));
        assert!(
            matches!(
                other,
                ResidentState::NotOurs { .. } | ResidentState::Unverifiable { .. }
            ),
            "换个期望路径就不该是 Running：{other:?}"
        );
        let _ = fs::remove_dir_all(&home);
    }

    /// **判据表**（这一刀在真机上栽过一次的地方）：账本 ＋ 身份核对决定动作，
    /// presence 只当读数；世界没在跑则一个都不起。
    #[test]
    fn the_action_table_is_the_one_written_down() {
        // 在跑 ⇒ 一个字都不动（哪怕世界没在跑）。
        assert_eq!(
            action_for(&ResidentState::Running { pid: 7 }, true),
            ResidentAction::Leave
        );
        assert_eq!(
            action_for(&ResidentState::Running { pid: 7 }, false),
            ResidentAction::Leave
        );
        // 账本不在 / 陈旧 / 指向别人 / **核对不了** ⇒ 都起它（接管；`up.js` 同款）。
        // ⚠ `Unverifiable` 必须是 Start：真机上它曾被当成"不动"，于是刚停过的住户**永远起不来**
        //   （日志读数就是 `住户 小月：unverifiable`）。
        for state in [
            ResidentState::NotRunning { stale_ledger: None },
            ResidentState::NotRunning {
                stale_ledger: Some(4_000_000),
            },
            ResidentState::NotOurs {
                pid: 8,
                exe: PathBuf::from("/usr/bin/other"),
            },
            ResidentState::Unverifiable {
                pid: 9,
                detail: "打不开（拒绝访问）".to_string(),
            },
        ] {
            assert_eq!(
                action_for(&state, true),
                ResidentAction::Start,
                "{state:?} 该被接管"
            );
            // 世界没在跑 ⇒ 不起（它起来也连不上谁）。
            assert_eq!(
                action_for(&state, false),
                ResidentAction::Leave,
                "{state:?}：世界没在跑时不该起"
            );
        }
    }

    #[test]
    fn outcomes_have_stable_names() {
        assert_eq!(
            StartOutcome::AlreadyRunning { pid: 1 }.kind(),
            "already-running"
        );
        assert_eq!(StartOutcome::Started { pid: 1 }.kind(), "started");

        assert_eq!(
            StartOutcome::RefusedMissingModel.kind(),
            "refused-missing-model"
        );
        assert_eq!(
            StartOutcome::RefusedNoCore {
                exe: PathBuf::from("/x")
            }
            .kind(),
            "refused-no-core"
        );
        assert_eq!(
            StartOutcome::RefusedMissingCredential {
                detail: "x".to_string()
            }
            .kind(),
            "refused-missing-credential"
        );
        assert_eq!(
            StartOutcome::Failed {
                detail: "x".to_string()
            }
            .kind(),
            "failed"
        );
    }
}
