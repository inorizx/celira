//! **住户的进程寿命**：住户名下起的一切，随住户一起结束（2026-10-11 主人裁决）。
//!
//! 这一格要的是**独立进程证据**：一个真的脱离了父进程的后代（真机同形——
//! `parent=(父进程已不在)`），在住户进程结束之后**必须消失**。
//!
//! ⚠ 同一条用例里带一个**负对照**：不收容时它必须**活着**。没有那一条，上面那句断言
//! 会"永远为真"——一条恒真的断言不是证据。
//!
//! 夹具**全是这一份 exe 自己**（换三个身份跑），不依赖这台机器上的任何程序：
//! - `…_HELPER=1`：收容自己（或不收容，看 `…_ADOPT`）→ 起一个**脱离**的后代 → 写下它的
//!   pid → **故意不等它**就退出；
//! - `…_DESCENDANT=1`：什么都不做，睡够（这就是"一件活着的、脱离了父进程的事"）。
//!
//! 用同一份 exe 而不是 `ping`：`ping` 会把**它自己的**行为（网络、沙箱、管道继承）
//! 带进这条判据里——第一版就是这么写的，结果整条命令被一个攥着管道的后代拖了 29 秒，
//! 而"后代还在不在"这件事反而量不清。

use celira_agent::resident_lifetime::adopt_current_process;

#[cfg(windows)]
use std::fs;
#[cfg(windows)]
use std::process::{Command, Stdio};
#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(windows)]
use celira_agent::resident_lifetime::CONTAINED;

/// 夹具模式（本 exe 换一个身份跑）。
const HELPER_ENV: &str = "CELIRA_RESIDENT_LIFETIME_HELPER";
/// 夹具这一跑要不要收容（`0` ⇒ 负对照那一档）。
const ADOPT_ENV: &str = "CELIRA_RESIDENT_LIFETIME_ADOPT";
/// 后代 pid 写到哪个文件。
const PID_FILE_ENV: &str = "CELIRA_RESIDENT_LIFETIME_PID_FILE";
/// 后代模式（睡够再走）。
const DESCENDANT_ENV: &str = "CELIRA_RESIDENT_LIFETIME_DESCENDANT";
/// 夹具把**收容读数**写到哪个文件（与 pid 同目录）。
///
/// ⚠ 为什么不从 stdout 读：子进程**会继承父进程管道的副本**（本仓 2026-10-11 那个
/// "攥着管道"的真机形状就是它），于是"等它打印完"会变成"等它的后代也退出"——第一版
/// 就是这么写的，一条本该 1 秒的用例被拖了 30 秒。落文件不碰管道，读数一样拿得到。
const READING_FILE_ENV: &str = "CELIRA_RESIDENT_LIFETIME_READING_FILE";

/// 宿主那一行读数的形状（与 `agent_host` 打的那一行逐字同形）。
const READING_LINE: &str = "[resident] processes = ";

/// 后代活多久（够长：判据是"它随住户一起消失"，不是"它自然老死"）。
#[cfg(windows)]
const DESCENDANT_LIFE: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// 夹具（同一份 exe，三个身份）
// ---------------------------------------------------------------------------

/// **后代身份**：什么都不做，睡够。正常跑测试套件时它立刻返回。
#[cfg(windows)]
#[test]
fn resident_lifetime_descendant() {
    if std::env::var(DESCENDANT_ENV).as_deref() == Ok("1") {
        std::thread::sleep(DESCENDANT_LIFE);
    }
}

/// **夹具身份**：收容自己（可选）→ 起一个脱离的后代 → 写下 pid → 退出。
/// 正常跑测试套件时它什么都不做（真正的判据在下面那条用例里）。
#[cfg(windows)]
#[test]
fn resident_lifetime_helper() {
    if std::env::var(HELPER_ENV).as_deref() != Ok("1") {
        return;
    }
    if std::env::var(ADOPT_ENV).as_deref() == Ok("1") {
        let reading = adopt_current_process();
        println!("{READING_LINE}{reading}");
        let reading_file = std::env::var(READING_FILE_ENV).expect("夹具要有读数文件路径");
        fs::write(reading_file, format!("{READING_LINE}{reading}")).expect("夹具要写得下读数");
        assert_eq!(reading, CONTAINED, "夹具这一跑必须收容成功");
    }
    // 一个**脱离**的后代：父进程一走它就成了孤儿。故意不 wait、不杀——它活着才是这一格的意义。
    #[allow(
        clippy::zombie_processes,
        reason = "这一格的判据就是「后代活得比父进程久」——等它就不是这条用例了"
    )]
    let child = Command::new(std::env::current_exe().expect("拿得到本 exe"))
        .args(["--exact", "resident_lifetime_descendant"])
        .env(DESCENDANT_ENV, "1")
        // 后代只是后代：不许继承夹具身份（否则它会再当一次夹具）。
        .env_remove(HELPER_ENV)
        .env_remove(ADOPT_ENV)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("夹具要起得起后代");
    let pid_file = std::env::var(PID_FILE_ENV).expect("夹具要有 pid 文件路径");
    fs::write(pid_file, child.id().to_string()).expect("夹具要写得下 pid");
    // 走到这里就退出——后代留在世上（这正是"孤儿"那个形状）。
}

