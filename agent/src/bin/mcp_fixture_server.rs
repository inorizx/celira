//! 离线夹具：一个**最小的 stdio MCP server**（只给测试用）。
//!
//! 它说什么、怎么答，全部由下面的形状定死；它不是产品的一部分（它在 `src/bin/` 这个
//! 测试夹具区里，不参与分层守卫，也不进任何产品路径）。
//!
//! 三样东西让它能当判据的演员：
//!
//! 1. `--secret <值>`：只有**真的调到** `reveal` 那条动词才拿得到的值（测试用随机串传进来，
//!    因此"主体答出了它"只可能来自一次真实的工具调用——它不在主体的视野的任何别处）；
//! 2. `--marker <路径>`：**被起起来那一刻**往那个文件追一行（测试据此判"懒展开"：
//!    没展开之前那个文件不该存在）；
//! 3. 一组刻意做坏的动词（超长正文、非文本块、`isError`、错误对象、噪声行、慢、掉线）。
//!
//! 用法：`mcp_fixture_server [--secret <值>] [--marker <路径>] [--era <世代>] [--input-required] [--tools-change]`，
//! 读标准输入、写标准输出，每行一条 JSON。
//!
//! `--tools-change`（2026-10-25）：modern 代收到 `subscriptions/listen` 时先回
//! `notifications/subscriptions/acknowledged`，再发一条 `notifications/tools/list_changed`；
//! 与此同时 `tools/list` 多出一条 `after` 动词。它只给“通知到达后重拉清单”这条真进程用例当演员。
//!
//! `--era`（2026-10-13，给"两代的话都会说"当演员）：
//! - **不给** ＝ legacy：答 `initialize`；`server/discover` 走"不认识的 method"（`-32601`）
//!   ——那正是 legacy server 的真实行为，也正是 spec 说"回退不许按单一错误码判断"的原因；
//! - `modern-only` ＝ 只会说新版：拒 `initialize`（`-32022` ＋ `supported`），答 `server/discover`
//!   ⇒ celira 该**改用现代框子接着说**（`_meta` 进每个请求）；
//! - `modern-and-ours` ＝ 会说新版**也**说我们会的那一版 ⇒ 同上（说得到一起）；
//! - `modern-other` ＝ modern，但它说的版本我们**一个都不会**（`2027-99`）
//!   ⇒ **只有这一格**才该报 `unsupported-version`。
//! - `modern-other-silent` ＝ 同上，但拒的时候**一个字的 message 都不给**（`error` 只有 `code`）
//!   ⇒ 它是"对面没留话"那一格的演员：**「没有」与「读不到」必须分得开**——回执里
//!   不该因此多出一块空的诊断（见 `app_contract::ExpansionRefusal` 的文档）。
//!
//! ⚠ **modern 那三档会拒不带 `_meta` 的 `tools/list` / `tools/call`**（回 `-32602`）——
//! 它把"我们真的说了现代话"变成一条**读数**，而不是靠"没报错"倒推。
//!
//! `--input-required`：每一次 `tools/call` 都回 `input_required`（spec `2026-07-28` 的 MRTR）
//! ⇒ celira 该把它读成**如实的拒绝**，不许读成成功。

use std::io::{BufRead, Write};
use std::process::ExitCode;

