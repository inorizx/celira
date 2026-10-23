//! TASK_36 验收：terminal 窗——档位、五堵墙、三态如实、效果归类。
//!
//! 平台策略是「**契约唯一、实现多份**」：编排与五堵墙只写一份，平台臂（今天两支：
//! Windows＝PowerShell、Linux＝bash）只提供四件。**真执行的那些用例**用
//! `#[cfg(any(windows, target_os = "linux"))]` 门控，命令表按平台参数化（同一批断言，两份
//! 等价命令——Windows 那列逐字照搬，Linux 那列是等价 bash）；别的平台上判的是另一件事——
//! **缝 fail-closed**（那一档根本没有实现，不静默换行为）。
//!
//! 判分只认回执：`exit` / 两条流的字节数与截断标记 / `scrubbed` / `unscrubbed` /
//! 失败分类。测试里的命令都是**真的跑**（PowerShell 或 bash），因为这一层的全部意义就是真跑。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use celira_agent::app_contract::{AppPort, EffectClass};
use celira_agent::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort,
};
use celira_agent::foundation::{AgentId, OperationId, ResultState, RunId, Turn};
use celira_agent::terminal_app::{
    CAPABILITY_HELD, CAPABILITY_RUN, CAPABILITY_STOP, ENCODING_UTF8, MAX_COMMAND_BYTES,
    MAX_OUTPUT_BYTES, MAX_TIMEOUT_SECONDS, TERMINAL_APP_ID, TerminalAccess, TerminalApp,
    TerminalFailure, TerminalTier, UNKNOWN_READING, head_limit, tail_limit,
};

fn agent() -> AgentId {
    AgentId::new("agent-t36")
}

fn operation() -> OperationId {
    OperationId::new(RunId::new("hand"), Turn::FIRST)
}

fn full(scrub: Vec<String>) -> TerminalAccess {
    TerminalAccess::new(TerminalTier::Full, scrub)
}

/// 一次性的环境根：**只有"活到关家"那一族用例真的会往里写**（写的是它自己那本账与输出文件），
/// 别的一律不碰它。
///
/// ⚠ **目录必须真的存在**（2026-10-14 起）：不传 `cwd` 时手就落在这儿，而"目录不存在"
/// 在这一层是**本地拒绝**（与"给了一个不存在的 `cwd`"同一条规矩）。这里建它，换来的是
/// "她的家不在"这件事在用例里不再是一个隐形前提。每次调用是**新的一条路径**：
/// 数账那几条用例不能被别的用例污染。
fn test_root() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let unique = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "celira-terminal-test-{}-{unique}",
        std::process::id()
    ));
    let _ = std::fs::create_dir_all(&path);
    path
}

fn call(app: &mut TerminalApp, params: ActionParams) -> ActionOutcome {
    app.execute(&agent(), &operation(), &run(params))
}

fn run(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_RUN), params)
}

fn command(text: &str) -> ActionParams {
    ActionParams::none().with("command", text)
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

/// 回执里某一条输出流那一行（`stdout = …` / `stderr = …`）。
///
/// 断言要落在**那一行**上，不能只 `contains` 一个字段：两条流都在同一份回执里，
/// 只匹配字段值的话，一条流的读数能替另一条流作证。
fn output_line(receipt: &str, name: &str) -> String {
    receipt
        .lines()
        .find(|line| line.starts_with(&format!("{name} = ")))
        .unwrap_or_default()
        .to_string()
}

/// 用例自己的临时目录。
///
/// ⚠ 名字里**必须带进程号**：测试二进制是一份一个进程，而不带进程号的名字在两份并行
/// （或两个写者同时跑）时算出同一条路径、**互相删目录**（2026-10-09 实测）。
fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("celira-t36-{}-{name}", std::process::id()));
    let _ = std::fs::create_dir_all(&path);
    path
}

// ---------------------------------------------------------------------------
// 平台参数化：同一批断言，两份命令表
// ---------------------------------------------------------------------------

/// 平台命令表：Windows 那列是**逐字照搬**（PowerShell），Linux 那列是等价 bash。
/// 只有真执行用例（`any(windows, target_os = "linux")` 门控）用到它。
#[cfg(any(windows, target_os = "linux"))]
mod cmds {
    /// 只列**变量名**（不打印取值）："值有没有漏"这件事由回执本身回答。
    #[cfg(windows)]
    pub const LIST_ENV_NAMES: &str = "Get-ChildItem Env: | ForEach-Object { $_.Name }";
    #[cfg(target_os = "linux")]
    pub const LIST_ENV_NAMES: &str = "env | cut -d= -f1";

    /// 先睡很久、再落一个记号文件：击杀真的生效时，那句话永远不会执行。
    #[cfg(windows)]
    pub fn sleep_then_mark(marker: &str) -> String {
        format!(
            "Start-Sleep -Seconds 30; New-Item -ItemType File -Path '{marker}' -Force | Out-Null"
        )
    }
    #[cfg(target_os = "linux")]
    pub fn sleep_then_mark(marker: &str) -> String {
        format!("sleep 30; touch '{marker}'")
    }

    /// 落一个记号文件（参数网关用例里那条"一次都不该跑起来"的命令）。
    #[cfg(windows)]
    pub fn create_marker(marker: &str) -> String {
        format!("New-Item -ItemType File -Path '{marker}' -Force | Out-Null")
    }
    #[cfg(target_os = "linux")]
    pub fn create_marker(marker: &str) -> String {
        format!("touch '{marker}'")
    }

    /// 用**相对路径**落一个记号：判"手落在哪个目录"要看行为，不看路径字符串
    /// （大小写、8.3 短名、编码都会让字符串比较变成假红）。
    #[cfg(windows)]
    pub fn mark_relative(name: &str) -> String {
        format!("New-Item -ItemType File -Path '{name}' -Force | Out-Null; Write-Output marked")
    }
    #[cfg(target_os = "linux")]
    pub fn mark_relative(name: &str) -> String {
        format!("touch '{name}'; echo marked")
    }

    /// 40000 字节的输出（超过 32 KiB 上限）。
    #[cfg(windows)]
    pub const OVER_LIMIT_OUTPUT: &str = "'x' * 40000";
    #[cfg(target_os = "linux")]
    pub const OVER_LIMIT_OUTPUT: &str = "printf 'x%.0s' {1..40000}";

    /// 三个记号：**头、中、尾**，总量超过上限（40026 字节左右）。
    ///
    /// 判据一把抓：截断之后**两头在、中间那个记号必须不在**——它证明缺的是中间那一段，
    /// 而不是"只给了一头"。
    #[cfg(windows)]
    pub const MARKS_OVER_LIMIT: &str =
        "'HEAD-MARK' + ('x' * 20000) + 'MID-MARK' + ('x' * 20000) + 'TAIL-MARK'";
    #[cfg(target_os = "linux")]
    pub const MARKS_OVER_LIMIT: &str = "printf 'HEAD-MARK'; printf 'x%.0s' {1..20000}; \
                                        printf 'MID-MARK'; printf 'x%.0s' {1..20000}; printf 'TAIL-MARK'";

    /// 同样的三个记号，但总量**在上限之内**（20026 字节左右，只是超过了头部窗口那一半）。
    ///
    /// 它量的是另一条分支：两段是**连续的**，所以一个字节都不许丢——连中间那个记号也要在。
    #[cfg(windows)]
    pub const MARKS_WITHIN_LIMIT: &str =
        "'HEAD-MARK' + ('x' * 10000) + 'MID-MARK' + ('x' * 10000) + 'TAIL-MARK'";
    #[cfg(target_os = "linux")]
    pub const MARKS_WITHIN_LIMIT: &str = "printf 'HEAD-MARK'; printf 'x%.0s' {1..10000}; \
                                          printf 'MID-MARK'; printf 'x%.0s' {1..10000}; printf 'TAIL-MARK'";

    /// 两条流各说一句话。
    #[cfg(windows)]
    pub const BOTH_STREAMS: &str = "Write-Output 'out'; [Console]::Error.WriteLine('bad')";
    #[cfg(target_os = "linux")]
    pub const BOTH_STREAMS: &str = "echo out; echo bad >&2";

    /// 先说话、再卡住（超时回执里必须交出的那一行）。
    #[cfg(windows)]
    pub const PRINT_THEN_HANG: &str =
        "Write-Output 'printed-before-the-hang'; Start-Sleep -Seconds 30";
    #[cfg(target_os = "linux")]
    pub const PRINT_THEN_HANG: &str = "echo printed-before-the-hang; sleep 30";

    /// 不存在的目录（参数网关用例）。
    #[cfg(windows)]
    pub const NO_SUCH_DIR: &str = "D:/celira-no-such-dir-t36";
    #[cfg(target_os = "linux")]
    pub const NO_SUCH_DIR: &str = "/celira-no-such-dir-t36";

    /// 起不来的 shell 名（缝用例）。
    pub const NO_SUCH_SHELL: &str = "celira-no-such-shell-t36";

    /// 一条什么都不干的命令（只在"起不来的 shell"用例里用：它一次都不会执行）。
    #[cfg(windows)]
    pub const TRIVIAL: &str = "Write-Output hi";
    #[cfg(target_os = "linux")]
    pub const TRIVIAL: &str = "echo hi";

    /// 一条无害命令（参数网关用例里那三个"超时不合法"的载荷：它一次都不会执行）。
    #[cfg(windows)]
    pub const HARMLESS: &str = "Write-Output ok";
    #[cfg(target_os = "linux")]
    pub const HARMLESS: &str = "echo ok";

    /// 一段普通 UTF-8 文本。
    #[cfg(windows)]
    pub const PLAIN_ASCII: &str = "Write-Output 'plain-ascii'";
    #[cfg(target_os = "linux")]
    pub const PLAIN_ASCII: &str = "echo plain-ascii";

    /// 20000 个汉字（60000 字节）：截断窗口（32768 = 3×10922 + 2）正好切在汉字中间。
    #[cfg(windows)]
    pub const MANY_CJK: &str = "'中' * 20000";
    #[cfg(target_os = "linux")]
    pub const MANY_CJK: &str = "printf '中%.0s' {1..20000}";

    /// 直接把**非 UTF-8** 的字节写进 stdout（GBK 的「本地连接」四个字，8 个字节）。
    /// Windows：绕开 PowerShell 自己的编码（`OpenStandardOutput().Write`）；
    /// Linux：`printf` 的十六进制转义原样吐出。
    #[cfg(windows)]
    pub const NON_UTF8_BYTES: &str = "$b=[byte[]]@(0xB1,0xBE,0xB5,0xD8,0xC1,0xAC,0xBD,0xD3); \
         [Console]::OpenStandardOutput().Write($b,0,$b.Length); \
         [Console]::OpenStandardOutput().Flush()";
    #[cfg(target_os = "linux")]
    pub const NON_UTF8_BYTES: &str = "printf '\\xB1\\xBE\\xB5\\xD8\\xC1\\xAC\\xBD\\xD3'";

