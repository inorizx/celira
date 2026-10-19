//! 只读文件视图的离线证明：通用 `files_list` / `files_read` 同时访问**授权资料**与**派生经历目录**。
//!
//! 全部离线、全合成数据、只用测试自建的临时目录。环境里的经历用存储层的只追加接口写成
//! 合成记录（不是伪造文件内容），视图读的是**同一个环境目录**的已确认前缀。
//!
//! 这里刻意不经过模型：直接驱动 OS 的使用作用域，用与真实运行同一套路由和同一个 ActionPort
//! 调用视图 App，验证通用文件动作的可见性、边界与失败分类。

use std::fs;
use std::path::PathBuf;

use celira_agent::core::{
    Action, ActionId, ActionOutcome, ActionParams, ActionPort, Capabilities, Decision,
    HttpStatusDetail, ModelFailureClass, ModelOutput, ModelResponse, ProtocolData, TransportDetail,
};
use celira_agent::file_app::{FILE_APP_ID, MAX_TEXT_BYTES};
use celira_agent::file_view::{FileView, FileViewFailure, WORKS_DIR};
use celira_agent::foundation::{
    AgentId, OperationId, Outcome, ResultState, RunId, SubjectEnding, SystemEnding, Turn,
};
use celira_agent::os::{MinimalOs, RunExpansion};
use celira_agent::store::{Environment, TracePayload, ending_payload, response_payload};

const EXPAND: &str = "open_app";
const LIST: &str = "files_list";
const READ: &str = "files_read";
const HISTORY: &str = "history";

/// 自建临时根：`root/env` 是身份环境，`root/materials` 是授权资料目录。
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let unique = format!(
            "celira-history-view-{}-{}-{}",
            std::process::id(),
            label,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("时钟可用")
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        fs::create_dir_all(root.join("materials")).expect("建资料目录");
        Self { root }
    }

    fn materials(&self) -> PathBuf {
        self.root.join("materials")
    }

    fn environment_root(&self) -> PathBuf {
        self.root.join("env")
    }

    fn write_material(&self, relative: &str, text: &str) {
        let path = self.materials().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("建子目录");
        }
        fs::write(path, text).expect("写合成资料");
    }

    fn agent() -> AgentId {
        AgentId::new("agent-alpha")
    }

    /// 建环境并写入若干合成经历。
    fn environment(&self, runs: &[(&str, Vec<TracePayload>)]) -> Environment {
        let mut env =
            Environment::create(&self.environment_root(), Self::agent()).expect("建立身份环境");
        for (run, payloads) in runs {
            for payload in payloads {
                env.append_trace(&RunId::new(*run), payload.clone())
                    .expect("追加合成经历");
            }
        }
        env
    }

    /// 用**同一个**只读文件视图装配这台 OS（资料目录 + 派生经历目录）。
    fn machine(&self) -> MinimalOs {
        MinimalOs::with_history(&self.materials(), &self.environment_root())
            .expect("装配只读文件视图")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn operation(run: &str) -> OperationId {
    OperationId::new(RunId::new(run), Turn::FIRST)
}

fn expand() -> Action {
    Action::with_params(
        ActionId::new(EXPAND),
        ActionParams::none().with("target", FILE_APP_ID),
    )
}

fn list(path: &str) -> Action {
    let params = if path.is_empty() {
        ActionParams::none()
    } else {
        ActionParams::none().with("path", path)
    };
    Action::with_params(ActionId::new(LIST), params)
}

fn read(path: &str) -> Action {
    Action::with_params(ActionId::new(READ), ActionParams::none().with("path", path))
}

/// 一条「活动开始 + 输入 + 一段注入 + 收场 + 封存」的合成经历。
fn complete_run(text: &str, note: &str) -> Vec<TracePayload> {
    vec![
        TracePayload::RunStarted { origin: None },
        TracePayload::RunInput {
            text: text.to_string(),
        },
        TracePayload::Injection {
            stage: "run-start".to_string(),
            turn: None,
            blocks: vec![("note".to_string(), note.to_string())],
        },
        ending_payload(Outcome::Subject(SubjectEnding::Completed)),
        TracePayload::Sealed,
    ]
}

/// 展开视图 App 并返回它这一次活动的作用域。
///
/// 展开状态是**自有值**（`RunExpansion`），因此由调用方持有并传进来：作用域只是它的临时视图，
/// 借用它、不拥有它。（旧规矩里这里还并列着 `PendingEffects`——未决效果集合已随门退役。）
fn expanded_scope<'a>(
    os: &'a mut MinimalOs,
    expansion: &'a mut RunExpansion,
    agent: &AgentId,
    run: &RunId,
) -> celira_agent::os::RunScope<'a> {
    let mut scope = os.scope(expansion);
    let outcome = scope.execute(agent, &operation(run.as_str()), &expand());
    assert_eq!(outcome.state(), ResultState::Success, "展开必须是明确成功");
    scope
}

