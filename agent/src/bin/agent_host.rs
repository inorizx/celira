//! 常驻宿主：**门铃响一次就开一段在场，此后每一声铃驱动同一段**（`agent_host`）。
//!
//! 它是"世界能叫醒主体"的第一件落地件：把 im 服务端那条唤醒流接到执行上，让"人给主体发一条消息"
//! 这件事**不再需要有人在终端敲一条命令**。它不做编排判断、不做调度、不做唤醒政策：
//!
//! ```text
//! 起一次门铃（im 的唤醒流）→ 等一次唤醒 → 开一段在场（空输入 / 门铃起源 / 机械标识）
//!                          → 这一段收场 → 回监听 …
//! ```
//!
//! ⚠ **在场跨多少声铃，由主体定；不是由铃定**（2026-10-10 在场那一刀）：开这一段的那次唤醒之后，
//! **后来每一声音铃都驱动同一段**（主体在场时的新动静由事件卡片那一格摆到眼前），
//! 直到主体**自己说结束**或系统终止。铃在传输层上只说"有新动静"，它不该顺手切断主体的在场。
//!
//! **它只做这三件事**：接门铃、按上面那条顺序跑一段在场、把每一次读数与每一次退出原因如实打出来。
//! 门铃不携带内容（主体要知道发生了什么，自己去看），门铃 Run **不带**任何授权族（本命令没有
//! `--create-dir`：能不能在材料目录里新建文本由宿主显式声明，本刀不声明；自己的笔记
//! 写在自己的环境里，本来就不需要声明）。
//!
//! 退出码是**读数语义**（常驻进程只在 fail-closed 时退出）：`0` 帮助 ／ `1` 参数错 ／
//! `2` 环境不可用 ／ `3` 需要用户动作（已暂停）／ `4` 唤醒通道终结 ／ `5` 模型凭据不可用。

use std::path::Path;
use std::process::ExitCode;

use celira_agent::clock::ClockHandle;
use celira_agent::composition::SessionDeclarations;
use celira_agent::context_reading::{ContextReading, ContextReadingWriter};
use celira_agent::cost::{UsageReport, UsageWriter};
use celira_agent::doorbell::{Doorbell, DoorbellAccess, DoorbellPolicy};
use celira_agent::foundation::{AgentId, Outcome, SystemEnding};
use celira_agent::host_loop::{
    Host, HostFailure, HostOptions, RunWatch, WriteQualification, admission_text, open_when_free,
};
use celira_agent::im_channel::ImAccess;
use celira_agent::manual_entry::escape_field;
use celira_agent::mcp::{McpAccess, parse_declaration, stdio_available};
use celira_agent::model_choice::ModelChoice;
use celira_agent::model_readings::{ModelReadings, ModelReadingsWriter};
use celira_agent::model_source;
use celira_agent::provider_adapter::{
    self, DEFAULT_MODEL, DeepSeekModel, ENDPOINT, ShapeTrace, nominal_context_window,
};
use celira_agent::responses_adapter::{self, ResponsesModel};
use celira_agent::self_concept::IdentityClaim;
use celira_agent::terminal_app::{TerminalAccess, TerminalTier};
use celira_agent::timer_app::TIMER_FILE;
use celira_agent::wake::{TimerSource, WakeHub, Woken};
use celira_agent::web_app::{SearchAccess, WebTier};

/// im 的 BackendAuth 凭据环境变量（与既有入口同一个变量、同一条纪律：只报变量名，不报值）。
const IM_CREDENTIAL_ENV: &str = "CELIRA_IM_BACKEND_AUTH";

/// 换家档的模型凭据环境变量（**唯一权威在这里**：装配那一处用它取值，模型门的冲突判据用它
/// 认"进程侧在不在说话"）。同样只报变量名，不报取值。
const CUSTOM_MODEL_CREDENTIAL_ENV: &str = "CELIRA_MODEL_API_KEY";

const USAGE: &str = "\
用法:
  agent_host --root <环境根> --agent <AgentId> --materials <授权资料根> --max-turns <正整数>
             --im-endpoint <URL> [--context-window <正整数>]
  agent_host --help

说明:
  常驻宿主：接 im 服务端的唤醒流（门铃），响一次就开一段在场；
  这一段此后被每一声音铃再驱动（同一段，上下文累加），直到主体自己说结束或系统终止。

  --root            环境根（一次一个环境）。
  --agent           身份（与环境里记录的权威身份不一致即拒绝）。
  --materials       显式授权的资料根；不会被默认为当前工作目录。
  --max-turns       一段在场最多几次模型调用（跨这一段的每一次唤醒累加；
                    它用尽即以系统收场结束，是这一段的硬上界）。
  --im-endpoint     声明 Celira IM 的端点：门铃就来自它。必给——没有它就没有门铃。
  --context-window  声明这次清醒期的上下文窗口（状态行 `context` 段的分母）；
                    不给时取适配层当前模型档位的公开标称值。`0` 不是合法声明。
  --terminal <tier> 声明 terminal 窗的档位：不给就没有这只手（动词不存在）；
                    v1 只有 `full`（整机全权）。未知档位直接拒绝，不静默降档。
  --mcp <声明>      装一台外部来源的 App（MCP server），可重复（一台一条）。
                    形状：`<名>=stdio:<命令>[,read-only][,timeout=<秒>]`
                      <名>      动词前缀：它的动词叫 `<名>_<工具名>`（ASCII 字母开头）。
                      stdio     通道（v1 只有这一种）；命令交给 Windows 自带命令行解释器。
                      read-only 宿主显式背书「这台不改外面的世界」；不给就是最严那档
                                （unverifiable-write：那件写没有同参复核可配）。
                      timeout   转发窗口秒数（1–600，缺省 120）。
                    装了不用一分钱不花：子进程不起、清单不拉，直到 `open_app` 那一下。
                    装/卸要重启本进程——授权面是装配期事实，不在 Run 中途变。
  --model <id>      模型名：默认档下可单独切本家的另一个档位；换家档下是必答件之一。
  --model-endpoint  声明模型端点（OpenAI 兼容的 chat completions）。一给就进换家档：
                    端点 / 凭据 / 模型名 / 标称窗口四件全显式，缺一件即退出码 5。
  --model-protocol  声明线协议：`chat-completions`（缺省，现状零迁移）或 `responses`。
                    它是换家档的第五件——`responses` 只在给了 `--model-endpoint` 时有效
                    （默认档是本家的那一个端点，没有第二种协议），否则退出码 5，不猜。
                    给了 `responses` 时，端点值就是完整地址（例：`…/v1/responses`）：
                    本层不替调用方拼路径，与 chat 档同一口径。

  DEEPSEEK_API_KEY       装配模型时读取；缺失或为空即退出码 5，取值不写入任何输出。
  CELIRA_IM_BACKEND_AUTH im 的 BackendAuth 凭据：必须给（门铃与 im 动词都用它）；
                          只进请求头，取值不打印、不落盘。
  CELIRA_MODEL_API_KEY   换家档的模型凭据（给了 --model-endpoint 时读取）：与端点同一次解析、
                          成对出现；缺失或为空即退出码 5。取值同样不打印、不落盘。

  模型配置门（清单归机器，绑定归 Agent）：这台机器有哪些钥匙写在 `<家>/model_sources.json`
  （「模型配置」页写），这一个 Agent 用哪一条写在环境根下的 `model_binding.json`；清单在哪
  由 `--model-sources <清单路径>` 指出。绑定在场 ⇒ 四件（端点 / 密钥 / 模型名 / 标称窗口，+ 可选
  线协议）从清单那一条解析，进程侧必须一言不发：再声明任何一个模型来源 ⇒ 拒绝启动，并说清
  是哪两个打架（不猜、不静默覆盖）；绑的那条被删了（悬空绑定）、清单路径没给、老的单份配置还
  在场（双权威）同样拒绝。绑定为源时每次唤醒重读：下一次活动用新配置，正在跑的活动绝不受
  打扰；读不出来如实失败，不回落进程侧。没有绑定时退回老形状：环境根下的
  `model_source.json`（四件套）在场即以它为准，再没有就是上面那几条进程侧来源。

  每一次唤醒都是空输入的活动（门铃不携带内容），起源事实如实写成门铃那句；
  活动标识由本进程机械生成（`host-<进程号>-<毫秒>-<序号>`）。

  它不自动重试、不自动换凭据、不自动清除暂停、不继续任何旧活动：
  唤醒通道终结与环境准入问题都以退出读数 + 一行 next 交回给操作者。
