//! 手动运行命令：显式初始化一个身份环境，或显式跑**一次**有限活动。
//!
//! 它只做接线与呈现：参数解析、装配真实模型、把结果打到 stdout 并用退出码区分结局。
//! 执行链、身份、准入、资料根边界、确认链全部沿用既有实现——本文件里没有第二条模型-行动
//! 循环，也不重新建立任何事实权威。
//!
//! **这不是聊天界面**：当前模型决策只有"行动/收场"两种选择，没有自由文本回复协议，
//! 因此本命令不会打印任何合成出来的助手答复；它只报告机械收场与有限读数。
//!
//! ```text
//! agent_run init --root <环境根> --agent <AgentId>
//! agent_run run  --root <环境根> --agent <AgentId> --materials <授权资料根> --run <RunId>
//!                --max-turns <正整数> [--input <文本> | --input-stdin] [--host-input]
//! agent_run --help
//! ```
//!
//! 命令行形状的**唯一权威是本文件的 `USAGE`**：上面那块只列最常用的两条形状，而逐项的说明
//! （含后加的那几格、各自的必答件与互斥关系）全在 `USAGE` 里——两处不一致时以 `USAGE` 为准。
//!
//! 顺序：参数与输入先判定 → 既有 `open`（身份、写锁、损坏都在这一步决定）→ 装配模型
//! （读凭据，**不联网**）→ 既有 `run_activity_in_environment`（执行准入、资料根装配与确认链
//! 都在它内部）→ 第一次模型调用。因此"入口自己能判定的错"排在最前、环境的拒绝排在装配模型
//! 之前（缺失凭据不会把"环境打不开"顶掉），而任何一次网络调用与动作执行都发生在身份、准入
//! 与资料根边界**之后**。
//!
//! 退出码（与 `celira_agent::manual_entry` 共用同一张表，不在这里另编）：
//!
//! | 码 | 含义 |
//! |---|---|
//! | 0 | 主体自己收场（**不是**任务完成证明） |
//! | 1 | 入口拒绝：参数不合法，或输入超上限 / 非法编码 / Turn 上限非正 |
//! | 2 | 环境不可用（不存在、身份不匹配、已有写者、身份或存储损坏） |
//! | 3 | **保留不用**：准入那两道闸（已记录暂停、没有收尾的活动）都已退役；
//!       与 `celira_agent::manual_entry` 共用同一张表，今天没有结论落在这个码上 |
//! | 4 | 启动被拒（资料根不可用或与环境内部记录重叠、装配冲突、活动层面拒绝） |
//! | 5 | 服务凭据不可用（模型服务的凭据缺失或为空，或 im 的端点与凭据只给了一半；
//!       只报告变量名，不报告取值） |
//! | 6 | 保存未完整确认（带确认点；不声称成功、不声称记录存在或不存在） |
//! | 7 | 系统终止（模型失败、结果无法确认、Turn 用尽、取消、崩溃） |
//! | 9 | 停在等待：活动**没有**结束，环境里留下一个未收尾的活动 |

use std::path::{Path, PathBuf};

use celira_agent::clock::ClockHandle;
use celira_agent::composition::SessionDeclarations;
use celira_agent::context_reading::{ContextReading, ContextReadingWriter};
use celira_agent::cost::{UsageReport, UsageWriter};
use celira_agent::creation_grant::CreationGrant;
use celira_agent::foundation::{AgentId, RunId};
use celira_agent::im_channel::ImAccess;
use celira_agent::manual_entry::{
    EXIT_MODEL_ASSEMBLY_FAILED, ManualInput, ManualRun, create_environment, describe_error,
    open_environment, render, render_created, run_host_session_in, run_manual_activity_in,
};
use celira_agent::mcp::{McpAccess, parse_declaration, stdio_available};
use celira_agent::model_choice::ModelChoice;
use celira_agent::model_readings::{ModelReadings, ModelReadingsWriter};
use celira_agent::model_source;
use celira_agent::provider_adapter::{
    self, DEFAULT_MODEL, DeepSeekModel, ENDPOINT, nominal_context_window,
};
use celira_agent::responses_adapter::{self, ResponsesModel};
use celira_agent::self_concept::IdentityClaim;
use celira_agent::terminal_app::{TerminalAccess, TerminalTier};
use celira_agent::web_app::{SearchAccess, WebTier};

/// 换家档的模型凭据环境变量（**唯一权威在这里**：装配那一处用它取值，模型门的冲突判据用它
/// 认"进程侧在不在说话"）。只报变量名，不报取值。
const CUSTOM_MODEL_CREDENTIAL_ENV: &str = "CELIRA_MODEL_API_KEY";

const USAGE: &str = "\
用法:
  agent_run init   --root <环境根> --agent <AgentId>
  agent_run run    --root <环境根> --agent <AgentId> --materials <授权资料根> --run <RunId>
                   --max-turns <正整数> [--input <文本> | --input-stdin] [--host-input]
                   [--create-dir <资料根内的单个目录名>]
                   [--context-window <正整数>] [--im-endpoint <URL>]
                   [--identity-agent <core 里的 id> --identity-name <此世界的名字>]
                   [--search-endpoint <URL>] [--terminal <tier>]
                   [--mcp <声明>]
                   [--model <id>] [--model-endpoint <URL>]
                   [--model-protocol <responses|chat-completions>]
  agent_run --help

