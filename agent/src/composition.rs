//! 最小组合入口：把 Core + 最小 OS 骨干（接入契约 / 展开 / 路由）+ 只读文件视图 + 一个 Model
//! 装起来，驱动有限活动。
//!
//! 它**不是操作系统**：没有通知、没有唤醒、没有调度、没有多 App 切换、没有授权系统、
//! 没有安装机制。装配只有一条静态路径——那些 App 从**入口**开始，能力要等模型自己展开。
//!
//! 处境事实由**外层来源**提供：**状态栏**（[`StatusBar`]，时间 + 轮次读数，每轮重摆）作为外层
//! 持着最小的手动环境来源作为内层，两处批次都递交。组合层只负责把它们接上现有接缝：本层不做
//! 判断、不解释内容，也不因为来源不同而出现第二条执行路径。
//!
//! 时间来源是**装配声明的一部分**（[`SessionRequest::clock`]）：谁在跑这个进程，谁就提供
//! "现在几点"。生产装配给真实钟，离线夹具给固定钟——同一段装配代码、同一个格式化点，差别只在
//! 构造参数。
//!
//! 授权资料根必须由调用者显式给出：这里没有默认值，也不猜（见 [`FileView::materials_only`]）。
//!
//! 本模块有两个驱动入口，共用同一条 Core 循环与同一段装配：
//!
//! - [`run_activity_in_environment`]：**一次性**——装配、驱动一次、把结果交回；
//! - [`RunSession`]：**会话**——装配一次、驱动多次，等待之后由宿主显式继续（见类型文档）。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::activity_origin::ActivityOrigin;
use crate::clock::{ClockHandle, LocalOffset, unix_millis};
use crate::commit::EnvironmentCommit;
use crate::context_reading::ContextReading;
use crate::core::{
    ActionId, Agent, Capabilities, Capability, CommitError, CommitStage, ContentBatch,
    ContentBlock, ContextSource, DriveOutcome, InjectionContext, Limits, ModelPort, NoCommit,
    NoContext, Run, RunError, RunInput, RunRequest, TraceEntry, combine, continue_activity,
    no_cost, run_activity_with,
};
use crate::cost::{RunCost, UsageReport};
use crate::creation_grant::CreationGrant;
use crate::event_cards::{EventCards, EventCardsReport};
use crate::file_app::FailureClass;
use crate::file_view::{FileView, FileViewFailure};
use crate::foundation::{AgentId, Ledger, Outcome, RunId, SubjectEnding, Turn};
use crate::holdings::Holdings;
use crate::im_app::{IM_APP_ID, ImApp};
use crate::im_channel::ImAccess;
use crate::im_profile_app::{IM_PROFILE_APP_ID, ImProfileApp};
use crate::im_social_app::{IM_SOCIAL_APP_ID, ImSocialApp};
use crate::machine::Machine;
use crate::mcp::{McpAccess, McpApp};
use crate::model_retry::{ModelRetry, RetryingModel, TRANSIENT_RETRY_ATTEMPTS};
use crate::notes_app::{NOTES_APP_ID, NotesApp};
use crate::notification_bar::NotificationBar;
use crate::os::{Desktop, MachineFailure, MinimalOs, RunExpansion};
use crate::pinned_actions::PinnedActions;
use crate::pins_board::PinsBoard;
use crate::previous_activity::{PreviousActivity, PreviousReading, UnsettledOperation};
use crate::recovery::{self, RunState};
use crate::self_concept::{IdentityClaim, SelfConceptBoard};
use crate::skills_desktop::{SkillsDesktop, read_skills};
use crate::status_bar::StatusBar;
use crate::store::{Admission, Environment, EnvironmentReader, TracePayload, WakeOrigin};
use crate::terminal_app::{TERMINAL_APP_ID, TerminalAccess, TerminalApp};
use crate::web_app::{SearchAccess, WEB_APP_ID, WebApp};

/// 一次只读文件活动的装配结果。
///
/// 手写 `Debug`（而不是要求 Foundation 的 `Ledger` 派生 `Debug`）：Foundation 的语义与派生
/// 本刀都不动，调试输出只报告事实条数，而不是把整本账打出来。
pub struct ReadOnlyFilesRun {
    /// 跑完或明确失败的那次活动。
    pub run: Run,
    /// 这次活动写下的全部事实（与 `run` 同一份记录的两个视角）。
    pub ledger: Ledger,
}

impl std::fmt::Debug for ReadOnlyFilesRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadOnlyFilesRun")
            .field("run", &self.run)
            .field("records", &self.ledger.records().len())
            .finish()
    }
}

impl ReadOnlyFilesRun {
    /// 这次活动是怎么收场的。
    pub fn outcome(&self) -> Option<Outcome> {
        self.run.outcome()
    }
}

/// 组合入口拒绝启动的原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompositionError {
    /// 只读文件视图拒绝启动（没有可用的授权资料根）。
    FileAppRefused(FailureClass),
    /// 装配被拒绝：授权资料根会暴露身份环境的内部记录，或 App 标识无法被唯一解释。
    ///
    /// 它带一个稳定分类标记：调用方需要区分"拒绝启动"，不需要认识配置细节。
    MachineRefused(String),
    /// 活动层面被拒绝（例如同一个 Agent 已经有未收场的活动）。
    Run(RunError),
    /// 身份环境打开失败（缺失 / 损坏 / 不属于这个身份 / 已有写者）。
    ///
    /// 它带一个稳定分类字符串，而不是把存储细节摊开：调用方需要区分"拒绝"，不需要认识格式。
    Environment(String),
    /// 这次活动**没有被完整确认为耐久**。
    ///
    /// 它把三件事分开报告，调用方不需要（也不能）从存储细节里猜：
    ///
    /// - `stage`：停在哪一个确认点；
    /// - `ending_confirmed`：终态是否**已经**被确认为耐久（封存阶段失败时为 `true`——
    ///   活动确实结束了，缺的只是封存标记）；为 `false` 时活动**没有**被确认结束；
    /// - `pause_recorded`：这次失败是否已经留下了持久的暂停记录。
    ///   ⚠ **2026-09-26 改**：为 `false` 时**不再有任何准入兜底**——原来那句"重开会由
    ///   `CompositionError::UnfinishedRun` 兜底拦住"随那个变体一起退役了。现在为 `false`
    ///   意味着这次失败**没有留下任何持久痕迹**：它只体现在**这一次的报告**里
    ///   （活动留下未收尾的形状，而"上一段没有收尾"由**开场那一轮**如实带给主体）。
    ///   ⚠ **2026-10-06 改**：为 `true` 时那条暂停记录**也不再拦开跑**（`StoragePaused` 那道闸
    ///   同日退役，见 `agent/README.md` §3.3）——它由开场那一块纸条带给主体，是**事实**不是闸。
    SaveFailed {
        /// 停在哪一个确认点。
        stage: CommitStage,
        /// 终态是否已经被确认为耐久。
        ending_confirmed: bool,
        /// 是否已经留下持久的暂停记录。
        pause_recorded: bool,
    },
    /// 这次会话**已经不能再被驱动**。
    ///
    /// 两种情形，都是"再来一次不可能成立"，因此都不允许被当成又一次等待：
    ///
    /// - 活动已经收场：`Run` 有终态，活动槽也已经交还；
    /// - 上一次驱动以失败告终（保存没被确认、账本写不下去……）：那一刻已经停止驱动这次活动，
    ///   而且**不**允许靠"再驱动一次"把失败抹成一次正常等待。
    SessionNotDrivable,
}

impl From<RunError> for CompositionError {
    fn from(error: RunError) -> Self {
        CompositionError::Run(error)
    }
}

/// 显式装配只读文件视图，用给定的 Model 驱动**一次**有限活动。
///
/// 只读：本入口不会产生任何写效应，因此也不需要 Grant。读免门，但**绝不免目录授权**——
/// 授权资料根由调用者显式传入，能力也只覆盖那个根内部。活动从**入口**开始：模型必须先展开该
/// App 的能力，之后才可能使用文件能力（能力只增不减，入口能力不会被顶掉）。
pub fn run_read_only_files<M: ModelPort>(
    material_root: &Path,
    agent_id: AgentId,
    run_id: RunId,
    limits: Limits,
    model: &mut M,
) -> Result<ReadOnlyFilesRun, CompositionError> {
    // 旧入口 = **空输入**的一次性装配：同一个实现，不是第二套装配。
    run_read_only_files_with_input(
        material_root,
        agent_id,
        run_id,
        limits,
        RunInput::empty(),
        model,
    )
}

/// 与 [`run_read_only_files`] 相同，只是带着这次活动的**初始输入**。
///
/// 输入属于这一次活动：它随 Run 一起结束，不会被下一次装配继承；正文也不进入 Foundation 的事实。
/// 它不额外注入任何上下文（空来源）。
pub fn run_read_only_files_with_input<M: ModelPort>(
    material_root: &Path,
    agent_id: AgentId,
    run_id: RunId,
    limits: Limits,
    input: RunInput,
    model: &mut M,
) -> Result<ReadOnlyFilesRun, CompositionError> {
    run_read_only_files_from(
        material_root,
        agent_id,
        model,
        RunRequest {
            run: run_id,
            limits,
            input,
            source: &mut NoContext,
            commit: &mut NoCommit,
            // 只读文件那几条路不记代价（这一格"没有被记"，盘上形状一个字不多）。
            cost: &no_cost,
        },
    )
}

/// 与 [`run_read_only_files`] 相同，只是带**手动环境来源**：这次活动由宿主手动发起。
///
/// 环境来源（[`ActivityOrigin`]）在 Run 开始注入一条机械事实，之后每轮返回空批次；
/// 它不决定何时开始活动、不看历史、不产生动作。这里不新增第二条装配路径：本入口同样委托到
/// [`run_read_only_files_from`]。
pub fn run_read_only_files_manually<M: ModelPort>(
    material_root: &Path,
    agent_id: AgentId,
    run_id: RunId,
    limits: Limits,
    model: &mut M,
) -> Result<ReadOnlyFilesRun, CompositionError> {
    let mut environment = ActivityOrigin::new();
    run_read_only_files_from(
        material_root,
        agent_id,
        model,
        RunRequest {
            run: run_id,
            limits,
            input: RunInput::empty(),
            source: &mut environment,
            commit: &mut NoCommit,
            cost: &no_cost,
        },
    )
}

/// 与上面两个入口相同，只是由调用方显式给出**上下文来源**（开场输入与来源一起传）。
///
/// 这是唯一一套装配：来源由外层对象实现并持有，Core 不保存它、也不跨活动搬运它缓存的东西。
/// 这个入口**不保留任何经历**（空提交），与"没有经历接缝"的行为完全一致。
pub fn run_read_only_files_from<M: ModelPort>(
    material_root: &Path,
    agent_id: AgentId,
    model: &mut M,
    request: RunRequest<'_>,
) -> Result<ReadOnlyFilesRun, CompositionError> {
    let RunRequest {
        run: run_id,
        limits,
        input,
        source,
        commit,
        cost,
    } = request;

    // 最小 OS 显式装配只读文件视图；拒绝启动就是拒绝启动，不猜授权根。
    let mut os = MinimalOs::open(material_root).map_err(machine_refused)?;

    let mut agent = Agent::new(agent_id.clone());
    let mut ledger = Ledger::new(agent.id().clone());

    // 展开状态是**自有值**，与这一次活动绑定；作用域只是它的临时视图，借用 OS 与它。
    // 函数返回时作用域销毁、展开状态一并回收，OS 与 App 仍然可以给下一次活动复用，
    // 且下一次从未展开开始。
    //
    let mut expansion = RunExpansion::new(agent_id.clone(), run_id.clone());
    let mut scope = os.scope(&mut expansion);
    let run = run_activity_with(
        &mut agent,
        RunRequest {
            run: run_id,
            limits,
            input,
            source,
            commit,
            cost,
        },
        model,
        &mut scope,
        &mut ledger,
    )?;

    Ok(ReadOnlyFilesRun { run, ledger })
}

/// 装配被拒绝 → 组合入口的错误分类。
///
/// 资料根自身不可用沿用文件分类；其余（资料根会暴露环境内部记录、App 标识无法唯一解释）
/// 带 OS 自己的稳定标记，不摊开配置细节。
fn machine_refused(failure: MachineFailure) -> CompositionError {
    match failure {
        MachineFailure::View(FileViewFailure::Materials(class)) => {
            CompositionError::FileAppRefused(class)
        }
        other => CompositionError::MachineRefused(other.as_str().to_string()),
    }
}

/// 在一次环境化活动里，调用方**声明的开场事实**。
///
/// 它只是参数的载体，不是新的一层抽象：把"哪个身份、哪次活动、边界多大、开场说什么"绑在一起，
/// 避免入口收下过于零散的同源参数。身份在这里是**待核对的声明**，权威身份仍然只从环境里读。
pub struct EnvironmentActivity<'a> {
    /// 调用方声明的身份。与环境里记录的权威身份不一致就是拒绝启动。
    pub declared_agent: &'a AgentId,
    /// 这次活动的标识。由调用方生成。
    pub run: RunId,
    /// 这次活动的资源边界。
    pub limits: Limits,
    /// 调用方明确给出的开场内容；没有就是 [`RunInput::empty`]。
    pub input: RunInput,
    /// 上下文来源；没有额外注入时用 [`NoContext`]。
    pub source: &'a mut dyn ContextSource,
    /// 宿主给出的**文本创建授权**；`None` 表示这一次活动没有任何写权限（默认）。
    ///
    /// 它只在这里被**读一次**，用来决定装配出来的能力目录里到底有没有创建能力：授权不进
    /// Foundation 的事实、不进经历、不跨越活动。
    pub creation: Option<&'a CreationGrant>,
}

