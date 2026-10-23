//! 文件视图：**一个 App、一个逻辑命名空间**。资料面与三个派生地址共用 `files_*`（默认只读，
//! 宿主给出创建授权时才多出写）；**主体自己的作品面**是另一格，动词名分开（`projects_*`）——
//! 那一格带写，而且**不需要任何授权**。
//!
//! - **授权资料目录**：调用方显式给出的真实目录，用通用 `files_list` / `files_read` 访问；
//! - **派生经历目录**：保留逻辑地址 `history/`，把**这个身份环境**里已确认可读的经历前缀
//!   按活动机械地呈现为只读逻辑文件，仍然只用同一套 `files_list` / `files_read` 访问；
//! - **派生单文件地址**：保留逻辑地址 `manual.md`，背后是**环境根下那一份手册**——主体在自己
//!   地盘上的文件（可删可改），在这里只读地可读；那一份不在环境根里时这个地址**缺席**。
//! - **派生单文件地址（第二例）**：保留逻辑地址 `self_concept`，背后是**主体自己写下的那一篇自识**
//!   （2026-10-13 加：在那之前 `write_self_concept` 是全库唯一一个**写完读不回来**的自持物）；
//! - **主体自己的作品面**：`<环境根>/projects`。读两件（`projects_list` / `projects_read`）、写两件
//!   （`projects_create` / `projects_edit`）＋ 各自**同参复核**——**不需要任何授权，也不需要任何
//!   声明**：有环境根的装配里这一格都在场，动词跟着在（装配期缺席才建那个目录）。
//!   ⚠ 归属与门在别处写死：它**不是**资料面的第四个地址（名字自答，见 [`WORKS_DIR`]），两种写面
//!   为什么各走各的门见 `file_app` 那两种写面——本视图不另立判据。
//!
//! 为什么经历是**派生视图**而不是真实文件：环境的内部记录（身份记录、两份追加存储、写锁、
//! 暂停记录）是**环境自己**的事实，不是 Agent 的普通资料。这里只读经历记录、按段机械渲染，
//! 既不复制一份耐久正文权威、不改写原文件，也不把内部文件名暴露成可访问路径。
//! **派生文件不等于物理历史文件**：本视图没有创造逐活动的物理文件，真实文件与其管理能力
//! 不在本刀范围内。
//!
//! 逻辑地址（稳定、与物理布局无关）：
//!
//! ```text
//! <资料目录里的普通文件与子目录>      ← 真实资料（默认只读）
//! history/                            ← 派生经历目录：每个活动一个逻辑子目录
//! history/<活动>/                     ← 一个活动的分段目录
//! history/<活动>/000.txt              ← 一段经历：机械事实行 + 按序号排列的记录
//! manual.md                           ← 派生单文件地址：环境根下那一份手册（环境根里有才在）
//! self_concept                        ← 派生单文件地址：主体自己写下的自识（写过了才有）
//! projects/                           ← 主体自己的作品目录：本视图唯一属于主体的写面
//! ```
//!
//! **三个派生地址是同一套模式的实例**，区别只在"背后是什么"：`history/` 背后是**一组记录**
//! （按活动与段机械切分），`manual.md` / `self_concept` 背后各是**一份文件**（整份读，没有"某一段"这回事）。
//! 它们共享同一件事：地址**不是真实路径**，存在与否由**装配那一刻的事实**定（给了环境根 ⇒ 经历
//! 地址在；环境根下真有那一份 ⇒ 那个单文件地址在），列与读都走同一套通用动词，从外面看不出"这个
//! 是派生的"。派生**不等于**环境内部记录可读：环境根永远不被当作普通文件根，只有白名单里的这几个
//! 地址有派生投影。**作品面不是派生地址**：它是环境根里名副其实的一个目录，只是走自己的动词名
//! （`projects_*`），因此不在这条白名单里——派生那一套"地址不是真实路径"的话不适用于它。
//!
//! ⚠ **2026-10-13 记账**：单文件那一路现在是**两份同形实现**（`ManualView` / `SelfConceptView`）。第三份
//! 出现时就该收成一张表（地址 → 环境根下的文件名），**不提前收**：两份还不值得一层抽象。
//!
//! 分段是唯一的分页方式：每段最多 [`RECORDS_PER_SEGMENT`] 条记录，并且不会因为字节上限
//! 而被切开某一条记录——读不全就明确报 `too-large`，**不静默截断、不摘要、不重排**。
//! 活动名是按活动标识机械映射出来的安全名字（非语义排序、非摘要），未知名字不会绕过任何检查。
//!
//! 边界（与"不替主体判断"直接相关）：
//!
//! - **资料面与三个派生地址不给写入口**：派生那三个永不可写；资料面的写只来自宿主显式给出的
//!   创建授权，那两件归 `files_*`；
//! - **唯一的写面是主体自己的作品面**：一样是独占新建 / 唯一替换 / 不改名不删不追加 / 64 KiB，
//!   差别只在**归属**（见 `file_app` 那两种写面）。它给的是写与读、**没有执行**——要跑这些文本
//!   得有 `terminal` 档（缺省没有这一档）；那是能力事实，不是建议；
//! - 经历正文只来自**已确认可读前缀**，读错误、未知活动、未知路径、超限与编码失败都是明确
//!   失败，绝不用空目录或空内容冒充"没有内容"；
//! - 查看经历形成**本次 Run 的一次观察**：它不重开旧活动、不重放旧动作、不改写已有经历；
//! - 资格判断不在这里：本 App 不认识 OS，展开状态与身份绑定由 OS 的作用域负责。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Capabilities,
    Capability, ContentBlock, ParamSpec,
};
use crate::creation_grant::CreationGrant;
use crate::file_app::{
    self, FILE_APP_ID, FailureClass, FileApp, MAX_TEXT_BYTES, PARAM_AFTER, PARAM_LIMIT, PARAM_NAME,
    PARAM_NEW_TEXT, PARAM_OFFSET, PARAM_OLD_TEXT, PARAM_PATH, PARAM_PATTERN, PARAM_TEXT,
};
use crate::foundation::{AgentId, OperationId, RunId};
use crate::store::{RunSummary, TraceRecord};

/// 派生经历目录保留的逻辑名。
///
/// 它是**逻辑地址**，不是真实目录：资料目录里同名条目会让装配被拒绝（见
/// [`FileViewFailure::ReservedAddressTaken`]），而不是被静默遮蔽。
pub const HISTORY_DIR: &str = "history";

/// 派生手册保留的逻辑地址（**单文件**，不是目录）。
///
/// 它与 [`HISTORY_DIR`] 同族：是**逻辑地址**，不是资料目录里的文件——资料目录里同名条目会让
/// 装配被拒绝（见 [`FileViewFailure::ReservedAddressTaken`]），而不是被静默遮蔽。
///
/// 名字**不在这里另写一遍**：它就是环境根下那一份的文件名（[`crate::orientation::MANUAL_FILE`]
/// ——主体是靠只读视野认这份东西的，两处各写一个名字迟早分叉）。
pub const MANUAL_ADDRESS: &str = crate::orientation::MANUAL_FILE;

/// 派生**自识**保留的逻辑地址（**单文件**，不是目录）。
///
/// 与 [`MANUAL_ADDRESS`] 同一套模式、同一个理由，而它是**主体自己那一份**（手册是系统给主体读的，
/// 自识是主体写给自己的）。地址是**逻辑地址**，不是资料目录里的文件——资料目录里同名条目会让装配
/// 被拒绝（见 [`FileViewFailure::ReservedAddressTaken`]），而不是被静默遮蔽。
///
/// ⚠ **2026-10-13 加**：它补的是主体自己列的第②条缺陷——「self_concept 写完不能核实」。在那之前
/// `write_self_concept` 是全库**唯一一个写完读不回来的自持物**（笔记有 `notes_read`、格位有 `pins_board`、
/// 约的时刻有 `timer_list`，而自识只有"下一场开场再读"）。⇒ 给它一条**现读**的路（手册那条路的
/// 同款：不缓存、每次现读 ⇒ 开场时地址已在，读到的是最新一份；开场时还没有，写下的那一份
/// 要下一次开场才出现）。
///
/// 名字不在这里另写一遍：它就是环境根下那一份的文件名（[`crate::self_concept::SELF_CONCEPT_FILE`]）。
pub const SELF_CONCEPT_ADDRESS: &str = crate::self_concept::SELF_CONCEPT_FILE;

/// 一段派生经历最多包含多少条记录。
pub const RECORDS_PER_SEGMENT: usize = 50;

/// 列目录能力的标识（与真实文件 App 同一个通用动作）。
const CAPABILITY_LIST: &str = "files_list";
/// 读文本能力的标识（与真实文件 App 同一个通用动作）。
const CAPABILITY_READ_TEXT: &str = "files_read";
/// 按子串查找能力的标识（与真实文件 App 同一个通用动作）。
const CAPABILITY_FIND_TEXT: &str = "files_grep";

