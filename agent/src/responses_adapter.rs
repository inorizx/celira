//! 第二条线协议：OpenAI **Responses**（TASK_44）。
//!
//! **抽象单位是线协议，不是供应商**：本文件实现 [`ModelPort`]，chat 那一条在
//! [`crate::provider_adapter`]。**共享的不是「机械件」而是一份语义核**——决策契约措辞、保留工具名
//! 与名字规则、`decode_invoke` / `decode_arguments`、finish / hold 的编码、`View` 渲染、失败折叠
//! ——它就住在 `provider_adapter` 里，本文件从它派生；**报文形状仍各自私有**，中间没有消息 IR
//! （两家协议时中间层是负资产，第三家那天再裁）。各写各的是请求体、响应解析与占用读数。
//!
//! 三条铁律（与 chat 侧同款，逐条落在本文件里）：
//!
//! 1. **状态主权**：请求**必带** `store: false`，每一轮把整段 `input` 重发一遍——服务端不持任何
//!    一份"我们的人生"，也没有 `previous_response_id` 那种链（那是把状态交给对面，不做）；
//! 2. **推理续接**：响应里的 reasoning 条目带 `encrypted_content` 时**原样**存进续接载荷、
//!    下一轮**原样**回传。这是 Responses 独有的真收益：o 系列推理链跨轮保留（chat completions 上
//!    它每轮被丢弃）。`encrypted_content` 是"服务端加密、客户端保存"——与自存铁律同构；
//! 3. **词表不跨层**：失败分类复用 chat 侧那九类（同一个 [`ProviderFailure`]，同样的状态码映射）。
//!
//! ## 一个名字不能用，是那一次调用的事
//!
//! 头部声明的三件（入口 / 收场 / 搁下）**每一件都要在解码处有自己的分支**：缺一件，那次调用
//! 就会掉进"以动作名直调"那条路、被**保留名**规则拒掉。判据（与 chat 侧同一条）：名字不能用是
//! **那一次调用**的拒绝（带一句读数、点着那个名字），不是整段清醒期判死刑；名字的合法性归
//! **声明侧**，回放侧只负责把模型当时说过的那次调用**原样引回去**（那次调用从未派发，回放它
//! 对配对是必需的，而且无损）。
//!
//! ⚠ **如实登记：本文件只做到解码那半，而且那半被自己的判序压住了。** `translate_response`
//! 里 `invoke` 那一支**不可达**（外层先拿保留名规则把它拒了），`history_items` 那边**仍旧**逐
//! 个校验名字 ⇒ 一次"名字不能用"的拒绝在下一轮**回放不出来**，整段仍以 `reserved-tool-name` /
//! `unsupported-tool-name` 收场。它是**已知缺口，不是已修事实**：两处各自在本文件的注释里点明，
//! **修法与归属待人工裁定**。同一条覆盖边界：这条 wire 的 `invoke` 解码**一条用例都没有**。
//!
//! ## 与 chat 侧的 wire 差异（本文件自己那一半）
//!
//! | | chat completions | 本文件（Responses） |
//! |---|---|---|
//! | 系统指令 | `messages[0]` = system | `instructions` 字段 |
//! | 对话 | `messages`（role 标记） | `input` 条目数组（user / function_call / function_call_output / reasoning / message） |
//! | 工具声明 | `tools[].function.{name,…}` 嵌套 | `tools[]` **平铺**（`type` / `name` / `description` / `parameters`） |
//! | 工具调用 | `message.tool_calls[]` + `finish_reason` | `output[]` 里的 `function_call` 条目（`call_id` / `name` / `arguments`） |
//! | 工具回执 | `role:"tool"` + `tool_call_id` | `input` 里的 `function_call_output` 条目（`call_id` / `output`） |
//! | 状态 | 无状态 | `store: false` 必带；`status` = `completed` / 别的 |
//! | 占用读数 | `usage.prompt_tokens`（送进去的上下文） | `usage.input_tokens`（同一口径） |
//!
//! **相邻配对**纪律与 chat 侧同款（它只认结构性关系，不是启发式）：一次请求了行动的响应必须紧邻
//! 它的回执、回执可以缺席（缺席时冲刷成**合成回执**，只存在于翻译产物里）、**没有对应请求的回执
//! 明确失败**。推理条目按服务端给的顺序原样摆在该轮 `function_call` **之前**——那一轮里它们本来
//! 就在前面，因此不会插进 `function_call` 与它的回执之间，相邻配对仍然成立。
//!
//! 官方依据（2026-09-18 核，`platform.openai.com/docs/api-reference/responses`）：端点
//! `POST /v1/responses`（宿主给出的端点值就是完整地址，与 chat 侧同一口径——本层**不**替它拼路径）、
//! `Authorization: Bearer <key>`、请求字段 `model` / `instructions` / `input` / `tools` / `store`。
//!
//! 本模块**不做**：托管工具（对面执行的那种搜索/代码解释器——执行面主权）、服务端状态
//! （`store: true` / `previous_response_id` 链）、自定义工具（自由文本参数）、自动重试、退避、
//! 流式、并发、供应商回退、多模型路由、统一中间表示。一次 `complete` 只发一次请求。

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::context_reading::ContextReadingWriter;
use crate::core::{
    Action, ActionId, Capabilities, Decision, Material, ModelError, ModelOutput, ModelPort,
    ModelResponse, ProtocolData, TraceEntry, View,
};
use crate::cost::{Usage, UsageWriter};
use crate::model_readings::{ModelReadingsWriter, TurnReadings};
use crate::provider_adapter::{
    ASSEMBLY_FAILURE, CLIENT_USER_AGENT, FINISH_DESCRIPTION, FINISH_TOOL, HOLD_DESCRIPTION,
    HOLD_TOOL, INVOKE_DESCRIPTION, INVOKE_TOOL, InvokeFailure, ProviderFailure, ShapeTrace,
    TIMEOUT, arguments_of, data_url, decision_contract, decode_arguments, decode_invoke,
    finish_ending, finish_schema, hold_ending, http_detail, injection_body, input_body,
    invoke_schema, observation_materials, observation_result, refusal_result, system_message,
    tool_name, tool_name_reading, unconfirmed_result,
};

/// 本协议写进续接载荷的**协议名**，也是换家档 `--model-protocol` 的取值之一。
///
/// 它与 chat 侧的那个名字不同，因此盘上旧载荷换协议后读不懂——那时明确失败（`protocol-payload`），
/// **不**按"看起来差不多"猜：**没有跨协议的迁移**，一段跑了一半的活动换协议续跑会被拒，不是
/// 把手里的载荷翻译过去。**公开**的理由与 chat 侧同款：入口拿它当唯一权威的名字。
pub const PROTOCOL: &str = "responses";
/// 本模块当前写出的续接载荷版本。换成不兼容的载荷形状时必须递增。
const PROTOCOL_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// wire 类型：私有，不离开本模块
// ---------------------------------------------------------------------------

