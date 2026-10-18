//! 阶段八端到端验收：**两个独立进程**共享同一个身份环境。
//!
//! 全部离线、全合成数据、只用测试自建的独立目录。
//!
//! 这里的"独立进程"是真的独立进程：本测试启动 `environment_worker` 这个**离线夹具二进制**
//! （见 `src/bin/environment_worker.rs`），由它扮演 A 或 B。两个进程之间**不共享任何内存**——
//! 身份环境是它们唯一的接触面。
//!
//! 验收链：
//!
//! 1. 进程 A 建立一个身份环境，跑一次活动，产出带正文的合成经历，正常收场后退出；
//! 2. 进程 B 独立打开**同一个身份**的环境，开始新的一次活动：
//!    - 第一轮不带 A 的正文（不自动摘要、不自动全量注入）；
//!    - 测试模型按**真实可见的能力目录**，用**通用文件动作**（`files_list` / `files_read`）
//!      主动去查 A 留下的经历（旧经历在同一个只读文件视图里是保留逻辑目录 `history`）；
//! 3. 别的身份打不开这个环境；
//! 4. 重复的活动标识在**模型调用与行动之前**就被拒绝；
//! 5. 同一环境不可能同时形成两个写者。

use std::path::{Path, PathBuf};
use std::process::Command;

use celira_agent::composition::{EnvironmentActivity, run_activity_in_environment};
use celira_agent::core::{ModelError, ModelOutput, ModelPort, NoContext, RunInput, View};
use celira_agent::foundation::{AgentId, RunId, SubjectEnding};
use celira_agent::store::Environment;
mod common;

const ROLE_ENV: &str = "CELIRA_PHASE08_ROLE";
const ROOT_ENV: &str = "CELIRA_PHASE08_ROOT";

fn agent_a() -> AgentId {
    AgentId::new("agent-alpha")
}

/// 用一个子进程跑某个角色，等待它退出。
///
/// 子进程就是本 crate 里那个离线夹具二进制；父进程只通过**环境目录**与它的 stdout 观察结果。
fn spawn(role: &str, root: &Path) -> std::process::Output {
    let exe = worker_binary();
    Command::new(exe)
        .env(ROLE_ENV, role)
        .env(ROOT_ENV, root)
        .output()
        .expect("启动子进程")
}

/// 夹具二进制的路径。
///
/// 它与测试二进制由同一次 `cargo test` 构建，所以就在测试可执行文件旁边。
fn worker_binary() -> PathBuf {
    common::binary("environment_worker")
}

// ---------------------------------------------------------------------------
// 真正跑的测试
// ---------------------------------------------------------------------------

#[test]
fn two_independent_processes_share_one_identity_environment() {
    let root = TempDir::new("cross-process");

    // 1. 进程 A：建立环境、产出经历、正常收场、退出。
    let a = spawn("a", root.path());
    assert!(
        a.status.success(),
        "进程 A 必须正常退出。stdout={} stderr={}",
        String::from_utf8_lossy(&a.stdout),
        String::from_utf8_lossy(&a.stderr)
    );

    // 2. 进程 B：独立打开同一身份，新活动，第一轮不带 A 正文，主动查 A。
    let b = spawn("b", root.path());
    let stdout = String::from_utf8_lossy(&b.stdout);
    assert!(
        b.status.success(),
        "进程 B 必须正常退出。stdout={stdout} stderr={}",
        String::from_utf8_lossy(&b.stderr)
    );

    let report = stdout
        .lines()
        .find_map(|line| line.strip_prefix("B_REPORT:"))
        .unwrap_or_else(|| panic!("B 必须回报结果，却得到: {stdout}"));
    let fields: Vec<&str> = report.split('|').collect();
    assert_eq!(fields.len(), 7, "回报字段不完整: {report}");

    assert_eq!(
        fields[0], "false",
        "B 第一次清醒期的视野里**不得**自动带上 A 的正文（不自动摘要、不自动全量注入）"
    );
    assert_eq!(
        fields[1], "true",
        "B 必须从真实能力目录里看到通用文件能力 `files_list`"
    );
    assert_eq!(
        fields[2], "true",
        "B 必须从真实能力目录里看到通用文件能力 `files_read`"
    );
    assert_eq!(
        fields[3], "true",
        "B 经**通用文件能力**读回来的内容必须**包含 A 保存的原文**"
    );
    assert_eq!(
        fields[4], "true",
        "查回来的内容必须带上 A 的活动标识与多条按序号排列的经历，而不是被摘要成一句"
    );
    assert_eq!(
        fields[5], "true",
        "B 第一次清醒、还没做任何事时，视野里就不得已经有观察（否则就是把 A 的行动搬了过来）"
    );
    assert_eq!(
        fields[6], "true",
        "跑完时的观察条数必须正好等于 B 自己发出的动作数：不自动重放 A 的行动"
    );
}