use serde_json::{Value, json};

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let secret = argument(&arguments, "--secret");
    let marker = argument(&arguments, "--marker");
    let era = argument(&arguments, "--era").unwrap_or_default();
    // `--input-required`：每一次 `tools/call` 都回 `input_required`（MRTR）。
    let input_required = arguments.iter().any(|each| each == "--input-required");
    // `--tools-change`：订阅后发一条清单变更通知，并让后续 `tools/list` 多一条动词。
    let tools_change = arguments.iter().any(|each| each == "--tools-change");
    // 订阅是否已经发生：只有发生之后，`tools/list` 才换成新清单（展开那一次仍旧拿旧清单）。
    let mut tools_changed = false;

    if let Some(path) = &marker {
        // 起没起过，**只有起了才知道**：这一行就是那个证据。
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "started pid={}", std::process::id());
        }
    }

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let id = message.get("id").cloned();

        match method.as_str() {
            // 通知：没有答复。
            "notifications/initialized" => {}
            "initialize" => {
                if era.starts_with("modern") {
                    // 只会说新版：spec 说 server **SHOULD** 在拒 `initialize` 时列出它支持的版本
                    // ——因为"这可能是老客户端唯一能拿到并展示给用户的诊断"。
                    //
                    // ⚠ `modern-other-silent` 是那条 SHOULD 的**反例演员**：它拒得一样干脆，
                    // 但**一个字的 message 都不给**（`error` 只有 `code`）⇒ 我们这一侧
                    // 必须如实"没有"，而不是编一句话、也不是留一块空的诊断。
                    if era == "modern-other-silent" {
                        error_silent(
                            &mut stdout,
                            id,
                            -32022,
                            json!({"supported": supported_versions(&era), "requested": "2024-11-05"}),
                        );
                    } else {
                        error_with_data(
                            &mut stdout,
                            id,
                            -32022,
                            "fixture: 不支持这个协议版本",
                            json!({"supported": supported_versions(&era), "requested": "2024-11-05"}),
                        );
                    }
                } else {
                    reply(
                        &mut stdout,
                        id,
                        json!({
                            "protocolVersion": "2024-11-05",
                            "capabilities": {"tools": {}},
                            "serverInfo": {"name": "celira-mcp-fixture", "version": "0"},
                        }),
                    );
                }
            }
            // 探测对面是哪一代（spec `2026-07-28`）：只有 modern 那一档会答。
            "server/discover" => {
                if era.starts_with("modern") {
                    reply(
                        &mut stdout,
                        id,
                        json!({"supportedVersions": supported_versions(&era)}),
                    );
                } else {
                    // legacy 的真实行为：对未知的 pre-`initialize` 请求回**实现自定义**的码。
                    error(&mut stdout, id, -32601, "fixture: 不认识的 method");
                }
            }
            "tools/list" => {
                if modern_without_meta(&era, &message) {
                    error(&mut stdout, id, -32602, "fixture: modern 请求缺 `_meta`");
                } else {
                    let tools = if tools_changed {
                        catalogue_changed()
                    } else {
                        catalogue()
                    };
                    reply(&mut stdout, id, json!({"tools": tools}));
                }
            }
            "subscriptions/listen" => {
                if era.starts_with("modern") {
                    let subscription = id.clone().unwrap_or(Value::Null);
                    let meta = json!({
                        "io.modelcontextprotocol/subscriptionId": subscription,
                    });
                    // spec：ack 是第一条带订阅 id 的消息；通知只能排在它后面。
                    if !send(
                        &mut stdout,
                        json!({
                            "jsonrpc": "2.0",
                            "method": "notifications/subscriptions/acknowledged",
                            "params": {
                                "_meta": meta,
                                "notifications": {"toolsListChanged": true},
                            },
                        }),
                    ) {
                        return ExitCode::SUCCESS;
                    }
                    if tools_change {
                        tools_changed = true;
                        if !send(
                            &mut stdout,
                            json!({
                                "jsonrpc": "2.0",
                                "method": "notifications/tools/list_changed",
                                "params": {"_meta": {
                                    "io.modelcontextprotocol/subscriptionId": subscription,
                                }},
                            }),
                        ) {
                            return ExitCode::SUCCESS;
                        }
                    }
                } else {
                    error(
                        &mut stdout,
                        id,
                        -32601,
                        "fixture: legacy 没有 subscriptions/listen",
                    );
                }
            }
            "tools/call" => {
                if modern_without_meta(&era, &message) {
                    error(&mut stdout, id, -32602, "fixture: modern 请求缺 `_meta`");
                } else if input_required {
                    // MRTR：对面"要多问一句才肯办"（spec `2026-07-28`）。celira **不实现**那一格
                    // ⇒ 它该把这一次读成**如实的拒绝**，不许读成成功。
                    reply(
                        &mut stdout,
                        id,
                        json!({
                            "resultType": "input_required",
                            "inputRequests": [{"kind": "fixture", "prompt": "再给一点信息"}],
                        }),
                    );
                } else {
                    let params = message.get("params").cloned().unwrap_or(Value::Null);
                    if !call(&mut stdout, id, &params, secret.as_deref()) {
                        // 掉线那条：一声不响地退出（对面看到的是"发出去之后通道没了"）。
                        return ExitCode::SUCCESS;
                    }
                }
            }
            _ => {
                error(&mut stdout, id, -32601, "fixture: 不认识的 method");
            }
        }
    }
    ExitCode::SUCCESS
}

/// 命令行上的一个可选项。
fn argument(arguments: &[String], name: &str) -> Option<String> {
    arguments
        .iter()
        .position(|each| each == name)
        .and_then(|index| arguments.get(index + 1))
        .cloned()
}

/// 清单变更通知之后的那份清单：原清单 ＋ 一条 `after`。
fn catalogue_changed() -> Value {
    let mut tools = catalogue().as_array().cloned().unwrap_or_default();
    tools.push(json!({
        "name": "after",
        "description": "订阅之后才出现的动词。",
        "inputSchema": {"type": "object", "properties": {}},
    }));
    Value::Array(tools)
}