";

struct Options {
    root: String,
    agent: String,
    materials: String,
    max_turns: u32,
    /// 标称窗口的**原始声明**（最终值要看档位 ⇒ 与模型源一起解析）。
    context_window: Option<String>,
    im_endpoint: String,
    terminal_tier: Option<String>,
    /// **出网档**的原始声明（`--web off|fetch|full`；缺省 full = 出厂以来的样子）。
    web_tier: Option<String>,
    /// `--mcp` 的**原始声明**（可重复，一台一条）。
    mcp: Vec<String>,
    /// 模型名（原始声明）。
    model: Option<String>,
    /// 模型端点（原始声明；**一给就进换家档**）。
    model_endpoint: Option<String>,
    /// 线协议（原始声明；缺省 = 本家那一条 chat completions）。
    model_protocol: Option<String>,
    /// **模型源清单**的路径（刀2b：宿主显式声明；这个 Agent 用清单哪一条看环境根的绑定文件）。
    model_sources: Option<String>,
    /// **主体是谁**的两个值（原始声明；**成对**给出，缺一半是入口拒绝）。
    ///
    /// 门铃路径也**当次声明**：此世界的名字会改，而"此刻我是谁"只能由这一次的宿主说得准。
    identity_agent: Option<String>,
    identity_name: Option<String>,
}

fn parse(args: &[String]) -> Result<Option<Options>, String> {
    let mut root: Option<String> = None;
    let mut agent: Option<String> = None;
    let mut materials: Option<String> = None;
    let mut max_turns: Option<String> = None;
    let mut context_window: Option<String> = None;
    let mut im_endpoint: Option<String> = None;
    let mut terminal_tier: Option<String> = None;
    let mut web_tier: Option<String> = None;
    let mut mcp: Vec<String> = Vec::new();
    let mut model: Option<String> = None;
    let mut model_endpoint: Option<String> = None;
    let mut model_protocol: Option<String> = None;
    let mut model_sources: Option<String> = None;
    let mut identity_agent: Option<String> = None;
    let mut identity_name: Option<String> = None;

    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_str();
        match argument {
            "--help" | "-h" => return Ok(None),
            "--root" | "--agent" | "--materials" | "--max-turns" | "--context-window"
            | "--terminal" | "--web" | "--mcp" | "--model" | "--model-endpoint"
            | "--model-protocol" | "--model-sources" | "--identity-agent" | "--identity-name"
            | "--im-endpoint" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| format!("{argument} 后面要给一个值"))?
                    .clone();
                if value.is_empty() {
                    return Err(format!("{argument} 的值不能为空"));
                }
                match argument {
                    "--root" => root = Some(value),
                    "--agent" => agent = Some(value),
                    "--materials" => materials = Some(value),
                    "--max-turns" => max_turns = Some(value),
                    "--context-window" => context_window = Some(value),
                    "--terminal" => terminal_tier = Some(value),
                    "--web" => web_tier = Some(value),
                    // **可重复**：一台 server 一条声明（重名由 resolve 那一关拒绝）。
                    "--mcp" => mcp.push(value),
                    "--model" => model = Some(value),
                    "--model-endpoint" => model_endpoint = Some(value),
                    "--model-protocol" => model_protocol = Some(value),
                    "--model-sources" => model_sources = Some(value),
                    "--identity-agent" => identity_agent = Some(value),
                    "--identity-name" => identity_name = Some(value),
                    _ => im_endpoint = Some(value),
                }
            }
            other => return Err(format!("未知参数: {other}")),
        }
        index += 1;
    }

    let max_turns: u32 = max_turns
        .ok_or_else(|| "缺 --max-turns".to_string())?
        .parse()
        .map_err(|_| "--max-turns 不是整数".to_string())?;
    if max_turns == 0 {
        return Err("--max-turns 必须是正整数".to_string());
    }
    // 窗口是**声明**：只做**语法**校验（最终值要看档位——换家档还要求它必须显式给出）。
    // `0` 不是"更小的窗口"，是一条无法解释的声明。
    if let Some(raw) = &context_window {
        let value: u32 = raw
            .parse()
            .map_err(|_| "--context-window 不是整数".to_string())?;
        if value == 0 {
            return Err("--context-window 必须是正整数".to_string());
        }
    }

    // **成对**：只给一半就是没说清是哪个 Agent（与 im 端点/凭据同一条规矩）。空串与"没给"同罪。
    match (&identity_agent, &identity_name) {
        (None, None) | (Some(_), Some(_)) => {}
        _ => {
            return Err(
                "--identity-agent 与 --identity-name 必须成对给出（缺一半就是没说清是哪个 Agent）"
                    .to_string(),
            );
        }
    }

    Ok(Some(Options {
        terminal_tier,
        web_tier,
        mcp,
        root: root.ok_or_else(|| "缺 --root".to_string())?,
        agent: agent.ok_or_else(|| "缺 --agent".to_string())?,
        materials: materials.ok_or_else(|| "缺 --materials".to_string())?,
        max_turns,
        context_window,
        model,
        model_endpoint,
        model_protocol,
        model_sources,
        identity_agent,
        identity_name,
        im_endpoint: im_endpoint.ok_or_else(|| "缺 --im-endpoint（门铃就来自它）".to_string())?,
    }))
}

/// 把两个原始声明拼成 [`IdentityClaim`]：**成对**才成立（`parse` 已经拦过"只给一半"）。
///
/// 空串在这里再拦一次：一个空的名字不是名字，摆出去只会让主体读到一个空壳。
fn identity_claim(options: &Options) -> Option<IdentityClaim> {
    let id = options.identity_agent.clone()?;
    let display_name = options.identity_name.clone()?;
    let claim = IdentityClaim { id, display_name };
    if claim.is_complete() {
        Some(claim)
    } else {
        None
    }
}

