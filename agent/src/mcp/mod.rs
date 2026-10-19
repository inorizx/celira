//! MCP 适配：**外部来源的 App**（TASK_42 第一刀，只做 stdio）。
//!
//! 市面上的 MCP server 是一台**被授权的 App**：宿主在开机时用一个旗标粗粒度声明它
//! （`--mcp <名>=stdio:<命令>`），主体要用的时候再**展开**把它请出来（展开是 OS 的动词，
//! 本层不认识那个动词——它只知道"有人请我把能力交出来"）。这一层把"外面那台 server 暴露的
//! 动词"接成我们自己的动词面——与出厂内置件（文件、笔记、im、web、terminal）走的是**同一份
//! 窄契约**（[`crate::app_contract::AppPort`]），因此 OS 一行不用认识 MCP。
//!
//! ## 三件事与本层的分工
//!
//! ```text
//! 宿主（命令行）    —— 声明：名字、命令、效果档、超时、要洗的凭据键名
//! 本层（App）      —— 动词面：名字、骨架、参数表、效果类别、回执；墙全部在本层
//! 通道（transport）—— 只做三件事：connect（起进程 + 握手）/ list_tools / call_tool
//! ```
//!
//! 通道被抽成一个**窄 trait**（[`Transport`]）：v1 只有 stdio 一个实现（[`stdio::StdioTransport`]），
//! 但"换一条通道"从此只是换一个实现——**不为想象中的远程 server 预建实现**（走网络那种通道
//! 不在本刀，真出现那天再说）。
//!
//! ## 授权 = 装配期事实（装/卸要重启宿主）
//!
//! `--mcp` 一给，这台 server 就进了**授权面**：展开动作认得这个名字。不给就是没装——
//! **零**（子进程不起、清单不拉、一个字节的动词都不进主体的视野）。这是档位门控哲学，与
//! `--terminal` 同构：授权是装配期的事实，**Run 中途热装 = 绕授权**，所以不做（换授权面就重启宿主，
//! 那是特性不是缺陷）。
//!
//! 一个 server 一授权，**不逐工具弹窗**：主体的自治在动词面之内（主体决定调哪个工具、给什么参数），
//! 边界由宿主一次性划清。
//!
//! ## 懒展开（词元经济的全部答案）
//!
//! 声明 `--mcp` 之后：**子进程不起、清单不拉**——主体能看到的只有入口里那一行"装了这台"。
//! 真到**展开那一下**才：起进程 → 握手 → 拉清单 → 回执是门户卡片（动词骨架清单）。
//! 从**下一轮**起那台 server 的动词骨架进 `tools` 数组，`inspect_action` 取详情照走。
//!
//! 子进程是**宿主级**的：首次展开起、宿主退出收（[`stdio::StdioTransport`] 的 `Drop` 收整棵树）。
//! 展开状态本身仍然归**这一次活动**（OS 的 `RunExpansion`）——所以下一次 Run 主体要**再说一次**
//! "我要用它"，而通道（进程）不必重建。
//!
//! ⇒ **"临时接入"不是含糊状态，是三个精确的落点**：身份（装配期）／在场（那一次活动）／通道
//! （宿主级）。因此它**不构成"它不该是 App"的理由**——App 契约本来就有"在场"那一格
//! （展开／`RunScope::is_open`）。
//!
//! ## 效果档（宿主显式背书，只有两档）
//!
//! ```text
//! --mcp <名>=stdio:<命令>              ⇒ unverifiable-write（最严：默认档）
//! --mcp <名>=stdio:<命令>,read-only    ⇒ read-only（宿主显式背书"这台只读"）
//! --mcp <名>=stdio:<命令>,timeout=<秒> ⇒ 转发窗口（1–600，缺省 120）
//! ```
//!
//! ⚠ **两个后缀都在时 `read-only` 收尾**（`…,timeout=30,read-only`）：解析是**从末尾逐条剥**，
//! 写反会**静默**落回最严那档（见 [`parse_declaration`]）。
//!
//! **默认档是最严的**：外来工具的写效应**无法同参复核**（再调一次不是复核，是又一次调用），
//! 因此它落在 [`crate::app_contract::EffectClass::UnverifiableWrite`]——TASK_36 为 terminal 立的
//! 第四类，本刀**不加新枚举值**。代价照旧：这一档没有同参复核可配 ⇒ **没有机械的解除手段**。
//! ⚠ 这里原来写的是"`unknown` 一旦挂上未决、最后一条是宿主显式 acknowledge"——那是**未决效果门**
//! 的说法，门已退役（门是指令，而世界只该给信息）：今天这一格是"那次写没有结果"的如实读数，
//! 要不要去看一眼由主体自己决定。
//!
//! `read-only` 是**宿主的一句话**，不是我们替 server 判断出来的：它只说"这台不会改外面的世界"。
//! 它有两条机械后果：① 那些动词**不声明写效应**（⚠ 原来写的是"不过未决效果门"——门已退役，
//! 判据回到**声明**本身）；② 通道断在调用中间时，只读档**不会**落进
//! `unknown`（读不改变世界，没有"可能已经发生"这一格）——见下面的失败形状。
//!
//! ## 调用路径（转发 + 回执墙 + 词表 + 洗凭据）
//!
//! 1. **转发**：JSON-RPC `tools/call`，窗口由声明给出（缺省 120 秒、上限 600——慢工具的世界，
//!    terminal 先例；逐动词还会挂一个可选的 `timeout` 旋钮，见 [`app`]）；
//! 2. **回执墙**：正文**读的时候就有界**（单次 ≤64 KiB，超限截断并带明确标记），非文本内容
//!    （图片、音频、资源引用……）**如实报类型与条数、不透传**——这一层就是登记过的
//!    "工具输出兜底"落在这件 App 上的那一格；
//! 3. **词表封闭**（见 [`McpFailure`]）：对面的错误码与原文**不进**我们的词汇；
//! 4. **凭据洗**：子进程环境按前缀（`CELIRA_`）与宿主点名的键名洗一遍，
//!    **先清空再交洗过的那一份**（只叠加等于没洗——TASK_36 的离实用例抓到过这堵假墙）。
//!
//! ## 平台缝：契约唯一、实现多份
//!
//! 通道实现是平台缝，形状与 terminal 那只手**同一份**：编排（三件事、四堵墙、收树判定）
//! 只写一份，平台差异收进 [`crate::platform`] 的**臂**里——**臂只留四件**（默认 shell 名、
//! 怎么拼命令行、怎么击杀进程树、凭据怎么洗）。今天两支臂：Windows＝这台机器自带的命令行
//! 解释器（`cmd.exe /C`，理由见 [`stdio`] 的模块文档），Linux＝bash（`bash -c`，子进程自持
//! 进程组、按组击杀）。**别的平台上入口拒绝开启** `--mcp`，本层也没有实现——不静默换一个
//! 近似行为。注意 [`stdio_available`] 问的是「**本构建里有没有能用的臂**」（**编译期**事实），
//! 它与「机器处境」那行的**运行时**读数不是一回事，不能互相顶替。
//!
//! ## 一处**已经收口**的取舍（原样留档）
//!
//! ⚠ **2026-10-13 补第二格**（本节原来写的是"原文没有落脚处"，那一句**今天不成立**）：
//!
//! 对面回**错误对象**（JSON-RPC 的 `error`）时，分类进我们的词表（`invalid-argument` /
//! `server-error` / `unsupported-version` / `handshake-failure`），而**原文今天有落脚处**：
//!
//! | 路径 | 原文住在哪 |
//! |---|---|
//! | **调用**（`call_tool`） | `CallReply::Refused { detail }` → 回执印 `error-detail = …`（一直在） |
//! | **展开 / 握手**（`connect`） | `StdioTransport::refusal_detail` → `ExpansionRefusal::with_detail` → OS 贴成随单内容 |
//!
//! 两处**同一条纪律**：分类是**词表**（逐字节稳定，落盘），原文是**内容**（只作携带，已消毒已截断）。
//! **对面没留话 ⇒ 那一格整块不出场**（"没有"与"读不到"分开表达，不写空值）。
//!
//! 判据不是"信任纪律"，是**记录诚实**：外来的话若不加标明地进记录，它看起来就像我们自己的读数。
//! 见 `outputs/IRREPLACEABLE_RECORD_RULING_DRAFT_2026-10-13.md` §3（**够得着 · 留得下 · 记诚实**）。
//!
//! 规模上它仍是罕见路径：工具级失败（"元素找不到"这类）走的是 `isError` 读数，
//! 那一路是**成功 + 读数**，原文照旧在主体的回执里（见 [`app`] 的收据渲染）。
//!
//! ## 「外部来源的 App」是一个类别，不只是一种来源（2026-10-06 登记）
//!
//! MCP server 是这一类别的**第一台**。它与出厂内置件的根本区别**不在"怎么接"**，而在
//! **实现不在我们手里**——我们写的是壳（[`app`] 的动词面 ＋ 本模块的墙），背后跑的是
//! **别人写的进程**。⇒ 这一类要九件额外纪律，一件都不能省：
//!
//! **授权** / **效果档** / **凭据隔离** / **有界回执** / **词表封闭** / **进程树收干净** /
//! **"发出去之后不知道"那一态**（[`CallFault::Indeterminate`]）/ **外部标签卫生** /
//! **名字合法性**。
//!
//! ⚠ **九件里有两件的实现不在本模块**：**凭据隔离**（[`crate::platform::child_environment`]
//! ＋ `apply_environment`）与**收树那一手**（[`crate::platform::kill_tree`]；"收干净了没有"
//! 那半在 [`stdio`] 的 `collect`）。理由不是"分层好看"，是**同一堵凭据墙有两份实现，两份
//! 迟早对不上**。
//!
//! **其余七件住在这里**（[`app`] 的名字与清单检查、[`sanitize`] / [`clip`] / [`foreign_line`]
//! 的标签卫生、四道墙），**而它们与协议无关**——[`sanitize`] / [`clip`] / [`foreign_line`]
//! 甚至只是 `pub(crate)`：今天的消费者只有本模块。
//!
//! ⇒ **第二个外部来源进来那天，这几件该提到中立件**；**今天不动是对的**（不变量 7：没有真实
//! 消费者和验收路径时，不预建高阶器官）。**这一格的判据不是"该不该重构"，是"第二个外部来源
//! 来了没有"。**
//!
//! ## 边界：Core / Foundation 不许认识本模块
//!
//! 「外面那台 server 暴露了什么工具」是**外部内容**，不是执行语义 ⇒ `mcp` 在
//! `tests/minimal_agent.rs` 的 `BOUNDARY_MODULES` 里。它是**第一个子目录模块**，因此
//! `core_sources` 把路径分隔符归一成 `/`——名单只写一种形状，换一台机器不需要第二份名字。

