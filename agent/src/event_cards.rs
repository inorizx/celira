//! 事件卡片：**Run 内动静的当轮可见**。
//!
//! 问题：主体正回话时世界又来一条，主体的**当轮**是瞎的（锚只保"下次醒来不漏"，保不了"本次看见"）。
//! 本件补的就是这一格：宿主在主体**跑着**的时候听到铃，就拉一次 B1（`since = 本次活动开始`）
//! 写进**细胞**（[`EventCardsReport`]，ContextReading 先例：宿主写、服务渲染），turn 边界把
//! **自上轮以来的增量**摆出来。
//!
//! ## 它是谁：系统服务层的第四个件
//!
//! 与状态栏、通知栏、纸条板同族：**Core / Foundation 不许认识它**——「本次 Run 里世界又动了什么」
//! 不是执行语义，Core 只认得那个中立接缝（`ContextSource`，它只看见"外层又给了一块"）。
//! **本件自己不读网络**：那次 B1 拉取是**宿主的动作**（铃回调，见 [`crate::host_loop::RunWatch`]），
//! 网络因此**不进 Core 的注入路径**——进 Core 的只有细胞里已经落下的计数。
//!
//! ## 与通知栏 / 锚的三方分工
//!
//! | 件 | 覆盖 | 形状 | 谁写 |
//! |---|---|---|---|
//! | 通知栏 | 本次醒来**之前**世界有什么新的 | 开场**快照**（分母 ＋ 有新的房逐行 ＋ 余数；零房**不点名**，TASK_73 起按事件定价） | 宿主读投影，开场摆一次 |
//! | **事件卡片** | **本次 Run 里**世界又动了什么 | Run 内**增量**（一条只播一次） | 宿主在 Run 中拉取写细胞 |
//! | 锚（`since`） | 只保**下次醒来不漏** | 一个时刻 | 宿主读一次、推进一次 |
//!
//! ## 四条纪律
//!
//! 1. **零正文**：卡片只有"哪间房 +N"（机械计数）。内容自己去读——与通知栏同一条律，
//!    也同一条负对照：回执里带着预览，卡片一个字节都不许有；
//! 2. **一条只播一次**：Run 级内存去重（已播的计数记在心里），**收场即散**（零持久）；
//!    事件是历史，重播即语义错误；
//! 3. **零动静不出块**：没有增量就**不出卡片**——与通知栏/组件区刻意相反：那两件是**快照**，
//!    空着也要说一声（缺省不等于零）；卡片是**增量**，静默就是它的默认；
//! 4. **永不让活动失败**：细胞读不到就留旧值，读不出增量就不出块（便利坏了不挡清醒）。
//!
//! ## 排序：机械的
//!
//! **增量多在前，同量按房名** ⇒ 同一份细胞**逐字节长成同一张卡**：顺序不随"先读到哪间房"变
//! （`tests/run_session.rs` 的 `the_same_cell_renders_the_same_card_bytes` 拿两份只差合并次序的
//! 细胞钉住它）。理由不是洁癖：顺序若变，同一件事在两次醒里就长得不一样，主体每次都得重新认这张卡。
//!
//! ## 延迟上限（如实登记）
//!
//! 铃驱动 ⇒ 卡片最早出现在**下一个 turn 边界**（已经发出去的那次请求不追溯）——上限一 turn。
//!
//! ## 零正文：由**逐字断言**守着（负对照是回执）
//!
//! `tests/run_session.rs` 的 `an_event_card_is_played_once_at_the_next_turn_and_never_again`
//! 把整块卡片**逐字**比成 `自上轮以来：\n值班室 +2\n清静间 +1`——多一个字节就红。⚠ **它真的会红**
//! （不是摆设）：往卡片里注入一段"顺手带上的正文" ⇒ **恰好**那两条形状用例红；还原后 sha1 一致、
//! 套件复绿。**负对照是回执**：回执带着预览，卡片**一个字节都不许有**。
//!
//! ## 真实读数（真 e2e：scratch im ＋ 常驻宿主 ＋ 真模型 ＋ 真门铃）
//!
//! 主体起跑后 **52 ms** 主人又发一条，那一趟活动里：`run-start` 那批**没有**卡片（开场通知栏说
//! 那一拍没有新的）⇒ `turn 2` 那批出现卡片（正文 `自上轮以来：` ＋ 一行 `值班室 +1`——**只算主体
//! 起跑之后**到达的那一条）⇒ 主体随即重读房间、翻记录，并在**同一轮里说了两句**（第一问 ＋ 第二问，
//! 含暗号）⇒ 各自复核 ⇒ 收场 `completed`。⇒ **同一次活动内**从卡片看见、自己读、两条都回应：
//! "回话时失明"这一格修掉了。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::core::combine;
use crate::core::{ContentBatch, ContentBlock, ContextSource, InjectionContext};