/// im 凭据：**只报变量名**，不报取值；缺失/为空都不是"降级"，是没有门铃。
fn im_credential() -> Result<String, String> {
    match std::env::var(IM_CREDENTIAL_ENV) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => Err(format!(
            "缺少 {IM_CREDENTIAL_ENV}（im 的 BackendAuth 凭据）"
        )),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse(&args) {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("agent_host: {message}\n");
            eprint!("{USAGE}");
            return ExitCode::from(1);
        }
    };

    // **住户的进程寿命**（2026-10-11 主人裁决：主体起的东西活得跟住户一样久）：把本进程收进
    // 住户的进程域——此后它拉起的一切（含主体在终端里脱离出去的服务）都随它一起结束。
    // 读数照打：建不起来**不拦启动**（那是寿命保证，不是安全墙），但不许静默降级。
    let resident_processes = celira_agent::resident_lifetime::adopt_current_process();
    println!("[resident] processes = {resident_processes}");

    let declared_agent = AgentId::new(options.agent.clone());
    let credential = match im_credential() {
        Ok(credential) => credential,
        Err(message) => {
            eprintln!("agent_host: {message}");
            return ExitCode::from(1);
        }
    };
    // 门铃与主体的 im 动词用的是**同一份声明**：端点来自命令行、凭据来自环境变量，两样都显式。
    let Some(access) = ImAccess::new(options.im_endpoint.clone(), credential.clone()) else {
        eprintln!("agent_host: im 端点与凭据都必须非空");
        return ExitCode::from(1);
    };
    let Some(doorbell_access) = DoorbellAccess::new(options.im_endpoint.clone(), credential) else {
        eprintln!("agent_host: 唤醒通道的端点与凭据都必须非空");
        return ExitCode::from(1);
    };

    // 模型源：**三档**（默认零迁移 / 换家四件套 / 环境根配置文件）。声明在横幅之前解析——
    // 横幅要打印生效的窗口与来源，而且"四件套缺一"与"两个来源打架"都必须在起监听之前拒绝。
    // 模型：屏幕报告 cell 的**写柄**交给适配层（唯一写者），读侧每次装配各持一份。
    // 先建通道再解析：解析要按**线协议**选实现，而"装上传感器"是两条协议共同的收尾——
    // 所以装配入口直接把写柄收进去，拿回来的就是一个已经装好传感器的模型。
    let (reading_writer, reading) = ContextReading::channel();
    // 用量 cell（TASK_78 刀三）：与屏幕同款——写柄交适配层（唯一写者），读侧进宿主；
    // 收场时由宿主读出来写进 `run-ended` 那一行的代价格。
    let (usage_writer, usage) = UsageReport::channel();
    // TASK_60 §6：形态指纹的去向。装配时递进模型，**跑完由这里取走打印**（库不打印）。
    let shape_trace = ShapeTrace::new();
    let root = Path::new(&options.root);
    // 逐轮读数账：与前两个 cell 同一条纪律（写柄交适配层、唯一写者是形状），但**落点是文件**——
    // 它服务的是**事后翻文件的操作者**，不是任何执行路径。
    //
    // ⚠ **构造不碰文件系统**：一次没写过，盘上就一份空的都没有（"没有凭据的时刻不得改动环境"
    // 是一条写死的纪律）。"打不开"因此**发现得晚**，但照旧被记下来、在 `[exit]` 里报出。
    let (readings_writer, readings_log) = ModelReadings::channel(root);
    // **模型门的来源判定**（TASK_77）：文件在场就以文件为源；文件在场而进程侧也在说话 ⇒ 拒绝，
    // 并把是哪两个打架说清楚（不猜、不静默覆盖）。
    let list_path = options.model_sources.as_deref().map(Path::new);
    let mut source = match model_source::decide(
        root,
        list_path,
        &model_flags(&options),
        &[DeepSeekModel::API_KEY_ENV, CUSTOM_MODEL_CREDENTIAL_ENV],
    ) {
        Ok(source) => source,
        Err(message) => {
            eprintln!("agent_host: {message}");
            return ExitCode::from(5);
        }
    };
    let (mut model, window) = match resolve_model_source(
        model_request(&options),
        &source,
        &reading_writer,
        &usage_writer,
        &shape_trace,
        Some(&readings_writer),
    ) {
        Ok(resolved) => resolved,
        Err((message, code)) => {
            eprintln!("agent_host: {message}");
            return ExitCode::from(code as u8);
        }
    };

    // 外部来源的 App 的**授权面**（`--mcp`，可重复）：在横幅之前解析完——横幅要打印生效的
    // 档位、窗口与**洗凭据读数**（那时还没起任何子进程：读数与"什么时候起进程"无关），
    // 而"声明读不懂"必须在起监听之前就拒绝。
    let mcp = match resolve_mcp(&options.mcp) {
        Ok(mcp) => mcp,
        Err((message, code)) => {
            eprintln!("{message}");
            return ExitCode::from(code as u8);
        }
    };

    let mut out = String::new();
    use std::fmt::Write as _;
    let _ = writeln!(out, "celira-agent-host v1");
    let _ = writeln!(out, "environment-root={}", escape_field(&options.root));
    let _ = writeln!(out, "agent={}", escape_field(&options.agent));
    let _ = writeln!(out, "material-root={}", escape_field(&options.materials));
    let _ = writeln!(out, "max-turns={}", options.max_turns);
    // 横幅打印**生效的**窗口（声明解析之后的值），而不是原始字符串。
    let _ = writeln!(out, "context-window={window}");
    let _ = writeln!(out, "im-endpoint={}", escape_field(&options.im_endpoint));
    // **模型来源**（TASK_77）：以文件为源时才出场，且**只报来源名与键名**——
    // 端点与模型名不是秘密（同一屏上 `--model-endpoint` 本来就直接进命令行），凭据只报键名。
    // 以进程侧为源时一行都不多：现状零改动。
    for line in model_source::banner_lines(&source) {
        let _ = writeln!(out, "{line}");
    }
    // 每台装上的 server 一行：档位、窗口、命令与**洗凭据读数**（只报键名，永不取值）。
    for access in &mcp {
        let declaration = access.declaration();
        let (scrubbed, unscrubbed) = access.credential_reading();
        let _ = writeln!(
            out,
            "mcp={} effect={} timeout={} scrub={} unscrubbed={} command={}",
            escape_field(declaration.name()),
            declaration.effect().as_str(),
            declaration.timeout_seconds(),
            scrubbed,
            if unscrubbed.is_empty() {
                "none".to_string()
            } else {
                unscrubbed.join(",")
            },
            escape_field(declaration.command()),
        );
    }

    // **先看准入**：环境不让人开始新活动就早失败——比"响了第一声才发现"好。
    //
    // 写资格那一步与唤醒走**同一个函数**：别人此刻正握着（家的只读状态口就是这种）就等，
    // 不是"环境不可用"。两处各写一遍判据迟早会分叉，而分叉的那一天症状是
    // "开机起得来、唤醒却判死"或者反过来。
    let startup = open_when_free(
        Path::new(&options.root),
        &declared_agent,
        WriteQualification::default(),
    );
    let admission = match startup.as_ref().map(|environment| environment.admission()) {
        Ok(Ok(admission)) => admission,
        Ok(Err(error)) => {
            print!("{out}");
            return exit_with(
                &HostFailure::Environment(error.as_str().to_string()),
                &options,
                &declared_agent,
                &readings_log,
            );
        }
        Err(failure) => {
            print!("{out}");
            return exit_with(failure, &options, &declared_agent, &readings_log);
        }
    };
    drop(startup);
    print!("{out}");
    println!("\n[state]\nadmission={}", admission_text(&admission));
    // ⚠ **2026-09-26 改**：`Unfinished` **不再拦**——进程没了就是那一段断了，新进程起来是
    // **新的一段**（见 `composition.rs` 装配点那一句）。
    // ⚠ **2026-10-06 改（主人裁决）**：`Paused` **也不再拦**——它是**事实**不是闸，
    // 由开场那一块纸条带给主体（`PreviousActivity` 的 `StorageFailure` 那一段）。
    // ⇒ 启动前这一处**不再有闸门**：准入读数照旧打出来（`admission=` 那一行），
    // 而"要不要继续"由主体看着事实自己定。真写不下去时，那一轮在**第一次提交**（run-start，
    // 模型调用之前）就停住（`save-failed:…`），不会反复烧调用。

    // terminal 窗的档位：**不给就没有这只手**；不认识的档位、或本平台没有实现的档位，
    // 都在这里直接拒绝（fail-closed，不静默降档、不静默换行为）。
    let terminal = match resolve_terminal(options.terminal_tier.as_deref()) {
        Ok(terminal) => terminal,
        Err((message, code)) => {
            eprintln!("{message}");
            return ExitCode::from(code as u8);
        }
    };

    // **出网档**（2026-10-13）：`--web off|fetch|full`，缺省 full（不改现有行为）。
    // 与 terminal 同一形状：不认识的档位**不降级**、直接拒绝启动（fail-closed）。
    let web_tier = match resolve_web(options.web_tier.as_deref()) {
        Ok(tier) => tier,
        Err((message, code)) => {
            eprintln!("{message}");
            return ExitCode::from(code as u8);
        }
    };

    // 搜索凭据（可选）：没有它不是错误——「查」那一条照旧在场，执行时如实说没配（`credential-absent`）。
    // ⚠ 决定"在不在场"的是上面那一格**出网档**（`--web`）；凭据只管"能不能执行"，两者是两件事。
    let search = search_access();

    // **在跑表**：门铃的铃回调与宿主共用同一份（跑着就拉一次 B1 写进事件卡片的细胞）。
    let watch = RunWatch::new();

    let doorbell_policy = DoorbellPolicy::production();
    let bell = Doorbell::open_with_ring_sink(
        doorbell_access,
        doorbell_policy,
        Some(watch.sink(access.clone())),
    );
    // 这一格**读数**留给宿主自己：门铃一旦交给门，外面就再也看不见它内部在重连了——
    // 而"这条通道一直在重连"正是最该被人看见的一件事（每一次重连都换来一次完整 Run）。
    let bell_reading = bell.reading_handle();
    // **一个钟**：宿主给活动的那一个、与钟源看表的那一个同源——主体看到的 `since` / `time`
    // 与"到点"因此落在同一个时间刻度上（三个读数说的是同一段时间）。
    let clock = ClockHandle::system();
    // TASK_75：**两处来源汇成一条等待**——im 门铃（世界有新动静）＋ 本地钟源（主体约的时刻到点）。
    // 钟源读的是 `<环境根>/timers`：它不经 im，也不改门铃的任何语义。
    //
    // **2026-10-06 删掉门（`gate`）**：门铃**直连**本件，中间不再有任何裁决点。判据是唤醒只决定
    // 「**何时**」，不决定「**要不要**」——"要不要"归主体自己设置，不归系统裁决
    // （`RUNTIME_TERMINAL_DECISIONS.md` §7；唤醒节流那一格见 `agent/README.md` §4）。
    let hub = WakeHub::new(
        bell,
        TimerSource::production(Path::new(&options.root), clock.clone()),
    );
    println!(
        "[listening]\nwake-channel=im-events backoff-ms={}..{} stable-after-ms={} max-age-ms={}\ntimer=<环境根>/{TIMER_FILE}",
        doorbell_policy.reconnect_backoff.as_millis(),
        doorbell_policy.max_backoff.as_millis(),
        doorbell_policy.stable_after.as_millis(),
        doorbell_policy.max_age.as_millis()
    );
    let mut host = Host::new(HostOptions {
        environment_root: Path::new(&options.root),
        declared_agent: &declared_agent,
        material_root: Path::new(&options.materials),
        max_turns: options.max_turns,
        context_window: window,
        // 写资格的耐心：**本件的策略**，不是命令行配置（唤醒政策归主体，耐心是工程上限）。
        write_qualification: WriteQualification::default(),
        clock: clock.clone(),
        reading,
        declarations: SessionDeclarations {
            // 这一段由谁想、想多用力：**从将要发请求的那个对象取**（`ModelChoice`）——入口不另算
            // 一份，否则"主体读到的那一行"与真正用的那一个迟早对不上。
            model: Some(model.model().to_string()),
            reasoning_effort: model.reasoning_effort().map(str::to_string),
            usage: Some(usage),
            // 门铃与主体的 im 动词用的是**同一份声明**（端点与凭据都只有这一份）。
            im: Some(access),
            // 身份行（TASK_70）：**宿主声明**的两个值（core 里那个 id 与此世界的名字）。
            // 没给旗标 ⇒ `None` ⇒ 开场跳过那一刻那一行（不编、也不拿本机身份顶替）。
            identity: identity_claim(&options),
            web: web_tier,
            search,
            terminal,
            mcp,
            // 这一次装配用的模型收不收得到媒体材料（2026-10-17）：与上面 `model` 同一处口径
            // ——从**模型来源**里取，入口不另算一份；进程侧来源没有这一格 ⇒ fail-closed。
            vision: source.vision(),
            ..Default::default()
        },
        watch: watch.clone(),
    });

    // **补响**（2026-10-13）：上一段收场时世界还有没摆到眼前的动静 ⇒ 循环顶上**不等下一声铃**。
    let mut rearm = false;
    loop {
        // **先等铃**（这一步会阻塞到有人叫醒主体），**再做边界重判**，最后开一段在场。
        //
        // ⚠ **2026-10-09 改**：从前这两步的次序是反的——边界检查在循环**顶上**，而那一刻宿主
        // **正阻塞在等铃里** ⇒ 用户在页面上改完模型、**紧接着的那一段清醒期仍然用旧的**
        // （真机实测：配完第 1 条消息仍是 `credential`，第 2 条才换），而页面写着"改完不用重启"。
        // 现在检查点落在"**铃已到、还没开跑**"⇒ 改完之后的**下一段**就用新的装配。
        //
        // ⚠ **2026-10-10 在场那一刀之后，这里重判的粒度是"开新段"**：一段在场跨多声铃，
        // 而装配在 Run 内钉住（见 `composition::RunSession`）⇒ 在场**期间**改配置不会换掉
        // 正在跑的那一段。页面那句「改动只影响下一次活动：改完不用重启」照旧是**字面为真**的
        // （一段在场就是一次活动），只是"下一次活动"如今可能隔得比一声铃远。
        // 要把这件事**告诉主体**（而不是只让操作者等），归后续那一刀（「重开一段」那个动词）。
        //
        // ⚠ **2026-10-13 补响那一刀**：上一段收场时若还有"没摆到眼前的动静"（`unshown`），
        // 这里**不等铃**直接再开一段去把它看见——因为那一声铃**已经被上一段吃掉了**
        // （通道容量 1，铃满了就丢），而上一段死在读到它之前（模型失败 / 崩溃）。
        // 起源如实报 `Changed`：唤醒通道**确实报过**那次变化，只是那一次没被摆到眼前。
        // 不新造起源值——`Woken` 的取值会进 trace 与页面，那是一份**起源词表**。
        let woken = if rearm {
            Woken::Changed
        } else {
            match host.next_wake(&hub) {
                Ok(woken) => woken,
                Err(failure) => {
                    return exit_with(&failure, &options, &declared_agent, &readings_log);
                }
            }
        };
        // **唤醒边界重读**（TASK-77 的模型配置门）：铃已经到手、这一段还没开始——配置变了就用
        // 新的装配，正在跑的清醒**绝不受打扰**（重装配只发生在这里，从不发生在活动中间）。
        //
        // ⚠ **2026-10-09 改（真机实测换来的）**：从前这里只对"启动时就已经听文件"的宿主生效
        // （模式在启动时定，这里只换内容）⇒ **全新家**（启动时还没有绑定、走进程侧那一路）在
        // 页面上配好模型之后**吃不到**：绑定已经落盘、宿主照旧拿旧的那把钥匙失败，而**页面自己
        // 写着"改完不用重启"**。现在这里**重新判一次来源**。
        //
        // 两条路各自的行为（刻意不对称，理由在括号里）：
        //   · 已经听文件 ⇒ 重读同一份权威；读不出来仍**退出 5**（那是"同一份权威坏了"，
        //     绝不静默回落到进程侧——回落会让主体手里那支密钥从台面下换掉）；
        //   · 还在听进程侧 ⇒ 再判一次来源；新配置**好**就换，**读不出来就保留旧的并如实报**
        //     （不因为一次写坏的配置把一支在跑的 Agent 杀掉；下一段清醒期还会再判，改好即生效）。
        //
        // 同一份内容 ⇒ 一个字节都不动（连请求客户端都不重建）：这是常态，也是"不打扰"的读数。
        let already_file = matches!(&source, model_source::Source::File(_));
        let fresh = if already_file {
            match model_source::reread(root, list_path) {
                Ok(fresh) => Some(fresh),
                Err(message) => {
                    eprintln!("agent_host: {message}");
                    return ExitCode::from(5);
                }
            }
        } else {
            match model_source::decide(
                root,
                list_path,
                &model_flags(&options),
                &[DeepSeekModel::API_KEY_ENV, CUSTOM_MODEL_CREDENTIAL_ENV],
            ) {
                Ok(fresh) => Some(fresh),
                Err(message) => {
                    eprintln!(
                        "agent_host: 新写的模型配置没生效（保留上一次的装配；下一次唤醒还会再判）：{message}"
                    );
                    None
                }
            }
        };
        if let Some(model_source::Source::File(fresh)) = fresh {
            let unchanged =
                matches!(&source, model_source::Source::File(current) if current == &fresh);
            if !unchanged {
                match resolve_model_source(
                    model_request(&options),
                    &model_source::Source::File(fresh.clone()),
                    &reading_writer,
                    &usage_writer,
                    &shape_trace,
                    Some(&readings_writer),
                ) {
                    Ok((next_model, next_window)) => {
                        model = next_model;
                        source = model_source::Source::File(fresh);
                        // **模型那一组声明一起换**（名字 / 思考强度 / 窗口）：它们都随模型走，
                        // 分开换就会留下"名字换了、档位还是上一份"的读数。值从刚装好的那个
                        // 对象取（与启动那一处同源）。
                        host.declare_model_assembly(&model, next_window);
                        println!(
                            "\n[model]\n{}",
                            model_source::banner_lines(&source).join("\n")
                        );
                    }
                    Err((message, code)) if already_file => {
                        eprintln!("agent_host: {message}");
                        return ExitCode::from(code as u8);
                    }
                    Err((message, _)) => {
                        eprintln!(
                            "agent_host: 新写的模型配置没生效（保留上一次的装配；下一次唤醒还会再判）：{message}"
                        );
                    }
                }
            }
        }
        // **在场期间的唤醒边界**（2026-10-10 在场那一刀）：这一段跨多声铃 ⇒ 装配在 Run 内**钉住**，
        // 而页面写着「改动只影响下一次活动：改完不用重启」。那句话**字面为真**，但"下一次活动"
        // 如今可能隔得比一声铃远。
        //
        // ⇒ 这里**不换装配**（换不了，也不该换：装配在 Run 内钉住，换掉会让"主体读到的那一行"
        // 与真正用的那一个分家），只给**操作者**一行读数：声明换了，而这一段仍跑在旧的上面。
        //
        // ⚠ **"把这条事实也告诉主体"登记为欠着的**（`agent/README.md` §3.66 的"欠着的"第 1 条）：
        // 它要在主体的视野里占一块，而**开场视野链的上限是 11 层**（守卫
        // `the_opening_view_chain_has_a_hard_ceiling`：要加第 12 层，先删一层）——今天没有一层
        // 是白占的，因此**不硬塞**，等一个真实读数或等某一层腾出来。
        let mut boundary = |_: &mut ModelChoice| -> Result<(), HostFailure> {
            let already_file = matches!(&source, model_source::Source::File(_));
            let fresh = if already_file {
                // 已经听文件 ⇒ 重读同一份权威。读不出来 ⇒ **保留旧的、不出块**
                // （不因为一次写坏的配置去打扰主体；下一声铃还会再判，改好即生效）。
                model_source::reread(root, list_path).ok()
            } else {
                model_source::decide(
                    root,
                    list_path,
                    &model_flags(&options),
                    &[DeepSeekModel::API_KEY_ENV, CUSTOM_MODEL_CREDENTIAL_ENV],
                )
                .ok()
            };
            let Some(model_source::Source::File(file)) = fresh else {
                return Ok(());
            };
            // 同一份内容 ⇒ 一个字都不说（常态，也是"不打扰"的读数）。
            if matches!(&source, model_source::Source::File(current) if current == &file) {
                return Ok(());
            }
            // **给操作者一行**：读数的读者不该只有主体一个，而这一格今天只有操作者读得到
            // （主体那一侧登记为欠着的，理由见上面）。
            println!(
                "\n[model-change]\n这一段仍跑在旧的装配上（装配在 Run 内钉住）；声明已经换成 {}。",
                file.model
            );
            Ok(())
        };
        match host.run_woken_with(woken, &hub, &mut model, &mut boundary) {
            Ok(run_reading) => {
                println!("\n[run]\n{}", run_reading.reading());
                // **通道读数**（唤醒通道那一刀）：健康基线**不打印**（零动静不出行）；一旦重连过、
                // 或此刻没连着，就必须有一行——在那之前，传输级失败只发生在读手线程内部
                // （睡一会儿再试），宿主一个字都看不到，而每一次重连都换来一次完整 Run。
                //
                // ⚠ **2026-10-13 换词**：这一行从前叫 `doorbell …`。那个词是**内部代称**（一个比喻），
                // 而机制的名字是**唤醒通道**（`wake-channel`，与起源文案那一句同源）——给操作者看的
                // 这一行与进主体视野的那句从此用**同一个名字**（三类读者第 3 档：**人不许比系统
                // 知道得更少**，也不该读到第二个名字）。
                let bell_state = bell_reading.latest();
                if !bell_state.is_healthy() {
                    println!("wake-channel {}", bell_state.as_fields());
                }
                // TASK_60 裁决 5：模型调用在**传输层瞬时失败**时被立刻重采了一次——
                // 这是它留下的痕迹（**诊断面**，不是事实：不进 `facts.ndjson`）。
                // 逐条打、恰好一次（会话那边是"取走"而不是"看一眼"）。
                for retry in &run_reading.model_retries {
                    eprintln!("{}", retry.trace_line());
                }
                // TASK_60 §6：**认不出**的传输失败留下的形态指纹（同样是诊断面）。
                // 取走 ⇒ 每条恰好打一次。
                for line in shape_trace.drain() {
                    eprintln!("{line}");
                }
                // 模型失败是"主体没走完"里最需要人当场看见的一种：stdout 那行只给到
                // `system:model-failed`，这里把**分类**与活动标识一起写进 stderr
                // （`logs/host.out` 把 stdout 与 stderr 收在同一个 fd 上，所以它进同一份日志，
                // 不会被丢掉）。分类没有被记录时如实写 `unrecorded`，**不编造**。
                //
                // 这一行是**半刀**：不引入新的日志层、不动 `ModelError` 的形状、不新增词表——
                // 只把已经算出来的读数写出去。
                if let Outcome::System(SystemEnding::ModelFailed(class)) = run_reading.outcome {
                    eprintln!(
                        "model-failed run={} failure={}",
                        escape_field(run_reading.run.as_str()),
                        class.unwrap_or("unrecorded")
                    );
                }
                // **补响的判据**：这一段收场时世界还有没摆到眼前的动静 ⇒ 下一圈不等铃。
                rearm = run_reading.unshown;
            }
            Err(failure) => {
                return exit_with(&failure, &options, &declared_agent, &readings_log);
            }
        }
    }
}

