//! MCP 适配的 **App 那一半**：动词面、卫生、参数还原、回执墙。
//!
//! 一台 server 暴露的动词进我们的动词面要过三道机械处理，全部在这里：
//!
//! 1. **名字**：`<server 名>_<对面原名>`；越界（超过工具名上限）**整台不上场**——
//!    名字越界不是"那一条不好用"，是每次请求都会在出口被拒；
//! 2. **骨架**：对面的说明是**外来文本**（控制字符、超长都是它的自由），因此一律消毒 + 截断
//!    + 带标记；对面没给说明就如实说没给（不编一句）；
//! 3. **参数表**：对面的 `inputSchema` 机械地读成我们的参数表。读得懂的是**两类**：
//!    标量（字符串 / 整数 / 小数 / 真假），以及**对象与数组**——后者内部按**一段 JSON 文本**搬，
//!    转发之前还原成真正的 JSON 值（[`JsonKind::Json`]）。
//!    读不出来的（`null`、认不出的类型名、**两种不同类型拼起来的联合**）**不跳过、不猜**：
//!    那条动词照旧在场，但**调用时本地回绝**（`unsupported-schema`），并在详情里说清是哪一格
//!    读不出来。理由与 `web_search` 缺凭据同款——让"有这条但用不了"看得见，比让它安静地消失诚实。
//!
//!    ⚠ 结构型参数还在**详情卡**里给一句**形状说明**（形如 `{status:串,limit:数}`）：
//!    只说"这是一段 JSON 文本"而不说里面有什么键，读的人只能猜、只能试。
//!    它进的是**详情**（按需取），**不进骨架**（每一轮都在前缀里）。
//!
//! ## 资源数据面：两个通用只读动作（2026-10-25）
//!
//! 对面在握手时声明 `resources`，展开后动词面里就多两条**合成**动作：
//! `<名>_resources_list`（可选 `cursor`）与 `<名>_resources_read`（必填 `uri`）。
//! 它们不是对面的工具：不进 [`map_tools`] 的清单检查，名字与真实工具冲突时**让位**给对面
//! （见 [`resource_specs`]）。两条都是 [`McpEffect::ReadOnly`]——**与宿主给整台的效果档无关**：
//! 读资源改变不了外面的世界，`,`read-only` 那句背书说的是工具。
//!
//! `resources_list` 只回元数据与下一页游标；`resources_read` 才取正文（文本有界、blob 不搬原文）。
//! 正文**不写回上下文**：它只出现在那一次调用的回执里，App 自己不留副本。
//!
//! ## 转发出去了、答复回来了就是成功
//!
//! 对面的 `isError` 是**读数**（工具自己说"这事没成"），不是我们的失败分类。这与 terminal 的
//! 退出码同款：**跑起来了、跑完了就是成功**。对面回**错误对象**时同样如此——那一次交换完成了，
//! 我们归一个类（`invalid-argument` / `server-error`）当读数报出来，原文照旧在主体的回执里。
//!
//! 只有**我们自己的机械故障**才落进失败/未知：本地参数不合法、窗口到点、通道断了、压根没跑起来。
//! 其中"写出去之后通道没了"（写档）落进 [`ActionOutcome::unknown`]——**效果不明**这一态
//! （只读档没有这一格：读不改变世界，没有"可能已经发生"）。⚠ 这里原来写的是"那是未决效果门要接
//! 的第三态"：**门已退役**（门是指令，而世界只该给信息），这一态本身照旧如实报，只是不再有人按它
//! 下指令。
//!
//! ## 可选旋钮：逐动词的 `timeout`
//!
//! 对面自己没有叫 `timeout` 的参数时，我们给这条动词挂一个可选旋钮：这次等多久（1–<上限> 秒，
//! 缺省取声明的窗口）。**对面占了这个名字就不挂**——那样我们的旋钮和它的参数会撞在同一个名字上，
//! 而"参数顺序就是语义"这条纪律下，撞名没有正确答案。碰不上的时候详情里会说清。
//!
//! ## 参数拒绝：分类是一个词，诊断贴在它旁边
//!
//! 本地参数拒绝的分类只有 `invalid-argument` 一个词（逐字节稳定，词表一行不加）；**哪一项错了、
//! 期望什么形状**是它**旁边**的读数（[`ArgumentRefusal`] → `parameters` 那一块）。
//! 它与 `os.rs` 的 `ParamShape` 是**同一个病的两半**：那边治**名字**（多给了 / 缺了），这边治
//! **值**（还原不成对面要的类型）。两半共用的是**词**与那条纪律，不是实现。
//!
//! ⚠ **这一格缺的从来不是"接不住结构"**：对象与数组早在 [`JsonKind::Json`] 那一格里按一段
//! 文本搬、转发前还原（形状说明进详情卡），值本来就是字符串（[`ActionParams`] 只装标量）。
//! 从前那四种本地拒绝塌成一个分类，缺的是**值级诊断**——按"接不住结构"去开刀会做错事。

use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::app_contract::{
    AppPort, EffectDeclaration, ExpansionRefusal, SourceClass, SourceDeclaration,
};
use crate::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Capabilities,
    Capability, ContentBlock, ParamSpec,
};
use crate::foundation::{AgentId, OperationId, RunId};

use super::{
    CallFault, CallReply, Lease, MAX_PARAM_DOC_BYTES, MAX_SKELETON_BYTES, MAX_TEXT_BYTES,
    MAX_TIMEOUT_SECONDS, McpAccess, McpEffect, McpEndpoint, McpFailure, McpNotification,
    PARAM_TIMEOUT, RawTool, ResourceBody, ResourceContent, ResourcePage, RmcpTransport, Transport,
    clip, fault_outcome, foreign_line, sanitize,
};

/// 形状说明里最多列几个键；多的**如实报数**，不静默丢。
///
/// 预算见 [`MAX_PARAM_DOC_BYTES`]（120 字节），而对面自己那句话还要挤进同一个格子。
const MAX_SHAPE_KEYS: usize = 8;

/// **免查**的字节预算：参数表压成一行之后最多这么长（判据在 [`ToolSpec::capability`]）。
///
/// 免查省掉"先看格式"那一步，代价是目录行变长——而目录行**每一轮都在**前缀里。
/// ⚠ 预算之所以**必须**存在：`params_line()` 会把**对面那句话**也渲染进去
/// （`名 = 说明（必填）`），而那是**外来文本**——参数一多就必然超。
/// 超了就**老实走"先看格式"**：多一个回合，胜过目录里塞一段读不完的话。
const EXEMPT_PARAMS_BUDGET: usize = 120;

/// 资源数据面那两个通用动作的后缀（动词名 = `<server>_<后缀>`）。
///
/// 它们**不是**对面上报的工具名：对面若真的报了同名工具，那两个合成动作会**跳过**
/// （见 [`resource_specs`]）——工具名归对面，我们不能把它的动词改个名字。
const SUFFIX_RESOURCES_LIST: &str = "resources_list";
const SUFFIX_RESOURCES_READ: &str = "resources_read";

/// 资源清单的分页参数名。
const PARAM_CURSOR: &str = "cursor";

/// 资源读取的 URI 参数名。
const PARAM_URI: &str = "uri";

/// 通道报告的那两行洗凭据读数：`(洗掉几个, 洗不掉的键名)`；`None` = 这条通道不起子进程。
///
/// 用具名类型收一下：它同时出现在调用返回、三种回执渲染与 `push_scrub` 上，
/// 原样写出来会是一串括号（也过不了 clippy 的 `type_complexity`）。
type ScrubReading = Option<(usize, Vec<String>)>;

