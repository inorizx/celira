//! Notes App：**第二个真实 App**，用它证明 App 的独立性不是一句接口声明。
//!
//! 它有自己的数据、动作、边界与失败语义，通过同一份 `AppPort` / `ActionPort` 窄契约接入：
//! OS 只多装配一个 App，`core.rs` 一行不改，它也不认识任何别的 App 的内部实现。
//!
//! ## 它拥有什么
//!
//! - **自己的存储**：`<环境根>/notes/<id>.note`，一个笔记一个文件，内容是这一条笔记的 JSON。
//!   正文只属于这里——**不进** Foundation 事实，不进经历，也不作为注入进入下一次 Run；
//! - **自己的身份**：`note-<n>` 由本 App 扫描自己的目录生成，随文件稳定保存。它不是数组位置，
//!   也不靠时间戳猜唯一性；
//! - **自己的动作**：`notes_create`（写到自己的环境里，**不需要任何授权**）、
//!   `notes_list` / `notes_read`（只读），以及 TASK_33 起的一对：`notes_pin` / `notes_unpin`
//!   （把纸条**贴到墙上 / 摘下来**——动的只是 `pinned` 这个标记，标题与正文一字不动；
//!   也是写自己的地盘，同样不需要授权），配一个共用的复核 `notes_verify_pinned`；
//!   此后又添了 **追加**（`notes_append`，原始记录那一侧）、**删除**（`notes_delete`，没有回收站）
//!   与 **就地改**（`notes_edit`，2026-10-14：整理/压缩那一侧——它与追加是两条路，理由见那一格的说明）。
//! - **自己的失败词汇**：见 [`NotesFailure`]。它与别的 App 的分类在**含义**上可以重合，
//!   但在**实现**上各归各的——没有共享枚举，也没有让一个 App 去读另一个 App 的模块。
//!
//! ## 它不是什么
//!
//! - 不是记忆系统：本 App 不判断什么值得记，不自动生成标题、摘要、记忆或交接内容，也不自动把
//!   旧笔记塞进新的活动。记录什么、什么时候记、要不要读回来，全是主体自己的选择；
//! - 不是通用存储框架：只有一个目录、一种文件格式、一条独占创建路径；没有索引、没有查询语言、
//!   没有跨 App 的存储协议；
//! - 不是沙盒：真正的边界是环境目录与能力，外加"受信目录"这个前提。
//!
//! ## 边界：写到谁的地盘
//!
//! 笔记落在**自己的环境目录**里（`<环境根>/notes/`）——那是主体的**自持物**，不是跨边界效应，
//! 所以这里**没有"授权"这一档**：过门控才能记事，荒谬（2026-09-12 字裁决）。
//! 需要授权的写是**写到别人的地盘**（宿主提供的材料目录），那条线在文件 App 那边，
//! 是两个独立类型：一个授权"哪个目录里能新建文本"，一个根本不需要授权。
//!
//! ## 它这一版**没有**什么
//!
//! 不改名、不搜索、不打标签、不排序业务字段、**没有回收站**（删就是除名，回执里如实声明）。
//! 正文只有一条增长路（新建 ＋ 往后接）＋ 一条整理路（就地改一处）。
//!
//! ⚠ **"删了就是删了"只说了一半**（2026-10-14 真机核实）：删掉的是**这一条笔记**——它从本 App
//! 的存储里除名，回执里这么说是诚实的。但主体**写下它时的原文**仍在主体的**经历**里（动作事实带
//! 正文，`history/` 按活动翻得到）⇒ 删得掉"随时读回来的那一份"，删不掉"主体写过这件事"。

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Capabilities,
    Capability, ContentBlock, ParamSpec,
};
use crate::foundation::{AgentId, OperationId, RunId};
use crate::pins_board::PinnedRow;
use crate::store::{EnvironmentReader, TracePayload, identity_at};

/// 这个 App 对外的稳定标识（展开动作的目标就是它）。
pub const NOTES_APP_ID: &str = "notes";

/// 存储子目录名（位于环境根之内）。
const STORAGE_DIR: &str = "notes";
/// 笔记文件后缀。
const NOTE_SUFFIX: &str = ".note";

/// 创建笔记能力的标识。
pub(crate) const CAPABILITY_CREATE_NOTE: &str = "notes_create";

/// 复核能力的标识。它与创建**同参数**，只回答"现在的真实状态是什么"。
pub(crate) const CAPABILITY_VERIFY_NOTE: &str = "notes_verify_note";
/// 列笔记能力的标识。
const CAPABILITY_LIST_NOTES: &str = "notes_list";
/// 读笔记能力的标识。
const CAPABILITY_READ_NOTE: &str = "notes_read";

/// 贴纸条能力的标识（TASK_33：主体策展、系统渲染里的"贴"这个动作）。
pub(crate) const CAPABILITY_PIN: &str = "notes_pin";
/// 摘纸条能力的标识。
pub(crate) const CAPABILITY_UNPIN: &str = "notes_unpin";
/// 贴/摘**共用**的复核能力标识：与两个动作同参数（`id`），只回答"这条笔记现在是不是贴着"。
///
/// ⚠️ 它**无法归因**到是贴还是摘（两个动作共用它）——与 `notes_verify_note` 的弱化同族，
/// 见模块文档与 [`NotesApp::pinned_verification_capability`]。
pub(crate) const CAPABILITY_VERIFY_PINNED: &str = "notes_verify_pinned";

/// 删掉一条笔记（TASK_76）：**直接除名**——主体是主人，处置得了自己的家当。
///
/// 与贴/摘同族（写在自己的地盘上、不需要授权），但它**不可逆**：没有回收站。
pub(crate) const CAPABILITY_DELETE: &str = "notes_delete";

/// 删除的复核（只读）：**现在还在不在**。与 `notes_verify_pinned` 同款——复核的是
/// **现在的状态**，不是"那一次动作"。
pub(crate) const CAPABILITY_VERIFY_DELETED: &str = "notes_verify_deleted";

/// 往一条已有笔记的正文**后面接一段**（TASK_80：主体自己列的第③条缺陷）。
///
/// ⚠ **它是"追加"，不是"改写"**：判据是仓库对主体外部空间那四条职责里的两条——**可恢复、
/// 不破坏原始历史**。真机上的替代做法是"新建一条 ＋ 摘掉旧的那条"（note-4 → note-5 →
/// note-8 → note-9，正文 95% 逐字重复，一夜 25 条），而那个做法要**靠标题里带计数器**才能表达
/// "同一条事的第几次"。有了追加，那个计数器就不必要了。
pub(crate) const CAPABILITY_APPEND: &str = "notes_append";

/// 追加的复核（只读，**与追加同参数**）：那条笔记**现在的正文是不是以这段结尾**。
///
/// ⚠ 与 `notes_verify_note` 的弱化同族、而且更窄：它认的是**结尾**，因此同一条被追加两次、
/// 两次内容相同 ⇒ **无法归因到哪一次**；两次内容不同 ⇒ 只认最后一次。
pub(crate) const CAPABILITY_VERIFY_APPENDED: &str = "notes_verify_appended";

/// 就地改（2026-10-14 主人裁，承主体自己那份清单的第 3 条）：把一条笔记正文里**恰好一处**
/// 换成另一段——整理、压缩、改错字。
///
/// ## 它与「追加」是两条路，两条都留着（这是本刀最要紧的一句）
///
/// [`CAPABILITY_APPEND`] 是**原始记录**那一侧：只往后接、已有的字一个都不动（那条"可恢复、
/// 不破坏原始历史"的职责）。这一件是**"我现在记着的东西"**那一侧：允许主体把一条读到 17 KB 的
/// 笔记整理成短的。判据不是"改了会不会丢历史"——**历史不在这里**：主体写下它时的原文与每一次改动
/// 都落在主体的**经历**里（动作事实带正文，`history/` 按活动翻得到，2026-10-14 真机核实：
/// 一条 `notes_create` 的记录里就带着 2025 字节正文）。⇒ 这一件改的是**这一条现在的内容**，
/// 「不破坏原始历史」那条职责仍然成立。
pub(crate) const CAPABILITY_EDIT: &str = "notes_edit";

/// 就地改的复核（只读，**与它同参数**）：现在这份正文里，要被换掉的那段还在不在、换成的那段在不在。
pub(crate) const CAPABILITY_VERIFY_EDIT: &str = "notes_verify_edited";

/// 创建动作的参数名。
const PARAM_TITLE: &str = "title";
const PARAM_TEXT: &str = "text";
/// 读动作的参数名（笔记身份）。
const PARAM_ID: &str = "id";
/// 就地改的参数名（与文件 App 的 `files_edit` **同名**：一个概念一个名称，两个面一套叫法）。
const PARAM_OLD_TEXT: &str = "old_text";
const PARAM_NEW_TEXT: &str = "new_text";

/// 一条笔记的标题上限（它要出现在列表里，所以必须短而且单行）。
pub const MAX_TITLE_BYTES: usize = 256;
/// 一条笔记的**正文**上限：与既有单次文件读取上限一致。
///
/// ⚠ **它量的是正文，不是文件**（2026-10-13 统一口径）：两个写入口（建 / 追加）判的是它，读回来判的
/// 也是它。从前读那一侧判的是**整份文件**（含 JSON 外壳）⇒ 正文贴着上限的那一条**写得进去、
/// 读不回来**——同一个上限量了两件事。上限的用途是"一次动作往主体的上下文里放多少字"，
/// 外壳是**存储开销**，不是主体要读的东西。
pub const MAX_NOTE_BYTES: usize = 64 * 1024;

/// **存储侧的内存护栏**（不是主体看到的那个上限）：一条笔记**文件**最多可能多大。
///
/// 它只用来决定"最多读多少字节进内存"，**不参与任何失败分类**。取值按 JSON 转义的**最坏情况**算：
/// 正文里每个字符都可能被写成 `\uXXXX`（6 字节）⇒ 正文上限 ×6，标题同理，再加外壳与键名的零头。
const MAX_NOTE_FILE_BYTES: usize = MAX_NOTE_BYTES * 6 + MAX_TITLE_BYTES * 6 + 128;
/// 一次列笔记最多返回多少条（按身份机械排序后的前若干条）。
pub const MAX_LISTED_NOTES: usize = 100;

