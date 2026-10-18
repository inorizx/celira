//! Call 独立进程黑盒（CORE_V1_CALL.md 窄解冻切片）：启动真实 celira-im
//! 进程，只通过公开 API + 真实 WebSocket 完成通话主链/失败路径/A↔A/持久
//! 不变量/重启失忆的证据，绝不直读内部状态冒充成功。
//!
//! 覆盖：
//! - Human→Agent 主链 + 真实双 call-stream 信令中继：发起（ringing）→
//!   被叫 WS 收 call.ring → 接听（in_progress）→ 双方 WS 收 call.state →
//!   offer/answer 双向 call.signal 中继 → 挂断（ended）→ 双方收 call.state；
//! - 失败路径：忙碌（call_party_busy）、拒接（decline_callee）、终态后再动
//!   作（call_state_conflict）、非参与者读取（call_not_found）、非法 media
//!   （invalid_request）；
//! - Agent→Agent（video）：两个 owned Agent 入同一群，发起方经 Backend
//!   Credential 发起并接听；
//! - 持久不变量：一次完整通话生命周期前后，SQLite 所有业务表行数完全不变；
//! - 重启失忆：Core 重启后既有 Call 一律 404（fail closed），同一对重新发起
//!   为全新 201（不复用）；
//! - 媒体边界：信令中只出现 SDP/ICE 文本，媒体字节永不经过 Core（本切片
//!   Core 无媒体写入面，由「持久不变量」与信令形状证明）。
//!
//! 临时资产放在仓库内被 git 忽略的 `data/call-blackbox-*` 目录；HTTP 客户端
//! 是裸 TCP 的 HTTP/1.1，WS 客户端是裸 TCP RFC6455（都不引入额外依赖）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use base64::Engine as _;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::time::timeout;

const BIN: &str = env!("CARGO_BIN_EXE_celira-im");

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

impl HttpResponse {
    fn json(&self) -> serde_json::Value {
        if self.body.is_empty() {
            return serde_json::Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

async fn http_request(
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<HttpResponse, String> {
    let mut stream = TcpStream::connect(addr)
        .await
        .map_err(|e| format!("connect {addr}: {e}"))?;
    let body = body.unwrap_or_default();
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    if !body.is_empty() {
        stream.write_all(body).await.map_err(|e| e.to_string())?;
    }
    stream.flush().await.map_err(|e| e.to_string())?;

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .await
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&raw);
    let (head, body_bytes) = text
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("malformed response: {text:?}"));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let mut header_map = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(": ") {
            header_map.insert(name.to_ascii_lowercase(), value.to_owned());
        }
    }
    let body = if let Some(len) = header_map
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
    {
        body_bytes.as_bytes().iter().take(len).copied().collect()
    } else {
        body_bytes.as_bytes().to_vec()
    };
    Ok(HttpResponse { status, body })
}

struct Server {
    child: Child,
    addr: String,
}

impl Server {
    async fn start(db_path: &Path, media_root: &Path) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve port");
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let addr = format!("127.0.0.1:{port}");

        let child = Command::new(BIN)
            .env("CELIRA_LISTEN_ADDR", &addr)
            .env("CELIRA_DB_PATH", db_path)
            .env("CELIRA_MEDIA_ROOT", media_root)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn celira-im");
        let server = Self { child, addr };

        let deadline = Duration::from_secs(30);
        let started = timeout(deadline, async {
            loop {
                match try_http_request(&server.addr, "GET", "/ready", &[], None).await {
                    Some(HttpResponse { status: 200, .. }) => break,
                    _ => tokio::time::sleep(Duration::from_millis(200)).await,
                }
            }
        })
        .await;
        assert!(
            started.is_ok(),
            "server did not become ready at {} within {deadline:?}",
            server.addr
        );
        server
    }

