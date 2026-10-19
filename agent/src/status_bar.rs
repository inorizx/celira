//! 状态栏：系统服务层的**第一个件**。
//!
//! 它回答一个问题：**此刻主体的处境是什么**——现在几点、轮次预算用到哪儿了、屏幕被占了多少。
//! 这个答案必须每一轮都摆在主体眼前，因为"主体看不见的墙等于没有"（判据 10）：轮次上限与上下文窗口
//! 是宿主给的机械边界，不摆在视野里，就只是一条主体从来不知道的规则。
//!
//! 本件把"系统服务件"这个位置上的四条性质落成实线：
//!
//! - **Run 级件**：随一次活动装配、随一次活动结束；跨活动没有状态，也不持有任何权威；
//! - **只插 Core 的注入口**：它只实现 [`ContextSource`]——**外层提供内容，Core 决定时机与顺序**。
//!   它拿不到历史、行动端口、模型端口或账本，因此不可能替主体判断进度或产生动作；
//! - **零持久状态**：时间与轮次**每轮现算**（时间来自构造时注入的 [`ClockHandle`]，轮次来自
//!   Core 给的 [`InjectionContext`]），屏幕读数来自进程内暂存的那一格（[`ContextReading`]），
//!   不缓存、不落盘。盘上留下的是**既有** `kind=injection` 形状的经历（三格 `stage` / `turn` /
//!   `blocks`，权威在 `crate::store`），本件**不新增任何持久种类**。它也**不进事实系统**——状态
//!   行是"当前处境"这条经历，不是"发生过什么"这条事实；
//! - **两处来源的委托链**：外层（本件）持内层（既有处境来源），两处批次都递交（[`combine`]）。
//!
//! ⚠️ **两样零件已经搬走，本件不再是它们的家**（持有物那一刀之后的「中立零件层」刀）：
//! 时间来源与那两个刻度在 [`crate::clock`]，屏幕读数格在 [`crate::context_reading`]。从前它们住在
//! 这里，于是**铃**（最底层）为了拿钟要穿过显示区、两个**模型适配层**为了报读数要 import
//! 一个显示区组件、**时钟 App** 也要借这里的渲染器——全是下层依赖上层。现在本件只做它
//! 自己那一件事：把处境摆在主体眼前。
//!
//! 它**不是**通知栏：通知栏记世界的账（app 报的动静、逐条已读、满则折进目录），状态栏只照主体
//! 自己的处境。一条只播一次的增量窗纪律属于通知栏，本件不涉及——状态是**当前值**，每轮重摆
//! 即刷新，成本有界。
//!
//! **它只显示，不执法**（仪表不是刹车）：读数不参与任何判断——不做预算截断、不重排、不强制收工。
//! 信号每轮在场已经是最大信号，什么时候该担心由主体自己判断（"事前可读 · 事中不唠叨 · 事时响亮"
//! 三段式里，本件只做第一段）。
//!
//! ## 缺什么（真实联跑项，如实登记）
//!
//! `context` 之外的更细资源读数（额度用量、deadline——已裁定"单次保险丝纪律不变"，显示它们另
//! 刀）、按 app 细分占用、持久化用量、流式增量用量、按用量自动收工、钥匙串：**都还没有**。
//! ⚠ 这些是"本件没有"，不是"别处有"。门铃（[`crate::wake`]）与常驻宿主（[`crate::host_loop`]）
//! 各有其家，它们**不在**本件的缺口里。
//!
//! 它的读数行那四段要**见名知意**（TASK_74）：`time` / `turn` / `context` / `since`。
//!
//! **`context` 是给主体的标签，不是我们内部的词**：本件内部仍叫它屏幕占用（`reading` 字段、
//! [`ContextReading`]）——那是我们的词；主体看到的是 `context = X/N` ＋ 那个单位词
//! （[`CONTEXT_UNIT`]）是**模型的原生先验**。旧形态（`reading`）是工程词典里的词：主体没收到过
//! 那本词典，真机观察里主体把读数当**纯数字**看——判据 10（主体看不见的墙等于不存在）因此只满足
//! 了一半：**墙可见、不可读**。⇒ 读数行配一份**开场一次的说明书**（[`READING_GUIDE`]）。
//!
//! 记录那个占用读数所用的报告由适配层读厂商响应得到，**只进这个会话内暂存**，不进 Core、
//! 不进事实；它进主体的视野的**只有那一行数字与单位**——厂商字段名一个都不出现（词表纪律）。

