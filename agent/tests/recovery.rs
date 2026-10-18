//! **只读判定**的验收：只从已确认记录推导一次活动的状态，而且一个字节都不写。
//!
//! ⚠ **旧规矩是什么、新规矩是什么**（2026-09-26）。这一份文件原来叫"跨进程 Run 恢复"验收，
//! 它证明的是**受理判据**那一层：五类结论 → 两条受理入口（严格 / 带未决效果）→ 建一个会话
//! **接着跑同一个 RunId**（预算不重置、展开状态从空开始、历史授权不续期、登记不重写）。
//!
//! **那条路已退役**：承载活动的进程没了，就是两个 Run——那一次连续
//! 就断了，新进程起来是**新的一段**，不是"顶着同一个 RunId 继续"。于是：
//!
//! - 原料（轮次 / 开场输入 / 轨迹 / 已确认事实）、两条受理入口、`Run::resume`、
//!   `RunSession::resume`、`ManualResume` 全部删掉；随之退役的用例是"接着跑"那一族
//!   （预算不重置、展开从空开始、授权不续期、收场后不得再驱动、轮次对得上）；
//! - **判定本身留下**，因为它还有一个真实消费者：`agent_host_status` 按活动逐个读它，
//!   把"这一个活动现在什么样"摆给操作者看。所以这一份文件现在只证明**读数**：
//!   五类结论（`unfinished` / `ended` / `blocked` / `unknown` / `refused`）由已确认的经历与
//!   事实推导出来，互相**不混同**、不压成布尔，而且判定过程**零写入**。
//!
//! "换一个进程"在这里仍然是**真的换一个进程内句柄**：进程 A 打开环境、驱动到等待、丢弃句柄；
//! 进程 B 重新 `open` 同一个目录（重新取写资格、重新读已确认前缀），再判定。
//!
//! 明确**不**声称：没有运行任何公网模型；没有验证"同一目录被两个进程同时打开"时的行为
//! （那是写资格审查的事，不在本包范围内）。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use celira_agent::clock::ClockHandle;
use celira_agent::composition::{RunSession, SessionDeclarations, SessionDrive, SessionRequest};
use celira_agent::context_reading::ContextReading;
use celira_agent::core::{
    Action, ActionId, ActionParams, Limits, ModelError, ModelFailureClass, ModelOutput, ModelPort,
    RunInput, View,
};
use celira_agent::file_app::FILE_APP_ID;
use celira_agent::foundation::{AgentId, Outcome, RunId, SubjectEnding};
use celira_agent::recovery::{self, RunState};
use celira_agent::store::{Environment, StoreError};

mod common;

use common::InspectsFirst;

const AGENT: &str = "agent-recovery";
const RUN: &str = "run-recovered";

/// 用例装配用的**时间来源**：固定钟——离线、可逐字节断言。
///
/// 生产装配给真实钟（`agent_run`），用例一律给固定钟：状态行的 `time` 因此在每次运行里都是同一个
/// 读数，任何"同输入同输出"的断言都不会被墙上时钟搅动。它只换时间来源，**不换**装配代码。
fn fixed_clock() -> ClockHandle {
    ClockHandle::fixed_at_unix_seconds(0)
}

/// 用例声明的**上下文窗口**（宿主声明，必须正整数）：取一个小而固定的数字，便于逐字节断言。
///
/// 生产入口缺省用适配层的公开标称值（1M），用例一律显式声明——窗口是声明不是测量，用例自己
/// 声明一个"屏幕有多小"才能把 `context` 那一段钉住。
const CONTEXT_WINDOW: u32 = 1_000;

/// 用例装配用的**屏幕报告 cell 的读侧**：默认是**空 cell** ⇒ 状态行读作 `context = unknown/… token`。
///
/// 要模拟"适配层报过一次"的形态，用 `ContextReading::channel()` 拿写柄自己写（写柄在这一侧只是
/// 用例的替身；生产里它只交给适配层——这条纪律由源码守卫
/// `only_the_adapter_writes_the_reading_cell` 机械检查）。
fn reading_cell() -> ContextReading {
    ContextReading::default()
}

