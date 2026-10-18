//! TASK_16 验收：产品宿主最小切片 `agent_host_status`（只读状态命令）。
//!
//! 它证明的是**呈现**这件事本身：一条命令把已确认事实读成"它是谁 / 现在什么状态 / 我该不该出手"，
//! 而且**一个字节都不写**。因此这一份用例的重点不是"输出好看"，而是：
//!
//! - 三节读数的**内容**与盘上事实逐项对得上（含 TASK_15 的 `failure=<class>`）；
//! - `next` 那一行是**判定类目 × 准入**的机械映射（表驱动 + 反例）；
//! - 退出码是**读数语义**（0/1/2/3 各是一条结论，不是"成功/失败"）；
//! - 命令前后：两份存储、身份文件与目录成员**逐字节不变**。
//!
//! 明确**不**声称：命令的"想法"（它不做编排判断，只把类目翻译成一句人话）；watch / 常驻 /
//! 推送 / 实时性（本刀没有，也不假装有）；`--materials` 的真实取值（资料根不在环境里，
//! 建议里给的是占位符）。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use celira_agent::core::{HttpStatusDetail, ModelFailureClass, TransportDetail};
use celira_agent::foundation::{
    AgentId, Outcome, Record, ResultState, RunId, SubjectEnding, SystemEnding,
};
use celira_agent::store::TracePayload;
mod common;
use common::{binary, create_environment, open_environment};

const AGENT: &str = "agent-host";
const RUN: &str = "run-1";

// ---------------------------------------------------------------------------
// 夹具：跑命令、造环境、对照文件
// ---------------------------------------------------------------------------

/// 跑一次状态命令，返回原始输出（调用方按需读 stdout / stderr / 退出码）。
fn status(root: &Path, arguments: &[&str]) -> Output {
    let mut command = Command::new(binary("agent_host_status"));
    command.arg(root);
    for argument in arguments {
        command.arg(argument);
    }
    // 与既有夹具同一条纪律：显式摘掉真实凭据与代理，命令的结论不该依赖它们。
    command
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env("NO_PROXY", "127.0.0.1,localhost");
    command.output().expect("启动 agent_host_status")
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

/// 报告里某一行的取值（`key=value`；找不到就 panic，避免断言悄悄失效）。
fn field(text: &str, key: &str) -> String {
    let needle = format!("{key}=");
    text.lines()
        .find_map(|line| line.strip_prefix(&needle))
        .unwrap_or_else(|| panic!("报告里没有 {key}=: {text}"))
        .to_string()
}

/// 取 `activities-page=<本页条数> of=<总数> scope=<哪一头>` 这一行的第 `index` 段
/// （`0` / `1` / `2`），并把 `键=` 前缀去掉。
///
/// 为什么要单独一个助手（2026-09-29）：三件事挤在一行，`field()` 只认行首，
/// 所以 `field(text, "of")` 找不到东西。**这一族字段历来是"一键多字段"**，
/// 拿整串去比或去 parse 都会读错（`parseInt("32 of=36")` 是 32，看着对）。
fn activity_reading(text: &str, index: usize) -> String {
    let segment = field(text, "activities-page")
        .split_whitespace()
        .nth(index)
        .unwrap_or_else(|| panic!("活动读数缺第 {index} 段: {text}"))
        .to_string();
    match segment.split_once('=') {
        Some((_, value)) => value.to_string(),
        None => segment,
    }
}

/// 一行 ndjson 里某个字符串字段的取值（找不到就 panic）。
///
/// 刻意不复用上面的 `field`：那个解的是 `key=value` 的报告行，拿来解 JSON 会**静默失配**
/// （`failure=` 在 `"failure":"…"` 里根本不存在），正是要避免的那种"断言悄悄失效"。
fn json_string_field(line: &str, key: &str) -> String {
    let needle = format!("\"{key}\":\"");
    let start = line
        .find(&needle)
        .unwrap_or_else(|| panic!("这行没有字段 {key}: {line}"))
        + needle.len();
    let rest = &line[start..];
    rest[..rest
        .find('"')
        .unwrap_or_else(|| panic!("字段 {key} 没有闭合引号: {line}"))]
        .to_string()
}

/// 报告里以某个词开头的整行（找不到就 panic）。
fn line_starting_with(text: &str, prefix: &str) -> String {
    text.lines()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("报告里没有以 {prefix} 开头的行: {text}"))
        .to_string()
}

/// 造一个空环境（只有身份）。
fn empty_environment(root: &Path, agent: &str) {
    drop(create_environment(root, AgentId::new(agent)));
}

/// 直接往环境里写一个**已收场**的活动（两侧都写全：经历侧有收场，事实侧也有收场）。
///
/// 只走公开的追加面，不碰任何私有布局——宿主读的就是这些记录。
fn add_ended_run(root: &Path, agent: &str, run: &str, outcome: Outcome) {
    let mut environment = open_environment(root, AgentId::new(agent));
    let run = RunId::new(run);
    environment
        .append_trace(&run, TracePayload::RunStarted { origin: None })
        .expect("经历：登记");
    environment
        .append_trace(&run, celira_agent::store::ending_payload(outcome))
        .expect("经历：收场");
    environment
        .append_fact(
            &run,
            &Record::RunStarted {
                agent: AgentId::new(agent),
                run: run.clone(),
            },
        )
        .expect("事实：登记");
    environment
        .append_fact(
            &run,
            &Record::RunEnded {
                run: run.clone(),
                outcome,
                cost: celira_agent::cost::RunCost::default(),
            },
        )
        .expect("事实：收场");
}

/// 只写事实侧的孤儿活动：经历侧没有它（真实系统里来自"事实写成、经历没写成"的那半步）。
fn add_facts_only_run(root: &Path, agent: &str, run: &str) {
    let mut environment = open_environment(root, AgentId::new(agent));
    let run = RunId::new(run);
    environment
        .append_fact(
            &run,
            &Record::RunStarted {
                agent: AgentId::new(agent),
                run: run.clone(),
            },
        )
        .expect("事实：登记");
}

