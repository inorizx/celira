//! 技能桌面：材料区里的**方法索引**（2026-10-20 从「一行读数」改成「有界的名字索引」）。
//!
//! 模型得知道技能区**有什么**才会想起来去翻——上下文里没有这条线索，就没有提取的起点。
//! 2026-09-22 的裁决把整块压成一行计数（"有时才需要 ⇒ 纯拉取"）；实测（`probe-recall`，28 次）
//! 证明**任务点到那份材料时**那一行够用（28/28 都翻了）。它没证的那一半是**任务没点名的情形**：
//! 那时"去翻"全靠主体自己想起来，而技能的价值恰恰是"任务来了能想起来用"。
//!
//! ## 判决（2026-10-20，主人裁）
//!
//! **名字与一句触发说明进场，正文不进场**。判据是把两件事拆开：
//!
//! - **名字 + 触发**（`description:` 那一行）＝**检索索引**：决定"要不要翻它"的那一格；
//! - **正文**＝**方法本身**：仍然是 `files_read` 的地盘，一个字节都不进场。
//!
//! 行业里"名字 + 描述放开场"（Agent Skills 那套三层渐进披露的第一层）之所以成立，就是因为
//! 第二层的"按需加载"**依赖第一层的触发线索**；没有第一层，"按需"退化成"靠主体想起去翻"。
//!
//! ## 三条纪律（与旧版一脉相承）
//!
//! 1. **只读、纯投影**：读材料区的目录与每个一级条目的 `SKILL.md` **开头**（≤8 KiB，只取
//!    `name` / `description` 两行），不持有跨活动状态，不落盘、不新增存储；
//! 2. **有界**：一级条目最多列 [`MAX_LISTED`] 个（按目录名排序，**稳定身份序、不是推荐序**），
//!    名字 ≤ [`MAX_NAME_BYTES`]、触发说明 ≤ [`MAX_HINT_BYTES`]（截断带 `…`），超出部分如实说
//!    `…另 N 项`。一行读数的**记账**变成"有界增长"，不再"不随 N 增长"；
//! 3. **零正文**：正文一个字都不进场——`SKILL.md` 的 `# 步骤` 那一段仍然只能 `files_read` 读到。
//!
//! ## 缺省不等于零
//!
//! `skills/` **不存在** ⇒ 这个块**整块不出场**（不是空标题）；存在但一个一级条目都没有 ⇒
//! 如实写 `skills: 0 项（技能区为空）`——`None ≠ Some(0)`，`0 项` 也不是"没有技能区"的另一种说法。
//! ⚠ **`None` 这一格里仍合着三种情形**（`skills/` 不存在 / 不是目录 / 读不出来）——本件不区分。
//!
//! ## 引用（2026-10-25）：库是家的，引用是每支 Agent 的
//!
//! [`SkillRef`] 是**装配期的可见性声明**：`All`（缺省）＝ 库里的全部一级条目；`None` ＝ 一项都不
//! 启用；`Only` ＝ 只启用名单里的**一级目录名**（不是 frontmatter 的显示名，也不是包里的子技能名）。
//! 它只过滤**开场索引**；正文仍然走通用 `files_*`，所以这是**软引用**，不是访问控制。引用也**不
//! 授予任何能力**——技能是方法，能力仍归 MCP / 终端那几份授权。
//!
//! 一个显式名单是**快照语义**：之后新装的技能不会自动进老 Agent 的索引；`All` 则跟随技能库。
//!
//! ## 物理约定（零机制，只定形状）
//!
//! - 技能根：`<材料根>/skills/`——只有这个目录下的才算；
//! - **一级条目 = `skills/` 的直接子目录**（散 skill 或包）；散**文件**不是技能，不计入；
//! - **数的是结构**：没有 `SKILL.md` 的一级目录照样算一项（名字回退成目录名）；
//! - 名字取 `SKILL.md` frontmatter 的 `name:`，缺省回退目录名；触发说明取 `description:`
//!   ——单行、引号、折叠块（`>` / `|`）三种写法都认，渲染时**按预算取整句**（切在句末，
//!   切不到才硬截并加 `…`）。**不认识的字段一律忽略**。
//!
//! ## 降级时删掉、这次**没有**捡回来的机制
//!
//! 60 字节预览与按条截断的旧算法、显示名"frontmatter 优先"的**逐条渲染**、一次最多 50 条——
//! 它们的形状属于"逐条清单"，不是本件这个"有界索引"；这次只按 [`MAX_LISTED`] / [`MAX_NAME_BYTES`] /
//! [`MAX_HINT_BYTES`] 三个界重写。
//!
//! ## 那一行读数的效用（2026-10-06 第一次量过）
//!
//! `probe-recall` 量了"一行线索够不够"：临时家里放一份**随机、猜不到**的码 ＋ 人发一条
//! **只说"你手上那份 ZOK"** 的任务，主判据完全黑盒（回信里有没有那个码）。真跑 28 次：
//! 翻了 **28/28**，报对 19/28（其余 9 次全部有解释，且都不是"找不回来"）。
//!
//! ⚠ **它证不到什么**：**一个任务族**（都是"按你手上那份 X 的规矩报 X"）、**一个模型**、
//! `N` 只到 30；它证明的是"任务点名了那份材料时，一行线索足够"，**不是**"任务没点名时主体会去翻"。
//! 后者是这一次改成名字索引的**理由**，也是它的**未验证假设**——要量就得另写一个"不点名"的探针。
//!
//! ## 权威位置
//!
//! **物理约定（技能根、一级条目、数的是结构）的唯一权威是这里**：`agent/SKILL_PACK_GUIDE.md`
//! 是**对外入口**（只写"外部作者怎么接"那半），`agent/README.md` §3.28 是**指针**。
//! 行为证明：[`tests/skills_desktop.rs`](../../tests/skills_desktop.rs)。

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::core::combine;
use crate::core::{ContentBatch, ContentBlock, ContextSource, InjectionContext};

