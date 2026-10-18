//! 关系对主体开放 · 真实进程黑盒（批次三·第二刀，
//! `docs/contracts/core-v1/CORE_V1_CONTACT_REVERSIBILITY.md`）：
//!
//! 只用 operator CLI 与公开 HTTP，在**独立 Core 进程**上走完整闭环：
//! 1. Human 向一支 Agent 发起关系申请 → **Agent 自己拉到它** → **Agent 自己接受**
//!    （同意归被申请的那一方，Owner 不代答）；
//! 2. **Agent 作为群 owner 邀请这位 Human 进自己的群**——第一刀登记的
//!    "Agent owner 邀请不到任何人"当场消失的反证；
//! 3. Agent 主动向另一支 Agent 发起申请 → 对方接受 → 两支 Agent 建立关系
//!    （Agent–Agent 关系可达）；
//! 4. 一方单方面解除 → 关系段闭合、列表里不再出现、重复解除幂等 204；
//! 5. 认证面：Human token / 无凭据一律 401；
//! 6. 关系动作**不产生任何事件**（Human Event 与 Agent Event 都不多一条）。
//!
//! 成功裁决不读 SQLite；每次测试使用 `data/` 下唯一临时目录与空闲 loopback
//! 端口，结束时回收进程与文件。

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

/// 黑盒运行目录 guard（`data/backend-contact-blackbox-<uuid>`，git 忽略）。
struct RunDir {
    dir: PathBuf,
    cleaned: bool,
}

impl RunDir {
    fn create() -> Self {
        let workdir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        std::fs::create_dir_all(&workdir).expect("create ignored data dir");
        let dir = workdir.join(format!("backend-contact-blackbox-{}", Uuid::new_v4()));
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

/// 建一支 Agent 并签发它的 Backend Credential，返回 (agent_id, backend_token)。
async fn agent_with_credential(
    server: &Server,
    owner_token: &str,
    key: &str,
    name: &str,
) -> (String, String) {
    let created = server
        .request(
            "POST",
            "/v1/agents",
            Some(owner_token),
            Some(&format!("{key}-a")),
            Some(&serde_json::json!({"display_name": name})),
        )
        .await;
    assert_eq!(created.status, 201, "create agent {name}: {created:?}");
    let agent_id = require_string(&created.json()["agent"], "id");
    let binding = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_id}/backend-bindings"),
            Some(owner_token),
            Some(&format!("{key}-b")),
            Some(&serde_json::json!({"label": "contact blackbox"})),
        )
        .await;
    assert_eq!(binding.status, 201, "binding: {binding:?}");
    let binding_id = require_string(&binding.json()["binding"], "id");
    let credential = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
            Some(owner_token),
            Some(&format!("{key}-c")),
            Some(&serde_json::json!({"replaces_credential_id": null})),
        )
        .await;
    assert_eq!(credential.status, 201, "credential: {credential:?}");
    (agent_id, require_string(&credential.json(), "token"))
}

fn contact_with<'a>(json: &'a Value, peer_id: &str) -> Option<&'a Value> {
    json["contacts"]
        .as_array()?
        .iter()
        .find(|c| c["peer"]["id"] == serde_json::json!(peer_id))
}

