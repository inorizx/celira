//! **沙箱后端端口**：`terminal=sandbox` 那一档「用哪只手执行」——端口在这里，实现可插拔。
//!
//! > **名字**：`docs/runtime/terminal/` 那份沙箱裁决稿（§4.1）给这个端口起的名字带「供应商」
//! > 那族英文词。代码里叫 `SandboxBackend`（`BwrapBackend` 是它的第一支实现）——因为本仓的
//! > 语言守卫（`tests/minimal_agent.rs` 里扫供应商词那一条）**把那一族词留给模型适配层**。
//! > 这里说的确实不是供应商，但**一个词两义**正是那条守卫要拦的东西；端口的形状与那份裁决
//! > 稿 §4.1 逐条对应（`name` / `guarantees` / `probe` / `command`）。
//!
//! ## 为什么要有这一件（而不是把 bwrap 塞进 terminal_app）
//!
//! `docs/runtime/terminal/` 那份沙箱裁决稿 §4 的裁决：**可插拔的是执行后端，
//! 不是权限语义**。`terminal` 永远只有 `none | sandbox | full` 三档，「sandbox 的最低保证集」
//! 固定；换的是**用什么机制**去兑现它（今天是 Linux 的 bubblewrap，将来可以是 rootless 容器、
//! 微虚拟机、Windows 的 AppContainer、macOS 的 sandbox-exec）。
//!
//! 因此这里只放三样东西：**请求**（[`SandboxRequest`]）、**端口**（[`SandboxBackend`]）、
//! **两支内置实现**（[`BwrapBackend`] 与 [`IdentityBackend`]）。五堵墙与失败词表**不在这里**——它们在
//! `terminal_app` 的编排层，两个档位共用同一份；一个后端能改的只有「命令行怎么拼」和
//! 「这台机器上能不能兑现保证」。
//!
//! ## 端口为什么长这样（每一格都在回答一个具体问题）
//!
//! - `name()`：**审计用的稳定名字**。回执里写的是它，不是「某个沙箱」；
//! - `guarantees()`：**它真的能兑现什么**。不是广告词——§4.1 那条纪律：没有对应保证的后端，
//!   不能被选来满足 `sandbox` 档。今天 bwrap 报的 `resources=none`（v1 不限 CPU / 内存）是
//!   **如实登记的缺口**，不是遗漏；
//! - `probe()`：**这台机器上可用吗**。它是**真跑**，不是 `which bwrap`——
//!   判据见 [`BwrapBackend::probe`]（命名空间 / 只读系统 / 断网三样都要有证据）；
//! - `command()`：**只拼命令行**，不起进程、不读环境、不碰 stdio。进程由编排层起
//!   （五堵墙在那一边），因此「谁有权起进程」这件事仍然只有一个出口。
//!
//! ## 没有合格后端 ⇒ 拒绝，**不退回宿主**
//!
//! 这是本模块唯一一条不许商量的纪律（§5 / §7）：`sandbox` 不可用时入口**非零退出**并报
//! [`SANDBOX_UNAVAILABLE`]。**不退回 `full`，也不在宿主上裸跑**——那会把「我要一个沙箱」
//! 悄悄读成「那就在外面跑吧」。
//!
//! ## 第二支实现：`identity`（2026-10-23；不建任何命名空间）
//!
//! `docs/runtime/terminal/SANDBOX_IDENTITY_DECISIONS.md` 的裁决：**边界装在住户身份上**。
//! 主流发行版把无特权 user namespace 关了（本机实测：bwrap 在 `RTM_NEWADDR` 上被拒），
//! 于是"要沙箱就得放开 userns"成了一个全局弱化——那一刀不接受。第二支实现换的是**代价**：
//!
//! | 那一半 | 怎么兑现 | 兑现不了什么 |
//! |---|---|---|
//! | 写面 | **DAC**：以非 root 的 uid 跑，`/etc`、别人的 700 目录由权限位挡住 | **不是挂载隔离**：uid 写得到的地方照旧写得到（家目录、仓库、`/tmp`） |
//! | 断网 | **seccomp**（`crate::seccomp`）：子进程建不出 `AF_INET` / `AF_INET6` 套接字 | **不是 netns**：`AF_UNIX` 照旧通；不认地址与端口 |
//! | 继承句柄 | `close_range(3, ~0U, CLOEXEC)`（同一份 seccomp 钩子） | 只标记 3 号之后；0/1/2 是 stdio，本来就要给 |
//!
//! **它的探针不是 `which`**：四件事都在真跑——有效 uid 不是 0、环境根里建得出文件、
//! `/etc` 里建不出文件、以及**一条真子进程**（装同一套 seccomp ＋ `close_range`）连不出网
//! 且看不到继承的 fd。判据与证据写进 [`IdentityBackend::probe`]。
//!
//! ⚠ **保证集照实降级**：`pid=none` / `ipc=none` / `uts=none`（没有命名空间）、
//! `resources=none`。它兑现的是"写不出系统、上不了网、拿不到调用方的 fd"，**不是**
//! "看不见主人的东西"——同一 uid 的进程照旧互相可见、可 kill。这与 bwrap 那一支的
//! 负空间是同一句话，不是新承诺。
//!
//! **入口的身份纪律**（两个 bin 共用一份，见 [`choose_for_entry`]）：以 root 跑 `sandbox`
//! 档 ⇒ 拒绝（写边界靠 uid，root 无视它）；选中 `identity` 而环境根的 owner 不是当前
//! 有效 uid ⇒ 拒绝并给可执行的修复说明（**不 chown、不回退**）。两者报的是
//! [`SANDBOX_IDENTITY_MISMATCH`]，**不是** `sandbox-unavailable`：后者是"这台机器没有能兑现
//! 的后端"（装一个），前者是"这机器能、但你用错了身份"（换一个身份跑）——修法不同，
//! 词就不能共用一个。
//!
//! **`systemd-run --user` 不是兜底**（2026-10-23 本机实测，见裁决文档 §5）：systemd 255 在
//! 用户管理器下会接受 `ProtectSystem` / `ProtectHome` / `PrivateTmp` / `PrivateNetwork`
//! 属性，但因为没有 `CAP_SYS_ADMIN`，挂载与网络命名空间**根本没建**——`ProtectSystem=strict`
//! 下仍然写得了工作区外。它不是沙箱，因此这里**没有**它的实现，也不留「先试 bwrap、不行换它」
//! 这种看似稳妥的分支。
//!
//! ## 这一档今天**不做**什么（负空间，写清楚免得被读成「已经安全」）
//!
//! - **不限资源**：CPU / 内存 / 进程数 / 墙钟都没有 cgroup；`timeout` 仍是编排层那一堵墙；
//! - **不做网络白名单**：`network` 只有 `none` 这一档（`--unshare-net`）。allowlist 要另一套
//!   机制（veth / slirp / 代理），不在第一刀里；
//! - **不做持久工作区以外的可写面**：除了环境根，系统一律只读（写 `/tmp` 落在 tmpfs 上，
//!   家一关就没）；
//! - **不是「防逃逸」的证明**：bwrap 的隔离边界是 Linux 命名空间，内核漏洞、`/proc` 的
//!   信息面、以及「同一个 uid 能读的宿主文件」都不在本件的射程内——`--ro-bind / /` 意味着
//!   **系统可读**，只是不可写；
//! - **不是保密边界**（2026-10-23 真机读数）：`--ro-bind / /` 把整个文件系统**挂上了**，
//!   于是同一个 uid 读得到的文件照旧读得到，环境里那些指向 gpg/ssh agent 与桌面会话总线的
//!   **unix socket 也照旧通**（路径在环境里、inode 在挂载里）。这一档兑现的是
//!   **"写不出去、上不了网、看不见别的进程"**，不是"看不见主人的东西"——后者要的是另一个
//!   uid 或一台虚拟机，不在这一刀里。环境里那些非凭据变量（代理、路径、会话号）也照旧传进去
//!   （洗的只是凭据家族那一份，与 `full` 同一条墙）。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 沙箱档不可用时的稳定分类标记（入口报错用；**不进动作失败词表**——那五项封闭不变）。
pub const SANDBOX_UNAVAILABLE: &str = "sandbox-unavailable";

/// 第一支内置后端的稳定名字（审计用）。
pub const BWRAP_BACKEND: &str = "bwrap";

/// 第二支内置后端的稳定名字（审计用）：**不建命名空间**，靠 uid ＋ seccomp。
pub const IDENTITY_BACKEND: &str = "identity";

/// 沙箱里的临时区（环境根之内；Landlock 下 `/tmp` 不可写）。
#[cfg(target_os = "linux")]
const IDENTITY_TEMP: &str = ".celira-tmp";

