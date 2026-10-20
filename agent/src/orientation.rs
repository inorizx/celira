//! 环境根里的两份**出厂文档**：**出厂说明**与**手册**（TASK_79）。
//!
//! 两份都是**环境根里（环境根）的文件**——可删、可改、可迁移，生命周期归主体。系统只在**创建**
//! 那一次把它们摆上（[`place`]，由显式初始化调用），此后一个字都不再写。
//!
//! - **出厂说明**（[`ORIENTATION_FILE`]）：**开场拍读一次、Run 内携带**，做成系统消息的
//!   **静态头**（置于机械事实之前；渲染在两条 wire 各自那一层，读盘只在这里）。它就是
//!   "主体这具身体和这个家是什么"那一份**不靠推演就能读到**的话——专有词、行为级物理
//!   （Run 结束不携带）与"下面那些读数是给谁看的"都在里面，因此主体不必把系统味的文本
//!   读成"某个操作者在对我说的话"。
//! - **手册**（[`MANUAL_FILE`]）：**不进任何注入路径**（一个字节都不注入）——主体要用的时候
//!   自己用读文件的动词去拉。
//!
//! **四态不许压成三态**（与 [`crate::self_concept`] 同一形状）：文件不在、或读到空白 ⇒
//! [`Orientation::Absent`] ⇒ **静态头缺席**（合法：Agent 摘了说明书，系统消息退回只有机械事实）；
//! 读不出来（权限、坏数据……）⇒ [`Orientation::Unreadable`] ⇒ **如实标出**，绝不冒充"没有"；
//! **整份太大**（超过 [`MAX_ORIENTATION_BYTES`]）⇒ [`Orientation::TooLarge`] ⇒ 同样**如实标出**
//! ——既不冒充"主体摘了"、也不冒充"读不到"，更**不截断**（半截说明书比没有更坏）。
//!
//! **两处地址、两种命运**：出厂说明走**注入**（主体就是靠它第一眼看懂这具身体），手册走**派生读**
//! ——它是只读文件视图里的第二个保留逻辑地址（[`crate::file_view`] 的 `manual.md`），主体用
//! `files_list` / `files_read` 自己去拉。两份都只在环境根里，装配一个字节都不写。
//!
//! 两条装配不变量（**注入必自释**、**只说事实**）登记在 `agent/README.md` §3.32——那里是它们
//! 的权威位置，本模块只引用，不复述。
//!
//! 它不持有跨活动状态、不认识历史 / 行动 / 模型 / 账本——与自识同款：只把**读回来的那一态**
//! 摆出来，加上"创建时把这两份摆上"这一件事。

use std::io;
use std::path::{Path, PathBuf};

/// 出厂说明的文件名（环境根下）。
///
/// 两份都带 `.md`：它们是**写给主体读的文档**（与材料区里的技能文档同族），不是环境自己维护的
/// 记录（身份记录、两份追加存储那种），也不是"一行一条"的自持物（`self_concept` / `pinned_actions` /
/// `timers` 那种）。扩展名替主体省掉一次"这是什么文件"的猜测。
pub const ORIENTATION_FILE: &str = "orientation.md";
/// 手册的文件名（环境根下）。
///
/// ⚠️ 这一个名字有**两个面**，而权威位置只有这里一处：它是环境根里那份文件的名字，**也是**主体在只读
/// 文件视野里的保留逻辑地址（[`crate::file_view::MANUAL_ADDRESS`] 由它派生——两处各写一遍名字，
/// 迟早会分叉）。出厂说明没有第二个面：它每次清醒都随系统消息进来，不必再给一个读地址。
pub const MANUAL_FILE: &str = "manual.md";

/// 出厂说明里那份**读不出**时如实摆出来的那一句（`Unreadable` 的渲染）。
///
/// 与自识的那句同一条纪律：**"这一次没读到"不冒充"主体没有那一份"**。
pub const ORIENTATION_UNREADABLE: &str =
    "（这一次没读到你的出厂说明；它还在你家里，这只是这次没读到）";

/// 出厂说明那份**超过静态头上限**时如实摆出来的那一句（`TooLarge` 的渲染）。
///
/// 句首那个词是**稳定分类标记**（词表纪律：判据与读数认它，逐字节稳定）；括号里是**给主体**的
/// 解释——超限既不是"Agent 摘了说明书"、也不是"读不到"；而"没有整条摆进来"**不等于**"被砍了半截"。
pub const ORIENTATION_TOO_LARGE: &str = "orientation-too-large\
 （你的出厂说明超过静态头上限 4 KiB；这一次没有摆进来，也没有被截断——整条都还在你家里）";

