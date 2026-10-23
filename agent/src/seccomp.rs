//! **子进程的系统调用过滤器**（Linux）：`terminal=sandbox` 的 identity 后端靠它断网。
//!
//! ## 为什么要有这一件（而不是去建一个网络命名空间）
//!
//! `docs/runtime/terminal/SANDBOX_IDENTITY_DECISIONS.md` §6 的裁决：这一档的边界装在**住户身份**
//! 上（一个非 root 的专用 uid），不装在每次活动的命名空间上。身份给了写边界（DAC），但**没有**
//! 给网络边界——不建 net 命名空间，也不按 uid 装防火墙规则（那条规则会把住户自己的模型调用
//! 一起堵死，因为调用与执行是同一个 uid）。
//!
//! 因此断网落在**子进程自己**身上：`execve` 之前装一个 seccomp 过滤器，让这一条命令（以及它
//! 派生的整棵进程树）**建不出 IP 套接字**。作用域是进程树，不是机器、不是 uid。
//!
//! ## 它拒什么、放过什么（别把它读成"断网"的等价物）
//!
//! | 系统调用 | 结果 | 为什么 |
//! |---|---|---|
//! | `socket(AF_INET, …)` / `socket(AF_INET6, …)` | `EACCES` | IP 出网只有这一扇门（TCP / UDP / raw 都要先建套接字） |
//! | `io_uring_setup` | `EACCES` | io_uring 的 `SOCKET` 操作**绕开 `socket(2)`**，不掐它上面的 `socket` 规则就是装饰 |
//! | 别的架构的 syscall 表 | `KILL_PROCESS` | 过滤器里写的是 x86_64 / aarch64 的号；换一个 ABI 进来一律杀掉（不做"大概相似"的映射） |
//! | 其它一切 | 放行 | 读文件、起子进程、unix 套接字都照常 |
//!
//! **不兑现**的三件事（照实写在这里，免得被读成"这一档上不了网"）：
//!
//! - **不是 netns**：`AF_UNIX` 照旧通——环境里指向 agent / 会话总线的套接字也照旧连得上。
//!   这一条与 bwrap 的 `--unshare-net` **不是**等价物；
//! - **不是防火墙**：它不认地址、不认端口、不认协议；拒绝的是"建 IP 套接字"这个动作本身；
//! - **拦不住已经建好的套接字**：调用方若把一条连着的 fd 交给子进程，过滤器不会替它关。
//!   那件事由 [`confine_child`] 的第二半（`close_range`）负责——**两半必须一起装**，
//!   装一半都不算兑现。
//!
//! ## 装法（2026-10-23 本机实测通过的语义）
//!
//! `prctl(PR_SET_NO_NEW_PRIVS, 1)` 然后 `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &程序)`。
//! 程序是 **classic BPF**（`sock_filter` 数组，`sock_fprog` 带走长度与地址）。
//!
//! ⚠ **实测序列里有一处写法在 classic BPF 里不存在**：`JEQ` 的 `jt` / `jf` 是**向前**的
//! 无符号偏移，跳不到前面的指令。规格里「第 9 条命中就返回 ERRNO」要求那个 `RET` 在**后面**，
//! 因此本文件的布局是那段语义的**前向等价体**（13 条，见 [`FILTER`] 的逐条注释）：
//! 每一条跳转的目标都写在行尾，`socket` 与 `AF_INET` 那一条在 `tests` 里被逐条钉住。
//! 语义与实测规格逐字相同——这一点不是推断：本机用同一份程序真跑过（见 §「实测」）。
//!
//! ## 为什么不用 libc / nix / seccompiler
//!
//! `tests/minimal_agent.rs` 的依赖守卫：这个 crate 的依赖是**封闭**的，且只服务模型适配层。
//! 因此 `prctl` / `close_range` 由 `core::arch::asm!` 直接发（与 `platform.rs` 手写 `kill(2)`
//! 同一条先例：两个固定签名不值得拉一个 crate）。`core::arch::asm!` 是标准库自带，
//! 不是一个新依赖。
//!
//! ## 实测（2026-10-23，Ubuntu 24.04 / 内核 6.8 / x86_64）
//!
//! 装好之后：`socket(AF_INET)` → `EACCES`，`socket(AF_INET6)` → `EACCES`，`socket(AF_UNIX)`
//! 正常，读文件正常，子进程正常，`getaddrinfo` 失败 `EAI_AGAIN`，`io_uring_setup` → `EACCES`，
//! 过滤器**跨 `execve` 继承**。`terminal_app` 的真执行用例与本文件的 unit 用例在每次
//! `cargo test` 里重新量一遍——这段读数不是一次性的。

