//! 记录 ↔ 轨迹（2026-10-08 从 `store.rs` 拆出；**盘上格式一个字节没动**）。
//!
//! 两个方向都在这一个文件里，因为它们在盘上那一行的**两侧**：
//!
//! - **写**：`*_payload` 把 Core 的轨迹条目搬成盘上那一行；
//! - **读**：`rehydrate` / `outcome_of` 把盘上那一行搬回轨迹条目（认不出 ⇒ 明确拒绝，不猜）；
//! - **说人话**：四个标签（`subject_label` / `system_label` / `outcome_label` / `ending_label`）
//!   ——词表与形状的权威就在盘上那张封闭表旁边；
//! - **按活动读回**：`list_runs` / `list_runs_at` / `read_run_at`。
//!
//! **范围**：从 [`payload_of`] 到 [`read_run_at`]，加上读回取数用的那几个私有件
//! （`parts_holding` / `part_run_records` / `page_of`）与末尾的用例模块。分卷与索引在 `parts.rs`、
//! 身份与准入在 `environment.rs`、格式与校验和在 `format.rs` ⇒ **本文件只搬运，不定义盘上格式**。
//!
//! ## 判据
//!
//! - **拆分只换「谁在哪个文件里实现这套搬运」，不换盘上格式、不换行为**：判据是**盘上格式一个
//!   字节不动 ＋ 行为零变化**（读数逐字相同；那几条读数属那一刀的经过，归 Git）。
//! - **一份记录的形状只有一处权威**：类型的构造入口。读侧**按判别式解码**、不重复那张表
//!   （理由与真机缺陷的形状见 [`outcome_of_observation`]）；新加一个构造入口就把往返守卫加一行。
//! - **「本来不是轨迹条目」与「读不出来」是两件事**：`Ok(None)` 只说前者、`Err` 才是后者
//!   （见 [`rehydrate`]）——**两件事不许压成一格**。
//!
//! ## ⚠ 编辑本文件时要知道的接线
//!
//! 父模块 `store.rs` 要用到本文件的名字时一律**显式 `use`**（清单在 `store.rs` 末尾），**不靠 glob**：
//! 子模块都 `use super::*`、父模块又 `pub use 子::*` ⇒ **glob 循环**。
//! ⚠ **它的形状骗人**：`cargo build` 过、`cargo test` 不过（`cfg(test)` 下父模块解析不到子模块
//! 的名字，`E0425`）⇒ **父模块自己要用的名字一律显式列**；那几个 `pub use 子::*` 只负责**公开面**。
//! 同一个坑的第二个形状：**显式 `use` 里再写一遍某个已经 `pub` 的名字，会 shadow 公开 glob**
//! （`-D warnings` 下是错误；当初踩出来的那一个是 `SealedPart`）⇒ 已由 glob 公开的名字，别再进显式清单。
//!
//! **公开面原样留在 `crate::store::` 下**（父模块 `pub use projection::*`），消费方一处未改。

use super::*;

/// 把 Core 的运行内轨迹条目转成一条可保存的经历。
pub fn payload_of(entry: &TraceEntry, turn: Option<Turn>) -> TracePayload {
    match entry {
        TraceEntry::Injection {
            stage,
            turn: at,
            batch,
        } => TracePayload::Injection {
            stage: stage_str(*stage).to_string(),
            turn: at.map(Turn::number).or_else(|| turn.map(Turn::number)),
            blocks: content_pairs(batch.blocks()),
        },
        TraceEntry::Refused {
            turn: at,
            action,
            class,
            content,
        } => TracePayload::Injection {
            stage: REFUSED_STAGE.to_string(),
            // 真实轮次：这次拒绝发生在哪一轮，就记哪一轮。
            turn: Some(at.number()),
            // 结构化块：动作标识与拒绝分类各自成块，不拼成一条可能产生歧义的文本。
            //
            // **随单内容接在这两块之后**（TASK_72）：它照旧是块（同一个 `content_pairs`），
            // 因此落盘形状与注入内容同族；两块标记照旧在前、逐字节不变——老轨迹（恰好两块）
            // 与新轨迹（两块 + 随单）在读回来那一侧是同一个解码口。
            blocks: {
                let mut blocks = vec![
                    ("action".to_string(), action.as_str().to_string()),
                    ("admission".to_string(), class.clone()),
                ];
                blocks.extend(content_pairs(content));
                blocks
            },
        },
        TraceEntry::ModelResponse(response) => response_payload(response),
        TraceEntry::Observation(observation) => TracePayload::Observed {
            turn: observation.operation().turn().number(),
            index: observation.operation().index(),
            action: observation.action().as_str().to_string(),
            params: params_pairs(observation.params()),
            state: observation.state(),
            failure: observation.outcome().failure_class().map(str::to_string),
            blocks: content_pairs(observation.outcome().content()),
        },
    }
}

/// 一条已确认经历**再水化**成 Core 轨迹条目时被拒绝的原因。
///
/// 它只出现在恢复路径上。打开环境时已经拒掉了"不认识的种类"与"不合法的模型响应形状"
/// （`corrupt-tail` / `malformed-entry`），这里多出的三类是打开阶段**不校验**的部分：
/// 注入阶段、拒绝标记、行动结果的三分与内容组合。恢复必须自己明确拒绝，不猜。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RehydrateError {
    /// 注入阶段不是本写入者可能写出的取值。
    UnknownStage(String),
    /// 一次注入没有轮次。
    MissingTurn,
    /// 盘上的轮次超出 `Turn` 的取值范围。
    TurnOutOfRange(u32),
    /// 拒绝经历的两块（动作标识 / 拒绝分类）不齐或多余。
    RefusedMarkerIncomplete,
    /// 决定取值不是封闭词汇表里的三种之一。
    UnknownDecision(String),
    /// 一次行动响应没有行动标识。
    MissingActionId,
    /// 一次 `act` 响应最终**一个动作都没有**（本写入者写不出这个形状）。
    EmptyAct,
    /// 一次结束响应没有结束取值。
    MissingEnding,
    /// 结束取值不在封闭词汇表里。
    UnknownSubjectEnding(String),
    /// 续接载荷的三个字段不齐，或无法构造成中立载荷。
    ProtocolPayloadBroken,
    /// 行动结果的三分、失败分类与内容组合不是本写入者可能写出的形状。
    OutcomeNotProducible,
    /// 内容批次超过既有上限。
    BatchRejected,
}

