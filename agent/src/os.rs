//! 最小 OS 骨干：**窄接入契约、发现、渐进展开与机械路由**。
//!
//! 它**不是**操作系统：没有通知、没有唤醒、没有调度、没有安装、没有授权（Grant）系统；
//! 也**没有桌面、没有"返回上一级"、没有页面互斥状态机**。骨干只做四件机械的事：
//!
//! 1. **接入契约**：一个 App 只需要回答"我是谁、我做什么、我拥有哪些行动标识、怎么执行"，
//!    OS 不认识文件、经历、存储或任何具体 App 的内部实现；
//! 2. **发现**：入口能力说明**已装配**了哪些 App，以及一个"展开能力"的动作；
//! 3. **展开**：把某个 App 的能力说明作为**这一次行动的结果**追加进运行轨迹，并让这些能力从
//!    下一轮起可用。展开不列目录、不读文件、不替模型选择下一步；
//! 4. **路由**：能力目录与动作分派使用**同一条归属规则**（行动标识的唯一拥有者）。
//!
//! 渐进式披露控制的是"接下来新增什么"，不是"隐藏已经看过的内容"：
//!
//! - 入口只给稳定标识与一句话说明，不给正文、目录或其它 App 的能力；
//! - **每个 App 的展开状态互相独立**：展开 A 不开放 B；一个 App 也不需要知道另一个 App 是否展开；
//! - 未展开时伪造能力会被明确拒绝且**不触碰任何 App**；身份不匹配时能力不泄露、动作 fail-closed。
//!
//! 生命周期（本模块的核心约束）：
//!
//! - [`MinimalOs`] 是**长期存在**的设施，只持有已装配的 App 与两张机械索引，
//!   **不保存任何活动的展开状态**；
//! - 一次 Run 的展开状态归 [`RunExpansion`]（**自有值**）：它与那一次活动的身份绑定，
//!   由**调用方**（当前是会话）持有，因此可以跨越"停下这一次驱动、稍后再驱动同一个 Run"
//!   存活。它不再藏在作用域里——藏在作用域里意味着每次重新建立作用域都会得到一份**空**的
//!   展开集合，历史里明明有展开经历、当前却不再拥有那些能力；
//! - [`RunScope`] 从"拥有展开状态"变成**临时视图**：它同时借用 OS 与调用方持有的展开状态，
//!   只活在一次驱动期间，用完即销毁。因此不存在"历史 Run 的索引"，也不需要清理任务或 TTL；
//! - 作用域绑定明确的 Agent + Run（绑定事实由 [`RunExpansion`] 自己保存）：身份不匹配时
//!   能力不泄露、动作 fail-closed 且不执行任何 App 逻辑。这是**进程内一次活动的作用域绑定**，
//!   不是跨进程的身份唯一性保证。绑定写在 [`RunExpansion`] 上还有一个附带好处：
//!   把一个 Run 的展开集交给另一个 Run 是**类型上就构造不出来**的（见 [`MinimalOs::scope`]）。
//!
//! 展开动作复用 [`ActionPort`] 这个统一的机械调用载体，但**不产生任何写效应**，
//! 因此不需要跨边界写授权。
//!
//! **预装常开**：装配期可以声明若干 App "从第一轮起就在场"（预装）——它们的能力不需要
//! `open_app` 展开就在目录里，`open_app` 对它们回一张**门卡**（已开、动作数、目录位置）。
//! 预装是**装配决定**，不是 App 的自述；它只改"什么时候在场"，不改任何动词语义。
//!
//! 另一件属于环境层、不属于 Core 的事是**提供处境事实**：那一组显示区各自交自己的一块，
//! 处境链的最内层是活动起源那一件。本模块**不**提供任何处境内容——它只管"醒、看、点、收场"
//! 的机器、动作的发现与路由，以及伸出去的那些插槽。

use std::collections::{BTreeMap, BTreeSet};

use crate::app_contract::{
    AppPort, AssemblyFailure, EffectClass, SourceClass, SourceTable, VerbGroup,
};
use crate::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Capabilities,
    Capability, ContentBlock, ParamSpec,
};
use crate::creation_grant::CreationGrant;
use crate::file_view::{FileView, FileViewFailure};
use crate::foundation::{AgentId, OperationId, ResultState, RunId};
use crate::pinned_actions::MAX_PINS;

/// 展开动作的能力标识。名字沿用 `open_app`，**语义是展开能力，不是页面跳转**。
const CAPABILITY_EXPAND: &str = "open_app";

/// 看详情的动作标识（TASK_39）：OS 自己的动词，与展开同层——因此**常驻在场**。
///
/// 它是"工具呈现骨架/详情分离"的尾部那一半：`tools` 数组里每个动作只有一句骨架，
/// 而一个动作的**详情**（机械读数怎么读、语义边界、上限、失败形状）在这里取。
const CAPABILITY_INSPECT: &str = "inspect_action";

/// 钉子与摘钉子（TASK_68 P3）：把某个动作的**详情**钉在自己的视野里 / 摘下来。
///
/// 它们是 OS 自己的动词（与展开、看详情同层），因此**常驻在场**；改的是主体的**自持物**
/// （`<环境根>/pinned_actions`）——写自己的地盘，按 notes 的先例不需要授权。
/// 名字里带 `action` 是为了不与"贴在墙上的纸条"（`pins_board` / `notes_pin`）混：一词一主。
const CAPABILITY_PIN: &str = "pin_action";
const CAPABILITY_UNPIN: &str = "unpin_action";

/// 写下 / 改写主体的**自识**（TASK_70）：把自己写的那一篇自述落到 `<环境根>/self_concept`。
///
/// 与 `pin_action` 一样是 **OS 自己的动词**（不归任何 App、常驻在场），改的也是主体的**自持物**；
/// 不同的是它写的是"主体是谁"那一份——可以是空的，此后由自己写。
///
/// ⚠️ **一词一主**：它与 `pin_action`（动作的格位）、`notes_pin`（贴在墙上的纸条）是三件事，
/// 所以名字带 `self_concept` 而不复用 `pin`——`pin_action` 管的是 `pinned_actions`，**不碰** self_concept。
const CAPABILITY_SELF_CONCEPT_WRITE: &str = "write_self_concept";

/// 写下 / 改写主体的**交接**（`<环境根>/handoff`）：给**下一次开场**的自己留下
/// "这一段干到哪、下一段接着做什么"。
///
/// 与 `write_self_concept` 同族（OS 自己的动词、常驻在场、改的是主体的**自持物**、写权归自己），
/// 但两件事**不一样**：自识回答「我是谁」（常驻、跨段为真），交接回答「这一段我干到哪、
/// 下一段接着做什么」——它**有确定的交付时刻**（紧邻的下一次开场），摆过一次就不再摆。
const CAPABILITY_HANDOFF_WRITE: &str = "write_handoff";

/// **OS 自己的动词**：它们不归任何 App，因此**免归属索引**（它们由本模块自己分派）。
///
/// ⚠️ 这份名单是**一份**：装配面（撞名检查两处）、准入面（`admit` 的放行）、执行面（`execute`
/// 的分派）都必须问它——**分散写三遍就是三条"目录里有、路由里没有"的缝**。
const OS_OWNED_ACTIONS: [&str; 6] = [
    CAPABILITY_EXPAND,
    CAPABILITY_INSPECT,
    CAPABILITY_PIN,
    CAPABILITY_UNPIN,
    CAPABILITY_SELF_CONCEPT_WRITE,
    CAPABILITY_HANDOFF_WRITE,
];

/// 看详情时要给的那一个参数名（动作标识）。
const PARAM_ACTION: &str = "action";
/// 正文参数名（`write_self_concept` 用它；与 notes 那两处的同名参数同一个词）。
const PARAM_TEXT: &str = "text";

/// "这个名字你还没看过格式"的稳定分类（TASK_69，裁决 5）。
///
/// 它不是"你没有这个能力"：是**先看格式**这一轮在调用面上的投影，看一次就放行。
const NEEDS_INSPECTION: &str = "needs-inspection";
/// 展开动作的目标参数名。
const PARAM_TARGET: &str = "target";
/// 展开行动的拥有者不是任何 App，而是 OS 自己。
const OS_OWNER: &str = "<os>";

/// 装配（打开）这台 OS 被拒绝的原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MachineFailure {
    /// 授权资料目录 / 经历目录的配置被拒绝。
    View(FileViewFailure),
    /// 已装配 App 之间无法唯一解释。
    Assembly(AssemblyFailure),
}

impl MachineFailure {
    /// 稳定分类标记。
    pub fn as_str(&self) -> &'static str {
        match self {
            MachineFailure::View(failure) => failure.as_str(),
            MachineFailure::Assembly(failure) => failure.as_str(),
        }
    }
}

/// OS 自己的稳定失败分类（与各 App 的分类互不混淆）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpansionFailure {
    /// 这个能力属于尚未展开的 App：先展开，再使用。
    NotExpanded,
    /// 要展开的目标不是已装配的 App。
    UnknownTarget,
    /// 这个标识不属于任何已知能力（包括已经移除的旧动作）。
    UnknownAction,
    /// 参数不符合动作模式：缺少必需参数，或出现不认识的参数名。
    InvalidParameters,
    /// 调用者声明的 Agent/Run 与这个使用作用域绑定的身份不一致：fail-closed，不触碰任何 App。
    IdentityMismatch,
}

impl ExpansionFailure {
    /// 稳定分类标记。
    pub fn as_str(self) -> &'static str {
        match self {
            ExpansionFailure::NotExpanded => "not-expanded",
            ExpansionFailure::UnknownTarget => "unknown-target",
            ExpansionFailure::UnknownAction => "unknown-action",
            ExpansionFailure::InvalidParameters => "invalid-parameters",
            ExpansionFailure::IdentityMismatch => "identity-mismatch",
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
            "not-expanded" => ExpansionFailure::NotExpanded,
            "unknown-target" => ExpansionFailure::UnknownTarget,
            "unknown-action" => ExpansionFailure::UnknownAction,
            "invalid-parameters" => ExpansionFailure::InvalidParameters,
            "identity-mismatch" => ExpansionFailure::IdentityMismatch,
            _ => return None,
        })
    }
}

/// 参数形状差异那一块的标签（它是**内容**，不是分类）。
const PARAM_SHAPE_LABEL: &str = "parameters";

/// 拒绝时那条**指路读数**的标签（2026-10-14）：名字不在当前在场表里 ⇒ 说清它**在哪扇门后面**。
///
/// 与 [`PARAM_SHAPE_LABEL`] 各管一件事：那个说"这次调用的参数形状哪里不对"，这个说"这个名字
/// 属于哪个还没展开的 app"。分类词一个字不改（拒绝的理由仍是 `invalid-argument`）。
const DOOR_LABEL: &str = "door";

/// 展开失败时**对面自己那句话**那一块的标签（它是**内容**，不是分类）。
///
/// 与 [`PARAM_SHAPE_LABEL`] 同款：分类只有一个词（逐字节稳定），这一块是它**旁边**的读数。
/// **只有对面真的留了话才出现**——没有就整块不出场（"没有"与"读不到"分开表达，
/// 不写空值、也不编一句话）。
const EXPANSION_DIAGNOSTIC_LABEL: &str = "diagnostic";