/// 造一个"**登记了、一条答复都没有**"的活动：两侧都只有登记。
///
/// 真实系统里这就是"进程在第一次模型调用之前就断了"的形状——**崩溃 / 被强杀 / 断电同形**。
/// ⚠ 别拿 [`add_facts_only_run`] 当它用：那只写事实侧，经历侧根本不认识这个活动，
/// 判定会读作 `refused(unknown-run)`（"读不出来"那一档），而不是"没走完"。
fn add_unfinished_run_without_reply(root: &Path, agent: &str, run: &str) {
    let mut environment = open_environment(root, AgentId::new(agent));
    let run = RunId::new(run);
    environment
        .append_trace(&run, TracePayload::RunStarted { origin: None })
        .expect("经历：登记");
    // 输入也记上了（否则判定读作 `blocked(input-not-confirmed)`——那是另一档），
    // 然后**一条答复都没有**：这就是"进程在第一次模型调用之前断了"的形状。
    environment
        .append_trace(
            &run,
            TracePayload::RunInput {
                text: "被叫醒了".to_string(),
            },
        )
        .expect("经历：输入");
    environment
        .append_fact(
            &run,
            &Record::RunStarted {
                agent: AgentId::new(agent),
                run: run.clone(),
            },
        )
        .expect("事实：登记");
}

/// 用夹具造一个**真实跑出来**的状态（脚本模型，零真实调用）。
///
/// `script` 是夹具的脚本步骤（`finish` / `say:…` / `act:…`）。
fn drive_with_fixture(root: &Path, agent: &str, materials: &Path, run: &str, script: &str) {
    let init = Command::new(binary("manual_entry_fixture"))
        .env("MANUAL_FIXTURE_ROOT", root)
        .env("MANUAL_FIXTURE_AGENT", agent)
        .env("MANUAL_FIXTURE_INIT", "1")
        .output()
        .expect("初始化环境");
    assert!(init.status.success(), "夹具初始化失败: {init:?}");

    let driven = Command::new(binary("manual_entry_fixture"))
        .env("MANUAL_FIXTURE_ROOT", root)
        .env("MANUAL_FIXTURE_AGENT", agent)
        .env("MANUAL_FIXTURE_MATERIALS", materials)
        .env("MANUAL_FIXTURE_RUN", run)
        .env("MANUAL_FIXTURE_MODEL", "script")
        .env("MANUAL_FIXTURE_SCRIPT", script)
        .env("MANUAL_FIXTURE_MAX_TURNS", "8")
        .env("MANUAL_FIXTURE_INPUT", "开场输入")
        .env_remove("DEEPSEEK_API_KEY")
        .output()
        .expect("跑夹具活动");
    assert!(
        driven.status.code().is_some(),
        "夹具活动没有正常退出: {driven:?}"
    );
}

/// 目录里每个文件的**字节快照**（名字排序）：零写入的对照物。
fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .expect("读目录")
        .map(|entry| entry.expect("目录项可读"))
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let bytes = std::fs::read(entry.path()).unwrap_or_default();
            (name, bytes)
        })
        .collect();
    entries.sort();
    entries
}

/// 一份存储的全部记录（每行一个 JSON 对象）。
fn objects_of(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .expect("读存储")
        .lines()
        .map(|line| serde_json::from_str(body_of(line)).expect("每一行都是一个对象"))
        .collect()
}

/// 一行的**正文**：存储行尾带校验和（`\t<8 位十六进制>`，2026-10-06 起）。
///
/// 这几条用例是**存储手术**（读—改—写回），不是读经历，因此它们必须认识格式。
/// ⚠ 写回去的行**没有**校验和——那正是"加校验和之前写下的旧记录"的形状，存储照旧读得出来
/// （见 `store::line_sum`：旧记录不校验、只进链），这正是这些用例要造的"被外部改过"的样子。
fn body_of(line: &str) -> &str {
    match line.rfind('\t') {
        Some(index) => &line[..index],
        None => line,
    }
}

/// 写回一组记录，并把序号重排成从 0 开始的连续值（存储要求序号连续）。
fn write_objects(path: &Path, objects: &[serde_json::Value]) {
    let mut text = String::new();
    for (index, object) in objects.iter().enumerate() {
        let mut object = object.clone();
        object["seq"] = serde_json::Value::from(index as u64);
        text.push_str(&object.to_string());
        text.push('\n');
    }
    std::fs::write(path, text).expect("写回存储");
}

/// 从一份存储里删掉所有含某个片段的行：造"崩在派发之后"的形状（既有夹具同款手法，
/// 删完**必须重排序号**——存储要求序号连续，跳号会让环境拒绝打开）。
fn drop_lines_containing(path: &Path, needle: &str) {
    let kept: Vec<serde_json::Value> = objects_of(path)
        .into_iter()
        .filter(|object| !object.to_string().contains(needle))
        .collect();
    write_objects(path, &kept);
}

