//! 格式与校验和（2026-10-08 从 `store.rs` 拆出；**盘上格式一个字节没动**）。
//!
//! 盘上的一行就是**一个 JSON 对象 + 换行**，外加行尾一枚校验和；一串记录靠**链值**串起来。
//! 这一块是全部四块里**最内聚**的一块，也是所有东西都依赖的一块：
//!
//! - **形状**：`TraceRecord` / `TracePayload` / `FactRow` / `RunSummary` / `TracePage` / `WakeOrigin`；
//! - **编码/解码**：`encode_*` / `decode_*`（认不出的取值**明确拒绝**，不就近猜）；
//! - **校验和链**：`line_sum` / `chain_checksum` / `verified_line`（正文逐字节可核）；折链核验的
//!   那一半与几条读路径在 `store/environment.rs` 与 `store/parts.rs`——边界见下面的「射程」；
//! - **写前的封闭表**：`checked_failure_class`（模型失败分类在落盘之前过一遍词表——
//!   它是"持久字段上的词表一律 fail-closed"缺的另一半）。
//!
//! **它不碰文件系统**（纯字符串工作）：四块里唯一如此的一块 ⇒ 它**不在** `FILE_SYSTEM_SOURCES`
//! 里；而 `serde_json` 照旧**显式列在** `SERDE_JSON_SOURCES`。两张表的权威都在
//! `tests/minimal_agent.rs`——**项数以表为准，这里不复述个数**（拆文件不是"让这些 API 随处可用"）。
//!
//! **公开面原样留在 `crate::store::` 下**（父模块 `pub use format::*`），消费方一处未改。
//!
//! ## 校验和的射程：它证明什么、不证明什么
//!
//! **证明**：那一行的字节与写下去时**逐字节相同**（手改与位翻都抓得住——只在正文里换一个汉字
//! 也算）；而且每一条都与**它前面的全部历史**绑在一起（改旧的一条 ⇒ 后面那条对不上）。
//!
//! **不证明**：① **它没有密钥**（CRC-32 不含秘密）⇒ 这是**一致性**读数，不是"没有人改过"的
//! 证明：能整段重算的人改完再补一遍链，它看不出来；② **旧记录**（加校验和之前写下的）自己
//! 没有校验和，读得出来、不校验、只进链——保护来自**它后面**那条带校验和的行，**尾部全是
//! 旧记录**的账本没有这一手；③ **活动段**那条定向读**不折链**（只解码它自己返回的行），要
//! "整份自洽"就走全量读或封口段那条路（见 `store/environment.rs` 的 `decode_trace_line`）。
//!
//! **旧记录读不读得回来**：读得回来——加校验和之前写下的旧账本**照常打开、还能继续往下写**；
//! 旧环境**不迁移、不覆盖、不补写**，新记录从旧记录的链值续算（真家升级走的就是这条路）。

use super::*;

/// 经历存储上的一条记录：稳定序号 + 归属活动 + 内容。
///
/// 序号是**次序的唯一依据**；归属活动说明这条经历属于哪一次 Run。两者都不因为读取而改变。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceRecord {
    pub(crate) seq: u64,
    pub(crate) run: RunId,
    /// **这一行是什么时候写的**（unix 毫秒）。
    ///
    /// `None` = 这一格没有被记——**旧记录**（加这一格之前写下的）与**当时没有钟**的环境都是这一档。
    /// 缺字段读作 `None`，**不编造**（与 `failure` / `cost` 同一条规矩）。
    pub(crate) at_millis: Option<u64>,
    pub(crate) entry: TracePayload,
}

impl TraceRecord {
    /// 这条经历在环境内的稳定序号。
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// 这条经历属于哪次活动。
    pub fn run(&self) -> &RunId {
        &self.run
    }

    /// **这条经历是什么时候写的**（unix 毫秒）；没有记 ⇒ `None`。
    pub fn at_millis(&self) -> Option<u64> {
        self.at_millis
    }

    /// 这条经历的内容。
    pub fn entry(&self) -> &TracePayload {
        &self.entry
    }
}

/// **唤醒缘由的封闭词表**：持久字段 `run-start.origin` 的取值。
///
/// 它答的是**「谁叫醒的」这一个问题**，取值镜像唤醒来源的种类（[`crate::wake::Woken`] 的那几档）。
///
/// ⚠ **2026-10-13：通道那一档分成两种**（原来只有 `Bell` 一个值，而它底下藏着两件不同的事）：
///
/// - [`WakeOrigin::Opened`]：**通道刚接上**（那条流的第一帧）——它覆盖**订阅间隙**：断着的那段
///   时间里可能发生过事情，而这一帧**不声称「刚刚有变化」**；
/// - [`WakeOrigin::Changed`]：**通道报了一次变化**（后续帧）——世界刚刚动了。
///
/// 分开的理由是**真话**：进主体视野的那句起源文案由这一格派生（`composition::origin_text`），
/// 而两件事共用一格时那句话只能二者取一——真机上它的代价是 **15 段"以为有事"的空醒**
/// （`agent/README.md` §3.74）。⚠ 分开**只让话变准，不改行为**：「连接建立即一次唤醒」照旧
/// （那是 fail-safe，收窄它会把断线窗口里的事件变成永远不响）。
///
/// ⚠ [`WakeOrigin::Bell`] **照读不写**（与 `SubjectEnding::Held` 同款）：旧账里那一格长得就是
/// `bell`，**读得出来**；线缆上再也产不出它，而旧记录**不替它认**是哪一种。
///
/// **为什么是词表而不是人话**：这一格是**持久字段**，而人话那句是**呈现**。同一事实两种表示
/// 会漂移（改一句文案就改变历史行的含义），而"为什么醒"的消费者（蒸馏端）不该去匹配自由文本。
/// 人话那句因此退为**派生视图**：由本值现查现派生（`composition::origin_text`），存储不用它。
///
/// 它与运行轨迹里 `injection`（stage=`run-start`）的 `activity-origin` 块**不是同一件东西**：
/// 那条是同一次开场注入的渲染正文（与状态栏、纸条板等块混在一批里），这一格是结构化登记。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeOrigin {
    /// **通道刚接上**（首帧；覆盖订阅间隙）。
    Opened,
    /// **通道报了一次变化**（后续帧）。
    Changed,
    /// 旧账里的 `bell`（两态分开之前）：**照读不写**。
    Bell,
    /// 本地钟源：自己约的那个时刻到点了（含离线错过之后的补响）。
    Timer,
    /// 宿主手动发起、没有外部事件触发。
    Manual,
}

impl WakeOrigin {
    /// 存进盘里的稳定字符串。**这几个字就是契约**：改了它等于改历史行的含义。
    ///
    /// ⚠ [`WakeOrigin::Bell`] 那一支**只为读旧账而存在**：新记录一个都写不出它
    /// （宿主按 [`crate::wake::Woken`] 给的是 [`WakeOrigin::Opened`] / [`WakeOrigin::Changed`]）。
    pub fn as_str(self) -> &'static str {
        match self {
            WakeOrigin::Opened => "wake-opened",
            WakeOrigin::Changed => "wake-changed",
            WakeOrigin::Bell => "bell",
            WakeOrigin::Timer => "timer",
            WakeOrigin::Manual => "manual",
        }
    }

    /// 从盘上的稳定字符串还原。**认不出的取值返回 `None`**——调用方据此拒绝整行（fail-closed），
    /// **绝不**静默降级成"没有起源"：那是把"外部改过的行"伪装成一条合法旧记录。
    ///
    /// 名字按设计固定，与 [`crate::core::ModelFailureClass::from_str`] 同一条理由：它**不**实现
    /// `std::str::FromStr`——那个 trait 要求一个 `Err` 类型，会把"认不出"变成一个错误**值**；
    /// 这里只有"认得"与"认不得"两种，多出来的那个类型没有消费者。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Option<Self> {
        match text {
            "wake-opened" => Some(WakeOrigin::Opened),
            "wake-changed" => Some(WakeOrigin::Changed),
            "bell" => Some(WakeOrigin::Bell),
            "timer" => Some(WakeOrigin::Timer),
            "manual" => Some(WakeOrigin::Manual),
            _ => None,
        }
    }
}

