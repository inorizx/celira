//! 只读判定：**只从已确认记录**推导一次活动现在的状态，并给出可区分的结论。
//!
//! 这一层不写任何东西、不调用模型、不新建 Run、也不动 Foundation：它读已确认的经历与事实，
//! 回答一个问题——"这一次活动现在是什么状态"。答案有五种，互斥，**不压成一个布尔**：
//!
//! ```text
//! Unfinished        没有收尾事实、没有未决外部效果、事实可解释 → 它停在半路
//! Ended             已经有已确认的收场事实（或已封存）
//! Blocked           有**可判定**的阻断原因（已准备未派发、环境暂停、两存储进度矛盾、事实不齐）
//! Unknown           外部效果或最终结果**无法确认**（已派发却没有结果）
//! Refused           事实不可读或不可解释（活动不存在、形状还原不出来）
//! ```
//!
//! ⚠️ **旧规矩是什么、新规矩是什么**（2026-09-26）：这一层从前是**跨进程恢复的受理判据**——
//! 它交回"继续这次活动所需的原料"（轮次 / 开场输入 / 轨迹 / 已确认事实），由两条受理入口
//! （严格 / 带未决效果）决定"能不能接着跑同一个 RunId"。那条路**已退役**：
//! **承载活动的进程没了，就是两个 Run**——那一次连续就断了；新进程
//! 起来是**新的一段**，不是"顶着同一个 RunId 继续"。
//! ⇒ 于是原料、两条受理入口与那五类里"可以继续"这一层含义一起退役；**判定本身留下**，
//! 因为它还有一个真实消费者：`agent_host_status` 按活动逐个读它，把"这一个活动现在什么样"
//! 摆给操作者看（`Unfinished` 那一档现在读作"**它停在半路**"，而**不再**读作"你可以接着跑它"）。
//!
//! 状态**不作为第二套事实源**：没有 `run-state` 记录、没有投影缓存，全部由已确认前缀推导。
//! 判定顺序是有意的，而且是可复核的（见 [`inspect`] 的分步注释）：
//!
//! 1. 环境级暂停 → `Blocked`（已记录在案的保存故障，重开不能绕过）；
//! 2. 活动不在这个环境里 / 没有登记 → `Refused`；
//! 3. 已有收场事实或已封存 → `Ended`；
//! 4. 两存储进度矛盾 → `Blocked`；
//! 5. 事实自相矛盾或读不出来 → `Refused`；
//! 6. 未决外部效果（先看"已派发无结果"→ `Unknown`，再看"已准备未派发"→ `Blocked`）；
//! 7. 经历还原不出来、或轨迹里出现"请求了行动却没有回执 / 有回执却没有请求"→ `Blocked`/`Refused`；
//! 8. 输入没有被确认 → `Blocked`（登记只写了一半）；
//! 9. 以上都干净 → `Unfinished`：**它停在半路，而记录解释得了它**。

use crate::core::{RunInput, TraceEntry};
use crate::foundation::{OperationId, Outcome, RunId, SubjectEnding, recover};
use crate::store::{self, Admission, EnvironmentReader, StoreError};

/// 从已确认记录推导出的活动状态。五类互斥，**不压成布尔**。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunState {
    /// 没有收尾事实、没有未决外部效果、事实可解释：**它停在半路**。
    ///
    /// ⚠️ 这一档从前叫 `ResumableWaiting` 并带着"继续它所需的原料"。原料与那条路一起退役
    /// （见模块文档），于是它成了一个**读数**而不是一句许可：它说的是"记录里没有收尾事实"，
    /// **不是**"你可以接着跑它"。
    Unfinished,
    /// 已经有已确认的收场事实（或已封存）。
    Ended {
        /// 那次收场是什么。
        outcome: Outcome,
        /// 是否已经封存。
        sealed: bool,
        /// **派发过、而没有结果**的那几件（"未确认的效果"）。
        ///
        /// ⚠ **「这一段结束了」与「它有未确认的效果」是两件独立的事**（2026-10-10 裁决）：
        /// 一次活动可以**带着未确认的效果收场**——`SystemEnding::Indeterminate` 说的就是这个。
        /// 从前这两件事被压成一个值（`Ended` 只带 `outcome`）⇒ 结束了的段里那几件
        /// **下一段一个字都看不到**，而且开场那张纸条会读成"**确实没有**"（不是"读不到"）。
        ///
        /// 判据只有一处（[`dispatched_unconfirmed`]）：本档与 [`RunState::Unknown`] **共用**它。
        unconfirmed: Vec<OperationId>,
    },
    /// 有可判定的阻断原因：原因说得清。
    Blocked {
        /// 稳定分类标记（不含正文）。
        reason: String,
    },
    /// 外部效果或最终结果无法确认：不重放、不自动查询，如实报出来。
    Unknown {
        /// 稳定分类标记。
        reason: String,
        /// 无法确认的那些操作（已派发但没有结果）。
        operations: Vec<OperationId>,
    },
    /// 事实不可读或不可解释：不改写任何东西，也不猜一个状态出来。
    Refused {
        /// 稳定分类标记。
        reason: String,
    },
}

