//! **执行端 ↔ im 的真机边界探针**：把只读动词对着**真的 im 服务端**跑一遍。
//!
//! ## 它补的是哪一类洞
//!
//! 2026-09-29 一天之内抓到两条同一族的缺陷，**都不是任何门禁发现的**，都是手工驱动真机才看见：
//!
//! - `im_my_profile` 把 GET 打到了只注册 `PUT` 的写路径上 ⇒ 服务端 `405` ⇒ 本层归入
//!   `unknown` ⇒ 整次活动 `system:indeterminate`，而**两边门禁各自全绿**；
//! - 在此之前，同一族还有一次 `unknown-action` 的准入拒绝。
//!
//! 为什么门禁看不见：执行端自己的用例打的是**假服务器**，假服务器按实现里的常量应答 ——
//! 实现与测试共享同一个错误假设。静态守卫（`scripts/check-boundary.mjs`）能盖住**路径与方法**，
//! 但盖不住**响应形状**与"这个动词在真机上到底通不通"。
//!
//! ⇒ 这条探针量的是**最后那一层**：每个只读动词，对着真服务端，**必须回 `success`**。
//!
//! ## 边界（如实登记）
//!
//! - **只跑只读动词**。写动词（`im_say` / `im_write_profile` / …）会改真家世界，本探针**不碰**；
//!   它们的边界由"写效应配同参复核"那套离线用例与真机演练分别负责。
//! - 它**不替代**离线门禁：要一个跑着的 im 与一支 BackendAuth 凭据，所以它进不了 `cargo test`。
//!   ⚠ **它和 `npm run verify:ui` 是同族**：那种"要真环境才能跑"的门禁**烂过一次**
//!   （verify-ui 有 8 处断言陈旧而没人知道，因为仓库里没有一处写过"什么时候该跑它"）。
//!   ⇒ **改动执行端的端点面（路径、方法、响应解析、失败分类）时必须跑它**，这条写在 `AGENTS.md`。
//! - 它只证明"通不通"，不证明"内容对不对"：回执正文不进判据（那要各自的用例）。
//!
//! ## 用法
//!
//! ```text
//! im_probe --im-endpoint <url> [--room <房间标识>]
//! ```
//!
//! 凭据只从环境变量 `CELIRA_IM_BACKEND_AUTH` 取（与 `agent_host` 同一支、同一个来源），
//! **不进命令行**（命令行会进进程表与日志）。日常跑它用 `node scripts/home/probe-agent-im.js`
//! —— 那一侧会按身份去本地存储里取凭据并注入，凭据不落任何输出。
//!
//! 退出码（**读数语义**：码是结论，不是"成功/失败"）：
//!
//! | 码 | 含义 |
//! |---|---|
//! | 0 | 每一个被探的动词都回了 `success` |
//! | 1 | 参数不合法（缺 `--im-endpoint`、未知选项、重复选项） |
//! | 2 | 装配不起来（端点或凭据缺一半） |
//! | 3 | 有动词回了**确定的失败** `failure`（权限 / 契约用法对不上） |
//! | 4 | 有动词回了 `unknown` —— **这一条就是本探针最想抓的那一类**（边界对不上） |

use std::fmt::Write as _;

use celira_agent::core::{Action, ActionId, ActionOutcome, ActionParams, ActionPort};
use celira_agent::foundation::{AgentId, OperationId, ResultState, RunId, Turn};
use celira_agent::im_app::{
    CAPABILITY_READ_HISTORY, CAPABILITY_ROOM_LIVE, CAPABILITY_ROOMS, ImApp,
};
use celira_agent::im_channel::ImAccess;
use celira_agent::im_profile_app::{CAPABILITY_MY_PROFILE, ImProfileApp};
use celira_agent::im_social_app::{CAPABILITY_BLOCKS, CAPABILITY_CONTACTS, ImSocialApp};