use crate::app_contract::EffectClass;
use crate::core::ActionOutcome;

pub mod app;
pub mod stdio;

pub use app::McpApp;
pub use stdio::StdioTransport;

/// 转发窗口的缺省值（秒）。与 terminal 同一量级：这是"慢工具的世界"。
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 120;

/// 转发窗口的硬上限（秒）：声明可以调，但**调不过它**。
pub const MAX_TIMEOUT_SECONDS: u64 = 600;

/// 单次调用搬回来的正文上限（字节）：超限**截断并带标记**。
///
/// 它是"读的时候就有的界"：不是读回来再砍（那要求先把无界正文读进内存），而是读到这个数就不再
/// 收——**显式截断合法，静默截断违禁**，所以回执里照样给出原文总字节数与截断标记。
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

/// 一台 server 最多能上多少条动词：超限**整台不上场**（如实报分类）。
///
/// 它是"每轮都要摆进主体的视野的动词面"的机械上限，不是一个调优参数。真实 server 远低于它。
pub const MAX_TOOLS: usize = 128;

/// 骨架（每一轮都在 `tools` 数组里）的字节上限：污染与长度都在这里被机械处理。
pub const MAX_SKELETON_BYTES: usize = 200;

/// 单项参数说明的字节上限。
pub const MAX_PARAM_DOC_BYTES: usize = 120;

