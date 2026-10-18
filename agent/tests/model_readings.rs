//! 逐轮读数账的离线证明：**一行一次、缺的字段缺席、账在第一次写之前不存在、坏过能报出来**。
//!
//! 全部离线、全合成数据、只用测试自建的临时目录；一次网络都不发。
//!
//! ⚠ **这里证明不了的两件事**（如实列出来，不假装）：
//!
//! - **真厂商报不报那个字段**：本文件只用合成载荷证明"报了就读得到、没报就缺席"；
//! - **写失败之后的行为**：追加句柄一旦拿到就有效，"写不进"这一档**没有注入点**，因此
//!   "第一次失败之后不再尝试"只有代码形状、没有行为证明。本文件覆盖到的是**打不开**那一档
//!   （它发生在第一次要写的时候，有注入点：给一个不存在的目录）。

use std::fs;
use std::path::{Path, PathBuf};

use celira_agent::foundation::{RunId, Turn};
use celira_agent::model_readings::{MODEL_READINGS_FILE, ModelReadings, TurnReadings};

/// 自建临时沙盒：只在系统临时目录下创建，用完自己删掉。
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let unique = format!(
            "celira-model-readings-{}-{}-{}",
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

    /// 盘上那一份账的每一行（**原样**，不解析）。
    fn lines(&self) -> Vec<String> {
        let text = fs::read_to_string(self.root.join(MODEL_READINGS_FILE)).expect("账读得回来");
        text.lines().map(str::to_string).collect()
    }

    /// 那一份账今天在不在盘上。
    fn account_exists(&self) -> bool {
        self.root.join(MODEL_READINGS_FILE).exists()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn readings(run: &str, turn: u32, cached: Option<u64>) -> TurnReadings {
    TurnReadings {
        run: RunId::new(run),
        turn: Turn::from_number(turn).expect("轮次从 1 起"),
        wire: "chat-completions",
        prompt_tokens: Some(120 * u64::from(turn)),
        completion_tokens: Some(30),
        total_tokens: Some(150 * u64::from(turn)),
        cached_prompt_tokens: cached,
    }
}

/// 一次响应报了什么就记什么：**四个计数原样落盘**，一行一次。
#[test]
fn a_recorded_turn_is_one_line_carrying_what_the_response_reported() {
    let sandbox = Sandbox::new("two-turns");
    let (writer, log) = ModelReadings::channel(sandbox.path());

    writer.record(&readings("run-a", 1, None));
    writer.record(&readings("run-a", 2, Some(180)));

    let lines = sandbox.lines();
    assert_eq!(lines.len(), 2, "一次响应一行：{lines:#?}");

    let first: serde_json::Value = serde_json::from_str(&lines[0]).expect("一行是一个对象");
    assert_eq!(first["run"], "run-a");
    assert_eq!(first["turn"], 1);
    assert_eq!(first["wire"], "chat-completions");
    assert_eq!(first["prompt_tokens"], 120);
    assert_eq!(first["total_tokens"], 150);
    // **没报的那一格缺席**：不是 `null`、更不是 `0`——"没报"与"报了零"是两件事。
    assert!(
        !lines[0].contains("cached_prompt_tokens"),
        "没报的字段必须缺席，不能补零：{}",
        lines[0]
    );

    let second: serde_json::Value = serde_json::from_str(&lines[1]).expect("一行是一个对象");
    assert_eq!(second["turn"], 2);
    assert_eq!(second["cached_prompt_tokens"], 180);
    assert_eq!(log.first_failure(), None, "一次都没坏过");
}

/// 一次**什么都没报**的响应照样留一行：那一行如实缺着计数。
///
/// 理由：对翻文件的人来说，"这一轮问过一次模型"本身就是事实——没有它，一条断掉的轮次
/// 与"根本没问过"长得一模一样。
#[test]
fn a_turn_that_reported_nothing_still_leaves_a_line() {
    let sandbox = Sandbox::new("bare");
    let (writer, _log) = ModelReadings::channel(sandbox.path());
    writer.record(&TurnReadings {
        run: RunId::new("run-bare"),
        turn: Turn::FIRST,
        wire: "responses",
        prompt_tokens: None,
        completion_tokens: None,
        total_tokens: None,
        cached_prompt_tokens: None,
    });

    let lines = sandbox.lines();
    assert_eq!(lines.len(), 1);
    let row: serde_json::Value = serde_json::from_str(&lines[0]).expect("一行是一个对象");
    assert_eq!(row["run"], "run-bare");
    assert_eq!(row["wire"], "responses");
    for absent in [
        "prompt_tokens",
        "completion_tokens",
        "total_tokens",
        "cached_prompt_tokens",
    ] {
        assert!(
            !lines[0].contains(absent),
            "{absent} 必须缺席：{}",
            lines[0]
        );
    }
}

/// **账在第一次真的记一行之前不存在**——这条是拿一条真实门禁换来的。
///
/// 判据一句话：**没有凭据的时刻不得改动环境**（`manual_entry` 那条用例钉着它）。构造期先建一个
/// 空文件，正好违反那一条：那**是**改动。所以构造**不碰文件系统**，文件在第一次写时才出现。
#[test]
fn the_account_does_not_exist_until_something_is_actually_recorded() {
    let sandbox = Sandbox::new("lazy");
    let (writer, log) = ModelReadings::channel(sandbox.path());

    assert!(!sandbox.account_exists(), "构造期一个字节都不该落盘");
    assert_eq!(log.first_failure(), None, "还没写过，也谈不上坏");
    // 路径是**打算落在哪儿**，不是"它已经在那儿"。
    assert!(log.path().ends_with(MODEL_READINGS_FILE));

    writer.record(&readings("run-lazy", 1, None));
    assert!(sandbox.account_exists(), "写第一行的时候它才出现");
    assert_eq!(sandbox.lines().len(), 1);
}

/// 打不开是**明确分类**，不是"这份账空着"：调用方拿得到是哪一类、什么形状。
///
/// 它**发现得晚**（第一次要写的时候）——那正是"构造不碰文件系统"的代价，如实钉在这里。
#[test]
fn an_unopenable_account_is_a_classified_failure_not_an_empty_one() {
    let sandbox = Sandbox::new("unopenable");
    let missing = sandbox.path().join("没有这个目录");
    let (writer, log) = ModelReadings::channel(&missing);

    // 还没写：什么都没发生（连失败都还没有——因为还没有人去开它）。
    assert_eq!(log.first_failure(), None);

    writer.record(&readings("run-x", 1, None));

    let error = log.first_failure().expect("第一次要写 ⇒ 打不开");
    assert_eq!(error.as_str(), "open", "分类是稳定词");
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    // 读出来是给操作者看的一句：类别在前，形状在后（**不带路径、不带凭据**）。
    let rendered = error.to_string();
    assert!(rendered.starts_with("open ("), "{rendered}");

    // 坏过之后不再重复尝试：再记一次也不会改变结论（也不会 panic）。
    writer.record(&readings("run-x", 2, None));
    assert_eq!(log.first_failure(), Some(error), "只留第一条");
}

/// **负空间（源码级）**：这份账不是任何执行路径的输入。
///
/// 判据一句话——**`recovery` / `store` / `core` / `foundation` / `file_view` 里一个都不许出现它**。
/// 前三者是"恢复只认已确认事实"，`file_view` 是"主体读不到它"（派生地址白名单只有 `history/`
/// 与 `manual.md`）。这一条量的是**源码里没有那条边**，不是"跑了没出问题"。
#[test]
fn nothing_on_the_execution_path_reads_this_account() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let forbidden_in = [
        "recovery.rs",
        "store.rs",
        "store/environment.rs",
        "store/format.rs",
        "store/projection.rs",
        "store/parts.rs",
        "core.rs",
        "foundation.rs",
        "file_view.rs",
        "file_app.rs",
        "diagnostics.rs",
    ];
    for name in forbidden_in {
        let path = root.join(name);
        let source = fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!("{name} 读不到（名单过期了？）: {error}");
        });
        assert!(
            !source.contains("model_readings") && !source.contains(MODEL_READINGS_FILE),
            "{name} 提到了逐轮读数账：它不是任何执行路径的输入，也不是主体能读到的地址"
        );
    }

    // 反例自检：这个扫描器认得出真违规（否则它就是空的）。
    let probe = "use crate::model_readings::ModelReadings;";
    assert!(probe.contains("model_readings"), "扫描器必须认得出真引用");
}
