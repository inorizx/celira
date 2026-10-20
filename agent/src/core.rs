//! Agent Core：一次有限活动的运行规则，以及唯一一条执行流程。
//!
//! Core 只回答一个问题：Agent 如何在有限的清醒时间与有限的模型能力中，走完一段
//! 可验证的活动。它不回答为什么此刻开始、哪个外部世界更重要、应该理解什么。
//!
//! 这里没有供应商协议、没有外部世界的词汇，也没有替主体 做判断的地方。

use crate::cost::RunCost;
use crate::foundation::{
    ActionResult, AgentId, Ledger, LedgerError, OperationId, Outcome, Record, ResultState, RunId,
    SubjectEnding, SystemEnding, Turn,
};

/// 一次活动的资源边界。它是机械上限，不是对 Agent 的判断。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// 这次活动最多处理多少次 Turn。
    pub max_turns: u32,
}

impl Limits {
    /// 给定 Turn 上限。
    pub fn new(max_turns: u32) -> Self {
        Self { max_turns }
    }
}

/// Agent：它拥有自己的身份，也拥有"当前是否已经有一个活动"这个不变量。
///
/// 活动占用不是调用方塞进来的参数，而是 Agent 自己的状态：只能通过开始一次活动占用，
/// 只能由该活动**合法收场**（[`Run::end`]）释放；驱动中途失败时由 Core 内部的私有清理
/// 释放。外部没有"按 RunId 清空活动槽"的公开入口。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    id: AgentId,
    activity: Option<RunId>,
}

impl Agent {
    /// 用稳定身份建立一个空闲的 Agent。
    pub fn new(id: AgentId) -> Self {
        Self { id, activity: None }
    }

    /// 这个 Agent 的稳定身份。
    pub fn id(&self) -> &AgentId {
        &self.id
    }

    /// 当前占用活动槽的活动身份；空闲时为 `None`。
    pub fn activity(&self) -> Option<&RunId> {
        self.activity.as_ref()
    }

    /// 失败清理：只有当槽里正好是这个活动时才清空。
    ///
    /// 它是**私有**的，只给 Core 的驱动流程在"这次调用不再驱动这个活动、但活动并没有
    /// 正常收场"时使用（Model 失败导致活动未收场、账本写入失败等）。它不是收场、不写
    /// 任何事实，外部也没法用它按 RunId 清空别人的活动槽；槽里不是这个活动时是空操作。
    ///
    /// 正常收场走 [`Run::end`]，由它释放槽位。
    fn clear_activity(&mut self, run: &RunId) {
        if self.activity.as_ref() == Some(run) {
            self.activity = None;
        }
    }

    /// 占用活动槽。检查与占用是同一次操作：已占用时拒绝，且不改变槽的状态。
    fn occupy(&mut self, run: &RunId) -> Result<(), RunError> {
        if self.activity.is_some() {
            return Err(RunError::ActiveRunExists);
        }
        self.activity = Some(run.clone());
        Ok(())
    }
}

/// 一次行动的不透明身份。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActionId(String);

impl ActionId {
    /// 建立一个行动身份。
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// 身份的不透明文本形式。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Agent 选择让系统尝试做的一件事。
///
/// 身份与参数**分开**：`id` 说明"做什么"（由定义该项能力的一方命名），`params` 只是中立的
/// 具名标量容器。Core 不解释任何一个参数名的含义，也不为具体世界保留字段分支——路径、目录
/// 这类业务概念只活在定义该能力的 App 里。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    id: ActionId,
    params: ActionParams,
    /// **形状读数**：`None` = 参数是能搬运的标量容器；`Some(reading)` = 这一次请求带过来的参数
    /// **搬不动**，这里带着那句"哪里不对"的形状说明。
    ///
    /// 它说的是**形状**（哪个键、是什么类型），不是取值——与凭据/正文那几条边界同一条纪律。
    shape: Option<String>,
}

impl Action {
    /// 用行动身份建立一个不带参数的行动。
    pub fn new(id: ActionId) -> Self {
        Self {
            id,
            params: ActionParams::none(),
            shape: None,
        }
    }

    /// 带上参数建立一个行动。
    pub fn with_params(id: ActionId, params: ActionParams) -> Self {
        Self {
            id,
            params,
            shape: None,
        }
    }

    /// **参数搬不动**的一次请求：`reading` 说的是形状（哪个键、是什么类型），不是取值。
    ///
    /// 为什么这一格必须存在（2026-10-11）：从前它不存在，于是翻译层遇到一次搬不动的参数时
    /// **表达不出来**，只能把**整段清醒期**判死（`contract-violation`）——主体一个字都说不出来。
    /// 而 Core 里早就写着这条教条：「**不把一次拒绝升级成整个活动的失败**」（见驱动那一段）。
    /// 它只是对**形状**也成立：模型给了一次搬不动的参数，是**那一次调用**的事。
    ///
    /// 它由 Core 自己转成一次 [`ActionAdmission::refused_with`]（分类用中立的
    /// [`INVALID_ARGUMENT`]，随单内容带那句读数），因此**执行端永远见不到它**——
    /// "没有可搬运的参数"就没有"要不要发出去"这件事。
    pub fn malformed(id: ActionId, reading: impl Into<String>) -> Self {
        Self {
            id,
            params: ActionParams::none(),
            shape: Some(reading.into()),
        }
    }

    /// 这次行动的身份（也就是能力标识）。
    pub fn id(&self) -> &ActionId {
        &self.id
    }

    /// 这次行动的参数。
    ///
    /// ⚠ 形状搬不动的那一次（[`Action::malformed`]）**永远到不了执行端**，所以这里给出的空参数
    /// 不会被谁当成"没带参数的一次调用"。
    pub fn params(&self) -> &ActionParams {
        &self.params
    }

    /// 这一次请求的形状读数（`None` = 参数是能搬运的标量容器）。
    pub fn shape_reading(&self) -> Option<&str> {
        self.shape.as_deref()
    }
}

/// 一次行动的中立参数容器：有序的具名标量。
///
/// Core 只搬运它：不解释名字、不解释取值、不做任何业务分支。名字的含义与合法取值由定义该
/// 能力的 App 拥有，因此 Core 里不会出现"路径""文件""目录"这类业务字段。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActionParams {
    entries: Vec<(String, String)>,
}

impl ActionParams {
    /// 没有任何参数。
    pub fn none() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// 追加一个具名标量；同名时后者覆盖前者（机械、可预测，不保留隐式顺序语义）。
    pub fn with(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        let name = name.into();
        self.entries.retain(|(existing, _)| existing != &name);
        self.entries.push((name, value.into()));
        self
    }

    /// 按名取一个标量。
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(existing, _)| existing == name)
            .map(|(_, value)| value.as_str())
    }

    /// 按插入顺序读回全部参数。
    pub fn entries(&self) -> &[(String, String)] {
        &self.entries
    }

    /// 参数个数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否没有任何参数。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 一项可以被选择的行动能力说明。
///
/// 它由定义该能力的一方（当前是文件 App）提供，组合入口显式放进**这一次活动**。它不是全局
/// 注册表：没有安装、没有发现、没有跨世界目录，一次活动只带当前打开的那个 App 的能力。
///
/// **拆写纪律**（分界线一句话）：**骨架 = 不读详情也能正确传参；详情 = 语义边界**
/// （机械读数怎么读、边界、失败形状、上限、效果与复核的配对）。骨架写"干什么"（一行以内，
/// 写不下就是写厚了）。**两条合法路径**：骨架足够时**直调**（零额外往返）；需要细节时
/// **先看卡片再调**（详情进历史，Run 内一直可用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capability {
    id: ActionId,
    /// **骨架**：一句话说清"这是干什么的"。它进 `tools` 数组（每一轮都在前缀里），
    /// 因此要小而稳；调用形状由参数表承担，语义边界在详情里（`inspect_action` 取）。
    skeleton: String,
    /// **详情**：机械读数怎么读、语义边界、失败形状、上限、效果与复核的配对。
    /// 它**不进** `tools` 数组——走尾部（`inspect_action` 的回执、展开回执）。
    details: String,
    params: Vec<ParamSpec>,
    /// 这条能力**不走"先看格式"那一步** ⇒ 目录行必须自带参数名（见
    /// [`Capability::is_exempt_from_inspection`]）。缺省 `false`。
    exempt_from_inspection: bool,
}

impl Capability {
    /// 建立一项能力说明：**骨架**（`tools` 数组里那一句）+ 参数表。
    ///
    /// 详情用 [`Capability::with_details`] 补；不补 = 骨架即全部。
    pub fn new(id: ActionId, skeleton: impl Into<String>, params: Vec<ParamSpec>) -> Self {
        Self {
            id,
            skeleton: skeleton.into(),
            details: String::new(),
            params,
            exempt_from_inspection: false,
        }
    }

    /// 补上**详情**（尾部那段）。同一件事只在这里声明一次，不产生第二份权威。
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = details.into();
        self
    }

    /// 能力标识：`Action::id` 里要用的那个。
    pub fn id(&self) -> &ActionId {
        &self.id
    }

    /// **骨架**：进 `tools` 数组的那一句。
    pub fn skeleton(&self) -> &str {
        &self.skeleton
    }

    /// **详情**：尾部那段（空 = 没有更多详情）。
    pub fn details(&self) -> &str {
        &self.details
    }

    /// 这项能力接受的具名标量参数。
    pub fn params(&self) -> &[ParamSpec] {
        &self.params
    }

    /// **这一条不走"先看格式"那一步**（由声明它的那一方决定；缺省是"要走"）。
    ///
    /// 声明它的**唯一后果**在呈现面：动作目录对这类条目**自带参数名**。
    /// 理由是一句自洽性要求——**"免查"必须同时"免学"**：一个动作既然不需要先看格式，
    /// 那么调用前就没有别的地方能知道它的参数名；目录若不给出，就等于要求对方凭空知道。
    /// （真机实测 2026-09-21：模型对那个"展开"动作的参数名连试三次都错 ⇒ 三个 Run 都卡在
    /// 第一轮。那一次的量见 TASK_69 竣工报告。）
    ///
    /// 缺省 `false`：App 动词照旧"要看格式就自己去看"（那一步是设计，不是负担——
    /// 查过之后进历史、Run 内保持）。
    pub fn is_exempt_from_inspection(&self) -> bool {
        self.exempt_from_inspection
    }

    /// 声明这条能力**不走"先看格式"那一步**（见 [`Capability::is_exempt_from_inspection`]）。
    pub fn exempt_from_inspection(mut self) -> Self {
        self.exempt_from_inspection = true;
        self
    }

    /// 参数表的**紧凑一行**（`名 = 说明（必填）; 名2 = 说明`；没有参数就是`（无参数）`）。
    ///
    /// **一份权威**：需要"把参数表压成一行"的地方（开场的格位块、动作目录里那些免查的条目）
    /// 都问这里，不在各自那里各写一遍连接串。
    pub fn params_line(&self) -> String {
        if self.params.is_empty() {
            return "（无参数）".to_string();
        }
        let parts: Vec<String> = self
            .params
            .iter()
            .map(|spec| {
                format!(
                    "{} = {}{}",
                    spec.name(),
                    spec.description(),
                    if spec.is_required() {
                        "（必填）"
                    } else {
                        ""
                    }
                )
            })
            .collect();
        parts.join("; ")
    }
}

/// 一项能力的参数说明。名字的含义归定义能力的 App，Core 只负责把它原样呈现给 Agent。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamSpec {
    name: String,
    required: bool,
    description: String,
}

impl ParamSpec {
    /// 建立一项参数说明。
    pub fn new(name: impl Into<String>, required: bool, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            required,
            description: description.into(),
        }
    }

    /// 参数名。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 是否必需。
    pub fn is_required(&self) -> bool {
        self.required
    }

    /// 给主体看的一句话说明。
    pub fn description(&self) -> &str {
        &self.description
    }
}

/// 这一次活动当前可用的能力集合。
///
/// 空集是合法状态：没有任何 App 时 Core 仍然成立（此时 Agent 只能选择结束活动）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    items: Vec<Capability>,
}

impl Capabilities {
    /// 没有任何能力。
    pub fn none() -> Self {
        Self { items: Vec::new() }
    }

    /// 用一组能力说明建立集合。
    pub fn of(items: Vec<Capability>) -> Self {
        Self { items }
    }

    /// 全部能力说明，按给出的顺序。
    pub fn items(&self) -> &[Capability] {
        &self.items
    }

    /// 按标识找一项能力。
    pub fn find(&self, id: &ActionId) -> Option<&Capability> {
        self.items.iter().find(|capability| capability.id() == id)
    }

    /// 能力条数。
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// 是否没有任何能力。
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// 行动面的**共用机械类别**：它们的权威在这里，只有一处。
///
/// 判据一句话：**这一类不是任何 App 的业务词汇，而是装配 / 路由 / 形状校验的判断结果**——
/// 谁的代码都可能得出同一个结论，所以那个词不能各拼一遍。App 自己的业务类别
/// （`not-text` / `unknown-room` / `version-conflict` / `unsupported-file-type`…）照旧归各 App，
/// 本文件不越界收编它们。
///
/// 为什么要立这个权威（2026-10-06）：`unknown-capability` 曾在**八个 App 里各拼一遍**（九处
/// 字面量），`invalid-argument` 也在路由那一层与五个枚举里各写一份。它们**逐字落盘**（进行动结果
/// 那一格），因此"同一个结论两种拼法"会让历史里出现两种事实，而没有任何判据会红。
///
/// ⚠ **解码端还没有收口**：这两个词落在盘上的那一格（`Observed.failure`）今天仍按裸字符串读回，
/// 没有封闭表校验——要收口就得给行动面一张中立封闭表（照 `ModelFailureClass` 那套做）或者一张
/// 装配期登记的类别表，那是 Core 中立面的改动，登记在 `README.md` §4 待裁。
pub const UNKNOWN_CAPABILITY: &str = "unknown-capability";

/// 参数形状不合法（机械校验；执行端因此**一次都不出网**）。见 [`UNKNOWN_CAPABILITY`] 的说明。
pub const INVALID_ARGUMENT: &str = "invalid-argument";

/// **形状读数**在随单内容里的块标签：一次请求的参数搬不动时（[`Action::malformed`]），
/// 那句"哪个键、是什么类型、怎么改"挂在它下面。
///
/// 它是**内容**的标签，不是失败分类——分类那一格用中立的 [`INVALID_ARGUMENT`]。
pub const SHAPE_READING_LABEL: &str = "shape";

/// 一次行动请求是否被**接纳**：执行端在 Prepared **之前**给出的准入判断。
///
/// 它与 [`ActionOutcome`] 是两件不同的事，混起来会说不清"到底发出去没有"：
/// [`ActionOutcome`] 是"已经发出的尝试回来了什么结果"，这里是"这次请求要不要发出去"。
/// 因此拒绝**不是**结果——被拒绝的请求没有操作身份，也不产生任何行动事实。
///
/// 它是**中立**的：Core 不认识授权、目录、具体 App 或任何供应方概念，只看一个稳定分类字符串。
/// 谁有权拒绝、按什么规则拒绝，全是执行端自己的事；Core 只负责"拒绝发生在准备之前"这个顺序。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionAdmission {
    /// 接纳：按既有顺序准备、派发、执行。
    Admitted,
    /// 拒绝：这次请求不被接纳。
    ///
    /// `class` 是稳定分类标记（由执行端给出，Core 不解释）。
    ///
    /// `content` 是**随单内容**——执行端在拒绝那一刻**已经知道**、而且这次请求正缺的那份事实
    /// （可以为空）。它走与结果内容同一形状的有界中立块，因此**不揉进 `class`**：同一故障两次
    /// 发生，`class` 仍然逐字节相同，附带的只是旁边的结构化读数。Core 不解释它，只把它记进轨迹、
    /// 好让下一次调用模型时它就在主体的视野里——"拒绝"与"缺的那份事实"不必分两轮交付。
    Refused {
        class: String,
        content: Vec<ContentBlock>,
    },
}

