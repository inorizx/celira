//! web 窗：主体的信息网（`web_search` + `web_fetch`）。
//!
//! 模型的知识边界停在训练截止。本件给主体**两条只读的出网动词**——**查**（搜索厂商）与**取**
//! （直连目标页）——于是"今天发生了什么"这类主体本来答不出来的事，主体能自己去问、自己去看。
//!
//! ## 两条动词，两个对面
//!
//! | 动词 | 对面是谁 | 凭据 | 回来的是什么 |
//! |---|---|---|---|
//! | `web_search` | **搜索厂商**（端点是**配置**：换厂商＝换配置与凭据） | 宿主声明的凭据；没声明 ⇒ `credential-absent`（**没配**，不是被拒），**动词照旧在场** | ≤5 条结果，每条一行：标题 / 地址 / 摘要 |
//! | `web_fetch` | **目标页本身**（没有厂商） | 不需要 | 有界纯文本（网页先做**机械抽取**） |
//!
//! **厂商方言锁在本件内**：主体的词永远是这两个；换一家搜索厂商，Agent 的能力面一个字节都不变。
//!
//! ## 五堵机械墙（都只作用在 `web_fetch` 上）
//!
//! 出网窗的风险全在机械面，所以这里**不设授权**（两动词都是只读：取东西不改变世界），
//! 只钉机械边界：
//!
//! 1. **只取不写**：只接受两种页面协议（明文那一种与加密那一种），一律用 `GET`；别的协议本地拒绝；
//! 2. **单次 ≤ 64 KiB**：超限**拒绝、不截断**（与 `files_read` 先例同一条纪律：宁可让主体换一个更小的
//!    目标，也不要给主体半截而以为什么都拿到了）；
//! 3. **单次请求 30 秒操作窗口**：连接/读/写各自超时即失败，不无限等；
//! 4. **重定向 ≤5 跳、且查环**：每一跳都重新过一遍地址检查（不然一个重定向就能绕过第 5 堵墙）；
//! 5. **解析后的地址在私网就拒绝**：这是**出网窗**，不是回家里的门——回路、内网、链路本地一律拒绝。
//!
//! ## 失败词汇（本件自己的封闭表）
//!
//! `transport-unreachable` / `timeout` / `credential-absent` / `credential-rejected` / `invalid-argument` /
//! `wrong-kind` / `too-large` / `redirect-loop` / `private-address-refused` / `server-error`。
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
use std::net::{IpAddr, ToSocketAddrs};
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

/// 结果里每一栏（标题 / 地址 / 摘要）的机械截断上限。
pub const MAX_FIELD_BYTES: usize = 200;

/// 单次取页的字节上限：超限**拒绝**，不截断。
pub const MAX_PAGE_BYTES: usize = 64 * 1024;

/// 一次请求的操作窗口（连接 / 读 / 写各自的墙钟上限，不是整条流的总时长）。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// 跟随重定向的上限（第 6 跳即判环）。
const MAX_REDIRECTS: usize = 5;

/// 只接受的两种页面协议（外部冻结的协议名：它们是标准里定下来的字面量，不是我们的词）。
const ALLOWED_SCHEMES: [&str; 2] = ["http", "https"];

/// 本件自己的失败词汇表（封闭九项）。
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
    /// `Soul::Absent` / `Soul::Unreadable` 同一条三态纪律。**动词照旧在场**：缺的是这一格，
    /// 不是这个能力。
    CredentialAbsent,
    /// 参数不合法（空查询、超长、地址读不懂、协议不在允许的两种里）。
    ///
    /// 这一族的共同点是**在本地就判掉了**：一次出网请求都不发。
    InvalidArgument,
    /// 对面给的不是文本（图片、压缩包、二进制……）。本件只搬文本。
    WrongKind,
    /// 超过了单次的字节上限（**拒绝**，不截断）。
    TooLarge,
    /// 重定向成了环，或者跳数超过上限。
    RedirectLoop,
    /// 解析后的地址落在私网（回路、内网、链路本地）：出网窗不指向家里。
    PrivateAddressRefused,
    /// 对面回了错误状态，或者回执读不懂。**不分解 4xx / 5xx**（见模块文档）。
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
            WebFailure::WrongKind => "wrong-kind",
            WebFailure::TooLarge => "too-large",
            WebFailure::RedirectLoop => "redirect-loop",
            WebFailure::PrivateAddressRefused => "private-address-refused",
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
            "wrong-kind" => WebFailure::WrongKind,
            "too-large" => WebFailure::TooLarge,
            "redirect-loop" => WebFailure::RedirectLoop,
            "private-address-refused" => WebFailure::PrivateAddressRefused,
            "server-error" => WebFailure::ServerError,
            _ => return None,
        })
    }
}

