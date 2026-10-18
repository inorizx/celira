//! 离线诊断命令：把一个身份环境里**已确认的持久事实**重读成"操作到了哪一步"。
//!
//! 它调用的是和宿主同一个生产诊断函数（`celira_agent::diagnostics`），不是测试专用副本：
//! 打开方式、解释方式、分页与错误分类都只有一处实现。
//!
//! **只读**：使用既有的 `Environment::open`（**不** create、不新增旁路打开），不追加任何记录，
//! 不解除暂停，不修复、不截断、不迁移、不重放，也不调用模型或任何行动。输出只写到 stdout，
//! 由调用方决定是否保存；诊断自己**不创建**任何报告文件。
//!
//! 用法：
//!
//! ```text
//! environment_diagnose <环境根> <预期 AgentId> [--run <活动标识>] [--after <活动标识>]
//!                      [--from-seq <序号>] [--limit <条数>]
//! ```
//!
//! - 不给 `--run`：列出有界的一页活动（默认 32 个）；`--after` 是上一页最后一个活动。
//!   报告里那两份附属列表（经历侧活动摘要、可判定的跨存储矛盾）**只覆盖本页活动**，整份
//!   条数写在 `storage-page` 行的 `*-total` 上；把活动分页走完就覆盖完整，不丢项。
//! - 给 `--run`：只诊断这一个活动，并按 `--from-seq`（含）分页返回它的事实行（默认 64 行）。
//! - `--after` 与 `--run` 不能同时给；`--from-seq` 只能与 `--run` 一起给。
//!
//! 退出码：
//!
//! | 码 | 含义 |
//! |---|---|
//! | 0 | 报告已产出，被选活动的事实**全部可解释**（`admission` 是 `ready`/`paused`/`unfinished` 都算） |
//! | 1 | 参数不合法（缺参数、未知选项、非法数值、未知游标语义、分页超限） |
//! | 2 | 环境不可打开或不可读取（身份不匹配、缺存储文件、结构损坏、已有写者） |
//! | 3 | 指定的活动**不在**这个环境里（未知活动 / 未知游标） |
//! | 4 | 报告已产出，但被选活动里至少有一个的事实**无法解释**（重建被拒或序列被拒）：列活动时指本页任何一个，`--run` 时指那一个 |
//!
//! 退出码是**分开的结论**，不是"成功/失败"：`2` 是"读不出来"，`3` 是"要问的东西不在这里"，
//! `4` 是"读出来了，但有东西解释不了"。三者都不能冒充"健康"或"空环境"。

use std::path::Path;

use celira_agent::diagnostics::{
    ACTIVITY_PAGE_LIMIT, DiagnosticError, FACT_PAGE_LIMIT, activity_is_unexplained,
    diagnose_activity, diagnose_environment, has_unexplained_activity, render_activity,
    render_environment,
};
use celira_agent::foundation::{AgentId, RunId};
use celira_agent::store::EnvironmentReader;

const USAGE: &str = "用法: environment_diagnose <环境根> <预期 AgentId> [--run <活动标识>] \
                     [--after <活动标识>] [--from-seq <序号>] [--limit <条数>]";

fn main() {
    std::process::exit(run(std::env::args().skip(1).collect()));
}

fn run(arguments: Vec<String>) -> i32 {
    let usage = match parse(arguments) {
        Ok(usage) => usage,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("{USAGE}");
            return 1;
        }
    };

    // **读者打开**（2026-10-06）：只读句柄，**不取写资格**——主体正在跑一次活动时这条命令照样读得到
    // （从前走 `Environment::open`，于是"看一眼处境"在最需要看处境的那一刻回 `already-open`）。
    // 只打开，不创建；读的是**已确认前缀**（写者此刻写到一半的那一行不算事实）。
    let environment =
        match EnvironmentReader::open(Path::new(&usage.root), AgentId::new(usage.agent.as_str())) {
            Ok(environment) => environment,
            Err(error) => {
                eprintln!("环境不可打开或不可读取: {}", error.as_str());
                return 2;
            }
        };

    match usage.mode {
        Mode::Activities { after, limit } => {
            let report = match diagnose_environment(&environment, after.as_ref(), limit) {
                Ok(report) => report,
                Err(error) => return report_failure(error),
            };
            print!("{}", render_environment(&report));
            if has_unexplained_activity(&report) {
                return 4;
            }
            0
        }
        Mode::Activity {
            run,
            from_seq,
            limit,
        } => {
            let detail = match diagnose_activity(&environment, &run, from_seq, limit) {
                Ok(detail) => detail,
                Err(error) => return report_failure(error),
            };
            print!("{}", render_activity(&detail));
            // 单个活动与整页必须用**同一个**判据：解释不了就是 4，不因为查询方式不同而报 0。
            if activity_is_unexplained(&detail.diagnosis) {
                return 4;
            }
            0
        }
    }
}

