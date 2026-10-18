//! **起子进程时不给它控制台窗口**（Windows；别的平台是空操作）。
//!
//! ## 为什么有它（2026-10-08 真机读数）
//!
//! 壳是 GUI 程序（`main.rs` 的 `windows_subsystem = "windows"`）⇒ **它自己没有控制台**；
//! 而它拉起来的每一个东西（`celira-im.exe`、`agent_host.exe`、两个 PowerShell 探针、
//! `agent_host_status.exe`、operator CLI 那几支）都是**控制台子系统**程序 ⇒ Windows 会给
//! **每一个**分配一个新的控制台 = 屏幕上一闪一个黑框。
//!
//! **基线（修之前，实测）**：`conhost.exe` 的父进程**正是** `celira-im.exe`
//! （`Get-CimInstance Win32_Process` 的父子链）—— 打开 app 就多一个黑框。
//! 发布形态里这是硬伤：用户第一次打开成品，看到的第一样东西不该是一个控制台。
//!
//! ## 判据与边界
//!
//! - **只动"有没有那个窗口"**，不动 stdio：这几处的输出本来就各有去处（世界与宿主的日志写
//!   文件、探针与 CLI 的返回走 `.output()` 的管道）⇒ **手工从终端跑它们时该有的输出一条不少**
//!   （手工跑时父进程自己有控制台，这个标志只影响"要不要**新开**一个"）。
//! - **它不是"藏起来跑"**：进程照旧是壳的孩子、照旧记账、照旧能被停（`local_services`）。
//!
//! ## 一处如实登记的重复
//!
//! 执行端那一边有一份同义的实现（`agent/src/platform.rs` 的 Windows 臂，只覆盖它自己那几个
//! 子进程）。两个 crate 之间没有可共享的依赖（壳 spawn 的是**产物**，不是 agent 的库），
//! 所以这条平台知识今天有**两份**：一份在这儿、一份在 `agent/src/platform.rs`。
//! 改一处要记得另一处（两处都写着这句话）。

use std::process::Command;

/// **给一个刚拼好的命令行装上"别开控制台窗口"**。
///
/// `creation_flags` 是**替换**语义（默认 0）⇒ 只装这一个标志是对的；收成一个函数是为了
/// 让那个常量**只有一份**（抄错一个字节是**静默**的：窗口照弹，谁也不响）。
#[cfg(windows)]
pub fn suppress(command: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    /// `CREATE_NO_WINDOW`（Win32 进程创建标志）。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW)
}

/// 非 Windows：**空操作**（没有"控制台窗口"这回事）。
#[cfg(not(windows))]
pub fn suppress(command: &mut Command) -> &mut Command {
    command
}
