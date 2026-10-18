//! Participant Presentation 真实进程黑盒（CORE_V1_PARTICIPANT_PRESENTATION.md
//! §8）：只通过 operator CLI 与公开 HTTP 验证 Agent Profile 与 Group Room
//! Presentation 的事实、写权限、幂等/base_version、只读投影、无 IM 副作用与
//! 重启持久性。成功裁决不读取 SQLite；每次测试使用 `data/` 下唯一临时目录与
//! 空闲 loopback 端口，结束时回收进程与文件（不使用 /tmp）。

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

    /// 任意原始 body / Content-Type（invalid JSON、NUL、超长字节预算等）。
    async fn request_raw(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        key: Option<&str>,
        content_type: &str,
        body: &[u8],
    ) -> Response {
        let mut headers: Vec<(String, String)> = Vec::new();
        if let Some(token) = token {
            headers.push(("Authorization".into(), format!("Bearer {token}")));
        }
        if let Some(key) = key {
            headers.push(("Idempotency-Key".into(), key.to_owned()));
        }
        headers.push(("Content-Type".into(), content_type.to_owned()));
        let header_refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        raw_request(&self.addr, method, path, &header_refs, Some(body))
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

/// 黑盒运行目录 guard（`data/participant-presentation-blackbox-<uuid>`，git
/// 忽略）：声明顺序保证 server 先 drop 再删目录。
struct RunDir {
    dir: PathBuf,
    cleaned: bool,
}

impl RunDir {
    fn create() -> Self {
        let workdir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        std::fs::create_dir_all(&workdir).expect("create ignored data dir");
        let dir = workdir.join(format!(
            "participant-presentation-blackbox-{}",
            Uuid::new_v4()
        ));
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
    resp.json()["agent"]["id"].as_str().unwrap().to_owned()
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
    resp.json()["room"]["id"].as_str().unwrap().to_owned()
}

async fn invite_member(
    server: &Server,
    token: &str,
    room_id: &str,
    participant_id: &str,
    key: &str,
) {
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/members"),
            Some(token),
            Some(key),
            Some(&serde_json::json!({ "participant_id": participant_id })),
        )
        .await;
    assert_eq!(resp.status, 201, "invite member: {}", resp.json());
}

fn put_profile_body(base_version: i64, short: Option<&str>, public: Option<&str>) -> Value {
    serde_json::json!({
        "base_version": base_version,
        "short_description": short,
        "public_description": public,
    })
}

fn put_presentation_body(
    base_version: i64,
    display: Option<&str>,
    role: Option<&str>,
    desc: Option<&str>,
) -> Value {
    serde_json::json!({
        "base_version": base_version,
        "room_display_name": display,
        "room_role_name": role,
        "room_role_description": desc,
    })
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

/// 断言 /members 形状（与既有契约一致）：membership 视图固定 6 键、不含任何
/// presentation 字段。
fn assert_members_shape(members: &Value) {
    let list = members["members"].as_array().expect("members array");
    for item in list {
        let membership = item["membership"].as_object().expect("membership object");
        assert_eq!(membership.len(), 6, "membership view must keep its shape");
        assert!(item.get("short_description").is_none());
        assert!(item.get("presentation").is_none());
    }
}

// ============================================================
// Test 1: Agent Profile PUT 生命周期（create/update/replay/conflict/stale）
// 与 fail-closed（非 owner、bad input、revoked）
// ============================================================

#[tokio::test(flavor = "multi_thread")]
async fn profile_put_lifecycle_and_fail_closed() {
    let mut run = RunDir::create();
    let db_path = run.db_path();
    let media_root = run.media_path();
    let mut server = Server::start(&db_path, &media_root).await;

    let owner = cli_json(&db_path, &["bootstrap-human", "--display-name", "Owner"]).await;
    let owner_token = require_string(&owner, "token");
    let outsider = cli_json(&db_path, &["create-human", "--display-name", "Outsider"]).await;
    let outsider_token = require_string(&outsider, "token");

    let agent_id = create_agent(&server, &owner_token, "a-1", "助手").await;
    let profile_path = format!("/v1/agents/{agent_id}/profile");

    // 无版本 → profile: null。
    let resp = server
        .request("GET", &profile_path, Some(&owner_token), None, None)
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert!(resp.json()["profile"].is_null());

    // 首次 PUT → 201（版本 0 → 1），result 是 create。
    let resp = server
        .request(
            "PUT",
            &profile_path,
            Some(&owner_token),
            Some("p-1"),
            Some(&put_profile_body(0, Some("短简介"), Some("公开介绍"))),
        )
        .await;
    assert_eq!(resp.status, 201, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["profile_version"], 1);
    assert_eq!(resp.json()["profile"]["short_description"], "短简介");
    assert_eq!(resp.json()["profile"]["public_description"], "公开介绍");
    assert_eq!(resp.json()["profile"]["published_by_kind"], "human");
    assert_eq!(resp.json()["idempotency_replayed"], false);

    // 更新（新 key、base_version=1）→ 200 版本 2。
    let resp = server
        .request(
            "PUT",
            &profile_path,
            Some(&owner_token),
            Some("p-2"),
            Some(&put_profile_body(1, Some("新简介"), None)),
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["profile_version"], 2);
    assert!(resp.json()["profile"]["public_description"].is_null());
    assert_eq!(resp.json()["idempotency_replayed"], false);

    // 同 key 同摘要重放首次：即使 head 已到 2，也返回首次成功结果（版本 1）。
    let resp = server
        .request(
            "PUT",
            &profile_path,
            Some(&owner_token),
            Some("p-1"),
            Some(&put_profile_body(0, Some("短简介"), Some("公开介绍"))),
        )
        .await;
    assert_eq!(resp.status, 201, "replay: {}", resp.json());
    assert_eq!(resp.json()["profile"]["profile_version"], 1);
    assert_eq!(resp.json()["idempotency_replayed"], true);

    // 同 key 不同请求摘要 → 409 idempotency_conflict。
    let resp = server
        .request(
            "PUT",
            &profile_path,
            Some(&owner_token),
            Some("p-1"),
            Some(&put_profile_body(0, Some("改了"), Some("公开介绍"))),
        )
        .await;
    assert_error(&resp, 409, "idempotency_conflict");

    // 新 key + 过期 base_version → 409 profile_stale。
    let resp = server
        .request(
            "PUT",
            &profile_path,
            Some(&owner_token),
            Some("p-3"),
            Some(&put_profile_body(0, Some("stale"), None)),
        )
        .await;
    assert_error(&resp, 409, "profile_stale");

    // 非 owner（Outsider）读/写一律 404 agent_not_found（隐藏存在性）。
    let resp = server
        .request("GET", &profile_path, Some(&outsider_token), None, None)
        .await;
    assert_error(&resp, 404, "agent_not_found");
    let resp = server
        .request(
            "PUT",
            &profile_path,
            Some(&outsider_token),
            Some("p-x"),
            Some(&put_profile_body(0, Some("x"), None)),
        )
        .await;
    assert_error(&resp, 404, "agent_not_found");

    // 输入卫生：invalid JSON / 未知字段 / 缺必填可空字段 → invalid_request；
    // NUL / 超字节上限 / 负 base_version → profile_invalid。
    let bad_bodies: Vec<(&str, u16, &str)> = vec![
        ("not json", 400, "invalid_request"),
        (
            r#"{"base_version":0,"short_description":"x","public_description":"y","extra":1}"#,
            400,
            "invalid_request",
        ),
        (
            r#"{"base_version":0,"public_description":null}"#,
            400,
            "invalid_request",
        ),
        (
            r#"{"base_version":0,"short_description":null}"#,
            400,
            "invalid_request",
        ),
        (
            r#"{"base_version":-1,"short_description":"x","public_description":null}"#,
            400,
            "profile_invalid",
        ),
    ];
    for (body, status, code) in bad_bodies {
        let resp = server
            .request_raw(
                "PUT",
                &profile_path,
                Some(&owner_token),
                Some("p-bad"),
                "application/json",
                body.as_bytes(),
            )
            .await;
        assert_eq!(resp.status, status, "body {body:?}: {}", resp.json());
        assert_eq!(resp.json()["error"]["code"], code, "body {body:?}");
    }
    // NUL：JSON 转义 \u0000 → 域层拒绝 → profile_invalid。
    let nul = server
        .request_raw(
            "PUT",
            &profile_path,
            Some(&owner_token),
            Some("p-nul"),
            "application/json",
            br#"{"base_version":0,"short_description":"a\u0000b","public_description":null}"#,
        )
        .await;
    assert_error(&nul, 400, "profile_invalid");
    // 超 512 字节短描述。
    let long_short = format!(
        r#"{{"base_version":0,"short_description":"{}","public_description":null}}"#,
        "a".repeat(513)
    );
    let resp = server
        .request_raw(
            "PUT",
            &profile_path,
            Some(&owner_token),
            Some("p-long"),
            "application/json",
            long_short.as_bytes(),
        )
        .await;
    assert_error(&resp, 400, "profile_invalid");

    // 缺 Idempotency-Key → idempotency_key_required；无认证 → 401。
    let resp = server
        .request(
            "PUT",
            &profile_path,
            Some(&owner_token),
            None,
            Some(&put_profile_body(0, Some("x"), None)),
        )
        .await;
    assert_error(&resp, 400, "idempotency_key_required");
    let resp = server
        .request(
            "PUT",
            &profile_path,
            None,
            Some("p-na"),
            Some(&put_profile_body(0, Some("x"), None)),
        )
        .await;
    assert_error(&resp, 401, "authentication_required");

    // 撤销 Agent（owner 关系与 agent 同时 revoked）→ GET/PUT 409 agent_not_active；
    // 撤销后即使曾有写入，授权先于重放也 fail closed。
    let del = server
        .request(
            "DELETE",
            &format!("/v1/agents/{agent_id}"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(del.status, 204);
    let resp = server
        .request("GET", &profile_path, Some(&owner_token), None, None)
        .await;
    assert_error(&resp, 409, "agent_not_active");
    let resp = server
        .request(
            "PUT",
            &profile_path,
            Some(&owner_token),
            Some("p-rev"),
            Some(&put_profile_body(0, Some("x"), None)),
        )
        .await;
    assert_error(&resp, 409, "agent_not_active");

    server.stop().await;
    run.cleanup();
}

// ============================================================
// Test 2: Group Room Presentation 全流程 + 只读投影 + 无 IM 副作用 + 重启
// ============================================================

#[tokio::test(flavor = "multi_thread")]
async fn presentation_flows_no_im_side_effects_and_restart() {
    let mut run = RunDir::create();
    let db_path = run.db_path();
    let media_root = run.media_path();
    let mut server = Server::start(&db_path, &media_root).await;

    let owner = cli_json(&db_path, &["bootstrap-human", "--display-name", "Owner"]).await;
    let owner_id = require_string(&owner, "participant_id");
    let owner_token = require_string(&owner, "token");
    let bob = cli_json(&db_path, &["create-human", "--display-name", "Bob"]).await;
    let bob_id = require_string(&bob, "participant_id");
    let bob_token = require_string(&bob, "token");

    // ---- Phase A：布置世界（会改变 Membership/事实，全部在基线之前）。----
    let agent_id = create_agent(&server, &owner_token, "a-1", "端侧助手").await;
    let group = create_group(&server, &owner_token, "g-1", "项目群").await;
    invite_member(&server, &owner_token, &group, &agent_id, "inv-a").await;
    // Bob（Ordinary Human）经联系人 → 入群成为普通 member（非 owner）。
    let req = server
        .request(
            "POST",
            "/v1/contacts/requests",
            Some(&owner_token),
            Some("c-1"),
            Some(&serde_json::json!({ "participant_id": bob_id })),
        )
        .await;
    assert_eq!(req.status, 201, "contact request: {}", req.json());
    let acpt = server
        .request(
            "POST",
            &format!("/v1/contacts/{owner_id}/accept"),
            Some(&bob_token),
            None,
            None,
        )
        .await;
    assert_eq!(acpt.status, 200, "accept: {}", acpt.json());
    invite_member(&server, &owner_token, &group, &bob_id, "inv-b").await;
    // 一个 Owner 拥有但不在该 group 的 Agent（无 Room 权威场景，先生成于基线前）。
    let outside_agent = create_agent(&server, &owner_token, "a-out", "外部助手").await;

    // 基线：公共消息 1 条 + human events 计数（之后 Profile/Presentation 写不得变更）。
    let msg = server
        .request(
            "POST",
            &format!("/v1/rooms/{group}/messages"),
            Some(&owner_token),
            Some("m-1"),
            Some(&serde_json::json!({ "body": "基线消息" })),
        )
        .await;
    assert_eq!(msg.status, 201, "message: {}", msg.json());
    let history_before = server
        .request(
            "GET",
            &format!("/v1/rooms/{group}/messages"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(
        history_before.json()["messages"].as_array().unwrap().len(),
        1
    );
    let events_before = server
        .request("GET", "/v1/human/events", Some(&owner_token), None, None)
        .await;
    let events_before_count = events_before.json()["events"].as_array().unwrap().len();

    // ---- Phase B：只做 Profile/Presentation 写与拒绝路径（窗口：无 IM 副作用）。----
    let profile_path = format!("/v1/agents/{agent_id}/profile");
    let pres_path = format!("/v1/rooms/{group}/members/{agent_id}/presentation");
    let resp = server
        .request(
            "PUT",
            &profile_path,
            Some(&owner_token),
            Some("p-1"),
            Some(&put_profile_body(
                0,
                Some("short bio"),
                Some("secret public bio"),
            )),
        )
        .await;
    assert_eq!(resp.status, 201, "profile create: {}", resp.json());

    let resp = server
        .request(
            "PUT",
            &pres_path,
            Some(&owner_token),
            Some("p-1"),
            Some(&put_presentation_body(
                0,
                Some("项目群显示"),
                Some("组长"),
                Some("负责全栈"),
            )),
        )
        .await;
    assert_eq!(resp.status, 201, "presentation create: {}", resp.json());
    assert_eq!(resp.json()["presentation"]["presentation_version"], 1);
    assert_eq!(resp.json()["presentation"]["published_by_kind"], "human");
    assert_eq!(resp.json()["idempotency_replayed"], false);

    // 更新（新 key、base 1）→ 200 版本 2；同 key 同摘要重放 → 201 版本 1
    // （即使 head 已到 2）；同 key（p-1）不同请求摘要 → 409 idempotency_conflict。
    let resp = server
        .request(
            "PUT",
            &pres_path,
            Some(&owner_token),
            Some("p-2"),
            Some(&put_presentation_body(1, Some("群显示二"), None, None)),
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["presentation"]["presentation_version"], 2);
    assert!(resp.json()["presentation"]["room_role_name"].is_null());
    let resp = server
        .request(
            "PUT",
            &pres_path,
            Some(&owner_token),
            Some("p-1"),
            Some(&put_presentation_body(
                0,
                Some("项目群显示"),
                Some("组长"),
                Some("负责全栈"),
            )),
        )
        .await;
    assert_eq!(resp.status, 201, "replay: {}", resp.json());
    assert_eq!(resp.json()["presentation"]["presentation_version"], 1);
    assert_eq!(resp.json()["idempotency_replayed"], true);
    let resp = server
        .request(
            "PUT",
            &pres_path,
            Some(&owner_token),
            Some("p-1"),
            Some(&put_presentation_body(
                1,
                Some("不同请求"),
                Some("组长"),
                Some("负责全栈"),
            )),
        )
        .await;
    assert_error(&resp, 409, "idempotency_conflict");
    let resp = server
        .request(
            "PUT",
            &pres_path,
            Some(&owner_token),
            Some("p-stale"),
            Some(&put_presentation_body(0, Some("X"), None, None)),
        )
        .await;
    assert_error(&resp, 409, "room_presentation_stale");

    // 超长 role description 写（head 已到 2）→ v3；summary preview 用。
    // 600 ASCII 字节：大于 512 preview 预算、小于 4096 上限。
    let long_desc = "r".repeat(600);
    let resp = server
        .request(
            "PUT",
            &pres_path,
            Some(&owner_token),
            Some("p-long"),
            Some(&put_presentation_body(
                2,
                Some("群显示二"),
                Some("角色"),
                Some(&long_desc),
            )),
        )
        .await;
    assert_eq!(resp.status, 200, "long desc write: {}", resp.json());

    // 失败路径（全部不改变事实）：普通成员 Bob 写、target 非 Agent 成员、Agent
    // Owner 无 Room 成员资格。
    let resp = server
        .request(
            "PUT",
            &pres_path,
            Some(&bob_token),
            Some("p-bob"),
            Some(&put_presentation_body(3, Some("D"), None, None)),
        )
        .await;
    assert_error(&resp, 404, "room_not_found");
    let resp = server
        .request(
            "PUT",
            &format!("/v1/rooms/{group}/members/{owner_id}/presentation"),
            Some(&owner_token),
            Some("p-target"),
            Some(&put_presentation_body(3, Some("D"), None, None)),
        )
        .await;
    assert_error(&resp, 404, "member_not_found");
    let resp = server
        .request(
            "PUT",
            &format!("/v1/rooms/{group}/members/{outside_agent}/presentation"),
            Some(&owner_token),
            Some("p-outsider"),
            Some(&put_presentation_body(3, Some("D"), None, None)),
        )
        .await;
    assert_error(&resp, 404, "member_not_found");

    // 无 IM 副作用：消息历史与 human events 数量在写前/写后不变；members 角色不变。
    let history_after = server
        .request(
            "GET",
            &format!("/v1/rooms/{group}/messages"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(
        history_after.json()["messages"].as_array().unwrap().len(),
        history_before.json()["messages"].as_array().unwrap().len(),
        "Profile/Presentation writes must not create public messages: {}",
        history_after.json()
    );
    let events_after = server
        .request("GET", "/v1/human/events", Some(&owner_token), None, None)
        .await;
    assert_eq!(
        events_after.json()["events"].as_array().unwrap().len(),
        events_before_count,
        "Profile/Presentation writes must not create public events: {}",
        events_after.json()
    );
    let members_resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group}/members"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(members_resp.status, 200, "{}", members_resp.json());
    let members_json = members_resp.json();
    assert_members_shape(&members_json);
    let list = members_json["members"].as_array().unwrap();
    assert_eq!(list.len(), 3, "owner + agent + bob: {list:?}");
    let agent_mem = list
        .iter()
        .find(|i| i["participant"]["id"] == agent_id)
        .expect("agent is a member");
    assert_eq!(agent_mem["membership"]["role"], "member");

    // ---- Phase C：只读投影 + 独立 Room 版本链 + Direct 不支持。----
    // Summary 含全部 active 成员（Human 与 Agent）；只有 Agent 有 presentation，
    // Human（owner 与 bob）的 presentation 字段为 null；绝不含 full descriptions；
    // preview 由 full stored room_role_description 读时生成（<= 512 字节）。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group}/member-presentations"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    let summary_json = resp.json();
    let members = summary_json["members"].as_array().unwrap();
    assert_eq!(members.len(), 3, "owner + agent + bob: {resp:?}");
    let m = members
        .iter()
        .find(|m| m["participant"]["id"] == agent_id)
        .expect("agent is in summary");
    assert_eq!(m["participant"]["kind"], "agent");
    assert_eq!(m["short_description"], "short bio");
    assert_eq!(m["room_display_name"], "群显示二");
    assert_eq!(m["room_role_name"], "角色");
    let preview = m["room_role_description_preview"].as_str().unwrap();
    assert!(
        !preview.is_empty() && preview.len() <= 512,
        "preview must be 1..=512 bytes"
    );
    for (human_id, human_name) in [(owner_id, "owner"), (bob_id, "bob")] {
        let h = members
            .iter()
            .find(|m| m["participant"]["id"] == human_id)
            .expect("active human member is in summary");
        assert_eq!(h["participant"]["kind"], "human", "{h:?}");
        assert!(h["short_description"].is_null(), "{h:?}");
        assert!(h["room_display_name"].is_null(), "{h:?}");
        assert!(h["room_role_name"].is_null(), "{h:?}");
        assert!(
            h["room_role_description_preview"].is_null(),
            "{human_name}: {h:?}"
        );
    }
    let summary_text = summary_json.to_string();
    for m in members {
        assert!(
            m.get("room_role_description").is_none() && m.get("public_description").is_none(),
            "summary must not expose full role description or public_description: {summary_text}"
        );
    }
    assert!(
        !summary_text.contains("secret public bio"),
        "summary must never include public_description: {summary_text}"
    );
    // 非成员（Bob 不在第二群）看不到 summary → 404 room_not_found。
    let group_b = create_group(&server, &owner_token, "g-2", "第二群").await;
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_b}/member-presentations"),
            Some(&bob_token),
            None,
            None,
        )
        .await;
    assert_error(&resp, 404, "room_not_found");

    // Member detail：participant + profile + presentation；直接看 full public bio。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group}/members/{agent_id}/profile"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["short_description"], "short bio");
    assert_eq!(
        resp.json()["profile"]["public_description"],
        "secret public bio"
    );
    assert_eq!(resp.json()["presentation"]["room_role_name"], "角色");

    // Direct Room 不支持 → 400 room_presentation_not_supported；GET 恒 null。
    let direct = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&owner_token),
            Some("d-1"),
            Some(&serde_json::json!({ "agent_id": agent_id })),
        )
        .await;
    assert_eq!(direct.status, 201, "{}", direct.json());
    let direct_room = direct.json()["room"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "PUT",
            &format!("/v1/rooms/{direct_room}/members/{agent_id}/presentation"),
            Some(&owner_token),
            Some("p-dir"),
            Some(&put_presentation_body(0, Some("D"), None, None)),
        )
        .await;
    assert_error(&resp, 400, "room_presentation_not_supported");
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{direct_room}/members/{agent_id}/presentation"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert!(resp.json()["presentation"].is_null());

    // 两个 group 独立版本链：第二群给同一 agent 单独写 v1；主群继续到 v4 且
    // null display name 继承语义不虚构。
    invite_member(&server, &owner_token, &group_b, &agent_id, "inv-gb").await;
    let resp = server
        .request(
            "PUT",
            &format!("/v1/rooms/{group_b}/members/{agent_id}/presentation"),
            Some(&owner_token),
            Some("pb-1"),
            Some(&put_presentation_body(0, None, Some("二群角色"), None)),
        )
        .await;
    assert_eq!(resp.status, 201, "{}", resp.json());
    assert_eq!(resp.json()["presentation"]["presentation_version"], 1);
    assert!(resp.json()["presentation"]["room_display_name"].is_null());
    let resp = server
        .request(
            "PUT",
            &pres_path,
            Some(&owner_token),
            Some("p-nulld"),
            Some(&put_presentation_body(3, None, Some("新角色名"), None)),
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["presentation"]["presentation_version"], 4);
    assert!(
        resp.json()["presentation"]["room_display_name"].is_null(),
        "null room_display_name must stay null (client inherits room name)"
    );
    assert_eq!(resp.json()["presentation"]["room_role_name"], "新角色名");
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_b}/members/{agent_id}/presentation"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.json()["presentation"]["presentation_version"], 1);

    // left member：把 agent 移出主群 → member detail 404 member_not_found。
    let rm = server
        .request(
            "DELETE",
            &format!("/v1/rooms/{group}/members/{agent_id}"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(rm.status, 204, "{}", rm.json());
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group}/members/{agent_id}/profile"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_error(&resp, 404, "member_not_found");

    // ---- 重启后读回不可变事实（Profile / Presentation 最新版本与 null 语义）。----
    server.stop().await;
    let mut server = Server::start(&db_path, &media_root).await;
    let resp = server
        .request(
            "GET",
            &format!("/v1/agents/{agent_id}/profile"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["short_description"], "short bio");
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_b}/members/{agent_id}/presentation"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["presentation"]["presentation_version"], 1);
    assert!(resp.json()["presentation"]["room_display_name"].is_null());
    server.stop().await;
    run.cleanup();
}