// ---------------------------------------------------------------------------
// 判据
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn a_detached_descendant_dies_with_the_resident() {
    // 正例：收容 ⇒ 后代随住户一起走。
    let contained_pid = run_helper(true);
    assert!(
        wait_until_gone(contained_pid, Duration::from_secs(20)),
        "住户进程结束之后，它名下的后代必须消失（pid {contained_pid} 还活着：{}）——\
         收容没生效，或者那条限制没设上",
        tasklist_row(contained_pid)
    );

    // 负对照：**不收容** ⇒ 它必须活着。否则上面那条断言恒真。
    let control_pid = run_helper(false);
    assert!(
        !wait_until_gone(control_pid, Duration::from_secs(3)),
        "负对照：没有收容时后代本来就该活着（pid {control_pid} 提前消失了：{}）——\
         那说明这条用例量不到它想量的东西",
        tasklist_row(control_pid)
    );

    // 收尾：负对照留下的那个后代由这里收掉，并确认收干净（用例不许留孤儿）。
    kill(control_pid);
    assert!(
        wait_until_gone(control_pid, Duration::from_secs(15)),
        "负对照的后代必须被收掉（pid {control_pid}：{}）",
        tasklist_row(control_pid)
    );
}

/// 没有臂的平台：**如实说没有**，不换一个近似兜底。
#[cfg(not(windows))]
#[test]
fn the_platform_without_an_arm_says_so() {
    assert_eq!(
        adopt_current_process(),
        celira_agent::resident_lifetime::NOT_CONTAINED_NO_ARM,
        "本平台今天没有臂 ⇒ 读数必须是 not-contained:no-arm"
    );
}

/// 跑一次夹具，返回它留下的那个后代的 pid。
#[cfg(windows)]
fn run_helper(adopt: bool) -> u32 {
    let stem = std::env::temp_dir().join(format!(
        "celira-resident-lifetime-{}-{}",
        std::process::id(),
        if adopt { 1 } else { 0 }
    ));
    let pid_file = stem.with_extension("pid");
    let reading_file = stem.with_extension("reading");
    let _ = fs::remove_file(&pid_file);
    let _ = fs::remove_file(&reading_file);
    // ⚠ `spawn` + `wait`，**不用 `output()`**：那会等到管道的副本全关（含后代），
    // 于是"夹具跑完了吗"变成"后代也退出了吗"——判据就读不出来了。
    let mut helper = Command::new(std::env::current_exe().expect("拿得到本 exe"))
        .args(["--exact", "resident_lifetime_helper"])
        .env(HELPER_ENV, "1")
        .env(ADOPT_ENV, if adopt { "1" } else { "0" })
        .env(PID_FILE_ENV, &pid_file)
        .env(READING_FILE_ENV, &reading_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("夹具要跑得起来");
    let status = helper.wait().expect("夹具要跑得完");
    assert!(
        status.success(),
        "夹具自己得跑过（收容={adopt}）：{status:?}"
    );
    if adopt {
        let reading = fs::read_to_string(&reading_file).unwrap_or_default();
        assert!(
            reading.contains(&format!("{READING_LINE}{CONTAINED}")),
            "收容读数必须是 contained（受限环境会让它失败，那时这里要看得到原因）：「{reading}」"
        );
    }
    let text = fs::read_to_string(&pid_file).expect("夹具要写下后代的 pid");
    text.trim().parse().expect("pid 要是数字")
}

/// 那个 pid 在 `tasklist` 里那一行（诊断用；不在这里下结论）。
///
/// 空的时候把 **stderr** 也带上：读数读不到时，"读不到"与"没有"必须分得开。
#[cfg(windows)]
fn tasklist_row(pid: u32) -> String {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("tasklist 要跑得起来");
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return format!("<空读数：exit={:?} stderr={stderr}>", output.status.code());
    }
    stdout
}

/// 那个 pid 还活着吗（`tasklist` 是这台机器上现成的读数，不再为它手写一个 FFI）。
#[cfg(windows)]
fn is_alive(pid: u32) -> bool {
    tasklist_row(pid).contains(&pid.to_string())
}

/// 在窗口内等它消失；到点还在 ⇒ 假。
#[cfg(windows)]
fn wait_until_gone(pid: u32, window: Duration) -> bool {
    let deadline = Instant::now() + window;
    while Instant::now() < deadline {
        if !is_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

#[cfg(windows)]
fn kill(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}
