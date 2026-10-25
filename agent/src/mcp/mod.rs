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
//! 通道（transport）—— 三个协议动作：connect / list_tools / call_tool；
//!                     另有**可选**的资源钩子（list_resources / read_resource），默认关闭
//! ```
//!
//! 通道被抽成一个**窄 trait**（[`Transport`]）：v1 只有 stdio 一个实现（[`stdio::StdioTransport`]），
//! 但"换一条通道"从此只是换一个实现——**不为想象中的远程 server 预建实现**（走网络那种通道
//! 不在本刀，真出现那天再说）。
//!
//! ## 2026-10-25：生产通道换成 `rmcp` SDK
//!
//! 上面的三件事（connect / list_tools / call_tool）**没有变**；变的只是通道的实现。
//! 生产路径现在由 [`rmcp_host::RmcpTransport`] 承担：MCP 的版本协商、现代/legacy 生命周期、
//! stdio 子进程、Streamable 网络、订阅通知全部交给 `rmcp` 3.7.0；异步 I/O 被关在一条独立的
//! tokio 宿主线程里，Core / Run / Turn / Trace / Ledger 仍然是同步、确定性状态机。
//!
//! [`stdio::StdioTransport`] 与 [`remote::RemoteTransport`] 保留为**失败诊断与既有用例的基线**：
//! 生产展开只在 rmcp 握手失败时用旧 stdio 读一次"对面到底说了什么"；成功路径不走手写协议栈。
//! 通知仍然只是**变更信号**——正文由 Runtime 重新 `tools/list` 拉取，本层不把它写进模型上下文。
//!
//! ## 资源（resources）：两个通用只读动作，不是每资源一个动词（2026-10-25）
//!
//! 一台 server 若在握手时声明 `resources`，展开之后它的 App 里**多出两个**动作：
//!
//! ```text
//! <名>_resources_list   [cursor?]   一次一页；回执只有 name/uri/mime/size 与下一页游标
//! <名>_resources_read   uri         按需读一块正文（文本有界，blob 只报大小）
//! ```
//!
//! 三条判据，缺一条就会长成别的东西：
//!
//! - **资源不是动作**：一条资源一个动词，会让每轮摆进主体视野的动词面随对面的**数据**长大
//!   （而动词面是能力面）。资源清单因此只进**回执**，动词面固定只有这两条。
//! - **正文按需拉**：`resources/list` **不带正文**；正文只在 `resources_read` 那一次调用里读，
//!   读完也只是那一次的回执——**不自动进模型上下文**（与 tools 同一条纪律）。
//! - **prompts 不在这里**：它不是动作面的一部分，采样 / elicitation 也不做。
//!
//! 两条都是 [`McpEffect::ReadOnly`]——**与宿主给这台 server 的效果档无关**：`read-only` 那句
//! 背书说的是工具，而"读资源"本来就改变不了外面的世界。后果是它们落进 [`fault_outcome`] 的
//! 只读那一格：通道断在中间也只是明确失败，**不是** [`ActionOutcome::unknown`]。
//! 失败用既有词表（`timeout` / `transport` / `server-error` / `tools-failed`），不新造对外词。
//!
//! 只有**对面真的声明了** `resources` 才挂这两个动作；旧 fixture 与没有资源面的通道
//! （[`Transport::supports_resources`] 默认 `false`）因此逐字节保持 tools-only 的旧行为。
//! 能力是**握手那一下的读数**：对面后来才长出 resources（没有重新握手）不会热加动作，
//! `notifications/resources/list_changed` 今天也不订阅——重新展开才会重新读。
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
//! 子进程是**宿主级租约**的：首次显式展开才起；空闲 [`DEFAULT_CHANNEL_IDLE_SECONDS`] 秒
//! （`,idle=` 可调，`,keep-alive` 退出）或宿主退出时收；同时最多 [`MAX_OPEN_CHANNELS`] 条，
//! 超了按最久没用的先走。展开状态本身仍然归**这一次活动**（OS 的 `RunExpansion`）——所以下一次
//! Run 主体要**再说一次**"我要用它"，而通道（进程）通常不必重建。
//!
//! ## 通道租约：寿命归宿主，不归对象图（2026-10-25）
//!
//! 从前通道住在 `McpAccess` 的一个 `Arc<Mutex<Option<Box<dyn Transport>>>>` 里：谁克隆到它、
//! 最后一个 `Arc` 什么时候掉，子进程就活到什么时候。于是同一条声明因为**构造位置**不同长出两套
//! 寿命（`--mcp` 是宿主级、`mcp.conf` 是每段 Run 重建）。现在寿命由 [`ChannelPool`] 统一发放：
//!
//! - **一张池，按 server 名索引**：`--mcp` 与 `mcp.conf` 共用同一张（[`McpAccess::with_pool`]）；
//!   `McpAccess::new` 只留给"这个对象就是全部世界"的测试与自包含对象。
//! - **只在两段 Run 之间回收**：宿主在 [`crate::host_loop::Host`] 开段之前 `sweep`（空闲到期 ∪
//!   超上限 LRU；一段 Run 里一次展开超过上限会暂时超出，下一次开段前收束），并按本段声明
//!   `retain`（没再声明的 server 当场收）。一段 Run 进行中绝不回收：
//!   OS 说"这个 App 在场"，它的动作就必须可用。
//! - **故障仍当场收、不自动重连**：调用路径上的任何故障把这一格的租约置空；下一次**显式展开**才重建。
//! - **回收留读数**：每次收掉都打一行（哪台、空闲多久/为何被挤掉），`reclaimed_count` 给测试用。
//!
//! 代价与边界：**没有后台定时器**——宿主长时间不醒时，已空闲的通道会占着内存到下一次开段；
//! 这是刻意的（不为回收单独造一个唤醒源）。
//!
//! ⇒ **"临时接入"不是含糊状态，是三个精确的落点**：身份（装配期）／在场（那一次活动）／通道
//! （宿主级）。因此它**不构成"它不该是 App"的理由**——App 契约本来就有"在场"那一格
//! （展开／`RunScope::is_open`）。
//!
//! ## 效果档（宿主显式背书，只有两档）
//!
//! ```text
//! --mcp <名>=stdio:<命令>                        ⇒ unverifiable-write（最严：默认档）
//! --mcp <名>=stdio:<命令>,read-only              ⇒ read-only（宿主显式背书这台只读）
//! --mcp <名>=stdio:<命令>,timeout=<秒>           ⇒ 转发窗口（1–600，缺省 120）
//! --mcp <名>=stdio:<命令>,idle=<秒>              ⇒ 通道空闲租期（0–86400，缺省 1800）
//! --mcp <名>=stdio:<命令>,keep-alive             ⇒ 不按时间收（等价 idle=0）
//! ```
//!
//! ⚠ 后缀是**从末尾逐条剥**的，几个开关**顺序不限**（每剥掉一个就回到循环开头）；唯一要避的是
//! 命令/地址本身末尾恰好长得像某个开关。`read-only` 与 `timeout` 写反**不会**静默降档——这一点
//! 现在有用例钉着（`suffix_options_are_peeled_in_any_order`）。
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
//! 拿那三问量这一格：第 ③ 问（**别人的话标明了吗**）在展开这条路径上从"否"变成"是"——
//! 判据一个字没改，改的是这一格的答。
//!
//! ⚠ **如实登记的覆盖边界**：这一格**没有真机证据**——要一台"只会说新版、又在 `error` 里带一句
//! 人话"的真 server 才有真机读数，今天没有这样的夹具；`tests/mcp_app.rs` 那两条是**真进程**
//! （夹具 server 真被起起来、真走 stdio 握手），但那个进程是**测试件**，不是市面上的 server。
//! ⇒ 它们证明的是"我们这一侧不丢那句话"，不是"市面上的 server 会这么说"。
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
use std::path::Path;
use std::time::{Duration, Instant};