/// 一条动词面条目的**来源**。
///
/// 三种条目共用一个结构（参数表、骨架、详情、效果档），但**调用形状不同**：
/// 工具走 `tools/call` 转发，资源走 `resources/list` / `resources/read` 两个通用动作。
/// 它也是"每条动作带自己的效果"的落点——见 [`ToolSpec::effect`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpecKind {
    /// 对面清单里的一条工具（名字与参数表都来自对面）。
    Tool,
    /// 合成的只读动作：列资源（不是对面的一条工具）。
    ResourcesList,
    /// 合成的只读动作：读一条资源（不是对面的一条工具）。
    ResourcesRead,
}

/// 一条动词的**还原类型**：我们的参数全是字符串，转发之前要按对面声明的类型还原回去。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JsonKind {
    /// 字符串：原样（**不 trim**：空白可能是它的内容）。
    Text,
    /// 整数。
    Integer,
    /// 小数。
    Number,
    /// 真假。
    Flag,
    /// **一段 JSON 文本**：对面声明的是对象或数组，我们内部仍按字符串搬，
    /// 转发之前用 `serde_json::from_str` 还原成真正的 JSON 值。
    ///
    /// ⚠ 这一格不是"我们支持了对象参数"，是「**我们不再要求对面符合我们的形状**」：
    /// 中立参数容器（`ActionParams`）只装标量，而**对面的参数形状不是我们定的**。
    /// 从前那种形状让**整条动词调不了**——那是拿我们的手型去量对面的手。
    ///
    /// 代价如实登记：参数要给成**一段 JSON 文本**（`"{\"status\":\"open\"}"`），**转义易错**。
    /// 缓解在形状说明（见 [`shape_of`]）——给例子比给语法便宜。
    ///
    /// ⚠ **如实登记（诊断那一格）**：这一档今天仍只报"**还原不成**"——这段文本**具体错在哪**
    /// （解析器自己那句话）**不进诊断**：它是**外部解析器的文本**，要进就得先想清"它算谁的话"
    /// （与展开失败带对面原话同一条纪律的下一步），所以宁可先不写。
    Json,
}

/// **参数值**诊断那一块的标签：与 `os.rs` 的 `PARAM_SHAPE_LABEL` **同一个词**。
///
/// 两处贴的是同一类东西（参数出了什么问题），读的人看到的**是一个词**——
/// 不因为"是 OS 拦的还是 App 拦的"换一个说法。共用的是**词**，不是主张谁拥有它：
/// 拼法各写一份（`os.rs` 那份带"接受名单"、这份带"期望形状"），值不值得合并看第三个消费者。
const ARGUMENTS_LABEL: &str = "parameters";

/// 本地参数拒绝的**诊断**：哪个名字、为什么、期望什么形状。
///
/// 它治的是与 `os.rs::ParamShape` **同一个形状的洞**，只是落在**值**那一半上：
/// `build_arguments` 的四种拒绝（不认识的参数名 / 取值还原不成对面要的类型 / 必填没到齐 /
/// 同一个参数给了两次）从前一律塌成一个 `invalid-argument`——**过程留了、原因没留**
/// （哪一次调用、哪个名字都在轨迹里，为哪一类不在），**主体当场不知道自己错在哪一项**。
///
/// 与 `ParamShape` 同一条纪律：**分类是词表**（`invalid-argument` 逐字节稳定，词表一行不动），
/// **诊断是内容**（贴在那个分类旁边）。
///
/// ⚠ **不回显取值**（判据是 `AGENTS.md` 的"错误对外展示不得泄露凭据或敏感正文"）：
/// 主体本来就知道自己给了什么，回显只是把可能敏感的那一段抄进记录。给的是
/// **名字 ＋ 原因 ＋ 期望形状**——那才是"下一步怎么填"。
#[derive(Clone, Debug, PartialEq, Eq)]
struct ArgumentRefusal {
    /// 出问题的那个参数名（必填没到齐、或不认识的参数名，可能没有名字）。
    name: String,
    /// 机械原因（封闭的几个词，见 [`ArgumentRefusal::text`]）。
    reason: ArgumentRefusalReason,
    /// 期望的形状：`integer` / `object|array（一段 JSON 文本）` 一类。
    ///
    /// 它是 [`JsonKind`] 自己那句话；不适用（如不认识的参数名）时为空 ⇒ **不出场**。
    expected: String,
}

/// 本地参数拒绝的**机械原因**（封闭表——同一故障两次发生逐字节相同）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArgumentRefusalReason {
    /// 这个名字不在对面声明的参数表里。
    UnknownParameter,
    /// 取值还原不成对面声明的类型。
    WrongKind,
    /// 同一个参数给了两次。
    Duplicated,
    /// 对面声明为必填，而这一次没给。
    Missing,
}

impl ArgumentRefusalReason {
    /// 稳定词（封闭表；新增取值必须同时想清"这个取值在**哪一格**读得到"）。
    fn as_str(self) -> &'static str {
        match self {
            ArgumentRefusalReason::UnknownParameter => "unknown-parameter",
            ArgumentRefusalReason::WrongKind => "wrong-kind",
            ArgumentRefusalReason::Duplicated => "duplicated",
            ArgumentRefusalReason::Missing => "missing-required",
        }
    }
}

impl ArgumentRefusal {
    /// 一段机械诊断：一行名字与原因，期望形状有就带上。
    fn text(&self) -> String {
        let mut text = String::new();
        if !self.name.is_empty() {
            text.push_str("parameter = ");
            text.push_str(&self.name);
            text.push('\n');
        }
        text.push_str("reason = ");
        text.push_str(self.reason.as_str());
        if !self.expected.is_empty() {
            text.push_str("\nexpected = ");
            text.push_str(&self.expected);
        }
        text
    }
}

impl JsonKind {
    /// 这个形状**叫什么**（进诊断的"期望"那一格；给的是"下一步该怎么填"，不是语法课）。
    ///
    /// 与 [`shape_of`] 的分工：那一件给**例子**（详情卡里"形如 …"），这一件给**名字**
    /// （回执里"expected = …"）。两处都不替读的人猜，只说这一格该长什么样。
    fn shape_name(self) -> &'static str {
        match self {
            JsonKind::Text => "字符串",
            JsonKind::Integer => "整数",
            JsonKind::Number => "小数",
            JsonKind::Flag => "true 或 false",
            JsonKind::Json => "object|array（一段 JSON 文本）",
        }
    }

    /// 把我们的一个字符串取值还原成对面声明的类型；还原不出来就是本地参数不合法。
    ///
    /// 失败只说"**还原不出来**"——原因与期望形状由调用方贴成诊断（见 [`ArgumentRefusal`]）。
    fn coerce(self, raw: &str) -> Result<Value, McpFailure> {
        match self {
            JsonKind::Text => Ok(Value::String(raw.to_string())),
            JsonKind::Integer => raw
                .trim()
                .parse::<i64>()
                .map(Value::from)
                .map_err(|_| McpFailure::InvalidArgument),
            JsonKind::Number => {
                let number: f64 = raw
                    .trim()
                    .parse()
                    .map_err(|_| McpFailure::InvalidArgument)?;
                if !number.is_finite() {
                    return Err(McpFailure::InvalidArgument);
                }
                Ok(Value::from(number))
            }
            JsonKind::Flag => match raw.trim().to_ascii_lowercase().as_str() {
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                _ => Err(McpFailure::InvalidArgument),
            },
            // 对面声明的是对象/数组：把这段 JSON 文本还原成真正的 JSON 值。
            // ⚠ 这里 trim（与 `Text` 相反）：JSON 里的空白不承载意思，去掉更宽容。
            // 还原不出来 ⇒ 本地参数不合法（**零转发**）——与其余三种标量同一条路。
            JsonKind::Json => {
                serde_json::from_str::<Value>(raw.trim()).map_err(|_| McpFailure::InvalidArgument)
            }
        }
    }
}

