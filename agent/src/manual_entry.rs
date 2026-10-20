//! 手动运行入口：把已经打通的执行链接成一条**普通使用者可用**的显式路径。
//!
//! 它只做三件本就属于入口的事，不另造执行机制：
//!
//! 1. **显式初始化**——按调用方给出的环境根与身份调用既有 `create`，并把环境里**实际记录**
//!    的身份读回来（不回显参数）；
//! 2. **显式启动**——按调用方给出的环境根、身份、授权资料根、活动标识与 Turn 上限，
//!    解析开场输入后经既有 `open` 与 [`run_activity_in_environment`] 跑**一次**有限活动；
//! 3. **如实报告**——把结果按 Core 自己的收场词汇分类，并给出稳定退出码与稳定文本。
//!
//! 它**不是**聊天产品、常驻服务或通用 Host：没有 REPL、没有流式输出、没有自动编号、
//! 没有自动恢复、没有第二次执行路径。这里也没有模型循环——唯一那条执行循环在 Core 里，
//! 本模块只把参数与来源接上去。
//!
//! ## 边界
//!
//! - 模型由**调用方**提供（`M: ModelPort`）：本模块不认识任何具体适配层，也不读任何凭据。
//!   生产命令自己装配真实的模型，离线验收自己注入回环适配层或替身——两条路走的是**同一段**
//!   启动代码，因此不为两边维护两套执行逻辑；
//! - 开场输入由调用方显式给出（命令行文本、显式从标准输入读取，或空）。标准输入只在被显式
//!   选择时读取，按 Core 的输入上限**有界**读取：先无限读入再校验等于给自己留一个不受限的
//!   内存入口，截断则等于伪造一份"看起来完整"的输入——两者都不做；
//! - 资料根由调用方显式给出：这里没有默认目录，也不会把工作目录自动授权给 Agent。
//!
//! ## 退出码不是任务成功证明
//!
//! [`ManualEnding::exit_code`] 的 `0` 只表示**主体自己收场**（Core 的 `Subject` 收场），
//! 不表示任务完成；系统终止、入口拒绝、准入拒绝与保存未完整确认各有自己的码。同一个事实
//! 不允许被两种说法覆盖，也不允许用一个 `success` 吞掉全部结局。

use std::fmt::Write as _;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};

use crate::clock::ClockHandle;
use crate::composition::{
    CompositionError, RunSession, SessionDeclarations, SessionDrive, SessionRequest,
};
use crate::context_reading::ContextReading;
use crate::core::{Limits, MAX_INPUT_BYTES, ModelPort, RunError, RunInput};
use crate::foundation::{AgentId, Outcome, RunId, SubjectEnding, SystemEnding};
use crate::store::{Environment, StoreError, subject_label, system_label};

/// 报告首行：让调用方能认出格式版本，而不是靠猜字段。
const REPORT_HEADER: &str = "celira-agent-run v1";

/// 装配模型失败时的退出码（模型服务凭据缺失或为空）。
///
/// 入口自己**不**读凭据、也不认识任何具体适配层：它在把模型交给启动函数**之前**就已经失败，
/// 因此这类结果归装配模型的一方判定。这里只保留这个稳定码，好让生产命令与离线消费者对
/// 同一类失败给出同一个码，不必各自发明一个数字。
///
/// ⚠ **它不只覆盖"凭据缺失"**（2026-10-08）：**配置配对错误**同样归这一族——清单/绑定里声明了
/// 思考强度，而线协议选的是**请求体上没有这一格**的那一条 ⇒ 两个生产入口的装配当场拒绝，
/// 码就是这一个。判据一句话：这属于"**配置配不上**"，不是参数写错（那样是 `1`），也不是一次
/// 收场（`0` 与各结局各自的码）。分类必须留着：码塌成一个，操作者就分不出"我哪里写错了"。
pub const EXIT_MODEL_ASSEMBLY_FAILED: u8 = 5;

/// 停在等待时的退出码：这次驱动停下了，但活动**没有**结束。
///
/// 它必须与 `0`（主体自己收场）分开：等待不是一个结局，把它报成 `0` 就等于说"活动结束了"，
/// 而环境里其实还留着一个未收尾的活动。`8` 保留不用，避免与已废弃的"收场事实缺失"码混同。
pub const EXIT_UNSETTLED: u8 = 9;