/// **主体的作品目录**的逻辑名：`<环境根>/projects`。
///
/// 与 `history` / 手册 / 自识那三个保留地址不同：那三个是**别人的或派生的**，这一格是**主体自己的**
/// ——所以它有自己的动词名（`projects_*`），而不是资料面里的第四个地址（归属由名字自答）。
pub const WORKS_DIR: &str = "projects";

/// 作品面的六件动词。
///
/// 前两件是读（主体写完要能读回来核实），后四件是写与它们各自的**同参复核**
/// （装配期自洽检查要求：写类别必须配复核）。
const CAPABILITY_WORKS_LIST: &str = "projects_list";
const CAPABILITY_WORKS_READ: &str = "projects_read";
const CAPABILITY_WORKS_CREATE: &str = "projects_create";
const CAPABILITY_WORKS_VERIFY_TEXT: &str = "projects_verify_text";
const CAPABILITY_WORKS_EDIT: &str = "projects_edit";
const CAPABILITY_WORKS_VERIFY_EDIT: &str = "projects_verify_edit";

/// 装配只读文件视图被拒绝的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileViewFailure {
    /// 授权资料目录本身不可用（不存在 / 不是目录 / 是链接 / 是文件系统根）。
    Materials(FailureClass),
    /// 资料目录与环境目录重叠（相等或互相包含）：那会把这个身份环境的内部记录当成
    /// 普通资料直接暴露给本轮模型，因此拒绝装配。
    MaterialsExposeEnvironment,
    /// 资料目录里已经有与**现存的**保留逻辑地址同名的真实条目（`history` / 手册）：
    /// 那个地址无法被唯一解释。
    ReservedAddressTaken,
    /// 创建授权目录不可用（不存在 / 不是目录 / 是链接或 reparse point / 没有权限）。
    CreationTargetUnusable(FailureClass),
    /// 创建授权目录不在资料根**之内**：等于资料根本身（那等于整个资料根可写）或越出它。
    CreationTargetOutsideMaterials,
}

impl FileViewFailure {
    /// 稳定分类标记。
    pub fn as_str(self) -> &'static str {
        match self {
            FileViewFailure::Materials(class) => class.as_str(),
            FileViewFailure::MaterialsExposeEnvironment => "materials-expose-environment",
            FileViewFailure::ReservedAddressTaken => "reserved-address-taken",
            FileViewFailure::CreationTargetUnusable(_) => "creation-target-unusable",
            FileViewFailure::CreationTargetOutsideMaterials => "creation-target-outside-materials",
        }
    }
}

/// 只读文件视图：真实资料目录 + 可选的派生经历目录 + 可选的派生手册。
#[derive(Debug)]
pub struct FileView {
    materials: FileApp,
    /// **这一格是机器档吗**（`files=full`）。
    ///
    /// 机器档与家档共用同一个视图：差别只有一条 —— `materials` 那一面换成了
    /// `FileApp::machine_with_writes()`（整台机器），于是**绝对路径**整条交给它，
    /// 而三个保留地址（`history` / 手册 / 自识）与作品面**一个字都不变**
    /// （它们不是资料面的地址，是派生的与主体自己的东西）。
    machine: bool,
    history: Option<HistoryView>,
    /// 派生手册：环境根下那一份（只读）。`None` = 环境根里**没有**那一份 ⇒ 地址**缺席**
    /// （不是错误，也不是"读不到"）。
    manual: Option<ManualView>,
    /// 派生**自识**：环境根下那一份（只读）。`None` = 环境根里**没有**那一份 ⇒ 地址**缺席**
    /// （不是错误，也不是"读不到"——主体还没写过，就是没有）。
    self_concept: Option<SelfConceptView>,
    /// **主体自己的作品面**（`<环境根>/projects`）：读 ＋ 写，**不需要任何授权**
    /// （判据：「自己的东西不用申请，别处的要」，与 `notes_*` 同族）。
    ///
    /// ⚠ 它**不是**资料面的一个地址：那三个保留地址（`history` / 手册 / 自识）都是**别人的或
    /// 派生的**，而这一格是**主体自己的**，连动词名都分开（`projects_*`）——归属由名字自答。
    works: WorksFace,
}

/// 主体的作品面：三态，不许压成两态。
#[derive(Debug)]
enum WorksFace {
    /// 在场：`<环境根>/projects` 是一个真目录，读与写都归它。
    Ready(FileApp),
    /// **这一格不在场**：这次装配没有环境根（`materials_only`）——主体那几件动词连声明都不出现。
    Absent,
    /// **在场但打不开**（那个名字底下是链接 / 不是目录 / I/O 失败）：如实记着那个分类，
    /// 那几件动词按它失败——**不冒充"这个地址不在"**。
    ///
    /// ⚠ 它**不拒绝启动**：作品面是主体自己的东西，主体有 shell 的时候完全可能把它弄成一个链接；
    /// 那不该让整个家起不来（"读不出来 ≠ 没有"，也不该升级成"全家停摆"）。
    Unusable(FailureClass),
}

/// 派生经历目录的来源：**环境目录**（只读）。
#[derive(Debug)]
struct HistoryView {
    root: PathBuf,
}

/// 派生手册的来源：**环境根下那一份文件**（只读）。
///
/// 与经历同一条纪律：它不复制正文、不缓存内容——每次 `files_read` 都现读那一份，因此主体改了自己
/// 那一份，下一次读到的就是新的（这一点与经历不同：经历是已确认记录的机械投影，不会变）。
#[derive(Debug)]
struct ManualView {
    path: PathBuf,
}

/// 装配期拍一次：**环境根里有没有**那一份手册（`<环境根>/manual.md`）。
///
/// 判据是 `symlink_metadata`（**不跟随链接**）：地址在不在是**名字**问题，不是"能不能读通"
/// 问题——读不通的那几种（权限、坏数据、那个名字底下不是普通文件）在读取那一刻如实报出来，
/// 绝不在这里压成"环境根里没有"。
fn manual_view(environment_root: &Path) -> Option<ManualView> {
    let path = environment_root.join(MANUAL_ADDRESS);
    match fs::symlink_metadata(&path) {
        Ok(_) => Some(ManualView { path }),
        Err(_) => None,
    }
}

/// 派生自识的来源：**环境根下那一份文件**（只读）。
///
/// 与手册同一条纪律：不复制正文、不缓存——地址在开场时已存在 ⇒ 每次 `files_read` 都现读那一份；
/// 开场时还没有 ⇒ 这个地址本段里不存在，刚写下的自识要下一次开场才出现。
#[derive(Debug)]
struct SelfConceptView {
    path: PathBuf,
}

/// 装配期拍一次：**环境根里有没有**那一份自识（`<环境根>/self_concept`）。
///
/// 判据与手册同款（`symlink_metadata`，**不跟随链接**）：在不在是**名字**问题，读不通的那几种
/// （权限、坏数据、那个名字底下不是普通文件）在读取那一刻如实报出来，绝不在这里压成"主体没有自识"。
fn self_concept_view(environment_root: &Path) -> Option<SelfConceptView> {
    let path = environment_root.join(SELF_CONCEPT_ADDRESS);
    match fs::symlink_metadata(&path) {
        Ok(_) => Some(SelfConceptView { path }),
        Err(_) => None,
    }
}

/// 装配主体的作品面：`<环境根>/projects`。
///
/// **缺席才建**（与 `agent/README.md` §3.104 的 `place()` 同一条纪律：绝不覆盖、绝不动已有的；
/// 目录是**空的**，不是内容——"给门牌"不等于"替主体决定写什么"）。
///
/// ⚠ **为什么是装配期建、而不是"主体自己建"**（对施工单 §7 的更正）：缺省不再给主体 shell 之后，
/// "主体自己建"物理上不可能——主体拿什么建？⇒ 第一份作品的落点由**这一次装配**铺好，之后主体
/// 写什么、删什么、怎么改都随主体（主体删掉之后下一次装配会补回一个空目录：那是**安装**的动作）。
///
/// 打不开（同名的是链接 / 文件 / I/O 失败）**不拒绝启动**，如实记成 [`WorksFace::Unusable`]。
fn works_face(environment_root: &Path) -> WorksFace {
    let root = environment_root.join(WORKS_DIR);
    if fs::symlink_metadata(&root).is_err()
        && let Err(error) = fs::create_dir(&root)
    {
        return WorksFace::Unusable(file_app::io_class(&error));
    }
    match FileApp::with_own_works(root) {
        Ok(app) => WorksFace::Ready(app),
        Err(class) => WorksFace::Unusable(class),
    }
}

impl FileView {
    /// 只装配授权资料目录：没有经历目录、也没有手册，逻辑地址里就只有资料本身。
    pub fn materials_only(root: &Path) -> Result<Self, FailureClass> {
        Ok(Self {
            materials: FileApp::open(root)?,
            machine: false,
            history: None,
            manual: None,
            self_concept: None,
            works: WorksFace::Absent,
        })
    }

