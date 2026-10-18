//! **家的状态口**（app 自己端）——**执行端平面的私有事实**，不经 im 服务端。
//!
//! ## 为什么有它，以及它为什么是"委派"而不是"重写"
//!
//! 「我的 Agent」每行那三格（在不在跑 / 用哪条模型 / 最近活动）与机器页那三间，
//! 读的都是**家口**（loopback HTTP）：web 形态由 `node home.js doors` 供，**app 里没人供**
//! ⇒ 页面显示"状态未知"。这一片把这道门在壳里端起来（不依赖 node，这是打包 exe 的前置）。
//!
//! 判据（**不发明**）：
//!
//! | 事实 | 权威 | 壳怎么做 |
//! |---|---|---|
//! | 载荷形状、三态、TTL/`stale` | `scripts/home/status-server.js` | 逐格照抄（端面只有 `GET /status` 与 `GET /healthz`） |
//! | **处境**（admission / activities / runs / next） | **运行时自己的 CLI** `agent_host_status`（解析权威 `host_status_parse.js`） | **spawn 它**（与 JS 口子同一个权威）——**壳不复制那套投影** |
//! | 文本输出的解析 | `host_status_parse.js` | 移植（严格：认不出就整体 `unreadable`，绝不猜） |
//! | `lastWake` | 宿主日志里最后一行 `wake ` | 读同一份日志 |
//! | `im` | `GET /health` 的状态码 | 同一判据（**0 不算读数**，如实给 `null`） |
//!
//! ## 这一刀**不给**的那一格（如实登记）
//!
//! `experience`（"经历"的机械投影，`status-server.js` 里的白名单投影）：那是**一套有权威的
//! 投影**，壳里抄第二份就是造第二个权威。⇒ 载荷里**缺这一格**，而页面本来就把缺省当
//! 「这一面读不到」（`HomeStatusView.experience?` 的注释写着）✓ —— 不假装、不填零。
//!
//! ## 两条纪律（都不是口味）
//!
//! 1. **env 白名单**（照 `minimalEnv`）：那个二进制本来就不吐凭据，但"不吐"是它的属性、
//!    "不给"是我们的纪律 —— 少一层依赖就少一个将来漏的机会；
//! 2. **口上无秘密**：这一片**不读**凭据目录、不报"有没有凭据"（JS 那份的理由原样成立：
//!    算出来的是另一个目录，报一个算错的"没有"比不报更坏）。
//!
//! ## 与世界的边界（双平面）
//!
//! 这道门**只读家**（`home.conf` / `identity.json` / 宿主日志 / 运行时 CLI），
//! **一个字节都不问世界**（除了 `GET /health` 那一句"世界在不在"——那是**探活**，不是读事实）。
//! 反过来，世界的公共事实（消息、房间、关系）也**不进这道门**。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::local_residents;
use crate::local_world::LocalFacts;
use crate::world_channel::WorldHttp;

/// 只读处的默认端口（`paths.js` 的 `DEFAULT_STATUS_PORT`）。
const DEFAULT_STATUS_PORT: &str = "5190";
/// 配置门的默认端口（`paths.js` 的 `DEFAULT_CONFIG_PORT`）。
const DEFAULT_CONFIG_PORT: &str = "5191";
/// TTL（`status-server.js` 的 `STATUS_TTL_MS`）：15 秒。
///
/// 为什么必须有：一次探测要 spawn `agent_host_status` 读整个账本（真家实测 ~3.7 秒）
/// ⇒ 页面每次进来都先等它。TTL 内直接给上次那份，代价是 `stale: true` **必须被标出来**。
const STATUS_TTL_MS: u64 = 15_000;
/// 明细只给最近几条（`status-server.js` 的 `RECENT_RUNS`）。
const RECENT_RUNS: usize = 32;
/// `agent_host_status` 的输出头（`host_status_parse.js` 的 `HEADER`）。
const HOST_STATUS_HEADER: &str = "celira-agent-host-status v1";

/// spawn `agent_host_status` 时只给这些环境变量（照 `ALLOWED_ENV_KEYS`，一字不差）。
const ALLOWED_ENV_KEYS: [&str; 9] = [
    "SystemRoot",
    "windir",
    "PATH",
    "PATHEXT",
    "TEMP",
    "TMP",
    "NUMBER_OF_PROCESSORS",
    "OS",
    "PROCESSOR_ARCHITECTURE",
];

// ---------------------------------------------------------------------------
// 起这两道门
// ---------------------------------------------------------------------------

/// 一次"确保状态口在跑"的结局。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DoorOutcome {
    /// 已经有人在这条口上答（多半是 `node home.js doors`）——**没动它**。
    AlreadyServed,
    /// 这一次起起来了（监听线程已在壳进程里）。
    Started { port: u16 },
    /// 端口读不懂 / 绑不上（原因原样）。
    Failed { detail: String },
}

impl DoorOutcome {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::AlreadyServed => "already-served",
            Self::Started { .. } => "started",
            Self::Failed { .. } => "failed",
        }
    }
}

/// 这道门的状态口端口（`home.conf` 的 `status_port`，缺省 5190）。
pub fn status_port(facts: &LocalFacts) -> u16 {
    facts
        .conf
        .status_port
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()))
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or_else(|| DEFAULT_STATUS_PORT.parse().unwrap_or(5190))
}

/// 这条口上有人在答吗（`GET /healthz` 回 200）——**探活优先**。
pub fn served(facts: &LocalFacts) -> bool {
    let port = status_port(facts);
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(600)));
    let request =
        format!("GET /healthz HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut buffer = [0u8; 256];
    match stream.read(&mut buffer) {
        Ok(read) => String::from_utf8_lossy(&buffer[..read]).starts_with("HTTP/1.1 200"),
        Err(_) => false,
    }
}

/// 确保状态口在跑（**探活优先**：已经有人供就不动它）。
///
/// ⚠ 起的是**壳进程里的一个监听线程**，不是子进程 —— 这道门是壳自己的 HTTP 面
/// （没有 node、没有第二个可执行件；进程死了口自然消失，不留野进程）。
pub fn ensure_status_door(facts: &LocalFacts) -> DoorOutcome {
    if served(facts) {
        return DoorOutcome::AlreadyServed;
    }
    let port = status_port(facts);
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(error) => {
            return DoorOutcome::Failed {
                detail: format!("状态口绑不上 127.0.0.1:{port}（{error}）"),
            }
        }
    };
    let home_dir = facts.home_dir.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            // 一条连接一次处理（`Connection: close`）：端面只有两条只读路由，
            // 不需要 keep-alive 那套；一个慢请求也拖不住别人（每条连接一个线程）。
            let home_dir = home_dir.clone();
            std::thread::spawn(move || {
                let _ = handle_status_connection(stream, &home_dir);
            });
        }
    });
    DoorOutcome::Started { port }
}

// ---------------------------------------------------------------------------
// 配置门（`/home-config` 的**读**那一半；写与探测是下一片）
// ---------------------------------------------------------------------------

/// 配置门的端口（`home.conf` 的 `config_port`，缺省 5191）。
pub fn config_port(facts: &LocalFacts) -> u16 {
    facts
        .conf
        .config_port
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()))
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or_else(|| DEFAULT_CONFIG_PORT.parse().unwrap_or(5191))
}

/// 这条口上有人在供吗（`GET /home-config` 答得出**带 `readAt` 的 JSON**）。
///
/// ⚠ 判据比状态口严一点：端口被**别的程序**占着时也给 200/404，但那不是这道门
/// ⇒ 只认"读得懂的那份快照"，别的都算"没人供"（调用方会去绑，绑不上就如实报失败）。
pub fn config_served(facts: &LocalFacts) -> bool {
    let port = config_port(facts);
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(800)));
    let request =
        format!("GET /home-config HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut buffer = Vec::new();
    if stream.read_to_end(&mut buffer).is_err() {
        return false;
    }
    let text = String::from_utf8_lossy(&buffer);
    let Some(body) = text.split("\r\n\r\n").nth(1) else {
        return false;
    };
    serde_json::from_str::<Value>(body)
        .ok()
        .map(|value| value.get("readAt").is_some())
        .unwrap_or(false)
}

/// 确保配置门在跑（**探活优先**；起的是壳进程里的监听线程）。
pub fn ensure_config_door(facts: &LocalFacts) -> DoorOutcome {
    if config_served(facts) {
        return DoorOutcome::AlreadyServed;
    }
    let port = config_port(facts);
    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => listener,
        Err(error) => {
            return DoorOutcome::Failed {
                detail: format!("配置门绑不上 127.0.0.1:{port}（{error}）"),
            }
        }
    };
    let home_dir = facts.home_dir.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let home_dir = home_dir.clone();
            std::thread::spawn(move || {
                let _ = handle_config_connection(stream, &home_dir);
            });
        }
    });
    DoorOutcome::Started { port }
}

fn handle_config_connection(mut stream: TcpStream, home_dir: &Path) -> std::io::Result<()> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    // 头（这一片只需要 `host` / `origin` / `content-length`）。
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name == "content-length" {
                content_length = value.parse::<usize>().unwrap_or(0);
            }
            headers.push((name, value));
        }
    }
    // 体（**带上限**：这是一份四件套，不是上传通道；超限就如实拒绝，不先分配再收）。
    let mut body_text = String::new();
    if content_length > BODY_LIMIT_BYTES {
        // ⚠ **先把那句拒绝写出去、再有界地收尾**：直接断连会让还在发的客户端拿到 RST
        // （真机验过：客户端只看到 `ECONNRESET`，读不到这句"体太大"）。
        write_json_with(&mut stream, 400, &json!({ "error": "body_too_large" }), "")?;
        let mut sink = vec![0u8; 8192];
        let mut drained = 0usize;
        while drained < content_length.min(1024 * 1024) {
            match reader.read(&mut sink) {
                Ok(0) | Err(_) => break,
                Ok(read) => drained += read,
            }
        }
        return Ok(());
    }
    if content_length > 0 {
        let mut buffer = vec![0u8; content_length];
        reader.read_exact(&mut buffer)?;
        body_text = String::from_utf8_lossy(&buffer).to_string();
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let path = target.split('?').next().unwrap_or("").to_string();

    // **来源校验**（照 `home-key.js` 的 `localOriginAllowed`，模式一专属）：
    // `Host` 必须是 loopback（挡"某个域名解析到 127.0.0.1"那类），带 `Origin` 时它也得是 loopback。
    let facts = LocalFacts::load(Some(home_dir), None);
    if !origin_allowed(&headers) {
        return write_json_with(
            &mut stream,
            403,
            &json!({ "error": "origin_not_allowed" }),
            "",
        );
    }

    // 端面是**穷举**的四条（照 `config-server.js`）：`/home-config`（清单读写）、
    // `/home-binding`（绑定读写）、`/home-config/probe`（只读探测，钥匙从请求体来）、
    // `/home-config/catalog`（只读目录，钥匙用清单里存着的那一条 —— 2026-10-14）。
    // `allow` 按**每条路由自己的**动词表给（照 JS：`/home-binding` 只有 PUT、探测与目录只有 POST）。
    let (status, body, allow) = match (method.as_str(), path.as_str()) {
        ("GET", "/home-config") => (200, config_snapshot(&facts), ""),
        ("POST", "/home-config") | ("PUT", "/home-config") => {
            let (status, body) = config_write(&facts, &method, &body_text);
            (status, body, "")
        }
        ("DELETE", "/home-config") => {
            let (status, body) = config_delete(&facts, query_param(&target, "id").as_deref());
            (status, body, "")
        }
        ("PUT", "/home-binding") => {
            let (status, body) = binding_write(&facts, &body_text);
            (status, body, "")
        }
        ("POST", "/home-config/probe") => {
            let (status, body) = probe_action(&body_text);
            (status, body, "")
        }
        ("POST", "/home-config/catalog") => {
            let (status, body) = catalog_action(&facts, &body_text);
            (status, body, "")
        }
        (_, "/home-config") => (
            405,
            json!({ "error": "method_not_allowed" }),
            "Allow: GET, POST, PUT, DELETE\r\n",
        ),
        (_, "/home-binding") => (
            405,
            json!({ "error": "method_not_allowed" }),
            "Allow: PUT\r\n",
        ),
        (_, "/home-config/probe") | (_, "/home-config/catalog") => (
            405,
            json!({ "error": "method_not_allowed" }),
            "Allow: POST\r\n",
        ),
        _ => (404, json!({ "error": "not_found" }), ""),
    };
    write_json_with(&mut stream, status, &body, allow)
}

fn write_json(stream: &mut TcpStream, status: u16, body: &Value) -> std::io::Result<()> {
    write_json_with(stream, status, body, "")
}

fn write_json_with(
    stream: &mut TcpStream,
    status: u16,
    body: &Value,
    extra_headers: &str,
) -> std::io::Result<()> {
    let text = body.to_string();
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json; charset=utf-8\r\n\
         Content-Length: {length}\r\nConnection: close\r\n{extra_headers}\r\n",
        // 状态行的措辞照 HTTP 本来的意思写（403 不是 "Not Found"）。
        reason = match status {
            200 => "OK",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            409 => "Conflict",
            500 => "Internal Server Error",
            _ => "Error",
        },
        length = text.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(text.as_bytes())?;
    stream.flush()
}

/// 这个家住着哪几支的**环境根**（含老家那一支 `<家>/env`）——与 `residents.js` 同款。
///
/// 为什么配置门要这份：绑定是**一支一份**的（`<环境根>/model_binding.json`），
/// 而"这个家绑了哪条"是各支的**并集**（都 absent ⇒ absent；有一支读不出来 ⇒ unreadable）。
fn env_roots_with_ids(home_dir: &Path) -> Vec<(Option<String>, std::path::PathBuf)> {
    let mut out: Vec<(Option<String>, std::path::PathBuf)> = Vec::new();
    for resident in local_residents::residents(home_dir) {
        out.push((
            env_identity(&resident.env_root).or(Some(resident.agent_id.clone())),
            resident.env_root,
        ));
    }
    // 老家那一支（`<家>/env`）：只认"有 identity.json"的（与 `residents.js` 的 legacyResident 同款）。
    let legacy = home_dir.join("env");
    if legacy.join("identity.json").is_file() {
        out.push((env_identity(&legacy), legacy));
    }
    out
}

