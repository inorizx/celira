//! stdio 通道：**起一条子进程，按行说 JSON**（v1 唯一的实现）。
//!
//! 它只做模块文档里那三件事：`connect`（起进程 + 握手）/ `list_tools` / `call_tool`。
//! 三件事的边界都是**有界**的：单行有上限（超限判这条通道坏了）、正文有上限（超限截断并报读数）、
//! 每一次等待都有窗口（到点先收树，再决定这次故障"确定了没有"）。
//!
//! ## 为什么默认 shell 是这台机器自带的命令行解释器（而不是 terminal 那档的 PowerShell）
//!
//! 声明是**宿主写的命令行**（`--mcp <名>=stdio:<命令>`），不是主体写的命令：主体只看得到动词，
//! 看不到命令。而真实世界的 MCP 声明长这样——`npx @playwright/mcp@latest`——那种写法在
//! PowerShell 里会被当成展开运算符。**声明是给宿主写的，就不该逼宿主先学会另一门词法。**
//! 因此这一档的默认 shell 是**这台机器自带的命令行解释器**：Windows 上是 `cmd.exe /C`，
//! Linux 上是 `bash -c`——命令词法随它（选择与拼装收在 `crate::platform` 的臂里，
//! 与 terminal 那只手共用同一份知识）。
//!
//! ## stderr 不搬
//!
//! 对面往 stderr 写什么，我们不读、不搬（那是它的日志，不是协议）。它接在**空设备**上，
//! 因此也不会因为写满管道把它自己卡住。
//!
//! ## 收进程树
//!
//! `cmd.exe → npx.cmd → node.exe` 这样的树要整棵收：Windows 上是 `taskkill /T /F`，
//! Linux 上按**进程组**发 `SIGKILL`（臂在 `crate::platform`）。
//! "杀不掉/杀不干净"是**第三种状态**（[`CallFault::Indeterminate`]），如实报出来——
//! 它原来是**未决效果门**要接的形状（门已退役：门是指令，而世界只该给信息；这一态本身照旧如实报）。
//! 通道被丢弃时（宿主退出、或它坏了被我们收回）也走同一条收尾。

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{
    CallFault, CallReply, MAX_TEXT_BYTES, MAX_TOOLS, McpDeclaration, McpFailure, RawTool,
    Transport, sanitize,
};

/// 我们在 **legacy 代**说的协议版本（`initialize` 的 `protocolVersion`）。
///
/// ⚠ **2026-10-13 更正一句从前写在这里的话**：它原来是"**不按版本分支**：我们只用 `tools`
/// 那一格，而那一格在所有已定版本里逐字相同"。**前半句现在不成立了**——`2026-07-28`
/// 把会话和握手一起删掉，两代的**说话方式**不同（一个靠握手、一个靠逐请求 `_meta`）。
/// ⇒ 今天按**世代**分支（见 [`ProtocolEra`]），而**这一版仍然是老话那一门**。
const PROTOCOL_VERSION: &str = "2024-11-05";

/// 自报的名字（对面在 `initialize` 的答复里会看到它）。
const CLIENT_NAME: &str = "celira-agent";

/// 我们在 **modern 代**说的协议版本：它进每一个请求的 `_meta`。
///
/// ⚠ 它与 [`PROTOCOL_VERSION`] **不是**"新旧二选一"，是**两门话**：
/// - 对面是 legacy ⇒ 说 [`PROTOCOL_VERSION`]（`initialize` 握手，请求不带 `_meta`）；
/// - 对面是 modern ⇒ 说**这一版**（没有握手，每个请求自带 `_meta`）。
///
/// 我们是**纯消费者**：只会说一门话就会丢掉另一门那边的所有 server。
/// ⇒ 所以这里是"**多会一门**"，不是"升级"（`PROTOCOL_VERSION` 一个字没动）。
const MODERN_PROTOCOL_VERSION: &str = "2026-07-28";

/// 探测的窗口。
///
/// 它**比调用窗口短得多**，因为它的读者不是对面而是我们：对面要么立刻回（modern 那两种），
/// 要么**根本不回**（legacy server 对未知的 pre-`initialize` 请求常常一声不响）。
/// ⇒ 拿 120 秒去等一句"它不会说"是把展开卡死。
const PROBE_WINDOW: Duration = Duration::from_secs(3);

/// `-32022`：spec `2026-07-28` 的 `UnsupportedProtocolVersionError`。
///
/// ⚠ **认 modern 只认这一个码。** spec 对**回退**有明文 MUST NOT（"不许按单一错误码判断"），
/// 因为 legacy server 回的是**实现自定义**的码；反过来，**认** modern 也必须有唯一判据，
/// 否则"对面是 modern"这句话就没有根据。
const MODERN_ERROR_UNSUPPORTED_VERSION: i64 = -32022;

/// 单条消息的字节上限：超限就判这条通道坏了（那已经不是我们搬运的形状）。
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// 翻清单时最多翻几页。
const MAX_PAGES: usize = 8;