/// 一次手动运行需要的全部参数。**每一样都由调用方显式给出**。
///
/// 这不是新的一层抽象：它只是把"在哪个环境、以哪个身份、用哪个资料根、跑哪一次活动、
/// 边界多大、开场说什么"绑在一起，好让启动函数不涨成十来个位置参数。
///
/// 可选的声明（创建授权 / 模型 / 用量接线 / im / 身份 / 搜索 / 终端档位 / MCP）收在
/// [`SessionDeclarations`] 里**一个包**，因此加一格不必在这里再铺一个字段。
pub struct ManualRun {
    /// 身份环境目录。必须**已经存在**：本模块不自动初始化。
    pub environment_root: PathBuf,
    /// 调用方声明的身份。与环境里记录的权威身份不一致就是拒绝启动。
    pub expected_agent: AgentId,
    /// 授权资料根。必须显式给出：没有默认值，也不使用工作目录。
    pub material_root: PathBuf,
    /// 这次活动的标识。由调用方生成——本版**不**自动编号，也不声称时间戳能保证唯一。
    pub run: RunId,
    /// Turn 上限，必须是正整数。
    pub max_turns: u32,
    /// 开场内容的明确来源。
    pub input: ManualInput,
    /// 宿主给出的**时间来源**：状态栏每一轮用它现取一次"现在几点"。
    ///
    /// 生产命令给真实钟（[`ClockHandle::system`]），离线夹具给固定钟——两处走的是同一段装配代码、
    /// 同一个格式化点，差别只在这一个构造参数。它不是一个可配置项：没有"要不要状态栏"的开关。
    pub clock: ClockHandle,
    /// 宿主声明的**上下文窗口**（状态行 `context` 段的分母，必须是正整数）。
    ///
    /// 生产命令缺省填适配层的公开标称值，`--context-window` 可以覆盖——声明权在宿主手里。
    pub context_window: u32,
    /// **屏幕报告 cell 的读侧**：装配模型的那一方拿到写柄，这里拿到读侧，两边共用同一个 cell。
    pub reading: ContextReading,
    /// **可选的那几格**（一个包，见 [`SessionDeclarations`]）：创建授权 / 模型名 / 用量接线 /
    /// im 访问 / 身份 / 搜索 / 终端档位 / MCP。**每一格的来路与语义写在包里那一格自己身上**。
    pub declarations: SessionDeclarations,
}

/// 开场内容的三种**明确**来源。
///
/// "命令行文本"与"显式从标准输入读取"互斥；不指定就是空输入。互斥由构造方（命令行的参数
/// 解析）保证：本模块只按拿到的那一种解析，不替调用方猜。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManualInput {
    /// 这次活动没有开场内容。空输入是合法活动，不是"参数忘了给"。
    Empty,
    /// 命令行直接给出的一段文本。
    Text(String),
    /// 显式从标准输入读取 UTF-8。
    Stdin,
}

impl ManualInput {
    /// 稳定分类标记（用于报告，不参与判断）。
    pub fn as_str(&self) -> &'static str {
        match self {
            ManualInput::Empty => "empty",
            ManualInput::Text(_) => "text",
            ManualInput::Stdin => "stdin",
        }
    }
}

/// 入口**自己**能拒绝的原因：都在打开环境、写任何事实或调用模型之前判定。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManualEntryError {
    /// Turn 上限不是正整数。
    TurnBudgetNotPositive { max_turns: u32 },
    /// 上下文窗口不是正整数。
    ///
    /// 窗口是**宿主声明**，没有"未声明"分支：`0` 不是一个更小的窗口，而是一条无法解释的声明
    /// （状态行会写出 `context = …/0 <单位>`）。因此这里明确拒绝，而不是让它变成一个看起来像读数的数。
    ContextWindowNotPositive { window: u32 },
    /// 开场输入超过 Core 的字节上限。拒绝就是拒绝：不截断。
    InputTooLarge { limit: usize },
    /// 从输入读到的不是合法 UTF-8。
    InputNotUtf8,
    /// 读输入本身失败。
    InputUnreadable,
    /// `--input-stdin` 与宿主输入循环不能同时生效。
    ///
    /// 开场输入会把标准输入整条读空，之后再也不会有一段"显式的下一段输入"。这里明确拒绝，
    /// 而不是悄悄给一个空开场、然后立刻报"流结束"。
    InputSourceConflictsWithHostLoop,
    /// 环境打不开（不存在、身份不匹配、已有写者、身份或存储损坏）。
    Environment(StoreError),
}

impl ManualEntryError {
    /// 稳定分类标记。
    pub fn as_str(&self) -> &'static str {
        match self {
            ManualEntryError::TurnBudgetNotPositive { .. } => "turn-budget-not-positive",
            ManualEntryError::ContextWindowNotPositive { .. } => "context-window-not-positive",
            ManualEntryError::InputTooLarge { .. } => "input-too-large",
            ManualEntryError::InputNotUtf8 => "input-not-utf8",
            ManualEntryError::InputUnreadable => "input-unreadable",
            ManualEntryError::InputSourceConflictsWithHostLoop => {
                "input-source-conflicts-with-host-loop"
            }
            ManualEntryError::Environment(_) => "environment",
        }
    }

    /// 稳定退出码。
    pub fn exit_code(&self) -> u8 {
        match self {
            ManualEntryError::Environment(_) => 2,
            _ => 1,
        }
    }
}

