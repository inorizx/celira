//! Run / Turn 两个上下文注入接缝的离线证明。
//!
//! 全部离线：不联网、不需要密钥、只用合成中立数据。这里不实现通知或闹钟，只用合成内容验证接缝。

use std::collections::VecDeque;

use celira_agent::composition::run_read_only_files;
use celira_agent::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Agent,
    Capabilities, ContentBatch, ContentBatchError, ContentBlock, ContextSource, Decision,
    InjectionContext, InjectionStage, Limits, ModelError, ModelFailureClass, ModelOutput,
    ModelPort, NoCommit, NoContext, RunInput, RunRequest, TraceEntry, View, no_cost,
    run_activity_with,
};
use celira_agent::file_app::FILE_APP_ID;
use celira_agent::foundation::{
    AgentId, Ledger, OperationId, Outcome, ResultState, RunId, SubjectEnding, SystemEnding, Turn,
};
use celira_agent::os::{MinimalOs, RunExpansion};

/// 造一批注入内容。
fn batch(label: &str, text: &str) -> ContentBatch {
    ContentBatch::of(vec![ContentBlock::new(label, text)]).expect("这批内容在上限内")
}

fn expand() -> Action {
    Action::with_params(
        ActionId::new("open_app"),
        ActionParams::none().with("target", FILE_APP_ID),
    )
}

fn listing() -> Action {
    Action::with_params(ActionId::new("files_list"), ActionParams::none())
}

/// 会记账的来源：Run 级一次、Turn 级按轮次给内容，并记录每次被调用时的真实身份。
struct RecordingSource {
    run_start: ContentBatch,
    turns: VecDeque<ContentBatch>,
    /// (阶段, Agent, Run, Turn)
    calls: Vec<(String, String, String, Option<u32>)>,
}

impl RecordingSource {
    fn new(run_start: ContentBatch, turns: Vec<ContentBatch>) -> Self {
        Self {
            run_start,
            turns: VecDeque::from(turns),
            calls: Vec::new(),
        }
    }

    fn record(&mut self, stage: &str, context: &InjectionContext) {
        self.calls.push((
            stage.to_string(),
            context.agent().as_str().to_string(),
            context.run().as_str().to_string(),
            context.turn().map(Turn::number),
        ));
    }
}

impl ContextSource for RecordingSource {
    fn on_run_start(&mut self, context: &InjectionContext) -> ContentBatch {
        self.record("run-start", context);
        self.run_start.clone()
    }

    fn before_turn(&mut self, context: &InjectionContext) -> ContentBatch {
        self.record("turn", context);
        self.turns.pop_front().unwrap_or_else(ContentBatch::empty)
    }
}

/// 按脚本作答的模型，同时记录每次调用看到的轨迹。
struct ScriptedModel {
    script: VecDeque<ModelOutput>,
    /// 每次调用看到的（轮次, 轨迹条数, 轨迹里的全部文本）。
    seen: Vec<(u32, usize, Vec<String>)>,
}

impl ScriptedModel {
    fn new(script: Vec<ModelOutput>) -> Self {
        Self {
            script: VecDeque::from(script),
            seen: Vec::new(),
        }
    }
}

fn trace_texts(view: &View) -> Vec<String> {
    view.trace()
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
        .collect()
}

impl ModelPort for ScriptedModel {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        self.seen
            .push((view.turn().number(), view.trace().len(), trace_texts(view)));
        Ok(self.script.pop_front().expect("脚本对每一次调用都有作答"))
    }
}

/// 数调用次数的模型。
struct CountingModel {
    calls: usize,
}

