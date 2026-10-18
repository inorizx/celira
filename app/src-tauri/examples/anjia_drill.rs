//! **手动触发一次安家**——本刀（安家·首刀）最窄面的触发入口，供 drill 与手工验收用。
//!
//! ```text
//! # 不带参数：读 CELIRA_HOME_DIR / CELIRA_REPO_DIR（或默认位置）
//! CELIRA_HOME_DIR=/tmp/anjia-drill/home CELIRA_REPO_DIR=/home/ubuntu/celira \
//!   cargo run --example anjia_drill -- <agent-id>
//! ```
//!
//! 退出码（给脚本判读；`kind` 的稳定词表见 `anjia::AnjiaOutcome::kind`）：
//!
//! | 码 | 结局 |
//! |---|---|
//! | 0 | `homed` 成功安家 |
//! | 10 | `already-homed` 已经安家（什么都不动） |
//! | 20 | 拒绝（`refused` / `world-unreadable` / `not-owned` / `not-active`） |
//! | 30 | `failed` 动手之后某一步没成 |
//! | 31 | `started-no-report` 起了但没等到报到 |
//!
//! ⚠ **这是临时入口**：只为本刀「安家执行」提供一个能从命令行触发的把手。
//! 后续段落把它接成 Tauri 命令 + 前端流程之后，这个文件即可删除
//! （在 `app/README.md` §5 与竣工报告里如实登记它存在过）。
//!
//! ⚠ 它**不写任何东西到仓库里**、不碰 `scripts/home/`、不起世界；
//! 它只调 [`celira_app_lib::anjia::run_anjia`]，那是壳里将来前端会走到的那条正路。

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let positional: Vec<&String> = args.iter().filter(|arg| !arg.starts_with("--")).collect();
    if positional.is_empty() || positional.len() > 1 {
        eprintln!("用法：cargo run --example anjia_drill -- <agent-id>（家的位置用 CELIRA_HOME_DIR / CELIRA_REPO_DIR 指）");
        std::process::exit(2);
    }
    let agent_id = positional[0].as_str();

    let request = celira_app_lib::anjia::AnjiaRequest {
        agent_id,
        home_dir: None,
        repo_dir: None,
    };
    let report = celira_app_lib::anjia::run_anjia(&request);
    for line in &report.lines {
        println!("{line}");
    }
    println!(
        "[安家] 结局：{}（kind={}）",
        match report.outcome {
            celira_app_lib::anjia::AnjiaOutcome::Homed => "成功安家",
            celira_app_lib::anjia::AnjiaOutcome::HomedNotStarted => {
                "接上了，但还没选模型 ⇒ 先没起它（不是失败）"
            }
            celira_app_lib::anjia::AnjiaOutcome::AlreadyHomed => "已经安家（什么都没动）",
            celira_app_lib::anjia::AnjiaOutcome::RefusedWorldUnreadable => "拒绝：问不到世界",
            celira_app_lib::anjia::AnjiaOutcome::RefusedNotOwned => "拒绝：不是你的",
            celira_app_lib::anjia::AnjiaOutcome::RefusedNotActive => "拒绝：不是 active",
            celira_app_lib::anjia::AnjiaOutcome::RefusedPrecondition => "拒绝：前置不满足",
            celira_app_lib::anjia::AnjiaOutcome::Failed => "失败",
            celira_app_lib::anjia::AnjiaOutcome::StartedNoReport => "起了但没等到报到",
        },
        report.outcome.kind()
    );
    std::process::exit(match report.outcome {
        celira_app_lib::anjia::AnjiaOutcome::Homed => 0,
        // 接上了、但还没选模型 ⇒ **单独的退出码**（它不是成功也不是失败；脚本要能分开判）。
        celira_app_lib::anjia::AnjiaOutcome::HomedNotStarted => 11,
        celira_app_lib::anjia::AnjiaOutcome::AlreadyHomed => 10,
        celira_app_lib::anjia::AnjiaOutcome::Failed => 30,
        celira_app_lib::anjia::AnjiaOutcome::StartedNoReport => 31,
        _ => 20,
    });
}