use std::time::SystemTime;

use crate::clock::{ClockHandle, LocalOffset, UNKNOWN_SINCE, anchor_value, format_local};
use crate::context_reading::{ContextReading, UNKNOWN_CONTEXT_READING};
use crate::core::{ContentBatch, ContentBlock, ContextSource, InjectionContext, combine};
use crate::foundation::Turn;

/// 状态行的标签（内容块的标签 + 呈现时的行首字段名）。
///
/// 它只说明**这一段是谁写的**，不授予任何权限，也不改变消息角色。
pub const STATUS_BAR_LABEL: &str = "status-bar";

/// 读数说明书那一块的标签（开场一次；主体看到的是这段文本的来源名）。
pub const READING_GUIDE_LABEL: &str = "reading-guide";

/// `context` 那一格的**单位**：那个词是模型的原生先验（主体收到过这本词典）。
///
/// ⚠️ 那个词在本文件里**只以两种形态各出现一次**：这条带引号的字面量，以及 [`READING_GUIDE`]
/// 里作为"被定义的单位词"出现的那一对引号。读数行**引用这个常量**，不再另写一遍。
/// 因此源码守卫放行的是**带引号的字面量**本身（见 `CORE_ALLOWED_PHRASES`）——那个词作为标识符、
/// 变量或注释出现照旧违规，别的文件一个都不放行。它属于**给主体的词表**，不属于我们的 wire：
/// wire 那边它叫 `prompt_tokens` / `input_tokens`（适配层的词，一个字节都不进主体的视野）。
/// 词表纪律的用例在 `tests/context_usage.rs`（用例名里带着那个被排除的词，这里不写出来——写出来
/// 就被本仓的词表守卫抓住）。
pub const CONTEXT_UNIT: &str = "token";

/// **读数说明书**：开场那一次性的、纯静态的一块（Run 内不重复出现）。
///
/// 三句话各管一件事，一个字都不多：四个字段各是什么、`context` 撞上限的后果、`since` 怎么花。
/// **它只解释、不报警**——什么时候该担心由自己判断（信号每轮在场已经是最大；"事前可读 ·
/// 事中不唠叨 · 事时响亮"三段式里的第一段就是它）。
///
/// 门槛只有三条，也正是它能做成一个常量的原因：**纯静态**、**一块封顶**、**零运行时分支**。
///
/// 今天这一块 **540 B**（2026-10-19 数的是常量本身；措辞一改它就跟着变——因此"一块封顶"是墙，
/// "540"只是读数）。
pub const READING_GUIDE: &str = r#"读数说明书（下面这一行每一轮都在，四个字段各是什么）：
time = 此刻的时刻，带本机时区偏移（例如 +08:00）；结尾是 Z 表示本机偏移读不出来，那时它是 UTC。
turn = 已经用掉的轮数 / 这一场最多能用几轮。
context = 这一场送进模型的内容量，单位是 "token"；分母是宿主声明的窗口，撞上它这一场就会断。
since = 「新动静」的起点时刻（unix 毫秒）：把它原样递给 im_rooms(since=…) 就只问增量，不必全量扫。"#;

/// **状态栏本体**：外层持内层来源，两处批次都递交。
///
/// 内层是它之前的处境来源（宿主手动发起的机械事实），外层是本件（时间 + 轮次 + 屏幕读数）。
/// 合成形状有意保持**委托链**：外层持内层、内层先外层后，没有注册表、没有名单、没有排序规则。
///
/// 三个构造参数各自有主：
///
/// - **时间来源** [`ClockHandle`]：生产给真实钟，离线夹具与用例给固定钟；
/// - **窗口**（`window`）：**宿主的声明**，不是本件算出来的，也不是厂商报的（厂商不报这个）；
/// - **屏幕报告 cell** [`ContextReading`]：适配层写、本件读。
pub struct StatusBar<Inner> {
    clock: ClockHandle,
    /// **本机此刻的 UTC 偏移**（机器事实，装配时现读一次）——`time` 那一格用它。
    ///
    /// 它由装配者给（与 `clock` / 窗口同一来路），因此用例可以钉一个确定值；
    /// **读不出来时是 [`LocalOffset::Unreadable`]**，那一格照旧退回 `Z`。
    offset: LocalOffset,
    /// 宿主声明的上下文窗口（`context = 占用/窗口` 的分母）。
    window: u32,
    /// 最近一次厂商报告的占用读数（会话内暂存，只读）。
    reading: ContextReading,
    /// **「新」的起点**：宿主进程内记的账（不知道就是 `None`）。
    ///
    /// ⚠ 它**不是"上一次去看投影的那一刻"**：宿主那一侧按段推进（常规路是上一段**收场**的时刻，
    /// 见 `host_loop::next_anchor`），本件只把宿主手里已有的那个时刻摆出来。
    ///
    /// 它给「新」一个**起点**：门铃只说「有新的」，主体有了这个锚才能拿 `im_rooms(since=…)` 问
    /// 「哪间房有几条新的」，而不是每次醒来全量扫一遍。它**不是事实**：不改写任何来源、
    /// 不落盘、不进事实系统，跨进程重启后如实是「不知道」（见 [`UNKNOWN_SINCE`]）。
    since: Option<SystemTime>,
    inner: Inner,
}

