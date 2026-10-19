//! 宿主状态命令：把**已确认事实**读成"它是谁 / 现在什么状态 / 我该不该出手"。
//!
//! 它是**呈现层**，不是事实层：一次执行、只读、即退。零模型调用、零凭据依赖、不写任何记录、
//! 不解除暂停、不修复、不迁移、不重放，也不代敲任何命令——`next` 那一行只说**该敲什么**。
//!
//! 用法：
//!
//! ```text
//! agent_host_status <环境根> --agent <AgentId>
//! ```
//!
//! - `--agent` 是**必填**：公开面里没有"从环境根读出声明身份"的只读函数（身份文件的解释者只有
//!   `store` 一处，见模块末尾的说明），因此本命令与 `environment_diagnose` 一样要求显式给出；
//!   给的与盘上不一致时由 `Environment::open` 校验并报"环境打不开"。
//! - 无子命令、无交互、无网络。
//!
//! 退出码（**读数语义**，与 `agent_run` 的纪律同族：码是结论，不是"成功/失败"）：
//!
//! | 码 | 含义 |
//! |---|---|
//! | 0 | 读到状态，且**没有**需要用户动作的东西 |
//! | 1 | 参数不合法（缺参数、未知选项、重复选项、空取值） |
//! | 2 | 环境打不开或不可读（不存在、身份不匹配、结构损坏、已有写者） |
//! | 3 | 需要用户动作：有活动停在半路，或环境处于未收尾 / 已暂停 |
//!
//! 输出三节（行式稳定文本）＋ **一个可选节**（`--actions`，见下）：
//!
//! - `identity`：环境根、身份、活动条数（**本页几条**与**一共几条**分开报；本页取自账本的
//!   **最近**那一头，要翻更早的用 `environment_diagnose --after`）；
//! - `state`：`admission=` 一行 + **每个活动一行**（五类判定 + 该类的读数）；
//! - `next`：**恰好一行**行动建议——判定类目 × 准入 → 文案的纯函数（见 [`next_line`]）。
//!
//! ⚠ **2026-09-26 起 `unfinished` 那一档的 `next` 不再指向"接着跑它"**：跨进程恢复
//! （`agent_run resume`）已退役——承载活动的进程没了就是两个 Run，
//! 那一次连续就断了。新的一段由**开场那一轮**接过"上一段没有正常收场"这句话（`previous-activity`）。
//! 判定类目本身仍是**读数**：它说"这一个活动现在什么样"，不是"你可以拿它怎么办"。
//!
//! **不显示会过期的投影**：轮次余量、剩余预算这类"随时间失真"的读数一条都不出现；只显示
//! 盘上事实可判读的东西。

use std::fmt::Write as _;
use std::path::Path;

use celira_agent::diagnostics::{
    ACTIVITY_PAGE_LIMIT, EnvironmentDiagnosis, diagnose_latest_activities,
};
use celira_agent::foundation::{AgentId, Outcome, RunId, SystemEnding};
use celira_agent::manual_entry::escape_field;
use celira_agent::recovery;
use celira_agent::store::{Admission, EnvironmentReader, TracePayload, outcome_label};

const HEADER: &str = "celira-agent-host-status v1";
const USAGE: &str = "用法: agent_host_status <环境根> --agent <AgentId> [--actions]";

const EXIT_OK: i32 = 0;
const EXIT_USAGE: i32 = 1;
const EXIT_UNREADABLE: i32 = 2;
const EXIT_NEEDS_ACTION: i32 = 3;

fn main() {
    std::process::exit(run(std::env::args().skip(1).collect()));
}