说明:
  init   显式初始化一个身份环境。只调用既有 create：不装配模型（因此也不读凭据）、
         不创建资料目录。
         环境已存在时按既有语义拒绝，不覆盖、不重置。
  run    在已经存在的环境里跑一次有限活动；环境不存在时不会自动初始化。
         --materials 是显式授权给 Agent 的资料根，不会被默认为当前工作目录。
         --run 由调用方给出：本版不自动编号。
         不给 --input / --input-stdin 就是空输入（合法活动，不是漏了参数）；
         --input-stdin 只在显式指定时读取标准输入。
         --host-input 显式启用同进程宿主输入循环：装配一次会话、驱动多次，标准输入按行
         读三种明确的宿主动作——一行非空文本 = 显式输入（下一次驱动带上它），一个空行 =
         显式继续（不给新内容），流结束 = 显式退出。等待之后不自动再驱动一次；
         退出时活动没有结束，环境里留下一个未收尾的活动（报告会说出来）。
         它与 --input-stdin 互斥：开场输入会把标准输入整条读空。
         --create-dir 显式授予本次活动在 <资料根>/<该目录> 里的写权限：新建 UTF-8 文本，
         以及把该目录（含它下面的任意深度）已有文件里的一段文本唯一替换掉。
         这个目录必须已经存在，而且只能是一个目录名（不含分隔符、不是根、不是 . 或 ..）。
         不给就是默认只读；授权只对这一次 Run 有效，旧参数与旧入口不因此获得写权限。
         两种写各只做一件事：新建就是新建（同名已存在就明确失败），改就是唯一替换
         （那段文本出现 0 次或多次都明确失败）；两者都不改名、不删除文件、不追加内容。
         笔记不需要任何授权（写在环境根里）：`notes_create` 随时可用，
         笔记正文只存在笔记存储里，不进入事实，也不自动进入下一次活动。
         --context-window 声明本次活动的上下文窗口（正整数）：状态行里 `context` 那一段的
         分母，也就是「屏幕有多大」。厂商不回传窗口，因此只能由宿主声明；不给时取适配层当前
         模型档位的公开标称值。`0` 不是合法声明（直接拒绝，不猜）。
         --im-endpoint 声明 Celira IM 的端点（形如 http://主机:端口）：给了它，这次就多了
         im 的动词（看自己参与的房间、翻房内记录、在房里说一句话、以及复核那句话）。
         它与环境变量 CELIRA_IM_BACKEND_AUTH 是一对：必须成对给——只给一半是入口拒绝，
         不静默降级。不给端点就是『这次没有 im app』（这次的能力面里不出现它）。凭据的具体
         取值不打印、不落盘。
         --mcp 装一台外部来源的 App（MCP server），可重复（一台一条）。形状：
         `<名>=stdio:<命令>[,read-only][,timeout=<秒>]`。不给就是没装：子进程不起、清单不拉，
         直到 `open_app` 那一下才真的连上——装了不用一分钱不花。
         `<名>` 是动词前缀（动词叫 `<名>_<工具名>`，ASCII 字母开头）；`,read-only` 是宿主
         显式背书「这台不改外面的世界」，不给就是最严那档（unverifiable-write：那件写
         没有同参复核可配）。装/卸要重启本进程——授权面是装配期事实，
         不在 Run 中途变。

环境变量:
  --model <id>         模型名。默认档下可以单独切（例如换成本家另一个档位）；
                       换家档下它是必答件之一。
  --model-endpoint     声明模型端点（OpenAI 兼容的 chat completions）。一给就进换家档：
                       端点 / 凭据 / 模型名 / 标称窗口四件全显式，缺一件即退出码 5。
  --model-protocol     声明线协议：`chat-completions`（缺省，现状零迁移）或 `responses`。
                       它是换家档的第五件——`responses` 只在给了 `--model-endpoint` 时有效
                       （默认档是本家的那一个端点，没有第二种协议），否则退出码 5，不猜。
                       给了 `responses` 时端点值就是完整地址（例：`…/v1/responses`）：
                       本层不替调用方拼路径，与 chat 档同一口径。
  --model-sources      声明模型源清单的路径（刀2b）：清单归机器（这台上有哪些钥匙），
                       用哪一条由环境根下的 `model_binding.json` 说（绑定归 Agent）。
                       绑定了却不说清单在哪 ⇒ 退出码 5，不猜。

  DEEPSEEK_API_KEY         默认档的凭据（不给 --model-endpoint 时读取）：缺失或为空即退出码 5，
                           取值不写入任何输出。
  CELIRA_MODEL_API_KEY     换家档的凭据（给了 --model-endpoint 时读取）：与端点同一次解析、
                           成对出现；缺失或为空即退出码 5。取值同样不写入任何输出。

  模型配置门（清单归机器，绑定归 Agent）：这台机器有哪些钥匙写在 `<家>/model_sources.json`，
  这一个 Agent 用哪一条写在环境根下的 `model_binding.json`；清单在哪由宿主用
  `--model-sources <清单路径>` 指出（`node home.js up` 会替你给）。绑定在场 ⇒ 四件（端点 / 密钥 /
  模型名 / 标称窗口，+ 可选线协议）从清单那一条解析，进程侧再声明任何一个模型来源 ⇒ 拒绝
  （说清哪两个打架）。本命令是一次性的：每次跑都重读绑定与清单，下一次活动自然吃到新配置。
  没有绑定时退回老形状：`<环境根>/model_source.json`（四件套）在场即以它为准，它同样与进程侧互斥。
  CELIRA_IM_BACKEND_AUTH
                     给了 --im-endpoint 时必须同时给出：im 的 BackendAuth 凭据（只进请求头，
                     不打印、不落盘、不进视野；缺失或为空即退出码 5，只报变量名）。
";

fn main() {
    std::process::exit(run(std::env::args().skip(1).collect()));
}

fn run(arguments: Vec<String>) -> i32 {
    // 帮助在任何路径之前：它只打印用法就返回，因此不会读凭据、不打开环境、不创建任何东西。
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        print!("{USAGE}");
        return 0;
    }

    let invocation = match parse(&arguments) {
        Ok(invocation) => invocation,
        Err(message) => {
            eprintln!("{message}");
            eprintln!();
            eprint!("{USAGE}");
            return 1;
        }
    };

    match invocation {
        Invocation::Init { root, agent } => init(&root, &agent),
        Invocation::Run {
            request,
            host_input,
            reading,
            usage,
            im_endpoint,
            search_endpoint,
            terminal_tier,
            web_tier,
            mcp,
            model,
            model_endpoint,
            model_protocol,
            model_sources,
            context_window,
        } => start(
            *request,
            host_input,
            reading,
            usage,
            Declarations {
                im_endpoint,
                search_endpoint,
                terminal_tier,
                web_tier,
                mcp,
                model_source: ModelSourceRequest {
                    model,
                    endpoint: model_endpoint,
                    context_window,
                    protocol: model_protocol,
                },
                model_sources,
            },
        ),
    }
}

