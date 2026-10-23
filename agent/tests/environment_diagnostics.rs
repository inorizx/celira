//! TASK_03 验收：**持久操作事实的重读与诊断闭环**。
//!
//! 这里证明的是一条闭环，而不是"多了一个函数"：
//!
//! ```text
//! 进程 A 经真实环境入口执行并保存 → A 退出
//!   → 进程 B 显式打开同一身份环境
//!   → 按 Run 读取并校验持久事实
//!   → 复用既有单 Run 解释器
//!   → 宿主拿到操作状态、结束事实及**独立**的存储状态
//! ```
//!
//! ## "只读、不执行"的三类证据，各自证明什么
//!
//! - **类型限制（TY）**：诊断函数只接受 `&Environment`，而追加事实/经历、写暂停标记与
//!   清除暂停都需要 `&mut Environment`，所以"写这个环境"在编译期就不成立。它**不**证明
//!   "模块里不存在任何执行调用"——构造一个模型替身并不需要环境，那个结论单靠类型得不到。
//! - **调用链审查（SR，文本级）**：[`the_diagnosis_path_has_no_execution_entry_point`]
//!   盯住诊断模块与诊断命令里没有执行循环、模型端口或行动端口的引用。这是**窄条件**，
//!   不是链接期证明，也不代替运行时证据。
//! - **文件对照（BT）**：跨进程与暂停环境两条测试比较调用前后的文件字节、目录成员与
//!   准入读数，证明"**这一次**调用没有改盘上状态"。它**不**证明"任何未来路径都不会执行"。
//!
//! 明确**不**声称的：
//!
//! - 不声称"诊断完全不接触经历存储"：`open` 本身要全量解码整份已确认前缀；sealed 也只记在
//!   经历侧。诊断**不读经历正文**、不从正文推断结果，但读取这件事发生过。
//! - 不声称"零经历 I/O"：只证明"只改正文不改操作解释"这一条窄性质。
//! - 不声称"没有执行调用"能由任何单一类别证据得出：它由上面三类**合起来**支持，各自只
//!   覆盖自己那一层。
use std::path::{Path, PathBuf};
use std::process::Command;

use celira_agent::commit::EnvironmentCommit;
use celira_agent::composition::{EnvironmentActivity, run_activity_in_environment};
use celira_agent::core::{
    Action, ActionAdmission, ActionOutcome, ActionParams, ActionPort, Agent, Capabilities, Limits,
    ModelError, ModelFailureClass, ModelOutput, ModelPort, NoContext, RunInput, RunRequest, View,
    no_cost, run_activity_with,
};
use celira_agent::diagnostics::{
    ACTIVITY_PAGE_LIMIT, ActivitySides, DiagnosticError, FACT_PAGE_LIMIT, diagnose_activity,
    diagnose_environment, has_unexplained_activity, render_activity, render_environment,
};
use celira_agent::foundation::{AgentId, OperationId, Outcome, Record, RunId, SubjectEnding};
use celira_agent::store::{Environment, FactRow, StoreError, records_of_run};
mod common;
use common::binary;

const ROLE_ENV: &str = "CELIRA_PHASE08_ROLE";
const ROOT_ENV: &str = "CELIRA_PHASE08_ROOT";

fn agent_a() -> AgentId {
    AgentId::new("agent-alpha")
}

fn root_env_of(root: &Path) -> PathBuf {
    root.join("env")
}

// ---------------------------------------------------------------------------
// 1. 五类记录的实际写入 → 读回 → 转换，以及分页不丢不重
// ---------------------------------------------------------------------------

/// 一次活动发出多少个行动（用来把事实行数推过一页）。
const ACTION_TURNS: u32 = 40;
/// 除两个主要活动之外再补多少个"立刻收场"的活动（用来把活动数推过一页）。
const FILLER_RUNS: usize = 35;