#[test]
fn another_identity_cannot_open_the_environment() {
    let root = TempDir::new("cross-identity");
    let a = spawn("a", root.path());
    assert!(a.status.success(), "A 必须正常退出");

    let other = spawn("other-identity", root.path());
    assert!(
        other.status.success(),
        "别的身份必须被拒绝。stderr={}",
        String::from_utf8_lossy(&other.stderr)
    );
}

#[test]
fn a_duplicate_run_id_is_refused_before_any_model_or_action() {
    let root = TempDir::new("cross-duplicate");
    let a = spawn("a", root.path());
    assert!(a.status.success(), "A 必须正常退出");

    let duplicate = spawn("duplicate-run", root.path());
    assert!(
        duplicate.status.success(),
        "重复活动标识必须在模型与动作之前被拒绝。stderr={}",
        String::from_utf8_lossy(&duplicate.stderr)
    );
}

#[test]
fn a_second_writer_cannot_open_the_same_environment() {
    let root = TempDir::new("cross-second-writer");
    let a = spawn("a", root.path());
    assert!(a.status.success(), "A 必须正常退出");

    // 父进程先取得写资格，再让子进程尝试打开：这就是"上一个写者还在"的真实情形。
    let held = Environment::open(&root.path().join("env"), agent_a()).expect("父进程先取得写资格");
    let second = spawn("second-writer", root.path());
    assert!(
        second.status.success(),
        "第二个写者必须被拒绝。stderr={}",
        String::from_utf8_lossy(&second.stderr)
    );
    drop(held);
}

