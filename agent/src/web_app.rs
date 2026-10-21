//! web 窗：主体的信息网（`web_search` + `web_fetch`）。
//!
//! 模型的知识边界停在训练截止。本件给主体**两条只读的出网动词**——**查**（搜索厂商）与**取**
//! （直连目标页）——于是"今天发生了什么"这类主体本来答不出来的事，主体能自己去问、自己去看。
//!
//! ## 两条动词，两个对面
//!
//! | 动词 | 对面是谁 | 凭据 | 回来的是什么 |
//! |---|---|---|---|
//! | `web_search` | **搜索厂商**（方言与端点是**配置**，见下） | 宿主声明的凭据；没声明 ⇒ `credential-absent`（**没配**，不是被拒），**动词照旧在场** | ≤5 条结果，每条一行：标题 / 地址 / 摘要 / 日期 |
//! | `web_fetch` | **目标页本身**（没有厂商） | 不需要 | 有界纯文本（网页先做**机械抽取**） |
//!
//! ## 搜索的两个方言（2026-10-20）
//!
//! 主体的词永远只有那两个；**方言**说的是"对面回执的形状"，只有两个取值（封闭表，认不得就拒绝）：
//!
//! | 方言 | 请求体 | 回执 | 代价 |
//! |---|---|---|---|
//! | `results-json` | `{query, max_results}` | `results[]`：标题 / 地址 / 摘要 / 时间 | 一次请求 |
//! | `search-api` | `{query, count, need_content:false}` | `results[]`：标题 / 地址 / `summary`（缺则 `snippet`）/ `date` | **按请求次数计费**（有账号级 QPS） |
//!
//! 两个方言各有一对「构造请求 / 折回执」，折出来的是同一个 [`Row`]，**渲染只有一处** ⇒ 换方言不动
//! 读数形状。厂商名**不在本件**：来源与方言由 [`crate::search_source`] 从「清单 + 绑定」读出来，
//! 厂商名只在写侧那张表与入口的凭据变量名里（语言守卫钉着）。
//!
//! **厂商方言锁在本件内**：换一家（方言不同）只改本件，Agent 的能力面一个字节都不变。
//!
//! ⚠ **`search-api` 的第二家形状没有现成消费者就不要加**：两个取值已经覆盖了"直接回结果列表"与
//! "搜索接口（count/need_content）"两种真实形状；第三种要等真的第二份回执出现。
//!
//! ## 读数：地址是**把手**，不是展示文本
//!
//! `web_search` 每条一行、四段；标题与摘要按展示预算截断，**地址不截断**——它要被原样交给
//! `web_fetch`，剪过的地址要么解析不了、要么取到别处。超过地址上限的那一行**不列**，首行的
//! `addresses_over_limit` 说有几行：少一行比给一个假地址诚实。日期是标签、不是把手，照展示预算截断。
//!
//! ## 抓取的四条机械限制（2026-10-20 起：**抓取也走对面 API**）
//!
//! 主人裁：`web_fetch` 与 `web_search` 同形——都走对面 API。**没有"自己抓"这一条回退**：
//! 想要原生抓取是 `bash` 的事（`--terminal full` 那一档），不是本件偷偷留的路。
//!
//! 因此能守的只剩四条（前三条是本件的行为契约，第四条是花钱的纪律）：
//!
//! 1. **地址只判形状**：必须是网页地址（明文或加密那两种协议）；别的本地拒绝；
//! 2. **单次回执 ≤ 64 KiB**：超限**拒绝、不截断**（与 `files_read` 先例同一条纪律）；
//! 3. **单次请求 30 秒操作窗口**：超时即失败，不无限等；
//! 4. **一次调用恰好一次请求**（零重试）——对面那条路按次计费，"悄悄重试"等于悄悄多花钱。
//!
//! ⚠ **私网、重定向、内容类型都不是我们能管的了**：那是对面去取，它的出口与策略不归我们。
//! 这一条写在这里而不是藏起来——**把"我们拒绝私网"这句话带进厂商那条路就是描述句与事实打架**。
//!
//! ## 失败词汇（本件自己的封闭表）
//!
//! `transport-unreachable` / `timeout` / `credential-absent` / `credential-rejected` / `invalid-argument` /
//! `too-large` / `server-error`。
//!
//! ⚠ **三格随之退役**（2026-10-20，直连抓取退役）：`wrong-kind`（对面给的不是正文）、`redirect-loop`
//! （重定向成环）、`private-address-refused`（解析到私网）——它们量的是"我们自己抓"那条路。
//!
//! ⚠ **凭据那两格是两件事**（2026-10-13 拆开）：`credential-absent` = **这台机器没配**（本地事实，
//! 一次出网请求都不发）；`credential-rejected` = **对面拒了**（401/403，对面的裁决）。同一条三态纪律
//! （"没有"与"读不到/被拒"不许压成一档）。
//!
//! 两条纪律：① **同一种故障逐字节相同**；② **一次调用恰好一次出网请求**（零重试——网络故障不
//! 自动重发，主体看到失败后自己决定做什么）。`server-error` **不分解 4xx 与 5xx**：对面不是一个有
//! 契约的服务端，同一个状态码在不同站点含义不同，我们不替它解释（与 im 那条"通用层只搬机械事实"
//! 同一条纪律）。
//!
//! ⚠ **凭据那两格是两件事**（2026-10-13 拆开）：`credential-absent` = **这台机器没配**（本地事实，
//! 一次出网请求都不发）；`credential-rejected` = **对面拒了**（401/403，对面的裁决）。同一条三态纪律
//! （"没有"与"读不到/被拒"不许压成一档）。
//!
//! 两条纪律：① **同一种故障逐字节相同**；② **一次调用恰好一次出网请求**（零重试——网络故障不
//! 自动重发，主体看到失败后自己决定做什么）。`server-error` **不分解 4xx 与 5xx**：对面不是一个有
//! 契约的服务端，同一个状态码在不同站点含义不同，我们不替它解释（与 im 那条"通用层只搬机械事实"
//! 同一条纪律）。
//!
//! ## 与材料窗的分工（窗界）
//!
//! **材料窗（`files`）= 宿主给主体的地界**（要点开才在场，跨边界写要授权）；**web 窗 = 外面那个世界**
//! （出厂内置、常开、只读）。两者都是"看"，但一个看的是**给主体的东西**，一个看的是**谁都能看的东西**。