#[test]
fn five_record_kinds_round_trip_and_paging_never_loses_or_repeats() {
    let root = TempDir::new("diag-paging");
    let sandbox = root.path().join("sandbox");
    std::fs::create_dir_all(&sandbox).expect("建沙盒");

    let mut env = Environment::create(&root_env_of(root.path()), agent_a()).expect("建环境");
    let mut agent = Agent::new(agent_a());
    let mut ledger = celira_agent::foundation::Ledger::new(agent_a());

    // 一个"发很多次行动"的活动：事实行数超过一页，逼出事实分页。
    let mut script: Vec<ModelOutput> = (0..ACTION_TURNS)
        .map(|_| ModelOutput::act(Action::with_params(action_id(), ActionParams::none())))
        .collect();
    script.push(ModelOutput::finish(SubjectEnding::Completed));
    let mut model = ScriptedModel::new(script);
    let mut actions = SuccessActions;
    let mut commit = EnvironmentCommit::new(&mut env);
    let long_run = run_activity_with(
        &mut agent,
        RunRequest {
            run: RunId::new("run-actions"),
            limits: Limits::new(ACTION_TURNS + 4),
            input: RunInput::empty(),
            source: &mut NoContext,
            commit: &mut commit,
            cost: &no_cost,
        },
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("长活动应正常收场");
    assert_eq!(
        long_run.outcome(),
        Some(Outcome::Subject(SubjectEnding::Completed))
    );
    // 写资格随 `commit` 的最后一次使用结束（它没有 `Drop`，不需要显式释放）。

    // 一个经**真实环境入口**收场的活动（证明诊断读的是同一个环境目录）。
    let mut entry_model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
    run_activity_in_environment(
        &sandbox,
        &mut env,
        EnvironmentActivity {
            declared_agent: &agent_a(),
            run: RunId::new("run-entry"),
            limits: Limits::new(4),
            input: RunInput::empty(),
            source: &mut NoContext,
            creation: None,
        },
        &mut entry_model,
    )
    .expect("入口活动应正常收场");

    // 若干"立刻收场"的活动：把活动数推过多页，同时证明逐活动不串线。
    for index in 0..FILLER_RUNS {
        let mut filler_model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
        let mut filler_actions = SuccessActions;
        let mut filler_agent = Agent::new(agent_a());
        let mut filler_ledger = celira_agent::foundation::Ledger::new(agent_a());
        let mut filler_commit = EnvironmentCommit::new(&mut env);
        run_activity_with(
            &mut filler_agent,
            RunRequest {
                run: RunId::new(format!("run-filler-{index}")),
                limits: Limits::new(4),
                input: RunInput::empty(),
                source: &mut NoContext,
                commit: &mut filler_commit,
                cost: &no_cost,
            },
            &mut filler_model,
            &mut filler_actions,
            &mut filler_ledger,
        )
        .expect("填充活动应正常收场");
    }

    // 两个**只在事实侧**出现的活动：让附属列表（可判定的跨存储矛盾）真的有内容，
    // 而且分页之后仍然完整。
    for run in ["run-only-a", "run-only-b"] {
        let run_id = RunId::new(run);
        env.append_fact(
            &run_id,
            &Record::RunStarted {
                agent: agent_a(),
                run: run_id.clone(),
            },
        )
        .expect("写事实侧孤立活动");
    }

    let all_facts = env.facts().expect("读回事实");
    let long_rows: Vec<FactRow> = all_facts
        .iter()
        .filter(|row| row.run() == "run-actions")
        .cloned()
        .collect();

    // ---- 五类记录：实际写入 → 读回 → 转换 ----
    let records = records_of_run(&long_rows, &agent_a()).expect("长活动的事实应可重建");
    assert_eq!(records.len(), long_rows.len(), "重建不得丢行或多行");
    let mut kinds: Vec<&'static str> = Vec::new();
    for record in &records {
        kinds.push(match record {
            Record::RunStarted { .. } => "run-started",
            Record::ActionPrepared { .. } => "action-prepared",
            Record::ActionDispatched { .. } => "action-dispatched",
            Record::ActionResultRecorded { .. } => "action-result",
            Record::RunEnded { .. } => "run-ended",
        });
    }
    for required in [
        "run-started",
        "action-prepared",
        "action-dispatched",
        "action-result",
        "run-ended",
    ] {
        assert!(kinds.contains(&required), "转换结果里缺少 {required}");
    }
    assert_eq!(kinds[0], "run-started", "第一条必须是活动开始");
    assert_eq!(kinds[kinds.len() - 1], "run-ended", "最后一条必须是收场");
    // 每条行动事实按"准备 → 发出 → 结果"成组，顺序不得被重排。
    for group in 0..ACTION_TURNS as usize {
        let base = 1 + group * 3;
        assert_eq!(
            &kinds[base..base + 3],
            &["action-prepared", "action-dispatched", "action-result"],
            "第 {group} 次尝试的三条事实必须成组且有序"
        );
    }
    // 重建出来的操作身份由 RunId + Turn 推出：序号必须逐条前进。
    for (index, record) in records.iter().enumerate() {
        if let Record::ActionPrepared { operation, .. } = record {
            assert_eq!(operation.run().as_str(), "run-actions");
            assert_eq!(operation.turn().number(), index as u32 / 3 + 1);
        }
    }
    // 收场事实的取值也必须被解释成主体收场。
    assert_eq!(
        records.last(),
        Some(&Record::RunEnded {
            run: RunId::new("run-actions"),
            outcome: Outcome::Subject(SubjectEnding::Completed),
            cost: celira_agent::cost::RunCost::default(),
        })
    );

    // ---- 单个活动的完整解释：始终基于完整序列 ----
    let detail = diagnose_activity(&env.reader(), &RunId::new("run-actions"), None, 8)
        .expect("长活动应可诊断");
    let reading = detail.diagnosis.reading.as_ref().expect("事实应可重建");
    assert_eq!(reading.fact_rows, long_rows.len());
    assert_eq!(reading.counts.prepared, ACTION_TURNS as usize);
    assert_eq!(reading.counts.dispatched, ACTION_TURNS as usize);
    assert_eq!(reading.counts.success, ACTION_TURNS as usize);
    assert_eq!(reading.counts.failure, 0);
    assert_eq!(reading.counts.unknown, 0);
    assert_eq!(reading.counts.prepared_without_dispatch, 0);
    assert_eq!(reading.counts.dispatched_without_result, 0);
    let recovered = reading.recovered.as_ref().expect("序列应能解释");
    assert_eq!(
        recovered.outcome,
        Some(Outcome::Subject(SubjectEnding::Completed))
    );
    assert_eq!(
        recovered.last_action_turn.map(|turn| turn.number()),
        Some(ACTION_TURNS)
    );
    assert!(recovered.unconfirmed.is_empty());

    // ---- 事实分页：逐页取回，不丢不重；每一页的解释都必须一样 ----
    let expected_seqs: Vec<u64> = long_rows.iter().map(|row| row.seq()).collect();
    let mut collected: Vec<u64> = Vec::new();
    let mut cursor: Option<u64> = None;
    let mut pages = 0usize;
    loop {
        let page = diagnose_activity(
            &env.reader(),
            &RunId::new("run-actions"),
            cursor,
            FACT_PAGE_LIMIT,
        )
        .expect("分页应成功");
        pages += 1;
        assert!(
            !page.rows.is_empty() || !page.has_more,
            "还有后续时不得返回空页"
        );
        assert_eq!(
            page.diagnosis
                .reading
                .as_ref()
                .ok()
                .map(|reading| reading.fact_rows),
            Some(long_rows.len()),
            "分页片段不得被当成完整活动来解释"
        );
        collected.extend(page.rows.iter().map(|row| row.seq()));
        if !page.has_more {
            assert_eq!(page.next_from, None, "取完之后不得再给游标");
            break;
        }
        cursor = page.next_from;
        assert!(cursor.is_some(), "还有后续时必须给出下一段游标");
        assert!(pages < 64, "分页没有收敛");
    }
    assert!(pages > 1, "这份事实必须超过一页，否则这条测试证明不了分批");
    assert_eq!(collected, expected_seqs, "逐页取回必须与完整序列逐条相同");

    // ---- 活动分页，连带有界的两份附属列表 ----
    let expected_runs: Vec<RunId> = activity_runs(&env);
    assert!(
        expected_runs.len() > ACTIVITY_PAGE_LIMIT,
        "活动数必须超过一页，否则这条测试证明不了分批"
    );

    // 整份存储的机械列表：附属列表必须与它逐项对应，而不是整份塞进每一页。
    let health = env.health().expect("读存储状态");
    let mut expected_issues: Vec<(String, String)> = health
        .cross_store()
        .iter()
        .map(|issue| (issue.run().as_str().to_string(), issue.kind().to_string()))
        .collect();
    let mut expected_summaries: Vec<String> = health
        .runs()
        .iter()
        .map(|summary| summary.run().as_str().to_string())
        .collect();
    assert!(
        !expected_issues.is_empty(),
        "夹具必须造出可判定的矛盾，否则这一条证明不了什么"
    );
    assert!(
        expected_summaries.len() > ACTIVITY_PAGE_LIMIT,
        "经历侧活动数必须超过一页"
    );

    let mut walked: Vec<RunId> = Vec::new();
    let mut gathered_issues: Vec<(String, String)> = Vec::new();
    let mut gathered_summaries: Vec<String> = Vec::new();
    let mut after: Option<RunId> = None;
    let mut rounds = 0usize;
    loop {
        let report = diagnose_environment(&env.reader(), after.as_ref(), ACTIVITY_PAGE_LIMIT)
            .expect("活动分页应成功");
        assert_eq!(report.agent, agent_a());

        // 附属列表逐页**有界**：不超过本页活动数，也不超过活动页上限。
        assert!(report.storage.cross_store.len() <= ACTIVITY_PAGE_LIMIT);
        assert!(report.storage.trace_activities.len() <= ACTIVITY_PAGE_LIMIT);
        // 整份条数必须与存储读数一致：少了它，调用方分不清"本页没有"与"整份没有"。
        assert_eq!(
            report.storage.cross_store_total,
            expected_issues.len(),
            "附属列表的整份条数必须与存储读数一致"
        );
        assert_eq!(
            report.storage.trace_activities_total,
            expected_summaries.len()
        );
        // 本页之外的项不是被丢掉，而是由下一页返回：附属列表不得越出本页活动。
        let page_runs: Vec<&RunId> = report
            .activities
            .iter()
            .map(|activity| &activity.run)
            .collect();
        for issue in &report.storage.cross_store {
            assert!(
                page_runs.contains(&issue.run()),
                "矛盾列表越出了本页活动: {}",
                issue.run()
            );
        }
        for summary in &report.storage.trace_activities {
            assert!(
                page_runs.contains(&summary.run()),
                "活动摘要越出了本页活动: {}",
                summary.run()
            );
        }

        walked.extend(
            report
                .activities
                .iter()
                .map(|activity| activity.run.clone()),
        );
        gathered_issues.extend(
            report
                .storage
                .cross_store
                .iter()
                .map(|issue| (issue.run().as_str().to_string(), issue.kind().to_string())),
        );
        gathered_summaries.extend(
            report
                .storage
                .trace_activities
                .iter()
                .map(|summary| summary.run().as_str().to_string()),
        );

        rounds += 1;
        if !report.has_more {
            assert_eq!(report.next_after, None);
            break;
        }
        after = report.next_after.clone();
        assert!(after.is_some(), "还有后续时必须给出下一段游标");
        assert!(rounds < 64, "活动分页没有收敛");
    }
    assert!(rounds > 1, "活动列表必须超过一页");
    assert_eq!(walked, expected_runs, "逐页取回必须与完整活动顺序逐条相同");

    // 两份附属列表：逐页取回的并集与整份存储逐项对应——不丢项、不重复。
    assert_eq!(
        gathered_issues.len(),
        expected_issues.len(),
        "矛盾列表不得丢项或重复"
    );
    assert_eq!(
        gathered_summaries.len(),
        expected_summaries.len(),
        "活动摘要不得丢项或重复"
    );
    gathered_issues.sort();
    expected_issues.sort();
    assert_eq!(
        gathered_issues, expected_issues,
        "矛盾列表逐页取回必须等于整份存储"
    );
    gathered_summaries.sort();
    expected_summaries.sort();
    assert_eq!(
        gathered_summaries, expected_summaries,
        "活动摘要逐页取回必须等于整份存储"
    );

    // 第一页必须真的**不完整**，否则"按页收窄"这件事在这条测试里没被证明。
    let first = diagnose_environment(&env.reader(), None, ACTIVITY_PAGE_LIMIT).expect("首页");
    assert!(first.has_more, "夹具必须超过一页");
    assert!(
        first.storage.trace_activities_total > first.storage.trace_activities.len(),
        "首页不得把整份附属列表塞进来"
    );

    // ---- 非法与未知的分页参数必须明确失败，不得静默截断 ----
    assert_eq!(
        diagnose_environment(&env.reader(), None, 0),
        Err(DiagnosticError::InvalidLimit {
            limit: 0,
            max: ACTIVITY_PAGE_LIMIT
        })
    );
    assert_eq!(
        diagnose_environment(&env.reader(), None, ACTIVITY_PAGE_LIMIT + 1),
        Err(DiagnosticError::InvalidLimit {
            limit: ACTIVITY_PAGE_LIMIT + 1,
            max: ACTIVITY_PAGE_LIMIT
        })
    );
    assert_eq!(
        diagnose_activity(
            &env.reader(),
            &RunId::new("run-actions"),
            None,
            FACT_PAGE_LIMIT + 1
        ),
        Err(DiagnosticError::InvalidLimit {
            limit: FACT_PAGE_LIMIT + 1,
            max: FACT_PAGE_LIMIT
        })
    );
    // 未知游标与"已经取完"是两件事。
    assert_eq!(
        diagnose_environment(&env.reader(), Some(&RunId::new("run-不存在")), 4),
        Err(DiagnosticError::UnknownCursor(RunId::new("run-不存在")))
    );
    // 未知活动与真实空环境也是两件事。
    assert_eq!(
        diagnose_activity(&env.reader(), &RunId::new("run-不存在"), None, 4),
        Err(DiagnosticError::UnknownActivity(RunId::new("run-不存在")))
    );
}

// ---------------------------------------------------------------------------
// 2. `open` 接受、但**无法解释**的行必须被点名，而不是被跳过
// ---------------------------------------------------------------------------

/// 打开环境能接受、但诊断重建不出来的事实行。
///
/// 每一行都单独占一个活动，好让"哪一个活动、哪一行、哪一类问题"可判错。
/// 这些行都通过了 `open` 的校验：事实种类名认识、收场取值合法或不带收场字段——
/// 缺 Turn、结果缺状态、状态取值不认识、身份不一致、多余字段都不在打开校验里。
///
/// `run-beyond` 这个活动**不是**异常：它的 Turn 序号（99）远大于该活动的事实行数，那是合法历史
/// （准入拒绝会让一个 Turn 一行事实都不留），因此它必须被**原样**还原，而不是被推断成越界。
///
/// 后三个活动走的是**另一条**路：事实**能**重建，但序列自相矛盾，既有解释器必须原样拒绝。
/// 诊断不得把"解释器拒绝了"改写成别的类别，也不得自己补一条规则把它读通。
const CRAFTED_FACTS: [&str; 24] = [
    r#"{"seq":0,"run":"run-ok","fact":"run-started","agent":"agent-crafted"}"#,
    r#"{"seq":1,"run":"run-ok","fact":"action-prepared","turn":1}"#,
    r#"{"seq":2,"run":"run-ok","fact":"action-dispatched","turn":1}"#,
    r#"{"seq":3,"run":"run-ok","fact":"action-result","turn":1,"state":"success"}"#,
    r#"{"seq":4,"run":"run-ok","fact":"run-ended","subject":"completed"}"#,
    r#"{"seq":5,"run":"run-no-turn","fact":"run-started","agent":"agent-crafted"}"#,
    r#"{"seq":6,"run":"run-no-turn","fact":"action-prepared"}"#,
    r#"{"seq":7,"run":"run-zero-turn","fact":"run-started","agent":"agent-crafted"}"#,
    r#"{"seq":8,"run":"run-zero-turn","fact":"action-prepared","turn":0}"#,
    r#"{"seq":9,"run":"run-beyond","fact":"run-started","agent":"agent-crafted"}"#,
    r#"{"seq":10,"run":"run-beyond","fact":"action-prepared","turn":99}"#,
    r#"{"seq":11,"run":"run-no-state","fact":"run-started","agent":"agent-crafted"}"#,
    r#"{"seq":12,"run":"run-no-state","fact":"action-result","turn":1}"#,
    r#"{"seq":13,"run":"run-bad-state","fact":"run-started","agent":"agent-crafted"}"#,
    r#"{"seq":14,"run":"run-bad-state","fact":"action-result","turn":1,"state":"maybe"}"#,
    r#"{"seq":15,"run":"run-foreign","fact":"run-started","agent":"someone-else"}"#,
    r#"{"seq":16,"run":"run-extra","fact":"run-started","agent":"agent-crafted","turn":1}"#,
    // 发出之前没有准备：重建得出事实，但序列自相矛盾。
    r#"{"seq":17,"run":"run-dispatch-only","fact":"run-started","agent":"agent-crafted"}"#,
    r#"{"seq":18,"run":"run-dispatch-only","fact":"action-dispatched","turn":1}"#,
    // 有结果却没有准备。
    r#"{"seq":19,"run":"run-result-only","fact":"run-started","agent":"agent-crafted"}"#,
    r#"{"seq":20,"run":"run-result-only","fact":"action-result","turn":1,"state":"success"}"#,
    // 一次活动出现两个收场事实。
    r#"{"seq":21,"run":"run-two-endings","fact":"run-started","agent":"agent-crafted"}"#,
    r#"{"seq":22,"run":"run-two-endings","fact":"run-ended","subject":"completed"}"#,
    r#"{"seq":23,"run":"run-two-endings","fact":"run-ended","system":"timeout"}"#,
];

#[test]
fn rows_that_open_accepts_but_cannot_be_explained_are_named_not_hidden() {
    let root = TempDir::new("diag-crafted");
    let env_dir = root_env_of(root.path());
    let crafter = AgentId::new("agent-crafted");

    // 用真实入口建立环境（身份记录 + 两份空存储 + 写锁），然后放掉写资格再改内容：
    // 这里要的正是"打开能过、诊断过不去"的那种行，公开接口写不出来，只能作为受控夹具。
    drop(Environment::create(&env_dir, crafter.clone()).expect("建环境"));
    let mut trace = String::new();
    for (seq, run) in ["run-ok"].iter().enumerate() {
        trace.push_str(&format!(
            "{{\"seq\":{seq},\"run\":\"{run}\",\"kind\":\"run-start\"}}\n"
        ));
    }
    std::fs::write(env_dir.join("trace.ndjson"), trace).expect("写经历夹具");
    let mut facts = String::new();
    for line in CRAFTED_FACTS {
        facts.push_str(line);
        facts.push('\n');
    }
    std::fs::write(env_dir.join("facts.ndjson"), facts).expect("写事实夹具");

    let env = Environment::open(&env_dir, crafter.clone()).expect("这些行必须能通过打开校验");

    let report =
        diagnose_environment(&env.reader(), None, ACTIVITY_PAGE_LIMIT).expect("诊断应产出报告");
    assert!(has_unexplained_activity(&report), "必须报出无法解释");

    let kind_of = |run: &str| -> Option<(&'static str, u64)> {
        report
            .activities
            .iter()
            .find(|activity| activity.run.as_str() == run)
            .and_then(|activity| match &activity.reading {
                Err(error) => Some((error.as_str(), error.seq())),
                Ok(_) => None,
            })
    };

    // 一行一类问题：类别与定位（稳定 seq）都要如实报出来。
    assert_eq!(kind_of("run-no-turn"), Some(("missing-turn", 6)));
    assert_eq!(kind_of("run-zero-turn"), Some(("illegal-turn", 8)));
    // Turn 序号远大于事实行数**不是**异常：它必须被原样还原（见下面的精确断言）。
    assert_eq!(kind_of("run-beyond"), None);
    assert_eq!(kind_of("run-no-state"), Some(("missing-state", 12)));
    assert_eq!(kind_of("run-bad-state"), Some(("illegal-state", 14)));
    assert_eq!(kind_of("run-foreign"), Some(("foreign-agent", 15)));
    assert_eq!(kind_of("run-extra"), Some(("unexpected-field", 16)));

    // 一条异常不得让整份报告冒充"全量成功"，也不得把别的活动一起吞掉。
    let good = report
        .activities
        .iter()
        .find(|activity| activity.run.as_str() == "run-ok")
        .expect("正常活动必须还在报告里");
    let reading = good.reading.as_ref().expect("正常活动的事实应可重建");
    assert_eq!(
        reading.recovered.as_ref().expect("序列应能解释").outcome,
        Some(Outcome::Subject(SubjectEnding::Completed))
    );
    assert_eq!(good.sides, ActivitySides::BothStores);

    // 只在事实侧出现的活动必须被如实标出，而不是被隐去。
    for run in ["run-no-turn", "run-foreign", "run-extra"] {
        let activity = report
            .activities
            .iter()
            .find(|activity| activity.run.as_str() == run)
            .expect("事实侧孤立活动必须被列出");
        assert_eq!(activity.sides, ActivitySides::FactsOnly, "{run}");
    }

    // 两侧锚点矛盾也要各自报出来：事实侧有、经历侧没有。
    let issues: Vec<(&str, &str)> = report
        .storage
        .cross_store
        .iter()
        .map(|issue| (issue.run().as_str(), issue.kind()))
        .collect();
    assert!(
        issues.contains(&("run-no-turn", "fact-without-start")),
        "事实侧孤立活动必须报出矛盾: {issues:?}"
    );
    assert!(
        issues.contains(&("run-ok", "fact-without-ending")),
        "经历侧缺结束锚点也必须报出: {issues:?}"
    );

    // ---- 另一条路：事实能重建，但序列自相矛盾 ----
    //
    // 诊断**不合成**任何解释错误，也不把"解释器拒绝了"改写成别的类别：把该活动的完整序列
    // 原样交给既有解释器，读出什么就报什么。（按 Run 分组之后，`foreign-run` 这一类在
    // 本路径上够不到——一行事实的活动永远是它自己那一组。）
    let rejection = |run: &str| -> Option<String> {
        report
            .activities
            .iter()
            .find(|activity| activity.run.as_str() == run)
            .and_then(|activity| activity.reading.as_ref().ok())
            .and_then(|reading| reading.recovered.as_ref().err())
            .map(|reason| format!("{reason:?}"))
    };
    assert!(
        rejection("run-dispatch-only")
            .is_some_and(|reason| reason.starts_with("DispatchWithoutPreparation")),
        "{:?}",
        rejection("run-dispatch-only")
    );
    assert!(
        rejection("run-result-only")
            .is_some_and(|reason| reason.starts_with("ResultWithoutPreparation")),
        "{:?}",
        rejection("run-result-only")
    );
    assert_eq!(
        rejection("run-two-endings").as_deref(),
        Some("MultipleEndings")
    );
    // 这三种"解释不了"同样不能被当成健康：它们的重建是成功的，所以只有解释器这一层能说清。
    for run in ["run-dispatch-only", "run-result-only", "run-two-endings"] {
        let activity = report
            .activities
            .iter()
            .find(|activity| activity.run.as_str() == run)
            .expect("必须被列出");
        assert!(activity.reading.is_ok(), "{run} 的事实本身应能重建");
    }

    // 渲染出来的报告必须把每一类问题都写成可被判错的文本。
    let rendered = render_environment(&report);
    assert!(rendered.contains("conversion=error kind=missing-turn seq=6"));
    assert!(rendered.contains("conversion=error kind=foreign-agent seq=15"));
    assert!(
        rendered.contains(
            "activity run=run-beyond sides=facts-only fact-rows=2 conversion=ok recover=ok"
        ),
        "{rendered}"
    );
    assert!(
        rendered.contains("last-action-turn=99"),
        "远大于事实行数的 Turn 必须原样报出来，不能被推断成越界: {rendered}"
    );
    assert!(rendered.contains("recover=rejected reason=dispatch-without-preparation"));
    assert!(rendered.contains("recover=rejected reason=result-without-preparation"));
    assert!(rendered.contains("recover=rejected reason=multiple-endings"));

    // 重建把 Turn 序号**原样**还原：既不放宽、也不收紧，更不因为"行数少"就判越界。
    let beyond_rows: Vec<FactRow> = env
        .facts()
        .expect("读回事实")
        .into_iter()
        .filter(|row| row.run() == "run-beyond")
        .collect();
    let rebuilt = records_of_run(&beyond_rows, &crafter).expect("远大于行数的 Turn 是合法历史");
    assert!(
        matches!(
            &rebuilt[1],
            Record::ActionPrepared { operation, .. }
                if operation.turn().number() == 99
        ),
        "{rebuilt:?}"
    );
    drop(env);

    // ---- 真实消费者：同样的环境，退出码必须把“解释不了”从“健康”里分出来 ----
    let env_root = || env_dir.to_string_lossy().into_owned();
    let cli = run_diagnose(&[env_root(), "agent-crafted".to_string()]);
    assert_eq!(
        cli.status.code(),
        Some(4),
        "有活动解释不了时不得返回 0。stdout={} stderr={}",
        String::from_utf8_lossy(&cli.stdout),
        String::from_utf8_lossy(&cli.stderr)
    );
    let cli_text = String::from_utf8_lossy(&cli.stdout).into_owned();
    assert!(
        cli_text.contains("conversion=error kind=missing-turn seq=6"),
        "{cli_text}"
    );
    assert!(
        cli_text.contains("recover=rejected reason=multiple-endings"),
        "{cli_text}"
    );
    // 两个来源各自说话，而且**不**被拼成一个看似正常的状态：经历侧只有一个活动开始、
    // 没有结束锚点，所以准入说“没有收尾”；事实侧同一活动的结束事实却已经在盘上。
    assert_eq!(field_of(&cli_text, "admission"), "unfinished");
    assert_eq!(field_of(&cli_text, "admission-run"), "run-ok");
    assert!(
        cli_text.contains("activity run=run-ok sides=both-stores"),
        "{cli_text}"
    );

    // ---- `--run` 必须用**同一个**判据：不得打印完就无条件返回 0 ----
    let single = |run: &str| {
        run_diagnose(&[
            env_root(),
            "agent-crafted".to_string(),
            "--run".into(),
            run.to_string(),
        ])
    };

    let healthy = single("run-ok");
    assert_eq!(healthy.status.code(), Some(0), "可解释的活动仍然是 0");
    assert!(
        String::from_utf8_lossy(&healthy.stdout).contains("recover=ok outcome=subject:completed"),
        "{}",
        String::from_utf8_lossy(&healthy.stdout)
    );

    // 反例一：事实重建不出来。
    let converted = single("run-no-turn");
    assert_eq!(
        converted.status.code(),
        Some(4),
        "重建不出来时必须 4。stdout={}",
        String::from_utf8_lossy(&converted.stdout)
    );
    let converted_text = String::from_utf8_lossy(&converted.stdout).into_owned();
    assert!(
        converted_text.contains("conversion=error kind=missing-turn seq=6"),
        "{converted_text}"
    );
    assert!(
        !converted_text.contains("recover=ok"),
        "重建失败时不得同时出现一个看似成功的解释"
    );

    // 反例二：事实重建成功，但既有解释器拒绝了序列。
    let rejected = single("run-dispatch-only");
    assert_eq!(
        rejected.status.code(),
        Some(4),
        "解释器拒绝时必须 4。stdout={}",
        String::from_utf8_lossy(&rejected.stdout)
    );
    let rejected_text = String::from_utf8_lossy(&rejected.stdout).into_owned();
    assert!(
        rejected_text.contains("recover=rejected reason=dispatch-without-preparation"),
        "{rejected_text}"
    );
    // 同一个活动，两种查询方式必须得到同一个结论——这正是这条测试要钉住的东西。
    assert!(
        cli_text.contains(
            "activity run=run-dispatch-only sides=facts-only fact-rows=2 conversion=ok recover=rejected"
        ),
        "{cli_text}"
    );
}

// ---------------------------------------------------------------------------
// 3. 事实与正文分离：只改经历正文，不改操作解释
// ---------------------------------------------------------------------------

#[test]
fn changing_only_the_recorded_text_does_not_change_the_operation_reading() {
    let root = TempDir::new("diag-body");
    let sandbox = root.path().join("sandbox");
    std::fs::create_dir_all(&sandbox).expect("建沙盒");
    std::fs::create_dir_all(root.path().join("materials")).expect("建资料目录");

    let env_dir = root_env_of(root.path());
    let mut env = Environment::create(&env_dir, agent_a()).expect("建环境");
    let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Completed)]);
    run_activity_in_environment(
        &sandbox,
        &mut env,
        EnvironmentActivity {
            declared_agent: &agent_a(),
            run: RunId::new("run-body"),
            limits: Limits::new(4),
            input: RunInput::text(ORIGINAL_TEXT).expect("在上限内"),
            source: &mut NoContext,
            creation: None,
        },
        &mut model,
    )
    .expect("活动应正常收场");

    let before = diagnose_activity(
        &env.reader(),
        &RunId::new("run-body"),
        None,
        FACT_PAGE_LIMIT,
    )
    .expect("诊断应成功");
    drop(env);

    // ⚠ **2026-10-06 改**（校验和那一刀）：从前这条用例是"复制一份环境、把经历正文改掉"——
    // 而校验和落地之后，**手改正文会被明确拒绝**（那正是新保证：改过的字节不再冒充写下的字节）。
    // 判据一个字没松，只是换一条**不破坏完整性**的造法：同一个活动、同样的行动，**两份环境只在
    // 开场输入正文上不同**——操作解释只接事实与已校验身份，因此两份必须一模一样。
    let other_dir = root.path().join("other");
    let mut other = Environment::create(&other_dir, agent_a()).expect("建第二份环境");
    let mut other_model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Completed)]);
    run_activity_in_environment(
        &sandbox,
        &mut other,
        EnvironmentActivity {
            declared_agent: &agent_a(),
            run: RunId::new("run-body"),
            limits: Limits::new(4),
            input: RunInput::text(REPLACEMENT_TEXT).expect("在上限内"),
            source: &mut NoContext,
            creation: None,
        },
        &mut other_model,
    )
    .expect("第二份活动也应正常收场");

    // 夹具必须真的"只差正文"：两份经历里各自有自己的那句，且没有对方那句。
    let first_trace = std::fs::read_to_string(env_dir.join("trace.ndjson")).expect("读经历");
    let second_trace = std::fs::read_to_string(other_dir.join("trace.ndjson")).expect("读经历");
    assert!(
        first_trace.contains(ORIGINAL_TEXT) && !first_trace.contains(REPLACEMENT_TEXT),
        "第一份经历应只有原来的正文"
    );
    assert!(
        second_trace.contains(REPLACEMENT_TEXT) && !second_trace.contains(ORIGINAL_TEXT),
        "第二份经历应只有换过的正文"
    );

    let after = diagnose_activity(
        &other.reader(),
        &RunId::new("run-body"),
        None,
        FACT_PAGE_LIMIT,
    )
    .expect("第二份诊断应成功");

    // 操作解释完全一样：它只接事实与已校验身份，不从经历正文推断结果。
    assert_eq!(
        before.diagnosis, after.diagnosis,
        "只改经历正文不得改变操作解释"
    );
    // 但这条**不**证明零经历 I/O，也不证明底层没有读过正文：只证明解释不受正文影响。
}

