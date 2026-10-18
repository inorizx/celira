//! 手动入口的**离线验收夹具**：它不是产品的一部分，也不是正式命令的第二个菜单项。
//!
//! 它存在的唯一理由，是让离线验收能把一个**离线模型**插进**同一段生产启动代码**：
//! 生产命令 `agent_run` 装配真实适配层，本夹具装配脚本模型、回环适配层或计数替身，
//! 但两者调用的都是 `celira_agent::manual_entry` 的同一个启动函数、读同一份参数结构、
//! 用同一张退出码表。因此这里验证到的接线与呈现就是生产的那一份。
//!
//! 用法（全部通过环境变量，参数与 `agent_run run` 一一对应）：
//!
//! | 变量 | 含义 |
//! |---|---|
//! | `MANUAL_FIXTURE_ROOT` / `_AGENT` / `_MATERIALS` / `_RUN` / `_MAX_TURNS` | 与命令行的 `--root` / `--agent` / `--materials` / `--run` / `--max-turns` 相同 |
//! | `MANUAL_FIXTURE_INPUT` | 开场文本；不给就是空输入 |
//! | `MANUAL_FIXTURE_STDIN=1` | 显式从标准输入读取（与上面互斥） |
//! | `MANUAL_FIXTURE_MODEL` | `script` / `read-back` / `loopback` / `counting` |
//! | `MANUAL_FIXTURE_SCRIPT` | `script` 模型的步骤，`;` 分隔：`finish`、`act:open_app?target=files` |
//! | `MANUAL_FIXTURE_ENDPOINT` / `_CREDENTIAL` / `_MODEL_NAME` | `loopback` 模型的回环端点与**哨兵**凭据（服务由验收测试侧起，本夹具只做客户端） |
//! | `MANUAL_FIXTURE_ECHO` | 指定一个动作标识：它交回的**观察正文**会被如实回显成 `FIXTURE:echo=…`（跨进程读回内容的证据来源） |
//! | `MANUAL_FIXTURE_PROBE` | `read-back` 模型要核对的探针文本（旧活动留下的正文） |
//! | `MANUAL_FIXTURE_CREATE_DIR` | 资料根内的**单个目录名**：显式授予本次活动在该目录创建文本的权限；不给就是只读 |
//! | `MANUAL_FIXTURE_HOST_LINES` | 显式启用**宿主输入循环**，并按 `\n` 分隔给出后续几行宿主动作（空行=显式继续，不给变量或空值=立刻 EOF）。消费的是与生产命令同一个会话循环 |
//! | `MANUAL_FIXTURE_INIT=1` | 改为跑初始化路径（只调用既有 create） |
//! | `MANUAL_FIXTURE_CLOCK` | 状态栏用的**时间来源**：自 1970-01-01T00:00:00Z 起的整数秒（不给就是真实钟） |
//! | `MANUAL_FIXTURE_CONTEXT_WINDOW` | 宿主声明的**上下文窗口**（正整数；不给就是适配层标称值） |
//! | `MANUAL_FIXTURE_IM_ENDPOINT` | Celira IM 的端点；给了它就必须同时给出 `CELIRA_IM_BACKEND_AUTH`（缺一半即入口拒绝） |
//!
//! 全程离线：不读真实凭据、不访问公网；stdout 只打稳定报告与一行夹具计数。

use std::collections::VecDeque;

use celira_agent::clock::ClockHandle;
use celira_agent::composition::SessionDeclarations;
use celira_agent::context_reading::{ContextReading, ContextReadingWriter};
use celira_agent::core::{
    Action, ActionId, ActionParams, ModelError, ModelFailureClass, ModelOutput, ModelPort,
    TraceEntry, View,
};
use celira_agent::cost::{UsageReport, UsageWriter};
use celira_agent::file_app::FILE_APP_ID;
use celira_agent::file_view::HISTORY_DIR;
use celira_agent::foundation::{AgentId, RunId, SubjectEnding};
use celira_agent::im_channel::ImAccess;
use celira_agent::manual_entry::{
    ManualInput, ManualRun, create_environment, describe_error, render, render_created,
    run_host_session, run_manual_activity,
};
use celira_agent::provider_adapter::{DeepSeekModel, ShapeTrace};
use celira_agent::responses_adapter::ResponsesModel;
use celira_agent::web_app::SearchAccess;