    /// **泄漏夹具**（2026-10-11）：先落一句记号，再让一个**孙进程**攥住这条管道，而直接子进程
    /// （shell）立刻退出。
    ///
    /// 形状是从真机事故里抄的：`renderloft-cli` 拉起的守护进程攥着**调用方**的管道，而 CLI
    /// 自己的 std 句柄被保护了（所以 shell 退得出来）——泄漏走的是**继承来的副本**，不是 std
    /// 句柄。`start /b`（Windows）与 `&`（POSIX）复现的正是这一格，而且两者都**不攥 shell 自己**
    /// 的输出管道（`> NUL`）：攥了的话 shell 会等它、就退不出来了，那成了另一个形状。
    ///
    /// 孙进程活 30 秒——比宽限窗口长一个量级，所以"动作提前返回了"是可判的。
    #[cfg(windows)]
    pub fn leak_then_exit(marker: &str) -> String {
        format!("Write-Output '{marker}'; cmd /c \"start /b ping -n 30 127.0.0.1 > NUL\"")
    }
    /// ⚠ **Linux 这一支在开发机（Windows）上没跑过**：`&` 起的后台作业握着这条管道、而 shell
    /// 不等它，是 POSIX 上的标准形状（`subprocess.run` 那个经典挂死就是它）；真跑要由 Linux
    /// 那一侧的门禁来证——**没跑过就不说它绿**。
    #[cfg(target_os = "linux")]
    pub fn leak_then_exit(marker: &str) -> String {
        format!("echo '{marker}'; sleep 30 &")
    }

    /// **先灌满头部窗口、再泄漏管道**（2026-10-11 那一刀带来的新责任）。
    ///
    /// 形状与 [`leak_then_exit`] 同源，只多一件事：**先说出 20000 字节**（比头部窗口那一半
    /// 16384 多）。于是"窗口到点时手里已经攒满头部窗口"这一档出现——那一档若不说出来，
    /// 就成了**静默截断**。
    #[cfg(windows)]
    pub fn flood_then_leak(bytes: usize) -> String {
        format!("'x' * {bytes}; cmd /c \"start /b ping -n 30 127.0.0.1 > NUL\"")
    }
    /// Linux 同款（判据与上面那段注释相同：这一支没在开发机上跑过）。
    #[cfg(target_os = "linux")]
    pub fn flood_then_leak(bytes: usize) -> String {
        format!("printf 'x%.0s' {{1..{bytes}}}; sleep 30 &")
    }
}

// ---------------------------------------------------------------------------
// 档位（封闭枚举）
// ---------------------------------------------------------------------------

#[test]
fn the_tier_is_a_closed_enum_and_unknown_names_are_not_guessed() {
    assert_eq!(TerminalTier::parse("full"), Some(TerminalTier::Full));
    assert_eq!(TerminalTier::Full.as_str(), "full");
    // 不认识就是不认识：不降级、不猜、大小写也不顺手接住。
    for unknown in [
        "",
        " ",
        "home-write",
        "read-only",
        "FULL",
        "Full",
        "full ",
        "root",
    ] {
        assert_eq!(
            TerminalTier::parse(unknown),
            None,
            "`{unknown}` 不是一档，必须拒绝而不是就近取一档"
        );
    }
}

/// **本构建里有臂的平台上**，这一档必须可用（今天两支臂：Windows、Linux）。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn the_platform_has_this_tier_where_an_arm_exists() {
    assert!(
        TerminalTier::Full.available(),
        "本构建里编进了这一档的臂（Windows 或 Linux）"
    );
}

// ---------------------------------------------------------------------------
// 五堵墙
// ---------------------------------------------------------------------------

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn the_credentials_are_scrubbed_from_the_child_environment() {
    // 哨兵要挑**确实在本进程环境里、而且外壳不会自己重建**的那种变量。
    // ⚠️ 实测教训：`USERPROFILE` 这类由系统管理的变量被洗掉之后，外壳会**重新造一个出来**
    // ——那不属于"凭据不继承"这件事的语义。凭据类变量不会：外壳不会自己发明它们。
    let sentinel = std::env::vars_os()
        .map(|(key, value)| {
            (
                key.to_string_lossy().to_string(),
                value.to_string_lossy().to_string(),
            )
        })
        .find(|(name, value)| {
            !name.is_empty()
                && !value.is_empty()
                && (name.starts_with("CELIRA_") || name == "DEEPSEEK_API_KEY")
        });
    let Some((name, value)) = sentinel else {
        // 环境里没有凭据形状的变量 ⇒ 这一格**如实报"不适用"**，不当通过。
        eprintln!("terminal_app：环境里既无 CELIRA_* 也无 DEEPSEEK_API_KEY，凭据洗这一格不适用");
        return;
    };

    let mut app = TerminalApp::new(full(vec![name.clone()]), &test_root());

    // 命令只列**变量名**（不打印取值）：这样"值有没有漏"这件事由回执本身回答。
    let outcome = call(&mut app, command(cmds::LIST_ENV_NAMES));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    let receipt = text_of(&outcome);

    assert!(
        !receipt.contains(&name),
        "被洗掉的键名（{name}）不该出现在子进程环境里：{receipt}"
    );
    assert!(
        !receipt.contains(&value),
        "被洗掉那个变量的**取值**更不该出现（它是凭据）：{receipt}"
    );
    assert!(
        receipt.contains("PATH"),
        "没被点名的变量照旧在（对照：不是把环境清空了）：{receipt}"
    );
    assert!(
        receipt.contains("scrubbed = ") && !receipt.contains("scrubbed = 0"),
        "洗掉几个要如实报：{receipt}"
    );
    assert!(
        receipt.contains("unscrubbed = none"),
        "洗不掉的要如实报（这里是 none）：{receipt}"
    );
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_timeout_kills_the_tree_and_the_work_does_not_finish() {
    let marker = scratch("killed").join("done.txt");
    let _ = std::fs::remove_file(&marker);
    let mut app = TerminalApp::new(full(vec![]), &test_root());

    // 先睡很久、再落一个记号文件：如果击杀真的生效，那句话永远不会执行。
    let outcome = call(
        &mut app,
        command(&cmds::sleep_then_mark(&marker.display().to_string())).with("timeout", "1"),
    );
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(
        failure_of(&outcome),
        "timeout",
        "击杀成功 ⇒ 命令中断是已知形状"
    );

    std::thread::sleep(Duration::from_secs(3));
    assert!(
        !marker.exists(),
        "被击杀的进程树不该再往下走一步：{}",
        marker.display()
    );
}

/// **原约束**（本条从前钉的）：留下的正好是上限那么多连续字节——那等于"只给头部"。
/// **新边界**（2026-10-11 改）：截断时**首尾各留一半**，标记落在两段**中间**；预算不变
/// （两头合起来仍是上限那么多）。真实消费者：构建、测试、报错的结论都在**尾部**，
/// 只给头部等于把"为什么失败"丢掉。
///
/// 负向断言一条都没放松，反而多了一条：**上限那么多连续字节从此不可能出现**——那正是
/// "标记真的插在中间"的证据。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn output_over_the_limit_keeps_both_ends_and_says_what_is_missing() {
    let mut app = TerminalApp::new(full(vec![]), &test_root());
    let outcome = call(&mut app, command(cmds::OVER_LIMIT_OUTPUT));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    let receipt = text_of(&outcome);

    assert!(
        receipt.contains("truncated = true"),
        "超限要**说出来**（显式截断合法，静默截断违禁）：{receipt}"
    );
    assert!(
        receipt.contains("此处截断"),
        "正文里也要有明确标记：{receipt}"
    );

    let head = head_limit(MAX_OUTPUT_BYTES);
    let tail = tail_limit(MAX_OUTPUT_BYTES);
    assert!(
        receipt.contains(&"x".repeat(head)),
        "开头那一段要在（窗口是 {head} 字节）：{receipt}"
    );
    assert!(
        receipt.contains(&"x".repeat(tail)),
        "结尾那一段也要在（窗口是 {tail} 字节）：{receipt}"
    );
    assert!(
        !receipt.contains(&"x".repeat(head + 1)),
        "开头不许超过自己那个窗口：{receipt}"
    );
    assert!(
        !receipt.contains(&"x".repeat(tail + 1)),
        "结尾不许超过自己那个窗口：{receipt}"
    );
    assert!(
        !receipt.contains(&"x".repeat(MAX_OUTPUT_BYTES)),
        "上限那么多**连续**字节从此不可能出现——标记就插在两段中间：{receipt}"
    );
    assert!(
        receipt.contains(&format!("开头 {head} 字节与结尾 {tail} 字节")),
        "缺口有多大，标记里要说出来：{receipt}"
    );

    // 读数那一行给的是这条流的**真总数**（不是留下来的那一段）——她照着它才知道还差多少。
    let stdout_reading = receipt
        .lines()
        .find(|line| line.starts_with("stdout = "))
        .unwrap_or("<没有读数>");
    let total: usize = stdout_reading
        .trim_start_matches("stdout = ")
        .split(' ')
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    assert!(
        total > MAX_OUTPUT_BYTES,
        "字节数是真总数，不是留下来的那一段：{stdout_reading}"
    );
}

/// **两端都在、中间那个记号不在**——这是"缺的是中间那一段"的直接证据（比"长度对不对"硬）。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn the_two_ends_survive_and_the_middle_is_what_is_missing() {
    let mut app = TerminalApp::new(full(vec![]), &test_root());
    let outcome = call(&mut app, command(cmds::MARKS_OVER_LIMIT));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    let receipt = text_of(&outcome);

    assert!(receipt.contains("truncated = true"), "{receipt}");
    assert!(receipt.contains("HEAD-MARK"), "开头那一段要在：{receipt}");
    assert!(receipt.contains("TAIL-MARK"), "结尾那一段要在：{receipt}");
    assert!(
        !receipt.contains("MID-MARK"),
        "中间那一段是真的缺了（它在缺口里）：{receipt}"
    );
    // 顺序也要对：头在前、尾在后，标记夹在中间。
    let head_at = receipt.find("HEAD-MARK").unwrap_or(usize::MAX);
    let cut_at = receipt.find("此处截断").unwrap_or(0);
    let tail_at = receipt.find("TAIL-MARK").unwrap_or(0);
    assert!(
        head_at < cut_at && cut_at < tail_at,
        "正文的顺序是「头 → 标记 → 尾」：{receipt}"
    );
}

