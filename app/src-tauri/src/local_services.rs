//! **本机服务**——「这台机器上跑着什么」与「把它们停下来」。
//!
//! ## 这一片按 §8.2 / §8.3 的既有裁决做（不发明）
//!
//! | 动作 | 结果 |
//! |---|---|
//! | 点窗口的 × | 窗口收起，**app 还在托盘**，本地进程照旧跑 |
//! | 托盘菜单「退出（停止本机服务）」 | 本地进程跟着停（它们的出身就是这 app） |
//!
//! **托盘解决的是"界面关了但 app 不退"，不是"进程活得比 app 久"** ⇒ 不需要守护进程，
//! 也不需要 PID 认领（§8.7）。§8.2 点名"必须一起做"的那一条：**托盘图标要能看出
//! "还有本地服务在跑"** —— 否则用户会以为已经全关了，而东西还在后台写磁盘、占端口。
//!
//! ## 停的纪律
//!
//! - **先验身份再杀**（[`crate::process_facts::terminate`] 内置：只有 `Ownership::Ours` 才动手）
//!   —— 账本会陈旧、pid 会被复用，落在别人身上就是误杀（`proc.js` 纪律第 3 条）；
//! - **谁起的谁停**：世界看 `logs/im.pid`，住户看 `logs/agents/<支>/host.pid`；
//! - **核不上就一个字都不动**，并把原因如实交出去（那是要摆给用户看的读数）。

use std::path::Path;

use crate::local_world::LocalFacts;
use crate::process_facts;

/// 一台机器上"跑着什么"的一次快照（**只读**；托盘图标与读数都用它）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicesSnapshot {
    /// 世界在跑吗（答得上 `GET /health`）。
    pub world_running: bool,
    /// 世界的账本 pid（没有 ⇒ `None`）。
    pub world_pid: Option<u32>,
    /// 这个家住着几支。
    pub residents: usize,
    /// 其中**在跑**的几支（账本 + 身份核对通过）。
    pub residents_running: usize,
    /// **这个家还是住户化之前的形状**（`<家>/env`）——2026-10-05 加。
    ///
    /// ⚠ 判据**不在这一层**（`home_doors::legacy_home_present`，与状态口那一格**同一份**）：
    /// 壳只认新布局，而老家是 `up.js` 搬的（app 不做迁移）⇒ 零住户有两种来路
    /// （真的没有 / 认不出这个形状），这一格就是用来分开它们的 ——
    /// **同一屏两句话不能互相打脸**（上面那张卡说"老形态"，这一格不许说"还没有 Agent"）。
    pub legacy_home: bool,
}

impl ServicesSnapshot {
    /// 「还有本地服务在跑吗」——托盘图标与那句话的判据。
    pub fn any_running(&self) -> bool {
        self.world_running || self.residents_running > 0
    }

    /// 托盘那一句人话（§8.2：让用户看得出还有东西在跑）。
    pub fn summary(&self) -> String {
        if !self.any_running() {
            return "本机服务都停了".to_string();
        }
        let mut parts: Vec<String> = Vec::new();
        if self.world_running {
            parts.push("世界在跑".to_string());
        }
        if self.residents_running > 0 {
            parts.push(format!("{} 支住户在跑", self.residents_running));
        }
        format!("本机服务：{}", parts.join(" · "))
    }
}

/// 读一次快照（**纯只读**：探活 + 账本 + 身份核对；不动作）。
pub fn snapshot(facts: &LocalFacts) -> ServicesSnapshot {
    let world_running = crate::world_channel::WorldHttp::new(&facts.world_base())
        .map(|http| http.health_ok())
        .unwrap_or(false);
    let residents = crate::local_residents::residents(&facts.home_dir);
    let running = residents
        .iter()
        .filter(|resident| {
            matches!(
                crate::local_residents::state_of(resident, &facts.layout.host_exe),
                crate::local_residents::ResidentState::Running { .. }
            )
        })
        .count();
    ServicesSnapshot {
        world_running,
        world_pid: crate::local_residents::ledger_pid(&facts.im_pid_file()),
        residents: residents.len(),
        residents_running: running,
        // 零住户有两种来路：真的没有 / 认不出这个形状（老家）。判据只有一份（见那一格）。
        legacy_home: crate::home_doors::legacy_home_present(facts),
    }
}

