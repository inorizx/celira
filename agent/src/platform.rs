//! 平台缝：**一份知识，两个消费者**（主体的手 与 MCP 的 stdio 通道）。
//!
//! 两个消费者各自需要一支「平台臂」——在真正的进程上执行东西，而两个平台的做法不同。
//! 这份知识（**默认 shell 名、怎么拼命令行（含启动卫生）、怎么击杀整棵进程树、凭据怎么洗**）
//! 从这一刀起**只写一份**：此前 terminal 与 MCP 各留一份（同一堵凭据墙有两份实现、
//! `taskkill` 有两份实现），两份实现迟早对不上。
//!
//! ## 形状：契约唯一、实现多份（与第五刀同一个形状）
//!
//! - **契约**是消费者自己的墙与词表（terminal 的五堵墙、MCP 的四堵墙）——主体在任何机器上
//!   看到的承诺必须一样，因此**只写一份**，在各自平台无关的编排层里，本模块碰不到它们；
//! - **臂**是本模块按平台实现的那几件。加一支臂 ＝ 在这里填那几件 ＋ 把两个消费者
//!   `available()` 的编译期条件加上它——**墙一个字不用动**（加臂不是加墙）。
//!
//! 今天两支臂：Windows（terminal＝PowerShell、MCP＝`cmd.exe`）与 Linux（两者都＝bash）。
//! **默认 shell 的选择是消费者的事**（terminal 选 PowerShell 的理由是主体的命令词法；
//! MCP 选 `cmd.exe` 的理由是"声明是宿主写的命令行"——见 `mcp/stdio.rs` 的模块文档），
//! 本模块只负责按名字把它们拼对。
//!
//! ## 没有臂的平台：fail-closed
//!
//! 没有臂的平台上两个入口都会**拒绝开启**（`TerminalTier::available` / `stdio_available`），
//! 本模块也不留近似实现。唯一的例外形状是 [`kill_tree`]——它在没有臂的平台上恒为假，
//! 因为编排层的收尾路径必须能编译（那台机器上它永远不会被真的用到）。
//!
//! ## 凭据洗（TASK_36 的假墙教训，写在这里免得日后重犯）
//!
//! [`child_environment`] 交出的那一份**必须先清空再交**——只叠加等于没洗，子进程照旧继承
//! 父进程的全部环境，这堵墙就成了装饰（TASK_36 的离实用例抓到过）。交出去这一步用
//! [`apply_environment`] 完成：**一条规矩、两个消费者共用**，不许各写一遍。
//!
//! 另一条既有语义：`USERPROFILE` 这类**由系统管理**的变量被洗掉之后，外壳会**重新造一个**
//! ——那不属于"凭据不继承"的语义；凭据类变量不会（外壳不会自己发明它们）。

use std::ffi::OsString;
use std::process::Command;

/// 系统自有前缀：这一族环境变量一律洗掉（家里那支凭据的家门）。
const CREDENTIAL_PREFIX: &str = "CELIRA_";

/// 这个键名属不属于凭据家族（系统自有前缀，或宿主点名的那几个）。
fn is_credential(name: &str, declared: &[String]) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with(CREDENTIAL_PREFIX)
        || declared
            .iter()
            .any(|each| each.eq_ignore_ascii_case(name) || each.eq_ignore_ascii_case(&upper))
}

/// 洗过的子进程环境：`(要交给子进程的那些键值, 洗掉几个, 洗不掉的键名)`。
///
/// **洗不掉的如实报**是拿**真正要交出去的那一份**再查一遍得出的（不是"我们以为干净"）：
/// 正常情况下它为空；真出现就点名——只报键名，**取值一个字节都不落**。
///
/// 拿到这份东西之后**不许直接 `envs()`**：先清空再交，见 [`apply_environment`]。
pub fn child_environment(declared: &[String]) -> (Vec<(OsString, OsString)>, usize, Vec<String>) {
    let mut kept: Vec<(OsString, OsString)> = Vec::new();
    let mut scrubbed = 0usize;
    for (key, value) in std::env::vars_os() {
        let name = key.to_string_lossy().to_string();
        if is_credential(&name, declared) {
            scrubbed += 1;
        } else {
            kept.push((key, value));
        }
    }
    let unscrubbed: Vec<String> = kept
        .iter()
        .map(|(key, _)| key.to_string_lossy().to_string())
        .filter(|name| is_credential(name, declared))
        .collect();
    (kept, scrubbed, unscrubbed)
}

/// 把洗过的那一份**先清空再交**给一条命令行（两个消费者共用的一条规矩）。
///
/// 不许省、也不许换成只 `envs()`：只叠加是**假墙**——子进程照旧继承父进程的全部环境。
pub fn apply_environment(builder: &mut Command, environment: Vec<(OsString, OsString)>) {
    builder.env_clear().envs(environment);
}

