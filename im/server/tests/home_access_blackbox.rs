//! Home Access A 包真实进程黑盒：只通过 operator CLI 与公开 HTTP 验证
//! Home Identity、一次性 Pairing Grant 与独立 Human Session。成功裁决不读取
//! SQLite；每次测试使用临时数据库与空闲 loopback 端口，结束时回收进程与文件。

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::time::timeout;

const BIN: &str = env!("CARGO_BIN_EXE_celira-im");

struct Response {
    status: u16,
    body: Vec<u8>,
}

impl Response {
    fn json(&self) -> serde_json::Value {
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
        .map_err(|error| format!("connect failed: {error}"))?;
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
        .map_err(|error| error.to_string())?;
    if !body.is_empty() {
        stream
            .write_all(body)
            .await
            .map_err(|error| error.to_string())?;
    }
    stream.flush().await.map_err(|error| error.to_string())?;

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .await
        .map_err(|error| error.to_string())?;
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
        body: Option<&serde_json::Value>,
    ) -> Response {
        let mut headers = Vec::new();
        if let Some(token) = token {
            headers.push(("Authorization", format!("Bearer {token}")));
        }
        if body.is_some() {
            headers.push(("Content-Type", "application/json".to_owned()));
        }
        let header_refs: Vec<_> = headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let body = body.map(|value| value.to_string().into_bytes());
        raw_request(&self.addr, method, path, &header_refs, body.as_deref())
            .await
            .expect("HTTP request")
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

async fn cli_json(db_path: &Path, args: &[&str]) -> serde_json::Value {
    let output = Command::new(BIN)
        .args(args)
        .env("CELIRA_DB_PATH", db_path)
        .output()
        .await
        .expect("run operator CLI");
    assert!(
        output.status.success(),
        "operator CLI must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("operator CLI stdout must be one JSON response")
}

fn require_string(json: &serde_json::Value, field: &str) -> String {
    json[field]
        .as_str()
        .unwrap_or_else(|| panic!("missing string field {field}"))
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn home_access_pairing_is_single_use_and_survives_core_restart() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("core.db");
    let media_root = temp.path().join("media");
    let mut server = Server::start(&db_path, &media_root).await;

    // Home 不是匿名发现对象。
    let anonymous = server.request("GET", "/v1/home", None, None).await;
    assert_eq!(anonymous.status, 401);
    assert_eq!(anonymous.json()["error"]["code"], "authentication_required");

    let owner = cli_json(&db_path, &["bootstrap-human", "--display-name", "Owner"]).await;
    let owner_id = require_string(&owner, "participant_id");
    let owner_token = require_string(&owner, "token");

    let home = server
        .request("GET", "/v1/home", Some(&owner_token), None)
        .await;
    assert_eq!(home.status, 200);
    let home_id = require_string(&home.json(), "home_id");

    // Human Session 绝不能冒充 Pairing Grant；匿名兑换不走 Human 认证路径。
    let human_as_grant = server
        .request(
            "POST",
            "/v1/home/pairing-grants/exchange",
            Some(&owner_token),
            Some(&serde_json::json!({})),
        )
        .await;
    assert_eq!(human_as_grant.status, 401);
    assert_eq!(human_as_grant.json()["error"]["code"], "grant_invalid");

    // 仅 operator CLI 的首台远程设备路径：签发一次、stdout 才出现原始 grant。
    let grant = cli_json(
        &db_path,
        &[
            "issue-pairing-grant",
            "--human-id",
            &owner_id,
            "--device-label",
            "first-remote-host",
        ],
    )
    .await;
    let initial_grant = require_string(&grant, "token");
    let initial_grant_id = require_string(&grant, "grant_id");

    let exchanged = server
        .request(
            "POST",
            "/v1/home/pairing-grants/exchange",
            Some(&initial_grant),
            Some(&serde_json::json!({"device_label":"remote-host"})),
        )
        .await;
    assert_eq!(exchanged.status, 201);
    let exchanged_json = exchanged.json();
    assert_eq!(exchanged_json["home"]["home_id"], home_id);
    assert_eq!(exchanged_json["session"]["human_id"], owner_id);
    let paired_session_id = require_string(&exchanged_json["session"], "id");
    let paired_token = require_string(&exchanged_json, "token");
    assert_ne!(paired_token, initial_grant);

    // 成功后的重放只得到匿名统一错误，不创建第二个 Session。
    let replay = server
        .request(
            "POST",
            "/v1/home/pairing-grants/exchange",
            Some(&initial_grant),
            Some(&serde_json::json!({})),
        )
        .await;
    assert_eq!(replay.status, 401);
    assert_eq!(replay.json()["error"]["code"], "grant_invalid");

    // 新 Session 可独立认证；公开列表不泄露任一 token/hash。
    let paired_home = server
        .request("GET", "/v1/home", Some(&paired_token), None)
        .await;
    assert_eq!(paired_home.status, 200);
    assert_eq!(paired_home.json()["home_id"], home_id);
    let sessions = server
        .request("GET", "/v1/sessions", Some(&owner_token), None)
        .await;
    assert_eq!(sessions.status, 200);
    let sessions_json = sessions.json();
    assert_eq!(sessions_json["sessions"].as_array().unwrap().len(), 2);
    let paired = sessions_json["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == paired_session_id)
        .expect("paired session is listed");
    assert_eq!(paired["source_grant_id"], initial_grant_id);
    let public_sessions = sessions_json.to_string();
    assert!(!public_sessions.contains(&paired_token));
    assert!(!public_sessions.contains(&initial_grant));

    // HTTP 签发/撤销路径复用同一 grant 状态机，撤销后不可兑换。
    let http_grant = server
        .request(
            "POST",
            "/v1/home/pairing-grants",
            Some(&owner_token),
            Some(&serde_json::json!({"device_label":"revoked-device"})),
        )
        .await;
    assert_eq!(http_grant.status, 201);
    let http_grant_json = http_grant.json();
    let revoked_grant_id = require_string(&http_grant_json, "grant_id");
    let revoked_grant = require_string(&http_grant_json, "token");
    let grants = server
        .request("GET", "/v1/home/pairing-grants", Some(&owner_token), None)
        .await;
    assert_eq!(grants.status, 200);
    let public_grants = grants.json().to_string();
    assert!(public_grants.contains(&revoked_grant_id));
    assert!(
        !public_grants.contains(&revoked_grant),
        "grant list must never return raw grant token"
    );
    let revoke = server
        .request(
            "DELETE",
            &format!("/v1/home/pairing-grants/{revoked_grant_id}"),
            Some(&owner_token),
            None,
        )
        .await;
    assert_eq!(revoke.status, 204);
    let revoked_exchange = server
        .request(
            "POST",
            "/v1/home/pairing-grants/exchange",
            Some(&revoked_grant),
            Some(&serde_json::json!({})),
        )
        .await;
    assert_eq!(revoked_exchange.status, 401);
    assert_eq!(revoked_exchange.json()["error"]["code"], "grant_invalid");

    // 只撤销配对设备：原 owner Session 保持可用，设备 Session fail closed。
    let revoke_session = server
        .request(
            "DELETE",
            &format!("/v1/sessions/{paired_session_id}"),
            Some(&owner_token),
            None,
        )
        .await;
    assert_eq!(revoke_session.status, 204);
    let revoked_session_home = server
        .request("GET", "/v1/home", Some(&paired_token), None)
        .await;
    assert_eq!(revoked_session_home.status, 401);
    assert_eq!(
        revoked_session_home.json()["error"]["code"],
        "invalid_session"
    );
    let owner_still_works = server
        .request("GET", "/v1/home", Some(&owner_token), None)
        .await;
    assert_eq!(owner_still_works.status, 200);

    // 第二位 Human 只能看、撤销自己的 Session；未知/非本人统一 not-found。
    let outsider = cli_json(&db_path, &["create-human", "--display-name", "Outsider"]).await;
    let outsider_session_id = require_string(&outsider, "session_id");
    let outsider_token = require_string(&outsider, "token");
    let outsider_sessions = server
        .request("GET", "/v1/sessions", Some(&outsider_token), None)
        .await;
    assert_eq!(outsider_sessions.status, 200);
    assert_eq!(
        outsider_sessions.json()["sessions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let foreign_revoke = server
        .request(
            "DELETE",
            &format!("/v1/sessions/{paired_session_id}"),
            Some(&outsider_token),
            None,
        )
        .await;
    assert_eq!(foreign_revoke.status, 404);
    assert_eq!(foreign_revoke.json()["error"]["code"], "session_not_found");
    let outsider_own_session = server
        .request("GET", "/v1/home", Some(&outsider_token), None)
        .await;
    assert_eq!(outsider_own_session.status, 200);
    assert_eq!(outsider_own_session.json()["home_id"], home_id);
    assert_ne!(outsider_session_id, paired_session_id);

    // 同一数据库重启后 Home Identity 不变；不把 URL 当作身份。
    server.stop().await;
    let mut server = Server::start(&db_path, &media_root).await;
    let after_restart = server
        .request("GET", "/v1/home", Some(&owner_token), None)
        .await;
    assert_eq!(after_restart.status, 200);
    assert_eq!(after_restart.json()["home_id"], home_id);
    server.stop().await;
}
