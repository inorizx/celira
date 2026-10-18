//! Agent 工程地基：事实、账本、操作身份、结果与恢复。
//!
//! 这一层只处理现实可靠性：什么算已经发生的事实、事实怎么按稳定身份追加与去重、
//! 一次外部尝试现在处于什么状态、中断后怎么只凭事实重建可验证状态。
//!
//! 这一层不做 Agent 的判断：不决定做什么、不排序、不总结、不生成长期记忆。
//!
//! 本轮只提供内存实现，不提供文件或数据库实现，也不宣称持久化已经完成。

use std::collections::BTreeSet;
use std::fmt;

/// Agent 的稳定身份。事实、活动与操作都按它隔离。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentId(String);

impl AgentId {
    /// 建立一个稳定身份。
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// 身份的不透明文本形式。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 一次有限活动的身份。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(String);

impl RunId {
    /// 建立一个活动身份。
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// 身份的不透明文本形式。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 一次活动内部的第几次处理。序号只能单调前进。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Turn(u32);

impl Turn {
    /// 活动开始之前不存在的 Turn。
    pub(crate) const NONE: Turn = Turn(0);

    /// 任何一个活动的第一个 Turn。
    pub const FIRST: Turn = Turn(1);

    /// 从一个已确认的序号还原 Turn。
    ///
    /// `0` 是"活动开始之前不存在的 Turn"，因此返回 `None`；`1..=u32::MAX` 精确保留原数值，
    /// 常数时间构造（不步进、不推算）。
    ///
    /// 它**只**回答"这个数字能不能表示成一个 Turn"，不回答"这次活动是否真的走到过这一轮"：
    /// `u32` 入参本身就有表示范围，能构造出来不等于历史里发生过。
    pub const fn from_number(number: u32) -> Option<Turn> {
        if number == 0 {
            None
        } else {
            Some(Turn(number))
        }
    }

    /// Turn 的序号。
    pub const fn number(self) -> u32 {
        self.0
    }

    pub(crate) const fn advance(self) -> Turn {
        Turn(self.0 + 1)
    }
}

/// 一次外部尝试的稳定身份。
///
/// 同一个 (活动, Turn, 序号) 永远得到同一个身份：重试不得改变操作身份，因此把同一次尝试
/// 的事实重复追加进账本，会被判定为重复而不是新事实。
///
/// **序号是什么**：一轮里模型可以请求**多个**动作（原生协议本来就允许），它们按模型给出的顺序
/// 依次执行，各自是一次独立的外部尝试。序号就是"这一轮里第几个请求"，**从 0 起**，而且**不是**
/// 轮次的一部分——[`Turn`] 的语义（"这是第几次模型调用"）一个字都没有变。
///
/// **第 0 个沿用既有形状**：`index == 0` 时盘上不写这个字段（见事实行的编码规则），
/// 因此"一轮一个动作"这种常规情形在盘上与从前**逐字节相同**，旧环境读出来就是"每轮第 0 个动作"。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperationId {
    run: RunId,
    turn: Turn,
    index: u16,
}

impl OperationId {
    /// 用活动身份与 Turn 推出操作身份（**这一轮的第 0 个**请求）。
    pub fn new(run: RunId, turn: Turn) -> Self {
        Self {
            run,
            turn,
            index: 0,
        }
    }

    /// 用活动身份、Turn 与**同一轮内的序号**推出操作身份。
    ///
    /// 序号从 0 起；`index == 0` 与 [`OperationId::new`] 完全等价。
    pub fn at(run: RunId, turn: Turn, index: u16) -> Self {
        Self { run, turn, index }
    }

    /// 这次尝试属于哪个活动。
    pub fn run(&self) -> &RunId {
        &self.run
    }

    /// 这次尝试属于哪个 Turn。
    pub fn turn(&self) -> Turn {
        self.turn
    }