/// **三合一快照**（`GET /home-config` 的载荷；形状权威 `config-server.js` 的 `snapshot`）。
///
/// 三层各说各的话：**清单**（机器级）／**绑定**（各支）／**由绑定算出来的派生字段**。
/// ⚠ `api_key` **一个字节都不在这个载荷里**（只有 `keyConfigured` 布尔）。
pub fn config_snapshot(facts: &LocalFacts) -> Value {
    use crate::home_facts::{read_model_binding, ModelBinding};
    use crate::model_sources::{read_list, EntryPayload, ListState};

    let home_dir = &facts.home_dir;
    let list = read_list(home_dir);
    let env_roots = env_roots_with_ids(home_dir);
    let bindings: Vec<(Option<String>, ModelBinding)> = env_roots
        .iter()
        .map(|(agent_id, env_root)| (agent_id.clone(), read_model_binding(env_root)))
        .collect();
    // ★ **这两份配置最后是谁改的**（2026-10-18；形状权威 `scripts/home/model_witness.js`）。
    //   一次读全，然后**按份看**（`哪一份 ⇒ 它自己最后一条`）—— 不是"整本账里有没有人动过"。
    //   两档分开报：门改的**有时刻**；别处改的（她走 terminal、别的工具）**只知"变了"**。
    let ledger = crate::model_witness::open(home_dir);
    let reading_of =
        |file: &Path| crate::model_witness::reading_for(&ledger, home_dir, file).to_json();

    let unreadable_one = bindings.iter().find_map(|(_, binding)| match binding {
        ModelBinding::Unreadable { reason } => Some(reason.clone()),
        _ => None,
    });
    let bound_one = bindings.iter().find_map(|(_, binding)| match binding {
        ModelBinding::Bound { source, .. } => Some(source.clone()),
        _ => None,
    });
    let (binding_state, binding_source, binding_reason) = match (&unreadable_one, &bound_one) {
        (Some(reason), _) => ("unreadable", None, Some(reason.clone())),
        (None, Some(source)) => ("bound", Some(source.clone()), None),
        (None, None) => ("absent", None, None),
    };
    // `bindingDivergent`：各支绑得不一样吗（一支一份文件 ⇒ 该如实标出来）。
    let distinct: std::collections::HashSet<Option<String>> = bindings
        .iter()
        .map(|(_, binding)| match binding {
            ModelBinding::Bound { source, .. } => Some(source.clone()),
            _ => None,
        })
        .collect();
    let divergent = distinct.len() > 1;

    let bound_entry = binding_source
        .as_ref()
        .and_then(|source| list.entries().iter().find(|entry| &entry.id == source));
    let dangling = binding_source.is_some() && bound_entry.is_none();
    let reason = match &list {
        ListState::Unreadable { reason } => Some(reason.clone()),
        ListState::Loaded { .. } => binding_reason.clone().or({
            if dangling {
                Some("binding_dangling".to_string())
            } else {
                None
            }
        }),
    };
    let state = if list.kind() == "unreadable" || binding_state == "unreadable" || dangling {
        "unreadable"
    } else if bound_entry.is_some() {
        "configured"
    } else {
        "absent"
    };

    let entries: Vec<EntryPayload> = list.entries().iter().map(EntryPayload::from).collect();
    json!({
        "readAt": now_iso8601(),
        "state": state,
        "reason": reason,
        // **门的回答只有这一个布尔**（取值永不回来）。
        "keyConfigured": bound_entry.map(|entry| !entry.api_key.trim().is_empty()).unwrap_or(false),
        "endpoint": bound_entry.map(|entry| entry.endpoint.clone()),
        "model": bound_entry.map(|entry| entry.model.clone()),
        "contextWindow": bound_entry.map(|entry| entry.context_window),
        "protocol": bound_entry.and_then(|entry| entry.protocol.clone()),
        "sourcesState": list.kind(),
        "entries": entries,
        "bindingState": binding_state,
        "bound": binding_source,
        "bindingDivergent": divergent,
        // ★ **这一扇门不提供"从别的机器接 Agent 上来"**（2026-10-15 远程接上那一刀）：
        //   壳自己端的这两颗口只绑 loopback（给本机 app 用）⇒ 远程那条路对它是**不适用**，
        //   如实报 `null`。
        //
        //   ⚠ 三态各有各的意思，**不许压成两态**：`"off"` 是"这台机器有那条路、只是没开"
        //   （源码形态的门才可能这么答；开它的命令是 `node home.js remote-control on`）；
        //   `null` 是"**这条路在这台机器上不存在**"（去开开关也没用）。页面按三态说三句话。
        "remoteControl": Value::Null,
        // ★ **这两份配置最后是谁改的**（2026-10-18）：顶层这一格是**清单**那一份的见证；
        //   逐支那两格的每一行带着**它自己那份绑定**的见证（见证跟它见证的那一份走，
        //   不另开一张平行的表）。`witnessChainBreakAt` = 账本链条的第一个断点
        //   （`null` = 完整）——"只追加 ＋ 链值"若没人读，它就是一个装饰。
        "listWitness": reading_of(&crate::model_sources::list_path(home_dir)),
        "witnessChainBreakAt": ledger.chain_break_at,
        "boundPerResident": bindings
            .iter()
            .zip(env_roots.iter())
            .map(|((agent_id, binding), (_, env_root))| {
                let (bound, model, effort) = match binding {
                    ModelBinding::Bound {
                        source,
                        model,
                        reasoning_effort,
                    } => (Some(source.clone()), model.clone(), reasoning_effort.clone()),
                    _ => (None, None, None),
                };
                json!({
                    "agentId": agent_id,
                    "state": match binding {
                        ModelBinding::Bound { .. } => "bound",
                        ModelBinding::Absent => "absent",
                        ModelBinding::Unreadable { .. } => "unreadable",
                    },
                    "bound": bound,
                    // ★ **支级覆盖那两格**（2026-10-14）：这一支**具体用哪个型号 / 什么思考档**
                    //   （`null` = 没覆盖，用它选的条目里的）。没有这两格，页面只能说"用哪一条来源"，
                    //   而运行时**一直**认它们 ⇒ 那是"页面上说的"与"实际在用的"对不上。
                    "model": model,
                    "reasoningEffort": effort,
                    // ★ 这一支那份 `model_binding.json` 的见证（**可选**：老门不给 ⇒ 调用方如实说"读不到"）。
                    "witness": reading_of(&env_root.join(crate::model_sources::BINDING_FILE)),
                })
            })
            .collect::<Vec<Value>>(),
        // ★ **还没住户化、但已经先选好了**的那几支（2026-10-14「选择先于接上」）：
        //   它还没被接到这台机器上，而有人已经在它的页面上给它选了哪一条 —— 单列一格，
        //   **不混进上面那几格**（混进去机器页就会把它画成住在这里）。形状权威是
        //   `scripts/home/model-sources.js` 的 `unhomedBindings`（两个生产者逐条同形）。
        "boundUnhomed": crate::model_sources::unhomed_bindings(home_dir)
            .into_iter()
            .map(|(agent_id, binding)| {
                let (state, bound, model, effort) = match &binding {
                    ModelBinding::Bound {
                        source,
                        model,
                        reasoning_effort,
                    } => (
                        "bound",
                        Some(source.clone()),
                        model.clone(),
                        reasoning_effort.clone(),
                    ),
                    ModelBinding::Absent => ("absent", None, None, None),
                    ModelBinding::Unreadable { .. } => ("unreadable", None, None, None),
                };
                json!({
                    "agentId": agent_id,
                    "state": state,
                    "bound": bound,
                    "model": model,
                    "reasoningEffort": effort,
                    // 她那份绑定（**先选好的那条**）也有见证 —— 与住户那一格同一个形状、同一个来路。
                    "witness": reading_of(
                        &crate::home_facts::agent_env_root(home_dir, &agent_id)
                            .join(crate::model_sources::BINDING_FILE),
                    ),
                })
            })
            .collect::<Vec<Value>>(),
    })
}

// ---------------------------------------------------------------------------
// 配置门的**写**那一半（动作面照 `config-server.js`；错误词表是封闭表，逐字对齐）
// ---------------------------------------------------------------------------

/// 请求体上限（`BODY_LIMIT_BYTES`）。
const BODY_LIMIT_BYTES: usize = 64 * 1024;
/// 试连超时（`PROBE_TIMEOUT_MS`）。
const PROBE_TIMEOUT_MS: u64 = 8_000;
/// 试连回执里最多带回多少条模型名（`MODEL_LIST_LIMIT`）。
const MODEL_LIST_LIMIT: usize = 200;
/// 线协议闭集（`PROTOCOLS`）。
const PROTOCOLS: [&str; 2] = ["chat-completions", "responses"];
/// 标称窗口的同语法预检（`WINDOW`：正整数，0 不是一条可解释的声明）。
fn window_shape_ok(value: &str) -> bool {
    !value.is_empty() && value.len() <= 10 && value.chars().all(|c| c.is_ascii_digit())
}

/// 从 `/models` 回执里读**标称窗口**时认哪些字段（**有序**：先命中的赢；点号往下一层找）。
const WINDOW_FIELDS: [&str; 9] = [
    "context_length",
    "context_window",
    "max_context_length",
    "max_context_tokens",
    "context_size",
    "max_input_tokens",
    "input_token_limit",
    "max_model_len",
    "top_provider.context_length",
];

/// **壳自己的来源**（Tauri 的自定义协议）—— 2026-10-05 主人裁：它也算本机。
///
/// 为什么非认它不可：app 的**打包形态**（`tauri build`）页面走内嵌产物，来源是
/// `http://tauri.localhost`（Windows/Linux 的 webview）或 `tauri://localhost`（macOS），
/// **不是 loopback** ⇒ 配置门会把自己的页面拒掉。真机实测：打包形态里机器页「模型」
/// 那一间显示"读不到模型配置 ／ 这个来源不被允许（只允许从本机访问）"，而 dev 形态
/// （页面在 `localhost:5173`）一直是绿的 —— 所以这一条**只在产品形态上成立**
/// （`app/README.md` §5 第 10 行）。
///
/// ⚠ **协议与主机成对匹配**（不是"协议对就行"）：只判协议会把 `tauri://evil.example`
/// 放进来 —— 那是一条**由别人供内容**的来源（2026-10-05 被自己的反例打回来过）。
/// 端口一律不看，与 loopback 那一条同款。权威在 `scripts/home/home-key.js` 的 `SHELL_ORIGINS`。
const SHELL_ORIGINS: [(&str, &str); 3] = [
    // Windows / Linux 的 webview：自定义协议挂在 `tauri.localhost` 这个名字上。
    ("http", "tauri.localhost"),
    ("https", "tauri.localhost"),
    // macOS 的 webview：自定义协议是 `tauri://localhost`。
    ("tauri", "localhost"),
];

/// 来源校验（照 `home-key.js` 的 `localOriginAllowed`）：`Host` 必须 loopback；
/// 带 `Origin` 时它得是 loopback **或壳自己的来源**（见 `SHELL_ORIGINS`）。
fn origin_allowed(headers: &[(String, String)]) -> bool {
    let find = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    // 没有 `Host` 头（裸 HTTP 客户端）⇒ 放行：这条判据防的是"浏览器里别的页面"，
    // 而浏览器一定会带 `Host`。
    let Some(host) = find("host") else {
        return true;
    };
    if !loopback_host(&host) {
        return false;
    }
    let Some(origin) = find("origin") else {
        return true;
    };
    if shell_origin(&origin) {
        return true;
    }
    let authority = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or("")
        .split('/')
        .next()
        .unwrap_or("");
    loopback_host(authority)
}

/// `Origin` 是不是壳自己（协议与主机成对，见 `SHELL_ORIGINS`）。
fn shell_origin(origin: &str) -> bool {
    let Some((scheme, rest)) = origin.split_once("://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or("");
    let host = authority
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    SHELL_ORIGINS
        .iter()
        .any(|(each_scheme, each_host)| *each_scheme == scheme && *each_host == host)
}

/// `127.0.0.1[:port]` / `localhost[:port]` / `[::1][:port]` —— loopback 才算自己的门。
fn loopback_host(authority: &str) -> bool {
    let host = if let Some(rest) = authority.strip_prefix('[') {
        // IPv6 字面量：`[::1]:5191`
        rest.split(']').next().unwrap_or("").to_string()
    } else {
        authority.split(':').next().unwrap_or("").to_string()
    };
    matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1")
}

/// 查询串里取一个参数（`?id=ms-1`）。
fn query_param(target: &str, name: &str) -> Option<String> {
    let query = target.split_once('?')?.1;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == name {
            return Some(value.to_string());
        }
    }
    None
}

fn parse_body(body_text: &str) -> Result<Value, Value> {
    if body_text.trim().is_empty() {
        return Err(json!({ "error": "bad_request" }));
    }
    serde_json::from_str::<Value>(body_text).map_err(|_| json!({ "error": "bad_request" }))
}

/// 一条声明（`validateDeclaration` 的产物）。
///
/// ⚠ JS 那一份还带 `provider`（预设 id）：它唯一的用途是"窗口留空、探测也没读到"时去
/// **社区兜底表**里**限家**查一条。壳里**没有端那张表**（那份数据是社区维护的静态表，
/// 嵌一份就是造第二个权威）⇒ 那一路如实落到"请用户自己填"（`window_unavailable`）✓，
/// `provider` 因此**只被接受、不起作用**（它不影响任何别的判据）。
struct Declaration {
    endpoint: String,
    credential: String,
    model: String,
    context_window: Option<i64>,
    protocol: Option<String>,
    name: Option<String>,
    bind: bool,
    /// **这一份模型收不收得到媒体材料**（2026-10-17）：**三态** —— `None` = 页面**没说**
    /// （老页面）⇒ 由 `upsert_entry` 从旧条目原样带过来；`Some(_)` = 说了。
    vision: Option<bool>,
}

/// 校验一条声明（照 `validateDeclaration`，**逐条同序**：地址 → 模型 → 名字 → 绑 → 窗口 → 协议 → 钥匙）。
fn validate_declaration(
    body: &Value,
    fallback_key: Option<&str>,
) -> Result<Declaration, &'static str> {
    if !body.is_object() {
        return Err("bad_request");
    }
    let text = |key: &str| {
        body.get(key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .unwrap_or("")
            .to_string()
    };
    let endpoint = text("endpoint");
    if endpoint.is_empty() || !endpoint_is_http(&endpoint) {
        return Err("invalid_endpoint");
    }
    let model = text("model");
    if model.is_empty() {
        return Err("invalid_model");
    }
    // 可选的 name：形状不对就**明确拒绝**，不猜。
    let name = match body.get("name") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if value.trim().is_empty() => None,
        Some(Value::String(value)) => Some(value.trim().to_string()),
        Some(_) => return Err("invalid_name"),
    };
    // 可选的"存完要不要绑"：形状不对 ⇒ 明确拒绝（不把 `"false"` 这种字符串当真）。
    let bind = match body.get("bind") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(value)) => *value,
        Some(_) => return Err("invalid_bind"),
    };
    let raw_window = match body.get("contextWindow") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(value)) => value.trim().to_string(),
        Some(Value::Number(value)) => value.to_string(),
        Some(_) => String::new(),
    };
    let context_window = if raw_window.is_empty() {
        None
    } else {
        if !window_shape_ok(&raw_window) || raw_window.parse::<i64>().unwrap_or(0) <= 0 {
            return Err("invalid_window");
        }
        Some(raw_window.parse::<i64>().unwrap_or(0))
    };
    let protocol = match body.get("protocol") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if value.trim().is_empty() => None,
        Some(Value::String(value)) if PROTOCOLS.contains(&value.as_str()) => {
            Some(value.to_string())
        }
        Some(_) => return Err("invalid_protocol"),
    };
    let given = body
        .get("apiKey")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let credential = if given.trim().is_empty() {
        fallback_key.unwrap_or("").to_string()
    } else {
        given.to_string()
    };
    if credential.trim().is_empty() {
        return Err("key_required");
    }
    // 可选的"这一份模型收不收得到媒体材料"（照 `validateDeclaration`，**位置也同一处**：
    // 钥匙之后）：**三态** —— 缺 / `null` = 页面**没说**（老页面）⇒ 写侧从旧条目原样带过来；
    // 给了就必须是布尔（非布尔 ⇒ 明确拒绝：半个形状比缺席更坏，照着重填会写出错的清单）。
    let vision = match body.get("vision") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => return Err("invalid_vision"),
    };
    Ok(Declaration {
        endpoint,
        credential,
        model,
        context_window,
        protocol,
        name,
        bind,
        vision,
    })
}

/// 地址必须是 http/https 的绝对地址（`new URL` + 协议两值）。
fn endpoint_is_http(endpoint: &str) -> bool {
    let lower = endpoint.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return false;
    }
    // `http://` 之后必须有主机名（`http://` 本身不是地址）。
    let rest = endpoint
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or("");
    !rest.split('/').next().unwrap_or("").is_empty()
}

/// 这一条存的是**哪一格**（照 `pickEntryId`）。
struct PickedEntry {
    id: String,
    previous: Option<crate::model_sources::ModelSourceEntry>,
    creating: bool,
}

fn pick_entry_id(
    body: &Value,
    entries: &[crate::model_sources::ModelSourceEntry],
    bound: Option<&str>,
    create: bool,
) -> Result<PickedEntry, &'static str> {
    use crate::model_sources::{is_entry_id, next_entry_id};
    let raw = body
        .get("id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if create {
        // 新建：id 由**门**给（单调不回收）；调用方指定 id 一律拒绝 —— 那是新建，不是覆盖。
        if !raw.is_empty() {
            return Err("bad_request");
        }
        return Ok(PickedEntry {
            id: next_entry_id(entries),
            previous: None,
            creating: true,
        });
    }
    if !raw.is_empty() && !is_entry_id(&raw) {
        return Err("invalid_id");
    }
    let previous = if raw.is_empty() {
        None
    } else {
        Some(
            entries
                .iter()
                .find(|entry| entry.id == raw)
                .ok_or("entry_not_found")?
                .clone(),
        )
    };
    // `PUT` 没给 id ⇒ **当前绑定的那一条**（老页面那个"保存"就是它）；连绑定都没有 ⇒ 新建。
    let id = if !raw.is_empty() {
        raw
    } else if let Some(bound) = bound {
        bound.to_string()
    } else {
        next_entry_id(entries)
    };
    let target = previous
        .clone()
        .or_else(|| entries.iter().find(|entry| entry.id == id).cloned());
    Ok(PickedEntry {
        creating: target.is_none(),
        previous: target,
        id,
    })
}

/// 清单里绑着 `id` 的那一支（删条目时的挡板：一支绑着就不许删）。
fn bound_anywhere(facts: &LocalFacts, id: &str) -> Option<String> {
    for (_, env_root) in env_roots_with_ids(&facts.home_dir) {
        if let crate::home_facts::ModelBinding::Bound { source, .. } =
            crate::home_facts::read_model_binding(&env_root)
        {
            if source == id {
                return Some(source);
            }
        }
    }
    None
}

/// 解除绑定：**每一支**都摘掉（撤销式：查无也成功）。
fn clear_binding_everywhere(facts: &LocalFacts) -> Result<(), String> {
    clear_binding_to(facts, &write_targets_of(facts, None))
}

/// `agentId` 的长度上界（与 JS 那一侧同一个数）。
const AGENT_ID_MAX: usize = 200;

/// **`agentId` 的形状**：它必须是一个**名字**（字母数字起头，后面只许字母数字 / `.` / `_` / `-`）。
///
/// 为什么是白名单而不是黑名单：这一格的值会**变成一个目录名**（`<家>/agents/<id>/`）——
/// 黑名单要同时挡住空白、控制字符、路径分隔符、冒号、`..`、以及各平台自己的怪东西，
/// 漏一个就是一条目录穿越；白名单一个字符集就把它关死了（真身份是 UUID，本来就落在这里）。
///
/// ⚠ **与 JS 那一侧逐字同形**（`scripts/home/config-server.js` 的 `agentIdShapeOk`）：
/// 两个生产者对同一个输入必须给同一个答案。长度按**字节**数（白名单全是 ASCII ⇒ 没有歧义）。
fn agent_id_shape_ok(value: &str) -> bool {
    let id = value.trim();
    if id.is_empty() || id.len() > AGENT_ID_MAX {
        return false;
    }
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    id.chars()
        .all(|each| each.is_ascii_alphanumeric() || each == '.' || each == '_' || each == '-')
}