/// **手改经历正文会被拒绝**（2026-10-06 起，校验和那一刀）。
///
/// 这正是上一条用例**从前**用来造夹具的做法（复制一份、把正文换掉）——校验和落地之后它不再是
/// "能打开但解释不变"，而是"**打不开**"：那串字节与写下去时不是同一串，可判定。
/// 为什么这条必须单独钉住：它是**唯一**能发现"文本字段里翻了一个 bit"的机制
/// （JSON 解析得出来、词表也解释得了，只有校验和看得见）。
#[test]
fn a_hand_edited_body_is_refused_by_its_checksum() {
    let root = TempDir::new("diag-body-edited");
    let sandbox = root.path().join("sandbox");
    std::fs::create_dir_all(&sandbox).expect("建沙盒");
    std::fs::create_dir_all(root.path().join("materials")).expect("建资料目录");

    let env_dir = root_env_of(root.path());
    let mut env = Environment::create(&env_dir, agent_a()).expect("建环境");
    let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Completed)]);
    run_activity_in_environment(
        &sandbox,
        &mut env,
        EnvironmentActivity {
            declared_agent: &agent_a(),
            run: RunId::new("run-body"),
            limits: Limits::new(4),
            input: RunInput::text(ORIGINAL_TEXT).expect("在上限内"),
            source: &mut NoContext,
            creation: None,
        },
        &mut model,
    )
    .expect("活动应正常收场");
    drop(env);

    let trace_path = env_dir.join("trace.ndjson");
    let trace = std::fs::read_to_string(&trace_path).expect("读经历");
    assert!(trace.contains(ORIGINAL_TEXT), "正文必须在经历里");
    std::fs::write(&trace_path, trace.replace(ORIGINAL_TEXT, REPLACEMENT_TEXT)).expect("写回经历");
    let rewritten = std::fs::read_to_string(&trace_path).expect("再读经历");
    assert!(
        rewritten.contains(REPLACEMENT_TEXT) && !rewritten.contains(ORIGINAL_TEXT),
        "夹具必须真的只改了正文"
    );

    match Environment::open(&env_dir, agent_a()) {
        Err(StoreError::ChecksumMismatch { .. }) => {}
        other => panic!("改过的正文必须被校验和拒绝，却得到 {other:?}"),
    }
}