/// 一次手动运行的结果分类。
///
/// 它**沿用** Core 的收场词汇（主体收场 / 系统终止），只在"入口被拒绝"这一层加上既有
/// [`CompositionError`] 的原始分类——不新造布尔、不把不同结局压成"成功/失败"。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManualEnding {
    /// 主体自己收场（完成、沉默、拒绝、等待）。
    Subject(SubjectEnding),
    /// 系统层面的终止（模型失败、取消、超时、崩溃、结果无法确认）。
    System(SystemEnding),
    /// 入口/准入/装配/保存层面被拒绝。
    ///
    /// `SaveFailed` 已经自带"停在哪一个确认点"，因此这里**不需要**从别处补齐信息；
    /// 本模块也不改写既有错误契约去追求好看的输出。
    Refused(CompositionError),
    /// 这次驱动停在**未收场**：活动**没有**结束。
    ///
    /// 它与主体的 `waiting` 收场是**两件不同的事**：
    ///
    /// - [`ManualEnding::Subject`] 里的 `SubjectEnding::Waiting` 是主体自己说"我在等外部的东西"
    ///   并因此**结束**这次活动；
    /// - 这一条是**这次驱动被停下了**：没有终态、活动槽还属于它、轨迹与预算都还在。
    ///   要接着跑，宿主必须显式给出下一段输入或显式继续。
    ///
    /// ⚠️ **它从前叫 `Waiting`**（2026-10-06 改）：上面那句话从前就写在本文件里——「**绝不能
    /// 压成一个词**」——而这一档当时**正好就叫 `Waiting`**。名字重合了，判据没重合：
    /// 现在这一档的名字说的是**活动的状态**（未收场），与收场那一档不再撞名。
    Unsettled,
}

impl ManualEnding {
    /// 稳定分类标记（开放文本仍然只有逃逸后的活动标识与身份）。
    ///
    /// 收场那两条路的词与形状归 `store`（盘上的封闭表在那里），这里只做委托——返回 `String`
    /// 而不是 `&'static str`，正是因为那份文本由权威那一处拼出来（2026-10-06 词表立权威那一刀）。
    pub fn as_str(&self) -> String {
        match self {
            ManualEnding::Subject(ending) => subject_label(*ending),
            ManualEnding::System(ending) => system_label(*ending),
            ManualEnding::Refused(_) => "refused".to_string(),
            // ⚠️ 这个**分类标记**照旧是 `waiting`（2026-10-06 只改了类型名，没动它）：
            // 它是**用户可见**的输出（`ending=…`），而它与收场那一档**本来就分得开**——
            // 主体那个是 `subject:waiting`（带前缀）。真正撞名的只有**类型名**，改的是那个。
            // 想改这个标记的话，它是一条**契约变更**，要连同断言与手册一起裁，不在改名那一刀里。
            ManualEnding::Unsettled => "waiting".to_string(),
        }
    }

    /// 稳定退出码。**`0` 不是任务完成证明**，它只表示主体自己收场。
    ///
    /// | 码 | 含义 |
    /// |---|---|
    /// | 0 | 主体自己收场 |
    /// | 1 | 入口拒绝：参数不合法或输入不合法（含超上限、非法编码、Turn 上限非正） |
    /// | 2 | 环境不可用（不存在、身份不匹配、已有写者、身份或存储损坏） |
    /// | 3 | 准入拒绝（已记录暂停） |
    /// | 4 | 启动被拒（资料根不可用或与环境内部记录重叠、装配冲突、活动层面拒绝） |
    /// | 5 | 服务凭据不可用：模型服务的凭据缺失或为空，或 im 的端点与凭据只给了一半（由入口判定，
    /// 只报变量名、不报取值） |
    /// | 6 | 保存未完整确认（带确认点；不声称成功、不声称记录不存在） |
    /// | 7 | 系统终止（模型失败、结果无法确认、Turn 用尽、取消、崩溃） |
    /// | 9 | 停在等待：活动**没有**结束，环境里留下一个未收尾的活动 |
    ///
    /// ⚠️ **`10` / `11` / `12` / `13` 已随跨进程恢复一起退役**（2026-09-26）：那四个码是
    /// "恢复被拒绝"的结论（已结束 / 被阻断 / 未知 / 不可读），而"顶着同一个 RunId 继续一段
    /// 没走完的生命"这条路已经不在了——进程死就是那一段断了，新进程起来是**新的一段**。
    /// 于是那些结论不再由**这个入口**给出；"这一次活动现在什么状态"改由 `agent_host_status`
    /// 按活动逐个读出来（见 `recovery::inspect`）。
    pub fn exit_code(&self) -> u8 {
        match self {
            ManualEnding::Subject(_) => 0,
            ManualEnding::System(_) => 7,
            ManualEnding::Unsettled => EXIT_UNSETTLED,
            ManualEnding::Refused(error) => refused_exit_code(error),
        }
    }
}