/// 差异旁边那句话：参数名不必猜，它印在主体的视野里那张卡上。
///
/// 它只说得出这句话，是因为"免查 ⇒ 免学"这条既有自洽性要求：能走到参数校验的动作，
/// 参数名必定已经进过主体的视野（卡随单附过、或主体查过、或它本来就在格位/目录里印着）。
const PARAM_SHAPE_HINT: &str =
    "（参数名不必猜：你视野里那张动作卡上印着每个参数的名字与是否必填——照它填。）";

/// 参数形状不符的**具体差异**：缺了哪些名字、多了哪些名字、（可选）**这个动作接受哪些名字**。
///
/// 它是 `invalid-parameters` 的**随单内容**（TASK_72）：分类词照旧只有一个、逐字节稳定；
/// 差异是它旁边的结构化读数。名字按机械顺序枚举（多出来的按主体给的顺序，缺的、接受的都是
/// 声明的顺序），因此同一份错误参数两次发生，这段文本逐字节相同。
///
/// ⚠ **它不给"例子"**：只回答名字（多了哪些、缺了哪些、这个动作接受哪些）——"这一项该填什么
/// 值"不是 OS 手里的事实，编一个就是替 App 说话（本体裁为"不多说一个字"：读数只在真有机械
/// 事实可说时出现）。
struct ParamShape {
    missing: Vec<String>,
    unexpected: Vec<String>,
    /// **这个动作接受哪些名字**（可选；空 ⇒ **不印这一行**，于是既有回执逐字节不变）。
    ///
    /// 它治的是真机那一类失败（2026-10-13）：主体知道"`text` 不对"，却不知道"**对的是什么**"
    /// ——同一个 `notes_read` 被猜过五种名字（`note` / `note_id` / `identity` / `args` /
    /// `action`），13 次 `invalid-parameter`。名字来自**能力表**（[`ParamSpec`]），是机械事实。
    accepted: Vec<String>,
}

impl ParamShape {
    /// 这段**差异读数**（分类之外的那一半）。
    ///
    /// 与 [`ParamShape::outcome`] 分开，是因为同一个读数要贴在**两个不同的分类**旁边：
    /// OS 自有动作的 `invalid-parameters`（参数形状不对）与 App 动作的 `invalid-argument`
    /// （参数不合规）——分类各是各的词，读数**只有一份拼法**。
    fn reading(&self) -> String {
        let mut text = String::new();
        if !self.missing.is_empty() {
            text.push_str("missing = ");
            text.push_str(&self.missing.join(", "));
            text.push('\n');
        }
        if !self.unexpected.is_empty() {
            text.push_str("unexpected = ");
            text.push_str(&self.unexpected.join(", "));
            text.push('\n');
        }
        if !self.accepted.is_empty() {
            text.push_str("accepted = ");
            text.push_str(&self.accepted.join(", "));
            text.push('\n');
        }
        text.push_str(PARAM_SHAPE_HINT);
        text
    }

    /// 这一类拒绝的结果：稳定分类 + 差异块。
    fn outcome(&self) -> ActionOutcome {
        ActionOutcome::failure_with_content(
            ExpansionFailure::InvalidParameters.as_str(),
            vec![ContentBlock::new(PARAM_SHAPE_LABEL, self.reading())],
        )
    }
}

/// 最小 OS：长期持有**已装配**的 App 与两张机械索引。
///
/// 它**不保存**任何活动的展开状态——没有键映射、没有历史表，因此不存在"历史 Run 累计"这件事，
/// 也不需要清理任务。展开状态请通过 [`MinimalOs::scope`] 取一次活动的使用作用域。
///
/// ⚠ **2026-09-26 改**：它原来还另持一张**效果声明表**（`EffectTable`：行动标识 → 效果类别 ＋ 复核
/// 能力）。那张表随未决效果门一起退役了——**门是它唯一的生产读者**（`RunScope::admit` 按类别拦写），
/// 门一走，它就只剩"装配期收集、之后无人读"这一半，于是连收集一起删掉（考古见 Git 历史）。
/// **效果声明本身没退**：`AppPort::declared_effects()` 与 [`check_declarations`] 一步没动——
/// "写效应必配复核动词"这条自洽检查照旧在装配期与展开期各跑一次。
///
/// ⚠ **2026-10-13 补**：它现在持一格**效果类别**（只有类别，没有复核能力那一半），
/// 但**不是那扇门回来了**——门是指令（"未决的写一律拒"），这一格是**读数**
/// （"这个动作改不改外面的世界"）。它唯一的读者是准入面的一处判断：
/// **读没有风险，就不该收"先看形状"的税**（只读动作首调直接执行；卡按需取，不随回执附）。
/// 它是各 App 自己声明的**投影**（与 `sources` 同款）：装配期与展开期各收一次，
/// 缺项按契约的默认方向读成只读。
pub struct MinimalOs {
    apps: Vec<Box<dyn AppPort>>,
    /// 行动标识 → 拥有它的 App 位置（唯一归属，装配时建立）。
    owners: BTreeMap<String, usize>,
    /// App 标识 → 位置。
    positions: BTreeMap<String, usize>,
    /// **效果类别**的投影（行动标识 → 类别；各 App 自己声明的）。
    ///
    /// **不是权限表**：它不拦人、不授予、也不替代准入；它只回答"这一次调用要不要先看形状"。
    /// 缺项 = 只读（契约的默认方向：不声明是合法状态，默认指向更少的能力）。
    effects: BTreeMap<String, EffectClass>,
    /// 来源声明表（装配期从已装配 App 收集，之后只读）。
    ///
    /// 它答"回执是谁说的"。它同样是机械收集，**不授予任何东西**，也不承载"可不可信"。
    sources: SourceTable,
    /// **预装常开**的 App 标识（装配期声明）。
    ///
    /// 它是**装配事实**，不是展开状态：不随活动变化、不写任何地方、也不撤销。预装的 App
    /// 的能力从第一轮起就在场（预装 = 永在已开清单），因此 `open_app` 对它们回一张门卡
    /// （已开、动作数、目录位置）。选择谁在这里归 Composition——本模块不认识任何具体 App。
    preinstalled: BTreeSet<String>,
    /// 自己的环境根（**装配事实**，TASK_68 P3）。
    ///
    /// 自持物（`<环境根>/notes/`、`<环境根>/pinned_actions`）都落在这里，所以"改自己格位"的动作
    /// 需要一个去向。它是 `Option`：**装配入口不都一样**——`assemble*` 系列（测试与构造入口）
    /// 不带环境根，于是那些舞台里这类动作**明确拒绝**（"这一次装配没有环境根"），
    /// 而不是静默写到一个猜出来的路径上。
    environment_root: Option<std::path::PathBuf>,
}

impl std::fmt::Debug for MinimalOs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ids: Vec<&str> = self.apps.iter().map(|app| app.app_id()).collect();
        f.debug_struct("MinimalOs").field("apps", &ids).finish()
    }
}

/// **桌面的一层**（TASK_71 桌面 P0）：一扇 App 的门。
///
/// - `id` 就是主体 `open_app` 要给的 target；
/// - `description` 是 App 自己那句话（门牌）；
/// - `verbs` 用 [`AppPort::declared_capabilities`] ——**与展开状态无关**的那份声明；
/// - `preinstalled` 是**在场**那一格：它的唯一权威就是本模块那份预装集合（[`MinimalOs::is_preinstalled`]）。
///
/// ⚠ 为什么"装配期静态事实"就够，以及**判据为什么是 `preinstalled` 而不是数动词**（2026-10-08 改）：
/// 目录只在**首轮**渲染（`is_opening_turn`），而那时**还没有任何展开** ⇒ 那一刻"开着的"**恰好就是
/// 常开的那几个**。因此门后那句「还没打开」必须由 `preinstalled` 决定——**不许**再由"它的动词有没有
/// 出现在这一次的能力面里"反推：那是同一件事的**第二处判据**，而两者在一个"**常开却零能力**"的装配上
/// 会分岔（首屏会说「还没打开」，而它其实是开着的）。展开之后新出现的动作仍由**展开回执**自带
/// （既有行为，不另做机制）。
///
/// ⚠ **"常开"与"零能力"可以同时成立**：装配期**允许**这种形状（"装配期拒绝常开零能力"
/// 那条提案已撤回），所以上面那个分岔不是纸面上的——它真实可达，不能靠"装配期不许这么装"绕过去。
///
/// ⚠ 这一格**不是**第二个「在场」判据：它就是唯一那条（[`MinimalOs::is_preinstalled`]）在"还没有任何
/// 展开"那一刻的取值，因此判据本身不动、展开状态也不动。
///
/// ⚠ 上面那个前提（**目录只在首轮渲染**）有一条守卫钉着：
/// `tests/minimal_agent.rs::the_action_directory_is_rendered_only_on_the_opening_turn`——`action_directory_batch`
/// 只许有**一处定义 ＋ 一处调用**，且那一处必须在 `is_opening_turn()` 为真的分支里
/// （证伪过：在别的轮次再加一处调用 ⇒ 当场红，实得 3 处 vs 期望 2 处）。
///
/// 证明（**行为**，不只是源码形状）：
/// `tests/status_bar.rs::the_action_directory_groups_verbs_under_their_app_and_keeps_unopened_doors_on_the_board`
/// ——预装的门列全它声明的动词、未展开的门门后只有一句提示。把 `preinstalled` 一律算成 `false`，
/// 那一条当场红。
///
/// ⚠ **如实登记**：那条守卫是**形状判据**，它**量不到**"换了形状之后这句提示还算得对不对"——它红只
/// 说明这个前提变了，要换的是判据（活的展开状态），不是断言。
pub struct AppDoor {
    /// App 标识（`open_app` 的 target）。
    pub id: String,
    /// App 自己写的那句话（门牌）。
    pub description: String,
    /// 它声明的动词标识（**声明顺序**，与展开状态无关）。
    pub verbs: Vec<String>,
    /// App 自己声明的**显示分组**（只影响动作目录的排列；空 = 平铺）。
    ///
    /// 分组不是能力声明：`verbs` 仍是唯一清单，未进组的动词由渲染者平铺补齐。
    pub verb_groups: Vec<VerbGroup>,
    /// 这个 App 是不是**预装常开**（`assemble_preinstalled` 声明的那几个）。
    pub preinstalled: bool,
}

