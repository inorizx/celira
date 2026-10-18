//! 安家 · **本机用户级环境变量**的读取（`scripts/home/userenv.js` 的直译）。
//!
//! 用途只有一个：`DEEPSEEK_API_KEY` 这条**老路 / 手工路**的取值来源
//! （模型那一半；后端凭据那一半走按身份分条的本地存储，见 `home.rs`）。
//!
//! - 非 Windows：`process.env`；
//! - Windows：**先问** `HKCU\Environment`（`[Environment]::GetEnvironmentVariable`，
//!   与 `userenv.js` 同一门路——那个值住在注册表、不一定在这个进程的环境里，
//!   而壳多半是被 Explorer 拉起来的，继承的可能是登录时那份快照）；
//!   PowerShell 起不来（环境问题，不是「变量不存在」）⇒ 退到 `process.env` 再看一眼。
//!
//! 纪律（与 `userenv.js` 的注释逐条对齐）：**只按变量名读**、值只回内存；
//! 本模块不打印任何取值、不落任何文件。取值只经环境注入子进程。

use std::process::Command;

/// 读一个用户级环境变量的值；没有/空 ⇒ `None`（空值按「没配」处理）。
pub fn read_user_env(name: &str) -> Option<String> {
    if !cfg!(windows) {
        return std::env::var(name).ok().filter(|value| !value.is_empty());
    }
    read_user_env_windows(name)
}

/// Windows 支：HKCU 优先（PowerShell），失败退进程环境。
///
/// ⚠ 本机是 Linux，这条支**未经真机验证**（如实登记在报告里）；命令形状与
/// `userenv.js` 逐字对齐，值不进 argv（只传变量名 `CELIRA_ENV_NAME`）。
fn read_user_env_windows(name: &str) -> Option<String> {
    const READ_SCRIPT: &str = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; \
        $v=[Environment]::GetEnvironmentVariable($env:CELIRA_ENV_NAME,\"User\"); \
        if ($v) { [Console]::Out.Write($v) }";
    let mut probe = Command::new("powershell.exe");
    probe
        .args(["-NoProfile", "-NonInteractive", "-Command", READ_SCRIPT])
        .env("CELIRA_ENV_NAME", name);
    // **不给它控制台窗口**（2026-10-08）：壳没有控制台，而 PowerShell 是控制台程序
    // ⇒ 少了这一行就是"每次读一次用户环境闪一下黑框"（开机那一步就会走这里）。
    crate::console_window::suppress(&mut probe);
    let output = probe.output();
    match output {
        Ok(output) if output.status.success() => {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if value.is_empty() {
                None
            } else {
                Some(value)
            }
        }
        // PowerShell 起不来是环境问题不是「没有这个变量」：退到进程环境里再看一眼。
        _ => std::env::var(name).ok().filter(|value| !value.is_empty()),
    }
}