impl RunState {
    /// 稳定状态标记（进读数，不重组、不美化）。
    pub fn as_str(&self) -> &'static str {
        match self {
            RunState::Unfinished => "unfinished",
            RunState::Ended { .. } => "ended",
            RunState::Blocked { .. } => "blocked",
            RunState::Unknown { .. } => "unknown",
            RunState::Refused { .. } => "refused",
        }
    }

    /// 阻断/拒绝/无法确认的稳定原因标记；其余两档是 `None`。
    pub fn reason(&self) -> Option<&str> {
        match self {
            RunState::Blocked { reason }
            | RunState::Unknown { reason, .. }
            | RunState::Refused { reason } => Some(reason),
            RunState::Unfinished | RunState::Ended { .. } => None,
        }
    }
}

/// 一次只读判定。
///
/// 它**不改写**任何东西：读经历、读事实、读准入读数，然后给出结论。所有"读不出来"的情形都被
/// 归类成 [`RunState::Refused`]，不抛错——因为"读不出来"本身就是一种必须被如实报告的结论。
///
/// ⚠️ 它从前返回一个 `RecoveryReport`（判定 ＋ 机械读数 ＋ 备注）。那个类型随恢复那条路一起
/// 退役了：**它剩下的每一个字段都只剩这一个消费者**（判定本身），于是它就是判定。
pub fn inspect(reader: &EnvironmentReader, run: &RunId) -> RunState {
    // 1. 环境级暂停：这是**已记录在案**的保存故障，重开不能绕过它。
    let admission = match reader.admission() {
        Ok(admission) => admission,
        Err(error) => {
            return RunState::Refused {
                reason: format!("admission:{}", error.as_str()),
            };
        }
    };
    if let Admission::Paused { reason } = admission {
        return RunState::Blocked {
            reason: format!("storage-paused:{}", escape_field(&reason)),
        };
    }

    // 2. 读已确认前缀（经历与事实各自读，两边都不假设对方完整）。
    let trace_records = match reader.trace() {
        Ok(records) => records,
        Err(error) => return unreadable("trace", error),
    };
    let fact_rows = match reader.facts() {
        Ok(rows) => rows,
        Err(error) => return unreadable("facts", error),
    };

    let mine: Vec<store::TraceRecord> = trace_records
        .into_iter()
        .filter(|record| record.run() == run)
        .collect();
    if mine.is_empty() {
        return RunState::Refused {
            reason: "unknown-run".to_string(),
        };
    }
    if !mine.iter().any(|record| record.entry().is_started()) {
        return RunState::Refused {
            reason: "run-not-registered".to_string(),
        };
    }

    // 3. 收场：已确认的收场记录或封存都意味着这次活动已经有结局。
    let sealed = mine.iter().any(|record| record.entry().is_sealed());
    let mut ended: Option<Outcome> = None;
    for record in &mine {
        // 分类（`failure`）必须一起解：否则从经历读出来的结局会比从事实读出来的少一档
        // （`ModelFailed(None)` vs `ModelFailed(Some(class))`）——同一份记录两处说法不同。
        if let store::TracePayload::Ending {
            subject,
            system,
            failure,
        } = record.entry()
        {
            match store::outcome_of(subject.as_deref(), system.as_deref(), failure.as_deref()) {
                Some(outcome) => ended = Some(outcome),
                None => {
                    return RunState::Refused {
                        reason: "ending-malformed".to_string(),
                    };
                }
            }
        }
    }

    // 4. 两存储之间**可判定**的进度矛盾：能读到就必须能如实说清，不当成"没有发现问题"。
    match reader.cross_store_issues() {
        Ok(issues) => {
            if let Some(issue) = issues.iter().find(|issue| issue.run() == run) {
                return RunState::Blocked {
                    reason: format!("cross-store:{}", issue.kind()),
                };
            }
        }
        Err(error) => return unreadable("cross-store", error),
    }

    // 5. 事实侧重建：读不出来或自相矛盾都不猜。
    let rows: Vec<store::FactRow> = fact_rows
        .into_iter()
        .filter(|row| row.run() == run.as_str())
        .collect();
    // 模型失败的分类**只**在事实侧那一条收场行上（经历侧那一格没有它）。认不出的取值在打开
    // 环境时就已经被整行拒绝（`illegal-failure`），因此这里取不到就是"没有被记录"。
    let recorded_model_failure = rows
        .iter()
        .filter(|row| row.fact() == "run-ended")
        .find_map(|row| row.failure())
        .and_then(crate::core::ModelFailureClass::from_str)
        .map(|class| class.as_str());
    let records = match store::records_of_run(&rows, reader.agent()) {
        Ok(records) => records,
        Err(error) => {
            return RunState::Refused {
                reason: format!("fact-row:{}", error.as_str()),
            };
        }
    };
    let recovered = match recover(&records) {
        Ok(recovered) => recovered,
        Err(error) => {
            return RunState::Refused {
                reason: format!("recovery:{}", recovery_label(&error)),
            };
        }
    };

    // 事实侧说"结束了"而经历侧没有收场记录：本写入者写不出这个形状（先写经历再写事实），
    // 因此不挑一边信，如实拒绝。
    match (&ended, recovered.outcome) {
        (None, Some(_)) => {
            return RunState::Refused {
                reason: "ending-divergence".to_string(),
            };
        }
        (Some(outcome), _) => {
            // 分类**记在事实侧**（`run-ended` 行的可选 `failure` 字段）：经历侧那一格只存收场
            // 字符串，而收场字符串一个字都没变。这里把两边**同一个事实**的两半合起来——只读、
            // 不改写，缺分类时保持"未记录"（**不编造**）。
            let outcome = match outcome {
                Outcome::System(crate::foundation::SystemEnding::ModelFailed(_)) => {
                    match recorded_model_failure {
                        Some(class) => Outcome::System(
                            crate::foundation::SystemEnding::ModelFailed(Some(class)),
                        ),
                        None => *outcome,
                    }
                }
                _ => *outcome,
            };
            // ⚠ **两件事一起带走**（2026-10-10 裁决）：这一段结束了，**而**它可能还带着
            // "派发过、没有结果"的那几件。判据**不在这里复制**——它与下面 `Unknown` 那一档
            // 共用同一个函数，否则这条缝会从另一个方向再长出来。
            let unconfirmed = dispatched_unconfirmed(&recovered, &records);
            return RunState::Ended {
                outcome,
                sealed,
                unconfirmed,
            };
        }
        (None, None) => {}
    }

    // 6. 未决外部效果：先看"已派发无结果"（可能已经发生 → Unknown），再看"已准备未派发"
    //    （可证明没有外部效果 → Blocked）。两者必须**可区分**。
    if !recovered.unconfirmed.is_empty() {
        let dispatched = dispatched_unconfirmed(&recovered, &records);
        return if dispatched.is_empty() {
            RunState::Blocked {
                reason: "prepared-without-dispatch".to_string(),
            }
        } else {
            // ⚠️ **故意不在这里跑第 7 步的配对检查**：崩溃窗口的轨迹本来就是"请求了行动却没有
            // 回执"，跑那条检查会把每一档 `Unknown` 都变成 `Blocked`——那等于把"无法确认"这个
            // 结论抹掉。判定顺序不变，是刻意的。
            RunState::Unknown {
                reason: "dispatched-without-result".to_string(),
                operations: dispatched,
            }
        };
    }

    // 7. 经历侧还原：还原不出来就拒绝；还原之后，一次"请求了行动的响应"必须紧邻它的回执。
    let mut trace = Vec::new();
    for record in &mine {
        match store::rehydrate(record) {
            Ok(Some(entry)) => trace.push(entry),
            Ok(None) => {}
            Err(error) => {
                return RunState::Refused {
                    reason: format!("rehydrate:{}", error.as_str()),
                };
            }
        }
    }
    if let Some(reason) = pairing_defect(&trace) {
        return RunState::Blocked { reason };
    }

    // 8. 开场输入必须**已经被确认**，而且必须是一份合法输入：缺了它说明"登记只写了一半"，
    //    写不成一份合法输入说明那条记录解释不了这次活动。
    let input = mine.iter().find_map(|record| match record.entry() {
        store::TracePayload::RunInput { text } => Some(text.clone()),
        _ => None,
    });
    let Some(input) = input else {
        return RunState::Blocked {
            reason: "input-not-confirmed".to_string(),
        };
    };
    if RunInput::text(input).is_err() {
        return RunState::Refused {
            reason: "input-too-large".to_string(),
        };
    }

    // 9. 干净：**它停在半路，而记录解释得了它**。
    RunState::Unfinished
}