const ORIGINAL_TEXT: &str = "原来的开场输入";
const REPLACEMENT_TEXT: &str = "换过之后的开场输入";

// ---------------------------------------------------------------------------
// 4. 跨进程：第二个进程读回第一个进程真正记下的东西，且什么都不改
// ---------------------------------------------------------------------------

#[test]
fn a_second_process_reads_back_what_the_first_process_recorded_and_changes_nothing() {
    let root = TempDir::new("diag-cross");
    let seeded = spawn_worker("diagnose-seed", root.path());
    assert!(
        seeded.status.success(),
        "建立合成环境应成功。stdout={} stderr={}",
        String::from_utf8_lossy(&seeded.stdout),
        String::from_utf8_lossy(&seeded.stderr)
    );
    let seed_report = report_line(&seeded, "SEED_REPORT:");
    let seed_facts = seed_count(&seed_report, "facts");
    let seed_trace = seed_count(&seed_report, "trace");

    let env_dir = root_env_of(root.path());
    let before = snapshot(&env_dir);

    // 第二个进程：只读诊断。
    let listing = run_diagnose(&[
        env_dir.to_string_lossy().into_owned(),
        "agent-alpha".to_string(),
    ]);
    assert_eq!(
        listing.status.code(),
        Some(0),
        "全部活动都应可解释。stdout={} stderr={}",
        String::from_utf8_lossy(&listing.stdout),
        String::from_utf8_lossy(&listing.stderr)
    );
    let text = String::from_utf8_lossy(&listing.stdout).into_owned();
    assert!(text.starts_with("celira-environment-diagnose v1"), "{text}");
    assert_eq!(field_of(&text, "agent"), "agent-alpha");

    // 准入读数：原因没有被记录下来的"没有收尾"，不是存储故障结论。
    assert_eq!(field_of(&text, "admission"), "unfinished");
    assert_eq!(field_of(&text, "admission-run"), "run-unfinished");
    assert!(
        !text.contains("admission=paused"),
        "不得把自己写成已记录的存储故障"
    );

    // 存储状态来源：条数必须与**另一个进程**独立读到的完全一致。
    let storage = first_line(&text, "storage ");
    assert_eq!(field_of(&storage, "trace-records"), seed_trace.to_string());
    assert_eq!(field_of(&storage, "fact-records"), seed_facts.to_string());
    assert_eq!(field_of(&storage, "paused"), "false");

    // 逐活动：结局、未确认操作、两侧是否都在，逐个核对。
    let activities: Vec<&str> = text
        .lines()
        .filter(|line| line.starts_with("activity "))
        .collect();
    let order: Vec<String> = activities
        .iter()
        .map(|line| field_of(line, "run"))
        .collect();
    assert_eq!(
        order,
        vec![
            "run-basic-1",
            "run-basic-2",
            "run-unknown",
            "run-unsealed",
            "run-facts-only",
            "run-unfinished",
        ],
        "活动顺序必须是机械的首次出现顺序"
    );

    let pick = |run: &str| -> String {
        activities
            .iter()
            .find(|line| field_of(line, "run") == run)
            .unwrap_or_else(|| panic!("报告里没有 {run}"))
            .to_string()
    };

    let basic = pick("run-basic-1");
    assert_eq!(field_of(&basic, "recover"), "ok");
    assert_eq!(field_of(&basic, "outcome"), "subject:completed");
    assert_eq!(field_of(&basic, "prepared"), "1");
    assert_eq!(field_of(&basic, "dispatched"), "1");
    assert_eq!(field_of(&basic, "success"), "1");

    let second = pick("run-basic-2");
    assert_eq!(field_of(&second, "outcome"), "subject:silent");
    assert_eq!(field_of(&second, "prepared"), "0");

    // 回执未达：那次尝试必须留在"未确认"里——不得被猜成成功或失败；收场是主体自己说的。
    let unknown = pick("run-unknown");
    assert_eq!(field_of(&unknown, "outcome"), "subject:silent");
    assert_eq!(field_of(&unknown, "unconfirmed"), "1");
    assert_eq!(field_of(&unknown, "unknown"), "1");
    assert_eq!(field_of(&unknown, "dispatched-without-result"), "0");

    // 结束已确认、封存没确认：操作解释说"结束了"，**存储状态**说"没有封存"。
    // 两个来源各自说话，不拼成一个看似正常的状态。
    let unsealed = pick("run-unsealed");
    assert_eq!(field_of(&unsealed, "recover"), "ok");
    assert_eq!(field_of(&unsealed, "outcome"), "subject:silent");
    let sealed_line = first_line(&text, "trace-activity run=run-unsealed");
    assert_eq!(field_of(&sealed_line, "status"), "ended-unsealed");
    assert_eq!(field_of(&sealed_line, "sealed"), "false");

    // 事实侧孤立活动必须显式出现，且与经历侧缺锚点同时报出。
    assert_eq!(field_of(&pick("run-facts-only"), "sides"), "facts-only");
    assert!(
        text.contains("cross-store run=run-facts-only kind=fact-without-start"),
        "{text}"
    );

    // 没有收尾的活动：不得被说成"还在运行"，也不得被说成可以继续。
    let unfinished = pick("run-unfinished");
    assert_eq!(field_of(&unfinished, "outcome"), "none");
    assert_eq!(field_of(&unfinished, "last-action-turn"), "none");
    assert!(!text.contains("可安全重试"), "报告不得给出任何重试结论");
    let interrupted = first_line(&text, "trace-activity run=run-unfinished");
    assert_eq!(field_of(&interrupted, "status"), "interrupted");

    // ---- 诊断没有改任何东西：文件字节与目录成员都不变 ----
    let after = snapshot(&env_dir);
    assert_eq!(before, after, "只读诊断不得改变任何文件或目录成员");

    // ---- 单活动详情也走同一个生产函数 ----
    let detail = run_diagnose(&[
        env_dir.to_string_lossy().into_owned(),
        "agent-alpha".to_string(),
        "--run".to_string(),
        "run-unknown".to_string(),
    ]);
    assert_eq!(detail.status.code(), Some(0));
    let detail_text = String::from_utf8_lossy(&detail.stdout).into_owned();
    assert!(detail_text.contains("mode=activity"), "{detail_text}");
    assert_eq!(
        field_of(&detail_text, "unconfirmed"),
        "1",
        "分页详情里的解释必须仍然基于完整序列"
    );
    let kinds: Vec<String> = detail_text
        .lines()
        .filter(|line| line.starts_with("fact "))
        .map(|line| field_of(line, "kind"))
        .collect();
    assert_eq!(
        kinds,
        vec![
            "run-started",
            "action-prepared",
            "action-dispatched",
            "action-result",
            "run-ended",
        ],
        "{detail_text}"
    );
    assert_eq!(
        field_of(&detail_text, "has-more"),
        "false",
        "这一页必须正好装下这个活动的全部事实"
    );

    let _ = render_activity(
        &diagnose_activity(
            &copied_env(&env_dir).reader(),
            &RunId::new("run-basic-1"),
            None,
            4,
        )
        .expect("详情应成功"),
    );
}