    /// 这次尝试是这一轮里的**第几个**请求（从 0 起）。
    pub fn index(&self) -> u16 {
        self.index
    }
}

/// 一次外部尝试回来时的结果类别。三种类别互斥，不能互相顶替。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResultState {
    /// 外部给出了明确的成功结果。
    Success,
    /// 外部给出了明确的失败结果。
    Failure,
    /// 外部是否真的发生过无法确认。不得被猜成成功或失败。
    Unknown,
}

/// 一次外部尝试的结果事实。
///
/// 它只记录"操作身份 + 结果类别"，不携带任何外部业务字段或错误码文本。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionResult {
    operation: OperationId,
    state: ResultState,
}

impl ActionResult {
    /// 为某次操作记录一个结果类别。
    pub fn new(operation: OperationId, state: ResultState) -> Self {
        Self { operation, state }
    }

    /// 这是哪一次尝试的结果。
    pub fn operation(&self) -> &OperationId {
        &self.operation
    }

    /// 结果类别。
    pub fn state(&self) -> ResultState {
        self.state
    }

    /// 这个结果是否已经把这次尝试判定为明确成功或明确失败。
    pub fn is_confirmed(&self) -> bool {
        self.state != ResultState::Unknown
    }
}

/// 主体自己选择的收场方式。它不是系统失败。
///
/// ⚠ **2026-10-10 收口（主人裁决）**：**主体收场只有一档——`Ended`**。
/// 「为什么结束」归**主体自己的记录**（笔记），不归终态；终态**写一次就封，下一次没人读它**
/// （见 `RUNTIME_TERMINAL_DECISIONS.md` §6 的 2026-10-10 收口）。
///
/// **下面那四个是旧值**：它们**照读不写**——账只追加，改一次主体就不是原来那个主体了（判据 22）。
/// 今天**唯一的生产者**是适配层（主体那个 `finish_run` 动词**不带参数**），因此新记录里
/// 只可能出现 `Ended`；四个旧值只为读回 2026-10-10 之前的历史而留着。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubjectEnding {
    /// **主体收场了**（不带理由：结束了就是结束了）。
    Ended,
    /// **主体搁下了**（不带理由）：不是"干完了"，也不是失败——是"我停在这儿，有事再叫我"。
    ///
    /// ⚠ 它与 [`SubjectEnding::Ended`] **机械上同一件事**（都结束这一段、都封存），
    /// **区别只在词**：那个词进账，下一段的开场材料据此说"上一段是你搁下的，不是断了"。
    /// ⚠ 它与**旧值 `Waiting` 的区别**：`Waiting` 声称"我在等某个外部结果"（那句话是假的，
    /// 真机上被当成"暂停"用了 9 次）；`Held` **不声称任何期待**，只声称"我停在这儿"。
    Held,
    /// 旧值（照读不写）：主体认为这次活动已经完成。
    Completed,
    /// 旧值（照读不写）：主体选择不回应。沉默不是失败。
    Silent,
    /// 旧值（照读不写）：主体选择拒绝这次活动。
    Declined,
    /// 旧值（照读不写）：主体选择等待外部结果，先结束这一次活动。
    Waiting,
}