impl RehydrateError {
    /// 稳定分类标记（可进恢复报告，不含任何正文）。
    pub fn as_str(&self) -> &'static str {
        match self {
            RehydrateError::UnknownStage(_) => "unknown-stage",
            RehydrateError::MissingTurn => "missing-turn",
            RehydrateError::TurnOutOfRange(_) => "turn-out-of-range",
            RehydrateError::RefusedMarkerIncomplete => "refused-marker-incomplete",
            RehydrateError::UnknownDecision(_) => "unknown-decision",
            RehydrateError::MissingActionId => "missing-action-id",
            RehydrateError::EmptyAct => "empty-act",
            RehydrateError::MissingEnding => "missing-ending",
            RehydrateError::UnknownSubjectEnding(_) => "unknown-subject-ending",
            RehydrateError::ProtocolPayloadBroken => "protocol-payload-broken",
            RehydrateError::OutcomeNotProducible => "outcome-not-producible",
            RehydrateError::BatchRejected => "batch-rejected",
        }
    }
}

/// **一次观察 → 行动结果**：按**判别式**解码，不枚举"写者可能写过什么"。
///
/// ## 为什么（2026-10-13 真机抓到的缺陷）
///
/// 从前这里枚举"本写入者可能写出的形状"：`(success, 无分类, 任意)` / `(failure, 有分类, 零内容)` /
/// `(unknown, 无分类, 零内容)`，其余一律 `OutcomeNotProducible`。那张表**漏了一格**——
/// `failure` **带随单读数**（[`ActionOutcome::failure_with_content`]，2026-10-11 立的那条
/// "一次调用的形状错误不报废这一段 ⇒ 一条**带读数的拒绝经历**"）。
///
/// 而写侧 [`observed_payload`] 从来不查表：它照 `outcome.state()` / `failure_class()` / `content()`
/// 三个读数**逐格**搬到盘上 ⇒ 写者能写出哪些形状，由 [`ActionOutcome`] 的构造入口决定。这份知识
/// 当年在这里**又被抄了一遍**——两份知识不会同时更新，于是：
/// **写者写得出来、读者读不回来** ⇒ 那一段之后的**每一段新活动**都在恢复那一步被拒
/// （真机实测：`state=refused reason=rehydrate:outcome-not-producible`，账里 291 行只有那一行
/// 不合法，而主体从此永远不再回话）。
///
/// ## 边界（这一格的判据）
///
/// **形状的知识只有一个权威位置**：类型的构造入口（[`ActionOutcome`]）。读者**不重复**那份知识
/// ——它按判别式解码，只校验**该分支自己**必须成立的事：
///
/// - `success`：分类必须缺席（成功不带分类）；
/// - `failure`：分类**必须**在；**读数可有可无**（它是"随单读数"，与分类是两个独立分量）；
/// - `unknown`：分类缺席，且今天没有"带读数的不知道"这个构造入口 ⇒ 有读数即拒绝（fail-closed）。
///
/// ⚠ 两侧会不会漂移，由**往返守卫**抓（`tests::every_shape_the_writer_can_write_reads_back_as_itself`
/// 用 [`observed_payload`] 写、用本函数读，逐格比）。**新加一个构造入口就把守卫加一行**——
/// 生产侧不再有一张会漂的表。
///
/// ## ⚠ 这一格改了什么、没改什么
///
/// **只动读侧**：盘上格式一个字节没改，[`observed_payload`] 与 [`ActionOutcome`] 的形状一字未动
/// ⇒ 旧账照读。真机上那一条读不回来的记录，正是一次"参数名猜错"的拒绝**连它的读数**（分类与
/// "缺哪一项"那块读数各自成格）——那本身就是 2026-10-11 起要的效果：让主体看得见参数名不必猜。
pub(super) fn outcome_of_observation(
    state: ResultState,
    failure: Option<&String>,
    blocks: &[(String, String)],
) -> Result<ActionOutcome, RehydrateError> {
    let content = blocks_of(blocks)?;
    match (state, failure) {
        (ResultState::Success, None) => Ok(ActionOutcome::success(content)),
        // **读路径唯一的开口**：盘上那根分类是 owned（别人写的），因此走 `recorded` 而不是
        // 写路径那两个只收 `&'static str` 的构造器（见 `core::ActionOutcome::recorded` 的文档）。
        (ResultState::Failure, Some(class)) => Ok(ActionOutcome::recorded(class.clone(), content)),
        (ResultState::Unknown, None) if content.is_empty() => Ok(ActionOutcome::unknown()),
        _ => Err(RehydrateError::OutcomeNotProducible),
    }
}