fn run(arguments: Vec<String>) -> i32 {
    // 帮助在任何路径之前：只打印用法就返回，因此不打开环境、不碰凭据、不读任何文件。
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!("{USAGE}");
        return EXIT_OK;
    }

    let invocation = match parse(&arguments) {
        Ok(invocation) => invocation,
        Err(message) => {
            eprintln!("{message}");
            eprintln!();
            eprintln!("{USAGE}");
            return EXIT_USAGE;
        }
    };

    // **读者打开**（2026-10-06）：只读句柄，**不取写资格**——主体正在跑一次活动时，这条命令照样
    // 读得到（从前走 `Environment::open`，于是"看一眼处境"在最需要看处境的那一刻回 `already-open`）。
    // 只打开，不创建；读的是**已确认前缀**（写者此刻写到一半的那一行不算事实）。
    let environment = match EnvironmentReader::open(
        Path::new(&invocation.root),
        AgentId::new(invocation.agent.as_str()),
    ) {
        Ok(environment) => environment,
        Err(error) => {
            eprintln!("环境不可打开或不可读取: {}", error.as_str());
            return EXIT_UNREADABLE;
        }
    };

    let report = match diagnose_latest_activities(&environment, ACTIVITY_PAGE_LIMIT) {
        Ok(report) => report,
        Err(error) => {
            // 环境能打开却读不出活动页：同样是"读不出来"，不降级成"空环境"。
            eprintln!("环境不可读取: {}", error.as_str());
            return EXIT_UNREADABLE;
        }
    };

    // 每个活动一条判定（只读；活动数有界，逐个调用）。
    let mut activities: Vec<ActivityLine> = Vec::new();
    for activity in &report.activities {
        let state = recovery::inspect(&environment, &activity.run);
        // **只在"没走完"时才去读账**（读一份账是线性开销）：正常收场、或从没跑过的活动一次都不读。
        let stopped = match state {
            recovery::RunState::Unfinished => {
                Some(recovery::stopped_in_place(&environment, &activity.run))
            }
            _ => None,
        };
        activities.push(ActivityLine::of(activity.run.clone(), &state, stopped));
    }

    let admission = &report.storage.admission;
    let needs_action = match admission {
        Admission::Ready => false,
        Admission::Paused { .. } | Admission::Unfinished { .. } => true,
    } || activities
        .iter()
        .any(|line| line.category == CategoryLabel::Unfinished);

    // 账**读一次**、两处用（时间锚 ＋ 动作面读数）：同一份事实只有一个读点。
    let trace = match environment.trace() {
        Ok(trace) => trace,
        Err(error) => {
            eprintln!("账本读不出来: {}", error.as_str());
            return EXIT_UNREADABLE;
        }
    };
    let times = ActivityTimes::of(&trace);
    // **动作面读数**（`--actions`）：从这一页活动的账里数出来，零新事实、零新写者、不进主体视野。
    let actions = if invocation.actions {
        Some(ActionTally::read(&trace, &activities))
    } else {
        None
    };

    print!(
        "{}",
        render(
            &invocation,
            admission,
            &activities,
            &report,
            &times,
            actions.as_ref()
        )
    );

    if needs_action {
        EXIT_NEEDS_ACTION
    } else {
        EXIT_OK
    }
}

/// 一个活动在宿主视图里的那一行。
struct ActivityLine {
    run: RunId,
    category: CategoryLabel,
    /// 该类目自带的读数（`reason` / `outcome` / `failure` / 未决操作条数）——**只搬事实**。
    reason: Option<String>,
    outcome: Option<String>,
    failure: Option<String>,
    unresolved: Option<usize>,
    /// ⚠ **只有 `unfinished` 那一档有这一格**（2026-10-13）：这一段**为什么**停在这里。
    ///
    /// 「**主体自己停下的**」与「**没有正常收场**」是两件事，两者都真；只报后者就是拿假话吓人
    /// （2026-10-10 裁决三）。这一格从前只在**给主体的**开场纸条里有（`previous-activity`），
    /// 操作者那一侧看不见 ⇒ 操作者读到的是"它停在半路"，却分不出"它自己停的"与"被杀在半路"。
    stopped: Option<recovery::StoppedInPlace>,
}

/// 判定类目的**封闭标签**：五种判定各一个，取值与 `recovery` 的稳定标签逐字同名。
#[derive(Clone, Copy, PartialEq, Eq)]
enum CategoryLabel {
    Unfinished,
    Ended,
    Blocked,
    Unknown,
    Refused,
}