const HEADER: &str = "celira-agent-im-probe v1";
const CREDENTIAL_ENV: &str = "CELIRA_IM_BACKEND_AUTH";
const USAGE: &str = "用法: im_probe --im-endpoint <url> [--room <房间标识>]";

const EXIT_OK: i32 = 0;
const EXIT_USAGE: i32 = 1;
const EXIT_UNASSEMBLED: i32 = 2;
const EXIT_FAILURE: i32 = 3;
const EXIT_UNKNOWN: i32 = 4;

/// 本地 AgentId 只用来满足 `execute` 的形状：真实身份来自**服务端**回答的
/// `GET /v1/backend/me`（见 `im_app` 的 `whoami`）—— 探针不假装知道自己是世界里的谁。
const LOCAL_AGENT: &str = "im-probe";

struct Invocation {
    endpoint: String,
    room: Option<String>,
}

fn main() {
    std::process::exit(run(std::env::args().skip(1).collect()));
}

fn run(arguments: Vec<String>) -> i32 {
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!("{USAGE}");
        return EXIT_OK;
    }

    let invocation = match parse(&arguments) {
        Ok(invocation) => invocation,
        Err(message) => {
            eprintln!("{message}");
            eprintln!();
            eprintln!("{USAGE}");
            return EXIT_USAGE;
        }
    };

    // 凭据只从环境变量取：与 `agent_host` 同一支、同一个来源。
    let credential = match std::env::var(CREDENTIAL_ENV) {
        Ok(value) if !value.trim().is_empty() => value,
        // 缺了就说缺了，**不回显任何取值**（哪怕是空的）。
        _ => {
            eprintln!(
                "缺少 {CREDENTIAL_ENV}（im 的 BackendAuth 凭据）—— 用 scripts/home/probe-agent-im.js 跑，它会按身份去取"
            );
            return EXIT_UNASSEMBLED;
        }
    };

    // 与宿主**同一段装配**：一份 `ImAccess` 声明装出三个 App（2026-10-02 拆包后，通信面 /
    // 社交面 / 资料面各有自己的实例）—— 探针绝不自己拼路径、自己拼请求，那正是它要量的东西。
    let Some(access) = ImAccess::new(invocation.endpoint.clone(), credential) else {
        eprintln!("端点或凭据缺一半，装配不起来");
        return EXIT_UNASSEMBLED;
    };
    let Some(mut app) = ImApp::new(access.clone()) else {
        eprintln!("im app 装配不起来（端点或凭据缺一半）");
        return EXIT_UNASSEMBLED;
    };
    let Some(mut social) = ImSocialApp::new(access.clone()) else {
        eprintln!("im-social app 装配不起来（端点或凭据缺一半）");
        return EXIT_UNASSEMBLED;
    };
    let Some(mut profile) = ImProfileApp::new(access) else {
        eprintln!("im-profile app 装配不起来（端点或凭据缺一半）");
        return EXIT_UNASSEMBLED;
    };

    let mut lines: Vec<String> = Vec::new();
    let mut worst = EXIT_OK;

    let agent = AgentId::new(LOCAL_AGENT);
    let operation = OperationId::new(RunId::new("probe"), Turn::FIRST);
    for capability in probe_plan(invocation.room.as_deref()) {
        let action = Action::with_params(ActionId::new(capability.id), capability.params);
        // 动词跟着**拥有它的包**走：通信面在 `im`，社交面在 `im-social`，资料面在 `im-profile`。
        let outcome = match capability.id {
            CAPABILITY_MY_PROFILE => profile.execute(&agent, &operation, &action),
            CAPABILITY_CONTACTS | CAPABILITY_BLOCKS => social.execute(&agent, &operation, &action),
            _ => app.execute(&agent, &operation, &action),
        };
        let (state, exit) = classify(&outcome);
        if exit > worst {
            worst = exit;
        }
        let mut line = format!(
            "verb={} state={} blocks={}",
            capability.id,
            state,
            outcome.content().len()
        );
        if let Some(class) = outcome.failure_class() {
            // 失败分类是**封闭词表**里的稳定标记（不是服务端原文），可以如实带出来。
            let _ = write!(line, " failure={class}");
        }
        lines.push(line);
    }

    let mut out = String::new();
    out.push_str(HEADER);
    out.push('\n');
    let _ = writeln!(out, "endpoint={}", invocation.endpoint);
    let _ = writeln!(out, "room={}", invocation.room.as_deref().unwrap_or("none"));
    for line in &lines {
        out.push_str(line);
        out.push('\n');
    }
    let _ = writeln!(out, "probed={} verdict={}", lines.len(), verdict(worst));
    print!("{out}");

    worst
}