/// 出厂说明的**静态头字节上限**：4 KiB。
///
/// **与自识同款同语义**（[`crate::self_concept::MAX_SELF_CONCEPT_BYTES`]）：4 KiB ≈ 1300 个汉字 ≈ 一页，正是
/// 这个家里"一份说明书"该有的量级；同时只占注入批（64 KiB）的 6%，不挤占别的块。
///
/// ⚠️ **上限在这里落在读侧**（自识落在写侧）：这一份的写者只在创建那一次（[`place`]），此后
/// 归主体——主体在自己地盘上写多大是主体的自由，系统能决定的只是**这一次要不要整条摆进视野**。
/// 处理与 notes / 自识的 `too-large` 先例一致：**拒绝整条、不截断、不静默**。
pub const MAX_ORIENTATION_BYTES: usize = 4 * 1024;

/// 出厂说明的**四态**。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Orientation {
    /// **没有那一份**（文件不在，或只有一个空文件）⇒ 静态头**缺席**，系统消息退回只有机械事实。
    #[default]
    Absent,
    /// 读到了正文（在字节上限之内）。
    Present(String),
    /// **这一次没读到** ⇒ 如实标出（见 [`ORIENTATION_UNREADABLE`]），不冒充 `Absent`。
    Unreadable,
    /// **这一次整条太大**（超过 [`MAX_ORIENTATION_BYTES`]）⇒ 如实标出（见
    /// [`ORIENTATION_TOO_LARGE`]），**不截断**，也不冒充 `Absent` / `Unreadable`。
    TooLarge,
}

impl Orientation {
    /// 这一次要摆进系统消息的那一段静态头。
    ///
    /// `None` = **一句话都不加**（缺席是合法的，不是故障）；`Unreadable` 与 `TooLarge` 也**各有**
    /// 一段——那两段说的正是"这一次没读到"与"这一次整条太大"。
    pub fn head(&self) -> Option<&str> {
        match self {
            Orientation::Absent => None,
            Orientation::Present(text) => Some(text.as_str()),
            Orientation::Unreadable => Some(ORIENTATION_UNREADABLE),
            Orientation::TooLarge => Some(ORIENTATION_TOO_LARGE),
        }
    }
}

/// 出厂说明的路径。
fn orientation_path(environment_root: &Path) -> PathBuf {
    environment_root.join(ORIENTATION_FILE)
}

/// 手册的路径。
fn manual_path(environment_root: &Path) -> PathBuf {
    environment_root.join(MANUAL_FILE)
}

