//! 最小闭环的端到端证明。
//!
//! 这里只使用本 crate 自己的类型：没有模型供应商、没有网络、没有具体应用、
//! 没有文件或数据库。Model 与 Action 都由测试脚本实现。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use celira_agent::app_contract::DOOR_PLATE_MAX_BYTES;
use celira_agent::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionPort, Agent, Capabilities, Limits,
    ModelError, ModelFailureClass, ModelOutput, ModelPort, Run, RunError, View, run_activity,
};
use celira_agent::foundation::{
    ActionResult, AgentId, Ledger, LedgerError, OperationId, Outcome, Record, RecoveryError,
    ResultState, RunId, SubjectEnding, SystemEnding, Turn, recover,
};

const BUDGET: u32 = 4;

/// **封闭**的比喻词表（2026-10-08 主人确认；**2026-10-18 扩族**）：它进不了主体的上下文。
///
/// 不按裸子串放宽，也不收中立词（"窗口""限制""视野"都不在内）——误伤一次，人就学会绕开守卫。
/// 两个守卫共用它：`her_visible_words_carry_no_new_metaphor`（她的视野）与
/// `the_door_plates_are_one_mechanical_sentence`（门牌那一行）。
///
/// ⚠ **封的是比喻，不是词**（2026-10-18 扩族）：原表只封了复合写法，**同一个比喻换个写法就漏过去**
/// ——实测漏掉的三处正是还留在她眼前的那几处（`手脚` 在表里而 `terminal 那只手` 不在；
/// `自留地` 在表里而 `你的地盘` 不在；`地界` 一个写法都不在）。⇒ 按**比喻族**补：
/// `那只手` / `这只手`（与 `手脚` 同族）、`地盘`（与 `自留地` 同族）、`地界`。
/// 裸 `手` **不进表**（`手册` / `手笔` / `助手` 都是合法词）——反例自检钉着这一条。
///
/// ⚠ **一处待裁的例外写在明处**：`身体`（= Runtime / 这台机器）今天**不在表里**，而它同时是
/// `AGENT_VOCABULARY.md` §2 认的当前口径（"换机器＝换身体"）⇒ 一个词两处身份，**主人未裁**。
/// 她视野里现存四处：`orientation.rs` 三处（静态头首句、手册首段、手册末段）与
/// `im_app.rs` 一处（"不是身体态"）。
/// 裁"进表"就一起改成机械说法；裁"是名字"就把这一段删掉——**不许两处都不说**。
const METAPHOR_WORDS: [&str; 16] = [
    "手机",
    "屏幕",
    "桌面",
    "墙",
    "纸条",
    "材料窗",
    "窗界",
    "自留地",
    "地盘",
    "地界",
    "油表",
    "电池",
    "手脚",
    "那只手",
    "这只手",
    "眼睛",
];

fn agent(name: &str) -> Agent {
    Agent::new(AgentId::new(name))
}

fn ledger_for(agent: &Agent) -> Ledger {
    Ledger::new(agent.id().clone())
}

fn run_id(name: &str) -> RunId {
    RunId::new(name)
}

fn action(name: &str) -> Action {
    Action::new(ActionId::new(name))
}

fn limits() -> Limits {
    Limits::new(BUDGET)
}

/// 只按脚本作答的 Model：它只看到中立视野，脚本里只有中立结果。
struct ScriptedModel {
    script: VecDeque<ModelOutput>,
    seen: Vec<(Turn, u32, usize)>,
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
        self.seen
            .push((view.turn(), view.remaining_turns(), view.observed().len()));
        Ok(self.script.pop_front().expect("脚本对每一次 Turn 都有作答"))
    }
}

/// 永远失败的 Model。
struct FailingModel;

impl ModelPort for FailingModel {
    fn complete(&mut self, _view: &View) -> Result<ModelOutput, ModelError> {
        Err(scripted_model_failure())
    }
}

/// 固定返回同一种结果的 Action 能力（不关心身份与参数）。
struct FixedActions {
    outcome: ActionOutcome,
}

impl FixedActions {
    /// 用 Core 的三分状态构造一个固定结果：成功不带内容、失败带一个测试分类。
    fn with(state: ResultState) -> Self {
        let outcome = match state {
            ResultState::Success => ActionOutcome::success(Vec::new()),
            ResultState::Failure => ActionOutcome::failure("test-failure"),
            ResultState::Unknown => ActionOutcome::unknown(),
        };
        Self { outcome }
    }
}

impl ActionPort for FixedActions {
    /// 测试替身不提供任何能力（Core 只搬运它，不解释）。
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
        self.outcome.clone()
    }
}

/// 会自己读 View 的 Model：它照自己看到的结果决定下一步。
///
/// 它不消费系统预排的答案，也不接受任何"失败已经替你总结过"的输入——它只看
/// `View` 里已有的行动结果事实。还没看到失败时它每次都选同一个 Action（这是
/// Model 自己的选择，不是系统的重试策略）；看到失败之后按脚本给出决定。
struct ReactiveModel {
    after_failure: VecDeque<ModelOutput>,
    seen: Vec<Vec<(u32, ResultState)>>,
}

impl ReactiveModel {
    fn new(after_failure: Vec<ModelOutput>) -> Self {
        Self {
            after_failure: VecDeque::from(after_failure),
            seen: Vec::new(),
        }
    }
}

impl ModelPort for ReactiveModel {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        let observed: Vec<(u32, ResultState)> = view
            .observed()
            .iter()
            .map(|result| (result.operation().turn().number(), result.state()))
            .collect();
        let saw_failure = observed
            .iter()
            .any(|(_, state)| *state == ResultState::Failure);
        self.seen.push(observed);

        if saw_failure {
            Ok(self
                .after_failure
                .pop_front()
                .expect("看到失败之后脚本对每一次 Turn 都有决定"))
        } else {
            Ok(ModelOutput::act(action("attempt")))
        }
    }
}

#[test]
fn an_agent_begins_one_bounded_activity() {
    let mut a = agent("agent-a");
    assert_eq!(a.activity(), None, "新建的 Agent 空闲");

    let run = Run::begin(&mut a, run_id("run-1"), limits()).expect("活动可以开始");

    assert_eq!(run.agent(), a.id());
    assert_eq!(
        a.activity(),
        Some(&run_id("run-1")),
        "活动开始之后活动槽被这次活动占用"
    );
    assert!(!run.is_ended());
    assert_eq!(run.outcome(), None);
    assert!(run.action_in_flight().is_none());

    // 同一个 Agent 不能同时有两个没有收场的活动。
    assert_eq!(
        Run::begin(&mut a, run_id("run-2"), limits()).unwrap_err(),
        RunError::ActiveRunExists
    );
    assert_eq!(a.activity(), Some(&run_id("run-1")), "被拒绝不得改变活动槽");

    // 别的 Agent 拥有自己的活动槽，不受影响。
    let mut b = agent("agent-b");
    assert_eq!(b.activity(), None);
    assert!(Run::begin(&mut b, run_id("run-9"), limits()).is_ok());
    assert_eq!(b.activity(), Some(&run_id("run-9")));

    // 资源边界必须至少允许一次 Turn；被拒绝时不占用活动槽。
    let mut fresh = agent("agent-c");
    assert_eq!(
        Run::begin(&mut fresh, run_id("run-4"), Limits::new(0)).unwrap_err(),
        RunError::NoTurnBudget
    );
    assert_eq!(fresh.activity(), None, "被拒绝的活动不得占用活动槽");
}

#[test]
fn an_unfinished_activity_holds_the_slot_and_only_a_legal_end_frees_it() {
    let mut a = agent("agent-a");
    let mut ledger = ledger_for(&a);

    // 活动 A 只走到"开始"，没有收场——活动槽被占住（不靠线程或锁伪造并发）。
    let mut activity_a = Run::begin(&mut a, run_id("run-a"), limits()).expect("活动 A 可以开始");
    assert!(!activity_a.is_ended());
    assert_eq!(a.activity(), Some(&run_id("run-a")));

    // 活动 A 还没收场期间，同一个 Agent 尝试开始活动 B。
    let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Completed)]);
    let mut actions = FixedActions::with(ResultState::Success);
    assert_eq!(
        run_activity(
            &mut a,
            run_id("run-b"),
            limits(),
            &mut model,
            &mut actions,
            &mut ledger,
        )
        .unwrap_err(),
        RunError::ActiveRunExists
    );

    // 拒绝没有任何副作用：没问 Model、没写任何事实、槽仍然属于活动 A。
    assert_eq!(model.seen.len(), 0, "被拒绝的调用不得调用 Model");
    assert!(ledger.records().is_empty(), "被拒绝的调用不得写入任何事实");
    assert_eq!(a.activity(), Some(&run_id("run-a")));

    // 唯一合法的释放方式是让活动 A 收场，而且收场必须带上它所属的 Agent。
    activity_a
        .end(&mut a, Outcome::Subject(SubjectEnding::Completed))
        .expect("活动 A 可以合法收场");
    assert!(activity_a.is_ended());
    assert_eq!(a.activity(), None, "槽位与活动收场同步释放");

    // 槽位释放之后，同一个 Agent 可以开始第二个活动（新的活动身份）。
    let second = Run::begin(&mut a, run_id("run-b"), limits()).expect("第二个活动可以开始");
    assert_eq!(second.id(), &run_id("run-b"));
    assert_eq!(a.activity(), Some(&run_id("run-b")));
    assert!(ledger.records().is_empty(), "手工路径不写事实");
}

#[test]
fn a_foreign_agent_cannot_end_another_agents_activity() {
    let mut a = agent("agent-a");
    let mut b = agent("agent-b");
    let mut run_a = Run::begin(&mut a, run_id("run-a"), limits()).expect("活动可以开始");
    let mut run_b = Run::begin(&mut b, run_id("run-b"), limits()).expect("活动可以开始");

    // 两个独立 Agent 实例同一时刻各持一个未收场活动。
    assert_eq!(a.activity(), Some(&run_id("run-a")));
    assert_eq!(b.activity(), Some(&run_id("run-b")));

    // B 想收 A 的活动：被拒绝，两边的 Run 状态与槽位都不变。
    assert_eq!(
        run_a
            .end(&mut b, Outcome::Subject(SubjectEnding::Completed))
            .unwrap_err(),
        RunError::AgentMismatch
    );
    assert!(!run_a.is_ended(), "被拒绝的收场不得改变 Run 状态");
    assert_eq!(a.activity(), Some(&run_id("run-a")));
    assert_eq!(b.activity(), Some(&run_id("run-b")));

    // 反向同样被拒绝。
    assert_eq!(
        run_b
            .end(&mut a, Outcome::Subject(SubjectEnding::Completed))
            .unwrap_err(),
        RunError::AgentMismatch
    );
    assert_eq!(a.activity(), Some(&run_id("run-a")));
    assert_eq!(b.activity(), Some(&run_id("run-b")));

    // 各自收自己的活动都可以，且互不影响。
    run_a
        .end(&mut a, Outcome::Subject(SubjectEnding::Completed))
        .expect("A 收自己的活动");
    assert_eq!(a.activity(), None);
    assert_eq!(b.activity(), Some(&run_id("run-b")));

    run_b
        .end(&mut b, Outcome::Subject(SubjectEnding::Completed))
        .expect("B 收自己的活动");
    assert_eq!(b.activity(), None);
}

