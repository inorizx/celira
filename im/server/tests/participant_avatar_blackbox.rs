//! Participant Avatar 真实进程黑盒（CORE_V1_AVATAR_P0.md §6）：只通过
//! operator CLI 与公开 HTTP 验证头像的事实、写权限（active Agent Owner）、
//! 幂等/base_version、字节 round-trip 与完整性校验、投影存在性、重启持久性
//! 与 fail-closed。成功裁决不读取 SQLite；每次测试使用 `data/` 下唯一临时
//! 目录与空闲 loopback 端口，结束时回收进程与文件（不使用 /tmp）。

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

const BOUNDARY: &str = "----avatar-blackbox-boundary";
/// 极小合法形状 PNG（Core 不做魔数嗅探，字节任意；用真实 PNG 头更接近现实）。
const PNG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\x0aIDATx\x9cc\x00\x01\x00\x00\x05\x00\x01\x0d\x0a\x2d\xb4\x00\x00\x00\x00IEND\xaeB`\x82";

struct Response {
    status: u16,
    headers: HashMap<String, String>,
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
    // 头部与 body 必须按字节分界（body 可能是二进制头像字节，绝不做 UTF-8
    // lossy 转换——那会破坏任意字节）。
    let separator = b"\r\n\r\n";
    let sep_pos = raw
        .windows(separator.len())
        .position(|w| w == separator)
        .ok_or_else(|| "malformed HTTP response".to_owned())?;
    let (head_bytes, body_bytes) = raw.split_at(sep_pos);
    let head = String::from_utf8_lossy(head_bytes);
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
    let body_bytes = &body_bytes[separator.len()..];
    let body = match headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
    {
        Some(length) => body_bytes[..length].to_vec(),
        None => body_bytes.to_vec(),
    };
    Ok(Response {
        status,
        headers,
        body,
    })
}

struct Server {
    child: Child,
    addr: String,
    media_root: PathBuf,
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
        let server = Self {
            child,
            addr,
            media_root: media_root.to_path_buf(),
        };
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

    /// 任意原始 body / Content-Type（multipart、invalid JSON 等）。
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

    async fn put_avatar(
        &self,
        token: &str,
        agent_id: &str,
        key: &str,
        base_version: i64,
        bytes: &[u8],
        content_type: &str,
    ) -> Response {
        let body = avatar_multipart(base_version, bytes, content_type);
        let ct = format!("multipart/form-data; boundary={BOUNDARY}");
        self.request_raw(
            "PUT",
            &format!("/v1/agents/{agent_id}/profile/avatar"),
            Some(token),
            Some(key),
            &ct,
            &body,
        )
        .await
    }

    /// 篡改媒体目录中的头像字节（测试专用：翻转首个字节），返回是否成功。
    fn tamper_avatar_bytes(&self) -> bool {
        let attachments = self.media_root.join("attachments");
        let Ok(entries) = std::fs::read_dir(&attachments) else {
            return false;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // 跳过 Core temp 与目录项；只翻转 UUID 命名的 final 字节。
            if name.contains(".tmp") || name.is_empty() {
                continue;
            }
            if let Ok(mut bytes) = std::fs::read(&path) {
                if bytes.is_empty() {
                    continue;
                }
                bytes[0] ^= 0xff;
                return std::fs::write(&path, bytes).is_ok();
            }
        }
        false
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

fn avatar_multipart(base_version: i64, bytes: &[u8], content_type: &str) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"base_version\"\r\n\r\n{base_version}\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"avatar.png\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
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

struct RunDir {
    dir: PathBuf,
    cleaned: bool,
}

impl RunDir {
    fn create() -> Self {
        let workdir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        std::fs::create_dir_all(&workdir).expect("create ignored data dir");
        let dir = workdir.join(format!("participant-avatar-blackbox-{}", Uuid::new_v4()));
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

#[tokio::test(flavor = "multi_thread")]
async fn avatar_put_lifecycle_idempotency_and_fail_closed() {
    let mut run = RunDir::create();
    let db_path = run.db_path();
    let media_root = run.media_path();
    let mut server = Server::start(&db_path, &media_root).await;

    let owner = cli_json(&db_path, &["bootstrap-human", "--display-name", "Owner"]).await;
    let owner_token = require_string(&owner, "token");
    let outsider = cli_json(&db_path, &["create-human", "--display-name", "Outsider"]).await;
    let outsider_token = require_string(&outsider, "token");

    let agent_id = create_agent(&server, &owner_token, "a-1", "助手").await;
    let avatar_path = format!("/v1/agents/{agent_id}/profile/avatar");

    // 首次 PUT → 201（版本 0 → 1），profile.avatar 投影存在（存在性 +
    // content_type + size_bytes；无 storage key / hash / 路径）。
    let resp = server
        .put_avatar(&owner_token, &agent_id, "av-1", 0, PNG_BYTES, "image/png")
        .await;
    assert_eq!(resp.status, 201, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["profile_version"], 1);
    assert_eq!(
        resp.json()["profile"]["avatar"]["content_type"],
        "image/png"
    );
    assert_eq!(
        resp.json()["profile"]["avatar"]["size_bytes"],
        PNG_BYTES.len() as i64
    );
    assert_eq!(resp.json()["idempotency_replayed"], false);
    let avatar_json = resp.json().to_string();
    for forbidden in ["storage", "sha256", "path", "key"] {
        assert!(
            !avatar_json.contains(forbidden),
            "avatar projection must not leak {forbidden:?}: {avatar_json}"
        );
    }

    // Owner profile GET 投影 avatar。
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
    assert_eq!(
        resp.json()["profile"]["avatar"]["content_type"],
        "image/png"
    );

    // 更新（新 key、base=1）→ 200 版本 2。
    let resp = server
        .put_avatar(&owner_token, &agent_id, "av-2", 1, PNG_BYTES, "image/jpeg")
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["profile_version"], 2);
    assert_eq!(
        resp.json()["profile"]["avatar"]["content_type"],
        "image/jpeg"
    );

    // 同 key 同摘要重放首次（head 已到 2）→ 201 版本 1、idempotency_replayed。
    let resp = server
        .put_avatar(&owner_token, &agent_id, "av-1", 0, PNG_BYTES, "image/png")
        .await;
    assert_eq!(resp.status, 201, "replay: {}", resp.json());
    assert_eq!(resp.json()["profile"]["profile_version"], 1);
    assert_eq!(resp.json()["idempotency_replayed"], true);

    // 同 key 不同请求摘要 → 409 idempotency_conflict。
    let resp = server
        .put_avatar(
            &owner_token,
            &agent_id,
            "av-1",
            0,
            b"different",
            "image/png",
        )
        .await;
    assert_error(&resp, 409, "idempotency_conflict");

    // 新 key 过期 base_version → 409 profile_stale。
    let resp = server
        .put_avatar(&owner_token, &agent_id, "av-3", 0, PNG_BYTES, "image/png")
        .await;
    assert_error(&resp, 409, "profile_stale");

    // 非 owner → 404 agent_not_found（存在性隐藏）。
    let resp = server
        .put_avatar(
            &outsider_token,
            &agent_id,
            "av-x",
            0,
            PNG_BYTES,
            "image/png",
        )
        .await;
    assert_error(&resp, 404, "agent_not_found");

    // 缺 Idempotency-Key → idempotency_key_required；无认证 → 401。
    let body = avatar_multipart(0, PNG_BYTES, "image/png");
    let ct = format!("multipart/form-data; boundary={BOUNDARY}");
    let resp = server
        .request_raw("PUT", &avatar_path, Some(&owner_token), None, &ct, &body)
        .await;
    assert_error(&resp, 400, "idempotency_key_required");
    let resp = server
        .request_raw("PUT", &avatar_path, None, Some("av-na"), &ct, &body)
        .await;
    assert_error(&resp, 401, "authentication_required");

    // SVG / 空字节 → 400 avatar_invalid；超 2 MiB → 413。
    let resp = server
        .put_avatar(
            &owner_token,
            &agent_id,
            "k-svg",
            0,
            b"<svg/>",
            "image/svg+xml",
        )
        .await;
    assert_error(&resp, 400, "avatar_invalid");
    let resp = server
        .put_avatar(&owner_token, &agent_id, "k-empty", 0, b"", "image/png")
        .await;
    assert_error(&resp, 400, "avatar_invalid");
    let big = vec![0u8; 2 * 1024 * 1024 + 1];
    let resp = server
        .put_avatar(&owner_token, &agent_id, "k-big", 0, &big, "image/png")
        .await;
    assert_error(&resp, 413, "payload_too_large");

    // 撤销 Agent → PUT 409 agent_not_active（授权先于重放 fail closed）。
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
        .put_avatar(&owner_token, &agent_id, "av-rev", 0, PNG_BYTES, "image/png")
        .await;
    assert_error(&resp, 409, "agent_not_active");

    server.stop().await;
    run.cleanup();
}

#[tokio::test(flavor = "multi_thread")]
async fn avatar_byte_read_round_trip_projection_tamper_and_restart() {
    let mut run = RunDir::create();
    let db_path = run.db_path();
    let media_root = run.media_path();
    let mut server = Server::start(&db_path, &media_root).await;

    let owner = cli_json(&db_path, &["bootstrap-human", "--display-name", "Owner"]).await;
    let owner_token = require_string(&owner, "token");
    let bob = cli_json(&db_path, &["create-human", "--display-name", "Bob"]).await;
    let bob_id = require_string(&bob, "participant_id");
    let bob_token = require_string(&bob, "token");

    let agent_id = create_agent(&server, &owner_token, "a-1", "助手").await;
    let group = create_group(&server, &owner_token, "g-1", "项目群").await;
    invite_member(&server, &owner_token, &group, &agent_id, "inv-a").await;
    // Bob 经联系人入群成为普通成员（读路径可见性用）。
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
    let owner_id = require_string(&owner, "participant_id");
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

    // 无头像 → 404 avatar_not_found。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group}/members/{agent_id}/avatar"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_error(&resp, 404, "avatar_not_found");

    // PUT 头像 → 成员字节端点 round-trip。
    let put = server
        .put_avatar(&owner_token, &agent_id, "av-1", 0, PNG_BYTES, "image/png")
        .await;
    assert_eq!(put.status, 201, "{}", put.json());
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group}/members/{agent_id}/avatar"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", String::from_utf8_lossy(&resp.body));
    assert_eq!(resp.body, PNG_BYTES, "bytes must round-trip");
    assert_eq!(
        resp.headers.get("content-type").map(String::as_str),
        Some("image/png")
    );
    assert_eq!(
        resp.headers
            .get("x-content-type-options")
            .map(String::as_str),
        Some("nosniff")
    );
    assert!(
        resp.headers.contains_key("etag"),
        "etag must address the version"
    );

    // 成员详情 + member-presentations 摘要投影 avatar 存在性。
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
    assert_eq!(
        resp.json()["profile"]["avatar"]["content_type"],
        "image/png"
    );
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
    let summary = resp.json();
    let members = summary["members"].as_array().unwrap();
    let agent_m = members
        .iter()
        .find(|m| m["participant"]["id"] == agent_id)
        .expect("agent in summary");
    assert_eq!(agent_m["avatar"]["content_type"], "image/png");
    let bob_m = members
        .iter()
        .find(|m| m["participant"]["id"] == bob_id)
        .expect("bob in summary");
    assert!(bob_m["avatar"].is_null(), "human without avatar: {bob_m:?}");

    // Bob 是同群成员 → 可读字节；另一群非成员 owner 不可见（统一 404）。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group}/members/{agent_id}/avatar"),
            Some(&bob_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "active member can read: {}", resp.json());
    let group_b = create_group(&server, &owner_token, "g-2", "第二群").await;
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_b}/members/{agent_id}/avatar"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_error(&resp, 404, "member_not_found");

    // ---- 重启持久性（在篡改/摘除之前验证不可变头像字节跨重启保留）。----
    server.stop().await;
    let mut server = Server::start(&db_path, &media_root).await;
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group}/members/{agent_id}/avatar"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "{}", String::from_utf8_lossy(&resp.body));
    assert_eq!(resp.body, PNG_BYTES, "avatar bytes must survive restart");

    // ---- 篡改媒体目录字节 → 下载失败（完整性校验生效，绝不返回字节）。----
    assert!(
        server.tamper_avatar_bytes(),
        "test must locate and tamper the avatar blob"
    );
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group}/members/{agent_id}/avatar"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(
        resp.status,
        500,
        "corrupt avatar must fail closed: {}",
        resp.json()
    );
    assert_eq!(resp.json()["error"]["code"], "internal_error");

    // ---- 摘除：DELETE → 新版本 avatar=null → 最新投影无头像、字节 404。----
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/agents/{agent_id}/profile/avatar"),
            Some(&owner_token),
            Some("del-1"),
            Some(&serde_json::json!({ "base_version": 1 })),
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.json());
    assert_eq!(resp.json()["profile"]["profile_version"], 2);
    assert!(resp.json()["profile"]["avatar"].is_null());
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group}/members/{agent_id}/avatar"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_error(&resp, 404, "avatar_not_found");
    // 成员详情投影同步无头像。
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
    assert!(resp.json()["profile"]["avatar"].is_null());

    server.stop().await;
    run.cleanup();
}
