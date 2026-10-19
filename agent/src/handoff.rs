//! 交接（handoff）：主体**在上一次活动里写给下一次开场的自己**的那一篇。
//!
//! ⚠ **它与 [`crate::self_concept`] 不是一回事**（虽然形状同款）：
//! 自识回答「**我是谁**」（常驻、跨段为真）；交接回答「**这一段我干到哪、下一段接着做什么**」
//! （**有确定的交付时刻**——紧邻的下一次开场）。判据一句话：
//! **注入与取阅的分界，是「有没有确定的交付时刻」**——有 ⇒ 该注入**一次**；
//! 没有（笔记、自识）⇒ 主体自己取阅。
//!
//! ## 三态不许压成两态
//!
//! - [`Handoff::Present`] ⇒ 原文摆出（**这是主体的字**，前后不加东西）；
//! - [`Handoff::Absent`] ⇒ **一个字都不说、连标签都不出现**（"写了才有、不写就没有"）；
//! - [`Handoff::Unreadable`] ⇒ 如实说"这一次没读到"，**不冒充 `Absent`**。
//!
//! ## 一次播放
//!
//! 交付**只发生一次**（下一段开场），此后不再摆——否则一份写完没改的交接会**每一段都重播**，
//! 读起来像"这一段还是那一段"。那一格由**调用方**告诉本模块（[`HandoffBoard::new`] 的
//! `delivered`）：本模块不自己去账里翻，也不落任何新事实（**零新写者**）。
//!
//! ## 写权归自己
//!
//! 与自识同款（主人 2026-09-21 裁决）：`write_handoff` 是**主体的自主动词**，
//! 系统不替主体写、也不替主体清。上限是**硬墙**：**拒绝、不落盘、不截断**，旧的一份原样保留。

use std::io;
use std::path::{Path, PathBuf};

use crate::core::combine;
use crate::core::{ContentBatch, ContentBlock, ContextSource, InjectionContext};

/// 交接那一块在注入里的标签。
pub const HANDOFF_LABEL: &str = "handoff";
/// 自持物里那个文件的名字：`<环境根>/handoff`。
pub const HANDOFF_FILE: &str = "handoff";

/// 交接的三态。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Handoff {
    /// **没有那一份**（文件不在，或只有一个空文件）⇒ 开场**优雅跳过**，不注入空壳。
    #[default]
    Absent,
    /// 读到了正文。
    Present(String),
    /// **这一次没读到**（权限、坏数据……）⇒ 如实标出，不冒充"主体没有交接"。
    Unreadable,
}

/// 交接的**字节上限**：**够一次交接、不够一段日志**。
///
/// **取值依据**：交接是**一段**的量（"我干到哪、下一段接着做什么"），不是流水账——
/// 所以它比 [`crate::self_concept::MAX_SELF_CONCEPT_BYTES`]（4 KiB，"够自识、不够自传"）
/// **更小**：2 KiB ≈ 650 个汉字，只占注入批（64 KiB）的 **3%**，不挤占别的块。
///
/// **为什么要在写入杠杆处拒绝**：这一块**每段开场都可能摆**，超限就该在**写的那一刻**说清，
/// 而不是让注入闸兜底成"整块没给"——那正是主体最需要的那一块裸奔。
/// **拒绝、不落盘、不截断**，旧的一份原样保留（对齐 notes 的 `too-large` 先例）。
pub const MAX_HANDOFF_BYTES: usize = 2 * 1024;

/// 超限写入的**稳定分类标记**（`write_handoff` 用它渲染拒绝读数；逐字节稳定，词表纪律）。
pub const HANDOFF_TOO_LARGE: &str = "handoff-too-large";

/// 自持物的路径。
fn handoff_path(environment_root: &Path) -> PathBuf {
    environment_root.join(HANDOFF_FILE)
}

/// 读主体的交接。
///
/// **文件不存在 = 主体没写过**（不写就没有）⇒ [`Handoff::Absent`]；
/// **其他读失败** ⇒ [`Handoff::Unreadable`]（**不冒充 Absent**）；
/// **读到空白** ⇒ 也算 `Absent`：一个空文件不是交接。写入口会拒绝写空，
/// 所以这一支只可能来自外部手改——把它读成"没有"比读成一个空洞好。
pub fn read(environment_root: &Path) -> Handoff {
    match std::fs::read_to_string(handoff_path(environment_root)) {
        Ok(text) => {
            if text.trim().is_empty() {
                Handoff::Absent
            } else {
                Handoff::Present(text)
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Handoff::Absent,
        Err(_) => Handoff::Unreadable,
    }
}

/// **写下这一步本身**：两段检查 + 一次 `write`。**它不是公开面**——
/// "多一个名字就多一条改交接的路"，所以模块的公开面仍然只有**两个**：
/// 写（[`write_from_subject`]）与读（[`read`]）。单元用例在同一个模块里，照旧能直接调它。
///
/// 两条拒绝（空正文 `InvalidInput`、超限 `InvalidData`）都发生在**动手之前**：
/// 这个函数里没有任何一条路径会在拒绝之后留下半个文件。
fn incept(environment_root: &Path, text: &str) -> io::Result<()> {
    if text.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "交接的正文不能是空的",
        ));
    }
    if text.len() > MAX_HANDOFF_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "交接的正文超过上限",
        ));
    }
    std::fs::write(handoff_path(environment_root), text)
}