/// **桌面**（TASK_71 桌面 P0）：主体醒来第一眼的两层结构 —— **门之外**那一组，以及每一扇门。
///
/// "门之外"= 不归任何 App 的动作（OS 自有那六个：展开、看格式、钉/摘、写自识、写交接）。
pub struct Desktop {
    /// 不归任何 App 的动作标识（**一份来源**：`OS_OWNED_ACTIONS`）。
    pub system: Vec<String>,
    /// `system` 里**负责开门的那一个**：未展开的门后那句提示要指名它。
    ///
    /// 它是**从 `system` 里挑出来的**，不是另写一个常量 —— 于是提示不可能指名一个不在榜上的动作；
    /// 而"导航属于谁"这条判断也留在 OS 自己这里：渲染者只是把 OS 给出的名字抄进一句话。
    pub opener: Option<String>,
    /// 已装配 App 的门（按装配顺序）。
    pub doors: Vec<AppDoor>,
    /// **写动作**的标识（装配期从效果声明收一次）：准入时它们要先给卡。
    ///
    /// 呈现面用同一条事实：不在这一格里的动作**不先看卡就能调**，所以目录行必须自带参数名。
    pub writes: BTreeSet<String>,
}

impl MinimalOs {
    /// 桌面的一层（见 [`Desktop`]）。**装配期事实**：调用它不读盘、不看展开状态。
    pub fn desktop(&self) -> Desktop {
        let system: Vec<String> = OS_OWNED_ACTIONS
            .iter()
            .map(|id| (*id).to_string())
            .collect();
        Desktop {
            opener: system
                .iter()
                .find(|id| id.as_str() == CAPABILITY_EXPAND)
                .cloned(),
            system,
            writes: self
                .effects
                .iter()
                .filter(|(_, effect)| effect.is_write())
                .map(|(id, _)| id.clone())
                .collect(),
            doors: self
                .apps
                .iter()
                .map(|app| AppDoor {
                    id: app.app_id().to_string(),
                    description: app.description().to_string(),
                    verbs: app
                        .declared_capabilities()
                        .items()
                        .iter()
                        .map(|capability| capability.id().as_str().to_string())
                        .collect(),
                    verb_groups: app.verb_groups(),
                    // **在场那一格派生自唯一权威**（本模块那份预装集合），不是另抄一份。
                    preinstalled: self.preinstalled.contains(app.app_id()),
                })
                .collect(),
        }
    }
}

impl MinimalOs {
    /// **显式装配**：按给定顺序接受已经构造好的 App。
    ///
    /// 具体选择与构造归 Composition；这里只做可判错的机械检查：重复的 App 标识、
    /// 被多个拥有者声明的行动标识（含与 OS 自己的展开行动冲突），以及**效果声明的自洽性**
    /// （声明落在自己的能力里、写类别配了复核、复核也在自己的能力里）。
    ///
    /// 效果检查是**装配期**的，因此它早于任何事实与任何模型调用：一个"说了写却没配复核"的 App
    /// 根本装不起来，不会等到崩溃之后才发现没有复核入口。这里不检查"声明是不是真的"——
    /// 那是行为测试与源码守卫能缓解、但无法机械封闭的一格（见 README 的缺口清单）。
    ///
    /// 没有插件发现、没有全局注册中心、没有安装机制，也没有事件总线。
    ///
    /// 这一版等价于 [`MinimalOs::assemble_preinstalled`] 传一个空的预装清单：谁都不是常开。
    pub fn assemble(apps: Vec<Box<dyn AppPort>>) -> Result<Self, AssemblyFailure> {
        Self::assemble_preinstalled(apps, &[])
    }

    /// **显式装配（含预装常开声明）**：`preinstalled` 里的 App 的能力从第一轮起就在场。
    ///
    /// 预装是**装配期的选择**，不是 App 的自述——所以它在这里给，而不是让 App 自己声明
    /// "我是预装的"（那是把装配决定写进被装配者）。
    /// 声明一个没装配的标识会被装配期拒绝（[`AssemblyFailure::UnknownPreinstalledApp`]）：
    /// 静默忽略等于"声明了一件不存在的事"。
    ///
    /// ⚠ 反向**不**检查：一个**常开却零能力**的 App 是**允许**的装配形状（"装配期拒绝它"那条提案
    /// 已撤回）。所以门后那句「还没打开」的分岔是真实可达的，见 [`AppDoor`] 的文档。
    pub fn assemble_preinstalled(
        apps: Vec<Box<dyn AppPort>>,
        preinstalled: &[&str],
    ) -> Result<Self, AssemblyFailure> {
        let mut owners: BTreeMap<String, usize> = BTreeMap::new();
        let mut positions: BTreeMap<String, usize> = BTreeMap::new();
        let mut effects: BTreeMap<String, EffectClass> = BTreeMap::new();

        for (index, app) in apps.iter().enumerate() {
            let id = app.app_id().to_string();
            if positions.insert(id.clone(), index).is_some() {
                return Err(AssemblyFailure::DuplicateAppId(id));
            }
            for action in check_declarations(app.as_ref())? {
                index_action(&mut owners, &apps, index, action)?;
            }
            // 效果类别**在装配期收一次**（`declared_effects()` 的文档要求按"此刻"读一次，
            // 因为它必须与实例当前的能力同源派生）。缺项不补——读的时候按只读兜底。
            for declaration in app.declared_effects() {
                effects.insert(
                    declaration.action().as_str().to_string(),
                    declaration.class(),
                );
            }
        }

        // 装配期已经保证每条声明都落在拥有者自己的能力里（上面那一轮立刻拒绝）、写类别都配了
        // 复核、**每个能力都被声明到了来源档**，因此这里只是**收集**，不重复校验。
        let mut sources = SourceTable::of(&apps);
        // OS 自己的六个动词**不归任何 App**，因此不由任何 App 声明。它们的唯一来源清单就是
        // 上面那份 `OS_OWNED_ACTIONS`——把它一并登记进来源表，免得主体的视野里出现
        // "系统自己的动作来源不知道"这种荒谬读数。
        //
        // 六个都是 `system`：展开 / 看详情 / 钉 / 摘 交回的是**系统渲染的读数**（能力骨架、
        // 格位现状），写自识与写交接交回的是**系统对这次动作的确认**——六段回执里都没有外来字。
        for action in OS_OWNED_ACTIONS {
            sources.declare_os_owned(action, SourceClass::System);
        }

        let mut declared_preinstalled: BTreeSet<String> = BTreeSet::new();
        for id in preinstalled {
            if !positions.contains_key(*id) {
                return Err(AssemblyFailure::UnknownPreinstalledApp {
                    app: (*id).to_string(),
                });
            }
            declared_preinstalled.insert((*id).to_string());
        }

        Ok(Self {
            apps,
            owners,
            positions,
            effects,
            sources,
            preinstalled: declared_preinstalled,
            // 装配入口默认**不带**环境根：只有明确声明过它的装配（见 `with_environment_root`）
            // 才能改自持物。缺省不是"随便找个路径"，是"这次装配没有"。
            environment_root: None,
        })
    }

    /// 声明这一次装配的**环境根**（TASK_68 P3）：自持物动作的去向。
    ///
    /// 做成链式而不是构造参数，是因为装配入口有五个（`assemble*` / `open` / `with_history*`），
    /// 而"要不要带环境根"与"怎么装 App"是两件事——只有真正托着主体的那一条装配需要它。
    /// **没有它时相关动作明确拒绝**（fail-closed），不会静默写到别处。
    pub fn with_environment_root(mut self, environment_root: &std::path::Path) -> Self {
        self.environment_root = Some(environment_root.to_path_buf());
        self
    }

    /// 这一次装配的环境根；`None` = 没声明过（自持物动作会拒绝）。
    pub fn environment_root(&self) -> Option<&std::path::Path> {
        self.environment_root.as_deref()
    }

    /// **展开那一下**把某个 App 交出来的清单并进两张索引：归属（路由用）与来源声明（回执口径用）。
    ///
    /// 出厂内置件的清单早在装配期就在那两张表里了，并一次是幂等的；只有**外部来源的 App**
    /// 会真的带来新东西——它们的清单在装配期还不存在。
    ///
    /// ⚠ **2026-09-26 改**：这里原来还并第三张表（效果声明表）。它与未决效果门一起退役了——
    /// 门是那张表唯一的生产读者（见 [`MinimalOs`] 的文档）。**声明自洽性检查一步没动**：
    /// 下面第一行的 [`check_declarations`] 在展开期照旧整批先判（外部 App 的声明装配期还不存在，
    /// 因此这一查不是"装配期查过就永久成立"）。
    ///
    /// ⚠ **2026-10-13 补**：效果**类别**那一格又并回来了（[`MinimalOs::effects`]），
    /// 但只是类别、不是那扇门——外部来源的 App 到展开这一下才有声明，因此它必须跟着并。
    /// 并法与归属/来源同一批：**先判后落**，拒绝时一格都不落。
    ///
    /// **整批先判后落**：撞名（与 OS 自己的动词撞、与别的 App 撞）一律拒绝，而且拒绝时**一格
    /// 都没落**——半截注册会让"目录里有、路由里没有"这种裂缝真的发生。分类沿用装配期那一套
    /// （同一份判据、同一批名字），因此主体看到的分类与宿主的诊断是同一个词。
    fn absorb(&mut self, index: usize) -> Result<(), AssemblyFailure> {
        let declared = check_declarations(self.apps[index].as_ref())?;
        for action in &declared {
            if OS_OWNED_ACTIONS.contains(&action.as_str()) {
                return Err(AssemblyFailure::ConflictingActionId {
                    action: action.clone(),
                    owner: OS_OWNER.to_string(),
                });
            }
            if let Some(previous) = self.owners.get(action)
                && *previous != index
            {
                return Err(AssemblyFailure::ConflictingActionId {
                    action: action.clone(),
                    owner: self.apps[*previous].app_id().to_string(),
                });
            }
        }
        for action in declared {
            self.owners.insert(action, index);
        }
        // 效果类别跟着同一批并进来（外部来源的 App 到展开这一下才有声明）。
        for declaration in self.apps[index].declared_effects() {
            self.effects.insert(
                declaration.action().as_str().to_string(),
                declaration.class(),
            );
        }
        self.sources.absorb(self.apps[index].as_ref());
        Ok(())
    }

    /// 这一刻这个动作**改不改外面的世界**（各 App 自己声明的投影）。
    ///
    /// 没声明 ⇒ **只读**（契约的默认方向：不声明是合法状态，默认指向更少的能力）。
    /// 它是**读数**：不授予、不拦人，只回答"这一次调用要不要先看形状"。
    fn effect_of(&self, action: &str) -> EffectClass {
        self.effects
            .get(action)
            .copied()
            .unwrap_or(EffectClass::ReadOnly)
    }

    /// 某个 App 是不是**预装常开**（装配期声明；只读机械事实）。
    pub fn is_preinstalled(&self, app_id: &str) -> bool {
        self.preinstalled.contains(app_id)
    }

    /// 只装配一个授权资料目录的只读文件视图（没有经历目录）。
    ///
    /// 根必须显式给出：打开失败就是拒绝启动。
    pub fn open(root: &std::path::Path) -> Result<Self, MachineFailure> {
        let view = FileView::materials_only(root)
            .map_err(|class| MachineFailure::View(FileViewFailure::Materials(class)))?;
        Self::assemble(vec![Box::new(view)]).map_err(MachineFailure::Assembly)
    }