/// 另一条分支：总量**在上限之内**（只是超过了头部窗口那一半）⇒ 两段是**连续的**，
/// 一个字节都不许丢——连中间那个记号也要在。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_stream_within_the_budget_is_whole_even_when_it_crosses_the_head_window() {
    let mut app = TerminalApp::new(full(vec![]), &test_root());
    let outcome = call(&mut app, command(cmds::MARKS_WITHIN_LIMIT));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    let receipt = text_of(&outcome);

    assert!(
        receipt.contains("truncated = false"),
        "没超上限就不叫截断：{receipt}"
    );
    assert!(
        !receipt.contains("此处截断"),
        "没有缺口就不要插标记：{receipt}"
    );
    for mark in ["HEAD-MARK", "MID-MARK", "TAIL-MARK"] {
        assert!(
            receipt.contains(mark),
            "{mark} 必须原样在场（两段是连续的，谁都不许丢）：{receipt}"
        );
    }
    assert!(
        receipt.contains(&format!("encoding = {ENCODING_UTF8}")),
        "拼起来解一次，读数照旧是 utf-8：{receipt}"
    );
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_stream_is_printed_once_and_the_overlapping_windows_do_not_repeat_it() {
    // 短输出：整段同时落在头部窗口与尾部窗口里 ⇒ 拼接时重叠的那一截必须剪掉。
    let mut app = TerminalApp::new(full(vec![]), &test_root());
    let short = call(&mut app, command(cmds::PLAIN_ASCII));
    let receipt = text_of(&short);
    assert_eq!(
        receipt.matches("plain-ascii").count(),
        1,
        "同一段输出只许出现一次（读数说的是 stdout 的真字节数）：{receipt}"
    );

    // 没超上限但跨过头部窗口：两段窗口重叠 12 KiB ⇒ 中间的记号只许出现一次。
    let within = call(&mut app, command(cmds::MARKS_WITHIN_LIMIT));
    let receipt = text_of(&within);
    for mark in ["HEAD-MARK", "MID-MARK", "TAIL-MARK"] {
        assert_eq!(
            receipt.matches(mark).count(),
            1,
            "{mark} 只许出现一次（重叠窗口不许把接缝印两遍）：{receipt}"
        );
    }
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn the_exit_code_and_both_streams_are_readings_not_failures() {
    let mut app = TerminalApp::new(full(vec![]), &test_root());

    // 非零退出码是**读数**：跑起来了、跑完了就是成功。（`exit 3` 两个平台同形。）
    let nonzero = call(&mut app, command("exit 3"));
    assert_eq!(
        nonzero.state(),
        ResultState::Success,
        "{}",
        text_of(&nonzero)
    );
    assert!(
        text_of(&nonzero).contains("exit = 3"),
        "{}",
        text_of(&nonzero)
    );

    // 两条流各归各位。
    let two = call(&mut app, command(cmds::BOTH_STREAMS));
    let receipt = text_of(&two);
    assert!(receipt.contains("exit = 0"), "{receipt}");
    assert!(receipt.contains("--- stdout ---"), "{receipt}");
    assert!(receipt.contains("out"), "{receipt}");
    assert!(receipt.contains("--- stderr ---"), "{receipt}");
    assert!(receipt.contains("bad"), "{receipt}");
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn the_parameter_gateway_refuses_before_anything_runs() {
    let marker = scratch("gateway").join("should-not-exist.txt");
    let _ = std::fs::remove_file(&marker);
    let mut app = TerminalApp::new(full(vec![]), &test_root());
    let create = cmds::create_marker(&marker.display().to_string());

    // 空命令 / 超长 / 工作目录不存在 / 超时不合法：一律 `invalid-argument`，且**一个动作都没有发生**。
    let cases = vec![
        command(""),
        command("   "),
        command(&"c".repeat(MAX_COMMAND_BYTES + 1)),
        command(&create).with("cwd", cmds::NO_SUCH_DIR),
        command(cmds::HARMLESS).with("timeout", "0"),
        command(cmds::HARMLESS).with("timeout", (MAX_TIMEOUT_SECONDS + 1).to_string()),
        command(cmds::HARMLESS).with("timeout", "abc"),
    ];
    for params in cases {
        let outcome = call(&mut app, params);
        assert_eq!(outcome.state(), ResultState::Failure);
        assert_eq!(failure_of(&outcome), "invalid-argument");
    }
    assert!(
        !marker.exists(),
        "参数网关先于执行：那条命令一次都不该跑起来"
    );
}

/// **手落在哪儿**（2026-10-14）：不传 `cwd` ⇒ 在**她自己的家**里执行；传了 ⇒ 在她点名的那一个。
///
/// 判据用**行为**（相对路径落到哪个目录），不用路径字符串：大小写、8.3 短名、编码都会让
/// 字符串比较变成假红。它同时也证明手册那句承诺（"能跑的文本放你自己家里的 `projects/`"）
/// 在相对路径这一层是真的。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn without_cwd_the_hand_works_in_her_own_home() {
    let home = test_root();
    let elsewhere = scratch("elsewhere");
    let mut app = TerminalApp::new(full(vec![]), &home);

    let in_home = "mark-in-home.txt";
    let outcome = call(&mut app, command(&cmds::mark_relative(in_home)));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    assert!(
        home.join(in_home).exists(),
        "不传 cwd ⇒ 相对路径落在她自己家里（默认是一个声明的地址，不是进程碰巧从哪儿起）"
    );

    let named = "mark-where-i-said.txt";
    let outcome = call(
        &mut app,
        command(&cmds::mark_relative(named)).with("cwd", elsewhere.display().to_string()),
    );
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    assert!(
        elsewhere.join(named).exists() && !home.join(named).exists(),
        "给了 cwd ⇒ 就落在她点名的那一个目录里（这一档仍是整机全权）"
    );
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn the_shell_seam_is_declared_and_a_shell_that_cannot_start_is_spawn_failure() {
    // 缝：换个起不来的 shell ⇒ 如实报"压根没跑起来"，**不**静默换一个近似的。
    let access = full(vec![]).with_shell(cmds::NO_SUCH_SHELL);
    let mut app = TerminalApp::new(access, &test_root());
    let outcome = call(&mut app, command(cmds::TRIVIAL));
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(failure_of(&outcome), "spawn-failure");
}

// ---------------------------------------------------------------------------
// 效果归类（显式的第四类）
// ---------------------------------------------------------------------------

/// **第五堵墙**：**没有 OS 沙箱，也不假装有**。
///
/// 它是一条**否定**判据，因此只能钉在词表上：失败词表是**封闭五项**，里面没有、也不许有
/// 「第三种隔离说法」（`sandbox-unavailable` 那种既像隔离又像故障的词）。理由：`full` 档是
/// 「这台机器随主体用」的**信任声明**，不是安全豁免；因此**不存在**「沙箱不可用」这种读法
/// ——它只会让人以为我们有隔离、只是坏了。
#[test]
fn there_is_no_third_isolation_word_in_the_failure_vocabulary() {
    // 词表五项（封闭）：逐条点名，多一个都不许。
    let vocabulary = [
        TerminalFailure::InvalidArgument,
        TerminalFailure::Timeout,
        TerminalFailure::SpawnFailure,
        TerminalFailure::OutputLost,
        TerminalFailure::Unknown,
    ];
    let words: Vec<&str> = vocabulary.iter().map(|each| each.as_str()).collect();
    assert_eq!(
        words,
        vec![
            "invalid-argument",
            "timeout",
            "spawn-failure",
            "output-lost",
            "unknown"
        ],
        "失败词表五项封闭、取值逐字"
    );
    for banned in [
        "sandbox",
        "isolation",
        "isolated",
        "jail",
        "container",
        "permission",
    ] {
        assert!(
            !words.iter().any(|word| word.contains(banned)),
            "失败词表里不许出现「隔离/权限」类说法（{banned}）——没有沙箱这件事不写成一种故障"
        );
    }
    // 档位那一侧同款：`full` 是**唯一**一档（封闭枚举），不许有第二档。
    assert_eq!(TerminalTier::Full.as_str(), "full");
}

#[test]
fn the_write_verbs_are_writes_with_no_verification() {
    let app = TerminalApp::new(full(vec![]), &test_root());
    assert_eq!(app.app_id(), TERMINAL_APP_ID);

    // ⚠ **原约束 → 新边界**（2026-10-11，「回得来」那一刀）：这条用例从前钉的是
    // **「恰好一条动词」**（本件只有 `terminal_run`）。那一刀给本件加了两个动词——
    // **一条读**（`terminal_held`：看我起过、还留着的东西）与**一条写**（`terminal_stop`：
    // 收掉一件）。⇒ 约束本身变了：今天该钉的是**「写的那几条都在这里、都是第四类、都没有
    // 同参复核」**，而**读的那一条不许出现在写效应表里**（读不是写）。这不是把断言放松：
    // 下面同时钉住"恰好两条写"与"读的那条不在其中"——从前那条"恰好一条"防的是"悄悄多出一条
    // 写"，今天同一件事由"这两条、且只有这两条"来防。
    let effects = app.declared_effects();
    assert_eq!(
        effects.len(),
        2,
        "写的那两条：起一件（run）与收一件（stop）"
    );
    let named: Vec<&str> = effects.iter().map(|each| each.action().as_str()).collect();
    assert!(
        named.contains(&CAPABILITY_RUN) && named.contains(&CAPABILITY_STOP),
        "写效应表里就是这两条（实际：{named:?}）"
    );
    assert!(
        !named.contains(&CAPABILITY_HELD),
        "**读不是写**：`terminal_held` 一个字节都不改，它不该出现在写效应表里"
    );
    for effect in &effects {
        assert_eq!(effect.class(), EffectClass::UnverifiableWrite);
        assert!(
            effect.class().is_write(),
            "它在声明里仍然是写（第四类豁免的只是「必须配复核」）"
        );
        assert!(
            effect.verification().is_none(),
            "终端没有同参复核：不配 verify，而且这一点是**显式**的——\
             起一件、收一件都没有「再读一次就知道那次成了没有」的复核"
        );
    }
    // ⚠ **旧规矩 → 新规矩**（2026-09-26，两刀）：这里原来还把这份声明装进一台 OS，再从
    // **效果声明表**（`os.effects()`：`is_write` / `class_if_assembled` / `verification_of`）
    // 把同样三件事读一遍——那张表是**未决效果门**的输入，而门已随跨进程恢复退役；门一走，
    // 表就只剩"装配期收集、之后无人读"这一半，因此本刀把表一起退役。三件事一个字没少：
    // 它们现在直接读**声明自己**（表本来就是这份声明的机械收集）。
}

#[cfg(not(any(windows, target_os = "linux")))]
#[test]
fn the_platform_seam_is_fail_closed_elsewhere() {
    // 没有能用的臂的平台上：这一档没有实现（入口会拒绝开启），而且件本身也不换一个近似行为。
    assert!(
        !TerminalTier::Full.available(),
        "没有臂的平台必须 fail-closed"
    );
    let mut app = TerminalApp::new(full(vec![]), &test_root());
    let outcome = call(&mut app, command("echo hi"));
    assert_eq!(failure_of(&outcome), "spawn-failure");
}

// ---------------------------------------------------------------------------
// 失败也带正文
// ---------------------------------------------------------------------------

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_timeout_still_hands_over_what_it_managed_to_print() {
    let mut app = TerminalApp::new(full(vec![]), &test_root());

    // 先说话、再卡住：那条命令**在被杀之前**已经打印了一行。
    //
    // ⚠ **预算必须把前提保证住**（2026-10-09）：这一条的判据是"超时的回执里要有**已经印出来的**
    // 那一行"，而那要求子进程**真的起来并写出那一行**。子进程是本机的 shell（Windows 上是
    // PowerShell），实测启动到打印 **≈200 ms**（空闲时五次测量 196–210 ms）⇒ 原来给的 1 秒只有
    // **5 倍余量**，满载（全套 60 个测试二进制一起跑）时不够 ⇒ 偶发假红。5 秒是 **25 倍余量**：
    // 这一格要判的从来不是"启动有多快"，而是"超时之后那些字节还在不在"。
    let outcome = call(
        &mut app,
        command(cmds::PRINT_THEN_HANG).with("timeout", "5"),
    );

    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(failure_of(&outcome), "timeout");
    let receipt = text_of(&outcome);

    assert!(
        !receipt.is_empty(),
        "失败不该比成功少给东西：超时的回执里必须有已经读到的正文与读数"
    );
    assert!(
        receipt.contains("printed-before-the-hang"),
        "卡住之前打印的那一行必须交出来：{receipt}"
    );
    assert!(
        receipt.contains("killed = true"),
        "这次是**击杀成功**（已知形状），要如实报：{receipt}"
    );
    assert!(
        receipt.contains("stdout = ") && receipt.contains("truncated = false"),
        "两条流的读数照给：{receipt}"
    );
    assert!(
        receipt.contains("note = "),
        "要说清这次只保证到什么程度：{receipt}"
    );
    // 时长是主体判断"还等不等"的材料：它必须带"等了多久 / 窗口多长"（秒级），
    // 而且必须是**已经发生的**读数——不预言、不编造。
    assert!(
        receipt.contains("waited = ") && receipt.contains("window = 5.0s"),
        "超时要交出等了多久与窗口：{receipt}"
    );
    assert!(
        !receipt.contains("exit = "),
        "进程是被杀的 ⇒ **没有退出码可报**，不许编一个出来：{receipt}"
    );
}

// ---------------------------------------------------------------------------
// 编码：字节怎么变成字，是一份读数
// ---------------------------------------------------------------------------

/// 非 UTF-8 的字节要**如实说**——每个平台说它自己的那份读数：Windows 按本机 OEM 代码页解
/// （`cp<页码>`，解不出就 `not-utf8`）；Linux 没有「本机代码页」这回事，如实说 `not-utf8`、
/// 只做有损替换。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn bytes_that_are_not_utf8_are_said_out_loud_by_the_platform_reading() {
    let mut app = TerminalApp::new(full(vec![]), &test_root());
    let outcome = call(&mut app, command(cmds::NON_UTF8_BYTES));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    let receipt = text_of(&outcome);

    // **共同判据**：八个字节一个不少地被读出来，而且这段字节**不许**被当成合法 UTF-8
    // 蒙混过去——每个平台都得把"它不是 UTF-8"这件事说出来。
    let stdout_reading = receipt
        .lines()
        .find(|line| line.starts_with("stdout = "))
        .unwrap_or("<没有读数>");
    assert!(
        stdout_reading.contains("stdout = 8 bytes"),
        "八个字节要一个不少地读出来：{receipt}"
    );
    assert!(
        !stdout_reading.contains(&format!("encoding = {ENCODING_UTF8}")),
        "非 UTF-8 的字节不许被读成 utf-8：{receipt}"
    );

    #[cfg(windows)]
    {
        assert!(
            !receipt.contains('\u{FFFD}'),
            "非 UTF-8 的字节**不许**被静默伪造成替换字符：{receipt}"
        );
        assert!(
            receipt.contains("encoding = cp"),
            "按代码页解了就要把页码说出来（那是读数，不是猜）：{receipt}"
        );
        // 强断言只在**这台机器的 OEM 代码页确实是 936** 时成立——不是 936 就如实报"不适用"，
        // 不假装判过（与"凭据洗"那一格同款纪律）。
        if receipt.contains("encoding = cp936") {
            assert!(
                receipt.contains("本地连接"),
                "GBK 的那八个字节要解成那四个字：{receipt}"
            );
        } else {
            eprintln!(
                "terminal_app：本机 OEM 代码页不是 936（读数：{stdout_reading}），「解成哪四个字」这一格不适用"
            );
        }
    }

    #[cfg(target_os = "linux")]
    {
        assert!(
            stdout_reading.contains("encoding = not-utf8"),
            "没有「本机代码页」这回事 ⇒ 如实说「这段不是 UTF-8」（不假装知道它是什么编码）：{receipt}"
        );
        assert!(
            receipt.contains('\u{FFFD}'),
            "认不出编码时只做**有损替换**——而且要把读数说出来（上面那条），不静默：{receipt}"
        );
    }
}

#[cfg(any(windows, target_os = "linux"))]
#[test]
fn utf8_output_is_decoded_as_utf8_and_a_cut_character_is_not_faked() {
    let mut app = TerminalApp::new(full(vec![]), &test_root());

    // 普通输出：合法 UTF-8 ⇒ 照 UTF-8 解，读数说 utf-8。
    let plain = call(&mut app, command(cmds::PLAIN_ASCII));
    let receipt = text_of(&plain);
    assert!(receipt.contains("plain-ascii"), "{receipt}");
    assert!(
        receipt.contains(&format!("encoding = {ENCODING_UTF8}")),
        "合法 UTF-8 就落在 utf-8 这一档：{receipt}"
    );

    // 截断正好切在一个汉字中间（32768 = 3×10922 + 2）：**不许**把切出来的半个字符伪造成
    // 替换字符——那不是"编码不对"，是我们自己切的位置。
    let cut = call(&mut app, command(cmds::MANY_CJK));
    let receipt = text_of(&cut);
    assert_eq!(cut.state(), ResultState::Success, "{receipt}");
    assert!(receipt.contains("truncated = true"), "{receipt}");
    assert!(
        !receipt.contains('\u{FFFD}'),
        "被截断窗口切掉的半个字符不许变成替换字符：{receipt}"
    );
    assert!(
        receipt.contains(&format!("encoding = {ENCODING_UTF8}")),
        "剪掉那半个字符之后，这一段仍然是 UTF-8：{receipt}"
    );
}

// ---------------------------------------------------------------------------
// Linux 侧真执行（bash 臂）：施工单点名的几格
// ---------------------------------------------------------------------------

/// **同词一致**：机器处境说的 shell（`machine.rs` 的唯一映射：linux → bash）与终端默认起的
/// shell，必须是**同一个词**——否则主体会按事实写 bash 语法、却被另一只 shell 执行。
///
/// 用 bash **专有**语法（`[[ ]]` 在 POSIX sh 里是语法错）真跑一条，再把执行者自己的名字
/// 读回来（`${BASH##*/}`），与那一行的词对照。
#[cfg(target_os = "linux")]
#[test]
fn the_bash_arm_speaks_the_same_word_as_the_machine_situation() {
    assert_eq!(
        celira_agent::machine::Platform::Linux.shell(),
        "bash",
        "机器处境那一行说的词（唯一一份映射）"
    );
    assert_eq!(
        celira_agent::platform::TERMINAL_SHELL,
        "bash",
        "臂的默认 shell 名（共用平台模块那一件）与机器处境说**同一个词**"
    );

    let mut app = TerminalApp::new(full(vec![]), &test_root());
    let outcome = call(
        &mut app,
        command("[[ -n \"$BASH_VERSION\" ]] && echo \"shell=${BASH##*/}\""),
    );
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    let receipt = text_of(&outcome);
    assert!(
        receipt.contains("shell=bash"),
        "默认起的必须是 bash 自己（bash 专有语法真跑通）：{receipt}"
    );
}

/// **profile 缝被摘掉**：`BASH_ENV` 指向的脚本**不会**被 `bash -c` 读（对应 Windows 臂的
/// `-NoProfile`）——主体的命令词法不该被一个 profile 文件悄悄改写。
///
/// 做法：把 `BASH_ENV` 指向一个会落记号的脚本；先**正对照**（直接起一支 bash，确认脚本本身
/// 是会被读的——不读它就说明这个用例坏在别处），再经主体的手跑一条无害命令，断言记号没出现、
/// 而且那个键名也不在子进程环境里。
///
/// ⚠ 这一格改的是**测试进程自己的环境**（`set_var`）：串行跑时是确定的；并行跑时最坏是别的
/// 用例也看到这个变量，而它们不读它（读它的只有子 shell，而子 shell 的环境由本臂决定）。
#[cfg(target_os = "linux")]
#[test]
fn the_bash_env_profile_seam_is_removed() {
    let dir = scratch("bash-env");
    let script = dir.join("seam.sh");
    let marker = dir.join("sourced.txt");
    let _ = std::fs::remove_file(&marker);
    std::fs::write(&script, format!("touch '{}'\n", marker.display())).expect("写 profile 缝脚本");

    // 正对照：**直接**起一支 bash（不经过本臂）⇒ 脚本真的会被读、记号真的会出现。
    let control = std::process::Command::new("bash")
        .arg("-c")
        .arg("true")
        .env("BASH_ENV", &script)
        .status()
        .expect("起对照用的 bash");
    assert!(control.success(), "对照用的 bash 自己得跑成");
    assert!(
        marker.exists(),
        "正对照失败：BASH_ENV 指向的脚本没有被读——先查脚本本身，别把用例坏当成墙生效"
    );
    let _ = std::fs::remove_file(&marker);

    // 经过主体的手：同样的 BASH_ENV 在测试进程环境里，命令还是那条无害的。
    // ⚠ `set_var` 在 edition 2024 里是 `unsafe`（进程级共享状态）：这里串行使用，用完立刻摘掉。
    unsafe { std::env::set_var("BASH_ENV", &script) };
    let mut app = TerminalApp::new(full(vec![]), &test_root());
    let outcome = call(&mut app, command("true"));
    let listing = call(&mut app, command(cmds::LIST_ENV_NAMES));
    unsafe { std::env::remove_var("BASH_ENV") };

    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    assert!(
        !marker.exists(),
        "BASH_ENV 指向的脚本**不许**被子进程读——那是 profile 缝：{}",
        text_of(&outcome)
    );
    assert!(
        !text_of(&listing).contains("BASH_ENV"),
        "被摘掉的键名不该出现在子进程环境里：{}",
        text_of(&listing)
    );
}

/// **信号读数**：被信号杀死的进程如实报 `exit = signal <n>`——可读的事实不许写成 `unknown`。
#[cfg(target_os = "linux")]
#[test]
fn a_signal_death_is_read_as_signal_not_unknown() {
    let mut app = TerminalApp::new(full(vec![]), &test_root());
    let outcome = call(&mut app, command("kill -KILL $$"));

    // 跑起来了、跑完了（被自己的信号结束也是"跑完了"）：它是成功执行 + 一份读数。
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    let receipt = text_of(&outcome);
    assert!(
        receipt.contains("exit = signal 9"),
        "被 SIGKILL 杀死要如实报出信号号：{receipt}"
    );
}

/// **进程组击杀的证据**：超时后**孙进程也死**。
///
/// 命令在后台起一个子 shell（孙进程），它两秒后落记号、随后前台睡 30 秒；`timeout = 1`
/// 到点按**进程组**发 SIGKILL。若只杀了直接子进程，那个后台子 shell 会继续走、记号会在
/// 两秒时出现——因此"等 3 秒看记号不在"正是"整组被杀"的证据。
#[cfg(target_os = "linux")]
#[test]
fn a_timeout_kills_the_grandchildren_too() {
    let marker = scratch("grandchild").join("done.txt");
    let _ = std::fs::remove_file(&marker);
    let mut app = TerminalApp::new(full(vec![]), &test_root());

    let outcome = call(
        &mut app,
        command(&format!(
            "( sleep 2; touch '{}' ) & sleep 30",
            marker.display()
        ))
        .with("timeout", "1"),
    );
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(failure_of(&outcome), "timeout", "{}", text_of(&outcome));

    std::thread::sleep(Duration::from_secs(3));
    assert!(
        !marker.exists(),
        "被按组击杀的孙进程不该再往下走一步：{}",
        marker.display()
    );
}

/// **命令的寿命归命令，流的寿命不归它**（2026-10-11）。
///
/// 夹具见 [`cmds::leak_then_exit`]：shell 落一句记号之后立刻退出，而它拉起的孙进程攥着这条
/// 管道不放（真机事故同形）。四条判据缺一条都不算：
///
/// ① **有界**：动作在宽限窗口附近就返回，而不是等孙进程跑完（夹具要跑 30 秒）；
/// ② **如实**：回执说出这条流没关——它是**读数**，不是失败；
/// ③ **不丢**：到那一刻为止读到的字节照给（记号还在正文里）；
/// ④ **不冒充读数**（**本刀加的**）：没读完 ⇒ 总数是**不知道**，不是零。
///    **原约束**：这一档从前写 `0 bytes, truncated = false`——两个看起来确定的读数替一段不
///    确定说话。**新边界**：写 `unknown`（与 `holdings` 的『缺省不等于零』同一条判据）。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn output_still_held_after_the_command_exits_is_reported_and_bounded() {
    const MARKER: &str = "leak-fixture-mark";
    let mut app = TerminalApp::new(full(vec![]), &test_root());

    let started = Instant::now();
    let outcome = call(&mut app, command(&cmds::leak_then_exit(MARKER)));
    let elapsed = started.elapsed();

    let receipt = text_of(&outcome);
    // ① 有界。夹具的孙进程活 30 秒；窗口是秒级的，所以 15 秒这条线怎么切都不会误判。
    assert!(
        elapsed < Duration::from_secs(15),
        "动作的寿命不该由孙进程决定（实测 {elapsed:?}）：{receipt}"
    );
    // ② 如实：命令跑完了就是成功，"流没关"是读数不是失败分类。
    assert_eq!(outcome.state(), ResultState::Success, "{receipt}");
    assert!(receipt.contains("exit = 0"), "{receipt}");
    assert!(
        output_line(&receipt, "stdout").ends_with("closed = false"),
        "流在命令退出后仍未关闭，回执必须说出来：{receipt}"
    );
    // ③ 不丢：到窗口为止读到的正文一个字节都不该少。
    assert!(
        receipt.contains(MARKER),
        "到那一刻为止读到的正文不该丢：{receipt}"
    );
    // ④ 不冒充读数：没读完 ⇒ 总数未知，不写 0；"有没有超上限"同款。
    let stdout = output_line(&receipt, "stdout");
    assert!(
        stdout.starts_with(&format!("stdout = {UNKNOWN_READING} bytes")),
        "没读完的流不许报数字（零 ≠ 不知道）：{stdout}"
    );
    assert!(
        stdout.contains(&format!("truncated = {UNKNOWN_READING}")),
        "总数不知道，那一格也就不知道：{stdout}"
    );
}

