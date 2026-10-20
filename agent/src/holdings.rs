//! 持有物（Holdings）：**主体自己的安排，系统只渲染**——Run 开场把主体还没了结的事摆出来。
//!
//! ## 为什么它必须自己出场
//!
//! 主体只有瞬间记忆：他要用上自己留下的东西，前提是**知道它存在**。知道只有两条路——记得，
//! 或者被摆出来。第一条按前提不成立，所以第二条是必需的。**一件他自己安排、却不出现的东西，
//! 等于不存在。**
//!
//! **判据一句话：持有物在开场必须自己出现**——主体**不必**去查。主体查得到（时钟 App 有
//! `timer_list`），但那份清单只在**主体想起来去看**的那一刻才存在，而"想起来"按前提不成立
//! ⇒ **查得到 ≠ 会去查**。开场这一块因此不是"又一个查询口"，而是那些事**不靠主体记得**的
//! 那一次出场。
//!
//! ## 两个成员（2026-10-11 起）
//!
//! 1. **闹钟**（时钟 App 里 `timer_set` 约下的时刻，`label` 是写给未来自己的那一行）；
//! 2. **主体自己起的东西**（终端 App 的「这件要活到关家」，见 `terminal_held`）——第二支成员
//!    经主人裁决加入（判据 17 的锚本来就写着「**进程/端口** ＋ 自 arm 闹钟清单，带年龄」）。
//!
//! ⚠ 成员**按裁决加入**，不是随手加的类型：**加一支之前先裁「它算不算持有物」**——本件**不预建**
//! 别的成员（"这一格的主语是谁"答不上来的那一类，就不是持有物）。
//!
//! ⚠ **两支各有自己的词**（[`HoldingState`] 是"约的时刻"的词，[`HeldState`] 是"起的东西"的词）：
//! 不合并，因为合并之后"这一格的主语是谁"就读不出来了。**没有第二支成员时，本件输出的每一个
//! 字节都与从前相同**（既有消费者一个字都不用改）。
//!
//! ## 素材从哪来：**App 交上来，本件不认识那个 App**
//!
//! 手机模型 §3 的分层是自下而上 **1 循环＋插槽 / 2 显示区 / 3 app / 4 Agent**，而第 3 条写着
//! **"app：往显示区交素材"**——在依赖上那就是 `app → 显示区`（**上层依赖下层**）。
//! 因此本件**定义**它收什么（[`HoldingRow`]），两个 App **依赖本件**、把素材填进来，而且**各交各的**：
//! 闹钟 → `timer_app::holdings_material`，起过的东西 → `terminal_app::held_material`。
//! 两支怎么接在一起归**装配层**（`composition.rs` 的 `merge_holdings`，它也只转手）。
//!
//! ⚠️ 反过来的写法（本件 import `timer_app` 的记录类型、装配层直接翻它的账本）是**下层认识上层**，
//! 曾经真的这么写过一版——那等于"锁屏去翻时钟 App 的数据库"，没有哪台手机会这么干。
//! 真实手机里 `Notification` 这个类是**平台**定义的、app 填内容，正是本件的形状。
//!
//! ## 与邻居的分工（一句话）
//!
//! - **纸条板 = 主体的字**（他写的内容，`notes_pin` 贴上墙，系统渲染预览）；
//! - **动作 pin = 主体的格位**（工具面，`pin_action` 钉住动作详情）；
//! - **持有物 = 主体的安排**（自己约的时刻 ＋ 自己起过的东西）——本件。
//!
//! ## 三条纪律
//!
//! 1. **系统永不自动了结 / 取消**：本件只读，一个字节都不写——"约什么、什么时候"永远是主体的
//!    动作（负对照：不存在任何自动消费的代码路径）；
//! 2. **缺省不等于零**：读失败 ⇒ 如实写 `holdings = unknown`（不冒充"没安排过"）；表在、一条
//!    也没有 ⇒ 写 `holdings = 0`。这是开场快照，与"零动静不出块"相反：快照不说话，主体就分不清
//!    "空"与"没读到"；
//! 3. **`pending` 永远优先，而且绝不静默折掉**：还没到点的排在前、按原定时刻升序；`fired` 是历史，
//!    取最近的，多的**如实报**。**已过期却还没响过的**（`pending` + `overdue`）照原样列出——
//!    它正是"这一次没响成"的读数，比任何一条 `fired` 都重要。
//!
//! ## 已知代价（如实记账）
//!
//! **表是权威 ⇒ `fired` 行每一场都看得见**："不重复报"这一格被**故意**放掉了——它要求有一个
//! "报过没有"的进程内账本，而那个账本会丢，丢的那一次就是「崩在收掉与开活动之间 ⇒ 永远不报」。
//! 宁可重复，也不丢。多出来的那几条，主体可以自己取消（时钟 App 的取消是**除名**）——
//! 本件**不替主体清**（纪律 1：只读）。
//!
//! ## 它为什么是开场快照，而不是每轮重摆
//!
//! 连续性宪法判据 17 的锚点写的是「状态栏（…自 arm 闹钟清单，带年龄）」。本件把**同一条判据**
//! 落成**开场快照**（与纸条板同款），理由是机械的：
//!
//! - 持有物在**两次清醒之间**变化——闹钟正是这样把人叫醒的——开场就是它必须被看见的那一刻；
//! - 而状态行是**每轮重摆**的，一条变长的列表会被反复注入，直接吃视野预算（判据 2）。
//!
//! 锚点差异如实记在这里，**判据本身一字未动**；要不要改成每轮重摆是主人的取舍。
//!
//! ## 它不是什么
//!
//! 它不认识历史 / 行动 / 模型 / 账本，也**不替主体判断**哪一条重要、该不该去处理。它只是把
//! 拿到的素材机械地摆出来，连"你还欠着什么"这句人话都不说。

