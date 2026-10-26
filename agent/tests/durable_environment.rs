//! 阶段八核心验收：**身份环境、增量确认点与保存失败后的保护**。
//!
//! 全部离线、全合成数据、只用自建的独立测试目录（不扫任何个人数据、不读任何密钥）。
//!
//! 这一份测试要证明的是**结构性事实**，不是"跑通一遍"：
//!
//! 1. 身份是稳定的，不是调用者手上一根字符串——归属不符、缺失、损坏一律明确拒绝；
//! 2. 每个确认点都**真的**先落盘再继续：输入、注入、选择、发出的动作、真实结果、终态与封存；
//! 3. 保存失败**停止**新的模型调用与新的行动尝试，但**不重发**已发出的动作、不伪造终态；
//! 4. 损坏尾部被**检测到**，而不是被顺手丢掉冒充"完整成功"；
//! 5. 重开之后能如实区分：身份在不在、前缀到哪、操作结果是否被确认。

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
// `Command` 与 `common::binary` **只被 Unix 那一支**（`RLIMIT_FSIZE` 夹具）用：不加 `cfg`
// 的话 Windows 构建里它们是死 import，而门禁带 `-D warnings`（会红）。
#[cfg(unix)]
use std::process::Command;
use std::rc::Rc;

mod common;
#[cfg(unix)]
use common::binary;

use celira_agent::composition::{
    CompositionError, EnvironmentActivity, run_activity_in_environment,
};
use celira_agent::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Agent,
    Capabilities, CommitError, CommitSeam, CommitStage, ContentBatch, ContentBlock, ContextSource,
    InjectionContext, Limits, ModelError, ModelFailureClass, ModelOutput, ModelPort, RunError,
    RunInput, RunRequest, TraceEntry, View, no_cost, run_activity_with,
};
use celira_agent::foundation::{
    ActionResult, AgentId, Ledger, OperationId, Outcome, Record, ResultState, RunId, SubjectEnding,
    SystemEnding, Turn,
};
use celira_agent::store::{
    Admission, Environment, EnvironmentReader, StoreError, TracePayload, ending_payload, list_runs,
    paused_reason, payload_of,
};

// ---------------------------------------------------------------------------
// 测试目录：每个测试自建独立目录，用完自己清掉。
// ---------------------------------------------------------------------------

/// 一个自建的独立测试目录。绝不指向桌面 / 家目录 / 仓库根。
///
/// 名字里带进程号与一个递增计数，避免并行测试互相踩；`Drop` 时递归清理。
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!("celira-phase08-{label}-{}-{n}", std::process::id()));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("清掉同名残留目录");
        }
        std::fs::create_dir_all(&path).expect("建立测试目录");
        Self { path }
    }

    /// 环境目录（测试目录下的 `env`）。
    fn env(&self) -> PathBuf {
        self.path.join("env")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// ---------------------------------------------------------------------------
// 中立测试替身：会记账的模型、会记账的执行端、会记账/会失败的提交接缝。
// ---------------------------------------------------------------------------

/// 按脚本作答的模型，同时记录每次被调用时看到的轨迹正文。
struct ScriptedModel {
    script: VecDeque<ModelOutput>,
    /// 每次调用看到的（轮次, 轨迹条数）。
    seen: Vec<(u32, usize)>,
}

impl ScriptedModel {
    fn new(script: Vec<ModelOutput>) -> Self {
        Self {
            script: VecDeque::from(script),
            seen: Vec::new(),
        }
    }

    fn calls(&self) -> usize {
        self.seen.len()
    }
}

impl ModelPort for ScriptedModel {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        self.seen.push((view.turn().number(), view.trace().len()));
        self.script.pop_front().ok_or(scripted_model_failure())
    }
}

/// Run 级一次 + 每次 Turn 一次的注入来源。
struct OneShotSource {
    run_start: String,
    turn: String,
    /// 每次被调用时的（阶段, 轮次）。
    calls: Vec<(String, Option<u32>)>,
}

impl OneShotSource {
    fn new(run_start: &str, turn: &str) -> Self {
        Self {
            run_start: run_start.to_string(),
            turn: turn.to_string(),
            calls: Vec::new(),
        }
    }
}

impl ContextSource for OneShotSource {
    fn on_run_start(&mut self, context: &InjectionContext) -> ContentBatch {
        self.calls
            .push(("run-start".to_string(), context.turn().map(Turn::number)));
        ContentBatch::of(vec![ContentBlock::new("run", self.run_start.clone())])
            .expect("这批内容在上限内")
    }

    fn before_turn(&mut self, context: &InjectionContext) -> ContentBatch {
        self.calls
            .push(("turn".to_string(), context.turn().map(Turn::number)));
        ContentBatch::of(vec![ContentBlock::new("turn", self.turn.clone())])
            .expect("这批内容在上限内")
    }
}

/// 会记账的执行端：记录每次真的被执行的（动作 id, 参数个数），并给出固定结果。
struct CountingActions {
    /// 每次执行的（动作 id, 参数个数）。
    executed: Vec<(String, usize)>,
    outcome: ActionOutcome,
}

impl CountingActions {
    fn success(text: &str) -> Self {
        Self {
            executed: Vec::new(),
            outcome: ActionOutcome::success(vec![ContentBlock::new("result", text)]),
        }
    }
}

impl ActionPort for CountingActions {
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
        action: &Action,
    ) -> ActionOutcome {
        self.executed
            .push((action.id().as_str().to_string(), action.params().len()));
        self.outcome.clone()
    }
}

/// 让第 `fail_at` 次提交调用失败的接缝（1 起数）。`None` 表示永不失败。
///
/// 它把**每一次被调用**记下来，这样测试能断言"失败之后没有下一次调用"，
/// 以及"失败的那一步没有落地"。
struct FlakyCommit<'a> {
    env: &'a mut Environment,
    fail_at: Option<usize>,
    calls: Rc<RefCell<Vec<String>>>,
    counter: usize,
}

impl<'a> FlakyCommit<'a> {
    fn new(env: &'a mut Environment, fail_at: Option<usize>) -> Self {
        Self {
            env,
            fail_at,
            calls: Rc::new(RefCell::new(Vec::new())),
            counter: 0,
        }
    }

    /// 数一次调用；到点就失败，**在写盘之前**返回——失败意味着"这一步没被确认"。
    fn gate(&mut self, label: &str, stage: CommitStage) -> Result<(), CommitError> {
        self.counter += 1;
        self.calls.borrow_mut().push(label.to_string());
        if self.fail_at == Some(self.counter) {
            return Err(CommitError::at(stage));
        }
        Ok(())
    }
}

fn input_text(input: &RunInput) -> String {
    input.as_str().to_string()
}

impl CommitSeam for FlakyCommit<'_> {
    fn commit_run_start(&mut self, agent: &AgentId, run: &RunId) -> Result<(), CommitError> {
        self.gate("run-start", CommitStage::RunStart)?;
        if self.env.agent() != agent {
            return Err(CommitError::at(CommitStage::RunStart));
        }
        self.env
            .append_trace(run, TracePayload::RunStarted { origin: None })
            .map_err(|_| CommitError::at(CommitStage::RunStart))?;
        self.env
            .append_fact(
                run,
                &Record::RunStarted {
                    agent: agent.clone(),
                    run: run.clone(),
                },
            )
            .map_err(|_| CommitError::at(CommitStage::RunStart))?;
        Ok(())
    }

    fn commit_input(&mut self, run: &RunId, input: &RunInput) -> Result<(), CommitError> {
        self.gate("input", CommitStage::Input)?;
        self.env
            .append_trace(
                run,
                TracePayload::RunInput {
                    text: input_text(input),
                },
            )
            .map_err(|_| CommitError::at(CommitStage::Input))?;
        Ok(())
    }

    fn commit_trace(
        &mut self,
        run: &RunId,
        turn: Option<Turn>,
        entry: &TraceEntry,
    ) -> Result<(), CommitError> {
        self.gate("trace", CommitStage::Trace)?;
        self.env
            .append_trace(run, payload_of(entry, turn))
            .map_err(|_| CommitError::at(CommitStage::Trace))?;
        Ok(())
    }

    fn commit_action_selected(
        &mut self,
        run: &RunId,
        operation: &OperationId,
        action: &Action,
    ) -> Result<(), CommitError> {
        self.gate("selected", CommitStage::ActionSelected)?;
        self.env
            .append_trace(
                run,
                celira_agent::store::selected_payload(
                    operation,
                    action.id().as_str(),
                    action.params(),
                ),
            )
            .map_err(|_| CommitError::at(CommitStage::ActionSelected))?;
        self.env
            .append_fact(
                run,
                &Record::ActionPrepared {
                    run: run.clone(),
                    operation: operation.clone(),
                },
            )
            .map_err(|_| CommitError::at(CommitStage::ActionSelected))?;
        Ok(())
    }

    fn commit_action_dispatched(
        &mut self,
        run: &RunId,
        operation: &OperationId,
    ) -> Result<(), CommitError> {
        self.gate("dispatched", CommitStage::ActionDispatched)?;
        self.env
            .append_fact(
                run,
                &Record::ActionDispatched {
                    run: run.clone(),
                    operation: operation.clone(),
                },
            )
            .map_err(|_| CommitError::at(CommitStage::ActionDispatched))?;
        Ok(())
    }

    fn commit_action_result(
        &mut self,
        run: &RunId,
        operation: &OperationId,
        outcome: &ActionOutcome,
    ) -> Result<(), CommitError> {
        self.gate("result", CommitStage::ActionResult)?;
        self.env
            .append_fact(
                run,
                &Record::ActionResultRecorded {
                    run: run.clone(),
                    action: ActionResult::new(operation.clone(), outcome.state()),
                },
            )
            .map_err(|_| CommitError::at(CommitStage::ActionResult))?;
        Ok(())
    }

    fn commit_ending(
        &mut self,
        run: &RunId,
        outcome: Outcome,
        cost: celira_agent::cost::RunCost,
    ) -> Result<(), CommitError> {
        self.gate("ending", CommitStage::Ending)?;
        // 与真实适配器一致：收场**分两步**写（经历先、事实后）。替身也必须这样，
        // 否则"终态是否真的落盘"这类断言会测到替身的形状而不是生产的形状。
        self.env
            .append_trace(run, celira_agent::store::ending_payload(outcome))
            .map_err(|_| CommitError::at(CommitStage::Ending))?;
        self.env
            .append_fact(
                run,
                &Record::RunEnded {
                    run: run.clone(),
                    outcome,
                    cost,
                },
            )
            .map_err(|_| CommitError::at(CommitStage::Ending))?;
        Ok(())
    }

    fn seal(&mut self, run: &RunId) -> Result<(), CommitError> {
        self.gate("seal", CommitStage::Seal)?;
        self.env
            .append_trace(run, TracePayload::Sealed)
            .map_err(|_| CommitError::at(CommitStage::Seal))?;
        Ok(())
    }
}

/// 默认的"什么都提交、什么都成功"接缝：用真实环境，不注入失败。
type HealthyCommit<'a> = FlakyCommit<'a>;

/// 标准脚本：先选一个动作，再收场。
fn probe_then_finish() -> Vec<ModelOutput> {
    vec![
        ModelOutput::act(Action::with_params(
            ActionId::new("probe"),
            ActionParams::none().with("k", "v"),
        )),
        ModelOutput::finish(SubjectEnding::Silent),
    ]
}

// ---------------------------------------------------------------------------
// 1. 身份：创建 / 打开 / 拒绝
// ---------------------------------------------------------------------------

#[test]
fn creating_an_environment_records_a_stable_identity() {
    let dir = TempDir::new("identity-create");
    let agent = AgentId::new("agent-alpha");

    let env = Environment::create(&dir.env(), agent.clone()).expect("首次创建应成功");
    assert_eq!(env.agent(), &agent);
    assert!(
        env.root().join("identity.json").exists(),
        "身份记录必须落盘"
    );
    drop(env);

    // 再次"创建"同一个环境必须被拒绝：这是打开，不是创建，不静默采用也不重置。
    match Environment::create(&dir.env(), agent) {
        Err(StoreError::AlreadyOpen) => {}
        other => panic!("重复创建应明确拒绝，却得到 {other:?}"),
    }
}

/// **丢了身份的位置不会被"创建"清空**（2026-10-06 防静默销毁那一刀）。
///
/// 形状：一个有历史的环境（存储里真的写了东西）→ 身份记录被误删 → 有人再敲一次 `init`。
/// 从前那条路会把两份存储 `truncate` 掉——**同一个损坏形状，`open` 是拒绝、`create` 却是销毁**。
/// 现在它明确拒绝（`directory-not-empty`），并且**一个字节都不动**。
#[test]
fn a_lost_identity_is_never_recreated_over_live_history() {
    let dir = TempDir::new("lost-identity");
    let agent = AgentId::new("agent-lost-identity");
    {
        let mut env = Environment::create(&dir.env(), agent.clone()).expect("先建一个环境");
        env.append_trace(
            &RunId::new("run-1"),
            TracePayload::RunInput {
                text: "一行历史".into(),
            },
        )
        .expect("写一行经历");
    }

    // 身份记录被误删——这正是那条销毁路径的入口形状。
    std::fs::remove_file(dir.env().join("identity.json")).expect("删掉身份记录");
    let trace_before = std::fs::read(dir.env().join("trace.ndjson")).expect("读经历");
    let facts_before = std::fs::read(dir.env().join("facts.ndjson")).expect("读事实");
    assert!(!trace_before.is_empty(), "这一段要的是**有历史**的环境");

    let refused =
        Environment::create(&dir.env(), agent.clone()).expect_err("不许在有历史的位置上创建");
    assert_eq!(refused.as_str(), "history-without-identity");
    match &refused {
        StoreError::HistoryWithoutIdentity { files } => assert!(
            files.contains(&"trace.ndjson"),
            "拒绝要说出它看见了什么：{files:?}"
        ),
        other => panic!("要的是 history-without-identity，却得到 {other:?}"),
    }

    // **一个字节都不动**——这才是这一条的全部意义。
    assert_eq!(
        std::fs::read(dir.env().join("trace.ndjson")).expect("读"),
        trace_before
    );
    assert_eq!(
        std::fs::read(dir.env().join("facts.ndjson")).expect("读"),
        facts_before
    );
    // 身份没了，`open` 照旧如实拒绝：同一个损坏形状，两条路都不猜。
    assert_eq!(
        Environment::open(&dir.env(), agent)
            .expect_err("身份没了就打不开")
            .as_str(),
        "identity-missing"
    );
}

/// **判据只覆盖这一次操作自己的写集**：`create` 唯一会清空的是两份存储，因此"这个位置里有别的
/// 东西"**不构成**拒绝的理由——家侧按设计就把 `agent.conf` 写在环境根里（真机门禁 2026-10-06
/// 抓到过：按"任何东西在场就拒绝"写会让**每一个新家**都装不起来，也会把一个半成品家永久锁死）。
///
/// 同一支用例给出两面：① 只有家侧配置（陌生内容）⇒ **照常创建**，而且那份内容原样不动；
/// ② 空目录 ⇒ 照常创建。
#[test]
fn a_home_side_config_in_the_environment_root_does_not_block_creation() {
    let dir = TempDir::new("home-side-config");
    let env_dir = dir.env();
    std::fs::create_dir_all(&env_dir).expect("建立环境目录");
    std::fs::write(env_dir.join("agent.conf"), "slot=1\n").expect("放一份家侧配置");

    let created =
        Environment::create(&env_dir, AgentId::new("agent-x")).expect("家侧配置不该挡创建");
    assert_eq!(created.agent().as_str(), "agent-x");
    assert_eq!(
        std::fs::read_to_string(env_dir.join("agent.conf")).expect("读"),
        "slot=1\n",
        "创建不能动家侧那份配置"
    );

    // 空目录（哪怕锁文件刚被建出来）照常创建。
    let fresh = TempDir::new("empty-create");
    std::fs::create_dir_all(fresh.env()).expect("建立环境目录");
    let created =
        Environment::create(&fresh.env(), AgentId::new("agent-y")).expect("空目录可以创建");
    assert_eq!(created.agent().as_str(), "agent-y");
}

/// **读者读得到写者正在持有的环境**（2026-10-06 读锁争用那一刀）。
///
/// 症状：`agent_host_status` / `environment_diagnose` 从前都走 `Environment::open`，而它第一件事是
/// 取排他写资格——于是她正在跑一次活动时，状态页拿到的是 `already-open`：一个"看一眼处境"的命令，
/// **在最需要看处境的那一刻恰好读不到**。
///
/// 判据两面：① 写资格仍然**只有一个持有者**（第二个写者照旧 `already-open`）；
/// ② 读者**不取**写资格，照样读得到同一批记录。
#[test]
fn a_reader_reads_the_environment_while_a_writer_holds_it() {
    let dir = TempDir::new("reader-while-writing");
    let agent = AgentId::new("agent-reader");
    let mut writer = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    writer
        .append_trace(
            &RunId::new("run-1"),
            TracePayload::RunStarted { origin: None },
        )
        .expect("登记一段生命");
    writer
        .append_trace(
            &RunId::new("run-1"),
            TracePayload::RunInput {
                text: "一行历史".into(),
            },
        )
        .expect("写一行经历");

    // ① 写资格照旧排他：第二个写者被拒。
    assert_eq!(
        Environment::open(&dir.env(), agent.clone())
            .expect_err("写者只能有一个")
            .as_str(),
        "already-open"
    );

    // ② 读者不取写资格，因此**写者还在**的时候就读得到。
    let reader = EnvironmentReader::open(&dir.env(), agent.clone()).expect("读者应当打得开");
    assert_eq!(reader.agent(), &agent);
    assert_eq!(reader.root(), dir.env().as_path());
    assert_eq!(reader.trace().expect("读经历").len(), 2);
    assert!(reader.facts().expect("读事实").is_empty());
    // 判据与写者**同一份实现**：这份前缀里有一段登记过、还没收尾的生命。
    assert!(matches!(
        reader.admission().expect("准入读数"),
        celira_agent::store::Admission::Unfinished { .. }
    ));
    let health = reader.health().expect("健康自检");
    assert_eq!(health.trace_records(), 2);
    assert!(!health.is_paused());

    // 读者仍然**一格都写不了**：它没有写方法，也不持有任何追加句柄（类型，不是约定）。
    drop(reader);
    writer
        .append_trace(
            &RunId::new("run-1"),
            ending_payload(Outcome::Subject(SubjectEnding::Completed)),
        )
        .expect("写者照旧写得下去");
}

/// **读法由证据决定**：最后一段没有换行时——**有写者在写** ⇒ 那是它此刻写到一半的那一行，
/// 读已确认前缀；**没有写者** ⇒ 那是没人收尾的残留（崩在半路），照旧**明确失败**，
/// 不许报告成一份更短的健康历史（这条判据由 `environment_diagnostics` 那条 CLI 用例钉着）。
///
/// 两支并排，证明这不是"读者一律放宽"，而是**贴着证据的两种读法**。
#[test]
fn the_torn_tail_is_read_as_a_prefix_only_while_a_writer_is_active() {
    use celira_agent::store::read_trace_at;

    // ① **没有写者**：残缺尾部照旧是损坏（读者与写者同一结论）。
    let quiet = TempDir::new("torn-tail-quiet");
    let quiet_agent = AgentId::new("agent-quiet");
    {
        let mut env = Environment::create(&quiet.env(), quiet_agent.clone()).expect("创建环境");
        env.append_trace(
            &RunId::new("run-1"),
            TracePayload::RunInput {
                text: "已确认的那一条".into(),
            },
        )
        .expect("写一行");
    }
    let torn = "{\"seq\":1,\"run\":\"run-1\",\"kind\":\"run-inp";
    let quiet_trace = quiet.env().join("trace.ndjson");
    let mut raw = std::fs::read_to_string(&quiet_trace).expect("读回");
    raw.push_str(torn);
    std::fs::write(&quiet_trace, &raw).expect("写入残缺尾部");

    assert!(
        matches!(
            EnvironmentReader::open(&quiet.env(), quiet_agent.clone()),
            Err(StoreError::CorruptTail { .. })
        ),
        "没人写的时候，残缺尾部是崩溃残留，不是'写了一半'"
    );
    assert!(
        matches!(
            read_trace_at(&quiet.env()),
            Err(StoreError::CorruptTail { .. })
        ),
        "按目录读也一样：读法看证据，不看是哪个入口"
    );

    // ② **有写者在写**：同一种残缺尾部读成已确认前缀（写者写完才 sync，那一段还不算事实）。
    let busy = TempDir::new("torn-tail-busy");
    let busy_agent = AgentId::new("agent-busy");
    let mut writer = Environment::create(&busy.env(), busy_agent.clone()).expect("创建环境");
    writer
        .append_trace(
            &RunId::new("run-1"),
            TracePayload::RunInput {
                text: "已确认的那一条".into(),
            },
        )
        .expect("写一行");
    let busy_trace = busy.env().join("trace.ndjson");
    let mut raw = std::fs::read_to_string(&busy_trace).expect("读回");
    raw.push_str(torn);
    std::fs::write(&busy_trace, &raw).expect("模拟写者此刻写到一半");

    let reader = EnvironmentReader::open(&busy.env(), busy_agent.clone()).expect("读者应当打得开");
    assert_eq!(
        reader.trace().expect("读已确认前缀").len(),
        1,
        "写者写到一半的那一段不是事实"
    );
    assert_eq!(
        read_trace_at(&busy.env()).expect("目录读法").len(),
        1,
        "同一条证据，两个入口同一结论"
    );

    // ⚠ 两种读法**都不放宽内容校验**：完整的一行读不出来照样拒绝，谁都不许"跳过去继续读"。
    std::fs::write(
        &busy_trace,
        "{\"seq\":0,\"run\":\"run-1\",\"kind\":\"run-inp\"}\n",
    )
    .expect("写一条完整但读不出来的行");
    assert!(matches!(
        read_trace_at(&busy.env()),
        Err(StoreError::CorruptTail { .. })
    ));
    let reader = EnvironmentReader::open(&busy.env(), busy_agent);
    assert!(
        matches!(reader, Err(StoreError::CorruptTail { .. })),
        "读者只放宽'最后一段没写完'，不放宽内容：{reader:?}"
    );
}

/// **开场读数**（2026-10-06）：准入与"历史上最后一次存储故障"从**同一份已确认前缀**里出来
/// （一次解析——那份账本真机上读到过 11.3 秒，各读一遍就是白读一遍）。
///
/// 钉住三条判据：① 没停过 ⇒ 那一格是 `None`；② 停过 ⇒ 哪一段、什么原因；③ 确认**不重写原因**
/// （它说的是"看过了"），因此确认之后原因照旧读得出来，只有"被看过"那一格翻过来。
#[test]
fn the_opening_reading_carries_the_last_storage_failure() {
    let dir = TempDir::new("opening-readings");
    let agent = AgentId::new("agent-readings");
    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");

    // ① 没停过：准入 Ready，故障那一格空着。
    let clean = env.opening_readings().expect("开场读数");
    assert_eq!(clean.admission, Admission::Ready);
    assert_eq!(clean.last_storage_failure, None);

    // ② 停过：哪一段、什么原因、还没被看过。
    let broken = RunId::new("run-broken");
    env.mark_paused(&broken, "save-failed:run-start")
        .expect("写下暂停");
    let paused = env.opening_readings().expect("开场读数");
    assert_eq!(
        paused.admission,
        Admission::Paused {
            reason: "save-failed:run-start".to_string()
        }
    );
    assert_eq!(
        paused.last_storage_failure,
        Some(celira_agent::store::StorageFailure {
            run: broken.clone(),
            reason: Some("save-failed:run-start".to_string()),
            acknowledged: false,
        })
    );

    // ③ 显式确认：准入回到 Ready，而"停在哪一段、为什么"照旧读得出来，只有"被看过"翻过来。
    celira_agent::store::clear_paused(&mut env, &broken).expect("显式确认");
    let cleared = env.opening_readings().expect("开场读数");
    assert_eq!(cleared.admission, Admission::Ready);
    assert_eq!(
        cleared.last_storage_failure,
        Some(celira_agent::store::StorageFailure {
            run: broken.clone(),
            reason: Some("save-failed:run-start".to_string()),
            acknowledged: true,
        })
    );

    // ④ 又停过一次（另一段、另一个原因）⇒ 报**最近**那一次，而且"被看过"重新归零。
    let broken_again = RunId::new("run-broken-again");
    env.mark_paused(&broken_again, "save-failed:static-head")
        .expect("再写一次暂停");
    let again = env.opening_readings().expect("开场读数");
    assert_eq!(
        again.last_storage_failure,
        Some(celira_agent::store::StorageFailure {
            run: broken_again,
            reason: Some("save-failed:static-head".to_string()),
            acknowledged: false,
        })
    );

    // 读者那一侧同一份判据（它不取写资格，因此这条读数在写者持有时也读得到）。
    let reader = EnvironmentReader::open(&dir.env(), agent).expect("读者");
    assert_eq!(
        reader.opening_readings().expect("开场读数"),
        again,
        "写者与读者同一份判据"
    );
}