pub mod app;
pub mod remote;
pub mod rmcp_host;
pub mod scheme;
pub mod stdio;
pub mod wire;
pub mod wire_tokens;

pub use app::McpApp;
pub use remote::RemoteTransport;
pub use rmcp_host::RmcpTransport;
pub use stdio::StdioTransport;

/// 转发窗口的缺省值（秒）。与 terminal 同一量级：这是"慢工具的世界"。
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 120;

/// 转发窗口的硬上限（秒）：声明可以调，但**调不过它**。
pub const MAX_TIMEOUT_SECONDS: u64 = 600;

/// 通道**空闲多久就收**的缺省值（秒）：30 分钟。
///
/// 判据：一次对话的停顿通常以秒/分钟计，半小时足够覆盖；而"用了一次的 Playwright 挂一整天"
/// 是真实成本。有状态的 server（浏览器会话、数据库连接）用 `,keep-alive` 显式退出。
pub const DEFAULT_CHANNEL_IDLE_SECONDS: u64 = 30 * 60;

/// 声明里 `,idle=` 的上限（秒）：一天。更长的时间用 `,keep-alive` 表达"不要按时间收"。
pub const MAX_CHANNEL_IDLE_SECONDS: u64 = 24 * 60 * 60;

/// 宿主同时最多开着几条 MCP 通道：超过就按"最久没用的先走"收。
///
/// 它不是 MCP 协议的上限，是**这台宿主愿意替外部进程承担的进程数**。真实用法是手边几台；
/// 8 条足够覆盖，且给重进程（几百 MB 一台）留了边界。
pub const MAX_OPEN_CHANNELS: usize = 8;

/// 单次调用搬回来的正文上限（字节）：超限**截断并带标记**。
///
/// 它是"读的时候就有的界"：不是读回来再砍（那要求先把无界正文读进内存），而是读到这个数就不再
/// 收——**显式截断合法，静默截断违禁**，所以回执里照样给出原文总字节数与截断标记。
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

/// 一台 server 最多能上多少条动词：超限**整台不上场**（如实报分类）。
///
/// 它是"每轮都要摆进主体的视野的动词面"的机械上限，不是一个调优参数。真实 server 远低于它。
pub const MAX_TOOLS: usize = 128;

/// 一页 `resources/list` 最多搬多少条：超限**这一页不搬**（如实报分类）。
///
/// 与 [`MAX_TOOLS`] 同一条理由的另一面：资源不进动词面，但一页回执也要有界——
/// 不能因为对面说"我有一百万条资源"就把一百万条**搬进回执**。分页游标原样透传，
/// 因此这个上限**不会**让我们漏掉后面的页，只让这一页如实失败。
///
/// 取 32（比 [`MAX_TOOLS`] 小得多）：每条资源的回执带 uri＋四种标签，32 条 × 各自的
/// 字节上限仍在一条可读回执的量级里；资源是**数据**，没人需要在一页里拿到 128 条。
/// ⚠ 界画在**本层搬运的那一面**：SDK 已经把那页解析成结构（与 `list_all_tools` 同一个
/// 既有性质），本层不再把超限的页继续交给 App。
pub const MAX_RESOURCES_PER_PAGE: usize = 32;

/// 资源 URI 的字节上限：超限**截断并显式标出**（那个 URI 原样读不回来）。
pub const MAX_RESOURCE_URI_BYTES: usize = 512;

/// 资源**标签**（名字 / 标题 / 说明 / 媒体类型）的字节上限：它们进的是回执，是元数据。
pub const MAX_RESOURCE_LABEL_BYTES: usize = 200;

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
    /// 走哪条通道（本机子进程 / 网络端点）。
    endpoint: McpEndpoint,
    effect: McpEffect,
    timeout_seconds: u64,
    /// 空闲多久收（`None` = 用池的缺省值；`Some(0)` = 不按时间收，等价 `keep-alive`）。
    idle_seconds: Option<u64>,
}

/// 一条声明走的**通道**。
///
/// 两种的**信任边界不同**，因此不是一个字段的两种写法：
/// - [`McpEndpoint::Stdio`]：同一台机器上的子进程——能洗环境、能收进程树；
/// - [`McpEndpoint::Remote`]：网络上一个第三方端点——数据出门、只能靠加密地址与地址墙。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpEndpoint {
    /// 本机子进程：命令交给平台默认 shell。
    Stdio { command: String },
    /// 网络端点：`credential_env` 是**环境变量的名字**——凭据本身不落声明、不进日志、
    /// 不进主体的视野。地址的 scheme 与墙见 [`parse_declaration`]。
    Remote {
        address: String,
        credential_env: Option<String>,
    },
}

impl McpDeclaration {
    /// server 名（动词前缀，也是 App 标识）。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 这条声明走哪条通道。
    pub fn endpoint(&self) -> &McpEndpoint {
        &self.endpoint
    }

    /// 是不是网络端点（远程那一档）。
    pub fn is_remote(&self) -> bool {
        matches!(self.endpoint, McpEndpoint::Remote { .. })
    }

    /// 要跑的命令行（只有 stdio 有；远程端点没有子进程）。
    pub fn stdio_command(&self) -> Option<&str> {
        match &self.endpoint {
            McpEndpoint::Stdio { command } => Some(command),
            McpEndpoint::Remote { .. } => None,
        }
    }

    /// 远程端点的地址（只有远程有）。
    pub fn remote_address(&self) -> Option<&str> {
        match &self.endpoint {
            McpEndpoint::Remote { address, .. } => Some(address),
            McpEndpoint::Stdio { .. } => None,
        }
    }

    /// 远程端点要带的凭据：**环境变量的名字**，不是取值。
    pub fn remote_credential_env(&self) -> Option<&str> {
        match &self.endpoint {
            McpEndpoint::Remote { credential_env, .. } => credential_env.as_deref(),
            McpEndpoint::Stdio { .. } => None,
        }
    }

    /// 宿主给的效果档。
    pub fn effect(&self) -> McpEffect {
        self.effect
    }

    /// 转发窗口（秒）。
    pub fn timeout_seconds(&self) -> u64 {
        self.timeout_seconds
    }

