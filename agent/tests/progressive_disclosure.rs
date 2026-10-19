//! 渐进式披露的离线证明：能力按需展开、经历只追加、授权与经历独立。
//!
//! 全部离线：不联网、不需要密钥、只用自建临时沙盒与合成内容。
//!
//! 与旧的"页面导航"契约相比，这里断言的是**历史保留**而不是"离开页面后隐藏正文"：
//! 展开只控制接下来新增什么能力，不会让已经发生的经历消失。

use std::fs;
use std::path::{Path, PathBuf};

use celira_agent::composition::run_read_only_files;
use celira_agent::core::{
    Action, ActionAdmission, ActionId, ActionParams, ActionPort, Agent, Limits, ModelError,
    ModelFailureClass, ModelOutput, ModelPort, TraceEntry, View, run_activity,
};
use celira_agent::file_app::FILE_APP_ID;
use celira_agent::foundation::{
    AgentId, Ledger, OperationId, Outcome, ResultState, RunId, SubjectEnding, SystemEnding, Turn,
};
use celira_agent::notes_app::{NOTES_APP_ID, NotesApp};
use celira_agent::os::{ExpansionFailure, MinimalOs, RunExpansion};
use celira_agent::pinned_actions::MAX_PINS;

mod common;

use common::InspectsFirst;

/// 展开动作的标识（由 OS 声明；测试按名字引用）。
const EXPAND: &str = "open_app";
/// 文件 App 的两项能力（由 App 声明；测试按名字引用）。
const LIST: &str = "files_list";
const READ: &str = "files_read";

/// 自建临时沙盒：只在系统临时目录下创建，用完自己删掉。
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let unique = format!(
            "celira-disclosure-{}-{}-{}",
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
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("可以建子目录");
        }
        fs::write(path, text).expect("可以写合成文件");
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// 看一个动作的卡片（TASK_39 的元工具）。
fn inspect(name: &str) -> Action {
    Action::with_params(
        ActionId::new("inspect_action"),
        ActionParams::none().with("action", name),
    )
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

fn read(relative: &str) -> Action {
    Action::with_params(
        ActionId::new(READ),
        ActionParams::none().with("path", relative),
    )
}

/// 直接驱动 OS 用的操作身份（真实流程里由 Core 产生；这里只用它携带 Run 身份）。
fn operation(run: &str) -> OperationId {
    OperationId::new(RunId::new(run), Turn::FIRST)
}

/// 会读视野的模型：依据**实际呈现的能力**决定下一步，并逐轮记录看到的能力与经历。
///
/// 它不消费预排的完整动作序列：展开、列目录、选哪个文件都由当轮视野决定。
struct DisclosureModel {
    /// 每次调用看到的（能力标识, 经历条数, 经历里的正文）。
    seen: Vec<(Vec<String>, usize, Vec<String>)>,
    read: Vec<String>,
    expanded: bool,
    re_expanded: bool,
}

impl DisclosureModel {
    fn new() -> Self {
        Self {
            seen: Vec::new(),
            read: Vec::new(),
            expanded: false,
            re_expanded: false,
        }
    }

    fn presented(view: &View, id: &str) -> bool {
        view.capabilities().find(&ActionId::new(id)).is_some()
    }

    /// 视野里最后一条列目录的内容里，挑一个还没读过的 `.txt`。
    fn unread_text_file(&self, view: &View) -> Option<String> {
        view.observed()
            .iter()
            .rev()
            .find(|observation| observation.action().as_str() == LIST)
            .and_then(|listing| {
                listing
                    .outcome()
                    .content()
                    .iter()
                    .flat_map(|block| block.text().lines())
                    .find_map(|line| {
                        let (name, kind) = line.split_once('\t')?;
                        (kind == "file"
                            && name.ends_with(".txt")
                            && !self.read.iter().any(|done| done == name))
                        .then(|| name.to_string())
                    })
            })
    }
}

impl ModelPort for DisclosureModel {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        self.seen.push((
            view.capabilities()
                .items()
                .iter()
                .map(|capability| capability.id().as_str().to_string())
                .collect(),
            view.observed().len(),
            view.observed()
                .iter()
                .flat_map(|observation| observation.outcome().content().iter())
                .map(|block| block.text().to_string())
                .collect(),
        ));

        // 还没有文件能力可用：先展开。
        if !Self::presented(view, LIST) {
            self.expanded = true;
            return Ok(ModelOutput::act(expand(FILE_APP_ID)));
        }

        // 展开之后还没看过目录：先列目录。
        if !view
            .observed()
            .iter()
            .any(|observation| observation.action().as_str() == LIST)
        {
            return Ok(ModelOutput::act(list()));
        }

        // 读了一个文件之后，再展开一次（能力只增不减，之前的正文不该消失）。
        if !self.read.is_empty() && !self.re_expanded {
            self.re_expanded = true;
            return Ok(ModelOutput::act(expand(FILE_APP_ID)));
        }

        match self.unread_text_file(view) {
            Some(name) => {
                self.read.push(name.clone());
                Ok(ModelOutput::act(read(&name)))
            }
            None => Ok(ModelOutput::finish(SubjectEnding::Completed)),
        }
    }
}

/// 按脚本作答的模型。
struct ScriptedModel {
    script: Vec<ModelOutput>,
}

impl ModelPort for ScriptedModel {
    fn complete(&mut self, _view: &View) -> Result<ModelOutput, ModelError> {
        Ok(self.script.remove(0))
    }
}

/// 1. 初始只有 App 入口：没有正文、没有目录、没有文件能力；模型直接 Finish 也不会自动展开。
#[test]
fn the_entry_only_announces_the_app_and_never_expands_itself() {
    let sandbox = Sandbox::new("entry");
    let marker = "entry-marker-not-disclosed";
    sandbox.write("secret.txt", marker);

    let agent = AgentId::new("agent-a");
    let run_id = RunId::new("run-1");
    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");
    let mut expansion = RunExpansion::new(agent.clone(), run_id.clone());
    let mut scope = os.scope(&mut expansion);

    let available = scope.capabilities(&agent, &run_id);
    assert_eq!(
        available.len(),
        5,
        "入口提供两项：展开 + 看详情（TASK_39：详情是尾部那一半，与展开同层常驻）"
    );
    let expand = available.find(&ActionId::new(EXPAND)).expect("展开在场");
    assert_eq!(expand.id().as_str(), EXPAND);
    assert!(
        expand.details().contains(FILE_APP_ID),
        "入口必须给出 App 的稳定标识（它在详情里）: {}",
        expand.details()
    );
    assert!(
        available.find(&ActionId::new("inspect_action")).is_some(),
        "看详情与展开同层，常驻在场"
    );
    assert!(available.find(&ActionId::new(LIST)).is_none());
    assert!(available.find(&ActionId::new(READ)).is_none());

    // 模型直接结束：不展开、不读取，能力集合不变。
    let mut model = ScriptedModel {
        script: vec![ModelOutput::finish(SubjectEnding::Silent)],
    };
    let mut agent_state = Agent::new(agent.clone());
    let mut ledger = Ledger::new(agent.clone());
    let run = run_activity(
        &mut agent_state,
        run_id.clone(),
        Limits::new(2),
        &mut model,
        &mut scope,
        &mut ledger,
    )
    .expect("活动可以跑完");

    assert_eq!(run.outcome(), Some(Outcome::Subject(SubjectEnding::Silent)));
    assert!(run.observed().is_empty(), "没有发生任何行动");
    assert_eq!(
        scope.capabilities(&agent, &run_id).len(),
        5,
        "Finish 之后仍然只有入口能力，系统没有自动展开"
    );
    assert!(!format!("{:?}", ledger.records()).contains(marker));
}