/// **入口的身份纪律**被破坏时的稳定分类标记。
///
/// 与 [`SANDBOX_UNAVAILABLE`] **刻意不是同一个词**：那一个说"这台机器上没有能兑现的后端"
/// （修法是装 bwrap / 换一台机器）；这一个说"机器能，但你用错了身份"（修法是以住户身份跑，
/// 或者把环境根交给那个身份）。两者都让入口**非零退出**，但操作者下一步做的事不同——
/// 合成一个词就等于把两种修法混成一句"沙箱不可用"。
pub const SANDBOX_IDENTITY_MISMATCH: &str = "sandbox-identity-mismatch";

/// 后端可执行文件的名字（`PATH` 上找它）。
const BWRAP_PROGRAM: &str = "bwrap";

/// 探针的最长等待（秒级）：探针自己挂了不能把入口挂死。
///
/// 正常机器上它是一次几毫秒的进程（2026-10-23 本机实测 ≈4.5 ms/次）；10 秒只是
/// 「它卡住了」与「它没事」之间的那条界。
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// 轮询探针子进程的间隔。
const PROBE_POLL: Duration = Duration::from_millis(10);

/// 探针自己的 `PATH`（一份最小集合：探针只用 shell 的内建与 `/proc`，不需要调用方的环境）。
const PROBE_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

// ---------------------------------------------------------------------------
// 保证集（后端声称兑现的东西）
// ---------------------------------------------------------------------------

/// 一个后端**真的**兑现的保证集。
///
/// 取值是**封闭的短语**，不是自由文本：`files` / `network` / `pid` / `ipc` / `uts` /
/// `environment` / `resources` / `workspace` 八格，每格一个词。判据一句话：
/// **说得出来的保证，必须有一条测试或一次真跑在背后**（§4.1）；说不出来的照实写
/// `none`，不许留白、也不许写成「尽力」。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guarantees {
    entries: Vec<(&'static str, &'static str)>,
}

impl Guarantees {
    /// 按给定的格子建一份（顺序就是渲染顺序）。
    pub fn of(entries: Vec<(&'static str, &'static str)>) -> Self {
        Self { entries }
    }

    /// 一格的值（不认识的名字 ⇒ `None`，**不猜**）。
    pub fn value(&self, name: &str) -> Option<&'static str> {
        self.entries
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| *value)
    }

    /// 一行读数（`files=… network=… …`）——入口那一行与审计都用它。
    pub fn reading(&self) -> String {
        self.entries
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

// ---------------------------------------------------------------------------
// 请求与命令行
// ---------------------------------------------------------------------------

/// 一次沙箱执行的**请求**（端口另一侧的输入）。
///
/// 它只携带「这一条命令要在哪儿、以什么词法、在哪个根里跑」——**不携带**洗过的环境
/// （那是编排层的五堵墙之一，由它自己装进进程），也不携带超时（同上）。
#[derive(Clone, Copy, Debug)]
pub struct SandboxRequest<'a> {
    /// 环境根：**唯一可写的那一块**（`--bind <根> <根>`）。系统其余部分只读。
    pub environment_root: &'a Path,
    /// 在哪个目录里执行（**必须**在环境根之内；缺省就是环境根本身）。
    pub cwd: &'a Path,
    /// 要执行的命令全文（交给 shell 原样执行）。
    pub command: &'a str,
    /// 命令词法所属的 shell（与平台臂同一个词）。
    pub shell: &'a str,
}

impl<'a> SandboxRequest<'a> {
    /// 建一份请求。
    pub fn new(
        environment_root: &'a Path,
        cwd: &'a Path,
        command: &'a str,
        shell: &'a str,
    ) -> Self {
        Self {
            environment_root,
            cwd,
            command,
            shell,
        }
    }
}

/// 编排层在**起进程时**要多做的那一件事（后端声明，编排层执行）。
///
/// 为什么它是命令行的一部分而不是后端自己的事：装过滤器/清 fd 都发生在 `execve` **之前**，
/// 而"起进程"只有编排层那一处出口（后端连 `Command` 都拿不到，见 [`SandboxBackend::command`]）。
/// 后端能做的只有**如实声明**"这一条 argv 要配哪一套 confinement"。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confinement {
    /// 不加：命令行自己就是完整的隔离（bwrap 建命名空间的那一支）。
    None,
    /// 子进程 `execve` 前：装 seccomp（拒 `AF_INET` / `AF_INET6` 套接字与 `io_uring_setup`）
    /// ＋ `close_range(3, ~0U, CLOEXEC)` 让继承的 fd 在 `execve` 时全部关掉（`crate::seccomp`）。
    SeccompNoIp,
}

impl Confinement {
    /// 稳定读数（回执/诊断里能写出"这一条命令靠什么 confinement"）。
    pub fn as_str(self) -> &'static str {
        match self {
            Confinement::None => "none",
            Confinement::SeccompNoIp => "seccomp-no-ip",
        }
    }
}

/// 后端给出的**命令行**（程序 ＋ 参数）。**数据，不是进程**。
///
/// 为什么不让后端直接 `spawn`：起进程是「谁有权碰整台机器」的那道口子，它必须只有一处
/// （编排层）；后端只回答「怎么拼」。这样端口可以离线判错（拼出来的 argv 是不是那十条），
/// 而五堵墙也不会被第二个实现悄悄绕开。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxCommand {
    program: String,
    args: Vec<String>,
    confinement: Confinement,
    working_directory: Option<PathBuf>,
    temp_directory: Option<PathBuf>,
}

impl SandboxCommand {
    /// 拼一份命令行（**没有** confinement、**不设**工作目录：bwrap 那一支的逐字形状）。
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            confinement: Confinement::None,
            working_directory: None,
            temp_directory: None,
        }
    }

    /// 拼一份**要配 confinement** 的命令行（identity 那一支）。
    pub fn confined(
        program: impl Into<String>,
        args: Vec<String>,
        confinement: Confinement,
    ) -> Self {
        Self {
            confinement,
            ..Self::new(program, args)
        }
    }

    /// 起进程时在**哪个目录**里（`None` ⇒ 不设，进程自己站在哪儿就是哪儿）。
    ///
    /// 为什么它是命令行的一格而不是后端"顺手"设的：`current_dir` 是 `Command` 的方法，
    /// 后端同样拿不到；bwrap 用 `--chdir`、identity 只能让编排层设进程自己的 cwd——
    /// **同一件事的两种落点**，因此归结到同一格里。
    pub fn with_working_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(directory.into());
        self
    }

    /// 这一条命令的 `TMPDIR` 该指向哪儿（`None` ⇒ 不设，用系统默认）。
    ///
    /// 为什么它是后端声明的一格：identity 那支的 Landlock **连 `/tmp` 都不许写**，命令一用
    /// 临时文件就失败；bwrap 那支的 `/tmp` 是私有 tmpfs，照常可写。两种落点，同一件事——
    /// 与 `working_directory` 同款：后端知道自己的隔离长什么样，编排层只负责设。
    pub fn with_temp_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.temp_directory = Some(directory.into());
        self
    }

    /// `TMPDIR` 该指向哪儿。
    pub fn temp_directory(&self) -> Option<&std::path::Path> {
        self.temp_directory.as_deref()
    }

    /// 要起哪个程序。
    pub fn program(&self) -> &str {
        &self.program
    }

    /// 参数表（原样交给 `Command::args`）。
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// 这条命令行要配的 confinement（编排层照着装）。
    pub fn confinement(&self) -> Confinement {
        self.confinement
    }

    /// 起进程时的工作目录（`None` ⇒ 不设）。
    pub fn working_directory(&self) -> Option<&Path> {
        self.working_directory.as_deref()
    }
}

/// 后端**拒绝**一份请求的理由（闭合两条）。
///
/// ⚠ 它**不是**动作失败词表：编排层把它映射成自己那五项里的 `invalid-argument`
/// （「这是一次无法执行的请求」——本地拒绝、零执行）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxRequestError {
    /// 工作目录越出了环境根（沙箱只允许写自己那一块）。
    CwdOutsideWorkspace,
    /// 环境根本身不是一个目录（无从绑定）。
    WorkspaceUnavailable,
}

impl SandboxRequestError {
    /// 稳定标记（诊断/回执用；映射成动作分类由编排层做）。
    pub fn as_str(self) -> &'static str {
        match self {
            SandboxRequestError::CwdOutsideWorkspace => "cwd-outside-workspace",
            SandboxRequestError::WorkspaceUnavailable => "workspace-unavailable",
        }
    }
}

// ---------------------------------------------------------------------------
// 不可用（**不是**「降级」，是拒绝）
// ---------------------------------------------------------------------------

/// 这台机器上**没有合格后端**：带上人话原因，绝不自己找一条更弱的路。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxUnavailable {
    reason: String,
}

