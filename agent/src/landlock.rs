//! **写边界的第二把锁**（Linux 5.13+ 的 Landlock）：把子进程的**写入**关进环境根。
//!
//! ## 为什么需要它（seccomp 管不了这一半）
//!
//! identity 后端不开命名空间，它原本的写边界是 **uid 的 DAC**——"系统写不了"由非 root 成立，
//! 但"**只能写环境根**"在同 uid 部署下是假的：住户能在主人的家里造兄弟目录、写主人的文件。
//! 2026-10-23 的验收工具在同一个 uid 下就是这一条 `FAIL`。
//!
//! Landlock 补的正是这一格：它是**无特权**的（不需要 root、不需要 user namespace、也不受
//! `apparmor_restrict_unprivileged_userns` 影响），内核在 `restrict_self` 之后对所有后代
//! 生效，且**不可撤销**。实测（Ubuntu 24.04 / 内核 6.8 / ABI 4）：
//!
//! ```text
//! 写环境根内     OK
//! 写根外/兄弟目录 Permission denied
//! 写 /tmp        Permission denied
//! 读 /etc        照常（读不在射程里）
//! ```
//!
//! ⚠ `/tmp` 写不了这一条会打断"用临时文件"的命令（`mktemp`、`git`、编译器）。因此编排层
//! 在装这一件之前把 `TMPDIR` 指到**环境根里**一个固定目录（`.celira-tmp`）：临时文件还是
//! 写在根里，边界不动。
//!
//! ## 它只做一件事：**写在哪儿**
//!
//! 处理的是 `handled_access_fs` 里的**写类**权限：`WRITE_FILE` / `REMOVE_*` / `MAKE_*` /
//! `REFER` / `TRUNCATE`。**不处理读**——Landlock 的规则是"允许清单"（父目录一允许就覆盖子树），
//! 做不出"整个系统可读、只挡住某个目录"这种减法。因此：
//!
//! - **能写哪儿**：这一件管；
//! - **能读哪儿**：管不了（同 uid 的文件照旧读得到）。要那一条，靠部署把住户放进专用 uid
//!   （`deploy/celira-identity-setup.sh`）。
//!
//! ## ABI 差异是判据，不是兼容性噪音
//!
//! `REFER`（ABI 2）与 `TRUNCATE`（ABI 3）是后加的位：把老内核不认识的位塞进 `handled_access_fs`
//! 会让 `create_ruleset` 直接 `EINVAL`——那等于**一整套写边界没装上**。因此版本先问一次
//! （`landlock_create_ruleset(NULL, 0, VERSION)`），再按版本裁位。ABI < 1（没有 Landlock）⇒
//! 这一件**如实不可用**，identity 后端跟着不可用——不退回 DAC 假装还成立。

/// Landlock 的 ABI 版本（`landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Abi(pub i32);

impl Abi {
    /// 这一版认得的**写类**权限位。
    ///
    /// 从 `WRITE_FILE` 到 `MAKE_SYM` 是 ABI 1 就有的；`REFER` 是 ABI 2；`TRUNCATE` 是 ABI 3。
    pub fn write_rights(self) -> u64 {
        const BASE: u64 = (1 << 1)   // WRITE_FILE
            | (1 << 4)               // REMOVE_DIR
            | (1 << 5)               // REMOVE_FILE
            | (1 << 6)               // MAKE_CHAR
            | (1 << 7)               // MAKE_DIR
            | (1 << 8)               // MAKE_REG
            | (1 << 9)               // MAKE_SOCK
            | (1 << 10)              // MAKE_FIFO
            | (1 << 11)              // MAKE_BLOCK
            | (1 << 12); // MAKE_SYM
        let mut rights = BASE;
        if self.0 >= 2 {
            rights |= 1 << 13; // REFER：跨目录改名/链接
        }
        if self.0 >= 3 {
            rights |= 1 << 14; // TRUNCATE：`truncate(2)` / `O_TRUNC`
        }
        rights
    }
}

/// 这一件装不上——**不是**"先跳过"。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LandlockUnavailable {
    reason: String,
}

impl LandlockUnavailable {
    /// 带原因建一份。
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    /// 稳定分类标记（诊断与入口报错用）。
    pub fn as_str(&self) -> &'static str {
        "landlock-unavailable"
    }

    /// 人话原因。
    pub fn detail(&self) -> &str {
        &self.reason
    }
}

#[cfg(target_os = "linux")]
mod arm {
    use super::Abi;
    use std::io;
    use std::os::raw::c_char;