/// **窗口到点时手里已经攒满头部窗口** ⇒ 必须说出"后面还有多少未知"。
///
/// 这一条是"首尾各半"带来的**新责任**（头部窗口小了一半）：不说出来，它就退化成**静默截断**
/// ——那正是本层禁的那一件。与上一条的区别只有一个：上一条手里只有几个字节，这一条已经攒满。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_full_head_window_with_an_open_stream_says_the_rest_is_unknown() {
    let head = head_limit(MAX_OUTPUT_BYTES);
    let mut app = TerminalApp::new(full(vec![]), &test_root());
    let outcome = call(&mut app, command(&cmds::flood_then_leak(head + 4000)));
    let receipt = text_of(&outcome);

    assert_eq!(outcome.state(), ResultState::Success, "{receipt}");
    let stdout = output_line(&receipt, "stdout");
    assert!(
        stdout.ends_with("closed = false"),
        "这条流在命令退出后仍未关闭：{receipt}"
    );
    assert!(
        stdout.starts_with(&format!("stdout = {UNKNOWN_READING} bytes")),
        "这一档的总数同样是**不知道**（我们只读到头部窗口就交回了）：{stdout}"
    );
    assert!(
        receipt.contains("此处截断"),
        "头部窗口满了而这一次没读完 ⇒ 必须说出来（静默截断违禁）：{receipt}"
    );
    assert!(
        receipt.contains("未知"),
        "要说清后面还有多少是**未知**，不是「就这些」：{receipt}"
    );
    assert!(
        receipt.contains(&"x".repeat(head)),
        "到那一刻为止读到的头部窗口一个字节都不许少：{receipt}"
    );
    assert!(
        !receipt.contains(&"x".repeat(head + 1)),
        "头部窗口就那么大：{receipt}"
    );
}