/// 读者的拒绝面与写者**同一份判据**：身份不符、位置不存在、存储缺一份，一律明确拒绝。
#[test]
fn a_reader_refuses_exactly_what_a_writer_refuses() {
    use celira_agent::store::EnvironmentReader;

    let dir = TempDir::new("reader-refusals");
    Environment::create(&dir.env(), AgentId::new("agent-owner")).expect("创建环境");

    assert_eq!(
        EnvironmentReader::open(&dir.env(), AgentId::new("agent-other"))
            .expect_err("身份不符不许读")
            .as_str(),
        "identity-mismatch"
    );
    assert_eq!(
        EnvironmentReader::open(&dir.env().join("nowhere"), AgentId::new("agent-owner"))
            .expect_err("位置不存在")
            .as_str(),
        "not-found"
    );
    std::fs::remove_file(dir.env().join("facts.ndjson")).expect("删掉一份存储");
    assert_eq!(
        EnvironmentReader::open(&dir.env(), AgentId::new("agent-owner"))
            .expect_err("存储缺一份是损坏")
            .as_str(),
        "storage-missing"
    );
}

#[test]
fn opening_with_a_different_identity_is_refused() {
    let dir = TempDir::new("identity-mismatch");
    Environment::create(&dir.env(), AgentId::new("agent-alpha")).expect("创建成功");

    match Environment::open(&dir.env(), AgentId::new("agent-beta")) {
        Err(StoreError::IdentityMismatch { expected, found }) => {
            assert_eq!(expected.as_str(), "agent-beta");
            assert_eq!(found.as_str(), "agent-alpha");
        }
        other => panic!("身份不符必须明确拒绝，却得到 {other:?}"),
    }
}

#[test]
fn opening_a_missing_or_corrupt_identity_is_refused_without_creating_one() {
    let dir = TempDir::new("identity-missing");

    // 目录都不存在：不是"顺手建一个"，是明确找不到。
    match Environment::open(&dir.env(), AgentId::new("agent-alpha")) {
        Err(StoreError::NotFound) => {}
        other => panic!("目录不存在应明确报缺失，却得到 {other:?}"),
    }
    assert!(!dir.env().exists(), "失败的打开不得顺手建目录");

    // 身份记录损坏：明确报损坏，不静默生成新身份。
    std::fs::create_dir_all(dir.env()).expect("建目录");
    std::fs::write(dir.env().join("identity.json"), "{ 这不是合法 JSON").expect("写坏身份");
    match Environment::open(&dir.env(), AgentId::new("agent-alpha")) {
        Err(StoreError::IdentityCorrupt) => {}
        other => panic!("身份损坏应报损坏，却得到 {other:?}"),
    }
    // 原始损坏内容必须原地保留：拒绝不等于重置。
    let raw = std::fs::read_to_string(dir.env().join("identity.json")).expect("读回");
    assert!(raw.contains("这不是合法 JSON"), "损坏内容不得被覆盖掉");
}

#[test]
fn a_second_opener_of_the_same_environment_is_refused() {
    let dir = TempDir::new("exclusive-open");
    let agent = AgentId::new("agent-alpha");

    let first = Environment::create(&dir.env(), agent.clone()).expect("创建成功");
    match Environment::open(&dir.env(), agent.clone()) {
        Err(StoreError::AlreadyOpen) => {}
        other => panic!("同一环境不得形成两个写者，却得到 {other:?}"),
    }
    // 第一个写者仍然有效：排他不是"把前一个挤掉"。
    assert_eq!(first.agent(), &agent);

    // 释放之后可以正常再打开。
    drop(first);
    Environment::open(&dir.env(), agent).expect("写者释放后应能打开");
}

// ---------------------------------------------------------------------------
// 2. 增量确认点：每一步都在下一次模型调用之前落盘
// ---------------------------------------------------------------------------

/// 一次活动：Run 级注入一次、Turn 级注入两次、执行一个动作、然后收场。
#[test]
fn every_checkpoint_lands_before_the_next_model_call() {
    let dir = TempDir::new("checkpoints");
    let agent = AgentId::new("agent-alpha");
    let run = RunId::new("run-1");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建成功");

    let mut source = OneShotSource::new("开场注入", "本轮注入");
    let mut model = ScriptedModel::new(probe_then_finish());
    let mut actions = CountingActions::success("踩到的结果");
    let mut commit = HealthyCommit::new(&mut env, None);
    let calls = commit.calls.clone();

    let mut agent_state = Agent::new(agent.clone());
    let mut ledger = Ledger::new(agent.clone());
    let run_result = run_activity_with(
        &mut agent_state,
        RunRequest {
            cost: &no_cost,
            run: run.clone(),
            limits: Limits::new(8),
            input: RunInput::text("初始输入").expect("输入在上限内"),
            source: &mut source,
            commit: &mut commit,
        },
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("这次活动应正常收场");
    assert_eq!(run_result.id(), &run);
    drop(commit);

    // 模型被调用了两次（先用一个动作，再决定收场）。
    assert_eq!(model.calls(), 2, "应当只有两次模型调用");
    // 执行端只被执行了一次：`Finish` 不会再发动作。
    assert_eq!(actions.executed.len(), 1, "只应有一次真的执行");
    assert_eq!(actions.executed[0].0, "probe");
    assert_eq!(actions.executed[0].1, 1, "那一个参数也必须在确认点里");

    // 注入接缝的时机没变：Run 级恰好一次（没有轮次），Turn 级每轮一次。
    assert_eq!(
        source.calls,
        vec![
            ("run-start".to_string(), None),
            ("turn".to_string(), Some(1)),
            ("turn".to_string(), Some(2)),
        ],
        "两个注入接缝的时机必须保持不变"
    );

    // 确认顺序必须与发生的顺序一致，且每一步都出现在下一步之前。
    let calls = calls.borrow().clone();
    assert_eq!(
        calls,
        vec![
            "run-start",  // 活动登记
            "input",      // 开场输入
            "trace",      // Run 级注入
            "trace",      // Turn 1 注入
            "trace",      // 首轮响应（先于它请求的行动被确认）
            "selected",   // 执行前的选择确认
            "dispatched", // 动作已发出
            "result",     // 真实结果（事实侧三分状态）
            "trace",      // 结果正文作为观察进入经历存储
            "trace",      // Turn 2 注入
            "trace",      // 次轮响应（主体收场）
            "ending",     // 终态
            "seal",       // 封存
        ],
        "确认点顺序错误"
    );

    // 盘上必须能看到全部关键经历与事实。
    let kinds: Vec<String> = env
        .trace()
        .expect("读回经历")
        .iter()
        .map(|record| record.entry().kind().to_string())
        .collect();
    for required in [
        "run-start",
        "run-input",
        "injection",
        "action-selected",
        "observed",
        "ending",
        "sealed",
    ] {
        assert!(
            kinds.contains(&required.to_string()),
            "经历里缺少 {required}：{kinds:?}"
        );
    }

    let fact_kinds: Vec<String> = env
        .facts()
        .expect("读回事实")
        .iter()
        .map(|row| row.fact().to_string())
        .collect();
    for required in [
        "run-started",
        "action-prepared",
        "action-dispatched",
        "action-result",
    ] {
        assert!(
            fact_kinds.contains(&required.to_string()),
            "事实里缺少 {required}：{fact_kinds:?}"
        );
    }

    // 这次活动是完整收场的：有终态、有封存。
    let runs = list_runs(&env).expect("列出活动");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status(), "complete", "{:?}", runs[0]);
    assert!(runs[0].is_complete());
    assert!(runs[0].is_sealed());
}

/// 开场输入必须在**第一次模型调用之前**被确认，而不是只在最后导出。
#[test]
fn the_input_is_confirmed_before_the_first_model_call() {
    let dir = TempDir::new("input-before-model");
    let agent = AgentId::new("agent-alpha");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建成功");

    let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
    let mut actions = CountingActions::success("未用到");
    let mut commit = HealthyCommit::new(&mut env, None);
    let calls = commit.calls.clone();
    {
        let mut agent_state = Agent::new(agent.clone());
        let mut ledger = Ledger::new(agent.clone());
        run_activity_with(
            &mut agent_state,
            RunRequest {
                cost: &no_cost,
                run: RunId::new("run-input"),
                limits: Limits::new(8),
                input: RunInput::text("这是一段必须被确认的输入").expect("在上限内"),
                source: &mut celira_agent::core::NoContext,
                commit: &mut commit,
            },
            &mut model,
            &mut actions,
            &mut ledger,
        )
        .expect("应正常收场");
    }

    let calls = calls.borrow().clone();
    assert_eq!(calls[..2], ["run-start".to_string(), "input".to_string()]);
    assert!(
        calls.contains(&"ending".to_string()),
        "终态也必须被确认：{calls:?}"
    );
    assert!(
        calls.contains(&"seal".to_string()),
        "封存也必须被确认：{calls:?}"
    );

    // 输入原文必须可从盘上读回。
    let trace = env.trace().expect("读回经历");
    let input = trace
        .iter()
        .find_map(|record| match record.entry() {
            TracePayload::RunInput { text } => Some(text.clone()),
            _ => None,
        })
        .expect("必须有输入记录");
    assert_eq!(input, "这是一段必须被确认的输入");
}

// ---------------------------------------------------------------------------
// 3. 保存失败：停止新的模型调用与新的行动尝试，但不重发、不伪造终态
// ---------------------------------------------------------------------------

/// 在**每一个**确认点逐一注入失败，断言：
///
/// - 返回的错误是"保存失败"，不是模型失败、不是主体完成；
/// - 失败点**之后**的模型调用与行动尝试都没有发生；
/// - 失败点**之前**已确认的前缀原样保留（不被改写、不被丢弃）。
#[test]
fn a_save_failure_stops_the_next_model_call_and_action_attempt() {
    // 每个点都试一遍：1..=13 对应上面那条"确认顺序"里的 13 个确认点。
    // 其中第 5 与第 11 个 `trace` 是两次**模型响应**的确认点——响应先于它请求的行动被确认。
    let expected_order = [
        "run-start",  // 1  活动登记
        "input",      // 2  开场输入
        "trace",      // 3  Run 级注入
        "trace",      // 4  Turn 1 注入
        "trace",      // 5  首轮响应
        "selected",   // 6  执行前的选择确认
        "dispatched", // 7  动作已发出
        "result",     // 8  真实结果
        "trace",      // 9  结果正文进入经历存储
        "trace",      // 10 Turn 2 注入
        "trace",      // 11 次轮响应（主体收场）
        "ending",     // 12 终态
        "seal",       // 13 封存
    ];

    for fail_at in 1..=expected_order.len() {
        let dir = TempDir::new(&format!("fail-at-{fail_at}"));
        let agent = AgentId::new("agent-alpha");
        let run = RunId::new("run-1");

        let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建成功");
        let mut source = OneShotSource::new("开场注入", "本轮注入");
        let mut model = ScriptedModel::new(probe_then_finish());
        let mut actions = CountingActions::success("踩到的结果");
        let mut commit = FlakyCommit::new(&mut env, Some(fail_at));
        let calls = commit.calls.clone();

        let mut agent_state = Agent::new(agent.clone());
        let mut ledger = Ledger::new(agent.clone());
        let result = run_activity_with(
            &mut agent_state,
            RunRequest {
                cost: &no_cost,
                run: run.clone(),
                limits: Limits::new(8),
                input: RunInput::text("初始输入").expect("在上限内"),
                source: &mut source,
                commit: &mut commit,
            },
            &mut model,
            &mut actions,
            &mut ledger,
        );
        let actual_calls = calls.borrow().clone();
        drop(commit);

        // 保存失败必须被如实报成保存失败——它不是模型失败，也不是主体完成。
        match result {
            Err(celira_agent::core::RunError::Commit(_)) => {}
            other => panic!(
                "第 {fail_at} 个确认点失败时应报 Commit，却得到 {other:?}（调用序列 {actual_calls:?}）"
            ),
        }

        // 确认序列必须**恰好停在**失败点上：不继续、也不重试。
        assert_eq!(
            actual_calls,
            expected_order[..fail_at].to_vec(),
            "第 {fail_at} 个确认点失败之后不得再有确认调用"
        );

        // 模型与执行端的调用次数必须与"走到哪一步"一致：
        // 前 4 个点都排在第一次模型调用之前，所以模型 0 次、执行端 0 次。
        let model_calls = model.calls();
        let action_calls = actions.executed.len();
        match fail_at {
            // run-start / input / trace / trace：还没到模型
            1..=4 => {
                assert_eq!(model_calls, 0, "第 {fail_at} 点：模型不该被调用");
                assert_eq!(action_calls, 0, "第 {fail_at} 点：不该有行动尝试");
            }
            // 响应确认：模型**已经**作答，但这一次响应没有被确认，
            // 所以它请求的行动绝不能发出——"响应先确认、再谈它请求了什么"。
            5 => {
                assert_eq!(model_calls, 1, "第 {fail_at} 点：模型应恰好被调用一次");
                assert_eq!(
                    action_calls, 0,
                    "第 {fail_at} 点：响应未确认，它请求的行动不得发出"
                );
            }
            // selected：模型已作答一次，动作还没发出
            6 => {
                assert_eq!(model_calls, 1, "第 {fail_at} 点：模型应恰好被调用一次");
                assert_eq!(
                    action_calls, 0,
                    "第 {fail_at} 点：选择确认失败，动作不得发出"
                );
            }
            // dispatched：这是"尝试已在途"的确认，排在**执行之前**——
            // 它失败，动作就根本不该被执行，更谈不上重发。
            7 => {
                assert_eq!(model_calls, 1);
                assert_eq!(
                    action_calls, 0,
                    "第 {fail_at} 点：在途确认失败，动作不得被执行"
                );
            }
            // result / trace：动作已执行、外部效果可能已发生，但结果没有被确认
            8 | 9 => {
                assert_eq!(
                    model_calls, 1,
                    "第 {fail_at} 点：结果未确认，不得进入下一轮"
                );
                assert_eq!(action_calls, 1, "第 {fail_at} 点：动作已经执行过一次");
            }
            // 第 10 个点是**第二轮注入**的确认：它排在第二次模型调用之前，所以此刻
            // 模型还只被调用过一次——确认先于下一步，这正是要证明的顺序。
            10 => {
                assert_eq!(
                    model_calls, 1,
                    "第 {fail_at} 点：注入确认排在第二次模型调用之前"
                );
                assert_eq!(action_calls, 1, "第 {fail_at} 点：不得重发同一个动作");
            }
            // 次轮响应确认：第二次模型调用已经发生（它选择了收场），只是响应未确认
            11 => {
                assert_eq!(model_calls, 2, "第 {fail_at} 点：模型应已进入第二轮");
                assert_eq!(action_calls, 1, "第 {fail_at} 点：不得重发同一个动作");
            }
            // ending / seal：收场还没被确认
            12..=13 => {
                assert_eq!(model_calls, 2, "第 {fail_at} 点：模型应已进入第二轮");
                assert_eq!(action_calls, 1, "第 {fail_at} 点：不得重发同一个动作");
            }
            _ => unreachable!(),
        }

        // 已确认前缀必须原样在盘上：确认过的种类不多不少。
        let kinds: Vec<String> = env
            .trace()
            .expect("读回经历")
            .iter()
            .map(|record| record.entry().kind().to_string())
            .collect();
        // 第 13 个点（seal）失败时，ending 必须已经落盘——"活动已结束"与"封存未确认"是两件事。
        if fail_at == 13 {
            assert!(
                kinds.contains(&"ending".to_string()),
                "封存失败时终态必须已在盘上：{kinds:?}"
            );
            assert!(
                !kinds.contains(&"sealed".to_string()),
                "封存失败时不得出现封存记录：{kinds:?}"
            );
        }
        // 无论如何都不得出现"封存"却"没有终态"这种倒挂。
        if kinds.contains(&"sealed".to_string()) {
            assert!(kinds.contains(&"ending".to_string()), "有封存就必须有终态");
        }
    }
}

/// 保存失败之后，**已经发出**的动作不会被重发：重开一个新 Run 也不会自动补发。
#[test]
fn a_dispatched_action_is_never_resent_after_a_save_failure() {
    let dir = TempDir::new("no-resend");
    let agent = AgentId::new("agent-alpha");
    let run = RunId::new("run-1");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建成功");
    // 第 8 个点（result）失败：动作已经执行、外部效果可能已发生，但结果没被确认。
    let mut source = OneShotSource::new("", "");
    let mut model = ScriptedModel::new(probe_then_finish());
    let mut actions = CountingActions::success("外部效果");
    let mut commit = FlakyCommit::new(&mut env, Some(8));
    {
        let mut agent_state = Agent::new(agent.clone());
        let mut ledger = Ledger::new(agent.clone());
        let result = run_activity_with(
            &mut agent_state,
            RunRequest {
                cost: &no_cost,
                run: run.clone(),
                limits: Limits::new(8),
                input: RunInput::empty(),
                source: &mut source,
                commit: &mut commit,
            },
            &mut model,
            &mut actions,
            &mut ledger,
        );
        assert!(matches!(
            result,
            Err(celira_agent::core::RunError::Commit(_))
        ));
    }
    assert_eq!(actions.executed.len(), 1, "动作发出了一次");

    // 重开环境：上一次的"已发出但结果未确认"必须如实可见，且系统不会替它补发。
    // 必须把上一个写者（连同它持有的排他锁）一起释放——这正是"同一环境只允许一个写者"。
    drop(commit);
    drop(env);
    let mut reopened = Environment::open(&dir.env(), agent.clone()).expect("重开应成功");
    let facts = reopened.facts().expect("读回事实");
    let has_dispatched = facts.iter().any(|row| row.fact() == "action-dispatched");
    let has_result = facts.iter().any(|row| row.fact() == "action-result");
    assert!(has_dispatched, "已发出的动作必须在盘上：{facts:?}");
    assert!(!has_result, "结果未被确认，就不该有结果事实");

    // 新 Run 用一个全新的执行端：如果系统偷偷重发，这个执行端就会被打到。
    let mut fresh_actions = CountingActions::success("不该发生");
    let mut fresh_model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
    let mut commit2 = FlakyCommit::new(&mut reopened, None);
    {
        let mut agent_state = Agent::new(agent.clone());
        let mut ledger = Ledger::new(agent.clone());
        run_activity_with(
            &mut agent_state,
            RunRequest {
                cost: &no_cost,
                run: RunId::new("run-2"),
                limits: Limits::new(8),
                input: RunInput::empty(),
                source: &mut celira_agent::core::NoContext,
                commit: &mut commit2,
            },
            &mut fresh_model,
            &mut fresh_actions,
            &mut ledger,
        )
        .expect("新活动应正常收场");
    }
    assert!(
        fresh_actions.executed.is_empty(),
        "重开之后系统不得替上一个活动重发任何动作：{:?}",
        fresh_actions.executed
    );
}

// ---------------------------------------------------------------------------
// 4. 损坏尾部：被检测到，不冒充完整成功
// ---------------------------------------------------------------------------