/// 宿主在命令行上声明的**装配面**：每个 App 在场的条件 + 模型源。
///
/// 它们一起从 `Invocation` 里端出来、一起被解析（`resolve_*` 那一族），因此绑成一个结构，
/// 而不是一路铺成位置参数——铺开已经到八个，再加一个就没人记得住顺序了。
struct Declarations {
    /// 宿主声明的 im 端点（与凭据成对；只给一半是入口拒绝）。
    im_endpoint: Option<String>,
    /// 宿主声明的搜索厂商端点（不给就用本仓认的那一个）。
    search_endpoint: Option<String>,
    /// 宿主声明的 terminal 档位（不给就没有这只手）。
    terminal_tier: Option<String>,
    /// **出网档**（`--web off|fetch|full`；缺省 full）。
    web_tier: Option<WebTier>,
    /// 宿主声明的外部来源 App（**原始声明**，可重复；空 = 一台都没装）。
    mcp: Vec<String>,
    /// 模型源的三件原始声明。
    model_source: ModelSourceRequest,
    /// **模型源清单**的路径（刀2b：宿主显式声明；用清单哪一条看环境根的绑定文件）。
    model_sources: Option<String>,
}

/// 初始化：只调用既有 create，并把环境里**实际记录**的身份报出来。
fn init(root: &Path, agent: &AgentId) -> i32 {
    match create_environment(root, agent) {
        Ok(recorded) => {
            print!("{}", render_created(root, agent, &recorded));
            0
        }
        Err(error) => {
            eprintln!(
                "初始化被拒绝: {} ({})",
                error.as_str(),
                describe_error(&error)
            );
            error.exit_code() as i32
        }
    }
}

/// 跑一次活动：参数与输入 → 打开环境 → 装配模型 → 交给共用的核心启动路径。
///
/// 顺序有意如此：**入口能自己判定的错**（参数、输入、Turn 上限）排在最前，环境层面的拒绝
/// 排在装配模型之前，因此"参数写错了"与"环境打不开"都不会被一个缺失凭据顶掉；而准入与
/// 资料根这些边界仍在核心路径里判定，排在第一次网络调用与任何动作之前。
fn start(
    mut request: ManualRun,
    host_input: bool,
    reading: ContextReadingWriter,
    usage: UsageWriter,
    declarations: Declarations,
) -> i32 {
    let Declarations {
        im_endpoint,
        search_endpoint,
        terminal_tier,
        web_tier,
        mcp,
        model_source,
        model_sources,
    } = declarations;
    if let Err(error) = request.check() {
        eprintln!("入口拒绝: {} ({})", error.as_str(), describe_error(&error));
        return error.exit_code() as i32;
    }

    // im 的端点与凭据必须成对：缺一半是**入口拒绝**（退出码与模型凭据同族），不静默降级。
    // 搜索凭据是**可选**的：没有它那不是入口错误——web 窗照旧在场，只有「查」这一条会如实
    // 说没有凭据。要报的是变量名，永远不报取值。
    request.declarations.search = resolve_search(&search_endpoint);
    // **出网档**（2026-10-13）：`--web off|fetch|full`，缺省 full（不改现有行为）。
    // 与 terminal 同形：不认识的档位不降级，直接拒绝。
    request.declarations.web = web_tier.unwrap_or_default();
    // terminal 的档位是**可选**的：不给就没有这只手；给了但档位不认识、或本平台没有实现，
    // 都在这里直接拒绝（fail-closed，不静默降档、不静默换行为）。
    request.declarations.terminal = match resolve_terminal(&terminal_tier) {
        Ok(terminal) => terminal,
        Err((message, code)) => {
            eprintln!("{message}");
            return code;
        }
    };
    // 外部来源的 App 的**授权面**：声明读不懂 / 两台同名 / 本平台没有通道实现 —— 都在这里
    // 直接拒绝（fail-closed，不猜、不静默换行为）。**可重复**，一台 server 一条。
    request.declarations.mcp = match resolve_mcp(&mcp) {
        Ok(mcp) => mcp,
        Err((message, code)) => {
            eprintln!("{message}");
            return code;
        }
    };

    request.declarations.im = match resolve_im(&im_endpoint) {
        Ok(im) => im,
        Err((message, code)) => {
            eprintln!("{message}");
            return code;
        }
    };

    // 宿主输入循环的开场输入先判定：来源冲突排在打开环境、装配模型之前被报出，
    // 否则一个"参数冲突"会被"凭据缺失"顶掉，用户拿到的结论就是错的。
    let initial = if host_input {
        match request.host_loop_initial_input() {
            Ok(input) => Some(input),
            Err(error) => {
                eprintln!("入口拒绝: {} ({})", error.as_str(), describe_error(&error));
                return error.exit_code() as i32;
            }
        }
    } else {
        None
    };

    let mut stdin = std::io::stdin().lock();
    let input = match initial {
        Some(input) => input,
        None => match request.resolve_input(&mut stdin) {
            Ok(input) => input,
            Err(error) => {
                eprintln!("入口拒绝: {} ({})", error.as_str(), describe_error(&error));
                return error.exit_code() as i32;
            }
        },
    };

    let mut environment = match open_environment(&request.environment_root, &request.expected_agent)
    {
        Ok(environment) => environment,
        Err(error) => {
            eprintln!("环境拒绝: {} ({})", error.as_str(), describe_error(&error));
            return error.exit_code() as i32;
        }
    };

    // 模型源：**三档 × 两条线协议**（默认零迁移 / 换家五件套 / 环境根配置文件）。窗口与模型在同
    // 一次解析里定下来——换家档要求窗口显式声明，因此它不能像从前那样在 `parse` 里就定死成数字。
    // 屏幕 cell 的写柄也交进去：解析要按协议选实现，而"装上传感器"是两条协议共同的收尾。
    //
    // TASK_77：**文件源的判定与常驻宿主同一处逻辑**（`model_source` 模块）——一次活动只有一次
    // 装配，它必须与常驻宿主同一条口径，"听谁的"不能有两套规则。
    //
    // 刀2b：**清单归机器，绑定归 Agent**。宿主把清单路径交进来（`--model-sources`），
    // "这一个 Agent 用哪一条"由环境根的绑定文件说；清单说"这台机器上有哪些钥匙"。
    let list_path = model_sources.as_deref().map(Path::new);
    let source = match model_source::decide(
        &request.environment_root,
        list_path,
        &model_flags(&model_source),
        &[DeepSeekModel::API_KEY_ENV, CUSTOM_MODEL_CREDENTIAL_ENV],
    ) {
        Ok(source) => source,
        Err(message) => {
            eprintln!("{message}");
            return EXIT_MODEL_ASSEMBLY_FAILED as i32;
        }
    };
    // 逐轮读数账：与两个 cell 同一条纪律（写柄交适配层、唯一写者是形状），但**落点是文件**——
    // 它服务的是**事后翻文件的操作者**，不是任何执行路径。
    //
    // ⚠ **构造不碰文件系统**：一次没写过，盘上就一份空的都没有（"没有凭据的时刻不得改动环境"
    // 是一条写死的纪律，用例 `the_creation_flag_is_explicit_and_never_implicit` 钉着它）。
    // "打不开"因此**发现得晚**，但照旧被记下来、在收尾时报出。
    let (readings_writer, readings_log) = ModelReadings::channel(&request.environment_root);
    let (mut model, _window) = match resolve_model_source(
        model_source,
        &source,
        &reading,
        &usage,
        Some(&readings_writer),
    ) {
        Ok(resolved) => resolved,
        Err((message, code)) => {
            eprintln!("{message}");
            return code;
        }
    };

    // 这一段由谁想、想多用力：**从将要发请求的那个对象取**（`ModelChoice`）——装配入口不另算一份，
    // 否则"主体读到的那一行"与真正用的那一个迟早对不上。两格都要等解析之后才有，因此在这里补进声明。
    request.declarations.model = Some(model.model().to_string());
    request.declarations.reasoning_effort = model.reasoning_effort().map(str::to_string);
    // 这一次装配用的模型收不收得到媒体材料（2026-10-17）：与上面那两格**同一处口径、同一时机**
    // ——从模型来源取（`Source::vision`），入口不另算一份。
    request.declarations.vision = source.vision();

    // 两条路共用同一段启动代码，区别只在"驱动几次"：一次性，还是由宿主显式继续。
    let result = if host_input {
        run_host_session_in(&mut environment, &request, input, &mut stdin, &mut model)
    } else {
        run_manual_activity_in(&mut environment, &request, input, &mut model)
    };

    match result {
        Ok(ending) => {
            print!("{}", render(&request, &ending));
            report_readings_failure(&readings_log);
            ending.exit_code() as i32
        }
        Err(error) => {
            eprintln!("入口拒绝: {} ({})", error.as_str(), describe_error(&error));
            report_readings_failure(&readings_log);
            error.exit_code() as i32
        }
    }
}