/// **这一笔写落在哪几支上**（2026-10-12 住户级覆盖；2026-10-14「选择先于接上」扩了一格；
/// 与 JS 门 `config-server.js` 的 `writeTargetsOf` 同一条口径）：
///
/// - `agent_id` **没给** ⇒ 这台机器上的**每一支**（家级默认）；
/// - `agent_id` **给了** ⇒ **只写那一支**（一支一份绑定本来就是住户级事实，缺的只是"只给某一支换"）；
/// - `agent_id` **给了、而这台机器上还没有这一支** ⇒ 写**她将来会住的那个环境根**
///   （`<家>/agents/<id>/`）：**选择先于接上** —— 绑定的落点本来就是"她的环境根"，
///   而环境根要等接她第 2 步才建。⚠ 这一格曾经是拒绝（`no_such_resident`），于是
///   "先给她选好、再接上"这条路在页面上走不通（2026-10-14 主人撞上：机器上有两条来源，
///   而接她的那一步按"选哪一条 = 花谁的钱，不替她决定"停在原地，两边互等）。
///
///   ⚠ **形状不合法不许走到这里**（调用方先过 [`agent_id_shape_ok`]，那一格回 `bad_agent_id`）。
///   ⚠ 空目录**不算住户**（住户判据要 `agent.conf` 或 `identity.json`）⇒ 写下的这一笔
///   不会凭空造出一支住户来（写失败时留下的空目录同理，与 `anjia` 侧
///   `bind_first_entry_if_unbound` 同款）。
fn write_targets_of(facts: &LocalFacts, agent_id: Option<&str>) -> Vec<std::path::PathBuf> {
    let residents = env_roots_with_ids(&facts.home_dir);
    let Some(wanted) = agent_id else {
        return residents
            .into_iter()
            .map(|(_, env_root)| env_root)
            .collect();
    };
    let mine: Vec<std::path::PathBuf> = residents
        .iter()
        .filter(|(id, _)| id.as_deref() == Some(wanted))
        .map(|(_, env_root)| env_root.clone())
        .collect();
    if !mine.is_empty() {
        return mine;
    }
    vec![crate::home_facts::agent_env_root(&facts.home_dir, wanted)]
}

/// 写绑定到**指定的那几支**上（`write_targets_of` 的消费者）。
///
/// `overrides` 是**支级覆盖那两格**（2026-10-14；三态见
/// [`crate::model_sources::BindingOverrides`]），缺省 = 不动它们（旧行为一字不变）。
///
/// ⚠ **一支写成就记一条见证**（不是等整圈写完再记）：半途失败时，"真被改过的那几份"
/// 必须都有见证 —— 否则它们会读成"别处有人动过它"（把第一档报成第二档）。
fn write_binding_to(
    facts: &LocalFacts,
    env_roots: &[std::path::PathBuf],
    source: &str,
    overrides: &crate::model_sources::BindingOverrides,
) -> Result<(), String> {
    for env_root in env_roots {
        crate::model_sources::write_binding_with(env_root, source, overrides)?;
        witness_write(facts, &env_root.join(crate::model_sources::BINDING_FILE));
    }
    Ok(())
}

/// 摘掉**指定的那几支**的绑定（撤销式：查无也成功）。**"清"也是一笔写**（见 [`witness_clear`]）。
fn clear_binding_to(facts: &LocalFacts, env_roots: &[std::path::PathBuf]) -> Result<(), String> {
    for env_root in env_roots {
        crate::model_sources::clear_binding(env_root)?;
        witness_clear(facts, &env_root.join(crate::model_sources::BINDING_FILE));
    }
    Ok(())
}

/// `POST` 新建 / `PUT` 更新（照 `config-server.js` 的那一段，**顺序与判据一字不动**）。
fn config_write(facts: &LocalFacts, method: &str, body_text: &str) -> (u16, Value) {
    use crate::model_sources::{upsert_entry, ModelSourceEntry};
    let creating = method == "POST";
    let body = match parse_body(body_text) {
        Ok(body) => body,
        Err(error) => return (400, error),
    };
    let before = config_snapshot(facts);
    if before["sourcesState"] == "unreadable" {
        return (500, json!({ "error": "read_failed" }));
    }
    let entries = crate::model_sources::read_list(&facts.home_dir)
        .entries()
        .to_vec();
    let bound = before["bound"].as_str().map(|value| value.to_string());
    let picked = match pick_entry_id(&body, &entries, bound.as_deref(), creating) {
        Ok(picked) => picked,
        Err(error) => {
            let status = if error == "entry_not_found" { 404 } else { 400 };
            return (status, json!({ "error": error }));
        }
    };
    // "留空 key 沿用"只对**更新**成立（改哪一条就沿用哪一条的钥匙，不用重贴）。
    let fallback = picked.previous.as_ref().map(|entry| entry.api_key.clone());
    let declaration = match validate_declaration(&body, fallback.as_deref()) {
        Ok(declaration) => declaration,
        Err(error) => return (400, json!({ "error": error })),
    };
    // ---- 窗口（不是配置，是体检）----
    //
    // 留空 ⇒ **先试连再落盘**（窗口要进文件，所以不能等写完了才问）；问到就落盘、问不到才回头要人填。
    // 给了窗口 ⇒ 顺序与从前一字未改：先落盘、再试连读数（"试连失败不拦保存"照旧成立）。
    let mut context_window = declaration.context_window;
    let mut window_source = declaration.context_window.map(|_| "given");
    let mut result: Option<Value> = None;
    if context_window.is_none() {
        let probe = probe_models(
            &declaration.endpoint,
            &declaration.credential,
            &declaration.model,
        );
        match probe.get("contextWindow").and_then(|value| value.as_i64()) {
            Some(window) => {
                context_window = Some(window);
                window_source = Some("probed");
            }
            None => {
                // 探测没读到 ⇒ 这时才报错。★ **病因分开**：钥匙被对方拒（401/403）与该接口
                // 不给窗口是两件事，用户该做的**相反** —— 前者去换钥匙（手填窗口照样连不上），
                // 后者才该手填窗口。试连读数一并交回去：人要看的正是"为什么没问到"。
                let rejected = matches!(
                    probe.get("status").and_then(|value| value.as_u64()),
                    Some(401) | Some(403)
                );
                let error = if rejected {
                    "credential_rejected"
                } else {
                    "window_unavailable"
                };
                return (400, json!({ "error": error, "probe": probe }));
            }
        }
        result = Some(probe);
    }
    let name = match declaration.name {
        Some(name) => name,
        None => picked
            .previous
            .as_ref()
            .map(|entry| entry.name.clone())
            .unwrap_or_else(|| declaration.model.clone()),
    };
    let entry = ModelSourceEntry {
        id: picked.id.clone(),
        name,
        endpoint: declaration.endpoint.clone(),
        api_key: declaration.credential.clone(),
        model: declaration.model.clone(),
        context_window: context_window.unwrap_or(0),
        // ⚠ **思考强度暂时落空**（2026-10-08）：页面表单**还没有这一格**，因此载荷里今天发不出它
        // ⇒ 这里如实 `None`（不是"默认档"，是"这一路还没有表达方式"）。⚠ 页面表单落地那一刀
        // **必须同时**把它从这里接上，否则表单填了也会被这里悄悄丢掉（与"写者漏字段 = 删事实"同族）。
        reasoning_effort: None,
        protocol: declaration.protocol.clone(),
        // **媒体材料**（2026-10-17）：这一格**页面今天有**（表单上那个勾）⇒ 这里给的是"页面说了
        // 什么"；**三态由 `upsert_entry` 收口** —— 来件**没说**（老页面）时它从**旧条目原样带
        // 过来**，说了（哪怕 `false`）就以它为准。⚠ 别在这一格上"缺省 `false`"了事：那会把用户
        // 开着的视觉在每一次保存里**静默关掉**（与上一格同族，但那一格今天还没有表达方式）。
        vision: declaration.vision.unwrap_or(false),
    };
    if upsert_entry(&facts.home_dir, &entry, declaration.vision).is_err() {
        return (500, json!({ "error": "save_failed" }));
    }
    // 清单被这道门改过了 ⇒ 记一条（这一刻它的字节就是刚写下去的那一份）。
    witness_write(facts, &crate::model_sources::list_path(&facts.home_dir));
    // **绑不绑由 `bind` 说**：缺省 true = 老页面那句"保存 = 用它"；清单页发 false。
    if declaration.bind {
        // 落点与"没点名的那一笔绑定"**同一个判据**（`write_targets_of(None)` = 这台机器上的每一支）。
        if write_binding_to(
            facts,
            &write_targets_of(facts, None),
            &picked.id,
            &crate::model_sources::BindingOverrides::default(),
        )
        .is_err()
        {
            return (500, json!({ "error": "save_failed" }));
        }
    }
    // 试连：**保存已经落定了**，这一步只出读数（失败不拦保存，也不回滚）。
    if result.is_none() {
        result = Some(probe_models(
            &declaration.endpoint,
            &declaration.credential,
            &declaration.model,
        ));
    }
    let mut payload = config_snapshot(facts);
    if let Some(object) = payload.as_object_mut() {
        object.insert("entryId".to_string(), json!(picked.id));
        object.insert("creating".to_string(), json!(picked.creating));
        object.insert("windowSource".to_string(), json!(window_source));
        object.insert("probe".to_string(), result.unwrap_or(Value::Null));
    }
    (200, payload)
}

/// `DELETE /home-config`：带 `id` = 删条目（被绑定挡住 ⇒ 409）；**不带 id = 解除绑定**。
fn config_delete(facts: &LocalFacts, id: Option<&str>) -> (u16, Value) {
    use crate::model_sources::RemoveOutcome;
    let Some(id) = id.filter(|value| !value.is_empty()) else {
        // **不带 id 的 DELETE = 解除绑定**（旧页面"删除配置"那个动作的干净对应：
        // 它的文案本来就是"删除之后 Agent 走环境变量"）。撤销式 ⇒ 查无也成功。
        if clear_binding_everywhere(facts).is_err() {
            return (500, json!({ "error": "save_failed" }));
        }
        let mut payload = config_snapshot(facts);
        if let Some(object) = payload.as_object_mut() {
            object.insert("probe".to_string(), Value::Null);
        }
        return (200, payload);
    };
    // 带 id = 删清单条目。**被绑定挡住就拒绝**，并把"被谁绑着"如实说出来。
    if let Some(bound_to) = bound_anywhere(facts, id) {
        return (409, json!({ "error": "entry_bound", "boundTo": bound_to }));
    }
    match crate::model_sources::remove_entry(&facts.home_dir, id, None) {
        RemoveOutcome::Removed => {
            // 清单被这道门改过了 ⇒ 记一条（这一刻它的字节就是刚写下去的那一份）。
            witness_write(facts, &crate::model_sources::list_path(&facts.home_dir));
            let mut payload = config_snapshot(facts);
            if let Some(object) = payload.as_object_mut() {
                object.insert("probe".to_string(), Value::Null);
            }
            (200, payload)
        }
        RemoveOutcome::NotFound => (404, json!({ "error": "entry_not_found" })),
        RemoveOutcome::Bound { bound_to } => {
            (409, json!({ "error": "entry_bound", "boundTo": bound_to }))
        }
        RemoveOutcome::Failed { .. } => (500, json!({ "error": "save_failed" })),
    }
}

/// `PUT /home-binding`：换绑 / 解绑（空 `source` = 解除）。
fn binding_write(facts: &LocalFacts, body_text: &str) -> (u16, Value) {
    let body = match parse_body(body_text) {
        Ok(body) => body,
        Err(error) => return (400, error),
    };
    let source = body
        .get("source")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    // `agentId`（可选）：**只给这一支**换 / 解（住户级覆盖）；不给 = 这台机器上的每一支（家级默认）。
    let agent_id = body
        .get("agentId")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    // ★ **点名的那一支先过形状**（2026-10-14）：它接下来会拼进路径（`<家>/agents/<id>/`）
    //   ⇒ 形状不合法一律拒，不拼、不猜、不回落"写每一支"。
    if let Some(wanted) = agent_id {
        if !agent_id_shape_ok(wanted) {
            return (400, json!({ "error": "bad_agent_id" }));
        }
    }
    // ★ **一台机器上一支住户都没有 ⇒ 这件事没有目标**（"落在每一支上" = 落在**零支**上）。
    //   不许静默成功：没有可绑/可解的那一支，就该说清（与 JS 门那一侧**同一个码、同一个状态**：
    //   `scripts/home/config-server.js` 的 `ERRORS.NO_RESIDENT`，客户端那份封闭表逐字对齐）。
    //   ⚠ **2026-10-14**：这一条只对"**没点名**"的那一笔成立 —— 点了名的（哪怕这台机器上
    //   一支住户都还没有）有目标：写在她将来会住的那个环境根上（"选择先于接上"）。
    if agent_id.is_none() && env_roots_with_ids(&facts.home_dir).is_empty() {
        return (409, json!({ "error": "no_resident" }));
    }
    let targets = write_targets_of(facts, agent_id);
    // ★ **支级覆盖那两格**（2026-10-14）："用哪个型号 / 什么思考档"**归这一支**（运行时一直认它们：
    //   `agent/src/model_source.rs` 的 `apply_binding_overrides`），而在此之前**没有任何写者**
    //   能写它们。三态：**不给** = 不动（重新绑定时沿用旧值）；`null` = 清掉；字符串 = 写成它。
    //
    //   ⚠ **只在点名 `agentId` 时接受**：那两格是**支级事实**，而"没点名 = 落在每一支上"的家级
    //   默认把它们写成"每一支都用同一个型号"，那不是任何人要的（不猜 ⇒ 明确拒）。
    let has_overrides = body.get("model").is_some() || body.get("reasoningEffort").is_some();
    if has_overrides && (agent_id.is_none() || source.is_empty()) {
        // 没点名（家级默认）或"解除绑定" + 覆盖两格 = 自相矛盾（文件要删了，覆盖没有落点）。
        return (400, json!({ "error": "bad_request" }));
    }
    let mut overrides = crate::model_sources::BindingOverrides::default();
    if has_overrides {
        if let Some(value) = body.get("model") {
            match value {
                Value::Null => overrides.model = Some(None),
                Value::String(text) if model_field_shape_ok(text) => {
                    overrides.model = Some(Some(text.trim().to_string()))
                }
                _ => return (400, json!({ "error": "invalid_model" })),
            }
        }
        if let Some(value) = body.get("reasoningEffort") {
            match value {
                Value::Null => overrides.reasoning_effort = Some(None),
                Value::String(text)
                    if crate::model_sources::REASONING_EFFORTS.contains(&text.trim()) =>
                {
                    overrides.reasoning_effort = Some(Some(text.trim().to_string()))
                }
                _ => return (400, json!({ "error": "invalid_effort" })),
            }
        }
    }
    if source.is_empty() {
        if clear_binding_to(facts, &targets).is_err() {
            return (500, json!({ "error": "save_failed" }));
        }
        return (200, config_snapshot(facts));
    }
    let list = crate::model_sources::read_list(&facts.home_dir);
    if list.kind() == "unreadable" {
        return (500, json!({ "error": "read_failed" }));
    }
    if !list.entries().iter().any(|entry| entry.id == source) {
        return (404, json!({ "error": "entry_not_found" }));
    }
    if write_binding_to(facts, &targets, &source, &overrides).is_err() {
        return (500, json!({ "error": "save_failed" }));
    }
    (200, config_snapshot(facts))
}

/// 型号那一格的形状（`scripts/home/model-sources.js` 的 `modelShapeOk` **逐条同形**）。
fn model_field_shape_ok(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= crate::model_sources::MODEL_MAX_BYTES
        && !value.chars().any(|each| each.is_control())
}

/// `POST /home-config/catalog`：**用存着的那把钥匙**问一次"这条能调哪些型号"（2026-10-14）。
///
/// 三条纪律与 [`probe_action`] **逐条相同**（只读 / 响应里没有 key / 连不上是读数不是错误），
/// 唯一的差别是**钥匙从哪儿来**：这里从清单里点名的那一条读（请求体只有 `id`）。
fn catalog_action(facts: &LocalFacts, body_text: &str) -> (u16, Value) {
    let body = match parse_body(body_text) {
        Ok(body) => body,
        Err(error) => return (400, error),
    };
    let id = body
        .get("id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if id.is_empty() {
        return (400, json!({ "error": "bad_request" }));
    }
    let list = crate::model_sources::read_list(&facts.home_dir);
    if list.kind() == "unreadable" {
        return (500, json!({ "error": "read_failed" }));
    }
    let Some(entry) = list.entries().iter().find(|entry| entry.id == id) else {
        return (404, json!({ "error": "entry_not_found" }));
    };
    // **兼容端点那条**（`GET <基址>/models`）：原生目录层要"是哪一家"，而条目里没有那一格
    // ⇒ 不猜、不假装（与 JS 门那一侧同一句）。
    (
        200,
        json!({ "probe": probe_models(&entry.endpoint, &entry.api_key, &entry.model) }),
    )
}

