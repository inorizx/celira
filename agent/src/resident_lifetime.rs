//! 一支住户的**进程寿命**：住户名下起的一切，随住户一起结束。
//!
//! ## 它解决什么（真机读数，2026-10-11）
//!
//! 主体能在终端里起一个服务，而那个服务**活在任何账之外**：它的父进程早就不在了
//! （真机读数 `parent=(父进程已不在)`），`node home.js down` 收不到它——实测：关家之后
//! 那个端口还在听，只能由主人按端口找到 pid 再手工收掉（收法归 [`crate::platform`] 那一层，
//! 本件不点名那个工具）。
//!
//! 裁决（2026-10-11 主人）：**主体起的东西活得跟住户一样久**。本件就是那句话的实现——
//! 住户的进程放进一个 OS 级的域，域随住户消失而消失 ⇒ **收口就是"住户结束"这件事本身**：
//! 不需要任何账、任何动词、任何人的记性。
//!
//! ## ⚠ 射程：它管的是"住户**自己拉起**的东西"（2026-10-11 门禁第一跑改的口径）
//!
//! 本件**不是**"孤儿从此不可能"。`scripts/home/probe-service-lifetime.js` 第一跑就抓到
//! 一条真路：主体把"起服务"这件事**交给别的进程去办**——
//! `Invoke-CimMethod -ClassName Win32_Process -MethodName Create …`（WMI）——于是新进程挂在
//! `WmiPrvSE.exe` 下、**根本不是宿主的后代** ⇒ 它从来没进过那个域 ⇒ 关家之后它照旧在听
//! （真机读数：`parent=WmiPrvSE.exe(4160)`，`down` 之后那个 pid 还在）。
//!
//! 所以准确的说法是：**住户自己拉起的一切（后代）在域内；由第三方代它拉起的
//! （WMI/CIM、计划任务、服务、别的常驻件）不在域内。** 后者要另一类收容
//! （受限令牌 / 独立身份 / 服务级），本刀**不做**，如实登记在这里与门禁文件头。
//! 本仓不许把"尽力"写成"绝对"——这一句就是那条纪律的产物。
//!
//! ## 为什么它不在 `platform.rs` 里
//!
//! `platform.rs` 的文件头把范围写死成"起子进程那几件"（默认 shell 名、怎么拼命令行、
//! 怎么击杀整棵进程树、凭据怎么洗）。本件是**寿命**那一格，不是那四件之一 ⇒ 另立一个
//! 同形状的平台缝（契约唯一、实现多份），而不是把那个文件的边界撑大。
//!
//! ## 契约（所有平台必须一样）
//!
//! **住户名下起的一切，随住户一起结束。** 平台臂可以不同，这句话不许不同——所以
//! 没有臂的平台**如实说"没有"**，不写一个近似的兜底（那会让同一条承诺在不同平台上
//! 变成两句不同的话）。
//!
//! ## 今天只有 Windows 臂
//!
//! Windows 臂用 Job Object（`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`）：住户进程进作业，
//! 它拉起的一切**默认留在作业里**（作业没开 breakaway ⇒ 子进程逃不掉，`Start-Process`
//! 那种"脱离"脱的只是父进程，不是作业），最后一个句柄一关（住户进程无论怎么结束）
//! 整组一起走。⚠ 这一条只管**后代**：不是它拉起的进程（见上一节那条 WMI 真机读数）
//! 从一开始就不在作业里，作业关不关都与它无关。
//!
//! **Linux 臂的触发条件**（写在这里，免得下次只剩"历史决定"）：真机上出现"孤儿活过
//! 关家"的读数时再立；cgroup / scope 各是一条路，都要真机验。今天 Linux 上这一格
//! **如实报 `not-contained:no-arm`**。
//!
//! ## 收容建不起来时：**不拒绝启动，但必须说出来**
//!
//! 它是**寿命**保证，不是安全墙：建不起来（受限环境、嵌套作业不允许等）时照常启动，
//! 但宿主必须打一行读数（`[resident] processes = …`）——否则就成了"承诺在、机制不在"
//! 的静默降级，而那正是本仓最不许的一种坏法。
//!
//! ## FFI 为什么手写
//!
//! 与 [`crate::platform`] 的 Linux 臂同一条先例（那里写着"为两个固定签名拉一个 crate
//! 不划算"）：这里只用几个固定签名的 Win32 调用（建作业、设限制、进作业、探活、关句柄）。
//! 签名由我们自己保证，并由 `tests/resident_lifetime.rs` 的**独立进程用例**钉住（真的起一个
//! 脱离的后代，再看它随住户一起消失没有）。