/// 搜索凭据的环境变量名（只报名字，不报取值）与本仓认的那一个端点。
const SEARCH_CREDENTIAL_ENV: &str = "TAVILY_API_KEY";
const DEFAULT_SEARCH_ENDPOINT: &str = "https://api.tavily.com/search";

/// 搜索厂商的声明（**可选**）：凭据来自环境变量，端点用本仓认的那一个。
///
/// 缺凭据**不是**常驻宿主起不来的理由：web 窗是出厂内置、预装常开的，缺的只是「查」那一条
/// 的凭据——它会如实失败 `credential-absent`（**没配**，不是被拒），而「取」不依赖厂商、照常可用。
///
/// ⚠ **2026-10-13 修**：从前只有"变量在但为空"才打那行说明，而**变量压根没设**（最常见的那种）
/// **一声不响**——操作者看到的是"被拒"，而真相是这台机器没配。⇒ 两条路都打，而且**报同一个
/// 分类词**（人不许比系统知道得更少）。
fn search_access() -> Option<SearchAccess> {
    const ENDPOINT_ENV: &str = "CELIRA_SEARCH_ENDPOINT";
    let credential = std::env::var(SEARCH_CREDENTIAL_ENV).ok();
    let usable = credential
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    let Some(credential) = usable else {
        let why = if credential.is_none() {
            "环境变量未设置"
        } else {
            "环境变量为空"
        };
        eprintln!(
            "agent_host: web 搜索凭据不可用（{SEARCH_CREDENTIAL_ENV}：{why}）⇒ web_search 会如实失败 \
             credential-absent（这台机器没配，不是对面拒绝）；web_fetch 不受影响"
        );
        return None;
    };
    let endpoint = std::env::var(ENDPOINT_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_SEARCH_ENDPOINT.to_string());
    SearchAccess::new(endpoint, credential.to_string())
}

