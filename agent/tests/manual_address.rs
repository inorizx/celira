//! 只读文件视图的**第二个派生地址**：`manual.md`（TASK_79 刀五）。
//!
//! 出厂说明随系统消息进来，**手册得自己去拉**——它落在环境根（不是资料根），而主体的只读命名
//! 空间是"资料根 ＋ 派生地址"。这一刀给主体一个**保留的单文件逻辑地址**，背后就是环境根里那一份，
//! 用同一套通用 `files_list` / `files_read` 访问。
//!
//! 这一份离线证明钉住五件事：
//!
//! 1. **地址随那一份在不在**：环境根里有手册 ⇒ 根列表里出现 `manual.md`、读它逐字返回；没有 ⇒
//!    列表里不出现、读它报 `not-found`（**缺席不是错误**，也不是空文件）；
//! 2. **它是一份文件不是目录**：列它 / 往它底下走都报 `wrong-kind`（与真实资料面同一条结论）；
//! 3. **读取规则与资料面同源**：整份读、**超限拒绝不截断**、窗口参数明确失败（它是自成一整份的
//!    派生地址，没有"某一段"这回事）；主体改了自己那一份，下一次读到的就是新的；
//! 4. **装配期的保留地址守卫覆盖它**：资料根里已有同名真实条目 ⇒ 拒绝装配（`reserved-address-taken`）
//!    ——但只在那个地址**真的在**的时候（地址不在就无从被遮蔽）；
//! 5. **经历那条地址一行没动**：同一个视图里 `history/` 的列与读照旧。

use std::fs;
use std::path::PathBuf;

use celira_agent::app_contract::AppPort;
use celira_agent::core::{Action, ActionId, ActionOutcome, ActionParams, ActionPort};
use celira_agent::file_app::{FILE_APP_ID, MAX_TEXT_BYTES};
use celira_agent::file_view::{FileView, FileViewFailure, HISTORY_DIR, MANUAL_ADDRESS};
use celira_agent::foundation::{
    AgentId, OperationId, Outcome, ResultState, RunId, SubjectEnding, Turn,
};
use celira_agent::os::{MinimalOs, RunExpansion};
use celira_agent::store::{Environment, TracePayload, ending_payload};

const EXPAND: &str = "open_app";
const LIST: &str = "files_list";
const READ: &str = "files_read";
const FIND: &str = "files_grep";

/// 自建临时根：`root/env` 是身份环境，`root/materials` 是授权资料目录。
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let unique = format!(
            "celira-manual-address-{}-{}-{}",
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
        AgentId::new("agent-manual")
    }

    /// 建身份环境（不写任何经历：这一份证明与经历无关）。
    fn environment(&self) -> Environment {
        Environment::create(&self.environment_root(), Self::agent()).expect("建立身份环境")
    }

    /// 把手册摆进环境根里（**环境根里那一份**，不是资料目录里的同名文件）。
    fn lay_manual(&self, text: &str) {
        fs::write(self.environment_root().join(MANUAL_ADDRESS), text).expect("摆下手册");
    }

    /// 用同一个只读文件视图装配这台 OS（资料目录 + 派生地址）。
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

/// 带窗口参数的读（`offset` / `limit` 各给一个）。
fn read_window(path: &str, offset: &str, limit: &str) -> Action {
    Action::with_params(
        ActionId::new(READ),
        ActionParams::none()
            .with("path", path)
            .with("offset", offset)
            .with("limit", limit),
    )
}

/// 按子串查找（`path` 可限定子树）。
fn find(pattern: &str, path: Option<&str>) -> Action {
    let mut params = ActionParams::none().with("pattern", pattern);
    if let Some(path) = path {
        params = params.with("path", path);
    }
    Action::with_params(ActionId::new(FIND), params)
}

/// 一条「活动开始 + 输入 + 收场 + 封存」的合成经历（经历那一半照旧要有东西可读）。
fn complete_run(text: &str) -> Vec<TracePayload> {
    vec![
        TracePayload::RunStarted { origin: None },
        TracePayload::RunInput {
            text: text.to_string(),
        },
        ending_payload(Outcome::Subject(SubjectEnding::Completed)),
        TracePayload::Sealed,
    ]
}

/// 展开视图 App 并返回它这一次活动的作用域。
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