/// 一条进了我们动词面的动词。
struct ToolSpec {
    /// 我们的动词名：`<server>_<对面原名>`。
    action: String,
    /// 对面的原名（转发时用它——**对面只认识自己的名字**）。合成动作没有这一格。
    tool: String,
    /// 这一条是真实工具还是资源数据面的通用动作。
    kind: SpecKind,
    /// **这一条自己的**效果档。
    ///
    /// 工具跟宿主给这台 server 的声明走（默认最严、`,read-only` 才只读）；
    /// 资源动作**永远只读**——读资源改变不了外面的世界，与那句背书无关。
    effect: McpEffect,
    skeleton: String,
    details: String,
    params: Vec<ParamSpec>,
    /// 对面自己的参数：`(名字, 要还原成的类型)`。
    kinds: Vec<(String, JsonKind)>,
    /// `Some(原因)` = 参数形状我们表达不了：动词照旧在场，调用本地回绝。
    trouble: Option<String>,
    /// 挂上了我们那个可选超时旋钮吗。
    timeout_knob: bool,
}

impl ToolSpec {
    /// 某个参数名要还原成什么类型。
    fn kind_of(&self, name: &str) -> Option<JsonKind> {
        self.kinds
            .iter()
            .find(|(existing, _)| existing == name)
            .map(|(_, kind)| *kind)
    }

    /// 这一条能力说明（骨架 + 详情 + 参数表）。
    fn capability(&self) -> Capability {
        let capability = Capability::new(
            ActionId::new(self.action.clone()),
            self.skeleton.clone(),
            self.params.clone(),
        )
        .with_details(self.details.clone());

        // **免查**：省掉"先看格式"那一步——而那一步**吃模型能力**（`core.rs:279` 记着一次真机实测：
        // 模型对参数名**连试三次都错**，三个 Run 都卡在第一轮）。代价是目录行变长，
        // 而目录行**每一轮都在**前缀里 ⇒ 所以判据是**压成一行之后有多长**，不是"要不要"。
        //
        // ⚠ 长度问 `params_line()`——它是"把参数表压成一行"的**唯一权威**，不在这里重写连接串。
        // ⚠ 参数形状表达不了的动词**不免查**：那种动词没有参数表可给，免查是空的。
        if self.trouble.is_none() && capability.params_line().len() <= EXEMPT_PARAMS_BUDGET {
            capability.exempt_from_inspection()
        } else {
            capability
        }
    }
}

/// 外部来源的 App：一台 server 的动词面。
pub struct McpApp {
    access: McpAccess,
    /// 这一次装配里已经拿到的动词面（[`AppPort::present`] 填；没展开过就是空的）。
    specs: Vec<ToolSpec>,
    /// 给主体看的那一句话（名字与档位在装配时就定了）。
    description: String,
    /// 清单变过、还没被主体“拉”过：下一次展开这个 App 回当前清单，而不是旧门卡。
    tools_changed: bool,
}

impl McpApp {
    /// 装配本件（通道还没建；`present()` 才是它被请出来的那一下）。
    pub fn new(access: McpAccess) -> Self {
        let declaration = access.declaration();
        let description = format!(
            "外部来源的 App：`{}` 那台 server 暴露的动词（宿主装机时声明，装了才在；用之前先展开）。\
             调用是转发：窗口 {} 秒（逐动词可以调），回执正文 ≤{} KiB 且超限截断带标记，\
             非文本内容只报类型与条数、不透传；子进程环境里洗掉凭据。效果档 = {}（宿主声明）。\
             若它声明 resources，展开后另有两个只读资源动作（只给清单与按需读取，资源本身不会各自变成动词）。",
            declaration.name(),
            declaration.timeout_seconds(),
            MAX_TEXT_BYTES / 1024,
            declaration.effect().as_str(),
        );
        Self {
            access,
            specs: Vec::new(),
            description,
            tools_changed: false,
        }
    }

    /// 这一次装配里已经拿到的动词面。
    fn own_capabilities(&self) -> Capabilities {
        Capabilities::of(self.specs.iter().map(ToolSpec::capability).collect())
    }

    /// 转发的窗口（声明的那个）。
    fn default_timeout(&self) -> Duration {
        Duration::from_secs(self.access.declaration().timeout_seconds())
    }

    /// 一次调用：真实工具走转发，资源数据面的两个通用动作走只读读取。
    ///
    /// 三条路径共用同一份参数还原与诊断（[`build_arguments`]）；**效果档按每一条自己那一格**
    /// （[`ToolSpec::effect`]）——工具跟宿主声明走，资源动作永远只读。
    fn call(&mut self, spec_index: usize, params: &ActionParams) -> ActionOutcome {
        let (kind, effect, action, troubled) = {
            let spec = &self.specs[spec_index];
            (
                spec.kind,
                spec.effect,
                spec.action.clone(),
                spec.trouble.is_some(),
            )
        };
        if troubled {
            return ActionOutcome::failure(McpFailure::UnsupportedSchema.as_str());
        }
        let (arguments, timeout) =
            match build_arguments(&self.specs[spec_index], params, self.default_timeout()) {
                Ok(built) => built,
                // 本地拒绝照旧是 `invalid-argument` 一个词，**诊断贴在它旁边**（见 `refused_arguments`）。
                Err(refusal) => return refused_arguments(&refusal),
            };
        let name = self.access.declaration().name().to_string();
        let content = match kind {
            SpecKind::Tool => {
                let tool = self.specs[spec_index].tool.clone();
                through_channel(&self.access, |transport| {
                    transport.call_tool(&tool, arguments, timeout)
                })
                .map(|(reply, reading)| {
                    let receipt = render_reply(&name, &tool, &reply, reading);
                    vec![ContentBlock::new(action.clone(), receipt)]
                })
            }
            SpecKind::ResourcesList => {
                // 空游标与"没给"是同一件事：都从第一页开始。
                let cursor = arguments
                    .get(PARAM_CURSOR)
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .filter(|cursor| !cursor.is_empty());
                through_channel(&self.access, |transport| {
                    transport.list_resources(cursor, timeout)
                })
                .map(|(page, reading)| {
                    let receipt = render_resource_list(&name, &page, reading);
                    vec![ContentBlock::new(action.clone(), receipt)]
                })
            }
            SpecKind::ResourcesRead => {
                let uri = arguments
                    .get(PARAM_URI)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                through_channel(&self.access, |transport| {
                    transport.read_resource(&uri, timeout)
                })
                .map(|(bodies, reading)| {
                    let receipt = render_resource_read(&name, &uri, &bodies, reading);
                    vec![ContentBlock::new(action.clone(), receipt)]
                })
            }
        };
        match content {
            Ok(blocks) => ActionOutcome::success(blocks),
            // 资源动作的效果是只读 ⇒ `fault_outcome` 不会落进 `unknown`：
            // 读不改变世界，没有"可能已经发生"这一格。
            Err(fault) => fault_outcome(&fault, effect),
        }
    }
}

