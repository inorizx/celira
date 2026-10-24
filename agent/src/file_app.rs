//! 文件 App：在**调用者显式授权**的一个沙盒根目录里做有界的事。
//!
//! **动词名 = App 名 `_` 动词**（`files_list` / `files_read` / `files_grep` / `files_create` /
//! `files_edit` / `files_verify_*`）：工具名是全局平铺词表，归属必须由名字自答。App 名承担归属
//! （装配期已保证唯一），动词只吃通用词（`read` / `list` / `grep` / `create` / `edit` / `verify`），
//! 同一个 App 里两个 `verify` 靠宾语消歧（`files_verify_text` / `files_verify_edit`）⇒ 派生两条
//! 装配期事实：**不同 App 的动词必然不同名**；同一个 App 内撞名由装配期拒绝
//! （`ConflictingActionId`，无新机制）。
//!
//! **默认只读**：不给创建授权时，这里只有三件事——列目录（可续列）、读文本（可续读）、
//! 按**子串**查找。这三个动词**都不声明写效应**（默认只读），因此装配期不为它们要求复核能力
//! ——它们没有任何写效应。
//! 只有宿主显式给出 [`CreationGrant`]（见 `creation_grant`）之后，才会多出**两件**写能力：
//! 在一个已经存在的授权子目录里**新建**一个 UTF-8 文本文件，以及把一份已存在文件里的
//! `old_text` 唯一一处换成 `new_text`。没有授权时这两件能力连声明都不出现。
//!
//! **写面有两种来源**（[`WritableFace`]）：宿主授权的一个已存在子目录，与**主体自己的作品目录**
//! （[`FileApp::with_own_works`]：根就是写面）。两种面的**写入语义一字不差**，差别只在**归属**
//! ——授权的那个要**对得上这次身份与活动**（那是别人决定的），作品面不绑这两样（主体自己的东西
//! 不用申请，只要这一格在这一次装配里在场）。⇒ 上面那句"没有授权就连声明都不出现"说的是
//! **授权面**；作品面不需要授权，它的目录由装配方缺席才建。
//!
//! 三个只读动词都是**原子**的、而且**平级**：同一个 App、同一份路径形状校验
//! （[`relative_names`]）、同一批上限（[`MAX_ENTRIES`] / [`MAX_TEXT_BYTES`] / [`MAX_SCANNED`] /
//! [`MAX_SNIPPET_BYTES`]）。谁都不编排下一步：`after` / `offset` / `limit` 只是把"接着看"
//! 这件事交回给主体，`files_grep` 只回答"哪里出现过这个子串"（不做排序、评分、摘要、索引，
//! 也不替它挑该读哪一份）。
//!
//! **★ 它是渐进披露的第四档：让主体自己检索**（2026-10-06 与真跑读数同批落地）。常驻的只有
//! 材料区的那一行读数，二级三级是 `files_list` / `files_read`；而材料份数 `N` 从 1 到 30 时，
//! 主体的 `files_*` 动作数**没有增长**（成功那几次都是 **3~4 个**）——因为**它自己会搜**：常见
//! 序列是 `files_grep` → `files_read`，**一次搜索就定位到**。
//! ⇒ 本 App 的"不替主体检索"（上面那条：不排序、不评分、不替它挑该读哪一份）**不等于"只给一份
//! 目录"**：真实形态是**给它目录 ＋ 给它一个机械的搜索工具，然后它自己搜**——既没有判断，也没有
//! 让它在 `N` 大时逐个翻完（那是负担）。
//! ⚠ **射程**：那一批真跑是**一个任务族、一个模型、`N` 只到 30**（出处与逐档读数见
//! [`crate::skills_desktop`] 的模块文档「那一行读数的效用」一节）。
//!
//! 边界（本模块是真实目录访问的唯一实现；`file_view` 复用它做真实路径那几块——派生只读的几块
//! 与主体自己的作品面）：
//!
//! - 根目录必须由组合入口显式传入；**没有根目录就拒绝启动**，绝不回退到当前目录、桌面、
//!   Home 或仓库根，也不自己猜一个；
//! - 模型只能给沙盒**相对**路径；绝对路径、盘符、UNC、设备路径、`..`、`:` 一律拒绝；
//! - 访问链上的每一段都检查符号链接 / junction / 其他 reparse point；
//! - 读侧三项能力（列目录 ≤100 项、读文本 ≤64 KiB、按子串查找 ≤100 条候选且最多扫 500 个文件）；
//!   写侧两项 **不改名、不删除任何文件**：创建是独占新建（`create_new`），改是只替换一处
//!   出现的文本（64 KiB，超限拒绝不截断）；
//! - 文件内容对 Agent 来说是**数据**，不是指令；真正的边界是目录与能力限制，不是措辞。
//!
//! **明确不做**：排序与评分、摘要、按语义相似度挑、向量检索、索引与缓存（会引入第二份持久
//! 状态）、正则与大小写开关（那是"参数平台"）、跨 App 的统一检索——本 App 只查自己那一个沙盒，
//! 而 OS 也不认识任何 App 的存储（见 `os` 的模块文档）；也不给"先找再读"这类编排建议——排序、
//! 评分、挑哪一份读仍是主体自己的事。
//!
//! **已知缺口（如实登记）**：
//!
//! 1. `files_grep` 是**子串**匹配：`Permission` 与 `permission`、`权限` 与 `權限` 互不命中。
//!    这是刻意的（可预期、无参数平台），不是待修的缺陷；
//! 2. **目录名不参与匹配**：它们不是材料；
//! 3. **一次续读拿不到比 [`MAX_TEXT_BYTES`] 更多的字节**：要更多只能多次续读；
//! 4. `after` 是**名字**不是游标：地址空间是**活的**，两次调用之间条目增删时，续列可能漏或
//!    重复——本版不引入快照语义；
//! 5. `files_list` 必须**枚举整个目录**才能给出 `after` 依赖的那个稳定全序：返回有界（一页 ≤
//!    [`MAX_ENTRIES`] 条），**读取不再有界**；
//! 6. `files_grep` 的**目录遍历本身**没有单独的条数上限（计数器管的是**文件**）：条目数与文件
//!    数有界，走的目录数没有。
//!
//! ⚠️ 竞态与安全等级：本版把沙盒限定为**受信目录**（测试自建、不允许不可信进程并发改写）。
//! 检查与打开之间存在理论上的替换窗口，本版**不构成 OS 级安全沙盒**，也不承诺抵御恶意并发
//! 路径替换。见 `resolve_existing` 的说明。
//!
//! ## 形状判据里**只有一条**与平台有关
//!
//! 相对路径的形状校验本身是**平台中立**的——`:`（含 Windows ADS）、前导 `\` 或 `/`、绝对路径、
//! `.`、`..`、空名字在**两个平台都拒**。但**反斜杠在 Linux 上不是分隔符**：`..\inside.txt`
//! 在那里是根内的一个**文件名**（真的读得到），在 Windows 上才是「父目录 ＋ 名字」。
//! ⇒ "逃逸清单"这类用例**必须分两列写**（`tests/file_app.rs` 按 `cfg(windows)` 分），
//! Linux 那列换成同一批**意图**在 Linux 上的形状；**不许**把某一平台的形状当成跨平台判据。
//! `is_reparse_point` 那一格只在 Windows 成立（Linux 没有 junction/reparse point，符号链接由
//! `is_symlink` 那条链兜住）——**那是平台臂，不是缺陷**。
//!
//! Linux 上实测过的判据：`invalid-path`（根外真有那个件，照样拒）、`link`（**目录**符号链接
//! 指向根外，`files_read` 与 `files_list` **两个消费者都拒**）、`wrong-kind`（目录当文件读）、
//! `not-found`（缺的反斜杠名字：是"没找到"，不是"路径拒绝"）。检查与打开之间那个替换窗口
//! **仍然存在**，只是被如实钉在 Linux 上，没有消除。

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Capabilities,
    Capability, ContentBlock, ParamSpec,
};
use crate::creation_grant::CreationGrant;
use crate::foundation::{AgentId, OperationId, RunId};
use crate::sandbox::{MachinePath, machine_path};

/// 一次列目录最多返回的直接子项数。
pub const MAX_ENTRIES: usize = 100;
/// 一次 `files_grep` 最多看多少个普通文件（名称或正文）。
///
/// 它与 [`MAX_ENTRIES`] 并列：一个是返回条数的上限，一个是**扫描**范围的上限。触到任一个都
/// 如实报截断——扫描有界不等于"就只有这些"。
pub const MAX_SCANNED: usize = 500;
/// 一次读取文本的最大字节数。
pub const MAX_TEXT_BYTES: u64 = 64 * 1024;
/// `files_grep` 每条候选给出的**该行片段**上限（UTF-8 字节，含截断标记 `…`）。
///
/// 这是**显示截断**，不是丢数据：正文还在盘上，主体可以用 `files_read` 把那一份完整读回来。
pub const MAX_SNIPPET_BYTES: usize = 120;
/// 这个 App 对外的稳定标识（展开动作的目标就是它）。
pub const FILE_APP_ID: &str = "files";

/// 列目录能力的标识（由本 App 命名；Core 只把它当不透明身份搬运）。
const CAPABILITY_LIST: &str = "files_list";
/// 读文本能力的标识。
const CAPABILITY_READ_TEXT: &str = "files_read";
/// 按子串查找能力的标识（与 `files_list` / `files_read` 平级的**第四个只读动词**）。
pub(crate) const CAPABILITY_FIND_TEXT: &str = "files_grep";
/// 创建文本能力的标识。
///
/// 它**只在宿主给出创建授权时才出现在能力目录里**：没有授权就连一个可被请求的动作名都不存在，
/// 而不是"存在但会被拒绝"。
pub(crate) const CAPABILITY_CREATE_TEXT: &str = "files_create";

/// 复核能力的标识。它与创建**同参数**，只回答"现在的真实状态是什么"。
pub(crate) const CAPABILITY_VERIFY_TEXT: &str = "files_verify_text";
/// 改文本能力的标识（把 `old_text` 唯一的一处出现换成 `new_text`）。
///
/// 与创建**同挂**创建授权：它同样是写，同样要宿主显式给过那个目录。
pub(crate) const CAPABILITY_EDIT: &str = "files_edit";
/// 改文本的复核能力标识。参数与 [`CAPABILITY_EDIT`] 完全相同。
pub(crate) const CAPABILITY_VERIFY_EDIT: &str = "files_verify_edit";
/// 唯一被读侧接受的参数名（含义归本 App）。
pub(crate) const PARAM_PATH: &str = "path";
/// 续列参数：一个**名字**，只返回严格大于它的下一批。
pub(crate) const PARAM_AFTER: &str = "after";
/// 续读参数：本段的起始字节（相对文件开头）。
pub(crate) const PARAM_OFFSET: &str = "offset";
/// 续读参数：本段的字节数上限。
pub(crate) const PARAM_LIMIT: &str = "limit";
/// 查找参数：要匹配的**子串**（区分大小写）。
pub(crate) const PARAM_PATTERN: &str = "pattern";
/// 创建动作的文件名参数：授权目录内的**单层**普通文件名。
pub(crate) const PARAM_NAME: &str = "name";
/// 创建动作的正文参数：UTF-8 文本，允许为空串。
pub(crate) const PARAM_TEXT: &str = "text";
/// 改文本动作的**被替换**片段：必须非空（空串会匹配一切，那不是替换）。
pub(crate) const PARAM_OLD_TEXT: &str = "old_text";
/// 改文本动作的**替换成**片段：允许为空串（替换成空 = 删掉那一段）。
pub(crate) const PARAM_NEW_TEXT: &str = "new_text";

