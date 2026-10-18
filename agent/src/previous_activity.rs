//! **上一段怎么了**：开场那一轮如实告诉主体"上一段没有正常收场"。
//!
//! ## 为什么需要它
//!
//! 一段清醒被一段不清醒隔开，就是两段：承载活动的进程没了，那一次连续就断了。
//! 新进程起来是**新的一段**——而**新进程的模型什么都没见过**。
//!
//! 它自己收的场它知道（`subject:*`），runtime 兜底收的场（`system:*`）它也知道——
//! **只有"根本没有收到场"这件事，它无从得知**：它睁开眼，看见的和一次普通唤醒一模一样。
//!
//! ⚠️ **它自己能查，但它查不出"有没有事"。** 经历就在它自己家里、`terminal_run` 是整机全权——
//! **"知道了有事之后怎么查"，它做得到**；而"**该不该去看一眼**"这条线索，只有世界能给。所以这一块
//! **不是"替它查"，是告诉它有事可查**。
//!
//! ⚠️ **指路只能指到它能走的路**（2026-10-06 修）：这句话从前写的是"你自己家里那份"＋一个
//! **内部存储文件名**——而视图**刻意不给**那几个名字（`the_view_never_exposes_environment_internals`
//! 钉着：身份记录与两份存储在视图里一律 `not-found`）。也就是说那句话**照着做会撞墙**：模型拿到的
//! 不是"去哪里看"，而是一个读不到的名字——那正是"**呈现事实，而不是制造迷惑**"要挡的东西。
//! 现在指到**逻辑地址** `history/`（派生经历目录，按活动分目录、按段读，走同一套
//! `files_list` / `files_read`）。
//!
//! （本件是中立层：那几个内部名字属于存储那一层的实现细节，**这里连写都不写**——短词守卫
//! 照旧生效，见 `tests/minimal_agent.rs` 里那条词表用例。）
//!
//! ## 边界（三条，都是刻意的）
//!
//! - **是信息，不是指令**：只说"没有正常收场、原因没有被记录下来"，**一个"该"字都没有**。
//!   它是独立的个体，不是"别人让它干嘛它就干嘛"——世界给的是**信息**，决定权在它。
//! - **不冒充原因**：崩溃、被强杀、断电都留下同一个形状，本件**分不出是哪一种**，
//!   因此**不写"崩溃了"**（那是替它认领一个我们不知道的结论）。
//! - **缺席不落记录**：正常收场时**一个字节都不写**（这一块整个不出场）——上下文是宝贵的，
//!   而"上一段正常结束了"它本来就知道（自己收的场 / runtime 兜底收的场）。
//!   ⇒ 正常情形**零上下文成本**，异常情形**一句话**。
//! - ⚠ **措辞是「有一段」，不是「上一段」（2026-09-26 真机更正）**：没有收尾的那一段**永远
//!   不会被补上收尾**，因此这个事实是**长期的**——只要它还在，**之后每一段**新开场都会带上
//!   同一张纸条（真机实测：run-4 没走完，之后的 -1、-2 与 timer 那一段 -3 **都**带着 run-4
//!   那张）。而对 -3 来说，它的"上一段"（-2）是**正常收场**的——**写「上一段」就是假话**。
//!   ⇒ 只报**事实**（"有一段没正常收场，是哪一段"），不报**相邻关系**（"就是刚才那一段"）。

use crate::clock::{LocalOffset, format_local};
use crate::core::{ContentBatch, ContentBlock, ContextSource, InjectionContext, combine};
use crate::foundation::RunId;
use crate::recovery::StoppedInPlace;
use crate::store::StorageFailure;

/// 这一块的标签（与 `status-bar` / `holdings` / `activity-origin` 同族）。
pub const PREVIOUS_ACTIVITY_LABEL: &str = "previous-activity";

/// **这一块最多列几个未结算的操作**（照 `holdings` / `pins_board` 的先例：有上限、超量如实报数）。
///
/// 它不是"截断"：超出的部分**报数**（"还有 N 项未列出"），因此"没列出来"与"没有"分得开。
pub const MAX_UNSETTLED: usize = 5;