/// 本文件真的实现了过滤器的架构。
///
/// ⚠ 它**不是**「Linux 就能装」：`AUDIT_ARCH` 与 syscall 号逐架构不同，写错一个号就是
/// 「过滤器装上了、但拒的是另一件事」——那比没装更坏。别的架构上 [`install`] 返回
/// [`SeccompUnavailable`]，identity 后端因此**如实不可用**，而不是装一个猜出来的表。
pub const SUPPORTED: bool = cfg!(any(target_arch = "x86_64", target_arch = "aarch64"));

/// `SECCOMP_RET_*` 的动作码（`include/uapi/linux/seccomp.h`，内核 ABI 常量）。
pub const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
pub const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
pub const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;

/// `EACCES`（"Permission denied"）。选它而不是 `ENOSYS`：调用方读到的是一句
/// **"内核不让"**，不是一个"这个系统调用不存在"的谎。
pub const EACCES: u32 = 13;

/// `close_range(2)` 的参数：关掉 **3 号及其之后**的全部 fd（0/1/2 是 stdio，不动）。
pub const CLOSE_RANGE_FIRST: u32 = 3;
/// `~0U`——一路关到这张表的最大值。
pub const CLOSE_RANGE_LAST: u32 = u32::MAX;
/// `flags = 4`（`CLOSE_RANGE_CLOEXEC`）：把这一段 fd **标记成 exec 时关闭**。
///
/// ⚠ **与规格的偏差，原因写在明处**（2026-10-23 真机实测）：规格写的是 `flags = 0`（立刻关闭）。
/// 立刻关闭会连 **std 自己的错误管**一起关掉（它也是 `fd > 2`），于是 `execve` 失败时
/// `std` 往那条管子写 errno 会断言失败——子进程 **abort**（`SIGABRT`，status 134），而
/// 父进程拿到的是 `spawn() = Ok`：一次"程序不存在"会被读成一次"跑完、退出码 134"。
/// `CLOEXEC` 给的是**同一条保证**（`execve` 之后一个继承 fd 都不剩——那正是被执行的
/// 程序能观察到的全部），而且错误管在 `execve` 之前照旧可写，分类因此还是 `spawn-failure`。
/// 两条路都在本机量过：flags=0 ⇒ abort/134；flags=4 ⇒ `spawn() = Err(ENOENT)`。
pub const CLOSE_RANGE_FLAGS: u32 = 4;

/// 过滤器装不上（或本架构没有实现）——**不是**"先跳过"。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeccompUnavailable {
    reason: String,
}

impl SeccompUnavailable {
    /// 带原因建一份。
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    /// 稳定分类标记（诊断与入口报错用）。
    pub fn as_str(&self) -> &'static str {
        "seccomp-unavailable"
    }

    /// 人话原因（哪一步、内核报了什么）。
    pub fn detail(&self) -> &str {
        &self.reason
    }
}

// ---------------------------------------------------------------------------
// 程序与安装（只在两种架构上存在）
// ---------------------------------------------------------------------------

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
mod arm {
    use super::{
        CLOSE_RANGE_FIRST, CLOSE_RANGE_LAST, EACCES, SECCOMP_RET_ALLOW, SECCOMP_RET_ERRNO,
        SECCOMP_RET_KILL_PROCESS, SeccompUnavailable,
    };
    use std::io;

    /// 一条 classic BPF 指令（`struct sock_filter`：`u16 code; u8 jt; u8 jf; u32 k`）。
    ///
    /// 布局是**内核 ABI**，不是我们的选择：`#[repr(C)]` ＋ 字段顺序逐字对应。
    /// 字段公开是为了让测试能逐条读它——那是这一件唯一能被离线判错的地方。
    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct SockFilter {
        pub code: u16,
        pub jt: u8,
        pub jf: u8,
        pub k: u32,
    }

    /// `struct sock_fprog`（内核要的是「长度 ＋ 指令数组的地址」）。
    #[repr(C)]
    struct SockFprog {
        len: u16,
        filter: *const SockFilter,
    }