impl ModelPort for CountingModel {
    fn complete(&mut self, _view: &View) -> Result<ModelOutput, ModelError> {
        self.calls += 1;
        Ok(ModelOutput::finish(SubjectEnding::Silent))
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

/// 会失败一次的执行端（第一次 Failure，之后 Success）。
struct FailingOnceActions {
    calls: usize,
}

impl ActionPort for FailingOnceActions {
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
        self.calls += 1;
        if self.calls == 1 {
            ActionOutcome::failure("not-found")
        } else {
            ActionOutcome::success(Vec::new())
        }
    }
}

/// 不该被调用的模型：被用到就 panic（用于证明"零模型调用"的路径）。
struct UnreachableModel;

impl ModelPort for UnreachableModel {
    fn complete(&mut self, _view: &View) -> Result<ModelOutput, ModelError> {
        panic!("这条路径不该调用模型");
    }
}

/// 不提供任何能力、也永远不该被真正执行的执行端。
///
/// 能力查询会正常发生（每一轮都会问当前可用能力），执行则一定会 panic——
/// 所以在"只关心钩子次数"的用例里它是安全的，在"必须零动作"的用例里它也会立刻暴露问题。
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

fn injection_texts(run: &celira_agent::core::Run) -> Vec<String> {
    run.trace()
        .iter()
        .filter_map(|entry| match entry {
            TraceEntry::Injection { batch, .. } => Some(
                batch
                    .blocks()
                    .iter()
                    .map(|b| b.text().to_string())
                    .collect::<Vec<_>>(),
            ),
            TraceEntry::Refused { .. }
            | TraceEntry::Observation(_)
            | TraceEntry::ModelResponse(_) => None,
        })
        .flatten()
        .collect()
}

/// 1. 空来源回归既有行为；非空来源：Run 钩子恰好一次，Turn 钩子次数等于实际模型调用次数。
#[test]
fn an_empty_source_changes_nothing_and_a_real_source_is_called_once_per_model_call() {
    let agent_id = AgentId::new("agent-a");
    let run_id = RunId::new("run-1");

    // 空来源：轨迹里只有行动结果，没有任何注入。
    let mut agent = Agent::new(agent_id.clone());
    let mut ledger = Ledger::new(agent_id.clone());
    let mut model = ScriptedModel::new(vec![
        ModelOutput::act(expand()),
        ModelOutput::finish(SubjectEnding::Completed),
    ]);
    let mut actions = FailingOnceActions { calls: 0 };
    let empty = run_activity_with(
        &mut agent,
        RunRequest {
            cost: &no_cost,
            run: run_id.clone(),
            limits: Limits::new(3),
            input: RunInput::empty(),
            source: &mut NoContext,
            commit: &mut NoCommit,
        },
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("空来源可以跑完");
    assert!(
        empty
            .trace()
            .iter()
            .all(|entry| !matches!(entry, TraceEntry::Injection { .. })),
        "空来源不该产生任何注入记录"
    );
    assert_eq!(injection_texts(&empty).len(), 0);

    // 非空来源：Run 钩子一次、Turn 钩子与模型调用次数相同。
    let mut agent = Agent::new(AgentId::new("agent-b"));
    let mut ledger = Ledger::new(AgentId::new("agent-b"));
    let mut source = RecordingSource::new(
        batch("opening", "run-opening-note"),
        vec![
            batch("turn-fact", "turn-1-fact"),
            batch("turn-fact", "turn-2-fact"),
        ],
    );
    let mut model = ScriptedModel::new(vec![
        ModelOutput::act(expand()),
        ModelOutput::act(listing()),
        ModelOutput::finish(SubjectEnding::Completed),
    ]);
    let mut actions = FailingOnceActions { calls: 0 };
    let run = run_activity_with(
        &mut agent,
        RunRequest {
            cost: &no_cost,
            run: RunId::new("run-2"),
            limits: Limits::new(4),
            input: RunInput::empty(),
            source: &mut source,
            commit: &mut NoCommit,
        },
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("带来源可以跑完");

    let stages: Vec<&str> = source.calls.iter().map(|(s, ..)| s.as_str()).collect();
    assert_eq!(
        stages,
        vec!["run-start", "turn", "turn", "turn"],
        "Run 钩子一次；Turn 钩子等于模型调用次数（3 次）"
    );
    assert_eq!(source.calls.len() - 1, model.seen.len());
    assert_eq!(
        source
            .calls
            .iter()
            .filter_map(|(stage, .., turn)| (stage == "turn").then_some(*turn))
            .collect::<Vec<_>>(),
        vec![Some(1), Some(2), Some(3)],
        "Turn 钩子拿到的是本轮真实序号"
    );
    assert_eq!(
        injection_texts(&run),
        vec![
            "run-opening-note".to_string(),
            "turn-1-fact".to_string(),
            "turn-2-fact".to_string(),
            "turn-3-fact".to_string() // 第三个批次没有内容 → 空批次
        ]
        .into_iter()
        .filter(|text| text != "turn-3-fact")
        .collect::<Vec<_>>(),
        "空批次不产生记录"
    );
}

/// 2. 唯一有序轨迹：前缀只增、原文各一次、顺序确定；重复构造视野不追加。
#[test]
fn the_trace_is_one_ordered_prefix_that_never_grows_by_reading_it() {
    let marker = "result-of-first-action";
    let mut agent = Agent::new(AgentId::new("agent-a"));
    let mut ledger = Ledger::new(AgentId::new("agent-a"));
    let mut source = RecordingSource::new(
        batch("opening", "opening-note"),
        vec![
            batch("turn-fact", "turn-1-fact"),
            batch("turn-fact", "turn-2-fact"),
        ],
    );

    /// 让第一次行动带回一段可辨认的内容。
    struct ContentActions;

    impl ActionPort for ContentActions {
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
            ActionOutcome::success(vec![ContentBlock::new("action", "result-of-first-action")])
        }
    }

    let mut model = ScriptedModel::new(vec![
        ModelOutput::act(Action::new(ActionId::new("probe"))),
        ModelOutput::finish(SubjectEnding::Completed),
    ]);
    let mut actions = ContentActions;
    let run = run_activity_with(
        &mut agent,
        RunRequest {
            cost: &no_cost,
            run: RunId::new("run-1"),
            limits: Limits::new(3),
            input: RunInput::text("input-text").expect("输入在上限内"),
            source: &mut source,
            commit: &mut NoCommit,
        },
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("可以跑完");

    // 每一轮看到的是最终轨迹的前缀，且前缀长度单调递增（每次多出上一轮的响应、上一轮的结果与本轮注入）。
    let final_len = run.trace().len();
    assert_eq!(
        final_len, 6,
        "Run 注入 + Turn 注入 + 响应 + 结果 + Turn 注入 + 响应"
    );
    let counts: Vec<usize> = model.seen.iter().map(|(_, count, _)| *count).collect();
    assert_eq!(
        counts,
        vec![2, 5],
        "第一轮只看到两次注入；第二轮看到前五项（含首轮响应与结果）"
    );

    // 顺序：Run 注入 → Turn1 注入 → 首轮响应 → 行动结果 → Turn2 注入 → 次轮响应。
    let kinds: Vec<&str> = run
        .trace()
        .iter()
        .map(|entry| match entry {
            TraceEntry::Injection {
                stage: InjectionStage::RunStart,
                ..
            } => "run-injection",
            TraceEntry::Injection {
                stage: InjectionStage::Turn,
                ..
            } => "turn-injection",
            TraceEntry::Refused { .. } => "action-refused",
            TraceEntry::Observation(_) => "action-result",
            TraceEntry::ModelResponse(_) => "model-response",
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "run-injection",
            "turn-injection",
            "model-response",
            "action-result",
            "turn-injection",
            "model-response"
        ]
    );

    // 每段原文恰好一次。
    let all = model.seen.last().expect("有调用").2.clone();
    for text in ["opening-note", "turn-1-fact", "turn-2-fact", marker] {
        assert_eq!(
            all.iter().filter(|seen| seen.as_str() == text).count(),
            1,
            "{text} 必须恰好出现一次: {all:?}"
        );
    }

    // 重复读取/重建视野不追加任何记录。
    let before = run.trace().len();
    for _ in 0..3 {
        let _ = run.trace();
        let _ = run.view(Capabilities::none());
        let _ = run.observed();
    }
    assert_eq!(run.trace().len(), before, "读轨迹不得改变轨迹");
}

/// 3. 两个不同的外层来源复用同一个 Core：Core 里没有它们的业务分支。
#[test]
fn two_different_outer_sources_reuse_the_same_core() {
    // 来源 A：固定开场说明。
    let mut fixed = RecordingSource::new(batch("note", "fixed-opening"), Vec::new());
    // 来源 B：按轮生成合成事实。
    let mut per_turn = RecordingSource::new(
        ContentBatch::empty(),
        vec![batch("synth", "synthetic-1"), batch("synth", "synthetic-2")],
    );

    let mut traces = Vec::new();
    for (index, source) in [&mut fixed as &mut dyn ContextSource, &mut per_turn]
        .into_iter()
        .enumerate()
    {
        let mut agent = Agent::new(AgentId::new("agent-a"));
        let mut ledger = Ledger::new(AgentId::new("agent-a"));
        let mut model = ScriptedModel::new(vec![
            ModelOutput::act(Action::new(ActionId::new("probe"))),
            ModelOutput::finish(SubjectEnding::Completed),
        ]);
        let mut actions = FailingOnceActions { calls: 0 };
        let run = run_activity_with(
            &mut agent,
            RunRequest {
                cost: &no_cost,
                run: RunId::new(format!("run-{index}")),
                limits: Limits::new(3),
                input: RunInput::empty(),
                source,
                commit: &mut NoCommit,
            },
            &mut model,
            &mut actions,
            &mut ledger,
        )
        .expect("两个来源都能跑完");
        traces.push(injection_texts(&run));
    }

    assert_eq!(traces[0], vec!["fixed-opening".to_string()]);
    assert_eq!(
        traces[1],
        vec!["synthetic-1".to_string(), "synthetic-2".to_string()]
    );
}

/// 4. 来源拿到真实身份；不同 Run 不继承；已结束的 Run 仍保留自己的注入。
#[test]
fn the_source_sees_the_real_identity_and_runs_do_not_inherit() {
    let mut agent = Agent::new(AgentId::new("agent-a"));
    let mut ledger = Ledger::new(AgentId::new("agent-a"));

    let mut source = RecordingSource::new(batch("note", "run-1-note"), Vec::new());
    let mut model = CountingModel { calls: 0 };
    let mut actions = UntouchedActions;
    let first = run_activity_with(
        &mut agent,
        RunRequest {
            cost: &no_cost,
            run: RunId::new("run-1"),
            limits: Limits::new(1),
            input: RunInput::empty(),
            source: &mut source,
            commit: &mut NoCommit,
        },
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("第一次活动");

    assert_eq!(
        source.calls[0],
        (
            "run-start".to_string(),
            "agent-a".to_string(),
            "run-1".to_string(),
            None
        ),
        "来源拿到的真实身份"
    );
    assert_eq!(
        source.calls[1],
        (
            "turn".to_string(),
            "agent-a".to_string(),
            "run-1".to_string(),
            Some(1)
        )
    );

    // 第二次活动用新的来源对象：它看不到第一次注入，第一次的 Run 仍保留自己的注入。
    let mut second_source = RecordingSource::new(batch("note", "run-2-note"), Vec::new());
    let mut model = CountingModel { calls: 0 };
    let mut actions = UntouchedActions;
    let second = run_activity_with(
        &mut agent,
        RunRequest {
            cost: &no_cost,
            run: RunId::new("run-2"),
            limits: Limits::new(1),
            input: RunInput::empty(),
            source: &mut second_source,
            commit: &mut NoCommit,
        },
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("第二次活动");

    assert_eq!(injection_texts(&first), vec!["run-1-note".to_string()]);
    assert_eq!(injection_texts(&second), vec!["run-2-note".to_string()]);
}

/// 5. 登记失败：Run::begin 拒绝 / 零预算 / 重复 RunId —— 钩子、模型、动作都零调用。
#[test]
fn nothing_is_injected_when_registration_fails() {
    // (a) 活动槽已被占用。
    let mut agent = Agent::new(AgentId::new("agent-a"));
    let held = celira_agent::core::Run::begin(&mut agent, RunId::new("run-held"), Limits::new(2))
        .expect("先占住活动槽");
    let mut ledger = Ledger::new(AgentId::new("agent-a"));
    let mut source = RecordingSource::new(batch("note", "never"), Vec::new());
    let mut model = UnreachableModel;
    let mut actions = UntouchedActions;

    let refused = run_activity_with(
        &mut agent,
        RunRequest {
            cost: &no_cost,
            run: RunId::new("run-1"),
            limits: Limits::new(2),
            input: RunInput::empty(),
            source: &mut source,
            commit: &mut NoCommit,
        },
        &mut model,
        &mut actions,
        &mut ledger,
    );
    assert!(refused.is_err(), "活动槽已占用必须被拒绝");
    assert!(source.calls.is_empty(), "被拒绝时不得调用任何钩子");
    assert!(ledger.records().is_empty(), "被拒绝时不得写任何事实");
    assert_eq!(agent.activity(), Some(held.id()), "既有占用不被破坏");

    // (b) 零预算。
    let mut agent = Agent::new(AgentId::new("agent-b"));
    let mut ledger = Ledger::new(AgentId::new("agent-b"));
    let mut source = RecordingSource::new(batch("note", "never"), Vec::new());
    let zero = run_activity_with(
        &mut agent,
        RunRequest {
            cost: &no_cost,
            run: RunId::new("run-1"),
            limits: Limits::new(0),
            input: RunInput::empty(),
            source: &mut source,
            commit: &mut NoCommit,
        },
        &mut UnreachableModel,
        &mut UntouchedActions,
        &mut ledger,
    );
    assert_eq!(
        zero.unwrap_err(),
        celira_agent::core::RunError::NoTurnBudget
    );
    assert!(source.calls.is_empty(), "零预算不得调用任何钩子");
    assert!(ledger.records().is_empty());

    // (c) 重复 RunId：RunStarted 登记失败，钩子零调用，既有事实不变。
    let mut agent = Agent::new(AgentId::new("agent-c"));
    let mut ledger = Ledger::new(AgentId::new("agent-c"));
    let mut first_source = RecordingSource::new(batch("note", "first"), Vec::new());
    let mut model = CountingModel { calls: 0 };
    run_activity_with(
        &mut agent,
        RunRequest {
            cost: &no_cost,
            run: RunId::new("run-same"),
            limits: Limits::new(1),
            input: RunInput::empty(),
            source: &mut first_source,
            commit: &mut NoCommit,
        },
        &mut model,
        &mut UntouchedActions,
        &mut ledger,
    )
    .expect("第一次登记成功");
    let records_after_first = ledger.records().len();

    let mut second_source = RecordingSource::new(batch("note", "second"), Vec::new());
    let duplicate = run_activity_with(
        &mut agent,
        RunRequest {
            cost: &no_cost,
            run: RunId::new("run-same"),
            limits: Limits::new(1),
            input: RunInput::empty(),
            source: &mut second_source,
            commit: &mut NoCommit,
        },
        &mut UnreachableModel,
        &mut UntouchedActions,
        &mut ledger,
    );
    assert!(duplicate.is_err(), "重复 RunId 必须登记失败");
    assert!(
        second_source.calls.is_empty(),
        "登记失败时 Run 钩子不得被调用"
    );
    assert_eq!(
        ledger.records().len(),
        records_after_first,
        "登记失败不得写入新事实"
    );
}

/// 6. 各终态的钩子次数精确，终态不变。
#[test]
fn every_terminal_path_has_exact_hook_counts() {
    // 首轮 Finish：Run 钩子一次 + Turn 钩子一次。
    let mut agent = Agent::new(AgentId::new("agent-a"));
    let mut ledger = Ledger::new(AgentId::new("agent-a"));
    let mut source = RecordingSource::new(batch("note", "opening"), Vec::new());
    let mut model = CountingModel { calls: 0 };
    let run = run_activity_with(
        &mut agent,
        RunRequest {
            cost: &no_cost,
            run: RunId::new("run-1"),
            limits: Limits::new(3),
            input: RunInput::empty(),
            source: &mut source,
            commit: &mut NoCommit,
        },
        &mut model,
        &mut UntouchedActions,
        &mut ledger,
    )
    .expect("可以跑完");
    assert_eq!(run.outcome(), Some(Outcome::Subject(SubjectEnding::Silent)));
    assert_eq!(source.calls.len(), 2, "Run 钩子 + 一次 Turn 钩子");
    assert_eq!(model.calls, 1);

    // ModelError：同样只有一次 Turn 钩子，之后不再有钩子与调用。
    let mut agent = Agent::new(AgentId::new("agent-b"));
    let mut ledger = Ledger::new(AgentId::new("agent-b"));
    let mut source = RecordingSource::new(batch("note", "opening"), Vec::new());
    let run = run_activity_with(
        &mut agent,
        RunRequest {
            cost: &no_cost,
            run: RunId::new("run-1"),
            limits: Limits::new(3),
            input: RunInput::empty(),
            source: &mut source,
            commit: &mut NoCommit,
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
    assert_eq!(source.calls.len(), 2);

    // 回执未达：活动不终止（2026-10-24）⇒ 主体看过读数后自己收场，两次 Turn 钩子。
    let mut agent = Agent::new(AgentId::new("agent-c"));
    let mut ledger = Ledger::new(AgentId::new("agent-c"));
    let mut source = RecordingSource::new(batch("note", "opening"), Vec::new());
    let mut model = ScriptedModel::new(vec![
        ModelOutput::act(Action::new(ActionId::new("probe"))),
        ModelOutput::finish(SubjectEnding::Silent),
    ]);
    let run = run_activity_with(
        &mut agent,
        RunRequest {
            cost: &no_cost,
            run: RunId::new("run-1"),
            limits: Limits::new(3),
            input: RunInput::empty(),
            source: &mut source,
            commit: &mut NoCommit,
        },
        &mut model,
        &mut UnknownActions,
        &mut ledger,
    )
    .expect("可以跑完");
    assert_eq!(run.outcome(), Some(Outcome::Subject(SubjectEnding::Silent)));
    assert_eq!(
        source.calls.len(),
        3,
        "Run 钩子 + 两次 Turn 钩子：没看见的那一轮之后还有一次"
    );
    assert_eq!(model.seen.len(), 2, "回执未达要进视野，主体还有一次机会");

    // 末轮预算耗尽：Turn 钩子等于轮数，不追加补偿调用。
    let mut agent = Agent::new(AgentId::new("agent-d"));
    let mut ledger = Ledger::new(AgentId::new("agent-d"));
    let mut source = RecordingSource::new(batch("note", "opening"), Vec::new());
    let mut model = ScriptedModel::new(vec![
        ModelOutput::act(Action::new(ActionId::new("probe"))),
        ModelOutput::act(Action::new(ActionId::new("probe"))),
    ]);
    let mut actions = FailingOnceActions { calls: 0 };
    let run = run_activity_with(
        &mut agent,
        RunRequest {
            cost: &no_cost,
            run: RunId::new("run-1"),
            limits: Limits::new(2),
            input: RunInput::empty(),
            source: &mut source,
            commit: &mut NoCommit,
        },
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("可以跑完");
    assert_eq!(run.outcome(), Some(Outcome::System(SystemEnding::Timeout)));
    assert_eq!(source.calls.len(), 3, "Run 钩子 + 两轮 Turn 钩子");
    assert_eq!(model.seen.len(), 2, "不得凭空追加补偿调用");

    // 普通 Action Failure：失败之后仍有轮数就继续注入并调用。
    let mut agent = Agent::new(AgentId::new("agent-e"));
    let mut ledger = Ledger::new(AgentId::new("agent-e"));
    let mut source = RecordingSource::new(batch("note", "opening"), Vec::new());
    let mut model = ScriptedModel::new(vec![
        ModelOutput::act(Action::new(ActionId::new("probe"))),
        ModelOutput::finish(SubjectEnding::Completed),
    ]);
    let mut actions = FailingOnceActions { calls: 0 };
    let run = run_activity_with(
        &mut agent,
        RunRequest {
            cost: &no_cost,
            run: RunId::new("run-1"),
            limits: Limits::new(2),
            input: RunInput::empty(),
            source: &mut source,
            commit: &mut NoCommit,
        },
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("可以跑完");
    assert_eq!(source.calls.len(), 3);
    let states: Vec<ResultState> = run.observed().iter().map(|o| o.state()).collect();
    assert_eq!(states, vec![ResultState::Failure]);
}

/// 7. 批次边界与来源失败：空批、块/字节上限、多字节、超限整体拒绝、失败与工具失败分清。
#[test]
fn batch_limits_are_whole_batch_and_source_failure_is_not_a_tool_failure() {
    // 空批次合法。
    assert!(ContentBatch::empty().is_empty());

    // 块数边界。
    let block = |index: usize| ContentBlock::new(format!("b{index}"), "x");
    let at_limit: Vec<ContentBlock> = (0..ContentBatch::MAX_BLOCKS).map(block).collect();
    assert_eq!(
        ContentBatch::of(at_limit)
            .expect("正好到块数上限")
            .blocks()
            .len(),
        ContentBatch::MAX_BLOCKS
    );
    let over: Vec<ContentBlock> = (0..=ContentBatch::MAX_BLOCKS).map(block).collect();
    assert_eq!(
        ContentBatch::of(over).unwrap_err(),
        ContentBatchError::TooManyBlocks {
            blocks: ContentBatch::MAX_BLOCKS + 1
        }
    );

    // 字节边界（含标签）与多字节。
    let label = "L";
    let text = "你".repeat((ContentBatch::MAX_BYTES - label.len()) / 3);
    let bytes = label.len() + text.len();
    assert!(bytes <= ContentBatch::MAX_BYTES);
    assert!(ContentBatch::of(vec![ContentBlock::new(label, text.clone())]).is_ok());
    let too_big = format!("{text}你");
    let refused = ContentBatch::of(vec![ContentBlock::new(label, too_big)]);
    assert!(
        matches!(refused, Err(ContentBatchError::TooLarge { .. })),
        "超限必须整体拒绝，不能截断成半批"
    );

    // 来源失败是**内容事实**，与工具 Failure 明确区分。
    let failure = ContentBatch::source_failure("source-unavailable", "cannot read the feed")
        .expect("在上限内");
    assert!(failure.is_source_failure());

    let mut agent = Agent::new(AgentId::new("agent-a"));
    let mut ledger = Ledger::new(AgentId::new("agent-a"));
    let mut source = SourceFromBatches::new(vec![failure.clone()]);
    let mut model = ScriptedModel::new(vec![
        ModelOutput::act(Action::new(ActionId::new("probe"))),
        ModelOutput::finish(SubjectEnding::Completed),
    ]);
    let mut actions = FailingOnceActions { calls: 0 };
    let run = run_activity_with(
        &mut agent,
        RunRequest {
            cost: &no_cost,
            run: RunId::new("run-1"),
            limits: Limits::new(2),
            input: RunInput::empty(),
            source: &mut source,
            commit: &mut NoCommit,
        },
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("可以跑完");

    // 顺序：Turn1 注入（来源失败）→ 首轮响应 → 行动结果 →（第二轮）Turn 注入为空、不产生记录。
    match &run.trace()[0] {
        TraceEntry::Injection { batch, .. } => {
            assert!(batch.is_source_failure(), "第一条是来源失败的内容事实");
        }
        TraceEntry::Refused { .. } => panic!("拒绝不该被记成来源注入"),
        TraceEntry::Observation(_) => panic!("注入不该被记成行动结果"),
        TraceEntry::ModelResponse(_) => panic!("注入不该被记成模型响应"),
    }
    match &run.trace()[1] {
        TraceEntry::ModelResponse(response) => {
            assert_eq!(response.turn().number(), 1, "响应带上它真实的轮次");
            assert!(
                matches!(response.decision(), Decision::Act(_)),
                "这一轮请求了行动"
            );
        }
        TraceEntry::Refused { .. } => panic!("拒绝不是模型响应"),
        TraceEntry::Injection { .. } => panic!("模型响应不该被记成注入"),
        TraceEntry::Observation(_) => panic!("模型响应先于它请求的行动结果"),
    }
    match &run.trace()[2] {
        TraceEntry::Observation(observation) => {
            assert_eq!(
                observation.state(),
                ResultState::Failure,
                "行动失败另成一条"
            );
            assert_eq!(observation.outcome().failure_class(), Some("not-found"));
        }
        TraceEntry::Refused { .. } => panic!("拒绝不是行动结果：它没有派发过"),
        TraceEntry::Injection { .. } => panic!("行动结果不该被记成注入"),
        TraceEntry::ModelResponse(_) => panic!("行动结果不该被记成模型响应"),
    }
}

/// 只返回给定批次的来源（第一轮给第一个，之后给空）。
struct SourceFromBatches {
    batches: VecDeque<ContentBatch>,
}

impl SourceFromBatches {
    fn new(batches: Vec<ContentBatch>) -> Self {
        Self {
            batches: VecDeque::from(batches),
        }
    }
}

impl ContextSource for SourceFromBatches {
    fn on_run_start(&mut self, _context: &InjectionContext) -> ContentBatch {
        ContentBatch::empty()
    }

    fn before_turn(&mut self, _context: &InjectionContext) -> ContentBatch {
        self.batches.pop_front().unwrap_or_else(ContentBatch::empty)
    }
}

/// 9. 注入与输入都不进入 Foundation 的事实。
#[test]
fn injections_never_reach_the_foundation_facts() {
    let sandbox_marker = "foundation-marker-injection";
    let mut agent = Agent::new(AgentId::new("agent-a"));
    let mut ledger = Ledger::new(AgentId::new("agent-a"));
    let mut source = RecordingSource::new(batch("note", sandbox_marker), Vec::new());
    let input_marker = "foundation-marker-input";
    let mut model = CountingModel { calls: 0 };
    let run = run_activity_with(
        &mut agent,
        RunRequest {
            cost: &no_cost,
            run: RunId::new("run-1"),
            limits: Limits::new(1),
            input: RunInput::text(input_marker).expect("输入在上限内"),
            source: &mut source,
            commit: &mut NoCommit,
        },
        &mut model,
        &mut UntouchedActions,
        &mut ledger,
    )
    .expect("可以跑完");

    let facts = format!("{:?}", ledger.records());
    assert!(!facts.contains(sandbox_marker), "注入不得进入事实");
    assert!(!facts.contains(input_marker), "开场输入不得进入事实");
    // 轨迹里仍然有它们。
    assert_eq!(injection_texts(&run), vec![sandbox_marker.to_string()]);
    assert_eq!(run.input().as_str(), input_marker);
}

/// 组合层：显式传来源的入口可用，且旧入口（空来源）行为不变。
#[test]
fn the_composition_entry_can_take_a_source_explicitly() {
    let sandbox = std::env::temp_dir().join(format!(
        "celira-slots-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("时钟可用")
            .as_nanos()
    ));
    std::fs::create_dir_all(&sandbox).expect("可以建临时沙盒");
    std::fs::write(sandbox.join("alpha.txt"), "slots-marker").expect("可以写合成文件");

    // 旧入口：空来源，行为不变（轨迹里只有模型自己的那一条响应，没有任何注入）。
    let mut old = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
    let old_run = run_read_only_files(
        &sandbox,
        AgentId::new("agent-old"),
        RunId::new("run-old"),
        Limits::new(2),
        &mut old,
    )
    .expect("旧入口可用");
    assert_eq!(old_run.run.trace().len(), 1);
    assert!(
        !old_run
            .run
            .trace()
            .iter()
            .any(|entry| matches!(entry, TraceEntry::Injection { .. })),
        "旧入口没有注入"
    );

    // 新入口：显式传来源。
    let mut source = RecordingSource::new(batch("note", "composition-note"), Vec::new());
    let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
    let mut os = MinimalOs::open(&sandbox).expect("可以装配");
    let mut agent = Agent::new(AgentId::new("agent-new"));
    let mut ledger = Ledger::new(AgentId::new("agent-new"));
    let mut expansion = RunExpansion::new(AgentId::new("agent-new"), RunId::new("run-new"));
    let mut scope = os.scope(&mut expansion);
    let run = run_activity_with(
        &mut agent,
        RunRequest {
            cost: &no_cost,
            run: RunId::new("run-new"),
            limits: Limits::new(2),
            input: RunInput::empty(),
            source: &mut source,
            commit: &mut NoCommit,
        },
        &mut model,
        &mut scope,
        &mut ledger,
    )
    .expect("带来源的活动可以跑完");
    assert_eq!(injection_texts(&run), vec!["composition-note".to_string()]);
    assert_eq!(source.calls.len(), 2);

    let _ = std::fs::remove_dir_all(&sandbox);
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
