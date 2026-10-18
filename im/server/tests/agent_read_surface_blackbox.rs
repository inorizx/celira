//! Agent 读取面对齐 · 真实进程黑盒（CORE_V1_AGENT_READ_SURFACE.md §7）：只用
//! operator CLI 与公开 HTTP 验证 6 条只读端点——成员名单/成员呈现/成员
//! Profile/成员头像/自己的 Profile，以及认证互斥、非成员
//! fail closed、跨重启稳定。
//!
//! ⚠ 2026-09-28：原为 7 条 —— 第 7 条 `GET /v1/backend/publication` 随
//! **Agent 服务层整体退役**删除。
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

/// 黑盒运行目录 guard（`data/agent-read-surface-blackbox-<uuid>`，git 忽略）。
struct RunDir {
    dir: PathBuf,
    cleaned: bool,
}

impl RunDir {
    fn create() -> Self {
        let workdir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        std::fs::create_dir_all(&workdir).expect("create ignored data dir");
        let dir = workdir.join(format!("agent-read-surface-blackbox-{}", Uuid::new_v4()));
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

/// 一个真实世界的最小装配：Human + Agent A/B + 一条 Group Room（两 Agent 在内）
/// + 一条 Direct Room（只有 A 在内）。全部经公开协议完成。
struct World {
    server: Server,
    human_token: String,
    agent_a: String,
    agent_b: String,
    agent_a_token: String,
    agent_b_token: String,
    direct_room: String,
    group_room: String,
}

async fn build_world(db: &Path, media: &Path) -> World {
    let server = Server::start(db, media).await;
    let boot = cli_json(db, &["bootstrap-human", "--display-name", "主人"]).await;
    let human_token = require_string(&boot, "token");

    let mut world = World {
        server,
        human_token,
        agent_a: String::new(),
        agent_b: String::new(),
        agent_a_token: String::new(),
        agent_b_token: String::new(),
        direct_room: String::new(),
        group_room: String::new(),
    };

    world.agent_a = world.create_agent("Agent A", "ra-1").await;
    world.agent_b = world.create_agent("Agent B", "ra-2").await;
    world.agent_a_token = world.issue_credential(&world.agent_a.clone(), "rb-a").await;
    world.agent_b_token = world.issue_credential(&world.agent_b.clone(), "rb-b").await;

    let created = world
        .server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&world.human_token),
            Some("rd-1"),
            Some(&serde_json::json!({"agent_id": world.agent_a.clone()})),
        )
        .await;
    assert_eq!(created.status, 201, "create direct room: {created:?}");
    world.direct_room = require_string(&created.json()["room"], "id");

    let created = world
        .server
        .request(
            "POST",
            "/v1/groups",
            Some(&world.human_token),
            Some("rg-1"),
            Some(&serde_json::json!({"name": "项目群"})),
        )
        .await;
    assert_eq!(created.status, 201, "create group: {created:?}");
    world.group_room = require_string(&created.json()["room"], "id");

    for (key, agent) in [("ri-1", &world.agent_a), ("ri-2", &world.agent_b)] {
        let invited = world
            .server
            .request(
                "POST",
                &format!("/v1/rooms/{}/members", world.group_room),
                Some(&world.human_token),
                Some(key),
                Some(&serde_json::json!({"participant_id": agent})),
            )
            .await;
        assert_eq!(invited.status, 201, "invite {agent}: {invited:?}");
    }
    world
}

impl World {
    async fn create_agent(&self, name: &str, key: &str) -> String {
        let created = self
            .server
            .request(
                "POST",
                "/v1/agents",
                Some(&self.human_token),
                Some(key),
                Some(&serde_json::json!({"display_name": name})),
            )
            .await;
        assert_eq!(created.status, 201, "create agent {name}: {created:?}");
        require_string(&created.json()["agent"], "id")
    }