/// 一条经历的内容。它是**经历正文**，不是操作事实。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TracePayload {
    /// 这次活动已经开始。
    ///
    /// `origin` 是**唤醒缘由**：这次活动是谁叫醒的。它是**这条登记自己的字段**，与运行轨迹里
    /// 那条 `injection`（stage=`run-start`）里的 `activity-origin` 块**不是同一件东西**：那条是
    /// 同一次开场注入的渲染正文（与状态栏、纸条板等块混在一批里），这里是把「谁叫醒的」
    /// **单独**登记在活动开始这一刻——因此它可独立查询，不必去解析一整批注入块。
    ///
    /// **取值是词表值，不是人话**（[`WakeOrigin`]）：持久字段存的是稳定字符串，人话那句是
    /// **派生视图**（渲染点现查现派生，不存储）。同一事实两种表示迟早会漂移，而蒸馏端不该
    /// 去匹配自由文本。
    ///
    /// 缺字段 ＝ 没有被记录（旧记录就是这一形态，那时起源还不在这里）⇒ 读出来是 `None`，
    /// **不编造**（与 `Ending.failure` 同一条规矩）。取值不在词表里**不读作 `None`**：
    /// 那一行整行读不出来（fail-closed，见 [`WakeOrigin::from_str`]）。
    RunStarted { origin: Option<WakeOrigin> },
    /// 这次活动的初始输入（调用方明确提供的那份内容）。
    RunInput { text: String },
    /// 一次外层注入。
    Injection {
        stage: String,
        turn: Option<u32>,
        blocks: Vec<(String, String)>,
    },
    /// Agent 选择了一次行动，执行之前的确认点。
    ActionSelected {
        turn: u32,
        /// 这一轮里的第几个动作（**第 0 个不写字段**）。
        index: u16,
        action: String,
        params: Vec<(String, String)>,
    },
    /// 一次行动的结果。
    Observed {
        turn: u32,
        /// 这一轮里的第几个动作（**第 0 个不写字段**）。
        index: u16,
        action: String,
        params: Vec<(String, String)>,
        state: ResultState,
        failure: Option<String>,
        blocks: Vec<(String, String)>,
    },
    /// 一次模型响应：正文、单个决定与必要的协议续接数据。
    ///
    /// 它是**语义明确的新顶层种类**，不是借 `injection` 伪装出来的模型响应：`kind` 就是
    /// `model-response`，决定种类由封闭词汇表 [`KNOWN_DECISIONS`] 校验。
    ///
    /// **协议续接载荷保存但不展示**：`protocol` / `protocol-version` / `payload` 三个字段一起
    /// 存在（见 [`TracePayload::text`]：它只渲染正文，不渲染这三者），Adapter 用它重建下一次
    /// 请求。载荷在这里**不是**凭据，也**不**替代操作身份。
    ModelResponse {
        turn: u32,
        /// 模型正文。可以为空（不带正文的行动或结束）。
        content: String,
        /// 决定种类：`act` / `finish` / `yield`。取值在封闭词汇表里，见 [`KNOWN_DECISIONS`]。
        decision: String,
        /// 决定是 `act` 时的**第一个**行动标识（第 0 个请求）。
        action: Option<String>,
        /// 决定是 `act` 时第一个请求的参数（有序）。
        params: Vec<(String, String)>,
        /// 决定是 `act` 时**其余**请求，按模型给出的顺序；没有更多请求时**不写这个字段**。
        ///
        /// 与 `action` / `params` 合起来才是这条回复完整的意思：一轮里模型可以请求多个动作，
        /// 它们按这里的顺序依次执行。**顺序是身份的一部分**（第几个就是 `OperationId` 的序号），
        /// 因此它是有序列表，不是集合。
        extra: ExtraActions,
        /// 决定是 `finish` 时的主体收场取值（取值同样在封闭词汇表里）。
        ending: Option<String>,
        /// 协议续接数据：所属协议、版本、载荷。三者要么一起在，要么一起不在。
        protocol: Option<(String, u32, String)>,
    },
    /// 这次活动如何收场。
    ///
    /// `failure` 是**模型失败的分类稳定名**（[`crate::core::ModelFailureClass::as_str`]），
    /// 只在"模型失败、且当时带着分类"时写；其余收场与旧记录都不写这个字段。
    ///
    /// 它与事实侧 `run-ended` 的 `failure` **同模式、同口径**：缺字段 ＝ 分类没有被记录
    /// （旧记录就是这一形态）⇒ 读出来是 `None`，**不编造**。
    ///
    /// 为什么要在这里也存一份：`system` 只给到"模型失败"这一层（见 [`system_str`]），
    /// 而 `SystemEnding::ModelFailed` 是**唯一带载荷的收场值**——分类是那次失败本身的一部分，
    /// 经历侧把它丢掉，会让主体读自己的经历时**永远说不出是哪一类失败**。
    Ending {
        subject: Option<String>,
        system: Option<String>,
        failure: Option<String>,
    },
    /// 这次活动已经封存（结束事实已确认之后的独立一步）。
    Sealed,
}

impl TracePayload {
    /// 稳定种类标记（存进文件、给外层看）。
    pub fn kind(&self) -> &'static str {
        match self {
            TracePayload::RunStarted { .. } => "run-start",
            TracePayload::RunInput { .. } => "run-input",
            TracePayload::Injection { .. } => "injection",
            TracePayload::ActionSelected { .. } => "action-selected",
            TracePayload::Observed { .. } => "observed",
            TracePayload::ModelResponse { .. } => "model-response",
            TracePayload::Ending { .. } => "ending",
            TracePayload::Sealed => "sealed",
        }
    }

    /// 这条经历是不是"这次活动已经开始"的登记。
    pub fn is_started(&self) -> bool {
        matches!(self, TracePayload::RunStarted { .. })
    }

    /// 这条经历是不是"这次活动的结束事实"。
    pub fn is_ending(&self) -> bool {
        matches!(self, TracePayload::Ending { .. })
    }

    /// 这条经历是不是"这次活动已经封存"。
    pub fn is_sealed(&self) -> bool {
        matches!(self, TracePayload::Sealed)
    }

    /// 这条经历可读的文本内容（没有正文时为空）。
    ///
    /// 它只做**机械展开**：把已经存在的块按原样拼出来，不摘要、不重排、不解释。
    ///
    /// **模型响应只渲染正文**：协议续接数据不进入这里，因此它也不会经由普通历史文件、字节
    /// 预算或任何"读经历正文"的调用方被展示出来。这是**单点收敛**——所有把经历变成正文的地方
    /// 都走这一条路，所以不必在每一处投影上各加一条过滤。
    pub fn text(&self) -> String {
        match self {
            // 起源**不进这里**：它是结构化字段（给"为什么醒"那条查询用），不是经历正文的渲染；
            // 正文那一行逐字节照旧，避免波及字节预算与既有渲染判据。
            TracePayload::RunStarted { .. } => "run started".to_string(),
            TracePayload::RunInput { text } => text.clone(),
            TracePayload::Injection {
                stage,
                turn,
                blocks,
            } => {
                let mut out = match turn {
                    Some(turn) => format!("injection[{stage}] turn {turn}"),
                    None => format!("injection[{stage}]"),
                };
                for (label, text) in blocks {
                    out.push('\n');
                    out.push_str(label);
                    out.push_str(": ");
                    out.push_str(text);
                }
                out
            }
            TracePayload::ActionSelected {
                turn,
                action,
                params,
                ..
            } => {
                let mut out = format!("turn {turn} selected {action}");
                for (name, value) in params {
                    out.push('\n');
                    out.push_str(name);
                    out.push_str(" = ");
                    out.push_str(value);
                }
                out
            }
            TracePayload::Observed {
                turn,
                action,
                state,
                failure,
                blocks,
                ..
            } => {
                let mut out = format!("turn {turn} {action} -> {}", state_str(*state));
                if let Some(class) = failure {
                    out.push_str(" (");
                    out.push_str(class);
                    out.push(')');
                }
                for (label, text) in blocks {
                    out.push('\n');
                    out.push_str(label);
                    out.push_str(": ");
                    out.push_str(text);
                }
                out
            }
            TracePayload::Ending {
                subject,
                system,
                failure,
            } => match (subject, system) {
                (Some(ending), None) => format!("subject ending: {ending}"),
                (None, Some(ending)) => match failure {
                    // 分类与分类名一起显示：`system ending: model-failed (transport-failure)`。
                    Some(class) => format!("system ending: {ending} ({class})"),
                    None => format!("system ending: {ending}"),
                },
                _ => "ending (malformed)".to_string(),
            },
            TracePayload::ModelResponse {
                content, decision, ..
            } => {
                // 正文就是模型正文本身，**不**在这里重复渲染决定（决定由紧随其后的记录体现）。
                // 没有正文时给一条机械说明，免得历史里出现一行看起来像空白的记录。
                if content.is_empty() {
                    format!("(no body; decision {decision})")
                } else {
                    content.clone()
                }
            }
            TracePayload::Sealed => "sealed".to_string(),
        }
    }
}