/// **这一块最多列几段没走完的**（照 `holdings` / `pins_board` 的先例：有上限、超量如实报数）。
pub const MAX_UNFINISHED: usize = 5;

/// **一个未结算的操作**：选过、发出去了，而**没有看到结果**。
///
/// ## 它是什么、不是什么（2026-10-06 主人裁决，第一性那条）
///
/// 终局要的五格里有一格叫 `target`（"这个操作的目标是什么"）。**本件【不产出】它**——
/// 判据是主体性：**「哪个参数算目标」是【解释】，而解释归主体**。系统给的是**机械事实**
/// （哪一件、停在哪一步、什么时候）＋ **原文在哪**（`history/`，那次选择的参数
/// **逐条原样**在那里，走同一套 `files_list` / `files_read`）。
///
/// ⚠ **为什么不在开场把参数再摊一遍**（不是因为"私密"，那个说法把系统说成了替主体挡东西的一方）：
/// **原文主体已经能看到。在开场再摊一遍，不是"给它看"，是"替它挑"。**
///
/// ⇒ 所以这里**只有机械事实**：哪一轮、那一轮的第几个、能力名、什么时候。**参数一个都不出现。**
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsettledOperation {
    /// 它在哪一轮。
    pub turn: u32,
    /// 那一轮的第几个动作（与经历侧 `action-selected` 的配对键同形）。
    pub index: u16,
    /// 当时选的是哪个能力。**只在经历侧有** ⇒ 读不到就是 `None`（不编一个）。
    pub capability: Option<String>,
    /// **这一行是什么时候写的**（unix 毫秒）。没有被记 ⇒ `None`。
    pub at_millis: Option<u64>,
}

/// 开场纸条要说的**事实**：各自缺席 ⇒ 各自不出现；全都缺席 ⇒ **整块不出场**（缺席不落记录）。
///
/// 三块事实的主语不同，因此分三段而不是合成一句：一段说"有一段没走完"，一段说
/// "存储出过故障、那一次停在哪"，一段说"最近那一段里有哪几件没结算"。它们都是**已经落在
/// 账本里的事实**——只是从前**只有操作者的命令读得到**（2026-10-06 起开场也带给主体）。
#[derive(Clone, Debug, Default)]
pub struct PreviousReading {
    /// 有一段没有正常收场（没有收尾事实）。
    ///
    /// 它是**最近那一段**（准入读数给的那一个），也是下面"未结算的操作"覆盖的那一段。
    pub unfinished: Option<RunId>,
    /// **全部**没有正常收场的段（按**序号**排 = 真实的先后）。
    ///
    /// ## 为什么要有它（2026-10-06 主人裁决）
    ///
    /// 准入读数只给**一个** id ⇒ 从前"更早还有没有没走完的"**报不出来**（**会漏**）。
    /// 而"没走完"这件事是**长期的**（它永远不会被补上收尾）⇒ 漏掉的那些会**一直**漏。
    ///
    /// ⚠ **为什么按序号排，不按活动 id**：`run-12` 按 id 排在 `run-4` **前面**——那不是先后。
    /// 序号（[`crate::store::RunSummary::first_seq`]）是**写入顺序**：机械，而且真实。
    /// ⇒ 材料里那条"跨 Run 的机械顺序没有裁决"**就此有了**（不评重要性、不排优先级）。
    ///
    /// ⚠ **它只覆盖"哪几段"**：每一段里**哪几件**没结算，仍然只算**最近那一段**
    /// （逐段算要逐段读那一卷，那是 N 次读）——渲染时如实说清这个作用域。
    pub unfinished_all: Vec<RunId>,
    /// 历史上最后一次存储故障（那一段写不下去时留下的记录）。
    pub storage_failure: Option<StorageFailure>,
    /// **最近那一段里未结算的操作**（已发出、没有结果记录）。
    ///
    /// ⚠ **只覆盖最近那一段**（今天系统手里只有一个"最近那段没走完"的 id）⇒ 渲染时**必须
    /// 如实说"更早的段读不到"**，不能让主体以为只有这一段。跨活动枚举是另一件事。
    pub unsettled: Vec<UnsettledOperation>,
    /// 上面那份清单**读不出来**时的稳定标记；`None` = 读出来了。
    ///
    /// ⚠ **"读不到"与"没有"必须分开说**（这是本仓库一贯的那条）：清单为空 **且** 这一格是
    /// `Some` ⇒ 我们**不知道**有没有；清单为空 **且** 这一格是 `None` ⇒ **确实没有**。
    pub unsettled_unreadable: Option<String>,
    /// **最近那一段是主体自己搁下的**（`subject:held`）⇒ `Some(那一段)`。
    ///
    /// ⚠ **它与"没有正常收场"是两件事，而且互斥**（2026-10-10 裁决三）：搁下的那一段**有**收尾
    /// 事实，没走完的那一段**没有**。下一段只能看到这张纸条 ⇒ 把"主体自己搁下的"读成"断了"
    /// 就是**拿假话吓人**（反过来也一样：把断了读成"你搁下的"就是替主体认领一个它没做过的决定）。
    ///
    /// ⚠ **2026-10-13：这一格今天恒为 `None`**——`subject:held` 自 §3.66（搁下不再收场）起
    /// **线缆上再也不产出**，它只服务旧账。**"搁下之后断了"这一档由下面那格 [`Self::stopped`] 接**。
    pub held: Option<RunId>,
    /// **最近那一段后来断了，而它是主体自己停在这儿的**（2026-10-13，**账上读出来的**）。
    ///
    /// ⚠ **它与 [`Self::held`] 不是同一格**：`held` 读的是**收尾事实**（那个值今天不再产出），
    /// 这一格读的是**经历侧**——那一段**最后一条答复**是不是"停在这儿"。两格覆盖同一件事的
    /// 两个时代：有收尾事实的那个时代，与"搁下不再收场"之后的这个时代。
    ///
    /// 三态各自分开（本仓库一贯那条）：`None` = 这一格不适用（那一段有收场事实、或根本没有那一段）
    /// ⇒ **一次账都不读**；[`StoppedInPlace::No`] = 读出来了、**不是**停在这儿 ⇒ 照旧说
    /// "没有正常收场"；[`StoppedInPlace::Unreadable`] = **不声称**是哪一种。
    pub stopped: Option<StoppedInPlace>,
}