    async fn issue_credential(&self, agent_id: &str, key: &str) -> String {
        let created = self
            .server
            .request(
                "POST",
                &format!("/v1/agents/{agent_id}/backend-bindings"),
                Some(&self.human_token),
                Some(&format!("{key}-b")),
                Some(&serde_json::json!({"label": "read surface blackbox"})),
            )
            .await;
        assert_eq!(created.status, 201, "create binding: {created:?}");
        let binding = require_string(&created.json()["binding"], "id");
        let issued = self
            .server
            .request(
                "POST",
                &format!("/v1/agents/{agent_id}/backend-bindings/{binding}/credentials"),
                Some(&self.human_token),
                Some(&format!("{key}-c")),
                Some(&serde_json::json!({"replaces_credential_id": null})),
            )
            .await;
        assert_eq!(issued.status, 201, "issue credential: {issued:?}");
        require_string(&issued.json(), "token")
    }
}

/// 6 条读取面端点（`{room}` / `{member}` 由调用方展开）。
///
/// ⚠ 2026-09-28：原为 7 条 —— 第 7 条 `GET /v1/backend/publication`（读自己的
/// Service Publication）随 **Agent 服务层整体退役**删除。
const READ_SURFACE: [&str; 6] = [
    "/v1/backend/rooms/{room}/members",
    "/v1/backend/rooms/{room}/member-presentations",
    "/v1/backend/rooms/{room}/members/{member}/profile",
    "/v1/backend/rooms/{room}/members/{member}/presentation",
    "/v1/backend/rooms/{room}/members/{member}/avatar",
    "/v1/backend/profile",
];

fn expand(template: &str, room: &str, member: &str) -> String {
    template.replace("{room}", room).replace("{member}", member)
}