    /// 装配授权资料目录与派生经历目录。
    ///
    /// 两个根必须**明确分开**：资料目录若与环境目录重叠，模型就能用通用文件动作读到环境的
    /// 内部记录，因此这里在装配期拒绝，而不是靠过滤几个文件名。
    pub fn with_history(
        materials: &Path,
        environment_root: &Path,
    ) -> Result<Self, FileViewFailure> {
        Self::with_history_and_creation(materials, environment_root, None)
    }

    /// 装配授权资料目录、派生经历目录，以及**可选的文本创建授权**。
    ///
    /// 创建授权只描述一件事：在资料根内哪一个**已存在子目录**里可以新建文本。它不改读的规则、
    /// 不改变资料根与环境根必须分开这条要求，也不给整个资料根开写口。目标目录不合法时**拒绝启动**。
    pub fn with_history_and_creation(
        materials: &Path,
        environment_root: &Path,
        creation: Option<&CreationGrant>,
    ) -> Result<Self, FileViewFailure> {
        Self::with_materials_scope(materials, environment_root, creation, false)
    }

    /// **机器档**（`files=full`）：资料面换成"整台机器"，其余照旧。
    ///
    /// 判据（2026-10-20 主人裁）：不给 ⇒ 只能动自己那一块；给了 ⇒ **任意文件夹**。
    /// 因此这一档**不装配资料根**（没有那一个根可装配）、也**不需要 `CreationGrant`**
    /// （那份授权说的是"资料根里哪一个子目录可以写"，机器档没有那个根）；
    /// 三个保留地址与作品面照旧 —— 它们不是资料面的地址。
    pub fn with_machine_scope(
        materials: &Path,
        environment_root: &Path,
        creation: Option<&CreationGrant>,
    ) -> Result<Self, FileViewFailure> {
        Self::with_materials_scope(materials, environment_root, creation, true)
    }

    fn with_materials_scope(
        materials: &Path,
        environment_root: &Path,
        creation: Option<&CreationGrant>,
        machine: bool,
    ) -> Result<Self, FileViewFailure> {
        // 机器档：读面与写面都是整台机器（写不需要授权——这一档本身就是主人的授权）。
        // 资料根与环境根那些"必须分开"的判据在机器档**无从成立**（整台机器当然包含环境根），
        // 所以那一组检查只在门档做；保留地址那一组同理由（没有资料面可被同名条目遮蔽）。
        let materials_app = if machine {
            FileApp::machine_with_writes()
        } else {
            match creation {
                Some(grant) => FileApp::with_creation(materials, grant)
                    .map_err(FileViewFailure::CreationTargetUnusable)?,
                None => FileApp::open(materials).map_err(FileViewFailure::Materials)?,
            }
        };
        if machine {
            let environment_root = fs::canonicalize(environment_root)
                .map_err(|error| FileViewFailure::Materials(file_app::io_class(&error)))?;
            let manual = manual_view(&environment_root);
            let self_concept = self_concept_view(&environment_root);
            let works = works_face(&environment_root);
            return Ok(Self {
                materials: materials_app,
                machine,
                history: Some(HistoryView {
                    root: environment_root,
                }),
                manual,
                self_concept,
                works,
            });
        }
        // 家档才有那一个根（机器档的根由每条绝对路径自己点名）⇒ 这一层装配只认家档；
        // 拿到没有根的实例就是装配期错误，不猜、也不悄悄退化成"没有资料面"。
        let Some(materials_root) = materials_app.home().map(Path::to_path_buf) else {
            return Err(FileViewFailure::Materials(FailureClass::InvalidPath));
        };
        // 规范化后比较：链接与路径别名指向同一目录时按**同一目录**处理。
        let environment_root = fs::canonicalize(environment_root)
            .map_err(|error| FileViewFailure::Materials(file_app::io_class(&error)))?;
        if materials_root.starts_with(&environment_root)
            || environment_root.starts_with(&materials_root)
        {
            return Err(FileViewFailure::MaterialsExposeEnvironment);
        }
        // **保留地址不许被同名真实条目遮蔽**：判据是"这个地址**现在在不在**"（见下），
        // 而不是"这个名字有没有被写过"——不在的地址无从被遮蔽，因此拿它当理由拒绝装配会冤枉
        // 一份完全无关的资料。`history` 的地址由装配本身建立（给了环境根就在）⇒ 它这一条**无条件**。
        if fs::symlink_metadata(materials_root.join(HISTORY_DIR)).is_ok() {
            return Err(FileViewFailure::ReservedAddressTaken);
        }
        // 手册的地址由**环境根里那一份在不在**决定（装配期拍一次，与自识 / 动作 pin / 技能桌面
        // 同族）：在 ⇒ 资料面里同名条目会让这个地址无法被唯一解释，拒绝装配；不在 ⇒ 地址压根
        // 不存在，资料面里有同名文件也无关。
        let manual = manual_view(&environment_root);
        if manual.is_some() && fs::symlink_metadata(materials_root.join(MANUAL_ADDRESS)).is_ok() {
            return Err(FileViewFailure::ReservedAddressTaken);
        }
        // 自识的地址同理（也由"环境根里那一份在不在"决定），只是那一份**主体自己写**。
        let self_concept = self_concept_view(&environment_root);
        if self_concept.is_some()
            && fs::symlink_metadata(materials_root.join(SELF_CONCEPT_ADDRESS)).is_ok()
        {
            return Err(FileViewFailure::ReservedAddressTaken);
        }
        // 作品面：**主体自己的东西**，不需要任何授权，也不需要任何声明——在每一支上都在场。
        let works = works_face(&environment_root);
        // 创建目标必须**严格**在资料根之内：等于根本身或越出它都不是本版给的授权。
        // （它与环境根不重叠这件事，由上面的资料根/环境根比较一并保证。）
        if let Some(grant) = creation {
            let target = materials_root.join(grant.directory());
            match fs::canonicalize(&target) {
                Ok(target) if target != materials_root && target.starts_with(&materials_root) => {}
                Ok(_) => return Err(FileViewFailure::CreationTargetOutsideMaterials),
                Err(error) => {
                    return Err(FileViewFailure::CreationTargetUnusable(file_app::io_class(
                        &error,
                    )));
                }
            }
        }
        Ok(Self {
            materials: materials_app,
            machine,
            history: Some(HistoryView {
                root: environment_root,
            }),
            manual,
            self_concept,
            works,
        })
    }

    /// 这个视图对外声明的三项能力（三项通用只读动作，顺序固定）。
    ///
    /// 说明里给出逻辑地址形状、参数语义、上限与截断行为：`state` 是**存储结构状态**
    /// （是否登记开始、有没有结束事实、有没有封存），不是任务完成判定。
    pub fn capabilities() -> Capabilities {
        Self::capabilities_in(false)
    }

    /// 按档位给的那份说明：机器档里 `path` 可以是**绝对路径**（整台机器），
    /// 相对路径仍然指向保留地址与视图本身 —— 这一句必须跟着档走，否则模型会按错的形状发。
    pub(crate) fn capabilities_in(machine: bool) -> Capabilities {
        let list_path = if machine {
            "视图内的相对目录（留空表示视图根），或这台机器上的绝对目录（如 `D:\\work`）"
        } else {
            "视图内的相对目录；留空表示视图根"
        };
        let read_path = if machine {
            "视图内的相对文件路径，或这台机器上的绝对文件路径（如 `D:\\work\\a.txt`）"
        } else {
            "视图内的相对文件路径"
        };
        Capabilities::of(vec![
            Capability::new(
                ActionId::new(CAPABILITY_LIST),
                "列出一个目录的直接子项（不递归，按名称排序）；接着列用 `after`",
                vec![
                    ParamSpec::new(PARAM_PATH, false, list_path),
                    ParamSpec::new(
                        PARAM_AFTER,
                        false,
                        "上一次返回过的条目名字；只返回严格大于它的下一批",
                    ),
                ],
            )
            .with_details(
                "读数：首行 `more` 是机械读数——`none` = 没有更多，`after <名字>` = 下次用这个名字\
                 接着列。上限：一次最多 100 项。边界：`after` 是名字不是下标；它不属于\
                 这个目录时明确失败（不猜近似名）。`history`（这个身份环境已确认的经历）与\
                 `manual.md`（你家里那份手册）与 `self_concept`（你自己的自识）是保留的逻辑地址，\
                 不是任务完成判定。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_READ_TEXT),
                "读一个 UTF-8 文本文件；整份读，或给 `offset`/`limit` 续读一段",
                vec![
                    ParamSpec::new(PARAM_PATH, true, read_path),
                    ParamSpec::new(PARAM_OFFSET, false, "本段的起始字节（默认 0）"),
                    ParamSpec::new(PARAM_LIMIT, false, "本段的字节数上限（默认 64 KiB）"),
                ],
            )
            .with_details(
                "上限：单次最多 64 KiB，超限拒绝、不截断（宁可让你换一段，也不给你半截）。\
                 续读：`offset` 是起始字节（默认 0，必须落在字符边界上、且不超过文件长度），\
                 `limit` 是本段字节数（默认到 64 KiB 上限，超过就拒绝）。`limit` 是字节数不是行数：\
                 想知道下一段从哪开始，用回执里那行读数。读数：续读时另给一行——本段覆盖的字节区间、\
                 全文总字节数、`more`（`none` = 到底了，`next <字节>` = 下次从这里接着读）。\
                 派生经历：`history/<活动>/<段>.txt` 按段读，段间不重叠、不截断；段首的 `state` 只\
                 说明存储结构（开始/结束事实/封存），不是任务完成判定；经历段自带分段，对它给窗口\
                 参数会明确失败。派生手册：`manual.md` 是一份文件（你家里那份手册的正文），\
                 整份读、没有分段，对它给窗口参数同样明确失败。派生自识：`self_concept` 是你自己写下的\
                 那一篇（`write_self_concept` 写的就是它）——同一条路整份读。注意：这个地址在这一段\
                 开场时是否在场就已定下；开场时没有 ⇒ 本段里读不到，刚写下的那一份下一次开场才出现；\
                 开场时已有 ⇒ 每次读都是现读最新内容。写入回执会给出长度，可先据此确认写成功。\
                 若文件长、先把这一段的长度读成小一点更省事。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_FIND_TEXT),
                "在授权资料目录里按子串找材料（文件名与正文都看，区分大小写）",
                vec![
                    ParamSpec::new(PARAM_PATTERN, true, "要匹配的子串，区分大小写，不能为空"),
                    ParamSpec::new(
                        PARAM_PATH,
                        false,
                        "授权资料目录内的相对目录，只在它之内查；留空表示资料目录",
                    ),
                ],
            )
            .with_details(
                "匹配：子串不是正则；文件名与正文都算（正文按行看）。候选形状：路径 + \
                 命中位置（`name` 或 `line=N`）+ 那一段片段（片段最多 120 字节，超长以 `…` 截断\
                 ——正文仍在盘上，要用就 `files_read`）。结果按路径名排序，同一条材料最多出现一次。\
                 上限：最多扫 500 个文件、最多返回 100 条候选；触到任一上限时首行如实报截断。\
                 不搜保留地址（`history/`、`manual.md` 与 `self_concept`）：经历有自己的分段结构、手册与\
                 自识各是一份文件，三者都用 `files_list` + `files_read` 看。",
            ),
        ])
    }

