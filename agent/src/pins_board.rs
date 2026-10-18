//! 组件区（Pin Board）：**主体策展、系统渲染**——Run 开场把主体贴在墙上的纸条摆出来。
//!
//! ## 与通知栏的分工（一句话）
//!
//! **通知栏 = 世界的账，系统照的**（每间房有几条新的，零正文，宿主读投影）；
//! **组件区 = 主体的纸条，主体贴的**（自己 `notes_pin` 贴、`notes_unpin` 摘，系统只渲染预览）。
//!
//! ## 它是什么、不是什么
//!
//! 它是**系统服务层的第三个件**（与状态栏、通知栏同族）：Core / Foundation 一样不许认识它——
//! 主体墙上有几张纸条**不是执行语义**，Core 只认识那个中立接缝（`ContextSource`）。
//! 它**不读网络、不读 Agent 的存储**：读是自己（`notes_read`）或装配期一次性只读的事
//! （`notes_app::pinned_material`），摆到这里的只是**已经读回来的行**。
//!
//! ## 素材从哪来：**App 交上来，本件不认识那个 App**
//!
//! 手机模型 §3 的分层是自下而上 **1 循环＋插槽 / 2 显示区 / 3 app / 4 Agent**，而第 3 条写着
//! **"app：往显示区交素材"**——在依赖上那就是 `app → 显示区`（**上层依赖下层**）。
//! 因此本件**定义**它收什么（[`PinnedRow`]），便签 App **依赖本件**并把素材填进来
//! （`notes_app::pinned_material`）。
//!
//! ⚠️ 从前本件 import 的是 `notes_app::PinnedNote`（App 的记录类型）——那是**下层认识上层**。
//! 与持有物同一刀改正：素材类型归显示区，App 只填内容，"怎么摆成一行"归本件
//! （手机模型 §5：「内容归 app 交，怎么显示手机说了算」）。
//!
//! ## 四条纪律
//!
//! 1. **系统永不自动 pin / unpin**：本件只读，一个字节都不写——"贴哪张"永远是主体的动作
//!    （负对照：不存在任何自动策展的代码路径）；
//! 2. **缺省不等于零**：读失败 ⇒ 如实写 `pinned = unknown`（不冒充"墙上没有"）；一张也没有 ⇒
//!    写 `pinned = 0`（墙上空着，而且**我看过**）。这是开场快照，与"零动静不出块"相反：
//!    快照不说话，主体就分不清"空"与"没读到"；
//! 3. **渲染有界**：每张一行，`身份 | 年龄 | 标题 | 正文`，标题与正文各取 **≤60 字节**预览
//!    （与 im 的房预览同一惯例），超出加 `…`；最多渲染 [`MAX_RENDERED`] 张，多的**如实报**
//!    （还有几张没列）。全文永远是 `notes_read` 的事——预览是免费的，不是代替；
//!    ⚠ **年龄那一格是「最后动过」（`touched-ago`），不是「创建于」**——笔记文件里没有时间
//!    字段，盘上只有 mtime，而 `notes_edit` / `notes_append` 会改写它。它与持有物那一格
//!    **同一条纪律**（宪法判据 17 的锚点原文就写着「自 arm 闹钟清单，**带年龄**」），
//!    词**不共用**（那是两个件各自的词，与 `holdings` 的 `due-in` / `overdue` / `fired-ago` 同规）；
//! 4. **永不让活动失败**：摆不出来就少摆一块，但**离开原地要留一条读数**（"这一块超限未给"），
//!    不许整块无声消失（与通知栏同律：便利坏了不挡清醒）。

use crate::core::combine;
use crate::core::{ContentBatch, ContentBlock, ContextSource, InjectionContext};

/// 组件区内容块的标签（呈现时的区域名）。
pub const PINS_BOARD_LABEL: &str = "pinned-notes";

/// 每张纸条的标题与正文各取多少字节做预览（与 im 的房预览同一惯例）。
pub const PREVIEW_BYTES: usize = 60;

/// 一次最多渲染几张（多的如实报"还有 N 张未列出"）。
pub const MAX_RENDERED: usize = 50;