/// 技能桌面的内容块标签（呈现时的区域名）。
pub const SKILLS_DESKTOP_LABEL: &str = "skills";

/// 技能根（材料区里的那一个目录名）。它同时是那条读数里给出的**相对路径**。
pub const SKILLS_DIR: &str = "skills";

/// 一级条目最多列几个（超出如实说"另 N 项"）。
pub const MAX_LISTED: usize = 12;

/// 显示名（frontmatter `name:` 或目录名）的字节上限。
pub const MAX_NAME_BYTES: usize = 64;

/// 触发说明（frontmatter `description:`）的字节预算：放不下就在预算内**按句末**切。
///
/// 160 字节 ≈ 53 个汉字：够写一句"什么时候用"，最坏 12 条约 2 KB（首屏现在约 9 KB）。
pub const MAX_HINT_BYTES: usize = 160;

/// 只读 `SKILL.md` 的开头这么多字节（frontmatter 在顶部；读正文是浪费）。
const MAX_MANIFEST_BYTES: u64 = 8 * 1024;

/// 清单文件名（物理约定的一部分）。
const MANIFEST_FILE: &str = "SKILL.md";

/// 材料根的技能目录路径（装配期要用它读一次）。
pub fn skills_root(material_root: &Path) -> PathBuf {
    material_root.join(SKILLS_DIR)
}

/// **这一支启用了哪些技能**（宿主声明，装配期事实）。
///
/// 匹配的是**一级目录名**（[`SkillEntry::id`]）——稳定身份，不是可改的显示名，也不是包里的子技能名。
/// 它只过滤开场索引：正文仍可由 `files_*` 读到（软引用），也不授予任何能力。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SkillRef {
    /// 库里的全部一级条目（缺省；跟随技能库，新装的下一次装配会出现）。
    #[default]
    All,
    /// 一项都不启用（开场索引如实写 0 项，并报技能区总数；不是假装技能区为空）。
    None,
    /// 只启用这些一级目录名；显式名单是快照，新装技能不会自动进。
    Only(Vec<String>),
}

