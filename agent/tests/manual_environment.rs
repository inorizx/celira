//! OS 第一条环境来源的离线证明：手动来源只提供一条机械事实，其余一切不变。
//!
//! 全部离线：不联网、不需要密钥，只用自建临时沙盒与合成内容。
//!
//! 这里验证的是**机械事实的存在与次数**，不验证模型如何理解它、也不断言"下一步该做什么"。

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

mod common;

use celira_agent::activity_origin::{ACTIVITY_ORIGIN_LABEL, ActivityOrigin, MANUAL_ORIGIN_TEXT};
use celira_agent::composition::{run_read_only_files, run_read_only_files_manually};
use celira_agent::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Agent,
    Capabilities, ContentBatch, ContentBlock, InjectionStage, Limits, ModelError,
    ModelFailureClass, ModelOutput, ModelPort, NoCommit, RunInput, TraceEntry, View, no_cost,
    run_activity_with,
};
use celira_agent::file_app::FILE_APP_ID;
use celira_agent::foundation::{
    AgentId, Ledger, OperationId, Outcome, Record, RunId, SubjectEnding, SystemEnding, Turn,
};
use celira_agent::os::{MinimalOs, RunExpansion};
use common::InspectsFirst;

/// 自建临时沙盒：只在系统临时目录下创建，用完自己删掉。
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let unique = format!(
            "celira-manual-{}-{}-{}",
            std::process::id(),
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("时钟可用")
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        fs::create_dir_all(&root).expect("可以建临时沙盒");
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn write(&self, relative: &str, text: &str) {
        fs::write(self.root.join(relative), text).expect("可以写合成文件");
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// 只读文件能力的两个标识（由 App 声明；测试按名字引用）。
const LIST: &str = "files_list";

fn expand() -> Action {
    Action::with_params(
        ActionId::new("open_app"),
        ActionParams::none().with("target", FILE_APP_ID),
    )
}

/// 数自己被调用多少次、并记录每次看到的轨迹长度。
struct ScriptedModel {
    script: VecDeque<ModelOutput>,
    seen: Vec<(u32, usize)>,
}

impl ScriptedModel {
    fn new(script: Vec<ModelOutput>) -> Self {
        Self {
            script: VecDeque::from(script),
            seen: Vec::new(),
        }
    }
}

impl ModelPort for ScriptedModel {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        self.seen.push((view.turn().number(), view.trace().len()));
        Ok(self.script.pop_front().expect("脚本对每一次调用都有作答"))
    }
}

/// 一直失败的模型。
struct FailingModel;

impl ModelPort for FailingModel {
    fn complete(&mut self, _view: &View) -> Result<ModelOutput, ModelError> {
        Err(scripted_model_failure())
    }
}

/// 永远返回"无法确认"的执行端。
struct UnknownActions;

impl ActionPort for UnknownActions {
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
        ActionOutcome::unknown()
    }
}

/// 不该被执行的执行端：能力查询空，执行一旦发生就 panic。
struct UntouchedActions;

impl ActionPort for UntouchedActions {
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
        panic!("这条路径不该执行任何动作");
    }
}

/// 轨迹里注入内容的（阶段, 轮次, 文本）。
fn injected(run: &celira_agent::core::Run) -> Vec<(InjectionStage, Option<u32>, Vec<String>)> {
    run.trace()
        .iter()
        .filter_map(|entry| match entry {
            TraceEntry::Injection { stage, turn, batch } => Some((
                *stage,
                turn.map(Turn::number),
                batch
                    .blocks()
                    .iter()
                    .map(|block| block.text().to_string())
                    .collect::<Vec<_>>(),
            )),
            TraceEntry::Refused { .. }
            | TraceEntry::Observation(_)
            | TraceEntry::ModelResponse(_) => None,
        })
        .collect()
}

