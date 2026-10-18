//! **家的形状迁移** —— `scripts/home/residents.js` 的 `migrateHomeShape` 的**壳侧镜像**。
//!
//! ## 为什么要它（2026-10-05 主人裁：壳自己搬）
//!
//! 老家形状（一支一个环境根：`<家>/env`）在 CLI 形态里**首次 `up` 就会自动迁成**
//! `<家>/agents/<支>/`（`up.js` 在读住户**之前**调 `migrateHomeShape`，幂等）。而 app 形态
//! 从前不迁 ⇒ 同一个家，用 CLI 打开会搬、用 app 打开不搬，读数还说"这台机器上还没有 Agent"
//! ——**同一件事两个口径**。判据两条：**用户直觉**（app 是"不用命令行那个形态"，不该让用户去敲
//! `node home.js up`）＋**主流做法**（内部布局的迁移在打开时自动做，用户根本不该知道
//! `<家>/env` 这种实现细节）。⇒ 壳在开机时照同一份语义搬。
//!
//! ## 权威与镜像纪律
//!
//! **语义权威是 `residents.js` 的 `migrateHomeShape`**（六种结局：`none / migrated / aside /
//! finish / refuse / failed`）。本文件逐条对齐它，与两个门（`home_doors.rs` 对
//! `status-server.js` / `config-server.js`）同一条镜像纪律：**形状与拒绝条件照抄，人话归调用方**。
//!
//! ## 一条**壳做不到**的（如实拒绝，不含糊）
//!
//! 更老的那一档 —— `home.conf` **没有 `home_id`** 而旧身份还在：换键会在世界里**另建一支
//! Agent**（旧那支留着不动）。那件事要动世界（重放布置键），**只有 CLI 能做** ⇒ 壳停手，
//! 并把那条真路径指出来（那不是"假路径"：那个用户本来就是从 CLI 时代过来的）。
//!
//! ## 纪律
//!
//! - **只改名，不删内容**：`<家>/env` 整份 `rename` 到住户位；目标被占时旧那份**整份让位**
//!   （`<家>/env.old-<支>`），**不覆盖、不合并、不删**；
//! - **搬不动就一个字都不改**（改名失败 ⇒ `failed`，可重跑）；
//! - **老宿主还在跑时一律停手**（搬文件不能在它用着那些文件的时候做）——判据与 JS 同款两条：
//!   账本（`logs/host.pid` 的 `ours` / **核对不了**）＋ 进程表里 `--root` 落在 `<家>/env` 的宿主；
//! - **幂等**：新形状的家 ⇒ 一眼看出"没得可迁"，什么都不做。

use std::fs;
use std::path::{Path, PathBuf};

use crate::home_facts;
use crate::local_residents;
use crate::process_facts::{self, Ownership};
use crate::registration;

/// 迁移的结局（**照 JS 的 `action` 逐字**）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShapeAction {
    /// 没得可迁（新形状的家 / 空 env / 没有身份）——一个字都不动。
    None,
    /// 老家整份迁成住户 1（`<家>/env` ⇒ `<家>/agents/<支>/`）。
    Migrated,
    /// 目标住户位已被占 ⇒ 旧那份**整份让位**（不删不合并）。
    Aside,
    /// 收尾：环境已经在住户位，而 `home.conf` 还留着旧两行 ⇒ 让位。
    Finish,
    /// **停手**：这一次什么都不改（`refuse` 里说清为什么）。
    Refuse,
    /// 改名失败 ⇒ 停在这里，什么都没改（可重跑）。
    Failed,
}

impl ShapeAction {
    /// 稳定的分类字（读数/日志用；**词汇表就是这四个字母以外的那几个**，不翻译）。
    pub fn as_str(&self) -> &'static str {
        match self {
            ShapeAction::None => "none",
            ShapeAction::Migrated => "migrated",
            ShapeAction::Aside => "aside",
            ShapeAction::Finish => "finish",
            ShapeAction::Refuse => "refuse",
            ShapeAction::Failed => "failed",
        }
    }
}