/// `files_list` 的**递归深度**（不给 ⇒ 1 ⇒ 只列直接子项，与从前逐字节相同）。
const PARAM_DEPTH: &str = "depth";
/// `files_read` 的**按行**坐标：从第几行起（1 起算）。
///
/// ⚠ **2026-10-14 加**：`files_grep` 报的是 `line=N`，而 `files_read` 从前只认**字节偏移** ⇒
/// 主体看到"第 1234 行出现过"却**跳不过去**；看代码最基本的动作就是"看 120–160 行"。两个动词
/// 从此共用**同一个坐标系**。
const PARAM_FROM_LINE: &str = "from_line";
/// `files_read` 按行读时的**行数**上限（不给 ⇒ 到文件末尾）。
const PARAM_LINES: &str = "lines";
/// 一次 `files_list` 最多下几层（递归是**有界**的，不是"随便翻"）。
pub const MAX_DEPTH: u64 = 8;
/// 一次递归列目录最多看几个目录（触到就停——扫描有界，与 [`MAX_SCANNED`] 同族）。
pub const MAX_DIRECTORIES: usize = 200;

/// 一次创建的正文上限：与单次读取上限一致——读得到的东西才写得出去。
pub const MAX_CREATE_BYTES: usize = MAX_TEXT_BYTES as usize;

/// 没有创建授权时的稳定拒绝分类（执行侧的复核拒绝）。
pub const REFUSAL_CREATION_NOT_GRANTED: &str = "creation-not-granted";
/// 创建授权与这次请求的身份/Run 不一致时的稳定拒绝分类。
pub const REFUSAL_CREATION_GRANT_MISMATCH: &str = "creation-grant-mismatch";

/// 本 App 的稳定失败分类。
///
/// Core 只看到三分里的 `Failure`；分类只是一个稳定标记，供 Agent 下一轮自己判断，Core 不解释它。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureClass {
    /// 相对路径本身不合法：绝对路径、盘符、UNC、设备路径、`:`（Windows ADS）、`.`、`..`、空路径。
    InvalidPath,
    /// 目标不存在。
    NotFound,
    /// 目标已经存在：本次**独占创建**没有发生，而且原名下的内容一个字节都没被动过。
    AlreadyExists,
    /// 目标类型不对：把目录当文件读、把文件当目录列，或不是普通文件。
    WrongKind,
    /// 访问链上出现符号链接、junction 或其他 reparse point。
    Link,
    /// 读取被拒绝（权限）。
    Denied,
    /// 文本超过上限：明确拒绝，**不**悄悄截断成"看起来完整"的内容。
    TooLarge,
    /// 不是合法 UTF-8 文本。
    NotUtf8,
    /// 这段内容读不出来（存储层报错：损坏、越界、未知尾部）。
    ///
    /// **不**用空结果冒充"没有内容"——读失败与"这里是空的"是两件事。
    Unreadable,
    /// 未知能力：能力标识不在本次活动注入的能力集合里。
    UnknownCapability,
    /// 缺少必需参数。
    MissingParameter,
    /// 出现了本能力不认识的参数名。
    UnknownParameter,
    /// 参数**形状对、取值不可接受**：`after` 不是这个列表里的名字、`offset` 越界或落在
    /// UTF-8 字符中间、字节数不是十进制非负整数、`pattern` 为空串。
    ///
    /// 它与 [`FailureClass::InvalidPath`] 分开：路径本身合法，错的是给这个动词的取值。
    InvalidArgument,
    /// 其他 I/O 失败。
    Io,
}

impl FailureClass {
    /// 稳定分类标记。
    pub fn as_str(self) -> &'static str {
        match self {
            FailureClass::InvalidPath => "invalid-path",
            FailureClass::NotFound => "not-found",
            FailureClass::AlreadyExists => "already-exists",
            FailureClass::WrongKind => "wrong-kind",
            FailureClass::Link => "link",
            FailureClass::Denied => "denied",
            FailureClass::TooLarge => "too-large",
            FailureClass::NotUtf8 => "not-utf8",
            FailureClass::Unreadable => "unreadable",
            FailureClass::UnknownCapability => crate::core::UNKNOWN_CAPABILITY,
            FailureClass::MissingParameter => "missing-parameter",
            FailureClass::UnknownParameter => "unknown-parameter",
            FailureClass::InvalidArgument => crate::core::INVALID_ARGUMENT,
            FailureClass::Io => "io",
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
            "invalid-path" => FailureClass::InvalidPath,
            "not-found" => FailureClass::NotFound,
            "already-exists" => FailureClass::AlreadyExists,
            "wrong-kind" => FailureClass::WrongKind,
            "link" => FailureClass::Link,
            "denied" => FailureClass::Denied,
            "too-large" => FailureClass::TooLarge,
            "not-utf8" => FailureClass::NotUtf8,
            "unreadable" => FailureClass::Unreadable,
            crate::core::UNKNOWN_CAPABILITY => FailureClass::UnknownCapability,
            "missing-parameter" => FailureClass::MissingParameter,
            "unknown-parameter" => FailureClass::UnknownParameter,
            crate::core::INVALID_ARGUMENT => FailureClass::InvalidArgument,
            "io" => FailureClass::Io,
            _ => return None,
        })
    }
}

/// 沙盒机制的错误 → 本 App 的失败词表：**一对一**，不合并、不做"尽力解释"。
///
/// 两套词表各自只有一份（机制在 [`crate::sandbox`]，语言在这里），中间只允许这一处翻译。
impl From<crate::sandbox::SandboxError> for FailureClass {
    fn from(error: crate::sandbox::SandboxError) -> Self {
        use crate::sandbox::SandboxError;
        match error {
            SandboxError::InvalidPath => FailureClass::InvalidPath,
            SandboxError::NotFound => FailureClass::NotFound,
            SandboxError::WrongKind => FailureClass::WrongKind,
            SandboxError::Link => FailureClass::Link,
            SandboxError::Denied => FailureClass::Denied,
            SandboxError::TooLarge => FailureClass::TooLarge,
            SandboxError::Unreadable => FailureClass::Unreadable,
            SandboxError::Io => FailureClass::Io,
        }
    }
}

/// 一个目录条目：名称 + 种类（`dir` / `file` / `link` / `unknown`）。
pub(crate) type DirectoryEntry = (String, &'static str);

/// 一个文件 App：绑定**一个面**（家 / 整台机器），外加**可选的一份写面**。
///
/// 两档的差别只有一条：**地址怎么解释** —— 家档是"相对于一个沙盒根的相对路径"，机器档是
/// "绝对路径自己点名的那个卷"（判据仍在 [`crate::sandbox`] 里、只有一份）。名份、上限、
/// 失败形状、复核规则**逐条相同**。
///
/// 家档没有写面时与从前完全一样：只有读侧三项能力。写面只增加"能往哪里写文本"这一件事。
#[derive(Debug)]
pub struct FileApp {
    scope: Scope,
    /// 这个实例的**写面**。`None` = 只读：写侧那两件能力连声明都不出现。
    writable: Option<WritableFace>,
}

/// 这个实例的**面**：家（一个沙盒根 ＋ 相对路径）还是整台机器（绝对路径）。
///
/// 机器档没有"根"——根由每一条路径自己点名的那个卷给出。因此它不复用家档那些
/// `root.join(...)` / `starts_with(root)` 的判据，而是走 `sandbox::machine_path` /
/// `resolve_machine` / `resolve_machine_parent` 那一套（**判据仍然只有一份**）。
#[derive(Debug)]
enum Scope {
    Home(PathBuf),
    Machine,
}

impl Scope {
    fn home(&self) -> Option<&Path> {
        match self {
            Scope::Home(root) => Some(root),
            Scope::Machine => None,
        }
    }

    fn is_machine(&self) -> bool {
        matches!(self, Scope::Machine)
    }
}

/// 这份写面是**谁的**——它决定要不要绑身份与活动，也决定回执里那几句话怎么说。
///
/// 两种面的**归属不同**，因此门也不同（本件里唯一一处"同一个动作、两种规矩"）：
///
/// - [`WritableFace::Granted`]：**宿主授权**给它写的一个已存在子目录。授权单位＝装配（§3.99），
///   所以它绑 `AgentId` 与 `RunId`——那是"别人决定的那一份"。
/// - [`WritableFace::Own`]：**主体自己的作品目录**（`<环境根>/projects`）。主体自己的东西不需要
///   申请（判据逐字：「**自己的东西不用申请，别处的要**」，与 `notes_*` 同族），因此**不绑**
///   身份与活动——它绑的是"这一次装配里，这一格在不在场"。
///
/// ⚠ 两种面**不会**同时出现在同一个实例上（一个 `FileApp` 只有一个写面）。而**一个视图里可以
/// 两处都在**：授权面在资料侧、作品面在主体自己家——各走各的门（动词名也分开：`files_*` 与
/// `projects_*`），因此不需要"一次装配只能有一个写面"这种人造约束。
#[derive(Clone, Debug)]
enum WritableFace {
    /// 已校验、已规范化的授权目标。
    Granted(GrantedCreation),
    /// 主体自己的作品目录：**根就是写面**。
    Own,
    /// **机器档的写面**：整台机器。它不是 `Granted`——那一份绑"资料根里的某一个子目录"，
    /// 而机器档没有那个根；它也不绑 `RunId`（这一档是**支级**的，主人给的是"这一支可以碰
    /// 整台机器"，不是"这一次活动可以写那几个文件"）。
    Machine,
}

/// 一份**已经校验过**的创建授权：绑定身份与活动，目标是规范化之后的绝对目录。
#[derive(Clone, Debug)]
pub(crate) struct GrantedCreation {
    agent: AgentId,
    run: RunId,
    root: PathBuf,
}

impl FileApp {
    /// 打开一个只读文件 App。
    ///
    /// 必须显式给出根目录：没有根目录就是拒绝启动。根必须已存在、是目录、不是符号链接或
    /// reparse point，也不能是文件系统根（`/`、`C:\` 显然不是沙盒）。
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, FailureClass> {
        Self::open_inner(root.into(), None)
    }

    /// 打开**机器档（只读）**：整台机器都可能被读到，写侧那两件能力**不声明**。
    ///
    /// 这一档由**主人**在装配时决定（`--files full`）。它不是主体能申请的东西，
    /// 也不是 [`CreationGrant`]——那份授权说的是"资料根里哪一个子目录可以写"。
    pub fn open_machine() -> Self {
        Self {
            scope: Scope::Machine,
            writable: None,
        }
    }

    /// 打开**机器档（可写）**：读三件 ＋ 写两件，地址一律是**绝对路径**。
    pub fn machine_with_writes() -> Self {
        Self {
            scope: Scope::Machine,
            writable: Some(WritableFace::Machine),
        }
    }

    /// 打开**主体自己的作品面**：根就是写面，不需要任何授权。
    ///
    /// 与 [`FileApp::with_creation`] 的差别只有**归属**（见 [`WritableFace`]）：语义一字不差
    /// ——只新建（`create_new`）、只唯一替换、不改名/不删除/不追加、上限同一份。
    /// 根必须已存在（缺席时由装配方**缺席才建**，见 `file_view`）。
    pub fn with_own_works(root: impl Into<PathBuf>) -> Result<Self, FailureClass> {
        Self::open_inner(root.into(), Some(WritableFace::Own))
    }

    /// 打开一个文件 App，并接受宿主给出的**文本创建授权**。
    ///
    /// 目标是**授权资料根内一个已经存在的子目录**：不存在、不是目录、是链接或 reparse point、
    /// 等于资料根本身、或规范化之后越出资料根，都在这里**拒绝启动**——不静默修正，也不拖到
    /// 真正执行时才说"写不进去"。
    pub fn with_creation(
        root: impl Into<PathBuf>,
        grant: &CreationGrant,
    ) -> Result<Self, FailureClass> {
        let app = Self::open(root)?;
        let Some(home) = app.home().map(Path::to_path_buf) else {
            return Err(FailureClass::InvalidPath);
        };
        let names = relative_names(grant.directory())?;
        // 授权目录是**一个目录名**，不是一个路径。
        if names.len() != 1 {
            return Err(FailureClass::InvalidPath);
        }
        let candidate = home.join(&names[0]);
        let metadata = fs::symlink_metadata(&candidate).map_err(|error| io_class(&error))?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Err(FailureClass::Link);
        }
        if !metadata.is_dir() {
            return Err(FailureClass::WrongKind);
        }
        let canonical = fs::canonicalize(&candidate).map_err(|error| io_class(&error))?;
        // 必须**严格**在资料根之内：等于根本身意味着"整个资料根都可写"，那不是本版给的授权。
        if canonical == home || !canonical.starts_with(&home) {
            return Err(FailureClass::InvalidPath);
        }
        Ok(Self {
            scope: Scope::Home(home),
            writable: Some(WritableFace::Granted(GrantedCreation {
                agent: grant.agent().clone(),
                run: grant.run().clone(),
                root: canonical,
            })),
        })
    }

    fn open_inner(root: PathBuf, writable: Option<WritableFace>) -> Result<Self, FailureClass> {
        let metadata = fs::symlink_metadata(&root).map_err(|error| io_class(&error))?;
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
            return Err(FailureClass::Link);
        }
        if !metadata.is_dir() {
            return Err(FailureClass::WrongKind);
        }

        let canonical = fs::canonicalize(&root).map_err(|error| io_class(&error))?;
        if canonical.parent().is_none() {
            return Err(FailureClass::InvalidPath);
        }
        Ok(Self {
            scope: Scope::Home(canonical),
            writable,
        })
    }

    /// 这个实例有没有写面（视图层据此决定要不要声明那几件写能力）。
    pub(crate) fn is_writable(&self) -> bool {
        self.writable.is_some()
    }

    /// 这次写落在哪儿：家档 ⇒ 一个基准目录（相对路径从它算起）；机器档 ⇒ 由路径自己点名。
    ///
    /// 三种面的**门**在这里分开：授权面绑身份与活动（那是别人的东西，谁、哪一次都要对得上）；
    /// 作品面是主体自己的东西，不绑这两样；机器档是**支级**的整档授权，不绑这一次活动。
    fn write_anchor(&self, agent: &AgentId, run: &RunId) -> Result<WriteAnchor, &'static str> {
        match (&self.writable, &self.scope) {
            (None, _) => Err(REFUSAL_CREATION_NOT_GRANTED),
            (Some(WritableFace::Granted(grant)), Scope::Home(_)) => {
                if grant.agent != *agent || grant.run != *run {
                    return Err(REFUSAL_CREATION_GRANT_MISMATCH);
                }
                Ok(WriteAnchor::Dir(grant.root.clone()))
            }
            (Some(WritableFace::Own), Scope::Home(root)) => Ok(WriteAnchor::Dir(root.clone())),
            (Some(WritableFace::Machine), Scope::Machine) => Ok(WriteAnchor::Machine),
            // 剩下的组合在构造期就被排除了（`Granted`/`Own` 只由家档构造，`Machine` 只由机器档
            // 构造）。落到这里不是"允许"，是**没有这一档**——与"没有写面"同一句话。
            _ => Err(REFUSAL_CREATION_NOT_GRANTED),
        }
    }

    /// **新建**的落点（`files_create` 与它的复核共用）：家档 ⇒ 基准目录里的**单层名字**；
    /// 机器档 ⇒ 绝对路径，父目录必须已经在，末段由独占创建去判在不在。
    ///
    /// 「末段不在」这件事**不能**在这里判：判了就等于把"新建"读成"改已有文件"。
    fn create_target(anchor: WriteAnchor, name: &str) -> Result<(PathBuf, String), FailureClass> {
        match anchor {
            WriteAnchor::Dir(base) => {
                let names = relative_names(name)?;
                // 单层普通文件名：没有第二段，也就没有中间路径可以被替换；控制字符也不收——
                // 它会破坏任何按行渲染这份名字的地方。
                if names.len() != 1 || names[0].chars().any(char::is_control) {
                    return Err(FailureClass::InvalidPath);
                }
                Ok((base, names[0].clone()))
            }
            WriteAnchor::Machine => {
                let target = machine_path(name)?;
                let (dir, last) = resolve_machine_parent(&target)?;
                // 控制字符会破坏任何按行渲染这份名字的地方（家档同一条）。
                if last.chars().any(char::is_control) {
                    return Err(FailureClass::InvalidPath);
                }
                Ok((dir, last))
            }
        }
    }

    /// **改**的落点（`files_edit` 与它的复核共用）：家档 ⇒ 基准目录里的相对路径；
    /// 机器档 ⇒ 绝对路径（末段必须存在、链上不许有链接）。
    fn write_target(anchor: WriteAnchor, path: &str) -> Result<PathBuf, FailureClass> {
        match anchor {
            WriteAnchor::Dir(base) => resolve_in_base(&base, path),
            WriteAnchor::Machine => {
                let target = machine_path(path)?;
                resolve_machine(&target)
            }
        }
    }
}