/// 在通道那一格上做一次调用：拿锁 → 调用 → 读凭据读数；**故障即收通道**。
///
/// 收掉不重连：`再来一次`是**下一次显式展开**的事，不是调用路径上的事（既有纪律，未改）。
fn through_channel<T>(
    access: &McpAccess,
    call: impl FnOnce(&mut dyn Transport) -> Result<T, CallFault>,
) -> Result<(T, ScrubReading), CallFault> {
    let slot = access.slot();
    let mut state = slot.state.lock().expect("通道那一格只被短临界区持有");
    let Some(lease) = state.lease.as_mut() else {
        // 通道不在（还没展开、或者上一次它坏了被我们收了）：调用路径上**不自动重连**。
        return Err(CallFault::Refused(McpFailure::Transport));
    };
    // 租期从**最后一次使用**起算；一次调用本身跨多久不算空闲。
    lease.last_used = Instant::now();
    match call(&mut *lease.transport) {
        Ok(value) => {
            let reading = lease.transport.credential_reading();
            Ok((value, reading))
        }
        Err(fault) => {
            // 我们自己的机械故障：这条通道不能再用了（进程可能要收，也可能已经死了）。
            state.lease = None;
            Err(fault)
        }
    }
}

impl ActionPort for McpApp {
    fn capabilities(&self, _agent: &AgentId, _run: &RunId) -> Capabilities {
        self.own_capabilities()
    }

    /// 局部准入：本层一律放行（**在场 = 被授权**）。
    ///
    /// 效果类别那一格不由本层判（它只是**声明**出来，见 [`AppPort::declared_effects`]）；档位那一格
    /// 由宿主在装配时划。⚠ 这里原来写的是"写效应那一格由 OS 的未决效果门按声明类别管"——**门已
    /// 退役**（门是指令，而世界只该给信息）：**声明与装配期自洽性检查照旧**，只是不再有一道门
    /// 按类别拦写。
    fn admit(&self, _agent: &AgentId, _run: &RunId, action: &Action) -> ActionAdmission {
        match self
            .specs
            .iter()
            .any(|spec| spec.action == action.id().as_str())
        {
            true => ActionAdmission::Admitted,
            false => ActionAdmission::refused(crate::core::UNKNOWN_CAPABILITY),
        }
    }

    fn execute(
        &mut self,
        _agent: &AgentId,
        _operation: &OperationId,
        action: &Action,
    ) -> ActionOutcome {
        match self
            .specs
            .iter()
            .position(|spec| spec.action == action.id().as_str())
        {
            Some(index) => self.call(index, action.params()),
            None => ActionOutcome::failure(McpFailure::InvalidArgument.as_str()),
        }
    }
}

impl AppPort for McpApp {
    /// App 标识 = **server 名**（宿主给的那个；动词名的前缀也是它）。
    fn app_id(&self) -> &str {
        self.access.declaration().name()
    }

    fn description(&self) -> &str {
        &self.description
    }

    /// 装配期：**一条都没有**（清单要等真的连上才拿到）。
    ///
    /// 这不是"没能力"：能力在 [`AppPort::present`] 那一下才交出来（见 `os` 的展开路径）。
    fn declared_capabilities(&self) -> Capabilities {
        self.own_capabilities()
    }

    /// 展开那一下：把通道建起来（首次）、把清单拉回来、把动词面交出去。
    ///
    /// 失败给一个 [`ExpansionRefusal`]：**稳定分类**（[`McpFailure`] 里的某一个）进主体的回执，
    /// **对面自己那句话**（若有）作为随单内容贴在它旁边。分类与内容**分开**——分类是词表，
    /// 同一故障两次逐字节相同；对面的话是内容，只作携带。
    ///
    /// ⚠ **握手被拒那一路是这一格存在的理由**：从前它只留下一个分类，对面"为什么拒"的原文
    /// 在整个适配层里被丢掉。现在通道把那一句留在 [`StdioTransport::refusal_detail`] 上，
    /// 这里取出来一起交出去（见 `mcp` 模块文档"一处如实的取舍"）。
    ///
    /// 任何一次失败都把通道收掉（不留下一条半死的连接）。
    fn present(&mut self) -> Result<(), ExpansionRefusal> {
        let declaration = self.access.declaration().clone();
        let timeout = self.default_timeout();
        let slot = self.access.slot();
        let mut state = slot.state.lock().expect("通道那一格只被短临界区持有");

        if state.lease.is_none() {
            // 生产通道由 `rmcp` 宿主承担；stdio 与 Streamable 网络通道在宿主内部按声明的
            // endpoint 分流，上层只看见同一份同步 `Transport`。
            let transport: Box<dyn Transport> = match declaration.endpoint() {
                McpEndpoint::Stdio { .. } | McpEndpoint::Remote { .. } => {
                    let mut transport = match RmcpTransport::new(&declaration, self.access.scrub())
                    {
                        Ok(transport) => transport,
                        Err(class) => return Err(ExpansionRefusal::classified(class.as_str())),
                    };
                    if let Err(fault) = transport.connect(timeout) {
                        return Err(ExpansionRefusal::from_detail(
                            fault.class().as_str(),
                            transport.refusal_detail(),
                        ));
                    }
                    Box::new(transport)
                }
            };
            state.lease = Some(Lease {
                transport,
                last_used: Instant::now(),
            });
        }

        let listed = match state.lease.as_mut() {
            Some(lease) => {
                // 展开也是一次真实使用：租期从这里重新起算。
                lease.last_used = Instant::now();
                lease.transport.list_tools(timeout)
            }
            None => Err(CallFault::Refused(McpFailure::Transport)),
        };
        let raw: Vec<RawTool> = match listed {
            Ok(raw) => raw,
            Err(fault) => {
                state.lease = None;
                return Err(ExpansionRefusal::classified(fault.class().as_str()));
            }
        };

        match map_tools(
            declaration.name(),
            declaration.effect(),
            declaration.timeout_seconds(),
            &raw,
        ) {
            Ok(mut specs) => {
                // 资源只有**对面声明了**才有这一面；没有就一个动作都不多出（旧行为逐字节不变）。
                let resources = state
                    .lease
                    .as_ref()
                    .map(|lease| lease.transport.supports_resources())
                    .unwrap_or(false);
                if resources {
                    specs.extend(resource_specs(
                        declaration.name(),
                        declaration.timeout_seconds(),
                        &specs,
                    ));
                }
                self.specs = specs;
                self.tools_changed = false;
                // 订阅是**增量**，不是展开的必要条件：订阅不上（legacy / 远程一问一答 /
                // 对面拒绝）不影响这次展开，清单已经拿到了。
                if let Some(lease) = state.lease.as_mut() {
                    let _ = lease.transport.subscribe_tools_changed(timeout);
                }
                Ok(())
            }
            Err(class) => {
                // 清单不是我们能搬运的形状：整台不上场，通道也不留。
                state.lease = None;
                Err(ExpansionRefusal::classified(class.as_str()))
            }
        }
    }

    /// 处理已经到达的 `tools/list_changed`：重新拉清单，并标记“变过”。
    ///
    /// 返回 `true` = 清单确实换了；`false` = 没有通知、通道不在、或重拉失败（保留旧清单）。
    /// **只处理类型，不把对面通知的正文搬进来**；新清单仍走 `tools/list`。
    fn refresh_capabilities(&mut self) -> bool {
        let declaration = self.access.declaration().clone();
        let timeout = self.default_timeout();
        let slot = self.access.slot();
        let mut state = slot.state.lock().expect("通道那一格只被短临界区持有");
        let Some(lease) = state.lease.as_mut() else {
            return false;
        };
        lease.last_used = Instant::now();
        if !lease
            .transport
            .poll_notifications()
            .contains(&McpNotification::ToolsListChanged)
        {
            return false;
        }
        let raw = match lease.transport.list_tools(timeout) {
            Ok(raw) => raw,
            Err(_) => return false,
        };
        let resources = lease.transport.supports_resources();
        let specs = match map_tools(
            declaration.name(),
            declaration.effect(),
            declaration.timeout_seconds(),
            &raw,
        ) {
            Ok(mut specs) => {
                if resources {
                    specs.extend(resource_specs(
                        declaration.name(),
                        declaration.timeout_seconds(),
                        &specs,
                    ));
                }
                specs
            }
            Err(_) => return false,
        };
        self.specs = specs;
        self.tools_changed = true;
        true
    }