    /// 这个实例当前声明的能力：读侧三项，加上有创建授权时的**那两项**写能力与它们各自的复核
    /// 能力，再加上**作品面**那六件（主体在场就在场——主体自己的东西不需要任何声明）。
    fn available_capabilities(&self) -> Capabilities {
        let mut items = Self::capabilities_in(self.machine).items().to_vec();
        if self.materials.is_writable() {
            // 写面那一族说明跟着档走：家档说相对路径，机器档说绝对路径。
            let mode = if self.machine {
                file_app::PathMode::Machine
            } else {
                file_app::PathMode::Home
            };
            items.push(FileApp::creation_capability(mode));
            items.push(FileApp::verification_capability(mode));
            items.push(FileApp::edit_capability(mode));
            items.push(FileApp::edit_verification_capability(mode));
        }
        if self.works_present() {
            items.extend(Self::works_capabilities().items().iter().cloned());
        }
        Capabilities::of(items)
    }

    /// 这一格**在不在场**（`Ready` 与 `Unusable` 都算在场：打不开是另一件事）。
    ///
    /// ⚠ 判据是"这一次装配里有没有这一格"，**不是"它能不能用"**：一个打不开的作品目录若因此
    /// 连动词都不声明，主体得到的读数是 `unknown-action`（"没有这个动作"）——那是**把"读不出来"
    /// 说成了"没有"**。当场在场、按真实分类失败，才是诚实的形状。
    fn works_present(&self) -> bool {
        !matches!(self.works, WorksFace::Absent)
    }

    /// 作品面的六件能力说明：**主体自己的地方**，读两件（写完读回来核实）＋ 写两件与各自复核。
    ///
    /// ⚠ 措辞里"要**跑**它们得有 `terminal` 档"这一句是**能力事实**，不是建议：缺省档里
    /// 主体能写、能读回，但**跑不起来**（执行是另一档的事，见 `terminal_app.rs` 的档位）。
    /// 作品面的准入：三态各说各的（**不把"打不开"说成"不在"**）。
    fn works_admission(&self) -> ActionAdmission {
        match &self.works {
            WorksFace::Ready(_) => ActionAdmission::Admitted,
            WorksFace::Absent => ActionAdmission::refused(FailureClass::UnknownCapability.as_str()),
            WorksFace::Unusable(class) => ActionAdmission::refused(class.as_str()),
        }
    }

    /// 把一件作品面动作交给那一格去做；三态各说各的。
    fn with_works<F>(&self, use_it: F) -> ActionOutcome
    where
        F: FnOnce(&FileApp) -> ActionOutcome,
    {
        match &self.works {
            WorksFace::Ready(app) => use_it(app),
            WorksFace::Absent => ActionOutcome::failure(FailureClass::UnknownCapability.as_str()),
            WorksFace::Unusable(class) => ActionOutcome::failure(class.as_str()),
        }
    }

    fn works_capabilities() -> Capabilities {
        Capabilities::of(vec![
            Capability::new(
                ActionId::new(CAPABILITY_WORKS_LIST),
                "列你自己作品目录里的直接子项（按名称排序）；接着列用 `after`",
                vec![
                    ParamSpec::new(
                        PARAM_PATH,
                        false,
                        "作品目录内的相对目录；留空表示作品目录根",
                    ),
                    ParamSpec::new(
                        PARAM_AFTER,
                        false,
                        "上一次返回过的条目名字；只返回严格大于它的下一批",
                    ),
                ],
            )
            .with_details(
                "这是你自己的地方（`projects/`）：写在这里的文本是你的，不需要任何授权。\
                 上限：一次最多 100 项。边界：`after` 是名字不是下标；它不属于这个\
                 目录时明确失败（不猜近似名）。要跑这些文本得有 `terminal` 档——这一档不在时，\
                 你能写、能读回，但跑不起来。查找（`files_grep`）看的是资料面，\
                 不是你这里。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_WORKS_READ),
                "读你自己作品目录里的一个 UTF-8 文本文件；整份读，或给 `offset`/`limit` 续读一段",
                vec![
                    ParamSpec::new(PARAM_PATH, true, "作品目录内的相对文件路径"),
                    ParamSpec::new(PARAM_OFFSET, false, "本段的起始字节（默认 0）"),
                    ParamSpec::new(PARAM_LIMIT, false, "本段的字节数上限（默认 64 KiB）"),
                ],
            )
            .with_details(
                "与 `files_read` 同一套读数与上限：单次最多 64 KiB，超限拒绝、不截断；`offset`\
                 必须落在字符边界上；续读时回执另给一行（覆盖区间、全文总字节、`more`）。\
                 写完读回来就用它——刚写下的一份，同一段里立刻读得到。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_WORKS_CREATE),
                "在你自己的作品目录里新建一个 UTF-8 文本文件（同名的已经在了就明确失败）",
                vec![
                    ParamSpec::new(PARAM_NAME, true, "作品目录内的单层普通文件名"),
                    ParamSpec::new(PARAM_TEXT, true, "要写入的 UTF-8 文本，允许为空串"),
                ],
            )
            .with_details(
                "边界：只创建——不覆盖、不追加、不改名、不删除；`name` 是单层文件名\
                 （不含路径分隔符与控制字符）。上限：正文最多 64 KiB，超限整体拒绝\
                 （不留半份），空文本合法。失败形状：同名已存在 ⇒ `already-exists`（原名下\
                 一个字节都不动）；创建之后写入或同步失败 ⇒ 无法确认（不是成功，也不是失败）。\
                 这是你自己的东西：不需要任何授权；写完用 `projects_read` 读回来核实。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_WORKS_VERIFY_TEXT),
                "复核一次创建现在的真实状态（与 `projects_create` 同参数，只读）",
                vec![
                    ParamSpec::new(PARAM_NAME, true, "作品目录内的单层普通文件名"),
                    ParamSpec::new(PARAM_TEXT, true, "要核对的 UTF-8 文本"),
                ],
            )
            .with_details(
                "它问的是现在这份参数对应的状态，不是「当时发生了什么」：文件在、是普通文件、\
                 字节与 `text` 完全相同 ⇒ 成功；确认不在 ⇒ `not-found`；存在但不一致 / 不是\
                 普通文件 / 是链接或 reparse point / 读不出来 ⇒ 无法确认。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_WORKS_EDIT),
                "把你作品目录里一份文件的 `old_text` 唯一一处换成 `new_text`",
                vec![
                    ParamSpec::new(PARAM_PATH, true, "作品目录内的相对文件路径"),
                    ParamSpec::new(PARAM_OLD_TEXT, true, "要被换掉的那一段（非空）"),
                    ParamSpec::new(PARAM_NEW_TEXT, true, "换成什么（允许空串＝删掉那一段）"),
                ],
            )
            .with_details(
                "`old_text` 必须恰好出现一次：0 次 ⇒ `not-found`；多次 ⇒ `invalid-argument`\
                 ——两种都一个字节不动，也不猜「你想改哪一处」。替换后全文上限同为 64 KiB，\
                 超限整体拒绝。不改名、不删除、不追加；基准是你自己的作品目录。\
                 写完用 `projects_read`（或 `projects_verify_edit`）核实。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_WORKS_VERIFY_EDIT),
                "复核一次改文本现在的真实状态（与 `projects_edit` 同参数，只读）",
                vec![
                    ParamSpec::new(PARAM_PATH, true, "作品目录内的相对文件路径"),
                    ParamSpec::new(PARAM_OLD_TEXT, true, "被换掉的那一段"),
                    ParamSpec::new(PARAM_NEW_TEXT, true, "换成了什么"),
                ],
            )
            .with_details(
                "三分是现在时：`old_text` 已不在且 `new_text` 在 ⇒ 成功；`old_text` 还在 ⇒\
                 `not-found`；读不出来（不存在 / 不是普通文件 / 链接 / 读失败 / 非 UTF-8）或\
                 「两份都不在」 ⇒ 无法确认。",
            ),
        ])
    }

