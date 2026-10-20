//! App 面向 OS 的**窄接入契约**。
//!
//! 这个模块只定义"OS 骨干要从一个 App 知道什么"，因此它既不属于 OS 骨干的路由实现，
//! 也不属于任何具体 App：
//!
//! ```text
//! OS 骨干  → 本契约（发现、展开与路由只依赖契约，不依赖任何 App 的内部实现或存储实现）
//! App      → 本契约 + Core 的中立载体（不认识 OS 骨干）
//! Composition → 具体实现与配置（选择谁、怎么构造、怎么接线）
//! ```
//!
//! 契约刻意只保留少数几项描述 + 一个执行入口，而且执行入口直接复用 Core 的中立 [`ActionPort`]：
//! 不为"统一"再造一套 Action/Result 类型，也不把 App 的业务对象搬到这里。
//!
//! **状态归属**：展开状态不在这里，也不在任何 App 里——它是**一次活动**的资格事实，
//! 由 OS 的使用作用域拥有。因此一个 App 不需要（也无法）知道另一个 App 是否已经展开。
//!
//! **效果归属**：一个能力"有没有写效应、由谁来复核"是 **App 自己的事实**，所以它由 App 说出来
//! （[`AppPort::declared_effects`]），不在这里替它猜、也不在任何中央权限表里登记。声明**不是权限**：
//! 它不授予任何东西，只说明"系统能不能替主体机械判断这一格"。默认不声明 = **只读**——
//! 默认方向是"更少的能力"，因此不需要写声明也算合法，而"说了写却没配复核"必须被拒绝。
//!
//! **来源归属**：一个能力的回执"是谁说的"同样是 **App 自己的事实**（[`AppPort::declared_sources`]），
//! 而且它**没有安全的默认值**：默认成"自己的字"是对外来的话撒谎，默认成"世界的"是对自己的字
//! 撒谎。来源是**事实**，事实不能有兜底值——所以这一格**必须显式声明**，漏填在装配期失败
//! （见 [`AssemblyFailure::CapabilityWithoutSource`]）。
//!
//! [`SourceClass`] 只回答**"这段内容从哪个口子进来"**，它**不回答"该不该信"**：给它加权重、
//! 加排序、加过滤都是把机械事实变成认知代劳（宪法判据 9），一律越界。署名也不等于安全——
//! 一个有署名的恶意对端照样能说话。

use std::collections::{BTreeMap, BTreeSet};

use crate::core::{ActionId, ActionPort, Capabilities};

/// OS 对「一个已装配 App」的全部要求。
pub trait AppPort: ActionPort {
    /// 这个 App 的稳定标识：装配时用它，展开动作的目标也是它。
    fn app_id(&self) -> &str;

    /// 给主体看的一句话说明（机械描述它提供什么，不替主体判断重要性）。
    ///
    /// **它是门牌**（2026-10-08 主人确认）：首屏里每一扇门印的那一行 `【<标识>】<这一句>`，
    /// 展开回执里还会再印一次。它决定主体**想不想得到**去用它，因此它自己有一条判据
    /// （守卫：`tests/minimal_agent.rs::the_door_plates_are_one_mechanical_sentence`）：
    ///
    /// 1. **单行字面量**（不许 `\` 续行）——门牌是一行字，不是一段介绍；
    /// 2. **不带 `**`**——那是标记语法，主体读到的是两个星号（反引号允许：那是本仓标识符的写法）；
    /// 3. **不带比喻词**（封闭词表）与**内部文件名**——比喻要主体先猜，内部名是环境的里子；
    /// 4. **长度 ≤ [`DOOR_PLATE_MAX_BYTES`]**——上限不是为了某个数字好看，是让"越写越长"
    ///    从默认允许变成必须先删。
    fn description(&self) -> &str;

    /// 这个 App **拥有**的行动标识（与展开状态无关）。
    ///
    /// 装配时用它建立唯一归属索引，运行时用它给出能力目录；两处用的是同一份声明，
    /// 因此不存在"目录里出现的行动却路由不到，或路由得到却没在目录里"的情况。
    fn declared_capabilities(&self) -> Capabilities;

