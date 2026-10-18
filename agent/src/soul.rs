//! 天性（soul，TASK_70）：主体出生那天写就、此后由自己改写的那一篇自述。
//!
//! 与 [`crate::pinned_actions`]（动作的格位）和 [`crate::pins_board`]（贴在墙上的纸条）是**三件事**：
//! 那两样是主体**后来自己弄的**，这一样是**出生那天写就**的那一份。名字各带自己的词——一词一主。
//!
//! - **存储**：`<环境根>/soul`（与 `pinned_actions` 同层的**自持物**，主体的地盘）。
//! - **只读装配**：装配期读回、注入开场（RunStart），**不在 Run 中途变**。
//! - **写权归自己**（主人 2026-09-21 裁决）：`write_soul` 是主体的自主动词（见 `os.rs` 的
//!   `OS_OWNED_ACTIONS`）。产品**不预装任何标准人格文本**——那是替所有用户的世界定"官方人格"。
//! - **三态不许压成两态**：[`Soul::Unreadable`] 绝不冒充 [`Soul::Absent`]——
//!   "读不到"与"主体还没有天性"是两件事，后者只见于文件**真的不在**。
//!
//! 它不持有跨活动状态、不认识历史 / 行动 / 模型 / 账本——与格位、纸条板同款：只把**读回来的那一态**摆出来。

use std::io;
use std::path::{Path, PathBuf};

use crate::core::combine;
use crate::core::{ContentBatch, ContentBlock, ContextSource, InjectionContext};

/// 天性那一块在注入里的标签。
pub const SOUL_LABEL: &str = "soul";
/// 身份行那一块在注入里的标签。
pub const IDENTITY_LABEL: &str = "identity";
/// 自持物里那个文件的名字：`<环境根>/soul`。
pub const SOUL_FILE: &str = "soul";

/// 天性的三态。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Soul {
    /// **没有那一份**（文件不在，或只有一个空文件）⇒ 开场**优雅跳过**，不注入空壳。
    #[default]
    Absent,
    /// 读到了正文。
    Present(String),
    /// **这一次没读到**（权限、坏数据……）⇒ 如实标出，不冒充"主体没有天性"。
    Unreadable,
}

/// 天性的**字节上限**（TASK_76）：够天性、不够自传。
///
/// **为什么要在写入杠杆处拒绝、而不是让注入闸兜底**：这一块是**唯一逐字整份每场注入**的常驻块
/// （最贵的自持物）——注入闸一旦兜底，失败形态是"天性**整块**超限未给"，也就是**最不该丢的那块
/// 裸奔**。所以上限是**硬墙**：**拒绝、不落盘、不截断**，旧的一份原样保留（对齐 notes 的
/// `too-large` 先例：超限拒绝，不截断）。
///
/// **取值依据**（TASK_76 报告里有完整交代）：主体本机那份 soul **还不存在**（现值 **0 字节**，
/// `read` 读作 [`Soul::Absent`]）⇒ 没有"现值"约束；参考量级取"够天性、不够自传"——
/// 4 KiB ≈ 1300 个汉字 ≈ 一页，同时只占注入批（64 KiB）的 **6%**，不挤占别的块。
pub const MAX_SOUL_BYTES: usize = 4 * 1024;

/// 超限写入的**稳定分类标记**（`write_soul` 用它渲染拒绝读数；逐字节稳定，词表纪律）。
pub const SOUL_TOO_LARGE: &str = "soul-too-large";

/// 自持物的路径。
fn soul_path(environment_root: &Path) -> PathBuf {
    environment_root.join(SOUL_FILE)
}