/// 把一条已确认经历**再水化**成 Core 的轨迹条目。
///
/// 三态返回，因为它们确实是三件事：
///
/// - `Ok(Some(entry))`：这条记录本来就是轨迹条目；
/// - `Ok(None)`：这条记录**合法地不是**轨迹条目（登记、输入、终态、封存、行动选择，
///   以及环境级暂停标记）——它们不是"读不出来"，而是本来就不在这条轨迹里；
/// - `Err(...)`：形状无法还原成轨迹条目 → 恢复必须拒绝这次继续，不猜测、不补默认值。
///
/// 只做机械还原：不改顺序、不丢字段、不把缺失值填成空。
pub fn rehydrate(record: &TraceRecord) -> Result<Option<TraceEntry>, RehydrateError> {
    match record.entry() {
        TracePayload::RunStarted { .. }
        | TracePayload::RunInput { .. }
        | TracePayload::Ending { .. }
        | TracePayload::Sealed
        // 行动选择是"执行之前"的确认点，不是 Core 的轨迹条目：它的结果是那条观察。
        | TracePayload::ActionSelected { .. } => Ok(None),
        TracePayload::Injection {
            stage,
            turn,
            blocks,
        } => {
            // 环境级暂停标记不是这次活动的轨迹条目。
            if stage == PAUSED_STAGE {
                return Ok(None);
            }
            // **静态头装订也不是**：Core 不认识静态头，看见的那一份由适配层在渲染那一刻
            // 拼进请求的尾巴。这一条只作见证（"这一刻装订的是哪一份"），不进视野。
            if stage == STATIC_HEAD_STAGE {
                return Ok(None);
            }
            if stage == REFUSED_STAGE {
                return refused_entry(turn.as_ref(), blocks).map(Some);
            }
            let stage = match stage.as_str() {
                RUN_START_STAGE => crate::core::InjectionStage::RunStart,
                TURN_STAGE => crate::core::InjectionStage::Turn,
                other => return Err(RehydrateError::UnknownStage(other.to_string())),
            };
            let turn = decoded_turn(*turn)?;
            let batch = ContentBatch::of(blocks_of(blocks)?)
                .map_err(|_| RehydrateError::BatchRejected)?;
            Ok(Some(TraceEntry::Injection { stage, turn, batch }))
        }
        TracePayload::ModelResponse {
            turn,
            content,
            decision,
            action,
            params,
            extra,
            ending,
            protocol,
        } => {
            let turn = decoded_turn(Some(*turn))?.ok_or(RehydrateError::MissingTurn)?;
            let decision = match decision.as_str() {
                "act" => {
                    let id = action
                        .as_ref()
                        .ok_or(RehydrateError::MissingActionId)?;
                    // 一轮可以请求多个动作：`action` 是第 0 个，`extra` 是其余（**按顺序**）。
                    let mut actions = Vec::with_capacity(extra.len() + 1);
                    actions.push(Action::with_params(
                        ActionId::new(id.clone()),
                        params_of(params),
                    ));
                    for (id, params) in extra {
                        actions.push(Action::with_params(
                            ActionId::new(id.clone()),
                            params_of(params),
                        ));
                    }
                    Decision::act(actions).ok_or(RehydrateError::EmptyAct)?
                }
                "finish" => {
                    let name = ending.as_ref().ok_or(RehydrateError::MissingEnding)?;
                    Decision::Finish(
                        subject_ending_of(name)
                            .ok_or_else(|| RehydrateError::UnknownSubjectEnding(name.clone()))?,
                    )
                }
                "yield" => Decision::Yield,
                other => return Err(RehydrateError::UnknownDecision(other.to_string())),
            };
            let protocol = match protocol {
                Some((name, version, payload)) => Some(
                    ProtocolData::new(name.clone(), *version, payload.clone())
                        .map_err(|_| RehydrateError::ProtocolPayloadBroken)?,
                ),
                None => None,
            };
            let output = ModelOutput::new(content.clone(), protocol, decision);
            Ok(Some(TraceEntry::ModelResponse(ModelResponse::new(
                turn, output,
            ))))
        }
        TracePayload::Observed {
            turn,
            index,
            action,
            params,
            state,
            failure,
            blocks,
        } => {
            let turn = decoded_turn(Some(*turn))?.ok_or(RehydrateError::MissingTurn)?;
            // 子序号**必须**带回来：一轮里第几个动作是身份的一部分，丢了它两个动作会塌成同一个。
            let operation = OperationId::at(record.run().clone(), turn, *index);
            let requested = Action::with_params(ActionId::new(action.clone()), params_of(params));
            // 按**判别式**解码（判据与真机缺陷的形状见 [`outcome_of_observation`]）：
            // 这里从前枚举"本写入者可能写出的形状"，而那张表漏了 `failure` **带随单读数**那一格
            // ⇒ 写者写得出来、读者读不回来 ⇒ 那一段之后的每一段新活动都在恢复那一步被拒。
            let outcome = outcome_of_observation(*state, failure.as_ref(), blocks)?;
            Ok(Some(TraceEntry::Observation(Observation::new(
                operation,
                &requested,
                outcome,
            ))))
        }
    }
}

/// 一次未获接纳的行动请求：两块标记（动作标识 / 拒绝分类）齐了才还原；**两块之后的块是随单内容**
/// （TASK_72），照旧收下——它们是块，不是标记，不需要解码器认识。
///
/// 向后兼容是**同一句话**：老轨迹里这两条经历只有两块，于是随单内容为空、行为与从前逐字节相同。
/// 畸形仍然 fail-closed：标记缺一个、或标记重复出现（连出现在内容里也算），一律拒绝。
fn refused_entry(
    turn: Option<&u32>,
    blocks: &[(String, String)],
) -> Result<TraceEntry, RehydrateError> {
    let turn = decoded_turn(turn.copied())?.ok_or(RehydrateError::MissingTurn)?;
    if blocks.len() < 2 {
        return Err(RehydrateError::RefusedMarkerIncomplete);
    }
    let mut action: Option<String> = None;
    let mut class: Option<String> = None;
    let mut content: Vec<ContentBlock> = Vec::new();
    for (label, text) in blocks {
        match label.as_str() {
            "action" if action.is_none() => action = Some(text.clone()),
            "admission" if class.is_none() => class = Some(text.clone()),
            // 重复的标记：不把它当成内容收下（那会把畸形读成一次"有随单"的拒绝）。
            "action" | "admission" => return Err(RehydrateError::RefusedMarkerIncomplete),
            _ => content.push(ContentBlock::new(label.clone(), text.clone())),
        }
    }
    Ok(TraceEntry::Refused {
        turn,
        action: ActionId::new(action.ok_or(RehydrateError::RefusedMarkerIncomplete)?),
        class: class.ok_or(RehydrateError::RefusedMarkerIncomplete)?,
        content,
    })
}

/// 盘上的轮次 → `Turn`；不在盘上就是 `None`（合法）。
fn decoded_turn(number: Option<u32>) -> Result<Option<Turn>, RehydrateError> {
    match number {
        Some(number) => Turn::from_number(number)
            .map(Some)
            .ok_or(RehydrateError::TurnOutOfRange(number)),
        None => Ok(None),
    }
}

/// 内容块对 → 中立内容块。
fn blocks_of(blocks: &[(String, String)]) -> Result<Vec<ContentBlock>, RehydrateError> {
    Ok(blocks
        .iter()
        .map(|(label, text)| ContentBlock::new(label.clone(), text.clone()))
        .collect())
}

/// 参数对 → 中立参数容器（按原有顺序）。
fn params_of(params: &[(String, String)]) -> ActionParams {
    let mut out = ActionParams::none();
    for (name, value) in params {
        out = out.with(name.clone(), value.clone());
    }
    out
}

/// 主体收场的**全部取值**（与 [`SUBJECT_ENDINGS`] 的名字表、`subject_str` 的穷举一一对应）。
///
/// 反向查表用它，而不是用一串字面量：`subject_str` 的穷举 match 保证"新增变体就编译失败"，
/// 名字表保证"取值在封闭词汇表里"，`tests` 里的守卫保证三张表不漂移。
///
/// ⚠ **2026-10-10**：五个值，而**写侧只有一个**（`Ended`——主体收场不带值）。
/// 四个旧值在这一层仍然要有名字：**读旧记录要用**（照读不写）。
const SUBJECT_ENDING_VALUES: [SubjectEnding; 6] = [
    SubjectEnding::Ended,
    SubjectEnding::Held,
    SubjectEnding::Completed,
    SubjectEnding::Silent,
    SubjectEnding::Declined,
    SubjectEnding::Waiting,
];