impl PreviousReading {
    /// **"没走完"那几段**：一段与多段**分开说**——两种都是事实，措辞各自准确。
    ///
    /// ⚠ **不许把多段压成"有一段"**：那会让主体以为只有一段（**那是假话**），
    /// 而"更早那些"正是最容易被漏掉的一格。
    ///
    /// ⚠ **名单不剔人**（2026-10-13）："主体自己停下的"那一段**也在这里面**——它**确实没有收尾
    /// 事实**，因此"没有正常收场"这句话对它是**真的**。要说的那一层另有一句限定
    /// （见 [`Self::stopped_text`]），**不靠把它从名单里拿掉来暗示**：剔掉它会让总数变成假的。
    fn unfinished_text(&self, latest: &RunId) -> String {
        let all = &self.unfinished_all;
        if all.len() <= 1 {
            return format!("有一段（{}）没有正常收场。\n", latest.as_str());
        }
        let listed: Vec<&str> = all
            .iter()
            .take(MAX_UNFINISHED)
            .map(|run| run.as_str())
            .collect();
        let mut text = format!("有 {} 段没有正常收场：{}。\n", all.len(), listed.join("、"));
        if all.len() > MAX_UNFINISHED {
            // **报数，不截断**：没列出来的说清还有几段。
            text.push_str(&format!("还有 {} 段未列出。\n", all.len() - MAX_UNFINISHED));
        }
        text
    }

