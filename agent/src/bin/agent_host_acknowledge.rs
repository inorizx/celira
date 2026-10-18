//! 宿主命令：**显式确认**（并清除）上一次保存失败留下的暂停标记。
//!
//! 这条命令把一件**既有**动作接上了入口：`store::clear_paused` 一直是公开 API（它的文档写着
//! "只有**显式**排除损坏并确认可用之后才允许调用"），但此前**没有任何命令**能调用它——
//! `agent_host_status` 的 `[next]` 于是让用户去做一件产品里做不到的事。本命令就是那条路。
//!
//! 它是**宿主的一次行政动作**，不是主体的决策空间：主体不能清自己的暂停（那是存储完整性的异常）。
//! 因此它的形状是"两步"：**先看见原因**，再 `--confirm` 确认。
//!
//! **它明确不做**：不修存储、不继续/重放/补偿任何活动、不删除任何事实（旧标记是真实发生过的事实）、
//! 不接着跑任何旧活动、不碰资料根、不动身份。
//!
//! 退出码是**读数语义**（与 `agent_host_status` 同一条纪律）：
//! `0` 已确认且确认之后准入 `ready` ／ `1` 参数错 ／ `2` 环境打不开（零写入）／
//! `3` 需要用户动作（没给 `--confirm`：零写入；或已确认但准入仍不可用）／
//! `4` 没有记录在案的暂停可确认（零写入）。

use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;

use celira_agent::foundation::{AgentId, RunId};
use celira_agent::manual_entry::escape_field;
use celira_agent::store::{self, Admission, Environment};

const USAGE: &str = "\
用法:
  agent_host_acknowledge <环境根> --agent <AgentId> --run <RunId> [--confirm]
  agent_host_acknowledge --help

说明:
  它只做一件事：**显式确认**上一次保存失败留下的暂停标记（并写下这次确认）。
  不给 --confirm 就是**只读预览**：把盘上记录的原因打出来，一个字节都不写。
  给了 --confirm 才真的确认——写下的是一条 `acknowledged` 环境级标记（**不删除**旧标记：
  旧标记是真实发生过的事实）。

  为什么要两步：这条命令解开的是**一道保护**，所以宿主必须先看见
   「要确认的到底是哪一条异常」，再决定。确认的范围是**此前**的一切记录：
   **未收尾的活动本来就不阻止新活动**（2026-09-26 起），确认解开的是**暂停**；
   「上一段没有收尾」由新的一段的开场那一拍带给主体，本命令不替它接着跑。

  它不修存储、不继续/重放任何活动、不删除任何事实、不接着跑任何旧活动、不碰资料根、不动身份。
  暂停是**环境级**异常，清它是**宿主**的行政动作，因此它不在 Agent 的工具面里。
";

/// 这次确认记录挂靠的活动名（`--run`）。
///
/// 环境级标记需要一个活动的名字来承载；它**不要求**那个活动真的存在——`save-failed:run-start`
/// 那一次可能一个活动记录都没有（失败发生在登记之前），要求存在会让命令正好在最需要它的时候
/// 用不了。是什么就写什么，命令行照实回显。
struct Options {
    root: String,
    agent: String,
    run: String,
    confirm: bool,
}

fn parse(args: &[String]) -> Result<Option<Options>, String> {
    let mut root: Option<String> = None;
    let mut agent: Option<String> = None;
    let mut run: Option<String> = None;
    let mut confirm = false;

    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_str();
        match argument {
            "--help" | "-h" => return Ok(None),
            "--confirm" => confirm = true,
            "--agent" | "--run" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| format!("{argument} 后面要给一个值"))?
                    .clone();
                if value.is_empty() {
                    return Err(format!("{argument} 的值不能为空"));
                }
                if argument == "--agent" {
                    agent = Some(value);
                } else {
                    run = Some(value);
                }
            }
            other if other.starts_with("--") => {
                return Err(format!("未知参数: {other}"));
            }
            other => {
                if root.is_some() {
                    return Err(format!("环境根只能给一个: {other}"));
                }
                root = Some(other.to_string());
            }
        }
        index += 1;
    }

    Ok(Some(Options {
        root: root.ok_or_else(|| "缺环境根".to_string())?,
        agent: agent.ok_or_else(|| "缺 --agent".to_string())?,
        run: run.ok_or_else(|| "缺 --run（环境级标记要挂在哪个活动名下）".to_string())?,
        confirm,
    }))
}