/// 拒绝 → 退出码。`CompositionError` 的既有分类被**原样**沿用，不重组、不丢阶段。
///
/// ⚠ **2026-10-06 改**：`StoragePaused` 那一档（码 3）随那道闸一起退役——暂停不再是拒绝
/// （它是**事实**，由开场纸条带给主体）。今天没有"需要你动手"这一档：装配失败一律归 2
/// （环境不可用）。
fn refused_exit_code(error: &CompositionError) -> u8 {
    match error {
        CompositionError::Environment(_) => 2,
        CompositionError::FileAppRefused(_) | CompositionError::MachineRefused(_) => 4,
        CompositionError::Run(RunError::Commit(_)) | CompositionError::SaveFailed { .. } => 6,
        CompositionError::Run(_) | CompositionError::SessionNotDrivable => 4,
    }
}

impl ManualRun {
    /// 入口能自己判定的校验：Turn 上限与上下文窗口都必须是正整数。
    ///
    /// 非法值在这里就停下：既不会打开环境，也不会调用模型。
    pub fn check(&self) -> Result<(), ManualEntryError> {
        if self.max_turns == 0 {
            return Err(ManualEntryError::TurnBudgetNotPositive {
                max_turns: self.max_turns,
            });
        }
        if self.context_window == 0 {
            return Err(ManualEntryError::ContextWindowNotPositive {
                window: self.context_window,
            });
        }
        Ok(())
    }

    /// 把明确给出的输入来源解析成这一次活动的开场输入。
    ///
    /// `reader` 由调用方提供：生产命令传标准输入，离线验收传一段字节。标准输入按上限
    /// **有界**读取（多读一个字节就能判定超限），因此不会先把不受限的内容读进内存。
    pub fn resolve_input(&self, reader: &mut dyn Read) -> Result<RunInput, ManualEntryError> {
        match &self.input {
            ManualInput::Empty => Ok(RunInput::empty()),
            ManualInput::Text(text) => {
                RunInput::text(text.clone()).map_err(|_| ManualEntryError::InputTooLarge {
                    limit: MAX_INPUT_BYTES,
                })
            }
            ManualInput::Stdin => {
                // 多读一个字节：读满上限 + 1 就是"超限"，不必把后续内容也读进来。
                let mut buffer = Vec::new();
                let mut bounded = reader.take(MAX_INPUT_BYTES as u64 + 1);
                bounded
                    .read_to_end(&mut buffer)
                    .map_err(|_| ManualEntryError::InputUnreadable)?;
                if buffer.len() > MAX_INPUT_BYTES {
                    return Err(ManualEntryError::InputTooLarge {
                        limit: MAX_INPUT_BYTES,
                    });
                }
                let text = String::from_utf8(buffer).map_err(|_| ManualEntryError::InputNotUtf8)?;
                // 上限在读取时已经判定过，这里仍然走同一构造函数：不另造一条"绕过校验"的路。
                RunInput::text(text).map_err(|_| ManualEntryError::InputTooLarge {
                    limit: MAX_INPUT_BYTES,
                })
            }
        }
    }

    /// 宿主输入循环的**开场输入**：只有"命令行文本"与"空"两种合法来源。
    ///
    /// 宿主输入循环把标准输入当作"下一步的显式输入"来读，所以它不能同时又被开场输入吃掉：
    /// 那会让等待之后永远读不到东西，而报告里看不出原因。拒绝是明确的，不静默降级。
    pub fn host_loop_initial_input(&self) -> Result<RunInput, ManualEntryError> {
        match &self.input {
            ManualInput::Empty => Ok(RunInput::empty()),
            ManualInput::Text(text) => {
                RunInput::text(text.clone()).map_err(|_| ManualEntryError::InputTooLarge {
                    limit: MAX_INPUT_BYTES,
                })
            }
            ManualInput::Stdin => Err(ManualEntryError::InputSourceConflictsWithHostLoop),
        }
    }
}