// ---------------------------------------------------------------------------
// 自建的独立测试目录与模型替身
// ---------------------------------------------------------------------------

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "celira-task09-recovery-{label}-{}-{n}",
            std::process::id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("清掉同名残留目录");
        }
        std::fs::create_dir_all(&path).expect("建立测试目录");
        Self { path }
    }

    fn materials(&self) -> PathBuf {
        let materials = self.path.join("materials");
        std::fs::create_dir_all(&materials).expect("建资料目录");
        std::fs::write(materials.join("alpha.txt"), "alpha").expect("写合成文件");
        materials
    }

    fn env_root(&self) -> PathBuf {
        self.path.join("env")
    }

    fn trace_path(&self) -> PathBuf {
        self.env_root().join("trace.ndjson")
    }

    fn facts_path(&self) -> PathBuf {
        self.env_root().join("facts.ndjson")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn agent_id() -> AgentId {
    AgentId::new(AGENT)
}

/// 按脚本作答的模型替身：它记下自己**被调用了几次**，以及每次看到的可用能力。
struct ScriptedModel {
    script: VecDeque<ModelOutput>,
    calls: usize,
    seen: Vec<Vec<String>>,
}

impl ScriptedModel {
    fn new(script: Vec<ModelOutput>) -> Self {
        Self {
            script: VecDeque::from(script),
            calls: 0,
            seen: Vec::new(),
        }
    }

    /// 主体这一程一共调了几次模型。
    ///
    /// ⚠️ `#[allow(dead_code)]`：这是**测试侧的辅助读数**，不是每个用例都用到——
    /// 不用它的那些文件里它照样该在（想数的时候就在手边）。
    #[allow(dead_code)]
    fn calls(&self) -> usize {
        self.calls
    }
}

impl ModelPort for ScriptedModel {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        self.calls += 1;
        self.seen.push(
            view.capabilities()
                .items()
                .iter()
                .map(|capability| capability.id().as_str().to_string())
                .collect(),
        );
        // 脚本用尽就是模型失败：那是系统终止，不是"正常等待"。
        self.script.pop_front().ok_or(scripted_model_failure())
    }
}

fn expand_files() -> ModelOutput {
    ModelOutput::act(Action::with_params(
        ActionId::new("open_app"),
        ActionParams::none().with("target", FILE_APP_ID),
    ))
}

fn list_files() -> ModelOutput {
    ModelOutput::act(Action::with_params(
        ActionId::new("files_list"),
        ActionParams::none().with("path", "."),
    ))
}

fn text(body: &str) -> ModelOutput {
    ModelOutput::say(body)
}

fn finish(ending: SubjectEnding) -> ModelOutput {
    ModelOutput::finish(ending)
}

// ---------------------------------------------------------------------------
// 读写环境的原始字节（用于"判定不改写任何东西"）
// ---------------------------------------------------------------------------

fn bytes_of(path: &Path) -> Vec<u8> {
    std::fs::read(path).expect("读文件")
}

/// 读回一行一个 JSON 对象（`seq` 只表示追加顺序，不表示内容）。
fn objects_of(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .expect("读文件")
        .lines()
        .map(|line| serde_json::from_str(body_of(line)).expect("每一行都是一个 JSON 对象"))
        .collect()
}

/// 一行的**正文**（存储行尾带校验和；这些用例是存储手术，见 `host_status.rs` 同款说明）。
fn body_of(line: &str) -> &str {
    match line.rfind('\t') {
        Some(index) => &line[..index],
        None => line,
    }
}