    /// 视图里的逻辑地址是不是派生经历目录。
    fn is_history_address(names: &[String]) -> bool {
        names.first().map(String::as_str) == Some(HISTORY_DIR)
    }

    /// 视图里的逻辑地址是不是派生手册（保留的**单文件**地址）。
    fn is_manual_address(names: &[String]) -> bool {
        names.first().map(String::as_str) == Some(MANUAL_ADDRESS)
    }

    /// 自识那个地址（单文件，与手册同款）。
    fn is_self_concept_address(names: &[String]) -> bool {
        names.first().map(String::as_str) == Some(SELF_CONCEPT_ADDRESS)
    }

    /// 列出派生经历目录。
    ///
    /// `after` 与资料面共用同一套续列语义：它必须是**这一份列表里**的名字。
    fn list_history(&self, rest: &[String], after: Option<&str>) -> ActionOutcome {
        let Some(history) = &self.history else {
            // 没有经历目录时这个地址不存在：明确失败，不冒充空目录。
            return ActionOutcome::failure(FailureClass::NotFound.as_str());
        };
        match rest.len() {
            0 => self.list_runs(history, after),
            1 => match self.run_history(history, &rest[0]) {
                Ok(Some(run)) => {
                    let items = segments_of(&run);
                    match file_app::render_listing_page(&items, after) {
                        Ok(text) => {
                            ActionOutcome::success(vec![ContentBlock::new(CAPABILITY_LIST, text)])
                        }
                        Err(class) => ActionOutcome::failure(class.as_str()),
                    }
                }
                Ok(None) => ActionOutcome::failure(FailureClass::NotFound.as_str()),
                Err(class) => ActionOutcome::failure(class.as_str()),
            },
            _ => ActionOutcome::failure(FailureClass::NotFound.as_str()),
        }
    }

    /// 列出这个身份环境里全部可查阅的活动（机械目录，不选相关性、不摘要）。
    fn list_runs(&self, history: &HistoryView, after: Option<&str>) -> ActionOutcome {
        let runs = match crate::store::list_runs_at(&history.root) {
            Ok(runs) => runs,
            Err(_) => return ActionOutcome::failure(FailureClass::Unreadable.as_str()),
        };
        let mut items: Vec<file_app::DirectoryEntry> = run_entries(runs)
            .into_iter()
            .map(|entry| (entry.name, "dir"))
            .collect();
        items.sort();
        match file_app::render_listing_page(&items, after) {
            Ok(text) => ActionOutcome::success(vec![ContentBlock::new(CAPABILITY_LIST, text)]),
            Err(class) => ActionOutcome::failure(class.as_str()),
        }
    }

    /// 读一段派生经历。
    fn read_history(&self, rest: &[String]) -> ActionOutcome {
        let Some(history) = &self.history else {
            return ActionOutcome::failure(FailureClass::NotFound.as_str());
        };
        match rest.len() {
            // `history` 与 `history/<活动>` 都是目录：对它们读文本是类型不对。
            0 | 1 => ActionOutcome::failure(FailureClass::WrongKind.as_str()),
            2 => match self.run_history(history, &rest[0]) {
                Err(class) => ActionOutcome::failure(class.as_str()),
                Ok(None) => ActionOutcome::failure(FailureClass::NotFound.as_str()),
                Ok(Some(run)) => {
                    let segments = run.segments.len();
                    let Some(index) =
                        (0..segments).find(|index| segment_name(*index, segments) == rest[1])
                    else {
                        return ActionOutcome::failure(FailureClass::NotFound.as_str());
                    };
                    let text = render_segment(&run, index);
                    if text.len() > MAX_TEXT_BYTES as usize {
                        // 单条记录本身就超过读取上限：明确拒绝，绝不截断半条。
                        return ActionOutcome::failure(FailureClass::TooLarge.as_str());
                    }
                    ActionOutcome::success(vec![ContentBlock::new(CAPABILITY_READ_TEXT, text)])
                }
            },
            _ => ActionOutcome::failure(FailureClass::NotFound.as_str()),
        }
    }

    /// 按逻辑名字取一个活动的经历视图：未知名字返回 `Ok(None)`，读失败返回 `Err`。
    fn run_history(
        &self,
        history: &HistoryView,
        name: &str,
    ) -> Result<Option<RunHistory>, FailureClass> {
        let runs =
            crate::store::list_runs_at(&history.root).map_err(|_| FailureClass::Unreadable)?;
        let Some(entry) = run_entries(runs)
            .into_iter()
            .find(|entry| entry.name == name)
        else {
            return Ok(None);
        };
        let run = entry.summary.run().clone();
        let page = crate::store::read_run_at(&history.root, &run, None, usize::MAX)
            .map_err(|_| FailureClass::Unreadable)?;
        let records = page.records().to_vec();
        let segments = segment_ranges(&records, header_budget(&run, &entry.summary));
        Ok(Some(RunHistory {
            run,
            summary: entry.summary,
            records,
            segments,
        }))
    }
}

impl ActionPort for FileView {
    /// 单独使用时，本 App 就是唯一的那个世界：能力就是它声明的两项。
    ///
    /// 装配进 OS 之后，能力的可用性由 OS 的作用域决定（未展开之前这里的能力不该被使用）。
    fn capabilities(&self, _agent: &AgentId, _run: &RunId) -> Capabilities {
        self.available_capabilities()
    }

    /// 局部准入：读三个动作免写效应门；两个写动作与它们各自的**复核**整条交给真实资料目录判断
    /// （授权在它手里）；作品面那六件由**它自己那一格在不在**决定。
    fn admit(&self, agent: &AgentId, run: &RunId, action: &Action) -> ActionAdmission {
        match action.id().as_str() {
            CAPABILITY_LIST | CAPABILITY_READ_TEXT | CAPABILITY_FIND_TEXT => {
                ActionAdmission::Admitted
            }
            file_app::CAPABILITY_CREATE_TEXT
            | file_app::CAPABILITY_VERIFY_TEXT
            | file_app::CAPABILITY_EDIT
            | file_app::CAPABILITY_VERIFY_EDIT => self.materials.admit(agent, run, action),
            // 作品面：主体自己的东西。**不绑身份与活动**——只要这一格在场就放行；
            // 不在场（`Absent`）连声明都没有，打不开（`Unusable`）则如实报那个分类。
            CAPABILITY_WORKS_LIST
            | CAPABILITY_WORKS_READ
            | CAPABILITY_WORKS_CREATE
            | CAPABILITY_WORKS_VERIFY_TEXT
            | CAPABILITY_WORKS_EDIT
            | CAPABILITY_WORKS_VERIFY_EDIT => self.works_admission(),
            _ => ActionAdmission::refused(FailureClass::UnknownCapability.as_str()),
        }
    }

    fn execute(
        &mut self,
        agent: &AgentId,
        operation: &OperationId,
        action: &Action,
    ) -> ActionOutcome {
        match action.id().as_str() {
            CAPABILITY_LIST => self.list(agent, operation, action, action.params()),
            CAPABILITY_READ_TEXT => self.read_text(agent, operation, action),
            // 查找只在真实资料目录里发生：经历是派生只读视图，它有自己的分段结构。
            CAPABILITY_FIND_TEXT => self.find_text(action.params()),
            // 创建只在真实资料目录里发生：经历目录（`history`）永远是派生的只读视图。
            file_app::CAPABILITY_CREATE_TEXT => {
                self.materials
                    .create_text(agent, operation, action.params())
            }
            // 复核也只对真实资料目录：派生经历是只读视图，不承载任何写效果。
            file_app::CAPABILITY_VERIFY_TEXT => {
                self.materials
                    .verify_text(agent, operation, action.params())
            }
            // 改文本同样只在真实资料目录里发生，基准是授权目录（不是资料根）。
            file_app::CAPABILITY_EDIT => {
                self.materials.edit_text(agent, operation, action.params())
            }
            file_app::CAPABILITY_VERIFY_EDIT => {
                self.materials
                    .verify_edit_text(agent, operation, action.params())
            }
            // 作品面那六件：整条交给**主体自己的那一格**（同一套墙、同一份实现，不同的门）。
            CAPABILITY_WORKS_LIST => self.with_works(|works| works.list(action.params())),
            CAPABILITY_WORKS_READ => self.with_works(|works| works.read_text(action.params())),
            CAPABILITY_WORKS_CREATE => {
                self.with_works(|works| works.create_text(agent, operation, action.params()))
            }
            CAPABILITY_WORKS_VERIFY_TEXT => {
                self.with_works(|works| works.verify_text(agent, operation, action.params()))
            }
            CAPABILITY_WORKS_EDIT => {
                self.with_works(|works| works.edit_text(agent, operation, action.params()))
            }
            CAPABILITY_WORKS_VERIFY_EDIT => {
                self.with_works(|works| works.verify_edit_text(agent, operation, action.params()))
            }
            _ => ActionOutcome::failure(FailureClass::UnknownCapability.as_str()),
        }
    }
}