use std::io::Read;
use std::time::Duration;

use crate::app_contract::{
    AppPort, EffectClass, EffectDeclaration, SourceClass, SourceDeclaration,
};
use crate::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Capabilities,
    Capability, ContentBlock, ParamSpec,
};
use crate::foundation::{AgentId, OperationId, RunId};

/// 这个 App 的稳定标识。
pub const WEB_APP_ID: &str = "web";

/// 查：问搜索厂商。
pub const CAPABILITY_SEARCH: &str = "web_search";

/// 取：直连目标页。
pub const CAPABILITY_FETCH: &str = "web_fetch";

const PARAM_QUERY: &str = "query";

/// 目标地址那个参数的名字（**值**按施工单定稿；**标识符**是我们自己的词）。
///
/// 它是**主体的工具面**上的一个名字，不是我们内部随手起的标识符：本文件里我们自己的标识符与
/// 变量一律叫 `target` / `PARAM_TARGET`，按定稿写下来的只有那一个字面量。
const PARAM_TARGET: &str = "url";

/// 一次查询最多回来几条（机械上限：多的不列，如实报截断）。
pub const MAX_RESULTS: usize = 5;

/// 查询的机械上限（厂商建议：一条查询别超过这个长度）。超限**本地拒绝、零出网**。
pub const MAX_QUERY_BYTES: usize = 400;

/// `search-api` 方言自己的查询上限：1～128 个 Unicode 字符（对面明写的）。
const SEARCH_API_MAX_QUERY_CHARS: usize = 128;

/// 结果里标题与摘要那两栏的机械截断上限。
pub const MAX_FIELD_BYTES: usize = 200;

/// 日期那一栏的机械截断上限（日期是标签，不是把手：厂商给长了就是给了别的东西）。
pub const MAX_DATE_BYTES: usize = 64;

/// 地址那一栏的上限：**地址是把手，所以不截断**——超限的那一行不列，不剪成半截
/// （剪过的地址会把 `web_fetch` 指到别处，比少一行更坏）。真实来源地址通常几百字节，
/// 这一格是保守上限。
pub const MAX_ADDRESS_BYTES: usize = 2048;

/// 单次取页的字节上限：超限**拒绝**，不截断。
pub const MAX_PAGE_BYTES: usize = 64 * 1024;

/// 一次请求的操作窗口（连接 / 读 / 写各自的墙钟上限，不是整条流的总时长）。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// 本件自己的失败词汇表（**封闭表**：全部取值与 [`WebFailure::as_str`] 逐条对称）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebFailure {
    /// 连不上（域名解析不出来、连接被拒、半截响应）。
    Unreachable,
    /// 超时（与 `Unreachable` 同属**传输**一族：这次交换没能完成）。
    Timeout,
    /// 对面拒绝了凭据（对面回了 401/403）。**不重试、不换凭据**。
    ///
    /// ⚠ **2026-10-13 拆开**：这一格从前把"**这台机器没配**凭据"也算了进来（旧注释原话：
    /// "或压根没有声明凭据"）——而那是**另一件事**：没配是**本地的事实**（一次出网请求都不发），
    /// 被拒是**对面的裁决**。混装一句话的代价很具体：看到"被拒"会去查凭据有效性，
    /// 而真正该做的是**配一个**。⇒ 没配现在是 [`WebFailure::CredentialAbsent`]。
    CredentialRejected,
    /// **这台机器没有配**这一格凭据（宿主没声明）⇒ 一次出网请求都不发。
    ///
    /// 与 `CredentialRejected` 的区别就是"**没有**"与"**被拒**"的区别——与
    /// `SelfConcept::Absent` / `SelfConcept::Unreadable` 同一条三态纪律。**动词照旧在场**：缺的是这一格，
    /// 不是这个能力。
    ///
    /// **两格指向两个不同的动作**：这一格 ⇒ 操作者该**配一个**（本机没声明，与对面无关）；被拒那一格 ⇒
    /// 该去查那一份**还有没有效**（对面明说了不认）。两句话**不许互相顶替**——报错一格，操作者就会去查
    /// 一件本来不存在的东西（旧注释把两件事写成一句，见 [`WebFailure::CredentialRejected`]）。
    CredentialAbsent,
    /// 参数不合法（空查询、超长、地址读不懂、不是网页地址）。
    ///
    /// 这一族的共同点是**在本地就判掉了**：一次出网请求都不发。
    InvalidArgument,
    /// 超过了单次回执的字节上限（**拒绝**，不截断）。
    TooLarge,
    /// 对面回了错误状态（401/403 之外），或者回执读不懂。**不分解 4xx / 5xx**（见模块文档）。
    ServerError,
}

