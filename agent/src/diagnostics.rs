//! 只读诊断：把已确认的持久事实重读成"操作到了哪一步"，并把**操作解释**与**存储状态**
//! 分成两个来源如实报告。
//!
//! 这一层回答两件事，而且严格分开来源：
//!
//! | 来源 | 能回答什么 | 不能推导什么 |
//! |---|---|---|
//! | 事实 → `Record` → 既有单活动解释器 | 活动归属、已确认结束、行动事实与未确认操作 | 经历是否封存、任务是否应该继续 |
//! | 既有存储状态读数 | 经历开始/结束/封存标记、暂停记录、执行准入、可判定的跨存储矛盾 | 外部操作真实成功、可安全重发 |
//!
//! **它不恢复任何东西。** 恢复、重发、续跑、解除暂停都不在这里，也不被这里的任何结论
//! 推导出来：报告说的只是"已经确认到哪里"。
//!
//! 关于"只读、不执行"，三类证据各自证明什么必须**分开**说：
//!
//! - **类型限制**证明的是"不能写这个环境"：诊断函数只接受 [`EnvironmentReader`]（一个**没有**
//!   任何写方法、也不持有追加句柄的只读句柄），而追加经历/事实、写暂停标记、清除暂停都要写者
//!   那一侧的句柄。⚠ **2026-10-06 起它比从前更强**：从前是 `&Environment`（不可变借用），而拿到
//!   那个借用**必须先取得排他写资格**——于是"看一眼处境"的命令在主体正在跑活动时读到的是
//!   `already-open`。现在读者**不取写资格**，读的是已确认前缀（见 `store` 的 `TailPolicy`）。
//!   它**不**证明"模块里
//!   不存在任何执行调用"——构造一个模型替身并不需要环境，所以那个结论单靠类型得不到；
//! - **调用链审查**（静态、文本级）证明的是"这条路径上没有执行入口"：本模块与诊断命令里
//!   没有引用唯一那条执行循环，也没有引用模型端口或行动端口
//!   （`tests/environment_diagnostics.rs` 有一条带反例自检的检查盯着它）。这是窄条件，
//!   不是链接期证明；
//! - **文件对照**（行为）证明的是"这一次调用没有改盘上状态"：调用前后两份存储、身份记录
//!   与写锁的字节、目录成员与准入读数都不变。它**不**证明"任何未来路径都不会执行"。
//!
//! 另外，打开环境本身可能全量解码整份经历前缀（见 `store` 的说明）。诊断**不读经历正文**、
//! 不从正文推断结果；但"底层的读取"这件事必须如实承认，不能用"只读"把它说成没发生。
//!
//! ## 有界与分页
//!
//! 活动列表与单个活动的事实详情都返回**有界**结果，并且显式给出下一段游标，不静默截断：
//!
//! - [`ACTIVITY_PAGE_LIMIT`]：一次最多返回多少个活动（单位：个）；
//! - [`FACT_PAGE_LIMIT`]：一个活动的事实详情一次最多返回多少行（单位：行）。
//!
//! 分页只影响"返回哪些行/哪些活动"：操作解释**始终**基于该活动的全部事实序列，
//! 不把一页片段当成完整活动来解释。
//!
//! 报告里还有两份**附属列表**（经历侧活动摘要、可判定的跨存储矛盾）。它们同样不整份返回，
//! 而是**与活动分页同界**：只覆盖本页活动，整份条数另列在 [`StorageReading`] 的 `*_total`
//! 字段里。把活动分页走完之后，两份列表的并集与整份存储逐一对应——不丢项，也不会因为只看
//! 一页就拿到一份没有上限、与这一页无关的清单。
//!
//! **返回有界不等于底层读取内存有界**：底层读取仍然把整份事实读入内存，诊断不为分页
//! 顺手重构存储。

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::foundation::{
    AgentId, OperationId, Outcome, Record, Recovered, RecoveryError, ResultState, RunId,
    SystemEnding, recover,
};
// 收场标记（`subject:*` / `system:*`）的词与形状的权威在 `store`（盘上的封闭表就在那里），
// 本件不另写一份：它只调 `outcome_label`（2026-10-06 词表立权威那一刀）。
use crate::store::{
    Admission, CrossStoreIssue, EnvironmentReader, FactRow, FactRowError, RunSummary, StoreError,
    TraceRecord, outcome_label, records_of_run,
};