/// 收容读数：**住户外的一切随住户结束**这一条成立。
pub const CONTAINED: &str = "contained";

/// 收容读数：本平台**没有臂**（如实说，不写近似兜底）。
pub const NOT_CONTAINED_NO_ARM: &str = "not-contained:no-arm";

/// 收容读数：建作业失败（`CreateJobObjectW` 没给句柄）。
pub const NOT_CONTAINED_JOB_CREATE_FAILED: &str = "not-contained:job-create-failed";

/// 收容读数：作业建起来了，但"最后一个句柄一关就收掉整组"这条限制没设上。
pub const NOT_CONTAINED_LIMITS_FAILED: &str = "not-contained:limits-failed";

/// 收容读数：把住户进程放进作业失败（受限环境、或平台不支持嵌套作业）。
pub const NOT_CONTAINED_ASSIGN_FAILED: &str = "not-contained:assign-failed";

/// 把**当前进程**收进住户的进程域，返回一行收容读数。
///
/// 调用点只有一处：住户宿主进程启动的最前面（在它拉起任何东西之前）。返回的字符串
/// 是**读数**、不是错误——失败不拦启动，只如实说（见模块文档最后两节）。
pub fn adopt_current_process() -> &'static str {
    arm::adopt()
}

/// 那个 pid 现在什么状态：`Some(true)` 在跑、`Some(false)` 不在了、`None` **读不到**。
///
/// 调用点是"主体问'我起的那件还在不在'"（`terminal_held` 那一行读数）。⚠ **三态不许压成两态**：
/// "读不到"（不是我的进程、权限不够）与"不在了"是两件事，把它们合成一个 `false` 就是把
/// "不知道"说成"没有"——本仓最不许的一种坏法（与 [`crate::clock::UNKNOWN_TIME`] 同一条纪律）。
pub fn process_is_alive(pid: u32) -> Option<bool> {
    arm::is_alive(pid)
}

#[cfg(windows)]
mod arm {
    use std::ffi::c_void;

    use super::{
        CONTAINED, NOT_CONTAINED_ASSIGN_FAILED, NOT_CONTAINED_JOB_CREATE_FAILED,
        NOT_CONTAINED_LIMITS_FAILED,
    };

    /// Win32 的句柄（不透明指针）。
    type Handle = *mut c_void;
    /// Win32 的 `BOOL`（`i32`，非零为真）。
    type Bool = i32;