/// 请求体。`store` **永远** false：服务端不持任何一份我们的人生。
#[derive(Debug, Serialize)]
struct WireRequest {
    model: String,
    /// 系统指令：Responses 把它放在**独立字段**里，而不是对话的第一条。
    instructions: String,
    /// 整段对话：条目数组（user / function_call / function_call_output / reasoning / message）。
    input: Vec<serde_json::Value>,
    tools: Vec<WireTool>,
    store: bool,
}

/// 一条工具声明：Responses 是**平铺**的，没有 chat 那层 `function` 嵌套。
#[derive(Debug, Serialize)]
struct WireTool {
    #[serde(rename = "type")]
    kind: &'static str,
    name: String,
    description: String,
    parameters: serde_json::Value,
}

/// 只接本次翻译需要的字段。
///
/// `output` 的条目**原样**收（`serde_json::Value`）：reasoning 条目要原样回传，过早类型化就会
/// 丢掉我们没打算解释的字段——而"原样"正是那条铁律的字面意思。
///
/// `status` 只有两个用途：缺席 = 容忍（有些兼容实现不回），出现且不是 `completed` = 这次生成
/// **不是一次正常收尾**（截断/中止），那不是"一次不太完整的决定"，是不能用的。
/// `usage` 的两个去向（刀三 B，与 chat 侧**同一条纪律、同一组中立名**）：屏幕那一格只取
/// `input_tokens`（这一次送进去的上下文），账本那一格取 `input` / `output` / `total`
/// 三个共同字段（经 [`ResponsesModel::with_usage`] 的写柄入代价 cell）。它同样**不参与
/// 任何判断**：字段缺席、类型不对都只意味着"这一轮没有对应的读数"。
#[derive(Debug, Deserialize)]
struct WireResponse {
    #[serde(default)]
    output: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    usage: Option<serde_json::Value>,
}

/// 一次续接载荷的内容。**只有本模块解释它**：Core 与存储层只把它当作不透明的文本搬运。
///
/// 三个字段都是"有就带、没有就不带"：没有可续接的东西时不写载荷，而不是写一个空对象。
#[derive(Debug, Default, Serialize, Deserialize)]
struct Continuation {
    /// 第一次请求的调用标识（配对它的回执要用它）。其余在 `extra_call_ids`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    call_id: Option<String>,
    /// **其余**请求的调用标识，按响应给出的顺序。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    extra_call_ids: Vec<String>,
    /// 上一轮那些**可回传的推理条目**（原样存、原样回传）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    reasoning: Vec<serde_json::Value>,
}

impl Continuation {
    fn is_empty(&self) -> bool {
        self.call_id.is_none() && self.extra_call_ids.is_empty() && self.reasoning.is_empty()
    }

    /// 这条回复请求的**全部**调用标识（第 0 个在前）。
    fn call_ids(&self) -> Vec<String> {
        let mut ids = Vec::with_capacity(self.extra_call_ids.len() + 1);
        if let Some(first) = &self.call_id {
            ids.push(first.clone());
        }
        ids.extend(self.extra_call_ids.iter().cloned());
        ids
    }
}

// ---------------------------------------------------------------------------
// 对外形状：一个实现 Core 窄接口的模型
// ---------------------------------------------------------------------------

/// 一个走 Responses 协议的模型端点。
#[derive(Debug)]
pub struct ResponsesModel {
    client: reqwest::blocking::Client,
    credential: String,
    model: String,
    endpoint: String,
    /// 屏幕报告 cell 的写柄：装上之后本层把每次成功响应里报告的占用读数记进去（唯一写者）。
    reading: Option<ContextReadingWriter>,
    /// **用量** cell 的写柄（代价那一格的量）：与 chat 侧同一个格子类型、同一条纪律。
    usage: Option<UsageWriter>,
    /// **逐轮读数账**的写柄：与 chat 侧同一个格子类型、同一条纪律（一次响应一行，追加）。
    model_readings: Option<ModelReadingsWriter>,
    /// 形态指纹的去向（TASK_60 §6）：与 chat 侧同一个格子类型。
    shape_trace: Option<ShapeTrace>,
    /// **这一次活动的静态头**（TASK_79）：与 chat 侧**同一个接缝、同一份渲染**
    /// （[`crate::provider_adapter::system_message`]），只是本协议把它放在**条目数组的最后一条**
    /// ——系统指令在 `instructions` 里，而机械事实是 `input` 的末条，静态头因此紧贴在它前面。
    ///
    /// `None` = 这一次没有静态头 ⇒ 那一条条目逐字节就是机械事实（今天的样子）。
    static_head: Option<String>,
}

impl ResponsesModel {
    /// 用它做实际调用的模型名。
    pub fn model(&self) -> &str {
        &self.model
    }

    /// **装配注入**：把屏幕报告 cell 的写柄交给本层（不装就是"本层不报"，绝不伪造读数）。
    pub fn with_reading(mut self, reading: ContextReadingWriter) -> Self {
        self.reading = Some(reading);
        self
    }

    /// **装配注入**：把用量 cell 的写柄交给本层（代价那一格的量）。
    ///
    /// 两格**分食不搬家**：屏幕那一格只取 `input_tokens`（"上下文有多满"），这一格取
    /// 三个共同的量（"这一次花了多少"）。不装就是"本层不报"，绝不伪造读数。
    pub fn with_usage(mut self, usage: UsageWriter) -> Self {
        self.usage = Some(usage);
        self
    }

    /// **装配注入**：把逐轮读数账的写柄交给本层（与 chat 侧同一条纪律，落点是文件）。
    pub fn with_model_readings(mut self, readings: ModelReadingsWriter) -> Self {
        self.model_readings = Some(readings);
        self
    }

    /// 装上形态指纹的去向（TASK_60 §6）。
    pub fn with_shape_trace(mut self, trace: ShapeTrace) -> Self {
        self.shape_trace = Some(trace);
        self
    }