/// **负对照**：没有谁攥着管道的普通命令必须读成 `closed = true`。
///
/// 没有它，上面那条判据会"永远为真"——一条恒真的断言不是证据。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_plain_command_reads_as_a_closed_output() {
    let mut app = TerminalApp::new(full(vec![]), &test_root());
    let outcome = call(&mut app, command(cmds::PLAIN_ASCII));
    let receipt = text_of(&outcome);

    assert_eq!(outcome.state(), ResultState::Success, "{receipt}");
    for name in ["stdout", "stderr"] {
        assert!(
            output_line(&receipt, name).ends_with("closed = true"),
            "{name} 该读成 closed = true：{receipt}"
        );
    }
    assert!(
        !receipt.contains("closed = false"),
        "没泄漏就不许报泄漏：{receipt}"
    );
}

// ---------------------------------------------------------------------------
// `sandbox` 档：档位、能力面、越界、keep、真执行
// ---------------------------------------------------------------------------
//
// 这一组的前五条**离线**（不需要 bwrap：它们判的是本层自己的规则）；后四条**真跑**，
// 前提是这台机器上的 bwrap 真的能兑现保证集——`bwrap_ready()` 会真跑一次探针，
// 不可用就 `skip:` 并把原因打出来（`--nocapture` 可见），**不冒充通过**。
//
// ⚠ 判据从"随便哪一支后端可用"改成"**bwrap** 可用"（2026-10-23）：表里现在有两支，
// 后面那几条用例装的是 `BwrapBackend`（它们判的是 bwrap 的命名空间效果）。拿
// `choose()` 当闸门的话，一台 bwrap 起不来、identity 可用的机器会走进来跑一堆
// bwrap 用例——那不是"通过"，是把闸门装错了地方。

/// 这台机器上的 **bwrap** 真的可用吗（真跑探针，不是 `which bwrap`）。
#[cfg(target_os = "linux")]
fn bwrap_ready() -> bool {
    match celira_agent::sandbox_backend::SandboxBackend::probe(
        &celira_agent::sandbox_backend::BwrapBackend::new(),
    ) {
        Ok(reading) => {
            eprintln!("bwrap 探针：{}", reading.evidence);
            true
        }
        Err(unavailable) => {
            eprintln!(
                "skip: 本机 bwrap 不可用（{}）——这一组真执行用例不跑",
                unavailable.message()
            );
            false
        }
    }
}

/// 一支装好 bwrap 后端的 `sandbox` 声明（入口探针通过之后就是这个形状）。
#[cfg(target_os = "linux")]
fn sandboxed() -> TerminalAccess {
    TerminalAccess::new(TerminalTier::Sandbox, vec![])
        .with_sandbox_backend(Box::new(celira_agent::sandbox_backend::BwrapBackend::new()))
}

/// 一支装好 identity 后端的 `sandbox` 声明（不建命名空间那一支）。
#[cfg(target_os = "linux")]
fn identity_sandboxed(root: &Path) -> TerminalAccess {
    TerminalAccess::new(TerminalTier::Sandbox, vec![]).with_sandbox_backend(Box::new(
        celira_agent::sandbox_backend::IdentityBackend::new(root),
    ))
}

/// **多出来的一档只是那一个词**：`sandbox` 认得，`full` 一个字都没变，别的一律不猜。
#[test]
fn the_sandbox_tier_is_its_own_word_and_nothing_else() {
    assert_eq!(TerminalTier::parse("sandbox"), Some(TerminalTier::Sandbox));
    assert_eq!(TerminalTier::Sandbox.as_str(), "sandbox");
    assert_eq!(TerminalTier::parse("full"), Some(TerminalTier::Full));
    for unknown in [
        "",
        " ",
        "Sandbox",
        "SANDBOX",
        " sandbox",
        "sandbox ",
        "sand-box",
        "sandboxes",
        "none",
        "bwrap",
        "read-only",
    ] {
        assert_eq!(
            TerminalTier::parse(unknown),
            None,
            "`{unknown}` 不是一档，必须拒绝而不是就近取一档"
        );
    }
}

/// **能力面就是授权面**：`sandbox` 只有 `terminal_run`，`keep` 那一族与它的两个配套动词不在场；
/// `full` 三条照旧（这一刀没有动它）。
#[test]
fn the_sandbox_tier_has_one_verb_and_no_keep_family() {
    let root = test_root();
    let app = TerminalApp::new(TerminalAccess::new(TerminalTier::Sandbox, vec![]), &root);
    let caps = app.declared_capabilities();
    let ids: Vec<&str> = caps.items().iter().map(|each| each.id().as_str()).collect();
    assert_eq!(
        ids,
        vec![CAPABILITY_RUN],
        "沙箱档的能力面只有一条动词：{ids:?}"
    );
    // 效应声明必须与能力面同一句话（装配期自洽性检查的判据）。
    assert_eq!(app.declared_effects().len(), 1);
    assert_eq!(
        app.declared_effects()[0].action().as_str(),
        CAPABILITY_RUN,
        "沙箱档只声明那一条写"
    );
    // 不在能力面上的动词，按名字调过来也不接纳。
    for id in [CAPABILITY_HELD, CAPABILITY_STOP] {
        let admission = ActionPort::admit(
            &app,
            &agent(),
            &RunId::new("hand"),
            &Action::with_params(ActionId::new(id), ActionParams::none()),
        );
        assert!(
            matches!(admission, ActionAdmission::Refused { .. }),
            "`{id}` 在 sandbox 档不该被接纳"
        );
    }
    // `full`：三条动词、两条写效应，逐字未变。
    let full_app = TerminalApp::new(full(vec![]), &test_root());
    assert_eq!(full_app.declared_capabilities().items().len(), 3);
    assert_eq!(full_app.declared_effects().len(), 2);
}