/// 收尾时把**逐轮读数账坏过没有**报出来（库不打印：打印是入口的事）。
///
/// 一次都没坏过 ⇒ 一个字都不说（它不是每次都要报的读数）；坏过 ⇒ 说清**是哪一份**、哪一类、
/// 什么形状，并说清它的后果——**后面那几轮没有落盘**。
fn report_readings_failure(log: &ModelReadings) {
    let Some(error) = log.first_failure() else {
        return;
    };
    eprintln!(
        "逐轮读数账写不进（{error}）：{}；从那一轮起不再有行落盘",
        log.path().display()
    );
}

/// 命令行解析结果。
enum Invocation {
    Init {
        root: PathBuf,
        agent: AgentId,
    },
    /// 装进 `Box`：`ManualRun` 比另一个变体大得多，留在同一个枚举里会让每个 `Init` 也占那么大。
    Run {
        request: Box<ManualRun>,
        /// 这次运行要不要走**宿主输入循环**（显式输入 / 显式继续 / 显式退出）。
        host_input: bool,
        /// 屏幕报告 cell 的写柄：交给适配层，只有它写。
        reading: ContextReadingWriter,
        /// **用量 cell 的写柄**（TASK_78 刀三）：与屏幕同款，交给适配层，只有它写。
        usage: UsageWriter,
        /// 宿主声明的 im 端点（原始取值）：凭据要等入口读环境变量，因此**在入口里**成对解析。
        im_endpoint: Option<String>,
        /// 宿主声明的搜索厂商端点（原始取值；不给就用本仓认的那一个）。
        search_endpoint: Option<String>,
        /// 宿主声明的 terminal 档位（原始取值；不给就没有这只手）。
        terminal_tier: Option<String>,
        /// **出网档**（`--web off|fetch|full`；缺省 full）。
        web_tier: Option<WebTier>,
        /// 宿主声明的外部来源 App（**原始声明**，可重复；空 = 一台都没装）。
        mcp: Vec<String>,
        /// 宿主声明的模型名（原始取值；默认档可单独切，换家档必答）。
        model: Option<String>,
        /// 宿主声明的模型端点（原始取值；**一给就进换家档**）。
        model_endpoint: Option<String>,
        /// 宿主声明的线协议（原始取值；缺省 = 本家那一条）。
        model_protocol: Option<String>,
        /// 宿主声明的**模型源清单**路径（刀2b）：清单归机器（这台上有哪些钥匙），
        /// 用哪一条由环境根的绑定文件说。不给 ⇒ 只认进程侧声明与老的单份配置。
        model_sources: Option<String>,
        /// 宿主声明的标称窗口（**原始取值**）：最终值要看档位——默认档可缺省、换家档必答，
        /// 因此和别的声明一样**在入口里**解析，不在这里定死成数字。
        context_window: Option<String>,
    },
}

