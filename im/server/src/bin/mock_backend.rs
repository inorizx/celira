//! 独立 Mock Agent Backend：真实独立进程协议客户端。
//!
//! 本二进制只作为 Celira Core 的公开协议消费者存在：仅使用真实 Backend
//! Credential 调用 `GET /v1/backend/events` 与
//! `POST /v1/backend/rooms/{room_id}/messages`（say）。它不导入 Core 内部
//! 模块、不调用 handler、不直接读取 SQLite。
//!
//! ⚠ **2026-09-28（Completion/ACK 整体退役）**：本进程原为「拉 Event →
//! POST Completion → ACK」；退役后改走**拉取 + say**——拉取模型下主体自己
//! 说，没有任何东西需要向 im 侧确认。
//!
//! 恢复契约：Mock 自己记一条本地 cursor（把响应里的 `next_after` 原样回传），
//! 断线/重启后从起点重读；重发由 say 的幂等键收敛——同一 Event 固定
//! `mock-backend-<event-id>` ⇒ 同摘要重放 200，不产生第二条公共 Message。
//!
//! Echo 规则：对每条 `message.created` Event——
//! - source Message 有正文且未撤回：say
//!   `{"body":"echo: <source-body>"}`，Idempotency-Key =
//!   `mock-backend-<event-id>`（固定 key 保证 HTTP 重试收敛）；
//! - source Message 无正文（附件-only）或已撤回（tombstone）：只观察、不发言
//!   （Echo 后端没有可回声的正文）；
//! - `message.recalled` 是更正通知、不是生成请求：同样只观察、不发言。
//!
//! 配置：
//!   CELIRA_IM_URL       Core 基础 URL，默认 http://127.0.0.1:8080
//!   CELIRA_BACKEND_TOKEN  Backend Credential（必填）
//!   CELIRA_MOCK_POLL_MS   无事件时的轮询间隔，默认 300
//!
//! 退出：Backend 认证失败（401）时以非零码退出；SIGTERM/SIGINT 由操作系统
//! 终止进程。Mock 不持有本地消费位置，重启后仍从 Core 的 ACK 位置恢复。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::ExitCode;
use std::time::Duration;

struct HttpResponse {
    status: u16,
    body: String,
}