    /// 装配授权资料目录与派生经历目录的只读文件视图。
    ///
    /// 两个根必须明确分开：资料根覆盖环境根会被拒绝（见 [`FileView::with_history`]）。
    pub fn with_history(
        materials: &std::path::Path,
        environment_root: &std::path::Path,
    ) -> Result<Self, MachineFailure> {
        Self::with_history_and_creation(materials, environment_root, None)
    }

    /// 与 [`MinimalOs::with_history`] 相同，只是可以带一份**文本创建授权**。
    ///
    /// 授权在**装配期**决定"这一项创建能力到底存不存在"：没有授权时它连声明都不会出现，
    /// 因此伪造同名动作在归属索引里就找不到主人。这是"默认只读"的结构性门——它早于任何事实与
    /// 任何模型调用。
    pub fn with_history_and_creation(
        materials: &std::path::Path,
        environment_root: &std::path::Path,
        creation: Option<&CreationGrant>,
    ) -> Result<Self, MachineFailure> {
        let view = FileView::with_history_and_creation(materials, environment_root, creation)
            .map_err(MachineFailure::View)?;
        Self::assemble(vec![Box::new(view)]).map_err(MachineFailure::Assembly)
    }

    /// 已装配 App 的标识（按装配顺序）。只用于诊断与测试。
    pub fn app_ids(&self) -> Vec<&str> {
        self.apps.iter().map(|app| app.app_id()).collect()
    }

    /// 装配期收集的**来源声明表**（只读）。
    ///
    /// 它是各 App 自己那句话的机械收集，**不授予任何东西**，也不回答"可不可信"。
    /// 查询一律走 [`SourceTable::class_if_assembled`]——"这次没有这件能力"必须读成**不知道**，
    /// 不许兜底成一个档。
    ///
    /// ⚠ **2026-09-26 改**：它原来与一张**效果声明表**并列（那张表答"会不会改世界"，这张表答
    /// "回执是谁说的"）。效果表随未决效果门退役了——门是它唯一的生产读者。**这一张本刀一步没动。**
    ///
    /// ⚠ **2026-10-08 改：这一张终于有了生产读者**（见 `agent/README.md` §3.45）——
    /// **详情卡**那一行 `receipt-source`（`inspect_action` 与准入面随单附卡**同一份渲染**）。
    /// 在那之前它只有测试消费者，而"判断归主体"那条设计**缺了它的输入**：
    /// 声明是装配期强制的（漏填即拒），答案却从不送出去。
    ///
    /// **为什么是"接"不是"撤"**（判据留在它的读者旁边）：撤了表，装配期那条
    /// [`AssemblyFailure::CapabilityWithoutSource`] 就变成"**回答给谁听**"——一道没有收件人的
    /// 必答题。⚠ 姊妹那张**效果声明表**退役的理由是"**门是它唯一的生产读者**"：那张表是
    /// **读者先没了**，这一张是**读者从没建过**——前者该随读者一起走，后者该把读者补上。
    pub fn sources(&self) -> &SourceTable {
        &self.sources
    }

    /// 为一次**具体活动**建立使用作用域（**临时视图**）。
    ///
    /// 构造者：驱动这次活动的调用方；持有者：这次活动的调用栈；结束：作用域随调用返回而销毁，
    /// OS 与 App 继续复用，**展开状态留在调用方手里**（它不随作用域销毁）。
    ///
    /// 这里返回的借用是 `&mut`，所以在作用域存活期间 OS 不能被别处同时拿去用；而展开状态也是
    /// `&mut`，因此"OS 与它作为两个不相交字段同时可变借用"是合法的——Rust 允许不相交字段同时
    /// 可变借用，不需要 `unsafe`、`RefCell` 或自引用对象。
    ///
    /// **重新驱动同一个 Run 必须复用同一份 [`RunExpansion`]**：这里没有"按身份新建一份空展开集"
    /// 的入口——那正是本刀修掉的形状（每次重建空集合会让已展开的能力凭空消失）。
    pub fn scope<'a>(&'a mut self, expansion: &'a mut RunExpansion) -> RunScope<'a> {
        RunScope {
            os: self,
            expansion,
        }
    }
}

/// 一个 App 的**声明自洽性检查**（装配期与展开期共用同一份判据）。
///
/// 效果三条互不顶替：声明必须落在自己的能力里、有路可复核的写必须配复核、复核必须也在自己的能力里。
/// 来源两条同样互不顶替：**多声明**了一个不属于自己的行动、**漏声明**了一个自己的能力。
/// 返回它声明的行动标识，**按声明顺序**（冲突报告因此与声明顺序同源）。
///
/// 装配期检查出厂内置件；**展开期检查外部来源的 App**——那里的声明来自一台外部 server，
/// 因此同一份判据要再走一次（不是"装配期查过就永久成立"）。
fn check_declarations(app: &dyn AppPort) -> Result<Vec<String>, AssemblyFailure> {
    let id = app.app_id().to_string();
    let mut declared: Vec<String> = Vec::new();
    let mut known: BTreeSet<String> = BTreeSet::new();
    for capability in app.declared_capabilities().items() {
        let action = capability.id().as_str().to_string();
        known.insert(action.clone());
        declared.push(action);
    }

    for effect in app.declared_effects() {
        let action = effect.action().as_str().to_string();
        if !known.contains(&action) {
            return Err(AssemblyFailure::UnknownDeclaredEffect { app: id, action });
        }
        let Some(verification) = effect.verification() else {
            // 只读声明不需要复核；**有路可复核的写**必须配；而 `unverifiable-write`
            // 自成一类的理由就是"没有同参复核可配"，因此放行——它在表里仍然是写
            // （代价与出路见那个类别的文档）。
            if effect.class().requires_verification() {
                return Err(AssemblyFailure::WriteWithoutVerification { app: id, action });
            }
            continue;
        };
        if !known.contains(verification.as_str()) {
            return Err(AssemblyFailure::VerificationNotDeclared {
                app: id,
                action,
                verification: verification.as_str().to_string(),
            });
        }
    }

    // 来源：先收声明（多出来的立刻拒绝），再要求**每一个能力**都被声明到。
    // 两个方向都要拒绝，因为来源没有安全的兜底值——漏一个就是把一句假话写进主体的视野。
    let mut sourced: BTreeSet<String> = BTreeSet::new();
    for source in app.declared_sources() {
        let action = source.action().as_str().to_string();
        if !known.contains(&action) {
            return Err(AssemblyFailure::UnknownDeclaredSource { app: id, action });
        }
        sourced.insert(action);
    }
    if let Some(missing) = declared.iter().find(|action| !sourced.contains(*action)) {
        return Err(AssemblyFailure::CapabilityWithoutSource {
            app: id,
            action: missing.clone(),
        });
    }

    Ok(declared)
}

/// 把一个行动标识登记到**唯一归属索引**上；撞了 OS 自己的动词或别人的标识就拒绝。
fn index_action(
    owners: &mut BTreeMap<String, usize>,
    apps: &[Box<dyn AppPort>],
    index: usize,
    action: String,
) -> Result<(), AssemblyFailure> {
    // OS 自己的两个动词（展开 / 看详情）不归任何 App：被谁声明了，谁就装不上。
    if OS_OWNED_ACTIONS.contains(&action.as_str()) {
        return Err(AssemblyFailure::ConflictingActionId {
            action,
            owner: OS_OWNER.to_string(),
        });
    }
    match owners.insert(action.clone(), index) {
        None => Ok(()),
        Some(previous) => Err(AssemblyFailure::ConflictingActionId {
            action,
            owner: apps[previous].app_id().to_string(),
        }),
    }
}

/// 一次活动的**展开状态**：与 `(Agent, Run)` 绑定，是会话状态的一部分，**跨"停下这次驱动"存活**。
///
/// 它是自有值而不是作用域里的临时集合，因此"同一次活动稍后再驱动"能继续拥有此前展开的能力。
/// 绑定的身份是**数据**而不是参数：把一个 Run 的展开集交给另一个 Run 在类型上就构造不出来
/// （[`MinimalOs::scope`] 只接受一份 `RunExpansion`，而它自己记着属于谁）。
///
/// 它只保存"哪些已装配 App 的能力被展开过"，没有历史映射、没有撤销、没有 TTL，也不认识任何
/// 具体 App。
pub struct RunExpansion {
    agent: AgentId,
    run: RunId,
    expanded: BTreeSet<String>,
    /// 这一次活动里**卡已经进过主体的视野**的动作（TASK_69 裁决 5；TASK_72 起有两个入口）。
    ///
    /// 免查判据之一：卡进过视野的可以直接调；没有卡也没被钉住的，要先看格式。
    ///
    /// **唯一台账**：两个入口都写它——`inspect_action` 交回卡片那一下，以及准入面的
    /// **随单附卡**（拒绝时把同一张卡附在答复上）。写点只有 [`RunExpansion::note_inspected`]
    /// 一处，因此"卡进视野"与"可直调"不可能各说各话。
    ///
    /// 用 `RefCell` 是因为**两个写点都拿不到 `&mut self`**（`ActionPort::admit` 是 `&self`，
    /// 而 `execute` 里那一下也用同一个入口），理由与下面 `pinned` 的 `OnceCell` 同款：
    /// `RunExpansion` 是独占借用（一个 Run 一个作用域），没有并发与重入可谈。
    inspected: std::cell::RefCell<BTreeSet<String>>,
    /// 生效的 pin 集，**惰性算一次**（TASK_69）。
    ///
    /// 读盘只发生在 Run 边界那一次（自持物 ＋ 出厂默认）；之后这一份就是**这一次活动的开场
    /// 视野**——这正是"pin 只在 Run 边界生效"的落点：一个 Run 之内主体再 pin 也改不了它。
    ///
    /// 用 `OnceCell` 是因为**判门的地方拿不到 `&mut self`**（`ActionPort::admit` 是 `&self`），
    /// 而"算一次"本身是幂等的；`RunExpansion` 又是独占借用（一个 Run 一个作用域），
    /// 因此这里的内部可变性没有并发与重入可谈。
    pinned: std::cell::OnceCell<BTreeSet<String>>,
}

impl std::fmt::Debug for RunExpansion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunExpansion")
            .field("agent", &self.agent)
            .field("run", &self.run)
            .field("expanded", &self.expanded)
            .finish()
    }
}

impl RunExpansion {
    /// 为一次**新的**活动建立展开状态：从"什么都没展开"开始。
    pub fn new(agent: AgentId, run: RunId) -> Self {
        Self {
            agent,
            run,
            expanded: BTreeSet::new(),
            inspected: std::cell::RefCell::new(BTreeSet::new()),
            pinned: std::cell::OnceCell::new(),
        }
    }

    /// 这份展开状态绑定的活动主体。
    pub fn agent(&self) -> &AgentId {
        &self.agent
    }

    /// 这份展开状态绑定的活动身份。
    pub fn run(&self) -> &RunId {
        &self.run
    }

    /// 这一次活动里某个 App 的能力是否已经展开。
    ///
    /// 它是**只读机械事实**：展开状态的唯一权威就在这份值里，这里只把它读出来，不提供任何修改
    /// 入口（展开只能通过 `open_app` 动作发生）。
    pub fn is_expanded(&self, app_id: &str) -> bool {
        self.expanded.contains(app_id)
    }