/// 宿主给本件的**声明**：搜索厂商的端点 + 凭据。
///
/// 两样都由宿主显式给出（端点是配置、凭据来自环境变量），本件自己不去找、不配置、不落盘。
/// 凭据只被写进请求头；本类型**不派生 `Debug`**，因此它不可能被顺手打进日志或报告。
///
/// 缺这一份声明不是"装不了"：`web_search` 照旧在 Agent 的能力面里，只是**执行时如实说没有凭据**
/// （`credential-rejected`）；`web_fetch` 与厂商无关，照常可用。
#[derive(Clone)]
pub struct SearchAccess {
    endpoint: String,
    credential: String,
}

impl SearchAccess {
    /// 用端点与凭据建立一份声明；空值一律拒绝（"没配好"不是一种运行状态）。
    pub fn new(endpoint: impl Into<String>, credential: impl Into<String>) -> Option<Self> {
        let endpoint = endpoint.into();
        let credential = credential.into();
        if endpoint.trim().is_empty() || credential.trim().is_empty() {
            return None;
        }
        Some(Self {
            endpoint,
            credential,
        })
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
    /// **被点名担保**的私网主机（生产：空 = 一律拒绝）。
    ///
    /// 它是一份**清单**而不是一个开关，理由是：部署如果要取内网页，那也是取**它点过名的**那几台；
    /// 而重定向把方向盘交给了对面——**每一跳都按同一份清单检查**，所以一个公网页面把我们往
    /// 清单之外的地址（含内网）指，照样被拒。空白清单（生产）意味着"任何私网地址、任何一跳都拒"。
    pub allowed_private_hosts: Vec<String>,
}

impl WebPolicy {
    /// 生产策略：30 秒窗口、私网一律拒绝（清单为空）。
    pub fn production() -> Self {
        Self {
            request_timeout: REQUEST_TIMEOUT,
            allowed_private_hosts: Vec::new(),
        }
    }
}

/// **出网档**（2026-10-13）：这台机器**给不给它出网窗**，由操作者在装配面声明——形状照
/// `--terminal` 的先例（封闭枚举、不认识的档位不降级、不预建更细的档：加档是加墙）。
///
/// 判据一句话：**"主体能不能出网"是主人的政策**——既不是 App 的属性，也不是"配置"。
/// 它与 terminal 的档位是同一类事实（`--terminal` 不给就没有那只手）。
///
/// ⚠ **`Off` 的含义是"不装配这件 App"**：主体的门里连 `web` 这扇门都没有（不是"有门但打不开"）。
/// 它由装配面消费——[`WebApp::with_tier`] 收到 `Off` 直接返回 `None`，装配面据此不摆这件。
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
    /// 搜索厂商的声明（可缺——缺了只有那一条动词失败，别的照旧）。
    search: Option<SearchAccess>,
    /// 请求客户端。**不跟随重定向**：每一跳都要我们自己过一遍（见模块文档第 4 堵墙）。
    client: reqwest::blocking::Client,
    /// 操作旋钮（生产 = `WebPolicy::production()`）。
    policy: WebPolicy,
    /// **这一台机器给到哪一档**（见 [`WebTier`]）。它只影响**声明面**（「查」在不在场），
    /// 不影响「取」——取页不依赖厂商、也不花搜索那份钱。
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