/// 本 App 自己的失败分类。
///
/// 它与别的 App 的分类在含义上可以重合（都有"太长了""不是合法路径"这类话），但**类型与实现
/// 各归各的**：没有共享枚举，也没有跨 App 的引用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotesFailure {
    /// 参数不齐、参数名不认识，或取值不合法（标题为空、标题带控制字符、正文超上限）。
    InvalidParameter,
    /// 笔记身份不合法：不是本 App 生成的那种形状，或带了分隔符/父路径之类的东西。
    InvalidId,
    /// 这个身份已经被占用：本次创建没有发生，也没有覆盖任何已有笔记。
    IdTaken,
    /// 目标笔记不存在。
    NotFound,
    /// 笔记在、但它**没有贴着**（只用于贴纸复核的"明确失败"：没贴 ≠ 找不到）。
    NotPinned,
    /// 笔记**还在**（只用于删除复核的"明确失败"：还在 ≠ 找不到——这两件事必须分开说）。
    StillPresent,
    /// 内容读不出来（不是合法 UTF-8，或存储层报错）。
    Unreadable,
    /// 长度超限（正文或标题）。
    TooLarge,
    /// 其他 I/O 失败。
    Io,
}

impl NotesFailure {
    /// 稳定分类标记（进 `ActionOutcome`，Core 只搬运不解释）。
    pub fn as_str(self) -> &'static str {
        match self {
            NotesFailure::InvalidParameter => "invalid-parameter",
            NotesFailure::InvalidId => "invalid-id",
            NotesFailure::IdTaken => "note-id-taken",
            NotesFailure::NotFound => "not-found",
            NotesFailure::NotPinned => "not-pinned",
            NotesFailure::StillPresent => "still-present",
            NotesFailure::Unreadable => "unreadable",
            NotesFailure::TooLarge => "too-large",
            NotesFailure::Io => "io",
        }
    }

    /// 从稳定标记还原（解码用）。认不出的取值就是 `None`——**不猜**，也不就近映射
    /// （调用方按 fail-closed 处理）。
    ///
    /// **标记表就是权威**：`as_str` 与它**逐条对称**，有测试钉住（`tests/action_failure_wordlist.rs`）。
    /// 新增取值必须**同一刀**补在这里——这一格落在盘上的**行动结果**里，缺了解码端就是
    /// 「写得进去、读不出来」。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Option<Self> {
        Some(match text {
            "invalid-parameter" => NotesFailure::InvalidParameter,
            "invalid-id" => NotesFailure::InvalidId,
            "note-id-taken" => NotesFailure::IdTaken,
            "not-found" => NotesFailure::NotFound,
            "not-pinned" => NotesFailure::NotPinned,
            "still-present" => NotesFailure::StillPresent,
            "unreadable" => NotesFailure::Unreadable,
            "too-large" => NotesFailure::TooLarge,
            "io" => NotesFailure::Io,
            _ => return None,
        })
    }
}

/// 一个 Notes App：绑定它所属的环境根。
///
/// **它的写是自己地盘上的写**：笔记落在 `<环境根>/notes/`，那是主体自持物，不是跨边界效应，
/// 因此**没有**宿主授权这一档（2026-09-12 字裁决：过门控才能记事=荒谬）。
/// 跨边界写（宿主提供的材料目录）的授权在文件 App 那边，是另一个类型、另一条线。
#[derive(Debug)]
pub struct NotesApp {
    /// 本 App 的存储目录（`<环境根>/notes`）。它在**第一条笔记真正创建成功时**才出现，
    /// 只读的活动不会因为装配而留下任何东西。
    storage: PathBuf,
}

impl NotesApp {
    /// 装配一个 Notes App。
    ///
    /// 环境根必须已存在、是目录、不是符号链接或 reparse point：Notes 的存储挨着环境记录，
    /// 因此那些记录所在的位置必须先被确认为一个普通目录。
    pub fn open(environment_root: impl Into<PathBuf>) -> Result<Self, NotesFailure> {
        let environment_root = environment_root.into();
        let metadata = fs::symlink_metadata(&environment_root).map_err(io_failure)?;
        if metadata.file_type().is_symlink() {
            return Err(NotesFailure::InvalidParameter);
        }
        if !metadata.is_dir() {
            return Err(NotesFailure::InvalidParameter);
        }
        let canonical = fs::canonicalize(&environment_root).map_err(io_failure)?;
        Ok(Self {
            storage: canonical.join(STORAGE_DIR),
        })
    }

    /// 本 App 的存储目录。只用于诊断与测试，**不进入任何 Model 可见内容**。
    pub fn storage_root(&self) -> &Path {
        &self.storage
    }

    /// 读侧两项能力的说明（顺序固定）。
    fn read_capabilities() -> Capabilities {
        Capabilities::of(vec![
            Capability::new(
                ActionId::new(CAPABILITY_LIST_NOTES),
                "列出你自己的笔记（不递归，按身份排序）；只给身份与标题，不给正文",
                vec![],
            )
            .with_details(format!(
                "上限：一次最多 {MAX_LISTED_NOTES} 条，超出时首行明确标记 `truncated`。\
                 身份（`note-<n>`）是 `{CAPABILITY_READ_NOTE}` / `{CAPABILITY_PIN}` / \
                 `{CAPABILITY_UNPIN}` 等动作要用的那个。",
            )),
            Capability::new(
                ActionId::new(CAPABILITY_READ_NOTE),
                "读一条自己笔记的标题与正文",
                vec![ParamSpec::new(PARAM_ID, true, "笔记身份（形如 note-1）")],
            )
            .with_details(format!(
                "上限：正文最多 {} KiB，超限拒绝（不截断）；身份取自 `{CAPABILITY_LIST_NOTES}`。",
                MAX_NOTE_BYTES / 1024
            )),
        ])
    }

    /// 追加能力的说明（**恒在场**：往自己的笔记里续一段不需要授权）。
    ///
    /// ⚠️ **它不替你加分隔符**：接法是**逐字节接在后面**（`旧正文 + 这一段`）。要另起一行，把换行
    /// 写进 `text` 里——那是你的排版，不是系统替你判断的。这条说明与"复核认的是结尾"配套：
    /// 只有这样，复核才能逐字节回答"现在的正文是不是以这段结尾"。
    fn append_capability() -> Capability {
        Capability::new(
            ActionId::new(CAPABILITY_APPEND),
            "往一条已有笔记的正文后面接一段（标题与贴纸标记一字不动）",
            vec![
                ParamSpec::new(PARAM_ID, true, "笔记身份（形如 note-1，取自 notes_list）"),
                ParamSpec::new(
                    PARAM_TEXT,
                    true,
                    "要接在后面的那一段（非空；要另起一行就把换行写进来）",
                ),
            ],
        )
        .with_details(format!(
            "接法：逐字节接在正文后面（`旧正文 + 这一段`），不替你加分隔符、不排版、不改标题\
             ——要另起一行就把 `\\n` 写进 `text`。**标题与 `pinned` 一字不动**。**上限**：接完之后的\
             正文最多 {} KiB，超限**拒绝且旧的正文原样保留**（不截断，与创建同款）。**空增量拒绝**\
             （那是一次什么都没写的动作）。身份不存在 ⇒ `not-found`；读不懂那一条 ⇒ `unreadable`。\
             **它不改写、不删除**：已有的字一个都不会消失（要改就地改，这一版没有那个动词）。",
            MAX_NOTE_BYTES / 1024
        ))
    }

    /// 追加的复核能力说明（**恒在场**，与追加能力同参数）。
    fn appended_verification_capability() -> Capability {
        Capability::new(
            ActionId::new(CAPABILITY_VERIFY_APPENDED),
            "复核一条笔记现在的正文是否以某一段结尾（只读）",
            vec![
                ParamSpec::new(PARAM_ID, true, "笔记身份（形如 note-1）"),
                ParamSpec::new(PARAM_TEXT, true, "当初接在后面的那一段"),
            ],
        )
        .with_details(
            "三分：这条笔记现在的正文确实以这段结尾 = 成功；那条在、但结尾不是它 =\
             明确失败 `not-found`（与 `notes_verify_note` 同一句话：没有一条符合这份参数的笔记）；\
             身份不存在或读不出来 = **无法确认**。⚠️ **弱在归因**：它认的是**结尾**，因此同一条被\
             追加两次、两次内容相同 ⇒ 它**分不清是哪一次**；两次内容不同 ⇒ 只认最后一次。\
             参数与追加**完全相同**；它回答的是**现在**。",
        )
    }

    /// 就地改能力的说明（**恒在场**：改自己的笔记不需要授权）。
    fn edit_capability() -> Capability {
        Capability::new(
            ActionId::new(CAPABILITY_EDIT),
            "把自己一条笔记的正文里恰好一处换成另一段（整理、压缩、改错字）",
            vec![
                ParamSpec::new(PARAM_ID, true, "笔记身份（形如 note-1）"),
                ParamSpec::new(PARAM_OLD_TEXT, true, "要被换掉的那一段（非空）"),
                ParamSpec::new(PARAM_NEW_TEXT, true, "换成什么（允许空串＝把那一段删掉）"),
            ],
        )
        .with_details(format!(
            "`old_text` 必须恰好出现一次：0 次 ⇒ `not-found`；多次 ⇒ `invalid-parameter`\
             ——两种都一个字节不动，也不猜你想改哪一处。换后正文最多 {} KiB，超限整体拒绝（不截断）。\
             标题、身份、贴纸标记一字不动；不改名、不删除、不追加。它与 `notes_append` 是两条路：\
             追加只往后接、已有的字一个都不动（原始记录那一侧）；这一件改的是这一条现在的内容\
             （整理那一侧）。你写下它时的原文与每一次改动都在你的经历里（`history/` 按活动翻得到）。",
            MAX_NOTE_BYTES / 1024
        ))
    }