/// 根列表里那些**条目名**（跳掉首行的机械读数）。
fn entry_names(listing: &str) -> Vec<String> {
    listing
        .lines()
        .skip(1)
        .filter_map(|line| line.split('\t').next())
        .map(str::to_string)
        .collect()
}

/// 1. 地址随那一份在不在：有 ⇒ 列得出来、读得到逐字正文；没有 ⇒ 缺席（`not-found`，不是空）。
#[test]
fn the_address_exists_only_while_she_has_that_document() {
    let fixture = Fixture::new("presence");
    fixture.write_material("alpha.txt", "资料正文");
    let mut env = fixture.environment();
    env.append_trace(
        &RunId::new("run-a"),
        TracePayload::RunStarted { origin: None },
    )
    .expect("追加合成经历");
    fixture.lay_manual("手册正文：想给自己做事的时候看这里。\n");

    let agent = Fixture::agent();
    let run = RunId::new("run-viewer");
    let op = operation("run-viewer");

    // 有那一份：根列表里出现这个名字，而且**是文件不是目录**。
    {
        let mut os = fixture.machine();
        let mut expansion = RunExpansion::new(agent.clone(), run.clone());
        let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);

        let root = text_of(&scope.execute(&agent, &op, &list("")));
        assert!(root.contains("manual.md\tfile"), "{root}");
        assert_eq!(
            entry_names(&root),
            vec![
                "alpha.txt".to_string(),
                HISTORY_DIR.to_string(),
                MANUAL_ADDRESS.to_string()
            ],
            "根列表 = 真实条目 + 两个保留地址（按名字排序）"
        );

        // 读它：逐字返回**自己家里那一份**的正文（不经任何再渲染）。
        let manual = scope.execute(&agent, &op, &read(MANUAL_ADDRESS));
        assert_eq!(manual.state(), ResultState::Success);
        assert_eq!(text_of(&manual), "手册正文：想给自己做事的时候看这里。\n");
    }

    // 主体摘掉那一份（那是主体的地盘）：重新装配之后**地址就不在了**——列表里没有它，
    // 读它报 `not-found`（缺席不是错误，也不是"空文件"）。
    fs::remove_file(fixture.environment_root().join(MANUAL_ADDRESS)).expect("摘掉那一份");
    let mut os = fixture.machine();
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    let root = text_of(&scope.execute(&agent, &op, &list("")));
    assert!(!root.contains(MANUAL_ADDRESS), "地址不在就不许出现: {root}");
    let missing = scope.execute(&agent, &op, &read(MANUAL_ADDRESS));
    assert_eq!(missing.state(), ResultState::Failure);
    assert_eq!(missing.failure_class(), Some("not-found"));
    assert!(missing.content().is_empty(), "失败不得夹带内容");
}

/// 2. 它是一份**文件**：列它、往它底下走都是类型不对（与真实资料面同一条结论）。
#[test]
fn the_manual_is_a_file_and_never_pretends_to_be_a_directory() {
    let fixture = Fixture::new("shape");
    let _env = fixture.environment();
    fixture.lay_manual("手册。\n");

    let agent = Fixture::agent();
    let run = RunId::new("run-shape");
    let mut os = fixture.machine();
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    let op = operation("run-shape");

    for action in [
        list(MANUAL_ADDRESS),
        list(&format!("{MANUAL_ADDRESS}/deeper")),
        read(&format!("{MANUAL_ADDRESS}/deeper")),
    ] {
        let outcome = scope.execute(&agent, &op, &action);
        assert_eq!(
            outcome.failure_class(),
            Some("wrong-kind"),
            "它是一份文件：{action:?}"
        );
    }
}

