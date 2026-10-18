//! TASK_19 验收：**上下文占用那一格**——状态行的第三段（`context = 最近一次报告 / 宿主声明的窗口`，
//! 单位词见 `status_bar::CONTEXT_UNIT`；TASK_74 起它的标签是 `context`）。
//!
//! 这一份文件证明的是"屏幕这道墙可见"这件事的三条判据，全部在进程内把形状钉死：
//!
//! 1. **段形状**：有报告 ⇒ `context = <占用>/<窗口> <单位>`；没有报告 ⇒ `context = unknown/<窗口> <单位>`；
//!    固定钟 + 固定 cell 下**逐字节稳定**（判据同 TASK_18）；
//! 2. **滞后一回合**：某一次响应里报告的占用**只出现在下一轮**状态行里——本轮的读数在响应之前
//!    就形成了，因此本轮行不可能显示本轮将要产生的占用；
//! 3. **窗口是宿主声明**：分母只随声明变，且**没有"未声明"分支**——状态行永远有那一格。
//!
//! 真实适配层那一半（厂商 `usage` 原文 → cell → 下一轮状态行）由 `tests/provider_wire.rs` 用
//! 真实 HTTP 回环证明；适配层自身的宽容读取由 `src/provider_adapter.rs` 的单元用例证明；
//! "只有适配层写 cell"由 `tests/minimal_agent.rs` 的源码守卫证明。
//!
//! 明确**不**声称：没有跑公网模型；屏幕读数**不**参与任何判断（预算执法、截断、强制收工都不做
//! ——仪表不是刹车）；不持久化、不进事实、不进失败分类。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use celira_agent::clock::{ClockHandle, LocalOffset, format_utc};
use celira_agent::composition::{RunSession, SessionDeclarations, SessionDrive, SessionRequest};
use celira_agent::context_reading::{ContextReading, ContextReadingWriter};
use celira_agent::core::{
    Limits, ModelError, ModelFailureClass, ModelOutput, ModelPort, RunInput, TraceEntry, View,
};
use celira_agent::foundation::{AgentId, RunId, SubjectEnding};
use celira_agent::store::Environment;

const AGENT: &str = "agent-reading";
const RUN: &str = "run-reading";

/// 一块注入内容：标签 + 正文。
type Block = (String, String);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        // 目录名**必须带进程号**：测试二进制是一份一个进程，而计数器只在进程内唯一 ⇒
        // 不带进程号的两份并行（或两个写者同时跑）会算出同一批路径、**互相删目录**。
        let mut path = std::env::temp_dir();
        path.push(format!(
            "celira-task19-reading-{}-{label}-{n}",
            std::process::id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("清掉同名残留目录");
        }
        std::fs::create_dir_all(&path).expect("建立测试目录");
        Self { path }
    }

    fn env_root(&self) -> PathBuf {
        self.path.join("env")
    }

    fn materials(&self) -> PathBuf {
        let materials = self.path.join("materials");
        std::fs::create_dir_all(&materials).expect("建资料目录");
        materials
    }

    fn trace_path(&self) -> PathBuf {
        self.env_root().join("trace.ndjson")
    }

    fn environment(&self) -> Environment {
        Environment::create(&self.env_root(), agent_id()).expect("建立身份环境")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn agent_id() -> AgentId {
    AgentId::new(AGENT)
}

/// 用例装配用的时间来源：固定钟（离线、可逐字节断言）。
fn fixed_clock() -> ClockHandle {
    ClockHandle::fixed_at_unix_seconds(0)
}

/// 一条状态行**应当**长什么样（与 `status_bar.rs` 的格式化点对齐，用来判"逐字节稳定"）。
///
/// ⚠ `time` 那一格带**本机偏移**（`LocalOffset::of_machine`，跑用例的机器说了算）⇒ 对照写法
/// 用**同一个机器事实**拼出来，但**不调用**被测的那个格式化点：先取 UTC 的形状，再按同一个
/// 偏移把结尾的 `Z` 换掉（偏移读不出来就保持 `Z`）。
fn status_line(used: u32, limit: u32, reported: &str, window: u32) -> String {
    format!(
        "time = {}\nturn = {used}/{limit}\ncontext = {reported}/{window} token\nsince = unknown",
        expected_time(SystemTime::UNIX_EPOCH)
    )
}

/// 同一个瞬间在**这台机器上**的可读形状（用例侧独立拼法）。
///
/// ⚠ 偏移要**同时**改两件事：把时刻挪过去（`at ＋ 偏移`）**再**把结尾的 `Z` 换成 `±hh:mm`。
/// 只换后缀、不挪时刻，得到的是"贴着偏移标签的 UTC 时刻"——那正好是一个**看起来对、其实错**
/// 的形状（`1970-01-01T00:00:00+08:00` 与她真正看到的 `…T08:00:00+08:00` 差八小时）。
fn expected_time(at: SystemTime) -> String {
    match LocalOffset::of_machine() {
        LocalOffset::Minutes(minutes) => {
            let shifted = if minutes >= 0 {
                at + Duration::from_secs(minutes as u64 * 60)
            } else {
                at - Duration::from_secs((-minutes) as u64 * 60)
            };
            format_utc(shifted).replace('Z', &offset_suffix(minutes))
        }
        LocalOffset::Unreadable => format_utc(at),
    }
}

/// `±hh:mm` 后缀（用例侧独立拼法）。
fn offset_suffix(minutes: i32) -> String {
    let sign = if minutes < 0 { '-' } else { '+' };
    format!("{sign}{:02}:{:02}", minutes.abs() / 60, minutes.abs() % 60)
}

/// 脚本模型替身；它还可以**替身适配层那一步**：在某一次调用里把占用写进 cell（写点与适配层相同，
/// 都在"响应已经形成"之后）。
struct ScriptedModel {
    steps: VecDeque<ModelOutput>,
    /// 第几次调用时报告（从 1 数）；`None` = 从不报告。
    report_at: Option<usize>,
    reporter: Option<ContextReadingWriter>,
    reported: u64,
    /// 每次调用看到的（轮次，轨迹里注入的块）。
    seen: Vec<(u32, Vec<Block>)>,
}

impl ScriptedModel {
    fn new(steps: Vec<ModelOutput>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            report_at: None,
            reporter: None,
            reported: 0,
            seen: Vec::new(),
        }
    }

    /// 在第 `call` 次调用时把 `value` 报告给 cell（替身适配层的写点）。
    fn reporting_at(mut self, call: usize, value: u64, reporter: ContextReadingWriter) -> Self {
        self.report_at = Some(call);
        self.reported = value;
        self.reporter = Some(reporter);
        self
    }
}

