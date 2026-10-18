//! Phase 03 真实模型只读文件闭环验收（opt-in）。
//!
//! 运行方式（只跑这一个用例）：
//!
//! ```text
//! cargo test --offline --test live_file_app -- --ignored --nocapture
//! ```
//!
//! 前置：环境变量 `DEEPSEEK_API_KEY` 已存在。**只读变量，不打印值、不写文件、不进 argv。**
//! 硬预算：一次 Run、最多 4 次真实模型请求；不重试、不换模型、不反复调用直到通过。
//!
//! 本文件只做两件事：**观察**与**断言**。它不替模型选文件、不补参数、不伪造结果、不放宽
//! FileApp 的校验；两个包装器每次都**原样委托**真实端口（唯一例外是硬预算：超预算的请求
//! 不会再发出去）。

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use celira_agent::core::{
    Action, ActionAdmission, ActionOutcome, ActionPort, Agent, Capabilities, Decision, Limits,
    ModelError, ModelFailureClass, ModelOutput, ModelPort, View, run_activity,
};
use celira_agent::file_app::FileApp;
use celira_agent::foundation::{AgentId, Ledger, OperationId, Outcome, RunId, SystemEnding};
use celira_agent::provider_adapter::DeepSeekModel;

/// 真实模型请求的硬预算。
const MODEL_REQUEST_BUDGET: usize = 4;

/// 验收判定：失败原因必须可区分。
#[derive(Clone, Debug, PartialEq, Eq)]
enum Verdict {
    Passed,
    /// 配置缺失（环境变量不在）。
    ConfigMissing,
    /// 传输或协议层失败。
    TransportOrProtocol,
    /// 模型没有完成指定行为（附一句具体说明）。
    ModelBehaviour(&'static str),
    /// 预算耗尽（第 5 次请求没有发出）。
    BudgetExhausted,
    /// 文件 App 拒绝启动。
    AppRefused,
    /// 验收代码自身的缺陷（不是模型或 App 的问题）。
    AssertionDefect(&'static str),
}

/// 每次真实模型调用看到的视野事实（用于证据核对；不含敏感内容）。
#[derive(Clone, Debug)]
struct ViewFacts {
    turn: u32,
    capabilities: usize,
    observations: usize,
    /// 本轮视野的观察内容里是否出现了合成标记。
    marker_in_view: bool,
}

/// 记账器：包装器只写它，验收代码只读它。
#[derive(Default)]
struct Recorder {
    views: Vec<ViewFacts>,
    /// 每次模型调用的输出类别（只记类别，不记原文）。
    outputs: Vec<String>,
    /// 真实 ActionPort 收到的行动：(能力, path 参数)。
    actions: Vec<(String, String)>,
    /// 每次行动的结果类别与稳定分类。
    outcomes: Vec<(String, Option<String>)>,
    /// 列目录返回的文本（用于核对读取路径确实来自列表）。
    listing: Option<String>,
    /// 是否发生过预算超限。
    over_budget: bool,
}

/// 透明包装的模型：计数、记录视野事实，然后原样委托。
struct CountingModel<'a> {
    inner: &'a mut DeepSeekModel,
    recorder: Rc<RefCell<Recorder>>,
    marker: String,
}

impl ModelPort for CountingModel<'_> {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        {
            let mut recorder = self.recorder.borrow_mut();
            if recorder.views.len() >= MODEL_REQUEST_BUDGET {
                // 硬预算：第 5 次请求不发出（真实请求计数就是预算本身）。
                recorder.over_budget = true;
                return Err(scripted_model_failure());
            }
            let facts = ViewFacts {
                turn: view.turn().number(),
                capabilities: view.capabilities().len(),
                observations: view.observed().len(),
                marker_in_view: view.observed().iter().any(|observation| {
                    observation
                        .outcome()
                        .content()
                        .iter()
                        .any(|block| block.text().contains(&self.marker))
                }),
            };
            recorder.views.push(facts);
        }

        // 原样委托真实端口：不改写视野、不伪造结果。
        let result = self.inner.complete(view);
        let mut recorder = self.recorder.borrow_mut();
        recorder.outputs.push(match &result {
            Ok(output) => match output.decision() {
                Decision::Act(actions) => format!(
                    "Act({})",
                    actions
                        .iter()
                        .map(|action| action.id().as_str())
                        .collect::<Vec<_>>()
                        .join("+")
                ),
                Decision::Finish(ending) => format!("Finish({ending:?})"),
                Decision::Yield => format!("Yield({:?})", output.content()),
            },
            Err(_) => "ModelError".to_string(),
        });
        result
    }
}

