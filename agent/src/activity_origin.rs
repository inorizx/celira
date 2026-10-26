//! **活动起源那一块**（显示区的一件）：把"这次 Run 是怎么开始的"摆成开场的一块。
//!
//! ## 它为什么自己一个家
//!
//! 它与时钟、屏幕读数格**同一个病**：这三件从前住在循环＋插槽那一层（`src/os.rs`）里，
//! 于是**每一个显示区**为了拿它当处境链的最内层，都要去 import 那一层；而**存储那一层**
//! 为了渲染一句人话，也要反过来去 import 它——按手机模型，**那是下层依赖上层**。
//!
//! 那两条反向依赖合起来，和另一条边构成过一个三节点的环（`循环层 → 文件视图 → 存储 → 循环层`）。
//! 起源是**显示区的内容**，不是循环的一部分：它的家在这里，环随之解开。
//!
//! ## 起源是一句声明，不是常量
//!
//! 同样是"被叫醒"，**手动发起、门铃发起、钟源发起的事实不同**，而起源一旦撒谎
//! （门铃那一次被记成"宿主手动发起"）就是在事实系统里写错话。因此正文由构造参数给出：
//! [`ActivityOrigin::new`] 用手动那一句，[`ActivityOrigin::with_origin`] 用调用方声明的那句。
//!
//! ## 起源那句只说自己
//!
//! 唤醒通道有**两态**，而两态覆盖的东西**不一样**：**刚接上**覆盖**订阅间隙**（断着的那段时间里
//! 可能发生过事情），**报了一次变化**只说线上来了一帧（帧里不带任何内容）。因此那几句正文只有
//! 一条判据：**唤醒通道是"可能有新的"这条信号，不是"有新的"这条事实**（契约
//! `CORE_V1_AGENT_PROTOCOL.md` §8 的口径：那条通道不带事实）⇒ 正文**只许说自己**（我是怎么被
//! 叫醒的），**两态都不许暗示世界里有内容**——"有新事件"是推断，不是它的事实。那句话的正身与
//! 更正，见 [`WAKE_OPENED_ORIGIN_TEXT`] 的注。
//!
//! ⚠ **那几句正文里不出现内部代称**（`doorbell` 是一个比喻）：比喻自带"有人来了"的推断，会连着
//! 它自带的推断一起搬进主体的视野，因此正文只说 "the world" 与唤醒通道。**代码标识符一个没动**
//! ——按「三类读者、三套判据」，内部代称归开发者、模型可见的那一份归主体。
//!
//! ⚠ **文书写对了不等于文案写对了**：2026-10-13 之前，开发者文档里两态都写着，而进主体视野的
//! 那一句仍只有一态——那一次正是这样漏的。⇒ 这条判据钉在**文案自己**身上，不钉在别处的描述上。
//!
//! ## 它不是什么
//!
//! 它是**环境内容**的搬运者，不是调度器、不是计划器、不是通知系统：
//!
//! - Run 级恰好给出一个内容块（[`ACTIVITY_ORIGIN_LABEL`] ＋ 那一句正文）：**一句话，没有字节上限**
//!   ——今天最长的那一句是 [`WAKE_OPENED_ORIGIN_TEXT`]（**199 B**，2026-10-19 数过），加标签仍是一行；
//! - Turn 级**稳定返回空批次**：没有"每轮变化的环境事实"，因此不产生任何记录，
//!   也不改变模型调用次数；
//! - 它**不持有**任何跨活动状态，也拿不到历史、行动端口、模型端口或账本（接缝给它的
//!   [`InjectionContext`] 是只读身份与轮次），因此不可能替主体判断进度或产生动作；
//! - 它**不读写任何存储**：正文由装配那一层读出来、交进来（这个模块不 import 存储）；
//! - 内容**不进入**地基的事实：它只活在这一次活动的运行轨迹里；
//! - 它是**事实提示**，不是安全隔离或权限边界：文本格式不授予任何权限，也不改变消息角色。
//!
//! 是否开始一次活动、什么时候开始，仍由调用方决定；这里不包含任何"醒来"的机制。

use crate::core::{ContentBatch, ContentBlock, ContextSource, InjectionContext};

/// Run 级内容块的标签：这条事实是"这次活动怎么被发起的"。
pub const ACTIVITY_ORIGIN_LABEL: &str = "activity-origin";

/// 手动发起的活动的起源正文：一条机械事实，不含建议、任务、进度或优先级。
pub const MANUAL_ORIGIN_TEXT: &str =
    "This activity was started manually by the host. No external event triggered it.";