fn text_of(outcome: &ActionOutcome) -> String {
    outcome
        .content()
        .iter()
        .map(|block| block.text())
        .collect::<String>()
}

/// 1. 一个视图、一套通用动作：资料目录与派生经历目录都能列、都能读。
#[test]
fn one_view_lists_and_reads_both_materials_and_derived_history() {
    let fixture = Fixture::new("both");
    fixture.write_material("alpha.txt", "资料正文 alpha");
    fixture.write_material("nested/inner.txt", "子目录正文");
    let _env = fixture.environment(&[("run-a", complete_run("第一段中文经历", "旧的合成注释"))]);

    let agent = Fixture::agent();
    let run = RunId::new("run-viewer");
    let mut os = fixture.machine();
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    let op = operation("run-viewer");

    // 视图根：真实条目 + 一个保留逻辑目录。
    let root_listing = scope.execute(&agent, &op, &list(""));
    assert_eq!(root_listing.state(), ResultState::Success);
    let root_text = text_of(&root_listing);
    assert!(root_text.contains("alpha.txt\tfile"), "{root_text}");
    assert!(root_text.contains("nested\tdir"), "{root_text}");
    assert!(root_text.contains("history\tdir"), "{root_text}");

    // 资料目录：整条动作委托给真实目录实现，正文逐字返回。
    let material = scope.execute(&agent, &op, &read("alpha.txt"));
    assert_eq!(material.state(), ResultState::Success);
    assert_eq!(text_of(&material), "资料正文 alpha");
    let nested = scope.execute(&agent, &op, &read("nested/inner.txt"));
    assert_eq!(text_of(&nested), "子目录正文");

    // 派生经历目录：同一套通用动作。
    let runs = scope.execute(&agent, &op, &list(HISTORY));
    let runs_text = text_of(&runs);
    assert!(runs_text.contains("run-a\tdir"), "{runs_text}");

    let segments = scope.execute(&agent, &op, &list(&format!("{HISTORY}/run-a")));
    let segments_text = text_of(&segments);
    assert!(segments_text.contains("000.txt\tfile"), "{segments_text}");

    let segment = scope.execute(&agent, &op, &read(&format!("{HISTORY}/run-a/000.txt")));
    assert_eq!(segment.state(), ResultState::Success);
    let text = text_of(&segment);
    assert!(text.starts_with("run = run-a\n"), "{text}");
    assert!(text.contains("state = complete\n"), "{text}");
    assert!(text.contains("ending = subject:completed\n"), "{text}");
    assert!(text.contains("records = 5\n"), "{text}");
    // 记录按序号排列，正文逐字返回（UTF-8 完整）。
    assert!(text.contains("#0 run-start\n"), "{text}");
    assert!(text.contains("#1 run-input\n第一段中文经历\n"), "{text}");
    assert!(text.contains("旧的合成注释"), "{text}");
    let first = text.find("#0 run-start").expect("第一条记录");
    let second = text.find("#1 run-input").expect("第二条记录");
    assert!(first < second, "记录必须按序号排列: {text}");
}