    /// 用显式给出的凭据、模型名与端点装配——生产自定义路径与离线端到端验收共用同一个入口。
    ///
    /// **一次构造 = 同次快照**：三元组只在这里落地一次，之后不跨操作重读、不缓存、不回落。
    /// 端点是**完整地址**（本层不替调用方拼路径，与 chat 侧同一口径）。
    pub fn at_endpoint(
        credential: impl Into<String>,
        model: impl Into<String>,
        endpoint: impl Into<String>,
    ) -> Result<Self, ModelError> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(CLIENT_USER_AGENT)
            .timeout(TIMEOUT)
            .build()
            .map_err(|_| ModelError {
                class: ASSEMBLY_FAILURE,
            })?;
        Ok(Self {
            client,
            credential: credential.into(),
            model: model.into(),
            endpoint: endpoint.into(),
            reading: None,
            usage: None,
            model_readings: None,
            shape_trace: None,
            static_head: None,
        })
    }

    /// 内到外：把这一轮的 `View` 渲染成一次请求。
    ///
    /// 条目顺序：非空输入时的输入正文 → **按发生顺序重建的结构化历史** → 机械资源事实。
    /// 系统指令走 `instructions`（**不**混进条目数组）。
    fn request_for(&self, view: &View) -> Result<WireRequest, ProviderFailure> {
        let mut input = Vec::new();
        if !view.input().is_empty() {
            input.push(user_item(input_body(view.input())));
        }
        input.extend(history_items(view)?);
        input.push(user_item(system_message(self.static_head.as_deref(), view)));

        Ok(WireRequest {
            model: self.model.clone(),
            instructions: decision_contract(),
            input,
            tools: tools_for(view.capabilities())?,
            store: false,
        })
    }

    /// 传输：一次请求，一次响应。失败按 `ProviderFailure` 分类，不重试、不流式、不并发。
    /// 传输：一次请求，一次响应。失败按 `ProviderFailure` 分类，不重试、不流式、不并发。
    ///
    /// `session` 是这一段活动的 id（同一个头、同一个值，两条 wire 一致——见
    /// [`crate::provider_adapter::SESSION_HEADER`] 的实测记录：那一家的两条 wire 都要求它，
    /// 空值也算没带）。它**不是**凭据，与正文同级。
    fn send(&self, request: &WireRequest, session: &str) -> Result<WireResponse, ProviderFailure> {
        let response = self
            .client
            .post(&self.endpoint)
            .header("Authorization", format!("Bearer {}", self.credential))
            .header("Content-Type", "application/json")
            .header(crate::provider_adapter::SESSION_HEADER, session)
            .json(request)
            .send()
            .map_err(|error| {
                crate::provider_adapter::transport_failure(
                    "send",
                    &error,
                    self.shape_trace.as_ref(),
                )
            })?;
        let status = response.status().as_u16();
        let body = response.text().map_err(|error| {
            crate::provider_adapter::transport_failure("read", &error, self.shape_trace.as_ref())
        })?;
        classify_response(status, &body)
    }
}

impl ModelPort for ResponsesModel {
    /// **装订这一次活动的静态头**（TASK_79）：与 chat 侧同一件事（本层同样不解释它）。
    fn declare_static_head(&mut self, head: Option<&str>) {
        self.static_head = head.map(str::to_string);
    }

    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        // 与 chat 侧同一条纪律：失败在这里被分类，然后**作为事实**交给 Core；屏幕读数只在
        // "翻译成功之后"写一次（失败不改动 cell ⇒ "最近一次"这个语义不需要额外的失效机制）。
        let translated = self
            .request_for(view)
            .and_then(|request| self.send(&request, view.run().as_str()))
            .and_then(|response| {
                let reported = reported_context_usage(&response);
                let usage = reported_usage(&response);
                let cached = reported_cached_prompt_tokens(&response);
                Ok((translate_response(response)?, reported, usage, cached))
            });

        match translated {
            Ok((output, reported, usage, cached)) => {
                if let (Some(writer), Some(reported)) = (self.reading.as_ref(), reported) {
                    writer.report(view.run(), reported);
                }
                // 用量：**整格都没有就不写**（三个字段一个都没报 ⇒ 这一轮什么都没说）。
                if let (Some(writer), Some(usage)) = (self.usage.as_ref(), usage)
                    && !usage.is_empty()
                {
                    writer.report(usage);
                }
                // 逐轮读数账：与 chat 侧同一条判据——**有账就记一行**，计数缺着也记
                // （"这一轮问过一次模型"本身就是要看的事实）。
                if let Some(writer) = self.model_readings.as_ref() {
                    writer.record(&TurnReadings {
                        run: view.run().clone(),
                        turn: view.turn(),
                        wire: PROTOCOL,
                        prompt_tokens: usage.and_then(|each| each.prompt_tokens),
                        completion_tokens: usage.and_then(|each| each.completion_tokens),
                        total_tokens: usage.and_then(|each| each.total_tokens),
                        cached_prompt_tokens: cached,
                    });
                }
                Ok(output)
            }
            Err(failure) => Err(ModelError {
                class: failure.class(),
            }),
        }
    }
}

/// 这一次响应报告的**上下文占用**：Responses 的 usage 是**平铺**的，这里取 `input_tokens`。
///
/// 与 DeepSeek 那条 wire 取 `prompt_tokens` 是**同一个口径**：送进去的那段上下文，不含本次生成的
/// 补全。两条 wire 那一格的含义必须一样，否则同一个格位在两条通道上说两件事。
///
/// 它是**遥测不是契约**：任一项缺席或类型不对就返回 `None`（调用方保持旧读数），
/// **绝不**因此把一次本来正常的响应判成失败，也不进 Core 的九类失败分类。
fn reported_context_usage(response: &WireResponse) -> Option<u64> {
    response.usage.as_ref()?.get("input_tokens")?.as_u64()
}

/// 这一次响应报告的**用量**（厂商原文的搬运，不做任何换算）。
///
/// Responses 的 usage 是**平铺**的，字段名与 chat 侧不同：`input_tokens` / `output_tokens` /
/// `total_tokens`。本层把它**映射到同一组中立名**（[`Usage`] 那三个字段），因此代价那一格在
/// 两条 wire 上说同一件事——否则同一个格位会因走哪条 wire 而有两种口径。
///
/// 与 [`reported_context_usage`] 是**两格分食**：那一格只取送进去的量（"上下文有多满"），
/// 这一格取三个共同的量（"这一次花了多少"）。
///
/// 同样是**遥测不是契约**：字段缺席或类型不对一律如实缺着（`None`），绝不因此判一次正常响应失败。
fn reported_usage(response: &WireResponse) -> Option<Usage> {
    let usage = response.usage.as_ref()?;
    Some(Usage {
        prompt_tokens: usage
            .get("input_tokens")
            .and_then(serde_json::Value::as_u64),
        completion_tokens: usage
            .get("output_tokens")
            .and_then(serde_json::Value::as_u64),
        total_tokens: usage
            .get("total_tokens")
            .and_then(serde_json::Value::as_u64),
    })
}

/// 这一次响应报告的**命中缓存的那部分上下文量**（厂商原文的搬运，不做任何换算）。
///
/// 与 chat 侧**同一个口径**（送进去的那一段里有多少是命中缓存服务的），只是这一条协议把它
/// **嵌在明细里**而不是平铺：`usage.input_tokens_details.cached_tokens`。字段名按供应商官方
/// 文档取——两条协议对同一个口径用两个名字、两种形状，映射到同一个中立名的这一步**只在本层**。
///
/// ⚠ 与另外两格同一条纪律：**遥测不是契约**——字段缺席、类型不对、结构变化一律如实缺着
/// （`None`），**绝不**因此把一次本来正常的响应判成失败、也不进 Core 的九类失败分类。
fn reported_cached_prompt_tokens(response: &WireResponse) -> Option<u64> {
    response
        .usage
        .as_ref()?
        .get("input_tokens_details")?
        .get("cached_tokens")?
        .as_u64()
}