/// 在**一个显式身份环境**里启动一次活动：身份从环境里读，经历写进环境。
///
/// 这是本刀的端到端入口。它兑现的顺序是：
///
/// 1. **执行准入**：先核对身份与环境的准入读数。已记录在案的保存失败、或没有收尾的旧活动，
///    都在**建立任何执行对象、调用任何模型之前**拒绝启动；
/// 2. 从环境里确认身份，用**同一个身份**建立 Agent 与账本（不采信调用者另外给的字符串）；
/// 3. 装配**只读文件视图**（授权资料目录 + 这个环境的派生经历目录）；资料根与环境根重叠会在
///    这一步被拒绝，而且仍然在建立执行对象之前；
/// 4. 经唯一一条 Core 循环驱动，每一步都由本函数**自己建立**的 [`EnvironmentCommit`]
///    确认为耐久；
/// 5. 一旦有一步没被确认为耐久，**立即停止**（Core 的 `?` 已经保证不再调用模型、不再尝试动作），
///    并尽力留下持久的暂停记录。标记写不下去时也不放松：重开由"没有收尾的活动"兜底拦住。
///
/// **为什么提交接缝由本函数自己建立**：经历要写进的就是这个环境，而派生经历目录同时也要被
/// 只读查阅。"谁来写"和"谁来读"必须是**同一个**环境目录，但一个 `&mut` 不能被借两次。
/// 所以调用方不能再自己塞一个提交接缝进来——那会逼出"两个写者"或"借用冲突"二选一。
/// 这个入口因此只收"声明 + 来源"，把"确认为耐久"这件事留在它自己手里。
///
/// **准入为什么必须在入口而不是别处**：这是**唯一**会拿一个环境去驱动模型的入口。
/// 把准入留在辅助函数里、或只加一个暂停字段而不查，都等于没有门禁。
pub fn run_activity_in_environment<M: ModelPort>(
    material_root: &Path,
    environment: &mut Environment,
    activity: EnvironmentActivity<'_>,
    model: &mut M,
) -> Result<ReadOnlyFilesRun, CompositionError> {
    let EnvironmentActivity {
        declared_agent,
        run: run_id,
        limits,
        input,
        source,
        creation,
    } = activity;

    // 环境与调用者声明的身份必须一致：不一致就是拒绝启动，不"就近采用"。
    // 分类文本由存储层自己的稳定标记给出，组合层不另编一个。
    if environment.agent() != declared_agent {
        return Err(CompositionError::Environment(
            crate::store::StoreError::IdentityMismatch {
                expected: declared_agent.clone(),
                found: environment.agent().clone(),
            }
            .as_str()
            .to_string(),
        ));
    }

    // 执行准入：只依据**已确认的耐久记录**判断，不默认健康。
    //
    // ⚠ **2026-09-26 改**：`Unfinished` **不再拒绝**（主人裁决）。理由是 Run 的边界：
    // **一段清醒被一段不清醒隔开，就是两段**——承载活动的进程没了，那一次连续就断了，
    // 那一段也就结束了。新进程起来是**新的一段**，不是"顶着同一个 RunId 继续"。
    //
    // 而"上一段没有收尾"这件事，**是主体唯一不可能知道的**：它自己收的场它知道，
    // runtime 兜底收的场（`system:*`）它也知道——只有"根本没收到场"，它无从得知。
    // ⇒ 那句话由**开场那一轮**如实带给它（见 `PreviousActivity`），**不用拒绝来挡**。
    //
    // ⚠ **2026-10-06 改（主人裁决）：`Paused` 也不再拒绝。** 它是一条**已经记录在案的存储故障**，
    // 与"未收尾"同族：**门是指令，而世界只该给信息**。这条记录由开场那一块纸条带给主体
    // （`PreviousActivity` 的 `StorageFailure` 那一段：哪一段停的、什么原因、有没有被确认过），
    // 能不能继续、要不要先修，由主体看着事实自己定——主体有一台**整机全权**的命令行，
    // 而执行端与 im 是编译好的产物（主体只能定位，重编归人）。
    // **写路径照旧 fail-closed**：真写不下去时，那一轮在**第一次提交**（run-start，
    // 模型调用之前）就停住，因此不会反复烧调用。
    // ⚠ 这一处**不绑定**那个活动：一次性入口**不装处境链**（它是 `run_read_only_files` 那条路），
    // 因此没有"开场那一轮"可以把这句话带给主体。会话路径（`RunSession`）才绑。

    // 权威身份只从环境里取：调用者的声明只用于核对，不用于装配。
    let agent_id = environment.agent().clone();

    // **建立执行对象之前**装配：授权资料根与环境根必须分开，资料根覆盖环境根就是拒绝启动
    // （否则模型能用通用文件动作读到身份记录、两份追加存储、写锁与暂停记录）。
    // 目录必须在借出写资格**之前**取下来——读目录这个动作本身也会借用环境。
    let environment_root = environment.root().to_path_buf();

    // 两个 App 显式装配，用的是**同一份窄契约**：OS 不知道它们各自是什么，只知道它们的标识、
    // 说明、声明能力与执行入口。Notes 永远装配（这样才读得到旧笔记），而且**预装常开**：
    // 写自己的笔记不需要任何授权（材料目录那种跨边界写才需要）。
    // 一次性入口**不接 im**（TASK_21 §4 明确不做）：这一条路径恒为 `None`，因此它的能力面与
    // 从前逐字相同。im 只在会话路径（`agent_run run`）按宿主的声明装配。
    let mut os = assemble_machine(
        material_root,
        &environment_root,
        creation,
        MachineDeclarations {
            // 一次性入口**不接 im**（TASK_21 §4 明确不做）：这一条路径恒为 `None`。
            im: None,
            // 一次性入口**没有档位声明**（与它不接 im 同款）：取默认档 `full` = 出厂以来的样子。
            web: WebDeclaration {
                search: None,
                tier: crate::web_app::WebTier::Full,
            },
            terminal: None,
            mcp: &[],
            // 一次性入口不接 im（见上）⇒ 这一格在它身上不产生任何效果：如实给 fail-closed 那一档。
            vision: false,
        },
    )?;

    // 这个块就是**写资格的借用期**：提交接缝在块内持有 `&mut Environment`，
    // 块一结束写资格就归还，下面才轮得到环境自己去留暂停记录。
    let outcome = {
        let mut agent = Agent::new(agent_id.clone());
        let mut ledger = Ledger::new(agent.id().clone());
        let mut commit = EnvironmentCommit::new(environment);
        // 展开状态是自有值，与这一次活动绑定；未展开时任何 App 能力都不在目录里、也不会被执行。
        let mut expansion = RunExpansion::new(agent_id.clone(), run_id.clone());
        let mut scope = os.scope(&mut expansion);
        run_activity_with(
            &mut agent,
            RunRequest {
                run: run_id.clone(),
                limits,
                input,
                source,
                commit: &mut commit,
                // 只读文件那条路不记代价。
                cost: &no_cost,
            },
            model,
            &mut scope,
            &mut ledger,
        )
        .map(|run| ReadOnlyFilesRun { run, ledger })
    };

    outcome.map_err(|error| finish_failure(environment, &run_id, error))
}

/// 一次活动没有被完整确认时怎么收尾：**尽力**留下持久暂停记录，并如实报告停在哪一步。
///
/// 三点必须是分开的事实，不能压成一个"失败"：
///
/// 1. 停在哪一个确认点（[`CommitStage`]）；
/// 2. 终态**有没有**被确认（封存失败时终态已经确认，活动确实结束了）；
/// 3. 暂停记录**有没有**真的留下。
///
/// 只有保存失败（[`RunError::Commit`]）才写暂停标记：模型失败、账本失败、预算耗尽都不是
/// "保存失败"，不该被写成存储故障。它们也不会放松保护——没有收尾的活动由准入挡住。
fn finish_failure(environment: &mut Environment, run: &RunId, error: RunError) -> CompositionError {
    let RunError::Commit(commit) = &error else {
        return CompositionError::Run(error);
    };
    let stage = commit.stage();
    let ending_confirmed = commit.after_confirmed_ending();
    // 尽力而为：这条记录本身也可能写不下去（例如上一次写入没停在行边界上）。
    // 写不下去**不**改变"立即停止"这件事，只改变重开时拦住它的理由。
    let reason = format!("save-failed:{}", stage.as_str());
    let pause_recorded = environment.mark_paused(run, &reason).is_ok();
    CompositionError::SaveFailed {
        stage,
        ending_confirmed,
        pause_recorded,
    }
}

// ---------------------------------------------------------------------------
// 会话：装配一次、驱动多次
// ---------------------------------------------------------------------------

/// 一次会话驱动停在哪儿。
///
/// 它与 Core 的 [`DriveOutcome`] 一一对应：`Unsettled` **不是**一种收场，`Ended` 才带结局。
/// 会话把这两者分开，是因为调用方对它们的下一步动作完全不同——未收场要继续驱动同一个活动，
/// 收场则不能再驱动。
///
/// ⚠️ **它从前叫 `Waiting`**（2026-10-06 改）：`SubjectEnding::Waiting` 是主体自己说"我在等
/// 外部的东西"并因此**收场**，与这一档**相反**。两个相反的结局不许共用一个词。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionDrive {
    /// 停在未收场：活动**没有**结束，活动槽仍然属于它，轨迹、轮次、预算与展开状态都还在。
    Unsettled,
    /// 活动已经收场（主体自己结束，或系统终止）。
    Ended(Outcome),
}

impl SessionDrive {
    /// 这次驱动是不是停在等待。
    pub fn is_unsettled(&self) -> bool {
        matches!(self, SessionDrive::Unsettled)
    }

    /// 活动收场了就是那个结局；停在等待就是 `None`——**不**编一个结局出来。
    pub fn outcome(&self) -> Option<Outcome> {
        match self {
            SessionDrive::Ended(outcome) => Some(*outcome),
            SessionDrive::Unsettled => None,
        }
    }
}

/// 这次活动**怎么被发起的**：起源是哪一种来源，锚是主体的处境。
///
/// 两样都是**宿主的声明**，因此放在一起给，而不是拆成两个参数或往 [`SessionRequest`] 里加字段：
///
/// - `origin` —— 起源的**词表值**（「谁叫主体醒的」，见 [`WakeOrigin`]）。写成同一句话就是在
///   事实系统里写错话；
/// - `since` —— **「新」的起点**：上一次投影是**什么时候去看的**（宿主记的账）。给「新」一个起点，
///   主体不带 `since` 去问就只能全量扫。宿主不知道就是 `None`（如实读作 `unknown`，**不编造**）。
///
/// 默认形状见 [`RunOrigin::manual`]：宿主手动发起、没有外部事件、也没有锚。
#[derive(Clone, Debug)]
pub struct RunOrigin<'a> {
    /// 起源的**词表值**。
    ///
    /// 它进两处，**一处存、一处渲染**：`run-start` 行存的是 [`WakeOrigin::as_str`] 那五个字；
    /// 开场注入的 `activity-origin` 块渲染的是 [`WakeOrigin::text`] 那句人话——
    /// 两边由同一个值派生，因此"存的那一格"与"主体读到的那个块"不可能说两件事。
    pub origin: WakeOrigin,
    /// 「新」的起点（`None` = 宿主也不知道）。
    pub since: Option<SystemTime>,
    /// 宿主读回来的**投影**：「自锚以来每间房有几条新的」（`None` = 没锚、或这一次没读到）。
    ///
    /// 它是**宿主**的读数（走 App 的只读投影入口），进的是**通知栏**那块呈现——
    /// 本层只搬运，不解析、不判断、不排序。**它不含任何正文**。
    pub notification: Option<&'a str>,
    /// **事件卡片的细胞**（Run 级内存，宿主写、本件读）。
    ///
    /// `None` = 这次装配没接细胞（离线夹具、恢复路径、不经门铃的入口）⇒ 永远不出卡片。
    /// 宿主在**跑着**的时候听到铃就拉一次 B1 并覆盖式并进这个细胞（见 `host_loop`）。
    pub cards: Option<EventCardsReport>,
}

impl<'a> RunOrigin<'a> {
    /// 既有形状：宿主手动发起、没有外部事件触发、没有锚。
    ///
    /// 它就是本类型加入之前的那一段行为——因此默认入口的行为一个字节都没变。
    pub fn manual() -> Self {
        Self {
            origin: WakeOrigin::Manual,
            since: None,
            notification: None,
            cards: None,
        }
    }
}