impl SandboxUnavailable {
    /// 带原因建一份。
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    /// 稳定分类标记（入口报错那一行必须带上它）。
    pub fn as_str(&self) -> &'static str {
        SANDBOX_UNAVAILABLE
    }

    /// 人话原因（哪一步、看到了什么）。
    pub fn detail(&self) -> &str {
        &self.reason
    }

    /// 一整行：`sandbox-unavailable：<原因>`。
    pub fn message(&self) -> String {
        format!("{SANDBOX_UNAVAILABLE}：{}", self.reason)
    }
}

// ---------------------------------------------------------------------------
// 入口的身份纪律（两个 bin 共用一份判据）
// ---------------------------------------------------------------------------

/// **入口的身份纪律**被破坏：以 root 组装 `sandbox` 档，或者环境根不属于当前有效 uid。
///
/// 它与 [`SandboxUnavailable`] **是两类失败**（见 [`SANDBOX_IDENTITY_MISMATCH`] 的注释）：
/// 一个是"机器不行"，一个是"身份不对"。**不许把它降级成 `sandbox-unavailable`**——
/// 那会让操作者去装一个后端，而真正要换的是跑这个进程的身份。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxIdentityFailure {
    reason: String,
}

impl SandboxIdentityFailure {
    /// 带原因建一份。
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    /// 稳定分类标记（入口报错那一行必须带上它）。
    pub fn as_str(&self) -> &'static str {
        SANDBOX_IDENTITY_MISMATCH
    }

    /// 人话原因（哪一条不成立、怎么修）。
    pub fn detail(&self) -> &str {
        &self.reason
    }

    /// 一整行：`sandbox-identity-mismatch：<原因>`。
    pub fn message(&self) -> String {
        format!("{SANDBOX_IDENTITY_MISMATCH}：{}", self.reason)
    }
}

/// 当前进程的**有效 uid**（读 `/proc/self/status` 的 `Uid:` 行，无新依赖）。
///
/// 为什么不用 `std::fs::metadata("/proc/self").uid()`：那读到的是 `/proc/self` 这个
/// **目录**的 owner（procfs 上恒为当前 uid），它证明的是"我在读我自己的 procfs 条目"，
/// 而不是"内核认为我的有效 uid 是多少"。`Uid:` 行的第二格就是 `geteuid(2)` 的读数。
///
/// 读不出来 ⇒ `None`（**fail-closed**：没有读数就不能声称"我不是 root"）。
#[cfg(unix)]
pub fn effective_uid() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    parse_effective_uid(&status)
}

/// 非 Unix：没有这套判据（`sandbox` 档在这些平台上本来就不在场）。
#[cfg(not(unix))]
pub fn effective_uid() -> Option<u32> {
    None
}

/// 从 `/proc/self/status` 的正文里读出有效 uid。
///
/// 格式固定：`Uid:` 后面是四个数（真实 / 有效 / 保存 / 文件系统），空格或制表符分隔。**取第二格**——第一格是真实 uid，
/// 它可以与有效 uid 不同（setuid 程序的形状）；这条纪律要判的是"内核现在按谁给我权限"。
fn parse_effective_uid(status: &str) -> Option<u32> {
    let line = status
        .lines()
        .find(|line| line.starts_with("Uid:"))?
        .trim_start_matches("Uid:");
    line.split_whitespace().nth(1)?.parse().ok()
}

/// `sandbox` 档的**第一条**身份判据：有效 uid 必须读得出来、且不是 0。
///
/// 单独一格是给 [`choose_for_entry`] 用的：它要在**查后端表之前**就有结论——一台后端都
/// 没有时，操作者该看到的仍然是"你用错了身份"（如果确实是），而不是一句
/// "这台机器没有沙箱"（那会让他去装一个后端，而真正要动的是启动身份）。
fn uid_refusal() -> Result<(), SandboxIdentityFailure> {
    uid_refusal_for(effective_uid())
}

/// [`uid_refusal`] 的**判据本身**（uid 由调用方给，便于用例拿夹具 uid 判错）。
fn uid_refusal_for(uid: Option<u32>) -> Result<(), SandboxIdentityFailure> {
    let Some(uid) = uid else {
        return Err(SandboxIdentityFailure::new(
            "读不到本进程的有效 uid（/proc/self/status 的 `Uid:` 行）：\
             证明不了 `sandbox` 档的写边界靠的是别的身份——拒绝启动",
        ));
    };
    if uid == 0 {
        return Err(SandboxIdentityFailure::new(
            "不要用 root 跑 `terminal=sandbox`：这一档的写边界靠 uid（DAC），\
             root 无视它——`/etc` 可写、别人的 700 目录可读，沙箱等于没有装。\
             修：用住户身份（非 root 的专用用户）启动；`--terminal full` 不受这条限制，\
             但那一条是整机全权，不是沙箱",
        ));
    }
    Ok(())
}

/// 这个进程是不是以 root 在跑（**有效** uid 为 0）。
///
/// 读不到有效 uid 时回答 `true`：`sandbox` 档的写边界靠 uid，读不到 uid 就等于
/// "证明不了它不是 root"，那必须按最坏的那一格处理（拒绝，而不是放行）。
pub fn running_as_root() -> bool {
    effective_uid().map(|uid| uid == 0).unwrap_or(true)
}

/// **入口的身份判据**（`sandbox` 档专用；`full` 档不受它约束）。
///
/// 两条，都 fail-closed：
///
/// 1. **有效 uid 不能是 0**：root 无视 DAC，`/etc` 可写、别人的 700 目录可读——这一档的
///    写边界整个不成立。以 root 跑 `sandbox` 档 ⇒ 拒绝（不是"警告"）；
/// 2. **identity 后端要求环境根的 owner 是当前有效 uid**：它不开命名空间，写边界就是
///    "这个 uid 能写这个目录"。owner 不是自己时，那条边界要么不成立（写不进去 ⇒ 命令
///    全失败），要么成立得莫名其妙（它恰好在一个别人可写的目录里）——两种都不该被
///    读成"沙箱装好了"。**不 chown、不回退**：改别人的文件所有权是安装期的动作，
///    不是一次启动的副作用。
///
/// ⚠ 判据只对 `identity` 生效 owner 那一半：bwrap 自己 `--bind` 环境根，与 owner 无关。
pub fn check_entry_identity(
    environment_root: &Path,
    backend: &(dyn SandboxBackend + Send + Sync),
) -> Result<(), SandboxIdentityFailure> {
    uid_refusal()?;
    let Some(uid) = effective_uid() else {
        return Err(SandboxIdentityFailure::new(
            "读不到本进程的有效 uid：证明不了 identity 后端的写边界",
        ));
    };
    check_owner(uid, backend.name(), environment_root)
}

/// [`check_entry_identity`] 的**第二半**（owner 判据；uid 由调用方给）。
///
/// 只有 `identity` 要这一半：bwrap 自己 `--bind` 环境根，owner 与它无关。
fn check_owner(
    uid: u32,
    backend_name: &str,
    environment_root: &Path,
) -> Result<(), SandboxIdentityFailure> {
    if backend_name != IDENTITY_BACKEND {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(environment_root).map_err(|error| {
            SandboxIdentityFailure::new(format!(
                "读不到环境根 `{}` 的 owner（{error}）：identity 后端的写边界就是\
                 「这个 uid 能写这个目录」，读不出 owner 就不能声称它成立",
                environment_root.display()
            ))
        })?;
        let owner = metadata.uid();
        if owner != uid {
            return Err(SandboxIdentityFailure::new(format!(
                "环境根 `{}` 的 owner 是 {owner}，当前有效 uid 是 {uid}：identity 后端不建命名空间，\
                 它只兑现「owner 能写自己的环境根」这一条。修（二选一）：\
                 ① 以 uid {owner} 运行；② 安装期把环境根交给住户身份：\
                 `chown -R {uid}:{uid} {}`（本进程不会替你 chown，也不会换成别的后端）",
                environment_root.display(),
                environment_root.display()
            )));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = environment_root;
        return Err(SandboxIdentityFailure::new(
            "本平台没有 uid 判据：identity 后端不在这里",
        ));
    }
    Ok(())
}

/// 探针通过时的**证据**（不是「成功了」，是「看到了什么」）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeReading {
    /// 稳定名字（审计用）。
    pub backend: &'static str,
    /// 真跑出来的证据行（命名空间 / 只读系统 / 断网各一格）。
    pub evidence: String,
}

// ---------------------------------------------------------------------------
// 端口
// ---------------------------------------------------------------------------

/// **沙箱后端端口**：一个后端要回答的四件事。
pub trait SandboxBackend {
    /// 稳定名字（审计用；回执里写的就是它）。
    fn name(&self) -> &'static str;

    /// 这个后端**真的**兑现什么（见 [`Guarantees`]）。
    fn guarantees(&self) -> Guarantees;

