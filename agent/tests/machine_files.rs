//! **文件 App 的机器档**（`FileApp::open_machine` / `machine_with_writes`）的行为证明。
//!
//! 产品口径：主人 2026-10-20 裁的「要么没有要么全开」——不给时只能动自己文件夹里的；
//! 给了（`files=full`）就**任意文件夹**都能读、能写。实现口径见 `sandbox` 与 `file_app` 的模块文档。
//!
//! 这里钉的四件事：
//! 1. **机器档收绝对路径**，读/列/搜/建/改都通；
//! 2. **机器档收相对路径 = invalid-path**（两档互不接受对方的写法，混着来最容易出事）；
//! 3. **家档仍然拒绝对路径** —— 加这一档不许把原来那条边界偷偷放宽；
//! 4. **能力说明里说的是绝对路径**：模型唯一能读到"该写哪种形状"的地方就是那一句，
//!    说错了它就会按错的形状发请求。
//!
//! 沙盒等级与家档同档（受信目录、检查与打开之间有理论替换窗口），不构成 OS 级安全沙盒。

use std::fs;
use std::path::{Path, PathBuf};

use celira_agent::core::{Action, ActionId, ActionOutcome, ActionParams, ActionPort};
use celira_agent::file_app::{FailureClass, FileApp};
use celira_agent::file_view::FileView;
use celira_agent::foundation::{AgentId, OperationId, RunId, Turn};

const AGENT: &str = "agent-machine";

