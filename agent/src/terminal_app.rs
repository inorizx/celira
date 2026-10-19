//! terminal 窗：**主体的手**（整机信任档）。
//!
//! 主体是聊天居民：说话、记事、看世界都齐了，但**做不了事**。本件给主体一次性执行一条命令的能力
//! ——这台机器上的任意命令，跑完即散。
//!
//! ## 档位就是授权本体
//!
//! 本件**不是**出厂内置：它由宿主用一个旗标声明（`--terminal full`）才装配；不给 ⇒ **这个 App 不在
//! 场**（动词不存在，能力面里一个字节都没有）。档位是**封闭枚举**，v1 只装 `full`（整机全权）一档；
//! 未知档位**拒绝启动**，不静默降级到某一档（降档是加墙，等真实消费者再立）。
//!
//! **`full` 是信任声明，不是安全豁免**：五堵墙一堵都不少；它不加的是**效果域**上的边界承诺
//! ——这正是"给够"的语义（零弹窗、不逐条批）。命令内容**不做语义分类**：没有黑白名单、没有
//! "这条命令危险"的判断（那一轴调研里市场也没做好，我们不接）。
//!
//! ## 手落在哪儿：不传 `cwd` ⇒ **主体自己的家**（2026-10-14 裁）
//!
//! 判据一句话：**默认落脚点必须是一个声明的地址，不能是"进程碰巧从哪儿起"。** 同一句话
//! `--materials` 早就写着（「**不会被默认为当前工作目录**」，`agent_host.rs`）；而 terminal
//! 从前恰恰是后者——它继承宿主进程的当前目录，那个目录是**机器级家根**（家里的配置、模型来源
//! 清单、别的住户都住在那儿），而主体的地盘在更深一层（`agents/<支>/`）。真机代价
//! （2026-10-14）：主体为了找到"我自己家在哪儿"多花三轮（`Get-Location` → 列 `agents/` →
//! 读配置），而手册承诺的 `projects/` 就在主体家里——**相对路径本该落在的地方**。
//!
//! 现在的形状：不给 `cwd` ⇒ 在**主体自己的环境根**里执行（就是写 `projects/` 的那个地方）；
//! 给了 ⇒ 在点名的那一个目录里执行（整机全权那一档的语义一字未变）。**两条走同一道检查**：
//! 目录不存在（或不是目录）⇒ 本地拒绝 `invalid-argument`——**没有静默回落**（回落就是把
//! "家不在"读成"那就在别处跑吧"）。
//!
//! ⚠ **它不是一道墙**：手仍是整机全权的（`cwd` 可以指向这台机器上任何存在的目录）。它改的是
//! **默认值**，不是**权限**——权限那一轴见 `outputs/AGENT_FILES_HOME_WORKORDER_2026-10-14.md`
//! §12（档位与拦截点，待裁）。
//!
//! ## 五堵墙（平台无关，全在本层）
//!
//! 1. **凭据洗**：子进程环境里洗掉凭据家族——系统自有前缀（`CELIRA_`）那族一律按前缀洗，
//!    另外由宿主声明的键名（模型、搜索等）逐个洗。洗掉了几个如实报；**洗不掉的**（万一有）
//!    也如实报（主体的回执里只出现**键名**，永远不出现取值）；
//! 2. **超时**：默认 120 秒、可参数调、**上限 600 秒**；超时**尽力击杀进程树**，并如实区分
//!    "杀掉了"（`timeout`）与"没杀掉/杀不干净"（`unknown`）；
//! 3. **输出有界**：stdout / stderr **各 ≤32 KiB**，超限**截断并带明确标记**（显式截断合法，
//!    静默截断违禁），退出码照报；**截断时首尾各留一半**（结论落在尾部，只给头部等于把
//!    "为什么失败"丢掉——两段之间那一段是真的缺了，标记里说清缺了多少）；读输出失败如实报
//!    `output-lost`。**收流也有界**（2026-10-11）：
//!    命令退出之后，两条流各自最多再等一个宽限窗口（[`DRAIN_WINDOW`]）——窗口内读到 EOF ⇒
//!    `closed = true`，到点还没关 ⇒ `closed = false`。判据一句话：**命令的寿命归
//!    命令，流的寿命不归它**——它顺手拉起的常驻进程不该拖着这一轮（实测：一条 1 秒跑完的
//!    命令挂了 68 分钟）。这一格**不是失败**，它说的是"这次读取覆盖到哪"；
//! 4. **审计免费**：每一条命令与它的结果都走既有的动作事实（本层不另建日志、不另立事实源）；
//! 5. **没有 OS 沙箱，也不假装有**：`full` 档本来就是"这台机器随主体用"，本层不做隔离，
//!    因此也不存在 `sandbox-unavailable` 这种既像隔离又像故障的第三种说法。
//!
//! ## 失败词汇（封闭五项）
//!
//! `invalid-argument`（空命令 / 超长 / 工作目录不存在 / 超时不合法）/ `timeout`（**击杀成功**：
//! 命令中断是已知形状）/ `spawn-failure`（压根没跑起来）/ `output-lost`（读输出失败）/
//! `unknown`（**击杀失败或杀不干净**——效果不明，如实说）。
//!
//! **跑起来了、跑完了 = 成功**：退出码是**读数**不是失败分类（非零退出码照样是成功执行）。
//!
//! ## 失败也带正文
//!
//! 超时与击杀失败这两条路上，**已经读到的东西照样交出去**：两条流各自的读数、正文与截断标记，
//! 加上 `killed` 与一句说明效果边界的 `note`。理由是同一条纪律的两面——**失败不等于没有内容**，
//! 而"卡住之前它打印了什么"恰恰是这时候最该给的信息。分类一个字不变（仍是封闭五项里那一个），
//! 变的是**载荷**：走 [`ActionOutcome::failure_with_content`]（Core 早就为"失败带结构化读数"
//! 留了这一格，`notes` / `os` / `timer` 都在用）。
//!
//! ## 编码：字节怎么变成字，是一份**读数**
//!
//! 子进程吐出来的是**字节**，不是字。本件的判据只有一条：**合法 UTF-8 就按 UTF-8 解**；
//! 不合法就按**本机 OEM 代码页**解（Windows 上 `GetOEMCP`），并把页码写进读数
//! （`encoding = cp936`）。稳定取值三种：`utf-8` / `cp<页码>` / `not-utf8`（解不出来时——
//! 那时只做有损替换，并**如实说这段不是 UTF-8**，绝不假装知道它是什么编码）。
//!
//! 唯一允许"自己剪"的例外是**被截断窗口切掉一半的那个字符**：`from_utf8` 报"输入在这里断了"
//! 时，剪掉那半个字符，剩下的仍然是 UTF-8——那不是编码不对，是我们自己切的位置。
//!
//! 为什么必须在这一层解、而不能"让命令自己输出 UTF-8"：**代码页是环境里可以被任何一条命令
//! 改掉的可变状态**（实测：一个兄弟进程跑了 `chcp 936` 之后，紧随其后那条 PowerShell 自己的
//! 中文输出也变成了 GBK）。三家参考实现给 PowerShell 加的那段 UTF-8 前导，实测对**原生命令**
//! 逐字节无效（`ipconfig` 前后都是同一串 GBK 字节，996 个，一个不差）。因此这条路只有一条：
//! **在边界解码，并如实报出用的是什么编码**。
//!
//! ## 效果归类：一个**显式**的偏差
//!
//! `terminal_run` 是写效应（同命令两次 = 两次执行 ⇒ [`EffectClass::RepeatableWrite`] 的语义），
//! 但终端**没有天然的同参复核**：再跑一遍不是复核，那是一次新的执行。因此本件声明的是
//! [`EffectClass::UnverifiableWrite`]（第四类，TASK_36 为它立）：它**仍是写**（⚠ 原来这里写的是
//! "未决效果门照旧拦它"——**门已退役**，门是指令而世界只该给信息；"是写"这件事照旧由**声明**说），
//! 但**不配复核动词**。
//!
//! 代价如实登记：这一档没有同参复核可配 ⇒ `unknown` **没有机械的解除手段**（没有复核能力能解除它）。
//! ⚠ 原来这里接着写的是"最后一条是宿主显式 acknowledge"——那是**未决效果门**的出口，门已退役：
//! 今天这一格是"那次执行没有结果"的如实读数，由主体自己去看（账本 `trace.ndjson`）。
//! v1 不为这条罕见路径预建机制。
//!
//! ## 平台缝（fail-closed）与「臂」
//!
//! 执行层是一个平台缝，形状是「**契约唯一、实现多份**」：
//!
//! - **五堵墙与失败词表是契约**——主体在任何机器上看到的承诺必须一样，因此**只写一份**
//!   （全部在平台无关的编排层里，平台臂碰不到它们）；
//! - 「**臂**」是实现：一支臂 ＝ {默认 shell 名、怎么拼命令行、怎么击杀进程树、非 UTF-8
//!   怎么解}。臂的集合是**开放的**（今天两支：Windows＝PowerShell、Linux＝bash）；将来
//!   一台机器上可以有多支臂（例如同时装着 pwsh 的 Linux），由**主体按判断选**——命令文本
//!   是自由的，一只手就能碰到这台机器上的任何解释器；`TerminalAccess::with_shell` 那条缝
//!   （换 shell ＝ 命令跟着变、动词不变）就是给「将来加第二支臂」留的口。
//! - 臂的四件里，**前三件收在共用平台模块**（`crate::platform`——MCP 的 stdio 通道用它
//!   同一份；凭据洗也在那里，两个消费者共用一条规矩）；**非 UTF-8 解码**只有这只手要它，
//!   留在本件自己的平台模块里（见文件尾）。
//! - [`TerminalTier::available`] 判的是「**本构建里有没有能用的臂**」——**编译期**事实
//!   （这一份二进制编进了哪些臂）；它与「机器处境」（`machine.rs`：**运行时**从机器现读
//!   它是什么——平台与 shell 名）**不是一回事**，两者不能互相顶替。
//!
//! 没有臂的平台上这一档为假 ⇒ 入口**拒绝开启**，本层也**没有实现**——不静默换一个近似行为。
//! 命令词法是**所选 shell 的**（PowerShell 语法 vs bash）：主体的动词不变、命令随 shell。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::app_contract::{
    AppPort, EffectClass, EffectDeclaration, SourceClass, SourceDeclaration,
};
use crate::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Capabilities,
    Capability, ContentBlock, ParamSpec,
};
use crate::foundation::{AgentId, OperationId, RunId};