/// 一次写请求的落点：家档是一个**基准目录**（相对路径从它算起），机器档由路径自己点名。
enum WriteAnchor {
    Dir(PathBuf),
    Machine,
}

/// 能力说明里那句"地址是相对路径还是绝对路径"的档位。
///
/// 它是模型唯一能读到这件事的地方——**说错了，它就会按错的形状发请求**（家档发绝对路径
/// ⇒ `invalid-path`；机器档发相对路径 ⇒ 同样 `invalid-path`）。所以这五句必须跟着档走。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathMode {
    Home,
    Machine,
}

impl PathMode {
    fn of_machine(machine: bool) -> Self {
        if machine {
            PathMode::Machine
        } else {
            PathMode::Home
        }
    }

    /// `files_list` 的 `path`。
    fn list_path(self) -> &'static str {
        match self {
            PathMode::Home => "授权资料根内的相对目录；留空表示资料根",
            PathMode::Machine => "这台机器上的绝对目录（如 `D:\\work`）；必须带盘符，不收相对路径",
        }
    }

    /// `files_read` 的 `path`。
    fn read_path(self) -> &'static str {
        match self {
            PathMode::Home => "授权资料根内的相对文件路径",
            PathMode::Machine => "这台机器上的绝对文件路径（如 `D:\\work\\a.txt`）",
        }
    }

    /// `files_grep` 的 `path`。
    fn grep_path(self) -> &'static str {
        match self {
            PathMode::Home => "授权资料根内的相对目录，只在它之内查；留空表示资料根",
            PathMode::Machine => "这台机器上的绝对目录（如 `D:\\work`），只在它之内查；必须带盘符",
        }
    }

    /// `files_create` / 复核的 `name`。
    fn create_name(self) -> &'static str {
        match self {
            PathMode::Home => "授权目录内的单层普通文件名",
            PathMode::Machine => {
                "这台机器上的绝对文件路径（如 `D:\\work\\new.txt`）；父目录必须已经存在"
            }
        }
    }

    /// `files_edit` / 复核的 `path`。
    fn edit_path(self) -> &'static str {
        match self {
            PathMode::Home => "授权目录内的相对文件路径",
            PathMode::Machine => "这台机器上的绝对文件路径（如 `D:\\work\\a.txt`）",
        }
    }

    /// 写面那一句"落在哪儿"（创建/改写各自的细节里再补一句，说明这一档是整台机器）。
    fn write_scope_note(self) -> &'static str {
        match self {
            PathMode::Home => "",
            PathMode::Machine => {
                "⚠ 这一档是全开：地址一律写绝对路径，写落在路径自己点名的位置（整台机器）。"
            }
        }
    }
}

impl FileApp {
    /// 创建能力的说明（只在有授权时才被声明）。
    pub(crate) fn creation_capability(mode: PathMode) -> Capability {
        Capability::new(
            ActionId::new(CAPABILITY_CREATE_TEXT),
            "在授权目录里新建一个 UTF-8 文本文件（同名的已经在了就明确失败）",
            vec![
                ParamSpec::new(PARAM_NAME, true, mode.create_name()),
                ParamSpec::new(PARAM_TEXT, true, "要写入的 UTF-8 文本，允许为空串"),
            ],
        )
        .with_details(
            [
                "边界：只创建——不覆盖、不追加、不改名、不删除；`name` 是单层文件名\
                 （不含路径分隔符与控制字符）。上限：正文最多 64 KiB，超限整体拒绝（不留半份）。\
                 失败形状：同名已存在 ⇒ 明确失败（那次创建没有发生，也没有覆盖任何东西）；\
                 创建之后写入或同步失败 ⇒ 无法确认（不是「失败」，也不是「成功」）。",
                mode.write_scope_note(),
            ]
            .concat(),
        )
    }

    /// 复核能力的说明（只在有授权时才被声明）。
    ///
    /// 参数与 [`FileApp::creation_capability`] **完全相同**：复核问的是"现在这份参数对应的状态
    /// 是什么"，因此不需要任何"操作句柄"这类新概念。它**只读**、不产生任何外部效果。
    pub(crate) fn verification_capability(mode: PathMode) -> Capability {
        Capability::new(
            ActionId::new(CAPABILITY_VERIFY_TEXT),
            "复核某项创建的当前状态：文件在且字节与参数完全一致 = 成功（只读）",
            vec![
                ParamSpec::new(PARAM_NAME, true, mode.create_name()),
                ParamSpec::new(PARAM_TEXT, true, "当初要写入的那份 UTF-8 文本"),
            ],
        )
        .with_details(
            "三分：文件存在且字节与参数完全一致 = 成功；确认不存在（含父目录不存在、名字形状\
             非法）= 明确失败；存在但字节不一致、不是普通文件或读不出来 = 无法确认。\
             现在时：它回答「现在是什么样」，不声称「那一次创建成功了」——参数与创建完全相同，\
             配对靠参数本身，不靠任何操作句柄。",
        )
    }

    /// 改文本能力的说明（只在有授权时才被声明）。
    ///
    /// 与创建**同挂**创建授权：它同样是写，同样落在宿主显式给过的那**一个目录**里。区别只在
    /// 形状——创建只给**单层**文件名，改给的是授权目录内的**相对路径**（可以在授权目录之下
    /// 任意深度，因为要改的材料常常不在最外层）。
    pub(crate) fn edit_capability(mode: PathMode) -> Capability {
        Capability::new(
            ActionId::new(CAPABILITY_EDIT),
            "把一份已存在文本文件里 `old_text` 的唯一一处换成 `new_text`（只改这一段）",
            vec![
                ParamSpec::new(PARAM_PATH, true, mode.edit_path()),
                ParamSpec::new(
                    PARAM_OLD_TEXT,
                    true,
                    "要被换掉的那段文本；必须非空且在这份文件里唯一出现",
                ),
                ParamSpec::new(PARAM_NEW_TEXT, true, "替换成的文本，允许为空串"),
            ],
        )
        .with_details(
            [
                "唯一定位：`old_text` 必须非空，且在文件里恰好出现一次——一次都没出现、\
                 或出现多次（不知道该换哪一处）都明确失败，而且一个字节都不动。`new_text` 可以是空串\
                 （等于删掉那一段）。`path` 是授权目录内的相对路径，可以任意深度（要改的材料常常不在\
                 最外层）。上限：替换后全文最多 64 KiB，超限整体拒绝、不截断。只替换文本：\
                 不改文件名、不删除文件、不追加内容；替换之后写入或同步失败 ⇒ 无法确认。",
                mode.write_scope_note(),
            ]
            .concat(),
        )
    }