    /// **"那一段是主体自己停下的"那一句限定**（2026-10-13）：紧接着"没有正常收场"说。
    ///
    /// 三态各有各的话，**一句都不省**：
    ///
    /// - **点名**（`hold_run`）与**纯正文**分成两种措辞：那是**账上读得出来的差别**（点名那一次
    ///   多一条工具调用记录），而用一句话盖住两种情形，会让主体**以为自己点过名**——那就是替它
    ///   认领一个它没做过的动作；
    /// - **读不出来**就如实说读不出来（**不声称**是哪一种）——那是本仓库一贯那条三态。
    ///
    /// ⚠ 措辞纪律与整块同一条：**一个"该"字都没有**、也不劝主体接着干；**不带 `**`**（那是标记
    /// 语法，主体读到的是两个星号）。
    fn stopped_text(&self, stopped: &StoppedInPlace) -> String {
        let how = match stopped {
            StoppedInPlace::Named => "你自己点名停下的（`hold_run`）",
            StoppedInPlace::Silent => "你自己停下的（那一次你只回了正文，没有点名）",
            // `No` 走不到这里（调用方先看它）——真到了这里就**只说共同的那一半**，不编一个来源。
            StoppedInPlace::No => "你自己停下的",
            // **读不出来就说读不出来**：三态里这一档**不声称**是哪一种（本仓库一贯那条）。
            StoppedInPlace::Unreadable(reason) => {
                return format!("它是不是你自己停下的：读不出来（{reason}）。\n");
            }
        };
        format!("它是{how}：此后这一段活动就断了——不是干完了，也不是你收的场。\n")
    }

    /// **"搁下"那一段**（2026-10-10 裁决三）：主体自己停在那儿，不是断了。
    ///
    /// ⚠ **措辞只说机械事实，不声称是哪个动作产生的**：`subject:held` 有**两个来源**——
    /// 主体点名（`hold_run`）与主体交回控制（纯正文，宿主据此把这一段停在"搁下"上）。
    /// 两者在这一格长得一样（都是"主体停在这儿"），**要分辨就去账上看**（点名的那一次有一条
    /// 工具调用记录）。⚠ 因此这里**不写 `hold_run`**——对纯正文那一次，那句话是假的。
    ///
    /// ⚠ 措辞纪律与整块同一条：**一个"该"字都没有**，也不劝主体接着干——它说的是"上一段是怎么
    /// 结束的"，不是"你该去把它捡起来"。
    fn held_text(&self, run: &RunId) -> String {
        format!(
            "有一段（{}）是搁下的：不是断了，也不是干完了——它停在那儿，等有事再叫你。\n\
             它走到哪一步记在 `history/` 里：按活动分目录，用 `files_list` / `files_read` 读。\n",
            run.as_str()
        )
    }

    /// **未结算操作那一段**（独立成函数 ⇒ 上限与"读不到/没有"两条判据可以单独验）。
    ///
    /// ⚠ 措辞纪律（与整块同一条）：**一个"该"字都没有**、不排序、不评重要性、不给建议。
    /// 它说的是"哪几件没有结果记录"，不是"哪几件重要"。
    fn unsettled_text(&self) -> String {
        if self.unsettled.is_empty() && self.unsettled_unreadable.is_none() {
            return String::new();
        }
        let mut text = String::new();
        if let Some(reason) = &self.unsettled_unreadable {
            // **读不到就说读不到**，不说"没有"。
            text.push_str(&format!(
                "最近那一段有没有未结算的操作：读不出来（{reason}）。\n"
            ));
            return text;
        }
        text.push_str("最近那一段里未结算的操作（选过、发出去了，而没有看到结果）：\n");
        for each in self.unsettled.iter().take(MAX_UNSETTLED) {
            let capability = each.capability.as_deref().unwrap_or("unknown");
            let when = each
                .at_millis
                .map(|at| {
                    // 这是**主体自己这一侧的记录**（那一刻的毫秒）⇒ 也按**那一刻**的本机偏移摆出来，
                    // 与状态行那一格同一个刻度（否则主体手里又会同时有两种形态）。
                    let instant =
                        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(at);
                    format_local(instant, LocalOffset::at_machine(instant))
                })
                .unwrap_or_else(|| "unknown".to_string());
            text.push_str(&format!(
                "- turn {} index {}：{}（已发出，没有结果记录；{}）\n",
                each.turn, each.index, capability, when
            ));
        }
        if self.unsettled.len() > MAX_UNSETTLED {
            // **报数，不截断**：没列出来的那几项说清有几项。
            text.push_str(&format!(
                "还有 {} 项未列出。\n",
                self.unsettled.len() - MAX_UNSETTLED
            ));
        }
        // ⚠ 作用域如实登记：这一格只覆盖最近那一段。
        text.push_str("（只覆盖最近那一段；更早的段有没有未结算，这一格读不到。）\n");
        text
    }
}