    /// 这台 server 的通道空闲多久收（`None` = 用池的缺省值；`Some(0)` = 不按时间收）。
    pub fn idle_seconds(&self) -> Option<u64> {
        self.idle_seconds
    }
}

/// 解析 `--mcp` 的一条声明；失败给**一个不带取值的稳定短语**（入口直接打出来）。
///
/// 两种形状：
/// - `<名>=stdio:<命令>[,read-only][,timeout=<秒>][,keep-alive|,idle=<秒>]`
/// - `<名>=<网络地址>[,credential-env=<环境变量名>][,read-only][,timeout=<秒>][,keep-alive|,idle=<秒>]`
///   （本机回环允许明文地址，只供本地测试）
///
/// 可选项**从末尾逐条剥**（不按逗号切，那样会把命令或地址里的逗号也切掉），顺序不限。
///
/// ⚠ **远程端点在解析期就过地址墙**：非加密地址（回环除外）、私网/回环/链路本地地址、地址里带
/// 凭据 —— 一律拒绝启动。那是"起任何东西之前"能拦下的那一半；运行期还有一条：**不跟随重定向**。
pub fn parse_declaration(text: &str) -> Result<McpDeclaration, String> {
    let Some((name, rest)) = text.split_once('=') else {
        return Err("声明缺少 `=`：形状是 `<名>=stdio:<命令>`，或 `<名>=<网络地址>`".to_string());
    };

    let mut body = rest.trim().to_string();
    let mut effect = McpEffect::UnverifiableWrite;
    let mut timeout_seconds = DEFAULT_TIMEOUT_SECONDS;
    let mut idle_seconds: Option<u64> = None;
    let mut credential_env: Option<String> = None;
    loop {
        if let Some(head) = body.strip_suffix(",read-only") {
            effect = McpEffect::ReadOnly;
            body = head.to_string();
            continue;
        }
        if let Some(head) = body.strip_suffix(",keep-alive") {
            if idle_seconds.is_some() {
                return Err("`keep-alive` 与 `idle=` 只能写一个".to_string());
            }
            idle_seconds = Some(0);
            body = head.to_string();
            continue;
        }
        if let Some((head, raw)) = body.rsplit_once(",idle=") {
            if idle_seconds.is_some() {
                return Err("`keep-alive` 与 `idle=` 只能写一个".to_string());
            }
            let seconds: u64 = raw
                .parse()
                .map_err(|_| "`idle=` 的值不是整数".to_string())?;
            if seconds > MAX_CHANNEL_IDLE_SECONDS {
                return Err(format!(
                    "`idle=` 必须在 0–{MAX_CHANNEL_IDLE_SECONDS} 秒之间（0 = 不按时间收）"
                ));
            }
            idle_seconds = Some(seconds);
            body = head.to_string();
            continue;
        }
        // ⚠ 顺序：**先把 credential-env 从尾巴上剥掉**，再找 `,timeout=`——否则
        // `…,timeout=30,credential-env=X` 会把 `30,credential-env=X` 当成窗口值。
        if let Some((head, raw)) = body.rsplit_once(",credential-env=") {
            if credential_env.is_some() {
                return Err("`credential-env=` 只能写一次".to_string());
            }
            let env_name = raw.trim();
            let mut chars = env_name.chars();
            let first = chars.next().unwrap_or('0');
            if !(first.is_ascii_alphabetic() || first == '_')
                || !env_name
                    .chars()
                    .all(|each| each.is_ascii_alphanumeric() || each == '_')
            {
                return Err(
                    "`credential-env=` 的值必须是环境变量的名字（ASCII 字母数字下划线）"
                        .to_string(),
                );
            }
            credential_env = Some(env_name.to_string());
            body = head.to_string();
            continue;
        }
        if let Some((head, raw)) = body.rsplit_once(",timeout=") {
            let seconds: u64 = raw
                .parse()
                .map_err(|_| "`timeout=` 的值不是整数".to_string())?;
            if !(1..=MAX_TIMEOUT_SECONDS).contains(&seconds) {
                return Err(format!("`timeout=` 必须在 1–{MAX_TIMEOUT_SECONDS} 之间"));
            }
            timeout_seconds = seconds;
            body = head.to_string();
            continue;
        }
        break;
    }

    let endpoint = if let Some(command) = body.strip_prefix("stdio:") {
        if credential_env.is_some() {
            return Err(
                "`credential-env=` 只用于网络端点：stdio 的 server 由我们起，凭据从它的环境里洗掉"
                    .to_string(),
            );
        }
        if command.trim().is_empty() {
            return Err("命令不能为空".to_string());
        }
        McpEndpoint::Stdio {
            command: command.to_string(),
        }
    } else if body.starts_with(scheme::SECURE) || body.starts_with(scheme::PLAIN) {
        check_remote_address(&body)?;
        McpEndpoint::Remote {
            address: body.clone(),
            credential_env,
        }
    } else {
        return Err(
            "只认得两种通道：`stdio:<命令>`，或一个网络地址（本机回环或加密地址）".to_string(),
        );
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

    Ok(McpDeclaration {
        name: name.to_string(),
        endpoint,
        effect,
        timeout_seconds,
        idle_seconds,
    })
}

/// 远程端点的**地址墙**（解析期，起任何东西之前）。
///
/// 它挡的是 SSRF 的第一步：**地址**。运行期还剩三步——不跟随重定向（一次重定向就是一个新的
/// 目的地）、响应有界、每次请求都有窗口。
fn check_remote_address(address: &str) -> Result<(), String> {
    let secure = address.starts_with(scheme::SECURE);
    let plain = address.starts_with(scheme::PLAIN);
    if !secure && !plain {
        return Err("远程端点缺少 scheme：要以加密地址开头（本机回环也可用明文）".to_string());
    }
    let rest = address
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or("");
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err("远程端点缺少主机".to_string());
    }
    if authority.contains('@') {
        return Err("端点里不许带凭据：凭据走 `,credential-env=<环境变量名>`".to_string());
    }
    let host = authority
        .rsplit_once(':')
        .filter(|(_, port)| port.chars().all(|each| each.is_ascii_digit()))
        .map(|(host, _)| host)
        .unwrap_or(authority)
        .trim_start_matches('[')
        .trim_end_matches(']');
    if host.is_empty() {
        return Err("远程端点缺少主机".to_string());
    }
    let loopback = host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1";
    if plain && !loopback {
        return Err("远程端点必须是加密地址（只有本机回环允许明文，供本地测试）".to_string());
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>()
        && !loopback
        && !is_public_address(ip)
    {
        return Err("拒绝私网/回环/链路本地地址：远程端点必须是公网地址".to_string());
    }
    Ok(())
}

/// 公网地址吗（私网、回环、链路本地、未指定、组播都不算）。
fn is_public_address(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(v4) => {
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast())
        }
        std::net::IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            let unique_local = (first & 0xfe00) == 0xfc00;
            let link_local = (first & 0xffc0) == 0xfe80;
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || unique_local
                || link_local)
        }
    }
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