    /// 改文本的复核能力说明（只在有授权时才被声明）。
    ///
    /// 参数与 [`FileApp::edit_capability`] 完全相同：复核问的是"现在这份参数对应的状态是什么"，
    /// 因此不需要任何"操作句柄"这类新概念。它**只读**、不产生任何外部效果。
    pub(crate) fn edit_verification_capability(mode: PathMode) -> Capability {
        Capability::new(
            ActionId::new(CAPABILITY_VERIFY_EDIT),
            "复核某项改写的当前状态：`old_text` 已不在且 `new_text` 在 = 成功（只读）",
            vec![
                ParamSpec::new(PARAM_PATH, true, mode.edit_path()),
                ParamSpec::new(PARAM_OLD_TEXT, true, "当初要被换掉的那段文本"),
                ParamSpec::new(PARAM_NEW_TEXT, true, "当初替换成的文本"),
            ],
        )
        .with_details(
            "三分：`old_text` 已经不在了而且 `new_text` 在 = 成功；`old_text` 还在 = 明确失败\
             （那次替换没有留在盘上）；读不出来（不存在、不是普通文件、读失败、不是合法 UTF-8）=\
             无法确认。现在时：它回答「现在是什么样」，不声称「那一次改写成功了」；\
             参数与改写完全相同，配对靠参数本身。",
        )
    }

    /// 这个实例当前可用的能力：读侧三项，加上**有写面时**的那两项写能力与它们各自的复核能力。
    pub(crate) fn available_capabilities(&self) -> Capabilities {
        let mode = PathMode::of_machine(self.scope.is_machine());
        let mut items = Self::capabilities_in(mode).items().to_vec();
        if self.is_writable() {
            items.push(Self::creation_capability(mode));
            items.push(Self::verification_capability(mode));
            items.push(Self::edit_capability(mode));
            items.push(Self::edit_verification_capability(mode));
        }
        Capabilities::of(items)
    }

    /// 这一次活动可以用的能力说明（三项只读，顺序固定；**家档**）。
    ///
    /// 它不是全局注册表：调用方在装配一次活动时显式把这份说明放进去，不注册、不安装、不发现。
    /// 说明里给的是**作用域、参数语义、上限与截断行为**，不写"你应该先找再读"这类编排建议。
    pub fn capabilities() -> Capabilities {
        Self::capabilities_in(PathMode::Home)
    }

    /// 按档位给的那份说明（机器档的差别只有一处：地址是绝对路径）。
    pub(crate) fn capabilities_in(mode: PathMode) -> Capabilities {
        Capabilities::of(vec![
            Capability::new(
                ActionId::new(CAPABILITY_LIST),
                "列出授权资料根里某个目录的子项（默认只看直接子项；给 `depth` 就往下递归，按名称排序）；接着列用 `after`",
                vec![
                    ParamSpec::new(PARAM_PATH, false, mode.list_path()),
                    ParamSpec::new(
                        PARAM_AFTER,
                        false,
                        "上一次返回过的条目名字（递归时是相对路径）；只返回严格大于它的下一批",
                    ),
                    ParamSpec::new(
                        PARAM_DEPTH,
                        false,
                        "递归深度：不给 ＝ 1（只列直接子项）；最多 8 层",
                    ),
                ],
            )
            .with_details(
                "读数：首行 `more` 是机械读数——`none` = 没有更多，`after <名字>` = 下次用这个名字\
                 接着列。上限：一次最多 100 项。边界：`after` 是名字不是下标；它不属于\
                 这个目录时明确失败（不猜近似名）。递归（`depth` > 1）：条目名是相对路径\
                 （`a/b.txt`），扫描最多 200 个目录（有界），不进符号链接 / junction。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_READ_TEXT),
                "读一个 UTF-8 文本文件；整份读，或给 `offset`/`limit` 续读一段字节，或给 `from_line`/`lines` 按行读一段",
                vec![
                    ParamSpec::new(PARAM_PATH, true, mode.read_path()),
                    ParamSpec::new(PARAM_OFFSET, false, "本段的起始字节（默认 0）"),
                    ParamSpec::new(PARAM_LIMIT, false, "本段的字节数上限（默认 64 KiB）"),
                    ParamSpec::new(PARAM_FROM_LINE, false, "按行读的起点行号，从 1 起算"),
                    ParamSpec::new(PARAM_LINES, false, "按行读的行数；不给 ＝ 到文件末尾"),
                ],
            )
            .with_details(
                "上限：单次最多 64 KiB，超限拒绝、不截断。续读：`offset` 是起始字节\
                 （默认 0，必须落在字符边界上、且不超过文件长度），`limit` 是本段字节数（默认到\
                 64 KiB 上限，超过就拒绝）——它是字节数不是行数。读数：续读时另给一行——本段\
                 覆盖的字节区间、全文总字节数、`more`（`none` = 到底了，`next <字节>` = 下次从这里接着读）。\
                 按行读：`from_line` 从 1 起算、`lines` 是行数（不给 ⇒ 到末尾），它与 `files_grep`\
                 报的 `line=N` 是同一个坐标系。⚠ 两套坐标不许同用（与字节窗口同时给 ⇒ 明确失败）；\
                 起点超过总行数、或这一段 / 整份文件超过 64 KiB ⇒ 明确失败、不截断。读数另给一行\
                 `lines = a..b, total = n, more = none|next-line <行>`。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_FIND_TEXT),
                "在授权资料根里按子串找材料（文件名与正文都看，区分大小写）；\
                 `path` 指到 `history` 时查你自己已确认的经历（最近的先，命中给活动/段 + 行号）",
                vec![
                    ParamSpec::new(PARAM_PATTERN, true, "要匹配的子串，区分大小写，不能为空"),
                    ParamSpec::new(
                        PARAM_PATH,
                        false,
                        mode.grep_path(),
                    ),
                ],
            )
            .with_details(
                "匹配：子串不是正则；文件名与正文都算（正文按行看）。候选形状：路径 + \
                 命中位置（`name` 或 `line=N`）+ 那一段片段（片段最多 120 字节，超长以 `…` 截断\
                 ——正文仍在盘上，要用就 `files_read`）。结果按路径名排序，同一条材料最多出现一次。\
                 上限：最多扫 500 个文件、最多返回 100 条候选；触到任一上限时首行如实报截断。\
                 不搜 `history/`（经历有自己的分段结构，用 `files_list` + `files_read` 看）。",
            ),
        ])
    }

    /// 家档的沙盒根。只用于诊断与测试，**不进入任何 Model 可见内容**。
    ///
    /// 机器档没有单一根（根由每一条绝对路径自己点名的那个卷给出）⇒ `None`。
    pub fn home(&self) -> Option<&Path> {
        self.scope.home()
    }

    /// 列目录：直接子项（`depth` > 1 时递归；**不给就是 1，与从前逐字节相同**）、最多
    /// [`MAX_ENTRIES`] 项、明确标记截断；可用 `after` 接着列。
    ///
    /// `after` 传了一个不存在于这个列表里的名字时**明确失败**（`invalid-argument`），不静默当成
    /// "从头开始"——静默重来会让主体以为自己在往前走。
    ///
    /// ⚠ **2026-10-14 加 `depth`**（"看代码"那一格）：真机账上：看项目结构只能走 shell
    /// （`Get-ChildItem -Recurse`）——因为这里**不递归**。加宽只加一格参数：递归时条目名是
    /// **相对路径**（`after` 照旧是"严格大于它的下一批"），扫描与返回**各有界**。
    pub(crate) fn list(&self, params: &ActionParams) -> ActionOutcome {
        let relative = match path_parameter_with(params, false, &[PARAM_AFTER, PARAM_DEPTH]) {
            Ok(Some(value)) => value,
            Ok(None) => String::new(),
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };
        let after = params.get(PARAM_AFTER);
        // `depth` 与字节数共用同一个机械解析（非空 + 全 ASCII 数字；不做"尽力解析"、不回落默认值）。
        let depth = match byte_parameter(params, PARAM_DEPTH) {
            Ok(Some(value)) => value,
            Ok(None) => 1,
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };
        if depth == 0 || depth > MAX_DEPTH {
            return ActionOutcome::failure(FailureClass::InvalidArgument.as_str());
        }

        let listed = match depth {
            1 => self.entries_of(&relative),
            _ => self.entries_deep_of(&relative, depth as usize),
        };
        match listed {
            // 机械顺序：按名称稳定排列（递归时是相对路径），不是重要性排序。
            Ok(items) => match render_listing_page(&items, after) {
                Ok(text) => ActionOutcome::success(vec![ContentBlock::new(CAPABILITY_LIST, text)]),
                Err(class) => ActionOutcome::failure(class.as_str()),
            },
            Err(class) => ActionOutcome::failure(class.as_str()),
        }
    }

    /// **递归列目录**（`depth` > 1）：条目名是相对路径（`a/b.txt`），整页按名称排序。
    ///
    /// 两条有界：**扫描**最多 [`MAX_DIRECTORIES`] 个目录（触到就停）、**返回**由
    /// [`render_listing_page`] 一页最多 [`MAX_ENTRIES`] 项。触到扫描上界时**照样返回**已看到的
    /// （不谎称"没有更多"——续列走到哪里由 `after` 在整份全序上决定）。
    ///
    /// ⚠ **不进任何 reparse point**（junction / 符号链接）：与 [`Self::entries_of`] 同一判据
    /// ——递归不是绕过那条边界的方法。
    fn entries_deep_of(
        &self,
        relative: &str,
        depth: usize,
    ) -> Result<Vec<DirectoryEntry>, FailureClass> {
        let root = self.resolve_existing(relative)?;
        let mut items: Vec<DirectoryEntry> = Vec::new();
        let mut scanned = 0usize;
        let mut queue: Vec<(PathBuf, String, usize)> = vec![(root, String::new(), 1)];
        while let Some((directory, prefix, level)) = queue.pop() {
            if scanned >= MAX_DIRECTORIES {
                break;
            }
            scanned += 1;
            let entries = fs::read_dir(&directory).map_err(|error| io_class(&error))?;
            let mut children: Vec<(String, PathBuf, &'static str)> = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|error| io_class(&error))?;
                let kind = match entry.file_type() {
                    Ok(file_type) if file_type.is_symlink() => "link",
                    Ok(file_type) if file_type.is_dir() => "dir",
                    Ok(_) => "file",
                    Err(_) => "unknown",
                };
                children.push((
                    entry.file_name().to_string_lossy().into_owned(),
                    entry.path(),
                    kind,
                ));
            }
            children.sort();
            for (name, path, kind) in children {
                let shown = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                items.push((shown, kind));
                if kind == "dir" && level < depth {
                    let child_prefix = if prefix.is_empty() {
                        name
                    } else {
                        format!("{prefix}/{name}")
                    };
                    queue.push((path, child_prefix, level + 1));
                }
            }
        }
        items.sort();
        Ok(items)
    }