use crate::core::combine;
use crate::core::{ContentBatch, ContentBlock, ContextSource, InjectionContext};

/// 持有物内容块的标签（呈现时的区域名）。
pub const HOLDINGS_LABEL: &str = "holdings";

/// 一行标签最多取多少字节做预览（与 im 的房预览、纸条板同一惯例）。
pub const PREVIEW_BYTES: usize = 60;

/// 一次最多渲染几行（`pending` 优先，`fired` 取最近的；多的如实报）。
///
/// 比纸条板的 50 小：纸条是主体策展过的长期内容，而闹钟是短命的——`fired` 行只增不减，
/// 有用的窗口就在最近这几条。
pub const MAX_RENDERED: usize = 20;

/// 「这一次没读到」时的机械读数（与通知栏、纸条板同族）。
pub const UNKNOWN_HOLDINGS: &str = "unknown";

/// 一条持有物的**状态**——本件自己的词。
///
/// 它与时钟 App 磁盘上那两个词（`timer_app::STATE_PENDING` / `STATE_FIRED`）**值相同、来源不同**：
/// 一个是"盘上怎么写"，一个是"给主体怎么看"。将来其中一个变了不该牵连另一个（与
/// `clock::UNKNOWN_TIME` / `reading::UNKNOWN_CONTEXT_READING` / `clock::UNKNOWN_SINCE` 同一条纪律）。
/// App 在交素材时把它的盘上状态**映射**进来，本件不解析任何磁盘词。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldingState {
    /// 还没到点。
    Pending,
    /// 到过点了。
    Fired,
}

impl HoldingState {
    /// 给主体看的那个词。
    pub fn as_str(self) -> &'static str {
        match self {
            HoldingState::Pending => "pending",
            HoldingState::Fired => "fired",
        }
    }
}

/// **一件活到关家的事**的状态——本件自己的词。
///
/// 它与时钟 App 的 [`HoldingState`] **分开**：那是"约下的时刻"的词（`pending`/`fired`），
/// 这是"起的东西"的词（`running`/`gone`/`unknown`）。两套各自有主、不合并——把进程塞进
/// 闹钟的状态与年龄词里，会让"这一格的主语是谁"变得读不出来。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeldState {
    /// 它还在跑。
    Running,
    /// 它不在了。
    Gone,
    /// **读不到**（不是我的进程、权限不够）——与"不在了"是**两件事**。
    Unknown,
}

impl HeldState {
    /// 给主体看的那个词。
    pub fn as_str(self) -> &'static str {
        match self {
            HeldState::Running => "running",
            HeldState::Gone => "gone",
            HeldState::Unknown => "unknown",
        }
    }
}

