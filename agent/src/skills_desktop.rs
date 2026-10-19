//! 技能桌面：材料区里的**资源索引**（系统服务第五件，照纸条板的结构与纪律做）。
//!
//! 模型得知道技能区**存在**才会想起来去翻——上下文里没有这条线索，就没有提取的起点。所以
//! "技能区有多少东西"常驻；但**逐条索引已经降级**（2026-09-22 裁决）：技能是"**有时**才需要"的
//! 信息，而逐条列名字 + 一句话是"每次开场"都要付的成本。尺子是「每次醒都需要 ⇒ 注入目录；
//! 有时才需要 ⇒ 纯拉取」——技能落在后者，于是整块只剩**一行读数**。
//!
//! **判据一句话：给目录，不给判断**——这一行只报"有什么、几个"，**不排序、不评分、不推荐、
//! 不挑该读哪一份**。翻哪一份是主体的判断，系统只保证两件事：它知道技能区里有东西、以及怎么翻。
//!
//! ```text
//! 一级（常驻，开场一行读数）：技能桌面——`skills: N 项（要翻看用 files_list，路径 skills/）`
//! 二级（按需，翻包）        ：files_list（已有动词）
//! 三级（按需，全文）        ：files_read（已有动词）
//! ```
//!
//! 这三档之外还有**第四档：`files_grep`**——"主体自己检索"的那一格（它不属于本件那一行读数）；
//! 判据与真跑读数在 [`crate::file_app`] 的模块文档里。
//!
//! 收益是机械的：读数**不随技能数增长**——三百个一级条目也仍是一行
//! （`tests/skills_desktop.rs::the_reading_does_not_grow_with_the_number_of_skills`）。日常形态逐字
//! 是 `skills: N 项（要翻看用 files_list，路径 skills/）`，为空则是 `skills: 0 项（技能区为空）`。
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
//! - **两种形状只说明"装在哪"，不参与判定**：散 skill `skills/<name>/SKILL.md`（单层）；包
//!   `skills/<pack>/<skill>/SKILL.md`（两层）；
//! - **一级条目 = `skills/` 的直接子目录**（散 skill 或包）——散**文件**不是技能，不计入。
//!
//! ⚠ **数的是结构，不是"能解析出的技能"**：一个**没有 `SKILL.md`** 的一级目录（包本身，或主体
//! 刚建的空目录）**照样算一项**。**本件不读任何 `SKILL.md`**——逐条内容既然不进场，读它就是
//! **纯浪费**（接替旧渲染纪律的两条用例：`the_count_is_the_number_of_top_level_directories`、
//! `the_manifest_content_no_longer_changes_the_reading`）。
//!
//! ## 纪律（逐条对齐纸条板）
//!
//! 1. **只读、纯投影**：直接读材料区的目录（与 files app 的展开无关——存在性线索不能等主体点开
//!    材料窗），不持有跨活动状态，不落盘、不新增存储；
//! 2. **缺省不等于零**：`skills/` **不存在** ⇒ 这个块**整块不出场**（不是空标题）；存在但一个
//!    一级条目都没有 ⇒ 如实写 `skills: 0 项（技能区为空）`——`None ≠ Some(0)`，`0 项` 也不是
//!    "没有技能区"的另一种说法。
//!    ⚠ **`None` 这一格里合着三种情形**（`skills/` 不存在 / 不是目录 / 读不出来）——本件**不区分**
//!    它们：这一格只有一个数，没有可标"读不出来"的位（纸条板那种三态在这里没有落脚处）。要守的
//!    分界是 `None` 与 `Some(0)` 分开，别把"没有技能区"写成"0 项"；
//! 3. **零正文**：桌面块只有这一行读数，**没有任何 SKILL.md 的名字、说明或正文**——要看自己
//!    `files_list` / `files_read`；
//! 4. **永不让活动失败**：读不出来就少摆一块（与通知栏同律：便利坏了不挡清醒）。
//!
//! ## 降级时删掉的那些机制（零消费者，别再捡回来）
//!
//! frontmatter 最小解析、`name:` / `description:` 取值、60 字节预览与截断、显示名"frontmatter
//! 优先"规则、"最多 50 条"上限——它们当时存在的**唯一理由**就是渲染逐条；逐条降级后**零消费者**
//! ⇒ 随降级一起删除。⚠ 本节就是那批删除的登记处：想在读数里再露出名字或说明，先回答"谁消费它"。
//!
//! ## 那一行读数的效用（2026-10-06 第一次量过）
//!
//! 上面那句动机（"模型得知道技能区存在才会想起来去翻"）**是一句可证伪的假设**；而在 2026-10-06
//! 之前，登记下来的只有**成本**（读数不随技能数增长，有用例钉着）——**效用一次都没量过**。探针
//! `scripts/home/probe-recall.js` 量了它：临时家里放一份**随机生成、猜不到**的码 ＋ 人发一条
//! **只说"你手上那份 ZOK"**的任务（不提路径、不提"技能区"），**主判据完全黑盒**（回信里有没有
//! 那个码）。**真跑 28 次**：
//!
//! | `N`（技能份数） | 翻了 | 报对 |
//! |---|---|---|
//! | 1（十六次） | **16/16** | 12/16 |
//! | 30（十二次） | **12/12** | 7/12 |
//!
//! **⇒ 翻了 28/28：一次都没靠猜。**"报对"那一栏剩下的 **9 次**全部有解释、没有一次是"找不回来"
//! ——4 次是**探针自己的措辞歧义**（第一版用"回执"，而它已经是【动作结果】的正式词）、4 次是
//! **探针自己的大小写不一致**（任务说 `ZOK`、技能里写 `zok` ⇒ `files_grep` 命中 0 条）、1 次是
//! 它那一场没读完。**两处都是探针的毛病，都已钉进用例**（`scripts/home/tests/probe-recall.test.js`）。
//!
//! **★ 还有一条是判据的活证据**：失败那几次主体**一次都没编**，而是如实说找不到——原话：
//! 「我不想凭空编一份像模像样的 ZOK 报告糊弄你」「我不打算凭印象编」「我报不出来——我手上找不到
//! 任何叫 ZOK 的规矩……候选 = 0，扫了 30 份材料，没有截断」。它不但不编，还把自己做过的核对
//! 逐条列出来。（这 28 次跑的是**正常模式**：目标那一份真的在场；"指一个不存在的东西、看它报
//! 什么"是后来另立的 `--absent` 模式，判据与退出码在探针文件头。）
//!
//! ⚠ **这 28 次证不到什么（如实登记，不许外推）**：**一个任务族**（都是"按你手上那份 X 的规矩
//! 报 X"）、**一个模型**、**`N` 只到 30**。⇒ 它证明的是"**在这个模型 ＋ 这个任务族上，那一行
//! 线索足够**"，**不是**"任何模型都会翻"，也**不是**"`N` 再大也够"。
//!
//! **跑法与触发条件**：`node scripts/home/probe-recall.js`（参数与退出码见探针自己的文件头；
//! 它花**真模型调用**）；**什么时候必须跑**写在 `AGENTS.md`「验证与汇报」——换模型 / 改技能桌面
//! 与开场线索 / 改 `files_*` 那一面。
//!
//! ## 权威位置
//!
//! **物理约定（技能根、一级条目、数的是结构）的唯一权威是这里**：`agent/SKILL_PACK_GUIDE.md` 是
//! **对外入口**（只写"外部作者怎么接"那半），`agent/README.md` §3.28 是**指针**——两处都
//! **不另立权威**。行为证明：`tests/skills_desktop.rs`。

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