/// 会话规格里**可选的那一半**：宿主声明的那几格与一个可选接线格，**一个包**。
///
/// **哪一格进这里**：它有没有"未声明"分支。有 ⇒ 进这个包（[`Default`] 就是"全都没声明"）；
/// 没有（身份、活动、上限、输入、钟、窗口、屏幕读数）⇒ 留在 [`SessionRequest`] 的必填那一半。
///
/// **为什么是一个包**（2026-10-08 记账）：Rust 没有"可选字段"的写法，平铺的会话规格要求
/// **每一处构造点**把每一格逐个写全。实测（加 `model` 那一格）：**22 个文件**、其中
/// **25 处机械补 `None`**（生产 3 · 选项结构 2 · 测试 20）。收成一个包之后，加一格的落点是
/// **这个包自己 ＋ 真正设置它的那一两处**（生产入口各一处），二十处测试构造点一处不改。
///
/// ⚠ 它**不是**第二份权威：包里的值各有各的来路（凭据来自入口那一次解析、模型名与思考强度
/// 来自将要发请求的那个对象），包只回答"**这一次装配哪些格在场**"。
///
/// ⚠ 与 [`SessionRequest`] 的分工：调用方把包**按值**交给会话（包里的访问声明仍是它自己那份），
/// 会话**借用**它。
#[derive(Clone, Default)]
pub struct SessionDeclarations {
    /// 宿主给出的文本创建授权；`None` = 这一次活动不能新建文本。
    pub creation: Option<CreationGrant>,
    /// **这一次装配用的模型名**（2026-10-08）：开场那一行 `model` 的取值。
    ///
    /// `None` = 这一次装配**没有模型**（不思考的那一档入口）⇒ 那一块**整块不出场**。它是**宿主的
    /// 声明**，由调用方从模型配置门的裁决里拿——本层**不猜、不给默认值**。
    ///
    /// 为什么是 Run 级快照而不是每轮读数：模型是 **Run 的属性**——换模型的生效点是**下一次装配**
    /// （"连续地想"要求同一段清醒期内模型不变：思考续接原文跟着模型走），因此一段清醒期恰好一个
    /// 模型名。它随 `run-start` 经历入账 ⇒ "**哪一段是哪个模型想的**"查得出来。
    pub model: Option<String>,
    /// **这一次装配的思考强度**（2026-10-08）：开场那一行 `reasoning_effort` 的取值。
    ///
    /// `None` = **不发那个键**（由供应商与模型自己决定），与 `Some("none")`（**明确要求不思考**）
    /// 是两件事——因此那一格**不出场**，而不是写 `unknown`。
    ///
    /// 与 `model` 同一格、同一来源、同一理由：值从**将要发请求的那个对象**里取（`ModelChoice`），
    /// 本层不猜、不给默认值；换档的生效点也在**下一次装配**，因此一段清醒期只有一个值。
    /// 它随 `run-start` 经历入账（那一行的 `machine` 块）⇒ "**同一份历史里哪几段是哪一档想的**"
    /// 查得出来——混着不同档位的输出，账上要说得出差别。
    pub reasoning_effort: Option<String>,
    /// **这一次装配用的模型收不收得到媒体材料**（2026-10-17）。
    ///
    /// 与 `model` / `reasoning_effort` **同一格、同一来源、同一理由**：值从模型配置门的裁决里拿
    /// （`FileSource::vision`），本层**不猜、不给默认值**；换模型的生效点也在**下一次装配**。
    ///
    /// ⚠ **`false` 是 fail-closed 的那一档**（不声明就是不支持）⇒ 读侧那两件动词的**可读类型
    /// 清单**里没有媒体类：工具面因此不摆出「读得动图片」这句话。它是**宿主的声明**，
    /// 不是我们探测的。
    pub vision: bool,
    /// **用量 cell 的读侧**（适配层写、收场时读）：`None` = 这次装配没接用量。
    ///
    /// 与 `reading` 同款但**两格分食**：那格是"上下文有多满"（给状态行），这格是"这一次花了
    /// 多少量"（给收场那一行）。不接就是"这一格没有被记"，**不伪造**一个读数。
    pub usage: Option<UsageReport>,
    /// **im 的访问声明**（端点 + 凭据）：`None` = 这次装配里**没有** im app。
    ///
    /// 端点由宿主的命令行给出、凭据由环境变量给出，两样都必须显式（缺一半是入口拒绝，不是
    /// 静默降级）。没声明时 Agent 的能力面里就不出现 im——不出现假装能用却用不了的条目。
    pub im: Option<ImAccess>,
    /// **宿主声明的身份**（TASK_70）：core 里那个 id 与此世界的名字，开场那两行里的一行。
    ///
    /// `None` = 这一次**没有声明** ⇒ 身份行**跳过**（不注入空壳）。与 `im` 同款：成对的值由
    /// 宿主在命令行上给出（`--identity-agent` / `--identity-name`），**装配只搬运**。
    pub identity: Option<IdentityClaim>,
    /// **搜索厂商的访问声明**（端点 + 凭据）：`None` = 没声明凭据。
    ///
    /// 与 `im` 不同，**没声明不等于没有这个 App**：web 窗是出厂内置的（两条动词从第一轮起就在场），
    /// 缺的只是搜索那一条执行时需要的凭据——那时它如实失败 `credential-absent`（**没配**，不是被拒），
    /// 而取页那一条与厂商无关，照常可用。因此这份声明只影响**一条动词能不能执行**，不影响能力面。
    /// ⚠ 真正决定能力面的是下面那一格**出网档**（`web`）：凭据管"能不能执行"，档位管"在不在场"。
    pub search: Option<SearchAccess>,
    /// **出网档**（2026-10-13）：这台机器给不给它出网窗——`off` / `fetch` / `full`，默认 `full`。
    ///
    /// 判据一句话：**"主体能不能出网"是主人的政策**（与 `--terminal` 同一类事实），因此：
    /// - `off` ⇒ **不装配 web 窗**（主体的门里连这扇门都没有；不是"有门但打不开"）；
    /// - `fetch` ⇒ 装配，但**「查」不进声明面**（主体知道地址才取得到）；
    /// - `full` ⇒ 今天的样子（两条都在场）。
    ///
    /// 它是**装配期声明**（不持久化）：生命周期 = 这一个进程；改档的生效点 = **下一次装配**
    /// （与 `model` / `terminal` 同款）。默认值取 `full` 的理由：**不改变现有行为**——而"花搜索
    /// 那份钱"这条风险已经由凭据在场与否把住了（没凭据 ⇒ 一次请求都不发）。
    pub web: crate::web_app::WebTier,
    /// **terminal 窗的档位声明**：`None` = 这次装配里**没有** terminal app（动词不存在）。
    ///
    /// 档位就是授权本体：宿主给出 `--terminal full` 才有这一份声明，给出即"这台机器随主体用"。
    /// 没有它时 Agent 的能力面里一个字节都不出现——不是"在但不可用"。
    pub terminal: Option<TerminalAccess>,
    /// **外部来源的 App 声明**（MCP server）：空 = 这次装配里一台都没装。
    ///
    /// 与 terminal 的差别只有一处，而那一处就是本刀的全部要点：**它们在装配期没有动词清单**
    /// （那要真的连上对面才拿得到）。因此它们进来时"只认得名字"，**展开那一下**才把动词面
    /// 交出来——装了不用就一分钱不花。授权面是**装配期事实**：换授权面要重启宿主。
    pub mcp: Vec<McpAccess>,
}

/// 打开一次会话需要的声明：**必填的那一半 ＋ 一个声明包**。
///
/// 它与 [`EnvironmentActivity`] 同源，但**不**包含上下文来源与提交接缝：那两样属于会话本身
/// （来源是会话自己的宿主输入接缝，提交接缝写进会话持有的那个环境）。把它们再交给调用方配置，
/// 就会出现"同一个会话两个来源"这种自相矛盾的形状。
///
/// 可选的那些格全在 [`SessionDeclarations`] 里（一个包，见那里的理由）。
pub struct SessionRequest<'a> {
    /// 调用方声明的身份。与环境里记录的权威身份不一致就是拒绝启动。
    pub declared_agent: &'a AgentId,
    /// 这次活动的标识。由调用方生成，整个会话不变。
    pub run: RunId,
    /// 这次活动的资源边界。整个会话共用一份，**不**在再次驱动时重置。
    pub limits: Limits,
    /// 这次活动的初始输入。它只在这里确定一次，不被后来的宿主输入覆盖。
    pub input: RunInput,
    /// **这次装配用的时间来源**：状态栏每一轮现取一次"现在几点"。
    ///
    /// 它由调用方在构造时给出（生产装配给 [`ClockHandle::system`]，离线夹具给固定钟），因此
    /// "同一时刻同一输入 ⇒ 逐字节相同输出"这条判据可判错，而格式化只有一份实现。它不是可配置
    /// 格式，也不是"要不要状态栏"的开关：状态栏一律装配。
    pub clock: ClockHandle,
    /// **宿主声明的上下文窗口**（状态行 `context` 段的分母）。
    ///
    /// 厂商不回传窗口，因此它只能由宿主声明——**没有"未声明"分支**：判据 10（主体看不见的墙等于
    /// 没有）由"必声明"兜底，状态行永远有 `context` 段。生产入口缺省填适配层的公开标称值，宿主
    /// 可以覆盖；`0` 不是合法声明（入口层直接拒绝，走不到这里）。
    pub context_window: u32,
    /// **屏幕报告 cell 的读侧**：适配层写、状态栏读（见 [`ContextReading`]）。
    ///
    /// 它只暂存"最近一次厂商报告的占用读数"，不落盘、不参与判断；恢复时**不从历史读回**——
    /// 上一次 Run 的读数对这一次会话没有意义，首轮如实读作 `unknown/窗口`。
    pub reading: ContextReading,
    /// **宿主声明的可选那几格**（一个包，见 [`SessionDeclarations`]）。
    pub declarations: SessionDeclarations,
}

/// 会话自己的上下文来源。
///
/// 它由**多个真实来源**合成，形状是一个**委托链**：外层是状态栏（[`StatusBar`]：时间 + 轮次读数，
/// 每一轮重摆），最内层是既有的手动环境来源（"这一次活动由宿主手动发起"）。每一层的批次都递交
/// （[`combine`]），内层先、外层后。没有注册表、没有名单、没有排序规则——每一层仍然只是
/// "外层持内层"。
///
/// **处境链的类型**：由外到内——状态栏（活状态）→ 通知栏（快照）→ 组件区（**交接 ＋ 纸条板**
/// 两块）→ **动作 pin** → 技能桌面 → 事件卡片 → **持有物**（自己约下的时刻）→ **机器处境**（这一段在
/// 什么样的执行体里想：平台 + shell ＋ **可选的第三格**"由谁想"，2026-10-08）→ **主体是谁**
/// （自识 ＋ 身份行，TASK_70）→ **上一段怎么了**（上一次活动有没有正常收场）→ 手动环境（起源那句）。
///
/// ⚠️ **层数**：十一层（硬上限见 `tests/minimal_agent.rs` 的
/// `the_opening_view_chain_has_a_hard_ceiling`）。交接**不是一层**——它是组件区那一格里的
/// 第二块（词表 §9「组件区 = pin 投影 ＋ 交接快照」，与状态栏那一层摆两块同款）。
///
/// 给它一个名字是因为它已经长到一行写不下（TASK_68 P3 加了一层，TASK_70 又加了一层，
/// 持有物那一刀再加一层，机器处境那一刀再加一层），而且这个名字就是"这一层由什么组成"的
/// **唯一权威说明**。装配只有一处（[`situation_chain`]），因此这份说明与代码不会再分家。
///
/// ⚠️ 内几层（起源 ＋ 上一段 ＋ 主体是谁 ＋ 机器处境）与**持有物**紧贴：批次是**内层先、外层后**
/// 递交，因此主体开场读到的顺序是「起源那句 → **上一段怎么了** → **主体是谁** → **这台机器是
/// 什么** → **我等着什么** → 技能桌面 → 事件卡片 → 钉住的动作 → **组件区（交接 → 纸条板）** →
/// 通知栏 → 状态行」——先知道"这次为什么醒"，再知道"我是谁、我在哪台机器上"，然后立刻知道自己
/// 安排过什么。
type SituationChain = StatusBar<
    NotificationBar<
        PinsBoard<
            PinnedActions<
                SkillsDesktop<
                    EventCards<
                        Holdings<Machine<SelfConceptBoard<PreviousActivity<ActivityOrigin>>>>,
                    >,
                >,
            >,
        >,
    >,
>;