/// **App 交给本件的素材**：一条持有物的结构化内容。
///
/// 它**不是**任何 App 的记录类型——那是它们自己的事。App 只交"主体安排了什么、现在什么状态"，
/// **怎么摆成一行归本件**（手机模型 §5：「内容归 app 交，怎么显示手机说了算」）。
///
/// **两个成员各占一支**（2026-10-11 主人裁决：第二支成员 = 主体**自己起的东西**）：
/// 一支一个词表，谁也不借谁的词。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HoldingRow {
    /// 主体**约下的一个时刻**（时钟 App 交上来）。
    Timer {
        /// 机械身份（主体约的时候拿到的那个）。
        id: String,
        /// 现在的状态。
        state: HoldingState,
        /// 主体约的那个时刻（unix 毫秒）。
        when_millis: u64,
        /// 写给未来自己的那一行（可能为空）。
        label: String,
    },
    /// 主体**起的一件活到关家的事**（终端 App 交上来）。
    Held {
        /// 主体起它时给的那个名字（看它、停它都用这个名字）。
        name: String,
        /// 现在什么状态（三态，见 [`HeldState`]）。
        state: HeldState,
        /// 主体起它的那个时刻（unix 毫秒）。
        started_millis: u64,
        /// 当时那条命令（原文）。
        command: String,
        /// 它的输出落在哪儿（主体用文件动词读那个路径）。
        log: String,
        /// 那个文件现在多少字节（`None` = **读不到**，不是 0）。
        bytes: Option<u64>,
    },
}

/// 持有物：一块开场快照 + 内层来源。
#[derive(Debug)]
pub struct Holdings<Inner> {
    /// 装配期由 App 交上来的素材。
    ///
    /// `None` = 这一次**没读成**（App 报错）——它与 `Some(vec![])`（读成了，一条也没有）是
    /// 两件事，因此这里必须是两层而不是一个空列表（缺省不等于零）。
    rows: Option<Vec<HoldingRow>>,
    /// 装配那一轮的钟（unix 毫秒）。年龄读数用它，**不另取系统钟**——夹具的固定钟因此照样
    /// 给出确定的读数，"同一时刻同一输入 ⇒ 逐字节相同输出"这条判据可判错。
    ///
    /// `None` = 钟读不出来（1970 之前）⇒ 年龄那一格如实写 `unknown`，**不编一个数**。
    now_millis: Option<u64>,
    inner: Inner,
}

impl<Inner> Holdings<Inner> {
    /// 用 App 交上来的素材、装配那一轮的钟与内层来源装配本件。
    pub fn new(rows: Option<&[HoldingRow]>, now_millis: Option<u64>, inner: Inner) -> Self {
        Self {
            rows: rows.map(|rows| rows.to_vec()),
            now_millis,
            inner,
        }
    }