/// 一次最多返回多少个活动（单位：个）。
pub const ACTIVITY_PAGE_LIMIT: usize = 32;

/// 一个活动的事实详情一次最多返回多少行（单位：行）。
pub const FACT_PAGE_LIMIT: usize = 64;

/// 报告首行：让调用方能认出格式版本，而不是靠猜字段。
const REPORT_HEADER: &str = "celira-environment-diagnose v1";

/// 一个活动在**两个存储**里各自是否出现。
///
/// 两个来源必须分开说：只在事实侧出现的活动是"经历侧没有对应锚点"，只在经历侧出现的
/// 活动是"没有可解释的操作事实"。把两者压成"这个活动存在"会掩盖这两种情况。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivitySides {
    /// 两份存储里都能看到。
    BothStores,
    /// 只有事实侧有：经历侧没有对应锚点。
    FactsOnly,
    /// 只有经历侧有：没有任何可解释的操作事实。
    TraceOnly,
}

impl ActivitySides {
    /// 稳定分类标记。
    pub fn as_str(&self) -> &'static str {
        match self {
            ActivitySides::BothStores => "both-stores",
            ActivitySides::FactsOnly => "facts-only",
            ActivitySides::TraceOnly => "trace-only",
        }
    }
}

/// 从**已验证事实**机械得到的行动事实分类。
///
/// 它是事实分类，不是第二套恢复裁决器：每个计数只数事实行的种类，不解释原因，也不推断
/// 外部效果是否真的发生。`prepared_without_dispatch` 与 `dispatched_without_result` 说明的是
/// "记录停在哪一步"，**不**说明"可以安全重试"——已经发出过的尝试不得因为这里读出缺口而重发。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActionCounts {
    /// 已经确认"准备"过的尝试数。
    pub prepared: usize,
    /// 已经确认"发出"过的尝试数。
    pub dispatched: usize,
    /// 记录结果为明确成功的尝试数。
    pub success: usize,
    /// 记录结果为明确失败的尝试数。
    pub failure: usize,
    /// 记录结果为"未知"的尝试数。
    pub unknown: usize,
    /// 准备过但没有发出记录的尝试数。
    pub prepared_without_dispatch: usize,
    /// 发出过但没有结果记录的尝试数。
    pub dispatched_without_result: usize,
}

/// 一个活动的完整操作读数。
///
/// `fact_rows` 是该活动在事实存储里的**全部**行数，与分页返回多少行无关；
/// `recovered` 是对**全部**事实序列调用既有单活动解释器的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationReading {
    /// 这个活动在事实存储里的全部行数。
    pub fact_rows: usize,
    /// 机械分类出的行动事实计数。
    pub counts: ActionCounts,
    /// 既有解释器对完整序列的结论（`Err` 表示事实能重建、但序列自相矛盾）。
    pub recovered: Result<Recovered, RecoveryError>,
}

/// 一个活动的诊断条目。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityDiagnosis {
    /// 活动身份。
    pub run: RunId,
    /// 这个活动在两个存储里各自是否出现。
    pub sides: ActivitySides,
    /// 事实无法重建时的明确原因；`Ok` 时才谈得上解释。
    pub reading: Result<OperationReading, FactRowError>,
}