/// 开场素材：处境链与开场注入要用到的那几样，**读法只有一份**（[`read_opening_materials`]）。
///
/// 从前两处装配点（新建 / 恢复）各自读五遍，并且靠注释要求"两处必须对齐"。现在读法与链的形状
/// 各只有一个权威位置，对齐不再是纪律问题。
struct OpeningMaterials {
    /// 主体贴在墙上的纸条（读失败 ⇒ `None` ⇒ 如实渲染 `unknown`，不冒充"墙上没有"）。
    pinned: Option<Vec<crate::pins_board::PinnedRow>>,
    /// 主体钉住的**动作详情**（自持物、跨 Run；`None` = 读不出来，不是"没钉"）。
    pinned_actions: Option<Vec<String>>,
    /// 技能桌面的一行读数（`None` = 材料区读不出来 ⇒ 那一块整块不出场）。
    skills: Option<usize>,
    /// 主体自己约下的时刻（**持有物**；读失败 ⇒ `None` ⇒ 如实渲染 `unknown`，不冒充"没安排过"）。
    holdings: Option<Vec<crate::holdings::HoldingRow>>,
    /// 自识（三态：缺席 / 在 / 读不出来）。
    self_concept: crate::self_concept::SelfConcept,
    /// **交接**（三态：缺席 / 在 / 读不出来）——上一次活动写给这一次开场的自己那一篇。
    ///
    /// ⚠ **它与自识不是一回事**（虽然形状同款）：自识回答「我是谁」（常驻），交接回答
    /// 「这一段我干到哪、下一段接着做什么」（**有确定的交付时刻**：紧邻的下一次开场）。
    handoff: crate::handoff::Handoff,
    /// **这一份交接已经交付过了吗**（`true` ⇒ 开场一个字都不说）。
    ///
    /// 判据见 [`handoff_delivered`]：**交付过 ⟺ 紧邻上一次活动的开场里摆过它，而且那一段真的想过**
    /// ——三格都是账里已有的事实，**零新事实、零新写者**。
    handoff_delivered: bool,
    /// 出厂说明（四态）。**不进处境链**——它是系统消息的静态头，由会话自己携带。
    orientation: crate::orientation::Orientation,
    /// **开场纸条要说的两条事实**：有一段没有正常收场、以及历史上最后一次存储故障。
    ///
    /// 它们由装配点在读**开场读数**时顺手带回来（`store::OpeningReadings`），**不新开读取点**：
    /// 那一读本来就在那儿（准入），现在**一次解析**同时带回这两格；从前它的去向是"拒绝"，
    /// 后来是"告诉它"，2026-10-06 起还包括"我上次为什么停"。
    previous: PreviousReading,
}

/// 处境的**活状态**：每次装配各不相同的那几格。
///
/// 它就是两处装配点之间的**全部差异**——从前那些差异表现为两段手写嵌套里的三个字面量，
/// 现在由 `None` 直接表达。
struct SituationLive<'a> {
    /// 「新」的起点（`None` = 不知道，如实读作 `unknown`，不编造）。
    since: Option<SystemTime>,
    /// 世界那一块投影（`None` = 没锚、或这一次没读到）。**不含正文**。
    notification: Option<&'a str>,
    /// 本次 Run 的事件卡片细胞（`None` = 这次装配没接细胞）。
    cards: Option<EventCardsReport>,
    /// 起源那句人话。由 [`WakeOrigin::text`] 派生（不在盘上存一句话）。
    origin: &'static str,
    /// 持有物年龄读数用的**装配那一轮**（unix 毫秒）。
    ///
    /// ⚠️ 它由**调用方**给，[`situation_chain`] **不自己再读一次钟**：新建路径复用它已经取过的
    /// `clock_at_open`（算时长用的那一次），因此"装配多读一次钟"这件事不会发生——递增钟的用例
    /// 因此仍能逐拍断言（`tests/status_bar.rs`）。
    now_millis: Option<u64>,
}

/// 把存储里的**起源词表值**翻成"这一次活动怎么被发起的"那一句人话。
///
/// **翻译发生在装配这一层，而且只在这里**：词表值归存储（`store::WakeOrigin`），那一句人话归
/// 显示区（`crate::activity_origin`）。两边互不认识——存储不再伸手去够显示区，显示区也不认识
/// 那张词表——由装配这一层把它们接起来。这正是装配层的本分。
fn origin_text(origin: WakeOrigin) -> &'static str {
    match origin {
        // **通道那一档的两态**（2026-10-13）：接上 vs 报变化——两件事两句话
        // （从前共用一句，而那句把"可能有新的"写成了"有新事件"）。
        WakeOrigin::Opened => crate::activity_origin::WAKE_OPENED_ORIGIN_TEXT,
        WakeOrigin::Changed => crate::activity_origin::WAKE_CHANGED_ORIGIN_TEXT,
        // 旧账（照读不写）：**不替它认**是哪一种，如实说"这一条没说是哪一种"。
        WakeOrigin::Bell => crate::activity_origin::WAKE_LEGACY_ORIGIN_TEXT,
        WakeOrigin::Timer => crate::activity_origin::TIMER_ORIGIN_TEXT,
        WakeOrigin::Manual => crate::activity_origin::MANUAL_ORIGIN_TEXT,
    }
}

/// 读一次开场素材。**两处装配点都调它**（新建 / 恢复），因此读法只有一份。
///
/// 五处的三态 / 四态语义各自保持：读失败与"没有"分得开（见各字段文档），这里一个都不压平。
fn read_opening_materials(
    environment_root: &Path,
    material_root: &Path,
    previous: PreviousReading,
    last_opening: &[(String, String)],
    last_activity_thought: bool,
) -> OpeningMaterials {
    // **交接**：读一次，交付判据用同一份（判据见 [`handoff_delivered`]）。
    let handoff = crate::handoff::read(environment_root);
    let handoff_delivered = handoff_delivered(&handoff, last_opening, last_activity_thought);
    OpeningMaterials {
        // **纸条板**：素材由**便签 App 自己交**（`pinned_material`，方向 `app → 显示区`）。
        // 装配这一层不翻它的存储、不认识它的记录类型。
        pinned: crate::notes_app::pinned_material(environment_root).ok(),
        pinned_actions: crate::pinned_actions::pinned_actions(environment_root).ok(),
        skills: read_skills(material_root),
        // **持有物**：主体自己约下的时刻。它与 `pinned` / `pinned_actions` 同款——读的是
        // 自己的地盘，因此不需要任何授权，也不需要宿主参与；`None` = 读不出来（不是"没约过"）。
        //
        // ⚠️ 素材由**时钟 App 自己交**（`holdings_material`，方向 `app → 显示区`：上层依赖下层，
        // 手机模型 §3 第 3 条"app 往显示区交素材"）。装配这一层**不翻它的账本、不认识它的记录
        // 类型**——它只把 App 交上来的那一份递给显示区。
        //
        // ⚠️ 这一读发生在宿主 `consume_due` **之后**（宿主那一步把到点的从 `pending` 挪到
        // `fired`），因此开场摆出来的表里，"刚响过的"已经标成 `fired`、"过期还没响成的"
        // 照旧标成 `pending` + `overdue`——**两种都能看见**，没有第三种（静默消失）的形状。
        holdings: merge_holdings(
            crate::timer_app::TimerApp::new(environment_root)
                .holdings_material()
                .ok(),
            TerminalApp::held_material(environment_root).ok(),
        ),
        self_concept: crate::self_concept::read(environment_root),
        handoff,
        handoff_delivered,
        orientation: crate::orientation::read(environment_root),
        // **上一段有没有收场**：由装配点在读准入时顺手带回（不在这里再读一次）。
        previous,
    }
}

/// **这一份交接交付过没有**：判据一句话——**交付过 ⟺ 紧邻上一次活动的开场里摆过它，而且那一段真的想过**。
///
/// 两格都是**账里已经有的事实**（零新事实、零新游标、零新写者）：
///
/// - 这一份的正文 = 装配期从 `<环境根>/handoff` 读回来的那一态（[`crate::handoff::read`]）；
/// - 上一次开场摆过什么 = 装配点在读**开场读数**时顺手带回来的那一批块
///   （`store::OpeningReadings::last_opening_blocks`，即那一条 `run-start` 注入的
///   `blocks[].text`）——**不新开读取点**；
/// - **那一段想过没有** = 同一份读数里的 `last_activity_thought`（账里有 `model-response`）。
///
/// ⚠ **按身份比，不按"上一次摆过一块交接没有"**：交接是能被重写的（B4.1「再写 → 新 handoff
/// 成为最新（下次开场交付新的）」）——只看"摆过一块"会把**重写后的新的一份**读成"已经交付过"
/// ⇒ 那一份**永远不摆**。改过一个字节就是新的一份，因此这里比的是**正文**。
///
/// ⚠ **"想过"那一格守的是方向**（B4.2：**丢失不可接受、重复可容忍**）：注入落账与第一次模型调用
/// 之间有一个窄窗口——**注入已落账、而这一段一个回合都没想成**（进程没了，或那一次调用失败收场）
/// ⇒ **主体没拿到**那一份。少了这一格，判据会在那个窗口里读作"交付过"、把那一份**沉底**（丢失）；
/// 有了它，那个窗口落到**重播**那一侧（多看一遍，非语义错误）。
///
/// ⚠ **看不见 ⇒ 读作没交付过**：读不出来、上一次活动的开场落在封口的上一卷里（活动段封口
/// 1 MiB 一次）、上一次那一块没给出去——一律算"没交付过" ⇒ 至多**重播一次**。**绝不静默跳过**。
fn handoff_delivered(
    handoff: &crate::handoff::Handoff,
    last_opening: &[(String, String)],
    last_activity_thought: bool,
) -> bool {
    // 三态里只有"在"才谈得上交付：缺席（没写过）与读不出来都不是"交付过"。
    let crate::handoff::Handoff::Present(text) = handoff else {
        return false;
    };
    // 上一次没想成 ⇒ 那一份没进过它的视野 ⇒ 不算交付过（重播，不沉底）。
    if !last_activity_thought {
        return false;
    }
    last_opening
        .iter()
        .any(|(label, seen)| label == crate::handoff::HANDOFF_LABEL && seen == text)
}

/// **把持有物的两支成员合成一份**（2026-10-11：第二支 = 主体自己起的东西）。
///
/// 方向照旧是 `app → 显示区`：装配这一层**只转手**，不认识任何 App 的记录类型——它只把两支
/// 交上来的素材接在一起（与它把 `holdings_material` 递给显示区是同一件事）。
///
/// ⚠ **有一半读不到 ⇒ 整块如实说 `unknown`**：不拿读到的那一半冒充全部（那是把"不知道"说成
/// "就这些"）。两支都读成了、一条也没有 ⇒ `Some(vec![])`（"空"与"没读到"照旧是两件事）。
fn merge_holdings(
    timers: Option<Vec<crate::holdings::HoldingRow>>,
    held: Option<Vec<crate::holdings::HoldingRow>>,
) -> Option<Vec<crate::holdings::HoldingRow>> {
    let (Some(mut timers), Some(held)) = (timers, held) else {
        return None;
    };
    timers.extend(held);
    Some(timers)
}

/// **最近那一段**的两格读数（**同一份 `inspect`** 出来的，不读第二遍）。
///
/// 两格是独立的事，因此各有各的缺席表达：清单读不出来 ⇒ `unsettled_unreadable` 带稳定标记
/// （**不冒充"没有"**）；那一段是不是主体搁下的 ⇒ `held`（读不出来时缺席，而同一块已经有一行
/// 如实说"读不出来"——不冒充"正常收场"）。
struct LatestSegment {
    /// 未结算的操作（已发出、没有结果记录）。
    unsettled: Vec<UnsettledOperation>,
    /// 上面那份清单读不出来时的稳定标记；`None` = 读出来了。
    unsettled_unreadable: Option<String>,
    /// **这一段是主体自己搁下的**（`subject:held`）⇒ `Some(那一段)`；别的形状都是 `None`。
    held: Option<RunId>,
    /// **这一段后来断了，而它是主体自己停在这儿的**（2026-10-13，读经历侧读出来的）。
    ///
    /// `None` = 这一格不适用（那一段有收场事实、或根本没有那一段）⇒ **不读账**（读一份账是线性
    /// 开销，正常路径上一分钱都不该付）。`Some(..)` 只在"没有正常收场"这一档去读。
    stopped: Option<crate::recovery::StoppedInPlace>,
}