/// 按给定脚本跑一次手动环境活动，返回（活动, 模型看到的轮次/轨迹长度, 环境来源）。
fn run_with_environment(
    sandbox: &Path,
    script: Vec<ModelOutput>,
) -> (celira_agent::core::Run, Vec<(u32, usize)>) {
    let mut environment = ActivityOrigin::new();
    let mut agent = Agent::new(AgentId::new("agent-manual"));
    let mut ledger = Ledger::new(AgentId::new("agent-manual"));
    let mut model = InspectsFirst::new(ScriptedModel::new(script));
    let mut os = MinimalOs::open(sandbox).expect("可以装配");
    let mut expansion = RunExpansion::new(AgentId::new("agent-manual"), RunId::new("run-manual"));
    let mut scope = os.scope(&mut expansion);
    let run = run_activity_with(
        &mut agent,
        celira_agent::core::RunRequest {
            run: RunId::new("run-manual"),
            // TASK_69：预算是"承认先看格式要走两轮"（开场入场能力一批、展开后新能力又一批），
            // 不是放宽校验——脚本里那三轮真实模型调用仍需各占一轮。
            limits: Limits::new(5),
            input: RunInput::empty(),
            source: &mut environment,
            commit: &mut NoCommit,
            cost: &no_cost,
        },
        &mut model,
        &mut scope,
        &mut ledger,
    )
    .expect("手动环境活动可以跑完");
    (run, model.inner().seen.clone())
}

/// 1. Run 级恰好注入一次；Turn 级按模型调用次数发生，但空批次不新增记录。
#[test]
fn the_manual_source_injects_once_and_empty_turn_batches_add_nothing() {
    let sandbox = Sandbox::new("once");
    sandbox.write("alpha.txt", "alpha");

    // 三轮模型调用：展开 → list → Finish。
    let (run, seen) = run_with_environment(
        sandbox.path(),
        vec![
            ModelOutput::act(expand()),
            ModelOutput::act(Action::with_params(
                ActionId::new(LIST),
                ActionParams::none(),
            )),
            ModelOutput::finish(SubjectEnding::Completed),
        ],
    );

    // 三轮**真实模型调用**（展开 → list → Finish）；TASK_69 的"先看格式"由外层包装补，
    // 它不占内层模型的调用次数。
    assert_eq!(seen.len(), 3, "三次真实模型调用");
    assert_eq!(
        seen.iter().map(|(turn, _)| *turn).collect::<Vec<_>>(),
        vec![2, 4, 5],
        "轮次号：内层模型在第 2/4/5 拍各被调用一次（第 1 拍与第 3 拍是开场先看格式）"
    );

    let injections = injected(&run);
    assert_eq!(
        injections.len(),
        1,
        "Turn 级返回空批次，不产生任何注入记录: {injections:?}"
    );
    assert_eq!(injections[0].0, InjectionStage::RunStart);
    assert_eq!(injections[0].1, None, "Run 级注入没有轮次");
    assert_eq!(
        injections[0].2,
        vec![MANUAL_ORIGIN_TEXT.to_string()],
        "内容就是那条机械事实"
    );

    // 轨迹 = Run 注入(1) + 每一轮一条模型响应 + 每个**执行过**的动作一条结果。
    //
    // TASK_69：主体开场"先看格式"那几轮也在轨迹里（每条 `inspect_action` 都真的执行过），
    // 所以这里写成**结构恒等式**，而不是写死条数——写死的条数会随轮数变，恒等式不会。
    let responses = run
        .trace()
        .iter()
        .filter(|entry| matches!(entry, TraceEntry::ModelResponse(_)))
        .count();
    let observations = run
        .trace()
        .iter()
        .filter(|entry| matches!(entry, TraceEntry::Observation(_)))
        .count();
    assert_eq!(
        run.trace().len(),
        1 + responses + observations,
        "轨迹 = Run 注入(1) + 每拍一条模型响应 + 每个执行过的动作一条结果"
    );
    // 只看**业务行动**：两次（展开、list）——"先看格式"那几轮不是业务行动。
    assert_eq!(
        run.trace()
            .iter()
            .filter(|entry| matches!(
                entry,
                TraceEntry::Observation(observation)
                    if observation.action().as_str() != "inspect_action"
            ))
            .count(),
        2,
        "只有两次业务行动产生结果"
    );
    assert_eq!(
        run.trace()
            .iter()
            .filter(|entry| matches!(
                entry,
                TraceEntry::Injection {
                    stage: InjectionStage::RunStart,
                    ..
                }
            ))
            .count(),
        1,
        "Run 注入恰好一次"
    );
}

