//! Group Room Profile 真实进程黑盒（CORE_V1_ROOM_PROFILE.md §7）。
//!
//! 只通过 operator CLI 与公开 HTTP 验证：群主写 / 成员读、版本单调与幂等
//! 重放 / 冲突、权限矩阵（403/404/400/409/401）、owner 转让后新群主可写、
//! 单房历史回执的房间头与分页不变、direct 房 profile 恒 null、重启持久性。
//! 成功裁决不读取 SQLite；每次测试使用 `data/` 下唯一临时目录与空闲
//! loopback 端口，结束时回收进程与文件（不使用 /tmp）。

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

struct RunDir {
    dir: PathBuf,
    cleaned: bool,
}

impl RunDir {
    fn create() -> Self {
        let workdir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        std::fs::create_dir_all(&workdir).expect("create ignored data dir");
        let dir = workdir.join(format!("room-profile-blackbox-{}", Uuid::new_v4()));
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

async fn create_agent(server: &Server, token: &str, key: &str, name: &str) -> String {
    let resp = server
        .request(
            "POST",
            "/v1/agents",
            Some(token),
            Some(key),
            Some(&serde_json::json!({ "display_name": name })),
        )
        .await;
    assert_eq!(resp.status, 201, "create agent: {}", resp.json());
    require_string(&resp.json()["agent"], "id")
}

async fn backend_token(server: &Server, human_token: &str, agent: &str, tag: &str) -> String {
    let resp = server
        .request(
            "POST",
            &format!("/v1/agents/{agent}/backend-bindings"),
            Some(human_token),
            Some(&format!("bb-{tag}")),
            Some(&serde_json::json!({ "label": "rp" })),
        )
        .await;
    assert_eq!(resp.status, 201, "binding: {}", resp.json());
    let binding = require_string(&resp.json()["binding"], "id");
    let resp = server
        .request(
            "POST",
            &format!("/v1/agents/{agent}/backend-bindings/{binding}/credentials"),
            Some(human_token),
            Some(&format!("bc-{tag}")),
            Some(&serde_json::json!({ "replaces_credential_id": null })),
        )
        .await;
    assert_eq!(resp.status, 201, "credential: {}", resp.json());
    require_string(&resp.json(), "token")
}

async fn create_group(server: &Server, token: &str, key: &str, name: &str) -> String {
    let resp = server
        .request(
            "POST",
            "/v1/groups",
            Some(token),
            Some(key),
            Some(&serde_json::json!({ "name": name })),
        )
        .await;
    assert_eq!(resp.status, 201, "create group: {}", resp.json());
    require_string(&resp.json()["room"], "id")
}

async fn invite(
    server: &Server,
    token: &str,
    room: &str,
    participant: &str,
    key: &str,
) -> Response {
    server
        .request(
            "POST",
            &format!("/v1/rooms/{room}/members"),
            Some(token),
            Some(key),
            Some(&serde_json::json!({ "participant_id": participant })),
        )
        .await
}

async fn accept_contact(server: &Server, token: &str, peer: &str) -> Response {
    server
        .request(
            "POST",
            &format!("/v1/contacts/{peer}/accept"),
            Some(token),
            None,
            None,
        )
        .await
}

fn put_body(base: i64, description: &str) -> Value {
    serde_json::json!({ "base_version": base, "description": description })
}

async fn put_backend_profile(
    server: &Server,
    token: Option<&str>,
    room: &str,
    key: Option<&str>,
    body: Value,
) -> Response {
    server
        .request(
            "PUT",
            &format!("/v1/backend/rooms/{room}/profile"),
            token,
            key,
            Some(&body),
        )
        .await
}

async fn put_profile(
    server: &Server,
    token: &str,
    room: &str,
    key: Option<&str>,
    body: Value,
) -> Response {
    server
        .request(
            "PUT",
            &format!("/v1/rooms/{room}/profile"),
            Some(token),
            key,
            Some(&body),
        )
        .await
}

fn assert_error(resp: &Response, status: u16, code: &str) {
    assert_eq!(
        resp.status,
        status,
        "expected {status} + {code}: {}",
        resp.json()
    );
    assert_eq!(
        resp.json()["error"]["code"],
        code,
        "expected code {code}: {}",
        resp.json()
    );
}

fn parse_time(value: &Value) -> chrono_like::Rfc3339 {
    chrono_like::Rfc3339::parse(value.as_str().expect("updated_at string"))
}

/// 极小的 RFC3339 形状检查器（不为一个断言引入依赖）：YYYY-MM-DDTHH:MM:SS…。
mod chrono_like {
    pub struct Rfc3339;
    impl Rfc3339 {
        pub fn parse(raw: &str) -> Self {
            let bytes = raw.as_bytes();
            assert!(bytes.len() >= 20, "too short: {raw}");
            assert_eq!(bytes[4], b'-', "{raw}");
            assert_eq!(bytes[7], b'-', "{raw}");
            assert_eq!(bytes[10], b'T', "{raw}");
            assert_eq!(bytes[13], b':', "{raw}");
            assert_eq!(bytes[16], b':', "{raw}");
            assert!(
                raw.ends_with('Z') || raw.ends_with("+00:00") || raw.len() >= 25,
                "{raw}"
            );
            Self
        }
    }
}

// ============================================================
// 完整黑盒矩阵
// ============================================================

#[tokio::test(flavor = "multi_thread")]
async fn room_profile_write_read_matrix_and_restart() {
    let mut run = RunDir::create();
    let db_path = run.db_path();
    let media_root = run.media_path();
    let mut server = Server::start(&db_path, &media_root).await;

    let alice = cli_json(&db_path, &["bootstrap-human", "--display-name", "Alice"]).await;
    let alice_token = require_string(&alice, "token");
    let alice_id = require_string(&alice, "participant_id");
    let bob = cli_json(&db_path, &["create-human", "--display-name", "Bob"]).await;
    let bob_token = require_string(&bob, "token");
    let bob_id = require_string(&bob, "participant_id");
    let carol = cli_json(&db_path, &["create-human", "--display-name", "Carol"]).await;
    let carol_token = require_string(&carol, "token");

    let group = create_group(&server, &alice_token, "g-1", "研发组").await;
    // 关系是群邀请资格：Alice 申请 → Bob 自己同意。
    let resp = server
        .request(
            "POST",
            "/v1/contacts/requests",
            Some(&alice_token),
            Some("rel-1"),
            Some(&serde_json::json!({ "participant_id": bob_id })),
        )
        .await;
    assert_eq!(resp.status, 201, "contact request: {}", resp.json());
    let resp = accept_contact(&server, &bob_token, &alice_id).await;
    assert_eq!(resp.status, 200, "contact accept: {}", resp.json());
    let resp = invite(&server, &alice_token, &group, &bob_id, "inv-bob").await;
    assert_eq!(resp.status, 201, "invite bob: {}", resp.json());

    let agent = create_agent(&server, &alice_token, "a-1", "助手").await;
    let resp = invite(&server, &alice_token, &group, &agent, "inv-agent").await;
    assert_eq!(resp.status, 201, "invite agent: {}", resp.json());
    let agent_token = backend_token(&server, &alice_token, &agent, "main").await;

    // 无简介 → null。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group}"),
            Some(&bob_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert!(resp.json()["profile"].is_null());
    assert!(resp.json()["room"].get("profile").is_none());

    // 群主首次写 → 200 v1（不是 201：这一族统一 200）。
    let text = "第一行\n第二行\r\n\t缩进 🙂 café";
    let resp = put_profile(
        &server,
        &alice_token,
        &group,
        Some("rp-1"),
        put_body(0, text),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["version"], 1);
    assert_eq!(resp.json()["profile"]["description"], text);
    assert_eq!(resp.json()["idempotency_replayed"], false);
    let updated_at = parse_time(&resp.json()["profile"]["updated_at"]);
    let _ = updated_at;

    // 同 key 同摘要重放 → 200 + replayed=true + 同一版本。
    let resp = put_profile(
        &server,
        &alice_token,
        &group,
        Some("rp-1"),
        put_body(0, text),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["idempotency_replayed"], true);
    assert_eq!(resp.json()["profile"]["version"], 1);

    // 非 owner 成员 → 403；非成员 → 404（不泄露存在性）。
    let resp = put_profile(
        &server,
        &bob_token,
        &group,
        Some("rp-bob"),
        put_body(0, "Bob"),
    )
    .await;
    assert_error(&resp, 403, "not_room_owner");
    let resp = put_profile(
        &server,
        &carol_token,
        &group,
        Some("rp-carol"),
        put_body(0, "Carol"),
    )
    .await;
    assert_error(&resp, 404, "room_not_found");
    let resp = put_profile(
        &server,
        &alice_token,
        &Uuid::new_v4().to_string(),
        Some("rp-unknown"),
        put_body(0, "未知"),
    )
    .await;
    assert_error(&resp, 404, "room_not_found");

    // direct 房 → 400 invalid_room_kind。
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&alice_token),
            Some("d-1"),
            Some(&serde_json::json!({ "agent_id": agent })),
        )
        .await;
    assert_eq!(resp.status, 201, "direct room: {}", resp.json());
    let direct = require_string(&resp.json()["room"], "id");
    let resp = put_profile(
        &server,
        &alice_token,
        &direct,
        Some("rp-d"),
        put_body(0, "私聊"),
    )
    .await;
    assert_error(&resp, 400, "invalid_room_kind");

    // 过期 base / 同 key 不同正文 / 正文边界 / 缺 key。
    let resp = put_profile(
        &server,
        &alice_token,
        &group,
        Some("rp-stale"),
        put_body(0, "过期"),
    )
    .await;
    assert_error(&resp, 409, "profile_version_conflict");
    let resp = put_profile(
        &server,
        &alice_token,
        &group,
        Some("rp-1"),
        put_body(0, "改过"),
    )
    .await;
    assert_error(&resp, 409, "idempotency_conflict");
    for (index, description) in ["", " \n\t", "a\u{0}b"].iter().enumerate() {
        let resp = put_profile(
            &server,
            &alice_token,
            &group,
            Some(&format!("rp-bad-{index}")),
            put_body(1, description),
        )
        .await;
        assert_error(&resp, 400, "invalid_profile_description");
    }
    let long = "a".repeat(2_001);
    let resp = put_profile(
        &server,
        &alice_token,
        &group,
        Some("rp-long"),
        put_body(1, &long),
    )
    .await;
    assert_error(&resp, 400, "invalid_profile_description");
    let resp = put_profile(&server, &alice_token, &group, None, put_body(1, "无 key")).await;
    assert_error(&resp, 400, "idempotency_key_required");
    let resp = server
        .request(
            "PUT",
            &format!("/v1/rooms/{group}/profile"),
            None,
            Some("rp-anon"),
            Some(&put_body(1, "匿名")),
        )
        .await;
    assert_error(&resp, 401, "authentication_required");

    // 新 key + base 1 → v2；列表不带简介。
    let resp = put_profile(
        &server,
        &alice_token,
        &group,
        Some("rp-2"),
        put_body(1, "第二版"),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["version"], 2);
    let resp = server
        .request("GET", "/v1/rooms", Some(&alice_token), None, None)
        .await;
    assert_eq!(resp.status, 200);
    let item = resp.json()["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["room"]["id"] == serde_json::json!(group))
        .expect("group in list")
        .clone();
    assert!(item.get("profile").is_none());
    assert!(item["room"].get("profile").is_none());

    // 历史回执：房间头 + 分页不变。
    for i in 1..=5 {
        let resp = server
            .request(
                "POST",
                &format!("/v1/rooms/{group}/messages"),
                Some(&alice_token),
                Some(&format!("m-{i}")),
                Some(&serde_json::json!({ "body": format!("msg-{i}") })),
            )
            .await;
        assert_eq!(resp.status, 201, "{}", resp.json());
    }
    let resp = server
        .request(
            "GET",
            &format!("/v1/backend/rooms/{group}/messages?limit=2"),
            Some(&agent_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    let page1 = resp.json();
    assert_eq!(page1["room"]["id"], serde_json::json!(group));
    assert_eq!(page1["room"]["kind"], "group");
    assert_eq!(page1["room"]["name"], "研发组");
    assert_eq!(page1["room"]["profile"]["version"], 2);
    assert_eq!(page1["room"]["profile"]["description"], "第二版");
    assert!(page1["room"]["profile"].get("updated_at").is_none());
    assert_eq!(page1["messages"].as_array().unwrap().len(), 2);
    assert_eq!(page1["messages"][0]["body"], "msg-4");
    assert_eq!(page1["messages"][1]["body"], "msg-5");
    let next = require_string(&page1, "next_before");
    let resp = server
        .request(
            "GET",
            &format!("/v1/backend/rooms/{group}/messages?limit=2&before={next}"),
            Some(&agent_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    let page2 = resp.json();
    assert_eq!(page2["room"], page1["room"]);
    assert_eq!(page2["messages"][0]["body"], "msg-2");
    assert_eq!(page2["messages"][1]["body"], "msg-3");
    let next = require_string(&page2, "next_before");
    let resp = server
        .request(
            "GET",
            &format!("/v1/backend/rooms/{group}/messages?limit=2&before={next}"),
            Some(&agent_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    let page3 = resp.json();
    assert_eq!(page3["messages"].as_array().unwrap().len(), 1);
    assert_eq!(page3["messages"][0]["body"], "msg-1");
    assert!(page3["next_before"].is_null());
    let mut ids: Vec<&str> = Vec::new();
    for page in [&page1, &page2, &page3] {
        for message in page["messages"].as_array().unwrap() {
            ids.push(message["id"].as_str().unwrap());
        }
    }
    assert_eq!(ids.len(), 5);
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        5,
        "分页零重叠：{ids:?}"
    );

    // 人类历史没有 room 头。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group}/messages?limit=2"),
            Some(&alice_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert!(resp.json().get("room").is_none());

    // direct 历史 → profile null；非成员 agent → 403；未知房 → 404。
    let resp = server
        .request(
            "GET",
            &format!("/v1/backend/rooms/{direct}/messages"),
            Some(&agent_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["room"]["kind"], "direct");
    assert!(resp.json()["room"]["profile"].is_null());
    let outsider_agent = create_agent(&server, &alice_token, "a-2", "旁观").await;
    let outsider_token = backend_token(&server, &alice_token, &outsider_agent, "out").await;
    let resp = server
        .request(
            "GET",
            &format!("/v1/backend/rooms/{group}/messages"),
            Some(&outsider_token),
            None,
            None,
        )
        .await;
    assert_error(&resp, 403, "not_room_member");
    let resp = server
        .request(
            "GET",
            &format!("/v1/backend/rooms/{}/messages", Uuid::new_v4()),
            Some(&agent_token),
            None,
            None,
        )
        .await;
    assert_error(&resp, 404, "room_not_found");

    // 群主转让：旧群主 403、新群主可写（v3）。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group}/owner-transfer"),
            Some(&alice_token),
            Some("tr-1"),
            Some(&serde_json::json!({ "participant_id": bob_id })),
        )
        .await;
    assert_eq!(resp.status, 200, "transfer: {}", resp.json());
    let resp = put_profile(
        &server,
        &alice_token,
        &group,
        Some("rp-old"),
        put_body(2, "旧群主再写"),
    )
    .await;
    assert_error(&resp, 403, "not_room_owner");
    let resp = put_profile(
        &server,
        &bob_token,
        &group,
        Some("rp-new"),
        put_body(2, "新群主写"),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["version"], 3);
    assert_eq!(resp.json()["profile"]["description"], "新群主写");

    // ── 2026-10-26：Agent 群主经 BackendAuth 写自己的群简介 ──────────────
    // 一支 Agent 自己建房并成为 owner（owner 是房内治理角色，不限 kind）；
    // Alice 经「Agent 发起关系 → 人自己接受」成为成员。
    let owner_agent = create_agent(&server, &alice_token, "a-3", "群主助手").await;
    let owner_token = backend_token(&server, &alice_token, &owner_agent, "owner").await;
    let resp = server
        .request(
            "POST",
            "/v1/backend/groups",
            Some(&owner_token),
            Some("bg-1"),
            Some(&serde_json::json!({ "name": "Agent 的群" })),
        )
        .await;
    assert_eq!(resp.status, 201, "backend group: {}", resp.json());
    let agent_group = require_string(&resp.json()["room"], "id");

    let resp = server
        .request(
            "POST",
            "/v1/backend/contacts/requests",
            Some(&owner_token),
            Some("bg-rel"),
            Some(&serde_json::json!({ "participant_id": alice_id })),
        )
        .await;
    assert_eq!(resp.status, 201, "agent contact request: {}", resp.json());
    let resp = accept_contact(&server, &alice_token, &owner_agent).await;
    assert_eq!(resp.status, 200, "human accepts agent: {}", resp.json());
    let resp = server
        .request(
            "POST",
            &format!("/v1/backend/rooms/{agent_group}/members"),
            Some(&owner_token),
            Some("bg-inv"),
            Some(&serde_json::json!({ "participant_id": alice_id })),
        )
        .await;
    assert_eq!(resp.status, 201, "backend invite: {}", resp.json());

    // 缺凭据 → 401；direct 房 → 400（agent 是成员，但不是 group）。
    let resp = put_backend_profile(
        &server,
        None,
        &agent_group,
        Some("bg-anon"),
        put_body(0, "匿名"),
    )
    .await;
    assert_error(&resp, 401, "authentication_required");
    let resp = put_backend_profile(
        &server,
        Some(&agent_token),
        &direct,
        Some("bg-direct"),
        put_body(0, "私聊"),
    )
    .await;
    assert_error(&resp, 400, "invalid_room_kind");

    // Agent 群主首次写 → v1；Human 成员读得到。
    let agent_text = "Agent 群主写的简介\n第二行 🙂";
    let resp = put_backend_profile(
        &server,
        Some(&owner_token),
        &agent_group,
        Some("bg-p1"),
        put_body(0, agent_text),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["version"], 1);
    assert_eq!(resp.json()["profile"]["description"], agent_text);
    assert_eq!(resp.json()["idempotency_replayed"], false);
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{agent_group}"),
            Some(&alice_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["version"], 1);
    assert_eq!(resp.json()["profile"]["description"], agent_text);

    // Human 成员不能代写 Agent 的群。
    let resp = put_profile(
        &server,
        &alice_token,
        &agent_group,
        Some("bg-human"),
        put_body(1, "Human 成员代写"),
    )
    .await;
    assert_error(&resp, 403, "not_room_owner");

    // 幂等重放 / 过期 / 同 key 异正文。
    let resp = put_backend_profile(
        &server,
        Some(&owner_token),
        &agent_group,
        Some("bg-p1"),
        put_body(0, agent_text),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["idempotency_replayed"], true);
    assert_eq!(resp.json()["profile"]["version"], 1);
    let resp = put_backend_profile(
        &server,
        Some(&owner_token),
        &agent_group,
        Some("bg-stale"),
        put_body(0, "过期"),
    )
    .await;
    assert_error(&resp, 409, "profile_version_conflict");
    let resp = put_backend_profile(
        &server,
        Some(&owner_token),
        &agent_group,
        Some("bg-p1"),
        put_body(0, "改过"),
    )
    .await;
    assert_error(&resp, 409, "idempotency_conflict");

    // 历史回执的房间头看到新版本。
    let resp = server
        .request(
            "GET",
            &format!("/v1/backend/rooms/{agent_group}/messages"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["room"]["id"], serde_json::json!(agent_group));
    assert_eq!(resp.json()["room"]["profile"]["version"], 1);
    assert_eq!(resp.json()["room"]["profile"]["description"], agent_text);

    // Agent → Human 转让：新 Human 群主立刻可写（v2），Agent 立刻 403。
    let resp = server
        .request(
            "POST",
            &format!("/v1/backend/rooms/{agent_group}/owner-transfer"),
            Some(&owner_token),
            Some("bg-tr"),
            Some(&serde_json::json!({ "participant_id": alice_id })),
        )
        .await;
    assert_eq!(resp.status, 200, "transfer: {}", resp.json());
    let resp = put_profile(
        &server,
        &alice_token,
        &agent_group,
        Some("bg-human-owner"),
        put_body(1, "Human 新群主写"),
    )
    .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["version"], 2);
    assert_eq!(resp.json()["profile"]["description"], "Human 新群主写");
    let resp = put_backend_profile(
        &server,
        Some(&owner_token),
        &agent_group,
        Some("bg-old-owner"),
        put_body(2, "Agent 旧群主再写"),
    )
    .await;
    assert_error(&resp, 403, "not_room_owner");

    // 重启：简介与版本仍在，历史头仍在。
    server.stop().await;
    let mut server = Server::start(&db_path, &media_root).await;
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group}"),
            Some(&bob_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["version"], 3);
    assert_eq!(resp.json()["profile"]["description"], "新群主写");
    let resp = server
        .request(
            "GET",
            &format!("/v1/backend/rooms/{group}/messages?limit=1"),
            Some(&agent_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["room"]["profile"]["version"], 3);

    // Agent 起的群也跨重启：v2（Human 新群主写）仍在，房间头同步。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{agent_group}"),
            Some(&alice_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["version"], 2);
    assert_eq!(resp.json()["profile"]["description"], "Human 新群主写");
    let resp = server
        .request(
            "GET",
            &format!("/v1/backend/rooms/{agent_group}/messages"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["room"]["profile"]["version"], 2);

    server.stop().await;
    run.cleanup();
}