/// 逐动词可选旋钮的名字（只在对面自己没有同名参数时挂上，见 [`app`]）。
pub const PARAM_TIMEOUT: &str = "timeout";

/// **本构建**里有没有 stdio 这一档的实现（平台缝 fail-closed：没有 ⇒ 入口拒绝开启 `--mcp`）。
///
/// 这是**编译期**判断——它回答"这一份二进制里编进了哪些臂"（今天两支：Windows 与 Linux）；
/// 与「机器处境」（`machine.rs`：**运行时**从机器现读它是什么）**不是一回事**，两者不能互相顶替。
pub fn stdio_available() -> bool {
    cfg!(any(windows, target_os = "linux"))
}

/// 宿主给一台 server 的**效果档**（封闭枚举，两档）。
///
/// 档位是**声明**不是判断：`read-only` 是宿主替这台 server 说的一句话，本层不校验它
/// （"我是不是真的只读"是行为事实，不是可以机械封闭的一格——见 README 的缺口清单）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpEffect {
    /// 宿主显式背书"这台不改外面的世界"。
    ReadOnly,
    /// 默认档：外来工具确实改变世界，但没有同参复核可配（最严）。
    UnverifiableWrite,
}

impl McpEffect {
    /// 映射到**既有**的效果类别（值不加新枚举——`unverifiable-write` 是 TASK_36 已有的）。
    pub fn class(self) -> EffectClass {
        match self {
            McpEffect::ReadOnly => EffectClass::ReadOnly,
            McpEffect::UnverifiableWrite => EffectClass::UnverifiableWrite,
        }
    }