impl CategoryLabel {
    fn as_str(self) -> &'static str {
        match self {
            CategoryLabel::Unfinished => "unfinished",
            CategoryLabel::Ended => "ended",
            CategoryLabel::Blocked => "blocked",
            CategoryLabel::Unknown => "unknown",
            CategoryLabel::Refused => "refused",
        }
    }
}

impl ActivityLine {
    /// 从一次只读判定里取出这一行要显示的东西。**不判断**、不排序、不解释。
    ///
    /// `stopped` 只有 `unfinished` 那一档用得上（调用方也只在那时去读账——读一份账是线性开销，
    /// 与 [`recovery::stopped_in_place`] 的注释同一条纪律）。
    fn of(
        run: RunId,
        state: &recovery::RunState,
        stopped: Option<recovery::StoppedInPlace>,
    ) -> Self {
        let (category, reason, outcome, failure, unresolved) = match state {
            recovery::RunState::Unfinished => (CategoryLabel::Unfinished, None, None, None, None),
            recovery::RunState::Ended { outcome, .. } => {
                let failure = match outcome {
                    Outcome::System(SystemEnding::ModelFailed(Some(class))) => {
                        Some(class.to_string())
                    }
                    _ => None,
                };
                (
                    CategoryLabel::Ended,
                    None,
                    Some(outcome_label(*outcome)),
                    failure,
                    None,
                )
            }
            recovery::RunState::Blocked { reason } => (
                CategoryLabel::Blocked,
                Some(reason.clone()),
                None,
                None,
                None,
            ),
            recovery::RunState::Unknown { reason, operations } => (
                CategoryLabel::Unknown,
                Some(reason.clone()),
                None,
                None,
                Some(operations.len()),
            ),
            recovery::RunState::Refused { reason } => (
                CategoryLabel::Refused,
                Some(reason.clone()),
                None,
                None,
                None,
            ),
        };
        Self {
            run,
            category,
            reason,
            outcome,
            failure,
            unresolved,
            // 别的类目没有这一格：只有"没走完"的活动才谈得上"它为什么停在这里"。
            stopped: match category {
                CategoryLabel::Unfinished => stopped,
                _ => None,
            },
        }
    }
}

/// 该出手的是哪一个活动：**按列出顺序的第一个"还没有收场"的活动**；全收场就用第一个。
///
/// 规则是机械的，不需要判断：活动顺序来自既有的只读诊断（事实侧追加顺序在前，只在经历侧出现的
/// 接在后面），"没结束的那个就是你该管的"——`ended` 之外的四类都算没结束。
fn attention_of(lines: &[ActivityLine]) -> Option<&ActivityLine> {
    lines
        .iter()
        .find(|line| line.category != CategoryLabel::Ended)
        .or_else(|| lines.first())
}