/// 入口错误的可读细节：一行说明 **这一类** 失败是什么意思。
///
/// 生产命令与离线夹具共用同一份，避免两边对同一类失败给出不同说法；它**不含**任何凭据、
/// 正文或请求内容——失败细节只描述类别与限制。
pub fn describe_error(error: &ManualEntryError) -> String {
    match error {
        ManualEntryError::TurnBudgetNotPositive { max_turns } => {
            format!("Turn 上限必须是正整数，收到 {max_turns}")
        }
        ManualEntryError::ContextWindowNotPositive { window } => {
            format!("上下文窗口必须是正整数（宿主声明），收到 {window}")
        }
        ManualEntryError::InputTooLarge { limit } => {
            format!("开场输入超过 {limit} 字节上限；不截断")
        }
        ManualEntryError::InputNotUtf8 => "读到的不是合法 UTF-8".to_string(),
        ManualEntryError::InputUnreadable => "输入读不出来".to_string(),
        ManualEntryError::InputSourceConflictsWithHostLoop => {
            "宿主输入循环与 --input-stdin 不能同时生效：开场输入会把标准输入整条读空".to_string()
        }
        // 环境这一类沿用存储层自己的稳定分类，入口不另编一个。
        //
        // 唯一例外是"身份不在、历史却在"（2026-10-06）：那一条**下一步动作很具体**（换一个位置，
        // 或先把那两份存储挪走），而它挡住的正是"用 init 去'修'一个丢了身份的环境"——那会把两份
        // 历史清空。因此这里给一句人话，别的环境失败照旧只报稳定分类。
        ManualEntryError::Environment(crate::store::StoreError::HistoryWithoutIdentity {
            ..
        }) => "这个位置里有历史记录、却没有身份：不按「创建」处理，一个字节都没动。\
             要重建就换一个位置，或先把那两份存储挪走"
            .to_string(),
        ManualEntryError::Environment(store) => store.as_str().to_string(),
    }
}

/// 显式初始化一个身份环境，并把**环境里实际记录**的身份读回来。
///
/// 它只调用既有 `create`：不调用模型、不读凭据、不建资料示例。身份记录之外它只多摆**环境根里的
/// 两份出厂文档**（TASK_79：出厂说明与手册）——那是这个家的出厂配置（主体可以删、可以改），
/// 不是"替主体建别的项目文件"。
/// 已经存在的环境按既有语义**拒绝**（不覆盖、不重置，也不把"创建失败"降级成"打开"）。
///
/// ⚠️ **两份文档写不进去时初始化如实报错**（不是静默跳过）：那一刻环境目录与户口**已经**写好，
/// 因此重试会读到 `already-open`——这条路径几乎不可达（紧邻的两次写刚刚成功），如实登记它，
/// 而不假装这一步是原子的。
pub fn create_environment(
    environment_root: &Path,
    agent: &AgentId,
) -> Result<AgentId, ManualEntryError> {
    let environment = Environment::create(environment_root, agent.clone())
        .map_err(ManualEntryError::Environment)?;
    // **出厂文档**：出厂说明（做系统消息的静态头）与手册（零注入，自己读）。
    // 摆在环境根里、与自识同层；创建之后系统不再动它们（生命周期归主体）。
    // ⚠ **创建不替主体写自识**：`self_concept` 那一份由主体自己的动词写（有一条守卫钉着它，
    // 用例名里带着人称代词、这里不写出来）——施工单里"创建时落三个文件"
    // 的那一项，在**今天的产品里就是不产出**（如实登记，不假装它产出过）。
    crate::orientation::place(environment_root)
        .map_err(|_| ManualEntryError::Environment(crate::store::StoreError::Io))?;
    Ok(environment.agent().clone())
}

/// 打开既有身份环境：身份不匹配、缺文件、结构损坏、已有写者都在这一步决定。
///
/// 它与"运行"被拆成两步，是为了让调用方能**先确认环境可用、再去装配模型**——否则一个
/// 不存在的环境会被一个"凭据缺失"顶替成另一种错误，用户拿到的结论就是错的。
pub fn open_environment(
    environment_root: &Path,
    expected_agent: &AgentId,
) -> Result<Environment, ManualEntryError> {
    Environment::open(environment_root, expected_agent.clone())
        .map_err(ManualEntryError::Environment)
}

/// **核心启动**：环境已经打开、开场输入已经解析好，模型由调用方给出。
///
/// 顺序是固定的，而且每一步都排在它该排的地方：既有 `open`（身份、写锁、损坏都在这一步
/// 决定）→ 既有 [`run_activity_in_environment`]（执行准入、资料根装配都在它内部，排在第一次
/// 模型调用与任何动作之前）→ 分类报出。
///
/// 因此模型的一次网络调用、以及任何一次动作执行，都发生在身份、准入与资料根边界**之后**；
/// 重复的活动标识也仍然在第一次模型调用之前被拒绝（那是既有确认链的性质，本模块不复制它）。
///
/// 校验与输入解析是它的两个**前置**步骤（[`ManualRun::check`] / [`ManualRun::resolve_input`]），
/// 调用方可以自己先做：这样"参数与输入写错了"能排在装配模型之前被报出，而边界判定顺序不变。
pub fn run_manual_activity_in<M: ModelPort>(
    environment: &mut Environment,
    run: &ManualRun,
    input: RunInput,
    model: &mut M,
) -> Result<ManualEnding, ManualEntryError> {
    let mut session = match open_session(environment, run, input) {
        Ok(session) => session,
        Err(error) => return Ok(ManualEnding::Refused(error)),
    };
    Ok(classify_drive(session.drive(model)))
}