impl WebFailure {
    /// 稳定分类标记（进 `ActionOutcome`，Core 只搬运不解释）。
    pub fn as_str(self) -> &'static str {
        match self {
            WebFailure::Unreachable => "transport-unreachable",
            WebFailure::Timeout => "timeout",
            WebFailure::CredentialRejected => "credential-rejected",
            WebFailure::CredentialAbsent => "credential-absent",
            WebFailure::InvalidArgument => crate::core::INVALID_ARGUMENT,
            WebFailure::TooLarge => "too-large",
            WebFailure::ServerError => "server-error",
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
            "transport-unreachable" => WebFailure::Unreachable,
            "timeout" => WebFailure::Timeout,
            "credential-rejected" => WebFailure::CredentialRejected,
            "credential-absent" => WebFailure::CredentialAbsent,
            crate::core::INVALID_ARGUMENT => WebFailure::InvalidArgument,
            "too-large" => WebFailure::TooLarge,
            "server-error" => WebFailure::ServerError,
            _ => return None,
        })
    }
}

/// 搜索**方言**：对面回的**形状**。封闭两档（认不出的取值拒绝，不降级、不就近映射）。
///
/// ⚠ **方言不是厂商名**：厂商名只在写侧（`scripts/home/search-sources.js`）与入口的环境变量名里；
/// 本件只认识"对面长什么样"。⇒ 换一家**同方言**的只换配置；换一家**方言不同**的，改的就是本件
/// 这一处（一个取值 = 一对「构造请求 / 折回执」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchWire {
    /// 对面**直接回一个结果列表**（`results[]`：标题 / 地址 / 摘要 / 日期）。请求体是
    /// `{query, max_results}`。
    ResultsJson,
    /// 一家的**搜索接口**：请求体是 `{query, count, need_content}`，回执同样是 `results[]`，
    /// 但摘要字段叫 `summary`（给模型读的长摘要，缺了退回 `snippet`）、时间字段叫 `date`。
    ///
    /// ⚠ 它按**请求次数**计费（不按字数、也不按结果条数），有账号级 QPS；这一格是**代价**，
    /// 不是形状——写在这里是因为选方言之前就该知道它花什么。
    SearchApi,
}

impl SearchWire {
    /// 配置里那个稳定词（**封闭**；认不得 ⇒ `None`，由调用方 fail-closed）。
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text.trim() {
            "results-json" => Self::ResultsJson,
            "search-api" => Self::SearchApi,
            _ => return None,
        })
    }

    /// 稳定词（与 [`SearchWire::parse`] 逐条对称）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ResultsJson => "results-json",
            Self::SearchApi => "search-api",
        }
    }

    /// 这个方言**自己**的查询字数上限（Unicode 字符）；`None` = 没有已知的上限。
    ///
    /// 它是**方言的事实**，不是我们的政策：`search-api` 对面明写 1～128 个字符。写在方言上而不是
    /// 写成一个全局上限，是因为换一家之后那个数就不成立了——而"把别人的上限当成我们的规矩"
    /// 会悄悄拒绝本来能用的查询。
    pub fn max_query_chars(self) -> Option<usize> {
        match self {
            Self::ResultsJson => None,
            Self::SearchApi => Some(SEARCH_API_MAX_QUERY_CHARS),
        }
    }
}

/// 宿主给本件的**声明**：一条搜索源——方言、端点、凭据。
///
/// 全由装配面给出（清单与绑定由 [`crate::search_source`] 读，入口那一处才认识厂商名），
/// 本件自己不去找、不配置、不落盘。凭据只被写进请求头；本类型**不派生 `Debug`**，
/// 因此它不可能被顺手打进日志或报告。
///
/// 缺这一份声明不是"装不了"：`web_search` 照旧在 Agent 的能力面里，只是**执行时如实说没有凭据**
/// （`credential-absent`——**这台机器没配**，不是对面拒了，见 [`WebFailure`]）；`web_fetch` 与厂商无关，
/// 照常可用。
#[derive(Clone)]
pub struct SearchAccess {
    wire: SearchWire,
    endpoint: String,
    credential: String,
}

impl SearchAccess {
    /// 旧路（2026-10-20 之前唯一的一条）：端点 + 凭据 ⇒ 结果列表方言。**行为一个字节都不变**。
    pub fn new(endpoint: impl Into<String>, credential: impl Into<String>) -> Option<Self> {
        Self::with_wire(SearchWire::ResultsJson, endpoint, credential)
    }

    /// 按方言建立一份声明；空值一律拒绝（"没配好"不是一种运行状态）。
    pub fn with_wire(
        wire: SearchWire,
        endpoint: impl Into<String>,
        credential: impl Into<String>,
    ) -> Option<Self> {
        let endpoint = endpoint.into();
        let credential = credential.into();
        if endpoint.trim().is_empty() || credential.trim().is_empty() {
            return None;
        }
        Some(Self {
            wire,
            endpoint,
            credential,
        })
    }