/// 2. 模型依据实际入口选择展开；下一轮才出现文件能力；展开本身不列目录、不读文件。
#[test]
fn the_model_expands_from_what_it_sees_and_expansion_reads_nothing() {
    let sandbox = Sandbox::new("expand");
    let marker = "expand-marker";
    sandbox.write("note.txt", marker);

    let mut model = InspectsFirst::new(DisclosureModel::new());
    let result = run_read_only_files(
        sandbox.path(),
        AgentId::new("agent-a"),
        RunId::new("run-1"),
        Limits::new(6),
        &mut model,
    )
    .expect("装配并跑完");

    // 第一次调用：只有展开能力，没有任何经历。
    assert_eq!(
        model.inner().seen[0].0,
        vec![
            EXPAND.to_string(),
            "inspect_action".to_string(),
            "pin_action".to_string(),
            "unpin_action".to_string(),
            "write_self_concept".to_string(),
        ]
    );
    // TASK_69：展开之前**没有任何业务经历**——那 5 条是主体开场"先看格式"的结果
    // （入场能力的逐条查阅，一轮整批落下），不是任何 App 干出来的事。
    assert_eq!(
        model.inner().seen[0].1,
        5,
        "展开之前只有'先看格式'那 5 条，没有任何业务经历"
    );
    // 那 5 条只有能力卡片，**没有正文**：展开之前沙盒里的东西一个字都没露出来。
    assert!(
        model.inner().seen[0]
            .2
            .iter()
            .all(|text| !text.contains(marker)),
        "展开之前不得出现沙盒正文"
    );

    // 展开之后：入口能力仍在，文件能力新增（能力只增不减）。
    let after_expand = &model.inner().seen[1].0;
    assert!(
        after_expand.contains(&EXPAND.to_string()),
        "入口能力不得被顶掉"
    );
    assert!(after_expand.contains(&LIST.to_string()));
    assert!(after_expand.contains(&READ.to_string()));

    // 展开这一次行动本身只带回能力说明，不带目录内容或正文。
    //
    // TASK_69：**按动作标识定位**——开场那批 `inspect_action` 也在结果里，"第 0 个"已经不是展开了。
    let observed = result.run.observed();
    let first_observation = observed
        .iter()
        .find(|observation| observation.action().as_str() == EXPAND)
        .expect("展开那一次必须留下结果");
    assert_eq!(first_observation.action().as_str(), EXPAND);
    assert_eq!(first_observation.state(), ResultState::Success);
    let expanded_text: String = first_observation
        .outcome()
        .content()
        .iter()
        .map(|block| block.text())
        .collect();
    assert!(
        expanded_text.contains(LIST),
        "能力说明要留痕: {expanded_text}"
    );
    assert!(
        expanded_text.contains(READ),
        "能力说明要留痕: {expanded_text}"
    );
    assert!(
        !expanded_text.contains(marker) && !expanded_text.contains("note.txt"),
        "展开不得带回目录内容或正文: {expanded_text}"
    );
}

/// 3. 展开 → list → read A → **再次展开** → read B：经历只追加、前缀扩展、A 的正文保留。
#[test]
fn the_run_trace_only_grows_and_earlier_bodies_survive_re_expansion() {
    let sandbox = Sandbox::new("trace");
    let marker_a = "trace-marker-A";
    let marker_b = "trace-marker-B";
    sandbox.write("alpha.txt", marker_a);
    sandbox.write("beta.txt", marker_b);

    let mut model = InspectsFirst::new(DisclosureModel::new());
    let result = run_read_only_files(
        sandbox.path(),
        AgentId::new("agent-a"),
        RunId::new("run-1"),
        Limits::new(8),
        &mut model,
    )
    .expect("装配并跑完");

    // 实际发生的顺序：expand → list → read A → expand → read B → finish。
    //
    // TASK_69：**只看真实业务动作**——主体开场那几批 `inspect_action`（"先看格式"）也是行动，
    // 混进来会让这份序列随手数一变就错位。这一段要证明的是"**业务步骤**每一步都是模型自己选的、
    // 顺序就是发生顺序"，所以按 `inspect_action` 之外的动作取序列。
    let actions: Vec<&str> = result
        .run
        .observed()
        .iter()
        .map(|observation| observation.action().as_str())
        .filter(|name| *name != "inspect_action")
        .collect();
    assert_eq!(
        actions,
        vec![EXPAND, LIST, READ, EXPAND, READ],
        "每一步都是模型自己选的，顺序就是发生顺序"
    );
    assert_eq!(model.inner().read, vec!["alpha.txt", "beta.txt"]);

    // 每一轮的经历是前一轮的前缀扩展：条数单调 +1，且没有重复插入。
    // 每一轮的经历是前一轮的前缀扩展。
    //
    // TASK_69：主体开场那几批"先看格式"（`inspect_action`）**也在经历里**，而且是整批一次落下的
    // ⇒ 条数不再是"每轮 +1"。那条"+1"是旧形态（一轮一动作）的算术巧合；本判据真正要守的是
    // **只追加、不重排、不重复**，所以这里断言的是"严格递增 + 逐字节前缀"。
    let counts: Vec<usize> = model
        .inner()
        .seen
        .iter()
        .map(|(_, count, _)| *count)
        .collect();
    assert!(
        counts.windows(2).all(|pair| pair[0] < pair[1]),
        "只追加，不重排、不重复: {counts:?}"
    );
    let rounds: Vec<&Vec<String>> = model
        .inner()
        .seen
        .iter()
        .map(|(_, _, bodies)| bodies)
        .collect();
    for pair in rounds.windows(2) {
        let (earlier, later) = (&pair[0], &pair[1]);
        assert!(
            later.len() >= earlier.len() && later[..earlier.len()] == earlier[..],
            "每一轮必须是上一轮的前缀扩展: {earlier:?} -> {later:?}"
        );
    }

    // A 的正文在读到它的那一轮之后**每一轮**都还在（包括再次展开之后）。
    for (index, (_, _, bodies)) in model.inner().seen.iter().enumerate() {
        if index >= 3 {
            assert!(
                bodies.iter().any(|text| text.contains(marker_a)),
                "第 {} 轮的轨迹里丢了 A 的正文: {bodies:?}",
                index + 1
            );
        }
    }
    // 最后一次模型看到的是 A 在前、B 在后，各自恰好一次。
    let last = &model.inner().seen.last().expect("至少调用过一次").2;
    let joined = last.join("\n");
    let a_at = joined.find(marker_a).expect("A 还在");
    let b_at = joined.find(marker_b).expect("B 也在");
    assert!(a_at < b_at, "顺序必须保持发生顺序");
    assert_eq!(joined.matches(marker_a).count(), 1, "每条经历恰好一次");
    assert_eq!(joined.matches(marker_b).count(), 1, "每条经历恰好一次");

    // 再次展开只留下一条明确痕迹，不重读、不复制正文。
    //
    // TASK_69：**按动作标识定位**，不按"第几个结果"——一轮多动作（批量 `inspect_action`）之后，
    // "第 N 个结果"与"第 N 个业务动作"已经脱钩（这是锚点偏移那一类的根源）。
    let observed = result.run.observed();
    let expansions: Vec<_> = observed
        .iter()
        .filter(|observation| observation.action().as_str() == EXPAND)
        .collect();
    let second_expand = expansions[1];
    assert_eq!(second_expand.action().as_str(), EXPAND);
    assert_eq!(second_expand.state(), ResultState::Success);
    let text: String = second_expand
        .outcome()
        .content()
        .iter()
        .map(|block| block.text())
        .collect();
    assert!(
        !text.contains(marker_a) && !text.contains("alpha.txt"),
        "重复展开不得复制之前正文: {text}"
    );
}