/// 一次迁移的读数（与 JS 的返回逐格对应）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeOutcome {
    pub ok: bool,
    pub action: ShapeAction,
    pub agent_id: Option<String>,
    pub env_aside: Option<PathBuf>,
    /// 做成了什么（要摆给人看的几行；`refuse` / `failed` 时为空）。
    pub lines: Vec<String>,
    /// 非空 ⇒ **这一次什么都没改**，原样把这句话交出去。
    pub refuse: Option<String>,
    /// 拒绝的**稳定原因码**（`world-key` / `host-running`）—— 人话归页面，这一格只报事实。
    pub blocker: Option<&'static str>,
}

/// 拒绝的两个原因码（**稳定字符串 = 一份词汇表**，页面按它选人话；不翻译、不拼词）。
pub const BLOCKER_WORLD_KEY: &str = "world-key";
pub const BLOCKER_HOST_RUNNING: &str = "host-running";
/// 第三个：改名/建目录失败（**没搬成，但什么都没改**，可重跑）——与上面两个的下一步不同，
/// 所以它也得有自己的码（不然页面只能说一句"没搬成"，而用户不知道该做什么）。
pub const BLOCKER_MOVE_FAILED: &str = "move-failed";

/// 上一次迁移留下的原因码（进程内；状态口那一格读它 —— **给页面说准是哪一种**）。
///
/// 为什么要有它：页面看到"老形态"时，两种可能的下一步完全不同（**停掉它，下次开机 app 自己搬**
/// vs **最老那一档要走 CLI**）。不报原因 ⇒ 页面只能把两种并列摆出来让人猜（2026-10-05 那一版
/// 就是这么写的）——那是"该说的话没说准"。
static LAST_BLOCKER: std::sync::Mutex<Option<&'static str>> = std::sync::Mutex::new(None);

/// 记下这一次的结局（开机序列调一次；`None` = 没有拦路的东西，**会清掉上一次的**）。
pub fn remember(outcome: &ShapeOutcome) {
    if let Ok(mut slot) = LAST_BLOCKER.lock() {
        *slot = outcome.blocker;
    }
}

/// 上一次迁移的原因码（没跑过 / 没拦路的 ⇒ `None`）。
pub fn last_blocker() -> Option<&'static str> {
    LAST_BLOCKER.lock().ok().and_then(|slot| *slot)
}

impl ShapeOutcome {
    fn none() -> Self {
        Self {
            ok: true,
            action: ShapeAction::None,
            agent_id: None,
            env_aside: None,
            lines: Vec::new(),
            refuse: None,
            blocker: None,
        }
    }

    fn refuse(message: String, blocker: &'static str) -> Self {
        Self {
            ok: false,
            action: ShapeAction::Refuse,
            agent_id: None,
            env_aside: None,
            lines: Vec::new(),
            refuse: Some(message),
            blocker: Some(blocker),
        }
    }

    fn failed(message: String) -> Self {
        Self {
            ok: false,
            action: ShapeAction::Failed,
            agent_id: None,
            env_aside: None,
            lines: Vec::new(),
            refuse: Some(message),
            blocker: Some(BLOCKER_MOVE_FAILED),
        }
    }
}

/// 老家那一支的环境根（`paths.js` 的 `legacyEnvRoot`）。
pub fn legacy_env_root(home_dir: &Path) -> PathBuf {
    home_dir.join("env")
}