/// 一个 Run 的历史摘要：一次查阅能拿到的最小事实。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunSummary {
    pub(crate) run: RunId,
    pub(crate) records: usize,
    pub(crate) first_seq: u64,
    pub(crate) last_seq: u64,
    pub(crate) ending: Option<String>,
    /// 模型失败的分类稳定名，与事实侧 `run-ended` 的 `failure` **同口径**。
    ///
    /// 缺 ＝ 当时就没带分类（主体收场、其余系统收场、旧记录都是这一形态），
    /// **不是**"没有失败"。它与 `ending` 分开的理由：`ending` 是"哪一类收场"的稳定名，
    /// 读它的人不需要更细；分类是复盘用的读数。
    pub(crate) failure: Option<String>,
    pub(crate) sealed: bool,
    pub(crate) started: bool,
}

impl RunSummary {
    /// 活动身份。
    pub fn run(&self) -> &RunId {
        &self.run
    }

    /// 归在这个活动名下的经历条数。
    pub fn records(&self) -> usize {
        self.records
    }

    /// 这个活动名下最早的序号。
    pub fn first_seq(&self) -> u64 {
        self.first_seq
    }

    /// 这个活动名下最晚的序号。
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    /// 结束事实的机械描述；没有结束事实时为 `None`（这次活动**没有**可靠结束事实）。
    pub fn ending(&self) -> Option<&str> {
        self.ending.as_deref()
    }

    /// 模型失败的分类稳定名；主体收场、其余系统收场与旧记录都是 `None`。
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// 结束事实是否已经封存。
    pub fn is_sealed(&self) -> bool {
        self.sealed
    }

    /// 是否有"活动已经开始"的记录。
    pub fn has_start(&self) -> bool {
        self.started
    }

    /// 这段历史是否完整收尾：已经开始、有结束事实、并且已经封存。
    pub fn is_complete(&self) -> bool {
        self.started && self.ending.is_some() && self.sealed
    }

    /// 状态描述：把三件不同的事分开说，不压成一个布尔值。
    pub fn status(&self) -> &'static str {
        if !self.started {
            "no-start"
        } else if self.ending.is_none() {
            "interrupted"
        } else if !self.sealed {
            "ended-unsealed"
        } else {
            "complete"
        }
    }
}

/// 一次分页读取的结果。**明确**告诉调用者是否还有后续内容，不静默截断冒充完整。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TracePage {
    pub(crate) records: Vec<TraceRecord>,
    pub(crate) has_more: bool,
}

impl TracePage {
    /// 这一页的记录，按稳定序号。
    pub fn records(&self) -> &[TraceRecord] {
        &self.records
    }

    /// 后面是否还有未返回的记录。
    pub fn has_more(&self) -> bool {
        self.has_more
    }
}

/// 一条操作事实的**只读行视图**：只报告"做过什么、结果是什么"，不重放、不重建状态。
///
/// 它本身不重建 Foundation 的 `Record`：重建走 [`records_of_run`]，那一步必须对照写入端的
/// 真实编码逐字段校验，不能靠字段名猜协议。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FactRow {
    seq: u64,
    run: String,
    fact: String,
    agent: Option<String>,
    turn: Option<u32>,
    /// 同一轮里的第几个动作（写入端**第 0 个不写这个字段**）。
    ///
    /// 原样保存、留到 `record_of_row` 里校验范围：与 `turn` 不同，"序号超范围"是**这一行的
    /// 取值非法**（`illegal-index`），而不是"整行读不出来"。
    index: Option<u64>,
    state: Option<String>,
    outcome: Option<String>,
    /// 模型失败的**分类**（只有 `run-ended` 且结局是模型失败、而且当时带着分类时才有）。
    ///
    /// 它与 `index` 同族：写入端"没有就不写这个字段"，因此**旧记录照常读**（缺字段 = `None` =
    /// 分类没有被记录，**不编造**）；新构建读到不认识的取值一律 fail-closed。
    failure: Option<String>,
    /// **代价**：收场那一行上的时长与用量（只有 `run-ended` 有）。
    ///
    /// 与 `failure` 同族：写入端"没有就不写字段"，因此**旧记录照常读**（缺字段 = 这一格
    /// 没有被记，**不编造**）。用量里逐个字段独立可选（厂商可能只报其中几个）。
    cost: RunCost,
    /// **这一行是什么时候写的**（unix 毫秒）。
    ///
    /// ⚠ 它是**通用**字段（任何种类都允许带，像 `seq` / `run` 一样），因此**不进**
    /// `fact_row_is_interpretable` 的逐种类检查。`None` = 这一格没有被记
    /// （旧记录、或当时没有钟），**不编造**。
    at_millis: Option<u64>,
}

impl FactRow {
    /// 这条事实在事实存储里的稳定序号。
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// 这条事实属于哪次活动。
    pub fn run(&self) -> &str {
        &self.run
    }

    /// 事实种类标记：`run-started` / `action-prepared` / `action-dispatched` /
    /// `action-result` / `run-ended`。
    pub fn fact(&self) -> &str {
        &self.fact
    }

    /// 行里保存的 Agent 字段（只有 `run-started` 会写）。
    ///
    /// 它**不是**身份权威：权威身份来自已打开并校验归属的环境（[`Environment::agent`]）。
    /// 这里的值只用于一致性核对——与环境身份不一致的行是"本写入者写不出的形状"。
    pub fn agent(&self) -> Option<&str> {
        self.agent.as_deref()
    }

    /// 这条事实涉及的 Turn（`run-started` 与 `run-ended` 没有）。
    pub fn turn(&self) -> Option<u32> {
        self.turn
    }

    /// 行里保存的**同一轮内子序号**（只有行动事实有，而且**第 0 个不写字段**）。
    ///
    /// `None` = 这一行是"这一轮的第 0 个动作"，与旧环境读出来的含义**完全相同**。
    pub fn index(&self) -> Option<u64> {
        self.index
    }

    /// 行动结果的三分状态（只有 `action-result` 有）。
    pub fn state(&self) -> Option<&str> {
        self.state.as_deref()
    }

    /// 收场结果（只有 `run-ended` 有）。
    pub fn outcome(&self) -> Option<&str> {
        self.outcome.as_deref()
    }

    /// 模型失败的分类标记（只有带分类的 `run-ended` 有；缺字段 = 没有被记录）。
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// **代价**（只有 `run-ended` 有；缺字段 = 这一格没有被记，不是"等于零"）。
    pub fn cost(&self) -> RunCost {
        self.cost
    }

    /// **这一行是什么时候写的**（unix 毫秒）；没有记 ⇒ `None`。
    pub fn at_millis(&self) -> Option<u64> {
        self.at_millis
    }
}

