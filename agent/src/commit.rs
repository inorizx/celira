//! 把 Core 的中立提交接缝接到身份环境上的适配器。
//!
//! 它是**翻译层**：Core 说"提交这条经历 / 这个事实"，适配器把它写成环境里的一行；
//! Core 的词汇表里因此不会出现路径、文件名或存储格式。
//!
//! 关键顺序约束（与执行单的检查点表一一对应）：
//!
//! ```text
//! run-start → input → 每条经历 → 行动选择（执行前）→ 发出 → 结果 → 终态 → 封存
//! ```
//!
//! 每个方法在返回 `Err` 之前**不改变任何已确认前缀**：已写的行保持已写，失败的那一步就是
//! 没有被确认。调用方（Core）看到 `CommitError` 会停止继续，因此不会出现"保存失败但模型
//! 又被调用了一次"这种情况。

use crate::core::{
    Action, ActionOutcome, CommitError, CommitSeam, CommitStage, RunInput, TraceEntry,
};
use crate::cost::RunCost;
use crate::foundation::{AgentId, OperationId, Outcome, Record, RunId, Turn};
use crate::store::{
    Environment, STATIC_HEAD_LABEL, STATIC_HEAD_STAGE, StoreError, TracePayload, WakeOrigin,
    ending_payload, observed_payload, payload_of, selected_payload,
};

/// 把一次活动的经历与事实写进一个身份环境。
///
/// 它**借用**环境：写资格仍归 [`Environment`] 独占持有，本适配器只负责在正确的时机追加。
pub struct EnvironmentCommit<'env> {
    env: &'env mut Environment,
    /// **唤醒缘由**：这次活动是谁叫醒的（宿主的声明）。`None` = 这次没有声明起源
    /// （手动默认形状、离线夹具的旧入口）⇒ `run-start` 行不写这个字段，与旧形状逐字节相同。
    ///
    /// 它是**词表值**（[`WakeOrigin`]）不是人话：持久字段存稳定字符串，人话那句由渲染点
    /// 现查现派生（见 [`WakeOrigin::text`]）。
    ///
    /// 它由调用方（会话装配层）随 [`EnvironmentCommit::with_origin`] 给出，Core 的接缝
    /// **不认识**它——起源是宿主的事实，不是执行层的中立概念。
    origin: Option<WakeOrigin>,
    /// **这一次装订的静态头**（会话装配层声明的事实）。`None` = 这一刻没有那一份
    /// （主体摘了说明书、家里本来就没有，或调用方没有声明它）。
    ///
    /// 与 `origin` 同款：Core 的接缝**不认识**它——出厂说明是**环境家具**，不是执行层的
    /// 中立概念。它在这里只有一个用处：装订那一刻被**如实写下来**
    /// （[`EnvironmentCommit::commit_static_head`]）。
    head: Option<String>,
}

impl<'env> EnvironmentCommit<'env> {
    /// 用一次活动的身份环境建立提交适配器（**不声明起源**：`run-start` 行不写起源字段）。
    pub fn new(env: &'env mut Environment) -> Self {
        Self {
            env,
            origin: None,
            head: None,
        }
    }

    /// 同上，外加**声明起源**：这一次活动的 `run-start` 经历里写上「谁叫醒的」。
    ///
    /// 它收的是**词表值**（[`WakeOrigin`]）——写进盘里的因此是稳定字符串，不是一句话。
    pub fn with_origin(env: &'env mut Environment, origin: Option<WakeOrigin>) -> Self {
        Self {
            env,
            origin,
            head: None,
        }
    }

    /// 同上，再外加**声明这一次装订的静态头**（见 [`EnvironmentCommit::commit_static_head`]）。
    pub fn with_origin_and_head(
        env: &'env mut Environment,
        origin: Option<WakeOrigin>,
        head: Option<String>,
    ) -> Self {
        Self { env, origin, head }
    }

    /// **把这一次装订的静态头写下来**（会话装配层自己发起，Core 不参与）。
    ///
    /// 它回答的问题是「**这一刻装订的是哪一份**」。出厂说明的家在环境根（环境家具），
    /// 但"当时看见的是哪一份"是**历史**——历史的家在这里。两个事实、两个家。
    ///
    /// **缺席不落记录**：没有那一份时一个字节都不写（与 `origin` / `failure` 同一条规矩——
    /// 缺字段 = 没有被记录）。因此"没有装订记录"读作"那一刻没有头"，不编造。
    ///
    /// **按轮次追加**：开场装订一次，此后**每一个新进程重新拍读**一次（"Run 内不变"的边界是
    /// 进程），每一次都写一条、带上它生效的那一轮。于是"第 N 轮当时生效的是哪一份"是查出来的。
    ///
    /// ⚠️ **只作见证，不进视野**：Core 的轨迹里没有这一条（[`crate::store::rehydrate`] 对它返回
    /// "合法地不是"）。看见的那一份仍由适配层在渲染那一刻拼进请求的尾巴。
    pub fn commit_static_head(&mut self, run: &RunId, turn: Turn) -> Result<(), CommitError> {
        let Some(head) = self.head.as_deref() else {
            return Ok(());
        };
        self.env
            .append_trace(
                run,
                TracePayload::Injection {
                    stage: STATIC_HEAD_STAGE.to_string(),
                    turn: Some(turn.number()),
                    blocks: vec![(STATIC_HEAD_LABEL.to_string(), head.to_string())],
                },
            )
            .map_err(|error| Self::map(CommitStage::StaticHead, error))?;
        Ok(())
    }