/// 轮询子进程的间隔（"到点就收"而不是被一次阻塞等待拖住）。
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// 收树之后等它真正消失的窗口。
///
/// ⚠ 它是**这一格的读数**的判据，不是"多久算够"的猜测：收干净了才敢说确定的 `timeout`，
/// 没等到就只能说保守的 `Indeterminate`（[`StdioTransport::on_timeout`]）。生产用这个缺省值；
/// **用例可以换一个**（[`StdioTransport::with_kill_window`]）——因为"等到"在满载机器上与空闲
/// 机器上不是同一个数，而门禁不该对机器负载敏感（§4 登记过那一条假红）。
const KILL_WINDOW: Duration = Duration::from_secs(10);

/// 通道被丢弃时等它消失的窗口（宿主退出的路径上不该被它拖住，因此比 [`KILL_WINDOW`] 短）。
const DROP_WINDOW: Duration = Duration::from_secs(3);

/// 读手线程送回来的东西。
enum Incoming {
    /// 一行（原文）。
    Line(String),
    /// 对面把输出关了（进程没了，或者它自己不说了）。
    Closed,
    /// 这一行超过了单条上限，或者读它出错了：这条通道不能再用了。
    Broken,
}

/// 收回来的一次答复。
enum Reply {
    /// 对面回了一个对象，带 `result`。
    Payload(Value),
    /// 对面回了一个**错误对象**（原文只作携带）。
    Refused(Value),
}

/// 等一条答复的三种出口（[`StdioTransport::wait_for_reply`] 的唯一产物）。
///
/// ⚠ **三条出口必须分开**：调用路径要把"到点"读成一次故障（而且要先收树），
/// 探测路径要把"到点"读成"没问出来"（**不杀进程**）。合成两条就会让其中一条撒谎。
enum Waited {
    /// 答复到了。
    Reply(Reply),
    /// 通道关了：请求交出去了，答复却没来。
    Closed,
    /// 到点了。
    Expired,
}

/// 从一个对象里读一串版本名；认不出就是**空表**（**不猜**，也不替对面编一个）。
///
/// ⚠ 空表与"没有这一格"**在这里是同一件事**，因为下游的判据是"**有没有我们会的那一版**"——
/// 空表与缺格在那个判据下答案相同（都没有）。⇒ 不为它们分两格（那会是没有消费者的第二格）。
fn versions_in(source: &Value, field: &str) -> Vec<String> {
    source
        .get(field)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// **对面**说的是哪一代协议（spec `2026-07-28` 的分代术语）。
///
/// ⚠ 它**不是**"我们支持哪个版本"——我们**两代都会说**（legacy 走 `initialize`、
/// modern 走逐请求 `_meta`）。这个字段回答的是"**该怎么跟它说话**"。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProtocolEra {
    /// 靠 `initialize` 握手建会话的那一代（`2025-11-25` 及以前）。
    Legacy,
    /// 没有握手、每个请求自带 `_meta` 的那一代（`2026-07-28` 及以后）。
    Modern,
}

/// 握手被拒之后**问出来的三种结局**（[`StdioTransport::classify_refusal`] 的唯一产物）。
///
/// ⚠ **三条必须分开**：一条通向"换一门话说下去"、一条通向一个精确的失败分类、
/// 一条通向今天那句含糊的兜底。合成两条就会让其中一条撒谎。
enum Refusal {
    /// 对面是 modern，而且它说我们会的那一版 ⇒ **改用现代框子接着说**（Dual-era 的那一半）。
    SpeakModern,
    /// 对面是 modern，但它支持的版本里**没有我们会说的任何一个**。
    Unsupported,
    /// 探不出（legacy，或通道坏了，或它答的东西读不懂）⇒ 维持"握手没成、原因不明"。
    Unknown,
}

/// 一个结果的 `resultType`（spec `2026-07-28`：所有结果都**必须**带它）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ResultType {
    /// 普通结果。
    Complete,
    /// **对面要多问一句才肯办**（MRTR）。本版**不实现**那一格——见 `call_tool` 的取舍。
    InputRequired,
}

/// 读一个结果的 `resultType`。
///
/// ⚠ **缺了就是 [`ResultType::Complete`]**——spec 明文：客户端 **MUST** 把老 server
/// 缺这个字段的结果当 `"complete"`。这不是宽容，是**跨代兼容的判据**：
/// legacy server 根本不知道有这个字段。
fn result_type(result: &Value) -> ResultType {
    match result.get("resultType").and_then(Value::as_str) {
        Some("input_required") => ResultType::InputRequired,
        _ => ResultType::Complete,
    }
}