    /// **按出网档装配**（生产走这一条）：`Off` ⇒ `None`（**不摆这件 App**——主体的门里没有它）。
    pub fn with_tier(
        search: Option<SearchAccess>,
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
            client,
            policy,
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
                    "在网上搜一次（最多 {MAX_RESULTS} 条结果，每条一行「标题 | 地址 | 摘要」）"
                ),
                vec![ParamSpec::new(
                    PARAM_QUERY,
                    true,
                    format!("查询词（非空、≤{MAX_QUERY_BYTES} 字节；超限本地拒绝、不出网）"),
                )],
            )
            .with_details(format!(
                "读数形状：每行三段——标题、地址、摘要；每栏最多 {MAX_FIELD_BYTES} 字节，超出即\
                 机械截断（带 `…`）。首行给出取回条数与 `truncated` 标记。要读某一条的全文，把它那一行的\
                 **地址原样**交给 `{CAPABILITY_FETCH}`。**只读**：不改变任何东西。**凭据**：搜索凭据由宿主\
                 声明——没声明时这个动词**照旧在场**，执行时如实失败 `credential-absent`（意思是**这台\
                 机器还没给它配**，不是对面拒绝了；对面拒了是 `credential-rejected`）。",
            )));
        }
        items.push(Capability::new(
            ActionId::new(CAPABILITY_FETCH),
                "取一个网页的正文（回来是纯文本；网页会先做机械抽取）",
                vec![ParamSpec::new(
                    PARAM_TARGET,
                    true,
                    "要取的地址（明文或加密那两种页面协议之一；原样交给本动作，不解析、不改写）",
                )],
            )
            .with_details(format!(
                "回来的形状：这一页的纯文本（网页先做机械抽取：去掉脚本、样式与标记，一字不改写原意）\
                 ——首行给出取自哪里与字节数。**五条机械限制**：① 只接受明文与加密这两种页面协议、只读不写；\
                 ② 单次最多 {} KiB（**超限拒绝**，不截断）；③ 一次 30 秒；④ 重定向最多 {MAX_REDIRECTS} 跳\
                 且不绕环；⑤ **指向私网（回路/内网/链路本地）的地址一律拒绝**——这是出网窗，不是回家里的门。\
                 **只读**：不改变任何东西。",
                MAX_PAGE_BYTES / 1024
            )),
        );
        Capabilities::of(items)
    }

    /// 查一次（搜索厂商的方言锁在这里，外面的世界只认识 `web_search`）。
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
        let response = match response {
            Ok(response) => response,
            Err(error) => return refused(classify_transport(&error)),
        };
        let status = response.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return refused(WebFailure::CredentialRejected);
        }
        if !status.is_success() {
            return refused(WebFailure::ServerError);
        }
        let text = match bounded_text(response, MAX_PAGE_BYTES) {
            Ok(text) => text,
            Err(failure) => return refused(failure),
        };
        let body: serde_json::Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => return refused(WebFailure::ServerError),
        };
        match render_results(&body) {
            Some(reading) => {
                ActionOutcome::success(vec![ContentBlock::new(CAPABILITY_SEARCH, reading)])
            }
            None => refused(WebFailure::ServerError),
        }
    }

    /// 取一个页面（没有厂商：直连目标本身）。
    fn fetch(&self, params: &ActionParams) -> ActionOutcome {
        let raw = match params.get(PARAM_TARGET) {
            Some(raw) => raw.trim(),
            None => return refused(WebFailure::InvalidArgument),
        };
        let mut target = match reqwest::Url::parse(raw) {
            Ok(target) => target,
            Err(_) => return refused(WebFailure::InvalidArgument),
        };
        if !allowed_scheme(&target) {
            return refused(WebFailure::InvalidArgument);
        }
        let mut seen: Vec<String> = Vec::new();
        let mut hops = 0usize;
        loop {
            let here = target.as_str().to_string();
            if seen.contains(&here) {
                return refused(WebFailure::RedirectLoop);
            }
            seen.push(here);
            // 每一跳都过同一份检查：被担保的私网主机之外，任何私网地址都不许去。
            if let Err(failure) = ensure_public(&target, &self.policy) {
                return refused(failure);
            }
            let response = match self.client.get(target.clone()).send() {
                Ok(response) => response,
                Err(error) => return refused(classify_transport(&error)),
            };
            let status = response.status();
            if status.is_redirection() {
                hops += 1;
                if hops > MAX_REDIRECTS {
                    return refused(WebFailure::RedirectLoop);
                }
                let location = match response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                {
                    Some(location) => location,
                    None => return refused(WebFailure::ServerError),
                };
                let next = match reqwest::Url::parse(location) {
                    Ok(next) => next,
                    // 相对地址按当前地址解析（这是唯一一处"读懂对面"的地方，仍然是机械的）。
                    Err(_) => match target.join(location) {
                        Ok(next) => next,
                        Err(_) => return refused(WebFailure::ServerError),
                    },
                };
                if !allowed_scheme(&next) {
                    return refused(WebFailure::InvalidArgument);
                }
                target = next;
                continue;
            }
            if !status.is_success() {
                return refused(WebFailure::ServerError);
            }
            let kind = match content_kind(&response) {
                Some(kind) => kind,
                None => return refused(WebFailure::WrongKind),
            };
            let text = match bounded_text(response, MAX_PAGE_BYTES) {
                Ok(text) => text,
                Err(failure) => return refused(failure),
            };
            let body = match kind {
                ContentKind::Plain => text,
                ContentKind::Markup => extract_text(&text),
            };
            let reading = format!(
                "source = {}\nbytes = {}\n{}",
                target.as_str(),
                body.len(),
                body
            );
            return ActionOutcome::success(vec![ContentBlock::new(CAPABILITY_FETCH, reading)]);
        }
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

/// 地址的协议在允许的两种里。
fn allowed_scheme(target: &reqwest::Url) -> bool {
    ALLOWED_SCHEMES.contains(&target.scheme())
}

/// **第五堵墙**：地址必须都在公网。
///
/// 名字解析不出来 ⇒ 传输失败（连不上）；只要有一个解析结果是私网就拒绝（保守面：宁可拒绝一个
/// 多地址的主机，也不要让它有可能落在家里）。
///
/// **IP 字面量按解析判，不交给解析器**：`host_str()` 对 IPv6 字面量给的是**带方括号**的串
/// （`[::1]` 这种写法），把那一串喂给 `to_socket_addrs` 在 Linux 上解析不了（Windows 的
/// getaddrinfo 宽容，能认）⇒ 同一个私网地址在两边落进**不同的格子**：Windows 上
/// `private-address-refused`，Linux 上 `transport-unreachable`。拒绝都发生了，但**分类错了**
/// ——而分类正是这堵墙的读数。判据：**字面量先 `IpAddr::parse`，只有域名才去解析**
/// （顺带少一次无谓的解析）。
///
/// vouch 也按**规范化后的主机文本**比（IPv6 去掉方括号），否则清单里写 `::1` 认不出带方括号的写法。
fn ensure_public(target: &reqwest::Url, policy: &WebPolicy) -> Result<(), WebFailure> {
    let host = target.host_str().ok_or(WebFailure::InvalidArgument)?;
    let literal = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    let vouched = policy
        .allowed_private_hosts
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(literal));
    let port = target
        .port_or_known_default()
        .ok_or(WebFailure::InvalidArgument)?;
    // 字面量：直接分类（不解析）。
    if let Ok(address) = literal.parse::<IpAddr>() {
        if !vouched && is_private(address) {
            return Err(WebFailure::PrivateAddressRefused);
        }
        return Ok(());
    }
    // 域名：解析后逐个分类。
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|_| WebFailure::Unreachable)?;
    let mut resolved = 0usize;
    for address in addresses {
        resolved += 1;
        if !vouched && is_private(address.ip()) {
            return Err(WebFailure::PrivateAddressRefused);
        }
    }
    if resolved == 0 {
        return Err(WebFailure::Unreachable);
    }
    Ok(())
}