/// terminal 档位的解析（在打开环境、起任何东西之前）。
///
/// 两条都是 fail-closed：**不认识的档位**不降级；**本平台没有实现**也不换一个近似行为。
fn resolve_terminal(tier: Option<&str>) -> Result<Option<TerminalAccess>, (String, i32)> {
    let Some(text) = tier else {
        return Ok(None);
    };
    let Some(tier) = TerminalTier::parse(text) else {
        return Err((format!("未知的 terminal 档位: {text}（v1 只有 full）"), 1));
    };
    if !tier.available() {
        return Err((
            format!(
                "本平台没有 terminal 的 {} 档实现（有实现的平台：Windows、Linux）：不静默换行为，直接拒绝",
                tier.as_str()
            ),
            1,
        ));
    }
    Ok(Some(TerminalAccess::new(tier, credential_names())))
}

/// **出网档的解析**（在打开环境、起任何东西之前）：照 `resolve_terminal` 的形状。
///
/// fail-closed：**不认识的档位不降级**（`off` / `fetch` / `full` 三个词，见 `web_app::WebTier`），
/// 也不拿一个近似的档位顶上——"主体能不能出网"是主人的政策，猜错就是替主人做主。
fn resolve_web(tier: Option<&str>) -> Result<WebTier, (String, i32)> {
    let Some(text) = tier else {
        return Ok(WebTier::default());
    };
    let Some(tier) = WebTier::parse(text) else {
        return Err((
            format!("未知的 web 档位: {text}（v1 只有 off / fetch / full）"),
            1,
        ));
    };
    Ok(tier)
}