/// 一条事实行**不能**重建为 Foundation 事实的原因。
///
/// 它只服务**诊断读取**这条路径：打开环境时已经接受的记录，仍可能缺字段、取值非法或字段
/// 组合无法解释。那时必须明确报错，而不是替它补一个默认 Turn、默认成功或默认收场，
/// 也不能把那一行悄悄丢掉。
///
/// **哪些分支在经 `open` 打开的环境里够不到**：`missing-outcome`、`illegal-outcome` 与
/// `unknown-kind` 三类，打开校验已经先一步拒绝（报 `MalformedEntry`）。它们仍然被显式
/// 写出来，是因为 `records_of_run` 是**公开的读侧转换**，不能靠"上游一定筛过"来保证行为；
/// 而"明确拒绝"与"补一个默认值"是两种不同的行为，该选哪一种不应取决于调用路径。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FactRowError {
    /// `run-started` 行没有保存身份字段。
    MissingAgent { seq: u64 },
    /// 行里保存的身份与已校验的环境身份不一致。
    ForeignAgent { seq: u64 },
    /// 行动事实没有带 Turn。
    MissingTurn { seq: u64 },
    /// Turn 序号是 `0`：那是"活动开始之前不存在的 Turn"，不可能出现在行动事实上。
    IllegalTurn { seq: u64, turn: u32 },
    /// `action-result` 行没有带三分状态。
    MissingState { seq: u64 },
    /// 三分状态取值不在封闭词汇表里。
    IllegalState { seq: u64 },
    /// `run-ended` 行没有带收场。
    MissingOutcome { seq: u64 },
    /// 收场取值无法解释成主体收场或系统收场之一。
    IllegalOutcome { seq: u64 },
    /// 模型失败的分类取值不在封闭词汇表里：这一行的结局说不清楚，**不猜**。
    ///
    /// 它与打开环境时的 `MalformedEntry` 是同一件事的两条路径：打开时校验整份前缀，
    /// 诊断读取时按行报错。
    IllegalFailure { seq: u64 },
    /// 这一行带了**本写入者不可能写出**的字段组合（多出的字段意味着文件被外部改过）。
    UnexpectedField { seq: u64 },
    /// 行动事实的子序号不是 `0..=65535` 里的整数。
    IllegalIndex { seq: u64 },
    /// 同一（事实种类 + 轮次）里的子序号没有按 `0`、`1`、`2`… 依次出现。
    ///
    /// 一轮里第几个动作**是身份的一部分**（`OperationId` 带着它），跳号或重复意味着盘上的
    /// 东西不再是本写入者写出的形状——那是事实与身份对不上，不是"读不出来"，但不许猜。
    ActionIndexOutOfOrder { seq: u64 },
    /// 事实种类名不认识。
    UnknownKind { seq: u64 },
}

impl FactRowError {
    /// 稳定分类标记。
    pub fn as_str(&self) -> &'static str {
        match self {
            FactRowError::MissingAgent { .. } => "missing-agent",
            FactRowError::ForeignAgent { .. } => "foreign-agent",
            FactRowError::MissingTurn { .. } => "missing-turn",
            FactRowError::IllegalTurn { .. } => "illegal-turn",
            FactRowError::MissingState { .. } => "missing-state",
            FactRowError::IllegalState { .. } => "illegal-state",
            FactRowError::MissingOutcome { .. } => "missing-outcome",
            FactRowError::IllegalOutcome { .. } => "illegal-outcome",
            FactRowError::IllegalFailure { .. } => "illegal-failure",
            FactRowError::UnexpectedField { .. } => "unexpected-field",
            FactRowError::IllegalIndex { .. } => "illegal-index",
            FactRowError::ActionIndexOutOfOrder { .. } => "action-index-out-of-order",
            FactRowError::UnknownKind { .. } => "unknown-kind",
        }
    }

    /// 出错的那一行在事实存储里的稳定序号（用来定位，不带任何正文）。
    pub fn seq(&self) -> u64 {
        match self {
            FactRowError::MissingAgent { seq }
            | FactRowError::ForeignAgent { seq }
            | FactRowError::MissingTurn { seq }
            | FactRowError::IllegalTurn { seq, .. }
            | FactRowError::MissingState { seq }
            | FactRowError::IllegalState { seq }
            | FactRowError::MissingOutcome { seq }
            | FactRowError::IllegalOutcome { seq }
            | FactRowError::IllegalFailure { seq }
            | FactRowError::UnexpectedField { seq }
            | FactRowError::IllegalIndex { seq }
            | FactRowError::ActionIndexOutOfOrder { seq }
            | FactRowError::UnknownKind { seq } => *seq,
        }
    }
}

/// 把一个活动的**全部**事实行（按稳定序号顺序）重建成 Foundation 的事实序列。
///
/// 它是**读侧转换**，因此只做解释、不做修复：`open` 已经接受的记录仍可能缺字段、取值非法或
/// 字段组合无法解释，这时一律明确拒绝——不填默认值、不静默丢行。
///
/// 转换只对照写入端的真实编码（`encode_fact`）：每一种事实必须**正好**带齐它写出的字段，
/// 多一个少一个都是"本写入者写不出的形状"。`run-started` 行里保存的身份只用于与已校验的
/// 环境身份核对；**身份权威仍然来自环境**，不从每一行解码。
///
/// `rows` 必须只含**同一个活动**的行并按稳定序号排列：本函数不分组、不按 Turn 重排、
/// 不去重。
pub fn records_of_run(rows: &[FactRow], owner: &AgentId) -> Result<Vec<Record>, FactRowError> {
    let mut records = Vec::with_capacity(rows.len());
    // 同一（事实种类 + 轮次）里的子序号必须**严格递增**。
    //
    // 序号是"这一轮里的第几个请求"。**跳号是合法的**：被准入拒绝的请求不产生任何事实，
    // 因此完全可能出现"只有第 1 个有事实、第 0 个什么都没留下"的一轮。两类形状不合法：
    // 序号**重复**（两个不同请求塌成同一个身份）与序号**倒退**（顺序不再是写入端的顺序）。
    let mut last: BTreeMap<(&str, u32), u16> = BTreeMap::new();
    for row in rows {
        if let (Some(turn), true) = (row.turn(), is_action_fact(row.fact())) {
            let index = row_index(row)?;
            if let Some(previous) = last.get(&(row.fact(), turn))
                && index <= *previous
            {
                return Err(FactRowError::ActionIndexOutOfOrder { seq: row.seq() });
            }
            last.insert((row.fact(), turn), index);
        }
        records.push(record_of_row(row, owner)?);
    }
    Ok(records)
}

/// 这条事实是不是"一次动作"的三种记法之一。
pub(crate) fn is_action_fact(fact: &str) -> bool {
    matches!(
        fact,
        "action-prepared" | "action-dispatched" | "action-result"
    )
}

/// 行动事实的**同一轮内子序号**：字段缺省就是 `0`（旧环境里**每一行**都是这样）。
fn row_index(row: &FactRow) -> Result<u16, FactRowError> {
    match row.index() {
        None => Ok(0),
        Some(raw) => u16::try_from(raw).map_err(|_| FactRowError::IllegalIndex { seq: row.seq() }),
    }
}