// ---------------------------------------------------------------------------
// 5. 退出码：读不出来 / 要问的东西不在这里 / 参数写错了，三者分开
// ---------------------------------------------------------------------------

#[test]
fn distinct_failure_outcomes_never_impersonate_health() {
    let root = TempDir::new("diag-exit");
    let seeded = spawn_worker("a", root.path());
    assert!(seeded.status.success(), "A 必须正常退出");
    let env_dir = root_env_of(root.path());
    let env = env_dir.to_string_lossy().into_owned();

    // 正常读数。
    assert_eq!(
        run_diagnose(&[env.clone(), "agent-alpha".to_string()])
            .status
            .code(),
        Some(0)
    );

    let wrong_identity = run_diagnose(&[env.clone(), "agent-beta".to_string()]);
    assert_eq!(wrong_identity.status.code(), Some(2), "错身份必须明确失败");
    assert!(
        String::from_utf8_lossy(&wrong_identity.stdout).is_empty(),
        "失败时不得产出看起来正常的报告"
    );

    let unknown_run = run_diagnose(&[
        env.clone(),
        "agent-alpha".to_string(),
        "--run".to_string(),
        "run-不存在".to_string(),
    ]);
    assert_eq!(
        unknown_run.status.code(),
        Some(3),
        "未知活动与空环境必须分开"
    );

    let unknown_cursor = run_diagnose(&[
        env.clone(),
        "agent-alpha".to_string(),
        "--after".to_string(),
        "run-不存在".to_string(),
    ]);
    assert_eq!(unknown_cursor.status.code(), Some(3), "未知游标不是取完了");

    let bad_limit = run_diagnose(&[
        env.clone(),
        "agent-alpha".to_string(),
        "--limit".to_string(),
        "0".to_string(),
    ]);
    assert_eq!(bad_limit.status.code(), Some(1), "超限分页必须明确失败");

    let no_arguments = run_diagnose(&[]);
    assert_eq!(no_arguments.status.code(), Some(1), "缺参数必须明确失败");

    // 末尾没有换行符的最后一段是**未知尾部**：拒绝打开，而不是把"完整的一行少了个换行"
    // 当成有效历史读出来。
    let facts_path = env_dir.join("facts.ndjson");
    let facts = std::fs::read_to_string(&facts_path).expect("读事实");
    assert!(facts.ends_with('\n'), "夹具前提：文件本来停在换行边界上");
    std::fs::write(&facts_path, facts.trim_end_matches('\n')).expect("去掉末尾换行");
    let unterminated = run_diagnose(&[env.clone(), "agent-alpha".to_string()]);
    assert_eq!(
        unterminated.status.code(),
        Some(2),
        "无换行尾部必须明确失败"
    );
    assert!(
        String::from_utf8_lossy(&unterminated.stdout).is_empty(),
        "不得把未知尾部报告成一份更短的健康历史"
    );
    std::fs::write(&facts_path, &facts).expect("还原事实存储");

    // 缺一份存储文件：这是损坏，不是"还没有历史"。
    std::fs::remove_file(env_dir.join("facts.ndjson")).expect("删掉事实存储");
    let missing = run_diagnose(&[env.clone(), "agent-alpha".to_string()]);
    assert_eq!(missing.status.code(), Some(2), "缺存储文件必须明确失败");
    assert!(
        String::from_utf8_lossy(&missing.stdout).is_empty(),
        "不得把损坏环境报告成空环境"
    );
}