    /// 这个 App 对**它自己能力的写效应**说出来的声明（`行动 → 类别 + 复核能力`）。
    ///
    /// 默认值空表示"只读"：不声明是合法状态，不是缺口——默认方向必须指向**更少的能力**。
    /// 说出来的部分要配得齐（写类别必须配复核心力），否则装配期拒绝，见
    /// [`AssemblyFailure`] 的三条效果分类。
    ///
    /// **声明不是权限**：它不授予、不撤销、也不替代准入；它只是"系统这一格能不能机械判断"
    /// 的依据。因此伪造一份声明不会得到写能力，只会被装配期按归属拒绝。
    ///
    /// 返回**自有值**而不是借用切片：声明必须与实例当前的能力**同源派生**（例如"有授权才有这一
    /// 项创建能力"），要求实现者把派生结果缓存成字段会让同一个事实出现两份可变权威。装配期只在
    /// 装配时调用它一次，代价可以忽略。
    fn declared_effects(&self) -> Vec<EffectDeclaration> {
        Vec::new()
    }

    /// 这个 App 对**它自己每一个能力**说出来的来源声明（`行动 → 来源档`）。
    ///
    /// **没有默认实现，也不许漏项**：来源是一份事实，而事实没有安全的兜底值——默认成
    /// [`SourceClass::SelfAuthored`] 是对外来的话撒谎，默认成 [`SourceClass::WorldUnsigned`] 是对主体
    /// 自己的字撒谎。所以这一格由**类型**要求每个实现者回答（没有默认实现），再由装配期的
    /// [`AssemblyFailure::CapabilityWithoutSource`] 要求**每个能力都被回答到**。
    ///
    /// 与 [`AppPort::declared_effects`] 同源派生：它必须与实例当前的能力一致（例如"没有授权时
    /// 那一项创建能力根本不在清单里"，它的来源声明也不该在）。
    ///
    /// **它不承载"可不可信"**：只说明这段回执从哪个口子进来。判断归主体。
    fn declared_sources(&self) -> Vec<SourceDeclaration>;

    /// **展开那一下，请这个 App 把能力交出来**（懒展开的接缝）。
    ///
    /// 出厂内置的 App 能力在装配期就写死了，默认实现什么都不做——它们的清单早在归属索引里。
    /// 但**外部来源的 App**（装在这一层的 MCP server）在装配期只知道自己的名字：动词清单要
    /// 真的连上对面才拿得到。于是"展开"这件事对它们多了一步真动作，而 OS 的展开路径正好是
    /// 唯一的时机：早于它的每一次能力查询都只能看到空清单（那就是"装了不用零成本"）。
    ///
    /// 返回 `Ok(())` = **此刻** [`AppPort::declared_capabilities`] 已经可以枚举，OS 会把这份
    /// 清单并进归属索引与来源表（同一次展开、同一份权威，不存在"目录里有、路由里没有"）；
    /// 返回 `Err(`[`ExpansionRefusal`]`)` = 这次展开失败，分类进主体的回执，对面的原文（若有）
    /// 作为**随单内容**贴在它旁边。
    ///
    /// 它是**幂等**的：重复展开不重连、不重读；失败之后通道由 App 自己收掉（不留半死的连接）。
    ///
    /// ⚠ **为什么它是这个类别唯一不能省的接缝**：外部来源的 App 在装配期**报不出一份动词清单**
    /// （要先连上对面），而效果声明与来源声明都**与清单同源派生**（[`AppPort::declared_effects`]
    /// ／[`AppPort::declared_sources`]）——没有清单，那两张表里也就没有它们的份。**默认方向**在
    /// 这里帮不上忙：效果类别的缺项按契约读成**只读**（默认指向更少的能力），于是一台真会改
    /// 外面世界的 server 若被漏掉那一格，它的写会被读成**只读**（**fail-open**）。⇒ 清单只能在
    /// **展开那一下**由 App 自己交出来，本契约**不给它一个默认值**。
    ///
    /// ⚠ **"忘了实现它"长什么样**（今天的实际行为，如实登记）：默认实现返回 `Ok(())` ⇒ 展开
    /// **成功**但一条动词都没有（OS 侧那一步会照实说"这次展开没有新增任何动词"），
    /// 那些动词进不了任何一张表、也**路由不到**（`unknown-capability`，fail-closed）——
    /// 所以"漏实现"今天落在**零动词**这一格，**不是**"被读成只读"那一格。
    fn present(&mut self) -> Result<(), ExpansionRefusal> {
        Ok(())
    }
}