/// 渲染三节。
fn render(
    invocation: &Invocation,
    admission: &Admission,
    activities: &[ActivityLine],
    report: &EnvironmentDiagnosis,
    times: &ActivityTimes,
    actions: Option<&ActionTally>,
) -> String {
    let mut out = String::new();
    out.push_str(HEADER);
    out.push('\n');
    let _ = writeln!(out, "environment-root={}", escape_field(&invocation.root));
    let _ = writeln!(out, "agent={}", escape_field(invocation.agent.as_str()));
    // **本页几条**与**一共几条**分开报（2026-09-29 真机实测）。
    //
    // 在此之前这里只报 `activities=<本页条数>`：活动数超过一页之后，它看着就像"一共这么多"，
    // 呈现层据此显示「活动次数 32」而真实是 36；而那一页又是**最早**的一页 ⇒ 最近的活动一条都
    // 看不见。两个事实（本页 / 总数）合成一个字段，是这条缺陷的根。
    //
    // ⚠ 本页取自账本的**最近**那一头（`diagnose_latest_activities`）：这条命令是"看一眼处境"，
    // 要看的是最近发生了什么，不是这个 Agent 一生的开头。要逐页翻更早的，用
    // `environment_diagnose --after`（它才是那个有翻页消费者的工具）。
    let _ = writeln!(
        out,
        "activities-page={} of={} scope=latest",
        activities.len(),
        report.storage.activities_total
    );
    // **时间锚**（2026-10-13）：进程在 ≠ 注意力在。这里只给**绝对时刻**（不会过期），
    // "它是不是卡住了"是操作者的判断，不写成读数（仓库那条纪律：不显示会过期的投影）。
    out.push_str(&times.render());
    out.push('\n');
    if report.storage.cross_store_total > 0 {
        // 只报异常：两份存储的进度对不上时给一条读数，正常时一个字都不打。
        let _ = writeln!(
            out,
            "health=cross-store count={}",
            report.storage.cross_store_total
        );
    }

    if let Some(tally) = actions {
        out.push('\n');
        out.push_str(&tally.render());
    }

    out.push('\n');
    out.push_str("[state]\n");
    match admission {
        Admission::Ready => out.push_str("admission=ready\n"),
        Admission::Paused { reason } => {
            let _ = writeln!(out, "admission=paused(reason={})", escape_field(reason));
        }
        Admission::Unfinished { run } => {
            let _ = writeln!(
                out,
                "admission=unfinished(run={})",
                escape_field(run.as_str())
            );
        }
    }
    for line in activities {
        let mut row = format!(
            "run={} state={}",
            escape_field(line.run.as_str()),
            line.category.as_str()
        );
        if let Some(reason) = &line.reason {
            let _ = write!(row, " reason={}", escape_field(reason));
        }
        if let Some(outcome) = &line.outcome {
            let _ = write!(row, " outcome={outcome}");
        }
        if let Some(failure) = &line.failure {
            let _ = write!(row, " failure={}", escape_field(failure));
        }
        if let Some(unresolved) = line.unresolved {
            let _ = write!(row, " unresolved={unresolved}");
        }
        // ⚠ **2026-10-13 加**：这一段**为什么**停在这里。取值就是事实里本来就有的那几个词
        // （`held` / `yielded` / `no-response` / `unreadable`）——操作者从此分得出
        // 「它自己停下的」与「被杀在半路」，而不是只读到一句"停在半路"。
        if let Some(stopped) = &line.stopped {
            let _ = write!(row, " stopped={}", stopped.as_str());
        }
        out.push_str(&row);
        out.push('\n');
    }

    out.push('\n');
    out.push_str("[next]\n");
    out.push_str(&next_line(
        admission,
        attention_of(activities),
        &invocation.root,
        invocation.agent.as_str(),
    ));
    out.push('\n');
    out
}

