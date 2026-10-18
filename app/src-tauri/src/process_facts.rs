//! **进程事实**——账本里那个 pid 到底是谁（杀之前的前置）。
//!
//! ## 为什么有这一片
//!
//! `scripts/home/proc.js` 的纪律第 3 条原文：**杀之前先验身份** ——
//! *"pid 只是个数字。落在一个不是我们的进程上（陈旧账本、数字撞车）时，杀它就是误杀无关进程。
//! 所以 `describeProcess(pid)` + `ownsPid(...)` 是**杀的前置**。"*
//!
//! 壳这一侧要用到它两处（都是 §8.2/8.3 的生命周期）：
//!
//! - **托盘菜单「退出（停止本机服务）」**：按账本停世界与住户 —— 停之前必须核对"这个 pid 还是
//!   我们那个程序吗"（账本会陈旧、pid 会被复用）；
//! - **住户在不在跑**：账本在不在 + 那个 pid 是不是我们那支宿主。
//!
//! ## 两条口径（照 `proc.js`，不发明）
//!
//! 1. **路径判据，不是名字判据**：同名 exe 在别的目录还有副本（真机见过）⇒ 比的是
//!    **可执行件的完整路径**（Windows 上大小写不敏感、去掉 `\\?\` 前缀），不是文件名。
//! 2. **行动只信账本，进程表只用来核对**：这一片**不扫进程表**（§8.7 撤掉的正是"认领别人的
//!    进程"那套）；它只回答"账本里写的这个 pid 现在是谁"。
//!
//! ⚠ 平台覆盖如实说：Windows（`OpenProcess` + `QueryFullProcessImageNameW`）与 Linux
//! （`/proc/<pid>/exe`）能给出**完整路径**；其余平台只知道"活着"（[`ProcessFact::AliveUnknownExe`]）
//! —— 那种情况下 [`Ownership::Unknown`]，调用方**不许据此动手**（fail-closed）。

use std::path::{Path, PathBuf};

/// 一个 pid 的现状（三态 + 一档"问不出来"）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessFact {
    /// 活着，且可执行件路径读得到（Windows / Linux）。
    Alive { exe: PathBuf },
    /// 活着，但路径读不到（平台不支持 / 权限不够）——**只知道它还活着**。
    AliveUnknownExe,
    /// 这个 pid 上什么都没有（进程不在 / 已经死了）。
    Gone,
    /// 问不出来（查询本身失败；与"不在"分开说）。
    Unknown(String),
}

/// 「这个 pid 是不是我们那个程序」的三态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ownership {
    /// 是：活着，且可执行件路径与期望一致。
    Ours { exe: PathBuf },
    /// 不是：活着，但那是**别的程序**（陈旧账本 / pid 撞车）——**绝不动它**。
    NotOurs { pid: u32, exe: PathBuf },
    /// 这个 pid 上什么都没有（进程不在）。
    Gone,
    /// 核对不了（路径读不到 / 平台不支持 / 查询失败）——**不许据此动手**（fail-closed）。
    Unknown(String),
}

/// 路径比较用的规范化：去掉 Windows 的 `\\?\` 前缀、统一分隔符、Windows 上折大小写。
fn normalized(path: &Path) -> String {
    let text = path.to_string_lossy().to_string();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text).to_string();
    let text = text.replace('/', "\\");
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

/// 期望的那个可执行件与进程实际的可执行件是不是同一个（**路径判据**）。
pub fn same_executable(expected: &Path, actual: &Path) -> bool {
    normalized(expected) == normalized(actual)
}

/// **账本里那个 pid 是谁**（杀/起的前置）。
pub fn ownership(pid: u32, expected_exe: &Path) -> Ownership {
    match describe(pid) {
        ProcessFact::Alive { exe } => {
            if same_executable(expected_exe, &exe) {
                Ownership::Ours { exe }
            } else {
                Ownership::NotOurs { pid, exe }
            }
        }
        ProcessFact::AliveUnknownExe => Ownership::Unknown(format!(
            "pid {pid} 活着，但这台平台上报不出它的可执行件路径 —— 核对不了，不动它"
        )),
        ProcessFact::Gone => Ownership::Gone,
        ProcessFact::Unknown(detail) => Ownership::Unknown(detail),
    }
}

