//! 模型调用的**有界重试**（TASK_60 裁决 5）：传输层瞬时失败 ⇒ **立即再采一次**，只一次。
//!
//! 为什么要有它：主体的模型调用会**窗口级**失败（实测：约 45 次开机里 7 拍失败，
//! 3 次是"该次开机头两轮全失败"⇒ 窗口比单次调用长）。而适配层**一次都不重试**，
//! 于是**一次网络抖动 = 主体那一轮一个字都没说**。
//!
//! 三条边界（都是裁决写死的，别在这里松动）：
//! 1. **只在模型调用层**。公共写（消息等公共事实上报）的零重试纪律**一字不动**——
//!    那条管的是公共消息的幂等与重放；模型调用失败时主体**还没说任何话、没碰任何公共事实**，
//!    所以重试不是"重发"。
//! 2. **有界 1 次、立即、零参数**（没有退避、没有间隔旋钮）。窗口内第二次采样**无论成败**
//!    都是定位数据；要退避得先有"窗口内重试全失败"的数据，不预建。
//! 3. **事实面不动**：重试不在 `facts.ndjson` / 持久 schema 里留任何新字段——
//!    重试**不是一个新的失败**，它只是同一个失败**多采了一次**。留痕走**诊断面**
//!    （见 [`ModelRetry`]：记录随这一次驱动的结果交回调用方，由**宿主**打印，
//!    见 `host_loop` → 宿主那行；本模块**自己不打任何字**——库不打印）。
//!
//! 触发判据（[`is_transient_failure`]）：**看行动句**——凡是"下一步 = 先立即重采一次"的档才触发。
//! 今天那是 `Transport` 的**四个细分档**（`dns` / `refused` / `timeout` / `io`，见 `core.rs` 上各档的行动句）；
//! **`Transport(None)`（未细分）不在族里**：认不出可能是有持久原因的那一类（证书／TLS 刻意没发布，
//! 它正落在这里），那种东西重采一次不会变 ⇒ 先取证（形态指纹那一行），量出形状再议。
//! `HttpStatus` 那几档也不在这里：它们的行动句是"等一会儿 / 换钥匙 / 改请求 / 上报"
//! （429 明确要**等**，不是立刻重采），而那一条**不在本刀**——要并进来得先有带码的 5xx 档。

use crate::core::{ModelError, ModelFailureClass, ModelOutput, ModelPort, View};

/// 第一次失败之后**允许再采几次**（裁决：1 次、立即、零参数）。
pub const TRANSIENT_RETRY_ATTEMPTS: u32 = 1;

/// 这一档失败**值不值得立刻再采一次**？（纯函数 ⇒ 可判错）
///
/// 判据是**这一族对应的下一步行动**，而 `Transport` 全族的行动是"够不着 ⇒ 先立刻再试"：
/// 五个价（`dns` / `refused` / `timeout` / `io` / 未细分）都可能只是**窗口内的瞬时**。
/// 其余八档都不在这里：它们是"响应到手了但不合契约"或"对方按规矩拒了"，**再采一次不会变**。
pub fn is_transient_failure(class: ModelFailureClass) -> bool {
    // **只看带细分的那些档**：`Transport(None)`（未细分）**不在族里**。
    // 为什么：认不出可能正是"有持久原因"的那一类（证书／TLS 刻意没发布，它落在这里），
    // 而对有持久原因的东西重采一次不会变。⇒ 那一档先靠**形态指纹**取证（TASK_60 §6），
    // 量出稳定形状之后再决定并进族里。**代价写在案上**：主体至今观测到的失败全是未细分
    // ⇒ 收窄之后那些拍**不会**被重试，直到指纹把形状叫出来。
    matches!(class, ModelFailureClass::Transport(Some(_)))
}

/// 记一次"采了第二次"——**诊断面**，不是事实（见模块头第 3 条）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelRetry {
    /// 这是第几次采样（`2` = 第一次失败之后的那一次）。
    pub attempt: u32,
    /// 第一次失败的分类（为什么重试）。
    pub reason: ModelFailureClass,
    /// 重试的结果：`Ok` = 这一次成功了（活动照常往下走）；`Err` = 还是失败（带它自己的分类）。
    pub outcome: Result<(), ModelFailureClass>,
}