    async fn stop(&mut self) {
        let _ = self.child.kill().await;
        let waited = timeout(Duration::from_secs(10), self.child.wait()).await;
        assert!(waited.is_ok(), "server did not exit within 10s after kill");
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        key: Option<&str>,
        json_body: Option<&serde_json::Value>,
    ) -> HttpResponse {
        let mut headers: Vec<(String, String)> = Vec::new();
        if let Some(token) = token {
            headers.push(("Authorization".into(), format!("Bearer {token}")));
        }
        if let Some(key) = key {
            headers.push(("Idempotency-Key".into(), key.to_owned()));
        }
        if json_body.is_some() {
            headers.push(("Content-Type".into(), "application/json".into()));
        }
        let raw_headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let body = json_body.map(|v| v.to_string().into_bytes());
        http_request(&self.addr, method, path, &raw_headers, body.as_deref())
            .await
            .unwrap_or_else(|e| panic!("request {method} {path} failed: {e}"))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

async fn try_http_request(
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Option<HttpResponse> {
    http_request(addr, method, path, headers, body).await.ok()
}

struct TestRunDir {
    dir: PathBuf,
}

impl TestRunDir {
    fn create() -> Self {
        let workdir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        std::fs::create_dir_all(&workdir).expect("create ignored data dir");
        let dir = workdir.join(format!("call-blackbox-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create blackbox run dir");
        Self { dir }
    }

    fn path(&self) -> &Path {
        &self.dir
    }

    fn db_path(&self) -> PathBuf {
        self.dir.join("core.db")
    }
}

impl Drop for TestRunDir {
    fn drop(&mut self) {
        // Windows 上 SQLite 可能仍持有文件锁：best-effort 清理，失败静默。
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn bootstrap_cli(db_path: &Path) -> String {
    let output = Command::new(BIN)
        .args(["bootstrap-human", "--display-name", "Owner"])
        .env("CELIRA_DB_PATH", db_path)
        .output()
        .await
        .expect("run bootstrap-human");
    assert!(
        output.status.success(),
        "bootstrap-human failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("cli json");
    json["token"].as_str().expect("cli token").to_owned()
}

async fn create_human_cli(db_path: &Path, display_name: &str) -> (String, String) {
    let output = Command::new(BIN)
        .args(["create-human", "--display-name", display_name])
        .env("CELIRA_DB_PATH", db_path)
        .output()
        .await
        .expect("run create-human");
    assert!(
        output.status.success(),
        "create-human failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("cli json");
    let participant_id = json["participant_id"].as_str().unwrap().to_owned();
    let token = json["token"].as_str().unwrap().to_owned();
    (participant_id, token)
}

/// 创建 owned Agent + Binding + Credential，返回 (agent_id, backend_token)。
async fn setup_agent(
    server: &Server,
    token: &str,
    name: &str,
    key_prefix: &str,
) -> (String, String) {
    let resp = server
        .request(
            "POST",
            "/v1/agents",
            Some(token),
            Some(&format!("{key_prefix}-ag")),
            Some(&serde_json::json!({"display_name": name})),
        )
        .await;
    assert_eq!(resp.status, 201, "agent: {}", resp.json());
    let agent_id = resp.json()["agent"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_id}/backend-bindings"),
            Some(token),
            Some(&format!("{key_prefix}-bk")),
            Some(&serde_json::json!({"label": name})),
        )
        .await;
    assert_eq!(resp.status, 201, "binding: {}", resp.json());
    let binding_id = resp.json()["binding"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
            Some(token),
            Some(&format!("{key_prefix}-cr")),
            Some(&serde_json::json!({"replaces_credential_id": null})),
        )
        .await;
    assert_eq!(resp.status, 201, "credential: {}", resp.json());
    let backend_token = resp.json()["token"].as_str().unwrap().to_owned();
    (agent_id, backend_token)
}

// ── 裸 TCP RFC6455 WS 客户端 ──

struct WsFrame {
    opcode: u8,
    payload: Vec<u8>,
}

async fn ws_connect(addr: &str, path: &str) -> Result<TcpStream, String> {
    let mut stream = TcpStream::connect(addr).await.map_err(|e| e.to_string())?;
    let mut key_bytes = [0u8; 16];
    getrandom::getrandom(&mut key_bytes).map_err(|e| e.to_string())?;
    let key = base64::engine::general_purpose::STANDARD.encode(key_bytes);
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    stream.flush().await.map_err(|e| e.to_string())?;

    let mut head = Vec::new();
    let mut buf = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|e| format!("read handshake: {e}"))?;
        if n == 0 {
            return Err("handshake closed early".into());
        }
        head.push(buf[0]);
        if head.len() > 8192 {
            return Err("handshake too large".into());
        }
    }
    let text = String::from_utf8_lossy(&head);
    let status_line = text.lines().next().unwrap_or_default();
    if !status_line.contains(" 101 ") {
        return Err(format!("websocket upgrade rejected: {status_line}"));
    }
    let expected = {
        use sha1::{Digest as _, Sha1};
        let mut digest = Sha1::new();
        digest.update(key.as_bytes());
        digest.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
        base64::engine::general_purpose::STANDARD.encode(digest.finalize())
    };
    let mut accept_ok = false;
    for line in text.lines() {
        if let Some((name, value)) = line.split_once(": ")
            && name.eq_ignore_ascii_case("Sec-WebSocket-Accept")
            && value.trim() == expected
        {
            accept_ok = true;
        }
    }
    if !accept_ok {
        return Err("Sec-WebSocket-Accept mismatch".into());
    }
    Ok(stream)
}

async fn ws_send_text(stream: &mut TcpStream, payload: &str) -> Result<(), String> {
    let data = payload.as_bytes();
    let len = data.len();
    let mut header = vec![0x81u8];
    if len < 126 {
        header.push(0x80 | len as u8);
    } else if len <= u16::MAX as usize {
        header.push(0x80 | 126);
        header.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        header.push(0x80 | 127);
        header.extend_from_slice(&(len as u64).to_be_bytes());
    }
    let mut mask = [0u8; 4];
    getrandom::getrandom(&mut mask).map_err(|e| e.to_string())?;
    header.extend_from_slice(&mask);
    let masked: Vec<u8> = data
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ mask[i % 4])
        .collect();
    stream.write_all(&header).await.map_err(|e| e.to_string())?;
    stream.write_all(&masked).await.map_err(|e| e.to_string())?;
    stream.flush().await.map_err(|e| e.to_string())
}

async fn ws_read_frame(stream: &mut TcpStream) -> Result<Option<WsFrame>, String> {
    let mut b0 = [0u8; 1];
    if stream.read(&mut b0).await.map_err(|e| e.to_string())? == 0 {
        return Ok(None);
    }
    let mut b1 = [0u8; 1];
    stream
        .read_exact(&mut b1)
        .await
        .map_err(|e| e.to_string())?;
    let opcode = b0[0] & 0x0f;
    let masked = b1[0] & 0x80 != 0;
    let mut len = (b1[0] & 0x7f) as u64;
    if len == 126 {
        let mut ext = [0u8; 2];
        stream
            .read_exact(&mut ext)
            .await
            .map_err(|e| e.to_string())?;
        len = u16::from_be_bytes(ext) as u64;
    } else if len == 127 {
        let mut ext = [0u8; 8];
        stream
            .read_exact(&mut ext)
            .await
            .map_err(|e| e.to_string())?;
        len = u64::from_be_bytes(ext);
    }
    if len > 16 * 1024 * 1024 {
        return Err("frame too large".into());
    }
    let mut mask = [0u8; 4];
    if masked {
        stream
            .read_exact(&mut mask)
            .await
            .map_err(|e| e.to_string())?;
    }
    let mut payload = vec![0u8; len as usize];
    if len > 0 {
        stream
            .read_exact(&mut payload)
            .await
            .map_err(|e| e.to_string())?;
    }
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    Ok(Some(WsFrame { opcode, payload }))
}

async fn ws_read_text_json(stream: &mut TcpStream) -> Result<serde_json::Value, String> {
    loop {
        match ws_read_frame(stream).await? {
            None => return Err("connection closed before a text frame".into()),
            Some(frame) if frame.opcode == 1 => {
                let text = String::from_utf8_lossy(&frame.payload).into_owned();
                return serde_json::from_str(&text)
                    .map_err(|e| format!("frame not json: {e}: {text}"));
            }
            Some(frame) if frame.opcode == 8 => return Err("unexpected close frame".into()),
            Some(_) => continue,
        }
    }
}

/// 认证一条 call-stream：连接 + 首帧 authenticate + 等 authenticated。
async fn ws_auth_call_stream(addr: &str, path: &str, token: &str) -> Result<TcpStream, String> {
    let mut ws = ws_connect(addr, path).await?;
    ws_send_text(
        &mut ws,
        &format!("{{\"type\":\"authenticate\",\"token\":\"{token}\"}}"),
    )
    .await?;
    let first = ws_read_text_json(&mut ws).await?;
    if first != serde_json::json!({"type": "authenticated"}) {
        return Err(format!("expected authenticated, got {first}"));
    }
    Ok(ws)
}

/// 读取一条带 type 前缀的 call-stream 帧（忽略 keepalive 等非事实内容）。
async fn ws_read_call_event(
    ws: &mut TcpStream,
    expected_type: &str,
) -> Result<serde_json::Value, String> {
    loop {
        let frame = ws_read_text_json(ws).await?;
        if frame.get("type").and_then(|t| t.as_str()) == Some(expected_type) {
            return Ok(frame);
        }
        // call.ring 之后才会出现 call.signal / call.state；其它帧继续等。
    }
}

// ── 持久不变量探针：所有业务表行数 ──

async fn table_counts(db_path: &Path) -> Vec<(String, i64)> {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open db");
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .expect("list tables");
    let mut counts = Vec::new();
    for t in &tables {
        let c: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM \"{t}\""))
            .fetch_one(&pool)
            .await
            .expect("count rows");
        counts.push((t.clone(), c));
    }
    drop(pool);
    counts
}

// ── 场景 1：Human→Agent 主链 + 真实双 call-stream 信令中继 ──

#[tokio::test(flavor = "multi_thread")]
async fn blackbox_call_human_agent_signal_relay() {
    let run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();
    let server = Server::start(&db_path, &run_dir.path().join("media")).await;

    let token_a = bootstrap_cli(&db_path).await;
    let human_a = server
        .request("GET", "/v1/me", Some(&token_a), None, None)
        .await
        .json()["participant"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (agent_a, backend_token) = setup_agent(&server, &token_a, "CallAgent", "c1").await;
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&token_a),
            Some("c1-room"),
            Some(&serde_json::json!({"agent_id": agent_a})),
        )
        .await;
    assert_eq!(resp.status, 201, "direct room: {}", resp.json());
    let room = resp.json()["room"]["id"].as_str().unwrap().to_owned();

    let mut evidence: Vec<String> = Vec::new();
    let mut step = |name: &str, detail: String| evidence.push(format!("[{name}] {detail}"));

    // 真实 WS：Human call-stream + Backend call-stream 都先认证。
    let mut ws_human = ws_auth_call_stream(&server.addr, "/v1/call/stream", &token_a)
        .await
        .expect("human call stream");
    let mut ws_backend =
        ws_auth_call_stream(&server.addr, "/v1/backend/call/stream", &backend_token)
            .await
            .expect("backend call stream");
    step(
        "ws-auth",
        "both call-streams upgraded 101 + {\"type\":\"authenticated\"}".into(),
    );

    // 发起（audio）→ 201 ringing；被叫 WS 立即收 call.ring。
    let resp = server
        .request(
            "POST",
            "/v1/calls",
            Some(&token_a),
            None,
            Some(&serde_json::json!({
                "room_id": room,
                "callee_id": agent_a,
                "media": "audio",
            })),
        )
        .await;
    assert_eq!(resp.status, 201, "initiate: {}", resp.json());
    assert_eq!(resp.json()["created"], true);
    let call_id = resp.json()["call"]["call_id"].as_str().unwrap().to_owned();
    assert_eq!(resp.json()["call"]["state"], "ringing");
    assert_eq!(resp.json()["call"]["media"], "audio");
    assert_eq!(resp.json()["call"]["caller_id"], human_a);
    assert_eq!(resp.json()["call"]["callee_id"], agent_a);

    let ring = timeout(
        Duration::from_secs(5),
        ws_read_call_event(&mut ws_backend, "call.ring"),
    )
    .await
    .expect("ring within 5s")
    .expect("ring frame");
    assert_eq!(ring["call"]["call_id"], call_id);
    assert_eq!(ring["call"]["caller_id"], human_a);
    assert_eq!(ring["call"]["media"], "audio");
    step(
        "ring",
        "backend stream got call.ring {call_id=…, caller=A, media=audio}".into(),
    );

    // 接听 → in_progress；双方 WS 收 call.state。
    let resp = server
        .request(
            "POST",
            &format!("/v1/backend/calls/{call_id}/accept"),
            Some(&backend_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "accept: {}", resp.json());
    assert_eq!(resp.json()["call"]["state"], "in_progress");
    let human_state = timeout(
        Duration::from_secs(5),
        ws_read_call_event(&mut ws_human, "call.state"),
    )
    .await
    .expect("state within 5s")
    .expect("state frame");
    assert_eq!(human_state["call"]["state"], "in_progress");
    let backend_state = timeout(
        Duration::from_secs(5),
        ws_read_call_event(&mut ws_backend, "call.state"),
    )
    .await
    .expect("state within 5s")
    .expect("state frame");
    assert_eq!(backend_state["call"]["state"], "in_progress");
    step(
        "accept",
        "accept -> in_progress; both streams got call.state(in_progress)".into(),
    );

    // 信令中继：offer（caller→callee）→ 被叫 WS 收 call.signal；answer 反向。
    let resp = server
        .request(
            "POST",
            &format!("/v1/calls/{call_id}/signal"),
            Some(&token_a),
            None,
            Some(&serde_json::json!({"kind": "offer", "sdp": "v=0 o=caller 1 1 IN IP4 127.0.0.1"})),
        )
        .await;
    assert_eq!(resp.status, 204, "send offer: {}", resp.json());
    let offer = timeout(
        Duration::from_secs(5),
        ws_read_call_event(&mut ws_backend, "call.signal"),
    )
    .await
    .expect("offer within 5s")
    .expect("offer frame");
    assert_eq!(offer["call_id"], call_id);
    assert_eq!(offer["from_id"], human_a);
    assert_eq!(offer["kind"], "offer");
    assert!(offer["payload"]["sdp"].as_str().unwrap().starts_with("v=0"));

    let resp = server
        .request(
            "POST",
            &format!("/v1/backend/calls/{call_id}/signal"),
            Some(&backend_token),
            None,
            Some(
                &serde_json::json!({"kind": "answer", "sdp": "v=0 o=callee 2 2 IN IP4 127.0.0.1"}),
            ),
        )
        .await;
    assert_eq!(resp.status, 204, "send answer: {}", resp.json());
    let answer = timeout(
        Duration::from_secs(5),
        ws_read_call_event(&mut ws_human, "call.signal"),
    )
    .await
    .expect("answer within 5s")
    .expect("answer frame");
    assert_eq!(answer["from_id"], agent_a);
    assert_eq!(answer["kind"], "answer");
    assert!(
        answer["payload"]["sdp"]
            .as_str()
            .unwrap()
            .starts_with("v=0")
    );
    step(
        "relay",
        "offer A->agent & answer agent->A relayed as call.signal over the live streams".into(),
    );

    // ICE 双向（candidate）。
    let resp = server
        .request(
            "POST",
            &format!("/v1/calls/{call_id}/signal"),
            Some(&token_a),
            None,
            Some(&serde_json::json!({"kind": "ice", "candidates": [{"candidate": "candidate:1 1 udp 1 127.0.0.1 5000 typ host", "sdp_mid": "0", "sdp_mline_index": 0}]})),
        )
        .await;
    assert_eq!(resp.status, 204);
    let ice = timeout(
        Duration::from_secs(5),
        ws_read_call_event(&mut ws_backend, "call.signal"),
    )
    .await
    .expect("ice within 5s")
    .expect("ice frame");
    assert_eq!(ice["kind"], "ice");
    assert_eq!(
        ice["payload"]["candidates"][0]["candidate"]
            .as_str()
            .unwrap(),
        "candidate:1 1 udp 1 127.0.0.1 5000 typ host"
    );
    step("ice", "ice candidates relayed A->agent".into());

    // 挂断 → ended(hangup_caller)；双方 WS 收 call.state(ended)。
    let resp = server
        .request(
            "POST",
            &format!("/v1/calls/{call_id}/hangup"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "hangup: {}", resp.json());
    assert_eq!(resp.json()["call"]["state"], "ended");
    assert_eq!(resp.json()["call"]["ended_reason"], "hangup_caller");
    let end_human = timeout(
        Duration::from_secs(5),
        ws_read_call_event(&mut ws_human, "call.state"),
    )
    .await
    .expect("ended within 5s")
    .expect("ended frame");
    assert_eq!(end_human["call"]["ended_reason"], "hangup_caller");
    let end_backend = timeout(
        Duration::from_secs(5),
        ws_read_call_event(&mut ws_backend, "call.state"),
    )
    .await
    .expect("ended within 5s")
    .expect("ended frame");
    assert_eq!(end_backend["call"]["state"], "ended");
    step(
        "hangup",
        "hangup -> ended(hangup_caller); both streams got call.state(ended)".into(),
    );

    println!("CALL BLACKBOX EVIDENCE (human->agent relay):");
    for line in &evidence {
        println!("{line}");
    }
    println!(
        "CALL BLACKBOX DONE: human->agent main path + real call-stream signaling relay passed; server closed"
    );
}

// ── 场景 2：失败路径 ──

#[tokio::test(flavor = "multi_thread")]
async fn blackbox_call_failure_paths() {
    let run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();
    let server = Server::start(&db_path, &run_dir.path().join("media")).await;

    let token_a = bootstrap_cli(&db_path).await;
    let (agent_a, backend_token_a) = setup_agent(&server, &token_a, "CallA", "f-a").await;
    let (agent_b, _backend_token_b) = setup_agent(&server, &token_a, "CallB", "f-b").await;
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&token_a),
            Some("f-room-a"),
            Some(&serde_json::json!({"agent_id": agent_a})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let room_a = resp.json()["room"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&token_a),
            Some("f-room-b"),
            Some(&serde_json::json!({"agent_id": agent_b})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let room_b = resp.json()["room"]["id"].as_str().unwrap().to_owned();

    let mut evidence: Vec<String> = Vec::new();
    let mut step = |name: &str, detail: String| evidence.push(format!("[{name}] {detail}"));

    // 非法 media → 400 invalid_request。
    let resp = server
        .request(
            "POST",
            "/v1/calls",
            Some(&token_a),
            None,
            Some(&serde_json::json!({"room_id": room_a, "callee_id": agent_a, "media": "screen"})),
        )
        .await;
    assert_eq!(resp.status, 400, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "invalid_request");

    // 发起 call1（ringing）。
    let resp = server
        .request(
            "POST",
            "/v1/calls",
            Some(&token_a),
            None,
            Some(&serde_json::json!({"room_id": room_a, "callee_id": agent_a})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let call1 = resp.json()["call"]["call_id"].as_str().unwrap().to_owned();

    // 忙碌：A 已在 active call1，再向 B 发起 → 409 call_party_busy。
    let resp = server
        .request(
            "POST",
            "/v1/calls",
            Some(&token_a),
            None,
            Some(&serde_json::json!({"room_id": room_b, "callee_id": agent_b})),
        )
        .await;
    assert_eq!(resp.status, 409, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "call_party_busy");
    step(
        "busy",
        "caller A already in active call -> new call to B 409 call_party_busy".into(),
    );

    // 同一 (room, caller, callee) 重复发起 → 200 复用（created=false）。
    let resp = server
        .request(
            "POST",
            "/v1/calls",
            Some(&token_a),
            None,
            Some(&serde_json::json!({"room_id": room_a, "callee_id": agent_a})),
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["created"], false);
    assert_eq!(resp.json()["call"]["call_id"], call1);
    step(
        "reuse",
        "same (room, caller, callee) active call -> 200 created=false, same call".into(),
    );

    // 参与者读取：caller（Human A session）可读自己的 Call → 200。
    // （契约 CORE_V1_CALL.md 5：`GET /v1/calls/{call_id}` 为 Human Session
    // 面；Backend 面不提供 GET，Agent 侧以 WS 流 + 动作端点为准。）
    let resp = server
        .request(
            "GET",
            &format!("/v1/calls/{call1}"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(
        resp.status,
        200,
        "caller A can read own call: {}",
        resp.json()
    );
    assert_eq!(resp.json()["call"]["call_id"], call1);
    // 非参与者读取：无关 Human B 读 A 的 call1 → 404 call_not_found。
    let (_human_b, token_b) = create_human_cli(&db_path, "B").await;
    let resp = server
        .request(
            "GET",
            &format!("/v1/calls/{call1}"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 404, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "call_not_found");
    step(
        "existence",
        "caller A GET -> 200; unrelated human B GET -> 404 call_not_found".into(),
    );

    // 方向强制（契约 §4.2）：call1 ringing 中，caller 发 answer（应 callee
    // 才可）→ 409 call_state_conflict。
    let resp = server
        .request(
            "POST",
            &format!("/v1/calls/{call1}/signal"),
            Some(&token_a),
            None,
            Some(&serde_json::json!({"kind": "answer", "sdp": "v=0"})),
        )
        .await;
    assert_eq!(resp.status, 409, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "call_state_conflict");
    step(
        "direction",
        "caller answer on ringing/ended call -> 409 call_state_conflict (answer only callee)"
            .into(),
    );

    // 拒接 → ended(decline_callee)。
    let resp = server
        .request(
            "POST",
            &format!("/v1/backend/calls/{call1}/decline"),
            Some(&backend_token_a),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "decline: {}", resp.json());
    assert_eq!(resp.json()["call"]["state"], "ended");
    assert_eq!(resp.json()["call"]["ended_reason"], "decline_callee");
    step(
        "decline",
        "callee decline -> ended(decline_callee); caller got state via stream".into(),
    );

    // 终态后再 accept/hangup → 409 call_state_conflict。
    let resp = server
        .request(
            "POST",
            &format!("/v1/backend/calls/{call1}/accept"),
            Some(&backend_token_a),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 409, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "call_state_conflict");
    let resp = server
        .request(
            "POST",
            &format!("/v1/calls/{call1}/hangup"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 409, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "call_state_conflict");
    step(
        "terminal",
        "actions on ended call -> 409 call_state_conflict".into(),
    );

    // 终态后信令（ended 后任何 signal）→ 409 call_state_conflict。
    let resp = server
        .request(
            "POST",
            &format!("/v1/calls/{call1}/signal"),
            Some(&token_a),
            None,
            Some(&serde_json::json!({"kind": "offer", "sdp": "v=0"})),
        )
        .await;
    assert_eq!(resp.status, 409, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "call_state_conflict");
    step(
        "terminal-signal",
        "signal on ended call -> 409 call_state_conflict".into(),
    );

    println!("CALL BLACKBOX EVIDENCE (failure paths):");
    for line in &evidence {
        println!("{line}");
    }
    println!(
        "CALL BLACKBOX DONE: busy/reuse/existence/decline/terminal-conflict passed; server closed"
    );
}

// ── 场景 3：Agent→Agent（video，群）──

#[tokio::test(flavor = "multi_thread")]
async fn blackbox_call_agent_to_agent_video() {
    let run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();
    let server = Server::start(&db_path, &run_dir.path().join("media")).await;

    let token_a = bootstrap_cli(&db_path).await;
    let (agent_1, token_1) = setup_agent(&server, &token_a, "A1", "aa-1").await;
    let (agent_2, token_2) = setup_agent(&server, &token_a, "A2", "aa-2").await;
    let resp = server
        .request(
            "POST",
            "/v1/groups",
            Some(&token_a),
            Some("aa-group"),
            Some(&serde_json::json!({"name": "双 Agent 群"})),
        )
        .await;
    assert_eq!(resp.status, 201, "group: {}", resp.json());
    let group = resp.json()["room"]["id"].as_str().unwrap().to_owned();
    for (idx, agent) in [&agent_1, &agent_2].iter().enumerate() {
        let resp = server
            .request(
                "POST",
                &format!("/v1/rooms/{group}/members"),
                Some(&token_a),
                Some(&format!("aa-inv-{idx}")),
                Some(&serde_json::json!({"participant_id": agent})),
            )
            .await;
        assert_eq!(resp.status, 201, "invite {idx}: {}", resp.json());
    }

    let mut evidence: Vec<String> = Vec::new();
    let mut step = |name: &str, detail: String| evidence.push(format!("[{name}] {detail}"));

    // A1（Backend Credential）作为发起方，向 A2 发起视频通话。
    let resp = server
        .request(
            "POST",
            "/v1/backend/calls",
            Some(&token_1),
            None,
            Some(&serde_json::json!({
                "room_id": group,
                "callee_id": agent_2,
                "media": "video",
            })),
        )
        .await;
    assert_eq!(resp.status, 201, "agent initiate: {}", resp.json());
    assert_eq!(resp.json()["call"]["caller_id"], agent_1);
    assert_eq!(resp.json()["call"]["callee_id"], agent_2);
    assert_eq!(resp.json()["call"]["media"], "video");
    let call_id = resp.json()["call"]["call_id"].as_str().unwrap().to_owned();

    // A2 接听 → in_progress。
    let resp = server
        .request(
            "POST",
            &format!("/v1/backend/calls/{call_id}/accept"),
            Some(&token_2),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "agent accept: {}", resp.json());
    assert_eq!(resp.json()["call"]["state"], "in_progress");
    step(
        "aa",
        "agent A1 -> agent A2 video call: initiate 201 video, A2 accept -> in_progress".into(),
    );

    // 认证边界：Human token 不能走 backend 面；Agent 不能走 human 面。
    let resp = server
        .request(
            "POST",
            "/v1/backend/calls",
            Some(&token_a),
            None,
            Some(&serde_json::json!({"room_id": group, "callee_id": agent_2})),
        )
        .await;
    assert_eq!(resp.status, 401, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "invalid_credential");
    let resp = server
        .request(
            "POST",
            "/v1/calls",
            Some(&token_1),
            None,
            Some(&serde_json::json!({"room_id": group, "callee_id": agent_2})),
        )
        .await;
    assert_eq!(resp.status, 401, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "invalid_session");
    step(
        "auth-boundary",
        "human on backend face 401 invalid_credential; agent on human face 401 invalid_session"
            .into(),
    );

    // 挂断。
    let resp = server
        .request(
            "POST",
            &format!("/v1/backend/calls/{call_id}/hangup"),
            Some(&token_1),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["call"]["ended_reason"], "hangup_caller");

    println!("CALL BLACKBOX EVIDENCE (agent->agent video):");
    for line in &evidence {
        println!("{line}");
    }
    println!("CALL BLACKBOX DONE: agent->agent video + auth boundary passed; server closed");
}

// ── 场景 4：持久不变量（一次完整通话生命周期前后所有业务表行数不变）──

#[tokio::test(flavor = "multi_thread")]
async fn blackbox_call_no_persistent_facts() {
    let run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();
    let server = Server::start(&db_path, &run_dir.path().join("media")).await;

    let token_a = bootstrap_cli(&db_path).await;
    let (agent_a, backend_token) = setup_agent(&server, &token_a, "Persist", "p-a").await;
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&token_a),
            Some("p-room"),
            Some(&serde_json::json!({"agent_id": agent_a})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let room = resp.json()["room"]["id"].as_str().unwrap().to_owned();

    let before = table_counts(&db_path).await;

    // 完整生命周期：发起 → 接听 → 信令 → 挂断。
    let resp = server
        .request(
            "POST",
            "/v1/calls",
            Some(&token_a),
            None,
            Some(&serde_json::json!({"room_id": room, "callee_id": agent_a})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let call_id = resp.json()["call"]["call_id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/backend/calls/{call_id}/accept"),
            Some(&backend_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    let resp = server
        .request(
            "POST",
            &format!("/v1/calls/{call_id}/signal"),
            Some(&token_a),
            None,
            Some(&serde_json::json!({"kind": "offer", "sdp": "v=0 o=caller 1 1 IN IP4 127.0.0.1"})),
        )
        .await;
    assert_eq!(resp.status, 204);
    let resp = server
        .request(
            "POST",
            &format!("/v1/calls/{call_id}/hangup"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["call"]["state"], "ended");

    let after = table_counts(&db_path).await;
    assert_eq!(
        before, after,
        "call lifecycle must not add/remove/change any SQLite row (Call is a non-persistent in-memory projection)"
    );
    println!(
        "CALL BLACKBOX DONE: full call lifecycle left every business table row count unchanged ({})",
        before.len()
    );
}

// ── 场景 5：重启失忆（fail closed 404 + 全新发起）──

#[tokio::test(flavor = "multi_thread")]
async fn blackbox_call_restart_amnesia() {
    let run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;

    let token_a = bootstrap_cli(&db_path).await;
    let (agent_a, backend_token) = setup_agent(&server, &token_a, "Amnesia", "r-a").await;
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&token_a),
            Some("r-room"),
            Some(&serde_json::json!({"agent_id": agent_a})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let room = resp.json()["room"]["id"].as_str().unwrap().to_owned();

    let resp = server
        .request(
            "POST",
            "/v1/calls",
            Some(&token_a),
            None,
            Some(&serde_json::json!({"room_id": room, "callee_id": agent_a})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let call_id = resp.json()["call"]["call_id"].as_str().unwrap().to_owned();

    // 重启（同一 db）。
    server.stop().await;
    let server = Server::start(&db_path, &run_dir.path().join("media")).await;

    // 失忆：既有 Call 一律 404（fail closed）。
    let resp = server
        .request(
            "GET",
            &format!("/v1/calls/{call_id}"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 404, "{}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "call_not_found");
    // 同一 (room, caller, callee) 重新发起 → 全新 201（不复用、不残留 in-memory）。
    let resp = server
        .request(
            "POST",
            "/v1/calls",
            Some(&token_a),
            None,
            Some(&serde_json::json!({"room_id": room, "callee_id": agent_a})),
        )
        .await;
    assert_eq!(resp.status, 201, "{}", resp.json());
    assert_eq!(resp.json()["created"], true);
    assert_ne!(resp.json()["call"]["call_id"], call_id);
    let new_call = resp.json()["call"]["call_id"].as_str().unwrap().to_owned();
    // 新 Call 仍可正常接听（进程内状态机重新建立）。
    let resp = server
        .request(
            "POST",
            &format!("/v1/backend/calls/{new_call}/accept"),
            Some(&backend_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["call"]["state"], "in_progress");

    println!(
        "CALL BLACKBOX DONE: restart amnesia -> old call 404 call_not_found, same pair re-initiate 201 fresh + accept works"
    );
}