    /// 有变更 ⇒ 下一次展开这个 App 回当前清单；取走即清。
    fn take_capability_change(&mut self) -> bool {
        let changed = self.tools_changed;
        self.tools_changed = false;
        changed
    }

    /// 每一条动作**自己那一格**的效果类别。
    ///
    /// 工具：**整台一个档**（一台 server 一授权，不逐工具判）。默认档是
    /// [`EffectClass::UnverifiableWrite`]：外来工具的写效应没有同参复核可配
    /// （再调一次不是复核，是又一次调用）——因此它**不配复核能力**，也就**没有机械的解除手段**。
    /// 只读档是宿主的显式背书。
    ///
    /// 资源动作：**永远只读**，与那句背书无关——读资源改变不了外面的世界。这一格不是
    /// "我们替 server 判了"，是动作本身的形状（`resources/list` / `resources/read` 是读）。
    ///
    /// ⚠ 这里原来写的是"未决一旦挂上就只有宿主 acknowledge 这一条出路"——那是**未决效果门**的
    /// 说法，门已退役（门是指令，而世界只该给信息）：今天"那次写到底发生了没有"由主体自己去看
    /// （它家里的账本 `trace.ndjson`）。
    fn declared_effects(&self) -> Vec<EffectDeclaration> {
        self.specs
            .iter()
            .map(|spec| {
                EffectDeclaration::new(
                    ActionId::new(spec.action.clone()),
                    spec.effect.class(),
                    None,
                )
            })
            .collect()
    }

    /// 来源档：**整台一个档，而且是最严的那一档**。
    ///
    /// 外来的 MCP server 说的是别人的话，没有任何署名可言（工具名与回执都由对面给），因此整台
    /// 落在 [`SourceClass::WorldUnsigned`]。这不是"忘了让宿主声明"——**外部来源的默认值就该是
    /// 最严档，不需要任何人签字**；想抬到别的档，得由主人显式背书（与 `,read-only` 那条后缀
    /// 同款：升降都要有人负责）。
    ///
    /// 与 [`AppPort::declared_capabilities`] 同源派生：清单是展开那一下才拉回来的，因此这份声明
    /// 必须跟着 `self.specs` 走，不能写死。
    fn declared_sources(&self) -> Vec<SourceDeclaration> {
        self.specs
            .iter()
            .map(|spec| {
                SourceDeclaration::new(
                    ActionId::new(spec.action.clone()),
                    SourceClass::WorldUnsigned,
                )
            })
            .collect()
    }
}

/// 清单 → 动词面（**整台**的机械检查都在这一个函数里）。
///
/// 任何一条不成立都**整台不上场**（`Err`），因为那些错误都不是"某一条动词不好用"：
/// 名字越界会让每次请求在出口被拒，清单超上限是"每轮摆进主体的视野的形状"被突破，
/// 同一个动词名出现两次是清单自相矛盾（路由到谁都没有正确答案）。
fn map_tools(
    server: &str,
    effect: McpEffect,
    timeout_seconds: u64,
    raw: &[RawTool],
) -> Result<Vec<ToolSpec>, McpFailure> {
    if raw.len() > super::MAX_TOOLS {
        return Err(McpFailure::TooManyTools);
    }
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut specs = Vec::with_capacity(raw.len());
    for tool in raw {
        // 名字得**真的是一个名字**：空白与控制字符不是名字的一部分（那些字符进动词面、
        // 进回执都会伪造版式），因此这一台整台不上场——不做"顺手消毒"（那会改掉我们转发的名字）。
        if tool.name.is_empty()
            || tool
                .name
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err(McpFailure::ToolsFailed);
        }
        let action = format!("{server}_{}", tool.name);
        if !super::verb_fits(server, &tool.name) {
            return Err(McpFailure::NameTooLong);
        }
        if !seen.insert(action.clone()) {
            return Err(McpFailure::ToolsFailed);
        }
        specs.push(build_spec(server, effect, timeout_seconds, &action, tool));
    }
    Ok(specs)
}

/// 一条工具 → 一条动词。
fn build_spec(
    server: &str,
    effect: McpEffect,
    timeout_seconds: u64,
    action: &str,
    tool: &RawTool,
) -> ToolSpec {
    let (params, kinds, trouble) = read_schema(&tool.input_schema);
    // 对面自己没有叫 `timeout` 的参数时，才挂我们那个旋钮（撞名没有正确答案）。
    let timeout_knob = trouble.is_none() && !kinds.iter().any(|(name, _)| name == PARAM_TIMEOUT);
    let mut params = params;
    if timeout_knob {
        params.push(ParamSpec::new(
            PARAM_TIMEOUT,
            false,
            timeout_doc(timeout_seconds),
        ));
    }
    let skeleton = match tool.description.as_deref() {
        Some(described) => foreign_line(described, MAX_SKELETON_BYTES),
        None => "（对面没有给这一条动词的说明）".to_string(),
    };

    let mut details = format!(
        "外部来源的动词（`{server}` 那台 server）：我们只转发，限制全部是机械的。效果档 = {}；\
         转发窗口 {} 秒。回执：正文 ≤{} KiB（超限截断并明确标出），非文本内容只报类型与条数、\
         不透传。对面的 `isError` 是读数不是失败分类——转发出去了、答复回来了就是成功。",
        effect.as_str(),
        timeout_seconds,
        MAX_TEXT_BYTES / 1024,
    );
    if trouble.is_none() {
        details.push_str(if timeout_knob {
            "可选参数 `timeout`（秒）只改这一次的等待窗口，不会被转发给对面。"
        } else {
            "对面自己占了 `timeout` 这个名字，因此这条动词没有我们的超时旋钮（那个参数归它）。"
        });
    }
    if let Some(reason) = &trouble {
        details.push_str(&format!(
            "这条动词的参数形状我们表达不了（{reason}）：参数表因此是空的，\
             调用会被本地回绝（unsupported-schema）——本层搬运标量，\
             以及按一段 JSON 文本给的对象与数组；其余形状搬不动。"
        ));
    }

    ToolSpec {
        action: action.to_string(),
        tool: tool.name.clone(),
        kind: SpecKind::Tool,
        effect,
        skeleton,
        details,
        params,
        kinds,
        trouble,
        timeout_knob,
    }
}

/// 我们那个可选超时旋钮的一句说明（工具与资源动作共用，避免两处措辞分叉）。
fn timeout_doc(timeout_seconds: u64) -> String {
    format!("这次等多久（秒，1–{MAX_TIMEOUT_SECONDS}；不给就用 {timeout_seconds} 秒）")
}