    /// 本件那一条读数。
    pub fn reading(&self) -> String {
        match &self.rows {
            // 没读成：如实说"这一次没读到"，不冒充"没安排过"。
            None => format!("holdings = {UNKNOWN_HOLDINGS}"),
            // 读成了、表里空着：把零说出口（主体才知道是"空"，不是"没读到"）。
            Some(rows) if rows.is_empty() => "holdings = 0\n（没有约下的时刻）".to_string(),
            Some(rows) => {
                // **闹钟那一半与从前逐字节相同**：`pending` 在前、按原定时刻升序（最快到的排最前），
                // `fired` 在后、按原定时刻降序（最近响过的排最前）。
                let mut pending: Vec<(&str, HoldingState, u64, &str)> = Vec::new();
                let mut fired: Vec<(&str, HoldingState, u64, &str)> = Vec::new();
                // **起的东西那一半**：按起它的时刻**降序**（最近起的排最前——有用的那一头在近处），
                // 只在真有的时候才出现（没有它时，这一块的每一个字节都与从前一样）。
                let mut held: Vec<HeldCells<'_>> = Vec::new();
                for row in rows {
                    match row {
                        HoldingRow::Timer {
                            id,
                            state,
                            when_millis,
                            label,
                        } => match state {
                            HoldingState::Pending => {
                                pending.push((id, *state, *when_millis, label));
                            }
                            HoldingState::Fired => fired.push((id, *state, *when_millis, label)),
                        },
                        HoldingRow::Held {
                            name,
                            state,
                            started_millis,
                            command,
                            log,
                            bytes,
                        } => held.push((name, *state, *started_millis, command, log, *bytes)),
                    }
                }
                pending.sort_by_key(|each| each.2);
                fired.sort_by_key(|each| std::cmp::Reverse(each.2));
                held.sort_by_key(|each| std::cmp::Reverse(each.2));

                let mut listed = String::new();
                let mut shown = 0usize;
                for (id, state, when_millis, label) in pending.iter().take(MAX_RENDERED) {
                    listed.push_str(&self.timer_line(id, *state, *when_millis, label));
                    shown += 1;
                }
                let pending_folded = pending.len() - shown;
                let mut fired_shown = 0usize;
                for (id, state, when_millis, label) in
                    fired.iter().take(MAX_RENDERED.saturating_sub(shown))
                {
                    listed.push_str(&self.timer_line(id, *state, *when_millis, label));
                    fired_shown += 1;
                }
                let fired_folded = fired.len() - fired_shown;
                if pending_folded + fired_folded > 0 {
                    listed.push_str(&format!(
                        "（还有 {} 项未列出：pending {} / fired {}）\n",
                        pending_folded + fired_folded,
                        pending_folded,
                        fired_folded
                    ));
                }
                // 起的东西那一半：**空的那些也照样报**（"起过几件、停过几件"是读数）。
                let held_shown = held
                    .len()
                    .min(MAX_RENDERED.saturating_sub(shown + fired_shown));
                for (name, state, started_millis, command, log, bytes) in
                    held.iter().take(held_shown)
                {
                    listed.push_str(&self.held_line(
                        name,
                        *state,
                        *started_millis,
                        command,
                        log,
                        *bytes,
                    ));
                }
                let held_folded = held.len() - held_shown;
                if held_folded > 0 {
                    listed.push_str(&format!("（还有 {held_folded} 件起过的东西未列出）\n"));
                }
                // **账单**：这一块要递出去的字节数（与纸条板、格位块同形）。持有物是主体自己
                // 约的、自己起的，因此这些数主体**改得动**——停几件、取消几条，下一场这里就小一截。
                // ⚠ 没有"起的东西"时这一行与从前**逐字节相同**（既有消费者一个字都不用改）。
                let billing = if held.is_empty() {
                    format!(
                        "holdings = {} (pending {} / fired {}), ~{}B\n",
                        rows.len(),
                        pending.len(),
                        fired.len(),
                        listed.len()
                    )
                } else {
                    format!(
                        "holdings = {} (pending {} / fired {} / held {}), ~{}B\n",
                        rows.len(),
                        pending.len(),
                        fired.len(),
                        held.len(),
                        listed.len()
                    )
                };
                format!("{billing}{listed}")
            }
        }
    }

    /// 一行闹钟的机械读数：身份 / 状态 / 原定时刻 / 年龄 / 标签。
    ///
    /// `when` 走**与锚同一个刻度**（unix 毫秒），因此主体可以把它原样递给 `timer_set(when=…)`；
    /// 年龄那一格是**读数不是判断**——`overdue` 只说"过了多久还没响"，不说"你该处理它了"。
    fn timer_line(&self, id: &str, state: HoldingState, when_millis: u64, label: &str) -> String {
        format!(
            "{} | {} | when={} | {} | {}\n",
            id,
            state.as_str(),
            when_millis,
            age_field(state, when_millis, self.now_millis),
            preview(label)
        )
    }

    /// 一行"起过的东西"的机械读数：名字 / 状态 / 起的时刻 / 年龄 / 输出落在哪儿 / 当时那条命令。
    ///
    /// 年龄那一格同样是**读数不是判断**：`started-ago` 只说"起了多久"，不说"你该不该停它"。
    fn held_line(
        &self,
        name: &str,
        state: HeldState,
        started_millis: u64,
        command: &str,
        log: &str,
        bytes: Option<u64>,
    ) -> String {
        let size = match bytes {
            Some(bytes) => format!("{bytes}B"),
            // 读不到 ⇒ 明说读不到（**不写 0**：0 是一个真实的读数）。
            None => "bytes=unknown".to_string(),
        };
        format!(
            "{} | {} | started={} | {} | output={} ({}) | {}\n",
            name,
            state.as_str(),
            started_millis,
            held_age_field(started_millis, self.now_millis),
            log,
            size,
            preview(command)
        )
    }

    /// 此刻的内容块（标签是本件的身份）。
    pub fn line(&self) -> ContentBlock {
        ContentBlock::new(HOLDINGS_LABEL, self.reading())
    }

    fn own_batch(&self) -> ContentBatch {
        ContentBatch::of_or_not_delivered(vec![self.line()], HOLDINGS_LABEL)
    }

    /// 内层来源（只读）。
    pub fn inner(&self) -> &Inner {
        &self.inner
    }
}