use std::collections::VecDeque;

/// 这个 App 的稳定标识。
pub const TERMINAL_APP_ID: &str = "terminal";

/// 执行一条命令。
pub const CAPABILITY_RUN: &str = "terminal_run";

/// 看主体起过、还留着的东西（2026-10-11：主体起的东西活得跟住户一样久 ⇒ **回得来**）。
pub const CAPABILITY_HELD: &str = "terminal_held";

/// 停掉一件（并按账追加一行 `stop`）。
pub const CAPABILITY_STOP: &str = "terminal_stop";

const PARAM_COMMAND: &str = "command";
const PARAM_CWD: &str = "cwd";
const PARAM_TIMEOUT: &str = "timeout";
/// **这件要活到关家**（缺省不写 ⇒ 行为与从前逐字节相同：跑完即散）。
const PARAM_KEEP: &str = "keep";
/// 那件的名字（停它、看它都用这个名字；不给就自动给一个）。
const PARAM_NAME: &str = "name";
/// `keep` 这一格只认这两个词（别的取值**本地拒绝**，不就近猜）。
const KEEP_TRUE: &str = "true";
const KEEP_FALSE: &str = "false";
/// 主体不给名字时自动给的那个前缀（后面接序号）。
const DEFAULT_NAME_PREFIX: &str = "p";

/// 一件的**年龄读数**读不出来时的那个词（与 `clock::UNKNOWN_TIME` 同一条纪律）。
pub const UNKNOWN_AGE: &str = "unknown";

/// `terminal_held` 每条给**几行**尾部输出（读数预算：给事实，不给全部）。
const TAIL_LINES: usize = 3;

/// 尾部那几行的字节窗口（先按字节取窗口，再按行对齐——见 `HeldThings::log_tail_bytes`）。
const TAIL_BYTES: usize = 240;

/// 命令文本的机械上限（超限本地拒绝、零执行）。
pub const MAX_COMMAND_BYTES: usize = 8 * 1024;

/// 默认超时（秒）。
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 120;

/// 超时的硬上限（秒）：声明可以调，但**调不过它**。
pub const MAX_TIMEOUT_SECONDS: u64 = 600;

/// 单条流（stdout / stderr）的字节上限：超限**截断并标记**。
pub const MAX_OUTPUT_BYTES: usize = 32 * 1024;

/// 尾部窗口占单流上限的份额（分母）：截断时**首尾各留一半**。
///
/// 判据是**读数的落点**，不是风格：构建、测试、报错这些命令的结论都在**尾部**（汇总行、
/// 第一个错），而只给头部等于把"为什么失败"丢掉。首尾各留一半对今天的头部行为**只增不减**
/// ——没有任何一段内容因为这一刀而看不见了。
const TAIL_SHARE: usize = 2;

/// 尾部窗口的字节数（另一半给头部）。
///
/// 公开的理由：**首尾各半是回执里就能照着算的机械事实**（那行标记也报这两个数），
/// 不是实现细节——判错用例按同一个权威算期望值。
pub fn tail_limit(limit: usize) -> usize {
    limit / TAIL_SHARE
}

/// 头部窗口的字节数（两段合计**不超过** `limit`）。
pub fn head_limit(limit: usize) -> usize {
    limit - tail_limit(limit)
}

/// 解码读数：这一段是合法 UTF-8（绝大多数命令都在这一档）。
///
/// 它是一份**词汇表**的取值，因此权威位置在这里一处：回执里 `encoding = …` 那一栏只许出现
/// [`ENCODING_UTF8`] / `cp<页码>` / [`ENCODING_NOT_UTF8`] 三种形状之一。
pub const ENCODING_UTF8: &str = "utf-8";

/// 解码读数：这一段不是合法 UTF-8，而且**我们连它是什么编码都不知道**
/// （拿不到本机代码页，或按代码页转换失败）——那时只做有损替换，并**如实这么说**。
pub const ENCODING_NOT_UTF8: &str = "not-utf8";

/// 轮询子进程的间隔（为了"到点就杀"，而不是被一次阻塞等待拖住）。
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// 击杀之后等它真正消失的窗口。
const KILL_WINDOW: Duration = Duration::from_secs(10);

/// 子进程**退出之后**还等两条流多久。
///
/// 判据一句话：**命令的寿命归命令，流的寿命不归它**。直接子进程一退出，这一次动作就已经做完了
/// （契约："一次性：跑完即散，没有会话、没有后台"）；此刻还攥着那条管道的，按定义**不是这条
/// 命令**——多半是它顺手拉起的某个常驻进程继承了句柄。没有这个窗口，动作的寿命就被一个**无关
/// 进程**决定了：2026-10-11 实测，一条 1 秒就跑完的只读命令挂了 **68 分钟**（`renderloft-cli`
/// 拉起的守护进程攥着管道不放），而这期间主体一个字都说不出来。
///
/// 窗口内 EOF ⇒ 与从前一字不差（绝大多数命令都在这一档）；到点没 EOF ⇒ **停止等待**，
/// 把"这条流还没关"如实报出来——读到多少给多少，**不假装完整，也不假装截断**。
///
/// ⚠ 它**不改 `timeout` 的含义**：那个参数管的是"这条命令能跑多久"，这个窗口管的是"我们读它
/// 读多久"。两者是两件事，揉在一起会让主体没法表达"这条命令可以慢，但别让别的进程拖着我"。
const DRAIN_WINDOW: Duration = Duration::from_secs(2);

/// 本件自己的失败词汇表（封闭五项）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalFailure {
    /// 参数不合法（空命令、超长、工作目录不是目录、超时不合法）。**本地拒绝、零执行**。
    InvalidArgument,
    /// 超时，而且**击杀成功**：命令中断是已知形状。
    Timeout,
    /// 根本没跑起来（起进程本身失败，或本平台没有这一档的实现）。
    SpawnFailure,
    /// 跑起来了但读输出失败。
    OutputLost,
    /// **击杀失败或杀不干净**：那次执行的效果不明——如实说，不猜。
    Unknown,
}

impl TerminalFailure {
    /// 稳定分类标记（进 `ActionOutcome`，Core 只搬运不解释）。
    pub fn as_str(self) -> &'static str {
        match self {
            TerminalFailure::InvalidArgument => crate::core::INVALID_ARGUMENT,
            TerminalFailure::Timeout => "timeout",
            TerminalFailure::SpawnFailure => "spawn-failure",
            TerminalFailure::OutputLost => "output-lost",
            TerminalFailure::Unknown => "unknown",
        }
    }

    /// 从稳定标记还原（解码用）。认不出的取值就是 `None`——**不猜**，也不就近映射
    /// （调用方按 fail-closed 处理）。
    ///
    /// **标记表就是权威**：`as_str` 与它**逐条对称**，有测试钉住（`tests/action_failure_wordlist.rs`）。
    /// 新增取值必须**同一刀**补在这里——这一格落在盘上的**行动结果**里，缺了解码端就是
    /// 「写得进去、读不出来」。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Option<Self> {
        Some(match text {
            crate::core::INVALID_ARGUMENT => TerminalFailure::InvalidArgument,
            "timeout" => TerminalFailure::Timeout,
            "spawn-failure" => TerminalFailure::SpawnFailure,
            "output-lost" => TerminalFailure::OutputLost,
            "unknown" => TerminalFailure::Unknown,
            _ => return None,
        })
    }
}

/// 档位（**封闭枚举**）。
///
/// v1 只有一档；加档是加墙，等真实消费者（多租户、多个执行点）再立。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalTier {
    /// 整机全权：这台机器上的任意命令。
    Full,
}

impl TerminalTier {
    /// 解析宿主给的档位名。**不认识就是不认识**：不降级、不猜、不就近取一档。
    ///
    /// 连空白都不顺手 trim：档位是**封闭枚举**，多一个空格就意味着宿主写的不是我们认识的那个词
    /// ——那正是该被拒绝的东西（fail-closed），而不是该被我们猜出来的东西。
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "full" => Some(TerminalTier::Full),
            _ => None,
        }
    }

    /// 稳定标识（入口报错时用它，不进主体的词表）。
    pub fn as_str(self) -> &'static str {
        match self {
            TerminalTier::Full => "full",
        }
    }

    /// **本构建**里有没有这一档能用的**臂**（平台缝 fail-closed：没有 ⇒ 入口拒绝开启）。
    ///
    /// 这是**编译期**判断——它回答"这一份二进制里编进了哪些臂"（今天两支：Windows 与
    /// Linux）；与「机器处境」（`machine.rs`：**运行时**从机器现读它是什么——平台与 shell
    /// 名）**不是一回事**，两者不能互相顶替：机器处境跟着机器走，能力面跟着**这一份构建**走。
    pub fn available(self) -> bool {
        cfg!(any(windows, target_os = "linux"))
    }
}