    // BPF 指令编码（`include/uapi/linux/bpf_common.h`）：`LD|W|ABS` / `JMP|JEQ|K` / `RET|K`。
    const LD_W_ABS: u16 = 0x20;
    const JMP_JEQ_K: u16 = 0x15;
    const RET_K: u16 = 0x06;

    /// `prctl(2)` 的选项号。
    const PR_SET_SECCOMP: u64 = 22;
    const PR_SET_NO_NEW_PRIVS: u64 = 38;
    /// `SECCOMP_MODE_FILTER`。
    const SECCOMP_MODE_FILTER: u64 = 2;

    /// `struct seccomp_data` 的字段偏移：`nr` 0、`arch` 4、`args[0]` 16（低 32 位）。
    const OFFSET_NR: u32 = 0;
    const OFFSET_ARCH: u32 = 4;
    const OFFSET_ARG0: u32 = 16;

    #[cfg(target_arch = "x86_64")]
    mod numbers {
        /// `AUDIT_ARCH_X86_64`。
        pub const AUDIT_ARCH: u32 = 0xC000_003E;
        /// `SYS_socket`。
        pub const SYS_SOCKET: u32 = 41;
        /// `SYS_prctl`（`__NR_prctl`，x86_64）。
        pub const SYS_PRCTL: i64 = 157;
        /// `SYS_close_range`（x86_64）。
        pub const SYS_CLOSE_RANGE: i64 = 436;
    }

    #[cfg(target_arch = "aarch64")]
    mod numbers {
        /// `AUDIT_ARCH_AARCH64`。
        pub const AUDIT_ARCH: u32 = 0xC000_00B7;
        /// `SYS_socket`（`__NR_socket`，asm-generic 表）。
        pub const SYS_SOCKET: u32 = 198;
        /// `SYS_prctl`（asm-generic 表）。
        pub const SYS_PRCTL: i64 = 167;
        /// `SYS_close_range`（asm-generic 表）。
        pub const SYS_CLOSE_RANGE: i64 = 436;
    }

    use numbers::{AUDIT_ARCH, SYS_CLOSE_RANGE, SYS_PRCTL, SYS_SOCKET};

    /// `SYS_io_uring_setup`——两种架构上同一个号（asm-generic 表）。
    const SYS_IO_URING_SETUP: u32 = 425;

    /// `AF_INET` / `AF_INET6`（`socket(2)` 的第一个参数）。
    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 10;

    /// `prctl` 的退出约定：原样是内核的返回值（0 或 `-errno`）。
    const fn denied(errno: u32) -> u32 {
        SECCOMP_RET_ERRNO | errno
    }

    /// **过滤器本体**（13 条）。逐条注释里的「→ N」是**跳转目标的下标**（不是偏移）。
    ///
    /// 语义（与规格逐条对应）：
    ///
    /// 1. 架构不对 ⇒ 杀（不猜 syscall 号）；
    /// 2. `socket` 的 `AF_INET` / `AF_INET6` ⇒ `EACCES`；同一系统调用的别的族 ⇒ 放行；
    /// 3. `io_uring_setup` ⇒ `EACCES`；
    /// 4. 其它 ⇒ 放行。
    ///
    /// ⚠ 这里**没有** `LD` 读 `args[1]`（type）：判据是族，不是"哪一种套接字类型"。
    /// 数据报（UDP）同样出网，按 type 放行就是把一半的门留着。
    pub const FILTER: [SockFilter; 13] = [
        // 0：读 arch。
        SockFilter {
            code: LD_W_ABS,
            jt: 0,
            jf: 0,
            k: OFFSET_ARCH,
        },
        // 1：不是本架构 ⇒ → 12（杀）。相等则落到 2。
        SockFilter {
            code: JMP_JEQ_K,
            jt: 0,
            jf: 10,
            k: AUDIT_ARCH,
        },
        // 2：读系统调用号。
        SockFilter {
            code: LD_W_ABS,
            jt: 0,
            jf: 0,
            k: OFFSET_NR,
        },
        // 3：不是 socket ⇒ → 9（io_uring 检查）。相等则落到 4。
        SockFilter {
            code: JMP_JEQ_K,
            jt: 0,
            jf: 5,
            k: SYS_SOCKET,
        },
        // 4：读 args[0]（族）的低 32 位。
        SockFilter {
            code: LD_W_ABS,
            jt: 0,
            jf: 0,
            k: OFFSET_ARG0,
        },
        // 5：AF_INET ⇒ → 8（EACCES）。不等则落到 6。
        SockFilter {
            code: JMP_JEQ_K,
            jt: 2,
            jf: 0,
            k: AF_INET,
        },
        // 6：AF_INET6 ⇒ → 8（EACCES）。不等则落到 7。
        SockFilter {
            code: JMP_JEQ_K,
            jt: 1,
            jf: 0,
            k: AF_INET6,
        },
        // 7：别的族（AF_UNIX 等）⇒ 放行。
        SockFilter {
            code: RET_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_ALLOW,
        },
        // 8：IP 套接字 ⇒ EACCES。
        SockFilter {
            code: RET_K,
            jt: 0,
            jf: 0,
            k: denied(EACCES),
        },
        // 9：io_uring_setup ⇒ → 11（EACCES）。不等则落到 10。
        SockFilter {
            code: JMP_JEQ_K,
            jt: 1,
            jf: 0,
            k: SYS_IO_URING_SETUP,
        },
        // 10：其它系统调用 ⇒ 放行。
        SockFilter {
            code: RET_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_ALLOW,
        },
        // 11：io_uring_setup ⇒ EACCES。
        SockFilter {
            code: RET_K,
            jt: 0,
            jf: 0,
            k: denied(EACCES),
        },
        // 12：架构不对 ⇒ 杀整条进程（不是"拒绝这一个系统调用"）。
        SockFilter {
            code: RET_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_KILL_PROCESS,
        },
    ];

