//! TASK_12 验收：三个**只读原子工具**——`files_list` 续列（`after`）、`files_read` 续读
//! （`offset` / `limit`）、按子串查找（`files_grep`）。
//!
//! 这一份文件把施工单 §6 的规范句逐条落成**可判错**的用例。全部离线、全合成数据、只用测试自建
//! 的临时目录；文件层直接驱动 App，视图层驱动最小 OS 的使用作用域（与真实运行同一套路由与
//! 同一个 ActionPort），不经过模型。
//!
//! 刻意**不**声称的东西：没有编排（谁都不替主体决定先找还是先读）；没有排序/评分/摘要/索引；
//! `files_grep` 是**子串**匹配（区分大小写），不是词法或语义；不扫 `history/`；
//! `after` 是**名字**而不是游标，条目在两次调用之间增删时续列可能漏或重（地址空间是活的）。

use std::fs;
use std::path::{Path, PathBuf};

use celira_agent::app_contract::{AppPort, EffectClass};
use celira_agent::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort,
};
use celira_agent::creation_grant::CreationGrant;
use celira_agent::file_app::{
    FILE_APP_ID, FailureClass, FileApp, MAX_ENTRIES, MAX_SCANNED, MAX_SNIPPET_BYTES, MAX_TEXT_BYTES,
};
use celira_agent::file_view::FileView;
use celira_agent::foundation::{
    AgentId, OperationId, Outcome, ResultState, RunId, SubjectEnding, Turn,
};
use celira_agent::os::{MinimalOs, RunExpansion};
use celira_agent::store::{Environment, TracePayload, ending_payload};

const LIST: &str = "files_list";
const READ: &str = "files_read";
const FIND: &str = "files_grep";
const EXPAND: &str = "open_app";
/// 看详情的动词（TASK_69：调用之前"先看格式"那一轮用的就是它）。
const INSPECT: &str = "inspect_action";
const HISTORY: &str = "history";
/// 授权创建的目录名（相对资料根的单层目录）。
const GRANT_DIR: &str = "workman";

// ---------------------------------------------------------------------------
// 自建的临时目录
// ---------------------------------------------------------------------------