/// 宿主给一台 server 的**访问声明**：解析出来的那几样 + 要洗的凭据键名 + **它挂在哪张宿主级通道池上**。
///
/// 通道**不归这个对象**：它归宿主级的 [`ChannelPool`]（按 server 名持租约）。本类型只是把
/// "这台 server 叫什么、怎么洗凭据、租期多长" 带到每一次装配里——因此它可以被每一段 Run
/// 各克隆一份，而子进程不会跟着装配复制或提前消失。
///
/// 本类型**不派生 `Debug`**，也**永远不携带任何取值**（洗凭据的读数只报键名、连命令原文都不
/// 打印在库里）。
#[derive(Clone)]
pub struct McpAccess {
    declaration: McpDeclaration,
    scrub: Vec<String>,
    channels: std::sync::Arc<ChannelPool>,
}

/// **宿主级的通道租约池**：一个 server 名一条租约（子进程或远程会话）。
///
/// 它回答的是"这台 server 的端点还活着吗、活了多久"，**不回答**"这一段 Run 有没有展开过"——
/// 后者仍然是 OS 的 `RunExpansion`，每段 Run 各自从空开始。
///
/// ## 三条纪律
///
/// 1. **只在 Run 之间回收**：`sweep()` 由宿主在开一段之前调用。一段 Run 进行中，OS 说"这个
///    App 在场"，它的动作就必须可用——中途把通道抽掉会让目录里明明在的动作一调就 `transport`。
///    因此本池**不挂后台定时器**，也不在取通道时驱逐。
/// 2. **失败即弃、不自动重连**：调用路径上的任何故障由调用方当场清空这一格；下一次
///    **显式展开**才重建。自动复活会把一次失败变成看不见的重试。
/// 3. **回收留读数**：空闲回收与上限驱逐都打一行（哪台、空闲多久/为什么），否则"这次为什么
///    慢了"永远无法解释。
pub struct ChannelPool {
    inner: std::sync::Mutex<PoolInner>,
    max: usize,
    idle: Duration,
}

struct PoolInner {
    /// 按 server 名（不是动词前缀）索引：与 `--mcp` / `mcp.conf` 的声明同一个键。
    slots: std::collections::BTreeMap<String, std::sync::Arc<ChannelSlot>>,
    /// 到今天为止收掉过几条通道（空闲 + 驱逐 + 取消声明）：回收读数的计数。
    reclaimed: usize,
}

/// 一个 server 名下的那一格：租约 ＋ 它的空闲期限。
pub(crate) struct ChannelSlot {
    pub(crate) state: std::sync::Mutex<SlotState>,
}

pub(crate) struct SlotState {
    /// `None` = 还没连（首次展开才连），或者**上一次它坏了**（我们把它收了，不自动重连）。
    pub(crate) lease: Option<Lease>,
    /// 空闲多久收（`Duration::ZERO` = 不按时间收，等价 `keep-alive`）。
    pub(crate) idle: Duration,
}

/// 一条活着的通道：传输本体 ＋ 最后一次被用来做事的时刻。
pub(crate) struct Lease {
    pub(crate) transport: Box<dyn Transport>,
    pub(crate) last_used: Instant,
}

impl ChannelPool {
    /// 造一张池（测试与宿主各取所需；生产用 [`ChannelPool::host_default`]）。
    pub fn new(max: usize, idle: Duration) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            inner: std::sync::Mutex::new(PoolInner {
                slots: std::collections::BTreeMap::new(),
                reclaimed: 0,
            }),
            max: max.max(1),
            idle,
        })
    }

    /// 生产宿主的缺省租约：最多 [`MAX_OPEN_CHANNELS`] 条，空闲 [`DEFAULT_CHANNEL_IDLE_SECONDS`]
    /// 秒后收。
    pub fn host_default() -> std::sync::Arc<Self> {
        Self::new(
            MAX_OPEN_CHANNELS,
            Duration::from_secs(DEFAULT_CHANNEL_IDLE_SECONDS),
        )
    }

    /// 一条不与人共享的池（`McpAccess::new` 的旧语义：没有上限、不按时间收）。
    ///
    /// 它只给"这个对象自己就是全部世界"的场合用（测试、自包含对象）；生产宿主必须用
    /// [`ChannelPool::host_default`] 并把同一份交给每一台 access，否则上限与租期都不成立。
    fn private() -> std::sync::Arc<Self> {
        Self::new(1, Duration::ZERO)
    }

    /// 池的缺省空闲期限（声明没写 `,idle=` / `,keep-alive` 时用它）。
    pub fn default_idle(&self) -> Duration {
        self.idle
    }

    /// 取（或建立）一个 server 名下的那一格；同时把这一份声明里的空闲期限落进去。
    ///
    /// 同一个名字在下一段 Run 的 `mcp.conf` 里再次被声明时，期限以**最新这份声明**为准；
    /// 宿主声明与主体声明同名的那一路会在合并时被丢掉，并由 `reset_idle` 把宿主的期限写回。
    pub(crate) fn slot(&self, name: &str, idle: Duration) -> std::sync::Arc<ChannelSlot> {
        let mut inner = self.inner.lock().expect("通道池只被短临界区持有");
        if let Some(slot) = inner.slots.get(name) {
            slot.state.lock().expect("通道那一格只被短临界区持有").idle = idle;
            return std::sync::Arc::clone(slot);
        }
        let slot = std::sync::Arc::new(ChannelSlot {
            state: std::sync::Mutex::new(SlotState { lease: None, idle }),
        });
        inner
            .slots
            .insert(name.to_string(), std::sync::Arc::clone(&slot));
        slot
    }

    /// 收掉**当前没有空闲期限**（`Duration::ZERO`）以外、已经闲够了的通道。
    ///
    /// 调用点只有一个：宿主**开一段之前**（两次 Run 之间）。回收动作本身不产生模型内容，
    /// 只留一行读数。
    pub fn sweep(&self) {
        let slots: Vec<(String, std::sync::Arc<ChannelSlot>)> = {
            let inner = self.inner.lock().expect("通道池只被短临界区持有");
            inner
                .slots
                .iter()
                .map(|(name, slot)| (name.clone(), std::sync::Arc::clone(slot)))
                .collect()
        };
        let now = Instant::now();
        let mut open: Vec<(String, std::sync::Arc<ChannelSlot>, Instant)> = Vec::new();
        for (name, slot) in slots {
            let (idle, last_used) = {
                let state = slot.state.lock().expect("通道那一格只被短临界区持有");
                let Some(lease) = state.lease.as_ref() else {
                    continue;
                };
                (state.idle, lease.last_used)
            };
            if !idle.is_zero() && now.saturating_duration_since(last_used) >= idle {
                if let Some(lease) = slot
                    .state
                    .lock()
                    .expect("通道那一格只被短临界区持有")
                    .lease
                    .take()
                {
                    let idle_seconds = now.saturating_duration_since(last_used).as_secs();
                    self.reclaimed();
                    drop(lease);
                    eprintln!(
                        "[mcp] 收掉空闲通道 `{name}`（空闲 {idle_seconds} 秒 ≥ {} 秒）",
                        idle.as_secs()
                    );
                }
            } else {
                open.push((name, slot, last_used));
            }
        }

        // 上限：**最久没用的先走**。走到这里时上一段 Run 已经收场，所以租约被收不会让任何
        // "在场"的动作变成假的——下一段 Run 本来就从未展开开始。
        let excess = open.len().saturating_sub(self.max);
        if excess > 0 {
            open.sort_by_key(|(_, _, last_used)| *last_used);
            for (name, slot, _) in open.into_iter().take(excess) {
                if let Some(lease) = slot
                    .state
                    .lock()
                    .expect("通道那一格只被短临界区持有")
                    .lease
                    .take()
                {
                    self.reclaimed();
                    drop(lease);
                    eprintln!(
                        "[mcp] 收掉通道 `{name}`（同时打开的通道超过上限 {}）",
                        self.max
                    );
                }
            }
        }
    }

    /// 只保留**这一段 Run 仍然声明着**的 server；其余的通道收掉。
    ///
    /// 它兑现的是"主体改 `mcp.conf`，下一段就少一台"：删掉那一行之后，那一台不该继续
    /// 在宿主里占着一个进程。
    pub fn retain(&self, active: &[String]) {
        let removed: Vec<(String, std::sync::Arc<ChannelSlot>)> = {
            let mut inner = self.inner.lock().expect("通道池只被短临界区持有");
            let names: Vec<String> = inner.slots.keys().cloned().collect();
            names
                .into_iter()
                .filter(|name| !active.iter().any(|declared| declared == name))
                .filter_map(|name| inner.slots.remove(&name).map(|slot| (name, slot)))
                .collect()
        };
        for (name, slot) in removed {
            if let Some(lease) = slot
                .state
                .lock()
                .expect("通道那一格只被短临界区持有")
                .lease
                .take()
            {
                self.reclaimed();
                drop(lease);
                eprintln!("[mcp] 收掉 `{name}` 的通道（这一段没有再声明它）");
            }
        }
    }

    /// 现在开着几条通道。
    pub fn open_count(&self) -> usize {
        let slots: Vec<std::sync::Arc<ChannelSlot>> = {
            let inner = self.inner.lock().expect("通道池只被短临界区持有");
            inner.slots.values().map(std::sync::Arc::clone).collect()
        };
        slots
            .iter()
            .filter(|slot| {
                slot.state
                    .lock()
                    .expect("通道那一格只被短临界区持有")
                    .lease
                    .is_some()
            })
            .count()
    }

    /// 到今天为止收掉过几条（空闲 / 驱逐 / 取消声明）。
    pub fn reclaimed_count(&self) -> usize {
        self.inner.lock().expect("通道池只被短临界区持有").reclaimed
    }

    fn reclaimed(&self) {
        self.inner.lock().expect("通道池只被短临界区持有").reclaimed += 1;
    }
}