    /// 发一个**五参数**系统调用（x86_64：`rax` 号 ＋ `rdi`/`rsi`/`rdx`/`r10`/`r8`）。
    ///
    /// ⚠ 为什么不能只发三个参数（2026-10-23 真机踩到）：`prctl` 的 `PR_SET_NO_NEW_PRIVS`
    /// 要求 `arg2 == 1` **且 `arg3`/`arg4`/`arg5` 全为 0**，否则返回 `EINVAL`。只传三个
    /// 参数时 `r10`/`r8` 里是调用方留下的**垃圾**——内核看到的"第 4、5 个参数"不是 0，
    /// 于是过滤器装不上（本机会以 errno 22 失败）。未指定的寄存器不是零。
    #[cfg(target_arch = "x86_64")]
    unsafe fn syscall5(number: i64, a: u64, b: u64, c: u64, d: u64, e: u64) -> i64 {
        let result: i64;
        // 安全：无指针、无栈操作；`rcx` / `r11` 是 `syscall` 自己会改的两格。
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") number => result,
                in("rdi") a,
                in("rsi") b,
                in("rdx") c,
                in("r10") d,
                in("r8") e,
                lateout("rcx") _,
                lateout("r11") _,
                options(nostack),
            );
        }
        result
    }

    /// 发一个五参数系统调用（aarch64：`x8` 号 ＋ `x0`…`x4`）。
    #[cfg(target_arch = "aarch64")]
    unsafe fn syscall5(number: i64, a: u64, b: u64, c: u64, d: u64, e: u64) -> i64 {
        let mut value = a as i64;
        // 安全：无指针、无栈操作；`svc 0` 只改 `x0` 与条件码。
        unsafe {
            core::arch::asm!(
                "svc 0",
                in("x8") number,
                inout("x0") value,
                in("x1") b,
                in("x2") c,
                in("x3") d,
                in("x4") e,
                options(nostack),
            );
        }
        value
    }

    /// `prctl(option, arg2, arg3)`；后两个参数**显式是 0**（见 [`syscall5`] 的注释）。
    ///
    /// ⚠ 判据必须是**负返回值**，不是 `errno`：这是裸系统调用，它不会替我们写
    /// 用户态的 `errno`——读 `last_os_error()` 会读到上一条无关调用的陈值。
    unsafe fn prctl(option: u64, arg2: u64, arg3: u64) -> i64 {
        // 安全：五个整数参数，无指针（`arg3` 由调用方保证指向有效内存）。
        unsafe { syscall5(SYS_PRCTL, option, arg2, arg3, 0, 0) }
    }

    /// 设 `no_new_privs`（幂等；`pre_exec` 路径可调，不分配）。
    ///
    /// 它是 Landlock 的 `restrict_self` 的前置条件（内核要求），也是"这一棵树不许再借
    /// setuid 提权"的那一条。
    pub fn set_no_new_privs() -> io::Result<()> {
        // 安全：三个整数参数，无指针。
        let returned = unsafe { prctl(PR_SET_NO_NEW_PRIVS, 1, 0) };
        if returned != 0 {
            return Err(io::Error::from_raw_os_error(refusal(returned)));
        }
        Ok(())
    }

    /// 装过滤器（可分配、可报人话）——供探针与诊断用。
    ///
    /// ⚠ 它**不能**在 `pre_exec` 里用：那份路径只能调 [`confine_child`]（见那里）。
    pub fn install() -> Result<(), SeccompUnavailable> {
        install_raw().map_err(|errno| {
            SeccompUnavailable::new(format!(
                "内核拒绝装 seccomp 过滤器（errno {errno}）：{}",
                if errno == 1 {
                    "这一进程没有 CAP_SYS_ADMIN，先设 no-new-privs 也失败了"
                } else {
                    "见 prctl(PR_SET_SECCOMP)"
                }
            ))
        })
    }

    /// 装过滤器（**不分配**：`pre_exec` 里唯一被允许的那条路）。
    ///
    /// 返回值是**内核的负返回值取反**后的 errno；`Ok(())` 表示过滤器已经在身。
    fn install_raw() -> Result<(), i32> {
        // `sock_fprog` 与指令数组都在**栈/静态区**：这条路径里一个字节都不许分配
        // （fork 之后、exec 之前，另一条线程可能正握着分配器的锁）。
        let program = SockFprog {
            len: FILTER.len() as u16,
            filter: FILTER.as_ptr(),
        };
        // 安全：`program` 在本次调用期间一直有效；指针与长度逐字来自上面的静态数组。
        let no_new_privs = unsafe { prctl(PR_SET_NO_NEW_PRIVS, 1, 0) };
        if no_new_privs != 0 {
            return Err(refusal(no_new_privs));
        }
        // 安全：同上；`arg3` 指向一个 `sock_fprog`（内核只读它）。
        let installed = unsafe {
            prctl(
                PR_SET_SECCOMP,
                SECCOMP_MODE_FILTER,
                &program as *const SockFprog as u64,
            )
        };
        if installed != 0 {
            return Err(refusal(installed));
        }
        Ok(())
    }

    /// 内核返回值 ⇒ 正 errno（`0` 不该走到这里，兜底成 `EINVAL`）。
    fn refusal(returned: i64) -> i32 {
        if returned < 0 {
            (-returned) as i32
        } else {
            22 // EINVAL：内核说"不合法"，而不是"我不告诉你"
        }
    }

    /// 把 3 号及之后的全部 fd **标记成 `execve` 时关闭**（`close_range(3, ~0U, CLOEXEC)`）。
    ///
    /// 为什么必须做：seccomp 只拦"新建 IP 套接字"。调用方若已经把一条**连着的**套接字
    /// （或者任何别的句柄）交给了子进程，过滤器不会替它关——那条连接照样能收发。
    /// `execve` 只替你关带 `CLOEXEC` 的那些，而**不是所有 fd 都带**。
    ///
    /// 为什么是 `CLOEXEC` 而不是立刻关闭：见 [`super::CLOSE_RANGE_FLAGS`]——立刻关闭会
    /// 把 `std` 的错误管一起关掉，`execve` 失败时子进程 abort、父进程却拿到 `Ok`。
    /// 被执行的程序看不到任何继承 fd，两件事的保证相同。
    ///
    /// 失败 ⇒ 拒绝（**不 exec**）：标记不上就意味着"我数不清子进程手里有什么"，
    /// 那时候继续执行等于把没证明的事当成已证明。
    fn close_inherited_fds() -> Result<(), i32> {
        // 安全：三个整数参数。
        let returned = unsafe {
            syscall5(
                SYS_CLOSE_RANGE,
                CLOSE_RANGE_FIRST as u64,
                CLOSE_RANGE_LAST as u64,
                super::CLOSE_RANGE_FLAGS as u64,
                0,
                0,
            )
        };
        if returned != 0 {
            return Err(refusal(returned));
        }
        Ok(())
    }

    /// **子进程那一半的全部工作**：装过滤器 ＋ 清继承的 fd。`pre_exec` 钩子就是它。
    ///
    /// 约束（`CommandExt::pre_exec` 的规矩，见 `std::os::unix`）：
    ///
    /// - **不许分配**：这里只用静态数组与栈上的 `sock_fprog`，没有 `Vec`、没有 `format!`；
    /// - **不许碰锁**：没有 `println!`、没有全局状态；
    /// - 两个系统调用都只用整数。
    ///
    /// 顺序是**语义**：先装过滤器再处理 fd。反过来的话，两条系统调用之间的那一小段里，
    /// 子进程还带着调用方的 fd 且没有过滤器——那一小段正是这套机制要消除的东西。
    pub fn confine_child() -> io::Result<()> {
        install_raw().map_err(io::Error::from_raw_os_error)?;
        close_inherited_fds().map_err(io::Error::from_raw_os_error)?;
        Ok(())
    }

    /// `socket(2)` 在本架构上的号（测试拿它与被钉住的程序对齐，不另抄一份）。
    pub const SOCKET_SYSCALL: u32 = SYS_SOCKET;
    /// `io_uring_setup(2)` 的号（两种架构同一个）。
    pub const IO_URING_SETUP_SYSCALL: u32 = SYS_IO_URING_SETUP;

    /// 过滤器的可读形状（测试与审计用；真跑走 [`FILTER`] 本身）。
    pub fn program() -> Vec<SockFilter> {
        FILTER.to_vec()
    }
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub use arm::{
    FILTER, IO_URING_SETUP_SYSCALL, SOCKET_SYSCALL, SockFilter, confine_child, install, program,
    set_no_new_privs,
};

