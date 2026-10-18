//! TASK_25 验收：宿主命令 `agent_host_acknowledge`（暂停的**显式确认**入口）。
//!
//! 它证明的是**一件事**：那条"走不通的路"接通了，而且它**说的每一句话都是真的**。
//! 因此这一份用例的重点是：
//!
//! - **证据面先于写入**：盘上记录的原因先打出来，加了 `--confirm` 才写；
//! - **写什么**：**恰好一条** `acknowledged` 环境级标记；旧标记**不删**（事实不可删）；
//!   ⚠ **2026-09-26 改**：这条标记的取值原来叫 `resumed`——那是"接着跑同一段活动"的说法，
//!   而 `resume` 那条路已退役；它真正说的是"**这条异常已经被看过了**"，因此改名。
//! - **范围的真实边界**：确认**只覆盖此前**（此前的未收尾痕迹不再拦、但**没有被解决**；
//!   之后**新**留下的未收尾痕迹照样拦）；
//! - **退出码是读数语义**（0/1/2/3/4 各是一条结论）；
//! - 命令前后：不该写的时候两份存储与身份文件**逐字节不变**。
//!
//! 明确**不**声称：它能修存储、能继续/重放任何活动、能解决那些活动（它只让它们不再拦新活动）。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use celira_agent::foundation::{AgentId, RunId};
use celira_agent::store::TracePayload;
mod common;
use common::{binary, create_environment, open_environment};

const AGENT: &str = "agent-ack";

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