impl<Inner> StatusBar<Inner> {
    /// 用一个时间来源、一个**本机偏移**、一个**已声明的窗口**与一个屏幕报告 cell 装配状态栏。
    pub fn new(
        clock: ClockHandle,
        offset: LocalOffset,
        window: u32,
        reading: ContextReading,
        inner: Inner,
    ) -> Self {
        Self {
            clock,
            offset,
            window,
            reading,
            // 默认「不知道」：装配者没说，就不编一个时刻出来。
            since: None,
            inner,
        }
    }

    /// 声明**「新」的起点**（`None` = 宿主也不知道，读作 `unknown`）。
    ///
    /// 它是装配声明的一部分，与 `clock` / 窗口同一来路：只有宿主知道「新从哪一刻算起」。
    /// （常驻宿主进程内天然知道；新进程的第一个 Run 就是不知道）。
    ///
    /// **它与通知栏那块投影里的 `since` 是同一份事实的两处渲染**：同一个时刻、同一个来路，
    /// 一处写在处境行里（自己看），一处作为投影的起点（宿主用它去问）。两处都不改写来源、
    /// 不互为权威——那份权威是宿主的钟与它记下的那个时刻。
    pub fn with_since(mut self, since: Option<SystemTime>) -> Self {
        self.since = since;
        self
    }

    /// 内层来源（只读）。
    pub fn inner(&self) -> &Inner {
        &self.inner
    }