impl McpAccess {
    /// 用一份解析好的声明与要洗的键名建立访问声明（通道还没建；租约归一张私有池）。
    pub fn new(declaration: McpDeclaration, scrub: Vec<String>) -> Self {
        Self::with_pool(declaration, scrub, ChannelPool::private())
    }

    /// 把这份声明挂到**宿主共用的那张池**上（生产路径）。
    ///
    /// `--mcp` 与 `mcp.conf` 必须走同一个池：寿命规则只有一条，不因为谁声明的而分岔。
    pub fn with_pool(
        declaration: McpDeclaration,
        scrub: Vec<String>,
        channels: std::sync::Arc<ChannelPool>,
    ) -> Self {
        let idle = declaration
            .idle_seconds()
            .map(Duration::from_secs)
            .unwrap_or_else(|| channels.default_idle());
        // 先把这一份声明登记的期限落进池里（即使这一台这一段没被展开，`retain` 也知道它属于
        // 当前声明集）。真正取用那一格时仍按名字现取，见 [`McpAccess::slot`]。
        let _ = channels.slot(declaration.name(), idle);
        Self {
            declaration,
            scrub,
            channels,
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
        let access = Self::new(declaration, scrub);
        let slot = access.slot();
        slot.state.lock().expect("通道那一格只被短临界区持有").lease = Some(Lease {
            transport,
            last_used: Instant::now(),
        });
        access
    }

    /// 把这条声明的空闲期限重新落进池里。
    ///
    /// 用在**同名冲突**那一处：主体清单里的同名行先被构造成 access（这一步会写池里的期限），
    /// 随后按"宿主为准"被丢掉——若不把宿主的期限写回去，一行同名文本就能偷偷改掉宿主那台的租期。
    pub(crate) fn reset_idle(&self) {
        let idle = self
            .declaration
            .idle_seconds()
            .map(Duration::from_secs)
            .unwrap_or_else(|| self.channels.default_idle());
        self.channels.slot(self.declaration.name(), idle);
    }

    /// 宿主面的声明。
    pub fn declaration(&self) -> &McpDeclaration {
        &self.declaration
    }

    /// 宿主共用的那张池（宿主在开段之前用它 sweep / retain）。
    pub fn channels(&self) -> std::sync::Arc<ChannelPool> {
        std::sync::Arc::clone(&self.channels)
    }

    /// 这个 server 名下的那一格（本模块内使用）。
    ///
    /// **每次按名字现取**、不把 `Arc` 存在 access 里：`retain` 可以把一格从池里摘掉，下一次
    /// 声明/展开会重新登记；否则一个仍被旧对象握着的格子会绕过上限与租期继续活着。
    pub(crate) fn slot(&self) -> std::sync::Arc<ChannelSlot> {
        let idle = self
            .declaration
            .idle_seconds()
            .map(Duration::from_secs)
            .unwrap_or_else(|| self.channels.default_idle());
        self.channels.slot(self.declaration.name(), idle)
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

/// 通道送上来的**变更通知**（当前只接一条）。
///
/// 它不是“对面推了一段上下文”，而是“对面说某类事实变了”；真正的内容仍由客户端重新拉取。
/// 这条区分是 MCP 最新协议 `2026-07-28` 的 `subscriptions/listen` 形状：通知只报变更类型，
/// 清单/资源正文走普通请求。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpNotification {
    /// `notifications/tools/list_changed`：工具清单变了，客户端应重新 `tools/list`。
    ToolsListChanged,
}

/// 通道的**窄接口**：三个协议动作 ＋ 两个可选的通知钩子。
///
/// 它只说"怎么跟对面说话"，不知道任何 App 的事；[`McpApp`] 只说"暴露哪些动词、墙在哪里"，
/// 不知道对面是 stdio 还是别的。通知钩子有默认实现：不支持通知的通道（远程一问一答、
/// 测试替身）不需要为了编译而写空壳。
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

    /// 这条通道对面声明了 `resources` 吗（**能力事实，不是我们的开关**）。
    ///
    /// 默认 `false`：旧基线通道（`stdio` / `remote`）与测试替身没有资源面，
    /// 展开时因此**一个资源动作都不多出**——既有 tools-only 的行为逐字节不变。
    fn supports_resources(&self) -> bool {
        false
    }

    /// 拉一页资源清单（游标原样透传）。
    ///
    /// 默认就是"这条通道没有资源面"：它报的是**清单拉不到**（[`McpFailure::ToolsFailed`]
    /// 是既有词表里那一格，不为资源另造一个对外词）。真正会被调到的实现只有
    /// [`rmcp_host::RmcpTransport`]——上层只在 [`Transport::supports_resources`] 为真时
    /// 才挂那两个动作，所以这条默认值**不该在正常路径上被走到**。
    fn list_resources(
        &mut self,
        _cursor: Option<String>,
        _timeout: std::time::Duration,
    ) -> Result<ResourcePage, CallFault> {
        Err(CallFault::Refused(McpFailure::ToolsFailed))
    }

    /// 读一条资源的正文（**有界**：文本截断带标记，blob 不搬原文）。
    ///
    /// 默认同 [`Transport::list_resources`]：没有资源面的通道如实拒绝。
    fn read_resource(
        &mut self,
        _uri: &str,
        _timeout: std::time::Duration,
    ) -> Result<Vec<ResourceContent>, CallFault> {
        Err(CallFault::Refused(McpFailure::ToolsFailed))
    }

    /// 订阅**工具清单变更**（`subscriptions/listen`）。
    ///
    /// 返回 `Ok(true)` = 订阅请求已经发出（服务端是否答应由后续通知/回执体现）；
    /// `Ok(false)` = 这条通道不支持通知（legacy 代、远程一问一答、测试替身）。
    /// **不把“不支持”当失败**：展开照旧成功，只是没有这条增量。
    fn subscribe_tools_changed(
        &mut self,
        _timeout: std::time::Duration,
    ) -> Result<bool, CallFault> {
        Ok(false)
    }

    /// 取走**已经到达**的通知（不阻塞）。没有就是空。
    ///
    /// 通道只存“变更类型”，不存任何正文；正文由调用方按类型重新拉取。
    fn poll_notifications(&mut self) -> Vec<McpNotification> {
        Vec::new()
    }

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

/// 对面清单里的一条资源（**不是动词**：资源是数据，只经两个通用只读动作取用）。
///
/// 所有文本都是**外来文本**：构造时消毒 + 截断（[`foreign_line`] / [`MAX_RESOURCE_LABEL_BYTES`]），
/// 因为回执是行式版式，对面塞一个换行就能伪造一行读数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawResource {
    /// 资源的 URI（读取时原样递回去；超长会截断并置 [`RawResource::uri_truncated`]）。
    pub uri: String,
    /// 对面的程序名。
    pub name: String,
    /// 对面的展示标题（可选）。
    pub title: Option<String>,
    /// 对面的一句话说明（可选）。
    pub description: Option<String>,
    /// 媒体类型（可选）。
    pub mime_type: Option<String>,
    /// 对面自报的原始字节数（可选；**不校验**，它是读数不是承诺）。
    pub size: Option<u64>,
    /// URI 超长被截断了吗。
    ///
    /// 截断的 URI **原样读不回来**，因此这一格必须显式出现在回执里——静默截断违禁。
    pub uri_truncated: bool,
}

/// 一页资源清单。
///
/// 游标是**对面的语义**：我们只透传，不解释、不编号、不自己造分页。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourcePage {
    /// 这一页的资源。
    pub resources: Vec<RawResource>,
    /// 下一页的游标（没有 = 这是最后一页）。
    pub next_cursor: Option<String>,
}