/// 「这一次没读到」时的机械读数（与通知栏的 `unknown` 同族，但在这里指"墙上"而不是"世界"）。
pub const UNKNOWN_PINS: &str = "unknown";

/// **App 交给本件的素材**：一张贴在墙上的纸条。
///
/// 它**不是**便签 App 的记录类型——那是它自己的事（存储里那张还有 `pinned` 标记等本件不该
/// 关心的字段）。App 只交"哪张贴上了、标题正文是什么、什么时候写下的、最后动过是什么时候"，
/// **截断与摆行归本件**。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedRow {
    /// 机械身份（`n-<序号>`）。
    pub id: String,
    /// 这张纸条**什么时候写下的**（unix 毫秒）——**从经历账本折出来**。
    ///
    /// `None` = 折不到（索引还没折到它 / 折不动）⇒ 那一格如实写 `unknown`。
    /// ⚠ 它与 [`PinnedRow::touched_millis`] 答的是**两个问题**，因此**两格都在**：
    /// 这一格是「**创建于**」（权威在账本：`notes_create` 的动作事实），那一格是「**最后动过**」
    /// （盘上只有 mtime）。判据 19：「系统建来源索引，**只记引用**，不给亲疏」。
    pub created_millis: Option<u64>,
    /// 这张纸条**最后动过**的时刻（unix 毫秒）。`None` = **读不出来**，不是 0。
    ///
    /// ⚠ 它是「**最后动过**」，**不是**「什么时候写的」——笔记文件里没有时间字段
    /// （只有 `pinned` / `title` / `text`），盘上只有 mtime，而 `notes_edit` / `notes_append`
    /// 会**改写**它。⇒ 拿它冒充"创建于"就是编事实，这一格的名字因此叫 `touched-ago`。
    pub touched_millis: Option<u64>,
    /// 标题原样（本件按字节截断）。
    pub title: String,
    /// 正文原样（本件按字节截断）。
    pub text: String,
}

/// 组件区：一块开场快照 + 内层来源。
///
/// 它**不持有**任何跨活动状态（摆完就完了）、不认识历史 / 行动 / 模型 / 账本——因此它不可能
/// 替主体判断"哪张该贴"或"该不该去读全文"。
#[derive(Debug)]
pub struct PinsBoard<Inner> {
    /// 装配期由便签 App 交上来的素材。
    ///
    /// `None` = 这一次**没读成**（读失败）——它与 `Some(vec![])`（读成了，一张也没有）是
    /// 两件事，因此这里必须是两层而不是一个空列表（缺省不等于零）。
    pinned: Option<Vec<PinnedRow>>,
    /// 装配那一轮的钟（unix 毫秒）。年龄读数用它，**不另取系统钟**——与持有物同一条纪律
    /// （夹具的固定钟因此照样给出确定的读数，"同一时刻同一输入 ⇒ 逐字节相同输出"可判错）。
    ///
    /// `None` = 钟读不出来（1970 之前）⇒ 年龄那一格如实写 `unknown`，**不编一个数**。
    now_millis: Option<u64>,
    inner: Inner,
}

impl<Inner> PinsBoard<Inner> {
    /// 用 App 交上来的那一份装配：纸条行（可缺）+ 装配那一轮的钟 + 内层来源。
    pub fn new(pinned: Option<&[PinnedRow]>, now_millis: Option<u64>, inner: Inner) -> Self {
        Self {
            pinned: pinned.map(|rows| rows.to_vec()),
            now_millis,
            inner,
        }
    }