/// 一条 stdio 通道。
pub struct StdioTransport {
    child: Child,
    /// 写给对面的那一端；收尾之后置空（不再写）。
    stdin: Option<ChildStdin>,
    /// 读手线程送回来的行。
    incoming: Receiver<Incoming>,
    /// 下一个请求号（只增）。
    next_id: u64,
    /// 起子进程那一刻的洗凭据读数。
    scrubbed: usize,
    unscrubbed: Vec<String>,
    /// 读进来但**不是一个对象**的行数（噪声读数，0 时不展）。
    noise: usize,
    /// **对面是哪一代**（决定"该怎么跟它说话"）。
    ///
    /// ⚠ 它在 [`Transport::connect`] 里定下来，**之后不再变**——era 是**server 的属性**，
    /// 不是单个请求的属性（spec 明文）。⇒ 今天它**只在进程内**活着：spec 说客户端 MAY
    /// 跨重启持久化，而我们**不做**（今天没有消费者，那会是为想象中的未来建设）。
    era: ProtocolEra,
    /// **收树窗口**：到点之后等树真正消失的上限（缺省 [`KILL_WINDOW`]，用例可换）。
    kill_window: Duration,
    /// **收树那一手的答案**：`None` = 用平台臂自己的读数（生产）；`Some(x)` = 用例给的确定答案。
    ///
    /// ⚠ 注入的是**读数，不是行为**：真进程照旧由平台臂收掉（[`Self::collect`] 每次都调它），
    /// "窗口内等到它消失"那一半也照旧量真的。为什么允许换答案：那一手的成败**不在我们手里**
    /// （命令没找到进程、或它自己起不来，都返回 `false` ⇒ 如实读成保守的 `Indeterminate`），
    /// 而"收干净了才是确定的那一格"这条规则必须能被**确定地**量到（§4 登记过那条假红）。
    kill_answer: Option<bool>,
    /// **握手被拒时对面自己那句话**（已消毒、已截断的原文）。
    ///
    /// 它治的是登记在案的那一格：拒握手时我们只留下一个分类，**对面的诊断没有落脚处**
    /// （见 `mcp` 模块文档"一处如实的取舍"）。分类**照旧**只有一个、逐字节稳定；这一格是
    /// 它**旁边**的读数——`AppPort::present()` 失败时把它交给 OS，由 OS 贴成随单内容。
    ///
    /// **只记"被拒"那一句**：`Some` 的含义就是"对面在握手上回了一个错误对象，这是它的原文"。
    /// 探代之后改用现代框子接着说（[`Refusal::SpeakModern`]）⇒ **握手其实成了**，
    /// 这一格与那条路无关。
    refusal_detail: Option<String>,
}

impl StdioTransport {
    /// 起一条 stdio 通道（**本平台有臂**：Windows＝`cmd.exe /C`、Linux＝`bash -c`——
    /// 命令的拼装与收树收在 `crate::platform` 里，与 terminal 那只手同一份知识）。
    #[cfg(any(windows, target_os = "linux"))]
    pub fn spawn(declaration: &McpDeclaration, scrub: &[String]) -> Result<Self, McpFailure> {
        let (mut environment, scrubbed, unscrubbed) = crate::platform::child_environment(scrub);
        let mut builder = crate::platform::mcp_command(
            crate::platform::MCP_SHELL,
            declaration.command(),
            &mut environment,
        );
        builder
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // 对面的日志不搬（接空设备：它写多少都不会被管道卡住）。
            .stderr(Stdio::null());
        // 洗过的那一份**先清空再交**（只叠加等于没洗——规矩在 `crate::platform` 里，两个消费者共用）。
        crate::platform::apply_environment(&mut builder, environment);
        let mut child = builder.spawn().map_err(|_| McpFailure::SpawnFailure)?;
        let Some(stdin) = child.stdin.take() else {
            let _ = crate::platform::kill_tree(child.id());
            return Err(McpFailure::SpawnFailure);
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = crate::platform::kill_tree(child.id());
            return Err(McpFailure::SpawnFailure);
        };
        let (sender, incoming) = channel();
        // 读手线程**不 join**：它随输出关闭自行结束（收尾时我们会把树收掉，那个管道随之关闭）。
        let _ = thread::spawn(move || read_loop(stdout, sender));
        Ok(StdioTransport {
            child,
            stdin: Some(stdin),
            incoming,
            next_id: 0,
            scrubbed,
            unscrubbed,
            noise: 0,
            // 还没连上 ⇒ 先按老话算（`connect` 会把它定下来；定下来之前不发任何请求）。
            era: ProtocolEra::Legacy,
            kill_window: KILL_WINDOW,
            kill_answer: None,
            // 还没握过手 ⇒ 还没有"被拒时那句话"。
            refusal_detail: None,
        })
    }

    /// **换一个收树窗口**（缺省 [`KILL_WINDOW`]，生产路径不调它）。
    ///
    /// 为什么可注入：这一格决定的是"收干净了吗"这条**读数**——收干净 ⇒ 确定的 `timeout`；
    /// 窗口内没等到 ⇒ 保守的 `Indeterminate`。而"等到"与机器负载有关 ⇒ 一条**用例**（"收干净了
    /// 才是确定的那一格"）会随负载偶发假红。用例给一个宽窗口，那条读数就**由构造保证**，
    /// 不再靠希望（§4 登记过那一条假红）。
    pub fn with_kill_window(mut self, window: Duration) -> Self {
        self.kill_window = window;
        self
    }

    /// **换一个"收树那一手的答案"**（缺省＝问平台臂，生产路径不调它）。
    ///
    /// 注入的是**读数，不是行为**：收树照旧由平台臂做，等它消失照旧是真的。用例给一个确定的
    /// 答案，是因为那一手的成败**不在我们手里**（见 [`Self::kill_answer`]）。
    pub fn with_kill_answer(mut self, answer: bool) -> Self {
        self.kill_answer = Some(answer);
        self
    }