impl<Inner: ContextSource> ContextSource for Holdings<Inner> {
    /// **开场一次**：与内层的开场事实一起递交。快照不是活状态，之后不重摆。
    fn on_run_start(&mut self, context: &InjectionContext) -> ContentBatch {
        combine(self.inner.on_run_start(context), self.own_batch())
    }

    /// 这里不产生持有物块（快照只在开场摆一次）。内层照旧。
    fn before_turn(&mut self, context: &InjectionContext) -> ContentBatch {
        self.inner.before_turn(context)
    }
}

/// "起过的东西"那一支在渲染时的中间量：名字 / 状态 / 起的时刻 / 当时那条命令 / 输出路径 / 字节数。
type HeldCells<'a> = (&'a str, HeldState, u64, &'a str, &'a str, Option<u64>);

/// 年龄读数：三种处境各说各的，**不许压成一格**。
///
/// - `pending` 且还没到点 ⇒ `due-in`（还有多久）；
/// - `pending` 却已经过了 ⇒ `overdue`（**这一次没响成**，正是"崩在收掉与开活动之间"留下的形状）；
/// - `fired` ⇒ `fired-ago`（响过多久了）；
/// - 钟读不出来（`None`）⇒ `age=unknown`——**四种**，不把"不知道现在几点"折进上面任何一格。
fn age_field(state: HoldingState, when_millis: u64, now_millis: Option<u64>) -> String {
    let Some(now_millis) = now_millis else {
        return "age=unknown".to_string();
    };
    match state {
        HoldingState::Pending if when_millis > now_millis => {
            format!("due-in={}s", (when_millis - now_millis) / 1000)
        }
        HoldingState::Pending => format!("overdue={}s", (now_millis - when_millis) / 1000),
        HoldingState::Fired => format!(
            "fired-ago={}s",
            now_millis.saturating_sub(when_millis) / 1000
        ),
    }
}

/// "起过的东西"那一支的年龄读数：**一种处境一个词**，与闹钟那一支的词不共用。
///
/// 钟读不出来（`None`）⇒ `age=unknown`——**不把"不知道现在几点"折进一个数里**。
fn held_age_field(started_millis: u64, now_millis: Option<u64>) -> String {
    let Some(now_millis) = now_millis else {
        return "age=unknown".to_string();
    };
    format!(
        "started-ago={}s",
        now_millis.saturating_sub(started_millis) / 1000
    )
}