    /// 就地改的复核能力说明（**恒在场**，与它同参数）。
    fn edited_verification_capability() -> Capability {
        Capability::new(
            ActionId::new(CAPABILITY_VERIFY_EDIT),
            "复核一次就地改现在的状态（与 `notes_edit` 同参数，只读）",
            vec![
                ParamSpec::new(PARAM_ID, true, "笔记身份（形如 note-1）"),
                ParamSpec::new(PARAM_OLD_TEXT, true, "被换掉的那一段"),
                ParamSpec::new(PARAM_NEW_TEXT, true, "换成了什么"),
            ],
        )
        .with_details(
            "三分是现在时：要被换掉的那段已经不在了、换成的那段在 ⇒ 成功；那段还在 ⇒ 明确失败\
             `still-present`（要被换掉的还在，与删除复核同一句话）；这一条不在、读不出来、不是本 App 的\
             形状，或两份都不在 ⇒ 无法确认。参数与 `notes_edit` 完全相同；它回答的是现在。",
        )
    }

    /// 创建能力的说明（**恒在场**：主体写自己的笔记不需要授权）。
    fn creation_capability() -> Capability {
        Capability::new(
            ActionId::new(CAPABILITY_CREATE_NOTE),
            "新建一条自己的笔记（只创建：不覆盖、不追加、不改名、不删除）",
            vec![
                ParamSpec::new(PARAM_TITLE, true, "笔记标题（单行、非空、≤256 字节）"),
                ParamSpec::new(PARAM_TEXT, true, "笔记正文，允许为空串"),
            ],
        )
        .with_details(format!(
            "边界：标题单行、非空、≤ {MAX_TITLE_BYTES} 字节且不含控制字符；正文最多 {} KiB，\
             允许空串；**身份由本 App 生成**（`note-<n>`，不接受调用方指定）。本版**只创建**：不覆盖、\
             不追加、不改名、不删除、不搜索。**失败形状**：创建之后写入或同步失败 ⇒ **无法确认**。",
            MAX_NOTE_BYTES / 1024
        ))
    }

    /// 复核能力的说明（**恒在场**，与创建能力一起）。
    ///
    /// ⚠️ **这个复核天生更弱**，弱在归因：创建时笔记身份（`note-<n>`）是**执行时抢的**，不在参数
    /// 里，所以同参数的一次复核只能回答"**现在至少存在一条**同标题同正文的笔记"，**无法**断言
    /// "就是那一次创建留下的那一条"——另一条内容相同的笔记会让它同样报成功。这个弱化写在这里，
    /// 不靠读者推断。
    ///
    /// 参数与 [`NotesApp::creation_capability`] **完全相同**；它是**只读**的，不产生任何效果，
    /// 而且回答的是**现在**（不是"当时发生了什么"）。
    fn verification_capability() -> Capability {
        Capability::new(
            ActionId::new(CAPABILITY_VERIFY_NOTE),
            "复核一条笔记的当前状态：现在至少存在一条同标题同正文的 = 成功（只读）",
            vec![
                ParamSpec::new(PARAM_TITLE, true, "当初创建的笔记标题"),
                ParamSpec::new(PARAM_TEXT, true, "当初创建的笔记正文"),
            ],
        )
        .with_details(
            "三分：现在至少存在一条同标题同正文的 = 成功；确定一条都没有（含存储目录不存在）=\
             明确失败；读不出来或状态可疑 = **无法确认**。⚠️ **弱在归因**：笔记身份是**执行时抢的**、\
             不在参数里，因此它**无法**断言「就是那一次创建留下的那一条」——另一条内容相同的笔记会让它\
             同样报成功。参数与创建**完全相同**；它回答的是**现在**，不是「当时发生了什么」。",
        )
    }

    /// 标上能力的说明（**恒在场**：标自己的笔记不需要授权）。
    fn pin_capability() -> Capability {
        Capability::new(
            ActionId::new(CAPABILITY_PIN),
            "给一条笔记标上 `pinned`（重复标 = 仍然标着，不报错）",
            vec![ParamSpec::new(
                PARAM_ID,
                true,
                "笔记身份（形如 note-1，取自 notes_list）",
            )],
        )
        .with_details(
            "只动 `pinned` 这一个标记：标题与正文一字不动；重复标 = 仍然标着（不报错）。\
             视野里只渲染预览（≤60 字节），全文仍走 `notes_read`。标着的笔记跨清醒期保留：\
             每次清醒期开场，标上的那几条的预览会出现在 `pinned-notes` 那一块；那是开场的快照，\
             本次清醒期内新标的下一次开场才摆出来。复核用 `notes_verify_pinned`。",
        )
    }

    /// 取消标记能力的说明（**恒在场**）。
    fn unpin_capability() -> Capability {
        Capability::new(
            ActionId::new(CAPABILITY_UNPIN),
            "取消一条笔记的 `pinned` 标记（重复取消 = 仍然没标着，不报错）",
            vec![ParamSpec::new(PARAM_ID, true, "笔记身份（形如 note-1）")],
        )
        .with_details(
            "只动 `pinned` 这一个标记：内容一字不动；重复取消 = 仍然没标着（不报错）。标/取消\
             **共用**同一个复核（`notes_verify_pinned`），因此它只能回答「现在标着没有」，\
             不能归因到某一次标或取消。",
        )
    }

    /// 贴/摘**共用**的复核能力说明（**恒在场**；与两个动作同参数）。
    ///
    /// ⚠️ **它无法归因**：贴与摘共用它，因此它只能回答「这条笔记**现在是**不是贴着」，
    /// **不能**声称"那一次贴（或摘）生效了"。与 [`NotesApp::verify_note`] 的弱化同族。
    fn pinned_verification_capability() -> Capability {
        Capability::new(
            ActionId::new(CAPABILITY_VERIFY_PINNED),
            "复核一条笔记现在的贴纸状态：贴着 = 成功（只读）",
            vec![ParamSpec::new(PARAM_ID, true, "笔记身份（形如 note-1）")],
        )
        .with_details(
            "三分：贴着 ⇒ 成功；没贴 ⇒ 明确失败（`not-pinned`，与「找不到这条笔记」不是一回事）；\
             读不出来 ⇒ **无法确认**。⚠️ **无法归因**：贴与摘共用它，所以它只能回答「这条笔记**现在是**\
             不是贴着」，**不能**声称「那一次贴（或摘）生效了」。",
        )
    }

    /// 这个实例当前声明的能力：读侧两项，加上**那一项**写能力与它的复核能力。
    ///
    /// 写能力**恒在场**——自己的笔记不需要任何人先授权。
    fn available_capabilities(&self) -> Capabilities {
        let mut items = Self::read_capabilities().items().to_vec();
        items.push(Self::creation_capability());
        items.push(Self::verification_capability());
        items.push(Self::pin_capability());
        items.push(Self::unpin_capability());
        items.push(Self::pinned_verification_capability());
        items.push(Self::delete_capability());
        items.push(Self::deleted_verification_capability());
        items.push(Self::append_capability());
        items.push(Self::appended_verification_capability());
        items.push(Self::edit_capability());
        items.push(Self::edited_verification_capability());
        Capabilities::of(items)
    }