/// 5. 未展开伪造读取被拒；展开后同一合法读取成功；未知目标/错误参数/旧 back 被拒且不改变能力。
#[test]
fn availability_is_not_tied_to_a_display_area() {
    let sandbox = Sandbox::new("availability");
    sandbox.write("readable.txt", "availability-marker");
    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");
    let agent = AgentId::new("agent-a");
    let mut expansion2 = RunExpansion::new(agent.clone(), RunId::new("run-1"));
    let mut scope = os.scope(&mut expansion2);

    // 未展开：伪造文件能力 → 明确拒绝，而且用的路径本来能读成功（说明没到文件 App）。
    for forged in [read("readable.txt"), list()] {
        let outcome = scope.execute(&agent, &operation("run-1"), &forged);
        assert_eq!(outcome.state(), ResultState::Failure);
        assert_eq!(
            outcome.failure_class(),
            Some(ExpansionFailure::NotExpanded.as_str()),
            "{} 未展开时必须被拒绝",
            forged.id().as_str()
        );
        assert!(outcome.content().is_empty());
    }

    // 展开之后同一个动作成功。
    assert_eq!(
        scope
            .execute(&agent, &operation("run-1"), &expand(FILE_APP_ID))
            .state(),
        ResultState::Success
    );
    let allowed = scope.execute(&agent, &operation("run-1"), &read("readable.txt"));
    assert_eq!(allowed.state(), ResultState::Success);
    assert!(allowed.content()[0].text().contains("availability-marker"));

    // 失败与未知都不会改变已有能力。
    let before = scope.capabilities(&agent, &RunId::new("run-1")).len();

    let wrong_target = scope.execute(&agent, &operation("run-1"), &expand("not-assembled"));
    assert_eq!(
        wrong_target.failure_class(),
        Some(ExpansionFailure::UnknownTarget.as_str())
    );

    let missing_param = scope.execute(
        &agent,
        &operation("run-1"),
        &Action::with_params(ActionId::new(EXPAND), ActionParams::none()),
    );
    assert_eq!(
        missing_param.failure_class(),
        Some(ExpansionFailure::InvalidParameters.as_str())
    );

    let extra_param = scope.execute(
        &agent,
        &operation("run-1"),
        &Action::with_params(
            ActionId::new(EXPAND),
            ActionParams::none()
                .with("target", FILE_APP_ID)
                .with("深度", "3"),
        ),
    );
    assert_eq!(
        extra_param.failure_class(),
        Some(ExpansionFailure::InvalidParameters.as_str())
    );

    // 旧动作 back 已移除：明确当作未知动作拒绝，不做隐藏兼容入口。
    let old_back = scope.execute(
        &agent,
        &operation("run-1"),
        &Action::with_params(ActionId::new("back"), ActionParams::none()),
    );
    assert_eq!(
        old_back.failure_class(),
        Some(ExpansionFailure::UnknownAction.as_str())
    );

    assert_eq!(
        scope.capabilities(&agent, &RunId::new("run-1")).len(),
        before,
        "失败与未知动作不得意外改变能力集合"
    );
    // 读取仍然照旧工作，且仍然走 App 自己的校验。
    assert_eq!(
        scope
            .execute(&agent, &operation("run-1"), &read("../escape.txt"))
            .failure_class(),
        Some("invalid-path")
    );
}

/// 6. 重复展开没有文件 I/O、不自动 list/read：用可观察结果证明，而不是只看文字。
#[test]
fn re_expansion_does_no_file_work() {
    let sandbox = Sandbox::new("re-expand");
    let marker = "re-expand-marker";
    sandbox.write("alpha.txt", marker);
    sandbox.write("beta.txt", "re-expand-marker-beta");

    let mut model = InspectsFirst::new(DisclosureModel::new());
    let result = run_read_only_files(
        sandbox.path(),
        AgentId::new("agent-a"),
        RunId::new("run-1"),
        Limits::new(8),
        &mut model,
    )
    .expect("装配并跑完");

    // TASK_69：**只看真实业务动作**——主体开场那几批 `inspect_action`（"先看格式"）也是行动，
    // 混进来会让"第几条就是再次展开"随手数一变就错位。
    let observed = result.run.observed();
    let observations: Vec<_> = observed
        .iter()
        .filter(|observation| observation.action().as_str() != "inspect_action")
        .collect();
    // 第 4 条就是"再次展开"：它既没有带回目录内容，也没有带回正文。
    let second = observations[3];
    assert_eq!(second.action().as_str(), EXPAND);
    let text: String = second
        .outcome()
        .content()
        .iter()
        .map(|block| block.text())
        .collect();
    assert!(!text.contains(marker), "不得自动重读正文: {text}");
    assert!(!text.contains('\t'), "不得附带目录条目: {text}");

    // 整个运行里 list 与 read 各只发生了一次：重复展开没有替模型多做一步。
    let lists = observations
        .iter()
        .filter(|observation| observation.action().as_str() == LIST)
        .count();
    let reads = observations
        .iter()
        .filter(|observation| observation.action().as_str() == READ)
        .count();
    assert_eq!(
        (lists, reads),
        (1, 2),
        "重复展开没有多做任何文件动作：只有一次 list、两次 read（各自被模型显式选择）"
    );
}

