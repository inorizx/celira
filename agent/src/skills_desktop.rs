//! 技能桌面：材料区里的**资源索引**（系统服务第五件）。
//!
//! 模型得知道技能区**存在**才会想起来去翻——上下文里没有这条线索，就没有提取的起点。所以
//! "技能区有多少东西"常驻；但**逐条索引已经降级**（2026-09-22 裁决）：技能是"**有时**才需要"的
//! 信息，而逐条列名字 + 一句话是"每次开场"都要付的成本。尺子是「每次醒都需要 ⇒ 注入目录；
//! 有时才需要 ⇒ 纯拉取」——技能落在后者，于是整块只剩**一行读数**。
//!
//! ```text
//! 一级（常驻，开场一行读数）：技能桌面——`skills: N 项（要翻看用 files_list，路径 skills/）`
//! 二级（按需，翻包）        ：files_list（已有动词）
//! 三级（按需，全文）        ：files_read（已有动词）
//! ```
//!
//! 逐条的名字与说明**本来就在 `files` 的地盘**（`files_list skills/` 列得出、`files_read` 读得到），
//! 这里再抄一份是重复，不是索引。技能区不存在时这一块**整块不出场**：那时连"有多少项"都不存在。
//!
//! 它是**第四块处境投影**：通知栏（世界的动静）｜纸条板（主体的 pin）｜事件卡片（Run 中新事件）｜
//! **技能桌面（材料区资源索引——宿主装了什么）**。
//!
//! ## 物理约定（零机制，只定形状）
//!
//! - 技能根：`<材料根>/skills/`——**只有这个目录下的才算技能**（别处不投影：渐进的默认不可见，
//!   扫描也有界）；
//! - 散 skill：`skills/<name>/SKILL.md`（单层）；包：`skills/<pack>/<skill>/SKILL.md`（两层）；
//! - **一级条目 = `skills/` 的直接子目录**（散 skill 或包）——散**文件**不是技能，不计入。
//!
//! ## 纪律（逐条对齐纸条板）
//!
//! 1. **只读、纯投影**：直接读材料区的目录（与 files app 的展开无关——存在性线索不能等主体点开
//!    材料窗），不持有跨活动状态，不落盘、不新增存储；
//! 2. **缺省不等于零**：`skills/` **不存在** ⇒ 这个块**整块不出场**（不是空标题）；存在但一个
//!    一级条目都没有 ⇒ 如实写 `skills: 0 项（技能区为空）`；
//! 3. **零正文**：桌面块只有这一行读数，**没有任何 SKILL.md 的名字、说明或正文**——要看自己
//!    `files_list` / `files_read`；
//! 4. **永不让活动失败**：读不出来就少摆一块（与通知栏同律：便利坏了不挡清醒）。

use std::fs;
use std::path::{Path, PathBuf};

use crate::core::combine;
use crate::core::{ContentBatch, ContentBlock, ContextSource, InjectionContext};

/// 技能桌面的内容块标签（呈现时的区域名）。
pub const SKILLS_DESKTOP_LABEL: &str = "skills";

/// 技能根（材料区里的那一个目录名）。它同时是那条读数里给出的**相对路径**——主体翻看时要用的
/// 正是这个字面量（读数与 `files_list` 的参数因此同源，不靠主体猜）。
pub const SKILLS_DIR: &str = "skills";

/// 材料根的技能目录路径（装配期要用它读一次；单独给出来是为了让调用方不必知道内部布局）。
pub fn skills_root(material_root: &Path) -> PathBuf {
    material_root.join(SKILLS_DIR)
}