/// 一条事实行 → 一条 Foundation 事实。缺字段、错取值、错组合都在这里被拒绝。
fn record_of_row(row: &FactRow, owner: &AgentId) -> Result<Record, FactRowError> {
    let seq = row.seq();
    let run = RunId::new(row.run());
    match row.fact() {
        "run-started" => {
            let agent = row.agent().ok_or(FactRowError::MissingAgent { seq })?;
            if agent != owner.as_str() {
                return Err(FactRowError::ForeignAgent { seq });
            }
            // 写入端只写 seq / run / fact / agent；多出行动、收场或子序号字段就是被外部改过。
            if row.turn().is_some()
                || row.index().is_some()
                || row.state().is_some()
                || row.outcome().is_some()
            {
                return Err(FactRowError::UnexpectedField { seq });
            }
            Ok(Record::RunStarted {
                agent: owner.clone(),
                run,
            })
        }
        "action-prepared" => {
            let turn = action_turn(row)?;
            let index = row_index(row)?;
            if row.state().is_some() || row.outcome().is_some() || row.agent().is_some() {
                return Err(FactRowError::UnexpectedField { seq });
            }
            Ok(Record::ActionPrepared {
                operation: OperationId::at(run.clone(), turn, index),
                run,
            })
        }
        "action-dispatched" => {
            let turn = action_turn(row)?;
            let index = row_index(row)?;
            if row.state().is_some() || row.outcome().is_some() || row.agent().is_some() {
                return Err(FactRowError::UnexpectedField { seq });
            }
            Ok(Record::ActionDispatched {
                operation: OperationId::at(run.clone(), turn, index),
                run,
            })
        }
        "action-result" => {
            let turn = action_turn(row)?;
            let index = row_index(row)?;
            let state_text = row.state().ok_or(FactRowError::MissingState { seq })?;
            let state = decode_state(state_text).ok_or(FactRowError::IllegalState { seq })?;
            if row.outcome().is_some() || row.agent().is_some() {
                return Err(FactRowError::UnexpectedField { seq });
            }
            let operation = OperationId::at(run.clone(), turn, index);
            Ok(Record::ActionResultRecorded {
                action: ActionResult::new(operation, state),
                run,
            })
        }
        "run-ended" => {
            let text = row.outcome().ok_or(FactRowError::MissingOutcome { seq })?;
            let outcome = parse_stored_outcome(text).ok_or(FactRowError::IllegalOutcome { seq })?;
            if row.turn().is_some()
                || row.index().is_some()
                || row.state().is_some()
                || row.agent().is_some()
            {
                return Err(FactRowError::UnexpectedField { seq });
            }
            // 分类**只**能挂在模型失败上，而且必须能解释：认不出的取值不猜（`illegal-failure`）。
            // 缺字段 = 分类没有被记录（旧记录就是这一形态）⇒ `None`，**不编造**。
            let outcome = match (outcome, row.failure()) {
                (Outcome::System(SystemEnding::ModelFailed(_)), None) => {
                    Outcome::System(SystemEnding::ModelFailed(None))
                }
                (Outcome::System(SystemEnding::ModelFailed(_)), Some(class)) => {
                    // 盘上那根字符串在这里**过封闭表**：认不出 ⇒ 整行拒绝（`illegal-failure`），
                    // 认得出 ⇒ 取**词表的规范形态**（`as_str()`），Foundation 手里因此永远只有
                    // 一个词表之内的稳定名（见 `SystemEnding::ModelFailed` 的文档）。
                    let class = crate::core::ModelFailureClass::from_str(class)
                        .ok_or(FactRowError::IllegalFailure { seq })?;
                    Outcome::System(SystemEnding::ModelFailed(Some(class.as_str())))
                }
                (other, None) => other,
                (Outcome::Subject(_) | Outcome::System(_), Some(_)) => {
                    return Err(FactRowError::UnexpectedField { seq });
                }
            };
            Ok(Record::RunEnded {
                run,
                outcome,
                // 代价**原样搬运**：解码层只保证"在不在、是不是数"，本层不解释它。
                cost: row.cost(),
            })
        }
        _ => Err(FactRowError::UnknownKind { seq }),
    }
}

/// 从一条行动事实的 Turn 序号还原 `Turn`。
///
/// 读侧必须成立的是：行动事实要带 Turn，而且序号要是 `u32` 能表示的**正数**。`0` 是"活动
/// 开始之前不存在的 Turn"，表示范围之外的数字在**解码**处就被拒绝（见 `decode_fact_row`）。
///
/// 这里**不再**用"该活动事实行数 + 1"作上界。那条推导依赖"除最后一个 Turn 之外，每个 Turn
/// 至少留下一条行动事实"；而行动请求可以在写入任何事实**之前**被准入拒绝，于是一个 Turn
/// 完全可以零事实。用行数推上界会把这类合法历史误判成越界，所以改成直接用 Foundation 的
/// 数字构造入口——它是常数时间的，不需要任何上界来保证有界。
///
/// 构造本身只回答"这个数字能不能表示成一个 Turn"，**不**声称这次活动真的走到过这一轮。
fn action_turn(row: &FactRow) -> Result<Turn, FactRowError> {
    let seq = row.seq();
    let number = row.turn().ok_or(FactRowError::MissingTurn { seq })?;
    Turn::from_number(number).ok_or(FactRowError::IllegalTurn { seq, turn: number })
}

/// 把行里保存的收场文本解释成主体收场或系统收场之一，取值必须在封闭词汇表里。
fn parse_stored_outcome(text: &str) -> Option<Outcome> {
    let (kind, value) = text.split_once(':')?;
    match kind {
        "subject" => subject_of(value).map(Outcome::Subject),
        "system" => system_of(value).map(Outcome::System),
        _ => None,
    }
}

/// 主体收场的取值 → 枚举。取值不在封闭词汇表里就是"无法解释"，不猜。
///
/// ⚠ **2026-10-10**：`ended` 是**今天唯一会写下去的那一个**（主体收场不带值）；
/// 四个旧值是 2026-10-10 之前的历史，**照读不写**（账只追加，改一次主体就不是原来那个主体了）。
fn subject_of(text: &str) -> Option<SubjectEnding> {
    Some(match text {
        "ended" => SubjectEnding::Ended,
        "held" => SubjectEnding::Held,
        "completed" => SubjectEnding::Completed,
        "silent" => SubjectEnding::Silent,
        "declined" => SubjectEnding::Declined,
        "waiting" => SubjectEnding::Waiting,
        _ => return None,
    })
}