/// 便利入口：校验 + 解析输入 + 打开环境 + 核心启动。夹具与简单消费者用它。
///
/// 生产命令自己分步调用同样的几个函数（参数与输入先判定、环境再打开、模型最后装配），
/// 顺序不同只影响"错先报哪一个"，执行链与分类仍然是同一段代码。
pub fn run_manual_activity<M: ModelPort>(
    run: &ManualRun,
    reader: &mut dyn Read,
    model: &mut M,
) -> Result<ManualEnding, ManualEntryError> {
    run.check()?;
    let input = run.resolve_input(reader)?;
    let mut environment = open_environment(&run.environment_root, &run.expected_agent)?;
    run_manual_activity_in(&mut environment, run, input, model)
}

// ---------------------------------------------------------------------------
// 同进程宿主输入循环
// ---------------------------------------------------------------------------

/// 便利入口：校验 + 开场输入 + 打开环境 + 宿主输入循环。
pub fn run_host_session<M: ModelPort>(
    run: &ManualRun,
    host: &mut dyn BufRead,
    model: &mut M,
) -> Result<ManualEnding, ManualEntryError> {
    run.check()?;
    // 开场输入先判定：来源冲突与超限都排在打开环境、装配模型之前被报出。
    let initial = run.host_loop_initial_input()?;
    let mut environment = open_environment(&run.environment_root, &run.expected_agent)?;
    run_host_session_in(&mut environment, run, initial, host, model)
}

/// **最小显式宿主输入循环**：建立一个会话，然后按宿主给出的动作驱动它。
///
/// 三种宿主动作都是显式的，没有一种是"隐式继续"：
///
/// - **显式输入**：一行非空文本 → 作为下一轮的 Turn 级内容呈现**一次**；
/// - **显式继续**：一个空行 → 不给新内容，直接驱动下一轮；
/// - **显式退出**：读到流结束（EOF）→ 停下，并如实报出"停在等待"。
///
/// 等待之后**绝不**自动再驱动一次：没有新输入时模型请求数不会增长。EOF 之后活动**没有**结束，
/// 环境里留下的是一个未收尾的活动——报告会说出来，不会假装它已经收场。
pub fn run_host_session_in<M: ModelPort>(
    environment: &mut Environment,
    run: &ManualRun,
    initial: RunInput,
    host: &mut dyn BufRead,
    model: &mut M,
) -> Result<ManualEnding, ManualEntryError> {
    let mut session = match open_session(environment, run, initial) {
        Ok(session) => session,
        Err(error) => return Ok(ManualEnding::Refused(error)),
    };
    drive_by_host(&mut session, host, model)
}

/// **手动宿主的**动作循环：三种宿主动作都是显式的，没有一种是"隐式继续"。
///
/// - **显式输入**：一行非空文本 → 作为下一轮的 Turn 级内容呈现**一次**；
/// - **显式继续**：一个空行 → 不给新内容，直接驱动下一轮；
/// - **显式退出**：读到流结束（EOF）→ 停下，并如实报出"停在等待"。
///
/// 等待之后**绝不**自动再驱动一次。开场输入已经在这一次调用里给出、宿主也已经明确发起，
/// 因此**第一次驱动立刻发生**（旧规矩里那一格"恢复要先等一个宿主动作"随跨进程恢复一起退役：
/// 新的一段有自己的开场，没有"盘上的输入等着被继续"这回事）。
///
/// ⚠ **它不是"宿主动作循环的唯一实现"（2026-10-06 更正，同一刀收）**：常驻宿主那一份在
/// `host_loop.rs` 的 `run_segment` 里，**策略刻意不同**——门铃 Run 没有一个可等的交互宿主，
/// 停在未收场时它**等下一次唤醒**（门铃，或主体自己约的时刻）再驱动**同一段**，
/// **不拿空输入反复问主体**（理由见 `host_loop::Host::run_segment`）。
/// 两份**都委托同一条 Run 循环**（`RunSession::drive` → Core），因此这不是"第二条执行链"；
/// 不同的是**谁替"继续"这个动作负责**：这里等一个明确的人，那里等下一声铃。
/// 判据：说"唯一"时必须说清是**哪一件事**的唯一——从前这句话把两件事说成了一件事。
fn drive_by_host<M: ModelPort>(
    session: &mut RunSession<'_>,
    host: &mut dyn BufRead,
    model: &mut M,
) -> Result<ManualEnding, ManualEntryError> {
    let mut ending = classify_drive(session.drive(model));
    while ending == ManualEnding::Unsettled {
        match read_host_step(host)? {
            // 显式退出。
            HostStep::Exit => break,
            // 显式继续：不排入任何新内容。
            HostStep::Continue => {}
            // 显式输入：排入一段新内容；排不进去就**不**驱动，宁可什么都不做。
            HostStep::Input(text) => {
                let input = RunInput::text(text).map_err(|_| ManualEntryError::InputTooLarge {
                    limit: MAX_INPUT_BYTES,
                })?;
                if let Err(error) = session.provide(input) {
                    ending = ManualEnding::Refused(error);
                    break;
                }
            }
        }
        ending = classify_drive(session.drive(model));
    }
    Ok(ending)
}