/// 写回一组记录，并把 `seq` **重排成从 0 开始的连续序号**。
///
/// 存储要求序号连续（`previous + 1`），因此"删掉中间一条"必须连序号一起重排——重排只动序号，
/// 不动任何内容字段。这样造出来的正是"一次提交只写成了前几步"那种真实形状。
fn write_objects(path: &Path, objects: &[serde_json::Value]) {
    let mut text = String::new();
    for (index, object) in objects.iter().enumerate() {
        let mut object = object.clone();
        object["seq"] = serde_json::Value::from(index as u64);
        text.push_str(&object.to_string());
        text.push('\n');
    }
    std::fs::write(path, text).expect("写回文件");
}

/// 只保留**不含** marker 的记录（用来造出"崩溃窗口"那种半截形状）。
fn drop_lines_containing(path: &Path, marker: &str) {
    let kept: Vec<serde_json::Value> = objects_of(path)
        .into_iter()
        .filter(|object| !object.to_string().contains(marker))
        .collect();
    write_objects(path, &kept);
}

/// 追加一条记录（用来造出"本写入者写不出来的形状"）。
fn append_object(path: &Path, object: serde_json::Value) {
    let mut objects = objects_of(path);
    objects.push(object);
    write_objects(path, &objects);
}

/// 一条合成记录：`{"seq":<自动>,"run":<RUN>,...其余字段}`。
fn record(fields: &str) -> serde_json::Value {
    serde_json::from_str(&format!("{{\"seq\":0,\"run\":\"{RUN}\",{fields}}}"))
        .expect("合成记录必须是合法 JSON")
}

// ---------------------------------------------------------------------------
// 一个真实的"停在半路"的活动
// ---------------------------------------------------------------------------

/// 进程 A：打开环境、建立活动、驱动到等待，然后**丢弃句柄**（相当于进程退出）。
///
/// 它用的是真实会话：登记、注入、模型响应、提交顺序都按生产路径发生。
fn drive_to_waiting(dir: &TempDir, script: Vec<ModelOutput>, limits: u32) {
    let materials = dir.materials();
    let mut environment = Environment::create(&dir.env_root(), agent_id()).expect("建立环境");
    let mut model = InspectsFirst::new(ScriptedModel::new(script));
    let declared = agent_id();
    let mut session = RunSession::open(
        &materials,
        &mut environment,
        SessionRequest {
            declarations: SessionDeclarations::default(),
            declared_agent: &declared,
            run: RunId::new(RUN),
            limits: Limits::new(limits),
            input: RunInput::text("开场输入").expect("在上限内"),
            clock: fixed_clock(),
            context_window: CONTEXT_WINDOW,
            reading: reading_cell(),
        },
    )
    .expect("会话可以打开");
    let driven = session.drive(&mut model).expect("驱动不应失败");
    assert!(driven.is_unsettled(), "这个夹具必须停在等待");
    drop(session);
    drop(environment);
}

/// 进程 B：重新打开同一个环境（新的写资格、新的内存状态）。
fn reopen(dir: &TempDir) -> Environment {
    Environment::open(&dir.env_root(), agent_id()).expect("重新打开环境")
}

/// 进程 A：把一个活动**跑完**（纯正文等待一次、再由宿主显式继续到主体收场）。
fn drive_to_ended(dir: &TempDir) {
    let materials = dir.materials();
    let mut environment = Environment::create(&dir.env_root(), agent_id()).expect("建立环境");
    let declared = agent_id();
    let mut session = RunSession::open(
        &materials,
        &mut environment,
        SessionRequest {
            declarations: SessionDeclarations::default(),
            declared_agent: &declared,
            run: RunId::new(RUN),
            limits: Limits::new(6),
            input: RunInput::text("开场输入").expect("在上限内"),
            clock: fixed_clock(),
            context_window: CONTEXT_WINDOW,
            reading: reading_cell(),
        },
    )
    .expect("会话可以打开");
    let mut model = InspectsFirst::new(ScriptedModel::new(vec![
        text("等"),
        finish(SubjectEnding::Silent),
    ]));
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());
    let ended = session
        .drive_with_input(RunInput::text("第二段").expect("在上限内"), &mut model)
        .expect("驱动");
    assert_eq!(
        ended,
        SessionDrive::Ended(Outcome::Subject(SubjectEnding::Silent)),
        "这个夹具必须真的收场"
    );
    drop(session);
    drop(environment);
}

