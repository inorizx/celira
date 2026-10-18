//! TASK_72 验收：**拒绝随单附卡**（诚实拒绝的完全体）。
//!
//! ## 这一刀治的是什么
//!
//! 主体盲调一个没查过、没钉住的工具 ⇒ 被 `needs-inspection` 拒 ⇒ **还得自己再花一轮**
//! `inspect_action` 取卡 ⇒ 第三轮才调对。拒绝发生在系统**最知道主体缺哪份事实**的时刻，却让主体
//! 再跑一趟去取那份事实：拍是预算里最贵的钱，而留在历史里的那次空跑是无用字节。
//!
//! 本刀把"那份事实"随**这一次拒绝的答复**一起给（同一个 `ContentBlock` 形状、同一份卡渲染），
//! 并且**卡进了主体的视野就算查过**——门语义一个字没动，只是交付时机提前了一轮。
//!
//! ## 这一份文件证明什么
//!
//! 全部走**真实驱动循环**（`RunSession` + 生产装配 + 脚本模型），判据是**主体的视野与轨迹**：
//!
//! 1. 命门：盲调 ⇒ 拒（分类不变）**且卡在场** ⇒ 下一轮**同一份载荷直调成功**，全程**没有**一次
//!    `inspect_action`（附卡不记 `inspected` 就是死循环装饰，这条不过整刀不成立）；
//! 2. `not-expanded` 的拒绝**不带任何随单内容**（三件里只有一件递东西）；
//! 3. 参数形状错 ⇒ 无卡重发，附**差异结构**（缺了哪个、多了哪个），而且分类串逐字节稳定；
//! 4. 卡与卡一致、拒绝分类不随随单而变的**渲染**判据在 `src/os.rs` 与 `src/provider_adapter.rs`
//!    的单元用例里（那里两个生产者都在手边）。
//!
//! ⚠ **2026-10-13 起这一份的射程收窄到「会改东西的动作」**：那一关今天问的是**这个动作会不会改
//! 外面的世界**（`EffectClass`），只读动作第一次调用**直接执行**（真机读数：125 次首调税里
//! 83 次落在只读动作上）。⇒ 本文件第 1 条换成 `notes_create`（声明了写类别），并另加一条
//! **对照用例**（`a_blind_read_runs_instead_of_being_stopped_for_its_shape`）钉住只读那一侧。

use std::collections::VecDeque;
use std::path::PathBuf;

use celira_agent::clock::ClockHandle;
use celira_agent::composition::{RunSession, SessionDeclarations, SessionDrive, SessionRequest};
use celira_agent::context_reading::ContextReading;
use celira_agent::core::{
    Action, ActionAdmission, ActionId, ActionParams, ActionPort, ContentBlock, Limits, ModelError,
    ModelFailureClass, ModelOutput, ModelPort, Observation, RunInput, TraceEntry, View,
};
use celira_agent::foundation::{AgentId, OperationId, ResultState, RunId, SubjectEnding, Turn};
use celira_agent::notes_app::{NOTES_APP_ID, NotesApp};
use celira_agent::os::{ExpansionFailure, MinimalOs, RunExpansion};
use celira_agent::store::Environment;

const AGENT: &str = "agent-refusal-remedy";
const RUN: &str = "run-refusal-remedy";
/// 宿主声明的上下文窗口（与判据无关；取一个固定值）。
const WINDOW: u32 = 1_000;