/// 系统终止的**全部取值**（与 [`SYSTEM_ENDINGS`] 的名字表、`system_str` 的穷举一一对应）。
const SYSTEM_ENDING_VALUES: [SystemEnding; 6] = [
    // 唯一带载荷的一格：这张表只回答"取值名字 ↔ 变体"，载荷在别处单独交代，因此这里是 `None`。
    SystemEnding::ModelFailed(None),
    SystemEnding::ActionFailed,
    SystemEnding::Cancelled,
    SystemEnding::Timeout,
    SystemEnding::Crashed,
    SystemEnding::Indeterminate,
];

/// 结束取值 → 主体收场（封闭词汇表；认不出就是认不出）。
fn subject_ending_of(name: &str) -> Option<SubjectEnding> {
    SUBJECT_ENDING_VALUES
        .into_iter()
        .find(|ending| subject_str(*ending) == name)
}

/// 系统收场取值 → 系统终止（封闭词汇表；认不出就是认不出）。
fn system_ending_of(name: &str) -> Option<SystemEnding> {
    SYSTEM_ENDING_VALUES
        .into_iter()
        .find(|ending| system_str(*ending) == name)
}

/// 某一次活动在**第 `turn` 轮当时生效**的那一份静态头。
///
/// 取的是"最后一次装订、且它的生效轮次 ≤ `turn`"——因为装订是**按轮次追加**的（开场一次、
/// 每一个新进程重新拍读一次），所以"当时生效的是哪一份"是**查出来的**，不是猜出来的。
///
/// 没有装订记录 ⇒ `None`：那一刻**没有**那一份（主体摘了说明书，或家里本来就没有）。
/// 这与 `origin` / `failure` 同一条规矩——**缺字段 = 没有被记录，不编造**。
pub fn static_head_at(records: &[TraceRecord], turn: u32) -> Option<String> {
    records
        .iter()
        .filter_map(|record| match record.entry() {
            TracePayload::Injection {
                stage,
                turn: at,
                blocks,
            } if stage == STATIC_HEAD_STAGE => {
                let at = at.unwrap_or(0);
                let text = blocks
                    .iter()
                    .find(|(label, _)| label == STATIC_HEAD_LABEL)
                    .map(|(_, text)| text.clone())?;
                Some((at, text))
            }
            _ => None,
        })
        .filter(|(at, _)| *at <= turn)
        .max_by_key(|(at, _)| *at)
        .map(|(_, text)| text)
}

/// 一条已确认的收场记录**解码**成中立结局。
///
/// 只认"恰好一种收场、取值在封闭词汇表里"的形状：两种都有或都没有都是本写入者写不出来的，
/// 因此返回 `None`，由调用方（恢复判定）如实拒绝，而不是挑一个看起来像的。
///
/// `failure` 是**模型失败的分类**（[`TracePayload::Ending`] 的 `failure` 字段）：它必须与该字段
/// 一起解，否则"从经历读出来的结局"会比"从事实读出来的结局"少一档——同一份记录两处说法不同，
/// 那正是这套存储最容易出的错。缺分类 ＝ 当时就没带（旧记录），⇒ `ModelFailed(None)`，**不编造**；
/// 分类挂在非模型失败上、或取值不在词表里 ⇒ `None`（fail-closed，与事实侧同一条规矩）。
pub fn outcome_of(
    subject: Option<&str>,
    system: Option<&str>,
    failure: Option<&str>,
) -> Option<Outcome> {
    match (subject, system) {
        // 主体收场不带分类：带着就是自相矛盾的形状。
        (Some(name), None) if failure.is_none() => subject_ending_of(name).map(Outcome::Subject),
        (None, Some(name)) => {
            let ending = system_ending_of(name)?;
            let ending = match (ending, failure) {
                (SystemEnding::ModelFailed(_), None) => SystemEnding::ModelFailed(None),
                (SystemEnding::ModelFailed(_), Some(class)) => SystemEnding::ModelFailed(Some(
                    crate::core::ModelFailureClass::from_str(class)?.as_str(),
                )),
                (other, None) => other,
                (_, Some(_)) => return None,
            };
            Some(Outcome::System(ending))
        }
        _ => None,
    }
}

/// 把一次模型响应变成一条可保存的经历。
///
/// 三种决定各写各的字段，**不**把行动、结束与正文拼成一段文本：正文归 `content`，决定归
/// `decision`，行动与参数、结束取值各自成字段。协议续接数据**保存**（Adapter 靠它重建下一次
/// 请求）但**不参与文本渲染**，见 [`TracePayload::text`]。
pub fn response_payload(response: &ModelResponse) -> TracePayload {
    let (decision, action, params, extra, ending) = match response.decision() {
        Decision::Act(actions) => {
            let mut ordered = actions.iter();
            // `Decision::Act` 的不变量是"非空"（构造入口挡住空列表），因此第一项一定在。
            let first = ordered.next().expect("act 至少有一个动作");
            (
                "act",
                Some(first.id().as_str().to_string()),
                params_pairs(first.params()),
                ordered
                    .map(|action| {
                        (
                            action.id().as_str().to_string(),
                            params_pairs(action.params()),
                        )
                    })
                    .collect::<Vec<_>>(),
                None,
            )
        }
        Decision::Finish(ending) => (
            "finish",
            None,
            Vec::new(),
            Vec::new(),
            Some(subject_str(*ending).to_string()),
        ),
        Decision::Yield => ("yield", None, Vec::new(), Vec::new(), None),
    };
    let protocol = response.protocol().map(|data| {
        (
            data.protocol().to_string(),
            data.version(),
            data.payload().to_string(),
        )
    });
    TracePayload::ModelResponse {
        turn: response.turn().number(),
        content: response.content().to_string(),
        decision: decision.to_string(),
        action,
        params,
        extra,
        ending,
        protocol,
    }
}