/// 资源数据面的两个**通用只读动作**（不是每资源一个动词）。
///
/// 判据：资源是**数据**，动作是**能力**。一条资源一个动词会让每轮摆进主体视野的动词面
/// 随对面的数据长大；两个通用动作把它固定成两条，资源清单只进回执。
///
/// 名字冲突：对面自己报了同名工具（`<server>_resources_list` / `<server>_resources_read`）时，
/// **跳过那一条合成动作**，不整台失败——工具名归对面（它可能真的有一个叫 `resources_list`
/// 的工具），我们不能把它的动词改名；另一个通用动作照旧挂上。代价如实登记：那条资源动作
/// 这时不可用（名字被对面的工具占了），但**不会**把对面的工具悄悄变成只读动作——每个
/// spec 自己带效果（见 [`ToolSpec::effect`]）。
fn resource_specs(server: &str, timeout_seconds: u64, existing: &[ToolSpec]) -> Vec<ToolSpec> {
    let mut specs = Vec::new();

    let list_action = format!("{server}_{SUFFIX_RESOURCES_LIST}");
    if !existing.iter().any(|spec| spec.action == list_action) {
        specs.push(ToolSpec {
            action: list_action,
            tool: String::new(),
            kind: SpecKind::ResourcesList,
            effect: McpEffect::ReadOnly,
            skeleton: "列出这台 server 的资源（只读；只给清单，正文按需读）".to_string(),
            details: format!(
                "外部来源的只读数据面（`{server}` 那台 server 在握手时声明了 `resources`）：\
                 一次一页，回执只给 name / uri / mime / size 与下一页游标（`cursor`），不带正文。\
                 资源不是动词——这台 server 的资源不会各自变成动作。\
                 效果档 = read-only（读不改变外面的世界）。"
            ),
            params: vec![
                ParamSpec::new(
                    PARAM_CURSOR,
                    false,
                    "上一页回执给的 next-cursor；不给就是第一页",
                ),
                ParamSpec::new(PARAM_TIMEOUT, false, timeout_doc(timeout_seconds)),
            ],
            kinds: vec![(PARAM_CURSOR.to_string(), JsonKind::Text)],
            trouble: None,
            timeout_knob: true,
        });
    }

    let read_action = format!("{server}_{SUFFIX_RESOURCES_READ}");
    if !existing.iter().any(|spec| spec.action == read_action) {
        specs.push(ToolSpec {
            action: read_action,
            tool: String::new(),
            kind: SpecKind::ResourcesRead,
            effect: McpEffect::ReadOnly,
            skeleton: "按 URI 读一条资源（只读；正文只有这一次调用才取）".to_string(),
            details: format!(
                "外部来源的只读数据面（`{server}` 那台 server 在握手时声明了 `resources`）：\
                 `uri` 从 resources_list 的回执里抄。正文按需取，读完只进这一次的回执，\
                 不会自动进模型上下文；文本 ≤{} KiB（超限截断并标出），\
                 二进制（blob）只报 mime 与大小、不搬原文。效果档 = read-only。",
                MAX_TEXT_BYTES / 1024
            ),
            params: vec![
                ParamSpec::new(
                    PARAM_URI,
                    true,
                    "要读的资源 URI（从 resources_list 的回执里抄）",
                ),
                ParamSpec::new(PARAM_TIMEOUT, false, timeout_doc(timeout_seconds)),
            ],
            kinds: vec![(PARAM_URI.to_string(), JsonKind::Text)],
            trouble: None,
            timeout_knob: true,
        });
    }

    specs
}

/// 对面的参数模式 → 我们的参数表（标量，以及按一段 JSON 文本搬的对象/数组；读不出来就给一个原因）。
///
/// 返回 `(参数表, 还原类型, 读不出来的原因)`：前两个同时为空、原因为 `None` = 这一条动词不吃参数。
fn read_schema(schema: &Value) -> (Vec<ParamSpec>, Vec<(String, JsonKind)>, Option<String>) {
    let Some(object) = schema.as_object() else {
        return (
            Vec::new(),
            Vec::new(),
            Some("参数模式不是一个对象".to_string()),
        );
    };
    let required: std::collections::BTreeSet<&str> = object
        .get("required")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let properties = match object.get("properties") {
        Some(value) => match value.as_object() {
            Some(properties) => properties,
            None => {
                return (
                    Vec::new(),
                    Vec::new(),
                    Some("`properties` 不是一个对象".to_string()),
                );
            }
        },
        None => {
            return if required.is_empty() {
                (Vec::new(), Vec::new(), None)
            } else {
                (
                    Vec::new(),
                    Vec::new(),
                    Some("有必填参数，却没有 `properties`".to_string()),
                )
            };
        }
    };

    // 必填的名字必须真的在参数表里，否则这条动词语义不完整（防御：外面那种写法读不出来）。
    if let Some(missing) = required
        .iter()
        .find(|name| !properties.contains_key(**name))
    {
        return (
            Vec::new(),
            Vec::new(),
            Some(format!("必填参数 `{missing}` 没有说明类型")),
        );
    }

    let mut params = Vec::with_capacity(properties.len());
    let mut kinds = Vec::with_capacity(properties.len());
    for (name, spec) in properties {
        let Some(kind) = read_kind(spec) else {
            return (
                Vec::new(),
                Vec::new(),
                Some(format!("参数 `{name}` 的类型我们表达不了")),
            );
        };
        // 对面自己那句话（外来文本：消毒 + 截断 + 标记）。
        let described = spec
            .get("description")
            .and_then(Value::as_str)
            .map(|text| foreign_line(text, MAX_PARAM_DOC_BYTES))
            .unwrap_or_default();
        // 结构型参数还要说清"那段 JSON 该长什么样"——否则只能猜、只能试。
        //
        // 判据（2026-10-13）：**好的信息能减少犯错的概率**。形状说明因此不是装饰，
        // 是"写不写得出来"的**必要条件**；而它进的是**详情卡**（按需取），不是骨架（每轮都在）。
        // ⚠ **形状写在前面**：截断砍的是尾巴（对面那句话），形状因此优先保住。
        let doc = match kind {
            JsonKind::Json => match shape_of(spec) {
                Some(shape) => {
                    let head = format!("一段 JSON 文本，形如 {shape}");
                    let full = if described.is_empty() {
                        head
                    } else {
                        format!("{head}。{described}")
                    };
                    clip_ours(&full, MAX_PARAM_DOC_BYTES)
                }
                None => described,
            },
            _ => described,
        };
        params.push(ParamSpec::new(
            name.clone(),
            required.contains(name.as_str()),
            doc,
        ));
        kinds.push((name.clone(), kind));
    }
    (params, kinds, None)
}

/// 按字节预算截断**我们自己拼的**一段文本，超限补标记（静默截断违禁）。
///
/// 它与 [`foreign_line`] 只差一处：**不再消毒**——这段是我们拼的，其中外来那一半
/// （对面那句话、键名）**已经各自消过毒**。再消毒一次会把我们有意留的结构符号也吃掉。
fn clip_ours(text: &str, limit: usize) -> String {
    let (clipped, truncated) = clip(text, limit);
    if truncated {
        format!("{clipped}…（截断）")
    } else {
        clipped
    }
}

/// 一个属性声明的 `type` 名字：也认 `["object","null"]` 这种写法（去掉 `null` 之后取那一个）。
///
/// 它与 [`read_kind`] 走**同一条口径**——否则会长出"类型读得懂、形状读不出"的不对称：
/// `["object","null"]` 在 MCP 世界里是常见写法，类型那一侧收了它，形状这一侧就不该放空。
fn declared_type(spec: &Value) -> Option<&str> {
    match spec.as_object()?.get("type")? {
        Value::String(kind) => Some(kind.as_str()),
        Value::Array(kinds) => kinds
            .iter()
            .filter_map(Value::as_str)
            .find(|kind| *kind != "null"),
        _ => None,
    }
}