/// 7. 展开状态属于一次活动的使用作用域：新 Run 从未展开开始，别的 Agent 不共享。
#[test]
fn expansion_lives_in_one_run_scope_and_never_leaks() {
    let sandbox = Sandbox::new("scope");
    sandbox.write("alpha.txt", "scope-marker");
    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");

    let agent_a = AgentId::new("agent-a");
    let agent_b = AgentId::new("agent-b");
    let run_1 = RunId::new("run-1");
    let run_2 = RunId::new("run-2");

    // A 在 run-1 的作用域里展开；作用域销毁之后不留任何历史记录。
    {
        let mut expansion3 = RunExpansion::new(agent_a.clone(), run_1.clone());
        let mut scope = os.scope(&mut expansion3);
        assert_eq!(
            scope
                .execute(&agent_a, &operation("run-1"), &expand(FILE_APP_ID))
                .state(),
            ResultState::Success
        );
        // 入口 4 项（展开 / 看详情 / 钉 / 摘）+ 展开后文件视图自己的 3 项只读动作（`files_list` / `files_read` / `files_grep`）。
        assert_eq!(scope.capabilities(&agent_a, &run_1).len(), 8);

        // 同一个作用域里，别的 Agent / 别的 Run 都不共享这份展开状态。
        assert_eq!(
            scope.capabilities(&agent_b, &run_1).len(),
            0,
            "身份不匹配时能力不泄露"
        );
        assert_eq!(
            scope
                .execute(&agent_b, &operation("run-1"), &list())
                .failure_class(),
            Some(ExpansionFailure::IdentityMismatch.as_str()),
            "别的 Agent 不能借着这个作用域使用文件能力"
        );
        assert_eq!(
            scope
                .execute(&agent_a, &operation("run-2"), &list())
                .failure_class(),
            Some(ExpansionFailure::IdentityMismatch.as_str()),
            "OperationId 里的 Run 与作用域不一致时 fail-closed"
        );
        assert_eq!(
            scope
                .execute(&agent_a, &operation("run-1"), &read("alpha.txt"))
                .state(),
            ResultState::Success,
            "正确绑定的读取必须成功（反向用例）"
        );
    }

    // 新的作用域（同一个 OS、同一个 Agent）：从未展开开始。
    let mut expansion4 = RunExpansion::new(agent_a.clone(), run_2.clone());
    let mut fresh = os.scope(&mut expansion4);
    assert_eq!(
        fresh.capabilities(&agent_a, &run_2).len(),
        5,
        "上一次活动的展开状态不得被继承"
    );
    assert_eq!(
        fresh
            .execute(&agent_a, &operation("run-2"), &list())
            .failure_class(),
        Some(ExpansionFailure::NotExpanded.as_str())
    );
    assert_eq!(
        fresh.capabilities(&agent_a, &run_2).len(),
        5,
        "被拒绝的调用不得改变这个作用域的能力集合"
    );

    // 轨迹隔离：两次 Run 各自的经历互不混入，事实里也没有正文。
    let mut first = ScriptedModel {
        script: vec![
            ModelOutput::act(expand(FILE_APP_ID)),
            // TASK_69：读之前先看格式——没查过的动作在准入面就被拒（"先看格式"，不是"你没有这个能力"）。
            ModelOutput::act(inspect(READ)),
            ModelOutput::act(read("alpha.txt")),
            ModelOutput::finish(SubjectEnding::Completed),
        ],
    };
    let mut agent_state = Agent::new(agent_a.clone());
    let mut ledger = Ledger::new(agent_a.clone());
    let mut expansion5 = RunExpansion::new(agent_a.clone(), run_1.clone());
    let mut first_scope = os.scope(&mut expansion5);
    let first_run = run_activity(
        &mut agent_state,
        run_1.clone(),
        Limits::new(4),
        &mut first,
        &mut first_scope,
        &mut ledger,
    )
    .expect("第一次活动");

    let mut second = ScriptedModel {
        script: vec![
            ModelOutput::act(list()),
            ModelOutput::finish(SubjectEnding::Silent),
        ],
    };
    let mut other_agent = Agent::new(agent_b.clone());
    let mut other_ledger = Ledger::new(agent_b.clone());
    let mut expansion6 = RunExpansion::new(agent_b.clone(), run_2.clone());
    let mut second_scope = os.scope(&mut expansion6);
    let second_run = run_activity(
        &mut other_agent,
        run_2.clone(),
        Limits::new(4),
        &mut second,
        &mut second_scope,
        &mut other_ledger,
    )
    .expect("第二次活动");

    // TASK_69：**只看真实业务动作**（展开 + 读取）——"先看格式"那一轮不计入业务步骤。
    let first_business = first_run
        .observed()
        .iter()
        .filter(|observation| observation.action().as_str() != "inspect_action")
        .count();
    assert_eq!(first_business, 2, "展开 + 读取");
    // 未展开的请求**没有被接纳**：它不再被记为"已派发并失败"，而是根本不产生行动结果，
    // 也不留下任何行动事实。它在轨迹里留一条可见的拒绝经历，让主体自己判断下一步。
    assert!(
        second_run.observed().is_empty(),
        "没有被接纳的请求不该出现在行动结果里"
    );
    let refusals: Vec<(&str, &str)> = second_run
        .trace()
        .iter()
        .filter_map(|entry| match entry {
            TraceEntry::Refused { action, class, .. } => Some((action.as_str(), class.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        refusals,
        vec![("files_list", ExpansionFailure::NotExpanded.as_str())],
        "未展开的请求必须留下一条可见的拒绝经历"
    );
    let second_facts = format!("{:?}", other_ledger.records());
    for fabricated in ["ActionPrepared", "ActionDispatched", "ActionResultRecorded"] {
        assert!(
            !second_facts.contains(fabricated),
            "被拒绝的请求不得产生 {fabricated}: {second_facts}"
        );
    }
    assert!(
        !format!("{:?}", other_ledger.records()).contains("scope-marker"),
        "B 的事实里不含 A 读到的正文"
    );
}

/// 生命周期：复用同一个 OS 连续跑多次活动，每次开始都只有入口能力。
///
/// 结构证据：`MinimalOs` 只有 `app` 一个字段，**没有可累计的集合**；行为证据见下。
#[test]
fn reusing_one_os_always_starts_unexpanded() {
    let sandbox = Sandbox::new("reuse");
    sandbox.write("alpha.txt", "reuse-marker");
    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");
    let agent = AgentId::new("agent-reuse");

    for index in 0..3 {
        let run_id = RunId::new(format!("run-{index}"));
        let mut model = ScriptedModel {
            script: vec![
                ModelOutput::act(expand(FILE_APP_ID)),
                // TASK_69：读之前先看格式（没查过的动作在准入面就被拒）。
                ModelOutput::act(inspect(READ)),
                ModelOutput::act(read("alpha.txt")),
                ModelOutput::finish(SubjectEnding::Completed),
            ],
        };
        let mut agent_state = Agent::new(agent.clone());
        let mut ledger = Ledger::new(agent.clone());
        let mut expansion7 = RunExpansion::new(agent.clone(), run_id.clone());
        let mut scope = os.scope(&mut expansion7);

        // 每次开始：只有入口能力。
        assert_eq!(
            scope.capabilities(&agent, &run_id).len(),
            5,
            "第 {index} 次活动开始时必须只有 OS 自己的五项能力（展开 / 看详情 / 钉 / 摘 / 写自识）"
        );

        let run = run_activity(
            &mut agent_state,
            run_id.clone(),
            Limits::new(4),
            &mut model,
            &mut scope,
            &mut ledger,
        )
        .expect("活动可以跑完");

        assert_eq!(
            run.outcome(),
            Some(Outcome::Subject(SubjectEnding::Completed))
        );
        // TASK_69：**只看真实业务动作**——展开 + 读取；"先看格式"那一轮不计入业务步骤。
        let business = run
            .observed()
            .iter()
            .filter(|observation| observation.action().as_str() != "inspect_action")
            .count();
        assert_eq!(business, 2, "展开 + 读取");
        assert!(
            !format!("{:?}", ledger.records()).contains("reuse-marker"),
            "事实里不含正文"
        );
    }
}

/// 生命周期：正常 Finish、ModelError、Turn 预算耗尽之后，下一次活动都从未展开开始。
#[test]
fn every_existing_failure_path_still_starts_the_next_run_unexpanded() {
    let sandbox = Sandbox::new("after-failure");
    sandbox.write("alpha.txt", "after-failure-marker");
    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");
    let agent = AgentId::new("agent-failure-paths");

    // (a) 展开之后 ModelError。
    {
        let mut model = ScriptedModel {
            script: vec![
                ModelOutput::act(expand(FILE_APP_ID)),
                ModelOutput::finish(SubjectEnding::Silent),
            ],
        };
        let mut failing = FailingAfterExpansion { inner: &mut model };
        let mut agent_state = Agent::new(agent.clone());
        let mut ledger = Ledger::new(agent.clone());
        let mut expansion8 = RunExpansion::new(agent.clone(), RunId::new("run-model-error"));
        let mut scope = os.scope(&mut expansion8);
        let run = run_activity(
            &mut agent_state,
            RunId::new("run-model-error"),
            Limits::new(4),
            &mut failing,
            &mut scope,
            &mut ledger,
        )
        .expect("活动可以跑完");
        assert_eq!(
            run.outcome(),
            Some(Outcome::System(SystemEnding::ModelFailed(Some(
                SCRIPTED_MODEL_FAILURE.as_str()
            ))))
        );
        assert_eq!(agent_state.activity(), None, "活动槽必须交还");
    }

    // (b) 展开之后 Turn 预算耗尽。
    {
        let mut model = ScriptedModel {
            script: vec![
                ModelOutput::act(expand(FILE_APP_ID)),
                ModelOutput::act(read("alpha.txt")),
                ModelOutput::act(read("alpha.txt")),
            ],
        };
        let mut agent_state = Agent::new(agent.clone());
        let mut ledger = Ledger::new(agent.clone());
        let mut expansion9 = RunExpansion::new(agent.clone(), RunId::new("run-timeout"));
        let mut scope = os.scope(&mut expansion9);
        let run = run_activity(
            &mut agent_state,
            RunId::new("run-timeout"),
            Limits::new(2),
            &mut model,
            &mut scope,
            &mut ledger,
        )
        .expect("活动可以跑完");
        assert_eq!(run.outcome(), Some(Outcome::System(SystemEnding::Timeout)));
        assert_eq!(agent_state.activity(), None, "活动槽必须交还");
    }

    // 两次之后：新的作用域仍然只有入口能力。
    let run_id = RunId::new("run-next");
    let mut expansion10 = RunExpansion::new(agent.clone(), run_id.clone());
    let mut scope = os.scope(&mut expansion10);
    assert_eq!(scope.capabilities(&agent, &run_id).len(), 5);
    assert_eq!(
        scope
            .execute(&agent, &operation("run-next"), &list())
            .failure_class(),
        Some(ExpansionFailure::NotExpanded.as_str())
    );
}

/// 让第二次调用直接失败（ModelError）的包装器：第一次照常委托。
struct FailingAfterExpansion<'a> {
    inner: &'a mut ScriptedModel,
}

impl ModelPort for FailingAfterExpansion<'_> {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        if view.observed().is_empty() {
            self.inner.complete(view)
        } else {
            Err(scripted_model_failure())
        }
    }
}