/// 存储状态来源：直接来自身份环境已经提供的**只读**状态读数。
///
/// 下面两份附属列表**与活动分页同界**：它们只覆盖**本页活动**，不整份返回。整份存储的机械
/// 总数另列在 `cross_store_total` / `trace_activities_total`，所以"本页看不到"与"整份没有"
/// 可以分开判读；把活动分页走完之后，两份列表的并集与整份存储逐一对应，不丢不重。
///
/// 为什么必须按页收窄：附属列表里的每一项本来都挂在某个活动上
/// （[`CrossStoreIssue::run`] / [`RunSummary::run`]），整份返回会让一个只看一页活动的调用方
/// 拿到与它无关、而且没有上限的清单——那正是绕过分页。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageReading {
    /// 既有执行准入读数。
    pub admission: Admission,
    /// 经历存储的已确认前缀条数。
    pub trace_records: usize,
    /// 事实存储的已确认前缀条数。
    pub fact_records: usize,
    /// 是否留下"保存失败暂停"标记。
    pub paused: bool,
    /// **本页活动**里可判定的矛盾。空表示"本页没有发现"，不表示整份存储已证明一致。
    pub cross_store: Vec<CrossStoreIssue>,
    /// 整份存储里可判定矛盾的条数（与是否分页无关）。
    pub cross_store_total: usize,
    /// **本页活动**在经历侧的机械摘要（存储层自己的视图）。
    pub trace_activities: Vec<RunSummary>,
    /// 整份存储里经历侧活动的条数（与是否分页无关）。
    pub trace_activities_total: usize,
    /// **活动一共几个**——本页所在的那个**全集**（事实侧 ∪ 经历侧，与 `order` 同源）。
    ///
    /// ⚠ **它才是 `of=` 该报的那一格**（2026-10-13 修正）：从前的 `of=` 报的是
    /// [`StorageReading::trace_activities_total`]（**只数经历侧**），而本页取的是
    /// **两份存储的并集** ⇒ 真机上出现过 `activities-page=25 of=4` 这种**本页比"一共"还多**
    /// 的读数：两个数**不可比却并排报**。判据「人不许比系统知道得更少」⇒ 两个数都留着
    /// （各自的名字说清自己是什么），但 `of=` 这个**名字**只属于全集。
    pub activities_total: usize,
}

/// 这一页活动取自账本的**哪一头** —— `has_more` 与 `next_after` 的含义由它定。
///
/// ⚠ 为什么这是一个显式字段而不是隐知识（2026-09-29 真机实测，源自"活动次数冻结在 32"那条）：
/// 同一个 `has_more` 在两个方向上说**相反**的话 —— 从最早那头取时说"还有更新的"，从最近那头
/// 取时说"还有更早的"。把它塞进布尔参数或让调用方自己记，迟早有人读反。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityWindow {
    /// 从**最早**那一头取的页（`environment_diagnose --after` 逐页往后翻走这条）。
    ///
    /// `has_more` = 还有**更新**的活动；`next_after` = 下一页（更晚）的游标。
    FromStart,
    /// 从**最近**那一头取的页（`agent_host_status` 的"看一眼处境"走这条）。
    ///
    /// `has_more` = 还有**更早**的活动；`next_after` 恒为 `None` —— 这条入口不提供向后翻。
    Latest,
}

/// 环境级诊断报告。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvironmentDiagnosis {
    /// 环境的权威身份。
    pub agent: AgentId,
    /// 存储状态来源。
    pub storage: StorageReading,
    /// 有界的一页活动（事实侧与经历侧的并集）。
    pub activities: Vec<ActivityDiagnosis>,
    /// 这一页取自账本的哪一头。
    pub window: ActivityWindow,
    /// 除本页之外、**`window` 指的那一头**是否还有活动（方向见 [`ActivityWindow`]）。
    pub has_more: bool,
    /// 下一页的游标（只有 `window == FromStart` 且 `has_more` 为真时才有）。
    pub next_after: Option<RunId>,
}

/// 单个活动的诊断详情：完整操作解释 + 有界的事实分页。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivityDetail {
    /// 这个活动的操作解释（基于全部事实行）。
    pub diagnosis: ActivityDiagnosis,
    /// 这一页的事实行。
    pub rows: Vec<FactRow>,
    /// 后面是否还有未返回的事实行。
    pub has_more: bool,
    /// 下一页的游标（`has_more` 为真时才有）。
    pub next_from: Option<u64>,
}