    /// 本件那一条读数。
    pub fn reading(&self) -> String {
        match &self.pinned {
            // 读失败：如实说"这一次没读到"，不冒充"一条也没有"。
            None => format!("pinned = {UNKNOWN_PINS}"),
            // 读成了、一条也没有：把零说出口（主体才知道是"空"，不是"没读到"）。
            Some(rows) if rows.is_empty() => "pinned = 0\n（一条也没标上）".to_string(),
            Some(rows) => {
                let mut listed = String::new();
                for row in rows.iter().take(MAX_RENDERED) {
                    listed.push_str(&format!(
                        "{} | {} | {} | {} | {}\n",
                        row.id,
                        created_age_field(row.created_millis, self.now_millis),
                        touched_age_field(row.touched_millis, self.now_millis),
                        preview(&row.title),
                        preview(&row.text)
                    ));
                }
                if rows.len() > MAX_RENDERED {
                    listed.push_str(&format!(
                        "（还有 {} 张未列出）\n",
                        rows.len() - MAX_RENDERED
                    ));
                }
                // **账单**：这一块要递出去的字节数（与格位块的 `~NB` 同形）。纸条是自己写的，
                // 因此这个数主体**改得动**——改短一张，下一轮这里就小一截。
                format!("pinned = {}, ~{}B\n{listed}", rows.len(), listed.len())
            }
        }
    }

    /// 此刻的内容块（标签是本件的身份）。
    pub fn line(&self) -> ContentBlock {
        ContentBlock::new(PINS_BOARD_LABEL, self.reading())
    }

    fn own_batch(&self) -> ContentBatch {
        ContentBatch::of_or_not_delivered(vec![self.line()], PINS_BOARD_LABEL)
    }

    /// 内层来源（只读）。
    pub fn inner(&self) -> &Inner {
        &self.inner
    }
}

impl<Inner: ContextSource> ContextSource for PinsBoard<Inner> {
    /// **开场一次**：与内层的开场事实一起递交。快照不是活状态，之后不重摆。
    fn on_run_start(&mut self, context: &InjectionContext) -> ContentBatch {
        combine(self.inner.on_run_start(context), self.own_batch())
    }

    /// 这里不产生组件区块（快照只在开场摆一次）。内层照旧。
    fn before_turn(&mut self, context: &InjectionContext) -> ContentBatch {
        self.inner.before_turn(context)
    }
}

/// 年龄读数：**「什么时候写下的」多久了**——权威在**经历账本**（`notes_create` 的动作事实），
/// 由便签 App 折出来交给本件（本件**不认识账本**，与它不认识那个 App 同一条纪律）。
///
/// 读不出来（折不到 / 钟读不出来 / 钟往回走）⇒ `age=unknown`，**不编一个数**。
/// ⚠ 词与 [`touched_age_field`] **不共用**：那一个答"最后动过"，这一个答"创建于"。
fn created_age_field(created_millis: Option<u64>, now_millis: Option<u64>) -> String {
    let (Some(created), Some(now)) = (created_millis, now_millis) else {
        return "age=unknown".to_string();
    };
    if now < created {
        return "age=unknown".to_string();
    }
    format!("created-ago={}s", (now - created) / 1000)
}

/// 年龄读数：**「最后动过」多久了**——一种来源一个词，与持有物那一支的词**不共用**。///
/// - 文件时间读不出来（`None`）**或**钟读不出来（`None`）⇒ `age=unknown`——**不编一个数**；
/// - 钟往回走（文件时间在未来）⇒ 也 `age=unknown`，不报一个负数。
///
/// ⚠ 它是 `touched-ago` 而**不是** `created-ago`：笔记文件里没有时间字段，盘上只有 mtime，
/// 而 `notes_edit` / `notes_append` 会改写它。**叫对名字比多一格更要紧**——
/// 拿"最后动过"冒充"创建于"就是编事实。
fn touched_age_field(touched_millis: Option<u64>, now_millis: Option<u64>) -> String {
    let (Some(touched), Some(now)) = (touched_millis, now_millis) else {
        return "age=unknown".to_string();
    };
    if now < touched {
        return "age=unknown".to_string();
    }
    format!("touched-ago={}s", (now - touched) / 1000)
}