impl FileView {
    /// 机器档下"这条地址要不要整条交给机器面"。
    ///
    /// 判据只有一条：**绝对路径**。相对路径仍然按家档那套解释（保留地址 / 资料面）——
    /// 这既让 `history/`、`manual`、`self_concept` 在机器档里照旧可用，也让"混着写"这类
    /// 失误落成一句明确的 `invalid-path`（机器面自己会说清它要绝对路径）。
    fn machine_delegates(&self, params: &ActionParams) -> bool {
        self.machine
            && params
                .get(PARAM_PATH)
                .is_some_and(|value| Path::new(value).is_absolute())
    }

    /// 列目录：保留地址走派生视图（`history` 列经历目录、`manual.md` 按"它是文件"如实报类型不
    /// 对），其余整条动作交给真实资料目录。
    ///
    /// `after`（续列）在这里只是**透传**：真实目录那一支由 `entries_of` + 同一份渲染处理，
    /// 派生地址这一支由派生列表自己处理——两处的语义同源，都是"这一份列表里的一个名字"。
    fn list(
        &mut self,
        agent: &AgentId,
        operation: &OperationId,
        action: &Action,
        params: &ActionParams,
    ) -> ActionOutcome {
        // **机器档：绝对路径整条交给机器面**（它自己判形状、自己逐段查链接）。
        // 判据只有一条：这条地址是绝对路径 —— 其余（`history/`、`manual`、`self_concept`、
        // 以及任何相对路径）照旧走派生地址与资料面，一个字不变。
        if self.machine_delegates(params) {
            return self.materials.list(params);
        }
        let relative = match file_app::path_parameter_with(params, false, &[PARAM_AFTER]) {
            Ok(Some(value)) => value,
            // 省略路径 = 视图根；这里只做合并，真实遍历仍由资料目录自己完成。
            Ok(None) => String::new(),
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };
        let after = params.get(PARAM_AFTER);
        let names = match file_app::relative_names(&relative) {
            Ok(names) => names,
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };
        if FileView::is_history_address(&names) {
            return self.list_history(&names[1..], after);
        }
        if FileView::is_manual_address(&names) {
            // 手册是**单文件**：对它列目录是类型不对（与在真实资料面里列一个文件同款）；
            // 地址本身不在（环境根里没有那一份）时是"没有这个地址"，不是"空目录"。
            return match self.manual {
                Some(_) => ActionOutcome::failure(FailureClass::WrongKind.as_str()),
                None => ActionOutcome::failure(FailureClass::NotFound.as_str()),
            };
        }
        if FileView::is_self_concept_address(&names) {
            // 自识也是**单文件**：列它是类型不对；地址不在（主体还没写过）时是"没有这个地址"，
            // 不是"空目录"。
            return match self.self_concept {
                Some(_) => ActionOutcome::failure(FailureClass::WrongKind.as_str()),
                None => ActionOutcome::failure(FailureClass::NotFound.as_str()),
            };
        }
        if names.is_empty()
            && (self.history.is_some() || self.manual.is_some() || self.self_concept.is_some())
        {
            // 视图根的合并列表：真实条目 + 保留的逻辑地址（哪个在就摆哪个）。
            return match self.materials.entries_of("") {
                Ok(mut items) => {
                    if self.history.is_some() {
                        items.push((HISTORY_DIR.to_string(), "dir"));
                    }
                    if self.manual.is_some() {
                        items.push((MANUAL_ADDRESS.to_string(), "file"));
                    }
                    if self.self_concept.is_some() {
                        items.push((SELF_CONCEPT_ADDRESS.to_string(), "file"));
                    }
                    items.sort();
                    match file_app::render_listing_page(&items, after) {
                        Ok(text) => {
                            ActionOutcome::success(vec![ContentBlock::new(CAPABILITY_LIST, text)])
                        }
                        Err(class) => ActionOutcome::failure(class.as_str()),
                    }
                }
                Err(class) => ActionOutcome::failure(class.as_str()),
            };
        }
        // 其余路径（含资料目录里的真实子目录）：整条动作委托给真实目录实现，
        // 复用它的路径校验、链接检查与有界读取。
        self.materials.execute(agent, operation, action)
    }

    /// 读文本：保留地址走派生视图（`history` 按段读经历、`manual.md` 整份读手册），其余整条动作
    /// 交给真实资料目录。
    ///
    /// 两个派生地址都**自成一整份**（经历自带分段、手册就是一份文件），因此窗口参数在它们身上
    /// 没有意义：给了就明确失败（`invalid-argument`），不静默忽略——静默忽略会让主体以为
    /// "读到的是某一段"。
    fn read_text(
        &mut self,
        agent: &AgentId,
        operation: &OperationId,
        action: &Action,
    ) -> ActionOutcome {
        let params = action.params();
        // 机器档：绝对路径整条交给机器面（与 `list` 同一条判据）。
        if self.machine_delegates(params) {
            return self.materials.read_text(params);
        }
        let relative =
            match file_app::path_parameter_with(params, true, &[PARAM_OFFSET, PARAM_LIMIT]) {
                Ok(Some(value)) => value,
                Ok(None) => return ActionOutcome::failure(FailureClass::MissingParameter.as_str()),
                Err(class) => return ActionOutcome::failure(class.as_str()),
            };
        if relative.is_empty() {
            return ActionOutcome::failure(FailureClass::InvalidPath.as_str());
        }
        let names = match file_app::relative_names(&relative) {
            Ok(names) => names,
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };
        if FileView::is_history_address(&names) {
            if params.get(PARAM_OFFSET).is_some() || params.get(PARAM_LIMIT).is_some() {
                return ActionOutcome::failure(FailureClass::InvalidArgument.as_str());
            }
            return self.read_history(&names[1..]);
        }
        if FileView::is_manual_address(&names) {
            if self.manual.is_none() {
                // 地址不在（环境根里没有那一份）：任何深度都是"没有这个路径"。
                return ActionOutcome::failure(FailureClass::NotFound.as_str());
            }
            if names.len() > 1 {
                // 它是**一份文件**：文件底下没有可读的子路径（与真实资料面同一条结论）。
                return ActionOutcome::failure(FailureClass::WrongKind.as_str());
            }
            // 单文件整份读：没有"某一段"这回事 ⇒ 窗口参数明确失败（与经历同款），不静默忽略。
            if params.get(PARAM_OFFSET).is_some() || params.get(PARAM_LIMIT).is_some() {
                return ActionOutcome::failure(FailureClass::InvalidArgument.as_str());
            }
            return self.read_manual();
        }
        if FileView::is_self_concept_address(&names) {
            if self.self_concept.is_none() {
                // 地址不在（主体还没写过自识）：任何深度都是"没有这个路径"。
                return ActionOutcome::failure(FailureClass::NotFound.as_str());
            }
            if names.len() > 1 {
                // 它是**一份文件**：文件底下没有可读的子路径（与手册同一条结论）。
                return ActionOutcome::failure(FailureClass::WrongKind.as_str());
            }
            if params.get(PARAM_OFFSET).is_some() || params.get(PARAM_LIMIT).is_some() {
                return ActionOutcome::failure(FailureClass::InvalidArgument.as_str());
            }
            return self.read_self_concept();
        }
        self.materials.execute(agent, operation, action)
    }

    /// 读自识的正文（**整份**，现读）。
    ///
    /// 与手册、与资料面的整份读取**共用同一条实现**（[`file_app::read_text_file`]）：同一个动词在
    /// 三个地址上不许有三套脾气。
    ///
    /// ⚠ **它就是"写完能不能核实"那条判据的落点**（2026-10-13）：但"地址在不在"是**这一段开场时**
    /// 拍下的；开场时已有 ⇒ `files_read("self_concept")` 每次现读最新；开场时没有 ⇒ 刚写下的那一份
    /// 要下一次开场才出现。
    fn read_self_concept(&self) -> ActionOutcome {
        let Some(self_concept) = &self.self_concept else {
            return ActionOutcome::failure(FailureClass::NotFound.as_str());
        };
        match file_app::read_text_file(&self_concept.path) {
            Ok(text) => ActionOutcome::success(vec![ContentBlock::new(CAPABILITY_READ_TEXT, text)]),
            Err(class) => ActionOutcome::failure(class.as_str()),
        }
    }