    /// **这台机器上可用吗**——真跑一次，给出证据或原因。
    fn probe(&self) -> Result<ProbeReading, SandboxUnavailable>;

    /// 把一份请求拼成命令行（不执行、不读环境、不碰 stdio）。
    fn command(&self, request: &SandboxRequest<'_>) -> Result<SandboxCommand, SandboxRequestError>;
}

/// 内置后端表（**顺序就是优先级**）。
///
/// 判据是**强度**：能建 user namespace 的机器用 bwrap（命名空间 ＋ 只读系统）；建不了的
/// 机器退到 identity（DAC ＋ seccomp）。两支持续独立兑现 `sandbox` 档的**最低保证集**
/// （写不出系统 / 断网 / 凭据不进子进程），差别写在各自的 [`Guarantees`] 里。
///
/// ⚠ **不是「强的不行就随便换一个」**：表里每一项都必须**独立**过探针；过不了的不会因为
/// 前一项过不了而被"凑数"选上——[`choose`] 逐个真跑，报的是每一支自己的反例。
///
/// 环境根是**参数**而不是后端的私有状态：identity 的探针要证明"这个根里建得出文件"，
/// 那不是 `which bwrap` 能回答的事，也不该用一个别的目录代答。
pub fn built_in(environment_root: &Path) -> Vec<Box<dyn SandboxBackend + Send + Sync>> {
    #[allow(unused_mut)]
    let mut backends: Vec<Box<dyn SandboxBackend + Send + Sync>> =
        vec![Box::new(BwrapBackend::new())];
    #[cfg(target_os = "linux")]
    backends.push(Box::new(IdentityBackend::new(environment_root)));
    #[cfg(not(target_os = "linux"))]
    let _ = environment_root;
    backends
}

/// **解析器**：按表逐个探针，选第一支真能兑现的；一支都不行 ⇒ 拒绝（带上每支的原因）。
///
/// 判据一句话：**没有合格后端就拒绝，不静默退回宿主**（§7 的「不做静默降级」）。
/// ⚠ 它**不判身份**：那是 [`choose_for_entry`] 的事（两个 bin 必须走那一个）。
pub fn choose(
    environment_root: &Path,
) -> Result<Box<dyn SandboxBackend + Send + Sync>, SandboxUnavailable> {
    let mut refusals: Vec<String> = Vec::new();
    for backend in built_in(environment_root) {
        match backend.probe() {
            Ok(_) => return Ok(backend),
            Err(unavailable) => {
                refusals.push(format!("{}: {}", backend.name(), unavailable.detail()))
            }
        }
    }
    Err(SandboxUnavailable::new(if refusals.is_empty() {
        "没有任何内置沙箱后端".to_string()
    } else {
        refusals.join("；")
    }))
}

/// 入口拿到的**两种拒绝之一**（两类失败，两个稳定词）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryRefusal {
    /// 这台机器上没有能兑现的后端（`sandbox-unavailable`）。
    Unavailable(SandboxUnavailable),
    /// 机器能，但身份不对（`sandbox-identity-mismatch`）。
    Identity(SandboxIdentityFailure),
}

impl EntryRefusal {
    /// 稳定分类标记（入口那一行必须带上它）。
    pub fn as_str(&self) -> &'static str {
        match self {
            EntryRefusal::Unavailable(unavailable) => unavailable.as_str(),
            EntryRefusal::Identity(mismatch) => mismatch.as_str(),
        }
    }

    /// 人话原因。
    pub fn detail(&self) -> &str {
        match self {
            EntryRefusal::Unavailable(unavailable) => unavailable.detail(),
            EntryRefusal::Identity(mismatch) => mismatch.detail(),
        }
    }

    /// 一整行：`<稳定词>：<原因>`。
    pub fn message(&self) -> String {
        match self {
            EntryRefusal::Unavailable(unavailable) => unavailable.message(),
            EntryRefusal::Identity(mismatch) => mismatch.message(),
        }
    }
}

/// **入口唯一该用的解析器**：先判身份、再选后端、最后判"选中的那一支与这个身份合不合"。
///
/// 顺序是语义：**身份错在"没有后端"之前**报。两者都在时，操作者先该修的是身份（那是他
/// 自己就能改的），而不是去装一个后端；反过来报会把人引到错误的方向。
///
/// `full` 档**不经过这一件**：它不承诺写边界，也就不受 uid 纪律约束。
pub fn choose_for_entry(
    environment_root: &Path,
) -> Result<Box<dyn SandboxBackend + Send + Sync>, EntryRefusal> {
    uid_refusal().map_err(EntryRefusal::Identity)?;
    let backend = choose(environment_root).map_err(EntryRefusal::Unavailable)?;
    check_entry_identity(environment_root, backend.as_ref()).map_err(EntryRefusal::Identity)?;
    Ok(backend)
}

// ---------------------------------------------------------------------------
// 路径判据：**工作目录必须在环境根之内**
// ---------------------------------------------------------------------------

/// 这个目录是不是**在**环境根之内（含根本身）。
///
/// 判据用 `canonicalize` 两边再按**路径组件**比较，因此 `..` 与符号链接都绕不出去
/// （`Path::starts_with` 是按组件比的，不是字符串前缀——`/a/bc` 不算在 `/a/b` 里）。
/// 读不出来（不存在、权限、环）⇒ **不是**在根里：**fail-closed**。
///
/// ⚠ 它**不是**安全边界本身：真正的边界是 `--ro-bind / /` ＋ `--bind <根> <根>`
/// （根外面根本没挂上可写的面）。这里判的是「这次请求是不是自相矛盾」——
/// 越界的 `cwd` 是一句立刻就能说清的 `invalid-argument`，不该变成一次跑起来才失败的命令。
pub fn within_workspace(environment_root: &Path, candidate: &Path) -> bool {
    let (Ok(root), Ok(target)) = (
        std::fs::canonicalize(environment_root),
        std::fs::canonicalize(candidate),
    ) else {
        return false;
    };
    target.starts_with(&root)
}

// ---------------------------------------------------------------------------
// bwrap（第一个实现）
// ---------------------------------------------------------------------------

/// **Linux 的第一个沙箱后端**：bubblewrap。
///
/// 它做的事就是裁决文档 §5 那条链的第一格：`bwrap` 可用 ⇒ 用它；不可用 ⇒
/// [`SandboxUnavailable`]。**没有第二格**（systemd-run 不是沙箱，见模块头）。
pub struct BwrapBackend {
    /// 程序名或路径（缺省 `bwrap`；用例用它指一个不存在的程序，钉住「没有就是拒绝」）。
    program: String,
}

impl Default for BwrapBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl BwrapBackend {
    /// 用 `PATH` 上的 `bwrap`。
    pub fn new() -> Self {
        Self {
            program: BWRAP_PROGRAM.to_string(),
        }
    }

    /// 指定程序（测试缝：探针「找不到程序」那条路要靠它，别去改 `PATH`）。
    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }

    /// **隔离参数**：探针与真跑**共用同一份**。
    ///
    /// 共用是判据，不是省事：探针证明的必须是**真跑时那一套**参数（否则探针就是在证明
    /// 另一件事）。三样东西在这里一次装好——文件面（系统只读、环境根可写）、
    /// 四个命名空间、以及寿命（`--die-with-parent` ＋ `--new-session`）。
    ///
    /// ⚠ 挂载顺序是语义：`--ro-bind / /` 先把整个系统按只读铺上，`/dev`、`/proc`、`/tmp`
    /// 逐个盖上去（`--tmpfs /tmp` ⇒ **`/tmp` 是可写的、但只是 tmpfs**，关家就没），
    /// 最后 `--bind <根> <根>` 才把**唯一那一块可写面**盖回来。
    fn isolation_args(environment_root: Option<&Path>) -> Vec<String> {
        let mut args: Vec<String> = [
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--tmpfs",
            "/tmp",
        ]
        .iter()
        .map(|each| each.to_string())
        .collect();
        if let Some(root) = environment_root {
            let root = root.to_string_lossy().to_string();
            args.extend(["--bind".to_string(), root.clone(), root]);
        }
        args.extend(
            [
                "--unshare-net",
                "--unshare-pid",
                "--unshare-ipc",
                "--unshare-uts",
                "--die-with-parent",
                "--new-session",
            ]
            .iter()
            .map(|each| each.to_string()),
        );
        args
    }

    /// **探针脚本**：一次真跑，四类证据。
    ///
    /// 它不是 `/bin/true`：只看退出码等于「什么都没证明」——一个把 `--unshare-net` 悄悄
    /// 忽略掉的包装器同样能返回 0。因此脚本在**沙箱里面**逐条量：
    ///
    /// 1. **命名空间**：`/proc/self/ns/{mnt,pid,net,ipc,uts}` 的标识必须与本进程的**不同**
    ///    （宿主侧的值由探针通过环境交进去；相同 ⇒ 那个命名空间根本没建）；
    /// 2. **只读系统**：在 `/etc` 下新建文件**必须失败**（`--ro-bind / /` 真的生效了）；
    /// 3. **断网**：新的 netns 里 `/proc/net/dev` 只有 `lo`，而且 `/proc/net/route` 没有路由
    ///    （`--unshare-net` 真的生效了——不是「有个空网卡」而是「没有别的网」）；
    /// 4. **自己的可写面**：`/tmp`（tmpfs）写得进去（挂载真的做上了）。
    ///
    /// ⚠ 两处"试写"都套在**子 shell** `( … )` 里，不是 `{ …; }`：POSIX 规定**特殊内建**
    /// （`:`）的重定向失败会让**非交互 shell 直接退出**——本机 dash 的表现是**静默 exit 2**，
    /// 于是"系统只读"会被误读成"探针崩了"（2026-10-23 真机踩到：整段脚本在第 6 行无声结束）。
    /// 子 shell 把那次退出关在里面，`if` 照常拿到非零状态。
    ///
    /// ⚠ 用 `/proc/net/*` 而不是 `/sys/class/net`：`/sys` 是从宿主只读绑进来的，
    /// 它给的是**宿主**的网卡表——在那里数网卡会永远数出宿主的来（判据写错的经典形状）。
    const PROBE_SCRIPT: &'static str = r#"set -u