/// 系统层面的收场方式。系统结果不是主体选择。
///
/// 模型失败与行动失败分开，因为错误必须保留它发生的层级。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemEnding {
    /// 模型这一层没有给出结果。**唯一带载荷的收场值**：`Some(_)` = 本次失败带了分类；
    /// `None` = 分类没有被记录（旧记录就是这一形态——引入新维度必须兼容旧口径，**不编造**）。
    ///
    /// ## 载荷为什么是一根**稳定字符串**，而不是那个分类枚举
    ///
    /// 那套词表（`core::ModelFailureClass` 与它那张封闭解码表）归 **Core**：它是**模型接缝**的
    /// 词汇——会说传输状态码、工具名这些**线路层**事实，**不属于执行地基**。这里存的只是它 `as_str()`
    /// 给出的那个**稳定名**，Foundation **一个字都不解释**。
    ///
    /// 三条理由，各自独立成立：
    ///
    /// 1. **盘上本来就是字符串**：持久化那一层的 `Ending.failure` 存的就是这个稳定名，读回时在
    ///    那一层用封闭表校验（认不出 ⇒ `illegal-failure` 整行拒绝）。让内存表示与盘上表示一致，
    ///    中间那次枚举 ↔ 字符串的来回就没有了；
    /// 2. **它让 Foundation 不再认识 Core**（`AGENTS.md` 的硬线："Foundation 不依赖 Core"）。
    ///    从前这一个字段是两个文件之间**唯一**的环——Core 从 Foundation 拿 12 样东西（上层用下层，
    ///    合法），Foundation 却反过来伸手够 Core 的一个类型。环的意思是：**规则书一改，账本跟着改**，
    ///    而账本（盘上格式）本该是最不容易变的东西；
    /// 3. **它把那条硬线变成可机械检查的**：守卫从前把 `core.rs` 与 `foundation.rs` 当成**一坨**
    ///    （`CORE_SOURCES`），因此只能问"这一坨有没有碰 adapter / app / 骨干"，**问不出内部谁依赖谁**
    ///    ——也就是说"Foundation 不依赖 Core"这条红线**当时没有任何守卫**，它已经被违反而无人发现。
    ///
    /// ⚠️ **取值永远来自那份词表**：写侧由 Core 用 `as_str()` 给出；读侧由持久化与恢复那两层
    /// 用封闭表 `from_str` **校验过**之后取它的规范形态。因此这里不会出现一个词表之外的字符串——
    /// 但**这一条靠的是上面那两处写点，不是靠类型**（这正是"词表不跨层"要付的代价，如实记在这里）。
    ModelFailed(Option<&'static str>),
    /// 一次外部尝试明确失败。
    ActionFailed,
    /// 活动被外部取消。
    Cancelled,
    /// 活动在资源边界内没有走完，或外部迟迟没有结果。
    Timeout,
    /// 承载活动的进程崩溃。
    Crashed,
    /// 外部结果无法确认，活动带着未确认效果收场。
    Indeterminate,
}

/// 一次活动是如何结束的。主体选择与系统失败必须分开。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// 主体自己选择的收场。
    Subject(SubjectEnding),
    /// 系统层面的收场或失败。
    System(SystemEnding),
}

/// 需要恢复或验证的耐久事实。它不是调试日志。
///
/// 只有一条闭环里"重启后仍然必须成立"的事情才进来：活动是否开始、一次外部尝试
/// 是否准备、是否已经发出、结果是什么、活动是否已经收场。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    /// 某次活动已经由某个 Agent 开始。这里是活动归属的唯一来源。
    RunStarted { agent: AgentId, run: RunId },
    /// 一次外部尝试在发出之前已经准备。
    ActionPrepared { run: RunId, operation: OperationId },
    /// 一次外部尝试已经发出，结果还没有回来。
    ActionDispatched { run: RunId, operation: OperationId },
    /// 一次外部尝试的结果已经确认（成功、失败或未知）。
    ActionResultRecorded { run: RunId, action: ActionResult },
    /// 一次活动已经以明确的 Outcome 收场。
    RunEnded {
        run: RunId,
        outcome: Outcome,
        /// **这一次的代价**（时长 ＋ 用量）。
        ///
        /// 两格各自独立可选，缺的就是没有被记（旧记录、恢复路径就是这一形态）⇒ **不编造**。
        /// 它是**搬运**不是解释：Core 把这个读数带到这里，不换算、不计算、不判断（见
        /// [`crate::cost`]：金额是派生事实，账本记事实不记汇率）。
        cost: crate::cost::RunCost,
    },
}

impl Record {
    /// 这条事实属于哪个活动。
    pub fn run(&self) -> &RunId {
        match self {
            Record::RunStarted { run, .. }
            | Record::ActionPrepared { run, .. }
            | Record::ActionDispatched { run, .. }
            | Record::ActionResultRecorded { run, .. }
            | Record::RunEnded { run, .. } => run,
        }
    }

