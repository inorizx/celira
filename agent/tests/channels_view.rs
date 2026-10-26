//! 外部输入收件箱的只读投影：`files_list("channels")` / `files_read("channels/<事件>")`。
//!
//! 这里证明的不是"入口会写文件"（那由 `tests/channels.rs` 真起 HTTP 服务证明），而是
//! **入口写下的那份东西，主体在自己的文件视图里读得到**——并且边界不松：
//!
//! - 地址只由 `<环境根>/channels/inbox` 这一只箱子决定：不在 ⇒ 地址缺席（不是空目录）；
//! - 在场但打不开 ⇒ 如实报分类，不冒充"没有输入"；
//! - 最近到的排在最前（收件箱的名字是随机的，名字序不表达"新"）；
//! - 只读：查找面不吞它（`files_grep` 明确 `invalid-argument`），写动词碰不到它；
//! - 资料面里同名条目会遮蔽这个地址时，**拒绝装配**（`reserved-address-taken`）。
//!
//! 全部离线：临时目录、手工写事件文件、直接驱动 `FileView`（与真实运行同一个 ActionPort）。

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use celira_agent::core::{Action, ActionId, ActionOutcome, ActionParams, ActionPort, ContentBlock};
use celira_agent::file_view::{CHANNELS_ADDRESS, FileView, FileViewFailure};
use celira_agent::foundation::{AgentId, OperationId, ResultState, RunId, Turn};