/// 这台 server 暴露的动词。
fn catalogue() -> Value {
    json!([
        {
            "name": "echo",
            "description": "把给的文本原样送回来。",
            "inputSchema": {
                "type": "object",
                "properties": {"text": {"type": "string", "description": "要送回的文本"}},
                "required": ["text"],
            },
        },
        {
            "name": "reveal",
            "description": "说出这台 server 自己知道的那个值（只有真的调到它才拿得到）。",
            "inputSchema": {"type": "object", "properties": {}},
        },
        {
            "name": "where",
            "description": "报告它自己在哪个目录、进程号是多少。",
            "inputSchema": {"type": "object", "properties": {}},
        },
        {
            "name": "numbers",
            "description": "把三种标量参数按 JSON 类型原样送回来。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "count": {"type": "integer"},
                    "ratio": {"type": "number"},
                    "flag": {"type": "boolean"},
                    "label": {"type": "string"},
                },
                "required": ["count"],
            },
        },
        {
            "name": "big",
            "description": "回一大段文本（测回执墙）。",
            "inputSchema": {
                "type": "object",
                "properties": {"size": {"type": "integer"}},
            },
        },
        {
            "name": "mixed",
            "description": "回一段文本加两个非文本块。",
            "inputSchema": {"type": "object", "properties": {}},
        },
        {
            "name": "fail",
            "description": "用 isError 如实说这条动词没成。",
            "inputSchema": {"type": "object", "properties": {}},
        },
        {
            "name": "blowup",
            "description": "回一个 JSON-RPC 错误对象（内部错误）。",
            "inputSchema": {"type": "object", "properties": {}},
        },
        {
            "name": "badparams",
            "description": "回一个「参数不合法」的错误对象。",
            "inputSchema": {"type": "object", "properties": {}},
        },
        {
            "name": "nested",
            "description": "参数里有一个数组：我们的参数表表达不了这种形状。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "paths": {"type": "array", "items": {"type": "string"}},
                },
            },
        },
        {
            "name": "noisy",
            "description": "回话之前先往输出里写一行不是协议的东西。",
            "inputSchema": {"type": "object", "properties": {}},
        },
        {
            "name": "slow",
            "description": "睡很久（测转发窗口）。",
            "inputSchema": {
                "type": "object",
                "properties": {"seconds": {"type": "integer"}},
            },
        },
        {
            "name": "hangup",
            "description": "收下请求，然后一声不响地退出。",
            "inputSchema": {"type": "object", "properties": {}},
        },
    ])
}

/// 转一次调用；返回"要不要继续活着"（掉线那条返回假）。
fn call(
    stdout: &mut std::io::Stdout,
    id: Option<Value>,
    params: &Value,
    secret: Option<&str>,
) -> bool {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
    let text = |key: &str| {
        arguments
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let number = |key: &str, fallback: i64| {
        arguments
            .get(key)
            .and_then(Value::as_i64)
            .unwrap_or(fallback)
    };

    match name {
        "echo" => text_result(stdout, id, &format!("echo: {}", text("text")), false),
        "reveal" => text_result(
            stdout,
            id,
            &format!("reveal: {}", secret.unwrap_or("(没有给 secret)")),
            false,
        ),
        "where" => text_result(
            stdout,
            id,
            &format!(
                "cwd={} pid={}",
                std::env::current_dir()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|_| "unknown".to_string()),
                std::process::id()
            ),
            false,
        ),
        "numbers" => text_result(stdout, id, &format!("arguments-as-seen={arguments}"), false),
        "big" => {
            let size = number("size", 96 * 1024).clamp(0, 4 * 1024 * 1024) as usize;
            text_result(stdout, id, &"x".repeat(size), false)
        }
        "mixed" => reply(
            stdout,
            id,
            json!({
                "isError": false,
                "content": [
                    {"type": "text", "text": "这里有文本，也有别的东西。"},
                    {"type": "image", "data": "AAAA", "mimeType": "image/png"},
                    {"type": "image", "data": "BBBB", "mimeType": "image/png"},
                ],
            }),
        ),
        "fail" => text_result(stdout, id, "这条动词自己说它没成：示例原因", true),
        "blowup" => error(stdout, id, -32603, "fixture: 内部错误示例"),
        "badparams" => error(stdout, id, -32602, "fixture: 参数不合法示例"),
        "noisy" => {
            // 协议之外的一行：对面往输出里写日志（那是它的毛病，但也不能当没发生）。
            let _ = writeln!(stdout, "fixture: 这不是一行协议");
            let _ = stdout.flush();
            text_result(stdout, id, "噪声之后的答复", false)
        }
        "slow" => {
            let seconds = number("seconds", 30).clamp(0, 300) as u64;
            std::thread::sleep(std::time::Duration::from_secs(seconds));
            text_result(stdout, id, "睡醒了", false)
        }
        // 掉线：**不答复**，直接退出（对面看到的是"发出去之后通道没了"）。
        "hangup" => return false,
        other => error(
            stdout,
            id,
            -32602,
            &format!("fixture: 不认识的 tool {other}"),
        ),
    };
    true
}

