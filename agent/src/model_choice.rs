//! 入口的**线协议选择**（TASK_44）：两条 wire 各是一个独立实现，而入口只要"一个模型"。
//!
//! 抽象单位是**线协议**而不是供应商：两个实现**互不认识**（`responses_adapter` 只从
//! `provider_adapter` 借与协议无关的机械件），是**这里**同时认识两者——选择因此只落在一个地方。
//!
//! 为什么是**枚举**而不是 `Box<dyn ModelPort>`：Core 与会话函数收的是 `M: ModelPort`
//! （默认大小已知），指针类型要满足它就得给 `Box<dyn …>` 补实现、或者把 `?Sized` 放宽到
//! 调用链深处——两条都为了"运行时选一个实现"去改中立层。枚举让"选一次、之后是同一个具体类型"
//! 成立：Core 一行不动，两个适配层也互不认识。
//!
//! 本模块**不做**：工厂注册表、按名字动态发现、多协议路由、回退——只有一个二选一，而且它由
//! 宿主显式声明（`--model-protocol`）。

use crate::core::{ModelError, ModelOutput, ModelPort, View};
use crate::cost::UsageWriter;
use crate::model_readings::ModelReadingsWriter;
use crate::provider_adapter::DeepSeekModel;
use crate::responses_adapter::ResponsesModel;

/// 已经装配好的一个模型：走这一条 wire，或者那一条。
///
/// 它**不碰屏幕写柄**：装上传感器那一步由装配入口在各自的实现上做（`with_reading`），
/// 因此"cell 的唯一写者"这条纪律在文本级也守得住（`only_the_adapter_writes_the_reading_cell`）。
#[derive(Debug)]
pub enum ModelChoice {
    /// OpenAI 兼容的 chat completions（本家默认档也是它）。
    Chat(DeepSeekModel),
    /// Responses。
    Responses(ResponsesModel),
}

impl ModelChoice {
    /// **这一段清醒期由谁想**：请求里将要发出去的那个模型名（2026-10-08）。
    ///
    /// 它是**一处权威**：名字从"将要发请求的那个对象"里取，装配入口**不另算一份**——否则
    /// "主体读到的那一行"与"真正用的那一个"迟早对不上（配置门那一侧已经有四件套的解析，名字不该
    /// 有第二个来源）。开场的 `model` 那一块读的就是它。
    pub fn model(&self) -> &str {
        match self {
            ModelChoice::Chat(model) => model.model(),
            ModelChoice::Responses(model) => model.model(),
        }
    }

    /// **这一段想多用力**：请求里将要发出去的那一档（2026-10-08）。
    ///
    /// 与 [`ModelChoice::model`] 同一条判据、同一个来源：值从"将要发请求的那个对象"里取，装配
    /// 入口**不另算一份**——否则"主体读到的那一行"与真正发出去的请求迟早对不上。
    ///
    /// ⚠ **responses 那一条 wire 没有这个键**（它的请求体里没有 `reasoning_effort` 这一格），
    /// 因此它**永远返回 `None`**：那不是"读不出来"，是"这一档不发这个键"。而"清单里声明了档位
    /// 却选了 responses"这种配对错误**在入口就被拒掉**（见 `bin/agent_run.rs` 与
    /// `bin/agent_host.rs` 的装配那一处）——不让它走到这里变成一句沉默。
    pub fn reasoning_effort(&self) -> Option<&str> {
        match self {
            ModelChoice::Chat(model) => model.reasoning_effort(),
            ModelChoice::Responses(_) => None,
        }
    }

    /// **装配注入**：把用量 cell 的写柄转交给**当前这一条 wire** 的实现。
    ///
    /// 为什么要有这个转发：入口按线协议选实现，而"装上代价传感器"是两条协议共同的收尾——
    /// 放在这里，装配入口就不必match 一遍再各写一次（那会让"两条 wire 都装上"变成约定
    /// 而不是形状）。
    pub fn with_usage(self, usage: UsageWriter) -> Self {
        match self {
            ModelChoice::Chat(model) => ModelChoice::Chat(model.with_usage(usage)),
            ModelChoice::Responses(model) => ModelChoice::Responses(model.with_usage(usage)),
        }
    }

    /// **装配注入**：把逐轮读数账的写柄转交给**当前这一条 wire** 的实现。
    ///
    /// 与 [`ModelChoice::with_usage`] 同一个理由：入口按线协议选实现，而"装上这个传感器"是
    /// 两条协议共同的收尾——放在这里，装配入口就不必 match 一遍再各写一次（那会让"两条 wire
    /// 都装上"变成约定而不是形状）。
    pub fn with_model_readings(self, readings: ModelReadingsWriter) -> Self {
        match self {
            ModelChoice::Chat(model) => ModelChoice::Chat(model.with_model_readings(readings)),
            ModelChoice::Responses(model) => {
                ModelChoice::Responses(model.with_model_readings(readings))
            }
        }
    }
}

impl ModelPort for ModelChoice {
    /// **装订静态头**（TASK_79）：与 [`ModelChoice::with_usage`] 同一个理由——这是**两条 wire
    /// 共同**的一件事（"这一次活动开场读到的那一份"），放在这里转发，装配入口就不必 match 一遍
    /// 再各写一次：那会让"两条 wire 都装上"变成约定而不是形状。
    fn declare_static_head(&mut self, head: Option<&str>) {
        match self {
            ModelChoice::Chat(model) => model.declare_static_head(head),
            ModelChoice::Responses(model) => model.declare_static_head(head),
        }
    }

    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        match self {
            ModelChoice::Chat(model) => model.complete(view),
            ModelChoice::Responses(model) => model.complete(view),
        }
    }
}