bad=0
[ "$(readlink /proc/self/ns/mnt)" != "$SANDBOX_PROBE_HOST_MNT" ] || { echo "namespace-mnt=host"; bad=1; }
[ "$(readlink /proc/self/ns/pid)" != "$SANDBOX_PROBE_HOST_PID" ] || { echo "namespace-pid=host"; bad=1; }
[ "$(readlink /proc/self/ns/net)" != "$SANDBOX_PROBE_HOST_NET" ] || { echo "namespace-net=host"; bad=1; }
[ "$(readlink /proc/self/ns/ipc)" != "$SANDBOX_PROBE_HOST_IPC" ] || { echo "namespace-ipc=host"; bad=1; }
[ "$(readlink /proc/self/ns/uts)" != "$SANDBOX_PROBE_HOST_UTS" ] || { echo "namespace-uts=host"; bad=1; }
if [ "$bad" = 0 ]; then echo "namespaces=isolated"; fi
if ( : > /etc/.celira-sandbox-probe ) 2>/dev/null; then echo "system=writable"; bad=1; else echo "system=read-only"; fi
interfaces=$(sed -n '3,$s/^ *\([^:]*\):.*/\1/p' /proc/net/dev | tr '\n' ',')
[ "$interfaces" = "lo," ] || { echo "network-interfaces=$interfaces"; bad=1; }
routes=$(sed '1d' /proc/net/route | wc -l | tr -d ' ')
[ "$routes" = "0" ] || { echo "network-routes=$routes"; bad=1; }
if ( : > /tmp/.celira-sandbox-probe ) 2>/dev/null; then echo "tmpfs=writable"; else echo "tmpfs=read-only"; bad=1; fi
echo "probe=$([ "$bad" = 0 ] && echo ok || echo failed)"
[ "$bad" = 0 ]
"#;
}

impl SandboxBackend for BwrapBackend {
    fn name(&self) -> &'static str {
        BWRAP_BACKEND
    }

    /// bwrap 兑现的八格。
    ///
    /// `resources=none` 与 `network=allowlist` 那一格**今天没有**（见模块头的负空间）——
    /// 照实写出来，比写一个「尽力」有用：选后端的人要能看出缺口在哪。
    fn guarantees(&self) -> Guarantees {
        Guarantees::of(vec![
            ("files", "system-read-only,workspace-writable"),
            ("network", "none"),
            ("pid", "namespace"),
            ("ipc", "namespace"),
            ("uts", "namespace"),
            // 洗环境**不是**后端做的：那是编排层的五堵墙之一（与 `full` 共用同一份），
            // 后端只是原样把它交下去。列在这里是因为它属于「这一档兑现了什么」。
            ("environment", "scrubbed-by-layer"),
            ("resources", "none"),
            ("workspace", "persistent-bind"),
        ])
    }

    fn probe(&self) -> Result<ProbeReading, SandboxUnavailable> {
        let program = resolve_program(&self.program).ok_or_else(|| {
            SandboxUnavailable::new(format!(
                "找不到 `{}`（`PATH` 上没有可执行的 bubblewrap；Linux 沙箱后端需要它）",
                self.program
            ))
        })?;

        // 宿主侧的命名空间标识：**由我们在外面读一次**，再交进沙箱里比。
        let mut host: Vec<(&'static str, String)> = Vec::new();
        for name in ["mnt", "pid", "net", "ipc", "uts"] {
            let value = std::fs::read_link(format!("/proc/self/ns/{name}"))
                .map(|path| path.to_string_lossy().replace(['[', ']'], ""))
                .map_err(|error| {
                    SandboxUnavailable::new(format!(
                        "读不到本进程的 `{name}` 命名空间标识（{error}）：没法证明新命名空间真的建了"
                    ))
                })?;
            host.push((name, value));
        }

        let mut command = Command::new(&program);
        command.args(Self::isolation_args(None));
        command.arg("--chdir").arg("/");
        command.arg("--").arg("/bin/sh").arg("-c");
        command.arg(Self::PROBE_SCRIPT);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // 探针**自己**的环境：一份都不继承（它不需要调用方的任何东西，也就不该有）。
        // bwrap 把这份环境原样交给沙箱里的 `sh`——三组 `SANDBOX_PROBE_HOST_*` 正是脚本要的。
        command.env_clear();
        command.env("PATH", PROBE_PATH);
        for (name, value) in &host {
            command.env(format!("SANDBOX_PROBE_HOST_{}", name.to_uppercase()), value);
        }

        let child = command.spawn().map_err(|error| {
            SandboxUnavailable::new(format!(
                "bwrap 起不来（{error}）：这一档需要非特权 user namespace"
            ))
        })?;
        let output = read_probe(child, PROBE_TIMEOUT)?;
        let status = output.status;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !status.success() {
            return Err(SandboxUnavailable::new(format!(
                "没能建立隔离（退出码 {}）：{}",
                exit_reading(status),
                first_line(&stderr).unwrap_or_else(
                    || first_line(&stdout).unwrap_or_else(|| "没有任何输出".to_string())
                )
            )));
        }
        // 退出码为 0 还不算数：**要看到证据**（脚本自己会把每一格打出来）。
        let evidence = stdout
            .lines()
            .filter(|line| line.contains('='))
            .collect::<Vec<_>>()
            .join(" ");
        for required in [
            "namespace-mnt=",
            "namespace-pid=",
            "namespace-net=",
            "namespace-ipc=",
            "namespace-uts=",
            "system=writable",
            "network-interfaces=",
            "network-routes=",
            "probe=failed",
        ] {
            if stdout.contains(required) {
                return Err(SandboxUnavailable::new(format!(
                    "探针在沙箱里量到反例（{required}）：{}",
                    evidence
                )));
            }
        }
        for required in [
            "system=read-only",
            "tmpfs=writable",
            "probe=ok",
            "namespaces=isolated",
        ] {
            if !stdout.contains(required) {
                return Err(SandboxUnavailable::new(format!(
                    "探针没给出证据 `{required}`：{}",
                    if evidence.is_empty() {
                        "没有任何输出".to_string()
                    } else {
                        evidence.clone()
                    }
                )));
            }
        }
        Ok(ProbeReading {
            backend: self.name(),
            evidence,
        })
    }

    fn command(&self, request: &SandboxRequest<'_>) -> Result<SandboxCommand, SandboxRequestError> {
        if !request.environment_root.is_dir() {
            return Err(SandboxRequestError::WorkspaceUnavailable);
        }
        if !within_workspace(request.environment_root, request.cwd) {
            return Err(SandboxRequestError::CwdOutsideWorkspace);
        }
        let mut args = Self::isolation_args(Some(request.environment_root));
        args.push("--chdir".to_string());
        args.push(request.cwd.to_string_lossy().to_string());
        args.push("--".to_string());
        args.push(request.shell.to_string());
        args.push("-c".to_string());
        args.push(request.command.to_string());
        Ok(SandboxCommand::new(self.program.clone(), args))
    }
}

// ---------------------------------------------------------------------------
// identity（第二支实现）：uid ＋ seccomp，不建任何命名空间
// ---------------------------------------------------------------------------