/// 停**这个家的一切**（世界 + 全部住户），每一步都先验身份。
///
/// 返回**逐条如实读数**（谁停了、谁本来就不在、谁核不上身份所以没动）——
/// 调用方把它摆出来或写进日志，**不吞**。
pub fn stop_all(facts: &LocalFacts) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    // 1) 住户先停（它们连的是世界；世界先没了它们会开始报错、日志变吵）。
    for resident in crate::local_residents::residents(&facts.home_dir) {
        lines.push(stop_resident(facts, &resident));
    }
    // 2) 世界。
    lines.push(stop_world(facts));
    lines
}

/// 停一支住户（账本 + 身份核对）。
pub fn stop_resident(facts: &LocalFacts, resident: &crate::local_residents::Resident) -> String {
    let label = resident.label();
    let Some(pid) = crate::local_residents::ledger_pid(&resident.pid_file) else {
        return format!("{label}：账本上没有这支 Agent 的宿主 —— 没动任何东西");
    };
    match process_facts::terminate(pid, &facts.layout.host_exe) {
        Ok(()) => {
            // 停完把账本清掉（否则下一次读数会说"陈旧账本"——那是**上次没停干净**的味道）。
            let _ = std::fs::remove_file(&resident.pid_file);
            format!("{label}：已停（pid {pid}）")
        }
        Err(detail) => format!("{label}：{detail}"),
    }
}

/// 停世界（账本 + 身份核对）。
pub fn stop_world(facts: &LocalFacts) -> String {
    let Some(pid) = crate::local_residents::ledger_pid(&facts.im_pid_file()) else {
        return "世界：账本上没有它 —— 没动任何东西".to_string();
    };
    match process_facts::terminate(pid, &facts.layout.im_exe) {
        Ok(()) => {
            let _ = std::fs::remove_file(facts.im_pid_file());
            format!("世界：已停（pid {pid}）")
        }
        Err(detail) => format!("世界：{detail}"),
    }
}

/// 一句给日志/读数的总结（**只报事实**，不美化）。
pub fn summary_line(lines: &[String]) -> String {
    if lines.is_empty() {
        return "没有本机服务可停".to_string();
    }
    lines.join("；")
}

/// 停掉本机服务并把逐条结果写进日志（**托盘退出与面板「全部停止」都走这里**）。
pub fn stop_local_services_and_report() -> Vec<String> {
    let facts = crate::local_world::LocalFacts::load(None, None);
    let lines = stop_all(&facts);
    for line in &lines {
        log::info!("[本机服务] {line}");
    }
    lines
}

/// 账本里那个 pid 是不是**我们的**（给调用方在动手前自己看一眼时用）。
pub fn ledger_is_ours(pid_file: &Path, expected_exe: &Path) -> bool {
    match crate::local_residents::ledger_pid(pid_file) {
        Some(pid) => matches!(
            process_facts::ownership(pid, expected_exe),
            process_facts::Ownership::Ours { .. }
        ),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// 壳 ↔ 前端（两个命令；面板上的总开关用）
// ---------------------------------------------------------------------------

/// 面板上那一格的形状（读数 + 生效中的退出行为）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServicesReadout {
    /// 世界在跑吗。
    pub world_running: bool,
    /// 这个家住着几支 / 其中几支在跑。
    pub residents: usize,
    pub residents_running: usize,
    /// **这个家还是住户化之前的形状**（见 `ServicesSnapshot::legacy_home`）。
    pub legacy_home: bool,
    /// 一句话（人话归页面；这一句是**读数**，页面可以直接摆）。
    pub summary: String,
    /// 「退出时停不停本机服务」的生效值（没表态 ⇒ `true`，§8.3 的默认）。
    pub quit_stops_services: bool,
    /// 这一格**表过态没有**（`false` ⇒ 生效值是默认值，页面该说清"默认"）。
    pub quit_stops_services_stated: bool,
}

fn services_readout_of(config_dir: &Path) -> ServicesReadout {
    let facts = crate::local_world::LocalFacts::load(None, None);
    let snapshot = snapshot(&facts);
    let settings = crate::machine_settings::read_settings(config_dir);
    ServicesReadout {
        world_running: snapshot.world_running,
        residents: snapshot.residents,
        residents_running: snapshot.residents_running,
        legacy_home: snapshot.legacy_home,
        summary: snapshot.summary(),
        quit_stops_services: settings.quit_stops_services_effective(),
        quit_stops_services_stated: settings.quit_stops_services.is_some(),
    }
}

/// 只读：这台机器上跑着什么 ＋ 退出行为（面板上那一格）。
#[tauri::command]
pub async fn local_services_readout(
    config: tauri::State<'_, crate::MachineConfigDir>,
) -> Result<ServicesReadout, String> {
    let config_dir = config.0.clone();
    tauri::async_runtime::spawn_blocking(move || Ok(services_readout_of(&config_dir)))
        .await
        .map_err(|error| format!("壳内部错误：本机服务读数没能回来（{error}）"))?
}

/// **[全部停止]**（§8.3 的总开关）：停世界 ＋ 停全部住户，逐条如实回报。
///
/// ⚠ 停的是**本机服务**，不是"关掉这个家"：环境根、账本之外的东西一个字都不动
/// （主体的经历、家的数据照旧在）。
#[tauri::command]
pub async fn local_services_stop(
    config: tauri::State<'_, crate::MachineConfigDir>,
) -> Result<ServicesReadout, String> {
    let config_dir = config.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let lines = stop_local_services_and_report();
        let mut readout = services_readout_of(&config_dir);
        // 逐条读数**原样**交给页面（谁停了、谁核不上身份所以没动）——不吞、不美化。
        readout.summary = format!("{}｜{}", summary_line(&lines), readout.summary);
        Ok(readout)
    })
    .await
    .map_err(|error| format!("壳内部错误：停本机服务没能回来（{error}）"))?
}