/// **家的形状迁移**（幂等）。`host_running` 是给用例注入的判据（缺省走账本 ＋ 进程表）。
pub fn migrate_home_shape(
    home_dir: &Path,
    host_exe: &Path,
    host_running: Option<&dyn Fn() -> bool>,
) -> ShapeOutcome {
    let mut lines: Vec<String> = Vec::new();
    let legacy_env = legacy_env_root(home_dir);
    let legacy_exists = legacy_env.exists();
    let conf = home_facts::read_conf(home_dir);
    let home_id = conf.home_id.unwrap_or_default();
    let conf_agent_id = conf.agent_id.unwrap_or_default();
    let conf_room_id = conf.room_id.unwrap_or_default();
    let identity_agent_id = home_facts::read_env_identity(&legacy_env).unwrap_or_default();
    // ⚠ 两处都可能没有 ⇒ 归一成空串（`Option` 的 `None` 与"空"必须是同一个意思，
    //   否则下面那些"老家还在不在"的判据会把全新家误判成老家 —— JS 那边真机撞过）。
    let stale_agent = if conf_agent_id.is_empty() {
        identity_agent_id
    } else {
        conf_agent_id
    };
    let running = || match host_running {
        Some(check) => check(),
        None => legacy_host_running(home_dir, host_exe),
    };

    // ── A) 没有家 id ＋ 旧身份还在 ⇒ 换键会在世界里另建一支：**这一步只有 CLI 能做** ──
    if home_id.is_empty() && !stale_agent.is_empty() {
        let running_note = if running() {
            "（而且它这会儿**正跑着** —— 先把它停下来。）"
        } else {
            ""
        };
        return ShapeOutcome::refuse(
            format!(
                "这个家还没有家 id（`home.conf` 里没有 `home_id`），而它世界里那支 Agent 是**旧的\
「世界级键」**建的：换形状会在世界里**另建一支 Agent**（旧那支留着不动）。\n\
这一步要动世界（重放布置键）——**只有 CLI 能做**：`node home.js up`。{running_note}"
            ),
            BLOCKER_WORLD_KEY,
        );
    }

    // ── B) 旧环境不在：也许 `home.conf` 里还留着那两行（上次迁到一半 / 环境已经不在）──
    if !legacy_exists {
        if stale_agent.is_empty() || home_id.is_empty() {
            return ShapeOutcome::none();
        }
        let target = home_facts::agent_env_root(home_dir, &stale_agent);
        if target.exists() {
            write_agent_conf_for(home_dir, &stale_agent, &conf_room_id);
            let moved = migrate_legacy_runtime_files(home_dir, &stale_agent);
            let _ = delete_conf_keys(home_dir);
            lines.push(format!(
                "住户化的收尾：{stale_agent} 的环境已在 `<家>/agents/` 里 ⇒ `home.conf` 那两行\
（`agent_id` / `room_id`）让位给支级 `agent.conf`"
            ));
            if !moved.is_empty() {
                lines.push(format!(
                    "运行账本跟着搬（{}）⇒ `logs/agents/{stale_agent}/`",
                    moved.join("、")
                ));
            }
            return ShapeOutcome {
                ok: true,
                action: ShapeAction::Finish,
                agent_id: Some(stale_agent),
                env_aside: None,
                lines,
                refuse: None,
                blocker: None,
            };
        }
        // 环境与住户目录都不在：那两行只是旧账 —— 删掉（世界侧身份由布置键重放找回）。
        let _ = delete_conf_keys(home_dir);
        lines.push(
            "住户化的收尾：`home.conf` 里还留着旧的 `agent_id` / `room_id`，而环境已经不在 —— \
让位（世界里那支由布置键重放找回）"
                .to_string(),
        );
        return ShapeOutcome {
            ok: true,
            action: ShapeAction::Finish,
            agent_id: Some(stale_agent),
            env_aside: None,
            lines,
            refuse: None,
            blocker: None,
        };
    }

    // ── C) 旧环境在场，而它没有身份可归属（没布置完的空 env）⇒ 什么都不动 ──
    if stale_agent.is_empty() {
        return ShapeOutcome::none();
    }

    // ── D) 有家 id（身份不变）：`<家>/env` 整份迁成住户 1 的环境根 ──
    if running() {
        return ShapeOutcome::refuse(
            "这个家还是**一支一个环境根**的旧形状（环境根 `<家>/env`），要把它迁成\
`<家>/agents/<支>/`。\n但家里那支 Agent **还在跑** —— 搬环境要在它不在跑的时候做：\
先把它停下来（`node home.js down`），下次开机这颗 app 就会搬。"
                .to_string(),
            BLOCKER_HOST_RUNNING,
        );
    }
    let target = home_facts::agent_env_root(home_dir, &stale_agent);
    let mut action = ShapeAction::Migrated;
    let mut env_aside: Option<PathBuf> = None;
    if !target.exists() {
        if let Some(parent) = target.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                return ShapeOutcome::failed(format!(
                    "旧环境迁到住户位失败（建目录 {}：{error}）—— 停在这里，什么都没改（重跑一次即可）",
                    parent.display()
                ));
            }
        }
        if let Err(error) = fs::rename(&legacy_env, &target) {
            return ShapeOutcome::failed(format!(
                "旧环境迁到住户位失败（{} ⇒ {}：{error}）—— 停在这里，什么都没改（重跑一次即可）",
                legacy_env.display(),
                target.display()
            ));
        }
    } else {
        // 目标被占（同一个身份不该出现两次）：旧的那份**整份让位**（不覆盖、不合并）。
        let aside = free_aside_path(&legacy_env, &stale_agent);
        if let Err(error) = fs::rename(&legacy_env, &aside) {
            return ShapeOutcome::failed(format!(
                "旧环境让位失败（{} ⇒ {}：{error}）—— 停在这里，什么都没改（重跑一次即可）",
                legacy_env.display(),
                aside.display()
            ));
        }
        env_aside = Some(aside);
        action = ShapeAction::Aside;
    }
    write_agent_conf_for(home_dir, &stale_agent, &conf_room_id);
    let moved = migrate_legacy_runtime_files(home_dir, &stale_agent);
    let _ = delete_conf_keys(home_dir);
    lines.push(format!(
        "住户化：环境根 {} ⇒ {}（住户 1：{stale_agent}；身份不变，世界里的记录照旧）",
        legacy_env.display(),
        target.display()
    ));
    if let Some(aside) = &env_aside {
        lines.push(format!(
            "⚠ 目标住户位已存在 ⇒ 旧的那份整份让位（不删）：{}",
            aside.display()
        ));
    }
    if !moved.is_empty() {
        lines.push(format!(
            "运行账本跟着搬（{}）⇒ `logs/agents/{stale_agent}/`",
            moved.join("、")
        ));
    }
    ShapeOutcome {
        ok: true,
        action,
        agent_id: Some(stale_agent),
        env_aside,
        lines,
        refuse: None,
        blocker: None,
    }
}