/// 读材料区里的技能**条目数**。
///
/// 返回 `None` = **这个材料根上没有技能区**（`skills/` 不存在、不是目录，或读不出来）⇒ 整块不出场；
/// `Some(0)` = 技能区在、但一个一级条目都没有（如实说"为空"）。
///
/// **数的是目录**（散 skill 与包都是一级条目）——与既有的物理约定同一口径；散文件不是技能。
/// 这里**不读任何 SKILL.md**：逐条的名字与说明不在本块出场（见模块文档），读它就没有消费者。
pub fn read_skills(material_root: &Path) -> Option<usize> {
    let root = skills_root(material_root);
    let metadata = fs::metadata(&root).ok()?;
    if !metadata.is_dir() {
        return None;
    }
    let children = fs::read_dir(&root).ok()?;
    Some(
        children
            .flatten()
            .filter(|child| child.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
            .count(),
    )
}

/// 技能桌面（与纸条板同款：装配期读一次，开场摆一次）。
pub struct SkillsDesktop<Inner> {
    /// 装配期读回来的**条目数**：`None` = 没有技能区（**整块不出场**），`Some(0)` = 有但是空的。
    entries: Option<usize>,
    inner: Inner,
}

impl<Inner> SkillsDesktop<Inner> {
    /// 用装配期读回来的那一份装配：条目数（可缺）+ 内层来源。
    pub fn new(entries: Option<usize>, inner: Inner) -> Self {
        Self { entries, inner }
    }

    /// 本件那一条读数；`None` = 这一块不出场。
    ///
    /// **一行说尽**，而且**免查也免学**：主体要知道怎么翻，就不用先去看格式——翻看的动词名与路径
    /// 随这一行一起给出（`files_list` 吃 `path`，值就是那个目录名）。
    pub fn reading(&self) -> Option<String> {
        let entries = self.entries?;
        if entries == 0 {
            return Some("skills: 0 项（技能区为空）".to_string());
        }
        Some(format!(
            "skills: {entries} 项（要翻看用 files_list，路径 {SKILLS_DIR}/）"
        ))
    }

    /// 此刻的内容块；没有技能区时**没有块**。
    pub fn line(&self) -> Option<ContentBlock> {
        self.reading()
            .map(|reading| ContentBlock::new(SKILLS_DESKTOP_LABEL, reading))
    }

    fn own_batch(&self) -> ContentBatch {
        match self.line() {
            Some(block) => ContentBatch::of_or_not_delivered(vec![block], SKILLS_DESKTOP_LABEL),
            None => ContentBatch::default(),
        }
    }

    /// 内层来源（只读）。
    pub fn inner(&self) -> &Inner {
        &self.inner
    }
}

impl<Inner: ContextSource> ContextSource for SkillsDesktop<Inner> {
    /// **开场一次**：与内层的开场事实一起递交（恢复路径也走这里——材料区结构与 Run 起源无关）。
    fn on_run_start(&mut self, context: &InjectionContext) -> ContentBatch {
        combine(self.inner.on_run_start(context), self.own_batch())
    }

    /// 这里不产生区块（快照只在开场摆一次）。内层照旧。
    fn before_turn(&mut self, context: &InjectionContext) -> ContentBatch {
        self.inner.before_turn(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity_origin::ActivityOrigin;

    /// **这一处的界算术**：技能桌面的读数在结构上**不可能**超限。
    ///
    /// 整块是一条定长句子 + 一个十进制数：取最坏的一个 `usize`（20 位数字）也远小于
    /// [`ContentBatch::MAX_BYTES`]（这条断言就是那句话的机械版）。
    ///
    /// 因此那一处的诚实回退是**防御性**的：这里**构造不出**一个超限块，"内容装不下"这一态在
    /// 这条路上不存在。它的纪律证据是形状守卫
    /// （`tests/minimal_agent.rs::a_batch_that_does_not_fit_is_never_silently_dropped`），
    /// 不是"喂一个超限输入给它"——那样的输入在这条路上不存在。
    #[test]
    fn the_reading_is_bounded_so_it_can_never_need_the_fallback() {
        let board = SkillsDesktop::new(Some(usize::MAX), ActivityOrigin::new());
        let empty = SkillsDesktop::new(Some(0), ActivityOrigin::new());
        for reading in [
            board.reading().expect("有技能区就有读数"),
            empty.reading().expect("有技能区就有读数"),
        ] {
            assert!(
                reading.len() < 8 * 1024,
                "最坏的输入下也远小于上限（实测 {} 字节）",
                reading.len()
            );
            assert!(
                reading.len() * 4 < ContentBatch::MAX_BYTES,
                "离上限还有四倍以上的余量（实测 {} 字节）",
                reading.len()
            );
            assert_eq!(reading.lines().count(), 1, "整块就是一行: {reading}");
        }
    }
}