/// 只读：退出行为（面板上那个下拉）。
#[tauri::command]
pub async fn local_quit_behaviour_readout(
    config: tauri::State<'_, crate::MachineConfigDir>,
) -> Result<ServicesReadout, String> {
    let config_dir = config.0.clone();
    tauri::async_runtime::spawn_blocking(move || Ok(services_readout_of(&config_dir)))
        .await
        .map_err(|error| format!("壳内部错误：退出行为读数没能回来（{error}）"))?
}

/// 写「退出时停不停本机服务」（§8.3：`退出时：[保持运行 ▸]`）。
#[tauri::command]
pub async fn local_quit_behaviour_set(
    config: tauri::State<'_, crate::MachineConfigDir>,
    stops_services: bool,
) -> Result<ServicesReadout, String> {
    let config_dir = config.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::machine_settings::write_quit_stops_services(&config_dir, stops_services)?;
        Ok(services_readout_of(&config_dir))
    })
    .await
    .map_err(|error| format!("壳内部错误：写退出行为没能回来（{error}）"))?
}

// ---------------------------------------------------------------------------
// 用例（纯逻辑：快照的两句话；**不起也不停任何进程**）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_says_what_is_running_and_never_pretends() {
        let nothing = ServicesSnapshot {
            world_running: false,
            world_pid: None,
            residents: 2,
            residents_running: 0,
            legacy_home: false,
        };
        assert!(!nothing.any_running());
        assert_eq!(nothing.summary(), "本机服务都停了");
        // 「都停了」说的是**服务**，不是"这个家没有住户"——住户数照旧在快照里。
        assert_eq!(nothing.residents, 2);

        let world_only = ServicesSnapshot {
            world_running: true,
            world_pid: Some(1),
            residents: 1,
            residents_running: 0,
            legacy_home: false,
        };
        assert!(world_only.any_running());
        assert_eq!(world_only.summary(), "本机服务：世界在跑");

        let both = ServicesSnapshot {
            world_running: true,
            world_pid: Some(1),
            residents: 1,
            residents_running: 1,
            legacy_home: false,
        };
        assert_eq!(both.summary(), "本机服务：世界在跑 · 1 支住户在跑");

        let resident_only = ServicesSnapshot {
            world_running: false,
            world_pid: None,
            residents: 1,
            residents_running: 1,
            legacy_home: false,
        };
        assert!(resident_only.any_running(), "住户在跑也算有服务在跑");
        assert_eq!(resident_only.summary(), "本机服务：1 支住户在跑");
    }

    #[test]
    fn a_missing_ledger_means_nothing_to_stop() {
        let home = std::env::temp_dir().join(format!("celira-svc-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&home);
        let pid_file = home.join("logs").join("im.pid");
        assert!(!ledger_is_ours(&pid_file, Path::new("/x/celira-im")));
        assert_eq!(summary_line(&[]), "没有本机服务可停");
        assert_eq!(
            summary_line(&["世界：已停（pid 1）".to_string()]),
            "世界：已停（pid 1）"
        );
        let _ = std::fs::remove_dir_all(&home);
    }
}