/// 别的架构：**没有实现**（不是"大概这样"）。
///
/// 一个猜出来的 syscall 表会让过滤器"装上了、拒的是别的系统调用"——那比不装更坏：
/// 探针会因为一条真跑的证据通过，而它在真机上根本没有兑现断网。
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
mod absent {
    use super::SeccompUnavailable;
    use std::io;

    /// 占位读数：本架构没有程序（长度 0）。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct SockFilter {
        pub code: u16,
        pub jt: u8,
        pub jf: u8,
        pub k: u32,
    }

    /// 空的过滤器表（任何安装尝试都会先被 [`install`] 拒绝）。
    pub const FILTER: [SockFilter; 0] = [];

    /// 本架构没有实现。
    pub fn install() -> Result<(), SeccompUnavailable> {
        Err(SeccompUnavailable::new(
            "本架构没有实现 seccomp 过滤器（只有 x86_64 与 aarch64 的 syscall 表）：\
             拒绝装一个猜出来的表",
        ))
    }

    /// `pre_exec` 路径同样拒绝（errno `ENOSYS`）。
    pub fn confine_child() -> io::Result<()> {
        Err(io::Error::from_raw_os_error(38))
    }

    /// 本架构没有实现（同 [`confine_child`]）。
    pub fn set_no_new_privs() -> io::Result<()> {
        Err(io::Error::from_raw_os_error(38))
    }

    /// 空表（没有可读的形状）。
    pub fn program() -> Vec<SockFilter> {
        Vec::new()
    }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub use absent::{FILTER, SockFilter, confine_child, install, program, set_no_new_privs};