    /// 目录的**机械条目**（名称 + 种类），按名称排序，**不在这里截断**。
    ///
    /// 它是 `files_list` 的唯一实现，供视图层（`file_view`）复用它自己的路径校验与目录读取，
    /// 而不是在外面重新实现一遍遍历规则。返回**全部**直接子项：续列（`after`）要在一个稳定的
    /// 全序上取"严格大于它的那些"，只读前 100 个再排序给不出这个全序。返回有界（一页 ≤100 条）
    /// 与读取有界是两件事。
    pub(crate) fn entries_of(&self, relative: &str) -> Result<Vec<DirectoryEntry>, FailureClass> {
        let directory = self.resolve_existing(relative)?;
        let metadata = fs::metadata(&directory).map_err(|error| io_class(&error))?;
        if !metadata.is_dir() {
            return Err(FailureClass::WrongKind);
        }

        let entries = fs::read_dir(&directory).map_err(|error| io_class(&error))?;

        let mut items: Vec<DirectoryEntry> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| io_class(&error))?;
            let kind = match entry.file_type() {
                Ok(file_type) if file_type.is_symlink() => "link",
                Ok(file_type) if file_type.is_dir() => "dir",
                Ok(_) => "file",
                Err(_) => "unknown",
            };
            items.push((entry.file_name().to_string_lossy().into_owned(), kind));
        }

        items.sort();
        Ok(items)
    }

    /// 读文本：普通 UTF-8 文件、最多 [`MAX_TEXT_BYTES`] 字节、有界读取；可用 `offset` / `limit` 续读。
    ///
    /// **不给窗口参数时走的就是原来那条路**（整份、有界、超限拒绝不截断、**没有**任何读数行），
    /// 因此旧调用零影响；给了 `offset` / `limit`（或 `from_line` / `lines`）才是分段读。
    pub(crate) fn read_text(&self, params: &ActionParams) -> ActionOutcome {
        let relative = match path_parameter_with(
            params,
            true,
            &[PARAM_OFFSET, PARAM_LIMIT, PARAM_FROM_LINE, PARAM_LINES],
        ) {
            Ok(Some(value)) => value,
            Ok(None) => return ActionOutcome::failure(FailureClass::MissingParameter.as_str()),
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };
        if relative.is_empty() {
            return ActionOutcome::failure(FailureClass::InvalidPath.as_str());
        }
        let offset = match byte_parameter(params, PARAM_OFFSET) {
            Ok(value) => value,
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };
        let limit = match byte_parameter(params, PARAM_LIMIT) {
            Ok(value) => value,
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };
        // 单段长度上限与整份读取同源：一次续读不可能拿到比它更多的字节。
        if limit.is_some_and(|limit| limit > MAX_TEXT_BYTES) {
            return ActionOutcome::failure(FailureClass::TooLarge.as_str());
        }
        // **按行读**（2026-10-14）：与 `files_grep` 的 `line=N` 同一坐标系。
        let from_line = match byte_parameter(params, PARAM_FROM_LINE) {
            Ok(value) => value,
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };
        let lines = match byte_parameter(params, PARAM_LINES) {
            Ok(value) => value,
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };
        if from_line.is_some() || lines.is_some() {
            // **两套坐标不许同用**：字节窗口与行窗口同时给 ⇒ 明确失败，不替主体挑一个
            //（"两个都像对"是读数里最坏的一种）。只给 `lines` 而不给起点也一样拒绝。
            let Some(from_line) = from_line else {
                return ActionOutcome::failure(FailureClass::InvalidArgument.as_str());
            };
            if offset.is_some() || limit.is_some() {
                return ActionOutcome::failure(FailureClass::InvalidArgument.as_str());
            }
            return self.read_text_lines(&relative, from_line, lines.unwrap_or(u64::MAX));
        }

        match (offset, limit) {
            (None, None) => self.read_text_whole(&relative),
            (offset, limit) => self.read_text_window(
                &relative,
                offset.unwrap_or(0),
                limit.unwrap_or(MAX_TEXT_BYTES),
            ),
        }
    }

    /// **按行读**（`from_line` 从 1 起算；`lines` 不给 ⇒ 到末尾）。
    ///
    /// 判据与字节窗口同款，且**两个坐标系各有一条同样的硬规则**：
    ///
    /// 1. 起点超过总行数 ⇒ **明确失败**（`invalid-argument`），不返回空串（与"字节越界"同款）；
    /// 2. 这一段文本超过 [`MAX_TEXT_BYTES`] ⇒ **拒绝、不截断**；**整份文件**超过同一上限也拒绝
    ///    ——按行读只服务**有界文本**，更大的用 `offset`/`limit` 分段（不在这里偷偷整份读进来）；
    /// 3. 读数另给一行 `lines = a..b, total = n, more = none|next-line <行>`——与字节窗口那一行
    ///    同族（`bytes = a..b, total = n, more = next <字节>`）。
    fn read_text_lines(&self, relative: &str, from_line: u64, lines: u64) -> ActionOutcome {
        let path = match self.resolve_existing(relative) {
            Ok(path) => path,
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => return ActionOutcome::failure(io_class(&error).as_str()),
        };
        if metadata.len() > MAX_TEXT_BYTES {
            return ActionOutcome::failure(FailureClass::TooLarge.as_str());
        }
        let text = match read_text_file(&path) {
            Ok(text) => text,
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };
        let all: Vec<&str> = text.lines().collect();
        let total = all.len() as u64;
        if from_line == 0 || from_line > total.max(1) {
            return ActionOutcome::failure(FailureClass::InvalidArgument.as_str());
        }
        let start = (from_line - 1) as usize;
        let wanted = lines.min(usize::MAX as u64) as usize;
        let end = start.saturating_add(wanted).min(all.len());
        let segment = all[start..end].join("\n");
        if segment.len() as u64 > MAX_TEXT_BYTES {
            return ActionOutcome::failure(FailureClass::TooLarge.as_str());
        }
        let more = match (end as u64) < total {
            true => format!("next-line {}", end as u64 + 1),
            false => "none".to_string(),
        };
        ActionOutcome::success(vec![
            ContentBlock::new(CAPABILITY_READ_TEXT, segment),
            ContentBlock::new(
                "files_read-lines",
                format!(
                    "lines = {from_line}..{}, total = {total}, more = {more}",
                    end as u64
                ),
            ),
        ])
    }

    /// 整份读取（`files_read` 不带窗口参数时的唯一实现）。
    fn read_text_whole(&self, relative: &str) -> ActionOutcome {
        let path = match self.resolve_existing(relative) {
            Ok(path) => path,
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };

        match read_text_file(&path) {
            // 只把字节当文本交出去；内容里的任何指令都只是数据。
            Ok(text) => ActionOutcome::success(vec![ContentBlock::new(CAPABILITY_READ_TEXT, text)]),
            Err(class) => ActionOutcome::failure(class.as_str()),
        }
    }

    /// 续读一段：把 `[offset, offset + limit)` 这一段字节交出去（`limit` ≤ [`MAX_TEXT_BYTES`]）。
    ///
    /// 三条硬规则：
    ///
    /// 1. `offset` 落在 UTF-8 字符中间 ⇒ **明确失败**（`invalid-argument`），绝不切出半个字符：
    ///    判据是本段第一个字节不是续字节（`0b10xxxxxx`）——文件是合法 UTF-8 时，这恰好等价于
    ///    "`offset` 是字符边界"；
    /// 2. `offset` 超过文件长度 ⇒ **明确失败**（`invalid-argument`），不返回空串；落在**恰好等于**
    ///    长度处是合法的"最后一页之后"：本段为空、`more = none`——那不是越界，是到底；
    /// 3. 有界读取：即使文件比整份读取的上限大得多（续读正是为它准备的），也只读这一段。
    ///
    /// **分段与整份的关系**：把各段按顺序拼起来 == 全文（文件不变时），读数里的区间与总长度
    /// 就是这一次看到的真相。
    ///
    /// 与整份读取的一处**刻意不同**：这里不对整份文件做编码校验（那要求整份读进来，正是续读要
    /// 避免的事），只要求**这一段**是合法 UTF-8。它是如实的分段读，不是"整份校验过"的证明。
    fn read_text_window(&self, relative: &str, offset: u64, limit: u64) -> ActionOutcome {
        let path = match self.resolve_existing(relative) {
            Ok(path) => path,
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };

        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(error) => return ActionOutcome::failure(io_class(&error).as_str()),
        };
        let metadata = match file.metadata() {
            Ok(metadata) => metadata,
            Err(error) => return ActionOutcome::failure(io_class(&error).as_str()),
        };
        if !metadata.is_file() {
            return ActionOutcome::failure(FailureClass::WrongKind.as_str());
        }
        let total = metadata.len();
        if offset > total {
            return ActionOutcome::failure(FailureClass::InvalidArgument.as_str());
        }
        if offset == total {
            return ActionOutcome::success(vec![
                ContentBlock::new("files_read-window", window_readings(offset, offset, total)),
                ContentBlock::new(CAPABILITY_READ_TEXT, String::new()),
            ]);
        }

        if let Err(error) = file.seek(SeekFrom::Start(offset)) {
            return ActionOutcome::failure(io_class(&error).as_str());
        }
        let want = limit.min(total - offset);
        let mut bytes = Vec::new();
        if let Err(error) = file.by_ref().take(want).read_to_end(&mut bytes) {
            return ActionOutcome::failure(io_class(&error).as_str());
        }
        // 第一个字节是续字节 ⇒ 起始位置落在某个字符中间：明确失败，不切半个字符。
        if bytes.first().is_some_and(|byte| byte & 0xC0 == 0x80) {
            return ActionOutcome::failure(FailureClass::InvalidArgument.as_str());
        }

        match String::from_utf8(bytes) {
            Ok(text) => {
                let end = offset + text.len() as u64;
                ActionOutcome::success(vec![
                    ContentBlock::new("files_read-window", window_readings(offset, end, total)),
                    ContentBlock::new(CAPABILITY_READ_TEXT, text),
                ])
            }
            Err(_) => ActionOutcome::failure(FailureClass::NotUtf8.as_str()),
        }
    }

    /// 按**子串**查找材料（文件名 + 正文），返回候选清单。
    ///
    /// 只做四件事：在授权资料根之内机械地走目录、按子串匹配、给出路径与命中位置、如实报上限与
    /// 截断。**不排序相关性、不评分、不摘要、不索引**——顺序只有一个来源：路径名。同一条材料
    /// 最多出现一次（取第一处命中）。
    ///
    /// 参数：`pattern` 必填（子串、区分大小写、不能为空），`path` 可选（限定子树，默认资料根）。
    pub(crate) fn find_text(&self, params: &ActionParams) -> ActionOutcome {
        let pattern = match search_pattern(params) {
            Ok(pattern) => pattern,
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };
        let relative = params.get(PARAM_PATH).unwrap_or("");

        let directory = match self.resolve_existing(relative) {
            Ok(directory) => directory,
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };
        match fs::metadata(&directory) {
            // `path` 限定的是**子树**，不是"就查这一个文件"：给文件就明确说类型不对。
            Ok(metadata) if !metadata.is_dir() => {
                return ActionOutcome::failure(FailureClass::WrongKind.as_str());
            }
            Ok(_) => {}
            Err(error) => return ActionOutcome::failure(io_class(&error).as_str()),
        }

        let mut scan = ScanState::default();
        let mut found: Vec<FoundMatch> = Vec::new();
        // 显式栈而不是递归：目录层级很深时不拿调用栈去赌。
        let mut pending: Vec<(PathBuf, String)> = vec![(directory, relative.to_string())];
        while let Some((directory, prefix)) = pending.pop() {
            let children = match sorted_children(&directory) {
                Ok(children) => children,
                Err(class) => return ActionOutcome::failure(class.as_str()),
            };
            let mut deeper: Vec<(PathBuf, String)> = Vec::new();
            for child in children {
                let Child { name, path, kind } = child;
                let child_path = match prefix.is_empty() {
                    true => name.clone(),
                    false => format!("{prefix}/{name}"),
                };
                match kind {
                    // 链接 / reparse point 一律**不跟进**、也不当成材料：它的真实内容在别处。
                    ChildKind::Link => {}
                    ChildKind::Dir => deeper.push((path, child_path)),
                    ChildKind::File => {
                        if scan.scanned >= MAX_SCANNED {
                            // 触到扫描上限：**停下并如实报截断**，不把"扫到一半"说成"只有这些"。
                            scan.truncated = true;
                            pending.clear();
                            deeper.clear();
                            break;
                        }
                        scan.scanned += 1;
                        if let Some(found_match) =
                            classify(&child_path, &name, &path, pattern, &mut scan)
                        {
                            found.push(found_match);
                        }
                    }
                }
            }
            // 反序压栈：出栈即名称升序，结果与文件系统的返回顺序无关。
            for entry in deeper.into_iter().rev() {
                pending.push(entry);
            }
        }

        // 路径名序（与 `files_list` 的机械顺序同源），与命中次数无关；同一条材料只出现一次。
        found.sort_by(|left, right| left.path.cmp(&right.path));
        ActionOutcome::success(vec![ContentBlock::new(
            CAPABILITY_FIND_TEXT,
            render_matches(&found, &scan),
        )])
    }

    /// 把一个地址解析成绝对路径（**按档分派**：家档 = 相对路径；机器档 = 绝对路径）。
    ///
    /// 规则是**拒绝优先**，而且**先校验形状、再碰文件系统**：[`relative_names`] /
    /// [`machine_path`] 先只看形状（`.`、`..`、根、盘符前缀、UNC、设备命名空间一律拒绝），
    /// 这一遍不动文件系统，所以 `sub/../../x` 会先被判非法，而不是先撞上"目录不存在"；
    /// 之后才逐段确认存在性与符号链接 / junction / 其他 reparse point。
    ///
    /// ⚠️ 本函数只做"检查"；真正的读发生在之后。检查与打开之间理论上存在替换窗口，
    /// 本版把沙盒限定为**受信目录**（测试自建、不允许不可信进程并发修改），因此
    /// **不构成 OS 级安全沙盒**。
    fn resolve_existing(&self, path: &str) -> Result<PathBuf, FailureClass> {
        match &self.scope {
            Scope::Home(root) => {
                let names = relative_names(path)?;
                // 空路径 = 沙盒根本身（列目录要它），因此这里不额外判空。
                resolve_within(root, &names)
            }
            // 机器档：`machine_path` 会拒掉相对路径（"相对于什么"在这里没有唯一答案）。
            Scope::Machine => {
                let target = machine_path(path)?;
                resolve_machine(&target)
            }
        }
    }

    /// 在授权目录里**新建**一个文本文件。
    ///
    /// 顺序是固定的：复核写面（身份/授权，或"这是主体自己的目录"）→ 校验参数与文件名 →
    /// `create_new` 独占创建 → 写入 → 同步。
    ///
    /// 从"文件已经被创建"那一刻起，任何失败都**不能**被说成"什么都没发生"：写入失败或同步失败
    /// 一律报**无法确认**，文件留在盘上由主体自己判断——系统不删除、不补写、不重试，也不把
    /// "写了一半"说成一次干净的成功。
    ///
    /// 授权复核在这里是**第二道**：准入已经拦过没有授权、身份不符的请求；这里再确认一次，是因为
    /// 真正要动文件的是这一步。它失败时属于"已派发之后的真实失败"，所以报 `failure`，而不是拒绝。
    pub(crate) fn create_text(
        &self,
        agent: &AgentId,
        operation: &OperationId,
        params: &ActionParams,
    ) -> ActionOutcome {
        for (name, _) in params.entries() {
            if name != PARAM_NAME && name != PARAM_TEXT {
                return ActionOutcome::failure(FailureClass::UnknownParameter.as_str());
            }
        }
        let (Some(name), Some(text)) = (params.get(PARAM_NAME), params.get(PARAM_TEXT)) else {
            return ActionOutcome::failure(FailureClass::MissingParameter.as_str());
        };

        // 落点判据只有一处（`create_target`）：家档是"基准目录里的单层名字"，机器档是
        // "绝对路径，父目录要在、末段不在"。这里**不再自己拼路径**。
        let anchor = match self.write_anchor(agent, operation.run()) {
            Ok(anchor) => anchor,
            Err(class) => return ActionOutcome::failure(class),
        };
        let (dir, file_name) = match Self::create_target(anchor, name) {
            Ok(target) => target,
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };
        // 上限：整体拒绝，不截断。空文本合法（0 字节文件是一个明确的效果）。
        if text.len() > MAX_CREATE_BYTES {
            return ActionOutcome::failure(FailureClass::TooLarge.as_str());
        }

        let target = dir.join(&file_name);
        // **独占创建**：`create_new` 是真实的原子语义，不用"先 exists 再普通写入"冒充。
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(file) => file,
            Err(error) => return ActionOutcome::failure(create_failure(&error).as_str()),
        };

        // 文件已经存在了。从这里开始的失败都不再说"没有发生"。
        // 用限定写法调用 `Write::write_all`：同一个文件对象上 `Read`/`Write` 都有 `by_ref`，
        // 把 `Write` 整个引入作用域会让既有的读取代码产生方法歧义。
        if std::io::Write::write_all(&mut file, text.as_bytes()).is_err() {
            return ActionOutcome::unknown();
        }
        if file.sync_all().is_err() {
            return ActionOutcome::unknown();
        }

        ActionOutcome::success(vec![ContentBlock::new(
            "created",
            format!("{} ({} bytes)", file_name, text.len()),
        )])
    }

    /// 复核一次创建**现在的真实状态**（不是"当时发生了什么"）。
    ///
    /// 顺序与 [`FileApp::create_text`] 完全对应（身份/授权 → 参数形状 → 名字形状 → 上限），
    /// 然后**只读**地看那个名字现在的样子。三分是刻意分开的：
    ///
    /// - **成功**：文件存在、是普通文件、而且字节与参数**完全相同**；
    /// - **明确失败**：确认不存在（`NotFound`，含父目录不存在、名字形状非法）——"这一次的效果
    ///   确实没有留在盘上"；
    /// - **无法确认**（[`ActionOutcome::unknown`]）：存在但字节不一致、不是普通文件、是链接 /
    ///   reparse point，或根本读不出来。
    ///
    /// 第三条的分寸是本能力的关键：**"存在一份不一样的东西"既不能断言成功、也不能断言失败**，
    /// 所以如实报"无法确认"，不得为了让写类型的门打开而报成功。名称与参数超上限同理——
    /// 那份参数不可能对应任何一次真实创建，属于确定不存在。
    pub(crate) fn verify_text(
        &self,
        agent: &AgentId,
        operation: &OperationId,
        params: &ActionParams,
    ) -> ActionOutcome {
        for (name, _) in params.entries() {
            if name != PARAM_NAME && name != PARAM_TEXT {
                return ActionOutcome::failure(FailureClass::UnknownParameter.as_str());
            }
        }
        let (Some(name), Some(text)) = (params.get(PARAM_NAME), params.get(PARAM_TEXT)) else {
            return ActionOutcome::failure(FailureClass::MissingParameter.as_str());
        };

        // 与 `create_text` **同一个落点判据**（`create_target`）：复核问的是"那份参数对应的
        // 位置现在是什么样"，两边各拼一次路径就会在某一档上验错地方。
        let anchor = match self.write_anchor(agent, operation.run()) {
            Ok(anchor) => anchor,
            Err(class) => return ActionOutcome::failure(class),
        };
        let (dir, file_name) = match Self::create_target(anchor, name) {
            Ok(target) => target,
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };
        if text.len() > MAX_CREATE_BYTES {
            return ActionOutcome::failure(FailureClass::TooLarge.as_str());
        }

        let target = dir.join(&file_name);
        let metadata = match fs::symlink_metadata(&target) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return ActionOutcome::failure(FailureClass::NotFound.as_str());
            }
            // 取不到形状（权限、路径过长、其他 I/O 失败）：读不出来就是读不出来。
            Err(_) => return ActionOutcome::unknown(),
        };
        // 不是普通文件就不做字节比较：目录、符号链接与 reparse point 都算"无法确认"。
        if metadata.file_type().is_symlink() || is_reparse_point(&metadata) || !metadata.is_file() {
            return ActionOutcome::unknown();
        }
        let bytes = match fs::read(&target) {
            Ok(bytes) => bytes,
            Err(_) => return ActionOutcome::unknown(),
        };
        if bytes != text.as_bytes() {
            return ActionOutcome::unknown();
        }

        ActionOutcome::success(vec![ContentBlock::new(
            "verified",
            format!("{} ({} bytes match)", file_name, text.len()),
        )])
    }

    /// 写面找不到目标时，把"为什么找不到"说全。
    ///
    /// 改写的落点是**授权目录**，而读的落点是资料根：同一个相对名字，在只读侧存在、
    /// 在写侧不存在，是完全正常的。若只报 `not-found`，主体会以为路径写错、反复重试；
    /// 所以这里补一句只有系统知道的事实——它在只读侧，不在这只手能改的范围里。
    fn missing_write_target(&self, relative: &str, class: FailureClass) -> ActionOutcome {
        if class == FailureClass::NotFound && self.resolve_existing(relative).is_ok() {
            return ActionOutcome::failure_with_content(
                class.as_str(),
                vec![ContentBlock::new(
                    "write-scope",
                    format!(
                        "{relative} 在只读侧存在，但写面只在授权目录内——读得到，不等于改得到。"
                    ),
                )],
            );
        }
        ActionOutcome::failure(class.as_str())
    }

    /// 把授权目录里一份已存在文本文件中 `old_text` 的**唯一一处**出现换成 `new_text`。
    ///
    /// 顺序是固定的：复核身份/授权 → 参数形状 → 解析路径 → 读取全文 → **唯一性判定** →
    /// 换算新全文与上限 → 截断重写 → 同步。
    ///
    /// 唯一性是本能力的关键：`old_text` 一次都没出现是"没找到"，出现多次是"不知道该换哪一处"，
    /// 两者都**明确失败**，而且一个字节都不动——不换第一处、不做全部替换、也不猜。
    ///
    /// 从"文件已经被截断重写"那一刻起，任何失败都不能被说成"什么都没发生"：写入失败或同步
    /// 失败一律报**无法确认**，文件留在盘上由主体自己判断——系统不删除、不补写、不重试，也不把
    /// "改了一半"说成一次干净的成功。
    pub(crate) fn edit_text(
        &self,
        agent: &AgentId,
        operation: &OperationId,
        params: &ActionParams,
    ) -> ActionOutcome {
        for (name, _) in params.entries() {
            if name != PARAM_PATH && name != PARAM_OLD_TEXT && name != PARAM_NEW_TEXT {
                return ActionOutcome::failure(FailureClass::UnknownParameter.as_str());
            }
        }
        let (Some(relative), Some(old_text), Some(new_text)) = (
            params.get(PARAM_PATH),
            params.get(PARAM_OLD_TEXT),
            params.get(PARAM_NEW_TEXT),
        ) else {
            return ActionOutcome::failure(FailureClass::MissingParameter.as_str());
        };
        // 空串会在每个位置匹配，那不是"替换某一段"：明确拒绝，不解释成"在开头插入"。
        // `new_text` 空串则合法——换成空就是把那一段删掉。
        if old_text.is_empty() {
            return ActionOutcome::failure(FailureClass::InvalidArgument.as_str());
        }

        // 落点判据只有一处（`write_target`）：家档相对基准目录，机器档是绝对路径。
        let anchor = match self.write_anchor(agent, operation.run()) {
            Ok(anchor) => anchor,
            Err(class) => return ActionOutcome::failure(class),
        };
        let target = match Self::write_target(anchor, relative) {
            Ok(target) => target,
            Err(class) => return self.missing_write_target(relative, class),
        };

        let current = match read_text_file(&target) {
            Ok(text) => text,
            Err(class) => return self.missing_write_target(relative, class),
        };

        // 唯一性：先取第一处，再多看一处。两处都只在**内存里**判定，盘上一个字节都没动。
        let mut matches = current.match_indices(old_text);
        let Some((start, _)) = matches.next() else {
            return ActionOutcome::failure(FailureClass::NotFound.as_str());
        };
        if matches.next().is_some() {
            return ActionOutcome::failure(FailureClass::InvalidArgument.as_str());
        }

        let mut next =
            String::with_capacity(current.len() + new_text.len().saturating_sub(old_text.len()));
        next.push_str(&current[..start]);
        next.push_str(new_text);
        next.push_str(&current[start + old_text.len()..]);
        // 上限：整体拒绝，不截断。先算后写——拒绝时盘上仍旧是原样。
        if next.len() > MAX_CREATE_BYTES {
            return ActionOutcome::failure(FailureClass::TooLarge.as_str());
        }

        // 文件已经在这里了。从截断这一刻开始的失败都不再说"没有发生"。
        let mut file = match OpenOptions::new().write(true).truncate(true).open(&target) {
            Ok(file) => file,
            Err(error) => return ActionOutcome::failure(io_class(&error).as_str()),
        };
        if std::io::Write::write_all(&mut file, next.as_bytes()).is_err() {
            return ActionOutcome::unknown();
        }
        if file.sync_all().is_err() {
            return ActionOutcome::unknown();
        }

        ActionOutcome::success(vec![ContentBlock::new(
            "edited",
            format!(
                "{} (1 match, {} bytes -> {} bytes)",
                relative,
                current.len(),
                next.len()
            ),
        )])
    }

    /// 复核一次改写**现在的真实状态**（不是"当时发生了什么"）。
    ///
    /// 顺序与 [`FileApp::edit_text`] 完全对应（身份/授权 → 参数形状 → 解析路径），然后**只读**
    /// 地看那份文件现在的样子。三分是刻意分开的：
    ///
    /// - **成功**：`old_text` 已经不在了，而且 `new_text` 在；
    /// - **明确失败**（`not-found`）：`old_text` **还在**——那次替换确实没有留在盘上（与
    ///   [`FileApp::verify_text`] 的失败分支同一含义：这次的效果没留在盘上）；路径形状本身
    ///   不合法（`invalid-path`）同属明确失败，因为那份参数不可能对应任何一次真实改写；
    /// - **无法确认**（[`ActionOutcome::unknown`]）：读不出来——不存在、不是普通文件、是链接 /
    ///   reparse point、读失败、不是合法 UTF-8，或 `old_text` 不在但 `new_text` 也不在。
    ///
    /// 最后一条的分寸与复核创建一致：**"存在一份不一样的东西"既不能断言成功、也不能断言失败**，
    /// 所以如实报"无法确认"，不得为了让写类型的门打开而报成功。
    pub(crate) fn verify_edit_text(
        &self,
        agent: &AgentId,
        operation: &OperationId,
        params: &ActionParams,
    ) -> ActionOutcome {
        for (name, _) in params.entries() {
            if name != PARAM_PATH && name != PARAM_OLD_TEXT && name != PARAM_NEW_TEXT {
                return ActionOutcome::failure(FailureClass::UnknownParameter.as_str());
            }
        }
        let (Some(relative), Some(old_text), Some(new_text)) = (
            params.get(PARAM_PATH),
            params.get(PARAM_OLD_TEXT),
            params.get(PARAM_NEW_TEXT),
        ) else {
            return ActionOutcome::failure(FailureClass::MissingParameter.as_str());
        };
        if old_text.is_empty() {
            return ActionOutcome::failure(FailureClass::InvalidArgument.as_str());
        }

        let anchor = match self.write_anchor(agent, operation.run()) {
            Ok(anchor) => anchor,
            Err(class) => return ActionOutcome::failure(class),
        };
        let target = match Self::write_target(anchor, relative) {
            Ok(target) => target,
            // 路径形状本身不合法：那是给这个动词的参数不对，属于**明确失败**，与读无关。
            Err(class @ FailureClass::InvalidPath) => {
                return ActionOutcome::failure(class.as_str());
            }
            // 其余一律是"读不出来"：不存在、链接 / reparse point、权限、I/O 失败。
            // 复核只能如实报**无法确认**——"看不到那份文件"既不能断言成功、也不能断言失败。
            Err(_) => return ActionOutcome::unknown(),
        };

        let current = match read_text_file(&target) {
            Ok(text) => text,
            // 读不出来就是读不出来：不是普通文件、超限、非 UTF-8、读失败一律同一句。
            Err(_) => return ActionOutcome::unknown(),
        };

        if current.contains(old_text) {
            return ActionOutcome::failure(FailureClass::NotFound.as_str());
        }
        if !current.contains(new_text) {
            return ActionOutcome::unknown();
        }

        ActionOutcome::success(vec![ContentBlock::new(
            "verified",
            format!(
                "{} ({} bytes, replacement in place)",
                relative,
                current.len()
            ),
        )])
    }
}