/// **展开失败**：我们的稳定分类 ＋（可选）**对面自己那句话**。
///
/// 形状与 `ActionOutcome::failure_with_content` 同一条纪律（Core 的文档原文：「`class` 照旧是
/// 稳定分类标记……`content` 是**旁边的结构化读数**……**分类是词表，读数是内容**」）：
///
/// - `class` **必须**是稳定标记——它进主体的回执、也可能落进盘上的行动结果，
///   因此**同一故障两次发生必须逐字节相同**。**动态文本一律不许揉进它**：
///   揉进去既污染封闭词表，又让"同一故障两次不同"。⚠ 这条"必须"由**类型**保证：
///   构造器只收 `&'static str`（与 [`crate::core::ActionOutcome::failure`] 同款）。
/// - `detail` 是**别人的话的落脚处**（如对面握手被拒时那句原文）。它**不是**分类，
///   也**不参与任何判断**——只让记录不必丢掉对面说过的话。
///
/// ⚠ **为什么需要这一格**（2026-10-13，判据不是"信任纪律"而是"记录诚实"）：
/// 外来的话若不加标明地进记录，它就看起来像我们自己的读数 ⇒ 记录在撒谎。
/// 同一条解法在"调用"那条路径上**早就落地了**（`CallReply::Refused { detail }` → 回执里
/// 印 `error-detail = …`），而展开这条路径只有一格（[`AppPort::present`] 的失败值从前是一个
/// `String`：分类装得下，对面那句话装不下）——那句话只能被丢掉。
///
/// 上游是 `outputs/IRREPLACEABLE_RECORD_RULING_DRAFT_2026-10-13.md` §5.2：那张表**缺的只有
/// 两格**，这是第 ① 格（第 ② 格是"参数**值**出错时的诊断"，另一件事，不在本类型的射程里）。
/// 拿那张表的三问（够得着 · 留得下 · 记诚实）量这一格：第 ③ 问是**别人的话标明了吗**——
/// 它在这里从"否"变成"是"；判据一个字没改，改的是这一格的答。
///
/// ⚠ **它不立第二张词表**：`class` 装的就是把它交上来的那个 App **自己那份**稳定词表里的
/// 一个标记（那份表的权威在它自己的 `as_str` / `from_str` 上）。本契约**不认识这个取值**——
/// 不解码、不校验、也不往任何一张表里加一格。形状正合"词表不跨层"那条：**归类 ＋ 携带原文**，
/// 两半各走各的路。
///
/// **负空间**：`detail` 是**别人的话**，因此①**不被解析**去推分类（分类只看结构字段），
/// ②**不被我们改写成一句自己的话**，③没有时**不替对面补一句理由**。三条是同一个判据的三面：
/// **记录不许替别人说话**。
///
/// **「没有」与「读不到」分开表达**：对面没给原文 ⇒ [`ExpansionRefusal::detail`] 是 `None`，
/// 渲染时**整格不出场**——不写空值、也不编一句话。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpansionRefusal {
    /// 稳定分类：**`&'static str`**（只收编译期常量，见 [`ExpansionRefusal::classified`]）。
    class: &'static str,
    detail: Option<String>,
}

impl ExpansionRefusal {
    /// 只有我们的分类：对面没留下别的话。
    ///
    /// ⚠ `class` 是 `&'static str`：它会被 OS 原样装进 [`crate::core::ActionOutcome::failure`]
    /// （写路径只收编译期常量），因此这里不能让一个 owned `String` 混进来。
    pub fn classified(class: &'static str) -> Self {
        Self {
            class,
            detail: None,
        }
    }

    /// 我们的分类 ＋ 对面自己那句话。
    ///
    /// ⚠ **空的／只有空白的"原文"等于没有原文**（在这里收口，不靠调用方自觉）：
    /// 那一格是**文本**，而"对面一个字都没说"与"我们没去读"必须分得开——
    /// 造一块空的诊断出来，就是让两件事看起来一样。
    pub fn with_detail(class: &'static str, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        Self {
            class,
            detail: (!detail.trim().is_empty()).then_some(detail),
        }
    }

    /// 我们的分类 ＋（可能没有的）对面那句话。
    ///
    /// 它是 [`ExpansionRefusal::classified`] 与 [`ExpansionRefusal::with_detail`] 的合流口：
    /// 调用方手上是一个 `Option<&str>` 时用它，**不必自己判空**（判空只有一处，见上）。
    pub fn from_detail(class: &'static str, detail: Option<&str>) -> Self {
        match detail {
            Some(detail) => Self::with_detail(class, detail),
            None => Self::classified(class),
        }
    }

    /// 稳定分类标记（进回执、可能落盘的那一半）。
    pub fn class(&self) -> &'static str {
        self.class
    }

    /// 对面自己那句话（没有就是没有）。
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

