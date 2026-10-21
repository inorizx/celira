//! 远程 MCP 通道（`mcp/remote.rs`）的验收：**一次真的 HTTP 往返**，不是 mock。
//!
//! 本文件起一个**真的 TCP 服务**（`std::net::TcpListener`），按 MCP 的三步说话
//! （`initialize` / `tools/list` / `tools/call`），然后走 `RemoteTransport` 走一遍。
//!
//! 钉住四件事：
//! 1. **三步都通**：握手拿到会话、清单（含翻页）与调用答复都按有界形状回来；
//! 2. **会话 id 带下去**：第二、三个请求带着首答给的那个头；
//! 3. **SSE 也能读**：答复是事件流时，按 `data:` 取出那一条；
//! 4. **三道墙各红一次**：重定向不跟随、超大有界、地址墙（解析期）拒绝私网/明文/带凭据。
//!
//! ⚠ **不测的**：TLS（本机测试只跑明文回环）、真实公网端点的行为、断线重连（今天不做）。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use celira_agent::mcp::remote::RemoteTransport;
use celira_agent::mcp::{
    CallFault, CallReply, McpEndpoint, McpFailure, Transport, parse_declaration,
};

/// 一次请求（只留断言用得上的那几样）。
#[derive(Debug)]
struct Request {
    headers: HashMap<String, String>,
    body: String,
}

impl Request {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

struct Response {
    status: u16,
    content_type: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Response {
    fn json(body: serde_json::Value) -> Self {
        Self {
            status: 200,
            content_type: "application/json".to_string(),
            headers: Vec::new(),
            body: body.to_string(),
        }
    }

    fn empty(status: u16) -> Self {
        Self {
            status,
            content_type: "text/plain".to_string(),
            headers: Vec::new(),
            body: String::new(),
        }
    }

    fn with(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    fn sse(body: String) -> Self {
        Self {
            status: 200,
            content_type: "text/event-stream".to_string(),
            headers: Vec::new(),
            body,
        }
    }
}

/// 起一个只服务 `requests` 次的 HTTP 服务；返回 `(端点, 收到的请求)`。
fn spawn_server<F>(requests: usize, handler: F) -> (String, Arc<Mutex<Vec<Request>>>)
where
    F: Fn(&Request) -> Response + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("本机回环可以监听");
    let port = listener.local_addr().expect("有本地地址").port();
    let seen: Arc<Mutex<Vec<Request>>> = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    thread::spawn(move || {
        for _ in 0..requests {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
            let request = match read_request(&mut stream) {
                Some(request) => request,
                None => return,
            };
            let response = handler(&request);
            log.lock().unwrap().push(request);
            let mut head = format!(
                "HTTP/1.1 {} X\r\nContent-Length: {}\r\nContent-Type: {}\r\nConnection: close\r\n",
                response.status,
                response.body.len(),
                response.content_type
            );
            for (name, value) in &response.headers {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            head.push_str("\r\n");
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(response.body.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://127.0.0.1:{port}/mcp"), seen)
}

fn read_request(stream: &mut std::net::TcpStream) -> Option<Request> {
    let mut raw = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read = stream.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&buffer[..read]);
        if let Some(at) = find_header_end(&raw) {
            let head = String::from_utf8_lossy(&raw[..at]).to_string();
            let mut headers = HashMap::new();
            for line in head.lines().skip(1) {
                if let Some((name, value)) = line.split_once(':') {
                    headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
                }
            }
            let length: usize = headers
                .get("content-length")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            let body_start = at + 4;
            while raw.len() < body_start + length {
                let read = stream.read(&mut buffer).ok()?;
                if read == 0 {
                    break;
                }
                raw.extend_from_slice(&buffer[..read]);
            }
            let body = String::from_utf8_lossy(&raw[body_start..body_start + length]).to_string();
            return Some(Request { headers, body });
        }
    }
    None
}

fn find_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|window| window == b"\r\n\r\n")
}

/// 一个会说三句话的最小 server。
fn mcp_server(request: &Request) -> Response {
    let body = request.json();
    let id = body.get("id").cloned().unwrap_or(serde_json::Value::Null);
    match body.get("method").and_then(|each| each.as_str()) {
        Some("initialize") => Response::json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": {"name": "fake", "version": "1"}
            }
        }))
        .with("Mcp-Session-Id", "session-abc"),
        Some("notifications/initialized") => Response::empty(202),
        Some("tools/list") => {
            let page = body
                .get("params")
                .and_then(|params| params.get("cursor"))
                .and_then(|cursor| cursor.as_str())
                .map(str::to_string);
            let (tools, next) = match page.as_deref() {
                None => (
                    serde_json::json!([{"name": "echo", "description": "回声", "inputSchema": {"type": "object"}}]),
                    serde_json::Value::String("page-2".to_string()),
                ),
                _ => (
                    serde_json::json!([{"name": "ping", "inputSchema": {"type": "object"}}]),
                    serde_json::Value::Null,
                ),
            };
            Response::json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"tools": tools, "nextCursor": next}
            }))
        }
        Some("tools/call") => Response::json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {"content": [{"type": "text", "text": "hello from the other side"}]}
        })),
        _ => Response::json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": "unknown method"}
        })),
    }
}