impl SkillRef {
    /// 解析宿主给的引用：`all` / `none` / 逗号分隔的一级目录名。
    ///
    /// `all` 与 `none` 只能单独写；空项、路径分隔符、`.` / `..` 都拒绝——引用不是路径。
    /// 重复名字按第一次出现保留；顺序不影响索引（索引仍按目录名排序）。
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("技能引用不能为空（全部要写 all，一项都不要写 none）".to_string());
        }
        match trimmed.to_ascii_lowercase().as_str() {
            "all" => return Ok(Self::All),
            "none" => return Ok(Self::None),
            _ => {}
        }
        let mut names: Vec<String> = Vec::new();
        for part in trimmed.split(',') {
            let name = part.trim();
            if name.is_empty() {
                return Err("技能引用里有空项".to_string());
            }
            if name.eq_ignore_ascii_case("all") || name.eq_ignore_ascii_case("none") {
                return Err("技能引用里的 all / none 只能单独写".to_string());
            }
            if name.contains('/') || name.contains('\\') || name == "." || name == ".." {
                return Err(format!("技能引用只写一级目录名（不是路径）：{name}"));
            }
            if !names.iter().any(|existing| existing == name) {
                names.push(name.to_string());
            }
        }
        Ok(Self::Only(names))
    }

    /// 宿主读数里的稳定写法。
    pub fn as_text(&self) -> String {
        match self {
            Self::All => "all".to_string(),
            Self::None => "none".to_string(),
            Self::Only(names) => names.join(","),
        }
    }

    /// 这个名字的一级条目是否启用。
    pub fn permits(&self, id: &str) -> bool {
        match self {
            Self::All => true,
            Self::None => false,
            Self::Only(names) => names.iter().any(|name| name == id),
        }
    }
}

/// 一次读取的结果：**启用条目的索引** ＋ **技能区原有的总数**。
///
/// 两个数分开，是为了让「技能区为空」与「有技能、但这一支一项都没启用」不混成同一句话。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillIndex {
    /// 启用的一级条目（已按目录名排序、已按引用过滤）。
    pub entries: Vec<SkillEntry>,
    /// 过滤前的总数（技能区里一级目录的个数）。
    pub total: usize,
}

impl SkillIndex {
    /// 全部启用（测试与 `All` 路径用）。
    pub fn all(entries: Vec<SkillEntry>) -> Self {
        let total = entries.len();
        Self { entries, total }
    }
}

/// 一个一级条目：显示名 ＋ 一句触发说明（正文不在里面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEntry {
    /// **稳定身份 = 一级目录名**（引用按它匹配；显示名可以改，它不改）。
    pub id: String,
    /// frontmatter `name:`，缺省回退目录名；已消毒（单行、≤ [`MAX_NAME_BYTES`]）。
    pub display: String,
    /// frontmatter `description:` 的一句触发（按预算取整句）；没有/不可解析 ⇒ `None`。
    pub hint: Option<String>,
}

/// 压成单行：控制字符换空格，去两头空白；空 ⇒ `None`；超界按 UTF-8 边界截断并加 `…`。
fn one_line(raw: &str, max_bytes: usize) -> Option<String> {
    let collapsed: String = raw
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    let trimmed = collapsed.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(truncate_bytes(trimmed, max_bytes))
}

/// 按**字节**截断，但只在字符边界上切；截了就加 `…`。
fn truncate_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// 取**一句触发**：先压成单行；放得下就整句；放不下就在预算内**最后一个句末**切开
/// （`。！？.!?`），切不到才硬截加 `…`。
///
/// 为什么不像 [`truncate_bytes`] 那样直接切：外部技能的 `description` 往往是一整段，
/// 直接按字节切会把"什么时候用"那半句切掉；按句子切，至少留下一个完整的判断。
fn cue(raw: &str, max_bytes: usize) -> Option<String> {
    let collapsed: String = raw
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    let trimmed = collapsed.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.len() <= max_bytes {
        return Some(trimmed.to_string());
    }
    let mut last_end = None;
    for (index, ch) in trimmed.char_indices() {
        if index >= max_bytes {
            break;
        }
        if matches!(ch, '。' | '！' | '？' | '.' | '!' | '?') {
            last_end = Some(index + ch.len_utf8());
        }
    }
    match last_end {
        Some(end) => Some(trimmed[..end].to_string()),
        None => Some(truncate_bytes(trimmed, max_bytes)),
    }
}

/// 去掉 YAML 值两头可能有的引号。
fn unquote(raw: &str) -> &str {
    let trimmed = raw.trim();
    let stripped = trimmed
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        });
    stripped.unwrap_or(trimmed)
}