/// **老家那支宿主还在跑吗**（迁移的前置判据）——与 JS 同款两条，**不新造判据**：
/// ① 账本 `logs/host.pid`（`ours` / **核对不了**都算在跑，fail-closed）；
/// ② 进程表里 `--root` 落在 `<家>/env` 上的宿主（账本丢了但进程还在的那种）。
fn legacy_host_running(home_dir: &Path, host_exe: &Path) -> bool {
    let pid_file = home_dir.join("logs").join("host.pid");
    if let Some(pid) = local_residents::ledger_pid(&pid_file) {
        match process_facts::ownership(pid, host_exe) {
            Ownership::Ours { .. } | Ownership::Unknown(_) => return true,
            Ownership::NotOurs { .. } | Ownership::Gone => {}
        }
    }
    !host_pids_with_root(&legacy_env_root(home_dir), host_exe).is_empty()
}

/// 进程表里那些**命令行带着 `--root <那个环境根>`** 的宿主 pid（账本说不了话时的那一条判据）。
///
/// ⚠ 只在**老家形状在场**时才被调到（迁移的前置），所以这点开销落在罕见路径上；
/// 判据与 `proc.js` 的 `hostOrphanPids` 同一条：**种类 ＋ 家事实**，不看 pid 文件。
fn host_pids_with_root(env_root: &Path, host_exe: &Path) -> Vec<u32> {
    let Some(name) = host_exe.file_name().and_then(|each| each.to_str()) else {
        return Vec::new();
    };
    let wanted = env_root.display().to_string();
    let command_lines = host_command_lines(name);
    command_lines
        .into_iter()
        .filter(|line| {
            // `--root <路径>`：按**词**取，不按子串（子串会把 `<家>/env-backup` 也认成老家）。
            flag_value(line, "--root")
                .map(|value| same_path(&value, &wanted))
                .unwrap_or(false)
        })
        .filter_map(|line| pid_of_line(&line))
        .collect()
}