/// 2. 存储结构状态被忠实区分：完整收尾 / 有结束未封存 / 中断，互不冒充。
#[test]
fn derived_state_separates_sealed_unsealed_and_interrupted() {
    let fixture = Fixture::new("state");
    let mut unsealed = complete_run("未封存的活动", "note");
    unsealed.pop(); // 去掉 Sealed：有结束事实、没有封存。
    let _env = fixture.environment(&[
        ("run-complete", complete_run("完整收尾", "note")),
        ("run-unsealed", unsealed),
        (
            "run-interrupted",
            vec![TracePayload::RunStarted { origin: None }],
        ),
    ]);

    let agent = Fixture::agent();
    let run = RunId::new("run-viewer");
    let mut os = fixture.machine();
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    let op = operation("run-viewer");

    let runs = text_of(&scope.execute(&agent, &op, &list(HISTORY)));
    for expected in [
        "run-complete\tdir",
        "run-interrupted\tdir",
        "run-unsealed\tdir",
    ] {
        assert!(runs.contains(expected), "缺少 {expected}: {runs}");
    }

    let state_of = |scope: &mut celira_agent::os::RunScope<'_>, name: &str| {
        let outcome = scope.execute(&agent, &op, &read(&format!("{HISTORY}/{name}/000.txt")));
        assert_eq!(outcome.state(), ResultState::Success, "{name} 应当可读");
        text_of(&outcome)
    };

    let complete = state_of(&mut scope, "run-complete");
    assert!(complete.contains("state = complete\n"), "{complete}");
    assert!(
        complete.contains("ending = subject:completed\n"),
        "{complete}"
    );

    let unsealed = state_of(&mut scope, "run-unsealed");
    assert!(unsealed.contains("state = ended-unsealed\n"), "{unsealed}");
    assert!(
        unsealed.contains("ending = subject:completed\n"),
        "{unsealed}"
    );

    let interrupted = state_of(&mut scope, "run-interrupted");
    assert!(
        interrupted.contains("state = interrupted\n"),
        "{interrupted}"
    );
    assert!(
        !interrupted.contains("ending = "),
        "中断的活动不得被描述成有结束事实: {interrupted}"
    );
    assert!(
        !interrupted.contains("complete\n"),
        "中断不得被写成任务完成: {interrupted}"
    );
}

/// 3. 大经历**显式分段**读完：不静默截断、不丢记录、不重复。
#[test]
fn a_long_run_is_read_back_in_explicit_segments_without_gaps() {
    let fixture = Fixture::new("segments");
    let mut payloads = vec![TracePayload::RunStarted { origin: None }];
    for turn in 1..=120u32 {
        payloads.push(TracePayload::Injection {
            stage: "turn".to_string(),
            turn: Some(turn),
            blocks: vec![("note".to_string(), format!("记录 {turn}"))],
        });
    }
    let _env = fixture.environment(&[("run-long", payloads)]);

    let agent = Fixture::agent();
    let run = RunId::new("run-viewer");
    let mut os = fixture.machine();
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    let op = operation("run-viewer");

    let segments_text = text_of(&scope.execute(&agent, &op, &list(&format!("{HISTORY}/run-long"))));
    let names: Vec<String> = segments_text
        .lines()
        .skip(1)
        .map(|line| line.split('\t').next().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["000.txt", "001.txt", "002.txt"],
        "121 条经历应当分成 3 段，实际: {segments_text}"
    );

    let mut seqs: Vec<u64> = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let outcome = scope.execute(&agent, &op, &read(&format!("{HISTORY}/run-long/{name}")));
        assert_eq!(outcome.state(), ResultState::Success);
        let text = text_of(&outcome);
        assert!(
            text.len() <= MAX_TEXT_BYTES as usize,
            "每一段都必须在上限之内：{}",
            text.len()
        );
        assert!(
            text.contains(&format!("segment = {}/{}\n", index + 1, names.len())),
            "{text}"
        );
        for line in text.lines().filter(|line| line.starts_with('#')) {
            let seq: u64 = line[1..]
                .split(' ')
                .next()
                .expect("序号")
                .parse()
                .expect("序号是数字");
            seqs.push(seq);
        }
    }

    let expected: Vec<u64> = (0..=120).collect();
    assert_eq!(seqs, expected, "分段读取不得丢记录或重复记录");
    // 分段边界上的正文各自只出现一次。
    let all = names
        .iter()
        .map(|name| {
            text_of(&scope.execute(&agent, &op, &read(&format!("{HISTORY}/run-long/{name}"))))
        })
        .collect::<String>();
    assert_eq!(all.matches("记录 50").count(), 1, "第 50 条只能出现一次");
    assert_eq!(all.matches("记录 51").count(), 1, "第 51 条只能出现一次");
}