/// 解析参数。任何缺失、重复、互斥冲突、未知选项或非法数值都在这里明确失败——
/// 不猜默认目录，不把工作目录当成资料根，也不为缺失项补默认值。
fn parse(arguments: &[String]) -> Result<Invocation, String> {
    let Some((subcommand, rest)) = arguments.split_first() else {
        return Err("缺少子命令（init / run）".to_string());
    };

    let mut root: Option<String> = None;
    let mut agent: Option<String> = None;
    let mut materials: Option<String> = None;
    let mut activity: Option<String> = None;
    let mut max_turns: Option<String> = None;
    let mut text_input: Option<String> = None;
    let mut create_dir: Option<String> = None;
    let mut context_window: Option<String> = None;
    let mut im_endpoint: Option<String> = None;
    let mut search_endpoint: Option<String> = None;
    let mut terminal_tier: Option<String> = None;
    let mut web_tier: Option<WebTier> = None;
    let mut mcp: Vec<String> = Vec::new();
    let mut model: Option<String> = None;
    let mut model_endpoint: Option<String> = None;
    let mut model_protocol: Option<String> = None;
    let mut model_sources: Option<String> = None;
    let mut identity_agent: Option<String> = None;
    let mut identity_name: Option<String> = None;
    let mut host_input = false;
    let mut stdin_input = false;

    let mut index = 0;
    while index < rest.len() {
        let option = rest[index].as_str();
        match option {
            "--root" => set_once(&mut root, option, take_value(rest, index, option)?)?,
            "--agent" => set_once(&mut agent, option, take_value(rest, index, option)?)?,
            "--materials" => set_once(&mut materials, option, take_value(rest, index, option)?)?,
            "--run" => set_once(&mut activity, option, take_value(rest, index, option)?)?,
            "--max-turns" => set_once(&mut max_turns, option, take_value(rest, index, option)?)?,
            "--input" => set_once(&mut text_input, option, take_value(rest, index, option)?)?,
            "--create-dir" => set_once(&mut create_dir, option, take_value(rest, index, option)?)?,
            "--context-window" => set_once(
                &mut context_window,
                option,
                take_value(rest, index, option)?,
            )?,
            "--im-endpoint" => {
                set_once(&mut im_endpoint, option, take_value(rest, index, option)?)?
            }
            "--search-endpoint" => set_once(
                &mut search_endpoint,
                option,
                take_value(rest, index, option)?,
            )?,
            "--terminal" => set_once(&mut terminal_tier, option, take_value(rest, index, option)?)?,
            // **出网档**（2026-10-13）：封闭枚举，**在解析这一处就 fail-closed**——不认识的档位不降级、
            // 也不拿近似档顶上（"主体能不能出网"是主人的政策，猜错就是替主人做主）。
            "--web" => {
                let value = take_value(rest, index, option)?;
                let tier = WebTier::parse(&value).ok_or_else(|| {
                    format!("未知的 web 档位: {value}（v1 只有 off / fetch / full）")
                })?;
                set_once(&mut web_tier, option, tier)?;
            }
            // **可重复**：一台 server 一条声明（重名由 resolve_mcp 那一关拒绝）。
            // 取值同样是下一项，因此这里要跨**两步**（与所有取值型选项一样）——跨一步会把声明
            // 本身当成下一个选项读（真活验收第一次跑就是这么撞上的：`多余的参数: …`）。
            "--mcp" => {
                mcp.push(take_value(rest, index, option)?);
                index += 2;
                continue;
            }
            "--model" => set_once(&mut model, option, take_value(rest, index, option)?)?,
            "--model-endpoint" => set_once(
                &mut model_endpoint,
                option,
                take_value(rest, index, option)?,
            )?,
            "--identity-agent" => set_once(
                &mut identity_agent,
                option,
                take_value(rest, index, option)?,
            )?,
            "--identity-name" => {
                set_once(&mut identity_name, option, take_value(rest, index, option)?)?
            }
            "--model-protocol" => set_once(
                &mut model_protocol,
                option,
                take_value(rest, index, option)?,
            )?,
            "--model-sources" => {
                set_once(&mut model_sources, option, take_value(rest, index, option)?)?
            }
            "--input-stdin" => {
                if stdin_input {
                    return Err("--input-stdin 给了不止一次".to_string());
                }
                stdin_input = true;
                index += 1;
                continue;
            }
            "--host-input" => {
                if host_input {
                    return Err("--host-input 给了不止一次".to_string());
                }
                host_input = true;
                index += 1;
                continue;
            }
            other if other.starts_with('-') => return Err(format!("未知选项: {other}")),
            other => return Err(format!("多余的参数: {other}")),
        }
        index += 2;
    }

    let root = required(subcommand, "--root", &root)?;
    let agent = AgentId::new(required(subcommand, "--agent", &agent)?);

    match subcommand.as_str() {
        "init" => {
            // init 路径不接收运行参数：给了就是写错了，不能静默忽略。
            for (name, present) in [
                ("--materials", materials.is_some()),
                ("--run", activity.is_some()),
                ("--max-turns", max_turns.is_some()),
                ("--input", text_input.is_some()),
                ("--input-stdin", stdin_input),
                ("--host-input", host_input),
                ("--create-dir", create_dir.is_some()),
                ("--context-window", context_window.is_some()),
                ("--im-endpoint", im_endpoint.is_some()),
                ("--search-endpoint", search_endpoint.is_some()),
                ("--terminal", terminal_tier.is_some()),
                ("--web", web_tier.is_some()),
                ("--mcp", !mcp.is_empty()),
                ("--model", model.is_some()),
                ("--model-endpoint", model_endpoint.is_some()),
                ("--model-protocol", model_protocol.is_some()),
                // 身份行是**运行期的声明**（开场那句"你是谁"），`init` 既不装配也不开场，
                // 给了就是写错了——不能静默忽略。
                ("--identity-agent", identity_agent.is_some()),
                ("--identity-name", identity_name.is_some()),
            ] {
                if present {
                    return Err(format!("init 不接受 {name}"));
                }
            }
            Ok(Invocation::Init {
                root: PathBuf::from(root),
                agent,
            })
        }
        "run" => {
            // 身份行（TASK_70）：两个值**成对**给出（缺一半就是没说清主体是谁）。
            let identity_claim = resolve_identity_claim(identity_agent, identity_name)?;
            let materials = required("run", "--materials", &materials)?;
            let activity = required("run", "--run", &activity)?;
            let max_turns = required("run", "--max-turns", &max_turns)?;
            let max_turns: u32 = max_turns
                .parse()
                .map_err(|_| format!("--max-turns 不是整数: {max_turns}"))?;
            // 窗口：这里定下**默认档**的值（原始声明，或适配层带出处的标称值）。
            // 换家档要求它**必须显式给出**——那条判断在 `resolve_model_source` 里（换家档的窗口
            // 必须重声明，缺省值属于默认档那个模型）。
            let window = match &context_window {
                Some(raw) => parse_context_window(raw)?,
                None => nominal_context_window(),
            };
            // 屏幕报告 cell：**写柄交给适配层**（唯一写者），读侧随声明进状态栏。
            let (reading_writer, reading) = ContextReading::channel();
            // 用量 cell（TASK_78 刀三）：与屏幕同款，写柄交适配层、读侧随声明进会话。
            let (usage_writer, usage) = UsageReport::channel();
            if text_input.is_some() && stdin_input {
                return Err("--input 与 --input-stdin 互斥".to_string());
            }
            let input = match (text_input, stdin_input) {
                (Some(text), _) => ManualInput::Text(text),
                (None, true) => ManualInput::Stdin,
                (None, false) => ManualInput::Empty,
            };
            let run_id = RunId::new(activity);
            // 授权只由宿主给出，而且**绑定这一次身份与活动**：模型既不能创建也不能扩大它。
            let creation = match &create_dir {
                Some(directory) => Some(
                    CreationGrant::new(agent.clone(), run_id.clone(), directory.as_str()).map_err(
                        |error| format!("--create-dir 不合法: {}（{directory}）", error.as_str()),
                    )?,
                ),
                None => None,
            };
            Ok(Invocation::Run {
                request: Box::new(ManualRun {
                    environment_root: PathBuf::from(root),
                    expected_agent: agent,
                    material_root: PathBuf::from(materials),
                    run: run_id,
                    max_turns,
                    input,
                    // 生产路径用**真实钟**：状态栏每一轮现取一次"现在几点"。
                    clock: ClockHandle::system(),
                    context_window: window,
                    reading,
                    declarations: SessionDeclarations {
                        creation,
                        usage: Some(usage),
                        // 身份行（TASK_70）：**宿主声明**的两个值（core 里那个 id 与此世界的名字，成对）。
                        identity: identity_claim.clone(),
                        // 其余几格（模型名 / 思考强度 / im / 搜索 / 终端 / MCP）**要等 `start` 里的解析**
                        // ——那些解析排在入口级判定之后，因此在这一层留空（`Default` 就是"全都没声明"）。
                        ..Default::default()
                    },
                }),
                host_input,
                reading: reading_writer,
                usage: usage_writer,
                im_endpoint,
                search_endpoint,
                terminal_tier,
                web_tier,
                mcp,
                model,
                model_endpoint,
                model_protocol,
                model_sources,
                context_window,
            })
        }
        other => Err(format!("未知子命令: {other}")),
    }
}