    /// **握手被拒时对面自己那句话**（没有就是没有）。
    ///
    /// 唯一消费者是 [`crate::mcp::app::McpApp::present`]：`connect` 失败之后它把这一格交给 OS，
    /// 由 OS 贴成展开失败的随单内容。**它不参与任何判断**——分类是分类，这句话是内容。
    ///
    /// ⚠ 与 [`crate::mcp::CallReply::Refused`] 的 `detail` **同一条纪律**：已消毒、已截断，
    /// 只作携带（对面说了什么就是什么，我们不改写它）。
    pub fn refusal_detail(&self) -> Option<&str> {
        self.refusal_detail.as_deref()
    }

    /// 平台缝：**本平台没有这一档的实现**（入口已经拒绝开启 `--mcp`）。万一走到这里，
    /// 如实说"没跑起来"，绝不换一个近似行为。
    #[cfg(not(any(windows, target_os = "linux")))]
    pub fn spawn(_declaration: &McpDeclaration, _scrub: &[String]) -> Result<Self, McpFailure> {
        Err(McpFailure::SpawnFailure)
    }

    /// 下一个请求号。
    fn take_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// 写一行出去。写不进去 = **确定没发出去**（通道不在了）。
    fn write_line(&mut self, value: &Value) -> Result<(), McpFailure> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(McpFailure::Transport);
        };
        let mut line = serde_json::to_string(value).map_err(|_| McpFailure::Transport)?;
        line.push('\n');
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .map_err(|_| McpFailure::Transport)
    }

    /// 等一条**属于这个请求号**的答复。别的行一律不看：
    ///
    /// - 通知（没有 `id`）是合法协议流量，静静跳过；
    /// - 迟到的答复（`id` 不是我们等的那一个）也跳过；
    /// - 读不懂的行**计入噪声读数**，同样跳过（大多数 server 会往输出里写日志，那是它的毛病，
    ///   不是我们的协议——但也不能安静地当没发生，因此那个数会进回执）。
    fn await_reply(&mut self, id: u64, timeout: Duration) -> Result<Reply, CallFault> {
        match self.wait_for_reply(id, timeout) {
            Waited::Reply(reply) => Ok(reply),
            Waited::Closed => Err(CallFault::Indeterminate(McpFailure::Transport)),
            Waited::Expired => Err(self.on_timeout()),
        }
    }

    /// **探测**用的问话：到点就当没问过——**不杀进程、不改通道状态**。
    ///
    /// 它与 [`Self::await_reply`] 的差别**只有一处**：到点**不调 [`Self::on_timeout`]**。
    /// 那条路是给"请求已经交出去了、这次调用的效果不明"用的（写档要的就是那一格）；
    /// 而**探测不是一次调用，是一句问话**：问不到就走兜底分类，通道由 `present()` 失败时
    /// 那个 `Drop` 去收（那里本来就会收）。
    ///
    /// ⚠ 因此它**不许**用在调用路径上——那条路必须区分"确定"与"效果不明"。
    fn peek_reply(&mut self, id: u64, window: Duration) -> Option<Reply> {
        match self.wait_for_reply(id, window) {
            Waited::Reply(reply) => Some(reply),
            Waited::Closed | Waited::Expired => None,
        }
    }

    /// 等一条指定 `id` 的答复——**两条路共用的一份**（策略在调用点，循环只写一遍）。
    fn wait_for_reply(&mut self, id: u64, timeout: Duration) -> Waited {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Waited::Expired;
            }
            match self.incoming.recv_timeout(left) {
                Ok(Incoming::Line(text)) => {
                    let Some(value) = as_object(&text) else {
                        self.noise += 1;
                        continue;
                    };
                    if value.get("id").and_then(Value::as_u64) != Some(id) {
                        continue;
                    }
                    if let Some(error) = value.get("error") {
                        return Waited::Reply(Reply::Refused(error.clone()));
                    }
                    return Waited::Reply(Reply::Payload(value));
                }
                Ok(Incoming::Closed) | Ok(Incoming::Broken) => return Waited::Closed,
                Err(RecvTimeoutError::Timeout) => return Waited::Expired,
                Err(RecvTimeoutError::Disconnected) => return Waited::Closed,
            }
        }
    }

    /// 握手被拒之后，问出**这次该怎么收场**。
    ///
    /// spec `2026-07-28` / stdio 的处方是"**先**探代"。我们**反过来**：先 `initialize`，
    /// **只在它被拒之后**才探。理由是 spec 自己也承认的一条事实——`initialize` **不是**
    /// era-ambiguous 的方法（modern server 对它一律**拒绝**，见兼容矩阵 "Legacy | Modern ⇒ Fails"），
    /// 而 spec 那句 SHOULD 的理由恰恰是"某些 legacy server 会把 era-ambiguous 的方法
    /// （如 `tools/call`）当 legacy 处理"。⇒ 我们**在知道世代之前从不发 era-ambiguous 的方法**，
    /// 那条理由不适用，而反过来做买到**零额外延迟**（每一次成功的展开都不多一次往返）。
    /// ⚠ 这是**登记在案的偏离**，不是"没读 spec"。
    ///
    /// 判据（照抄 spec 的三结果，一条都不简化）：
    /// - 回 `DiscoverResult` ⇒ **modern**；
    /// - 回**已识别的 modern 错误**（`-32022`）⇒ **modern**；
    /// - **其他任何东西**（别的码 / 超时 / 看不懂）⇒ **探不出** ⇒ 维持 `handshake-failure`。
    ///
    /// ⚠ 探出 modern 之后还要再分一次：**它支持的版本里有没有我们会的那一门？**
    /// - **有** ⇒ [`Refusal::SpeakModern`]：**接着说下去**（改用现代框子），不报错；
    /// - **没有** ⇒ [`Refusal::Unsupported`]：这一格才叫"版本不支持"。
    fn classify_refusal(&mut self, timeout: Duration) -> Refusal {
        let window = PROBE_WINDOW.min(timeout);
        match self.probe_modern(window) {
            Some(versions) if versions.iter().any(|each| each == MODERN_PROTOCOL_VERSION) => {
                Refusal::SpeakModern
            }
            Some(_) => Refusal::Unsupported,
            None => Refusal::Unknown,
        }
    }

    /// 这一次请求要带的 `_meta`：**modern 代才有**；legacy 代是 `None`（一个字段都不加）。
    ///
    /// spec `2026-07-28`：每个请求都 **MUST** 带 `protocolVersion` 与 `clientCapabilities`，
    /// **SHOULD** 带 `clientInfo`。⇒ 三样都给。
    fn request_meta(&self) -> Option<Value> {
        match self.era {
            ProtocolEra::Legacy => None,
            ProtocolEra::Modern => Some(json!({
                "io.modelcontextprotocol/protocolVersion": MODERN_PROTOCOL_VERSION,
                "io.modelcontextprotocol/clientInfo": {
                    "name": CLIENT_NAME,
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "io.modelcontextprotocol/clientCapabilities": {},
            })),
        }
    }

    /// 把 `_meta` 并进 `params`（**modern 代才并**；legacy 代原样返回，一个字段都不多）。
    fn with_meta(&self, params: Value) -> Value {
        let Some(meta) = self.request_meta() else {
            return params;
        };
        match params {
            Value::Object(mut object) => {
                object.insert("_meta".to_string(), meta);
                Value::Object(object)
            }
            other => other,
        }
    }

    /// 发一次 `server/discover` 问代。
    ///
    /// 返回 `Some(它支持的版本列表)` ＝ **对面是 modern**；`None` ＝ **探不出**
    /// （legacy，或通道坏了，或它答的东西读不懂）。
    ///
    /// ⚠ **它不改通道状态**：只是一句问话，问不到就当没问过——兜底分类仍是
    /// `handshake-failure`，所以这条路的**最坏结果就是今天的行为**。
    fn probe_modern(&mut self, window: Duration) -> Option<Vec<String>> {
        let id = self.take_id();
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "server/discover",
            "params": {"_meta": {
                "io.modelcontextprotocol/protocolVersion": MODERN_PROTOCOL_VERSION,
                "io.modelcontextprotocol/clientInfo": {
                    "name": CLIENT_NAME,
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "io.modelcontextprotocol/clientCapabilities": {},
            }},
        });
        self.write_line(&request).ok()?;
        match self.peek_reply(id, window)? {
            // 一：对面回了一个结果对象 ⇒ 它会说现代协议。
            Reply::Payload(payload) => {
                let result = Self::require_result(&payload, McpFailure::HandshakeFailure).ok()?;
                Some(versions_in(&result, "supportedVersions"))
            }
            // 二：对面回**版本协商错误** ⇒ 它同样是 modern（它认识这个码，就说明它说现代协议）。
            // 版本列表在 `data.supported` 里（spec 的 `UnsupportedProtocolVersionError` 形状）。
            Reply::Refused(error) => match error.get("code").and_then(Value::as_i64) {
                Some(MODERN_ERROR_UNSUPPORTED_VERSION) => {
                    error.get("data").map(|data| versions_in(data, "supported"))
                }
                // 三：别的码一律当 legacy——spec 明文：legacy server 回**实现自定义**的码
                // （常见 `-32601` / `-32602`）**或者干脆不回**。⇒ **不按单一码判断回退**。
                _ => None,
            },
        }
    }

    /// 到点了：先尽力把树收干净，再决定这次故障是"确定"还是"效果不明"。
    ///
    /// 窗口给得比丢弃时宽：这里"收干净"这一步是有信息量的（收干净了才敢说 `timeout`）。
    fn on_timeout(&mut self) -> CallFault {
        let collected = self.collect(self.kill_window);
        timeout_reading(collected)
    }

    /// 尽力收掉整棵树，并在窗口内等它真的消失；返回"收干净了吗"。
    ///
    /// 收树那一手**每次都真的调平台臂**；`kill_answer` 只换它报回来的那个读数（生产是 `None`
    /// ⇒ 原样用它自己的读数）。
    fn collect(&mut self, window: Duration) -> bool {
        // 先把写的那一端关掉：收尾之后不再往一条死通道里写。
        self.stdin = None;
        let pid = self.child.id();
        let signalled = crate::platform::kill_tree(pid);
        let signalled = self.kill_answer.unwrap_or(signalled);
        let gone = wait_within(&mut self.child, window).is_some();
        signalled && gone
    }

    /// 从对面回的对象里取 `result`；读不懂就是这一格"答复不合形状"。
    fn require_result(payload: &Value, class: McpFailure) -> Result<Value, CallFault> {
        payload
            .get("result")
            .cloned()
            .ok_or(CallFault::Refused(class))
    }
}