/// **门牌的长度上限**（2026-10-08 主人确认）：`description()` 那一句话的字节上限。
///
/// 上限存在的意义不是"这个数字好看"，是让**越写越长**从默认允许变成**必须先删**——
/// 与"新增一个动词必须删掉一个""处境链不许超过 11 层"是同一条纪律。
/// 判据见 [`AppPort::description`]；守卫在 `tests/minimal_agent.rs`。
pub const DOOR_PLATE_MAX_BYTES: usize = 384;

/// 一个能力对**外部世界**的效果类别。
///
/// 它是稳定分类字符串的来源（`read-only` / `exclusive-create` / `repeatable-write`），
/// 与 [`crate::core::ActionAdmission::Refused`] 的分类、`ActionOutcome` 的失败分类同一纪律：
/// **Core 只搬运、不解释、不分支**。类别属于能力自己的事实，不属于任何中央权限表。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffectClass {
    /// 只读：执行不改变外部世界（未声明时的默认类别）。
    ReadOnly,
    /// **独占创建**：同参数第二次必然失败——名字已被占用、旧文本已被替换，本次效果没有发生。
    ExclusiveCreate,
    /// **可重复写**：同参数第二次通常会**成功**，每次产生新的一份效果（身份不是参数决定的）。
    RepeatableWrite,
    /// **不可验证的写**（TASK_36）：它确实改变外部世界，但系统里**不存在**一条"同参数只读复核"
    /// 能回答"那次效果发生了没有"——例如执行一条命令（`terminal_run`）：再跑一次不是复核，
    /// 那是一次**新的执行**。
    ///
    /// 它与前两类的差别只有一处，而这一处让它必须自成一类：装配期要求"写类别必须配复核"，
    /// 而这一类的**定义**就是"没有复核可配"。所以它是显式的第四类，不是"忘配复核"：
    ///
    /// - 它在**声明**里仍然是**写**（[`EffectClass::is_write`] 为真）；
    /// - 它**不**要求复核（[`EffectClass::requires_verification`] 为假）；
    /// - ⚠ **2026-09-26 改**：这一条原来还写着"有未决时未决效果门照旧拦它，而代价是未决一旦挂上
    ///   就没有机械出路"。**门已退役**：现在"那次写到底发生了没有"由主体自己去看、自己决定——
    ///   它手里有账本（`trace.ndjson`）与 `terminal_run` 那条整机全权的路。这一类的定义
    ///   （"没有同参复核可配"）一个字没变。
    UnverifiableWrite,
}

impl EffectClass {
    /// 稳定分类标记。
    pub fn as_str(self) -> &'static str {
        match self {
            EffectClass::ReadOnly => "read-only",
            EffectClass::ExclusiveCreate => "exclusive-create",
            EffectClass::RepeatableWrite => "repeatable-write",
            EffectClass::UnverifiableWrite => "unverifiable-write",
        }
    }

    /// 是不是写效应。
    ///
    /// 它是类别自己的读数（"这个动作会不会改外面的世界"）。⚠ **2026-09-26 改**：收集它的那张
    /// **效果声明表**随未决效果门退役了（门是那张表唯一的生产读者），因此这一格现在只被声明本身、
    /// 源码守卫与测试读。装配期自洽性检查用的判据是 [`EffectClass::requires_verification`]。
    pub fn is_write(self) -> bool {
        !matches!(self, EffectClass::ReadOnly)
    }

    /// 这一类的写声明**要不要**配复核能力（装配期按它决定报不报 `WriteWithoutVerification`）。
    ///
    /// 两个"有路可复核"的写类别返回真；[`EffectClass::UnverifiableWrite`] 返回假——它自成一类的
    /// 全部理由就是"没有同参复核可配"。
    pub fn requires_verification(self) -> bool {
        matches!(
            self,
            EffectClass::ExclusiveCreate | EffectClass::RepeatableWrite
        )
    }
}