#[tokio::test(flavor = "multi_thread")]
async fn relations_are_open_to_any_subject_on_a_real_core() {
    let mut run = RunDir::create();
    let db_path = run.db_path();
    let media_root = run.media_path();
    let mut server = Server::start(&db_path, &media_root).await;

    let owner = cli_json(&db_path, &["bootstrap-human", "--display-name", "主人"]).await;
    let owner_token = require_string(&owner, "token");
    let owner_id = require_string(&owner, "participant_id");
    // 第二位 Human 用 `create-human`（`bootstrap-human` 一个家只能做一次——它是
    // 家的奠基动作）。
    let other = cli_json(&db_path, &["create-human", "--display-name", "阿元"]).await;
    let other_token = require_string(&other, "token");

    let (agent_a, token_a) = agent_with_credential(&server, &owner_token, "ca", "小满").await;
    let (agent_b, token_b) = agent_with_credential(&server, &other_token, "cb", "小仓").await;

    // ── 认证面 ─────────────────────────────────────────────────────────────
    for (label, token) in [
        ("no header", None),
        ("human token", Some(owner_token.as_str())),
    ] {
        let resp = server
            .request("GET", "/v1/backend/contacts", token, None, None)
            .await;
        assert_eq!(resp.status, 401, "{label}: {resp:?}");
        let expected = if token.is_none() {
            "authentication_required"
        } else {
            "invalid_credential"
        };
        assert_eq!(resp.json()["error"]["code"], expected, "{label}");
    }

    // ── 1. Human → Agent 申请；Agent 自己看见、自己接受 ────────────────────
    let request = server
        .request(
            "POST",
            "/v1/contacts/requests",
            Some(&owner_token),
            Some("c-h2a"),
            Some(&serde_json::json!({"participant_id": agent_a})),
        )
        .await;
    assert_eq!(request.status, 201, "human requests agent: {request:?}");
    assert_eq!(request.json()["contact"]["peer"]["kind"], "agent");

    let listed = server
        .request("GET", "/v1/backend/contacts", Some(&token_a), None, None)
        .await;
    assert_eq!(listed.status, 200, "{listed:?}");
    let listed_json = listed.json();
    let pending = contact_with(&listed_json, &owner_id).expect("agent sees the request");
    assert_eq!(pending["status"], "pending", "{listed:?}");

    let accepted = server
        .request(
            "POST",
            &format!("/v1/backend/contacts/{owner_id}/accept"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "agent accepts on its own: {accepted:?}"
    );
    assert_eq!(accepted.json()["contact"]["status"], "accepted");

    // ── 2. Agent owner 邀请它的联系人进自己的群（第一刀那条边界的反证） ──────
    let group = server
        .request(
            "POST",
            "/v1/backend/groups",
            Some(&token_a),
            Some("c-g1"),
            Some(&serde_json::json!({"name": "小满与主人"})),
        )
        .await;
    assert_eq!(group.status, 201, "agent creates group: {group:?}");
    let group_id = require_string(&group.json()["room"], "id");
    let invite = server
        .request(
            "POST",
            &format!("/v1/backend/rooms/{group_id}/members"),
            Some(&token_a),
            Some("c-g1-i"),
            Some(&serde_json::json!({"participant_id": owner_id})),
        )
        .await;
    assert_eq!(
        invite.status, 201,
        "an Agent owner may invite the human it has a relation with: {invite:?}"
    );
    // 那位 Human 现在真的在那间房里（Human 侧读得到）。
    let human_reads = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(human_reads.status, 200, "{human_reads:?}");
    assert_eq!(
        human_reads.json()["room"]["id"],
        serde_json::json!(group_id)
    );

    // ── 3. Agent → Agent：另一支 Agent 主动申请，对方接受 ──────────────────
    let a2b = server
        .request(
            "POST",
            "/v1/backend/contacts/requests",
            Some(&token_a),
            Some("c-a2b"),
            Some(&serde_json::json!({"participant_id": agent_b})),
        )
        .await;
    assert_eq!(a2b.status, 201, "agent requests agent: {a2b:?}");
    let b_accepts = server
        .request(
            "POST",
            &format!("/v1/backend/contacts/{agent_a}/accept"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(b_accepts.status, 200, "{b_accepts:?}");
    let b_view = server
        .request("GET", "/v1/backend/contacts", Some(&token_b), None, None)
        .await;
    assert_eq!(
        contact_with(&b_view.json(), &agent_a).unwrap()["status"],
        "accepted",
        "{b_view:?}"
    );

    // 关系动作不产生事件：两支 Agent 的 event 流里没有新东西。
    let events = server
        .request(
            "GET",
            "/v1/backend/events?limit=50",
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(events.status, 200, "{events:?}");
    assert_eq!(
        events.json()["events"].as_array().map(|a| a.len()),
        Some(0),
        "relations must not generate agent events: {events:?}"
    );

    // ── 4. 单方面解除：闭合、不再列出、重复解除幂等 ────────────────────────
    let ended = server
        .request(
            "DELETE",
            &format!("/v1/backend/contacts/{agent_b}"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(ended.status, 204, "{ended:?}");
    let after = server
        .request("GET", "/v1/backend/contacts", Some(&token_a), None, None)
        .await;
    assert!(
        contact_with(&after.json(), &agent_b).is_none(),
        "the closed relation must not be listed: {after:?}"
    );
    let again = server
        .request(
            "DELETE",
            &format!("/v1/backend/contacts/{agent_b}"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(
        again.status, 204,
        "unfriending twice is an idempotent no-op"
    );

    // ── 5. 目标不可用一律与「不存在」同形 404 participant_not_found ────────
    let ghost = server
        .request(
            "POST",
            "/v1/backend/contacts/requests",
            Some(&token_a),
            Some("c-ghost"),
            Some(&serde_json::json!({"participant_id": Uuid::new_v4().to_string()})),
        )
        .await;
    assert_eq!(ghost.status, 404, "{ghost:?}");
    assert_eq!(ghost.json()["error"]["code"], "participant_not_found");

    server.stop().await;
    run.cleanup();
}
