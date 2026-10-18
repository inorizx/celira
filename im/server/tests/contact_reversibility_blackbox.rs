//! Contact Reversibility 真实进程黑盒（CORE_V1_CONTACT_REVERSIBILITY.md §6）：
//! 只通过 operator CLI 与公开 HTTP 验证拒绝 / 解除 / 拉黑的事实、幂等与
//! fail-closed（房间不可达、列表消失、被拉黑拦截与「目标不存在」同形）。
//! 所有行为证据来自公开协议，不直读 SQLite；每次测试使用 `data/` 下唯一
//! 临时目录与空闲 loopback 端口，结束时回收进程与文件（不使用 /tmp）。

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

/// 黑盒运行目录 guard（`data/contact-reversibility-blackbox-<uuid>`，git
/// 忽略）：声明顺序保证 server 先 drop 再删目录。
struct RunDir {
    dir: PathBuf,
    cleaned: bool,
}

impl RunDir {
    fn create() -> Self {
        let workdir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        std::fs::create_dir_all(&workdir).expect("create ignored data dir");
        let dir = workdir.join(format!("contact-reversibility-blackbox-{}", Uuid::new_v4()));
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

/// 两个 Human（owner + peer）与各自 token/id 的便捷载体。
struct Humans {
    a_id: String,
    a_token: String,
    b_id: String,
    b_token: String,
}

async fn bootstrap_humans(db_path: &Path) -> Humans {
    let a = cli_json(db_path, &["bootstrap-human", "--display-name", "Alice"]).await;
    let b = cli_json(db_path, &["create-human", "--display-name", "Bob"]).await;
    Humans {
        a_id: require_string(&a, "participant_id"),
        a_token: require_string(&a, "token"),
        b_id: require_string(&b, "participant_id"),
        b_token: require_string(&b, "token"),
    }
}

async fn request_contact(server: &Server, token: &str, target: &str, key: &str) -> Response {
    server
        .request(
            "POST",
            "/v1/contacts/requests",
            Some(token),
            Some(key),
            Some(&serde_json::json!({ "participant_id": target })),
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

async fn create_human_direct_room(server: &Server, token: &str, peer: &str, key: &str) -> Response {
    server
        .request(
            "POST",
            "/v1/human-direct-rooms",
            Some(token),
            Some(key),
            Some(&serde_json::json!({ "human_id": peer })),
        )
        .await
}

async fn send_message(server: &Server, token: &str, room: &str, key: &str, body: &str) -> Response {
    server
        .request(
            "POST",
            &format!("/v1/rooms/{room}/messages"),
            Some(token),
            Some(key),
            Some(&serde_json::json!({ "body": body })),
        )
        .await
}

async fn list_contacts(server: &Server, token: &str) -> Vec<Value> {
    let resp = server
        .request("GET", "/v1/contacts", Some(token), None, None)
        .await;
    assert_eq!(resp.status, 200, "list contacts: {}", resp.json());
    resp.json()["contacts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

// ============================================================
// Test 1: 拒绝 → 新 key 再申请 → 拉黑拦截 → 解除拉黑 → 再申请
// ============================================================

#[tokio::test(flavor = "multi_thread")]
async fn decline_block_unblock_lifecycle_and_idempotency() {
    let mut run = RunDir::create();
    let db_path = run.db_path();
    let media_root = run.media_path();
    let mut server = Server::start(&db_path, &media_root).await;

    let h = bootstrap_humans(&db_path).await;

    // A→B 申请 → 201 pending。
    let resp = request_contact(&server, &h.a_token, &h.b_id, "c-1").await;
    assert_eq!(resp.status, 201, "first request: {}", resp.json());
    assert_eq!(resp.json()["contact"]["status"], "pending");
    // 同 key 幂等重放 → 200 replayed=true，同一关系。
    let resp = request_contact(&server, &h.a_token, &h.b_id, "c-1").await;
    assert_eq!(resp.status, 200, "replay: {}", resp.json());
    assert_eq!(resp.json()["idempotency_replayed"], true);

    // B 拒绝 → 204。
    let resp = server
        .request(
            "POST",
            &format!("/v1/contacts/{}/decline", h.a_id),
            Some(&h.b_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204, "decline: {}", resp.json());

    // 拒绝后 A 的列表里不再有该关系（closed 段不列出）。
    let contacts = list_contacts(&server, &h.a_token).await;
    assert!(
        contacts.is_empty(),
        "declined relation must disappear from the list: {contacts:?}"
    );
    // 拒绝是幂等的吗？第二次拒绝 → 404（没有 active 段，不泄露历史）。
    let resp = server
        .request(
            "POST",
            &format!("/v1/contacts/{}/decline", h.a_id),
            Some(&h.b_token),
            None,
            None,
        )
        .await;
    assert_error(&resp, 404, "contact_not_found");
    // requester 不能拒绝（只有 addressee 能）。
    let resp = request_contact(&server, &h.a_token, &h.b_id, "c-2").await;
    assert_eq!(
        resp.status,
        201,
        "re-request after decline: {}",
        resp.json()
    );
    let resp = server
        .request(
            "POST",
            &format!("/v1/contacts/{}/decline", h.b_id),
            Some(&h.a_token),
            None,
            None,
        )
        .await;
    assert_error(&resp, 404, "contact_not_found");

    // A 拉黑 B → 204（同一事务内结束既有 pending 段并写黑名单段）。
    let resp = server
        .request(
            "POST",
            &format!("/v1/blocks/{}", h.b_id),
            Some(&h.a_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204, "block: {}", resp.json());
    // 拉黑幂等：重复 → 204。
    let resp = server
        .request(
            "POST",
            &format!("/v1/blocks/{}", h.b_id),
            Some(&h.a_token),
            None,
            None,
        )
        .await;
    assert_eq!(
        resp.status,
        204,
        "repeat block is idempotent: {}",
        resp.json()
    );

    // 拉黑列表只展示当前 Human 拉黑的对象（且不含内部 block_id）。
    let resp = server
        .request("GET", "/v1/blocks", Some(&h.a_token), None, None)
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    let blocks = resp.json()["blocks"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    assert_eq!(blocks[0]["peer"]["id"], h.b_id);
    assert!(
        blocks[0].get("block_id").is_none(),
        "internal id must not leak"
    );
    // B 的拉黑列表为空（方向独立，且不泄露自己被拉黑）。
    let resp = server
        .request("GET", "/v1/blocks", Some(&h.b_token), None, None)
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["blocks"].as_array().unwrap().len(), 0);

    // 双方任一方向 active 黑名单时，申请与「目标不存在」同形 404（不泄露）。
    let resp = request_contact(&server, &h.a_token, &h.b_id, "c-3").await;
    assert_error(&resp, 404, "participant_not_found");
    let ghost = Uuid::new_v4().to_string();
    let resp = request_contact(&server, &h.a_token, &ghost, "c-ghost").await;
    assert_error(&resp, 404, "participant_not_found");
    // B → A 反方向申请同样被拦。
    let resp = request_contact(&server, &h.b_token, &h.a_id, "c-4").await;
    assert_error(&resp, 404, "participant_not_found");

    // 自拉黑 → 400；拉黑 Agent 目标 / 未知目标 → 404。
    let resp = server
        .request(
            "POST",
            &format!("/v1/blocks/{}", h.a_id),
            Some(&h.a_token),
            None,
            None,
        )
        .await;
    assert_error(&resp, 400, "invalid_request");
    let resp = server
        .request(
            "POST",
            &format!("/v1/blocks/{ghost}"),
            Some(&h.a_token),
            None,
            None,
        )
        .await;
    // 批次三第三刀 第①步：拉黑不再只接受 Human ⇒ 目标不可用的 404 由
    // human_not_found 换成 participant_not_found（同第二刀口径）。
    assert_error(&resp, 404, "participant_not_found");

    // 解除拉黑 → 204（幂等）。
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/blocks/{}", h.b_id),
            Some(&h.a_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204, "unblock: {}", resp.json());
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/blocks/{}", h.b_id),
            Some(&h.a_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204, "repeat unblock: {}", resp.json());

    // 解除拉黑后可以重新申请（关系不自动恢复，需要新申请）。
    let resp = request_contact(&server, &h.a_token, &h.b_id, "c-5").await;
    assert_eq!(resp.status, 201, "request after unblock: {}", resp.json());
    assert_eq!(resp.json()["contact"]["status"], "pending");

    server.stop().await;
    run.cleanup();
}

// ============================================================
// Test 2: 解除 → 房间不可达 → 复合 → 新房可用、旧房不可达
// ============================================================

#[tokio::test(flavor = "multi_thread")]
async fn end_contact_seals_room_and_reconciled_pair_gets_a_fresh_room() {
    let mut run = RunDir::create();
    let db_path = run.db_path();
    let media_root = run.media_path();
    let mut server = Server::start(&db_path, &media_root).await;

    let h = bootstrap_humans(&db_path).await;

    // 建立 accepted 关系并建房 + 双方各发一条消息。
    let resp = request_contact(&server, &h.a_token, &h.b_id, "c-1").await;
    assert_eq!(resp.status, 201, "{}", resp.json());
    let resp = accept_contact(&server, &h.b_token, &h.a_id).await;
    assert_eq!(resp.status, 200, "accept: {}", resp.json());
    // 重复接受幂等。
    let resp = accept_contact(&server, &h.b_token, &h.a_id).await;
    assert_eq!(resp.status, 200, "re-accept: {}", resp.json());

    let resp = create_human_direct_room(&server, &h.a_token, &h.b_id, "r-1").await;
    assert_eq!(resp.status, 201, "create room: {}", resp.json());
    let room_1 = require_string(&resp.json()["room"], "id");
    // 同 key 重放 → 200 同一房间。
    let resp = create_human_direct_room(&server, &h.a_token, &h.b_id, "r-1").await;
    assert_eq!(resp.status, 200, "room replay: {}", resp.json());
    assert_eq!(resp.json()["room"]["id"], room_1);
    // 同一段关系不能建第二房（新 key）→ 409 human_direct_room_exists。
    let resp = create_human_direct_room(&server, &h.a_token, &h.b_id, "r-2").await;
    assert_error(&resp, 409, "human_direct_room_exists");

    let resp = send_message(&server, &h.a_token, &room_1, "m-1", "你好").await;
    assert_eq!(resp.status, 201, "send: {}", resp.json());

    // 双方都在各自的房间列表里看到该房。
    for token in [&h.a_token, &h.b_token] {
        let resp = server
            .request("GET", "/v1/rooms", Some(token), None, None)
            .await;
        assert_eq!(resp.status, 200, "{}", resp.json());
        let rooms = resp.json()["rooms"].as_array().cloned().unwrap_or_default();
        assert!(
            rooms.iter().any(|r| r["room"]["id"] == room_1),
            "room must be listed before ending"
        );
    }

    // A 单方面解除（无需对方同意）→ 204；重复解除幂等 204。
    for expect in ["first end", "repeat end"] {
        let resp = server
            .request(
                "DELETE",
                &format!("/v1/contacts/{}", h.b_id),
                Some(&h.a_token),
                None,
                None,
            )
            .await;
        assert_eq!(resp.status, 204, "{expect}: {}", resp.json());
    }
    // 解除后 A 的联系人列表为空（accepted 段已闭合，不列出）。
    assert!(list_contacts(&server, &h.a_token).await.is_empty());

    // 旧房对双方都不可读、不可发（房间从双方列表消失）。
    for token in [&h.a_token, &h.b_token] {
        let resp = server
            .request(
                "GET",
                &format!("/v1/rooms/{room_1}"),
                Some(token),
                None,
                None,
            )
            .await;
        assert_error(&resp, 404, "room_not_found");
        let resp = server
            .request(
                "GET",
                &format!("/v1/rooms/{room_1}/messages"),
                Some(token),
                None,
                None,
            )
            .await;
        assert_error(&resp, 404, "room_not_found");
        let resp = send_message(&server, token, &room_1, "m-x", "还能说话吗").await;
        assert_error(&resp, 404, "room_not_found");
    }
    for token in [&h.a_token, &h.b_token] {
        let resp = server
            .request("GET", "/v1/rooms", Some(token), None, None)
            .await;
        let rooms = resp.json()["rooms"].as_array().cloned().unwrap_or_default();
        assert!(
            !rooms.iter().any(|r| r["room"]["id"] == room_1),
            "sealed room must disappear from the list"
        );
    }

    // 未 accepted 不能建房：解除后直接建房 → 409 contact_not_accepted。
    let resp = create_human_direct_room(&server, &h.a_token, &h.b_id, "r-3").await;
    assert_error(&resp, 409, "contact_not_accepted");

    // 复合（新申请 → 接受），另建新房可用；旧房仍不可达。
    let resp = request_contact(&server, &h.a_token, &h.b_id, "c-2").await;
    assert_eq!(resp.status, 201, "re-request: {}", resp.json());
    let resp = accept_contact(&server, &h.b_token, &h.a_id).await;
    assert_eq!(resp.status, 200, "re-accept: {}", resp.json());
    let resp = create_human_direct_room(&server, &h.b_token, &h.a_id, "r-4").await;
    assert_eq!(
        resp.status,
        201,
        "new room after reconcile: {}",
        resp.json()
    );
    let room_2 = require_string(&resp.json()["room"], "id");
    assert_ne!(room_1, room_2, "a new period must get a new room");
    let resp = send_message(&server, &h.a_token, &room_2, "m-2", "新房").await;
    assert_eq!(resp.status, 201, "send in the new room: {}", resp.json());
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_1}"),
            Some(&h.a_token),
            None,
            None,
        )
        .await;
    assert_error(&resp, 404, "room_not_found");

    server.stop().await;
    run.cleanup();
}

// ============================================================
// Test 3: 拉黑自动结束既有关系并封存房间（拉黑 ⊇ 解除）
// ============================================================

#[tokio::test(flavor = "multi_thread")]
async fn block_ends_existing_relation_and_seals_its_room() {
    let mut run = RunDir::create();
    let db_path = run.db_path();
    let media_root = run.media_path();
    let mut server = Server::start(&db_path, &media_root).await;

    let h = bootstrap_humans(&db_path).await;

    let resp = request_contact(&server, &h.a_token, &h.b_id, "c-1").await;
    assert_eq!(resp.status, 201, "{}", resp.json());
    let resp = accept_contact(&server, &h.b_token, &h.a_id).await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    let resp = create_human_direct_room(&server, &h.a_token, &h.b_id, "r-1").await;
    assert_eq!(resp.status, 201, "{}", resp.json());
    let room = require_string(&resp.json()["room"], "id");

    // B 拉黑 A：既有 accepted 段被同事务结束、房间封存。
    let resp = server
        .request(
            "POST",
            &format!("/v1/blocks/{}", h.a_id),
            Some(&h.b_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204, "block: {}", resp.json());
    assert!(
        list_contacts(&server, &h.a_token).await.is_empty(),
        "blocking must end the existing relation"
    );
    for token in [&h.a_token, &h.b_token] {
        let resp = server
            .request("GET", &format!("/v1/rooms/{room}"), Some(token), None, None)
            .await;
        assert_error(&resp, 404, "room_not_found");
    }
    // 被拉黑方看不到任何黑名单语义：B→A 申请同样只得到 404（与目标不存在同形）。
    let resp = request_contact(&server, &h.a_token, &h.b_id, "c-2").await;
    assert_error(&resp, 404, "participant_not_found");
    let resp = request_contact(&server, &h.b_token, &h.a_id, "c-3").await;
    assert_error(&resp, 404, "participant_not_found");

    // 解除拉黑后仍须重新建立关系才能建房（关系不自动恢复）。
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/blocks/{}", h.a_id),
            Some(&h.b_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204, "{}", resp.json());
    let resp = create_human_direct_room(&server, &h.a_token, &h.b_id, "r-2").await;
    assert_error(&resp, 409, "contact_not_accepted");

    server.stop().await;
    run.cleanup();
}

// ============================================================
// Test 4: 重启持久性与未认证 fail-closed
// ============================================================

#[tokio::test(flavor = "multi_thread")]
async fn reversibility_state_survives_restart_and_requires_auth() {
    let mut run = RunDir::create();
    let db_path = run.db_path();
    let media_root = run.media_path();
    let mut server = Server::start(&db_path, &media_root).await;

    let h = bootstrap_humans(&db_path).await;

    let resp = request_contact(&server, &h.a_token, &h.b_id, "c-1").await;
    assert_eq!(resp.status, 201, "{}", resp.json());
    let resp = accept_contact(&server, &h.b_token, &h.a_id).await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    let resp = create_human_direct_room(&server, &h.a_token, &h.b_id, "r-1").await;
    assert_eq!(resp.status, 201, "{}", resp.json());
    let room = require_string(&resp.json()["room"], "id");
    // B 拒绝被拉黑的反向申请前先拉黑：A → B（A 拉黑 B）。
    let resp = server
        .request(
            "POST",
            &format!("/v1/blocks/{}", h.b_id),
            Some(&h.a_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204, "{}", resp.json());

    // 未认证：全部新端点 401。
    for (method, path) in [
        ("POST", format!("/v1/contacts/{}/decline", h.b_id)),
        ("DELETE", format!("/v1/contacts/{}", h.b_id)),
        ("GET", "/v1/blocks".to_string()),
        ("POST", format!("/v1/blocks/{}", h.b_id)),
        ("DELETE", format!("/v1/blocks/{}", h.b_id)),
    ] {
        let resp = server.request(method, &path, None, None, None).await;
        assert_error(&resp, 401, "authentication_required");
    }

    // 重启后黑名单段与封存房间保持。
    server.stop().await;
    let mut server = Server::start(&db_path, &media_root).await;
    let resp = server
        .request("GET", "/v1/blocks", Some(&h.a_token), None, None)
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    let blocks = resp.json()["blocks"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(blocks.len(), 1, "block must survive restart: {blocks:?}");
    assert_eq!(blocks[0]["peer"]["id"], h.b_id);
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room}"),
            Some(&h.a_token),
            None,
            None,
        )
        .await;
    assert_error(&resp, 404, "room_not_found");
    // 关系段已闭合：申请仍被黑名单拦截。
    let resp = request_contact(&server, &h.b_token, &h.a_id, "c-after").await;
    assert_error(&resp, 404, "participant_not_found");
    // 解除拉黑后关系不自动恢复。
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/blocks/{}", h.b_id),
            Some(&h.a_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204, "{}", resp.json());
    assert!(list_contacts(&server, &h.a_token).await.is_empty());

    server.stop().await;
    run.cleanup();
}