/// 一次 `resources/read` 搬回来的一块正文（**有界**：正文在通道里就截断）。
///
/// 一次答复最多搬**一块**：MCP 允许回多块内容，而一层只搬第一块，其余用
/// [`ResourceContent::omitted`] 如实报数（不静默丢）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceContent {
    /// 这一块自己带的 URI（对面会再报一次，可能与我们请求的不同）。
    pub uri: String,
    /// 媒体类型（可选）。
    pub mime_type: Option<String>,
    /// 正文——最多一块，且二进制不搬原文。
    pub body: ResourceBody,
    /// 同一次答复里我们**没搬**的后续内容块数（0 = 只有这一块）。
    pub omitted: usize,
}

/// 资源正文的两种形状（＋不认识的形状）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceBody {
    /// 文本正文（已在 [`MAX_TEXT_BYTES`] 处截断；`text_bytes` 是截断前的读数）。
    Text {
        /// 截断后的正文。
        text: String,
        /// 原文一共多少字节。
        text_bytes: usize,
        /// 有没有截断。
        truncated: bool,
    },
    /// 二进制正文：**不搬原文字节**，只报它有多大。
    Blob {
        /// 对面给的 base64 文本有多少字节（不是解码后的字节数）。
        encoded_bytes: usize,
    },
    /// 这一块既不是文本也不是 blob（SDK 的枚举留了扩展位）：如实说"搬不了"。
    Unsupported,
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

// ---------------------------------------------------------------------------
// 主体自己的那一份：`<环境根>/mcp.conf`（2026-10-20）
// ---------------------------------------------------------------------------

/// 宿主声明的**凭据洗名单**（这一版固定三条：模型两把、im 一把）。
///
/// ⚠ **它必须同时用在两条路上**：`--mcp` 装的那几台，与 Agent 自己写在 `mcp.conf` 里的那几台。
/// 如果后者拿到的子进程环境更干净，"装一台 server"就成了偷另一台被洗掉钥匙的路子——
/// 所以名单只有这一份（入口 `agent_host` 也从这里取）。
pub fn credential_names() -> Vec<String> {
    vec![
        "DEEPSEEK_API_KEY".to_string(),
        "TAVILY_API_KEY".to_string(),
        "CELIRA_IM_BACKEND_AUTH".to_string(),
    ]
}

/// 主体自己的外部工具清单（自持物，与 `notes` 同层、在环境根里）。
pub const SELF_DECLARATIONS_FILE: &str = "mcp.conf";