/// 2. 首轮与后续视野都保留来源事实，原文只出现一次、顺序稳定。
#[test]
fn the_origin_fact_is_visible_in_every_turn_exactly_once_and_in_order() {
    let sandbox = Sandbox::new("visible");
    sandbox.write("alpha.txt", "alpha");

    let (run, seen) = run_with_environment(
        sandbox.path(),
        vec![
            ModelOutput::act(Action::new(ActionId::new("probe"))),
            ModelOutput::finish(SubjectEnding::Completed),
        ],
    );

    // 内层模型只被调用两次（probe、Finish），轮次与轨迹长度都只往前长。
    //
    // TASK_69：条数不再是 (1, 3)——开场"先看格式"那 5 拍也在轨迹里，所以这里守的是
    // **轮次递增 + 轨迹长度严格递增**（"每一轮都看得到来源事实"由下面那份文本清单去证）。
    let turns: Vec<u32> = seen.iter().map(|(turn, _)| *turn).collect();
    assert!(
        turns.windows(2).all(|pair| pair[0] < pair[1]),
        "轮次递增: {turns:?}"
    );
    let lengths: Vec<usize> = seen.iter().map(|(_, length)| *length).collect();
    assert!(
        lengths.windows(2).all(|pair| pair[0] < pair[1]),
        "轨迹只在前缀上按发生顺序增加: {lengths:?}"
    );

    let texts: Vec<String> = run
        .trace()
        .iter()
        .flat_map(|entry| match entry {
            TraceEntry::Injection { batch, .. } => batch
                .blocks()
                .iter()
                .map(|block| block.text().to_string())
                .collect::<Vec<_>>(),
            TraceEntry::Refused { .. } => Vec::new(),
            // 模型响应不是结果：它的正文由结构化历史承载，不混进这份结果文本。
            TraceEntry::ModelResponse(_) => Vec::new(),
            TraceEntry::Observation(observation) => observation
                .outcome()
                .content()
                .iter()
                .map(|block| block.text().to_string())
                .collect::<Vec<_>>(),
        })
        .collect();
    assert_eq!(
        texts
            .iter()
            .filter(|text| *text == MANUAL_ORIGIN_TEXT)
            .count(),
        1,
        "原文恰好一次: {texts:?}"
    );
    assert_eq!(texts[0], MANUAL_ORIGIN_TEXT, "来源事实排在轨迹最前");
}

/// 3. 空 Turn 批次不触发额外动作或额外模型调用。
#[test]
fn empty_turn_batches_trigger_nothing_extra() {
    let sandbox = Sandbox::new("no-extra");
    sandbox.write("alpha.txt", "alpha");

    // 单轮 Finish：模型只被调用一次，动作从不发生。
    let mut environment = ActivityOrigin::new();
    let mut agent = Agent::new(AgentId::new("agent-manual"));
    let mut ledger = Ledger::new(AgentId::new("agent-manual"));
    let mut model = InspectsFirst::new(ScriptedModel::new(vec![ModelOutput::finish(
        SubjectEnding::Silent,
    )]));
    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");
    let mut expansion2 = RunExpansion::new(AgentId::new("agent-manual"), RunId::new("run-manual"));
    let _scope = os.scope(&mut expansion2);
    let run = run_activity_with(
        &mut agent,
        celira_agent::core::RunRequest {
            run: RunId::new("run-manual"),
            limits: Limits::new(3),
            input: RunInput::empty(),
            source: &mut environment,
            commit: &mut NoCommit,
            cost: &no_cost,
        },
        &mut model,
        &mut UntouchedActions,
        &mut ledger,
    )
    .expect("可以跑完");

    assert_eq!(run.outcome(), Some(Outcome::Subject(SubjectEnding::Silent)));
    assert_eq!(model.inner().seen.len(), 1, "不得因为空批次追加调用");
    assert_eq!(injected(&run).len(), 1, "只有 Run 级那一条");
}