/// 4. 未知路径、未知活动、类型不符与越界路径都是**明确失败**，不冒充空目录。
#[test]
fn unknown_and_out_of_bounds_addressing_never_looks_empty() {
    let fixture = Fixture::new("failures");
    fixture.write_material("alpha.txt", "资料正文");
    let _env = fixture.environment(&[("run-a", complete_run("经历", "note"))]);

    let agent = Fixture::agent();
    let run = RunId::new("run-viewer");
    let mut os = fixture.machine();
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    let op = operation("run-viewer");

    let cases: Vec<(Action, &str)> = vec![
        (list(&format!("{HISTORY}/nope")), "not-found"),
        (read(&format!("{HISTORY}/nope/000.txt")), "not-found"),
        (read(&format!("{HISTORY}/run-a/999.txt")), "not-found"),
        (list(&format!("{HISTORY}/run-a/000.txt")), "not-found"),
        (
            list(&format!("{HISTORY}/run-a/000.txt/deeper")),
            "not-found",
        ),
        (read(HISTORY), "wrong-kind"),
        (read(&format!("{HISTORY}/run-a")), "wrong-kind"),
        (list("alpha.txt"), "wrong-kind"),
        (read(&format!("{HISTORY}/../alpha.txt")), "invalid-path"),
        (read("../../alpha.txt"), "invalid-path"),
        (read("C:/alpha.txt"), "invalid-path"),
        (read(""), "invalid-path"),
        (read("missing.txt"), "not-found"),
    ];
    for (action, expected) in cases {
        let outcome = scope.execute(&agent, &op, &action);
        assert_eq!(
            outcome.state(),
            ResultState::Failure,
            "必须明确失败：{action:?}"
        );
        assert_eq!(
            outcome.failure_class(),
            Some(expected),
            "失败分类不对：{action:?}"
        );
        assert!(outcome.content().is_empty(), "失败不得夹带内容：{action:?}");
    }
}

/// **她自己写下的自识有一条现读的路**（2026-10-13）。
///
/// 与上一条**不冲突，而且是它的另一半**：环境**内部记录**（身份 / 两份存储 / 写锁）一律不可读；
/// 而她**自己的自持物**里，自识从前是唯一一个"写完读不回来"的——笔记有 `notes_read`、格位有
/// `pins_board`、约的时刻有 `timer_list`，只有自识要等下一场开场才看得见（她自己列的第②条缺陷）。
///
/// 判据落在那条路的**每一步**上：在场时列得出来、读得到、`write_self_concept` 写进去的**同一段里**读得到
/// 新内容（不缓存）；不在场时是"没有这个地址"（不是空文件）；它不是目录、也不在查找面内。
#[test]
fn her_own_self_concept_is_readable_at_its_reserved_address() {
    use celira_agent::self_concept::SELF_CONCEPT_FILE;

    let fixture = Fixture::new("self_concept-address");
    fixture.write_material("alpha.txt", "资料正文");
    // 自识**不在**时：地址缺席（缺席 ≠ 空文件，也 ≠ 读不到）。
    let _env = fixture.environment(&[("run-a", complete_run("经历", "note"))]);

    let agent = Fixture::agent();
    let run = RunId::new("run-self_concept");
    let mut os = fixture.machine();
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    let op = operation("run-self_concept");

    let mut listing = text_of(&scope.execute(&agent, &op, &list("")));
    assert!(
        !listing.contains(SELF_CONCEPT_FILE),
        "还没写过自识 ⇒ 地址缺席，根列表里也不该有它: {listing}"
    );
    assert_eq!(
        scope
            .execute(&agent, &op, &read(SELF_CONCEPT_FILE))
            .failure_class(),
        Some("not-found"),
        "地址不在 ⇒ 没有这个路径（不是空文件）"
    );

    // 她写下一段自识（写入口就是 `write_self_concept` 那份存储；这里直接落盘，等价于她刚写完那一刻）。
    let written = "我是这样一个人：写得慢，但写完的会一直算数。";
    std::fs::write(fixture.environment_root().join(SELF_CONCEPT_FILE), written).expect("写下自识");

    // **装配期拍一次**：地址在不在是那一刻的事实 ⇒ 重新装配一次视图（真机上"写完立刻读"发生在
    // 同一段里，靠的是**读的时候现读**，见下面那条）。
    let mut os = fixture.machine();
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    listing = text_of(&scope.execute(&agent, &op, &list("")));
    assert!(
        listing.contains(SELF_CONCEPT_FILE),
        "写过之后地址要在根列表里: {listing}"
    );
    let read_back = scope.execute(&agent, &op, &read(SELF_CONCEPT_FILE));
    assert_eq!(
        read_back.state(),
        ResultState::Success,
        "读得到她自己写的那一篇: {read_back:?}"
    );
    assert_eq!(text_of(&read_back), written, "逐字是她写下的那一份");

    // 它是**一份文件**：底下没有子路径；也不在查找面内（能整份读的东西不需要查）。
    assert_eq!(
        scope
            .execute(&agent, &op, &read(&format!("{SELF_CONCEPT_FILE}/x")))
            .failure_class(),
        Some("wrong-kind")
    );
    assert_eq!(
        scope
            .execute(&agent, &op, &list(SELF_CONCEPT_FILE))
            .failure_class(),
        Some("wrong-kind"),
        "对它列目录是类型不对"
    );
    let found = Action::with_params(
        ActionId::new("files_grep"),
        ActionParams::none()
            .with("pattern", "算数")
            .with("path", SELF_CONCEPT_FILE),
    );
    assert_eq!(
        scope.execute(&agent, &op, &found).failure_class(),
        Some("invalid-argument"),
        "保留的单文件地址不在查找面内"
    );
}