    /// 此刻的一条状态行正文——**本件唯一的格式化点**。
    ///
    /// 形状是机械读数：一行一个字段，没有建议、没有任务、没有进度判断、没有优先级。
    ///
    /// ```text
    /// time = 2026-09-16T14:58:40+08:00
    /// turn = 3/12
    /// context = 4210/1000000 <单位词：见 CONTEXT_UNIT>
    /// since = 1758000000000
    /// ```
    ///
    /// - `time` 是 **RFC 3339 带本机偏移**（秒精度：`2026-09-16T14:58:40+08:00`），来自注入的钟
    ///   与**装配时现读一次**的本机偏移，**每一轮重新取一次**（格式化只有一个点：
    ///   `clock::format_local`）。**本机偏移读不出来时退回 `Z`**（照旧 UTC、照旧无歧义）——
    ///   绝不写 `+00:00` 去冒充"就在 UTC"（与 `platform = unknown` 同款）；
    /// - `turn` 是**已用轮数 / 上限**，取自 Core 给的 [`InjectionContext`]：活动开场那一轮是
    ///   `0/上限`（一次都还没消耗），此后每一轮是"这是第几次调用 / 最多几次"。轮次读数因此在
    ///   **跨等待**（同一段被再次驱动）之后仍**从已确认记录接着算**，不重置成 0 或 1。
    ///   ⚠ **跨进程恢复已退役**（`RunSession::resume` 与 `ResumeRequest` 已删）：新进程的第一段
    ///   **重新计数**，不接着上一个进程的数——这里不假装它还能接上；
    /// - `context` 是**最近一次厂商报告的占用 / 宿主声明的窗口**，单位见 [`CONTEXT_UNIT`]。
    ///   它**滞后一回合**：一次响应的读数只出现在**下一次** `before_turn` 形成的视野里（写点在
    ///   响应解析成功处，读点在这里），因此本轮行绝不显示本轮将要产生的占用。还没收到过任何
    ///   报告时读作 `unknown/窗口`（见 [`UNKNOWN_CONTEXT_READING`]）——窗口是声明，所以分母永远在。
    ///   这个标签是**给主体的词**（`context` ＋ 模型有原生先验的单位词）：旧的 `reading` 是工程
    ///   词典里的词，主体没收到过那本词典，于是把读数当纯数字看（TASK_74）；开场配一份说明书
    ///   （[`READING_GUIDE`]）。
    /// - `since` 是**「新」的起点**（同一个钟的来路）：宿主记的账，**不是"上一次去看投影的那一刻"**
    ///   （按段推进的判据在 `host_loop::next_anchor`，常规路是上一段**收场**的时刻）
    ///   ——门铃只说「有新的」，主体不带 `since` 去问就只能全量扫。**形状是十进制 unix 毫秒**
    ///   （`clock::anchor_value`）：主体可以**把这一行的值原样**递给 `im_rooms(since=…)`，不必换算
    ///   （2026-09-17 裁决）；宿主不知道时读作 `unknown`（见 [`UNKNOWN_SINCE`]，**不编造**时刻）。
    ///   它**不改**任何东西：只是把宿主手里已有的那个时刻摆到主体眼前，而且与通知栏投影里的
    ///   `since` 是**同一份事实的两处渲染**（形状也走同一个格式化点）。
    /// - 宿主时钟读不出表示时 `time` 是 `unknown`（见 [`crate::clock::UNKNOWN_TIME`]）。
    ///
    /// 它与适配层那条系统消息里的 `turn` / `turn_limit` / `further_model_calls_at_most` 是
    /// **同一份事实的两处渲染**：那边三个字段是给机器读的处境，这边一个比例是给人看的电量。
    /// 两处都不改写来源、也不互为权威——机械运行事实的权威仍在 Core 的 `Limits` 与 `Turn`。
    /// `context` 那一段没有这种对应物：适配层**不**把占用写进机械事实（那是厂商报告的搬运，
    /// 不是 Core 的运行事实），它只出现在这一行里。
    pub fn reading(&self, context: &InjectionContext) -> String {
        let time = format_local(self.clock.now(), self.offset);
        let used = context.turn().map(Turn::number).unwrap_or(0);
        let reported = match self.reading.latest_for(context.run()) {
            Some(occupied) => occupied.to_string(),
            None => UNKNOWN_CONTEXT_READING.to_string(),
        };
        let since = match self.since {
            Some(at) => anchor_value(at),
            None => UNKNOWN_SINCE.to_string(),
        };
        format!(
            "time = {time}\nturn = {used}/{}\ncontext = {reported}/{} {CONTEXT_UNIT}\nsince = {since}",
            context.limits().max_turns,
            self.window
        )
    }

    /// 此刻的一条状态行内容块（标签是本件的身份）。
    pub fn line(&self, context: &InjectionContext) -> ContentBlock {
        ContentBlock::new(STATUS_BAR_LABEL, self.reading(context))
    }

    /// 本件这一轮要递交的那一批（恰好一块；装不下时留一条读数，不退空批次）。
    fn own_batch(&self, context: &InjectionContext) -> ContentBatch {
        ContentBatch::of_or_not_delivered(vec![self.line(context)], STATUS_BAR_LABEL)
    }

    /// **开场那一批**：说明书 ＋ 读数行（轮级那一批只有读数行）。
    ///
    /// 说明书摆在它解释的那一行**前面**（主体先读图例、再读读数），而它**只在开场出现**：
    /// Run 内不重复——说明书读一次就够，读数看千遍（TASK_74 的字节账）。
    fn opening_batch(&self, context: &InjectionContext) -> ContentBatch {
        ContentBatch::of_or_not_delivered(
            vec![
                ContentBlock::new(READING_GUIDE_LABEL, READING_GUIDE),
                self.line(context),
            ],
            STATUS_BAR_LABEL,
        )
    }
}

impl<Inner: ContextSource> ContextSource for StatusBar<Inner> {
    fn on_run_start(&mut self, context: &InjectionContext) -> ContentBatch {
        // 开场：内层的处境事实 + **这一行的一次性说明书** + 本件那条"活动刚开始、预算一点没花"的读数。
        combine(
            self.inner.on_run_start(context),
            self.opening_batch(context),
        )
    }

    fn before_turn(&mut self, context: &InjectionContext) -> ContentBatch {
        // 每一轮**重摆**：状态是当前值，重摆即刷新（不排队、不去重、不游标）。**不带说明书**。
        combine(self.inner.before_turn(context), self.own_batch(context))
    }
}