/// **不建 user namespace 的沙箱后端**：写面靠 uid（DAC），断网靠 seccomp。
///
/// 它存在的原因是**代价**，不是"想多一支实现"：主流发行版关掉无特权 user namespace 之后，
/// bwrap 在真机上起不来（本机实测 `loopback: Failed RTM_NEWADDR: Operation not permitted`），
/// 而"要沙箱就先放宽一个全局 sysctl"不是一个可以替用户做的决定。这一支换掉了那个前提：
///
/// - **写面**：以非 root 的专用 uid 运行 ⇒ `/etc` 与别人的 700 目录由内核权限位挡住。
///   ⚠ 它**不是挂载隔离**：那个 uid 写得到的地方照旧写得到（家目录、仓库、`/tmp`）；
/// - **断网**：子进程装 seccomp，`socket(AF_INET)` / `socket(AF_INET6)` 返回 `EACCES`，
///   `io_uring_setup` 一并拒（否则可以绕开 `socket`）。⚠ 它**不是 netns**：`AF_UNIX` 照旧通；
/// - **句柄**：`close_range(3, ~0U, CLOEXEC)`，调用方的已连接套接字进不来（`execve` 之后
///   一个继承 fd 都不剩）。
///
/// 三件事**都不是**这一件自己做的：写面靠的是"谁在跑"，断网与句柄靠的是
/// `crate::seccomp`（编排层在 `execve` 前装）。这一件只回答"这台机器上三件事成不成立"
/// （[`probe`](SandboxBackend::probe)）与"命令行怎么拼"（[`command`](SandboxBackend::command)）。
///
/// ## 探针为什么必须拿着环境根
///
/// "环境根里建得出文件"是这一支的**保证之一**，不是一个假设：owner 不是当前 uid、
/// 目录只读、或者根根本不在，都会让这条保证在真跑时变成"每条命令都失败"。因此
/// [`probe`](SandboxBackend::probe) 要一个**真的**环境根，并**在里面真写一个文件**
/// （写完删掉）——只看 `which` 或者只看 owner 都是把"大概能写"当成"写过了"。
///
/// ⚠ **这个根必须已经存在**：探针证明的是"这个目录可写"，不是一个"将来会被创建"的目录。
/// `agent_host` 先开环境、再解析 terminal，不受影响；`agent_run` 的一次性入口在根还没
/// 创建时会如实拒绝——先建环境，再开这一档（顺序本身就是"先有家、再有手"）。
#[cfg(target_os = "linux")]
pub struct IdentityBackend {
    /// 唯一可写的那一块（真跑时也由编排层交给 `current_dir`，这里只用于探针与 owner 判据）。
    environment_root: PathBuf,
}

#[cfg(target_os = "linux")]
impl IdentityBackend {
    /// 用给定的环境根建一份。
    pub fn new(environment_root: impl Into<PathBuf>) -> Self {
        Self {
            environment_root: environment_root.into(),
        }
    }

    /// 探针要验的那个根（诊断/回执用）。
    pub fn environment_root(&self) -> &Path {
        &self.environment_root
    }

    /// 探针用的 shell：`/bin/bash`，回退 `PATH` 上的 `bash`。
    ///
    /// 为什么必须是 bash：判据是 `/dev/tcp`，那是 bash 的内建重定向（dash/sh 没有）。
    /// 没有它 ⇒ **不可用**并说清原因——不是"换个判据"，因为换出来的判据证明不了断网。
    fn probe_shell() -> Option<PathBuf> {
        let absolute = Path::new("/bin/bash");
        if is_executable(absolute) {
            return Some(absolute.to_path_buf());
        }
        resolve_program("bash")
    }

    /// 起一条**装好 confinement** 的探针子进程（与真跑同一条 spawn 路径）。
    ///
    /// 同一路径是判据，不是省事：探针证明的必须是**真跑时那一套**（seccomp ＋ `close_range`
    /// ＋ 清空环境）。否则它就是在一台"没有过滤器"的机器上证明"过滤器有效"。
    fn run_confined(
        &self,
        script: &str,
        environment: &[(&str, String)],
    ) -> Result<std::process::Output, SandboxUnavailable> {
        let shell = Self::probe_shell().ok_or_else(|| {
            SandboxUnavailable::new(
                "找不到 /bin/bash（也没有 PATH 上的 bash）：断网判据用的是它的 `/dev/tcp` 内建，\
                 没有它就证明不了过滤器生效",
            )
        })?;
        let args = vec!["-c".to_string(), script.to_string()];
        let mut builder = crate::platform::argv_command(shell.to_string_lossy().as_ref(), &args);
        builder
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // 探针自己**不继承**调用方的环境（它不需要任何东西；也就不该有）。
        builder.env_clear();
        builder.env("PATH", PROBE_PATH);
        builder.env("LC_ALL", "C");
        for (key, value) in environment {
            builder.env(key, value);
        }
        crate::platform::confine_command(&mut builder, &self.environment_root).map_err(
            |reason| SandboxUnavailable::new(format!("identity 探针装不上 confinement：{reason}")),
        )?;
        let child = builder
            .spawn()
            .map_err(|error| SandboxUnavailable::new(format!("identity 探针起不来（{error}）")))?;
        read_probe(child, PROBE_TIMEOUT)
    }

    /// 四格证据（每一格都是一次真跑，不是一次推理）。
    ///
    /// 顺序是**从便宜到贵**，也是从"环境"到"命令"：uid → 环境根 → `/etc` → 断网 → 句柄。
    /// 每一格失败都带上**那一格的原始输出**，让读的人自己判断是哪一半没成立。
    fn measure(&self) -> Result<String, SandboxUnavailable> {
        let uid = effective_uid().ok_or_else(|| {
            SandboxUnavailable::new("读不到本进程的有效 uid（/proc/self/status 的 `Uid:` 行）")
        })?;
        if uid == 0 {
            return Err(SandboxUnavailable::new(
                "以 root 运行：identity 后端的写边界靠 Landlock 与 DAC，root 可以撤销它\
                 （`/etc` 可写、别人的 700 目录可读）——这一支不成立",
            ));
        }
        if crate::landlock::abi().is_none() {
            return Err(SandboxUnavailable::new(
                "内核没有 Landlock（< 5.13 或未编入）：identity 后端的写边界只能退回 uid 的 DAC，\
                 而 DAC 挡不住同一个 uid 里的写入（实测：住户能在主人的家里造兄弟目录）",
            ));
        }
        if !self.environment_root.is_dir() {
            return Err(SandboxUnavailable::new(format!(
                "环境根 `{}` 不是一个目录：identity 后端的可写面就是这个目录",
                self.environment_root.display()
            )));
        }

        let tag = std::process::id();
        // ① 环境根里建得出文件（写完立刻删掉：探针不许留下任何东西）。
        let workspace_probe = self
            .environment_root
            .join(format!(".celira-identity-probe-{tag}"));
        let workspace = self.run_confined(
            ": > \"$CELIRA_IDENTITY_WORKSPACE\"",
            &[(
                "CELIRA_IDENTITY_WORKSPACE",
                workspace_probe.to_string_lossy().to_string(),
            )],
        )?;
        // ⚠ 先看它**在不在**，再删：反过来的话，"建出来了"这件事会被自己的清理抹掉，
        // 探针就变成了一条永远失败的用例（2026-10-23 本机踩到）。
        let created = workspace_probe.exists();
        let _ = std::fs::remove_file(&workspace_probe);
        if !workspace.status.success() || !created {
            return Err(SandboxUnavailable::new(format!(
                "环境根 `{}` 里建不出文件（退出码 {}）：{}",
                self.environment_root.display(),
                exit_reading(workspace.status),
                first_line(&String::from_utf8_lossy(&workspace.stderr))
                    .unwrap_or_else(|| "没有任何输出".to_string())
            )));
        }

        // ② 系统面：`/etc` 下必须建不出文件（DAC 真的挡住了这个 uid）。
        let system_probe = PathBuf::from(format!("/etc/.celira-identity-probe-{tag}"));
        let system = self.run_confined(
            ": > \"$CELIRA_IDENTITY_SYSTEM\"",
            &[(
                "CELIRA_IDENTITY_SYSTEM",
                system_probe.to_string_lossy().to_string(),
            )],
        )?;
        if system.status.success() {
            // 写成功了：这个 uid 不受 DAC 约束（多半是 root 的某个变体）。别留垃圾。
            let _ = std::fs::remove_file(&system_probe);
            return Err(SandboxUnavailable::new(format!(
                "`/etc` 下写得进去（{}）：这个 uid 不是受 DAC 约束的身份，\
                 identity 后端的写边界不成立",
                system_probe.display()
            )));
        }

        // ③ 写边界：**环境根的父目录**里建不出东西（Landlock 真的把"写入"关进了根）。
        //
        // 这一格是同 uid 部署与专用 uid 部署**都**要过的：前者靠 Landlock，后者两者都有。
        // 它证明的不是"系统只读"（那一格是 `/etc`），而是"**造不出一个别人的家**"。
        let parent = match self.environment_root.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => {
                return Err(SandboxUnavailable::new(
                    "环境根没有父目录（它是根目录？）：写边界没有可判的「外面」",
                ));
            }
        };
        let sibling = parent.join(format!(".celira-identity-sibling-{tag}"));
        let outside = self.run_confined(
            ": > \"$CELIRA_IDENTITY_SIBLING\"",
            &[(
                "CELIRA_IDENTITY_SIBLING",
                sibling.to_string_lossy().to_string(),
            )],
        )?;
        let escaped = outside.status.success();
        let _ = std::fs::remove_file(&sibling);
        if escaped {
            return Err(SandboxUnavailable::new(format!(
                "环境根的父目录 `{}` 里写得进去：Landlock 的写边界没有生效",
                parent.display()
            )));
        }