    /// `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` 那一类的编号（Win32 常量）。
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: u32 = 9;
    /// 作业的最后一个句柄关闭时，**收掉作业里所有进程**（Win32 常量）。
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimitInformation {
        basic_limit_information: BasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    // 几个固定签名的 Win32 调用（见模块文档"FFI 为什么手写"）。
    unsafe extern "system" {
        // 建一个匿名作业（不命名 ⇒ 不与别的进程撞名）。
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle;
        // 给作业设限制（这里只设"最后一个句柄关闭即收掉整组"）。
        fn SetInformationJobObject(job: Handle, class: u32, info: *mut c_void, length: u32)
        -> Bool;
        // 把一个进程放进作业。
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> Bool;
        // 当前进程的伪句柄（不需要、也不该关）。
        fn GetCurrentProcess() -> Handle;
        // 打开一个进程（只问状态，因此只要 `PROCESS_QUERY_LIMITED_INFORMATION`）。
        fn OpenProcess(access: u32, inherit: Bool, pid: u32) -> Handle;
        // 它在跑还是已经退了（`STILL_ACTIVE` = 259）。
        fn GetExitCodeProcess(process: Handle, code: *mut u32) -> Bool;
        // 关掉我们自己打开的那个句柄。
        fn CloseHandle(handle: Handle) -> Bool;
        // 上一个调用的错误码（用来把"它不在"与"我看不到它"分开）。
        fn GetLastError() -> u32;
    }

    /// `PROCESS_QUERY_LIMITED_INFORMATION`：只读状态所需的最小权限（Win32 常量）。
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    /// `STILL_ACTIVE`：还在跑（Win32 常量）。
    const STILL_ACTIVE: u32 = 259;
    /// `ERROR_INVALID_PARAMETER`：那个 pid 不存在（Win32 常量）。
    const ERROR_INVALID_PARAMETER: u32 = 87;

    /// 那个 pid 现在什么状态：`Some(true)` 在跑、`Some(false)` 不在了、`None` **读不到**。
    ///
    /// ⚠ 三种必须分得开（"缺省不等于零"）：打不开一个进程既可能是"它已经没了"，
    /// 也可能是"它是别人的、我没权限看"——**后者不许说成"不在了"**，所以要看错误码：
    /// `ERROR_INVALID_PARAMETER`（pid 不存在）⇒ 不在；其余 ⇒ 读不到。
    pub fn is_alive(pid: u32) -> Option<bool> {
        // 安全：出参只有一个句柄与一个 u32；句柄由我们自己关掉，不逃逸。
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return match GetLastError() {
                    ERROR_INVALID_PARAMETER => Some(false),
                    _ => None,
                };
            }
            let mut code: u32 = 0;
            let asked = GetExitCodeProcess(handle, &mut code);
            CloseHandle(handle);
            if asked == 0 {
                return None;
            }
            Some(code == STILL_ACTIVE)
        }
    }

    pub fn adopt() -> &'static str {
        // 安全：四个调用都是"无指针逃逸"的 Win32 调用——出参只有句柄与返回码；
        // 结构体是我们自己按 Win32 布局声明的，长度由 `size_of` 给。
        unsafe {
            let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if job.is_null() {
                return NOT_CONTAINED_JOB_CREATE_FAILED;
            }
            let mut limits = ExtendedLimitInformation::default();
            limits.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set = SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                (&mut limits as *mut ExtendedLimitInformation).cast(),
                std::mem::size_of::<ExtendedLimitInformation>() as u32,
            );
            if set == 0 {
                return NOT_CONTAINED_LIMITS_FAILED;
            }
            if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
                return NOT_CONTAINED_ASSIGN_FAILED;
            }
            // ⚠ 句柄**故意不关、也不存**：它必须活到本进程结束。本进程一结束（无论怎么结束），
            // 系统关掉最后一个句柄，上面那条限制于是把整组一起带走——这就是"跟住户一样久"。
            // 谁要是给它加一个 `Drop` 去 `CloseHandle`，这一格会在**启动的瞬间**把住户自己
            // 连同刚起的东西全杀掉；`tests/resident_lifetime.rs` 的独立进程用例钉着它。
            CONTAINED
        }
    }
}

#[cfg(target_os = "linux")]
mod arm {
    use super::NOT_CONTAINED_NO_ARM;

    /// 平台缝：**本平台今天没有收容臂**。如实说"没有"——不换一个近似行为（那会让同一条
    /// 承诺在不同平台上变成两句不同的话）。触发条件见模块文档。
    pub fn adopt() -> &'static str {
        NOT_CONTAINED_NO_ARM
    }

    /// 那个 pid 现在什么状态（Linux 臂）：`kill(pid, 0)` 只问"这个进程在不在"，
    /// **不发任何信号**（信号 0 是 POSIX 的探活约定）。
    ///
    /// 三种必须分得开：`Ok` ⇒ 在跑；`ESRCH` ⇒ 不在了；`EPERM` ⇒ **在，但不是我的、我看不到**
    /// ⇒ 照旧算"在跑"（POSIX 明写它表示进程存在而无权发信号）；其余 errno ⇒ 读不到。
    pub fn is_alive(pid: u32) -> Option<bool> {
        let Ok(pid) = i32::try_from(pid) else {
            return Some(false);
        };
        // 安全：无指针、只读系统状态；信号 0 不发信号。
        if unsafe { sys::kill(pid, 0) } == 0 {
            return Some(true);
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(3) => Some(false),
            Some(1) => Some(true),
            _ => None,
        }
    }

    /// 一个**从 POSIX 起签名没变过**的系统调用（与 `crate::platform` 的 Linux 臂同一条先例）。
    mod sys {
        unsafe extern "C" {
            /// 发信号（`kill(2)`）。`signal` 为 0 时**只做存在性与权限检查**，不送信号。
            pub fn kill(pid: i32, signal: i32) -> i32;
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod arm {
    use super::NOT_CONTAINED_NO_ARM;

    /// 平台缝：**本平台今天没有臂**。如实说"没有"——不换一个近似行为。
    pub fn adopt() -> &'static str {
        NOT_CONTAINED_NO_ARM
    }

    /// 没有臂的平台：连"它在不在"都读不到 ⇒ `None`（**不许拿"读不到"当"不在了"**）。
    pub fn is_alive(_pid: u32) -> Option<bool> {
        None
    }
}