/// **整份读一个普通文本文件**——"整份读"这件事的**唯一实现**。
///
/// 三个调用面共用它：`files_read` 不带窗口参数那一支、编辑/编辑复核的前置读取，以及只读文件
/// 视图里那些**保留单文件地址**（派生手册，见 [`crate::file_view`]——它背后是环境根下的一份
/// 文件，读取规则必须与资料面**逐条相同**，否则同一个动词在两个地址上会有两套脾气）。
///
/// 规则按顺序：
///
/// 1. **先判类型再打开**：在 Windows 上把目录当文件打开会得到 `PermissionDenied`，那会把
///    "这是目录"错报成"权限被拒"，所以先确定地判成 `WrongKind`；
/// 2. 只打开一次，之后的类型判断与读取都在这个句柄上做：缩小"检查"与"使用"之间的窗口；
/// 3. **有界读取**：超过 [`MAX_TEXT_BYTES`] ⇒ `too-large`，**不截断**（宁可整份不给，也不给半份）；
///    即使文件在上一次判断之后变大，最多也只可能读到上限 + 1 字节；
/// 4. 最后才按 UTF-8 解码（失败 ⇒ `not-utf8`）。
pub(crate) fn read_text_file(path: &Path) -> Result<String, FailureClass> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => return Err(FailureClass::WrongKind),
        Ok(_) => {}
        Err(error) => return Err(io_class(&error)),
    }

    let mut file = File::open(path).map_err(|error| io_class(&error))?;
    let metadata = file.metadata().map_err(|error| io_class(&error))?;
    if !metadata.is_file() {
        return Err(FailureClass::WrongKind);
    }
    if metadata.len() > MAX_TEXT_BYTES {
        return Err(FailureClass::TooLarge);
    }

    let mut bytes = Vec::new();
    let read = file
        .by_ref()
        .take(MAX_TEXT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_class(&error))?;
    if read as u64 > MAX_TEXT_BYTES {
        return Err(FailureClass::TooLarge);
    }

    String::from_utf8(bytes).map_err(|_| FailureClass::NotUtf8)
}