// ---------------------------------------------------------------------------
// 自建测试目录 / 记录型脚本模型（不指桌面、不指家目录、不指仓库根）
// ---------------------------------------------------------------------------

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        // 目录名**必须带进程号**：测试二进制是一份一个进程，而计数器只在进程内唯一 ⇒
        // 不带进程号的两份并行（或两个写者同时跑）会算出同一批路径、**互相删目录**。
        let mut path = std::env::temp_dir();
        path.push(format!("celira-refusal-{}-{label}-{n}", std::process::id()));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("清掉同名残留目录");
        }
        std::fs::create_dir_all(&path).expect("建立测试目录");
        Self { path }
    }

    fn env_root(&self) -> PathBuf {
        self.path.join("env")
    }

    fn materials(&self) -> PathBuf {
        let materials = self.path.join("materials");
        std::fs::create_dir_all(&materials).expect("建资料目录");
        materials
    }

    fn environment(&self) -> Environment {
        Environment::create(&self.env_root(), AgentId::new(AGENT)).expect("建立身份环境")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// 脚本模型：按序交出自己要做的事，并把**每一轮看到的轨迹**记下来。
///
/// 记轨迹（而不是记请求文本）是有意的：轨迹就是适配层渲染请求的那份事实源，因此"卡有没有进主体
/// 的视野"在这一层可判——渲染那半由 `src/provider_adapter.rs` 的单元用例钉住。
struct Recorder {
    steps: VecDeque<ModelOutput>,
    seen: Vec<Vec<TraceEntry>>,
}

impl Recorder {
    fn new(steps: Vec<ModelOutput>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            seen: Vec::new(),
        }
    }

    fn calls(&self) -> usize {
        self.seen.len()
    }
}

impl ModelPort for Recorder {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        self.seen.push(view.trace().to_vec());
        self.steps.pop_front().ok_or(ModelError {
            class: ModelFailureClass::ContractViolation,
        })
    }
}

/// 打开一次会话（生产入口，只有时间来源换成固定钟）。
fn session<'a>(dir: &TempDir, environment: &'a mut Environment, limits: u32) -> RunSession<'a> {
    let materials = dir.materials();
    let declared_agent = AgentId::new(AGENT);
    RunSession::open(
        &materials,
        environment,
        SessionRequest {
            declarations: SessionDeclarations::default(),
            declared_agent: &declared_agent,
            run: RunId::new(RUN),
            limits: Limits::new(limits),
            input: RunInput::text("开场输入").expect("在上限内"),
            clock: ClockHandle::fixed_at_unix_seconds(0),
            context_window: WINDOW,
            reading: ContextReading::default(),
        },
    )
    .expect("会话可以打开")
}

/// 一条拒绝的可读形状：动作标识、稳定分类、随单块。
type Refusal<'a> = (&'a str, &'a str, &'a [ContentBlock]);

/// 轨迹里的全部拒绝条目。
fn refusals(trace: &[TraceEntry]) -> Vec<Refusal<'_>> {
    trace
        .iter()
        .filter_map(|entry| match entry {
            TraceEntry::Refused {
                action,
                class,
                content,
                ..
            } => Some((action.as_str(), class.as_str(), content.as_slice())),
            _ => None,
        })
        .collect()
}

/// 轨迹里的全部观察。
fn observations(trace: &[TraceEntry]) -> Vec<&Observation> {
    trace
        .iter()
        .filter_map(|entry| match entry {
            TraceEntry::Observation(observation) => Some(observation),
            _ => None,
        })
        .collect()
}

/// 这个动作在轨迹里被**碰过**吗（被拒或被执行过都算）。
fn touched(trace: &[TraceEntry], id: &str) -> bool {
    refusals(trace).iter().any(|(action, _, _)| *action == id)
        || observations(trace)
            .iter()
            .any(|observation| observation.action().as_str() == id)
}

/// 驱动到收场，返回模型替身（用它读"每一轮主体看到了什么"）。
fn drive(session: &mut RunSession<'_>, steps: Vec<ModelOutput>) -> Recorder {
    let mut model = Recorder::new(steps);
    assert!(
        matches!(
            session.drive(&mut model).expect("驱动成功"),
            SessionDrive::Ended(_)
        ),
        "这一次应当以收场结束"
    );
    model
}

// ---------------------------------------------------------------------------
// 一、命门：盲调 ⇒ 拒（附卡）⇒ 同一份载荷直调成功，**全程没有 inspect_action**
// ---------------------------------------------------------------------------