impl Ownership {
    /// 一句人话（读数用；**不暴露凭据**）。
    pub fn describe(&self) -> String {
        match self {
            Self::Ours { exe } => format!("是我们的（{}）", exe.display()),
            Self::NotOurs { pid, exe } => format!(
                "不是我们的：pid {pid} 上是 {}（账本陈旧 / 数字撞车）—— 不动它",
                exe.display()
            ),
            Self::Gone => "账本里那个 pid 上什么都没有（进程不在）".to_string(),
            Self::Unknown(detail) => detail.clone(),
        }
    }
}

/// **停一个进程**（账本说它该在跑的那个）。
///
/// ⚠ **身份核对是这一句的前置，不是调用方的义务**：只有 [`Ownership::Ours`] 才动手 ——
/// `proc.js` 的纪律第 3 条（"杀之前先验身份……落在一个不是我们的进程上时，杀它就是误杀
/// 无关进程"）。核不上 ⇒ 一个字都不动，把原因交给调用方（那是要如实报出去的读数）。
pub fn terminate(pid: u32, expected_exe: &Path) -> Result<(), String> {
    match ownership(pid, expected_exe) {
        Ownership::Ours { .. } => platform::terminate(pid),
        Ownership::Gone => Err(format!("pid {pid} 上什么都没有（它已经停了）")),
        Ownership::NotOurs { pid, exe } => Err(format!(
            "pid {pid} 上是 {}，不是我们的 —— 没动它",
            exe.display()
        )),
        Ownership::Unknown(detail) => Err(detail),
    }
}