        // ④ 断网：一条**真子进程**连不出去，而且拒绝的理由必须是"没有权限"或"没有路由"。
        let network = self.run_confined("exec 3<>/dev/tcp/1.1.1.1/443", &[])?;
        let network_stderr = String::from_utf8_lossy(&network.stderr).to_string();
        if network.status.success() {
            return Err(SandboxUnavailable::new(
                "装了 seccomp 之后仍然连得上 1.1.1.1:443：过滤器没有生效\
                 （这一支的断网承诺不成立）",
            ));
        }
        let network_denied = network_stderr.contains("Permission denied")
            || network_stderr.contains("Network is unreachable");
        if !network_denied {
            return Err(SandboxUnavailable::new(format!(
                "连不出去，但理由不是过滤器或本机离线（退出码 {}）：{}",
                exit_reading(network.status),
                first_line(&network_stderr).unwrap_or_else(|| "没有任何输出".to_string())
            )));
        }
        let network_reading = if network_stderr.contains("Permission denied") {
            "denied"
        } else {
            "unreachable"
        };

        // ④ 继承的 fd：子进程只该看见 0/1/2（＋它自己读目录那一格）。
        let fds = self.run_confined("cd /proc/self/fd && echo *", &[])?;
        let listing = String::from_utf8_lossy(&fds.stdout).trim().to_string();
        let leaked: Vec<&str> = listing
            .split_whitespace()
            .filter(|field| field.parse::<u32>().map(|fd| fd > 3).unwrap_or(true))
            .collect();
        if !fds.status.success() || !leaked.is_empty() {
            return Err(SandboxUnavailable::new(format!(
                "子进程看得见继承的句柄（{}）：`close_range` 没有生效",
                if listing.is_empty() {
                    "没有任何输出".to_string()
                } else {
                    listing.clone()
                }
            )));
        }

        Ok(format!(
            "uid={uid} workspace=writable system=read-only outside=denied \
             network={network_reading} fds=0,1,2 (+reader)"
        ))
    }
}

#[cfg(target_os = "linux")]
impl SandboxBackend for IdentityBackend {
    fn name(&self) -> &'static str {
        IDENTITY_BACKEND
    }

    /// identity 兑现的八格（**照实降级**：没有命名空间、不限资源）。
    ///
    /// `network=none-ip` 不是 `none` 的同义词：它只覆盖 IP 家族（`AF_UNIX` 照旧通）。
    /// `files=workspace-writable,landlock` 与 bwrap 的 `system-read-only,workspace-writable`
    /// **同向但不同机制**：写边界由 Landlock 把"写入"关进环境根（不靠挂载、不靠 uid），
    /// 读仍是全系统可读——这一格不承诺保密。
    fn guarantees(&self) -> Guarantees {
        Guarantees::of(vec![
            ("files", "workspace-writable,landlock"),
            ("network", "none-ip"),
            ("pid", "none"),
            ("ipc", "none"),
            ("uts", "none"),
            ("environment", "scrubbed-by-layer"),
            ("resources", "none"),
            ("workspace", "persistent"),
        ])
    }

    fn probe(&self) -> Result<ProbeReading, SandboxUnavailable> {
        let evidence = self.measure()?;
        Ok(ProbeReading {
            backend: self.name(),
            evidence,
        })
    }

    /// 命令行：`<shell> -c <命令原文>`，**不包 bwrap**。
    ///
    /// 三格与 bwrap 那一支对齐、两格不同：
    ///
    /// - 程序是请求给的 shell（`bash` 在 Linux 上），**不是** `bwrap`；
    /// - `confinement = SeccompNoIp`：编排层必须在 `execve` 前装过滤器 ＋ 清 fd；
    /// - `working_directory`：没有 `--chdir`，工作目录由编排层设进程自己的 cwd；
    /// - 环境根**不进命令行**（写面靠 uid，不靠 bind）；
    /// - 越界的 `cwd` / 不存在的根：**同一份拒绝**（与 bwrap 逐条同源）。
    fn command(&self, request: &SandboxRequest<'_>) -> Result<SandboxCommand, SandboxRequestError> {
        if !request.environment_root.is_dir() {
            return Err(SandboxRequestError::WorkspaceUnavailable);
        }
        if !within_workspace(request.environment_root, request.cwd) {
            return Err(SandboxRequestError::CwdOutsideWorkspace);
        }
        // 临时区：Landlock 把 `/tmp` 也关上了，命令一用临时文件就失败。给一个**根内**的固定
        // 目录，并让编排层把它设成 `TMPDIR`——写进那儿的一律还在环境根里，边界不动。
        let temp = request.environment_root.join(IDENTITY_TEMP);
        let _ = std::fs::create_dir_all(&temp);
        Ok(SandboxCommand::confined(
            request.shell.to_string(),
            vec!["-c".to_string(), request.command.to_string()],
            Confinement::SeccompNoIp,
        )
        .with_working_directory(request.cwd.to_path_buf())
        .with_temp_directory(temp))
    }
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/// 等一个探针子进程收场（**有上限**），把两条流一起读回来。
///
/// 两个后端的探针共用这一份：bwrap 的探针在沙箱里量，identity 的探针在装了 seccomp 的
/// 子进程里量——"怎么等"不该因此分成两份（两份就会有一天只在一份上修好超时）。
///
/// 超时 ⇒ **按进程组杀整棵树**（探针会起子进程：bwrap 里的 `sh`、identity 里的 `bash`
/// 派生），杀完再等一次直接子进程；报的原因带上限值，不猜"它可能只是慢"。
fn read_probe(
    mut child: std::process::Child,
    timeout: Duration,
) -> Result<std::process::Output, SandboxUnavailable> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(error) => {
                return Err(SandboxUnavailable::new(format!(
                    "探针子进程等不到退出（{error}）"
                )));
            }
        }
        if Instant::now() >= deadline {
            crate::platform::kill_tree(child.id());
            let _ = child.kill();
            let _ = child.wait();
            return Err(SandboxUnavailable::new(format!(
                "探针 {} 秒内没有结束",
                timeout.as_secs()
            )));
        }
        std::thread::sleep(PROBE_POLL);
    }
    child
        .wait_with_output()
        .map_err(|error| SandboxUnavailable::new(format!("读不到探针的输出（{error}）")))
}

/// 在 `PATH` 上找一个可执行的程序（绝对路径直接看它自己）。
///
/// 判据是**能执行**（文件 ＋ 任一执行位），不是「名字出现过」——`bwrap` 是一个空文件、
/// 或者一个没有执行位的脚本，起不来就是起不来。
fn resolve_program(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        return is_executable(Path::new(program)).then(|| PathBuf::from(program));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(program))
        .find(|candidate| is_executable(candidate))
}

/// 这个路径是一个能执行的文件吗。
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// 没有 `unix` 的平台：沙箱后端在这类平台上本来就不在场（`TerminalTier::available` 为假），
/// 这里如实回答「没有」。
#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    false
}