    /// 这一条是哪种方言（读数与用例用；不参与请求构造）。
    pub fn wire(&self) -> SearchWire {
        self.wire
    }
}

/// 抓取**方言**：对面回的**形状**。封闭两档（认不出的取值拒绝，不降级）。
///
/// ⚠ **没有"自己抓"这一档**（2026-10-20 主人裁）：`web_fetch` 与 `web_search` 同形——都走对面
/// API。想要"原生抓"是 `bash` 的事，不是本件偷偷留的一条回退。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchWire {
    /// 一家**取一个网页**的接口：请求带一个地址，回执是标题与正文（Markdown）。
    Doc,
    /// 一家**批量取网页**的接口：请求带地址数组与格式，回执取第一条结果的正文。
    /// ⚠ **状态码 200 里 `results` 也可能是空**（整批都没取到），失败逐条在 `failed_results[]`。
    Raw,
}

impl FetchWire {
    /// 配置里那个稳定词（**封闭**；认不得 ⇒ `None`，由调用方 fail-closed）。
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text.trim() {
            "fetch-doc" => Self::Doc,
            "fetch-raw" => Self::Raw,
            _ => return None,
        })
    }

    /// 稳定词（与 [`FetchWire::parse`] 逐条对称）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Doc => "fetch-doc",
            Self::Raw => "fetch-raw",
        }
    }
}

/// 宿主给本件的**抓取声明**：方言 ＋ 端点 ＋ 凭据。
///
/// 与 [`SearchAccess`] 同一族：只被写进请求头，**不派生 `Debug`** ⇒ 不可能被顺手打进日志。
#[derive(Clone)]
pub struct FetchAccess {
    wire: FetchWire,
    endpoint: String,
    credential: String,
}

impl FetchAccess {
    /// 建立一份抓取声明；空值一律拒绝（"没配好"不是一种运行状态）。
    pub fn new(
        wire: FetchWire,
        endpoint: impl Into<String>,
        credential: impl Into<String>,
    ) -> Option<Self> {
        let endpoint = endpoint.into();
        let credential = credential.into();
        if endpoint.trim().is_empty() || credential.trim().is_empty() {
            return None;
        }
        Some(Self {
            wire,
            endpoint,
            credential,
        })
    }

    /// 这一条是哪种方言（读数与用例用）。
    pub fn wire(&self) -> FetchWire {
        self.wire
    }
}

/// 出网窗的那两个**操作旋钮**（与门铃的 `DoorbellPolicy` 同一形状：把"多久算超时"与
/// "要不要连本机/内网"显式说出来，生产给生产值，离线用例给能把回环假服务接进来的值）。
///
/// 它不是安全开关的集合：**五堵墙里的另外四堵没有旋钮**（只取不写、单次上限、重定向上限、
/// 内容类型），因为它们是本件的行为契约。这两个之所以有旋钮，是因为它们度量的是**部署环境**
/// ——同一份代码在内网受控环境里取内网页是合法需求，而"出网窗不指向家里"是本机单用户环境里
/// 的**保守选择**（见交付报告）。装配一律用 [`WebPolicy::production`]。
#[derive(Clone, Debug)]
pub struct WebPolicy {
    /// 单次请求的操作窗口（连接 / 读 / 写各自的上限）。
    pub request_timeout: Duration,
}

impl WebPolicy {
    /// 生产策略：30 秒窗口。
    pub fn production() -> Self {
        Self {
            request_timeout: REQUEST_TIMEOUT,
        }
    }
}

/// **出网档**（2026-10-13）：这台机器**给不给它出网窗**，由操作者在装配面声明——形状照
/// `--terminal` 的先例（封闭枚举、不认识的档位不降级、不预建更细的档：加档是加墙）。
///
/// 判据一句话：**"主体能不能出网"是主人的政策**——既不是 App 的属性，也不是"配置"。
/// 它与 terminal 的档位是同一类事实（`--terminal` 不给就没有那只手）。
///
/// **这个轴为什么必须归主人**：web 是**出网窗**——机器可能在气隙网里、搜索按次计费、或者主人
/// 就是不希望这一件出网。三件事没有一件是这件 App 的属性；**也不给主体开一个它自己控制不了
/// 自己的开关**——在不在场由装配面声明，不由本件自己决定。
///
/// ⚠ **`Off` 的含义是"不装配这件 App"**：主体的门里连 `web` 这扇门都没有（不是"有门但打不开"）。
/// 它由装配面消费——[`WebApp::with_tier`] 收到 `Off` 直接返回 `None`，装配面据此不摆这件。
///
/// ⚠ **档位不是 [`WebPolicy`] 的第三个旋钮**：那两格（操作窗口、私网清单）度量的是**部署环境**，
/// 逐跳检查的是"取哪个地址"；档位度量的是"这件 App 摆不摆"。两者都是装配期输入，**不是同一根轴**
/// ——别把一个部署旋钮读成"给不给它出网"。
///
/// ⚠ **更细的档不预建**（域名白名单之类）：今天没有任何读数在说"某些地址不许取"，**触发条件**
/// 就是那份读数真出现时再加。别先造一格，再去找它的用户。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WebTier {
    /// **不给出网窗**：这一档不装配本件（见上）。
    Off,
    /// **只取**：主体知道地址才取得到——不依赖厂商、不花搜索那份钱。
    Fetch,
    /// **查 ＋ 取**（默认，也是本件出厂以来的样子）。
    #[default]
    Full,
}