// ---------------------------------------------------------------------------
// 条目与工具声明（纯函数，可离线测试）
// ---------------------------------------------------------------------------

/// 一条用户正文条目（输入正文、注入正文、机械事实都走它）。
fn user_item(text: String) -> serde_json::Value {
    serde_json::json!({"role": "user", "content": text})
}

/// 一条助手正文条目。**只在正文非空时产生**：空正文的助手条目没有它要表达的东西，
/// 而"它说过什么"这件事由正文承载（有工具调用的轮次由 `function_call` 条目承载）。
fn message_item(text: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "message",
        "role": "assistant",
        "content": [{"type": "output_text", "text": text}],
    })
}

/// 一次工具调用条目。
fn function_call_item(call_id: &str, name: &str, arguments: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "function_call",
        "call_id": call_id,
        "name": name,
        "arguments": arguments,
    })
}

/// 一条工具回执条目（`output` 是它的结果正文）。
fn function_call_output_item(call_id: String, output: String) -> serde_json::Value {
    serde_json::json!({"type": "function_call_output", "call_id": call_id, "output": output})
}

/// 一条**带着材料**的工具回执条目。
///
/// 没有材料时**逐字节等于** [`function_call_output_item`]（`output` 仍是一个字符串）；
/// 有材料时 `output` 变成部件数组：**文本在前、材料在后**——形状照
/// `ResponseFunctionCallOutputItemParam = Union[Text, Image, File]`，两件的 JSON 标签是
/// `input_text` / `input_image`，而 `input_image.image_url` 的文档原话就是
/// "base64 encoded image in a data URL"（与另一条 wire 同一份字节、同一个形状）。
fn function_call_output_item_with(
    call_id: String,
    output: String,
    materials: Vec<&Material>,
) -> serde_json::Value {
    if materials.is_empty() {
        return function_call_output_item(call_id, output);
    }
    let mut parts = vec![serde_json::json!({"type": "input_text", "text": output})];
    for material in materials {
        parts.push(serde_json::json!({
            "type": "input_image",
            "image_url": data_url(material),
        }));
    }
    serde_json::json!({"type": "function_call_output", "call_id": call_id, "output": parts})
}

/// 生成**平铺**的 `tools` 声明：**永远恰好三件**——[`INVOKE_TOOL`] / [`FINISH_TOOL`] /
/// [`HOLD_TOOL`]（TASK_68 裁决 1；第三件是 2026-10-10 裁决三加的），与 chat 侧同一个头部。
///
/// 与另一条 wire **同源同形**：`invoke` 的说明与参数形状都从 `provider_adapter` 借，
/// 所以两条协议给模型的**说明书逐字相同**，差别只在声明本身的形状（嵌套 vs 平铺）。
/// **它不再渲染能力面**：头部恒定，"展开"改的是注入的目录（TASK_68 P2）；
/// 但标识仍必须**能作为工具名表达**（主体经 `invoke` 用到的就是这个名字）⇒ 校验留在这里。
fn tools_for(capabilities: &Capabilities) -> Result<Vec<WireTool>, ProviderFailure> {
    for capability in capabilities.items() {
        tool_name(capability.id())?;
    }
    Ok(vec![
        WireTool {
            kind: "function",
            name: INVOKE_TOOL.to_string(),
            description: INVOKE_DESCRIPTION.to_string(),
            parameters: invoke_schema(),
        },
        WireTool {
            kind: "function",
            name: FINISH_TOOL.to_string(),
            description: FINISH_DESCRIPTION.to_string(),
            parameters: finish_schema(),
        },
        // 2026-10-10（裁决三）：**搁下**那一件与收场并排，两条 wire 同源同形。
        WireTool {
            kind: "function",
            name: HOLD_TOOL.to_string(),
            description: HOLD_DESCRIPTION.to_string(),
            parameters: finish_schema(),
        },
    ])
}

// ---------------------------------------------------------------------------
// 历史重建：`function_call` ↔ `function_call_output` 相邻配对
// ---------------------------------------------------------------------------

/// 一条**等着回执**的请求：那次调用的标识与当时请求的行动（合成回执要用行动标识）。
#[derive(Clone, Debug)]
struct AwaitingReceipt {
    call_id: String,
    action: ActionId,
}

/// 把这一轮的轨迹**结构化**重建为 `input` 条目（与 chat 侧同一套纪律、另一种形状）。
///
/// 配对规则是**结构性**的，与 chat 侧逐条对应：见本文件头部那段。三个冲刷边界也一样——
/// 非空注入批之前、下一条模型响应之前、轨迹遍历结束。
///
/// ⚠ **如实登记（待人工归属）**：重建时**仍逐个校验名字**（下面那一处 `tool_name(…)?`）⇒
/// 一次"名字不能用"的拒绝**回放不出来**：请求重建当场失败，整段仍以 `reserved-tool-name` /
/// `unsupported-tool-name` 收场，那次拒绝白做。另一条 wire 已经改成**一律原样引用**（那次调用
/// 从未派发、准入会拒它，紧跟着它的就是我们的拒绝读数 ⇒ 回放它对配对是必需的，而且无损），
/// 这条还没跟上；名字的合法性本来就归**声明侧**，不归回放侧。
fn history_items(view: &View) -> Result<Vec<serde_json::Value>, ProviderFailure> {
    let mut items = Vec::new();
    let mut pending: VecDeque<AwaitingReceipt> = VecDeque::new();

    for entry in view.trace() {
        match entry {
            TraceEntry::Injection { stage, turn, batch } => {
                if batch.is_empty() {
                    continue;
                }
                flush_awaiting(&mut pending, &mut items);
                items.push(user_item(injection_body(*stage, *turn, batch)));
            }
            TraceEntry::ModelResponse(response) => {
                flush_awaiting(&mut pending, &mut items);
                let continuation = continuation_of(response)?;
                // 推理条目**原样**摆在这一轮 `function_call` 之前（服务端本来就在前面）。
                if let Some(continuation) = &continuation {
                    items.extend(continuation.reasoning.iter().cloned());
                }
                let requested = response.decision().actions();
                if !requested.is_empty() {
                    let ids = continuation
                        .as_ref()
                        .map(Continuation::call_ids)
                        .unwrap_or_default();
                    if ids.len() != requested.len() {
                        return Err(ProviderFailure::ProtocolPayload);
                    }
                    for (action, call_id) in requested.iter().zip(ids) {
                        items.push(function_call_item(
                            &call_id,
                            tool_name(action.id())?,
                            &arguments_of(action.params()),
                        ));
                        pending.push_back(AwaitingReceipt {
                            call_id,
                            action: action.id().clone(),
                        });
                    }
                }
                if !response.content().is_empty() {
                    items.push(message_item(response.content()));
                }
            }
            TraceEntry::Refused {
                action,
                class,
                content,
                ..
            } => {
                let awaiting = pending
                    .pop_front()
                    .ok_or(ProviderFailure::UnpairedHistory)?;
                items.push(function_call_output_item(
                    awaiting.call_id,
                    refusal_result(action, class, content),
                ));
            }
            TraceEntry::Observation(observation) => {
                let awaiting = pending
                    .pop_front()
                    .ok_or(ProviderFailure::UnpairedHistory)?;
                // **材料在这里出门**：只进这一次请求，不进账本、不进经历。
                items.push(function_call_output_item_with(
                    awaiting.call_id,
                    observation_result(observation),
                    observation_materials(observation),
                ));
            }
        }
    }

    flush_awaiting(&mut pending, &mut items);
    Ok(items)
}