impl ActionAdmission {
    /// 拒绝，**不带**随单内容。
    ///
    /// 只给稳定分类：这种情况是"这次请求不该发出去"这件事本身已经说完了，旁边没有别的可给。
    pub fn refused(class: impl Into<String>) -> Self {
        Self::Refused {
            class: class.into(),
            content: Vec::new(),
        }
    }

    /// 拒绝，并随单带上那份事实（[`ActionAdmission::Refused`] 的 `content`）。
    pub fn refused_with(class: impl Into<String>, content: Vec<ContentBlock>) -> Self {
        Self::Refused {
            class: class.into(),
            content,
        }
    }

    /// 随单内容（不是拒绝时为空）。
    pub fn content(&self) -> &[ContentBlock] {
        match self {
            Self::Admitted => &[],
            Self::Refused { content, .. } => content,
        }
    }
}

/// 一次行动的结果：**类别与内容分开**。
///
/// - [`ActionOutcome::state`]：Foundation 的三分（成功 / 失败 / 无法确认），Core 只认这个；
/// - [`ActionOutcome::failure_class`]：失败时由 App 给出的**稳定分类**，Core 不解释、不分支；
/// - [`ActionOutcome::content`]：有界的中立内容块，可以为空。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionOutcome {
    state: ResultState,
    failure: Option<String>,
    content: Vec<ContentBlock>,
}

impl ActionOutcome {
    /// 明确成功，带可有界的返回内容。
    pub fn success(content: Vec<ContentBlock>) -> Self {
        Self {
            state: ResultState::Success,
            failure: None,
            content,
        }
    }

    /// 明确失败，带一个稳定分类。
    pub fn failure(class: impl Into<String>) -> Self {
        Self {
            state: ResultState::Failure,
            failure: Some(class.into()),
            content: Vec::new(),
        }
    }

    /// 明确失败，带一个稳定分类**与随单内容**。
    ///
    /// 与 [`ActionOutcome::failure`] 是同一件事的两种载荷：`class` 照旧是稳定分类标记（同一故障
    /// 两次发生逐字节相同），`content` 是**旁边的结构化读数**——"这次为什么不成、缺的是哪一项"
    /// 一类当场就能给出的事实。它**不揉进 `class`**：分类是词表，读数是内容。
    pub fn failure_with_content(class: impl Into<String>, content: Vec<ContentBlock>) -> Self {
        Self {
            state: ResultState::Failure,
            failure: Some(class.into()),
            content,
        }
    }

    /// 结果无法确认。执行端应当谨慎使用：普通错误不是未知。
    pub fn unknown() -> Self {
        Self {
            state: ResultState::Unknown,
            failure: None,
            content: Vec::new(),
        }
    }

    /// 结果类别。
    pub fn state(&self) -> ResultState {
        self.state
    }

    /// 稳定失败分类（成功或未知时为 `None`）。
    pub fn failure_class(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// 有界的中立返回内容。
    pub fn content(&self) -> &[ContentBlock] {
        &self.content
    }
}

/// 结果里的一块有界中立内容：一个标签、一段文本，**以及可选的随块材料**。
///
/// Core **不解释**标签的含义，也**不解释**材料的类型：两者都是不透明的字符串。
///
/// ## 材料是什么、不是什么
///
/// 材料是**这一块内容背后那份字节**（一块图片、一段声音…）。它存在的唯一理由是：**编码那一刻**
/// 有人要把它折成模型输入的一部分。它**不是**事实——
///
/// - **它不落盘**：账本那条投影（`store::projection`）只取 [`Self::label`] 与 [`Self::text`]，
///   因此材料**在构造上进不了经历**（不是靠约定，是靠形状）；
/// - **它不进批预算**：[`ContentBatch`] 的上限量的是**文本**（标签 ＋ 正文），材料不参与——
///   它的成本是模型的**感知成本**，不是这批注入的字节成本。
///
/// ⇒ 判据一句话：**引用进经历，字节只进编码。** 一条没有材料的块与从前逐字节相同。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentBlock {
    label: String,
    text: String,
    material: Option<Material>,
}

/// 一块内容背后那份字节：类型 ＋ 字节。
///
/// **Core 不解释类型**（与不解释标签同一条），也**不嗅探**内容——类型是产生它的那一方说的，
/// 对不对由那一方负责。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Material {
    content_type: String,
    bytes: Vec<u8>,
}

impl Material {
    /// 建立一份材料。
    pub fn new(content_type: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            content_type: content_type.into(),
            bytes,
        }
    }

    /// 类型（不透明字符串）。
    pub fn content_type(&self) -> &str {
        &self.content_type
    }

    /// 字节。
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl ContentBlock {
    /// 建立一块内容（**没有材料**）。
    pub fn new(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            text: text.into(),
            material: None,
        }
    }

    /// 给这一块挂上一份材料：**只进编码，不进经历**（见类型文档）。
    pub fn with_material(mut self, material: Material) -> Self {
        self.material = Some(material);
        self
    }

    /// 内容标签。
    pub fn label(&self) -> &str {
        &self.label
    }

    /// 内容文本。
    pub fn text(&self) -> &str {
        &self.text
    }

    /// 随块材料；没有就是 `None`。
    pub fn material(&self) -> Option<&Material> {
        self.material.as_ref()
    }
}

/// 一次活动的初始输入上限（UTF-8 字节）。
///
/// ⚠️ 这是**输入内容的字节上限**，不是模型容量，也不是上下文窗口：它只保证"这一次的起点内容
/// 是有界的"，不表示系统知道模型能处理多少内容。
pub const MAX_INPUT_BYTES: usize = 64 * 1024;

/// 建立初始输入被拒绝的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunInputError {
    /// 内容超过 [`MAX_INPUT_BYTES`]。不做截断：拒绝就是拒绝。
    TooLarge { bytes: usize },
}

/// 一次活动的**初始输入**：调用方明确提供的文本内容。
///
/// - 它是**这次经历的起点**，不是供应商专用提示词，也不是任务定义：Core 不检查"这是不是任务"、
///   不要求它是命令——它同样可以是情况说明或问题；
/// - **空输入合法**（[`RunInput::empty`]）：没有初始内容也是一次正常活动；
/// - 输入在 Run 开始时**一次确定**，Run 持有，之后不修改、不追加；每一轮视野读到的都是同一份；
/// - 内容**不**进入 Foundation 的事实（`Record`/`Ledger`），本刀也不提供任何跨 Run 存储。
///
/// 超过上限的内容无法构造出来，因此"超限"必然发生在占用活动槽、写事实、调用模型或行动之前。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RunInput {
    text: String,
}

impl RunInput {
    /// 空输入：这次活动没有初始内容。
    pub fn empty() -> Self {
        Self {
            text: String::new(),
        }
    }

    /// 用一段文本建立初始输入；超过 [`MAX_INPUT_BYTES`] 直接拒绝（不截断）。
    pub fn text(text: impl Into<String>) -> Result<Self, RunInputError> {
        let text = text.into();
        if text.len() > MAX_INPUT_BYTES {
            return Err(RunInputError::TooLarge { bytes: text.len() });
        }
        Ok(Self { text })
    }

    /// 输入原文。
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// 输入是否为空。
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// 输入的 UTF-8 字节数。
    pub fn len(&self) -> usize {
        self.text.len()
    }
}

/// 一次行动回到这次活动里的观察。
///
/// 它是**本次运行内**的痕迹：关联 `OperationId`、实际发出的 `Action`（含参数）与有界结果内容。
/// 它**不是事实记录**——不进 Ledger，不改 Foundation 的 `Record`/`ActionResult`，也不做完整
/// Trace 持久化。运行内的经历**只追加**：先发生的观察不会因为之后的能力变化而消失或被改写。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    operation: OperationId,
    action: ActionId,
    params: ActionParams,
    outcome: ActionOutcome,
}

impl Observation {
    /// 用 Core 自己记载的身份与参数 + 执行端交回的结果组装一次观察。
    pub fn new(operation: OperationId, action: &Action, outcome: ActionOutcome) -> Self {
        Self {
            operation,
            action: action.id().clone(),
            params: action.params().clone(),
            outcome,
        }
    }

    /// 这次观察对应的操作身份（由 Core 产生）。
    pub fn operation(&self) -> &OperationId {
        &self.operation
    }

    /// 这次实际发出的行动身份。
    pub fn action(&self) -> &ActionId {
        &self.action
    }

    /// 这次实际发出的参数。
    pub fn params(&self) -> &ActionParams {
        &self.params
    }

    /// 完整结果。
    pub fn outcome(&self) -> &ActionOutcome {
        &self.outcome
    }

    /// 结果类别。
    pub fn state(&self) -> ResultState {
        self.outcome.state()
    }
}

/// 一批外层注入的内容：具名文本块，可以为空。
///
/// 上限是**宿主接口边界**，不是模型容量：整批文本（**含标签**）不超过
/// [`ContentBatch::MAX_BYTES`] 字节、块数不超过 [`ContentBatch::MAX_BLOCKS`]。
/// 校验发生在构造阶段，超限**整体拒绝**——不静默截断、不产生半批。
///
/// 拒绝之后调用点该退回什么，见 [`ContentBatch::of_or_not_delivered`]：**空批次**不是合格答案
/// （它让那一整块内容无声消失），合格答案是"这一块超限未给"这条读数。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ContentBatch {
    blocks: Vec<ContentBlock>,
    source_failed: bool,
}

/// 构造一批注入内容被拒绝的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentBatchError {
    /// 块数超过上限。
    TooManyBlocks { blocks: usize },
    /// 整批文本（含标签）超过上限。
    TooLarge { bytes: usize },
}

/// 一块内容没能交付时那句读数的固定前半句（见 [`ContentBatch::not_delivered`]）。
const NOT_DELIVERED: &str = "超限未给";

/// 那条读数的标签上限（UTF-8 字节，按字符边界截断）。
///
/// 它只为一件事实服务：**读数自己一定装得下**。读数正文是一句常数，标签封顶之后整块远小于
/// [`ContentBatch::MAX_BYTES`]。本 crate 的调用点给的都是短常量标签，因此这条截断在正常路径上
/// 永不生效——它是"诚实读数不许自己变成静默丢"的兜底。
const NOT_DELIVERED_LABEL_MAX_BYTES: usize = 64;

/// 把标签截到 [`NOT_DELIVERED_LABEL_MAX_BYTES`]，且不切开一个字符。
fn bounded_label(label: &str) -> &str {
    if label.len() <= NOT_DELIVERED_LABEL_MAX_BYTES {
        return label;
    }
    let mut end = NOT_DELIVERED_LABEL_MAX_BYTES;
    while !label.is_char_boundary(end) {
        end -= 1;
    }
    &label[..end]
}

/// 一批块的**文本字节数**（标签计入）：这是上限 [`ContentBatch::MAX_BYTES`] 唯一的口径，
/// 校验与读数都问这里，不各算一遍。
fn text_bytes(blocks: &[ContentBlock]) -> usize {
    blocks
        .iter()
        .map(|block| block.label().len() + block.text().len())
        .sum()
}

impl ContentBatch {
    /// 一批最多多少块。
    pub const MAX_BLOCKS: usize = 64;
    /// 一批的文本上限（UTF-8 字节，标签计入）。
    pub const MAX_BYTES: usize = 64 * 1024;

    /// 空批次：这次没有注入内容。它不产生任何记录。
    pub fn empty() -> Self {
        Self::default()
    }

    /// 用一组块构造一批注入内容；超限直接拒绝。
    pub fn of(blocks: Vec<ContentBlock>) -> Result<Self, ContentBatchError> {
        Self::checked(blocks, false)
    }

    /// 外层来源**自己没能读取成功**时返回的批次。
    ///
    /// 它是一个内容事实（"来源报告它读不到"），**不是**模型错误、**不是**工具/行动结果，
    /// 也**不是**成功：Core 只把它当作一条来源失败的内容记进轨迹。
    pub fn source_failure(
        label: impl Into<String>,
        detail: impl Into<String>,
    ) -> Result<Self, ContentBatchError> {
        Self::checked(vec![ContentBlock::new(label, detail)], true)
    }

    /// 交付一批内容；**装不下（超限）时不留空批次**，改用 [`Self::not_delivered`] 那条读数替它。
    ///
    /// 它是"块级静默丢"的**唯一**处置点：调用点此前写成"构造不出来就退回空批次"，于是超限时那一整块
    /// 内容**无声消失**——读主体的视野的人与自己都只看到"这里什么都没有"，分不清"没有"与"没读到"。
    ///
    /// `label` 是**这个来源自己的**标签（各调用点各给各的），它既进读数、也当那一块的标签。
    ///
    /// 它**不改变** [`Self::of`] 的校验语义：超限仍然是拒绝、仍然不产生半批。换掉的只是调用点那一步
    /// "退回什么"。
    pub fn of_or_not_delivered(blocks: Vec<ContentBlock>, label: impl AsRef<str>) -> Self {
        let label = label.as_ref();
        match Self::checked(blocks, false) {
            Ok(batch) => batch,
            Err(refusal) => Self::not_delivered(label, refusal),
        }
    }