/// 宿主给 terminal 窗的**声明**：档位 + 要洗掉的凭据键名。
///
/// 键名由宿主给出（本件**不读环境变量**：那是入口的事）：系统自有前缀那族按前缀洗，
/// 这里给的是另外几个（模型、搜索……）。本类型**不派生 `Debug`**，也永远不携带取值。
#[derive(Clone)]
pub struct TerminalAccess {
    tier: TerminalTier,
    scrub: Vec<String>,
    shell: Option<String>,
}

impl TerminalAccess {
    /// 用档位与要洗的键名建立一份声明（shell 用平台默认）。
    pub fn new(tier: TerminalTier, scrub: Vec<String>) -> Self {
        Self {
            tier,
            scrub,
            shell: None,
        }
    }

    /// **缝**：声明这一档用哪个 shell。
    ///
    /// 命令词法是所选 shell 的（PowerShell 语法 vs bash），因此「换 shell」＝命令跟着变、
    /// **主体的动词不变**。不给就用平台默认；给了但起不来如实报 `spawn-failure`（不换一个近似行为）。
    pub fn with_shell(mut self, shell: impl Into<String>) -> Self {
        self.shell = Some(shell.into());
        self
    }

    /// 声明的档位。
    pub fn tier(&self) -> TerminalTier {
        self.tier
    }

    /// 这次要用哪个 shell（`None` = 平台默认）。
    fn shell(&self) -> Option<&str> {
        self.shell.as_deref()
    }
}

/// 主体的手。
pub struct TerminalApp {
    access: TerminalAccess,
    /// 主体名下那些**活到关家**的事（一本只追加的账，住在环境根里）。
    ///
    /// 它只持有环境根，**不做任何 IO**（账可能还不存在）；读与写都发生在动作那一刻。
    held: crate::terminal_held::HeldThings,
}

impl TerminalApp {
    /// 装配本件（档位来自宿主声明；本件自己不判"要不要在场"——那是装配期的事）。
    ///
    /// 环境根是那本账住的地方（`<环境根>/terminal` 与 `<环境根>/terminal-logs/`）——
    /// 与时钟 App、纸条板同款：**自持物住在自己的地盘上**，不需要任何授权，也不碰环境里
    /// 任何既有文件。
    pub fn new(access: TerminalAccess, environment_root: &std::path::Path) -> Self {
        Self {
            access,
            held: crate::terminal_held::HeldThings::new(environment_root),
        }
    }

    /// 本件声明的能力（三条：跑一次、跑一件留着、停一件）。
    fn own_capabilities() -> Capabilities {
        Capabilities::of(vec![
            Capability::new(
                ActionId::new(CAPABILITY_RUN),
                "在这台机器上执行一条命令（一次性：跑完即散，没有会话、没有后台）",
                vec![
                    ParamSpec::new(
                        PARAM_COMMAND,
                        true,
                        format!("要执行的命令（非空、≤{MAX_COMMAND_BYTES} 字节；交给 shell 原样执行）"),
                    ),
                    ParamSpec::new(
                        PARAM_CWD,
                        false,
                        "在哪个目录里执行（不给就在你自己的家里执行；必须是存在的目录）",
                    ),
                    ParamSpec::new(
                        PARAM_TIMEOUT,
                        false,
                        format!("超时秒数（1–{MAX_TIMEOUT_SECONDS}，缺省 {DEFAULT_TIMEOUT_SECONDS}）"),
                    ),
                    ParamSpec::new(
                        PARAM_KEEP,
                        false,
                        format!(
                            "这件要活到关家（`{KEEP_TRUE}` / `{KEEP_FALSE}`，缺省 `{KEEP_FALSE}`）。\
                             给 `{KEEP_TRUE}` ⇒ 它不在这条命令里跑完：系统替你留着它、把它的输出写进\
                             一个文件，并把这件事记进「我起过的东西」那本账（回执给名字、pid、输出路径）"
                        ),
                    ),
                    ParamSpec::new(
                        PARAM_NAME,
                        false,
                        format!(
                            "给这件起个名字（只在 `{PARAM_KEEP}={KEEP_TRUE}` 时有意义；不给就自动给一个）。\
                             名字 ≤{} 字节、不含路径分隔符与控制字符——看它、停它都用这个名字",
                            crate::terminal_held::MAX_NAME_BYTES
                        ),
                    ),
                ],
            )
            .with_details(format!(
                "命令词汇：交给本平台默认的 shell（就是这个平台自带那个），词法就是那个 shell 的\
                 ——换 shell 会让命令语法变，但这个动词不变。状态不保留：每次调用都是全新的 \
                 shell 进程——`cd`、变量、函数都不过夜，所以别指望上一条命令留下的处境；要在哪个目录\
                 里执行就传 `cwd`（不传就在你自己的家里执行——你放 `projects/` 的那个地方），\
                 要跨命令留东西就自己落到文件里。\
                 机械边界：默认 {DEFAULT_TIMEOUT_SECONDS} 秒超时\
                 （可以调，最多 {MAX_TIMEOUT_SECONDS} 秒，到点尽力杀掉整棵进程树）；stdout 与 stderr 各最多\
                 {} KiB，超出即截断并明确标出（显式截断，不是静默丢）——截断时\
                 首尾各留一半，中间那一段没有（构建、测试、报错的结论都在尾部，别只看开头\
                 那一半就下判断）；缺的那一段不替你留着——要它就把这条命令的输出自己重定向到\
                 文件（`… > 文件`）再分片读，或者换一条只取你要的那一段的命令；子进程环境里\
                 洗掉凭据，\
                 洗掉几个会如实报（洗不掉的会点名）。回执：退出码、两条流各自的字节数、截断标记与\
                 解码读数（`encoding = utf-8` / `cp<页码>` / `not-utf8`——输出不是合法 UTF-8 时\
                 如实报出这一段用的是什么编码：本机认得出就报 `cp<页码>`，认不出就如实说\
                 `not-utf8`；绝不静默把字节变成 `�`）。两条读数别读错：`truncated = true` 说的是\
                 你手里的不是全部（缺中间那一段），不是「这条流就这些」；而 `closed = false` 说的是\
                 这次读取只覆盖到那一刻——它既不是「命令没做完」，也不是「没有输出」。\
                 收流读数：命令退出之后，两条流各自\
                 最多再等 {} 秒的宽限窗口——窗口内读完是 `closed = true`，到点还没关是\
                 `closed = false`（上面那些是到那一刻为止读到的；此刻攥着这条流的\
                 按定义不是这条命令，多半是它顺手拉起的常驻进程继承了句柄）。它不是失败：\
                 退出码是读数，命令跑完了就是成功。失败也带正文：超时、\
                 或没杀干净时，回执里照样给出已经读到的两条流与读数（退出码未知就如实不写），\
                 另有一行 `note` 说明这次只保证到什么程度。写动作：同一条命令跑两次就是执行两次。\
                 跑起来了、跑完了就是成功——退出码是读数，不是失败分类。\
                 `{PARAM_KEEP}`（2026-10-11 加的那一档）：给了 `{KEEP_TRUE}`，这条命令就\
                 不再「跑完即散」——系统替你留着它（它活到家关，不是活到某次调用结束），\
                 输出写进 `<环境根>/terminal-logs/<名字>.out`，这件事记进「我起过的东西」那本账。\
                 ⚠ 它是给「要一直跑着的东西」用的（服务、watcher、长构建）：给它一条前台命令\
                 就行——不要自己去「脱离」（`Start-Process`、托 WMI 之类）：那会让它落在系统\
                 看得见、收得住的范围之外（收不住，也停不掉）。回执给的是名字、pid、输出路径\
                 三样；它到底起没起成，读那个输出文件（那是它自己说的话）。一件活到关家的事，\
                 用 `{CAPABILITY_HELD}` 看、用 `{CAPABILITY_STOP}` 停；家一关，它跟着走。",
                MAX_OUTPUT_BYTES / 1024,
                DRAIN_WINDOW.as_secs()
            )),
            Capability::new(
                ActionId::new(CAPABILITY_HELD),
                "看我起过、还留着的东西（名字、起了多久、pid、还在不在、输出写到哪儿）",
                vec![],
            )
            .with_details(
                "这是读数，不是动作：一个字节都不改。它摆出的是我起过的、还留着的那些\
                 （用 `terminal_run` 带 `keep` 起的）——每条给：名字、起了多久（`age`）、pid、\
                 还在不在（`state` 三态：`alive` 在跑／`gone` 不在了／`unknown` 读不到——\
                 ⚠ 后两个不是一回事，别把「我看不到它」读成「它没了」）、当时那条命令、以及输出文件\
                 的路径与字节数，还有它最近说的那几行（`tail`：最多 {TAIL_LINES} 行 / {TAIL_BYTES} 字节\
                 ——这是读数，不是判断：它不告诉你「这算不算正常」；要看更多，用文件动词读上面那个路径，\
                 那是同一份东西）。读输出用文件动词（`files_*`）读那个路径——本件不另造一个读输出的\
                 动作。账读不出来时这一行是 `held = unknown`（不是 `held = 0`：那是「我一件\
                 都没起过」，两件事）。⚠ 死条目不会自己走：一件起来就崩了，它还留在账上（`state = gone`\
                 是读数）——什么时候把它划掉是你的事（`{CAPABILITY_STOP}`）。家一关，账上那些跟着走\
                 （它们活到关家）。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_STOP),
                "停掉一件我起过、还留着的东西（连它拉起的那些一起收）",
                vec![ParamSpec::new(
                    PARAM_NAME,
                    true,
                    "要停的那件的名字（`terminal_held` 里给的那个）",
                )],
            )
            .with_details(
                "它是一次动作：按账上记的那个 pid 尽力收掉整棵进程树，并在账上追加一行\
                 `stop`（账只追加，不改写历史）。收完之后这件就从「我起过的东西」里消失了。\
                 回执如实报结果：`killed = true` 是我报出去的那一下成功了，不等于树真的\
                 没了（它可能早就自己退了、也可能收不动——那时 `state` 那一格会说）。\
                 ⚠ 射程（如实说清）：收的是那棵树——从账上那个 pid 出发。那条命令早就退了、\
                 而它拉起的东西又脱离了那棵树时（例如托 WMI、计划任务、服务去起的），这一下够不着\
                 它；那种形状今天收不住，要你自己去收。名字不在账上 ⇒ 本地拒绝（`invalid-argument`），\
                 不猜。",
            ),
        ])
    }