/// 4. 有初始输入时顺序不变：输入 → Run 注入 → 行动结果。
#[test]
fn an_initial_input_still_comes_before_the_origin_fact() {
    let sandbox = Sandbox::new("input-order");
    sandbox.write("alpha.txt", "alpha");

    let marker = "caller-provided-content";
    let mut environment = ActivityOrigin::new();
    let mut agent = Agent::new(AgentId::new("agent-manual"));
    let mut ledger = Ledger::new(AgentId::new("agent-manual"));
    let mut model = InspectsFirst::new(ScriptedModel::new(vec![ModelOutput::finish(
        SubjectEnding::Silent,
    )]));
    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");
    let mut expansion3 = RunExpansion::new(AgentId::new("agent-manual"), RunId::new("run-manual"));
    let mut scope = os.scope(&mut expansion3);
    let run = run_activity_with(
        &mut agent,
        celira_agent::core::RunRequest {
            run: RunId::new("run-manual"),
            limits: Limits::new(2),
            input: RunInput::text(marker).expect("输入在上限内"),
            source: &mut environment,
            commit: &mut NoCommit,
            cost: &no_cost,
        },
        &mut model,
        &mut scope,
        &mut ledger,
    )
    .expect("可以跑完");

    assert_eq!(run.input().as_str(), marker, "初始输入原文不变");
    assert_eq!(
        injected(&run).len(),
        1,
        "环境来源不复制初始输入，只给一条自己的事实"
    );
    assert!(
        !injected(&run)[0].2.iter().any(|text| text == marker),
        "来源内容与初始输入互相独立"
    );
    // 事实次序：Run 注入排在轨迹最前，初始输入不占轨迹条目。
    assert!(matches!(
        run.trace()[0],
        TraceEntry::Injection {
            stage: InjectionStage::RunStart,
            ..
        }
    ));
}

/// 5. 既有失败路径不改变接缝次数与终态。
#[test]
fn the_existing_failure_paths_are_unchanged() {
    let sandbox = Sandbox::new("failures");
    sandbox.write("alpha.txt", "alpha");

    // (a) 活动槽被占用：来源零调用、零事实。
    let mut agent = Agent::new(AgentId::new("agent-manual"));
    let held = celira_agent::core::Run::begin(&mut agent, RunId::new("run-held"), Limits::new(2))
        .expect("先占住活动槽");
    let mut ledger = Ledger::new(AgentId::new("agent-manual"));
    let mut environment = ActivityOrigin::new();
    let mut model = InspectsFirst::new(ScriptedModel::new(vec![]));
    let refused = run_activity_with(
        &mut agent,
        celira_agent::core::RunRequest {
            run: RunId::new("run-manual"),
            limits: Limits::new(2),
            input: RunInput::empty(),
            source: &mut environment,
            commit: &mut NoCommit,
            cost: &no_cost,
        },
        &mut model,
        &mut UntouchedActions,
        &mut ledger,
    );
    assert!(refused.is_err(), "活动槽已占用必须被拒绝");
    assert_eq!(agent.activity(), Some(held.id()), "既有占用不被破坏");
    assert!(ledger.records().is_empty(), "被拒绝时不得写任何事实");
    assert!(model.inner().seen.is_empty(), "被拒绝时不得调用模型");

    // (b) 零预算：连 Run 注入都不该发生。
    let mut agent = Agent::new(AgentId::new("agent-zero"));
    let mut ledger = Ledger::new(AgentId::new("agent-zero"));
    let mut environment = ActivityOrigin::new();
    let zero = run_activity_with(
        &mut agent,
        celira_agent::core::RunRequest {
            run: RunId::new("run-zero"),
            limits: Limits::new(0),
            input: RunInput::empty(),
            source: &mut environment,
            commit: &mut NoCommit,
            cost: &no_cost,
        },
        &mut ScriptedModel::new(vec![]),
        &mut UntouchedActions,
        &mut ledger,
    );
    assert_eq!(
        zero.unwrap_err(),
        celira_agent::core::RunError::NoTurnBudget
    );
    assert!(ledger.records().is_empty());

    // (c) 模型失败：Run 注入仍然恰好一次，终态是系统侧失败。
    // 这里不经过组合层，所以直接给出活动、账本与展开作用域；展开作用域本用例不需要用到。
    let mut environment = ActivityOrigin::new();
    let mut agent = Agent::new(AgentId::new("agent-model-error"));
    let mut ledger = Ledger::new(AgentId::new("agent-model-error"));
    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");
    let mut expansion_model_error = RunExpansion::new(
        AgentId::new("agent-model-error"),
        RunId::new("run-model-error"),
    );
    let _scope = os.scope(&mut expansion_model_error);
    let run = run_activity_with(
        &mut agent,
        celira_agent::core::RunRequest {
            run: RunId::new("run-model-error"),
            limits: Limits::new(3),
            input: RunInput::empty(),
            source: &mut environment,
            commit: &mut NoCommit,
            cost: &no_cost,
        },
        &mut FailingModel,
        &mut UntouchedActions,
        &mut ledger,
    )
    .expect("可以跑完");
    assert_eq!(
        run.outcome(),
        Some(Outcome::System(SystemEnding::ModelFailed(Some(
            SCRIPTED_MODEL_FAILURE.as_str()
        ))))
    );
    assert_eq!(injected(&run).len(), 1, "模型失败不改接缝次数");

    // (d) 回执未达：同样只有一次 Run 注入；活动不再由系统收场，主体自己决定。
    let mut environment = ActivityOrigin::new();
    let mut agent = Agent::new(AgentId::new("agent-unknown"));
    let mut ledger = Ledger::new(AgentId::new("agent-unknown"));
    let mut model = InspectsFirst::new(ScriptedModel::new(vec![
        ModelOutput::act(Action::new(ActionId::new("probe"))),
        ModelOutput::finish(SubjectEnding::Silent),
    ]));
    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");
    let mut expansion4 =
        RunExpansion::new(AgentId::new("agent-unknown"), RunId::new("run-unknown"));
    let _scope = os.scope(&mut expansion4);
    let run = run_activity_with(
        &mut agent,
        celira_agent::core::RunRequest {
            run: RunId::new("run-unknown"),
            limits: Limits::new(3),
            input: RunInput::empty(),
            source: &mut environment,
            commit: &mut NoCommit,
            cost: &no_cost,
        },
        &mut model,
        &mut UnknownActions,
        &mut ledger,
    )
    .expect("可以跑完");
    assert_eq!(
        run.outcome(),
        Some(Outcome::Subject(SubjectEnding::Silent)),
        "收场是主体自己说的"
    );
    assert_eq!(injected(&run).len(), 1);
    assert_eq!(
        model.inner().seen.len(),
        2,
        "没看见之后主体还得有一次机会（读数已经在视野里）"
    );
}