    /// 读手册的正文（**整份**）。
    ///
    /// 与资料面的整份读取**共用同一条实现**（[`file_app::read_text_file`]）：同一个动词在两个
    /// 地址上不许有两套脾气。每次现读那一份，不缓存——主体改了自己那一份，下一次读到的就是新的。
    fn read_manual(&self) -> ActionOutcome {
        let Some(manual) = &self.manual else {
            return ActionOutcome::failure(FailureClass::NotFound.as_str());
        };
        match file_app::read_text_file(&manual.path) {
            Ok(text) => ActionOutcome::success(vec![ContentBlock::new(CAPABILITY_READ_TEXT, text)]),
            Err(class) => ActionOutcome::failure(class.as_str()),
        }
    }

    /// 按子串查找：**资料面与经历面都查**（手册与自识不在查找面内）。
    ///
    /// ⚠ **2026-10-07 解冻**（判据在理念层，见 `AGENT_PHILOSOPHY.md` §3/§4）：从前 `history` 被划在
    /// 查找面之外。那条与理念直接冲突——§3 把「**查找**」列成**主体在外部空间里的动作之一**，而系统
    /// 在那里的职责只有四条（有事实来源、有权限边界、可恢复、不破坏原始历史）；**替主体划定哪一面
    /// 可查**正是 §4 第一条禁止的形状（"决定什么对 Agent 有意义"）。
    ///
    /// **与资料面同一套脾气**：同一个动词、同一套参数、同形的读数、同一条"没找到 ≠ 没有看"的
    /// 纪律（共用 [`file_app::search_pattern`] / [`file_app::match_in_text`] /
    /// [`file_app::render_matches`]）。只有一处随地址空间变，而且写在文档里：**扫描顺序**——
    /// 资料面按路径名序，经历面按**最近的先**（活动按它最后一条记录的序号倒序）。经历面是**有
    /// 时间**的空间，要找的多半是"我上次处理这个是什么时候"。
    ///
    /// **它只搜读得到的那一份投影**：经历面走的就是 `files_read` 那条渲染路（`render_segment`），
    /// 因此续接载荷、内部文件名、行尾校验和一列都进不了命中。
    ///
    /// **`path` 限定的是子树**：`history`（整个经历面）或 `history/<活动>`。指到一个**段文件**上
    /// 是类型不对（`wrong-kind`），段名对不上是地址不存在（`not-found`）——两件事不许顶替。
    ///
    /// **手册与自识不在查找面内**（两个面都不在）：它们各是**一份文件**，不是空间——能整份读的
    /// 东西不需要查；查找面服务的是"要走一遍才知道有什么"的地方。对它们给 `path` ⇒ 那个地址在场时
    /// `invalid-argument`（**它是保留地址，不能说成"不存在"**），缺席时才是 `not-found`。因此它们的
    /// 内容不会被 `files_grep` 命中——这一条如实登记。
    fn find_text(&self, params: &ActionParams) -> ActionOutcome {
        // 机器档：绝对目录整条交给机器面（它自己判形状；相对路径照旧走派生地址）。
        if self.machine_delegates(params) {
            return self.materials.find_text(params);
        }
        if let Some(path) = params.get(PARAM_PATH) {
            match file_app::relative_names(path) {
                Ok(names) if FileView::is_history_address(&names) => {
                    return match &self.history {
                        Some(history) => self.find_history(history, params, &names),
                        None => ActionOutcome::failure(FailureClass::NotFound.as_str()),
                    };
                }
                Ok(names) if FileView::is_manual_address(&names) => {
                    return match self.manual {
                        Some(_) => ActionOutcome::failure(FailureClass::InvalidArgument.as_str()),
                        None => ActionOutcome::failure(FailureClass::NotFound.as_str()),
                    };
                }
                Ok(names) if FileView::is_self_concept_address(&names) => {
                    return match self.self_concept {
                        Some(_) => ActionOutcome::failure(FailureClass::InvalidArgument.as_str()),
                        None => ActionOutcome::failure(FailureClass::NotFound.as_str()),
                    };
                }
                Ok(_) => {}
                Err(class) => return ActionOutcome::failure(class.as_str()),
            }
        }
        self.materials.find_text(params)
    }

    /// 在经历面里查：**最近的先**扫各活动的各段，命中给"活动/段 + 行号 + 片段"。
    ///
    /// 上限与资料面**同源**：候选 ≤ [`file_app::MAX_ENTRIES`]、扫描 ≤ [`file_app::MAX_SCANNED`]
    /// （这里的单位是**段**，与读数里的 `scanned` 一致）。够了就停，并**如实说截断**。
    fn find_history(
        &self,
        history: &HistoryView,
        params: &ActionParams,
        names: &[String],
    ) -> ActionOutcome {
        let pattern = match file_app::search_pattern(params) {
            Ok(pattern) => pattern.to_string(),
            Err(class) => return ActionOutcome::failure(class.as_str()),
        };
        let runs = match crate::store::list_runs_at(&history.root) {
            Ok(runs) => runs,
            Err(_) => return ActionOutcome::failure(FailureClass::Unreadable.as_str()),
        };
        let mut entries = run_entries(runs);
        // `path` 限定的是**子树**：`history`（整个经历面）或 `history/<活动>`。
        // 指到一个**段文件**上是类型不对（与资料面同一条规则：给文件就说类型不对）；段名对不上
        // 则是地址不存在。两种都在下面"走到那一个活动"时判——那时它的分段刚算出来，不重读一遍。
        let mut limited: Option<String> = None;
        let mut wanted_segment: Option<&str> = None;
        if names.len() > 1 {
            if !entries.iter().any(|entry| entry.name == names[1]) {
                return ActionOutcome::failure(FailureClass::NotFound.as_str());
            }
            limited = Some(names[1].clone());
            if names.len() > 2 {
                wanted_segment = Some(names[2].as_str());
            }
        }
        // **最近的先**：按活动最后一条记录的序号倒序。序号是账本的次序，不是名字的字典序
        // （名字只是机械标识，"最新的一般在最后"这句话不该由字典序担保）。
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.summary.last_seq()));

        let mut scan = file_app::ScanState::default();
        let mut found: Vec<file_app::FoundMatch> = Vec::new();
        for entry in entries {
            if limited.as_deref().is_some_and(|name| name != entry.name) {
                continue;
            }
            if scan.scanned >= file_app::MAX_SCANNED || found.len() >= file_app::MAX_ENTRIES {
                scan.truncated = true;
                break;
            }
            let run = entry.summary.run().clone();
            let Ok(page) = crate::store::read_run_at(&history.root, &run, None, usize::MAX) else {
                scan.skipped += 1;
                continue;
            };
            let records = page.records().to_vec();
            let segments = segment_ranges(&records, header_budget(&run, &entry.summary));
            // 限定的那一个活动：先把"指到段文件上"的两种结论判掉（那时分段刚算出来）。
            if let Some(wanted) = wanted_segment {
                let count = segments.len();
                return match (0..count).any(|index| segment_name(index, count) == wanted) {
                    true => ActionOutcome::failure(FailureClass::WrongKind.as_str()),
                    false => ActionOutcome::failure(FailureClass::NotFound.as_str()),
                };
            }
            let history_run = RunHistory {
                run: run.clone(),
                summary: entry.summary.clone(),
                records,
                segments,
            };
            for index in 0..history_run.segments.len() {
                if scan.scanned >= file_app::MAX_SCANNED || found.len() >= file_app::MAX_ENTRIES {
                    scan.truncated = true;
                    break;
                }
                scan.scanned += 1;
                let text = render_segment(&history_run, index);
                // 单条记录本身超过读取上限的段：**读不到**（与 `files_read` 对它的结论一致），
                // 计入 `skipped`——"没找到"与"没有看"必须分得开。
                if text.len() > file_app::MAX_TEXT_BYTES as usize {
                    scan.skipped += 1;
                    continue;
                }
                let relative = format!(
                    "history/{}/{}",
                    entry.name,
                    segment_name(index, history_run.segments.len())
                );
                if let Some(found_match) = file_app::match_in_text(&relative, &text, &pattern) {
                    found.push(found_match);
                }
            }
        }
        ActionOutcome::success(vec![ContentBlock::new(
            CAPABILITY_FIND_TEXT,
            file_app::render_matches(&found, &scan),
        )])
    }
}

/// 一个活动在派生视图里的可读形态。
struct RunHistory {
    run: RunId,
    summary: RunSummary,
    records: Vec<TraceRecord>,
    /// 段边界：`(起始下标, 结束下标)`，左闭右开。
    segments: Vec<(usize, usize)>,
}

/// 活动的逻辑名字（机械映射；同名时按活动顺序追加后缀，不覆盖）。
struct RunEntry {
    name: String,
    summary: RunSummary,
}