impl ModelPort for ScriptedModel {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        let blocks = view
            .trace()
            .iter()
            .filter_map(|entry| match entry {
                TraceEntry::Injection { batch, .. } => Some(
                    batch
                        .blocks()
                        .iter()
                        .map(|block| (block.label().to_string(), block.text().to_string()))
                        .collect::<Vec<_>>(),
                ),
                TraceEntry::Refused { .. }
                | TraceEntry::Observation(_)
                | TraceEntry::ModelResponse(_) => None,
            })
            .flatten()
            .collect();
        self.seen.push((view.turn().number(), blocks));

        let output = self.steps.pop_front().ok_or(ModelError {
            class: ModelFailureClass::ContractViolation,
        })?;

        // 适配层那一步的替身：**响应已经形成之后**才把占用记进 cell，并记下是哪一段报的。
        if self.report_at == Some(self.seen.len())
            && let Some(reporter) = &self.reporter
        {
            reporter.report(view.run(), self.reported);
        }
        Ok(output)
    }
}

/// 打开一次会话（生产入口，只换时间来源与窗口声明）。
fn open<'a>(
    dir: &TempDir,
    environment: &'a mut Environment,
    window: u32,
    reading: ContextReading,
    limits: u32,
) -> RunSession<'a> {
    open_as(dir, environment, RUN, window, reading, limits)
}

/// 同 [`open`]，只是**点名是哪一段**（跨段那一条用例要两段共用一个 cell）。
fn open_as<'a>(
    dir: &TempDir,
    environment: &'a mut Environment,
    run: &str,
    window: u32,
    reading: ContextReading,
    limits: u32,
) -> RunSession<'a> {
    let materials = dir.materials();
    let declared = agent_id();
    RunSession::open(
        &materials,
        environment,
        SessionRequest {
            declarations: SessionDeclarations::default(),
            declared_agent: &declared,
            run: RunId::new(run),
            limits: Limits::new(limits),
            input: RunInput::text("开场输入").expect("在上限内"),
            clock: fixed_clock(),
            context_window: window,
            reading,
        },
    )
    .expect("会话可以打开")
}

/// 某一次视野里 `status-bar` 那一段的正文（最后一条 = 那一轮的读数）。
fn reading_at(model: &ScriptedModel, index: usize) -> String {
    let (_, blocks) = &model.seen[index];
    blocks
        .iter()
        .rev()
        .find(|(label, _)| label == "status-bar")
        .map(|(_, text)| text.clone())
        .expect("每次调用都看得见状态行")
}

// ---------------------------------------------------------------------------
// 判据 1 + 2：段形状与"滞后一回合"
// ---------------------------------------------------------------------------