/// 读主体自己的清单：一行一条 `<名>=stdio:<命令>[,read-only][,timeout=<秒>][,keep-alive|,idle=<秒>]`；
/// 空行与 `#` 跳过。通道挂到宿主共用的那张池上（与 `--mcp` 同一套租期规则）。
///
/// 坏行**跳过并记一句话**，不让整段起不来——这是主体自己的文件，fail-soft 才合理
/// （与宿主 `--mcp` 的 fail-closed 是两种情形：那份是启动者写的，错了该拦）。
pub fn read_self_declarations(
    env_root: &Path,
    channels: &std::sync::Arc<ChannelPool>,
) -> (Vec<McpAccess>, Vec<String>) {
    let mut accesses: Vec<McpAccess> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    if !stdio_available() {
        notes.push("本平台没有 stdio 通道，mcp.conf 里的声明一台都不挂".to_string());
        return (accesses, notes);
    }
    let Ok(text) = std::fs::read_to_string(env_root.join(SELF_DECLARATIONS_FILE)) else {
        return (accesses, notes);
    };
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match parse_declaration(line) {
            Ok(declaration) => {
                // **远程端点只许人声明**：主体自己的清单只接受本机子进程。加一个网络端点
                // 就不是"多一只手"，而是"多一条把数据送出去的路"——那由启动者划，不由清单的写者划。
                if declaration.is_remote() {
                    notes.push(format!(
                        "{SELF_DECLARATIONS_FILE} 第 {} 行：远程端点只许人声明，这一行跳过",
                        index + 1
                    ));
                    continue;
                }
                if accesses
                    .iter()
                    .any(|each| each.declaration().name() == declaration.name())
                {
                    notes.push(format!(
                        "{SELF_DECLARATIONS_FILE} 第 {} 行：`{}` 已经有一台，这一行跳过",
                        index + 1,
                        declaration.name()
                    ));
                    continue;
                }
                accesses.push(McpAccess::with_pool(
                    declaration,
                    credential_names(),
                    std::sync::Arc::clone(channels),
                ));
            }
            Err(reason) => notes.push(format!(
                "{SELF_DECLARATIONS_FILE} 第 {} 行读不懂：{reason}",
                index + 1
            )),
        }
    }
    (accesses, notes)
}

/// 把主体自己的清单并进宿主的：**同名以宿主为准**（不能用同名换掉启动者给的那条命令）。
pub fn merge_own_declarations(
    host: &[McpAccess],
    own: Vec<McpAccess>,
) -> (Vec<McpAccess>, Vec<String>) {
    let mut merged: Vec<McpAccess> = host.to_vec();
    let mut notes = Vec::new();
    for access in own {
        let name = access.declaration().name().to_string();
        if merged
            .iter()
            .any(|each| each.declaration().name() == name.as_str())
        {
            // 宿主的命令与**租期**都归宿主：同名行不能借 `,keep-alive` 改掉它的寿命。
            if let Some(existing) = merged
                .iter()
                .find(|each| each.declaration().name() == name.as_str())
            {
                existing.reset_idle();
            }
            notes.push(format!(
                "`{name}` 与宿主声明同名，以宿主为准（不能用它替换那条命令）"
            ));
            continue;
        }
        merged.push(access);
    }
    (merged, notes)
}