/// `next` 这一行：**判定类目 × 准入 → 文案的纯函数**。
///
/// 两个输入各说一件事，所以输出是把两句拼起来（准入在前，类目在后）：
///
/// - **准入**决定"能不能开新活动"（`paused` 是环境级的存储故障；`unfinished` 是"上一次没走完"）；
/// - **类目**决定"那个活动本身该怎么处理"。
///
/// 它只把机械事实翻译成一句人话，不做编排判断：文案里出现的命令**只是该敲什么**，本命令不代敲、
/// 不验证、也不承诺它会成功。`--materials` 的取值**不在环境里**（资料根是每次装配时由宿主给出的，
/// 不是持久事实），因此建议里给的是占位符 `<材料根>`——不猜。
///
/// 不可达的组合：`Ready` 与四个"未收场"类目**同时出现**在真实环境里做不到（一个已登记开始、
/// 又没有可靠结束事实的活动，必然让准入变成 `unfinished`）。这几支仍然写全：准入与判定是两个
/// 各自独立的只读读数，这个函数不该靠"它们之间恰好有这个关系"活着。
fn next_line(
    admission: &Admission,
    attention: Option<&ActivityLine>,
    root: &str,
    agent: &str,
) -> String {
    // **已暂停**是环境级结论，它一句话就把该做的事说完了：此时每个活动自己的状态往往正是这条
    // 暂停造成的阻断，再把活动那一句接上去就是把同一件事说两遍。因此这里直接给出环境级建议，
    // 活动各自那一行仍在 `state` 节里照实列出（信息没有少，只是不重复）。
    //
    // TASK_25 起它**指向一条真能走的命令**：`agent_host_acknowledge`（在此之前，这一行让用户去做
    // 一件产品里做不到的事——"显式清除暂停"当时没有入口）。命令名与参数照抄可跑，仍**不代敲**。
    if let Admission::Paused { reason } = admission {
        return format!(
            "环境已暂停（{}）：先 `agent_host_acknowledge {} --agent {} --run <活动> --confirm` 看懂原因再显式确认，才能开始新活动",
            escape_field(reason),
            escape_field(root),
            escape_field(agent)
        );
    }

    // 准入那一句（`Ready` 时不出现——它没有要用户处理的事）。
    let situation = match admission {
        Admission::Ready => None,
        Admission::Paused { .. } => unreachable!("已在上面的早返回里处理"),
        Admission::Unfinished { run } => {
            Some(format!("有未收尾活动（{}）", escape_field(run.as_str())))
        }
    };

    // 类目那一句。
    let advice = match attention {
        None => {
            if situation.is_some() {
                // 暂停标记与"没有留下活动"可以并存：这时环境级那一句已经把事情说完了。
                return situation.unwrap_or_default();
            }
            "没有活动：无需动作，可以开始新活动".to_string()
        }
        Some(line) => match line.category {
            CategoryLabel::Ended => "活动已收场——无需动作，或开始新活动".to_string(),
            // ⚠ **旧规矩 → 新规矩**（2026-09-26）：这里原来指向 `agent_run resume`（"接着跑
            // 同一个 RunId"）。那条路**已退役**：承载活动的进程没了就是两个 Run，
            // 那一次连续就断了。⇒ 现在**不**建议去接着跑它，而是**重启**——新的一段
            // 会在开场带上那张纸条（`previous-activity`），如实告诉主体"上一段没有正常收场、
            // 原因没有被记录下来、要看它走到哪一步去自己家里的 trace.ndjson"。
            CategoryLabel::Unfinished => {
                // ⚠ **2026-10-13 加**：这一句从前只说"停在半路"——那对**两种**情形都成立，于是
                // 操作者分不出「它自己停下的」与「被杀在半路」（后者才是"没有正常收场"）。
                // 前半句一个字没动（既有断言与既有读法照旧），只在后面补上那一格事实。
                let why = match &line.stopped {
                    Some(recovery::StoppedInPlace::Named) => {
                        "；它自己停在这里（账上有收场结论：停下并留在原地）".to_string()
                    }
                    Some(recovery::StoppedInPlace::Silent) => {
                        "；它把控制交回了、没有收场结论（账上最后一条答复是让出）".to_string()
                    }
                    Some(recovery::StoppedInPlace::No) => {
                        "；账上一条答复都没有——崩溃 / 被强杀 / 断电同形，分不出是哪种".to_string()
                    }
                    Some(recovery::StoppedInPlace::Unreadable(why)) => {
                        format!("；它为什么停在这里读不出来（{}）——不猜", escape_field(why))
                    }
                    None => String::new(),
                };
                format!(
                    "活动停在半路——不接着跑它：重启宿主即可，新的一段会在开场如实告诉它「上一段没有正常收场」（`agent_host_status {} --agent {}` 看清它走到哪一步）{why}",
                    escape_field(root),
                    escape_field(agent)
                )
            }
            // ⚠ **旧规矩 → 新规矩**（2026-09-26）：这里原来写的是"继续前写会被拒，复核可解除"
            // ——那是**未决效果门**的指令。门退役了（门是指令，而世界只该给信息），因此这一格
            // 现在只说**事实**：那一次写没有结果，而"要不要去看一眼"由主体自己决定。
            CategoryLabel::Unknown => format!(
                "有未决操作（{} 条）——那一次写没有结果（要看它走到哪一步：它自己家里的 trace.ndjson）",
                line.unresolved.unwrap_or(0)
            ),
            CategoryLabel::Blocked => format!(
                "活动被阻断（{}）——处理原因后重试",
                escape_field(line.reason.as_deref().unwrap_or("unknown"))
            ),
            CategoryLabel::Refused => "状态不可判读——见 environment_diagnose".to_string(),
        },
    };

    match situation {
        None => advice,
        Some(situation) => format!("{situation}：{advice}"),
    }
}

