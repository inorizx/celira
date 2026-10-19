//! OS 窄接入契约与路由的离线证明。
//!
//! 这里用**一个真正的文件消费者**（只读文件视图）加**一个仅测试使用的小 App 替身**
//! 证明：两者由同一份契约装配、互不依赖、可替换；能力目录与分派用同一条归属规则；
//! 身份不匹配或未展开时**一次都不会碰到 App**（替身自己数访问次数，不靠"返回里没有正文"推断）。
//!
//! 全部离线、全合成数据、只用测试自建的临时目录。

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use celira_agent::app_contract::{AppPort, AssemblyFailure, SourceClass, SourceDeclaration};
use celira_agent::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Capabilities,
    Capability, ContentBlock, ParamSpec,
};
use celira_agent::file_app::FILE_APP_ID;
use celira_agent::file_view::FileView;
use celira_agent::foundation::{AgentId, OperationId, ResultState, RunId, Turn};
use celira_agent::os::{ExpansionFailure, MinimalOs, RunExpansion};

const EXPAND: &str = "open_app";
const LIST: &str = "files_list";
const READ: &str = "files_read";

/// 只数访问次数的 App 替身：它没有业务，只证明"这一次调用有没有真的走到 App"。
struct CountingApp {
    id: String,
    action: String,
    /// 除主行动外再声明一个行动（用来构造"同一个标识被声明两次"的真实反例）。
    extra_action: Option<String>,
    visits: Rc<Cell<usize>>,
}

impl CountingApp {
    fn new(id: &str, action: &str) -> (Self, Rc<Cell<usize>>) {
        let visits = Rc::new(Cell::new(0));
        (
            Self {
                id: id.to_string(),
                action: action.to_string(),
                extra_action: None,
                visits: Rc::clone(&visits),
            },
            visits,
        )
    }

    fn with_extra(id: &str, action: &str, extra: &str) -> Self {
        Self {
            id: id.to_string(),
            action: action.to_string(),
            extra_action: Some(extra.to_string()),
            visits: Rc::new(Cell::new(0)),
        }
    }

    fn declared(&self) -> Capabilities {
        let mut items = vec![Capability::new(
            ActionId::new(self.action.clone()),
            "测试替身：只记录自己被调用了几次",
            vec![ParamSpec::new("note", false, "替身不解释它")],
        )];
        if let Some(extra) = &self.extra_action {
            items.push(Capability::new(
                ActionId::new(extra.clone()),
                "测试替身：第二个行动声明",
                Vec::new(),
            ));
        }
        Capabilities::of(items)
    }
}

impl ActionPort for CountingApp {
    fn capabilities(&self, _agent: &AgentId, _run: &RunId) -> Capabilities {
        self.declared()
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
        self.visits.set(self.visits.get() + 1);
        ActionOutcome::success(vec![ContentBlock::new("counted", "app was visited")])
    }
}

impl AppPort for CountingApp {
    fn app_id(&self) -> &str {
        &self.id
    }

    fn description(&self) -> &str {
        "测试替身 App"
    }

    fn declared_capabilities(&self) -> Capabilities {
        self.declared()
    }

    /// 替身不携带任何外来内容：它的回执一律是"系统对这次动作的确认"。
    ///
    /// 本文件里这一格是**装配期的必填项**：漏了它，连"重复标识"这类用例都会先撞上
    /// `capability-without-source`——那正是 fail-closed 想要的效果。
    fn declared_sources(&self) -> Vec<SourceDeclaration> {
        self.declared()
            .items()
            .iter()
            .map(|capability| SourceDeclaration::new(capability.id().clone(), SourceClass::System))
            .collect()
    }
}