// ---------------------------------------------------------------------------
// 平台臂（每份构建里编进本平台那一支；有没有臂由两个消费者各自 `available()` 回答）
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod arm {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;

    /// **起子进程时不许给它一个控制台窗口**（Win32 的 `CREATE_NO_WINDOW`）。
    ///
    /// ## 为什么（2026-10-08 真机读数）
    ///
    /// 宿主是被**壳**拉起来的（壳是 GUI 程序，`windows_subsystem = "windows"`）⇒ 它自己
    /// **没有控制台**；而它与它拉起来的东西（terminal 的 PowerShell、MCP 的 cmd、`taskkill`）
    /// 都是**控制台子系统**程序 ⇒ Windows 会给**每一个**分配一个新的控制台 = 屏幕上一闪一个
    /// 黑框。真机基线（修之前）：`conhost.exe` 的父进程**正是** `celira-im.exe` —— 打开 app
    /// 就多一个黑框，而主体每跑一条终端命令还会再弹一个。发布形态里这是硬伤。
    ///
    /// ## 判据与边界
    ///
    /// - **只动"有没有那个窗口"**，不动 stdio：需要输出的地方本来就自己重定向（terminal 两条
    ///   都 piped、MCP 两条 piped ＋ stderr 接空设备、`taskkill` 三条全 null）⇒ 从终端**手工**
    ///   跑这些程序时该有的输出一条不少（手工跑时父进程自己有控制台，这个标志只影响
    ///   "要不要**新开**一个"）。
    /// - 它**不是**"藏起来跑"：命令与它的输出照旧归主体，`terminal` 那一面的账一条不变。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// **臂的一件**：给一个刚拼好的命令行装上"别开控制台窗口"。
    ///
    /// 为什么收成一个函数而不是三处各写一遍：`creation_flags` 是**替换**语义（默认 0），
    /// 三处各写一遍就是三份 `0x0800_0000` —— 抄错一个字节是**静默**的（窗口照弹，谁也不响）。
    fn without_console_window(builder: &mut Command) -> &mut Command {
        builder.creation_flags(CREATE_NO_WINDOW)
    }

    /// terminal 的默认 shell 名（命令词法就是它的）。
    pub const TERMINAL_SHELL: &str = "powershell.exe";

    /// MCP 的默认 shell 名（**声明是宿主写的命令行**，词法随这台机器自带的解释器；
    /// `npx @scope/pkg` 那种写法在 PowerShell 里会被当成展开运算符——见 `mcp/stdio.rs`）。
    pub const MCP_SHELL: &str = "cmd.exe";

    /// **臂的一件**：怎么拼 terminal 的命令行（PowerShell 的启动卫生；逐字搬移自第五刀）。
    ///
    /// `stdin`、两条流、环境与工作目录由平台无关的编排统一装（墙只写一份）。Windows 没有
    /// `bash -c` 那种 profile 文件缝（`-NoProfile` 已堵住），环境原样交出去。
    pub fn terminal_command(
        shell: &str,
        command: &str,
        _environment: &mut Vec<(OsString, OsString)>,
    ) -> Command {
        let mut builder = Command::new(shell);
        builder
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-Command")
            .arg(command);
        // 主体每跑一条命令都会经过这里 ⇒ 少了这一行就是"每条命令闪一个黑框"。
        without_console_window(&mut builder);
        builder
    }

    /// **臂的一件**：怎么拼 MCP 的命令行（`cmd.exe /C <命令原样>`；逐字搬移自 TASK_42）。
    pub fn mcp_command(
        shell: &str,
        command: &str,
        _environment: &mut Vec<(OsString, OsString)>,
    ) -> Command {
        let mut builder = Command::new(shell);
        builder.arg("/C").arg(command);
        // MCP server 是**长命**子进程 ⇒ 少了这一行就是一个一直挂着的黑框。
        without_console_window(&mut builder);
        builder
    }

    /// **臂的一件**：尽力击杀**整棵进程树**；返回"击杀命令报成功了吗"（逐字搬移）。
    ///
    /// 它报成功不等于树真的没了（因此调用方还要等我们那个直接子进程消失，并把两个条件合起来
    /// 才敢说"命令被中断了"）——这正是"效果不明"那一态存在的理由。
    pub fn kill_tree(pid: u32) -> bool {
        let mut command = Command::new("taskkill");
        command
            .arg("/T")
            .arg("/F")
            .arg("/PID")
            .arg(pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // 它是一次**短命**子进程：不装这个标志就是"中断一条命令闪一下黑框"。
        without_console_window(&mut command);
        command
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
}

#[cfg(target_os = "linux")]
mod arm {
    use super::*;
    use std::os::unix::process::CommandExt;

    /// terminal 的默认 shell 名（命令词法就是它的；与「机器处境」那一行说**同一个词**）。
    pub const TERMINAL_SHELL: &str = "bash";

    /// MCP 的默认 shell 名（同一台机器自带的解释器；声明是宿主写的命令行，词法随它）。
    pub const MCP_SHELL: &str = "bash";

    /// `bash -c` 的「profile 缝」键名：非交互 shell 会去读它们指向的文件。
    const PROFILE_SEAM_KEYS: [&str; 2] = ["BASH_ENV", "ENV"];

    /// **臂的一件**：怎么拼一条「交给 bash 原样执行」的命令行（含本臂的启动卫生）。
    ///
    /// 命令行是 `bash -c <命令原样>`；`stdin`、两条流、环境与工作目录由平台无关的编排统一装
    /// （墙只写一份）。另外**摘掉 `BASH_ENV` 与 `ENV`**——它们是 `bash -c` 的「profile 缝」
    /// （非交互 shell 会去读它们指向的文件），对应 Windows 臂的 `-NoProfile`：命令词法不该被
    /// 一个 profile 文件悄悄改写。它们不是凭据，不占洗凭据的读数（`-NoProfile` 同样不进读数）。
    ///
    /// 不加 `--noprofile` / `--norc`：那两个旗标只管**登录/交互** shell 的启动文件，对 `-c`
    /// 这种非交互、非登录的调用**没有作用**——加了就是"看着安全其实无意义"。
    fn bash_command(
        shell: &str,
        command: &str,
        environment: &mut Vec<(OsString, OsString)>,
    ) -> Command {
        environment.retain(|(key, _)| !is_profile_seam(key));
        let mut builder = Command::new(shell);
        builder.arg("-c").arg(command);
        // 子进程**自持一个进程组**（组长就是它自己）：超时击杀按组发、整棵树一起收。
        builder.process_group(0);
        builder
    }

    /// **臂的一件**：怎么拼 terminal 的命令行（bash 的形状）。
    pub fn terminal_command(
        shell: &str,
        command: &str,
        environment: &mut Vec<(OsString, OsString)>,
    ) -> Command {
        bash_command(shell, command, environment)
    }

    /// **臂的一件**：怎么拼 MCP 的命令行（同一支 bash 臂——与 terminal 同款：
    /// `bash -c`、自持进程组、摘 profile 缝；契约与 Windows 臂相同，不换近似行为）。
    pub fn mcp_command(
        shell: &str,
        command: &str,
        environment: &mut Vec<(OsString, OsString)>,
    ) -> Command {
        bash_command(shell, command, environment)
    }

    /// 这个键名是不是 `bash -c` 的 profile 缝（逐字节、大小写敏感——Unix 的环境变量名是
    /// 大小写敏感的，bash 也只认这两个拼写）。
    fn is_profile_seam(key: &OsString) -> bool {
        key.to_str()
            .is_some_and(|name| PROFILE_SEAM_KEYS.contains(&name))
    }

    /// **臂的一件**：尽力击杀**整棵进程树**；返回"击杀报成功了吗"。
    ///
    /// 与 Windows 臂同一个契约（两条件的一半，判定在编排层）：这里按**进程组**发 `SIGKILL`
    /// ——子进程自持一组（见 [`bash_command`] 的 `process_group(0)`），组长就是它自己，因此
    /// `-pid` 一下收掉整棵树。报成功仍不等于树真的没了（僵尸、竞态都还在），所以调用方还要
    /// 等直接子进程消失、把两个条件合起来才敢说"命令被中断了"。
    pub fn kill_tree(pid: u32) -> bool {
        let Ok(pid) = i32::try_from(pid) else {
            return false;
        };
        // 安全：无指针、只读系统状态；信号是常量。
        unsafe { sys::kill(-pid, SIGKILL) == 0 }
    }

    /// 一个**从 POSIX 起签名没变过**的系统调用（与 Windows 臂手写 `GetOEMCP` /
    /// `MultiByteToWideChar` 同一条先例：为两个固定签名拉一个 crate 不划算；`libc` 本就随
    /// std 链进二进制）。签名由我们自己保证——它只在 Linux 上编译，并由 `tests/terminal_app.rs`
    /// 的真执行用例钉住（超时用例真的按组杀掉整棵树）。
    mod sys {
        unsafe extern "C" {
            /// 发信号（`kill(2)`）。`pid` 为负 ⇒ 发给**那个进程组**（`-pgid`）。
            pub fn kill(pid: i32, signal: i32) -> i32;
        }
    }

    /// `SIGKILL` 的编号（POSIX 上恒为 9；它不可捕获、不可忽略）。
    const SIGKILL: i32 = 9;
}

#[cfg(not(any(windows, target_os = "linux")))]
mod arm {
    /// 平台缝：**本平台没有臂**（两个入口都会拒绝开启；编排层的收尾路径仍要能编译）。
    /// 万一被真的用到，如实说"没杀掉"，绝不换一个近似行为。
    pub fn kill_tree(_pid: u32) -> bool {
        false
    }
}

pub use arm::kill_tree;
#[cfg(any(windows, target_os = "linux"))]
pub use arm::{MCP_SHELL, TERMINAL_SHELL, mcp_command, terminal_command};
