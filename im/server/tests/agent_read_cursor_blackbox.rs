//! Agent 读游标 · 真实进程黑盒（CORE_V1_AGENT_READ_CURSOR.md §8）：只用
//! operator CLI 与公开 HTTP 验证切片 M1——Agent 经 BackendAuth 显式推进读
//! 游标（单调 / no-op / 非法目标 / 非成员 / Human token 拒绝），通讯录
//! `read_state` 与该端点一致，且位置**跨 Core 重启存活**。
//!
//! 成功裁决不读 SQLite；每次测试使用 `data/` 下唯一临时目录与空闲 loopback
//! 端口，结束时回收进程与文件（不使用 /tmp）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::time::timeout;
use uuid::Uuid;

/// cargo 注入的测试用二进制路径（保留连字符原名）。
const BIN: &str = env!("CARGO_BIN_EXE_celira-im");

struct Response {
    status: u16,
    body: Vec<u8>,
}

impl Response {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .field("json", &self.json())
            .finish()
    }
}

async fn raw_request(
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<Response, String> {
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
    let raw = String::from_utf8_lossy(&raw);
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| "malformed HTTP response".to_owned())?;
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| "missing HTTP status".to_owned())?;
    let headers: HashMap<_, _> = lines
        .filter_map(|line| line.split_once(": "))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.to_owned()))
        .collect();
    let body = match headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
    {
        Some(length) => body.as_bytes()[..length].to_vec(),
        None => body.as_bytes().to_vec(),
    };
    Ok(Response { status, body })
}

struct Server {
    child: Child,
    addr: String,
}

impl Server {
    async fn start(db_path: &Path, media_root: &Path) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve port");
        let addr = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        drop(listener);