#[test]
fn a_blind_call_is_refused_with_the_card_and_the_very_next_call_just_works() {
    let dir = TempDir::new("blind-call");
    let mut environment = dir.environment();
    let mut session = session(&dir, &mut environment, 4);

    // 一个**没查过、也没被钉住**的动词，而且它**会写**（`notes_create` 声明了写类别）
    // ——2026-10-13 起，"先看形状"那一关只挡这一类；只读动作第一次调用直接执行
    // （对照用例：`a_blind_read_runs_instead_of_being_stopped_for_its_shape`）。
    let blind = Action::with_params(
        ActionId::new("notes_create"),
        ActionParams::none()
            .with("title", "门")
            .with("text", "先看形状再动手"),
    );
    let model = drive(
        &mut session,
        vec![
            ModelOutput::act(blind.clone()),
            ModelOutput::act(blind.clone()),
            ModelOutput::finish(SubjectEnding::Completed),
        ],
    );

    let trace = session.trace();

    // (1) 第一轮：**被拒**，分类没变，而卡随这次拒绝一起给了主体。
    let refused = refusals(trace);
    assert_eq!(refused.len(), 1, "恰好一次拒绝：第二拍就放行了");
    let (action, class, content) = refused[0];
    assert_eq!(action, "notes_create");
    assert_eq!(class, "needs-inspection", "分类词一字没动");
    assert_eq!(content.len(), 1, "随单恰好一块：那张卡");
    assert_eq!(content[0].label(), "notes_create", "卡带着它自己的标签");
    assert!(
        content[0].text().contains("notes_create: "),
        "卡里是骨架 + 详情 + 参数表: {}",
        content[0].text()
    );

    // (2) 第二轮：**同一份载荷直接执行成功**——没有第二次拒绝，也没有"再跑一趟取卡"。
    let done: Vec<&Observation> = observations(trace)
        .into_iter()
        .filter(|observation| observation.action().as_str() == "notes_create")
        .collect();
    assert_eq!(done.len(), 1, "它只被执行过一次（第一次是拒绝，不是结果）");
    assert_eq!(done[0].state(), ResultState::Success, "直调成功");

    // (3) **全程没有 `inspect_action`**：卡是随拒绝给的，不是主体再拉一轮取来的。
    assert!(
        !touched(trace, "inspect_action"),
        "这一段经历里不许出现 inspect_action（附卡的意义就在这里）"
    );
    assert_eq!(
        model.calls(),
        3,
        "三拍：盲调 / 直调 / 收场（若是四拍就说明卡没随单给）"
    );

    // (4) 主体**第二轮**看到的轨迹里，那张卡已经在场：拒绝与卡**同一次答复**到。
    let second = &model.seen[1];
    assert!(
        refusals(second)
            .iter()
            .any(|(_, _, content)| content.iter().any(|block| block.label() == "notes_create")),
        "第二拍之前，那张卡必须已经进过主体的视野"
    );
}

/// **只读动作不收"先看形状"的税**（2026-10-13）：盲调它**直接执行**——不拒绝、不附卡，
/// 而"形状"照旧按需拿得到（`inspect_action` 那条路一个字没动）。
///
/// 判据在 `os.rs::admit`：那一关问的是**这个动作会不会改外面的世界**（`EffectClass`）。
/// 读一个字节都改不了，而"先看一眼"的代价是**一轮模型调用**。
/// 真机读数（小欣 22 段 Run）：125 次首调税里 **83 次（66%）落在只读动作上**。
#[test]
fn a_blind_read_runs_instead_of_being_stopped_for_its_shape() {
    let dir = TempDir::new("blind-read");
    let mut environment = dir.environment();
    let mut session = session(&dir, &mut environment, 4);

    // `notes_list` **没有**效果声明（`notes_app` 只声明它的那几件写）⇒ 只读。
    let blind = Action::new(ActionId::new("notes_list"));
    let model = drive(
        &mut session,
        vec![
            ModelOutput::act(blind.clone()),
            ModelOutput::finish(SubjectEnding::Completed),
        ],
    );

    let trace = session.trace();

    // (1) **一次拒绝都没有**：读动作第一次调用直接执行。
    assert!(
        refusals(trace).is_empty(),
        "只读动作不该被「先看形状」拦下: {:?}",
        refusals(trace)
    );
    // (2) 它照旧**只执行一次**、而且成功。
    let done: Vec<&Observation> = observations(trace)
        .into_iter()
        .filter(|observation| observation.action().as_str() == "notes_list")
        .collect();
    assert_eq!(done.len(), 1, "一拍就干完了");
    assert_eq!(done[0].state(), ResultState::Success);
    // (3) 也**没有**多花一轮去取卡：形状按需，不挡执行。
    assert!(!touched(trace, "inspect_action"), "读它不需要先取卡");
    assert_eq!(model.calls(), 2, "两拍：直调 / 收场");
}