    /// 一块内容**没能交付**的读数：它本该在这里，但它自己超出了批次上限而**整块**给不出来。
    ///
    /// 它不是来源失败（`source_failed` 仍为 `false`）：这次交付是成功的，成功的内容就是"那一块没给"
    /// 这句话本身。它呈现为
    /// `<label>: 超限未给（原始 N B）`（块数超限时给块数），`label` 由调用方按来源给。
    ///
    /// **它一定装得下**——读数是一句常数（约 30 字节），标签按 [`NOT_DELIVERED_LABEL_MAX_BYTES`]
    /// 截断，且只有一块，因此直接装配而不必再过一次校验：万一它自己也装不下，那才是真的无声消失。
    #[must_use]
    pub fn not_delivered(label: impl AsRef<str>, refusal: ContentBatchError) -> Self {
        let reason = match refusal {
            ContentBatchError::TooLarge { bytes } => format!("原始 {bytes} B"),
            ContentBatchError::TooManyBlocks { blocks } => format!("原始 {blocks} 块"),
        };
        Self::reading(label.as_ref(), &reason)
    }

    /// 同 [`Self::not_delivered`]，只是原始规模**直接给数**。
    ///
    /// "把两批内容合成一批"的场合用它：那里被拒绝的是**拼起来的整批**，而没能交付的只有其中
    /// 一半——读数要报的是**那一半**多大，不是两半加起来的数。
    #[must_use]
    pub fn not_delivered_bytes(label: impl AsRef<str>, bytes: usize) -> Self {
        Self::reading(label.as_ref(), &format!("原始 {bytes} B"))
    }

    /// 这批内容的字节数（标签计入；与构造时的校验同一口径）。
    pub fn bytes(&self) -> usize {
        text_bytes(&self.blocks)
    }

    /// 那条读数的**唯一**装配点：标签封顶、块只有一块，因此它一定装得下。
    fn reading(label: &str, reason: &str) -> Self {
        Self {
            blocks: vec![ContentBlock::new(
                bounded_label(label),
                format!("{NOT_DELIVERED}（{reason}）"),
            )],
            source_failed: false,
        }
    }

    fn checked(blocks: Vec<ContentBlock>, source_failed: bool) -> Result<Self, ContentBatchError> {
        if blocks.len() > Self::MAX_BLOCKS {
            return Err(ContentBatchError::TooManyBlocks {
                blocks: blocks.len(),
            });
        }
        let bytes = text_bytes(&blocks);
        if bytes > Self::MAX_BYTES {
            return Err(ContentBatchError::TooLarge { bytes });
        }
        Ok(Self {
            blocks,
            source_failed,
        })
    }

    /// 批里的块，按给出顺序。
    pub fn blocks(&self) -> &[ContentBlock] {
        &self.blocks
    }

    /// 这批是不是空批次。
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// 这批是不是外层来源报告的读取失败。
    pub fn is_source_failure(&self) -> bool {
        self.source_failed
    }
}

/// 两处批次都递交：**别人的内容在前、本件自己的读数在后**，一块都不丢。
///
/// 它是"两处内容都要交上去"这条纪律的**唯一**实现：处境链上的每一层委托都调它一次，
/// 而不是各写一遍（两份实现迟早会分叉）。它不排队、不去重、不做优先级，因此不是管道：
/// 输入就是两批内容，输出就是一批内容。
///
/// **为什么它住在 Core**：它是一条**关于 [`ContentBatch`] 的规则**，而那个类型是 Core 定义的
/// 注入接缝载荷。它只碰 [`ContentBatch`] 与 [`ContentBlock`]，不认识任何别的模块，因此放在这里
/// 不添任何依赖；而调用它的那些显示区件本来就已经依赖 Core，于是这条规则**一处定义、零新增边**。
///
/// 参数名字就是让位规则：`other` 是**别人的内容**（永不放弃），`own` 是这一层自己的读数
/// （超出批次容量时让位）。两个例外都指向这同一句话：
///
/// 1. 任一处报告"来源读不到"（`source_failure`）⇒ 原样交回**报告失败的那一批**。一个批次只有
///    一个来源判定（这是接缝的词汇，不是调用方能改的），把一条成功的机械读数并进"读不到"的批次，
///    等于告诉模型"连同这条读数都读不到"。
///    ⚠ **这一格今天不可达**（如实登记）：装配期已保证每个在场能力的来源都在场，所以走不到这里；
///    **写全它**是为了将来换内层时**不许静默丢内容**——不是一个已验证的失败路径。
/// 2. 拼接后超过批次上限（块数或字节）⇒ 交回 **`other` ＋ 一条读数**。自己那一半下一轮还会重摆，
///    别人的内容只有这一份——但**让掉的那一份要留下名字**：那条读数写的就是"它超限未给（原始
///    N B）"，否则让位与消失在主体的视野里是同一件事。**这一条是可达的**：宿主输入顶到
///    [`ContentBatch::MAX_BYTES`] 时就会走到这里（用例
///    `a_host_input_near_the_batch_limit_is_still_delivered` 把它钉住）——因此调用方必须把
///    **别人的那一份**放在第一个参数上，放反了就是"静默吞掉宿主显式交出的文本"。
///
///    第 2 条例外有**一个退无可退的角**：`other` 一个人就占满了整批预算（字节或块数到顶），
///    连一行读数都塞不进去 ⇒ 只能交回 `other`。它不是取舍，是上限本身：要留下读数就得动
///    `other` 的内容，而那正是"别人的内容只有这一份"要保的东西。
///
/// 第 1 条例外今天**不可达**（各处的 `other` 与 `own` 都由"构造不出失败批次"的来源造出来），
/// 写全它是为了"将来换内层时不许静默丢内容"。
pub fn combine(other: ContentBatch, own: ContentBatch) -> ContentBatch {
    if other.is_source_failure() {
        return other;
    }
    if own.is_source_failure() {
        return own;
    }
    let mut blocks = other.blocks().to_vec();
    blocks.extend(own.blocks().iter().cloned());
    match ContentBatch::of(blocks) {
        Ok(batch) => batch,
        Err(_) => {
            // 让位可以，**无声消失不行**：把"它超限未给"这条读数并进 `other` 再装配一次。
            //
            // 读数报的是**让位那一半原本多大**（`own.bytes()`），不是拼起来的总数——被拒绝的是
            // 整批，没能交付的却只有这一半。让位者的名字也取自它自己的块标签（多块就都写上）。
            let label = own
                .blocks()
                .iter()
                .map(ContentBlock::label)
                .collect::<Vec<_>>()
                .join("+");
            let notice = ContentBatch::not_delivered_bytes(label, own.bytes());
            let mut retained = other.blocks().to_vec();
            retained.extend(notice.blocks().iter().cloned());
            match ContentBatch::of(retained) {
                Ok(batch) => batch,
                // `other` 一个人顶满了整批预算 ⇒ 连一行读数都塞不进去（见上面文档里那个角）。
                Err(_) => other,
            }
        }
    }
}

/// 一次注入发生在哪一段。由 **Core** 标记，外层不能伪造。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InjectionStage {
    /// 活动开始时的注入（RunStarted 登记成功之后、第一个 Turn 之前）。
    RunStart,
    /// 某一轮真正调用模型之前的注入。
    Turn,
}

/// 注入接缝拿到的**只读**上下文：Core 给外层的全部信息。
///
/// 它刻意不含 `&mut` 历史、行动端口、模型端口或账本：外层拿不到修改历史、行动或终态的接口，
/// 只能读身份、轮次与资源边界，然后返回一批内容。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InjectionContext {
    agent: AgentId,
    run: RunId,
    turn: Option<Turn>,
    limits: Limits,
    /// 这一轮**当前可用**的能力面（TASK_68 P2）。
    ///
    /// 它是 Core 交给外层的**只读处境**的一部分（不是端口、不是 `&mut` 历史），
    /// 让贡献者能产出"主体有哪些动作"这类内容——能力每轮都可能变（展开会追加），
    /// 所以每一轮都随上下文重新交一份；Run 级那一轮给的是**开场时**的能力。
    capabilities: Capabilities,
}

impl InjectionContext {
    /// 这次注入属于哪个 Agent。
    pub fn agent(&self) -> &AgentId {
        &self.agent
    }

    /// 这次注入属于哪次活动。
    pub fn run(&self) -> &RunId {
        &self.run
    }

    /// 这是哪一轮的注入；活动开始时的注入是 `None`。
    pub fn turn(&self) -> Option<Turn> {
        self.turn
    }

    /// 这次活动的资源边界。
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// 这一轮当前可用的能力面（只读）。贡献者据此产出"主体有哪些动作"这类内容；
    /// 能不能产出、产出什么，仍由外层自己决定（读不到不等于必须写点什么）。
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// 这一轮是不是**首轮**（RunStart 之后第一次真正调用模型的那一轮）。
    ///
    /// 它是"开场关头的处境"里的一个只读事实——外层不必知道轮次表示法就能问这一句
    /// （TASK_68 P2：首轮那份动作目录只发一次，判据就是它）。
    pub fn is_opening_turn(&self) -> bool {
        self.turn == Some(Turn::FIRST)
    }
}

/// 上下文注入接缝：**外层提供内容，Core 决定时机与顺序**。
///
/// 两个方法都是同步的，返回已经校验过的中立内容批次；返回空批次是合法的（等于这次没有注入）。
/// 实现由外层持有；Core 不保存它、不做注册、不保证清除它自己缓存的东西。
pub trait ContextSource {
    /// 活动开始时恰好一次（在活动登记成功之后、第一个 Turn 之前）。
    fn on_run_start(&mut self, context: &InjectionContext) -> ContentBatch;

    /// 每一次真正要调用模型的 Turn 之前恰好一次，附带这一轮的真实序号。
    fn before_turn(&mut self, context: &InjectionContext) -> ContentBatch;
}

/// 没有任何注入的来源：既有入口都用它，行为与"没有这个接缝"完全一致。
#[derive(Debug, Default)]
pub struct NoContext;

impl ContextSource for NoContext {
    fn on_run_start(&mut self, _context: &InjectionContext) -> ContentBatch {
        ContentBatch::empty()
    }

    fn before_turn(&mut self, _context: &InjectionContext) -> ContentBatch {
        ContentBatch::empty()
    }
}

/// 一个确认点。它属于 Core 自己的词汇表，不是存储格式。
///
/// 它存在的唯一理由是让调用方**如实区分**"哪一步没有被确认为耐久"。压成一个布尔值会逼出
/// 两种错误说法：把"结束没确认"说成"活动没结束"，或把"封存没确认"说成"结束没确认"。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitStage {
    /// 活动开始。
    RunStart,
    /// 开场输入。
    Input,
    /// 一条运行内经历（注入或观察）。
    Trace,
    /// 行动选择与参数（执行之前的确认点）。
    ActionSelected,
    /// 行动已经发出。
    ActionDispatched,
    /// 行动的真实结果。
    ActionResult,
    /// 活动如何收场。
    Ending,
    /// 结束确认之后的封存。
    Seal,
    /// **静态头装订**：装配层声明的那一件事，Core 不认识它的内容。
    ///
    /// 它不在 Core 的提交序列里——这一笔由**会话装配层**自己发起（出厂说明是环境家具，不是
    /// 执行层的中立概念）。放在这张表里只有一个理由：让"装订没被确认"像别的确认点一样有一个
    /// **稳定阶段名**，而不是被折进 `Trace` 去冒充一条普通的运行内经历。
    StaticHead,
}

impl CommitStage {
    /// 稳定分类标记（给外层做机械描述，不参与领域判断）。
    pub fn as_str(&self) -> &'static str {
        match self {
            CommitStage::RunStart => "run-start",
            CommitStage::Input => "input",
            CommitStage::Trace => "trace",
            CommitStage::ActionSelected => "action-selected",
            CommitStage::ActionDispatched => "action-dispatched",
            CommitStage::ActionResult => "action-result",
            CommitStage::Ending => "ending",
            CommitStage::Seal => "seal",
            CommitStage::StaticHead => "static-head",
        }
    }
}

/// 经历提交被拒绝的原因。它**不带存储细节**：Core 不把存储路径、文件名或数据库错误带进领域。
/// 它只带一个中立的**阶段**：调用方据此知道"停在哪一步"，而不知道也不需要知道底层为什么失败。
///
/// 它与 [`ModelError`] 是两件不同的事：保存失败不是模型失败，也不能被伪装成模型失败。
///
/// **一个阶段失败不等于"这一步什么都没留下"**：一次提交可能分几次写下（例如收场先写经历、
/// 再写事实），其中一部分可能已经落盘。因此调用方可以断言"这一步没有被完整确认"，**不得**
/// 断言"这一步的记录一定不存在"。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommitError {
    stage: CommitStage,
}

impl CommitError {
    /// 建立一个"停在某一步"的提交失败。
    pub fn at(stage: CommitStage) -> Self {
        Self { stage }
    }

    /// 停在哪一步。
    pub fn stage(&self) -> CommitStage {
        self.stage
    }

    /// 这次失败是否发生在"活动已经按确认的终态结束"之后。
    ///
    /// 只有封存属于这一类：结束事实已经被确认，缺的只是"到此为止"的封存标记。
    pub fn after_confirmed_ending(&self) -> bool {
        matches!(self.stage, CommitStage::Seal)
    }
}

/// **经历提交接缝**：只表达"把这条经历／事实提交为已确认"与"提交失败"。
///
/// 它刻意**不是**上下文注入接缝：[`ContextSource`] 的时机是"往这一轮视野里放内容"，
/// 这里是"把已经发生的经历确认为耐久"。两者不能互相顶替——拿注入钩子去写盘，或用写盘
/// 钩子去改视野，都会破坏"视野是只读投影"这条边界。
///
/// Core 只认识中立的经历条目与事实身份，**不认识**文件路径、历史工具或数据库；
/// 实现由外层提供，组合入口显式接上（本文件刻意不点名任何边界模块）。
///
/// 每个方法都必须在**推进到下一下载入点之前**返回：返回 `Err` 表示"这一步没有被确认为
/// 耐久"，Core 会停止继续，不重发已经发出的动作、不改写已确认前缀、不伪造终态。
pub trait CommitSeam {
    /// 这次活动已经开始（在第一个 Turn 之前确认）。
    fn commit_run_start(&mut self, agent: &AgentId, run: &RunId) -> Result<(), CommitError>;

    /// 这次活动的初始输入（在第一次模型调用之前确认）。
    fn commit_input(&mut self, run: &RunId, input: &RunInput) -> Result<(), CommitError>;

    /// 一条运行内经历（注入或观察）；在它发生之后、下一次模型调用之前确认。
    fn commit_trace(
        &mut self,
        run: &RunId,
        turn: Option<Turn>,
        entry: &TraceEntry,
    ) -> Result<(), CommitError>;

    /// Agent 选择了一次行动、以及它的参数：这是**执行之前**的确认点。
    ///
    /// 没有这一步，就无法把"已经发出过的外部尝试"与任何已确认记录关联起来。
    fn commit_action_selected(
        &mut self,
        run: &RunId,
        operation: &OperationId,
        action: &Action,
    ) -> Result<(), CommitError>;