/// 读主体的天性。
///
/// **文件不存在 = 主体还是新生儿**（还没写过）⇒ [`Soul::Absent`]；
/// **其他读失败** ⇒ [`Soul::Unreadable`]（**不冒充 Absent**）；
/// **读到空白** ⇒ 也算 `Absent`：一个空文件不是天性。写入口会拒绝写空
/// （见 [`incept`]），所以这一支只可能来自外部手改——把它读成"没有"比读成一个空洞好。
pub fn read(environment_root: &Path) -> Soul {
    match std::fs::read_to_string(soul_path(environment_root)) {
        Ok(text) => {
            if text.trim().is_empty() {
                Soul::Absent
            } else {
                Soul::Present(text)
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Soul::Absent,
        Err(_) => Soul::Unreadable,
    }
}

/// **写下这一步本身**：三段检查 + 一次 `write`。**它不是公开面**（2026-10-08 收口）。
///
/// 它是 [`write_from_subject`] 的里子——"多一个名字就多一条改天性的路"，所以模块的公开面
/// 仍然只有**两个**：写（[`write_from_subject`]）与读（[`read`]）。单元用例在同一个模块里，
/// 照旧能直接调它来验三段拒绝。
///
/// **空正文拒绝**（`InvalidInput`）："没有天性"应当是**根本没有那一份**，而不是留一个空洞
/// （否则开场会读到一个空壳）。
///
/// **超限拒绝**（`InvalidData`）：超过 [`MAX_SOUL_BYTES`] ⇒ **不写、不截断**，旧的一份**原样保留**
/// ——调用方按错误种类渲染稳定分类（[`SOUL_TOO_LARGE`]）。两条拒绝都发生在**动手之前**：
/// 这个函数里没有任何一条路径会在拒绝之后留下半个文件。
///
/// 两条拒绝都发生在**动手之前**：这个函数里没有任何一条路径会在拒绝之后留下半个文件。
fn incept(environment_root: &Path, text: &str) -> io::Result<()> {
    if text.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "天性的正文不能是空的",
        ));
    }
    if text.len() > MAX_SOUL_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "天性的正文超过上限",
        ));
    }
    std::fs::write(soul_path(environment_root), text)
}

/// 写下天性时的**失败分类**（封闭；稳定字符串进主体的回执）。
///
/// 两条都发生在**动手之前**、都**不截断、不覆盖**——旧的那一份原样保留。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoulWriteFailure {
    /// 超过 [`MAX_SOUL_BYTES`]（带上"想写多大 / 旧的现在多大"两件机械事实）。
    TooLarge { wanted: usize, current: usize },
    /// 写不进去（权限、盘满…）。
    Unwritable,
}

impl SoulWriteFailure {
    /// 稳定分类标记（进读数）。
    pub fn as_str(self) -> &'static str {
        match self {
            SoulWriteFailure::TooLarge { .. } => SOUL_TOO_LARGE,
            SoulWriteFailure::Unwritable => "soul-unwritable",
        }
    }

    /// 拒绝读数（只有超限那一档带正文）：**想写多大、上限多大、旧的还在不在**，三件事一次说清
    /// ——主体不用去别处查。
    pub fn reading(self) -> Option<ContentBlock> {
        match self {
            SoulWriteFailure::TooLarge { wanted, current } => Some(ContentBlock::new(
                SOUL_LABEL,
                format!(
                    "soul = {wanted} B\n上限 = {MAX_SOUL_BYTES} B\n旧的 soul 原样保留（现值 {current} B）"
                ),
            )),
            SoulWriteFailure::Unwritable => None,
        }
    }
}

