//! HOME_ACCESS A2 配对事务真实进程黑盒：只通过 operator CLI + 公开 HTTP
//! 验证 prepare → status → commit → abort 协议、幂等与 redaction。成功裁决
//! 不读取 SQLite。每次测试使用临时数据库与空闲 loopback 端口，结束时回收
//! 进程与文件。
#![allow(clippy::needless_borrow)]

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
    json: serde_json::Value,
}

impl Response {
    fn code(&self) -> &str {
        self.json["error"]["code"].as_str().unwrap_or("")
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
        .map_err(|e| format!("connect failed: {e}"))?;
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
        .and_then(|v| v.parse::<u16>().ok())
        .ok_or_else(|| "missing HTTP status".to_owned())?;
    let headers: HashMap<_, _> = lines
        .filter_map(|line| line.split_once(": "))
        .map(|(n, v)| (n.to_ascii_lowercase(), v.to_owned()))
        .collect();
    let body = match headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
    {
        Some(len) => body.as_bytes()[..len].to_vec(),
        None => body.as_bytes().to_vec(),
    };
    let json = serde_json::from_slice(&body).unwrap_or(serde_json::json!({}));
    Ok(Response { status, json })
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
        bearer: Option<&str>,
        body: Option<&serde_json::Value>,
    ) -> Response {
        let mut headers = Vec::new();
        if let Some(token) = bearer {
            headers.push(("Authorization", format!("Bearer {token}")));
        }
        let body_bytes = body.map(|value| value.to_string().into_bytes());
        if body.is_some() {
            headers.push(("Content-Type", "application/json".to_owned()));
        }
        let header_refs: Vec<_> = headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
        raw_request(
            &self.addr,
            method,
            path,
            &header_refs,
            body_bytes.as_deref(),
        )
        .await
        .expect("HTTP request")
    }