/// **"收干净了吗" → 这一次故障是确定的还是保守的**：规则只有这一处。
///
/// 收干净了（击杀被系统接受 **且** 窗口内等到它真的消失）才敢说确定的
/// [`McpFailure::Timeout`]；否则只能说保守的 [`CallFault::Indeterminate`]——"请求发出去了、
/// 效果不明"正是那一格存在的理由。
///
/// ⚠ **为什么单独是一个函数**：这条规则的两半里有一半**不在我们手里**（`kill_tree` 报的是
/// 系统那条击杀命令自己的成败）。集成用例因此有一个它保证不住的前提——满载时那条命令自己
/// 失败（或子进程消失得比第一次轮询还快），读数就会如实翻成保守的那一格，用例偶发假红
/// （§4 登记过）。把这个函数**与进程无关地**钉住，规则本身就有了确定的证据；集成用例继续
/// 量真实进程那一半（⚠ 实测：把收树窗口缩到 1 毫秒、8 份并行满载跑，那条用例仍全绿 ⇒
/// 窗口不是那一次假红的机制，如实登记在案）。
fn timeout_reading(collected: bool) -> CallFault {
    if collected {
        CallFault::Refused(McpFailure::Timeout)
    } else {
        CallFault::Indeterminate(McpFailure::Timeout)
    }
}