/// 要洗掉的凭据键名（**只报名字**，永不取值）：这台机器上这个进程认得的全部凭据。
/// 系统自有的 `CELIRA_` 前缀那族由件自己按前缀洗，这里列的是另外几个。
fn credential_names() -> Vec<String> {
    vec![
        "DEEPSEEK_API_KEY".to_string(),
        "TAVILY_API_KEY".to_string(),
        "CELIRA_IM_BACKEND_AUTH".to_string(),
    ]
}

/// 外部来源的 App 的**授权面**解析（在打开环境、起任何东西之前）。
///
/// 三条都是 fail-closed：声明读不懂 ⇒ 拒绝启动（不猜）；两台同名 ⇒ 拒绝启动（动词前缀会撞，
/// 路由到谁都没有正确答案）；本平台没有 stdio 通道的实现 ⇒ 拒绝启动（不换一个近似行为）。
fn resolve_mcp(declarations: &[String]) -> Result<Vec<McpAccess>, (String, i32)> {
    if declarations.is_empty() {
        return Ok(Vec::new());
    }
    if !stdio_available() {
        return Err((
            "本平台没有 stdio MCP 通道的实现（有实现的平台：Windows、Linux）：不静默换行为，直接拒绝"
                .to_string(),
            1,
        ));
    }
    let mut accesses: Vec<McpAccess> = Vec::new();
    for text in declarations {
        let declaration = parse_declaration(text)
            .map_err(|message| (format!("agent_host: --mcp 声明读不懂（{message}）"), 1))?;
        if accesses
            .iter()
            .any(|each| each.declaration().name() == declaration.name())
        {
            return Err((
                format!(
                    "agent_host: --mcp 里有两台同名的 server：{}",
                    declaration.name()
                ),
                1,
            ));
        }
        accesses.push(McpAccess::new(declaration, credential_names()));
    }
    Ok(accesses)
}