/// 6. 不同活动各自独立：来源文本不跨活动自动复制。
#[test]
fn different_runs_do_not_share_the_origin_fact_automatically() {
    let sandbox = Sandbox::new("independence");
    sandbox.write("alpha.txt", "alpha");

    // 第一次活动用环境来源；第二次活动用空来源（旧入口语义）。
    let mut agent = Agent::new(AgentId::new("agent-manual"));
    let mut ledger = Ledger::new(AgentId::new("agent-manual"));

    let first = run_with_environment(
        sandbox.path(),
        vec![ModelOutput::finish(SubjectEnding::Silent)],
    );
    assert_eq!(injected(&first.0).len(), 1);

    // 空来源活动：轨迹里没有任何注入——上一条事实没有被继承。
    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");
    let mut expansion5 = RunExpansion::new(AgentId::new("agent-manual"), RunId::new("run-second"));
    let mut scope = os.scope(&mut expansion5);
    let mut model = InspectsFirst::new(ScriptedModel::new(vec![ModelOutput::finish(
        SubjectEnding::Silent,
    )]));
    let second = run_activity_with(
        &mut agent,
        celira_agent::core::RunRequest {
            run: RunId::new("run-second"),
            limits: Limits::new(2),
            input: RunInput::empty(),
            source: &mut celira_agent::core::NoContext,
            commit: &mut NoCommit,
            cost: &no_cost,
        },
        &mut model,
        &mut scope,
        &mut ledger,
    )
    .expect("第二次活动可以跑完");
    assert!(
        injected(&second).is_empty(),
        "空来源活动不得继承上一次的来源事实"
    );
    // 这次活动只登记一次开始、只收场一次。
    //
    // TASK_69：不能再数 `records().len()`——主体开场"先看格式"那几轮每个动作都会留下
    // 准备/派发/结果三条事实，条数与"几次活动"无关。本判据要问的是**登记与收场的次数**。
    let started = ledger
        .records()
        .iter()
        .filter(|record| matches!(record, Record::RunStarted { .. }))
        .count();
    let ended = ledger
        .records()
        .iter()
        .filter(|record| matches!(record, Record::RunEnded { .. }))
        .count();
    assert_eq!(
        (started, ended),
        (1, 1),
        "这次活动只登记一次开始、只收场一次"
    );

    // 同一个来源对象用于两次活动：每次都重新给一次，不累计、不缓存。
    let mut shared = ActivityOrigin::new();
    let mut texts = Vec::new();
    for index in 0..2 {
        let mut agent = Agent::new(AgentId::new(format!("agent-{index}")));
        let mut ledger = Ledger::new(AgentId::new(format!("agent-{index}")));
        let mut model = InspectsFirst::new(ScriptedModel::new(vec![ModelOutput::finish(
            SubjectEnding::Silent,
        )]));
        let run = run_activity_with(
            &mut agent,
            celira_agent::core::RunRequest {
                run: RunId::new(format!("run-{index}")),
                limits: Limits::new(2),
                input: RunInput::empty(),
                source: &mut shared,
                commit: &mut NoCommit,
                cost: &no_cost,
            },
            &mut model,
            &mut UntouchedActions,
            &mut ledger,
        )
        .expect("可以跑完");
        texts.push(injected(&run).len());
    }
    assert_eq!(texts, vec![1, 1], "每次活动各自拿到一次来源事实");
}