/// 透明包装的行动端口：记录实际收到的行动与结果，然后原样委托。
struct CountingActions<'a> {
    inner: &'a mut FileApp,
    recorder: Rc<RefCell<Recorder>>,
}

impl ActionPort for CountingActions<'_> {
    /// 原样委托：能力就是内层文件 App 声明的能力（本验收不装配最小 OS）。
    fn capabilities(&self, agent: &AgentId, run: &RunId) -> Capabilities {
        self.inner.capabilities(agent, run)
    }

    // 准入策略：本替身**不**模拟授权判断，一律接纳——它的用途不是检验这一层。
    fn admit(&self, _agent: &AgentId, _run: &RunId, _action: &Action) -> ActionAdmission {
        ActionAdmission::Admitted
    }

    fn execute(
        &mut self,
        agent: &AgentId,
        operation: &OperationId,
        action: &Action,
    ) -> ActionOutcome {
        let outcome = self.inner.execute(agent, operation, action);

        let mut recorder = self.recorder.borrow_mut();
        recorder.actions.push((
            action.id().as_str().to_string(),
            action.params().get("path").unwrap_or("").to_string(),
        ));
        recorder.outcomes.push((
            format!("{:?}", outcome.state()),
            outcome.failure_class().map(str::to_string),
        ));
        if action.id().as_str() == "files_list"
            && let Some(block) = outcome.content().first()
        {
            recorder.listing = Some(block.text().to_string());
        }

        outcome
    }
}

/// 本次验收的合成沙盒：只在系统临时目录下创建。
struct LiveSandbox {
    root: PathBuf,
    /// 文件名 → 精确内容（合成标记）。
    files: Vec<(String, String)>,
}