fn http_request(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
    token: &str,
    json_body: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<HttpResponse, String> {
    let mut stream = TcpStream::connect((host, port)).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let body = json_body.unwrap_or_default();
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\
         Authorization: Bearer {token}\r\nContent-Length: {}\r\n",
        body.len()
    );
    if json_body.is_some() {
        request.push_str("Content-Type: application/json\r\n");
    }
    if let Some(key) = idempotency_key {
        request.push_str(&format!("Idempotency-Key: {key}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body_bytes) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| "malformed response".to_owned())?;
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let mut content_length = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(": ")
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.parse::<usize>().ok();
        }
    }
    let body = match content_length {
        Some(len) => body_bytes.chars().take(len).collect::<String>(),
        None => body_bytes.to_owned(),
    };
    Ok(HttpResponse { status, body })
}

/// 把 source body 回声为 reply body：保证结果满足普通 Message 正文规则
/// （1–16384 UTF-8 bytes、无 NUL），超长时按字符边界截断。
fn echo_body(source: &str) -> String {
    const PREFIX: &str = "echo: ";
    const MAX: usize = 16384;
    let budget = MAX - PREFIX.len();
    let mut result = String::with_capacity(PREFIX.len() + source.len().min(budget));
    result.push_str(PREFIX);
    for ch in source.chars() {
        if result.len() + ch.len_utf8() > MAX {
            break;
        }
        result.push(ch);
    }
    result
}

/// 以成员身份对一条 Event say 一条回声。
///
/// 调用方已筛掉不该发言的 Event（非 `message.created`、撤回 tombstone、
/// 附件-only）。401 一律作为致命错误返回（Backend 认证已失效）；其余失败只
/// 记录并按下一轮重试（say 的幂等键保证重发收敛）。
fn handle_event(
    host: &str,
    port: u16,
    token: &str,
    event_id: &str,
    room_id: &str,
    source_body: &str,
) -> Result<(), String> {
    let path = format!("/v1/backend/rooms/{room_id}/messages");
    let key = format!("mock-backend-{event_id}");
    let payload = serde_json::json!({ "body": echo_body(source_body) });
    match http_request(
        host,
        port,
        "POST",
        &path,
        token,
        Some(&payload.to_string()),
        Some(&key),
    ) {
        Ok(resp) if resp.status == 201 => {
            eprintln!("said echo for event {event_id}");
            Ok(())
        }
        Ok(resp) if resp.status == 200 => {
            eprintln!("event {event_id} already answered (idempotent replay)");
            Ok(())
        }
        Ok(resp) if resp.status == 401 => Err("backend credential rejected (401)".to_owned()),
        Ok(resp) => {
            eprintln!(
                "say for {event_id} failed with HTTP {}, retrying",
                resp.status
            );
            Ok(())
        }
        Err(e) => {
            eprintln!("say for {event_id} failed: {e}, retrying");
            Ok(())
        }
    }
}

/// 从 Mock 自己的 cursor 之后拉取一页 Event 并逐条处理（拉取 + say）。
///
/// 本地 cursor 是 Mock 的恢复点：`next_after` 原样回传；重启后从起点重读，
/// 重发由 say 的幂等键收敛（同摘要重放 200，不产生第二条公共 Message）。
fn poll_and_reply(core_url: &str, token: &str, cursor: &mut Option<String>) -> Result<(), String> {
    let (host, port, _) = parse_url(core_url)?;
    let path = match cursor.as_deref() {
        Some(after) => format!("/v1/backend/events?after={after}&limit=100"),
        None => "/v1/backend/events?limit=100".to_owned(),
    };
    let response = http_request(&host, port, "GET", &path, token, None, None)?;
    if response.status == 401 {
        return Err("backend credential rejected (401)".to_owned());
    }
    if response.status != 200 {
        return Err(format!("events pull returned HTTP {}", response.status));
    }
    let json: serde_json::Value =
        serde_json::from_str(&response.body).map_err(|e| format!("invalid events json: {e}"))?;
    let events = json["events"].as_array().cloned().unwrap_or_default();
    for event in events {
        let event_id = event["id"].as_str().ok_or("event without id")?.to_owned();
        let kind = event["kind"].as_str().unwrap_or_default().to_owned();
        // tombstone 判定：source Message 已撤回时视图 body=null 且
        // recalled_at 非 null（未撤回的附件-only Message 是 body=null +
        // recalled_at=null，只是没有可回声的正文）。
        let source_recalled = event["message"]["recalled_at"].is_string();
        let source_body = event["message"]["body"].as_str();
        let room_id = event["message"]["room_id"].as_str();
        // 只对「普通新消息 + 未撤回 + 有正文」发言：撤回 tombstone、
        // message.recalled 与附件-only 都只观察。
        let (Some(body), Some(room_id)) = (source_body, room_id) else {
            continue;
        };
        if kind != "message.created" || source_recalled {
            continue;
        }
        handle_event(&host, port, token, &event_id, room_id, body)?;
    }
    if let Some(next) = json["next_after"].as_str() {
        *cursor = Some(next.to_owned());
    }
    Ok(())
}

fn parse_url(core_url: &str) -> Result<(String, u16, String), String> {
    let rest = core_url
        .strip_prefix("http://")
        .ok_or_else(|| format!("unsupported core url {core_url:?} (only http://)"))?;
    let (host, path) = match rest.split_once('/') {
        Some((host, path)) => (host, format!("/{path}")),
        None => (rest, String::new()),
    };
    let (host, port) = match host.rsplit_once(':') {
        Some((host, port)) => (
            host.to_owned(),
            port.parse::<u16>()
                .map_err(|_| format!("bad port in {core_url:?}"))?,
        ),
        None => (host.to_owned(), 8080),
    };
    Ok((host, port, path))
}

fn main() -> ExitCode {
    let core_url =
        std::env::var("CELIRA_IM_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".to_owned());
    let token = match std::env::var("CELIRA_BACKEND_TOKEN") {
        Ok(token) if !token.trim().is_empty() => token,
        _ => {
            eprintln!("CELIRA_BACKEND_TOKEN is required");
            return ExitCode::FAILURE;
        }
    };
    let poll_ms: u64 = std::env::var("CELIRA_MOCK_POLL_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);

    eprintln!("mock backend started for {core_url} (pull + say)");
    let mut cursor: Option<String> = None;
    loop {
        match poll_and_reply(&core_url, &token, &mut cursor) {
            Ok(()) => {
                std::thread::sleep(Duration::from_millis(poll_ms));
            }
            Err(e) => {
                eprintln!("mock backend fatal: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
}