    /// 稳定标记（读数与入口横幅用它）。
    pub fn as_str(self) -> &'static str {
        self.class().as_str()
    }

    /// 是不是**写**档（决定"通道断在调用中间"落进哪一格）。
    pub fn is_write(self) -> bool {
        self.class().is_write()
    }
}

/// 本件自己的失败词汇表（**封闭**）。
///
/// 纪律与其它 App 逐字同款：稳定字符串是词汇表、`as_str()` 权威，载荷存分类不存详情，
/// 词表不跨层。**对面的错误码与原文不进这里**——它们要么被归类成下面的某一个，
/// 要么只作读数（`isError`）出现在回执里。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpFailure {
    /// 参数不合法：缺必需参数、出现不认识的参数名、取值还原不成对面要的类型、
    /// 超时不合法、这条动词的参数形状我们表达不了。**本地拒绝、零转发**。
    ///
    /// ⚠ 它是**一个词**（逐字节稳定）；**哪一项错了、期望什么形状**是对面旁边的诊断
    /// （`mcp::app::ArgumentRefusal` → `parameters` 那一块），**不揉进这个词里**。
    InvalidArgument,
    /// 到点没答复（而且我们把它收干净了）。
    Timeout,
    /// 通道不在了：进程死了、管道断了、写不进去、连接压根没建起来。
    Transport,
    /// 首次展开时**压根没跑起来**（起进程本身失败，或本平台没有这一档的实现）。
    SpawnFailure,
    /// 握手没成：`initialize` 被拒，或答复读不懂。
    HandshakeFailure,
    /// 清单拉不到：`tools/list` 报错，或回执读不懂。
    ToolsFailed,
    /// 对面回了一个**错误对象**（JSON-RPC 的 `error`）。**不分解**它的码（码只用来在两格之间
    /// 选一个，见 [`app`] 的 `classify_error_code`）；原文**只作携带**、不进词表
    /// （[`CallReply::Refused`] 的 `detail` → 回执的 `error-detail`，见模块文档的取舍那一节）。
    ServerError,
    /// 至少有一条动词的名字加上 server 名前缀会超过工具名上限。
    ///
    /// 它是**整台不上场**的原因之一，而不是"跳过那一条"：名字超限的动词一旦上场，
    /// 每次请求都会在出口被拒——那时坏的就不只是那一条动词了。
    NameTooLong,
    /// 清单条数超过 [`MAX_TOOLS`]：整台不上场（不静默截断动词面）。
    TooManyTools,
    /// 这一条动词的参数形状我们表达不了（`null`、认不出的类型名、**两种不同类型拼起来的联合**）：
    /// **这条动词照旧在场**，但调用时如实回绝。理由与 `web_search` 缺凭据同款：
    /// 让"有这条但用不了"看得见，比让它安静地消失诚实。
    ///
    /// ⚠ **2026-10-13 射程收窄**：**对象与数组不再归这里**——它们从"整条动词作废"变成"能用"
    /// （内部按一段 JSON 文本搬，见 `mcp::app::JsonKind::Json`）。判据是主人那句
    /// 「**它的边界是存在的，不是我们要管的**」：从前那种做法是拿我们的手型去量对面的手。
    UnsupportedSchema,
    /// **对面只会说比我们会说的更新的协议**：探测确认它是 modern 代，而它支持的版本里
    /// 没有我们会说的那一个。
    ///
    /// 它与 [`McpFailure::HandshakeFailure`] **必须分开**：后者是"握手没成、原因不明"，
    /// 这一格是"**握手没成，而且知道为什么**"。分不开的代价是实测过的形状——
    /// 常驻宿主那一侧曾经把四类完全不同的失败塌成同一个读数，那正是
    /// "必须保留影响调用方决策的类别"要挡的。
    ///
    /// ⚠ 它是**我们这边的分类**，不是对面的原文。对面在拒绝 `initialize` 时 SHOULD 列出
    /// 它支持的版本，而那句原文**今天的落脚处是展开失败的随单内容**
    /// （`app_contract::ExpansionRefusal` → `os::expand` → `ActionOutcome::failure_with_content`，
    /// 见 `agent/README.md` §3.93）——这一格本身仍只存分类。
    UnsupportedVersion,
}