/// 命令行参数解析结果。
struct Invocation {
    root: String,
    agent: String,
    /// `--actions`：多报一节**动作面读数**（从账里派生的计数）。
    ///
    /// ⚠ **为什么是开关而不是默认**：`[state]` 段有真实消费者，而它**认不出的行就整体失败**
    /// （`scripts/home/host_status_parse.js`："宁可整体失败，也不安静跳过"）⇒ 默认输出必须
    /// 逐字节不变。要读这一节的人（操作者 / 飞轮）自己加上它就是。
    actions: bool,
}

/// 解析参数。任何不合法组合都在这里明确失败，不用默认值兜住。
fn parse(arguments: &[String]) -> Result<Invocation, String> {
    let mut root: Option<String> = None;
    let mut agent: Option<String> = None;
    let mut actions = false;

    let mut index = 0;
    while index < arguments.len() {
        let option = arguments[index].as_str();
        match option {
            "--agent" => {
                let value = arguments
                    .get(index + 1)
                    .ok_or_else(|| "--agent 缺少取值".to_string())?
                    .clone();
                if agent.is_some() {
                    return Err("--agent 给了不止一次".to_string());
                }
                agent = Some(value);
                index += 2;
            }
            "--actions" => {
                actions = true;
                index += 1;
            }
            other if other.starts_with("--") => return Err(format!("未知选项: {other}")),
            other => {
                if root.is_some() {
                    return Err(format!("需要恰好一个环境根，多收到: {other}"));
                }
                root = Some(other.to_string());
                index += 1;
            }
        }
    }

    let root = root.ok_or_else(|| "缺少环境根".to_string())?;
    let agent = agent.ok_or_else(|| "缺少 --agent".to_string())?;
    if root.is_empty() {
        return Err("环境根不能为空".to_string());
    }
    if agent.is_empty() {
        return Err("--agent 不能为空".to_string());
    }
    Ok(Invocation {
        root,
        agent,
        actions,
    })
}

/// 动作面的**派生读数**：把这一页活动的账里 `observed` 那几条数出来。
///
/// **零新事实**：每一条都是从账里直接读出来的（动作、结果三分、失败分类词表、第几个动作、
/// 属于哪一轮）。判据是"**错误率本身不是判据**"那一条：这里只报**计数**，不报分数、不做判断
/// ——「要避免这一次错误，需要知道的东西当时在不在它手里」是**判断**，归写代码的人与主人。
///
/// ⚠ **它必须是开关（`--actions`），不能默认出现**：`[state]` 那一段有真实消费者，而
/// `scripts/home/host_status_parse.js` 的纪律是"**宁可整体失败，也不安静跳过**"——它认不出的行
/// 会让整份读数失败 ⇒ **默认输出必须逐字节不变**。用例钉着：默认没有 `[actions]`；带开关时
/// 那一行与分类行逐字节对得上账（`tests/host_status.rs` 的 `the_action_tally_is_opt_in_and_counts_the_ledger`）。
///
/// **怎么读这张表**（判据，不是表的一部分）：按「**要避免这一次错误，需要知道的东西当时在不在
/// 主体手里**」分两摞——**在** ⇒ 模型/网关（只观察，是**选模型的证据**，不是改产品的理由）；
/// **不在** ⇒ 形状缺格（补）；**作用域被升级**（一次调用的错罚到整段）⇒ 永远是我们的事。
/// ⚠ **不许把它做成"健康分"**——那是把判断伪装成物理量。
#[derive(Default)]
struct ActionTally {
    runs: usize,
    calls: u64,
    failed: u64,
    unknown: u64,
    /// **首次调用税**：一轮里**第一个**动作就失败的那些轮的数量。
    first_call_tax: u64,
    /// 失败分类 → 计数（`BTreeMap`：输出顺序稳定，同一形状两次逐字节相同）。
    by_class: std::collections::BTreeMap<String, ClassTally>,
}