fn main() {
    std::process::exit(run());
}

/// 夹具给出的**时间来源**：`MANUAL_FIXTURE_CLOCK` 是自 1970-01-01T00:00:00Z 起的整数秒。
///
/// 给了就固定在那个时刻（离线验收因此可以对状态行逐字节断言），不给就是真实钟——两条路走的是
/// **同一段装配代码、同一个格式化点**（见 `status_bar`），差别只在这一个构造参数。
fn fixture_clock() -> ClockHandle {
    match std::env::var("MANUAL_FIXTURE_CLOCK") {
        Ok(seconds) => ClockHandle::fixed_at_unix_seconds(
            seconds.parse().expect("MANUAL_FIXTURE_CLOCK 必须是整数秒"),
        ),
        Err(_) => ClockHandle::system(),
    }
}

/// 夹具声明的**上下文窗口**：`MANUAL_FIXTURE_CONTEXT_WINDOW`（正整数），不给就是适配层标称值。
///
/// 与生产命令同一套规矩：窗口只能由宿主声明，`0` 不是一条可解释的声明。
fn fixture_context_window() -> u32 {
    match std::env::var("MANUAL_FIXTURE_CONTEXT_WINDOW") {
        Ok(text) => {
            let window: u32 = text
                .parse()
                .expect("MANUAL_FIXTURE_CONTEXT_WINDOW 必须是整数");
            assert!(window > 0, "MANUAL_FIXTURE_CONTEXT_WINDOW 必须是正整数");
            window
        }
        Err(_) => celira_agent::provider_adapter::nominal_context_window(),
    }
}

/// 夹具的搜索声明：端点来自 `MANUAL_FIXTURE_SEARCH_ENDPOINT`，凭据来自其配对变量。
///
/// 与 im 那条同一条纪律：**两者必须成对**，缺一半直接拒绝（报变量名不报取值）。
fn fixture_search() -> Option<SearchAccess> {
    const CREDENTIAL_ENV: &str = "MANUAL_FIXTURE_SEARCH_CREDENTIAL";
    let endpoint = std::env::var("MANUAL_FIXTURE_SEARCH_ENDPOINT").ok();
    let credential = std::env::var(CREDENTIAL_ENV).ok();
    match (endpoint, credential) {
        (None, None) => None,
        (Some(base), Some(secret)) => Some(
            SearchAccess::new(base, secret)
                .expect("MANUAL_FIXTURE_SEARCH_ENDPOINT 与凭据都必须非空"),
        ),
        _ => panic!("搜索端点与 {CREDENTIAL_ENV} 必须成对给出"),
    }
}

/// 夹具的 im 声明：端点来自 `MANUAL_FIXTURE_IM_ENDPOINT`，凭据来自 `CELIRA_IM_BACKEND_AUTH`。
///
/// 与生产命令同一套规矩：**两者必须成对**，缺一半直接拒绝（报变量名不报取值）。
fn fixture_im() -> Option<ImAccess> {
    const CREDENTIAL_ENV: &str = "CELIRA_IM_BACKEND_AUTH";
    let endpoint = std::env::var("MANUAL_FIXTURE_IM_ENDPOINT").ok();
    let credential = std::env::var(CREDENTIAL_ENV).ok();
    match (endpoint, credential) {
        (None, None) => None,
        (Some(base), Some(secret)) => {
            Some(ImAccess::new(base, secret).expect("MANUAL_FIXTURE_IM_ENDPOINT 与凭据都必须非空"))
        }
        _ => panic!("im 的端点与 {CREDENTIAL_ENV} 必须成对给出"),
    }
}