// ---------------------------------------------------------------------------
// 二、`not-expanded` 的拒绝**不带**任何随单内容（三件里只有一件递东西）
// ---------------------------------------------------------------------------

#[test]
fn a_not_expanded_refusal_brings_nothing_along() {
    let dir = TempDir::new("not-expanded");
    let mut environment = dir.environment();
    let mut session = session(&dir, &mut environment, 4);

    // files 装上了但**没有预装常开**（生产装配里只有 notes / web / im 那一档是预装的）⇒
    // 它的动词要先 `open_app` 才谈得上"用"。
    let blind = Action::new(ActionId::new("files_list"));
    drive(
        &mut session,
        vec![
            ModelOutput::act(blind),
            ModelOutput::finish(SubjectEnding::Completed),
        ],
    );

    let refused = refusals(session.trace());
    assert_eq!(refused.len(), 1);
    let (action, class, content) = refused[0];
    assert_eq!(action, "files_list");
    assert_eq!(class, "not-expanded", "分类没变");
    assert!(
        content.is_empty(),
        "没展开的拒绝不附东西（开场目录已列装了哪些 App）: {content:?}"
    );
}

// ---------------------------------------------------------------------------
// 三、同一次装配里的两条路：随单的卡 == 查卡交回的卡（逐字节）；参数差异只写真话
// ---------------------------------------------------------------------------

/// 装一个**预装常开**的 notes（它的动词从第一轮起就在场），不带任何 pin。
///
/// 返回装配好的两件；`scope` 由每个用例自己建（借用生命期只在用例内）。
fn notes_parts(root: &std::path::Path, agent: &AgentId, run: &RunId) -> (MinimalOs, RunExpansion) {
    let notes = NotesApp::open(root).expect("Notes App");
    let os = MinimalOs::assemble_preinstalled(vec![Box::new(notes)], &[NOTES_APP_ID])
        .expect("可以装配")
        .with_environment_root(root);
    (os, RunExpansion::new(agent.clone(), run.clone()))
}

fn operation(run: &RunId) -> OperationId {
    OperationId::new(run.clone(), Turn::FIRST)
}

/// `inspect_action` 的载荷（动作标识按名字引用，与既有用例同一惯例）。
fn inspect(name: &str) -> Action {
    Action::with_params(
        ActionId::new("inspect_action"),
        ActionParams::none().with("action", name),
    )
}