    /// 一次行动已经发出。
    fn commit_action_dispatched(
        &mut self,
        run: &RunId,
        operation: &OperationId,
    ) -> Result<(), CommitError>;

    /// 一次行动的真实结果。
    fn commit_action_result(
        &mut self,
        run: &RunId,
        operation: &OperationId,
        outcome: &ActionOutcome,
    ) -> Result<(), CommitError>;

    /// 这次活动如何收场。
    ///
    /// `cost` 是**搬进来的代价读数**（时长 ＋ 用量）：Core 不认识这两样，也不去算它们——
    /// 调用方（持有那两个来源的那一层）算好递进来，本接缝只把它带进事实行。不关心代价的
    /// 调用方给 [`RunCost::default`]（两格都"没有被记"，不是"等于零"）。
    fn commit_ending(
        &mut self,
        run: &RunId,
        outcome: Outcome,
        cost: RunCost,
    ) -> Result<(), CommitError>;

    /// 结束事实确认之后的**封存**：它不等于覆盖原记录，只是给历史一个明确的"到此为止"。
    fn seal(&mut self, run: &RunId) -> Result<(), CommitError>;
}

/// 什么都不提交的接缝：既有入口都用它，行为与"没有这个接缝"完全一致。
#[derive(Debug, Default)]
pub struct NoCommit;

impl CommitSeam for NoCommit {
    fn commit_run_start(&mut self, _agent: &AgentId, _run: &RunId) -> Result<(), CommitError> {
        Ok(())
    }

    fn commit_input(&mut self, _run: &RunId, _input: &RunInput) -> Result<(), CommitError> {
        Ok(())
    }

    fn commit_trace(
        &mut self,
        _run: &RunId,
        _turn: Option<Turn>,
        _entry: &TraceEntry,
    ) -> Result<(), CommitError> {
        Ok(())
    }

    fn commit_action_selected(
        &mut self,
        _run: &RunId,
        _operation: &OperationId,
        _action: &Action,
    ) -> Result<(), CommitError> {
        Ok(())
    }

    fn commit_action_dispatched(
        &mut self,
        _run: &RunId,
        _operation: &OperationId,
    ) -> Result<(), CommitError> {
        Ok(())
    }

    fn commit_action_result(
        &mut self,
        _run: &RunId,
        _operation: &OperationId,
        _outcome: &ActionOutcome,
    ) -> Result<(), CommitError> {
        Ok(())
    }

    fn commit_ending(
        &mut self,
        _run: &RunId,
        _outcome: Outcome,
        _cost: RunCost,
    ) -> Result<(), CommitError> {
        Ok(())
    }

    fn seal(&mut self, _run: &RunId) -> Result<(), CommitError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Model 契约：中立响应、单个决定与必要的协议续接数据
// ---------------------------------------------------------------------------

/// 协议续接载荷的**中立上限**（UTF-8 字节）。
///
/// 它是机械边界，不是模型容量：超限**整体拒绝**，不静默截断——截断过的续接数据看起来像一份
/// 完整的载荷，而它其实已经坏了。
pub const PROTOCOL_PAYLOAD_BYTES: usize = 64 * 1024;

/// 构造协议续接数据被拒绝的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolDataError {
    /// 没有说明这段载荷编码在哪一个协议里：缺了它，载荷无法被任何一层正确解释。
    MissingProtocol,
    /// 载荷为空：一个"存在但没有内容"的续接字段不是一份可解释的载荷。
    EmptyPayload,
    /// 载荷超过 [`PROTOCOL_PAYLOAD_BYTES`]。
    TooLarge { bytes: usize },
}

/// 一次响应里**必要的协议续接数据**：编码所属协议、该协议的版本与载荷。
///
/// 它是**中立载体**，不是 Core 的领域概念：
///
/// - Core 只**搬运**它，并实施唯一一条**中立限额**（载荷字节数）。协议名与版本号的含义、
///   载荷的结构与合法性，全部归 Adapter 解释，Core 一个记号都不解释；
/// - 它**不是凭据**：不装认证信息、不装密钥，也不是整份交换；它也**不替代**
///   [`OperationId`]——操作身份由 Core 自己产生，续接数据不能用来指代一次外部尝试；
/// - 它**不进 Foundation 的事实**（`Record`/`Ledger`），也**不**经普通历史、诊断正文或日志
///   展示。它随经历保存（见 [`TraceEntry::ModelResponse`]），并由 Adapter 用来重建下一次请求，
///   因此它**可能**再次进入模型上下文——"不作为普通记忆呈现"说的是**呈现方式**，不是
///   "绝不被模型接收"。
///
/// `Debug` 只打印协议名、版本与载荷长度，**不打印载荷本身**：调试输出不是展示载荷的地方。
#[derive(Clone, PartialEq, Eq)]
pub struct ProtocolData {
    protocol: String,
    version: u32,
    payload: String,
}

impl std::fmt::Debug for ProtocolData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtocolData")
            .field("protocol", &self.protocol)
            .field("version", &self.version)
            .field("payload_bytes", &self.payload.len())
            .finish()
    }
}

impl ProtocolData {
    /// 用"所属协议 + 版本 + 载荷"建立一份续接数据：空协议名、空载荷与超限都明确拒绝。
    pub fn new(
        protocol: impl Into<String>,
        version: u32,
        payload: impl Into<String>,
    ) -> Result<Self, ProtocolDataError> {
        let protocol = protocol.into();
        if protocol.is_empty() {
            return Err(ProtocolDataError::MissingProtocol);
        }
        let payload = payload.into();
        if payload.is_empty() {
            return Err(ProtocolDataError::EmptyPayload);
        }
        if payload.len() > PROTOCOL_PAYLOAD_BYTES {
            return Err(ProtocolDataError::TooLarge {
                bytes: payload.len(),
            });
        }
        Ok(Self {
            protocol,
            version,
            payload,
        })
    }

    /// 这段载荷编码在哪一个协议里（名字由 Adapter 拥有，Core 只搬运）。
    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    /// 这个协议的版本。
    pub fn version(&self) -> u32 {
        self.version
    }

    /// 载荷原文。Adapter 用它重建请求；Core 只搬运、只限额。
    pub fn payload(&self) -> &str {
        &self.payload
    }
}

/// 一次响应里的中立决定。
///
/// 三种形状互斥：结束与普通行动不能并存（那是两份互相矛盾的意图）。**一个行动决定可以带
/// 多个动作**——原生协议本来就允许一次回复请求多个工具调用，Core 按模型给出的顺序**依次**
/// 处理它们，各自是一次独立的外部尝试（各有自己的操作身份）。
///
/// 这里刻意**没有** `Continue` 这样的名字：交回控制**不等于**自动再调用一次模型。纯正文映射成
/// [`Decision::Yield`]，它明确说的是"交回控制，不结束、不请求任何行动"。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// 请求**一个或多个**行动（按顺序处理）：Core 对每一个走准入、准备、派发、结果四步。
    ///
    /// **不变量：至少一个**。空列表不是"什么都不做"（那是 [`Decision::Yield`]），而是构造不出来
    /// 的形状——用 [`Decision::act`] 建立，它给不出空的。
    Act(Vec<Action>),
    /// 主体自己决定结束这次活动。
    Finish(SubjectEnding),
    /// 纯正文：交回控制，**不结束**这次活动，也不请求任何行动。
    ///
    /// 它必须带着正文；"既没有正文、也没有行动或结束意图"的空响应不是一次等待，是一次失败
    /// （判据由 Adapter 在翻译时给出，Core 不再另立一份）。
    Yield,
}

impl Decision {
    /// 一轮里能表达的动作数上限：它就是操作身份里序号字段的取值范围。
    ///
    /// 序号是身份的一部分（同一轮的第几个），装不下就不表达——**不截断、不折叠**。
    pub const MAX_ACTIONS: usize = u16::MAX as usize + 1;

    /// 用**有序**的动作列表建立一次行动决定；空列表或超过 [`Decision::MAX_ACTIONS`] 给 `None`。
    ///
    /// 这里挡住这两件事，是为了让"动作数在可表达的范围内"成为**构造上的事实**：别处
    /// （落盘、恢复、执行、配对）都不必再判一次。
    pub fn act(actions: Vec<Action>) -> Option<Self> {
        if actions.is_empty() || actions.len() > Self::MAX_ACTIONS {
            return None;
        }
        Some(Decision::Act(actions))
    }

    /// 决定请求的那些动作（不是行动决定时为空切片）。
    pub fn actions(&self) -> &[Action] {
        match self {
            Decision::Act(actions) => actions,
            _ => &[],
        }
    }
}

/// Model 返回的**一次中立响应**：正文、必要的协议续接数据、一个中立决定。
///
/// **正文绝不解析成控制信息**：它就是给主体与宿主看的文字，可以同时伴随一次行动或一次结束，
/// 也可以单独存在（[`Decision::Yield`]）。谁想从正文里读出指令，谁就把外界内容当成了自己的
/// 上级——那是这一层明确不做的事。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelOutput {
    content: String,
    protocol: Option<ProtocolData>,
    decision: Decision,
}

impl ModelOutput {
    /// 用正文、续接数据与决定组装一次响应。
    pub fn new(
        content: impl Into<String>,
        protocol: Option<ProtocolData>,
        decision: Decision,
    ) -> Self {
        Self {
            content: content.into(),
            protocol,
            decision,
        }
    }

    /// 不带正文的一次行动（**单个**动作）。
    pub fn act(action: Action) -> Self {
        Self::new(String::new(), None, Decision::Act(vec![action]))
    }

    /// 不带正文的一次行动（**多个**动作，按给定顺序）。
    ///
    /// 空列表给 `None`：它不是"什么都不做"，而是构造不出来的形状（要交回控制就用
    /// [`ModelOutput::say`]）。
    pub fn act_many(actions: Vec<Action>) -> Option<Self> {
        Decision::act(actions).map(|decision| Self::new(String::new(), None, decision))
    }

    /// 不带正文的一次主体结束。
    pub fn finish(ending: SubjectEnding) -> Self {
        Self::new(String::new(), None, Decision::Finish(ending))
    }

    /// 纯正文：交回控制（[`Decision::Yield`]），不结束、不请求任何行动。
    pub fn say(content: impl Into<String>) -> Self {
        Self::new(content, None, Decision::Yield)
    }

    /// 这次响应的正文（可以为空：一次不带正文的行动或结束）。
    pub fn content(&self) -> &str {
        &self.content
    }

    /// 这次响应带的协议续接数据；没有就是 `None`。
    pub fn protocol(&self) -> Option<&ProtocolData> {
        self.protocol.as_ref()
    }

    /// 这次响应的单个决定。
    pub fn decision(&self) -> &Decision {
        &self.decision
    }
}

/// 一次模型响应在运行轨迹里的**记录单元**：它属于哪一轮，以及那一次中立响应本身。
///
/// 它是"模型到底说了什么"的记录，与"Core 据此做了什么"（[`TraceEntry::Observation`]、
/// [`TraceEntry::Refused`]）分开：决定与执行各留各的痕迹，谁也不冒充谁。后续的准备、派发与
/// 结果仍然分别记录，不合并、不补齐。
///
/// **配对**：一次请求了行动的响应**紧邻**它的回执——要么是 [`TraceEntry::Observation`]，
/// 要么是准入拒绝的 [`TraceEntry::Refused`]；两者之间不插入别的输入。因此从轨迹重建请求时，
/// assistant 帧与其回执按相邻关系配对，既不需要猜，也不允许捏造没有发生过的调用。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelResponse {
    turn: Turn,
    output: ModelOutput,
}

impl ModelResponse {
    /// 把一次中立响应记到某一轮上。
    pub fn new(turn: Turn, output: ModelOutput) -> Self {
        Self { turn, output }
    }

    /// 这次响应属于哪一轮（真实轮次，不是推算出来的）。
    pub fn turn(&self) -> Turn {
        self.turn
    }

    /// 那次中立响应本身。
    pub fn output(&self) -> &ModelOutput {
        &self.output
    }

    /// 这次响应的正文。
    pub fn content(&self) -> &str {
        self.output.content()
    }

    /// 这次响应带的协议续接数据。
    pub fn protocol(&self) -> Option<&ProtocolData> {
        self.output.protocol()
    }

    /// 这次响应的单个决定。
    pub fn decision(&self) -> &Decision {
        self.output.decision()
    }
}

/// `Transport` 档内的**细分**：按"接下来该动哪一手"分档，不按外部形状逐个建档。
///
/// **分档判据（主人 2026-09-19 裁）**：同一行动归一档；每个变体都要能回答"这一档对应的下一步
/// 行动是什么"——答案写在变体上。
///
/// 为什么需要它：`Transport` 原来把 DNS / 连接被拒 / 超时 / 读写中断并成一档 ⇒ **根因不可查**
/// （2026-09-19 实测：同机同 key 的最小调用连续 6 次成功，而主体的调用在两处家都是
/// `transport-failure`——分不出是哪一种）。
///
/// ⚠ **每一档都有实测出来的机械判据，且都进了常驻用例**（TASK_60）：
/// 折叠表在**适配层**（"外部形状 → 档内细分"只在那里发生，Core 只看封闭枚举）。
/// **没有这样判据的档不发布**——例如"TLS 握手失败"：同一个逻辑故障在**经代理**时给
/// `io::ErrorKind::InvalidData`、**绕代理**时给 `Other`（两种签名都量到过）⇒ 没有稳定判据
/// ⇒ 它**不在这里**，那种情况归"未细分"（`None`）。宁可少一档，也不留一个有时撒谎的档。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportDetail {
    /// 域名解析不出结果 ⇒ 下一步：**先立即重采一次**（解析器可能只是抖了一下）；仍不行再查域名 / DNS / 网络出口。
    ///
    /// **只有 Windows 有稳定读数**（Winsock 的 11001 族系统错误码）；别的平台没有 ⇒ 归未细分。
    Dns,
    /// 对方没在听（连接被拒）⇒ 下一步：**先立即重采一次**（对方可能正在重启）；仍被拒再查地址端口 / 服务在不在 / 防火墙。
    Refused,
    /// 超时 ⇒ 下一步：**先立即重采一次**（也可能只是这一次慢）；仍超时再查网络快慢与超时设置。
    Timeout,
    /// 连接建立之后的读写中断 ⇒ 下一步：**先立即重采一次**（多半是瞬态）；仍断再查链路稳定性。
    Io,
}