fn run() -> i32 {
    let root = std::path::PathBuf::from(env_or_panic("MANUAL_FIXTURE_ROOT"));
    let agent = AgentId::new(env_or_panic("MANUAL_FIXTURE_AGENT"));

    // 屏幕报告 cell：**写柄进适配层**（唯一写者），读侧随声明进状态栏。
    let (reading_writer, reading) = ContextReading::channel();
    // 用量 cell（TASK_78 刀三）：与屏幕同款——写柄交适配层，读侧进会话（回环模型可借它证明
    // "用量入账"这一条：适配层写、收场读）。
    let (usage_writer, usage) = UsageReport::channel();
    // TASK_60 §6：形态指纹的去向。装配时递进模型，**跑完由这里取走打印**（库不打印）。
    let shape_trace = ShapeTrace::new();
    let context_window = fixture_context_window();

    if std::env::var("MANUAL_FIXTURE_INIT").is_ok() {
        return match create_environment(&root, &agent) {
            Ok(recorded) => {
                print!("{}", render_created(&root, &agent, &recorded));
                0
            }
            Err(error) => {
                eprintln!("入口拒绝: {} ({})", error.as_str(), describe_error(&error));
                error.exit_code() as i32
            }
        };
    }

    let run_id = RunId::new(env_or_panic("MANUAL_FIXTURE_RUN"));
    // 授权只由宿主（这里就是夹具的调用者）通过环境变量给出；夹具自己不发明权限。
    let creation = match std::env::var("MANUAL_FIXTURE_CREATE_DIR") {
        Ok(directory) => Some(
            celira_agent::creation_grant::CreationGrant::new(
                agent.clone(),
                run_id.clone(),
                directory,
            )
            .expect("夹具给出的授权目录名必须合法"),
        ),
        Err(_) => None,
    };

    let request = ManualRun {
        environment_root: root,
        expected_agent: agent,
        material_root: std::path::PathBuf::from(env_or_panic("MANUAL_FIXTURE_MATERIALS")),
        run: run_id,
        max_turns: env_or_panic("MANUAL_FIXTURE_MAX_TURNS")
            .parse()
            .expect("上限必须是整数"),
        input: read_input(),
        clock: fixture_clock(),
        context_window,
        reading,
        declarations: SessionDeclarations {
            creation,
            usage: Some(usage),
            im: fixture_im(),
            search: fixture_search(),
            // 其余几格**夹具都不声明**：模型名与思考强度（它不接真模型 ⇒ 开场那两格整块不出场，
            // 与身份行同款：不编）、身份行（TASK_70）、终端档位、MCP（离线夹具不装外部来源的 App：
            // 它的用例直接走 `MinimalOs::assemble`，而"跨进程 + 真 server"那条路由生产入口的
            // `--mcp` 覆盖）。`Default` 就是"全都没声明"。
            ..Default::default()
        },
    };

    let kind = std::env::var("MANUAL_FIXTURE_MODEL").unwrap_or_else(|_| "script".to_string());
    let mut model = build_model(&kind, &reading_writer, &usage_writer, &shape_trace);
    let mut stdin = std::io::stdin().lock();

    // 宿主输入循环：**同一个**生产会话接口，只是后续几行由环境变量给出（离线、确定、可断言）。
    // 不给这个变量就是一次性路径——两条路走的是同一段启动代码与同一张退出码表。
    let host_lines = std::env::var("MANUAL_FIXTURE_HOST_LINES").ok();

    // 无论哪条路径，都打一行"模型被调用了几次"：拒绝路径的"零调用"由它作证。
    let outcome = match &host_lines {
        Some(lines) => {
            let mut host = std::io::Cursor::new(lines.as_bytes());
            run_host_session(&request, &mut host, &mut model)
        }
        None => run_manual_activity(&request, &mut stdin, &mut model),
    };
    // TASK_60 §6：**认不出**的失败留下的那一行（夹具是测试件 ⇒ 与宿主同样打出来，
    // 端到端判据才有东西可断）。
    for line in shape_trace.drain() {
        eprintln!("{line}");
    }
    match outcome {
        Ok(ending) => {
            print!("{}", render(&request, &ending));
            println!("FIXTURE:model-calls={}", model.calls);
            ending.exit_code() as i32
        }
        Err(error) => {
            eprintln!("入口拒绝: {} ({})", error.as_str(), describe_error(&error));
            println!("FIXTURE:model-calls={}", model.calls);
            error.exit_code() as i32
        }
    }
}