/// **随单的卡与查卡交回的是同一份渲染**（判据 4）：逐字节相同。
///
/// 同一次装配、同一个动作，两条路各取一次：准入面拒绝时附上的那一块，与主体主动
/// `inspect_action` 拿回来的那一块，必须一模一样——两处各写一份渲染就迟早分叉。
#[test]
fn a_refusal_brings_the_same_card_the_inspect_path_brings() {
    let root = TempDir::new("card").env_root();
    std::fs::create_dir_all(&root).expect("建临时根");
    let agent = AgentId::new("agent-refusal-card");
    let run = RunId::new("run-refusal-card");
    let (mut os, mut expansion) = notes_parts(&root, &agent, &run);
    let mut scope = os.scope(&mut expansion);

    // 一个**没查过、也没被钉住**、而且**会写**的动词（`notes_create` 声明了写类别）——
    // "先看形状"那一关 2026-10-13 起只挡这一类（只读动作直接执行）。
    let blind = Action::with_params(
        ActionId::new("notes_create"),
        ActionParams::none()
            .with("title", "卡")
            .with("text", "两处渲染必须逐字节相同"),
    );
    let refused = scope.admit(&agent, &run, &blind);
    let ActionAdmission::Refused { class, content } = &refused else {
        panic!("没查过又没钉住的动作应当被拒: {refused:?}");
    };
    assert_eq!(class, "needs-inspection", "分类词没变");
    assert_eq!(content.len(), 1, "随单恰好一块：那张卡");
    assert_eq!(content[0].label(), "notes_create");

    // 同一个动作，走主体主动查卡那条路。
    let card = scope.execute(&agent, &operation(&run), &inspect("notes_create"));
    assert_eq!(card.state(), ResultState::Success);
    assert_eq!(
        content.as_slice(),
        card.content(),
        "随单的卡与查卡交回的是同一份渲染（逐字节）"
    );

    // 而且**卡进过视野就算查过**：同一份载荷从此直接可调（免查判据没动：查过 ∪ 被钉住）。
    assert_eq!(
        scope.admit(&agent, &run, &blind),
        ActionAdmission::Admitted,
        "随单附卡也写 inspected ⇒ 不再死循环"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **参数形状错**：分类词只有一个、逐字节稳定；差异是它旁边的结构化读数。
///
/// 主体当场就知道缺了哪个、多了哪个，不必再去翻那张卡；分类串本身不含任何动态文本。
#[test]
fn a_parameter_shape_refusal_names_what_is_missing_and_unexpected() {
    let root = TempDir::new("params").env_root();
    std::fs::create_dir_all(&root).expect("建临时根");
    let agent = AgentId::new("agent-refusal-params");
    let run = RunId::new("run-refusal-params");
    let (mut os, mut expansion) = notes_parts(&root, &agent, &run);
    let mut scope = os.scope(&mut expansion);

    // `open_app` 只吃 `target`：这里**多给一个**、**少给一个**。
    let wrong = Action::with_params(
        ActionId::new("open_app"),
        ActionParams::none().with("unexpected_name", "x"),
    );
    let outcome = scope.execute(&agent, &operation(&run), &wrong);

    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(
        outcome.failure_class(),
        Some(ExpansionFailure::InvalidParameters.as_str()),
        "分类词就是那一个、逐字节稳定"
    );
    assert_eq!(outcome.content().len(), 1, "差异是旁边的结构化读数");
    assert_eq!(outcome.content()[0].label(), "parameters");
    let text = outcome.content()[0].text();
    assert!(
        text.contains("missing = target"),
        "缺了哪个要写出来: {text}"
    );
    assert!(
        text.contains("unexpected = unexpected_name"),
        "多了哪个也要写出来: {text}"
    );
    assert!(
        text.contains("动作卡"),
        "还要指向主体的视野里的参数表: {text}"
    );

    // 同一份错误参数两次发生 ⇒ 这段读数逐字节相同。
    let again = scope.execute(&agent, &operation(&run), &wrong);
    assert_eq!(again.content(), outcome.content());
    assert_eq!(again.failure_class(), outcome.failure_class());

    let _ = std::fs::remove_dir_all(&root);
}

/// **只有缺**（名字全对但没给）时，差异里只有 `missing` 那一行——不为空的那一项发明一行。
#[test]
fn a_shape_refusal_says_only_what_is_true() {
    let root = TempDir::new("params-missing-only").env_root();
    std::fs::create_dir_all(&root).expect("建临时根");
    let agent = AgentId::new("agent-refusal-missing");
    let run = RunId::new("run-refusal-missing");
    let (mut os, mut expansion) = notes_parts(&root, &agent, &run);
    let mut scope = os.scope(&mut expansion);

    let outcome = scope.execute(
        &agent,
        &operation(&run),
        &Action::new(ActionId::new("open_app")),
    );
    assert_eq!(outcome.failure_class(), Some("invalid-parameters"));
    let text = outcome.content()[0].text();
    assert!(text.contains("missing = target"), "{text}");
    assert!(
        !text.contains("unexpected ="),
        "没有多给的，就不写那一行: {text}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// **一次拒绝要指路**（2026-10-13）：失败回执**旁边**要给出"哪个名字不在表里，以及
/// **这个动作接受哪些名字**"——她缺的正是那一格。
///
/// 真机读数（小欣，22 段 Run）：同一个 `notes_read` 被猜过**五种**参数名
/// （`note` / `note_id` / `identity` / `args` / `action`），13 次 `invalid-parameter`。
/// 分类词一个字没动（那一个词照旧由 App 给）。
#[test]
fn an_app_refusal_names_the_parameter_it_does_not_know() {
    let root = TempDir::new("app-params").env_root();
    std::fs::create_dir_all(&root).expect("建临时根");
    let agent = AgentId::new("agent-app-params");
    let run = RunId::new("run-app-params");
    let (mut os, mut expansion) = notes_parts(&root, &agent, &run);
    let mut scope = os.scope(&mut expansion);

    // (1) 她真机上试过的那一种错法：`notes_read(note="note-1")`——**名字不对**。
    let wrong = Action::with_params(
        ActionId::new("notes_read"),
        ActionParams::none().with("note", "note-1"),
    );
    let outcome = scope.execute(&agent, &operation(&run), &wrong);
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(
        outcome.failure_class(),
        Some("invalid-parameter"),
        "分类词逐字节不变（**notes 自己的那个词**——这一层不认分类，也不改它）"
    );
    let text = outcome.content()[0].text();
    assert!(text.contains("unexpected = note"), "{text}");
    assert!(
        text.contains("accepted = id (required)"),
        "要说清**对的是什么**——她缺的正是这一格: {text}"
    );

    // (2) 名字都对、只是**没给**必须的那一个 ⇒ 只写 `missing`（不为没有的那一项发明一行）。
    let outcome = scope.execute(
        &agent,
        &operation(&run),
        &Action::new(ActionId::new("notes_read")),
    );
    assert_eq!(outcome.failure_class(), Some("invalid-parameter"));
    let text = outcome.content()[0].text();
    assert!(text.contains("missing = id"), "{text}");
    assert!(!text.contains("unexpected ="), "{text}");

    // (3) **反例**：名字全对、App 因为**取值**拒绝（空标题）⇒ **什么都不加**，
    //     回执逐字节等于从前——读数只在真有机械事实可说时才出现。
    let outcome = scope.execute(
        &agent,
        &operation(&run),
        &Action::with_params(
            ActionId::new("notes_create"),
            ActionParams::none().with("title", "").with("text", "x"),
        ),
    );
    assert_eq!(outcome.failure_class(), Some("invalid-parameter"));
    assert!(
        outcome.content().is_empty(),
        "名字层面没有可说的事实 ⇒ 不夹带读数: {:?}",
        outcome.content()
    );

    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// 四、参数形状错走**整条驱动循环**：分类与差异都要真的进主体的视野，且两次逐字节相同
// ---------------------------------------------------------------------------
/// 一次参数错的读数形状：稳定分类 + 随单块（标签、正文）。
type ShapeReading = (Option<String>, Vec<(String, String)>);

#[test]
fn a_wrong_parameter_shape_comes_back_with_the_diff_and_a_stable_class() {
    // 同一份错参数**两次**（两次驱动各一次），分类与差异都必须逐字节相同。
    let mut seen: Vec<ShapeReading> = Vec::new();
    for attempt in ["first", "second"] {
        let dir = TempDir::new(&format!("params-{attempt}"));
        let mut environment = dir.environment();
        let mut session = session(&dir, &mut environment, 4);

        // `open_app` 只吃 `target`：这里**多给一个**（`oops`）、**少给一个**（target）。
        let wrong = Action::with_params(
            ActionId::new("open_app"),
            ActionParams::none().with("oops", "x"),
        );
        drive(
            &mut session,
            vec![
                ModelOutput::act(wrong),
                ModelOutput::finish(SubjectEnding::Completed),
            ],
        );

        let trace = session.trace();
        let done: Vec<&Observation> = observations(trace)
            .into_iter()
            .filter(|observation| observation.action().as_str() == "open_app")
            .collect();
        assert_eq!(
            done.len(),
            1,
            "它被执行过一次（参数形状错是**结果**，不是准入拒绝）"
        );

        let outcome = done[0].outcome();
        assert_eq!(outcome.state(), ResultState::Failure);
        assert_eq!(outcome.failure_class(), Some("invalid-parameters"));
        assert_eq!(outcome.content().len(), 1, "差异是旁边的结构化读数");
        let blocks: Vec<(String, String)> = outcome
            .content()
            .iter()
            .map(|block| (block.label().to_string(), block.text().to_string()))
            .collect();
        let text = &blocks[0].1;
        assert!(text.contains("missing = target"), "{text}");
        assert!(text.contains("unexpected = oops"), "{text}");
        // **无卡重发**：这一块不是"那张卡"（标签不是动作名）。
        assert_ne!(blocks[0].0, "open_app", "参数错时不重发卡: {blocks:?}");

        seen.push((outcome.failure_class().map(str::to_string), blocks));
    }

    assert_eq!(
        seen[0], seen[1],
        "同一故障两次发生：分类串与差异读数都必须逐字节相同"
    );
}