#[test]
fn every_ending_path_hands_the_activity_slot_back() {
    // 1) Agent 自己结束。
    let mut a = agent("agent-a");
    let mut ledger = ledger_for(&a);
    let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
    let mut actions = FixedActions::with(ResultState::Success);
    run_activity(
        &mut a,
        run_id("run-1"),
        limits(),
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("活动可以跑完");
    assert_eq!(a.activity(), None, "主体收场之后必须释放活动槽");

    // 2) Model 失败。
    let mut a = agent("agent-a");
    let mut ledger = ledger_for(&a);
    let mut model = FailingModel;
    let mut actions = FixedActions::with(ResultState::Success);
    run_activity(
        &mut a,
        run_id("run-2"),
        limits(),
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("系统失败也是一次明确收场");
    assert_eq!(a.activity(), None, "Model 失败之后必须释放活动槽");

    // 3) 结果无法确认，由系统收场。
    let mut a = agent("agent-a");
    let mut ledger = ledger_for(&a);
    let mut model = ScriptedModel::new(vec![ModelOutput::act(action("attempt"))]);
    let mut actions = FixedActions::with(ResultState::Unknown);
    run_activity(
        &mut a,
        run_id("run-3"),
        limits(),
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("活动以明确收场结束");
    assert_eq!(a.activity(), None, "无法确认的收场之后必须释放活动槽");

    // 4) Turn 预算耗尽。
    let mut a = agent("agent-a");
    let mut ledger = ledger_for(&a);
    let mut model = ScriptedModel::new(vec![
        ModelOutput::act(action("attempt")),
        ModelOutput::act(action("attempt")),
        ModelOutput::act(action("attempt")),
        ModelOutput::act(action("attempt")),
    ]);
    let mut actions = FixedActions::with(ResultState::Success);
    run_activity(
        &mut a,
        run_id("run-4"),
        limits(),
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("活动可以跑完");
    assert_eq!(a.activity(), None, "预算耗尽之后必须释放活动槽");

    // 5) 行动失败之后由 Agent 自己决定结束。
    let mut a = agent("agent-a");
    let mut ledger = ledger_for(&a);
    let mut model = ReactiveModel::new(vec![ModelOutput::finish(SubjectEnding::Declined)]);
    let mut actions = FixedActions::with(ResultState::Failure);
    run_activity(
        &mut a,
        run_id("run-5"),
        limits(),
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("活动可以跑完");
    assert_eq!(a.activity(), None, "行动失败后由主体收场也必须释放活动槽");

    // 6) 账本写入失败：可返回的活动级错误也不允许把 Agent 留在占用状态。
    let mut a = agent("agent-a");
    let mut foreign_ledger = Ledger::new(AgentId::new("agent-b"));
    let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Completed)]);
    let mut actions = FixedActions::with(ResultState::Success);
    assert_eq!(
        run_activity(
            &mut a,
            run_id("run-6"),
            limits(),
            &mut model,
            &mut actions,
            &mut foreign_ledger,
        )
        .unwrap_err(),
        RunError::Ledger(LedgerError::ForeignAgent)
    );
    assert_eq!(a.activity(), None, "账本写入失败之后也必须释放活动槽");
    assert_eq!(model.seen.len(), 0, "第一个事实没写成功就不该开始处理");
}

#[test]
fn an_agent_can_start_a_second_activity_after_the_first_ended() {
    let mut a = agent("agent-a");
    let mut ledger = ledger_for(&a);

    let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Completed)]);
    let mut actions = FixedActions::with(ResultState::Success);
    let first = run_activity(
        &mut a,
        run_id("run-1"),
        limits(),
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("第一个活动可以跑完");
    assert_eq!(a.activity(), None, "第一个活动结束后必须空闲");

    // 第二个活动用新的 RunId，正常写出自己的事实，不被第一次的残留状态阻塞。
    let mut model = ScriptedModel::new(vec![
        ModelOutput::act(action("attempt")),
        ModelOutput::finish(SubjectEnding::Completed),
    ]);
    let mut actions = FixedActions::with(ResultState::Success);
    let second = run_activity(
        &mut a,
        run_id("run-2"),
        limits(),
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("第二个活动可以跑完");

    assert_eq!(first.id(), &run_id("run-1"));
    assert_eq!(second.id(), &run_id("run-2"));
    assert_eq!(a.activity(), None);

    let started: Vec<RunId> = ledger
        .records()
        .iter()
        .filter_map(|record| match record {
            Record::RunStarted { run, .. } => Some(run.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(started, vec![run_id("run-1"), run_id("run-2")]);
}

#[test]
fn one_turn_asks_the_model_and_gets_one_action() {
    let mut a = agent("agent-a");
    let mut ledger = ledger_for(&a);
    let mut model = ScriptedModel::new(vec![
        ModelOutput::act(action("attempt")),
        ModelOutput::finish(SubjectEnding::Completed),
    ]);
    let mut actions = FixedActions::with(ResultState::Success);

    let run = run_activity(
        &mut a,
        run_id("run-1"),
        limits(),
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("活动可以跑完");

    assert_eq!(model.seen.len(), 2, "两次 Turn 各问了一次 Model");
    assert_eq!(run.observed().len(), 1);
    assert_eq!(run.observed()[0].state(), ResultState::Success);
    assert_eq!(run.observed()[0].operation().turn(), Turn::FIRST);
    assert_eq!(run.last_turn().number(), 2);
}

#[test]
fn the_view_only_carries_this_activitys_facts_and_limits() {
    let mut a = agent("agent-a");
    let mut ledger = ledger_for(&a);
    let mut model = ScriptedModel::new(vec![
        ModelOutput::act(action("attempt")),
        ModelOutput::finish(SubjectEnding::Completed),
    ]);
    let mut actions = FixedActions::with(ResultState::Success);

    run_activity(
        &mut a,
        run_id("run-1"),
        limits(),
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("活动可以跑完");

    let (first_turn, first_remaining, first_observed) = model.seen[0];
    assert_eq!(first_turn, Turn::FIRST);
    assert_eq!(first_remaining, BUDGET - 1);
    assert_eq!(first_observed, 0, "第一次处理时还没有任何已确认结果");

    let (second_turn, second_remaining, second_observed) = model.seen[1];
    assert_eq!(second_turn.number(), 2);
    assert_eq!(second_remaining, BUDGET - 2);
    assert_eq!(second_observed, 1, "第二次处理时能看到上一次的结果");
}

#[test]
fn action_result_and_outcome_land_in_the_ledger_as_facts() {
    let mut a = agent("agent-a");
    let mut ledger = ledger_for(&a);
    let mut model = ScriptedModel::new(vec![
        ModelOutput::act(action("attempt")),
        ModelOutput::finish(SubjectEnding::Silent),
    ]);
    let mut actions = FixedActions::with(ResultState::Success);

    let run = run_activity(
        &mut a,
        run_id("run-1"),
        limits(),
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("活动可以跑完");

    assert_eq!(run.outcome(), Some(Outcome::Subject(SubjectEnding::Silent)));

    let kinds: Vec<&str> = ledger
        .records()
        .iter()
        .map(|record| match record {
            Record::RunStarted { .. } => "started",
            Record::ActionPrepared { .. } => "prepared",
            Record::ActionDispatched { .. } => "dispatched",
            Record::ActionResultRecorded { .. } => "result",
            Record::RunEnded { .. } => "ended",
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["started", "prepared", "dispatched", "result", "ended"]
    );

    assert_eq!(
        ledger.records().last(),
        Some(&Record::RunEnded {
            run: run_id("run-1"),
            outcome: Outcome::Subject(SubjectEnding::Silent),
            cost: celira_agent::cost::RunCost::default(),
        })
    );
}

#[test]
fn recovery_rebuilds_an_ended_activity_from_facts_only() {
    // 只把事实带走：内存里的 Run 与 Ledger 都不参与重建。
    let snapshot: Vec<Record> = {
        let mut a = agent("agent-a");
        let mut ledger = ledger_for(&a);
        let mut model = ScriptedModel::new(vec![
            ModelOutput::act(action("attempt")),
            ModelOutput::finish(SubjectEnding::Completed),
        ]);
        let mut actions = FixedActions::with(ResultState::Success);
        run_activity(
            &mut a,
            run_id("run-1"),
            limits(),
            &mut model,
            &mut actions,
            &mut ledger,
        )
        .expect("活动可以跑完");
        ledger.records().to_vec()
    };

    let recovered = recover(&snapshot).expect("事实自洽");

    assert_eq!(recovered.agent, AgentId::new("agent-a"));
    assert_eq!(recovered.run, run_id("run-1"));
    assert_eq!(
        recovered.outcome,
        Some(Outcome::Subject(SubjectEnding::Completed))
    );
    assert!(recovered.is_ended());
    assert_eq!(
        recovered.last_action_turn,
        Some(Turn::FIRST),
        "只有第一个 Turn 留下了行动事实；收场发生在第二个 Turn，但它没有行动事实"
    );
    assert!(!recovered.has_unconfirmed_effects());
    assert!(
        !recovered.may_dispatch_again(),
        "活动已经收场，不再发出外部效果"
    );
}

#[test]
fn model_failure_and_action_failure_stay_distinct() {
    let model_failed = {
        let mut a = agent("agent-a");
        let mut ledger = ledger_for(&a);
        let mut model = FailingModel;
        let mut actions = FixedActions::with(ResultState::Success);
        run_activity(
            &mut a,
            run_id("run-m"),
            limits(),
            &mut model,
            &mut actions,
            &mut ledger,
        )
        .expect("系统失败也是一次明确收场")
    };

    // Action Failure 不再是 Run 的系统级收场：它必须作为事实交回 Agent 自己判断。
    let (action_failed, action_failed_ledger) = {
        let mut a = agent("agent-a");
        let mut ledger = ledger_for(&a);
        let mut model = ReactiveModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
        let mut actions = FixedActions::with(ResultState::Failure);
        let run = run_activity(
            &mut a,
            run_id("run-x"),
            limits(),
            &mut model,
            &mut actions,
            &mut ledger,
        )
        .expect("活动可以跑完");
        (run, ledger)
    };

    assert_eq!(
        model_failed.outcome(),
        Some(Outcome::System(SystemEnding::ModelFailed(Some(
            SCRIPTED_MODEL_FAILURE.as_str()
        ))))
    );
    assert!(
        model_failed.observed().is_empty(),
        "模型没有给结果，就没有行动结果"
    );

    // 两条失败路径仍然分开：Model Failure 终结活动（系统层），
    // Action Failure 只是被记下来的行动结果（行动层），收场由 Agent 自己选择。
    assert_eq!(
        action_failed.outcome(),
        Some(Outcome::Subject(SubjectEnding::Silent))
    );
    assert!(
        !matches!(
            action_failed.outcome(),
            Some(Outcome::System(SystemEnding::ActionFailed))
        ),
        "Action Failure 不得再走系统级收场路径"
    );
    assert_eq!(action_failed.observed().len(), 1);
    assert_eq!(action_failed.observed()[0].state(), ResultState::Failure);

    let recorded: Vec<ResultState> = action_failed_ledger
        .records()
        .iter()
        .filter_map(|record| match record {
            Record::ActionResultRecorded { action, .. } => Some(action.state()),
            _ => None,
        })
        .collect();
    assert_eq!(
        recorded,
        vec![ResultState::Failure],
        "行动失败仍然是账本里的事实，没有被改写"
    );

    assert_ne!(model_failed.outcome(), action_failed.outcome());
}

#[test]
fn a_failed_action_is_handed_back_to_the_agent_instead_of_ending_the_activity() {
    let mut a = agent("agent-a");
    let mut ledger = ledger_for(&a);
    let mut model = ReactiveModel::new(vec![ModelOutput::finish(SubjectEnding::Declined)]);
    let mut actions = FixedActions::with(ResultState::Failure);

    let run = run_activity(
        &mut a,
        run_id("run-1"),
        limits(),
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("活动可以跑完");

    // 失败已经被记成事实，并且保留 Failure，没有被改写成别的类别。
    let recorded: Vec<ResultState> = ledger
        .records()
        .iter()
        .filter_map(|record| match record {
            Record::ActionResultRecorded { action, .. } => Some(action.state()),
            _ => None,
        })
        .collect();
    assert_eq!(recorded, vec![ResultState::Failure]);

    // 活动没有在失败处结束：失败之后还走到了第二个 Turn，Model 被再次调用。
    assert_eq!(run.last_turn().number(), 2, "失败之后活动必须还能继续");
    assert_eq!(model.seen.len(), 2, "失败之后必须再问一次 Model");

    // 第二轮 Model 看到的是上一轮的失败事实本身（第几个 Turn + 什么类别）。
    assert_eq!(
        model.seen[1],
        vec![(1, ResultState::Failure)],
        "第二轮 Model 必须能自己观察到上一轮的行动失败"
    );

    // 收场表达的是 Agent 自己的选择，不是系统替它做的决定。
    assert_eq!(
        run.outcome(),
        Some(Outcome::Subject(SubjectEnding::Declined))
    );
}

#[test]
fn after_a_failed_action_the_agent_can_choose_another_action() {
    let mut a = agent("agent-a");
    let mut ledger = ledger_for(&a);
    let mut model = ReactiveModel::new(vec![
        ModelOutput::act(action("fallback")),
        ModelOutput::finish(SubjectEnding::Completed),
    ]);
    let mut actions = FixedActions::with(ResultState::Failure);

    let run = run_activity(
        &mut a,
        run_id("run-1"),
        limits(),
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("活动可以跑完");

    // 失败之后 Agent 自己换了一个 Action，于是出现了第二次尝试。
    let observed: Vec<(u32, ResultState)> = run
        .observed()
        .iter()
        .map(|result| (result.operation().turn().number(), result.state()))
        .collect();
    assert_eq!(
        observed,
        vec![(1, ResultState::Failure), (2, ResultState::Failure)],
        "第二次尝试来自 Agent 的决定，不是系统重试"
    );

    // 第二、第三轮 Model 都看到了累积到那时为止的失败事实。
    assert_eq!(model.seen.len(), 3);
    assert_eq!(model.seen[1], vec![(1, ResultState::Failure)]);
    assert_eq!(
        model.seen[2],
        vec![(1, ResultState::Failure), (2, ResultState::Failure)]
    );

    // 没有自动重发：两次尝试各只有一次准备与一次发出。
    let prepared = ledger
        .records()
        .iter()
        .filter(|record| matches!(record, Record::ActionPrepared { .. }))
        .count();
    let dispatched = ledger
        .records()
        .iter()
        .filter(|record| matches!(record, Record::ActionDispatched { .. }))
        .count();
    assert_eq!(prepared, 2, "系统没有自动重发同一个操作");
    assert_eq!(dispatched, 2);
    assert!(matches!(
        ledger.records().last(),
        Some(Record::RunEnded { .. })
    ));

    assert_eq!(
        run.outcome(),
        Some(Outcome::Subject(SubjectEnding::Completed))
    );
}

#[test]
fn unknown_is_never_rewritten_as_success_or_failure() {
    let mut a = agent("agent-a");
    let mut ledger = ledger_for(&a);
    let mut model = ScriptedModel::new(vec![ModelOutput::act(action("attempt"))]);
    let mut actions = FixedActions::with(ResultState::Unknown);

    let run = run_activity(
        &mut a,
        run_id("run-1"),
        limits(),
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("活动以明确收场结束");

    assert_eq!(run.observed()[0].state(), ResultState::Unknown);
    assert_eq!(
        run.outcome(),
        Some(Outcome::System(SystemEnding::Indeterminate))
    );
    assert_eq!(
        model.seen.len(),
        1,
        "无法确认的结果不允许再发新效果，所以没有第二轮 Model——这与 Action Failure 不同"
    );

    let recorded = ledger.records().iter().find_map(|record| match record {
        Record::ActionResultRecorded { action, .. } => Some(action.state()),
        _ => None,
    });
    assert_eq!(recorded, Some(ResultState::Unknown));

    let recovered = recover(ledger.records()).expect("事实自洽");
    assert!(
        recovered.has_unconfirmed_effects(),
        "未知结果必须留在无法确认的集合里"
    );
    assert!(
        !recovered.may_dispatch_again(),
        "可能已经发生过的效果不允许再次发出"
    );
}

#[test]
fn an_activity_without_a_choice_ends_at_its_resource_boundary() {
    let mut a = agent("agent-a");
    let mut ledger = ledger_for(&a);
    let mut model = ScriptedModel::new(vec![
        ModelOutput::act(action("attempt")),
        ModelOutput::act(action("attempt")),
        ModelOutput::act(action("attempt")),
        ModelOutput::act(action("attempt")),
    ]);
    let mut actions = FixedActions::with(ResultState::Success);

    let run = run_activity(
        &mut a,
        run_id("run-1"),
        limits(),
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("活动可以跑完");

    assert_eq!(run.outcome(), Some(Outcome::System(SystemEnding::Timeout)));
    assert_eq!(run.last_turn().number(), BUDGET);
    assert_eq!(run.observed().len(), BUDGET as usize);
}

#[test]
fn repeating_the_same_operation_identity_adds_no_second_fact() {
    let a = agent("agent-a");
    let mut ledger = ledger_for(&a);
    let started = Record::RunStarted {
        agent: a.id().clone(),
        run: run_id("run-1"),
    };

    ledger.append(started.clone()).expect("第一次开始活动");
    assert_eq!(ledger.append(started), Err(LedgerError::Duplicate));

    let operation = OperationId::new(run_id("run-1"), Turn::FIRST);
    let prepared = Record::ActionPrepared {
        run: run_id("run-1"),
        operation: operation.clone(),
    };
    let result = Record::ActionResultRecorded {
        run: run_id("run-1"),
        action: ActionResult::new(operation, ResultState::Success),
    };

    ledger.append(prepared.clone()).expect("第一次准备尝试");
    ledger.append(result.clone()).expect("第一次记录结果");
    let before = ledger.records().len();

    assert_eq!(ledger.append(prepared), Err(LedgerError::Duplicate));
    assert_eq!(ledger.append(result), Err(LedgerError::Duplicate));
    assert_eq!(
        ledger.records().len(),
        before,
        "重复的操作身份没有产生第二份事实"
    );
}

#[test]
fn facts_never_cross_agent_boundaries() {
    let a = agent("agent-a");
    let b = agent("agent-b");
    let mut ledger = ledger_for(&a);

    assert_eq!(
        ledger.append(Record::RunStarted {
            agent: b.id().clone(),
            run: run_id("run-b"),
        }),
        Err(LedgerError::ForeignAgent)
    );

    assert_eq!(
        ledger.append(Record::ActionPrepared {
            run: run_id("run-1"),
            operation: OperationId::new(run_id("run-1"), Turn::FIRST),
        }),
        Err(LedgerError::UnregisteredRun)
    );

    assert!(ledger.records().is_empty());
    assert_eq!(ledger.owner(), a.id());
}

#[test]
fn recovery_refuses_effects_that_were_never_prepared() {
    let a = agent("agent-a");
    let mut ledger = ledger_for(&a);
    let operation = OperationId::new(run_id("run-1"), Turn::FIRST);

    ledger
        .append(Record::RunStarted {
            agent: a.id().clone(),
            run: run_id("run-1"),
        })
        .expect("登记活动开始");
    ledger
        .append(Record::ActionResultRecorded {
            run: run_id("run-1"),
            action: ActionResult::new(operation.clone(), ResultState::Success),
        })
        .expect("账本照实追加");

    assert_eq!(
        recover(ledger.records()),
        Err(RecoveryError::ResultWithoutPreparation(operation))
    );
}

#[test]
fn recovery_of_a_dispatched_effect_that_never_answered_stays_unconfirmed() {
    let a = agent("agent-a");
    let mut ledger = ledger_for(&a);
    let operation = OperationId::new(run_id("run-1"), Turn::FIRST);

    ledger
        .append(Record::RunStarted {
            agent: a.id().clone(),
            run: run_id("run-1"),
        })
        .expect("登记活动开始");
    ledger
        .append(Record::ActionPrepared {
            run: run_id("run-1"),
            operation: operation.clone(),
        })
        .expect("登记尝试已经准备");
    ledger
        .append(Record::ActionDispatched {
            run: run_id("run-1"),
            operation: operation.clone(),
        })
        .expect("登记尝试已经发出");

    let recovered = recover(ledger.records()).expect("事实自洽");

    assert!(!recovered.is_ended(), "发出后没有结果，活动不算收场");
    assert_eq!(recovered.unconfirmed, vec![operation]);
    assert!(!recovered.may_dispatch_again());
}

// ---------------------------------------------------------------------------
// 边界守卫：Core 源码不得出现供应商 / Provider wire 语言，也不得出现外部依赖
// ---------------------------------------------------------------------------

/// 长且无歧义的词：直接做子串匹配，不会误伤正常词。
const FORBIDDEN_SUBSTRINGS: [&str; 27] = [
    // 旧 Runtime 术语
    "wakeup",
    "dialogue",
    "presence",
    "coordinator",
    "hosttool",
    "turndecision",
    "celira_runtime",
    // 供应商名
    "openai",
    "anthropic",
    "gemini",
    "qwen",
    "deepseek",
    "claude",
    // TASK_35：搜索那一家。主体的词只有 `web_search` / `web_fetch`，厂商名不许出现在任何
    // 被扫描的源文件里——端点是**配置**（入口给），方言锁在 web_app 自己那一层。
    "tavily",
    // 供应商 wire 字段
    "tool_calls",
    "tool_call",
    "function_call",
    "response_id",
    "thread_id",
    "chat_completion",
    "system_prompt",
    "max_tokens",
    "logprobs",
    "messages",
    "contents",
    // 通用边界词
    "provider",
    "vendor",
];

/// 短词：会出现在 `author` / `tokenizer` / `streamline` / `curl` 里，
/// 因此**不能**用子串匹配，必须走词边界匹配（见 `forbidden_word_hits`）。
const FORBIDDEN_WORDS: [&str; 7] = ["auth", "http", "https", "json", "stream", "token", "url"];

/// 子串扫描：返回源码里出现的禁用词。
fn forbidden_substring_hits<'a>(source: &str, forbidden: &[&'a str]) -> Vec<&'a str> {
    let lowered = source.to_lowercase();
    forbidden
        .iter()
        .copied()
        .filter(|word| lowered.contains(word))
        .collect()
}

/// 词边界扫描：只有整词命中才算违规。
///
/// `_` 也是词边界，所以 `auth_token` 会被拆成 `auth` + `token`（两个都算违规），
/// 而 `authority` / `tokenizer` / `streamline` / `curl` 不会被误伤。
fn forbidden_word_hits(source: &str, forbidden: &[&str]) -> Vec<String> {
    source
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|token| forbidden.contains(&token.as_str()))
        .collect()
}

/// 块级静默丢的机械扫描：`ContentBatch::of(...)` 之后**紧跟着**回退到别的批次，就是违规。
///
/// 判据按**形状**，不点名文件、不列白名单：
///
/// - 起点是 `ContentBatch::of(` 这个调用本身（**注释行不参与**：注释里提到这句调用不算违规，
///   但注释也不会被当成起点）；
/// - 终点是**同一个表达式**：从起点往后拼代码，遇到语句结束符 `;` 就停（最多看 8 行，
///   覆盖 `of(vec![\n …\n])` 这种跨行写法）；
/// - 窗口里出现 `unwrap_or` 家族（`unwrap_or` / `unwrap_or_default` / `unwrap_or_else`）⇒ 违规。
///   这正是"超限即退回空批次 / 退回另一份"的形状：内容装不下时那一整块**无声消失**。
///
/// 合格出路只有两条：把失败传出去（`?` / `map_err` / 显式 `match`/`if let Err`），
/// 或者用 `ContentBatch::of_or_not_delivered` 留一条读数。
fn silent_batch_fallbacks_in(source: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = source.lines().collect();
    let mut hits = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        // 注释行不当起点（`//` 开头的都是说明，不是代码）。
        if line.trim_start().starts_with("//") {
            continue;
        }
        if !line.contains("ContentBatch::of(") {
            continue;
        }
        let mut window = String::new();
        for next in lines.iter().skip(index).take(8) {
            // 行尾注释不参与（注释里写 `unwrap_or` 不算代码）。
            let code = next.split("//").next().unwrap_or("");
            window.push_str(code);
            if code.contains(';') {
                break;
            }
        }
        if window.contains("unwrap_or") {
            hits.push((index + 1, line.trim().to_string()));
        }
    }
    hits
}

/// 可执行入口区（`src/bin/`）：**产品入口与夹具/探针住在同一个前缀下**。
///
/// ⚠ **2026-10-10 改**：这个常量从前叫 `FIXTURE_BIN_PREFIX`（"入口夹具区"），理由是"夹具本来
/// 就要造目录、起进程、读环境文件——这些正是产品模块被禁止做的事，把它们当产品模块守卫只会
/// 逼夹具改措辞"。那个理由对**夹具**仍然成立，但它被**按整片前缀**执行了，而 `src/bin/` 里同时
/// 住着生产入口（常驻宿主、生产命令、操作者命令）⇒ 它们跟着一起免了。后果是具体的：
/// **"唯一循环"那条判据的射程不含 bin**（它走 `core_sources()`），第二个执行循环可以住在入口
/// 文件里而没有任何判据会红。
///
/// 今天的分工：整片前缀照旧不进 `core_sources()`（供应商语言、代词、比喻那些守卫对**入口**不
/// 适用——入口本来就是挑供应商、报读数的那一层），而**产品入口逐个点名进"唯一循环"的射程**。
/// 判据因此不是"哪个前缀"，而是**这个文件是不是产品的运行形态会启动的那一个**。
const BIN_ENTRY_PREFIX: &str = "bin";

/// `src/bin/` 下的**产品入口**：产品的运行形态会启动它们（第二列是理由）。
///
/// 进这张表只有一个后果，但它是实打实的：**它们进"唯一循环"那条判据的射程**
/// （见 `only_the_one_loop_writes_the_trace`）——入口不许自己往轨迹里写。
const PRODUCT_ENTRIES: [(&str, &str); 5] = [
    (
        "agent_host.rs",
        "常驻宿主入口：产品的运行形态就是它（家侧 `up` 起的就是这个）",
    ),
    (
        "agent_run.rs",
        "生产命令 `init` / `run`：操作者按文档敲的就是它",
    ),
    (
        "agent_host_status.rs",
        "产品宿主的只读状态命令：操作者判断「该不该出手」看的就是它",
    ),
    (
        "agent_host_acknowledge.rs",
        "宿主行政动作入口：显式确认并清除上一次保存失败留下的暂停标记",
    ),
    (
        "environment_diagnose.rs",
        "离线诊断命令：环境起不来时操作者跑的就是它",
    ),
];

/// `src/bin/` 下的**非产品入口**：夹具、探针、测试辅助（第二列是理由）。
///
/// 它们**不进**"唯一循环"的射程：这些文件本来就要造"产品模块被禁止做的形状"（孤儿操作、
/// 切分的操作、真实的写失败、真起一台外部 server），拿产品判据去量它们只会逼它们改措辞，
/// 守卫也会退化成用词约束。它们的正确性由各自的**真机 / 跨进程门禁**证明。
const NON_PRODUCT_ENTRIES: [(&str, &str); 5] = [
    (
        "manual_entry_fixture.rs",
        "离线验收夹具：把脚本模型 / 回环适配层 / 计数替身注入同一段生产启动代码",
    ),
    (
        "environment_worker.rs",
        "跨进程历史测试的辅助入口，不是常驻宿主",
    ),
    (
        "im_probe.rs",
        "执行端真机探针（只读）：消费者是 `scripts/home/probe-agent-im.js`，不是产品",
    ),
    (
        "mcp_fixture_server.rs",
        "MCP 夹具 server：给 `tests/mcp_app.rs` 当「外面那台 server」",
    ),
    (
        "write_refusal_fixture.rs",
        "写入被真拒绝的夹具（内核级写失败），只被 Unix 臂用",
    ),
];

/// **入口射程逐个点名，不按前缀整片豁免**（2026-10-10）。
///
/// 判据（派生）：`src/bin/` 下**每一个** `.rs` 都必须在 [`PRODUCT_ENTRIES`] 或
/// [`NON_PRODUCT_ENTRIES`] 里挑一张，而且**写明理由**。新加一个可执行文件 ⇒ 这条判据红，
/// 逼你回答"它是产品入口，还是只为证明某条路径而存在"——**而不是靠人记得回来补名单**。
#[test]
fn every_bin_entry_is_named_with_a_reason() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(BIN_ENTRY_PREFIX);
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_rust_sources(&root, &mut paths);
    let mut on_disk: Vec<String> = paths
        .iter()
        .map(|path| {
            path.strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    on_disk.sort();

    assert!(
        on_disk.len() >= 2,
        "{} 下只认到 {} 个 .rs 文件 —— 这条判据在空跑",
        root.display(),
        on_disk.len()
    );

    // 两张表各自：名字必须在盘上（名单过期要红）、理由必须写（空理由等于没写）。
    for (table, entries) in [
        ("PRODUCT_ENTRIES", PRODUCT_ENTRIES.as_slice()),
        ("NON_PRODUCT_ENTRIES", NON_PRODUCT_ENTRIES.as_slice()),
    ] {
        assert!(!entries.is_empty(), "{table} 是空的 —— 这条判据在空跑");
        for (file, reason) in entries {
            assert!(
                on_disk.iter().any(|each| each == file),
                "{table} 里的 {file} 不在 {} 下：名单过期了",
                root.display()
            );
            assert!(
                !reason.trim().is_empty(),
                "{table} 里的 {file} 没写理由 —— 两张表的分工就是「凭什么」"
            );
        }
    }

    // 每个文件**恰好**在一张表里：漏了 ⇒ 新入口逃过射程；两边都占 ⇒ 分工说不清。
    for file in &on_disk {
        let product = PRODUCT_ENTRIES.iter().any(|(each, _)| each == file);
        let exempt = NON_PRODUCT_ENTRIES.iter().any(|(each, _)| each == file);
        assert!(
            product ^ exempt,
            "{BIN_ENTRY_PREFIX}/{file} 在两张表里{} —— 每一个可执行文件必须恰好在一张表里、并写明理由",
            if product {
                "都出现了（分工说不清）"
            } else {
                "一张都没出现（新入口逃过射程）"
            }
        );
    }
}

/// `src/bin/` 下**产品入口**的源文件（`bin/<名>` + 内容）——只读 [`PRODUCT_ENTRIES`] 点名的那些。
///
/// 存在的理由只有一个：让**唯一循环**那条判据看得见入口文件（见 [`PRODUCT_ENTRIES`]）。
/// 名字形状与 `core_sources()` 一致（相对 `src/`），因此报红时点的是 `bin/agent_host.rs:NNN`。
fn product_entry_sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(BIN_ENTRY_PREFIX);
    PRODUCT_ENTRIES
        .iter()
        .map(|(file, _)| {
            let path = root.join(file);
            let text = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("读不到 {}: {error}", path.display()));
            (format!("{BIN_ENTRY_PREFIX}/{file}"), text)
        })
        .collect()
}

/// 拒绝/失败的**分类词**不许是动态串（形状扫，TASK_72）。
///
/// 词表纪律（AGENTS.md 三条，2026-09-16）：分类是一个**稳定标记**，判据是**同一故障两次发生
/// 该字符串逐字节相同**。随单内容可以放在**第二个实参**上（那是数据），但**第一个实参**一旦出现
/// `format!` / `concat!` / `+` / `{}`，分类就随内容变了——那一刻它不再是词表，而是一条消息。
///
/// 返回 (行号, 那一行)。
fn dynamic_class_hits(source: &str) -> Vec<(usize, String)> {
    const CALLS: [&str; 4] = [
        "ActionAdmission::refused_with(",
        "ActionAdmission::refused(",
        "ActionOutcome::failure_with_content(",
        "ActionOutcome::failure(",
    ];
    let mut hits: Vec<(usize, String)> = Vec::new();
    for call in CALLS {
        let mut from = 0usize;
        while let Some(found) = source[from..].find(call) {
            let at = from + found;
            let open = at + call.len() - 1;
            let argument = first_argument(source, open);
            if ["format!", "concat!", "+", "{", "}"]
                .iter()
                .any(|marker| argument.contains(marker))
            {
                let line = source[..at].lines().count();
                let text = source[..at]
                    .rsplit_once('\n')
                    .map(|(_, tail)| tail)
                    .unwrap_or(&source[..at]);
                hits.push((line, text.trim().to_string()));
            }
            from = open + 1;
        }
    }
    hits
}

/// 一段源码里 `(` 起的**第一个实参**文本（到第一个顶层逗号或配对右括号为止）。
fn first_argument(source: &str, open: usize) -> String {
    let bytes = source.as_bytes();
    let mut depth = 1usize;
    let mut index = open + 1;
    let start = index;
    while index < bytes.len() {
        match bytes[index] as char {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return source[start..index].to_string();
                }
            }
            ',' if depth == 1 => return source[start..index].to_string(),
            _ => {}
        }
        index += 1;
    }
    source[start..].to_string()
}

/// **"唯一循环"必须是机械可判的**（2026-10-08，修 §4「"唯一循环"没有类型级拦阻」那一格）。
///
/// `drive_run` 与 `Run` 的六个轨迹写入口（`accept_injection` / `accept_response` / `accept_refusal` /
/// `prepare` / `mark_dispatched` / `observe`）今天都是 `pub`——"唯一"靠**调用纪律**，不靠可见性。
///
/// ⚠ **为什么不能只靠可见性（实测）**：`pub` 是**被测试逼出来的**，不是产品需要——
/// `provider_adapter.rs` 那些调用在 `#[cfg(test)] mod tests` 里（库内用例），而
/// `tests/provider_wire.rs` / `tests/request_recomputability.rs` 是**集成用例**（独立 crate ⇒
/// 只能看 `pub`），它们要造**驱动循环造不出来的形状**（孤儿操作、切分的操作）⇒ 收成 `pub(crate)`
/// 会打断它们。⇒ 这一刀把纪律变成**判据**：`src/` 里这六个入口的**生产**调用者**只许有 `drive_run`**。
///
/// ⚠ 它**不是类型级拦阻**（那是 §4 里登记的另一刀：把手工搭轨迹的集成用例搬进库内，才收得紧可见性）。
/// 它抓的是**第二个循环长出来**——那一刻**账会有第二个写者**，而这是条件级的。
///
/// ⚠ **2026-10-10 扩射程两处**（都是"看着盖住了其实没盖住"的形状）：
/// ① **射程含产品入口**——从前它走 `core_sources()`，而那一份**整片跳过 `src/bin/`**
///    （见 [`BIN_ENTRY_PREFIX`]）⇒ 入口文件里可以长第二个循环；
/// ② **两种调用形态都认**——只认 `.prepare(` 时，`Run::prepare(`（完全限定写法）溜得过去，
///    而"六个入口每一个都至少被找到一次"那条下界**照旧满足**，所以它拦不住。
#[test]
fn only_the_one_loop_writes_the_trace() {
    const ENTRIES: [&str; 6] = [
        "accept_injection",
        "accept_response",
        "accept_refusal",
        "prepare",
        "mark_dispatched",
        "observe",
    ];

    let mut seen: Vec<(String, String, usize, String)> = Vec::new();
    // **射程含产品入口**（2026-10-10）：`core_sources()` 跳过整片 `src/bin/`（见
    // [`BIN_ENTRY_PREFIX`] 的理由），而生产入口住在那里 ⇒ 从前第二个执行循环可以在入口文件里
    // 安家而没有任何判据会红。夹具 / 探针仍然不进射程（它们本来就要造产品模块被禁止做的形状）。
    let mut scanned = core_sources();
    scanned.extend(product_entry_sources());
    for (name, text) in scanned {
        let production = without_test_modules(&text, &name);
        for entry in ENTRIES {
            // **两种调用形态都要认**（2026-10-10）：方法调用 `.prepare(`，以及**完全限定**的
            // `Run::prepare(`。只认前者时换一个写法就能绕过这条判据——而"六个入口每一个都必须
            // 至少被找到一次"那条下界照旧满足（原来的调用点还在），所以它拦不住。
            // 定义行（`pub fn prepare(`）两种形态都不含，因此扩这一条不会假红。
            for pattern in [format!(".{entry}("), format!("::{entry}(")] {
                for (at, _) in production.match_indices(&pattern) {
                    let owner = enclosing_fn(&production, at)
                        .unwrap_or_else(|| panic!("{name}: 找不到 `{pattern}` 所在的那个函数"));
                    let line = production[..at].lines().count();
                    seen.push((entry.to_string(), owner, line, name.clone()));
                }
            }
        }
    }

    // 下界：六个入口**每一个**都必须至少被找到一次，否则这条判据在空跑。
    for entry in ENTRIES {
        assert!(
            seen.iter().any(|(each, ..)| each == entry),
            "`{entry}` 一处调用点都没找到 —— 这条判据在空跑（形状变了就先改它）"
        );
    }

    let strays: Vec<&(String, String, usize, String)> = seen
        .iter()
        .filter(|(_, owner, ..)| owner != "drive_run")
        .collect();
    assert!(
        strays.is_empty(),
        "这六个轨迹写入口的**生产**调用者只许是 `drive_run` —— 多出来的就是**第二条循环**：\n{}",
        strays
            .iter()
            .map(|(entry, owner, line, file)| format!(
                "      {file}:{line}  {owner}(…) 调了 {entry}(…)（方法调用与完全限定都算）"
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// 砍掉 `#[cfg(test)] mod tests { … }` 整块（按**括号配对**，不是按"第一个 `#[cfg(test)]` 就截断"）。
///
/// 自检在函数里：`#[cfg(test)]` 后面**必须**是 `mod tests`。若哪天它挂到了某个函数上，
/// "砍掉测试"会顺手砍掉生产代码——那时**先改这个守卫**，别让它悄悄少盖一块。
fn without_test_modules(text: &str, name: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("#[cfg(test)]") {
        out.push_str(&rest[..at]);
        let after = &rest[at..];
        let head: Vec<&str> = after.lines().take(4).collect();
        assert!(
            head.iter()
                .any(|line| line.trim_start().starts_with("mod tests")),
            "{name} 里的 `#[cfg(test)]` 后面不是 `mod tests`（前四行：{head:?}）—— \
             本守卫按模块形状砍测试，形状变了先改它"
        );
        let open = after.find('{').expect("测试模块的开括号");
        let bytes = after.as_bytes();
        let mut depth = 0usize;
        let mut index = open;
        while index < bytes.len() {
            match bytes[index] as char {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        index += 1;
                        break;
                    }
                }
                _ => {}
            }
            index += 1;
        }
        rest = &after[index..];
    }
    out.push_str(rest);
    out
}

/// 一段源码里某个位置**所属的那个函数名**（往前找最近的 `fn 名字(` 或 `fn 名字<`）。
fn enclosing_fn(text: &str, at: usize) -> Option<String> {
    let head = &text[..at];
    let mut found = None;
    for (index, _) in head.match_indices("fn ") {
        let after = &head[index + 3..];
        let name: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() {
            continue;
        }
        let rest = after[name.len()..].trim_start();
        if rest.starts_with('(') || rest.starts_with('<') {
            found = Some(name);
        }
    }
    found
}

/// **首轮那一拍的前提必须有人钉住**（2026-10-08，修"在场判据有第二处"那一刀）。
///
/// 门后那句「还没打开」现在由 **`AppDoor::preinstalled`**（装配期事实）决定，而**不是**由
/// "它的动词有没有出现在能力面里"反推。这条判据成立**全靠一个前提**：
/// **动作目录只在首轮渲染**（`is_opening_turn`）——那一刻还没有任何展开，所以"开着的"
/// **恰好就是常开的那几个**。
///
/// 有人让目录在**别的轮次**也渲染（那本身可能是对的）⇒ **这一条当场红**，逼他把判据换成
/// **活的展开状态**，而不是让首屏开始撒谎（"开着"却说"还没打开"）。
#[test]
fn the_action_directory_is_rendered_only_on_the_opening_turn() {
    let (_, text) = core_sources()
        .into_iter()
        .find(|(name, _)| name == "composition.rs")
        .expect("composition.rs 在扫描面里");

    // 一处定义 ＋ 一处调用。多出来的调用点就是"这个前提变了"的信号。
    let sites: Vec<usize> = text
        .match_indices("action_directory_batch(")
        .map(|(at, _)| at)
        .collect();
    assert_eq!(
        sites.len(),
        2,
        "`action_directory_batch` 应当只有一处定义 ＋ 一处调用，实得 {} 处 —— \
         新增调用点之前先读 `AppDoor::preinstalled` 的文档：门后那句「还没打开」的判据\
         只在**首轮**成立",
        sites.len()
    );

    // 定义那一处的正前方是 `fn`；调用点不是。
    let call_sites: Vec<usize> = sites
        .iter()
        .copied()
        .filter(|at| !text[..*at].trim_end().ends_with("fn"))
        .collect();
    assert_eq!(
        call_sites.len(),
        1,
        "分不出哪一处是调用点（`fn action_directory_batch` 的形状变了）—— 实得 {} 处非定义",
        call_sites.len()
    );
    let at = call_sites[0];
    // ⚠ 按**行**判，不按字节窗口判：源码里全是中文，按字节切会切在多字节字符中间（实测 panic 过）。
    let line_at = |offset: usize| text[..offset].lines().count();
    let call_line = line_at(at);
    let opening_line = line_at(
        text.find("match context.is_opening_turn()")
            .expect("composition.rs 里找不到 `match context.is_opening_turn()` —— 形状变了"),
    );
    assert!(
        call_line > opening_line && call_line <= opening_line + 8,
        "那一处调用**不在** `is_opening_turn()` 为真的分支里（第 {call_line} 行 vs 第 {opening_line} 行）\
         —— 而门后「还没打开」的判据（`AppDoor::preinstalled`）只在首轮等于「开着的」。\
         要么把判据换成活的展开状态（`RunScope::is_open`），要么别在别的轮次渲染目录。"
    );
    assert!(
        text[at..]
            .lines()
            .take(8)
            .any(|line| line.contains("false =>")),
        "那一处调用后面 8 行里没有 `false =>` 那一支 —— 形状变了，这条守卫看不见它了"
    );
}

/// `agent/src/` 下全部 `.rs` 文件（**相对 `src/` 的路径** + 内容），**不含入口夹具区**。
///
/// 目录取自 cargo 在**编译期**提供的 `CARGO_MANIFEST_DIR`，因此既不写死机器路径，
/// 也不依赖测试进程的当前工作目录。**递归**枚举：将来新增子目录模块也必须被扫描，
/// 不允许用浅层枚举逃过守卫（下面"每个文件要么被扫描、要么明确列在边界名单里"的断言
/// 依赖这一点）。符号链接目录不会被跟进，避免扫描被引到源码树之外或死循环。
///
/// 唯一被跳过的顶层区域是 [`BIN_ENTRY_PREFIX`]（`src/bin/`）——**入口区**（产品入口与夹具/探针
/// 混住）。跳过的理由与后果写在那个常量的文档里；**唯一循环那条判据例外**：它把
/// [`PRODUCT_ENTRIES`] 点名的文件显式加回射程（[`product_entry_sources`]）。
fn core_sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_rust_sources(&root, &mut paths);
    paths.retain(|path| {
        let relative = path.strip_prefix(&root).unwrap_or(path);
        !relative.starts_with(BIN_ENTRY_PREFIX)
    });
    paths.sort();

    assert!(
        !paths.is_empty(),
        "{} 下必须至少有一个 .rs 文件，否则守卫形同虚设",
        root.display()
    );

    paths
        .into_iter()
        .map(|path| {
            // 名字里的分隔符统一成 `/`：**子目录模块**（`mcp/…`）因此与平台无关——
            // 名单只写一种形状，守卫不会因为换一个平台就要求另一份名字。
            let name = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let text = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("读不到 {}: {error}", path.display()));
            (name, text)
        })
        .collect()
}

/// 递归收集 `.rs` 文件。
fn collect_rust_sources(directory: &Path, paths: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("读不到 {}: {error}", directory.display()));

    for entry in entries {
        let path = entry.expect("目录项可读").path();
        let file_type = fs::symlink_metadata(&path).expect("可读元数据").file_type();
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_rust_sources(&path, paths);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            paths.push(path);
        }
    }
}

// ---------------------------------------------------------------------------
// 声音纪律（2026-09-25 立）
// ---------------------------------------------------------------------------

/// **声音纪律**：`agent/src/` 里**任何一行**都不出现第三人称单数女性代词。
///
/// **原约束**：仓库的散文层习惯用那个人称代词讲一个具体的 Agent——那是**解释层**的叙述视角，
/// 跟 `Bed`/`Desk`/`House` 那类比喻同级。
/// **新边界**：它**不许**出现在 `agent/src/` 的任何地方——标识符、稳定字符串、能力说明、
/// 回执、CLI 帮助，**以及注释**。要面对任意 Agent（"哥哥"也在内）；而一个代词同时断言了三件事
/// ——性别、单一连续人格、可被第三人称叙述——**这三件都不是机械事实**。
/// **真实消费者**：能力说明与回执会被渲染进 Agent 的视野（`terminal_app` 的参数说明、
/// `notes_app` 的删除回执、`os.rs` 的起源句都曾经中过），CLI 帮助面向操作者。
///
/// **本文件自己不许出现那个字**：它写成转义常量 [`PRONOUN_SHE`]，夹具用 [`with_she`] 拼。
/// 这不是洁癖——守卫要是写得出它，就会被"清一遍"的机械替换顺手改掉，守卫当场失效。
/// **这件事真实发生过一次**（2026-09-25 清注释层那一刀），代价是这一格整段重写。
///
/// **为什么只扫 `src/`**：`tests/` 里的夹具**必须**能写出这个字才能证明守卫拦得住它，
/// 因此测试目录不在扫描面上。这条边界是**故意的**，不许把它读成"测试里可以随便写"。
///
/// **`它` 不在此列**：它指物是正常的中文，本守卫只拦**人称**代词
/// （[`PRONOUN_SHE`] / `他` / `she` / `her` / `hers`）。但"拿 `它` 指 Agent"是另一种毛病
/// （把主体降成物），它不可机械判定，因此只写在这里。
#[test]
fn no_person_pronouns_in_agent_sources() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_rust_sources(&root, &mut paths);
    paths.sort();
    assert!(
        !paths.is_empty(),
        "{} 下必须至少有一个 .rs 文件，否则守卫形同虚设",
        root.display()
    );

    let mut offenders: Vec<String> = Vec::new();
    for path in &paths {
        let text = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("读不到 {}: {error}", path.display()));
        let name = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        for (index, line) in text.lines().enumerate() {
            if let Some(pronoun) = person_pronoun_in(line) {
                offenders.push(format!(
                    "{name}:{}: 「{pronoun}」 {}",
                    index + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "出现人称代词（声音纪律）：\n{}",
        offenders.join("\n")
    );
}

/// **不许伪造身份标识**（2026-10-06 立）。
///
/// 判据一句话：**`RunId` / `AgentId` 是"已经发生的那件事"的标识，不是占位符。**
/// 空字符串的标识在盘上、在归属索引里、在审计里都是一个**看起来合法**的值——它不报错，
/// 只是让"这是哪一次"从此没有答案。
///
/// 这一条是**拿真实违规换来的**：`src/timer_app.rs` 从前在**装配期**（根本没有 Run）为了走
/// 一条带 Run 作用域的接缝，写了 `ActionPort::capabilities(self, &AgentId::new(""),
/// &RunId::new(""))`——**全 crate 唯一一处**。它不报错、也不影响行为（那个实现忽略参数），
/// 因此**别的守卫都看不见它**；同族的 App 走的是不依赖 Run 的入口。
///
/// 出路有两条：**要么走不依赖身份的那条接缝**（`own_capabilities` /
/// `available_capabilities`），**要么把这个能力声明成它真的需要一个身份**——
/// 不许拿一个空值把类型哄过去。
#[test]
fn no_app_forges_an_activity_identity() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_rust_sources(&root, &mut paths);
    paths.sort();
    assert!(
        !paths.is_empty(),
        "{} 下必须至少有一个 .rs 文件，否则守卫形同虚设",
        root.display()
    );

    // 自检：判据**真的会红**，也**不误报**（免得守卫写成永远为真）。
    assert!(
        flags_forged_identity("let id = RunId::new(\"\");"),
        "判据自己失效了：这一行必须被认出来"
    );
    assert!(
        !flags_forged_identity("let id = RunId::new(\"run-1\");"),
        "判据不许误报：正常的标识不是伪造"
    );

    let mut offenders: Vec<String> = Vec::new();
    for path in &paths {
        let text = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("读不到 {}: {error}", path.display()));
        let name = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        for (index, line) in text.lines().enumerate() {
            if flags_forged_identity(line) {
                offenders.push(format!("{name}:{}", index + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "拿空字符串伪造身份标识（`RunId` / `AgentId` 是事实的标识，不是占位符）：\n{}",
        offenders.join("\n")
    );
}

/// 这一行是不是在伪造身份标识。**行注释不算**——讲这条规矩本身要能提它。
fn flags_forged_identity(line: &str) -> bool {
    const FORGERIES: [&str; 2] = ["RunId::new(\"\")", "AgentId::new(\"\")"];
    let code = line.split("//").next().unwrap_or("");
    FORGERIES.iter().any(|forgery| code.contains(forgery))
}

/// 被禁的那个人称代词，**写成转义**（见守卫文档：本文件自己不许出现它）。
const PRONOUN_SHE: char = '\u{5979}';

/// 把夹具里的 `{}` 换成那个字——夹具因此不必在源码里写出它。
fn with_she(text: &str) -> String {
    text.replace("{}", &PRONOUN_SHE.to_string())
}

/// 这一行里出现的**人称**代词（没有就是 `None`）。
///
/// 中文**只认** [`PRONOUN_SHE`]：另一个第三人称代词"他"不进这一格，因为它在中立词里到处都是
/// ——`其他` / `排他` / `他方`——机械判据分不开，硬拦就是"用裸子串误报中立词汇"。
/// 这是**有意的取舍**：宁可漏一个不常用的代词，也不让守卫天天喊狼来了。
///
/// 英文按**词边界**匹配。左边界：前一个字符不是 ASCII 字母数字即算边界（`_` **算边界**），
/// 因此 `her_view` 会被抓，`other` / `whether` 里的 `her` 不会。
/// 右边界分三档（这一格是唯一需要想清楚的地方）：
/// - 行尾，或后一个字符不是 ASCII 字母数字（`_`、空格、引号、点）⇒ **边界**；
/// - 后一个字符是小写字母或数字 ⇒ **同一个词**（`Shelf` / `Sherry` / `powershell`）；
/// - 后一个字符是**大写**：再看它后面那一个——是小写就是驼峰换词（`HerOwn` ⇒ 边界），
///   还是大写就是全大写词（`DEFAULT_SHELL` / `DECISION_SHEET` ⇒ 同一个词）。
///
/// 实现上**在小写副本里找位置、在原文里判边界**：`to_ascii_lowercase` 只改 ASCII 且不改变
/// 字节长度，因此两边的下标可以直接对齐；而边界必须看**原文**，否则 `HerOwn` 会被当成
/// `herown` 而漏掉、`DEFAULT_SHELL` 又会被当成两个词而误报。
fn person_pronoun_in(line: &str) -> Option<&'static str> {
    if line.contains(PRONOUN_SHE) {
        return Some("she-zh");
    }
    let lowered = line.to_ascii_lowercase();
    for word in ["she", "her", "hers"] {
        for (at, _) in lowered.match_indices(word) {
            let before = line[..at].chars().next_back();
            let mut after = line[at + word.len()..].chars();
            let sticks_left = before.is_some_and(|c| c.is_ascii_alphanumeric());
            let sticks_right = match after.next() {
                None => false,
                Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => true,
                Some(c) if c.is_ascii_uppercase() => {
                    after.next().is_some_and(|next| !next.is_ascii_lowercase())
                }
                Some(_) => false,
            };
            if !sticks_left && !sticks_right {
                return Some(word);
            }
        }
    }
    None
}

/// 守卫自己的**正反例**（守卫也要能判错：判据写错时这条先红）。
#[test]
fn the_voice_guard_separates_comments_from_product_text() {
    // 反例：任何一行都不行——注释、字符串、参数说明、CLI 帮助一视同仁。
    assert_eq!(
        person_pronoun_in(&with_she("/// {}自己的笔记，跨清醒期保留")),
        Some("she-zh")
    );
    assert_eq!(person_pronoun_in(&with_she("//! {}是谁")), Some("she-zh"));
    assert_eq!(
        person_pronoun_in(&with_she(" * 块注释续行里的{}")),
        Some("she-zh")
    );
    assert_eq!(
        person_pronoun_in(&with_she(r#"format!("{{id}} 从{}的存储里除名了")"#)),
        Some("she-zh")
    );
    assert_eq!(
        person_pronoun_in(&with_she(r#"    "不给就用{}的环境根","#)),
        Some("she-zh")
    );
    assert_eq!(
        person_pronoun_in(&with_she("  --materials 显式授权给{}的资料根")),
        Some("she-zh")
    );
    assert_eq!(
        person_pronoun_in("fn her_view_never_contains_vendor_words() {"),
        Some("her")
    );
    assert_eq!(person_pronoun_in("a time she set for herself"), Some("she"));
    assert_eq!(
        person_pronoun_in("let owner = HerOwn::default();"),
        Some("her")
    );

    // 不许误伤中立词汇：这一格最容易喊狼来了，因此正例要具体到本仓库真实出现过的词。
    assert_eq!(person_pronoun_in("let shelf = Shelf::new();"), None);
    assert_eq!(person_pronoun_in("其他房间不动"), None);
    assert_eq!(person_pronoun_in("先取排他写资格"), None);
    assert_eq!(
        person_pronoun_in(r#"const DEFAULT_SHELL: &str = "powershell.exe";"#),
        None
    );
    assert_eq!(
        person_pronoun_in("见 `outputs/AGENT_NEXT_STEP_DECISION_SHEET.md`"),
        None
    );
    assert_eq!(person_pronoun_in("whether 与 together 里都有"), None);
    assert_eq!(person_pronoun_in(&with_she("{}的视野")), Some("she-zh"));
}

/// 阶段二起，crate 内多了一个 **Adapter 文件**：它本来就该说供应商语言，
/// 因此不参与「Core 语言」扫描。这个名单是**封闭**的——只有列在这里的文件被跳过，
/// 其余 `src/*.rs` 一律扫描；下面还会断言这些文件真的存在、且真的含有供应商语言
/// （否则"排除"就成了掩盖）。
/// 适配层源文件：**封闭名单**。它是"允许说 wire 语言"的地方，也是"公开面受检"的地方。
///
/// TASK_44 起有三件（两条线协议 + 入口的选择）：`responses_adapter.rs` 是第二条 wire（Responses），
/// `model_choice.rs` 是"入口只要一个模型"那一处选择（两个实现互不认识，是它同时认识两者）。
const ADAPTER_SOURCES: [&str; 3] = [
    "provider_adapter.rs",
    "responses_adapter.rs",
    "model_choice.rs",
];

/// 允许直接说**网络/序列化**语言（`reqwest` / `serde_json`）的源文件：**封闭名单**。
///
/// 这份名单原来只有供应商适配层一个文件。TASK_21 起多了 `im_app.rs`：它是第一条**跨系统
/// 边界的只读通道**（对面是 Celira 自己的 im 服务端，不是供应商），同样必须自己发请求、
/// 自己读响应。这是一次**必须被人看见**的变化：新文件要进来就得改这份名单——改不了，
/// 就说明它不该自己说网络语言。
///
/// TASK_26 起多了 `doorbell.rs`：**门铃**——宿主侧那条唤醒长连接的客户端。它与上两个的理由
/// 不同：不是"某个 App 要读外部世界"，而是"**宿主需要一个叫醒主体的入口**"，而那条入口是一条
/// 由 im 服务端持有的长连接（`CORE_V1_AGENT_PROTOCOL.md` §8）。它同样只搬机械事实（一个事件名），
/// 解析都不需要，也就不可能把事实漏进事实系统。
///
/// TASK_35 起多了 `web_app.rs`：**web 窗**——外面那个世界的两条只读动词。它必须自己发请求、
/// 自己读响应，理由与 `im_app.rs` 同类（"某个 App 要读外部世界"），只是对面不是我们的服务端，
/// 而是**任何一个站点**与一家搜索厂商。它搬回来的东西**一律是外部内容**（不是我们的任何事实），
/// 而且五堵机械墙全部钉在它自己这一层。
const NETWORK_SOURCES: [&str; 6] = [
    "provider_adapter.rs",
    // TASK_44：第二条 wire（Responses）同样自己发请求、自己读响应——它自己的九类失败与
    // 四种机械墙（`store: false` / 相邻配对 / 状态非 completed 即拒 / 平铺 usage）钉在它自己那层。
    "responses_adapter.rs",
    "im_app.rs",
    "doorbell.rs",
    "web_app.rs",
    // **2026-10-02 im 拆包**：那条通道的**机械搬运**从 `im_app.rs` 搬到中立层 `im_channel.rs`——
    // 三个 im 包（通信 / 社交 / 资料）共用它，而 App 之间不得有实现依赖。自己发请求、自己读响应
    // 这件事因此**多了一个文件**（不是变成"随处可用"）：按同一份纪律显式列入。
    "im_channel.rs",
];

/// Core 自己实现的两个文件：它们不得引用 Adapter / App / OS，也不得引用它们的依赖。
///
/// ⚠️ 这两条**一起**扫，是因为它们对"上面那些层"的态度完全相同。但**它们彼此之间的方向**
/// 不在这张名单的射程里——把两个文件当一坨，就问不出"这一坨内部谁依赖谁"。那一条由
/// [`foundation_never_knows_core`] 单独钉（`AGENTS.md`："Foundation 不依赖 Core"）。
const CORE_SOURCES: [&str; 2] = ["core.rs", "foundation.rs"];

/// 边界模块名：Core 与 Foundation 一个都不许引用（App 不认识 OS，OS 只认识接入契约）。
///
/// `store` 是身份环境与只追加存储；`file_view` 是"资料 + 派生经历目录"的只读视图。
/// Core / Foundation 同样不许认识它们——Core 只通过中立的 `CommitSeam` 说话，
/// 不认识环境目录、文件视图或存储格式。
///
/// `diagnostics` 是只读诊断层：它把已确认的持久事实重读成操作读数，并调用存储层的只读
/// 转换。Core / Foundation 一样不许认识它——"能重读出什么"不该进入执行语义，否则"读到的
/// 事实"迟早会被当成"可以据此继续"的依据。
///
/// `manual_entry` 是手动运行入口：它做显式初始化、启动一次活动并如实分类结果，认识存储与
/// OS 环境来源，也认识组合入口。Core / Foundation 一样不许认识它——"现在谁来启动"不属于
/// 执行语义，否则"调用方怎么用"迟早会被写进循环里。
///
/// `notes_app` 是第二个真实 App：Core / Foundation 一样不许认识它——"这个 App 有什么业务"
/// 不属于执行语义。
///
/// `creation_grant` 是那份窄授权本身（谁、哪次活动、哪个目录）：Core / Foundation 一样不许认识
/// 它——"谁被允许写"不该进入执行语义。
///
/// `status_bar` 是**系统服务层的第一个件**：它只实现上下文注入口，把"此刻主体的处境"每轮重摆。
/// Core / Foundation 一样不许认识它——"现在几点、预算用到哪儿"是**环境内容**，不是执行语义；
/// Core 只认识那个中立接缝（`ContextSource`），不认识任何一个具体来源。
///
/// `im_app` 是**第三个真实 App**（第一条跨系统边界的只读通道）：Core / Foundation 一样不许
/// 认识它——"哪个房间、哪条消息"是外部世界的内容，不是执行语义。
///
/// `doorbell` 是**门铃**（宿主侧那条唤醒长连接的客户端）：Core / Foundation 一样不许认识它——
/// "谁把主体叫醒的"是**宿主**的事，不是执行语义；执行语义只认识"这次活动怎么被发起的"那句
/// 事实（起源），而那句话由 OS 的环境来源注入，不由门铃直接注入。
///
/// `host_loop` 是**常驻宿主的最小件**（等一次唤醒、跑一次活动、回监听）：Core / Foundation 一样
/// 不许认识它——"什么时候开始一次活动"不属于执行语义，否则"谁在叫醒主体"迟早会被写进循环里。
///
/// `notification_bar` 是**系统服务层的第二个件**（通知栏）：Core / Foundation 一样不许认识它——
/// "外面有什么新的"是**世界**的内容，不是执行语义；Core 只认识那个中立接缝（`ContextSource`）。
/// 它**不读网络**（读是宿主的事），因此不在网络名单里。
///
/// `pins_board` 是**系统服务层的第三个件**（组件区）：Core / Foundation 一样不许认识它——
/// 「墙上贴着哪几张纸条」是主体**自己贴的**呈现，不是执行语义。它既不读网络、也不读 Agent 的存储
/// （读是装配期一次性只读的事），因此不在网络名单里。
///
/// `event_cards` 是**系统服务层的第四个件**（事件卡片）：Core / Foundation 一样不许认识它——
/// 「本次 Run 里世界又动了什么」不是执行语义。它自己**不读网络**（铃驱动的那次拉取由宿主做、
/// 写进细胞，本件只读细胞），因此不在网络名单里。
///
/// **匹配方式**：这批名字里有 `os` 这样的短名，因此比对时**不按裸子串**，只认
/// `crate::<模块>` 与独立标识符两种形态（见 `boundary_apis_stay_in_their_own_layer`）。
///
/// `mcp` 是**外部来源的 App**（TASK_42）：一台被授权的 stdio MCP server 的动词面 + 通道 +
/// 调用路径。Core / Foundation 一样不许认识它——"外面那台 server 有什么工具"是外部内容，
/// 不是执行语义。它是**第一个子目录模块**，因此名单里的名字按 `mcp/…` 写（见 `core_sources`
/// 的分隔符归一）。
///
/// ⚠ **这张表不许靠"有人记得"维护**（2026-10-18）：它必须与**派生集**逐名对齐，由
/// [`every_module_is_accounted_for_in_the_layering_ledger`] 机械检查——新模块会自动要一个说法，
/// 账本里留着一个不存在的模块也会红。**代价的来历**：这份名单手抄了两次都漏项
/// （2026-10-10 一次补 11 项；2026-10-11 落的两件又漏了），缺的从来不是记性。
const BOUNDARY_MODULES: [&str; 47] = [
    "provider_adapter",
    "file_app",
    "file_view",
    "os",
    "store",
    "diagnostics",
    "doorbell",
    "host_loop",
    "notification_bar",
    "manual_entry",
    "creation_grant",
    "notes_app",
    "recovery",
    "status_bar",
    "im_app",
    "pins_board",
    "event_cards",
    "web_app",
    "terminal_app",
    "mcp",
    // TASK_44：第二条线协议与"入口的选择"都不许进 Core / Foundation——"用哪一条 wire"是
    // 装配期事实，不是执行语义（Core 只认识那个中立接口）。
    "responses_adapter",
    "model_choice",
    // **逐轮读数账**那一刀：`<环境根>/model_readings.ndjson` 记的是"每一次模型响应报了什么"，
    // 它服务的是**事后翻文件的操作者**，不是执行语义——Core / Foundation 一样不许认识它
    // （Core 连"有没有这份账"都不知道）。按"搬出去就得跟着进名单"那条纪律显式列入。
    "model_readings",
    // TASK_77：**模型配置门**（环境根下的私有配置文件）同样不许进 Core / Foundation——
    // "模型凭据从哪儿来"是装配期事实，不是执行语义。
    "model_source",
    // TASK_79：**环境根里的两份出厂文档**（出厂说明 ＋ 手册）同样不许进 Core / Foundation——
    // "环境根里有哪两份文件、读回哪一态"是装配期事实，不是执行语义。Core 只认识那条中立的接缝：
    // 装配把静态头装订给模型（`ModelPort::declare_static_head`），Core 连它的内容都不知道。
    "orientation",
    // **持有物**那一刀：主体的持有物是**处境读数**，不是执行语义——Core 只认识"有一批内容要注入"。
    "holdings",
    // **中立零件层**那一刀：这两样从前住在 `status_bar.rs`（因此受同一条守卫覆盖），搬出去之后
    // 必须**跟着进名单**，否则"Core 不认识显示区"这条约束会从新家的门上漏掉。
    // Core 的语义是轮次而不是墙上时间（`InjectionContext` 给 `turn`，不给钟），
    // 屏幕占用更是纯粹的展示读数。
    "clock",
    "context_reading",
    // **机器处境**那一刀：主体读到的那一条是**处境读数**（这台机器是什么），不是执行语义——
    // Core 只认识"有一批内容要注入"，不认识平台、也不认识 shell。
    "machine",
    // **平台缝**那一刀：两个消费者（主体的手、MCP 的通道）共用的臂都在那里——默认 shell 名、
    // 怎么拼命令行、怎么击杀进程树、凭据怎么洗。Core / Foundation 一样不许认识它——
    // "这一份构建里编进了哪些臂、进程怎么拼"是装配与执行基础设施，不是执行语义。
    "platform",
    // **2026-10-02 im 拆包**：im 从"整包"分成三个包（通信 `im_app` / 社交 `im_social_app` /
    // 资料 `im_profile_app`），共用的通道机制落在中立层 `im_channel`。四个都是 App 层／中立层的
    // 模块，Core / Foundation 一样不许认识——"哪几条动词归哪个包、怎么发请求"是 App 与装配事实，
    // 不是执行语义。搬出来就得**跟着进名单**，否则"Core 不认识 App 层"这条约束会从新家的门上漏掉。
    "im_channel",
    "im_social_app",
    "im_profile_app",
    // **2026-10-10 补齐 11 项**（§4「守卫的边界是枚举式的，而且有盲区」那一格）：这张表是
    // **手抄的**，而后加的模块没人回头补 ⇒ "Core 不认识它"这条约束从新家的门上漏掉了。
    // 补之前**逐条量过**：这 11 个今天补进去是绿的（`core.rs` 只 import `cost` 与 `foundation`，
    // `foundation.rs` 只 import std）。**没有照单全补**——另外两个量出来会红，成因各不相同，
    // 进了下面的豁免表（见 `CORE_BOUNDARY_EXEMPTIONS`）。
    "activity_origin",
    "app_contract",
    "composition",
    "model_retry",
    "pinned_actions",
    "previous_activity",
    "sandbox",
    "skills_desktop",
    "self_concept",
    // 交接（`<环境根>/handoff`）：与自识同层、同款——主体的自持物，装配期读回、挂在 Run 级插槽上。
    // 它碰文件系统（读回 ＋ 落盘），因此照样进这条封闭名单。
    "handoff",
    "timer_app",
    "wake",
    // **2026-10-18 补齐两项**：`every_module_is_accounted_for_in_the_layering_ledger` 当场抓出的
    // 漏项——上一个手抄名单的人还没见过它们。逐条量过：两份核心源码里这两个名字**连独立标识符
    // 都不出现**（`core.rs` 只 import `cost` 与 `foundation`，`foundation.rs` 只 import std）
    // ⇒ 补进去是绿的，它们本来只是没人回头补。
    "resident_lifetime",
    "terminal_held",
];

/// 进不了 [`BOUNDARY_MODULES`] 的模块：**照单补进去会在 `core.rs` 上当场红**，而红的原因不是
/// "Core 真的引用了它"，是**这条判据在它们身上量不出区别**（第二列是理由）。
///
/// 它是一本**债务账**（与 `the_source_dependency_graph_has_no_cycles` 那张同款）：每条豁免今天
/// 都必须**仍然成立**——守卫会断言"补进去真的会红"。原因一旦被修掉（例如把 `RunCost` 搬进
/// `core`），豁免就成了空账，**必须跟着删掉**，否则红。
const CORE_BOUNDARY_EXEMPTIONS: [(&str, &str); 2] = [
    (
        "cost",
        "`RunCost` 是 **Core 自己的中立词汇**（`RunRequest.cost` / `Record::RunEnded { cost }`）：\
         Core 认识这个类型、不解释它。`core.rs` 与 `foundation.rs` 都写着 `crate::cost::RunCost`，\
         因此照单补进去会红——而那不是违规，是这条规则比 Core 的真实边界粗一格。\
         要收掉这条豁免，得把 `RunCost` 搬进 `core`（动中立面，另一刀）",
    ),
    (
        "commit",
        "模块名与 **Core 自己的词汇同名**：`CommitSeam` / `CommitStage` / `CommitError` 都归 Core，\
         而 `commit_run_start` 这类方法名按 `_` 拆词之后必然产出 `commit` 这个整词 ⇒ 独立标识符\
         那条形态在 `core.rs` 里命中上百次。它量的是「同名」，不是「引用了那个模块」\
         （`crate::commit` 一次都没出现）⇒ 这条判据在它身上失效，如实登记为豁免",
    ),
];

/// 一个模块名在这段**代码**里被引用了吗。
///
/// 两种形态与 [`BOUNDARY_MODULES`] 的说明一致：路径形式的 `crate::<模块>`，以及作为独立标识符
/// 出现的模块名。**判据落在代码行上**（注释行由 [`code_lines`] 剔除）：文档里点名一个模块、
/// 或写一条 `[`crate::x`]` 文档链接，**不是依赖**——守卫不该逼着代码为它改措辞，否则它就从
/// "约束结构"退化成"约束用词"（同一条判据见 `code_lines` 与 `a_display_area_never_knows_an_app`）。
///
/// ⚠ **2026-10-10 改这一处**：从前它拿**整份文本**（含注释）量独立标识符那条形态。`composition`
/// 就是这样被挡在名单外的——`core.rs` 里只有**一句文档指路**提到它（"要多次驱动同一个活动请用
/// 会话（见 `composition`）"），没有任何引用。收窄到代码行之后它进得了名单，而真引用照旧会红。
fn references_module(code: &str, module: &str) -> bool {
    code.contains(&format!("crate::{module}")) || !forbidden_word_hits(code, &[module]).is_empty()
}

/// `lib.rs` 是 crate 门面，**必须**声明 Adapter 模块，因此它的模块名本身会出现
/// `provider` 这个词。扫描它之前只剥掉这一个**已声明的标识符**，其余内容照扫；
/// 下面还会断言这个声明确实存在，避免"剥掉"变成对空白的豁免。
const FACADE_SOURCE: &str = "lib.rs";
const FACADE_MODULE_NAME: &str = "provider_adapter";

/// 只允许 Adapter 拥有的依赖。**这是一个封闭名单**：新增依赖必须先改这里，
/// 否则守卫会失败——这正是我们想要的（依赖变化必须被人看见）。
///
/// 阶段八没有新增任何**依赖**：身份环境与只追加存储用 `std::fs`，序列化复用已有的
/// `serde_json`。“复用已有依赖”因此不改变这份名单；`serde_json` 的使用范围另由
/// `SERDE_JSON_SOURCES` 单独约束（见下）。
const ADAPTER_DEPENDENCIES: [&str; 3] = ["reqwest", "serde", "serde_json"];

/// 允许使用 `serde_json` 的源文件。
///
/// 第二个真实 App（`notes_app.rs`）用它写**自己的**笔记文件：那是一个 App 私有的存储格式，
/// 与 Foundation 的事实格式无关，也不共享任何存储协议。这是一次必须被人看见的变化，因此
/// 它同样是一条显式、封闭的名单。
///
/// 阶段八把 `serde_json` 的使用从“只有 Adapter”扩到“Adapter + 身份环境存储”：
/// 存储把经历与事实写成 NDJSON，**不引入新依赖**。这是一次必须被人看见的变化，
/// 所以它是一条显式、封闭的名单，而不是让 `serde_json` 变成随处可用。
const SERDE_JSON_SOURCES: [&str; 20] = [
    "provider_adapter.rs",
    "store.rs",
    // 2026-10-08（同上那一刀）：NDJSON 的编解码跟着**分卷**走，因此新文件也显式列入。
    "store/parts.rs",
    // 第二刀（读回与投影）：`store/projection.rs` 把盘上那一行读回来／搬出去，同款显式列入。
    "store/projection.rs",
    // 第三刀（身份与准入）：`store/environment.rs` 读身份记录、读两份存储、查目录 ⇒ 同款显式列入。
    "store/environment.rs",
    // 第四刀（格式与校验和）：`store/format.rs` 编解码 NDJSON ⇒ 同款显式列入（它是四块里唯一不碰文件系统的）。
    "store/format.rs",
    "notes_app.rs",
    // TASK_75：timer 把主体的表写成**一行一条 JSON 对象**（`<环境根>/timers`）——与 notes 同款：
    // App 私有格式、不共享任何存储协议。按同一份纪律显式列入。
    "timer_app.rs",
    // 2026-10-11：**第四个 App 拥有自己的存储**——`<环境根>/terminal`（主体名下那些活到关家的事：
    // 一本只追加的账）。与 notes / timers 同款：App 私有格式、一行一条 JSON 对象、不共享任何
    // 存储协议 ⇒ 按同一份纪律显式列入（**不是"让 serde_json 随处可用"**）。
    "terminal_held.rs",
    // im app 读的是 im 服务端的 JSON 投影（外部冻结形状）：它按同一份纪律显式列入。
    "im_app.rs",
    // TASK_35：web 窗读的是搜索厂商的回执（同样是外部冻结形状）。它按同一份纪律显式列入——
    // 这不是"让 serde_json 随处可用"，是又一次必须被人看见的变化。
    "web_app.rs",
    // TASK_42：外部来源的 App（MCP）整条路径都是 JSON 消息——声明、清单、参数、回执。
    // 三件（协议面 / 通道 / 动词面）各自显式列入，理由与上面同款。
    "mcp/mod.rs",
    "mcp/stdio.rs",
    "mcp/app.rs",
    // TASK_44：第二条 wire（Responses）的请求体与响应同样是 JSON（`input` 条目**原样**收与回传，
    // 因此这里用 `serde_json::Value` 而不是强类型）。它按同一份纪律显式列入。
    "responses_adapter.rs",
    // TASK_77：**模型配置门**读的是环境根下那个私有文件的 JSON（我们自己的本机格式，
    // 不是任何外部协议）。它按同一份纪律显式列入——不是"让 serde_json 随处可用"。
    "model_source.rs",
    // **2026-10-02 im 拆包**：im 的动词按能力域分成三个包（通信 / 社交 / 资料），读的仍是 im
    // 服务端那份**外部冻结形状**的 JSON 投影，而机械搬运落在中立层。三个新文件按同一份纪律
    // 显式列入——这同样不是"让 serde_json 随处可用"。
    "im_channel.rs",
    "im_social_app.rs",
    "im_profile_app.rs",
    // **逐轮读数账**那一刀：它把一行行读数写成 NDJSON（与两份追加存储同一门格式，
    // **不引入新依赖**）。这是一次必须被人看见的变化，因此照样显式列入。
    "model_readings.rs",
];

/// 允许碰文件系统的源文件：只读文件 App、把资料与派生经历目录合成一个视图的只读视图、
/// 把它装起来的组合入口，身份环境存储，以及**跨 App 的沙盒路径机制**。
///
/// ⚠ **下面这张表就是权威**（2026-10-06 更正）：这里从前写着"`store.rs` 是除去前两者之外
/// **唯一**允许碰文件系统的模块"——那句话从第二个 App 起就不成立了（拥有自己存储的 App
/// 自己碰文件系统），而它和这张表只隔一行，正是"描述句与事实打架"的典型形状。
/// 表里每一行各自带理由，判据只认这张表。
const FILE_SYSTEM_SOURCES: [&str; 19] = [
    "file_app.rs",
    "file_view.rs",
    "composition.rs",
    "store.rs",
    // 2026-10-08：`store.rs` 拆出**分卷与索引**那一段（新文件 `store/parts.rs`）。它照旧读目录、
    // 读卷、写索引 ⇒ 按同一份纪律**显式列入**——**拆文件不是"让文件系统 API 随处可用"**，
    // 这是又一次必须被人看见的变化。
    "store/parts.rs",
    // 2026-10-08 第二刀（读回与投影）：`store/projection.rs` 照旧按活动读卷、写静态头 ⇒ 同款显式列入。
    "store/projection.rs",
    // 第三刀（身份与准入）：`store/environment.rs` 读身份记录、读两份存储、查目录 ⇒ 同款显式列入。
    "store/environment.rs",
    // 第二个 App 拥有**自己的**存储（`<环境根>/notes/`），因此它自己碰文件系统。
    // 这是"App 有自己的数据"应有的样子，不是串层：它没有碰环境的既有记录格式。
    "notes_app.rs",
    // TASK_41：技能桌面**只读**材料区的 `<材料根>/skills/`（**一次目录读**——2026-09-22 降级为
    // 一行读数后不再读任何 SKILL.md）。
    // 它是纯投影（不写、不落盘），但"能看见什么"仍然是边界事实，因此照样进这条封闭名单。
    // TASK_75：**第三个 App 拥有自己的存储**——`<环境根>/timers`（主体的第三件自持物：约时间）。
    // 与 notes 同款：那是 App 私有格式、与既有记录格式无关；读的是自己地盘，不需要授权、
    // 不写环境里任何既有文件。这是一次必须被人看见的变化，因此照样进这条封闭名单。
    "timer_app.rs",
    // 2026-10-11：同上那一刀——`terminal_held.rs` 读自己那本账、追加一行，并把那件事的输出写进
    // `<环境根>/terminal-logs/`。读的是自己地盘、**不写环境里任何既有文件**，与 notes / timers
    // 同款 ⇒ 照样进这条封闭名单。
    "terminal_held.rs",
    "skills_desktop.rs",
    // TASK_65：**沙盒路径机制**从文件 App 里搬了出来，落在中立层——`files` / `file_view` / `im`
    // 都要"一个相对路径在某个根之下意味着什么"这一条规则，而 App 之间不得有实现依赖
    // （本文件下面的独立性断言机械检查它；这一刀正是被它拦下过一次）。它仍然碰文件系统
    // （逐段 `symlink_metadata` + 读字节），因此照样进这条封闭名单；它**不写**任何东西。
    "sandbox.rs",
    // TASK_68 P3：**动作 pin** 读自己地盘上的那一份（`<环境根>/pinned_actions`，一行一个
    // 标识）——与纸条板同款：读的是主体自持物，不需要授权、不写任何东西（读失败 ⇒ `None`
    // ⇒ 如实渲染 `unknown`）。因此照样进这条封闭名单。
    "pinned_actions.rs",
    // TASK_70：**自识**（`<环境根>/self_concept`）与它同层、同款——Agent 的自持物，装配期读回、注入开场。
    // ⚠️ 与上面那个**不同**的一点：这一份**会被自己写**（`write_self_concept`，OS 自有动词），
    // 而那一次的写路径落在 `os.rs` 里（通过本模块的 `incept`），不在这里直接开文件写。
    // 它仍然碰文件系统（读回 + 落盘），因此照样进这条封闭名单。
    "self_concept.rs",
    // 交接（`<环境根>/handoff`）：与自识同款——读回由装配做、写由 `write_handoff` 那一次做
    // （通过本模块的 `incept`）。它读的是主体自己的地盘、不需要授权，因此照样进这条封闭名单。
    "handoff.rs",
    // TASK_77：**模型配置门**读环境根下的 `<环境根>/model_source.json`（**只读**，不写任何
    // 东西）。它在装配期判"这一次听谁的"（文件 / 进程侧），因此照样进这条封闭名单。
    "model_source.rs",
    // TASK_79：**出厂说明与手册**（`<环境根>/orientation.md` 与 `<环境根>/manual.md`）——
    // 读回由装配做（与自识同款），而**写**只在创建那一次（`orientation::place`，从显式初始化
    // 调过来）。它读的是自己的地盘、不需要授权，因此照样进这条封闭名单。
    "orientation.rs",
    // **机器处境**那一刀：这一件读的**不是环境根、也不是主体的自持物**，而是**机器自己的报告**
    // （Linux 内核在 procfs 里自报操作系统名那个文件）——"这台机器是什么"的真理源在机器上，
    // 不在我们的地盘里。它**只读一个文件、不写任何东西、不读目录**；"能看见什么"仍然是边界事实，
    // 因此照样进这条封闭名单（谁要碰文件系统，就得在这里被人看见）。
    "machine.rs",
    // **逐轮读数账**那一刀：`<环境根>/model_readings.ndjson`——**自己开一个追加句柄、只写不读**
    // （不回读任何一行；回读是操作者翻文件的事）。它是这一族读数唯一的落点，因此照样进这条
    // 封闭名单：**谁要碰文件系统，就得在这里被人看见**。
    //
    // ⚠ 它**不自己取锁**：单写者由**环境那把写锁**派生——写只发生在真的调用模型时，而调用
    // 需要过准入，准入要那把锁。因此两个进程不会同时往这一份账上写。
    "model_readings.rs",
];

/// 文件系统 API 记号：只允许出现在上面那两个文件里。
const FILE_SYSTEM_MARKERS: [&str; 6] = [
    "std::fs",
    "fs::",
    "File::",
    "read_dir",
    "canonicalize",
    "symlink_metadata",
];

/// 允许**起进程**的源文件：**封闭名单**。
///
/// 这是比"碰文件系统"更重的一件事：读写文件改的是数据，起一个进程改的是**整台机器**。因此它
/// 同样是一条显式的、封闭的名单——谁要起进程，就得在这里被人看见。三个：terminal 窗
/// （主体的那只手，TASK_36）、MCP 的 stdio 通道（外面那台 server 的子进程，TASK_42）、
/// 以及**平台缝本身**（`platform.rs`：两个消费者的臂都在那里拼命令行、杀进程树——它是这两条
/// 起进程路径的共同出口，因此必须与它们一样被看见）。
const PROCESS_SOURCES: [&str; 3] = ["terminal_app.rs", "mcp/stdio.rs", "platform.rs"];

/// 进程执行 API 记号：只允许出现在上面那份名单里。
///
/// **`std::process` 本身不是记号**：读一个进程号（`std::process::id()`，活动标识里就用它）不是
/// 起进程，把它算进来只会逼无关代码改名。记号只覆盖"真的起一个进程"的那几步。
const PROCESS_MARKERS: [&str; 4] = ["process::Command", "Command::new", "Stdio::", "taskkill"];

/// 供应商 API 记号：只允许出现在 Adapter 里；其中 `serde_json` 另按 [`SERDE_JSON_SOURCES`]
/// 单独放行（身份环境存储用它写 NDJSON，不引入新依赖）。
const PROVIDER_MARKERS: [&str; 3] = ["reqwest", "serde::", "serde_json"];

/// 长词记号层面的排除项：这些记号在 `PROVIDER_MARKERS` 里，但允许出现在名单内的其它层。
///
/// 只有 `serde_json`——它是"序列化格式"而不是"供应商协议"。
const PROVIDER_MARKER_EXCEPTIONS: [&str; 1] = ["serde_json"];

/// 短词层面的排除项：`json` 是上面的 `serde_json` 被词边界切开后的形态。
///
/// 两个层面的排除必须**各自显式**列出来：长词扫描与短词扫描是两套判断，共用一份名单会漏。
const FORBIDDEN_WORD_EXCEPTIONS: [&str; 1] = ["json"];

/// 中立层里**按整条短语**放行的记号：`(文件, 短语)`。
///
/// 与上面的 `json` 例外同一个来路，但更窄：那里放行的是"某个词在某个文件里"，这里放行的是
/// **某一条完整短语**。两类条目：
///
/// - 模型失败分类名 `http-status-failure`——它与适配层的稳定分类名**逐字同名**是设计裁决
///   （TASK_15 D1：一次映射、零改名，分类要作为事实进盘、进终端、进诊断），而这条短词守卫的
///   本意是"中立层不做传输层的事"：一个分类**名字**里带协议词不是那件事；
/// - **TASK_26**：门铃必须原样说的三个**外部/已定稿的字面量**——那条由 im 服务端持有的长连接
///   路径、事件流的媒体类型（我们对它的称呼是"唤醒通道"，但"连到哪儿、要什么媒体类型"只能在
///   连接处照契约写一次），以及**门铃起源事实的正文**（它是产品定稿的一句话，逐字节写进事实；
///   里面那个词指的是那条通道，不是"我们要做一个流式平台"）。
///
/// 放行方式是"把这短语从待扫描文本里去掉再扫"——因此同一个词在**别处**出现（别的文件、别的
/// 形态、别的短语）仍然照旧违规，整个词与整份文件都没有被豁免。
const CORE_ALLOWED_PHRASES: [(&str, &str); 10] = [
    ("core.rs", "http-status-failure"),
    ("doorbell.rs", "\"/v1/backend/events/stream\""),
    ("doorbell.rs", "\"text/event-stream\""),
    (
        // **豁免跟着短语走**：这一段正文原来住 `os.rs`，随"起源整件搬出循环层"一起搬到
        // `activity_origin.rs`。放行的始终是**这条短语**（产品定稿的一句话，逐字节写进事实），
        // 不是那个词、也不是整个文件——所以它换了个家，这一行也跟着换，而不是多出一条。
        "activity_origin.rs",
        "the backend event stream signaled that new events are available",
    ),
    // TASK_35：web 窗必须原样说的三个**外部/已定稿的字面量**。
    // 前两个是标准里定下来的协议名（一个网页地址只可能是这两种之一，我们按白名单收）；
    // 第三个是**主体的工具面**上那个参数的已定稿名字（施工单写明），也是我们从搜索结果里读出来的
    // 那个字段名。三处都是**带引号的字面量**：同一个词作为标识符、注释或别的形态出现照旧违规。
    ("web_app.rs", "\"http\""),
    ("web_app.rs", "\"https\""),
    ("web_app.rs", "\"url\""),
    // 第四处是一条**依赖路径**：请求客户端自己的地址类型名（那个类型就叫这个名字）。
    // 我们自己的标识符与变量一律不叫它——放行的是这条**路径**，不是那个词。
    ("web_app.rs", "reqwest::Url"),
    // TASK_74：`context` 那一格的**单位词**进主体的视野（读数行 + 开场说明书）。
    //
    // 这是本表里唯一一处"**故意给主体**"的词：它属于给主体的词表（模型有原生先验），不属于我们的
    // wire——wire 那边它叫 `prompt_tokens` / `input_tokens`，仍在适配层里、仍照旧受这两张表约束。
    // 放行的仍是**带引号的字面量**：`status_bar.rs` 里它只以两种形态各出现一次
    // （`CONTEXT_UNIT` 的定义、说明书里"被定义的单位词"那一对引号）；作为标识符/变量/注释出现
    // 照旧违规，别的文件一个都不放行。
    ("status_bar.rs", "\"token\""),
    // 2026-10-11（发送侧不限类型）：`application/octet-stream` —— **标准 MIME 类型名**，
    // 与 `text/event-stream` 同一类（外部定稿的字面量，agent 侧只能原样说它）。
    // 放行的是**这一条 token**：那个词作为标识符、变量或别的短语出现照旧违规，
    // 别的文件一个都不放行。
    ("im_app.rs", "application/octet-stream"),
];

/// 中立层里**按整条短语**放行的长词记号：`(文件, 短语)`。
///
/// 与 [`CORE_ALLOWED_PHRASES`] 同族，区别是它作用在**子串扫描**上（那一条作用于短词扫描）。
/// 两条条目都来自 im 侧契约的**外部冻结形状**（agent 侧只能原样说它们），而 `messages` 同时
/// 又被列在供应商 wire 词里：字段名 `"messages"` 与路由末段 `"/messages"`。放行的是**带引号的
/// 字面量**本身——同一个词不带引号出现（标识符、注释、别的形态）仍然照旧违规，别的文件一个
/// 都不放行；本层自己的标识符用我们自己的词（如 `rows`），外部名字只在连接处各出现一次。
/// 两条对照断言见 `the_boundary_scanners_report_violations_when_they_exist`。
///
/// TASK_42 的第三处是一条**依赖路径**（与 `web_app.rs` 的 `reqwest::Url` 同一族）：
/// 工具名的长度上限归适配层（那是 wire 事实），而外部来源的 App 造动词名时必须**读那一份**，
/// 不许另抄一个数字。放行的是这条路径——`provider` 那个词在本文件里换个形态出现照旧违规
/// （短词扫描照旧生效）。
const SUBSTRING_ALLOWED_PHRASES: [(&str, &str); 3] = [
    ("im_app.rs", "\"messages\""),
    ("im_app.rs", "\"/messages\""),
    ("mcp/mod.rs", "provider_adapter::TOOL_NAME_MAX"),
];

/// 按 [`CORE_ALLOWED_PHRASES`] 把允许的短语从待扫描文本里去掉（只对点名的那个文件生效）。
fn strip_allowed_phrases(text: &str, name: &str, allowed: &[(&str, &str)]) -> String {
    let mut out = text.to_string();
    for (file, phrase) in allowed {
        if *file == name {
            out = out.replace(phrase, "");
        }
    }
    out
}

/// 展开/披露记号：只允许出现在 OS 骨干里（适配层不得决定能力可用性，App 不得认识 OS）。
///
/// 展开与"未展开/已展开"的判断**只有 OS 的作用域一处权威**：把展开状态留在 App 里会引出
/// "一个 App 读另一个 App 是否展开"的隐式耦合，所以本刀把它从文件/经历组件里收回到 OS。
///
/// **不含 `identity-mismatch`**：那是身份/存储事实的不匹配结果，由身份环境层（`store`）
/// 与展开失败（`os`）各自独立表达，不是"能力是否可用"的导航语言。把它算作导航记号，
/// 只会逼着存储层为了守卫改措辞，守卫就退化成约束用词。
const NAVIGATION_MARKERS: [&str; 3] = ["open_app", "not-expanded", "already-expanded"];

/// 允许出现导航记号的源文件。
const NAVIGATION_SOURCES: [&str; 1] = ["os.rs"];

/// **im 家族的三个包**（2026-10-02 拆包）：文件名 → 模块名。
///
/// 它们共用中立层 `im_channel`（机制，不是任何 App 的内部——与 `sandbox` 同款），
/// 但**彼此不得引用实现**：这条由 `boundary_apis_stay_in_their_own_layer` 逐个检查。
const IM_PACKS: [(&str, &str); 3] = [
    ("im_app.rs", "im_app"),
    ("im_social_app.rs", "im_social_app"),
    ("im_profile_app.rs", "im_profile_app"),
];

/// 一个记号是否作为**独立标识符/独立词**出现。
///
/// 不能退化成裸子串匹配：`open_app` 是导航动作的身份，而 `open_append` 不是它——
/// 前者是导航，后者只是碰巧同前缀。同理 `not-expanded` 不该被 `xnot-expanded` 命中。
/// 判定用"前后不是标识符字符"作边界（`_` 与字母数字都算标识符字符）。
fn marker_present(source: &str, marker: &str) -> bool {
    let bytes = source.as_bytes();
    let needle = marker.as_bytes();
    if needle.is_empty() || bytes.len() < needle.len() {
        return false;
    }
    let mut from = 0usize;
    while let Some(found) = source[from..].find(marker) {
        let start = from + found;
        let end = start + needle.len();
        let before_ok = start == 0 || !is_identifier_byte(bytes[start - 1]);
        let after_ok = end == bytes.len() || !is_identifier_byte(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

/// 这个字节算不算"标识符的一部分"。
fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// 源码里出现的文件系统记号。
fn file_system_markers_in(source: &str) -> Vec<String> {
    let lowered = source.to_lowercase();
    FILE_SYSTEM_MARKERS
        .iter()
        .filter(|marker| lowered.contains(&marker.to_lowercase()))
        .map(|marker| (*marker).to_string())
        .collect()
}

/// 源码里出现的进程执行记号。
///
/// **按原样匹配**（不做大小写折叠），与文件系统/供应商那两条不同：这三个记号是**代码标识符
/// 或可执行的路径**，大小写是语义的一部分。而大小写不敏感会造出真误判——本仓自己的
/// `mcp::stdio` 子模块写成 `stdio::StdioTransport` 时，小写形态正好等于进程那一个记号，
/// 于是它逼着一个**根本不起进程**的文件去改名字。那正是上一条守卫自己声明要避免的事
/// （"`std::process` 本身不是记号……把它算进来只会逼无关代码改名"）。
///
/// 少掉的覆盖面可以忽略：真起进程的代码**必然**经过 `Command::new`，它照旧命中。
fn process_markers_in(source: &str) -> Vec<String> {
    PROCESS_MARKERS
        .iter()
        .filter(|marker| source.contains(**marker))
        .map(|marker| (*marker).to_string())
        .collect()
}

/// 源码里出现的供应商记号。
fn provider_markers_in(source: &str) -> Vec<String> {
    let lowered = source.to_lowercase();
    PROVIDER_MARKERS
        .iter()
        .filter(|marker| lowered.contains(&marker.to_lowercase()))
        .map(|marker| (*marker).to_string())
        .collect()
}

/// 屏幕报告 cell 的**唯一写者**守卫：`ContextReadingWriter` 这个类型只允许出现在定义处与两条适配层。
///
/// 它是 TASK_19 那条状态归属声明（"允许的写者 = 仅适配层"）的机械版：cell 的写入口不是约定，
/// 而是**只交给装配模型的那一方**。这条守卫拦住"顺手从显示区/组合层/入口写一下"这类改动——
/// 那会让"最近一次厂商报告"变成可以由别处编造的东西，读数的来源就不可解释了。
///
/// TASK_44 起是**两个**适配层文件各持一份写柄（chat 与 Responses 各自的写点），
/// 因此名单从 2 条变 3 条——这是一次必须被人看见的变化，不是"顺手加一个"。
///
/// ⚠️ 「中立零件层」那一刀把**定义处**从 `status_bar.rs` 搬到了 `context_reading.rs`（显示区不再是这一格的家：
/// 芯片不该认识状态栏）。名单因此换的是**定义处那一条**，写者仍然是两条适配层——判据一字未动。
///
/// 文本级判断（不是链接期证明）：按**词**匹配类型名，因此 `reading_writer` 这类变量名也照抓。
#[test]
fn only_the_adapter_writes_the_reading_cell() {
    const ALLOWED: [&str; 3] = [
        "context_reading.rs",
        "provider_adapter.rs",
        "responses_adapter.rs",
    ];
    let sources = core_sources();
    for (name, source) in &sources {
        if ALLOWED.contains(&name.as_str()) {
            continue;
        }
        let hits = forbidden_word_hits(&source.to_lowercase(), &["contextreadingwriter"]);
        assert!(
            hits.is_empty(),
            "{name} 出现了屏幕写柄 ContextReadingWriter：唯一写者是适配层"
        );
    }
    // 自检：两个被点名的文件确实持有着它（否则这条守卫是空的）。
    for allowed in ALLOWED {
        let source = sources
            .iter()
            .find(|(name, _)| name == allowed)
            .map(|(_, source)| source)
            .unwrap_or_else(|| panic!("{allowed} 不在 src/ 下"));
        assert!(
            !forbidden_word_hits(&source.to_lowercase(), &["contextreadingwriter"]).is_empty(),
            "{allowed} 应当持有屏幕写柄（定义处 / 唯一写者）"
        );
    }
}

/// **逐轮读数账的唯一写者**守卫：`ModelReadingsWriter` 这个类型只允许出现在定义处、两条适配层，
/// 以及**转发处**（`model_choice`）。
///
/// 它拦的是"顺手从组合层、显示区或入口写一行"：那会让这份账变成可以由别处编造的东西，而它的
/// 全部价值就在"这一行是**那一次响应**报的"——来源不可解释，账就不值得翻。
///
/// ⚠ 名单里的 `model_choice.rs` 是**转发处不是写者**：它按线协议选实现，而"装上传感器"是两条
/// 协议共同的收尾，因此写柄经它交出去（与 `with_usage` 同款）。它一次都不调 `record`。
///
/// 文本级判断（不是链接期证明）：按**词**匹配类型名，因此 `readings_writer` 这类变量名也照抓。
#[test]
fn only_the_adapter_writes_the_readings_log() {
    const ALLOWED: [&str; 4] = [
        "model_readings.rs",
        "provider_adapter.rs",
        "responses_adapter.rs",
        "model_choice.rs",
    ];
    let sources = core_sources();
    for (name, source) in &sources {
        if ALLOWED.contains(&name.as_str()) {
            continue;
        }
        let hits = forbidden_word_hits(&source.to_lowercase(), &["modelreadingswriter"]);
        assert!(
            hits.is_empty(),
            "{name} 出现了逐轮读数账的写柄 ModelReadingsWriter：唯一写者是适配层"
        );
    }
    // 自检：被点名的文件确实持有着它（否则这条守卫是空的）。
    for allowed in ALLOWED {
        let source = sources
            .iter()
            .find(|(name, _)| name == allowed)
            .map(|(_, source)| source)
            .unwrap_or_else(|| panic!("{allowed} 不在 src/ 下"));
        assert!(
            !forbidden_word_hits(&source.to_lowercase(), &["modelreadingswriter"]).is_empty(),
            "{allowed} 应当持有逐轮读数账的写柄（定义处 / 唯一写者 / 转发处）"
        );
    }
}

/// **块级静默丢**的守卫：`ContentBatch::of` 之后不许出现"退回空批次 / 退回另一份"。
///
/// 这是"内容装不下时必须留下读数"这条纪律的机械版：超限是**整体拒绝**（不产生半批），
/// 于是调用点那一步"退回什么"决定了那一块内容**是消失了还是被读出来了**。
/// 合格出路见 [`silent_batch_fallbacks_in`] 的文档；唯一的诚实回退助手是
/// `ContentBatch::of_or_not_delivered`（它自己的装不下由 `not_delivered` 兜住，不在这里判）。
#[test]
fn a_batch_that_does_not_fit_is_never_silently_dropped() {
    let sources = core_sources();
    let mut violations: Vec<String> = Vec::new();
    for (name, source) in &sources {
        for (line, text) in silent_batch_fallbacks_in(source) {
            violations.push(format!("{name}:{line} {text}"));
        }
    }
    assert!(
        violations.is_empty(),
        "装不下就退回空批次（或别人的那一份）= 那一整块内容无声消失；\
         请改用 `ContentBatch::of_or_not_delivered(blocks, \"<这个来源的标签>\")`，\
         或把失败传出去：\n  {}",
        violations.join("\n  ")
    );

    // 自检：诚实出路确实在被用——否则上面那条可能只是"没人写 ContentBatch::of"（空守卫）。
    let owners: Vec<&str> = sources
        .iter()
        .filter(|(_, source)| source.contains("ContentBatch::of_or_not_delivered"))
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(
        owners.len() >= 8,
        "每一处会超限的来源都该走诚实回退，现在只有 {owners:?}"
    );
}

/// 扫描器自检：给它一个明确含静默回退形状的样本，它必须报出违规；给合格形状必须不报。
///
/// 与上面那条守卫互为对照——扫描逻辑被改坏、窗口拼错或文件枚举失效时，
/// 上面那条会永久假绿，而这一条会失败。
#[test]
fn the_silent_drop_scanner_reports_when_it_exists() {
    // 违规：单行的"退回空批次"。
    let silent =
        "fn f() -> ContentBatch {\n    ContentBatch::of(vec![block]).unwrap_or_default()\n}";
    assert_eq!(
        silent_batch_fallbacks_in(silent).len(),
        1,
        "退回空批次必须被抓到"
    );

    // 违规：跨行写法 + "退回另一份"。
    let cross_line =
        "let b = ContentBatch::of(\n        vec![block],\n    )\n    .unwrap_or(other);";
    assert_eq!(
        silent_batch_fallbacks_in(cross_line).len(),
        1,
        "跨行 + 退回另一份也必须被抓到"
    );

    // 合格：诚实回退助手（它自己一定装得下，因此不需要回退）。
    let honest = "ContentBatch::of_or_not_delivered(vec![block], STATUS_BAR_LABEL)";
    assert!(
        silent_batch_fallbacks_in(honest).is_empty(),
        "诚实回退不是违规"
    );

    // 合格：显式处理拒绝（match / map_err / ?）。
    let explicit = "let b = match ContentBatch::of(blocks) {\n        Ok(b) => b,\n        Err(_) => other,\n    };";
    assert!(
        silent_batch_fallbacks_in(explicit).is_empty(),
        "显式 match 不是违规"
    );
    let propagated = "ContentBatch::of(vec![block]).map_err(|_| CompositionError::Refused)?";
    assert!(
        silent_batch_fallbacks_in(propagated).is_empty(),
        "把失败传出去不是违规"
    );

    // 合格：别的语句里的 `unwrap_or` 不算在这句调用头上（窗口在 `;` 处收尾）。
    let unrelated =
        "let b = ContentBatch::of(blocks).expect(\"合法\");\nlet x = maybe.unwrap_or(0);";
    assert!(
        silent_batch_fallbacks_in(unrelated).is_empty(),
        "同函数里别的 unwrap_or 不该被算到这句调用头上"
    );
}

/// **分类词稳定性**守卫：拒绝/失败的分类不许是动态串（形状扫，TASK_72）。
///
/// 随单内容（拒绝时附上的那张卡、参数差异那块读数）走**第二个实参**，因此这条守卫与它
/// **不冲突**：它只管"分类那一格还是不是一个稳定标记"。
#[test]
fn a_refusal_or_failure_class_is_never_a_dynamic_string() {
    let mut violations: Vec<String> = Vec::new();
    for (name, source) in core_sources() {
        for (line, text) in dynamic_class_hits(&source) {
            violations.push(format!("{name}:{line} {text}"));
        }
    }
    assert!(
        violations.is_empty(),
        "分类词是词表，不是消息：它不许由内容拼出来（同一故障两次发生必须逐字节相同）。\
         随单内容请放第二个实参：\n  {}",
        violations.join("\n  ")
    );

    // 自检：这条守卫不是在扫空——分类调用点确实存在。
    let calls: usize = core_sources()
        .iter()
        .map(|(_, source)| {
            [
                "refused(",
                "refused_with(",
                "failure(",
                "failure_with_content(",
            ]
            .iter()
            .map(|call| source.matches(call).count())
            .sum::<usize>()
        })
        .sum();
    assert!(calls >= 10, "分类构造点少得可疑（只有 {calls} 处）");
}

/// 扫描器自检：负向样例（拼出来的分类）必须被抓到，合格形状必须不报。
#[test]
fn the_dynamic_class_scanner_reports_when_it_exists() {
    let dynamic = [
        r#"fn f() -> ActionAdmission { ActionAdmission::refused(format!("{}-x", id)) }"#,
        r#"fn g() -> ActionAdmission { ActionAdmission::refused_with(&("a".to_string() + "b"), vec![]) }"#,
        r#"fn h() -> ActionOutcome { ActionOutcome::failure(format!("bad-{}", kind)) }"#,
    ];
    for sample in dynamic {
        assert_eq!(
            dynamic_class_hits(sample).len(),
            1,
            "拼出来的分类必须被抓到: {sample}"
        );
    }

    let stable = [
        r#"ActionAdmission::refused("not-expanded")"#,
        r#"ActionAdmission::refused_with(NEEDS_INSPECTION, card)"#,
        r#"ActionAdmission::refused(ExpansionFailure::UnknownAction.as_str())"#,
        r#"ActionOutcome::failure_with_content(ExpansionFailure::InvalidParameters.as_str(), blocks)"#,
    ];
    for sample in stable {
        assert!(
            dynamic_class_hits(sample).is_empty(),
            "稳定标记不是违规: {sample}"
        );
    }
}

/// 分层守卫：文件系统 API 只能在文件 App 与组合入口，供应商 API 只能在 Adapter。
///
/// 这条是"边界 API 不串层"的机械版：Core 或 Foundation 一旦自己去开文件、读环境或引供应商
/// 库，就会被这里拦住。它是文本级判断，不是链接期证明。
#[test]
fn boundary_apis_stay_in_their_own_layer() {
    for (name, source) in core_sources() {
        let lowered = source.to_lowercase();

        if !FILE_SYSTEM_SOURCES.contains(&name.as_str()) {
            let hits = file_system_markers_in(&source);
            assert!(hits.is_empty(), "{name} 碰了文件系统 API: {hits:?}");
        }
        if !PROCESS_SOURCES.contains(&name.as_str()) {
            let hits = process_markers_in(&source);
            assert!(hits.is_empty(), "{name} 碰了进程执行 API: {hits:?}");
        }
        if !NETWORK_SOURCES.contains(&name.as_str()) {
            // `serde_json` 是序列化格式记号，不是供应商协议：它按 SERDE_JSON_SOURCES 单独放行。
            let hits: Vec<String> = provider_markers_in(&source)
                .into_iter()
                .filter(|marker| {
                    !SERDE_JSON_SOURCES.contains(&name.as_str())
                        || !PROVIDER_MARKER_EXCEPTIONS.contains(&marker.as_str())
                })
                .collect();
            assert!(hits.is_empty(), "{name} 碰了供应商 API: {hits:?}");
        }
        // 只有 OS 与历史能力可以说导航语言：适配层不得决定导航，App 不得认识 OS。
        //
        // 导航记号是**标识符/词**，不是任意子串：`open_app` 是导航动作的身份，
        // 而 `open_append`（一个"打开追加句柄"的普通函数名）不是它。按裸子串匹配会把
        // 正常命名判成违规，逼着代码为守卫改名字——守卫也就退化成了用词约束。
        if !NAVIGATION_SOURCES.contains(&name.as_str()) {
            for marker in NAVIGATION_MARKERS {
                assert!(
                    !marker_present(&lowered, marker),
                    "{name} 出现了导航记号 {marker}：导航只属于 OS 或历史能力"
                );
            }
        }
        // 适配层不认识 OS，也不认识 App。
        if name == ADAPTER_SOURCES[0] {
            for marker in ["crate::os", "minimalos", "file_app"] {
                assert!(
                    !lowered.contains(marker),
                    "provider_adapter.rs 认识了 {marker}：它只该认识 Core 的中立视野"
                );
            }
        }
        // **第二个 App 的独立性**：Notes 不得引用文件 App 的实现，文件 App 也不得引用 Notes。
        // 这是"独立性由第二个真实消费者证明"里可机械检查的那一半；另一半由行为测试与
        // "拿掉 File App 之后 Notes 照样工作"来证明。
        if name == "notes_app.rs" {
            for marker in ["file_app", "file_view"] {
                assert!(
                    !lowered.contains(marker),
                    "{name} 引用了文件 App 的实现 {marker}：两个 App 之间不得有实现依赖"
                );
            }
        }
        if name == "file_app.rs" || name == "file_view.rs" {
            assert!(
                !lowered.contains("notes_app"),
                "{name} 引用了 Notes App：文件 App 不得成为 Notes 的内部依赖"
            );
        }
        // **第三个 App 的独立性**：im app 不认识任何别的 App，别的 App 也不认识它。
        // 「加一个 App 便宜」这条判断的前提就是这一格：App 之间没有实现依赖。
        //
        // **2026-10-02 im 拆包**：im 家族从"整包"分成三个包（通信 / 社交 / 资料）。它们共用
        // 中立层 `im_channel`，但**彼此不得引用实现**——守卫因此从"im 是第三个 App"扩到
        // "im 家族的三个包互不认识"。包间判据扫的是**模块路径**（`crate::<模块>` / `<模块>::`）
        // 且只看**代码行**：包之间在文档里互相点名（"这一组动词住在哪个包"）是说明，不是依赖。
        if let Some(module) = IM_PACKS
            .iter()
            .find(|(file, _)| file == &name.as_str())
            .map(|(_, module)| *module)
        {
            let code = code_lines(&source);
            for (_, other) in IM_PACKS {
                if other == module {
                    continue;
                }
                for marker in [format!("crate::{other}"), format!("{other}::")] {
                    assert!(
                        !code.contains(&marker),
                        "{name} 引用了 im 家族另一个包的实现（{marker}）：App 之间不得有实现依赖"
                    );
                }
            }
            // 别的 App：按**独立标识符**判，不按裸子串——`im_profile_app` 里那个 `file_app`
            // 只是名字里的巧合（`profile_app` 的后半段），把正常命名判成违规会让守卫从
            // "约束结构"退化成"约束用词"（与导航记号那条同一判据：`open_append` 不是 `open_app`）。
            for marker in ["file_app", "file_view", "notes_app"] {
                assert!(
                    !marker_present(&lowered, marker),
                    "{name} 引用了别的 App（{marker}）：App 之间不得有实现依赖"
                );
            }
        }
        if name == "file_app.rs" || name == "file_view.rs" || name == "notes_app.rs" {
            for marker in ["im_app", "im_social_app", "im_profile_app"] {
                assert!(
                    !marker_present(&lowered, marker),
                    "{name} 引用了 im 家族的包（{marker}）：App 之间不得有实现依赖"
                );
            }
        }

        // App 不认识 OS 骨干（窄接入契约在 `app_contract`，它只依赖 Core 的中立载体）。
        if name == "file_app.rs"
            || name == "file_view.rs"
            || name == "notes_app.rs"
            || name == "im_app.rs"
        {
            for marker in ["crate::os", "minimalos"] {
                assert!(
                    !lowered.contains(marker),
                    "{name} 认识了 {marker}：App 不该认识 OS 骨干"
                );
            }
        }
        // 接入契约不认识任何具体实现：不导入 OS 骨干，也不导入 App 或存储。
        if name == "app_contract.rs" {
            for marker in [
                "crate::os",
                "minimalos",
                "file_app",
                "file_view",
                "crate::store",
            ] {
                assert!(
                    !lowered.contains(marker),
                    "app_contract.rs 认识了 {marker}：它只该描述 OS 需要知道什么"
                );
            }
        }
        // 平台缝是**最底层**的一份知识：它不认识任何消费者（主体的手、MCP 的通道）——
        // 两个消费者各自引用它，它一个都不许回头引用（方向只有一条）。
        if name == "platform.rs" {
            for marker in ["crate::terminal_app", "crate::mcp"] {
                assert!(
                    !lowered.contains(marker),
                    "platform.rs 引用了消费者 {marker}：平台缝不该认识谁在用它"
                );
            }
        }
    }

    // 自检：同一套判断面对明确的违规样本必须报出违规，否则守卫可能是永久绿的。
    assert!(
        !file_system_markers_in("let f = std::fs::File::open(path)?;").is_empty(),
        "文件系统记号扫描必须能报出违规"
    );
    assert!(
        !provider_markers_in("let c = reqwest::blocking::Client::new();").is_empty(),
        "供应商记号扫描必须能报出违规"
    );
    assert!(
        !process_markers_in(r#"let mut child = std::process::Command::new("x");"#).is_empty(),
        "进程执行记号扫描必须能报出违规"
    );
    // 反例：读进程号不是起进程（活动标识里就用它），名字里带 process 的普通函数也不算。
    assert!(
        process_markers_in("let pid = std::process::id();").is_empty(),
        "读进程号不该被当成起进程"
    );
    assert!(
        process_markers_in("fn process_markers_in(source: &str) -> Vec<String> {").is_empty(),
        "`process_markers_in` 不是进程执行记号"
    );
    // 大小写是语义的一部分：本仓的子模块路径 `stdio::…` 不是进程 API `Stdio::…`。
    assert!(
        process_markers_in("use crate::mcp::stdio::StdioTransport;").is_empty(),
        "`stdio::` 这条模块路径不是进程执行记号（大小写敏感）"
    );
    assert!(
        !process_markers_in("let child = Stdio::piped();").is_empty(),
        "按原样写出来的进程记号必须照旧命中"
    );
    // 子串短语放行必须**只**放行那一个带引号的字面量：
    // 同一个词不带引号照旧违规，别的文件一个都不放行。
    let quoted = strip_allowed_phrases(
        "let list = body.get(\"messages\");",
        "im_app.rs",
        &SUBSTRING_ALLOWED_PHRASES,
    );
    assert!(
        forbidden_substring_hits(&quoted, &FORBIDDEN_SUBSTRINGS).is_empty(),
        "带引号的字面量应当被放行"
    );
    let bare = strip_allowed_phrases("let messages = 1;", "im_app.rs", &SUBSTRING_ALLOWED_PHRASES);
    assert!(
        !forbidden_substring_hits(&bare, &FORBIDDEN_SUBSTRINGS).is_empty(),
        "同一个词不带引号必须照旧违规"
    );
    let elsewhere = strip_allowed_phrases(
        "let list = body.get(\"messages\");",
        "store.rs",
        &SUBSTRING_ALLOWED_PHRASES,
    );
    assert!(
        !forbidden_substring_hits(&elsewhere, &FORBIDDEN_SUBSTRINGS).is_empty(),
        "别的文件不得被放行"
    );
    // TASK_35 的三条新放行同样是**短语级**的：把短语去掉之后，短词守卫照旧生效。
    // ① 带引号的那个字面量在 web_app 里放行……
    let anchor_name = strip_allowed_phrases(
        "const PARAM_TARGET: &str = \"url\";",
        "web_app.rs",
        &CORE_ALLOWED_PHRASES,
    );
    assert!(
        forbidden_word_hits(&anchor_name, &FORBIDDEN_WORDS).is_empty(),
        "web_app 里那个带引号的字面量应当被放行"
    );
    // ② ……但同一个词当标识符用（不带引号）必须照旧违规；
    let bare_url = strip_allowed_phrases(
        "let url = target.to_string();",
        "web_app.rs",
        &CORE_ALLOWED_PHRASES,
    );
    assert!(
        !forbidden_word_hits(&bare_url, &FORBIDDEN_WORDS).is_empty(),
        "同一个词当标识符用必须照旧违规"
    );
    // ③ 同一个字面量出现在**别的文件**里也不放行；
    let elsewhere_url = strip_allowed_phrases(
        "const PARAM_TARGET: &str = \"url\";",
        "im_app.rs",
        &CORE_ALLOWED_PHRASES,
    );
    assert!(
        !forbidden_word_hits(&elsewhere_url, &FORBIDDEN_WORDS).is_empty(),
        "别的文件不得被放行"
    );
    // ④ 依赖路径那条放行只覆盖那一条路径本身。
    let dependency_path = strip_allowed_phrases(
        "let target = reqwest::Url::parse(raw)?;",
        "web_app.rs",
        &CORE_ALLOWED_PHRASES,
    );
    assert!(
        forbidden_word_hits(&dependency_path, &FORBIDDEN_WORDS).is_empty(),
        "那条依赖路径应当被放行"
    );
    let other_path = strip_allowed_phrases("use reqwest::Url;", "im_app.rs", &CORE_ALLOWED_PHRASES);
    assert!(
        !forbidden_word_hits(&other_path, &FORBIDDEN_WORDS).is_empty(),
        "依赖路径的放行不许扩到别的文件"
    );
    assert!(
        marker_present("let a = open_app;", "open_app"),
        "导航记号扫描必须能报出违规"
    );
    assert!(
        marker_present("action \"not-expanded\"", "not-expanded"),
        "带连字符的导航记号必须能被认出"
    );
    // 反例：同前缀的普通命名不得被当成导航记号，否则守卫只会逼代码改名。
    assert!(
        !marker_present("let handle = open_append(root);", "open_app"),
        "`open_append` 不是导航记号 `open_app`"
    );
    // App 独立性守卫的自检：它必须能报出"一个 App 引用了另一个 App"这种反例。
    assert!(
        "use crate::file_app::FileApp;"
            .to_lowercase()
            .contains("file_app"),
        "App 独立性守卫必须能认出跨 App 的引用"
    );
    // im 家族包间引用的自检：**代码行**里的模块路径必须被认出；文档里点名一个包不是依赖
    // （`code_lines` 的判据与显示区那条守卫同一份）。
    assert!(
        code_lines("use crate::im_social_app::ImSocialApp;").contains("crate::im_social_app"),
        "im 家族包间引用的守卫必须能认出代码行里的模块路径"
    );
    assert!(
        code_lines("let app = im_profile_app::ImProfileApp::new(access);")
            .contains("im_profile_app::"),
        "包间引用也可能是调用形态（`<模块>::`），同样必须被认出"
    );
    assert!(
        !code_lines("//! 这一组动词住在 `im_social_app` 那个包里。")
            .contains("crate::im_social_app"),
        "文档里点名一个包不是实现依赖"
    );
    assert!(
        !marker_present("let x = reopen_application();", "open_app"),
        "`reopen_application` 不是导航记号 `open_app`"
    );
}

/// 一个源文件里**代码行**（注释行剔除）的全部文本。
///
/// 文档里点名一个模块（或一条 `[`crate::x`]` 文档链接）**不是依赖**——守卫不该逼着代码为它改
/// 措辞，否则守卫就从"约束结构"退化成"约束用词"（判据见 `a_display_area_never_knows_an_app`）。
fn code_lines(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// **分层守卫：显示区不许认识 App**——手机模型里最硬的那条方向线。
///
/// §3 的四层自下而上是 **1 循环＋插槽 / 2 显示区 / 3 app / 4 Agent**，规则是
/// "**上层依赖下层，下层不认识上层**"；同一节第 3 条又写着 app **"往显示区交素材"**。
/// 两句话合起来只允许一个方向：**`app → 显示区`**。
///
/// 素材的形状由**显示区**定义——真实手机里 `Notification` 那个类是**平台**定义的、app 填内容。
/// 因此 [`crate::holdings::HoldingRow`] / [`crate::pins_board::PinnedRow`] 住在显示区，
/// `timer_app` / `notes_app` 依赖它们把内容填进去。反过来的写法（显示区 import 某个 App 的记录
/// 类型、装配层直接翻那个 App 的私有账本）等于"锁屏去翻时钟 App 的数据库"——本项目里曾经真的
/// 这么写过两处（组件区认 `notes_app::PinnedNote`、持有物认 `timer_app::Entry`），本守卫就是
/// 为了不让它第三次出现。
///
/// ⚠️ **注释行不参与**：文档里提到一个 App 的名字（或一条 `[`crate::im_app`]` 文档链接）不是依赖。
/// 守卫不该逼着代码为它改措辞——那会让守卫从"约束结构"退化成"约束用词"。
///
/// 文本级判断（不是链接期证明）。
#[test]
fn a_display_area_never_knows_an_app() {
    // ⚠ **2026-10-06 补两个漏项**：`previous_activity`（开场那张纸条）与 `activity_origin`
    // （起源那句）都是显示区，而它们从前**不在名单里** ⇒ **不受这条守卫管**。
    //
    // ⚠ **而"显示区不许认识 `store`"那条判据【不成立】，因此这里不量它**：`previous_activity`
    // 直接 import 了 `crate::store::StorageFailure`——那是**设计要求**（那张纸条的职责之一就是
    // 把"历史上最后一次存储故障"带给主体）。⇒ 本守卫的判据仍是原来那一条（**不许认识 App**），
    // 一个字没放宽、也没新增。
    const DISPLAY_AREAS: [&str; 12] = [
        "status_bar",
        "notification_bar",
        "pins_board",
        "pinned_actions",
        "skills_desktop",
        "event_cards",
        "self_concept",
        "handoff",
        "holdings",
        "machine",
        "previous_activity",
        "activity_origin",
    ];
    const APPS: [&str; 9] = [
        "file_app",
        "file_view",
        "notes_app",
        "im_app",
        // **2026-10-02 im 拆包**：社交面与资料面是**两个新的 App**，显示区一样不许认识它们。
        "im_social_app",
        "im_profile_app",
        "web_app",
        "terminal_app",
        "timer_app",
    ];
    let sources = core_sources();
    for area in DISPLAY_AREAS {
        // `core_sources` 给的是**文件名**（带 `.rs`）；下面判的是**模块路径**（不带）。
        let source = sources
            .iter()
            .find(|(name, _)| name == &format!("{area}.rs"))
            .map(|(_, source)| source)
            .unwrap_or_else(|| panic!("{area}.rs 不在 src/ 下：名单过期了"));
        let code = code_lines(source);
        for app in APPS {
            assert!(
                !code.contains(&format!("crate::{app}")),
                "{area} 认识了 App {app}：显示区在下、app 在上，下层不许认识上层"
            );
        }
    }

    // 自检：**必须有 App 真的依赖显示区**（交素材），否则这条守卫是空的。
    let mut materials = 0usize;
    for (name, source) in &sources {
        if !APPS.iter().any(|app| name == &format!("{app}.rs")) {
            continue;
        }
        let code = code_lines(source);
        for area in DISPLAY_AREAS {
            if code.contains(&format!("crate::{area}")) {
                materials += 1;
            }
        }
    }
    assert!(
        materials >= 2,
        "至少要有一个 App 依赖显示区并交素材，否则守卫是空的（实得 {materials}）"
    );

    // 反例：扫描器必须认得出真正的违规，否则上面那两条可能是永久绿的。
    assert!(
        code_lines("use crate::timer_app::Entry;").contains("crate::timer_app"),
        "代码行里的跨层引用必须被认出来"
    );
    assert!(
        !code_lines("//! 素材的唯一装配点是 [`crate::im_app`] 的渲染。").contains("crate::im_app"),
        "注释行里的文档链接不是依赖"
    );
}

/// **地基方向守卫：Foundation 不许认识 Core。**
///
/// `AGENTS.md` 的硬线写着"**Foundation 不依赖 Core**"，而它从前**没有任何守卫**：下面那张
/// [`CORE_SOURCES`] 名单把 `core.rs` 与 `foundation.rs` 当成**一坨**，于是它只能问
/// "**这一坨**有没有碰 adapter / app / os"，**问不出这一坨内部谁依赖谁**。
///
/// 结果是那条硬线被违反了很久：`SystemEnding::ModelFailed(Option<crate::core::ModelFailureClass>)`
/// 就是那个环——Core 从 Foundation 拿 12 样东西（上层用下层，合法），Foundation 却反过来伸手够
/// Core 的一个类型。而且它是两个文件之间**唯一**的一处。
///
/// ⚠️ 环的坏处不是编译不过（同一个 crate 编得过），是**方向**：Foundation 是账本（盘上格式，
/// 最不容易变的东西），Core 是规则书（天天在变）。环的意思是**规则书一改，账本跟着改**。
///
/// 方向必须**两边都证明**，否则这条守卫可以靠"两边都不引用"假绿：
///
/// - **下行合法且必须存在**：Core 依赖 Foundation（上层用下层）；
/// - **上行禁止**：Foundation 一个字都不许提 Core。
#[test]
fn foundation_never_knows_core() {
    let sources = core_sources();
    let source_of = |file: &str| -> String {
        sources
            .iter()
            .find(|(name, _)| name == file)
            .map(|(_, source)| source.clone())
            .unwrap_or_else(|| panic!("{file} 不在 src/ 下"))
    };

    let foundation = source_of("foundation.rs");
    assert!(
        !foundation.contains("crate::core"),
        "foundation.rs 认识了 Core：地基在下、Core 在上，下层不许认识上层"
    );

    // 反向自检：Core **必须**依赖 Foundation，否则上面那条可以靠"两边都不引用"假绿。
    assert!(
        source_of("core.rs").contains("crate::foundation"),
        "core.rs 没有依赖 foundation：方向没被证明，上面那条守卫是空的"
    );

    // 反例：扫描器必须认得出真正的违规（就是刚拆掉的那一行）。
    assert!(
        "ModelFailed(Option<crate::core::ModelFailureClass>),".contains("crate::core"),
        "扫描器必须认得出跨层引用"
    );
}

#[test]
fn minimal_language_stays_free_of_legacy_and_vendor_words() {
    let sources = core_sources();
    let names: Vec<String> = sources.iter().map(|(name, _)| name.clone()).collect();

    for adapter in ADAPTER_SOURCES {
        assert!(
            names.iter().any(|name| name == adapter),
            "跳过名单里的 {adapter} 不在 src/ 下：名单过期了"
        );
    }

    let mut scanned = 0usize;
    for (name, source) in &sources {
        if ADAPTER_SOURCES.contains(&name.as_str()) {
            // Adapter 必须真的在说供应商语言，否则把它排除在 Core 扫描之外毫无意义。
            assert!(
                !forbidden_substring_hits(source, &FORBIDDEN_SUBSTRINGS).is_empty(),
                "{name} 被当作 Adapter 排除，却没有出现任何供应商语言"
            );
            continue;
        }

        scanned += 1;

        let mut text = source.clone();
        if name == FACADE_SOURCE {
            assert!(
                text.contains(&format!("pub mod {FACADE_MODULE_NAME};")),
                "{FACADE_SOURCE} 里没有 Adapter 模块声明，豁免就失去依据"
            );
            text = text.replace(FACADE_MODULE_NAME, "adapter_module");
        }

        // 子串扫描自己有一份**短语级**放行名单（与短词那份互不影响）：见
        // `SUBSTRING_ALLOWED_PHRASES` 的理由。
        let substring_text = strip_allowed_phrases(&text, name, &SUBSTRING_ALLOWED_PHRASES);
        let substrings = forbidden_substring_hits(&substring_text, &FORBIDDEN_SUBSTRINGS);
        assert!(
            substrings.is_empty(),
            "{name} 出现了本轮明确排除的词: {substrings:?}"
        );

        // 中立层里**按整条短语**放行的记号：见 `CORE_ALLOWED_PHRASES` 的理由。
        // 放行的是"这条短语"，不是"这个词"——短语被去掉之后，短词守卫照旧生效。
        let text = strip_allowed_phrases(&text, name, &CORE_ALLOWED_PHRASES);

        let words: Vec<String> = forbidden_word_hits(&text, &FORBIDDEN_WORDS)
            .into_iter()
            .filter(|word| {
                // `json` 是序列化格式记号，不是供应商协议：它按 SERDE_JSON_SOURCES 单独放行。
                !(SERDE_JSON_SOURCES.contains(&name.as_str())
                    && FORBIDDEN_WORD_EXCEPTIONS.contains(&word.as_str()))
            })
            .collect();
        assert!(words.is_empty(), "{name} 出现了越界短词: {words:?}");
    }

    assert_eq!(
        scanned,
        sources.len() - ADAPTER_SOURCES.len(),
        "每个源文件要么被扫描，要么明确列在 Adapter 名单里"
    );

    // **豁免是债务**：每条今天都必须仍然成立（照名单补进去真的会红）。原因修掉之后豁免就成了
    // 空账，必须跟着删——这一格与 `the_source_dependency_graph_has_no_cycles` 的债务账同款。
    let mut exemptions_still_needed: Vec<&str> = Vec::new();

    for core_name in CORE_SOURCES {
        let source = sources
            .iter()
            .find(|(name, _)| name == core_name)
            .map(|(_, source)| source)
            .unwrap_or_else(|| panic!("{core_name} 不在 src/ 下"));
        let lowered = source.to_lowercase();
        // ⚠ **判据落在代码行上**（注释行剔除，2026-10-10）：文档里点名一个模块不是依赖。
        // 收窄的理由与两种形态的说明都在 `references_module` 上。
        let code = code_lines(&lowered);

        // 边界模块名按**整词 / 路径记号**匹配，不按裸子串。
        //
        // `os`、`store`、`history` 这类短名当裸子串扫描会大量误伤正常词
        // （`chosen`、`composition`、`fail-closed`……），一旦为了让守卫"变绿"就去改正常
        // 措辞，守卫就从"约束结构"退化成"约束用词"。所以这里只认两种形态：
        // 路径形式的 `crate::<模块>`，以及作为独立标识符出现的模块名。
        for boundary in BOUNDARY_MODULES {
            assert!(
                !references_module(&code, boundary),
                "{core_name} 引用了边界模块 {boundary}：Core 不该认识适配层或 App 层"
            );
        }
        for (boundary, _) in CORE_BOUNDARY_EXEMPTIONS {
            if references_module(&code, boundary) {
                exemptions_still_needed.push(boundary);
            }
        }
        for dependency in ADAPTER_DEPENDENCIES {
            assert!(
                !lowered.contains(dependency),
                "{core_name} 引用了 Adapter 的传输依赖 {dependency}"
            );
        }
    }

    for (boundary, reason) in CORE_BOUNDARY_EXEMPTIONS {
        assert!(
            !reason.trim().is_empty(),
            "豁免「{boundary}」没写理由 —— 豁免的分工就是「为什么这条判据在它身上失效」"
        );
        assert!(
            exemptions_still_needed.contains(&boundary),
            "豁免「{boundary}」今天不再需要了（{reason}）—— 把它从 CORE_BOUNDARY_EXEMPTIONS \
             删掉、放回 BOUNDARY_MODULES，别留一条空豁免"
        );
    }
}

/// **模块账本守卫**（2026-10-18）：`src/` 里每一个顶层模块都必须在分层账本里**有名字**。
///
/// **原约束**：[`BOUNDARY_MODULES`] 是**手抄的**——"Core / Foundation 不许认识它"这条约束只覆盖
/// 名单里写下的名字，因此**名单少一个 = 少一条约束**，而那是**静默**的。这条机制的代价，
/// 名单自己的注释里就记着（"后加的模块没人回头补"）：**2026-10-10 一次性补了 11 项**
/// （原因正是它），**2026-10-11 落的两件又漏了**（`resident_lifetime` / `terminal_held`）。
/// ⇒ 同一个形状**复发两次**，说明缺的不是"下次记得补"，是**判据没有派生口**。
///
/// **新边界**：模块名单改成**派生的**——谁都不用记得回头补，**新模块自己会要一个说法**。
/// 三张账合起来才算记账：[`BOUNDARY_MODULES`]（受这条约束）∪ [`CORE_SOURCES`]（约束的施加者）
/// ∪ [`CORE_BOUNDARY_EXEMPTIONS`]（写明"这条判据在它身上为什么失效"）。落在三者之外 ⇒ 红。
///
/// **真实消费者**：[`boundary_apis_stay_in_their_own_layer`] 逐条遍历 `BOUNDARY_MODULES` 去量
/// 两份核心源码——名单与约束是同一件事的两面，所以这张表不许靠"有人记得"维护。
///
/// **判据是相等，不是包含**（与 `the_source_dependency_graph_has_no_cycles` 的债务账同款）：
/// - 新增模块没记账 ⇒ 红（挡住新债）；
/// - 账本里留着一个**已经不存在的模块** ⇒ 也红（名单过期与漏项同罪：两者都让"全绿"贬值）。
///
/// ⚠ **它量不到**：名字进了账本、但**这条约束对那个模块是否恰当**——那要人判，
/// [`CORE_BOUNDARY_EXEMPTIONS`] 的"理由"那一列就是留给人的位置。
///
/// ⚠ **射程**：粒度是**顶层模块**（与依赖图守卫同款），子目录内部的模块不在射程里；
/// 非 `pub` 的私有子模块也不进模块表（`declared_modules` 只认 `pub mod`）——那种情况下
/// 第一条断言（声明 ↔ 文件）会红，因为文件在而声明不在。
#[test]
fn every_module_is_accounted_for_in_the_layering_ledger() {
    // **派生一**：`lib.rs` 的模块表（`pub mod x;`）——§6 说的"模块表"就是它。
    let sources = core_sources();
    let facade = sources
        .iter()
        .find(|(name, _)| name == FACADE_SOURCE)
        .map(|(_, source)| source)
        .unwrap_or_else(|| panic!("{FACADE_SOURCE} 不在 src/ 下"));
    let declared = declared_modules(facade);

    // **派生二**：`src/` 下真实存在的顶层模块（`lib` 自己是门面，不是模块）。
    let mut present: BTreeSet<String> = BTreeSet::new();
    for (file, _) in &sources {
        let module = top_level_module(file);
        if module != "lib" {
            present.insert(module);
        }
    }

    // 两个派生口必须**逐名对齐**：声明了却没有文件、或有文件却没声明，都是模块表在漂。
    assert_eq!(
        declared, present,
        "`lib.rs` 声明的模块表与 `src/` 下的文件对不上 —— 声明与文件必须一一对应。\n\
         §6：**新增一个模块必须说明它替掉了谁、消掉了哪份重复**，因此新模块要进模块表，\n\
         不能只放一个文件在那里。"
    );

    // 账本：三张账合起来才算记账。
    let mut ledger: BTreeSet<String> = BTreeSet::new();
    for name in BOUNDARY_MODULES {
        ledger.insert(name.to_string());
    }
    for name in CORE_SOURCES {
        ledger.insert(name.trim_end_matches(".rs").to_string());
    }
    for (name, _) in CORE_BOUNDARY_EXEMPTIONS {
        ledger.insert(name.to_string());
    }

    // **新增模块没记账** ⇒ 红。
    let unaccounted: Vec<&String> = declared.difference(&ledger).collect();
    assert!(
        unaccounted.is_empty(),
        "这些模块没有账：{unaccounted:?}\n\
         判据是「Core / Foundation 该不该认识它」，不是「有没有人记得补」：\n\
         · 不该认识 ⇒ 加进 BOUNDARY_MODULES；\n\
         · 该认识（它是 Core 自己的中立词汇）⇒ 加进 CORE_BOUNDARY_EXEMPTIONS，并写明这条\n\
           判据在它身上为什么失效——那份理由是留给人的位置，守卫替你判不了。"
    );

    // **账本里的过期名字** ⇒ 也红。
    let stale: Vec<&String> = ledger.difference(&declared).collect();
    assert!(
        stale.is_empty(),
        "账本里有已经不存在的模块：{stale:?} —— 名单过期与漏项同罪：两者都会让「全绿」这句话贬值"
    );

    // --- 自检四条：防止上面三条永久假绿 ---

    // ① 派生口真的派生出东西了吗。
    assert!(
        declared.len() >= 40,
        "只从 {FACADE_SOURCE} 认出 {} 个模块 —— 解析器坏了，守卫形同虚设",
        declared.len()
    );

    // ② 反例：没记账的模块必须被报出来。
    let mut with_newcomer = declared.clone();
    with_newcomer.insert("a_module_nobody_accounted_for".to_string());
    assert_eq!(
        with_newcomer.difference(&ledger).count(),
        1,
        "扫描器必须报出没记账的模块"
    );

    // ③ 反例：账本里的过期名字必须被报出来。
    let mut with_ghost = ledger.clone();
    with_ghost.insert("a_module_that_no_longer_exists".to_string());
    assert_eq!(
        with_ghost.difference(&declared).count(),
        1,
        "扫描器必须报出账本里的过期名字"
    );

    // ④ 解析器认得出声明、跳得过注释、也不把非 pub 的声明当模块表。
    assert!(
        declared_modules("pub mod os;\n").contains("os"),
        "代码行里的 `pub mod` 必须被认出来"
    );
    assert!(
        declared_modules("//! 从前这里写着 pub mod gate;\n").is_empty(),
        "注释里的声明不是模块"
    );
    assert!(
        declared_modules("mod private_helper;\n").is_empty(),
        "非 pub 的声明不进模块表"
    );
}

/// 扫描一段**代码行**里出现的字面量（注释行已被剔除）。
fn literal_hits(source: &str, literal: &str) -> usize {
    code_lines(source).matches(literal).count()
}

/// **词表权威守卫**（2026-10-06）：共用的机械类别与收场显示文本**只许有一处拼法**。
///
/// 为什么这条要单独守：它们是**逐字落盘、逐字给人看**的稳定词——同一个结论两种拼法，历史里就会
/// 出现两种事实（或同一条事实有两种说法），而没有任何别的判据会红。2026-10-06 审计实测：
/// `unknown-capability` 在**八个 App 里各拼一遍**（九处字面量）；收场显示文本有**四处私有副本**
/// （`agent_host_status` / `host_loop` / `manual_entry` / `diagnostics`），其中两处同名函数的形状
/// 还不一样。
///
/// ⚠ 两点与别的守卫不同：
///
/// - 它**含 `src/bin/`**（分层守卫跳过夹具区）：重复的渲染副本里就有一个在 bin 里
///   （`agent_host_status`），跳过它就等于让守卫看不见真实违规；
/// - 判据是**带引号的字面量**，不是裸子串：文档里点名一个分类（讲清楚某条路会回什么）照旧可以写，
///   守卫不该逼着代码为它改措辞。
///
/// 反例自检在末尾：同一个扫描器认得出真违规，也不把注释里的点名当违规。
#[test]
fn the_shared_class_words_and_ending_labels_have_one_authority() {
    // (字面量, 唯一允许拼它的文件)
    const ONE_AUTHORITY: [(&str, &str); 4] = [
        ("\"unknown-capability\"", "core.rs"),
        ("\"invalid-argument\"", "core.rs"),
        // 收场显示文本的拼法：`"subject:` / `"system:` 只许出现在权威那一处
        // （权威自己用 `{SUBJECT_PREFIX}:` 拼，因此这两条今天是"哪儿都不许出现"）。
        ("\"subject:", "store.rs"),
        ("\"system:", "store.rs"),
    ];
    // 正例：这些 App 的准入面必须**引用**那一处权威，而不是自己再拼一遍字面量。
    const DELEGATING: [&str; 9] = [
        "file_app.rs",
        "im_app.rs",
        "im_profile_app.rs",
        "im_social_app.rs",
        "mcp/app.rs",
        "notes_app.rs",
        "terminal_app.rs",
        "timer_app.rs",
        "web_app.rs",
    ];

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_rust_sources(&root, &mut paths);
    assert!(!paths.is_empty(), "守卫形同虚设：一个源文件都没扫到");

    let mut violations: Vec<String> = Vec::new();
    for path in &paths {
        let relative = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let source = std::fs::read_to_string(path).expect("读源码");
        for (literal, authority) in ONE_AUTHORITY {
            if relative == authority {
                continue;
            }
            let hits = literal_hits(&source, literal);
            if hits > 0 {
                violations.push(format!(
                    "{relative} 里有 {hits} 处 {literal}（权威在 {authority}）"
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "共用的词只许有一处拼法：\n{}",
        violations.join("\n")
    );

    for name in DELEGATING {
        let source = std::fs::read_to_string(root.join(name))
            .unwrap_or_else(|_| panic!("{name} 不在 src/ 下"));
        assert!(
            code_lines(&source).contains("UNKNOWN_CAPABILITY"),
            "{name} 的准入面必须引用那一处权威（`crate::core::UNKNOWN_CAPABILITY`），而不是自己拼"
        );
    }

    // 反例自检：真违规认得出；注释里的点名不算违规。
    assert_eq!(
        literal_hits(
            "let class = \"unknown-capability\";",
            "\"unknown-capability\""
        ),
        1
    );
    assert_eq!(
        literal_hits(
            "// 这条路会回 \"unknown-capability\"",
            "\"unknown-capability\""
        ),
        0,
        "注释行不参与：文档里点名一个分类不是拼第二份"
    );
}

/// **活动失败的十个词只有一处拼法，两个渲染面只许委托**（2026-10-06 收）。
///
/// 为什么要有这一条：`RunError` 的十个稳定标记从前只以**私有副本**活在 `manual_entry.rs` 里，
/// 而常驻宿主那一侧**一个词都不区分**——四类完全不同的失败（事实写失败 / 保存未确认 /
/// 活动槽被占 / 轮次用尽）在操作者读数里塌成同一个 `run-refused`，退出码也一律相同。
/// 今天它们归 `core.rs` 的 [`RunError::as_str`]，两个渲染面**委托**它。
///
/// ⚠ **它只证明两件事**：① 那十个字面量只出现在权威那一处；② 两个渲染面**引用了**权威。
/// 它量不到"引用得对不对"（`error.as_str()` 出现在别的上下文里它照样绿），也量不到
/// 退出码那一侧的分类——那两条归各自的用例与读数。
///
/// 反例自检在末尾：同一个扫描器认得出别处再拼一遍。
#[test]
fn the_activity_failure_labels_have_one_authority() {
    const AUTHORITY: &str = "core.rs";
    // `RunError` 的十个取值，逐字照 `as_str` 抄——少一个、错一个，这条守卫自己就红了。
    const LABELS: [&str; 10] = [
        "\"active-run-exists\"",
        "\"no-turn-budget\"",
        "\"already-ended\"",
        "\"operation-in-flight\"",
        "\"turn-budget-exhausted\"",
        "\"operation-out-of-order\"",
        "\"operation-mismatch\"",
        "\"agent-mismatch\"",
        "\"ledger\"",
        "\"commit\"",
    ];
    // 正例：这两处渲染面必须**委托**权威，不许自己再拼一遍（判据见下面 ③）。

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_rust_sources(&root, &mut paths);
    assert!(!paths.is_empty(), "守卫形同虚设：一个源文件都没扫到");

    // ① 权威那一处必须真的拼着这十个词（表被清空、名字被改都会红）。
    let authority = std::fs::read_to_string(root.join(AUTHORITY)).expect("读权威");
    for label in LABELS {
        assert!(
            code_lines(&authority).contains(label),
            "{AUTHORITY} 里少了 {label}——十个词少一个，守卫就是假绿"
        );
    }

    // ② 别处一个都不许有。
    let mut violations: Vec<String> = Vec::new();
    for path in &paths {
        let relative = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if relative == AUTHORITY {
            continue;
        }
        let source = std::fs::read_to_string(path).expect("读源码");
        for label in LABELS {
            let hits = literal_hits(&source, label);
            if hits > 0 {
                violations.push(format!(
                    "{relative} 里有 {hits} 处 {label}（权威在 {AUTHORITY}）"
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "活动失败的词只许有一处拼法：\n{}",
        violations.join("\n")
    );

    // ③ 两个渲染面必须**带着分类**委托权威那一处。
    //
    // ⚠ 这一半的判据是**两条一起**，因为单独任何一条都太松：
    // - 只查 `error.as_str()`：`host_loop.rs` 里 `open_when_free` 那一行也是 `error.as_str()`
    //   （那是 `StoreError`）⇒ 把活动面改回粗词它照样绿（**已实测**：第一版就是这么写的）；
    // - 只查格式串：`format!("activity-failure={}", 一个写死的词)` 也满足。
    // 两条一起，才钉住"前缀还在、分类由权威给"这个形状。真实回归的两条路因此都被盖住：
    // **塌回一个词**（格式串不再带占位）与**再抄一份词**（①会红）。
    const DELEGATION_SHAPE: [(&str, &str); 2] = [
        ("manual_entry.rs", "\"activity-failure={}\""),
        ("host_loop.rs", "\"run-refused:{}\""),
    ];
    for (name, shape) in DELEGATION_SHAPE {
        let source = std::fs::read_to_string(root.join(name))
            .unwrap_or_else(|_| panic!("{name} 不在 src/ 下"));
        let code = code_lines(&source);
        assert!(
            code.contains(shape),
            "{name} 的活动失败读数必须带着分类（{shape}），不许塌回一个词"
        );
        assert!(
            code.contains("error.as_str()"),
            "{name} 的活动失败读数必须委托 `RunError::as_str`，而不是自己拼一个粗的词"
        );
    }

    // 反例自检：别处再拼一遍，扫描器认得出。
    assert_eq!(
        literal_hits("let label = \"agent-mismatch\";", "\"agent-mismatch\""),
        1
    );
    assert_eq!(
        literal_hits("// 这条路会回 \"agent-mismatch\"", "\"agent-mismatch\""),
        0,
        "注释行不参与：文档里点名一个分类不是拼第二份"
    );
}

/// **处境链的硬上限：11 层**（2026-10-08 主人确认，C8）。
///
/// 判据与"**新增一个动词必须删掉一个**"同款：**要加第 12 层，先删一层**。
///
/// 为什么要有它：链是**编译期嵌套**（`composition::SituationChain`）——加一层要同时改嵌套类型、
/// 唯一装配点与文档；而换成注册表/列表被纪律禁了（不预建万能注册中心）⇒ **这里没有免费午餐，
/// 只能设上限**。上限存在的意义不是"11 这个数字好"，是让"攒层"从**默认允许**变成**必须先删**。
///
/// 数法：`type SituationChain = <嵌套泛型>;` 里 `<` 的个数 **＋ 1**（最内层那个件没有自己的 `<`）。
/// 认不出那条别名 ⇒ **报错**（不许静默通过）；反例自检在末尾。
#[test]
fn the_opening_view_chain_has_a_hard_ceiling() {
    const CEILING: usize = 11;
    let source =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/composition.rs"))
            .expect("读 composition.rs");
    let layers =
        chain_layers(&source).expect("composition.rs 里必须有一条 `type SituationChain = …;`");
    assert!(
        layers <= CEILING,
        "处境链已经 {layers} 层，上限是 {CEILING}：要加第 {} 层，先删一层（C8）",
        CEILING + 1
    );
    // 反例自检：数法与上限都真的在量东西。
    assert_eq!(chain_layers("type SituationChain = A<B<C>>;"), Some(3));
    assert_eq!(
        chain_layers("type Other = A<B<C<D<E<F<G<H<I<J<K<L>>>>>>>>>>;"),
        None
    );
    assert!(
        chain_layers("type SituationChain = A<B<C<D<E<F<G<H<I<J<K<L>>>>>>>>>>;")
            .is_some_and(|n| n > CEILING),
        "12 层必须被判成超限"
    );
}

/// 数一条 `type SituationChain = …;` 的层数：`<` 的个数 ＋ 1。找不到别名 ⇒ `None`。
fn chain_layers(source: &str) -> Option<usize> {
    const HEAD: &str = "type SituationChain =";
    let start = source.find(HEAD)? + HEAD.len();
    let body = &source[start..];
    let end = body.find(';')?;
    Some(body[..end].matches('<').count() + 1)
}

/// **人不许比系统知道得更少**（2026-10-08 主人确认，三类读者里的第 3 档）。
///
/// 判据一句话：**同一族的两个不同类别，操作者不许读到同一句话。**
///
/// 它防的正是 2026-10-06 修过的那类毛病：四种 Run 失败在宿主读数里塌成一个 `run-refused`。
/// 退出码**可以**粗（`2` / `4` / `6` 是三个桶），**读数行必须细**——桶里是哪一类，必须在那一行
/// 里读得出来；`[next]` 那一行也不能各族共用一句话（那等于让人去修一个没坏的东西）。
///
/// 判法是**完备**的：把 `HostFailure` 的每一个取值（含五个门铃分类）都造出来，断言
/// ① 分类标记两两不同；② `[next]` 那一行两两不同；③ 每个标记都是稳定机读形状（小写与连字符）。
/// 另外钉两条**形状**：两份 `as_str()` 的 match 里**不许有兜底 `_ =>`**——有了它，新加一个分类
/// 就会静默地拿到别人的标签，而这里量不出来。
///
/// ⚠ 边界：它只量**宿主退出**那一族。其余给操作者的读数由
/// `the_activity_failure_labels_have_one_authority`（活动失败十词唯一权威）分担；
/// 两者加起来才算这一档守住了。
#[test]
fn no_two_operator_readings_are_the_same() {
    use celira_agent::doorbell::DoorbellFailure;
    use celira_agent::host_loop::HostFailure;

    let root = Path::new("/tmp/celira-home");
    let agent = AgentId::new("celira-agent");
    let failures: Vec<HostFailure> = vec![
        HostFailure::Environment("资料根不可用".to_string()),
        HostFailure::EnvironmentBusy { waited_ms: 1200 },
        HostFailure::Doorbell(DoorbellFailure::CredentialRejected),
        HostFailure::Doorbell(DoorbellFailure::ChannelLimitExceeded),
        HostFailure::Doorbell(DoorbellFailure::UnexpectedResponse),
        HostFailure::Doorbell(DoorbellFailure::UnexpectedMediaType),
        HostFailure::Doorbell(DoorbellFailure::ChannelLost),
    ];

    let marks: Vec<&str> = failures.iter().map(HostFailure::as_str).collect();
    let nexts: Vec<String> = failures
        .iter()
        .map(|failure| failure.next_line(root, &agent))
        .collect();

    for (index, left) in marks.iter().enumerate() {
        assert!(
            left.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
            "分类标记必须是稳定机读形状（小写与连字符），而 `{left}` 不是"
        );
        assert!(
            !left.is_empty(),
            "每一类都要有自己的标记：第 {index} 类没有"
        );
        for (other, right) in marks.iter().enumerate() {
            assert!(
                index == other || left != right,
                "两类失败共用一个分类标记 `{left}`——那正是「四种失败塌成一个词」那个毛病"
            );
        }
        for (other, right) in nexts.iter().enumerate() {
            assert!(
                index == other || nexts[index] != *right,
                "两类失败的 `[next]` 是同一句话：{right}\n\
                 那会让人照着这句话去修一个没坏的东西。"
            );
        }
    }

    // 形状：两份词表的 match 不许有兜底臂。
    let sources = [
        ("host_loop.rs", "pub fn as_str(&self) -> &'static str {"),
        ("doorbell.rs", "pub fn as_str(&self) -> &'static str {"),
    ];
    for (name, mark) in sources {
        let source =
            fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(name))
                .unwrap_or_else(|error| panic!("读不到 {name}: {error}"));
        let body = source
            .split(mark)
            .nth(1)
            .unwrap_or_else(|| panic!("{name} 里找不到 `{mark}`"));
        let body = &body[..body.find("\n    }").unwrap_or(body.len())];
        assert!(
            !body.contains("_ =>"),
            "{name} 的分类 match 里有兜底臂 `_ =>`：新加一个分类会静默拿到别人的标签，\
             而完备性用例量不出来。要么逐个列出来，要么在词表那层显式委托。"
        );
    }
}

/// **她看得见的那几行字里，一处比喻也不许有**（2026-10-08 主人确认，C10）。
///
/// 判据（2026-10-05 裁决）：**比喻不进主体的上下文**。
///
/// **账已经还清**：2026-10-08 之前这里是一张**债务账**（18 行 / 7 个文件，新增即红、修掉也要红）。
/// 那一刀把 18 行全改成机械说法（`贴到墙上` → `标上 pinned`、`纸条` → `笔记`、
/// `五堵机械墙` → `五条机械限制`、`材料窗` → `只读文件视图`）⇒ 账目清零，
/// **判法升格成"一处也不许有"**：今天再有命中就是**新增**——不许记账，直接改。
///
/// ⚠ **边界写在明处**：它只认 `METAPHOR_WORDS` 那十六个词，只扫 `HER_SURFACE` 那批
/// **会进她视野的文件**，注释行不参与（注释是给写代码的人的那一档；那边另有一笔
/// **别名债**记在 `agent/README.md` §3.37 ④）。它**不是**"表述全清干净"的证明——
/// 门牌那种"散文式的判断"它量不到（那要 C10 的措辞判据：一句话 / 不带标记语法 / 不带比喻）。
/// 反例自检在末尾。
///
/// **2026-10-18 扩族那一刀**（同一天改掉的那几处）：`terminal 那只手` ×4
/// （`file_view.rs` 的作品面详情、`orientation.rs` 手册三处）⇒ **`terminal` 档在场 / 不在**；
/// `你的地盘`（`self_concept.rs` 的"读不到自识"那一句）⇒ **你的环境根**；
/// `宿主授权的地界`（`web_app.rs` 门牌）⇒ **宿主授权的资料目录**。
/// `身体` 那三处**没动**——它待裁（见 `METAPHOR_WORDS` 的例外段）。
#[test]
fn her_visible_words_carry_no_new_metaphor() {
    /// 会进她视野的那批源文件（显示区 ＋ App ＋ 出厂说明 ＋ 装配与路由）。
    const HER_SURFACE: [&str; 25] = [
        "activity_origin.rs",
        "composition.rs",
        "event_cards.rs",
        "file_app.rs",
        "file_view.rs",
        "holdings.rs",
        "im_app.rs",
        "im_profile_app.rs",
        "im_social_app.rs",
        "machine.rs",
        "mcp/app.rs",
        "notes_app.rs",
        "notification_bar.rs",
        "orientation.rs",
        "os.rs",
        "pinned_actions.rs",
        "pins_board.rs",
        "previous_activity.rs",
        "skills_desktop.rs",
        "self_concept.rs",
        "handoff.rs",
        "status_bar.rs",
        "terminal_app.rs",
        "timer_app.rs",
        "web_app.rs",
    ];
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found: Vec<(&str, usize)> = Vec::new();
    for name in HER_SURFACE {
        let path = root.join(name);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("{name} 不在 src/ 下——表面名单必须指着真文件"));
        let hits = metaphor_hits(&source, &METAPHOR_WORDS);
        if hits > 0 {
            found.push((name, hits));
        }
    }
    assert!(
        found.is_empty(),
        "她的视野里出现了比喻（2026-10-05 裁决：比喻不进主体的上下文）：{found:?}\n\
         改成机械说法：位置与东西用标识符（`pinned` / `notes_pin` / `pinned-notes`），\
         边界用「机械限制」，不要用环境比喻。"
    );

    // 反例自检：扫描器认得出，且注释行与中立词不误伤。
    assert_eq!(
        metaphor_hits("let text = \"墙上没有\";", &METAPHOR_WORDS),
        1
    );
    assert_eq!(metaphor_hits("// 墙上没有", &METAPHOR_WORDS), 0);
    assert_eq!(
        metaphor_hits(
            "let window = 上下文窗口; let cap = 五条机械限制;",
            &METAPHOR_WORDS
        ),
        0,
        "中立词（窗口）不在封闭表里，不许误伤"
    );
    // **扩族那一半的反例**（2026-10-18）：同一个比喻的其它写法必须被抓到——
    // 这三条正是原表漏过去、而她看得见的那三处。
    assert_eq!(
        metaphor_hits(
            "let text = \"要跑它得有 `terminal` 那只手\";",
            &METAPHOR_WORDS
        ),
        1,
        "`手脚` 的同族写法必须抓到（它就是从前漏掉的那一处）"
    );
    assert_eq!(
        metaphor_hits("let text = \"它还在你的地盘上\";", &METAPHOR_WORDS),
        1,
        "`自留地` 的同族写法必须抓到"
    );
    assert_eq!(
        metaphor_hits("let text = \"宿主授权的地界\";", &METAPHOR_WORDS),
        1
    );
    // **不误伤那一半**：裸 `手` 不进表，所以这三个合法词一个都不许命中。
    assert_eq!(
        metaphor_hits(
            "let text = \"手册在 files 视野的 manual.md 里；自识是主体的手笔；助手不是名字\";",
            &METAPHOR_WORDS
        ),
        0,
        "`手册` / `手笔` / `助手` 不是那个比喻——误伤一次，人就学会绕开守卫"
    );
}

/// 一段源码里**非注释行**命中封闭比喻词表的行数（一行算一次）。
fn metaphor_hits(source: &str, words: &[&str]) -> usize {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .filter(|line| words.iter().any(|word| line.contains(word)))
        .count()
}

/// **门牌是一句机械的话**（2026-10-08 主人确认，C10 的另一半）。
///
/// 门牌 = `AppPort::description()` 返回的那一句，也就是首屏里 `【<标识>】<这一句>`、
/// 以及展开回执里各印一次的那一行。它决定主体**想不想得到**去用它，所以它自己要有判据
/// （判据写在 `app_contract::AppPort::description` 的文档里，与
/// `DOOR_PLATE_MAX_BYTES` 同处）：**单行字面量 / 不带 `**` / 不带比喻词 / ≤ 上限**。
///
/// **动态门牌（返回 `&self.…`）必须登记在 `DYNAMIC_DOOR_PLATES` 里并写明理由**——
/// 这样"看不见源的门牌"不会悄悄变多。今天只有一条：`mcp/app.rs`（那台 server 的名字
/// 装配期才知道，门牌由装配期拼出来，因此源文本不落在 `description()` 里；它的措辞由
/// `her_visible_words_carry_no_new_metaphor` 那张表面名单兜着）。
///
/// 反例自检在末尾：**每一条判据都有自己的反例**（续行 / `**` / 比喻 / 超长 / 门牌不见了）。
#[test]
fn the_door_plates_are_one_mechanical_sentence() {
    const APPS: [&str; 9] = [
        "notes_app.rs",
        "file_view.rs",
        "im_app.rs",
        "im_social_app.rs",
        "im_profile_app.rs",
        "timer_app.rs",
        "web_app.rs",
        "terminal_app.rs",
        "mcp/app.rs",
    ];
    /// 门牌不由 `description()` 里的字面量给出来的那几个（每条都要写理由）。
    const DYNAMIC_DOOR_PLATES: [&str; 1] = ["mcp/app.rs"];

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for name in APPS {
        let source = std::fs::read_to_string(root.join(name))
            .unwrap_or_else(|_| panic!("{name} 不在 src/ 下——App 名单必须指着真文件"));
        let body = door_plate_body(&source)
            .unwrap_or_else(|| panic!("{name} 里找不到 fn description(&self) -> &str 的函数体"));
        if body.contains("&self.") {
            assert!(
                DYNAMIC_DOOR_PLATES.contains(&name),
                "{name} 的门牌不是字面量（返回 `&self.…`）：要么改成一句话的字面量，\
                 要么登记进 DYNAMIC_DOOR_PLATES 并写清理由——门牌是主体用来判断\
                 「这扇门值不值得开」的那一行，它不该有看不见的源"
            );
            continue;
        }
        assert!(
            door_plate_trouble(body).is_none(),
            "{name} 的门牌不合格：{}",
            door_plate_trouble(body).unwrap_or_default()
        );
    }

    // 反例自检：五条判据各有一条反例，外加一条正例。
    assert!(door_plate_trouble("\"一句话，讲清它提供什么\"").is_none());
    assert!(
        door_plate_trouble("\"一半\\\n 另一半\"").is_some(),
        "续行要被抓"
    );
    assert!(
        door_plate_trouble("\"带 **星号** 的门牌\"").is_some(),
        "`**` 要被抓"
    );
    assert!(
        door_plate_trouble("\"把笔记贴到墙上\"").is_some(),
        "比喻要被抓"
    );
    assert!(
        door_plate_trouble(&format!("\"{}\"", "很长".repeat(DOOR_PLATE_MAX_BYTES))).is_some(),
        "超长要被抓"
    );
    assert!(
        door_plate_trouble("不是字面量").is_some(),
        "没有字面量要被抓"
    );
}

/// 一条门牌正文的第一处违规（`None` = 合格）。抽出来是为了**能自检**：正反例都过它。
fn door_plate_trouble(body: &str) -> Option<String> {
    if body.contains("\\\n") {
        return Some("门牌不许用 `\\` 续行——它是一行字".to_string());
    }
    let (Some(open), Some(close)) = (body.find('"'), body.rfind('"')) else {
        return Some("门牌必须是一个字符串字面量".to_string());
    };
    if close <= open {
        return Some("门牌必须是一个字符串字面量".to_string());
    }
    let literal = &body[open + 1..close];
    if literal.contains("**") {
        return Some("门牌里不许有 `**`——主体读到的是两个星号".to_string());
    }
    if let Some(word) = METAPHOR_WORDS.iter().find(|word| literal.contains(*word)) {
        return Some(format!("门牌里不许有比喻词「{word}」"));
    }
    if literal.len() > DOOR_PLATE_MAX_BYTES {
        return Some(format!(
            "门牌 {} 字节，上限 {DOOR_PLATE_MAX_BYTES} 字节（越写越长要先删，不是先加）",
            literal.len()
        ));
    }
    None
}

/// 从一份源码里取出 `fn description(&self) -> &str` 的**函数体正文**。
fn door_plate_body(source: &str) -> Option<&str> {
    const MARK: &str = "fn description(&self) -> &str";
    let at = source.find(MARK)? + MARK.len();
    let rest = &source[at..];
    let open = rest.find('{')? + 1;
    let rest = &rest[open..];
    let close = rest.find("\n    }")?;
    Some(rest[..close].trim())
}

/// 扫描器自检：给一个明确含违禁词的样本，扫描器必须报出违规。
///
/// 这一条与上面的守卫互为对照——词表被清空、文件枚举失效或扫描逻辑被改坏时，
/// 上面的守卫会永久假绿，而这一条会失败。
#[test]
fn the_boundary_scanners_report_violations_when_they_exist() {
    let sample = "let provider = tool_calls; let messages = serde_json; let auth_token = 1;";

    let substrings = forbidden_substring_hits(sample, &FORBIDDEN_SUBSTRINGS);
    assert!(substrings.contains(&"provider"), "{substrings:?}");
    assert!(substrings.contains(&"messages"), "{substrings:?}");
    assert!(substrings.contains(&"tool_calls"), "{substrings:?}");

    let words = forbidden_word_hits(sample, &FORBIDDEN_WORDS);
    assert!(words.contains(&"json".to_string()), "{words:?}");

    // 短语放行**只放行那一条短语**：同一个词别的出现形态照旧违规，别的文件里连那条短语也不放行。
    let partially = strip_allowed_phrases(
        "let x = \"http-status-failure\"; let y = http_probe();",
        "core.rs",
        &CORE_ALLOWED_PHRASES,
    );
    assert!(
        forbidden_word_hits(&partially, &FORBIDDEN_WORDS).contains(&"http".to_string()),
        "短语之外的出现形态必须仍然违规"
    );
    let elsewhere = strip_allowed_phrases("http-status-failure", "store.rs", &CORE_ALLOWED_PHRASES);
    assert!(
        forbidden_word_hits(&elsewhere, &FORBIDDEN_WORDS).contains(&"http".to_string()),
        "别的文件不享受这条放行"
    );

    // 门铃那两条冻结字面量同样只放行**那一条短语**：短词"stream"在别处、别的形态照旧违规。
    let doorbell_ok = strip_allowed_phrases(
        "const RING_PATH: &str = \"/v1/backend/events/stream\";",
        "doorbell.rs",
        &CORE_ALLOWED_PHRASES,
    );
    assert!(
        forbidden_word_hits(&doorbell_ok, &FORBIDDEN_WORDS).is_empty(),
        "带引号的那条短语应当被放行: {doorbell_ok}"
    );
    let doorbell_bare =
        strip_allowed_phrases("let ring_stream = 1;", "doorbell.rs", &CORE_ALLOWED_PHRASES);
    assert!(
        forbidden_word_hits(&doorbell_bare, &FORBIDDEN_WORDS).contains(&"stream".to_string()),
        "同一个短词不带引号/不成那条短语必须照旧违规"
    );
    let doorbell_elsewhere = strip_allowed_phrases(
        "const P: &str = \"/v1/backend/events/stream\";",
        "store.rs",
        &CORE_ALLOWED_PHRASES,
    );
    assert!(
        forbidden_word_hits(&doorbell_elsewhere, &FORBIDDEN_WORDS).contains(&"stream".to_string()),
        "别的文件不享受这条放行"
    );
    assert!(words.contains(&"auth".to_string()), "{words:?}");
    assert!(words.contains(&"token".to_string()), "{words:?}");

    // 反例：短词必须只按整词命中，不得误伤正常词。
    let clean = "author, authority, tokenizer, streamline, curly, httpish, jsonnet";
    assert!(forbidden_substring_hits(clean, &FORBIDDEN_SUBSTRINGS).is_empty());
    let clean_words = forbidden_word_hits(clean, &FORBIDDEN_WORDS);
    assert!(
        clean_words.is_empty(),
        "词边界匹配不得误伤正常词，却报了: {clean_words:?}"
    );

    // 边界模块比对必须既抓得住真引用，又不误伤含同名子串的正常词。
    assert!(
        !forbidden_word_hits("use crate::os::MinimalOs;", &["os"]).is_empty(),
        "`os` 作为独立标识符出现时必须被认出"
    );
    assert!(
        !forbidden_word_hits("let os = open();", &["os"]).is_empty(),
        "`os` 作为独立标识符出现时必须被认出"
    );
    assert!(
        forbidden_word_hits("let c = composition; let s = store_error;", &["os"]).is_empty(),
        "`os` 不得在 `composition` / `store_error` 这类正常词里误报"
    );
    // `forbidden_word_hits` 按非字母数字切词，`_` 也算分隔符：所以 `history_app_id`
    // 会被切成 `history` / `app` / `id`。这是**期望**行为——校验只作用于 Core 与
    // Foundation 两个文件，它们连这类命名都不该带边界模块的名字。
    assert!(
        !forbidden_word_hits("let h = history_app_id;", &["history"]).is_empty(),
        "首段是边界模块名的标识符必须被认出（守卫只扫 Core/Foundation，不会误伤边界层自身）"
    );

    // 依赖守卫自检：它必须认出各种依赖表写法，而不只是 `[dependencies]`。
    for header in [
        "[dependencies]",
        "[dependencies] # 注释",
        "[ dependencies ]",
        "[dev-dependencies]",
        "[build-dependencies]",
        "[target.'cfg(unix)'.dependencies]",
        "[dependencies.serde]",
    ] {
        assert!(is_dependency_table(header), "{header} 应该被认成依赖表");
    }
    for header in ["[package]", "[workspace]", "[profile.release]", "[lib]"] {
        assert!(!is_dependency_table(header), "{header} 不该被认成依赖表");
    }

    // 反向端到端证明：拿**真实源码文本**做底，拼上一个违禁词后扫描器必须报出来。
    // 这样不修改 src/** 也能证明"源码一旦越界，上面的守卫会红"。
    let (name, source) = core_sources().into_iter().next().expect("至少有一个源文件");
    let tampered = format!("{source}\n// provider\n");
    let hits = forbidden_substring_hits(&tampered, &FORBIDDEN_SUBSTRINGS);
    assert!(
        hits.contains(&"provider"),
        "把违禁词拼进 {name} 的真实源码文本后，扫描器必须报出违规，实际: {hits:?}"
    );
}

/// 表头是否是"依赖表"：去掉行内注释、方括号与空白后，把表名按 `.` 切段，
/// 任一段是 `dependencies` / `dev-dependencies` / `build-dependencies` 即算。
///
/// 覆盖 `[dependencies]`、`[dependencies] # 注释`、`[ dependencies ]`、
/// `[dev-dependencies]`、`[build-dependencies]`、
/// `[target.'cfg(unix)'.dependencies]` 与 `[dependencies.serde]`。
///
/// 这是**文本检测，不是 TOML 解析**：它不认识多行字符串或行内表，只保证本 manifest 的
/// 依赖表不会被漏判。
fn is_dependency_table(header: &str) -> bool {
    let inside = header
        .split('#')
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim();

    inside.split('.').any(|segment| {
        let segment = segment.trim();
        segment == "dependencies"
            || segment == "dev-dependencies"
            || segment == "build-dependencies"
    })
}

/// 从 manifest 里收集直接依赖名。
///
/// 两种形态都收：表内的 `name = ...`（以及 `name` 简写），与表头里的内联形态
/// `[dependencies.name]` / `[target.'cfg(..)'.dependencies.name]`。
fn declared_dependencies(manifest: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut inside_dependency_table = false;

    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            inside_dependency_table = is_dependency_table(trimmed);
            if inside_dependency_table {
                let header = trimmed
                    .split('#')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .trim();
                if let Some(segment) = header.split('.').next_back() {
                    let segment = segment.trim().trim_matches('"');
                    if segment != "dependencies"
                        && segment != "dev-dependencies"
                        && segment != "build-dependencies"
                    {
                        names.push(segment.to_string());
                    }
                }
            }
            continue;
        }

        if !inside_dependency_table || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        match trimmed.split_once('=') {
            Some((name, _)) => names.push(name.trim().to_string()),
            None => names.push(trimmed.to_string()),
        }
    }

    names.sort();
    names.dedup();
    names
}

/// 从 Cargo.lock 取本 crate 的**直接**依赖名：锁文件里每个 package 的
/// `dependencies = [...]` 只列直接依赖，因此它能验证清单与锁文件是否一致
/// （不依赖任何具体版本号）。
///
/// 每一项可能是裸包名（`"serde"`），也可能带版本后缀（`"serde 1.0.229"`）——
/// 后者在同一 crate 出现多版本时才会写出来。两者都必须被读成**包名**。
fn locked_direct_dependencies(lock: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut in_own_package = false;
    let mut in_list = false;

    for line in lock.lines() {
        let trimmed = line.trim();
        if trimmed == "[[package]]" {
            in_own_package = false;
            in_list = false;
            continue;
        }
        if trimmed == "name = \"celira-agent\"" {
            in_own_package = true;
            continue;
        }
        if !in_own_package {
            continue;
        }
        if in_list {
            if trimmed == "]" {
                in_list = false;
                continue;
            }
            let entry = trimmed.trim_matches(|ch: char| ch == '"' || ch == ',');
            if let Some(name) = entry.split_whitespace().next() {
                names.push(name.to_string());
            }
            continue;
        }
        if trimmed.starts_with("dependencies = [") {
            in_list = true;
        }
    }

    names.sort();
    names.dedup();
    names
}

/// 依赖守卫（阶段二起改写）：依赖不再为零，但必须是**封闭且清单/锁文件一致**的一组，
/// 而且只服务于 Adapter。
///
/// 覆盖范围：`[dependencies]`、`[dev-dependencies]`、`[build-dependencies]`、
/// `[target.'cfg(..)'.dependencies]`、带空格或行内注释的表头、`[dependencies.name]`
/// 内联形态，以及 Cargo.lock 里本 crate 的直接依赖列表。
///
/// 仍然是**文本检测，不是 TOML 解析**；它只保证本 crate 的依赖变化一定会被看见。
#[test]
fn the_skeleton_declares_only_the_adapter_dependencies() {
    let expected = ADAPTER_DEPENDENCIES.map(String::from).to_vec();

    let declared = declared_dependencies(include_str!("../Cargo.toml"));
    assert_eq!(
        declared, expected,
        "依赖集合变了：新增或删除依赖必须先改这个守卫"
    );

    let locked = locked_direct_dependencies(include_str!("../Cargo.lock"));
    assert_eq!(
        locked, expected,
        "Cargo.lock 里本 crate 的直接依赖与清单不一致"
    );
}

/// 锁文件解析（F5）：裸名与 `name version` 两种形态都要读成包名；未知直接依赖仍被拒。
#[test]
fn the_lockfile_guard_reads_package_names_with_optional_versions() {
    let expected = ADAPTER_DEPENDENCIES.map(String::from).to_vec();

    let bare = "[[package]]\nname = \"celira-agent\"\nversion = \"0.1.0\"\ndependencies = [\n \"reqwest\",\n \"serde\",\n \"serde_json\",\n]\n";
    assert_eq!(locked_direct_dependencies(bare), expected);

    // 同一 crate 有多版本时，锁文件会写成 `name version`。
    let versioned = "[[package]]\nname = \"celira-agent\"\nversion = \"0.1.0\"\ndependencies = [\n \"reqwest\",\n \"serde 1.0.229\",\n \"serde_json 1.0.151\",\n]\n";
    assert_eq!(
        locked_direct_dependencies(versioned),
        expected,
        "带版本后缀的名字必须被读成包名"
    );

    // 未知直接依赖：必须造成与封闭集合的不一致。
    let unknown = "[[package]]\nname = \"celira-agent\"\nversion = \"0.1.0\"\ndependencies = [\n \"reqwest\",\n \"serde\",\n \"serde_json\",\n \"some-new-crate 2.0.0\",\n]\n";
    let parsed = locked_direct_dependencies(unknown);
    assert_ne!(parsed, expected, "未知直接依赖必须被拦住");
    assert!(
        parsed.iter().any(|name| name == "some-new-crate"),
        "未知包名要原样读出来，便于人看清是什么: {parsed:?}"
    );
}

/// Adapter 允许的公开面（**封闭名单**）。任何其它 `pub` 项都必须让守卫失败。
///
/// 这是**文本级**守卫，不是 Rust 编译器级别的可见性证明：它只保证"公开面变了会被看见"。
///
/// `pub fn at_endpoint` 是 2026-09-14 显式扩入的一项：它让**离线端到端验收**可以指向本机
/// 回环端点、观察真实请求，因此不需要凭据、也不需要联网。它同样只接受调用方显式给出的凭据，
/// 不读环境变量，也不改变 wire 类型必须留在内部的约束。
///
/// `pub fn nominal_context_window` 与 `pub fn with_reading` 是 2026-09-16（TASK_19）显式扩入的
/// 两项：前者是**宿主声明窗口时的缺省值**（一个数字，附出处；声明权在宿主手里，可用
/// `--context-window` 覆盖），后者是**装配注入**——把屏幕报告 cell 的写柄交给本层，让"只有
/// 适配层写"这件事成为形状。两者都不放宽任何既有约束：wire 类型仍在内部、凭据仍由调用方给。
///
/// ⚠️ 刀三 B 起，适配层的**用量读数**有了**第二个去向**：除了屏幕那一行，它还经
/// [`celira_agent::cost::UsageWriter`] 记进代价 cell、收场时随 `RunEnded` 落盘。这**没有**
/// 放宽任何约束——写柄仍然是「装配注入、唯一写者」那两件，本层仍然只是搬运厂商原文：
/// 它不进 Core 的判断、不换算、不参与任何决定（那些纪律由本列表之外的守卫继续盯）。
const ADAPTER_PUBLIC_SURFACE: [&str; 22] = [
    "pub struct DeepSeekModel",
    "pub fn from_env_var",
    "pub fn from_env",
    "pub const API_KEY_ENV",
    // TASK_38：默认档也要走同一条显式装配路径，因此这两个既有的值常量升为公开
    // （**值逐字节未改**，只是可见性）。它们是"本层带出处的默认值"，入口不另写一份。
    "pub const DEFAULT_MODEL",
    "pub const ENDPOINT",
    // TASK_42：工具名的长度上限同样是**本层的事实**，而外部来源的 App（MCP）造动词名时要读它。
    // 值一个字节没改，升的只是可见性——谁都不许另抄一份（同一事实两份权威迟早分叉）。
    "pub const TOOL_NAME_MAX",
    // TASK_44：**协议名**同样是本层的事实，而换家档的 `--model-protocol` 要拿它当权威名字。
    // 两条 wire 各有一个常量，这一条名字覆盖两处（两个协议名因此各只有一处定义）。
    "pub const PROTOCOL",
    // TASK_44：第二条 wire 的模型与"入口的选择"。后者是**枚举**而不是 trait 对象：
    // Core 与会话函数收的是 `M: ModelPort`（大小已知），枚举让 Core 一行不动。
    "pub struct ResponsesModel",
    "pub enum ModelChoice",
    "pub fn model",
    "pub fn at_endpoint",
    "pub fn nominal_context_window",
    "pub fn with_reading",
    // 刀三 B：用量 cell 的写柄（与 `with_reading` 同款、同一条纪律：装配注入、唯一写者）。
    "pub fn with_usage",
    // 2026-10-08：**思考强度**那一格由来源清单给（唯一读侧已按封闭表校验），适配层只搬运它到
    // 请求体（`reasoning_effort`，`None` 时那个键根本不出现）⇒ 与 `with_reading` / `with_usage`
    // 同一条纪律：装配注入、本层不解释、不做判定。
    "pub fn with_reasoning_effort",
    // 2026-10-08（第三腿）：**读回那一格**。消费者是装配那一层——它要从"将要发请求的那个对象"
    // 里取"这一段想多用力"，好让主体读到的那一行与真正发出去的请求同源（`ModelChoice::model`
    // 的同一款：一处权威，入口不另算一份）。它**只读自己持有的那一格**，不做任何判定。
    "pub fn reasoning_effort",
    // TASK_60 §6：**形态指纹的去向**必须跨出适配层——它由**装配那一层**（宿主的入口）创建、
    // 递进模型、跑完再取走打印（库不许打印：那是 TASK_51 删过的侧信道形状）。
    // 因此在适配层公开的只有"这个格子"与"把柄递进去 / 取出来"这三件，**不含任何判定逻辑**。
    "pub struct ShapeTrace",
    "pub fn new",
    "pub fn with_shape_trace",
    "pub fn drain",
    // **逐轮读数账**那一刀：与前三个传感器**同一条纪律**（装配注入、唯一写者、本层只搬运），
    // 只是落点不同——前两个写进进程内的 cell，这一个**落盘**（一次响应一行，追加）。
    //
    // 它没有放宽任何既有约束：写柄仍由装配给、wire 类型仍在内部、本层仍不解释读数。
    // 新增的**唯一**一件事是"多了一个落点"，而那个落点属于**操作者**（事后翻文件），
    // 不属于任何执行路径（`tests/model_readings.rs` 有一条源码级负空间钉着这一点）。
    "pub fn with_model_readings",
];

/// **crate 内共用件**（`pub(crate)`）：第二条线协议借用的、与协议无关的机械件。
///
/// 与 [`ADAPTER_PUBLIC_SURFACE`] 分开列，因为两者是不同的事：前者是"crate 外面能看见的"，
/// 这里是"crate 内可见的"。但**可见性扩张就是可见性扩张**——每一条都要点名，
/// 否则"顺手把一个内部函数提成 pub(crate)"会没人看见。
///
/// 这一批的边界很窄：**一个文件里的机械件**（契约文本、收场变体、渲染器、失败分类、工具名规则），
/// 两条 wire 因此不会各说一套；它们都不进外部面，也不放宽任何既有约束。
const ADAPTER_CRATE_SHARED: [&str; 39] = [
    "pub(crate) const ASSEMBLY_FAILURE",
    "pub(crate) const TIMEOUT",
    // 2026-10-06（接 opencode go 那一刀）：**客户端自我介绍**（HTTP `User-Agent`）。
    // 两条 wire 必须发同一个名字（各写一份迟早漂），而 crate 外面没有人需要它 ⇒
    // `pub(crate)` 共用件，不是公开面。集成测试断言的是**产品名**（不绑版本号）。
    "pub(crate) const CLIENT_USER_AGENT",
    // 2026-10-09（接 opencode go 的会话头那一格）：**两条 wire 必须发同一个头名与同一个取值**
    // （chat 侧定义、responses 侧借用）；crate 外面没有人需要它 ⇒ 共用件，不是公开面。
    "pub(crate) const SESSION_HEADER",
    "pub(crate) const FINISH_TOOL",
    "pub(crate) const FINISH_DESCRIPTION",
    // 2026-10-10（裁决三：在席第三选择）：**搁下**那一件与它的说明书——同样是"两条 wire 同源同形"，
    // 因此和 finish 那一对并排登记。
    "pub(crate) const HOLD_TOOL",
    "pub(crate) const HOLD_DESCRIPTION",
    // TASK_68：头部两件套的入口与它的说明书——两条 wire 同源同形，这三项必须跨适配层文件共享
    // （chat 侧定义、responses 侧借用）。登记在这里是**声明**，不是放宽：谁多暴露一项都要先改这份名单。
    "pub(crate) const INVOKE_TOOL",
    "pub(crate) const INVOKE_DESCRIPTION",
    "pub(crate) fn invoke_schema",
    "pub(crate) enum ProviderFailure",
    "pub(crate) fn class",
    "pub(crate) fn decision_contract",
    "pub(crate) fn finish_schema",
    "pub(crate) fn tool_name",
    "pub(crate) fn arguments_of",
    "pub(crate) fn decode_arguments",
    // 2026-10-11（一次调用的形状错误不报废这一段）：**"搬不动的参数"这一格与它的读数**。
    // 两条 wire 必须给出**同一份读数**（同一个故障两种说法，就是那条缝），而 crate 外面没有人
    // 需要它 ⇒ `pub(crate)` 共用件，不是公开面。登记在这里是**声明**，不是放宽。
    "pub(crate) enum ArgumentShape",
    "pub(crate) fn shape_reading",
    "pub(crate) fn finish_ending",
    // 2026-10-10：两个结束类动词共用**同一个参数校验**（`ending_of`），收场与搁下各一个薄入口。
    "pub(crate) fn ending_of",
    "pub(crate) fn hold_ending",
    "pub(crate) fn input_body",
    "pub(crate) fn mechanical_facts",
    // TASK_79：**两段式系统消息**（静态头 ＋ 机械事实）的渲染也只有一处权威——两条 wire 共用它，
    // 否则"摆对了两段式"会变成两条协议各自的自觉。
    "pub(crate) fn system_message",
    "pub(crate) fn injection_body",
    "pub(crate) fn refusal_result",
    "pub(crate) fn unconfirmed_result",
    "pub(crate) fn observation_result",
    // TASK_60：两条 wire 共用同一份"外部形状 → 档内细分"的折叠规则。
    // 共用的理由就是判据本身——**同一个故障在两处必须得到同一档**；各写一套迟早漂移。
    "pub(crate) fn transport_detail",
    "pub(crate) fn http_detail",
    // TASK_60 §6：两条 wire 共用一份"传输失败 = 折档 ＋（认不出时）记一行指纹"，
    // 以及那一行的载荷构造。可见性扩张到此为止：判定逻辑不出去。
    "pub(crate) fn transport_failure",
    "pub(crate) fn failure_shape_fingerprint",
    "pub(crate) fn push",
    // 2026-10-13：两条 wire 共用同一份 invoke 解码与同一句「名字不能用」的读数
    // （Responses wire 从前没有 invoke 分支，模型一调它就整段死——见 provider_wire 的回归用例）。
    "pub(crate) enum InvokeFailure",
    "pub(crate) fn decode_invoke",
    // 2026-10-17（主体直接感知媒体材料）：**材料出门的那两件**——两条 wire 共用同一份
    // "一份材料 → data URL"与同一份"这次观察里随行出门的是哪些材料"。共用的理由就是判据本身：
    // 同一个字节在两处必须折成同一个形状，各写一套迟早漂移。它们**不进外部面**，
    // 也不放宽任何既有约束；可见性扩张到此为止（怎么折进请求体是各 wire 自己的事）。
    "pub(crate) fn data_url",
    "pub(crate) fn observation_materials",
];

/// 供应商 wire 类型：一个都不许公开。
const ADAPTER_WIRE_TYPES: [&str; 5] = [
    "WireRequest",
    "WireMessage",
    "WireResponse",
    "WireChoice",
    "WireOutgoing",
];

/// 源码里所有**不在允许公开面内**的 `pub` 项（含 `pub(...)` 形式）。
///
/// 两份名单并集放行：**公开面**与 **crate 内共用件**——它们的可见性不同，但都必须是被人点过名的。
fn unauthorized_public_items(source: &str) -> Vec<String> {
    source
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("pub ") || line.starts_with("pub("))
        .filter(|line| {
            !ADAPTER_PUBLIC_SURFACE
                .iter()
                .chain(ADAPTER_CRATE_SHARED.iter())
                .any(|allowed| line.starts_with(allowed))
        })
        .map(str::to_string)
        .collect()
}

/// 适配层的全部源文件（顺序按名单）。
fn adapter_sources() -> Vec<(String, String)> {
    core_sources()
        .into_iter()
        .filter(|(name, _)| ADAPTER_SOURCES.contains(&name.as_str()))
        .collect()
}

/// 公开面守卫（F4）：wire 类型不许公开，公开面只允许名单里那些项。
///
/// TASK_44 起适配层有**三个**文件，因此这条守卫对**每一个**都跑一遍——只扫其中一个，
/// 就等于"多出来的那条 wire 随便暴露"。
#[test]
fn the_adapter_exposes_only_its_declared_surface() {
    let adapters = adapter_sources();
    assert_eq!(
        adapters.len(),
        ADAPTER_SOURCES.len(),
        "名单里的适配层文件必须都在 src/ 下"
    );

    let mut combined = String::new();
    for (name, source) in &adapters {
        let unauthorized = unauthorized_public_items(source);
        assert!(
            unauthorized.is_empty(),
            "{name} 出现了未声明的公开项: {unauthorized:?}"
        );

        for wire in ADAPTER_WIRE_TYPES {
            for form in [
                "pub struct ",
                "pub enum ",
                "pub(crate) struct ",
                "pub(crate) enum ",
            ] {
                assert!(
                    !source.contains(&format!("{form}{wire}")),
                    "{name}: {wire} 被公开了（{form}）：wire 类型必须留在 Adapter 内部"
                );
            }
        }
        combined.push_str(source);
    }

    for allowed in ADAPTER_PUBLIC_SURFACE {
        assert!(
            combined.contains(allowed),
            "公开面少了 {allowed}（名单过期，或它被删掉了）"
        );
    }
    for shared in ADAPTER_CRATE_SHARED {
        assert!(
            combined.contains(shared),
            "crate 内共用件少了 {shared}（名单过期，或它被删掉了）"
        );
    }
}

/// 守卫自检：给一份含假 `pub struct` 的样本，它必须报出违规。
#[test]
fn the_public_surface_guard_reports_a_fake_public_type() {
    let fake = "pub struct WireFake {\n    id: String,\n}\n\npub fn model(&self) -> &str {\n    \"x\"\n}\n";
    assert_eq!(
        unauthorized_public_items(fake),
        vec!["pub struct WireFake {".to_string()],
        "守卫必须能报出假的公开类型，且不误伤允许名单里的项"
    );
}

/// **会话声明包不许逐格写全**（§3.49 那笔债的守卫，2026-10-08）。
///
/// 判据一句话：**每一个 `SessionDeclarations { … }` 字面量都必须以 `..Default::default()` 收尾。**
///
/// 为什么要有它：那笔债的形状就是"每个构造点把每一格逐个写全"（加一格 ⇒ 二十五处机械改动）。
/// 包本身只解决了一半——**只要还有一个构造点把九格写全，下一格就又要改它**。这条守卫把
/// "下一格的路费"钉在形状上，而不是钉在记性上（本刀实测：三个测试构造点就是那么写全的）。
#[test]
fn the_session_declaration_pack_is_never_spelled_out_field_by_field() {
    // 自证扫描器真的在扫：反例（没有 `..Default::default()`）必须被认出来，正例不许误报。
    // ⚠ 样本分两段拼出来：本文件也在扫描射程里，整行写出那个记号会被自己抓住。
    let spelled_out = concat!("let x = SessionDeclarations", " {\n    model: None,\n};\n");
    let with_default = concat!(
        "let x = SessionDeclarations",
        " {\n    model: None,\n    ..Default::default()\n};\n"
    );
    assert_eq!(
        packs_without_default(spelled_out),
        vec![1],
        "反例必须被认出来"
    );
    assert!(
        packs_without_default(with_default).is_empty(),
        "带 `..Default::default()` 的正例不许误报"
    );
    // 定义本身不是构造点（它没有"要不要写 `..Default::default()`"这回事）。
    let definition = concat!(
        "pub struct SessionDeclarations",
        " {\n    pub model: Option<String>,\n}\n"
    );
    assert!(
        packs_without_default(definition).is_empty(),
        "类型定义不许被当成构造点"
    );

    let mut offenders: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    for root in [
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"),
    ] {
        let mut paths: Vec<PathBuf> = Vec::new();
        collect_rust_sources(&root, &mut paths);
        paths.sort();
        for path in paths {
            let source = fs::read_to_string(&path).expect("读源码");
            if !source.contains("SessionDeclarations") {
                continue;
            }
            scanned += 1;
            for line in packs_without_default(&source) {
                offenders.push(format!("{}:{line}", path.display()));
            }
        }
    }
    assert!(
        scanned >= 10,
        "扫到的文件太少（{scanned}）——守卫可能读错了地方"
    );
    assert!(
        offenders.is_empty(),
        "这些声明包逐格写全了（下一格就要改它们，而那正是 §3.49 还掉的债）: {offenders:#?}"
    );
}

/// **测试的临时目录必须按进程唯一**（2026-10-09）。
///
/// 判据一句话：`tests/*.rs` 里每一处 `std::env::temp_dir()` 所在的那个函数，必须提到
/// `std::process::id()`（或者写一条豁免注释，见下）。
///
/// 为什么要有它：测试二进制是**一份一个进程**，而目录名里的计数器只在**进程内**唯一 ⇒
/// 不带进程号的两份并行（或两个写者同时跑）会算出同一批路径、**互相删目录**。
/// 2026-10-09 实测：6 份并行跑 `provider_wire` ⇒ **每份红 6–13 条**，报的是
/// `清掉同名残留目录: DirectoryNotEmpty` 与 `建环境: StorageMissing`；同一批代码单跑全绿。
/// 那一族当时有 **10 个文件**是这么写的（另外 30 个带了进程号）。
///
/// 豁免：确实**故意共享**的常量路径（例如"这个路径一定不存在"那种），在那一行写
/// `// temp-dir: 共享是故意的` 即可——豁免是**具名**的，不是整片文件。
#[test]
fn every_test_temp_dir_is_unique_per_process() {
    // 自证扫描器真的在扫：反例必须被认出来，正例与具名豁免不许误报。
    // ⚠ 样本分两段拼出来：本文件也在扫描射程里，整行写出那个记号会被自己抓住。
    let bad = concat!("fn f() {\n    let p = std::env::temp_dir", "();\n}\n");
    let good = concat!(
        "fn f() {\n    let p = std::env::temp_dir",
        "().join(format!(\"x-{}\", std::process::id()));\n}\n"
    );
    let exempt = concat!(
        "fn f() {\n    // temp-dir: 共享是故意的\n    let p = std::env::temp_dir",
        "().join(\"x\");\n}\n"
    );
    assert_eq!(
        temp_dirs_without_a_process_id(bad),
        vec![1],
        "反例必须被认出来"
    );
    assert!(
        temp_dirs_without_a_process_id(good).is_empty(),
        "带进程号的正例不许误报"
    );
    assert!(
        temp_dirs_without_a_process_id(exempt).is_empty(),
        "具名豁免不许误报"
    );

    let mut offenders: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    for root in [
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests"),
    ] {
        let mut paths: Vec<PathBuf> = Vec::new();
        collect_rust_sources(&root, &mut paths);
        paths.sort();
        for path in paths {
            let source = fs::read_to_string(&path).expect("读源码");
            scanned += 1;
            for line in temp_dirs_without_a_process_id(&source) {
                offenders.push(format!("{}:{line}", path.display()));
            }
        }
    }
    assert!(
        scanned >= 50,
        "扫到的文件太少（{scanned}）——守卫可能读错了地方"
    );
    assert!(
        offenders.is_empty(),
        "这些临时目录不带进程号（两份并行会互相删目录）: {offenders:#?}"
    );
}

/// 找出**不带进程号**的临时目录构造：返回那一行所在**函数**的起始行号（1 起）。
///
/// 判据落在"那个函数里有没有 `std::process::id()`"，而不是"那一行长什么样"——因为路径
/// 既可能当场拼、也可能由同一个函数里的 `unique` 之类先算好（后者是对的写法）。
/// `// temp-dir: 共享是故意的` 是**具名豁免**（写在哪个函数里就豁免哪个函数）。
///
/// 函数边界按**缩进**找：函数头是缩进 ≤ 4 的 `fn `（本仓的临时目录助手不是顶层 `fn`，就是
/// `impl` 里的一层），闭包与调用都不长这样。
fn temp_dirs_without_a_process_id(source: &str) -> Vec<usize> {
    let lines: Vec<&str> = source.lines().collect();
    let is_head = |line: &str| {
        let indent = line.len() - line.trim_start().len();
        indent <= 4 && line.trim_start().starts_with("fn ") && line.contains('(')
    };
    let mut found = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.contains("temp_dir()") {
            continue;
        }
        let Some(head) = (0..=index).rev().find(|back| is_head(lines[*back])) else {
            continue;
        };
        let end = (head + 1..lines.len())
            .find(|ahead| is_head(lines[*ahead]))
            .unwrap_or(lines.len());
        let body = lines[head..end].join("\n");
        if body.contains("std::process::id()") || body.contains("temp-dir: 共享是故意的") {
            continue;
        }
        if found.last() != Some(&(head + 1)) {
            found.push(head + 1);
        }
    }
    found
}

/// 找出所有**没有**以 `..Default::default()` 收尾的声明包字面量，返回起始行号（1 起）。
///
/// 块按花括号配对切；**类型定义**（`pub struct SessionDeclarations {`）与
/// `SessionDeclarations::default()`（没有字段列表）都不是构造点，因此不会被算进来。
fn packs_without_default(source: &str) -> Vec<usize> {
    let lines: Vec<&str> = source.lines().collect();
    let mut found = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        if !lines[index].contains("SessionDeclarations {") || lines[index].contains("struct ") {
            index += 1;
            continue;
        }
        let start = index;
        let mut depth = 0i32;
        let mut has_default = false;
        while index < lines.len() {
            depth += lines[index].matches('{').count() as i32;
            depth -= lines[index].matches('}').count() as i32;
            if lines[index].contains("..Default::default()") {
                has_default = true;
            }
            index += 1;
            if depth <= 0 {
                break;
            }
        }
        if !has_default {
            found.push(start + 1);
        }
    }
    found
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

// ---------------------------------------------------------------------------
// TASK_70：自识（self_concept）的**写路径**——只许自己的动词写
// ---------------------------------------------------------------------------

/// **写自识的封闭名单**：`self_concept.rs` 里那一次写是 crate 里唯一能改自识的地方，而它**只许由主体的
/// 动词**（`write_self_concept`，OS 自有动作，见 `os.rs` 的 `OS_OWNED_ACTIONS`）经
/// `self_concept::write_from_subject` 走到。
///
/// ⚠️ 这条判据的形状**变过两次**，值得记下：裁决之前它是"只许创建流程调用"（名单＝入口那一处）；
/// 主人 2026-09-21 裁决「写权归自己」之后，同一条纪律换成**判主体的动词面**——因为
/// "没有编辑入口"这句话的语义本来就不是"全 crate 只有一次写入"，而是"**只有自己**能写"。
/// **2026-10-08** 收口那一刀把域规则与回执措辞搬回它们的所有者（`self_concept.rs`），于是写路径不再
/// 经过 `os`：**同一条判据，落点换了文件**。而且新形状**更强**——从"某个文件里出现过"
/// 收紧成"**恰好一次，且在那一个函数体里**"。
/// ⇒ **判据跟着语义走，不是跟着上一次的写法走。**
const SELF_CONCEPT_WRITE_CALL: &str = "incept(environment_root, text)";

/// 那唯一一次写必须落在**这个函数体**里（顶格函数）。
const SELF_CONCEPT_WRITE_OWNER: &str = "pub fn write_from_subject";

/// 主体的那个动词在源码里的名字：必须在那份**唯一的动词名单**里被认出来
/// （装配／准入／执行三面都问 `OS_OWNED_ACTIONS`）。
const SELF_CONCEPT_WRITE_VERB: &str = "CAPABILITY_SELF_CONCEPT_WRITE";

/// **路由层只许拥有"能不能调"，不许拥有"事实是什么"**（2026-10-08 主人确认）。
///
/// 起因：`os` 是**路由层**（发现 / 展开 / 派发），而它原来同时住着约 180 行**域规则**——
/// 格位上限、空自识要拒、超限读数、以及三句回执的措辞。那些是**主体自持物**的事实，
/// 它们的所有者是 `pinned_actions` / `self_concept` 两个模块（搬回之后，那两个模块各自是那一份事实的
/// **唯一写者**）。判据一句话：**域分类与域措辞只许在所有者里拼。**
///
/// 判法是**逐个字面量**：下面这些字符串一旦在 `os.rs` 里出现，就是"路由层又在替别人说话"。
/// 它们今天各自只在所有者里出现一次（`os` 用 `as_str()` 去引，不抄字面量）。
///
/// ⚠ **历史包袱**：`MAX_PINS` / `MAX_SELF_CONCEPT_BYTES` 这两个**数字**仍然出现在 `os` 的**动作说明**里
/// ——那是"这扇门怎么用"的门面话，说的是"上限是几"，由所有者导出（`{MAX_PINS}`）、`os` 只是引用。
/// 真正不许抄的是**分类串**与**回执措辞**：抄了就会出现第二份真相。
#[test]
fn the_router_owns_no_domain_words() {
    const DOMAIN_WORDS: [&str; 9] = [
        // 域分类（各自属于一个所有者）
        "pinned-limit-reached",
        "pinned-actions-unreadable",
        "pinned-actions-unwritable",
        "self_concept-too-large",
        "self_concept-unwritable",
        // 域回执的措辞（谁是主语、什么时候生效，都由所有者自己说）
        "你现在自己钉着",
        "出厂格位不因摘除而消失",
        "自识已写下",
        "旧的 self_concept 原样保留",
    ];

    let sources = core_sources();
    let os_source = sources
        .iter()
        .find(|(name, _)| name == "os.rs")
        .map(|(_, text)| text.clone())
        .expect("os.rs 应当在 src/ 下");
    for word in DOMAIN_WORDS {
        assert!(
            !os_source.contains(word),
            "`os` 是路由层，出现了域分类或域回执的措辞「{word}」：\
             它们只许在那一份事实的所有者里拼（`pinned_actions` / `self_concept`），\
             `os` 要用就引它导出的 `as_str()`。"
        );
    }

    // 反例自检：那些字面量**确实**在所有者里（否则这条守卫就是在守空气）。
    for (owner, word) in [
        ("pinned_actions.rs", "pinned-limit-reached"),
        ("pinned_actions.rs", "你现在自己钉着"),
        ("self_concept.rs", "self_concept-too-large"),
        ("self_concept.rs", "自识已写下"),
    ] {
        let source = sources
            .iter()
            .find(|(name, _)| name == owner)
            .map(|(_, text)| text.clone())
            .unwrap_or_else(|| panic!("{owner} 应当在 src/ 下"));
        assert!(
            source.contains(word),
            "{owner} 里必须留着「{word}」——它才是那一份事实的所有者"
        );
    }
}

/// **自识只许自己的动词写**（TASK_70）。
///
/// 判三件事：
/// 1. 那一次写（[`SELF_CONCEPT_WRITE_CALL`]）在**全部核心源码里恰好出现一次**，而且落在
///    [`SELF_CONCEPT_WRITE_OWNER`] 的函数体里（多一次、或落到别的函数里 = 多了一条改自识的路）；
/// 2. 主体的动词 [`SELF_CONCEPT_WRITE_VERB`] **真的在那份唯一的动词名单里**——装配／准入／执行三面都问
///    `OS_OWNED_ACTIONS`，所以"在名单里"就等于三面都认得它；漏一面就是「目录里有、路由里没有」；
/// 3. `self_concept.rs` 的**模块级公开面**只有两个函数：写（`write_from_subject`）与读（`read`）。
///    谁加了 `set_self_concept` / `edit_self_concept` 这类名字、或者把里子 `incept` 又公开出去，这条就红。
#[test]
fn the_self_concept_can_only_be_written_by_her_own_verb() {
    let sources = core_sources();

    // 判据 1：写路径**恰好一次**，且在那一个函数体里。
    let mut total = 0usize;
    let mut owners: Vec<String> = Vec::new();
    for (name, text) in &sources {
        // ⚠ **只看 `self_concept.rs`**（2026-10-19 收紧）：`incept` 是那个模块的**私有**函数，
        // 别的文件里出现同一行调用写的是**它自己**那一份（如 `handoff.rs` 的交接）。
        // 照旧实现会按**裸子串**命中 ⇒ 把一份写得完全正确的模块判成"多了一条改自识的路"。
        if name.as_str() != "self_concept.rs" {
            continue;
        }
        let hits = text.matches(SELF_CONCEPT_WRITE_CALL).count();
        if hits == 0 {
            continue;
        }
        total += hits;
        let at = text.find(SELF_CONCEPT_WRITE_CALL).unwrap_or_default();
        let owner_at = text.find(SELF_CONCEPT_WRITE_OWNER).unwrap_or(usize::MAX);
        // 那一次调用必须排在它的所有者之后，且中间不再出现一个"顶格 `}` 收尾"的函数结尾。
        let between = &text[owner_at..at];
        if owner_at < at && !between.contains("\n}\n") {
            owners.push(name.clone());
        }
    }
    assert_eq!(
        total, 1,
        "写自识的落点全 crate 只许有一处：多一处就是多一条改 self_concept 的路"
    );
    assert_eq!(
        owners,
        vec!["self_concept.rs".to_string()],
        "那唯一一次写必须落在 `{SELF_CONCEPT_WRITE_OWNER}` 的函数体里"
    );

    // 判据 2：主体的动词在**那份唯一的名单**里（装配／准入／执行三面都问它）。
    let os_source = sources
        .iter()
        .find(|(name, _)| name == "os.rs")
        .map(|(_, text)| text.clone())
        .expect("os.rs 应当在 src/ 下");
    assert!(
        os_source.contains(SELF_CONCEPT_WRITE_VERB),
        "写自识的动词必须在 OS 自有动词名单里——漏一面就是「目录里有、路由里没有」"
    );

    // 判据 3：`self_concept.rs` 的模块级公开面（**顶格**的 `pub fn`）。
    // 按**排序**比，不依赖源码里的书写顺序（今天 read 写在前面、明天写写在前面，都该绿）。
    let self_concept_source = sources
        .iter()
        .find(|(name, _)| name == "self_concept.rs")
        .map(|(_, text)| text.clone())
        .expect("self_concept.rs 应当在 src/ 下");
    let mut module_exports: Vec<String> = self_concept_source
        .lines()
        .filter_map(|line| line.strip_prefix("pub fn "))
        .map(|rest| {
            rest.split(['(', '<'])
                .next()
                .unwrap_or("")
                .trim()
                .to_string()
        })
        .collect();
    module_exports.sort();
    assert_eq!(
        module_exports,
        vec!["read".to_string(), "write_from_subject".to_string()],
        "self_concept 模块只许导出写（write_from_subject）与读（read）——\
         多出来的名字意味着多了一条改自识的路"
    );
    assert!(
        !self_concept_source.contains("\npub fn incept"),
        "`incept` 是那一次写的里子，不许再公开：公开面每多一个名字，就多一条改自识的路"
    );
}

/// `#[ignore]` **必须带理由**（为什么忽略 · 什么时候重跑），且不许是空串或占位符。
///
/// 为什么值得一条机械判据：`#[ignore]` 是"这条用例不算数"的**唯一标记** —— 没有理由的 ignore
/// 等于悄悄删掉一条用例：下一个读的人无从判断它该不该在、什么时候该把它跑起来。
///
/// 判据只要两条：**理由非空**，且同时写清「为什么」与「什么时候」。**不规定措辞**（守卫不许
/// 退化成约束用词）。扫描范围与分层守卫**不同**：这里连 `src/bin/` 一起扫（入口夹具也可能带
/// ignored 用例），另加 `tests/`。
#[test]
fn every_ignored_test_states_why_and_when() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths: Vec<PathBuf> = Vec::new();
    for relative in ["src", "tests"] {
        collect_rust_sources(&root.join(relative), &mut paths);
    }
    paths.sort();

    let mut checked = 0usize;
    for path in &paths {
        let source = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("读不到 {}: {error}", path.display()));
        let name = path
            .strip_prefix(root)
            .unwrap_or(path.as_path())
            .display()
            .to_string();
        for line in source.lines() {
            let Some(rest) = line.trim_start().strip_prefix("#[ignore") else {
                continue;
            };
            // 形状必须是 `#[ignore = "……"]`：裸 `#[ignore]` 与 `#[ignore = ""]` 都算"没有理由"。
            let reason = rest
                .strip_prefix(" = \"")
                .and_then(|tail| tail.strip_suffix("\"]"))
                .unwrap_or("");
            assert!(
                !reason.trim().is_empty(),
                "{name}：有一处 `#[ignore]` 没带理由 —— 写 `#[ignore = \"…\"]`（裸 `#[ignore]` 等于悄悄删用例）"
            );
            assert!(
                reason.contains("为什么") && reason.contains("什么时候"),
                "{name}：`#[ignore]` 的理由要说清「为什么」与「什么时候重跑」——现在写的是：{reason}"
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 2,
        "只认到 {checked} 条 ignored 用例 —— 它们不该消失（不许删用例；≤0 更说明扫描器坏了）"
    );
}

// ---------------------------------------------------------------------------
// 无环守卫：分层的第一道，也是唯一一道**不需要先知道"谁是上层"**的守卫
// ---------------------------------------------------------------------------

/// **源码级依赖图不许有环。**
///
/// 分层的规则是"**上层依赖下层**"——**一条环就是这个规则的反例**，而且判它**不需要先知道
/// 谁是上层、谁是下层**。这正是它能当第一道守卫的原因：那张分层图今天还不完整（`store` /
/// `recovery` / `commit` / `diagnostics` 没有格子），而**环判定不依赖那张图**。
///
/// 它替掉的是一条**具体的**环检查：`foundation_never_knows_core` 只管
/// `foundation → core` 这一条 2 节点边。而在写这条守卫的时候，源码里真的躺着一条
/// **3 节点**的环（`os → file_view → store → os`）——**那条具体的检查看不见它**。
///
/// ## 已知的环：一张**债务账**，不是许可
///
/// 清单里的每一条都是**违规**，不是"允许"。它存在只为一件事：**让门禁现在就能绿，同时
/// 把违规点名**——而不是把它们藏在"守卫只扫两个文件"的盲区里（那正是这条环长出来的原因）。
///
/// 判据是**相等**，不是包含：新增一条环会红（挡住新债），**修掉一条环也会红**（逼这张账
/// 跟着变准）。**它只能变短。**
///
/// ⚠️ **粒度是顶层模块**：`crate::mcp::app` 与 `crate::mcp` 都记成 `mcp`，
/// 因此**子目录内部的环看不见**。如实记在这里，不假装它覆盖了。
///
/// ⚠️ **注释行不参与**：文档里提到一个模块名不是依赖。
#[test]
fn the_source_dependency_graph_has_no_cycles() {
    // **账已还清**：这条守卫写下时图里有一条三节点的环（`os → file_view → store → os`）——
    // `os` 构造 `FileView`、`file_view` 的 `history/` 读经历、而 `store` 的 `WakeOrigin::text()`
    // 反过来取 `os` 的四个 `*_ORIGIN_TEXT` 文案。那一刀把起源文案与那个处境来源搬出循环层
    // （`src/activity_origin.rs`），翻译改由装配层做，于是 `store → os` 这条边消失、环解开。
    //
    // **清单留在这里，空着。** 它空着才说明"图上没有环"是一句断言，而不是"没人查过"。
    // 一旦再长出环，这条守卫立刻红。
    const KNOWN_CYCLES: [&str; 0] = [];

    let graph = module_dependency_graph();
    assert!(
        graph.len() >= 30,
        "只认出 {} 个顶层模块 —— 扫描器坏了，守卫形同虚设",
        graph.len()
    );

    let cycles = cycles_in(&graph);
    assert_eq!(
        cycles,
        KNOWN_CYCLES
            .iter()
            .map(|members| members.to_string())
            .collect::<Vec<_>>(),
        "源码依赖图的环集合变了。\n\
         新增环 ⇒ 分层被破（不许）；\n\
         少了环 ⇒ 债还了，把 `KNOWN_CYCLES` 里对应那条删掉（这张账只能变短）。\n\
         实得：{cycles:?}"
    );

    // 自检：扫描器必须认得出真的环、也必须不冤枉无环图，否则上面那条可以永久假绿。
    let cyclic = BTreeMap::from([
        ("a".to_string(), BTreeSet::from(["b".to_string()])),
        ("b".to_string(), BTreeSet::from(["c".to_string()])),
        ("c".to_string(), BTreeSet::from(["a".to_string()])),
        ("d".to_string(), BTreeSet::from(["a".to_string()])),
    ]);
    assert_eq!(
        cycles_in(&cyclic),
        vec!["a, b, c".to_string()],
        "环检测必须报出 a → b → c → a，而且不把 d 算进去"
    );
    let acyclic = BTreeMap::from([
        ("a".to_string(), BTreeSet::from(["b".to_string()])),
        ("b".to_string(), BTreeSet::new()),
    ]);
    assert!(cycles_in(&acyclic).is_empty(), "无环图不许被报成有环");
    // 反例：`crate::` 出现在注释里不算依赖。
    assert!(
        crate_references("//! 它认识 [`crate::os`] 的文案\n").is_empty(),
        "注释行里的 `crate::` 不是依赖"
    );
    assert!(
        !crate_references("use crate::os::MANUAL_ORIGIN_TEXT;\n").is_empty(),
        "代码行里的 `crate::` 必须被认出来"
    );
}

/// 顶层模块 → 它引用的顶层模块（`crate::x` 形式的引用；注释行不参与）。
fn module_dependency_graph() -> BTreeMap<String, BTreeSet<String>> {
    let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (file, source) in core_sources() {
        let module = top_level_module(&file);
        let entry = graph.entry(module.clone()).or_default();
        for target in crate_references(&source) {
            if target != module {
                entry.insert(target);
            }
        }
    }
    graph
}

/// `os.rs` → `os`；`mcp/mod.rs` → `mcp`；`mcp/app.rs` → `mcp`。
fn top_level_module(file: &str) -> String {
    file.split('/')
        .next()
        .unwrap_or(file)
        .trim_end_matches(".rs")
        .to_string()
}

/// `lib.rs` 里声明的模块名（`pub mod x;`）——**模块表的派生口**。
///
/// 只认 `pub mod`：模块表是**公开面**（§6 说的"模块表"就是它），私有子模块不进这张表。
/// 注释先按 `//` 整段切掉（说明里提到一个模块名不是声明，行尾注释也不该让一条真声明漏掉）。
fn declared_modules(facade: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for raw in facade.lines() {
        let line = raw.split("//").next().unwrap_or("").trim();
        let Some(rest) = line.strip_prefix("pub mod ") else {
            continue;
        };
        let Some(name) = rest.strip_suffix(';') else {
            continue;
        };
        let name = name.trim();
        if !name.is_empty() {
            out.insert(name.to_string());
        }
    }
    out
}

/// 一行代码里出现的全部 `crate::<名字>`。**注释行整行跳过**（文档里提到一个模块名不是依赖）。
fn crate_references(source: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in source.lines() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        let mut rest = line;
        while let Some(at) = rest.find("crate::") {
            rest = &rest[at + "crate::".len()..];
            let name: String = rest
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                .collect();
            if !name.is_empty() {
                out.insert(name);
            }
        }
    }
    out
}

/// 图里的全部环，每个环报成一行 `成员, 成员, …`（成员按字典序，行按字典序）。
///
/// 判据是**互相可达**：一个模块能回到自己 ⇒ 它在环里；同环成员 = 与它互相可达的那些。
fn cycles_in(graph: &BTreeMap<String, BTreeSet<String>>) -> Vec<String> {
    // 可达集：迭代到不动点（不用递归，免得深图把栈打爆）。
    let mut reach: BTreeMap<String, BTreeSet<String>> = graph
        .iter()
        .map(|(module, targets)| (module.clone(), targets.clone()))
        .collect();
    loop {
        let snapshot = reach.clone();
        let mut changed = false;
        for targets in reach.values_mut() {
            let mut add: BTreeSet<String> = BTreeSet::new();
            for target in targets.iter() {
                if let Some(next) = snapshot.get(target) {
                    add.extend(next.iter().cloned());
                }
            }
            for item in add {
                if targets.insert(item) {
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut cycles = Vec::new();
    for module in graph.keys() {
        if !reach[module].contains(module) {
            continue;
        }
        let members: Vec<String> = graph
            .keys()
            .filter(|other| {
                *other == module
                    || (reach[module].contains(*other) && reach[*other].contains(module))
            })
            .cloned()
            .collect();
        cycles.push(members.join(", "));
    }
    cycles.sort();
    cycles.dedup();
    cycles
}

/// **夹具二进制的后缀只许在一处拼**（`tests/common/mod.rs` 的 `binary()`）。
///
/// 判据：`tests/` 下的 `.rs`（除 `common/mod.rs`）里**不许自己拼可执行件后缀**——
/// 也就是不许出现「`.exe"` 与 `format!(` 或 `join(` 同处一行」这种形状。
///
/// 为什么值得一条机械判据：仓库里曾有十来份各自拼 `.exe` 的副本，其中**一份漏了平台判断**
/// （`responses_protocol`），于是在 Linux 上它拼出 `agent_run.exe`（不存在）⇒
/// 那一支 6 条用例全死在"夹具跑得起来"（ENOENT），**在 Linux 上从来没跑过真东西**。
/// 症状（夹具跑不起来）与根因（平台形状散成多份）隔着好几层，只有看源码才看得见。
///
/// 不误伤：产品里的 shell 名（`powershell.exe` 这类**字面量**，不是拼出来的路径）不算违规——
/// 判据只认「拼」这个动作，不认 `.exe` 这四个字符。
#[test]
fn the_fixture_suffix_is_built_in_exactly_one_place() {
    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut offenders: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    for entry in fs::read_dir(&tests).expect("读得到测试目录") {
        let path = entry.expect("目录项可读").path();
        if path.extension().and_then(|each| each.to_str()) != Some("rs") {
            continue; // `common/` 是目录，跳过
        }
        let name = path
            .file_name()
            .and_then(|each| each.to_str())
            .unwrap_or_default()
            .to_string();
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("读不到 {}: {error}", path.display()));
        scanned += 1;
        for (index, line) in source.lines().enumerate() {
            let trimmed = line.trim_start();
            // 注释是散文，不是"拼路径"这个动作——判据只认代码行。
            if trimmed.starts_with("//") || trimmed.starts_with('*') {
                continue;
            }
            if !line.contains(".exe\"") {
                continue;
            }
            if line.contains("format!(") || line.contains("join(") {
                offenders.push(format!("{name}:{} {}", index + 1, line.trim()));
            }
        }
    }
    assert!(scanned > 0, "一条测试源码都没扫到——判据空跑了");
    assert!(
        offenders.is_empty(),
        "夹具后缀只许在 tests/common/mod.rs 里拼（平台形状只许有一份）：{offenders:#?}"
    );

    // 反面之外还要有正面：那一份**真的在**，而且**真的带平台判断**——
    // 否则这条判据可以靠"谁都不拼后缀"通过。
    let shared = fs::read_to_string(tests.join("common").join("mod.rs")).expect("读得到共用件");
    assert!(
        shared.contains("pub fn binary"),
        "共用件里该有夹具路径那一份"
    );
    assert!(
        shared.contains("cfg!(windows)"),
        "共用件里该有平台判断（这个平台上的后缀只有它知道）"
    );
}