/// 写下交接时的**失败分类**（封闭；稳定字符串进主体的回执）。
///
/// 两条都发生在**动手之前**、都**不截断、不覆盖**——旧的那一份原样保留。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandoffWriteFailure {
    /// 超过 [`MAX_HANDOFF_BYTES`]（带上"想写多大 / 旧的现在多大"两件机械事实）。
    TooLarge { wanted: usize, current: usize },
    /// 写不进去（权限、盘满…）。
    Unwritable,
}

impl HandoffWriteFailure {
    /// 稳定分类标记（进读数）。
    pub fn as_str(self) -> &'static str {
        match self {
            HandoffWriteFailure::TooLarge { .. } => HANDOFF_TOO_LARGE,
            HandoffWriteFailure::Unwritable => "handoff-unwritable",
        }
    }

    /// 拒绝读数（只有超限那一档带正文）：**想写多大、上限多大、旧的还在不在**，三件事一次说清。
    pub fn reading(self) -> Option<ContentBlock> {
        match self {
            HandoffWriteFailure::TooLarge { wanted, current } => Some(ContentBlock::new(
                HANDOFF_LABEL,
                format!(
                    "handoff = {wanted} B\n上限 = {MAX_HANDOFF_BYTES} B\n旧的 handoff 原样保留（现值 {current} B）"
                ),
            )),
            HandoffWriteFailure::Unwritable => None,
        }
    }
}

/// **写下 / 改写主体的交接**：这一份事实的**唯一写者**。
///
/// 域规则全在这里：空正文与超限**拒绝、不写、不截断**，旧的那一份原样保留。
/// 成功时交回**给主体的那一块回执**（含"现在多大 / 上限多大"，以及"下一次开场交付"）。
///
/// ⚠ **不在这里的两件**（那是**参数事实**与**路由事实**）：① 空正文（与"缺参数、参数形状不对"
/// 同族，归 `os` 的 `require_params` 那一侧）；② 这一次装配有没有环境根。
pub fn write_from_subject(
    environment_root: &Path,
    text: &str,
) -> Result<ContentBlock, HandoffWriteFailure> {
    let current = match read(environment_root) {
        Handoff::Present(text) => text.len(),
        _ => 0,
    };
    match incept(environment_root, text) {
        Ok(()) => Ok(ContentBlock::new(
            HANDOFF_LABEL,
            format!(
                "交接已写下（handoff = {} B，上限 {MAX_HANDOFF_BYTES} B）。\
                 **下一次开场交付一次**：交付过就不再摆——所以没改的话，它不会每一段都重播。",
                text.len()
            ),
        )),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            Err(HandoffWriteFailure::TooLarge {
                wanted: text.len(),
                current,
            })
        }
        Err(_) => Err(HandoffWriteFailure::Unwritable),
    }
}

/// 开场那一块：**上一次活动写给这一次的交接**。
///
/// 三态各自的呈现见模块头。⚠ **`delivered` 那一格是"一次播放"的开关**：
/// `true` ⇒ **一个字都不说**（这一份已经交付过了）；调用方负责从**已有的事实**里算出它
/// （本模块不落新事实、也不自己去翻账）。
#[derive(Debug)]
pub struct HandoffBoard<Inner> {
    /// 装配期读回来的那一态。
    handoff: Handoff,
    /// 这一份**已经交付过**了吗（`true` ⇒ 不再摆）。
    delivered: bool,
    inner: Inner,
}

impl<Inner> HandoffBoard<Inner> {
    /// 用装配期读回来的那一态与"交付过没有"装配。
    pub fn new(handoff: Handoff, delivered: bool, inner: Inner) -> Self {
        Self {
            handoff,
            delivered,
            inner,
        }
    }

    /// 读完之后的**装配期**构造：从环境根读一次（同一份读法，两处装配点共用）。
    pub fn from_environment(environment_root: &Path, delivered: bool, inner: Inner) -> Self {
        Self::new(read(environment_root), delivered, inner)
    }

    /// 这一次要递交的块（**可能 0 或 1 块**——缺席的不占位）。
    fn own_batch(&self) -> ContentBatch {
        let mut blocks = Vec::new();
        match &self.handoff {
            // **一次播放**：交付过就不再摆（否则一份没改的交接会每一段都重播）。
            _ if self.delivered => {}
            // 原文摆出：一个字都不加。
            Handoff::Present(text) => blocks.push(ContentBlock::new(HANDOFF_LABEL, text.clone())),
            // **优雅跳过**：不注入空壳、连标签都不出现（写了才有、不写就没有）。
            Handoff::Absent => {}
            Handoff::Unreadable => blocks.push(ContentBlock::new(
                HANDOFF_LABEL,
                "（这一次没读到你的交接；它还在你的环境根里，这只是这次没读到）",
            )),
        }
        ContentBatch::of_or_not_delivered(blocks, HANDOFF_LABEL)
    }