    /// 新建一条笔记。
    ///
    /// 顺序：校验标题与正文 → 生成身份 → `create_new` 独占创建 → 写入 → 同步。
    /// 从"文件已经被创建"那一刻起，任何失败都不再说"没有发生"：写入或同步失败一律报
    /// **无法确认**，文件留在盘上由主体自己判断——本 App 不删除、不补写、不重试。
    ///
    /// **没有授权这一档**：笔记写在自己的环境目录里（自持物），不是跨边界写。
    fn create_note(
        &self,
        _agent: &AgentId,
        _operation: &OperationId,
        params: &ActionParams,
    ) -> ActionOutcome {
        for (name, _) in params.entries() {
            if name != PARAM_TITLE && name != PARAM_TEXT {
                return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
            }
        }
        let (Some(title), Some(text)) = (params.get(PARAM_TITLE), params.get(PARAM_TEXT)) else {
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        };
        if !valid_title(title) {
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        }
        if text.len() > MAX_NOTE_BYTES {
            return ActionOutcome::failure(NotesFailure::TooLarge.as_str());
        }

        if let Err(failure) = self.ensure_storage() {
            return ActionOutcome::failure(failure.as_str());
        }
        let id = match self.next_identity() {
            Ok(id) => id,
            Err(failure) => return ActionOutcome::failure(failure.as_str()),
        };

        // **独占创建**：身份靠 `create_new` 抢，不用"先 exists 再普通写入"冒充原子拒绝。
        let path = self.path_of(&id);
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) => {
                // 身份被别人抢走了：本次创建没有发生，也没有覆盖任何东西。本版不自动改号。
                let failure = if error.kind() == std::io::ErrorKind::AlreadyExists {
                    NotesFailure::IdTaken
                } else {
                    io_failure(error)
                };
                return ActionOutcome::failure(failure.as_str());
            }
        };

        let body = encode_note(title, text, false);
        // 文件已经存在了。从这里开始的失败都不再说"没有发生"。
        if std::io::Write::write_all(&mut file, body.as_bytes()).is_err() {
            return ActionOutcome::unknown();
        }
        if file.sync_all().is_err() {
            return ActionOutcome::unknown();
        }

        ActionOutcome::success(vec![ContentBlock::new(
            "created",
            format!(
                "{id} (title {} bytes, text {} bytes)",
                title.len(),
                text.len()
            ),
        )])
    }

    /// 复核一条笔记**现在的真实状态**（不是"当时发生了什么"）。
    ///
    /// 顺序与 [`NotesApp::create_note`] 完全对应（参数与标题形状 → 上限），
    /// 然后**只读**地扫一遍自己的存储。三分：
    ///
    /// - **成功**：现在**至少存在一条**同标题同正文的笔记；
    /// - **明确失败**（`NotFound`）：确定一条都没有（存储目录不存在也算"没有"）；
    /// - **无法确认**（[`ActionOutcome::unknown`]）：存储读不出来（权限、损坏、不可解释的文件）。
    ///
    /// ⚠️ **它无法归因**：笔记身份是执行时抢的、不在参数里，所以这里只能回答"现在有没有一条
    /// 符合这份参数的笔记"，**不能**声称"那一次创建成功了"。同标题同正文的另一条笔记同样会让
    /// 它报成功——这个弱化是类别本身带来的（见 [`NotesApp::verification_capability`] 的说明）。
    fn verify_note(
        &self,
        _agent: &AgentId,
        _operation: &OperationId,
        params: &ActionParams,
    ) -> ActionOutcome {
        for (name, _) in params.entries() {
            if name != PARAM_TITLE && name != PARAM_TEXT {
                return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
            }
        }
        let (Some(title), Some(text)) = (params.get(PARAM_TITLE), params.get(PARAM_TEXT)) else {
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        };
        if !valid_title(title) {
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        }
        if text.len() > MAX_NOTE_BYTES {
            return ActionOutcome::failure(NotesFailure::TooLarge.as_str());
        }

        let notes = match self.read_all() {
            Ok(notes) => notes,
            // 读不出来就是读不出来：不许把"没读到"说成"确定不存在"。
            Err(_) => return ActionOutcome::unknown(),
        };
        let found = notes
            .iter()
            .any(|note| note.title == title && note.text == text);
        if !found {
            return ActionOutcome::failure(NotesFailure::NotFound.as_str());
        }

        ActionOutcome::success(vec![ContentBlock::new(
            "verified",
            format!(
                "at least one note now matches this title and body ({} notes scanned)",
                notes.len()
            ),
        )])
    }

    /// 列出这个 Agent 自己的笔记：身份 + 标题，按身份机械排序，有界。
    fn delete_capability() -> Capability {
        Capability::new(
            ActionId::new(CAPABILITY_DELETE),
            "删掉一条笔记（不可恢复：直接除名，没有回收站）",
            vec![ParamSpec::new(PARAM_ID, true, "笔记身份（形如 note-1）")],
        )
        .with_details(
            "查无此条 ⇒ 明确拒绝（`not-found`）：删除是破坏性的，不装成功——要核对先 \
             `notes_list`。标着的那条也照删：下一场开场就不会再有它。\
             复核用 `notes_verify_deleted`（只读：还在不在）。\
             ⚠ 删得掉的是这一条（随时读回来的那一份）；你写下它时的原文仍在你的经历里\
             （`history/` 按活动翻得到）——「不可恢复」说的是这一条，不是那件事。",
        )
    }

    fn deleted_verification_capability() -> Capability {
        Capability::new(
            ActionId::new(CAPABILITY_VERIFY_DELETED),
            "复核一条笔记现在的存在状态（只读）",
            vec![ParamSpec::new(PARAM_ID, true, "笔记身份（形如 note-1）")],
        )
        .with_details(
            "三分：不在了 ⇒ 成功（这一次删除生效）；还在 ⇒ 明确失败（`still-present`，\
             与「找不到这条笔记」不是一回事）；读不出来 ⇒ **无法确认**。",
        )
    }

    /// 删掉一条笔记——**直接除名**。
    ///
    /// **查无此条 ⇒ 明确拒绝**（`not-found`，并随拒绝附一句"拿 `notes_list` 核对"）：
    /// 删除是破坏性操作，`not-found` 必须当失败报，**不许装成功**；幂等重放走 `OperationId`
    /// 那套既有机制（Core 不会把一次已确认的操作再派一遍），不靠"删不存在的也算成功"来凑。
    ///
    /// **没有回收站**：删就是除名，回执里**如实声明不可恢复**——不可逆的决定也是主体的决定，
    /// 回收站是替主体管手。贴着的那条也照删：纸条板是**装配期一次性只读**的那一份，
    /// 下一场它自然就不在了（没有孤儿条目）。
    fn delete_note(&self, params: &ActionParams) -> ActionOutcome {
        for (name, _) in params.entries() {
            if name != PARAM_ID {
                return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
            }
        }
        let Some(id) = params.get(PARAM_ID) else {
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        };
        if !valid_identity(id) {
            return ActionOutcome::failure(NotesFailure::InvalidId.as_str());
        }
        let path = self.path_of(id);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return ActionOutcome::failure_with_content(
                    NotesFailure::NotFound.as_str(),
                    vec![ContentBlock::new(
                        "notes",
                        format!("{id} 不在（`{CAPABILITY_LIST_NOTES}` 逐条列出的才是现有的）"),
                    )],
                );
            }
            Err(error) => return ActionOutcome::failure(io_failure(error).as_str()),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return ActionOutcome::failure(NotesFailure::Unreadable.as_str());
        }
        if let Err(error) = fs::remove_file(&path) {
            return ActionOutcome::failure(io_failure(error).as_str());
        }
        ActionOutcome::success(vec![ContentBlock::new(
            "notes",
            format!(
                "{id} 已删除、不可恢复。\n（这一条从自己的笔记存储里除名了：没有回收站。\
                 你写下它时的原文仍在你的经历里——`history/` 按活动翻得到。）"
            ),
        )])
    }

    /// 复核一条笔记**现在的**存在状态（`notes_delete` 的复核；只读）。
    fn verify_deleted(&self, params: &ActionParams) -> ActionOutcome {
        for (name, _) in params.entries() {
            if name != PARAM_ID {
                return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
            }
        }
        let Some(id) = params.get(PARAM_ID) else {
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        };
        if !valid_identity(id) {
            return ActionOutcome::failure(NotesFailure::InvalidId.as_str());
        }
        match fs::symlink_metadata(self.path_of(id)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ActionOutcome::success(vec![ContentBlock::new(
                    "notes",
                    format!("{id} 不在了（这一次删除生效）"),
                )])
            }
            Ok(_) => ActionOutcome::failure(NotesFailure::StillPresent.as_str()),
            Err(error) => ActionOutcome::failure(io_failure(error).as_str()),
        }
    }

    /// 往一条已有笔记的正文后面**接一段**（`notes_append`）。
    ///
    /// **逐字节接在后面**（`旧正文 + 这一段`）：不替你加分隔符、不排版、不改标题、不动 `pinned`。
    /// 判据是"你的排版归你"——而这也让复核能逐字节回答"现在是不是以这段结尾"。
    ///
    /// 四条拒绝都在**动手之前**（拒绝之后一个字节都不写、旧正文原样保留）：参数不齐 / 身份不合法 /
    /// **空增量** / 接完超上限。身份不存在 ⇒ `not-found`；那条在但读不懂 ⇒ `unreadable`。
    /// 打开之后才发生的失败（写、同步）⇒ **无法确认**（与贴/摘同款：不删、不补写、不重试）。
    fn append_note(&self, params: &ActionParams) -> ActionOutcome {
        for (name, _) in params.entries() {
            if name != PARAM_ID && name != PARAM_TEXT {
                return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
            }
        }
        let (Some(id), Some(extra)) = (params.get(PARAM_ID), params.get(PARAM_TEXT)) else {
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        };
        if !valid_identity(id) {
            return ActionOutcome::failure(NotesFailure::InvalidId.as_str());
        }
        if extra.is_empty() {
            // **空增量拒绝**：那是一次"什么都没写"的动作——给它成功回执，就是让它以为写进去了。
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        }
        let path = self.path_of(id);
        let body = match fs::read_to_string(&path) {
            Ok(body) => body,
            Err(error) => return ActionOutcome::failure(io_failure(error).as_str()),
        };
        let Some((title, text, pinned)) = decode_note(&body) else {
            return ActionOutcome::failure(NotesFailure::Unreadable.as_str());
        };
        let joined = format!("{text}{extra}");
        if joined.len() > MAX_NOTE_BYTES {
            // 超限 ⇒ **拒绝、不写、不截断**，旧正文原样保留（与创建同款）。
            return ActionOutcome::failure(NotesFailure::TooLarge.as_str());
        }
        let before = text.len();
        let next = encode_note(&title, &joined, pinned);
        let mut file = match OpenOptions::new().write(true).truncate(true).open(&path) {
            Ok(file) => file,
            // 文件已经被打开过了：从这里开始的失败都不再说"没有发生"。
            Err(_) => return ActionOutcome::unknown(),
        };
        if std::io::Write::write_all(&mut file, next.as_bytes()).is_err() {
            return ActionOutcome::unknown();
        }
        if file.sync_all().is_err() {
            return ActionOutcome::unknown();
        }
        ActionOutcome::success(vec![ContentBlock::new(
            "appended",
            format!(
                "{id} 的正文接上了：{before} → {} 字节（＋{}）；标题与贴纸标记一字未动。",
                joined.len(),
                extra.len()
            ),
        )])
    }

    /// 复核一条笔记**现在的正文是否以某一段结尾**（`notes_append` 的复核；只读三分）。
    ///
    /// 是 ⇒ **成功**；那条在、结尾不是它 ⇒ **明确失败 `not-found`**（与 `notes_verify_note` 同一句
    /// 话：没有一条符合这份参数的笔记）；这一条不在、读不出来、或不是本 App 的形状 ⇒ **无法确认**。
    ///
    /// ⚠️ 它与追加**同参数**，因此它**分不清**同一条被追加两次（内容相同）还是只有一次
    /// （见 [`NotesApp::appended_verification_capability`]）。
    fn verify_appended(&self, params: &ActionParams) -> ActionOutcome {
        for (name, _) in params.entries() {
            if name != PARAM_ID && name != PARAM_TEXT {
                return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
            }
        }
        let (Some(id), Some(extra)) = (params.get(PARAM_ID), params.get(PARAM_TEXT)) else {
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        };
        if !valid_identity(id) {
            return ActionOutcome::failure(NotesFailure::InvalidId.as_str());
        }
        if extra.is_empty() {
            // 空增量复核没有意义（任何正文都以空串结尾）⇒ 明确拒绝，不给一个恒真的答案。
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        }
        let path = self.path_of(id);
        match fs::read_to_string(&path) {
            // 这一条不在 ⇒ "没有任何正文以这段结尾"是**确定**的 ⇒ 与"结尾不对"同一个答案。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ActionOutcome::failure(NotesFailure::NotFound.as_str())
            }
            // 其它读失败 ⇒ **不声称**（三态里的第三档）。
            Err(_) => ActionOutcome::unknown(),
            Ok(body) => match decode_note(&body) {
                None => ActionOutcome::unknown(),
                Some((_, text, _)) if text.ends_with(extra) => {
                    ActionOutcome::success(vec![ContentBlock::new(
                        "appended",
                        format!("{id} 的正文确实以这段结尾"),
                    )])
                }
                Some(_) => ActionOutcome::failure(NotesFailure::NotFound.as_str()),
            },
        }
    }

    /// 就地改一条笔记的正文里**恰好一处**（`notes_edit`）。
    ///
    /// 四条拒绝都在**动手之前**（拒绝之后一个字节都不写，旧正文原样保留）：参数不齐 / 身份不合法 /
    /// `old_text` 是空串 / 出现**不止一处**（含糊的替换不做；0 处 ⇒ `not-found`）/ 换后超上限。
    /// 打开之后才发生的失败（写、同步）⇒ **无法确认**（不删、不补写、不重试）。
    ///
    /// 身份、标题与贴纸标记**一字不动**——这一件只动正文里被点名的那一段。
    fn edit_note(&self, params: &ActionParams) -> ActionOutcome {
        for (name, _) in params.entries() {
            if name != PARAM_ID && name != PARAM_OLD_TEXT && name != PARAM_NEW_TEXT {
                return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
            }
        }
        let (Some(id), Some(old_text), Some(new_text)) = (
            params.get(PARAM_ID),
            params.get(PARAM_OLD_TEXT),
            params.get(PARAM_NEW_TEXT),
        ) else {
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        };
        if !valid_identity(id) {
            return ActionOutcome::failure(NotesFailure::InvalidId.as_str());
        }
        // 空串会在每个位置匹配，那不是"换掉某一段"：明确拒绝，不解释成"插到最前面"。
        // `new_text` 空串则合法——换成空就是把那一段删掉。
        if old_text.is_empty() {
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        }
        let path = self.path_of(id);
        let body = match fs::read_to_string(&path) {
            Ok(body) => body,
            Err(error) => return ActionOutcome::failure(io_failure(error).as_str()),
        };
        let Some((title, text, pinned)) = decode_note(&body) else {
            return ActionOutcome::failure(NotesFailure::Unreadable.as_str());
        };
        // 唯一性：先取第一处，再多看一处。两处都只在**内存里**判定，盘上一个字节都没动。
        let mut matches = text.match_indices(old_text);
        let Some((start, _)) = matches.next() else {
            return ActionOutcome::failure(NotesFailure::NotFound.as_str());
        };
        if matches.next().is_some() {
            // 出现不止一处 ⇒ 不猜"你想改哪一处"（与 `files_edit` 同款：含糊的替换不做）。
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        }
        let mut next =
            String::with_capacity(text.len() + new_text.len().saturating_sub(old_text.len()));
        next.push_str(&text[..start]);
        next.push_str(new_text);
        next.push_str(&text[start + old_text.len()..]);
        if next.len() > MAX_NOTE_BYTES {
            // 超限 ⇒ **拒绝、不写、不截断**，旧正文原样保留（与创建、追加同款）。
            return ActionOutcome::failure(NotesFailure::TooLarge.as_str());
        }
        let before = text.len();
        let encoded = encode_note(&title, &next, pinned);
        let mut file = match OpenOptions::new().write(true).truncate(true).open(&path) {
            Ok(file) => file,
            // 文件已经被打开过了：从这里开始的失败都不再说"没有发生"。
            Err(_) => return ActionOutcome::unknown(),
        };
        if std::io::Write::write_all(&mut file, encoded.as_bytes()).is_err() {
            return ActionOutcome::unknown();
        }
        if file.sync_all().is_err() {
            return ActionOutcome::unknown();
        }
        ActionOutcome::success(vec![ContentBlock::new(
            "edited",
            format!(
                "{id} 改好了：{before} → {} 字节（被换掉 {} 字节、换上 {} 字节）；标题与贴纸标记一字未动。",
                next.len(),
                old_text.len(),
                new_text.len()
            ),
        )])
    }

    /// 复核一次就地改**现在的状态**（`notes_edit` 的复核；只读三分）。
    ///
    /// **现在**这份正文里：`old_text` 已经不在、`new_text` 在 ⇒ 成功；`old_text` 还在 ⇒
    /// 明确失败 `still-present`（要被换掉的还在，与删除复核同一句话）；这一条不在、读不出来、
    /// 不是本 App 的形状，或两份都不在 ⇒ **无法确认**。
    fn verify_edited(&self, params: &ActionParams) -> ActionOutcome {
        for (name, _) in params.entries() {
            if name != PARAM_ID && name != PARAM_OLD_TEXT && name != PARAM_NEW_TEXT {
                return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
            }
        }
        let (Some(id), Some(old_text), Some(new_text)) = (
            params.get(PARAM_ID),
            params.get(PARAM_OLD_TEXT),
            params.get(PARAM_NEW_TEXT),
        ) else {
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        };
        if !valid_identity(id) {
            return ActionOutcome::failure(NotesFailure::InvalidId.as_str());
        }
        if old_text.is_empty() {
            // 空串复核没有意义（任何正文都"包含"空串）⇒ 明确拒绝，不给一个恒真的答案。
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        }
        let path = self.path_of(id);
        match fs::read_to_string(&path) {
            // 这一条不在 ⇒ "要被换掉的那段不在了"是**确定**的…但"换成的那段在不在"无从谈起
            // ⇒ 与"读不出来"同一档：**不声称**（第三档），不许拿它冒充成功。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => ActionOutcome::unknown(),
            Err(_) => ActionOutcome::unknown(),
            Ok(body) => match decode_note(&body) {
                None => ActionOutcome::unknown(),
                Some((_, text, _)) if text.contains(old_text) => {
                    ActionOutcome::failure(NotesFailure::StillPresent.as_str())
                }
                Some((_, text, _)) if text.contains(new_text) => {
                    ActionOutcome::success(vec![ContentBlock::new(
                        "edited",
                        format!("{id} 的正文里那段已经不在了、换上的是它"),
                    )])
                }
                Some(_) => ActionOutcome::unknown(),
            },
        }
    }

    fn list_notes(&self, params: &ActionParams) -> ActionOutcome {
        if !params.is_empty() {
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        }
        let entries = match self.read_all() {
            Ok(entries) => entries,
            Err(failure) => return ActionOutcome::failure(failure.as_str()),
        };
        let mut items: Vec<(String, String)> = entries
            .iter()
            .map(|note| (note.id.clone(), note.title.clone()))
            .collect();
        items.sort();
        let truncated = items.len() > MAX_LISTED_NOTES;
        items.truncate(MAX_LISTED_NOTES);

        let mut text = format!("notes = {}, truncated = {}\n", items.len(), truncated);
        for (id, title) in &items {
            text.push_str(id);
            text.push('\t');
            text.push_str(title);
            text.push('\n');
        }
        ActionOutcome::success(vec![ContentBlock::new(CAPABILITY_LIST_NOTES, text)])
    }

    /// 读取一条笔记的标题与正文。
    fn read_note(&self, params: &ActionParams) -> ActionOutcome {
        for (name, _) in params.entries() {
            if name != PARAM_ID {
                return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
            }
        }
        let Some(id) = params.get(PARAM_ID) else {
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        };
        if !valid_identity(id) {
            return ActionOutcome::failure(NotesFailure::InvalidId.as_str());
        }

        let path = self.path_of(id);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return ActionOutcome::failure(NotesFailure::NotFound.as_str());
            }
            Err(error) => return ActionOutcome::failure(io_failure(error).as_str()),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return ActionOutcome::failure(NotesFailure::Unreadable.as_str());
        }
        if metadata.len() > MAX_NOTE_FILE_BYTES as u64 {
            return ActionOutcome::failure(NotesFailure::TooLarge.as_str());
        }

        let mut bytes = Vec::new();
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) => return ActionOutcome::failure(io_failure(error).as_str()),
        };
        if file
            .take(MAX_NOTE_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .is_err()
        {
            return ActionOutcome::failure(NotesFailure::Unreadable.as_str());
        }
        if bytes.len() > MAX_NOTE_FILE_BYTES {
            return ActionOutcome::failure(NotesFailure::TooLarge.as_str());
        }
        let body = match String::from_utf8(bytes) {
            Ok(body) => body,
            Err(_) => return ActionOutcome::failure(NotesFailure::Unreadable.as_str()),
        };
        let Some((title, text, _)) = decode_note(&body) else {
            // 这一条读得出来但不是本 App 写的形状：不猜、不当成空笔记。
            return ActionOutcome::failure(NotesFailure::Unreadable.as_str());
        };
        if text.len() > MAX_NOTE_BYTES {
            // **正文超上限** ⇒ 明确失败（**不截断**：宁可让主体换一条路，也不给半截）。
            // ⚠ 判在**解码之后**（2026-10-13）：这样"正文贴着上限的那一条"读得回来——口径与两个写
            // 入口判的是同一件事（从前这里判的是整份文件，于是那种笔记**写得进去、读不回来**）。
            return ActionOutcome::failure(NotesFailure::TooLarge.as_str());
        }
        ActionOutcome::success(vec![
            ContentBlock::new("title", title),
            ContentBlock::new("text", text),
        ])
    }

    /// 贴一张纸条 / 摘一张纸条（`pinned = true` 是贴，`false` 是摘）。
    ///
    /// **幂等**：本来就是这个状态 ⇒ 成功，而且**不重写文件**（贴两次 = 贴着，摘两次 = 摘了）。
    /// 只动 `pinned` 标记：标题与正文原样保留。未知身份 ⇒ 明确失败 `not-found`；
    /// 文件在但读不懂 ⇒ `unreadable`；写或同步失败 ⇒ 如实报**无法确认**（与 `notes_create`
    /// 同一纪律：不删、不补写、不重试）。
    fn set_pinned(&self, params: &ActionParams, pinned: bool) -> ActionOutcome {
        for (name, _) in params.entries() {
            if name != PARAM_ID {
                return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
            }
        }
        let Some(id) = params.get(PARAM_ID) else {
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        };
        if !valid_identity(id) {
            return ActionOutcome::failure(NotesFailure::InvalidId.as_str());
        }
        let path = self.path_of(id);
        let body = match fs::read_to_string(&path) {
            Ok(body) => body,
            Err(error) => return ActionOutcome::failure(io_failure(error).as_str()),
        };
        let Some((title, text, current)) = decode_note(&body) else {
            return ActionOutcome::failure(NotesFailure::Unreadable.as_str());
        };
        if current == pinned {
            // 幂等：本来就是这个状态——不重写文件，也不谎称"改过了"。
            let (label, what) = if pinned {
                ("already-pinned", "pinned")
            } else {
                ("already-unpinned", "unpinned")
            };
            return ActionOutcome::success(vec![ContentBlock::new(
                label,
                format!("{id} was already {what}"),
            )]);
        }

        // 标记更新：`encode_note` 只改 `pinned`，标题与正文原样（逐字节）写回。
        let next = encode_note(&title, &text, pinned);
        let mut file = match OpenOptions::new().write(true).truncate(true).open(&path) {
            Ok(file) => file,
            // 文件已经被打开过了：从这里开始的失败都不再说"没有发生"。
            Err(_) => return ActionOutcome::unknown(),
        };
        if std::io::Write::write_all(&mut file, next.as_bytes()).is_err() {
            return ActionOutcome::unknown();
        }
        if file.sync_all().is_err() {
            return ActionOutcome::unknown();
        }

        ActionOutcome::success(vec![ContentBlock::new(
            if pinned { "pinned" } else { "unpinned" },
            format!(
                "{id} (title {} bytes, text {} bytes)",
                title.len(),
                text.len()
            ),
        )])
    }

    /// 复核一条笔记**现在的**贴纸状态（只读三分）。
    ///
    /// 贴着 ⇒ **成功**；没贴 ⇒ **明确失败** `not-pinned`（它在，只是没贴——不是"找不到"）；
    /// 读不出来 / 不是本 App 的形状 ⇒ **无法确认**。
    ///
    /// ⚠️ 它与贴、摘两个动作**同参数**，因此它**分不清**是贴还是摘
    /// （见 [`NotesApp::pinned_verification_capability`]）。
    fn verify_pinned(&self, params: &ActionParams) -> ActionOutcome {
        for (name, _) in params.entries() {
            if name != PARAM_ID {
                return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
            }
        }
        let Some(id) = params.get(PARAM_ID) else {
            return ActionOutcome::failure(NotesFailure::InvalidParameter.as_str());
        };
        if !valid_identity(id) {
            return ActionOutcome::failure(NotesFailure::InvalidId.as_str());
        }
        let body = match fs::read_to_string(self.path_of(id)) {
            Ok(body) => body,
            Err(error) => return ActionOutcome::failure(io_failure(error).as_str()),
        };
        let Some((_, _, pinned)) = decode_note(&body) else {
            return ActionOutcome::unknown();
        };
        if pinned {
            ActionOutcome::success(vec![ContentBlock::new("pinned", format!("{id} is pinned"))])
        } else {
            ActionOutcome::failure(NotesFailure::NotPinned.as_str())
        }
    }

    /// 需要写时再建立存储目录：只读的活动不会因为装配而留下任何东西。
    fn ensure_storage(&self) -> Result<(), NotesFailure> {
        match fs::create_dir(&self.storage) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&self.storage).map_err(io_failure)?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(NotesFailure::Io);
                }
                Ok(())
            }
            Err(error) => Err(io_failure(error)),
        }
    }

    /// 下一个身份：扫描自己的目录取最大编号 + 1。
    ///
    /// 它只做**一次**尝试：并发抢到同一个编号时 `create_new` 会让其中一个明确失败（`note-id-taken`），
    /// 本版**不自动改号**——自动改号看起来像成功，实际把"我拿到的是几号"变成了一个不确定的事。
    fn next_identity(&self) -> Result<String, NotesFailure> {
        let mut highest = 0u64;
        for entry in fs::read_dir(&self.storage).map_err(io_failure)? {
            let Ok(entry) = entry else { continue };
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(number) = identity_number(&name) {
                highest = highest.max(number);
            }
        }
        Ok(format!("note-{}", highest + 1))
    }

    /// 读回自己存储里的全部笔记（机械排序前）。
    fn read_all(&self) -> Result<Vec<StoredNote>, NotesFailure> {
        let entries = match fs::read_dir(&self.storage) {
            Ok(entries) => entries,
            // 还没有任何一条笔记：那是"空"，不是错误。
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io_failure(error)),
        };

        let mut notes = Vec::new();
        for entry in entries {
            let Ok(entry) = entry else { continue };
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(NOTE_SUFFIX).map(str::to_string) else {
                // 不是本 App 的文件：跳过，不猜、不报错——它不归这个 App 管。
                continue;
            };
            if !valid_identity(&id) {
                continue;
            }
            let mut bytes = Vec::new();
            let Ok(file) = File::open(entry.path()) else {
                continue;
            };
            if file
                .take(MAX_NOTE_FILE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .is_err()
                || bytes.len() > MAX_NOTE_FILE_BYTES
            {
                continue;
            }
            let Ok(body) = String::from_utf8(bytes) else {
                continue;
            };
            let Some((title, text, pinned)) = decode_note(&body) else {
                continue;
            };
            // 文件**最后动过**的时刻。读不出来就是 `None`（如实写 `unknown`，**不编一个数**）：
            // 拿"读不到时间"冒充"刚刚动过"会让纸条板上那一格撒谎。
            let touched_millis = entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|since| since.as_millis() as u64);
            notes.push(StoredNote {
                id,
                title,
                text,
                pinned,
                touched_millis,
            });
        }
        Ok(notes)
    }

    fn path_of(&self, id: &str) -> PathBuf {
        self.storage.join(format!("{id}{NOTE_SUFFIX}"))
    }
}