/// `POST /home-config/probe`：**只读探测**（探测即测试，保存才落盘）。
///
/// 三条纪律：**不写任何文件**；**响应里没有 key**（只有那份读数）；**试连失败是读数不是错误**
/// （连不上 / 超时照样 200，`probe.ok === false` 让页面如实说"这家现在连不上"）——
/// 只有**请求形状**不对（地址读不懂 / 没给钥匙 / 协议不在闭集）才 4xx。
fn probe_action(body_text: &str) -> (u16, Value) {
    let body = match parse_body(body_text) {
        Ok(body) => body,
        Err(error) => return (400, error),
    };
    if !body.is_object() {
        return (400, json!({ "error": "bad_request" }));
    }
    let text = |key: &str| {
        body.get(key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .unwrap_or("")
            .to_string()
    };
    let endpoint = text("endpoint");
    if endpoint.is_empty() || !endpoint_is_http(&endpoint) {
        return (400, json!({ "error": "invalid_endpoint" }));
    }
    // 线协议：给了就必须落在闭集里（**只做形状预检**：探测打的是 `/models`，与线协议无关）。
    match body.get("protocol") {
        None | Some(Value::Null) => {}
        Some(Value::String(value)) if value.trim().is_empty() => {}
        Some(Value::String(value)) if PROTOCOLS.contains(&value.as_str()) => {}
        Some(_) => return (400, json!({ "error": "invalid_protocol" })),
    }
    // 可选的模型名：给了就顺手把**它**的标称窗口读回来（复用同一个试连）；不给就是纯拉目录。
    let model = text("model");
    // key 只从请求体来（没有"留空沿用那一条"这回事：探测的语义就是"用**这一把**试一试"）。
    let credential = body
        .get("api_key")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if credential.is_empty() {
        return (400, json!({ "error": "key_required" }));
    }
    (
        200,
        json!({ "probe": probe_models(&endpoint, &credential, &model) }),
    )
}

// ---------------------------------------------------------------------------
// 试连（兼容端点那条：`GET <基址>/models`）
// ---------------------------------------------------------------------------

/// `modelsUrl` 的直译：摘掉线协议那一段，再拼 `/models`。
///
/// ⚠ **端点是完整地址**（Rust 侧口径：`https://api.deepseek.com/chat/completions`）⇒
/// 直接 `endpoint + '/models'` 会探到不存在的路径。
fn models_url(endpoint: &str) -> String {
    let trimmed = endpoint.trim_end_matches('/');
    let stripped = trimmed
        .strip_suffix("/chat/completions")
        .or_else(|| trimmed.strip_suffix("/completions"))
        .or_else(|| trimmed.strip_suffix("/responses"))
        .unwrap_or(trimmed);
    format!("{stripped}/models")
}

/// 试连一次，交回那份**读数**（形状照 `probeModels`：`url / ok / status / reason / contextWindow /
/// windowField / models`）。**这个载荷里没有那把钥匙**。
///
/// ⚠ 与 JS 的差别（**有意的、如实登记的**）：JS 在兼容端点之前还会试**各家的原生目录**
/// （`probeNative`，只有百炼那一家有），那层是"锦上添花"（它的注释原话），拿不到就退回这里。
/// 壳里不端那层：**原生目录少给的，这里也少给**，不假装。
fn probe_models(endpoint: &str, credential: &str, model: &str) -> Value {
    let url = models_url(endpoint);
    let no_window = |url: &str, ok: bool, status: Option<u16>, reason: Option<&str>| {
        json!({
            "url": url,
            "ok": ok,
            "status": status,
            "reason": reason,
            "contextWindow": null,
            "windowField": null,
            "models": [],
        })
    };
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(PROBE_TIMEOUT_MS))
        .build()
    {
        Ok(client) => client,
        Err(_) => return no_window(&url, false, None, Some("unreachable")),
    };
    let response = client
        .get(&url)
        .header("authorization", format!("Bearer {credential}"))
        .header("accept", "application/json")
        .send();
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            let reason = if error.is_timeout() {
                "timeout"
            } else {
                "unreachable"
            };
            return no_window(&url, false, None, Some(reason));
        }
    };
    let status = response.status().as_u16();
    let ok = response.status().is_success();
    if !ok {
        // 回执读不读得懂都不影响这一格：**失败是读数**（`ok:false` + 状态码）。
        return no_window(&url, false, Some(status), None);
    }
    let body: Value = match response.json() {
        Ok(body) => body,
        Err(_) => return no_window(&url, true, Some(status), None),
    };
    let (context_window, window_field) = window_from_models(&body, model);
    json!({
        "url": url,
        "ok": true,
        "status": status,
        "reason": null,
        "contextWindow": context_window,
        "windowField": window_field,
        "models": models_from_body(&body),
    })
}

/// 目录条目数组（三种形状：回执本身是数组 / `data` / `models`）—— **只有这一处认识目录长什么样**。
fn catalog_entries(body: &Value) -> Option<&Vec<Value>> {
    if let Some(array) = body.as_array() {
        return Some(array);
    }
    for key in ["data", "models"] {
        if let Some(array) = body.get(key).and_then(|value| value.as_array()) {
            return Some(array);
        }
    }
    None
}

/// 从回执里读**模型名**（保序、去重、封顶 200）。
fn models_from_body(body: &Value) -> Vec<String> {
    let Some(entries) = catalog_entries(body) else {
        return Vec::new();
    };
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        let raw = ["id", "name", "model"]
            .iter()
            .filter_map(|key| entry.get(key).and_then(|value| value.as_str()))
            .map(str::trim)
            .find(|value| !value.is_empty());
        let Some(name) = raw else { continue };
        if !names.iter().any(|each| each == name) {
            names.push(name.to_string());
        }
        if names.len() >= MODEL_LIST_LIMIT {
            break;
        }
    }
    names
}

/// 从回执里读**标称窗口**（只读点名的字段，并如实回是哪一件命中的）。
fn window_from_models(body: &Value, model: &str) -> (Option<i64>, Option<String>) {
    if model.is_empty() {
        return (None, None);
    }
    let Some(entries) = catalog_entries(body) else {
        return (None, None);
    };
    let entry = entries.iter().find(|entry| {
        ["id", "name", "model"]
            .iter()
            .any(|key| entry.get(key).and_then(|value| value.as_str()) == Some(model))
    });
    let Some(entry) = entry else {
        return (None, None);
    };
    for field in WINDOW_FIELDS {
        if let Some(window) = positive_integer(value_at(entry, field)) {
            return (Some(window), Some(field.to_string()));
        }
    }
    (None, None)
}

/// 点号路径取值（`top_provider.context_length`）。
fn value_at<'a>(value: &'a Value, dotted: &str) -> Option<&'a Value> {
    let mut current = value;
    for part in dotted.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

/// 只认正整数（或恰好是数字的字符串）；别的形状一律当"这一格没给"。
fn positive_integer(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(number) => number.as_i64().filter(|value| *value > 0),
        Value::String(text) => {
            let trimmed = text.trim();
            if window_shape_ok(trimmed) {
                trimmed.parse::<i64>().ok().filter(|value| *value > 0)
            } else {
                None
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// HTTP：两条路由，别的都不认
// ---------------------------------------------------------------------------

fn handle_status_connection(mut stream: TcpStream, home_dir: &Path) -> std::io::Result<()> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    // 头读完就够（这一片没有请求体：两条路由都是 GET）。
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("");
    let path = target.split('?').next().unwrap_or("");

    let (status, body) = match (method, path) {
        ("GET", "/healthz") => (200, json!({ "ok": true })),
        ("GET", "/status") => (200, status_payload(&LocalFacts::load(Some(home_dir), None))),
        (_, "/healthz") | (_, "/status") => (405, json!({ "error": "method_not_allowed" })),
        _ => (404, json!({ "error": "not_found" })),
    };
    write_json(&mut stream, status, &body)
}

// ---------------------------------------------------------------------------
// 载荷（形状权威：`status-server.js` 的 `buildStatusPayload`）
// ---------------------------------------------------------------------------

/// 上次那一份（TTL 缓存：一次探测 ~3.7 秒，页面不该每次进来都等它）。
fn cache() -> &'static Mutex<Option<(Instant, Value)>> {
    static CACHE: OnceLock<Mutex<Option<(Instant, Value)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// 组装 `GET /status` 的载荷。**只读读数，不含凭据。**
///
/// `stale`：TTL 内给的是上一次那一份（`readAt` 原样带着那次真读数的时刻，绝不改成"当下"）。
pub fn status_payload(facts: &LocalFacts) -> Value {
    let mut guard = match cache().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some((at, payload)) = guard.as_ref() {
        if at.elapsed() < Duration::from_millis(STATUS_TTL_MS) {
            let mut fresh = payload.clone();
            if let Some(object) = fresh.as_object_mut() {
                object.insert("stale".to_string(), json!(false));
            }
            return fresh;
        }
    }
    let payload = build_payload(facts);
    *guard = Some((Instant::now(), payload.clone()));
    payload
}

fn build_payload(facts: &LocalFacts) -> Value {
    let im = match WorldHttp::new(&facts.world_base()) {
        Ok(http) => {
            // 判据照 JS：**0 不是 HTTP 状态码** ⇒ 连不上时给 `null`（把"读不到"说成一个具体读数
            // 是最容易犯的错：页面上会出现"没在跑（HTTP 0）"）。
            //
            // ⚠ **派生字段 `listening` 已删**（2026-10-11，与 `status-server.js` 同一刀）：
            // 它是 `health == Some(200)`，而页面拿它当主词 ⇒ 「世界活着、只是自报 500」会被写成
            // 「没在跑（HTTP 500）」。**有应答就证明那个口上有人在** ⇒ 那是"在跑"，不是"没在跑"；
            // `None` 才是"它没答话"（读不到，不等于它停了）。一个事实一个权威。
            let health = http.health_code();
            json!({ "health": health.filter(|code| *code > 0) })
        }
        Err(_) => json!({ "health": null }),
    };

    // **住户清单**（★ 2026-10-08）：先整份拿到手，再挑第一支 —— 清单本身要进载荷（见下）。
    let residents = local_residents::residents(&facts.home_dir);
    // **第一支住户**（按住户序号；`residents.js` 的次序）——顶层那几格（`agent` / `lastWake` /
    // `host`）是它的（与 `status-server.js` 同一条规矩：第一支抬到顶层，冻结前端照旧读）。
    let resident = residents
        .iter()
        .min_by_key(|each| each.conf.slot.unwrap_or(u32::MAX));

    let (mut agent, last_wake) = match resident {
        Some(resident) => (
            read_agent_status(
                &facts.layout.status_exe,
                &resident.env_root,
                &resident.agent_id,
            ),
            last_wake_of(&resident.log_file),
        ),
        // **这个家里一支住户都没有** —— 与"二进制不在"是两件事（`status-server.js` 同款，
        // 2026-10-05 起分开说）：从前两者共用 `missing-binary`，页面于是把"还没有 Agent"
        // 说成"本机缺少必要的程序"（真机撞到）。判据：**缺省不等于故障**。
        None => (json!({ "state": "no-resident" }), None),
    };
    // **Agent 的身份**（环境根 `identity.json` 的记录）——刻意放在这一层、与 Agent 的**状态无关**：
    // 「这一轮在忙」「二进制缺」「账本读不出来」都不该让页面答不出"这是哪个 Agent 的账本"
    // （页面的「在线」那一格靠 `agentAgentMatch` 才敢说"同一支 / 不是这一支"，
    //  没有它就只能说"不知道"—— 那正是 app 里今天显示"状态未知"的原因）。
    if let Some(resident) = resident {
        if let Some(id) = env_identity(&resident.env_root) {
            if let Some(object) = agent.as_object_mut() {
                object.insert("agentId".to_string(), json!(id));
            }
        }
    }

    let mut payload = json!({
        "readAt": now_iso8601(),
        "stale": false,
        "im": im,
        "agent": agent,
        "lastWake": last_wake,
        // `experience`：**这一刀不给**（那套投影的权威在 `status-server.js`，
        // 壳里抄第二份就是造第二个权威）——页面**从 `agent.runs` 降级渲染**（2026-10-05）。
    });
    // ★ **宿主那一格**（2026-10-08）：**第一支**的（与 `status-server.js` 同一条规矩）。
    //   客户端契约里它早就有位置（`hostStateOf` 四态），而壳从来没给过它 ⇒ 页面在 app 里
    //   只能写「机器在不在读不到」—— 那一句读数**没有生产者**。这一格补上它。
    //   一支住户都没有时**整格不给**（缺省 = 这一面没读数；给 `null` 会让客户端把**整份**
    //   读数判成形状坏 —— 那条守卫写着"给了就必须认得出形状"）。
    // ★ **住户清单**（2026-10-08）：每一支的 id / 序号 / **它自己的宿主状态**。
    //   为什么必须有：上面那两格是**第一支**的，而页面拿它当"这台机器上那一支"用 ⇒
    //   第 2、3 支的页面被写成「还没有机器在跑它」——**假话**（它们就在这台机器上跑着，
    //   只是各住一个环境根）。判据：**认得出的形状不许说成不存在**（与 `legacyHome` 同条）。
    //   ⚠ 逐支的**账本 / 经历**不给：那两套投影的权威在 `status-server.js`（壳里抄第二份
    //   就是造第二个权威，与 `experience` 那条理由一样）⇒ 这里只给"住址 + 宿主"两格，
    //   页面据此说"它住在本机、它的宿主在不在跑"，**不许**把第一支的账本读数安到它头上。
    if let Some(object) = payload.as_object_mut() {
        if let Some(resident) = resident {
            object.insert("host".to_string(), host_view(resident, facts));
        }
        object.insert("residents".to_string(), roster(&residents, facts));
    }
    // **老家形态**那一格：只在**是**的时候出现（客户端守卫写着"多一个字段不管"）。
    if legacy_home_present(facts) {
        if let Some(object) = payload.as_object_mut() {
            object.insert("legacyHome".to_string(), json!(true));
            // 这一轮**为什么没搬成**（稳定原因码；人话归页面）—— 开机时那次迁移记下来的。
            if let Some(blocker) = crate::home_shape::last_blocker() {
                object.insert("legacyBlocker".to_string(), json!(blocker));
            }
        }
    }
    payload
}

/// **宿主那一格**的载荷形状（与 `status-server.js` 的 `host` **逐字同形**：`state` / `pid` / `log`）。
///
/// ⚠ **词表必须与 JS 门那一份相同**（`running` / `running-orphan` / `stopped` / `unknown`）：
/// 客户端按它归一（`hostStateOf`），**认不出的取值会整格读成"没读数"** —— 不报错，但也没信息。
/// 映射：在跑 ⇒ `running`；没在跑 ⇒ `stopped`；**核对不了 / pid 撞车 ⇒ `unknown`**
/// （如实说"这一格没读数"，**不编一个"没在跑"** —— 那是把读不到说成事实）。
fn host_view(resident: &local_residents::Resident, facts: &LocalFacts) -> Value {
    let state = local_residents::state_of(resident, &facts.layout.host_exe);
    let kind = match &state {
        local_residents::ResidentState::Running { .. } => "running",
        local_residents::ResidentState::NotRunning { .. } => "stopped",
        local_residents::ResidentState::NotOurs { .. }
        | local_residents::ResidentState::Unverifiable { .. } => "unknown",
    };
    // pid 照 JS 门那一份：没在跑给 `null`（撞车 / 核对不了时**照给**——那正是"这个号上坐着谁"
    // 的线索，而 `state: unknown` 已经说明"别据此动手"）。
    let pid = match &state {
        local_residents::ResidentState::Running { pid } => Some(*pid),
        local_residents::ResidentState::NotRunning { .. } => None,
        local_residents::ResidentState::NotOurs { pid, .. }
        | local_residents::ResidentState::Unverifiable { pid, .. } => Some(*pid),
    };
    let log = resident
        .log_file
        .strip_prefix(&facts.home_dir)
        .unwrap_or(&resident.log_file);
    json!({ "state": kind, "pid": pid, "log": log.display().to_string() })
}

/// **住户清单**：每一支的 id / 序号 / 宿主那一格（**逐支**，见 `build_payload` 里那段理由）。
fn roster(residents: &[local_residents::Resident], facts: &LocalFacts) -> Value {
    Value::Array(
        residents
            .iter()
            .map(|resident| {
                json!({
                    // 环境根 `identity.json` 的记录（认领的判据在客户端：`agentId` 精确相等）。
                    "agentId": env_identity(&resident.env_root),
                    "slot": resident.conf.slot,
                    "host": host_view(resident, facts),
                })
            })
            .collect(),
    )
}

/// **这个家还是住户化之前的形状吗**（环境根 `<家>/env` 那一支）——**只读，一个字都不动**。
///
/// 判据照 `residents.js` 的 `legacyResident`：`home.conf` 里有 `agent_id`，**或**
/// `<家>/env/identity.json` 里记着一支 —— 两处都没有 ⇒ 不是老家（全新的家也是"没有"）。
///
/// ⚠ **为什么要有这一格**（2026-10-05）：壳只认新布局 `<家>/agents/<支>/`，而把老家搬成
/// 新形态是 `up.js` 的 `migrateHomeShape`（**app 不做迁移**，见 `app/README.md` §5 第 9 行）
/// ⇒ 老家在壳里会落到"零住户"，页面于是说「**这台机器上还没有 Agent**」，而**她明明在**
/// （只是住在老形态里）。那是把"我认不出这个形状"说成"它不存在"。
/// 判据一句话：**认不出的形状不许说成不存在** —— 这一格只报事实，人话归页面。
///
/// ⚠ 两条读数**共用它**（`pub(crate)`）：状态口那一格（`GET /status` 的 `legacyHome`）与
/// 「本机服务」那一格（`ServicesSnapshot.legacy_home`）—— **同一屏两句话不能互相打脸**。
pub(crate) fn legacy_home_present(facts: &LocalFacts) -> bool {
    if crate::home_facts::read_conf(&facts.home_dir)
        .agent_id
        .is_some()
    {
        return true;
    }
    env_identity(&facts.home_dir.join("env")).is_some()
}

/// **环境根记的是哪一支**（`identity.json` 的 `{agent: <id>}`；`readEnvIdentity` 的直译）。
///
/// 严格照 JS：文件不在 / 读不懂 / `agent` 不是非空字符串 ⇒ `None`（**不猜**）。
///
/// ⚠ 2026-10-05：实现搬到 `home_facts::read_env_identity`（**一处权威**）—— 家的形状迁移
/// （`home_shape.rs`）也要问同一个问题，两边各留一份就会迟早对不上。
fn env_identity(env_root: &Path) -> Option<String> {
    crate::home_facts::read_env_identity(env_root)
}

/// 宿主日志里最后一行**在场读数**。
///
/// ⚠ 2026-10-10 在场那一刀：那一行的前缀从 `[wake]` 改成 `[run]`，内容行从 `wake run=…`
/// 变成 `run=…`（一段在场会吸收后来的每一声音铃，一次唤醒不再是"一段"）。
///
/// ⚠ **两个形状都认**（与 `tools/trace_view.py` 同一条纪律）：本函数读的是**盘上那份日志**，
/// 而它可能是**升级之前**写的（旧形状），也可能是升级之后写的（新形状）。只认一个形状的后果
/// 不是"少一行读数"，而是**把"读不到"渲染成"没有"**——机器页那一格会从「未知」变成
/// 「这一家一次都没醒过」，那是一句假话。⇒ **旧形状照读，不猜、不迁移。**
fn last_wake_of(log_file: &Path) -> Option<String> {
    let text = std::fs::read_to_string(log_file).ok()?;
    text.lines()
        .rfind(|line| line.starts_with("run=") || line.starts_with("wake "))
        .map(|line| line.trim_end().to_string())
}

fn now_iso8601() -> String {
    // 只为一个时刻，不引时间库：`readAt` 是给人看的读数，格式与 JS 的 `toISOString()` 同族。
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let millis = now.subsec_millis();
    let (year, month, day, hour, minute, second) = civil_from_unix(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// **此刻的毫秒**（见证账本记的那一格；JS 那侧是 `Date.now()`，同一个数）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// ★ **记一条见证**（2026-10-18）：这一笔写是**从这道门**出去的 ⇒ 记下"哪一份、什么时候、
/// 那一刻它的摘要"。两档（页面改的／别处改的）、不是封印、绝不拦写 —— 全在
/// [`crate::model_witness`] 的文件头。
///
/// ⚠ **缝在这道门上，不在 `model_sources::write_atomic` 上**：那一个函数也承载**不是页面**的写
/// （`home.js up` 的迁移、布置时的绑定携带）—— 在那里记账会把"布置"说成"页面改的"。
///
/// ⚠ **记不下绝不让这一笔写失败**：回一行 stderr，配置照旧是写成功的。
fn witness_write(facts: &LocalFacts, file: &std::path::Path) {
    if let Err(reason) = crate::model_witness::record_write(&facts.home_dir, file, now_ms()) {
        eprintln!("[见证] 这一笔没记下：{}（{reason}）", file.display());
    }
}

/// **"清"也是一笔写**：不记它，页面上的"解除指定"就会读成"别处有人动过它"（第一档报成第二档）。
fn witness_clear(facts: &LocalFacts, file: &std::path::Path) {
    if let Err(reason) = crate::model_witness::record_clear(&facts.home_dir, file, now_ms()) {
        eprintln!("[见证] 这一笔没记下：{}（{reason}）", file.display());
    }
}

/// Unix 秒 → 公历（UTC）。**不引时间库**：这一片只需要一个能被人读的时刻。
fn civil_from_unix(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hour, minute, second) = (
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    );
    // Howard Hinnant 的 civil_from_days（公历，无闰秒）。
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d, hour, minute, second)
}

// ---------------------------------------------------------------------------
// 「处境」：**委派给运行时自己的 CLI**（`agent_host_status`）
// ---------------------------------------------------------------------------

/// 跑一次 `agent_host_status` 并把它的两股输出收全，然后按 `host_status_parse.js` 解析。
fn read_agent_status(status_exe: &Path, env_root: &Path, agent_id: &str) -> Value {
    if !status_exe.is_file() {
        return json!({ "state": "missing-binary" });
    }
    let mut command = Command::new(status_exe);
    command
        .arg(env_root)
        .arg("--agent")
        .arg(agent_id)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear();
    for key in ALLOWED_ENV_KEYS {
        if let Ok(value) = std::env::var(key) {
            command.env(key, value);
        }
    }
    // **不给它控制台窗口**（2026-10-08）：它是控制台程序、壳没有控制台 ⇒ 少了这一行
    // 就是"每读一次运行状态闪一下黑框"。读数照旧走 `.output()` 的管道。
    crate::console_window::suppress(&mut command);
    let output = match command.output() {
        Ok(output) => output,
        // 起不来与"读不出来"同一档（照 JS：退出码不参与判读，判读全靠文本）。
        Err(_) => return json!({ "state": "unreadable", "reason": "spawn-failed" }),
    };
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    match parse_host_status(&text) {
        HostStatus::Busy => json!({
            "state": "busy",
            "note": "Agent 这一拍正在跑一次活动（环境同时只有一个写者）",
        }),
        HostStatus::Unreadable { reason } => json!({ "state": "unreadable", "reason": reason }),
        HostStatus::Loaded {
            admission,
            activities,
            runs,
            next,
        } => {
            let truncated = activities
                .map(|total| total as usize > runs.len())
                .unwrap_or(false);
            json!({
                "state": "loaded",
                "admission": admission,
                "activities": activities,
                "truncated": truncated,
                "runs": runs,
                "runsTotal": activities.unwrap_or(runs.len() as i64),
                "next": next,
            })
        }
    }
}

/// 解析结局（三态；`busy` 是**一等结果**，不是错误）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostStatus {
    Loaded {
        admission: String,
        activities: Option<i64>,
        runs: Vec<Value>,
        next: String,
    },
    /// Agent 这一轮正在跑一次活动（环境同时只有一个写者）。
    Busy,
    Unreadable {
        reason: String,
    },
}

/// `host_status_parse.js` 的直译。**严格**：认不出的形状一律 `unreadable`，绝不猜。
///
/// 最要紧的一条是**版本头**：认不出 `celira-agent-host-status v1` 就说"读不到账本"——
/// 那是 Agent 这一轮在忙，必须与"读不到（真故障）"分开。
pub fn parse_host_status(text: &str) -> HostStatus {
    if text.is_empty() {
        return HostStatus::Unreadable {
            reason: "empty".to_string(),
        };
    }
    if text.contains("already-open") {
        return HostStatus::Busy;
    }
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| line.trim_end_matches('\r').trim_end().to_string())
        .collect();
    if lines.first().map(String::as_str) != Some(HOST_STATUS_HEADER) {
        return HostStatus::Unreadable {
            reason: "unknown-header".to_string(),
        };
    }
    let state_at = lines.iter().position(|line| line == "[state]");
    let next_at = lines.iter().position(|line| line == "[next]");
    let (Some(state_at), Some(next_at)) = (state_at, next_at) else {
        return HostStatus::Unreadable {
            reason: "missing-state-section".to_string(),
        };
    };
    if next_at <= state_at {
        return HostStatus::Unreadable {
            reason: "missing-next-section".to_string(),
        };
    }
    let head = &lines[1..state_at];
    let section = &lines[state_at + 1..next_at];
    let Some(admission) = field(section, "admission").filter(|value| !value.is_empty()) else {
        return HostStatus::Unreadable {
            reason: "missing-admission".to_string(),
        };
    };
    let mut runs: Vec<Value> = Vec::new();
    for line in section {
        if line.is_empty() || line.starts_with("admission=") {
            continue;
        }
        // 认不出的行就是认不出：**宁可整体失败，也不安静跳过**
        // （跳过会让"少了一条活动"看起来像"Agent 本来就没跑过那一轮"）。
        let Some(run) = parse_run_line(line) else {
            return HostStatus::Unreadable {
                reason: "malformed-run-line".to_string(),
            };
        };
        runs.push(run);
    }
    // 明细只给最近几条（口子的职责是"看一眼处境"，不是导出账本）。
    if runs.len() > RECENT_RUNS {
        runs = runs.split_off(runs.len() - RECENT_RUNS);
    }
    let next = lines[next_at + 1..]
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    // `activities-page=<本页条数> of=<一共几条> scope=latest`（一行三段 ⇒ 必须按空白切）。
    // **取不到 `of=` 就是 `null`，绝不拿本页条数冒充总数。**
    let activities = segment(head, "activities-page", 1)
        .and_then(|value| value.strip_prefix("of=").map(|rest| rest.to_string()))
        .and_then(|value| value.parse::<i64>().ok());
    HostStatus::Loaded {
        admission,
        activities,
        runs,
        next,
    }
}