/// `HttpStatus` 档内的**细分**：同样按"接下来该动哪一手"分档。
///
/// 为什么需要它：非 2xx 原来只有一档、**连状态码都不留** ⇒ 401（换钥匙）与 429（等一会儿）
/// 与 5xx（重试/上报）在读数里长得一模一样，而它们的下一步动作完全不同。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpStatusDetail {
    /// 凭据 / 账号这一侧要人动手（401 未授权、403 拒绝、402 欠费）
    /// ⇒ 下一步：换一把钥匙，或查账号余额与权限。
    Credential,
    /// 被限流（429）⇒ 下一步：等一会儿再试（这一档重试是理性选择，不是浪费）。
    RateLimited,
    /// 对方自己出问题（5xx）⇒ 下一步：先重试；持续如此就上报供应商。
    ServerSide,
    /// 请求被拒（其余 4xx）⇒ 下一步：改请求（模型名、参数、长度）。
    RequestRejected,
}

/// Model 这一层的失败**分类**：Core 自己的封闭词汇。
///
/// 九个取值与适配层的稳定分类名**逐字同名**：适配层分出来的那一层信息因此不必在边界上被压扁，
/// 也不需要第二套名字（一次映射、零改名、零损失）。分类名本身是中立的——它描述的是"模型这一层
/// 发生了什么"，与具体服务实现无关（任何模型服务都可能回"非 2xx"）。
///
/// **它是事实，不是日志**：只有作为事实，它才能跟着收场进盘、进终端、进诊断与恢复报告；
/// 留在适配层的一行 `eprintln!` 里，就等于让跨进程的调查去读侧信道。
///
/// **两个变体带档内细分**（TASK_60）：`Transport` 与 `HttpStatus` 各带一个 `Option<…>`——
/// `None` 的语义是"**已知这一类失败，但没细分到哪一档**"（旧记录就是这一形态，所以解码必须认它），
/// `Some(…)` 是多知道了一层。其余七档没有细分，保持原样。
/// 形态上仍然：`Copy`、封闭、`as_str`/`from_str` 对称、**不含任何自由文本**
/// （外部错误码与原文仍留在适配层——那条纪律没动）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFailureClass {
    /// 连接、发送或读取本身失败（够不着模型服务）。载荷见 [`TransportDetail`]。
    ///
    /// ⚠ **`None`（未细分）不进重试族**（判据见 `model_retry::is_transient_failure`）：
    /// 认不出可能是"有持久原因"的那一类（例如证书／TLS 握手——它刻意没发布，见 [`TransportDetail`]），
    /// 而对有持久原因的东西重采一次不会变。⇒ 那一档**先取证**：形态指纹那一行就是它的取证面
    /// （TASK_60 §6），量出稳定形状之后再决定要不要并进重试族。
    Transport(Option<TransportDetail>),
    /// 服务端回了非 2xx。载荷见 [`HttpStatusDetail`]。
    HttpStatus(Option<HttpStatusDetail>),
    /// 响应不是我们需要的形状（读不出约定的结构）。
    UnparsableResponse,
    /// 形状不符：取值不是字符串、参数不是对象、类型不是函数调用、结束取值不在封闭表里、
    /// 结束参数多一个字段、空响应。
    ContractViolation,
    /// 生成被截断或中止。
    StoppedGeneration,
    /// 一个能力标识无法作为工具名表达。
    UnsupportedToolName,
    /// 一个能力标识占用了保留的结束类工具名。
    ReservedToolName,
    /// 轨迹无法表达成一份合法请求（没有对应请求的回执、请求与回执之间插入了注入）。
    UnpairedHistory,
    /// 协议续接载荷读不懂或写不出。
    ProtocolPayload,
}

impl ModelFailureClass {
    /// 稳定分类标记（进事实、进终端、进诊断与恢复报告；与适配层同名同值）。
    ///
    /// **细分拼法**：`<分类>:<细分>`（如 `transport-failure:dns`）。
    /// 这样拼有两个好处：① **旧词是新词的前缀** ⇒ 没细分的那一形态与今天**逐字节相同**
    /// （旧记录、旧消费方都不用改）；② 整串**没有空白**，`\S+` 的消费方（宿主 stderr 行、
    /// 状态口解析器的 `failure=\S+`）一个字都不用动。
    ///
    /// 判据：**同一故障两次发生，这个字符串逐字节相同**（没有随机、没有路径、没有原文）。
    pub fn as_str(self) -> &'static str {
        match self {
            ModelFailureClass::Transport(None) => "transport-failure",
            ModelFailureClass::Transport(Some(TransportDetail::Dns)) => "transport-failure:dns",
            ModelFailureClass::Transport(Some(TransportDetail::Refused)) => {
                "transport-failure:refused"
            }
            ModelFailureClass::Transport(Some(TransportDetail::Timeout)) => {
                "transport-failure:timeout"
            }
            ModelFailureClass::Transport(Some(TransportDetail::Io)) => "transport-failure:io",
            ModelFailureClass::HttpStatus(None) => "http-status-failure",
            ModelFailureClass::HttpStatus(Some(HttpStatusDetail::Credential)) => {
                "http-status-failure:credential"
            }
            ModelFailureClass::HttpStatus(Some(HttpStatusDetail::RateLimited)) => {
                "http-status-failure:rate-limited"
            }
            ModelFailureClass::HttpStatus(Some(HttpStatusDetail::ServerSide)) => {
                "http-status-failure:server-side"
            }
            ModelFailureClass::HttpStatus(Some(HttpStatusDetail::RequestRejected)) => {
                "http-status-failure:request-rejected"
            }
            ModelFailureClass::UnparsableResponse => "unparsable-response",
            ModelFailureClass::ContractViolation => "contract-violation",
            ModelFailureClass::StoppedGeneration => "stopped-generation",
            ModelFailureClass::UnsupportedToolName => "unsupported-tool-name",
            ModelFailureClass::ReservedToolName => "reserved-tool-name",
            ModelFailureClass::UnpairedHistory => "unpaired-history",
            ModelFailureClass::ProtocolPayload => "protocol-payload",
        }
    }

    /// 从稳定标记还原（解码用）。认不出的取值就是 `None`——**不猜**，也不就近映射成一个看起来
    /// 像的类别（调用方按 fail-closed 处理）。
    ///
    /// **分词表就是权威**（TASK_60 起这同时是"解码端校验"）：旧词（无细分）与带细分的词都认，
    /// **认不出的细分明确拒绝**（`transport-failure:nope` ⇒ `None`，不会被读成"某一档"）。
    /// 新增取值必须同一刀补在这里——`as_str` 与它逐条对称，有测试钉住。
    ///
    /// 名字按设计固定。它**不**实现 `std::str::FromStr`：那个 trait 要求一个 `Err` 类型，会把
    /// "认不出"变成一个错误**值**；这里只有"认得"与"认不得"两种，多出来的那个类型没有消费者。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Option<Self> {
        Some(match text {
            "transport-failure" => ModelFailureClass::Transport(None),
            "transport-failure:dns" => ModelFailureClass::Transport(Some(TransportDetail::Dns)),
            "transport-failure:refused" => {
                ModelFailureClass::Transport(Some(TransportDetail::Refused))
            }
            "transport-failure:timeout" => {
                ModelFailureClass::Transport(Some(TransportDetail::Timeout))
            }
            "transport-failure:io" => ModelFailureClass::Transport(Some(TransportDetail::Io)),
            "http-status-failure" => ModelFailureClass::HttpStatus(None),
            "http-status-failure:credential" => {
                ModelFailureClass::HttpStatus(Some(HttpStatusDetail::Credential))
            }
            "http-status-failure:rate-limited" => {
                ModelFailureClass::HttpStatus(Some(HttpStatusDetail::RateLimited))
            }
            "http-status-failure:server-side" => {
                ModelFailureClass::HttpStatus(Some(HttpStatusDetail::ServerSide))
            }
            "http-status-failure:request-rejected" => {
                ModelFailureClass::HttpStatus(Some(HttpStatusDetail::RequestRejected))
            }
            "unparsable-response" => ModelFailureClass::UnparsableResponse,
            "contract-violation" => ModelFailureClass::ContractViolation,
            "stopped-generation" => ModelFailureClass::StoppedGeneration,
            "unsupported-tool-name" => ModelFailureClass::UnsupportedToolName,
            "reserved-tool-name" => ModelFailureClass::ReservedToolName,
            "unpaired-history" => ModelFailureClass::UnpairedHistory,
            "protocol-payload" => ModelFailureClass::ProtocolPayload,
            _ => return None,
        })
    }
}

/// Model 这一层的失败：它**带着分类**。
///
/// Core 仍然不把外部错误码或错误文本带进领域：这里只搬运一个分类名（Core 自己的封闭词汇），
/// 状态码、响应体、请求头与凭据都留在适配层。分类是事实不是日志——它跟着收场进盘、进终端、
/// 进诊断与恢复报告，因此消费方不必再去读 stderr。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelError {
    /// 失败分类（封闭词汇，见 [`ModelFailureClass`]）。
    pub class: ModelFailureClass,
}

/// Model 的窄边界：Core 只请求一次中立响应。
///
/// 它**不**回答"要不要继续"：Core 不把"再调用一次"的许可交给 Model 层。一次响应只说自己是什么
/// （行动 / 结束 / 纯正文），是否继续由 [`Decision`] 与 [`View`] 里的机械事实共同决定。
pub trait ModelPort {
    /// 用当前视野换一次中立响应。
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError>;

    /// **装订这一次活动的静态头**（TASK_79）：装配读回来的那一段，由装配在每一次驱动之前推一次。
    ///
    /// 它不是"一次调用"，而是"**这一次活动开场时读到的那一份**"——所以 Run 内重复推的是同一个值
    /// （文件在 Run 里被改也不变），下一次活动的开场会重新拍。`None` = 这一次没有静态头
    /// （主体的家里没有那一份，或主体把它摘了）⇒ 请求里一个字节都不加。
    ///
    /// **为什么它是这个接口上的一条、而不是 `complete` 的一个参数**：它属于**装配与执行端**之间的
    /// 一件事（谁读盘、什么时候拍、怎么渲染都在那两层），Core 既不知道它的内容、也不解释它；
    /// 而每一次调用都带一个参数，会把五十来个测试替身一起改一遍。
    ///
    /// **默认空实现**：替身（不发请求，也就没有请求可装订）不必关心它。两条生产 wire 各把它存进
    /// 字段，渲染时置于机械事实**之前**（静态在前、动态在后）。
    fn declare_static_head(&mut self, _head: Option<&str>) {}
}

/// 运行轨迹里的一条：外层注入、模型响应或行动结果，按发生顺序。
///
/// 只有**一个**权威有序轨迹：注入、模型响应与行动结果不放在三个各自排序的集合里，也不按类型
/// 聚合。顺序就是发生顺序，"下一条是什么"因此可判定。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TraceEntry {
    /// 外层注入的一段内容。阶段与轮次由 Core 标记。
    Injection {
        stage: InjectionStage,
        turn: Option<Turn>,
        batch: ContentBatch,
    },
    /// 一次行动请求**没有被接纳**。
    ///
    /// 它不是行动结果（那要求已经派发过），也不是来源注入（那属于注入接缝）：它是执行端在
    /// 准备之前给出的拒绝，因此这里只带四件真实的东西——**哪一轮、请求的是哪个动作、拒绝的
    /// 稳定分类，以及随单内容**。模型给的参数不进来：拒绝不需要复述参数，少留一份就不多一份风险。
    Refused {
        /// 这次拒绝发生在哪一轮（真实轮次，不是推算出来的）。
        turn: Turn,
        /// 被拒绝的动作标识。
        action: ActionId,
        /// 执行端给出的稳定拒绝分类。
        class: String,
        /// 随单内容：执行端在拒绝那一刻已经知道、而这次请求正缺的那份事实（可以为空）。
        ///
        /// 它与分类分开：分类照旧逐字节稳定，这里放的是旁边的结构化读数（见
        /// [`ActionAdmission::Refused`]）。
        content: Vec<ContentBlock>,
    },
    /// 一次行动的结果。
    Observation(Observation),
    /// 一次模型响应：正文、单个决定与必要的协议续接数据。
    ///
    /// 它排在**它所请求的行动**之前：Core 先确认响应耐久，才可能派发那次行动。因此"这条响应
    /// 请求了什么"是明确的——请求了行动时，紧随其后的就是它的回执。
    ModelResponse(ModelResponse),
}

/// 启动一次活动需要的东西：活动身份、资源边界、开场输入与上下文来源。
///
/// 用一个小的请求结构而不是继续加位置参数，是为了让新增接缝不把启动函数撑成十来个参数。
pub struct RunRequest<'a> {
    /// 这次活动的身份。由调用方生成。
    pub run: RunId,
    /// 这次活动的资源边界。
    pub limits: Limits,
    /// 调用方明确给出的开场内容；没有就是 [`RunInput::empty`]。
    pub input: RunInput,
    /// 上下文来源；没有额外注入时用 [`NoContext`]。
    pub source: &'a mut dyn ContextSource,
    /// 经历提交接缝；不保留任何经历时用 [`NoCommit`]。
    pub commit: &'a mut dyn CommitSeam,
    /// **收场那一刻现取**的代价读数（时长 ＋ 用量）。
    ///
    /// 它是个**取数函数**而不是一个值：时长是"从起点到收场"这一个区间，只有在收场那一轮
    /// 才成立（活动还没结束就先算好，那个数迟早是假的）。等待路径一次都不调它。
    ///
    /// Core 不认识它带回来的东西——只原样搬进 `RunEnded`（见 [`crate::cost`]）。
    /// 不关心代价的调用方给 [`no_cost`]。
    pub cost: &'a dyn Fn() -> RunCost,
}

/// **不记代价**：这一格永远"没有被记"（`None`），不是"等于零"。
///
/// 它是给不关系代价的入口用的默认值：离线夹具、既有用例、一次性驱动都请用它，
/// 因此那些路径的盘上形状**一个字都不多**。
pub fn no_cost() -> RunCost {
    RunCost::default()
}

/// 这一轮 Model 被允许看到的输入。
///
/// 它只投影这次活动自己的事实、资源边界、**当前可用的能力**与**运行轨迹**：
/// 轨迹按发生顺序完整保留（只追加，不隐藏、不摘要、不排序），不含系统总结、优先级或隐藏指令，
/// 也不含任何外部世界的业务字段分支。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    run: RunId,
    turn: Turn,
    limits: Limits,
    input: RunInput,
    capabilities: Capabilities,
    trace: Vec<TraceEntry>,
}

impl View {
    /// 视野属于哪次活动。
    pub fn run(&self) -> &RunId {
        &self.run
    }

    /// 视野是在第几个 Turn 形成的。
    pub fn turn(&self) -> Turn {
        self.turn
    }

    /// 这一次活动的初始输入。每一轮读到的都是同一份：Run 开始确定后不再修改、不追加。
    pub fn input(&self) -> &RunInput {
        &self.input
    }