/// 宿主声明的上下文窗口：给了就用给的，没给就取适配层当前模型档位的**公开标称值**。
///
/// 窗口是**声明**，不是测量：厂商不回传它，所以"没有声明"这件事必须在这里被堵死——要么宿主
/// 显式给出，要么采用适配层带着出处的标称值，不存在第三条路。`0` 不是"更小的窗口"，而是一条
/// 无法解释的声明（状态行会写出 `reading = …/0`），因此直接拒绝，不猜、不改写。
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

/// 模型源的三件**原始**声明（入口读完环境变量之后交给 [`resolve_model_source`]）。
struct ModelSourceRequest {
    model: Option<String>,
    endpoint: Option<String>,
    context_window: Option<String>,
    /// 线协议（原始声明；缺省 = 本家那一条）。
    protocol: Option<String>,
}

/// 模型门要的**进程侧声明**（旗标那四件，原始取值）。
fn model_flags(request: &ModelSourceRequest) -> model_source::FlagDeclarations<'_> {
    model_source::FlagDeclarations {
        endpoint: request.endpoint.as_deref(),
        model: request.model.as_deref(),
        context_window: request.context_window.as_deref(),
        protocol: request.protocol.as_deref(),
    }
}

/// 解析并装配模型（**三档**），并给出这一档的标称窗口。
///
/// - **默认档**（没给 `--model-endpoint`，也没有配置文件）：零迁移。凭据 = `DEEPSEEK_API_KEY`；
///   端点与模型名由适配层拥有（`--model` 可以单独切本家的另一个档位）；窗口 = `--context-window`
///   或适配层的标称值。
/// - **换家档**（给了端点）：**四件全显式**——端点 / 凭据（`CELIRA_MODEL_API_KEY`）/ 模型名 / 标称窗口，
///   缺一件即失败（退出码与模型装配同族）。
///   ⚠ **配对只朝一个方向拒**：`CELIRA_MODEL_API_KEY` 单独在场、没给端点时它是**环境音**——这一档
///   根本不读它 ⇒ 忽略、照走默认档；被拒的只有"给了端点却缺配对件"。
/// - **文件档**（TASK_77：环境根下的私有配置文件在场）：形状与换家档同一套（四件 + 可选协议），
///   只是四件来自文件。它与另两档**互斥**——`model_source::decide` 已经拒绝了"文件在场而进程侧
///   也在说话"，因此走到 `Source::File` 时进程侧必然是沉默的。
///
/// **为什么换家档必须重声明窗口**：适配层的缺省窗口（1M）是**默认档那个模型**的声明，别家模型用它
/// 等于说谎。这不是新发明——适配层早就写着"它是声明不是实测：换模型档位必须重声明"，本函数只是把
/// 那句话落到装配入口上。
///
/// **一次调用 = 同次快照**：三元组只在这里解析并构造一次，绝不跨操作缓存、绝不回落。
/// 失败消息只报**变量名与旗标名**，永不报取值。
/// 屏幕 cell 的写柄由这里装上去（唯一写者），因此返回值是一个已经装好传感器的模型。
fn resolve_model_source(
    request: ModelSourceRequest,
    source: &model_source::Source,
    reading: &ContextReadingWriter,
    usage: &UsageWriter,
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
            EXIT_MODEL_ASSEMBLY_FAILED as i32,
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
            EXIT_MODEL_ASSEMBLY_FAILED as i32,
        ));
    }
    let build = |credential: String,
                 model: String,
                 endpoint: String|
     -> Result<ModelChoice, (String, i32)> {
        let failure = |_| {
            (
                "模型装配失败（请求客户端建不起来）".to_string(),
                EXIT_MODEL_ASSEMBLY_FAILED as i32,
            )
        };
        let built = if protocol == responses_adapter::PROTOCOL {
            ModelChoice::Responses(
                ResponsesModel::at_endpoint(credential, model, endpoint)
                    .map_err(failure)?
                    .with_reading(reading.clone())
                    .with_usage(usage.clone()),
            )
        } else {
            ModelChoice::Chat(
                DeepSeekModel::at_endpoint(credential, model, endpoint)
                    .map_err(failure)?
                    // **这一档真的发出去**：值由清单那一条给（唯一读侧已按封闭表校验过），
                    // 本层只搬运；`None` = 请求体里没有这个键（逐字节就是今天的样子）。
                    .with_reasoning_effort(declared_effort.clone())
                    .with_reading(reading.clone())
                    .with_usage(usage.clone()),
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
    if let model_source::Source::File(file) = source {
        let built = build(
            file.credential.clone(),
            file.model.clone(),
            file.endpoint.clone(),
        )?;
        return Ok((built, file.context_window));
    }
    match &request.endpoint {
        // ---- 默认档：取值与失败消息与从前**逐字节相同** ----
        None => {
            if protocol != provider_adapter::PROTOCOL {
                return Err((
                    format!(
                        "--model-protocol {protocol} 只在换家档有效：默认档是本家的 {} 端点，\
                         它没有第二种线协议。要换家就给一个 --model-endpoint。",
                        provider_adapter::PROTOCOL
                    ),
                    EXIT_MODEL_ASSEMBLY_FAILED as i32,
                ));
            }
            let credential = std::env::var(DeepSeekModel::API_KEY_ENV).ok();
            let credential = match credential {
                Some(value) if !value.trim().is_empty() => value,
                _ => {
                    return Err((
                        format!(
                            "模型服务凭据不可用: 请设置 {}（缺失或为空）。本命令不打印它的取值。",
                            DeepSeekModel::API_KEY_ENV
                        ),
                        EXIT_MODEL_ASSEMBLY_FAILED as i32,
                    ));
                }
            };
            let model = request
                .model
                .clone()
                .unwrap_or_else(|| DEFAULT_MODEL.to_string());
            let window = match &request.context_window {
                Some(raw) => parse_context_window(raw).map_err(|message| (message, 1))?,
                // 缺省值来自适配层：它自带出处，而且与它服务的那个模型档位同处一个文件。
                None => nominal_context_window(),
            };
            let built = build(credential, model, ENDPOINT.to_string())?;
            Ok((built, window))
        }
        // ---- 换家档：四件全显式，缺一即拒 ----
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
                        EXIT_MODEL_ASSEMBLY_FAILED as i32,
                    ));
                }
            };
            let Some(model) = request.model.clone() else {
                return Err((
                    format!(
                        "换家档必须显式给出模型名（--model）：{} 只在默认档有意义，本命令不猜。",
                        DEFAULT_MODEL
                    ),
                    EXIT_MODEL_ASSEMBLY_FAILED as i32,
                ));
            };
            let Some(raw_window) = request.context_window.clone() else {
                return Err((
                    "换家档必须显式声明标称窗口（--context-window）：缺省值属于默认档那个模型，\
                     别家模型用它等于说谎。"
                        .to_string(),
                    EXIT_MODEL_ASSEMBLY_FAILED as i32,
                ));
            };
            let window = parse_context_window(&raw_window).map_err(|message| (message, 1))?;
            let built = build(credential, model, endpoint.clone())?;
            Ok((built, window))
        }
    }
}