/// 取一个键的**第一段**（按空白切；整行比较会把后缀字段读进来）。
fn field(lines: &[String], name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    lines
        .iter()
        .find(|line| line.starts_with(&prefix))
        .map(|line| line[prefix.len()..].to_string())
}

fn segment(lines: &[String], name: &str, index: usize) -> Option<String> {
    let raw = field(lines, name)?;
    raw.split_whitespace()
        .nth(index)
        .map(|part| part.to_string())
}

/// `run=<id> state=<state> [reason=<reason>] [outcome=<outcome>] [failure=<class>] [unresolved=<n>]`
/// （照 `host_status_parse.js` 2026-10-05 修后的正则 —— 段的**名字与顺序**是 CLI 的写出顺序）。
///
/// ⚠ **`outcome=` 是可选的**：**未收尾的活动没有结果**（真机读数：同一个家 38 条活动里 12 条
/// 是这样，例 `run=host-13556-1791090187913-2 state=unfinished`）。要求它 ⇒ **一条未收尾活动
/// 就让整份读数被判 `malformed-run-line`** ⇒ 页面说"读不到账本"，而 Agent 其实好好的。
///
/// ⚠ **`reason=` 与 `unresolved=` 也是可选的**（2026-10-05 真机撞到，同一个坑第三次）：
/// CLI 的第三类判定长这样（那一行逐字抄自真输出）——
///
/// ```text
/// run=host-17680-1791175794423-1 state=unknown reason=dispatched-without-result unresolved=1
/// ```
///
/// 老解析器不认这两段 ⇒ 账本里只要有一条"已派发却没有结果"的活动，**整份读数**又变
/// "读不到"（app 与网页一起瞎，而 Agent 好好的）。两段**认得出、不带出来**（今天没有消费者）。
///
/// ⚠ **`stopped=` 也是可选的尾段**（2026-10-18 修，**同一个坑第四次**，`verify:ui:app` 真机撞到）：
/// CLI 2026-10-13 起在行尾多写这一段（为什么停在这里：`held` / `yielded` / `no-response` /
/// `unreadable`），真输出长这样 ——
///
/// ```text
/// run=host-33736-1792318397097-1 state=unfinished stopped=no-response
/// ```
///
/// 两份解析器（本镜像与 `host_status_parse.js`）都还停在六段上 ⇒ 账本里只要有一条说得出
/// 为什么停下的未收尾活动，**整份读数**又变"读不到"（机器页「运行」那一间整间瞎）。
/// 这一段同样**认得出、不带出来**（操作者那一行是原始输出，页面要用它得先开一格）。
fn parse_run_line(line: &str) -> Option<Value> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let mut cursor = 0;
    // 前两段**必须有**；后面五段各自可选，且**按 CLI 的写出顺序**依次试吃
    // （顺序是判据的一部分：认不出的段 ⇒ 整行认不出，不安静跳过）。
    let id = take_segment(&parts, &mut cursor, "run=")?;
    let state = take_segment(&parts, &mut cursor, "state=")?;
    let _reason = take_segment(&parts, &mut cursor, "reason=");
    let outcome = take_segment(&parts, &mut cursor, "outcome=");
    let failure = take_segment(&parts, &mut cursor, "failure=");
    let _unresolved = take_segment(&parts, &mut cursor, "unresolved=");
    let _stopped = take_segment(&parts, &mut cursor, "stopped=");
    if cursor != parts.len() {
        return None;
    }
    Some(json!({
        "id": id,
        "state": state,
        // 没收尾就是 `null`（**不编一个 "ended" 出来**）。
        "outcome": outcome,
        "failure": failure,
    }))
}

/// 吃下一段（`prefix=` 打头才算）：吃到了就前进游标，没吃到**不动**游标。
fn take_segment(parts: &[&str], cursor: &mut usize, prefix: &str) -> Option<String> {
    let part = parts.get(*cursor)?;
    let value = part.strip_prefix(prefix)?;
    *cursor += 1;
    Some(value.to_string())
}