    /// 这一次活动**当前可用**的能力。
    ///
    /// 这里只有**当前**可用的能力，没有历史：本版实现的展开策略是"同一次活动内只增不减"，
    /// 但这不是 Core 对将来任何能力提供者的要求。
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// 这次活动的**唯一**有序轨迹：外层注入与行动结果按发生顺序完整保留。
    ///
    /// 只追加：先发生的内容不会因为之后的能力变化而消失、被摘要或被重排，也不按类型重新聚合。
    /// 渲染给 Model 的视图必须走这里，才能得到确定的整体顺序。
    pub fn trace(&self) -> &[TraceEntry] {
        &self.trace
    }

    /// 轨迹里行动结果这一部分的**只读筛选投影**（顺序不变）。
    ///
    /// 它是 [`View::trace`] 的投影，不是另一份历史；需要同时看注入与结果时用 `trace()`。
    /// **拒绝不是结果**：被拒绝的请求没有派发过，因此它不出现在这里。
    /// **模型响应也不是结果**：它只说模型说了什么，出没出效果由随后的行动结果说话。
    pub fn observed(&self) -> Vec<&Observation> {
        self.trace
            .iter()
            .filter_map(|entry| match entry {
                TraceEntry::Observation(observation) => Some(observation),
                TraceEntry::Injection { .. }
                | TraceEntry::Refused { .. }
                | TraceEntry::ModelResponse(_) => None,
            })
            .collect()
    }

    /// 这次活动的资源边界。
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// 这次活动还剩多少次 Turn。
    pub fn remaining_turns(&self) -> u32 {
        self.limits.max_turns.saturating_sub(self.turn.number())
    }
}

/// Action 的窄边界：Core 请求一次外部尝试，并**每一轮**从这里取当前可用的能力。
///
/// 两个方法放在同一个接缝上，是为了让"当前能力"与"执行行动"由同一个拥有者给出——
/// 这样不需要 `RefCell`、锁或第二套状态来共享它。
///
/// 操作身份由 Core 拥有：Core **显式传入**本次操作身份与行动主体，执行端只能使用、不能选择。
/// `run` 让执行端能把"当前可用的能力"限定在这一次活动里（Core 只搬运身份，不解释归属规则）。
pub trait ActionPort {
    /// 这一次活动当前可用的能力。
    ///
    /// 能力集合可能随活动进展变化（本版实现里是只增不减的展开策略），所以每一轮都要重新问一次；
    /// Core 不解释返回的能力集合，只把它原样放进视野。
    ///
    /// **能力目录不是安全门**：它只说明"现在有哪些动作可以被请求"，不决定某一次请求是否获准。
    fn capabilities(&self, agent: &AgentId, run: &RunId) -> Capabilities;

    /// 这次行动请求是否**被接纳**（在 Prepared 之前问一次）。
    ///
    /// 它**没有默认实现**：默认无条件允许会让"忘了接线"看起来像"一切正常"，而这一层正是
    /// 写效应的门。每个执行端都必须明确说出自己的准入策略——包括测试替身。
    ///
    /// 判断本身不得产生任何业务效果、不得消耗授权、不得创建操作身份。
    fn admit(&self, agent: &AgentId, run: &RunId, action: &Action) -> ActionAdmission;

    /// 尝试执行一次行动。`operation` 是 Core 为这次尝试产生的身份，执行端不得改写。
    fn execute(
        &mut self,
        agent: &AgentId,
        operation: &OperationId,
        action: &Action,
    ) -> ActionOutcome;
}

/// 正在途中的一次外部尝试。
#[derive(Clone, Debug)]
struct InFlight {
    operation: OperationId,
    action: ActionId,
    dispatched: bool,
}

/// 一次有限活动：它拥有自己的状态、Turn 顺序、在途行动与结束不变量。
///
/// 它**不持有**能力与展开状态：当前可用的能力每一轮由执行端给出，因此活动本身不认识入口、
/// App 或"哪些能力已经展开"。
#[derive(Debug)]
pub struct Run {
    agent: AgentId,
    id: RunId,
    limits: Limits,
    input: RunInput,
    turn: Turn,
    in_flight: Option<InFlight>,
    /// 这次活动的**唯一**有序轨迹：外层注入、模型响应与行动结果，按发生顺序。
    trace: Vec<TraceEntry>,
    outcome: Option<Outcome>,
    /// Run 级登记（登记开始 / 确认输入 / Run 级注入）是否已经**完整走完**。
    ///
    /// 一次活动可以被**驱动多次**（例如模型交回控制之后）。这个标记让"登记恰好一次"成为机械
    /// 事实：重复驱动沿用同一份 Run、Trace 与预算，但不会重复登记、不会重复 Run 级注入。
    started: bool,
}

impl Run {
    /// 开始一次活动。
    ///
    /// 活动槽由 Agent 自己检查并占用：同一个 Agent 不允许同时存在两个没有收场的活动。
    /// 被拒绝时不占用活动槽、不写任何事实，也不会调用 Model 或 Action。
    /// 资源边界必须至少允许一次 Turn。
    ///
    /// 占用之后必须由 [`Run::end`] 合法收场来释放；驱动中途失败时由 Core 的私有清理释放。
    ///
    /// 这个入口等价于**空输入**开始一次活动；带初始内容请用 [`Run::begin_with_input`]。
    pub fn begin(agent: &mut Agent, id: RunId, limits: Limits) -> Result<Run, RunError> {
        Self::begin_with_input(agent, id, limits, RunInput::empty())
    }

    /// 开始一次带初始输入的活动。
    ///
    /// 初始输入在此时一次确定：Run 持有它，之后不修改、不追加。输入本身已经过上限检查
    /// （见 [`RunInput::text`]），所以到这里不会有"半截输入"。
    pub fn begin_with_input(
        agent: &mut Agent,
        id: RunId,
        limits: Limits,
        input: RunInput,
    ) -> Result<Run, RunError> {
        if limits.max_turns == 0 {
            return Err(RunError::NoTurnBudget);
        }
        agent.occupy(&id)?;
        Ok(Run {
            agent: agent.id().clone(),
            id,
            limits,
            input,
            turn: Turn::NONE,
            in_flight: None,
            trace: Vec::new(),
            outcome: None,
            started: false,
        })
    }

    /// 这次活动的身份。
    pub fn id(&self) -> &RunId {
        &self.id
    }
    /// 这次活动属于哪个 Agent。
    pub fn agent(&self) -> &AgentId {
        &self.agent
    }

    /// 这次活动的资源边界。
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// 这次活动的初始输入（Run 开始时确定，之后不变）。
    pub fn input(&self) -> &RunInput {
        &self.input
    }

    /// 已经走到第几个 Turn。
    pub fn last_turn(&self) -> Turn {
        self.turn
    }

    /// 这次活动的**唯一**有序轨迹：外层注入、模型响应与行动结果，按发生顺序完整保留。
    ///
    /// 读它、或据此重新构造视野与请求，都不会触发任何注入，也不会改变轨迹。
    pub fn trace(&self) -> &[TraceEntry] {
        &self.trace
    }

    /// 轨迹里行动结果这一部分的**只读筛选投影**（顺序不变）。
    ///
    /// 与 [`View::observed`] 同一条规则：拒绝与模型响应都不是结果。
    pub fn observed(&self) -> Vec<&Observation> {
        self.trace
            .iter()
            .filter_map(|entry| match entry {
                TraceEntry::Observation(observation) => Some(observation),
                TraceEntry::Injection { .. }
                | TraceEntry::Refused { .. }
                | TraceEntry::ModelResponse(_) => None,
            })
            .collect()
    }

    /// 交给注入来源的**只读**上下文：身份、轮次与资源边界。
    ///
    /// 这里刻意只给读的东西：来源拿不到 `&mut` 历史、行动端口、模型端口或账本。
    pub(crate) fn injection_context(
        &self,
        turn: Option<Turn>,
        capabilities: Capabilities,
    ) -> InjectionContext {
        InjectionContext {
            agent: self.agent.clone(),
            run: self.id.clone(),
            turn,
            limits: self.limits,
            capabilities,
        }
    }

    /// 接收一批注入：**空批次不产生任何记录**，非空批次按发生顺序追加一条。
    ///
    /// 返回**本次真正新增的那一条**：空批次返回 `None`。调用方据此提交经历，不需要（也不能）
    /// 用"轨迹的最后一条"去猜——空批次之后，最后一条仍然是上一条旧经历。
    ///
    /// 只有 Core 的驱动流程会调用它：外层来源拿到的是只读上下文，没有修改轨迹的接口。
    ///
    /// ⚠ **账只能有一个写者**（这是 `Run` 那六个轨迹写入口共同的判据）：生产里它们的调用者
    /// **只许是 `drive_run`**，多出来的就是**第二条循环**——那一刻账会有第二个写者。
    ///
    /// **这条判据是守卫、不是类型**：六个入口今天都是 `pub`，而那不是产品需要——集成用例是
    /// **独立 crate**，只能看 `pub`，而它们要造驱动循环造不出来的形状（孤儿操作、切分的操作）。
    /// 收成 `pub(crate)` 会打断它们 ⇒ **"优先用可见性"这条路今天走不通**（这是读出来的事实，
    /// 不是取舍）。于是落成源码守卫 `tests/minimal_agent.rs::only_the_one_loop_writes_the_trace`：
    /// 先按**括号配对**砍掉文件末尾那个单元测试模块整块（不是见到属性就截断，且有自检）之后，
    /// 要求这六个入口的**生产**调用者只许是 `drive_run`，多出来的**点名到文件:行**；六个入口
    /// 每一个都必须至少找到一次（**防空跑**）。
    /// ⚠ **别在本文件里原样引用那个属性名**（`#` 加 `[cfg(test)]`）：那条守卫扫的是**文本**，
    /// 注释里出现一次它就会把注释当成测试模块的起点、当场判"形状变了"。
    /// ⚠ **如实登记**：它抓的是"第二个生产调用者"，**不是编译不过**——不许说成"已由类型保证"；
    /// **可见性欠账照旧**（真收口要把手工搭轨迹的集成用例搬进库内），那是另一刀。
    pub(crate) fn accept_injection(
        &mut self,
        stage: InjectionStage,
        turn: Option<Turn>,
        batch: ContentBatch,
    ) -> Result<Option<TraceEntry>, RunError> {
        if self.is_ended() {
            return Err(RunError::AlreadyEnded);
        }
        if batch.is_empty() {
            return Ok(None);
        }
        let entry = TraceEntry::Injection { stage, turn, batch };
        self.trace.push(entry.clone());
        Ok(Some(entry))
    }

    /// 活动是否已经收场，以及怎么收场的。
    pub fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }

    /// 这次活动是否已经在 Core 里完成 **Run 级登记**。
    ///
    /// 登记包含三件事：把"这次活动已经开始"确认为耐久、确认这次活动的初始输入、调用一次 Run
    /// 级注入并确认它。三者**恰好一次**，而且都排在第一次模型调用之前。
    ///
    /// 它是**机械事实**，不是进度判断：一次活动可以被驱动多次，"已经登记过"与"走到第几轮"
    /// 是两件事。重复驱动同一个活动时，Core 靠它保证不重复登记、不重复注入。
    pub fn is_started(&self) -> bool {
        self.started
    }

    /// 标记 Run 级登记已经**完整走完**。
    ///
    /// **私有**：只有 Core 的唯一驱动流程在登记序列全部成功之后调用它。登记中途失败时它不会被
    /// 调用——那次会话不再被驱动，也不会出现"登记了一半却当成已登记"的形状。
    fn mark_started(&mut self) {
        self.started = true;
    }

    /// 活动是否已经收场。
    pub fn is_ended(&self) -> bool {
        self.outcome.is_some()
    }

    /// 现在有没有一次外部尝试还没有结算。
    pub fn action_in_flight(&self) -> Option<&ActionId> {
        self.in_flight.as_ref().map(|flight| &flight.action)
    }

    /// 这次活动是否还能再处理一次 Turn。
    pub fn can_advance(&self) -> bool {
        !self.is_ended() && self.turn.number() < self.limits.max_turns
    }

    /// 开始下一个 Turn。Turn 只能单调前进。
    pub fn next_turn(&mut self) -> Result<Turn, RunError> {
        if self.is_ended() {
            return Err(RunError::AlreadyEnded);
        }
        if self.in_flight.is_some() {
            return Err(RunError::OperationInFlight);
        }
        if !self.can_advance() {
            return Err(RunError::TurnBudgetExhausted);
        }
        self.turn = self.turn.advance();
        Ok(self.turn)
    }

    /// 当前视野：这次活动自己的事实、资源边界，加上**传入的当前可用能力**。
    ///
    /// 能力由调用方（当前的驱动流程）每一轮从执行端取得；`Run` 自己不持有它，
    /// 因此活动不会把某一次的能力集合固化成状态。
    pub fn view(&self, capabilities: Capabilities) -> View {
        View {
            run: self.id.clone(),
            turn: self.turn,
            limits: self.limits,
            input: self.input.clone(),
            capabilities,
            trace: self.trace.clone(),
        }
    }

    /// 追加一条"这次行动请求没有被接纳"的经历，并返回**本次新增的那一条**。
    ///
    /// 它与 [`Run::prepare`] 是两条互斥的路：拒绝发生在准备**之前**，因此这里不产生操作身份、
    /// 不占用在途尝试，也不写任何行动事实。调用方据此把这条经历确认为耐久。
    ///
    /// `content` 是随单内容（见 [`ActionAdmission::Refused`]）：它为空的拒绝与从前逐字节相同，
    /// 因此"只给分类"的拒绝不受这条通道影响。
    pub fn accept_refusal(
        &mut self,
        turn: Turn,
        action: &ActionId,
        class: String,
        content: Vec<ContentBlock>,
    ) -> Result<TraceEntry, RunError> {
        if self.is_ended() {
            return Err(RunError::AlreadyEnded);
        }
        if self.in_flight.is_some() {
            return Err(RunError::OperationInFlight);
        }
        let entry = TraceEntry::Refused {
            turn,
            action: action.clone(),
            class,
            content,
        };
        self.trace.push(entry.clone());
        Ok(entry)
    }

    /// 记录一次模型响应，并返回**本次新增的那一条**。
    ///
    /// 驱动流程在**每一次**模型调用返回之后立刻调用它，因此"响应先于它请求的行动被确认"
    /// 这条顺序只有一个调用点。调用方自己拼轨迹时也要守住同一条相邻关系：请求了行动的响应
    /// 紧邻它的回执，两者之间不插入别的输入（见 [`ModelResponse`]）。
    pub fn accept_response(
        &mut self,
        turn: Turn,
        output: ModelOutput,
    ) -> Result<ModelResponse, RunError> {
        if self.is_ended() {
            return Err(RunError::AlreadyEnded);
        }
        if self.in_flight.is_some() {
            return Err(RunError::OperationInFlight);
        }
        let response = ModelResponse::new(turn, output);
        self.trace.push(TraceEntry::ModelResponse(response.clone()));
        Ok(response)
    }

    /// 记录一次行动已经准备，并给出这次尝试的稳定操作身份。
    ///
    /// `index` 是**这一轮里的第几个动作**（从 0 起）。一轮可以请求多个动作，它们各自是一次
    /// 独立的外部尝试，因此身份里必须带上"第几个"——否则同一轮的两个动作会塌成同一个身份。
    pub fn prepare(&mut self, action: &Action, index: u16) -> Result<OperationId, RunError> {
        if self.is_ended() {
            return Err(RunError::AlreadyEnded);
        }
        if self.in_flight.is_some() {
            return Err(RunError::OperationInFlight);
        }
        let operation = OperationId::at(self.id.clone(), self.turn, index);
        self.in_flight = Some(InFlight {
            operation: operation.clone(),
            action: action.id().clone(),
            dispatched: false,
        });
        Ok(operation)
    }

    /// 记录一次行动已经发出。没有准备过的尝试不允许发出。
    pub fn mark_dispatched(&mut self, operation: &OperationId) -> Result<(), RunError> {
        let flight = self
            .in_flight
            .as_mut()
            .ok_or(RunError::OperationOutOfOrder)?;
        if &flight.operation != operation {
            return Err(RunError::OperationMismatch);
        }
        if flight.dispatched {
            return Err(RunError::OperationOutOfOrder);
        }
        flight.dispatched = true;
        Ok(())
    }

    /// 记录一次行动的结果，并把这次观察放进运行轨迹。没有发出过的尝试不允许结算。
    ///
    /// 观察的组装由 Core 完成：身份与参数用 Core 自己的记载（`action`），`surface` 是这次行动
    /// 执行端交回的只有结果。因此执行端既不能改写操作身份，也不能替换"这次到底发出了什么"。
    ///
    /// 返回**本次真正新增的那一条观察**：与 [`Run::accept_injection`] 同理，调用方据此提交经历，
    /// 不需要用"轨迹的最后一条"去猜。
    pub fn observe(
        &mut self,
        action: &Action,
        outcome: ActionOutcome,
    ) -> Result<TraceEntry, RunError> {
        let flight = self
            .in_flight
            .as_ref()
            .ok_or(RunError::OperationOutOfOrder)?;
        if !flight.dispatched {
            return Err(RunError::OperationOutOfOrder);
        }
        let operation = flight.operation.clone();
        self.in_flight = None;
        let entry = TraceEntry::Observation(Observation::new(operation, action, outcome));
        self.trace.push(entry.clone());
        Ok(entry)
    }

    /// 收场：活动结束 → Run 收场 → 它所属 Agent 的活动槽释放。
    ///
    /// 这是唯一的正常收场入口，也是唯一的合法释放点：必须带上**本活动所属的 Agent**，
    /// 收场成功（且只有成功）才释放槽位。还有未结算的外部尝试时不允许收场，活动也不
    /// 允许收场两次。
    pub fn end(&mut self, agent: &mut Agent, outcome: Outcome) -> Result<(), RunError> {
        if agent.id() != &self.agent {
            return Err(RunError::AgentMismatch);
        }
        if self.outcome.is_some() {
            return Err(RunError::AlreadyEnded);
        }
        if self.in_flight.is_some() {
            return Err(RunError::OperationInFlight);
        }
        self.outcome = Some(outcome);
        agent.clear_activity(&self.id);
        Ok(())
    }
}