    /// 内层来源（只读）。
    pub fn inner(&self) -> &Inner {
        &self.inner
    }
}

impl<Inner: ContextSource> ContextSource for HandoffBoard<Inner> {
    /// **开场一次**：与内层的开场事实一起递交（快照不是活状态，之后不重摆）。
    fn on_run_start(&mut self, context: &InjectionContext) -> ContentBatch {
        combine(self.inner.on_run_start(context), self.own_batch())
    }

    /// 轮级照旧交给内层：这一件只在开场露面。
    fn before_turn(&mut self, context: &InjectionContext) -> ContentBatch {
        self.inner.before_turn(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时根：按进程号与标签分开，避免并行测试互相踩。
    fn temp_root(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("celira-handoff-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("建临时根");
        root
    }

    /// **写了才有、不写就没有**：没写过 ⇒ `Absent`，而且开场**一个字都不说**。
    #[test]
    fn a_handoff_that_was_never_written_is_absent_and_says_nothing() {
        let root = temp_root("absent");
        assert_eq!(read(&root), Handoff::Absent);
        let board = HandoffBoard::new(Handoff::Absent, false, ());
        let batch = board.own_batch();
        assert!(batch.blocks().is_empty(), "缺席的交接不许占位：{batch:?}");
    }

    /// **读不到不冒充没有**：把 handoff 换成目录 ⇒ 读失败 ⇒ `Unreadable`，而且**说出来**。
    #[test]
    fn a_failed_read_is_unreadable_and_never_pretends_there_is_none() {
        let root = temp_root("unreadable");
        std::fs::create_dir_all(root.join(HANDOFF_FILE)).expect("造目录顶替那一份");
        assert_eq!(read(&root), Handoff::Unreadable);
        let board = HandoffBoard::new(read(&root), false, ());
        let batch = board.own_batch();
        assert_eq!(batch.blocks().len(), 1);
        assert!(
            batch.blocks()[0].text().contains("这一次没读到"),
            "读不到要如实说读不到"
        );
    }

    /// 写下了 ⇒ 原文摆出（一个字都不加）。
    #[test]
    fn a_written_handoff_is_delivered_verbatim() {
        let root = temp_root("present");
        let text = "我干到第三步：正在等对面回话。下一段接着问。";
        write_from_subject(&root, text).expect("写");
        assert_eq!(read(&root), Handoff::Present(text.to_string()));
        let board = HandoffBoard::new(read(&root), false, ());
        let batch = board.own_batch();
        assert_eq!(batch.blocks().len(), 1);
        assert_eq!(batch.blocks()[0].text(), text, "原文摆出，前后不加东西");
    }

    /// **一次播放**：交付过 ⇒ 一个字都不说（否则没改的交接会每一段都重播）。
    #[test]
    fn a_delivered_handoff_is_not_shown_again() {
        let root = temp_root("delivered");
        write_from_subject(&root, "这一段要接着做 X").expect("写");
        let board = HandoffBoard::new(read(&root), true, ());
        let batch = board.own_batch();
        assert!(batch.blocks().is_empty(), "交付过的不许再摆：{batch:?}");
    }

    /// **空正文拒绝**：没有交接应当是"根本没有那一份"，而不是留一个空洞。
    #[test]
    fn an_empty_handoff_is_refused() {
        let root = temp_root("empty");
        assert!(incept(&root, "").is_err(), "空串必须拒绝");
        assert!(incept(&root, "   \n").is_err(), "只有空白也必须拒绝");
        assert_eq!(read(&root), Handoff::Absent, "拒绝之后不该留下文件");
        // 走到公开面那一侧：空正文归"写不进去"那一档（参数形状那半归 `os`）。
        assert_eq!(
            write_from_subject(&root, "   \n"),
            Err(HandoffWriteFailure::Unwritable)
        );
    }

    /// **超限拒绝、不截断、旧的原样保留**。
    #[test]
    fn an_over_large_handoff_is_refused_and_the_old_one_survives() {
        let root = temp_root("too-large");
        let old = "旧的那一份";
        write_from_subject(&root, old).expect("先写一份小的");
        let big = "字".repeat(MAX_HANDOFF_BYTES);
        match write_from_subject(&root, &big) {
            Err(HandoffWriteFailure::TooLarge { wanted, current }) => {
                assert_eq!(wanted, big.len());
                assert_eq!(current, old.len());
            }
            other => panic!("应当超限拒绝：{other:?}"),
        }
        assert_eq!(
            read(&root),
            Handoff::Present(old.to_string()),
            "拒绝之后旧的一份必须原样保留"
        );
    }

    /// **空文件也算没有**（写入口拒绝写空 ⇒ 这一支只可能来自外部手改）。
    #[test]
    fn a_blank_file_reads_as_absent() {
        let root = temp_root("blank");
        std::fs::write(root.join(HANDOFF_FILE), "  \n").expect("手写一个空壳");
        assert_eq!(read(&root), Handoff::Absent);
    }
}
