//! remote 通道：**网络上的 MCP 端点**（一次请求、一条答复）。
//!
//! 与 stdio 同一份 [`Transport`] 契约（`connect` / `list_tools` / `call_tool`），但下面那层不是
//! 管道而是一次次网络请求：JSON-RPC 放在 POST 正文里，答复可能是 `application/json`，
//! 也可能是一条事件流（SSE）里的事件。
//!
//! ## 这条通道的墙（与 stdio 的**不一样**）
//!
//! - **地址**：解析期就过（加密地址；私网/回环/链路本地一律拒；地址里不许带凭据）——见
//!   [`super::parse_declaration`] 的 `check_remote_address`；
//! - **重定向**：**不跟随**（一次重定向就是一个新的目的地，而目的地不在声明里）；
//! - **有界**：一条答复正文超过 [`MAX_BODY_BYTES`] 直接拒绝（控制帧不该长这么大）；
//! - **窗口**：每次请求都带声明的超时；超时按**效果不明**报（请求可能已经送到对面）；
//! - **凭据**：只从**环境变量**取（声明里存的是变量名），只进请求的那个头——
//!   不进日志、不进回执、不进主体的视野。
//!
//! ## 会话
//!
//! 首答里带 `Mcp-Session-Id` 就记下来，之后每个请求都带上；没带就是不保持会话的端点。
//! `notifications/initialized` 按规范发一次，**失败不致命**（它是通知，不是请求）。
//!
//! ## 今天**不做**的
//!
//! - 现代那一代（`2026-07-28`，无状态 + 每请求 `_meta`）：远程这一档只说 `initialize` 那一代；
//!   server 只认新一代时如实回 `unsupported-version`。
//! - 断线重连与事件续传（`Last-Event-ID`）：一次请求一次窗口，失败就是失败。
//! - DNS 层的私网复核：解析期只能看**字面地址**；主机名在连接时不做二次解析检查（已登记的边界）。

use std::io::Read;
use std::time::Duration;

use serde_json::{Value, json};

use super::wire::{as_object, describe_error, read_result, read_tool};
use super::wire_tokens::{ACCEPT, BODY_TYPE, EVENT_TYPE};
use super::{CallFault, CallReply, MAX_TOOLS, McpFailure, RawTool, Transport};

/// 一条答复正文的上限（超过就拒绝）。
const MAX_BODY_BYTES: u64 = 1024 * 1024;

/// 我们在远程这一档说的协议版本（`initialize` 握手那一代）。
const PROTOCOL_VERSION: &str = "2024-11-05";

/// JSON-RPC 的 `UnsupportedProtocolVersionError`（对面只认更新的一代时用它）。
const CODE_UNSUPPORTED_VERSION: i64 = -32022;

/// 一台远程 MCP server 的通道。
pub struct RemoteTransport {
    endpoint: String,
    /// **环境变量的名字**（不是取值）。
    credential_env: Option<String>,
    session: Option<String>,
    next_id: u64,
    client: reqwest::blocking::Client,
}

impl RemoteTransport {
    /// 建一条还没连的通道。**不跟随重定向**——那是这一档的第一道运行期墙。
    pub fn new(endpoint: &str, credential_env: Option<&str>) -> Result<Self, McpFailure> {
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| McpFailure::Transport)?;
        Ok(Self {
            endpoint: endpoint.to_string(),
            credential_env: credential_env.map(str::to_string),
            session: None,
            next_id: 1,
            client,
        })
    }

    fn credential(&self) -> Result<Option<String>, McpFailure> {
        match &self.credential_env {
            None => Ok(None),
            // 变量不在 = 这条通道**建不起来**（不是"对面拒绝"）：归类进 `transport`。
            Some(name) => std::env::var(name)
                .map(Some)
                .map_err(|_| McpFailure::Transport),
        }
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// 一次请求 → 对面的答复对象（`result` 或 `error` 两种都在里面）。
    fn round_trip(&mut self, payload: Value, timeout: Duration) -> Result<Value, CallFault> {
        let credential = self.credential().map_err(CallFault::Refused)?;
        let mut request = self
            .client
            .post(&self.endpoint)
            .timeout(timeout)
            .header("content-type", BODY_TYPE)
            .header("accept", ACCEPT)
            .header("mcp-protocol-version", PROTOCOL_VERSION)
            .json(&payload);
        if let Some(credential) = &credential {
            request = request.bearer_auth(credential);
        }
        if let Some(session) = &self.session {
            request = request.header("mcp-session-id", session);
        }
        let response = request.send().map_err(|error| {
            if error.is_timeout() {
                // 请求可能已经送到对面：**效果不明**，不是"确定没发"。
                CallFault::Indeterminate(McpFailure::Timeout)
            } else {
                CallFault::Refused(McpFailure::Transport)
            }
        })?;
        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
        {
            self.session = Some(session.to_string());
        }
        let status = response.status();
        let sse = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with(EVENT_TYPE));
        // 重定向：不跟随，也不当成答复。
        if status.is_redirection() {
            return Err(CallFault::Refused(McpFailure::Transport));
        }
        let mut body = String::new();
        response
            .take(MAX_BODY_BYTES)
            .read_to_string(&mut body)
            .map_err(|_| CallFault::Refused(McpFailure::Transport))?;
        if body.len() as u64 >= MAX_BODY_BYTES {
            return Err(CallFault::Refused(McpFailure::Transport));
        }
        if body.trim().is_empty() {
            if status.is_success() {
                return Ok(Value::Null);
            }
            return Err(CallFault::Refused(McpFailure::Transport));
        }
        let value = if sse {
            parse_sse(&body, &payload).ok_or(CallFault::Refused(McpFailure::Transport))?
        } else {
            as_object(&body).ok_or(CallFault::Refused(McpFailure::Transport))?
        };
        Ok(value)
    }

    /// 发一条**通知**（没有 id、不期待答复）。失败不致命。
    fn notify(&mut self, method: &str, params: Value, timeout: Duration) {
        let credential = self.credential().ok().flatten();
        let mut request = self
            .client
            .post(&self.endpoint)
            .timeout(timeout)
            .header("content-type", BODY_TYPE)
            .header("accept", ACCEPT)
            .json(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
        if let Some(credential) = &credential {
            request = request.bearer_auth(credential);
        }
        if let Some(session) = &self.session {
            request = request.header("mcp-session-id", session);
        }
        let _ = request.send();
    }
}