/// 活动推进被拒绝的原因。每一类都保留自己发生的层级。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunError {
    /// 同一个 Agent 已经有一个没有收场的活动。
    ActiveRunExists,
    /// 活动的资源边界不允许任何 Turn。
    NoTurnBudget,
    /// 活动已经收场，不能再推进。
    AlreadyEnded,
    /// 已经有未结算的外部尝试，不能同时存在两次。
    OperationInFlight,
    /// Turn 资源已经用尽。
    TurnBudgetExhausted,
    /// 操作顺序不合法：没有准备就发出，或者没有发出就结算。
    OperationOutOfOrder,
    /// 操作身份与正在途中的尝试不一致。
    OperationMismatch,
    /// 这个活动不属于这个 Agent。
    AgentMismatch,
    /// 事实写入失败。
    Ledger(LedgerError),
    /// 经历提交失败：这一步没有被确认为耐久，活动不得继续推进。
    ///
    /// 它与 [`RunError::Ledger`] 区分，也与 [`SystemEnding::ModelFailed`] 区分：保存失败不是
    /// 模型失败，也不是主体完成，因此不能借任何一个终态把这件事说成别的。
    Commit(CommitError),
}

impl From<CommitError> for RunError {
    fn from(error: CommitError) -> Self {
        RunError::Commit(error)
    }
}

impl From<LedgerError> for RunError {
    fn from(error: LedgerError) -> Self {
        RunError::Ledger(error)
    }
}

impl RunError {
    /// **稳定分类标记**：每一类保留自己的层级，一个取值都不许并进别的取值。
    ///
    /// 它是这张词表的**唯一权威**（贴着拥有语义的 `RunError`，与 `StoreError::as_str` /
    /// `Admission::as_str` / `RunState::as_str` 同款）。从前这十个词只以**私有副本**的形式
    /// 活在 `manual_entry.rs` 里，而常驻宿主那一侧**一个词都不区分**：四类完全不同的失败
    /// （事实写失败 / 保存未确认 / 活动槽被占 / 轮次用尽）在操作者读数里塌成同一个
    /// `run-refused`，退出码也一律相同——那正是"必须保留影响调用方决策的类别"要挡的形状。
    ///
    /// **判据**：同一件事发生两次，这个字符串必须逐字节相同；两个不同的类别不许给出同一个词。
    pub fn as_str(&self) -> &'static str {
        match self {
            RunError::ActiveRunExists => "active-run-exists",
            RunError::NoTurnBudget => "no-turn-budget",
            RunError::AlreadyEnded => "already-ended",
            RunError::OperationInFlight => "operation-in-flight",
            RunError::TurnBudgetExhausted => "turn-budget-exhausted",
            RunError::OperationOutOfOrder => "operation-out-of-order",
            RunError::OperationMismatch => "operation-mismatch",
            RunError::AgentMismatch => "agent-mismatch",
            RunError::Ledger(_) => "ledger",
            RunError::Commit(_) => "commit",
        }
    }
}

/// 一次驱动的结果：这次活动**收场了**，还是**只是停下、并没有收场**。
///
/// 两者必须分开说，因为它们对调用方意味着完全不同的事：
///
/// - [`DriveOutcome::Ending`] 带着一个**已经确认为耐久**的终态，活动槽已经释放；
/// - [`DriveOutcome::Unsettled`] 只说"这次驱动停在这里"：它**不是**收场、**不是**失败，也**不是**
///   "模型什么都没说"——合法纯正文就走这条路。
///
/// 错误既不在这里，也不伪装成 [`DriveOutcome::Unsettled`]：它按既有分类以 `Err(RunError)` 返回。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveOutcome {
    /// 活动已经按这个终态收场（主体结束或系统终止），活动槽已释放。
    Ending(Outcome),
    /// 这次驱动停下来了，活动**没有**结束：Run、轮次、轨迹、展开状态与预算都留着，等下一次驱动。
    ///
    /// ⚠️ **它从前叫 `Waiting`**（2026-10-06 改）：而 `foundation::SubjectEnding::Waiting` 是
    /// **另一件事**——那是主体自己说"我在等外部的东西"并因此**收场**（活动**结束**了）。
    /// 两个相反的结局共用一个词，正是本模块文档自己说的"绝不能压成一个词"。
    /// 现在这一档的名字说的是**活动的状态**（**未收场**），与收场那一档不再撞名。
    Unsettled,
}

/// 唯一的执行流程：开始一次活动，逐 Turn 处理，最后收场。
///
/// 它只负责调用顺序——什么时候形成视野、什么时候请求 Model、什么时候落事实、
/// 什么时候调用外部能力、什么时候收场。状态合法性由 `Run` 与 `Agent` 自己保证，
/// 外部能力由 Port 提供，事实由 `Ledger` 保存。
///
/// **一次调用只驱动一次**。活动被驱动到"合法停下来"（[`DriveOutcome::Unsettled`]）时它**不**收场、
/// **不**释放活动槽：同一个 Run 稍后还要继续被驱动。要多次驱动同一个活动请用会话
/// （见 `composition`）——它持有 Agent、Run、账本、装配好的执行环境与那一次活动的展开状态。
///
/// 活动占用也在 Agent 自己手里：`Run::begin` 检查并占用活动槽，本函数在**错误路径**上保证它
/// 被交还——正常收场由 `settle` → `Run::end` 释放槽位，中途失败由私有的失败清理释放。
/// 因此返回 `Err` 之后 Agent 一定不再被占用；返回 `Ok(DriveOutcome::Ending)` 时槽位已经由收场
/// 释放；而返回 `Ok(DriveOutcome::Unsettled)` 时槽位**仍然属于这次活动**——那正是它的定义。
pub fn run_activity<M, A>(
    agent: &mut Agent,
    run_id: RunId,
    limits: Limits,
    model: &mut M,
    actions: &mut A,
    ledger: &mut Ledger,
) -> Result<Run, RunError>
where
    M: ModelPort,
    A: ActionPort,
{
    // 空输入 + 空来源 + 空提交：同一个实现，不是第二套循环。
    run_activity_with_input(
        agent,
        run_id,
        limits,
        RunInput::empty(),
        model,
        actions,
        ledger,
    )
}

/// 与 [`run_activity`] 完全相同，只是带着这次活动的**初始输入**开始。
///
/// 它不额外注入任何上下文，也不保留任何经历（空来源 + 空提交）。
pub fn run_activity_with_input<M, A>(
    agent: &mut Agent,
    run_id: RunId,
    limits: Limits,
    input: RunInput,
    model: &mut M,
    actions: &mut A,
    ledger: &mut Ledger,
) -> Result<Run, RunError>
where
    M: ModelPort,
    A: ActionPort,
{
    run_activity_with(
        agent,
        RunRequest {
            run: run_id,
            limits,
            input,
            source: &mut NoContext,
            commit: &mut NoCommit,
            // 这两条便捷入口不记代价（既有调用点与用例**一行不改**，盘上形状一个字不多）。
            cost: &no_cost,
        },
        model,
        actions,
        ledger,
    )
}

/// 唯一实现：带着开场输入、**上下文来源**与**经历提交接缝**启动一次活动并驱动一次。
///
/// 这是全 crate 唯一一条执行循环；上面两个入口都委托到这里。
///
/// 返回的 `Run` 可能是**没结束**的：这次驱动停在 [`DriveOutcome::Unsettled`] 时，`Run::outcome`
/// 是 `None`、活动槽仍然被占用。这一刻不是失败，也不允许被当成失败收场——"同一个活动稍后再
/// 被驱动"是一条合法路径。
pub fn run_activity_with<M, A>(
    agent: &mut Agent,
    request: RunRequest<'_>,
    model: &mut M,
    actions: &mut A,
    ledger: &mut Ledger,
) -> Result<Run, RunError>
where
    M: ModelPort,
    A: ActionPort,
{
    let RunRequest {
        run: run_id,
        limits,
        input,
        source,
        commit,
        cost,
    } = request;

    let mut run = Run::begin_with_input(agent, run_id, limits, input)?;

    continue_activity(
        &mut run, agent, model, actions, ledger, source, commit, cost,
    )
    .map(|_| run)
}

/// 驱动一次，并且只在**收场**时完成"确认为耐久的收场"。
///
/// 等待路径在这里什么都不做：不写终态、不封存、不释放活动槽——它只是把"停在这里"如实交回。
///
/// 它是**同一个 Run 的后续驱动**入口：[`run_activity_with`] 负责"开始一次活动并驱动一次"，
/// 它负责"再驱动一个已经开始的同一次活动"。两者共用同一条循环（[`drive_run`]），因此
/// 登记恰好一次、轮次只增、预算不重置、等待不耗轮这些性质只有一份实现。
///
/// **失败清理只在这里**：返回 `Err` 时这次调用已经不再驱动这个活动，活动槽必须交还，否则会
/// 留下永久占用。合法等待**不**走这条路径——等待的定义就是"活动还没结束、槽还属于它"。把清理
/// 放在驱动入口而不是放在某个调用点，是为了让"开始一次"与"再驱动一次"两条来路都只需要记住
/// 自己那一件事。
///
/// `cost_of` 是**收场那一刻现取**的代价读数：只在真收场时被调用一次（等待路径一次都不调），
/// 因为"时长"是"从起点到收场"这一个区间——它只能在收场那一刻才成立。Core 不认识它的含义，
/// 只把它原样带进 `RunEnded`（见 [`crate::cost`]）。
///
/// 8 个参数各自是**不同协作者**（不自成一组的独立接缝），刀三 B 新增的 `cost_of` 是其中一个
/// 真实的新协作者（代价来源与那七个都不重合）；把它们捆成一个结构体只是换个地方数参数，
/// 不会让这里更清楚，因此明确留白这条 lint。
#[allow(clippy::too_many_arguments)]
pub fn continue_activity<M, A>(
    run: &mut Run,
    agent: &mut Agent,
    model: &mut M,
    actions: &mut A,
    ledger: &mut Ledger,
    source: &mut dyn ContextSource,
    commit: &mut dyn CommitSeam,
    cost_of: &dyn Fn() -> RunCost,
) -> Result<DriveOutcome, RunError>
where
    M: ModelPort,
    A: ActionPort,
{
    let result = drive_run(run, model, actions, ledger, source, commit).and_then(|outcome| {
        if let DriveOutcome::Ending(ending) = outcome {
            // **现取**：收场那一轮才问"代价是多少"（等待路径不会走到这里）。
            settle(run, agent, ledger, commit, ending, cost_of())?;
        }
        Ok(outcome)
    });

    if result.is_err() {
        // 正常收场时槽位已在 settle → Run::end 里释放，这一行是幂等的空操作；
        // 中途失败时（账本写入失败、保存未确认等）槽位必须交还。
        agent.clear_activity(run.id());
    }

    result
}