/// 这个操作在事实里有没有"已经派发"的记录。
/// **这一段最后是不是主体自己"停在这儿"**（2026-10-13）。四态互斥，**不压成布尔**。
///
/// 用途**两处**（原话是"只有一处"，2026-10-13 起了第二个消费者）：① 一段**没有收场**就断了的时候，
/// 下一段读到的纸条要能分清「**你自己停下的**（此后这一段活动断了）」与「**没有正常收场**」——
/// 两者都真，但**只说后者就是拿假话吓人**（2026-10-10 裁决三：把"主体自己搁下的"读成"断了"）；
/// ② 操作者那一行（`agent_host_status` 的 `run=… stopped=…`）同一条判据：**人不许比系统知道得更少**。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoppedInPlace {
    /// 主体**点了名**停在这儿（`hold_run`，账上多一条工具调用记录）。
    Named,
    /// 主体**只是把控制交回**（那一条答复是纯正文，没有任何工具调用）。
    ///
    /// 与 [`StoppedInPlace::Named`] **机械上同一件事**（都停在这儿、活动槽都还属于这一段），
    /// 区别只在账上——见 `core.rs` 那一处的注。
    Silent,
    /// 最后一条答复**不是**"停在这儿"：它在干活，或者已经收场了。
    No,
    /// 账**读不出来** ⇒ **不声称**是哪一种。
    Unreadable(String),
}