/// 诊断失败的原因。每一类保留自己的层级，不压成一个字符串。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticError {
    /// 环境本身读不出来（缺失、身份不匹配、结构损坏、已有写者）。
    Storage(StoreError),
    /// 指定的活动既不在事实侧、也不在经历侧。
    UnknownActivity(RunId),
    /// 指定的分页游标不是一个已知活动。未知游标与"已经取完"必须分开。
    UnknownCursor(RunId),
    /// 分页参数不在允许范围内（`0` 或超过上限）。超限不静默截断，而是明确失败。
    InvalidLimit { limit: usize, max: usize },
}

impl DiagnosticError {
    /// 稳定分类标记。
    pub fn as_str(&self) -> &'static str {
        match self {
            DiagnosticError::Storage(_) => "storage",
            DiagnosticError::UnknownActivity(_) => "unknown-activity",
            DiagnosticError::UnknownCursor(_) => "unknown-cursor",
            DiagnosticError::InvalidLimit { .. } => "invalid-limit",
        }
    }
}

impl From<StoreError> for DiagnosticError {
    fn from(error: StoreError) -> Self {
        DiagnosticError::Storage(error)
    }
}

/// 在一个**已经打开**的环境上产出只读诊断：有界的一页活动 + 每个活动的完整操作解释。
///
/// 它可以用在 `Ready`、`Paused`、`Unfinished` 三种处境上：诊断**不**调用执行准入来禁止读数，
/// 也不修改准入。活动列表是事实侧与经历侧的**并集**（只按事实侧枚举会漏掉只在经历侧出现的
/// 活动，只按经历侧枚举会漏掉事实侧孤立活动）。
///
/// 活动顺序是**机械的**：事实侧按首次出现的追加顺序在前，只在经历侧出现的活动按经历侧的
/// 首次出现顺序接在后面；不按重要性、不按结束时间排序。
///
/// `after` 是上一页最后一个活动（`None` 表示从第一个开始）；未知游标明确失败。
pub fn diagnose_environment(
    reader: &EnvironmentReader,
    after: Option<&RunId>,
    limit: usize,
) -> Result<EnvironmentDiagnosis, DiagnosticError> {
    let ledger = read_ledger(reader)?;
    let start = match after {
        Some(cursor) => ledger
            .order
            .iter()
            .position(|run| run == cursor)
            .map(|position| position + 1)
            .ok_or_else(|| DiagnosticError::UnknownCursor(cursor.clone()))?,
        None => 0,
    };
    ledger.diagnose(reader, start, limit, ActivityWindow::FromStart)
}

/// 取**最近**的一页活动 —— "看一眼 Agent 的处境"要的是这一头。
///
/// ⚠ **2026-09-29 真机实测新增**：在此之前 `agent_host_status` 用 `diagnose_environment(None, …)`
/// 取到的其实是**最早**那一页，于是活动数超过一页之后：
/// 呈现层把"第一页 32 条"当成"一共 32 条"报出去（真家实测 36 条报 32 条，界面据此显示
/// 「活动次数 32（总数照给…）」），而且**最近的活动一条都看不见** —— 连 `needs_action`
/// 都是在最早那一页上算的，最新几个活动里有没有问题它根本不判断。
///
/// 与 `diagnose_environment` 分开而不是加开关：两者要的是账本的**两头**，"从哪一头"会改变
/// `has_more` 的含义（见 [`ActivityWindow`]），合成一个布尔参数只会把那个方向变成隐知识。
/// 逐页往后翻的协议一字未动 —— 它属于 `environment_diagnose`，那里有真实的翻页消费者。
pub fn diagnose_latest_activities(
    reader: &EnvironmentReader,
    limit: usize,
) -> Result<EnvironmentDiagnosis, DiagnosticError> {
    let ledger = read_ledger(reader)?;
    let start = ledger.order.len().saturating_sub(limit);
    ledger.diagnose(reader, start, limit, ActivityWindow::Latest)
}

/// 一次读到的账本：两份存储读数 + **按首次出现顺序**排好的活动。
struct Ledger {
    facts: Vec<FactRow>,
    trace: Vec<TraceRecord>,
    order: Vec<RunId>,
}

fn read_ledger(reader: &EnvironmentReader) -> Result<Ledger, DiagnosticError> {
    let facts = reader.facts()?;
    let trace = reader.trace()?;
    let order = activity_order(&facts, &trace);
    Ok(Ledger {
        facts,
        trace,
        order,
    })
}

