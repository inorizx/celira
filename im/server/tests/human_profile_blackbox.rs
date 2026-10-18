//! **人写自己的资料**的真实进程黑盒（表达权第四刀，2026-10-17 主人批准）。
//!
//! 只走公开面：operator CLI（建世界的第一位 Human）＋ HTTP（邀请码兑换、改名、
//! 上传头像、房间成员头像字节读取）＋ **重启一次**。不读 SQLite 断言业务事实。
//!
//! 判据一条链，缺一段都不算数：
//!
//! 1. 第一位 Human 由 CLI 建出；他用**邀请码**（`new_human`）请进第二个人 ——
//!    名字由**被邀请者自己**给（"自己的名字自己说"）；
//! 2. 第二个人**改自己的名字**、**上传自己的头像**；
//! 3. **重启 Core**：名字与头像都还在（持久事实，不是进程内投影）；
//! 4. 邀请者与新人建立关系并开一间私人房后，邀请者经**房间成员头像路径**拿到的字节
//!    与新人上传的**逐字节相同** —— 这就是"别人看得见"的硬证据；
//! 5. 反面：新人**改不到别人**（路径里没有"写谁的"），没有凭据一律 401。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::time::timeout;

const BIN: &str = env!("CARGO_BIN_EXE_celira-im");
const BOUNDARY: &str = "----human-profile-blackbox-boundary";

/// 合法的 1×1 PNG（头像白名单只认 png/jpeg/webp，且会核magic）。
const PNG_BYTES: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

struct Response {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Response {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("HTTP response must be JSON")
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
    // 头与 body 必须按**字节**分界：body 可能是任意二进制（头像），不许 UTF-8 lossy。
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

    async fn stop(mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
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
        let refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let bytes = body.map(|value| value.to_string().into_bytes());
        raw_request(&self.addr, method, path, &refs, bytes.as_deref())
            .await
            .unwrap_or_else(|e| panic!("request {method} {path} failed: {e}"))
    }

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
        let refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        raw_request(&self.addr, method, path, &refs, Some(body))
            .await
            .unwrap_or_else(|e| panic!("request {method} {path} failed: {e}"))
    }
}

/// 头像写入的 multipart 体：文件那一格必须叫 `file`（解析器只认 base_version 与 file）。
fn avatar_multipart(base_version: i64, bytes: &[u8], content_type: &str) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"base_version\"\r\n\r\n");
    body.extend_from_slice(base_version.to_string().as_bytes());
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"a.png\"\r\n",
    );
    body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
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