/// 诊断函数自己报的失败 → 退出码。`2`/`3` 与 `1` 分开，是因为"读不出来"、"要问的东西不在这里"
/// 与"参数写错了"是三件不同的事。
fn report_failure(error: DiagnosticError) -> i32 {
    match &error {
        DiagnosticError::InvalidLimit { limit, max } => {
            eprintln!("分页参数超限: limit={limit} max={max}");
            1
        }
        DiagnosticError::UnknownActivity(run) => {
            eprintln!("这个环境里没有活动: {}", run.as_str());
            3
        }
        DiagnosticError::UnknownCursor(run) => {
            eprintln!("未知的分页游标活动: {}", run.as_str());
            3
        }
        DiagnosticError::Storage(error) => {
            eprintln!("环境不可读取: {}", error.as_str());
            2
        }
    }
}

/// 命令行参数解析结果。
struct Usage {
    root: String,
    agent: String,
    mode: Mode,
}

enum Mode {
    Activities {
        after: Option<RunId>,
        limit: usize,
    },
    Activity {
        run: RunId,
        from_seq: Option<u64>,
        limit: usize,
    },
}

/// 解析参数。任何不合法组合都在这里明确失败，不用默认值兜住。
fn parse(arguments: Vec<String>) -> Result<Usage, String> {
    let mut positional: Vec<String> = Vec::new();
    let mut run: Option<String> = None;
    let mut after: Option<String> = None;
    let mut from_seq: Option<String> = None;
    let mut limit: Option<String> = None;

    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        match argument {
            "--run" | "--after" | "--from-seq" | "--limit" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or_else(|| format!("{argument} 缺少取值"))?
                    .clone();
                index += 2;
                match argument {
                    "--run" => run = Some(value),
                    "--after" => after = Some(value),
                    "--from-seq" => from_seq = Some(value),
                    _ => limit = Some(value),
                }
            }
            other if other.starts_with("--") => return Err(format!("未知选项: {other}")),
            other => {
                positional.push(other.to_string());
                index += 1;
            }
        }
    }

    if positional.len() != 2 {
        return Err(format!("需要两个位置参数，收到 {} 个", positional.len()));
    }
    let root = positional[0].clone();
    let agent = positional[1].clone();

    if root.is_empty() || agent.is_empty() {
        return Err("环境根与预期 AgentId 都不能为空".to_string());
    }
    if run.is_some() && after.is_some() {
        return Err("--run 与 --after 不能同时给".to_string());
    }
    if from_seq.is_some() && run.is_none() {
        return Err("--from-seq 只能与 --run 一起给".to_string());
    }

    let parsed_limit = match &limit {
        Some(text) => Some(
            text.parse::<usize>()
                .map_err(|_| format!("--limit 不是数字: {text}"))?,
        ),
        None => None,
    };

    match run {
        Some(text) => {
            if text.is_empty() {
                return Err("--run 不能为空".to_string());
            }
            let from_seq = match &from_seq {
                Some(text) => Some(
                    text.parse::<u64>()
                        .map_err(|_| format!("--from-seq 不是数字: {text}"))?,
                ),
                None => None,
            };
            Ok(Usage {
                root,
                agent,
                mode: Mode::Activity {
                    run: RunId::new(text),
                    from_seq,
                    limit: parsed_limit.unwrap_or(FACT_PAGE_LIMIT),
                },
            })
        }
        None => {
            if let Some(text) = &after
                && text.is_empty()
            {
                return Err("--after 不能为空".to_string());
            }
            Ok(Usage {
                root,
                agent,
                mode: Mode::Activities {
                    after: after.map(RunId::new),
                    limit: parsed_limit.unwrap_or(ACTIVITY_PAGE_LIMIT),
                },
            })
        }
    }
}