impl LiveSandbox {
    fn new(marker: &str) -> Self {
        let unique = format!(
            "celira-live-file-app-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("时钟可用")
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        fs::create_dir_all(&root).expect("可以建临时沙盒");

        // 两个以上合成文本文件；内容用本次生成的唯一非敏感标记。
        let files = vec![
            (
                "alpha-notes.txt".to_string(),
                format!("{marker}-ALPHA\nline two\n"),
            ),
            ("beta-notes.txt".to_string(), format!("{marker}-BETA\n")),
            ("gamma-report.txt".to_string(), format!("{marker}-GAMMA\n")),
        ];
        for (name, text) in &files {
            fs::write(root.join(name), text).expect("可以写合成文件");
        }
        // 一个非文本文件，让选择不是"目录里只有一种东西"。
        fs::write(root.join("binary.bin"), [0xffu8, 0xfe, 0x00]).expect("可以写合成文件");

        Self { root, files }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    /// 目录叶名（用于报告复核；不打印带用户名的绝对路径）。
    fn leaf_name(&self) -> String {
        self.root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

/// 打印可复核的证据摘要：只有类别、计数与合成文件名，不含密钥、请求体、响应体与个人绝对路径。
fn print_evidence(
    verdict: &Verdict,
    recorder: &Recorder,
    outcome: Option<Outcome>,
    records: usize,
    body_in_ledger: bool,
) {
    println!("=== Phase 03 真实验收证据 ===");
    println!("判定: {verdict:?}");
    println!("真实模型请求次数: {}", recorder.views.len());
    for (index, facts) in recorder.views.iter().enumerate() {
        let output = recorder
            .outputs
            .get(index)
            .map(String::as_str)
            .unwrap_or("(未返回)");
        println!(
            "  调用 {}: turn={} caps={} observations={} marker_in_view={} → {}",
            index + 1,
            facts.turn,
            facts.capabilities,
            facts.observations,
            facts.marker_in_view,
            output
        );
    }
    for (index, (capability, path)) in recorder.actions.iter().enumerate() {
        let outcome = &recorder.outcomes[index];
        println!(
            "  行动 {}: {capability}(path={path:?}) → {outcome:?}",
            index + 1
        );
    }
    if let Some(listing) = &recorder.listing {
        let entries = listing.lines().skip(1).count();
        println!("  列目录条目数: {entries}（截断标记见首行）");
    }
    println!("Outcome: {outcome:?}");
    println!("Ledger 记录数: {records}；事实中含文件正文: {body_in_ledger}");
    println!("=================================");
}

/// 真实验收：一次 Run、最多四次模型请求，证明真实模型依据局部视野选择读取。
#[test]
#[ignore = "为什么：它跑一次**真实端到端闭环**（真模型 + 合成沙盒），要网络与真密钥——要外部世界参与的用例不能进默认门禁（默认那套必须离线可重跑）。什么时候重建／重跑：改了文件 App 的只读面、装配路径或结果回执之后手工跑一次，换模型档位时也跑一次；跑法 `cargo test --offline --test live_file_app -- --ignored --nocapture`（需 DEEPSEEK_API_KEY）"]
fn live_read_only_files_closure() {
    // 1) 配置缺失必须明确失败，绝不返回 Ok 冒充通过。
    let mut model = match DeepSeekModel::from_env() {
        Ok(model) => model,
        Err(_) => {
            let recorder = Recorder::default();
            print_evidence(&Verdict::ConfigMissing, &recorder, None, 0, false);
            panic!(
                "未运行：{} 未设置（判定 ConfigMissing，不冒充通过；不向主人索要密钥值）",
                DeepSeekModel::API_KEY_ENV
            );
        }
    };

    // 2) 合成沙盒（自建、只用合成内容）。
    let marker = format!(
        "CELIRA-LIVE-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("时钟可用")
            .as_nanos()
    );
    let sandbox = LiveSandbox::new(&marker);

    // 3) 真实文件 App：根显式传入，拒绝启动就是 AppRefused。
    let mut app = match FileApp::open(sandbox.path()) {
        Ok(app) => app,
        Err(class) => {
            let recorder = Recorder::default();
            print_evidence(&Verdict::AppRefused, &recorder, None, 0, false);
            panic!("未通过：文件 App 拒绝启动（{class:?}）");
        }
    };
    let capabilities = FileApp::capabilities();

    // 4) 第一次调用之前，视野里不得有文件内容或文件名——能力说明本身也不含文件名。
    for capability in capabilities.items() {
        assert!(
            !capability.skeleton().contains(".txt") && !capability.details().contains(".txt"),
            "能力说明（骨架与详情）不该提到沙盒里的具体文件"
        );
        for spec in capability.params() {
            assert!(
                !spec.description().contains(".txt"),
                "参数说明不该提到沙盒里的具体文件"
            );
        }
    }

    // 5) 装配：Core + 文件 App + 真实 Adapter，两个端口都用透明包装器观察。
    let recorder = Rc::new(RefCell::new(Recorder::default()));
    let mut agent = Agent::new(AgentId::new("agent-live"));
    let mut ledger = Ledger::new(agent.id().clone());

    let run_result = {
        let mut counting_model = CountingModel {
            inner: &mut model,
            recorder: Rc::clone(&recorder),
            marker: marker.clone(),
        };
        let mut counting_actions = CountingActions {
            inner: &mut app,
            recorder: Rc::clone(&recorder),
        };
        run_activity(
            &mut agent,
            RunId::new("run-live"),
            Limits::new(MODEL_REQUEST_BUDGET as u32),
            &mut counting_model,
            &mut counting_actions,
            &mut ledger,
        )
    };

    let recorder_snapshot = recorder.borrow();
    let records = ledger.records().to_vec();
    let ledger_debug = format!("{records:?}");
    let body_in_ledger = sandbox
        .files
        .iter()
        .any(|(_, text)| ledger_debug.contains(text.trim()));

    let run = match run_result {
        Ok(run) => run,
        Err(error) => {
            print_evidence(
                &Verdict::AssertionDefect("活动没有跑完"),
                &recorder_snapshot,
                None,
                records.len(),
                body_in_ledger,
            );
            panic!("未通过：活动层面返回错误 {error:?}");
        }
    };
    let outcome = run.outcome();

    // 6) 逐条核对证据，得出可区分的判定。
    let verdict = classify(&recorder_snapshot, &sandbox, outcome, body_in_ledger);
    print_evidence(
        &verdict,
        &recorder_snapshot,
        outcome,
        records.len(),
        body_in_ledger,
    );

    match verdict {
        Verdict::Passed => {
            // 通过时清理自己创建的沙盒。
            let _ = fs::remove_dir_all(sandbox.path());
            println!("沙盒已清理（自建临时目录）。");
        }
        other => {
            // 未通过时保留沙盒供复核；只报叶名，不打印带用户名的绝对路径。
            println!(
                "沙盒保留供复核：系统临时目录下 {}（本次运行失败，未自动清理）",
                sandbox.leaf_name()
            );
            panic!("未通过：{other:?}");
        }
    }
}

/// 把观察到的证据映射成判定。顺序体现优先级：预算 > 传输 > 模型行为 > 证据不足。
fn classify(
    recorder: &Recorder,
    sandbox: &LiveSandbox,
    outcome: Option<Outcome>,
    body_in_ledger: bool,
) -> Verdict {
    if recorder.over_budget {
        return Verdict::BudgetExhausted;
    }
    if recorder.outputs.iter().any(|output| output == "ModelError") {
        return Verdict::TransportOrProtocol;
    }
    if body_in_ledger {
        return Verdict::AssertionDefect("文件正文进入了 Ledger 事实");
    }

    // 模型必须至少被调用一次。
    let Some(first) = recorder.views.first() else {
        return Verdict::AssertionDefect("模型一次都没有被调用");
    };
    if first.observations != 0 || first.marker_in_view {
        return Verdict::AssertionDefect("第一次调用时视野里已经有观察或内容");
    }

    // 必须有一次真实模型选择的列目录，并且 App 真的返回了列表。
    let Some(listing) = &recorder.listing else {
        return Verdict::ModelBehaviour("模型没有做出列目录的选择");
    };

    // 读取必须由模型选择，且路径确实来自先前返回的列表，内容确实读出来了。
    let read_index = recorder
        .actions
        .iter()
        .position(|(capability, _)| capability == "files_read");
    let Some(read_index) = read_index else {
        return Verdict::ModelBehaviour("模型没有选择读取任何文件");
    };
    let (_, path) = &recorder.actions[read_index];
    if path.is_empty() {
        return Verdict::ModelBehaviour("读取没有给出路径");
    }
    let listed = listing
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once('\t').map(|(name, _)| name))
        .any(|name| name == path);
    if !listed {
        return Verdict::ModelBehaviour("读取路径不是来自先前返回的列表");
    }
    if !sandbox.files.iter().any(|(name, _)| name == path) {
        return Verdict::AssertionDefect("读到的不是本任务建立的合成文件");
    }
    match &recorder.outcomes[read_index].0 {
        state if state == "Success" => {}
        _ => return Verdict::ModelBehaviour("读取没有成功"),
    }

    // 读取之后必须还有一次模型调用，且那次调用**在视野里看到了精确内容**。
    if recorder.views.len() <= read_index + 1 {
        return Verdict::ModelBehaviour("读取之后没有进入下一次模型调用");
    }
    let after_read = &recorder.views[read_index + 1];
    if !after_read.marker_in_view {
        return Verdict::ModelBehaviour("后续视野里没有出现读取到的内容");
    }

    // 活动必须以明确 Outcome 收场：主体收场，或预算耗尽导致的系统收场
    // （后者是资源边界，不是模型行为失败）。
    match outcome {
        Some(Outcome::Subject(_)) | Some(Outcome::System(SystemEnding::Timeout)) => Verdict::Passed,
        _ => Verdict::ModelBehaviour("活动没有以明确收场结束"),
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