#[tokio::test]
async fn a_human_renames_and_sets_an_avatar_and_another_member_reads_the_same_bytes() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("core.db");
    let media_root: PathBuf = dir.path().join("media");

    let server = Server::start(&db_path, &media_root).await;

    // ① 世界的第一个人由 CLI 建出；他用邀请码请进第二个人（名字由被邀请者自己给）。
    let owner = cli_json(&db_path, &["bootstrap-human", "--display-name", "Owner"]).await;
    let owner_id = require_string(&owner, "participant_id");
    let owner_token = require_string(&owner, "token");

    let issued = server
        .request(
            "POST",
            "/v1/home/pairing-grants",
            Some(&owner_token),
            None,
            Some(&json!({"grants": "new_human"})),
        )
        .await;
    assert!(
        issued.status == 200 || issued.status == 201,
        "issue grant: {} {}",
        issued.status,
        issued.json()
    );
    let invite = require_string(&issued.json(), "token");

    let exchanged = server
        .request(
            "POST",
            "/v1/home/pairing-grants/exchange",
            Some(&invite),
            None,
            Some(&json!({"display_name": "小乙"})),
        )
        .await;
    // 兑换成功是 201（新 Human 被创建）；重放才是 200 —— 这里只要求"成功了"。
    assert!(
        exchanged.status == 200 || exchanged.status == 201,
        "exchange: {} {}",
        exchanged.status,
        exchanged.json()
    );
    let guest = exchanged.json();
    let guest_token = require_string(&guest, "token");
    let guest_id = require_string(&guest["session"], "human_id");

    // ② 新人的名字是**他自己说的**。
    let profile = server
        .request("GET", "/v1/me/profile", Some(&guest_token), None, None)
        .await;
    assert_eq!(profile.status, 200, "{}", profile.json());
    assert_eq!(profile.json()["display_name"], "小乙");
    assert_eq!(profile.json()["profile"], Value::Null, "还没写过呈现");

    // 改名（状态写：没有版本行、没有幂等键）。
    let renamed = server
        .request(
            "PUT",
            "/v1/me/profile",
            Some(&guest_token),
            None,
            Some(&json!({"display_name": "小乙 二号"})),
        )
        .await;
    assert_eq!(renamed.status, 200, "{}", renamed.json());
    assert_eq!(renamed.json()["display_name"], "小乙 二号");

    // 上传头像（版本链：key + base_version + 字节）。
    let body = avatar_multipart(0, PNG_BYTES, "image/png");
    let uploaded = server
        .request_raw(
            "PUT",
            "/v1/me/profile/avatar",
            Some(&guest_token),
            Some("k-guest-avatar"),
            &format!("multipart/form-data; boundary={BOUNDARY}"),
            &body,
        )
        .await;
    assert_eq!(uploaded.status, 201, "{}", uploaded.json());

    // ③ **重启 Core**：名字与头像都是持久事实。
    server.stop().await;
    let server = Server::start(&db_path, &media_root).await;

    let after_restart = server
        .request("GET", "/v1/me/profile", Some(&guest_token), None, None)
        .await;
    assert_eq!(after_restart.status, 200, "{}", after_restart.json());
    assert_eq!(
        after_restart.json()["display_name"],
        "小乙 二号",
        "重启后名字还在（持久事实，不是进程内投影）"
    );
    assert_eq!(
        after_restart.json()["profile"]["avatar"]["content_type"],
        "image/png"
    );
    assert_eq!(
        after_restart.json()["profile"]["avatar"]["size_bytes"],
        PNG_BYTES.len() as i64
    );

    // ④ **别人看得见**：新人接受邀请者的关系申请 → 邀请者开一间私人房 →
    //    经房间成员头像路径读到的字节与上传的**逐字节相同**。
    let accepted = server
        .request(
            "POST",
            &format!("/v1/contacts/{owner_id}/accept"),
            Some(&guest_token),
            Some("k-accept"),
            None,
        )
        .await;
    assert!(
        accepted.status == 200 || accepted.status == 204,
        "accept: {} {}",
        accepted.status,
        accepted.json()
    );

    let room = server
        .request(
            "POST",
            "/v1/human-direct-rooms",
            Some(&owner_token),
            Some("k-room"),
            Some(&json!({"human_id": guest_id})),
        )
        .await;
    assert!(
        room.status == 200 || room.status == 201,
        "create room: {} {}",
        room.status,
        room.json()
    );
    // 建房响应的房间 id 嵌在 `room.id`（不是顶层 room_id）。
    let room_id = require_string(&room.json()["room"], "id");

    let bytes = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/members/{guest_id}/avatar"),
            Some(&owner_token),
            None,
            None,
        )
        .await;
    assert_eq!(bytes.status, 200, "read avatar bytes: {}", bytes.json());
    assert_eq!(
        bytes.body, PNG_BYTES,
        "★ 别人读到的字节必须与上传的逐字节相同（这是「别人看得见」的硬证据）"
    );
    assert_eq!(
        bytes
            .headers
            .get("content-type")
            .map(String::as_str)
            .unwrap_or(""),
        "image/png"
    );

    // ⑤ 反面：新人**改不到别人**（路径里没有"写谁的"）——他的改名只动了自己那一行。
    let owner_profile = server
        .request("GET", "/v1/me/profile", Some(&owner_token), None, None)
        .await;
    assert_eq!(owner_profile.json()["display_name"], "Owner");

    // ⑥ 没有凭据一律 401。
    let anonymous = server
        .request("GET", "/v1/me/profile", None, None, None)
        .await;
    assert_eq!(anonymous.status, 401, "{}", anonymous.json());

    server.stop().await;
}