/// 5. 视图不暴露环境内部记录；资料根与环境根重叠时**拒绝装配**。
#[test]
fn the_view_never_exposes_environment_internals() {
    let fixture = Fixture::new("isolation");
    fixture.write_material("alpha.txt", "资料正文");
    let _env = fixture.environment(&[("run-a", complete_run("经历", "note"))]);

    let agent = Fixture::agent();
    let run = RunId::new("run-viewer");
    let mut os = fixture.machine();
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    let op = operation("run-viewer");

    // 环境目录里的内部文件名在这个视图里都不是可访问路径。
    let internals = ["identity.json", "trace.ndjson", "facts.ndjson"];
    let mut root_listing = text_of(&scope.execute(&agent, &op, &list("")));
    for name in internals {
        assert_eq!(
            scope.execute(&agent, &op, &read(name)).failure_class(),
            Some("not-found"),
            "{name} 不该可读"
        );
        assert!(!root_listing.contains(name), "根列表不得列出 {name}");
    }
    assert!(!root_listing.contains("writer"), "根列表不得列出写锁");
    root_listing.push_str(&text_of(&scope.execute(&agent, &op, &list(HISTORY))));
    for name in internals {
        assert!(
            !root_listing.contains(name),
            "任何一层列表都不得列出 {name}: {root_listing}"
        );
    }
    // 派生经历目录的内容只来自经历记录：逐条都是记录形状。
    let segment = text_of(&scope.execute(&agent, &op, &read(&format!("{HISTORY}/run-a/000.txt"))));
    assert!(segment.lines().any(|line| line.starts_with("#0 run-start")));

    // 装配期拒绝：资料根就是环境根 / 资料根覆盖环境根 / 路径别名指向环境根。
    let environment_root = fixture.environment_root();
    for alias in [
        environment_root.clone(),
        fixture.root.clone(),
        environment_root.join("..").join("env"),
    ] {
        assert_eq!(
            FileView::with_history(&alias, &environment_root).unwrap_err(),
            FileViewFailure::MaterialsExposeEnvironment,
            "资料根 {} 必须被拒绝",
            alias.display()
        );
    }

    // 装配期拒绝：资料根里已经有名为 history 的真实条目（地址无法唯一解释）。
    let taken = fixture.root.join("taken");
    fs::create_dir_all(taken.join(HISTORY)).expect("建同名真实目录");
    assert_eq!(
        FileView::with_history(&taken, &environment_root).unwrap_err(),
        FileViewFailure::ReservedAddressTaken
    );
}

/// 6. 单条记录本身就超上限：它独占一段，读取时明确拒绝，**不**截断半条。
#[test]
fn an_oversized_single_record_is_refused_instead_of_truncated() {
    let fixture = Fixture::new("oversized");
    let huge = "汉".repeat(30_000); // 90_000 字节，超过 64 KiB 的读取上限。
    let _env = fixture.environment(&[(
        "run-huge",
        vec![
            TracePayload::RunStarted { origin: None },
            TracePayload::RunInput { text: huge },
        ],
    )]);

    let agent = Fixture::agent();
    let run = RunId::new("run-viewer");
    let mut os = fixture.machine();
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    let op = operation("run-viewer");

    // 目录仍然可列：这一次活动确实是可查阅的。
    let listing = text_of(&scope.execute(&agent, &op, &list(&format!("{HISTORY}/run-huge"))));
    assert!(listing.contains("000.txt\tfile"), "{listing}");
    assert!(listing.contains("001.txt\tfile"), "{listing}");
    let names: Vec<&str> = listing
        .lines()
        .skip(1)
        .map(|line| line.split('\t').next().unwrap_or_default())
        .collect();

    // 超上限的那条**独占**一段：相邻那段照常可读，说明边界是显式的，而不是悄悄丢内容。
    let small = scope.execute(&agent, &op, &read(&format!("{HISTORY}/run-huge/000.txt")));
    assert_eq!(small.state(), ResultState::Success);
    assert!(text_of(&small).contains("#0 run-start\n"));

    let last = names.last().copied().expect("至少有一段");
    let outcome = scope.execute(&agent, &op, &read(&format!("{HISTORY}/run-huge/{last}")));
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(outcome.failure_class(), Some("too-large"));
    assert!(outcome.content().is_empty());
}