/// 宿主那一步的三种明确形状。
enum HostStep {
    /// 一行非空文本。
    Input(String),
    /// 一个空行：显式继续，不给新内容。
    Continue,
    /// 流结束：显式退出。
    Exit,
}

/// 读宿主的一行。读取是**有界**的：多读一点就足以判定超限，不会先把不受限的内容读进内存。
fn read_host_step(host: &mut dyn BufRead) -> Result<HostStep, ManualEntryError> {
    let mut line = String::new();
    let mut bounded = host.take(MAX_INPUT_BYTES as u64 + 2);
    let read = bounded
        .read_line(&mut line)
        .map_err(|_| ManualEntryError::InputUnreadable)?;
    if read == 0 {
        return Ok(HostStep::Exit);
    }
    let text = line.trim_end_matches(['\r', '\n']);
    if text.len() > MAX_INPUT_BYTES {
        return Err(ManualEntryError::InputTooLarge {
            limit: MAX_INPUT_BYTES,
        });
    }
    if text.is_empty() {
        return Ok(HostStep::Continue);
    }
    Ok(HostStep::Input(text.to_string()))
}

/// 建立一个会话：身份核对、执行准入、资料根装配都在它内部，而且都排在第一次驱动之前。
fn open_session<'a>(
    environment: &'a mut Environment,
    run: &ManualRun,
    input: RunInput,
) -> Result<RunSession<'a>, CompositionError> {
    RunSession::open(
        &run.material_root,
        environment,
        SessionRequest {
            declared_agent: &run.expected_agent,
            run: run.run.clone(),
            limits: Limits::new(run.max_turns),
            input,
            // 时间来源从宿主的当次声明里取（句柄是共享的只读引用，因此这里一次克隆就够）。
            clock: run.clock.clone(),
            context_window: run.context_window,
            reading: run.reading.clone(),
            // 可选的那几格**整包**交出去（身份行、im 凭据、模型名……各格的语义在包自己身上）。
            declarations: run.declarations.clone(),
        },
    )
}

/// 会话的一次驱动 → 入口能如实报告的形状——**不新增 Outcome、不改写类别**。
///
/// 等待与主体收场在这里就分开了：等待**没有**结局，主体收场有。
fn classify_drive(result: Result<SessionDrive, CompositionError>) -> ManualEnding {
    match result {
        Ok(SessionDrive::Ended(Outcome::Subject(ending))) => ManualEnding::Subject(ending),
        Ok(SessionDrive::Ended(Outcome::System(ending))) => ManualEnding::System(ending),
        Ok(SessionDrive::Unsettled) => ManualEnding::Unsettled,
        Err(error) => ManualEnding::Refused(error),
    }
}

// ---------------------------------------------------------------------------
// 呈现：稳定的行式文本
// ---------------------------------------------------------------------------

/// 把一次运行的参数与结局渲染成稳定的行式文本（一行一条，空格分隔，`键=值`）。
///
/// 开放文本（环境根、身份、活动标识、暂停原因、存储分类标记）按 [`escape_field`] 转义，
/// 因此值里不会出现空格、`=` 或控制字符——终端控制字符不能借这些字段伪造报告。
/// 默认**不**倾倒运行轨迹、文件正文或模型请求：机械收场与有限读数就够了。
pub fn render(run: &ManualRun, ending: &ManualEnding) -> String {
    let mut out = String::new();
    out.push_str(REPORT_HEADER);
    out.push('\n');
    let _ = writeln!(out, "mode=run");
    let _ = writeln!(out, "agent={}", escape_field(run.expected_agent.as_str()));
    let _ = writeln!(out, "run={}", escape_field(run.run.as_str()));
    let _ = writeln!(
        out,
        "environment-root={}",
        escape_field(&run.environment_root.to_string_lossy())
    );
    let _ = writeln!(
        out,
        "material-root={}",
        escape_field(&run.material_root.to_string_lossy())
    );
    let _ = writeln!(out, "max-turns={}", run.max_turns);
    let _ = writeln!(out, "input={}", run.input.as_str());
    // 这一次活动有没有创建授权，以及授权到哪个目录：报告只说事实，不替主体判断用途。
    let _ = writeln!(
        out,
        "creation={}",
        match &run.declarations.creation {
            Some(grant) => format!("create_text@{}", escape_field(grant.directory())),
            None => "none".to_string(),
        }
    );
    let _ = writeln!(out, "ending={}", ending.as_str());
    // 模型失败带分类时多一行：分类是事实，不是日志（旧记录没有分类就不打这一行）。
    if let Some(line) = model_failure_line(ending) {
        let _ = writeln!(out, "{line}");
    }

    match ending {
        ManualEnding::Subject(_) => {}
        ManualEnding::System(_) => {}
        ManualEnding::Unsettled => {
            // 这一条必须说清两件事：状态是什么，以及环境里留下了什么后果。
            let _ = writeln!(
                out,
                "note=这次驱动停在等待：活动没有结束，轨迹、轮次与预算都还在"
            );
            let _ = writeln!(
                out,
                "note=环境里因此留下一个未收尾的活动；没有新输入或显式继续时不会再调用模型"
            );
            let _ = writeln!(
                out,
                "note=要接着跑必须显式给出下一段输入或显式继续；本入口不自动续跑，也不假装正常收场"
            );
        }
        ManualEnding::Refused(error) => {
            for line in refusal_lines(error) {
                let _ = writeln!(out, "{line}");
            }
        }
    }

    let _ = writeln!(
        out,
        "diagnose-hint=environment_diagnose {} {}",
        escape_field(&run.environment_root.to_string_lossy()),
        escape_field(run.expected_agent.as_str())
    );
    out
}