/// **唤醒通道刚接上**那一档的起源正文（2026-10-13 起与"报了一次变化"分开）。
///
/// 它同样只是**一条机械事实**——只说"谁把主体叫醒的"。而它必须说清那一帧**覆盖什么**：
/// 覆盖**订阅间隙**（断着的那段时间里可能发生过事情），**不声称"刚刚有变化"**。
///
/// ⚠ 这一格从前与"报了一次变化"共用一句，而那句写的是
/// `...the backend event stream signaled that new events are available in the im app.`
/// ——**把"可能有新的"写成了"有新事件"**。真机上它的代价是 **15 段以为有事的空醒**
/// （`agent/README.md` §3.74）：不是主体笨，是那句话替机制多说了一句。
pub const WAKE_OPENED_ORIGIN_TEXT: &str = "This activity was started by a wake-up from the world: the wake-up channel had just been (re)established, which covers the time it was down — it does not say whether anything happened in that time.";

/// **唤醒通道报了一次变化**那一档的起源正文。
///
/// 线缆上**不带内容**：「有变化」是"服务端在事件变化时才发这一帧"这件机械事实，
/// **不是**"im 里有新事件"——说了什么、有几条、要不要看，**全都不在线缆上**。
pub const WAKE_CHANGED_ORIGIN_TEXT: &str = "This activity was started by a wake-up from the world: the wake-up channel reported a change. Nothing about what changed is on the line.";

/// **旧账**那一档的起源正文（`run-start.origin = bell`，两态分开之前）。
///
/// **照读不写**：新记录产不出它。它**不替旧记录认**是哪一种——那句话如实说"这一条没说是哪一种"。
pub const WAKE_LEGACY_ORIGIN_TEXT: &str = "This activity was started by a wake-up from the world (this record is from before the two kinds of wake-up were told apart).";

/// 本地钟源发起的活动的起源正文：同样是**一条机械事实**——只说"谁把主体叫醒的"。它对应的是
/// **这个环境里约下的那个时刻**到点了（含离线错过之后的补响；迟到也一样说"到了"，不声称准点）。
pub const TIMER_ORIGIN_TEXT: &str =
    "This activity was started by the timer: an alarm set in this environment has arrived.";

/// **外部输入入口**发起的活动的起源正文：一条机械事实——只说"有外部事件落进了这只环境的
/// 收件箱"，**不说**它是什么、急不急、该不该处理（那些都要主体自己去看）。
///
/// 正文不在唤醒线上；入口那条线只回答"有一件事落了盘"。读法也写在这句里，因为它是
/// **机械形状**（在哪能读到），不是对内容的判断。
pub const CHANNEL_ORIGIN_TEXT: &str = "External input for this environment has arrived; the event is in the channels inbox, readable through the files view. Its content is not on the wake-up line.";

/// 处境链的最内层：只说"这次活动是怎么被发起的"。
///
/// 默认构造给的是手动那一句；装配那一层把**存储里的词表值**翻译成对应的人话，再用
/// [`ActivityOrigin::with_origin`] 交进来——**翻译发生在装配那一层**，本件不认识那个词表，
/// 存储也不认识本件。
#[derive(Debug, Clone)]
pub struct ActivityOrigin {
    origin: String,
}

impl Default for ActivityOrigin {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivityOrigin {
    /// 装配一个手动来源：起源 ＝ 宿主手动发起、没有外部事件触发（既有形状）。
    pub fn new() -> Self {
        Self {
            origin: MANUAL_ORIGIN_TEXT.to_string(),
        }
    }

    /// 用调用方**声明**的起源正文装配。
    ///
    /// 空正文会被拒绝（退回既有那句）——"没有起源"不是一种可以如实表达的处境。
    /// 除此之外它不做拼接、不改写：起源是宿主的声明，本层只搬运。
    pub fn with_origin(origin: impl Into<String>) -> Self {
        let origin = origin.into();
        if origin.trim().is_empty() {
            return Self::new();
        }
        Self { origin }
    }

    /// 这次活动的起源正文（只读）。
    pub fn text(&self) -> &str {
        self.origin.as_str()
    }
}

impl ContextSource for ActivityOrigin {
    fn on_run_start(&mut self, _context: &InjectionContext) -> ContentBatch {
        let block = ContentBlock::new(ACTIVITY_ORIGIN_LABEL, self.origin.as_str());
        // 起源正文由调用方声明（可以很长）：装不下时留一条读数，**不**静默退回空批次——
        // "这一次没有起源"与"起源超限未给"是两句不同的话。
        ContentBatch::of_or_not_delivered(vec![block], ACTIVITY_ORIGIN_LABEL)
    }

    fn before_turn(&mut self, _context: &InjectionContext) -> ContentBatch {
        ContentBatch::empty()
    }
}