/// 7. 组合入口跑通，且没有第二条循环。
#[test]
fn the_composition_entry_runs_the_same_core() {
    let sandbox = Sandbox::new("composition");
    sandbox.write("alpha.txt", "alpha");

    // 手动环境入口：Run 级事实进轨迹。
    let mut model = InspectsFirst::new(ScriptedModel::new(vec![ModelOutput::finish(
        SubjectEnding::Silent,
    )]));
    let manual = run_read_only_files_manually(
        sandbox.path(),
        AgentId::new("agent-manual"),
        RunId::new("run-manual"),
        Limits::new(2),
        &mut model,
    )
    .expect("手动入口可用");
    assert_eq!(injected(&manual.run).len(), 1);
    assert_eq!(
        manual.run.trace()[0],
        TraceEntry::Injection {
            stage: InjectionStage::RunStart,
            turn: None,
            batch: ContentBatch::of(vec![ContentBlock::new(
                ACTIVITY_ORIGIN_LABEL,
                MANUAL_ORIGIN_TEXT
            )])
            .expect("这批内容在上限内"),
        },
        "组合入口装配的正是那条机械事实"
    );

    // 旧入口：空来源，行为不变（轨迹里仍然只有模型自己的响应，没有任何注入）。
    let mut old = InspectsFirst::new(ScriptedModel::new(vec![ModelOutput::finish(
        SubjectEnding::Silent,
    )]));
    let plain = run_read_only_files(
        sandbox.path(),
        AgentId::new("agent-plain"),
        RunId::new("run-plain"),
        Limits::new(2),
        &mut old,
    )
    .expect("旧入口可用");
    // 旧入口仍然**没有注入**——这条才是本判据的语义。
    //
    // TASK_69：原来那句 `trace().len() == 1` 是"只有一条模型响应"的意思；主体开场"先看格式"
    // 那几轮现在也在轨迹里，条数会随轮数变。改成说清**轨迹里只有什么**：
    // 模型自己的响应、它执行过的动作、以及没被接纳的请求——一条注入都没有。
    assert!(
        plain.run.trace().iter().all(|entry| matches!(
            entry,
            TraceEntry::ModelResponse(_) | TraceEntry::Observation(_) | TraceEntry::Refused { .. }
        )),
        "旧入口的轨迹里只有模型自己的响应与它执行过的动作，没有任何注入: {:?}",
        plain.run.trace()
    );
    assert!(
        !plain
            .run
            .trace()
            .iter()
            .any(|entry| matches!(entry, TraceEntry::Injection { .. })),
        "旧入口仍然没有注入"
    );
}

/// 8. 来源正文不进入 Foundation 的事实。
#[test]
fn the_origin_text_never_reaches_the_foundation_facts() {
    let sandbox = Sandbox::new("facts");
    sandbox.write("alpha.txt", "alpha");

    let mut environment = ActivityOrigin::new();
    let mut agent = Agent::new(AgentId::new("agent-manual"));
    let mut ledger = Ledger::new(AgentId::new("agent-manual"));
    let mut model = InspectsFirst::new(ScriptedModel::new(vec![ModelOutput::finish(
        SubjectEnding::Completed,
    )]));
    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");
    let mut expansion6 = RunExpansion::new(AgentId::new("agent-manual"), RunId::new("run-manual"));
    let mut scope = os.scope(&mut expansion6);
    let run = run_activity_with(
        &mut agent,
        celira_agent::core::RunRequest {
            run: RunId::new("run-manual"),
            limits: Limits::new(2),
            input: RunInput::empty(),
            source: &mut environment,
            commit: &mut NoCommit,
            cost: &no_cost,
        },
        &mut model,
        &mut scope,
        &mut ledger,
    )
    .expect("可以跑完");

    let facts = format!("{:?}", ledger.records());
    assert!(!facts.contains(MANUAL_ORIGIN_TEXT), "来源正文不得进入事实");
    assert!(
        !facts.contains(ACTIVITY_ORIGIN_LABEL),
        "来源标签也不得进入事实"
    );
    // 轨迹里仍然有它。
    assert_eq!(injected(&run).len(), 1);
}