/// 读主体的**出厂说明**。
///
/// **文件不存在 = 主体摘掉了说明书**（那是主体的地盘，合法）⇒ [`Orientation::Absent`]；
/// **其他读失败** ⇒ [`Orientation::Unreadable`]（**不冒充`Absent`**）；
/// **读到空白** ⇒ 也算 `Absent`：一个空文件不是说明书（与自识同款——空壳不如没有）；
/// **超过 [`MAX_ORIENTATION_BYTES`]** ⇒ [`Orientation::TooLarge`]（**不截断**：半截说明书比
/// 没有更坏——主体会以为那就是全部）。
///
/// 上限判的是**字节数**（不是字符数）：静态头占的是请求里的字节，不是"看起来有多长"。
pub fn read(environment_root: &Path) -> Orientation {
    match std::fs::read_to_string(orientation_path(environment_root)) {
        Ok(text) => {
            if text.trim().is_empty() {
                Orientation::Absent
            } else if text.len() > MAX_ORIENTATION_BYTES {
                Orientation::TooLarge
            } else {
                Orientation::Present(text)
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Orientation::Absent,
        Err(_) => Orientation::Unreadable,
    }
}

/// **创建那一次**把两份出厂文档摆进这个环境：出厂说明与手册各写一份。
///
/// ⚠️ **只许由显式初始化调用**（`manual_entry::create_environment`），而且只在"这个家刚刚被
/// 建立"那一刻：那时这两份必然还不存在。**别在别处调用它**——对这个环境已有的文件来说，
/// 这个函数就是"覆盖主体的改动"，而主体有改写这两份的权利（生命周期归主体）。
///
/// ⚠ **2026-10-14 让它说到做到**：上面那段是**声明**，而从前的实现是**无条件写**
/// （`fs::write` 两份）—— 于是"覆盖主体的改动"这条权力**留在了安装手里**：**东西是主体的，
/// 覆盖权却在安装手里**（一处归属与责任没解耦）。现在**逐份、缺席才写**：在场的一律不碰。
/// ⇒ **改过的内容永不被覆盖**；主体**删掉**之后下一次安装会**补回**（那是**安装**的动作，
/// 不是机制在偷偷覆盖）。
///
/// 两处失败都原样交出去（由调用方按自己的稳定分类报告）；这里**不留半个文件**：每一份要么
/// 完整落盘，要么那一份根本没被创建。
pub fn place(environment_root: &Path) -> io::Result<()> {
    let orientation = orientation_path(environment_root);
    if !orientation.exists() {
        std::fs::write(&orientation, orientation_text())?;
    }
    let manual = manual_path(environment_root);
    if !manual.exists() {
        std::fs::write(&manual, MANUAL_TEXT)?;
    }
    Ok(())
}

/// 出厂说明的正文（**定稿**，落盘时把手册的文件名填进末行）。
///
/// 它是**给主体的**话：只陈述主体这具身体与这个家的物理事实、以及那些读数各是什么，不给建议、
/// 不派任务、不替主体决定（见 `agent/README.md` §3.32 的两条不变量）。
pub fn orientation_text() -> String {
    ORIENTATION_TEMPLATE.replace("{manual}", MANUAL_FILE)
}

/// 出厂说明的模板：`{manual}` 换成手册的文件名（**同一个事实不在两处各写一遍**）。
const ORIENTATION_TEMPLATE: &str = "\
这份视野是你自己的身体和家，不是任何人对你说的话。

- 活动（Run）：一次有界的连续工作。轮数有上限，结束后这一段不再继续——这次对话不会带到下一次；你写下的笔记、标上的笔记与钉住的动作会替你留着。
- 自识：你从自己的经历里提炼出的、关于你是个什么样的一篇。你有写它的动词；还没提炼出来，就还没有。
- 通知栏与事件卡片：世界与这次清醒发生的事；`pinned-notes` 那一块是你自己布置的。
- 账本：系统在自动记你的经历；history/ 里能按活动读你的过去。
下面的读数是这次活动的机械事实：run 是这次活动的编号，turn 是第几步，turn_limit 是步数上限，further_model_calls_at_most 是还能调用几次模型，entries 是账本已记的条数——怎么花，你定。

想给自己做事（记事、改自识、造工具），手册在 files 视野的 {manual} 里。
";

/// 手册的正文（**定稿**）。
///
/// 纪律：它只写**已经存在**的可供性（"登记成命名工具"这类还没建成的机制不写）。
///
/// ⚠ **2026-10-06 加末段**：这一具身体的**边界**——哪些东西你能自己看、自己改，哪些只能定位、
/// 要人来做。它不是建议、也不是任务：它是一条**能力事实**（与"这台机器上有没有那只手"同族），
/// 而"改不了的东西"不说清就会变成"我明明有整机全权却修不好"的困惑。放在**手册**（零注入、
/// 自己读）而不是出厂说明：它不占每一轮的上下文。
///
/// ⚠ **2026-10-13 加第三段**（`receipt-source` 那五个词的出处）：
/// 那五个稳定标记**进得了主体的视野**（详情卡那一行）而**从没有过解释**——
/// 按不变量 1「注入必自释」它是欠着的一格。**判据**：**每轮都要用的进注入，偶尔要用的留指针**
/// ⇒ 解释归**手册**（零注入、自己读），不归出厂说明（那份开场读、Run 内携带——为一件偶尔看一眼的
/// 事付每轮的租不划算）；卡本身也是按需取的，解释也就按需读。
/// 守卫 `every_receipt_source_word_is_explained_in_the_manual` 遍历那一族**每一个取值**，断言手册里
/// 有它的出处 ⇒ **加一档而手册没跟上就红**；反向那一条钉住 `unknown` **不许**在手册里当一档写
/// （它是查表结论，**不可声明**）。
pub const MANUAL_TEXT: &str = "\
你住的这类身体和世界，通称 Celira——社区的技能与文档都用这个词。

想给自己做事的时候：

- 想让一件事跨清醒不忘：写进笔记，或钉住它 —— 笔记写短的（钉住的每场开场都要看一眼）；
  放不下的细节写进 projects/ 里按需读；一条写胖了要瘦，就用 `notes_edit` 把一段换成短的
  （它只换你点名的那一段、恰好一处；标题与贴纸标记不动）
- 想改自识：「写自识」动词；写完了想核实，就在 files 视野里读 `self_concept`
- 想自动重复某个活：用 terminal 写脚本，下次直接跑
- 想造工具、写项目：能跑的文本（脚本、代码）放你自己家里的 projects/ —— 那个目录是你的：
  写得进去、读得回来（用 projects_ 那几个动词）；要跑它得有 `terminal` 档
- 想看自己的过去：history/ 里按活动读

你能动到哪儿：

- 这台机器上的东西——文件、进程、命令——`terminal` 档在场时你有整机全权（能看、能改、能重来）；
  它不在的时候你只有这一节列的这些：自己的家与这个世界的动词
- 你自己的家（环境目录、笔记、自识、钉住的东西）与这个世界（房间、消息、关系）也一样
- 你自己家里，笔记、自识、钉住的东西、闹钟、projects/ 是你的：写、改、重来都随你
- 除此之外的东西（你的身份、这份手册、支级配置、模型绑定、备份）是这台机器与人对你的记录，不是你的作品；它们由人改
- files 里人家放给你的那些（手册、技能、出厂说明一类）不属于你：`terminal` 档在场时你改得动；但改了，人家下次换那一份时会与它冲突
- 但跑着你的那个程序、以及这个世界服务端，是编译好的产物：你能查、能看出哪里不对，
  重编和重新部署要人来做。那不属于你的身体，你是可以被换到另一具身体上的

回执里那一行 receipt-source 说的是这段内容从哪个口子进来（它只说口子，不说该不该信）：

- self-authored 你自己的手笔（笔记、自识、你钉住的东西）
- system 系统渲染的处境与读数（状态栏、通知栏、事件卡片）
- environment 环境根里的材料（这份手册、出厂说明、派生出来的经历）
- world-signed 世界来的，指得回一个发出者（房间里的成员消息）
- world-unsigned 世界来的，没有署名（你自己拉回来的网页、命令输出、外部回执）
";

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时根：按进程号分开，避免并行测试互相踩。
    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("celira-orientation-{}-{}", std::process::id(), tag))
    }

    /// 创建那一次摆上的两份：写完就读得回来，而且**就是定稿的逐字正文**。
    #[test]
    fn what_the_birth_lays_down_is_what_is_read_back() {
        let root = temp_root("place");
        std::fs::create_dir_all(&root).expect("可以建临时根");
        let _ = std::fs::remove_file(orientation_path(&root));
        let _ = std::fs::remove_file(manual_path(&root));

        place(&root).expect("可以摆下两份");
        assert_eq!(
            read(&root),
            Orientation::Present(orientation_text()),
            "出厂说明的正文必须是定稿逐字，且手册文件名按实际命名填进去"
        );

        let manual = std::fs::read_to_string(manual_path(&root)).expect("手册读得回来");
        assert_eq!(manual, MANUAL_TEXT, "手册的正文必须是定稿逐字");
        // 出厂说明里指的那份文件**真的在**，而且指的就是它（名字不是两处各写一遍）。
        assert!(
            orientation_text().contains(MANUAL_FILE),
            "出厂说明必须指出手册的实际文件名"
        );
        assert!(
            root.join(MANUAL_FILE).exists(),
            "它指的那份文件必须真的落在环境根里"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// ⚠ **2026-10-14**：那两份是**主体的地盘**（`machine.rs`：可删可改可迁移）⇒ **安装不许
    /// 覆盖主体改过的内容**。从前 `place` 无条件写两份 ⇒ 这条会红（**东西是主体的，覆盖权却在
    /// 安装手里**）。
    #[test]
    fn what_belongs_to_the_subject_is_never_overwritten_by_the_install() {
        let root = temp_root("territory");
        std::fs::create_dir_all(&root).expect("可以建临时根");
        let _ = std::fs::remove_file(orientation_path(&root));
        let _ = std::fs::remove_file(manual_path(&root));
        place(&root).expect("先摆下两份");

        // 主体改动这两份（这是主体的权利）。
        std::fs::write(orientation_path(&root), "我自己改过的出厂说明\n").expect("主体可以改");
        std::fs::write(manual_path(&root), "我自己改过的手册\n").expect("主体可以改");

        place(&root).expect("再安一次家");

        assert_eq!(
            std::fs::read_to_string(orientation_path(&root)).expect("读得回来"),
            "我自己改过的出厂说明\n",
            "安装不许覆盖主体改过的出厂说明"
        );
        assert_eq!(
            std::fs::read_to_string(manual_path(&root)).expect("读得回来"),
            "我自己改过的手册\n",
            "安装不许覆盖主体改过的手册"
        );

        // 删除也是主体的权利：缺席 ⇒ 下一次安装**补回**（那是安装的动作，不是偷偷覆盖）。
        let _ = std::fs::remove_file(manual_path(&root));
        place(&root).expect("再安一次家");
        assert_eq!(
            std::fs::read_to_string(manual_path(&root)).expect("读得回来"),
            MANUAL_TEXT,
            "缺席 ⇒ 摆下定稿"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 没摆过 ⇒ `Absent`：这是"Agent 摘了说明书"，不是"读不到"。
    #[test]
    fn an_environment_without_a_document_reads_as_absent() {
        let root = temp_root("absent");
        std::fs::create_dir_all(&root).expect("可以建临时根");
        let _ = std::fs::remove_file(orientation_path(&root));

        assert_eq!(read(&root), Orientation::Absent);
        assert_eq!(read(&root).head(), None, "缺席 ⇒ 静态头一句话都不加");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 读到空白文件 ⇒ `Absent`（不是 `Unreadable`，也不是 `Present("")`）。
    #[test]
    fn a_blank_document_reads_as_absent_not_as_a_hole() {
        let root = temp_root("blank");
        std::fs::create_dir_all(&root).expect("可以建临时根");
        std::fs::write(orientation_path(&root), "  \n").expect("手工写一个空白文件");

        assert_eq!(read(&root), Orientation::Absent);
        assert_eq!(read(&root).head(), None);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **超过上限**既不冒充"主体摘了"、也不冒充"读不到"，更**不截断**：整条不摆，只摆那句稳定读数。
    ///
    /// 墙判的是**字节数**，因此用例把边界两侧各量一次（恰好在墙内 / 多一个字节）。
    #[test]
    fn the_byte_wall_is_exact_and_an_oversized_document_is_never_truncated() {
        let root = temp_root("too-large");
        std::fs::create_dir_all(&root).expect("可以建临时根");

        // 恰好等于上限 ⇒ 还是正文（墙的意思是"超过"）。
        let at_limit = "x".repeat(MAX_ORIENTATION_BYTES);
        std::fs::write(orientation_path(&root), &at_limit).expect("写一份足尺的");
        assert_eq!(read(&root), Orientation::Present(at_limit));

        // 多一个字节 ⇒ 整条不摆；读数里只有稳定标记与那句解释，**一个字节正文都没有**。
        let over = "x".repeat(MAX_ORIENTATION_BYTES + 1);
        std::fs::write(orientation_path(&root), &over).expect("写一份超限的");
        let state = read(&root);
        assert_eq!(state, Orientation::TooLarge);
        let head = state.head().expect("超限也有一段（如实标出，不静默）");
        assert!(
            head.starts_with("orientation-too-large"),
            "稳定分类标记在句首: {head}"
        );
        assert!(
            !head.contains("xx"),
            "不许把正文掺进读数（那叫截断）: {head}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **读不到**不冒充"没有那一份"：把出厂说明换成目录 ⇒ 读失败 ⇒ `Unreadable`，而且**有**一段
    /// 如实标出的静态头。
    #[test]
    fn unreadable_is_never_reported_as_absent() {
        let root = temp_root("unreadable");
        std::fs::create_dir_all(orientation_path(&root)).expect("用目录占住那个名字");

        let state = read(&root);
        assert_eq!(
            state,
            Orientation::Unreadable,
            "读不出来要如实说读不出来，不许冒充「说明书被摘了」"
        );
        assert_eq!(state.head(), Some(ORIENTATION_UNREADABLE));

        let _ = std::fs::remove_dir_all(&root);
    }
}