// ---------------------------------------------------------------------------
// 平台实现
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod platform {
    use super::ProcessFact;
    use std::path::PathBuf;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    pub fn describe(pid: u32) -> ProcessFact {
        if pid == 0 {
            return ProcessFact::Gone;
        }
        // 只要"问一句"的权限（PROCESS_QUERY_LIMITED_INFORMATION）：不要求 PROCESS_ALL_ACCESS，
        // 别人的进程也能问到路径 —— 这正是核对身份要的。
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        // windows-sys 0.52 的 HANDLE 是 `isize`：0 = 失败（不是 `Option`/指针）。
        if handle == 0 || handle == INVALID_HANDLE_VALUE {
            // 打不开 = 这个 pid 上没东西（ERROR_INVALID_PARAMETER）或权限不够。
            // 两者分开：权限不够时**不许**当成"不在"（那会让调用方去起第二个）。
            let error = std::io::Error::last_os_error();
            return match error.raw_os_error() {
                Some(87) => ProcessFact::Gone, // ERROR_INVALID_PARAMETER：没有这个 pid
                Some(5) => ProcessFact::Unknown(format!("pid {pid} 打不开（拒绝访问）")),
                _ => ProcessFact::Unknown(format!("pid {pid} 查询失败：{error}")),
            };
        }
        let mut buffer = vec![0u16; 32_768];
        let mut size = buffer.len() as u32;
        let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
        unsafe { CloseHandle(handle) };
        if ok == 0 {
            return ProcessFact::AliveUnknownExe;
        }
        let text = String::from_utf16_lossy(&buffer[..size as usize]);
        if text.trim().is_empty() {
            ProcessFact::AliveUnknownExe
        } else {
            ProcessFact::Alive {
                exe: PathBuf::from(text),
            }
        }
    }

    /// 停（`PROCESS_TERMINATE` + `TerminateProcess`）——调用方已经核对过身份。
    pub fn terminate(pid: u32) -> Result<(), String> {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, TerminateProcess, PROCESS_TERMINATE,
        };
        let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
        if handle == 0 || handle == INVALID_HANDLE_VALUE {
            return Err(format!(
                "停 pid {pid} 没成：打不开它（{}）",
                std::io::Error::last_os_error()
            ));
        }
        let ok = unsafe { TerminateProcess(handle, 0) };
        unsafe { CloseHandle(handle) };
        if ok == 0 {
            return Err(format!(
                "停 pid {pid} 没成：{}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::ProcessFact;
    use std::path::PathBuf;

    pub fn describe(pid: u32) -> ProcessFact {
        if pid == 0 {
            return ProcessFact::Gone;
        }
        let proc_dir = PathBuf::from(format!("/proc/{pid}"));
        if !proc_dir.is_dir() {
            return ProcessFact::Gone;
        }
        match std::fs::read_link(proc_dir.join("exe")) {
            Ok(exe) => ProcessFact::Alive { exe },
            // 目录在、链接读不到：多半是别人的进程（权限）——**活着，但路径未知**。
            Err(_) => ProcessFact::AliveUnknownExe,
        }
    }

    /// 停（`SIGTERM`）——调用方已经核对过身份。
    pub fn terminate(pid: u32) -> Result<(), String> {
        let result = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        if result == 0 {
            Ok(())
        } else {
            Err(format!(
                "停 pid {pid} 没成：{}",
                std::io::Error::last_os_error()
            ))
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use super::ProcessFact;

    /// 其余平台（macOS 等）：这一片**不猜** —— 报"问不出来"，调用方 fail-closed。
    /// 为什么不做 `kill(pid, 0)` 那种"只知活着"：知道了活着也核不出身份，
    /// 而这一片的**唯一用途**就是核对身份（杀之前的前置）。
    pub fn describe(pid: u32) -> ProcessFact {
        ProcessFact::Unknown(format!(
            "这个平台（{}）上壳还不会核对 pid {pid} 的身份 —— 不动它",
            std::env::consts::OS
        ))
    }

    /// 这个平台上**不会停进程** —— 如实拒绝（fail-closed：绝不"试试看"）。
    pub fn terminate(pid: u32) -> Result<(), String> {
        Err(format!(
            "这个平台（{}）上壳还不会停进程（pid {pid}）—— 没动它",
            std::env::consts::OS
        ))
    }
}

pub use platform::describe;

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_executable_is_a_path_judgement_not_a_name_judgement() {
        // 同一个路径、两种写法 ⇒ 同一个程序（Windows 上大小写不敏感、前缀可省）。
        assert!(same_executable(
            Path::new(r"D:\celira\target\release\agent_host.exe"),
            Path::new(r"D:\celira\target\release\agent_host.exe"),
        ));
        if cfg!(windows) {
            assert!(same_executable(
                Path::new(r"D:\celira\target\release\agent_host.exe"),
                Path::new(r"\\?\D:\CELIRA\target\release\Agent_Host.exe"),
            ));
            assert!(same_executable(
                Path::new(r"D:\celira\agent\target\release\agent_host.exe"),
                Path::new("D:/celira/agent/target/release/agent_host.exe"),
            ));
            // **同名不同目录 ⇒ 不是同一个**（proc.js 的原话：D 盘还有别的副本，同名不许牵连）。
            assert!(!same_executable(
                Path::new(r"D:\celira\target\release\agent_host.exe"),
                Path::new(r"D:\celira-tmp\agent_host.exe"),
            ));
        }
    }

    #[test]
    fn a_pid_that_cannot_exist_is_gone_not_unknown() {
        // 0 号 pid 永远不是我们的进程 ⇒ 如实说"不在"（不是"问不出来"）。
        assert_eq!(describe(0), ProcessFact::Gone);
    }

    #[test]
    fn our_own_process_is_recognized_as_ours() {
        // 拿**本进程**当靶子：它的可执行件路径就是 `current_exe()` —— 这是唯一能跨平台
        // 自证的一条（用例进程本身一定活着）。
        let me = std::process::id();
        let exe = std::env::current_exe().expect("取得到自己的可执行件路径");
        match ownership(me, &exe) {
            Ownership::Ours { .. } => {}
            #[cfg(not(any(windows, target_os = "linux")))]
            Ownership::Unknown(_) => {}
            other => panic!("本进程该被认成自己人：{other:?}"),
        }
        // 而"期望路径"换成一个不存在的程序 ⇒ **不是我们的**（或平台问不出来），绝不是 Ours。
        let other = ownership(me, Path::new(r"D:\definitely\not\us\agent_host.exe"));
        assert!(
            !matches!(other, Ownership::Ours { .. }),
            "换个期望路径就该核对不上：{other:?}"
        );
    }

    #[test]
    fn ownership_readouts_never_pretend() {
        assert_eq!(
            Ownership::Gone.describe(),
            "账本里那个 pid 上什么都没有（进程不在）"
        );
        let not_ours = Ownership::NotOurs {
            pid: 42,
            exe: PathBuf::from("/usr/bin/other"),
        };
        assert!(not_ours.describe().contains("不是我们的"));
        assert!(not_ours.describe().contains("不动"));
        let unknown = Ownership::Unknown("问不出来".to_string());
        assert_eq!(unknown.describe(), "问不出来");
    }
}