/// 自建临时资料目录。
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(label: &str) -> Self {
        let unique = format!(
            "celira-app-contract-{}-{}-{}",
            std::process::id(),
            label,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("时钟可用")
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        fs::create_dir_all(&root).expect("建临时目录");
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn write(&self, name: &str, text: &str) {
        fs::write(self.root.join(name), text).expect("写合成资料");
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn operation(run: &str) -> OperationId {
    OperationId::new(RunId::new(run), Turn::FIRST)
}

fn expand(target: &str) -> Action {
    Action::with_params(
        ActionId::new(EXPAND),
        ActionParams::none().with("target", target),
    )
}

fn list() -> Action {
    Action::with_params(ActionId::new(LIST), ActionParams::none())
}

fn call(id: &str) -> Action {
    Action::with_params(ActionId::new(id), ActionParams::none())
}

fn capability_ids(
    scope: &celira_agent::os::RunScope<'_>,
    agent: &AgentId,
    run: &RunId,
) -> Vec<String> {
    scope
        .capabilities(agent, run)
        .items()
        .iter()
        .map(|capability| capability.id().as_str().to_string())
        .collect()
}

/// 1. 文件组件与独立小 App 由**同一份契约**装配；展开互相独立，能力只增不减。
#[test]
fn two_apps_from_one_contract_expand_independently() {
    let sandbox = Sandbox::new("independent");
    sandbox.write("alpha.txt", "资料正文");

    let (counter, visits) = CountingApp::new("counter", "echo");
    let mut os = MinimalOs::assemble(vec![
        Box::new(FileView::materials_only(sandbox.path()).expect("装配只读文件视图")),
        Box::new(counter),
    ])
    .expect("两个 App 由同一份契约装配");
    assert_eq!(os.app_ids(), vec![FILE_APP_ID, "counter"]);

    let agent = AgentId::new("agent-a");
    let run = RunId::new("run-1");
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = os.scope(&mut expansion);

    // 入口只有 OS 自己的那几项（展开 / 看详情 / 钉 / 摘 / 写自识）：App 的内部能力都不在目录里。
    let entry = scope.capabilities(&agent, &run);
    assert_eq!(
        entry.len(),
        5,
        "入口只有 OS 自己的五项能力（展开 / 看详情 / 钉 / 摘 / 写自识）"
    );
    let expand_capability = entry.find(&ActionId::new("open_app")).expect("展开在场");
    let details = expand_capability.details().to_string();
    assert!(details.contains(FILE_APP_ID), "{details}");
    assert!(details.contains("counter"), "{details}");
    assert!(entry.find(&ActionId::new(LIST)).is_none());
    assert!(entry.find(&ActionId::new("echo")).is_none());

    // 展开 A（文件视图）：只有 A 的能力出现，B 没被顺带打开。
    assert_eq!(
        scope
            .execute(&agent, &operation("run-1"), &expand(FILE_APP_ID))
            .state(),
        ResultState::Success
    );
    let after_files = capability_ids(&scope, &agent, &run);
    assert!(after_files.contains(&LIST.to_string()));
    assert!(after_files.contains(&READ.to_string()));
    assert!(
        !after_files.contains(&"echo".to_string()),
        "{after_files:?}"
    );
    // 未展开的 B：明确拒绝，而且**一次都没有走到 B**。
    let refused = scope.execute(&agent, &operation("run-1"), &call("echo"));
    assert_eq!(
        refused.failure_class(),
        Some(ExpansionFailure::NotExpanded.as_str())
    );
    assert_eq!(visits.get(), 0, "未展开时不得触碰 App");

    // 展开 B：B 的能力出现，A 的能力仍在（入口能力也没有被顶掉）。
    assert_eq!(
        scope
            .execute(&agent, &operation("run-1"), &expand("counter"))
            .state(),
        ResultState::Success
    );
    let after_both = capability_ids(&scope, &agent, &run);
    for expected in [EXPAND, LIST, READ, "echo"] {
        assert!(after_both.contains(&expected.to_string()), "{after_both:?}");
    }

    // 重复展开 B：只留一条痕迹，不重读、不重复注入能力说明。
    let again = scope.execute(&agent, &operation("run-1"), &expand("counter"));
    assert_eq!(again.state(), ResultState::Success);
    assert_eq!(again.content()[0].label(), "already-expanded");
    assert_eq!(
        again.content()[0].text().matches("echo").count(),
        0,
        "重复展开不得重新注入能力说明"
    );

    // 展开之后 B 才真的会被调用。
    let visited = scope.execute(&agent, &operation("run-1"), &call("echo"));
    assert_eq!(visited.state(), ResultState::Success);
    assert_eq!(visits.get(), 1);
}

/// 2. 伪造身份：能力目录为空、动作被明确拒绝，而且**一次都不会碰到 App**。
#[test]
fn a_forged_identity_gets_no_capabilities_and_touches_no_app() {
    let sandbox = Sandbox::new("forged");
    sandbox.write("alpha.txt", "资料正文");

    let (counter, visits) = CountingApp::new("counter", "echo");
    let mut os = MinimalOs::assemble(vec![
        Box::new(FileView::materials_only(sandbox.path()).expect("装配只读文件视图")),
        Box::new(counter),
    ])
    .expect("装配");
    let agent = AgentId::new("agent-a");
    let run = RunId::new("run-1");
    let mut expansion2 = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = os.scope(&mut expansion2);
    // 两个 App 都展开：下面才谈得上"伪造身份时是否真的没走到 App"。
    for target in ["counter", FILE_APP_ID] {
        assert_eq!(
            scope
                .execute(&agent, &operation("run-1"), &expand(target))
                .state(),
            ResultState::Success,
            "展开 {target}"
        );
    }

    let other_agent = AgentId::new("agent-b");
    let other_run = RunId::new("run-2");

    // 能力目录：换身份、换 Run 都返回空集合。
    assert_eq!(scope.capabilities(&other_agent, &run).len(), 0);
    assert_eq!(scope.capabilities(&agent, &other_run).len(), 0);

    // 动作：身份不匹配一律 fail-closed，且不会走到任何 App。
    for (caller, op_run) in [(&other_agent, "run-1"), (&agent, "run-2")] {
        let outcome = scope.execute(caller, &operation(op_run), &call("echo"));
        assert_eq!(
            outcome.failure_class(),
            Some(ExpansionFailure::IdentityMismatch.as_str())
        );
        assert!(outcome.content().is_empty());
    }
    assert_eq!(visits.get(), 0, "身份不匹配时不得触碰 App");

    // 文件侧同理：资料目录已经不存在了，伪造身份仍然只得到身份拒绝（不落到文件系统错误）。
    fs::remove_dir_all(sandbox.path()).expect("删掉资料目录");
    let forged_file = scope.execute(&other_agent, &operation("run-1"), &list());
    assert_eq!(
        forged_file.failure_class(),
        Some(ExpansionFailure::IdentityMismatch.as_str()),
        "伪造调用不得以文件系统结果回答"
    );
    // 反证：同一作用域里用自己的身份调用，才会真的走到文件系统。
    let real = scope.execute(&agent, &operation("run-1"), &list());
    assert_eq!(real.state(), ResultState::Failure);
    assert_eq!(real.failure_class(), Some("not-found"));
}

/// 3. 替换或移除一个 App：另一个照常工作，OS 骨干与 Core 不需要任何改动。
#[test]
fn removing_or_replacing_one_app_leaves_the_other_intact() {
    let sandbox = Sandbox::new("replace");
    sandbox.write("alpha.txt", "资料正文");
    let agent = AgentId::new("agent-a");
    let run = RunId::new("run-1");

    // 只装配文件视图：替身 App 不在，它的标识就是未知目标。
    let mut files_only = MinimalOs::assemble(vec![Box::new(
        FileView::materials_only(sandbox.path()).expect("装配只读文件视图"),
    )])
    .expect("装配");
    {
        let mut expansion3 = RunExpansion::new(agent.clone(), run.clone());
        let mut scope = files_only.scope(&mut expansion3);
        let unknown = scope.execute(&agent, &operation("run-1"), &expand("counter"));
        assert_eq!(
            unknown.failure_class(),
            Some(ExpansionFailure::UnknownTarget.as_str())
        );
        assert_eq!(
            scope
                .execute(&agent, &operation("run-1"), &expand(FILE_APP_ID))
                .state(),
            ResultState::Success
        );
        assert_eq!(
            scope.execute(&agent, &operation("run-1"), &list()).state(),
            ResultState::Success,
            "移除替身之后文件视图照常工作"
        );
    }

    // 只装配替身（换成另一个标识/行动）：同一份契约、同一个 OS，行为一致。
    let (replacement, visits) = CountingApp::new("counter-2", "echo-2");
    let mut counter_only = MinimalOs::assemble(vec![Box::new(replacement)]).expect("装配");
    assert_eq!(counter_only.app_ids(), vec!["counter-2"]);
    {
        let mut expansion4 = RunExpansion::new(agent.clone(), run.clone());
        let mut scope = counter_only.scope(&mut expansion4);
        let unknown = scope.execute(&agent, &operation("run-1"), &expand(FILE_APP_ID));
        assert_eq!(
            unknown.failure_class(),
            Some(ExpansionFailure::UnknownTarget.as_str())
        );
        assert_eq!(
            scope
                .execute(&agent, &operation("run-1"), &expand("counter-2"))
                .state(),
            ResultState::Success
        );
        assert_eq!(
            scope
                .execute(&agent, &operation("run-1"), &call("echo-2"))
                .state(),
            ResultState::Success
        );
        assert_eq!(visits.get(), 1);
    }
}

/// 4. 无法唯一解释的装配**在装配期拒绝**，不按顺序静默覆盖。
#[test]
fn preinstalling_a_name_that_was_never_assembled_is_refused() {
    // 预装常开是**装配期的声明**：声明一个没装配的标识等于"说了一件不存在的事"，
    // 静默忽略它会让目录与声明悄悄不一致——所以在这一步就停下来。
    let (only, _) = CountingApp::new("counter", "echo");
    let refused = MinimalOs::assemble_preinstalled(vec![Box::new(only)], &["notes"])
        .expect_err("预装一个没装配的 App 必须被拒绝");
    assert_eq!(
        refused,
        AssemblyFailure::UnknownPreinstalledApp {
            app: "notes".to_string()
        }
    );
    assert_eq!(refused.as_str(), "unknown-preinstalled-app");

    // 反过来：声明**已装配**的标识一切照旧（不多不少）。
    let (present, _) = CountingApp::new("counter", "echo");
    let os = MinimalOs::assemble_preinstalled(vec![Box::new(present)], &["counter"])
        .expect("预装已装配的 App 必须成功");
    assert!(os.is_preinstalled("counter"));
    assert!(!os.is_preinstalled("notes"));
}

#[test]
fn ambiguous_assemblies_are_refused() {
    let (first, _) = CountingApp::new("counter", "echo");
    let (second, _) = CountingApp::new("counter", "echo-2");
    let duplicate = MinimalOs::assemble(vec![Box::new(first), Box::new(second)])
        .expect_err("重复 App 标识必须被拒绝");
    assert_eq!(
        duplicate,
        AssemblyFailure::DuplicateAppId("counter".to_string())
    );
    assert_eq!(duplicate.as_str(), "duplicate-app-id");

    let (left, _) = CountingApp::new("left", "echo");
    let (right, _) = CountingApp::new("right", "echo");
    let conflict = MinimalOs::assemble(vec![Box::new(left), Box::new(right)])
        .expect_err("同一个行动标识被两个 App 声明必须被拒绝");
    assert_eq!(
        conflict,
        AssemblyFailure::ConflictingActionId {
            action: "echo".to_string(),
            owner: "left".to_string(),
        }
    );
    assert_eq!(conflict.as_str(), "conflicting-action-id");

    // OS 自己的展开行动不能被 App 占用。
    let reserved = MinimalOs::assemble(vec![Box::new(CountingApp::with_extra(
        "sneaky", "peek", EXPAND,
    ))])
    .expect_err("与展开行动冲突必须被拒绝");
    assert_eq!(
        reserved,
        AssemblyFailure::ConflictingActionId {
            action: EXPAND.to_string(),
            owner: "<os>".to_string(),
        }
    );

    // 同一个 App 自己声明两次同一个标识也属于"无法唯一解释"。
    let self_conflict = MinimalOs::assemble(vec![Box::new(CountingApp::with_extra(
        "double", "echo", "echo",
    ))])
    .expect_err("自身重复声明必须被拒绝");
    assert_eq!(
        self_conflict,
        AssemblyFailure::ConflictingActionId {
            action: "echo".to_string(),
            owner: "double".to_string(),
        }
    );
}

/// 5. 入口与展开只增加能力与说明：不代执行、不预读，也不触碰任何 App。
#[test]
fn entry_and_expansion_never_dispatch_an_app() {
    let (counter, visits) = CountingApp::new("counter", "echo");
    let mut os = MinimalOs::assemble(vec![Box::new(counter)]).expect("装配");
    let agent = AgentId::new("agent-a");

    {
        let run = RunId::new("run-1");
        let mut expansion5 = RunExpansion::new(agent.clone(), run.clone());
        let mut scope = os.scope(&mut expansion5);

        // 问能力目录不执行任何东西。
        assert_eq!(
            capability_ids(&scope, &agent, &run),
            vec![
                EXPAND.to_string(),
                "inspect_action".to_string(),
                "pin_action".to_string(),
                "unpin_action".to_string(),
                "write_self_concept".to_string(),
            ]
        );
        assert_eq!(visits.get(), 0);

        // 展开只带回能力说明，不执行替身的任何行动。
        let expanded = scope.execute(&agent, &operation("run-1"), &expand("counter"));
        assert_eq!(expanded.state(), ResultState::Success);
        assert!(expanded.content()[0].text().contains("echo"));
        assert_eq!(visits.get(), 0, "展开不得顺手执行 App 的行动");
    }

    // 新的一次活动从未展开开始：未展开就调用，明确拒绝，仍然一次都不碰 App。
    {
        let mut expansion6 = RunExpansion::new(agent.clone(), RunId::new("run-2"));
        let mut fresh = os.scope(&mut expansion6);
        let refused = fresh.execute(&agent, &operation("run-2"), &call("echo"));
        assert_eq!(
            refused.failure_class(),
            Some(ExpansionFailure::NotExpanded.as_str())
        );
        assert_eq!(visits.get(), 0);

        // 未知动作同样是明确失败，不被当作 App 的结果。
        let unknown = fresh.execute(&agent, &operation("run-2"), &call("no-such-action"));
        assert_eq!(
            unknown.failure_class(),
            Some(ExpansionFailure::UnknownAction.as_str())
        );
        assert_eq!(visits.get(), 0);
    }
}

/// 6. **展开回执只讲这一次新开的那个 App**（2026-10-14）。
///
/// 那一下的事实是"**这个 App 刚被打开**" ⇒ 回执只该讲它。从前渲染的是
/// `expanded_capabilities()`（这一段里**所有已开**的 App）⇒ 开第二个会把第一个的动词**再印
/// 一遍**：那既不是**新事实**（主体早看过那份回执），也不是**"看一眼"**（主体没要）—— 纯冗余。
///
/// ⚠ 这一条**只收窄"讲谁的"**，不动"给多全"：那一次给全量仍然是对的（它买的是"不必为每个动词
/// 各付一个回合"）。要把这条与"参数名要不要常驻在目录里"分开看：后者是**每一轮**的钱。
#[test]
fn an_expansion_receipt_only_covers_the_app_it_just_opened() {
    let (first, _) = CountingApp::new("counter", "echo");
    let (second, _) = CountingApp::new("ledger", "record");
    let mut os = MinimalOs::assemble(vec![Box::new(first), Box::new(second)]).expect("装配");
    let agent = AgentId::new("agent-a");
    let run = RunId::new("run-1");
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = os.scope(&mut expansion);

    let one = scope.execute(&agent, &operation("run-1"), &expand("counter"));
    assert_eq!(one.state(), ResultState::Success);
    assert!(
        one.content()[0].text().contains("echo"),
        "第一次展开：讲它自己：{}",
        one.content()[0].text()
    );

    let two = scope.execute(&agent, &operation("run-1"), &expand("ledger"));
    assert_eq!(two.state(), ResultState::Success);
    let receipt = two.content()[0].text();
    assert!(
        receipt.contains("record"),
        "第二次展开：讲刚开的那个：{receipt}"
    );
    assert!(
        !receipt.contains("echo"),
        "第二次**不许**再印第一个 App 的动词（既不是新事实、也不是看一眼）：{receipt}"
    );
}