/// 预览：**按字节**截断到 [`PREVIEW_BYTES`]，但**不切开一个字符**；截过就加 `…`。
///
/// 换行与控制字符在预览里换成空格：预览是"一行"，多一行会伪造出另一条持有物。
fn preview(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    if cleaned.len() <= PREVIEW_BYTES {
        return cleaned;
    }
    let mut cut = PREVIEW_BYTES;
    while cut > 0 && !cleaned.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &cleaned[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity_origin::ActivityOrigin;

    fn row(id: &str, when_millis: u64, state: HoldingState, label: &str) -> HoldingRow {
        HoldingRow::Timer {
            id: id.to_string(),
            state,
            when_millis,
            label: label.to_string(),
        }
    }

    /// 一件"起过的东西"的素材（终端 App 那一支交上来的形状）。
    fn held(name: &str, started_millis: u64, state: HeldState) -> HoldingRow {
        HoldingRow::Held {
            name: name.to_string(),
            state,
            started_millis,
            command: format!("run {name}"),
            log: format!("logs/{name}.out"),
            bytes: Some(12),
        }
    }

    fn reading_of(rows: &[HoldingRow], now_millis: u64) -> String {
        Holdings::new(Some(rows), Some(now_millis), ActivityOrigin::new()).reading()
    }

    /// **读失败与"一条也没有"是两件事**（缺省不等于零）。
    #[test]
    fn the_second_member_is_purely_additive_and_never_touches_the_timer_half() {
        // 没有"起过的东西"时：**每一个字节都与从前相同**（既有消费者一个字都不用改）。
        let timers = vec![
            row("t-1", 500, HoldingState::Fired, "早响的"),
            row("t-2", 9_000, HoldingState::Pending, "晚到的"),
        ];
        let without = reading_of(&timers, 1_000);
        assert!(
            without.starts_with("holdings = 2 (pending 1 / fired 1)"),
            "{without}"
        );
        assert!(
            !without.contains("held"),
            "没有第二支成员时不该多出那一格：{without}"
        );

        // 有"起过的东西"时：账单那一行多一格 `held`，并且**多出它自己那几行**。
        let mut with = timers.clone();
        with.push(held("server", 900, HeldState::Running));
        let text = reading_of(&with, 1_000);
        assert!(
            text.starts_with("holdings = 3 (pending 1 / fired 1 / held 1)"),
            "{text}"
        );
        assert!(
            text.contains("server | running | started=900 | started-ago=0s"),
            "起过的东西那一行用自己的词（`running` / `started-ago`）：{text}"
        );
        assert!(
            text.contains("output=logs/server.out (12B)"),
            "输出落在哪儿要摆出来（主体读得到那一份）：{text}"
        );
        // 闹钟那两行**照旧**（第二支成员不碰第一支的摆法）。
        assert!(text.contains("t-2 | pending | when=9000"), "{text}");
        assert!(text.contains("t-1 | fired | when=500"), "{text}");
    }

    #[test]
    fn held_things_are_listed_newest_first_and_the_three_states_stay_apart() {
        let rows = vec![
            held("老的", 100, HeldState::Gone),
            held("新的", 900, HeldState::Unknown),
            held("中间的", 500, HeldState::Running),
        ];
        let text = reading_of(&rows, 1_000);
        let newest = text.find("新的").expect("新的那一行要在");
        let middle = text.find("中间的").expect("中间那一行要在");
        let oldest = text.find("老的").expect("老的那一行要在");
        assert!(
            newest < middle && middle < oldest,
            "最近起的排最前（有用的那一头在近处）：{text}"
        );
        // 三态**不许压成一格**：`gone`（不在了）与 `unknown`（读不到）是两个词。
        assert!(text.contains("| gone |"), "{text}");
        assert!(text.contains("| unknown |"), "{text}");
        assert!(text.contains("| running |"), "{text}");
    }

    /// **读失败与"一条也没有"是两件事**（缺省不等于零）。
    #[test]
    fn a_failed_read_is_unknown_and_never_pretends_nothing_was_ever_set() {
        let board: Holdings<ActivityOrigin> = Holdings::new(None, Some(0), ActivityOrigin::new());
        assert_eq!(board.reading(), format!("holdings = {UNKNOWN_HOLDINGS}"));
    }

    /// **钟读不出来时不编一个数**：年龄那一格如实写 `unknown`，其余字段照旧。
    #[test]
    fn an_unreadable_clock_never_fabricates_an_age() {
        let rows = vec![row("t-1", 1_000, HoldingState::Fired, "标签还在")];
        let board: Holdings<ActivityOrigin> =
            Holdings::new(Some(&rows), None, ActivityOrigin::new());
        let text = board.reading();
        assert!(text.contains("age=unknown"), "{text}");
        assert!(text.contains("when=1000"), "{text}");
        assert!(text.contains("标签还在"), "{text}");
    }

    #[test]
    fn an_empty_table_says_zero_out_loud() {
        let text = reading_of(&[], 1_000);
        assert!(text.starts_with("holdings = 0"), "{text}");
        assert!(text.contains("没有约下的时刻"), "{text}");
    }

    /// **`pending` 排在 `fired` 前面**，而且各自按"最有用的那一头"排。
    #[test]
    fn pending_comes_first_soonest_first_and_fired_follows_most_recent_first() {
        let rows = vec![
            row("t-1", 500, HoldingState::Fired, "早响的"),
            row("t-2", 9_000, HoldingState::Pending, "晚到的"),
            row("t-3", 900, HoldingState::Fired, "晚响的"),
            row("t-4", 2_000, HoldingState::Pending, "快到的"),
        ];
        let text = reading_of(&rows, 1_000);
        let order: Vec<&str> = text
            .lines()
            .filter(|line| line.starts_with("t-"))
            .map(|line| line.split(" | ").next().expect("首格是身份"))
            .collect();
        assert_eq!(order, vec!["t-4", "t-2", "t-3", "t-1"], "{text}");
        assert!(
            text.starts_with("holdings = 4 (pending 2 / fired 2)"),
            "{text}"
        );
    }

    /// **三种年龄各说各的**：还没到 / 已过期 / 响过了——尤其 `overdue` 不许被折进别的说法。
    #[test]
    fn the_three_ages_are_three_distinct_readings() {
        let rows = vec![
            row("t-1", 5_000, HoldingState::Pending, ""),
            row("t-2", 1_000, HoldingState::Pending, ""),
            row("t-3", 1_000, HoldingState::Fired, ""),
        ];
        let text = reading_of(&rows, 3_000);
        assert!(text.contains("due-in=2s"), "{text}");
        assert!(text.contains("overdue=2s"), "{text}");
        assert!(text.contains("fired-ago=2s"), "{text}");
    }

    /// **已过期却还没响过的照原样列出**：它正是"这一次没响成"的读数，比任何 `fired` 都重要。
    #[test]
    fn an_overdue_pending_row_is_never_folded_away_by_fired_rows() {
        let mut rows: Vec<HoldingRow> = (0..MAX_RENDERED * 2)
            .map(|n| {
                row(
                    &format!("f-{n}"),
                    10_000 - n as u64,
                    HoldingState::Fired,
                    "",
                )
            })
            .collect();
        rows.push(row("overdue-one", 1, HoldingState::Pending, "没响成"));
        // 年龄按**整秒**报（`(now - when) / 1000`），因此这里取一个整秒的差：100_001 - 1 = 100s。
        let text = reading_of(&rows, 100_001);
        assert!(
            text.starts_with("holdings = 41 (pending 1 / fired 40)"),
            "{text}"
        );
        assert!(text.contains("overdue-one"), "{text}");
        assert!(text.contains("overdue=100s"), "{text}");
    }

    /// **折叠如实报**，而且 pending / fired 分开报——不把两件事压成一个数。
    #[test]
    fn folding_is_reported_honestly_and_split_by_state() {
        let mut rows: Vec<HoldingRow> = (0..MAX_RENDERED + 3)
            .map(|n| row(&format!("p-{n}"), 100 + n as u64, HoldingState::Pending, ""))
            .collect();
        rows.extend((0..5).map(|n| row(&format!("f-{n}"), 50 + n as u64, HoldingState::Fired, "")));
        let text = reading_of(&rows, 0);
        // pending 占满上限 ⇒ fired 一条都没列，余数分别报出。
        assert!(
            text.contains(&format!("（还有 {} 项未列出：pending 3 / fired 5）", 8)),
            "{text}"
        );
    }

    /// 标签预览按字节截断、不切开字符，并且**不伪造第二行**。
    #[test]
    fn a_long_label_is_clipped_without_splitting_a_character_or_faking_a_second_row() {
        let rows = vec![row("t-1", 1_000, HoldingState::Fired, &"字".repeat(100))];
        let text = reading_of(&rows, 2_000);
        let rows_listed: Vec<&str> = text.lines().filter(|l| l.starts_with("t-")).collect();
        assert_eq!(rows_listed.len(), 1, "{text}");
        assert!(rows_listed[0].contains('…'), "{text}");
        // 60 字节 = 20 个汉字，加一个 `…`。
        assert!(
            rows_listed[0].ends_with(&format!("{}…", "字".repeat(20))),
            "{text}"
        );
    }

    /// 换行被压成空格：预览是"一行"，多一行会伪造出另一条持有物。
    #[test]
    fn a_label_with_a_newline_never_becomes_a_second_row() {
        let rows = vec![row("t-1", 1_000, HoldingState::Fired, "上\n下")];
        let text = reading_of(&rows, 2_000);
        assert_eq!(
            text.lines().filter(|l| l.starts_with("t-")).count(),
            1,
            "{text}"
        );
        assert!(text.contains("上 下"), "{text}");
    }

    /// **块级诚实回退**：这一块自己超限时留下一句读数，**不退空批次**（与纸条板同律）。
    #[test]
    fn an_oversize_block_leaves_a_reading_instead_of_vanishing() {
        let rows = vec![row(&"t".repeat(70_000), 1_000, HoldingState::Fired, "")];
        let board = Holdings::new(Some(&rows), Some(2_000), ActivityOrigin::new());
        let expected = HOLDINGS_LABEL.len() + board.reading().len();
        assert!(expected > ContentBatch::MAX_BYTES, "前提：这一块真的超限");

        let batch = board.own_batch();
        assert_eq!(batch.blocks().len(), 1, "留下一块读数，不是空批次");
        assert_eq!(batch.blocks()[0].label(), HOLDINGS_LABEL);
        assert_eq!(
            batch.blocks()[0].text(),
            format!("超限未给（原始 {expected} B）")
        );
        assert!(!batch.is_source_failure(), "它不是来源失败");
    }
}