/// im 的声明：端点来自命令行、凭据来自环境变量——**两者必须成对**。
///
/// 只给一半一律拒绝：给端点却不给凭据（或反过来）不是『少配了一点』，而是一条无法解释的声明
/// （主体会看到一个列得出来却永远读不通的 im）。拒绝发生在打开环境、装配模型之前，只报变量名、
/// 不报取值，退出码与『模型服务凭据不可用』同族。
fn resolve_im(endpoint: &Option<String>) -> Result<Option<ImAccess>, (String, i32)> {
    const CREDENTIAL_ENV: &str = "CELIRA_IM_BACKEND_AUTH";
    let credential = std::env::var(CREDENTIAL_ENV).ok();
    match (endpoint, credential) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err((
            format!(
                "给了 {CREDENTIAL_ENV} 但没给 --im-endpoint：im 的端点是宿主的声明，本命令不猜地址"
            ),
            EXIT_MODEL_ASSEMBLY_FAILED as i32,
        )),
        (Some(base), Some(secret)) => match ImAccess::new(base.clone(), secret) {
            Some(access) => Ok(Some(access)),
            None => Err((
                format!("--im-endpoint 与 {CREDENTIAL_ENV} 都必须非空"),
                EXIT_MODEL_ASSEMBLY_FAILED as i32,
            )),
        },
        (Some(_), None) => Err((
            format!("im 凭据不可用: 请设置 {CREDENTIAL_ENV}（缺失或为空）。本命令不打印它的取值。"),
            EXIT_MODEL_ASSEMBLY_FAILED as i32,
        )),
    }
}

/// 本仓认的那一个搜索厂商端点（**配置**：换厂商就是换这一行或 `--search-endpoint`）。
const DEFAULT_SEARCH_ENDPOINT: &str = "https://api.tavily.com/search";
/// 搜索凭据的环境变量名（只报名字，不报取值）。
const SEARCH_CREDENTIAL_ENV: &str = "TAVILY_API_KEY";

/// 搜索厂商的声明：**凭据**来自环境变量、**端点**来自命令行（不给就用本仓认的那一个）。
///
/// 与 `resolve_im` 有意不同：**缺凭据不是入口错误**。web 窗是出厂内置的（两条动词从第一轮起
/// 就在场），没有凭据只意味着「查」这一条会如实失败 `credential-absent`（**没配**，是本地事实，
/// 不是对面拒了），而「取」不依赖厂商、照常可用。这里报变量名、不报取值，也绝不拿一个空凭据去试。
fn resolve_search(endpoint: &Option<String>) -> Option<SearchAccess> {
    let credential = std::env::var(SEARCH_CREDENTIAL_ENV).ok()?;
    if credential.trim().is_empty() {
        eprintln!(
            "web 搜索凭据不可用：{SEARCH_CREDENTIAL_ENV} 为空。本命令不打印它的取值；\
             `web_search` 会如实失败（credential-rejected），`web_fetch` 不受影响。"
        );
        return None;
    }
    let endpoint = endpoint
        .clone()
        .unwrap_or_else(|| DEFAULT_SEARCH_ENDPOINT.to_string());
    match SearchAccess::new(endpoint, credential) {
        Some(access) => Some(access),
        None => {
            eprintln!("搜索端点与 {SEARCH_CREDENTIAL_ENV} 都必须非空");
            None
        }
    }
}