/// **最近那一段里未结算的操作 ＋ 它是不是主体搁下的**（给开场纸条用；2026-10-06 立，2026-10-10 加第二格）。
///
/// ## 它做什么、不做什么
///
/// **不复制判据**：哪几件没结算、以及"这一段有没有收尾事实"，都由**既有的单活动解释器**
/// （[`recovery::inspect`]）给。本函数只做两件它不做的事：
///
/// 1. **把配对键换成"当时选的是哪个能力"** —— 能力名**只在经历侧**（`action-selected`），
///    事实侧只有 `(run, turn, index)` 三元组，因此这里按那个三元组**配对两份存储**；
/// 2. **取那一刻的时刻**（`at-ms`，2026-10-06 起每条记录都带；旧记录读作 `None`）。
///
/// ## ⚠ 「读不到」与「没有」分开（本仓库一贯那条）
///
/// 读不出来 ⇒ 第二个返回值是 `Some(稳定标记)`，由渲染点如实说"读不到"；**确实没有** ⇒ `None`。
/// 两者在纸上长得完全不一样，**不许压成一档**。
///
/// ## ⚠ 「主体搁下的」与「没有正常收场」也分开（2026-10-10 裁决三）
///
/// 两者在账上长得不一样：前者有收尾事实（`subject:held`），后者**根本没有收尾事实**。
/// 而下一段只能看到纸条 ⇒ 把"主体自己搁下的"读成"断了"就是**拿假话吓人**。
///
/// ## ⚠ 它【不】产出 `target`
///
/// 终局五格里的 `target` **本件不做**，判据是主体性：**「哪个参数算目标」是解释，解释归主体**。
/// 系统给机械事实 ＋ 原文在哪（`history/` 里那次选择的参数**逐条原样**）。
/// 参数**一个都不进开场**——不是因为"私密"，而是因为**原文主体已经能看到，在开场再摊一遍
/// 不是"给它看"，是"替它挑"**。
fn latest_segment_of(environment: &Environment, run: &RunId) -> LatestSegment {
    let root = environment.root().to_path_buf();
    // 读者是**只读**那条路（不取写资格）：写资格此刻在 `environment` 手里。
    let reader = match EnvironmentReader::open(&root, environment.agent().clone()) {
        Ok(reader) => reader,
        Err(error) => {
            return LatestSegment {
                unsettled: Vec::new(),
                unsettled_unreadable: Some(error.as_str().to_string()),
                held: None,
                stopped: None,
            };
        }
    };
    let state = recovery::inspect(&reader, run);
    // **主体搁下的**：有收尾事实、而且收尾是主体那一侧的 `Held`。
    let held = match &state {
        RunState::Ended {
            outcome: Outcome::Subject(SubjectEnding::Held),
            ..
        } => Some(run.clone()),
        _ => None,
    };
    // **这一段后来断了，而它是主体自己停在这儿的**（2026-10-13）：**只在"没有收场"这一档去读账**
    // ——那一档正是纸条要说"没有正常收场"的地方，而真相可能是"主体先停下来了，然后进程没了"。
    // 别的状态各有各的表达（有收场事实的走 `held`；从来没跑过的没有这一格）⇒ **一次都不多读**。
    let stopped = match &state {
        RunState::Unfinished => Some(crate::recovery::stopped_in_place(&reader, run)),
        _ => None,
    };
    let operations = match state {
        RunState::Unknown { operations, .. } => operations,
        // ⚠ **结束了的段也可以带着未确认的效果**（2026-10-10 裁决）：那几件照样要摆出来。
        // 判据在 `recovery` 那一处（`dispatched_unconfirmed`），本件**不复制**它。
        RunState::Ended { unconfirmed, .. } => unconfirmed,
        // 别的状态**没有"未结算的操作"这一格可报**（`Unfinished` = 停在半路但每一件都有结果；
        // `Blocked` / `Refused` 同理）⇒ **空，而不是"读不到"**。
        _ => {
            return LatestSegment {
                unsettled: Vec::new(),
                unsettled_unreadable: None,
                held,
                stopped,
            };
        }
    };
    // 能力名与时刻**只在经历侧**：按 `(turn, index)` 配对——与 `action-selected` 的配对键同形。
    let page = match crate::store::read_run_at(&root, run, None, usize::MAX) {
        Ok(page) => page,
        Err(error) => {
            return LatestSegment {
                unsettled: Vec::new(),
                unsettled_unreadable: Some(error.as_str().to_string()),
                held,
                stopped,
            };
        }
    };
    let mut chosen: Vec<UnsettledOperation> = Vec::new();
    for record in page.records() {
        if let TracePayload::ActionSelected {
            turn,
            index,
            action,
            ..
        } = record.entry()
        {
            chosen.push(UnsettledOperation {
                turn: *turn,
                index: *index,
                capability: Some(action.clone()),
                at_millis: record.at_millis(),
            });
        }
    }
    // ⚠ **经历侧配对不上时不许编一个能力名**：如实给 `None`（渲染成 `unknown`），
    // 而那一件照样在清单里——"哪一件没结算"是事实侧给的，不依赖经历侧读得出来。
    let unsettled = operations
        .into_iter()
        .map(|operation| {
            let turn = operation.turn().number();
            let index = operation.index();
            chosen
                .iter()
                .find(|each| each.turn == turn && each.index == index)
                .cloned()
                .unwrap_or(UnsettledOperation {
                    turn,
                    index,
                    capability: None,
                    at_millis: None,
                })
        })
        .collect();
    LatestSegment {
        unsettled,
        unsettled_unreadable: None,
        held,
        stopped,
    }
}

/// 处境链的**唯一装配点**：层数与顺序只有这一份。
///
/// 两处调用点只差 [`SituationLive`] 那四格——恢复路径没有锚、没有投影、没有细胞，起源回到
/// "宿主手动发起"那一档（与从前的 `ActivityOrigin::new()` 逐字节相同）。
fn situation_chain(
    clock: ClockHandle,
    context_window: u32,
    reading: ContextReading,
    materials: &OpeningMaterials,
    declarations: &SessionDeclarations,
    live: SituationLive<'_>,
) -> SituationChain {
    // 持有物的年龄读数用**调用方给的装配那一轮**（见 [`SituationLive::now_millis`]）：
    // 本函数**不自己读钟**，因此"装配多读一次钟"不会发生。
    let now_millis = live.now_millis;
    StatusBar::new(
        clock,
        // **本机偏移：装配时现读一次**（与 `machine` 行那条"每次开场从机器现读"同款）。
        // 读不出来就是 `Unreadable` ⇒ `time` 那一格退回 `Z`（不冒充本地时间）。
        LocalOffset::of_machine(),
        context_window,
        reading,
        NotificationBar::new(
            live.notification,
            live.since,
            PinsBoard::new(
                materials.pinned.as_deref(),
                // 纸条板上那一格的年龄用**同一轮装配的钟**（与持有物同一条纪律，见上一行的注释）。
                now_millis,
                // **这一格的第二块：交接**（词表 §9「组件区 = pin 投影 ＋ 交接快照」）。
                // 它**不占一层**（C8 的硬上限 11 层一个字不动）：三态与"一次播放"由
                // `crate::handoff` 解释，摆在这一格的哪儿由 `pins_board` 决定。
                materials.handoff.clone(),
                materials.handoff_delivered,
                PinnedActions::new(
                    materials.pinned_actions.as_deref(),
                    SkillsDesktop::new(
                        materials.skills,
                        EventCards::new(
                            live.cards,
                            // **持有物紧贴"我是谁"外面一圈**：主体开场读到的顺序是
                            // 「起源那句 → 我是谁 → **我等着什么 / 我欠着什么** → 技能桌面 → …」。
                            // 对只有瞬间记忆的主体，这三样是开场的核心三连——他必须在自己安排的
                            // 事情**自己出现**之前，先知道"这次为什么醒"和"我是谁"。
                            Holdings::new(
                                materials.holdings.as_deref(),
                                now_millis,
                                // **机器处境紧贴"我是谁"外面一圈**：主体开场读到的顺序是
                                // 「我是谁 → **我在哪台机器上** → 我等着什么 → …」——身份与处境
                                // 是同一口气里的两件事（你是谁、你在哪），因此它们相邻。
                                Machine::new(
                                    declarations.model.clone(),
                                    declarations.reasoning_effort.clone(),
                                    SelfConceptBoard::new(
                                        materials.self_concept.clone(),
                                        declarations.identity.clone(),
                                        // **上一段怎么了**：紧贴起源那句——一块说"这次为什么醒"，
                                        // 一块说"上一段怎么结束的"。两块主语不同，因此不合并。
                                        PreviousActivity::new(
                                            ActivityOrigin::with_origin(live.origin),
                                            materials.previous.clone(),
                                        ),
                                    ),
                                ),
                            ),
                        ),
                    ),
                ),
            ),
        ),
    )
    .with_since(live.since)
}

/// 此外它还把宿主**显式交出**的一段文本呈现**一次**，之后就没了——不排队、不重放、也不覆盖
/// 这次活动的初始输入。空批次不产生任何记录（这条由 Core 的既有规则保证）。
struct HostSource {
    /// 处境链（组成见 [`SituationChain`]）。
    origin: SituationChain,
    /// 宿主的**桌面**（TASK_71 桌面 P0）：门之外那一组 + 每一扇 App 的门。
    ///
    /// 装配期取一次（[`MinimalOs::desktop`]，不读盘、不看展开状态）⇒ 首轮渲染目录时用它分组。
    desktop: Desktop,
    /// 宿主显式交出、还没被呈现的那一段文本。
    pending: Option<ContentBatch>,
}

impl ContextSource for HostSource {
    fn on_run_start(&mut self, context: &InjectionContext) -> ContentBatch {
        // 开场那一轮只有处境链：内层的起源事实 + 状态栏那条"刚开始、预算一点没花"的读数。
        self.origin.on_run_start(context)
    }

    fn before_turn(&mut self, context: &InjectionContext) -> ContentBatch {
        // `take`：一段宿主输入只被呈现一次。真正的"恰好一次"由这里兑现，而不是靠调用方自觉。
        // 恒摆的状态行与这段输入是**两处内容**，因此两处都递交。
        //
        // **参数顺序就是这个接缝的让位规则**：宿主显式交出的文本是 `other`（永不放弃），
        // 自己的读数（本轮的恒摆状态行）是 `own`（合并超出批次容量时让位——它下一轮还会重摆，
        // 而这段文本只有这一份）。**顺序反了就是静默吞掉宿主输入**：验收复核就是在这里
        // 用一条顶到上限的输入把它抓出来的（新增用例
        // `a_host_input_near_the_batch_limit_is_still_delivered`）。
        let base = match self.pending.take() {
            None => self.origin.before_turn(context),
            Some(input) => combine(input, self.origin.before_turn(context)),
        };
        // TASK_68 P2：**首轮带一份动作目录**（每个动作一行：标识 | 骨架）。
        //
        // 头部只剩入口之后，这是主体唯一一次自动知道自己有哪些动作；之后新展开的 App 由
        // **展开回执**自带（既有行为，不另做机制）。只在首轮发，是为了不让目录在长 Run 里
        // 每轮重复进历史。
        //
        // 让位规则沿用上面那条：目录是"自己的读数"（下一轮还会重摆），因此它是 `other`——
        // 合并超容量时让位给宿主输入与状态行，而不是把它们挤掉。
        match context.is_opening_turn() {
            true => combine(
                base,
                action_directory_batch(context.capabilities(), &self.desktop),
            ),
            false => base,
        }
    }
}

/// 目录里**一行动作**（既有格式）：`<标识> | <骨架>`，免查的再带一行参数表（两格缩进 = 它在门里）。
///
/// - 标识**就是**工具名（本 crate 不做名称映射表）；主体照着目录就能经入口调用它们；
/// - 骨架本来就是一行，这里再压一次（防它带换行、破坏"一行一个动作"的形状）；
/// - **参数表默认不进这里**：要走"先看格式"那条路的动作，看格式是自己的动作
///   （`inspect_action` 的回执）。**例外**只有一个，而且它是自洽性要求：
///   由声明方标了"**免查**"的那几条**自带参数名**（`名称 = 说明（必填）`）——
///   一个动作既然不走"先看格式"，调用前就没有别的地方能知道它的参数名；
///   目录若不给出，就等于要求对方凭空知道（TASK_69 真机四证的实测缺口）。
fn push_action_line(text: &mut String, capability: &Capability) {
    text.push_str("  ");
    text.push_str(capability.id().as_str());
    text.push_str(" | ");
    text.push_str(capability.skeleton().lines().next().unwrap_or("").trim());
    if capability.is_exempt_from_inspection() && !capability.params().is_empty() {
        text.push_str("（参数：");
        text.push_str(&capability.params_line());
        text.push('）');
    }
    text.push('\n');
}

/// **动作目录**（TASK_68 P2 立；**TASK_71 桌面 P0 改成分组**）：桌面的两层 ——
/// **门之外那一组 ＋ 每一扇 App 的门**。
///
/// - **门之外**那组 = OS 自有动作（不归任何 App：展开、看格式、钉/摘、写自识），组头写明这一点；
/// - **每扇门**一行门牌（`【<App 标识>】<App 自己那句话>`），门后是它**声明**的动词；
///   门内那几行**保持既有格式**（`<标识> | <骨架>`，只是缩进两格）——这样"照目录念一行就能调"
///   这条既有读法不变，读它也的人与断言都不用换读法；
/// - **门还没打开** ⇒ 门后只有一句提示，不把"未展开的动作"伪装成能调的（开门那个动作一开，
///   动作由**展开回执**自带，既有行为）；提示里要指名的那个动作，由 [`Desktop::opener`] 给出 ——
///   渲染者不自己认识导航记号。
///   ⚠ **"开没开"由 `AppDoor::preinstalled` 决定，不由"数动词"反推**（2026-10-08 改）：
///   目录只在首轮渲染，而首轮那一刻还没有任何展开 ⇒ 开着的**恰好**是常开的那几个（唯一判据在
///   `os.rs`）。"一个动词都没列出来"是**另一件事**——常开却零能力的 App 也一个都列不出来，
///   但它明明是开着的。⚠ 那个前提有守卫钉着（见 [`AppDoor`] 的文档）。
/// - 每扇门**都在榜上**，不管开没开 —— 这正是"桌面先摆出几个 App，进 App 才看到工具"。
///
/// 动作目录那一块的标签（呈现时的区域名；TASK_71 前是内联字面量，现在两处引用——块与超限读数）。
const ACTION_DIRECTORY_LABEL: &str = "available-actions";