/// **写下 / 改写主体的天性**：这一份事实的**唯一写者**。
///
/// 域规则全在这里：超限**拒绝、不写、不截断**，旧的那一份原样保留。
/// 成功时交回**给主体的那一块回执**（含"现在多大 / 上限多大"的账，以及"下一次开场生效"）。
///
/// ⚠ **不在这里的两件**（那是**参数事实**与**路由事实**）：① 空正文（与"缺参数、参数形状不对"
/// 同族，归 `os` 的 `require_params` 那一侧）；② 这一次装配有没有环境根。两件都是"能不能调"，
/// 不是"天性是什么"。
pub fn write_from_subject(
    environment_root: &Path,
    text: &str,
) -> Result<ContentBlock, SoulWriteFailure> {
    // 现值：用本模块**既有的**读入口算（本模块只导出「写」与「读」两个入口——
    // 「多一个名字就多一条改天性的路」，守卫钉着这条）。
    let current = match read(environment_root) {
        Soul::Present(text) => text.len(),
        _ => 0,
    };
    match incept(environment_root, text) {
        Ok(()) => Ok(ContentBlock::new(
            SOUL_LABEL,
            format!(
                // **账单**（TASK_76）：每一笔写都带上"现在多大 / 上限多大"——杠杆处看得见，
                // 开场那块注入一个字节都不加（开场预算纪律优先）。
                "天性已写下（soul = {} B，上限 {MAX_SOUL_BYTES} B）。\
                 **下一次开场生效**：这一次的视野已经定了，改动不回头改它。",
                text.len()
            ),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
            Err(SoulWriteFailure::TooLarge {
                wanted: text.len(),
                current,
            })
        }
        Err(_) => Err(SoulWriteFailure::Unwritable),
    }
}

/// **宿主声明的身份**（TASK_70）：core 侧那个 id 与此世界的名字。///
/// 两个值**同源**——都出自宿主那一次 `GET /v1/backend/me`（`agent.id` 与 `agent.display_name`）。
///
/// ⚠️ **这不是本机身份**：本机环境里那份身份记录（`store` 建环境时写下的那一份）里的
/// `agent.id` 属于**另一套命名空间**（本地身份归环境、im 身份归 im），两者**从不相等**。
/// 把它们并进同一句话，会让主体以为"我在这台机器上是谁"与"我在这个世界里是谁"是同一个人
/// 在两套系统里的两个名字。
///
/// ⚠️ **装配只搬运、不发请求**：这一格是**声明**（与 `origin` 同族）。让装配期自己现取
/// 会打破既有舞台（装配链要能拿 IO／网络，且每次开场多一次往返）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdentityClaim {
    /// core 侧的 Agent id。
    pub id: String,
    /// 此世界里的名字（`display_name`）。
    pub display_name: String,
}

impl IdentityClaim {
    /// 开场那一行：**两个值原样摆出来**。
    pub fn line(&self) -> String {
        format!("你是 {}，在此世界名为 {}", self.id, self.display_name)
    }

    /// 两个值都在（空串与"没给"同罪——一个空的名字不是名字）。
    pub fn is_complete(&self) -> bool {
        !self.id.is_empty() && !self.display_name.is_empty()
    }
}

/// 开场那两块：**天性**（主体是谁，自己写）与**身份行**（主体在哪个世界叫什么，世界给的）。
///
/// 两块**挨着摆**，因为它们回答的正是同一个问题——只是来源不同：
/// 天性是主体的手笔、住在主体的地盘上；身份行是世界的事实、由宿主声明带进来。
///
/// 三态各自的呈现：
/// - `Present(text)` ⇒ 原文摆出（前后不加东西——**这是主体的字**）；
/// - `Absent` ⇒ **一个字都不说**（连标签都不出现：开场**跳过**，不注入空壳）；
/// - `Unreadable` ⇒ 如实说"这一次没读到"，**不冒充 `Absent`**。
#[derive(Debug)]
pub struct SoulBoard<Inner> {
    /// 装配期读回来的那一态。
    soul: Soul,
    /// 宿主声明的身份（`None` = 这一次没声明 ⇒ 身份行也跳过）。
    identity: Option<IdentityClaim>,
    inner: Inner,
}

impl<Inner> SoulBoard<Inner> {
    /// 用装配期读回来的那一态与宿主声明装配。
    pub fn new(soul: Soul, identity: Option<IdentityClaim>, inner: Inner) -> Self {
        Self {
            soul,
            identity,
            inner,
        }
    }