impl WebTier {
    /// 命令行那三个词（**封闭**）。
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "off" => WebTier::Off,
            "fetch" => WebTier::Fetch,
            "full" => WebTier::Full,
            _ => return None,
        })
    }

    /// 稳定词（与 [`WebTier::parse`] 逐条对称）。
    pub fn as_str(self) -> &'static str {
        match self {
            WebTier::Off => "off",
            WebTier::Fetch => "fetch",
            WebTier::Full => "full",
        }
    }
}

/// 主体的信息网。
pub struct WebApp {
    /// 搜索源的声明（可缺——缺了只有「查」那一条失败，别的照旧）。
    search: Option<SearchAccess>,
    /// **抓取源的声明**（2026-10-20 起：抓取与搜索同形，都走对面 API）。
    ///
    /// ⚠ 这一格缺席就是"这台机器没配这一格"（执行时 `credential-absent`）——**没有自己抓的回退**。
    fetch: Option<FetchAccess>,
    /// 请求客户端（单次请求一个窗口；**不跟随重定向**：对面回 3xx 按 `server-error` 如实报）。
    client: reqwest::blocking::Client,
    /// **这一台机器给到哪一档**（见 [`WebTier`]）。它只影响**声明面**（「查」在不在场）。
    tier: WebTier,
}

impl WebApp {
    /// 装配一件 web 窗（请求客户端在这里建一次，含单次请求的操作窗口）。
    ///
    /// 生产装配只走这一条：私网拒绝、30 秒窗口。
    pub fn new(search: Option<SearchAccess>) -> Option<Self> {
        Self::with_tier(search, WebTier::Full, WebPolicy::production())
    }

    /// 用显式策略装配（生产不用它；离线用例用它把窗口调小、把回环假服务接进来）。
    pub fn with_policy(search: Option<SearchAccess>, policy: WebPolicy) -> Option<Self> {
        Self::with_tier(search, WebTier::Full, policy)
    }

    /// **搜索 ＋ 抓取两份声明 ＋ 出网档**（2026-10-20）：抓取也走对面 API 之后，装配要同时给出
    /// 两条来源；档位照旧是"摆不摆这件 App"。
    pub fn with_sources(
        search: Option<SearchAccess>,
        fetch: Option<FetchAccess>,
        tier: WebTier,
        policy: WebPolicy,
    ) -> Option<Self> {
        Self::assemble(search, fetch, tier, policy)
    }

    /// **按出网档装配**（生产走这一条）：`Off` ⇒ `None`（**不摆这件 App**——主体的门里没有它）。
    pub fn with_tier(
        search: Option<SearchAccess>,
        tier: WebTier,
        policy: WebPolicy,
    ) -> Option<Self> {
        Self::assemble(search, None, tier, policy)
    }