/// 换一个新句柄读一次判定。
fn inspect(dir: &TempDir) -> RunState {
    let environment = reopen(dir);
    recovery::inspect(&environment.reader(), &RunId::new(RUN))
}

// ---------------------------------------------------------------------------
// 1. 停在半路：没有收尾事实，而记录解释得了它
// ---------------------------------------------------------------------------

#[test]
fn a_run_left_unfinished_reads_as_unfinished() {
    let dir = TempDir::new("unfinished");
    drive_to_waiting(&dir, vec![expand_files(), text("我先说一句，等宿主")], 6);

    // ⚠ **旧规矩 → 新规矩**：这一档从前叫 `resumable-waiting`，读作"**可以显式继续**"。
    // 新规矩下它是一个**读数**："记录里没有收尾事实，而且记录解释得了它"——**不是**一句许可。
    let state = inspect(&dir);
    assert_eq!(state.as_str(), "unfinished");
    assert_eq!(state.reason(), None, "这一档没有阻断原因");
    assert!(matches!(state, RunState::Unfinished));
}

// ---------------------------------------------------------------------------
// 2. 已经结束的活动：读成 `ended`，而且判定零写入
// ---------------------------------------------------------------------------

#[test]
fn an_ended_run_reads_as_ended_and_the_judgement_writes_nothing() {
    let dir = TempDir::new("ended");
    drive_to_ended(&dir);
    let trace_before = bytes_of(&dir.trace_path());
    let facts_before = bytes_of(&dir.facts_path());

    let state = inspect(&dir);
    assert_eq!(state.as_str(), "ended");
    match state {
        RunState::Ended {
            outcome,
            sealed,
            unconfirmed,
        } => {
            assert_eq!(outcome, Outcome::Subject(SubjectEnding::Silent));
            assert!(sealed, "收场之后必须已经封存");
            // ⚠ **两件事是独立的**（2026-10-10 裁决）：这一段正常收场了，**而且**它没有
            // 未确认的效果——`Ended` 从此把那一格一起带走，缺了它下一段就看不到那几件。
            assert!(
                unconfirmed.is_empty(),
                "这一段每一件都有结果 ⇒ 这一格是空，不是「读不到」: {unconfirmed:?}"
            );
        }
        other => panic!("应当是 ended，却得到 {other:?}"),
    }

    assert_eq!(bytes_of(&dir.trace_path()), trace_before, "判定不得写经历");
    assert_eq!(bytes_of(&dir.facts_path()), facts_before, "判定不得写事实");
}

// ---------------------------------------------------------------------------
// 3. 未决外部效果：已准备未派发 = 阻断；已派发无结果 = 未知
// ---------------------------------------------------------------------------

#[test]
fn a_prepared_but_undispatched_action_reads_as_blocked() {
    let dir = TempDir::new("prepared");
    drive_to_waiting(&dir, vec![expand_files(), list_files(), text("等")], 6);
    // 崩溃窗口：准备了、也派发过，却没留下结果 → 这里再退一步，连"已派发"都没有。
    drop_lines_containing(&dir.facts_path(), "action-dispatched");
    drop_lines_containing(&dir.facts_path(), "action-result");

    let state = inspect(&dir);
    assert_eq!(state.as_str(), "blocked");
    assert_eq!(state.reason(), Some("prepared-without-dispatch"));
}