/// 系统收场的取值 → 枚举（同上）。
fn system_of(text: &str) -> Option<SystemEnding> {
    Some(match text {
        // 分类**不在这一格**：它由 `run-ended` 行的 `failure` 字段单独交代（见 `record_of_row`），
        // 这里先给出"分类未被记录"的形态，取值字符串与封闭词汇一个字都没变。
        "model-failed" => SystemEnding::ModelFailed(None),
        "action-failed" => SystemEnding::ActionFailed,
        "cancelled" => SystemEnding::Cancelled,
        "timeout" => SystemEnding::Timeout,
        "crashed" => SystemEnding::Crashed,
        "indeterminate" => SystemEnding::Indeterminate,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// 编码 / 解码：格式就是"一行一个 JSON 对象 + 换行"，依赖 serde_json
// ---------------------------------------------------------------------------

/// 一次回复里**其余**请求的形态：行动标识 + 有序参数。
///
/// 第 0 个请求沿用 `action` / `params`，其余按顺序放在这里——因此"一轮一个动作"的常规情形
/// 在盘上一个字段都不多。
pub type ExtraActions = Vec<(String, Vec<(String, String)>)>;

/// 事实行上的**同一轮内子序号**字段名。
pub(crate) const INDEX_FIELD: &str = "index";

/// `run-start` 经历行上的**唤醒缘由**字段名。
const ORIGIN_FIELD: &str = "origin";

/// **这一行是什么时候写的**（unix 毫秒）——**每一条记录都带它**（2026-10-06 立）。
///
/// 与上面那些**逐种类**的字段不同，它是**通用**字段（像 `seq` / `run` 一样）：任何一条记录都
/// 允许带它，因此它**不进** `fact_row_is_interpretable` 的逐种类检查。
///
/// **没有就不写字段**（与 `origin` / `failure` / `elapsed-ms` 同辙）：环境**没有钟**时一个字节
/// 都不写 ⇒ 旧记录照读、离线夹具的字节逐字节不变。缺字段 = 这一格没有被记，**不编造**。
pub(crate) const AT_FIELD: &str = "at-ms";

/// `run-ended` 事实行上的**代价**字段名：时长（毫秒）。
const ELAPSED_FIELD: &str = "elapsed-ms";
/// 用量的三个字段名。三条 wire 的原文名不同，落到盘上的是**同一组中立名**——
/// 否则同一个格位会因走哪条 wire 而有两种口径。
const USAGE_PROMPT_FIELD: &str = "prompt-tokens";
const USAGE_COMPLETION_FIELD: &str = "completion-tokens";
const USAGE_TOTAL_FIELD: &str = "total-tokens";

/// 写**代价**：时长与用量各自"没有就不写字段"。
///
/// 与 `write_index` / `failure` 同一条规矩：**缺字段 = 这一格没有被记**，不是"等于零"。
/// 旧记录（两格都没有）因此在盘上逐字节不变；用量里**某**个字段缺（厂商漏报）就只缺那一个，
/// 不补零——补零会说"用了 0"，那是假话。
fn write_cost(object: &mut serde_json::Map<String, serde_json::Value>, cost: &RunCost) {
    if let Some(millis) = cost.elapsed_millis {
        object.insert(ELAPSED_FIELD.to_string(), serde_json::Value::from(millis));
    }
    // 三个用量字段**一个都没有** ⇒ 整格不写（"这次没报"与"报了三个空"不该混成同一行）。
    if let Some(usage) = cost.usage
        && !usage.is_empty()
    {
        for (name, value) in [
            (USAGE_PROMPT_FIELD, usage.prompt_tokens),
            (USAGE_COMPLETION_FIELD, usage.completion_tokens),
            (USAGE_TOTAL_FIELD, usage.total_tokens),
        ] {
            if let Some(value) = value {
                object.insert(name.to_string(), serde_json::Value::from(value));
            }
        }
    }
}

/// 写子序号：**第 0 个不写这个字段**。
///
/// 这条规则是刻意的，而且与 `OperationId` 的语义一致：一轮一个动作这种常规情形在盘上
/// 与从前**逐字节相同**，旧环境读出来就是"每轮第 0 个动作"。
fn write_index(object: &mut serde_json::Map<String, serde_json::Value>, index: u16) {
    if index != 0 {
        object.insert(INDEX_FIELD.to_string(), serde_json::Value::from(index));
    }
}

/// 写「这一行是什么时候写的」：**没有钟就不写这个字段**。
///
/// 与 [`write_index`] 同辙：环境没有钟时（离线夹具、旧入口）那一格整个不出现，因此那些路径
/// 写出来的字节与从前**逐字节相同**；有钟时（生产装配）每一条记录都带它。
fn write_at(object: &mut serde_json::Map<String, serde_json::Value>, at: Option<u64>) {
    if let Some(at) = at {
        object.insert(AT_FIELD.to_string(), serde_json::Value::from(at));
    }
}

/// 写盘前把模型失败分类过一遍**封闭表**：认不出就**拒绝写**（[`StoreError::UnknownFailureClass`]）。
///
/// ## 为什么写侧也要查
///
/// 那条纪律是"**持久字段上的词表一律 fail-closed**"。**读侧早就做到了**——认不出的取值
/// 整行拒绝（`malformed-entry` / `illegal-failure`，两条用例各自钉着）。**写侧从前一个字都不查。**
///
/// 而 `SystemEnding::ModelFailed` 的载荷在拆掉 Foundation ↔ Core 那个环之后是一根**裸字符串**
/// （见 `SystemEnding` 的文档）：**类型不再保证它在词表之内**，而盘上那一行一旦落下去，
/// **谁也读不回来**——一个活动会永远停在"读不出来"。⇒ 写之前查，是这条纪律缺的另一半。
///
/// ## 为什么写在这里
///
/// `store` **认识 `core` 是下行**（存储格式在 Core 之下，封闭表就在 Core 里），合法；
/// 而这里是**唯一**的写入口——`encode_trace` 与 `encode_fact` 是通向盘上的最后一道。
///
/// 返回的是**词表的规范形态**（`as_str()`），不是传进来的那根字符串：即便将来有人从别处
/// 拿到一个"看着一样"的字符串，落盘的也永远是词表自己的那一份。
fn checked_failure_class(class: &str) -> Result<&'static str, StoreError> {
    crate::core::ModelFailureClass::from_str(class)
        .map(|class| class.as_str())
        .ok_or(StoreError::UnknownFailureClass)
}

/// 把一条经历编码成一行。
///
/// 返回 `Result` 只为一件新事：**模型失败分类要过封闭表**（见 [`checked_failure_class`]）。
/// 其余字段照旧不会失败。
///
/// ⚠ **2026-10-06 起带校验和**：返回"整行 ＋ 新的链值"。正文（制表符之前那一段）与加校验和
/// 之前**逐字节相同**，只是行尾多了 `\t<8 位十六进制>`——见 [`line_sum`] 与 [`chain_checksum`]。
pub(crate) fn encode_trace(
    seq: u64,
    run: &RunId,
    payload: &TracePayload,
    previous: u32,
    at: Option<u64>,
) -> Result<(String, u32), StoreError> {
    let mut object = serde_json::Map::new();
    object.insert("seq".to_string(), serde_json::Value::from(seq));
    object.insert("run".to_string(), serde_json::Value::from(run.as_str()));
    write_at(&mut object, at);
    object.insert("kind".to_string(), serde_json::Value::from(payload.kind()));

    match payload {
        // 起源：**没有就不写字段**（与 `write_index` 同辙）——旧形状（无起源）在盘上逐字节不变。
        // 写下去的是**词表值**（`WakeOrigin::as_str`），人话那句留在渲染点现查现派生。
        TracePayload::RunStarted { origin } => {
            if let Some(origin) = origin {
                object.insert(
                    ORIGIN_FIELD.to_string(),
                    serde_json::Value::from(origin.as_str()),
                );
            }
        }
        TracePayload::RunInput { text } => {
            object.insert("text".to_string(), serde_json::Value::from(text.as_str()));
        }
        TracePayload::Injection {
            stage,
            turn,
            blocks,
        } => {
            object.insert("stage".to_string(), serde_json::Value::from(stage.as_str()));
            if let Some(turn) = turn {
                object.insert("turn".to_string(), serde_json::Value::from(*turn));
            }
            object.insert("blocks".to_string(), blocks_value(blocks));
        }
        TracePayload::ActionSelected {
            turn,
            index,
            action,
            params,
        } => {
            object.insert("turn".to_string(), serde_json::Value::from(*turn));
            write_index(&mut object, *index);
            object.insert(
                "action".to_string(),
                serde_json::Value::from(action.as_str()),
            );
            object.insert("params".to_string(), pairs_value(params));
        }
        TracePayload::Observed {
            turn,
            index,
            action,
            params,
            state,
            failure,
            blocks,
        } => {
            object.insert("turn".to_string(), serde_json::Value::from(*turn));
            write_index(&mut object, *index);
            object.insert(
                "action".to_string(),
                serde_json::Value::from(action.as_str()),
            );
            object.insert("params".to_string(), pairs_value(params));
            object.insert(
                "state".to_string(),
                serde_json::Value::from(state_str(*state)),
            );
            if let Some(class) = failure {
                object.insert(
                    "failure".to_string(),
                    serde_json::Value::from(class.as_str()),
                );
            }
            object.insert("blocks".to_string(), blocks_value(blocks));
        }
        TracePayload::ModelResponse {
            turn,
            content,
            decision,
            action,
            params,
            extra,
            ending,
            protocol,
        } => {
            object.insert("turn".to_string(), serde_json::Value::from(*turn));
            object.insert(
                "content".to_string(),
                serde_json::Value::from(content.as_str()),
            );
            object.insert(
                "decision".to_string(),
                serde_json::Value::from(decision.as_str()),
            );
            if let Some(action) = action {
                object.insert(
                    "action".to_string(),
                    serde_json::Value::from(action.as_str()),
                );
            }
            if !params.is_empty() {
                object.insert("params".to_string(), pairs_value(params));
            }
            // 只有**还有更多请求**时才写这个字段：一轮一个动作的常规情形与从前逐字节相同。
            if !extra.is_empty() {
                object.insert("extra-actions".to_string(), extra_value(extra));
            }
            if let Some(ending) = ending {
                object.insert(
                    "ending".to_string(),
                    serde_json::Value::from(ending.as_str()),
                );
            }
            // 三个字段一起写：半个续接载荷不是"缺失"，是损坏。
            if let Some((protocol, version, payload)) = protocol {
                object.insert(
                    "protocol".to_string(),
                    serde_json::Value::from(protocol.as_str()),
                );
                object.insert(
                    "protocol-version".to_string(),
                    serde_json::Value::from(*version),
                );
                object.insert(
                    "payload".to_string(),
                    serde_json::Value::from(payload.as_str()),
                );
            }
        }
        TracePayload::Ending {
            subject,
            system,
            failure,
        } => {
            if let Some(ending) = subject {
                object.insert(
                    "subject".to_string(),
                    serde_json::Value::from(ending.as_str()),
                );
            }
            if let Some(ending) = system {
                object.insert(
                    "system".to_string(),
                    serde_json::Value::from(ending.as_str()),
                );
            }
            // 分类**只**在带分类的模型失败时写；其余收场与旧记录都不写这个字段
            // （与事实侧 `run-ended` 逐字同一条规矩）。写之前**过封闭表**。
            if let Some(class) = failure {
                object.insert(
                    "failure".to_string(),
                    serde_json::Value::from(checked_failure_class(class)?),
                );
            }
        }
        TracePayload::Sealed => {}
    }

    Ok(signed_line(
        serde_json::Value::Object(object).to_string(),
        previous,
    ))
}

// ---------------------------------------------------------------------------
// 记录校验和：CRC-32 链
// ---------------------------------------------------------------------------

/// 空账本的链初值。
///
/// 链的定义是 `chain_i = crc32(chain_{i-1}, body_i)`——**把上一条的结果当作这一条的初值**。
/// 于是每一条都与它前面的**全部历史**绑在一起：改一条旧记录，后面那条的校验和就对不上。
/// 同族做法：SQLite WAL 的累积校验和（每帧把上一帧的结果算进去）、Certificate Transparency
/// 的签名树头（拿旧树头验增量的一致性证明）。
///
/// **为什么要有它**（2026-10-06，长跑读数那一刀）：在那之前，一条记录的"完整"只靠
/// "JSON 解析得出来 ＋ 词表解释得了"——**一个文本字段里翻一个 bit，完全看不出来**。
/// 加上链之后它是**可判定的**：那串字节与它写下去时的样子对不上，就明确失败。
pub(crate) const CHAIN_SEED: u32 = 0;

/// CRC-32 的表（IEEE 802.3 反射多项式 `0xEDB88320`），**编译期**算出来：无依赖、无运行期初始化。
const fn crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0usize;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 {
                0xEDB8_8320 ^ (value >> 1)
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

static CRC32_TABLE: [u32; 256] = crc32_table();

/// 把 `body` 续到链上（`previous` 是上一条的结果；空账本用 [`CHAIN_SEED`]）。
///
/// 不做首尾取反：取反只影响常数项，而这里要的是**链**——同一个初值必须逐字节可复现，
/// 因为校验发生在**另一次运行**里。
pub(crate) fn chain_checksum(previous: u32, body: &[u8]) -> u32 {
    let mut crc = previous;
    for byte in body {
        crc = CRC32_TABLE[((crc ^ u32::from(*byte)) & 0xff) as usize] ^ (crc >> 8);
    }
    crc
}

/// 一行的成品：正文 ＋ 行尾校验和 ＋ 换行。
///
/// 校验和算在**正文**上（制表符之前那一段），正文本身逐字节不变——因此它校验的是
/// **盘上真正的那串字节**，读回来时不需要任何规范化重排。
fn signed_line(body: String, previous: u32) -> (String, u32) {
    let chain = chain_checksum(previous, body.as_bytes());
    let mut line = body;
    line.push('\t');
    line.push_str(&format!("{chain:08x}"));
    line.push('\n');
    (line, chain)
}

/// 一行的校验和读数。
pub(crate) enum LineSum<'a> {
    /// **旧记录**：加校验和之前写下的，没有校验和。读得出来、不做校验；它照样进链，
    /// 因此**后面**那条新记录的校验和会把它一起护住（改它就等于改后面那条的链）。
    Absent(&'a str),
    /// 带着校验和：正文 ＋ 值。
    Present(&'a str, u32),
    /// 声称带校验和、却读不出来（尾巴被截断，或这一行被改过）⇒ 明确失败。
    Unreadable,
}

/// 把一行切成"正文 ＋ 校验和"。
///
/// 校验和写在**行尾、制表符之后**，而不是塞进 JSON 里：这样校验的是**盘上真正的那串字节**
/// （塞进 JSON 就得先做一次规范化重排再算——既贵又脆）。正文里不可能出现裸制表符：
/// JSON 会把制表符转义成 `\t` 两个字符（`escape_json` 与 `serde_json` 都这样），
/// 因此按**最后一个**制表符切开是无歧义的。
pub(crate) fn line_sum(line: &str) -> LineSum<'_> {
    let Some(index) = line.rfind('\t') else {
        return LineSum::Absent(line);
    };
    match u32::from_str_radix(&line[index + 1..], 16) {
        Ok(sum) => LineSum::Present(&line[..index], sum),
        Err(_) => LineSum::Unreadable,
    }
}

/// 从一个**已经解析过**的 JSON 值里解出正文。
///
/// 拆出这一层的理由与 [`summaries_of`] 同族（2026-10-06 长跑读数那一刀）：一行里的 `seq`、
/// `run`、正文从前**各解析一遍**（`extract_u64_field` 一次、`extract_string_field` 一次、
/// 这里再一次），于是"打开一份账本"要付**三倍**的 JSON 解析钱。现在读侧解析一次、
/// 三样都从同一个 `Value` 里取（实测见 `tests/durability_reading.rs`）。
pub(crate) fn decode_trace_value(value: &serde_json::Value) -> Option<TracePayload> {
    let kind = value.get("kind")?.as_str()?;
    Some(match kind {
        // 缺字段 = 起源没有被记录（旧记录就是这一形态）⇒ `None`，**不编造**。
        //
        // 取值不认识**不读作 `None`**：那一行整行读不出来（`None` 返回给调用方，它会按
        // `MalformedEntry` 拒绝打开）——静默降级会把"被外部改过的行"伪装成一条合法旧记录，
        // 而"没有起源"与"起源读不出来"是两件不同的事实，不能压成同一个结果。
        "run-start" => TracePayload::RunStarted {
            origin: match value.get(ORIGIN_FIELD).and_then(|origin| origin.as_str()) {
                Some(text) => Some(WakeOrigin::from_str(text)?),
                None => None,
            },
        },
        "run-input" => TracePayload::RunInput {
            text: value.get("text")?.as_str()?.to_string(),
        },
        "injection" => TracePayload::Injection {
            stage: value.get("stage")?.as_str()?.to_string(),
            turn: value
                .get("turn")
                .and_then(|turn| turn.as_u64())
                .map(|t| t as u32),
            blocks: decode_blocks(value.get("blocks"))?,
        },
        "action-selected" => TracePayload::ActionSelected {
            turn: value.get("turn")?.as_u64()? as u32,
            // 缺字段 = 第 0 个动作（旧环境里**每一行**都是这样）。
            index: stored_index(value)?,
            action: value.get("action")?.as_str()?.to_string(),
            params: decode_pairs(value.get("params"))?,
        },
        "observed" => TracePayload::Observed {
            turn: value.get("turn")?.as_u64()? as u32,
            index: stored_index(value)?,
            action: value.get("action")?.as_str()?.to_string(),
            params: decode_pairs(value.get("params"))?,
            state: decode_state(value.get("state")?.as_str()?)?,
            failure: value
                .get("failure")
                .and_then(|class| class.as_str())
                .map(str::to_string),
            blocks: decode_blocks(value.get("blocks"))?,
        },
        // `turn` 是 `u32`：盘上的数字超出范围时**整行读不出来**，绝不截断成一个合法轮次——
        // 截断过的轮次会让"这一次响应属于哪一轮"变成一句假话。
        "model-response" => TracePayload::ModelResponse {
            turn: u32::try_from(value.get("turn")?.as_u64()?).ok()?,
            content: value.get("content")?.as_str()?.to_string(),
            decision: value.get("decision")?.as_str()?.to_string(),
            action: value
                .get("action")
                .and_then(|action| action.as_str())
                .map(str::to_string),
            params: match value.get("params") {
                Some(params) => decode_pairs(Some(params))?,
                None => Vec::new(),
            },
            extra: decode_extra(value.get("extra-actions"))?,
            ending: value
                .get("ending")
                .and_then(|ending| ending.as_str())
                .map(str::to_string),
            // 三个字段必须**一起**出现：只有一部分就是损坏，不当成"没有续接数据"。
            protocol: match (
                value.get("protocol").and_then(|item| item.as_str()),
                value.get("protocol-version").and_then(|item| item.as_u64()),
                value.get("payload").and_then(|item| item.as_str()),
            ) {
                (Some(protocol), Some(version), Some(payload)) => Some((
                    protocol.to_string(),
                    u32::try_from(version).ok()?,
                    payload.to_string(),
                )),
                (None, None, None) => None,
                _ => return None,
            },
        },
        "ending" => TracePayload::Ending {
            subject: value
                .get("subject")
                .and_then(|ending| ending.as_str())
                .map(str::to_string),
            system: value
                .get("system")
                .and_then(|ending| ending.as_str())
                .map(str::to_string),
            // 缺字段 = 分类没有被记录（旧记录就是这一形态）⇒ `None`，**不编造**。
            failure: value
                .get("failure")
                .and_then(|class| class.as_str())
                .map(str::to_string),
        },
        TRACE_SEALED => TracePayload::Sealed,
        _ => return None,
    })
}

/// 把一条事实编码成一行。
///
/// 返回 `Result` 只为一件新事：**模型失败分类要过封闭表**（见 [`checked_failure_class`]）。
/// 其余字段照旧不会失败。
///
/// ⚠ **2026-10-06 起带校验和**：返回"整行 ＋ 新的链值"（与 [`encode_trace`] 同一条判据；
/// 事实侧有**自己的一条链**，两条链互不影响）。
pub(crate) fn encode_fact(
    seq: u64,
    run: &RunId,
    record: &Record,
    previous: u32,
    at: Option<u64>,
) -> Result<(String, u32), StoreError> {
    let mut object = serde_json::Map::new();
    object.insert("seq".to_string(), serde_json::Value::from(seq));
    object.insert("run".to_string(), serde_json::Value::from(run.as_str()));
    write_at(&mut object, at);

    match record {
        Record::RunStarted { agent, .. } => {
            object.insert("fact".to_string(), serde_json::Value::from("run-started"));
            object.insert("agent".to_string(), serde_json::Value::from(agent.as_str()));
        }
        Record::ActionPrepared { operation, .. } => {
            object.insert(
                "fact".to_string(),
                serde_json::Value::from("action-prepared"),
            );
            object.insert(
                "turn".to_string(),
                serde_json::Value::from(operation.turn().number()),
            );
            write_index(&mut object, operation.index());
        }
        Record::ActionDispatched { operation, .. } => {
            object.insert(
                "fact".to_string(),
                serde_json::Value::from("action-dispatched"),
            );
            object.insert(
                "turn".to_string(),
                serde_json::Value::from(operation.turn().number()),
            );
            write_index(&mut object, operation.index());
        }
        Record::ActionResultRecorded { action, .. } => {
            object.insert("fact".to_string(), serde_json::Value::from("action-result"));
            object.insert(
                "turn".to_string(),
                serde_json::Value::from(action.operation().turn().number()),
            );
            object.insert(
                "state".to_string(),
                serde_json::Value::from(state_str(action.state())),
            );
            write_index(&mut object, action.operation().index());
        }
        Record::RunEnded { outcome, cost, .. } => {
            object.insert("fact".to_string(), serde_json::Value::from("run-ended"));
            match outcome {
                Outcome::Subject(ending) => {
                    object.insert(
                        "subject".to_string(),
                        serde_json::Value::from(subject_str(*ending)),
                    );
                }
                Outcome::System(ending) => {
                    object.insert(
                        "system".to_string(),
                        serde_json::Value::from(system_str(*ending)),
                    );
                    // 分类**只**在模型失败且当时带着分类时写；其余收场与旧记录都不写这个字段。
                    // 写之前**过封闭表**：词表之外的值落下去就是一行谁也读不回来的记录。
                    if let SystemEnding::ModelFailed(Some(class)) = ending {
                        object.insert(
                            "failure".to_string(),
                            serde_json::Value::from(checked_failure_class(class)?),
                        );
                    }
                }
            }
            // **代价**：时长与用量各自"没有就不写字段"（与 `write_index`、`failure` 同辙）
            // ——旧记录（两者都没有）在盘上逐字节不变。
            write_cost(&mut object, cost);
        }
    }

    Ok(signed_line(
        serde_json::Value::Object(object).to_string(),
        previous,
    ))
}

/// 从一个**已经解析过**的 JSON 值里解出一条事实行（同 [`decode_trace_value`] 的理由）。
pub(crate) fn decode_fact_row_value(value: &serde_json::Value) -> Option<FactRow> {
    let seq = value.get("seq")?.as_u64()?;
    let run = value.get("run")?.as_str()?.to_string();
    let fact = value.get("fact")?.as_str()?.to_string();
    let agent = value
        .get("agent")
        .and_then(|agent| agent.as_str())
        .map(str::to_string);
    // Turn 是 `u32`：盘上的数字超出这个范围时**整行读不出来**，绝不截断成一个合法 Turn。
    // 极端数字被截断成 1..=u32::MAX 里某个值，会把"被外部改过的行"伪装成一次正常行动。
    let turn = match value.get("turn").and_then(|turn| turn.as_u64()) {
        Some(turn) => Some(u32::try_from(turn).ok()?),
        None => None,
    };
    // 子序号**先原样收下**（`u64`），合法范围留到 `record_of_row` 里按行报错——
    // 与 `turn` 的处理不同：Turn 超范围是"整行读不出来"，而序号超范围是**这一行的取值非法**，
    // 两者要能分开报告。
    let index = value.get("index").and_then(|index| index.as_u64());
    let state = value
        .get("state")
        .and_then(|state| state.as_str())
        .map(str::to_string);
    let outcome = ending_label(
        value.get("subject").and_then(|ending| ending.as_str()),
        value.get("system").and_then(|ending| ending.as_str()),
    );
    // 模型失败的分类：**先原样收下**，取值的封闭校验留给 `fact_row_is_interpretable`
    // （与 `index` 的处理同族：这里是结构解码，那里是语义解释）。
    let failure = value
        .get("failure")
        .and_then(|failure| failure.as_str())
        .map(str::to_string);
    // **代价**：与 `failure` 同族——这里是结构解码（"这个字段在不在、是不是数"），
    // "这一行允不允许带它"留给 `fact_row_is_interpretable`。
    //
    // 类型不对**读作缺字段**（不读作 0）：与 `usage` 在适配层"遥测不是契约"同一条纪律——
    // 一个坏的时长不该让整行读不出来，也不该被当成"0 毫秒"。它只是"这一格没有被记"。
    let cost = RunCost {
        elapsed_millis: value.get(ELAPSED_FIELD).and_then(serde_json::Value::as_u64),
        usage: {
            let usage = Usage {
                prompt_tokens: value
                    .get(USAGE_PROMPT_FIELD)
                    .and_then(serde_json::Value::as_u64),
                completion_tokens: value
                    .get(USAGE_COMPLETION_FIELD)
                    .and_then(serde_json::Value::as_u64),
                total_tokens: value
                    .get(USAGE_TOTAL_FIELD)
                    .and_then(serde_json::Value::as_u64),
            };
            if usage.is_empty() { None } else { Some(usage) }
        },
    };
    Some(FactRow {
        seq,
        run,
        fact,
        agent,
        turn,
        index,
        state,
        outcome,
        failure,
        cost,
        at_millis: value.get(AT_FIELD).and_then(|at| at.as_u64()),
    })
}

/// 行动请求被准入拒绝时使用的注入阶段名。
///
/// 它**是新增的持久语义值**：字段形状沿用既有的 `kind=injection`（与暂停标记用
/// `stage="storage-failure"` 是同一类做法），但"这个阶段名表示一次未被接纳的行动请求"这件事
/// 是本次新增的语义。既有解码器只要求 `kind` 已知、`stage` 是字符串，因此它**能读**这一行，
/// 但**不理解**它的含义——可解析不等于语义兼容。
pub(crate) const REFUSED_STAGE: &str = "admission-refused";