    fn assemble(
        search: Option<SearchAccess>,
        fetch: Option<FetchAccess>,
        tier: WebTier,
        policy: WebPolicy,
    ) -> Option<Self> {
        if tier == WebTier::Off {
            return None;
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(policy.request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()?;
        Some(Self {
            search,
            fetch,
            client,
            tier,
        })
    }

    /// 本件声明的全部能力（顺序固定）：两条只读动词。
    fn own_capabilities(&self) -> Capabilities {
        // **档位排除的动词不进声明面**（`fetch` 档里「查」不在场——与 `--terminal` 不给就没有那只手
        // 同款）：声明面就是主体读到的那扇门，执行面同源（见 `execute` 里那一支防御）。
        let mut items = Vec::new();
        if self.tier == WebTier::Full {
            items.push(Capability::new(
                ActionId::new(CAPABILITY_SEARCH),
                format!(
                    "在网上搜一次（最多 {MAX_RESULTS} 条结果，每条一行「标题 | 地址 | 摘要 | 日期」）"
                ),
                vec![ParamSpec::new(
                    PARAM_QUERY,
                    true,
                    format!(
                        "查询词（非空、≤{MAX_QUERY_BYTES} 字节；有的方言还限字数——超了也是本地拒绝、不出网）"
                    ),
                )],
            )
            .with_details(format!(
                "读数形状：每行四段——标题、地址、摘要、日期。标题与摘要每栏最多 {MAX_FIELD_BYTES} \
                 字节、日期最多 {MAX_DATE_BYTES} 字节，超出即机械截断（带 `…`）；地址是把手，不截断\
                 （超过 {MAX_ADDRESS_BYTES} 字节的那一行不列，首行用 `addresses_over_limit` 说有几行）。\
                 首行给出取回条数、`truncated` 与那两个计数。要读某一条的全文，把它那一行的\
                 地址原样交给 `{CAPABILITY_FETCH}`。只读：不改变任何东西。凭据：搜索凭据由宿主\
                 声明——没声明时这个动词照旧在场，执行时如实失败 `credential-absent`（意思是这台\
                 机器还没给它配，不是对面拒绝了；对面拒了是 `credential-rejected`）。",
            )));
        }
        items.push(Capability::new(
            ActionId::new(CAPABILITY_FETCH),
                "取一个网页的正文（交给配置好的那家取；回来是它给的正文）",
                vec![ParamSpec::new(
                    PARAM_TARGET,
                    true,
                    "要取的地址（网页地址；原样交给本动作，不解析、不改写）",
                )],
            )
            .with_details(format!(
                "回来的形状：首行三格——`source`（取自哪里）、`via`（哪一家取的）、`bytes`；下面是它给的正文\
                 （Markdown）。四条机械限制：① 只接受网页地址（明文或加密两种协议）；② 单次最多 {} KiB（超限拒绝，\
                 不截断）；③ 一次 30 秒；④ 一次调用恰好一次请求、不重试。只读：不改变任何东西。\
                 凭据：抓取传递由宿主声明——没声明时这个动词照旧在场，执行时如实失败 \
                 `credential-absent`（这台机器还没配，不是对面拒绝了）。",
                MAX_PAGE_BYTES / 1024
            )),
        );
        Capabilities::of(items)
    }

    /// 查一次（**方言的差别锁在两个 `*_search` 里**，外面的世界只认识 `web_search`）。
    ///
    /// 两个方言共用这一层的东西：查询校验、一次调用恰好一次出网请求、状态码分类、
    /// 有界正文、整份读数的渲染（条数上限、地址是把手那一条）。方言只负责
    /// **构造请求**与**把回执折成行**。
    fn search(&self, params: &ActionParams) -> ActionOutcome {
        let query = match params.get(PARAM_QUERY) {
            Some(query) => query.trim(),
            None => return refused(WebFailure::InvalidArgument),
        };
        if query.is_empty() || query.len() > MAX_QUERY_BYTES {
            return refused(WebFailure::InvalidArgument);
        }
        // 没有凭据就是没有：动词在场、失败如实（**不**编一个空凭据去试）。
        // ⚠ 报的是"**没配**"（本地事实），不是"被拒"（对面的裁决）——2026-10-13 拆开。
        let Some(access) = &self.search else {
            return refused(WebFailure::CredentialAbsent);
        };
        // **方言自己的字数上限**：`search-api` 对面明写 1～128 个 Unicode 字符。超了本地拒绝
        // （`invalid-argument`）——放过去只会换来对面 400，而那个 400 会被归成 `server-error`：
        // 同一件事分成两格，操作者会去查"服务端怎么了"。
        if access
            .wire
            .max_query_chars()
            .is_some_and(|limit| query.chars().count() > limit)
        {
            return refused(WebFailure::InvalidArgument);
        }
        let found = match access.wire {
            SearchWire::ResultsJson => self.search_results_json(access, query),
            SearchWire::SearchApi => self.search_api(access, query),
        };
        match found {
            Ok((rows, truncated)) => ActionOutcome::success(vec![ContentBlock::new(
                CAPABILITY_SEARCH,
                render_rows(&rows, truncated),
            )]),
            Err(failure) => refused(failure),
        }
    }

    /// 方言一：对面直接回一个结果列表（请求体 `{query, max_results}`）。
    fn search_results_json(
        &self,
        access: &SearchAccess,
        query: &str,
    ) -> Result<(Vec<Row>, bool), WebFailure> {
        let body = serde_json::json!({
            "query": query,
            "max_results": MAX_RESULTS,
        });
        let response = self
            .client
            .post(&access.endpoint)
            .header("Authorization", format!("Bearer {}", access.credential))
            .header("Content-Type", "application/json")
            .json(&body)
            .send();
        let body = read_json(response)?;
        let rows = parse_results_json(&body)?;
        let truncated = rows.len() > MAX_RESULTS;
        Ok((rows, truncated))
    }

    /// 方言二：一家的**搜索接口**（请求体 `{query, count, need_content}`）。
    ///
    /// `need_content` 恒为 `false`：正文那一格是整页文本，读数用不上它（摘要已经够），
    /// 开着只是让回执变大、还更快撞上单次回执的上限。
    ///
    /// ⚠ **按请求次数计费**（不是按字数、也不按结果条数）⇒ 一次调用只发一次请求这条纪律
    /// 在这一格上直接关系到花多少钱。
    fn search_api(
        &self,
        access: &SearchAccess,
        query: &str,
    ) -> Result<(Vec<Row>, bool), WebFailure> {
        let body = serde_json::json!({
            "query": query,
            "count": MAX_RESULTS,
            "need_content": false,
        });
        let response = self
            .client
            .post(&access.endpoint)
            .header("Authorization", format!("Bearer {}", access.credential))
            .header("Content-Type", "application/json")
            .json(&body)
            .send();
        let body = read_json(response)?;
        let rows = parse_search_api(&body)?;
        let truncated = rows.len() > MAX_RESULTS;
        Ok((rows, truncated))
    }

    /// 取一个页面：**把地址交给对面**，取回它给的正文。
    ///
    /// 没有抓取声明 ⇒ `credential-absent`（这台机器没配这一格）——与「查」同形，**没有自己抓的回退**。
    fn fetch(&self, params: &ActionParams) -> ActionOutcome {
        let Some(access) = &self.fetch else {
            return refused(WebFailure::CredentialAbsent);
        };
        self.fetch_via(access, params)
    }

    /// 方言那一条：把地址交给对面，取回它给的正文。
    ///
    /// 能守的只剩三件：**地址只判形状**（网页地址）、**回执有界**（超限拒绝、不截断）、
    /// **一次调用恰好一次请求**。私网、重定向、内容类型都是对面的事了——这是"借它的手"的代价，
    /// 写在这里而不是藏起来。
    fn fetch_via(&self, access: &FetchAccess, params: &ActionParams) -> ActionOutcome {
        let address = match params.get(PARAM_TARGET) {
            Some(raw) => raw.trim(),
            None => return refused(WebFailure::InvalidArgument),
        };
        if reqwest::Url::parse(address).is_err() || !address.starts_with("http") {
            return refused(WebFailure::InvalidArgument);
        }
        let body = match access.wire {
            FetchWire::Doc => serde_json::json!({ "url": address }),
            FetchWire::Raw => serde_json::json!({ "urls": [address], "format": "markdown" }),
        };
        let response = self
            .client
            .post(&access.endpoint)
            .header("Authorization", format!("Bearer {}", access.credential))
            .header("Content-Type", "application/json")
            .json(&body)
            .send();
        let body = match read_json(response) {
            Ok(body) => body,
            Err(failure) => return refused(failure),
        };
        let content = match access.wire {
            FetchWire::Doc => body.get("content").and_then(|value| value.as_str()),
            FetchWire::Raw => body
                .get("results")
                .and_then(|value| value.as_array())
                .and_then(|rows| rows.first())
                .and_then(|row| row.get("raw_content"))
                .and_then(|value| value.as_str()),
        };
        // 对面说"取不到"时 `content` 是空的（`fetch-raw` 甚至可能回 200 ＋ 空 `results`）——
        // 与"这一页是空的"分开：前者是失败；后者至少说明对面确实取回了一个空页。
        let Some(content) = content.filter(|text| !text.trim().is_empty()) else {
            return refused(WebFailure::ServerError);
        };
        let reading = format!(
            "source = {}\nvia = {}\nbytes = {}\n{}",
            address,
            access.wire.as_str(),
            content.len(),
            content
        );
        ActionOutcome::success(vec![ContentBlock::new(CAPABILITY_FETCH, reading)])
    }
}

impl ActionPort for WebApp {
    fn capabilities(&self, _agent: &AgentId, _run: &RunId) -> Capabilities {
        self.own_capabilities()
    }

    /// 局部准入：两条动词一律放行——它们都是**只读**，本层没有要判的东西。
    fn admit(&self, _agent: &AgentId, _run: &RunId, action: &Action) -> ActionAdmission {
        match action.id().as_str() {
            CAPABILITY_SEARCH | CAPABILITY_FETCH => ActionAdmission::Admitted,
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
            CAPABILITY_SEARCH => self.search(action.params()),
            CAPABILITY_FETCH => self.fetch(action.params()),
            _ => refused(WebFailure::InvalidArgument),
        }
    }
}

impl AppPort for WebApp {
    fn app_id(&self) -> &str {
        WEB_APP_ID
    }

    fn description(&self) -> &str {
        "外面的那个世界：查（搜一次，回来看结果列表）与取（把一个网页的正文取回来）。两条都是只读；取的时候有五条机械限制（只取不写、单次有界、有超时、重定向有上限且查环、私网地址一律拒绝）。它看的是谁都能看的东西，与只读文件视图那个「宿主授权的资料目录」不是一回事"
    }

    fn declared_capabilities(&self) -> Capabilities {
        self.own_capabilities()
    }

    /// 两条动词都是**只读**。
    ///
    /// 显式说出来而不是靠默认值：默认也是只读（`EffectClass::ReadOnly` 是缺省类别），但"我们这两条
    /// 不改变世界"是一件**有内容**声明，写出来它才可能被人看见与被检验；它们也不需要任何复核
    /// （复核是写效应的配对物，读没有可配的东西）。
    fn declared_effects(&self) -> Vec<EffectDeclaration> {
        vec![
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_SEARCH),
                EffectClass::ReadOnly,
                None,
            ),
            EffectDeclaration::new(ActionId::new(CAPABILITY_FETCH), EffectClass::ReadOnly, None),
        ]
    }