/// 活动主体：登记活动开始，逐 Turn 处理，直到这次驱动停下或活动收场。
///
/// 它不决定活动何时**收场**——Model 的决定、无法确认的结果和资源边界共同决定；它只保证每一步
/// 的顺序，并把失败原样返回给调用方。
///
/// **登记恰好一次**：Run 级登记（登记开始 → 确认输入 → Run 级注入）只在 `Run` 还没有完成登记时
/// 发生。同一个活动被驱动多次时，第二次起沿用同一份 Run、轨迹、轮次与预算，不重复登记、
/// 不重复 Run 级注入。
///
/// 当前可用能力**每一轮**都从执行端重新取得：展开会追加能力，所以 Run 不持有它。
///
/// 两个注入接缝的时机也在这里唯一确定：**登记成功之后**调用一次 Run 级注入，
/// **每一轮 next_turn 成功之后、形成视野与实际调用模型之前**调用一次 Turn 级注入。
/// 查询视野、重新渲染请求、读取已收场的活动都不会触发它们。
///
/// **经历提交接缝**的时机同样在这里唯一确定，而且全部都排在它对应的"下一步"之前：
/// 登记 → 输入 → Run 注入 → 每轮 Turn 注入 → **响应 → 准入 →** 行动选择 → 发出 → 结果。
/// 每一步确认失败都会直接返回 `RunError::Commit`，**不**继续模型调用、**不**执行动作、
/// **不**伪造终态、也**不**返回正常等待。
///
/// **响应排在它所请求的行动之前**：收到并通过校验的响应先在派发任何外部行动之前确认为耐久。
/// 保存失败时那次行动根本不会发出，下一次模型也不会被调用。
///
/// **准入排在准备之前**：行动请求先经 [`ActionPort::admit`] 判断是否被接纳，只有获准的请求才会
/// 产生操作身份与行动事实；被拒绝的请求只留一条中立拒绝经历（见 [`TraceEntry::Refused`]）。
pub fn drive_run<M, A>(
    run: &mut Run,
    model: &mut M,
    actions: &mut A,
    ledger: &mut Ledger,
    source: &mut dyn ContextSource,
    commit: &mut dyn CommitSeam,
) -> Result<DriveOutcome, RunError>
where
    M: ModelPort,
    A: ActionPort,
{
    // Run 级登记：恰好一次。第二次驱动同一个活动时整段跳过——包括那两个"恰好一次"的确认。
    if !run.is_started() {
        ledger.append(Record::RunStarted {
            agent: run.agent().clone(),
            run: run.id().clone(),
        })?;

        // 活动开始：先确认"这次活动已经开始"，再确认这次活动的初始输入。
        // 两者都排在第一次模型调用之前——不能等到最后再导出整段。
        commit.commit_run_start(run.agent(), run.id())?;
        commit.commit_input(run.id(), run.input())?;

        // Run 级注入：登记成功之后、第一个 Turn 之前，恰好一次。
        // 登记失败（含重复 RunId）在上面就已经返回，所以那时注入还没被调用。
        // TASK_68 P2：这一轮的处境里也带**开场时**的能力面（此后每一轮重新取）。
        let opening = actions.capabilities(run.agent(), run.id());
        let start_batch = source.on_run_start(&run.injection_context(None, opening));
        let start_entry = run.accept_injection(InjectionStage::RunStart, None, start_batch)?;

        // Run 级注入一旦被接受，就确认进经历存储。**空批次零新增、零提交**：
        // 提交的是本次真正新增的那一条，而不是"轨迹当前碰巧的最后一条"。
        if let Some(entry) = &start_entry {
            commit.commit_trace(run.id(), None, entry)?;
        }

        // 登记序列**完整走完**才算已登记：中途失败时这个活动不会被当成"登记过一半"。
        run.mark_started();
    }

    while run.can_advance() {
        run.next_turn()?;

        // 每一轮重新取当前可用的能力：展开会追加能力，所以不能固定成开局快照。
        // TASK_68 P2：它现在读在**注入之前**——注入的处境里要带它（"主体此刻有哪些动作"）。
        // 读到前面不改变注入内容：产不产出由来源自己决定，空来源仍然是空（谁都不许替谁补块）。
        let capabilities = actions.capabilities(run.agent(), run.id());

        // Turn 级注入：这一轮真的要调用模型了，恰好一次；不为没有发生的下一轮预先注入。
        let turn = run.last_turn();
        let turn_batch =
            source.before_turn(&run.injection_context(Some(turn), capabilities.clone()));
        let turn_entry = run.accept_injection(InjectionStage::Turn, Some(turn), turn_batch)?;

        // 这一轮的注入同样在模型调用之前确认；同样是空批次零新增、零提交。
        if let Some(entry) = &turn_entry {
            commit.commit_trace(run.id(), Some(turn), entry)?;
        }

        let view = run.view(capabilities);

        let output = match model.complete(&view) {
            Ok(output) => output,
            // 这一层失败**带着分类**一起收场：分类跟着 `SystemEnding::ModelFailed` 进盘、进终端、
            // 进诊断与恢复报告。零自动重试不变：一次失败就是一次收场。
            Err(error) => {
                return Ok(DriveOutcome::Ending(Outcome::System(
                    SystemEnding::ModelFailed(Some(error.class.as_str())),
                )));
            }
        };

        // **响应先确认，再谈它请求了什么**：正文、决定与必要的协议续接数据一起进经历存储，
        // 而且排在"派发它请求的那次外部行动"之前。保存失败就在这里返回——那次行动不会发出，
        // 下一次模型也不会被调用，更不会返回一个"正常等待"。
        let response = run.accept_response(turn, output)?;
        commit.commit_trace(
            run.id(),
            Some(turn),
            &TraceEntry::ModelResponse(response.clone()),
        )?;

        let requested = match response.decision() {
            // 纯正文：交回控制。**不**收场、**不**释放活动槽、**不**自动再调用一次模型。
            Decision::Yield => return Ok(DriveOutcome::Unsettled),
            // **「搁下」不是收场**（2026-10-10 在场那一刀）：主体点名 `hold_run` 说的是
            // 「我停在这儿，有事叫我」——主体**仍在场**，只是此刻没事。它与纯正文**机械上
            // 同一件事**（都停在这里、活动槽都还属于这一段），区别只在账上多一条工具调用记录
            // （那一次是主体**点了名**的）。
            //
            // ⚠ **它从前是收场**（`Outcome::Subject(Held)` ⇒ 释放活动槽 ⇒ 封存）：那条路让
            // **每一声铃都开一段新活动**——铃在传输层上只说"有新动静"，却顺手切断了这一段。
            // 判据一句话：**收场只有一个来源，就是主体说"结束了"**；"我停在这儿"不是结束。
            //
            // ⚠ **`SubjectEnding::Held` 这个值留在盘上的封闭表里照读不写**：旧账里那 7 段
            // （真机 2026-10-10）读得出来，而线缆上再也产不出它。
            //
            // ⚠ **"搁下"只有这一支入口**：Core 另外有过一个"搁下并收场"的入口
            // （`hold_activity`），`RunSession` 上也有过一支同义的（`hold`）——两者随这一道判据
            // **一并删除**。它们存在的理由正是"被搁下的一段永远没有收尾事实"，而那个理由随
            // 在场一起消失了。**别加回来**：多一支"搁下"的入口，就多一个"谁来收场"的候选，
            // 而收场只有一个来源。
            Decision::Finish(SubjectEnding::Held) => return Ok(DriveOutcome::Unsettled),
            Decision::Finish(ending) => return Ok(DriveOutcome::Ending(Outcome::Subject(*ending))),
            Decision::Act(actions) => actions.clone(),
        };

        // **一轮里可以请求多个动作**：按模型给出的顺序**依次**处理，第几个就是它的操作身份
        // 里的序号（`Decision::Act` 的不变量是"非空"，而序号的上限由它一并挡住，因此这里的
        // 转换不会失败——留这一支是为了不让截断静默改写身份）。
        for (position, action) in requested.iter().enumerate() {
            let Ok(index) = u16::try_from(position) else {
                // 这一支**不可达**（`Decision::act` 保证动作数在可表达范围内）。留它在这里是为了
                // 不让截断静默改写身份；真到了这里，那是决策违反了自己的契约——因此带上
                // `contract-violation`，不为一条不可达路径发明新词汇。
                return Ok(DriveOutcome::Ending(Outcome::System(
                    SystemEnding::ModelFailed(Some(ModelFailureClass::ContractViolation.as_str())),
                )));
            };

            // **准入**：在准备之前问一次"这次请求要不要发出去"。
            //
            // 被拒绝的请求**不是**一次结果：它不产生操作身份，也不写任何行动事实（没有 prepared /
            // dispatched / result）。它只把一条中立的拒绝经历追加进这次活动的轨迹，并在下一次模型
            // 调用**之前**确认为耐久。下一步仍由模型自己判断——系统不自动重试，也不把一次拒绝
            // 升级成整个活动的失败。**同轮里其余的动作照常处理**：拒绝是这一个请求的事。
            //
            // **随单内容一并落进那条经历**（TASK_72）：执行端在拒绝那一刻已经知道、而这次请求正缺的
            // 那份事实，不必等主体再跑一轮照旧取——它在**下一次**模型调用时就摆在视野里，与拒绝同时到。
            // **参数搬不动**的请求：它到不了准入——没有可搬运的参数，就没有"要不要发出去"这件事。
            //
            // 这不是另立一条失败路，而是 [`INVALID_ARGUMENT`]（Core 的中立词，它的定义就是
            // "参数形状不合法（机械校验）"）对**形状**也成立。从前这一格表达不出来，于是翻译层
            // 只能把**整段清醒期**判死（`contract-violation`），主体一个字都说不出来——而这条
            // 教条在下面几行就写着：**不把一次拒绝升级成整个活动的失败**。
            let admission = match action.shape_reading() {
                Some(reading) => ActionAdmission::refused_with(
                    INVALID_ARGUMENT,
                    vec![ContentBlock::new(SHAPE_READING_LABEL, reading.to_string())],
                ),
                None => actions.admit(run.agent(), run.id(), action),
            };
            if let ActionAdmission::Refused { class, content } = admission {
                let entry = run.accept_refusal(turn, action.id(), class, content)?;
                commit.commit_trace(run.id(), Some(turn), &entry)?;
                continue;
            }

            let operation = run.prepare(action, index)?;
            ledger.append(Record::ActionPrepared {
                run: run.id().clone(),
                operation: operation.clone(),
            })?;

            // 执行之前确认"选择了什么、参数是什么"：没有这一步，一次已经发出的外部尝试
            // 就找不到任何可以关联的已确认记录。
            commit.commit_action_selected(run.id(), &operation, action)?;
            run.mark_dispatched(&operation)?;
            ledger.append(Record::ActionDispatched {
                run: run.id().clone(),
                operation: operation.clone(),
            })?;
            commit.commit_action_dispatched(run.id(), &operation)?;

            // 执行端拿到的只有 Core 给的身份、行动主体与这次行动本身；它交回结果，不能改写身份。
            let outcome = actions.execute(run.agent(), &operation, action);

            // 事实侧照旧只记三分状态（Foundation 不增加任何内容字段）。
            let result = ActionResult::new(operation.clone(), outcome.state());
            ledger.append(Record::ActionResultRecorded {
                run: run.id().clone(),
                action: result,
            })?;

            // 运行内痕迹：Agent 下一轮能看到自己做过什么、得到了什么（有界内容，不入账）。
            // 经历只追加：这条观察之后不会被能力变化抹掉或改写。
            let state = outcome.state();

            // 结果回来后先确认真实结果，再允许下一次模型调用。保存失败不重发动作。
            // 结果的正文（内容块与稳定失败分类）在这一步一起确认，观察本身只是同一份事实的运行内痕迹。
            commit.commit_action_result(run.id(), &operation, &outcome)?;
            let observation = run.observe(action, outcome)?;

            // 结果**作为经历**也要确认：事实侧只记三分状态，正文归经历存储——否则重开之后
            // 只能看到"某次行动有了结果"，却看不到 Agent 当时看到的内容，经历就残缺了。
            // 提交的是本次真正新增的那条观察，不是"碰巧在末尾"的一条。
            commit.commit_trace(run.id(), Some(operation.turn()), &observation)?;

            // 明确结果（成功或失败）都已经被记成事实，交回下一轮 Model 自己判断：
            // Action Failure 不是 Run Failure，系统不得因为一次行动失败替主体 结束活动，
            // 也不得自动重发同一个操作身份。**同轮里其余的动作照常处理**。
            //
            // 只有结果无法确认时才由系统收场：再发一次可能重复已经发生的外部效果。**这一轮
            // 剩下的动作一个都不发出**——它们还没有准备、没有派发，因此盘上零操作身份、零行动事实。
            // Foundation 仍保留 `SystemEnding::ActionFailed` 这个词汇（本刀不得改 Foundation），
            // 但 Core 不再生成它。
            if state == ResultState::Unknown {
                return Ok(DriveOutcome::Ending(Outcome::System(
                    SystemEnding::Indeterminate,
                )));
            }
        }
    }

    Ok(DriveOutcome::Ending(Outcome::System(SystemEnding::Timeout)))
}

/// 收场：**耐久确认先于内存成功表象**。
///
/// 顺序是有约束的，不是随便排的：
///
/// 1. `commit_ending` —— 把终态确认为耐久。只有它成功，"这次活动结束了"才成立；
/// 2. 写内存事实（`Ledger::append(RunEnded)`）；
/// 3. `run.end` —— 内存收场，同时释放该 Agent 的活动槽；
/// 4. `commit.seal` —— 结束确认**之后**的封存。
///
/// 反过来的写法（先 `run.end` 再确认）会让内存先冒充"已结束"，耐久却没跟上——那正是
/// 契约禁止的"内存冒充已结束"。
///
/// 封存失败与"活动没有结束"是两件事：结束事实已经确认，缺的只是"到此为止"的封存标记。
/// 因此它**不**回到活动未结束的状态，也不改写已确认的终态；它让调用方拿到一个带
/// [`CommitStage::Seal`] 的失败，从而如实区分"结束未完整确认"与"结束已确认、封存未确认"。
///
/// `cost` 是**搬进来的**：Core 不认识时长与用量，也不去算它们（见 [`crate::cost`]）。
/// 它由调用方（持有那两个来源的那一层）算好递进来，本函数只把它原样带进两条记录。
fn settle(
    run: &mut Run,
    agent: &mut Agent,
    ledger: &mut Ledger,
    commit: &mut dyn CommitSeam,
    outcome: Outcome,
    cost: RunCost,
) -> Result<(), RunError> {
    // 1. 耐久终态先确认。失败就在这里返回：活动没有结束，也不写内存事实、不释放槽位。
    commit.commit_ending(run.id(), outcome, cost)?;
    // 2. 内存事实。
    ledger.append(Record::RunEnded {
        run: run.id().clone(),
        outcome,
        cost,
    })?;
    // 3. 内存收场（释放活动槽）。到这里活动确实结束了。
    run.end(agent, outcome)?;
    // 4. 封存：它失败不代表活动没结束。
    commit.seal(run.id())?;
    Ok(())
}