/// 跑一次确认命令。`arguments` 里**不含**环境根（它被放在最前面，与产品用法一致）。
fn acknowledge(root: &Path, arguments: &[&str]) -> Output {
    let mut command = Command::new(binary("agent_host_acknowledge"));
    command.arg(root);
    for argument in arguments {
        command.arg(argument);
    }
    command
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env("NO_PROXY", "127.0.0.1,localhost");
    command.output().expect("启动 agent_host_acknowledge")
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

/// 报告里某一行的取值（找不到就 panic，避免断言悄悄失效）。
fn field(text: &str, key: &str) -> String {
    let needle = format!("{key}=");
    text.lines()
        .find_map(|line| line.strip_prefix(&needle))
        .unwrap_or_else(|| panic!("报告里没有 {key}=: {text}"))
        .to_string()
}

fn empty_environment(root: &Path) {
    drop(create_environment(root, AgentId::new(AGENT)));
}

/// 一个**未收尾**的活动（经历侧：登记了、没有收场）。它就是"此前的痕迹"。
fn add_unfinished_run(root: &Path, run: &str) {
    let mut environment = open_environment(root, AgentId::new(AGENT));
    environment
        .append_trace(&RunId::new(run), TracePayload::RunStarted { origin: None })
        .expect("经历：登记");
}

/// 记下这次暂停（`save-failed` 是真实系统里那条原因的形状）。
fn pause(root: &Path, run: &str, reason: &str) {
    let mut environment = open_environment(root, AgentId::new(AGENT));
    environment
        .mark_paused(&RunId::new(run), reason)
        .expect("写下暂停标记");
}

/// 两份存储 + 身份文件的字节快照（"零写入"的读数）。
fn footprint(root: &Path) -> Vec<(String, Vec<u8>)> {
    ["facts.ndjson", "trace.ndjson", "identity.json"]
        .iter()
        .map(|name| {
            let path = root.join(name);
            (name.to_string(), std::fs::read(&path).unwrap_or_default())
        })
        .collect()
}

fn trace_records(root: &Path) -> usize {
    std::fs::read_to_string(root.join("trace.ndjson"))
        .expect("读经历侧")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
}

// ---------------------------------------------------------------------------
// 1. 证据面先于写入
// ---------------------------------------------------------------------------

#[test]
fn the_preview_shows_the_recorded_reason_and_writes_nothing() {
    let dir = tempdir("preview");
    empty_environment(&dir);
    pause(&dir, "run-1", "save-failed:run-start");
    let before = footprint(&dir);

    let output = acknowledge(&dir, &["--agent", AGENT, "--run", "run-1"]);
    let text = stdout(&output);
    assert_eq!(code(&output), 3, "没确认就是'需要用户动作': {text}");
    assert_eq!(
        field(&text, "paused-reason"),
        "save-failed:run-start",
        "原因必须**逐字符**照盘上记录的打出来"
    );
    assert!(text.contains("[preview]"), "预览模式要明说这是预览: {text}");
    assert!(
        !text.contains("[acknowledged]"),
        "没给 --confirm 就不该有'已确认'那一节: {text}"
    );
    assert_eq!(footprint(&dir), before, "预览模式一个字节都不许写");
}

// ---------------------------------------------------------------------------
// 2 & 3. 确认写一条记录，准入真的变了
// ---------------------------------------------------------------------------

#[test]
fn confirming_records_exactly_one_acknowledged_marker_and_liberates_admission() {
    let dir = tempdir("confirm");
    empty_environment(&dir);
    add_unfinished_run(&dir, "run-1");
    pause(&dir, "run-2", "save-failed:run-start");

    // 确认之前：准入是"已暂停"。
    let environment = open_environment(&dir, AgentId::new(AGENT));
    assert_eq!(
        environment.admission().expect("读准入").as_str(),
        "paused",
        "夹具必须真的处在暂停态"
    );
    drop(environment);

    let before = trace_records(&dir);
    let output = acknowledge(&dir, &["--agent", AGENT, "--run", "run-2", "--confirm"]);
    let text = stdout(&output);
    assert_eq!(code(&output), 0, "确认成功: {text}");
    assert_eq!(
        field(&text, "admission-after"),
        "ready",
        "确认之后准入必须真的变: {text}"
    );
    assert!(
        text.contains("并没有被解决"),
        "必须说出'那些痕迹没有被解决': {text}"
    );
    assert!(!text.contains("已恢复"), "不许声称环境已恢复: {text}");

    // 经历侧**只多一条**记录，而且它就是那条 `acknowledged` 标记（序号与回执一致）。
    assert_eq!(trace_records(&dir), before + 1, "确认只该写一条记录");
    let reported: u64 = field(&text, "acknowledged-seq")
        .parse()
        .expect("序号是数字");
    let last = std::fs::read_to_string(dir.join("trace.ndjson"))
        .expect("读经历侧")
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .expect("有记录")
        .to_string();
    assert!(
        last.contains("\"acknowledged\"") && last.contains(&format!("\"seq\":{reported}")),
        "最后一条记录必须就是那条 acknowledged 标记（seq={reported}）: {last}"
    );

    // 旧标记**仍在**（事实不可删）。
    let trace = std::fs::read_to_string(dir.join("trace.ndjson")).expect("读经历侧");
    assert!(
        trace.contains("\"paused\"") && trace.contains("save-failed:run-start"),
        "旧标记不许被删掉"
    );

    // 确认之后重新打开：准入确实是 ready（不是内存里的假设）。
    let environment = open_environment(&dir, AgentId::new(AGENT));
    assert_eq!(environment.admission().expect("读准入").as_str(), "ready");
}

// ---------------------------------------------------------------------------
// 4. 范围的边界：只覆盖此前；之后的新痕迹照样拦
// ---------------------------------------------------------------------------

#[test]
fn the_acknowledgement_covers_only_what_came_before_it() {
    let dir = tempdir("scope");
    empty_environment(&dir);
    add_unfinished_run(&dir, "run-before");
    pause(&dir, "run-before", "save-failed:run-start");

    // 此前的未收尾痕迹 + 暂停：拦。
    let environment = open_environment(&dir, AgentId::new(AGENT));
    assert_eq!(environment.admission().expect("读准入").as_str(), "paused");
    drop(environment);

    let output = acknowledge(
        &dir,
        &["--agent", AGENT, "--run", "run-before", "--confirm"],
    );
    assert_eq!(code(&output), 0);
    let environment = open_environment(&dir, AgentId::new(AGENT));
    assert_eq!(
        environment.admission().expect("读准入").as_str(),
        "ready",
        "确认覆盖**此前**的痕迹：那个未收尾的活动不再拦"
    );
    drop(environment);

    // **之后**再留下一个未收尾的活动：照样拦（范围只朝前）。
    add_unfinished_run(&dir, "run-after");
    let environment = open_environment(&dir, AgentId::new(AGENT));
    let admission = environment.admission().expect("读准入");
    assert_eq!(admission.as_str(), "unfinished");
    assert!(
        format!("{admission:?}").contains("run-after"),
        "拦路的是**新的**那个活动: {admission:?}"
    );
}

// ---------------------------------------------------------------------------
// 5 & 6. 没有暂停 / 环境打不开：零写入
// ---------------------------------------------------------------------------

#[test]
fn an_environment_without_a_pause_is_never_handed_a_fake_acknowledgement() {
    let dir = tempdir("nopause");
    empty_environment(&dir);
    let before = footprint(&dir);

    let output = acknowledge(&dir, &["--agent", AGENT, "--run", "run-1", "--confirm"]);
    let text = stdout(&output);
    assert_eq!(code(&output), 4, "没有暂停可确认: {text}");
    assert_eq!(field(&text, "paused-reason"), "none");
    assert!(
        !text.contains("[acknowledged]"),
        "不许制造'我确认过了'的假记录: {text}"
    );
    assert_eq!(footprint(&dir), before, "零写入");

    // 确认过之后再跑一次：读数一致（不重复确认）。
    let dir2 = tempdir("twice");
    empty_environment(&dir2);
    pause(&dir2, "run-1", "save-failed:run-start");
    assert_eq!(
        code(&acknowledge(
            &dir2,
            &["--agent", AGENT, "--run", "run-1", "--confirm"]
        )),
        0
    );
    let before = footprint(&dir2);
    let again = acknowledge(&dir2, &["--agent", AGENT, "--run", "run-1", "--confirm"]);
    assert_eq!(
        code(&again),
        4,
        "同一条暂停不该被确认两次（第一次之后已无暂停）"
    );
    assert_eq!(footprint(&dir2), before, "Zero writes on the second run");
}

#[test]
fn an_unreadable_environment_is_never_written() {
    // 根不存在。
    let missing = tempdir("missing").join("not-here");
    let output = acknowledge(&missing, &["--agent", AGENT, "--run", "run-1", "--confirm"]);
    assert_eq!(code(&output), 2, "{}", stdout(&output));
    assert!(stdout(&output).contains("environment-unreadable"));

    // 身份与盘上不一致。
    let dir = tempdir("wrong-agent");
    empty_environment(&dir);
    pause(&dir, "run-1", "save-failed:run-start");
    let before = footprint(&dir);
    let output = acknowledge(
        &dir,
        &["--agent", "someone-else", "--run", "run-1", "--confirm"],
    );
    assert_eq!(code(&output), 2, "{}", stdout(&output));
    assert_eq!(footprint(&dir), before, "打不开就一个字节都不许写");
}

// ---------------------------------------------------------------------------
// 7. 参数与帮助
// ---------------------------------------------------------------------------

#[test]
fn missing_arguments_and_the_help_path_are_readings_not_writes() {
    let dir = tempdir("usage");
    empty_environment(&dir);
    pause(&dir, "run-1", "save-failed:run-start");
    let before = footprint(&dir);

    for arguments in [
        vec![],
        vec!["--agent", AGENT],
        vec!["--run", "run-1"],
        vec!["--agent", AGENT, "--run", "run-1", "--wat"],
    ] {
        let output = acknowledge(&dir, &arguments);
        assert_eq!(code(&output), 1, "参数错: {arguments:?}");
        assert!(
            stderr(&output).contains("用法"),
            "参数错要把用法打到 stderr: {arguments:?}"
        );
        assert!(stdout(&output).is_empty(), "参数错时 stdout 不该有报告");
    }
    assert_eq!(footprint(&dir), before, "参数错零写入");

    // `--help`：只打印用法，退出码与状态命令同一约定（0）。
    let help = Command::new(binary("agent_host_acknowledge"))
        .arg("--help")
        .output()
        .expect("跑帮助");
    assert_eq!(code(&help), 0);
    assert!(stdout(&help).contains("用法:"));
    assert_eq!(footprint(&dir), before, "帮助路径零写入");
}

// ---------------------------------------------------------------------------
// 8. 点名一个"从来没有存在过"的活动也可以确认
// ---------------------------------------------------------------------------

/// `save-failed:run-start` 那一次可能**一个活动记录都没有**（失败发生在登记之前），
/// 因此 `--run` 只要求"显式点名"，不要求那个活动存在——否则命令正好在最需要它的时候用不了。
#[test]
fn a_run_name_that_never_existed_is_still_accepted() {
    let dir = tempdir("nofacts");
    empty_environment(&dir);
    pause(&dir, "run-1", "save-failed:run-start");

    let output = acknowledge(
        &dir,
        &[
            "--agent",
            AGENT,
            "--run",
            "run-never-registered",
            "--confirm",
        ],
    );
    let text = stdout(&output);
    assert_eq!(code(&output), 0, "{text}");
    assert_eq!(field(&text, "run"), "run-never-registered", "照实回显");
    assert_eq!(field(&text, "admission-after"), "ready");
}

fn tempdir(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "celira-ack-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("时钟")
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).expect("建临时目录");
    path
}