impl Transport for StdioTransport {
    fn connect(&mut self, timeout: Duration) -> Result<(), CallFault> {
        let id = self.take_id();
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION")},
            },
        });
        self.write_line(&request).map_err(CallFault::Refused)?;
        match self.await_reply(id, timeout)? {
            Reply::Payload(payload) => {
                let _ = Self::require_result(&payload, McpFailure::HandshakeFailure)?;
                self.era = ProtocolEra::Legacy;
            }
            // 对面**回了错误**：它可能只是**不说老话**——modern server 对 `initialize` 一律拒绝
            // （见 spec 的兼容矩阵 "Legacy | Modern ⇒ Fails"）。
            //
            // ⚠ **2026-10-13**：从前这里一律 `handshake-failure`——"接不上，但不知道为什么"。
            // 现在**只在失败路径上**探一次代（成功路径一个字不变、**零额外延迟**）：
            // - 探得出"它是 modern、而且它说我们会的那一版" ⇒ **改用现代框子接着说**；
            // - 探得出但它说的版本我们一个都不会 ⇒ `unsupported-version`；
            // - 探不出 ⇒ 维持 `handshake-failure`（今天那句）。
            Reply::Refused(error) => {
                // **先把对面那句话留下**（已消毒、已截断），再决定这一格该怎么收场：
                // 无论下面判成哪一类，这句话都是"对面为什么拒"的唯一原文——
                // 从前它在这里被整个丢掉（`Reply::Refused(_)`），记录因此只剩一个分类。
                self.refusal_detail = Some(describe_error(&error));
                match self.classify_refusal(timeout) {
                    Refusal::SpeakModern => {
                        self.era = ProtocolEra::Modern;
                        // ⚠ **不发** `notifications/initialized`：那一格属于 legacy 的握手，
                        // 而 modern 把会话和握手一起删掉了（spec `2026-07-28`）。
                        return Ok(());
                    }
                    Refusal::Unsupported => {
                        return Err(CallFault::Refused(McpFailure::UnsupportedVersion));
                    }
                    Refusal::Unknown => {
                        return Err(CallFault::Refused(McpFailure::HandshakeFailure));
                    }
                }
            }
        }
        // 收尾通知：**只有 legacy 代有这一步**。
        let _ = self.write_line(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        }));
        Ok(())
    }

    fn list_tools(&mut self, timeout: Duration) -> Result<Vec<RawTool>, CallFault> {
        let mut tools: Vec<RawTool> = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let id = self.take_id();
            let params = match &cursor {
                Some(after) => json!({"cursor": after}),
                None => json!({}),
            };
            // modern 代：每个请求自带 `_meta`（legacy 代原样，一个字段都不多）。
            let params = self.with_meta(params);
            let request = json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/list",
                "params": params,
            });
            self.write_line(&request).map_err(CallFault::Refused)?;
            let payload = match self.await_reply(id, timeout)? {
                Reply::Payload(payload) => payload,
                Reply::Refused(_) => return Err(CallFault::Refused(McpFailure::ToolsFailed)),
            };
            let result = Self::require_result(&payload, McpFailure::ToolsFailed)?;
            // `input_required` 对**清单**没有意义（清单不是一次"要多问一句"的动作）：
            // 对面若这么答，那就是答复不合形状。
            if result_type(&result) == ResultType::InputRequired {
                return Err(CallFault::Refused(McpFailure::ToolsFailed));
            }
            let page = result
                .get("tools")
                .and_then(Value::as_array)
                .ok_or(CallFault::Refused(McpFailure::ToolsFailed))?;
            for item in page {
                tools.push(read_tool(item).ok_or(CallFault::Refused(McpFailure::ToolsFailed))?);
            }
            if tools.len() > MAX_TOOLS {
                return Err(CallFault::Refused(McpFailure::TooManyTools));
            }
            match result.get("nextCursor").and_then(Value::as_str) {
                Some(after) if !after.is_empty() => cursor = Some(after.to_string()),
                _ => return Ok(tools),
            }
        }
        // 翻到页数上限还有下一页：这台 server 的清单不是我们能搬运的形状。
        Err(CallFault::Refused(McpFailure::TooManyTools))
    }

    fn call_tool(
        &mut self,
        tool: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<CallReply, CallFault> {
        let id = self.take_id();
        // modern 代：每个请求自带 `_meta`（legacy 代原样）。
        let params = self.with_meta(json!({"name": tool, "arguments": arguments}));
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": params,
        });
        // 写不进去 = 这次请求**确定**没交出去（通道不在了）。
        self.write_line(&request).map_err(CallFault::Refused)?;
        match self.await_reply(id, timeout)? {
            Reply::Refused(error) => Ok(CallReply::Refused {
                code: error.get("code").and_then(Value::as_i64),
                detail: describe_error(&error),
            }),
            Reply::Payload(payload) => {
                let result = Self::require_result(&payload, McpFailure::ServerError)?;
                // ⚠ **丙1（2026-10-13 裁）**：对面回 `input_required` ＝ 它要多问一句才肯办
                // （spec `2026-07-28` 的 MRTR）。本版**不实现**那一格——那要引入
                // "回头再问一次"的**执行语义**，不是 `mcp/` 一层能定的 ⇒ 当作**一次如实的拒绝**。
                // ⚠ **不许当成功**：那是把"对面没说完"读成"对面说完了"。
                if result_type(&result) == ResultType::InputRequired {
                    return Ok(CallReply::Refused {
                        code: None,
                        detail: "对面回的是 `input_required`：它要多问一句才肯办这件事（MRTR）。\
                                 本版不实现那一格，因此这一次没有办。"
                            .to_string(),
                    });
                }
                Ok(read_result(&result, self.noise))
            }
        }
    }

    fn credential_reading(&self) -> Option<(usize, Vec<String>)> {
        Some((self.scrubbed, self.unscrubbed.clone()))
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        let _ = self.collect(DROP_WINDOW);
    }
}