    /// 执行一条命令。
    fn run(&self, params: &ActionParams) -> ActionOutcome {
        let command = match params.get(PARAM_COMMAND) {
            Some(command) => command.trim(),
            None => return refused(TerminalFailure::InvalidArgument),
        };
        if command.is_empty() || command.len() > MAX_COMMAND_BYTES {
            return refused(TerminalFailure::InvalidArgument);
        }
        // **不传 `cwd` ＝ 在主体自己的家里执行**（2026-10-14 裁）：默认是一个**声明的地址**
        // （主体的环境根），不是"进程碰巧从哪儿起"——同一条判据 `--materials` 早就写着
        // （「不会被默认为当前工作目录」）。给了与不给走**同一道检查**：不存在（或不是目录）
        // ⇒ 本地拒绝（那是一次**无法执行**的请求，不值得为它起一个注定失败的进程）；
        // **没有静默回落**（回落就是把"家不在"读成"那就在别处跑吧"）。
        let directory = match params.get(PARAM_CWD) {
            Some(raw) => PathBuf::from(raw.trim()),
            None => self.held.environment_root().to_path_buf(),
        };
        if !directory.is_dir() {
            return refused(TerminalFailure::InvalidArgument);
        }
        let timeout = match params.get(PARAM_TIMEOUT) {
            Some(raw) => match raw.trim().parse::<u64>() {
                Ok(seconds) if (1..=MAX_TIMEOUT_SECONDS).contains(&seconds) => {
                    Duration::from_secs(seconds)
                }
                _ => return refused(TerminalFailure::InvalidArgument),
            },
            None => Duration::from_secs(DEFAULT_TIMEOUT_SECONDS),
        };
        let keep = match params.get(PARAM_KEEP) {
            Some(raw) => match raw.trim() {
                KEEP_TRUE => true,
                KEEP_FALSE => false,
                // 这一格只认那两个词：别的取值**本地拒绝**（不就近猜成 false）。
                _ => return refused(TerminalFailure::InvalidArgument),
            },
            None => false,
        };
        if !keep {
            // 缺省那一档：**与从前逐字节相同**（跑完即散，五堵墙一条不少）。
            return execute(&self.access, command, &directory, timeout);
        }
        let wanted = params.get(PARAM_NAME).map(str::trim).unwrap_or("");
        self.keep_it(command, &directory, wanted)
    }

    /// **这件要活到关家**：起它、把它的输出写进文件、把这件事记进账，**不等它**。
    ///
    /// 与 `run_tree` 的关系：**墙共用同一套**（凭据洗、平台 shell、cwd、不给 stdin），差别只在
    /// 三处——**不等它**、两条流**写进文件**（不是管道）、**多记一行账**。超时那一堵墙在这一档
    /// **不适用**：这一档的定义就是"不在这条命令里跑完"。
    #[cfg(any(windows, target_os = "linux"))]
    fn keep_it(&self, command: &str, directory: &Path, wanted: &str) -> ActionOutcome {
        let name = if wanted.is_empty() {
            self.next_held_name()
        } else {
            wanted.to_string()
        };
        if crate::terminal_held::name_problem(&name).is_some() {
            return refused(TerminalFailure::InvalidArgument);
        }
        let log = self.held.log_path(&name);
        // 开输出文件、读它多大，都归**账本模块**（"输出落在哪儿"是它说的事）——本件不碰
        // 文件系统（源码守卫 `boundary_apis_stay_in_their_own_layer` 钉着这条分层）。
        let file = match self.held.open_log(&name) {
            Ok(file) => file,
            Err(_) => return refused(TerminalFailure::SpawnFailure),
        };
        let stderr = match file.try_clone() {
            Ok(handle) => handle,
            Err(_) => return refused(TerminalFailure::OutputLost),
        };
        let (mut environment, _scrubbed, _unscrubbed) =
            crate::platform::child_environment(&self.access.scrub);
        let shell = self
            .access
            .shell()
            .unwrap_or(crate::platform::TERMINAL_SHELL);
        let mut builder = crate::platform::terminal_command(shell, command, &mut environment);
        // 两条流**写进文件**（不是管道）：这就是"回得来"的那一半——那件事吐出来的话落在
        // 主体读得回来的地方（用文件动词读），而不是随这次调用一起消失。
        builder
            .stdin(Stdio::null())
            .stdout(Stdio::from(file))
            .stderr(Stdio::from(stderr));
        crate::platform::apply_environment(&mut builder, environment);
        builder.current_dir(directory);
        let child = match builder.spawn() {
            Ok(child) => child,
            Err(_) => return refused(TerminalFailure::SpawnFailure),
        };
        let pid = child.id();
        let log_text = log.to_string_lossy().to_string();
        if self
            .held
            .append_start(&name, command, pid, &log_text, now_millis())
            .is_err()
        {
            // 记不下来就留不住 ⇒ **不留下没被记过的东西**：尽力收掉它，并如实说这一档没成
            // （收不收得掉是读数，不是承诺）。
            crate::platform::kill_tree(pid);
            return ActionOutcome::failure_with_content(
                TerminalFailure::Unknown.as_str(),
                vec![ContentBlock::new(
                    CAPABILITY_RUN,
                    format!(
                        "held = {name}\npid = {pid}\noutput = {log_text}\nnote = 它起来了，但那本账写不进去 ⇒ 我把它收了（没被记过的东西不留）"
                    ),
                )],
            );
        }
        ActionOutcome::success(vec![ContentBlock::new(
            CAPABILITY_RUN,
            format!(
                "held = {name}\npid = {pid}\noutput = {log_text}\nnote = 它活到关家（家一关它跟着走）；\
                 看它用 {CAPABILITY_HELD}，停它用 {CAPABILITY_STOP} name={name}；\
                 它到底起没起成，读那个输出文件（那是它自己说的话）"
            ),
        )])
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    fn keep_it(&self, _command: &str, _directory: &Path, _wanted: &str) -> ActionOutcome {
        // 平台缝：本平台没有臂 ⇒ 这一档起不来（入口已经拒绝开启整档，正常走不到这里）。
        refused(TerminalFailure::SpawnFailure)
    }

    /// **往显示区交素材**（方向 `app → 显示区`，与时钟 App 同款）：把「我起过、还留着的东西」
    /// 填进 [`crate::holdings::HoldingRow::Held`]。
    ///
    /// ⚠ 与动词那一侧的读数是**同一份来源**（那本账），只是**摆法归显示区**：这里只交结构化内容
    /// （名字、状态、起的时刻、当时那条命令、输出路径与字节数）——**怎么摆成一行不是本件的事**。
    /// 装配层因此不用翻那本账（`app → 显示区` 的方向与时钟 App 一模一样）。
    ///
    /// 状态那一格是**此刻问操作系统一次**得来的（三态：在跑／不在／**读不到**）：账上不记它——
    /// 记了就等于把"当时"当成"现在"。
    pub fn held_material(
        environment_root: &Path,
    ) -> std::io::Result<Vec<crate::holdings::HoldingRow>> {
        let held = crate::terminal_held::HeldThings::new(environment_root);
        Ok(held
            .held()?
            .entries
            .into_iter()
            .map(|each| {
                let bytes = held.log_bytes(&each.name);
                crate::holdings::HoldingRow::Held {
                    name: each.name,
                    state: match crate::resident_lifetime::process_is_alive(each.pid) {
                        Some(true) => crate::holdings::HeldState::Running,
                        Some(false) => crate::holdings::HeldState::Gone,
                        None => crate::holdings::HeldState::Unknown,
                    },
                    started_millis: each.at_millis,
                    command: each.command,
                    log: each.log,
                    bytes,
                }
            })
            .collect())
    }

    /// 自动给一个还没被用过的名字（`p1`、`p2`…）。
    fn next_held_name(&self) -> String {
        let used = self
            .held
            .held()
            .map(|held| held.entries)
            .unwrap_or_default();
        let mut index = 1usize;
        loop {
            let candidate = format!("{DEFAULT_NAME_PREFIX}{index}");
            if !used.iter().any(|each| each.name == candidate) {
                return candidate;
            }
            index += 1;
        }
    }

    /// **看我起过、还留着的东西**（读数：一个字节都不改）。
    fn held_reading(&self) -> ActionOutcome {
        let held = match self.held.held() {
            Ok(held) => held,
            // 读不出来 ⇒ `unknown`：**不是 `0`**（那是"一件都没起过"，两件事）。
            Err(_) => {
                return ActionOutcome::success(vec![ContentBlock::new(
                    CAPABILITY_HELD,
                    "held = unknown\nnote = 那本账这次读不出来（它不是空的，是没读到）",
                )]);
            }
        };
        let mut receipt = format!("held = {}\n", held.entries.len());
        if held.stopped > 0 {
            receipt.push_str(&format!("stopped-before = {}\n", held.stopped));
        }
        let now = now_millis();
        for each in held.entries.iter().take(crate::terminal_held::MAX_RENDERED) {
            receipt.push_str(&format!(
                "- {}  age = {}  pid = {}  state = {}  output = {} ({} bytes)\n  command = {}\n  tail = {}\n",
                each.name,
                age_reading(now, each.at_millis),
                each.pid,
                state_word(crate::resident_lifetime::process_is_alive(each.pid)),
                each.log,
                // 读不到 ⇒ `unknown`（**不拿 0 冒充"它还没说话"**——0 是一个真实的读数）。
                self.held
                    .log_bytes(&each.name)
                    .map(|bytes| bytes.to_string())
                    .unwrap_or_else(|| UNKNOWN_AGE.to_string()),
                each.command,
                self.tail_reading(&each.name),
            ));
        }
        let rest = held
            .entries
            .len()
            .saturating_sub(crate::terminal_held::MAX_RENDERED);
        if rest > 0 {
            receipt.push_str(&format!("（还有 {rest} 条没摆出来）\n"));
        }
        ActionOutcome::success(vec![ContentBlock::new(CAPABILITY_HELD, receipt)])
    }

    /// 一件**最近说了什么**（尾部几行；**读不到**与**还没说话**分开说）。
    ///
    /// ⚠ 这是**读数**：它不判断"这算不算正常"。上限就 [`TAIL_LINES`] 行 / [`TAIL_BYTES`] 字节——
    /// 要看更多，用文件动词读上面那一行给的路径（**那是同一份东西，不是第二份**）。
    fn tail_reading(&self, name: &str) -> String {
        let Some(bytes) = self.held.log_tail_bytes(name, TAIL_BYTES) else {
            return UNKNOWN_AGE.to_string();
        };
        if bytes.is_empty() {
            return "（还没有输出）".to_string();
        }
        let (text, encoding) = decode_bytes(&bytes);
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim_end)
            .filter(|each| !each.trim().is_empty())
            .collect();
        let start = lines.len().saturating_sub(TAIL_LINES);
        let body = lines[start..].join(" | ");
        if encoding == ENCODING_UTF8 {
            body
        } else {
            format!("{body}（encoding = {encoding}）")
        }
    }