/// **`keep` 在沙箱档明确拒绝**（`invalid-argument`），而且是在**执行之前**拒的。
#[test]
fn keep_is_refused_in_the_sandbox_tier_before_anything_runs() {
    let root = test_root();
    let mut app = TerminalApp::new(TerminalAccess::new(TerminalTier::Sandbox, vec![]), &root);
    let outcome = call(
        &mut app,
        command("touch keep-marker.txt").with("keep", "true"),
    );
    assert_eq!(failure_of(&outcome), "invalid-argument");
    assert!(
        !root.join("keep-marker.txt").exists(),
        "「不支持」要在执行之前说清：{}",
        text_of(&outcome)
    );
    assert!(
        !root.join("terminal").exists(),
        "沙箱档不该写那本「我起过的东西」的账"
    );
    // `keep=false` 不被这一道拒——它走的是执行那条路。
    let outcome = call(&mut app, command("true").with("keep", "false"));
    assert_ne!(
        failure_of(&outcome),
        "invalid-argument",
        "`keep=false` 是这一档唯一的行为，不该被拒：{}",
        text_of(&outcome)
    );
}

/// **没有后端就不跑**：`sandbox` 档没装上后端时每条命令都是 `spawn-failure`，
/// **绝不在宿主上裸跑顶替**（这条用例要一个文件记号来证明"什么都没执行"）。
#[test]
fn a_sandbox_without_a_backend_never_falls_back_to_the_host() {
    let root = test_root();
    let mut app = TerminalApp::new(TerminalAccess::new(TerminalTier::Sandbox, vec![]), &root);
    let outcome = call(&mut app, command("touch bare-host-marker.txt"));
    assert_eq!(
        failure_of(&outcome),
        "spawn-failure",
        "没有后端 ⇒ 如实说没跑起来：{}",
        text_of(&outcome)
    );
    assert!(
        !root.join("bare-host-marker.txt").exists(),
        "没有任何一条命令该落到宿主上"
    );
}

/// **越界的 `cwd` 是本地拒绝**：在根之外执行一条命令，一次也不许跑起来。
///
/// 同一支用例带**正对照**：根之内的子目录不被这道检查挡（挡对了也挡错了同样是错）。
#[cfg(target_os = "linux")]
#[test]
fn a_cwd_outside_the_environment_root_is_refused_in_the_sandbox_tier() {
    let root = test_root();
    let inside = root.join("sub");
    std::fs::create_dir_all(&inside).expect("建子目录");
    let outside = scratch("sandbox-outside");
    let escaped = outside.join("escaped.txt");
    let _ = std::fs::remove_file(&escaped);

    let mut app = TerminalApp::new(sandboxed(), &root);
    let outcome = call(
        &mut app,
        command(&format!("touch '{}'", escaped.display()))
            .with("cwd", outside.to_string_lossy().as_ref()),
    );
    assert_eq!(
        failure_of(&outcome),
        "invalid-argument",
        "越界的 cwd 必须本地拒绝：{}",
        text_of(&outcome)
    );
    assert!(!escaped.exists(), "拒绝要在执行之前");

    // 正对照：根之内的目录不该被这一道挡（它的结局是执行，不是 invalid-argument）。
    let outcome = call(
        &mut app,
        command("pwd").with("cwd", inside.to_string_lossy().as_ref()),
    );
    assert_ne!(
        failure_of(&outcome),
        "invalid-argument",
        "根之内的 cwd 不该被拒：{}",
        text_of(&outcome)
    );
}

/// **真执行：写得到自己的根，写不到外面**（系统只读 ＋ 环境根可写 ＋ `/tmp` 是沙箱自己的）。
#[cfg(target_os = "linux")]
#[test]
fn a_sandboxed_command_writes_its_own_root_and_nothing_else() {
    if !bwrap_ready() {
        return;
    }
    let root = test_root();
    let host_tmp_name = format!("celira-sbx-tmpfs-{}", std::process::id());
    let host_tmp = std::env::temp_dir().join(&host_tmp_name);
    let _ = std::fs::remove_file(&host_tmp);
    let escaped = scratch("sandbox-escape").join("escaped.txt");
    let _ = std::fs::remove_file(&escaped);

    let mut app = TerminalApp::new(sandboxed(), &root);
    let outcome = call(
        &mut app,
        command(&format!(
            "echo inside > inside.txt; echo wrote-inside; \
             touch /etc/.celira-sbx-escape 2>/dev/null && echo system-writable || echo system-read-only; \
             : > /tmp/{host_tmp_name} && echo tmp-writable; \
             touch '{escaped}' 2>/dev/null && echo outside-writable || echo outside-read-only",
            escaped = escaped.display()
        )),
    );
    let receipt = text_of(&outcome);
    assert_eq!(outcome.state(), ResultState::Success, "{receipt}");
    // 回执说清是哪只手执行的（审计那一格）。
    assert!(
        receipt.contains("sandbox = bwrap"),
        "沙箱回执要带上后端名字（`sandbox = bwrap`）：{receipt}"
    );
    // 环境根真的可写（`--bind <根> <根>` 生效）。
    assert_eq!(
        std::fs::read_to_string(root.join("inside.txt"))
            .expect("根里那个文件该在")
            .trim(),
        "inside"
    );
    // 系统只读：`/etc` 下写不进去。
    assert!(
        receipt.contains("system-read-only"),
        "`/etc` 必须写不进去（`--ro-bind / /`）：{receipt}"
    );
    // `/tmp` 是沙箱自己的 tmpfs：写进去的东西不落到宿主。
    assert!(
        receipt.contains("tmp-writable"),
        "`/tmp` 是可写的 tmpfs：{receipt}"
    );
    assert!(
        !host_tmp.exists(),
        "沙箱里的 `/tmp` 不是宿主那个 `/tmp`（写进去不该出现）"
    );
    // 根外面的普通路径同样写不进去。
    assert!(
        receipt.contains("outside-read-only"),
        "根外面的路径必须只读：{receipt}"
    );
    assert!(!escaped.exists(), "根外面一个字节都不该写出来");
}

/// **真执行：默认断网**（新的 netns 里只有回环、没有路由）。
#[cfg(target_os = "linux")]
#[test]
fn a_sandboxed_command_has_no_network() {
    if !bwrap_ready() {
        return;
    }
    let mut app = TerminalApp::new(sandboxed(), &test_root());
    let outcome = call(&mut app, command("cat /proc/net/dev; cat /proc/net/route"));
    let receipt = text_of(&outcome);
    assert_eq!(outcome.state(), ResultState::Success, "{receipt}");
    // `/proc/net/*` 是**这个 netns** 的那一份（宿主那一份在另一次挂载里）。
    assert!(receipt.contains("lo:"), "沙箱里至少看得见回环：{receipt}");
    assert!(
        !receipt.contains("eth0:"),
        "宿主的网卡不该出现在沙箱里（`--unshare-net` 没生效？）：{receipt}"
    );
    // 没有路由（默认路由那一行的 Destination 是 `00000000`）。
    assert!(
        !receipt.contains("00000000"),
        "沙箱里不该有任何路由：{receipt}"
    );
}

/// **真执行：调用方的 secret 不进子进程**（`CELIRA_TEST_SECRET`，看取值与键名两样）。
#[cfg(target_os = "linux")]
#[test]
fn the_caller_secret_never_reaches_a_sandboxed_child() {
    if !bwrap_ready() {
        return;
    }
    let secret = format!("s3cr3t-sandbox-{}", std::process::id());
    // ⚠ `set_var` 在 edition 2024 里是 `unsafe`（进程级共享状态）：这里串行使用，用完立刻摘掉。
    unsafe { std::env::set_var("CELIRA_TEST_SECRET", &secret) };
    let mut app = TerminalApp::new(sandboxed(), &test_root());
    let outcome = call(&mut app, command("env"));
    unsafe { std::env::remove_var("CELIRA_TEST_SECRET") };

    let receipt = text_of(&outcome);
    assert_eq!(outcome.state(), ResultState::Success, "{receipt}");
    assert!(
        !receipt.contains(&secret),
        "凭据的**取值**不许出现在子进程环境里：{receipt}"
    );
    assert!(
        !receipt.contains("CELIRA_TEST_SECRET"),
        "连键名都不该在子进程环境里：{receipt}"
    );
    let scrubbed: usize = receipt
        .lines()
        .find_map(|line| line.strip_prefix("scrubbed = "))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    assert!(
        scrubbed >= 1,
        "洗掉几个要如实报（这一次至少那个 secret）：{receipt}"
    );
}

/// **真执行：超时那一堵墙在沙箱里同样管用**（bwrap 自持一个进程组，按组杀整棵树）。
#[cfg(target_os = "linux")]
#[test]
fn a_sandboxed_timeout_kills_the_command_and_says_so() {
    if !bwrap_ready() {
        return;
    }
    let root = test_root();
    let late = root.join("late.txt");
    let _ = std::fs::remove_file(&late);
    let mut app = TerminalApp::new(sandboxed(), &root);
    let outcome = call(
        &mut app,
        command("sleep 3; touch late.txt").with("timeout", "1"),
    );
    let receipt = text_of(&outcome);
    assert_eq!(failure_of(&outcome), "timeout", "{receipt}");
    assert!(receipt.contains("killed = true"), "{receipt}");
    // 等过那条命令本可以跑完的时刻：击杀真的生效时它一个字都不会落下。
    std::thread::sleep(Duration::from_millis(3000));
    assert!(
        !late.exists(),
        "超时之后那条命令不该还有机会写文件：{receipt}"
    );
}

/// **入口**：`--terminal sandbox` 只有两条路——真的装上**某一支**后端（并打印 `backend =`），
/// 或者**非零退出**并带上一个**稳定词**。**没有第三条路**（不退回 `full`，也不在宿主上裸跑）。
///
/// 两个稳定词是不同的失败：`sandbox-unavailable`（这台机器没有能兑现的后端）与
/// `sandbox-identity-mismatch`（机器能、身份不对）。这一条钉的是"二选一"本身，不钉
/// 这台机器落在哪一边——同一份源码要能在装了 bwrap 的机器与没装 userns 的机器上都成立。
///
/// 这一条走的是**真的二进制**（`agent_run`），不是库里的某个函数：入口那一行话是操作者
/// 唯一能看见的东西，因此它必须被真进程钉住。
#[cfg(target_os = "linux")]
#[test]
fn the_entry_installs_a_backend_or_refuses_with_a_stable_word() {
    let root = std::env::temp_dir().join(format!("celira-sbx-entry-{}", std::process::id()));
    // 环境根真的存在（identity 的探针要往里写一个文件；这也是真实装机的形状）。
    std::fs::create_dir_all(&root).expect("建环境根");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent_run"))
        .args([
            "run",
            "--root",
            root.to_string_lossy().as_ref(),
            "--agent",
            "agent-t36",
            "--materials",
            std::env::temp_dir().to_string_lossy().as_ref(),
            "--run",
            "sbx",
            "--max-turns",
            "1",
            "--terminal",
            "sandbox",
        ])
        .output()
        .expect("起得来 agent_run");
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let installed = stderr.contains("backend = ");
    let refused =
        stderr.contains("sandbox-unavailable") || stderr.contains("sandbox-identity-mismatch");
    assert!(
        installed ^ refused,
        "入口必须二选一：装上一支后端（打印 `backend =`），或者带稳定词拒绝；\
         不许既不装、也不说，更不许两条同时出现：{stderr}"
    );
    if refused {
        assert!(
            !output.status.success(),
            "拒绝必须非零退出（不能假装跑起来了）：{stderr}"
        );
    }
    // 装上了后端 ⇒ 那支后端必须真的过了探针（打印的是它的保证集读数）。
    if installed {
        assert!(
            stderr.contains("network=") && stderr.contains("files="),
            "装上了后端就要把保证集读出来（审计那一格）：{stderr}"
        );
    }
}