/// 超限时**留下一句读数**（"目录超限未给"），而不是退成空批次：目录没了与"这一轮没摆"必须分得开
/// （与 [`ActivityOrigin`] 的 `on_run_start` 同款处置）。
fn action_directory_batch(capabilities: &Capabilities, desktop: &Desktop) -> ContentBatch {
    let mut text = String::new();
    text.push_str("【系统】不归任何 App 的动作\n");
    for id in &desktop.system {
        if let Some(capability) = capabilities.find(&ActionId::new(id.clone())) {
            push_action_line(&mut text, capability);
        }
    }
    for door in &desktop.doors {
        text.push_str(&format!("【{}】{}\n", door.id, door.description));
        for id in &door.verbs {
            if let Some(capability) = capabilities.find(&ActionId::new(id.clone())) {
                push_action_line(&mut text, capability);
            }
        }
        // **门后那句提示由"在不在场"决定，不由"数动词"反推**（2026-10-08）。
        // 唯一判据在 `os.rs`（`is_preinstalled || is_expanded`）；而这里只在首轮渲染，那时还没有任何
        // 展开 ⇒ `preinstalled` **就是**此刻的 `is_open`。见 [`AppDoor`] 的文档与钉住那条前提的守卫。
        if !door.preinstalled {
            match &desktop.opener {
                Some(opener) => text.push_str(&format!("  （还没打开 —— {opener} 可展开它）\n")),
                // 榜上连开门那个动作都没有（装配异常）⇒ 只说事实，不指名不存在的东西。
                None => text.push_str("  （还没打开）\n"),
            }
        }
    }
    ContentBatch::of_or_not_delivered(
        vec![ContentBlock::new(ACTION_DIRECTORY_LABEL, text)],
        ACTION_DIRECTORY_LABEL,
    )
}
/// 会话能不能再被驱动。
///
/// 三个状态是**互斥**的，而且都不是从别处推断出来的：`Ready` 是唯一可以被驱动的状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionPhase {
    /// 还能被驱动：活动已经开始、没有收场，也没有以失败告终。
    Ready,
    /// 已经收场。
    Ended,
    /// 上一次驱动以失败告终。失败**不**降级成等待，也不允许被再驱动一次抹掉。
    Failed,
}

/// 一次同进程会话：**装配一次、驱动多次**。
///
/// ## 所有权拓扑
///
/// 会话持有这一轮执行需要的**全部**可变状态，因此"同一个活动被驱动多次"不需要任何一侧
/// 重新装配：`Agent`（活动槽）、`Run`（轨迹、轮次、预算、登记标记）、`Ledger`（事实）、
/// `MinimalOs`（App 与能力）、`RunExpansion`（展开状态，**自有值**，不是每次重建的空集合）、
/// 以及宿主输入接缝。每次驱动只借用其中的一部分，展开状态因此可以跨驱动保留，
/// 而不需要 `unsafe`、自引用或每次把展开集合清空。
///
/// ## 谁保证写资格不被提前释放
///
/// 会话持有 `&'a mut Environment`：**借用的生命周期就是会话的生命周期**。因此在会话活着的时候，
/// 调用方既不可能把写锁提前放掉，也不可能把一个别的环境接进这个会话——两者都由借用检查器
/// 保证，而不是靠约定。会话结束时（无论正常收场、失败还是被丢弃）借用才归还，写锁才可能被释放。
///
/// 每次驱动前仍然会重新核对一次绑定（身份与目录），把"这个会话绑定的是这个环境"这条不变量
/// 摆在驱动入口上：在当前设计下它由借用保证、不可违反，这个检查因此是**不变量的再确认**，
/// 不是运行期防御。
///
/// ## 被丢弃意味着什么
///
/// 会话被丢弃**不**伪造任何收场：不写终态、不封存、不替主体宣布完成。环境里留下的是一个
/// "没有收尾的活动"——而那**不是**一条拦住下一次启动的门（2026-09-26：进程死就是那一段断了，
/// 新进程起来是新的一段）；"上一段没有收尾"由**新的一段的开场那一轮**如实带给主体。
pub struct RunSession<'a> {
    /// 装配件：整个会话只装配一次。
    os: MinimalOs,
    /// 展开状态是自有值，与这一次活动绑定；跨驱动保留，不重建、不清空。
    expansion: RunExpansion,
    /// 会话状态。
    agent: Agent,
    run: Run,
    ledger: Ledger,
    /// 环境写资格：覆盖整个会话（见类型文档）。
    environment: &'a mut Environment,
    /// 绑定时记下来的身份与目录，每次驱动前核对。
    agent_id: AgentId,
    environment_root: PathBuf,
    /// 宿主输入接缝。
    source: HostSource,
    /// **这一次活动的唤醒缘由**（宿主的声明，见 [`RunOrigin::origin`]）。
    ///
    /// 它是**会话级**的一格：在一次活动开始那一刻随 `run-start` 经历一起落进 trace
    /// （经 [`EnvironmentCommit::with_origin`]），此后不再改。每一次会话都是**新的一段**，
    /// 因此这一格**必然在场**——`Some(...)` 由会话装配层补上（`EnvironmentCommit::new` 那条
    /// 一次性入口没有起源可声明，它仍然给 `None`）。
    ///
    /// 存的是**词表值**；开场注入那一块渲染的人话由同一个值当场派生（见 [`WakeOrigin::text`]）。
    declared_origin: WakeOrigin,
    /// **这次的钟**（[`SessionRequest::clock`] 那一只的副本）。
    ///
    /// 状态栏那一边拿走了原份，这里留一份只为收场时读"现在几点"算时长——同一个来源，
    /// 因此状态行报的时刻与时长量的是同一个钟，不会一个走系统钟一个走夹具钟。
    clock: ClockHandle,
    /// **这一次活动的起点**（会话打开那一刻，从装配注入的钟现取）。
    ///
    /// 它只服务一件事：收场时算出**时长**。起点取在**会话打开那一刻**（[`RunSession::open`] /
    /// [`RunSession::open_declared`] 里现取一次），而不是第一次驱动那一刻——宿主建立会话之后
    /// 立刻驱动，两者之间只有装配本身那点时间；而"这一次活动本身有多久"正是要含住这段的
    /// （它是这次活动的一部分）。⚠️ 这道读**不**与 `run-start` 那一行严格同拍：登记发生在
    /// 第一次驱动里，起点比它早一点点。差的是装配那一点时间，量级可忽略，但**不声称同源**。
    ///
    /// 它**必然在场**：会话只能从「这一次活动就是这一段」开始（`RunSession::open` /
    /// `open_declared`），起点因此总在这一侧手里。旧规矩里那一格"恢复路径不记时长"随
    /// **跨进程恢复**一起退役了（2026-09-26：进程死就是那一段断了，重启开的是新的一段，
    /// 它有自己的起点）——于是这一格不再需要 `Option`。
    clock_at_open: SystemTime,
    /// **这一次活动的用量读数**（适配层写、这里读）。
    ///
    /// `None` = 这次装配没接用量 cell（离线夹具）⇒ 用量那一格不写。
    usage: Option<UsageReport>,
    phase: SessionPhase,
    /// **上一次驱动里发生过的模型重试**（TASK_60 裁决 5）。
    ///
    /// 为什么记在会话上、而不是让库打印：诊断面要进的是**宿主日志**（宿主那个进程的 stderr），
    /// 而库打印正是 TASK_51 删过的那种"侧信道"形状。⇒ 记录**交给调用方**，由它打。
    /// 它是**诊断**不是事实：不进 `facts.ndjson`、不参与任何判断。
    model_retries: Vec<ModelRetry>,
    /// **环境根里的出厂说明**（TASK_79）：装配期（[`RunSession::open`] / [`RunSession::open_declared`]）
    /// **拍读一次**，此后 Run 内携带——每一次驱动都把它重新装订给模型
    /// （[`crate::core::ModelPort::declare_static_head`]）。
    ///
    /// 为什么读在装配、装订在驱动：读盘是**装配期事实**（与自识、纸条板、技能桌面同款——两处
    /// 装配点都读），而"模型端要看得见"这件事只有在驱动那一刻才成立。两者之间**不重读**
    /// ⇒ 主体在 Run 里改这个文件**不影响本次的静态头**，下一次活动的开场才变。
    ///
    /// ⚠️ **"Run 内不变"的边界是进程**（2026-09-23 刀五定为**故意**，不是偶然）：新的一段
    /// （新进程、新的 [`RunSession::open`]）会**重新拍读**一次。理由是这一份是**环境家具**
    /// （环境根里的一份文件），不是 Run 状态——Run 状态的家在已确认记录里，而**家具的家在
    /// 环境根**。同族的先例是 `reading`：新进程不知道上一个进程拍到了什么，于是照当次的真实
    /// 事实重新拍。判据钉在 `tests/orientation.rs` 的"新的一段会重新拍读"那一条上。
    ///
    /// **每一次装订都留一条证**（[`EnvironmentCommit::commit_static_head`]）：重新拍读照旧，
    /// 但"那一刻装订的是哪一份"从此也在记录里。家具的家还在环境根，**历史**的家在记录里——
    /// 两个事实、两个家，谁也不冒充谁。
    orientation: crate::orientation::Orientation,
    /// **这一次进程有没有已经给静态头留过证**（见 [`EnvironmentCommit::commit_static_head`]）。
    ///
    /// 一个会话就是一次进程：装订**一次进程一次**——开场一次，此后每一个新进程重新拍读一次。
    /// 每驱动一次都写一条会把"当时看见的是哪一份"淹掉，也会让轨迹无谓地长。
    static_head_recorded: bool,
}

