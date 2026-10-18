//! Agent 侧房治理 · 真实进程黑盒（批次三·第一刀「owner = 创建者」，
//! `docs/contracts/core-v1/CORE_V1_IM_CONTRACT.md` §5.1/§5.4/§5.6）：
//!
//! 只用 operator CLI 与公开 HTTP，在**独立 Core 进程**上验：
//! 1. 一支 Agent 经 BackendAuth **建群**，它自己是 owner（Agent–Agent 空间可达）；
//! 2. **没有 Human 在场的房**：那个群对任何 Human 都 404（不存在性不泄露、也不假装共享）；
//! 3. Human 侧的群主转让可以转给**房里的 Agent 成员**（owner 是房内的治理角色）；
//! 4. Agent owner 的治理动作：邀请（一条判据，今天取不到关系事实 ⇒ 稳定 404）、
//!    移除 Human 成员（204，被移除者此后 404）、转让、以及"退房先转让"（409）；
//! 5. 认证面：Human token 与无凭据一律 401。
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

/// 黑盒运行目录 guard（`data/backend-group-admin-blackbox-<uuid>`，git 忽略）。
struct RunDir {
    dir: PathBuf,
    cleaned: bool,
}

impl RunDir {
    fn create() -> Self {
        let workdir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        std::fs::create_dir_all(&workdir).expect("create ignored data dir");
        let dir = workdir.join(format!("backend-group-admin-blackbox-{}", Uuid::new_v4()));
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
            Some(&serde_json::json!({"label": "group admin blackbox"})),
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

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_can_own_and_govern_a_group_room() {
    let mut run = RunDir::create();
    let db_path = run.db_path();
    let media_root = run.media_path();
    let mut server = Server::start(&db_path, &media_root).await;

    let owner = cli_json(&db_path, &["bootstrap-human", "--display-name", "主人"]).await;
    let owner_token = require_string(&owner, "token");
    let owner_id = require_string(&owner, "participant_id");

    let (agent_a, token_a) = agent_with_credential(&server, &owner_token, "ga", "甲").await;
    let (agent_b, _token_b) = agent_with_credential(&server, &owner_token, "gb", "乙").await;

    // ── 认证面：无凭据 / Human token 一律 401 ───────────────────────────────
    for (label, token) in [
        ("no header", None),
        ("human token", Some(owner_token.as_str())),
    ] {
        let resp = server
            .request(
                "POST",
                "/v1/backend/groups",
                token,
                Some("ga-auth"),
                Some(&serde_json::json!({"name": "不该建成"})),
            )
            .await;
        assert_eq!(resp.status, 401, "{label}: {resp:?}");
        let expected = if token.is_none() {
            "authentication_required"
        } else {
            "invalid_credential"
        };
        assert_eq!(resp.json()["error"]["code"], expected, "{label}");
    }

    // ── 1. Agent 建群 ⇒ 它自己是 owner ──────────────────────────────────────
    let created = server
        .request(
            "POST",
            "/v1/backend/groups",
            Some(&token_a),
            Some("ga-g1"),
            Some(&serde_json::json!({"name": "甲乙的房间"})),
        )
        .await;
    assert_eq!(created.status, 201, "agent creates a group: {created:?}");
    let agent_room = require_string(&created.json()["room"], "id");
    assert_eq!(created.json()["room"]["kind"], "group");
    let memberships = created.json()["memberships"].as_array().unwrap().clone();
    assert_eq!(memberships.len(), 1, "founding owner only: {created:?}");
    assert_eq!(memberships[0]["participant_id"], serde_json::json!(agent_a));
    assert_eq!(memberships[0]["role"], "owner");

    // 同 key 同群名 ⇒ 200 重放；异群名 ⇒ 409。
    let replay = server
        .request(
            "POST",
            "/v1/backend/groups",
            Some(&token_a),
            Some("ga-g1"),
            Some(&serde_json::json!({"name": "甲乙的房间"})),
        )
        .await;
    assert_eq!(replay.status, 200, "replay: {replay:?}");
    assert_eq!(replay.json()["idempotency_replayed"], true);
    assert_eq!(replay.json()["room"]["id"], serde_json::json!(agent_room));
    let conflict = server
        .request(
            "POST",
            "/v1/backend/groups",
            Some(&token_a),
            Some("ga-g1"),
            Some(&serde_json::json!({"name": "换个名字"})),
        )
        .await;
    assert_eq!(conflict.status, 409, "{conflict:?}");
    assert_eq!(conflict.json()["error"]["code"], "idempotency_conflict");

    // ── 2. 没有 Human 在场的房：Human 读不到它（也不假装共享）───────────────
    let by_human = server
        .request(
            "GET",
            &format!("/v1/rooms/{agent_room}"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(
        by_human.status, 404,
        "a room with no human member must not be readable by a human: {by_human:?}"
    );
    assert_eq!(by_human.json()["error"]["code"], "room_not_found");
    // Agent 自己的通讯录里有它（Agent 读面）。
    let directory = server
        .request("GET", "/v1/backend/rooms", Some(&token_a), None, None)
        .await;
    assert_eq!(directory.status, 200, "{directory:?}");
    let directory_json = directory.json();
    let hit = directory_json["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["room_id"] == serde_json::json!(agent_room))
        .expect("the agent's own group must be in its directory");
    assert_eq!(hit["kind"], "group");

    // ── 3. Human 建群 → 拉 Agent A → 把 owner 转让给 A（Human 侧端点）────────
    let human_group = server
        .request(
            "POST",
            "/v1/groups",
            Some(&owner_token),
            Some("ga-hg"),
            Some(&serde_json::json!({"name": "主人建的群"})),
        )
        .await;
    assert_eq!(human_group.status, 201, "{human_group:?}");
    let human_room = require_string(&human_group.json()["room"], "id");
    let invite = server
        .request(
            "POST",
            &format!("/v1/rooms/{human_room}/members"),
            Some(&owner_token),
            Some("ga-hg-i"),
            Some(&serde_json::json!({"participant_id": agent_a})),
        )
        .await;
    assert_eq!(invite.status, 201, "invite the agent: {invite:?}");
    let transfer = server
        .request(
            "POST",
            &format!("/v1/rooms/{human_room}/owner-transfer"),
            Some(&owner_token),
            Some("ga-hg-t"),
            Some(&serde_json::json!({"participant_id": agent_a})),
        )
        .await;
    assert_eq!(
        transfer.status, 200,
        "an Agent member is a valid transfer target: {transfer:?}"
    );
    assert_eq!(transfer.json()["membership"]["role"], "owner");
    assert_eq!(
        transfer.json()["membership"]["participant_id"],
        serde_json::json!(agent_a)
    );

    // ── 4. Agent owner 的治理动作 ───────────────────────────────────────────
    // (a) 邀请：一条判据——今天它两条关系都没有 ⇒ 稳定 404（不新增错误码）。
    let invite_out = server
        .request(
            "POST",
            &format!("/v1/backend/rooms/{human_room}/members"),
            Some(&token_a),
            Some("ga-inv-b"),
            Some(&serde_json::json!({"participant_id": agent_b})),
        )
        .await;
    assert_eq!(invite_out.status, 404, "{invite_out:?}");
    assert_eq!(invite_out.json()["error"]["code"], "member_not_found");

    // (b) owner 自己退房 ⇒ 409（M2 既有规则，不分 kind）。
    let leave_as_owner = server
        .request(
            "POST",
            &format!("/v1/backend/rooms/{human_room}/leave"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(leave_as_owner.status, 409, "{leave_as_owner:?}");
    assert_eq!(
        leave_as_owner.json()["error"]["code"],
        "owner_transfer_required"
    );

    // (c) 转让回那位 Human（Agent actor 经 BackendAuth 发起）⇒ 200。
    let transfer_back = server
        .request(
            "POST",
            &format!("/v1/backend/rooms/{human_room}/owner-transfer"),
            Some(&token_a),
            Some("ga-hg-t2"),
            Some(&serde_json::json!({"participant_id": owner_id})),
        )
        .await;
    assert_eq!(
        transfer_back.status, 200,
        "the agent owner can transfer ownership away: {transfer_back:?}"
    );
    assert_eq!(
        transfer_back.json()["membership"]["participant_id"],
        serde_json::json!(owner_id)
    );

    // (d) 转让之后它不再是 owner ⇒ 可以自己退房（204）——不会被困在房里。
    let leave = server
        .request(
            "POST",
            &format!("/v1/backend/rooms/{human_room}/leave"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(leave.status, 204, "the agent leaves: {leave:?}");

    // (e) 非成员此后对这个房的一切读写 404（窗口关闭）。
    let after_leave = server
        .request("GET", "/v1/backend/rooms", Some(&token_a), None, None)
        .await;
    assert_eq!(after_leave.status, 200, "{after_leave:?}");
    assert!(
        !after_leave.json()["rooms"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["room_id"] == serde_json::json!(human_room)),
        "the room it left must vanish from its directory: {after_leave:?}"
    );

    // ── 5. 移除 Human 成员（Agent 当 owner 时）：另开一局 ────────────────────
    let human_group2 = server
        .request(
            "POST",
            "/v1/groups",
            Some(&owner_token),
            Some("ga-hg2"),
            Some(&serde_json::json!({"name": "主人的第二个群"})),
        )
        .await;
    assert_eq!(human_group2.status, 201, "{human_group2:?}");
    let human_room2 = require_string(&human_group2.json()["room"], "id");
    let invite2 = server
        .request(
            "POST",
            &format!("/v1/rooms/{human_room2}/members"),
            Some(&owner_token),
            Some("ga-hg2-i"),
            Some(&serde_json::json!({"participant_id": agent_a})),
        )
        .await;
    assert_eq!(invite2.status, 201, "{invite2:?}");
    let transfer2 = server
        .request(
            "POST",
            &format!("/v1/rooms/{human_room2}/owner-transfer"),
            Some(&owner_token),
            Some("ga-hg2-t"),
            Some(&serde_json::json!({"participant_id": agent_a})),
        )
        .await;
    assert_eq!(transfer2.status, 200, "{transfer2:?}");
    let remove_human = server
        .request(
            "DELETE",
            &format!("/v1/backend/rooms/{human_room2}/members/{owner_id}"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(
        remove_human.status, 204,
        "an Agent owner may remove a Human member: {remove_human:?}"
    );
    let removed_human = server
        .request(
            "GET",
            &format!("/v1/rooms/{human_room2}"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(removed_human.status, 404, "{removed_human:?}");
    assert_eq!(removed_human.json()["error"]["code"], "room_not_found");

    // owner 不能移除自己（既有判据，逐条不变）。
    let remove_self = server
        .request(
            "DELETE",
            &format!("/v1/backend/rooms/{human_room2}/members/{agent_a}"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(remove_self.status, 409, "{remove_self:?}");
    assert_eq!(remove_self.json()["error"]["code"], "owner_not_removable");

    server.stop().await;
    run.cleanup();
}