/// **读者读得到写者正在持有的环境**（CLI 级，2026-10-06 读锁争用那一刀）。
///
/// 修复前的形状：本进程持有写资格时，`environment_diagnose` 报 `already-open`（退出码 2）——
/// 一个"看一眼处境"的命令，**在最需要看处境的那一刻恰好读不到**。现在它**读者打开**（不取写资格、
/// 读已确认前缀），因此照常出报告；写者那一侧一字未改（第二个写者照旧被拒）。
#[test]
fn the_diagnose_command_reads_while_a_writer_holds_the_environment() {
    let root = TempDir::new("diag-while-writing");
    let env_dir = root_env_of(root.path());
    // 本进程就是那个写者：这个句柄一直开到用例结束。
    let writer = Environment::create(&env_dir, agent_a()).expect("建环境");

    let output = run_diagnose(&[
        env_dir.to_string_lossy().to_string(),
        agent_a().as_str().to_string(),
    ]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "写者持有时必须照常出报告（而不是 already-open）：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("celira-environment-diagnose v1"),
        "报告头要在：{text}"
    );
    assert!(
        !text.contains("already-open"),
        "读者不该报写者那一类的失败：{text}"
    );

    // 写资格照旧排他：第二个写者被拒（修复没有放松"单一写者"）。
    assert_eq!(
        Environment::open(&env_dir, agent_a())
            .expect_err("写者只能有一个")
            .as_str(),
        "already-open"
    );
    drop(writer);
}