/// 这一段实际生效的外部工具：宿主声明 ＋（**只有开了终端档时**才读）主体自己的清单。
///
/// 判据：自配 MCP 不新增权力——`terminal=full` 时主体本来就能跑任意命令；没开时它就是一条
/// 绕过终端档的后门（挂一台 `stdio:任意命令` 等于有手），所以那一档下 `mcp.conf` **不读**。
pub fn declarations_for_run(
    base: &[McpAccess],
    env_root: &Path,
    has_terminal: bool,
    channels: &std::sync::Arc<ChannelPool>,
) -> (Vec<McpAccess>, Vec<String>) {
    if !has_terminal {
        return (base.to_vec(), Vec::new());
    }
    let (own, notes) = read_self_declarations(env_root, channels);
    let (merged, conflicts) = merge_own_declarations(base, own);
    let mut all = notes;
    all.extend(conflicts);
    (merged, all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn env_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "celira-mcp-self-{}-{}-{}",
            std::process::id(),
            tag,
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_own_file_is_read_line_by_line_and_bad_lines_are_only_notes() {
        let root = env_root("read");
        std::fs::write(
            root.join(SELF_DECLARATIONS_FILE),
            "# 注释\n\nbrowser=stdio:npx playwright\nnot-a-declaration\n\
             db=stdio:node db.js,read-only,timeout=30\nbrowser=stdio:other\n",
        )
        .unwrap();
        let channels = ChannelPool::host_default();
        let (accesses, notes) = read_self_declarations(&root, &channels);
        let names: Vec<&str> = accesses
            .iter()
            .map(|each| each.declaration().name())
            .collect();
        assert_eq!(names, ["browser", "db"], "坏行与同名重复行只记账，不起作用");
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].contains("读不懂"), "{notes:?}");
        assert!(notes[1].contains("已经有一台"), "{notes:?}");
    }

    #[test]
    fn a_remote_endpoint_in_the_own_list_is_skipped() {
        let root = env_root("remote");
        std::fs::write(
            root.join(SELF_DECLARATIONS_FILE),
            format!(
                "cloud={}example.com/mcp\nlocal=stdio:node x.js\n",
                scheme::SECURE
            ),
        )
        .unwrap();
        let channels = ChannelPool::host_default();
        let (accesses, notes) = read_self_declarations(&root, &channels);
        let names: Vec<&str> = accesses
            .iter()
            .map(|each| each.declaration().name())
            .collect();
        assert_eq!(names, ["local"], "远程端点只许人声明：那一行不进清单");
        assert!(notes[0].contains("只许人声明"), "{notes:?}");
    }

    #[test]
    fn the_launchers_declaration_always_wins_on_the_same_name() {
        let host = parse_declaration("db=stdio:host-command").expect("宿主那份合法");
        let host = vec![McpAccess::new(host, credential_names())];
        let own = parse_declaration("db=stdio:own-command").expect("主体那台也合法");
        let (merged, notes) = merge_own_declarations(&host, vec![McpAccess::new(own, Vec::new())]);
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].declaration().stdio_command(),
            Some("host-command"),
            "同名以宿主为准——换不掉启动者给的那条命令"
        );
        assert!(notes[0].contains("以宿主为准"), "{notes:?}");
    }

    #[test]
    fn a_resident_without_the_hand_never_reads_the_own_file() {
        let root = env_root("gate");
        std::fs::write(root.join(SELF_DECLARATIONS_FILE), "x=stdio:whatever\n").unwrap();
        let host = vec![McpAccess::new(
            parse_declaration("host=stdio:cmd").expect("合法"),
            credential_names(),
        )];
        let channels = ChannelPool::host_default();
        let (mcp, notes) = declarations_for_run(&host, &root, false, &channels);
        assert_eq!(mcp.len(), 1, "没有终端档 ⇒ 只有宿主声明");
        assert_eq!(mcp[0].declaration().name(), "host");
        assert!(notes.is_empty(), "不读也不记");
    }

    // ---------------------------------------------------------------------
    // 通道租约（2026-10-25）：空闲期限、上限、取消声明
    // ---------------------------------------------------------------------

    /// 一条只会被池子搬来搬去的假通道：它唯一要证明的事是"我什么时候真的被收掉"。
    struct FakeTransport {
        dropped: std::sync::Arc<AtomicU32>,
    }

    impl Drop for FakeTransport {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl Transport for FakeTransport {
        fn connect(&mut self, _: Duration) -> Result<(), CallFault> {
            Ok(())
        }
        fn list_tools(&mut self, _: Duration) -> Result<Vec<RawTool>, CallFault> {
            Err(CallFault::Refused(McpFailure::Transport))
        }
        fn call_tool(
            &mut self,
            _: &str,
            _: serde_json::Value,
            _: Duration,
        ) -> Result<CallReply, CallFault> {
            Err(CallFault::Refused(McpFailure::Transport))
        }
    }

    fn fake_lease(
        pool: &std::sync::Arc<ChannelPool>,
        name: &str,
        idle: Duration,
        age: Duration,
    ) -> std::sync::Arc<AtomicU32> {
        let slot = pool.slot(name, idle);
        let dropped = std::sync::Arc::new(AtomicU32::new(0));
        slot.state.lock().expect("通道那一格只被短临界区持有").lease = Some(Lease {
            transport: Box::new(FakeTransport {
                dropped: std::sync::Arc::clone(&dropped),
            }),
            last_used: Instant::now()
                .checked_sub(age)
                .expect("测试用的时间偏移不该越过时钟起点"),
        });
        dropped
    }

    #[test]
    fn suffix_options_are_peeled_in_any_order() {
        let declaration = parse_declaration("notes=stdio:cmd,read-only,timeout=30").expect("合法");
        assert_eq!(declaration.effect(), McpEffect::ReadOnly);
        assert_eq!(declaration.timeout_seconds(), 30);
        assert_eq!(declaration.idle_seconds(), None);

        let declaration =
            parse_declaration("notes=stdio:cmd,timeout=30,read-only,idle=7").expect("合法");
        assert_eq!(declaration.effect(), McpEffect::ReadOnly);
        assert_eq!(declaration.timeout_seconds(), 30);
        assert_eq!(declaration.idle_seconds(), Some(7));
    }

    #[test]
    fn a_declaration_can_carry_its_own_idle_lease() {
        let idle = parse_declaration("lab=stdio:node x,idle=5").expect("合法");
        assert_eq!(idle.idle_seconds(), Some(5));
        let kept = parse_declaration("lab=stdio:node x,keep-alive").expect("合法");
        assert_eq!(kept.idle_seconds(), Some(0));

        assert!(
            parse_declaration("lab=stdio:node x,keep-alive,idle=5").is_err(),
            "两种写法只能选一个"
        );
        assert!(
            parse_declaration(&format!(
                "lab=stdio:node x,idle={}",
                MAX_CHANNEL_IDLE_SECONDS + 1
            ))
            .is_err(),
            "超过上限要拒绝，不能悄悄截断"
        );
        assert!(
            parse_declaration("lab=stdio:node x,idle=abc").is_err(),
            "不是整数就拒绝"
        );
    }

    #[test]
    fn the_pool_reaps_idle_channels_but_never_keep_alive_ones() {
        let pool = ChannelPool::new(8, Duration::from_secs(1));
        let idle = fake_lease(
            &pool,
            "idle",
            Duration::from_secs(1),
            Duration::from_secs(5),
        );
        let kept = fake_lease(&pool, "kept", Duration::ZERO, Duration::from_secs(5));

        assert_eq!(pool.open_count(), 2);
        pool.sweep();
        assert_eq!(pool.open_count(), 1, "只有闲够的那一台被收");
        assert_eq!(pool.reclaimed_count(), 1);
        assert_eq!(idle.load(Ordering::SeqCst), 1);
        assert_eq!(kept.load(Ordering::SeqCst), 0, "keep-alive 不按时间收");
    }

    #[test]
    fn the_pool_reclaims_the_least_recently_used_when_over_the_cap() {
        let pool = ChannelPool::new(2, Duration::from_secs(60));
        let oldest = fake_lease(&pool, "a", Duration::from_secs(60), Duration::from_secs(30));
        let middle = fake_lease(&pool, "b", Duration::from_secs(60), Duration::from_secs(20));
        let newest = fake_lease(&pool, "c", Duration::from_secs(60), Duration::from_secs(10));

        pool.sweep();
        assert_eq!(pool.open_count(), 2, "上限是硬边界");
        assert_eq!(oldest.load(Ordering::SeqCst), 1, "最久没用的先走");
        assert_eq!(middle.load(Ordering::SeqCst), 0);
        assert_eq!(newest.load(Ordering::SeqCst), 0);
        assert_eq!(pool.reclaimed_count(), 1);
    }

    #[test]
    fn removing_a_declaration_closes_its_channel_too() {
        let pool = ChannelPool::new(8, Duration::from_secs(60));
        let kept = fake_lease(&pool, "a", Duration::ZERO, Duration::from_secs(1));
        let removed_b = fake_lease(&pool, "b", Duration::ZERO, Duration::from_secs(1));
        let removed_c = fake_lease(&pool, "c", Duration::ZERO, Duration::from_secs(1));

        pool.retain(&["a".to_string()]);
        assert_eq!(kept.load(Ordering::SeqCst), 0);
        assert_eq!(removed_b.load(Ordering::SeqCst), 1);
        assert_eq!(removed_c.load(Ordering::SeqCst), 1);
        assert_eq!(pool.open_count(), 1);
    }

    #[test]
    fn the_own_file_and_the_launcher_share_one_pool() {
        let root = env_root("one-pool");
        std::fs::write(
            root.join(SELF_DECLARATIONS_FILE),
            "own=stdio:node x,keep-alive\n",
        )
        .unwrap();
        let pool = ChannelPool::host_default();
        let host = vec![McpAccess::with_pool(
            parse_declaration("host=stdio:node h").expect("合法"),
            credential_names(),
            std::sync::Arc::clone(&pool),
        )];
        let (declared, notes) = declarations_for_run(&host, &root, true, &pool);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(declared.len(), 2);
        assert!(
            std::sync::Arc::ptr_eq(&declared[0].channels(), &declared[1].channels()),
            "两种声明来源必须落在同一张池上——寿命规则只有一条"
        );
        assert_eq!(
            declared[1]
                .slot()
                .state
                .lock()
                .expect("通道那一格只被短临界区持有")
                .idle,
            Duration::ZERO,
            "`keep-alive` 落到这一格上"
        );
    }

    #[test]
    fn a_same_name_own_line_cannot_change_the_launcher_lease() {
        let root = env_root("lease-conflict");
        std::fs::write(
            root.join(SELF_DECLARATIONS_FILE),
            "db=stdio:own-command,keep-alive\n",
        )
        .unwrap();
        let pool = ChannelPool::host_default();
        let host = vec![McpAccess::with_pool(
            parse_declaration("db=stdio:host-command,idle=60").expect("合法"),
            credential_names(),
            std::sync::Arc::clone(&pool),
        )];
        let (declared, notes) = declarations_for_run(&host, &root, true, &pool);
        assert_eq!(declared.len(), 1, "同名以宿主为准");
        assert!(
            notes.iter().any(|note| note.contains("以宿主为准")),
            "{notes:?}"
        );
        assert_eq!(
            declared[0]
                .slot()
                .state
                .lock()
                .expect("通道那一格只被短临界区持有")
                .idle,
            Duration::from_secs(60),
            "同名行不能借 keep-alive 改掉宿主那台的租期"
        );
    }
}
