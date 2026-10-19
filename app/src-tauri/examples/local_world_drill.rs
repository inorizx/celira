//! **手动触发一次「本机世界」**——本刀（完整 app 自带的那一半：起世界 + 首布置 + 会话）
//! 最窄面的命令行把手，供真机读数与手工验收用。
//!
//! ```text
//! # 读数（只读，不动作）
//! cargo run --example local_world_drill -- readout
//! # 确保世界在跑（探活优先；不在才起）
//! cargo run --example local_world_drill -- ensure
//! # 交一支本机会话（需要时先铸首位 Human；token 只打印长度，绝不打印原文）
//! cargo run --example local_world_drill -- session
//! # 给一支**已经安家**的住户补登记（`agent.conf` ＋ 凭据的世界分条；幂等，不碰环境根与进程）
//! cargo run --example local_world_drill -- register <agent-id>
//! ```
//!
//! 退出码（给脚本判读）：
//!
//! | 码 | 结局 |
//! |---|---|
//! | 0 | 成了（读数拿到了 / 世界在跑 / 会话拿到 / 登记已就位） |
//! | 3 | 缺可执行件（这台机器上还没有 world 二进制） |
//! | 4 | 起不来 / 世界没在跑 / 会话失败（原因原样在读数里） |
//! | 20 | 补登记被拒（id 形状 / 没有环境根 / 问不到世界 / 不是你的 / 不是 active） |
//!
//! ⚠ **这是临时入口**（与 `examples/anjia_drill.rs` 同一处置）：只为本刀提供一个能从命令行
//! 触发的把手；前端接上三个 Tauri 命令（`local_world_readout` / `local_world_enable` /
//! `local_world_session`）之后即可删除 —— 删除要在 `app/README.md` 与竣工报告里登记。
//!
//! ⚠ **它不打印任何凭据**：会话原文只报长度与它代表谁（凭据纪律：绝不进日志或输出）。
//! ⚠ 它**不动 `home.conf` 的其它键**、不建 Agent、不做托盘与退出语义。

use celira_app_lib::local_world::{
    ensure_world, human_id_of, local_session, readout, world_state, EnsureOutcome, LocalFacts,
    SessionOutcome,
};