#[test]
fn a_report_written_during_one_turn_shows_up_in_the_next_turn_only() {
    let dir = TempDir::new("lag");
    let mut environment = dir.environment();
    let (writer, cell) = ContextReading::channel();
    let mut session = open(&dir, &mut environment, 1_000, cell.clone(), 4);

    // 第 1 轮响应里报告 4210（替身适配层的写点），第 2 轮再报告 9999。
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等"), ModelOutput::say("再等")])
        .reporting_at(1, 4_210, writer.clone());
    let mut second =
        ScriptedModel::new(vec![ModelOutput::say("再等")]).reporting_at(1, 9_999, writer.clone());

    assert!(session.drive(&mut model).expect("驱动").is_unsettled());
    assert!(session.drive(&mut second).expect("驱动").is_unsettled());

    // 第 1 轮：还没有任何报告 ⇒ unknown（本轮行在响应之前就形成了）。
    assert_eq!(
        reading_at(&model, 0),
        status_line(1, 4, "unknown", 1_000),
        "本轮的读数不可能包含本轮响应里才产生的占用"
    );
    // 第 2 轮：显示的是第 1 轮报告的 4210——**滞后一回合**，而且不是第 2 轮自己的 9999。
    let second_reading = reading_at(&second, 0);
    assert!(
        second_reading.contains("context = 4210/1000 token"),
        "第 2 轮必须显示第 1 轮报告的读数: {second_reading}"
    );
    assert!(
        !second_reading.contains("9999"),
        "本轮响应里的读数不得出现在本轮行里: {second_reading}"
    );
    // 第 2 轮自己报告的 9999 也滞后到"下一次"——它没有被这一轮的行走漏。
    assert_eq!(
        cell.latest_for(&RunId::new(RUN)),
        Some(9_999),
        "cell 里存的是这一段最近一次报告"
    );
}

/// **一段的读数不许出现在另一段里**（2026-10-10 在场那一刀）。
///
/// 这一格从前**不认段**：它只存"最近一个数"。于是一段在场跨多声铃之后，一段的**开场那一行**
/// 会显示**上一段**最后一次调用的占用——而那一行的标签是 `context = 这一场送进模型的内容量`。
/// **那是拿另一段的数说这一段的事。**
///
/// 判据：第一段报过 4210；第二段**开场那一行**必须读作 `unknown`——对这一段而言，
/// "上一段报过"与"还没有人报过"确实是同一件事。
#[test]
fn a_report_from_another_segment_is_never_shown_in_this_one() {
    let dir = TempDir::new("cross-segment");
    let mut environment = dir.environment();
    let (writer, cell) = ContextReading::channel();

    // 第一段：报 4210，然后收场。
    {
        let mut session = open_as(&dir, &mut environment, RUN, 1_000, cell.clone(), 4);
        let mut model = ScriptedModel::new(vec![
            ModelOutput::say("等"),
            ModelOutput::finish(SubjectEnding::Completed),
        ])
        .reporting_at(1, 4_210, writer);
        assert!(session.drive(&mut model).expect("驱动").is_unsettled());
        assert!(matches!(
            session.drive(&mut model).expect("驱动"),
            SessionDrive::Ended(_)
        ));
    }
    assert_eq!(
        cell.latest_for(&RunId::new(RUN)),
        Some(4_210),
        "第一段那一格确实记下了"
    );

    // 第二段（新的一段、同一个 cell）：开场必须读 unknown，**不许**带出第一段的 4210。
    let mut session = open_as(&dir, &mut environment, "run-reading-2", 1_000, cell, 2);
    let mut model = ScriptedModel::new(vec![ModelOutput::say("接着来")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let opening = reading_at(&model, 0);
    assert_eq!(
        opening,
        status_line(1, 2, "unknown", 1_000),
        "另一段的读数不许进这一段的开场"
    );
    assert!(
        !opening.contains("4210"),
        "上一段的数一个字都不许来: {opening}"
    );
}

#[test]
fn the_context_segment_reads_unknown_until_a_report_arrives() {
    let dir = TempDir::new("unknown");
    let mut environment = dir.environment();
    let (_, cell) = ContextReading::channel();
    let mut session = open(&dir, &mut environment, 250_000, cell, 3);
    let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Completed)]);

    assert!(matches!(
        session.drive(&mut model).expect("驱动"),
        SessionDrive::Ended(_)
    ));

    // 开场那一轮与本次唯一一轮都读作 unknown：真数只能来自厂商响应，这一份会话里没有过。
    assert_eq!(reading_at(&model, 0), status_line(1, 3, "unknown", 250_000));
    let texts: Vec<Block> = model.seen[0].1.clone();
    assert!(
        texts
            .iter()
            .all(|(label, text)| label != "status-bar" || !text.contains("context = 0/")),
        "没有报告时不得编一个 0: {texts:?}"
    );
}

// ---------------------------------------------------------------------------
// 判据 1：固定钟 + 固定 cell ⇒ 逐字节稳定
// ---------------------------------------------------------------------------