impl Transport for RemoteTransport {
    fn connect(&mut self, timeout: Duration) -> Result<(), CallFault> {
        let id = self.next_id();
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "celira", "version": env!("CARGO_PKG_VERSION")}
            }
        });
        let value = self.round_trip(payload, timeout)?;
        if let Some(error) = value.get("error") {
            let code = error.get("code").and_then(Value::as_i64);
            return Err(CallFault::Refused(
                if code == Some(CODE_UNSUPPORTED_VERSION) {
                    McpFailure::UnsupportedVersion
                } else {
                    McpFailure::HandshakeFailure
                },
            ));
        }
        let result = value
            .get("result")
            .ok_or(CallFault::Refused(McpFailure::HandshakeFailure))?;
        if result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .is_none()
        {
            return Err(CallFault::Refused(McpFailure::HandshakeFailure));
        }
        // 通知：发一次，失败不管（它是通知，不是请求）。
        self.notify("notifications/initialized", json!({}), timeout);
        Ok(())
    }

    fn list_tools(&mut self, timeout: Duration) -> Result<Vec<RawTool>, CallFault> {
        let mut tools: Vec<RawTool> = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let id = self.next_id();
            let params = match &cursor {
                Some(cursor) => json!({ "cursor": cursor }),
                None => json!({}),
            };
            let value = self.round_trip(
                json!({"jsonrpc": "2.0", "id": id, "method": "tools/list", "params": params}),
                timeout,
            )?;
            if value.get("error").is_some() {
                return Err(CallFault::Refused(McpFailure::ToolsFailed));
            }
            let result = value
                .get("result")
                .ok_or(CallFault::Refused(McpFailure::ToolsFailed))?;
            let page = result
                .get("tools")
                .and_then(Value::as_array)
                .ok_or(CallFault::Refused(McpFailure::ToolsFailed))?;
            for item in page {
                tools.push(read_tool(item).ok_or(CallFault::Refused(McpFailure::ToolsFailed))?);
                if tools.len() > MAX_TOOLS {
                    return Err(CallFault::Refused(McpFailure::TooManyTools));
                }
            }
            match result.get("nextCursor").and_then(Value::as_str) {
                Some(next) if !next.is_empty() => cursor = Some(next.to_string()),
                _ => break,
            }
        }
        Ok(tools)
    }

    fn call_tool(
        &mut self,
        tool: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<CallReply, CallFault> {
        let id = self.next_id();
        let value = self.round_trip(
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {"name": tool, "arguments": arguments}
            }),
            timeout,
        )?;
        if let Some(error) = value.get("error") {
            return Ok(CallReply::Refused {
                code: error.get("code").and_then(Value::as_i64),
                detail: describe_error(error),
            });
        }
        let result = value
            .get("result")
            .ok_or(CallFault::Refused(McpFailure::ServerError))?;
        Ok(read_result(result, 0))
    }
}

/// 从 SSE 正文里取出**这一次请求**的答复对象（按 `id` 对；对不上就取最后一个）。
fn parse_sse(body: &str, request: &Value) -> Option<Value> {
    let wanted = request.get("id").and_then(Value::as_i64);
    let mut fallback = None;
    for event in body.split("\n\n") {
        let mut data = String::new();
        for line in event.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(rest.trim_start());
            }
        }
        if data.is_empty() {
            continue;
        }
        let Some(value) = as_object(&data) else {
            continue;
        };
        if value.get("id").and_then(Value::as_i64) == wanted {
            return Some(value);
        }
        if value.get("result").is_some() || value.get("error").is_some() {
            fallback = Some(value);
        }
    }
    fallback
}

/// 读数用的端点：**去掉查询串与片段**（凭据可能就在查询串里）。
pub fn redacted_endpoint(address: &str) -> String {
    let without_fragment = address.split('#').next().unwrap_or(address);
    without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment)
        .to_string()
}