fn env_or_panic(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("夹具必须给出 {name}"))
}

fn read_input() -> ManualInput {
    if std::env::var("MANUAL_FIXTURE_STDIN").is_ok() {
        return ManualInput::Stdin;
    }
    match std::env::var("MANUAL_FIXTURE_INPUT") {
        Ok(text) => ManualInput::Text(text),
        Err(_) => ManualInput::Empty,
    }
}

// ---------------------------------------------------------------------------
// 模型替身
// ---------------------------------------------------------------------------

/// 可注入的模型：不同种类只为覆盖不同验收面，都走同一条生产启动路径。
///
/// 这里用**本地枚举**而不是 `Box<dyn ModelPort>`：在 bin 里为 `Box<dyn 外来 trait>` 实现该
/// 外来 trait 会违反孤儿规则，而枚举分派既合法又不引入第二层抽象。
struct FixtureModel {
    inner: AnyModel,
    calls: usize,
}

enum AnyModel {
    /// 按脚本作答；脚本用尽就报模型失败（用来制造系统终止）。
    ///
    /// `reported` 记住已经报告到轨迹的哪一条：脚本模型是唯一"看得到"轨迹的替身，因此由它把
    /// 新出现的拒绝与结果如实打出来，供验收核对。
    Script {
        script: VecDeque<ModelOutput>,
        reported: usize,
    },
    /// 只数调用次数，永远立刻收场（用来断言"该被拒绝时一次都没被叫到"）。
    Counting,
    /// 主动查历史的模型（用来证明能力目录真的可见、历史真的读得回来）。
    ReadBack(ReadBackModel),
    /// 真实适配层指向回环端点（用来证明真实请求形状与行动-结果往返）。
    Loopback(DeepSeekModel),
    /// 同上，但走**第二条线协议**（Responses）：两条 wire 各自端到端一次。
    LoopbackResponses(ResponsesModel),
}

impl ModelPort for FixtureModel {
    /// **透明转发**（TASK_79）：静态头由装配装订给"这一次真正发请求的那个模型"。
    ///
    /// 不转发的话，装上真实适配层那几档（`Loopback` / `LoopbackResponses`）就会**静默丢掉**
    /// 静态头——而这一层只是替身的外壳，它不该改变请求的内容。
    fn declare_static_head(&mut self, head: Option<&str>) {
        match &mut self.inner {
            AnyModel::ReadBack(model) => model.declare_static_head(head),
            AnyModel::Loopback(model) => model.declare_static_head(head),
            AnyModel::LoopbackResponses(model) => model.declare_static_head(head),
            // 脚本替身与计数替身不发请求 ⇒ 没有"请求里摆什么"这回事。
            AnyModel::Script { .. } | AnyModel::Counting => {}
        }
    }

    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        self.calls += 1;
        match &mut self.inner {
            AnyModel::Script { script, reported } => {
                report_new_entries(view, reported);
                script.pop_front().ok_or(scripted_model_failure())
            }
            AnyModel::Counting => Ok(ModelOutput::finish(SubjectEnding::Ended)),
            AnyModel::ReadBack(model) => model.complete(view),
            AnyModel::Loopback(model) => model.complete(view),
            AnyModel::LoopbackResponses(model) => model.complete(view),
        }
    }
}