    /// 这一次活动里某个动作的**卡是否已经进过主体的视野**。
    ///
    /// 只读机械事实（同 [`RunExpansion::is_expanded`]：权威在值里，这里只读）。
    pub fn is_inspected(&self, action: &str) -> bool {
        self.inspected.borrow().contains(action)
    }

    /// 记下"这个动作的卡已经进过主体的视野"。
    ///
    /// **唯一写口**：`inspect_action` 交回卡片那一下，与准入面的随单附卡，都从这里进。
    /// 语义只有一个：**卡进过视野**（不是"主体学会了"、不是"主体被授权了"）。
    pub fn note_inspected(&self, action: &str) {
        self.inspected.borrow_mut().insert(action.to_string());
    }
}

/// 一次活动的展开**使用作用域**：同一份状态同时提供**能力**与**执行准入**。
///
/// 它只是 [`RunExpansion`] 与 [`MinimalOs`] 的临时视图，**不拥有**任何状态；
/// 绑定的 Agent/Run 之外的调用一律 fail-closed：不泄露能力、不触碰任何 App。
pub struct RunScope<'a> {
    os: &'a mut MinimalOs,
    expansion: &'a mut RunExpansion,
}

impl std::fmt::Debug for RunScope<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunScope")
            .field("agent", self.expansion.agent())
            .field("run", self.expansion.run())
            .field("expansion", &self.expansion)
            .finish()
    }
}

impl RunScope<'_> {
    /// 这个作用域绑定的活动主体。
    pub fn agent(&self) -> &AgentId {
        self.expansion.agent()
    }

    /// 这个作用域绑定的活动身份。
    pub fn run(&self) -> &RunId {
        self.expansion.run()
    }

    /// 这一次活动里某个 App 的能力是否已经展开（读的是调用方持有的那份展开状态）。
    pub fn is_expanded(&self, app_id: &str) -> bool {
        self.expansion.is_expanded(app_id)
    }

    /// 调用者声明的身份是否就是这个作用域绑定的身份。
    fn matches(&self, agent: &AgentId, run: &RunId) -> bool {
        agent == self.expansion.agent() && run == self.expansion.run()
    }

    /// 这个 App 的能力**此刻是否在场**：**预装常开**，或这一次活动里展开过。
    ///
    /// "在场"只有这一处判据：能力目录、`open_app` 的重复展开答复、准入与分派四处都用它。
    /// 四处各写一遍，就总有一天会写出"目录里有、分派里没有"这种裂缝。
    fn is_open(&self, app_id: &str) -> bool {
        self.os.is_preinstalled(app_id) || self.expansion.is_expanded(app_id)
    }

    /// 入口能力：说明已装配了哪些 App、它们各自做什么，以及"展开能力"。
    ///
    /// 刻意**不**出现任何 App 的内部能力、参数模式、文件名或正文——那些要等展开之后才出现。
    /// 预装常开的 App 在这里被标注出来：它们的能力已经在场，`open_app` 对它们回一张门卡
    /// （已开、动作数、目录位置）——这件事必须在**入口**就说清，否则门卡看起来像故障。
    fn entry_capabilities(&self) -> Capabilities {
        let described: Vec<String> = self
            .os
            .apps
            .iter()
            .map(|app| {
                if self.is_open(app.app_id()) {
                    format!("{}（{}，已在场）", app.app_id(), app.description())
                } else {
                    format!("{}（{}）", app.app_id(), app.description())
                }
            })
            .collect();
        Capabilities::of(vec![
            Capability::new(
                ActionId::new(CAPABILITY_EXPAND),
                "展开一个已装配 App 的能力；展开之后它的能力才会出现在可用能力里；已开的再调回一张门卡",
                vec![ParamSpec::new(PARAM_TARGET, true, "要展开的 App 标识")],
            )
            .exempt_from_inspection()
            .with_details(format!(
                "给一个 App 标识：它的能力随后进入你的可用能力（重复展开不重读、不复制动词表；\
                 回一张门卡：名字、已开、动作数、目录位置）。装了什么：{}。没装的名字会被拒\
                 （unknown-target），不猜近似名。详情卡片用 `{CAPABILITY_INSPECT}` 取。",
                described.join("；")
            )),
            Capability::new(
                ActionId::new(CAPABILITY_INSPECT),
                "看一个动作的详情卡片：骨架 + 详情 + 参数表（只读，不改任何东西）",
                vec![ParamSpec::new(
                    PARAM_ACTION,
                    true,
                    "要看的动作名（就是工具名，例如 files_read）",
                )],
            )
            .exempt_from_inspection()
            .with_details(
                "可看的范围 = 你此刻真能调的那些动作（含 `open_app` 与它自己）：没展开的 App \
                 的动作、以及不存在的名字，一律 `invalid-argument`——不猜近似名、也不透传。\
                 卡片是一次性的机械渲染，看它不消耗行动资格、不产生任何效果。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_PIN),
                "把一个动作的详情钉在自己的视野里：下一次开场它会连着参数表一起摆出来",
                vec![ParamSpec::new(
                    PARAM_ACTION,
                    true,
                    "要钉的动作名（就是工具名，例如 notes_read）",
                )],
            )
            .exempt_from_inspection()
            .with_details(format!(
                "钉住之后，下一次开场（不是这一次）它的详情会随开场的格位清单一起在场：\
                 不用再看一眼格式就能直接调。可钉的范围 = 你此刻真能调的那些动作（与 \
                 `{CAPABILITY_INSPECT}` 同一份判据）；不在场的名字 `invalid-argument`。\
                 重复钉是明确的成功结果。摘掉用 `{CAPABILITY_UNPIN}`：你摘的只是「你自己钉的那一份」，\
                 那个动作本身还在。\
                 上限 {MAX_PINS} 项（只数你自己钉的那些）——\
                 满了再钉会被明确拒绝（`{limit}`），不会替你淘汰任何一项：\
                 先 `{CAPABILITY_UNPIN}` 摘掉一个你想腾出来的，再钉。",
                limit = crate::pinned_actions::PinFailure::LimitReached.as_str()
            )),
            Capability::new(
                ActionId::new(CAPABILITY_UNPIN),
                "把一个动作从你自己的格位清单里摘掉（摘的是你钉的那一份，动作本身还在）",
                vec![ParamSpec::new(
                    PARAM_ACTION,
                    true,
                    "要摘的动作名（就是工具名）",
                )],
            )
            .exempt_from_inspection()
            .with_details(format!(
                "只摘掉「你钉过的那一份」：那个动作本身不因此消失——\
                 它能不能调，由它所属的那门开着没有决定，与你的格位无关。\
                 摘一个本来就没钉的名字是明确的成功结果\
                 （不报错）。与 `{CAPABILITY_PIN}` 一样，下一次开场生效。"
            )),
            Capability::new(
                ActionId::new(CAPABILITY_SELF_CONCEPT_WRITE),
                "写下 / 改写你的自识：从你自己的经历里提炼出你觉得自己是个什么样的。还没提炼出来，就还没有——它不欠着",
                vec![ParamSpec::new(PARAM_TEXT, true, "你的自识正文")],
            )
            .exempt_from_inspection()
            .with_details(
                "写下之后，下一次开场它就常驻在你视野的开头（当下的这一段不会变）。\
                 它与「标上 `pinned` 的笔记」和「钉住的动作格位」是三件事：那两样是具体的东西（一条笔记、一个格位），\
                 这一样是关于你自己的一篇——你什么时候想写就写，没写过就没有。\
                 空正文会被拒绝——「没有自识」应当根本没有这一份，而不是留一个空洞。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_HANDOFF_WRITE),
                "写下 / 改写你的交接：给下一次开场的自己留下「这一段干到哪、下一段接着做什么」。它只在下一次开场摆一次",
                vec![ParamSpec::new(PARAM_TEXT, true, "你的交接正文")],
            )
            .exempt_from_inspection()
            .with_details(format!(
                "写下之后，下一次开场它摆出一次（原文，前后不加东西），摆过就不再摆\
                 ——所以没改的话，它不会每一段都重播。它与「标上 `pinned` 的笔记」和\
                 「钉住的动作格位」是三件事：那两样每一段开场都在，这一样只交付一次。\
                 没写过就没有（开场一个字都不提）；空正文会被拒绝——「没有交接」应当根本没有那一份，\
                 而不是留一个空洞。\
                 上限 {max} 字节：写不下会被明确拒绝（`{too_large}`），旧的那一份原样不动\
                 （不截断、也不落半个文件）。",
                max = crate::handoff::MAX_HANDOFF_BYTES,
                too_large = crate::handoff::HANDOFF_TOO_LARGE,
            )),
        ])
    }

    /// 已展开 App 的能力：按装配顺序取它们**自己声明**的行动标识。
    ///
    /// "已展开"按 [`RunScope::is_open`] 判：预装常开的 App 的能力**从第一轮起**就在这里。
    fn expanded_capabilities(&self) -> Capabilities {
        let mut items = Vec::new();
        for app in &self.os.apps {
            if self.is_open(app.app_id()) {
                items.extend(app.declared_capabilities().items().iter().cloned());
            }
        }
        Capabilities::of(items)
    }

    /// 当前**全部可用**的能力：入口 + 已展开 App 的能力。
    ///
    /// 本版策略：在**同一次活动内**展开只增不减（入口能力不会被顶掉）。这只是本版的展开策略，
    /// 不是 Core 对将来任何能力提供者的要求，也不代表授权永不撤销——历史保留不等于永久授权。
    fn available_capabilities(&self) -> Capabilities {
        let mut items = self.entry_capabilities().items().to_vec();
        items.extend(self.expanded_capabilities().items().iter().cloned());
        Capabilities::of(items)
    }

    /// 某个动作此刻的**详情卡片**（与 `inspect_action` 交回的那一张是**同一份渲染**）。
    ///
    /// 找不到 ⇒ `None`：那说明这个标识不在**当前在场的能力表**里，本件不猜、也不编一张卡
    /// （准入面据此 fail-closed，见 `admit`）。
    fn card_for(&self, id: &str) -> Option<Vec<ContentBlock>> {
        self.available_capabilities()
            .items()
            .iter()
            .find(|each| each.id().as_str() == id)
            .map(|capability| {
                vec![ContentBlock::new(
                    id,
                    capability_card(capability, self.os.sources().class_if_assembled(id)),
                )]
            })
    }

    /// 参数模式检查：只允许列出的名字，而且每一项都必须到齐。
    ///
    /// 不符时交回的是**具体差异**（多了哪些名字、缺了哪些名字），不是光秃秃一个分类
    /// （TASK_72）：分类照旧只有 `invalid-parameters` 一个词、逐字节稳定，差异是它**旁边**的
    /// 结构化读数——主体当场就知道自己填错在哪一项，不必再去翻那张卡。
    fn require_params(params: &ActionParams, allowed: &[&str]) -> Result<(), ParamShape> {
        let mut shape = ParamShape {
            missing: Vec::new(),
            unexpected: Vec::new(),
            // OS 自有动作**不进这一格**：它们的既有回执里没有这一行，填了就把形状改了。
            // "接受哪些名字"只补在 App 拒绝的那一侧（见 `explain_shape_refusal`）。
            accepted: Vec::new(),
        };
        for (name, _) in params.entries() {
            if !allowed.contains(&name.as_str()) {
                shape.unexpected.push(name.clone());
            }
        }
        for name in allowed {
            if params.get(name).is_none() {
                shape.missing.push((*name).to_string());
            }
        }
        if shape.missing.is_empty() && shape.unexpected.is_empty() {
            Ok(())
        } else {
            Err(shape)
        }
    }

    /// 展开某个已装配 App 的能力。
    ///
    /// 只展开：不列目录、不读文件、不替模型选择下一步——路由在这里就返回了，**根本不会走到**
    /// 那个 App 的业务动作。重复展开不重读、不复制动词表；回一张**门卡**（已开、动作数、
    /// 目录位置），主体要动词就去看目录那一格。
    ///
    /// **出厂内置件的展开是纯记账**（它们的能力早在装配期就在索引里）；**外部来源的 App**
    /// （装在这一层的 MCP server）在这一步才真的被请出来：`present()` 起通道、拉清单，
    /// 交出来的那一刻能力才进归属索引与来源表。清单没拿到就**不算展开过**——回到
    /// 未展开的样子，主体下一次再试。
    fn expand(&mut self, action: &Action) -> ActionOutcome {
        if let Err(shape) = Self::require_params(action.params(), &[PARAM_TARGET]) {
            return shape.outcome();
        }
        let target = action.params().get(PARAM_TARGET).unwrap_or_default();
        let Some(index) = self.os.positions.get(target).copied() else {
            return ActionOutcome::failure(ExpansionFailure::UnknownTarget.as_str());
        };

        if self.is_open(target) {
            // 重复展开（含**预装常开**的 App）：回一张**门卡**——已开、动作数、目录位置。
            //
            // ⚠ **不重注入动词表**（2026-10-14 裁决）：动作数取自**装配期声明表**
            // （`declared_capabilities`），不读 App 存储、不 `present()`、不 `absorb()`、
            // 不改展开状态——这一条只是把已经在场的事实念一遍，不是再开一次。
            // 主体要动词就去看目录那一格（那里逐条列着），回执不再抄一遍。
            let count = self.os.apps[index].declared_capabilities().items().len();
            let door = if count == 0 {
                format!("{target} 本次 Run 已经开着；它没有暴露任何动作。没有重读。")
            } else {
                format!(
                    "{target} 本次 Run 已经开着；共 {count} 个动作，\
                     列在动作目录的【{target}】下。没有重读。"
                )
            };
            return ActionOutcome::success(vec![ContentBlock::new("already-expanded", door)]);
        }

        // **展开那一下的真动作**：请它把能力交出来。出厂内置件的默认实现什么都不做
        // （它们的清单早就在索引里）；外部来源的 App 在这里起通道、拉清单。
        //
        // 失败分两半交出去（与 `ParamShape` 同一条纪律）：**分类是词表**（逐字节稳定，
        // 落进盘上的行动结果），**对面那句话是内容**（随单贴在旁边）。
        // ⚠ 对面没留话 ⇒ **逐字节维持今天的行为**（只报分类，不产生空块）。
        //
        // ⚠ **不新造载体**：Core 早就有这一格（`failure_with_content`，文档原文「分类是词表，
        // 读数是内容」），`ParamShape` 也早在用——这里只是把**同一条解法**补到展开这条路径上。
        // ⚠ **App 交上来的 class 不进 [`ExpansionFailure`]**：那张表仍是 OS 自己那五个词，
        // 一个取值不加；它照原样进回执与行动结果，OS 不解释、也不解码它。
        if let Err(refusal) = self.os.apps[index].present() {
            // `class()` 是 `&'static str`（`ExpansionRefusal` 只收编译期常量）⇒ 不必再分配一份
            // owned 文本：写路径那两个构造器要的正是这个类型。
            return match refusal.detail() {
                Some(detail) => ActionOutcome::failure_with_content(
                    refusal.class(),
                    vec![ContentBlock::new(EXPANSION_DIAGNOSTIC_LABEL, detail)],
                ),
                None => ActionOutcome::failure(refusal.class()),
            };
        }
        // 交出来的清单**整批**并进两张索引（归属 + 效果声明），撞名就整台不上场、
        // 而且一格都不落。没并进去的能力既不能路由、也不该出现在目录里。
        if let Err(failure) = self.os.absorb(index) {
            return ActionOutcome::failure(failure.as_str());
        }
        self.expansion.expanded.insert(target.to_string());

        // 第一次展开：把**这一次新开的这一个 App** 的能力说明作为这次行动的结果留痕
        // （机械渲染，不做摘要）。
        //
        // ⚠ **2026-10-14 收窄**：从前这里给的是 `expanded_capabilities()`——**这一段里所有已开
        // app** ⇒ 开第二个 app 会把第一个的动词**再印一遍**。那既不是**新事实**（主体早看过那份
        // 回执），也不是**"看一眼"**（主体没要）——是纯冗余。判据就是那一下的事实：
        // **"这个 app 刚被打开"** ⇒ 回执只该讲它。
        //
        // 为什么"给全量"这件事本身是对的（别顺手把它也收了）：那一次的钱买的是"**不必为每个
        // 动词各付一个回合**"——实测真账上一个 app 声明约 6 条、一段里用 2~3 条 ⇒ 用几条参数表
        // 换掉几个回合，划算（而目录行是**每一轮**的钱，所以参数名不能搬到那里去）。
        let newly = Capabilities::of(
            self.os
                .apps
                .iter()
                .filter(|app| app.app_id() == target)
                .flat_map(|app| app.declared_capabilities().items().to_vec())
                .collect(),
        );
        let lines = capability_lines(&newly);
        let lines = if lines.trim().is_empty() {
            // 外部来源的 App 可以一条动词都没有：展开成功、但没有新东西可列——这不是故障，
            // 也不能让回执变成空块（空正文在注入批里等于没发生）。
            format!("{target} 这次展开没有新增任何动词：它没有暴露工具。")
        } else {
            lines
        };
        ActionOutcome::success(vec![ContentBlock::new("expanded", lines)])
    }
}