/// 别的架构：没有 `socket` 号可读（也没有可装的表）。
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub const SOCKET_SYSCALL: u32 = 0;
/// 别的架构：没有 `io_uring_setup` 号可读。
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub const IO_URING_SETUP_SYSCALL: u32 = 0;

#[cfg(test)]
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
mod tests {
    use super::*;

    /// 程序长度与关键跳转：**socket + AF_INET 被拒**这一条必须真的在表里。
    ///
    /// 判据不是"某条指令的 `k` 等于 41"（那只证明有人写过 41）：它顺着**跳转目标**走一遍
    /// ——`JEQ socket` 的相等分支落到读族那一条，`JEQ AF_INET` 的相等分支落到
    /// `RET ERRNO|EACCES`。改坏任何一格（跳转偏移、动作码、族号）都会红。
    #[test]
    fn the_filter_refuses_ip_sockets_and_nothing_else() {
        assert_eq!(FILTER.len(), 13, "程序的长度是规格的一部分");
        assert_eq!(program().len(), FILTER.len());

        let at = |index: usize| FILTER[index];
        // 0/2/4：三次读内存（arch / nr / args[0]）。
        assert_eq!(at(0).code, 0x20);
        assert_eq!(at(0).k, 4, "先读 arch");
        assert_eq!(at(2).k, 0, "再读系统调用号");
        assert_eq!(at(4).k, 16, "最后读 args[0]（族的低 32 位）");

        // 1：架构不等 ⇒ 杀。目标 = 1 + 1 + jf = 12。
        assert_eq!(at(1).code, 0x15);
        assert_eq!(
            (1 + 1 + at(1).jf as usize),
            FILTER.len() - 1,
            "架构不对必须落到最后那条 KILL"
        );
        assert_eq!(at(FILTER.len() - 1).k, SECCOMP_RET_KILL_PROCESS);

        // 3：是 socket 吗。目标 = 3 + 1 + jf = 9（io_uring 检查）。
        assert_eq!(at(3).code, 0x15);
        assert_eq!(at(3).k, SOCKET_SYSCALL, "socket 的系统调用号");
        assert_eq!(3 + 1 + at(3).jf as usize, 9, "不是 socket 就去看 io_uring");

        // 5：AF_INET 的相等分支必须落到一条 `RET ERRNO|EACCES`。
        assert_eq!(at(5).k, 2, "AF_INET");
        let inet_target = 5 + 1 + at(5).jt as usize;
        assert_eq!(at(inet_target).code, 0x06, "落在一条 RET 上");
        assert_eq!(
            at(inet_target).k,
            SECCOMP_RET_ERRNO | EACCES,
            "AF_INET 必须被拒成 EACCES"
        );
        // 6：AF_INET6 同理（落到同一条 RET）。
        assert_eq!(at(6).k, 10, "AF_INET6");
        assert_eq!(6 + 1 + at(6).jt as usize, inet_target);

        // 7：别的族（AF_UNIX）放行；8：IP 族被拒。
        assert_eq!(at(7).k, SECCOMP_RET_ALLOW, "AF_UNIX 要照常");
        assert_eq!(at(8).k, SECCOMP_RET_ERRNO | EACCES);

        // 9：io_uring_setup 必须被拒（否则可用它的 SOCKET 操作绕开 socket）。
        assert_eq!(at(9).k, IO_URING_SETUP_SYSCALL);
        let io_target = 9 + 1 + at(9).jt as usize;
        assert_eq!(
            at(io_target).k,
            SECCOMP_RET_ERRNO | EACCES,
            "io_uring_setup 必须被拒成 EACCES"
        );
        assert_eq!(at(10).k, SECCOMP_RET_ALLOW);
    }