/// 3. 读取规则**与资料面同源**，而不是"派生地址另有一套"。
#[test]
fn the_manual_obeys_the_same_read_rules_as_the_materials_face() {
    let fixture = Fixture::new("same-rules");
    let _env = fixture.environment();
    fixture.lay_manual("第一版手册。\n");

    let agent = Fixture::agent();
    let run = RunId::new("run-rules");
    let mut os = fixture.machine();
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    let op = operation("run-rules");

    // ① 主体改了自己那一份 ⇒ 下一次读到的就是新的（派生地址**不缓存**正文）。
    let first = text_of(&scope.execute(&agent, &op, &read(MANUAL_ADDRESS)));
    assert_eq!(first, "第一版手册。\n");
    fixture.lay_manual("第二版手册（自己加的）。\n");
    let second = text_of(&scope.execute(&agent, &op, &read(MANUAL_ADDRESS)));
    assert_eq!(second, "第二版手册（自己加的）。\n");

    // ② 窗口参数：它是自成一整份的派生地址（没有"某一段"这回事）⇒ 明确失败，不静默忽略。
    for offset in ["0", "1"] {
        let outcome = scope.execute(&agent, &op, &read_window(MANUAL_ADDRESS, offset, "10"));
        assert_eq!(
            outcome.failure_class(),
            Some("invalid-argument"),
            "{offset}"
        );
        assert!(outcome.content().is_empty());
    }

    // ③ 超限：与资料面**同一条实现** ⇒ 整份拒绝、**一个字节都不截**。
    let oversized = "x".repeat(MAX_TEXT_BYTES as usize + 1);
    fixture.lay_manual(&oversized);
    let outcome = scope.execute(&agent, &op, &read(MANUAL_ADDRESS));
    assert_eq!(outcome.failure_class(), Some("too-large"));
    assert!(outcome.content().is_empty(), "超限不许给半份");
}

/// 4. 装配期守卫：**现存的**保留地址被资料面里同名条目遮蔽 ⇒ 拒绝装配。
///
/// 两条对照：地址在 ⇒ 拒绝；地址不在（环境根里没有那一份）⇒ 同名资料**无关**，照旧装配。
#[test]
fn a_material_shadowing_the_manual_address_is_refused_only_while_it_exists() {
    let fixture = Fixture::new("reserved");
    fixture.write_material(MANUAL_ADDRESS, "资料目录里的同名文件");
    let environment_root = fixture.environment_root();

    // 地址不在：资料面里有同名文件不影响装配（那个地址压根不存在）。
    Environment::create(&environment_root, Fixture::agent()).expect("建立身份环境");
    assert!(
        FileView::with_history(&fixture.materials(), &environment_root).is_ok(),
        "地址不在时同名资料不是冲突"
    );

    // 地址在：同名条目会让这个逻辑地址无法被唯一解释 ⇒ 装配期拒绝。
    fixture.lay_manual("手册。\n");
    assert_eq!(
        FileView::with_history(&fixture.materials(), &environment_root).unwrap_err(),
        FileViewFailure::ReservedAddressTaken,
        "保留地址被同名真实条目遮蔽时必须明确拒绝装配"
    );
    assert_eq!(
        FileViewFailure::ReservedAddressTaken.as_str(),
        "reserved-address-taken"
    );
}

/// 5. 手册不在本动词的**搜索面**内（查找只在资料面）；地址在不在决定"拒绝"还是"没有这个路径"。
#[test]
fn the_search_face_stays_off_the_manual() {
    let fixture = Fixture::new("search");
    fixture.write_material("alpha.txt", "资料里没有那个串\n");
    let _env = fixture.environment();
    fixture.lay_manual("手册里写着 手册暗号。\n");

    let agent = Fixture::agent();
    let run = RunId::new("run-search");
    let mut os = fixture.machine();
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    let op = operation("run-search");

    // 手册的正文不进搜索面（它是一份文件，用 `files_read` 看）。
    let searched = text_of(&scope.execute(&agent, &op, &find("手册暗号", None)));
    assert_eq!(
        searched.lines().next(),
        Some("candidates = 0, scanned = 1, skipped = 0, truncated = false"),
        "{searched}"
    );
    assert!(!searched.contains("手册暗号"), "手册正文不在搜索面内");

    // 把手册当查找子树：它是保留地址 ⇒ 明确拒绝（不是"不存在"）。
    let outcome = scope.execute(&agent, &op, &find("手册暗号", Some(MANUAL_ADDRESS)));
    assert_eq!(
        outcome.failure_class(),
        Some("invalid-argument"),
        "保留地址不在查找面内"
    );

    // 地址不在时：同一个请求按既有规则报 `not-found`（与 `files_list` / `files_read` 同款）。
    fs::remove_file(fixture.environment_root().join(MANUAL_ADDRESS)).expect("摘掉那一份");
    let mut os = fixture.machine();
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    let outcome = scope.execute(&agent, &op, &find("手册暗号", Some(MANUAL_ADDRESS)));
    assert_eq!(outcome.failure_class(), Some("not-found"));
}

