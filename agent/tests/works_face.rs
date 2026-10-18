//! **主体自己的作品面**（`<环境根>/projects`，2026-10-14）。
//!
//! 判据一句话：**她自己的东西不用申请**（与 `notes_*` 同族）——因此这一格**不需要任何声明**，
//! 每一支都在场；而它的边界是**构造性的**：六件动词只认那一个目录，出不去。
//!
//! 这一份离线证明钉住五件事：
//!
//! 1. **装配期缺席才建**：`<环境根>/projects` 不在就建一个**空目录**，在场则一个字节都不动；
//! 2. **写进去、读回来**：`projects_create` 落的是真文件（盘上那一个），`projects_read` 逐字读回，
//!    `projects_list` 列得出——**写完立刻核实**这条路是通的；
//! 3. **两个面互不串**：资料面的 `files_*` 看不见她的作品，作品面的 `projects_*` 看不见资料；
//!    同名文件各在各的目录里，谁也不动谁；
//! 4. **出不去**：`..`、盘符、绝对路径、带分隔符的名字一律 `invalid-path`（与资料面同一套墙）；
//! 5. **三态不许压成两态**：那个名字底下打不开（是文件 / 链接）⇒ **在场但打不开**（如实报分类），
//!    不是"这个地址不在"；而**没有环境根**的装配里，她那几件动词连声明都不出现。

use std::fs;
use std::path::PathBuf;

use celira_agent::app_contract::AppPort;
use celira_agent::core::{Action, ActionId, ActionOutcome, ActionParams, ActionPort};
use celira_agent::file_app::FILE_APP_ID;
use celira_agent::file_view::{FileView, WORKS_DIR};
use celira_agent::foundation::{AgentId, OperationId, ResultState, RunId, Turn};
use celira_agent::os::{MinimalOs, RunExpansion};
use celira_agent::store::{Environment, TracePayload};