/// 解析 frontmatter 的 `name` / `description`（只认文件开头 `---` 到下一个 `---` 那一段）。
///
/// **两个分隔符都要在**：只有开头、没有收口的那一段不算 frontmatter（坏清单 ⇒ 回退目录名，
/// 不拿半截正文当名字）。
///
/// 兼容外部技能的三种写法：单行值、引号值、**折叠/字面块**（`>` / `|` 及其 `-` / `+` 变体——
/// 收后面的缩进行、按空格拼起来再取一句）。**不认识的字段一律忽略**（`license` / `metadata` /
/// `allowed-tools` 这些都不拦、不报错）。
fn parse_frontmatter(text: &str) -> (Option<String>, Option<String>) {
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return (None, None);
    }
    let body: Vec<&str> = lines.collect();
    let Some(close) = body.iter().position(|line| line.trim_end() == "---") else {
        return (None, None);
    };
    let block = &body[..close];
    let mut name = None;
    let mut hint = None;
    let mut index = 0;
    while index < block.len() {
        let line = block[index];
        if name.is_none()
            && let Some(value) = line.strip_prefix("name:")
        {
            name = one_line(unquote(value), MAX_NAME_BYTES);
            index += 1;
            continue;
        }
        if hint.is_none()
            && let Some(value) = line.strip_prefix("description:")
        {
            let value = unquote(value);
            if matches!(value, ">" | "|" | ">-" | "|-" | ">+" | "|+") {
                // 折叠块：收后面的缩进行，直到非缩进行 / 收口 / 结束。
                let mut collected: Vec<&str> = Vec::new();
                index += 1;
                while index < block.len() {
                    let next = block[index];
                    if next.trim().is_empty() {
                        index += 1;
                        continue;
                    }
                    if next.starts_with(' ') || next.starts_with('\t') {
                        collected.push(next.trim());
                        index += 1;
                        continue;
                    }
                    break;
                }
                hint = cue(&collected.join(" "), MAX_HINT_BYTES);
                continue;
            }
            hint = cue(value, MAX_HINT_BYTES);
            index += 1;
            continue;
        }
        index += 1;
    }
    (name, hint)
}

/// 读一个一级条目的清单开头；读不到/读不懂 ⇒ `(None, None)`（回退目录名，不阻断）。
fn read_manifest(path: &Path) -> (Option<String>, Option<String>) {
    let Ok(file) = fs::File::open(path) else {
        return (None, None);
    };
    let mut head = String::new();
    if file
        .take(MAX_MANIFEST_BYTES)
        .read_to_string(&mut head)
        .is_err()
    {
        return (None, None);
    }
    parse_frontmatter(&head)
}