/// 从一个角色的 stdout 里取出一条带前缀的报告行。
fn report_line(output: &std::process::Output, prefix: &str) -> String {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| {
            panic!(
                "没有找到 {prefix} 报告行。stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        })
        .to_string()
}

/// 进程一经真实入口留下保存失败记录；**另一个进程**重开时**照常执行**（2026-10-06 新规矩）。
///
/// ⚠ **旧规矩是什么**：这里原来断言 `REFUSAL_REPORT:paused|save-failed:run-start`——
/// "以已记录的存储故障理由拒绝，而且一次模型都不打"。那道闸 2026-10-06 按主人裁决退役：
/// **门是指令，而世界只该给信息**——那条故障由开场那一块纸条带给主体（`PreviousActivity`
/// 的 `StorageFailure` 那一段），能不能继续由她看着事实自己定。
///
/// **新规矩**：照常执行（模型真的被调用），而**放行不等于没有记录**——准入读数照旧说
/// `paused(reason=save-failed:run-start)`，这一格由同一次报告一起打出来。
/// 跨进程这一层证据一个字没变：两个进程不共享任何内存，唯一的接触面是环境目录。
#[test]
fn a_paused_environment_still_runs_from_a_later_process() {
    let root = TempDir::new("cross-process-pause");

    let paused = spawn("pause-by-real-failure", root.path());
    assert!(
        paused.status.success(),
        "制造保存失败应当成功：{}",
        String::from_utf8_lossy(&paused.stderr)
    );
    assert_eq!(
        report_line(&paused, "PAUSE_REPORT:"),
        "PAUSE_REPORT:run-start|false|true",
        "阶段应是活动开始、终态未被确认、暂停记录确实落盘"
    );

    // 另一个进程：**照常执行**（模型被调用一次），而那条故障照旧读得出来。
    let proceeded = spawn("must-run", root.path());
    assert!(
        proceeded.status.success(),
        "放行本身应正常退出：{}",
        String::from_utf8_lossy(&proceeded.stderr)
    );
    assert_eq!(
        report_line(&proceeded, "REFUSAL_REPORT:"),
        "REFUSAL_REPORT:none|calls=1|admission=paused(reason=save-failed:run-start)",
        "暂停不再是闸：照常跑，而那条记录照旧在准入读数里"
    );
}

/// 没有暂停记录、只有"没有收尾的活动"：另一个进程**不再拒绝执行**（2026-09-26 新规矩）——
/// 进程没了就是那一段断了，新进程起来是**新的一段**。
///
/// ⚠ **旧规矩是什么**：这里原来断言 `REFUSAL_REPORT:unfinished|run-unfinished`，
/// 即"以「活动没有收尾」拒绝，而不是冒充存储故障"。
/// **新规矩**：不再拒绝，因此也不再有那个读数；而"**不得冒充存储故障**"这半句**一个字没松**——
/// 它由上面那条 `paused` 用例守着（真正的存储故障仍然如实记在准入读数里）。
/// 而"上一段没有收尾"这件事，改由**开场那一轮**如实带给主体（见 `PreviousActivity`）。
#[test]
fn an_unfinished_activity_no_longer_refuses_execution_from_a_later_process() {
    let root = TempDir::new("cross-process-unfinished");

    let unfinished = spawn("leave-unfinished", root.path());
    assert!(
        unfinished.status.success(),
        "制造未收尾活动应当成功：{}",
        String::from_utf8_lossy(&unfinished.stderr)
    );

    let proceeded = spawn("must-run", root.path());
    assert!(
        proceeded.status.success(),
        "放行本身应正常退出：{}",
        String::from_utf8_lossy(&proceeded.stderr)
    );
    let report = report_line(&proceeded, "REFUSAL_REPORT:");
    assert_eq!(
        report, "REFUSAL_REPORT:none|calls=1|admission=unfinished(run=run-unfinished)",
        "未收尾的旧活动不再拒绝新活动（模型照常被调用），而它照旧在准入读数里；\
         而「不得冒充存储故障」由上面那条 paused 用例守着"
    );
}

/// **新的一段 + 那张纸条**：进程 A 留下一个未收尾的活动，进程 B 起来开**新的一段**——
/// 它**不再被拦**（上一条用例），而且**开场那一轮**如实告诉它"上一段没有正常收场"。
///
/// ⚠ **旧规矩是什么、新规矩是什么**（2026-09-26）：这条不变量从前由**准入**守着——未收尾的活动
/// 拦住新活动（旧读数 `REFUSAL_REPORT:unfinished|run-unfinished`），于是"上一段没走完"这件事
/// **永远到不了主体手里**（它只到操作者手里，而操作者还得自己去接）。
/// 新规矩：**承载活动的进程没了就是两个 Run**——准入不再拦，那句话改由**新的一段的开场
/// 那一轮**带给主体（`previous-activity`，见 `src/previous_activity.rs`）。
/// ⇒ 不变量本身（"主体必须知道上一段没有正常收场"）**一个字没变**，换的是**载体**：
/// 从"操作者看到的拒绝读数"换成"主体开场读到的那块纸条"。
///
/// 为什么必须**两个真进程**：这一段与上一段之间隔着一次**进程消失**——那正是要证明的形状；
/// 同一个进程里造出来的"未收尾"证明不了"新进程起来看见的是上一段"。
#[test]
fn a_new_segment_is_told_the_previous_one_did_not_finish() {
    let root = TempDir::new("cross-process-note");

    let unfinished = spawn("leave-unfinished", root.path());
    assert!(
        unfinished.status.success(),
        "制造未收尾活动应当成功：{}",
        String::from_utf8_lossy(&unfinished.stderr)
    );

    let next = spawn("note-in-new-segment", root.path());
    assert!(
        next.status.success(),
        "新的一段应当正常退出：{}",
        String::from_utf8_lossy(&next.stderr)
    );
    assert_eq!(
        report_line(&next, "NOTE_REPORT:"),
        "NOTE_REPORT:true|true",
        "新的一段的开场必须有那块纸条，而且它必须点名上一段那个未收尾的活动"
    );
}

/// 端到端入口本身的冒烟：`run_activity_in_environment` 能把身份环境与文件 App 一起接上。
#[test]
fn the_environment_entry_wires_identity_and_files_together() {
    let root = TempDir::new("entry-smoke");
    let sandbox = root.path().join("sandbox");
    std::fs::create_dir_all(&sandbox).expect("建沙盒");

    let mut env = Environment::create(&root.path().join("env"), agent_a()).expect("建环境");
    let mut model = FinishingModel;

    let run = run_activity_in_environment(
        &sandbox,
        &mut env,
        EnvironmentActivity {
            declared_agent: &agent_a(),
            run: RunId::new("run-smoke"),
            limits: celira_agent::core::Limits::new(4),
            input: RunInput::empty(),
            source: &mut NoContext,
            creation: None,
        },
        &mut model,
    )
    .expect("这次活动应正常收场");

    assert_eq!(run.run.id().as_str(), "run-smoke");
    assert!(env.trace_records() > 0, "经历必须真的写进环境");
}

/// 立刻收场的模型：这里只需要证明"入口能把环境接上"，不需要真的做事。
struct FinishingModel;

impl ModelPort for FinishingModel {
    fn complete(&mut self, _view: &View) -> Result<ModelOutput, ModelError> {
        Ok(ModelOutput::finish(SubjectEnding::Silent))
    }
}

// ---------------------------------------------------------------------------
// 测试目录
// ---------------------------------------------------------------------------

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "celira-phase08-x-{label}-{}-{n}",
            std::process::id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("清掉同名残留目录");
        }
        std::fs::create_dir_all(&path).expect("建立测试目录");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