/// 把剩下的待回执调用**冲刷**成合成回执，按请求顺序逐条追加（队列空时什么都不做）。
fn flush_awaiting(pending: &mut VecDeque<AwaitingReceipt>, items: &mut Vec<serde_json::Value>) {
    for awaiting in pending.drain(..) {
        items.push(function_call_output_item(
            awaiting.call_id,
            unconfirmed_result(&awaiting.action),
        ));
    }
}

// ---------------------------------------------------------------------------
// 外到内：响应 → 中立响应
// ---------------------------------------------------------------------------

/// 状态码 + 响应体 → wire 响应或私有失败分类（与 chat 侧同一套判断；**折叠规则也共用那一份**）。
fn classify_response(status: u16, body: &str) -> Result<WireResponse, ProviderFailure> {
    if !(200..300).contains(&status) {
        return Err(ProviderFailure::HttpStatus(http_detail(status)));
    }
    serde_json::from_str::<WireResponse>(body).map_err(|_| ProviderFailure::Unparsable)
}

/// 一条 reasoning 条目里**可回传**的那一份（带 `encrypted_content` 才算）。
///
/// 没有加密内容的推理条目是"这一轮没有可回传的推理链"——**不**伪造一条空的回传：
/// 那会让服务端收到一条它看不懂的条目，而"我们没读到"与"我们编了一条"是两件事。
fn reasoning_payload(item: &serde_json::Value) -> Option<serde_json::Value> {
    let encrypted = item.get("encrypted_content")?.as_str()?;
    if encrypted.is_empty() {
        return None;
    }
    Some(item.clone())
}

/// 一条 `message` 条目里的正文。
///
/// 只认两种内容块：`output_text`（正常正文）与 `refusal`（模型明确不肯答）——两者都是
/// "它说了什么"，因此都进正文。**别的块类型一律明确失败**：我们只声明标准 function 工具，
/// 出现别的块说明这次响应不是我们那份请求的答复，宽松解析在这里不是兼容，是失明。
fn message_text(item: &serde_json::Value) -> Result<String, ProviderFailure> {
    let parts = item
        .get("content")
        .and_then(|value| value.as_array())
        .ok_or(ProviderFailure::ContractViolation)?;
    let mut text = String::new();
    for part in parts {
        let kind = part
            .get("type")
            .and_then(|value| value.as_str())
            .ok_or(ProviderFailure::ContractViolation)?;
        if kind != "output_text" && kind != "refusal" {
            return Err(ProviderFailure::ContractViolation);
        }
        text.push_str(
            part.get("text")
                .and_then(|value| value.as_str())
                .ok_or(ProviderFailure::ContractViolation)?,
        );
    }
    Ok(text)
}