/// 把行动结果变成一条"观察到结果"的经历。
pub fn observed_payload(
    operation: &OperationId,
    action: &str,
    params: &ActionParams,
    outcome: &ActionOutcome,
) -> TracePayload {
    TracePayload::Observed {
        turn: operation.turn().number(),
        index: operation.index(),
        action: action.to_string(),
        params: params_pairs(params),
        state: outcome.state(),
        failure: outcome.failure_class().map(str::to_string),
        blocks: outcome
            .content()
            .iter()
            .map(|block| (block.label().to_string(), block.text().to_string()))
            .collect(),
    }
}

/// 把一次行动选择变成一条"执行之前"的经历。
pub fn selected_payload(
    operation: &OperationId,
    action: &str,
    params: &ActionParams,
) -> TracePayload {
    TracePayload::ActionSelected {
        turn: operation.turn().number(),
        index: operation.index(),
        action: action.to_string(),
        params: params_pairs(params),
    }
}

/// 把收场结果变成一条结束经历。
pub fn ending_payload(outcome: Outcome) -> TracePayload {
    match outcome {
        Outcome::Subject(ending) => TracePayload::Ending {
            subject: Some(subject_str(ending).to_string()),
            system: None,
            failure: None,
        },
        Outcome::System(ending) => TracePayload::Ending {
            subject: None,
            system: Some(system_str(ending).to_string()),
            // 分类跟着一起进盘：`SystemEnding::ModelFailed` 是**唯一带载荷的收场值**，
            // 而 `system_str` 只给到"模型失败"这一层 ⇒ 载荷在这里补回来。
            // 与事实侧 `run-ended` 写 `failure` 是**同一个口径**（缺 = 当时就没带分类）。
            failure: match ending {
                crate::foundation::SystemEnding::ModelFailed(Some(class)) => {
                    Some(class.to_string())
                }
                _ => None,
            },
        },
    }
}

/// 内容块的机械转换：**只搬标签与文本**。
///
/// ⚠ **它是"材料不落盘"那条判据的落点，而且靠的是形状不是约定**：产物类型是
/// `Vec<(String, String)>`，装不下一份材料（[`ContentBlock::material`]）。⇒ 谁想把字节写进账本，
/// 得先改这个函数的返回类型——那是一个会被看见的动作，而不是一次疏忽。
///
/// 判据一句话：**引用进经历，字节只进编码。** 一块带材料的块与不带材料的块，在这里产出**逐字节
/// 相同**的那一对（用例：`a_material_never_reaches_the_ledger`）。
pub fn content_pairs(blocks: &[ContentBlock]) -> Vec<(String, String)> {
    blocks
        .iter()
        .map(|block| (block.label().to_string(), block.text().to_string()))
        .collect()
}

/// 参数对的机械转换：按插入顺序，不改名、不解释。
pub fn params_pairs(params: &ActionParams) -> Vec<(String, String)> {
    params
        .entries()
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// 两个注入阶段落到盘上的名字。**编码与解码共用同一对常量**（权威在
/// [`crate::store::RUN_START_STAGE`] 那一处，本件不另写一遍字面量）。
fn stage_str(stage: crate::core::InjectionStage) -> &'static str {
    match stage {
        crate::core::InjectionStage::RunStart => RUN_START_STAGE,
        crate::core::InjectionStage::Turn => TURN_STAGE,
    }
}

pub(super) fn state_str(state: ResultState) -> &'static str {
    match state {
        ResultState::Success => "success",
        ResultState::Failure => "failure",
        ResultState::Unknown => "unknown",
    }
}

pub(super) fn decode_state(text: &str) -> Option<ResultState> {
    Some(match text {
        "success" => ResultState::Success,
        "failure" => ResultState::Failure,
        "unknown" => ResultState::Unknown,
        _ => return None,
    })
}

pub(super) fn subject_str(ending: crate::foundation::SubjectEnding) -> &'static str {
    match ending {
        // ⚠ **今天会写下去的两个**（2026-10-10）：`ended`（收场）与 `held`（搁下）。
        // 区别只在动词，两个都不带值。
        crate::foundation::SubjectEnding::Ended => "ended",
        crate::foundation::SubjectEnding::Held => "held",
        // 四个**旧值**：盘上有它们（2026-10-10 之前的历史），因此编码侧也要认——
        // 但**没有任何生产路径会再写它们**（唯一生产者是适配层，它只会给 `Ended`）。
        crate::foundation::SubjectEnding::Completed => "completed",
        crate::foundation::SubjectEnding::Silent => "silent",
        crate::foundation::SubjectEnding::Declined => "declined",
        crate::foundation::SubjectEnding::Waiting => "waiting",
    }
}

/// 系统收场的**分类名**（粗粒度那一层）。
///
/// 注意它**有意只给到这一层**：`ModelFailed(_)` 的载荷（[`crate::core::ModelFailureClass`]）
/// 不在这里，而是由 [`ending_payload`] 一并写进 `Ending::failure`。两处分工的理由：
/// `system` 是"哪一类收场"的稳定名（读它的人不需要更细），`failure` 是"那次失败具体怎么了"
/// 的读数（只有需要复盘的人要）。**不要再把载荷塞回这个函数的返回值**——它返回 `&'static str`，
/// 塞进去必然又丢一次。
pub(super) fn system_str(ending: crate::foundation::SystemEnding) -> &'static str {
    match ending {
        crate::foundation::SystemEnding::ModelFailed(_) => "model-failed",
        crate::foundation::SystemEnding::ActionFailed => "action-failed",
        crate::foundation::SystemEnding::Cancelled => "cancelled",
        crate::foundation::SystemEnding::Timeout => "timeout",
        crate::foundation::SystemEnding::Crashed => "crashed",
        crate::foundation::SystemEnding::Indeterminate => "indeterminate",
    }
}

/// 收场显示文本的**前缀**：它们只有一处拼法（见 [`subject_label`] / [`system_label`]）。
const SUBJECT_PREFIX: &str = "subject";
const SYSTEM_PREFIX: &str = "system";

/// 主体收场的**稳定显示文本**（今天写下去的那一个是 `subject:ended`；四个旧值照旧**读得回来**，
/// 历史上那些记录显示的是它们各自的词）。
///
/// **一处权威**（2026-10-06）：盘上的词表（[`SUBJECT_ENDINGS`] / [`system_str`]）与这个显示形状
/// 只在这里各写一遍。从前它有**四处私有副本**（`agent_host_status` / `host_loop` /
/// `manual_entry` / `diagnostics` 各一份），同一个收场在不同命令里各拼一遍——而它是**人看的那
/// 一格**，拼歪了就是"同一个事实两种说法"。
pub fn subject_label(ending: SubjectEnding) -> String {
    format!("{SUBJECT_PREFIX}:{}", subject_str(ending))
}