/// 6. **经历那条地址一行没动**：同一个视图里 `history/` 的列与读照旧（第二处地址不重构派生层）。
#[test]
fn the_history_address_keeps_its_old_semantics_beside_the_new_one() {
    let fixture = Fixture::new("history-untouched");
    fixture.write_material("alpha.txt", "资料正文");
    let mut env = fixture.environment();
    for payload in complete_run("第一段中文经历") {
        env.append_trace(&RunId::new("run-a"), payload)
            .expect("追加合成经历");
    }
    fixture.lay_manual("手册。\n");

    let agent = Fixture::agent();
    let run = RunId::new("run-history");
    let mut os = fixture.machine();
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    let op = operation("run-history");

    // 列经历目录：一个活动一个逻辑子目录（照旧）。
    let runs = text_of(&scope.execute(&agent, &op, &list(HISTORY_DIR)));
    assert_eq!(
        runs.lines().next(),
        Some("entries = 1, truncated = false, more = none"),
        "{runs}"
    );
    assert_eq!(entry_names(&runs), vec!["run-a".to_string()]);

    // 读一段经历：段首是机械事实行（照旧）。
    let segment =
        text_of(&scope.execute(&agent, &op, &read(&format!("{HISTORY_DIR}/run-a/000.txt"))));
    assert!(
        segment.lines().any(|line| line.starts_with("run = run-a")),
        "{segment}"
    );

    // 经历同款：对目录读文本是类型不对；对它给窗口参数明确失败。
    assert_eq!(
        scope
            .execute(&agent, &op, &read(HISTORY_DIR))
            .failure_class(),
        Some("wrong-kind")
    );
    assert_eq!(
        scope
            .execute(
                &agent,
                &op,
                &read_window(&format!("{HISTORY_DIR}/run-a/000.txt"), "0", "10")
            )
            .failure_class(),
        Some("invalid-argument")
    );

    // 手册与经历**同处一个根列表**里，各自是各自的样子。
    let root = text_of(&scope.execute(&agent, &op, &list("")));
    assert_eq!(
        entry_names(&root),
        vec![
            "alpha.txt".to_string(),
            HISTORY_DIR.to_string(),
            MANUAL_ADDRESS.to_string()
        ],
        "{root}"
    );
}

/// 7. 这条地址**只读**：写动词落不到它身上（这个视图没有创建授权），而且那一份一个字节没动。
#[test]
fn nothing_writable_lands_on_the_manual() {
    let fixture = Fixture::new("read-only");
    let _env = fixture.environment();
    fixture.lay_manual("手册。\n");

    let agent = Fixture::agent();
    let run = RunId::new("run-readonly");
    let mut os = fixture.machine();
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = expanded_scope(&mut os, &mut expansion, &agent, &run);
    let op = operation("run-readonly");

    // 没有创建授权 ⇒ 两个写动词连能力都不存在：归属索引里找不到主人 ⇒ `unknown-action`。
    for capability in ["files_create", "files_edit"] {
        let action = Action::with_params(
            ActionId::new(capability),
            ActionParams::none()
                .with("path", MANUAL_ADDRESS)
                .with("text", "试图改写"),
        );
        let outcome = scope.execute(&agent, &op, &action);
        assert_eq!(
            outcome.failure_class(),
            Some("unknown-action"),
            "{capability} 不该落在这条地址上"
        );
    }
    assert_eq!(
        fs::read_to_string(fixture.environment_root().join(MANUAL_ADDRESS)).expect("读回那一份"),
        "手册。\n",
        "那一份必须原样"
    );
}

/// 8. 能力面**没有多也没有少**：仍是三项只读动词（派生地址不是新动作），而描述里两个地址都在。
#[test]
fn the_derived_address_adds_no_new_capability() {
    let fixture = Fixture::new("capabilities");
    let environment_root = fixture.environment_root();
    Environment::create(&environment_root, Fixture::agent()).expect("建立身份环境");

    let declared: Vec<String> = FileView::capabilities()
        .items()
        .iter()
        .map(|capability| capability.id().as_str().to_string())
        .collect();
    assert_eq!(declared, vec![LIST, READ, FIND]);

    // 视图的描述里两个保留地址都在（主体看得见"这里还有一份手册"）。
    let view = FileView::with_history(&fixture.materials(), &environment_root).expect("装配视图");
    let description = view.description().to_string();
    assert!(description.contains(HISTORY_DIR), "{description}");
    assert!(description.contains(MANUAL_ADDRESS), "{description}");
}