    /// **停掉一件**（动作：收进程树 ＋ 在账上追加一行 `stop`）。
    fn stop_it(&self, params: &ActionParams) -> ActionOutcome {
        let name = params.get(PARAM_NAME).map(str::trim).unwrap_or("");
        if name.is_empty() {
            return refused(TerminalFailure::InvalidArgument);
        }
        let held = match self.held.held() {
            Ok(held) => held,
            Err(_) => return refused(TerminalFailure::Unknown),
        };
        // 名字不在账上 ⇒ **本地拒绝**（不猜、也不去猜一个 pid）。
        let Some(entry) = held.entries.iter().find(|each| each.name == name) else {
            return refused(TerminalFailure::InvalidArgument);
        };
        let before = crate::resident_lifetime::process_is_alive(entry.pid);
        let killed = crate::platform::kill_tree(entry.pid);
        if self.held.append_stop(name, now_millis()).is_err() {
            return ActionOutcome::failure_with_content(
                TerminalFailure::Unknown.as_str(),
                vec![ContentBlock::new(
                    CAPABILITY_STOP,
                    format!(
                        "stopped = {name}\npid = {}\nkilled = {killed}\nnote = 收那一下报的是 {killed}，但那本账写不进去 ⇒ 下一轮它还会在账上（读数不是承诺）",
                        entry.pid
                    ),
                )],
            );
        }
        ActionOutcome::success(vec![ContentBlock::new(
            CAPABILITY_STOP,
            format!(
                "stopped = {name}\npid = {}\nbefore = {}\nkilled = {killed}\nstate = {}\nnote = 「killed = true」是我报出去的那一下成功了，不等于树真的没了（`state` 那一格才是读数）",
                entry.pid,
                state_word(before),
                state_word(crate::resident_lifetime::process_is_alive(entry.pid)),
            ),
        )])
    }
}

/// 现在几点（unix 毫秒）。钟读不出来 ⇒ `0`（读数那一格会说 `unknown`，不编一个数）。
fn now_millis() -> u64 {
    crate::clock::ClockHandle::system()
        .now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|each| each.as_millis() as u64)
        .unwrap_or(0)
}

/// 一件的年龄读数（秒）。
fn age_reading(now_millis: u64, at_millis: u64) -> String {
    if now_millis == 0 || at_millis == 0 || at_millis > now_millis {
        return UNKNOWN_AGE.to_string();
    }
    format!("{}s", (now_millis - at_millis) / 1000)
}

/// 存活三态的那个词（**读不到不许写成"不在了"**）。
fn state_word(alive: Option<bool>) -> &'static str {
    match alive {
        Some(true) => "alive",
        Some(false) => "gone",
        None => "unknown",
    }
}

/// 这一族的统一出口。
fn refused(failure: TerminalFailure) -> ActionOutcome {
    ActionOutcome::failure(failure.as_str())
}

/// 读线程发回来的东西。
///
/// **为什么边走边发、而不是读完了再交**：子进程退出之后我们只等一个**有界的收流窗口**
/// （[`DRAIN_WINDOW`]），窗口到点时线程可能还没结束——那一刻调用方手里必须**已经**有"到那时
/// 为止读到的字节"。若等 `join()`，它只能报"什么都没有"，而那是**假话**：命令明明说话了。
enum OutputMsg {
    /// 这一流又读到一段**头部窗口**（超过 [`head_limit`] 的部分不在这里，因此通道里攒不下
    /// 无界的数据）；尾部窗口不走这条路——它只在读完之后随 [`Self::Done`] 一起回去。
    Chunk(Vec<u8>),
    /// 读完了：这一流**一共**读出多少字节（含被上限截掉的部分），外加**尾部窗口**那一段。
    ///
    /// 为什么尾部只能在这一刻交：**尾只在末尾才存在**。收流窗口到点时这一条还没来，那一次
    /// 读取就只有头部——那一档由既有的 `closed = false` 如实说，不另立第三态。
    Done { total: usize, tail: Vec<u8> },
    /// 读这条流本身失败了。
    Failed,
}

/// 两条流共用一条消息通道，所以每条消息都要说清**它是哪条流的**。
#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputSide {
    Out,
    Err,
}

/// 一条流**到这一刻为止**的读数。
///
/// `closed` 是第三态那一格：`false` 不等于"空"，也不等于"读失败"——它说的是
/// **这次读取覆盖到哪儿**（流在命令退出后仍未关闭）。三者分开，是为了让回执不撒谎。
#[derive(Default)]
struct OutputReading {
    /// **头部窗口**那一段字节。
    kept: Vec<u8>,
    /// **尾部窗口**那一段字节：只在读完之后才有（见 [`OutputMsg::Done`]）。
    ///
    /// 它只在一件事上特殊：**不截断时它与 `kept` 是连续的**（同一条流的两段），拼接起来就是
    /// 全部输出；**截断时中间真的缺了一段**，那时两段各自解、中间插标记。
    tail: Vec<u8>,
    total: usize,
    truncated: bool,
    /// 读到 EOF 了（这一条流收干净了）。`false` = 窗口到点它还没关。
    ///
    /// 它是**第三态**，与 [`Self::lost`] 和 [`Self::total`] 各说各的事：`lost` 是"读它本身
    /// 失败"，`total` 是"读到多少"，而它是"**这次读取覆盖到哪**"。
    closed: bool,
    /// 读这条流本身失败。
    lost: bool,
}

impl OutputReading {
    /// 收下一条消息。`limit` 与读线程用的是同一个上限——两边各留一份余量，
    /// 是为了"窗口到点时按已收下的算"与"读完了按总数算"给出**同一个** `truncated`。
    ///
    /// 两个窗口各自的容量由 [`head_limit`] / [`tail_limit`] 定：**合起来不超 `limit`**
    /// （这是"有界"的全部内容，别让它变成两倍预算）。
    fn absorb(&mut self, msg: OutputMsg, limit: usize) {
        match msg {
            OutputMsg::Chunk(bytes) => {
                let room = head_limit(limit).saturating_sub(self.kept.len());
                self.kept.extend_from_slice(&bytes[..bytes.len().min(room)]);
            }
            OutputMsg::Done { total, tail } => {
                self.total = total;
                self.truncated = total > limit;
                // 再截一次只是**带外余量**（泵那一侧已经按同一个上限收过）：两侧各留一份，
                // 与 `kept` 同款。
                self.tail = tail[..tail.len().min(tail_limit(limit))].to_vec();
                self.closed = true;
            }
            OutputMsg::Failed => self.lost = true,
        }
    }
}

/// 有界地把一条流**读到末尾**（不然子进程会被满管道卡住），把**头部窗口**边走边发回去，
/// 把**尾部窗口**留到读完之后随 `Done` 一起交回去。
///
/// 交出来的是**字节**、不是文本：怎么把字节变成字是 [`decode_bytes`] 那一格的事，两件事分开
/// ——分开的理由是"读多少"是机械边界，而"按哪个编码解"是**读数**，各有各的权威。
fn pump_output(
    side: OutputSide,
    mut reader: impl Read,
    limit: usize,
    tx: Sender<(OutputSide, OutputMsg)>,
) {
    let head = head_limit(limit);
    let tail_cap = tail_limit(limit);
    let mut buffer = [0u8; 8 * 1024];
    let mut total = 0usize;
    let mut sent = 0usize;
    // **尾部窗口**：一直读到末尾，只留最后 `tail_cap` 字节。它**不占通道**（不随读随发），
    // 所以"通道里攒不下无界的数据"这一条照旧成立——有界的是它自己的容量。
    let mut tail: VecDeque<u8> = VecDeque::with_capacity(tail_cap);
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                total += read;
                if sent < head {
                    let take = read.min(head - sent);
                    sent += take;
                    // 调用方已经不等了（窗口到点、通道被丢）：这条流不再有人要，收工。
                    if tx
                        .send((side, OutputMsg::Chunk(buffer[..take].to_vec())))
                        .is_err()
                    {
                        return;
                    }
                }
                if tail_cap > 0 {
                    for byte in &buffer[..read] {
                        if tail.len() == tail_cap {
                            tail.pop_front();
                        }
                        tail.push_back(*byte);
                    }
                }
            }
            Err(_) => {
                let _ = tx.send((side, OutputMsg::Failed));
                return;
            }
        }
    }
    let _ = tx.send((
        side,
        OutputMsg::Done {
            total,
            tail: tail.into_iter().collect(),
        },
    ));
}