#[derive(Default)]
struct ClassTally {
    calls: u64,
    first_call: u64,
}

impl ActionTally {
    /// 读这一页活动的账。**读不出来就如实失败**（与 `[state]` 那条纪律同款：不安静跳过）。
    fn read(trace: &[celira_agent::store::TraceRecord], activities: &[ActivityLine]) -> Self {
        let all = trace;
        let mut tally = ActionTally::default();
        for line in activities {
            tally.runs += 1;
            let mut turns_seen: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
            for record in all.iter().filter(|record| record.run() == &line.run) {
                let TracePayload::Observed {
                    turn,
                    state,
                    failure,
                    ..
                } = record.entry()
                else {
                    continue;
                };
                tally.calls += 1;
                // **首次调用税**按"轮"算：一轮里第一个动作就失败，是主体最贵的那种错。
                let first = turns_seen.insert(*turn);
                match state {
                    celira_agent::foundation::ResultState::Failure => {
                        tally.failed += 1;
                        if first {
                            tally.first_call_tax += 1;
                        }
                        if let Some(class) = failure {
                            let entry = tally.by_class.entry(class.clone()).or_default();
                            entry.calls += 1;
                            if first {
                                entry.first_call += 1;
                            }
                        }
                    }
                    celira_agent::foundation::ResultState::Unknown => tally.unknown += 1,
                    celira_agent::foundation::ResultState::Success => {}
                }
            }
        }
        tally
    }

    /// 行式稳定文本：字段固定、分类按名字排序，同一形状两次逐字节相同。
    fn render(&self) -> String {
        let mut out = String::from("[actions]\n");
        let _ = writeln!(
            out,
            "scope=runs-in-page runs={} calls={} failed={} unknown={} first-call-tax={}",
            self.runs, self.calls, self.failed, self.unknown, self.first_call_tax
        );
        for (class, tally) in &self.by_class {
            let _ = writeln!(
                out,
                "failure={} calls={} first-call={}",
                escape_field(class),
                tally.calls,
                tally.first_call
            );
        }
        out
    }
}

/// **时间锚**：账上最后一条经历的时刻，以及**它上一次醒**（最后一条 `run-start`）的时刻。
///
/// 它答的是操作者那个问题——「**进程在，它还在动吗**」。判据是**只给物理量**：
/// 这里只有**绝对时刻**（打印出来不会过期），而"它是不是卡住了、该不该出手"是**操作者的判断**
/// ——不写成读数（仓库那条纪律：不显示会过期的投影 ⇒ **不打印"安静了多久"**）。
///
/// **零新事实、零新写者**：两格都从账里现读（记录本来就带时刻）。「没有」与「读不到」分开表达：
/// 账上一条都没有 ⇒ `none`；有记录但那条没带时刻 ⇒ `unknown`。
#[derive(Default)]
struct ActivityTimes {
    last_wake: Option<u64>,
    last_record: Option<u64>,
    records: usize,
}

impl ActivityTimes {
    fn of(trace: &[celira_agent::store::TraceRecord]) -> Self {
        let mut times = ActivityTimes {
            records: trace.len(),
            ..ActivityTimes::default()
        };
        for record in trace {
            let Some(at) = record.at_millis() else {
                continue;
            };
            times.last_record = Some(at);
            if matches!(record.entry(), TracePayload::RunStarted { .. }) {
                times.last_wake = Some(at);
            }
        }
        times
    }

    /// 两行**稳定字段**（行式、同一形状两次逐字节相同）。
    fn render(&self) -> String {
        format!(
            "last-wake-at={}\nlast-record-at={}",
            stamp(self.last_wake, self.records),
            stamp(self.last_record, self.records)
        )
    }
}

/// 一个时刻的稳定文本：RFC 3339（UTC）。没有账 ⇒ `none`；有账但没带时刻 ⇒ `unknown`（不编造）。
fn stamp(at: Option<u64>, records: usize) -> String {
    match at {
        Some(ms) => celira_agent::clock::format_utc(
            std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms),
        ),
        None if records == 0 => "none".to_string(),
        None => "unknown".to_string(),
    }
}