    /// 来源档 = **这一段回执里携带的内容从哪个口子进来**。
    ///
    /// 两条动词都是主体把外面的东西**拉回来**：搜索结果与页面正文都是**世界写的、没有署名**的
    /// 文本（一个地址不算署名——它只说"从哪取的"，不说"谁写的"）。因此两条都是
    /// [`crate::app_contract::SourceClass::WorldUnsigned`]。
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

/// 这一族失败的统一出口（分类名进回执，Core 只搬运）。
fn refused(failure: WebFailure) -> ActionOutcome {
    ActionOutcome::failure(failure.as_str())
}

/// 传输层失败的分类：超时与"连不上"同属一族，但**分开说**（主体能看出来是等太久还是压根没到）。
fn classify_transport(error: &reqwest::Error) -> WebFailure {
    if error.is_timeout() {
        WebFailure::Timeout
    } else {
        WebFailure::Unreachable
    }
}

/// **第二堵墙**：有界地读回执；超过上限就**拒绝**（不截断、不静默丢弃）。
fn bounded_text(response: reqwest::blocking::Response, limit: usize) -> Result<String, WebFailure> {
    let mut body = Vec::new();
    // 多读一个字节：正好用它分辨"刚好等于上限"与"超了"。
    let mut reader = response.take((limit + 1) as u64);
    if reader.read_to_end(&mut body).is_err() {
        return Err(WebFailure::Unreachable);
    }
    if body.len() > limit {
        return Err(WebFailure::TooLarge);
    }
    Ok(String::from_utf8_lossy(&body).to_string())
}

/// 回执折出来的一行（两个方言折成同一个形状，渲染只有一处）。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    title: String,
    address: String,
    summary: String,
    date: String,
}