/// 这个地址算不算"家里"（保守面：回路、私网、链路本地、未指定、广播、文档网段、组播都算）。
fn is_private(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || octets[0] == 0
                || (octets[0] == 100 && (64..128).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                || v6
                    .to_ipv4_mapped()
                    .map(|v4| is_private(IpAddr::V4(v4)))
                    .unwrap_or(false)
        }
    }
}

/// 对面给的这一份是什么（本件只搬文本）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContentKind {
    /// 纯文本一类的：原样搬。
    Plain,
    /// 标记语言那一类：先做机械抽取。
    Markup,
}

/// 按对面声明的类型分类；**说不清就拒绝**（不猜）。
fn content_kind(response: &reqwest::blocking::Response) -> Option<ContentKind> {
    let declared = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    // 只取分号之前那一段（参数如字符集不参与分类），大小写不敏感。
    let media = declared
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if media == "text/html" || media == "application/xhtml+xml" {
        Some(ContentKind::Markup)
    } else if media.starts_with("text/") {
        Some(ContentKind::Plain)
    } else {
        None
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

/// 把搜索厂商的回执渲染成主体的读数（机械风格：计数 + 每行三段，三栏都截断）。
fn render_results(body: &serde_json::Value) -> Option<String> {
    let rows = body.get("results")?.as_array()?;
    let mut out = format!(
        "results = {}, truncated = {}\n",
        rows.len().min(MAX_RESULTS),
        rows.len() > MAX_RESULTS
    );
    for row in rows.iter().take(MAX_RESULTS) {
        let field = |name: &str| -> String {
            match row.get(name).and_then(|value| value.as_str()) {
                Some(value) => truncate_field(value),
                None => "-".to_string(),
            }
        };
        out.push_str(&format!(
            "{} | {} | {}\n",
            field("title"),
            field("url"),
            field("content")
        ));
    }
    Some(out)
}

/// 结果里一栏的机械截断（按字节、落在字符边界上，超出加省略号）。
fn truncate_field(text: &str) -> String {
    let text = text.replace(['\n', '\r', '\t'], " ");
    let text = text.trim();
    if text.len() <= MAX_FIELD_BYTES {
        return text.to_string();
    }
    let mut cut = MAX_FIELD_BYTES;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &text[..cut])
}

/// **机械抽取**：把标记语言那一类变成可读文本。
///
/// 只做机械变换，**不摘要、不改写、不推断**：丢掉脚本与样式整段、丢掉标记本身、把块级标记的
/// 位置换成换行（不然一整页会挤成一行）、解码几个最常见的实体、把连续空白压成一个空格。
/// 它是 best-effort——抽出来的形状入交付报告，语义改写才是违禁的那件事。
fn extract_text(markup: &str) -> String {
    let mut out = String::with_capacity(markup.len());
    let mut rest = markup;
    while let Some(open) = rest.find('<') {
        push_text(&mut out, &rest[..open]);
        let after = &rest[open..];
        let end = match after.find('>') {
            Some(offset) => offset + 1,
            None => {
                // 没有收口：剩下的当标记整段丢掉（不与它猜）。
                break;
            }
        };
        let tag = &after[..end];
        match skipped_element(tag) {
            Some(name) => {
                // 脚本/样式：整段丢，直到它的收口（找不到就当丢到页尾）。
                let tail = &after[end..];
                let lowered = tail.to_ascii_lowercase();
                let close = format!("</{name}");
                let stop = match lowered.find(&close) {
                    // 收口位置要从**这一段的起点**数：先找到 `</style`，再找到它的那个 `>`。
                    Some(outer) => match tail[outer..].find('>') {
                        Some(inner) => end + outer + inner + 1,
                        None => after.len(),
                    },
                    None => after.len(),
                };
                rest = &after[stop..];
                continue;
            }
            None => {
                if is_block_tag(tag) {
                    push_break(&mut out);
                }
                rest = &after[end..];
            }
        }
    }
    push_text(&mut out, rest);
    collapse(&out)
}

/// 这个标记是不是"整段跳过"的元素（脚本与样式）：回它的元素名。
fn skipped_element(tag: &str) -> Option<&'static str> {
    let name = tag_name(tag)?;
    ["script", "style"]
        .into_iter()
        .find(|candidate| name == *candidate)
}