const LIST: &str = "files_list";
const READ: &str = "files_read";
const GREP: &str = "files_grep";

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let unique = format!(
            "celira-channels-view-{}-{}-{}",
            std::process::id(),
            label,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("时钟可用")
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        fs::create_dir_all(root.join("materials")).expect("建资料目录");
        fs::create_dir_all(root.join("env")).expect("建环境目录");
        Self { root }
    }

    fn materials(&self) -> PathBuf {
        self.root.join("materials")
    }

    fn environment_root(&self) -> PathBuf {
        self.root.join("env")
    }

    fn inbox(&self) -> PathBuf {
        self.environment_root().join("channels").join("inbox")
    }

    fn write_event(&self, name: &str, text: &str) {
        fs::create_dir_all(self.inbox()).expect("建收件箱");
        fs::write(self.inbox().join(name), text).expect("写事件");
    }

    fn view(&self) -> FileView {
        FileView::with_history(&self.materials(), &self.environment_root()).expect("装配视图")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn operation() -> OperationId {
    OperationId::new(RunId::new("run-channels-view"), Turn::FIRST)
}

fn agent() -> AgentId {
    AgentId::new("agent-alpha")
}

fn list(path: &str) -> Action {
    Action::with_params(ActionId::new(LIST), ActionParams::none().with("path", path))
}

fn read(path: &str) -> Action {
    Action::with_params(ActionId::new(READ), ActionParams::none().with("path", path))
}

fn read_windowed(path: &str, offset: &str, limit: &str) -> Action {
    Action::with_params(
        ActionId::new(READ),
        ActionParams::none()
            .with("path", path)
            .with("offset", offset)
            .with("limit", limit),
    )
}

fn text_of(outcome: &ActionOutcome) -> String {
    outcome
        .content()
        .iter()
        .map(ContentBlock::text)
        .collect::<String>()
}

fn assert_failure(outcome: &ActionOutcome, class: &str) {
    assert_eq!(outcome.state(), ResultState::Failure, "{:?}", outcome);
    assert_eq!(outcome.failure_class(), Some(class));
}

/// 两条事件：列得到、读得到、**最近的先**。
#[test]
fn the_inbox_is_listed_newest_first_and_read_whole_or_in_a_window() {
    let fixture = Fixture::new("list-read");
    fixture.write_event("github--old.json", "old body");
    std::thread::sleep(Duration::from_millis(20));
    fixture.write_event("deploy--new.json", "new body");
    let mut view = fixture.view();

    let listing = view.execute(&agent(), &operation(), &list(CHANNELS_ADDRESS));
    assert_eq!(listing.state(), ResultState::Success, "{listing:?}");
    let text = text_of(&listing);
    assert!(
        text.starts_with("entries = 2, truncated = false, more = none\n"),
        "{text}"
    );
    let new_at = text.find("deploy--new.json").expect("列得到新事件");
    let old_at = text.find("github--old.json").expect("列得到旧事件");
    assert!(new_at < old_at, "最近到的必须排在前：{text}");
    assert!(
        text.contains("mute-file = projects/channels-mute"),
        "静音表的位置要摆出来：{text}"
    );

    let read = view.execute(&agent(), &operation(), &read("channels/deploy--new.json"));
    assert_eq!(read.state(), ResultState::Success, "{read:?}");
    assert_eq!(text_of(&read), "new body");

    // 窗口参数照旧有效（与资料面同一条读取实现）。
    let windowed = view.execute(
        &agent(),
        &operation(),
        &read_windowed("channels/deploy--new.json", "0", "3"),
    );
    assert_eq!(windowed.state(), ResultState::Success, "{windowed:?}");
    let text = text_of(&windowed);
    assert!(
        text.contains("bytes = 0..3, total = 8, more = next 3"),
        "{text}"
    );
}

/// 没有入口落点 ⇒ 地址**缺席**（不是空目录）：列它是 not-found，视图根里也不摆它。
#[test]
fn without_an_inbox_the_address_is_absent_not_empty() {
    let fixture = Fixture::new("absent");
    let mut view = fixture.view();

    assert_failure(
        &view.execute(&agent(), &operation(), &list(CHANNELS_ADDRESS)),
        "not-found",
    );

    let root = view.execute(&agent(), &operation(), &list(""));
    assert_eq!(root.state(), ResultState::Success, "{root:?}");
    assert!(
        !text_of(&root).contains(CHANNELS_ADDRESS),
        "地址不在时不能凭空摆一个空入口：{}",
        text_of(&root)
    );
}

/// 收件箱在场但打不开（同名的是普通文件）⇒ 视图根照旧摆出这个名字，访问时如实报分类。
#[test]
fn an_unusable_inbox_is_reported_not_hidden_as_missing() {
    let fixture = Fixture::new("unusable");
    fs::create_dir_all(fixture.environment_root().join("channels")).expect("建 channels");
    fs::write(fixture.inbox(), "not a directory").expect("放一个同名文件");
    let mut view = fixture.view();

    let root = view.execute(&agent(), &operation(), &list(""));
    assert_eq!(root.state(), ResultState::Success, "{root:?}");
    assert!(
        text_of(&root).contains(CHANNELS_ADDRESS),
        "打不开也要摆出名字：{}",
        text_of(&root)
    );

    assert_failure(
        &view.execute(&agent(), &operation(), &list(CHANNELS_ADDRESS)),
        "wrong-kind",
    );
}

/// 只读：查找面不吞它（保留地址在场 ⇒ invalid-argument，缺席 ⇒ not-found）。
#[test]
fn the_inbox_is_read_only_and_outside_the_search_face() {
    let fixture = Fixture::new("grep");
    fixture.write_event("github--one.json", "hello");
    let mut view = fixture.view();

    let grep = Action::with_params(
        ActionId::new(GREP),
        ActionParams::none()
            .with("path", "channels")
            .with("pattern", "hello"),
    );
    assert_failure(
        &view.execute(&agent(), &operation(), &grep),
        "invalid-argument",
    );
}

/// 资料面里同名条目会遮蔽这个地址 ⇒ 装配期拒绝，而不是静默遮蔽。
#[test]
fn a_material_entry_with_the_same_name_refuses_the_assembly() {
    let fixture = Fixture::new("reserved");
    fixture.write_event("github--one.json", "hello");
    fs::create_dir_all(fixture.materials().join(CHANNELS_ADDRESS)).expect("建同名资料目录");

    match FileView::with_history(&fixture.materials(), &fixture.environment_root()) {
        Err(FileViewFailure::ReservedAddressTaken) => {}
        other => panic!("必须拒绝装配，得到 {other:?}"),
    }
}

/// 读一个**目录**（地址本身）是 invalid-path，与在资料面里读目录同款。
#[test]
fn reading_the_address_itself_is_a_path_error() {
    let fixture = Fixture::new("read-dir");
    fixture.write_event("github--one.json", "hello");
    let mut view = fixture.view();
    assert_failure(
        &view.execute(&agent(), &operation(), &read(CHANNELS_ADDRESS)),
        "invalid-path",
    );
}