/// 一个 App 对**它拥有的一个能力**的效果声明。
///
/// 复核能力是**同参数的只读动作**（"现在的真实状态符不符合当初那份参数"），因此这里不需要
/// 任何"操作句柄"：配对靠参数本身，不靠系统记住一次操作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectDeclaration {
    action: ActionId,
    class: EffectClass,
    verification: Option<ActionId>,
}

impl EffectDeclaration {
    /// 声明一个能力的效果类别，并（写类别时）指定它的复核能力。
    pub fn new(action: ActionId, class: EffectClass, verification: Option<ActionId>) -> Self {
        Self {
            action,
            class,
            verification,
        }
    }

    /// 被声明的行动标识。
    pub fn action(&self) -> &ActionId {
        &self.action
    }

    /// 声明的效果类别。
    pub fn class(&self) -> EffectClass {
        self.class
    }

    /// 复核能力（只读动作）；没有就是没配。
    pub fn verification(&self) -> Option<&ActionId> {
        self.verification.as_ref()
    }
}

/// 一个能力的**回执从哪个口子进来**——机械事实，不是可信度评级。
///
/// 它存在的理由是**混淆**：一段外来文本与一段自己的字，如果在主体的视野里长得一模一样，那这个
/// 混淆就是**系统造成的**。分类改变不了世界有多脏，它只保证"谁说的"不由系统替主体猜。
///
/// 分档只有一条轴——**口子**，不是"归属者"（谁的机器、谁的凭据、谁付的钱是另外几件各自独立
/// 的事实，不在这里），也不是"关系"（主人/哥哥/妹妹是**世界的内容**，架构对这一格必须角色盲）。
///
/// 五个取值都**可声明**；"不知道"**不可声明**——它是查表时那件能力这次没装配的结论
/// （见 [`SourceTable::class_if_assembled`]），不是某台 App 能声称的状态。
///
/// ⚠ **它落在哪里（2026-10-08 起，见 `agent/README.md` §3.45）**：**详情卡**那一行
/// `receipt-source`（`inspect_action` 与准入面随单附卡**同一份渲染**）。在那之前这一格
/// **从不进主体的视野**——而"判断归主体"要成立，**主体**必须看得到"这段是谁说的"。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceClass {
    /// **主体自己写的字**：笔记、自识、自己钉的纸条与格位。
    ///
    /// 名字刻意**不带人称**：架构对这一格必须**角色盲**——被创建的可能是"哥哥"，也可能是"妹妹"，
    /// 而"自己写的"这句话里的性别是叙述层的事，不是结构事实。同一个理由让这一格不能叫
    /// "本地的"或"私有的"：它说的是**谁的手笔**，不是**存在哪台机器上**。
    SelfAuthored,
    /// **机器的**：系统自己渲染的处境与投影（状态栏、通知栏、事件卡片、读数说明书）。
    System,
    /// **环境根里的材料**：授权资料根里的文件、出厂说明、手册、派生经历。
    Environment,
    /// **世界来的，有署名**：读到的一段话能指回一个明确的发出者（im 房间里的成员消息）。
    ///
    /// 注意它**不等于可信**：一个署了名的对端照样可以骗主体。这一档只保证"混淆不是系统造成的"。
    WorldSigned,
    /// **世界来的，没有署名**：主体自己去拉回来的外部内容（网页、搜索结果、命令输出、外部回执）。
    ///
    /// 这是**默认最严**的一档，也是外部来源 App（MCP / 未来用户自定义）不必声明任何东西就落在
    /// 的那一档——要抬到别的档，得由主人显式背书。
    WorldUnsigned,
}

impl SourceClass {
    /// 稳定分类标记。
    pub fn as_str(self) -> &'static str {
        match self {
            SourceClass::SelfAuthored => "self-authored",
            SourceClass::System => "system",
            SourceClass::Environment => "environment",
            SourceClass::WorldSigned => "world-signed",
            SourceClass::WorldUnsigned => "world-unsigned",
        }
    }
}

/// 一个 App 对**它拥有的一个能力**的来源声明。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceDeclaration {
    action: ActionId,
    class: SourceClass,
}

