//! 「回得来」那一刀的验收（2026-10-11）：**一件要活到关家的事**——起它、看它、读它的输出、停它。
//!
//! 这一格的形状是"账只追加、版面是折出来的"（`<环境根>/terminal` 一行一条 JSON，与
//! `<环境根>/timers` 同款）。判据分两层：
//!
//! - **账本模块**（平台无关、纯逻辑）：编解码、词表封闭、折版面、名字校验；
//! - **动词面**（真执行，`#[cfg(any(windows, target_os = "linux"))]` 门控）：起一件真的活着的东西、
//!   看它、停它，并**确认它真的没了**——三态（`alive` / `gone` / `unknown`）每一态都要有读数。
//!
//! ⚠ 每条起了东西的用例**自己收尾**（不留孤儿）；起的那条命令都是短寿命的（几十秒），
//! 这样万一用例中途失败，泄漏也是自限的。

use std::path::PathBuf;

use celira_agent::core::{Action, ActionId, ActionOutcome, ActionParams, ActionPort};
use celira_agent::foundation::{AgentId, OperationId, RunId, Turn};
use celira_agent::terminal_app::{
    CAPABILITY_HELD, CAPABILITY_RUN, CAPABILITY_STOP, TerminalAccess, TerminalApp, TerminalTier,
};
use celira_agent::terminal_held::{
    Held, HeldEntry, HeldThings, KIND_START, KIND_STOP, MAX_NAME_BYTES, name_problem,
};

fn agent() -> AgentId {
    AgentId::new("agent-held")
}

fn operation() -> OperationId {
    OperationId::new(RunId::new("hand"), Turn::FIRST)
}

fn run_action(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_RUN), params)
}

fn held_action() -> Action {
    Action::with_params(ActionId::new(CAPABILITY_HELD), ActionParams::none())
}

fn stop_action(name: &str) -> Action {
    Action::with_params(
        ActionId::new(CAPABILITY_STOP),
        ActionParams::none().with("name", name),
    )
}

fn call(app: &mut TerminalApp, action: &Action) -> ActionOutcome {
    app.execute(&agent(), &operation(), action)
}