/// 准入的稳定行式（与 `agent_host_status` 同一个词表：`ready` / `paused(reason=…)` / `unfinished(run=…)`）。
fn admission_field(admission: &Admission) -> String {
    match admission {
        Admission::Ready => "ready".to_string(),
        Admission::Paused { reason } => format!("paused(reason={})", escape_field(reason)),
        Admission::Unfinished { run } => format!("unfinished(run={})", escape_field(run.as_str())),
    }
}

/// `[next]`：**恰好一行**，而且**只指向真能走的下一步**。
///
/// 它**不**说"环境已恢复"——确认只表示"这条异常已经被看过"，不代表任何东西被修好；
/// 因此确认成功之后指向的是"看清那些仍未收尾的活动"（`agent_host_status`），而不是"完事了"。
///
/// ⚠ **2026-09-26 改**：这里原来还有半句"再决定用 `agent_run resume` 继续哪一个"。跨进程恢复
/// 已退役（承载活动的进程没了就是两个 Run），于是那半句没有了——**旧痕迹仍然不会被解决**，
/// 但它也**不再拦任何东西**；"上一段没有收尾"由新的一段的开场那一轮带给主体。
fn next_line(confirm: bool, after: &Admission, root: &str, agent: &str) -> String {
    if !confirm {
        return "看清上面记录的原因，确认要确认的就是这一条，然后加 `--confirm` 重跑（本次零写入）"
            .to_string();
    }
    match after {
        Admission::Ready => format!(
            "准入已放行；此前的未收尾痕迹没有被解决——用 `agent_host_status {} --agent {}` 看清它们各自的状态（它们不拦新活动，但事实一条不少）",
            escape_field(root),
            escape_field(agent)
        ),
        other => format!(
            "确认之后准入仍不可用（{}）：用 `agent_host_status {} --agent {}` 看清还剩下什么在拦",
            admission_field(other),
            escape_field(root),
            escape_field(agent)
        ),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse(&args) {
        Ok(Some(options)) => options,
        // 帮助在任何路径之前：只打印用法就返回——不打开环境、不读任何文件、**一个字节都不写**
        // （退出码与 `agent_host_status` 同一约定：帮助是 0，用法错才是 1）。
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("agent_host_acknowledge: {message}\n");
            eprint!("{USAGE}");
            return ExitCode::from(1);
        }
    };

    let mut out = String::new();
    let _ = writeln!(out, "celira-agent-host-acknowledge v1");
    let _ = writeln!(out, "environment-root={}", escape_field(&options.root));
    let _ = writeln!(out, "agent={}", escape_field(&options.agent));
    let _ = writeln!(out, "run={}", escape_field(&options.run));

    // 环境打不开：一条结论，零写入。
    let mut environment = match Environment::open(
        Path::new(&options.root),
        AgentId::new(options.agent.clone()),
    ) {
        Ok(environment) => environment,
        Err(error) => {
            print!("{out}");
            println!();
            println!("[state]\nadmission-after=unknown(error=environment-unreadable)");
            println!();
            println!(
                "[next]\n环境打不开（{}）：先 `environment_diagnose --root {} --agent {}` 看清原因；本命令零写入",
                escape_field(error.as_str()),
                escape_field(&options.root),
                escape_field(&options.agent)
            );
            return ExitCode::from(2);
        }
    };

    // **证据面**：确认之前先把盘上记录的原因打出来（它先于任何写入）。
    let recorded = store::paused_reason(Path::new(&options.root)).unwrap_or(None);
    let _ = writeln!(out, "\n[recorded]");
    let _ = writeln!(
        out,
        "paused-reason={}",
        recorded
            .as_deref()
            .map(escape_field)
            .unwrap_or_else(|| "none".to_string())
    );

    let Some(reason) = recorded else {
        // 没有记录在案的暂停：**不制造"我确认过了"的假记录**，一个字节都不写。
        print!("{out}");
        println!();
        println!(
            "[state]\nadmission-after={}",
            admission_field_or_unknown(&environment)
        );
        println!();
        println!(
            "[next]\n没有记录在案的暂停可确认：这次零写入。要看清现在什么状态用 `agent_host_status {} --agent {}`",
            escape_field(&options.root),
            escape_field(&options.agent)
        );
        return ExitCode::from(4);
    };
    let _ = reason;

    if !options.confirm {
        print!("{out}");
        println!();
        println!("[preview]");
        println!("scope=everything-recorded-before-this-point");
        println!("would-write=one-acknowledged-marker");
        println!("note=本次零写入（没给 --confirm）：先把上面那条原因看清，再决定");
        println!();
        println!(
            "[state]\nadmission-after={}",
            admission_field_or_unknown(&environment)
        );
        println!();
        println!(
            "[next]\n{}",
            next_line(false, &Admission::Ready, &options.root, &options.agent)
        );
        return ExitCode::from(3);
    }

    // **真正的那一次写入**：一条 `acknowledged` 环境级标记。旧标记不删（它是真发生过的事实）。
    //
    // ⚠ **2026-09-26 改**：这条标记的取值原来叫 `resumed`——那是"接着跑同一段活动"的说法，
    // 而 `resume` 那条路已退役；它真正说的是"**这条异常已经被看过了**"，因此改名 `acknowledged`。
    let acknowledged_seq = match store::clear_paused(
        &mut environment,
        &RunId::new(options.run.clone()),
    ) {
        Ok(seq) => seq,
        Err(error) => {
            print!("{out}");
            println!();
            println!("[state]\nadmission-after=unknown(error=write-failed)");
            println!();
            println!(
                "[next]\n写不下这条确认（{}）：存储可能还有问题，先 `environment_diagnose --root {} --agent {}`；没有确认成功",
                escape_field(error.as_str()),
                escape_field(&options.root),
                escape_field(&options.agent)
            );
            return ExitCode::from(2);
        }
    };

    let after = environment.admission();
    let _ = writeln!(out, "\n[acknowledged]");
    let _ = writeln!(out, "acknowledged-seq={acknowledged_seq}");
    let _ = writeln!(out, "scope=everything-recorded-before-this-point");
    let _ = writeln!(
        out,
        "note=确认的是「这条异常已经被看过」，不是「环境已修好」：不修复、不继续、不重放、不删除任何事实"
    );
    let _ = writeln!(
        out,
        "note=确认范围含此前的未收尾痕迹——它们不再阻止新活动，但并没有被解决"
    );
    print!("{out}");

    let after = match after {
        Ok(admission) => admission,
        Err(_) => {
            println!();
            println!("[state]\nadmission-after=unknown(error=environment-unreadable)");
            println!();
            println!(
                "[next]\n确认已经写下，但确认之后的准入读不出来：用 `environment_diagnose --root {} --agent {}` 再看一眼",
                escape_field(&options.root),
                escape_field(&options.agent)
            );
            return ExitCode::from(3);
        }
    };

    println!();
    println!("[state]\nadmission-after={}", admission_field(&after));
    println!();
    println!(
        "[next]\n{}",
        next_line(true, &after, &options.root, &options.agent)
    );

    match after {
        Admission::Ready => ExitCode::SUCCESS,
        _ => ExitCode::from(3),
    }
}

/// 预览 / 无事可确认两处的准入读数：读不出来时**不猜**（写成 `unknown(error=…)`）。
fn admission_field_or_unknown(environment: &Environment) -> String {
    match environment.admission() {
        Ok(admission) => admission_field(&admission),
        Err(error) => format!("unknown(error={})", escape_field(error.as_str())),
    }
}