    /// 机械去重键：同一个键只允许出现一次。它不是领域概念，只服务"追加不得静默覆盖"。
    fn key(&self) -> RecordKey {
        match self {
            Record::RunStarted { run, .. } => RecordKey::RunStarted(run.clone()),
            Record::ActionPrepared { operation, .. } => {
                RecordKey::ActionPrepared(operation.clone())
            }
            Record::ActionDispatched { operation, .. } => {
                RecordKey::ActionDispatched(operation.clone())
            }
            Record::ActionResultRecorded { action, .. } => {
                RecordKey::ActionResult(action.operation().clone())
            }
            Record::RunEnded { run, .. } => RecordKey::RunEnded(run.clone()),
        }
    }
}

/// 事实的机械去重键。
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum RecordKey {
    RunStarted(RunId),
    ActionPrepared(OperationId),
    ActionDispatched(OperationId),
    ActionResult(OperationId),
    RunEnded(RunId),
}

/// 账本拒绝一次追加的原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerError {
    /// 同一事实已经存在。旧事实没有被覆盖，新事实也没有落地。
    Duplicate,
    /// 这条事实属于另一个 Agent。
    ForeignAgent,
    /// 这条事实的活动还没有在本账本登记开始。
    UnregisteredRun,
}

/// 内存版事实账本：追加式、按 Agent 隔离、按稳定身份去重。
///
/// 它是"事实"的唯一入口：只接受 Record，不覆盖、不重排、不解释。
pub struct Ledger {
    owner: AgentId,
    runs: BTreeSet<RunId>,
    seen: BTreeSet<RecordKey>,
    records: Vec<Record>,
}

impl Ledger {
    /// 为一个 Agent 建立账本。账本只接受这个 Agent 的事实。
    pub fn new(owner: AgentId) -> Self {
        Self {
            owner,
            runs: BTreeSet::new(),
            seen: BTreeSet::new(),
            records: Vec::new(),
        }
    }

    /// 账本属于哪个 Agent。
    pub fn owner(&self) -> &AgentId {
        &self.owner
    }

    /// 按追加顺序读回全部事实。
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// 追加一条事实。
    ///
    /// 追加不会覆盖任何旧事实；重复的事实、别的 Agent 的事实、以及活动尚未登记的
    /// 事实都会被拒绝。
    pub fn append(&mut self, record: Record) -> Result<(), LedgerError> {
        match &record {
            Record::RunStarted { agent, run } => {
                if *agent != self.owner {
                    return Err(LedgerError::ForeignAgent);
                }
                self.runs.insert(run.clone());
            }
            other => {
                if !self.runs.contains(other.run()) {
                    return Err(LedgerError::UnregisteredRun);
                }
            }
        }

        if !self.seen.insert(record.key()) {
            return Err(LedgerError::Duplicate);
        }

        self.records.push(record);
        Ok(())
    }

    /// 只从已追加的事实重建状态，不读取任何其他内存状态。
    pub fn recover(&self) -> Result<Recovered, RecoveryError> {
        recover(&self.records)
    }
}

/// 从事实重建出的可验证状态快照。
///
/// 它只回答"现在能确认什么"，不猜测任何不确定的事情。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recovered {
    /// 这些事实属于哪个 Agent。
    pub agent: AgentId,
    /// 这些事实讲的是哪次活动。
    pub run: RunId,
    /// 活动是否已经收场，以及怎么收场的。`None` 表示还在进行中。
    pub outcome: Option<Outcome>,
    /// 事实里最后一个留下了行动事实的 Turn；没有任何行动事实时为 `None`。
    ///
    /// 这是事实的机械投影，不等于"活动走到第几个 Turn"：一次活动完全可能在最后
    /// 一个 Turn 上直接收场，而不留下任何行动事实。
    pub last_action_turn: Option<Turn>,
    /// 结果无法确认的外部尝试：已经准备但没有明确结果，或者结果是"未知"。
    /// 这些效果可能已经发生，恢复时不得重复发出。
    pub unconfirmed: Vec<OperationId>,
}

impl Recovered {
    /// 活动是否已经以明确结果收场。
    pub fn is_ended(&self) -> bool {
        self.outcome.is_some()
    }