fn build_model(
    kind: &str,
    reading: &ContextReadingWriter,
    usage: &UsageWriter,
    shape_trace: &ShapeTrace,
) -> FixtureModel {
    let inner = match kind {
        "script" => AnyModel::Script {
            script: VecDeque::from(parse_script(
                &std::env::var("MANUAL_FIXTURE_SCRIPT").unwrap_or_default(),
            )),
            reported: 0,
        },
        "counting" => AnyModel::Counting,
        "read-back" => AnyModel::ReadBack(ReadBackModel::default()),
        "loopback" => AnyModel::Loopback(
            DeepSeekModel::at_endpoint(
                std::env::var("MANUAL_FIXTURE_CREDENTIAL")
                    .unwrap_or_else(|_| "fixture-sentinel".to_string()),
                std::env::var("MANUAL_FIXTURE_MODEL_NAME")
                    .unwrap_or_else(|_| "fixture-model".to_string()),
                env_or_panic("MANUAL_FIXTURE_ENDPOINT"),
            )
            .expect("回环适配层应当可以装配")
            .with_shape_trace(shape_trace.clone())
            // 装上传感器：回环响应的 usage 从此会更新那个 cell（只有适配层写）。
            .with_reading(reading.clone())
            .with_usage(usage.clone()),
        ),
        "loopback-responses" => AnyModel::LoopbackResponses(
            ResponsesModel::at_endpoint(
                std::env::var("MANUAL_FIXTURE_CREDENTIAL")
                    .unwrap_or_else(|_| "fixture-sentinel".to_string()),
                std::env::var("MANUAL_FIXTURE_MODEL_NAME")
                    .unwrap_or_else(|_| "fixture-model".to_string()),
                env_or_panic("MANUAL_FIXTURE_ENDPOINT"),
            )
            .expect("回环适配层应当可以装配")
            .with_reading(reading.clone())
            .with_usage(usage.clone()),
        ),
        other => panic!("未知的夹具模型种类: {other}"),
    };
    FixtureModel { inner, calls: 0 }
}

/// 把轨迹里**新出现**的条目如实打出来：拒绝（动作 + 分类）与行动结果（动作 + 三分 + 失败分类）。
///
/// 这是"拒绝经历对下一次模型调用可见"的证据来源——它读的是主体真正看得到的那份视野。
fn report_new_entries(view: &View, reported: &mut usize) {
    for entry in view.trace().iter().skip(*reported) {
        match entry {
            TraceEntry::Injection { .. } => {}
            // 模型响应不是"拒绝"也不是"结果"：它的正文由结构化历史承载，这里不重复回显。
            TraceEntry::ModelResponse(_) => {}
            TraceEntry::Refused { action, class, .. } => {
                println!(
                    "FIXTURE:saw-refusal={}:{}",
                    escaped(action.as_str()),
                    escaped(class)
                );
            }
            TraceEntry::Observation(observation) => {
                println!(
                    "FIXTURE:saw-result={}:{}:{}",
                    escaped(observation.action().as_str()),
                    state_name(observation.state()),
                    escaped(observation.outcome().failure_class().unwrap_or("none"))
                );
                // 需要时把某个动作交回的内容**原文**回显出来：这是"跨进程真的读到了什么"的证据。
                if std::env::var("MANUAL_FIXTURE_ECHO").as_deref()
                    == Ok(observation.action().as_str())
                {
                    let text: String = observation
                        .outcome()
                        .content()
                        .iter()
                        .map(|block| block.text())
                        .collect::<Vec<_>>()
                        .join("\n");
                    println!("FIXTURE:echo={}", escaped(&text));
                }
            }
        }
    }
    *reported = view.trace().len();
}

/// 三分结果的稳定名字。
fn state_name(state: celira_agent::foundation::ResultState) -> &'static str {
    match state {
        celira_agent::foundation::ResultState::Success => "success",
        celira_agent::foundation::ResultState::Failure => "failure",
        celira_agent::foundation::ResultState::Unknown => "unknown",
    }
}