impl<'a> RunSession<'a> {
    /// 打开一次会话：核对身份与准入、装配执行对象、登记活动——但**不**驱动。
    ///
    /// 顺序有意与一次性入口一致：身份核对与执行准入排在**建立任何执行对象之前**，资料根装配
    /// 排在第一次模型调用之前。第一次驱动由调用方显式发起（[`RunSession::drive`]），因此
    /// "什么时候真的调用模型"是调用方的事实，不是装配的副作用。
    pub fn open(
        material_root: &Path,
        environment: &'a mut Environment,
        request: SessionRequest<'_>,
    ) -> Result<RunSession<'a>, CompositionError> {
        // **委托**，不是第二份执行链：下面那一个函数是唯一一条装配路径。
        Self::open_declared(material_root, environment, RunOrigin::manual(), request)
    }

    /// 与 [`RunSession::open`] 相同，只是**这次活动怎么被发起的，由调用方声明**（见 [`RunOrigin`]）。
    ///
    /// 声明有两样：起源那句（谁把主体叫醒的）与锚（「新」的起点 = 上一次去看的时刻）。两者都是
    /// 宿主的账：门铃发起与手动发起不是同一件事，把起源写成同一句话就是在事实系统里写错话；
    /// 而「新」的起点只有宿主知道（新进程的第一个 Run 就是不知道）。
    /// 默认入口仍用 `RunOrigin::manual()`，因此既有调用点与既有用例**一行不改**。
    pub fn open_declared(
        material_root: &Path,
        environment: &'a mut Environment,
        declared: RunOrigin<'_>,
        request: SessionRequest<'_>,
    ) -> Result<RunSession<'a>, CompositionError> {
        let SessionRequest {
            declared_agent,
            run: run_id,
            limits,
            input,
            clock,
            context_window,
            reading,
            declarations,
        } = request;

        // **这次活动的起点**：取在装配这一轮，用的是装配声明的那只钟（不是系统钟）——
        // 夹具的固定钟因此照样给得出确定的时长，而生产读的就是真时刻。
        let clock_at_open = clock.now();

        // **把钟交给环境**（2026-10-06）：此后**每一条记录**都带 `at-ms`（「它是什么时候写的」）。
        //
        // 为什么在这里：钟是**装配声明的一部分**（[`SessionRequest::clock`]），而装配是唯一一条
        // 把"谁在跑这个进程"讲清楚的地方。生产给系统钟（真时刻），夹具给固定钟（**确定值**，
        // 因此那些用例的 `at-ms` 可以照常断言）。**每一次会话都装** ⇒ **没有"忘了装"这一档**，
        // 也就没有"记录悄悄少了时刻"这种静默降级。
        //
        // 它**不改任何既有判据的形状**：直接走 `Environment::append_trace` 的那些路径
        // （不经装配）仍然没有钟，写出来的字节与从前逐字节相同。
        environment.set_clock(clock.clone());

        // 环境与调用者声明的身份必须一致：不一致就是拒绝启动，不"就近采用"。
        if environment.agent() != declared_agent {
            return Err(CompositionError::Environment(
                crate::store::StoreError::IdentityMismatch {
                    expected: declared_agent.clone(),
                    found: environment.agent().clone(),
                }
                .as_str()
                .to_string(),
            ));
        }

        // 执行准入：只依据**已确认的耐久记录**判断，不默认健康。
        //
        // ⚠ **2026-09-26 改**：`Unfinished` **不再拒绝**——与一次性入口同一句理由（见那里）：
        // 进程没了就是那一段断了，新进程起来是**新的一段**；"上一段没有收尾"由开场那一轮带给主体。
        // ⚠ **2026-10-06 改（主人裁决）**：`Paused` **也不再拒绝**——同一条理由（门是指令，
        // 而世界只该给信息）；这条事实由开场那一块纸条带给主体，能不能继续由主体看着事实自己定。
        // ⚠ **2026-10-06 加**：同一次读里还带回"历史上最后一次存储故障"（`OpeningReadings`，
        // 一次解析）——它从前只有操作者的命令读得到。
        let readings = environment
            .opening_readings()
            .map_err(|error| CompositionError::Environment(error.as_str().to_string()))?;
        let unfinished = match readings.admission {
            Admission::Ready | Admission::Paused { .. } => None,
            Admission::Unfinished { run } => Some(run),
        };
        // ⚠ "我上次为什么停"这一格与准入**无关**：正常收场（`Ready`）也可能刚经历过一次存储
        // 故障，而它照样该知道（两格各自缺席 ⇒ 各自不出现）。
        //
        // **未结算的操作**（2026-10-06 立，2026-10-10 扩作用域）：终局要的四件里，这一件从前
        // 没有落点（`agent/README.md` 那句"因此这一格不立项"已按裁决翻掉）。
        //
        // ⚠ **2026-10-10 改（主人裁决）**：作用域从「最近那一段**没走完**」扩到
        // 「**最近那一段，不论有没有收场**」。判据是那两件事本来就独立：一段可以**带着未确认的
        // 效果收场**（`system:indeterminate` 说的就是这个）——按旧作用域，它派发过的那几件
        // 下一段**一个字都看不到**，而纸条还会读成"**确实没有**"（那是假话，不是"读不到"）。
        // 哪几件没结算仍由既有的单活动解释器给（[`unsettled_of`]），这里**不复制**它的判据。
        //
        // 一次 `list_runs` 就够（它同时读**目录**与**活动段**两半）：**最近那一段**按**序号**取最大
        // ——**不按活动 id**：`run-12` 按 id 排在 `run-4` 前面，那不是先后。序号是写入顺序。
        let listed = crate::store::list_runs(environment);
        let latest = match &listed {
            Ok(runs) => match runs
                .iter()
                // ⚠ **不把自己算进去**（与"没走完"那一格同一条判据）：开场这一刻这一段**还在跑**，
                // 把它的清单当成"上一段没结算的"就是假话。
                .filter(|summary| summary.run() != &run_id)
                .max_by_key(|summary| summary.first_seq())
            {
                Some(latest) => latest_segment_of(environment, latest.run()),
                // 一段都没有（第一次开机）⇒ **确实没有**，不是"读不到"。
                None => LatestSegment {
                    unsettled: Vec::new(),
                    unsettled_unreadable: None,
                    held: None,
                    stopped: None,
                },
            },
            // 读不出来 ⇒ 如实说"读不到"（**不冒充"没有"**）。⚠ `held` 在这一档**缺席**，
            // 而同一块已经有一行如实说"读不出来"——不冒充"正常收场"。
            Err(error) => LatestSegment {
                unsettled: Vec::new(),
                unsettled_unreadable: Some(error.as_str().to_string()),
                held: None,
                stopped: None,
            },
        };
        // **全部没走完的段**（2026-10-06）：准入读数只给**一个** id ⇒ 更早的报不出来（**会漏**），
        // 而"没走完"这件事是**长期的**（它永远不会被补上收尾）⇒ 漏掉的那些会**一直**漏。
        let unfinished_all = match &listed {
            Ok(runs) => {
                let mut runs: Vec<_> = runs
                    .iter()
                    .filter(|summary| summary.status() == "interrupted")
                    .collect();
                runs.sort_by_key(|summary| summary.first_seq());
                runs.into_iter()
                    .map(|summary| summary.run().clone())
                    .collect()
            }
            // 读不出来 ⇒ 退回"只有准入那一个"：`unfinished_all` 空时渲染走"有一段"那一档，
            // 而那**正是准入读数给的事实**（不假装读全了，也不编一个数）。
            Err(_) => Vec::new(),
        };
        let previous = PreviousReading {
            unfinished,
            unfinished_all,
            storage_failure: readings.last_storage_failure,
            unsettled: latest.unsettled,
            unsettled_unreadable: latest.unsettled_unreadable,
            held: latest.held,
            stopped: latest.stopped,
        };

        // 权威身份只从环境里取：调用者的声明只用于核对，不用于装配。
        let agent_id = environment.agent().clone();
        // 目录必须在借出写资格**之前**取下来——读目录这个动作本身也会借用环境。
        let environment_root = environment.root().to_path_buf();

        let os = assemble_machine(
            material_root,
            &environment_root,
            declarations.creation.as_ref(),
            MachineDeclarations {
                im: declarations.im.as_ref(),
                web: WebDeclaration {
                    search: declarations.search.as_ref(),
                    tier: declarations.web,
                },
                terminal: declarations.terminal.as_ref(),
                mcp: &declarations.mcp,
                vision: declarations.vision,
            },
        )?;
        // 开场素材：**读法只有一份**（见 [`read_opening_materials`]）。三态 / 四态语义在那一处
        // 各自保持——读失败与"没有"分得开，这里一个都不压平。
        //
        // ⚠ 最后那两格（**上一次活动的开场摆了哪几块** ＋ **那一段想过没有**）是**交接交付判据**
        // 要的那一份事实：它们与准入、"最后一次存储故障"同属**那一次读**（`store::OpeningReadings`）
        // ⇒ 不新开读取点。
        let materials = read_opening_materials(
            &environment_root,
            material_root,
            previous,
            &readings.last_opening_blocks,
            readings.last_activity_thought,
        );

        let mut agent = Agent::new(agent_id.clone());
        let run = Run::begin_with_input(&mut agent, run_id, limits, input)?;
        // 桌面的一层：**装配期取一次**（不读盘、不看展开状态）。目录只在首轮用它分组渲染；
        // 之后新展开的动作由展开回执自带（既有行为）——所以这一份静态事实就够。
        let desktop = os.desktop();

        Ok(RunSession {
            os,
            expansion: RunExpansion::new(agent_id.clone(), run.id().clone()),
            agent,
            run,
            ledger: Ledger::new(agent_id.clone()),
            environment,
            agent_id,
            environment_root,
            source: HostSource {
                // 桌面的一层（TASK_71 桌面 P0）：门之外那一组 + 每扇 App 的门。
                desktop,
                // 处境链：**形状与顺序只有一份**（见 [`situation_chain`]）。本处是**新建路径**：
                // 有锚、有投影、有细胞，起源是这一次真实的唤醒缘由。
                // 钟留一份在会话上（算时长用）；两份同源，不会一个走系统钟一个走夹具钟。
                origin: situation_chain(
                    clock.clone(),
                    context_window,
                    reading,
                    &materials,
                    &declarations,
                    SituationLive {
                        since: declared.since,
                        notification: declared.notification,
                        cards: declared.cards.clone(),
                        origin: origin_text(declared.origin),
                        // **复用**装配那一轮已经取过的钟（`clock_at_open`，算时长用的那一次），
                        // 因此本刀没有让装配多读一次钟。
                        now_millis: unix_millis(clock_at_open),
                    },
                ),
                pending: None,
            },
            // 唤醒缘由随 `run-start` 经历入账（见 `EnvironmentCommit::with_origin`）：
            // 这一格是**结构化**的「谁叫醒的」（词表值），与处境链里那条渲染出来的
            // `activity-origin` 块各司其职——那条给模型看，这一格给"为什么醒"那条查询用。
            declared_origin: declared.origin,
            clock,
            // 起点取在**会话打开**这一刻（与刚刚写下的 `run-start` 同一轮）。
            clock_at_open,
            usage: declarations.usage,
            model_retries: Vec::new(),
            phase: SessionPhase::Ready,
            orientation: materials.orientation.clone(),
            static_head_recorded: false,
        })
    }

    /// 这次会话的活动标识。
    pub fn run(&self) -> &RunId {
        self.run.id()
    }

    /// 这次会话绑定的身份。
    pub fn agent(&self) -> &AgentId {
        &self.agent_id
    }

    /// 这次活动当前的轨迹（只读）。
    pub fn trace(&self) -> &[TraceEntry] {
        self.run.trace()
    }

    /// 活动还没有收场时是 `None`；收场了就是那个结局。
    pub fn outcome(&self) -> Option<Outcome> {
        self.run.outcome()
    }

    /// 显式继续：给下一次驱动排入一段宿主输入。
    ///
    /// 三种明确的错误都不静默：会话已经不可驱动、已经排了一段还没被呈现的输入（**不建队列**）、
    /// 或者这段文本超过既有内容批次上限。校验在这里做完，因此排进来的批次一定能在下一轮被
    /// 原样呈现——不会等到驱动到一半才发现它递不进去。
    pub fn provide(&mut self, input: RunInput) -> Result<(), CompositionError> {
        if self.phase != SessionPhase::Ready {
            return Err(CompositionError::SessionNotDrivable);
        }
        if self.source.pending.is_some() {
            return Err(CompositionError::Run(RunError::OperationInFlight));
        }
        let block = ContentBlock::new(HOST_INPUT_LABEL, input.as_str());
        let batch = ContentBatch::of(vec![block]).map_err(|_| {
            CompositionError::MachineRefused(format!("host-input:{}", HOST_INPUT_TOO_LARGE))
        })?;
        self.source.pending = Some(batch);
        Ok(())
    }

    /// 驱动一次：**同一个** Run、同一份轨迹与预算，不重复登记、不重复 Run 级注入。
    ///
    /// 已经排入的宿主输入会在这一轮的 Turn 级接缝上被呈现一次（`before_turn` 的时机不变）。
    pub fn drive<M: ModelPort>(&mut self, model: &mut M) -> Result<SessionDrive, CompositionError> {
        if self.phase != SessionPhase::Ready {
            return Err(CompositionError::SessionNotDrivable);
        }
        self.check_binding()?;

        // ── **这一段生命还能不能开始：在留证之前问**（2026-09-26）────────────────────
        //
        // 为什么必须排在这里：紧接着下面那一笔是**装订留证**，而它是一条**见证**
        // —— 代码自己写着「它不在 Core 的提交序列里」「**只作见证，不进视野**」。
        // **见证是记录，不该有否决权。** 但从前的顺序让它成了判决点：重复的活动标识
        // 先撞上它、报成 `stage=static-head`，于是 store 里那个**专门为这件事准备**的
        // `RunAlreadyKnown`（`append_trace` 里 `is_start` 那一支）**永远到不了**。
        //
        // 判决点还给活动身份本身 ⇒ 报出来的阶段是 `run-start`（"这次活动开始了"那一步），
        // 与 `core.rs` 里那句「登记失败（**含重复 RunId**）在上面就已经返回」重新对上。
        //
        // 只在**这一次活动还没开始**时问：恢复路径上这段生命早就在盘上了，那一问会误伤它
        // —— 与 Core 里 `if !run.is_started()` 同一条件。这一问仍在**任何模型调用之前**，
        // 所以下面「写不下来就不驱动」那条规矩一字未改。
        if !self.run.is_started() && self.environment.run_is_taken(self.run.id()) {
            self.phase = SessionPhase::Failed;
            return Err(finish_failure(
                self.environment,
                self.run.id(),
                RunError::Commit(CommitError::at(CommitStage::RunStart)),
            ));
        }

        // **静态头装订**（TASK_79）：装配时拍读的那一份，每一次驱动前重新推给模型。
        //
        // 推的是**同一个值**（Run 内不重读盘）⇒ 主体在 Run 里改那个文件不影响本次活动的请求；
        // 下一次活动的开场会重新拍一次。`None` = 这一次没有静态头（家里没有那一份，或主体摘了）
        // ⇒ 请求里一个字节都不加——**缺席也是明确推一次**，不是"没推就沿用上一次"：
        // 同一台宿主跨 Run 复用同一个模型，漏推一次就会让上一个 Run 的头留在那里。
        model.declare_static_head(self.orientation.head());

        // **装订留证**：一次进程一次，写在装订**那一刻之前**——写不下来就不驱动。
        // 宁可什么都不做，也不留一条"装订过、但没人知道装订了什么"的痕迹。
        //
        // 生效轮次取"这一轮之后要驱动的第一轮"：开场装订从第 1 轮起生效。于是"第 N 轮当时
        // 生效的是哪一份"是**查出来**的。
        if !self.static_head_recorded {
            let bind = Turn::from_number(self.run.last_turn().number().saturating_add(1));
            let written = match bind {
                Some(bind) => {
                    let mut commit = EnvironmentCommit::with_origin_and_head(
                        self.environment,
                        Some(self.declared_origin),
                        self.orientation.head().map(str::to_string),
                    );
                    commit.commit_static_head(&self.run.id().clone(), bind)
                }
                // 轮次已经到顶（下一轮不存在）⇒ 没有可生效的轮次，也就没有可留的证。
                None => Ok(()),
            };
            if let Err(error) = written {
                self.phase = SessionPhase::Failed;
                return Err(finish_failure(
                    self.environment,
                    self.run.id(),
                    crate::core::RunError::Commit(error),
                ));
            }
            self.static_head_recorded = true;
        }

        let result = {
            // 提交接缝持有的就是会话自己那个环境：写资格不经过调用方的手。
            // 起源由会话声明（`declared_origin`）：只有**这一次活动开始**那一轮会用到它
            // （`commit_run_start` 只在 `!run.is_started()` 时被调用），此后驱动不再看它。
            let mut commit =
                EnvironmentCommit::with_origin(self.environment, Some(self.declared_origin));
            let mut scope = self.os.scope(&mut self.expansion);
            // TASK_60 裁决 5：模型调用外面包一层**有界重试**（传输层瞬时族：1 次、立即、零参数）。
            // 它是**调用行为**不是协议细节 ⇒ 两条 wire 与未来的别家共用这一份实现；
            // 记录**带走**交给调用方打（库不打印：那是 TASK_51 删过的侧信道形状）。
            let mut model = RetryingModel::new(model, TRANSIENT_RETRY_ATTEMPTS);
            // **代价在收场那一刻现取**（`settle` 只在真收场时调它一次）。
            //
            // 时长要"现在几点"：钟是会话自己的那一只（`SessionRequest::clock`），因此离线夹具
            // 的固定钟照样给得出确定的时长。用量取自 cell——`None`（没接 cell）或"一条都没报过"
            // 都如实读作"这一格没有被记"，**不编造**。
            let clocks = self.clock.clone();
            let usage_cell = self.usage.clone();
            let started_at = self.clock_at_open;
            let cost = move || -> RunCost {
                let elapsed = clocks
                    .now()
                    .duration_since(started_at)
                    .ok()
                    .map(|elapsed| elapsed.as_millis() as u64);
                RunCost {
                    elapsed_millis: elapsed,
                    usage: usage_cell.as_ref().and_then(UsageReport::latest),
                }
            };
            let outcome = continue_activity(
                &mut self.run,
                &mut self.agent,
                &mut model,
                &mut scope,
                &mut self.ledger,
                &mut self.source,
                &mut commit,
                &cost,
            );
            // **累积**：一次唤醒可能驱动好几轮（等一会儿再继续），不能只留最后一轮的。
            self.model_retries.extend(model.into_report());
            outcome
        };

        match result {
            Ok(DriveOutcome::Unsettled) => Ok(SessionDrive::Unsettled),
            Ok(DriveOutcome::Ending(outcome)) => {
                self.phase = SessionPhase::Ended;
                Ok(SessionDrive::Ended(outcome))
            }
            Err(error) => {
                // 活动槽已经由驱动入口在错误路径上交还（见 `core::continue_activity`）。
                // 这里只把这次会话标成"不可再驱动"：失败**不**降级成一次正常等待。
                self.phase = SessionPhase::Failed;
                Err(finish_failure(self.environment, self.run.id(), error))
            }
        }
    }

    /// **取走**自上次取走以来发生过的模型重试（TASK_60 裁决 5）：诊断面，由调用方（宿主）打印。
    ///
    /// 为什么是"取走"而不是"看一眼"：一次唤醒要驱动好几轮，每轮都可能重试——
    /// 逐个"看一眼"会让宿主重复打同一行；取走则**每条恰好打一次**。
    /// 空 = 其间一次都没重试（绝大多数情况）。它**不是**事实面的一部分。
    pub fn take_model_retries(&mut self) -> Vec<ModelRetry> {
        std::mem::take(&mut self.model_retries)
    }

    /// 排入一段宿主输入并驱动一次：宿主那一步的**唯一**写法。
    ///
    /// 排入失败时**不**驱动——宁可什么都不做，也不驱动一次没有把输入带进去的轮次。
    pub fn drive_with_input<M: ModelPort>(
        &mut self,
        input: RunInput,
        model: &mut M,
    ) -> Result<SessionDrive, CompositionError> {
        self.provide(input)?;
        self.drive(model)
    }

    /// 每次驱动前重新核对绑定：这个会话绑定的就是这一个环境。
    fn check_binding(&self) -> Result<(), CompositionError> {
        if self.environment.agent() != &self.agent_id {
            return Err(CompositionError::Environment(
                crate::store::StoreError::IdentityMismatch {
                    expected: self.agent_id.clone(),
                    found: self.environment.agent().clone(),
                }
                .as_str()
                .to_string(),
            ));
        }
        if self.environment.root() != self.environment_root {
            return Err(CompositionError::Environment(
                crate::store::StoreError::IdentityMismatch {
                    expected: self.agent_id.clone(),
                    found: self.environment.agent().clone(),
                }
                .as_str()
                .to_string(),
            ));
        }
        Ok(())
    }
}