impl ActionPort for RunScope<'_> {
    fn capabilities(&self, agent: &AgentId, run: &RunId) -> Capabilities {
        if !self.matches(agent, run) {
            // 身份不匹配：不泄露任何能力。
            return Capabilities::none();
        }
        self.available_capabilities()
    }

    /// 准入：在 Core 准备这次行动**之前**判断"要不要发出去"。
    ///
    /// 它是能力的**第二道**（第一道是装配期：没有授权时那件能力根本不在目录里，也就没有主人），
    /// 也是**权威的一道**：目录只是说明，不是安全门。
    ///
    /// 拒绝只给稳定分类，不产生任何效果：不写文件、不创建操作身份、不生成行动事实。
    ///
    /// 顺序是固定的：身份 → 展开动词（OS 自有，免写效应门）→ 归属索引 → 是否已展开 →
    /// 是否查过格式 → `app.admit`。
    ///
    /// ⚠ **2026-09-26 改**：这一串里原来还有一道**未决效果门**（"写类别在未决解除之前一律被拒"），
    /// 排在最后、排在交 App 之前。它与跨进程恢复一起退役了——门是指令，而世界只该给信息；
    /// 未决的写由"开场那块纸条 ＋ 账本 ＋ 它自己去核"接住。**它的读表件（效果声明表）同刀退役**：
    /// 门一没，那张表就没有读者了。
    ///
    /// **2026-10-13 补：只读动作免"先看形状"的税，拒绝改走指路。** 三件只在这里说清：
    ///
    /// - **这条规则必须写在不会消失的地方**：它从前只印在 `pinned_actions` 的**零态**块里
    ///   ——主体一旦钉了 ≥1 项，那句提示就不再渲染。真机现场：主体正是靠那 125 次经验自己
    ///   总结出"回 `needs-inspection` 就先 `inspect_action`、再重试"，写进自己的笔记
    ///   （`note-12` 那一格的"固定处置"）。⇒ 那句话今天在 `INVOKE_DESCRIPTION`（`tools`
    ///   数组：每一轮、每个 Run 都在），并跟着改窄成"只挡会改东西的动作"。
    /// - **自持物写也是写**：写自己家里的一份文件照样改得动外面的世界 ⇒ 照样过这一关。
    ///   `timer` 与 `notes` 今天**同待遇**（2026-10-13 补齐声明：借既有的 `repeatable-write`
    ///   类别 ＋ 各配一个同参复核；历史与理由见 `timer_app.rs` 的 `declared_effects`）。
    /// - ⚠ **免查那一格不是准入判据**：[`Capability::is_exempt_from_inspection`] 是**呈现面**
    ///   的声明（目录行自带参数名——"免查必须同时免学"）。拿它当权限，就是在这一道之外造出
    ///   **第二份准入判据**：同一个问题两个答案。
    fn admit(&self, agent: &AgentId, run: &RunId, action: &Action) -> ActionAdmission {
        if !self.matches(agent, run) {
            return ActionAdmission::refused(ExpansionFailure::IdentityMismatch.as_str());
        }
        let id = action.id().as_str();
        // 展开、看详情、钉/摘都是 OS 自己的动词（不是跨边界写效应），免写效应门。
        // ⚠️ 这个 early-return 不只是"免门"：它同时是**准入面的认领**——不在这里放行，
        // 下面那句 `owners.get(id)` 会把这些动作当成 `UnknownAction` 拒掉。
        if OS_OWNED_ACTIONS.contains(&id) {
            return ActionAdmission::Admitted;
        }
        // 能力目录与分派用同一张归属索引：没有主人的标识在这里就被拒。
        let Some(index) = self.os.owners.get(id).copied() else {
            return ActionAdmission::refused(ExpansionFailure::UnknownAction.as_str());
        };
        if !self.is_open(self.os.apps[index].app_id()) {
            return ActionAdmission::refused(ExpansionFailure::NotExpanded.as_str());
        }
        // TASK_69（裁决 5）：**没查过格式、又没被钉住**的动作，先拒绝。
        //
        // 位置是挑过的：在"未知动作"与"未展开"**之后**（那两条说的是"这个名字根本不在场"，
        // 比"你还没看它的格式"更基础）。
        //
        // 拒绝的语义是**先看格式**（不是"你没这个能力"）：看一次 `inspect_action` 就放行，
        // 或者把它钉住（那本来就是这个动作的用途）。
        //
        // **TASK_72：拒绝随单附卡**——拒绝正好发生在系统最知道主体缺哪份事实的时刻，因此那份事实
        // 就随这次答复一起给，不必让主体再花一轮去取。附的是 `inspect_action` 交回的**同一张卡**
        // （同一个 `capability_card`，逐字节相同），而且**卡进了主体的视野就算查过**：
        // `inspected` 是"卡已进视野"的唯一台账，随单与查卡两个入口都写它——不写就是死循环，
        // 下一次同载荷还会被同一扇门拦住。
        if !self.may_call(id) {
            // **只读的动作不收"先看形状"的税**（2026-10-13）：它一个字节都改不了外面的世界，
            // 那张卡的唯一作用就是让主体"先看一眼"——而看一眼的代价是**一轮模型调用**。
            // 真机读数（小欣 22 段 Run / 414 次模型调用）：125 次首调税（≈30%）里
            // **83 次（66%）落在只读动作上**
            // （`im_contacts` 19 / `notes_list` 16 / `notes_read` 13 / `files_list` 9 / `files_read` 8 /
            // `im_blocks` 7 …）——**读一个字节都改不了的东西，收了一轮模型调用**。
            // ⇒ 放行。**形状不随回执附**：它按需（`inspect_action`）、按钉住（格位那一行），
            // 或者在参数名出错时由 `explain_shape_refusal` 精确点名（见 `execute`）。
            //
            // **写动作照旧先给卡**：那是真安全属性（先看形状再动手），一个字不动。
            if !self.os.effect_of(id).is_write() {
                return ActionAdmission::Admitted;
            }
            return match self.card_for(id) {
                Some(card) => {
                    self.expansion.note_inspected(id);
                    ActionAdmission::refused_with(NEEDS_INSPECTION, card)
                }
                // **不可达**（能走到这里说明它的 App 已在场，那张卡一定在场）；真到了这里就
                // fail-closed：**不附卡、也不记 inspected**——没有卡就没有"已进视野"这回事，
                // 绝不用"放行"来假装那份事实给过了。
                None => ActionAdmission::refused(NEEDS_INSPECTION),
            };
        }
        self.os.apps[index].admit(agent, run, action)
    }

    fn execute(
        &mut self,
        agent: &AgentId,
        operation: &OperationId,
        action: &Action,
    ) -> ActionOutcome {
        // 身份绑定：Agent 与 OperationId 里的 Run 都必须匹配，否则 fail-closed（不触碰任何 App）。
        if agent != self.expansion.agent() || operation.run() != self.expansion.run() {
            return ActionOutcome::failure(ExpansionFailure::IdentityMismatch.as_str());
        }

        let id = action.id().as_str();

        // 展开动作是本作用域自有的：同一活动内可以再次调用（重复展开是明确的成功结果）。
        if id == CAPABILITY_EXPAND {
            return self.expand(action);
        }

        // 看详情同样是本作用域自有的：**只读**，不看也不碰任何 App（它读的是能力表）。
        if id == CAPABILITY_INSPECT {
            return self.inspect(action);
        }

        // 钉住 / 摘掉一个动作的详情（TASK_68 P3）：改的是**自己的格位**（自持物）。
        //
        // ⚠️ 与上面两个不同：它**要写盘** ⇒ 需要这一次装配声明的环境根；
        // 没声明（测试构造的那些装配）时**明确拒绝**，不静默写到别处。
        if id == CAPABILITY_PIN {
            return self.pin(action, true);
        }
        if id == CAPABILITY_UNPIN {
            return self.pin(action, false);
        }

        // 写下 / 改写主体的**自识**（TASK_70）：写权归自己，改的是**自己的地盘**。
        // 与上面两个同款：要写盘 ⇒ 需要这一次装配声明的环境根；没声明就 fail-closed。
        if id == CAPABILITY_SELF_CONCEPT_WRITE {
            return self.write_self_concept(action);
        }

        // 写下 / 改写主体的**交接**：与自识同族（写权归自己、要写盘 ⇒ 需要环境根），
        // 但它的读者是**下一次开场**的自己——摆过一次就不再摆。
        if id == CAPABILITY_HANDOFF_WRITE {
            return self.write_handoff(action);
        }

        // 能力目录与分派用的是**同一张归属索引**：行动标识只有一个拥有者。
        let Some(index) = self.os.owners.get(id).copied() else {
            // 其余一律未知动作（包括已经移除的旧动作，不做隐藏兼容入口）。
            return ActionOutcome::failure(ExpansionFailure::UnknownAction.as_str());
        };

        // 未展开的 App：明确拒绝，而且**不触碰那个 App**。（预装常开的 App 永远在场。）
        if !self.is_open(self.os.apps[index].app_id()) {
            return ActionOutcome::failure(ExpansionFailure::NotExpanded.as_str());
        }

        // ⚠ **2026-09-26 改**：这里原来在结果回来后还调一次 `self.pending.resolve(...)`——那是未决
        // 效果门"同一次驱动内复核之后可以再写"的那一半。门退役了，集合也删了，因此这一行没有了；
        // 动作结果照旧如实交回，一个字节没变。
        //
        // ⚠ **2026-10-13（只读免收税那一刀）**：只读动作在准入面被放行之后，**形状不随回执附**
        // ——它按需（`inspect_action`）、按钉住（格位那一行），或者**在参数名出错时**由
        // [`RunScope::explain_shape_refusal`] 精确点名。两个理由：
        // ① 与那条读数**重复**（那边给的是"哪个名字不对、对的是什么"，比整张卡更小更准）；
        // ② **失败回执不许夹带内容**是既有纪律（"not-found 不夹带正文"一类），
        //    附一张卡就把"结果"与"形状"两条通道混在一起了。
        let outcome = self.os.apps[index].execute(agent, operation, action);
        self.explain_shape_refusal(id, action, outcome)
    }
}