impl StoppedInPlace {
    /// 操作者那一行里的稳定词（**不是**给主体的散文——主体那一侧读的是开场那张纸条）。
    ///
    /// 取值就取事实里本来就有的词，一个都不新造：`held`（`SubjectEnding::Held`）、
    /// `yielded`（那一条答复的决定是 `yield`）、`no-response`（一条答复都没有）、
    /// `unreadable`（账读不出来）。⚠ 它**不落盘**（只出现在读数行里），因此不进词表守卫那张封闭表；
    /// 真要落盘的那天，必须**同一刀**补解码端。
    pub fn as_str(&self) -> &'static str {
        match self {
            StoppedInPlace::Named => "held",
            StoppedInPlace::Silent => "yielded",
            StoppedInPlace::No => "no-response",
            StoppedInPlace::Unreadable(_) => "unreadable",
        }
    }
}

/// **读账，不猜**：这一段最后一条 `model-response` 的决定是不是"停在这儿"（见 [`StoppedInPlace`]）。
///
/// ⚠ **它与 [`inspect`] 读的是同一份经历，但只在需要时才调**：正常路径（有收场事实、或从没跑过）
/// **一次都不读**——读一份账是线性开销，不该让每一次开场都付它。调用方把这一条写在注释里。
///
/// ⚠ **没有答复 ⇒ [`StoppedInPlace::No`]**（不是"停在这儿"）：那一段在第一次模型调用之前就断了。
pub fn stopped_in_place(reader: &EnvironmentReader, run: &RunId) -> StoppedInPlace {
    let records = match reader.trace() {
        Ok(records) => records,
        Err(error) => return StoppedInPlace::Unreadable(error.as_str().to_string()),
    };
    let last = records
        .iter()
        .filter(|record| record.run() == run)
        .filter_map(|record| match record.entry() {
            // 一条答复一个形状；**取最后一条**（它才是"断掉的那一刻主体在做什么"）。
            //
            // ⚠ 这两个字面量（`yield` / `finish`）是**已确认记录的封闭词表**里的取值，权威在
            // `store::environment::KNOWN_DECISIONS`（解码端逐条校验过，读到这里的一定在表里）；
            // 收场那一格用 [`SubjectEnding::as_str`] 那份权威，不另写一个字符串。
            store::TracePayload::ModelResponse {
                decision, ending, ..
            } => Some(match decision.as_str() {
                "yield" => StoppedInPlace::Silent,
                // 收场那一格**走存储自己的解码权威**（`store::outcome_of`）——
                // 不在这一层再写一份收场词表（同一件事只有一个权威位置）。
                "finish" => match store::outcome_of(ending.as_deref(), None, None) {
                    Some(Outcome::Subject(SubjectEnding::Held)) => StoppedInPlace::Named,
                    _ => StoppedInPlace::No,
                },
                _ => StoppedInPlace::No,
            }),
            _ => None,
        })
        .next_back();
    // 一条答复都没有 ⇒ 那一段在第一次模型调用之前就断了 ⇒ **不是"停在这儿"**。
    last.unwrap_or(StoppedInPlace::No)
}