    /// `landlock_create_ruleset` 的 `flags`：只问版本，不建规则集。
    const CREATE_RULESET_VERSION: u32 = 1;
    /// `landlock_add_rule` 的规则类型：`LANDLOCK_RULE_PATH_BENEATH`。
    const RULE_PATH_BENEATH: i32 = 1;

    /// `struct landlock_ruleset_attr`（只要 `handled_access_fs`；**8 字节**，
    /// 这样 ABI 1..=5 都收——网络那一格我们不用（断网是 seccomp 的事））。
    #[repr(C)]
    struct RulesetAttr {
        handled_access_fs: u64,
    }

    /// `struct landlock_path_beneath_attr`：**packed，12 字节**（`u64` ＋ `s32`）。
    ///
    /// 布局是内核 ABI：`__attribute__((packed))`。写成 16 字节会让 `add_rule` 读错。
    #[repr(C, packed)]
    struct PathBeneath {
        allowed_access: u64,
        parent_fd: i32,
    }

    #[cfg(target_arch = "x86_64")]
    mod numbers {
        pub const SYS_LANDLOCK_CREATE_RULESET: i64 = 444;
        pub const SYS_LANDLOCK_ADD_RULE: i64 = 445;
        pub const SYS_LANDLOCK_RESTRICT_SELF: i64 = 446;
        pub const SYS_OPENAT: i64 = 257;
        pub const SYS_CLOSE: i64 = 3;
    }

    #[cfg(target_arch = "aarch64")]
    mod numbers {
        pub const SYS_LANDLOCK_CREATE_RULESET: i64 = 444;
        pub const SYS_LANDLOCK_ADD_RULE: i64 = 445;
        pub const SYS_LANDLOCK_RESTRICT_SELF: i64 = 446;
        pub const SYS_OPENAT: i64 = 56;
        pub const SYS_CLOSE: i64 = 57;
    }

    use numbers::*;

    /// `O_PATH`（只要一个"指向这个目录"的句柄，不读内容）。
    const O_PATH: i32 = 0o10000000;
    /// `O_CLOEXEC`。
    const O_CLOEXEC: i32 = 0o2000000;
    /// `AT_FDCWD`。
    const AT_FDCWD: i32 = -100;

    /// 发一个系统调用（x86_64 / aarch64 都是 `syscall` 那一族；这里只需 0..=4 个参数）。
    #[cfg(target_arch = "x86_64")]
    unsafe fn syscall(number: i64, a: u64, b: u64, c: u64, d: u64) -> i64 {
        let result: i64;
        // 安全：整数参数；`rcx` / `r11` 由 `syscall` 自己改。
        unsafe {
            core::arch::asm!(
                "syscall",
                inlateout("rax") number => result,
                in("rdi") a,
                in("rsi") b,
                in("rdx") c,
                in("r10") d,
                lateout("rcx") _,
                lateout("r11") _,
                options(nostack),
            );
        }
        result
    }

    #[cfg(target_arch = "aarch64")]
    unsafe fn syscall(number: i64, a: u64, b: u64, c: u64, d: u64) -> i64 {
        let mut value = a as i64;
        // 安全：整数参数；`svc 0` 只改 `x0` 与条件码。
        unsafe {
            core::arch::asm!(
                "svc 0",
                in("x8") number,
                inout("x0") value,
                in("x1") b,
                in("x2") c,
                in("x3") d,
                options(nostack),
            );
        }
        value
    }

    /// 内核里有多新（`None` ⇒ 没有 Landlock）。
    pub fn abi() -> Option<Abi> {
        // 安全：`landlock_create_ruleset` 在这里只被用来"问版本"（指针为空、长度为 0）。
        let version = unsafe {
            syscall(
                SYS_LANDLOCK_CREATE_RULESET,
                0,
                0,
                CREATE_RULESET_VERSION as u64,
                0,
            )
        };
        if version < 1 {
            return None;
        }
        Some(Abi(version as i32))
    }