impl ModelRetry {
    /// 诊断行（**宿主**打这一行；本模块只给格式，不打印）。
    ///
    /// 形状与既有的 `model-failed run=… failure=…` 同族：一行、`key=value`、无空白取值。
    /// `retry=yes` 是"这是重试"的标记（让日志里一眼能捞出来），`reason=` 是为什么重试，
    /// `result=` 是结果（`ok` 或那一次失败自己的分类）。
    pub fn trace_line(&self) -> String {
        let result = match self.outcome {
            Ok(()) => "ok".to_string(),
            Err(class) => class.as_str().to_string(),
        };
        format!(
            "model-retry attempt={} reason={} result={}",
            self.attempt,
            self.reason.as_str(),
            result
        )
    }
}

/// 把"再采一次"包在**已经装好的**那个模型外面（`inner` 是借来的 ⇒ 不改变模型的类型与所有权）。
///
/// 为什么是**装饰器**而不是改适配层：重试是**调用行为**，不是某一家的协议细节——
/// 两条 wire（chat / responses）与未来的别家都该一样。放在这一层，实现只有一份。
#[derive(Debug)]
pub struct RetryingModel<'a, M: ModelPort> {
    inner: &'a mut M,
    attempts: u32,
    report: Vec<ModelRetry>,
}

impl<'a, M: ModelPort> RetryingModel<'a, M> {
    /// `attempts` = 第一次失败之后还允许再采几次（产品给 [`TRANSIENT_RETRY_ATTEMPTS`]；
    /// 用例给 `0` 就是"关掉重试"的负对照，见 `model_retry` 自己的用例）。
    pub fn new(inner: &'a mut M, attempts: u32) -> Self {
        Self {
            inner,
            attempts,
            report: Vec::new(),
        }
    }

    /// 这一次（驱动／活动）里发生过的重试，按发生顺序。
    pub fn report(&self) -> &[ModelRetry] {
        &self.report
    }

    /// 取走记录（调用方把它接到自己的读数上；记录不留在模型里）。
    pub fn into_report(self) -> Vec<ModelRetry> {
        self.report
    }
}