/// 把一段字节解成文本，并**如实报出用的是什么编码**。
///
/// 判据只有一条：**合法 UTF-8 就按 UTF-8 解**（绝大多数命令都在这一档）；不合法就交给平台缝
/// （Windows 上按本机 OEM 代码页解，见 `platform::decode_native`）。
///
/// 唯一允许"自己剪"的例外：`from_utf8` 报**"输入在这里断了"**（`error_len()` 为 `None`）时，
/// 那半个字符是**我们自己截断窗口切的位置**，不是编码不对——剪掉它，这一段仍然是 UTF-8。
fn decode_bytes(bytes: &[u8]) -> (String, String) {
    match std::str::from_utf8(bytes) {
        Ok(text) => (text.to_string(), ENCODING_UTF8.to_string()),
        Err(error) => {
            if error.error_len().is_none()
                && let Ok(text) = std::str::from_utf8(&bytes[..error.valid_up_to()])
            {
                return (text.to_string(), ENCODING_UTF8.to_string());
            }
            platform::decode_native(bytes)
        }
    }
}

/// 一条流没读完时的读数取值：**缺省不等于零**（与 `holdings` / `machine` 同一条判据）。
///
/// 公开的理由与 [`tail_limit`] 同款：它是**读数词汇表**里的一个取值，判错用例要按同一个权威
/// 断言它——不是实现细节。
pub const UNKNOWN_READING: &str = "unknown";

/// 把一条流的**读数**渲染进回执（字节数 + 截断标记 + 解码读数 + **这一条流关没关**）。
///
/// 第四格不是装饰：前三个说的是**内容**（读到多少、有没有超上限、按什么编码解），
/// 它说的是**这次读取覆盖到哪**。两件事揉成一行会读错——"读到了 355 字节"很容易被读成
/// "这条流就这些"，而真相可能是"到窗口为止就这些，还有别的进程攥着它"。
///
/// **没读完的流不许报数字**（2026-10-11 补）：总数只在"读完了"那一条消息里来，所以没读完时
/// 它照旧是 0——那不是"零字节"，是**不知道**。拿 `0 bytes` 与 `truncated = false` 去说一件事
/// 我们并不知道的，就是用两个看起来确定的读数替一段不确定说话；这里如实写 `unknown`。
fn push_output_reading(receipt: &mut String, name: &str, reading: &OutputReading, encoding: &str) {
    let (total, truncated) = if reading.closed {
        (reading.total.to_string(), reading.truncated.to_string())
    } else {
        (UNKNOWN_READING.to_string(), UNKNOWN_READING.to_string())
    };
    receipt.push_str(&format!(
        "{name} = {total} bytes, truncated = {truncated}, encoding = {encoding}, closed = {}\n",
        reading.closed
    ));
}

/// 尾部窗口的**开头**可能落在字符中间——那是**我们自己切的位置**，不是编码不对。
///
/// 与头部"剪掉结尾那半个字符"（见 [`decode_bytes`]）是**同一条判据的两面**：只有我们自己切
/// 出来的那半个字符才剪，剪完之后这一段仍然是 UTF-8。
///
/// ⚠ **只在不截断的时候才叫"切"**：不截断时两段是连续的，跨缝的那个字符是**真的**（它只
/// 是被分在两次读里），那时拼接起来解，一个字节都不许剪（见 [`render_body`]）。
fn trim_leading_partial(bytes: &[u8]) -> &[u8] {
    let mut start = 0;
    while start < bytes.len() && (0x80..=0xBF).contains(&bytes[start]) {
        start += 1;
    }
    &bytes[start..]
}

/// 正文**缺了东西**的两种情形——判据是"我们**知不知道**缺了多少"（都不知道 ≠ 都不说）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cut {
    /// 读完了，总数超过上限：**中间**那一段没有，两头的字节数都是真数。
    Middle,
    /// 没读完（收流窗口到点、这条流还攥在别的进程手里），而头部窗口已经满了：
    /// 这次读取就到此为止，**后面还有多少未知**。
    ///
    /// ⚠ 这一格是"首尾各半"带来的**新责任**，不是顺带：头部窗口小了一半，这一档若不说出来，
    /// 它就成了**静默截断**——那正是本层禁的那一件（显式截断合法）。
    RestUnknown,
}

/// 一条流交给回执的**正文档位**。
///
/// 不截断 ⇒ 只有 [`Self::head`]（那就是全部输出）；截断 ⇒ 首尾两段，**中间那一段是真的缺了**。
struct OutputBody {
    head: String,
    tail: String,
    encoding: String,
    /// 缺的是哪一段（什么都不缺就是 `None`）。
    cut: Option<Cut>,
}

/// 把一条流的两个窗口摆成正文。
///
/// 三条分支的区别只在**中间缺没缺、缺多少知不知道**：
/// - 什么都不缺：两段是连续的（只是分两次读到），**拼接起来解一次**——跨缝的字符因此不会被
///   我们的切法弄坏，编码读数也只判定一次；
/// - 缺中间（读完了、超上限）：两段各自解，尾部窗口开头那半个字符按 [`trim_leading_partial`]
///   剪掉。编码读数取**头部那一次判定**（截断之前本来也只解头部）；
/// - 缺后面（没读完、头部窗口已满）：手里只有头部那一段（尾部窗口只在 `Done` 里来）。
fn render_body(reading: &OutputReading, limit: usize) -> OutputBody {
    let cut = if reading.truncated {
        Some(Cut::Middle)
    } else if !reading.closed && reading.kept.len() >= head_limit(limit) {
        Some(Cut::RestUnknown)
    } else {
        None
    };
    match cut {
        None => {
            let joined: Vec<u8> = reading
                .kept
                .iter()
                .chain(reading.tail.iter())
                .copied()
                .collect();
            let (text, encoding) = decode_bytes(&joined);
            OutputBody {
                head: text,
                tail: String::new(),
                encoding,
                cut: None,
            }
        }
        Some(Cut::Middle) => {
            let (head, encoding) = decode_bytes(&reading.kept);
            let (tail, _) = decode_bytes(trim_leading_partial(&reading.tail));
            OutputBody {
                head,
                tail,
                encoding,
                cut,
            }
        }
        Some(Cut::RestUnknown) => {
            let (head, encoding) = decode_bytes(&reading.kept);
            OutputBody {
                head,
                tail: String::new(),
                encoding,
                cut,
            }
        }
    }
}

/// 把一条流的**正文**渲染进回执（含截断标记；成功与失败两条路共用同一份渲染）。
///
/// **首尾各半**：标记落在两段**中间**，说清缺的是中间那一段与两头的字节数——两头各自读到
/// 的都是真数，只有中间那一段没有。
fn push_output_body(receipt: &mut String, name: &str, body: &OutputBody) {
    receipt.push_str(&format!("--- {name} ---\n"));
    receipt.push_str(&body.head);
    match body.cut {
        None => {}
        Some(Cut::Middle) => receipt.push_str(&format!(
            "\n（此处截断：这一流超过 {MAX_OUTPUT_BYTES} 字节，只给出开头 {} 字节与结尾 {} 字节，\
             中间那一段没有；上面的字节数是这条流的真数）\n",
            head_limit(MAX_OUTPUT_BYTES),
            tail_limit(MAX_OUTPUT_BYTES)
        )),
        Some(Cut::RestUnknown) => receipt.push_str(&format!(
            "\n（此处截断：这次读取到头部窗口 {} 字节为止就交回——命令退出之后这一条流还没有关，\
             后面还有多少未知；上面那些字节是到那一刻为止读到的真数）\n",
            head_limit(MAX_OUTPUT_BYTES)
        )),
    }
    receipt.push_str(&body.tail);
    if !(body.head.is_empty() && body.tail.is_empty()) && !receipt.ends_with('\n') {
        receipt.push('\n');
    }
}