#[tokio::test]
async fn agent_read_surface_over_real_process() {
    let mut run = RunDir::create();
    let db = run.db_path();
    let media = run.media_path();
    let mut world = build_world(&db, &media).await;

    // Owner 为 Agent B 写公共呈现（写路径仍只对 Owner —— 本刀不碰）。
    let written = world
        .server
        .request(
            "PUT",
            &format!("/v1/agents/{}/profile", world.agent_b),
            Some(&world.human_token),
            Some("rp-b"),
            Some(&serde_json::json!({
                "base_version": 0,
                "short_description": "记录员",
                "public_description": "负责把讨论整理成条目。"
            })),
        )
        .await;
    assert_eq!(written.status, 201, "owner writes profile: {written:?}");

    // 1) 成员名单：Agent A 在群里看到 3 名 active 成员（Human + A + B）。
    let members = world
        .server
        .request(
            "GET",
            &format!("/v1/backend/rooms/{}/members", world.group_room),
            Some(&world.agent_a_token),
            None,
            None,
        )
        .await;
    assert_eq!(members.status, 200, "member list: {members:?}");
    assert_eq!(
        members.json()["members"].as_array().unwrap().len(),
        3,
        "Human + Agent A + Agent B"
    );

    // 2) 同房成员 Profile：看到 Owner 写的那一份公共呈现。
    let profile = world
        .server
        .request(
            "GET",
            &format!(
                "/v1/backend/rooms/{}/members/{}/profile",
                world.group_room, world.agent_b
            ),
            Some(&world.agent_a_token),
            None,
            None,
        )
        .await;
    assert_eq!(profile.status, 200, "member profile: {profile:?}");
    assert_eq!(profile.json()["participant"]["id"], world.agent_b.as_str());
    assert_eq!(profile.json()["profile"]["short_description"], "记录员");
    assert_eq!(
        profile.json()["profile"]["published_by_kind"],
        "human",
        "呈现由 Owner 发布（写路径本刀不改）"
    );

    // 3) 自己的 Profile：身份只来自认证，无版本为 null。
    // ⚠ 2026-09-28：这一步原先还打 `GET /v1/backend/publication`（读自己的
    // Service Publication）——那条端点随 Agent 服务层整体退役删除。
    let own_profile = world
        .server
        .request(
            "GET",
            "/v1/backend/profile",
            Some(&world.agent_a_token),
            None,
            None,
        )
        .await;
    assert_eq!(own_profile.status, 200, "self profile: {own_profile:?}");
    assert!(
        own_profile.json()["profile"].is_null(),
        "A 尚无 Profile 版本"
    );

    // 4) 成员 Presentation 摘要：Human 行存在但 Presentation 字段为 null。
    let summaries = world
        .server
        .request(
            "GET",
            &format!(
                "/v1/backend/rooms/{}/member-presentations",
                world.group_room
            ),
            Some(&world.agent_a_token),
            None,
            None,
        )
        .await;
    assert_eq!(summaries.status, 200, "summaries: {summaries:?}");
    let summaries_json = summaries.json();
    let human_row = summaries_json["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["participant"]["kind"] == "human")
        .expect("human member row present");
    assert!(
        human_row["room_role_name"].is_null(),
        "Human 无 Presentation"
    );

    // 5) 无头像 → 404 avatar_not_found（不是 500，也不是空 200）。
    let avatar = world
        .server
        .request(
            "GET",
            &format!(
                "/v1/backend/rooms/{}/members/{}/avatar",
                world.group_room, world.agent_b
            ),
            Some(&world.agent_a_token),
            None,
            None,
        )
        .await;
    assert_eq!(avatar.status, 404, "no avatar: {avatar:?}");
    assert_eq!(avatar.json()["error"]["code"], "avatar_not_found");

    world.server.stop().await;
    run.cleanup();
}

#[tokio::test]
async fn human_token_and_non_member_fail_closed_over_real_process() {
    let mut run = RunDir::create();
    let db = run.db_path();
    let media = run.media_path();
    let mut world = build_world(&db, &media).await;

    // Human Session token 打 6 条端点 → 一律 401 invalid_credential。
    for template in READ_SURFACE {
        let path = expand(template, &world.group_room, &world.agent_b);
        let response = world
            .server
            .request("GET", &path, Some(&world.human_token), None, None)
            .await;
        assert_eq!(response.status, 401, "Human token must not pass: {path}");
        assert_eq!(
            response.json()["error"]["code"],
            "invalid_credential",
            "on {path}"
        );
    }

    // 缺 Authorization → 401 authentication_required。
    let anon = world
        .server
        .request(
            "GET",
            &format!("/v1/backend/rooms/{}/members", world.group_room),
            None,
            None,
            None,
        )
        .await;
    assert_eq!(anon.status, 401);
    assert_eq!(anon.json()["error"]["code"], "authentication_required");

    // Agent B 不是 Direct Room 成员 → 5 条房间范围端点一律 404 room_not_found。
    for template in &READ_SURFACE[..5] {
        let path = expand(template, &world.direct_room, &world.agent_a);
        let response = world
            .server
            .request("GET", &path, Some(&world.agent_b_token), None, None)
            .await;
        assert_eq!(response.status, 404, "non-member must fail closed: {path}");
        assert_eq!(response.json()["error"]["code"], "room_not_found");
    }

    world.server.stop().await;
    run.cleanup();
}

#[tokio::test]
async fn read_surface_is_durable_across_restart() {
    let mut run = RunDir::create();
    let db = run.db_path();
    let media = run.media_path();
    let mut world = build_world(&db, &media).await;

    let before = world
        .server
        .request(
            "GET",
            &format!("/v1/backend/rooms/{}/members", world.group_room),
            Some(&world.agent_a_token),
            None,
            None,
        )
        .await;
    assert_eq!(before.status, 200, "members before restart: {before:?}");
    let before_members = before.json()["members"].clone();

    world.server.stop().await;

    // 同一数据库重启：读取面只依赖耐久公共事实，不依赖进程内状态。
    let mut restarted = Server::start(&db, &media).await;
    let after = restarted
        .request(
            "GET",
            &format!("/v1/backend/rooms/{}/members", world.group_room),
            Some(&world.agent_a_token),
            None,
            None,
        )
        .await;
    assert_eq!(after.status, 200, "members after restart: {after:?}");
    assert_eq!(
        after.json()["members"],
        before_members,
        "重启前后成员投影必须逐字段一致"
    );

    restarted.stop().await;
    run.cleanup();
}