/// 一条 `isError` 答复。
fn text_result(
    stdout: &mut std::io::Stdout,
    id: Option<Value>,
    text: &str,
    is_error: bool,
) -> bool {
    reply(
        stdout,
        id,
        json!({"isError": is_error, "content": [{"type": "text", "text": text}]}),
    )
}

/// 一条答复。
fn reply(stdout: &mut std::io::Stdout, id: Option<Value>, result: Value) -> bool {
    send(
        stdout,
        json!({"jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result}),
    )
}

/// 一条错误答复。
fn error(stdout: &mut std::io::Stdout, id: Option<Value>, code: i64, message: &str) -> bool {
    send(
        stdout,
        json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(Value::Null),
            "error": {"code": code, "message": message},
        }),
    )
}

/// 一条**带 `data`** 的错误答复（版本协商错误要它：`supported` 列表住在 `data` 里）。
fn error_with_data(
    stdout: &mut std::io::Stdout,
    id: Option<Value>,
    code: i64,
    message: &str,
    data: Value,
) -> bool {
    send(
        stdout,
        json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(Value::Null),
            "error": {"code": code, "message": message, "data": data},
        }),
    )
}

/// 一条**没有 `message`、但 `data.supported` 照给**的错误答复（`modern-other-silent` 的演员）。
///
/// ⚠ 两格都必须在，缺一个就演错了角色：
/// - **`data.supported` 在** ⇒ 它仍然是"探得出 modern、而它说的版本我们一个都不会"
///   （`unsupported-version`）。少了它，这一格会落回**含糊的** `handshake-failure`
///   ——那是"原因不明"，不是"版本不支持"（`MCP_SERVER_GUIDE.md` §9 登记过这一条）。
///   ⚠ 第一版夹具**只给 `code`、漏了 `data.supported`** ⇒ 用例当场红在分类上：
///   **那是用例造错了格子，不是实现错了**（分类照设计含糊，两格缺一不可）。
/// - **`message` 不在** ⇒ 这正是要演的那一格：**「对面没留话」必须与「我们读不到」分得开**。
///   spec 对拒 `initialize` 只写 SHOULD 给诊断 ⇒ 真机上有 server 就是不给——我们这一侧
///   必须如实"没有"，而不是编一句话、也不是留一块空的诊断。
fn error_silent(stdout: &mut std::io::Stdout, id: Option<Value>, code: i64, data: Value) -> bool {
    send(
        stdout,
        json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(Value::Null),
            "error": {"code": code, "data": data},
        }),
    )
}

/// 这个世代自报支持哪些协议版本。
fn supported_versions(era: &str) -> Value {
    match era {
        // 会说新版，**也**说我们会的那一版 ⇒ 探得出 modern，而两边**说得到一起**。
        "modern-and-ours" => json!(["2026-07-28", "2024-11-05"]),
        // modern，但它说的版本我们**一个都不会** ⇒ **只有这一格**才该报 `unsupported-version`。
        // `modern-other-silent` 与它**同一档版本**，差别只在"给不给 message"（见 `error_silent`）。
        "modern-other" | "modern-other-silent" => json!(["2027-99"]),
        _ => json!(["2026-07-28"]),
    }
}

/// 一个 **modern 代却没带 `_meta`** 的请求。
///
/// fixture 用它把"我们真的发了 `_meta`"变成一条**读数**——否则用例只能证明"它没报错"，
/// 证明不了"我们说了现代话"。
fn modern_without_meta(era: &str, message: &Value) -> bool {
    era.starts_with("modern")
        && message
            .get("params")
            .and_then(|params| params.get("_meta"))
            .is_none()
}

/// 写一行出去（立刻刷）。
fn send(stdout: &mut std::io::Stdout, message: Value) -> bool {
    if writeln!(stdout, "{message}").is_err() {
        return false;
    }
    stdout.flush().is_ok()
}