/// 测试自建的临时目录（只在系统临时目录下，用完删掉自己那一份）。
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let unique = format!(
            "celira-machine-files-{}-{}-{}",
            std::process::id(),
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("时钟可用")
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        fs::create_dir_all(&root).expect("可以建临时目录");
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("可以建子目录");
        }
        fs::write(&path, text).expect("可以写合成内容");
        path
    }

    fn absolute(&self, relative: &str) -> String {
        self.root.join(relative).display().to_string()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn operation() -> OperationId {
    OperationId::new(RunId::new("machine-run"), Turn::FIRST)
}

fn call(app: &mut FileApp, id: &str, params: ActionParams) -> ActionOutcome {
    app.execute(
        &AgentId::new(AGENT),
        &operation(),
        &Action::with_params(ActionId::new(id), params),
    )
}

fn read(path: &str) -> ActionParams {
    ActionParams::none().with("path", path)
}

fn list(path: &str) -> ActionParams {
    ActionParams::none().with("path", path)
}

fn create(name: &str, text: &str) -> ActionParams {
    ActionParams::none().with("name", name).with("text", text)
}

fn edit(path: &str, old: &str, new: &str) -> ActionParams {
    ActionParams::none()
        .with("path", path)
        .with("old_text", old)
        .with("new_text", new)
}

fn text_of(outcome: &ActionOutcome) -> String {
    outcome
        .content()
        .iter()
        .map(|block| block.text().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 驱动**视图**那一层（`files` 这个 App 在生产里就是它）。
fn view_call(view: &mut FileView, id: &str, params: ActionParams) -> ActionOutcome {
    view.execute(
        &AgentId::new(AGENT),
        &operation(),
        &Action::with_params(ActionId::new(id), params),
    )
}

/// 能力说明里 `path`/`name` 那句（模型唯一能看到"该写哪种形状"的地方）。
fn path_spec_of(app: &FileApp, action: &str, param: &str) -> String {
    let operation = operation();
    let caps = ActionPort::capabilities(app, &AgentId::new(AGENT), operation.run());
    let capability = caps
        .items()
        .iter()
        .find(|each| each.id().as_str() == action)
        .unwrap_or_else(|| panic!("能力目录里没有 {action}"));
    capability
        .params()
        .iter()
        .find(|spec| spec.name() == param)
        .unwrap_or_else(|| panic!("{action} 的参数里没有 {param}"))
        .description()
        .to_string()
}

#[test]
fn machine_scope_reads_any_absolute_path() {
    let scratch = Scratch::new("read");
    scratch.write("deep/inner/a.txt", "hello machine");

    let mut app = FileApp::open_machine();

    // 列目录：绝对路径。
    let listed = call(&mut app, "files_list", list(&scratch.absolute("deep")));
    assert_eq!(listed.failure_class(), None, "机器档应当列得出 {listed:?}");
    assert!(text_of(&listed).contains("inner"), "{}", text_of(&listed));

    // 读文件：绝对路径。
    let read_back = call(
        &mut app,
        "files_read",
        read(&scratch.absolute("deep/inner/a.txt")),
    );
    assert_eq!(read_back.failure_class(), None);
    assert!(text_of(&read_back).contains("hello machine"));

    // 搜：绝对路径的目录。
    let deep = scratch.absolute("deep");
    let found = call(
        &mut app,
        "files_grep",
        ActionParams::none()
            .with("pattern", "machine")
            .with("path", &deep),
    );
    assert_eq!(found.failure_class(), None, "{}", text_of(&found));
    assert!(text_of(&found).contains("a.txt"), "{}", text_of(&found));
}

#[test]
fn machine_scope_refuses_relative_paths() {
    let mut app = FileApp::open_machine();
    // 两档互不接受对方的写法：机器档收到"相对路径"是形状错误，不是"找不到"。
    for params in [read("deep/a.txt"), list("deep")] {
        let outcome = if params.get("path") == Some("deep") {
            call(&mut app, "files_list", params)
        } else {
            call(&mut app, "files_read", params)
        };
        assert_eq!(
            outcome.failure_class(),
            Some(FailureClass::InvalidPath.as_str()),
            "机器档不许收相对路径：{outcome:?}",
        );
    }
}

#[test]
fn home_scope_still_refuses_absolute_paths() {
    let scratch = Scratch::new("home");
    scratch.write("a.txt", "hello home");
    let mut app = FileApp::open(scratch.path()).expect("家档可以打开");

    let outcome = call(&mut app, "files_read", read(&scratch.absolute("a.txt")));
    assert_eq!(
        outcome.failure_class(),
        Some(FailureClass::InvalidPath.as_str()),
        "家档必须照旧拒绝对路径 —— 加机器档不许把这条边界放宽",
    );
}

#[test]
fn machine_writes_land_where_the_path_points() {
    let scratch = Scratch::new("write");
    fs::create_dir_all(scratch.path().join("out")).expect("可以建子目录");
    let mut app = FileApp::machine_with_writes();

    // 新建：绝对路径。
    let target = scratch.absolute("out/new.txt");
    let created = call(&mut app, "files_create", create(&target, "v1"));
    assert_eq!(created.failure_class(), None, "{created:?}");
    assert_eq!(
        fs::read_to_string(scratch.path().join("out/new.txt")).expect("文件应当在"),
        "v1",
    );

    // 同名再来一次：明确失败，而且一个字节都不动（独占创建）。
    let again = call(&mut app, "files_create", create(&target, "v2"));
    assert_eq!(
        again.failure_class(),
        Some(FailureClass::AlreadyExists.as_str()),
        "{again:?}",
    );
    assert_eq!(
        fs::read_to_string(scratch.path().join("out/new.txt")).expect("文件还在"),
        "v1",
    );

    // 改：绝对路径，唯一一处替换。
    let edited = call(&mut app, "files_edit", edit(&target, "v1", "v9"));
    assert_eq!(edited.failure_class(), None, "{edited:?}");
    assert_eq!(
        fs::read_to_string(scratch.path().join("out/new.txt")).expect("文件还在"),
        "v9",
    );

    // 父目录不在 ⇒ not-found（这一层不替主体建目录）。
    let orphan = scratch.absolute("nope/child.txt");
    let missing = call(&mut app, "files_create", create(&orphan, "x"));
    assert_eq!(
        missing.failure_class(),
        Some(FailureClass::NotFound.as_str()),
        "{missing:?}",
    );
}

#[test]
fn machine_scope_without_the_write_face_does_not_declare_writes() {
    let mut app = FileApp::open_machine();
    let scratch = Scratch::new("readonly");
    let target = scratch.absolute("a.txt");
    let outcome = call(&mut app, "files_create", create(&target, "x"));
    // 只读的机器档**不声明**写能力；准入这一层给出的就是那句"没有授权写"，
    // 与只读的家档同一句话（两档在"读写面"上是同一条规矩）。
    assert_eq!(
        outcome.failure_class(),
        Some("creation-not-granted"),
        "{outcome:?}",
    );
    assert!(!scratch.path().join("a.txt").exists(), "什么都没写出来");
}

#[test]
fn the_view_in_machine_scope_reads_absolute_paths_and_keeps_reserved_addresses() {
    let scratch = Scratch::new("view-machine");
    // 环境根：一份手册（保留地址）。
    let environment = scratch.path().join("env");
    fs::create_dir_all(&environment).expect("可以建环境根");
    fs::write(environment.join("manual.md"), "手写的说明").expect("可以写手册");
    // 资料根：机器档**不装配**它（整台机器就是读面）——这一份读不到才对。
    let materials = scratch.path().join("materials");
    fs::create_dir_all(&materials).expect("可以建资料根");
    fs::write(materials.join("only-home.txt"), "只有家档看得到").expect("可以写资料");
    // 「任意文件夹」：临时目录里另一处，与资料根毫无关系。
    let elsewhere = scratch.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).expect("可以建任意目录");
    fs::write(elsewhere.join("a.txt"), "hello anywhere").expect("可以写文件");

    let mut view =
        FileView::with_machine_scope(&materials, &environment, None).expect("机器档可以装配");

    // ① 绝对路径：读到任意文件夹里的那一份。
    let absolute = elsewhere.join("a.txt").display().to_string();
    let read_back = view_call(&mut view, "files_read", read(&absolute));
    assert_eq!(read_back.failure_class(), None, "{read_back:?}");
    assert!(text_of(&read_back).contains("hello anywhere"));

    // ② 保留地址照旧按名字可用（它不是资料面的地址，是派生的）。
    let manual = view_call(&mut view, "files_read", read("manual.md"));
    assert_eq!(manual.failure_class(), None, "{manual:?}");
    assert!(text_of(&manual).contains("手写的说明"));

    // ③ 相对路径**不再指向资料根**：机器档的地址一律是绝对路径。
    let relative = view_call(&mut view, "files_read", read("only-home.txt"));
    assert_eq!(
        relative.failure_class(),
        Some(FailureClass::InvalidPath.as_str()),
        "{relative:?}",
    );

    // ④ 能力说明跟着档走（模型据此决定写哪种形状）。
    let caps = ActionPort::capabilities(&view, &AgentId::new(AGENT), operation().run());
    let read_path = caps
        .items()
        .iter()
        .find(|each| each.id().as_str() == "files_read")
        .expect("能力目录里有 files_read")
        .params()
        .iter()
        .find(|spec| spec.name() == "path")
        .expect("files_read 有 path 参数");
    assert!(
        read_path.description().contains("绝对"),
        "机器档的说明必须说清可以写绝对路径：{}",
        read_path.description(),
    );
}

#[test]
fn the_capability_text_follows_the_scope() {
    let scratch = Scratch::new("spec");
    // 两边都取**可写**的实例：写能力的说明只在那时才被声明。
    let home = FileApp::with_own_works(scratch.path()).expect("家档可以打开");
    let machine = FileApp::machine_with_writes();

    // 家档：说相对路径。
    let home_spec = path_spec_of(&home, "files_read", "path");
    assert!(home_spec.contains("相对"), "家档的说明：{home_spec}");
    let home_create = path_spec_of(&home, "files_create", "name");
    assert!(
        home_create.contains("相对") && home_create.contains("父目录必须已经存在"),
        "家档的新建说明必须说清「相对路径 ＋ 父目录必须已经存在」：{home_create}",
    );

    // 机器档：说绝对路径（模型据此决定写什么形状）。
    let machine_spec = path_spec_of(&machine, "files_read", "path");
    assert!(
        machine_spec.contains("绝对"),
        "机器档的说明必须说清是绝对路径：{machine_spec}",
    );
    let machine_create = path_spec_of(&machine, "files_create", "name");
    assert!(
        machine_create.contains("绝对"),
        "机器档的新建说明必须说清是绝对路径：{machine_create}",
    );
}