#[cfg(any(windows, target_os = "linux"))]
fn execute(
    access: &TerminalAccess,
    command: &str,
    directory: &Path,
    timeout: Duration,
) -> ActionOutcome {
    run_tree(access, command, directory, timeout)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn execute(
    _access: &TerminalAccess,
    _command: &str,
    _directory: &Path,
    _timeout: Duration,
) -> ActionOutcome {
    // 平台缝：**本平台没有能用的臂**。入口已经拒绝开启这一档（见 `TerminalTier::available`），
    // 因此正常装配根本走不到这里；万一走到，如实说"没跑起来"，绝不换一个近似行为。
    refused(TerminalFailure::SpawnFailure)
}

// ---------------------------------------------------------------------------
// 平台无关的编排（**只写一份**：五堵墙与失败分类都在这里）
// ---------------------------------------------------------------------------

/// 起一个进程、读两条流、按超时收场。
///
/// **编排与五堵墙全在这一份里**：凭据洗、两条流有界读、超时轮询、两条件判定、回执渲染、
/// 失败分类。平台臂的四件（默认 shell 名、怎么拼命令行、怎么击杀进程树、非 UTF-8 怎么解）
/// 住在**共用平台模块**（`crate::platform`，MCP 的通道用它同一份）与本件自己的平台模块
/// （非 UTF-8 解码那一件）里。把编排复制给第二个平台，加第三支臂就得复制第三份墙
/// ——那不是渐进，是把安全逻辑变成多份副本。
#[cfg(any(windows, target_os = "linux"))]
fn run_tree(
    access: &TerminalAccess,
    command: &str,
    directory: &Path,
    timeout: Duration,
) -> ActionOutcome {
    let (mut environment, scrubbed, unscrubbed) = crate::platform::child_environment(&access.scrub);
    let shell = access.shell().unwrap_or(crate::platform::TERMINAL_SHELL);
    let mut builder = crate::platform::terminal_command(shell, command, &mut environment);
    builder
        // 一次性：**不给它 stdin**（没有交互，也没有第二回输入）。
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // ⚠️ 先**清空**再交我们洗过的那一份（规矩写在 `crate::platform::apply_environment` 里，
    // 两个消费者共用同一条）：只 `envs()` 是**叠加**，子进程照旧继承父进程的全部环境
    // ——那样凭据一个都没洗掉，这堵墙就成了装饰（本刀的离线用例抓到过这个）。
    crate::platform::apply_environment(&mut builder, environment);
    builder.current_dir(directory);
    let mut child = match builder.spawn() {
        Ok(child) => child,
        Err(_) => return refused(TerminalFailure::SpawnFailure),
    };
    let Some(stdout) = child.stdout.take() else {
        return refused(TerminalFailure::OutputLost);
    };
    let Some(stderr) = child.stderr.take() else {
        return refused(TerminalFailure::OutputLost);
    };
    // 两条流各起一个**泵**：读到末尾，边走边把前 MAX_OUTPUT_BYTES 字节发回来。
    // 两条流共用一条消息通道 ⇒ 下面收口那一处只有**一个**窗口，不会出现"一条流等完了另一条
    // 还在等"的叠加延迟。
    let (tx, rx) = mpsc::channel::<(OutputSide, OutputMsg)>();
    let out_tx = tx.clone();
    let err_tx = tx.clone();
    // 原件丢掉：通道断开只该由两个泵说了算（留着它，这里会立刻读到 Disconnected）。
    drop(tx);
    thread::spawn(move || pump_output(OutputSide::Out, stdout, MAX_OUTPUT_BYTES, out_tx));
    thread::spawn(move || pump_output(OutputSide::Err, stderr, MAX_OUTPUT_BYTES, err_tx));

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if Instant::now() >= deadline {
            timed_out = true;
            break None;
        }
        thread::sleep(POLL_INTERVAL);
    };

    let killed = if timed_out {
        crate::platform::kill_tree(child.id())
    } else {
        true
    };
    let waited = match status {
        Some(status) => Some(status),
        // 等它真的消失，但**只在窗口内等**：杀不掉就一直等下去会把整场活动挂死——
        // 那种"等不到"本身就是第三态（效果不明），要如实报出来，不能变成一次挂起。
        None => wait_within(&mut child, KILL_WINDOW),
    };

    // **收流**：子进程那一段的判据已经落定（退出码、或击杀与等它消失），这里只等两条流把话
    // 说完——而且**只等一个有界窗口**（[`DRAIN_WINDOW`]）。窗口到点还没关的流如实标成
    // `open-after-exit`：那是第三态，既不是"读失败"，也不是"没有内容"。
    let mut out = OutputReading::default();
    let mut err = OutputReading::default();
    let drain_deadline = Instant::now() + DRAIN_WINDOW;
    while !(out.closed || out.lost) || !(err.closed || err.lost) {
        let left = drain_deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left) {
            Ok((OutputSide::Out, msg)) => out.absorb(msg, MAX_OUTPUT_BYTES),
            Ok((OutputSide::Err, msg)) => err.absorb(msg, MAX_OUTPUT_BYTES),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
        }
    }
    // 读这条流**本身**失败仍然是 `output-lost`（跑起来了却读不到它说了什么）；
    // 而"窗口到点还没关"不是失败——命令的结局早就定了，那一格只是**这次读取覆盖到哪**。
    //
    // ⚠ **代价如实登记**：窗口到点时，那两个泵线程可能还阻塞在 `read()` 上（它们要等写端真的
    // 关闭）。这里**不 join、也不强杀**——它们各自只握着一条管道的读端，攥着它的那个进程收工
    // 就自己结束；而且调用方一放手，它们下一次 `send` 就失败并收工，**不会再攒任何东西**。
    // 替代方案（关读端 / 取消同步 IO）在 Windows 上要么不安全、要么没有可移植形状，
    // 代价比"一个睡着的线程"大。这一格是**已知成本**，不是被忘掉的。
    if out.lost || err.lost {
        return refused(TerminalFailure::OutputLost);
    }
    let stdout_body = render_body(&out, MAX_OUTPUT_BYTES);
    let stderr_body = render_body(&err, MAX_OUTPUT_BYTES);

    // **一份回执，两条路共用**：成功与失败只在头一行与收场那一步不同——正文、读数、截断标记、
    // 凭据读数全是同一份渲染。失败不该比成功少给东西（这就是"失败也带正文"的全部内容）。
    let mut receipt = String::new();
    if timed_out {
        receipt.push_str(&format!("killed = {killed}\n"));
    } else {
        receipt.push_str(&format!("exit = {}\n", exit_reading(waited)));
    }
    push_output_reading(&mut receipt, "stdout", &out, &stdout_body.encoding);
    push_output_reading(&mut receipt, "stderr", &err, &stderr_body.encoding);
    receipt.push_str(&format!("scrubbed = {scrubbed}\n"));
    receipt.push_str(&format!(
        "unscrubbed = {}\n",
        if unscrubbed.is_empty() {
            "none".to_string()
        } else {
            unscrubbed.join(",")
        }
    ));
    if timed_out {
        // **只保证到这里**：`note` 说的是这次判定覆盖到哪一步，不是又一条失败分类。
        receipt.push_str(&format!(
            "note = {}\n",
            if killed && waited.is_some() {
                "进程已中断；这不保证它在被中断前没有改变世界"
            } else {
                "没杀干净：这次执行的效果不明"
            }
        ));
    }
    push_output_body(&mut receipt, "stdout", &stdout_body);
    push_output_body(&mut receipt, "stderr", &stderr_body);

    if timed_out {
        // 击杀成功了 ⇒ 命令中断是**已知形状**（`timeout`）；没杀掉、或者等不到它消失
        // ⇒ 效果不明，如实说 `unknown`（它原来是**未决效果门**要接的第三种状态；门已退役，
        // 门是指令而世界只该给信息——这一态照旧如实报，要不要去看一眼由主体自己决定）。
        // 两条路的**载荷相同**：已经读到的东西照给，差的只是分类与那一行 `note`。
        let class = if killed && waited.is_some() {
            TerminalFailure::Timeout
        } else {
            TerminalFailure::Unknown
        };
        return ActionOutcome::failure_with_content(
            class.as_str(),
            vec![ContentBlock::new(CAPABILITY_RUN, receipt)],
        );
    }
    ActionOutcome::success(vec![ContentBlock::new(CAPABILITY_RUN, receipt)])
}

/// 退出读数（**平台读数差异**，与编码那栏同一类，不是契约分叉）。
///
/// - `waited` 为 `None`（击杀之后窗口内没等到它消失）⇒ `unknown`——不猜；
/// - 退出码读得出来 ⇒ 照报；
/// - POSIX 上被信号杀死：`code()` 为 `None` 而 `signal()` 有值——**可读的事实不许写成
///   `unknown`**，如实报 `signal <n>`（被哪个信号杀的是能读出来的事实）；
/// - Windows 上没有信号这回事：`code()` 读不出来 ⇒ `unknown`（那里的既有读数一字不变）。
#[cfg(any(windows, target_os = "linux"))]
fn exit_reading(waited: Option<std::process::ExitStatus>) -> String {
    let Some(status) = waited else {
        return "unknown".to_string();
    };
    if let Some(code) = status.code() {
        return code.to_string();
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal {signal}");
        }
    }
    "unknown".to_string()
}

/// 在给定窗口内等它真的消失（`None` = 窗口内没等到）。
///
/// 它与"击杀命令报成功"是**两个条件**，合起来才敢说"命令确实被中断了"（`timeout`）；
/// 缺任何一个都只能说"效果不明"（`unknown`）——那正是第三种状态存在的理由。
#[cfg(any(windows, target_os = "linux"))]
fn wait_within(
    child: &mut std::process::Child,
    window: Duration,
) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + window;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {}
            Err(_) => return None,
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// 本件自己的平台模块：臂的**非 UTF-8 解码**那一件（另外三件——默认 shell 名、怎么拼命令行、
/// 怎么击杀进程树——在**共用平台模块** `crate::platform` 里，MCP 的 stdio 通道用它同一份）。
#[cfg(target_os = "linux")]
mod platform {
    use super::ENCODING_NOT_UTF8;

    /// **臂的一件（只有这只手要它）**：本平台没有「本机代码页」这回事——POSIX 的约定就是 UTF-8。
    ///
    /// 走到这里说明字节**不是**合法 UTF-8（那在这类平台上是异常形状），因此如实说"这段不是
    /// UTF-8"，只做有损替换：**不假装知道它是什么编码**。
    ///
    /// **不读 locale**：locale 与 Windows 的 `chcp` 是同一类"任何一条命令都能改掉的可变状态"
    /// ——读它等于把可变状态当真理源。
    pub(super) fn decode_native(bytes: &[u8]) -> (String, String) {
        (
            String::from_utf8_lossy(bytes).to_string(),
            ENCODING_NOT_UTF8.to_string(),
        )
    }
}