// ---------------------------------------------------------------------------
// 来源索引：这份字**是什么时候、因哪次活动**写下的（判据 19）
// ---------------------------------------------------------------------------

/// 来源索引的文件名（住在**环境根**，与 `notes/` 平级）。
///
/// ⚠ 它**不是**笔记存储的一部分：`notes/` 里只放 `.note`；这是**派生**的索引——
/// 从经历账本折出来，丢了就重折。判据 19：「**系统建来源索引，只记引用，不给亲疏**」。
const ORIGINS_FILE: &str = "notes-origins.ndjson";

/// 一条来源边：这份字是什么时候、因哪次活动写下的。
///
/// ⚠ **只记引用**（身份 ＋ 时刻 ＋ 活动），**不记正文**——正文在笔记存储与账本里各有一份，
/// 这里再抄一份就是第二份真相（判据 19 的后半句："只记引用"）。
#[derive(Clone, Debug, PartialEq, Eq)]
struct OriginEdge {
    /// 被指的字（`note-<n>`）。
    target: String,
    /// 写下它的时刻（账本里那一格的 `at-ms`）；没记 ⇒ `None`，**不编造**。
    at_millis: Option<u64>,
    /// 因哪次活动而起（引用，不是复制）。
    run: String,
}

/// 折出来的索引：**每条字最早那一条边**（＝创建）＋ 折到哪儿了。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Origins {
    /// 目标 → 最早那一条边。
    created: std::collections::BTreeMap<String, OriginEdge>,
    /// 已经折到哪个序号（`None` = 一条都没折过）。
    ///
    /// ⚠ 它记的是**读到的最大序号**，不是"最后一条边的序号"——否则长期没有新字时，
    /// 每一轮都会把同一段重新扫一遍，成本又变回随历史增长。
    cursor: Option<u64>,
}