    /// 读完之后的**装配期**构造：从环境根读一次（同一份读法，两处装配点共用）。
    pub fn from_environment(
        environment_root: &Path,
        identity: Option<IdentityClaim>,
        inner: Inner,
    ) -> Self {
        Self::new(read(environment_root), identity, inner)
    }

    /// 这一次要递交的块（**可能 0、1 或 2 块**——缺席的不占位）。
    fn own_batch(&self) -> ContentBatch {
        let mut blocks = Vec::new();
        match &self.soul {
            // 原文摆出：一个字都不加。
            Soul::Present(text) => blocks.push(ContentBlock::new(SOUL_LABEL, text.clone())),
            // **优雅跳过**：不注入空壳、连标签都不出现。
            Soul::Absent => {}
            Soul::Unreadable => blocks.push(ContentBlock::new(
                SOUL_LABEL,
                "（这一次没读到你的天性；它还在你的环境根里，这只是这次没读到）",
            )),
        }
        // 身份行：**两个值都在**才摆（空串与"没给"同罪——一个空的名字不是名字）。
        if let Some(identity) = self.identity.as_ref().filter(|claim| claim.is_complete()) {
            blocks.push(ContentBlock::new(IDENTITY_LABEL, identity.line()));
        }
        // 装不下（自己写的天性可以很大）：**不留空批次**，留一条读数，让"天性这一次没给"
        // 在视野里看得见——它正是这一批里唯一可能真超限的那一块。
        ContentBatch::of_or_not_delivered(blocks, SOUL_LABEL)
    }

    /// 内层来源（只读）。
    pub fn inner(&self) -> &Inner {
        &self.inner
    }
}

impl<Inner: ContextSource> ContextSource for SoulBoard<Inner> {
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

    /// 临时根：按进程号分开，避免并行测试互相踩。
    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("celira-soul-{}-{}", std::process::id(), tag))
    }

    /// 往返：写进去什么，读出来就是什么；**没写过**读成 `Absent`（不是故障）。
    #[test]
    fn what_is_written_is_what_comes_back() {
        let root = temp_root("roundtrip");
        std::fs::create_dir_all(&root).expect("可以建临时根");
        let _ = std::fs::remove_file(soul_path(&root));

        // 没写过 ⇒ Absent：这是"主体还没有天性"，不是"读不到"。
        assert_eq!(read(&root), Soul::Absent);

        let text = "我是阿默：先看事实，再说话。\n";
        incept(&root, text).expect("可以写");
        assert_eq!(read(&root), Soul::Present(text.to_string()));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **空正文拒绝**：没有天性应当是"根本没有那一份"，而不是留一个空洞。
    #[test]
    fn an_empty_soul_is_refused_instead_of_written_as_a_hole() {
        let root = temp_root("empty");
        std::fs::create_dir_all(&root).expect("可以建临时根");

        assert!(incept(&root, "").is_err(), "空串必须拒绝");
        assert!(incept(&root, "   \n").is_err(), "只有空白也必须拒绝");
        assert_eq!(read(&root), Soul::Absent, "拒绝之后不该留下文件");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 读取侧：读到空白文件 ⇒ `Absent`（不是 `Unreadable`，也不是 `Present("")`）。
    #[test]
    fn a_blank_file_reads_as_absent_not_as_a_hole() {
        let root = temp_root("blank");
        std::fs::create_dir_all(&root).expect("可以建临时根");
        std::fs::write(soul_path(&root), "  \n").expect("手工写一个空白文件");

        assert_eq!(read(&root), Soul::Absent);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **读不到**不冒充"没有天性"：把 soul 换成目录 ⇒ 读失败 ⇒ `Unreadable`。
    #[test]
    fn unreadable_is_never_reported_as_absent() {
        let root = temp_root("unreadable");
        std::fs::create_dir_all(soul_path(&root)).expect("用目录占住那个名字");

        assert_eq!(
            read(&root),
            Soul::Unreadable,
            "读不出来要如实说读不出来，不许冒充「天性不存在」"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