/// 供应商响应 → Core 的中立响应。
///
/// 只认**三种条目**（`reasoning` / `function_call` / `message`）：别的 `type` 说明这次响应
/// **不是我们那份请求的答复**（我们只声明标准 function 工具）——与内容块同一条判据，
/// 理由写在 [`message_text`] 上。
///
/// 形状规则全部是**明确失败**，没有一条"就近猜一个"的退路：缺 `output`、`status` 出现但
/// 不是 `completed`、`function_call` 条目缺 `call_id`、一次回复里结束与普通行动并存——都
/// 各自拒绝，理由与 chat 侧逐条对应。
fn translate_response(response: WireResponse) -> Result<ModelOutput, ProviderFailure> {
    if let Some(status) = response.status.as_deref()
        && status != "completed"
    {
        return Err(ProviderFailure::StoppedGeneration);
    }
    let output = response.output.ok_or(ProviderFailure::Unparsable)?;

    let mut text = String::new();
    let mut calls: Vec<(String, String, String)> = Vec::new();
    let mut reasoning: Vec<serde_json::Value> = Vec::new();

    for item in &output {
        let kind = item
            .get("type")
            .and_then(|value| value.as_str())
            .ok_or(ProviderFailure::ContractViolation)?;
        match kind {
            "reasoning" => {
                if let Some(payload) = reasoning_payload(item) {
                    reasoning.push(payload);
                }
            }
            "function_call" => {
                let call_id = item
                    .get("call_id")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                if call_id.is_empty() {
                    // 没有调用标识就无法配对它的回执；这不是"可以不配对"，是载荷缺失。
                    return Err(ProviderFailure::ProtocolPayload);
                }
                let name = item
                    .get("name")
                    .and_then(|value| value.as_str())
                    .ok_or(ProviderFailure::ContractViolation)?
                    .to_string();
                let arguments = item
                    .get("arguments")
                    .and_then(|value| value.as_str())
                    .ok_or(ProviderFailure::ContractViolation)?
                    .to_string();
                calls.push((call_id, name, arguments));
            }
            "message" => text.push_str(&message_text(item)?),
            _ => return Err(ProviderFailure::ContractViolation),
        }
    }

    let mut continuation = Continuation {
        reasoning,
        ..Continuation::default()
    };

    let decision = if calls.is_empty() {
        if text.trim().is_empty() {
            return Err(ProviderFailure::ContractViolation);
        }
        Decision::Yield
    } else {
        // 两个**结束类**动词（收场 / 搁下）与"请求行动"三者互斥：一次回复只有一个意图。
        //
        // ⚠ **2026-10-11 按真机补齐**（这一格原先**只认收场**，漏了搁下）：漏掉的代价是具体的——
        // `HOLD_TOOL` 在 [`tools_for`] 里**照样声明**（模型看得到、可以调），可这里没有它的分支
        // ⇒ 那次调用掉进下面"以动作名直调"那一条，被 [`tool_name`] 按**保留名**拒绝
        // （`hold_run` 是保留名，正因为它该走这里而不是那条）⇒ 主体一选"搁下"，**整段 Run
        // 当场打成 `system:model-failed`**。实测（opencode go 的 zen 网关、grok-4.6）：
        // 两段都在第 1 轮 `reserved-tool-name` 收场，而读数账里**一行都没有**（一次成功的响应
        // 都没留下）。另一条 wire 一直是对的（它的解码本来就 .any(收场 ∨ 搁下)）——
        // 这条 wire 从未在真机上跑过，所以这个不对称活了很久。
        let finishing = calls
            .iter()
            .any(|(_, name, _)| name == FINISH_TOOL || name == HOLD_TOOL);
        if finishing {
            if calls.len() > 1 {
                return Err(ProviderFailure::ContractViolation);
            }
            Decision::Finish(if calls[0].1 == HOLD_TOOL {
                hold_ending(&calls[0].2)?
            } else {
                finish_ending(&calls[0].2)?
            })
        } else {
            let mut actions = Vec::with_capacity(calls.len());
            let mut call_ids = Vec::with_capacity(calls.len());
            for (call_id, name, arguments) in calls {
                // ⚠ **两条入口一个口径**（2026-10-13 按真机补）：
                // ① 名字不对是**那一次调用**的事（与 Chat wire 同一条判据，见 `tool_name_reading`）；
                // ② `INVOKE_TOOL` 在头部里**照样声明**（模型看得到、可以调），这条 wire 从前
                //    没有它的分支 ⇒ 那次调用掉进"以动作名直调"那一条、被**保留名**拒绝 ⇒
                //    整段 Run 当场 `model-failed`。这与 2026-10-11 修掉的 `hold_run` 是**同一类
                //    缺陷、差一个工具**（那条 wire 从未在真机上跑过，所以不对称活了很久）。
                let action = match tool_name(&ActionId::new(name.clone())) {
                    Err(_) => {
                        Action::malformed(ActionId::new(name.clone()), tool_name_reading(&name))
                    }
                    // ⚠ **如实登记（待人工归属）**：这一支今天**不可达**——`tool_name` 把三件
                    // 元工具名（`invoke` 自己也在内）一律当**保留名**拒，而前一条 `Err(_)` 先接住了
                    // 它 ⇒ 在这条 wire 上，模型调 `invoke` 会拿到「`invoke` 是保留名，不能以动作名
                    // 直调」那句读数（读数本身没错，但这一次它说的是**入口自己**）。错的是**判序**
                    // （外层该先认出 `invoke`，名字规则只管直调那条路），**不是**判据本身。
                    Ok(_) if name == INVOKE_TOOL => match decode_invoke(&arguments) {
                        Ok((inner, params)) => Action::with_params(ActionId::new(inner), params),
                        Err(InvokeFailure::Fatal(failure)) => return Err(failure),
                        Err(InvokeFailure::Shape { action, shape }) => Action::malformed(
                            ActionId::new(action.clone()),
                            shape.shape_reading(&action),
                        ),
                        // 内层**名字**不能用：同样是那一次调用的事（与 Chat wire 逐字同款）。
                        // 被拒的是**内层那个名字**（账里只存动作、不存信封），读数由 `tool_name_reading` 给。
                        // ⚠ 回放那一半**没有**跟着改：这里拒绝了它，`history_items` 下一轮仍会把
                        // 这个名字拿去过一遍合法性 ⇒ 拒绝回放不出来（见那个函数的文档）。
                        Err(InvokeFailure::Name { action }) => Action::malformed(
                            ActionId::new(action.clone()),
                            tool_name_reading(&action),
                        ),
                    },
                    // 与 `invoke` 包裹**同一条口径**：搬不动的参数是那一次调用的事。
                    Ok(_) => match decode_arguments(&arguments) {
                        Ok(params) => Action::with_params(ActionId::new(name.clone()), params),
                        Err(shape) => Action::malformed(
                            ActionId::new(name.clone()),
                            shape.shape_reading(&name),
                        ),
                    },
                };
                call_ids.push(call_id);
                actions.push(action);
            }
            let mut ids = call_ids.into_iter();
            continuation.call_id = ids.next();
            continuation.extra_call_ids = ids.collect();
            Decision::act(actions).ok_or(ProviderFailure::ContractViolation)?
        }
    };

    Ok(ModelOutput::new(
        text,
        protocol_data(&continuation)?,
        decision,
    ))
}

/// 一次续接内容 → 中立载荷（空内容**不**写载荷；载荷太大由 `ProtocolData::new` 明确拒绝）。
fn protocol_data(continuation: &Continuation) -> Result<Option<ProtocolData>, ProviderFailure> {
    if continuation.is_empty() {
        return Ok(None);
    }
    let payload =
        serde_json::to_string(continuation).map_err(|_| ProviderFailure::ProtocolPayload)?;
    ProtocolData::new(PROTOCOL, PROTOCOL_VERSION, payload)
        .map(Some)
        .map_err(|_| ProviderFailure::ProtocolPayload)
}

/// 一次响应里保存的续接载荷 → 本模块自己的续接内容。
///
/// 缺失是**合法**的；存在但读不懂（协议名不符——例如盘上那条是 chat 侧写的、版本不认识、
/// 载荷损坏）一律明确失败：这一条让"换一个新的 Adapter 实例照样能重建请求"成立。
fn continuation_of(response: &ModelResponse) -> Result<Option<Continuation>, ProviderFailure> {
    let Some(data) = response.protocol() else {
        return Ok(None);
    };
    if data.protocol() != PROTOCOL || data.version() != PROTOCOL_VERSION {
        return Err(ProviderFailure::ProtocolPayload);
    }
    serde_json::from_str::<Continuation>(data.payload())
        .map(Some)
        .map_err(|_| ProviderFailure::ProtocolPayload)
}