impl Origins {
    /// 这条字是什么时候写下的。没折到 ⇒ `None`（那一格如实写 `unknown`，**不编造**）。
    fn created_at(&self, target: &str) -> Option<u64> {
        self.created.get(target).and_then(|edge| edge.at_millis)
    }
}

/// 一个**安全字段**：本索引的行是制表符分隔的裸字段，因此值里不许有控制字符、引号或反斜杠。
///
/// 不满足 ⇒ `None`：**这一条边不写**（如实少一条，不写一行读不回来的东西）。
/// 判据：本索引的值只有 `note-<n>` 与活动身份，两者都不该出现那些字符——
/// 出现了说明上游变了，那时该由**那件事**去报，不该由这里悄悄编一个转义。
fn safe_field(value: &str) -> Option<&str> {
    if value.is_empty()
        || value
            .chars()
            .any(|ch| ch.is_control() || ch == '"' || ch == '\\')
    {
        return None;
    }
    Some(value)
}

/// 一条边编成一行（`edge\t<目标>\t<时刻或 ->\t<活动>`）。
fn encode_edge(edge: &OriginEdge) -> Option<String> {
    let target = safe_field(&edge.target)?;
    let run = safe_field(&edge.run)?;
    let at = edge
        .at_millis
        .map(|ms| ms.to_string())
        .unwrap_or_else(|| "-".to_string());
    Some(format!("edge\t{target}\t{at}\t{run}\n"))
}

/// 游标编成一行（`cursor\t<序号>`）。
fn encode_cursor(seq: u64) -> String {
    format!("cursor\t{seq}\n")
}

/// 索引的**格式版本**。
///
/// ⚠ 它存在是因为**游标会越过没认出来的条目**：一旦提取规则变了（或从前那条规则有 bug），
/// 旧索引的游标照旧"已经折过"——**那个 bug 就被永久冻住了**。版本行是那个重折触发器。
/// 判据来自真机：第一版提取规则太严，真机 **0/32**，而游标已经推到 6959。
const ORIGINS_VERSION: u64 = 1;