/// 与适配层同一套转义：开放文本进单行字段前必须不能伪造行结构。
fn escaped(text: &str) -> String {
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

/// 解析脚本：`;` 分隔步骤；`finish`、`say:<正文>`，或 `act:<标识>`（后面可跟
/// `?键=值&键=值`）。
///
/// `finish` **不带值**（2026-10-10 裁决：结束了就是结束了）——夹具是**模型的替身**，
/// 因此它必须与线缆同形：从前那个 `finish:<ending>` 形状**线缆上已经产不出来**，
/// 留着它只会让用例模拟一件主体做不到的事。
///
/// `say:` 是**纯正文**步骤：它对应"交回控制、活动不结束"的那一种中立决定，是离线复现
/// "停在等待"的唯一手段——因此它必须走与生产同一条构造路径（`ModelOutput::say`）。
fn parse_script(text: &str) -> Vec<ModelOutput> {
    let mut steps = Vec::new();
    for step in text.split(';').filter(|step| !step.trim().is_empty()) {
        let step = step.trim();
        if step == "finish" {
            steps.push(ModelOutput::finish(SubjectEnding::Ended));
            continue;
        }
        if let Some(body) = step.strip_prefix("say:") {
            steps.push(ModelOutput::say(body));
            continue;
        }
        let act = step
            .strip_prefix("act:")
            .unwrap_or_else(|| panic!("夹具脚本只认识 finish/say:/act: 三种步骤，收到: {step}"));
        let (id, params) = match act.split_once('?') {
            Some((id, query)) => (id, parse_params(query)),
            None => (act, ActionParams::none()),
        };
        steps.push(ModelOutput::act(Action::with_params(
            ActionId::new(id),
            params,
        )));
    }
    steps
}

fn parse_params(query: &str) -> ActionParams {
    let mut params = ActionParams::none();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair
            .split_once('=')
            .unwrap_or_else(|| panic!("参数必须是 键=值: {pair}"));
        params = params.with(name, value);
    }
    params
}

/// 主动查历史的模型：每一步都先读**真实可见的能力目录**，再从上一轮的观察里取下一步的地址。
///
/// 这样才证明得了"能力目录真的可见、历史真的能从通用文件能力读回来"，而不是照剧本盲发动作。
/// 活动目录名与分段名都从真实列表结果里取，不写死。
#[derive(Default)]
struct ReadBackModel {
    step: usize,
    run_dir: Option<String>,
    /// 上一次查过的能力面（用于发现"视野变了"）——与测试侧 `InspectsFirst` 同款。
    seen: Vec<String>,
    /// 自己发出的动作总数（**含**"先看格式"那些）——`observations-match-own-actions` 用它。
    own_actions: usize,
}

impl ModelPort for ReadBackModel {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        let probe = std::env::var("MANUAL_FIXTURE_PROBE").unwrap_or_default();
        let ids: Vec<String> = view
            .capabilities()
            .items()
            .iter()
            .map(|capability| capability.id().as_str().to_string())
            .collect();

        // TASK_69：**先看格式**——每一轮把"**在场却还没看过**"的都看一遍
        // （照测试侧 `InspectsFirst` 的形状）。
        //
        // ⚠️ **不能写死一批**：`inspect` 的范围是"**此刻真能调的那些**"——
        // 展开**之前** `files_*` 根本不在场（我第一次就是这么错的：一上来去看八个还没出现的
        // 名字 ⇒ 当场被拒 ⇒ 夹具连一行 `FIXTURE:` 都没打印就死了，退出码 7）。
        let unseen: Vec<String> = ids
            .iter()
            .filter(|name| !self.seen.contains(name))
            .cloned()
            .collect();
        if !unseen.is_empty() {
            self.seen = ids.clone();
            self.own_actions += unseen.len();
            let batch: Vec<Action> = unseen
                .iter()
                .map(|name| {
                    Action::with_params(
                        ActionId::new("inspect_action"),
                        ActionParams::none().with("action", name.clone()),
                    )
                })
                .collect();
            return Ok(ModelOutput::act_many(batch).expect("一批至少一个动作"));
        }