    /// 存储错误 → Core 的中立提交失败。**只带阶段，不带任何存储细节**：Core 不需要认识它们。
    fn map(stage: CommitStage, error: StoreError) -> CommitError {
        let _ = error;
        CommitError::at(stage)
    }
}

impl CommitSeam for EnvironmentCommit<'_> {
    fn commit_run_start(&mut self, agent: &AgentId, run: &RunId) -> Result<(), CommitError> {
        // 归属由环境自己校验：写进来的活动必须属于这个环境持有的身份。
        if self.env.agent() != agent {
            return Err(CommitError::at(CommitStage::RunStart));
        }
        self.env
            .append_trace(
                run,
                TracePayload::RunStarted {
                    origin: self.origin,
                },
            )
            .map_err(|error| Self::map(CommitStage::RunStart, error))?;
        // 同一条事实也进事实存储：两份存储各自确认，不假设跨存储原子。
        self.env
            .append_fact(
                run,
                &Record::RunStarted {
                    agent: agent.clone(),
                    run: run.clone(),
                },
            )
            .map_err(|error| Self::map(CommitStage::RunStart, error))?;
        Ok(())
    }

    fn commit_input(&mut self, run: &RunId, input: &RunInput) -> Result<(), CommitError> {
        self.env
            .append_trace(
                run,
                TracePayload::RunInput {
                    text: input.as_str().to_string(),
                },
            )
            .map_err(|error| Self::map(CommitStage::Input, error))?;
        Ok(())
    }

    fn commit_trace(
        &mut self,
        run: &RunId,
        turn: Option<Turn>,
        entry: &TraceEntry,
    ) -> Result<(), CommitError> {
        self.env
            .append_trace(run, payload_of(entry, turn))
            .map_err(|error| Self::map(CommitStage::Trace, error))?;
        Ok(())
    }

    fn commit_action_selected(
        &mut self,
        run: &RunId,
        operation: &OperationId,
        action: &Action,
    ) -> Result<(), CommitError> {
        // 经历侧：Agent 选择了什么、参数是什么（这是"执行之前"的确认点）。
        self.env
            .append_trace(
                run,
                selected_payload(operation, action.id().as_str(), action.params()),
            )
            .map_err(|error| Self::map(CommitStage::ActionSelected, error))?;
        // 事实侧：操作已经准备。
        self.env
            .append_fact(
                run,
                &Record::ActionPrepared {
                    run: run.clone(),
                    operation: operation.clone(),
                },
            )
            .map_err(|error| Self::map(CommitStage::ActionSelected, error))?;
        Ok(())
    }

    fn commit_action_dispatched(
        &mut self,
        run: &RunId,
        operation: &OperationId,
    ) -> Result<(), CommitError> {
        self.env
            .append_fact(
                run,
                &Record::ActionDispatched {
                    run: run.clone(),
                    operation: operation.clone(),
                },
            )
            .map_err(|error| Self::map(CommitStage::ActionDispatched, error))?;
        Ok(())
    }

    fn commit_action_result(
        &mut self,
        run: &RunId,
        operation: &OperationId,
        outcome: &ActionOutcome,
    ) -> Result<(), CommitError> {
        // 事实侧：结果三分（不装正文）。
        self.env
            .append_fact(
                run,
                &Record::ActionResultRecorded {
                    run: run.clone(),
                    action: crate::foundation::ActionResult::new(
                        operation.clone(),
                        outcome.state(),
                    ),
                },
            )
            .map_err(|error| Self::map(CommitStage::ActionResult, error))?;
        Ok(())
    }

    fn commit_ending(
        &mut self,
        run: &RunId,
        outcome: Outcome,
        cost: RunCost,
    ) -> Result<(), CommitError> {
        // 经历侧：终态。
        self.env
            .append_trace(run, ending_payload(outcome))
            .map_err(|error| Self::map(CommitStage::Ending, error))?;
        // 事实侧：活动结束 ＋ **这一次的代价**（时长与用量，各自"没有就不写字段"）。
        //
        // **这里失败时，终态经历可能已经写下**：调用方可以断言"结束没有被完整确认"，
        // 不得断言"结束记录不存在"。
        //
        // 代价是**搬进来的**：本适配器不认识时长与用量，也不去算它们（见 `cost` 模块）。
        self.env
            .append_fact(
                run,
                &Record::RunEnded {
                    run: run.clone(),
                    outcome,
                    cost,
                },
            )
            .map_err(|error| Self::map(CommitStage::Ending, error))?;
        Ok(())
    }

    fn seal(&mut self, run: &RunId) -> Result<(), CommitError> {
        self.env
            .append_trace(run, TracePayload::Sealed)
            .map_err(|error| Self::map(CommitStage::Seal, error))?;
        Ok(())
    }
}

/// 把一次行动结果转成一条可保存的经历（给需要单独调用 `commit_trace` 的场合用）。
pub fn observation_payload(
    operation: &OperationId,
    action: &str,
    params: &crate::core::ActionParams,
    outcome: &ActionOutcome,
) -> TracePayload {
    observed_payload(operation, action, params, outcome)
}