/// 系统终止的稳定显示文本（`system:model-failed`）。
///
/// 需要更多细节的调用方**在这上面加**，不要另写一份：`host_loop` 就是这么做的
/// （`system:model-failed model-failure=<class>`）。
pub fn system_label(ending: SystemEnding) -> String {
    format!("{SYSTEM_PREFIX}:{}", system_str(ending))
}

/// 收场结果的稳定显示文本（两条路各归各的前缀）。
pub fn outcome_label(outcome: Outcome) -> String {
    match outcome {
        Outcome::Subject(ending) => subject_label(ending),
        Outcome::System(ending) => system_label(ending),
    }
}

/// 由**盘上那两个已经校验过的词**拼同一份显示文本（解码侧用它，免得再拼一份前缀）。
pub fn ending_label(subject: Option<&str>, system: Option<&str>) -> Option<String> {
    match (subject, system) {
        (Some(word), None) => Some(format!("{SUBJECT_PREFIX}:{word}")),
        (None, Some(word)) => Some(format!("{SYSTEM_PREFIX}:{word}")),
        _ => None,
    }
}

pub(super) fn blocks_value(blocks: &[(String, String)]) -> serde_json::Value {
    serde_json::Value::Array(
        blocks
            .iter()
            .map(|(label, text)| {
                let mut object = serde_json::Map::new();
                object.insert("label".to_string(), serde_json::Value::from(label.as_str()));
                object.insert("text".to_string(), serde_json::Value::from(text.as_str()));
                serde_json::Value::Object(object)
            })
            .collect(),
    )
}

pub(super) fn decode_blocks(value: Option<&serde_json::Value>) -> Option<Vec<(String, String)>> {
    let array = value?.as_array()?;
    let mut blocks = Vec::new();
    for item in array {
        blocks.push((
            item.get("label")?.as_str()?.to_string(),
            item.get("text")?.as_str()?.to_string(),
        ));
    }
    Some(blocks)
}

pub(super) fn pairs_value(pairs: &[(String, String)]) -> serde_json::Value {
    blocks_value(pairs)
}

/// `extra-actions` 的 JSON 形状：`[{"action": …, "params": [{"label":…,"text":…}, …]}, …]`（**有序**）。
///
/// 顺序是身份的一部分：列表里第 k 项就是这一轮的第 k+1 个动作。
pub(super) fn extra_value(extra: &ExtraActions) -> serde_json::Value {
    serde_json::Value::Array(
        extra
            .iter()
            .map(|(action, params)| {
                let mut object = serde_json::Map::new();
                object.insert(
                    "action".to_string(),
                    serde_json::Value::from(action.as_str()),
                );
                object.insert("params".to_string(), pairs_value(params));
                serde_json::Value::Object(object)
            })
            .collect(),
    )
}

/// 读回 `extra-actions`：**没有这个字段就是"没有更多请求"**，不是损坏。
pub(super) fn decode_extra(value: Option<&serde_json::Value>) -> Option<ExtraActions> {
    let Some(value) = value else {
        return Some(Vec::new());
    };
    let array = value.as_array()?;
    let mut extra = Vec::with_capacity(array.len());
    for item in array {
        let action = item.get("action")?.as_str()?.to_string();
        let params = decode_pairs(item.get("params"))?;
        extra.push((action, params));
    }
    Some(extra)
}

/// 经历侧的**同一轮内子序号**：**缺字段 = 0**（旧环境里每一行都是这样）。
///
/// 有字段但不是 `0..=65535` 的整数 ⇒ 这一行读不出来。经历侧与事实侧的报错口径不同：
/// 事实侧要区分"取值范围非法"（`illegal-index`）与"整行损坏"，而经历侧解码的返回值本身
/// 就是"这一行能不能读出来"，因此这里不细分。
pub(super) fn stored_index(value: &serde_json::Value) -> Option<u16> {
    match value.get(INDEX_FIELD) {
        None => Some(0),
        Some(raw) => u16::try_from(raw.as_u64()?).ok(),
    }
}

pub(super) fn decode_pairs(value: Option<&serde_json::Value>) -> Option<Vec<(String, String)>> {
    let array = value?.as_array()?;
    let mut pairs = Vec::new();
    for item in array {
        pairs.push((
            item.get("label")?.as_str()?.to_string(),
            item.get("text")?.as_str()?.to_string(),
        ));
    }
    Some(pairs)
}

/// 从一个 JSON 行里取出字符串字段。
///
/// 这是**文本级最小解析**，不引入第二个解析器：只支持本模块自己写出的格式，
/// 读不到就是"损坏"，绝不猜一个默认值。
pub(super) fn extract_string_field(line: &str, field: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    Some(value.get(field)?.as_str()?.to_string())
}

/// JSON 字符串转义：只需要处理引号与反斜杠，以及控制字符。
pub(super) fn escape_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out
}

/// 尽力同步目录项（Windows 上打开目录句柄不被支持时静默跳过）。
pub(super) fn sync_directory(root: &Path) {
    #[cfg(not(windows))]
    {
        if let Ok(handle) = File::open(root) {
            let _ = handle.sync_all();
        }
    }
    #[cfg(windows)]
    {
        let _ = root;
    }
}

/// I/O 错误 → 稳定分类。
pub(super) fn io_error(error: std::io::Error) -> StoreError {
    match error.kind() {
        std::io::ErrorKind::NotFound => StoreError::NotFound,
        _ => StoreError::Io,
    }
}

/// 从一个已打开的环境里列出全部可查阅的活动（机械目录，无语义排序）。
///
/// 顺序是外部身份顺序（`RunId` 的字典序），不是"重要性"排序。
pub fn list_runs(env: &Environment) -> Result<Vec<RunSummary>, StoreError> {
    list_runs_at(env.root())
}

/// 按**环境目录**列出全部可查阅的活动。
///
/// 与 [`list_runs`] 是同一件事，只是入口是目录而不是已打开的写句柄。这样"读"不需要
/// 借用写者的 `&mut`——只读查阅是目录的函数，不是写者的函数。
///
/// 它是**读者**：读法由**写资格那条证据**决定（见 [`reading_tail`]），不由调用方挑。
pub fn list_runs_at(root: &Path) -> Result<Vec<RunSummary>, StoreError> {
    let mut runs = summaries_at(root, reading_tail(root))?;
    runs.sort_by(|left, right| left.run.cmp(&right.run));
    Ok(runs)
}