/// 读材料区里的技能索引。
///
/// 返回 `None` = **这个材料根上没有技能区**（`skills/` 不存在、不是目录，或读不出来）⇒ 整块不出场；
/// `Some(entries)` = 技能区在（`Some(vec![])` 就是"为空"，如实说）。
///
/// 顺序是**目录名的稳定身份序**（不是推荐序、不是评分序）——"给目录不给判断"。
pub fn read_skills(material_root: &Path) -> Option<Vec<SkillEntry>> {
    let root = skills_root(material_root);
    let metadata = fs::metadata(&root).ok()?;
    if !metadata.is_dir() {
        return None;
    }
    let mut dirs: Vec<PathBuf> = fs::read_dir(&root)
        .ok()?
        .flatten()
        .filter(|child| child.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .map(|child| child.path())
        .collect();
    dirs.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
    Some(
        dirs.into_iter()
            .map(|dir| {
                let dir_name = dir
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default();
                let (name, hint) = read_manifest(&dir.join(MANIFEST_FILE));
                SkillEntry {
                    id: dir_name.clone(),
                    display: name
                        .or_else(|| one_line(&dir_name, MAX_NAME_BYTES))
                        .unwrap_or_default(),
                    hint,
                }
            })
            .collect(),
    )
}

/// 按一支 Agent 的引用读技能索引。
///
/// `None` = 材料根上没有技能区（与 [`read_skills`] 同义）；`Some(index)` = 技能区在。
/// `index.total` 是过滤前的总数，`index.entries` 只含启用条目——两个数分开，
/// 「技能区为空」与「有技能但一项都没启用」因此说得清。
pub fn read_skills_for(material_root: &Path, reference: &SkillRef) -> Option<SkillIndex> {
    let entries = read_skills(material_root)?;
    let total = entries.len();
    Some(SkillIndex {
        entries: entries
            .into_iter()
            .filter(|entry| reference.permits(&entry.id))
            .collect(),
        total,
    })
}

/// 技能桌面（与纸条板同款：装配期读一次，开场摆一次）。
pub struct SkillsDesktop<Inner> {
    /// 装配期读回来的索引：`None` = 没有技能区（**整块不出场**）。
    index: Option<SkillIndex>,
    inner: Inner,
}

impl<Inner> SkillsDesktop<Inner> {
    /// 用装配期读回来的那一份装配：索引（可缺）＋ 内层来源。
    ///
    /// 这个构造点把一份 `Vec` 视为「全部启用」；按引用过滤过的路径用 [`Self::with_index`]。
    pub fn new(entries: Option<Vec<SkillEntry>>, inner: Inner) -> Self {
        Self {
            index: entries.map(SkillIndex::all),
            inner,
        }
    }

    /// 用已按引用过滤的索引装配（`total` 是过滤前的总数）。
    pub fn with_index(index: Option<SkillIndex>, inner: Inner) -> Self {
        Self { index, inner }
    }

    /// 本件那一条读数；`None` = 这一块不出场。
    ///
    /// **有界**：最多 [`MAX_LISTED`] 条，其余如实说"另 N 项"；每条只有名字与一句触发说明。
    pub fn reading(&self) -> Option<String> {
        let index = self.index.as_ref()?;
        if index.entries.is_empty() {
            return Some(if index.total == 0 {
                "skills: 0 项（技能区为空）".to_string()
            } else {
                format!(
                    "skills: 0 项（没有启用任何技能；技能区共 {} 项）",
                    index.total
                )
            });
        }
        let mut out = if index.entries.len() == index.total {
            format!(
                "skills: {} 项（要翻看用 files_list，路径 {SKILLS_DIR}/）",
                index.entries.len()
            )
        } else {
            format!(
                "skills: {} 项（已启用；技能区共 {} 项；要翻看用 files_list，路径 {SKILLS_DIR}/）",
                index.entries.len(),
                index.total
            )
        };
        for entry in index.entries.iter().take(MAX_LISTED) {
            out.push('\n');
            match &entry.hint {
                Some(hint) => out.push_str(&format!("  {} — {hint}", entry.display)),
                None => out.push_str(&format!("  {}", entry.display)),
            }
        }
        if index.entries.len() > MAX_LISTED {
            out.push('\n');
            out.push_str(&format!("  …另 {} 项", index.entries.len() - MAX_LISTED));
        }
        Some(out)
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

    /// **这一处的界算术**：有界索引在结构上不可能撞批次上限。
    ///
    /// 最坏形状 = 12 条 × （名字 64 ＋ `…` ＋ ` — ` ＋ 说明 160 ＋ `…`）＋ 头 ＋ "另 N 项"。
    /// 这条断言就是那句话的机械版；构造得更大的输入在这条路上不存在——`read_skills` 交出来的
    /// 每个字段都已经过 [`one_line`] / [`cue`] 的界，渲染只 take [`MAX_LISTED`] 条。
    #[test]
    fn the_reading_is_bounded_so_it_can_never_need_the_fallback() {
        let display = one_line(&"名".repeat(200), MAX_NAME_BYTES).expect("非空");
        let hint = cue(&"说".repeat(200), MAX_HINT_BYTES).expect("非空");
        let entries: Vec<SkillEntry> = (0..MAX_LISTED)
            .map(|_| SkillEntry {
                id: "one".to_string(),
                display: display.clone(),
                hint: Some(hint.clone()),
            })
            .collect();
        let board = SkillsDesktop::new(Some(entries), ActivityOrigin::new());
        let empty = SkillsDesktop::new(Some(Vec::new()), ActivityOrigin::new());
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
        }
    }

    #[test]
    fn frontmatter_takes_name_and_trigger_in_all_three_shapes() {
        // ① 单行值。
        let (name, hint) =
            parse_frontmatter("---\nname: receipt-check\ndescription: 核回执时用\n---\n# 正文\n");
        assert_eq!(name.as_deref(), Some("receipt-check"));
        assert_eq!(hint.as_deref(), Some("核回执时用"));

        // ② 引号值。
        let (name, hint) =
            parse_frontmatter("---\nname: \"quoted\"\ndescription: '有人要你核时用'\n---\n");
        assert_eq!(name.as_deref(), Some("quoted"));
        assert_eq!(hint.as_deref(), Some("有人要你核时用"));

        // ③ 折叠块（外部技能常见的写法）：后面的缩进行按空格拼成一句。
        let (name, hint) = parse_frontmatter(
            "---\nname: folded\ndescription: >\n  有人要你核对\n  刚收到的东西时用。\n---\n# 正文\n",
        );
        assert_eq!(name.as_deref(), Some("folded"));
        assert_eq!(hint.as_deref(), Some("有人要你核对 刚收到的东西时用。"));

        // ④ 不认识的字段忽略，不打断解析。
        let (name, hint) = parse_frontmatter(
            "---\nlicense: MIT\nallowed-tools: [Bash]\nname: with-extra\ndescription: 核回执时用\nmetadata:\n  owner: x\n---\n",
        );
        assert_eq!(name.as_deref(), Some("with-extra"));
        assert_eq!(hint.as_deref(), Some("核回执时用"));

        // ⑤ 没有开头的 `---`、或有开头没收口 ⇒ 两格都没有（不猜）。
        assert_eq!(parse_frontmatter("name: x\n"), (None, None));
        assert_eq!(
            parse_frontmatter("---\nname: x\ndescription: y\n"),
            (None, None)
        );
    }

    #[test]
    fn a_trigger_is_cut_at_the_last_sentence_end_within_budget() {
        // 放不下 ⇒ 切在预算内最后一个句末（留下完整的一句，而不是半句）。
        let text = format!("{}。{}。", "甲".repeat(20), "乙".repeat(200));
        let cut = cue(&text, MAX_HINT_BYTES).expect("非空");
        assert_eq!(cut, format!("{}。", "甲".repeat(20)), "切在句末");
        // 预算内一个句末都没有 ⇒ 硬截并标记。
        let long = "描".repeat(100);
        let cut = cue(&long, MAX_HINT_BYTES).expect("非空");
        assert!(cut.ends_with('…'), "{cut}");
        assert!(
            cut.len() <= MAX_HINT_BYTES + '…'.len_utf8(),
            "{}",
            cut.len()
        );
        assert!(
            cut.trim_end_matches('…').chars().all(|ch| ch == '描'),
            "不许切出半个字"
        );
    }

    #[test]
    fn a_skill_reference_parses_all_none_and_a_directory_name_list() {
        assert_eq!(SkillRef::parse("all"), Ok(SkillRef::All));
        assert_eq!(SkillRef::parse(" ALL "), Ok(SkillRef::All));
        assert_eq!(SkillRef::parse("none"), Ok(SkillRef::None));
        assert_eq!(
            SkillRef::parse("release-note, ops,release-note"),
            Ok(SkillRef::Only(vec![
                "release-note".to_string(),
                "ops".to_string()
            ])),
            "逗号分隔、去空白、去重；顺序保持输入顺序"
        );
        assert_eq!(
            SkillRef::parse("release-note").expect("单项").as_text(),
            "release-note"
        );
        assert_eq!(SkillRef::All.as_text(), "all");
        assert_eq!(SkillRef::None.as_text(), "none");

        for bad in ["", "all,none", "a,,b", "a/b", ".."] {
            assert!(SkillRef::parse(bad).is_err(), "应当拒绝：{bad}");
        }
    }

    #[test]
    fn a_filtered_index_says_enabled_count_and_catalog_total() {
        let entries = vec![SkillEntry {
            id: "alpha".to_string(),
            display: "pretty-alpha".to_string(),
            hint: None,
        }];
        let reading = SkillsDesktop::with_index(
            Some(SkillIndex { entries, total: 2 }),
            ActivityOrigin::new(),
        )
        .reading()
        .expect("有索引就有读数");
        assert_eq!(
            reading,
            "skills: 1 项（已启用；技能区共 2 项；要翻看用 files_list，路径 skills/）\n  pretty-alpha"
        );

        let empty = SkillsDesktop::with_index(
            Some(SkillIndex {
                entries: Vec::new(),
                total: 2,
            }),
            ActivityOrigin::new(),
        )
        .reading()
        .expect("有索引就有读数");
        assert_eq!(
            empty, "skills: 0 项（没有启用任何技能；技能区共 2 项）",
            "有技能但一项都没启用，不许说成技能区为空"
        );
    }
}