#[test]
fn the_same_input_at_the_same_instant_is_byte_identical_with_a_reported_reading() {
    let first = TempDir::new("same-a");
    let second = TempDir::new("same-b");

    for dir in [&first, &second] {
        let mut environment = dir.environment();
        let (writer, cell) = ContextReading::channel();
        let mut session = open(dir, &mut environment, 1_000, cell, 4);
        let mut model = ScriptedModel::new(vec![
            ModelOutput::say("等"),
            ModelOutput::finish(SubjectEnding::Completed),
        ])
        .reporting_at(1, 4_210, writer);
        assert!(session.drive(&mut model).expect("驱动").is_unsettled());
        assert!(matches!(
            session.drive(&mut model).expect("驱动"),
            SessionDrive::Ended(_)
        ));
    }

    let a = std::fs::read(first.trace_path()).expect("读第一份经历");
    let b = std::fs::read(second.trace_path()).expect("读第二份经历");
    assert_eq!(a, b, "固定钟 + 固定 cell 下必须是逐字节相同的经历");
    let text = String::from_utf8(a).expect("是 UTF-8");
    assert!(
        text.contains("context = unknown/1000 token"),
        "首拍 unknown: {text}"
    );
    assert!(
        text.contains("context = 4210/1000 token"),
        "次拍显示报告值: {text}"
    );
}

// ---------------------------------------------------------------------------
// 判据 3：窗口是宿主声明（分母只随声明变；没有"未声明"分支）
// ---------------------------------------------------------------------------

#[test]
fn the_window_is_whatever_the_host_declared() {
    for window in [1u32, 4_096, 1_000_000] {
        let dir = TempDir::new("window");
        let mut environment = dir.environment();
        let (_, cell) = ContextReading::channel();
        let mut session = open(&dir, &mut environment, window, cell, 2);
        let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Completed)]);
        assert!(matches!(
            session.drive(&mut model).expect("驱动"),
            SessionDrive::Ended(_)
        ));

        let reading = reading_at(&model, 0);
        assert_eq!(reading, status_line(1, 2, "unknown", window));
        assert!(
            reading.contains(&format!("/{window}")),
            "分母就是宿主声明的那个数: {reading}"
        );
    }
}

// ---------------------------------------------------------------------------
// 词表纪律：主体的视野里不得出现厂商 wire 词（TASK_74 起那个单位词是**故意给主体**的）
// ---------------------------------------------------------------------------

#[test]
fn the_view_never_contains_vendor_words() {
    let dir = TempDir::new("words");
    let mut environment = dir.environment();
    let (writer, cell) = ContextReading::channel();
    let mut session = open(&dir, &mut environment, 1_000, cell.clone(), 4);
    let mut model = ScriptedModel::new(vec![
        ModelOutput::say("等"),
        ModelOutput::finish(SubjectEnding::Completed),
    ])
    .reporting_at(1, 4_210, writer);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());
    assert!(matches!(
        session.drive(&mut model).expect("驱动"),
        SessionDrive::Ended(_)
    ));
    let view_text: String = model
        .seen
        .iter()
        .flat_map(|(_, blocks)| blocks.iter().map(|(_, text)| text.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    drop(session);

    let trace = std::fs::read_to_string(dir.trace_path()).expect("读经历");
    // **厂商词的形态**照旧一个都不许来（wire 字段名）。
    //
    // TASK_74 把最后一个条目（那个单位词的**裸词**）从这里拿掉了：它现在**故意**进主体的视野
    // （读数行的单位 ＋ 开场说明书里"被定义的单位词"），那是给主体的词表、不是厂商 wire——
    // 厂商那边它叫 `prompt_tokens` / `input_tokens`，仍在下面这张表里。**不是放宽**：
    // 紧接着那条断言把"主体看得见的只有那个单位词本身、且只在那两处"钉住。
    for forbidden in ["usage", "total_tokens", "prompt_tokens", "input_tokens"] {
        assert!(
            !view_text.contains(forbidden),
            "主体的视野里不得出现厂商词 {forbidden}: {view_text}"
        );
        assert!(
            !trace.contains(forbidden),
            "经历里不得出现厂商词 {forbidden}: {trace}"
        );
    }
    // 那个单位词：**只在以 `context = ` 开头的那两行里**出现（每轮读数一行 ＋ 说明书里那行），
    // 别的地方一次都不许有——"主体能看见的只有那个单位词本身，且只在给它用的地方"。
    let context_lines = view_text
        .lines()
        .filter(|line| line.starts_with("context = "))
        .count();
    assert!(context_lines > 0, "至少要有读数行: {view_text}");
    assert_eq!(
        view_text.matches("token").count(),
        context_lines,
        "单位词只出现在 `context = ` 那两处（读数行 / 说明书）: {view_text}"
    );
}