/// 8. App 失败之后活动继续；事实里没有正文；活动槽正常交还。
#[test]
fn an_app_failure_continues_and_facts_keep_no_bodies() {
    let sandbox = Sandbox::new("failure");
    sandbox.write("ok.txt", "fine");

    let mut model = ScriptedModel {
        script: vec![
            ModelOutput::act(expand(FILE_APP_ID)),
            // TASK_69：读之前先看格式——否则两次读都在准入面被拒，就到不了 App 自己的失败。
            ModelOutput::act(inspect(READ)),
            ModelOutput::act(read("missing.txt")),
            ModelOutput::act(read("ok.txt")),
            ModelOutput::finish(SubjectEnding::Completed),
        ],
    };
    let agent = AgentId::new("agent-a");
    let mut agent_state = Agent::new(agent.clone());
    let mut ledger = Ledger::new(agent.clone());
    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");
    let mut expansion11 = RunExpansion::new(agent.clone(), RunId::new("run-1"));
    let mut scope = os.scope(&mut expansion11);
    let run = run_activity(
        &mut agent_state,
        RunId::new("run-1"),
        Limits::new(6),
        &mut model,
        &mut scope,
        &mut ledger,
    )
    .expect("装配并跑完");

    // TASK_69：**只看真实业务动作**的结果——"先看格式"那一轮混进来会让这份序列错位。
    let states: Vec<ResultState> = run
        .observed()
        .iter()
        .filter(|observation| observation.action().as_str() != "inspect_action")
        .map(|observation| observation.state())
        .collect();
    assert_eq!(
        states,
        vec![
            ResultState::Success,
            ResultState::Failure,
            ResultState::Success
        ],
        "失败没有终结活动，模型自己继续"
    );
    assert_eq!(
        run.outcome(),
        Some(Outcome::Subject(SubjectEnding::Completed))
    );
    assert!(
        !format!("{:?}", ledger.records()).contains("fine"),
        "Foundation 的事实里不得出现文件正文"
    );
    assert_eq!(agent_state.activity(), None, "活动收场后活动槽必须交还");
    // 既有 Action Failure / Unknown / 恢复行为仍由 tests/minimal_agent.rs 覆盖。
}

/// 结构性证据：展开与重复展开都**不触碰文件系统**。
///
/// 证明方式：先把沙盒目录**删掉**（此后任何真实文件访问都不可能成功），再展开——它照样成功；
/// 同一个作用域里的读取则必然失败，反证展开真的没有读文件。这也同时给出重复展开零 I/O 的证据。
#[test]
fn expansion_and_re_expansion_do_not_touch_the_filesystem() {
    let sandbox = Sandbox::new("no-io");
    sandbox.write("alpha.txt", "marker");

    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");
    let agent = AgentId::new("agent-a");
    let run_id = RunId::new("run-1");

    // 删掉沙盒：此后任何文件访问都不可能成功（注意 Sandbox 的 Drop 会忽略这里的删除）。
    fs::remove_dir_all(sandbox.path()).expect("可以删掉临时沙盒");

    let mut expansion12 = RunExpansion::new(agent.clone(), run_id.clone());
    let mut scope = os.scope(&mut expansion12);
    let first = scope.execute(&agent, &operation("run-1"), &expand(FILE_APP_ID));
    assert_eq!(
        first.state(),
        ResultState::Success,
        "第一次展开不该触碰文件系统"
    );

    let second = scope.execute(&agent, &operation("run-1"), &expand(FILE_APP_ID));
    assert_eq!(
        second.state(),
        ResultState::Success,
        "重复展开同样不该触碰文件系统"
    );
    assert_eq!(
        second.content()[0].label(),
        "already-expanded",
        "重复展开只留一条明确痕迹"
    );

    // 反证：文件能力此时必然失败（沙盒已经不在了）。
    let read_after = scope.execute(&agent, &operation("run-1"), &read("alpha.txt"));
    assert_eq!(read_after.state(), ResultState::Failure);
    assert_eq!(read_after.failure_class(), Some("not-found"));
}