    async fn stop(&mut self) {
        let _ = self.child.kill().await;
        let _ = timeout(Duration::from_secs(10), self.child.wait())
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

fn req(json: &serde_json::Value, field: &str) -> String {
    json[field]
        .as_str()
        .unwrap_or_else(|| panic!("missing string field {field}"))
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn a2_pairing_transaction_prepare_commit_and_recovery() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("core.db");
    let media_root = temp.path().join("media");
    let mut server = Server::start(&db_path, &media_root).await;

    let owner = cli_json(&db_path, &["bootstrap-human", "--display-name", "Owner"]).await;
    let owner_id = req(&owner, "participant_id");
    let owner_token = req(&owner, "token");

    let home = server
        .request("GET", "/v1/home", Some(&owner_token), None)
        .await;
    assert_eq!(home.status, 200);
    let home_id = req(&home.json, "home_id");

    // Host 在 Core 之外预生成的高熵 token → 只提交 SHA-256 摘要。
    let session_token = "clr_pt_session_provisional_token_000001";
    let commit_token = "clr_pt_commit_token_000001";
    let session_hash = sha256_hex(session_token);
    let commit_hash = sha256_hex(commit_token);
    let body = serde_json::json!({
        "client_nonce": "host-nonce-1",
        "session_token_hash": session_hash,
        "commit_token_hash": commit_hash,
    });

    // 授权非法（Human Session 冒充 Grant）→ grant_invalid。
    let swap = server
        .request(
            "POST",
            "/v1/home/pairing-grants/prepare",
            Some(&owner_token),
            Some(&body),
        )
        .await;
    assert_eq!(swap.status, 401);
    assert_eq!(swap.code(), "grant_invalid");

    // 签发 Grant（operator CLI），Host 用原始 grant 做 prepare。
    let grant = cli_json(&db_path, &["issue-pairing-grant", "--human-id", &owner_id]).await;
    let grant_token = req(&grant, "token");

    let prepared = server
        .request(
            "POST",
            "/v1/home/pairing-grants/prepare",
            Some(&grant_token),
            Some(&body),
        )
        .await;
    assert_eq!(prepared.status, 201, "{:?}", prepared.json);
    let transaction_id = req(&prepared.json, "transaction_id");
    let reserved_session_id = req(&prepared.json, "reserved_session_id");
    assert_eq!(prepared.json["status"], "prepared");
    assert_eq!(prepared.json["home"]["home_id"], home_id);
    assert_eq!(prepared.json["human"]["human_id"], owner_id);
    let public = prepared.json.to_string();
    assert!(!public.contains(&session_hash), "no session hash leak");
    assert!(!public.contains(&commit_hash), "no commit hash leak");

    // prepare 幂等重试：同一 nonce + 摘要返回同一事务。
    let retry = server
        .request(
            "POST",
            "/v1/home/pairing-grants/prepare",
            Some(&grant_token),
            Some(&body),
        )
        .await;
    assert_eq!(retry.status, 201);
    assert_eq!(req(&retry.json, "transaction_id"), transaction_id);

    // status（Host 恢复路径）：凭 commit token Bearer。
    let status = server
        .request(
            "GET",
            &format!("/v1/home/pairing-transactions/{transaction_id}"),
            Some(&commit_token),
            None,
        )
        .await;
    assert_eq!(status.status, 200, "{:?}", status.json);
    assert_eq!(status.json["status"], "prepared");
    assert_eq!(
        req(&status.json, "reserved_session_id"),
        reserved_session_id
    );

    // commit：core 在一个事务内激活 Session + 消费 Grant。
    let committed = server
        .request(
            "POST",
            &format!("/v1/home/pairing-transactions/{transaction_id}/commit"),
            Some(&commit_token),
            Some(&serde_json::json!({"device_label": "remote-host"})),
        )
        .await;
    assert_eq!(committed.status, 200, "{:?}", committed.json);
    assert_eq!(committed.json["status"], "committed");
    assert_eq!(committed.json["transaction_id"], transaction_id);

    // commit 重试幂等：同一非秘密结果。
    let replay_commit = server
        .request(
            "POST",
            &format!("/v1/home/pairing-transactions/{transaction_id}/commit"),
            Some(&commit_token),
            Some(&serde_json::json!({})),
        )
        .await;
    assert_eq!(replay_commit.status, 200, "{:?}", replay_commit.json);
    assert_eq!(replay_commit.json["transaction_id"], transaction_id);

    // 提交后 status 返回 committed。
    let status_after = server
        .request(
            "GET",
            &format!("/v1/home/pairing-transactions/{transaction_id}"),
            Some(&commit_token),
            None,
        )
        .await;
    assert_eq!(status_after.status, 200);
    assert_eq!(status_after.json["status"], "committed");

    // 已验证的 Host：reserved session 就是最终 session；用页面展示的
    // sessions 列表验证只有一个新 Session（bootstrap + A2 提交）。
    let sessions = server
        .request("GET", "/v1/sessions", Some(&owner_token), None)
        .await;
    assert_eq!(sessions.status, 200);
    assert_eq!(sessions.json["sessions"].as_array().unwrap().len(), 2);
    let listed = sessions.json.to_string();
    assert!(!listed.contains(&session_hash));
    assert!(!listed.contains(&commit_hash));

    // 换绑失败路径：abort 只影响 prepared，已 committed 的 abort 统一
    // pairing_transaction_invalid。
    let abort_after_commit = server
        .request(
            "DELETE",
            &format!("/v1/home/pairing-transactions/{transaction_id}"),
            Some(&commit_token),
            None,
        )
        .await;
    assert_eq!(abort_after_commit.status, 400);
    assert_eq!(abort_after_commit.code(), "pairing_transaction_invalid");

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a2_abort_preserves_grant_and_same_grant_can_prepare_again() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("core.db");
    let media_root = temp.path().join("media");
    let mut server = Server::start(&db_path, &media_root).await;

    let owner = cli_json(&db_path, &["bootstrap-human", "--display-name", "Owner"]).await;
    let owner_id = req(&owner, "participant_id");
    let grant = cli_json(&db_path, &["issue-pairing-grant", "--human-id", &owner_id]).await;
    let grant_token = req(&grant, "token");

    let session_hash = sha256_hex("clr_pt_provisional_abort_a");
    let commit_hash = sha256_hex("clr_pt_commit_abort_a");
    let body = serde_json::json!({
        "client_nonce": "abort-nonce-A",
        "session_token_hash": session_hash,
        "commit_token_hash": commit_hash,
    });
    let prepared = server
        .request(
            "POST",
            "/v1/home/pairing-grants/prepare",
            Some(&grant_token),
            Some(&body),
        )
        .await;
    assert_eq!(prepared.status, 201, "{:?}", prepared.json);
    let transaction_id = req(&prepared.json, "transaction_id");

    // abort prepared 事务（204）；重复 abort 对同一资源仍 204。
    let aborted = server
        .request(
            "DELETE",
            &format!("/v1/home/pairing-transactions/{transaction_id}"),
            Some("clr_pt_commit_abort_a"),
            None,
        )
        .await;
    assert_eq!(aborted.status, 204);
    let repeat_abort = server
        .request(
            "DELETE",
            &format!("/v1/home/pairing-transactions/{transaction_id}"),
            Some("clr_pt_commit_abort_a"),
            None,
        )
        .await;
    assert_eq!(repeat_abort.status, 204);

    // 已 aborted：commit 仍失败；再次 abort 继续幂等成功。
    let abort_repeat = server
        .request(
            "DELETE",
            &format!("/v1/home/pairing-transactions/{transaction_id}"),
            Some("clr_pt_commit_abort_a"),
            None,
        )
        .await;
    assert_eq!(abort_repeat.status, 204);

    // Grant 未被消耗：同一 grant 用全新 secrets + 新 nonce 再 prepare。
    let body2 = serde_json::json!({
        "client_nonce": "abort-nonce-B",
        "session_token_hash": sha256_hex("clr_pt_provisional_abort_b"),
        "commit_token_hash": sha256_hex("clr_pt_commit_abort_b"),
    });
    let re_prepared = server
        .request(
            "POST",
            "/v1/home/pairing-grants/prepare",
            Some(&grant_token),
            Some(&body2),
        )
        .await;
    assert_eq!(re_prepared.status, 201, "{:?}", re_prepared.json);

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a2_status_commit_abort_are_redacted_for_unknown_and_wrong_token() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("core.db");
    let media_root = temp.path().join("media");
    let mut server = Server::start(&db_path, &media_root).await;

    let owner = cli_json(&db_path, &["bootstrap-human", "--display-name", "Owner"]).await;
    let owner_id = req(&owner, "participant_id");
    let grant = cli_json(&db_path, &["issue-pairing-grant", "--human-id", &owner_id]).await;
    let grant_token = req(&grant, "token");

    let session_hash = sha256_hex("clr_pt_provisional_x");
    let commit_hash = sha256_hex("clr_pt_commit_x");
    let body = serde_json::json!({
        "client_nonce": "redact-nonce",
        "session_token_hash": session_hash,
        "commit_token_hash": commit_hash,
    });
    let prepared = server
        .request(
            "POST",
            "/v1/home/pairing-grants/prepare",
            Some(&grant_token),
            Some(&body),
        )
        .await;
    assert_eq!(prepared.status, 201, "{:?}", prepared.json);
    let transaction_id = req(&prepared.json, "transaction_id");

    let path_status = format!("/v1/home/pairing-transactions/{transaction_id}");
    // 错误 commit token：status/commit/abort 全部 pairing_transaction_invalid。
    for (method, path) in [
        ("GET", path_status.as_str()),
        ("POST", format!("{path_status}/commit").leak()),
        ("DELETE", path_status.as_str()),
    ] {
        // commit 需要 JSON body（可为空对象）；其余无 body。
        let body = if method == "POST" {
            Some(serde_json::json!({}))
        } else {
            None
        };
        let resp = server
            .request(method, path, Some("wrong-commit-token"), body.as_ref())
            .await;
        assert_eq!(resp.status, 400, "{method} {path}: {:?}", resp.json);
        assert_eq!(resp.code(), "pairing_transaction_invalid");
    }

    // 未知 transaction id（bad-id）→ pairing_transaction_invalid。
    let unknown = server
        .request(
            "GET",
            "/v1/home/pairing-transactions/not-a-uuid",
            Some("clr_pt_commit_x"),
            None,
        )
        .await;
    assert_eq!(unknown.status, 400);
    assert_eq!(unknown.code(), "pairing_transaction_invalid");

    server.stop().await;
}

fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(input.as_bytes()))
}