/// 一个**结构型**参数的形状说明（紧凑记法：**深度一层**、键数有界）。
///
/// 它服务的是"写不写得出来"：只说"这是一段 JSON 文本"而不说里面有什么键，
/// **只能猜、只能试**。因此这一段是必要的，不是锦上添花。
///
/// ⚠ **键名是外来文本**（对面写的），逐个过 [`sanitize`]——防它在键名里塞换行伪造版式。
/// ⚠ **只渲染一层**：更深的层**不渲染**（只有**键数**超限才报数）——
/// 本层不声称"更深的东西也说清了"，那件事归对面自己的 `description`。
fn shape_of(spec: &Value) -> Option<String> {
    let object = spec.as_object()?;
    match declared_type(spec)? {
        "object" => {
            let Some(properties) = object.get("properties").and_then(Value::as_object) else {
                // 对面只说"是个对象"、没说里面有什么：如实说"键不限"，**不编**。
                return Some("{…}".to_string());
            };
            let mut parts: Vec<String> = Vec::new();
            let mut hidden = 0usize;
            for (name, sub) in properties {
                if parts.len() >= MAX_SHAPE_KEYS {
                    hidden += 1;
                    continue;
                }
                parts.push(format!(
                    "{}:{}",
                    sanitize(name),
                    short_kind(declared_type(sub).unwrap_or_default())
                ));
            }
            let mut out = format!("{{{}}}", parts.join(","));
            if hidden > 0 {
                out.push_str(&format!("…还有 {hidden} 个键"));
            }
            Some(out)
        }
        "array" => {
            let kind = object.get("items").and_then(declared_type);
            Some(match kind {
                Some(kind) => format!("[{},…]", short_kind(kind)),
                None => "[…]".to_string(),
            })
        }
        _ => None,
    }
}

/// 类型名的**紧凑记法**（尽量一个汉字）：这段进的是**详情卡**，而预算是 120 字节。
///
/// 认不出的类型如实写 `?`——**不猜**（与 [`read_kind`] 同一条口径）。
fn short_kind(kind: &str) -> &'static str {
    match kind {
        "string" => "串",
        "integer" | "number" => "数",
        "boolean" => "真",
        "object" => "对",
        "array" => "列",
        _ => "?",
    }
}

/// 一个参数声明的类型 → 我们的还原类型；读不出来就是 `None`（**不猜**）。
///
/// 读得懂的是**两类**：四种标量 ＋ **对象/数组**（后者内部按一段 JSON 文本搬，
/// 见 [`JsonKind::Json`]）。其余一律 `None`——`null`、认不出的类型名、
/// 以及**两种不同类型拼起来的联合**，都不做就近映射。
fn read_kind(spec: &Value) -> Option<JsonKind> {
    let object = spec.as_object()?;
    match object.get("type") {
        Some(Value::String(kind)) => read_type(kind),
        // 多类型：去掉 `null` 之后只剩**同一种**才算读得懂（`["string","null"]` 是常见写法）。
        Some(Value::Array(kinds)) => {
            let mut found: Option<JsonKind> = None;
            for kind in kinds {
                let kind = kind.as_str()?;
                if kind == "null" {
                    continue;
                }
                let parsed = read_type(kind)?;
                match found {
                    None => found = Some(parsed),
                    Some(existing) if existing == parsed => {}
                    Some(_) => return None,
                }
            }
            found
        }
        // 没有 `type`、但有 `enum` 而且取值全是字符串：那种写法按字符串收。
        None => object
            .get("enum")
            .and_then(Value::as_array)
            .filter(|values| !values.is_empty() && values.iter().all(Value::is_string))
            .map(|_| JsonKind::Text),
        Some(_) => None,
    }
}

/// 一个 JSON 类型名 → 我们的还原类型。
///
/// 标量四种见 [`scalar_kind`]；**对象与数组**归 [`JsonKind::Json`]——
/// 对面声明的是结构，我们内部仍按一段 JSON 文本搬，转发之前还原。
fn read_type(kind: &str) -> Option<JsonKind> {
    match kind {
        "object" | "array" => Some(JsonKind::Json),
        _ => scalar_kind(kind),
    }
}

/// 一个 JSON 类型名 → 我们的还原类型（只有四种标量读得懂）。
fn scalar_kind(kind: &str) -> Option<JsonKind> {
    match kind {
        "string" => Some(JsonKind::Text),
        "integer" => Some(JsonKind::Integer),
        "number" => Some(JsonKind::Number),
        "boolean" => Some(JsonKind::Flag),
        _ => None,
    }
}

/// 我们的参数 → 对面的参数对象（+ 这一次的窗口）。
///
/// 本地就能判掉的一律在这里判掉（**零转发**）：出现不认识的参数名、取值还原不成对面要的类型、
/// 必填没到齐、同一个参数给了两次。
///
/// ⚠ **四种拒绝各带自己的诊断**（[`ArgumentRefusal`]）：分类照旧只有 `invalid-argument` 一个词、
/// 逐字节稳定，**原因与期望形状是它旁边的读数**。从前这四种塌成一个分类，
/// 主体当场不知道自己错在哪一项——与 `os.rs::ParamShape` 治的是同一个病。
fn build_arguments(
    spec: &ToolSpec,
    params: &ActionParams,
    default_timeout: Duration,
) -> Result<(Value, Duration), ArgumentRefusal> {
    let mut arguments = Map::new();
    let mut timeout = default_timeout;
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for (name, raw) in params.entries() {
        if !seen.insert(name.as_str()) {
            return Err(ArgumentRefusal {
                name: name.clone(),
                reason: ArgumentRefusalReason::Duplicated,
                expected: String::new(),
            });
        }
        if name == PARAM_TIMEOUT && spec.timeout_knob {
            let Ok(seconds) = raw.trim().parse::<u64>() else {
                return Err(ArgumentRefusal {
                    name: name.clone(),
                    reason: ArgumentRefusalReason::WrongKind,
                    expected: format!("整数（1–{MAX_TIMEOUT_SECONDS} 秒）"),
                });
            };
            if !(1..=MAX_TIMEOUT_SECONDS).contains(&seconds) {
                return Err(ArgumentRefusal {
                    name: name.clone(),
                    reason: ArgumentRefusalReason::WrongKind,
                    expected: format!("1–{MAX_TIMEOUT_SECONDS} 秒之间"),
                });
            }
            timeout = Duration::from_secs(seconds);
            continue;
        }
        let Some(kind) = spec.kind_of(name) else {
            return Err(ArgumentRefusal {
                name: name.clone(),
                reason: ArgumentRefusalReason::UnknownParameter,
                // 期望形状是"这条动词认哪些名字"：从声明来，机械枚举。
                expected: spec
                    .params
                    .iter()
                    .map(|each| each.name().to_string())
                    .collect::<Vec<String>>()
                    .join(", "),
            });
        };
        let Ok(value) = kind.coerce(raw) else {
            return Err(ArgumentRefusal {
                name: name.clone(),
                reason: ArgumentRefusalReason::WrongKind,
                expected: kind.shape_name().to_string(),
            });
        };
        arguments.insert(name.clone(), value);
    }
    for expected in &spec.params {
        if expected.is_required() && !arguments.contains_key(expected.name()) {
            return Err(ArgumentRefusal {
                name: expected.name().to_string(),
                reason: ArgumentRefusalReason::Missing,
                expected: spec
                    .kind_of(expected.name())
                    .map(|kind| kind.shape_name().to_string())
                    .unwrap_or_default(),
            });
        }
    }
    Ok((Value::Object(arguments), timeout))
}