#[test]
fn a_remote_server_speaks_the_same_three_steps_and_the_session_is_carried() {
    // initialize + initialized + list(page1) + list(page2) + call = 5 次请求。
    let (endpoint, seen) = spawn_server(5, mcp_server);
    let mut transport = RemoteTransport::new(&endpoint, None).expect("建得起来");
    transport
        .connect(Duration::from_secs(3))
        .expect("握手应当成功");
    let tools = transport
        .list_tools(Duration::from_secs(3))
        .expect("清单应当拉得到");
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(names, ["echo", "ping"], "两页都读了");
    let reply = transport
        .call_tool(
            "echo",
            serde_json::json!({"text": "hi"}),
            Duration::from_secs(3),
        )
        .expect("调用应当成功");
    match reply {
        CallReply::Replied {
            text,
            is_error,
            truncated,
            ..
        } => {
            assert_eq!(text, "hello from the other side");
            assert!(!is_error);
            assert!(!truncated);
        }
        other => panic!("应当是答复：{other:?}"),
    }
    let requests = seen.lock().unwrap();
    assert_eq!(requests.len(), 5);
    assert!(
        !requests[0].headers.contains_key("mcp-session-id"),
        "第一个请求还没有会话"
    );
    for request in requests.iter().skip(1) {
        assert_eq!(
            request.headers.get("mcp-session-id").map(String::as_str),
            Some("session-abc"),
            "首答给的会话要一路带下去"
        );
    }
    // 第四个请求是第二页（带游标）。
    assert_eq!(
        requests[3]
            .json()
            .get("params")
            .and_then(|params| params.get("cursor"))
            .and_then(|cursor| cursor.as_str()),
        Some("page-2")
    );
}

#[test]
fn an_event_stream_reply_is_read_like_a_plain_one() {
    let (endpoint, _seen) = spawn_server(3, |request| {
        let body = request.json();
        let id = body.get("id").cloned().unwrap_or(serde_json::Value::Null);
        match body.get("method").and_then(|each| each.as_str()) {
            Some("initialize") => Response::json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"protocolVersion": "2024-11-05"}
            })),
            Some("notifications/initialized") => Response::empty(202),
            _ => Response::sse(format!(
                "event: message\ndata: {{\"jsonrpc\":\"2.0\",\"id\":{},\"result\":{{\"content\":[{{\"type\":\"text\",\"text\":\"from sse\"}}]}}}}\n\n",
                id
            )),
        }
    });
    let mut transport = RemoteTransport::new(&endpoint, None).expect("建得起来");
    transport
        .connect(Duration::from_secs(3))
        .expect("握手应当成功");
    let reply = transport
        .call_tool("echo", serde_json::json!({}), Duration::from_secs(3))
        .expect("调用应当成功");
    match reply {
        CallReply::Replied { text, .. } => assert_eq!(text, "from sse"),
        other => panic!("应当是答复：{other:?}"),
    }
}

#[test]
fn a_redirect_is_refused_and_never_followed() {
    // 只服务一次：initialize 就被重定向。
    let (endpoint, _seen) = spawn_server(1, |_request| {
        Response::empty(302).with("Location", "http://127.0.0.1:1/elsewhere")
    });
    let mut transport = RemoteTransport::new(&endpoint, None).expect("建得起来");
    let fault = transport
        .connect(Duration::from_secs(3))
        .expect_err("重定向必须被拒");
    assert!(matches!(fault, CallFault::Refused(McpFailure::Transport)));
}

#[test]
fn an_oversized_reply_is_refused_instead_of_read() {
    let (endpoint, _seen) = spawn_server(1, |_request| Response {
        status: 200,
        content_type: "application/json".to_string(),
        headers: Vec::new(),
        body: "x".repeat(1024 * 1024 + 8),
    });
    let mut transport = RemoteTransport::new(&endpoint, None).expect("建得起来");
    let fault = transport
        .connect(Duration::from_secs(3))
        .expect_err("超大正文必须被拒");
    assert!(matches!(fault, CallFault::Refused(McpFailure::Transport)));
}

#[test]
fn a_missing_credential_variable_makes_the_channel_unusable() {
    let (endpoint, _seen) = spawn_server(0, |_request| Response::empty(200));
    let mut transport =
        RemoteTransport::new(&endpoint, Some("CELIRA_TEST_SURELY_UNSET")).expect("建得起来");
    let fault = transport
        .connect(Duration::from_secs(3))
        .expect_err("凭据变量不在 ⇒ 这条通道不可用");
    assert!(matches!(fault, CallFault::Refused(McpFailure::Transport)));
}

#[test]
fn the_address_wall_refuses_what_it_should() {
    // 明文只给本机回环。
    assert!(parse_declaration("x=http://example.com/mcp").is_err());
    assert!(parse_declaration("x=https://example.com/mcp").is_ok());
    assert!(parse_declaration("x=http://127.0.0.1:9/mcp").is_ok());
    // 私网/回环/链路本地一律拒（字面地址）。
    assert!(parse_declaration("x=https://10.1.2.3/mcp").is_err());
    assert!(parse_declaration("x=https://192.168.1.5/mcp").is_err());
    assert!(parse_declaration("x=https://169.254.169.254/mcp").is_err());
    assert!(parse_declaration("x=https://[fe80::1]/mcp").is_err());
    // 地址里带凭据：拒（凭据走 `credential-env=`）。
    assert!(parse_declaration("x=https://user:pass@example.com/mcp").is_err());
    // 远程端点不接受 stdio 的那几个词里的 `credential-env` 之外的东西。
    assert!(parse_declaration("x=stdio:echo,credential-env=FOO").is_err());
    // 正例：效果档与窗口仍从末尾剥。
    let declaration =
        parse_declaration("x=https://example.com/mcp,read-only,timeout=30,credential-env=MCP_KEY")
            .expect("合法");
    assert!(declaration.is_remote());
    assert_eq!(
        declaration.remote_address(),
        Some("https://example.com/mcp")
    );
    assert_eq!(declaration.remote_credential_env(), Some("MCP_KEY"));
    assert!(matches!(declaration.endpoint(), McpEndpoint::Remote { .. }));
    assert_eq!(declaration.timeout_seconds(), 30);
}