impl Ledger {
    /// 从 `start` 起取 `limit` 条；`window` 决定"还有没有"与"游标指哪边"。
    fn diagnose(
        &self,
        reader: &EnvironmentReader,
        start: usize,
        limit: usize,
        window: ActivityWindow,
    ) -> Result<EnvironmentDiagnosis, DiagnosticError> {
        if limit == 0 || limit > ACTIVITY_PAGE_LIMIT {
            return Err(DiagnosticError::InvalidLimit {
                limit,
                max: ACTIVITY_PAGE_LIMIT,
            });
        }

        let traced: BTreeSet<&str> = self
            .trace
            .iter()
            .map(|record| record.run().as_str())
            .collect();
        let page: Vec<RunId> = self.order.iter().skip(start).take(limit).cloned().collect();
        let (has_more, next_after) = match window {
            // 还有**更晚**的没列出来：游标指本页最后一条，下一页从它往后接。
            ActivityWindow::FromStart => {
                let more = self.order.len() > start + page.len();
                (more, if more { page.last().cloned() } else { None })
            }
            // 还有**更早**的没列出来：这条入口不给向后翻的游标（没有消费者，不预建）。
            ActivityWindow::Latest => (start > 0, None),
        };

        let mut activities = Vec::with_capacity(page.len());
        for run in &page {
            if let Some(diagnosis) =
                diagnose_run(run, &self.facts, traced.contains(run.as_str()), reader)
            {
                activities.push(diagnosis);
            }
        }

        Ok(EnvironmentDiagnosis {
            agent: reader.agent().clone(),
            // 附属列表按**同一页活动**收窄：本页之外的项由下一页返回，不是被丢掉。
            storage: storage_reading(reader, &page, self.order.len())?,
            activities,
            window,
            has_more,
            next_after,
        })
    }
}

/// 单个活动的诊断详情：完整操作解释 + 有界的事实分页。
///
/// 解释**始终**基于该活动的全部事实行；`from_seq` 只决定返回哪一段行。指定一个两处都没有的
/// 活动明确失败，不冒充空环境。
pub fn diagnose_activity(
    reader: &EnvironmentReader,
    run: &RunId,
    from_seq: Option<u64>,
    limit: usize,
) -> Result<ActivityDetail, DiagnosticError> {
    if limit == 0 || limit > FACT_PAGE_LIMIT {
        return Err(DiagnosticError::InvalidLimit {
            limit,
            max: FACT_PAGE_LIMIT,
        });
    }

    let facts = reader.facts()?;
    let trace = reader.trace()?;
    let in_trace = trace.iter().any(|record| record.run() == run);
    let Some(diagnosis) = diagnose_run(run, &facts, in_trace, reader) else {
        return Err(DiagnosticError::UnknownActivity(run.clone()));
    };

    let all_rows: Vec<FactRow> = facts
        .into_iter()
        .filter(|row| row.run() == run.as_str())
        .filter(|row| from_seq.is_none_or(|from| row.seq() >= from))
        .collect();
    let has_more = all_rows.len() > limit;
    let mut rows = all_rows;
    rows.truncate(limit);
    let next_from = if has_more {
        rows.last().map(|row| row.seq() + 1)
    } else {
        None
    };

    Ok(ActivityDetail {
        diagnosis,
        rows,
        has_more,
        next_from,
    })
}

/// 一个活动在事实侧的**全部**行（按稳定序号），以及它在经历侧是否出现。
///
/// 两处都没有时返回 `None`：调用方据此区分"未知活动"与"真实空环境"。
fn diagnose_run(
    run: &RunId,
    facts: &[FactRow],
    in_trace: bool,
    reader: &EnvironmentReader,
) -> Option<ActivityDiagnosis> {
    let rows: Vec<FactRow> = facts
        .iter()
        .filter(|row| row.run() == run.as_str())
        .cloned()
        .collect();
    let sides = match (rows.is_empty(), in_trace) {
        (false, true) => ActivitySides::BothStores,
        (false, false) => ActivitySides::FactsOnly,
        (true, true) => ActivitySides::TraceOnly,
        (true, false) => return None,
    };

    // 身份权威来自环境：行里保存的那个只用于一致性核对（见 `records_of_run`）。
    let reading = records_of_run(&rows, reader.agent()).map(|records| OperationReading {
        fact_rows: rows.len(),
        counts: counts_of(&records),
        recovered: recover(&records),
    });

    Some(ActivityDiagnosis {
        run: run.clone(),
        sides,
        reading,
    })
}