        let child = Command::new(BIN)
            .env("CELIRA_LISTEN_ADDR", &addr)
            .env("CELIRA_DB_PATH", db_path)
            .env("CELIRA_MEDIA_ROOT", media_root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn core");
        let server = Self { child, addr };
        timeout(Duration::from_secs(30), async {
            loop {
                if matches!(
                    raw_request(&server.addr, "GET", "/ready", &[], None).await,
                    Ok(Response { status: 200, .. })
                ) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("core did not become ready");
        server
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        key: Option<&str>,
        body: Option<&Value>,
    ) -> Response {
        let mut headers: Vec<(String, String)> = Vec::new();
        if let Some(token) = token {
            headers.push(("Authorization".into(), format!("Bearer {token}")));
        }
        if let Some(key) = key {
            headers.push(("Idempotency-Key".into(), key.to_owned()));
        }
        if body.is_some() {
            headers.push(("Content-Type".into(), "application/json".into()));
        }
        let header_refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let body_bytes = body.map(|value| value.to_string().into_bytes());
        raw_request(
            &self.addr,
            method,
            path,
            &header_refs,
            body_bytes.as_deref(),
        )
        .await
        .unwrap_or_else(|e| panic!("request {method} {path} failed: {e}"))
    }

    async fn stop(&mut self) {
        let _ = self.child.kill().await;
        let _status = timeout(Duration::from_secs(10), self.child.wait())
            .await
            .expect("core did not stop")
            .expect("core wait failed");
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

async fn cli_json(db_path: &Path, args: &[&str]) -> Value {
    let output = Command::new(BIN)
        .args(args)
        .env("CELIRA_DB_PATH", db_path)
        .output()
        .await
        .expect("run operator CLI");
    assert!(
        output.status.success(),
        "operator CLI {args:?} must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("operator CLI stdout must be one JSON response")
}

fn require_string(json: &Value, field: &str) -> String {
    json[field]
        .as_str()
        .unwrap_or_else(|| panic!("missing string field {field} in {json}"))
        .to_owned()
}

/// 黑盒运行目录 guard（`data/agent-read-cursor-blackbox-<uuid>`，git 忽略）。
struct RunDir {
    dir: PathBuf,
    cleaned: bool,
}

impl RunDir {
    fn create() -> Self {
        let workdir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        std::fs::create_dir_all(&workdir).expect("create ignored data dir");
        let dir = workdir.join(format!("agent-read-cursor-blackbox-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create blackbox run dir");
        Self {
            dir,
            cleaned: false,
        }
    }

    fn db_path(&self) -> PathBuf {
        self.dir.join("core.db")
    }

    fn media_path(&self) -> PathBuf {
        self.dir.join("media")
    }

    fn cleanup(&mut self) {
        std::fs::remove_dir_all(&self.dir).expect("cleanup blackbox assets");
        self.cleaned = true;
    }
}

impl Drop for RunDir {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// 一个真实世界的最小装配：Human + Agent A + 一条 Direct Room（A 在内）
/// + 一条只有 Human 的 Group Room + 两条 Human 消息。全部经公开协议完成。
struct World {
    server: Server,
    human_token: String,
    agent_token: String,
    direct_room: String,
    outsider_room: String,
    messages: Vec<String>,
}

async fn build_world(db: &Path, media: &Path) -> World {
    let server = Server::start(db, media).await;
    let boot = cli_json(db, &["bootstrap-human", "--display-name", "主人"]).await;
    let human_token = require_string(&boot, "token");

    let created = server
        .request(
            "POST",
            "/v1/agents",
            Some(&human_token),
            Some("rc-ag-1"),
            Some(&serde_json::json!({"display_name": "Agent A"})),
        )
        .await;
    assert_eq!(created.status, 201, "create agent: {created:?}");
    let agent_id = require_string(&created.json()["agent"], "id");

    let created = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_id}/backend-bindings"),
            Some(&human_token),
            Some("rc-b-1"),
            Some(&serde_json::json!({"label": "read cursor blackbox"})),
        )
        .await;
    assert_eq!(created.status, 201, "create binding: {created:?}");
    let binding_id = require_string(&created.json()["binding"], "id");
    let issued = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
            Some(&human_token),
            Some("rc-c-1"),
            Some(&serde_json::json!({"replaces_credential_id": null})),
        )
        .await;
    assert_eq!(issued.status, 201, "issue credential: {issued:?}");
    let agent_token = require_string(&issued.json(), "token");

    let created = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&human_token),
            Some("rc-d-1"),
            Some(&serde_json::json!({"agent_id": agent_id.clone()})),
        )
        .await;
    assert_eq!(created.status, 201, "create direct room: {created:?}");
    let direct_room = require_string(&created.json()["room"], "id");

    let created = server
        .request(
            "POST",
            "/v1/groups",
            Some(&human_token),
            Some("rc-g-1"),
            Some(&serde_json::json!({"name": "只有人的群"})),
        )
        .await;
    assert_eq!(created.status, 201, "create group: {created:?}");
    let outsider_room = require_string(&created.json()["room"], "id");

    let mut messages = Vec::new();
    for (key, body) in [("rc-m-1", "你好"), ("rc-m-2", "在吗")] {
        let sent = server
            .request(
                "POST",
                &format!("/v1/rooms/{direct_room}/messages"),
                Some(&human_token),
                Some(key),
                Some(&serde_json::json!({"body": body})),
            )
            .await;
        assert_eq!(sent.status, 201, "send {body}: {sent:?}");
        messages.push(require_string(&sent.json()["message"], "id"));
    }

    World {
        server,
        human_token,
        agent_token,
        direct_room,
        outsider_room,
        messages,
    }
}

impl World {
    async fn put_cursor(&self, room: &str, message_id: &str) -> Response {
        self.server
            .request(
                "PUT",
                &format!("/v1/backend/rooms/{room}/read-cursor"),
                Some(&self.agent_token),
                None,
                Some(&serde_json::json!({"message_id": message_id})),
            )
            .await
    }

    /// 通讯录里该房的 read_state（走公开 Backend 读面）。
    async fn read_state(&self, room: &str) -> Value {
        let listed = self
            .server
            .request(
                "GET",
                "/v1/backend/rooms",
                Some(&self.agent_token),
                None,
                None,
            )
            .await;
        assert_eq!(listed.status, 200, "directory: {listed:?}");
        listed.json()["rooms"]
            .as_array()
            .and_then(|list| {
                list.iter()
                    .find(|r| r["room_id"].as_str() == Some(room))
            })
            .unwrap_or_else(|| panic!("room {room} must be in the directory: {listed:?}"))
            ["read_state"]
            .clone()
    }
}

#[tokio::test]
async fn agent_read_cursor_over_real_process() {
    let mut run = RunDir::create();
    let mut world = build_world(&run.db_path(), &run.media_path()).await;

    // 起点：位置为空、两条未读。
    let state = world.read_state(&world.direct_room).await;
    assert_eq!(state["last_read_message_id"], Value::Null);
    assert_eq!(state["unread_count"].as_i64(), Some(2));

    // 推进到第一条：200 advanced=true，位置与未读都动。
    let advanced = world
        .put_cursor(&world.direct_room, &world.messages[0])
        .await;
    assert_eq!(advanced.status, 200, "advance: {advanced:?}");
    assert_eq!(advanced.json()["advanced"].as_bool(), Some(true));
    assert_eq!(
        advanced.json()["read_state"]["unread_count"].as_i64(),
        Some(1)
    );
    let state = world.read_state(&world.direct_room).await;
    assert_eq!(
        state["last_read_message_id"].as_str(),
        Some(world.messages[0].as_str())
    );
    assert_eq!(state["unread_count"].as_i64(), Some(1));

    // 同值重放：成功 no-op（无 Idempotency-Key，单调推进天然幂等）。
    let replayed = world
        .put_cursor(&world.direct_room, &world.messages[0])
        .await;
    assert_eq!(replayed.status, 200, "no-op: {replayed:?}");
    assert_eq!(replayed.json()["advanced"].as_bool(), Some(false));

    // 他 Room 的真实 Message → 400 invalid_read_cursor。
    let outsider = world
        .server
        .request(
            "POST",
            &format!("/v1/rooms/{}/messages", world.outsider_room),
            Some(&world.human_token),
            Some("rc-out-1"),
            Some(&serde_json::json!({"body": "别人房间的消息"})),
        )
        .await;
    assert_eq!(outsider.status, 201, "outsider message: {outsider:?}");
    let outsider_message = require_string(&outsider.json()["message"], "id");
    let refused = world
        .put_cursor(&world.direct_room, &outsider_message)
        .await;
    assert_eq!(refused.status, 400, "other-room message: {refused:?}");
    assert_eq!(
        refused.json()["error"]["code"].as_str(),
        Some("invalid_read_cursor")
    );

    // 非成员房间 → 404 room_not_found。
    let refused = world
        .put_cursor(&world.outsider_room, &world.messages[0])
        .await;
    assert_eq!(refused.status, 404, "non-member room: {refused:?}");
    assert_eq!(
        refused.json()["error"]["code"].as_str(),
        Some("room_not_found")
    );

    // Human token 与未知 token 一律 401 invalid_credential。
    for token in [world.human_token.as_str(), "clr_bc_not-a-real-token"] {
        let refused = world
            .server
            .request(
                "PUT",
                &format!("/v1/backend/rooms/{}/read-cursor", world.direct_room),
                Some(token),
                None,
                Some(&serde_json::json!({"message_id": world.messages[0]})),
            )
            .await;
        assert_eq!(refused.status, 401, "token {token}: {refused:?}");
        assert_eq!(
            refused.json()["error"]["code"].as_str(),
            Some("invalid_credential")
        );
    }

    // Human 侧读游标端点语义不变（回归）：Human 也能推进同一房间。
    let human = world
        .server
        .request(
            "PUT",
            &format!("/v1/rooms/{}/read-cursor", world.direct_room),
            Some(&world.human_token),
            None,
            Some(&serde_json::json!({"message_id": world.messages[1]})),
        )
        .await;
    assert_eq!(human.status, 200, "human advance: {human:?}");
    assert_eq!(human.json()["advanced"].as_bool(), Some(true));

    // Agent 的位置不受 Human 推进影响（各自的位置各自记）。
    let state = world.read_state(&world.direct_room).await;
    assert_eq!(
        state["last_read_message_id"].as_str(),
        Some(world.messages[0].as_str())
    );

    world.server.stop().await;
    run.cleanup();
}

#[tokio::test]
async fn read_cursor_survives_core_restart() {
    let mut run = RunDir::create();
    let mut world = build_world(&run.db_path(), &run.media_path()).await;

    let advanced = world
        .put_cursor(&world.direct_room, &world.messages[1])
        .await;
    assert_eq!(advanced.status, 200, "advance: {advanced:?}");
    assert_eq!(
        advanced.json()["read_state"]["unread_count"].as_i64(),
        Some(0)
    );

    // 重启同一个库：位置与未读一字不变（位置是世界的事实，不是进程的）。
    world.server.stop().await;
    let mut restarted = Server::start(&run.db_path(), &run.media_path()).await;
    let listed = restarted
        .request(
            "GET",
            "/v1/backend/rooms",
            Some(&world.agent_token),
            None,
            None,
        )
        .await;
    assert_eq!(listed.status, 200, "directory after restart: {listed:?}");
    let state = listed.json()["rooms"]
        .as_array()
        .and_then(|list| {
            list.iter()
                .find(|r| r["room_id"].as_str() == Some(world.direct_room.as_str()))
        })
        .expect("direct room after restart")["read_state"]
        .clone();
    assert_eq!(
        state["last_read_message_id"].as_str(),
        Some(world.messages[1].as_str()),
        "position must survive a restart"
    );
    assert_eq!(state["unread_count"].as_i64(), Some(0));

    // 先等进程真的退出再删目录（Windows 上文件还被占着时删不掉）。
    restarted.stop().await;
    run.cleanup();
}