/// 事件卡片内容块的标签（呈现时的区域名）。
pub const EVENT_CARDS_LABEL: &str = "event-cards";

/// 事件卡片细胞：**宿主写、服务读**的 Run 级内存（零持久）。
///
/// 与 [`crate::context_reading::ContextReading`] 同族，但存的是"每间房自**本次活动开始**以来有几条新的"
/// ——宿主每次听到铃就拉一次 B1 覆盖式合并进来（后到的 pull 更全），因此**两铃自然合并成一张卡**。
///
/// | 问题 | 答案 |
/// |---|---|
/// | 权威所有者 | 运行时（本次活动内）：它只是"宿主最近一次读到的计数"，不参与任何判断 |
/// | 允许的写者 | **只有宿主**（它持有写柄 [`EventCardsWriter`]） |
/// | 生命周期 | **Run 级**：活动收场即散，跨活动/跨进程都不带走（零持久） |
/// | 失败后状态 | **保持旧值**（读失败不需要另设失效机制） |
#[derive(Clone, Debug, Default)]
pub struct EventCardsReport {
    seen: Arc<Mutex<BTreeMap<String, i64>>>,
    /// **到达过几次动静**（铃响一次记一次）与**渲染过几次**（卡片进过一次注入记一次）。
    ///
    /// 为什么要这两格（2026-10-13 按真机补）：主体"看见了吗"是**独立的物理事实**，不能由
    /// "这一段跑到哪了"推出来——细胞并进去了不等于摆到了眼前（细胞随那一段一起散掉）。
    /// 判据因此是两个机械计数：`rings > renders` ⇒ **有动静到达之后还没被渲染过**。
    marks: Arc<Mutex<(u64, u64)>>,
}

impl EventCardsReport {
    /// 新开一个空细胞，同时交出它的**唯一写柄**。
    pub fn channel() -> (EventCardsWriter, EventCardsReport) {
        let seen = Arc::new(Mutex::new(BTreeMap::new()));
        let marks = Arc::new(Mutex::new((0_u64, 0_u64)));
        (
            EventCardsWriter {
                seen: Arc::clone(&seen),
                marks: Arc::clone(&marks),
            },
            EventCardsReport { seen, marks },
        )
    }

    /// 此刻细胞里的计数（房名 → 自本次活动开始以来的条数）。
    pub fn counts(&self) -> BTreeMap<String, i64> {
        self.seen
            .lock()
            .expect("卡片细胞的锁只被短临界区持有")
            .clone()
    }

    /// **这一次渲染**（卡片进了某一次注入）：主体"被摆到眼前"的机械证据。
    pub fn mark_rendered(&self) {
        self.marks.lock().expect("标记的锁只被短临界区持有").1 += 1;
    }

    /// **有动静到达之后还没被渲染过吗**（`true` = 那一段收场时世界还有没摆到眼前的东西）。
    pub fn unshown(&self) -> bool {
        let (rings, renders) = *self.marks.lock().expect("标记的锁只被短临界区持有");
        rings > renders
    }
}

/// 事件卡片细胞的**写**侧：只有宿主持有它。
#[derive(Clone, Debug)]
pub struct EventCardsWriter {
    seen: Arc<Mutex<BTreeMap<String, i64>>>,
    marks: Arc<Mutex<(u64, u64)>>,
}

impl EventCardsWriter {
    /// 把一次 B1 拉取的结果**覆盖式**并进细胞（同房以后到的值为准：它的窗口是"自活动开始"）。
    ///
    /// 零计数也记下来：它与"没读到这间房"是两件事，覆盖式合并要的是"这条窗最全的读数"。
    /// **有动静到达之后还没被渲染过吗**（活动收场时由宿主问一次）。
    pub fn unshown(&self) -> bool {
        let (rings, renders) = *self.marks.lock().expect("标记的锁只被短临界区持有");
        rings > renders
    }

    /// **记一次到达**（铃响一次）：它只记机械事实，不判断轻重。
    pub fn ring(&self) {
        self.marks.lock().expect("标记的锁只被短临界区持有").0 += 1;
    }

    /// **新的一段活动开始**：两格一起归零（细胞本来就是 Run 级的，计数跟着它走）。
    pub fn reset_marks(&self) {
        *self.marks.lock().expect("标记的锁只被短临界区持有") = (0, 0);
    }

    pub fn merge(&self, rows: &[(String, i64)]) {
        let mut seen = self.seen.lock().expect("卡片细胞的锁只被短临界区持有");
        for (name, count) in rows {
            seen.insert(name.clone(), *count);
        }
    }
}