    /// **把写入关进这个目录**（`execve` 之前调；之后不可撤销）。
    ///
    /// `root` 是**已经打开好的目录 fd**（`O_PATH`）——不在 `pre_exec` 里拼路径、不分配。
    /// 调用方负责在父进程里打开它（见 [`super::open_directory`]）。
    ///
    /// 顺序是语义：**先建规则集、再加规则、最后 `restrict_self`**；中间任何一步失败都
    /// 返回 `Err`，调用方必须**放弃 exec**——装了一半等于没装。
    pub fn restrict_writes_to(root: i32) -> io::Result<()> {
        let Some(abi) = abi() else {
            return Err(io::Error::from_raw_os_error(38)); // ENOSYS
        };
        let rights = abi.write_rights();
        let attr = RulesetAttr {
            handled_access_fs: rights,
        };
        // 安全：`attr` 在本次调用期间有效；长度是结构体的真实大小（8 字节）。
        let ruleset = unsafe {
            syscall(
                SYS_LANDLOCK_CREATE_RULESET,
                &attr as *const RulesetAttr as u64,
                std::mem::size_of::<RulesetAttr>() as u64,
                0,
                0,
            )
        };
        if ruleset < 0 {
            return Err(io::Error::from_raw_os_error((-ruleset) as i32));
        }
        let ruleset = ruleset as i32;

        let rule = PathBeneath {
            allowed_access: rights,
            parent_fd: root,
        };
        // 安全：`rule` 在本次调用期间有效；内核只读它。
        let added = unsafe {
            syscall(
                SYS_LANDLOCK_ADD_RULE,
                ruleset as u64,
                RULE_PATH_BENEATH as u64,
                &rule as *const PathBeneath as u64,
                0,
            )
        };
        if added != 0 {
            // 安全：只关一个本进程刚开的 fd。
            unsafe { syscall(SYS_CLOSE, ruleset as u64, 0, 0, 0) };
            return Err(io::Error::from_raw_os_error((-added) as i32));
        }

        // `restrict_self` 要求先有 `no_new_privs`（由调用方在 seccomp 那一步设过；
        // 这里再设一次是幂等的，且不依赖调用顺序）。
        let nnp = crate::seccomp::set_no_new_privs();
        if let Err(error) = nnp {
            // 安全：同上。
            unsafe { syscall(SYS_CLOSE, ruleset as u64, 0, 0, 0) };
            return Err(error);
        }
        // 安全：`ruleset` 是本进程的有效 fd。
        let restricted = unsafe { syscall(SYS_LANDLOCK_RESTRICT_SELF, ruleset as u64, 0, 0, 0) };
        // 安全：同上。
        unsafe { syscall(SYS_CLOSE, ruleset as u64, 0, 0, 0) };
        if restricted != 0 {
            return Err(io::Error::from_raw_os_error((-restricted) as i32));
        }
        Ok(())
    }

    /// 在**子进程**里打开一个目录（`O_PATH|O_CLOEXEC`），给 [`restrict_writes_to`] 用。
    ///
    /// 为什么用它而不是标准库的文件句柄：`pre_exec` 里不许分配、也不该拼路径字符串。这里拿
    /// 一个**父进程里备好**的 C 字符串指针，直接发一个 `openat`。
    ///
    /// # Safety
    ///
    /// `path` 必须指向一个以 NUL 结尾、且在本次调用期间一直有效的字符串（调用方持有它
    /// 到 `spawn` 结束）。传空指针或悬垂指针是未定义行为。
    pub unsafe fn open_directory(path: *const c_char) -> io::Result<i32> {
        // 安全：调用方保证 `path` 有效。
        let fd = unsafe {
            syscall(
                SYS_OPENAT,
                AT_FDCWD as i64 as u64,
                path as u64,
                (O_PATH | O_CLOEXEC) as u64,
                0,
            )
        };
        if fd < 0 {
            return Err(io::Error::from_raw_os_error((-fd) as i32));
        }
        Ok(fd as i32)
    }
}

#[cfg(target_os = "linux")]
pub use arm::{abi, open_directory, restrict_writes_to};

/// 别的平台：没有 Landlock（Windows 要走另一套）。**不是**"大概能装"。
#[cfg(not(target_os = "linux"))]
pub fn abi() -> Option<Abi> {
    None
}

/// 别的平台：没有实现。
#[cfg(not(target_os = "linux"))]
pub fn restrict_writes_to(_root: i32) -> std::io::Result<()> {
    Err(std::io::Error::from_raw_os_error(38))
}

/// 别的平台：没有实现。
#[cfg(not(target_os = "linux"))]
pub fn open_directory(_path: *const std::os::raw::c_char) -> std::io::Result<i32> {
    Err(std::io::Error::from_raw_os_error(38))
}

/// 把"这台内核支持到哪一版"翻成一句人话（诊断/回执用）。
pub fn reading() -> String {
    match abi() {
        Some(abi) => format!("landlock = abi {}（写入关进环境根）", abi.0),
        None => "landlock = 不可用（内核没有或没编进去）".to_string(),
    }
}

/// 装不上时的统一错误（给探针用）。
pub fn unavailable() -> Option<LandlockUnavailable> {
    if abi().is_none() {
        Some(LandlockUnavailable::new(
            "内核没有 Landlock（< 5.13 或未编入）：identity 后端的写边界只能退回 uid 的 DAC",
        ))
    } else {
        None
    }
}