#[test]
fn a_corrupt_tail_is_reported_instead_of_being_silently_dropped() {
    let dir = TempDir::new("corrupt-tail");
    let agent = AgentId::new("agent-alpha");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建成功");
    env.append_trace(
        &RunId::new("run-1"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("写一条");
    drop(env);

    // 追加一段"写了一半"的尾部：没有结尾换行，也不是合法 JSON。
    let trace_path = dir.env().join("trace.ndjson");
    let mut raw = std::fs::read_to_string(&trace_path).expect("读回");
    raw.push_str("{\"seq\":1,\"run\":\"run-1\",\"kind\":\"run-inp");
    std::fs::write(&trace_path, &raw).expect("写入残缺尾部");

    // 残缺尾部必须被检测到并**拒绝**：宁可打不开，也不能带着不自洽的前缀继续追加。
    match Environment::open(&dir.env(), agent.clone()) {
        Err(StoreError::CorruptTail { seq }) => {
            assert_eq!(seq, 1, "应指出残缺的是第几条");
        }
        other => panic!("残缺尾部必须被检测到并拒绝，却得到 {other:?}"),
    }

    // 拒绝不等于"顺手把坏行删掉"：文件必须原地不动，人才能去看、去修。
    let after = std::fs::read_to_string(&trace_path).expect("读回");
    assert_eq!(after, raw, "检测到损坏不得改写文件");
}

// ---------------------------------------------------------------------------
// 5. 重开后的诚实呈现：身份、前缀、操作状态各自分开
// ---------------------------------------------------------------------------

/// 一个"活动已开始、但没到终态就断了"的环境，重开之后必须被如实标成中断，
/// 而不是被说成完成，也不是自动续跑。
#[test]
fn an_abnormally_ended_run_is_reported_as_interrupted_and_never_continued_by_itself() {
    let dir = TempDir::new("interrupted");
    let agent = AgentId::new("agent-alpha");
    let run = RunId::new("run-1");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建成功");
    // 第 3 个点失败：活动已登记、输入已确认、Run 注入已确认，然后就断了。
    let mut source = OneShotSource::new("开场注入", "");
    let mut model = ScriptedModel::new(probe_then_finish());
    let mut actions = CountingActions::success("不该走到这");
    let mut commit = FlakyCommit::new(&mut env, Some(3));
    {
        let mut agent_state = Agent::new(agent.clone());
        let mut ledger = Ledger::new(agent.clone());
        let result = run_activity_with(
            &mut agent_state,
            RunRequest {
                cost: &no_cost,
                run: run.clone(),
                limits: Limits::new(8),
                input: RunInput::text("初始输入").expect("在上限内"),
                source: &mut source,
                commit: &mut commit,
            },
            &mut model,
            &mut actions,
            &mut ledger,
        );
        assert!(matches!(
            result,
            Err(celira_agent::core::RunError::Commit(_))
        ));
    }
    drop(commit);

    // 模拟"上一个写者没来得及留下暂停标记就异常退出"：这里连暂停标记都没有。
    // 上一个写者必须先释放排他锁，才能由新的写者重新打开。
    drop(env);
    let env = Environment::open(&dir.env(), agent.clone()).expect("重开应成功");

    // 不伪造完成：状态必须是中断，不是完整。
    let runs = list_runs(&env).expect("列出活动");
    assert_eq!(runs.len(), 1);
    assert_eq!(
        runs[0].status(),
        "interrupted",
        "异常结束的活动必须如实呈现为中断：{:?}",
        runs[0]
    );
    assert!(!runs[0].is_complete());
    assert!(runs[0].has_start());
    assert!(runs[0].ending().is_none(), "中断的活动不得有终态");
    assert!(!runs[0].is_sealed());

    // 三个事实必须**分开**呈现：身份在、前缀到哪、操作结果是否确认。
    let health = env.health().expect("环境健康读数");
    assert_eq!(health.agent(), &agent, "身份是独立的一格");
    assert!(health.trace_records() > 0, "已确认的经历前缀是可读的");
    assert_eq!(
        health.interrupted().len(),
        1,
        "中断的活动必须能被单独列出来：{health:?}"
    );

    // 没有任何自动续跑的入口：读活动不会推进任何东西。
    let before = env.trace_records();
    let _ = env.read_run(&run, None, 50).expect("只读一段");
    let _ = env.fact_view().expect("只读事实视图");
    assert_eq!(env.trace_records(), before, "只读操作不得推进任何序号");
}

/// "活动已结束但封存未确认"必须与"活动被中断"区分开。
#[test]
fn an_unsealed_ending_is_distinguished_from_an_interrupted_activity() {
    let dir = TempDir::new("ended-unsealed");
    let agent = AgentId::new("agent-alpha");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建成功");
    // 第 7 个点（seal）失败：活动有终态，只是封存没确认。
    let mut source = OneShotSource::new("", "");
    let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
    let mut actions = CountingActions::success("未用到");
    // 只收场、不动手：确认序列是 run-start → input → trace（Run 注入）→ trace（Turn 注入）
    // → trace（响应）→ ending → seal，第 7 个点才是封存。
    let mut commit = FlakyCommit::new(&mut env, Some(7));
    {
        let mut agent_state = Agent::new(agent.clone());
        let mut ledger = Ledger::new(agent.clone());
        let result = run_activity_with(
            &mut agent_state,
            RunRequest {
                cost: &no_cost,
                run: RunId::new("run-1"),
                limits: Limits::new(8),
                input: RunInput::empty(),
                source: &mut source,
                commit: &mut commit,
            },
            &mut model,
            &mut actions,
            &mut ledger,
        );
        assert!(matches!(
            result,
            Err(celira_agent::core::RunError::Commit(_))
        ));
    }
    drop(commit);
    drop(env);

    let reopened = Environment::open(&dir.env(), agent.clone()).expect("重开");
    let runs = list_runs(&reopened).expect("列出活动");
    assert_eq!(runs.len(), 1);
    assert_eq!(
        runs[0].status(),
        "ended-unsealed",
        "有终态但没封存，必须与中断分开：{:?}",
        runs[0]
    );
    assert!(runs[0].ending().is_some(), "终态必须在");
    assert!(!runs[0].is_sealed(), "封存确实没确认");
    // 它**不是**中断：活动是自己结束的，只是历史封存未确认。
    assert!(
        !runs[0].status().eq("interrupted"),
        "不能把'封存未确认'笼统写成'活动被中断'"
    );
}

/// 暂停标记是**一条真实记录**，重开时能被读到；只有显式确认才算数。
#[test]
fn a_pause_mark_survives_reopen_and_is_only_cleared_explicitly() {
    let dir = TempDir::new("pause-mark");
    let agent = AgentId::new("agent-alpha");
    let run = RunId::new("run-1");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建成功");
    env.append_trace(&run, TracePayload::RunStarted { origin: None })
        .expect("写一条");
    env.mark_paused(&run, "storage-failure")
        .expect("留下暂停标记");
    drop(env);

    // 重开之后，暂停原因必须还在——重启不得绕过这层保护。
    let mut reopened = Environment::open(&dir.env(), agent.clone()).expect("重开");
    assert_eq!(
        paused_reason(reopened.root())
            .expect("读暂停原因")
            .as_deref(),
        Some("storage-failure"),
        "重启不得绕过暂停保护"
    );

    // 只有显式确认才算数，而且确认本身也是一条记录。
    celira_agent::store::clear_paused(&mut reopened, &run).expect("显式清除");
    assert!(
        paused_reason(reopened.root()).expect("读回").is_none(),
        "显式清除之后不应再有暂停原因"
    );
}

/// **暂停标记的标签是封闭词表**：认不出的取值（包括旧构建写下的 `resumed`）**明确拒绝**，
/// 不许静默读成"暂停、原因未说明"。
///
/// ⚠ **旧规矩 → 新规矩**（2026-09-26）：清除标记的取值从 `resumed` 改成 `acknowledged`——
/// `resume` 在本仓库已特指刚退役的那条路（跨进程继续同一段活动），而这个标记说的是
/// "**这条异常已经被看过了**"。从前 `paused_reason` 认不出就落到 `Some("unspecified")`，
/// 于是**一个已经被显式确认过的环境会被读成又一次暂停**（旧取值正好落进那一支，读出来是反的）。
/// 现在它是一条拒绝：持久字段上的词表一律 fail-closed（与 `unknown-failure-class` 同一条纪律）。
#[test]
fn an_unknown_pause_marker_is_refused_instead_of_being_read_as_paused() {
    let dir = TempDir::new("unknown-pause-marker");
    let agent = AgentId::new("agent-alpha");
    let run = RunId::new("run-1");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建成功");
    env.append_trace(&run, TracePayload::RunStarted { origin: None })
        .expect("写一条");
    drop(env);

    let trace_path = dir.env().join("trace.ndjson");
    // 第一条是那条 `run-start`（序号从 0 起），暂停标记跟在它后面。
    let first = confirmed_lines_of(&trace_path)[0].clone();
    let marker = |label: &str| {
        format!(
            r#"{{"seq":1,"run":"run-1","kind":"injection","stage":"storage-failure","turn":null,"blocks":[{{"label":"{label}","text":"explicitly cleared"}}]}}"#
        )
    };

    // 对照（先跑，证明这条拒绝不是"一律拒绝"）：**当前**取值读得出来，而且读成"没有暂停原因"。
    overwrite(
        &trace_path,
        &format!("{first}\n{}\n", marker("acknowledged")),
    );
    assert_eq!(
        paused_reason(&dir.env()).expect("当前取值必须读得出来"),
        None,
        "acknowledged = 这条异常已经被确认过"
    );

    // 旧构建写下的 `resumed`，以及任何词表之外的取值：**拒绝**。
    for label in ["resumed", "cleared", "paused-x"] {
        overwrite(&trace_path, &format!("{first}\n{}\n", marker(label)));
        assert_eq!(
            paused_reason(&dir.env()),
            Err(StoreError::UnknownPauseMarker { seq: 1 }),
            "{label}：词表之外的取值必须被拒绝，不许读成「暂停、原因未说明」"
        );
        // 准入那条路同样拒绝：它先读暂停标记，再谈未收尾——不许绕过。
        let environment = Environment::open(&dir.env(), agent.clone()).expect("打开环境");
        assert_eq!(
            environment.admission(),
            Err(StoreError::UnknownPauseMarker { seq: 1 }),
            "{label}：准入不许绕过这条拒绝"
        );
    }
}

// ---------------------------------------------------------------------------
// 6. 注入提交：每条**新增**经历恰好一次
// ---------------------------------------------------------------------------

/// 按给定脚本返回批次的来源：可以把"非空 Run 注入"与"空 Turn 批次"自由组合。
///
/// `None` 表示**空批次**（`ContentBatch::empty()`），与"一个内容为空的块"不同：后者仍是
/// 一条真实新增的记录。这里要考的就是前者。
struct ScriptedSource {
    run_start: Option<&'static str>,
    turns: Vec<Option<&'static str>>,
    calls: Vec<(String, Option<u32>)>,
}

impl ScriptedSource {
    fn new(run_start: Option<&'static str>, turns: Vec<Option<&'static str>>) -> Self {
        Self {
            run_start,
            turns,
            calls: Vec::new(),
        }
    }
}

impl ContextSource for ScriptedSource {
    fn on_run_start(&mut self, context: &InjectionContext) -> ContentBatch {
        self.calls
            .push(("run-start".to_string(), context.turn().map(Turn::number)));
        match self.run_start {
            Some(text) => ContentBatch::of(vec![ContentBlock::new("run", text)]).expect("在上限内"),
            None => ContentBatch::empty(),
        }
    }

    fn before_turn(&mut self, context: &InjectionContext) -> ContentBatch {
        let turn = context.turn().map(Turn::number);
        self.calls.push(("turn".to_string(), turn));
        let index = turn.map(|number| (number - 1) as usize).unwrap_or(0);
        match self.turns.get(index).copied().flatten() {
            Some(text) => {
                ContentBatch::of(vec![ContentBlock::new("turn", text)]).expect("在上限内")
            }
            None => ContentBatch::empty(),
        }
    }
}

/// 跑一次活动，返回盘上的经历种类序列与提交接缝的调用序列。
fn run_and_read(
    dir: &TempDir,
    run: &RunId,
    source: &mut ScriptedSource,
    script: Vec<ModelOutput>,
) -> (Vec<String>, Vec<String>) {
    let agent = AgentId::new("agent-alpha");
    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    let mut model = ScriptedModel::new(script);
    let mut actions = CountingActions::success("踩到的结果");
    let mut commit = FlakyCommit::new(&mut env, None);
    let calls = commit.calls.clone();
    {
        let mut agent_state = Agent::new(agent.clone());
        let mut ledger = Ledger::new(agent.clone());
        run_activity_with(
            &mut agent_state,
            RunRequest {
                cost: &no_cost,
                run: run.clone(),
                limits: Limits::new(8),
                input: RunInput::empty(),
                source,
                commit: &mut commit,
            },
            &mut model,
            &mut actions,
            &mut ledger,
        )
        .expect("这次活动应正常收场");
    }
    drop(commit);
    let kinds: Vec<String> = env
        .trace()
        .expect("读回经历")
        .iter()
        .map(|record| record.entry().kind().to_string())
        .collect();
    let calls = calls.borrow().clone();
    (calls, kinds)
}

/// 注入记录里每个非空批次的**正文**（空批次不该出现）。
fn injection_texts(dir: &TempDir) -> Vec<String> {
    let env = Environment::open(&dir.env(), AgentId::new("agent-alpha")).expect("重开");
    env.trace()
        .expect("读回经历")
        .iter()
        .filter(|record| matches!(record.entry(), TracePayload::Injection { .. }))
        .map(|record| record.entry().text())
        .collect()
}

/// 非空的 Run 注入之后接一个**空** Turn 批次：Run 注入只能被提交一次。
///
/// 这是"提交对象必须是本次新增的条目"的核心反例：如果按"轨迹当前最后一条是不是注入"判断，
/// 空批次之后最后一条仍然是 Run 注入，于是它会被**再提交一次**。
#[test]
fn a_nonempty_run_injection_followed_by_an_empty_turn_is_committed_only_once() {
    let dir = TempDir::new("inj-run-then-empty-turn");
    let mut source = ScriptedSource::new(Some("开场注入"), vec![None]);
    let (calls, kinds) = run_and_read(
        &dir,
        &RunId::new("run-1"),
        &mut source,
        vec![ModelOutput::finish(SubjectEnding::Silent)],
    );

    assert_eq!(
        calls,
        vec![
            "run-start",
            "input",
            "trace", // Run 级注入
            "trace", // 响应（空 Turn 批次不产生记录）
            "ending",
            "seal",
        ],
        "空 Turn 批次不得产生任何提交"
    );
    assert_eq!(
        kinds.iter().filter(|kind| *kind == "injection").count(),
        1,
        "只应有一条注入记录（Run 级）：{kinds:?}"
    );
    let texts = injection_texts(&dir);
    assert_eq!(texts.len(), 1, "注入不得被重复落盘：{texts:?}");
    assert!(texts[0].contains("开场注入"));
}

/// 连续两个空 Turn 批次：一次注入都不新增，也就一次都不提交。
#[test]
fn consecutive_empty_turns_add_no_injection_records() {
    let dir = TempDir::new("inj-consecutive-empty-turns");
    let mut source = ScriptedSource::new(Some("开场注入"), vec![None, None]);
    let (calls, kinds) = run_and_read(&dir, &RunId::new("run-1"), &mut source, probe_then_finish());

    assert_eq!(
        calls,
        vec![
            "run-start",
            "input",
            "trace", // Run 级注入
            "trace", // 首轮响应
            "selected",
            "dispatched",
            "result",
            "trace", // 观察正文
            "trace", // 次轮响应（两个空 Turn 批次都不产生记录）
            "ending",
            "seal",
        ],
        "两个空 Turn 批次都不得产生提交"
    );
    assert_eq!(
        kinds.iter().filter(|kind| *kind == "injection").count(),
        1,
        "只应有一条注入记录：{kinds:?}"
    );
    assert_eq!(injection_texts(&dir).len(), 1);
}

/// 非空的 Turn 注入之后接一个空 Turn 批次：那条 Turn 注入同样只提交一次。
#[test]
fn a_nonempty_turn_injection_followed_by_an_empty_turn_is_committed_only_once() {
    let dir = TempDir::new("inj-turn-then-empty-turn");
    let mut source = ScriptedSource::new(None, vec![Some("本轮注入"), None]);
    let (calls, kinds) = run_and_read(&dir, &RunId::new("run-1"), &mut source, probe_then_finish());

    assert_eq!(
        calls,
        vec![
            "run-start",
            "input",
            "trace", // Turn 1 注入（Run 级为空）
            "trace", // 首轮响应
            "selected",
            "dispatched",
            "result",
            "trace", // 观察正文
            "trace", // 次轮响应（空 Turn 批次不产生记录）
            "ending",
            "seal",
        ],
        "空 Turn 批次不得重复提交上一条注入"
    );
    let texts = injection_texts(&dir);
    assert_eq!(texts.len(), 1, "Turn 注入不得被重复落盘：{texts:?}");
    assert!(texts[0].contains("本轮注入"));
    assert_eq!(
        kinds.iter().filter(|kind| *kind == "injection").count(),
        1,
        "只应有一条注入记录：{kinds:?}"
    );
}

// ---------------------------------------------------------------------------
// 7. 结束确认、内存收场与封存分开
// ---------------------------------------------------------------------------

/// 跑一次"只收场、不动手"的活动，在第 `fail_at` 次提交失败；返回错误与重开后的读视图。
fn run_until_commit_fails(fail_at: usize) -> (RunError, TempDir) {
    let dir = TempDir::new("settle-order");
    let agent = AgentId::new("agent-alpha");
    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    // 空来源 → 只有 run-start / input / trace（响应） / ending / seal 五个确认点。
    let mut source = ScriptedSource::new(None, vec![None]);
    let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
    let mut actions = CountingActions::success("未用到");
    let mut commit = FlakyCommit::new(&mut env, Some(fail_at));
    let error = {
        let mut agent_state = Agent::new(agent.clone());
        let mut ledger = Ledger::new(agent.clone());
        run_activity_with(
            &mut agent_state,
            RunRequest {
                cost: &no_cost,
                run: RunId::new("run-1"),
                limits: Limits::new(8),
                input: RunInput::empty(),
                source: &mut source,
                commit: &mut commit,
            },
            &mut model,
            &mut actions,
            &mut ledger,
        )
        .expect_err("应当失败")
    };
    assert_eq!(model.calls(), 1, "收场阶段的失败不该引来第二次模型调用");
    drop(commit);
    drop(env);
    (error, dir)
}

/// 终态**没有**被确认：活动不算结束，盘上也不该留下终态。
#[test]
fn an_unconfirmed_ending_is_not_reported_as_an_ended_activity() {
    let (error, dir) = run_until_commit_fails(4);

    let RunError::Commit(commit) = &error else {
        panic!("应当是提交失败：{error:?}");
    };
    assert_eq!(commit.stage(), CommitStage::Ending);
    assert!(
        !commit.after_confirmed_ending(),
        "终态没被确认，不能说是'结束之后'才失败的"
    );

    let env = Environment::open(&dir.env(), AgentId::new("agent-alpha")).expect("重开");
    let kinds: Vec<String> = env
        .trace()
        .expect("读回经历")
        .iter()
        .map(|record| record.entry().kind().to_string())
        .collect();
    assert!(
        !kinds.contains(&"ending".to_string()),
        "终态没有被确认，盘上就不该有终态：{kinds:?}"
    );
    let runs = list_runs(&env).expect("列出活动");
    assert_eq!(runs[0].status(), "interrupted", "必须如实呈现为中断");
}

/// 终态**已经**被确认、封存没有：这两种失败必须能被分开说出来。
///
/// 这条同时是"耐久收场先于内存成功表象"的证据：调用返回了失败，但终态**在盘上**。
#[test]
fn a_confirmed_ending_with_an_unconfirmed_seal_is_reported_distinctly() {
    let (error, dir) = run_until_commit_fails(5);

    let RunError::Commit(commit) = &error else {
        panic!("应当是提交失败：{error:?}");
    };
    assert_eq!(commit.stage(), CommitStage::Seal);
    assert!(
        commit.after_confirmed_ending(),
        "封存失败发生在终态被确认之后，必须能被区分出来"
    );

    let env = Environment::open(&dir.env(), AgentId::new("agent-alpha")).expect("重开");
    let kinds: Vec<String> = env
        .trace()
        .expect("读回经历")
        .iter()
        .map(|record| record.entry().kind().to_string())
        .collect();
    assert!(
        kinds.contains(&"ending".to_string()),
        "终态必须在调用失败之前就已经确认耐久：{kinds:?}"
    );
    assert!(!kinds.contains(&"sealed".to_string()), "封存确实没确认");

    let facts = env.facts().expect("读回事实");
    assert!(
        facts.iter().any(|row| row.fact() == "run-ended"),
        "终态事实也必须已经确认"
    );

    let runs = list_runs(&env).expect("列出活动");
    assert_eq!(
        runs[0].status(),
        "ended-unsealed",
        "不能把'封存未确认'笼统写成'活动被中断'"
    );
    assert!(runs[0].ending().is_some(), "终态必须在");
}

// ---------------------------------------------------------------------------
// 8. 保存失败后的执行准入
// ---------------------------------------------------------------------------

/// 一个**真实入口**产生的保存失败：使用已经封存过的活动身份再开一次。
///
/// 它走的就是 `run_activity_in_environment`：环境的准入读数、提交接缝、失败处理全是生产路径，
/// 没有任何测试专用的旁路。存储在这里**真的**拒绝了写入（同一个 RunId 只能登记一次）。
#[test]
fn a_save_failure_stops_this_call_and_blocks_the_next_activity_in_the_same_environment() {
    let dir = TempDir::new("admission-same-env");
    let agent = AgentId::new("agent-alpha");
    let sandbox = dir.path.join("sandbox");
    std::fs::create_dir_all(&sandbox).expect("建沙盒");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    let mut first = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
    run_activity_in_environment(
        &sandbox,
        &mut env,
        EnvironmentActivity {
            declared_agent: &agent,
            run: RunId::new("run-1"),
            limits: Limits::new(4),
            input: RunInput::empty(),
            source: &mut ScriptedSource::new(None, vec![None]),
            creation: None,
        },
        &mut first,
    )
    .expect("第一次应正常收场");

    // 真实保存失败。
    let mut second = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
    let failure = run_activity_in_environment(
        &sandbox,
        &mut env,
        EnvironmentActivity {
            declared_agent: &agent,
            run: RunId::new("run-1"),
            limits: Limits::new(4),
            input: RunInput::empty(),
            source: &mut ScriptedSource::new(None, vec![None]),
            creation: None,
        },
        &mut second,
    )
    .expect_err("重复活动身份必须失败");
    assert_eq!(second.calls(), 0);

    let CompositionError::SaveFailed {
        stage,
        ending_confirmed,
        pause_recorded,
    } = failure
    else {
        panic!("应当是保存失败：{failure:?}");
    };
    assert_eq!(stage, CommitStage::RunStart);
    assert!(!ending_confirmed);
    assert!(pause_recorded, "暂停记录必须真的落在盘上：{failure:?}");

    // ⚠ **2026-10-06 改（主人裁决）**：那条暂停记录**不再拦开跑**——它是**事实**不是闸
    // （由开场那一块纸条带给主体）。因此同环境的下一个活动**照常执行**（模型真的被调用一次），
    // 而"这次失败留下了持久的暂停记录"这一格由**准入读数**照旧说着。
    let mut next = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
    run_activity_in_environment(
        &sandbox,
        &mut env,
        EnvironmentActivity {
            declared_agent: &agent,
            run: RunId::new("run-2"),
            limits: Limits::new(4),
            input: RunInput::empty(),
            source: &mut ScriptedSource::new(None, vec![None]),
            creation: None,
        },
        &mut next,
    )
    .expect("暂停不再拦开跑");
    assert_eq!(next.calls(), 1, "它真的跑了：模型被调用一次");
    assert!(
        matches!(env.admission().expect("准入读数"), Admission::Paused { .. }),
        "那条记录照旧在准入读数里（放行不等于没有记录）"
    );
}

/// 重新打开之后，**已经记录在案**的保存失败仍然拦得住执行。
#[test]
fn reopening_after_a_recorded_save_failure_still_refuses_to_execute() {
    let dir = TempDir::new("admission-reopen-paused");
    let agent = AgentId::new("agent-alpha");
    let sandbox = dir.path.join("sandbox");
    std::fs::create_dir_all(&sandbox).expect("建沙盒");

    {
        let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
        let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
        run_activity_in_environment(
            &sandbox,
            &mut env,
            EnvironmentActivity {
                declared_agent: &agent,
                run: RunId::new("run-1"),
                limits: Limits::new(4),
                input: RunInput::empty(),
                source: &mut ScriptedSource::new(None, vec![None]),
                creation: None,
            },
            &mut model,
        )
        .expect("应正常收场");
        // 留下真实的暂停记录。
        env.mark_paused(&RunId::new("run-1"), "save-failed:test")
            .expect("写暂停记录");
    }

    // 独立的一次打开：进程换了，保护不能换。
    let mut reopened = Environment::open(&dir.env(), agent.clone()).expect("重开");
    assert_eq!(
        reopened.admission().expect("准入读数"),
        Admission::Paused {
            reason: "save-failed:test".to_string()
        }
    );

    let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
    run_activity_in_environment(
        &sandbox,
        &mut reopened,
        EnvironmentActivity {
            declared_agent: &agent,
            run: RunId::new("run-2"),
            limits: Limits::new(4),
            input: RunInput::empty(),
            source: &mut ScriptedSource::new(None, vec![None]),
            creation: None,
        },
        &mut model,
    )
    .expect("重开之后照常执行");
    assert_eq!(model.calls(), 1, "它真的跑了：模型被调用一次");
    assert!(
        matches!(
            reopened.admission().expect("准入读数"),
            Admission::Paused { .. }
        ),
        "跨进程这一层证据一个字没变：那条记录照旧读得出来（放行不等于没有记录）"
    );
}

/// 没有收尾的活动也拦得住执行，但**不得**被说成"存储故障"。
///
/// 这正是"暂停标记自己也写失败"留下的形状：盘上只有"活动开始了"，没有终态，
/// 也没有任何暂停记录。准入必须拦住它，同时如实说"原因没有被记录"。
#[test]
fn an_unrecorded_incomplete_activity_blocks_execution_without_being_called_a_storage_fault() {
    let dir = TempDir::new("admission-unrecorded");
    let agent = AgentId::new("agent-alpha");
    let sandbox = dir.path.join("sandbox");
    std::fs::create_dir_all(&sandbox).expect("建沙盒");

    {
        let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
        // 第 3 个确认点（Run 注入）失败：活动开始了，却没有收尾，也没有暂停记录。
        let mut source = ScriptedSource::new(Some("开场注入"), vec![None]);
        let mut model = ScriptedModel::new(probe_then_finish());
        let mut actions = CountingActions::success("不该走到这");
        let mut commit = FlakyCommit::new(&mut env, Some(3));
        {
            let mut agent_state = Agent::new(agent.clone());
            let mut ledger = Ledger::new(agent.clone());
            let result = run_activity_with(
                &mut agent_state,
                RunRequest {
                    cost: &no_cost,
                    run: RunId::new("run-1"),
                    limits: Limits::new(8),
                    input: RunInput::empty(),
                    source: &mut source,
                    commit: &mut commit,
                },
                &mut model,
                &mut actions,
                &mut ledger,
            );
            assert!(matches!(result, Err(RunError::Commit(_))));
        }
        drop(commit);
    }

    let mut reopened = Environment::open(&dir.env(), agent.clone()).expect("重开");
    assert_eq!(
        reopened.admission().expect("准入读数"),
        Admission::Unfinished {
            run: RunId::new("run-1")
        },
        "必须认出没有收尾的活动"
    );
    assert!(
        paused_reason(reopened.root())
            .expect("读暂停原因")
            .is_none(),
        "没有任何暂停记录：这就不能被称为存储故障"
    );

    // ⚠ **2026-09-26 改（旧规矩 → 新规矩，主人裁决）**
    //
    // 旧：没有收尾的旧活动**拦住**新活动，报 `CompositionError::UnfinishedRun`，一次模型都不打。
    // 新：**一段清醒被一段不清醒隔开就是两段**——承载活动的进程没了，那一次连续就断了，
    //     那一段也就结束了。新进程起来是**新的一段**，不再拦。
    //     而"上一段没有收尾"这件事，由**开场那一轮**如实带给主体（见 `PreviousActivity`）。
    //
    // **放行 ≠ 解决**：旧的那一段在盘上仍然如实呈现（下面那条断言守的就是这半句，一字未松）。
    let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
    run_activity_in_environment(
        &sandbox,
        &mut reopened,
        EnvironmentActivity {
            declared_agent: &agent,
            run: RunId::new("run-2"),
            limits: Limits::new(4),
            input: RunInput::empty(),
            source: &mut ScriptedSource::new(None, vec![None]),
            creation: None,
        },
        &mut model,
    )
    .expect("没有收尾的旧活动不再拦住新活动");
    assert!(
        model.calls() >= 1,
        "放行之后模型**该被调用**——旧规矩下这里恒为 0，那是它被拦住的样子"
    );
    assert!(
        list_runs(&reopened)
            .expect("列出活动")
            .iter()
            .any(|run| run.status() == "interrupted"),
        "放行不等于解决：旧的那一段仍然如实呈现为 interrupted"
    );
}

/// **显式确认**是唯一的前进方式；确认之后准入重新变绿。
///
/// ⚠ 这条用例说的是 `store::clear_paused`（宿主用 `agent_host_acknowledge` 做的行政动作：
/// "这条异常已经被看过"），**不是**"接着跑那一段没走完的生命"——后者（跨进程恢复）已退役。
/// 用例名里的 `resume` 因此改成 `acknowledgement`：一个概念只有一个名字。
#[test]
fn an_explicit_acknowledgement_is_the_only_way_forward() {
    let dir = TempDir::new("admission-explicit-acknowledgement");
    let agent = AgentId::new("agent-alpha");
    let sandbox = dir.path.join("sandbox");
    std::fs::create_dir_all(&sandbox).expect("建沙盒");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    env.append_trace(
        &RunId::new("run-1"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("登记活动");
    env.mark_paused(&RunId::new("run-1"), "save-failed:test")
        .expect("写暂停记录");
    assert!(!env.admission().expect("准入读数").is_ready());

    // 读一眼不能恢复：准入只读记录，读操作不改变任何东西。
    let _ = env.health().expect("健康读数");
    assert!(!env.admission().expect("准入读数").is_ready());
    assert!(
        list_runs(&env).expect("列出活动")[0].status() == "interrupted",
        "没有收尾的活动仍然如实呈现"
    );

    celira_agent::store::clear_paused(&mut env, &RunId::new("run-1")).expect("显式确认");
    assert_eq!(
        env.admission().expect("准入读数"),
        Admission::Ready,
        "显式确认之后才允许新活动"
    );
}

/// 只回一个"无法确认"的执行端：用来制造 Unknown 结果。
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

/// 动作结果 Unknown **不再收场**：它进视野，主体自己决定（2026-10-24）。
///
/// 这一条守两件事：① "没看见"之后主体还有一次机会（模型被再叫一次，且它看到的观察里带着读数）；
/// ② 这样收场之后环境不锁——活动有可靠终态，下一位住户照常进门。
#[test]
fn an_unknown_action_result_does_not_lock_the_environment() {
    let dir = TempDir::new("admission-unknown-result");
    let agent = AgentId::new("agent-alpha");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    let mut source = ScriptedSource::new(None, vec![None]);
    let mut model = ScriptedModel::new(vec![
        ModelOutput::act(Action::with_params(
            ActionId::new("probe"),
            ActionParams::none().with("note", "踩一下"),
        )),
        // 看见"没拿到回执"之后，主体自己决定收场。
        ModelOutput::finish(SubjectEnding::Silent),
    ]);
    let mut actions = UnknownActions;
    let mut commit = FlakyCommit::new(&mut env, None);
    {
        let mut agent_state = Agent::new(agent.clone());
        let mut ledger = Ledger::new(agent.clone());
        let run = run_activity_with(
            &mut agent_state,
            RunRequest {
                cost: &no_cost,
                run: RunId::new("run-1"),
                limits: Limits::new(4),
                input: RunInput::empty(),
                source: &mut source,
                commit: &mut commit,
            },
            &mut model,
            &mut actions,
            &mut ledger,
        )
        .expect("没看见回执不终止活动");
        assert_eq!(
            run.outcome(),
            Some(Outcome::Subject(SubjectEnding::Silent)),
            "收场是主体自己说的，不是系统替它说的"
        );
        assert_eq!(model.calls(), 2, "没看见之后，主体还得有一次机会");
        let observed = run.observed();
        assert_eq!(observed.len(), 1, "那一次尝试留下了一条观察");
        assert_eq!(observed[0].state(), ResultState::Unknown);
        assert!(
            !observed[0].outcome().content().is_empty(),
            "回执未达要带读数：它是什么、不是什么、下一步能做什么"
        );
    }
    drop(commit);

    assert_eq!(
        env.admission().expect("准入读数"),
        Admission::Ready,
        "Unknown 不是永久封锁：活动已经如实收尾"
    );
    let runs = list_runs(&env).expect("列出活动");
    assert!(
        runs[0].ending().is_some(),
        "这一轮确实有可靠终态，因此它和'没有收尾'是两件事"
    );
}

// ---------------------------------------------------------------------------
// 9. 存储读取与确认边界
// ---------------------------------------------------------------------------

/// 一行**完整合法**的 JSON，但没有末尾换行：它没有被确认为耐久，属于未知尾部。
///
/// 只测"截断成非法 JSON"会漏掉这一种：内容本身能解析，只是没写完。按行切分再数行数
/// 认不出它——因为按行切分会把换行信息丢掉。
#[test]
fn a_complete_json_line_without_a_trailing_newline_is_refused() {
    let dir = TempDir::new("no-trailing-newline");
    let agent = AgentId::new("agent-alpha");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    env.append_trace(
        &RunId::new("run-1"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("写一条");
    drop(env);

    let trace_path = dir.env().join("trace.ndjson");
    let before = std::fs::read_to_string(&trace_path).expect("读回");
    // 追加一条**合法**的 JSON，但去掉末尾换行——"写了一半，但 JSON 碰巧合法"。
    let extra = "{\"seq\":1,\"run\":\"run-1\",\"kind\":\"run-input\",\"text\":\"half\"}";
    std::fs::write(&trace_path, format!("{before}{extra}")).expect("写入无换行尾行");

    match Environment::open(&dir.env(), agent.clone()) {
        Err(StoreError::CorruptTail { seq }) => assert_eq!(seq, 1, "应指出残缺的是第几条"),
        other => panic!("完整 JSON 但没有末尾换行也必须被拒绝，却得到 {other:?}"),
    }

    let after = std::fs::read_to_string(&trace_path).expect("读回");
    assert_eq!(
        after,
        format!("{before}{extra}"),
        "检测到未知尾部不得改写文件"
    );
}

/// 覆盖一份存储文件：测试故意用它造出"外部改写过、不自洽"的文件。
fn overwrite(path: &std::path::Path, content: &str) {
    std::fs::write(path, content).expect("改写存储文件");
}

/// 一份存储文件里已确认的行（按换行边界，不丢换行信息）。
fn confirmed_lines_of(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .expect("读回存储文件")
        .lines()
        .map(str::to_string)
        .collect()
}

/// 序号必须**严格前进**：倒退与重复都不是"顺序"了，必须拒绝，且不得改写文件。
#[test]
fn a_repeated_or_backward_sequence_is_refused_in_both_stores() {
    let dir = TempDir::new("sequence");
    let agent = AgentId::new("agent-alpha");
    let run = RunId::new("run-1");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    env.append_trace(&run, TracePayload::RunStarted { origin: None })
        .expect("经历：登记");
    env.append_trace(&run, TracePayload::Sealed)
        .expect("经历：封存");
    env.append_fact(
        &run,
        &Record::RunStarted {
            agent: agent.clone(),
            run: run.clone(),
        },
    )
    .expect("事实：登记");
    env.append_fact(
        &run,
        &Record::RunEnded {
            run: run.clone(),
            outcome: Outcome::Subject(SubjectEnding::Silent),
            cost: celira_agent::cost::RunCost::default(),
        },
    )
    .expect("事实：结束");
    drop(env);

    let trace_path = dir.env().join("trace.ndjson");
    let facts_path = dir.env().join("facts.ndjson");
    let trace = confirmed_lines_of(&trace_path);
    let facts = confirmed_lines_of(&facts_path);
    assert_eq!(trace.len(), 2, "经历侧应有两条已确认记录");
    assert_eq!(facts.len(), 2, "事实侧应有两条已确认记录");

    // ⚠ **2026-10-06 起这里有两条规则，各自管一半**（长跑读数那一刀的校验和）：
    //   · **重复一行**：序号规则先看见它（第 1 行的序号是 0，前面刚出现过 0）⇒ `OutOfOrder`，
    //     新旧账本都一样——那是更具体的读数（"哪两个序号不自洽"）。
    //   · **把两行调个头**：第 0 行放的是**第 1 行的字节**，序号规则看不见它（它是文件第一行），
    //     于是**校验和说话** ⇒ `ChecksumMismatch`（事实是"这串字节不是写在这里的那串"）；
    //     而**旧账本**（没有校验和）没有这一手，只能由序号规则在第 1 行上抓住 ⇒ `OutOfOrder`。
    // 因此这条用例把两条路**各钉一遍**：先看带校验和的样子，再把校验和剥掉（模拟加校验和之前
    // 写下的旧账本），看序号规则还在不在。

    // 同一序号出现两次（带校验和）：序号规则先说话。
    let duplicated = format!("{}\n{}\n", trace[0], trace[0]);
    overwrite(&trace_path, &duplicated);
    match Environment::open(&dir.env(), agent.clone()) {
        Err(StoreError::OutOfOrder { previous, found }) => {
            assert_eq!((previous, found), (0, 0), "应指出是哪两个序号不自洽");
        }
        other => panic!("重复序号必须被拒绝，却得到 {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(&trace_path).expect("读回"),
        duplicated,
        "拒绝不得改写文件"
    );

    // 序号倒退：文件被外部换成了反序（带校验和）⇒ 第 0 行就与它自己的校验和对不上。
    let backward = format!("{}\n{}\n", trace[1], trace[0]);
    overwrite(&trace_path, &backward);
    match Environment::open(&dir.env(), agent.clone()) {
        Err(StoreError::ChecksumMismatch { seq }) => {
            assert_eq!(seq, 0, "第 0 行放的是第 1 行的字节");
        }
        other => panic!("序号倒退必须被拒绝，却得到 {other:?}"),
    }

    // 把校验和剥掉 ⇒ 这就是**加校验和之前**写下的旧账本：序号规则照旧是唯一判据。
    let strip = |line: &str| match line.rfind('\t') {
        Some(index) => line[..index].to_string(),
        None => line.to_string(),
    };
    let bare: Vec<String> = trace.iter().map(|line| strip(line)).collect();
    let bare_backward = format!("{}\n{}\n", bare[1], bare[0]);
    overwrite(&trace_path, &bare_backward);
    match Environment::open(&dir.env(), agent.clone()) {
        Err(StoreError::OutOfOrder { previous, found }) => {
            assert_eq!((previous, found), (1, 0), "倒退同样必须被指出");
        }
        other => panic!("旧账本的序号倒退必须被指出，却得到 {other:?}"),
    }
    let bare_duplicated = format!("{}\n{}\n", bare[0], bare[0]);
    overwrite(&trace_path, &bare_duplicated);
    match Environment::open(&dir.env(), agent.clone()) {
        Err(StoreError::OutOfOrder { previous, found }) => {
            assert_eq!((previous, found), (0, 0), "应指出是哪两个序号不自洽");
        }
        other => panic!("旧账本的重复序号必须被指出，却得到 {other:?}"),
    }

    // 经历侧恢复之后能打开；事实侧单独出问题时同样拒绝。
    overwrite(&trace_path, &format!("{}\n{}\n", trace[0], trace[1]));
    Environment::open(&dir.env(), agent.clone()).expect("经历侧恢复之后应能打开");

    overwrite(&facts_path, &format!("{}\n{}\n", facts[0], facts[0]));
    match Environment::open(&dir.env(), agent.clone()) {
        Err(StoreError::OutOfOrder { previous, found }) => {
            assert_eq!((previous, found), (0, 0), "事实侧同样先由序号规则抓住");
        }
        other => panic!("事实侧的重复序号也必须被拒绝，却得到 {other:?}"),
    }
    // 事实侧的旧账本形状：序号规则照旧。
    let bare_facts: Vec<String> = facts.iter().map(|line| strip(line)).collect();
    overwrite(
        &facts_path,
        &format!("{}\n{}\n", bare_facts[0], bare_facts[0]),
    );
    match Environment::open(&dir.env(), agent.clone()) {
        Err(StoreError::OutOfOrder { previous, found }) => {
            assert_eq!((previous, found), (0, 0));
        }
        other => panic!("事实侧的重复序号也必须被拒绝，却得到 {other:?}"),
    }
    overwrite(&facts_path, &format!("{}\n{}\n", facts[0], facts[1]));
    Environment::open(&dir.env(), agent.clone()).expect("两侧都恢复之后应能打开");
}

// ---------------------------------------------------------------------------
// 校验和：每条记录一行尾校验和（CRC-32 链）——2026-10-06 长跑读数那一刀
// ---------------------------------------------------------------------------

/// 一行的正文（剥掉行尾 `\t<8 位十六进制>`；没有就原样返回 = 加校验和之前写下的旧记录）。
fn body_of(line: &str) -> &str {
    match line.rfind('\t') {
        Some(index) => &line[..index],
        None => line,
    }
}

/// 造一个三条记录的账本：登记、输入（正文里有那句可辨认的话）、收场。
fn ledger_with_text(dir: &TempDir, agent: &AgentId, text: &str) -> std::path::PathBuf {
    let run = RunId::new("run-1");
    {
        let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
        env.append_trace(&run, TracePayload::RunStarted { origin: None })
            .expect("经历：登记");
        env.append_trace(
            &run,
            TracePayload::RunInput {
                text: text.to_string(),
            },
        )
        .expect("经历：输入");
        env.append_trace(
            &run,
            ending_payload(Outcome::Subject(SubjectEnding::Completed)),
        )
        .expect("经历：收场");
    }
    dir.env().join("trace.ndjson")
}

/// **文本字段里翻一个字节会被抓出来**——这一刀的头号判据。
///
/// 为什么在加校验和之前抓不出来：那一行**照样解析得出来**（JSON 没坏）、**照样解释得了**
/// （词表没坏），于是改动会**静默地**变成"她当时看到的就是这句"。
#[test]
fn a_flipped_byte_inside_a_recorded_text_is_caught() {
    let dir = TempDir::new("checksum-flip");
    let agent = AgentId::new("agent-alpha");
    let path = ledger_with_text(&dir, &agent, "原来的开场输入");

    let text = std::fs::read_to_string(&path).expect("读经历");
    assert!(text.contains("原来的开场输入"), "夹具前提：正文在盘上");
    // 只动一个字节：换成一个同样合法的汉字——JSON 过得去、词表也过得去。
    let flipped = text.replace("原来的开场输入", "元来的开场输入");
    assert_ne!(flipped, text, "夹具必须真的改了一个字节");
    overwrite(&path, &flipped);

    match Environment::open(&dir.env(), agent.clone()) {
        Err(StoreError::ChecksumMismatch { .. }) => {}
        other => panic!("翻过一个字节的记录必须被拒绝，却得到 {other:?}"),
    }
}

/// **加校验和之前写下的旧账本照旧能打开，而且还能继续往下写**。
///
/// 这是**不迁移**那条规矩的机械保证：旧环境不覆盖、不重写、不补校验和；新记录从旧记录的链值
/// 继续算下去（旧记录自己没校验和，但它们进了链）。判据：剥掉校验和后能打开、能追加、
/// 再打开还是好的——这正是真家升级到这一版时会发生的事。
#[test]
fn a_ledger_written_before_checksums_still_opens_and_keeps_growing() {
    let dir = TempDir::new("checksum-old-ledger");
    let agent = AgentId::new("agent-alpha");
    let path = ledger_with_text(&dir, &agent, "旧账本里的那句话");

    let text = std::fs::read_to_string(&path).expect("读经历");
    let bare: String = text
        .lines()
        .map(|line| format!("{}\n", body_of(line)))
        .collect();
    assert!(!bare.contains('\t'), "剥过之后不该再有校验和那一截：{bare}");
    overwrite(&path, &bare);

    let mut env = Environment::open(&dir.env(), agent.clone()).expect("旧账本必须能打开");
    env.append_trace(
        &RunId::new("run-2"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("旧账本上必须还能追加（新记录接着旧记录的链往下算）");
    drop(env);

    // 再打开一次：新记录带着校验和，而它前面那些旧记录没有——链必须仍然自洽。
    let reopened = Environment::open(&dir.env(), agent.clone()).expect("追加之后必须仍然自洽");
    assert_eq!(reopened.trace_records(), 4, "旧三条 ＋ 新一条");
}

/// **链护得住那些自己没有校验和的旧记录**。
///
/// 只把**最后一条**的校验和留下、前面几条剥掉：这时改动**前面**任何一条，最后那条的校验和
/// 就对不上——因为链把每一条与它前面的全部历史绑在一起。这正是"旧记录不可校验"这件事的补丁：
/// 它们由**后面**那条新记录护着。
#[test]
fn the_chain_protects_records_that_have_no_checksum_of_their_own() {
    let dir = TempDir::new("checksum-chain");
    let agent = AgentId::new("agent-alpha");
    let path = ledger_with_text(&dir, &agent, "链护着的那句话");

    let text = std::fs::read_to_string(&path).expect("读经历");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "夹具前提：三条记录");
    // 前两条剥掉校验和（模拟旧记录），最后一条留着（它是在有校验和之后写下的）。
    let mixed = format!(
        "{}\n{}\n{}\n",
        body_of(lines[0]),
        body_of(lines[1]),
        lines[2]
    );
    overwrite(&path, &mixed);
    Environment::open(&dir.env(), agent.clone()).expect("混合账本（旧记录 ＋ 新记录）必须能打开");

    // 改**第一条**（它自己没有校验和）⇒ 最后那条的链对不上。
    let tampered = mixed.replace("链护着的那句话", "改过的那句话");
    assert_ne!(tampered, mixed);
    overwrite(&path, &tampered);
    match Environment::open(&dir.env(), agent.clone()) {
        Err(StoreError::ChecksumMismatch { .. }) => {}
        other => panic!("改过的旧记录必须由后面那条的链抓出来，却得到 {other:?}"),
    }
}

/// **校验和不是主体读得到的东西**：读一段经历、渲染它的正文，都不带那一截。
///
/// 判据：盘上的行里有 `\t<8 位十六进制>`，而 `read_run` 读回来的每一条渲染正文里没有制表符
/// ——存储格式不渗进她的视野（与"派生视图不进注入"同一条纪律）。
#[test]
fn the_checksum_never_reaches_what_the_subject_reads() {
    let dir = TempDir::new("checksum-not-in-view");
    let agent = AgentId::new("agent-alpha");
    let path = ledger_with_text(&dir, &agent, "正文里不该出现校验和");

    let raw = std::fs::read_to_string(&path).expect("读经历");
    assert!(
        raw.lines().all(|line| line.contains('\t')),
        "盘上每一行都该带校验和：{raw}"
    );

    let env = Environment::open(&dir.env(), agent.clone()).expect("打开");
    let page = env
        .read_run(&RunId::new("run-1"), None, 50)
        .expect("读一段");
    assert!(!page.records().is_empty(), "这一段必须有记录");
    for record in page.records() {
        let rendered = record.entry().text();
        assert!(
            !rendered.contains('\t'),
            "渲染正文里不该出现制表符（那是存储格式）：{rendered}"
        );
    }
}

// ---------------------------------------------------------------------------
// 分卷：写满就封口 ＋ 一张目录（市面标准形态第二步，2026-10-06）
// ---------------------------------------------------------------------------

/// 一条大到足以把活动段顶过封口线的记录（阈值 1 MiB ⇒ 这条约 2 MiB）。
fn huge_text(tag: &str) -> String {
    format!("{tag}{}", "字".repeat(2 * 1024 * 1024 / 3))
}

fn parts_dir(dir: &TempDir) -> std::path::PathBuf {
    dir.env().join("trace.parts")
}

fn parts_index(dir: &TempDir) -> std::path::PathBuf {
    dir.env().join("trace.parts.idx")
}

/// 盘上的**卷**文件名（`trace.parts/` 里只算 `<首序号 12 位>.ndjson`）。
///
/// 同一个目录里还躺着每卷一份的**卷内偏移索引**（`<卷名>.idx`）——那是索引，不是卷；
/// 数卷的时候不能把它数进去。
fn sealed_parts(dir: &TempDir) -> Vec<String> {
    let mut files: Vec<String> = std::fs::read_dir(parts_dir(dir))
        .expect("读封口段目录")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".ndjson"))
        .collect();
    files.sort();
    files
}

/// 一卷的卷内偏移索引路径。
fn part_index(dir: &TempDir, part: &str) -> std::path::PathBuf {
    parts_dir(dir).join(format!("{part}.idx"))
}

/// 写到封口为止：登记一条大活动，让它把活动段顶过阈值。返回（卷文件名清单，目录行数）。
fn write_until_sealed(dir: &TempDir, agent: &AgentId) -> (Vec<String>, usize) {
    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    env.append_trace(
        &RunId::new("run-big"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("登记");
    // 这一条下去之后活动段超过 1 MiB ⇒ 封口就在这次追加里发生。
    env.append_trace(
        &RunId::new("run-big"),
        TracePayload::RunInput {
            text: huge_text("大正文"),
        },
    )
    .expect("写大正文");
    drop(env);
    let files = sealed_parts(dir);
    let index = std::fs::read_to_string(parts_index(dir)).expect("读目录");
    // 目录里每一卷**两行**：骨架行（开机只解它）＋ 摘要行（要"一份完整读数"的路才解）。
    let lines = index
        .lines()
        .filter(|line| line.contains("\"kind\":\"part\""))
        .count();
    let summaries = index
        .lines()
        .filter(|line| line.contains("\"kind\":\"runs\""))
        .count();
    assert_eq!(summaries, lines, "每一卷都该带着自己的摘要行");
    (files, lines)
}

/// **写满就封口**：活动段被搬进 `trace.parts/`，目录记下这一卷，活动段重新变小。
#[test]
fn a_full_active_segment_is_sealed_and_recorded_in_the_index() {
    let dir = TempDir::new("segment-roll");
    let agent = AgentId::new("agent-alpha");
    let (files, lines) = write_until_sealed(&dir, &agent);

    assert_eq!(files.len(), 1, "应当恰好封出一卷：{files:?}");
    assert_eq!(lines, 1, "目录应当恰好记一卷");
    let active = std::fs::metadata(dir.env().join("trace.ndjson"))
        .expect("活动段还在")
        .len();
    assert!(active < 1024, "封口之后活动段应当重新变小：{active} 字节");
    let sealed = std::fs::metadata(parts_dir(&dir).join(&files[0]))
        .expect("卷在")
        .len();
    assert!(sealed > 1024 * 1024, "卷里装着那条大记录：{sealed} 字节");

    // 再打开：能开、序号接着走、这一段还在（从目录里读得到）。
    let mut reopened = Environment::open(&dir.env(), agent.clone()).expect("重开");
    assert_eq!(reopened.trace_records(), 2, "两段记录（都在封口段里）");
    let page = reopened
        .read_run(&RunId::new("run-big"), None, 50)
        .expect("读这一段");
    assert_eq!(page.records().len(), 2, "封口段里的记录照旧读得到");
    // 还能继续写：新记录落在新的活动段里。
    reopened
        .append_trace(
            &RunId::new("run-next"),
            TracePayload::RunStarted { origin: None },
        )
        .expect("封口之后照旧能追加");
}

/// **开机不读封口段**：把封口段改坏，`open` 照旧成功（它只读目录 ＋ 活动段）。
///
/// 这是这一刀的**行为证据**，也是**如实登记的代价**：旧卷里的静默损坏不再挡住新写入——
/// 它在"读那一卷"或"显式全量读"时暴露。下面两条把这两半各钉一遍。
#[test]
fn opening_does_not_read_sealed_parts_but_reading_them_still_catches_damage() {
    let dir = TempDir::new("segment-not-read");
    let agent = AgentId::new("agent-alpha");
    let (files, _) = write_until_sealed(&dir, &agent);

    let part = parts_dir(&dir).join(&files[0]);
    let text = std::fs::read_to_string(&part).expect("读卷");
    let tampered = text.replace("大正文", "大正大");
    assert_ne!(tampered, text, "夹具必须真的改了一个字节");
    std::fs::write(&part, &tampered).expect("写回卷");

    // ① 开机照旧成功（不读旧卷）。
    let env = Environment::open(&dir.env(), agent.clone()).expect("开机不读旧卷，因此照旧能开");
    // ② 读那一卷时暴露。
    match env.read_run(&RunId::new("run-big"), None, 50) {
        Err(StoreError::ChecksumMismatch { .. }) => {}
        other => panic!("读被改过的旧卷必须失败，却得到 {other:?}"),
    }
    drop(env);
    // ③ 显式全量读（整份重验）同样暴露。
    match celira_agent::store::read_trace_at(&dir.env()) {
        Err(StoreError::ChecksumMismatch { .. }) => {}
        other => panic!("全量读必须暴露被改过的旧卷，却得到 {other:?}"),
    }
}

/// **增量读只读它该读的**：越过一卷已折过的封口段时，那一卷**一个字节都不读**。
///
/// 判据走的是**行为证据**，不是数内部计数器：把那一卷**改坏**——
/// - `trace_after(该卷的末序号)` 越过它 ⇒ **照旧成功**（它没读它）；
/// - 整本读 `trace()` ⇒ **当场红**（它读了）。
///
/// 这与上一条（"开机不读封口段"）**同一个形状**：分卷之后"谁读了哪一卷"是可判错的，
/// 而判法就是**把那一卷改坏，看谁还活着**。
///
/// ⚠ 同时钉住**链没有断**：跳过一卷时链值从目录里记着的**封口链值**续起，
/// 因此活动段里新写的记录照旧能逐条核对——**这一条是增量读能不能成立的关键**。
#[test]
fn the_incremental_read_does_not_touch_parts_it_has_already_folded() {
    let dir = TempDir::new("trace-after");
    let agent = AgentId::new("agent-alpha");
    let (files, _) = write_until_sealed(&dir, &agent);
    assert_eq!(files.len(), 1, "夹具前提：封出了一卷");

    // 那一卷里有哪些序号（封口之后活动段重新变小，因此这几条都在卷里）。
    let sealed = celira_agent::store::read_trace_at(&dir.env()).expect("全量读");
    let last_seq = sealed
        .iter()
        .map(|record| record.seq())
        .max()
        .expect("有记录");

    // 封口之后再写一条：它落在**新的活动段**里，链从那一卷的封口值续起。
    let mut env = Environment::open(&dir.env(), agent.clone()).expect("打开");
    env.append_trace(
        &RunId::new("run-next"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("封口之后照旧能追加");
    drop(env);

    // 把那一卷改坏。
    let part = parts_dir(&dir).join(&files[0]);
    let text = std::fs::read_to_string(&part).expect("读卷");
    let tampered = text.replace("大正文", "大正大");
    assert_ne!(tampered, text, "夹具必须真的改了一个字节");
    std::fs::write(&part, &tampered).expect("写回卷");

    let reader = celira_agent::store::EnvironmentReader::open(&dir.env(), agent.clone())
        .expect("只读地打开");

    // ① 增量读越过那一卷 ⇒ 照旧成功，而且**只拿到封口之后新写的那一条**。
    let fresh = reader
        .trace_after(last_seq)
        .expect("越过已折过的卷 ⇒ 不读它，因此照旧成功");
    assert_eq!(fresh.len(), 1, "只要封口之后新写的那一条：{fresh:?}");
    assert!(
        fresh[0].seq() > last_seq,
        "增量读不许把折过的条目再交出来：seq={} after={last_seq}",
        fresh[0].seq()
    );

    // ② 同一个读者整本读 ⇒ 当场红（它读了那一卷）。
    match reader.trace() {
        Err(StoreError::ChecksumMismatch { .. }) => {}
        other => panic!("整本读必须暴露被改过的旧卷，却得到 {other:?}"),
    }

    // ③ 增量读的边界是**开区间**：`after = 最后一条的序号` ⇒ 空。
    let up_to_date = reader.trace_after(u64::MAX).expect("越过全部 ⇒ 空");
    assert!(up_to_date.is_empty(), "{up_to_date:?}");
}

/// **目录是检查点**：封口之前留下的"暂停"与"没有收尾的活动"，封口之后照旧读得出来。
///
/// 这一条是分卷能成立的关键：开机只读目录 ＋ 活动段，因此那两格必须在封口时**折进目录**，
/// 否则"主体上次为什么停""上一段有没有收尾"就得重读全部旧卷才答得出来。
#[test]
fn the_index_carries_the_admission_readings_across_a_seal() {
    let dir = TempDir::new("segment-fold");
    let agent = AgentId::new("agent-alpha");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    // 一次**没有收尾**的活动，外加一条**已记录在案**的存储故障。
    env.append_trace(
        &RunId::new("run-left"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("登记一个没收尾的活动");
    env.mark_paused(&RunId::new("run-left"), "save-failed:run-start")
        .expect("写下暂停记录");
    // 顶过阈值：封口在这一条之后发生。
    env.append_trace(
        &RunId::new("run-big"),
        TracePayload::RunInput {
            text: huge_text("顶过阈值"),
        },
    )
    .expect("写大正文");
    drop(env);

    // 夹具前提：封口**真的**发生了（否则下面那条"折进目录"根本没被验到）。
    assert!(
        parts_dir(&dir).exists() && parts_index(&dir).exists(),
        "夹具前提：这一条应当把活动段顶过封口线"
    );

    let reopened = Environment::open(&dir.env(), agent.clone()).expect("重开");
    assert_eq!(
        reopened.admission().expect("准入读数"),
        Admission::Paused {
            reason: "save-failed:run-start".to_string()
        },
        "封口段里的暂停必须照旧读得出来（它折在目录里）"
    );
    assert_eq!(
        reopened
            .opening_readings()
            .expect("开场读数")
            .last_storage_failure
            .map(|failure| failure.reason),
        Some(Some("save-failed:run-start".to_string())),
        "历史上最后一次存储故障也折在目录里"
    );
    // 列表也照旧：旧卷里的活动从目录里列出来。
    let runs = list_runs(&reopened).expect("列活动");
    assert!(
        runs.iter()
            .any(|summary| summary.run().as_str() == "run-left"),
        "封口段里的活动照旧列得出来：{runs:?}"
    );
}

/// **一个解释不了的暂停检查点不被采信**：目录里写着"还停着"、却不说为什么 ⇒ 整份目录不可信 ⇒
/// 由**卷**重建，暂停与它的原因从记录里**重新算出来**。
///
/// 为什么这一条必须存在：`reason: null` ＋ `acknowledged: false` 这个组合**没有任何含义**——
/// 原因缺席只有一种已定义的含义（"账本里只有确认、没有它确认的那条暂停"，见
/// `StorageFailure::reason`），而那种形状里 `acknowledged` **必定为真**。反过来说，"还停着"
/// 却不说为什么，是一句谁也解释不了的话。
///
/// 它从前被收下，然后在渲染面折成空串：`环境已暂停（）：…` ——**一对空括号**，而系统明明
/// 知道"原因没有被记录"这条事实。今天它在**解码端**被拒绝（"能解析但无法解释"该走的正是
/// 这条路），代价只是重读一次卷。
///
/// 判据两面：① 解释不了的那一行**不许被采信**（否则渲染面就会说出那句它解释不了的话）；
/// ② 真相照旧回得来——**卷是权威、目录只是索引**。
#[test]
fn an_uninterpretable_pause_checkpoint_is_not_trusted() {
    let dir = TempDir::new("segment-pause-unreasoned");
    let agent = AgentId::new("agent-alpha");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    env.append_trace(
        &RunId::new("run-left"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("登记一个没收尾的活动");
    env.mark_paused(&RunId::new("run-left"), "save-failed:run-start")
        .expect("写下暂停记录");
    env.append_trace(
        &RunId::new("run-big"),
        TracePayload::RunInput {
            text: huge_text("顶过阈值"),
        },
    )
    .expect("写大正文");
    drop(env);

    // 夹具前提：封口真的发生了（否则下面改的那一行根本不在）。
    assert!(
        parts_index(&dir).exists(),
        "夹具前提：这一条应当把活动段顶过封口线"
    );

    // 手改目录：把暂停的原因抹掉，**保留"没有被确认"**——于是这一行成了谁也解释不了的话。
    let index = std::fs::read_to_string(parts_index(&dir)).expect("读目录");
    let mut edited = String::new();
    let mut touched = 0usize;
    for line in index.lines() {
        if line.contains("\"paused\":{") {
            assert!(
                line.contains("\"reason\":\"save-failed:run-start\""),
                "夹具前提：检查点里带着那条原因：{line}"
            );
            assert!(
                line.contains("\"acknowledged\":false"),
                "夹具前提：这一条是「没有被确认」的暂停：{line}"
            );
            edited
                .push_str(&line.replace("\"reason\":\"save-failed:run-start\"", "\"reason\":null"));
            touched += 1;
        } else {
            edited.push_str(line);
        }
        edited.push('\n');
    }
    assert_eq!(touched, 1, "夹具前提：目录里恰好有一行带着暂停检查点");
    std::fs::write(parts_index(&dir), edited).expect("写回目录");

    let reopened = Environment::open(&dir.env(), agent.clone()).expect("重开");
    assert_eq!(
        reopened.admission().expect("准入读数"),
        Admission::Paused {
            reason: "save-failed:run-start".to_string()
        },
        "解释不了的检查点不许被采信：真相由卷重建出来，而不是折成一个空原因"
    );
}

/// **目录丢了可以重建**：删掉目录，开机重读所有卷把它重算出来（慢一次，之后照旧）。
///
/// 判据是"卷是权威、目录只是索引"：她的记忆在卷里，不在目录里。
#[test]
fn a_missing_index_is_rebuilt_from_the_sealed_parts() {
    let dir = TempDir::new("segment-rebuild");
    let agent = AgentId::new("agent-alpha");
    let (files, _) = write_until_sealed(&dir, &agent);

    std::fs::remove_file(parts_index(&dir)).expect("删掉目录");
    let env = Environment::open(&dir.env(), agent.clone()).expect("目录丢了必须能重建");
    assert_eq!(env.trace_records(), 2, "重建之后条数照旧");
    let rebuilt = std::fs::read_to_string(parts_index(&dir)).expect("目录被写了回来");
    assert!(
        rebuilt.contains(&files[0]),
        "重建出来的目录要点到那一卷：{rebuilt}"
    );
    let page = env
        .read_run(&RunId::new("run-big"), None, 50)
        .expect("读这一段");
    assert_eq!(page.records().len(), 2, "重建之后照旧读得到");
}

/// 封出一段**横跨两卷 ＋ 活动段**的活动（3 条大记录 + 1 条收场），返回（环境目录, 活动）。
///
/// 形状的来路：封口是**按字节**触发的，切点可能落在一段活动中间。
fn seal_straddling_run(dir: &TempDir, agent: &AgentId) -> RunId {
    let run = RunId::new("run-straddle");
    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    env.append_trace(&run, TracePayload::RunStarted { origin: None })
        .expect("登记");
    // 两条大正文：第一条把活动段顶过封口线（切点落在这段活动中间），第二条再封一卷。
    env.append_trace(
        &run,
        TracePayload::RunInput {
            text: huge_text("切点之前"),
        },
    )
    .expect("第一条大正文");
    env.append_trace(
        &run,
        TracePayload::RunInput {
            text: huge_text("切点之后"),
        },
    )
    .expect("第二条大正文");
    // 收场落在**新的活动段**里 ⇒ 这一段活动横跨两卷 ＋ 活动段。
    env.append_trace(
        &run,
        ending_payload(Outcome::Subject(SubjectEnding::Completed)),
    )
    .expect("收场");
    drop(env);
    run
}

/// **一段活动横跨两卷时，列表里只算它一次**，条数与序号区间是**整段**的。
///
/// 判据：活动是主体认得的地址（`history/<活动>/`）——同一个地址在列表里出现两次，等于让她看见
/// 两段自己；而"这一卷里有多少条"是**卷的**读数，不是**活动的**读数。合并规则：条数相加、
/// 序号区间取并、收场与标志取"有就带上"。
#[test]
fn an_activity_that_straddles_two_sealed_parts_is_listed_once() {
    let dir = TempDir::new("segment-list-once");
    let agent = AgentId::new("agent-alpha");
    let run = seal_straddling_run(&dir, &agent);

    let env = Environment::open(&dir.env(), agent.clone()).expect("重开");
    let runs = list_runs(&env).expect("列活动");
    let mine: Vec<_> = runs
        .iter()
        .filter(|summary| summary.run() == &run)
        .collect();
    assert_eq!(
        mine.len(),
        1,
        "横跨两卷的活动只该列**一次**（同一个地址不许出现两次）：{runs:?}"
    );
    assert_eq!(mine[0].records(), 4, "条数是**整段**的（两卷 ＋ 活动段）");
    assert_eq!(mine[0].last_seq(), 3, "序号区间取并：到收场那一条");
    assert_eq!(
        mine[0].ending(),
        Some("subject:completed"),
        "收场跟着整段走：{}",
        mine[0].ending().unwrap_or("none")
    );
}

/// **一段活动的记录可以横跨两卷**，读它必须**收全**（2026-10-06 修的那个 bug）。
///
/// 为什么会有这个形状：封口是**按字节**触发的，切点可能落在一段活动中间；而且环境级记录
/// （例如事后写下的存储故障）会在活动封口**之后**才落盘。从前 `read_run_at` 只取**第一个**
/// 装着该活动的卷就返回 ⇒ 会只读到前半截——这条用例就是钉住它。
#[test]
fn a_run_that_straddles_two_sealed_parts_is_read_whole() {
    let dir = TempDir::new("segment-straddle");
    let agent = AgentId::new("agent-alpha");
    let run = seal_straddling_run(&dir, &agent);

    // 夹具前提：真的封了两卷，而且活动段里还有这一段的记录。
    let sealed_count = sealed_parts(&dir).len();
    assert_eq!(sealed_count, 2, "应当封出两卷");
    let active = std::fs::read_to_string(dir.env().join("trace.ndjson")).expect("读活动段");
    assert!(active.contains("run-straddle"), "收场那条落在活动段里");

    let reopened = Environment::open(&dir.env(), agent.clone()).expect("重开");
    let page = reopened.read_run(&run, None, 50).expect("读这一段");
    assert_eq!(
        page.records().len(),
        4,
        "四段记录必须**全**读回来（横跨两卷 ＋ 活动段）"
    );
    let kinds: Vec<&str> = page
        .records()
        .iter()
        .map(|record| record.entry().kind())
        .collect();
    assert!(
        kinds.contains(&"ending") && kinds.contains(&"run-start"),
        "两头的记录都不能丢：{kinds:?}"
    );

    // 每一卷都有自己的**卷内偏移索引**，而且都记着这一段（跨卷收全靠的是"每一卷各读自己那一段"）。
    for part in sealed_parts(&dir) {
        let spans = spans_of(&dir, &part);
        assert!(
            spans.iter().any(|span| span["run"] == "run-straddle"),
            "{part} 的索引里应当有这一段：{spans:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 卷内偏移索引：每卷一份 `<卷名>.idx`，按活动记边界（市面标准形态第三步，2026-10-06）
// ---------------------------------------------------------------------------

/// 一卷的偏移索引里那些**读得出来**的区间（每行一个 JSON 对象；空行不算）。
///
/// 与读侧同一条规矩：**尾部半行不算损坏**，读不出来就当它没有（前面那些照用）——因此这个帮手
/// 在第一个解不出来的地方停下，而不是 panic。要钉"索引被写回了几行"，数的就是这里。
fn spans_of(dir: &TempDir, part: &str) -> Vec<serde_json::Value> {
    let mut spans = Vec::new();
    for line in std::fs::read_to_string(part_index(dir, part))
        .expect("卷内偏移索引应当在封口时写好")
        .lines()
    {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str(line) {
            Ok(span) => spans.push(span),
            Err(_) => break,
        }
    }
    spans
}

/// 一卷的盘上字节。
fn part_bytes(dir: &TempDir, part: &str) -> Vec<u8> {
    std::fs::read(parts_dir(dir).join(part)).expect("读卷")
}

/// 取一卷里某个活动的区间（`first-offset` / `end-offset`）。
fn range_of(dir: &TempDir, part: &str, run: &str) -> (usize, usize) {
    let spans = spans_of(dir, part);
    let span = spans
        .iter()
        .find(|span| span["run"] == run)
        .unwrap_or_else(|| panic!("{part} 的索引里应当有 {run}：{spans:?}"));
    (
        span["first-offset"].as_u64().expect("first-offset") as usize,
        span["end-offset"].as_u64().expect("end-offset") as usize,
    )
}

/// 封出一卷，里面装着**两段活动**：`run-small`（小）在前、`run-big`（顶过阈值的那条）在后。
///
/// 两段同卷才验得到"区间不只是从 0 开始"：后一段的 `first-offset` 必须真的落在它自己第一行上。
fn seal_two_runs_in_one_part(dir: &TempDir, agent: &AgentId) -> String {
    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    env.append_trace(
        &RunId::new("run-small"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("登记小活动");
    env.append_trace(
        &RunId::new("run-small"),
        TracePayload::RunInput {
            text: "小正文".to_string(),
        },
    )
    .expect("写小正文");
    env.append_trace(
        &RunId::new("run-big"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("登记大活动");
    // 这一条下去之后活动段超过 1 MiB ⇒ 封口就在这次追加里发生（两段活动同在这一卷里）。
    env.append_trace(
        &RunId::new("run-big"),
        TracePayload::RunInput {
            text: huge_text("大正文"),
        },
    )
    .expect("写大正文");
    drop(env);
    let files = sealed_parts(dir);
    assert_eq!(files.len(), 1, "夹具前提：应当恰好封出一卷：{files:?}");
    files[0].clone()
}

/// **索引的区间就是这一卷里那几行的真实字节位置**：seek 过去读出来，正好是这一段的行。
///
/// 这是"读一段旧活动"能只读一小段的**前提**：区间要是偏一个字节，读出来的就不是行。
#[test]
fn the_offset_index_points_at_the_real_lines_of_each_activity() {
    let dir = TempDir::new("span-offsets");
    let agent = AgentId::new("agent-alpha");
    let part = seal_two_runs_in_one_part(&dir, &agent);
    let bytes = part_bytes(&dir, &part);
    let text = String::from_utf8(bytes.clone()).expect("卷是 UTF-8");

    let (small_first, small_end) = range_of(&dir, &part, "run-small");
    let (big_first, big_end) = range_of(&dir, &part, "run-big");

    // ① 第一段从卷首开始，两段首尾相接、合起来正好是整卷（中间没有缝、也没有重叠）。
    assert_eq!(small_first, 0, "第一段从卷首开始");
    assert_eq!(
        small_end, big_first,
        "两段首尾相接：{small_end} / {big_first}"
    );
    assert_eq!(
        big_end,
        bytes.len(),
        "最后一段到卷尾：{big_end} / {}",
        bytes.len()
    );

    // ② 每一段的字节区间**整行对齐**，而且正好装着这一段的每一行。
    for (run, first, end) in [
        ("run-small", small_first, small_end),
        ("run-big", big_first, big_end),
    ] {
        let slice = &text[first..end];
        assert!(slice.ends_with('\n'), "{run} 的区间必须以换行结尾");
        let lines: Vec<&str> = slice.lines().collect();
        assert!(!lines.is_empty(), "{run} 的区间不是空的");
        for line in &lines {
            assert!(
                line.contains(&format!("\"run\":\"{run}\"")),
                "{run} 的区间里混进了别人的行：{line}"
            );
            let other = if run == "run-small" {
                "run-big"
            } else {
                "run-small"
            };
            assert!(
                !line.contains(&format!("\"run\":\"{other}\"")),
                "{run} 的区间里混进了 {other} 的行"
            );
        }
        // 这一段在整卷里的行数与区间里的行数一致（没有漏掉尾巴）。
        let total = text
            .lines()
            .filter(|line| line.contains(&format!("\"run\":\"{run}\"")))
            .count();
        assert_eq!(lines.len(), total, "{run} 的区间必须收全它的行");
    }

    // ③ `chain-at-start`：第一段从链的种子起算（0），第二段不是——它是第一段折完之后的值。
    let spans = spans_of(&dir, &part);
    let chain_of = |run: &str| {
        spans
            .iter()
            .find(|span| span["run"] == run)
            .expect("有这一段")["chain-at-start"]
            .as_str()
            .expect("chain-at-start 是十六进制文本")
            .to_string()
    };
    assert_eq!(
        chain_of("run-small"),
        "00000000",
        "卷首那一段从链的种子起算"
    );
    assert_ne!(
        chain_of("run-big"),
        "00000000",
        "第二段带的是区间起点的链值"
    );
    assert_eq!(
        spans.len(),
        2,
        "一卷里两段活动就两行：{}",
        serde_json::to_string(&spans).expect("可序列化")
    );
}

/// **读一段旧活动只读它那一段字节**：卷里**别处**的损坏不挡这一次读。
///
/// 这是这一刀买到的速度的**直接后果**，也是**如实登记的代价**（同分卷那一刀）：从前读一段要
/// 整卷核一遍，因此卷里任何一处损坏都会挡住它。现在两半各钉一遍——
/// 区间**外**的损坏不挡（省下来的就是这一段），区间**内**的损坏照旧当场拒绝，
/// 而"整份重验"那条路（`read_trace_at`）照旧看得见全部损坏。
#[test]
fn reading_one_activity_reads_only_its_bytes_so_damage_elsewhere_does_not_block_it() {
    let dir = TempDir::new("span-scope");
    let agent = AgentId::new("agent-alpha");
    let part = seal_two_runs_in_one_part(&dir, &agent);

    // 把小活动那一行**改成同样长**的一串字节：改的是内容，不动偏移（否则验的就不是"区间"了）。
    let text = String::from_utf8(part_bytes(&dir, &part)).expect("卷是 UTF-8");
    let tampered = text.replace("小正文", "小正大");
    assert_ne!(tampered, text, "夹具必须真的改了一个字节");
    assert_eq!(tampered.len(), text.len(), "夹具必须**等长**替换，不动偏移");
    std::fs::write(parts_dir(&dir).join(&part), &tampered).expect("写回卷");

    // ① 区间**外**的损坏不挡：读大活动照旧拿到它自己那两条。
    let page = celira_agent::store::read_run_at(&dir.env(), &RunId::new("run-big"), None, 50)
        .expect("这一段自己的字节没坏，就必须读得出来");
    assert_eq!(page.records().len(), 2, "大活动两条记录照旧收全");

    // ② 区间**内**的损坏照旧当场拒绝（切片的链值校验是真的在跑）。
    match celira_agent::store::read_run_at(&dir.env(), &RunId::new("run-small"), None, 50) {
        Err(StoreError::ChecksumMismatch { .. }) => {}
        other => panic!("读被改过的那一段必须失败，却得到 {other:?}"),
    }

    // ③ "整份重验"那条路照旧看得见它（损坏没有被抹掉，只是不再挡住别人的那一段）。
    match celira_agent::store::read_trace_at(&dir.env()) {
        Err(StoreError::ChecksumMismatch { .. }) => {}
        other => panic!("全量读必须暴露被改过的卷，却得到 {other:?}"),
    }
}

/// **索引尾部半行不算损坏**：读不到的那个区间由"重建"补上，而且顺手把索引写回完整。
///
/// 形状来自封口那一刻崩在写索引中间：卷是权威、索引只是索引，因此这里**不报损坏**，
/// 只是那一格暂时读不到——下次要读它时重读该卷补上（自愈，不需要谁去修）。
#[test]
fn a_torn_index_line_is_tolerated_and_the_missing_span_comes_back_on_demand() {
    let dir = TempDir::new("span-torn");
    let agent = AgentId::new("agent-alpha");
    let part = seal_two_runs_in_one_part(&dir, &agent);

    // 砍掉最后一行的尾巴（`run-big` 那一行）——它还在卷里，只是索引里没了。
    let index = part_index(&dir, &part);
    let text = std::fs::read_to_string(&index).expect("读索引");
    let last = text.rfind('\n').expect("索引以换行结尾");
    let cut = last - 10;
    std::fs::write(&index, &text[..cut]).expect("写回半行索引");
    assert_eq!(
        spans_of(&dir, &part).len(),
        1,
        "夹具前提：最后一行的区间确实没了"
    );

    // ① 半行不算损坏：另一段（区间还在）照旧读得到。
    let page = celira_agent::store::read_run_at(&dir.env(), &RunId::new("run-small"), None, 50)
        .expect("索引尾部半行不该被读成损坏");
    assert_eq!(page.records().len(), 2);

    // ② 读不到区间的那一段：重读该卷，记录照旧收全，索引顺手写回完整（自愈）。
    let page = celira_agent::store::read_run_at(&dir.env(), &RunId::new("run-big"), None, 50)
        .expect("区间丢了必须重读该卷，而不是报损坏");
    assert_eq!(page.records().len(), 2, "重读该卷之后记录照旧收全");
    assert_eq!(spans_of(&dir, &part).len(), 2, "重建把索引写回了完整的两行");
}

/// **索引是派生的**：丢了、或与卷对不上（链值不对），一律重读该卷重建——记录一条都不少。
#[test]
fn an_index_that_disagrees_with_the_part_is_rebuilt_from_it() {
    let dir = TempDir::new("span-rebuild");
    let agent = AgentId::new("agent-alpha");
    let part = seal_two_runs_in_one_part(&dir, &agent);
    let index = part_index(&dir, &part);

    // ① 整份丢了：读得到，而且被写了回来。
    std::fs::remove_file(&index).expect("删掉索引");
    let page = celira_agent::store::read_run_at(&dir.env(), &RunId::new("run-small"), None, 50)
        .expect("索引丢了必须重读该卷");
    assert_eq!(page.records().len(), 2);
    assert!(index.exists(), "重建之后索引回到盘上");

    // ② 与卷对不上：把区间起点的链值改错（**等长**替换，索引本身仍然是一行合法 JSON）。
    let text = std::fs::read_to_string(&index).expect("读索引");
    let tampered = text.replace("00000000", "deadbeef");
    assert_ne!(tampered, text, "夹具必须真的改了链值");
    assert_eq!(tampered.len(), text.len(), "等长替换");
    std::fs::write(&index, &tampered).expect("写回错链值");

    let page = celira_agent::store::read_run_at(&dir.env(), &RunId::new("run-small"), None, 50)
        .expect("链值对不上必须重读该卷重建，而不是把错读数报成损坏");
    assert_eq!(page.records().len(), 2, "记录照旧收全");
    assert!(
        std::fs::read_to_string(&index)
            .expect("读索引")
            .contains("00000000"),
        "重建把正确的链值写了回去"
    );

    // ③ **目录**（`trace.parts.idx`）整个丢了：定位那一趟问不出答案 ⇒ 走权威路（它会重读所有卷
    //    把目录重建出来），记录照旧一条不少。
    std::fs::remove_file(parts_index(&dir)).expect("删掉目录");
    let page = celira_agent::store::read_run_at(&dir.env(), &RunId::new("run-big"), None, 50)
        .expect("目录丢了必须走权威路");
    assert_eq!(page.records().len(), 2, "记录照旧收全");
    assert!(parts_index(&dir).exists(), "权威路把目录写了回来");

    // ④ 目录与盘**对不上**（多了一条读不出来的尾巴）：也不信它，走权威路。
    let mut text = std::fs::read_to_string(parts_index(&dir)).expect("读目录");
    text.push_str("{\"kind\":\"pa");
    std::fs::write(parts_index(&dir), text).expect("写回半行目录");
    let page = celira_agent::store::read_run_at(&dir.env(), &RunId::new("run-small"), None, 50)
        .expect("目录对不上必须走权威路，而不是按它对不上的一半去找");
    assert_eq!(page.records().len(), 2, "记录照旧收全");
}

// ---------------------------------------------------------------------------
// 事实侧分卷：同一套形状，另一份存储（市面标准形态第二步的对称那一半，2026-10-06）
// ---------------------------------------------------------------------------

fn facts_parts_dir(dir: &TempDir) -> std::path::PathBuf {
    dir.env().join("facts.parts")
}

fn facts_index(dir: &TempDir) -> std::path::PathBuf {
    dir.env().join("facts.parts.idx")
}

/// 盘上的**事实卷**文件名（只算 `<首序号 12 位>.ndjson`）。
///
/// 目录还不存在时算"一卷都没有"（封口之前就是这一形状），不是失败。
fn sealed_fact_parts(dir: &TempDir) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(facts_parts_dir(dir)) else {
        return Vec::new();
    };
    let mut files: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".ndjson"))
        .collect();
    files.sort();
    files
}

/// 写事实直到封口为止（阈值 128 KiB ⇒ 一条事实约 120 字节，因此约 1100 条）。
///
/// 返回（封口前的条数, 封出的卷文件名清单）。
fn write_facts_until_sealed(dir: &TempDir, agent: &AgentId) -> (usize, Vec<String>) {
    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    let mut written = 0usize;
    while sealed_fact_parts(dir).is_empty() {
        let run = RunId::new(format!("run-{written:05}"));
        env.append_fact(
            &run,
            &Record::ActionPrepared {
                run: run.clone(),
                operation: OperationId::new(run.clone(), Turn::FIRST),
            },
        )
        .expect("写事实");
        written += 1;
        assert!(written < 5000, "夹具前提：128 KiB 之内必须封口");
    }
    drop(env);
    (written, sealed_fact_parts(dir))
}

/// **事实写满就封口**：活动段被搬进 `facts.parts/`，目录记下这一卷，活动段重新变小。
///
/// 这一格是"开机那一拍"从前最大的开销（实测 1200 段活动上占 85%）：它今天只读**目录 ＋ 活动段**。
#[test]
fn a_full_facts_segment_is_sealed_and_recorded_in_its_index() {
    let dir = TempDir::new("facts-roll");
    let agent = AgentId::new("agent-alpha");
    let (written, files) = write_facts_until_sealed(&dir, &agent);

    assert_eq!(files.len(), 1, "应当恰好封出一卷：{files:?}");
    let index = std::fs::read_to_string(facts_index(&dir)).expect("读事实目录");
    let lines: Vec<&str> = index
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(lines.len(), 1, "目录应当恰好记一行");
    let row: serde_json::Value = serde_json::from_str(lines[0]).expect("目录行是 JSON");
    assert_eq!(row["kind"], "fact-part", "事实目录有自己的种类名");
    assert_eq!(
        row["records"].as_u64(),
        Some(written as u64),
        "这一卷记着条数"
    );
    let active = std::fs::metadata(dir.env().join("facts.ndjson"))
        .expect("活动段还在")
        .len();
    assert!(
        active < 1024,
        "封口之后事实活动段应当重新变小：{active} 字节"
    );
    let sealed = std::fs::metadata(facts_parts_dir(&dir).join(&files[0]))
        .expect("卷在")
        .len();
    assert!(
        sealed >= 128 * 1024,
        "卷里装着写满的那些事实：{sealed} 字节"
    );

    // 重开：能开、序号接着走、条数与内容都照旧（**跨卷收全**）。
    let mut reopened = Environment::open(&dir.env(), agent.clone()).expect("重开");
    assert_eq!(reopened.fact_records(), written, "已确认条数照旧");
    let rows = reopened.facts().expect("全量读事实");
    assert_eq!(rows.len(), written, "封口段里的事实照旧全读得到");
    assert_eq!(rows.last().expect("有行").seq() as usize, written - 1);
    // 还能继续写：新事实落在新的活动段里，链从上一卷续起。
    let next = RunId::new("run-next");
    reopened
        .append_fact(
            &next,
            &Record::ActionPrepared {
                run: next.clone(),
                operation: OperationId::new(next.clone(), Turn::FIRST),
            },
        )
        .expect("封口之后照旧能追加");
    assert_eq!(reopened.fact_records(), written + 1);
    let rows = reopened.facts().expect("全量读事实");
    assert_eq!(rows.len(), written + 1, "跨卷 ＋ 活动段收全");
    assert_eq!(rows.last().expect("有行").run(), "run-next");
}

/// **开机不读事实封口段**：把封口段改坏，`open` 照旧成功（它只读目录 ＋ 活动段）。
///
/// 这是这一刀的**行为证据**，也是**如实登记的代价**（同经历那一刀）：旧卷里的静默损坏不再挡住
/// 开机——它在"整份重验"那条路（`facts()` / 健康自检）上暴露。
#[test]
fn opening_does_not_read_sealed_fact_parts_but_reading_them_still_catches_damage() {
    let dir = TempDir::new("facts-not-read");
    let agent = AgentId::new("agent-alpha");
    let (written, files) = write_facts_until_sealed(&dir, &agent);

    let part = facts_parts_dir(&dir).join(&files[0]);
    let text = std::fs::read_to_string(&part).expect("读卷");
    // **等长**替换：改的是内容，不动行边界（否则验的就不是"内容被改"了）。
    let tampered = text.replace("\"run\":\"run-00000\"", "\"run\":\"run-0000x\"");
    assert_ne!(tampered, text, "夹具必须真的改了一个字节");
    assert_eq!(tampered.len(), text.len(), "等长替换");
    std::fs::write(&part, &tampered).expect("写回卷");

    // ① 开机照旧成功（不读旧卷）。
    let env = Environment::open(&dir.env(), agent.clone()).expect("开机不读旧卷，因此照旧能开");
    assert_eq!(env.fact_records(), written, "条数从目录里拿，不重读旧卷");
    // ② 整份读事实时暴露（校验和逐条核）。
    match env.facts() {
        Err(StoreError::ChecksumMismatch { .. }) => {}
        other => panic!("读被改过的旧卷必须失败，却得到 {other:?}"),
    }
    // ③ 健康自检同样暴露（它也走整份读）。
    match env.health() {
        Err(StoreError::ChecksumMismatch { .. }) => {}
        other => panic!("健康自检必须暴露被改过的事实卷，却得到 {other:?}"),
    }
}

/// **事实目录丢了可以重建**：删掉目录，开机重读所有卷把它重算出来（慢一次，之后照旧）。
#[test]
fn a_missing_facts_index_is_rebuilt_from_the_sealed_parts() {
    let dir = TempDir::new("facts-rebuild");
    let agent = AgentId::new("agent-alpha");
    let (written, files) = write_facts_until_sealed(&dir, &agent);

    std::fs::remove_file(facts_index(&dir)).expect("删掉目录");
    let env = Environment::open(&dir.env(), agent.clone()).expect("目录丢了必须能重建");
    assert_eq!(env.fact_records(), written, "重建之后条数照旧");
    let rebuilt = std::fs::read_to_string(facts_index(&dir)).expect("目录被写了回来");
    assert!(
        rebuilt.contains(&files[0]),
        "重建出来的目录要点到那一卷：{rebuilt}"
    );
    assert_eq!(env.facts().expect("全量读事实").len(), written);
}

/// **事实的链跨卷续起**：封口之后新写的事实，校验和接着上一卷的链值算。
///
/// 判据比"改一个字节"更准：把封口段的**最后一条整行删掉**——那一卷里剩下的行**自己都还对得上**
/// （改一个字节会被那一行自己的校验和抓住，那证明不了跨卷那条链），而活动段里那条新记录的校验和
/// 是接着**被删掉的那一条**算的 ⇒ 它必须对不上。因此"链把全部历史绑在一起"在事实侧分卷之后
/// 照旧成立。
#[test]
fn the_facts_chain_carries_across_a_seal() {
    let dir = TempDir::new("facts-chain");
    let agent = AgentId::new("agent-alpha");
    let (written, files) = write_facts_until_sealed(&dir, &agent);

    // 封口之后再写一条：它落在新活动段里，链从这一卷的末尾续起。
    let mut env = Environment::open(&dir.env(), agent.clone()).expect("重开");
    let next = RunId::new("run-after-seal");
    env.append_fact(
        &next,
        &Record::ActionPrepared {
            run: next.clone(),
            operation: OperationId::new(next.clone(), Turn::FIRST),
        },
    )
    .expect("封口之后追加");
    drop(env);
    let active = std::fs::read_to_string(dir.env().join("facts.ndjson")).expect("读活动段");
    assert!(active.contains("run-after-seal"), "新事实在活动段里");

    // 删掉封口段的最后一条：卷里剩下的行逐条自己仍然对得上。
    let part = facts_parts_dir(&dir).join(&files[0]);
    let text = std::fs::read_to_string(&part).expect("读卷");
    let cut = text.rfind('\n').expect("卷以换行结尾");
    let cut = text[..cut].rfind('\n').expect("卷里不止一行") + 1;
    std::fs::write(&part, &text[..cut]).expect("写回短了一行的卷");
    assert_eq!(
        std::fs::read_to_string(&part)
            .expect("读卷")
            .lines()
            .count(),
        written - 1,
        "夹具前提：封口段真的少了一条"
    );

    // ① 开机照旧（它只读目录 ＋ 活动段：活动段那条的链值来自**目录**，因此这里看不出来）。
    let env = Environment::open(&dir.env(), agent.clone()).expect("开机照旧（不读旧卷）");
    // ② 整份读：活动段那条的校验和接着被删掉的那一条算 ⇒ 必须对不上。
    match env.facts() {
        Err(StoreError::ChecksumMismatch { .. }) => {}
        other => panic!("少了封口段的尾巴，链就该对不上，却得到 {other:?}"),
    }
}

/// **身份不在、只有封口段在场**时同样不许创建：封口段也是历史。
///
/// 判据与"两份活动段在场"同一条（`create` 只该在"这里没有历史"的位置上创建）：把旧卷留在原地
/// 再铸一个新身份，会造出一个"新身份 ＋ 旧历史"的环境。
#[test]
fn a_lost_identity_is_never_recreated_over_sealed_parts_only() {
    let dir = TempDir::new("facts-identity-gate");
    let agent = AgentId::new("agent-alpha");
    let (_, _) = write_facts_until_sealed(&dir, &agent);

    // 造出"身份不在、两份活动段也不在，只有封口段在"的形状。
    for name in ["identity.json", "trace.ndjson", "facts.ndjson"] {
        std::fs::remove_file(dir.env().join(name)).expect("挪走");
    }
    let part = facts_parts_dir(&dir).join(&sealed_fact_parts(&dir)[0]);
    let before = std::fs::read(&part).expect("卷还在");
    match Environment::create(&dir.env(), agent) {
        Err(StoreError::HistoryWithoutIdentity { files }) => {
            assert!(
                files.contains(&"facts.parts"),
                "拒绝时要说出是哪一份历史：{files:?}"
            );
        }
        other => panic!("身份不在、历史却在，必须拒绝创建，却得到 {other:?}"),
    }
    assert_eq!(
        std::fs::read(&part).expect("卷还在"),
        before,
        "拒绝时一个字节都不动"
    );
    assert!(
        !dir.env().join("identity.json").exists(),
        "拒绝时不铸新身份"
    );
}
/// **骨架行与摘要行是两行**：开机只解骨架行（那一串八字段的摘要是 O(全部活动) 的解析，
/// 正是"开机那一拍随历史变慢"的最后一处）。
///
/// 两半各钉一遍：**开机不依赖摘要行**（把它删掉/改坏，`open` 照旧成功、折好的标志照旧管用），
/// 而**要完整读数的路**（`list_runs`）在没有它时走重建把它补回来。
#[test]
fn the_directory_splits_the_skeleton_from_the_run_summaries() {
    let dir = TempDir::new("segment-split");
    let agent = AgentId::new("agent-alpha");
    let (files, _) = write_until_sealed(&dir, &agent);

    let index = std::fs::read_to_string(parts_index(&dir)).expect("读目录");
    let skeleton: Vec<&str> = index
        .lines()
        .filter(|line| line.contains("\"kind\":\"part\""))
        .collect();
    let summaries: Vec<&str> = index
        .lines()
        .filter(|line| line.contains("\"kind\":\"runs\""))
        .collect();
    assert_eq!(skeleton.len(), 1, "一卷一行骨架");
    assert_eq!(summaries.len(), 1, "一卷一行摘要");
    assert!(
        skeleton[0].contains("\"run-big\""),
        "骨架行折着这一卷的活动身份：{}",
        skeleton[0]
    );
    assert!(
        !skeleton[0].contains("\"ending\""),
        "骨架行上不该有完整摘要的字段"
    );
    assert!(
        summaries[0].contains("\"ending\""),
        "摘要行才是完整那一份：{}",
        summaries[0]
    );

    // ① 摘要行**改坏**（解不出来）：开机照旧成功——它根本不看这一行。
    let mut broken = String::new();
    for line in index.lines() {
        if line.contains("\"kind\":\"runs\"") {
            broken
                .push_str("{\"kind\":\"runs\",\"file\":\"000000000000.ndjson\",\"runs\":[{\"run\"");
        } else {
            broken.push_str(line);
        }
        broken.push('\n');
    }
    std::fs::write(parts_index(&dir), &broken).expect("写回改坏的摘要行");
    let mut env = Environment::open(&dir.env(), agent.clone()).expect("开机不看摘要行，照旧能开");
    assert_eq!(env.trace_records(), 2, "条数从骨架行里折好的那一格来");

    // ② **折好的标志照旧管用**：这一段已经登记过 ⇒ 再登记一次必须被拒（判据只从骨架行来）。
    match env.append_trace(
        &RunId::new("run-big"),
        TracePayload::RunStarted { origin: None },
    ) {
        Err(StoreError::RunAlreadyKnown { .. }) => {}
        other => panic!("重复登记必须被拒，却得到 {other:?}"),
    }

    // ③ **要完整读数的路**（`list_runs`）在摘要行读不出来时走重建：把完整摘要补回来。
    let runs = list_runs(&env).expect("列活动：摘要行坏了就重读该卷重建");
    let big = runs
        .iter()
        .find(|summary| summary.run().as_str() == "run-big")
        .unwrap_or_else(|| panic!("重建之后这一段照旧列得出来：{runs:?}"));
    assert_eq!(big.records(), 2, "条数照旧");
    assert_eq!(big.first_seq(), 0, "序号区间照旧");
    let rebuilt = std::fs::read_to_string(parts_index(&dir)).expect("读目录");
    assert!(
        rebuilt
            .lines()
            .filter(|line| line.contains("\"kind\":\"runs\""))
            .count()
            == 1,
        "重建把摘要行写了回来：{rebuilt}"
    );
    drop(env);

    // ④ **老形状**（摘要塞在骨架行里、没有摘要行）：目录与盘对不上 ⇒ 同样重建，一卷都不少。
    let old_shape = format!(
        "{{\"kind\":\"part\",\"file\":\"{}\",\"seq-start\":0,\"seq-end\":1,\"bytes\":0,\
         \"chain\":\"00000000\",\"records\":2,\"runs\":[[\"run-big\",1]],\
         \"paused\":null,\"unfinished\":null,\"acknowledged-through\":null}}\n",
        files[0]
    );
    std::fs::write(parts_index(&dir), old_shape).expect("写回老形状的目录");
    let env = Environment::open(&dir.env(), agent.clone()).expect("老形状必须走重建，而不是拒绝");
    assert_eq!(env.trace_records(), 2, "重建之后条数照旧");
    assert!(
        list_runs(&env)
            .expect("列活动")
            .iter()
            .any(|summary| summary.run().as_str() == "run-big"),
        "重建之后这一段照旧列得出来"
    );
}

/// **老形状的目录**（摘要塞在骨架行里、没有摘要行）⇒ 一律**重建**，一卷都不少、也不拒绝打开。
///
/// 判据：目录是**派生**的，而它的形状会随构建变（这一刀就把完整摘要挪到了另一行上）。把它读成
/// "环境坏了"，等于让一份派生文件**锁死**主体的记忆。夹具特意封出**两卷**：老形状里第二行之后
/// 还有行，若按"中间的行读不出来就是损坏"处理，真家（只要封过两卷）就会打不开。
#[test]
fn an_old_shape_directory_is_rebuilt_instead_of_refusing_to_open() {
    let dir = TempDir::new("segment-old-shape");
    let agent = AgentId::new("agent-alpha");
    let run = RunId::new("run-straddle");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    env.append_trace(&run, TracePayload::RunStarted { origin: None })
        .expect("登记");
    env.append_trace(
        &run,
        TracePayload::RunInput {
            text: huge_text("切点之前"),
        },
    )
    .expect("第一条大正文");
    env.append_trace(
        &run,
        TracePayload::RunInput {
            text: huge_text("切点之后"),
        },
    )
    .expect("第二条大正文");
    env.append_trace(
        &run,
        ending_payload(Outcome::Subject(SubjectEnding::Completed)),
    )
    .expect("收场");
    drop(env);
    let files = sealed_parts(&dir);
    assert_eq!(
        files.len(),
        2,
        "夹具前提：要封出两卷（老形状的第二行之后还有行）"
    );

    // 老形状：一行一卷，摘要（八字段对象）就塞在骨架行里。
    let mut old = String::new();
    for (index, file) in files.iter().enumerate() {
        old.push_str(&format!(
            "{{\"kind\":\"part\",\"file\":\"{file}\",\"seq-start\":{index},\"seq-end\":{index},\
             \"bytes\":0,\"chain\":\"00000000\",\
             \"runs\":[{{\"run\":\"run-straddle\",\"records\":2,\"first-seq\":0,\"last-seq\":1,\
             \"ending\":null,\"failure\":null,\"sealed\":false,\"started\":true}}],\
             \"paused\":null,\"unfinished\":null,\"acknowledged-through\":null}}\n"
        ));
    }
    std::fs::write(parts_index(&dir), old).expect("写回老形状的目录");

    let env = Environment::open(&dir.env(), agent.clone()).expect("老形状必须走重建，而不是拒绝");
    assert_eq!(env.trace_records(), 4, "重建之后条数照旧");
    assert_eq!(
        env.read_run(&run, None, 50)
            .expect("读这一段")
            .records()
            .len(),
        4,
        "重建之后这一段照旧收全（横跨两卷 ＋ 活动段）"
    );
    let rebuilt = std::fs::read_to_string(parts_index(&dir)).expect("读目录");
    assert_eq!(
        rebuilt
            .lines()
            .filter(|line| line.contains("\"kind\":\"runs\""))
            .count(),
        2,
        "重建把两卷的摘要行都写了回来：{rebuilt}"
    );
}

/// 一行**能解析**但内容不合法：解出来也没有可解释的含义，必须按"不合法条目"拒绝。
///
/// 这与"读不出来"是两类问题，必须分开报告——但都**拒绝打开**：不能用"就近猜一个含义"
/// 把非法内容读成有效历史。
#[test]
fn an_entry_that_parses_but_cannot_be_interpreted_is_refused() {
    let dir = TempDir::new("malformed-entry");
    let agent = AgentId::new("agent-alpha");
    let run = RunId::new("run-1");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    env.append_trace(&run, TracePayload::RunStarted { origin: None })
        .expect("经历：登记");
    env.append_fact(
        &run,
        &Record::RunStarted {
            agent: agent.clone(),
            run: run.clone(),
        },
    )
    .expect("事实：登记");
    drop(env);

    let trace_path = dir.env().join("trace.ndjson");
    let facts_path = dir.env().join("facts.ndjson");
    let trace = confirmed_lines_of(&trace_path);

    let trace_cases = [
        (
            "两种收场同时存在（自相矛盾）",
            r#"{"seq":1,"run":"run-1","kind":"ending","subject":"completed","system":"timeout"}"#,
        ),
        (
            "什么收场都没说（无从解释）",
            r#"{"seq":1,"run":"run-1","kind":"ending"}"#,
        ),
        (
            "收场取值不在已知词汇表里",
            r#"{"seq":1,"run":"run-1","kind":"ending","subject":"whatever"}"#,
        ),
        // 模型响应：决定是枚举，因此"取值不在封闭词汇表里"无法被解释成任何一种决定。
        (
            "决定取值不在封闭词汇表里（例如旧实现里的自动继续）",
            r#"{"seq":1,"run":"run-1","kind":"model-response","turn":1,"content":"正文","decision":"continue"}"#,
        ),
        (
            "act 却同时带结束取值（两份互相矛盾的意图）",
            r#"{"seq":1,"run":"run-1","kind":"model-response","turn":1,"content":"","decision":"act","action":"files_list","ending":"completed"}"#,
        ),
        (
            "act 却没有行动标识",
            r#"{"seq":1,"run":"run-1","kind":"model-response","turn":1,"content":"","decision":"act"}"#,
        ),
        (
            "finish 却带着行动参数",
            r#"{"seq":1,"run":"run-1","kind":"model-response","turn":1,"content":"","decision":"finish","ending":"silent","params":[{"label":"path","text":"a.txt"}]}"#,
        ),
        (
            "finish 的结束取值不在封闭词汇表里",
            r#"{"seq":1,"run":"run-1","kind":"model-response","turn":1,"content":"","decision":"finish","ending":"maybe"}"#,
        ),
        (
            "yield 却带着行动",
            r#"{"seq":1,"run":"run-1","kind":"model-response","turn":1,"content":"正文","decision":"yield","action":"files_list"}"#,
        ),
        (
            "续接数据的协议名为空",
            r#"{"seq":1,"run":"run-1","kind":"model-response","turn":1,"content":"","decision":"act","action":"files_list","protocol":"","protocol-version":1,"payload":"{}"}"#,
        ),
    ];
    for (label, line) in trace_cases {
        let content = format!("{}\n{line}\n", trace[0]);
        overwrite(&trace_path, &content);
        match Environment::open(&dir.env(), agent.clone()) {
            Err(error @ StoreError::MalformedEntry { seq }) => {
                assert_eq!(seq, 1, "{label}：应指出是哪一条");
                assert!(error.is_structural_damage(), "{label}：这是结构损坏");
            }
            other => panic!("{label}：必须被拒绝为不合法条目，却得到 {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(&trace_path).expect("读回"),
            content,
            "{label}：拒绝不得改写文件"
        );
    }

    // 恢复经历侧，再单独查事实侧。
    overwrite(&trace_path, &format!("{}\n", trace[0]));
    Environment::open(&dir.env(), agent.clone()).expect("经历侧恢复之后应能打开");

    let fact_line0 = confirmed_lines_of(&facts_path)
        .into_iter()
        .next()
        .expect("事实存储应至少有一条已确认记录");
    let fact_cases = [
        (
            "收场事实什么收场都没说",
            r#"{"seq":1,"run":"run-1","fact":"run-ended"}"#,
        ),
        (
            "收场取值不在已知词汇表里",
            r#"{"seq":1,"run":"run-1","fact":"run-ended","subject":"whatever"}"#,
        ),
        (
            "不认识的种类名",
            r#"{"seq":1,"run":"run-1","fact":"something-new"}"#,
        ),
        (
            "失败分类取值不在封闭词汇表里",
            r#"{"seq":1,"run":"run-1","fact":"run-ended","system":"model-failed","failure":"no-such-class"}"#,
        ),
        (
            "失败分类用了 App 私有词表里的名字",
            r#"{"seq":1,"run":"run-1","fact":"run-ended","system":"model-failed","failure":"already-exists"}"#,
        ),
        (
            "结局不是模型失败却带着分类（自相矛盾）",
            r#"{"seq":1,"run":"run-1","fact":"run-ended","system":"timeout","failure":"transport-failure"}"#,
        ),
        (
            "分类出现在非收场行上",
            r#"{"seq":1,"run":"run-1","fact":"action-prepared","turn":1,"failure":"transport-failure"}"#,
        ),
    ];
    for (label, line) in fact_cases {
        let content = format!("{fact_line0}\n{line}\n");
        overwrite(&facts_path, &content);
        match Environment::open(&dir.env(), agent.clone()) {
            Err(StoreError::MalformedEntry { seq }) => assert_eq!(seq, 1, "{label}"),
            other => panic!("{label}：必须被拒绝为不合法条目，却得到 {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(&facts_path).expect("读回"),
            content,
            "{label}：拒绝不得改写文件"
        );
    }
}

/// 一条**这一行读不出来**的经历（种类名不认识、或必须一起出现的几个字段只写了一部分）：
/// 环境拒绝打开。
///
/// 这是"旧构建拒绝新记录"在本仓库里**可验证**的那一半：解码表是封闭的（`decode_trace` 对
/// 不认识的种类直接返回"读不出来"，没有兜底分支），因此一个没有 `model-response` 分支的旧
/// 写入者读到新记录时走的正是这条路——拒绝打开，而不是把它当成一条无害的未知记录读过去。
/// 这一条**不**声称旧构建被真的跑过：这里注入的是一个合成种类名，效果与旧构建看到新种类名
/// 相同（同一段解码代码、同一个拒绝分类），但不是同一个二进制的实测。
#[test]
fn a_trace_line_outside_the_writers_table_is_refused_rather_than_tolerated() {
    let dir = TempDir::new("unknown-kind");
    let agent = AgentId::new("agent-alpha");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    env.append_trace(
        &RunId::new("run-1"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("经历：登记");
    drop(env);

    let trace_path = dir.env().join("trace.ndjson");
    let trace = confirmed_lines_of(&trace_path);

    for (label, line) in [
        (
            "旧构建读不到的模型响应记录（用另一个种类名模拟同一个解码分支）",
            r#"{"seq":1,"run":"run-1","kind":"model-response-v2","turn":1,"content":"正文","decision":"yield"}"#,
        ),
        (
            "完全陌生的种类名",
            r#"{"seq":1,"run":"run-1","kind":"something-new"}"#,
        ),
        (
            // 续接数据的三个字段必须**一起**出现，只有一部分连"读出来"这一步都过不去：
            // 这与"读得出来但含义不合法"是两类问题，因此落在同一个读不出来的分类上。
            "续接数据只写了一半（协议名没有版本与载荷）",
            r#"{"seq":1,"run":"run-1","kind":"model-response","turn":1,"content":"","decision":"act","action":"files_list","protocol":"chat-completions"}"#,
        ),
    ] {
        let content = format!("{}\n{line}\n", trace[0]);
        overwrite(&trace_path, &content);
        match Environment::open(&dir.env(), agent.clone()) {
            Err(error @ StoreError::CorruptTail { seq }) => {
                assert_eq!(seq, 1, "{label}：应指出读不出来的是哪一条");
                assert!(error.is_structural_damage(), "{label}：这是结构损坏");
            }
            other => panic!("{label}：必须被拒绝，却得到 {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(&trace_path).expect("读回"),
            content,
            "{label}：拒绝不得改写文件"
        );
    }

    // 对照：同一条记录写成**本写入者认识**的种类名就是合法的——拒绝的原因只是"认不出"，
    // 不是"模型响应本身不被接受"。
    let known = format!(
        "{}\n{}\n",
        trace[0],
        r#"{"seq":1,"run":"run-1","kind":"model-response","turn":1,"content":"正文","decision":"yield"}"#
    );
    overwrite(&trace_path, &known);
    Environment::open(&dir.env(), agent.clone()).expect("认识这个种类名时应当能打开");
}

/// 已建立的环境缺一份存储文件：拒绝，不按空历史重建。
#[test]
fn an_established_environment_missing_a_storage_file_is_refused_not_rebuilt() {
    let dir = TempDir::new("storage-missing");
    let agent = AgentId::new("agent-alpha");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    env.append_trace(
        &RunId::new("run-1"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("写一条");
    drop(env);

    std::fs::remove_file(dir.env().join("facts.ndjson")).expect("删掉事实存储");

    match Environment::open(&dir.env(), agent.clone()) {
        Err(StoreError::StorageMissing { file }) => assert_eq!(file, "facts.ndjson"),
        other => panic!("缺存储文件必须被拒绝，而不是按空历史打开，却得到 {other:?}"),
    }
    // 不得顺手建一份空的：文件必须**仍然**不存在，人才能去看。
    assert!(
        !dir.env().join("facts.ndjson").exists(),
        "拒绝不等于顺手重建空文件"
    );

    // 经历侧同理。
    std::fs::remove_file(dir.env().join("trace.ndjson")).expect("删掉经历存储");
    match Environment::open(&dir.env(), agent.clone()) {
        Err(StoreError::StorageMissing { file }) => assert_eq!(file, "trace.ndjson"),
        other => panic!("缺经历存储也必须被拒绝，却得到 {other:?}"),
    }
}

/// 同一个目录上的并发创建：排他保护先于身份检查，因此只能有一个赢家。
#[test]
fn two_racing_creators_cannot_both_win() {
    use std::sync::{Arc, Barrier};

    let dir = TempDir::new("racing-creators");
    let env_dir = dir.env();
    std::fs::create_dir_all(&env_dir).expect("建立环境目录");

    let barrier = Arc::new(Barrier::new(4));
    let mut handles = Vec::new();
    for index in 0..4 {
        let env_dir = env_dir.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            let agent = AgentId::new(format!("agent-{index}"));
            Environment::create(&env_dir, agent).is_ok()
        }));
    }
    let winners: usize = handles
        .into_iter()
        .map(|handle| handle.join().expect("线程不应 panic"))
        .filter(|won| *won)
        .count();

    assert_eq!(winners, 1, "并发创建只能有一个赢家");

    // 赢家的身份必须完整、可读，没有被别人覆盖成混合状态。
    let recorded = std::fs::read_to_string(env_dir.join("identity.json")).expect("读身份");
    let winner = (0..4)
        .map(|index| format!("agent-{index}"))
        .find(|name| recorded.contains(name.as_str()));
    let winner = winner.expect("身份记录必须是某一个完整身份");
    Environment::open(&env_dir, AgentId::new(winner)).expect("赢家的身份必须能打开");
}

/// 封存之后拒绝继续追加该活动的经历；**新**活动仍然可以追加。
#[test]
fn a_sealed_activity_refuses_further_appends_while_a_new_one_is_allowed() {
    let dir = TempDir::new("sealed-append");
    let agent = AgentId::new("agent-alpha");
    let run = RunId::new("run-1");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    env.append_trace(&run, TracePayload::RunStarted { origin: None })
        .expect("登记活动");
    env.append_trace(&run, TracePayload::Sealed).expect("封存");

    match env.append_trace(
        &run,
        TracePayload::RunInput {
            text: "太晚了".into(),
        },
    ) {
        Err(StoreError::RunSealed { run: found }) => assert_eq!(found, run),
        other => panic!("封存之后不得再追加该活动的经历，却得到 {other:?}"),
    }

    // 新活动不受影响。
    env.append_trace(
        &RunId::new("run-2"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("新活动仍可追加");
}

/// 跨存储矛盾：各自自洽、只是进度不一致——必须**报告**，不能用相同 RunId 冒充事务一致。
#[test]
fn a_determinable_cross_store_contradiction_is_reported() {
    let dir = TempDir::new("cross-store");
    let agent = AgentId::new("agent-alpha");
    let run = RunId::new("run-1");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    // 只写经历侧的"开始"与"结束"，事实侧故意只写开始：这就是"分几步写、只写成前几步"。
    env.append_trace(&run, TracePayload::RunStarted { origin: None })
        .expect("经历：开始");
    env.append_fact(
        &run,
        &Record::RunStarted {
            agent: agent.clone(),
            run: run.clone(),
        },
    )
    .expect("事实：开始");
    env.append_trace(
        &run,
        ending_payload(Outcome::Subject(SubjectEnding::Silent)),
    )
    .expect("经历：结束");

    let issues = env.cross_store_issues().expect("跨存储读数");
    assert_eq!(issues.len(), 1, "应当报出恰好一条可判定的矛盾：{issues:?}");
    assert_eq!(issues[0].run(), &run);
    assert_eq!(issues[0].kind(), "ending-without-fact");

    // 它**不是**结构损坏：两份存储各自都能完整读出。
    assert!(env.trace().is_ok());
    assert!(env.facts().is_ok());
    let health = env.health().expect("健康读数");
    assert_eq!(health.cross_store().len(), 1);
}

/// 反方向同样可判定：事实侧写了锚点、经历侧没有——必须报告，不得冒充一致。
///
/// 正常写入顺序是"经历先写、事实后写"，所以这个方向产生不出来；它意味着文件被外部改动过。
/// 能被判定的矛盾就必须被说出来，不能因为"另一方向很干净"而沉默。
#[test]
fn a_fact_without_its_trace_anchor_is_reported_too() {
    let dir = TempDir::new("cross-store-reverse");
    let agent = AgentId::new("agent-alpha");
    let run = RunId::new("run-1");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    // 只写事实侧：经历侧一个锚点都没有。
    env.append_fact(
        &run,
        &Record::RunStarted {
            agent: agent.clone(),
            run: run.clone(),
        },
    )
    .expect("事实：开始");

    let issues = env.cross_store_issues().expect("跨存储读数");
    assert_eq!(issues.len(), 1, "事实侧的开始没有经历锚点：{issues:?}");
    assert_eq!(issues[0].run(), &run);
    assert_eq!(issues[0].kind(), "fact-without-start");

    env.append_fact(
        &run,
        &Record::RunEnded {
            run: run.clone(),
            outcome: Outcome::Subject(SubjectEnding::Silent),
            cost: celira_agent::cost::RunCost::default(),
        },
    )
    .expect("事实：结束");

    let kinds: Vec<&str> = env
        .cross_store_issues()
        .expect("跨存储读数")
        .iter()
        .map(|issue| issue.kind())
        .collect();
    assert_eq!(
        kinds,
        ["fact-without-start", "fact-without-ending"],
        "两个锚点都要各自报告"
    );

    // 依然不是结构损坏：两份存储各自都读得出来。
    assert!(env.trace().is_ok());
    assert!(env.facts().is_ok());
    drop(env);

    // 重开之后矛盾仍然可查，而不是被"新的进程"洗掉。
    let reopened = Environment::open(&dir.env(), agent.clone()).expect("重开");
    assert_eq!(
        reopened.health().expect("健康读数").cross_store().len(),
        2,
        "重开之后矛盾仍在"
    );
}

/// 把路径设成/取消"只读"（Windows 上就是只读属性；Unix 上就是去掉写位）。
fn set_readonly(path: &std::path::Path, readonly: bool) {
    let mut permissions = std::fs::metadata(path).expect("读权限").permissions();
    permissions.set_readonly(readonly);
    std::fs::set_permissions(path, permissions).expect("改权限");
}

/// **真实存储路径**上的 I/O 故障：拒绝打开，并且什么都不改。
///
/// 这不是接缝替身，而是真的把一个存储文件变成不可写，再走真实的打开路径。
#[test]
fn a_real_io_fault_on_the_storage_path_refuses_to_open_and_changes_nothing() {
    let dir = TempDir::new("real-io-fault");
    let agent = AgentId::new("agent-alpha");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    env.append_trace(
        &RunId::new("run-1"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("写一条");
    drop(env);

    let trace_path = dir.env().join("trace.ndjson");
    let before = std::fs::read(&trace_path).expect("读回");
    set_readonly(&trace_path, true);

    match Environment::open(&dir.env(), agent.clone()) {
        Err(error) => assert!(
            !error.is_structural_damage(),
            "这是 I/O 拒绝，不是「文件不自洽」：{error:?}"
        ),
        Ok(_) => panic!("不可写的存储文件不得被当成可用环境打开"),
    }

    set_readonly(&trace_path, false);
    let after = std::fs::read(&trace_path).expect("再次读回");
    assert_eq!(after, before, "故障不得改动任何已有内容");
    // 恢复可写之后必须还能正常打开：被拒绝过的环境不会因此变成另一种形状。
    assert!(
        Environment::open(&dir.env(), agent.clone()).is_ok(),
        "排除故障之后应当能重新打开"
    );
}

/// 故障注入必须**被验证**，不能被假定——这段断言本身就是验证。
///
/// 本平台上实测到的两件事：
///
/// - 打开**之前**把存储文件设成只读 → 打开被拒绝（见上一条测试，那是有效的故障注入）；
/// - 打开**之后**再设只读 → 写入**照样成功**。
///
/// 所以"把文件/目录设成只读"**不能**当作"写入中途失败"的替身：它根本不制造失败。
/// 想验证"写入中途失败"，只能靠中立接缝的故障替身，或者声明这个缺口——不能拿它假装。
#[test]
fn a_readonly_flag_is_not_by_itself_a_write_fault() {
    let dir = TempDir::new("readonly-not-a-fault");
    let agent = AgentId::new("agent-alpha");
    let run = RunId::new("run-1");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    env.append_trace(&run, TracePayload::RunStarted { origin: None })
        .expect("写一条");

    // 文件只读：已打开的句柄仍然写得进去。
    let trace_path = dir.env().join("trace.ndjson");
    set_readonly(&trace_path, true);
    let after_file_readonly = env.append_trace(
        &run,
        TracePayload::RunInput {
            text: "文件只读也写得进去".into(),
        },
    );
    set_readonly(&trace_path, false);
    assert!(
        after_file_readonly.is_ok(),
        "只读标记不阻止已打开句柄写入，因此它不是有效的写入故障注入"
    );

    // 目录只读：同理，写已有文件不受影响。
    let env_dir = dir.env();
    set_readonly(&env_dir, true);
    let after_dir_readonly = env.append_trace(
        &run,
        TracePayload::RunInput {
            text: "目录只读也写得进去".into(),
        },
    );
    set_readonly(&env_dir, false);
    assert!(
        after_dir_readonly.is_ok(),
        "只读目录不阻止写已有文件，因此它不是有效的写入故障注入（这是被验证过的事实，不是假设）"
    );
}

/// 对一份存储文件持有**排他字节范围锁**的干扰者。锁在 `Drop` 时释放。
///
/// 这是"真实写入被拒"的注入手段。本平台上（Windows）字节范围锁是**强制**的：另一个句柄
/// 往被锁区间里写会真的失败。只读属性做不到这件事——那已经在上一条测试里被验证过了。
#[cfg(windows)]
struct ByteRangeBlocker {
    handle: std::fs::File,
}

#[cfg(windows)]
impl ByteRangeBlocker {
    fn hold(path: &std::path::Path) -> Self {
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .expect("打开第二个句柄");
        handle.try_lock().expect("取得排他字节范围锁");
        Self { handle }
    }
}

#[cfg(windows)]
impl Drop for ByteRangeBlocker {
    fn drop(&mut self) {
        let _ = self.handle.unlock();
    }
}

/// **写入被真实拒绝**之后：不再继续追加，盘上只留可证实的前缀，重开靠"未收尾"兜住。
///
/// README 此前把"一次写入失败之后继续追加会把损坏滚大"这条防线列为**无法触发**的缺口；
/// 这条测试用真实的 I/O 拒绝把它补上，并且顺带验证"暂停标记自己也写不下去"时会发生什么：
/// 保护不会失效，只是理由从"已记录的存储故障"变成"未收尾、原因没有被记录"。
///
/// 只在 Windows 上跑：别的平台上字节范围锁只是建议性的，这个注入**不产生故障**，
/// 那样写出来的会是假证据——所以宁可不跑，也不假装。
#[test]
#[cfg(windows)]
fn a_real_write_refusal_stops_further_appends_and_leaves_only_verifiable_state() {
    let dir = TempDir::new("write-refusal");
    let agent = AgentId::new("agent-alpha");
    let run = RunId::new("run-1");

    let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
    env.append_trace(&run, TracePayload::RunStarted { origin: None })
        .expect("登记活动");

    let trace_path = dir.env().join("trace.ndjson");
    let blocker = ByteRangeBlocker::hold(&trace_path);

    // 真实拒绝：这一行没有写进去，也没有被确认为耐久。
    let refused = env.append_trace(
        &run,
        TracePayload::RunInput {
            text: "这一行写不进去".into(),
        },
    );
    let error = refused.expect_err("被锁住的写入必须真的失败");
    assert_eq!(error, StoreError::Io, "这是底层 I/O 拒绝：{error:?}");
    assert!(
        !error.is_structural_damage(),
        "这是 I/O 拒绝，不是「文件不自洽」：{error:?}"
    );

    // 防线：同一次写入失败之后，这个写者**不再**往同一份文件里追加。
    assert_eq!(
        env.append_trace(
            &run,
            TracePayload::RunInput {
                text: "第二次".into(),
            },
        ),
        Err(StoreError::TailUnconfirmed),
        "写入没有停在已确认的行边界之后，必须停止继续追加"
    );

    // 暂停标记也写进同一份文件，所以它也写不下去——这正是"标记自己也失败"的形状。
    assert_eq!(
        env.mark_paused(&run, "save-failed:trace"),
        Err(StoreError::TailUnconfirmed),
        "暂停标记写不下去时，保护只能靠可验证的未收尾状态兜住"
    );

    drop(blocker);

    // 盘上只留下可证实的前缀：失败的那些行一个字节都没有落进去，前缀停在换行边界上。
    let on_disk = std::fs::read_to_string(&trace_path).expect("读回经历");
    assert_eq!(
        on_disk.lines().count(),
        1,
        "只有已确认的那一条：{on_disk:?}"
    );
    assert!(on_disk.ends_with('\n'), "已确认前缀必须停在换行边界上");
    drop(env);

    // 重开：安全由"没有收尾的活动"保持，而不是由那条写不下去的标记保持。
    let mut reopened = Environment::open(&dir.env(), agent.clone()).expect("重开");
    assert_eq!(
        reopened.admission().expect("准入读数"),
        Admission::Unfinished { run: run.clone() },
        "标记写不下去时，未收尾状态必须兜住"
    );
    assert!(
        paused_reason(reopened.root())
            .expect("读暂停原因")
            .is_none(),
        "这一次**没有**任何暂停记录，因此不得被说成「已记录的存储故障」"
    );

    // 真实入口：**不再拒绝**新活动（2026-09-26 新规矩，理由见上面那条用例）。
    let sandbox = dir.path.join("sandbox");
    std::fs::create_dir_all(&sandbox).expect("建沙盒");
    let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
    run_activity_in_environment(
        &sandbox,
        &mut reopened,
        EnvironmentActivity {
            declared_agent: &agent,
            run: RunId::new("run-2"),
            limits: Limits::new(4),
            input: RunInput::empty(),
            source: &mut ScriptedSource::new(None, vec![None]),
            creation: None,
        },
        &mut model,
    )
    .expect("没有收尾的旧活动不再拦住新活动");
    assert!(
        model.calls() >= 1,
        "放行之后模型该被调用；而**不得把它说成已记录的存储故障**这半句由 `Paused` 那条路守着"
    );
    // 放行不等于解决：旧的那一段仍然如实留在盘上。
    assert!(
        list_runs(&reopened)
            .expect("列出活动")
            .iter()
            .any(|run| run.status() == "interrupted"),
        "旧的那一段仍然如实呈现为 interrupted"
    );
}

/// **Unix 上的同一件事**：真实 I/O 拒绝（进程级文件大小上限）之后，行为与上面那条**逐条对齐**。
///
/// 为什么机制不同：Windows 那条靠"另一个句柄持排他字节范围锁"；Unix 上字节范围锁是**建议性**的
/// （不产生故障，写出来的是假证据），等价手段是 `RLIMIT_FSIZE` ＋ 忽略 `SIGXFSZ`——写超出上限的
/// 那一次由**内核**拒绝（`EFBIG`），已写下的前缀照旧在盘上，正是这条契约要的形状。
///
/// **更正**：README 曾把这一格记成"这个注入只在 Windows 上成立，别的平台没有等价手段"。
/// 那句话不成立——Unix 有等价手段，这一条就是它的证据。
///
/// 为什么拒绝由**独立进程**制造：`RLIMIT_FSIZE` 是**进程级**的，在测试进程里设它会把并行的
/// 别的用例一起拖下水（`cargo test` 默认并行）。夹具只做注入与读数；"盘上只剩可证实前缀"与
/// "重开靠未收尾兜住"这两件在夹具退出、环境放开之后由这里自己断言。
///
/// 先跑**正对照**（同一支夹具、同一个环境形状，只不装那道上限）：没有它，"拒绝了"就可能来自
/// 别的原因（身份不对、写资格没放开、夹具根本没跑起来）。
#[cfg(unix)]
#[test]
fn a_real_write_refusal_stops_further_appends_and_leaves_only_verifiable_state() {
    let agent = AgentId::new("agent-alpha");
    let run = RunId::new("run-1");

    // ---- 正对照：不装上限 ⇒ 三笔都真的写进去了 ----
    let control = TempDir::new("write-refusal-unix-control");
    {
        let mut env = Environment::create(&control.env(), agent.clone()).expect("创建环境");
        env.append_trace(&run, TracePayload::RunStarted { origin: None })
            .expect("登记活动");
    }
    let control_readings = run_write_refusal_fixture(&control, &agent, &run, "control");
    assert!(
        control_readings.contains("refused-append = ok"),
        "正对照里这三笔都该成功：{control_readings}"
    );
    assert!(
        control_readings.contains("second-append = ok"),
        "{control_readings}"
    );
    assert!(
        control_readings.contains("mark-paused = ok"),
        "{control_readings}"
    );
    let written = std::fs::read_to_string(control.env().join("trace.ndjson")).expect("读回经历");
    assert_eq!(
        written.lines().count(),
        4,
        "正对照里三笔真的都落盘了（两条输入 ＋ **暂停标记自己也写进去了**）：{written:?}"
    );
    assert!(
        written.contains("save-failed:trace"),
        "没有故障时，暂停标记是**写得进去**的——所以「标记写不下去」确实是那次拒绝的结果：{written:?}"
    );

    // ---- 真的那一次：上限 = 已确认前缀的大小 ⇒ 此后一个字节都写不进去 ----
    let dir = TempDir::new("write-refusal-unix");
    {
        let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
        env.append_trace(&run, TracePayload::RunStarted { origin: None })
            .expect("登记活动");
    } // 放开写资格：拒绝要由**另一个进程**制造

    let readings = run_write_refusal_fixture(&dir, &agent, &run, "limit");
    assert!(
        readings.contains("refused-append = io"),
        "被上限挡住的那一行必须是底层 I/O 拒绝：{readings}"
    );
    assert!(
        readings.contains("second-append = tail-unconfirmed"),
        "写入没有停在已确认的行边界之后，必须停止继续追加：{readings}"
    );
    assert!(
        readings.contains("mark-paused = tail-unconfirmed"),
        "暂停标记写进同一份文件，所以它也写不下去：{readings}"
    );

    // 盘上只留下可证实的前缀：失败的那些行一个字节都没有落进去，前缀停在换行边界上。
    let on_disk = std::fs::read_to_string(dir.env().join("trace.ndjson")).expect("读回经历");
    assert_eq!(
        on_disk.lines().count(),
        1,
        "只有已确认的那一条：{on_disk:?}"
    );
    assert!(on_disk.ends_with('\n'), "已确认前缀必须停在换行边界上");

    // 重开：安全由"没有收尾的活动"保持，而不是由那条写不下去的标记保持。
    let reopened = Environment::open(&dir.env(), agent.clone()).expect("重开");
    assert_eq!(
        reopened.admission().expect("准入读数"),
        Admission::Unfinished { run: run.clone() },
        "标记写不下去时，未收尾状态必须兜住"
    );
    assert!(
        paused_reason(reopened.root())
            .expect("读暂停原因")
            .is_none(),
        "这一次**没有**任何暂停记录，因此不得被说成「已记录的存储故障」"
    );
    assert!(
        list_runs(&reopened)
            .expect("列出活动")
            .iter()
            .any(|each| each.status() == "interrupted"),
        "旧的那一段仍然如实呈现为 interrupted"
    );
}

/// 跑一次 `write_refusal_fixture`，把它的读数原样带回来（退出码不是 0 就算这次取证失败）。
#[cfg(unix)]
fn run_write_refusal_fixture(dir: &TempDir, agent: &AgentId, run: &RunId, mode: &str) -> String {
    let output = Command::new(binary("write_refusal_fixture"))
        .env("CELIRA_WRITE_REFUSAL_ROOT", dir.env())
        .env("CELIRA_WRITE_REFUSAL_AGENT", agent.as_str())
        .env("CELIRA_WRITE_REFUSAL_RUN", run.as_str())
        .env("CELIRA_WRITE_REFUSAL_MODE", mode)
        .output()
        .expect("夹具跑得起来");
    let readings = String::from_utf8_lossy(&output.stdout).into_owned();
    let complaints = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "夹具（{mode}）退出码不是 0：{readings}｜stderr：{complaints}"
    );
    readings
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

/// `run-ended` 上那个**可选**的失败分类：认得的取值如实带出来，缺字段是**旧记录**（不编造），
/// 而"带分类的模型失败"在恢复报告里多一行 note（跨进程调查不必去读 stderr）。
///
/// 三种形态共用同一条收场事实，只差一个字段：这就是"D3 的 `Option`"全部的含义。
#[test]
fn the_optional_failure_class_is_read_back_and_a_missing_one_is_not_invented() {
    use celira_agent::core::ModelFailureClass;
    use celira_agent::foundation::SystemEnding;

    let agent = AgentId::new("agent-alpha");
    let run = RunId::new("run-1");
    let cases = [
        (
            "缺字段（旧记录）",
            r#"{"seq":1,"run":"run-1","fact":"run-ended","system":"model-failed"}"#,
            None,
        ),
        (
            "认得的分类（**旧词**：无细分的形态 —— 解码必须认它，语义是「已知但未细分」）",
            r#"{"seq":1,"run":"run-1","fact":"run-ended","system":"model-failed","failure":"http-status-failure"}"#,
            Some(ModelFailureClass::HttpStatus(None)),
        ),
        (
            "另一类认得的分支",
            r#"{"seq":1,"run":"run-1","fact":"run-ended","system":"model-failed","failure":"unpaired-history"}"#,
            Some(ModelFailureClass::UnpairedHistory),
        ),
    ];

    for (label, ended_line, expected) in cases {
        let dir = TempDir::new(&format!("optional-failure-{expected:?}"));
        let mut env = Environment::create(&dir.env(), agent.clone()).expect("创建环境");
        env.append_trace(&run, TracePayload::RunStarted { origin: None })
            .expect("经历：登记");
        env.append_trace(
            &run,
            TracePayload::Ending {
                subject: None,
                system: Some("model-failed".to_string()),
                // 这条用例测的是**事实侧**的可选分类；经历侧这条收场不带分类
                // （它自己的 Optional 形态由 store 的往返用例覆盖）。
                failure: None,
            },
        )
        .expect("经历：收场");
        env.append_fact(
            &run,
            &Record::RunStarted {
                agent: agent.clone(),
                run: run.clone(),
            },
        )
        .expect("事实：登记");
        drop(env);

        let trace_path = dir.env().join("trace.ndjson");
        let facts_path = dir.env().join("facts.ndjson");
        let trace_lines = confirmed_lines_of(&trace_path);
        assert!(trace_lines.len() >= 2, "经历：登记 + 收场");
        let fact_line = confirmed_lines_of(&facts_path)
            .into_iter()
            .next()
            .expect("事实存储应至少有一条已确认记录");
        overwrite(&facts_path, &format!("{fact_line}\n{ended_line}\n"));

        let env = Environment::open(&dir.env(), agent.clone())
            .unwrap_or_else(|error| panic!("{label}：必须能打开，却得到 {}", error.as_str()));
        let state = celira_agent::recovery::inspect(&env.reader(), &run);
        assert_eq!(state.as_str(), "ended", "{label}：{state:?}");
        match state {
            celira_agent::recovery::RunState::Ended { outcome, .. } => {
                assert_eq!(
                    outcome,
                    Outcome::System(SystemEnding::ModelFailed(
                        expected.map(|class| class.as_str())
                    )),
                    "{label}：分类必须如实读出（缺字段就是 None，不编造）"
                );
            }
            other => panic!("{label}：应为 ended，得到 {other:?}"),
        }
        // ⚠ **2026-09-26 改**：这里原来还断言"恢复报告里那一行 `note=model-failure=`（有分类才加）"。
        // `RecoveryReport::render` 与恢复那条路一起退役了，而**同一条不变量一个字没松**：
        // 分类现在直接读自 `RunState::Ended { outcome }`——上面那条 `assert_eq!` 就是它，
        // 而且更强（它判的是结构化取值，不是一行文本里有没有某个子串）。
        // 只读：两份存储逐字节不变。
        assert_eq!(
            std::fs::read_to_string(&facts_path).expect("读回事实"),
            format!("{fact_line}\n{ended_line}\n"),
            "{label}：判定与报告都不得改写存储"
        );
        assert_eq!(
            confirmed_lines_of(&trace_path),
            trace_lines,
            "{label}：判定与报告都不得改写经历存储"
        );
    }
}

/// 经历侧收场的 `failure`：**带分类就往返保留，缺字段不编造**。
///
/// 与事实侧 `run-ended` 的 `failure` 同一口径。用途很具体：主体读自己的经历时，
/// 「那次失败属于哪一类」只能从这一处得到 —— 事实侧一直带着分类，但事实不是主体的动词。
#[test]
fn the_trace_ending_keeps_the_failure_class_and_never_invents_one() {
    use celira_agent::core::ModelFailureClass;

    let dir = TempDir::new("ending-failure");
    let agent = AgentId::new("agent-alpha");
    let mut env = Environment::create(&dir.env(), agent).expect("创建环境");

    let with_class = RunId::new("run-with-class");
    let unrecorded = RunId::new("run-unrecorded");
    let cases = [
        (
            &with_class,
            Outcome::System(SystemEnding::ModelFailed(Some(
                ModelFailureClass::Transport(None).as_str(),
            ))),
        ),
        (
            &unrecorded,
            Outcome::System(SystemEnding::ModelFailed(None)),
        ),
    ];
    for (run, outcome) in cases {
        env.append_trace(run, TracePayload::RunStarted { origin: None })
            .expect("经历：登记");
        env.append_trace(run, ending_payload(outcome))
            .expect("经历：收场");
    }

    // 1. 摘要（主体读经历时走的就是这条路）：带分类的带得出，没分类的**不**编造。
    let runs = list_runs(&env).expect("列活动");
    let found = runs
        .iter()
        .find(|summary| summary.run() == &with_class)
        .expect("带分类的活动有摘要");
    assert_eq!(found.ending(), Some("system:model-failed"));
    assert_eq!(found.failure(), Some("transport-failure"));
    let bare = runs
        .iter()
        .find(|summary| summary.run() == &unrecorded)
        .expect("没分类的活动有摘要");
    assert_eq!(bare.ending(), Some("system:model-failed"));
    assert_eq!(bare.failure(), None, "缺分类不许编造");

    // 2. 盘上原文：带分类的那行有这个字段，另一行**根本不写这个字段**（不是空串）。
    let text = std::fs::read_to_string(dir.env().join("trace.ndjson")).expect("读经历文件");
    let endings: Vec<&str> = text
        .lines()
        .filter(|line| line.contains(r#""kind":"ending""#))
        .collect();
    assert_eq!(endings.len(), 2, "{text}");
    assert_eq!(
        endings
            .iter()
            .filter(|line| line.contains(r#""failure":"transport-failure""#))
            .count(),
        1,
        "{text}"
    );
}

/// 不可解释的分类形状：**读的时候就拒绝**（fail-closed），不当成"没有分类"读出来。
///
/// 三种形状各错一处：认不出的取值、分类挂在非模型失败上、主体收场带分类。
#[test]
fn an_uninterpretable_trace_failure_is_refused_when_read_back() {
    use celira_agent::store::read_trace_at;

    let dir = TempDir::new("ending-failure-refused");
    let agent = AgentId::new("agent-alpha");
    let _env = Environment::create(&dir.env(), agent).expect("创建环境");

    let cases = [
        (
            "认不出的取值",
            r#"{"seq":0,"run":"run-bad","kind":"ending","system":"model-failed","failure":"no-such-class"}"#,
        ),
        (
            "分类挂在非模型失败上",
            r#"{"seq":0,"run":"run-bad","kind":"ending","system":"timeout","failure":"transport-failure"}"#,
        ),
        (
            "主体收场带分类",
            r#"{"seq":0,"run":"run-bad","kind":"ending","subject":"completed","failure":"transport-failure"}"#,
        ),
    ];
    for (label, line) in cases {
        std::fs::write(dir.env().join("trace.ndjson"), format!("{line}\n")).expect("写合成经历");
        let read = read_trace_at(&dir.env());
        assert!(
            matches!(read, Err(StoreError::MalformedEntry { .. })),
            "{label}：不可解释的分类必须被拒绝（而不是读成「没有分类」）: {read:?}"
        );
    }
}

/// **写侧的 fail-closed**：词表之外的模型失败分类**不许落盘**。
///
/// 它是上面那条读侧用例的**另一半**——两条合起来才是那条纪律的全文：
/// **持久字段上的词表一律 fail-closed**。读侧早就做到了；**写侧从前一个字都不查**。
///
/// 为什么写侧真的会不合法：`SystemEnding::ModelFailed` 的载荷在拆掉 Foundation ↔ Core 那个环
/// 之后是一根**裸字符串**（见 `SystemEnding` 的文档）——**类型不再保证它在词表之内**。
/// 一个词表之外的值一旦落盘，那一行**谁也读不回来**：一个活动会永远停在"读不出来"。
///
/// 两条写路各验一次（经历侧 `ending` 与事实侧 `run-ended`），而且都断言
/// **文件一个字节都没动**——拒绝要发生在写之前，不是写坏了再报。
#[test]
fn a_failure_class_outside_the_vocabulary_is_never_written() {
    let dir = TempDir::new("ending-failure-refused-on-write");
    let agent = AgentId::new("agent-alpha");
    let mut env = Environment::create(&dir.env(), agent).expect("创建环境");
    let run = RunId::new("run-bad-write");
    let trace_path = dir.env().join("trace.ndjson");
    let facts_path = dir.env().join("facts.ndjson");

    // ---- 经历侧：`ending` 那一格 ----
    let before = std::fs::read_to_string(&trace_path).expect("读经历文件");
    assert_eq!(
        env.append_trace(
            &run,
            TracePayload::Ending {
                subject: None,
                system: Some("model-failed".to_string()),
                failure: Some("no-such-class".to_string()),
            },
        ),
        Err(StoreError::UnknownFailureClass),
        "词表之外的分类必须被拒绝写"
    );
    assert_eq!(
        std::fs::read_to_string(&trace_path).expect("读经历文件"),
        before,
        "拒绝要发生在写之前：经历文件一个字节都不许动"
    );

    // ---- 事实侧：`run-ended` 那一格 ----
    let before = std::fs::read_to_string(&facts_path).expect("读事实文件");
    assert_eq!(
        env.append_fact(
            &run,
            &Record::RunEnded {
                run: run.clone(),
                outcome: Outcome::System(SystemEnding::ModelFailed(Some("no-such-class"))),
                cost: celira_agent::cost::RunCost::default(),
            },
        ),
        Err(StoreError::UnknownFailureClass),
        "词表之外的分类必须被拒绝写（事实侧）"
    );
    assert_eq!(
        std::fs::read_to_string(&facts_path).expect("读事实文件"),
        before,
        "拒绝要发生在写之前：事实文件一个字节都不许动"
    );

    // 自检：**词表之内的**分类照旧写得下去——否则上面两条可以靠"什么都拒绝"假绿。
    env.append_trace(
        &run,
        TracePayload::Ending {
            subject: None,
            system: Some("model-failed".to_string()),
            failure: Some("transport-failure".to_string()),
        },
    )
    .expect("词表之内的分类必须写得下去");
    assert!(
        std::fs::read_to_string(&trace_path)
            .expect("读经历文件")
            .contains(r#""failure":"transport-failure""#),
        "词表之内的分类落盘逐字节照旧"
    );
}

// ---------------------------------------------------------------------------
// 刀三 A：唤醒缘由的**词表值**
// ---------------------------------------------------------------------------
/// **判据 A（正）**：`run-start` 行上的 `origin` 是**词表值**，几个取值各写各的字符串。
///
/// 它钉住的是"持久字段是稳定字符串"这一条：写下去的是 `WakeOrigin::as_str`，读回来是同一个
/// 枚举，人话那句**不落盘**（它由渲染点现派生，见 `composition::origin_text`）。几个取值都走一遍，
/// 因此"只实现了其中一种"不会蒙混过去。
///
/// ⚠ **2026-10-13**：通道那一档分成两态（`wake-opened` / `wake-changed`），而 `bell` 成了
/// **旧账那一档**（照读不写）。这张表必须**照旧认得它**（旧记录读得出来），所以这里照写不误
/// ——**写它正是为了证明读得回来**。
#[test]
fn the_run_start_origin_is_a_vocabulary_value_on_disk() {
    use celira_agent::store::{WakeOrigin, read_trace_at};

    let dir = TempDir::new("origin-vocabulary");
    let agent = AgentId::new("agent-alpha");
    let mut env = Environment::create(&dir.env(), agent).expect("创建环境");

    // 几条活动各写一种缘由。
    for (run, origin) in [
        ("run-wake-opened", WakeOrigin::Opened),
        ("run-wake-changed", WakeOrigin::Changed),
        ("run-bell", WakeOrigin::Bell),
        ("run-timer", WakeOrigin::Timer),
        ("run-channel", WakeOrigin::Channel),
        ("run-manual", WakeOrigin::Manual),
    ] {
        env.append_trace(
            &RunId::new(run),
            TracePayload::RunStarted {
                origin: Some(origin),
            },
        )
        .expect("写一处登记");
    }

    // 盘上是**词表字符串**（不是人话）：逐行核对的原始文本。
    let raw = std::fs::read_to_string(dir.env().join("trace.ndjson")).expect("读原始盘面");
    for value in [
        "wake-opened",
        "wake-changed",
        "bell",
        "timer",
        "channel",
        "manual",
    ] {
        assert!(
            raw.contains(&format!("\"origin\":\"{value}\"")),
            "盘上必须是词表值 {value:?}：{raw}"
        );
    }
    // 人话那句**一个字都不该落盘**。
    for sentence in [
        celira_agent::activity_origin::MANUAL_ORIGIN_TEXT,
        celira_agent::activity_origin::WAKE_OPENED_ORIGIN_TEXT,
        celira_agent::activity_origin::WAKE_CHANGED_ORIGIN_TEXT,
        celira_agent::activity_origin::WAKE_LEGACY_ORIGIN_TEXT,
        celira_agent::activity_origin::TIMER_ORIGIN_TEXT,
        celira_agent::activity_origin::CHANNEL_ORIGIN_TEXT,
    ] {
        assert!(
            !raw.contains(sentence),
            "人话那句是**派生视图**，不许出现在盘上：{sentence:?}"
        );
    }

    // 读回来是同一个枚举。
    let trace = read_trace_at(&dir.env()).expect("读回来");
    let mut seen = Vec::new();
    for record in &trace {
        if let TracePayload::RunStarted { origin } = record.entry() {
            seen.push((record.run().as_str().to_string(), *origin));
        }
    }
    assert!(seen.contains(&("run-wake-opened".to_string(), Some(WakeOrigin::Opened))));
    assert!(seen.contains(&("run-wake-changed".to_string(), Some(WakeOrigin::Changed))));
    assert!(seen.contains(&("run-bell".to_string(), Some(WakeOrigin::Bell))));
    assert!(seen.contains(&("run-timer".to_string(), Some(WakeOrigin::Timer))));
    assert!(seen.contains(&("run-channel".to_string(), Some(WakeOrigin::Channel))));
    assert!(seen.contains(&("run-manual".to_string(), Some(WakeOrigin::Manual))));
}

/// **判据 A（fail-closed）**：`origin` 是一个**认不出的取值**时，**整行拒绝**——
/// **不**静默读成"没有起源"。
///
/// 这是与 `an_uninterpretable_trace_failure_is_refused_when_read_back` 同一条纪律：
/// 把"外部改过的行"读成一条合法旧记录，是在事实系统里说谎。`None`（字段缺席）与
/// `Some(认不出)` 是两件**必须分得开**的事。
///
/// ⚠️ 拒绝的**变体**与那条 ending 用例不同（这里如实钉住，免得读的人以为两者一样）：
/// 认不出的词让 `decode_trace` 直接返回 `None`，于是 `read_trace` 报 **`CorruptTail`**
/// （"这一行解不出来"）；那条 ending 用例的字段都是合法字符串、解得出来，
/// 是被 `trace_entry_is_interpretable` 判**语义不通**才报 `MalformedEntry`。
/// 两条都是 fail-closed 的拒绝——一条都不许被读成"没有起源"。
#[test]
fn an_unknown_run_start_origin_is_refused_when_read_back() {
    use celira_agent::store::read_trace_at;

    let dir = TempDir::new("origin-unknown-refused");
    let agent = AgentId::new("agent-alpha");
    let _env = Environment::create(&dir.env(), agent).expect("创建环境");

    // 两种"认不出"：一个从没见过的词，一个大小写不对的已知词。
    let cases = [
        (
            "从没见过的取值",
            r#"{"seq":0,"run":"run-bad","kind":"run-start","origin":"doorbell-please"}"#,
        ),
        (
            "大小写不对的已知词",
            r#"{"seq":0,"run":"run-bad","kind":"run-start","origin":"Bell"}"#,
        ),
    ];
    for (label, line) in cases {
        std::fs::write(dir.env().join("trace.ndjson"), format!("{line}\n")).expect("写合成经历");
        let read = read_trace_at(&dir.env());
        assert!(
            matches!(
                read,
                Err(StoreError::CorruptTail { .. }) | Err(StoreError::MalformedEntry { .. })
            ),
            "{label}：认不出的起源必须被拒绝（而不是读成「没有起源」）: {read:?}"
        );
    }
}

/// **判据 A（旧形状兼容）**：`origin` 字段**缺席** ⇒ 读成 `None`，照常读得出来。
///
/// 与上一条成对：这正是刀三要保住的"旧记录零迁移"——盘上根本没收过这个字段的那些行，
/// 行为与从前逐字节相同，**不是**"认不出的取值"，所以必须放行。
#[test]
fn a_run_start_without_an_origin_field_still_reads() {
    use celira_agent::store::read_trace_at;

    let dir = TempDir::new("origin-absent");
    let agent = AgentId::new("agent-alpha");
    let _env = Environment::create(&dir.env(), agent).expect("创建环境");

    std::fs::write(
        dir.env().join("trace.ndjson"),
        "{\"seq\":0,\"run\":\"run-old\",\"kind\":\"run-start\"}\n",
    )
    .expect("写旧形状经历");

    let trace = read_trace_at(&dir.env()).expect("旧形状必须照常读得出来");
    let starts: Vec<Option<celira_agent::store::WakeOrigin>> = trace
        .iter()
        .filter_map(|record| match record.entry() {
            TracePayload::RunStarted { origin } => Some(*origin),
            _ => None,
        })
        .collect();
    assert_eq!(
        starts,
        vec![None],
        "字段缺席读作 `None`（与认不出的取值是两件事）"
    );
}

// ---------------------------------------------------------------------------
// 刀三 B：时长的入账形状（**缺字段 = 没被记**）
// ---------------------------------------------------------------------------

/// **判据 B（形状）**：`run-ended` 事实行上的代价**没有就不写字段**。
///
/// 两个方向都要钉：
/// - 两格都没记（`RunCost::default()`）⇒ 那一行**一个代价字段都没有**，与旧记录逐字节相同；
/// - 只记了时长 / 只记了用量 ⇒ 只写对应的那几个字段，**不补零**（补零会说"用了 0"，那是假话）。
///
/// 这里量的是**盘面上的字段名**（不是内存里的结构）：`elapsed-ms` 与三个中立用量名，
/// 因为"信封形状"是持久契约的一部分，换名字就是换契约。
#[test]
fn the_run_ended_line_writes_only_the_cost_fields_it_has() {
    use celira_agent::cost::{RunCost, Usage};
    use celira_agent::store::read_trace_at;

    let dir = TempDir::new("cost-absent");
    let agent = AgentId::new("agent-alpha");
    let mut env = Environment::create(&dir.env(), agent).expect("创建环境");
    let run = RunId::new("run-cost");

    env.append_trace(&run, TracePayload::RunStarted { origin: None })
        .expect("经历：登记");
    // 收场：**两格都没记**（默认代价）。
    env.append_trace(
        &run,
        celira_agent::store::ending_payload(Outcome::Subject(SubjectEnding::Silent)),
    )
    .expect("经历：收场");
    env.append_fact(
        &run,
        &Record::RunEnded {
            run: run.clone(),
            outcome: Outcome::Subject(SubjectEnding::Silent),
            cost: RunCost::default(),
        },
    )
    .expect("事实：收场");

    let raw = std::fs::read_to_string(dir.env().join("facts.ndjson")).expect("读原始事实面");
    for field in [
        "elapsed-ms",
        "prompt-tokens",
        "completion-tokens",
        "total-tokens",
    ] {
        assert!(
            !raw.contains(field),
            "两格都没记 ⇒ 不许出现代价字段 {field:?}（缺字段 = 这一格没被记）: {raw}"
        );
    }

    // 现在换一条活动：只记时长。
    let dir2 = TempDir::new("cost-elapsed");
    let agent2 = AgentId::new("agent-alpha");
    let mut env2 = Environment::create(&dir2.env(), agent2).expect("创建环境");
    let run2 = RunId::new("run-cost-2");
    env2.append_trace(&run2, TracePayload::RunStarted { origin: None })
        .expect("经历：登记");
    env2.append_fact(
        &run2,
        &Record::RunEnded {
            run: run2.clone(),
            outcome: Outcome::Subject(SubjectEnding::Silent),
            cost: RunCost {
                elapsed_millis: Some(1_234),
                usage: None,
            },
        },
    )
    .expect("事实：收场");

    let raw2 = std::fs::read_to_string(dir2.env().join("facts.ndjson")).expect("读原始事实面");
    assert!(
        raw2.contains("\"elapsed-ms\":1234"),
        "记了时长就写这一格：{raw2}"
    );
    // 用量整格没记 ⇒ 三个用量字段一个都不许出现，也**不许补 0**。
    for field in ["prompt-tokens", "completion-tokens", "total-tokens"] {
        assert!(
            !raw2.contains(field),
            "用量整格没记 ⇒ 不许出现 {field:?}：{raw2}"
        );
    }

    // 第三条：只记用量（且厂商只报了两个字段）—— 第三个**如实缺着**。
    let dir3 = TempDir::new("cost-usage");
    let agent3 = AgentId::new("agent-alpha");
    let mut env3 = Environment::create(&dir3.env(), agent3).expect("创建环境");
    let run3 = RunId::new("run-cost-3");
    env3.append_trace(&run3, TracePayload::RunStarted { origin: None })
        .expect("经历：登记");
    env3.append_fact(
        &run3,
        &Record::RunEnded {
            run: run3.clone(),
            outcome: Outcome::Subject(SubjectEnding::Silent),
            cost: RunCost {
                elapsed_millis: None,
                usage: Some(Usage {
                    prompt_tokens: Some(11),
                    completion_tokens: Some(7),
                    total_tokens: None,
                }),
            },
        },
    )
    .expect("事实：收场");

    let raw3 = std::fs::read_to_string(dir3.env().join("facts.ndjson")).expect("读原始事实面");
    assert!(raw3.contains("\"prompt-tokens\":11"), "报了就记：{raw3}");
    assert!(raw3.contains("\"completion-tokens\":7"), "报了就记：{raw3}");
    assert!(
        !raw3.contains("total-tokens"),
        "厂商没报总量 ⇒ 这一格如实缺着，不许补零：{raw3}"
    );
    assert!(
        !raw3.contains("elapsed-ms"),
        "时长没记 ⇒ 不许出现这一格：{raw3}"
    );

    // 读回来：代价**往返无损**（写什么读什么，本层不换算）。
    let trace3 = read_trace_at(&dir3.env()).expect("读回来");
    assert!(!trace3.is_empty(), "读回来的经历不该是空的");
}

// ---------------------------------------------------------------------------
// 「这一行是什么时候写的」（`at-ms`，2026-10-06）
// ---------------------------------------------------------------------------

/// **每一条记录都带它；没有钟就不带；旧记录照读。**
///
/// 这一条把三件事一次钉死：
///
/// 1. **没有钟 ⇒ 那一格一个字节都不出现** —— 直接走 [`Environment::append_trace`] 的那些路径
///    （离线夹具、诊断、旧入口）走这一档，因此它们写出来的字节与从前**逐字节相同**。
///    ⚠ **这一条同时就是"旧记录照读"的证据**：盘上那一行与"加这一格之前写下的记录"
///    **字节等价**（都没有 `at-ms`），而它今天读得出来、那一格读作 `None`；
/// 2. **装上钟 ⇒ 每一条记录都带它**，而且读得回来（经历侧与事实侧**同一条判据、同一个口径**）；
/// 3. **不补写历史**：装钟**之前**写下的那一条，读回来仍然是「没有被记」（`None`）——
///    给旧记录补一个时刻就是**编造**。
#[test]
fn every_record_can_carry_the_instant_it_was_written() {
    use celira_agent::clock::ClockHandle;

    let dir = TempDir::new("at-ms");
    let agent = AgentId::new("agent-at-ms");
    let run = RunId::new("run-at-ms");

    // ---- 1) 没有钟：那一格整个不出现（＝旧格式）----
    let mut environment = Environment::create(&dir.env(), agent.clone()).expect("建立环境");
    environment
        .append_trace(&run, TracePayload::RunStarted { origin: None })
        .expect("写一条经历");
    let without = environment.trace().expect("读回经历");
    assert_eq!(
        without[0].at_millis(),
        None,
        "没有钟 ⇒ 这一格没有被记（不是「等于零」）"
    );
    let raw = std::fs::read_to_string(dir.env().join("trace.ndjson")).expect("读原始行");
    // ⚠ **精确匹配那一格，不查裸子串**：这条用例的活动身份**故意**叫 `run-at-ms` ——
    // 它本身就含 `at-ms` 这个子串。⇒ 裸子串断言会在这里**假红**，而这条数据正好是那个反例
    // （与 `AGENTS.md`「守卫…不用裸子串误报中立词汇」同一条纪律）。
    assert!(
        !raw.contains("\"at-ms\":"),
        "没有钟时那一格一个字节都不许出现（离线夹具的字节因此逐字节不变）：{raw}"
    );
    assert!(
        raw.contains("run-at-ms"),
        "而活动身份照旧写着 —— 上面那条断言查的是【那一格】，不是这个词出现在哪里：{raw}"
    );

    // ---- 2) 装上钟：此后每一条都带它，而**之前那条不被追溯补写** ----
    environment.set_clock(ClockHandle::fixed_at_unix_seconds(1_700_000_000));
    environment
        .append_trace(
            &run,
            TracePayload::RunInput {
                text: "喂".to_string(),
            },
        )
        .expect("再写一条经历");
    let with = environment.trace().expect("读回经历");
    assert_eq!(
        with[1].at_millis(),
        Some(1_700_000_000_000),
        "装钟之后写的那一条带着那一刻（unix 毫秒）"
    );
    assert_eq!(
        with[0].at_millis(),
        None,
        "装钟之前写的那一条仍然是「没有被记」—— 补写历史就是编造"
    );

    // ---- 3) 事实侧同一条判据 ----
    environment
        .append_fact(
            &run,
            &Record::RunStarted {
                agent: agent.clone(),
                run: run.clone(),
            },
        )
        .expect("写一条事实");
    let facts = environment.facts().expect("读回事实");
    assert_eq!(
        facts[0].at_millis(),
        Some(1_700_000_000_000),
        "事实侧也带它：同一个钟、同一个口径"
    );
    let raw_facts = std::fs::read_to_string(dir.env().join("facts.ndjson")).expect("读原始事实行");
    assert!(
        raw_facts.contains("\"at-ms\":1700000000000"),
        "事实侧盘上真的写了那一格：{raw_facts}"
    );
}