/// 把一次初始化渲染成稳定的行式文本。
pub fn render_created(environment_root: &Path, requested: &AgentId, recorded: &AgentId) -> String {
    let mut out = String::new();
    out.push_str(REPORT_HEADER);
    out.push('\n');
    let _ = writeln!(out, "mode=init");
    let _ = writeln!(
        out,
        "environment-root={}",
        escape_field(&environment_root.to_string_lossy())
    );
    let _ = writeln!(out, "requested-agent={}", escape_field(requested.as_str()));
    // 报的是**环境里记录的身份**，不是回显请求参数：两者不一致时应当看得见。
    let _ = writeln!(out, "recorded-agent={}", escape_field(recorded.as_str()));
    let _ = writeln!(out, "status=created");
    out
}

/// 被拒绝时打印的细节行。每一类都保留自己的层级，并指出可以用既有诊断命令继续查。
fn refusal_lines(error: &CompositionError) -> Vec<String> {
    match error {
        CompositionError::Environment(reason) => vec![format!(
            "refused=environment reason={}",
            escape_field(reason)
        )],
        CompositionError::FileAppRefused(class) => vec![
            "refused=material-root".to_string(),
            format!("material-root-failure={}", class.as_str()),
        ],
        CompositionError::MachineRefused(reason) => vec![
            "refused=machine".to_string(),
            format!("machine-failure={}", escape_field(reason)),
        ],
        CompositionError::Run(RunError::Commit(_)) => vec![
            "refused=save-failed".to_string(),
            "note=这次活动没有被完整确认为耐久；不得据此说成成功，也不得说记录一定不存在"
                .to_string(),
        ],
        CompositionError::Run(error) => vec![
            "refused=activity".to_string(),
            format!("activity-failure={}", error.as_str()),
        ],
        CompositionError::SessionNotDrivable => vec![
            "refused=session-not-drivable".to_string(),
            "note=这次会话已经不能再被驱动：活动已经收场，或上一次驱动以失败告终".to_string(),
            "note=失败不会被再驱动一次抹成一次正常等待".to_string(),
        ],
        CompositionError::SaveFailed {
            stage,
            ending_confirmed,
            pause_recorded,
        } => vec![
            "refused=save-failed".to_string(),
            format!(
                "save-failed stage={} ending-confirmed={} pause-recorded={}",
                stage.as_str(),
                ending_confirmed,
                pause_recorded
            ),
            "note=只报告当前调用能证明的边界：这一步没有被完整确认".to_string(),
            "note=不重发已发出的动作；不声称记录不存在，也不声称行动已撤销".to_string(),
        ],
    }
}

/// 带分类的模型失败在报告里的**一行稳定输出**（有分类才打）。
///
/// 分类是事实不是日志：以前它只留在适配层的一行 stderr 里，跨进程就丢；现在它跟着收场一路
/// 到这里，终端因此能直接报出来。分类**没有**被记录时（旧记录）**不打**这一行——不编造。
///
/// 退出码表**不因它改变**：本函数只多输出一行读数。
fn model_failure_line(ending: &ManualEnding) -> Option<String> {
    match ending {
        ManualEnding::System(SystemEnding::ModelFailed(Some(class))) => {
            Some(format!("model-failure={}", escape_field(class)))
        }
        _ => None,
    }
}

/// 把开放文本转义成**一个**字段。
///
/// 输出是一行一条、空格分隔的 `键=值`，所以值里不能出现空格、`=` 或控制字符。转义是机械的：
/// `\` → `\\`、换行 → `\n`、回车 → `\r`、制表 → `\t`、空格 → `\s`、`=` → `\e`；其余字符原样。
pub fn escape_field(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ' ' => out.push_str("\\s"),
            '=' => out.push_str("\\e"),
            other => out.push(other),
        }
    }
    out
}