impl SourceDeclaration {
    /// 声明一个能力的来源档。
    pub fn new(action: ActionId, class: SourceClass) -> Self {
        Self { action, class }
    }

    /// 被声明的行动标识。
    pub fn action(&self) -> &ActionId {
        &self.action
    }

    /// 声明的来源档。
    pub fn class(&self) -> SourceClass {
        self.class
    }
}

/// 装配期建立的**来源声明表**：行动标识 → 来源档，外加"这次装配里到底有哪些能力"。
///
/// 它只做一件事：把各 App 自己说出来的话**机械收集**成一处可查，不授予任何东西。
///
/// ⚠ **2026-09-26 改**：它原来与一张**效果声明表**并列（那张表答"这个动作会不会改世界"，
/// 这张表答"这个动作的回执是谁说的"）。效果表随未决效果门退役了——**门是它唯一的生产读者**；
/// **这一张本刀一步没动。**
///
/// **没有兜底默认值**：查询一律返回 `Option`，`None` = "这次装配里没有这件能力，所以不知道它的
/// 来源"。事实没有安全的默认值——"没声明"只能通向一句假话，不能兜底成任何一档。
#[derive(Clone, Debug, Default)]
pub struct SourceTable {
    entries: BTreeMap<String, SourceClass>,
    /// 这次装配里**存在**的行动标识（各 App 自己声明的能力）。
    capabilities: BTreeSet<String>,
}

impl SourceTable {
    /// 从已装配的 App **机械**收集声明与能力清单。
    ///
    /// 装配期已经保证"每条声明都落在拥有者自己的能力里"且"每个能力都被声明到"
    /// （见 [`AssemblyFailure`] 的两条来源分类），因此这里不重复校验，只收集。
    pub fn of(apps: &[Box<dyn AppPort>]) -> Self {
        let mut table = Self::default();
        for app in apps {
            table.absorb(app.as_ref());
        }
        table
    }

    /// 把**一个** App 的声明并进表里（**展开那一下**用）。
    ///
    /// 出厂内置件的声明在装配期（[`SourceTable::of`]）就收完了；外部来源的 App（MCP server）
    /// 的清单要等真的连上才拿得到，因此并表这条路径必须存在——**同一份声明、同一个收集动作，
    /// 只是时机晚了一点**，不是第二份权威。
    ///
    /// 少了它的后果是 fail-closed：那些动词不在表里 ⇒ [`SourceTable::class_if_assembled`] 读成
    /// **不知道**。这不是"可以不管"的理由——漏一格就是丢了一个事实（把"我不知道这段从哪来"
    /// 说成别的任何一句都是系统制造的混淆）。
    pub fn absorb(&mut self, app: &dyn AppPort) {
        for capability in app.declared_capabilities().items() {
            self.capabilities
                .insert(capability.id().as_str().to_string());
        }
        for source in app.declared_sources() {
            self.entries
                .insert(source.action().as_str().to_string(), source.class());
        }
    }

    /// 把 **OS 自己的**动词登记进来。
    ///
    /// 那六个动词（展开 / 看详情 / 钉 / 摘 / 写自识 / 写交接）**不归任何 App**，因此不由任何
    /// App 声明——它们的唯一来源清单在 `os.rs`（`OS_OWNED_ACTIONS`），这里只是让那张清单也进来源表，
    /// 免得主体的视野里出现"系统自己的动作来源不知道"这种荒谬读数。
    pub fn declare_os_owned(&mut self, action: &str, class: SourceClass) {
        self.capabilities.insert(action.to_string());
        self.entries.insert(action.to_string(), class);
    }

    /// 这个行动**在这次装配里确实存在**时它的来源档；这次没有这件能力就是 `None`（**不知道**）。
    ///
    /// 两个答案绝不能压成一个：
    ///
    /// - `Some(class)` = 这次确实有这件能力，它自己说了回执从哪个口子进来；
    /// - `None` = 这次装配里**没有**这件能力（没授权 / 没装配）⇒ **不知道这段是谁说的**。
    ///
    /// "不知道"必须如实说出来，不许退化成"未署名"——"这段没有署名"与"我不知道这段从哪来"
    /// 是两句不同的事实，把后者说成前者就是**系统制造的混淆**。
    pub fn class_if_assembled(&self, action: &str) -> Option<SourceClass> {
        if !self.capabilities.contains(action) {
            return None;
        }
        self.entries.get(action).copied()
    }