// ---------------------------------------------------------------------------
// 用例（纯逻辑：解析器与形状；不 spawn、不监听）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "celira-agent-host-status v1";

    fn sample(state_lines: &[&str], next_lines: &[&str]) -> String {
        let mut text = String::from(HEAD);
        text.push_str("\nenvironment-root=/home/u/celira-home/agents/a-1\nagent=a-1\n");
        text.push_str("activities-page=32 of=36 scope=latest\n[state]\n");
        for line in state_lines {
            text.push_str(line);
            text.push('\n');
        }
        text.push_str("[next]\n");
        for line in next_lines {
            text.push_str(line);
            text.push('\n');
        }
        text
    }

    #[test]
    fn parser_mirrors_host_status_parse() {
        // 正常一份：admission ＋ 两条活动（其中一条带失败分类）。
        let text = sample(
            &[
                "admission=ready",
                "run=host-1-2-1 state=ended outcome=subject:replied",
                "run=host-1-2-2 state=ended outcome=system:model-failed failure=transport-failure",
            ],
            &["host-1-2-1"],
        );
        match parse_host_status(&text) {
            HostStatus::Loaded {
                admission,
                activities,
                runs,
                next,
            } => {
                assert_eq!(admission, "ready");
                assert_eq!(activities, Some(36), "总数取 of=，不是本页条数 32");
                assert_eq!(runs.len(), 2);
                assert_eq!(runs[0]["id"], "host-1-2-1");
                assert_eq!(runs[1]["failure"], "transport-failure");
                assert_eq!(next, "host-1-2-1");
            }
            other => panic!("该读成 loaded：{other:?}"),
        }
    }

    #[test]
    fn parser_accepts_the_third_activity_shape_reason_and_unresolved() {
        // 2026-10-05 真机撞出来的坑（**同一个坑第三次**）：CLI 的第三类判定
        // `state=unknown reason=dispatched-without-result unresolved=1` 两段都在 ⇒
        // 老解析器认不出 ⇒ 账本里只要有一条这样的活动，**整份读数**又变"读不到"
        // （app 与网页一起瞎，而 Agent 好好的）。夹具**逐字**抄自那一次真输出。
        let text = sample(
            &[
                "admission=ready",
                "run=host-1-1000-1 state=ended outcome=subject:completed",
                "run=host-17680-1791175794423-1 state=unknown reason=dispatched-without-result unresolved=1",
                "run=host-1-2000-2 state=unfinished",
            ],
            &["n"],
        );
        match parse_host_status(&text) {
            HostStatus::Loaded { runs, .. } => {
                assert_eq!(
                    runs.len(),
                    3,
                    "三条活动都要在，不许因为一条带两段后缀就整份丢"
                );
                assert_eq!(runs[1]["state"], "unknown");
                assert!(
                    runs[1]["outcome"].is_null(),
                    "这一类没有 outcome ⇒ null，不编"
                );
                assert!(runs[1]["failure"].is_null());
                assert_eq!(runs[0]["outcome"], "subject:completed");
                assert_eq!(runs[2]["state"], "unfinished");
            }
            other => panic!("该读成 loaded：{other:?}"),
        }
        // 段序是判据的一部分（CLI 定的）：换了顺序就是另一种形状 ⇒ 整行认不出，不安静吃。
        assert!(
            parse_run_line("run=r-1 state=unknown unresolved=1 reason=dispatched-without-result")
                .is_none(),
            "段的顺序换了 ⇒ 认不出"
        );
        assert!(
            parse_run_line("run=r-1 state=unknown reason=dispatched-without-result unresolved=1")
                .is_some(),
            "CLI 的顺序 ⇒ 认得出"
        );
    }

    #[test]
    fn parser_accepts_the_stop_reason_segment_added_in_2026_10_13() {
        // 2026-10-18 真机撞出来的坑（**同一个坑第四次**）：CLI 2026-10-13 起在行尾多写
        // `stopped=`（为什么停在这里：`held` / `yielded` / `no-response` / `unreadable`），
        // 而两份解析器都还停在六段上 ⇒ 账本里只要有一条说得出为什么停下的未收尾活动，
        // **整份读数**又被判 `malformed-run-line` ⇒ 机器页「运行」那一间整间说"读不到"
        // （真机读数：打包形态门禁 13/14，红的那一条正是活动读数 —— 而 Agent 好好的）。
        // 夹具**逐字**抄自那一次真输出。
        let text = sample(
            &[
                "admission=unfinished(run=host-31240-1791817079006-2)",
                "run=host-33736-1792318397097-1 state=unfinished stopped=no-response",
                "run=host-2188-1792318525403-1 state=unfinished stopped=no-response",
                "run=host-1-1000-1 state=ended outcome=subject:completed",
            ],
            &["上一段没有收尾 —— 看它停在哪"],
        );
        match parse_host_status(&text) {
            HostStatus::Loaded { runs, .. } => {
                assert_eq!(runs.len(), 3, "三条活动都要在，不许因为一条带尾段就整份丢");
                assert_eq!(runs[0]["state"], "unfinished");
                assert!(runs[0]["outcome"].is_null(), "未收尾 ⇒ null，不编");
                assert_eq!(runs[2]["outcome"], "subject:completed", "前后正常的照旧");
                // 认得出、**不带出来**：`stopped` 今天没有消费者（操作者那一行是原始输出）。
                assert!(runs[0].get("stopped").is_none(), "这一段今天不上载荷");
            }
            other => panic!("该读成 loaded：{other:?}"),
        }
        // 段序仍是判据的一部分：`stopped=` 跑到前面 ⇒ 认不出。
        assert!(
            parse_run_line("run=r-1 stopped=no-response state=unfinished").is_none(),
            "段的顺序换了 ⇒ 认不出"
        );
        assert!(
            parse_run_line("run=r-1 state=unfinished stopped=no-response").is_some(),
            "CLI 的顺序 ⇒ 认得出"
        );
    }

    #[test]
    fn parser_accepts_unfinished_runs_without_outcome() {
        // 2026-10-03 真机撞出来的坑：未收尾的活动行**没有 `outcome=`**。要求它 ⇒ 一条未收尾
        // 活动就让整份读数被判 `malformed-run-line` ⇒ 页面说"读不到账本"，而 Agent 好好的。
        // 夹具**逐字**抄自那一次真实输出。
        let text = sample(
            &[
                "admission=unfinished(run=host-13556-1791090187913-2)",
                "run=host-13556-1791090175692-1 state=ended outcome=subject:silent",
                "run=host-13556-1791090187913-2 state=unfinished",
            ],
            &["有未收尾活动（host-13556-1791090187913-2）：活动停在半路"],
        );
        match parse_host_status(&text) {
            HostStatus::Loaded { runs, .. } => {
                assert_eq!(runs.len(), 2, "两条活动都要在，不许因为一条没收尾就整份丢");
                assert_eq!(runs[1]["state"], "unfinished");
                assert!(
                    runs[1]["outcome"].is_null(),
                    "没收尾 ⇒ outcome 是 null，不编一个出来"
                );
                assert_eq!(runs[0]["outcome"], "subject:silent");
            }
            other => panic!("该读成 loaded：{other:?}"),
        }
        // 未收尾与失败分类可以同时在（真要出事也是这个形状）。
        let both = sample(
            &[
                "admission=unfinished(run=r-2)",
                "run=r-2 state=unfinished failure=transport-failure",
            ],
            &["n"],
        );
        match parse_host_status(&both) {
            HostStatus::Loaded { runs, .. } => {
                assert!(runs[0]["outcome"].is_null());
                assert_eq!(runs[0]["failure"], "transport-failure");
            }
            other => panic!("该读成 loaded：{other:?}"),
        }
    }

    #[test]
    fn parser_refuses_to_guess() {
        // 版本头认不出 ⇒ **读不到账本**（不是"没有活动"）。
        assert_eq!(
            parse_host_status("something else\n[state]\nadmission=ready\n[next]\n"),
            HostStatus::Unreadable {
                reason: "unknown-header".to_string()
            }
        );
        assert_eq!(
            parse_host_status(""),
            HostStatus::Unreadable {
                reason: "empty".to_string()
            }
        );
        // Agent 这一轮在忙：**一等结果**，与"读不到"分开。
        assert_eq!(
            parse_host_status("celira-agent-host-status v1\nalready-open\n"),
            HostStatus::Busy
        );
        // 缺 admission / 缺 next 段 / 认不出的活动行 ⇒ 各自的原因，绝不安静跳过。
        assert_eq!(
            parse_host_status(&sample(&["run=host-1 state=ended outcome=x"], &["n"])),
            HostStatus::Unreadable {
                reason: "missing-admission".to_string()
            }
        );
        assert_eq!(
            parse_host_status(&format!("{HEAD}\n[state]\nadmission=ready\n")),
            HostStatus::Unreadable {
                reason: "missing-state-section".to_string()
            }
        );
        assert_eq!(
            parse_host_status(&sample(&["admission=ready", "这不是一条活动行"], &["n"])),
            HostStatus::Unreadable {
                reason: "malformed-run-line".to_string()
            }
        );
        // 取不到 `of=` ⇒ 总数是 `null`（**不拿本页条数冒充**）。
        let no_total = format!("{HEAD}\n[state]\nadmission=ready\n[next]\n");
        match parse_host_status(&no_total) {
            HostStatus::Loaded { activities, .. } => assert_eq!(activities, None),
            other => panic!("该读成 loaded：{other:?}"),
        }
    }

    #[test]
    fn runs_are_capped_to_the_recent_ones() {
        let mut many: Vec<String> = vec!["admission=ready".to_string()];
        many.extend(
            (0..40).map(|index| format!("run=r-{index} state=ended outcome=subject:silent")),
        );
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        match parse_host_status(&sample(&refs, &["n"])) {
            HostStatus::Loaded { runs, .. } => {
                assert_eq!(runs.len(), RECENT_RUNS, "明细只给最近 N 条");
                assert_eq!(runs[0]["id"], "r-8", "取的是**最近**那一头");
            }
            other => panic!("该读成 loaded：{other:?}"),
        }
    }

    #[test]
    fn time_and_port_follow_the_same_defaults() {
        // 时刻形状（JS 的 `toISOString()` 同族）：给一个已知的 Unix 秒对一遍。
        assert_eq!(civil_from_unix(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil_from_unix(1_700_000_000), (2023, 11, 14, 22, 13, 20));
        assert!(now_iso8601().ends_with('Z'));
    }

    #[test]
    fn env_identity_is_strict_and_never_guesses() {
        // 「这是哪个 Agent 的账本」这一格：读得出就给，读不出就**没有这一格**（页面据此说"不知道"，
        // 而不是猜一个 id 出来 —— `agentAgentMatch` 的三态就靠它）。
        let dir = std::env::temp_dir().join(format!("celira-door-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("identity.json");

        std::fs::write(&file, "{\"agent\":\"a-1\"}").unwrap();
        assert_eq!(env_identity(&dir).as_deref(), Some("a-1"));
        std::fs::write(&file, "{\"agent\":\"  a-1  \"}").unwrap();
        assert_eq!(env_identity(&dir).as_deref(), Some("a-1"), "两侧空白去掉");

        for bad in [
            "{\"agent\":\"\"}",
            "{\"agent\":\"   \"}",
            "{\"agent\":123}",
            "{}",
            "not json",
        ] {
            std::fs::write(&file, bad).unwrap();
            assert_eq!(env_identity(&dir), None, "读不出就不给：{bad}");
        }
        std::fs::remove_file(&file).unwrap();
        assert_eq!(env_identity(&dir), None, "文件不在 ⇒ 没有这一格");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 配置门那份快照：**三态不许压成两态**（读不出来 ≠ 没配）、**悬空绑定**要如实说、
    /// 而 `api_key` 的取值**一个字节都不许进载荷**（只有 `keyConfigured` 布尔）。
    #[test]
    fn config_snapshot_is_three_state_and_never_leaks_the_key() {
        use crate::home_facts::{HomeConf, Layout, LayoutKind};
        use crate::model_sources;

        let home = std::env::temp_dir().join(format!("celira-door-cfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let env_root = home.join("agents").join("a-1");
        std::fs::create_dir_all(&env_root).unwrap();
        std::fs::write(env_root.join("identity.json"), "{\"agent\":\"a-1\"}").unwrap();
        let facts = LocalFacts {
            home_dir: home.clone(),
            conf: HomeConf::default(),
            layout: Layout {
                kind: LayoutKind::Source,
                repo: home.clone(),
                im_exe: home.join("celira-im"),
                host_exe: home.join("agent_host"),
                run_exe: home.join("agent_run"),
                status_exe: home.join("agent_host_status"),
            },
            repo_dir: None,
        };

        // ① 清单不在、绑定不在 ⇒ `absent`（**不是** `unreadable`）。
        let snapshot = config_snapshot(&facts);
        assert_eq!(snapshot["state"], "absent");
        assert_eq!(snapshot["sourcesState"], "loaded", "文件不在就是空表");
        assert_eq!(snapshot["bindingState"], "absent");
        assert_eq!(snapshot["bound"], Value::Null);
        assert_eq!(snapshot["reason"], Value::Null);
        assert!(snapshot["entries"].as_array().unwrap().is_empty());

        // ② 清单一条 ＋ 这一支绑着它 ⇒ `configured`；key 的取值不出门。
        std::fs::write(
            model_sources::list_path(&home),
            r#"[{"id":"ms-1","name":"深度求索","endpoint":"https://api.deepseek.com/chat/completions",
                 "api_key":"sk-secret","model":"deepseek-chat","context_window":65536}]"#,
        )
        .unwrap();
        std::fs::write(env_root.join("model_binding.json"), "{\"source\":\"ms-1\"}").unwrap();
        let snapshot = config_snapshot(&facts);
        let text = snapshot.to_string();
        assert_eq!(snapshot["state"], "configured");
        assert_eq!(snapshot["bound"], "ms-1");
        assert_eq!(snapshot["model"], "deepseek-chat");
        assert_eq!(snapshot["contextWindow"], 65536);
        assert_eq!(snapshot["keyConfigured"], true);
        assert!(!text.contains("sk-secret"), "key 不许进载荷：{text}");
        assert_eq!(snapshot["entries"][0]["keyConfigured"], true);
        assert_eq!(snapshot["entries"][0]["contextWindow"], 65536);
        assert_eq!(snapshot["boundPerResident"][0]["agentId"], "a-1");
        assert_eq!(snapshot["bindingDivergent"], false, "只有一支 ⇒ 谈不上分歧");

        // ③ **悬空绑定**（指向已被删掉的条目）⇒ `unreadable` ＋ `binding_dangling`，
        //    **不静默回落**成"没配"（那是两件不同的事：前者要用户去修，后者只是还没配）。
        std::fs::write(model_sources::list_path(&home), "[]").unwrap();
        let snapshot = config_snapshot(&facts);
        assert_eq!(snapshot["state"], "unreadable");
        assert_eq!(snapshot["reason"], "binding_dangling");
        assert_eq!(snapshot["bound"], "ms-1", "绑定照说（它读得出来）");

        // ④ 清单本身就是坏文件 ⇒ `unreadable` ＋ 原因原样。
        std::fs::write(model_sources::list_path(&home), "not json").unwrap();
        let snapshot = config_snapshot(&facts);
        assert_eq!(snapshot["state"], "unreadable");
        assert_eq!(snapshot["reason"], "not_json");
        assert_eq!(snapshot["sourcesState"], "unreadable");

        let _ = std::fs::remove_dir_all(&home);
    }

    /// 写路径的**校验器**（照 `validateDeclaration` / `validateProbe` / `pickEntryId` 的判据与顺序）。
    #[test]
    fn declaration_validators_mirror_the_authority() {
        use crate::model_sources::ModelSourceEntry;
        let entry = |id: &str, key: &str| ModelSourceEntry {
            id: id.to_string(),
            name: "n".to_string(),
            endpoint: "https://api.deepseek.com/chat/completions".to_string(),
            model: "deepseek-chat".to_string(),
            context_window: 65536,
            protocol: None,
            reasoning_effort: None,
            vision: false,
            api_key: key.to_string(),
        };
        let entries = vec![entry("ms-1", "sk-old")];

        // ① 地址：空 / 不是 http(s) / 只有协议头 / 没有协议 ⇒ 都是 `invalid_endpoint`。
        for bad in ["", "ftp://x/y", "http://", "api.deepseek.com"] {
            let body = json!({ "endpoint": bad, "model": "m", "apiKey": "k" });
            assert_eq!(
                validate_declaration(&body, None).err(),
                Some("invalid_endpoint"),
                "{bad} 该被拒"
            );
        }
        // ② 模型名必须有（`invalid_model`）。
        assert_eq!(
            validate_declaration(
                &json!({ "endpoint": "https://x/v1", "model": "  " }),
                Some("k")
            )
            .err(),
            Some("invalid_model")
        );
        // ③ 钥匙：新建必须给；更新可沿用（`key_required` 只在两处都空时）。
        assert_eq!(
            validate_declaration(&json!({ "endpoint": "https://x/v1", "model": "m" }), None).err(),
            Some("key_required")
        );
        let reused = validate_declaration(
            &json!({ "endpoint": "https://x/v1", "model": "m", "apiKey": "   " }),
            Some("sk-old"),
        )
        .expect("留空 key 该沿用");
        assert_eq!(reused.credential, "sk-old", "沿用那一条的钥匙");
        // ④ 窗口：只认正整数（0 不是一条可解释的声明）。
        for bad in ["0", "-1", "12a", "99999999999"] {
            assert_eq!(
                validate_declaration(
                    &json!({ "endpoint": "https://x/v1", "model": "m", "apiKey": "k", "contextWindow": bad }),
                    None
                )
                .err(),
                Some("invalid_window"),
                "{bad} 该被拒"
            );
        }
        let given = validate_declaration(
            &json!({ "endpoint": "https://x/v1", "model": "m", "apiKey": "k", "contextWindow": " 65536 " }),
            None,
        )
        .expect("合法窗口");
        assert_eq!(given.context_window, Some(65536), "两侧空白不算数");
        // ⑤ 协议闭集；`bind` 不许是字符串；`name` 不许是数字。
        assert_eq!(
            validate_declaration(
                &json!({ "endpoint": "https://x/v1", "model": "m", "apiKey": "k", "protocol": "grpc" }),
                None
            )
            .err(),
            Some("invalid_protocol")
        );
        assert_eq!(
            validate_declaration(
                &json!({ "endpoint": "https://x/v1", "model": "m", "apiKey": "k", "bind": "false" }),
                None
            )
            .err(),
            Some("invalid_bind"),
            "字符串 \"false\" 不当真"
        );
        assert_eq!(
            validate_declaration(
                &json!({ "endpoint": "https://x/v1", "model": "m", "apiKey": "k", "name": 7 }),
                None
            )
            .err(),
            Some("invalid_name")
        );
        // ⑥ `bind` 缺省 = true（老页面那句"保存 = 用它"）。
        assert!(
            validate_declaration(
                &json!({ "endpoint": "https://x/v1", "model": "m", "apiKey": "k" }),
                None
            )
            .expect("合法")
            .bind
        );

        // ⑦ **`vision` 那一格是三态**（2026-10-17，照 `validateDeclaration`）：缺 / `null` ⇒
        // **没说**（`None`，写侧从旧条目带过来）；布尔 ⇒ 以它为准；非布尔 ⇒ `invalid_vision`
        // （半个形状比缺席更坏：照着重填会写出错的清单）。
        let vision_of =
            |body: serde_json::Value| validate_declaration(&body, Some("k")).map(|d| d.vision);
        assert_eq!(
            vision_of(json!({ "endpoint": "https://x/v1", "model": "m" })).unwrap(),
            None,
            "没说 ⇒ None（**不是** Some(false)：那会把旧条目的值顶掉）"
        );
        assert_eq!(
            vision_of(json!({ "endpoint": "https://x/v1", "model": "m", "vision": null })).unwrap(),
            None,
            "`null` 与缺场同义（照 JS 那一侧）"
        );
        assert_eq!(
            vision_of(json!({ "endpoint": "https://x/v1", "model": "m", "vision": true })).unwrap(),
            Some(true)
        );
        assert_eq!(
            vision_of(json!({ "endpoint": "https://x/v1", "model": "m", "vision": false }))
                .unwrap(),
            Some(false),
            "说了 `false` ⇒ 以它为准（关得掉）"
        );
        assert_eq!(
            vision_of(json!({ "endpoint": "https://x/v1", "model": "m", "vision": "yes" })).err(),
            Some("invalid_vision"),
            "字符串不当真"
        );

        // ⑧ `pickEntryId`：新建不给 id（给了就拒）；更新点名必须**在场**；不给 id 用当前绑定的那条。
        assert_eq!(
            pick_entry_id(&json!({ "id": "ms-9" }), &entries, None, true).err(),
            Some("bad_request"),
            "新建是指定 id ⇒ 拒绝（那是新建，不是覆盖）"
        );
        assert_eq!(
            pick_entry_id(&json!({}), &entries, None, true)
                .expect("新建（不给 id）")
                .id,
            "ms-2",
            "id 由门给，单调不回收"
        );
        assert_eq!(
            pick_entry_id(&json!({ "id": "nope" }), &entries, None, false).err(),
            Some("invalid_id")
        );
        assert_eq!(
            pick_entry_id(&json!({ "id": "ms-9" }), &entries, None, false).err(),
            Some("entry_not_found"),
            "点名一条不在场的 ⇒ 绝不悄悄新建一条"
        );
        let picked =
            pick_entry_id(&json!({}), &entries, Some("ms-1"), false).expect("用绑定的那条");
        assert_eq!(picked.id, "ms-1");
        assert!(!picked.creating);
        assert_eq!(
            picked.previous.map(|each| each.api_key),
            Some("sk-old".to_string())
        );
        let fresh = pick_entry_id(&json!({}), &entries, None, false).expect("没绑定 ⇒ 新建");
        assert_eq!(fresh.id, "ms-2");
        assert!(fresh.creating);
    }

    /// 试连那两件纯逻辑：地址拼法（**端点是完整地址**）与窗口/目录的读法。
    #[test]
    fn probe_shapes_mirror_the_authority() {
        // 端点是完整地址 ⇒ 必须摘掉线协议那一段再拼 `/models`（直接拼会探到不存在的路径）。
        for (endpoint, expected) in [
            (
                "https://api.deepseek.com/chat/completions",
                "https://api.deepseek.com/models",
            ),
            (
                "https://api.deepseek.com/v1/chat/completions/",
                "https://api.deepseek.com/v1/models",
            ),
            (
                "https://api.openai.com/v1/completions",
                "https://api.openai.com/v1/models",
            ),
            (
                "https://api.openai.com/v1/responses",
                "https://api.openai.com/v1/models",
            ),
            (
                "https://api.deepseek.com/v1",
                "https://api.deepseek.com/v1/models",
            ),
        ] {
            assert_eq!(models_url(endpoint), expected, "{endpoint}");
        }
        // 目录三种形状都认；名字按 id → name → model 取第一个非空的；去重保序。
        let body = json!({ "data": [
            { "id": "deepseek-chat" },
            { "id": "deepseek-reasoner" },
            { "name": "deepseek-chat" },
            { "model": "third" },
            { "nothing": 1 }
        ]});
        assert_eq!(
            models_from_body(&body),
            vec!["deepseek-chat", "deepseek-reasoner", "third"]
        );
        assert!(models_from_body(&json!({ "nope": [] })).is_empty());
        // 窗口：点名那条的标称窗口，按**有序字段表**先命中的赢，并如实回是哪一件命中的。
        let with_window = json!({ "data": [
            { "id": "deepseek-chat", "context_window": 65536 },
            { "id": "other", "top_provider": { "context_length": "128000" } }
        ]});
        assert_eq!(
            window_from_models(&with_window, "deepseek-chat"),
            (Some(65536), Some("context_window".to_string()))
        );
        assert_eq!(
            window_from_models(&with_window, "other"),
            (
                Some(128000),
                Some("top_provider.context_length".to_string())
            ),
            "点号往下一层找；字符串里的数字也算"
        );
        assert_eq!(window_from_models(&with_window, "absent"), (None, None));
        assert_eq!(
            window_from_models(&with_window, ""),
            (None, None),
            "没给模型名就是纯拉目录"
        );
        // 非正整数一律当"这一格没给"（0 / 负数 / 小数 / 乱码）。
        for bad in [json!(0), json!(-5), json!(1.5), json!("abc")] {
            let body = json!({ "data": [{ "id": "m", "context_window": bad }] });
            assert_eq!(window_from_models(&body, "m"), (None, None), "{bad}");
        }
    }

    /// 来源校验（照 `home-key.js` 的 `localOriginAllowed`）：`Host` 必须 loopback；带 `Origin` 时也得是。
    #[test]
    fn origin_check_follows_the_local_rule() {
        let head = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect()
        };
        assert!(origin_allowed(&head(&[("host", "127.0.0.1:5191")])));
        assert!(origin_allowed(&head(&[("host", "localhost:5191")])));
        assert!(origin_allowed(&head(&[("host", "[::1]:5191")])));
        assert!(origin_allowed(&[]), "裸客户端（没有 Host）⇒ 放行");
        assert!(
            !origin_allowed(&head(&[("host", "evil.example.com")])),
            "域名解析到 127.0.0.1 那类：Host 不是 loopback ⇒ 拒"
        );
        assert!(origin_allowed(&head(&[
            ("host", "127.0.0.1:5191"),
            ("origin", "http://127.0.0.1:5173")
        ])));
        assert!(
            !origin_allowed(&head(&[
                ("host", "127.0.0.1:5191"),
                ("origin", "https://evil.example.com")
            ])),
            "别的页面来敲门 ⇒ 拒"
        );
        // ★ 2026-10-05 主人裁：**壳自己的来源也算本机**（打包形态页面就是它，
        //   从前这一条把壳自己的页面拒在门外 —— 见 `SHELL_ORIGINS` 那段）。
        assert!(origin_allowed(&head(&[
            ("host", "127.0.0.1:5191"),
            ("origin", "http://tauri.localhost")
        ])));
        assert!(origin_allowed(&head(&[
            ("host", "127.0.0.1:5191"),
            ("origin", "tauri://localhost")
        ])));
        // 反例（这一条才是"没放宽"的证据）：协议与主机要成对，长得像的、别人的一律拒。
        for origin in [
            "tauri://evil.example",
            "http://tauri.localhost.evil.example",
            "https://evil.example",
            "file:///c:/x.html",
            "not a url",
        ] {
            assert!(
                !origin_allowed(&head(&[("host", "127.0.0.1:5191"), ("origin", origin)])),
                "{origin} 不该被当成本机来源"
            );
        }
        // Host 那一道一个字没动：Host 非 loopback 时，壳自己的来源也过不去。
        assert!(!origin_allowed(&head(&[
            ("host", "tauri.localhost:5191"),
            ("origin", "http://tauri.localhost")
        ])));
    }

    #[test]
    fn wake_line_comes_from_the_host_log() {
        let dir = std::env::temp_dir().join(format!("celira-door-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let log = dir.join("host.out");
        std::fs::write(
            &log,
            "[listening]\nwake run=host-1-1-1 drives=1 ending=subject:silent\nnoise\nwake run=host-1-1-2 drives=2\n",
        )
        .unwrap();
        assert_eq!(
            last_wake_of(&log).as_deref(),
            Some("wake run=host-1-1-2 drives=2"),
            "取最后一条 wake（不是第一条）"
        );
        assert_eq!(last_wake_of(&dir.join("nope.out")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ★ 2026-10-05：**零住户与"二进制不在"是两件事**（真机撞出来的）。
    ///
    /// 从前两者共用 `missing-binary` ⇒ 刚建好世界的新用户在「运行」那一格读到
    /// 「本机缺少必要的程序」—— 一句**假警报**（机器好好的，只是还没有 Agent 住进来）。
    /// 判据：**缺省不等于故障**。与 `status-server.js` 同款（那边有对应用例）。
    #[test]
    fn an_empty_home_says_no_resident_while_a_missing_binary_still_says_so() {
        let root = std::env::temp_dir().join(format!("celira-door-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        // 一个**没有可执行件**的仓库根：形态判据走源码形态 ⇒ `agent_host_status` 找不到。
        let repo = root.join("repo");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&repo).unwrap();

        // ① 这个家一支住户都没有 ⇒ `no-resident`（**不是**缺件）
        let payload = build_payload(&LocalFacts::load(Some(&home), Some(&repo)));
        assert_eq!(
            agent_state_of(&payload).as_deref(),
            Some("no-resident"),
            "还没有 Agent 不是缺件"
        );

        // ② 有住户、而运行时那个二进制不在 ⇒ 那才是缺件（这一态一个字都不许变）
        let env_root = home.join("agents").join("a-1");
        std::fs::create_dir_all(&env_root).unwrap();
        std::fs::write(env_root.join("identity.json"), r#"{"agent":"a-1"}"#).unwrap();
        let payload = build_payload(&LocalFacts::load(Some(&home), Some(&repo)));
        assert_eq!(
            agent_state_of(&payload).as_deref(),
            Some("missing-binary"),
            "缺件照旧是缺件"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// ★ 2026-10-12：**住户表为空 ⇒ 换绑 / 解绑没有目标**（不许静默成功）。
    ///
    /// 这一格与 JS 门那一侧**同一个码、同一个状态**（`config-server.js` 的 `ERRORS.NO_RESIDENT`，
    /// 客户端那份封闭表逐字对齐）。为什么不能"当成成功"：`PUT /home-binding` 问的是
    /// "**哪一支**用哪一条"，0 住户时它落在**零支**上 —— 报 200 等于告诉页面"换好了"，
    /// 而磁盘上一个字节都没变（读那一侧同样给 0 行，见 `config_snapshot`）。
    #[test]
    fn binding_without_a_resident_is_refused_not_silently_ignored() {
        let root =
            std::env::temp_dir().join(format!("celira-door-noresident-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();

        let facts = LocalFacts::load(Some(&home), Some(&home));
        // ① 一支住户都没有：换绑（给了 source）与解绑（空 source）**都拒**。
        assert_eq!(binding_write(&facts, r#"{"source":"ms-1"}"#).0, 409);
        assert_eq!(
            binding_write(&facts, r#"{"source":"ms-1"}"#)
                .1
                .get("error")
                .and_then(Value::as_str),
            Some("no_resident")
        );
        assert_eq!(binding_write(&facts, r#"{"source":""}"#).0, 409);
        // ② **一个字节都不落**：不许造出 `<家>/env`（合成一支住户会留下的孤儿）。
        assert!(!home.join("env").exists(), "0 住户时不许写任何绑定文件");

        // ③ 有住户之后，这一笔照旧走原来的路（清单里没有 ms-1 ⇒ `entry_not_found`，不再是 no_resident）。
        let env_root = home.join("agents").join("a-1");
        std::fs::create_dir_all(&env_root).unwrap();
        std::fs::write(env_root.join("identity.json"), r#"{"agent":"a-1"}"#).unwrap();
        let facts = LocalFacts::load(Some(&home), Some(&home));
        let (status, body) = binding_write(&facts, r#"{"source":"ms-1"}"#);
        assert_eq!(status, 404);
        assert_eq!(
            body.get("error").and_then(Value::as_str),
            Some("entry_not_found")
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// ★ 2026-10-12：**住户级覆盖** —— `agentId` 给了就**只写那一支**；
    /// ★ 2026-10-14「选择先于接上」：点名一支**还没住户化**的 ⇒ 落在**她将来会住的那个
    /// 环境根**上（`<家>/agents/<id>/`）；点名一个**形状不合法**的 ⇒ `400 bad_agent_id`
    /// （这一格会变成目录名 ⇒ 不预检就是目录穿越）。
    ///
    /// 与 JS 门那一侧同一条口径（`config-server.js` 的 `writeTargetsOf` /`agentIdShapeOk`）。
    ///
    /// ⚠ **判据翻转的记账**：② 从前是 `409 no_such_resident`（"这台机器上没有这一支 ⇒ 拒"）。
    /// 翻转的理由不是放宽，是**那条判据缺了一个真处境**：绑定本来就住在"她的环境根"里，
    /// 而环境根要等接她第 2 步才建 ⇒ "先给她选好、再接上"这条路在页面上走不通
    /// （2026-10-14 主人撞上：机器上有两条来源，接她时按"选哪条 = 花谁的钱"停在原地，
    /// 而选的那一页要求她已经在机器上 —— 两边互等）。老消费者不少什么：`no_such_resident`
    /// 照旧留在封闭表里（**旧版的门**在远程那台还这么答）。
    #[test]
    fn binding_can_target_one_resident_and_seeds_an_unhomed_one() {
        let root =
            std::env::temp_dir().join(format!("celira-door-perresident-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let first = home.join("agents").join("a-1");
        let second = home.join("agents").join("a-2");
        for (dir, id) in [(&first, "a-1"), (&second, "a-2")] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("identity.json"), format!(r#"{{"agent":"{id}"}}"#)).unwrap();
            crate::model_sources::write_binding(dir, "ms-1").unwrap();
        }
        let entries: Vec<crate::model_sources::ModelSourceEntry> = ["ms-1", "ms-2"]
            .iter()
            .map(|id| crate::model_sources::ModelSourceEntry {
                id: (*id).to_string(),
                name: format!("钥匙 {id}"),
                endpoint: "https://x.invalid/v1/chat/completions".to_string(),
                model: "m".to_string(),
                context_window: 8192,
                protocol: None,
                reasoning_effort: None,
                vision: false,
                api_key: "k".to_string(),
            })
            .collect();
        crate::model_sources::write_list(&home, &entries).unwrap();
        let binding_text = |dir: &std::path::Path| {
            std::fs::read_to_string(dir.join("model_binding.json")).unwrap()
        };

        let facts = LocalFacts::load(Some(&home), Some(&home));
        // ① 只给 a-2 换 ⇒ 只有它那份文件变了。
        assert_eq!(
            binding_write(&facts, r#"{"source":"ms-2","agentId":"a-2"}"#).0,
            200
        );
        assert!(
            binding_text(&second).contains("ms-2"),
            "点名的那一支要换过去"
        );
        assert!(
            binding_text(&first).contains("ms-1"),
            "没点名的那一支一个字节都不许动"
        );

        // ② 点名一支**还没住户化**的 ⇒ 落在她将来会住的那个环境根上；别人一个字节不动。
        let fresh = home.join("agents").join("a-9");
        let (status, body) = binding_write(&facts, r#"{"source":"ms-2","agentId":"a-9"}"#);
        assert_eq!(status, 200, "{body}");
        assert!(
            binding_text(&fresh).contains("ms-2"),
            "先选好的那一笔要落在她的环境根上"
        );
        assert!(
            !fresh.join("agent.conf").exists(),
            "空目录不许变成住户（住户判据要 agent.conf 或 identity.json）"
        );
        assert!(
            binding_text(&first).contains("ms-1"),
            "先选好的那一笔不许顺手改别人"
        );
        // 读面：她进 `boundUnhomed`（"先选好了"），**不进** `boundPerResident`（"住户"）。
        let snapshot = config_snapshot(&facts);
        let unhomed: Vec<&str> = snapshot["boundUnhomed"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|row| row["agentId"].as_str())
            .collect();
        assert_eq!(unhomed, vec!["a-9"], "先选好的那一支要如实读回来");
        let residents: Vec<&str> = snapshot["boundPerResident"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|row| row["agentId"].as_str())
            .collect();
        assert_eq!(
            residents,
            vec!["a-1", "a-2"],
            "她不是住户 ⇒ 不许混进住户那一格"
        );

        // ③ **形状不合法** ⇒ 400，且连目录都不建。
        for bad in ["../evil", "a/b", "a b", ".", "..", "a:b"] {
            let body_text = format!(r#"{{"source":"ms-2","agentId":"{bad}"}}"#);
            let (status, body) = binding_write(&facts, &body_text);
            assert_eq!(status, 400, "{bad} 该被拒：{body}");
            assert_eq!(
                body.get("error").and_then(Value::as_str),
                Some("bad_agent_id")
            );
        }
        assert!(!root.join("evil").exists(), "不许拼出 home 之外的路径");
        assert!(
            binding_text(&first).contains("ms-1"),
            "被拒的那几笔一个字节都没写"
        );

        // ④ 不给 `agentId` ⇒ 家级默认（每一支都换），与从前一字不变。
        assert_eq!(binding_write(&facts, r#"{"source":"ms-2"}"#).0, 200);
        assert!(binding_text(&first).contains("ms-2"));
        assert!(binding_text(&second).contains("ms-2"));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// ★ 2026-10-14：**试连读数的形状 —— 每一档都要带 `models`**。
    ///
    /// 壳这一侧一直带着它（`no_window` 里有 `"models": []`），而 JS 门那一侧的"连不上"档
    /// 从前**漏了它** ⇒ 客户端那份形状守卫（按纪律要求它在场）把整份读数判成"认不出的形状"
    /// ⇒ 页面把"连不上"说成"读不到这份读数"（选型号那一格点了没反应）。
    /// 这条钉住壳这一侧**不许**跟着漏（JS 那一侧由 `config-server.test.js` 的同名判据钉）。
    #[test]
    fn probe_reading_always_carries_the_model_list() {
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let reading = probe_models(
            &format!("http://127.0.0.1:{closed}/v1/chat/completions"),
            "sk-x",
            "m",
        );
        assert_eq!(reading["ok"], false);
        assert_eq!(reading["reason"], "unreachable");
        assert!(
            reading["models"].is_array(),
            "连不上那一档也必须带 models：{reading}"
        );
        assert_eq!(reading["models"].as_array().unwrap().len(), 0);
    }

    fn agent_state_of(payload: &Value) -> Option<String> {
        payload
            .get("agent")?
            .get("state")?
            .as_str()
            .map(str::to_string)
    }

    /// ★ 2026-10-14：**支级覆盖那两格**（"这一支具体用哪个型号 / 什么思考档"）。
    ///
    /// 症状：运行时**一直**认它俩（`agent/src/model_source.rs` 的 `apply_binding_overrides`：
    /// 绑定的型号/档位赢过条目里的），而**没有任何写者** ⇒ 页面只能靠"再建一条只差型号的条目"
    /// 来间接表达"换个型号"。判据四条：① 写进去、读面带回来；② `null` 只清那一格；
    /// ③ 坏形状 400 且一个字节都不写；④ 没点名 / 解绑时带它 ⇒ 400（那两格是**支级事实**）。
    #[test]
    fn binding_carries_the_per_agent_model_and_effort_overrides() {
        let root = std::env::temp_dir().join(format!("celira-door-effort-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let first = home.join("agents").join("a-1");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::write(first.join("identity.json"), r#"{"agent":"a-1"}"#).unwrap();
        crate::model_sources::write_binding(&first, "ms-1").unwrap();
        let entries: Vec<crate::model_sources::ModelSourceEntry> = ["ms-1", "ms-2"]
            .iter()
            .map(|id| crate::model_sources::ModelSourceEntry {
                id: (*id).to_string(),
                name: format!("钥匙 {id}"),
                endpoint: "https://x.invalid/v1/chat/completions".to_string(),
                model: "m".to_string(),
                context_window: 8192,
                protocol: None,
                reasoning_effort: None,
                vision: false,
                api_key: "k".to_string(),
            })
            .collect();
        crate::model_sources::write_list(&home, &entries).unwrap();
        let binding_text = || std::fs::read_to_string(first.join("model_binding.json")).unwrap();
        let facts = LocalFacts::load(Some(&home), Some(&home));

        // ① 两格都写 ⇒ 文件里是它们，读面那一行也带回来。
        let (status, body) = binding_write(
            &facts,
            r#"{"source":"ms-1","agentId":"a-1","model":"deepseek-v4.1-flash","reasoningEffort":"max"}"#,
        );
        assert_eq!(status, 200, "{body}");
        let written: Value = serde_json::from_str(&binding_text()).unwrap();
        assert_eq!(written["model"], "deepseek-v4.1-flash");
        assert_eq!(written["reasoning_effort"], "max");
        let rows = config_snapshot(&facts);
        assert_eq!(rows["boundPerResident"][0]["model"], "deepseek-v4.1-flash");
        assert_eq!(rows["boundPerResident"][0]["reasoningEffort"], "max");

        // ② `null` 只清那一格（另一格原样）。
        let (status, body) =
            binding_write(&facts, r#"{"source":"ms-1","agentId":"a-1","model":null}"#);
        assert_eq!(status, 200, "{body}");
        let written: Value = serde_json::from_str(&binding_text()).unwrap();
        assert!(written["model"].is_null(), "清掉之后那一格不该还在");
        assert_eq!(
            written["reasoning_effort"], "max",
            "没点名要清的那一格不许动"
        );
        assert!(config_snapshot(&facts)["boundPerResident"][0]["model"].is_null());

        // ③ 坏形状 ⇒ 400，且一个字节都不写（每一格各自一条）。
        for (body, code) in [
            (
                r#"{"source":"ms-1","agentId":"a-1","model":""}"#,
                "invalid_model",
            ),
            (
                r#"{"source":"ms-1","agentId":"a-1","model":"   "}"#,
                "invalid_model",
            ),
            (
                r#"{"source":"ms-1","agentId":"a-1","model":42}"#,
                "invalid_model",
            ),
            (
                r#"{"source":"ms-1","agentId":"a-1","reasoningEffort":"turbo"}"#,
                "invalid_effort",
            ),
            (
                r#"{"source":"ms-1","agentId":"a-1","reasoningEffort":"MAX"}"#,
                "invalid_effort",
            ),
            (
                r#"{"source":"ms-1","agentId":"a-1","reasoningEffort":3}"#,
                "invalid_effort",
            ),
        ] {
            let before = binding_text();
            let (status, body) = binding_write(&facts, body);
            assert_eq!(status, 400, "{body} 该被拒：{body}");
            assert_eq!(body.get("error").and_then(Value::as_str), Some(code));
            assert_eq!(binding_text(), before, "被拒的那几笔一个字节都没写");
        }

        // ④ 没点名（家级默认）/ 解绑时带它 ⇒ 400（支级事实，不许写成"每一支同一个型号"）。
        for body in [
            r#"{"source":"ms-1","model":"x"}"#,
            r#"{"source":"","agentId":"a-1","model":"x"}"#,
        ] {
            let before = binding_text();
            let (status, payload) = binding_write(&facts, body);
            assert_eq!(status, 400, "{body}");
            assert_eq!(
                payload.get("error").and_then(Value::as_str),
                Some("bad_request")
            );
            assert_eq!(binding_text(), before, "那两笔也没写");
        }

        // ⑤ 读侧 fail-closed：文件里那两格写坏了 ⇒ 那一行读成 `unreadable`（宿主会拒绝启动，
        //    壳说"没覆盖"就是撒谎）。
        std::fs::write(
            first.join("model_binding.json"),
            r#"{"source":"ms-1","reasoning_effort":"turbo"}"#,
        )
        .unwrap();
        let rows = config_snapshot(&facts);
        assert_eq!(rows["boundPerResident"][0]["state"], "unreadable");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// ★ 2026-10-18：**模型配置的见证**（"谁最后改过这一格"）。
    ///
    /// 症状（`agent/README.md` §4 那一格）：这一格从前**没有见证** —— 装配横幅报的是
    /// "这一拍从哪儿读的"，不是"谁改的"。判据四条，一条都不许压：
    /// ① **按份记**：这道门写了几份就记几条（清单 / 每一支的绑定各自一条）；
    /// ② **"清"也是一笔写**（`sha256: null`）—— 不记它，页面上的"解除指定"会读成"别处有人动过它"；
    /// ③ **两档分开**：门改的 ⇒ `same` **且有时刻**；门外那只手改的（她走 terminal）⇒
    ///    `changed/content`，而**是谁这条路上答不出来**（时刻仍是"门最后写于何时"）；
    /// ④ **判不了 ≠ 没改过**：从没被门碰过 ⇒ `no-witness`。
    #[test]
    fn the_door_leaves_a_witness_for_every_file_it_writes() {
        let root = std::env::temp_dir().join(format!("celira-door-witness-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let first = home.join("agents").join("a-1");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::write(first.join("identity.json"), r#"{"agent":"a-1"}"#).unwrap();
        crate::model_sources::write_binding(&first, "ms-1").unwrap();
        let entries: Vec<crate::model_sources::ModelSourceEntry> = ["ms-1", "ms-2"]
            .iter()
            .map(|id| crate::model_sources::ModelSourceEntry {
                id: (*id).to_string(),
                name: format!("钥匙 {id}"),
                endpoint: "https://x.invalid/v1/chat/completions".to_string(),
                model: "m".to_string(),
                context_window: 8192,
                protocol: None,
                reasoning_effort: None,
                vision: false,
                api_key: "k".to_string(),
            })
            .collect();
        // ⚠ 这两笔是**直接写文件**（不是门写的）⇒ 见证那一层此刻应当是 `no-witness`。
        crate::model_sources::write_list(&home, &entries).unwrap();
        let facts = LocalFacts::load(Some(&home), Some(&home));
        let list_path = crate::model_sources::list_path(&home);
        let binding_path = first.join(crate::model_sources::BINDING_FILE);

        // ④ 从没被门碰过 ⇒ 判不了（**不是**"没改过"）
        let before = config_snapshot(&facts);
        assert_eq!(before["listWitness"]["state"], "no-witness");
        assert!(before["listWitness"]["atMs"].is_null());
        assert_eq!(
            before["boundPerResident"][0]["witness"]["state"],
            "no-witness"
        );
        assert!(
            !crate::model_witness::witness_path(&home).exists(),
            "一个字节都没写"
        );

        // ① 门写一支的绑定 ⇒ 只记那一份（清单没动，照旧判不了）
        let (status, body) = binding_write(&facts, r#"{"source":"ms-2","agentId":"a-1"}"#);
        assert_eq!(status, 200, "{body}");
        let after = config_snapshot(&facts);
        assert_eq!(after["boundPerResident"][0]["witness"]["state"], "same");
        assert!(
            after["boundPerResident"][0]["witness"]["atMs"].is_number(),
            "门改的**有时刻**"
        );
        assert_eq!(
            after["listWitness"]["state"], "no-witness",
            "没写的那一份照旧判不了"
        );
        let records = crate::model_witness::read_records(&home).unwrap();
        assert_eq!(records.len(), 1, "一份文件一条");
        assert_eq!(
            records[0].file, "agents/a-1/model_binding.json",
            "**按份**记"
        );
        assert!(after["witnessChainBreakAt"].is_null(), "链是完整的");

        // ③ 门外那只手改的（她走 terminal）⇒ `changed/content`，而"是谁"答不出来
        std::fs::write(&binding_path, "{\n  \"source\": \"ms-1\"\n}\n").unwrap();
        let touched = config_snapshot(&facts);
        assert_eq!(
            touched["boundPerResident"][0]["witness"]["state"],
            "changed"
        );
        assert_eq!(
            touched["boundPerResident"][0]["witness"]["detail"],
            "content"
        );
        assert!(
            touched["boundPerResident"][0]["witness"]["atMs"].is_number(),
            "知道的只是**门最后写于何时**"
        );

        // ① 门写清单 ⇒ 清单那一份也记一条（这一刻它的字节就是刚写下去的那一份）
        let (status, body) = config_write(
            &facts,
            "PUT",
            r#"{"id":"ms-1","endpoint":"https://x.invalid/v1/chat/completions","api_key":"k","model":"m2","contextWindow":8192,"bind":false}"#,
        );
        assert_eq!(status, 200, "{body}");
        let written = config_snapshot(&facts);
        assert_eq!(written["listWitness"]["state"], "same");
        assert!(written["listWitness"]["atMs"].is_number());
        assert_eq!(
            crate::model_witness::read_records(&home).unwrap().len(),
            2,
            "两份文件两条（**按份**，不合并）"
        );
        assert!(list_path.exists() && binding_path.exists());

        // ② "清"也是一笔写：摘掉之后读数是 `same`（门摘的），而**不是**"别处有人动过它"
        let (status, body) = config_delete(&facts, None);
        assert_eq!(status, 200, "{body}");
        assert!(!binding_path.exists());
        let cleared = config_snapshot(&facts);
        assert_eq!(cleared["boundPerResident"][0]["witness"]["state"], "same");
        assert!(cleared["boundPerResident"][0]["witness"]["detail"].is_null());
        let last = crate::model_witness::read_records(&home)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(last.file, "agents/a-1/model_binding.json");
        assert!(last.sha256.is_none(), "那一刻它不在 —— 摘要那一格是 null");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// ★ **半途失败也不留"改了却没见证"的那一份**（2026-10-18）：**一支写成就记一条** ——
    /// 不许等整圈写完再记：那一圈半路抛了，前面**真被改过**的那几份就会读成"别处有人动过它"
    /// （把第一档报成第二档，而这一层存在的全部理由就是不许把这两档混成一件）。
    #[test]
    fn a_half_written_binding_still_carries_its_witness() {
        let root = std::env::temp_dir().join(format!("celira-door-half-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let first = home.join("agents").join("a-1");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::write(first.join("identity.json"), r#"{"agent":"a-1"}"#).unwrap();
        // 第二支：**住户在**（有 `identity.json`），而它的 `model_binding.json` 是一个**目录**
        // ⇒ 写到它必失败（原子写的最后一步是改名，改到一个目录上必失败）—— 这就是"半绑"。
        let second = home.join("agents").join("a-2");
        std::fs::create_dir_all(second.join("model_binding.json")).unwrap();
        std::fs::write(second.join("identity.json"), r#"{"agent":"a-2"}"#).unwrap();
        crate::model_sources::write_list(
            &home,
            &[crate::model_sources::ModelSourceEntry {
                id: "ms-1".to_string(),
                name: "主钥匙".to_string(),
                endpoint: "https://x.invalid/v1/chat/completions".to_string(),
                model: "m".to_string(),
                context_window: 8192,
                protocol: None,
                reasoning_effort: None,
                vision: false,
                api_key: "k".to_string(),
            }],
        )
        .unwrap();
        let facts = LocalFacts::load(Some(&home), Some(&home));

        let (status, body) = binding_write(&facts, r#"{"source":"ms-1"}"#);
        assert_eq!(status, 500, "半绑必须**如实失败**（不许静默）：{body}");
        let view = config_snapshot(&facts);
        let row = &view["boundPerResident"][0];
        assert_eq!(row["agentId"], "a-1");
        assert_eq!(row["witness"]["state"], "same", "先写成的那一份有见证");
        assert_eq!(
            crate::model_witness::read_records(&home)
                .unwrap()
                .iter()
                .map(|record| record.file.clone())
                .collect::<Vec<_>>(),
            vec!["agents/a-1/model_binding.json".to_string()],
            "只记真写成的那一份"
        );
        // 写失败的那一支**没有见证**：它那一份此刻也读不出来（那是一个目录）⇒ 如实说"判不了"。
        assert_eq!(
            view["boundPerResident"][1]["witness"]["state"],
            "unreadable"
        );
        assert_eq!(view["boundPerResident"][1]["witness"]["detail"], "file");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// ★ 2026-10-14：**只读目录**（`POST /home-config/catalog`）——"这条钥匙能调哪些型号"。
    ///
    /// 判据：`id` 认不出 ⇒ 404；没给 ⇒ 400；**响应里没有那把钥匙**；**一个字节都不落盘**；
    /// 连不上是**读数**不是错误（200 + `probe.ok=false`）——与探测那一格逐条同规。
    #[test]
    fn catalog_reads_the_stored_key_and_never_writes_or_leaks_it() {
        let root = std::env::temp_dir().join(format!("celira-door-catalog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let first = home.join("agents").join("a-1");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::write(first.join("identity.json"), r#"{"agent":"a-1"}"#).unwrap();
        crate::model_sources::write_binding(&first, "ms-1").unwrap();
        // 指向**一个刚放掉的端口**（几乎必然没人听）⇒ "连不上"是确定性的，且不碰外网。
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let canary = "sk-catalog-canary-7d31";
        let entries = vec![crate::model_sources::ModelSourceEntry {
            id: "ms-1".to_string(),
            name: "主钥匙".to_string(),
            endpoint: format!("http://127.0.0.1:{closed}/v1/chat/completions"),
            model: "m1".to_string(),
            context_window: 8192,
            protocol: None,
            reasoning_effort: None,
            vision: false,
            api_key: canary.to_string(),
        }];
        crate::model_sources::write_list(&home, &entries).unwrap();
        let list_before = std::fs::read_to_string(crate::model_sources::list_path(&home)).unwrap();
        let binding_before = std::fs::read_to_string(first.join("model_binding.json")).unwrap();
        let facts = LocalFacts::load(Some(&home), Some(&home));

        let (status, body) = catalog_action(&facts, r#"{"id":"ms-1"}"#);
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["probe"]["ok"], false, "连不上 ⇒ 读数是 false");
        assert_eq!(body["probe"]["reason"], "unreachable");
        assert!(!body.to_string().contains(canary), "钥匙不许进响应：{body}");
        assert_eq!(
            std::fs::read_to_string(crate::model_sources::list_path(&home)).unwrap(),
            list_before,
            "清单必须一个字节没动"
        );
        assert_eq!(
            std::fs::read_to_string(first.join("model_binding.json")).unwrap(),
            binding_before,
            "绑定必须一个字节没动"
        );

        // `id` 认不出 ⇒ 404；没给 ⇒ 400（都不猜）。
        let (status, body) = catalog_action(&facts, r#"{"id":"ms-9"}"#);
        assert_eq!(status, 404);
        assert_eq!(body["error"], "entry_not_found");
        let (status, body) = catalog_action(&facts, r#"{}"#);
        assert_eq!(status, 400);
        assert_eq!(body["error"], "bad_request");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// ★ **2026-10-08：住户清单 ＋ 顶层宿主那一格**（一个家住多支时页面靠它们才不认错人）。
    ///
    /// 两条判据，都是真机/契约咬出来的：
    /// ① `residents[]` **必须逐支点名**（id 精确相等是客户端认领的判据）—— 少了它，第 2 支起
    ///    的页面会写「还没有机器在跑它」（**假话**：它们就在这台机器上跑着，各住一个环境根）；
    /// ② `host` 的取值**必须落在 JS 门那一份词表里**（`running` / `running-orphan` / `stopped` /
    ///    `unknown`）—— 客户端按它归一，认不出的取值会**整格读成"没读数"**（不报错，也没信息）。
    ///    这一格从前**两个门都没给过** ⇒ 页面那句「机器在不在跑」永远只会说"读不到"。
    #[test]
    fn the_payload_names_every_resident_and_speaks_the_host_vocabulary() {
        let root = std::env::temp_dir().join(format!("celira-door-roster-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let repo = root.join("repo");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&repo).unwrap();

        // 两支住户（一支一个环境根；`slot` 决定次序 ⇒ 住户 1 是顶层那一支）。
        for (slot, id) in [(1_u32, "a-1"), (2, "a-2")] {
            let env_root = home.join("agents").join(id);
            std::fs::create_dir_all(&env_root).unwrap();
            std::fs::write(
                env_root.join("identity.json"),
                format!(r#"{{"agent":"{id}"}}"#),
            )
            .unwrap();
            std::fs::write(env_root.join("agent.conf"), format!("slot={slot}\n")).unwrap();
        }

        let payload = build_payload(&LocalFacts::load(Some(&home), Some(&repo)));

        let roster = payload
            .get("residents")
            .and_then(Value::as_array)
            .expect("住户清单必须在载荷里");
        let ids: Vec<&str> = roster
            .iter()
            .filter_map(|resident| resident.get("agentId").and_then(Value::as_str))
            .collect();
        assert_eq!(ids.len(), 2, "两支都要点名：{ids:?}");
        assert!(ids.contains(&"a-1") && ids.contains(&"a-2"), "{ids:?}");

        // 顶层那一格是**住户 1** 的（`slot` 小的先）—— 页面据此知道"其余几支不在顶层"。
        assert_eq!(
            payload
                .get("agent")
                .and_then(|agent| agent.get("agentId"))
                .and_then(Value::as_str),
            Some("a-1"),
            "顶层那一格必须是住户 1 的"
        );

        // 宿主词表：**逐支**都要在 JS 门那一份里（认不出 = 整格读成"没读数"）。
        const VOCABULARY: [&str; 4] = ["running", "running-orphan", "stopped", "unknown"];
        for resident in roster {
            let state = resident
                .get("host")
                .and_then(|host| host.get("state"))
                .and_then(Value::as_str)
                .expect("每一支都要有宿主那一格");
            assert!(VOCABULARY.contains(&state), "认不出的取值：{state}");
        }
        let top = payload
            .get("host")
            .and_then(|host| host.get("state"))
            .and_then(Value::as_str)
            .expect("顶层宿主那一格必须在（第一支的）");
        assert!(VOCABULARY.contains(&top), "认不出的取值：{top}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// ★ 2026-10-05：**认不出的形状不许说成不存在**。
    ///
    /// 壳只认新布局，而把老家（`<家>/env`）搬成新形态是 `up.js` 的事（app 不做迁移）
    /// ⇒ 老家会落到"零住户"，页面说"这台机器上还没有 Agent"，**而她明明在**。
    /// 这一格只报事实（`legacyHome: true`），人话归页面。
    #[test]
    fn a_legacy_shaped_home_is_reported_as_such_not_as_absent() {
        let root = std::env::temp_dir().join(format!("celira-door-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let repo = root.join("repo");
        std::fs::create_dir_all(home.join("env")).unwrap();
        std::fs::create_dir_all(&repo).unwrap();

        // ① 新形态的家（一支在 `agents/` 里）：**没有**这一格。
        let env_root = home.join("agents").join("a-1");
        std::fs::create_dir_all(&env_root).unwrap();
        std::fs::write(env_root.join("identity.json"), r#"{"agent":"a-1"}"#).unwrap();
        let payload = build_payload(&LocalFacts::load(Some(&home), Some(&repo)));
        assert!(
            payload.get("legacyHome").is_none(),
            "新形态的家不许带这一格（多一格就是把缺省说成事实）"
        );

        // ② 老家形态：`<家>/env` 里记着一支（此时 `agents/` 那一支先撤掉）
        std::fs::remove_dir_all(home.join("agents")).unwrap();
        std::fs::write(
            home.join("env").join("identity.json"),
            r#"{"agent":"legacy-1"}"#,
        )
        .unwrap();
        let payload = build_payload(&LocalFacts::load(Some(&home), Some(&repo)));
        assert_eq!(
            payload.get("legacyHome"),
            Some(&json!(true)),
            "老家要如实报出来"
        );
        assert_eq!(
            agent_state_of(&payload).as_deref(),
            Some("no-resident"),
            "读数那一格照旧说'这个门没有住户'——两件事分开说"
        );

        // ③ 认不出身份的 `<家>/env`（空目录）**不算**老家 —— 与 `legacyResident` 同款：
        //    两处都没有 id 就是"没有"，不猜。
        std::fs::remove_file(home.join("env").join("identity.json")).unwrap();
        let payload = build_payload(&LocalFacts::load(Some(&home), Some(&repo)));
        assert!(payload.get("legacyHome").is_none(), "空 env 不是老家");

        // ④ `home.conf` 里有 `agent_id`（环境已经不在）**也算** —— 与 JS 的 `legacyResident` 同一条。
        std::fs::write(home.join("home.conf"), "human_id=h-1\nagent_id=legacy-1\n").unwrap();
        let payload = build_payload(&LocalFacts::load(Some(&home), Some(&repo)));
        assert_eq!(payload.get("legacyHome"), Some(&json!(true)));

        let _ = std::fs::remove_dir_all(&root);
    }
}