/// 本件自己的平台模块（没有臂的平台）：只为**解码读数**留一份如实答复——真正的执行路径
/// 不在这里（见 [`TerminalTier::available`] 与 `execute` 的桩），更不会换近似行为。
#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use super::ENCODING_NOT_UTF8;

    /// **平台缝**：本平台没有能用的臂——不是合法 UTF-8 就说"这段不是 UTF-8"，只做有损替换，
    /// **不假装知道它是什么编码**。
    pub(super) fn decode_native(bytes: &[u8]) -> (String, String) {
        (
            String::from_utf8_lossy(bytes).to_string(),
            ENCODING_NOT_UTF8.to_string(),
        )
    }
}

/// 本件自己的平台模块：臂的**非 UTF-8 解码**那一件（Windows 的读法是本机 OEM 代码页）。
#[cfg(windows)]
mod platform {
    use super::*;

    /// **臂的一件（只有这只手要它）**：把一段不是合法 UTF-8 的字节按**本机 OEM 代码页**解出来。
    ///
    /// 为什么是 OEM 代码页、不是控制台代码页：控制台代码页是**可以被任何一条命令改掉**的状态
    /// （`chcp`），而原生命令输出用的正是这台机器的 OEM 代码页（实测 `ipconfig` 的中文就是
    /// 那台机器 OEM 代码页的字节）。解不出来就如实说 `not-utf8`——**不猜**。
    pub(super) fn decode_native(bytes: &[u8]) -> (String, String) {
        match wide_from_code_page(bytes) {
            Some((wide, code_page)) => (String::from_utf16_lossy(&wide), format!("cp{code_page}")),
            None => (
                String::from_utf8_lossy(bytes).to_string(),
                ENCODING_NOT_UTF8.to_string(),
            ),
        }
    }

    /// 按本机 OEM 代码页把字节转成 UTF-16；拿不到代码页、或转换失败就是 `None`。
    fn wide_from_code_page(bytes: &[u8]) -> Option<(Vec<u16>, u32)> {
        let code_page = win32::oem_code_page()?;
        let wide = win32::multi_byte_to_wide(bytes, code_page)?;
        Some((wide, code_page))
    }

    /// 两个**从 Windows 2000 起签名没变过**的系统调用。
    ///
    /// 这里**手写声明**而不引 `windows-sys`：本 crate 的依赖面是刻意薄的（请求客户端与
    /// 序列化那几件，`tests/minimal_agent.rs` 还有边界守卫），为两个固定签名的函数
    /// 拉进一个生成式大 crate 不划算。代价是签名由我们自己保证——它们只在 `#[cfg(windows)]`
    /// 下编译，并由 `tests/terminal_app.rs` 的真执行用例钉住（那条用例真的把非 UTF-8 字节喂进来，
    /// 看解出来的是不是那两个汉字）。
    mod win32 {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            /// 本机 OEM 代码页（中文 Windows 上是 936）。
            fn GetOEMCP() -> u32;
            /// 多字节 → UTF-16。`flags = 0` 表示**不做严格校验**：非法序列换成默认字符，
            /// 而不是整段失败——我们要的是"尽力解 + 如实报编码"，不是"解不了就什么都没有"。
            fn MultiByteToWideChar(
                code_page: u32,
                flags: u32,
                bytes: *const u8,
                byte_count: i32,
                wide: *mut u16,
                wide_count: i32,
            ) -> i32;
        }

        /// 本机 OEM 代码页；`0` 视为**拿不到**（调用方据此如实说 `not-utf8`）。
        pub(super) fn oem_code_page() -> Option<u32> {
            // 安全：无参数、无指针、只读系统状态。
            let code_page = unsafe { GetOEMCP() };
            (code_page != 0).then_some(code_page)
        }

        /// 按代码页把字节转成 UTF-16。
        ///
        /// 两趟调用是 `MultiByteToWideChar` 的常规用法：第一趟只问长度（目标为空指针、长度 0），
        /// 第二趟的缓冲区**就是**第一趟问出来的那个长度，因此不存在越界写。
        pub(super) fn multi_byte_to_wide(bytes: &[u8], code_page: u32) -> Option<Vec<u16>> {
            if bytes.is_empty() {
                return Some(Vec::new());
            }
            let byte_count = i32::try_from(bytes.len()).ok()?;
            // 安全：源切片有效、长度如实给出；目标为空指针 + 长度 0 ⇒ 这一趟只读不写。
            let needed = unsafe {
                MultiByteToWideChar(
                    code_page,
                    0,
                    bytes.as_ptr(),
                    byte_count,
                    std::ptr::null_mut(),
                    0,
                )
            };
            if needed <= 0 {
                return None;
            }
            let mut wide = vec![0u16; needed as usize];
            // 安全：目标缓冲区长度**就是**上一趟返回的 `needed`，源长度与 `byte_count` 一致。
            let written = unsafe {
                MultiByteToWideChar(
                    code_page,
                    0,
                    bytes.as_ptr(),
                    byte_count,
                    wide.as_mut_ptr(),
                    needed,
                )
            };
            if written <= 0 {
                return None;
            }
            wide.truncate(written as usize);
            Some(wide)
        }
    }
}

impl ActionPort for TerminalApp {
    fn capabilities(&self, _agent: &AgentId, _run: &RunId) -> Capabilities {
        Self::own_capabilities()
    }

    /// 局部准入：本层一律放行。
    ///
    /// 这里没有要判的东西：**档位就是授权本体**（在场 = 被授权）。⚠ 原来这里接着写的是"写效应那一格
    /// 由 OS 的未决效果门按声明类别管（本件声明的是写 ⇒ 有未决时它照旧被拦）"——**门已退役**
    /// （门是指令，而世界只该给信息）：本件**照旧声明**它是写（见 [`AppPort::declared_effects`]），
    /// 声明自洽性检查照旧在装配期跑，只是不再有一道门按类别拦写。
    fn admit(&self, _agent: &AgentId, _run: &RunId, action: &Action) -> ActionAdmission {
        match action.id().as_str() {
            CAPABILITY_RUN | CAPABILITY_HELD | CAPABILITY_STOP => ActionAdmission::Admitted,
            _ => ActionAdmission::refused(crate::core::UNKNOWN_CAPABILITY),
        }
    }

    fn execute(
        &mut self,
        _agent: &AgentId,
        _operation: &OperationId,
        action: &Action,
    ) -> ActionOutcome {
        match action.id().as_str() {
            CAPABILITY_RUN => self.run(action.params()),
            CAPABILITY_HELD => self.held_reading(),
            CAPABILITY_STOP => self.stop_it(action.params()),
            _ => refused(TerminalFailure::InvalidArgument),
        }
    }
}

impl AppPort for TerminalApp {
    fn app_id(&self) -> &str {
        TERMINAL_APP_ID
    }

    fn description(&self) -> &str {
        "在这台机器上执行命令；`keep=true` ⇒ 这件要活到关家（输出落文件、记进账，`terminal_held` 看、`terminal_stop` 停）。档位就是授权：`full` 档在场即整机随你用，不做命令内容判断；限制全是机械的——洗凭据、超时尽力杀进程树、输出有界且截断带标记。没有 OS 沙箱，也不假装有"
    }

    fn declared_capabilities(&self) -> Capabilities {
        Self::own_capabilities()
    }

    /// 一条写效应，**而且是一个显式的类别偏差**。
    ///
    /// `terminal_run` 是 [`EffectClass::UnverifiableWrite`]：它确实改变世界，但系统里**没有**
    /// 一条同参只读复核能回答"那次执行发生了没有"（再跑一遍不是复核，是新的执行）。因此它
    /// **不配复核能力**——这也是它必须自成一类的原因（写类别默认要求配复核，见装配期检查）。
    ///
    /// 代价如实登记（TASK_36 报告）：这一档没有同参复核可配 ⇒ `unknown` **机械上解除不了**。
    /// ⚠ 原来这里接着写的是"最后一条出路是宿主显式 acknowledge"——那是**未决效果门**的出口，
    /// 门已退役（门是指令，而世界只该给信息）：今天这一格是"那次执行没有结果"的如实读数，
    /// 由主体自己去看（账本 `trace.ndjson`）。
    fn declared_effects(&self) -> Vec<EffectDeclaration> {
        vec![
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_RUN),
                EffectClass::UnverifiableWrite,
                None,
            ),
            // `terminal_stop` 同款：它确实改变世界（收掉一棵进程树），而"那次收干净了没有"
            // **没有**一条同参只读复核能回答——再收一次不是复核，是又一次收。
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_STOP),
                EffectClass::UnverifiableWrite,
                None,
            ),
        ]
    }

    /// 来源档 = **这一段回执里携带的内容从哪个口子进来**。
    ///
    /// 命令输出是**世界写的、没有任何署名**的一段文本：它可能来自任何程序、任何对端。
    /// 因此它是 [`crate::app_contract::SourceClass::WorldUnsigned`]——本系统里最严的那一档，
    /// 也是"自己去拉回来的外部内容"的默认档。
    fn declared_sources(&self) -> Vec<SourceDeclaration> {
        self.declared_capabilities()
            .items()
            .iter()
            .map(|capability| {
                SourceDeclaration::new(capability.id().clone(), SourceClass::WorldUnsigned)
            })
            .collect()
    }
}