impl McpFailure {
    /// 稳定分类标记（进 `ActionOutcome`，Core 只搬运不解释）。
    pub fn as_str(self) -> &'static str {
        match self {
            McpFailure::InvalidArgument => crate::core::INVALID_ARGUMENT,
            McpFailure::Timeout => "timeout",
            McpFailure::Transport => "transport",
            McpFailure::SpawnFailure => "spawn-failure",
            McpFailure::HandshakeFailure => "handshake-failure",
            McpFailure::ToolsFailed => "tools-failed",
            McpFailure::ServerError => "server-error",
            McpFailure::NameTooLong => "name-too-long",
            McpFailure::TooManyTools => "too-many-tools",
            McpFailure::UnsupportedSchema => "unsupported-schema",
            McpFailure::UnsupportedVersion => "unsupported-version",
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
            crate::core::INVALID_ARGUMENT => McpFailure::InvalidArgument,
            "timeout" => McpFailure::Timeout,
            "transport" => McpFailure::Transport,
            "spawn-failure" => McpFailure::SpawnFailure,
            "handshake-failure" => McpFailure::HandshakeFailure,
            "tools-failed" => McpFailure::ToolsFailed,
            "server-error" => McpFailure::ServerError,
            "name-too-long" => McpFailure::NameTooLong,
            "too-many-tools" => McpFailure::TooManyTools,
            "unsupported-schema" => McpFailure::UnsupportedSchema,
            "unsupported-version" => McpFailure::UnsupportedVersion,
            _ => return None,
        })
    }
}

/// 一条 `--mcp` 声明**解析出来**的东西（宿主面的输入，还没有授权任何东西）。
///
/// 名字是动词前缀：`<名字>_<工具名>`。名字由宿主给，因此它得满足我们这边的名字规矩
/// （见 [`parse_declaration`]）——那是**入口能在起任何东西之前就拒掉**的一件事。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpDeclaration {
    name: String,
    command: String,
    effect: McpEffect,
    timeout_seconds: u64,
}

impl McpDeclaration {
    /// server 名（动词前缀，也是 App 标识）。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 要跑的命令行（交给平台默认 shell，与 terminal 的"命令词法随 shell"同款）。
    pub fn command(&self) -> &str {
        &self.command
    }

    /// 宿主给的效果档。
    pub fn effect(&self) -> McpEffect {
        self.effect
    }

    /// 转发窗口（秒）。
    pub fn timeout_seconds(&self) -> u64 {
        self.timeout_seconds
    }
}