impl RunScope<'_> {
    /// 看一个动作的**详情卡片**（骨架 + 详情 + 参数表）。
    ///
    /// 只读：不改任何东西，也不触碰任何 App——它读的是能力表本身。
    fn inspect(&mut self, action: &Action) -> ActionOutcome {
        if let Err(shape) = Self::require_params(action.params(), &[PARAM_ACTION]) {
            return shape.outcome();
        }
        let name = action.params().get(PARAM_ACTION).unwrap_or_default();
        let available = self.available_capabilities();
        let Some(capability) = available
            .items()
            .iter()
            .find(|each| each.id().as_str() == name)
        else {
            // fail-closed：不在**当前在场能力表**里的名字一律拒绝（不猜近似名、不透传别家的动作）。
            //
            // 分类用 `invalid-argument`（**参数值**不在表里）而不是 `invalid-parameters`
            // （参数**形状**不对）——上面那条 `require_params` 才是后者。两个名字各管一件事：
            // 一个是"你给的名字我不认识"，一个是"你给的东西根本不像参数"。
            //
            // ⚠ **2026-10-14 补读数**（真机现场抓到的）：分类**一个字不改**（上面那句是对的），
            // 缺的是「**一次拒绝要指路**」那一半——OS 手里有两件机械事实：这个名字属于哪个 app、
            // 那个 app 开没开。真机现场：主体在展开之前 `inspect_action(files_list)`，回执
            // **一个字都没有**，读起来像"你参数写错了"；主体后来自己改用 `open_app` 绕过去了，
            // **但那是运气**（同一个模型换一天可能就卡在那儿反复试同一个名字）。
            // 读数只搬 OS 手里的事实，不替主体判断"该不该开那扇门"。
            let mut door: Option<String> = None;
            for app in &self.os.apps {
                let id = app.app_id();
                if self.is_open(id) {
                    continue;
                }
                let declares = app
                    .declared_capabilities()
                    .items()
                    .iter()
                    .any(|each| each.id().as_str() == name);
                // 外部来源（MCP）在展开之前**拉不到清单**（"真到展开那一下才起进程、拉清单"），
                // 因此还要认它的前缀规则：动词叫 `<名>_<工具名>`（见 `agent_run --mcp`）。
                let prefixed = name.starts_with(&format!("{id}_"));
                if declares || prefixed {
                    door = Some(id.to_string());
                    break;
                }
            }
            let reading = match door {
                Some(app) => format!(
                    "{name} 不在当前在场的名字里：它属于 {app}，而那个 app 还没展开 ⇒ \
                     先 open_app(target=\"{app}\")，然后再调它。"
                ),
                None => format!("{name} 不在当前在场的名字里，也没有哪个已装配的 app 声明过它。"),
            };
            return ActionOutcome::failure_with_content(
                crate::core::INVALID_ARGUMENT,
                vec![ContentBlock::new(DOOR_LABEL, reading)],
            );
        };
        // 卡片交回去了就算"进过主体的视野"（TASK_69；TASK_72 起这**不是**唯一入口——准入面随单附卡
        // 也走同一个写口）：这是免查判据的**唯一**来源——主体看过格式之后，这个名字从此可以直接调；
        // 在那之前要先看（见 `admit`）。
        self.expansion.note_inspected(name);
        ActionOutcome::success(vec![ContentBlock::new(
            name,
            capability_card(capability, self.os.sources().class_if_assembled(name)),
        )])
    }

    /// 这一刻主体**能不能调**这个名字（裁决 5 的"在场集合"）。
    ///
    /// 两类免查：**卡已经进过主体的视野的**（主体主动查的，或拒绝时随单附给主体的），以及**被钉住的**
    /// （开场那份视野）。其余一律要在准入面被拒——拒绝的语义是"先看格式"，
    /// 不是"你没有这个能力"。
    fn may_call(&self, id: &str) -> bool {
        if self.expansion.is_inspected(id) {
            return true;
        }
        self.expansion
            .pinned
            .get_or_init(|| self.current_pins())
            .contains(id)
    }

    /// **一次拒绝要指路**（2026-10-13）：把"关于这次调用的形状事实"补在失败回执旁边。
    ///
    /// ⚠ **它不认任何分类词**——`invalid-argument`（Core 的词）与 `invalid-parameter`（notes 自己的
    /// 词）在真机上指的是同一件事，而 OS **不该认识任何 App 的业务词**（词表不跨层）。
    /// 判据因此是"名字层面**确有**机械事实可说"，而那件事与失败**为什么**发生无关：
    /// 这里给的两件（哪些名字不在表里、哪些必填缺席）都是关于**这次调用**的事实。
    /// 名字对不对是 OS 手里就有的事实——能力表由它渲染，卡也由它渲染。
    ///
    /// ⚠ 两件都没有 ⇒ **什么都不加**（回执逐字节等于从前）；**成功 ⇒ 也不加**
    /// （回执就是回执，不夹带形状 —— 与"失败不得夹带内容"同一条纪律的另一面）。
    /// ⚠ 分类词**逐字节不变**（还是 App 给的那一个）：读数是它**旁边**的内容，两样不揉在一起。
    fn explain_shape_refusal(
        &self,
        id: &str,
        action: &Action,
        outcome: ActionOutcome,
    ) -> ActionOutcome {
        if outcome.state() != ResultState::Failure {
            return outcome;
        }
        if outcome.failure_class().is_none() {
            // 失败却没有分类：不可达（`ActionOutcome::failure*` 都要一个分类），
            // 真到了这里**也不编一个**——原样交回。
            return outcome;
        }
        let available = self.available_capabilities();
        let Some(capability) = available.find(&ActionId::new(id.to_string())) else {
            return outcome;
        };
        let specs = capability.params();
        let mut unexpected: Vec<String> = Vec::new();
        for (name, _) in action.params().entries() {
            if !specs.iter().any(|spec| spec.name() == name.as_str()) {
                unexpected.push(name.clone());
            }
        }
        let mut missing: Vec<String> = Vec::new();
        for spec in specs {
            if spec.is_required() && action.params().get(spec.name()).is_none() {
                missing.push(spec.name().to_string());
            }
        }
        if missing.is_empty() && unexpected.is_empty() {
            return outcome;
        }
        // 接受名单按**声明顺序**枚举，必填带标记——与卡里那一行的写法同款。
        let accepted: Vec<String> = specs
            .iter()
            .map(|spec| {
                if spec.is_required() {
                    format!("{} (required)", spec.name())
                } else {
                    spec.name().to_string()
                }
            })
            .collect();
        let shape = ParamShape {
            missing,
            unexpected,
            accepted,
        };
        // **附在 App 自己带回的正文后面，不替换它**：失败也可能带正文（`terminal_run` 那一类
        // "失败也带正文"的动词），丢掉它就是这一层替它删话。
        let mut blocks = outcome.content().to_vec();
        blocks.push(ContentBlock::new(PARAM_SHAPE_LABEL, shape.reading()));
        // **分类原样沿用**，只在其旁边补一块读数：不重新产生分类（它可能来自写路径的静态常量，
        // 也可能来自读路径的 owned 文本），因此既不分配、也不经过 `failure_with_content`。
        outcome.replacing_content(blocks)
    }

    /// 此刻生效的 pin 集：**就是主体自己钉的那一份**（2026-10-10 起常驻只剩这一个来源），
    /// **只保留当前在场**的。
    ///
    /// 只在 Run 边界读一次盘（由 `may_call` 缓存进 `RunExpansion`）：这是"**pin 只在 Run 边界
    /// 生效**"的实现处。没声明环境根的装配读到空集（自持物读不到——那是"这次装配没有根"，
    /// 不是"主体没 pin 过"；两者后果一样）。**只保留在场的**：那个 App 没装配、或还没展开的动作，
    /// 钉了也不该免查（凭空造不出动作）。
    fn current_pins(&self) -> BTreeSet<String> {
        let mut pins: BTreeSet<String> = BTreeSet::new();
        if let Some(root) = self.os.environment_root()
            && let Ok(mine) = crate::pinned_actions::pinned_actions(root)
        {
            pins.extend(mine);
        }
        let available = self.available_capabilities();
        pins.retain(|id| available.find(&ActionId::new(id.clone())).is_some());
        pins
    }

    /// 钉住 / 摘掉一个动作的详情（TASK_68 P3）：改的是**主体自己的格位**（自持物）。
    ///
    /// **本函数只做路由判定**（2026-10-08 收口）——三件都是"能不能调"：
    /// - **参数形状**：`require_params`；
    /// - **只钉在场的动作**：凭空钉一个不存在（或那个 App 没装配）的名字没有意义；
    /// - **有没有环境根**：这一次装配没声明根（测试构造的舞台）⇒ **明确拒绝**，不静默写到别处。
    ///
    /// ⚠ **域规则与回执措辞不在这里**：上限、读不到就拒、写失败如实报、那一块回执长什么样，
    /// 全归 [`crate::pinned_actions::set_pinned`]——那一份事实的**唯一写者**。
    /// 判据一句话：**路由层只许拥有"能不能调"，不许拥有"格位是什么"。**
    fn pin(&self, action: &Action, pinned: bool) -> ActionOutcome {
        if let Err(shape) = Self::require_params(action.params(), &[PARAM_ACTION]) {
            return shape.outcome();
        }
        let name = action.params().get(PARAM_ACTION).unwrap_or_default();
        // 与 `inspect` 同款：不在**当前在场能力表**里的名字一律拒绝（不猜近似名）。
        if !self
            .available_capabilities()
            .items()
            .iter()
            .any(|each| each.id().as_str() == name)
        {
            return ActionOutcome::failure(crate::core::INVALID_ARGUMENT);
        }
        let Some(root) = self.os.environment_root() else {
            return ActionOutcome::failure("no-environment-root");
        };
        match crate::pinned_actions::set_pinned(root, name, pinned) {
            Ok(block) => ActionOutcome::success(vec![block]),
            Err(failure) => ActionOutcome::failure(failure.as_str()),
        }
    }

    /// 写下 / 改写主体的**自识**（TASK_70）：改的是**主体自己的地盘**。
    ///
    /// **本函数只做路由判定**（2026-10-08 收口）——两件都是"能不能调"：
    /// - **参数形状**：`require_params`；**空正文明确拒绝**（`invalid-argument`，与"缺参数 /
    ///   参数形状不对"同族——自识不会被清成空白："没有自识"应当是根本没有那一份）；
    /// - **有没有环境根**：没声明 ⇒ **fail-closed**，不写也不假装写了。
    ///
    /// ⚠ **域规则与回执措辞不在这里**：超限拒绝、不写不截断、那一块回执与那三段读数长什么样，
    /// 全归 [`crate::self_concept::write_from_subject`]——那一份事实的**唯一写者**。
    fn write_self_concept(&self, action: &Action) -> ActionOutcome {
        if let Err(shape) = Self::require_params(action.params(), &[PARAM_TEXT]) {
            return shape.outcome();
        }
        let text = action.params().get(PARAM_TEXT).unwrap_or_default();
        if text.trim().is_empty() {
            return ActionOutcome::failure(crate::core::INVALID_ARGUMENT);
        }
        let Some(root) = self.os.environment_root() else {
            return ActionOutcome::failure("no-environment-root");
        };
        match crate::self_concept::write_from_subject(root, text) {
            Ok(block) => ActionOutcome::success(vec![block]),
            Err(failure) => match failure.reading() {
                Some(reading) => {
                    ActionOutcome::failure_with_content(failure.as_str(), vec![reading])
                }
                None => ActionOutcome::failure(failure.as_str()),
            },
        }
    }

    /// 写下 / 改写主体的**交接**：改的是**主体自己的自持物**，读者是**下一次开场**的自己。
    ///
    /// **本函数只做路由判定**（与 `write_self_concept` 同款、同一条纪律）——两件都是"能不能调"：
    /// - **参数形状**：`require_params`；**空正文明确拒绝**（`invalid-argument`，与"缺参数 /
    ///   参数形状不对"同族——"没有交接"应当是根本没有那一份，而不是留一个空洞）；
    /// - **有没有环境根**：没声明 ⇒ **fail-closed**，不写也不假装写了。
    ///
    /// ⚠ **域规则与回执措辞不在这里**：超限拒绝、不写不截断、那一块回执与拒绝读数长什么样，
    /// 全归 [`crate::handoff::write_from_subject`]——那一份事实的**唯一写者**。
    fn write_handoff(&self, action: &Action) -> ActionOutcome {
        if let Err(shape) = Self::require_params(action.params(), &[PARAM_TEXT]) {
            return shape.outcome();
        }
        let text = action.params().get(PARAM_TEXT).unwrap_or_default();
        if text.trim().is_empty() {
            return ActionOutcome::failure(crate::core::INVALID_ARGUMENT);
        }
        let Some(root) = self.os.environment_root() else {
            return ActionOutcome::failure("no-environment-root");
        };
        match crate::handoff::write_from_subject(root, text) {
            Ok(block) => ActionOutcome::success(vec![block]),
            Err(failure) => match failure.reading() {
                Some(reading) => {
                    ActionOutcome::failure_with_content(failure.as_str(), vec![reading])
                }
                None => ActionOutcome::failure(failure.as_str()),
            },
        }
    }
}