/// 文件层的临时沙盒（只建一个目录）。
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(label: &str) -> Self {
        let root = temp_root("celira-atomic-read", label);
        fs::create_dir_all(&root).expect("建临时目录");
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn write(&self, relative: &str, text: &str) {
        self.write_bytes(relative, text.as_bytes());
    }

    fn write_bytes(&self, relative: &str, bytes: &[u8]) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("建子目录");
        }
        fs::write(&path, bytes).expect("写合成内容");
    }

    fn open(&self) -> FileApp {
        FileApp::open(self.root.clone()).expect("沙盒可以打开")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// 视图层的临时目录：`root/materials` 是授权资料目录，`root/env` 是身份环境。
struct ViewFixture {
    root: PathBuf,
}

impl ViewFixture {
    fn new(label: &str) -> Self {
        let root = temp_root("celira-atomic-view", label);
        fs::create_dir_all(root.join("materials").join(GRANT_DIR)).expect("建资料目录");
        Self { root }
    }

    fn materials(&self) -> PathBuf {
        self.root.join("materials")
    }

    fn env_root(&self) -> PathBuf {
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

    /// 建身份环境，并写入一个已收尾活动的合成经历（正文里带一句可查找的串）。
    fn environment(&self, marker: &str) -> Environment {
        let mut env = Environment::create(&self.env_root(), Self::agent()).expect("建立身份环境");
        let run = RunId::new("run-history");
        for payload in [
            TracePayload::RunStarted { origin: None },
            TracePayload::RunInput {
                text: marker.to_string(),
            },
            TracePayload::Injection {
                stage: "run-start".to_string(),
                turn: None,
                blocks: vec![("note".to_string(), "a synthetic note".to_string())],
            },
            ending_payload(Outcome::Subject(SubjectEnding::Completed)),
            TracePayload::Sealed,
        ] {
            env.append_trace(&run, payload).expect("追加合成经历");
        }
        env
    }
}

impl Drop for ViewFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn temp_root(prefix: &str, label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("{prefix}-{}-{label}-{n}", std::process::id()))
}

// ---------------------------------------------------------------------------
// 动作构造与读数解析
// ---------------------------------------------------------------------------

fn direct_operation() -> OperationId {
    OperationId::new(RunId::new("direct"), Turn::FIRST)
}

/// 一次活动里的第一个操作（作用域会核对身份与活动）。
fn operation(run: &RunId) -> OperationId {
    OperationId::new(run.clone(), Turn::FIRST)
}

fn call(app: &mut FileApp, action: &Action) -> ActionOutcome {
    app.execute(&AgentId::new("agent-direct"), &direct_operation(), action)
}

fn expand() -> Action {
    Action::with_params(
        ActionId::new(EXPAND),
        ActionParams::none().with("target", FILE_APP_ID),
    )
}

/// `inspect_action` 的载荷（TASK_69：调用之前先看格式那一轮）。
fn inspect(name: &str) -> Action {
    Action::with_params(
        ActionId::new(INSPECT),
        ActionParams::none().with("action", name),
    )
}

/// TASK_69：**先查格式再调**——展开之后新出现的动作不在"在场集合"里，
/// 所以调用之前要先各看一次。这是新形态下主体本来就要做的那一轮，不是绕过校验。
fn inspect_all(
    scope: &mut celira_agent::os::RunScope<'_>,
    agent: &AgentId,
    run: &RunId,
    names: &[&str],
) {
    for name in names {
        let outcome = scope.execute(agent, &operation(run), &inspect(name));
        assert_eq!(
            outcome.state(),
            ResultState::Success,
            "先看格式这一拍必须成功: {name} {outcome:?}"
        );
    }
}

/// `files_list`：`after` 给不给都由这里决定。
fn list_after(after: Option<&str>) -> Action {
    let params = match after {
        Some(name) => ActionParams::none().with("after", name),
        None => ActionParams::none(),
    };
    Action::with_params(ActionId::new(LIST), params)
}

fn read(path: &str) -> Action {
    Action::with_params(ActionId::new(READ), ActionParams::none().with("path", path))
}

/// `files_read`：窗口参数按需给出（`None` 表示**不给这个参数**，而不是给 0）。
fn read_window(path: &str, offset: Option<&str>, limit: Option<&str>) -> Action {
    let mut params = ActionParams::none().with("path", path);
    if let Some(offset) = offset {
        params = params.with("offset", offset);
    }
    if let Some(limit) = limit {
        params = params.with("limit", limit);
    }
    Action::with_params(ActionId::new(READ), params)
}

fn find(pattern: &str, path: Option<&str>) -> Action {
    let mut params = ActionParams::none().with("pattern", pattern);
    if let Some(path) = path {
        params = params.with("path", path);
    }
    Action::with_params(ActionId::new(FIND), params)
}

fn text_of(outcome: &ActionOutcome) -> String {
    outcome
        .content()
        .iter()
        .map(|block| block.text())
        .collect::<String>()
}

/// 首行的 `more` 读数（`none` 或 `after <名字>` / `next <字节>`）。
fn more_of(text: &str) -> String {
    let header = text.lines().next().expect("列表有首行");
    let (_, more) = header.split_once(", more = ").expect("首行带 more 读数");
    more.to_string()
}

/// 列表的数据行（跳过首行）里的名字列。
fn entry_names(text: &str) -> Vec<String> {
    text.lines()
        .skip(1)
        .map(|line| line.split('\t').next().unwrap_or_default().to_string())
        .collect()
}

/// `files_grep` 的候选行：`路径 \t 命中位置 \t 片段`。
fn candidates(text: &str) -> Vec<(String, String, String)> {
    text.lines()
        .skip(1)
        .map(|line| {
            let mut parts = line.split('\t');
            (
                parts.next().unwrap_or_default().to_string(),
                parts.next().unwrap_or_default().to_string(),
                parts.next().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// 续读的机械读数。
struct Window {
    start: u64,
    end: u64,
    total: u64,
    more: String,
}

fn parse_window(readings: &str) -> Window {
    let rest = readings
        .strip_prefix("bytes = ")
        .expect("读数以字节区间开头");
    let (range, rest) = rest.split_once(", total = ").expect("读数带总长度");
    let (total, more) = rest.split_once(", more = ").expect("读数带 more");
    let (start, end) = range.split_once("..").expect("区间是 start..end");
    Window {
        start: start.parse().expect("起始字节是数字"),
        end: end.parse().expect("结束字节是数字"),
        total: total.parse().expect("总长度是数字"),
        more: more.to_string(),
    }
}

/// 一次续读结果的两个块：机械读数 + 本段正文。
fn window_of(outcome: &ActionOutcome) -> (Window, String) {
    let blocks = outcome.content();
    assert_eq!(blocks.len(), 2, "续读给出读数与正文两个块");
    assert_eq!(blocks[0].label(), "files_read-window", "第一个块是读数");
    assert_eq!(blocks[1].label(), "files_read", "第二个块是本段正文");
    (parse_window(blocks[0].text()), blocks[1].text().to_string())
}

/// 第 `nth` 个字符的字节偏移（`nth` 超出长度时给整长）。
fn boundary(text: &str, nth: usize) -> u64 {
    text.char_indices()
        .nth(nth)
        .map(|(index, _)| index as u64)
        .unwrap_or(text.len() as u64)
}

// ---------------------------------------------------------------------------
// 规范句 1 / 2：`files_list` 续列不重不漏、到底明说、给出的 `after` 确实能接着取
// ---------------------------------------------------------------------------

/// 句 1：用 `after` 一页页取出来的并集 == 全量列表（少于一个上限时逐项相等）。
#[test]
fn list_after_pages_the_directory_without_gaps_or_repeats() {
    // 少于一个上限：逐项相等。
    let small = Sandbox::new("page-small");
    for name in ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"] {
        small.write(name, "x");
    }
    let mut app = small.open();
    let all = text_of(&call(&mut app, &list_after(None)));
    let names = entry_names(&all);
    assert_eq!(names, vec!["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"]);

    // 从第 2 项接着列：剩下的必须**恰好**是它后面的那几项。
    let rest = text_of(&call(&mut app, &list_after(Some("b.txt"))));
    assert_eq!(entry_names(&rest), names[2..].to_vec());

    // 超过一个上限：一页页取出来的并集 == 全量名字集合，不重不漏。
    let big = Sandbox::new("page-big");
    let total = MAX_ENTRIES + 30;
    for index in 0..total {
        big.write(&format!("file-{index:04}.txt"), "x");
    }
    let mut app = big.open();

    let first = text_of(&call(&mut app, &list_after(None)));
    assert_eq!(entry_names(&first).len(), MAX_ENTRIES);
    assert_eq!(
        more_of(&first),
        format!("after file-{:04}.txt", MAX_ENTRIES - 1)
    );

    let second = text_of(&call(
        &mut app,
        &list_after(Some(&format!("file-{:04}.txt", MAX_ENTRIES - 1))),
    ));
    let second_names = entry_names(&second);
    assert_eq!(second_names.len(), 30);

    let mut union = entry_names(&first);
    union.extend(second_names);
    let expected: Vec<String> = (0..total)
        .map(|index| format!("file-{index:04}.txt"))
        .collect();
    assert_eq!(union, expected, "并集必须等于全量列表，而且不重不漏");
}

/// 句 2：最后一页明确说没有更多；被截断时给出的 `after` 确实能接着取。
#[test]
fn list_says_when_there_is_no_more_and_hands_a_usable_after() {
    let sandbox = Sandbox::new("page-more");
    for index in 0..(MAX_ENTRIES + 5) {
        sandbox.write(&format!("n-{index:04}.txt"), "x");
    }
    let mut app = sandbox.open();

    let first = text_of(&call(&mut app, &list_after(None)));
    assert!(
        first.starts_with(&format!("entries = {MAX_ENTRIES}, truncated = true")),
        "首行仍与从前同源: {first}"
    );
    let handed = more_of(&first);
    let last = format!("n-{:04}.txt", MAX_ENTRIES - 1);
    assert_eq!(handed, format!("after {last}"), "截断时给出下次该用的名字");
    assert!(!handed.contains("none"), "被截断就不能说没有更多");

    // 拿首行给的那个名字接着取：真的能取到剩下的，而且严格在它之后。
    let next = text_of(&call(&mut app, &list_after(Some(&last))));
    let next_names = entry_names(&next);
    assert_eq!(next_names.len(), 5);
    assert!(next_names.iter().all(|name| name.as_str() > last.as_str()));
    assert_eq!(more_of(&next), "none", "到底了就明确写没有更多");
    assert!(
        next.starts_with("entries = 5, truncated = false, more = none"),
        "{next}"
    );
}

// ---------------------------------------------------------------------------
// 规范句 3 / 4 / 5：`files_read` 续读
// ---------------------------------------------------------------------------

/// 句 3：分段读出来的内容拼接 == 全文；覆盖区间与总长度读数与实际相符。
#[test]
fn read_text_windows_join_into_the_whole_file() {
    let sandbox = Sandbox::new("read-join");
    let text = "第一行 alpha\n第二行 beta\n第三行 gamma\n";
    sandbox.write("multi.txt", text);
    let mut app = sandbox.open();

    let breaks = [
        0u64,
        boundary(text, 3),
        boundary(text, 8),
        boundary(text, 13),
        text.len() as u64,
    ];
    let mut joined = String::new();
    for pair in breaks.windows(2) {
        let (start, stop) = (pair[0], pair[1]);
        // 每一步正好取到下一个边界：因此本段的结束字节必须恰好落在那个边界上。
        let step = (stop - start).to_string();
        let outcome = call(
            &mut app,
            &read_window("multi.txt", Some(&start.to_string()), Some(&step)),
        );
        assert_eq!(outcome.state(), ResultState::Success, "{start}..{stop}");
        let (window, segment) = window_of(&outcome);
        assert_eq!(window.start, start, "读数里的起始字节与实际相符");
        assert_eq!(window.end, stop, "读数里的结束字节与实际相符");
        assert_eq!(window.total, text.len() as u64, "全文总字节数如实");
        let expected_more = if stop < text.len() as u64 {
            "next"
        } else {
            "none"
        };
        assert!(
            window.more.starts_with(expected_more),
            "还有更多 / 到底了的读数不对: {}",
            window.more
        );
        assert!(segment.len() as u64 <= stop - start, "本段不超过给的 limit");
        joined.push_str(&segment);
    }
    assert_eq!(joined, text, "分段读出来的内容拼接必须等于全文");

    // 恰好停在文件末尾那一页之后：本段为空、明确说没有更多（不是越界）。
    let outcome = call(
        &mut app,
        &read_window("multi.txt", Some(&text.len().to_string()), Some("8")),
    );
    assert_eq!(outcome.state(), ResultState::Success);
    let (window, segment) = window_of(&outcome);
    assert_eq!(segment, "", "末尾之后没有字节");
    assert_eq!(window.more, "none");
    assert_eq!(window.total, text.len() as u64);
}

/// 句 4：`offset` 落在字符中间 ⇒ 明确失败；`offset` 越过全文 ⇒ 明确失败。
#[test]
fn read_text_refuses_a_mid_character_offset_and_an_offset_past_the_end() {
    let sandbox = Sandbox::new("read-boundary");
    let text = "中文abc";
    sandbox.write("mixed.txt", text);
    let mut app = sandbox.open();

    // 第一个字符占 3 字节：偏移 1、2 都落在它中间。
    for middle in ["1", "2"] {
        let outcome = call(&mut app, &read_window("mixed.txt", Some(middle), None));
        assert_eq!(
            outcome.state(),
            ResultState::Failure,
            "偏移 {middle} 必须失败"
        );
        assert_eq!(
            outcome.failure_class(),
            Some(FailureClass::InvalidArgument.as_str())
        );
        assert!(outcome.content().is_empty(), "失败时不带回半截内容");
    }
    // 合法边界仍然可读，而且读出来的东西不带半个字符。
    let outcome = call(&mut app, &read_window("mixed.txt", Some("3"), Some("3")));
    assert_eq!(outcome.state(), ResultState::Success);
    let (_, segment) = window_of(&outcome);
    assert_eq!(segment, "文");

    // 越过全文长度：明确失败，**不**返回空串。
    let past = (text.len() + 1).to_string();
    let outcome = call(&mut app, &read_window("mixed.txt", Some(&past), None));
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(
        outcome.failure_class(),
        Some(FailureClass::InvalidArgument.as_str())
    );
    assert!(outcome.content().is_empty(), "越界不得返回空串");
}

/// 句 5：不给 `offset` / `limit` 的 `files_read` 与从前逐字节相同。
///
/// 可取到的对照是"输出 == 文件本身的字节"加上"形状没有多出任何读数"：这个版本里没有旧二进制
/// 可比，因此逐字节比对的对象是**独立读回来的同一份文件**，而且既有的
/// `tests/file_app.rs` / `tests/history_view.rs` 期望**一字未改**地继续通过。
#[test]
fn read_text_without_window_parameters_is_byte_identical() {
    let sandbox = Sandbox::new("read-identical");
    let exact = "z".repeat(MAX_TEXT_BYTES as usize);
    let cases: Vec<(&str, &[u8])> = vec![
        ("empty.txt", b""),
        ("no-newline.txt", b"alpha"),
        ("crlf.txt", b"line-1\r\nline-2\r\n"),
        ("chinese.txt", "中文内容，逐字节相同".as_bytes()),
        ("exact.txt", exact.as_bytes()),
    ];
    let mut app = sandbox.open();

    for (name, bytes) in cases {
        sandbox.write_bytes(name, bytes);
        let outcome = call(&mut app, &read(name));
        assert_eq!(outcome.state(), ResultState::Success, "{name} 应当可读");

        let blocks = outcome.content();
        assert_eq!(
            blocks.len(),
            1,
            "{name} 无参读取只有一个块（形状与从前相同）"
        );
        assert_eq!(blocks[0].label(), READ);
        let expected = String::from_utf8(bytes.to_vec()).expect("用例都是合法 UTF-8");
        assert_eq!(
            blocks[0].text().as_bytes(),
            expected.as_bytes(),
            "{name} 必须逐字节等于文件本身"
        );
        assert!(
            !blocks[0].text().contains("total = "),
            "{name} 不该多出任何续读读数"
        );
    }

    // 上限语义一字未动：正好到上限可读，多一个字节仍明确拒绝。
    sandbox.write_bytes("over.txt", &vec![b'z'; MAX_TEXT_BYTES as usize + 1]);
    let outcome = call(&mut app, &read("over.txt"));
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(
        outcome.failure_class(),
        Some(FailureClass::TooLarge.as_str())
    );
}

// ---------------------------------------------------------------------------
// 规范句 6 / 7 / 8 / 9：`files_grep`
// ---------------------------------------------------------------------------

/// 句 6：命中文件名与命中正文两种情形都能给出，而且标明命中在哪。
#[test]
fn find_text_reports_both_a_name_hit_and_a_body_hit() {
    let sandbox = Sandbox::new("find-both");
    sandbox.write("needle-name.txt", "unrelated body\n");
    sandbox.write(
        "body.txt",
        "开头一行\n这一行里有 needle 出现在中间\n结尾一行\n",
    );
    sandbox.write("quiet.txt", "no match here\n");
    let mut app = sandbox.open();

    let outcome = call(&mut app, &find("needle", None));
    assert_eq!(outcome.state(), ResultState::Success);
    let text = text_of(&outcome);
    assert!(
        text.starts_with("candidates = 2, scanned = 3, skipped = 0, truncated = false"),
        "{text}"
    );
    let found = candidates(&text);
    assert_eq!(
        found,
        vec![
            (
                "body.txt".to_string(),
                "line=2".to_string(),
                "这一行里有 needle 出现在中间".to_string()
            ),
            (
                "needle-name.txt".to_string(),
                "name".to_string(),
                "needle-name.txt".to_string()
            ),
        ],
        "名字命中与正文命中都要给出，并标明命中在哪"
    );
}

/// 句 7：同一条材料最多出现一次；顺序是**路径名序**，与命中次数无关。
#[test]
fn find_text_lists_each_material_once_in_path_order() {
    let sandbox = Sandbox::new("find-order");
    // 命中 6 次，但按路径名排在后面。
    sandbox.write("zzz-many.txt", "针\n针\n针\n针\n针\n针\n");
    // 只命中 1 次，但按路径名排在前面。
    sandbox.write("aaa-once.txt", "针\n");
    let mut app = sandbox.open();

    let text = text_of(&call(&mut app, &find("针", None)));
    let found = candidates(&text);
    assert_eq!(
        found.iter().map(|item| item.0.as_str()).collect::<Vec<_>>(),
        vec!["aaa-once.txt", "zzz-many.txt"],
        "顺序只能是路径名序，命中次数不得让它提前"
    );
    assert_eq!(found.len(), 2, "同一条材料最多出现一次");
    assert!(
        text.starts_with("candidates = 2, scanned = 2, skipped = 0, truncated = false"),
        "{text}"
    );
    assert_eq!(found[1].1, "line=1", "取第一处命中");
}

/// 句 8：超过返回条数或扫描文件数上限 ⇒ 首行如实报截断。
#[test]
fn find_text_reports_truncation_instead_of_pretending_there_is_no_more() {
    // 返回条数上限：命中很多，只给 100 条，但**明说**被截断。
    let many = Sandbox::new("find-many-hits");
    for index in 0..(MAX_ENTRIES + 1) {
        many.write(&format!("hit-{index:04}.txt"), "针\n");
    }
    let mut app = many.open();
    let text = text_of(&call(&mut app, &find("针", None)));
    let found = candidates(&text);
    assert_eq!(found.len(), MAX_ENTRIES);
    assert!(
        text.starts_with(&format!(
            "candidates = {MAX_ENTRIES}, scanned = {}, skipped = 0, truncated = true",
            MAX_ENTRIES + 1
        )),
        "{text}"
    );

    // 扫描文件数上限：文件远多于上限，前三个命中；**不许**说成"就只有这三个"。
    let deep = Sandbox::new("find-scan-cap");
    for index in 0..3 {
        deep.write(&format!("hit-{index:03}.txt"), "针\n");
    }
    for index in 0..(MAX_SCANNED + 4) {
        deep.write(&format!("plain-{index:04}.txt"), "没有\n");
    }
    let mut app = deep.open();
    let text = text_of(&call(&mut app, &find("针", None)));
    let found = candidates(&text);
    assert_eq!(found.len(), 3);
    assert!(
        text.starts_with(&format!(
            "candidates = 3, scanned = {MAX_SCANNED}, skipped = 0, truncated = true"
        )),
        "扫到上限必须如实报截断: {text}"
    );
}

/// 看不了正文的文件**计入 `skipped`**：`没找到`与`没有看`必须能分开。
#[test]
fn find_text_says_it_did_not_look_inside_what_it_could_not_read() {
    let sandbox = Sandbox::new("find-skipped");
    sandbox.write("big.txt", &"x".repeat(MAX_TEXT_BYTES as usize + 1));
    sandbox.write_bytes("binary.bin", &[0xff, 0xfe, 0xfd, 0x00]);
    sandbox.write("plain.txt", "nothing\n");
    let mut app = sandbox.open();

    let text = text_of(&call(&mut app, &find("针", None)));
    assert_eq!(
        text.lines().next(),
        Some("candidates = 0, scanned = 3, skipped = 2, truncated = false"),
        "{text}"
    );
}

/// 句 9：`path` 限定子树有效——子树外的材料不出现在结果里。
#[test]
fn find_text_stays_inside_the_subtree() {
    let sandbox = Sandbox::new("find-subtree");
    sandbox.write("sub/hit.txt", "目标串 在这里\n");
    sandbox.write("sub/deep/hit.txt", "目标串 也在深处\n");
    sandbox.write("other/hit.txt", "目标串 在别处\n");
    let mut app = sandbox.open();

    // 不限子树：三份都能找到（顺序是路径名序）。
    let all = candidates(&text_of(&call(&mut app, &find("目标串", None))));
    assert_eq!(
        all.iter().map(|item| item.0.as_str()).collect::<Vec<_>>(),
        vec!["other/hit.txt", "sub/deep/hit.txt", "sub/hit.txt"]
    );

    // 限定 `sub`：子树外的 `other/hit.txt` 一个都不许出现。
    let text = text_of(&call(&mut app, &find("目标串", Some("sub"))));
    let scoped = candidates(&text);
    assert_eq!(
        scoped
            .iter()
            .map(|item| item.0.as_str())
            .collect::<Vec<_>>(),
        vec!["sub/deep/hit.txt", "sub/hit.txt"]
    );
    assert!(!text.contains("other/"), "子树外的材料不得出现: {text}");

    // `path` 限定的是**子树**：给一个文件是类型不对；越界路径与不存在都是既有分类。
    let outcome = call(&mut app, &find("目标串", Some("sub/hit.txt")));
    assert_eq!(
        outcome.failure_class(),
        Some(FailureClass::WrongKind.as_str())
    );
    let outcome = call(&mut app, &find("目标串", Some("../outside")));
    assert_eq!(
        outcome.failure_class(),
        Some(FailureClass::InvalidPath.as_str())
    );
    let outcome = call(&mut app, &find("目标串", Some("nope")));
    assert_eq!(
        outcome.failure_class(),
        Some(FailureClass::NotFound.as_str())
    );
}

/// 边界：遍历**不跟进**链接与 reparse point——链接指向资料根之外时，查找不得跟着出去。
///
/// 能建就验证拒绝，建不了就明确报告未覆盖（与 `tests/file_app.rs` 的链接用例同一套纪律）。
#[test]
fn find_text_does_not_follow_links_out_of_the_materials() {
    let outside = Sandbox::new("find-link-outside");
    outside.write("secret.txt", "目标串 在资料根之外\n");

    let sandbox = Sandbox::new("find-link");
    sandbox.write("plain.txt", "目标串 在里面\n");
    let link_dir = sandbox.path().join("linked-dir");
    let link_file = sandbox.path().join("linked.txt");

    #[cfg(windows)]
    let (dir_ok, file_ok) = (
        std::os::windows::fs::symlink_dir(outside.path(), &link_dir).is_ok(),
        std::os::windows::fs::symlink_file(outside.path().join("secret.txt"), &link_file).is_ok(),
    );
    #[cfg(not(windows))]
    let (dir_ok, file_ok) = (
        std::os::unix::fs::symlink(outside.path(), &link_dir).is_ok(),
        std::os::unix::fs::symlink(outside.path().join("secret.txt"), &link_file).is_ok(),
    );
    if !dir_ok || !file_ok {
        eprintln!(
            "未覆盖：本环境无法创建符号链接（Windows 通常需要开发者模式或管理员权限）——\
             查找不跟进链接这条边界未验证"
        );
        return;
    }

    let mut app = sandbox.open();
    let text = text_of(&call(&mut app, &find("目标串", None)));
    let found = candidates(&text);
    assert_eq!(
        found.iter().map(|item| item.0.as_str()).collect::<Vec<_>>(),
        vec!["plain.txt"],
        "链接指向的东西不得出现在结果里: {text}"
    );
    assert!(
        text.starts_with("candidates = 1, scanned = 1, skipped = 0, truncated = false"),
        "链接与 reparse point 不算材料，也不计入扫描数: {text}"
    );
}

// ---------------------------------------------------------------------------
// 可判错的边界（施工单 §7.3）与规范句 10
// ---------------------------------------------------------------------------

/// 可判错的边界：非法 `after`、空 `pattern`、超长 `limit`、非数字窗口参数。
#[test]
fn the_new_parameters_have_judgeable_refusals() {
    let sandbox = Sandbox::new("refusals");
    sandbox.write("alpha.txt", "alpha");
    let mut app = sandbox.open();

    // 非法 `after`：不静默当成"从头开始"。
    let outcome = call(&mut app, &list_after(Some("nope.txt")));
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(
        outcome.failure_class(),
        Some(FailureClass::InvalidArgument.as_str())
    );

    // 空 `pattern`：拒绝，不是匹配一切。
    let outcome = call(&mut app, &find("", None));
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(
        outcome.failure_class(),
        Some(FailureClass::InvalidArgument.as_str())
    );

    // 缺 `pattern`：缺参数；不认识的参数名：未知参数。
    let outcome = call(
        &mut app,
        &Action::with_params(ActionId::new(FIND), ActionParams::none()),
    );
    assert_eq!(
        outcome.failure_class(),
        Some(FailureClass::MissingParameter.as_str())
    );
    let outcome = call(
        &mut app,
        &Action::with_params(
            ActionId::new(FIND),
            ActionParams::none()
                .with("pattern", "pattern")
                .with("path", "alpha.txt")
                .with("depth", "2"),
        ),
    );
    assert_eq!(
        outcome.failure_class(),
        Some(FailureClass::UnknownParameter.as_str())
    );

    // 超长 `limit`：与整份读取同源的 `too-large`，不截断。
    let over = (MAX_TEXT_BYTES + 1).to_string();
    let outcome = call(&mut app, &read_window("alpha.txt", None, Some(&over)));
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(
        outcome.failure_class(),
        Some(FailureClass::TooLarge.as_str())
    );

    // 非十进制非负整数的窗口参数：明确拒绝，不尽力解析。
    for bad in ["-1", "+1", " 1", "1 ", "0x10", "1.5", ""] {
        let outcome = call(&mut app, &read_window("alpha.txt", Some(bad), None));
        assert_eq!(
            outcome.failure_class(),
            Some(FailureClass::InvalidArgument.as_str()),
            "offset = {bad:?} 应当被拒绝"
        );
        let outcome = call(&mut app, &read_window("alpha.txt", None, Some(bad)));
        assert_eq!(
            outcome.failure_class(),
            Some(FailureClass::InvalidArgument.as_str()),
            "limit = {bad:?} 应当被拒绝"
        );
    }

    // 片段上限：超长的一行被显示截断到 120 字节以内，并标出来。
    let long = Sandbox::new("snippet");
    let line = "针".repeat(200);
    long.write("long.txt", &format!("{line}\n"));
    let mut app = long.open();
    let text = text_of(&call(&mut app, &find("针", None)));
    let found = candidates(&text);
    assert_eq!(found.len(), 1);
    let snippet = &found[0].2;
    assert!(
        snippet.len() <= MAX_SNIPPET_BYTES,
        "片段不得超过上限: {}",
        snippet.len()
    );
    assert!(snippet.ends_with('…'), "超长要标出来");
    assert!(snippet.starts_with('针'));
}

/// 句 10：三个动词都不声明写效应；装配期不为它们要求复核；**读类别与写类别都照常准入**。
///
/// ⚠ **旧规矩 → 新规矩**（2026-09-26，两刀）：这条用例原来读的是**装配期收集的效果声明表**
/// （`os.effects()`：`class_if_assembled` / `verification_of` / `is_write`），因为**未决效果门**
/// 就是按那张表决定"这一格要不要拦"；再往后那扇门与跨进程恢复一起退役（门是指令，而世界只该给
/// 信息）。**本刀把表本身也退役了**：门是它唯一的生产读者，门一走，它就只剩"装配期收集、之后
/// 无人读"这一半。**声明一个字没退**——读数回到**权威源**：视图自己的 `declared_effects()`，
/// 装配期与展开期的 `check_declarations` 按同一份声明做自洽性检查。
#[test]
fn the_three_read_verbs_declare_no_write_effect_and_everything_is_admitted() {
    let fixture = ViewFixture::new("effects");
    fixture.write_material("alpha.txt", "alpha");
    fixture.environment("marker");
    let agent = ViewFixture::agent();
    let run = RunId::new("run-effects");
    let grant = CreationGrant::new(agent.clone(), run.clone(), GRANT_DIR).expect("授权请求合法");

    // 视图先造出来，**读它自己的声明**，再装进 OS（`MinimalOs::with_history_and_creation`
    // 做的就是这两步：同一个视图、同一份声明，只是这里把中间那一步露出来）。
    let view = FileView::with_history_and_creation(
        &fixture.materials(),
        &fixture.env_root(),
        Some(&grant),
    )
    .expect("装配只读视图 + 创建授权");
    let declared = view.declared_effects();

    // 三个只读动词**一个声明都没有**（不声明 = 只读：默认方向通向更少的能力），
    // 因此也没有任何复核要求。
    for id in [LIST, READ, FIND] {
        assert!(
            declared.iter().all(|effect| effect.action().as_str() != id),
            "{id} 是只读：它不声明写效应"
        );
    }
    // 声明里是**三对**写能力（2026-10-26 起）：资料面那一对（授权来的：独占创建 ＋ 改文本）
    // 与目录级那一对（同样挂在授权上：建目录 ＋ 改名），各自配了复核；
    // 再加上**作品面**那一对（她自己那个 `projects/`：同样两件）——三个只读动词一条声明都没有。
    assert_eq!(declared.len(), 6, "三对写：资料面 ＋ 目录级 ＋ 作品面");
    for (action, verification) in [
        (CREATE_TEXT, "files_verify_text"),
        ("files_edit", "files_verify_edit"),
        ("files_mkdir", "files_verify_mkdir"),
        ("files_rename", "files_verify_rename"),
    ] {
        let one = declared
            .iter()
            .find(|effect| effect.action().as_str() == action)
            .unwrap_or_else(|| panic!("{action} 必须声明效果"));
        assert_eq!(one.class(), EffectClass::ExclusiveCreate);
        assert_eq!(
            one.verification().map(|id| id.as_str()),
            Some(verification),
            "有路可复核的写必须配复核（装配期按同一条判据拒绝 `write-without-verification`）"
        );
    }

    let mut os = MinimalOs::assemble(vec![Box::new(view)]).expect("装配只读视图 + 创建授权");

    // ⚠ **旧规矩 → 新规矩**（2026-09-26）：这里原来造一条"解除不了的未决"（`PendingEffects::pair`
    // 读不到原料 ⇒ fail-closed），断言**写类别在未决解除之前被拒**（分类
    // `pending-effect-unverified`），再拿一个空集合做对照。**未决效果门退役了**（门是指令，
    // 而世界只该给信息），因此那一整段没有了——留下的这一条把**门退役之后**的读数钉住：
    // 读类别与写类别**都**照常准入。

    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = os.scope(&mut expansion);
    assert_eq!(
        scope.execute(&agent, &operation(&run), &expand()).state(),
        ResultState::Success
    );

    // TASK_69：调用之前**先看格式**——展开之后新出现的动作不在"在场集合"里，
    // 要先各看一次（这是新形态下主体本来就要做的那一轮，不是绕过校验）。
    inspect_all(&mut scope, &agent, &run, &[LIST, READ, FIND, CREATE_TEXT]);

    // 读类别一律放行——**写类别也一样**：没有未决效果门了。
    for action in [
        list_after(None),
        read("alpha.txt"),
        find("alpha", None),
        create_text_action(),
    ] {
        assert_eq!(
            scope.admit(&agent, &run, &action),
            ActionAdmission::Admitted,
            "{} 照常准入（未决效果门已退役）",
            action.id().as_str()
        );
    }
    // 而"它是写"这件事照旧读得到——读的是**它自己的声明**（把声明收集成表的那一半已退役）。
    // 对照：同一份声明里，创建文本是写；列目录连声明都没有（不声明 = 只读 ⇒ 它不是写）。
    assert!(
        declared
            .iter()
            .any(|effect| { effect.action().as_str() == CREATE_TEXT && effect.class().is_write() }),
        "创建文本是写效应"
    );
    assert!(
        declared
            .iter()
            .all(|effect| effect.action().as_str() != LIST),
        "列目录没有写声明 ⇒ 按「不声明 = 只读」它不是写"
    );
}

const CREATE_TEXT: &str = "files_create";

fn create_text_action() -> Action {
    Action::with_params(
        ActionId::new(CREATE_TEXT),
        ActionParams::none()
            .with("name", "draft.txt")
            .with("text", "v1"),
    )
}

// ---------------------------------------------------------------------------
// 视图层：搜索面覆盖资料面与经历面，两边同一套脾气
// ---------------------------------------------------------------------------

/// 视图层：`files_grep` **也搜经历面**（2026-10-07 解冻），而手册仍是**一份文件**、不在查找面内。
///
/// 判据在理念层（`AGENT_PHILOSOPHY.md` §3/§4）："**查找**"是她在外部空间里的动作之一，系统在那里
/// 的职责只有四条（有事实来源、有权限边界、可恢复、不破坏原始历史）——**替她划定哪一面可查**
/// 正是 §4 第一条禁止的形状。与资料面**同一套脾气**：同一个动词、同一套参数、同形的读数、同一条
/// "没找到 ≠ 没有看"的纪律；只有扫描顺序随地址空间变（资料面按名字序，经历面按**最近的先**）。
#[test]
fn the_view_searches_the_history_directory_too() {
    let fixture = ViewFixture::new("view-find");
    let marker = "经历里的目标串";
    fixture.write_material("alpha.txt", "资料里没有那个串\n");
    fixture.environment(marker);
    let mut os =
        MinimalOs::with_history(&fixture.materials(), &fixture.env_root()).expect("装配只读视图");

    let agent = ViewFixture::agent();
    let run = RunId::new("run-view-find");
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = os.scope(&mut expansion);
    assert_eq!(
        scope.execute(&agent, &operation(&run), &expand()).state(),
        ResultState::Success
    );

    // ① 不给 `path`：默认还是**资料面**（那里没有那个串）——不静默把她整段过去扫一遍。
    let text = text_of(&scope.execute(&agent, &operation(&run), &find("目标串", None)));
    assert_eq!(
        text.lines().next(),
        Some("candidates = 0, scanned = 1, skipped = 0, truncated = false"),
        "{text}"
    );
    assert!(!text.contains(marker), "不给 path 时不该去经历里捞: {text}");

    // ② `path = history`：查得到，而且命中的地址是"活动/段 + 行号"（拿到就能读那一段）。
    let text = text_of(&scope.execute(&agent, &operation(&run), &find("目标串", Some(HISTORY))));
    assert_eq!(
        text.lines().next(),
        Some("candidates = 1, scanned = 1, skipped = 0, truncated = false"),
        "{text}"
    );
    let row = text.lines().nth(1).expect("一行候选");
    let cells: Vec<&str> = row.split('\t').collect();
    assert_eq!(
        cells[0], "history/run-history/000.txt",
        "候选给可读的地址：{row}"
    );
    assert!(cells[1].starts_with("line="), "命中给行号：{row}");
    assert!(cells[2].contains(marker), "片段里有那个串：{row}");

    // ③ 子树限定与地址形状：`history/<活动>` 同一结论；指到**段文件**上是类型不对；不存在的活动是不存在。
    let text = text_of(&scope.execute(
        &agent,
        &operation(&run),
        &find("目标串", Some("history/run-history")),
    ));
    assert_eq!(
        text.lines().next(),
        Some("candidates = 1, scanned = 1, skipped = 0, truncated = false"),
        "{text}"
    );
    let outcome = scope.execute(
        &agent,
        &operation(&run),
        &find("目标串", Some("history/run-history/000.txt")),
    );
    assert_eq!(
        outcome.failure_class(),
        Some(FailureClass::WrongKind.as_str()),
        "段文件是文件，不是子树"
    );
    let outcome = scope.execute(
        &agent,
        &operation(&run),
        &find("目标串", Some("history/nope")),
    );
    assert_eq!(
        outcome.failure_class(),
        Some(FailureClass::NotFound.as_str())
    );

    // 续列与续读在视图层同样成立：`history` 是根列表里的一个名字，`after` 可以用它。
    let root = text_of(&scope.execute(&agent, &operation(&run), &list_after(None)));
    assert!(
        root.starts_with("entries = 3, truncated = false, more = none"),
        "{root}"
    );
    assert_eq!(
        entry_names(&root),
        vec![
            "alpha.txt".to_string(),
            HISTORY.to_string(),
            GRANT_DIR.to_string()
        ]
    );
    let tail = text_of(&scope.execute(&agent, &operation(&run), &list_after(Some(HISTORY))));
    assert_eq!(entry_names(&tail), vec![GRANT_DIR.to_string()]);
    assert_eq!(more_of(&tail), "none");
    let outcome = scope.execute(&agent, &operation(&run), &list_after(Some("nope")));
    assert_eq!(
        outcome.failure_class(),
        Some(FailureClass::InvalidArgument.as_str())
    );

    // 经历段自带分段：对它给窗口参数明确失败，不静默忽略。
    let outcome = scope.execute(
        &agent,
        &operation(&run),
        &read_window("history/run-history/000.txt", Some("1"), None),
    );
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(
        outcome.failure_class(),
        Some(FailureClass::InvalidArgument.as_str())
    );
    // 不给窗口参数时，经历段照旧能整段读。
    let outcome = scope.execute(
        &agent,
        &operation(&run),
        &read("history/run-history/000.txt"),
    );
    assert_eq!(outcome.state(), ResultState::Success);
    assert!(text_of(&outcome).contains(marker), "经历段仍可整段读");
}

/// 视图层：只装资料（没有经历目录）时，`history` 按既有规则报"不存在"，不是"不在搜索面内"。
#[test]
fn the_materials_only_view_still_reports_history_as_absent() {
    let sandbox = Sandbox::new("view-materials-only");
    sandbox.write("alpha.txt", "alpha\n");
    let mut view = FileView::materials_only(sandbox.path()).expect("只装资料");

    let outcome = view.execute(
        &AgentId::new("agent-alpha"),
        &direct_operation(),
        &find("alpha", Some(HISTORY)),
    );
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(
        outcome.failure_class(),
        Some(FailureClass::NotFound.as_str())
    );
}