/// 解析 `--mcp` 的一条声明；失败给**一个不带取值的稳定短语**（入口直接打出来）。
///
/// 形状：`<名>=stdio:<命令>[,timeout=<秒>][,read-only]`——⚠ **`read-only` 收尾**：两个可选项
/// 是**从末尾逐条剥**下来的（先剥 `,read-only`，再找 `,timeout=`），不是按逗号切（那样会把
/// 命令里的逗号也切掉）。
///
/// 两条机械规矩：① 只认 `stdio:` 这一种通道——认不出来的前缀**不猜**；
/// ② 名字必须是我们这边的合法前缀（ASCII 字母数字下划线、首字符是字母、且给工具名留得下位置），
/// 否则这台 server 的动词名必然越界或读不出来，那时拒绝启动比让主体拿到一堆用不了的动词好。
///
/// ⚠ **写反顺序会静默**（`…,read-only,timeout=30`）：`,read-only` 在末尾剥不下来 ⇒ 它被当成
/// **命令的一部分**交给 shell，而效果档**静默**落回最严的 `unverifiable-write`。方向是安全的
/// （更严的一档），但那不是声明者要的档，而且命令里多了一段。**如实登记：这一种今天没有用例
/// 钉着**——它是"读解析器读出来的"，不是"测出来的"。
pub fn parse_declaration(text: &str) -> Result<McpDeclaration, String> {
    let Some((name, rest)) = text.split_once('=') else {
        return Err("声明缺少 `=`：形状是 `<名>=stdio:<命令>`".to_string());
    };
    let Some(command) = rest.strip_prefix("stdio:") else {
        return Err("只认得 stdio 这一种通道：声明要以 `stdio:` 开头".to_string());
    };

    // 尾巴上的两个可选项：从**末尾**逐条剥（不按逗号切，那样会把命令里的逗号也切掉）。
    let (command, effect) = match command.strip_suffix(",read-only") {
        Some(head) => (head, McpEffect::ReadOnly),
        None => (command, McpEffect::UnverifiableWrite),
    };
    let (command, timeout_seconds) = match command.rsplit_once(",timeout=") {
        Some((head, raw)) => {
            let seconds: u64 = raw
                .parse()
                .map_err(|_| "`timeout=` 的值不是整数".to_string())?;
            if !(1..=MAX_TIMEOUT_SECONDS).contains(&seconds) {
                return Err(format!("`timeout=` 必须在 1–{MAX_TIMEOUT_SECONDS} 之间"));
            }
            (head, seconds)
        }
        None => (command, DEFAULT_TIMEOUT_SECONDS),
    };

    if name.is_empty() {
        return Err("server 名不能为空".to_string());
    }
    let mut characters = name.chars();
    let first = characters.next().unwrap_or('0');
    if !first.is_ascii_alphabetic()
        || !name
            .chars()
            .all(|each| each.is_ascii_alphanumeric() || each == '_')
    {
        return Err(
            "server 名只能是 ASCII 字母数字下划线、且以字母开头（它是动词名的前缀）".to_string(),
        );
    }
    // 至少给工具名留一个字符：留不下就是这台 server 的动词必然越界。
    if !verb_fits(name, "x") {
        return Err("server 名太长：加上工具名会超过工具名上限".to_string());
    }
    if command.trim().is_empty() {
        return Err("命令不能为空".to_string());
    }

    Ok(McpDeclaration {
        name: name.to_string(),
        command: command.to_string(),
        effect,
        timeout_seconds,
    })
}

/// 工具名的长度上限（**权威在适配层**：那是 wire 事实）。
///
/// 本层只**读**它一次（不另抄一个数字：同一事实两份权威迟早分叉），而且**隔离在这一个函数里**
/// ——动词面那一半（`app`）只问"这个名字放不下"，不问这个数从哪儿来。
fn name_limit() -> usize {
    crate::provider_adapter::TOOL_NAME_MAX
}

/// `<server 名>_<工具名>` 放得下吗。
pub(crate) fn verb_fits(server: &str, tool: &str) -> bool {
    server.len() + 1 + tool.len() <= name_limit()
}

/// 宿主给一台 server 的**访问声明**：解析出来的那几样 + 要洗的凭据键名 + **宿主级的通道持有**。
///
/// 通道持有放在 [`std::sync::Arc`] 里，因为本类型会被**每一次活动的装配各克隆一份**（那是既有
/// 装配形状），而子进程必须是宿主级的：第一次展开起，直到宿主退出。本类型**不派生 `Debug`**，
/// 也**永远不携带任何取值**（洗凭据的读数只报键名、连命令原文都不打印在库里）。
#[derive(Clone)]
pub struct McpAccess {
    declaration: McpDeclaration,
    scrub: Vec<String>,
    state: std::sync::Arc<SharedTransport>,
}

/// 一台 server 在**宿主进程里**的那一格：通道。
///
/// `None` = 还没连（首次展开才连），或者**上一次它坏了**（我们把它收了，不自动重连）。
/// 它跨活动存活（"第一次展开起、宿主退出收"），而"这一次活动有没有展开过"**不在这里**——
/// 那是 OS 的 `RunExpansion`，每一次 Run 各自从空开始。
pub(crate) type SharedTransport = std::sync::Mutex<Option<Box<dyn Transport>>>;