/// 按**环境目录**读一段经历（只读；读法见 [`reading_tail`]）。
///
/// **它读多少**（2026-10-06 卷内偏移索引那一刀）：先看**目录**知道该去哪一卷，再看那一卷的
/// **卷内偏移索引**（`<卷名>.idx`）拿到这一段活动的**字节区间**与**区间起点的链值**，然后
/// **seek 只读那一段**（约 31 KB）——不再把整卷读进来再筛。活动段没有索引（它还在长），
/// 走的是"整份读进来、但只解码这一段的行"那条路（见 [`read_trace_file_for_run`]）。
///
/// ⚠ **它保证什么、不保证什么**：**封口段**那几刀从索引给的链值起**独立校验**（因此读一个
/// 切片不必从头折叠整卷）；**活动段**那一半只保证"返回的那些行解码得出来、能解释、属于这一段、
/// 序号严格递增"（理由见 [`read_trace_file_for_run`]）。返回的每一条都解码得出来、都能被解释、
/// 都属于这一段。它**不**声称整份账本健康，也**不**读封口段的其余部分——**卷里别处**的静默损坏
/// 因此不挡这一次读；要"整份前缀都自洽"的读数，走 [`read_trace_at`] 或
/// [`EnvironmentReader::trace`]（"整份重验"那条路）。
pub fn read_run_at(
    root: &Path,
    run: &RunId,
    from_seq: Option<u64>,
    limit: usize,
) -> Result<TracePage, StoreError> {
    // ⚠ **2026-10-06 分卷之后**：先看**目录**——它记着每一卷里有哪几段活动，因此
    // **不读任何旧卷**就知道该去哪一卷找；不在任何旧卷里 ⇒ 在活动段。
    // 目录正是为此存在的（同族：Kafka 的段索引、KurrentDB 的 chunk 索引）。
    // 目录与盘对不上（或读不出来）时走 [`load_parts`] 那条**权威路**：它会重读所有卷把目录
    // 重建出来，再按它找（慢一次，之后照旧）。
    let parts: Vec<SealedPart> = match parts_holding(root, run)? {
        Some(parts) => parts,
        None => load_parts(root)?
            .into_iter()
            .filter(|part| part.holds(run))
            .collect(),
    };
    // ⚠ **一段活动的记录可以横跨两卷**（封口按字节触发，可能切在活动中间；而且环境级记录
    // ——例如事后写下的存储故障——会在活动封口之后才落盘）。因此这里**收全**：所有装着它的卷
    // 按顺序拼起来，再看活动段。从前只取**第一个**装它的卷 ⇒ 会**只读到前半截**。
    let mut selected: Vec<TraceRecord> = Vec::new();
    for part in &parts {
        selected.extend(part_run_records(root, part, run)?);
    }
    selected.extend(read_trace_file_for_run(
        &root.join(TRACE_FILE),
        reading_tail(root),
        run,
    )?);
    Ok(page_of(selected, from_seq, limit))
}

/// 目录里**装着这一段活动**的那几卷（只解那几行，不把整份目录解一遍）。
///
/// **为什么要有它**：目录一行一卷，每行还带着**这一卷的活动摘要**（`list_runs` 与准入读数要
/// 那几格）——那是 O(全部活动) 的一串 JSON 对象。而"查一段旧活动"只要"哪一卷装着它"：
/// 整份解码目录会让这一问随**全部历史**变慢（实测 1200 段活动上占 13–15 ms，是这一刀之前
/// 的大头），而它该是一个常数。
///
/// **判据与 [`load_parts`] 完全相同**（目录与盘上的卷**逐一对应**才用它），只是核对按
/// **文件名**做：行数一致，而且第 i 行点的就是盘上第 i 卷（两边都按首序号排序）。对不上 ⇒
/// `None`，调用方走 [`load_parts`] 那条权威路——**尾部半行**也落在"对不上"里（权威路容忍它、
/// 快路不认它）：那只是慢一点，不是错一点；下一次封口整份重写目录时它自然消失。
///
/// **它会不会漏行**：预筛的针与写侧**同一个序列化器**生成（`serde_json::Value::from(run)`），
/// 另加"活动身份原样出现"那一针取并集——宁可多解一行，不可漏一行；命中错了无害（解出来还有
/// [`SealedPart::holds`] 兜着）。
fn parts_holding(root: &Path, run: &RunId) -> Result<Option<Vec<SealedPart>>, StoreError> {
    let text = match fs::read_to_string(root.join(PARTS_INDEX)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(io_error(error)),
    };
    // 只看**骨架行**：摘要行那一串八字段对象是 O(全部活动) 的解析，而这里要的只是"哪一卷装着它"。
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.contains(RUNS_LINE_MARK))
        .collect();
    let on_disk = part_files(root);
    // 逐一对应：行数一致，而且第 i 行点的就是盘上第 i 卷。名字按**字段形状**核
    // （`"file":"<名字>"`），不是裸子串——裸子串会让"某一段活动恰好叫这个名字"冒充成一条卷。
    let in_sync = lines.len() == on_disk.len()
        && lines.iter().zip(on_disk.iter()).all(|(line, (_, path))| {
            path.file_name().is_some_and(|name| {
                line.contains(&format!("\"file\":\"{}\"", name.to_string_lossy()))
            })
        });
    if !in_sync {
        return Ok(None);
    }
    // 针：折好的身份在骨架行里就是一个 **JSON 字符串**（`["<身份>",<标志>]`），因此针取
    // "同一个序列化器写出的那个字符串"（与写侧逐字节同形，不是手写转义）。
    let needle = serde_json::Value::from(run.as_str()).to_string();
    let mut parts = Vec::new();
    for line in lines {
        if !line.contains(&needle) && !line.contains(run.as_str()) {
            continue;
        }
        // 这一行读不出来：整份目录都不可信 ⇒ 交给权威那一路（它会重建）。
        let Some(part) = decode_part(line) else {
            return Ok(None);
        };
        if part.holds(run) {
            parts.push(part);
        }
    }
    parts.sort_by_key(|part| part.seq_start);
    Ok(Some(parts))
}