/// 把搜索回执渲染成主体的读数（机械风格：计数 + 每行四段）。
///
/// **截断只落在展示栏上**（标题 / 摘要 / 日期）；**地址不截断**——它是下一跳的把手，见模块文档。
/// 地址超过 [`MAX_ADDRESS_BYTES`] 的那一行**不列**（不剪成半截），首行的 `addresses_over_limit` 如实
/// 说有几行：少一行比给一个假地址诚实，其余几行照旧可用。
fn render_rows(rows: &[Row], truncated: bool) -> String {
    let mut listed: Vec<&Row> = Vec::new();
    let mut over_limit = 0usize;
    for row in rows {
        if one_line(&row.address).len() > MAX_ADDRESS_BYTES {
            over_limit += 1;
            continue;
        }
        if listed.len() < MAX_RESULTS {
            listed.push(row);
        }
    }
    let mut out = format!(
        "results = {}, truncated = {}, addresses_over_limit = {}\n",
        listed.len(),
        truncated,
        over_limit
    );
    for row in listed {
        out.push_str(&format!(
            "{} | {} | {} | {}\n",
            truncate_field(&row.title, MAX_FIELD_BYTES),
            one_line(&row.address),
            truncate_field(&row.summary, MAX_FIELD_BYTES),
            truncate_field(&row.date, MAX_DATE_BYTES),
        ));
    }
    out
}

/// 回执里某一栏的原样文本；缺这一栏就是 `-`。
///
/// 压成一行不是修饰、是版式防线：对面塞一个换行就能伪造出第五段——那件事在渲染那一处做，
/// 这里只保证"缺"与"空"都不丢格子。
fn cell(row: &serde_json::Value, name: &str) -> String {
    match row.get(name).and_then(|value| value.as_str()) {
        Some(value) => value.to_string(),
        None => "-".to_string(),
    }
}

/// 方言一：`results[]` 列表（每行的四栏都由对面直接给）。
fn parse_results_json(body: &serde_json::Value) -> Result<Vec<Row>, WebFailure> {
    let rows = body
        .get("results")
        .and_then(|value| value.as_array())
        .ok_or(WebFailure::ServerError)?;
    Ok(rows
        .iter()
        .map(|row| Row {
            title: cell(row, "title"),
            address: cell(row, "url"),
            summary: cell(row, "content"),
            date: cell(row, "published_date"),
        })
        .collect())
}

/// 方言二：搜索接口的 `results[]`。
///
/// 摘要取 `summary`（对面写明是**给模型读**的长摘要），缺了退回 `snippet`（短摘要）——两格都没有
/// 才是 `-`。⚠ 这两格是**同一件事的两种长度**，不是两次猜测：顺序写在这里，不散在别处。
/// 按要求，`results` 是空数组是**正常的**（没匹配到），不是失败。
fn parse_search_api(body: &serde_json::Value) -> Result<Vec<Row>, WebFailure> {
    let rows = body
        .get("results")
        .and_then(|value| value.as_array())
        .ok_or(WebFailure::ServerError)?;
    Ok(rows
        .iter()
        .map(|row| Row {
            title: cell(row, "title"),
            address: cell(row, "url"),
            summary: first_text(row, &["summary", "snippet"]),
            date: cell(row, "date"),
        })
        .collect())
}

/// 依次取第一格**非空**的文本栏；都缺 ⇒ `-`。
fn first_text(row: &serde_json::Value, names: &[&str]) -> String {
    for name in names {
        if let Some(text) = row
            .get(*name)
            .and_then(|value| value.as_str())
            .filter(|text| !text.trim().is_empty())
        {
            return text.to_string();
        }
    }
    "-".to_string()
}

/// 一次请求的共同收口：传输分类 → 状态分类 → 有界正文 → JSON。
///
/// **一次调用恰好一次出网请求**（零重试）：这里不重发，失败就如实交出去。
fn read_json(
    response: Result<reqwest::blocking::Response, reqwest::Error>,
) -> Result<serde_json::Value, WebFailure> {
    let response = response.map_err(|error| classify_transport(&error))?;
    let status = response.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(WebFailure::CredentialRejected);
    }
    if !status.is_success() {
        return Err(WebFailure::ServerError);
    }
    let text = bounded_text(response, MAX_PAGE_BYTES)?;
    serde_json::from_str(&text).map_err(|_| WebFailure::ServerError)
}

/// 外来文本压成一行（换行 / 回车 / 制表符变空格，去首尾空白）。
fn one_line(text: &str) -> String {
    text.replace(['\n', '\r', '\t'], " ").trim().to_string()
}

/// 展示栏的机械截断（按字节、落在字符边界上，超出加省略号）。
fn truncate_field(text: &str, cap: usize) -> String {
    let text = one_line(text);
    if text.len() <= cap {
        return text;
    }
    let mut cut = cap;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &text[..cut])
}