/// 宿主输入在轨迹里的条目标签。它是**内容标签**，不是权限：这一段仍然是环境内容。
const HOST_INPUT_LABEL: &str = "host-input";

/// 宿主输入超过既有内容批次上限时的稳定分类标记。
const HOST_INPUT_TOO_LARGE: &str = "too-large";

/// 装配一台最小 OS：授权资料根 + 经历目录 + Notes App + web 窗。
///
/// 两个入口（一次性驱动与会话）共用它，因此"怎么装配"只有一份实现；差别只在"驱动几次"。
/// Notes 永远装配（这样才读得到旧笔记），而且**预装常开**：主体的笔记是自己的字，
/// 写它不需要任何人先授权（2026-09-12 字裁决）。
///
/// **web 窗按出网档装配**（2026-10-13）：`off` ⇒ 不摆这件 App；`fetch` ⇒ 摆，但「查」不进声明面；
/// `full` ⇒ 两条都在场（出厂以来的样子）。凭据只影响**一条动词的执行**，档位决定**在不在场**——
/// 两者是两件事（这一点对恢复是承重的：工具面必须与上一段经历一致）。
///
/// web 窗这一次装配的**两格声明**——**不是一格**：凭据管"能不能执行"（缺了动词照旧在场），
/// 档位管"在不在场"（`off` ⇒ 连门都没有）。绑成一个参数只是为了不把本函数铺到八个位置参数
/// （与 `agent_run` 的 `Declarations` 同一条理由：铺开就没人记得住顺序了）；两格事实**各自有
/// 名字、各自的文档**，谁也没被并进谁。
#[derive(Clone, Copy)]
struct WebDeclaration<'a> {
    search: Option<&'a SearchAccess>,
    tier: crate::web_app::WebTier,
}

/// 一次装配的**机器声明**：宿主说这台机器上**有哪些东西**。
///
/// 与 [`WebDeclaration`] **同一条理由**（"铺开就没人记得住顺序了"）：2026-10-17 加上
/// `vision` 那一格时，`assemble_machine` 正好越过 clippy 的七格线——这不是洁癖，**七格以上
/// 的位置参数，调用点迟早把两格写反**，而写反的后果是"某个 App 悄悄不在场"。
///
/// ⚠ 包里每一格**各自有名字、各自的文档**，谁也没被并进谁：它们**同源同命**（都由宿主声明、
/// 都决定"哪些 App 与哪些动词在场"），但**不是同一件事**。
#[derive(Clone, Copy)]
struct MachineDeclarations<'a> {
    /// im 的访问声明（端点 + 凭据）；`None` = 这次装配里没有 im app。
    im: Option<&'a ImAccess>,
    /// web 窗的两格声明（见 [`WebDeclaration`]）。
    web: WebDeclaration<'a>,
    /// terminal 的档位声明；`None` = 这只手不在。
    terminal: Option<&'a TerminalAccess>,
    /// MCP server 的声明（宿主声明了几台就有几台）。
    mcp: &'a [McpAccess],
    /// 这一次装配用的模型收不收得到媒体材料（见 [`SessionDeclarations::vision`]）。
    vision: bool,
}

fn assemble_machine(
    material_root: &Path,
    environment_root: &Path,
    creation: Option<&CreationGrant>,
    declarations: MachineDeclarations<'_>,
) -> Result<MinimalOs, CompositionError> {
    let MachineDeclarations {
        im,
        web,
        terminal,
        mcp,
        vision,
    } = declarations;
    let view = FileView::with_history_and_creation(material_root, environment_root, creation)
        .map_err(|failure| machine_refused(MachineFailure::View(failure)))?;
    let notes = NotesApp::open(environment_root).map_err(|failure| {
        CompositionError::MachineRefused(format!("notes-app:{}", failure.as_str()))
    })?;
    let mut apps: Vec<Box<dyn crate::app_contract::AppPort>> =
        vec![Box::new(view), Box::new(notes)];
    // **预装常开**：notes 与 im 的能力从第一轮起就在场（材料窗照旧要点开才展开）。
    // im 只有被装配时才预装——声明一个没装配的 App 会被装配期拒绝，这是故意的。
    let mut preinstalled: Vec<&str> = vec![NOTES_APP_ID];
    // TASK_75：主体的第三件自持物（约时间）——**不预装**：与 files 同款，展开之后那三条动词
    // 才进场；没展开就被既有的展开门拒（门语义一个字没动）。它的表住在**环境根**
    // （`<环境根>/timers`），因此构造期只给根，不做任何 IO。
    apps.push(Box::new(crate::timer_app::TimerApp::new(environment_root)));
    // **web 窗按出网档装配**（2026-10-13）：`off` ⇒ 这一档**不摆这件 App**（主体的门里连这扇门都
    // 没有）；`fetch` ⇒ 摆，但「查」不进声明面；`full` ⇒ 两条都在场（出厂以来的样子）。
    // 判据：**"主体能不能出网"是主人的政策**，与 `--terminal` 不给就没有那只手同款。
    // ⚠ 凭据（`search`）只管**能不能执行**，不管在不在场——两者是两件事。
    if let Some(app) = WebApp::with_tier(
        web.search.cloned(),
        web.tier,
        crate::web_app::WebPolicy::production(),
    ) {
        apps.push(Box::new(app));
        preinstalled.push(WEB_APP_ID);
    }
    // **档位门控装配**：terminal 只有宿主声明了档位才在场（`--terminal full`）。
    // 没有声明就是"这只手不在"——动词不存在，能力面里一个字节都没有；不预建窄档（加档是加墙）。
    if let Some(access) = terminal {
        apps.push(Box::new(TerminalApp::new(access.clone(), environment_root)));
        preinstalled.push(TERMINAL_APP_ID);
    }
    // **宿主声明了才装配**：没声明就是"这台机器上没有 im"——Agent 的能力面里因此不出现它，
    // 也不会出现"列得到却用不了"的条目。装配期建不出请求客户端是装配失败（不是主体的失败）。
    //
    // **2026-10-02 拆包**：一份声明装出三个 App——通信面 `im`（**预装常开**）与社交面
    // `im-social`、资料面 `im-profile`（**不预装**：主体展开那一下才上场）。三个 App
    // 共用同一份 [`ImAccess`]，但**各自建自己的请求客户端**（多两个连接池，可接受）。
    if let Some(access) = im {
        let app = ImApp::new(access.clone())
            .ok_or_else(|| {
                CompositionError::MachineRefused(format!("{IM_APP_ID}-app:client-unavailable"))
            })?
            // 递文件那只手读的是**自己的世界**（与 files / notes 同一个根）。宿主不声明这个世界，
            // 那只手就不在：给了 `file` 也读不出来，回执如实报 `file-unreadable`（不猜、不假装）。
            .with_world_root(environment_root)
            // **读侧的可读类型清单由模型事实派生**（2026-10-17）：这个模型收不到图像块时，
            // 工具面不该说「读得动图片」——清单里就没有媒体类（与今天逐字节相同）。
            .with_vision(vision);
        apps.push(Box::new(app));
        preinstalled.push(IM_APP_ID);
        // 社交面：与通信面同端点、同凭据；**不进预装名单**。
        let social = ImSocialApp::new(access.clone()).ok_or_else(|| {
            CompositionError::MachineRefused(format!("{IM_SOCIAL_APP_ID}-app:client-unavailable"))
        })?;
        apps.push(Box::new(social));
        // 资料面：同款；换头像那只手读的是**自己的世界**（与 `im_say` 递文件同一个根）。
        let profile = ImProfileApp::new(access.clone())
            .ok_or_else(|| {
                CompositionError::MachineRefused(format!(
                    "{IM_PROFILE_APP_ID}-app:client-unavailable"
                ))
            })?
            .with_world_root(environment_root);
        apps.push(Box::new(profile));
    }
    // **外部来源的 App**（MCP server）：宿主声明了几台就有几台；没声明就是一台都没有。
    //
    // 它们**不预装**，而且这是本刀的核心：装配期拿不到动词清单（要真的连上对面才有），
    // 因此它们在装配期"只认得名字"——**展开那一下**才把动词面交出来（见 `AppPort::present`）。
    // 名字撞车（两台同名、或与出厂内置件同名）由装配期按 `DuplicateAppId` 拒绝。
    for access in mcp {
        apps.push(Box::new(McpApp::new(access.clone())));
    }

    MinimalOs::assemble_preinstalled(apps, &preinstalled)
        .map_err(|failure| machine_refused(MachineFailure::Assembly(failure)))
        // TASK_68 P3：**这一条装配托着主体**，所以它知道 Agent 的环境根——自持物动作（pin/unpin）的去向。
        // 别的装配入口（`assemble*` 那些测试构造）没有它，那种舞台里相关动作会明确拒绝。
        .map(|os| os.with_environment_root(environment_root))
}