/// 一个临时的环境 + 资料根。
struct Fixture {
    root: PathBuf,
    env: PathBuf,
    materials: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut root = std::env::temp_dir();
        root.push(format!(
            "celira-task16-host-{label}-{}-{n}",
            std::process::id()
        ));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("清掉同名残留目录");
        }
        let materials = root.join("materials");
        std::fs::create_dir_all(&materials).expect("建资料目录");
        Self {
            env: root.join("env"),
            materials,
            root,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// ---------------------------------------------------------------------------
// 1. 干净环境：无需动作，退出码 0
// ---------------------------------------------------------------------------

#[test]
fn a_clean_environment_needs_no_action() {
    let fixture = Fixture::new("clean");
    empty_environment(&fixture.env, AGENT);

    let run = status(&fixture.env, &["--agent", AGENT]);
    assert_eq!(code(&run), 0, "{}", stderr(&run));
    let report = stdout(&run);
    assert!(
        report.starts_with("celira-agent-host-status v1\n"),
        "{report}"
    );
    assert_eq!(field(&report, "agent"), AGENT, "{report}");
    // 空环境：本页 0 条、一共 0 条。
    assert_eq!(activity_reading(&report, 0), "0", "{report}");
    assert_eq!(activity_reading(&report, 1), "0", "{report}");
    assert!(report.contains("[state]\n"), "{report}");
    assert!(report.contains("admission=ready\n"), "{report}");
    assert!(report.contains("[next]\n"), "{report}");
    assert!(
        report.contains("没有活动：无需动作，可以开始新活动"),
        "{report}"
    );

    // 已收场的活动同样"无需动作"，但读数要说清它收在哪。
    add_ended_run(
        &fixture.env,
        AGENT,
        RUN,
        Outcome::Subject(SubjectEnding::Completed),
    );
    let run = status(&fixture.env, &["--agent", AGENT]);
    assert_eq!(code(&run), 0, "{}", stderr(&run));
    let report = stdout(&run);
    assert_eq!(activity_reading(&report, 1), "1", "一共一个活动: {report}");
    assert!(
        line_starting_with(&report, "run=").contains("state=ended outcome=subject:completed"),
        "{report}"
    );
    assert!(
        report.contains("活动已收场——无需动作，或开始新活动"),
        "{report}"
    );
}

// ---------------------------------------------------------------------------
// 2. 停在半路的活动：指向**新的一段**（不是"接着跑它"），退出码 3
// ---------------------------------------------------------------------------

/// ⚠ **旧规矩是什么、新规矩是什么**（2026-09-26）。这条用例从前叫
/// `a_waiting_activity_names_the_resume_command`：它断言 `next` 里给出 `agent_run resume` 的
/// 全部命令要素（环境根 / 身份 / 材料根占位符 / 活动标识 / "并给一个宿主动作"）。
///
/// **旧规矩**：这一档读作"**可以接着跑它**"（`state=resumable-waiting`），`next` 因此指向那条
/// 恢复命令。**新规矩**：承载活动的进程没了就是两个 Run——那一次连续
/// 就断了；新进程起来是**新的一段**。⇒ 判定改名 `unfinished`（**读数**，不是许可），`next`
/// 不再指向"接着跑它"，而是指向**重启**：新的一段会在开场带上那张纸条，如实告诉主体
/// "上一段没有正常收场"。
///
/// **不变量本身没变**：`next` 仍然**恰好一行**、仍然只指向真能走的下一步、仍然把环境根与身份
/// 写全。换的是"下一步是什么"。
#[test]
fn a_run_left_unfinished_points_at_the_new_segment_instead_of_resuming_it() {
    let fixture = Fixture::new("waiting");
    drive_with_fixture(
        &fixture.env,
        AGENT,
        &fixture.materials,
        RUN,
        "say:我先停一下",
    );

    let run = status(&fixture.env, &["--agent", AGENT]);
    assert_eq!(
        code(&run),
        3,
        "停在半路的活动需要用户动作: {}",
        stdout(&run)
    );
    let report = stdout(&run);
    assert!(
        report.contains(&format!("admission=unfinished(run={RUN})")),
        "未收尾要如实报出那个活动: {report}"
    );
    let activity = line_starting_with(&report, "run=");
    assert!(activity.contains("state=unfinished"), "{report}");

    // next 恰好一行，而且把该敲的命令要素都写全（材料根不在环境里 ⇒ 给占位符）。
    let next = line_starting_with(&report, "有未收尾活动");
    let suggestions: Vec<&str> = report
        .lines()
        .skip_while(|line| *line != "[next]")
        .skip(1)
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(suggestions.len(), 1, "next 必须恰好一行: {report}");
    for expected in [
        "活动停在半路",
        "不",
        "接着跑它",
        "重启宿主即可",
        "新的一段会在开场",
        "上一段没有正常收场",
        "agent_host_status",
        "--agent",
    ] {
        assert!(next.contains(expected), "next 少了 {expected}: {next}");
    }
    assert!(
        !next.contains("agent_run resume"),
        "恢复那条路已退役，next 不得再指向它: {next}"
    );
    assert!(
        next.contains(&fixture.env.to_string_lossy().replace('\\', "\\\\"))
            || next.contains(&fixture.env.to_string_lossy().to_string()),
        "next 里的环境根必须与命令行给的一致: {next}"
    );
}

// ---------------------------------------------------------------------------
// 3. 未收尾 / 已暂停：准入如实、建议对应、退出码 3
// ---------------------------------------------------------------------------

#[test]
fn a_paused_environment_says_so_and_asks_for_the_cause_to_be_handled() {
    let fixture = Fixture::new("paused");
    empty_environment(&fixture.env, AGENT);
    add_ended_run(
        &fixture.env,
        AGENT,
        RUN,
        Outcome::Subject(SubjectEnding::Completed),
    );
    // 暂停是**已记录在案**的存储故障：由文件权限之外的路径（保存失败）产生，这里用公开面直接记录。
    {
        let mut environment = open_environment(&fixture.env, AgentId::new(AGENT));
        environment
            .mark_paused(&RunId::new(RUN), "保存失败：磁盘写不进去")
            .expect("记录暂停");
    }

    let run = status(&fixture.env, &["--agent", AGENT]);
    assert_eq!(code(&run), 3, "已暂停需要用户动作: {}", stdout(&run));
    let report = stdout(&run);
    assert!(
        report.contains("admission=paused(reason=保存失败：磁盘写不进去)"),
        "暂停原因要如实带出: {report}"
    );
    // TASK_25 起这一行指向**一条真能走的命令**（此前它让用户去做一件产品里做不到的事）。
    assert!(
        report.contains("环境已暂停（保存失败：磁盘写不进去）"),
        "{report}"
    );
    assert!(
        report.contains("agent_host_acknowledge") && report.contains("--confirm"),
        "建议里要出现那条真能跑的确认命令: {report}"
    );
    // 环境根在报告里是**转义后**的形状（`escape_field` 把反斜杠写成两个），比较时要照同一个形状比。
    let escaped_root = fixture.env.to_string_lossy().replace('\\', "\\\\");
    assert!(
        report.contains(&format!("--agent {AGENT}")) && report.contains(&escaped_root),
        "命令要素要带出来（环境根与身份）: {report}"
    );
    // 活动行照实说：这条暂停正是它被阻断的原因（判定层把暂停读成阻断，宿主不重新解释）。
    let activity = line_starting_with(&report, "run=");
    assert!(
        activity.contains("state=blocked") && activity.contains("reason=storage-paused:"),
        "{report}"
    );
    // 而 `next` **只**说环境级那一件（同一件事不说两遍）。
    assert_eq!(
        report
            .lines()
            .skip_while(|line| *line != "[next]")
            .skip(1)
            .filter(|line| !line.is_empty())
            .count(),
        1,
        "{report}"
    );
    assert!(
        !report
            .lines()
            .skip_while(|line| *line != "[next]")
            .skip(1)
            .any(|line| line.contains("活动被阻断")),
        "暂停那一句已经说完了，不该再叠一句同样的: {report}"
    );
}

#[test]
fn a_dispatched_action_without_a_result_is_reported_as_unknown_with_its_count() {
    let fixture = Fixture::new("unknown");
    // 真跑出一个活动（展开 + 列目录），再像崩溃那样把它的事实结果删掉。
    drive_with_fixture(
        &fixture.env,
        AGENT,
        &fixture.materials,
        RUN,
        "act:open_app?target=files;act:inspect_action?action=files_list;act:files_list?path=.;say:等",
    );
    drop_lines_containing(&fixture.env.join("facts.ndjson"), "action-result");

    let run = status(&fixture.env, &["--agent", AGENT]);
    assert_eq!(code(&run), 3, "未收尾的活动需要用户动作: {}", stdout(&run));
    let report = stdout(&run);
    let activity = line_starting_with(&report, "run=");
    assert!(
        activity.contains("state=unknown") && activity.contains("reason=dispatched-without-result"),
        "{report}"
    );
    // TASK_69：这份脚本现在有**三次**派发（展开 / 先看格式 / 列目录）——"派发无结果"要按
    // **已派发而无结果的操作条数**如实报，不能只数业务动作（`inspect_action` 同样是派发过的操作）。
    assert!(activity.contains("unresolved=3"), "{activity}");
    // ⚠ **旧规矩 → 新规矩**（2026-09-26）：这里原来断言的是"继续前写会被拒，复核可解除"
    // ——那是**未决效果门**的指令。门退役了（门是指令，而世界只该给信息），这一格现在只说
    // 事实：那一次写没有结果，而"要不要去看一眼"由主体自己决定。
    assert!(
        report.contains("有未决操作（3 条）——那一次写没有结果"),
        "{report}"
    );
    assert!(
        !report.contains("会被拒"),
        "门已退役，读数里不得再出现它的指令: {report}"
    );
}

// ---------------------------------------------------------------------------
// 4. 带分类的模型失败：`failure=` 与盘上字段一致
// ---------------------------------------------------------------------------

#[test]
fn a_classified_model_failure_is_shown_with_the_class_that_is_on_disk() {
    let fixture = Fixture::new("model-failed");
    empty_environment(&fixture.env, AGENT);
    add_ended_run(
        &fixture.env,
        AGENT,
        RUN,
        Outcome::System(SystemEnding::ModelFailed(Some(
            ModelFailureClass::HttpStatus(None).as_str(),
        ))),
    );

    let run = status(&fixture.env, &["--agent", AGENT]);
    assert_eq!(code(&run), 0, "已收场不需要动作: {}", stdout(&run));
    let report = stdout(&run);
    let activity = line_starting_with(&report, "run=");
    assert!(
        activity.contains("state=ended outcome=system:model-failed failure=http-status-failure"),
        "{report}"
    );

    // 与盘上字段逐字对照：显示的分类就是那一行的分类，不翻译、不加工。
    let facts = std::fs::read_to_string(fixture.env.join("facts.ndjson")).expect("读事实");
    let ended = facts
        .lines()
        .find(|line| line.contains("\"fact\":\"run-ended\""))
        .expect("有收场行");
    assert!(
        ended.contains("\"failure\":\"http-status-failure\""),
        "{ended}"
    );
    assert!(
        activity.contains("failure=http-status-failure"),
        "{activity}"
    );

    // 分类没有记录时（旧记录形态）**不编造**：一行都不出现。
    let dir = Fixture::new("model-failed-old");
    empty_environment(&dir.env, AGENT);
    add_ended_run(
        &dir.env,
        AGENT,
        RUN,
        Outcome::System(SystemEnding::ModelFailed(None)),
    );
    let run = status(&dir.env, &["--agent", AGENT]);
    let report = stdout(&run);
    assert!(
        line_starting_with(&report, "run=").contains("outcome=system:model-failed"),
        "{report}"
    );
    assert!(!report.contains("failure="), "缺分类时不得编造: {report}");
}

/// 细分档也要**原样读回**：写进去的是 `<分类>:<细分>`，终端读数就是同一个串。
///
/// 这一格用两个**不同 family** 的细分档各走一遍（HTTP 侧与传输侧），防止"只有某一族接了细分"。
/// 与上一格的分工：上一格守"没有细分的老记录不被加工（`HttpStatus(None)` ⇒ 不带冒号）"，
/// 这一格守"有细分的记录不被吞掉（带冒号也必须逐字出来）"。
#[test]
fn a_detailed_model_failure_is_read_back_verbatim() {
    for (ending, expected) in [
        (
            SystemEnding::ModelFailed(Some(
                ModelFailureClass::HttpStatus(Some(HttpStatusDetail::ServerSide)).as_str(),
            )),
            "http-status-failure:server-side",
        ),
        (
            SystemEnding::ModelFailed(Some(
                ModelFailureClass::Transport(Some(TransportDetail::Refused)).as_str(),
            )),
            "transport-failure:refused",
        ),
    ] {
        let fixture = Fixture::new("model-failed-detail");
        empty_environment(&fixture.env, AGENT);
        add_ended_run(&fixture.env, AGENT, RUN, Outcome::System(ending));

        let run = status(&fixture.env, &["--agent", AGENT]);
        assert_eq!(code(&run), 0, "已收场不需要动作: {}", stdout(&run));
        let report = stdout(&run);
        let activity = line_starting_with(&report, "run=");
        assert!(
            activity.contains(&format!("failure={expected}")),
            "细分档必须原样出现在读数里（期望 {expected}）: {report}"
        );

        // 盘上那一行就是读数的来源：两个串必须逐字相等。
        let facts = std::fs::read_to_string(fixture.env.join("facts.ndjson")).expect("读事实");
        let ended_line = facts
            .lines()
            .find(|line| line.contains("\"fact\":\"run-ended\""))
            .expect("有收场行");
        let on_disk = json_string_field(ended_line, "failure");
        assert_eq!(on_disk, expected, "盘上分类与期望不符: {ended_line}");
        assert!(
            activity.contains(&format!("failure={on_disk}")),
            "读数与盘上字段必须逐字一致: {activity}"
        );
    }
}

// ---------------------------------------------------------------------------
// 5. 零写入：命令前后逐字节不变
// ---------------------------------------------------------------------------

#[test]
fn the_command_writes_nothing() {
    let fixture = Fixture::new("no-write");
    drive_with_fixture(
        &fixture.env,
        AGENT,
        &fixture.materials,
        RUN,
        "act:open_app?target=files;act:inspect_action?action=files_list;act:files_list?path=.;say:等",
    );
    drop_lines_containing(&fixture.env.join("facts.ndjson"), "action-result");

    let before = snapshot(&fixture.env);
    // 三种结论各跑一次（需要动作 / 读得出来），每次都不许动一个字节。
    for arguments in [vec!["--agent", AGENT], vec!["--agent", AGENT]] {
        let run = status(&fixture.env, &arguments);
        assert!(!stdout(&run).is_empty(), "{}", stderr(&run));
    }
    // 参数错与帮助也走一遍：连错误路径都不写。
    let _ = status(&fixture.env, &[]);
    let _ = status(&fixture.env, &["--agent", AGENT, "--agent", AGENT]);
    let _ = Command::new(binary("agent_host_status"))
        .arg("--help")
        .output()
        .expect("跑帮助");

    let after = snapshot(&fixture.env);
    assert_eq!(before, after, "命令不得写任何东西（含身份文件与目录成员）");
}

// ---------------------------------------------------------------------------
// 7. 这一段**为什么**停在这里（2026-10-13）：操作者那一行从此分得出两种情形
// ---------------------------------------------------------------------------

/// 「它自己停下的」与「没有正常收场」是两件事，两者从前都只读作"停在半路"。
///
/// 这一格与**给主体**的那张开场纸条**同源**（同一条 `recovery::stopped_in_place`、同一个四态），
/// 判据是三类读者第 3 档：**人不许比系统知道得更少**。取值就是事实里本来就有的词
/// （`held` / `yielded` / `no-response` / `unreadable`），一个都不新造。
#[test]
fn the_row_says_why_the_run_stopped_there() {
    // ① 它把控制交回了（账上最后一条答复是让出）：`yielded`，而且那句话说清"没有收场结论"。
    let fixture = Fixture::new("stopped-yielded");
    drive_with_fixture(
        &fixture.env,
        AGENT,
        &fixture.materials,
        RUN,
        "say:我先停一下",
    );
    let report = stdout(&status(&fixture.env, &["--agent", AGENT]));
    let row = line_starting_with(&report, &format!("run={RUN} state=unfinished"));
    assert!(
        row.contains("stopped=yielded"),
        "这一行要带上它为什么停在这里: {row}"
    );
    let next = line_starting_with(&report, "有未收尾活动");
    assert!(next.contains("把控制交回了"), "next 要说清是让出: {next}");
    assert!(
        !next.contains("一条答复都没有"),
        "别把「让出」读成「被杀」: {next}"
    );

    // ② 一条答复都没有（真实系统里"进程在第一次模型调用之前就断了"的形状）：`no-response`，
    //    而且那句话说清"崩溃 / 被强杀 / 断电同形"——**不猜**是哪一种。
    let fixture = Fixture::new("stopped-no-response");
    empty_environment(&fixture.env, AGENT);
    add_unfinished_run_without_reply(&fixture.env, AGENT, RUN);
    let report = stdout(&status(&fixture.env, &["--agent", AGENT]));
    let row = line_starting_with(&report, &format!("run={RUN} state="));
    assert!(row.contains("stopped=no-response"), "这一行: {row}");
    let next = line_starting_with(&report, "有未收尾活动");
    assert!(next.contains("崩溃"), "next 要说清这一档的界限: {next}");
}

// ---------------------------------------------------------------------------
// 6. next 表驱动：每个可达组合一条唯一文案 + 反例
// ---------------------------------------------------------------------------

/// 表里的一行：标签、造法、期望的 next 关键片段、期望退出码、不该出现的片段。
type NextCase = (
    &'static str,
    Box<dyn Fn(&Fixture)>,
    &'static str,
    i32,
    Vec<&'static str>,
);

/// `next` 的表：输入是（准入，活动类目），输出是那**一句**话加退出码。
///
/// `Ready` 与"未收场"的四类**同时出现**在真实环境里做不到（一个已登记开始、又没有可靠结束
/// 事实的活动，必然让准入变成 `unfinished`），因此表里覆盖的是**可达**组合；不可达的那几支
/// 由最后一条"反例"断言钉住它们的**不出现**（而不是假装它们被跑到过）。
#[test]
fn the_next_line_is_a_mechanical_table_and_never_mixes_two_suggestions() {
    // (标签, 造法, 期望 next 里的关键片段, 期望退出码, 不该出现的片段)
    let cases: Vec<NextCase> = vec![
        (
            "空环境",
            Box::new(|fixture: &Fixture| empty_environment(&fixture.env, AGENT)),
            "没有活动：无需动作，可以开始新活动",
            0,
            vec![
                "活动停在半路",
                "有未决操作",
                "已被阻断",
                "不可判读",
                "已暂停",
            ],
        ),
        (
            "全部收场",
            Box::new(|fixture: &Fixture| {
                empty_environment(&fixture.env, AGENT);
                add_ended_run(
                    &fixture.env,
                    AGENT,
                    RUN,
                    Outcome::Subject(SubjectEnding::Completed),
                );
            }),
            "活动已收场——无需动作，或开始新活动",
            0,
            vec![
                "活动停在半路",
                "有未决操作",
                "已被阻断",
                "不可判读",
                "已暂停",
            ],
        ),
        (
            "未收尾 + 停在半路",
            Box::new(|fixture: &Fixture| {
                drive_with_fixture(&fixture.env, AGENT, &fixture.materials, RUN, "say:停")
            }),
            // ⚠ 旧规矩：这一格断言 `agent_run resume` 的要素（"接着跑它"）。新规矩：指向**重启**
            // ——新的一段会在开场带上那张纸条（见上面那条用例的说明）。
            "有未收尾活动（run-1）：活动停在半路——不接着跑它：重启宿主即可",
            3,
            vec!["有未决操作", "已被阻断", "不可判读", "已暂停"],
        ),
        (
            "未收尾 + 未决操作",
            Box::new(|fixture: &Fixture| {
                drive_with_fixture(
                    &fixture.env,
                    AGENT,
                    &fixture.materials,
                    RUN,
                    "act:open_app?target=files;act:inspect_action?action=files_list;act:files_list?path=.;say:等",
                );
                drop_lines_containing(&fixture.env.join("facts.ndjson"), "action-result");
            }),
            // TASK_69：脚本有三次派发（展开 / 先看格式 / 列目录）⇒ 未决条数是 3。
            // ⚠ **旧规矩 → 新规矩**（2026-09-26）：这一格原来写的是"继续前写会被拒，复核可解除"
            // ——那是**未决效果门**的指令。门退役了（门是指令，而世界只该给信息），这一格现在
            // 只说事实：那一次写没有结果，而"要不要去看一眼"由主体自己决定。
            "有未收尾活动（run-1）：有未决操作（3 条）——那一次写没有结果",
            3,
            vec!["活动停在半路", "已被阻断", "不可判读", "已暂停"],
        ),
        (
            "未收尾 + 阻断",
            Box::new(|fixture: &Fixture| {
                drive_with_fixture(
                    &fixture.env,
                    AGENT,
                    &fixture.materials,
                    RUN,
                    "act:open_app?target=files;act:inspect_action?action=files_list;act:files_list?path=.;say:等",
                );
                // 连"已派发"都没有：准备了、却没发出去 ⇒ 可证明没有外部效果。
                drop_lines_containing(&fixture.env.join("facts.ndjson"), "action-dispatched");
                drop_lines_containing(&fixture.env.join("facts.ndjson"), "action-result");
            }),
            "有未收尾活动（run-1）：活动被阻断（prepared-without-dispatch）——处理原因后重试",
            3,
            vec!["活动停在半路", "有未决操作", "不可判读", "已暂停"],
        ),
        (
            "读得出但不可判读",
            Box::new(|fixture: &Fixture| {
                empty_environment(&fixture.env, AGENT);
                add_facts_only_run(&fixture.env, AGENT, RUN);
            }),
            "状态不可判读——见 environment_diagnose",
            0,
            vec!["活动停在半路", "有未决操作", "已被阻断", "已暂停", "已收场"],
        ),
        (
            "已暂停",
            Box::new(|fixture: &Fixture| {
                empty_environment(&fixture.env, AGENT);
                add_ended_run(
                    &fixture.env,
                    AGENT,
                    RUN,
                    Outcome::Subject(SubjectEnding::Completed),
                );
                let mut environment = open_environment(&fixture.env, AgentId::new(AGENT));
                environment
                    .mark_paused(&RunId::new(RUN), "磁盘写不进去")
                    .expect("记录暂停");
            }),
            // TASK_25 起这一档指向**一条真能走的命令**（此前那句"显式清除暂停"没有入口）。
            "环境已暂停（磁盘写不进去）：先 `agent_host_acknowledge",
            3,
            vec![
                "活动停在半路",
                "有未决操作",
                "已被阻断",
                "不可判读",
                "活动已收场",
            ],
        ),
    ];

    for (label, build, expected, expected_code, absent) in cases {
        let fixture = Fixture::new(label);
        build(&fixture);
        let run = status(&fixture.env, &["--agent", AGENT]);
        assert_eq!(
            code(&run),
            expected_code,
            "{label}: 退出码应当是结论 {expected_code}\n{}",
            stdout(&run)
        );
        let report = stdout(&run);
        let suggestion = report
            .lines()
            .skip_while(|line| *line != "[next]")
            .skip(1)
            .find(|line| !line.is_empty())
            .unwrap_or_else(|| panic!("{label}: next 之后必须有且只有一行"));
        assert!(
            suggestion.contains(expected),
            "{label}: next 应当是「{expected}」，实际「{suggestion}」"
        );
        for other in absent {
            assert!(
                !suggestion.contains(other),
                "{label}: next 里不该出现别的类目的建议「{other}」：{suggestion}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 7. 多活动：逐行列全；超页上限如实报截断
// ---------------------------------------------------------------------------

#[test]
fn every_activity_gets_a_line_and_truncation_is_reported_truthfully() {
    let fixture = Fixture::new("multi");
    empty_environment(&fixture.env, AGENT);
    for index in 0..3 {
        add_ended_run(
            &fixture.env,
            AGENT,
            &format!("run-{index:03}"),
            Outcome::Subject(SubjectEnding::Completed),
        );
    }

    let run = status(&fixture.env, &["--agent", AGENT]);
    assert_eq!(code(&run), 0, "{}", stderr(&run));
    let report = stdout(&run);
    // **本页几条**与**一共几条**分开报（2026-09-29 起）：这里两者相等（只有 3 个活动）。
    assert_eq!(activity_reading(&report, 0), "3", "{report}");
    assert_eq!(activity_reading(&report, 1), "3", "{report}");
    assert_eq!(activity_reading(&report, 2), "latest", "{report}");
    let lines: Vec<&str> = report
        .lines()
        .filter(|line| line.starts_with("run="))
        .collect();
    assert_eq!(lines.len(), 3, "每个活动一行: {report}");
    for index in 0..3 {
        assert!(
            report.contains(&format!("run=run-{index:03} state=ended")),
            "{report}"
        );
    }

    // 超过诊断分页上限（32）时：本页照实列 32 条，**并且报出总数**。
    //
    // ⚠ 2026-09-29 真机实测修：这一条原先断言 `activities=32` + `next-after=run-031`
    // —— 那正是缺陷本身：`activities` 被当成总数读（真家 36 条报 32，界面据此显示
    // 「活动次数 32」并从此冻住），而那一页取自账本**最早**的一头 ⇒ 最近的活动一条都
    // 看不见，连 `needs_action` 都是在最早那一页上算的。
    let many = Fixture::new("many");
    empty_environment(&many.env, AGENT);
    for index in 0..33 {
        add_ended_run(
            &many.env,
            AGENT,
            &format!("run-{index:03}"),
            Outcome::Subject(SubjectEnding::Completed),
        );
    }
    let run = status(&many.env, &["--agent", AGENT]);
    assert_eq!(code(&run), 0, "{}", stderr(&run));
    let report = stdout(&run);
    assert_eq!(
        activity_reading(&report, 0),
        "32",
        "本页条数应当等于诊断分页上限: {report}"
    );
    assert_eq!(
        activity_reading(&report, 1),
        "33",
        "**总数**必须如实报出来（33 条就报 33，不许拿本页条数冒充）: {report}"
    );
    assert_eq!(activity_reading(&report, 2), "latest", "{report}");
    // 这一页取自**最近**那一头：33 个活动里列的是最后 32 个（run-001..run-032），
    // 最早那个 run-000 不在本页。
    assert!(
        report.contains("run=run-032 state=ended"),
        "本页必须含**最新**那一个活动: {report}"
    );
    assert!(
        !report.contains("run=run-000 state=ended"),
        "本页取自最近那一头，最早的那个不该在场: {report}"
    );
    assert_eq!(
        report
            .lines()
            .filter(|line| line.starts_with("run="))
            .count(),
        32,
        "{report}"
    );
}

// ---------------------------------------------------------------------------
// 8. 无凭据环境：帮助、参数错、正常读数的结论都不变
// ---------------------------------------------------------------------------

#[test]
fn conclusions_do_not_depend_on_a_credential() {
    let fixture = Fixture::new("credential");
    drive_with_fixture(&fixture.env, AGENT, &fixture.materials, RUN, "say:停");

    // 帮助在任何路径之前：不打开环境、不读凭据，退出码 0。
    let help = Command::new(binary("agent_host_status"))
        .arg("--help")
        .env_remove("DEEPSEEK_API_KEY")
        .output()
        .expect("跑帮助");
    assert_eq!(code(&help), 0, "{}", stderr(&help));
    assert!(stdout(&help).contains("用法"), "{}", stdout(&help));

    // 参数错：退出码 1，且与凭据无关。
    for arguments in [
        vec![],
        vec!["--agent", AGENT, "--agent", AGENT],
        vec!["--agent", AGENT, "--run", "x"],
        vec!["--agent"],
    ] {
        let missing = Command::new(binary("agent_host_status"))
            .args(&arguments)
            .env_remove("DEEPSEEK_API_KEY")
            .output()
            .expect("跑参数错");
        assert_eq!(code(&missing), 1, "{arguments:?}: {}", stderr(&missing));
        assert!(!stdout(&missing).is_empty() || !stderr(&missing).is_empty());
    }
    // 少了 `--agent`：明确说缺什么，不猜一个身份。
    let no_agent = status(&fixture.env, &[]);
    assert_eq!(code(&no_agent), 1, "{}", stderr(&no_agent));
    assert!(
        stderr(&no_agent).contains("--agent"),
        "{}",
        stderr(&no_agent)
    );

    // 正常读数：结论与"有凭据"时逐字节相同（本命令根本不读凭据）。
    let without = status(&fixture.env, &["--agent", AGENT]);
    let with = {
        let mut command = Command::new(binary("agent_host_status"));
        command
            .arg(&fixture.env)
            .args(["--agent", AGENT])
            .env("DEEPSEEK_API_KEY", "fixture-sentinel-credential");
        command.output().expect("带凭据跑一次")
    };
    assert_eq!(code(&with), code(&without));
    assert_eq!(stdout(&with), stdout(&without));
    assert!(
        !stdout(&with).contains("fixture-sentinel-credential"),
        "报告里不得出现凭据"
    );
}

// ---------------------------------------------------------------------------
// 9. 环境打不开 / 身份不匹配：退出码 2
// ---------------------------------------------------------------------------

#[test]
fn an_unreadable_environment_is_exit_two_not_an_empty_one() {
    let fixture = Fixture::new("unreadable");

    // 不存在：不自动创建（与既有入口同一条规则）。
    let missing = status(&fixture.env, &["--agent", AGENT]);
    assert_eq!(code(&missing), 2, "{}", stdout(&missing));
    assert!(!fixture.env.exists(), "命令不得创建环境");

    empty_environment(&fixture.env, AGENT);
    let wrong = status(&fixture.env, &["--agent", "agent-other"]);
    assert_eq!(
        code(&wrong),
        2,
        "身份不匹配必须报读不出来: {}",
        stdout(&wrong)
    );
    assert!(
        stderr(&wrong).contains("环境不可打开或不可读取"),
        "{}",
        stderr(&wrong)
    );
}

// ---------------------------------------------------------------------------
// TASK_25：`paused` 档给的那条建议必须**真能跑**
// ---------------------------------------------------------------------------

/// 读数指的路要是能走的路：把 `[next]` 里点名的确认命令**照着跑一遍**，它得真的工作。
///
/// `--run` 在建议里是**占位符**（暂停原因不带活动名，公开面也读不到，本命令**不猜**），
/// 因此这里按宿主的真实做法取一个 `[state]` 里列出的活动名。
#[test]
fn the_paused_advice_names_a_command_that_actually_works() {
    let fixture = Fixture::new("paused-runnable");
    empty_environment(&fixture.env, AGENT);
    add_ended_run(
        &fixture.env,
        AGENT,
        RUN,
        Outcome::Subject(SubjectEnding::Completed),
    );
    {
        let mut environment = open_environment(&fixture.env, AgentId::new(AGENT));
        environment
            .mark_paused(&RunId::new(RUN), "save-failed:run-start")
            .expect("记录暂停");
    }

    let report = stdout(&status(&fixture.env, &["--agent", AGENT]));
    let advice = report
        .lines()
        .skip_while(|line| *line != "[next]")
        .nth(1)
        .expect("有 next 行")
        .to_string();
    assert!(
        advice.contains("agent_host_acknowledge") && advice.contains("--confirm"),
        "建议里要出现可跑的确认命令: {advice}"
    );

    // 照宿主的真实做法：先只读预览（零写入），看清原因再确认。
    let preview = Command::new(binary("agent_host_acknowledge"))
        .arg(&fixture.env)
        .args(["--agent", AGENT, "--run", RUN])
        .output()
        .expect("预览");
    assert_eq!(preview.status.code(), Some(3), "预览是'需要用户动作'");
    assert!(stdout(&preview).contains("paused-reason=save-failed:run-start"));

    let confirm = Command::new(binary("agent_host_acknowledge"))
        .arg(&fixture.env)
        .args(["--agent", AGENT, "--run", RUN, "--confirm"])
        .output()
        .expect("确认");
    assert_eq!(
        confirm.status.code(),
        Some(0),
        "建议里的命令照抄（--run 取活动名）就该能跑通: {}",
        stdout(&confirm)
    );
    assert!(stdout(&confirm).contains("admission-after=ready"));

    // 确认之后，状态命令不再说"已暂停"。
    let after = stdout(&status(&fixture.env, &["--agent", AGENT]));
    assert!(!after.contains("admission=paused"), "{after}");
}

// ---------------------------------------------------------------------------
// 动作面读数（`--actions`）：从账里数出来，而且**只在开关打开时**出现
// ---------------------------------------------------------------------------

/// 往一个活动里写一条**行动结果**（只走公开的追加面；这是**读者**那一侧的用例，
/// 所以不借用写侧函数，直接写它要读的形状）。签名压在 7 个参数以内：身份用模块常量。
fn add_observed(
    root: &Path,
    run: &str,
    turn: u32,
    index: u16,
    action: &str,
    state: ResultState,
    failure: Option<&str>,
) {
    let mut environment = open_environment(root, AgentId::new(AGENT));
    let payload = TracePayload::Observed {
        turn,
        index,
        action: action.to_string(),
        params: Vec::new(),
        state,
        failure: failure.map(str::to_string),
        blocks: Vec::new(),
    };
    environment
        .append_trace(&RunId::new(run), payload)
        .expect("经历：行动结果");
}

/// **动作面读数是开关式的**，而且数与账逐项对得上（2026-10-13）。
///
/// ⚠ **为什么必须是开关**（这条用例守的就是它）：`[state]` 段有真实消费者，而它**认不出的行
/// 就整体失败**（`scripts/home/host_status_parse.js`："宁可整体失败，也不安静跳过"）⇒
/// 默认输出必须逐字节不变；要读这一节的人（操作者 / 飞轮）自己加上 `--actions`。
///
/// 判据三条：① 默认输出里**没有** `[actions]`；② 带开关时，那一行与分类行**逐字节**对得上账；
/// ③ **首次调用税**按"**轮**"算——一轮里第一个动作就失败，是主体最贵的那种错。
#[test]
fn the_action_tally_is_opt_in_and_counts_the_ledger() {
    let fixture = Fixture::new("actions");
    empty_environment(&fixture.env, AGENT);

    let env = fixture.env.clone();
    // 第一段：轮 1 首发失败（invalid-parameter）＋ 同轮第二个动作成功；轮 2 两个都成功。
    add_observed(
        &env,
        "run-1",
        1,
        0,
        "notes_read",
        ResultState::Failure,
        Some("invalid-parameter"),
    );
    add_observed(
        &env,
        "run-1",
        1,
        1,
        "notes_list",
        ResultState::Success,
        None,
    );
    add_observed(
        &env,
        "run-1",
        2,
        0,
        "notes_list",
        ResultState::Success,
        None,
    );
    add_observed(
        &env,
        "run-1",
        2,
        1,
        "notes_append",
        ResultState::Success,
        None,
    );
    // 第二段：轮 1 首发失败（not-found）—— 首次调用税要跨活动累加。
    add_observed(
        &env,
        "run-2",
        1,
        0,
        "files_read",
        ResultState::Failure,
        Some("not-found"),
    );

    // ① 默认输出：一个字都不多（消费者的形状没变）。
    let plain = stdout(&status(&fixture.env, &["--agent", AGENT]));
    assert!(
        !plain.contains("[actions]"),
        "默认输出不许出现 [actions]（那一段的消费者认不出就整体失败）：{plain}"
    );

    // ② 带开关：那一行与分类行**逐字节**对得上账（分类按名字排序 ⇒ 顺序稳定）。
    let report = stdout(&status(&fixture.env, &["--agent", AGENT, "--actions"]));
    let scope = report
        .lines()
        .find(|line| line.starts_with("scope="))
        .unwrap_or_else(|| panic!("带 --actions 就该有 [actions] 那一行：{report}"));
    assert_eq!(
        scope, "scope=runs-in-page runs=2 calls=5 failed=2 unknown=0 first-call-tax=2",
        "{report}"
    );
    let classes: Vec<&str> = report
        .lines()
        .filter(|line| line.starts_with("failure="))
        .collect();
    assert_eq!(
        classes,
        vec![
            "failure=invalid-parameter calls=1 first-call=1",
            "failure=not-found calls=1 first-call=1",
        ],
        "{report}"
    );
}

// ---------------------------------------------------------------------------
// 本页与"一共几条"：两个数必须可比（`of=` 的口径）
// ---------------------------------------------------------------------------

/// **`of=` 报的是本页所在的全集**（事实侧 ∪ 经历侧）⇒ `本页 ≤ of` **永远成立**（2026-10-13 修正）。
///
/// 从前 `of=` 报的是**经历侧**的总数，而本页取的是两份存储的并集 ⇒ 真机上出现过
/// `activities-page=25 of=4`——**本页比"一共"还多**：两个数不可比却并排报。
/// 判据「人不许比系统知道得更少」：两个数都留着（各自的名字说清自己是什么），
/// 但 `of=` 这个**名字**只属于全集。
#[test]
fn the_page_total_counts_both_stores_so_it_is_never_smaller_than_the_page() {
    let fixture = Fixture::new("page-total");
    empty_environment(&fixture.env, AGENT);
    // 一个两侧都写全的活动 ＋ 一个**只在事实侧**出现的活动（真机形状：事实写成、经历没写成）。
    add_ended_run(
        &fixture.env,
        AGENT,
        "run-full",
        Outcome::Subject(SubjectEnding::Completed),
    );
    add_facts_only_run(&fixture.env, AGENT, "run-facts-only");

    let report = stdout(&status(&fixture.env, &["--agent", AGENT]));
    let page: usize = activity_reading(&report, 0).parse().expect("本页条数是数");
    let total: usize = activity_reading(&report, 1).parse().expect("一共几条是数");
    assert_eq!(page, 2, "本页把只在事实侧出现的活动也列出来：{report}");
    assert_eq!(total, 2, "of= 要数两份存储的并集：{report}");
    assert!(page <= total, "本页条数永远不该超过一共几条：{report}");
}

// ---------------------------------------------------------------------------
// 时间锚：进程在 ≠ 注意力在
// ---------------------------------------------------------------------------

/// **时间锚**（2026-10-13）：head 里报「账上最后一条经历在哪一刻」与「它上一次醒在哪一刻」——
/// 答的是操作者那个问题「**进程在，它还在动吗**」。
///
/// 判据是**只给物理量**：两格都是**绝对时刻**（打印出来不会过期）；"它是不是卡住了"是操作者的
/// 判断 ⇒ **不打印"安静了多久"**（那是会过期的投影，仓库明写不显示）。「没有」与「读不到」分开：
/// 账上一条都没有 ⇒ `none`；有记录但没带时刻 ⇒ `unknown`。
#[test]
fn the_report_carries_absolute_time_anchors() {
    // 空环境：账上一条都没有 ⇒ `none`。
    let empty = Fixture::new("times-empty");
    empty_environment(&empty.env, AGENT);
    let report = stdout(&status(&empty.env, &["--agent", AGENT]));
    assert_eq!(field(&report, "last-record-at"), "none", "{report}");
    assert_eq!(field(&report, "last-wake-at"), "none", "{report}");

    // 有账：两格都在，形状是 RFC 3339（UTC）。
    let fixture = Fixture::new("times");
    empty_environment(&fixture.env, AGENT);
    add_ended_run(
        &fixture.env,
        AGENT,
        "run-1",
        Outcome::Subject(SubjectEnding::Completed),
    );
    let report = stdout(&status(&fixture.env, &["--agent", AGENT]));
    for label in ["last-wake-at", "last-record-at"] {
        let value = field(&report, label);
        // **契约**（与夹具怎么写无关）：`none` 只表示"账上一条记录都没有"；有记录时只允许
        // 两种——一个**绝对时刻**，或 `unknown`（那条记录没带时刻：读不到就说读不到，不编造）。
        assert!(
            value != "none",
            "账上已经有记录了 ⇒ 不许说 none：{label}={value}"
        );
        assert!(
            value == "unknown" || (value.contains('T') && value.ends_with('Z')),
            "{label} 只能是 unknown 或一个绝对时刻（RFC 3339 UTC）：{value}"
        );
    }
}