/// 有暂停记录的环境：诊断必须如实报告暂停**原因**，而且诊断本身不解除它。
#[test]
fn a_paused_environment_is_reported_as_paused_and_stays_paused() {
    let root = TempDir::new("diag-paused");
    let paused = spawn_worker("pause-by-real-failure", root.path());
    assert!(
        paused.status.success(),
        "制造保存失败应成功：{}",
        String::from_utf8_lossy(&paused.stderr)
    );

    let env_dir = root_env_of(root.path());
    let before = snapshot(&env_dir);

    let listing = run_diagnose(&[
        env_dir.to_string_lossy().into_owned(),
        "agent-alpha".to_string(),
    ]);
    assert_eq!(listing.status.code(), Some(0));
    let text = String::from_utf8_lossy(&listing.stdout).into_owned();
    assert_eq!(field_of(&text, "admission"), "paused");
    assert_eq!(
        field_of(&text, "admission-reason"),
        "save-failed:run-start",
        "暂停原因来自那条真实记录"
    );
    assert_eq!(field_of(&text, "paused"), "true");

    // 诊断不解除暂停、也不追加任何记录：字节与目录成员都不变。
    let after = snapshot(&env_dir);
    assert_eq!(before, after, "只读诊断不得改变环境");

    // 再读一次：仍然暂停（没有"读一次就把标记清了"这种事）。
    let again = run_diagnose(&[
        env_dir.to_string_lossy().into_owned(),
        "agent-alpha".to_string(),
    ]);
    assert_eq!(
        field_of(&String::from_utf8_lossy(&again.stdout), "admission"),
        "paused"
    );

    // 单活动详情：这个活动的操作事实仍然可解释（终止事实在盘上）。
    let detail = run_diagnose(&[
        env_dir.to_string_lossy().into_owned(),
        "agent-alpha".to_string(),
        "--run".to_string(),
        "run-pause".to_string(),
    ]);
    assert_eq!(detail.status.code(), Some(0));
    assert_eq!(
        field_of(&String::from_utf8_lossy(&detail.stdout), "outcome"),
        "subject:completed"
    );
}