/// 本地参数拒绝 → 有诊断的失败结果。
///
/// 它是这一格唯一的出口：**分类是词表，诊断是内容**——与 `os.rs::ParamShape::outcome`
/// 逐字同一条纪律，只是那边治名字、这边治值。
fn refused_arguments(refusal: &ArgumentRefusal) -> ActionOutcome {
    ActionOutcome::failure_with_content(
        McpFailure::InvalidArgument.as_str(),
        vec![ContentBlock::new(ARGUMENTS_LABEL, refusal.text())],
    )
}

/// 对面的错误码 → 我们归的那一格（**码本身不进词表**，只用来在两格之间选一个）。
fn classify_error_code(code: Option<i64>) -> &'static str {
    match code {
        // JSON-RPC 的「参数不合法」：那是我们这次请求的形状问题（主体可以改）。
        Some(-32602) => McpFailure::InvalidArgument.as_str(),
        // 其余（内部错误、方法不认识、服务端自定义段……）：对面那一侧的问题。
        _ => McpFailure::ServerError.as_str(),
    }
}

/// 一次答复 → 主体的回执。
fn render_reply(server: &str, tool: &str, reply: &CallReply, reading: ScrubReading) -> String {
    let mut out = String::new();
    out.push_str(&format!("server = {server}\n"));
    out.push_str(&format!("tool = {tool}\n"));
    match reply {
        CallReply::Replied {
            is_error,
            text,
            text_bytes,
            truncated,
            unsupported,
            noise,
        } => {
            out.push_str("state = replied\n");
            out.push_str(&format!("is-error = {is_error}\n"));
            out.push_str(&format!(
                "text = {text_bytes} bytes, truncated = {truncated}\n"
            ));
            out.push_str(&format!(
                "unsupported-content = {}\n",
                if unsupported.is_empty() {
                    "none".to_string()
                } else {
                    unsupported
                        .iter()
                        .map(|(kind, count)| format!("{kind}×{count}"))
                        .collect::<Vec<String>>()
                        .join(",")
                }
            ));
            if *noise > 0 {
                out.push_str(&format!("noise = {noise}\n"));
            }
            push_scrub(&mut out, reading);
            out.push_str("--- result ---\n");
            out.push_str(text);
            if *truncated {
                out.push_str(&format!(
                    "\n（此处截断：正文超过 {MAX_TEXT_BYTES} 字节，只给出前 {MAX_TEXT_BYTES} 字节）\n"
                ));
            } else if !text.is_empty() && !text.ends_with('\n') {
                out.push('\n');
            }
        }
        CallReply::Refused { code, detail } => {
            out.push_str("state = refused-by-server\n");
            out.push_str(&format!("error-class = {}\n", classify_error_code(*code)));
            out.push_str(&format!("error-detail = {detail}\n"));
            push_scrub(&mut out, reading);
        }
    }
    out
}

/// 一页资源清单 → 回执（**只有元数据**：正文一律不在这一格）。
///
/// 这是"资源正文不自动进模型上下文"的第一半：清单只给 name / uri / mime / size
/// （外加对面给了的标题与说明），要正文得再调 `resources_read`。
fn render_resource_list(server: &str, page: &ResourcePage, reading: ScrubReading) -> String {
    let mut out = String::new();
    out.push_str(&format!("server = {server}\n"));
    out.push_str("action = resources/list\n");
    out.push_str(&format!("count = {}\n", page.resources.len()));
    if page.resources.is_empty() {
        out.push_str("resource-list = 0 项（这台 server 现在没有声明任何资源）\n");
    }
    for resource in &page.resources {
        out.push_str(&format!("resource = {}\n", resource.name));
        out.push_str(&format!("  uri = {}\n", resource.uri));
        out.push_str(&format!(
            "  mime = {}\n",
            resource.mime_type.as_deref().unwrap_or("unknown")
        ));
        out.push_str(&format!(
            "  size = {}\n",
            resource
                .size
                .map(|size| format!("{size} bytes"))
                .unwrap_or_else(|| "unknown".to_string())
        ));
        if let Some(title) = resource.title.as_deref().filter(|title| !title.is_empty()) {
            out.push_str(&format!("  title = {title}\n"));
        }
        if let Some(described) = resource
            .description
            .as_deref()
            .filter(|text| !text.is_empty())
        {
            out.push_str(&format!("  description = {described}\n"));
        }
        if resource.uri_truncated {
            out.push_str("  uri-truncated = true（这个 URI 太长，已截断：原样读不回来）\n");
        }
    }
    match &page.next_cursor {
        Some(cursor) => out.push_str(&format!("next-cursor = {cursor}\n")),
        None => out.push_str("next-cursor = none（没有更多页）\n"),
    }
    push_scrub(&mut out, reading);
    out
}

/// 一次资源读取 → 回执。
///
/// **正文只进这一次回执**：读到的字节不在 App 里留副本，也不会被下一轮自动注入。
/// blob 不搬原文，只报它有多大；文本超限截断并带标记。
fn render_resource_read(
    server: &str,
    requested_uri: &str,
    bodies: &[ResourceContent],
    reading: ScrubReading,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("server = {server}\n"));
    out.push_str("action = resources/read\n");
    out.push_str(&format!("requested-uri = {}\n", sanitize(requested_uri)));
    let Some(first) = bodies.first() else {
        out.push_str("state = replied\n");
        out.push_str("content = 0 块（对面没有给正文）\n");
        push_scrub(&mut out, reading);
        return out;
    };
    out.push_str("state = replied\n");
    out.push_str(&format!(
        "uri = {}\n",
        if first.uri.is_empty() {
            "unknown（对面没给可读的 URI）"
        } else {
            first.uri.as_str()
        }
    ));
    out.push_str(&format!(
        "mime = {}\n",
        first.mime_type.as_deref().unwrap_or("unknown")
    ));
    if first.omitted > 0 {
        out.push_str(&format!(
            "omitted-blocks = {}（同一次答复只搬第一块）\n",
            first.omitted
        ));
    }
    match &first.body {
        ResourceBody::Text {
            text,
            text_bytes,
            truncated,
        } => {
            out.push_str(&format!(
                "text = {text_bytes} bytes, truncated = {truncated}\n"
            ));
            push_scrub(&mut out, reading);
            out.push_str("--- content ---\n");
            out.push_str(text);
            if *truncated {
                out.push_str(&format!(
                    "\n（此处截断：资源正文超过 {} KiB，只给出前 {} KiB 字节）\n",
                    MAX_TEXT_BYTES / 1024,
                    MAX_TEXT_BYTES / 1024
                ));
            } else if !text.is_empty() && !text.ends_with('\n') {
                out.push('\n');
            }
        }
        ResourceBody::Blob { encoded_bytes } => {
            out.push_str(&format!(
                "body = blob（base64 编码 {encoded_bytes} 字符，未搬正文）\n"
            ));
            push_scrub(&mut out, reading);
        }
        ResourceBody::Unsupported => {
            out.push_str("body = unsupported（这一块既不是文本也不是 blob，本层搬不了）\n");
            push_scrub(&mut out, reading);
        }
    }
    out
}

/// 洗凭据那两行读数（通道不起子进程时如实说"不适用"）。
fn push_scrub(out: &mut String, reading: ScrubReading) {
    match reading {
        Some((scrubbed, unscrubbed)) => {
            out.push_str(&format!("scrubbed = {scrubbed}\n"));
            out.push_str(&format!(
                "unscrubbed = {}\n",
                if unscrubbed.is_empty() {
                    "none".to_string()
                } else {
                    unscrubbed.join(",")
                }
            ));
        }
        None => out.push_str("scrubbed = n/a（这条通道不起子进程）\n"),
    }
}