/// 窗口声明的**语法**校验（值本身要等档位才知道，见 [`resolve_model_source`]）。
fn parse_context_window(text: &str) -> Result<u32, String> {
    let window: u32 = text
        .parse()
        .map_err(|_| format!("--context-window 不是整数: {text}"))?;
    if window == 0 {
        return Err("--context-window 必须是正整数（0 不是一条可解释的声明）".to_string());
    }
    Ok(window)
}

/// 模型门要的**进程侧声明**（旗标那四件，原始取值）。
///
/// 一处构造、两处使用（启动判定 + 唤醒边界重读）：口径只有一份，免得哪天只改了一边。
fn model_flags(options: &Options) -> model_source::FlagDeclarations<'_> {
    model_source::FlagDeclarations {
        endpoint: options.model_endpoint.as_deref(),
        model: options.model.as_deref(),
        context_window: options.context_window.as_deref(),
        protocol: options.model_protocol.as_deref(),
    }
}

/// 模型源的三件**原始**声明（入口读完环境变量之后交给 [`resolve_model_source`]）。
struct ModelSourceRequest {
    model: Option<String>,
    endpoint: Option<String>,
    context_window: Option<String>,
    /// 线协议（原始声明；缺省 = 本家那一条）。
    protocol: Option<String>,
}

/// 从入口参数拼出 [`ModelSourceRequest`]（同上：一处构造，启动与重读共用）。
fn model_request(options: &Options) -> ModelSourceRequest {
    ModelSourceRequest {
        model: options.model.clone(),
        endpoint: options.model_endpoint.clone(),
        context_window: options.context_window.clone(),
        protocol: options.model_protocol.clone(),
    }
}