/// 一张动作卡片的机械渲染：骨架 + 详情 + **回执来源** + 参数表
/// （→ 与主体看到的 `tools` 那一行正好是同一份声明）。
///
/// ⚠ **`receipt-source` 那一行是 2026-10-08 加的**（见 `agent/README.md` §3.45）：每个 App 都必须
/// 为它的每个能力回答"回执从哪个口子进来"（装配期漏填即拒，`CapabilityWithoutSource`），
/// 而那个答案此前**从不送给主体**——[`SourceClass`] 自己的文档写着「它不承载"可不可信"……
/// **判断归主体**」，于是**判断缺了它的输入**（与"写得进去、读不出来"同一个形状）。
/// 它落在**详情卡**里：按需取，不占首屏、不占每轮预算，而那一拍正是主体在决定要不要用它。
/// 位置固定——**详情之后、`params:` 之前**（卡片的第三格），也是这张卡里**唯一**渲染来源的地方。
///
/// - 渲染的是**稳定标记**（`world-unsigned` 这类），不是散文：那一格是机器可读的事实；
/// - **读不到时渲染 `unknown`，绝不省略那一行**：「这一格没有值」与「读不到这一格」必须分得开，
///   省略会让**主体**以为"这件事本来就没有这一格"。
///   ⚠ 如实登记：`unknown` 那一支**今天不可达**（装配期已保证每个在场能力都有来源声明，
///   OS 自有六个也由 `declare_os_owned` 登记）⇒ 它是 **fail-closed 的形状**，不是一条路径，
///   因此**没有故障用例**（无法注入）。
///
/// **负空间**：这一格只**渲染**已经收集好的那一份声明——不动装配期检查、不动任何 App 的声明、
/// 不动词表、不进首屏（门与能力目录里都没有这一行）、不加新动作、不碰 Core。
fn capability_card(capability: &Capability, source: Option<SourceClass>) -> String {
    let mut text = String::new();
    text.push_str(capability.id().as_str());
    text.push_str(": ");
    text.push_str(capability.skeleton());
    if capability.details().is_empty() {
        text.push_str("\n  （没有更多详情：骨架即全部）");
    } else {
        text.push_str("\n  ");
        text.push_str(capability.details());
    }
    text.push_str("\n  receipt-source: ");
    text.push_str(source.map(SourceClass::as_str).unwrap_or("unknown"));
    text.push_str("\n  params:");
    for spec in capability.params() {
        text.push_str("\n    ");
        text.push_str(spec.name());
        text.push_str(if spec.is_required() {
            " (required)"
        } else {
            " (optional)"
        });
        if !spec.description().is_empty() {
            text.push_str(": ");
            text.push_str(spec.description());
        }
    }
    text
}

/// 把能力说明渲染成一段机械文本：只搬运，不做摘要、不改写。
///
/// **两段都渲染**（骨架 + 详情）：这条回执是**一次性的尾部流量**（展开那一下进历史），
/// 因此不该比从前少给——`tools` 数组里省下的字，在这里照旧给主体。
fn capability_lines(capabilities: &Capabilities) -> String {
    let mut text = String::new();
    for capability in capabilities.items() {
        text.push_str(capability.id().as_str());
        text.push_str(": ");
        text.push_str(capability.skeleton());
        if !capability.details().is_empty() {
            text.push('\n');
            text.push_str("  ");
            text.push_str(capability.details());
        }
        for spec in capability.params() {
            text.push('\n');
            text.push_str("  ");
            text.push_str(spec.name());
            text.push_str(if spec.is_required() {
                " (required): "
            } else {
                " (optional): "
            });
            text.push_str(spec.description());
        }
        text.push('\n');
    }
    text
}