fn dispatched_in(records: &[crate::foundation::Record], operation: &OperationId) -> bool {
    records.iter().any(|record| {
        matches!(
            record,
            crate::foundation::Record::ActionDispatched { operation: other, .. } if other == operation
        )
    })
}

/// **派发过、而没有结果**的那几件——"未确认的效果"。
///
/// ⚠ 它与「这一段结束了没有」是**两件独立的事**（2026-10-10 裁决）：一次活动可以**带着未确认的
/// 效果收场**。因此本函数有**两个读者、一份判据**：[`RunState::Unknown`]（没结束的）与
/// [`RunState::Ended`]（结束了的）。**不许各写一份**——这条缝当初就是从"只有一个读者"长出来的。
///
/// 它只筛"派发过"：`recovered.unconfirmed` 里那些**已准备未派发**的，是可证明没有外部效果的
/// 那一档（那是 `Blocked` 的事，不是这一格）。
fn dispatched_unconfirmed(
    recovered: &crate::foundation::Recovered,
    records: &[crate::foundation::Record],
) -> Vec<OperationId> {
    recovered
        .unconfirmed
        .iter()
        .filter(|operation| dispatched_in(records, operation))
        .cloned()
        .collect()
}

/// 轨迹里的配对缺陷：一次请求了行动的响应必须紧邻**它的每一条**回执；回执也必须有对应的请求。
///
/// 一轮可以请求 **N** 个动作，因此这里等的是"**N 条**回执"而不是"一条"——条数不足与条数过多
/// 都是缺陷。它仍然是**兜底**：事实侧已经先判过"未决效果"。这里能抓到的是"响应写了、回执还没写"
/// 这种只存在于经历侧的半截形状——那是"这次活动的记录自己解释不了自己"的一格。
fn pairing_defect(trace: &[TraceEntry]) -> Option<String> {
    let mut pending = 0usize;
    for entry in trace {
        match entry {
            TraceEntry::ModelResponse(response) => {
                if pending > 0 {
                    return Some("unpaired-trace".to_string());
                }
                pending = response.decision().actions().len();
            }
            TraceEntry::Refused { .. } | TraceEntry::Observation(_) => {
                if pending == 0 {
                    return Some("unpaired-trace".to_string());
                }
                pending -= 1;
            }
            TraceEntry::Injection { .. } => {
                if pending > 0 {
                    return Some("unpaired-trace".to_string());
                }
            }
        }
    }
    if pending > 0 {
        return Some("unpaired-trace".to_string());
    }
    None
}

/// 读不出来的统一收尾：报成 `Refused`，不抛错、不猜。
fn unreadable(source: &str, error: StoreError) -> RunState {
    RunState::Refused {
        reason: format!("{source}:{}", error.as_str()),
    }
}

/// `RecoveryError` 的稳定标记（Foundation 的既有分类，不重组）。
fn recovery_label(error: &crate::foundation::RecoveryError) -> &'static str {
    use crate::foundation::RecoveryError;
    match error {
        RecoveryError::NoRecords => "no-records",
        RecoveryError::MixedAgents => "mixed-agents",
        RecoveryError::MultipleRuns => "multiple-runs",
        RecoveryError::ForeignRun(_) => "foreign-run",
        RecoveryError::DispatchWithoutPreparation(_) => "dispatch-without-preparation",
        RecoveryError::ResultWithoutPreparation(_) => "result-without-preparation",
        RecoveryError::MultipleEndings => "multiple-endings",
    }
}

/// 开放文本进单行字段前必须不能伪造行结构（与入口/适配层同一套转义）。
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
