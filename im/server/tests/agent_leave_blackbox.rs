//! Agent 退房 · 真实进程黑盒（CORE_V1_AGENT_LEAVE.md §7）：只用 operator CLI
//! 与公开 HTTP 验证切片 M2——Agent 经 BackendAuth 主动退出房间（群与 Direct
//! Room 都能退、非成员/未知/形状非法一律 404、重复退房 404、Human token 与
//! 未知 token 401），退出后该房对它彻底消失，Human 留在房里且成员名单如实
//! 呈现 `closed_by='leave'`，**退出跨 Core 重启存活**。
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
        let dir = workdir.join(format!("agent-leave-blackbox-{}", Uuid::new_v4()));
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

/// 一个真实世界的最小装配：Human + Agent A + 一条 Direct Room（A 在内）+
/// 一条只有 Human 的 Group Room + 一条 A 也在内的 Group Room。
/// 全部经公开协议完成。
struct World {
    server: Server,
    human_token: String,
    agent_token: String,
    direct_room: String,
    outsider_room: String,
    /// Alice(owner) + Agent A(member) 的群：退群用例的主场。
    group_with_agent: String,
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

    // A 也在内的群（退群用例的主场）。
    let created = server
        .request(
            "POST",
            "/v1/groups",
            Some(&human_token),
            Some("rc-g-2"),
            Some(&serde_json::json!({"name": "有 Agent 的群"})),
        )
        .await;
    assert_eq!(created.status, 201, "create group with agent: {created:?}");
    let group_with_agent = require_string(&created.json()["room"], "id");
    let invited = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_with_agent}/members"),
            Some(&human_token),
            Some("rc-g-2-i"),
            Some(&serde_json::json!({"participant_id": agent_id.clone()})),
        )
        .await;
    assert_eq!(invited.status, 201, "invite agent: {invited:?}");

    World {
        server,
        human_token,
        agent_token,
        direct_room,
        outsider_room,
        group_with_agent,
    }
}

impl World {
    /// 一次退房（BackendAuth；无体、无幂等键）。
    async fn leave(&self, room: &str) -> Response {
        self.server
            .request(
                "POST",
                &format!("/v1/backend/rooms/{room}/leave"),
                Some(&self.agent_token),
                None,
                None,
            )
            .await
    }

    /// 该房是否还在 Agent 的通讯录里（走公开 Backend 读面）。
    async fn lists_room(&self, room: &str) -> bool {
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
            .expect("rooms array")
            .iter()
            .any(|r| r["room_id"].as_str() == Some(room))
    }
}

#[tokio::test]
async fn agent_leave_over_real_process() {
    let mut run = RunDir::create();
    let mut world = build_world(&run.db_path(), &run.media_path()).await;

    // 退房前：群里能看到这间房。
    assert!(
        world.lists_room(&world.group_with_agent).await,
        "退房前：群在 Agent 的通讯录里"
    );

    // 退群：204 无体。
    let left = world.leave(&world.group_with_agent).await;
    assert_eq!(left.status, 204, "退群: {left:?}");

    // 退房后：该房彻底消失（通讯录不再列）；重复退房 404（周期已关闭）。
    assert!(
        !world.lists_room(&world.group_with_agent).await,
        "退房后：通讯录不再列这间房"
    );
    let again = world.leave(&world.group_with_agent).await;
    assert_eq!(again.status, 404, "重复退房: {again:?}");
    assert_eq!(
        again.json()["error"]["code"].as_str(),
        Some("room_not_found")
    );

    // 非成员房 / 未知房 / 形状非法：一律 404（不泄露存在性）。
    let outsider = world.leave(&world.outsider_room).await;
    assert_eq!(outsider.status, 404, "非成员房: {outsider:?}");
    let unknown = world.leave(&Uuid::new_v4().to_string()).await;
    assert_eq!(unknown.status, 404, "未知房: {unknown:?}");
    let malformed = world.leave("not-a-uuid").await;
    assert_eq!(malformed.status, 404, "形状非法: {malformed:?}");

    // 退出 Direct Room：Agent 出局，**Human 留在房里**（成员名单里那条周期
    // 如实说 closed_by='leave'）。
    let left = world.leave(&world.direct_room).await;
    assert_eq!(left.status, 204, "退出 direct 房: {left:?}");
    assert!(
        !world.lists_room(&world.direct_room).await,
        "Agent 侧：direct 房不再出现在它的通讯录里"
    );
    let members = world
        .server
        .request(
            "GET",
            &format!("/v1/rooms/{}/members", world.direct_room),
            Some(&world.human_token),
            None,
            None,
        )
        .await;
    assert_eq!(members.status, 200, "Human 侧成员名单: {members:?}");
    let agent_member = members.json()["members"]
        .as_array()
        .expect("members")
        .iter()
        .find(|m| m["participant"]["kind"].as_str() == Some("agent"))
        .cloned()
        .expect("那位 Agent 还在名单里（周期 closed，不删行）");
    assert_eq!(
        agent_member["membership"]["closed_by"].as_str(),
        Some("leave"),
        "成员投影如实说它离开了: {agent_member}"
    );
    assert!(
        agent_member["membership"]["left_at"].is_string(),
        "left_at 落定: {agent_member}"
    );

    // Human 侧退房端点语义不变（回归）：owner 退自己的群 → 409 先转让。
    let refused = world
        .server
        .request(
            "POST",
            &format!("/v1/rooms/{}/leave", world.outsider_room),
            Some(&world.human_token),
            None,
            None,
        )
        .await;
    assert_eq!(refused.status, 409, "Human owner 退群: {refused:?}");
    assert_eq!(
        refused.json()["error"]["code"].as_str(),
        Some("owner_transfer_required")
    );

    // 认证：Human token / 未知 token 打 Backend 退房端点一律 401。
    for token in [world.human_token.as_str(), "clr_bc_not-a-real-token"] {
        let refused = world
            .server
            .request(
                "POST",
                &format!("/v1/backend/rooms/{}/leave", world.direct_room),
                Some(token),
                None,
                None,
            )
            .await;
        assert_eq!(refused.status, 401, "token {token}: {refused:?}");
        assert_eq!(
            refused.json()["error"]["code"].as_str(),
            Some("invalid_credential")
        );
    }

    world.server.stop().await;
    run.cleanup();
}

/// **退出是耐久事实**：Core 重启之后那间房仍然不在它的通讯录里。
#[tokio::test]
async fn agent_leave_survives_core_restart() {
    let mut run = RunDir::create();
    let mut world = build_world(&run.db_path(), &run.media_path()).await;

    let left = world.leave(&world.group_with_agent).await;
    assert_eq!(left.status, 204, "退群: {left:?}");

    // 重启真实 Core（同一 db / media / 端口）。
    let db = run.db_path();
    let media = run.media_path();
    world.server.stop().await;
    world.server = Server::start(&db, &media).await;

    assert!(
        !world.lists_room(&world.group_with_agent).await,
        "重启后：那间房仍然不在通讯录里"
    );
    let again = world.leave(&world.group_with_agent).await;
    assert_eq!(again.status, 404, "重启后重复退房仍是 404: {again:?}");

    world.server.stop().await;
    run.cleanup();
}