/// 一个只读动词 + 它的参数。
struct ProbeStep {
    id: &'static str,
    params: ActionParams,
}

/// 探针清单：**先探不依赖房间的，再探要房间的**（后者只在给了 `--room` 时才跑）。
///
/// ⚠ 清单变长**不是**这里的判据；名单要跟着"执行端有哪些**只读**动词"走。
/// 写动词一律不在清单里（它们会改真家世界）。
fn probe_plan(room: Option<&str>) -> Vec<ProbeStep> {
    let mut steps = vec![
        ProbeStep {
            id: CAPABILITY_ROOMS,
            params: ActionParams::none(),
        },
        ProbeStep {
            id: CAPABILITY_MY_PROFILE,
            params: ActionParams::none(),
        },
        ProbeStep {
            id: CAPABILITY_CONTACTS,
            params: ActionParams::none(),
        },
        ProbeStep {
            id: CAPABILITY_BLOCKS,
            params: ActionParams::none(),
        },
    ];
    if let Some(room) = room {
        steps.push(ProbeStep {
            id: CAPABILITY_READ_HISTORY,
            params: ActionParams::none().with("room", room),
        });
        steps.push(ProbeStep {
            id: CAPABILITY_ROOM_LIVE,
            params: ActionParams::none().with("room", room),
        });
    }
    steps
}

/// 一个回执的读法：稳定标记 + 它把这次的结论压到哪一档退出码。
fn classify(outcome: &ActionOutcome) -> (&'static str, i32) {
    match outcome.state() {
        ResultState::Success => ("success", EXIT_OK),
        ResultState::Failure => ("failure", EXIT_FAILURE),
        // **最想抓的那一档**：服务端答了一个契约没列的东西（或压根没答），本层不猜。
        ResultState::Unknown => ("unknown", EXIT_UNKNOWN),
    }
}

fn verdict(exit: i32) -> &'static str {
    match exit {
        EXIT_UNKNOWN => "unknown-boundary",
        EXIT_FAILURE => "refused",
        EXIT_OK => "all-success",
        _ => "usage",
    }
}

/// 参数解析：**每个选项至多一次**，未知选项明确失败（不静默忽略）。
fn parse(arguments: &[String]) -> Result<Invocation, String> {
    let mut endpoint: Option<String> = None;
    let mut room: Option<String> = None;
    let mut index = 0;
    while index < arguments.len() {
        let flag = arguments[index].as_str();
        let value = arguments.get(index + 1);
        match flag {
            "--im-endpoint" => {
                if endpoint.is_some() {
                    return Err("--im-endpoint 给了不止一次".to_owned());
                }
                endpoint = Some(non_empty(value, "--im-endpoint")?);
                index += 2;
            }
            "--room" => {
                if room.is_some() {
                    return Err("--room 给了不止一次".to_owned());
                }
                room = Some(non_empty(value, "--room")?);
                index += 2;
            }
            other => return Err(format!("不认识的参数：{other}")),
        }
    }
    match endpoint {
        Some(endpoint) => Ok(Invocation { endpoint, room }),
        None => Err("缺少 --im-endpoint".to_owned()),
    }
}

fn non_empty(value: Option<&String>, flag: &str) -> Result<String, String> {
    match value {
        Some(value) if !value.trim().is_empty() => Ok(value.clone()),
        _ => Err(format!("{flag} 后面要给一个非空取值")),
    }
}