        match self.step {
            0 => {
                println!(
                    "FIXTURE:first-view-had-probe={}",
                    view_text(view).contains(&probe)
                );
                println!(
                    "FIXTURE:first-view-had-foreign-actions={}",
                    // TASK_69：自己开场那批"先看格式"的结果是**自己的**动作，
                    // 不算"从别处搬来的"。这一格问的是：视野里有没有**别的东西**。
                    view.observed()
                        .iter()
                        .any(|observation| observation.action().as_str() != "inspect_action")
                );
                self.step = 1;
                self.own_actions += 1;
                Ok(ModelOutput::act(Action::with_params(
                    ActionId::new("open_app"),
                    ActionParams::none().with("target", FILE_APP_ID),
                )))
            }
            1 => {
                let saw_list = ids.iter().any(|id| id == "files_list");
                let saw_read = ids.iter().any(|id| id == "files_read");
                println!("FIXTURE:saw-list={saw_list}");
                println!("FIXTURE:saw-read={saw_read}");
                if !saw_list || !saw_read {
                    return Err(scripted_model_failure());
                }
                self.step = 2;
                self.own_actions += 1;
                Ok(ModelOutput::act(Action::with_params(
                    ActionId::new("files_list"),
                    ActionParams::none().with("path", HISTORY_DIR),
                )))
            }
            2 => {
                let Some(run_dir) = first_entry_of_kind(&last_observation_text(view), "dir") else {
                    return Err(scripted_model_failure());
                };
                self.run_dir = Some(run_dir.clone());
                self.step = 3;
                self.own_actions += 1;
                Ok(ModelOutput::act(Action::with_params(
                    ActionId::new("files_list"),
                    ActionParams::none().with("path", format!("{HISTORY_DIR}/{run_dir}")),
                )))
            }
            3 => {
                let Some(segment) = first_entry_of_kind(&last_observation_text(view), "file")
                else {
                    return Err(scripted_model_failure());
                };
                let run_dir = self.run_dir.clone().ok_or(scripted_model_failure())?;
                self.step = 4;
                self.own_actions += 1;
                Ok(ModelOutput::act(Action::with_params(
                    ActionId::new("files_read"),
                    ActionParams::none().with("path", format!("{HISTORY_DIR}/{run_dir}/{segment}")),
                )))
            }
            _ => {
                println!(
                    "FIXTURE:read-back-had-probe={}",
                    last_observation_text(view).contains(&probe)
                );
                println!(
                    "FIXTURE:observations-match-own-actions={}",
                    view.observed().len() == self.own_actions
                );
                Ok(ModelOutput::finish(SubjectEnding::Ended))
            }
        }
    }
}

fn view_text(view: &View) -> String {
    let mut out = String::new();
    for entry in view.trace() {
        match entry {
            TraceEntry::Injection { batch, .. } => {
                for block in batch.blocks() {
                    out.push_str(block.text());
                    out.push('\n');
                }
            }
            TraceEntry::Observation(observation) => {
                for block in observation.outcome().content() {
                    out.push_str(block.text());
                    out.push('\n');
                }
            }
            // 拒绝不是结果，也不是注入：夹具把它的动作标识与分类如实拼进视野文本。
            TraceEntry::Refused { action, class, .. } => {
                out.push_str(action.as_str());
                out.push(' ');
                out.push_str(class);
                out.push('\n');
            }
            // 模型响应同样不是结果：它的正文由结构化历史承载，不在这里拼进扁平文本。
            TraceEntry::ModelResponse(_) => {}
        }
    }
    out
}

fn last_observation_text(view: &View) -> String {
    view.observed()
        .last()
        .map(|observation| {
            observation
                .outcome()
                .content()
                .iter()
                .map(|block| block.text())
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// 从一段目录列表正文里机械地取第一个某种种类的条目名。
fn first_entry_of_kind(text: &str, kind: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (name, entry_kind) = line.split_once('\t')?;
        (entry_kind == kind).then(|| name.to_string())
    })
}

/// 这一份用例里的"模型失败"分类：脚本用尽 = 这次响应没有履约。
///
/// 这是**测试替身**的失败，不是某个真实服务行为；它只用来把"失败分类"这条路径走通。
/// 生产路径的分类由适配层按九类如实给出。
const SCRIPTED_MODEL_FAILURE: ModelFailureClass = ModelFailureClass::ContractViolation;

/// 这一份用例里的"模型失败"（带上上面那个分类）。
fn scripted_model_failure() -> ModelError {
    ModelError {
        class: SCRIPTED_MODEL_FAILURE,
    }
}