/// 活动顺序：事实侧首次出现顺序在前，只在经历侧出现的活动按经历侧首次出现顺序接在后面。
fn activity_order(facts: &[FactRow], trace: &[TraceRecord]) -> Vec<RunId> {
    let mut order: Vec<RunId> = Vec::new();
    for row in facts {
        let run = RunId::new(row.run());
        if !order.contains(&run) {
            order.push(run);
        }
    }
    for record in trace {
        let run = record.run().clone();
        if !order.contains(&run) {
            order.push(run);
        }
    }
    order
}

/// 从已验证事实机械数出行动分类。
fn counts_of(records: &[Record]) -> ActionCounts {
    let mut counts = ActionCounts::default();
    let mut prepared: BTreeSet<OperationId> = BTreeSet::new();
    let mut dispatched: BTreeSet<OperationId> = BTreeSet::new();
    let mut resulted: BTreeSet<OperationId> = BTreeSet::new();

    for record in records {
        match record {
            Record::ActionPrepared { operation, .. } => {
                prepared.insert(operation.clone());
            }
            Record::ActionDispatched { operation, .. } => {
                dispatched.insert(operation.clone());
            }
            Record::ActionResultRecorded { action, .. } => {
                resulted.insert(action.operation().clone());
                match action.state() {
                    ResultState::Success => counts.success += 1,
                    ResultState::Failure => counts.failure += 1,
                    ResultState::Unknown => counts.unknown += 1,
                }
            }
            Record::RunStarted { .. } | Record::RunEnded { .. } => {}
        }
    }

    counts.prepared = prepared.len();
    counts.dispatched = dispatched.len();
    counts.prepared_without_dispatch = prepared.difference(&dispatched).count();
    counts.dispatched_without_result = dispatched.difference(&resulted).count();
    counts
}

/// 存储状态来源：只读身份环境已经提供的状态读数。
///
/// 两份附属列表按 `page` 收窄（只留**本页活动**上的项），整份条数另外报出来。收窄是**机械**
/// 过滤：不重排、不去重、不改分类，因此逐页取回的并集与整份列表逐项对应。
fn storage_reading(
    reader: &EnvironmentReader,
    page: &[RunId],
    activities_total: usize,
) -> Result<StorageReading, StoreError> {
    let health = reader.health()?;
    let in_page = |run: &RunId| page.contains(run);
    let cross_store = health
        .cross_store()
        .iter()
        .filter(|issue| in_page(issue.run()))
        .cloned()
        .collect();
    let trace_activities = health
        .runs()
        .iter()
        .filter(|summary| in_page(summary.run()))
        .cloned()
        .collect();
    Ok(StorageReading {
        admission: reader.admission()?,
        trace_records: health.trace_records(),
        fact_records: health.fact_records(),
        paused: health.is_paused(),
        cross_store,
        cross_store_total: health.cross_store().len(),
        trace_activities,
        trace_activities_total: health.runs().len(),
        activities_total,
    })
}

/// 一个活动是否**无法解释**：事实重建被拒绝，或者重建成功、但既有解释器拒绝了这个序列。
///
/// 只有经历侧、没有任何事实行可以解释的活动**不算**——对它如实说“没有操作事实”就是诊断
/// 结论本身，把它算成解释失败会掩盖真正的解释错误。
///
/// 单活动查询与整页查询必须用**同一个**判据：否则 `--run` 会对着一个解释不了的活动返回 0，
/// 而列活动时对同一份环境返回 4，同一个事实因查询方式不同得到两种结论。
pub fn activity_is_unexplained(activity: &ActivityDiagnosis) -> bool {
    match &activity.reading {
        Err(_) => true,
        Ok(reading) => reading.fact_rows > 0 && reading.recovered.is_err(),
    }
}