/// 预览：**按字节**截断到 [`PREVIEW_BYTES`]，但**不切开一个字符**；截过就加 `…`。///
/// 换行与控制字符在预览里换成空格：预览是"墙上一行"，多一行会伪造出另一张纸条。
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

    /// **块级诚实回退**：纸条板那一块自己超限时留下一句读数，**不退空批次**。
    ///
    /// 生产里进不到这里（每张纸条的标题与正文都经 [`preview`] 截到 60 字节、最多
    /// [`MAX_RENDERED`] 张，身份又受文件名的长度所限），因此这条用例**构造**一个超限块。
    /// 它证明的是"这一处不是静默的"：超限时主体会读到那句读数，而不是"墙上什么都没有"。
    #[test]
    fn an_oversize_block_leaves_a_reading_instead_of_vanishing() {
        let rows = vec![PinnedRow {
            id: "n".repeat(70_000),
            created_millis: None,
            touched_millis: None,
            title: "标题".to_string(),
            text: "正文".to_string(),
        }];
        let board = PinsBoard::new(Some(&rows), None, ActivityOrigin::new());

        // 那一块原本多大（标签 + 正文），读数要原样报出来。
        let expected = PINS_BOARD_LABEL.len() + board.reading().len();
        assert!(expected > ContentBatch::MAX_BYTES, "前提：这一块真的超限");

        let batch = board.own_batch();
        assert_eq!(batch.blocks().len(), 1, "留下一块读数，不是空批次");
        assert_eq!(batch.blocks()[0].label(), PINS_BOARD_LABEL);
        assert_eq!(
            batch.blocks()[0].text(),
            format!("超限未给（原始 {expected} B）")
        );
        assert!(!batch.is_source_failure(), "它不是来源失败");
    }

    fn row(id: &str, touched: Option<u64>) -> PinnedRow {
        PinnedRow {
            id: id.to_string(),
            created_millis: None,
            touched_millis: touched,
            title: "标题".to_string(),
            text: "正文".to_string(),
        }
    }

    /// **年龄那一格：叫对名字。** 它是「最后动过」，**不是**「创建于」——笔记文件里没有时间
    /// 字段，盘上只有 mtime，而 `notes_edit` / `notes_append` 会改写它。
    /// 这一条钉的是**措辞**：把 `touched-ago` 改成 `created-ago` 就是编事实，必须红。
    #[test]
    fn the_age_field_says_touched_not_created() {
        let rows = vec![row("note-1", Some(1_000_000))];
        // 装配那一轮的钟比它晚 100.001 秒 ⇒ 整秒报 100s。
        let text = PinsBoard::new(Some(&rows), Some(1_100_001), ActivityOrigin::new()).reading();
        assert!(text.contains("touched-ago=100s"), "{text}");
        assert!(!text.contains("created"), "不许把它说成「创建于」: {text}");
    }

    /// **不编一个数**：文件时间读不出来 / 钟读不出来 / 钟往回走 —— 三种都如实写 `unknown`。
    #[test]
    fn an_unreadable_or_rewound_clock_never_fabricates_an_age() {
        let cases: [(Option<u64>, Option<u64>); 3] = [
            (None, Some(1_100_001)),            // 文件时间读不出来
            (Some(1_000_000), None),            // 钟读不出来
            (Some(2_000_000), Some(1_100_001)), // 钟往回走（文件时间在未来）
        ];
        for (touched, now) in cases {
            let rows = vec![row("note-1", touched)];
            let text = PinsBoard::new(Some(&rows), now, ActivityOrigin::new()).reading();
            assert!(
                text.contains("age=unknown"),
                "({touched:?},{now:?}) ⇒ {text}"
            );
            assert!(!text.contains("touched-ago"), "不许报一个数: {text}");
        }
    }

    /// **与持有物同一条纪律、词不共用**：`age=unknown` 那一格共用，年龄词各归各的。
    #[test]
    fn the_reading_has_one_row_per_note_with_the_age_in_front() {
        let rows = vec![
            row("note-1", Some(1_000_000)),
            row("note-2", Some(1_000_000)),
        ];
        let text = PinsBoard::new(Some(&rows), Some(1_050_000), ActivityOrigin::new()).reading();
        assert!(text.starts_with("pinned = 2, ~"), "{text}");
        assert_eq!(text.matches("touched-ago=50s").count(), 2, "{text}");
        assert_eq!(
            text.matches("age=unknown").count(),
            2,
            "折不到创建时刻就如实说：{text}"
        );
        assert!(
            text.contains("note-1 | age=unknown | touched-ago=50s | 标题 | 正文"),
            "{text}"
        );
    }
}