/// 一段输出里第一行非空的内容（错误原因就在那儿）。
fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// 退出读数（与 `terminal_app` 的那一格同一条判据：读得出来就照报，读不出来就说 `unknown`）。
fn exit_reading(status: std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => code.to_string(),
        None => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **端口的数据形状**：argv 里有那十条，一块不多、一块不少。
    ///
    /// 判据是**顺序**也照钉：`--ro-bind / /` 必须在 `--bind <根> <根>` 之前
    /// （反过来的话环境根会被只读的那一层盖住，命令一写就失败——而那是运行期才看得见的错）。
    #[test]
    fn the_bwrap_plan_pins_the_required_isolation_flags() {
        let root = std::env::temp_dir().join(format!("celira-sbx-plan-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建临时环境根");
        let request = SandboxRequest::new(&root, &root, "echo hi", "bash");
        let plan = BwrapBackend::new().command(&request).expect("拼得出命令行");
        assert_eq!(plan.program(), "bwrap");
        let args: Vec<&str> = plan.args().iter().map(String::as_str).collect();
        for required in [
            "--ro-bind",
            "--dev",
            "--proc",
            "--tmpfs",
            "--bind",
            "--unshare-net",
            "--unshare-pid",
            "--unshare-ipc",
            "--unshare-uts",
            "--die-with-parent",
            "--new-session",
            "--chdir",
        ] {
            assert!(
                args.contains(&required),
                "argv 里少了 `{required}`：{args:?}"
            );
        }
        let ro = args
            .iter()
            .position(|each| *each == "--ro-bind")
            .expect("有");
        let bind = args.iter().position(|each| *each == "--bind").expect("有");
        assert!(
            ro < bind,
            "只读那一层必须先铺、可写那一块必须后盖：{args:?}"
        );
        assert_eq!(
            &args[args.len() - 4..],
            ["--", "bash", "-c", "echo hi"],
            "命令行尾巴必须是 `<-- 分隔> <shell> -c <命令原文>`：{args:?}"
        );
    }

    /// **越界的 `cwd` 是本地拒绝**：它不能变成一次跑起来才失败的命令。
    #[test]
    fn a_cwd_outside_the_workspace_is_refused_not_run() {
        let root = std::env::temp_dir().join(format!("celira-sbx-unit-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建临时环境根");
        let outside =
            std::env::temp_dir().join(format!("celira-sbx-outside-{}", std::process::id()));
        let request = SandboxRequest::new(&root, &outside, "true", "bash");
        assert_eq!(
            BwrapBackend::new().command(&request).err(),
            Some(SandboxRequestError::CwdOutsideWorkspace)
        );
        // 根自己**在**根里（缺省 cwd 就是这个地址）。
        assert!(within_workspace(&root, &root));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 保证集是**可读的封闭取值**：缺口（`resources=none`）必须写得出来。
    #[test]
    fn the_guarantees_say_what_is_missing_too() {
        let guarantees = BwrapBackend::new().guarantees();
        assert_eq!(guarantees.value("network"), Some("none"));
        assert_eq!(guarantees.value("pid"), Some("namespace"));
        assert_eq!(
            guarantees.value("files"),
            Some("system-read-only,workspace-writable")
        );
        // 如实登记的缺口：v1 不限资源。
        assert_eq!(guarantees.value("resources"), Some("none"));
        assert!(guarantees.reading().contains("network=none"));
        assert_eq!(guarantees.value("没有这一格"), None);
    }

    /// **非 root 判据**：`Uid:` 行取**第二格**（有效 uid），root 与读不到都拒绝。
    #[test]
    fn the_effective_uid_reading_and_the_root_refusal() {
        assert_eq!(
            parse_effective_uid("Name:\tbash\nUid:\t1000\t1000\t1000\t1000\n"),
            Some(1000)
        );
        // setuid 的形状：真实 uid 与有效 uid 不同 ⇒ 判据取**有效**那一个。
        assert_eq!(parse_effective_uid("Uid:\t1000\t0\t0\t0\n"), Some(0));
        assert_eq!(parse_effective_uid("Name:\tbash\n"), None);
        assert_eq!(parse_effective_uid("Uid:\tnot-a-number\n"), None);

        let uid = effective_uid().expect("Linux 上读得到有效 uid");
        assert_eq!(running_as_root(), uid == 0, "判据必须是 uid == 0");
        assert_ne!(uid, 0, "这条用例的前提：测试进程不是以 root 跑");

        // root 夹具：必须拒绝，而且原因说的是「不要用 root」，不是「没有后端」。
        let refusal = uid_refusal_for(Some(0)).expect_err("root 必须拒绝");
        assert_eq!(refusal.as_str(), SANDBOX_IDENTITY_MISMATCH);
        assert!(refusal.detail().contains("root"), "{}", refusal.detail());
        assert!(!refusal.message().contains(SANDBOX_UNAVAILABLE));
        // 读不到 uid ⇒ fail-closed。
        assert!(uid_refusal_for(None).is_err());
        // 非 root ⇒ 通过。
        assert!(uid_refusal_for(Some(1000)).is_ok());
    }

    /// **owner 判据**：identity 要求环境根属于当前 uid；bwrap 不看这一格。
    #[test]
    fn an_identity_root_must_be_owned_by_the_running_uid() {
        let root = std::env::temp_dir().join(format!("celira-sbx-owner-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建临时环境根");
        let uid = effective_uid().expect("读得到 uid");

        // 自己的根：通过（真读 metadata.uid()）。
        assert!(check_owner(uid, IDENTITY_BACKEND, &root).is_ok());
        // 换一个 uid 判：owner 不匹配 ⇒ 拒绝，且给得出 chown 修复行（不替人 chown）。
        let failure = check_owner(uid.wrapping_add(1), IDENTITY_BACKEND, &root)
            .expect_err("owner 不同必须拒绝");
        assert_eq!(failure.as_str(), SANDBOX_IDENTITY_MISMATCH);
        assert!(
            failure.detail().contains("chown -R"),
            "拒绝要给一条可执行的修复命令：{}",
            failure.detail()
        );
        assert!(
            !failure.message().contains(SANDBOX_UNAVAILABLE),
            "身份不对不许说成「没有后端」：{}",
            failure.message()
        );
        // bwrap **不看** owner：同一个目录、同一个「错」uid 也通过（它自己 --bind）。
        assert!(check_owner(uid.wrapping_add(1), BWRAP_BACKEND, &root).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **内置表顺序**：bwrap 在前（能建 userns 的机器用更强的），identity 在后。
    #[test]
    fn the_built_in_table_puts_the_stronger_backend_first() {
        let root = std::env::temp_dir().join(format!("celira-sbx-order-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建临时环境根");
        let table = built_in(&root);
        let names: Vec<&str> = table.iter().map(|backend| backend.name()).collect();
        #[cfg(target_os = "linux")]
        assert_eq!(names, vec![BWRAP_BACKEND, IDENTITY_BACKEND]);
        #[cfg(not(target_os = "linux"))]
        assert_eq!(names, vec![BWRAP_BACKEND]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **identity 的 argv 是裸 shell**：不包 bwrap、自己声明 confinement、工作目录单列。
    #[cfg(target_os = "linux")]
    #[test]
    fn the_identity_plan_is_a_bare_shell_with_confinement() {
        let root = std::env::temp_dir().join(format!("celira-sbx-identity-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("建临时环境根");
        let request = SandboxRequest::new(&root, &root, "echo hi", "bash");
        let plan = IdentityBackend::new(&root)
            .command(&request)
            .expect("拼得出命令行");
        assert_eq!(plan.program(), "bash", "identity 不包 bwrap");
        assert_eq!(plan.args(), ["-c", "echo hi"]);
        assert_eq!(plan.confinement(), Confinement::SeccompNoIp);
        assert_eq!(plan.working_directory(), Some(root.as_path()));
        // bwrap 那一支逐字不变：不声明 confinement、不设工作目录（`--chdir` 在 argv 里）。
        let bwrap = BwrapBackend::new().command(&request).expect("bwrap 拼得出");
        assert_eq!(bwrap.confinement(), Confinement::None);
        assert_eq!(bwrap.working_directory(), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **保证集照实降级**：identity 不许借用 bwrap 的词（`none` / `system-read-only`）。
    #[cfg(target_os = "linux")]
    #[test]
    fn the_identity_guarantees_do_not_borrow_bwraps_words() {
        let guarantees = IdentityBackend::new("/tmp").guarantees();
        assert_eq!(
            guarantees.value("files"),
            Some("workspace-writable,landlock")
        );
        assert_eq!(guarantees.value("network"), Some("none-ip"));
        assert_ne!(guarantees.value("network"), Some("none"));
        for absent in ["pid", "ipc", "uts", "resources"] {
            assert_eq!(guarantees.value(absent), Some("none"), "{absent}");
        }
        assert_eq!(guarantees.value("workspace"), Some("persistent"));
        assert_eq!(guarantees.value("environment"), Some("scrubbed-by-layer"));
    }

    /// **没有后端就是拒绝**：程序不存在时探针必须给 `sandbox-unavailable`，
    /// 而不是「那就别沙箱了」。
    #[test]
    fn a_missing_backend_is_unavailable_and_never_a_fallback() {
        let backend = BwrapBackend::with_program("celira-no-such-bwrap-binary");
        let unavailable = backend.probe().expect_err("必须拒绝");
        assert_eq!(unavailable.as_str(), SANDBOX_UNAVAILABLE);
        assert!(
            unavailable.detail().contains("找不到"),
            "原因要说清是「找不到程序」：{}",
            unavailable.detail()
        );
        assert!(unavailable.message().starts_with(SANDBOX_UNAVAILABLE));
    }
}