// ---------------------------------------------------------------------------
// 离线测试：不使用凭据、不访问网络
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Decision, ModelResponse};
    use crate::foundation::{SubjectEnding, Turn};

    /// 一条 wire 响应（按 JSON 拼，贴近真实报文的形状）。
    fn response(body: &str) -> WireResponse {
        serde_json::from_str(body).unwrap_or_else(|error| panic!("夹具报文应当可解析: {error}"))
    }

    fn translated(body: &str) -> Result<ModelOutput, ProviderFailure> {
        translate_response(response(body))
    }

    /// 一条带工具调用的响应。
    fn call(name: &str, arguments: &str, call_id: &str) -> String {
        serde_json::json!({
            "status": "completed",
            "output": [{
                "type": "function_call",
                "call_id": call_id,
                "name": name,
                "arguments": arguments,
            }],
        })
        .to_string()
    }

    #[test]
    fn a_reported_occupancy_is_the_input_side_of_the_flat_fields() {
        // 送进去的那段上下文 = `input_tokens`；`output_tokens` 是这一轮的产物，不计入（与 chat 侧取
        // `prompt_tokens` 同一口径）。
        let with_usage = response(
            "{\"status\":\"completed\",\"output\":[],\
             \"usage\":{\"input_tokens\":5,\"output_tokens\":7}}",
        );
        assert_eq!(reported_context_usage(&with_usage), Some(5));

        // 只有输出侧报得出 ⇒ 仍然没有"上下文占用"读数（不拿补全冒充满度）。
        let output_only =
            response("{\"status\":\"completed\",\"output\":[],\"usage\":{\"output_tokens\":7}}");
        assert_eq!(reported_context_usage(&output_only), None);
    }

    /// 遥测不是契约：字段缺席、类型不对、结构变化**都不是失败**，只是"这一轮没有读数"。
    #[test]
    fn a_malformed_or_missing_usage_is_never_a_failure() {
        for body in [
            "{\"status\":\"completed\",\"output\":[]}",
            "{\"status\":\"completed\",\"output\":[],\"usage\":{}}",
            "{\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":\"5\"}}",
            "{\"status\":\"completed\",\"output\":[],\"usage\":null}",
        ] {
            assert_eq!(reported_context_usage(&response(body)), None, "{body}");
        }
    }

    /// **两条 wire 同一份字节、同一个形状**：材料折成 `input_image`，而且**文本部件在前**。
    ///
    /// 没有材料时它必须**逐字节等于**那条不带材料的条目——这是"这一步不改变任何可观察行为"
    /// 的判据（另一条 wire 的同款判据在 `provider_adapter::tests`）。
    #[test]
    fn a_receipt_with_material_becomes_input_text_plus_input_image() {
        let plain = function_call_output_item("call-1".to_string(), "读数".to_string());
        assert!(plain["output"].is_string(), "没有材料时 output 是字符串");

        assert_eq!(
            function_call_output_item_with("call-1".to_string(), "读数".to_string(), Vec::new()),
            plain,
            "空材料不得改变条目形状"
        );

        let material = Material::new("image/png", vec![0x89, 0x50, 0x4e, 0x47]);
        let with_material = function_call_output_item_with(
            "call-1".to_string(),
            "读数".to_string(),
            vec![&material],
        );
        let parts = with_material["output"]
            .as_array()
            .expect("有材料时 output 是部件数组");
        assert_eq!(parts.len(), 2, "一个文本部件 ＋ 一个材料部件");
        assert_eq!(parts[0]["type"], "input_text");
        assert_eq!(parts[0]["text"], "读数", "读数在前");
        assert_eq!(parts[1]["type"], "input_image");
        assert_eq!(parts[1]["image_url"], "data:image/png;base64,iVBORw==");
    }

    /// `status` 出现但不是 `completed`：这次生成**不是**一次正常收尾（截断/中止），不能用。
    #[test]
    fn a_non_completed_status_is_never_a_normal_response() {
        assert_eq!(
            translated("{\"status\":\"incomplete\",\"output\":[]}"),
            Err(ProviderFailure::StoppedGeneration)
        );
        // 缺席是容忍的（有些兼容实现不回），因此它走的是"没有条目"那条判断。
        assert_eq!(
            translated("{\"output\":[]}"),
            Err(ProviderFailure::ContractViolation)
        );
    }

    #[test]
    fn a_response_without_output_is_unparsable() {
        assert_eq!(
            translated("{\"status\":\"completed\"}"),
            Err(ProviderFailure::Unparsable)
        );
        // 非 2xx 与结构不符都由这一条分类拦在外面（与 chat 侧同一套；**折叠规则也共用那一份**）。
        assert_eq!(
            classify_response(500, "{}").err(),
            Some(ProviderFailure::HttpStatus(Some(
                crate::core::HttpStatusDetail::ServerSide
            )))
        );
        assert_eq!(
            classify_response(200, "不是 JSON").err(),
            Some(ProviderFailure::Unparsable)
        );
    }

    #[test]
    fn plain_text_is_a_yield_and_an_empty_response_is_a_failure() {
        let text = r#"{"status":"completed","output":[{"type":"message","role":"assistant",
            "content":[{"type":"output_text","text":"说一句"}]}]}"#;
        let output = translated(text).expect("可翻译");
        assert_eq!(output.content(), "说一句");
        assert!(matches!(output.decision(), Decision::Yield));
        assert!(output.protocol().is_none(), "没有可续接的东西就不写载荷");

        // 空响应既没说做什么、也没说结束 —— 明确失败，绝不冒充"正常等待"。
        assert_eq!(
            translated("{\"status\":\"completed\",\"output\":[]}"),
            Err(ProviderFailure::ContractViolation)
        );
        assert_eq!(
            translated(
                "{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\
                 \"content\":[{\"type\":\"output_text\",\"text\":\"   \"}]}]}"
            ),
            Err(ProviderFailure::ContractViolation)
        );
    }

    /// 只认三种条目与两种内容块：别的形状**明确失败**（我们只声明标准 function 工具，
    /// 出现别的东西说明这次响应不是我们那份请求的答复）。
    #[test]
    fn an_unknown_item_or_a_bad_message_part_is_a_contract_violation() {
        for body in [
            r#"{"status":"completed","output":[{"type":"web_search_call"}]}"#,
            r#"{"status":"completed","output":[{"role":"assistant"}]}"#,
            r#"{"status":"completed","output":[{"type":"message","role":"assistant","content":"纯文本"}]}"#,
            r#"{"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"input_image"}]}]}"#,
            r#"{"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text"}]}]}"#,
            r#"{"status":"completed","output":[{"type":"function_call","call_id":"c1","name":"notes_list"}]}"#,
        ] {
            assert_eq!(
                translated(body),
                Err(ProviderFailure::ContractViolation),
                "{body}"
            );
        }
        // 模型的**拒绝**（refusal）是"它说了什么"，因此照旧进正文。
        let refused = translated(
            r#"{"status":"completed","output":[{"type":"message","role":"assistant",
                "content":[{"type":"refusal","text":"我不做这件事"}]}]}"#,
        )
        .expect("可翻译");
        assert_eq!(refused.content(), "我不做这件事");
    }

    #[test]
    fn calls_become_actions_and_finish_run_ends_the_activity() {
        let output = translated(&call("notes_list", "{}", "call-7")).expect("可翻译");
        let actions = output.decision().actions();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].id().as_str(), "notes_list");
        let data = output.protocol().expect("有续接载荷");
        assert_eq!(data.protocol(), PROTOCOL);
        assert_eq!(data.version(), PROTOCOL_VERSION);
        assert!(data.payload().contains("call-7"), "载荷里带着配对标识");

        let finished = translated(&call("finish_run", "{}", "call-8")).expect("可翻译");
        assert!(
            matches!(finished.decision(), Decision::Finish(SubjectEnding::Ended)),
            "结束工具不带参数（2026-10-10 裁决）：空对象就是收场"
        );

        // **结束与普通行动并存**：两份互相矛盾的意图，整体拒绝、一个都不执行。
        let both = serde_json::json!({
            "status": "completed",
            "output": [
                {"type": "function_call", "call_id": "c1", "name": "notes_list", "arguments": "{}"},
                {"type": "function_call", "call_id": "c2", "name": "finish_run",
                 "arguments": "{}"},
            ],
        })
        .to_string();
        assert_eq!(translated(&both), Err(ProviderFailure::ContractViolation));
    }

    /// **搁下**那一件也必须解得出（2026-10-11 按真机补）。
    ///
    /// 这条守的是一个**真机抓到的缺陷**：头部里一直声明着 `hold_run`（模型看得到、可以调），
    /// 可解码处只认收场 ⇒ 那次调用掉进"以动作名直调"那一条，被保留名拒绝
    /// （`reserved-tool-name`）⇒ **主体一选"搁下"，整段 Run 当场 `model-failed`**。
    /// 判据三条：搁下 ⇒ `Held`（**不是** `Ended`，两者是两件事）；它与普通行动并存 ⇒ 整体拒绝；
    /// 而且它**不是**保留名错误（那正是从前的样子，所以这一条是回归的）。
    #[test]
    fn hold_run_is_decoded_as_a_hold_and_never_as_a_reserved_name() {
        let held = translated(&call("hold_run", "{}", "call-9")).expect("可翻译");
        assert!(
            matches!(held.decision(), Decision::Finish(SubjectEnding::Held)),
            "搁下 ≠ 收场：它带的是 Held"
        );

        let both = serde_json::json!({
            "status": "completed",
            "output": [
                {"type": "function_call", "call_id": "c1", "name": "notes_list", "arguments": "{}"},
                {"type": "function_call", "call_id": "c2", "name": "hold_run",
                 "arguments": "{}"},
            ],
        })
        .to_string();
        assert_eq!(translated(&both), Err(ProviderFailure::ContractViolation));
    }

    #[test]
    fn a_call_without_an_identifier_is_a_payload_problem() {
        let body = serde_json::json!({
            "status": "completed",
            "output": [{"type": "function_call", "name": "notes_list", "arguments": "{}"}],
        })
        .to_string();
        assert_eq!(translated(&body), Err(ProviderFailure::ProtocolPayload));
    }

    /// 只有**带加密内容**的推理条目才进续接载荷：没有加密内容就没有可回传的链，不伪造一条空的。
    #[test]
    fn only_encrypted_reasoning_is_stored_and_replayed() {
        let with = serde_json::json!({
            "status": "completed",
            "output": [
                {"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "opaque"},
                {"type": "function_call", "call_id": "c1", "name": "notes_list", "arguments": "{}"},
            ],
        })
        .to_string();
        let data = translated(&with)
            .expect("可翻译")
            .protocol()
            .expect("有续接载荷")
            .payload()
            .to_string();
        assert!(data.contains("opaque"), "原样存下来: {data}");
        let stored: Continuation = serde_json::from_str(&data).expect("载荷可解析");
        assert_eq!(stored.reasoning.len(), 1);
        assert_eq!(stored.reasoning[0]["id"], serde_json::json!("rs_1"));
        assert_eq!(stored.call_id.as_deref(), Some("c1"));

        let without = serde_json::json!({
            "status": "completed",
            "output": [
                {"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "想了"}]},
                {"type": "function_call", "call_id": "c1", "name": "notes_list", "arguments": "{}"},
            ],
        })
        .to_string();
        let stored: Continuation = serde_json::from_str(
            translated(&without)
                .expect("可翻译")
                .protocol()
                .expect("有续接载荷")
                .payload(),
        )
        .expect("载荷可解析");
        assert!(stored.reasoning.is_empty(), "没有加密内容就不存");
    }

    /// 盘上那条续接载荷若是**别的协议**写的：明确失败，不按"看起来差不多"猜。
    #[test]
    fn a_continuation_written_by_the_other_protocol_is_refused() {
        let mine = Continuation {
            call_id: Some("c1".to_string()),
            extra_call_ids: Vec::new(),
            reasoning: Vec::new(),
        };
        let data = protocol_data(&mine).expect("可构造载荷").expect("有载荷");
        let response = ModelResponse::new(
            Turn::FIRST,
            ModelOutput::new(String::new(), Some(data), Decision::Yield),
        );
        assert!(
            continuation_of(&response)
                .expect("自己的载荷读得懂")
                .is_some()
        );

        // 同一个 JSON 载荷，但协议名写成 chat 侧那一个。
        let foreign = ProtocolData::new(
            crate::provider_adapter::PROTOCOL,
            PROTOCOL_VERSION,
            serde_json::to_string(&mine).expect("可序列化"),
        )
        .expect("可构造载荷");
        let response = ModelResponse::new(
            Turn::FIRST,
            ModelOutput::new(String::new(), Some(foreign), Decision::Yield),
        );
        assert_eq!(
            continuation_of(&response).err(),
            Some(ProviderFailure::ProtocolPayload)
        );
    }

    /// 工具声明是**平铺**的（没有 chat 那层 `function` 嵌套），头部**恰好三件**
    /// （入口 ＋ 收场 ＋ 搁下；TASK_68 裁决 1，第三件是 2026-10-10 裁决三加的）。
    #[test]
    fn the_tools_are_declared_flat_with_the_finish_tool_always_present() {
        let tools = tools_for(&Capabilities::none()).expect("可以声明");
        assert_eq!(tools.len(), 3, "头部恒定三件：入口 ＋ 收场 ＋ 搁下");
        let wire: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&tools).expect("可序列化"))
                .expect("合法 JSON");
        assert_eq!(wire[0]["type"], serde_json::json!("function"));
        assert_eq!(wire[0]["name"], serde_json::json!(INVOKE_TOOL));
        assert!(
            wire[0].get("function").is_none(),
            "平铺：名字与说明就在这一层"
        );
        assert_eq!(wire[1]["name"], serde_json::json!(FINISH_TOOL));
        // **不带参数**（2026-10-10 裁决）：形状封闭，`properties` 与 `required` 都空。
        assert_eq!(
            wire[1]["parameters"]["properties"],
            serde_json::json!({}),
            "结束工具没有参数格"
        );
        assert_eq!(
            wire[1]["parameters"]["required"],
            serde_json::json!([]),
            "也没有必填格"
        );
        assert_eq!(
            wire[0]["description"],
            serde_json::json!(INVOKE_DESCRIPTION),
            "入口的说明与 chat 侧是同一份（换 wire 不该让模型看到两套契约）"
        );
        assert_eq!(
            wire[1]["description"],
            serde_json::json!(FINISH_DESCRIPTION),
            "收场工具的说明同样只有一份"
        );
    }
}