/// 7. 读取视图不改写任何可靠性文件：环境的字节与目录项都不变。
#[test]
fn reading_derived_history_never_rewrites_the_environment() {
    let fixture = Fixture::new("read-only");
    fixture.write_material("alpha.txt", "资料正文");
    let _env = fixture.environment(&[
        ("run-a", complete_run("第一段", "note-a")),
        ("run-b", complete_run("第二段", "note-b")),
    ]);

    let environment_root = fixture.environment_root();

    let agent = Fixture::agent();
    let run = RunId::new("run-viewer");
    let mut os = fixture.machine();
    // ⚠ **快照取在装配之后**（2026-10-14）：装配期会给**作品面**铺一个空目录
    // （`<环境根>/projects`，缺席才建——见 `file_view::works_face` 与施工单 §12.12）：
    // 那是**安装**的动作，不是"读"的动作。这一条用例量的是**读**——装配完成之后，
    // 读经历不许再动环境里的任何一样东西（作品目录那条路另有用例钉着）。
    assert!(
        environment_root.join(WORKS_DIR).is_dir(),
        "前提：装配期已经把作品目录铺好"
    );
    let trace_before = fs::read(environment_root.join("trace.ndjson")).expect("读回经历存储");
    let facts_before = fs::read(environment_root.join("facts.ndjson")).expect("读回事实存储");
    let mut names_before: Vec<String> = fs::read_dir(&environment_root)
        .expect("列环境目录")
        .map(|entry| {
            entry
                .expect("目录项")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names_before.sort();

    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    let op = operation("run-viewer");
    for target in ["", HISTORY, "history/run-a", "history/run-b"] {
        let outcome = scope.execute(&agent, &op, &list(target));
        assert_eq!(outcome.state(), ResultState::Success, "列 {target}");
    }
    for target in ["history/run-a/000.txt", "history/run-b/000.txt"] {
        assert_eq!(
            scope.execute(&agent, &op, &read(target)).state(),
            ResultState::Success
        );
    }

    assert_eq!(
        fs::read(environment_root.join("trace.ndjson")).expect("再读经历存储"),
        trace_before,
        "读取视图不得改写经历存储"
    );
    assert_eq!(
        fs::read(environment_root.join("facts.ndjson")).expect("再读事实存储"),
        facts_before,
        "读取视图不得改写事实存储"
    );
    let mut names_after: Vec<String> = fs::read_dir(&environment_root)
        .expect("列环境目录")
        .map(|entry| {
            entry
                .expect("目录项")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names_after.sort();
    assert_eq!(names_after, names_before, "读取视图不得新建或删除环境文件");
}

/// 8. 没有经历目录的视图（一次性装配）里，`history` 这个地址不存在：明确失败，不冒充空目录。
#[test]
fn a_view_without_history_has_no_derived_directory() {
    let fixture = Fixture::new("no-history");
    fixture.write_material("alpha.txt", "资料正文");

    let mut os = MinimalOs::open(&fixture.materials()).expect("装配只读资料视图");
    let agent = Fixture::agent();
    let run = RunId::new("run-viewer");
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    let op = operation("run-viewer");

    let listing = text_of(&scope.execute(&agent, &op, &list("")));
    assert!(listing.contains("alpha.txt\tfile"), "{listing}");
    assert!(!listing.contains(HISTORY), "没有经历目录时不该出现保留地址");

    let outcome = scope.execute(&agent, &op, &list(HISTORY));
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(outcome.failure_class(), Some("not-found"));
}

/// 只读视图本身不越权：未展开时任何文件动作都被拒绝，且不触碰文件系统。
#[test]
fn an_unexpanded_view_dispatches_nothing() {
    let fixture = Fixture::new("unexpanded");
    fixture.write_material("alpha.txt", "资料正文");
    let _env = fixture.environment(&[]);

    let agent = Fixture::agent();
    let run = RunId::new("run-viewer");
    let mut os = fixture.machine();
    let mut expansion2 = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = os.scope(&mut expansion2);
    let op = operation("run-viewer");

    let capabilities: Capabilities = scope.capabilities(&agent, &run);
    assert_eq!(
        capabilities.len(),
        6,
        "入口只有 OS 自己的六项能力（展开 / 看详情 / 钉 / 摘 / 写自识 / 写交接）"
    );
    assert!(capabilities.find(&ActionId::new(LIST)).is_none());

    for action in [
        list(""),
        read("alpha.txt"),
        read(&format!("{HISTORY}/run-a/000.txt")),
    ] {
        let outcome = scope.execute(&agent, &op, &action);
        assert_eq!(outcome.state(), ResultState::Failure);
        assert_eq!(outcome.failure_class(), Some("not-expanded"));
    }
}

/// 9. 模型响应进普通历史：**读得到正文、读不到续接载荷**，而且正文只出现一次。
///
/// 这是"单点收敛"的行为证据：所有把经历变成正文的地方都走 `TracePayload::text()`，
/// 因此续接载荷既不进历史文件、也不进字节预算，不必在每一处投影上各加一条过滤。
#[test]
fn derived_history_shows_the_body_once_and_never_the_continuation_payload() {
    let fixture = Fixture::new("model-response");
    const BODY: &str = "模型正文标记-ONLY-BODY";
    const PAYLOAD: &str = "续接载荷标记-ONLY-PAYLOAD";

    // 一条带正文与续接载荷的行动响应，后面跟着它自己的回执（真实驱动会写出的相邻形状）。
    let act = ModelResponse::new(
        Turn::FIRST,
        ModelOutput::new(
            BODY,
            Some(
                ProtocolData::new(
                    "synthetic-wire",
                    7,
                    format!("{{\"call_id\":\"call-9\",\"reasoning\":\"{PAYLOAD}\"}}"),
                )
                .expect("可以构造续接数据"),
            ),
            Decision::Act(vec![Action::with_params(
                ActionId::new(EXPAND),
                ActionParams::none().with("target", FILE_APP_ID),
            )]),
        ),
    );
    // 一条**没有正文**的响应：历史里给一条机械说明，而不是一行看起来像空白的记录。
    let silent = ModelResponse::new(
        Turn::from_number(2).expect("第 2 轮"),
        ModelOutput::new("", None, Decision::Finish(SubjectEnding::Silent)),
    );

    let payloads = vec![
        TracePayload::RunStarted { origin: None },
        TracePayload::RunInput {
            text: "开场".to_string(),
        },
        response_payload(&act),
        TracePayload::Observed {
            turn: 1,
            index: 0,
            action: EXPAND.to_string(),
            params: vec![("target".to_string(), FILE_APP_ID.to_string())],
            state: ResultState::Success,
            failure: None,
            blocks: vec![("listing".to_string(), "alpha.txt".to_string())],
        },
        response_payload(&silent),
        ending_payload(Outcome::Subject(SubjectEnding::Silent)),
        TracePayload::Sealed,
    ];
    let _env = fixture.environment(&[("run-model", payloads)]);

    let agent = Fixture::agent();
    let run = RunId::new("run-viewer");
    let mut os = fixture.machine();
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    let op = operation("run-viewer");

    let outcome = scope.execute(&agent, &op, &read(&format!("{HISTORY}/run-model/000.txt")));
    assert_eq!(outcome.state(), ResultState::Success, "普通历史应当可读");
    let text = text_of(&outcome);

    // 正文读得到，而且**只有一次**（不重复渲染、也不在别处再抄一遍）。
    assert_eq!(
        text.matches(BODY).count(),
        1,
        "模型正文必须在历史里恰好出现一次: {text}"
    );
    assert!(text.contains("model-response"), "{text}");
    // 续接载荷**一处都不出现**：不是"被截断"，而是根本不进这条路。
    for leaked in [PAYLOAD, "call-9", "synthetic-wire", "reasoning"] {
        assert!(
            !text.contains(leaked),
            "续接载荷不得经由普通历史暴露: {leaked}\n{text}"
        );
    }
    // 没有正文的那一条：给出机械说明，也不把载荷或决定细节当作正文补出来。
    assert!(
        text.contains("(no body; decision finish)"),
        "没有正文时给机械说明: {text}"
    );
    // 决定本身不在有正文的那一条里被复述（正文就是正文）。
    assert!(
        !text.contains("decision act"),
        "有正文时不得额外复述决定: {text}"
    );
}

/// 模型失败的收场：主体的视角里除了 `ending = system:model-failed`，还要有
/// `ending` 下面那行 `failure = transport-failure`。
///
/// 为什么这条判据重要：**派生经历是主体唯一读得到"那次失败属于哪一类"的地方**。
/// 事实侧（`run-ended`）一直带着分类，但事实不是主体的动词；宿主 stdout 也有，但
/// 那份日志不在主体的可见范围内。分类不进这里，主体复盘时就只能说"模型失败了"，
/// 说不出是哪一类失败 —— 那不是主体的问题，是记录形态的问题。
#[test]
fn a_model_failure_shows_its_class_in_derived_history() {
    let fixture = Fixture::new("failure-class");
    let failed = vec![
        TracePayload::RunStarted { origin: None },
        TracePayload::RunInput {
            text: String::new(),
        },
        TracePayload::Injection {
            stage: "run-start".to_string(),
            turn: None,
            blocks: vec![("note".to_string(), "门铃唤醒".to_string())],
        },
        ending_payload(Outcome::System(SystemEnding::ModelFailed(Some(
            ModelFailureClass::Transport(None).as_str(),
        )))),
        TracePayload::Sealed,
    ];
    // 旧记录形态：分类**没有被记录**。缺字段时不许编造一个出来。
    let unrecorded = vec![
        TracePayload::RunStarted { origin: None },
        ending_payload(Outcome::System(SystemEnding::ModelFailed(None))),
        TracePayload::Sealed,
    ];
    let _env = fixture.environment(&[("run-failed", failed), ("run-old", unrecorded)]);

    let agent = Fixture::agent();
    let run = RunId::new("run-viewer");
    let mut os = fixture.machine();
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    let op = operation("run-viewer");

    let segment = scope.execute(&agent, &op, &read(&format!("{HISTORY}/run-failed/000.txt")));
    assert_eq!(segment.state(), ResultState::Success);
    let text = text_of(&segment);
    assert!(text.contains("state = complete\n"), "{text}");
    assert!(text.contains("ending = system:model-failed\n"), "{text}");
    assert!(
        text.contains("failure = transport-failure\n"),
        "分类必须出现在段首: {text}"
    );

    // 旧记录：读得出来，但**不**给一个编出来的分类。
    let old = scope.execute(&agent, &op, &read(&format!("{HISTORY}/run-old/000.txt")));
    assert_eq!(old.state(), ResultState::Success);
    let old_text = text_of(&old);
    assert!(
        old_text.contains("ending = system:model-failed\n"),
        "{old_text}"
    );
    assert!(
        !old_text.contains("failure = "),
        "缺分类时不得凭空补一行: {old_text}"
    );
}

/// 带**细分档**的分类在经历侧也**原样**出现（TASK_60）。
///
/// 判据落在"落点不压扁"这一句上：细分的拼法（`<分类>:<细分>`）由 `as_str` 定，经历侧的段首
/// 直接转抄那个串 ⇒ 两个 family 各来一格，防止"只有某一族接上了细分"。
/// 与上一格的分工：上一格守"没细分的老记录不被加工"，这一格守"有细分的记录不被截断"。
#[test]
fn a_detailed_model_failure_shows_its_detail_in_derived_history() {
    let fixture = Fixture::new("failure-detail");
    let cases = [
        (
            "run-http",
            SystemEnding::ModelFailed(Some(
                ModelFailureClass::HttpStatus(Some(HttpStatusDetail::ServerSide)).as_str(),
            )),
            "failure = http-status-failure:server-side\n",
        ),
        (
            "run-transport",
            SystemEnding::ModelFailed(Some(
                ModelFailureClass::Transport(Some(TransportDetail::Timeout)).as_str(),
            )),
            "failure = transport-failure:timeout\n",
        ),
    ];

    let traces: Vec<(&str, Vec<TracePayload>)> = cases
        .iter()
        .map(|(run, ending, _)| {
            (
                *run,
                vec![
                    TracePayload::RunStarted { origin: None },
                    TracePayload::RunInput {
                        text: String::new(),
                    },
                    ending_payload(Outcome::System(*ending)),
                    TracePayload::Sealed,
                ],
            )
        })
        .collect();
    let _env = fixture.environment(&traces);

    let agent = Fixture::agent();
    let mut os = fixture.machine();
    let mut expansion = RunExpansion::new(agent.clone(), RunId::new("run-viewer"));
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &RunId::new("run-viewer"));
    let op = operation("run-viewer");
    for (run, _ending, expected) in cases {
        let segment = scope.execute(&agent, &op, &read(&format!("{HISTORY}/{run}/000.txt")));
        assert_eq!(segment.state(), ResultState::Success);
        let text = text_of(&segment);
        assert!(
            text.contains("ending = system:model-failed\n"),
            "{run}: {text}"
        );
        assert!(
            text.contains(expected),
            "{run}: 细分必须逐字出现在段首（期望 {expected:?}）: {text}"
        );
    }
}