/// 开场那一块：上一段没有正常收场、或存储出过故障时，如实说一句。
///
/// 两个事实都缺席 ⇒ **整块不出场**。
pub struct PreviousActivity<Inner> {
    inner: Inner,
    reading: PreviousReading,
}

impl<Inner> PreviousActivity<Inner> {
    /// 用**内层来源**与这两条事实装配。
    pub fn new(inner: Inner, reading: PreviousReading) -> Self {
        Self { inner, reading }
    }

    /// 内层来源（只读）。
    pub fn inner(&self) -> &Inner {
        &self.inner
    }

    /// 这一块自己的那一批。**两条事实都没有时是空批次**（缺席不落记录）。
    fn own_batch(&self) -> ContentBatch {
        let mut text = String::new();
        if let Some(run) = &self.reading.unfinished {
            text.push_str(&self.reading.unfinished_text(run));
            text.push_str(
                "原因没有被记录下来 —— 崩溃、被强杀、断电都会留下这个形状，分不出是哪一种。\n\
                 它走到哪一步记在 `history/` 里：按活动分目录，用 `files_list` / `files_read` 读。\n",
            );
            // **那一段是主体自己停下的吗**（2026-10-13）：紧接着上面那句说。上面那句**是真的**
            // （那一段确实没有收尾事实），这一句补上**真相的另一层**：主体**先停下来了**。
            // ⚠ **不靠"把它的名字从名单里拿掉"来暗示**——剔掉它会让总数变成假的（见
            // [`PreviousReading::unfinished_text`]）。
            if let Some(stopped) = self
                .reading
                .stopped
                .as_ref()
                .filter(|each| !matches!(each, StoppedInPlace::No))
            {
                text.push_str(&self.reading.stopped_text(stopped));
            }
        }
        // **主体自己搁下的那一段**（旧账那一档：收尾事实 `subject:held`，今天不再产出）。
        // 与上面几段**互斥**——它**有**收尾事实，而"没有正常收场"那几段根本没有。
        if let Some(run) = &self.reading.held {
            text.push_str(&self.reading.held_text(run));
        }
        if let Some(failure) = &self.reading.storage_failure {
            text.push_str(&format!(
                "存储出过一次故障：最近一次停在这一段（{}）。\n",
                failure.run.as_str()
            ));
            match &failure.reason {
                Some(reason) => text.push_str(&format!("原因是 {reason}。\n")),
                // 账本里只有"确认"、没有它确认的那条暂停 ⇒ 如实说没有记录，不编一个。
                None => text.push_str("原因没有被记录下来。\n"),
            }
            text.push_str("那是写不下去的时候留下的记录：系统不会静默继续，也不会重放那一次写。\n");
            if failure.acknowledged {
                text.push_str("那次异常已经被显式确认过一次。\n");
            }
        }
        // **未结算的操作**（2026-10-06）：终局要的四件里，这一件从前没有落点。
        text.push_str(&self.reading.unsettled_text());
        if text.is_empty() {
            return ContentBatch::empty();
        }
        ContentBatch::of_or_not_delivered(
            vec![ContentBlock::new(PREVIOUS_ACTIVITY_LABEL, text)],
            PREVIOUS_ACTIVITY_LABEL,
        )
    }
}

impl<Inner: ContextSource> ContextSource for PreviousActivity<Inner> {
    /// **开场一次**：与内层的开场事实一起递交（它是快照，之后不重摆）。
    fn on_run_start(&mut self, context: &InjectionContext) -> ContentBatch {
        combine(self.inner.on_run_start(context), self.own_batch())
    }

    /// 轮级照旧交给内层：这一件只在开场露面。
    fn before_turn(&mut self, context: &InjectionContext) -> ContentBatch {
        self.inner.before_turn(context)
    }
}