impl McpAccess {
    /// 用一份解析好的声明与要洗的键名建立访问声明（通道还没建）。
    pub fn new(declaration: McpDeclaration, scrub: Vec<String>) -> Self {
        Self {
            declaration,
            scrub,
            state: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// 用一份**已经准备好的通道**装配（只有测试会用它）。
    ///
    /// 通道被抽成窄 trait 的全部意义就在这里：墙上的每一条（回执截断、非文本、词表、
    /// 参数还原）都能在没有进程的情况下逐条验证，而不必去造一个能演全部坏形状的 server。
    pub fn with_transport(
        declaration: McpDeclaration,
        scrub: Vec<String>,
        transport: Box<dyn Transport>,
    ) -> Self {
        Self {
            declaration,
            scrub,
            state: std::sync::Arc::new(std::sync::Mutex::new(Some(transport))),
        }
    }

    /// 宿主面的声明。
    pub fn declaration(&self) -> &McpDeclaration {
        &self.declaration
    }

    /// 共享的那一格（本模块内使用）。
    pub(crate) fn state(&self) -> &std::sync::Arc<SharedTransport> {
        &self.state
    }

    /// 要洗的凭据键名（本模块内使用）。
    pub(crate) fn scrub(&self) -> &[String] {
        &self.scrub
    }

    /// 现在就起子进程的话，会洗掉几个、有没有洗不掉的。
    ///
    /// 宿主用它把这条读数打在**开机横幅**上（那时还没起任何子进程）：读数本来就是"这台机器上
    /// 有几支凭据会被挡住"，与"什么时候起进程"无关，而"起子进程"是这台 server 被真用起来
    /// 才发生的事。真正的读数（真的交出去的那一份再查一遍）在
    /// [`crate::platform::child_environment`] 里，**同一个判据**——不是两份实现
    /// （terminal 那只手用它同一份）。
    pub fn credential_reading(&self) -> (usize, Vec<String>) {
        credential_reading(&self.scrub)
    }
}

/// 按判据数一遍当前进程的环境：`(洗掉几个, 洗不掉的键名)`。
///
/// **洗不掉的如实报**是拿"真正要交出去的那一份"再查一遍得出的（不是"我们以为干净"）。
/// 取值一个字节都不落。判据本体在 [`crate::platform`]（与 terminal 共用一份）。
pub(crate) fn credential_reading(declared: &[String]) -> (usize, Vec<String>) {
    let (_, scrubbed, unscrubbed) = crate::platform::child_environment(declared);
    (scrubbed, unscrubbed)
}

/// 通道的**窄接口**：三个方法，不多不少。
///
/// 它只说"怎么跟对面说话"，不知道任何 App 的事；[`McpApp`] 只说"暴露哪些动词、墙在哪里"，
/// 不知道对面是 stdio 还是别的。两个方向都只有一个实现，因此这条缝今天不花任何运行代价。
pub trait Transport: Send {
    /// 起通道并握手（stdio：起进程 + `initialize`）。
    fn connect(&mut self, timeout: std::time::Duration) -> Result<(), CallFault>;

    /// 拉清单（stdio：`tools/list`，跟着游标翻页，翻到 [`MAX_TOOLS`] 为止）。
    fn list_tools(&mut self, timeout: std::time::Duration) -> Result<Vec<RawTool>, CallFault>;

    /// 转一次调用。
    fn call_tool(
        &mut self,
        tool: &str,
        arguments: serde_json::Value,
        timeout: std::time::Duration,
    ) -> Result<CallReply, CallFault>;

    /// 这条通道自己起的那个子进程的洗凭据读数（`(洗掉几个, 洗不掉的键名)`）。
    ///
    /// 它不是第四个协议步骤，只是一格**读数**：不起进程的通道（测试用的那种）返回 `None`，
    /// 回执里就如实写"这条通道不起子进程"。
    fn credential_reading(&self) -> Option<(usize, Vec<String>)> {
        None
    }
}

/// 通道上的一次故障。
///
/// 两种，而且**不能压成一种**：一种是"确定没成"（分类进词表），一种是"发出去之后不知道"——
/// 后者就是 [`CallFault::Indeterminate`] 这一态（⚠ 它原来是**未决效果门**要接的第三态；门已退役，
/// 门是指令而世界只该给信息，这一态本身照旧如实报）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallFault {
    /// 明确失败：分类稳定（[`McpFailure`]）。
    Refused(McpFailure),
    /// **效果不明**：请求已经交出去了，可我们既没拿到答复、也没能把对面收干净。
    Indeterminate(McpFailure),
}

impl CallFault {
    /// 分类标记（"效果不明"也带着它观察到的、最贴切的那一个）。
    pub fn class(&self) -> McpFailure {
        match self {
            CallFault::Refused(class) | CallFault::Indeterminate(class) => *class,
        }
    }