/// 把活动标识机械映射成逻辑名字。
///
/// 只保留字母数字与 `-_.`，其余字符换成 `_`：它不是排序依据、不是摘要，只是稳定地址。
/// 名字为空或退化成 `.` / `..` 时用位置编号兜底（这两个形状不能作为路径段被解释）。
fn run_entries(runs: Vec<RunSummary>) -> Vec<RunEntry> {
    let mut used: BTreeMap<String, usize> = BTreeMap::new();
    let mut entries = Vec::new();
    for (index, summary) in runs.into_iter().enumerate() {
        let mapped: String = summary
            .run()
            .as_str()
            .chars()
            .map(|ch| {
                if ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                    ch
                } else {
                    '_'
                }
            })
            .collect();
        let base = if mapped.is_empty() || mapped == "." || mapped == ".." {
            format!("run~{index}")
        } else {
            mapped
        };
        let mut name = base.clone();
        let mut suffix = 1usize;
        while used.contains_key(&name) {
            name = format!("{base}~{suffix}");
            suffix += 1;
        }
        used.insert(name.clone(), index);
        entries.push(RunEntry { name, summary });
    }
    entries
}

/// 一段经历的稳定名字：零填充序号 + `.txt`。
fn segment_name(index: usize, count: usize) -> String {
    let width = count.saturating_sub(1).to_string().len().max(3);
    format!("{index:0width$}.txt")
}

/// 这个活动的分段列表（机械顺序，不排序、不截断）。
fn segments_of(run: &RunHistory) -> Vec<(String, &'static str)> {
    let count = run.segments.len();
    (0..count)
        .map(|index| (segment_name(index, count), "file"))
        .collect()
}

/// 把一个活动的记录切成段：每段最多 [`RECORDS_PER_SEGMENT`] 条，且不会把一条记录切开。
///
/// 字节预算只用来决定"下一条记录放不放得进这一段"，因此段与段之间既不重叠也不丢记录；
/// 单条记录本身超过读取上限时，它独占一段并在读取时明确报 `too-large`。
fn segment_ranges(records: &[TraceRecord], header_budget: usize) -> Vec<(usize, usize)> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    let mut bytes = 0usize;
    for (index, record) in records.iter().enumerate() {
        let size = rendered_record_len(record);
        if index > start
            && (index - start >= RECORDS_PER_SEGMENT
                || bytes + size + header_budget > MAX_TEXT_BYTES as usize)
        {
            ranges.push((start, index));
            start = index;
            bytes = 0;
        }
        bytes += size;
    }
    ranges.push((start, records.len()));
    ranges
}

/// 一条记录渲染后的字节数（`#序号 种类` 一行 + 正文一行）。
fn rendered_record_len(record: &TraceRecord) -> usize {
    2 + record.seq().to_string().len() + record.entry().kind().len() + record.entry().text().len()
}

/// 段首机械事实行占用的字节预算。
fn header_budget(run: &RunId, summary: &RunSummary) -> usize {
    let mut len = "run = \n".len() + run.as_str().len();
    len += "state = \n".len() + summary.status().len();
    if let Some(ending) = summary.ending() {
        len += "ending = \n".len() + ending.len();
    }
    // 分类是段首的一行**正文**，它的字节必须并进预算——否则加了行就会撑破段大小。
    if let Some(failure) = summary.failure() {
        len += "failure = \n".len() + failure.len();
    }
    len += "records = \n".len() + 20;
    len += "segment = \n".len() + 32;
    len += "segment-records = \n".len() + 20;
    len + 64
}

/// 渲染一段经历：机械事实行 + 按序号排列的记录原文。
///
/// 只搬运、不摘要、不重排：`state` 是存储结构状态，`ending` 是已确认的结束事实原文，
/// `failure` 是模型失败的分类（有才出现），活动标识与序号都是事实本身。
fn render_segment(run: &RunHistory, index: usize) -> String {
    let (start, end) = run.segments[index];
    let mut text = String::new();
    push_header_line(&mut text, "run", run.run.as_str());
    push_header_line(&mut text, "state", run.summary.status());
    if let Some(ending) = run.summary.ending() {
        push_header_line(&mut text, "ending", ending);
    }
    // 分类单独一行：主体读自己的经历时，这是"那次失败属于哪一类"的唯一落脚处。
    if let Some(failure) = run.summary.failure() {
        push_header_line(&mut text, "failure", failure);
    }
    push_header_line(&mut text, "records", &run.summary.records().to_string());
    push_header_line(
        &mut text,
        "segment",
        &format!("{}/{}", index + 1, run.segments.len()),
    );
    push_header_line(&mut text, "segment-records", &(end - start).to_string());
    for record in &run.records[start..end] {
        text.push('#');
        text.push_str(&record.seq().to_string());
        text.push(' ');
        text.push_str(record.entry().kind());
        text.push('\n');
        text.push_str(&record.entry().text());
        text.push('\n');
    }
    text
}

/// 追一行机械事实行。
fn push_header_line(text: &mut String, name: &str, value: &str) {
    text.push_str(name);
    text.push_str(" = ");
    text.push_str(value);
    text.push('\n');
}

impl crate::app_contract::AppPort for FileView {
    fn app_id(&self) -> &str {
        FILE_APP_ID
    }

    fn description(&self) -> &str {
        "一个只读文件视图：授权资料目录里的真实文件，加上保留逻辑目录 `history`（这个身份环境已确认的经历，按活动机械呈现）、保留单文件地址 `manual.md`（你自己家里那份手册），以及你自己作品目录 `projects/` 的读写动作"
    }

    fn declared_capabilities(&self) -> Capabilities {
        // 声明必须与实例的真实能力一致：装配期的归属索引就是按这份声明建的，
        // 少声明一件就会"目录里有、路由不到"，多声明一件则相反。
        self.available_capabilities()
    }

    /// 写得出来的部分必须配得齐：**有创建授权时**才声明 `files_create` 是独占创建、
    /// `files_edit` 也是独占创建，并各自指向同属本 App 的只读复核能力（`files_verify_text` /
    /// `files_verify_edit`）。
    ///
    /// 改文本与独占创建同类，判据是**同参重放行为**：第一次消耗前置（`old_text` 被换掉），
    /// 第二次同参数必然明确失败且无害。
    ///
    /// 没有授权时这里返回空——那时两个写能力连能力都不存在，声明它们会被装配期按
    /// `unknown-declared-effect` 拒绝；这正是"声明与实例能力同源派生"的含义。
    fn declared_effects(&self) -> Vec<crate::app_contract::EffectDeclaration> {
        use crate::app_contract::{EffectClass, EffectDeclaration};
        let mut declared = Vec::new();
        if self.materials.is_writable() {
            declared.push(EffectDeclaration::new(
                ActionId::new(file_app::CAPABILITY_CREATE_TEXT),
                EffectClass::ExclusiveCreate,
                Some(ActionId::new(file_app::CAPABILITY_VERIFY_TEXT)),
            ));
            declared.push(EffectDeclaration::new(
                ActionId::new(file_app::CAPABILITY_EDIT),
                EffectClass::ExclusiveCreate,
                Some(ActionId::new(file_app::CAPABILITY_VERIFY_EDIT)),
            ));
        }
        // 作品面：同样两对（独占创建 ＋ 唯一替换），复核与写动词**同参数**。
        if self.works_present() {
            declared.push(EffectDeclaration::new(
                ActionId::new(CAPABILITY_WORKS_CREATE),
                EffectClass::ExclusiveCreate,
                Some(ActionId::new(CAPABILITY_WORKS_VERIFY_TEXT)),
            ));
            declared.push(EffectDeclaration::new(
                ActionId::new(CAPABILITY_WORKS_EDIT),
                EffectClass::ExclusiveCreate,
                Some(ActionId::new(CAPABILITY_WORKS_VERIFY_EDIT)),
            ));
        }
        declared
    }

    /// 来源档 = **这一段回执里携带的内容从哪个口子进来**。
    ///
    /// 读动词（列目录 / 读文本 / 查找 / 两次复核）带回的是**环境根里给主体的材料**，因此是
    /// [`crate::app_contract::SourceClass::Environment`]；两个写动词只回报"系统对这次动作的确认"
    /// （建了几个字节），回执里**没有外来字**，因此是 [`crate::app_contract::SourceClass::System`]。
    ///
    /// 与 [`AppPort::declared_capabilities`] **同源派生**：能力清单少一件，来源声明就少一件，
    /// 因此"目录里有、来源表里没有"在结构上不可能发生——这正是装配期那条
    /// `capability-without-source` 检查要保证的事。
    fn declared_sources(&self) -> Vec<crate::app_contract::SourceDeclaration> {
        use crate::app_contract::{SourceClass, SourceDeclaration};
        self.declared_capabilities()
            .items()
            .iter()
            .map(|capability| {
                let class = match capability.id().as_str() {
                    file_app::CAPABILITY_CREATE_TEXT
                    | file_app::CAPABILITY_EDIT
                    | CAPABILITY_WORKS_CREATE
                    | CAPABILITY_WORKS_EDIT => SourceClass::System,
                    _ => SourceClass::Environment,
                };
                SourceDeclaration::new(capability.id().clone(), class)
            })
            .collect()
    }
}