/// 把独占创建的失败翻译成稳定分类。
///
/// `AlreadyExists` 是**明确失败**：本次创建没有发生，而且原名下的内容一个字节都没被动过。
fn create_failure(error: &std::io::Error) -> FailureClass {
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        FailureClass::AlreadyExists
    } else {
        io_class(error)
    }
}

/// 把一个**沙盒相对路径**拆成普通名字组件（第一遍：只看形状，不碰文件系统）。
///
/// 空串返回空组件表（调用方自行决定是否允许，空串表示沙盒根）。拒绝绝对路径、`\` 或 `/`
/// 开头、UNC、设备路径、任何含 `:` 的写法（含 Windows ADS）、`.`、`..`、根与盘符前缀。
///
/// 视图层（`file_view`）复用同一条形状规则处理派生逻辑地址：派生目录不是真实目录，
/// 但它接受的路径形状**不允许比真实目录更宽松**。
pub(crate) fn relative_names(relative: &str) -> Result<Vec<String>, FailureClass> {
    // 规则的家在 `crate::sandbox`（`files` / `file_view` / `im` 共用同一份实现，而 **App 之间
    // 不得有实现依赖**）；这里只把它的分类翻成本 App 的词表。
    crate::sandbox::relative_names(relative).map_err(FailureClass::from)
}

/// **第二遍**：在一个沙盒根下逐段落实一个已经通过形状校验的相对路径。
///
/// 空组件表返回根本身（`files_list` 的沙盒根就是它的合法目标）。规则的家同样在
/// [`crate::sandbox`]，这里只把分类翻成本 App 的词表。
///
/// ⚠️ 与 [`FileApp::resolve_existing`] 同样的等级声明：只做"检查"，真正的读发生在之后，
/// 检查与打开之间存在理论上的替换窗口，本版把沙盒限定为**受信目录**（不构成 OS 级安全沙盒）。
pub(crate) fn resolve_within(root: &Path, names: &[String]) -> Result<PathBuf, FailureClass> {
    crate::sandbox::resolve_within(root, names).map_err(FailureClass::from)
}

/// 在**写面目录**（宿主授权的那一个子目录，或主体自己的作品目录）里解析一个相对路径。
///
/// 与 [`resolve_within`] 是同一套规则、同一份实现，只是基准来自"这一次装配的写面"而不是资料根：
/// 写面说的就是"这**一个**目录你可以写"，换成资料根会让改写范围比创建宽出一大截。
///
/// 空路径在这里是非法值——改写要的是一份**文件**，那个目录本身不是。
pub(crate) fn resolve_in_base(base: &Path, relative: &str) -> Result<PathBuf, FailureClass> {
    let names = relative_names(relative)?;
    if names.is_empty() {
        return Err(FailureClass::InvalidPath);
    }
    resolve_within(base, &names)
}

/// **机器档的第二遍**：逐段落实一条绝对路径（末段也必须存在）。规则的家在 [`crate::sandbox`]。
pub(crate) fn resolve_machine(target: &MachinePath) -> Result<PathBuf, FailureClass> {
    crate::sandbox::resolve_machine(target).map_err(FailureClass::from)
}

/// **机器档的"新建"用**：父目录逐段落实（存在、是目录、链上无链接），末段原样交回。
/// 规则的家同上；这里只把分类翻成本 App 的词表。
pub(crate) fn resolve_machine_parent(
    target: &MachinePath,
) -> Result<(PathBuf, String), FailureClass> {
    crate::sandbox::resolve_machine_parent(target).map_err(FailureClass::from)
}

/// 把一份**已经按名称排序的全量条目**取成一页并渲染（`files_list` 的唯一渲染形状）。
///
/// `after` 是上一次返回过的**名字**：只保留严格大于它的那些；它不是这份列表里的名字时
/// **明确失败**（`invalid-argument`），不静默当成"从头开始"。本批之后还有条目时，首行的
/// `more` 给出下次该用的名字——那是**搬运一个名字**，不是替主体算；到底时明确写 `more = none`、
/// 不给 `after`。**续列不重不漏**：因此（目录不变时）一页页取出来的并集 == 全量列表。
///
/// 首行形状与从前同源（`entries = N, truncated = B`），只在**后面追加** `more` 读数。
pub(crate) fn render_listing_page(
    items: &[DirectoryEntry],
    after: Option<&str>,
) -> Result<String, FailureClass> {
    let start = match after {
        None => 0,
        Some(name) => match items.iter().position(|(existing, _)| existing == name) {
            Some(index) => index + 1,
            None => return Err(FailureClass::InvalidArgument),
        },
    };

    let remaining = &items[start..];
    let truncated = remaining.len() > MAX_ENTRIES;
    let page = &remaining[..remaining.len().min(MAX_ENTRIES)];
    let more = match (truncated, page.last()) {
        (true, Some((name, _))) => format!("after {name}"),
        // 没有被截断就是**没有更多**：明确写出来，不靠"没有字段"暗示。
        _ => "none".to_string(),
    };

    let mut text = format!(
        "entries = {}, truncated = {}, more = {}\n",
        page.len(),
        truncated,
        more
    );
    for (name, kind) in page {
        text.push_str(name);
        text.push('\t');
        text.push_str(kind);
        text.push('\n');
    }
    Ok(text)
}

/// 续读的机械读数行：本段覆盖的字节区间、全文总字节数、还有没有更多。
///
/// `more` 是 `none`（到底了）或 `next <字节>`（下次从这里接着读）——同样只是搬运。
fn window_readings(start: u64, end: u64, total: u64) -> String {
    let more = match end < total {
        true => format!("next {end}"),
        false => "none".to_string(),
    };
    format!("bytes = {start}..{end}, total = {total}, more = {more}")
}

/// 一个直接子项的名字、绝对路径与种类。
struct Child {
    name: String,
    path: PathBuf,
    kind: ChildKind,
}

/// 子项在遍历时的三类归属：目录、普通文件、链接 / reparse point。
enum ChildKind {
    Dir,
    File,
    Link,
}