/// 解一行。**认不得 ⇒ `None`**：不是"跳过这一行"，而是"**整份索引不可信**"——
/// 它是**派生**的，因此正确反应是**重折**（与"目录与盘上对不上就由卷重建"同一条纪律）。
fn decode_origin_line(line: &str) -> Option<OriginLine> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let fields: Vec<&str> = line.split('\t').collect();
    match fields.as_slice() {
        ["version", version] => Some(OriginLine::Version(version.parse::<u64>().ok()?)),
        ["cursor", seq] => Some(OriginLine::Cursor(seq.parse::<u64>().ok()?)),
        ["edge", target, at, run] => {
            if !valid_identity(target) {
                return None;
            }
            let run = safe_field(run)?;
            let at_millis = match *at {
                "-" => None,
                digits => Some(digits.parse::<u64>().ok()?),
            };
            Some(OriginLine::Edge(OriginEdge {
                target: target.to_string(),
                at_millis,
                run: run.to_string(),
            }))
        }
        _ => None,
    }
}

/// 回执那一块里的身份。
///
/// ⚠ **形状照抄产者**：那一块的正文是 `<身份> (title N bytes, text M bytes)`
/// ——真机读数：`note-1 (title 38 bytes, text 693 bytes)`。因此身份是**第一个空白之前**
/// 那一段，后面那截是明细。**不从中挑子串**：第一段不是合法身份就少一条边
/// （而"少一条边"只让那一格的年龄写 `unknown`，比编一个身份便宜得多）。
///
/// ⚠ 这一格是**拿真机读数换来的**：夹具里我把块正文写成了裸 `note-1`，
/// 于是**夹具绿、真机 0/32**——夹具编码的是我的假设，不是产者的形状。
fn identity_in(text: &str) -> Option<String> {
    let head = text.trim().split(char::is_whitespace).next().unwrap_or("");
    if valid_identity(head) {
        Some(head.to_string())
    } else {
        None
    }
}

/// 索引文件里的一行。
enum OriginLine {
    Version(u64),
    Cursor(u64),
    Edge(OriginEdge),
}

impl NotesApp {
    /// 环境根（`notes/` 的上一级）。索引住在它下面。
    fn root(&self) -> PathBuf {
        self.storage
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// 索引文件的路径（住在环境根，与 `notes/` 平级）。
    fn origins_path(&self) -> PathBuf {
        self.root().join(ORIGINS_FILE)
    }

    /// 读回索引。`Ok(None)` ＝ **整份不可信**（有认不得的行）⇒ 调用方**重折**。
    /// 文件不在 ⇒ 空索引（还没折过，不是损坏）。
    fn read_origins(&self) -> Result<Option<Origins>, NotesFailure> {
        let text = match fs::read_to_string(self.origins_path()) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Some(Origins::default()));
            }
            Err(error) => return Err(io_failure(error)),
        };
        let mut origins = Origins::default();
        let mut version: Option<u64> = None;
        for line in text.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() {
                continue;
            }
            let Some(parsed) = decode_origin_line(line) else {
                return Ok(None);
            };
            match parsed {
                OriginLine::Version(found) => version = Some(found),
                OriginLine::Cursor(seq) => origins.cursor = Some(seq),
                OriginLine::Edge(edge) => {
                    // **最早那一条说了算**（＝创建）；后来的边不改它。
                    match origins.created.get(&edge.target) {
                        Some(existing) if existing.at_millis <= edge.at_millis => {}
                        _ => {
                            origins.created.insert(edge.target.clone(), edge);
                        }
                    }
                }
            }
        }
        // **没有版本行、或版本对不上 ⇒ 整份不可信 ⇒ 重折**。
        // 游标会越过**没认出来的**条目，因此提取规则一变，旧游标必须作废
        // （否则那个 bug 被永久冻住——第一版就是 0/32 而游标照旧推到了 6959）。
        if version != Some(ORIGINS_VERSION) {
            return Ok(None);
        }
        Ok(Some(origins))
    }

    /// **增量折**：把"上次折到之后"新写下的来源边补进索引。
    ///
    /// - 只读**活动段 ＋ 还没折过的那些卷**（`EnvironmentReader::trace_after`）⇒ 成本是
    ///   **O(新增)**，不随历史增长；
    /// - 索引是**派生**的：认不得 / 读不回来 ⇒ **从头重折并整份重写**（与"目录与盘上对不上
    ///   就由卷重建"同一条纪律——**卷是权威，索引只是索引**）；
    /// - **折不动不挡任何事**：它只影响那一格的年龄，调用方据此如实写 `unknown`。
    fn fold_origins(&self) -> Result<Origins, NotesFailure> {
        let cached = self.read_origins()?;
        let rebuilding = cached.is_none();
        let base = cached.unwrap_or_default();
        let root = self.root();
        // 下层失败在这里**归类**（不携带原文穿透：本 App 不认识存储层的细节）。
        let agent = identity_at(&root).map_err(|_| NotesFailure::Unreadable)?;
        let reader = EnvironmentReader::open(&root, agent).map_err(|_| NotesFailure::Unreadable)?;
        // **游标为空 ⇒ 整份读一次**（第一次折，或重折）。这是**一次性**的：折过之后游标就在了。
        // ⚠ 不许把它写成 `trace_after(0)`——那是"序号**大于** 0"，而**第一条记录的序号就是 0**
        // ⇒ 那一条会被静默漏掉（这一格是拿一条真用例换来的）。
        let after = if rebuilding { None } else { base.cursor };
        let fresh = match after {
            Some(seq) => reader
                .trace_after(seq)
                .map_err(|_| NotesFailure::Unreadable)?,
            None => reader.trace().map_err(|_| NotesFailure::Unreadable)?,
        };
        if fresh.is_empty() && !rebuilding {
            return Ok(base);
        }

        let mut folded = if rebuilding { Origins::default() } else { base };
        let mut max_seq = after;
        let mut appended = String::new();
        for record in &fresh {
            max_seq = Some(max_seq.map_or(record.seq(), |seen| seen.max(record.seq())));
            let TracePayload::Observed { action, blocks, .. } = record.entry() else {
                continue;
            };
            if action != CAPABILITY_CREATE_NOTE {
                continue;
            }
            for (label, text) in blocks {
                if label != "created" {
                    continue;
                }
                let Some(target) = identity_in(text) else {
                    continue;
                };
                let edge = OriginEdge {
                    target: target.clone(),
                    at_millis: record.at_millis(),
                    run: record.run().as_str().to_string(),
                };
                let Some(encoded) = encode_edge(&edge) else {
                    continue;
                };
                // **最早那一条说了算**（＝创建）：已经有了就不再插。
                folded.created.entry(target).or_insert(edge);
                appended.push_str(&encoded);
            }
        }
        // **一条都没读到 ⇒ 不写游标**：写一个 `0` 会让下一次从"大于 0"读起，把 `seq = 0` 漏掉。
        if let Some(seq) = max_seq {
            appended.push_str(&encode_cursor(seq));
        }

        if rebuilding {
            // 重折：整份重写，**版本行在最前**（下次读它才知道这份索引是哪一版规则折的）。
            let mut fresh_file = format!("version\t{ORIGINS_VERSION}\n");
            fresh_file.push_str(&appended);
            fs::write(self.origins_path(), &fresh_file).map_err(io_failure)?;
        } else {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.origins_path())
                .map_err(io_failure)?;
            file.write_all(appended.as_bytes()).map_err(io_failure)?;
        }
        Ok(folded)
    }
}

/// **交素材**给显示区（装配期用，不是主体的动词）。
///
/// 方向是 `app → 显示区`：手机模型 §3 的分层自下而上是 **1 循环＋插槽 / 2 显示区 / 3 app /
/// 4 Agent**，而第 3 条写着"**app：往显示区交素材**"——上层依赖下层，合法。
///
/// 因此本 App **依赖 [`crate::pins_board`]**、把**现在贴在墙上的纸条**填进 [`PinnedRow`]：
///
/// - 交的是**结构化内容**（哪张贴上了、标题正文原样），**不是本 App 的记录类型**
///   （存储里那张还有 `pinned` 标记等组件区不该关心的字段）；
/// - **截断与摆行归显示区**（手机模型 §5：「内容归 app 交，怎么显示手机说了算」）——
///   这里一个字节都不截，读的是原样；
/// - 顺序按身份编号机械排序（与 `notes_list` 同一口径）。
///
/// 它只读：不改标记、不写任何东西；读不出来 ⇒ 明确失败（显示区据此如实渲染 `unknown`，
/// **绝不冒充"没有贴"**）。
///
/// ⚠️ 反过来的形状（显示区 import 本 App 的 `PinnedNote`）是**下层认识上层**，本刀改正。
pub fn pinned_material(environment_root: &Path) -> Result<Vec<PinnedRow>, NotesFailure> {
    let app = NotesApp::open(environment_root)?;
    // **来源索引折一次**（增量：只读上次折到之后那一段）⇒ 拿到"什么时候写下的"。
    // ⚠ **折不动不挡装配**：那一格如实写 `unknown`（与纸条板"永不让活动失败"同一条纪律，
    // 也与"缺省不等于零"同规——读不到 ≠ 没有）。
    let origins = app.fold_origins().ok();
    let mut rows: Vec<PinnedRow> = app
        .read_all()?
        .into_iter()
        .filter(|note| note.pinned)
        .map(|note| PinnedRow {
            created_millis: origins
                .as_ref()
                .and_then(|folded| folded.created_at(&note.id)),
            id: note.id,
            touched_millis: note.touched_millis,
            title: note.title,
            text: note.text,
        })
        .collect();
    rows.sort_by_key(|row| identity_number(&row.id).unwrap_or(u64::MAX));
    Ok(rows)
}