/// 9. 来源是环境内容而不是系统契约：它只出现在运行事实里，标签不能改变角色。
#[test]
fn the_origin_is_environment_content_not_a_system_contract() {
    use celira_agent::provider_adapter::DeepSeekModel;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    // 本地回环的最小服务器：只答应答，记录收到的原始请求体。
    let listener = TcpListener::bind("127.0.0.1:0").expect("可以绑定回环端口");
    let port = listener.local_addr().expect("有地址").port();
    let endpoint = format!("http://127.0.0.1:{port}");
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().expect("可以克隆流"));
            let mut content_length = 0usize;
            let mut body = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if line.to_ascii_lowercase().starts_with("content-length:") {
                    content_length = line
                        .split(':')
                        .nth(1)
                        .and_then(|value| value.trim().parse().ok())
                        .unwrap_or(0);
                }
                if line == "\r\n" {
                    break;
                }
            }
            if content_length > 0 {
                let mut buffer = vec![0u8; content_length];
                if reader.read_exact(&mut buffer).is_ok() {
                    body = String::from_utf8_lossy(&buffer).to_string();
                }
            }
            // 一次正常的收场应答（**原生工具调用、不带参数**）。这条用例只看**请求**形状
            // （角色顺序），应答是什么都不影响判据——但夹具仍与线缆同形：正文里的控制 JSON
            // 早就不是通路了（TASK_08 阶段 B），把它留在这里会让读者以为它还是。
            let reply = r#"{"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"c1","type":"function","function":{"name":"finish_run","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.len(),
                reply
            );
            let _ = stream.flush();
            let _ = sender.send(body);
        }
    });

    let sandbox = Sandbox::new("wire");
    sandbox.write("alpha.txt", "alpha");
    let mut environment = ActivityOrigin::new();
    let mut agent = Agent::new(AgentId::new("agent-wire"));
    let mut ledger = Ledger::new(AgentId::new("agent-wire"));
    let mut model = DeepSeekModel::at_endpoint("synthetic-credential", "synthetic-model", endpoint)
        .expect("可以装配适配层");
    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");
    let mut expansion7 = RunExpansion::new(AgentId::new("agent-wire"), RunId::new("run-wire"));
    let mut scope = os.scope(&mut expansion7);
    run_activity_with(
        &mut agent,
        celira_agent::core::RunRequest {
            run: RunId::new("run-wire"),
            limits: Limits::new(1),
            input: RunInput::empty(),
            source: &mut environment,
            commit: &mut NoCommit,
            cost: &no_cost,
        },
        &mut model,
        &mut scope,
        &mut ledger,
    )
    .expect("可以跑完");

    let body = receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("服务器收到了请求");

    // 角色顺序：system 契约 → 来源（环境内容，非 system）→ 机械事实。
    // 来源不改变角色：它**是**一条普通的环境内容消息，不是系统契约。
    let roles: Vec<String> = body
        .split("\"role\":\"")
        .skip(1)
        .map(|tail| tail.split('"').next().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        roles,
        vec!["system", "user", "user"],
        "system 契约 + 来源内容 + 机械事实，三者角色固定: {body}"
    );

    // 机械事实只报**条目数**，不再把每一条条目摊平成正文——历史由结构化重建承担。
    assert!(body.contains("entries = 1"), "机械事实报的是条目数: {body}");
    assert_eq!(
        body.matches(MANUAL_ORIGIN_TEXT).count(),
        1,
        "来源原文恰好一次: {body}"
    );
    // system 契约里没有来源正文。
    let system_part = body
        .split("\"role\":\"user\"")
        .next()
        .expect("有 system 段");
    assert!(
        !system_part.contains(MANUAL_ORIGIN_TEXT),
        "来源不得进入 system 契约"
    );
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