fn main() {
    let command = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "readout".to_string());
    let facts = LocalFacts::load(None, None);

    match command.as_str() {
        "readout" => {
            let view = readout(&facts, world_state(&facts), None);
            println!("[本机世界] 家 = {}", view.home_dir);
            println!("[本机世界] 世界 = {}（state={}）", view.address, view.state);
            println!("[本机世界] 可执行件 = {}", view.im_exe);
            println!(
                "[本机世界] 家的门 = {}（状态口）/ {}（模型配置门）",
                view.status_base, view.config_base
            );
            println!(
                "[本机世界] 主人 = {}",
                view.human_id
                    .as_deref()
                    .unwrap_or("（还没有——首布置会铸一位）")
            );
            match view.state.as_str() {
                "running" => std::process::exit(0),
                "no-core" => std::process::exit(3),
                _ => std::process::exit(4),
            }
        }
        "ensure" => {
            let outcome = ensure_world(&facts);
            let code = match &outcome {
                EnsureOutcome::AlreadyRunning => {
                    println!("[本机世界] 已经在跑（不是这次起的——同一个家只该有一个世界）");
                    0
                }
                EnsureOutcome::Started { pid } => {
                    println!(
                        "[本机世界] 起来了：pid {pid}，日志 {}",
                        facts.im_log_file().display()
                    );
                    0
                }
                EnsureOutcome::NoCore => {
                    println!("[本机世界] 缺可执行件：{}", facts.layout.im_exe.display());
                    3
                }
                EnsureOutcome::Failed { detail } => {
                    println!("[本机世界] 起不来：{detail}");
                    4
                }
            };
            std::process::exit(code);
        }
        "session" => {
            match local_session(&facts) {
                SessionOutcome::Ok {
                    human_id,
                    token,
                    minted_human,
                } => {
                    // 报一句"代表谁（按名字）"：显示名是**公共呈现**，而首次布置那条规矩
                    // （取机器用户名，不是关系词）要在这里看得见。读不到就如实说读不到。
                    let named = celira_app_lib::world_channel::WorldHttp::me_display_name(
                        &facts.world_base(),
                        &token,
                    );
                    println!(
                        "[本机世界] 会话拿到：代表 {}（{human_id}），长度 {} 字节（**原文不打印**）",
                        named.as_deref().unwrap_or("（读不到显示名）"),
                        token.len()
                    );
                    println!(
                        "[本机世界] 首次布置：{}",
                        if minted_human {
                            "这一次刚铸的首位 Human"
                        } else {
                            "既有主人，补发一支"
                        }
                    );
                    println!("[本机世界] 世界 = {}", facts.world_base());
                    std::process::exit(0);
                }
                SessionOutcome::WorldDown => {
                    println!("[本机世界] 世界没在跑（先 ensure）——什么都没动");
                    std::process::exit(4);
                }
                SessionOutcome::NoCore => {
                    println!("[本机世界] 缺可执行件：{}", facts.layout.im_exe.display());
                    std::process::exit(3);
                }
                SessionOutcome::Failed { detail } => {
                    println!(
                        "[本机世界] 会话失败：{detail}（主人 = {}）",
                        human_id_of(&facts).unwrap_or_else(|| "（读不到）".to_string())
                    );
                    std::process::exit(4);
                }
            }
        }
        "register" => {
            let agent_id = std::env::args().nth(2).unwrap_or_default();
            if agent_id.trim().is_empty() {
                eprintln!("用法：cargo run --example local_world_drill -- register <agent-id>");
                std::process::exit(2);
            }
            let report = celira_app_lib::anjia::ensure_registration(&agent_id, None, None);
            for line in &report.lines {
                println!("{line}");
            }
            println!("[登记] 结局：{}", report.outcome.kind());
            std::process::exit(match report.outcome {
                celira_app_lib::anjia::RegisterOutcome::Registered { .. } => 0,
                celira_app_lib::anjia::RegisterOutcome::Failed => 4,
                _ => 20,
            });
        }
        "residents" => {
            // 「打开 app ⇒ 它就在」那条路的手动把手：报现状 → 确保都在跑 → 贴日志尾。
            let list = celira_app_lib::local_residents::residents(&facts.home_dir);
            if list.is_empty() {
                println!("[住户] 这个家还没有住户（安家那只手还没安过谁）");
                std::process::exit(0);
            }
            println!("[住户] 这个家住着 {} 支：", list.len());
            for resident in &list {
                let state =
                    celira_app_lib::local_residents::state_of(resident, &facts.layout.host_exe);
                let pid = celira_app_lib::local_residents::ledger_pid(&resident.pid_file);
                println!(
                    "[住户]   {}（{}）：{}（账本 {}）",
                    resident.label(),
                    resident.agent_id,
                    state.kind(),
                    pid.map(|p| p.to_string())
                        .unwrap_or_else(|| "无".to_string())
                );
            }
            let token = match local_session(&facts) {
                SessionOutcome::Ok { token, .. } => Some(token),
                _ => {
                    println!(
                        "[住户] ⚠ 拿不到主人会话 —— 「世界说它在不在线」那一问跳过（账本仍然照判）"
                    );
                    None
                }
            };
            let reports =
                celira_app_lib::local_residents::ensure_residents(&facts, token.as_deref());
            let mut started = 0;
            for report in &reports {
                let Some(outcome) = &report.outcome else {
                    continue;
                };
                let detail = match outcome {
                    celira_app_lib::local_residents::StartOutcome::Started { pid } => {
                        started += 1;
                        format!("pid {pid}（账本已记）")
                    }
                    celira_app_lib::local_residents::StartOutcome::AlreadyRunning { pid } => {
                        format!("pid {pid}")
                    }
                    celira_app_lib::local_residents::StartOutcome::RefusedNoCore { exe } => {
                        format!("缺 {}", exe.display())
                    }
                    celira_app_lib::local_residents::StartOutcome::RefusedMissingModel => {
                        "模型那一半没人在".to_string()
                    }
                    celira_app_lib::local_residents::StartOutcome::RefusedMissingCredential {
                        detail,
                    } => detail.clone(),
                    celira_app_lib::local_residents::StartOutcome::Failed { detail } => {
                        detail.clone()
                    }
                };
                println!("[住户]   {}：{} —— {detail}", report.label, outcome.kind());
            }
            // 日志尾（报到那行在不在，看得见）。
            for resident in &list {
                if !resident.log_file.is_file() {
                    continue;
                }
                let tail = celira_app_lib::anjia::host_process::log_tail(&resident.log_file, 3);
                if tail.is_empty() {
                    continue;
                }
                println!("[住户]   {} 的日志尾：", resident.label());
                for line in tail {
                    println!("           {line}");
                }
            }
            let _ = started;
            std::process::exit(0);
        }
        "stop" => {
            // 「退出（停止本机服务）」那条路的手动把手：**先验身份再杀**（§8.2 + proc.js 纪律）。
            let lines = celira_app_lib::local_services::stop_all(&facts);
            for line in &lines {
                println!("[本机服务] {line}");
            }
            println!(
                "[本机服务] 收场：{}",
                celira_app_lib::local_services::summary_line(&lines)
            );
            let snapshot = celira_app_lib::local_services::snapshot(&facts);
            println!("[本机服务] 现在的样子：{}", snapshot.summary());
            std::process::exit(0);
        }
        other => {
            eprintln!(
                "不认识的子命令：{other}（用法：readout | ensure | session | residents | register <agent-id> | stop）"
            );
            std::process::exit(2);
        }
    }
}