    /// `close_range` 的参数是**契约**：从 3 号开始、到 `~0U`、打 `CLOEXEC` 标记。
    ///
    /// ⚠ 第三条断言钉的是**实测结论**，不是规格原文：规格写 0（立刻关闭），真机上那样会
    /// 连带关掉 `std` 的错误管、让 `execve` 失败 abort 成 134（见 [`CLOSE_RANGE_FLAGS`]）。
    /// 4 给的是同一条继承保证，且保住 `spawn-failure` 的分类。
    #[test]
    fn close_range_starts_after_stdio_and_closes_everything() {
        assert_eq!(CLOSE_RANGE_FIRST, 3, "0/1/2 是 stdio，不许动");
        assert_eq!(CLOSE_RANGE_LAST, u32::MAX, "一路到最大值（~0U）");
        assert_eq!(
            CLOSE_RANGE_FLAGS, 4,
            "4 = CLOEXEC（exec 时关）；0 会打断错误上报"
        );
    }

    /// 本机装得上（**真发一次 prctl**）：装不上时 identity 后端必须如实不可用。
    ///
    /// ⚠ 它在本进程里装过滤器——之后这个测试进程**建不了 IP 套接字**。测试二进制是
    /// 一次性的，但同进程里别的用例可能起子进程（子进程照样继承，不影响它们读文件、
    /// 起 shell）。因此这一条只做**安装成功**这一件事，不做网络尝试（那在子进程里做，
    /// 见 `tests/terminal_app.rs` 的真执行用例）。
    #[test]
    fn the_filter_installs_on_this_machine() {
        if !SUPPORTED {
            eprintln!("skip: 本架构没有实现过滤器");
            return;
        }
        install().expect("本机必须装得上（identity 后端的断网全靠它）");
        // 再装一次同样成功：过滤器可以叠加，且每一条都比前一条更严或相同。
        install().expect("叠第二层同样成立");
    }
}