    /// 是否存在结果无法确认的外部效果。
    pub fn has_unconfirmed_effects(&self) -> bool {
        !self.unconfirmed.is_empty()
    }

    /// 恢复之后是否还允许再次发出外部效果。
    pub fn may_dispatch_again(&self) -> bool {
        self.unconfirmed.is_empty() && !self.is_ended()
    }
}

/// 事实序列自相矛盾时重建会拒绝继续。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryError {
    /// 没有任何新增事实，重建不出任何活动。
    NoRecords,
    /// 事实里出现了两个不同的 Agent。
    MixedAgents,
    /// 事实里出现了两次不同的活动开始。
    MultipleRuns,
    /// 一条事实提到的活动与已经开始的不是同一个。
    ForeignRun(RunId),
    /// 一次尝试还没有准备就已经发出。
    DispatchWithoutPreparation(OperationId),
    /// 一次尝试还没有准备就已经有结果。
    ResultWithoutPreparation(OperationId),
    /// 一次活动出现了两个收场事实。
    MultipleEndings,
}

/// 只从事实重建可验证状态。
///
/// 重建不重新生成、不重新猜测、不丢弃事实：它按追加顺序读事实，检查顺序是否自相
/// 矛盾，然后回答"活动是否收场、哪些效果无法确认"。没有明确结果的效果一律留在
/// `unconfirmed` 里，不会被改写成成功、失败或无事发生。
pub fn recover(records: &[Record]) -> Result<Recovered, RecoveryError> {
    let mut owner: Option<AgentId> = None;
    let mut run: Option<RunId> = None;
    let mut outcome: Option<Outcome> = None;
    let mut last_action_turn: Option<Turn> = None;
    let mut prepared: BTreeSet<OperationId> = BTreeSet::new();
    let mut confirmed: BTreeSet<OperationId> = BTreeSet::new();

    for record in records {
        if let Some(started) = &run
            && record.run() != started
        {
            return Err(RecoveryError::ForeignRun(record.run().clone()));
        }

        match record {
            Record::RunStarted {
                agent,
                run: started,
            } => {
                if owner.as_ref().is_some_and(|existing| existing != agent) {
                    return Err(RecoveryError::MixedAgents);
                }
                if run.as_ref().is_some_and(|existing| existing != started) {
                    return Err(RecoveryError::MultipleRuns);
                }
                owner = Some(agent.clone());
                run = Some(started.clone());
            }
            Record::ActionPrepared { operation, .. } => {
                last_action_turn = later(last_action_turn, operation.turn());
                prepared.insert(operation.clone());
            }
            Record::ActionDispatched { operation, .. } => {
                last_action_turn = later(last_action_turn, operation.turn());
                if !prepared.contains(operation) {
                    return Err(RecoveryError::DispatchWithoutPreparation(operation.clone()));
                }
            }
            Record::ActionResultRecorded { action, .. } => {
                last_action_turn = later(last_action_turn, action.operation().turn());
                if !prepared.contains(action.operation()) {
                    return Err(RecoveryError::ResultWithoutPreparation(
                        action.operation().clone(),
                    ));
                }
                if action.is_confirmed() {
                    confirmed.insert(action.operation().clone());
                }
            }
            Record::RunEnded { outcome: ended, .. } => {
                if outcome.is_some() {
                    return Err(RecoveryError::MultipleEndings);
                }
                outcome = Some(*ended);
            }
        }
    }

    match (owner, run) {
        (Some(agent), Some(run)) => {
            let unconfirmed = prepared.difference(&confirmed).cloned().collect();
            Ok(Recovered {
                agent,
                run,
                outcome,
                last_action_turn,
                unconfirmed,
            })
        }
        _ => Err(RecoveryError::NoRecords),
    }
}

/// 记下更靠后的那个 Turn。
fn later(previous: Option<Turn>, turn: Turn) -> Option<Turn> {
    Some(match previous {
        Some(known) => known.max(turn),
        None => turn,
    })
}