/// 把一行读成一个**对象**（不是一个对象就当噪声：剥掉可能的前导字节序标记与空白再解析）。
fn as_object(text: &str) -> Option<Value> {
    let cleaned = text.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
    let value: Value = serde_json::from_str(cleaned).ok()?;
    value.is_object().then_some(value)
}

/// 错误对象 → 一句**只作携带**的原文（已消毒、已截断）。
///
/// ⚠ **只搬运"对面说出来的话"**（2026-10-13 收口，理由见下）：
/// - 有 `message` ⇒ `code=<码>: <那句话>`（码是附带事实，不是我们编的）；
/// - **没有 `message`** ⇒ 只给 `code=<码>`。
///
/// **不再把整个错误对象 JSON 化当原文。** 那条旧写法有两个毛病：
/// ① 它**不是"对面的话"**，是我们把对面的**结构**序列化了一遍——记进"原文"那一格就是
/// 记录在替对面说话；② 那一格是**文本**，`data` 是任意嵌套结构（可以很大、可以有我们
/// 读不懂的东西），把它摊平成文本，等于让一个"携带原文"的格子去承载结构。
/// 结构该有自己的格子，那是另一件事（见 `mcp` 模块文档里登记的那两格）。
///
/// ⇒ 缺 `message` 时**如实只报码**："对面没留话"与"我们读到了话"因此分得开。
fn describe_error(error: &Value) -> String {
    let code = error.get("code").and_then(Value::as_i64);
    let raw = match error.get("message").and_then(Value::as_str) {
        Some(message) => match code {
            Some(code) => format!("code={code}: {message}"),
            None => message.to_string(),
        },
        None => match code {
            Some(code) => format!("code={code}"),
            // 连码都没有：那就真的没有可携带的东西了。
            None => String::new(),
        },
    };
    let clean = sanitize(&raw);
    let (clipped, truncated) = super::clip(&clean, 400);
    if truncated {
        format!("{clipped}…（截断）")
    } else {
        clipped
    }
}

/// 清单里的一条工具。名字是**必须有**的；说明与参数模式可以缺。
fn read_tool(item: &Value) -> Option<RawTool> {
    let name = item.get("name").and_then(Value::as_str)?.to_string();
    if name.is_empty() {
        return None;
    }
    Some(RawTool {
        name,
        description: item
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        input_schema: item
            .get("inputSchema")
            .cloned()
            .unwrap_or_else(|| json!({})),
    })
}