// ---------------------------------------------------------------------------
// `sandbox` 档的第二支后端：identity（uid ＋ seccomp，不建命名空间）
// ---------------------------------------------------------------------------
//
// 这一组**不依赖 bwrap**：它存在的理由正是那些"bwrap 起不来"的机器（本机就是：
// Ubuntu 24.04 默认 AppArmor 下 bwrap 在 `RTM_NEWADDR` 上被拒）。判据全部是**真跑**：
// 子进程写自己的根、写不了 `/etc`、连不出网、看不见调用方的 fd。没有一条是"看代码推断"。

/// 造一个**没有 `CLOEXEC`** 的 fd（`dup(2)`）。
///
/// 为什么不能拿 `File::open` 造夹具：std 在 Linux 上给每个新 fd 都打 `O_CLOEXEC`，
/// 那种 fd 本来就不会跨 `execve`——用它测"继承的句柄"是一条永远会通过的假用例。
/// `dup(2)` 交出来的 fd **不带** 那个标记，正是 `close_range` 要收的那一种。
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn dup_inheritable(fd: i32) -> Option<i32> {
    const SYS_DUP: i64 = 32;
    let returned: i64;
    // 安全：`dup` 只读一个整数、不改内存。
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") SYS_DUP => returned,
            in("rdi") fd as u64,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    (returned > 0).then_some(returned as i32)
}

/// aarch64 那一份（`__NR_dup` = 23）。
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn dup_inheritable(fd: i32) -> Option<i32> {
    const SYS_DUP: i64 = 23;
    let mut value = fd as i64;
    // 安全：`dup` 只读一个整数、不改内存。
    unsafe {
        core::arch::asm!(
            "svc 0",
            in("x8") SYS_DUP,
            inout("x0") value,
            options(nostack),
        );
    }
    (value > 0).then_some(value as i32)
}

/// 别的架构：没有夹具 ⇒ 这条用例如实跳过（并在 `--nocapture` 里说清原因）。
#[cfg(all(
    target_os = "linux",
    not(any(target_arch = "x86_64", target_arch = "aarch64"))
))]
fn dup_inheritable(_fd: i32) -> Option<i32> {
    None
}

/// 连 `dup(2)` 几次，得到一串**高位**的、不带 `CLOEXEC` 的 fd。
///
/// ⚠ 为什么不能只 dup 一次：最低空闲号就是 3，而子进程里 bash 读 `/proc/self/fd` 时
/// **也会拿到 3**——"泄漏的 3"和"读目录的 3"撞在同一个号上，那条用例就永远分不清
/// （2026-10-23 本机踩到：正对照与 identity 两端都印 `0 1 2 3`）。连开几个之后，
/// 泄漏的那几个落在 4 以上，判据才有区分度。
#[cfg(target_os = "linux")]
fn dup_chain(count: usize) -> Vec<std::fs::File> {
    use std::os::fd::FromRawFd;
    let mut fds = Vec::new();
    let mut current = 1;
    for _ in 0..count {
        let Some(next) = dup_inheritable(current) else {
            break;
        };
        fds.push(unsafe { <std::fs::File as FromRawFd>::from_raw_fd(next) });
        current = next;
    }
    fds
}

/// 回执里 `fds=…` 那一行的列表（`cd /proc/self/fd && printf fds=; echo *` 的输出）。
#[cfg(target_os = "linux")]
fn fd_listing(receipt: &str) -> String {
    receipt
        .lines()
        .find_map(|line| line.trim().strip_prefix("fds="))
        .unwrap_or("")
        .trim()
        .to_string()
}

/// **argv 与保证集**（离线）：identity 那一支不包 bwrap、自己声明 confinement、
/// 工作目录由编排层设；保证集照实降级（`network=none-ip` 不是 bwrap 的 `network=none`）。
#[cfg(target_os = "linux")]
#[test]
fn the_identity_backend_declares_its_own_argv_and_guarantees() {
    use celira_agent::sandbox_backend::{
        Confinement, IdentityBackend, SandboxBackend, SandboxRequest, SandboxRequestError,
    };

    let root = test_root();
    let backend = IdentityBackend::new(&root);
    assert_eq!(SandboxBackend::name(&backend), "identity");
    assert_eq!(backend.environment_root(), root.as_path());

    // argv：`<shell> -c <命令原文>`——不包 bwrap，环境根不进命令行。
    let request = SandboxRequest::new(&root, &root, "echo hi", "bash");
    let plan = backend.command(&request).expect("拼得出命令行");
    assert_eq!(plan.program(), "bash");
    assert_eq!(plan.args(), ["-c", "echo hi"]);
    assert_eq!(
        plan.confinement(),
        Confinement::SeccompNoIp,
        "identity 的断网与 fd 纪律全靠编排层在 execve 前装"
    );
    assert_eq!(plan.working_directory(), Some(root.as_path()));

    // 越界的 cwd / 不存在的根：与 bwrap **同一份**拒绝（不是第二套规则）。
    let outside = scratch("identity-outside");
    let escaped = SandboxRequest::new(&root, &outside, "true", "bash");
    assert_eq!(
        backend.command(&escaped).err(),
        Some(SandboxRequestError::CwdOutsideWorkspace)
    );
    let missing = root.join("does-not-exist");
    let gone = SandboxRequest::new(&missing, &missing, "true", "bash");
    assert_eq!(
        backend.command(&gone).err(),
        Some(SandboxRequestError::WorkspaceUnavailable)
    );

    // 保证集：**说得出来的必须是它真兑现的**。identity 不是 netns、不是挂载隔离。
    let guarantees = SandboxBackend::guarantees(&backend);
    assert_eq!(guarantees.value("network"), Some("none-ip"));
    assert_ne!(
        guarantees.value("network"),
        Some("none"),
        "`none` 是 bwrap 的 netns 那一格：identity 只是拒 IP 套接字（AF_UNIX 照旧通）"
    );
    assert_eq!(
        guarantees.value("files"),
        Some("workspace-writable,landlock")
    );
    assert_eq!(guarantees.value("pid"), Some("none"));
    assert_eq!(guarantees.value("ipc"), Some("none"));
    assert_eq!(guarantees.value("uts"), Some("none"));
    assert_eq!(guarantees.value("resources"), Some("none"));
    assert_eq!(guarantees.value("workspace"), Some("persistent"));
    assert_eq!(guarantees.value("没有这一格"), None);
}

/// **真跑一**：环境根里写得出、`/etc` 里写不出（系统面挡住）。
#[cfg(target_os = "linux")]
#[test]
fn an_identity_command_writes_its_root_and_never_etc() {
    let root = test_root();
    let escaped = format!("/etc/.celira-identity-escape-{}", std::process::id());
    let _ = std::fs::remove_file(&escaped);
    let mut app = TerminalApp::new(identity_sandboxed(&root), &root);
    let outcome = call(
        &mut app,
        command(
            "echo inside > inside.txt; \
             if touch /etc/.celira-identity-escape-PROBE 2>/dev/null; then echo etc-writable; \
             else echo etc-read-only; fi",
        ),
    );
    let receipt = text_of(&outcome);
    eprintln!("identity 写面回执：\n{receipt}");
    assert_eq!(outcome.state(), ResultState::Success, "{receipt}");
    assert!(
        receipt.contains("sandbox = identity"),
        "回执要带上后端名字：{receipt}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("inside.txt"))
            .expect("环境根里那个文件该在")
            .trim(),
        "inside",
        "identity 的可写面就是环境根：{receipt}"
    );
    assert!(
        receipt.contains("etc-read-only") && !receipt.contains("etc-writable"),
        "`/etc` 必须写不进去（DAC）：{receipt}"
    );
    let probe = "/etc/.celira-identity-escape-PROBE";
    assert!(!Path::new(probe).exists(), "`/etc` 下一个字节都不该写出来");
    assert!(!Path::new(&escaped).exists());
}

/// **真跑一（第二半）**：**环境根之外写不进去**——Landlock 把"写入"关进了根。
///
/// 这一条是 identity 与 DAC 的分水岭：测试进程与子进程**是同一个 uid**，父目录也归它所有，
/// 所以**没有 Landlock 时这条命令必然成功**（2026-10-23 真机：`touch` 成功、兄弟目录被造出来）。
/// 它证明的不是"系统只读"（那是 `/etc` 那一格），而是"造不出一个别人的家"。
#[cfg(target_os = "linux")]
#[test]
fn an_identity_command_cannot_write_outside_its_root() {
    let root = test_root();
    let parent = root.parent().expect("临时目录有父目录").to_path_buf();
    let escaped = parent.join(format!("celira-escape-{}", std::process::id()));
    let _ = std::fs::remove_file(&escaped);

    let mut app = TerminalApp::new(identity_sandboxed(&root), &root);
    let outcome = call(
        &mut app,
        command(&format!(
            "if touch {} 2>/dev/null; then echo escaped; else echo confined; fi",
            escaped.display()
        )),
    );
    let receipt = text_of(&outcome);
    eprintln!("identity 根外写回执：\n{receipt}");
    assert_eq!(outcome.state(), ResultState::Success, "{receipt}");
    assert!(
        receipt.contains("confined") && !receipt.contains("escaped"),
        "环境根之外必须写不进去（Landlock）：{receipt}"
    );
    assert!(!escaped.exists(), "根外那个文件一个字节都不该写出来");
}

/// **真跑一（第三半）**：**临时区在环境根里**——Landlock 连 `/tmp` 都不许写，所以编排层必须
/// 把 `TMPDIR` 指到根内；否则 `mktemp`、`git`、编译器这类命令一用临时文件就失败。
#[cfg(target_os = "linux")]
#[test]
fn a_sandboxed_command_gets_a_temp_dir_inside_its_root() {
    let root = test_root();
    let mut app = TerminalApp::new(identity_sandboxed(&root), &root);
    let outcome = call(&mut app, command("mktemp > where.txt && cat where.txt"));
    let receipt = text_of(&outcome);
    eprintln!("identity 临时区回执：\n{receipt}");
    assert_eq!(outcome.state(), ResultState::Success, "{receipt}");
    let written = std::fs::read_to_string(root.join("where.txt"))
        .expect("命令要把它自己的临时路径写下来")
        .trim()
        .to_string();
    let path = std::path::Path::new(&written);
    assert!(
        path.starts_with(std::fs::canonicalize(&root).unwrap()),
        "临时文件必须落在环境根里（`/tmp` 那一格是写不了的）：{written}"
    );
}