/// **事件卡片本体**：turn 边界的增量渲染件 + 内层来源。
///
/// 它记住"已经播过什么"（`played`），因此同一份增量**只播一次**；`before_turn` 里先读内层
/// （状态行照旧每轮重摆），再决定这一轮有没有卡片。
pub struct EventCards<Inner> {
    /// 细胞（`None` = 这次装配没接细胞：离线夹具与恢复路径可以没有它 ⇒ 永远不出卡片）。
    report: Option<EventCardsReport>,
    /// 已经播过的计数（Run 级内存；收场即散）。
    played: BTreeMap<String, i64>,
    inner: Inner,
}

impl<Inner> EventCards<Inner> {
    /// 用细胞（可缺）+ 内层来源装配。
    pub fn new(report: Option<EventCardsReport>, inner: Inner) -> Self {
        Self {
            report,
            played: BTreeMap::new(),
            inner,
        }
    }

    /// 这一轮该摆的那一块（没有增量 ⇒ 空批）。
    fn own_batch(&mut self) -> ContentBatch {
        let Some(report) = &self.report else {
            return ContentBatch::default();
        };
        let counts = report.counts();
        // **摆到眼前了**：这一轮确实渲染过（哪怕这一批是空的）——它是"看见了吗"的机械证据。
        report.mark_rendered();
        let mut increments: Vec<(String, i64)> = Vec::new();
        for (name, count) in &counts {
            let played = self.played.get(name).copied().unwrap_or(0);
            if *count > played {
                increments.push((name.clone(), count - played));
            }
        }
        // 播过的一律记成"现在的数"：同一份增量不会再播第二遍。
        for (name, count) in &counts {
            self.played.insert(name.clone(), *count);
        }
        if increments.is_empty() {
            return ContentBatch::default();
        }
        // 机械排序：先按增量多在前，再按房名（固定细胞下逐字节稳定）。
        increments.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        let mut text = String::from("自上轮以来：");
        for (name, delta) in increments {
            text.push('\n');
            text.push_str(&name);
            text.push_str(&format!(" +{delta}"));
        }
        // 装不下时留一条读数（不退空批次）：卡片没了与"这次没读到"必须分得开。
        ContentBatch::of_or_not_delivered(
            vec![ContentBlock::new(EVENT_CARDS_LABEL, text)],
            EVENT_CARDS_LABEL,
        )
    }
}

impl<Inner: ContextSource> ContextSource for EventCards<Inner> {
    /// **开场不摆卡片**：那是通知栏（快照）的活；本件只说"本次 Run 里的增量"。
    fn on_run_start(&mut self, context: &InjectionContext) -> ContentBatch {
        self.inner.on_run_start(context)
    }

    /// 每一轮：内层照旧，外加"自上轮以来的增量"（没有就不加）。
    fn before_turn(&mut self, context: &InjectionContext) -> ContentBatch {
        let inner = self.inner.before_turn(context);
        combine(inner, self.own_batch())
    }
}

#[cfg(test)]
mod tests {
    use super::EventCardsReport;

    /// **"主体看见了吗"是独立的机械事实**（2026-10-13 按真机补）。
    ///
    /// 真机缺陷的形状：一段活动开场之后来了一声铃（`ring`），而那段**死在渲染之前**
    /// （模型失败，第 1 轮就没了）⇒ 那次动静既没进它的卡片（细胞随它散掉）、也不在下一段的
    /// 窗口里（锚已推到收场时刻）⇒ 两个窗口都不落。判据因此不能是"并进细胞了吗"，
    /// 只能是"**渲染进过某一次注入吗**"。
    #[test]
    fn a_ring_that_never_reaches_an_injection_stays_unshown() {
        let (writer, report) = EventCardsReport::channel();
        writer.reset_marks();
        assert!(!writer.unshown(), "刚开场：没有未摆到眼前的动静");

        // 跑着的时候来了一声铃，而这一段死在渲染之前 ⇒ 还没摆到眼前。
        writer.ring();
        assert!(writer.unshown(), "到达过、没渲染过 ⇒ 未摆到眼前");

        // 渲染过一次（卡片进了一次注入）⇒ 摆到眼前了。
        report.mark_rendered();
        assert!(!writer.unshown(), "渲染过就不再是未摆到眼前");

        // 两声响、只渲染一次 ⇒ 还有没看见的。
        writer.ring();
        writer.ring();
        report.mark_rendered();
        assert!(writer.unshown(), "两声响、一次渲染 ⇒ 还有没看见的");

        // 新的一段开场：两格归零（细胞与计数同命，都是 Run 级）。
        writer.reset_marks();
        assert!(!writer.unshown(), "新的一段从零起");
    }
}