/// 这一页里是否有活动无法解释。
pub fn has_unexplained_activity(report: &EnvironmentDiagnosis) -> bool {
    report.activities.iter().any(activity_is_unexplained)
}

// ---------------------------------------------------------------------------
// 渲染：稳定的行式结构，不是一段调试文本
// ---------------------------------------------------------------------------

/// 把一个环境级报告渲染成稳定的行式文本（一行一条，字段以空格分隔，`键=值`）。
///
/// 开放文本（活动标识、暂停原因）按 [`escape_field`] 转义，因此值里不会出现空格、`=` 或
/// 控制字符；其余字段都来自封闭词汇表。**输出只写调用方的 stdout**：诊断不追加记录、
/// 不创建任何旁车文件。
pub fn render_environment(report: &EnvironmentDiagnosis) -> String {
    let mut out = String::new();
    out.push_str(REPORT_HEADER);
    out.push('\n');
    let _ = writeln!(out, "mode=activities");
    let _ = writeln!(out, "agent={}", escape_field(report.agent.as_str()));

    match &report.storage.admission {
        Admission::Ready => {
            let _ = writeln!(out, "admission=ready");
        }
        Admission::Paused { reason } => {
            let _ = writeln!(out, "admission=paused");
            let _ = writeln!(out, "admission-reason={}", escape_field(reason));
        }
        Admission::Unfinished { run } => {
            let _ = writeln!(out, "admission=unfinished");
            let _ = writeln!(out, "admission-run={}", escape_field(run.as_str()));
        }
    }

    let storage = &report.storage;
    let _ = writeln!(
        out,
        "storage trace-records={} fact-records={} paused={} cross-store={} trace-activities={}",
        storage.trace_records,
        storage.fact_records,
        storage.paused,
        storage.cross_store.len(),
        storage.trace_activities.len()
    );
    // 附属列表的两个数字**都**要说清：本页返回了几条、整份存储一共有几条。少了后者，
    // 调用方无法分辨"本页没有"与"整份没有"。
    let _ = writeln!(
        out,
        "storage-page scope=activity-page cross-store-total={} trace-activities-total={}",
        storage.cross_store_total, storage.trace_activities_total
    );
    for issue in &storage.cross_store {
        let _ = writeln!(
            out,
            "cross-store run={} kind={}",
            escape_field(issue.run().as_str()),
            issue.kind()
        );
    }
    for summary in &storage.trace_activities {
        let _ = writeln!(
            out,
            "trace-activity run={} records={} status={} ending={} sealed={}",
            escape_field(summary.run().as_str()),
            summary.records(),
            summary.status(),
            escape_field(summary.ending().unwrap_or("none")),
            summary.is_sealed()
        );
    }
    for activity in &report.activities {
        let _ = writeln!(out, "{}", render_activity_line(activity));
    }
    let _ = writeln!(
        out,
        "page window={} has-more={} next-after={}",
        match report.window {
            ActivityWindow::FromStart => "from-start",
            ActivityWindow::Latest => "latest",
        },
        report.has_more,
        optional_run(&report.next_after)
    );
    out
}

/// 把一个活动详情渲染成稳定的行式文本（同一套字段与转义规则）。
pub fn render_activity(detail: &ActivityDetail) -> String {
    let mut out = String::new();
    out.push_str(REPORT_HEADER);
    out.push('\n');
    let _ = writeln!(out, "mode=activity");
    let _ = writeln!(out, "{}", render_activity_line(&detail.diagnosis));
    let _ = writeln!(
        out,
        "fact-page rows={} has-more={} next-from={}",
        detail.rows.len(),
        detail.has_more,
        optional_number(detail.next_from)
    );
    for row in &detail.rows {
        let _ = writeln!(
            out,
            "fact seq={} kind={} turn={} state={} outcome={} agent={}",
            row.seq(),
            escape_field(row.fact()),
            optional_number(row.turn().map(|turn| turn as u64)),
            escape_field(row.state().unwrap_or("none")),
            escape_field(row.outcome().unwrap_or("none")),
            escape_field(row.agent().unwrap_or("none"))
        );
    }
    out
}

