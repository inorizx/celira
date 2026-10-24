//! 中立初始输入的离线证明：输入随 Run 一次确定、每轮可见、进入真实请求，且不被继承、不写进事实。
//!
//! 全部离线：不联网、不需要密钥、只用自建临时沙盒与合成内容。

use std::fs;
use std::path::{Path, PathBuf};

use celira_agent::composition::{run_read_only_files, run_read_only_files_with_input};
use celira_agent::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Agent,
    Capabilities, Limits, MAX_INPUT_BYTES, ModelError, ModelOutput, ModelPort, RunInput,
    RunInputError, View, run_activity_with_input,
};
use celira_agent::file_app::FILE_APP_ID;
use celira_agent::foundation::{AgentId, Ledger, OperationId, ResultState, RunId, SubjectEnding};
use celira_agent::os::{MinimalOs, RunExpansion};

mod common;

use common::InspectsFirst;

/// 自建临时沙盒：只在系统临时目录下创建，用完自己删掉。
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let unique = format!(
            "celira-run-input-{}-{}-{}",
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

fn expand() -> Action {
    Action::with_params(
        ActionId::new("open_app"),
        ActionParams::none().with("target", FILE_APP_ID),
    )
}

fn listing_action() -> Action {
    Action::with_params(ActionId::new("files_list"), ActionParams::none())
}

fn read(relative: &str) -> Action {
    Action::with_params(
        ActionId::new("files_read"),
        ActionParams::none().with("path", relative),
    )
}

/// 会读视野的模型：展开 → 列目录 → 按**输入里点名的文件**读取 → 再次展开 → 结束。
///
/// 它不消费预排的完整动作序列：要读的名字来自 `view.input()`，而且只有在**实际列出的**条目里
/// 真出现过才会去读。同一份输入点不同的名，就会走到不同的文件。
struct InputDrivenModel {
    /// 每次调用看到的（输入原文, 能力数, 经历条数, 经历里的正文）。
    seen: Vec<(String, usize, usize, Vec<String>)>,
    picked: Option<String>,
    re_expanded: bool,
}

impl InputDrivenModel {
    fn new() -> Self {
        Self {
            seen: Vec::new(),
            picked: None,
            re_expanded: false,
        }
    }

    fn listed_names(view: &View) -> Vec<String> {
        view.observed()
            .iter()
            .rev()
            .find(|observation| observation.action().as_str() == "files_list")
            .map(|listing| {
                listing
                    .outcome()
                    .content()
                    .iter()
                    .flat_map(|block| block.text().lines())
                    .filter_map(|line| line.split_once('\t').map(|(name, _)| name.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl ModelPort for InputDrivenModel {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        self.seen.push((
            view.input().as_str().to_string(),
            view.capabilities().len(),
            view.observed().len(),
            view.observed()
                .iter()
                .flat_map(|observation| observation.outcome().content().iter())
                .map(|block| block.text().to_string())
                .collect(),
        ));

        // 依据**实际呈现的能力**决定先展开，而不是硬编码流程。
        if view
            .capabilities()
            .find(&ActionId::new("files_list"))
            .is_none()
        {
            return Ok(ModelOutput::act(expand()));
        }
        // 读之前必须真的看过目录。
        if !view
            .observed()
            .iter()
            .any(|observation| observation.action().as_str() == "files_list")
        {
            return Ok(ModelOutput::act(listing_action()));
        }

        let wanted = view.input().as_str().trim().to_string();
        if wanted.is_empty() {
            return Ok(ModelOutput::finish(SubjectEnding::Silent));
        }

        match self.picked.as_deref() {
            // 名字必须真的出现在列表里才读；否则这就是一次明确拒绝，而不是猜路径。
            None => {
                if !Self::listed_names(view).iter().any(|name| name == &wanted) {
                    return Ok(ModelOutput::finish(SubjectEnding::Declined));
                }
                self.picked = Some(wanted.clone());
                Ok(ModelOutput::act(read(&wanted)))
            }
            // 读过之后再展开一次，然后自己结束。
            Some(_) if !self.re_expanded => {
                self.re_expanded = true;
                Ok(ModelOutput::act(expand()))
            }
            Some(_) => Ok(ModelOutput::finish(SubjectEnding::Completed)),
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

/// 1. 空输入保留原有无任务行为；非空输入在每一轮原样可见、只有一份，重复展开也不消失。
#[test]
fn the_input_is_visible_in_every_turn_and_survives_re_expansion() {
    let sandbox = Sandbox::new("visible");
    sandbox.write("alpha.txt", "visible-marker-alpha");

    // 空输入：模型看不到任何初始内容，活动照常跑。
    let mut empty_model = ScriptedModel {
        script: vec![ModelOutput::finish(SubjectEnding::Silent)],
    };
    let empty = run_read_only_files(
        sandbox.path(),
        AgentId::new("agent-empty"),
        RunId::new("run-empty"),
        Limits::new(2),
        &mut empty_model,
    )
    .expect("空输入也应当能跑");
    assert!(empty.run.input().is_empty(), "空输入就是没有初始内容");
    assert_eq!(
        empty.outcome(),
        Some(celira_agent::foundation::Outcome::Subject(
            SubjectEnding::Silent
        ))
    );

    // 非空输入：每一轮都读到同一份原文，重复展开之后仍在。
    let input = "alpha.txt";
    let mut model = InspectsFirst::new(InputDrivenModel::new());
    let result = run_read_only_files_with_input(
        sandbox.path(),
        AgentId::new("agent-a"),
        RunId::new("run-1"),
        // TASK_69：预算是"承认先看格式要走一轮"，不是放宽校验——主体开场那两批查阅占了两轮。
        Limits::new(7),
        RunInput::text(input).expect("输入在上限内"),
        &mut model,
    )
    .expect("装配并跑完");

    assert_eq!(result.run.input().as_str(), input);
    assert!(
        model.inner().seen.len() >= 5,
        "展开→列目录→读取→再次展开→结束"
    );
    for (index, (seen_input, ..)) in model.inner().seen.iter().enumerate() {
        assert_eq!(
            seen_input,
            input,
            "第 {} 轮必须读到同一份初始输入（原样、只有一份）",
            index + 1
        );
    }
    // 再次展开发生在读取之后，而输入在那之后仍然可见（上面已逐轮断言）。
    assert!(
        result
            .run
            .observed()
            .iter()
            .filter(|observation| observation.action().as_str() == "open_app")
            .count()
            >= 2,
        "这次活动确实发生过重复展开"
    );

    // 事实里没有输入正文。
    assert!(
        !format!("{:?}", result.ledger.records()).contains(input),
        "初始输入不得进入 Foundation 的事实"
    );
}

/// 2. 两份不同输入导致读取不同的已知合法目标；读取前必须真的看过目录。
#[test]
fn two_different_inputs_lead_to_two_different_files() {
    let sandbox = Sandbox::new("two-inputs");
    sandbox.write("alpha.txt", "two-inputs-alpha");
    sandbox.write("beta.txt", "two-inputs-beta");

    let mut picks = Vec::new();
    for (run, input) in [("run-alpha", "alpha.txt"), ("run-beta", "beta.txt")] {
        let mut model = InspectsFirst::new(InputDrivenModel::new());
        let result = run_read_only_files_with_input(
            sandbox.path(),
            AgentId::new("agent-a"),
            RunId::new(run),
            Limits::new(6),
            RunInput::text(input).expect("输入在上限内"),
            &mut model,
        )
        .expect("装配并跑完");

        // 读取这一轮之前，一定存在一次列目录：模型是先看目录再取文件的。
        let read_index = result
            .run
            .observed()
            .iter()
            .position(|observation| observation.action().as_str() == "files_read")
            .expect("读过一次");
        assert!(
            result.run.observed()[..read_index]
                .iter()
                .any(|observation| observation.action().as_str() == "files_list"),
            "读取之前必须先列目录"
        );
        assert_eq!(
            result.run.observed()[read_index].params().get("path"),
            Some(input)
        );
        picks.push(model.inner().picked.clone());
    }

    assert_eq!(
        picks,
        vec![Some("alpha.txt".to_string()), Some("beta.txt".to_string())],
        "不同输入必须导致不同的目标"
    );
}

/// 3. 输入 → 展开 → 列目录 → 读取 → 再次展开：经历前缀扩展，正文不重复、不隐藏。
#[test]
fn the_input_leads_the_trace_and_the_trace_only_grows() {
    let sandbox = Sandbox::new("prefix");
    let marker = "prefix-marker-alpha";
    sandbox.write("alpha.txt", marker);

    let input = "alpha.txt";
    let mut model = InspectsFirst::new(InputDrivenModel::new());
    run_read_only_files_with_input(
        sandbox.path(),
        AgentId::new("agent-a"),
        RunId::new("run-1"),
        Limits::new(6),
        RunInput::text(input).expect("输入在上限内"),
        &mut model,
    )
    .expect("装配并跑完");

    // TASK_69：主体开场那几批"先看格式"（`inspect_action`）**也在经历里**，而且是整批一次落下
    // ⇒ 条数不再是"每轮 +1"（那是旧形态一轮一动作的算术巧合）。本判据真正要守的是
    // **只追加、不重排、不重复**，所以断言"严格递增 + 逐字节前缀"。
    let counts: Vec<usize> = model
        .inner()
        .seen
        .iter()
        .map(|(_, _, count, _)| *count)
        .collect();
    assert!(
        counts.windows(2).all(|pair| pair[0] < pair[1]),
        "经历只追加：每轮是上一轮的前缀扩展: {counts:?}"
    );
    let rounds: Vec<&Vec<String>> = model
        .inner()
        .seen
        .iter()
        .map(|(_, _, _, bodies)| bodies)
        .collect();
    for pair in rounds.windows(2) {
        let (earlier, later) = (&pair[0], &pair[1]);
        assert!(
            later.len() >= earlier.len() && later[..earlier.len()] == earlier[..],
            "每轮必须是上一轮的前缀扩展: {earlier:?} -> {later:?}"
        );
    }

    for (index, (seen_input, _, _, bodies)) in model.inner().seen.iter().enumerate() {
        assert_eq!(seen_input, input, "第 {} 轮输入仍在前", index + 1);
        if index >= 3 {
            let joined = bodies.join("\n");
            assert_eq!(
                joined.matches(marker).count(),
                1,
                "第 {} 轮里读取到的正文必须恰好出现一次（不重复、不隐藏）",
                index + 1
            );
        }
    }
}

/// 5. 输入上限：空、边界、多字节边界、超限；超限时不调用模型/动作，不写事实，不动既有活动槽。
#[test]
fn the_input_limit_is_enforced_before_anything_happens() {
    // 空输入合法。
    assert!(RunInput::empty().is_empty());
    assert_eq!(RunInput::text("").expect("空文本合法").len(), 0);

    // 字节边界：上限本身合法，多一个字节就拒绝。
    let at_limit = "a".repeat(MAX_INPUT_BYTES);
    let input = RunInput::text(at_limit).expect("正好到上限应当合法");
    assert_eq!(input.len(), MAX_INPUT_BYTES);
    assert_eq!(
        RunInput::text("a".repeat(MAX_INPUT_BYTES + 1)).unwrap_err(),
        RunInputError::TooLarge {
            bytes: MAX_INPUT_BYTES + 1
        }
    );

    // 多字节 UTF-8：按**字节**而不是字符计数。
    let chars_at_limit = MAX_INPUT_BYTES / '你'.len_utf8();
    let multibyte = "你".repeat(chars_at_limit);
    assert_eq!(multibyte.len(), MAX_INPUT_BYTES - (MAX_INPUT_BYTES % 3));
    let multibyte_input = RunInput::text(multibyte.clone()).expect("整字到上限内应当合法");
    assert_eq!(multibyte_input.len(), multibyte.len());
    let one_more = format!("{multibyte}你");
    assert!(one_more.len() > MAX_INPUT_BYTES);
    assert!(RunInput::text(one_more).is_err(), "多一个多字节字符就超限");

    // 超限发生在占用活动槽、写事实、调用模型或动作之前。
    //
    // 这里的证明是**结构性**的：`RunInput::text` 是产生非空输入的唯一入口，`Run::begin_with_input`
    // 与 `run_activity_with_input` 都只接受已经过检查的 `RunInput`，所以超限内容在类型上就到不了
    // 驱动流程——不存在"先占用槽位再发现超限"的路径。下面同时从外面确认槽位与账本都没被碰过。
    let mut agent = Agent::new(AgentId::new("agent-a"));
    let held = celira_agent::core::Run::begin(&mut agent, RunId::new("run-held"), Limits::new(2))
        .expect("先占住活动槽");
    let ledger = Ledger::new(AgentId::new("agent-a"));

    let refused = RunInput::text("x".repeat(MAX_INPUT_BYTES + 1));
    assert_eq!(
        refused.unwrap_err(),
        RunInputError::TooLarge {
            bytes: MAX_INPUT_BYTES + 1
        },
        "超限必须在构造输入时就被拒绝"
    );
    assert_eq!(
        agent.activity(),
        Some(held.id()),
        "被拒绝的输入不得改变既有活动槽"
    );
    assert!(ledger.records().is_empty(), "被拒绝的输入不得写入任何事实");
}

/// 6. 同一 Agent 顺序两次 Run：不继承前一次输入或正文（不需要新的 Host 类型）。
#[test]
fn a_second_run_never_inherits_the_previous_input_or_bodies() {
    let sandbox = Sandbox::new("no-inherit");
    let first_marker = "no-inherit-first-marker";
    sandbox.write("alpha.txt", first_marker);

    let mut agent = Agent::new(AgentId::new("agent-a"));
    let mut ledger = Ledger::new(AgentId::new("agent-a"));
    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");

    // 第一次：带输入，读完 alpha。
    let mut first = ScriptedModel {
        script: vec![
            ModelOutput::act(expand()),
            ModelOutput::act(listing_action()),
            ModelOutput::act(read("alpha.txt")),
            ModelOutput::finish(SubjectEnding::Completed),
        ],
    };
    let mut expansion = RunExpansion::new(AgentId::new("agent-a"), RunId::new("run-1"));
    let mut first_scope = os.scope(&mut expansion);
    let first_run = run_activity_with_input(
        &mut agent,
        RunId::new("run-1"),
        Limits::new(5),
        RunInput::text("第一次：alpha.txt").expect("输入合法"),
        &mut first,
        &mut first_scope,
        &mut ledger,
    )
    .expect("第一次活动");

    // 第二次：不同输入、空正文起手，看不到上一次的输入或正文。
    let mut second = ScriptedModel {
        script: vec![
            ModelOutput::act(expand()),
            ModelOutput::act(listing_action()),
            ModelOutput::finish(SubjectEnding::Silent),
        ],
    };
    let mut expansion2 = RunExpansion::new(AgentId::new("agent-a"), RunId::new("run-2"));
    let mut second_scope = os.scope(&mut expansion2);
    let second_run = run_activity_with_input(
        &mut agent,
        RunId::new("run-2"),
        Limits::new(4),
        RunInput::text("第二次：只看目录").expect("输入合法"),
        &mut second,
        &mut second_scope,
        &mut ledger,
    )
    .expect("第二次活动");

    assert_eq!(first_run.input().as_str(), "第一次：alpha.txt");
    assert_eq!(second_run.input().as_str(), "第二次：只看目录");
    let second_view_text = format!(
        "{:?}",
        second_run
            .observed()
            .iter()
            .flat_map(|observation| observation.outcome().content().iter())
            .map(|block| block.text())
            .collect::<Vec<_>>()
    );
    assert!(
        !second_view_text.contains(first_marker),
        "新 Run 不得继承上一次的正文"
    );
    assert!(
        !second_run.input().as_str().contains("第一次"),
        "新 Run 不得继承上一次的输入"
    );
    // 两本活动都在同一本账里，但账里没有输入正文。
    assert!(
        !format!("{:?}", ledger.records()).contains(first_marker)
            && !format!("{:?}", ledger.records()).contains("第一次"),
        "事实里既没有文件正文，也没有初始输入"
    );
}

/// 7（部分）+ 8：回执未达不终止活动（2026-10-24），主体看过读数后自己收场；事实里没有输入与正文。
#[test]
fn an_unknown_result_keeps_the_run_alive_and_leaves_no_input_in_facts() {
    /// 永远返回"无法确认"的执行端。
    struct UnknownActions {
        calls: usize,
    }

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
            self.calls += 1;
            ActionOutcome::unknown()
        }
    }

    /// 数调用次数的模型：第一次请求行动，看见"没拿到回执"之后自己收场。
    struct CountingModel {
        calls: usize,
    }

    impl ModelPort for CountingModel {
        fn complete(&mut self, _view: &View) -> Result<ModelOutput, ModelError> {
            self.calls += 1;
            Ok(if self.calls == 1 {
                ModelOutput::act(Action::new(ActionId::new("probe")))
            } else {
                ModelOutput::finish(SubjectEnding::Silent)
            })
        }
    }

    let marker = "unknown-marker-input";
    let mut agent = Agent::new(AgentId::new("agent-a"));
    let mut ledger = Ledger::new(AgentId::new("agent-a"));
    let mut model = CountingModel { calls: 0 };
    let mut actions = UnknownActions { calls: 0 };

    let run = run_activity_with_input(
        &mut agent,
        RunId::new("run-1"),
        Limits::new(4),
        RunInput::text(marker).expect("输入合法"),
        &mut model,
        &mut actions,
        &mut ledger,
    )
    .expect("活动可以跑完");

    assert_eq!(
        run.outcome(),
        Some(celira_agent::foundation::Outcome::Subject(
            SubjectEnding::Silent
        )),
        "回执未达不终止活动：收场是主体自己说的"
    );
    assert_eq!(
        model.calls, 2,
        "没看见之后主体还得有一次机会（读数已经在视野里）"
    );
    assert_eq!(actions.calls, 1);
    assert_eq!(run.observed()[0].state(), ResultState::Unknown);
    assert!(
        !format!("{:?}", ledger.records()).contains(marker),
        "事实里不得出现初始输入"
    );
    assert_eq!(agent.activity(), None, "收场后活动槽必须交还");
}