/// 存储里的一条笔记（列表只用身份与标题）。
#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredNote {
    id: String,
    title: String,
    text: String,
    /// 这份文件**最后动过**的时刻（unix 毫秒）。`None` = 读不出来，**不是 0**。
    ///
    /// ⚠ 它是「最后动过」而不是「创建于」：文件里没有时间字段，而 `notes_edit` /
    /// `notes_append` 会改写它。要真正的创建时刻，得从**账本**里折（那是另一件事）。
    touched_millis: Option<u64>,
    /// 主体有没有把这张纸条贴到墙上（`notes_pin` 的标记）；随笔记文件一起走。
    pinned: bool,
}

/// 标题规则：非空、单行（不含控制字符）、在上限之内。
///
/// 它要出现在列表里，所以"单行"不是洁癖：带换行的标题会伪造出列表里的另一行。
fn valid_title(title: &str) -> bool {
    !title.is_empty() && title.len() <= MAX_TITLE_BYTES && !title.chars().any(char::is_control)
}

/// 身份规则：`note-<十进制数字>`，非零、无前导零之外的花样。
///
/// 只认这一种形状，因此 `../x`、`note-1/x`、空串、带冒号的别名都不是身份——它们连拼成路径的
/// 机会都没有。
fn valid_identity(id: &str) -> bool {
    let Some(number) = id.strip_prefix("note-") else {
        return false;
    };
    !number.is_empty() && number.chars().all(|ch| ch.is_ascii_digit()) && !number.starts_with('0')
}

/// 文件名 → 编号（只有合法身份才算）。
fn identity_number(name: &str) -> Option<u64> {
    let id = name.strip_suffix(NOTE_SUFFIX)?;
    if !valid_identity(id) {
        return None;
    }
    id.strip_prefix("note-")?.parse().ok()
}

/// 笔记的存储编码：一条笔记一个 JSON 对象（`pinned` 是 TASK_33 起的贴纸标记）。
///
/// 用既有依赖即可，不引入新的序列化方式，也不把正文拼进任何有歧义的分隔格式里。
fn encode_note(title: &str, text: &str, pinned: bool) -> String {
    let object = serde_json::json!({ "title": title, "text": text, "pinned": pinned });
    format!("{object}\n")
}

/// 解码一条笔记；不是本 App 写的形状就返回 `None`（调用方据此报"读不出来"，不猜）。
///
/// `pinned` **宽容一格**：旧文件（TASK_33 之前写的）没有这个键，那就是"没贴"；
/// 但键在而值不是布尔，说明这不是本 App 写的形状 ⇒ fail-closed 返回 `None`。
fn decode_note(body: &str) -> Option<(String, String, bool)> {
    let value: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    let title = value.get("title")?.as_str()?.to_string();
    let text = value.get("text")?.as_str()?.to_string();
    let pinned = match value.get("pinned") {
        None => false,
        Some(flag) => flag.as_bool()?,
    };
    // ⚠ **解码器不判正文上限**（2026-10-13）：上限只在"正文进主体的上下文"（`read_note`）与
    // "正文增长"（建 / 追加）那两处判。解码器只管**形状**——否则一条正文超限的笔记会在列表里
    // **静默消失**（"读不出来"与"没有"被压成一档）。
    if !valid_title(&title) {
        return None;
    }
    Some((title, text, pinned))
}

/// I/O 错误 → 稳定分类。
fn io_failure(error: std::io::Error) -> NotesFailure {
    match error.kind() {
        std::io::ErrorKind::NotFound => NotesFailure::NotFound,
        std::io::ErrorKind::PermissionDenied => NotesFailure::Io,
        _ => NotesFailure::Io,
    }
}

impl ActionPort for NotesApp {
    fn capabilities(&self, _agent: &AgentId, _run: &RunId) -> Capabilities {
        self.available_capabilities()
    }

    /// 局部准入：六个动作一视同仁——写自己笔记的那几项与它们的复核**不需要任何授权**
    /// （自持物），读两项照旧。身份与活动由 OS 的使用作用域把住，这里不再重复判。
    fn admit(&self, _agent: &AgentId, _run: &RunId, action: &Action) -> ActionAdmission {
        match action.id().as_str() {
            CAPABILITY_LIST_NOTES
            | CAPABILITY_READ_NOTE
            | CAPABILITY_CREATE_NOTE
            | CAPABILITY_VERIFY_NOTE
            | CAPABILITY_PIN
            | CAPABILITY_UNPIN
            | CAPABILITY_VERIFY_PINNED => ActionAdmission::Admitted,
            CAPABILITY_DELETE | CAPABILITY_VERIFY_DELETED => ActionAdmission::Admitted,
            CAPABILITY_APPEND | CAPABILITY_VERIFY_APPENDED => ActionAdmission::Admitted,
            CAPABILITY_EDIT | CAPABILITY_VERIFY_EDIT => ActionAdmission::Admitted,
            _ => ActionAdmission::refused(crate::core::UNKNOWN_CAPABILITY),
        }
    }

    fn execute(
        &mut self,
        agent: &AgentId,
        operation: &OperationId,
        action: &Action,
    ) -> ActionOutcome {
        match action.id().as_str() {
            CAPABILITY_CREATE_NOTE => self.create_note(agent, operation, action.params()),
            CAPABILITY_VERIFY_NOTE => self.verify_note(agent, operation, action.params()),
            CAPABILITY_LIST_NOTES => self.list_notes(action.params()),
            CAPABILITY_READ_NOTE => self.read_note(action.params()),
            CAPABILITY_PIN => self.set_pinned(action.params(), true),
            CAPABILITY_UNPIN => self.set_pinned(action.params(), false),
            CAPABILITY_VERIFY_PINNED => self.verify_pinned(action.params()),
            CAPABILITY_DELETE => self.delete_note(action.params()),
            CAPABILITY_VERIFY_DELETED => self.verify_deleted(action.params()),
            CAPABILITY_APPEND => self.append_note(action.params()),
            CAPABILITY_VERIFY_APPENDED => self.verify_appended(action.params()),
            CAPABILITY_EDIT => self.edit_note(action.params()),
            CAPABILITY_VERIFY_EDIT => self.verify_edited(action.params()),
            _ => ActionOutcome::failure(NotesFailure::InvalidParameter.as_str()),
        }
    }
}

impl crate::app_contract::AppPort for NotesApp {
    fn app_id(&self) -> &str {
        NOTES_APP_ID
    }

    fn description(&self) -> &str {
        "这个 Agent 自己的笔记：读得到旧笔记，也随时能新建一条，还能给某几条标上 `pinned`（标上的那几条，预览会出现在每次清醒期开场；标与取消只动标记、不碰内容；笔记存在自己的环境里、跨清醒期保留，不需要任何授权）"
    }

    fn declared_capabilities(&self) -> Capabilities {
        // 声明必须与实例的真实能力一致：OS 的归属索引就是按这份声明建的。
        self.available_capabilities()
    }

    /// `notes_create` 的真实类别是**可重复写**，不是独占创建：它抢的是**笔记身份**（`note-<n>`）
    /// 而不是内容，所以**同参数第二次会成功**（拿到一个新身份、多出一条内容相同的笔记）。
    /// 由此它的复核语义天生更弱（无法归因到那一次操作），见
    /// [`NotesApp::verification_capability`]。
    ///
    /// 贴/摘（TASK_33）照这个先例：类别同为**可重复写**、各自配复核，而且**共用**同一份复核
    /// （`notes_verify_pinned`）——它在本地同步写上"贴两次 = 贴着"是幂等的，因此门语义与
    /// `notes_create` 无差异（第四档 `idempotent-write` 的第二例，理由见交付报告）。
    fn declared_effects(&self) -> Vec<crate::app_contract::EffectDeclaration> {
        use crate::app_contract::{EffectClass, EffectDeclaration};
        vec![
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_CREATE_NOTE),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_VERIFY_NOTE)),
            ),
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_PIN),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_VERIFY_PINNED)),
            ),
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_DELETE),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_VERIFY_DELETED)),
            ),
            // **删除也是写**（自己的地盘上的写，不需要授权），因此照样配一个复核：
            // 删错了没有后悔药，更要让"删掉了吗"这件事在机制上可确认。
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_UNPIN),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_VERIFY_PINNED)),
            ),
            // **追加也是写**（TASK_80）：它**只往后接、不改已有的字**，因此同参数第二次会成功
            // （多出一段）。复核与它**同参数**（那一条现在的正文是不是以这段结尾）。
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_APPEND),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_VERIFY_APPENDED)),
            ),
            // **就地改**（2026-10-14）：与 `files_edit` **同类**——判据是同参重放：第一次消耗
            // 前置（`old_text` 被换走），第二次同参数必然明确失败且无害 ⇒ 独占创建那一档的语义。
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_EDIT),
                EffectClass::ExclusiveCreate,
                Some(ActionId::new(CAPABILITY_VERIFY_EDIT)),
            ),
        ]
    }

    /// 来源档 = **这一段回执里携带的内容从哪个口子进来**。
    ///
    /// 读动词（列纸条 / 读纸条 / 四条复核）带回的是**自己写下的字**，因此是
    /// [`crate::app_contract::SourceClass::SelfAuthored`]——这是全库唯一一档"内容确实是主体的手笔"的来源；
    /// 五个写动词（建 / 贴 / 摘 / 删 / 追加）只回报系统对这次动作的确认，因此是
    /// [`crate::app_contract::SourceClass::System`]。
    fn declared_sources(&self) -> Vec<crate::app_contract::SourceDeclaration> {
        use crate::app_contract::{SourceClass, SourceDeclaration};
        self.declared_capabilities()
            .items()
            .iter()
            .map(|capability| {
                let class = match capability.id().as_str() {
                    CAPABILITY_CREATE_NOTE
                    | CAPABILITY_PIN
                    | CAPABILITY_UNPIN
                    | CAPABILITY_DELETE
                    | CAPABILITY_APPEND
                    | CAPABILITY_EDIT => SourceClass::System,
                    _ => SourceClass::SelfAuthored,
                };
                SourceDeclaration::new(capability.id().clone(), class)
            })
            .collect()
    }
}