/// 一条活动的单行描述：`conversion=` 之后正好是三种形态之一。
fn render_activity_line(activity: &ActivityDiagnosis) -> String {
    let mut line = format!(
        "activity run={} sides={} fact-rows={} conversion=",
        escape_field(activity.run.as_str()),
        activity.sides.as_str(),
        activity
            .reading
            .as_ref()
            .map(|reading| reading.fact_rows)
            .unwrap_or(0)
    );
    match &activity.reading {
        Err(error) => {
            let _ = write!(line, "error kind={} seq={}", error.as_str(), error.seq());
        }
        Ok(reading) => match &reading.recovered {
            Err(reason) => {
                let _ = write!(
                    line,
                    "ok recover=rejected reason={}{}",
                    recovery_label(reason),
                    recovery_detail(reason)
                );
            }
            Ok(recovered) => {
                let counts = &reading.counts;
                // 分类是事实：模型失败且当时带着分类时，就在 outcome 后面补一格读数
                // （没有分类就不写这一格——旧记录照旧）。
                let recorded_failure = match recovered.outcome {
                    Some(Outcome::System(SystemEnding::ModelFailed(Some(class)))) => {
                        format!(" failure={}", escape_field(class))
                    }
                    _ => String::new(),
                };
                let _ = write!(
                    line,
                    "ok recover=ok outcome={}{} last-action-turn={} unconfirmed={} prepared={} dispatched={} success={} failure={} unknown={} prepared-without-dispatch={} dispatched-without-result={}",
                    recovered
                        .outcome
                        .map(outcome_label)
                        .unwrap_or_else(|| "none".to_string()),
                    recorded_failure,
                    optional_number(recovered.last_action_turn.map(|turn| turn.number() as u64)),
                    recovered.unconfirmed.len(),
                    counts.prepared,
                    counts.dispatched,
                    counts.success,
                    counts.failure,
                    counts.unknown,
                    counts.prepared_without_dispatch,
                    counts.dispatched_without_result
                );
            }
        },
    }
    line
}

/// 既有解释器的拒绝原因 → 稳定标记（不新造含义，也不把拒绝原因改写成别的类别）。
fn recovery_label(reason: &RecoveryError) -> &'static str {
    match reason {
        RecoveryError::NoRecords => "no-records",
        RecoveryError::MixedAgents => "mixed-agents",
        RecoveryError::MultipleRuns => "multiple-runs",
        RecoveryError::ForeignRun(_) => "foreign-run",
        RecoveryError::DispatchWithoutPreparation(_) => "dispatch-without-preparation",
        RecoveryError::ResultWithoutPreparation(_) => "result-without-preparation",
        RecoveryError::MultipleEndings => "multiple-endings",
    }
}

/// 拒绝原因里可以如实带出来的定位信息（**不带**经历正文、参数正文或任何凭据）。
fn recovery_detail(reason: &RecoveryError) -> String {
    match reason {
        RecoveryError::ForeignRun(run) => format!(" detail-run={}", escape_field(run.as_str())),
        RecoveryError::DispatchWithoutPreparation(operation)
        | RecoveryError::ResultWithoutPreparation(operation) => {
            format!(" detail-turn={}", operation.turn().number())
        }
        _ => String::new(),
    }
}

fn optional_run(run: &Option<RunId>) -> String {
    match run {
        Some(run) => escape_field(run.as_str()),
        None => "none".to_string(),
    }
}

fn optional_number(number: Option<u64>) -> String {
    match number {
        Some(number) => number.to_string(),
        None => "none".to_string(),
    }
}

/// 把开放文本（活动标识、暂停原因）转义成**一个**字段。
///
/// 输出是一行一条、空格分隔的 `键=值`，所以值里不能出现空格、`=` 或控制字符。转义是机械的：
/// `\` → `\\`、换行 → `\n`、回车 → `\r`、制表 → `\t`、空格 → `\s`、`=` → `\e`；其余字符原样。
fn escape_field(text: &str) -> String {
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