/// 跑着的宿主进程各自的「pid ＋ 命令行」。
///
/// Windows：PowerShell 的 `Get-CimInstance Win32_Process`（**与 `userenv.js` 用 PowerShell
/// 同一个理由**：它是系统自带的、不需要额外二进制）；别的平台：`ps -eo pid=,args=`。
fn host_command_lines(exe_name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if cfg!(windows) {
        let script = format!(
            "Get-CimInstance Win32_Process -Filter \"Name='{exe_name}'\" | \
ForEach-Object {{ \"$($_.ProcessId) $($_.CommandLine)\" }}"
        );
        let mut probe = std::process::Command::new("powershell.exe");
        probe.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        // **不给它控制台窗口**（2026-10-08）：壳没有控制台，而 PowerShell 是控制台程序
        // ⇒ 少了这一行就是"每次探一次进程表闪一下黑框"。
        crate::console_window::suppress(&mut probe);
        let Ok(done) = probe.output() else {
            return out;
        };
        out.extend(
            String::from_utf8_lossy(&done.stdout)
                .lines()
                .map(str::to_string),
        );
        return out;
    }
    let Ok(done) = std::process::Command::new("ps")
        .args(["-eo", "pid=,args="])
        .output()
    else {
        return out;
    };
    out.extend(
        String::from_utf8_lossy(&done.stdout)
            .lines()
            .filter(|line| line.contains(exe_name))
            .map(str::to_string),
    );
    out
}

/// 一行「pid 命令行」里的 pid（读不出来 ⇒ `None`，**不猜**）。
fn pid_of_line(line: &str) -> Option<u32> {
    line.split_whitespace().next()?.parse().ok()
}

/// 命令行里某个旗标的取值（`--root <路径>`）：照 `proc.js` 的 `flagValueFromCommandLine`。
fn flag_value(line: &str, flag: &str) -> Option<String> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    for index in 0..parts.len().saturating_sub(1) {
        if parts[index] == flag {
            return Some(parts[index + 1].trim_matches('"').to_string());
        }
    }
    None
}

/// 两个路径是不是同一个（分隔符与大小写按平台归一；只比字符串，不做 IO）。
fn same_path(left: &str, right: &str) -> bool {
    let norm = |value: &str| {
        let trimmed = value.trim_end_matches(['\\', '/']).replace('/', "\\");
        if cfg!(windows) {
            trimmed.to_ascii_lowercase()
        } else {
            trimmed
        }
    };
    norm(left) == norm(right)
}

/// 旧环境让位时那个空位（`freeAsidePath` 直译：`<路径>.old-<支>`，被占就 `-2`…`-99`）。
fn free_aside_path(legacy_env: &Path, agent_id: &str) -> PathBuf {
    let base = PathBuf::from(format!("{}.old-{agent_id}", legacy_env.display()));
    if !base.exists() {
        return base;
    }
    for n in 2..100 {
        let candidate = PathBuf::from(format!("{}-{n}", base.display()));
        if !candidate.exists() {
            return candidate;
        }
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|each| each.as_millis())
        .unwrap_or(0);
    PathBuf::from(format!("{}-{stamp}", base.display()))
}

/// 支级登记（`agent.conf`）写进**住户位**：`room` 取 `home.conf` 那份、`slot` 沿用已有的（缺省 1）、
/// 显示名原样保留 —— 与 JS 的三元表达式逐格对应。
fn write_agent_conf_for(home_dir: &Path, agent_id: &str, conf_room_id: &str) {
    let env_root = home_facts::agent_env_root(home_dir, agent_id);
    let existing = registration::read_agent_conf(&env_root);
    let room = if conf_room_id.trim().is_empty() {
        existing.room_id.clone()
    } else {
        conf_room_id.to_string()
    };
    let values = registration::AgentConf {
        agent_id: agent_id.to_string(),
        room_id: room,
        slot: existing.slot.or(Some(1)),
        display_name: existing.display_name.clone(),
    };
    // 写失败不拦迁移（环境已经搬到位了；登记缺了下次还会补）—— 但**不吞**：交给日志。
    if let Err(error) = registration::write_agent_conf(&env_root, &values) {
        eprintln!(
            "[home_shape] 支级登记写不进去（{}）：{error}",
            env_root.display()
        );
    }
}