    /// 表里有没有任何来源声明。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 表里全部声明，**按行动标识排序**（读数用；顺序不承载任何判断）。
    pub fn actions(&self) -> Vec<(&str, SourceClass)> {
        self.entries
            .iter()
            .map(|(action, class)| (action.as_str(), *class))
            .collect()
    }
}

/// 把一组 App 装配成一台机器时**无法唯一解释**的原因。
///
/// 两个 App 用同一个标识、或一个行动标识被两个拥有者声明时，路由到谁都没有正确答案；
/// 这时必须拒绝装配，而不是按装配顺序静默覆盖。
///
/// 后三条是**效果声明**的机械检查：声明必须落在自己的能力里、写类别必须配复核、复核必须在
/// 自己的能力里。最后两条是**来源声明**的机械检查：声明必须落在自己的能力里、**每一个能力都
/// 必须被声明到**。各条分类稳定且**互不顶替**——"漏了复核"与"复核不属于自己"是两件事，
/// "多声明了一个能力"与"漏声明了一个能力"也是两件事。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssemblyFailure {
    /// 两个已装配 App 用了同一个标识。
    DuplicateAppId(String),
    /// 一个行动标识被多个拥有者声明（含与 OS 自己的展开行动冲突）。
    ConflictingActionId {
        /// 冲突的行动标识。
        action: String,
        /// 已经拥有它的那一方（App 标识，或 OS 自己）。
        owner: String,
    },
    /// 效果声明里的行动标识**不属于**这个 App（不在它自己声明的能力清单里）。
    UnknownDeclaredEffect {
        /// 声明它的 App。
        app: String,
        /// 被声明的行动标识。
        action: String,
    },
    /// 声明了写类别，却**没有**配复核能力。
    WriteWithoutVerification {
        /// 声明它的 App。
        app: String,
        /// 被声明的行动标识。
        action: String,
    },
    /// 配的复核能力**不在**这个 App 自己的能力清单里。
    VerificationNotDeclared {
        /// 声明它的 App。
        app: String,
        /// 被声明的行动标识。
        action: String,
        /// 配的复核能力（它不属于这个 App）。
        verification: String,
    },
    /// 装配期把某个标识声明成**预装常开**，但那个标识根本没有装配。
    ///
    /// 静默忽略它等于"声明了一件不存在的事"：那是装配期的错，不是运行期的噪声。
    UnknownPreinstalledApp {
        /// 被声明成预装、却没有装配的 App 标识。
        app: String,
    },
    /// 来源声明里的行动标识**不属于**这个 App（不在它自己声明的能力清单里）。
    UnknownDeclaredSource {
        /// 声明它的 App。
        app: String,
        /// 被声明的行动标识。
        action: String,
    },
    /// 一个能力**没有**来源声明。
    ///
    /// 它和 [`AssemblyFailure::UnknownDeclaredEffect`] 是一对：那边管"声明了不存在的东西"，
    /// 这边管"存在的东西没被声明"。漏填必须在这里失败——来源没有安全的默认值，静默兜底
    /// 就是把一句假话写进主体的视野（见 [`SourceClass`] 的文档）。
    CapabilityWithoutSource {
        /// 漏声明来源的 App。
        app: String,
        /// 没有被声明来源的行动标识。
        action: String,
    },
}

impl AssemblyFailure {
    /// 稳定分类标记。
    pub fn as_str(&self) -> &'static str {
        match self {
            AssemblyFailure::DuplicateAppId(_) => "duplicate-app-id",
            AssemblyFailure::ConflictingActionId { .. } => "conflicting-action-id",
            AssemblyFailure::UnknownDeclaredEffect { .. } => "unknown-declared-effect",
            AssemblyFailure::WriteWithoutVerification { .. } => "write-without-verification",
            AssemblyFailure::VerificationNotDeclared { .. } => "verification-not-declared",
            AssemblyFailure::UnknownPreinstalledApp { .. } => "unknown-preinstalled-app",
            AssemblyFailure::UnknownDeclaredSource { .. } => "unknown-declared-source",
            AssemblyFailure::CapabilityWithoutSource { .. } => "capability-without-source",
        }
    }
}