/// 一个目录的直接子项，按名称排序（与文件系统返回顺序无关）。
///
/// 形状用 `symlink_metadata` 读（**不跟随**）：链接、junction 与任何 reparse point 都归
/// [`ChildKind::Link`]。遍历查找时**既不跟进也不当成材料**——否则一个指向资料根之外的
/// junction 就能让查找越过那条边界。这一条与 [`FileApp::resolve_existing`] 的逐段检查同源：
/// 只是那里检查的是**请求路径**的每一段，这里检查的是**遍历发现的**每一项。
fn sorted_children(directory: &Path) -> Result<Vec<Child>, FailureClass> {
    let entries = fs::read_dir(directory).map_err(|error| io_class(&error))?;
    let mut children: Vec<Child> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| io_class(&error))?;
        let path = entry.path();
        let kind = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() || is_reparse_point(&metadata) => {
                ChildKind::Link
            }
            Ok(metadata) if metadata.is_dir() => ChildKind::Dir,
            Ok(metadata) if metadata.is_file() => ChildKind::File,
            // 形状读不出来（权限、路径过长），或既不是普通文件也不是目录：不作为材料，也不跟进。
            Ok(_) | Err(_) => ChildKind::Link,
        };
        children.push(Child {
            name: entry.file_name().to_string_lossy().into_owned(),
            path,
            kind,
        });
    }
    children.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(children)
}

/// 一次查找的机械计数：看过几个文件、有几个没能看正文、有没有触到上限。
#[derive(Default)]
pub(crate) struct ScanState {
    pub(crate) scanned: usize,
    pub(crate) skipped: usize,
    pub(crate) truncated: bool,
}

/// 一条候选：路径 + 命中位置 + 该行片段。
pub(crate) struct FoundMatch {
    pub(crate) path: String,
    pub(crate) location: String,
    pub(crate) snippet: String,
}

/// 看一条材料：**先名字、后正文**，同一条材料最多产出一条候选。
///
/// 名字命中就不再读正文（"最多出现一次"取的是第一处命中）。正文命中的是**第一处**含
/// `pattern` 的行，给出 1 起算的行号与该行片段。
///
/// 看不了正文的文件（超过单次读取上限、不是 UTF-8、读失败）**计入 `skipped`**，而不是悄悄
/// 当成"没命中"——"没找到"与"没有看"必须能分开。
fn classify(
    relative: &str,
    name: &str,
    path: &Path,
    pattern: &str,
    scan: &mut ScanState,
) -> Option<FoundMatch> {
    if name.contains(pattern) {
        return Some(FoundMatch {
            path: relative.to_string(),
            location: "name".to_string(),
            snippet: snippet_of(name),
        });
    }

    let Ok(mut file) = File::open(path) else {
        scan.skipped += 1;
        return None;
    };
    let mut bytes = Vec::new();
    if file
        .by_ref()
        .take(MAX_TEXT_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        scan.skipped += 1;
        return None;
    }
    if bytes.len() as u64 > MAX_TEXT_BYTES {
        scan.skipped += 1;
        return None;
    }
    let Ok(text) = String::from_utf8(bytes) else {
        scan.skipped += 1;
        return None;
    };

    for (index, line) in text.lines().enumerate() {
        if line.contains(pattern) {
            return Some(FoundMatch {
                path: relative.to_string(),
                location: format!("line={}", index + 1),
                snippet: snippet_of(line),
            });
        }
    }
    None
}

/// 在一份**已经拿到的正文**里找第一处命中（两个查找面共用同一条判据）。
///
/// 资料面把文件读成正文后调它，经历面把一段渲染好的经历交给它——因此"命中哪一行、片段怎么裁"
/// 在两个面上逐字节同形，不会各长一套脾气。
pub(crate) fn match_in_text(relative: &str, text: &str, pattern: &str) -> Option<FoundMatch> {
    for (index, line) in text.lines().enumerate() {
        if line.contains(pattern) {
            return Some(FoundMatch {
                path: relative.to_string(),
                location: format!("line={}", index + 1),
                snippet: snippet_of(line),
            });
        }
    }
    None
}

/// 查找的参数面（两个查找面共用）：参数名封闭、`pattern` 必填、空串拒绝。
///
/// **空串匹配一切**：那不是查找，是"把所有东西都倒出来"，明确拒绝（不是"匹配全部"）。
pub(crate) fn search_pattern(params: &ActionParams) -> Result<&str, FailureClass> {
    for (name, _) in params.entries() {
        if name != PARAM_PATTERN && name != PARAM_PATH {
            return Err(FailureClass::UnknownParameter);
        }
    }
    let Some(pattern) = params.get(PARAM_PATTERN) else {
        return Err(FailureClass::MissingParameter);
    };
    if pattern.is_empty() {
        return Err(FailureClass::InvalidArgument);
    }
    Ok(pattern)
}

/// 一次查找的读数 ＋ 候选行（两个查找面共用同一条渲染）。
///
/// 首行的四个计数是**可判错的读数**：`scanned` 说看了几个，`skipped` 说有几个没能看正文
/// （"没找到"与"没有看"必须分得开），`truncated` 说有没有触到上限——**不把"扫到一半"说成
/// "只有这些"**。
pub(crate) fn render_matches(found: &[FoundMatch], scan: &ScanState) -> String {
    let truncated = scan.truncated || found.len() > MAX_ENTRIES;
    let shown = found.len().min(MAX_ENTRIES);
    let mut text = format!(
        "candidates = {}, scanned = {}, skipped = {}, truncated = {}\n",
        shown, scan.scanned, scan.skipped, truncated
    );
    for found_match in found.iter().take(MAX_ENTRIES) {
        text.push_str(&found_match.path);
        text.push('\t');
        text.push_str(&found_match.location);
        text.push('\t');
        text.push_str(&found_match.snippet);
        text.push('\n');
    }
    text
}

/// 一段**显示用**片段：控制字符（含制表符）按空格呈现，超长按字符边界截到 120 字节以内并标 `…`。
///
/// 它是显示截断，不是丢数据：正文仍在盘上，要完整内容就 `files_read`。
fn snippet_of(line: &str) -> String {
    let normalized: String = line
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    if normalized.len() <= MAX_SNIPPET_BYTES {
        return normalized;
    }
    // 截断标记也算在上限内：`…` 占 3 字节，因此最多抄到 117 字节处。
    let budget = MAX_SNIPPET_BYTES - '…'.len_utf8();
    let mut out = String::new();
    for ch in normalized.chars() {
        if out.len() + ch.len_utf8() > budget {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

impl ActionPort for FileApp {
    /// 单独使用时，本 App 就是唯一的那个世界：读侧三项，加上有授权时的创建能力。
    ///
    /// 装配进最小 OS 之后，能力的可用性由 OS 决定（未展开之前这里的能力不该被使用）。
    fn capabilities(&self, _agent: &AgentId, _run: &RunId) -> Capabilities {
        self.available_capabilities()
    }

    /// 局部准入：读三个动作免门；两个写动作与它们各自的**复核**都要有**写面**。
    ///
    /// 写面有两种（见 [`WritableFace`]）：授权的那个要**对得上这次身份与活动**；主体自己的作品面
    /// 不绑这两样（主体自己的东西不用申请），只要这一格在这一次装配里在场就放行。
    fn admit(&self, agent: &AgentId, run: &RunId, action: &Action) -> ActionAdmission {
        match action.id().as_str() {
            CAPABILITY_LIST | CAPABILITY_READ_TEXT | CAPABILITY_FIND_TEXT => {
                ActionAdmission::Admitted
            }
            CAPABILITY_CREATE_TEXT
            | CAPABILITY_VERIFY_TEXT
            | CAPABILITY_EDIT
            | CAPABILITY_VERIFY_EDIT => match &self.writable {
                None => ActionAdmission::refused(REFUSAL_CREATION_NOT_GRANTED),
                Some(WritableFace::Own) => ActionAdmission::Admitted,
                // 机器档是**支级**的整档授权（主人给的），不绑这一次活动。
                Some(WritableFace::Machine) => ActionAdmission::Admitted,
                Some(WritableFace::Granted(grant))
                    if grant.agent != *agent || grant.run != *run =>
                {
                    ActionAdmission::refused(REFUSAL_CREATION_GRANT_MISMATCH)
                }
                Some(WritableFace::Granted(_)) => ActionAdmission::Admitted,
            },
            _ => ActionAdmission::refused(FailureClass::UnknownCapability.as_str()),
        }
    }

    /// 执行一次行动。
    ///
    /// 创建动作与它的复核、改文本动作与它的复核**需要**主体身份与操作身份：授权绑定的是
    /// "这个 Agent 的这一次活动"，执行前必须再对一次（见 [`FileApp::create_text`] /
    /// [`FileApp::verify_text`] / [`FileApp::edit_text`] / [`FileApp::verify_edit_text`]）。
    /// 读侧三个动作不需要它们——没有写效应，也就没有需要跨重启对账的副作用。
    fn execute(
        &mut self,
        agent: &AgentId,
        operation: &OperationId,
        action: &Action,
    ) -> ActionOutcome {
        match action.id().as_str() {
            CAPABILITY_LIST => self.list(action.params()),
            CAPABILITY_READ_TEXT => self.read_text(action.params()),
            CAPABILITY_FIND_TEXT => self.find_text(action.params()),
            CAPABILITY_CREATE_TEXT => self.create_text(agent, operation, action.params()),
            CAPABILITY_VERIFY_TEXT => self.verify_text(agent, operation, action.params()),
            CAPABILITY_EDIT => self.edit_text(agent, operation, action.params()),
            CAPABILITY_VERIFY_EDIT => self.verify_edit_text(agent, operation, action.params()),
            _ => ActionOutcome::failure(FailureClass::UnknownCapability.as_str()),
        }
    }
}

/// 取出 `path`，同时允许这**一个能力**自己接受的额外参数名。
///
/// 未知参数名与缺失必需参数都明确失败；不做"忽略多余参数"的宽容处理。额外参数名由调用能力
/// 显式列出（`files_list` 的 `after`、`files_read` 的 `offset` / `limit`），因此参数面仍然是
/// 封闭的：不属于这个动词的名字一律 `unknown-parameter`。
pub(crate) fn path_parameter_with(
    params: &ActionParams,
    required: bool,
    extra: &[&str],
) -> Result<Option<String>, FailureClass> {
    for (name, _) in params.entries() {
        if name != PARAM_PATH && !extra.contains(&name.as_str()) {
            return Err(FailureClass::UnknownParameter);
        }
    }
    match params.get(PARAM_PATH) {
        Some(value) => Ok(Some(value.to_string())),
        None if required => Err(FailureClass::MissingParameter),
        None => Ok(None),
    }
}

/// 取出一个**可选**的字节数参数：十进制非负整数。
///
/// 没给就是 `None`；给了但不是十进制非负整数（含空串、负号、`+` 号、空白、任何非数字字符）
/// 都是 `invalid-argument`——不做"尽力解析"，也不静默回到默认值。
pub(crate) fn byte_parameter(
    params: &ActionParams,
    name: &str,
) -> Result<Option<u64>, FailureClass> {
    match params.get(name) {
        None => Ok(None),
        Some(value) if is_byte_count(value) => value
            .parse()
            .map(Some)
            // 全是数字但超出 `u64`：如实拒绝，不折成别的数。
            .map_err(|_| FailureClass::InvalidArgument),
        Some(_) => Err(FailureClass::InvalidArgument),
    }
}

/// 一个字节数参数的形状：非空，而且全是 ASCII 数字。
///
/// 因此负号、`+` 号、空白、十六进制前缀、小数点一律**不是**字节数——不做"尽力解析"，
/// 也不静默回到默认值。
fn is_byte_count(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// 是否是符号链接 / junction / 其他 reparse point。
///
/// Windows 上 junction 也是 reparse point；只看 `is_symlink()` 不足以覆盖全部情况，
/// 因此这里直接看 `FILE_ATTRIBUTE_REPARSE_POINT`。
fn is_reparse_point(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

/// I/O 错误 → 稳定分类。
pub(crate) fn io_class(error: &std::io::Error) -> FailureClass {
    match error.kind() {
        std::io::ErrorKind::NotFound => FailureClass::NotFound,
        std::io::ErrorKind::PermissionDenied => FailureClass::Denied,
        std::io::ErrorKind::InvalidData => FailureClass::NotUtf8,
        _ => FailureClass::Io,
    }
}