    /// 这次故障**确定了没有**（不确定 = 效果不明）。
    pub fn is_indeterminate(&self) -> bool {
        matches!(self, CallFault::Indeterminate(_))
    }
}

/// 对面清单里的一条工具（**还没进我们的动词面**：名字要加前缀、参数表要过机械映射）。
#[derive(Clone, Debug)]
pub struct RawTool {
    /// 对面的工具名（原文）。
    pub name: String,
    /// 对面给的一句话说明（**外来文本**：进视野之前要过卫生与长度）。
    pub description: Option<String>,
    /// 对面的参数模式原文（外部冻结形状；我们只按机械规则读得懂的那部分）。
    pub input_schema: serde_json::Value,
}

/// 一次调用的答复（已经是**有界**的形状：正文在通道里就被截断）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallReply {
    /// 对面答复了。`is_error` 是**它的读数**（工具自己说"这事没成"），不是我们的失败分类——
    /// 与 terminal 的退出码同款：**转发出去了、答复回来了就是成功**。
    Replied {
        /// 对面标的 `isError`。
        is_error: bool,
        /// 拼好的正文（已在 [`MAX_TEXT_BYTES`] 处截断）。
        text: String,
        /// 原文一共多少字节（截断前的读数）。
        text_bytes: usize,
        /// 有没有截断。
        truncated: bool,
        /// 我们**没搬**的那些内容块：`(类型名, 块数)`，按类型名排序。
        unsupported: Vec<(String, usize)>,
        /// 这条通道上读进来但**不是一个对象**的行数（0 = 干净；噪声进不了我们的协议）。
        noise: usize,
    },
    /// 对面回了一个**错误对象**：分类由调用方定（`code` 只用来在两格之间选一个，
    /// **它本身不进我们的词汇**），原文只作携带（`detail`）。
    Refused {
        /// 错误对象里的码（用来归类，不搬运）。
        code: Option<i64>,
        /// 错误对象里的原文（已消毒、已截断）。**它不进我们的词表。**
        detail: String,
    },
}

/// 把外来文本消毒：控制字符压成空格、连续空白收成一个、首尾去空。
///
/// 外来文本**不当可信**：一台 server 可以往说明里塞换行、制表、回车来伪造版式，
/// 因此进主体的视野之前一律过这一步（机械处理，不做语义判断）。
pub(crate) fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for character in text.chars() {
        if character.is_control() || character.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(character);
    }
    out
}

/// 按**字符边界**截断到给定字节数；返回 `(留下的, 有没有截断)`。
///
/// 截断是**显式**的：调用方要在留下的文本后面补标记（静默截断违禁）。
pub(crate) fn clip(text: &str, limit: usize) -> (String, bool) {
    if text.len() <= limit {
        return (text.to_string(), false);
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// 消毒 + 截断 + 补标记：外来的一句话进视野之前的**唯一**路径。
///
/// ⚠ **它只治一行标签，不治正文**（判据：这段话是**元数据**还是**内容**）：标签（对面给的说明、
/// 形状说明里的键名）是**元数据**——对面塞一个换行就能伪造版式 ⇒ **必须**压成一行；而**正文是
/// 内容**，版式就是它的意思 ⇒ **不消毒**（`web_app` 搬回来的网页正文照旧原样），靠适配层的
/// **围栏** `<<<DATA` / `>>>` 兜（模型适配层那一侧）。**看起来像"同一件事两种写法"，
/// 其实不是**：这两处治的是两件不同的事。
pub(crate) fn foreign_line(text: &str, limit: usize) -> String {
    let clean = sanitize(text);
    let (clipped, truncated) = clip(&clean, limit);
    if truncated {
        format!("{clipped}…（截断）")
    } else {
        clipped
    }
}

/// 失败分类 → 我们的结果形状。
///
/// **写档**在"效果不明"那一格落进 [`ActionOutcome::unknown`]（第三态：它原来是**未决效果门**要接
/// 的形状，门已退役——门是指令而世界只该给信息；这一态照旧如实报，要不要去看一眼由主体自己决定）；
/// **只读档**没有"可能已经发生"这一格，因此它一律如实落进明确失败（读不改变世界，也就没有可重复的
/// 效果要担心）。
pub(crate) fn fault_outcome(fault: &CallFault, effect: McpEffect) -> ActionOutcome {
    match (fault, effect) {
        (CallFault::Refused(class), _) => ActionOutcome::failure(class.as_str()),
        (CallFault::Indeterminate(_), McpEffect::UnverifiableWrite) => ActionOutcome::unknown(),
        (CallFault::Indeterminate(class), McpEffect::ReadOnly) => {
            ActionOutcome::failure(class.as_str())
        }
    }
}