/// 解析并装配模型（**三档 × 两条线协议**），并给出这一档的标称窗口。
///
/// - **默认档**（没给 `--model-endpoint`，也没有配置文件）：零迁移。凭据 = `DEEPSEEK_API_KEY`；
///   端点与模型名由适配层拥有（`--model` 可以单独切本家的另一个档位）；窗口 = `--context-window`
///   或适配层的标称值。**线协议只能是本家那一条**：默认档没有第二个端点，给别的协议是配对错误
///   （exit 5），不猜。
/// - **换家档**（给了端点）：**四件全显式**——端点 / 凭据（`CELIRA_MODEL_API_KEY`）/ 模型名 /
///   标称窗口，缺一件即失败。**第五件**（TASK_44）是线协议：缺省仍是本家那一条，
///   `--model-protocol responses` 于是选到第二个实现。
/// - **文件档**（TASK_77：环境根下的私有配置文件在场）：形状与换家档同一套（四件 + 可选协议），
///   只是四件来自文件而不是命令行/环境。它与另两档**互斥**——`decide` 已经在入口处拒绝
///   "文件在场而进程侧也在说话"，因此这里看到 `Source::File` 就只认文件。
///
/// **为什么换家档必须重声明窗口**：适配层的缺省窗口是**默认档那个模型**的声明，别家模型用它等于说谎。
/// 这不是新发明——适配层早就写着"它是声明不是实测：换模型档位必须重声明"。
///
/// **一次调用 = 同次快照**；失败消息只报**变量名与旗标名**，永不报取值。
/// 屏幕 cell 的写柄由这里装上去（唯一写者），因此返回值是一个已经装好传感器的模型。
fn resolve_model_source(
    request: ModelSourceRequest,
    source: &model_source::Source,
    reading: &ContextReadingWriter,
    usage: &UsageWriter,
    shape_trace: &ShapeTrace,
    readings: Option<&ModelReadingsWriter>,
) -> Result<(ModelChoice, u32), (String, i32)> {
    const CUSTOM_CREDENTIAL_ENV: &str = CUSTOM_MODEL_CREDENTIAL_ENV;
    // 协议名取自两侧适配层的**公开常量**（不在这里抄字面量：两个名字各只有一处权威）。
    // 文件为源时"第五件"来自文件里那一件；缺省仍是本家那一条。
    let declared_protocol = match source {
        model_source::Source::File(file) => file.protocol.clone(),
        model_source::Source::Process => request.protocol.clone(),
    };
    let protocol = declared_protocol.unwrap_or_else(|| provider_adapter::PROTOCOL.to_string());
    if protocol != provider_adapter::PROTOCOL && protocol != responses_adapter::PROTOCOL {
        // 报**是哪一件**不认识：文件里那一件与命令行那个旗标不是同一个名字。
        let declared_as = match source {
            model_source::Source::File(_) => "模型配置文件里的 protocol",
            model_source::Source::Process => "--model-protocol",
        };
        return Err((
            format!(
                "{declared_as} 不认识: {protocol}（只有 {} 与 {}）",
                provider_adapter::PROTOCOL,
                responses_adapter::PROTOCOL
            ),
            5,
        ));
    }
    // **思考强度只在 chat-completions 那一档有去处**（2026-10-08）：`reasoning_effort` 是那条 wire
    // 请求体上的键，responses 那一条没有这一格。声明了档位却选了那一条 ⇒ **配对错误，明确拒绝**：
    // 静默丢掉它会让操作者以为"这一档真的生效了"，而请求里根本没有这个键——那正是"一格在撒谎"。
    // 拒绝排在装配（与任何网络调用）之前，错误里点名是哪一件、并给出两条出路。
    let declared_effort = match source {
        model_source::Source::File(file) => file.reasoning_effort.clone(),
        model_source::Source::Process => None,
    };
    if let Some(effort) = &declared_effort
        && protocol != provider_adapter::PROTOCOL
    {
        let declared_as = match source {
            model_source::Source::File(_) => "模型配置文件里的 reasoning_effort",
            model_source::Source::Process => "--reasoning-effort",
        };
        return Err((
            format!(
                "{declared_as} 声明了 `{effort}`，但线协议 {protocol} 不认这一格：\
                 `reasoning_effort` 是 {} 那一档请求体上的键。\
                 要么去掉这一格，要么把 protocol 换回 {}。",
                provider_adapter::PROTOCOL,
                provider_adapter::PROTOCOL
            ),
            5,
        ));
    }
    let build = |credential: String,
                 model: String,
                 endpoint: String|
     -> Result<ModelChoice, (String, i32)> {
        let failure = |_| ("模型装配失败（请求客户端建不起来）".to_string(), 5);
        let built = if protocol == responses_adapter::PROTOCOL {
            ModelChoice::Responses(
                ResponsesModel::at_endpoint(credential, model, endpoint)
                    .map_err(failure)?
                    .with_reading(reading.clone())
                    .with_usage(usage.clone())
                    .with_shape_trace(shape_trace.clone()),
            )
        } else {
            ModelChoice::Chat(
                DeepSeekModel::at_endpoint(credential, model, endpoint)
                    .map_err(failure)?
                    // **这一档真的发出去**：值由清单那一条给（唯一读侧已按封闭表校验过），
                    // 本层只搬运；`None` = 请求体里没有这个键（逐字节就是今天的样子）。
                    .with_reasoning_effort(declared_effort.clone())
                    .with_reading(reading.clone())
                    .with_usage(usage.clone())
                    .with_shape_trace(shape_trace.clone()),
            )
        };
        // 逐轮读数账：**两条 wire 共同的一件事**，因此装在选完之后那一处（`ModelChoice` 转发），
        // 而不是在两个分支里各写一遍。没装 = 这一台机器不记这份账，绝不伪造行。
        Ok(match readings {
            Some(writer) => built.with_model_readings(writer.clone()),
            None => built,
        })
    };
    // ---- 第三条来源：环境根的私有配置文件（换家档形状；四件与协议都来自文件）----
    //
    // 它排在两档之前，是因为文件在场时进程侧必须是沉默的（`decide` 已经拒绝了两边同时说话）：
    // 走到这里还带着端点旗标是不可能的，所以这个分支不会被"顺手"走到。
    if let model_source::Source::File(file) = source {
        let built = build(
            file.credential.clone(),
            file.model.clone(),
            file.endpoint.clone(),
        )?;
        return Ok((built, file.context_window));
    }
    match &request.endpoint {
        None => {
            if protocol != provider_adapter::PROTOCOL {
                return Err((
                    format!(
                        "--model-protocol {protocol} 只在换家档有效：默认档是本家的 {} 端点，\
                         它没有第二种线协议。要换家就给一个 --model-endpoint。",
                        provider_adapter::PROTOCOL
                    ),
                    5,
                ));
            }
            let credential = std::env::var(DeepSeekModel::API_KEY_ENV).ok();
            let credential = match credential {
                Some(value) if !value.trim().is_empty() => value,
                _ => {
                    return Err((
                        format!(
                            "模型凭据不可用（环境变量 {} 缺失或为空）",
                            DeepSeekModel::API_KEY_ENV
                        ),
                        5,
                    ));
                }
            };
            let model = request
                .model
                .clone()
                .unwrap_or_else(|| DEFAULT_MODEL.to_string());
            let window = match &request.context_window {
                Some(raw) => parse_context_window(raw).map_err(|message| (message, 1))?,
                None => nominal_context_window(),
            };
            let built = build(credential, model, ENDPOINT.to_string())?;
            Ok((built, window))
        }
        Some(endpoint) => {
            let credential = std::env::var(CUSTOM_CREDENTIAL_ENV).ok();
            let credential = match credential {
                Some(value) if !value.trim().is_empty() => value,
                _ => {
                    return Err((
                        format!(
                            "换家档的凭据不可用: 请设置 {CUSTOM_CREDENTIAL_ENV}（缺失或为空）。\
                             本命令不打印它的取值。"
                        ),
                        5,
                    ));
                }
            };
            let Some(model) = request.model.clone() else {
                return Err((
                    format!(
                        "换家档必须显式给出模型名（--model）：{DEFAULT_MODEL} 只在默认档有意义，本命令不猜。"
                    ),
                    5,
                ));
            };
            let Some(raw_window) = request.context_window.clone() else {
                return Err((
                    "换家档必须显式声明标称窗口（--context-window）：缺省值属于默认档那个模型，\
                     别家模型用它等于说谎。"
                        .to_string(),
                    5,
                ));
            };
            let window = parse_context_window(&raw_window).map_err(|message| (message, 1))?;
            let built = build(credential, model, endpoint.clone())?;
            Ok((built, window))
        }
    }
}

/// 退出读数：一行原因 + **恰好一行** next。它不代敲命令、不自动重试、不声称环境已恢复。
fn exit_with(
    failure: &HostFailure,
    options: &Options,
    agent: &AgentId,
    readings_log: &ModelReadings,
) -> ExitCode {
    println!("\n[exit]\nreason={}", failure.as_str());
    match failure {
        // 环境不可用：把**哪一类**带出来（不含配置细节）。
        HostFailure::Environment(detail) => println!("detail={}", escape_field(detail)),
        // 写资格被握着：带出**等了多久**。它是这条读数里唯一可量化的东西，
        // 也是"里面那个持有者是不是活得太久"的判据（一次读十几秒是正常，几分钟就不是）。
        HostFailure::EnvironmentBusy { waited_ms } => println!("waited_ms={waited_ms}"),
        _ => {}
    }
    // 逐轮读数账坏过没有：**收尾这一处说清**（库不打印：打印是入口的事）。一次都没坏过
    // 就一个字都不说；坏过就说清是哪一类，以及后果——后面那几轮没有落盘。
    if let Some(error) = readings_log.first_failure() {
        println!("readings_log=write-failed class={}", error.as_str());
    }
    println!(
        "\n[next]\n{}",
        failure.next_line(Path::new(&options.root), agent)
    );
    ExitCode::from(failure.code() as u8)
}

/// 入口参数的解析用例（理由同 `agent_run` 的那一段：命令行接线没有别的地方能覆盖，
/// 而"某个旗标跨错了一步"这种错会**全绿通过**，直到真活验收才暴露）。
#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(list: &[&str]) -> Vec<String> {
        list.iter().map(|each| (*each).to_string()).collect()
    }

    /// `--mcp` 消费它的取值，而且可以重复（每台 server 一条）。
    #[test]
    fn the_mcp_flag_takes_its_value_and_can_repeat() {
        let mut argv = arguments(&[
            "--root",
            "env",
            "--agent",
            "a",
            "--materials",
            "materials",
            "--max-turns",
            "3",
            "--im-endpoint",
            "http://127.0.0.1:1",
        ]);
        argv.extend(arguments(&[
            "--mcp",
            "one=stdio:cmd1",
            "--mcp",
            "two=stdio:cmd2 --flag",
        ]));

        let options = parse(&argv)
            .expect("这条命令行应当能解析")
            .expect("不是 --help");
        assert_eq!(
            options.mcp,
            vec![
                "one=stdio:cmd1".to_string(),
                "two=stdio:cmd2 --flag".to_string()
            ],
            "声明要原样收下（逗号与空格都是命令的一部分）"
        );
    }
}