/// 这个标记是不是块级（它结束就意味着"一段完了"）：换行。
fn is_block_tag(tag: &str) -> bool {
    const BLOCK: [&str; 22] = [
        "p",
        "div",
        "section",
        "article",
        "header",
        "footer",
        "main",
        "aside",
        "nav",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "li",
        "tr",
        "table",
        "ul",
        "ol",
        "blockquote",
        "pre",
    ];
    let Some(name) = tag_name(tag) else {
        return false;
    };
    if name == "br" || name == "hr" {
        return true;
    }
    BLOCK.contains(&name.as_str())
}

/// 从标记里取出元素名（小写；`</p>` 与 `<p class="x">` 都给 `p`）。
fn tag_name(tag: &str) -> Option<String> {
    let inner = tag
        .trim_start_matches('<')
        .trim_start_matches('/')
        .trim_start_matches('!');
    let name: String = inner
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric())
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name.to_ascii_lowercase())
    }
}

/// 文本片段：解码几个最常见的实体后照抄。
fn push_text(out: &mut String, text: &str) {
    if text.is_empty() {
        return;
    }
    let decoded = text
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    out.push_str(&decoded);
}

/// 一个"段完了"的记号（连续多个只算一个）。
fn push_break(out: &mut String) {
    if !out.ends_with('\n') {
        out.push('\n');
    }
}

/// 把连续空白压成一个空格，并去掉行首尾空白与多余空行。
fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    let mut pending_break = false;
    for ch in text.chars() {
        if ch == '\n' {
            pending_break = true;
            continue;
        }
        if ch.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_break {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            pending_break = false;
        } else if pending_space && !out.is_empty() && !out.ends_with('\n') {
            out.push(' ');
        }
        pending_space = false;
        out.push(ch);
    }
    out.trim().to_string()
}