// ---------------------------------------------------------------------------
// 6. 调用链审查：诊断路径上没有执行入口（静态、文本级）
// ---------------------------------------------------------------------------

/// 诊断模块与诊断命令里不得出现**执行入口**的引用。
///
/// 这条检查证明的是**窄条件**："这两个文件没有引用唯一那条执行循环，也没有引用模型端口或
/// 行动端口"。它不是链接期证明，也不代替运行时证据（"这一次调用没改盘上状态"由文件对照
/// 那一组负责）。三类证据各自证明什么，见本文件开头的说明。
///
/// 检查名单里**故意不含** `&mut Environment`：写侧限制由类型系统保证（诊断函数只收
/// 不可变借用），把它做成文本检查只会惩罚文档里正常的说明文字——那正是"守卫退化成约束
/// 用词"的老毛病。
#[test]
fn the_diagnosis_path_has_no_execution_entry_point() {
    const EXECUTION_ENTRY_POINTS: [&str; 5] = [
        "run_activity",
        "ModelPort",
        "ActionPort",
        "CommitSeam",
        "EnvironmentCommit",
    ];

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for name in ["diagnostics.rs", "bin/environment_diagnose.rs"] {
        let source = std::fs::read_to_string(root.join(name))
            .unwrap_or_else(|error| panic!("读不到 {name}: {error}"));
        let hits: Vec<&str> = EXECUTION_ENTRY_POINTS
            .iter()
            .copied()
            .filter(|marker| source.contains(marker))
            .collect();
        assert!(hits.is_empty(), "{name} 出现了执行入口记号: {hits:?}");
    }

    // 自检：同一套判断面对明确的违规样本必须报出违规，否则它可能是永久绿的。
    let sample =
        "run_activity_with(&mut run_agent, request, &mut model, &mut actions, &mut ledger);";
    let hits: Vec<&str> = EXECUTION_ENTRY_POINTS
        .iter()
        .copied()
        .filter(|marker| sample.contains(marker))
        .collect();
    assert_eq!(hits, vec!["run_activity"], "自检样本必须被判为违规");
}

// ---------------------------------------------------------------------------
// 夹具与工具
// ---------------------------------------------------------------------------

fn action_id() -> celira_agent::core::ActionId {
    celira_agent::core::ActionId::new("work")
}

/// 每次都明确成功的执行端。
struct SuccessActions;

impl ActionPort for SuccessActions {
    fn capabilities(&self, _agent: &AgentId, _run: &RunId) -> Capabilities {
        Capabilities::none()
    }

    // 准入策略：本替身**不**模拟授权判断，一律接纳——它的用途不是检验这一层。
    fn admit(&self, _agent: &AgentId, _run: &RunId, _action: &Action) -> ActionAdmission {
        ActionAdmission::Admitted
    }

    fn execute(
        &mut self,
        _agent: &AgentId,
        _operation: &OperationId,
        _action: &Action,
    ) -> ActionOutcome {
        ActionOutcome::success(vec![])
    }
}

/// 按脚本作答的模型。
struct ScriptedModel {
    script: std::collections::VecDeque<ModelOutput>,
}

impl ScriptedModel {
    fn new(script: Vec<ModelOutput>) -> Self {
        Self {
            script: std::collections::VecDeque::from(script),
        }
    }
}

impl ModelPort for ScriptedModel {
    fn complete(&mut self, _view: &View) -> Result<ModelOutput, ModelError> {
        self.script.pop_front().ok_or(scripted_model_failure())
    }
}

/// 环境里全部活动（事实侧与经历侧的并集，按机械的首次出现顺序）。
fn activity_runs(env: &Environment) -> Vec<RunId> {
    let mut runs: Vec<RunId> = Vec::new();
    for row in env.facts().expect("读回事实") {
        let run = RunId::new(row.run());
        if !runs.contains(&run) {
            runs.push(run);
        }
    }
    for record in env.trace().expect("读回经历") {
        let run = record.run().clone();
        if !runs.contains(&run) {
            runs.push(run);
        }
    }
    runs
}

/// 打开一个环境只为了读一次（写完即放）。
fn copied_env(dir: &Path) -> Environment {
    Environment::open(dir, agent_a()).expect("打开环境")
}

/// 目录快照：全部普通文件的**名字与字节**，按名字排序。
///
/// 两次快照相等 = 诊断既没有改内容，也没有增删目录成员。
fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .expect("读目录")
        .map(|entry| entry.expect("目录项可读"))
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let bytes = std::fs::read(entry.path()).unwrap_or_default();
            (name, bytes)
        })
        .collect();
    entries.sort();
    entries
}

fn spawn_worker(role: &str, root: &Path) -> std::process::Output {
    Command::new(binary("environment_worker"))
        .env(ROLE_ENV, role)
        .env(ROOT_ENV, root)
        .output()
        .expect("启动夹具进程")
}

fn run_diagnose(arguments: &[String]) -> std::process::Output {
    Command::new(binary("environment_diagnose"))
        .args(arguments)
        .output()
        .expect("启动诊断命令")
}

/// 从输出里取一条带前缀的报告行。
fn report_line(output: &std::process::Output, prefix: &str) -> String {
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.lines()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("没有 {prefix} 报告行: {text}"))
        .to_string()
}

/// 第一行以该前缀开头的行。
fn first_line(text: &str, prefix: &str) -> String {
    text.lines()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("没有以 {prefix} 开头的行: {text}"))
        .to_string()
}

/// 从夹具的 `SEED_REPORT:facts=<n>|trace=<n>` 里取一个计数。
fn seed_count(report: &str, key: &str) -> usize {
    let body = report
        .strip_prefix("SEED_REPORT:")
        .unwrap_or_else(|| panic!("不是种子报告: {report}"));
    body.split('|')
        .find_map(|field| field.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("种子报告里没有 {key}: {report}"))
        .parse()
        .expect("计数必须是数字")
}

/// 在一行（或整段文本）里取某个 `键=值` 的值。
fn field_of(text: &str, key: &str) -> String {
    let needle = format!("{key}=");
    for line in text.lines() {
        for field in line.split(' ') {
            if let Some(value) = field.strip_prefix(&needle) {
                return value.to_string();
            }
        }
    }
    panic!("没有字段 {key}: {text}");
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!("celira-task03-{label}-{}-{n}", std::process::id()));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("清掉同名残留目录");
        }
        std::fs::create_dir_all(&path).expect("建立测试目录");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// 这一份用例里的"模型失败"分类：脚本用尽 = 这次响应没有履约。
///
/// 这是**测试替身**的失败，不是某个真实服务行为；它只用来把"失败分类"这条路径走通。
/// 生产路径的分类由适配层按九类如实给出。
const SCRIPTED_MODEL_FAILURE: ModelFailureClass = ModelFailureClass::ContractViolation;

/// 这一份用例里的"模型失败"（带上上面那个分类）。
fn scripted_model_failure() -> ModelError {
    ModelError {
        class: SCRIPTED_MODEL_FAILURE,
    }
}