/// terminal 档位的解析。两条都是 fail-closed：**不认识的档位**不降级；**本平台没有实现**
/// 也不换一个近似行为（退出码与"参数写错"同族）。
fn resolve_terminal(tier: &Option<String>) -> Result<Option<TerminalAccess>, (String, i32)> {
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

/// 外部来源的 App（MCP server）的**授权面**解析。
///
/// 三条都是 fail-closed：声明读不懂 ⇒ 拒绝（不猜）；两台同名 ⇒ 拒绝（动词前缀会撞，
/// 路由到谁都没有正确答案）；本平台没有 stdio 通道的实现 ⇒ 拒绝（不换一个近似行为）。
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
            .map_err(|message| (format!("--mcp 声明读不懂（{message}）"), 1))?;
        if accesses
            .iter()
            .any(|each| each.declaration().name() == declaration.name())
        {
            return Err((
                format!("--mcp 里有两台同名的 server：{}", declaration.name()),
                1,
            ));
        }
        accesses.push(McpAccess::new(declaration, credential_names()));
    }
    Ok(accesses)
}

/// 要洗掉的凭据键名（**只报名字**，永不取值）：这台机器上这个进程认得的全部凭据。
fn credential_names() -> Vec<String> {
    vec![
        "DEEPSEEK_API_KEY".to_string(),
        "TAVILY_API_KEY".to_string(),
        "CELIRA_IM_BACKEND_AUTH".to_string(),
    ]
}

/// 取一个选项的取值；缺取值就明确失败。
fn take_value(rest: &[String], index: usize, option: &str) -> Result<String, String> {
    rest.get(index + 1)
        .cloned()
        .ok_or_else(|| format!("{option} 缺少取值"))
}

/// 同一个选项只允许出现一次：重复等于调用方以为给了两样东西，不能只认其中一个。
///
/// 泛型只为**已经在解析处定型的取值**（如 `--web` 那三个词解析成的档位）：它们不该退回去存字符串
/// ——存字符串会让 `Invocation` 那个枚举的大变体再涨一格（clippy 的 `large_enum_variant` 当场报过）。
fn set_once<T>(slot: &mut Option<T>, option: &str, value: T) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{option} 给了不止一次"));
    }
    *slot = Some(value);
    Ok(())
}

/// 必填项：缺失或为空都明确失败。
fn required(subcommand: &str, name: &str, value: &Option<String>) -> Result<String, String> {
    match value {
        Some(value) if !value.is_empty() => Ok(value.clone()),
        Some(_) => Err(format!("{subcommand} 的 {name} 不能为空")),
        None => Err(format!("{subcommand} 缺少 {name}")),
    }
}

/// **身份行的两个值必须成对**（TASK_70）：只给一半是入口拒绝，不静默降级成"没有那一行"
/// （与 im 端点/凭据同一条规矩）。空串与"没给"同罪：一个空的名字不是名字。
fn resolve_identity_claim(
    agent: Option<String>,
    name: Option<String>,
) -> Result<Option<IdentityClaim>, String> {
    match (agent, name) {
        (None, None) => Ok(None),
        (Some(id), Some(display_name)) => {
            let claim = IdentityClaim { id, display_name };
            if claim.is_complete() {
                Ok(Some(claim))
            } else {
                Err("--identity-agent 与 --identity-name 都不能是空串".to_string())
            }
        }
        _ => Err(
            "--identity-agent 与 --identity-name 必须成对给出（缺一半就是没说清是哪个 Agent）"
                .to_string(),
        ),
    }
}

/// 入口参数的解析用例。
///
/// **为什么参数解析要有用例**（TASK_42 真活验收教的）：命令行的旗标接线没有任何别的地方能覆盖
/// ——离线套件构造的是 `ManualRun`，不是命令行。结果就是"新旗标跨错了一步"这种错**全绿通过**，
/// 一直到真活验收才以 `多余的参数: …` 暴露出来。入口是产品的第一道面，它自己也得有测量。
#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(list: &[&str]) -> Vec<String> {
        list.iter().map(|each| (*each).to_string()).collect()
    }

    /// 一条能过 `run` 必填项检查的最小命令行（不含被测旗标）。
    fn run_head() -> Vec<String> {
        arguments(&[
            "run",
            "--root",
            "env",
            "--agent",
            "a",
            "--materials",
            "materials",
            "--run",
            "run-1",
            "--max-turns",
            "3",
        ])
    }

    /// `--mcp` **消费它的取值**，而且可以重复（每台一条）。
    #[test]
    fn the_mcp_flag_takes_its_value_and_can_repeat() {
        let mut argv = run_head();
        argv.extend(arguments(&[
            "--mcp",
            "one=stdio:cmd1",
            "--mcp",
            "two=stdio:cmd2 --flag",
        ]));

        let Invocation::Run { mcp, .. } = parse(&argv).expect("这条命令行应当能解析")
        else {
            panic!("应当是 run");
        };
        assert_eq!(
            mcp,
            vec![
                "one=stdio:cmd1".to_string(),
                "two=stdio:cmd2 --flag".to_string()
            ],
            "声明要原样收下（逗号与空格都是命令的一部分）"
        );
    }

    /// 不给取值就是入口错误（不静默当成空声明）。
    #[test]
    fn the_mcp_flag_needs_a_value() {
        let mut argv = run_head();
        argv.push("--mcp".to_string());
        // `Invocation` 持着写柄、不派生 `Debug`（那是入口的既有形状），因此不借 `expect_err`。
        let message = match parse(&argv) {
            Err(message) => message,
            Ok(_) => panic!("缺取值应当被拒"),
        };
        assert!(message.contains("缺少取值"), "{message}");
    }

    /// `init` 不接受运行参数：给了就是写错了，不能静默忽略。
    #[test]
    fn init_refuses_the_mcp_flag() {
        let mut argv = arguments(&["init", "--root", "env", "--agent", "a"]);
        argv.extend(arguments(&["--mcp", "one=stdio:cmd1"]));
        let message = match parse(&argv) {
            Err(message) => message,
            Ok(_) => panic!("init 不接受 --mcp"),
        };
        assert!(message.contains("init 不接受 --mcp"), "{message}");
    }
}