/// 一次调用的答复 → 有界的回执形状。
///
/// **有界**是这里做的：正文只留到 [`MAX_TEXT_BYTES`]（读的时候就不再多收），而原文一共多少字节
/// 照旧报出来。非文本的块**不搬**，只按类型记条数——主体看到的是"有什么"，不是"几兆的字节"。
fn read_result(result: &Value, noise: usize) -> CallReply {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut text = String::new();
    let mut text_bytes = 0usize;
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();

    if let Some(blocks) = result.get("content").and_then(Value::as_array) {
        for block in blocks {
            let kind = block
                .get("type")
                .and_then(Value::as_str)
                .map(sanitize)
                .filter(|each| !each.is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            if kind != "text" {
                *counts.entry(kind).or_insert(0) += 1;
                continue;
            }
            let part = block.get("text").and_then(Value::as_str).unwrap_or("");
            text_bytes += part.len();
            add_bounded(&mut text, part);
        }
    }
    // 结构化的那一份我们也不搬（v1 只搬文本）——它出现就要看得见。
    if result.get("structuredContent").is_some() {
        *counts.entry("structured-content".to_string()).or_insert(0) += 1;
    }

    CallReply::Replied {
        is_error,
        truncated: text_bytes > MAX_TEXT_BYTES,
        text,
        text_bytes,
        unsupported: counts.into_iter().collect(),
        noise,
    }
}

/// 往正文里追加，**只追加到上限为止**（不再多收）。
fn add_bounded(text: &mut String, part: &str) {
    if text.len() >= MAX_TEXT_BYTES {
        return;
    }
    if !text.is_empty() {
        text.push('\n');
    }
    let room = MAX_TEXT_BYTES.saturating_sub(text.len());
    let (piece, _) = super::clip(part, room);
    text.push_str(&piece);
}

/// 有界地读一行；返回它是什么。
enum LineRead {
    /// 读到一整行（不含行尾）。
    Line,
    /// 输出到头了。
    End,
    /// 这一行超过了单条上限（这条通道不能再用了）。
    TooLong,
}

/// 按**行**读，而且**有上限**：一个不肯换行、或者一行塞几百兆的对面，不该把我们撑住。
fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    out: &mut Vec<u8>,
    cap: usize,
) -> std::io::Result<LineRead> {
    out.clear();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(if out.is_empty() {
                LineRead::End
            } else {
                LineRead::Line
            });
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.unwrap_or(available.len());
        if out.len() + take > cap {
            return Ok(LineRead::TooLong);
        }
        // 先把这一段拷出来，读完再交回缓冲（断掉借用，不留一个悬着的切片）。
        let chunk: Vec<u8> = available[..take].to_vec();
        reader.consume(take + usize::from(newline.is_some()));
        out.extend_from_slice(&chunk);
        if newline.is_some() {
            return Ok(LineRead::Line);
        }
    }
}

/// 读手线程：把子进程的输出按行送回来（**只送、不判断**）。
fn read_loop(stdout: ChildStdout, sender: Sender<Incoming>) {
    let mut reader = BufReader::new(stdout);
    let mut buffer: Vec<u8> = Vec::new();
    loop {
        let next = match read_bounded_line(&mut reader, &mut buffer, MAX_LINE_BYTES) {
            Ok(LineRead::Line) => Incoming::Line(String::from_utf8_lossy(&buffer).into_owned()),
            Ok(LineRead::End) => Incoming::Closed,
            Ok(LineRead::TooLong) | Err(_) => Incoming::Broken,
        };
        let done = matches!(next, Incoming::Closed | Incoming::Broken);
        if sender.send(next).is_err() {
            return;
        }
        if done {
            return;
        }
    }
}

/// 在给定窗口内等它真的消失（`None` = 窗口内没等到）。
fn wait_within(child: &mut Child, window: Duration) -> Option<std::process::ExitStatus> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// **"收干净了才是确定的那一格"——这条规则本身，与进程无关地钉住。**
    ///
    /// 为什么要有它：集成用例（`tests/mcp_app.rs`）量的是真进程，而它的前提里有一半不在我们
    /// 手里（系统那条击杀命令自己的成败）⇒ 满载时会如实翻成保守的那一格、用例偶发假红
    /// （§4 登记过）。规则在这里是**确定的**：两个输入四种组合，逐格点名。
    #[test]
    fn the_timeout_reading_is_determined_only_by_a_clean_collection() {
        // 收干净了（击杀被接受 ＋ 窗口内等到它消失）⇒ 确定的 `timeout`。
        assert!(matches!(
            timeout_reading(true),
            CallFault::Refused(McpFailure::Timeout)
        ));
        // 没等到 ⇒ 保守的"效果不明"（不是成功、也不是另一个分类）。
        let conservative = timeout_reading(false);
        assert!(matches!(
            conservative,
            CallFault::Indeterminate(McpFailure::Timeout)
        ));
        assert!(conservative.is_indeterminate());
        assert_eq!(conservative.class(), McpFailure::Timeout);
        // 负向对照：两条路的**分类相同、确定度相反**——不许把它们写成同一格。
        assert_ne!(
            format!("{:?}", timeout_reading(true)),
            format!("{:?}", timeout_reading(false))
        );
    }
}