fn text_of(outcome: &ActionOutcome) -> String {
    outcome
        .content()
        .iter()
        .map(|block| block.text().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn failure_of(outcome: &ActionOutcome) -> String {
    outcome.failure_class().unwrap_or("<none>").to_string()
}

/// 一次性的环境根（用例自己收尾；目录名带进程号与标签，彼此不撞）。
fn fresh_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("celira-held-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("建得起临时环境根");
    root
}

fn app_on(root: &std::path::Path) -> TerminalApp {
    TerminalApp::new(TerminalAccess::new(TerminalTier::Full, Vec::new()), root)
}

/// 一条**会活着几十秒、并且先说一句话**的命令（两条臂各一份，同一批断言）。
#[cfg(windows)]
const KEEPABLE: &str = "Write-Output ready; Start-Sleep -Seconds 30";
#[cfg(target_os = "linux")]
const KEEPABLE: &str = "echo ready; sleep 30";

/// 一条**活着但一句话都不说**的命令（量"还没说话"与"读不到"分不分得开）。
#[cfg(windows)]
const SILENT: &str = "Start-Sleep -Seconds 30";
#[cfg(target_os = "linux")]
const SILENT: &str = "sleep 30";

// ---------------------------------------------------------------------------
// 账本模块（平台无关）
// ---------------------------------------------------------------------------

fn line(kind: &str, name: &str, at: u64) -> HeldEntry {
    HeldEntry {
        name: name.to_string(),
        command: String::new(),
        pid: 0,
        log: String::new(),
        at_millis: at,
        kind: kind.to_string(),
    }
}

#[test]
fn the_ledger_folds_starts_and_stops_into_what_is_left() {
    let held = Held::fold(&[
        line(KIND_START, "a", 10),
        line(KIND_START, "b", 20),
        line(KIND_STOP, "a", 30),
    ]);
    assert_eq!(
        held.entries
            .iter()
            .map(|each| each.name.as_str())
            .collect::<Vec<_>>(),
        vec!["b"],
        "停掉的从版面上走，还留着的按起的先后排"
    );
    assert_eq!(held.stopped, 1, "停过几条要如实报（不静默丢）");
}

#[test]
fn the_same_name_taken_again_replaces_the_older_one() {
    // 判据：**一个名字在任何时刻只指一件事**——否则停它就有歧义。
    let held = Held::fold(&[line(KIND_START, "a", 10), line(KIND_START, "a", 20)]);
    assert_eq!(held.entries.len(), 1);
    assert_eq!(held.entries[0].at_millis, 20, "留下的是后起的那一件");
}

#[test]
fn a_ledger_that_is_not_there_is_empty_not_an_error() {
    let root = fresh_root("empty");
    let things = HeldThings::new(&root);
    assert!(things.lines().expect("没账 = 空账").is_empty());
    assert_eq!(things.held().expect("折得出空版面").entries.len(), 0);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_row_with_an_unknown_kind_or_a_broken_shape_makes_the_ledger_unreadable() {
    let root = fresh_root("broken");
    let things = HeldThings::new(&root);
    // 词表是封闭的：认不出的 `kind` **明确拒绝**，不就近猜。
    std::fs::write(
        things.path(),
        "{\"kind\":\"fly\",\"name\":\"a\",\"at\":1}\n",
    )
    .unwrap();
    assert!(things.lines().is_err(), "认不出的取值不许当默认值读过去");
    // 形状不符同款（少一个必填键）。
    std::fs::write(things.path(), "{\"kind\":\"start\",\"name\":\"a\"}\n").unwrap();
    assert!(things.lines().is_err(), "缺 `at` 这一行读不懂");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn appending_keeps_every_earlier_line() {
    // 判据 22「账只追加」：追加 stop 之后，start 那一行**照旧在盘上**。
    let root = fresh_root("append");
    let things = HeldThings::new(&root);
    things
        .append_start("a", "echo hi", 42, "log.txt", 10)
        .unwrap();
    things.append_stop("a", 20).unwrap();
    let raw = std::fs::read_to_string(things.path()).unwrap();
    assert_eq!(raw.lines().count(), 2, "两行都在（stop 不改写 start）");
    assert!(raw.contains("\"kind\":\"start\"") && raw.contains("\"kind\":\"stop\""));
    let held = things.held().unwrap();
    assert!(held.entries.is_empty(), "版面上它已经走了");
    assert_eq!(held.stopped, 1);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn names_that_cannot_become_a_file_name_are_refused() {
    assert!(name_problem("dev-server").is_none());
    assert_eq!(name_problem("   "), Some("empty"));
    assert_eq!(name_problem("../escape"), Some("not-a-name"));
    assert_eq!(name_problem("a\\b"), Some("not-a-name"));
    assert_eq!(name_problem("c:name"), Some("not-a-name"));
    assert_eq!(
        name_problem(&"x".repeat(MAX_NAME_BYTES + 1)),
        Some("too-long")
    );
}

// ---------------------------------------------------------------------------
// 动词面（真执行）
// ---------------------------------------------------------------------------

#[test]
fn an_empty_ledger_reads_as_zero_not_unknown() {
    // 缺省不等于零：**读成了、一条也没有** ⇒ `held = 0`；读不出来 ⇒ `held = unknown`。
    let root = fresh_root("read-zero");
    let mut app = app_on(&root);
    let outcome = call(&mut app, &held_action());
    let text = text_of(&outcome);
    assert!(text.contains("held = 0"), "空账是 0：{text}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_unreadable_ledger_reads_as_unknown_not_zero() {
    let root = fresh_root("read-unknown");
    let things = HeldThings::new(&root);
    std::fs::write(things.path(), "这不是一行 JSON\n").unwrap();
    let mut app = app_on(&root);
    let text = text_of(&call(&mut app, &held_action()));
    assert!(
        text.contains("held = unknown"),
        "读不出来是 unknown，**不许说成 0**：{text}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_keep_value_that_is_not_one_of_the_two_words_is_refused() {
    let root = fresh_root("keep-word");
    let mut app = app_on(&root);
    let outcome = call(
        &mut app,
        &run_action(
            ActionParams::none()
                .with("command", "echo hi")
                .with("keep", "maybe"),
        ),
    );
    assert_eq!(
        failure_of(&outcome),
        "invalid-argument",
        "只认 true / false"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_name_that_cannot_become_a_file_name_is_refused_by_the_verb() {
    let root = fresh_root("keep-name");
    let mut app = app_on(&root);
    let outcome = call(
        &mut app,
        &run_action(
            ActionParams::none()
                .with("command", "echo hi")
                .with("keep", "true")
                .with("name", "../escape"),
        ),
    );
    assert_eq!(failure_of(&outcome), "invalid-argument");
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn keep_starts_it_records_it_and_writes_its_output_into_a_file() {
    let root = fresh_root("keep-start");
    let mut app = app_on(&root);
    let outcome = call(
        &mut app,
        &run_action(
            ActionParams::none()
                .with("command", KEEPABLE)
                .with("keep", "true")
                .with("name", "server"),
        ),
    );
    let text = text_of(&outcome);
    assert_eq!(failure_of(&outcome), "<none>", "起这一下不该失败：{text}");
    assert!(text.contains("held = server"), "回执给名字：{text}");
    assert!(text.contains("pid = "), "回执给 pid：{text}");
    assert!(text.contains("output = "), "回执给输出路径：{text}");

    // 账上有一行 `start`，输出文件真的在那个路径上（它自己说的话落进去了）。
    let things = HeldThings::new(&root);
    let lines = things.lines().expect("账读得回来");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].kind, KIND_START);
    assert_eq!(lines[0].name, "server");
    assert_eq!(lines[0].command, KEEPABLE);
    assert!(lines[0].pid > 0, "记下的 pid 要是一个真的进程");
    let log = things.log_path("server");
    let mut seen = String::new();
    for _ in 0..50 {
        seen = std::fs::read_to_string(&log).unwrap_or_default();
        if seen.contains("ready") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        seen.contains("ready"),
        "那件事吐出来的话必须落在她读得回来的地方（读到：{seen:?}）"
    );

    // 看它：账上那条 + 三态里的 `alive` + **它最近说的那几行**。
    let reading = text_of(&call(&mut app, &held_action()));
    assert!(reading.contains("held = 1"), "{reading}");
    assert!(reading.contains("server"), "{reading}");
    assert!(reading.contains("state = alive"), "它真的在跑：{reading}");
    assert!(
        reading.contains("tail = "),
        "读数要带尾部几行（省她一轮）：{reading}"
    );
    assert!(
        reading.contains("ready"),
        "尾部就是它最近说的那几行：{reading}"
    );
    assert!(
        reading.contains("command = "),
        "读数要带当时那条命令：{reading}"
    );

    // 收尾：停它，并确认它真的没了（**用例不留孤儿**）。
    let stopped = call(&mut app, &stop_action("server"));
    assert_eq!(failure_of(&stopped), "<none>", "{}", text_of(&stopped));
    let gone = text_of(&call(&mut app, &held_action()));
    assert!(gone.contains("held = 0"), "停掉之后版面上没有了：{gone}");
    assert!(
        gone.contains("stopped-before = 1"),
        "停过几条要如实报：{gone}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn the_tail_says_whether_it_has_spoken_yet() {
    // "还没说话"与"读不到"必须分得开（缺省不等于零）：一件刚起、一句话都还没说的东西，
    // 尾部那一格是「（还没有输出）」，**不是** `unknown`。
    let root = fresh_root("tail-empty");
    let mut app = app_on(&root);
    let started = call(
        &mut app,
        &run_action(
            ActionParams::none()
                .with("command", SILENT)
                .with("keep", "true")
                .with("name", "quiet"),
        ),
    );
    assert_eq!(failure_of(&started), "<none>", "{}", text_of(&started));
    let reading = text_of(&call(&mut app, &held_action()));
    assert!(
        reading.contains("（还没有输出）"),
        "一句话都还没说 ⇒ 明说「还没有输出」：{reading}"
    );
    assert!(
        !reading.contains("tail = unknown"),
        "它不是「读不到」——那个文件是在的、只是空的：{reading}"
    );
    let _ = call(&mut app, &stop_action("quiet"));
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn stopping_something_that_is_not_on_the_ledger_is_refused() {
    let root = fresh_root("stop-unknown");
    let mut app = app_on(&root);
    let outcome = call(&mut app, &stop_action("never-started"));
    assert_eq!(
        failure_of(&outcome),
        "invalid-argument",
        "名字不在账上就不猜"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn keep_false_is_the_one_shot_it_always_was() {
    // 缺省那一档**行为与从前逐字节相同**：跑完即散，回执是 `exit = …`（不是 `held = …`），
    // 而且账上一个字都不写。
    let root = fresh_root("keep-false");
    let mut app = app_on(&root);
    let outcome = call(
        &mut app,
        &run_action(
            ActionParams::none()
                .with("command", "echo hi")
                .with("keep", "false"),
        ),
    );
    let text = text_of(&outcome);
    assert!(text.contains("exit = 0"), "{text}");
    assert!(!text.contains("held = "), "一次性的那一条不进账：{text}");
    assert!(
        HeldThings::new(&root)
            .lines()
            .expect("没账就是空")
            .is_empty(),
        "账上一个字都不写"
    );
    let _ = std::fs::remove_dir_all(&root);
}