/// 读**一卷里某一段活动**的记录：索引给的区间优先，读不通就重读该卷重建。
///
/// 三条一起成立（[`RunSpan`] 的文档解释了为什么）：
///
/// - **索引是派生的**：丢了、中间的行读不出来、或这一刀没读通 ⇒ 重读该卷**重建**一次
///   （卷是权威，索引只是索引）。重建会把整卷从头核一遍，因此"索引错"与"卷坏"在这里分得清：
///   前者重建之后这一刀就通了，后者会在重建那一步**如实报出同一条损坏**（不吞掉）。
/// - **横跨两卷照旧收全**：这里只管"这一卷里有哪些"；跨卷的拼接收口在 [`read_run_at`]。
/// - **区间只是超集**：`[first, end)` 里可能夹着别人的行，解码后按活动身份再筛一次。
fn part_run_records(
    root: &Path,
    part: &SealedPart,
    run: &RunId,
) -> Result<Vec<TraceRecord>, StoreError> {
    if let Some(spans) = read_part_index(root, part)?
        && let Some(span) = spans.iter().find(|span| &span.run == run)
        && let Some(records) = read_trace_slice(&part.path(root), span, run)?
    {
        return Ok(records);
    }
    let (_, records) = rebuild_part_index(root, part)?;
    Ok(records
        .into_iter()
        .filter(|record| record.run() == run)
        .collect())
}

/// 从一组记录里取出某一段的那一页（`has_more` 按过滤后的条数算）。
fn page_of(records: Vec<TraceRecord>, from_seq: Option<u64>, limit: usize) -> TracePage {
    let mut selected: Vec<TraceRecord> = records
        .into_iter()
        .filter(|record| from_seq.is_none_or(|from| record.seq() >= from))
        .collect();
    let has_more = selected.len() > limit;
    selected.truncate(limit);
    TracePage {
        records: selected,
        has_more,
    }
}

#[cfg(test)]
mod tests {
    use super::{TracePayload, observed_payload, outcome_of_observation};
    use crate::core::{ActionOutcome, ActionParams, ContentBlock, Material};
    use crate::foundation::{OperationId, ResultState, RunId, Turn};

    /// **写侧能写出的每一种形状，读侧都必须读回成它自己**（2026-10-13 按真机补）。
    ///
    /// 这一条守的是"同一份记录两处说法不同"：形状的知识只有一处（构造入口），两侧由**往返**对齐
    /// ——生产侧不再有一张会漂的枚举表。真机缺陷的形状：`failure` 带随单读数（参数拒绝的读数）
    /// 写得出来、读不回来 ⇒ 那一段之后的每一段新活动都在恢复那一步被拒，主体从此不再回话。
    #[test]
    fn every_shape_the_writer_can_write_reads_back_as_itself() {
        let operation = OperationId::at(RunId::new("run-1"), Turn::FIRST, 0);
        let params = ActionParams::none();
        let shapes = vec![
            ActionOutcome::success(vec![ContentBlock::new("read", "读数")]),
            ActionOutcome::success(Vec::new()),
            ActionOutcome::failure("invalid-parameter"),
            // ⚠ **就是这一格**：失败 ＋ 随单读数。
            ActionOutcome::failure_with_content(
                "invalid-parameter",
                vec![ContentBlock::new("parameters", "missing = id")],
            ),
            ActionOutcome::unknown(),
        ];
        for outcome in shapes {
            let payload = observed_payload(&operation, "act", &params, &outcome);
            let TracePayload::Observed {
                state,
                failure,
                blocks,
                ..
            } = &payload
            else {
                panic!("观察必须写成 observed 那一格");
            };
            let back = outcome_of_observation(*state, failure.as_ref(), blocks)
                .expect("写过的形状必须读得回来");
            assert_eq!(back.state(), outcome.state(), "三分必须一致");
            assert_eq!(
                back.failure_class(),
                outcome.failure_class(),
                "分类必须一致"
            );
            assert_eq!(
                back.content().len(),
                outcome.content().len(),
                "随单读数必须一致"
            );
        }
    }

    /// **材料不进账本**——这是**形状保证**，不是约定。
    ///
    /// 判据：一块**带材料**的块与一块**不带材料**的块，写进记录的那一对**逐字节相同**；读回来之后
    /// 材料是 `None`（它从来没有被写下去过）。⇒ "抽走图片"这件事在**经历层面不可能发生**，
    /// 因为**经历里从来没有过像素**。
    #[test]
    fn a_material_never_reaches_the_ledger() {
        let operation = OperationId::at(RunId::new("run-1"), Turn::FIRST, 0);
        let params = ActionParams::none();
        let text = "attachment = abc, content_type = image/png";
        let carried = ActionOutcome::success(vec![
            ContentBlock::new("im_read_attachment", text)
                .with_material(Material::new("image/png", vec![0x89, 0x50, 0x4e, 0x47])),
        ]);
        let plain = ActionOutcome::success(vec![ContentBlock::new("im_read_attachment", text)]);

        let written = observed_payload(&operation, "act", &params, &carried);
        assert_eq!(
            written,
            observed_payload(&operation, "act", &params, &plain),
            "材料不得改变写下去的那一对（产物类型装不下它）"
        );

        let TracePayload::Observed {
            state,
            failure,
            blocks,
            ..
        } = &written
        else {
            panic!("观察必须写成 observed 那一格");
        };
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].1, text, "读数是文本，照旧在");

        let back = outcome_of_observation(*state, failure.as_ref(), blocks).expect("读得回来");
        assert!(
            back.content()
                .iter()
                .all(|block| block.material().is_none()),
            "读回来的块不带材料——它从来没有被写下去过"
        );
    }

    /// **读侧不认识的形状仍然 fail-closed**（收窄只朝一个方向：多认一格写过的，不少拒一格读不懂的）。
    #[test]
    fn a_shape_the_writer_never_writes_is_still_refused() {
        // 「不知道」今天没有带读数的构造入口 ⇒ 有读数即拒绝。
        let refused = outcome_of_observation(
            ResultState::Unknown,
            None,
            &[("read".to_string(), "读数".to_string())],
        );
        assert!(refused.is_err(), "读不懂的形状不许被读成某一档");
        // 失败却不带分类 ⇒ 同样拒绝。
        let classless = outcome_of_observation(ResultState::Failure, None, &[]);
        assert!(classless.is_err(), "失败必须带分类");
        // 成功却带分类 ⇒ 同样拒绝。
        let overclaimed = outcome_of_observation(
            ResultState::Success,
            Some(&"invalid-parameter".to_string()),
            &[],
        );
        assert!(overclaimed.is_err(), "成功不带分类");
    }
}