/// 老家的运行账本（`host.pid` / `host.out`）跟着搬到支级账本位；**只在目标缺失时搬**
/// （rename，保留 mtime；目标在场 ⇒ 一个字节都不动）。返回搬了哪几个（文件名）。
fn migrate_legacy_runtime_files(home_dir: &Path, agent_id: &str) -> Vec<String> {
    let moves = [
        (
            home_dir.join("logs").join("host.pid"),
            registration::host_pid_file(home_dir, agent_id),
        ),
        (
            home_dir.join("logs").join("host.out"),
            registration::host_log_file(home_dir, agent_id),
        ),
    ];
    let mut moved: Vec<String> = Vec::new();
    for (from, to) in moves {
        if !from.exists() || to.exists() {
            continue;
        }
        if let Some(parent) = to.parent() {
            if fs::create_dir_all(parent).is_err() {
                continue;
            }
        }
        if fs::rename(&from, &to).is_ok() {
            if let Some(name) = from.file_name().and_then(|each| each.to_str()) {
                moved.push(name.to_string());
            }
        }
    }
    moved
}

/// `home.conf` 里那两行（`agent_id` / `room_id`）让位给支级 `agent.conf`（**只删这两行**）。
fn delete_conf_keys(home_dir: &Path) -> Result<(), String> {
    let path = home_dir.join("home.conf");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let (after_agent, changed_agent) = crate::local_world::conf_delete_text(&existing, "agent_id");
    let (after_room, changed_room) = crate::local_world::conf_delete_text(&after_agent, "room_id");
    if !changed_agent && !changed_room {
        return Ok(());
    }
    fs::write(&path, after_room)
        .map_err(|error| format!("写 home.conf 失败（{}）：{error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "celira-shape-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|each| each.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    /// 造一个**老家形状**的家：`home.conf`（有家 id ＋ 旧两行）＋ `<家>/env/identity.json`。
    fn legacy_home(label: &str, conf_lines: &[&str]) -> PathBuf {
        let home = scratch(label);
        fs::create_dir_all(home.join("env")).unwrap();
        fs::write(home.join("env").join("identity.json"), r#"{"agent":"a-1"}"#).unwrap();
        fs::write(home.join("env").join("notes.md"), "她的笔记").unwrap();
        fs::create_dir_all(home.join("logs")).unwrap();
        let mut conf = String::from("human_id=h-1\n");
        for line in conf_lines {
            conf.push_str(line);
            conf.push('\n');
        }
        fs::write(home.join("home.conf"), conf).unwrap();
        home
    }

    fn never_running() -> bool {
        false
    }

    #[test]
    fn a_new_shaped_home_is_left_alone() {
        let home = scratch("new");
        fs::create_dir_all(home.join("agents").join("a-1")).unwrap();
        fs::write(
            home.join("agents").join("a-1").join("identity.json"),
            r#"{"agent":"a-1"}"#,
        )
        .unwrap();
        fs::write(home.join("home.conf"), "human_id=h-1\nhome_id=H-1\n").unwrap();
        let out = migrate_home_shape(&home, Path::new("host.exe"), Some(&never_running));
        assert_eq!(out.action, ShapeAction::None);
        assert!(out.ok && out.lines.is_empty() && out.refuse.is_none());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_legacy_home_moves_whole_into_the_resident_slot() {
        let home = legacy_home("migrated", &["home_id=H-1", "agent_id=a-1", "room_id=r-1"]);
        fs::write(home.join("logs").join("host.pid"), "4242\n").unwrap();
        fs::write(home.join("logs").join("host.out"), "日志尾巴\n").unwrap();
        let out = migrate_home_shape(&home, Path::new("host.exe"), Some(&never_running));
        assert_eq!(
            out.action,
            ShapeAction::Migrated,
            "{}",
            out.refuse.unwrap_or_default()
        );
        assert_eq!(out.agent_id.as_deref(), Some("a-1"));
        // 环境整份搬过去（**内容一个字节不动**）。
        assert!(!home.join("env").exists(), "旧位置该空出来");
        let env = home.join("agents").join("a-1");
        assert_eq!(
            fs::read_to_string(env.join("notes.md")).unwrap(),
            "她的笔记",
            "环境里的东西照旧"
        );
        // 支级登记写上（room 来自 home.conf，slot 缺省 1）。
        let conf = registration::read_agent_conf(&env);
        assert_eq!(conf.agent_id, "a-1");
        assert_eq!(conf.room_id, "r-1");
        assert_eq!(conf.slot, Some(1));
        // 账本跟着搬 —— ⚠ 落在**支级账本位**（`logs/agents/<支>/`），不是环境根里
        // （`paths.js` 的 `agentPidFile` / `agentHostLog` 就是那儿）。
        let ledger = home.join("logs").join("agents").join("a-1");
        assert!(
            ledger.join("host.pid").exists() && ledger.join("host.out").exists(),
            "账本要搬到 {}",
            ledger.display()
        );
        assert!(!home.join("logs").join("host.pid").exists());
        // home.conf 那两行让位，别的行照旧。
        let text = fs::read_to_string(home.join("home.conf")).unwrap();
        assert!(text.contains("human_id=h-1") && text.contains("home_id=H-1"));
        assert!(!text.contains("agent_id=") && !text.contains("room_id="));
        // 幂等：再跑一次什么都不做。
        let again = migrate_home_shape(&home, Path::new("host.exe"), Some(&never_running));
        assert_eq!(again.action, ShapeAction::None);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_running_legacy_host_stops_the_migration_cold() {
        let home = legacy_home("running", &["home_id=H-1", "agent_id=a-1"]);
        let running = || true;
        let out = migrate_home_shape(&home, Path::new("host.exe"), Some(&running));
        assert_eq!(out.action, ShapeAction::Refuse);
        assert!(!out.ok);
        assert!(out.refuse.unwrap().contains("还在跑"));
        // **一个字都没改**。
        assert!(home.join("env").join("identity.json").exists());
        assert!(!home.join("agents").exists());
        assert!(fs::read_to_string(home.join("home.conf"))
            .unwrap()
            .contains("agent_id=a-1"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn the_oldest_shape_refuses_because_only_the_cli_can_rebuild_the_world_key() {
        // 没有 `home_id`（更老的那一档）：换键会在世界里另建一支 ⇒ 壳停手，指出 CLI 那条真路径。
        let home = legacy_home("oldest", &["agent_id=a-1", "room_id=r-1"]);
        let out = migrate_home_shape(&home, Path::new("host.exe"), Some(&never_running));
        assert_eq!(out.action, ShapeAction::Refuse);
        let why = out.refuse.unwrap();
        assert!(
            why.contains("没有 `home_id`") && why.contains("node home.js up"),
            "{why}"
        );
        assert!(home.join("env").exists(), "一个字都没改");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_taken_slot_makes_the_old_copy_step_aside_instead_of_merging() {
        let home = legacy_home("aside", &["home_id=H-1", "agent_id=a-1"]);
        // 住户位已经有一份（同一个身份出现两次 ⇒ 旧那份整份让位，不覆盖不合并）。
        let taken = home.join("agents").join("a-1");
        fs::create_dir_all(&taken).unwrap();
        fs::write(taken.join("identity.json"), r#"{"agent":"a-1"}"#).unwrap();
        fs::write(taken.join("keep.md"), "住户位那一份").unwrap();
        let out = migrate_home_shape(&home, Path::new("host.exe"), Some(&never_running));
        assert_eq!(
            out.action,
            ShapeAction::Aside,
            "{}",
            out.refuse.unwrap_or_default()
        );
        let aside = out.env_aside.expect("要让位");
        assert!(aside.exists(), "旧那份**不删**，只是让位");
        assert!(aside.display().to_string().contains(".old-a-1"));
        // 住户位那份一个字节没被覆盖。
        assert_eq!(
            fs::read_to_string(taken.join("keep.md")).unwrap(),
            "住户位那一份"
        );
        assert!(!home.join("env").exists());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn the_finish_branch_cleans_the_two_leftover_lines() {
        // 环境已经在住户位，而 home.conf 还留着旧两行 ⇒ 收尾（不搬任何东西）。
        let home = scratch("finish");
        let env = home.join("agents").join("a-1");
        fs::create_dir_all(&env).unwrap();
        fs::write(env.join("identity.json"), r#"{"agent":"a-1"}"#).unwrap();
        fs::write(
            home.join("home.conf"),
            "human_id=h-1\nhome_id=H-1\nagent_id=a-1\nroom_id=r-1\n",
        )
        .unwrap();
        let out = migrate_home_shape(&home, Path::new("host.exe"), Some(&never_running));
        assert_eq!(out.action, ShapeAction::Finish);
        assert_eq!(out.agent_id.as_deref(), Some("a-1"));
        let text = fs::read_to_string(home.join("home.conf")).unwrap();
        assert!(
            !text.contains("agent_id=")
                && !text.contains("room_id=")
                && text.contains("home_id=H-1")
        );
        // 支级登记补上（room 从 home.conf 带过来）。
        assert_eq!(registration::read_agent_conf(&env).room_id, "r-1");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn an_empty_legacy_env_is_not_a_home_to_migrate() {
        // `<家>/env` 在场但没有身份（没布置完）⇒ 什么都不动（不猜它属于谁）。
        let home = scratch("empty-env");
        fs::create_dir_all(home.join("env")).unwrap();
        fs::write(home.join("home.conf"), "human_id=h-1\nhome_id=H-1\n").unwrap();
        let out = migrate_home_shape(&home, Path::new("host.exe"), Some(&never_running));
        assert_eq!(out.action, ShapeAction::None);
        assert!(home.join("env").exists());
        assert!(!home.join("agents").exists());
        let _ = fs::remove_dir_all(&home);
    }

    /// 每种拒绝都带**自己的稳定原因码**（页面靠它选人话）；成功那几种不带码；
    /// `remember` / `last_blocker` 记下、也能**清掉**（新形状的家不许留着上一次的旧码）。
    #[test]
    fn every_refusal_carries_its_own_stable_code() {
        let oldest = legacy_home("codes-oldest", &["agent_id=a-1"]);
        let out = migrate_home_shape(&oldest, Path::new("host.exe"), Some(&never_running));
        assert_eq!(out.blocker, Some(BLOCKER_WORLD_KEY));
        assert_eq!(out.action, ShapeAction::Refuse);

        let running_home = legacy_home("codes-running", &["home_id=H-1", "agent_id=a-1"]);
        let running = || true;
        let out = migrate_home_shape(&running_home, Path::new("host.exe"), Some(&running));
        assert_eq!(out.blocker, Some(BLOCKER_HOST_RUNNING));

        let ok_home = legacy_home("codes-ok", &["home_id=H-1", "agent_id=a-1"]);
        let out = migrate_home_shape(&ok_home, Path::new("host.exe"), Some(&never_running));
        assert_eq!(out.action, ShapeAction::Migrated);
        assert_eq!(out.blocker, None, "搬成了就不该留原因码");

        remember(&ShapeOutcome::refuse("x".to_string(), BLOCKER_WORLD_KEY));
        assert_eq!(last_blocker(), Some(BLOCKER_WORLD_KEY));
        remember(&ShapeOutcome::none());
        assert_eq!(last_blocker(), None, "没拦路的东西 ⇒ 清掉上一次的旧码");

        let _ = fs::remove_dir_all(&oldest);
        let _ = fs::remove_dir_all(&running_home);
        let _ = fs::remove_dir_all(&ok_home);
    }

    #[test]
    fn a_pid_line_is_read_without_guessing() {
        assert_eq!(
            pid_of_line("4242 C:\\x\\agent_host.exe --root D:\\h\\env"),
            Some(4242)
        );
        assert_eq!(pid_of_line("不是 pid 的行"), None);
        assert_eq!(
            flag_value("a --root D:\\h\\env --agent a-1", "--root"),
            Some("D:\\h\\env".to_string())
        );
        assert_eq!(
            flag_value("a --root", "--root"),
            None,
            "旗标后面没值 ⇒ 没有（不猜）"
        );
        assert!(
            same_path("D:\\h\\env\\", "d:/h/env"),
            "分隔符与大小写按平台归一"
        );
        assert!(
            !same_path("D:\\h\\env", "D:\\h\\env-backup"),
            "子串不算同一个"
        );
    }
}