const EXPAND: &str = "open_app";
const LIST: &str = "projects_list";
const READ: &str = "projects_read";
const CREATE: &str = "projects_create";
const VERIFY_TEXT: &str = "projects_verify_text";
const EDIT: &str = "projects_edit";

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let unique = format!(
            "celira-works-face-{}-{}-{}",
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

    fn works(&self) -> PathBuf {
        self.environment_root().join(WORKS_DIR)
    }

    fn write_material(&self, relative: &str, text: &str) {
        let path = self.materials().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("建子目录");
        }
        fs::write(path, text).expect("写合成资料");
    }

    fn agent() -> AgentId {
        AgentId::new("agent-works")
    }

    fn environment(&self) -> Environment {
        Environment::create(&self.environment_root(), Self::agent()).expect("建立身份环境")
    }

    fn machine(&self) -> MinimalOs {
        MinimalOs::with_history(&self.materials(), &self.environment_root())
            .expect("装配只读文件视图（作品面随之在场）")
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

fn action(id: &str, params: ActionParams) -> Action {
    Action::with_params(ActionId::new(id), params)
}

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

fn failure_of(outcome: &ActionOutcome) -> String {
    outcome.failure_class().unwrap_or("<none>").to_string()
}

/// 1. 作品面在每一支上都在场，而且装配期**缺席才建**一个空目录。
#[test]
fn the_works_face_is_always_there_and_the_directory_is_created_when_absent() {
    let fixture = Fixture::new("presence");
    let mut env = fixture.environment();
    env.append_trace(
        &RunId::new("run-a"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("追加合成经历");
    assert!(!fixture.works().exists(), "前提：装配之前她还没有作品目录");

    let mut os = fixture.machine();
    assert!(
        fixture.works().is_dir(),
        "装配期**缺席才建**：装配之后那个目录在，而且是目录"
    );

    let agent = Fixture::agent();
    let run = RunId::new("run-a");
    let mut expansion = RunExpansion::new(Fixture::agent(), RunId::new("run-a"));
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);

    let outcome = scope.execute(
        &agent,
        &operation("run-a"),
        &action(LIST, ActionParams::none()),
    );
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    assert!(
        text_of(&outcome).contains("entries = 0"),
        "刚建好的作品目录是**空的**（不是缺席）：{}",
        text_of(&outcome)
    );
}

/// 2. 写进去、读回来：落的是盘上那个真文件，正文逐字读得回来。
#[test]
fn what_she_writes_lands_on_disk_in_her_own_directory_and_reads_back() {
    let fixture = Fixture::new("write-read");
    let mut env = fixture.environment();
    env.append_trace(
        &RunId::new("run-a"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("追加合成经历");

    let mut os = fixture.machine();
    let agent = Fixture::agent();
    let run = RunId::new("run-a");
    let mut expansion = RunExpansion::new(Fixture::agent(), RunId::new("run-a"));
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);

    let text = "print('hello from her own place')\n";
    let outcome = scope.execute(
        &agent,
        &operation("run-a"),
        &action(
            CREATE,
            ActionParams::none()
                .with("name", "hello.py")
                .with("text", text),
        ),
    );
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    assert_eq!(
        fs::read_to_string(fixture.works().join("hello.py")).expect("盘上那一份"),
        text,
        "落的是真文件，字节逐字相同"
    );
    assert!(
        !fixture.materials().join("hello.py").exists(),
        "它**不在资料面**里——两个面各在各的目录"
    );

    let outcome = scope.execute(
        &agent,
        &operation("run-a"),
        &action(READ, ActionParams::none().with("path", "hello.py")),
    );
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    assert!(text_of(&outcome).contains("hello from her own place"));

    // 写类别必须配**同参复核**，而复核问的是"现在这份参数对应的状态"。
    let outcome = scope.execute(
        &agent,
        &operation("run-a"),
        &action(
            VERIFY_TEXT,
            ActionParams::none()
                .with("name", "hello.py")
                .with("text", text),
        ),
    );
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );

    // 同名再来一次：明确失败，原名下一个字节都不动。
    let outcome = scope.execute(
        &agent,
        &operation("run-a"),
        &action(
            CREATE,
            ActionParams::none()
                .with("name", "hello.py")
                .with("text", "别的正文"),
        ),
    );
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(failure_of(&outcome), "already-exists");
    assert_eq!(
        fs::read_to_string(fixture.works().join("hello.py")).expect("盘上那一份"),
        text,
        "同名失败之后原名下的内容一个字节都没变"
    );
}

/// 3. 改：唯一一处替换走得通，0 次与多次都**明确失败且一个字节不动**。
#[test]
fn edit_replaces_the_only_occurrence_and_refuses_missing_or_ambiguous() {
    let fixture = Fixture::new("edit");
    let mut env = fixture.environment();
    env.append_trace(
        &RunId::new("run-a"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("追加合成经历");

    let mut os = fixture.machine();
    let agent = Fixture::agent();
    let run = RunId::new("run-a");
    let mut expansion = RunExpansion::new(Fixture::agent(), RunId::new("run-a"));
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);

    let outcome = scope.execute(
        &agent,
        &operation("run-a"),
        &action(
            CREATE,
            ActionParams::none()
                .with("name", "note.txt")
                .with("text", "标题：甲\n正文：乙\n"),
        ),
    );
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );

    let outcome = scope.execute(
        &agent,
        &operation("run-a"),
        &action(
            EDIT,
            ActionParams::none()
                .with("path", "note.txt")
                .with("old_text", "乙")
                .with("new_text", "丙"),
        ),
    );
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    assert_eq!(
        fs::read_to_string(fixture.works().join("note.txt")).expect("盘上那一份"),
        "标题：甲\n正文：丙\n"
    );

    // 0 次 ⇒ not-found（一个字节不动）。
    let outcome = scope.execute(
        &agent,
        &operation("run-a"),
        &action(
            EDIT,
            ActionParams::none()
                .with("path", "note.txt")
                .with("old_text", "不在的片段")
                .with("new_text", "x"),
        ),
    );
    assert_eq!(failure_of(&outcome), "not-found");

    // 多次 ⇒ invalid-argument（不猜"你想改哪一处"），盘上照旧。
    let outcome = scope.execute(
        &agent,
        &operation("run-a"),
        &action(
            CREATE,
            ActionParams::none()
                .with("name", "twice.txt")
                .with("text", "甲甲"),
        ),
    );
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    let outcome = scope.execute(
        &agent,
        &operation("run-a"),
        &action(
            EDIT,
            ActionParams::none()
                .with("path", "twice.txt")
                .with("old_text", "甲")
                .with("new_text", "乙"),
        ),
    );
    assert_eq!(failure_of(&outcome), "invalid-argument");
    assert_eq!(
        fs::read_to_string(fixture.works().join("twice.txt")).expect("盘上那一份"),
        "甲甲"
    );
}

/// 4. **出不去**：越界路径与带分隔符的名字一律拒绝（同一套墙，构造性的）。
#[test]
fn paths_cannot_escape_her_own_directory() {
    let fixture = Fixture::new("escape");
    fixture.write_material("materials-only.txt", "资料");
    let mut env = fixture.environment();
    env.append_trace(
        &RunId::new("run-a"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("追加合成经历");
    // 环境根里有一份"机器与人的记录"：作品面**够不到**它。
    fs::write(
        fixture.environment_root().join("agent.conf"),
        "terminal=none\n",
    )
    .expect("摆一份支级配置");

    let mut os = fixture.machine();
    let agent = Fixture::agent();
    let run = RunId::new("run-a");
    let mut expansion = RunExpansion::new(Fixture::agent(), RunId::new("run-a"));
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);

    for escape in [
        "../agent.conf",
        "..\\agent.conf",
        "C:/Windows/win.ini",
        "/etc/passwd",
    ] {
        let outcome = scope.execute(
            &agent,
            &operation("run-a"),
            &action(READ, ActionParams::none().with("path", escape)),
        );
        assert_eq!(
            failure_of(&outcome),
            "invalid-path",
            "越界路径必须拒绝：{escape}"
        );
    }
    let outcome = scope.execute(
        &agent,
        &operation("run-a"),
        &action(
            CREATE,
            ActionParams::none()
                .with("name", "sub/boom.txt")
                .with("text", "x"),
        ),
    );
    assert_eq!(
        failure_of(&outcome),
        "invalid-path",
        "创建只收**单层**文件名"
    );

    // 两个面互不串：资料面的文件在作品面里读不到，反之亦然。
    let outcome = scope.execute(
        &agent,
        &operation("run-a"),
        &action(
            READ,
            ActionParams::none().with("path", "materials-only.txt"),
        ),
    );
    assert_eq!(failure_of(&outcome), "not-found");
}

/// 5. **三态不许压成两态**：那个名字底下打不开 ⇒ 如实报分类，不是"地址不在"；
///    没有环境根的装配里，她那几件动词**连声明都不出现**。
#[test]
fn an_unopenable_works_directory_is_unusable_not_absent() {
    let fixture = Fixture::new("unusable");
    let mut env = fixture.environment();
    env.append_trace(
        &RunId::new("run-a"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("追加合成经历");
    // 那个名字底下放一个**文件**（不是目录）：装配照常成功，那一格如实报"打不开"。
    fs::write(fixture.works(), "我不是目录").expect("用文件占住那个名字");

    let mut os = fixture.machine();
    let agent = Fixture::agent();
    let run = RunId::new("run-a");
    let mut expansion = RunExpansion::new(Fixture::agent(), RunId::new("run-a"));
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);

    let outcome = scope.execute(
        &agent,
        &operation("run-a"),
        &action(LIST, ActionParams::none()),
    );
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(
        failure_of(&outcome),
        "wrong-kind",
        "**打不开**就是打不开：不冒充'这个地址不在'（那是 not-found）"
    );

    // 没有环境根的装配（`materials_only`）：作品面那一格**不在场** ⇒ 动词连声明都没有。
    let view = FileView::materials_only(&fixture.materials()).expect("只装资料面");
    let ids: Vec<String> = view
        .declared_capabilities()
        .items()
        .iter()
        .map(|capability| capability.id().as_str().to_string())
        .collect();
    assert!(
        !ids.iter().any(|id| id.starts_with("projects_")),
        "没有环境根就没有作品面：{ids:?}"
    );
}