impl<M: ModelPort> ModelPort for RetryingModel<'_, M> {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        let first = match self.inner.complete(view) {
            Ok(output) => return Ok(output),
            Err(error) => error,
        };
        if self.attempts == 0 || !is_transient_failure(first.class) {
            return Err(first);
        }
        // **立即**再采一次：同一个视野、零参数、没有间隔。第一次失败的那一份**不被丢弃**——
        // 若第二次也失败，交出去的是**第二次**（最近一次读数），而第一次的分类留在记录里。
        let mut retry = ModelRetry {
            attempt: 2,
            reason: first.class,
            outcome: Ok(()),
        };
        match self.inner.complete(view) {
            Ok(output) => {
                self.report.push(retry);
                Ok(output)
            }
            Err(second) => {
                retry.outcome = Err(second.class);
                self.report.push(retry);
                Err(second)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        Agent, Capabilities, Decision, HttpStatusDetail, Limits, ModelFailureClass, ModelOutput,
        Run, TransportDetail,
    };
    use crate::foundation::{AgentId, RunId};

    /// 替身模型：按脚本依次作答，**把被调用了几次记下来**（重试判据就落在这个计数上）。
    struct Scripted {
        answers: Vec<Result<(), ModelFailureClass>>,
        calls: usize,
    }

    impl Scripted {
        /// 永远按同一个答案作答 —— 用来判"重试是不是有界"。
        fn always(answer: Result<(), ModelFailureClass>) -> Self {
            Self {
                answers: vec![answer],
                calls: 0,
            }
        }

        /// 依次作答：`first` 之后一律 `then` —— 用来判"重试成功 / 仍失败"两条路。
        fn once_then(
            first: Result<(), ModelFailureClass>,
            then: Result<(), ModelFailureClass>,
        ) -> Self {
            Self {
                answers: vec![first, then],
                calls: 0,
            }
        }
    }

    impl ModelPort for Scripted {
        fn complete(&mut self, _view: &View) -> Result<ModelOutput, ModelError> {
            let index = self.calls.min(self.answers.len() - 1);
            let answer = self.answers[index];
            self.calls += 1;
            match answer {
                Ok(()) => Ok(ModelOutput::new(String::new(), None, Decision::Yield)),
                Err(class) => Err(ModelError { class }),
            }
        }
    }

    /// 造一个可以喂给 `complete` 的视野（替身不看它，但签名要）。
    fn view() -> View {
        let mut agent = Agent::new(AgentId::new("agent-retry"));
        let run = Run::begin(&mut agent, RunId::new("run-retry"), Limits::new(1))
            .expect("可以开始一个活动");
        run.view(Capabilities::none())
    }

    /// 跑一次 `complete`，把**结果与记录**带出来（装饰器借走的 `inner` 在出块时还给外面，
    /// 这样断言才能读 `inner.calls`）。
    fn drive_once(
        inner: &mut Scripted,
        attempts: u32,
    ) -> (Result<(), ModelFailureClass>, Vec<ModelRetry>) {
        let mut model = RetryingModel::new(inner, attempts);
        let outcome = model
            .complete(&view())
            .map(|_| ())
            .map_err(|error| error.class);
        let report = model.into_report();
        (outcome, report)
    }

    #[test]
    fn a_transient_transport_failure_is_sampled_once_more_and_can_recover() {
        let mut inner = Scripted::once_then(
            Err(ModelFailureClass::Transport(Some(TransportDetail::Refused))),
            Ok(()),
        );
        let (outcome, report) = drive_once(&mut inner, TRANSIENT_RETRY_ATTEMPTS);
        assert_eq!(outcome, Ok(()), "第二次采成功了 ⇒ 这次调用当成成功交出去");
        assert_eq!(inner.calls, 2, "恰好再采一次");
        assert_eq!(report.len(), 1);
        assert_eq!(report[0].attempt, 2);
        assert_eq!(
            report[0].reason,
            ModelFailureClass::Transport(Some(TransportDetail::Refused))
        );
        assert_eq!(report[0].outcome, Ok(()));
    }

    #[test]
    fn the_retry_is_bounded_to_one_more_sample_and_surfaces_what_it_saw() {
        // 两次都失败：**不能**再采第三次；交出去的是**第二次**那份（最近一次读数），
        // 第一次的分类留在记录里（"为什么重试"）。
        let mut inner = Scripted::once_then(
            Err(ModelFailureClass::Transport(Some(TransportDetail::Timeout))),
            Err(ModelFailureClass::Transport(Some(TransportDetail::Io))),
        );
        let (outcome, report) = drive_once(&mut inner, TRANSIENT_RETRY_ATTEMPTS);
        assert_eq!(inner.calls, 2, "有界：只许再采一次");
        assert_eq!(
            outcome,
            Err(ModelFailureClass::Transport(Some(TransportDetail::Io))),
            "交出去的是最近一次读数"
        );
        assert_eq!(report.len(), 1);
        assert_eq!(
            report[0].reason,
            ModelFailureClass::Transport(Some(TransportDetail::Timeout))
        );
        assert_eq!(
            report[0].outcome,
            Err(ModelFailureClass::Transport(Some(TransportDetail::Io)))
        );
    }

    #[test]
    fn attempts_zero_turns_the_retry_off() {
        // 门禁 6 的负对照：把次数关成 0 ⇒ 只采一次、错误照旧交出去、记录为空。
        // 判据的反向读法：把 `attempts == 0` 那条早退去掉 ⇒ 这条立刻红。
        let mut inner = Scripted::always(Err(ModelFailureClass::Transport(Some(
            TransportDetail::Refused,
        ))));
        let (outcome, report) = drive_once(&mut inner, 0);
        assert!(outcome.is_err());
        assert_eq!(inner.calls, 1, "关掉就是一次都不多采");
        assert!(report.is_empty());
    }

    #[test]
    fn non_transport_failures_are_not_retried() {
        // 这些档的行动句不是"立刻再采"：429 要**等**、契约类要**改**、凭据类要**换钥匙**——
        // 再采一次都不会变。
        for class in [
            // **未细分放在最前面**：它是本刀收窄的落点（见上一条用例）。
            ModelFailureClass::Transport(None),
            ModelFailureClass::HttpStatus(Some(HttpStatusDetail::RateLimited)),
            ModelFailureClass::HttpStatus(Some(HttpStatusDetail::ServerSide)),
            ModelFailureClass::HttpStatus(Some(HttpStatusDetail::Credential)),
            ModelFailureClass::HttpStatus(None),
            ModelFailureClass::ContractViolation,
            ModelFailureClass::UnparsableResponse,
            ModelFailureClass::StoppedGeneration,
            ModelFailureClass::ProtocolPayload,
            ModelFailureClass::UnpairedHistory,
            ModelFailureClass::UnsupportedToolName,
            ModelFailureClass::ReservedToolName,
        ] {
            let mut inner = Scripted::always(Err(class));
            let (outcome, report) = drive_once(&mut inner, TRANSIENT_RETRY_ATTEMPTS);
            assert_eq!(outcome, Err(class));
            assert_eq!(inner.calls, 1, "{class:?} 不该重试");
            assert!(report.is_empty(), "{class:?} 不该留下重试记录");
        }
    }

    #[test]
    fn the_transient_family_is_exactly_the_four_detailed_transport_classes() {
        // 触发判据本身可判错：**四个细分档**为真；**未细分（`None`）为假**——
        // 收窄的判据就在这里（认不出可能是有持久原因的那一类，先取证再议）。
        for class in [
            ModelFailureClass::Transport(Some(TransportDetail::Dns)),
            ModelFailureClass::Transport(Some(TransportDetail::Refused)),
            ModelFailureClass::Transport(Some(TransportDetail::Timeout)),
            ModelFailureClass::Transport(Some(TransportDetail::Io)),
        ] {
            assert!(is_transient_failure(class), "{class:?} 属于瞬时候选族");
        }
        assert!(
            !is_transient_failure(ModelFailureClass::Transport(None)),
            "未细分不在族里：认不出可能正是有持久原因的那一类（证书／TLS）"
        );
        for class in [
            ModelFailureClass::HttpStatus(None),
            ModelFailureClass::HttpStatus(Some(HttpStatusDetail::Credential)),
            ModelFailureClass::HttpStatus(Some(HttpStatusDetail::RateLimited)),
            ModelFailureClass::HttpStatus(Some(HttpStatusDetail::ServerSide)),
            ModelFailureClass::HttpStatus(Some(HttpStatusDetail::RequestRejected)),
            ModelFailureClass::UnparsableResponse,
            ModelFailureClass::ContractViolation,
            ModelFailureClass::StoppedGeneration,
            ModelFailureClass::UnsupportedToolName,
            ModelFailureClass::ReservedToolName,
            ModelFailureClass::UnpairedHistory,
            ModelFailureClass::ProtocolPayload,
        ] {
            assert!(!is_transient_failure(class), "{class:?} 不在瞬时候选族里");
        }
    }

    #[test]
    fn the_trace_line_marks_the_retry_and_carries_its_result() {
        // 裁决要的那一行：**含"这是重试"的标记与结果**，形状与既有 `model-failed` 行同族
        // （一行、key=value、取值无空白 ⇒ `\S+` 的消费方整吃）。
        let recovered = ModelRetry {
            attempt: 2,
            reason: ModelFailureClass::Transport(Some(TransportDetail::Refused)),
            outcome: Ok(()),
        };
        assert_eq!(
            recovered.trace_line(),
            "model-retry attempt=2 reason=transport-failure:refused result=ok"
        );
        let still_failing = ModelRetry {
            attempt: 2,
            reason: ModelFailureClass::Transport(None),
            outcome: Err(ModelFailureClass::Transport(Some(TransportDetail::Timeout))),
        };
        assert_eq!(
            still_failing.trace_line(),
            "model-retry attempt=2 reason=transport-failure result=transport-failure:timeout"
        );
    }
}