/// **结束了 ≠ 结算了**（2026-10-10 裁决）：一段**已经收场**的活动，照样可能带着"派发过、
/// 没有结果"的那几件——而它们必须**一起被带走**，否则下一段的她一个字都看不到。
///
/// 从前 `Ended` 只带 `outcome`：一次"带着未确认效果收场"的活动（`SystemEnding::Indeterminate`
/// 说的就是这个）在读出来的时候，那几件**凭空消失**，而开场那张纸条还会读成"确实没有"。
#[test]
fn an_ended_run_still_carries_its_unconfirmed_operations() {
    let dir = TempDir::new("ended-with-unconfirmed");
    // 真的派发过动作（`InspectsFirst` 先展开一次，展开本身也是派发过的操作）。
    drive_to_waiting(&dir, vec![expand_files(), list_files(), text("等")], 6);
    // 崩溃窗口：**收场写下了**，而结果没有回来 —— 收场与"这一件结算了没有"是两件事。
    drop_lines_containing(&dir.facts_path(), "action-result");
    append_object(
        &dir.trace_path(),
        record(r#""kind":"ending","subject":"silent""#),
    );
    append_object(
        &dir.facts_path(),
        record(r#""fact":"run-ended","subject":"silent""#),
    );

    let state = inspect(&dir);
    assert_eq!(state.as_str(), "ended", "它确实收场了");
    let dispatched = objects_of(&dir.facts_path())
        .iter()
        .filter(|object| {
            object.get("fact").and_then(|value| value.as_str()) == Some("action-dispatched")
        })
        .count();
    assert!(dispatched > 0, "夹具前提：确实有已派发记录");
    match state {
        RunState::Ended { unconfirmed, .. } => assert_eq!(
            unconfirmed.len(),
            dispatched,
            "**结束了不等于结算了**：每一件已派发无结果的操作都要带走"
        ),
        other => panic!("应当是 ended，却得到 {other:?}"),
    }
}

#[test]
fn a_dispatched_action_without_a_result_reads_as_unknown_not_blocked() {
    let dir = TempDir::new("dispatched");
    drive_to_waiting(&dir, vec![expand_files(), list_files(), text("等")], 6);
    // 崩溃窗口：派发已经写到盘上，结果没有 → 外部效果**可能已经发生**。
    drop_lines_containing(&dir.facts_path(), "action-result");

    let state = inspect(&dir);
    assert_eq!(state.as_str(), "unknown");
    assert_eq!(state.reason(), Some("dispatched-without-result"));
    // TASK_69：主体开场那几批"先看格式"也是**派发过的操作** ⇒ 条数不能写死。
    // 按盘上"已派发"的条数对账：结果被删光了，所以每一个都该在未决清单里。
    let dispatched = objects_of(&dir.facts_path())
        .iter()
        .filter(|object| {
            object.get("fact").and_then(|value| value.as_str()) == Some("action-dispatched")
        })
        .count();
    assert!(dispatched > 2, "夹具前提：确实有多条已派发记录");
    match state {
        RunState::Unknown { operations, .. } => {
            // 每个操作都已经派发、都没有结果：它们各自都是"可能已经发生"。
            assert_eq!(
                operations.len(),
                dispatched,
                "每个已派发无结果的操作都要留在这里"
            );
        }
        other => panic!("应当是 unknown，却得到 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 4. 两份存储互相矛盾、输入未确认：阻断，不挑一边信
// ---------------------------------------------------------------------------

#[test]
fn facts_that_contradict_the_trace_read_as_blocked() {
    let dir = TempDir::new("cross-store");
    drive_to_waiting(&dir, vec![text("等")], 6);
    // 经历里说"活动开始"，事实里没有：这是"只写成前几步"留下的可判定形状。
    drop_lines_containing(&dir.facts_path(), "run-started");

    let state = inspect(&dir);
    assert_eq!(state.as_str(), "blocked");
    assert_eq!(state.reason(), Some("cross-store:start-without-fact"));
}

#[test]
fn a_run_whose_input_was_never_confirmed_reads_as_blocked() {
    let dir = TempDir::new("no-input");
    drive_to_waiting(&dir, vec![text("等")], 6);
    // 登记只写了一半：活动开始了，开场输入没有确认。
    drop_lines_containing(&dir.trace_path(), r#""kind":"run-input""#);

    let state = inspect(&dir);
    assert_eq!(state.as_str(), "blocked");
    assert_eq!(state.reason(), Some("input-not-confirmed"));
}

// ---------------------------------------------------------------------------
// 5. 事实不可读 / 不可解释：拒绝，而且不改写
// ---------------------------------------------------------------------------

#[test]
fn an_unknown_run_reads_as_refused() {
    let dir = TempDir::new("unknown-run");
    drive_to_waiting(&dir, vec![text("等")], 6);
    let environment = reopen(&dir);
    let state = recovery::inspect(&environment.reader(), &RunId::new("run-never-existed"));
    assert_eq!(state.as_str(), "refused");
    assert_eq!(state.reason(), Some("unknown-run"));
}

#[test]
fn shapes_this_writer_cannot_produce_are_refused_with_a_distinguishable_reason() {
    // (1) 注入阶段不是本写入者可能写出的取值。
    let unknown_stage = TempDir::new("unknown-stage");
    drive_to_waiting(&unknown_stage, vec![text("等")], 6);
    append_object(
        &unknown_stage.trace_path(),
        record(r#""kind":"injection","stage":"something-new","turn":1,"blocks":[]"#),
    );
    let state = inspect(&unknown_stage);
    assert_eq!(state.as_str(), "refused");
    assert_eq!(state.reason(), Some("rehydrate:unknown-stage"));

    // (2) 拒绝经历的两块不齐。
    let broken_refusal = TempDir::new("broken-refusal");
    drive_to_waiting(&broken_refusal, vec![text("等")], 6);
    append_object(
        &broken_refusal.trace_path(),
        record(
            r#""kind":"injection","stage":"admission-refused","turn":1,"blocks":[{"label":"action","text":"files_list"}]"#,
        ),
    );
    let state = inspect(&broken_refusal);
    assert_eq!(state.reason(), Some("rehydrate:refused-marker-incomplete"));

    // (3) 行动结果的三分与内容组合不是本写入者可能写出的形状（失败却带内容）。
    let impossible_outcome = TempDir::new("impossible-outcome");
    drive_to_waiting(&impossible_outcome, vec![text("等")], 6);
    append_object(
        &impossible_outcome.trace_path(),
        record(
            r#""kind":"observed","turn":1,"action":"files_list","params":[],"state":"failure","blocks":[{"label":"listing","text":"x"}]"#,
        ),
    );
    let state = inspect(&impossible_outcome);
    assert_eq!(state.reason(), Some("rehydrate:outcome-not-producible"));
}

// ---------------------------------------------------------------------------
// 6. 收场词汇表的解码守卫
// ---------------------------------------------------------------------------

#[test]
fn every_known_ending_decodes_and_unknown_ones_do_not() {
    use celira_agent::core::ModelFailureClass;
    use celira_agent::foundation::SystemEnding;
    use celira_agent::store::outcome_of;

    for (name, ending) in [
        ("completed", SubjectEnding::Completed),
        ("silent", SubjectEnding::Silent),
        ("declined", SubjectEnding::Declined),
        ("waiting", SubjectEnding::Waiting),
    ] {
        assert_eq!(
            outcome_of(Some(name), None, None),
            Some(Outcome::Subject(ending))
        );
    }
    for (name, ending) in [
        ("model-failed", SystemEnding::ModelFailed(None)),
        ("action-failed", SystemEnding::ActionFailed),
        ("cancelled", SystemEnding::Cancelled),
        ("timeout", SystemEnding::Timeout),
        ("crashed", SystemEnding::Crashed),
        ("indeterminate", SystemEnding::Indeterminate),
    ] {
        assert_eq!(
            outcome_of(None, Some(name), None),
            Some(Outcome::System(ending))
        );
    }
    // 两种都有、两种都没有、取值不认识：都是"本写入者写不出来的形状"，一律不解码。
    assert_eq!(outcome_of(Some("completed"), Some("timeout"), None), None);
    assert_eq!(outcome_of(None, None, None), None);
    assert_eq!(outcome_of(Some("whatever"), None, None), None);
    assert_eq!(outcome_of(None, Some("whatever"), None), None);

    // 失败分类：与事实侧 `run-ended` 的 `failure` 同一条规矩 —— 认得的带出来，
    // 认不出的不解码（fail-closed），挂在非模型失败上不解码（自相矛盾），
    // 缺了就是 `ModelFailed(None)`（旧记录，**不编造**）。
    assert_eq!(
        outcome_of(None, Some("model-failed"), Some("transport-failure")),
        Some(Outcome::System(SystemEnding::ModelFailed(Some(
            ModelFailureClass::Transport(None).as_str()
        ))))
    );
    assert_eq!(
        outcome_of(None, Some("model-failed"), Some("no-such-class")),
        None
    );
    assert_eq!(
        outcome_of(None, Some("timeout"), Some("transport-failure")),
        None
    );
    assert_eq!(
        outcome_of(Some("completed"), None, Some("transport-failure")),
        None
    );
}

// ---------------------------------------------------------------------------
// 7. 打开环境时不做任何事（只读判定与写资格是两件事）
// ---------------------------------------------------------------------------

#[test]
fn merely_opening_the_environment_changes_nothing() {
    let dir = TempDir::new("open-only");
    drive_to_waiting(&dir, vec![text("等")], 6);
    let trace_before = bytes_of(&dir.trace_path());
    let facts_before = bytes_of(&dir.facts_path());

    let environment = reopen(&dir);
    // 打开 + 只读判定 + 读准入读数，都不该写任何字节。
    let state = recovery::inspect(&environment.reader(), &RunId::new(RUN));
    assert_eq!(state.as_str(), "unfinished");
    let admission = environment.admission().expect("准入读数");
    // 准入读数照旧如实说"环境里有一个没走完的活动"——它**不再拦**新活动（2026-09-26），
    // 但它也**没有被解决**：事实一条不少。
    assert_eq!(admission.as_str(), "unfinished");
    drop(environment);

    assert_eq!(bytes_of(&dir.trace_path()), trace_before);
    assert_eq!(bytes_of(&dir.facts_path()), facts_before);
}

// ---------------------------------------------------------------------------
// 8. 身份与目录仍然是权威的
// ---------------------------------------------------------------------------

#[test]
fn a_foreign_identity_cannot_open_this_environment() {
    let dir = TempDir::new("foreign");
    drive_to_waiting(&dir, vec![text("等")], 6);

    let wrong = AgentId::new("agent-other");
    // 环境本身的身份校验在打开那一步就挡着——**不依赖**任何上层入口。
    let opened = Environment::open(&dir.env_root(), wrong.clone());
    assert!(matches!(
        opened.expect_err("身份错配必须被拒绝"),
        StoreError::IdentityMismatch { .. }
    ));

    // 而正确的身份照旧打得开：上面的拒绝不是"谁都打不开"。
    let environment = reopen(&dir);
    assert_eq!(environment.agent(), &agent_id());
}

// ---------------------------------------------------------------------------
// 9. 写资格必须重新取得：旧进程持有的资格不跨进程延续
// ---------------------------------------------------------------------------

#[test]
fn the_judgement_does_not_inherit_the_write_qualification() {
    let dir = TempDir::new("write-qualification");
    drive_to_waiting(&dir, vec![text("等")], 6);

    // 一个**还活着**的句柄持着这个环境的写资格。
    let held = reopen(&dir);
    // 此刻再开一次（相当于"另一个进程"）必须被拒绝，而不是共享一份写资格。
    let second = Environment::open(&dir.env_root(), agent_id());
    assert!(
        matches!(second.expect_err("写资格不共享"), StoreError::AlreadyOpen),
        "同一时刻只允许一个写者"
    );

    // 归还之后才能重新取得——判定依赖的正是"重新 open 一次"，而不是继承旧资格。
    drop(held);
    let fresh = reopen(&dir);
    let state = recovery::inspect(&fresh.reader(), &RunId::new(RUN));
    assert_eq!(state.as_str(), "unfinished");
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