/// 作用域生命周期：复用同一个 OS，新的作用域从未展开开始（回收由作用域负责）。
#[test]
fn a_new_scope_never_inherits_a_previous_expansion() {
    let sandbox = Sandbox::new("scope-lifetime");
    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");
    let agent = AgentId::new("agent-a");

    {
        let run_id = RunId::new("run-1");
        let mut expansion13 = RunExpansion::new(agent.clone(), run_id.clone());
        let mut scope = os.scope(&mut expansion13);
        assert_eq!(scope.capabilities(&agent, &run_id).len(), 5);
        assert_eq!(
            scope
                .execute(&agent, &operation("run-1"), &expand(FILE_APP_ID))
                .state(),
            ResultState::Success
        );
        // 入口 4 项（展开 / 看详情 / 钉 / 摘）+ 展开后文件视图自己的 3 项只读动作（`files_list` / `files_read` / `files_grep`）。
        assert_eq!(scope.capabilities(&agent, &run_id).len(), 8);
    }

    // 前一个作用域已销毁：同一个 OS 上的新作用域没有继承任何展开状态。
    let run_id = RunId::new("run-2");
    let mut expansion14 = RunExpansion::new(agent.clone(), run_id.clone());
    let mut scope = os.scope(&mut expansion14);
    assert_eq!(scope.capabilities(&agent, &run_id).len(), 5);
    assert_eq!(
        scope
            .execute(&agent, &operation("run-2"), &list())
            .failure_class(),
        Some(ExpansionFailure::NotExpanded.as_str())
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

/// TASK_39：`inspect_action` 的读数——卡片自洽、**不在场**的动作明确拒绝、展开之后才看得到。
///
/// 看详情的范围恰好是"主体此刻真能调的**那些**动作"：没展开的 App 的动作、以及根本不存在的名字，
/// 都归同一个稳定分类（`invalid-argument`）——fail-closed，不猜近似名、也不透传。
#[test]
fn inspect_action_hands_back_a_card_and_refuses_what_is_not_in_play() {
    let sandbox = Sandbox::new("inspect-card");
    sandbox.write("secret.txt", "内容");
    let agent = AgentId::new("agent-a");
    let run_id = RunId::new("run-card");
    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");
    let mut expansion = RunExpansion::new(agent.clone(), run_id.clone());
    let mut scope = os.scope(&mut expansion);
    let op = operation("run-card");

    // (1) 入口自己那两项：卡片自洽（骨架 + 详情 + 参数表）。
    let card = scope.execute(&agent, &op, &inspect(EXPAND));
    assert_eq!(card.state(), ResultState::Success);
    let text = card.content()[0].text().to_string();
    assert!(text.contains("open_app: "), "卡片带骨架: {text}");
    assert!(text.contains("params:"), "卡片带参数表: {text}");
    assert!(text.contains(FILE_APP_ID), "详情里写着装了什么: {text}");

    // (2) 没展开的 App 的动作：不在**当前在场**的表里 ⇒ 明确拒绝，**并且指路**。
    //
    // ⚠ **2026-10-14 补**（真机现场抓到的）：分类一个字不改，但回执从前是**空的** —— 读起来像
    // "你参数写错了"，而真相是"它在一扇门后面"。主体后来自己改用 `open_app` 绕过去了，**那是运气**；
    // 读数要能直接告诉主体：这个名字属于哪个 app、那个 app 还没展开、先开它。
    let not_yet = scope.execute(&agent, &op, &inspect(LIST));
    assert_eq!(not_yet.state(), ResultState::Failure);
    assert_eq!(not_yet.failure_class(), Some("invalid-argument"));
    let door = not_yet
        .content()
        .iter()
        .map(|block| block.text())
        .collect::<String>();
    assert!(door.contains(FILE_APP_ID), "要说清它属于哪个 app: {door}");
    assert!(door.contains("还没展开"), "要说清那个 app 还没展开: {door}");
    assert!(
        door.contains("open_app"),
        "要指路（先开那扇门），而不是让主体自己去猜: {door}"
    );

    // (3) 根本不存在的名字：**同一个分类**（不猜近似名），但读数**不说**"某扇门后面"
    //     —— OS 手里没有那个事实，就不编一个（"没有"与"读不到"分开表达）。
    let unknown = scope.execute(&agent, &op, &inspect("no_such_action"));
    assert_eq!(unknown.failure_class(), Some("invalid-argument"));
    let reading = unknown
        .content()
        .iter()
        .map(|block| block.text())
        .collect::<String>();
    assert!(
        reading.contains("没有哪个已装配的 app 声明过它"),
        "不存在的名字照实说: {reading}"
    );

    // (4) 展开之后，同一个动作看得到了；卡片拿的是**同一份声明**（骨架 + 详情）。
    assert_eq!(
        scope.execute(&agent, &op, &expand(FILE_APP_ID)).state(),
        ResultState::Success
    );
    let card = scope.execute(&agent, &op, &inspect(LIST));
    assert_eq!(card.state(), ResultState::Success, "展开之后它在场");
    let text = card.content()[0].text().to_string();
    assert!(text.contains(&format!("{LIST}: ")), "卡片带骨架: {text}");
    assert!(text.contains("after"), "卡片带详情: {text}");
    assert!(text.contains("params:"), "卡片带参数表: {text}");
}

/// **那一格终于送到了主体的视野**（2026-10-08，见 `agent/README.md` §3.45）。
///
/// 每个 App 都必须为它的每个能力回答"回执从哪个口子进来"（装配期漏填即拒，
/// `CapabilityWithoutSource`），而那个答案此前**从不送给主体**——`SourceClass` 自己的文档写着
/// 「它不承载"可不可信"……**判断归主体**」，于是**判断缺了它的输入**。
/// 现在它落在详情卡那一行 `receipt-source`（按需取，不占首屏、不占每轮预算）。
#[test]
fn the_detail_card_carries_the_receipt_source() {
    let sandbox = Sandbox::new("card-source");
    sandbox.write("secret.txt", "内容");
    let agent = AgentId::new("agent-a");
    let run_id = RunId::new("run-card-source");
    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");
    let mut expansion = RunExpansion::new(agent.clone(), run_id.clone());
    let mut scope = os.scope(&mut expansion);
    let op = operation("run-card-source");

    // (1) OS 自有动词：回执是**系统**渲染的读数（由 `declare_os_owned` 登记，不是任何 App 声明的）。
    let card = scope.execute(&agent, &op, &inspect(EXPAND));
    assert_eq!(card.state(), ResultState::Success);
    let text = card.content()[0].text().to_string();
    assert!(
        text.contains("receipt-source: system"),
        "OS 自有动词的卡要写明回执来自系统: {text}"
    );

    // (2) 一个**真实 App** 的读动词：回执是**环境根里的材料**（`file_view` 自己那一份声明）。
    assert_eq!(
        scope.execute(&agent, &op, &expand(FILE_APP_ID)).state(),
        ResultState::Success
    );
    let card = scope.execute(&agent, &op, &inspect(LIST));
    assert_eq!(card.state(), ResultState::Success);
    let text = card.content()[0].text().to_string();
    assert!(
        text.contains("receipt-source: environment"),
        "文件 App 的读动词，回执来自环境根: {text}"
    );

    // (3) 那一行**与参数表分开**，而且是稳定标记（不是散文）——照它就能机械地对上词汇表。
    assert!(text.contains("params:"), "参数表照旧在: {text}");
    let line = text
        .lines()
        .find(|line| line.trim_start().starts_with("receipt-source:"))
        .expect("卡片必须有那一行");
    assert_eq!(line.trim(), "receipt-source: environment");
}

/// `pin_action` / `unpin_action` 的载荷（测试按名字引用 OS 的动作）。
fn pin(action: &str) -> Action {
    Action::with_params(
        ActionId::new("pin_action"),
        ActionParams::none().with("action", action),
    )
}

fn unpin(action: &str) -> Action {
    Action::with_params(
        ActionId::new("unpin_action"),
        ActionParams::none().with("action", action),
    )
}

/// TASK_68 P3：**钉住一个动作 ⇒ 自持物里记下来**（动作级的行为判据）。
///
/// "只在 Run 边界生效"的正面形状：这一次的 pin 写进自己的地盘，下一次装配读回来。
/// ⚠️ 这条判据也钉住了**环境根的出处**：`MinimalOs::open` 本身不知道 Agent 的环境根，
/// 要用 `with_environment_root` 显式声明（真实装配路径就是这么做的）；没声明时**明确拒绝**，
/// 不静默写到别处。
#[test]
fn pinning_an_action_is_recorded_in_her_own_place() {
    let sandbox = Sandbox::new("pin");
    let agent = AgentId::new("pin-agent");
    let run_id = RunId::new("run-pin");

    // ① 真实装配：显式声明环境根（自持物的去向）。
    let mut os = MinimalOs::open(sandbox.path())
        .expect("可以装配")
        .with_environment_root(sandbox.path());
    let mut expansion = RunExpansion::new(agent.clone(), run_id.clone());
    let mut scope = os.scope(&mut expansion);

    // 钉一个**常驻在场**的动作（`open_app` 不依赖展开）。
    let pinned = scope.execute(&agent, &operation("run-pin"), &pin(EXPAND));
    assert_eq!(pinned.state(), ResultState::Success, "{pinned:?}");

    // ② 自持物里真的有它；回执说清**生效时机**（下一次开场，不是这一次）。
    assert_eq!(
        celira_agent::pinned_actions::pinned_actions(sandbox.path()).expect("可以读"),
        vec![EXPAND.to_string()]
    );
    let text = pinned.content()[0].text().to_string();
    assert!(
        text.contains("下一次开场生效"),
        "回执要说清生效时机: {text}"
    );
    // 重复钉是明确的成功结果（不报错），且**不重复写**。
    let again = scope.execute(&agent, &operation("run-pin"), &pin(EXPAND));
    assert_eq!(again.state(), ResultState::Success, "{again:?}");
    assert_eq!(
        celira_agent::pinned_actions::pinned_actions(sandbox.path())
            .expect("可以读")
            .len(),
        1,
        "重复钉不重复写"
    );

    // ③ 摘掉 ⇒ 自持物里没有它（出厂格位是常量，不受摘除影响——回执里说清了）。
    let removed = scope.execute(&agent, &operation("run-pin"), &unpin(EXPAND));
    assert_eq!(removed.state(), ResultState::Success, "{removed:?}");
    assert!(
        celira_agent::pinned_actions::pinned_actions(sandbox.path())
            .expect("可以读")
            .is_empty()
    );

    // ④ **不在场的名字钉不上**（凭空钉不出来）：与看详情同款，明确拒绝。
    let ghost = scope.execute(&agent, &operation("run-pin"), &pin("notes_list"));
    assert_eq!(ghost.failure_class(), Some("invalid-argument"));

    // ⑤ **没有环境根的装配** ⇒ 明确拒绝（fail-closed），不静默写到别处。
    let mut bare = MinimalOs::open(sandbox.path()).expect("可以装配");
    let mut expansion2 = RunExpansion::new(agent.clone(), run_id.clone());
    let mut bare_scope = bare.scope(&mut expansion2);
    let refused = bare_scope.execute(&agent, &operation("run-pin"), &pin(EXPAND));
    assert_eq!(refused.failure_class(), Some("no-environment-root"));

    // ⑥ **准入面**（`admit`，不是直接 `execute`）：这两个动词必须被**认领**并放行。
    //
    // ⚠️ 这一格是判错即红逼出来的：注入"准入面不再认领"时，上面所有格**一条都没红**——
    // 因为它们都走 `execute`，**跳过 `admit`**。一个动作以自有身份活着，要在装配、**准入**、
    // 执行、声明四个面上同时被认出来；只有这里才看得见准入面。
    assert_eq!(
        scope.admit(&agent, &run_id, &pin(EXPAND)),
        ActionAdmission::Admitted,
        "钉 / 摘是 OS 自己的动词：准入面必须认领（不认领就会被当成未知动作拒掉）"
    );
    assert_eq!(
        scope.admit(&agent, &run_id, &unpin(EXPAND)),
        ActionAdmission::Admitted
    );
    // **对照**：动作标识本身不在归属索引里的东西，在同一面上被拒 —— 证明这一面真的在判。
    //
    // ⚠️ 顺便钉住两个面的**分工**：准入面判的是**动作标识**（`pin_action` 是 OS 自有的 ⇒ 放行），
    // 而参数值（"要钉哪个动作"）的合法性归**执行面**（上面 ④ 那格 `invalid-argument`）。
    // 一开始我把对照写成"钉一个不存在的动作名会被准入拒"，实测是 `Admitted`——那不是 bug，
    // 是准入面根本不看参数值。
    assert_eq!(
        scope.admit(
            &agent,
            &run_id,
            &Action::with_params(ActionId::new("no_such_action"), ActionParams::none())
        ),
        ActionAdmission::refused("unknown-action")
    );

    let _ = fs::remove_dir_all(sandbox.path());
}

/// TASK_71 桌面 P0 第 2 件：**自己那一份上限 7** —— 超限明确拒绝、**不替主体淘汰**、
/// 摘一个之后又能钉。
///
/// ⚠️ 上限只数**自己钉的那一份**：出厂格位（`FACTORY_DEFAULT`）不落盘、也摘不掉 ⇒ 若把它算进
/// 上限，主体会腾不出位置（自锁）。判据落在**自持物文件的内容**上（"前 7 个仍在"只有读盘才算数），
/// 而不是回执文案上。
#[test]
fn the_pin_limit_refuses_the_eighth_and_evicts_nothing_on_its_own() {
    let sandbox = Sandbox::new("pin-limit");
    let agent = AgentId::new("pin-limit-agent");
    let run_id = RunId::new("run-pin-limit");
    let mut os = MinimalOs::open(sandbox.path())
        .expect("可以装配")
        .with_environment_root(sandbox.path());
    let mut expansion = RunExpansion::new(agent.clone(), run_id.clone());
    let mut scope = os.scope(&mut expansion);
    let op = operation("run-pin-limit");

    // 上限是个**产品决定**：钉在常量上 —— 改数字要过这条断言，不会悄悄跟着实现变。
    assert_eq!(MAX_PINS, 7);

    // 展开材料窗 ⇒ 在场 5 + 3 = 8 个动作（刚好够试出第 8 个）。
    assert_eq!(
        scope.execute(&agent, &op, &expand(FILE_APP_ID)).state(),
        ResultState::Success
    );

    let candidates = [
        EXPAND,
        "inspect_action",
        "pin_action",
        "unpin_action",
        "write_self_concept",
        "files_list",
        "files_read",
        "files_grep",
    ];
    for name in &candidates[..MAX_PINS] {
        let pinned = scope.execute(&agent, &op, &pin(name));
        assert_eq!(
            pinned.state(),
            ResultState::Success,
            "钉 `{name}`: {pinned:?}"
        );
    }
    let full = celira_agent::pinned_actions::pinned_actions(sandbox.path()).expect("可以读");
    assert_eq!(full.len(), MAX_PINS, "{full:?}");

    // 第 8 个：**明确拒绝**（稳定分类），而且**一个字节都没动** —— 不淘汰、不半写。
    let eighth = candidates[MAX_PINS];
    let refused = scope.execute(&agent, &op, &pin(eighth));
    assert_eq!(refused.state(), ResultState::Failure, "{refused:?}");
    assert_eq!(refused.failure_class(), Some("pinned-limit-reached"));
    assert_eq!(
        celira_agent::pinned_actions::pinned_actions(sandbox.path()).expect("可以读"),
        full,
        "超限不许替主体挑一个扔出去（淘汰是主体的决策），也不许半写"
    );

    // 重复钉**不增加条数** ⇒ 满了之后重复钉照旧是明确的成功（上限不是"什么都不许做"）。
    assert_eq!(
        scope.execute(&agent, &op, &pin(candidates[0])).state(),
        ResultState::Success
    );

    // **出路是"先摘一个"**：摘掉一个 ⇒ 再钉第 8 个就成了。
    assert_eq!(
        scope.execute(&agent, &op, &unpin(candidates[0])).state(),
        ResultState::Success
    );
    assert_eq!(
        scope.execute(&agent, &op, &pin(eighth)).state(),
        ResultState::Success
    );
    let after = celira_agent::pinned_actions::pinned_actions(sandbox.path()).expect("可以读");
    assert_eq!(after.len(), MAX_PINS, "{after:?}");
    assert!(
        after.contains(&eighth.to_string()) && !after.contains(&candidates[0].to_string()),
        "摘掉的那个换成新钉的，其余一个不动: {after:?}"
    );

    // 上限的**规则与出路**写在 `pin_action` 的详情卡里：失败回执只带一个**分类**（Core 的
    // `ActionOutcome::failure` 不带正文）⇒ 那句"已满 7，先摘一个"必须在**主体问得出来**的地方。
    let card = scope.execute(&agent, &op, &inspect("pin_action"));
    let text = card.content()[0].text().to_string();
    assert!(
        text.contains(&format!("上限 {MAX_PINS} 项"))
            && text.contains("pinned-limit-reached")
            && text.contains("unpin_action"),
        "详情卡要说清：上限几项、被拒时看到什么分类、出路是什么: {text}"
    );

    let _ = fs::remove_dir_all(sandbox.path());
}

/// `notes_create` / `notes_read` 的载荷（只为过**准入面**；参数值归执行面）。
fn create_note() -> Action {
    Action::with_params(
        ActionId::new("notes_create"),
        ActionParams::none().with("title", "t").with("text", "b"),
    )
}

fn read_note() -> Action {
    Action::with_params(
        ActionId::new("notes_read"),
        ActionParams::none().with("id", "note-1"),
    )
}

/// 一次拒绝的稳定分类（用例自己知道前提：这一次确实被拒）。
fn refusal_class(admission: &ActionAdmission) -> &str {
    match admission {
        ActionAdmission::Refused { class, .. } => class,
        ActionAdmission::Admitted => panic!("这一次请求被接纳了，这条用例的前提不成立"),
    }
}

/// TASK_71 桌面 P0 第 3 件（另一半）：出厂精选缩到 4 之后，**没被精选的动词回到既有的
/// `inspect_action` 路径**（查一次、这一程之内免查）——**门语义一个字没动**。
///
/// **TASK_72 升级**：那条路径的**交付时机**提前了一轮——拒绝**随单附卡**（卡＝`inspect_action`
/// 交回的同一张），因此"被拒"之后**同一份载荷直接可调**，不必再跑一趟取卡。
///
/// ⚠️ **2026-10-13 收窄**：那一关问的是**这个动作会不会改外面的世界**（`EffectClass`），
/// 因此它今天只挡**写**动词；**只读**动词第一次调用直接执行（真机读数：125 次首调税里 83 次
/// 落在只读动作上）。⇒ 本用例测的是**写**那一侧，只读那一侧的对照断言在中间那一段。
///
/// ⚠️ 判据落在**准入面**（`admit`），不是 `execute`：门装在那里，`execute` 会跳过它。
/// "进不进得了场"（展开）与"免不免查"（精选 / 钉住 / 查过）是两件事，这条只判后者。
#[test]
fn every_writing_verb_goes_back_to_the_inspect_first_path_until_she_pins_it() {
    let sandbox = Sandbox::new("factory-selection");
    let agent = AgentId::new("factory-selection-agent");
    let run_id = RunId::new("run-factory-selection");
    // 只装 notes：它的 7 条动词现在**一条都不开箱免查**（2026-10-10：出厂精选已删——
    // 常驻只能是主体自己 pin 的）。
    let notes = NotesApp::open(sandbox.path()).expect("Notes App");
    let mut os = MinimalOs::assemble(vec![Box::new(notes)])
        .expect("可以装配")
        .with_environment_root(sandbox.path());
    let mut expansion = RunExpansion::new(agent.clone(), run_id.clone());
    let mut scope = os.scope(&mut expansion);
    let op = operation("run-factory-selection");

    // 展开 ⇒ 它的 7 条动词进场。
    assert_eq!(
        scope.execute(&agent, &op, &expand(NOTES_APP_ID)).state(),
        ResultState::Success
    );

    // ⚠ **没有任何动作开箱免查**（这一条就是那一刀的行为证明）：`notes_create` 从前靠
    // "出厂精选"直接可调，现在**也要先看格式**——而拒绝的语义仍然是"先看格式"，
    // 不是"你没这个能力"（它的动词上面刚展开过）。
    let refused = scope.admit(&agent, &run_id, &create_note());
    assert_eq!(
        refusal_class(&refused),
        "needs-inspection",
        "常驻只能是主体自己 pin 的 ⇒ 没钉过的都要先看格式"
    );

    // **只读动词**（`notes_read`，没有效果声明）今天**不收这笔税**（2026-10-13）：那一关问的是
    // "会不会改外面的世界"，读一个字节都改不了 ⇒ **直接放行**。对照的是上一段那条写动词。
    assert_eq!(
        scope.admit(&agent, &run_id, &read_note()),
        ActionAdmission::Admitted,
        "只读动词第一次调用直接执行（形状按需，不挡执行）"
    );

    // 门本身还在：**另有一条**没钉过、也没撞过门的**写**动词（`notes_pin`），它照旧走"先看格式"
    // ——而且拒绝照旧**随单附卡**（TASK_72 的交付时机没动）。
    let fresh = scope.admit(&agent, &run_id, &Action::new(ActionId::new("notes_pin")));
    assert_eq!(
        refusal_class(&fresh),
        "needs-inspection",
        "没撞过门的**写**动词照旧先看格式"
    );
    assert_eq!(fresh.content().len(), 1, "它也一样随单附卡");
    assert_eq!(fresh.content()[0].label(), "notes_pin");

    // 主体**自己主动查**那条路照旧：查一次 ⇒ 这一程之内直接可调（既有免查语义，没动）。
    // 用第三条**写**动词验（`notes_delete`）——因为只读动词本来就免这笔税，用它验不出"查过就放行"。
    assert_eq!(
        scope.execute(&agent, &op, &inspect("notes_delete")).state(),
        ResultState::Success
    );
    assert_eq!(
        scope.admit(&agent, &run_id, &Action::new(ActionId::new("notes_delete"))),
        ActionAdmission::Admitted,
        "查过一次就放行（免查 ⇒ 免学的既有语义）"
    );

    let _ = fs::remove_dir_all(sandbox.path());
}

// ---------------------------------------------------------------------------
// TASK_76（A）：self_concept 账单 ＋ 上限——拒绝在**写入杠杆**处，不在注入闸兜底
// ---------------------------------------------------------------------------

/// 一个 `write_self_concept` 动作（OS 自有动词；参数名就是主体看到的那个 `text`）。
fn self_concept_write(text: &str) -> Action {
    Action::with_params(
        ActionId::new("write_self_concept"),
        ActionParams::none().with("text", text),
    )
}

/// **账单贴在杠杆处、上限是硬墙**：写得下就带回执账单；写不下就**明确拒绝**，
/// 旧的一份**逐字节原样保留**（不截断、不落盘）。
///
/// 判错即红：上限只写在文档里而不在代码里 ⇒ 超限那条会写进去（文件变了）；拒绝不带读数 ⇒ 红；
/// 空正文那条既有拒绝被顺手改掉 ⇒ 红。
#[test]
fn the_self_concept_bill_rides_on_the_write_lever_and_the_limit_is_a_hard_wall() {
    let sandbox = Sandbox::new("self_concept-bill");
    let agent = AgentId::new("self_concept-agent");
    let run_id = RunId::new("run-self_concept-bill");
    let mut os = MinimalOs::open(sandbox.path())
        .expect("可以装配")
        .with_environment_root(sandbox.path());
    let mut expansion = RunExpansion::new(agent.clone(), run_id.clone());
    let mut scope = os.scope(&mut expansion);
    let op = operation("run-self_concept-bill");

    // 上限是个**产品决定**：钉在常量上——改数字要过这条断言，不会悄悄跟着实现变。
    assert_eq!(celira_agent::self_concept::MAX_SELF_CONCEPT_BYTES, 4 * 1024);

    // ① 写得下 ⇒ 回执带**账单**（现在多大 / 上限多大），而且只在回执里。
    let text = "我是阿默：先看事实，再说话。\n";
    let written = scope.execute(&agent, &op, &self_concept_write(text));
    assert_eq!(written.state(), ResultState::Success, "{written:?}");
    let receipt: String = written
        .content()
        .iter()
        .map(|block| block.text().to_string())
        .collect();
    assert!(
        receipt.contains(&format!("self_concept = {} B", text.len())),
        "账单要给现值: {receipt}"
    );
    assert!(
        receipt.contains(&format!(
            "上限 {} B",
            celira_agent::self_concept::MAX_SELF_CONCEPT_BYTES
        )),
        "账单要给上限: {receipt}"
    );

    // ② 账单**不落盘**：盘上逐字节就是主体写的那一份（开场注入因此一个字节都没变——
    //    "开场 = 文件原文"由 `status_bar.rs` 的 TASK_70 用例继续钉着）。
    assert_eq!(
        std::fs::read_to_string(sandbox.path().join("self_concept")).expect("读回自识"),
        text,
    );

    // ③ 超限 ⇒ **明确拒绝**（稳定分类），读数里三件事一次说清，旧的一份原样保留。
    let too_long = "长".repeat(celira_agent::self_concept::MAX_SELF_CONCEPT_BYTES); // 每字 3 字节 ⇒ 必超
    let refused = scope.execute(&agent, &op, &self_concept_write(&too_long));
    assert_eq!(refused.state(), ResultState::Failure, "{refused:?}");
    assert_eq!(refused.failure_class(), Some("self_concept-too-large"));
    let refusal: String = refused
        .content()
        .iter()
        .map(|block| block.text().to_string())
        .collect();
    assert!(
        refusal.contains(&format!("self_concept = {} B", too_long.len())),
        "拒绝要说清主体想写的多大: {refusal}"
    );
    assert!(
        refusal.contains(&format!(
            "上限 = {} B",
            celira_agent::self_concept::MAX_SELF_CONCEPT_BYTES
        )),
        "{refusal}"
    );
    assert!(refusal.contains("原样保留"), "{refusal}");
    assert_eq!(
        std::fs::read_to_string(sandbox.path().join("self_concept")).expect("读回自识"),
        text,
        "拒绝之后旧的那一份逐字节不变（不截断、不落盘）"
    );

    // ④ **既有的空正文拒绝**不受影响（仍是 `invalid-argument`，不是新分类）。
    let empty = scope.execute(&agent, &op, &self_concept_write("   \n"));
    assert_eq!(empty.state(), ResultState::Failure, "{empty:?}");
    assert_eq!(empty.failure_class(), Some("invalid-argument"));
}