/// **真跑二**：断网——子进程连 `/dev/tcp` 必须失败，而且拒绝理由必须是 `Permission denied`
/// （seccomp 生效），不是"超时"或"连接被拒"（后者说明连接真的走出去了）。
#[cfg(target_os = "linux")]
#[test]
fn an_identity_command_cannot_open_an_ip_socket() {
    let root = test_root();
    let mut app = TerminalApp::new(identity_sandboxed(&root), &root);
    let outcome = call(
        &mut app,
        command("( exec 3<>/dev/tcp/1.1.1.1/443 ) 2>&1; echo probe-exit=$?"),
    );
    let receipt = text_of(&outcome);
    eprintln!("identity 断网回执：\n{receipt}");
    assert_eq!(outcome.state(), ResultState::Success, "{receipt}");
    // 本机在线时，拒绝的理由必须是内核的 `EACCES`（`Permission denied`）——那是过滤器
    // 真的生效的证据；离线的机器只会说 `Network is unreachable`，同样过不了网，但不是
    // 过滤器挡住了它（探针两面都接受，见 `IdentityBackend`）。**两种都不许是"连接成功"
    // 或者"连接被拒"**：后者说明包真的发出去了。
    assert!(
        receipt.contains("Permission denied") || receipt.contains("Network is unreachable"),
        "拒绝的理由必须是 EACCES（在线）或没有路由（离线）：{receipt}"
    );
    assert!(
        !receipt.contains("Connection refused"),
        "`Connection refused` 说明连接真的走出去了（过滤器没生效）：{receipt}"
    );
    assert!(
        receipt.contains("probe-exit=1"),
        "那次连接必须失败（不是\"连上了但没说话\"）：{receipt}"
    );
    // 正对照：换一个不存在的族（AF_UNIX 路径）不该被这道过滤器挡——断网不是"什么都不能建"。
    let outcome = call(
        &mut app,
        command("( exec 4<>/dev/tcp/1.1.1.1/443 ) 2>&1; echo again=$?"),
    );
    let receipt = text_of(&outcome);
    assert!(
        receipt.contains("Permission denied"),
        "同一支过滤器必须在第二次调用里照样生效：{receipt}"
    );
}

/// **真跑三**：继承的 fd 不泄漏。
///
/// 夹具是**真的**：先在测试进程里 `dup(1)` 造一个不带 `CLOEXEC` 的 fd，然后用**同一份**
/// 命令在 `full` 档里跑一次——它必须看得见那个号（正对照）；再在 identity 档里跑一次，
/// 它必须看不见（`close_range` 生效）。没有正对照的话，"看不见"可能只是夹具没造出来。
#[cfg(target_os = "linux")]
#[test]
fn an_identity_child_inherits_no_caller_fd() {
    // 连开五个：第一个一定是 3，而 3 恰好是子进程读 `/proc/self/fd` 时自己拿到的号——
    // 它的存在**不区分**"泄漏"与"读目录"。因此判据只看 3 之后的那些号（4/5/6/7）。
    let held = dup_chain(5);
    let leaked: Vec<String> = held
        .iter()
        .map(std::os::fd::AsRawFd::as_raw_fd)
        .filter(|fd| *fd > 3)
        .map(|fd| fd.to_string())
        .collect();
    if leaked.is_empty() {
        eprintln!("skip: 本架构没有 `dup` 夹具——继承 fd 那一条不跑");
        return;
    }
    let root = test_root();
    let inspect = "cd /proc/self/fd && printf fds=; echo *";

    // 正对照（`full` 档）：这几个 fd 必须出现在子进程的 fd 表里。
    let mut host = TerminalApp::new(full(vec![]), &root);
    let host_receipt = text_of(&call(&mut host, command(inspect)));
    let host_fds = fd_listing(&host_receipt);
    for fd in &leaked {
        assert!(
            host_fds.split_whitespace().any(|token| token == fd),
            "夹具必须真的会泄漏（`full` 档应当看得见 fd {fd}）：{host_receipt}"
        );
    }

    // identity：同一个命令，调用方的 fd 必须已经不在了（0/1/2 ＋ 读目录那一格之外没有）。
    let mut app = TerminalApp::new(identity_sandboxed(&root), &root);
    let receipt = text_of(&call(&mut app, command(inspect)));
    let fds = fd_listing(&receipt);
    eprintln!("fd 对照（full 看见 {host_fds:?}；identity 看见 {fds:?}）");
    for fd in &leaked {
        assert!(
            !fds.split_whitespace().any(|token| token == fd),
            "调用方的 fd {fd} 不许出现在子进程里（`close_range` 没生效？）：{receipt}"
        );
    }
    assert!(
        fds.split_whitespace()
            .all(|token| token.parse::<u32>().map(|fd| fd <= 3).unwrap_or(false)),
        "子进程只该看见 0/1/2（＋读目录那一格）：{fds:?}（{receipt}）"
    );
}

/// **探针真跑**：identity 的探针给出四格读数，而且它自己不留垃圾。
#[cfg(target_os = "linux")]
#[test]
fn the_identity_probe_measures_instead_of_guessing() {
    use celira_agent::sandbox_backend::{IdentityBackend, SandboxBackend};
    let root = test_root();
    let reading = IdentityBackend::new(&root)
        .probe()
        .expect("本机 identity 必须可用（真跑）");
    eprintln!("identity 探针：{}", reading.evidence);
    assert_eq!(reading.backend, "identity");
    for required in [
        "uid=",
        "workspace=writable",
        "system=read-only",
        "fds=0,1,2",
    ] {
        assert!(
            reading.evidence.contains(required),
            "探针证据里少了 `{required}`：{}",
            reading.evidence
        );
    }
    // 断网那一格：本机在线 ⇒ `denied`（过滤器生效）；离线的机器 ⇒ `unreachable`
    // （探针把两者都算过，见 `IdentityBackend` 的判据）。**不许**是 `reachable`。
    assert!(
        reading.evidence.contains("network=denied")
            || reading.evidence.contains("network=unreachable"),
        "断网那一格必须是 denied 或 unreachable：{}",
        reading.evidence
    );
    assert!(
        !reading.evidence.contains("network=reachable"),
        "连得上就不是这一档：{}",
        reading.evidence
    );
    // 探针写完就删：环境根里不许留下 `.celira-identity-probe-*`。
    let leftovers: Vec<String> = std::fs::read_dir(&root)
        .expect("读环境根")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.starts_with(".celira-identity-probe-"))
        .collect();
    assert!(leftovers.is_empty(), "探针留下了垃圾：{leftovers:?}");
}

/// **入口的身份判据**（离线可判的那一半）：环境根属于当前 uid 时通过；
/// 不属于时拒绝，且**给得出修复命令**（不 chown、不回退）。
#[cfg(target_os = "linux")]
#[test]
fn the_entry_identity_rule_refuses_a_foreign_root_with_a_fix() {
    use celira_agent::sandbox_backend::{IdentityBackend, check_entry_identity};
    let root = test_root();
    // 正例：这个目录是我们自己的（测试进程建的）。
    check_entry_identity(&root, &IdentityBackend::new(&root)).expect("自己的根必须通过");
    // 反例：拿一个**不归当前 uid** 的目录（`/etc` 的 owner 是 root）。
    // 判据是 owner 比较，因此这一条在非 root 进程里必然拒绝——不需要 chown，也不需要 root。
    let foreign = IdentityBackend::new("/etc");
    let failure = check_entry_identity(Path::new("/etc"), &foreign).expect_err("必须拒绝");
    assert_eq!(failure.as_str(), "sandbox-identity-mismatch");
    assert!(
        !failure.message().contains("sandbox-unavailable"),
        "身份不对不是\"没有后端\"：{}",
        failure.message()
    );
    assert!(
        failure.detail().contains("chown -R"),
        "拒绝必须给一条可执行的修复命令：{}",
        failure.detail()
    );
    // `full` 档不经过这条判据（它不承诺写边界）：这里的证据是"这条函数只被 sandbox 调用"，
    // 由两个 bin 的分支结构钉住（`TerminalTier::Full => Ok(Some(access))` 在调用之前返回）。
}

/// **缺省档**：不给 `--terminal` 时，Linux 上会试着装 `sandbox`；本机（bwrap 不可用、
/// identity 可用）必须落到 identity，而不是"没有手"、更不是 `full`。
#[cfg(target_os = "linux")]
#[test]
fn the_default_tier_picks_the_strongest_backend_that_really_works() {
    use celira_agent::sandbox_backend::{BwrapBackend, IdentityBackend, SandboxBackend};
    let root = test_root();
    let bwrap = BwrapBackend::new().probe().is_ok();
    let identity = IdentityBackend::new(&root).probe().is_ok();
    let chosen = celira_agent::sandbox_backend::choose(&root);
    match (bwrap, identity) {
        (true, _) => assert_eq!(
            chosen.expect("bwrap 可用就必须选它").name(),
            "bwrap",
            "能建命名空间的机器用更强的"
        ),
        (false, true) => assert_eq!(
            chosen
                .expect("bwrap 不行、identity 行 ⇒ 选 identity")
                .name(),
            "identity"
        ),
        (false, false) => assert!(
            chosen.is_err(),
            "两支都不可用 ⇒ 必须拒绝（没有\"那就别沙箱了\"这条路）"
        ),
    }
}

/// **exec 失败仍然是 `spawn-failure`**（不是"跑完、退出码 134"）。
///
/// 这一条钉的是 `crate::seccomp::CLOSE_RANGE_FLAGS` 那个偏差的**理由**：`close_range` 若用
/// 立刻关闭（flags=0），std 的错误管也会被关掉，子进程在 `execve` 失败时自己 abort——
/// 父进程拿到 `spawn() = Ok`，一次"程序不存在"就被读成一次"跑完了"。用 `CLOEXEC`（4）
/// 之后，同一条路径仍是 `spawn-failure`。这一条是那条注释的**可复核证据**。
#[cfg(target_os = "linux")]
#[test]
fn an_identity_shell_that_cannot_start_is_still_spawn_failure() {
    use celira_agent::sandbox_backend::IdentityBackend;
    let root = test_root();
    let access = TerminalAccess::new(TerminalTier::Sandbox, vec![])
        .with_sandbox_backend(Box::new(IdentityBackend::new(&root)))
        .with_shell("celira-no-such-shell-for-identity");
    let mut app = TerminalApp::new(access, &root);
    let outcome = call(&mut app, command("true"));
    let receipt = text_of(&outcome);
    assert_eq!(
        failure_of(&outcome),
        "spawn-failure",
        "引擎起不来必须是 spawn-failure，不是一次『跑完』的读数：{receipt}"
    );
    assert!(
        !receipt.contains("134"),
        "更不许变成 abort 的读数（close_range 旗标用错了？）：{receipt}"
    );
}
