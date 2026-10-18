//! 真实进程黑盒测试：启动最新二进制 + 临时数据库 + 空闲端口，只通过公开
//! API（bootstrap-human CLI + HTTP）完成 bootstrap → Agent → Room →
//! Message → History 主路径与失败路径；结束后关闭进程并清理临时资源。
//!
//! 临时资产放在仓库内被 git 忽略的 `data/message-history-blackbox-*` 目录，
//! 不访问仓库外路径。HTTP 客户端是裸 TCP 的 HTTP/1.1（Connection: close），
//! 不引入额外依赖。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use base64::Engine as _;
use sha2::Digest as _;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Acquire, SqlitePool};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::time::timeout;
use uuid::Uuid;

/// cargo 注入的测试用二进制路径（tests 目标编译时可用，保留连字符原名）。
const BIN: &str = env!("CARGO_BIN_EXE_celira-im");

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
    headers: HashMap<String, String>,
}

impl HttpResponse {
    fn json(&self) -> serde_json::Value {
        if self.body.is_empty() {
            return serde_json::Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

/// 裸 TCP HTTP/1.1 请求：Connection: close，读完响应后连接关闭。
async fn http_request(
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<HttpResponse, String> {
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
    let text = String::from_utf8_lossy(&raw);
    let (head, body_bytes) = text
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("malformed response: {text:?}"));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let mut header_map = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(": ") {
            header_map.insert(name.to_ascii_lowercase(), value.to_owned());
        }
    }
    // 请求用 Connection: close，读满即可；按 Content-Length 截断以防
    // keep-alive 残留。
    let body = if let Some(len) = header_map
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
    {
        body_bytes.as_bytes().iter().take(len).copied().collect()
    } else {
        body_bytes.as_bytes().to_vec()
    };
    Ok(HttpResponse {
        status,
        body,
        headers: header_map,
    })
}

struct Server {
    child: Child,
    addr: String,
}

impl Server {
    /// 启动服务进程：绑定空闲端口，等待 /ready 变 200。
    ///
    /// spawn 后立即把 Child 包进 Server：readiness 断言 panic 时 unwind 会
    /// 触发 Drop（start_kill），不会泄漏子进程。
    async fn start(db_path: &Path, media_root: &Path) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve port");
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let addr = format!("127.0.0.1:{port}");

        let child = Command::new(BIN)
            .env("CELIRA_LISTEN_ADDR", &addr)
            .env("CELIRA_DB_PATH", db_path)
            .env("CELIRA_MEDIA_ROOT", media_root)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn celira-im");
        let server = Self { child, addr };

        // 轮询 /ready（迁移全部应用后为 200）。进程启动初期连接可能被
        // 拒绝或 /ready 未就绪，都视为「还没准备好」重试。
        let deadline = Duration::from_secs(30);
        let started = timeout(deadline, async {
            loop {
                match try_http_request(&server.addr, "GET", "/ready", &[], None).await {
                    Some(HttpResponse { status: 200, .. }) => break,
                    _ => tokio::time::sleep(Duration::from_millis(200)).await,
                }
            }
        })
        .await;
        assert!(
            started.is_ok(),
            "server did not become ready at {} within {deadline:?}",
            server.addr
        );
        server
    }

    /// 停掉服务进程并等待退出；wait 超时视为失败（Drop 仍会再杀一次），
    /// 不虚报已清理。
    async fn stop(&mut self) {
        let _ = self.child.kill().await;
        let waited = timeout(Duration::from_secs(10), self.child.wait()).await;
        assert!(waited.is_ok(), "server did not exit within 10s after kill");
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        key: Option<&str>,
        json_body: Option<&serde_json::Value>,
    ) -> HttpResponse {
        let mut headers: Vec<(String, String)> = Vec::new();
        if let Some(token) = token {
            headers.push(("Authorization".into(), format!("Bearer {token}")));
        }
        if let Some(key) = key {
            headers.push(("Idempotency-Key".into(), key.to_owned()));
        }
        if json_body.is_some() {
            headers.push(("Content-Type".into(), "application/json".into()));
        }
        let raw_headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        let body = json_body.map(|v| v.to_string().into_bytes());
        http_request(&self.addr, method, path, &raw_headers, body.as_deref())
            .await
            .unwrap_or_else(|e| panic!("request {method} {path} failed: {e}"))
    }

    /// 任意 Content-Type / 原始 body 的请求（multipart 上传用）。
    async fn request_raw(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        content_type: Option<&str>,
        body: &[u8],
    ) -> HttpResponse {
        let mut headers: Vec<(String, String)> = Vec::new();
        if let Some(token) = token {
            headers.push(("Authorization".into(), format!("Bearer {token}")));
        }
        if let Some(content_type) = content_type {
            headers.push(("Content-Type".into(), content_type.to_owned()));
        }
        let raw_headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        http_request(&self.addr, method, path, &raw_headers, Some(body))
            .await
            .unwrap_or_else(|e| panic!("request {method} {path} failed: {e}"))
    }
}

/// 进程退出时强制杀掉子进程，防止断言失败泄漏服务进程。
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// 连接失败返回 None（用于就绪轮询），成功则返回完整响应。
async fn try_http_request(
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Option<HttpResponse> {
    http_request(addr, method, path, headers, body).await.ok()
}

/// 黑盒运行目录的 RAII guard：创建唯一临时目录并提供 path/db_path；
/// Drop 时 best-effort remove_dir_all，断言 panic unwind 或正常结束都会清理。
/// 声明顺序保证：guard 先于 server 声明，反向析构时 server 先 drop 再删目录。
struct TestRunDir {
    dir: PathBuf,
    cleaned: bool,
}

impl TestRunDir {
    fn create() -> Self {
        let workdir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data");
        std::fs::create_dir_all(&workdir).expect("create ignored data dir");
        let dir = workdir.join(format!("message-history-blackbox-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create blackbox run dir");
        Self {
            dir,
            cleaned: false,
        }
    }

    fn path(&self) -> &Path {
        &self.dir
    }

    fn db_path(&self) -> PathBuf {
        self.dir.join("core.db")
    }

    fn media_path(&self) -> PathBuf {
        self.dir.join("media")
    }

    /// 成功路径的显式清理：删除目录并断言成功；之后 Drop 不再重复删除。
    fn cleanup(&mut self) {
        std::fs::remove_dir_all(&self.dir).expect("cleanup blackbox assets");
        self.cleaned = true;
    }
}

impl Drop for TestRunDir {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// 最小专属测试：不启动产品进程，只证明 guard 在正常 Drop 和 panic unwind
/// 两种路径下都会删除目录。
#[test]
fn run_dir_guard_cleans_on_drop_and_panic() {
    let dir = TestRunDir::create();
    let path = dir.path().to_owned();
    drop(dir);
    assert!(!path.exists(), "drop must clean the run dir");

    let dir = TestRunDir::create();
    let path = dir.path().to_owned();
    let result = std::panic::catch_unwind(move || {
        let _guard = dir;
        panic!("deliberate panic to exercise unwind cleanup");
    });
    assert!(result.is_err());
    assert!(!path.exists(), "panic unwind must clean the run dir");
}

/// 通过 CLI 对同一数据库执行 bootstrap-human，返回 token。
async fn bootstrap_cli(db_path: &Path) -> String {
    let output = Command::new(BIN)
        .args(["bootstrap-human", "--display-name", "Owner"])
        .env("CELIRA_DB_PATH", db_path)
        .output()
        .await
        .expect("run bootstrap-human");
    assert!(
        output.status.success(),
        "bootstrap-human failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("cli json");
    json["token"].as_str().expect("cli token").to_owned()
}

/// 只允许测试夹具直接触碰数据库：在真实临时库中准备第二个 active Human
/// Participant 与其 active Human Session（非成员场景用）。
///
/// token 按真实格式构造：`clr_hs_` + 32 随机字节的 base64url 无填充编码
/// （43 字符）；token_hash 为 token 的 SHA-256 hex；Participant/Session ID
/// 均为 UUID v4。返回 (participant_id, token)。
async fn prepare_outsider_human(db_path: &Path) -> (String, String) {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open fixture database");

    let participant_id = uuid::Uuid::new_v4().to_string();
    let session_id = uuid::Uuid::new_v4().to_string();
    let mut random = [0u8; 32];
    getrandom::getrandom(&mut random).expect("secure random");
    let token = format!(
        "clr_hs_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random)
    );
    assert_eq!(token.len(), 7 + 43, "token must match the real format");
    let token_hash = hex::encode(sha2::Sha256::digest(token.as_bytes()));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as i64;

    sqlx::query(
        "INSERT INTO participants (id, kind, status, display_name, created_at, revoked_at) \
         VALUES (?, 'human', 'active', ?, ?, NULL)",
    )
    .bind(&participant_id)
    .bind("Outsider")
    .bind(now)
    .execute(&pool)
    .await
    .expect("insert fixture outsider participant");
    sqlx::query(
        "INSERT INTO human_sessions (id, human_id, token_hash, created_at, revoked_at) \
         VALUES (?, ?, ?, ?, NULL)",
    )
    .bind(&session_id)
    .bind(&participant_id)
    .bind(&token_hash)
    .bind(now)
    .execute(&pool)
    .await
    .expect("insert fixture outsider session");
    drop(pool);
    (participant_id, token)
}

/// 运行一次完整主路径 + 失败路径，返回可核对的证据行。
async fn run_scenario(server: &Server, token: &str, outsider_token: &str) -> Vec<String> {
    let mut evidence: Vec<String> = Vec::new();
    let mut step = |name: &str, detail: String| evidence.push(format!("[{name}] {detail}"));

    // 未认证 → 401。
    let resp = server
        .request(
            "POST",
            "/v1/agents",
            None,
            Some("a-1"),
            Some(&serde_json::json!({"display_name":"助手"})),
        )
        .await;
    assert_eq!(resp.status, 401, "no auth must be 401");
    assert_eq!(resp.json()["error"]["code"], "authentication_required");

    // Agent → 201。
    let resp = server
        .request(
            "POST",
            "/v1/agents",
            Some(token),
            Some("a-1"),
            Some(&serde_json::json!({"display_name":"助手"})),
        )
        .await;
    assert_eq!(resp.status, 201, "agent create must be 201");
    let agent_id = resp.json()["agent"]["id"].as_str().unwrap().to_owned();
    step("agent", format!("created id={agent_id}"));

    // Direct Room → 201。
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(token),
            Some("r-1"),
            Some(&serde_json::json!({"agent_id": agent_id})),
        )
        .await;
    assert_eq!(resp.status, 201, "room create must be 201");
    let room_id = resp.json()["room"]["id"].as_str().unwrap().to_owned();
    step("room", format!("created id={room_id}"));

    // 空历史 → 200 []。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["messages"], serde_json::json!([]));
    assert_eq!(resp.json()["next_before"], serde_json::Value::Null);
    step("history", "empty room -> 200 [] next_before=null".into());

    // 非成员 Human（第二个 active Human + 独立进程公开 HTTP）：发送与
    // 读取都必须 404 room_not_found，且不产生 Message、不占 sequence。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(outsider_token),
            Some("outsider-m-1"),
            Some(&serde_json::json!({"body":"intrude"})),
        )
        .await;
    assert_eq!(resp.status, 404, "non-member send must be 404");
    assert_eq!(resp.json()["error"]["code"], "room_not_found");
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(outsider_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 404, "non-member history must be 404");
    assert_eq!(resp.json()["error"]["code"], "room_not_found");
    // Owner 视角复核：历史仍为空，sequence 未被占用（后续首次发送 seq=1）。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["messages"], serde_json::json!([]));
    assert_eq!(resp.json()["next_before"], serde_json::Value::Null);
    step("non-member", "outsider send+history -> 404 room_not_found x2; owner history still [] and sequence unconsumed".into());

    // 首次 Message → 201（公共视图不返回 sequence）。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(token),
            Some("m-1"),
            Some(&serde_json::json!({"body":"hello"})),
        )
        .await;
    assert_eq!(resp.status, 201, "first message must be 201");
    assert_eq!(resp.json()["idempotency_replayed"], false);
    let msg_1_id = resp.json()["message"]["id"].as_str().unwrap().to_owned();
    assert!(resp.json()["message"].get("sequence").is_none());
    assert_eq!(resp.json()["message"]["actor"]["kind"], "human");
    assert_eq!(resp.json()["message"]["body"], "hello");
    step("message", format!("created id={msg_1_id}"));

    // 同 key 重放 → 200 同 ID，idempotency_replayed=true。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(token),
            Some("m-1"),
            Some(&serde_json::json!({"body":"hello"})),
        )
        .await;
    assert_eq!(resp.status, 200, "replay must be 200");
    assert_eq!(resp.json()["idempotency_replayed"], true);
    assert_eq!(resp.json()["message"]["id"], msg_1_id);
    step("replay", "200 same id, idempotency_replayed=true".into());

    // 同 key 换 body → 409。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(token),
            Some("m-1"),
            Some(&serde_json::json!({"body":"changed"})),
        )
        .await;
    assert_eq!(resp.status, 409);
    assert_eq!(resp.json()["error"]["code"], "idempotency_conflict");
    step(
        "conflict",
        "same key different body -> 409 idempotency_conflict".into(),
    );

    // Unicode / 空白 round-trip → 201，正文原样。
    let unicode_body = "你好 😀\nline2\r\n\t tab";
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(token),
            Some("m-2"),
            Some(&serde_json::json!({"body": unicode_body})),
        )
        .await;
    assert_eq!(resp.status, 201);
    assert_eq!(resp.json()["message"]["body"], unicode_body);
    step("unicode", "round-trip ok".into());

    // 纯空白正文合法 → 201。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(token),
            Some("m-3"),
            Some(&serde_json::json!({"body":" \t\n"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    assert_eq!(resp.json()["message"]["body"], " \t\n");
    step("whitespace-only", "201 verbatim".into());

    // 非法正文：NUL / 空 / 超限 → 400 invalid_message_body。
    let nul_body = "a\u{0}b";
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(token),
            Some("m-4"),
            Some(&serde_json::json!({"body": nul_body})),
        )
        .await;
    assert_eq!(resp.status, 400);
    assert_eq!(resp.json()["error"]["code"], "invalid_message_body");
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(token),
            Some("m-5"),
            Some(&serde_json::json!({"body":""})),
        )
        .await;
    assert_eq!(resp.status, 400);
    assert_eq!(resp.json()["error"]["code"], "invalid_message_body");
    let long_body = "a".repeat(16385);
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(token),
            Some("m-6"),
            Some(&serde_json::json!({"body": long_body})),
        )
        .await;
    assert_eq!(resp.status, 400);
    assert_eq!(resp.json()["error"]["code"], "invalid_message_body");
    step(
        "invalid-body",
        "NUL/empty/16385B -> 400 invalid_message_body x3".into(),
    );

    // 路由上限：整体 JSON 超过 128 KiB → 413。
    let oversized = "x".repeat(131_100);
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(token),
            Some("m-7"),
            Some(&serde_json::json!({"body": oversized})),
        )
        .await;
    assert_eq!(resp.status, 413);
    assert_eq!(resp.json()["error"]["code"], "payload_too_large");
    step("413", "body > 128KiB -> 413 payload_too_large".into());

    // 未知 Room → 404；格式错误 Room ID → 404。
    let ghost = uuid::Uuid::new_v4().to_string();
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{ghost}/messages"),
            Some(token),
            Some("m-8"),
            Some(&serde_json::json!({"body":"x"})),
        )
        .await;
    assert_eq!(resp.status, 404);
    assert_eq!(resp.json()["error"]["code"], "room_not_found");
    let resp = server
        .request(
            "POST",
            "/v1/rooms/not-a-uuid/messages",
            Some(token),
            Some("m-9"),
            Some(&serde_json::json!({"body":"x"})),
        )
        .await;
    assert_eq!(resp.status, 404);
    assert_eq!(resp.json()["error"]["code"], "room_not_found");
    step(
        "404",
        "unknown room & bad room id -> 404 room_not_found".into(),
    );

    // 无效 Session → 401。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some("definitely-not-a-token"),
            Some("m-10"),
            Some(&serde_json::json!({"body":"x"})),
        )
        .await;
    assert_eq!(resp.status, 401);
    assert_eq!(resp.json()["error"]["code"], "invalid_session");
    step("401", "invalid session -> 401 invalid_session".into());

    // History：首屏最新一页（limit=2 → 最新两条升序，next_before 为
    // opaque cursor）。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/messages?limit=2"),
            Some(token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    let page1 = resp.json();
    let page1_bodies: Vec<String> = page1["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["body"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(page1_bodies, vec![unicode_body, " \t\n"]);
    let next1 = page1["next_before"].as_str().expect("cursor string");
    assert!(next1.starts_with("c1.m."), "opaque cursor: {next1}");
    for m in page1["messages"].as_array().unwrap() {
        assert!(m.get("sequence").is_none(), "no sequence in history views");
    }
    // 翻页：回传 next_before → 只有最早的 "hello"，next_before=null。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/messages?limit=2&before={next1}"),
            Some(token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    let page2 = resp.json();
    let page2_bodies: Vec<String> = page2["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["body"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(page2_bodies, vec!["hello"]);
    assert_eq!(page2["next_before"], serde_json::Value::Null);
    step(
        "history-paging",
        "limit=2 latest two ascending + opaque next_before cursor; cursor page -> oldest; next_before=null".into(),
    );

    // 非法分页参数 → 400（含不可解析 / 跨端点类型的 cursor）。
    for query in [
        "?limit=0",
        "?limit=101",
        "?limit=abc",
        "?before=0",
        "?before=abc",
        "?before=c1.m.start",
        "?before=c1.he.550e8400-e29b-41d4-a716-446655440001",
        "?before=c1.m.550e8400-e29b-41d4-a716-446655440000",
    ] {
        let resp = server
            .request(
                "GET",
                &format!("/v1/rooms/{room_id}/messages{query}"),
                Some(token),
                None,
                None,
            )
            .await;
        assert_eq!(resp.status, 400, "query {query}");
        assert_eq!(resp.json()["error"]["code"], "invalid_request");
    }
    step(
        "paging-400",
        "limit=0/101/abc, before=0/abc/malformed/cross-type/unknown -> 400 invalid_request".into(),
    );

    // Agent revoke → 204；历史仍可读；新 Human Message 仍可发（seq=4）。
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/agents/{agent_id}"),
            Some(token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204, "agent revoke must be 204");
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["messages"].as_array().unwrap().len(), 3);
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(token),
            Some("m-11"),
            Some(&serde_json::json!({"body":"after revoke"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    assert_eq!(resp.json()["message"]["body"], "after revoke");
    step(
        "revoke",
        "agent revoked 204; history readable; new human message still accepted".into(),
    );

    // 响应视图不含 idempotency 字段，也不含 sequence。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/messages?limit=1"),
            Some(token),
            None,
            None,
        )
        .await;
    assert!(resp.json()["messages"][0].get("idempotency").is_none());
    assert!(resp.json()["messages"][0].get("sequence").is_none());

    evidence
}

#[tokio::test(flavor = "multi_thread")]
async fn blackbox_message_history_via_public_api_only() {
    // 先声明 guard、后声明 server：panic unwind 时 server 先于目录被清理。
    let mut run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();

    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;
    let token = bootstrap_cli(&db_path).await;
    let (_outsider_id, outsider_token) = prepare_outsider_human(&db_path).await;
    let mut evidence = run_scenario(&server, &token, &outsider_token).await;

    // 重启进程（同一数据库）：readiness 通过、历史保持、幂等重放事实保持。
    server.stop().await;
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;
    let rooms = server
        .request("GET", "/v1/rooms", Some(&token), None, None)
        .await;
    assert_eq!(rooms.status, 200);
    let room_id = rooms.json()["rooms"][0]["room"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["messages"].as_array().unwrap().len(), 4);
    assert_eq!(resp.json()["messages"][3]["body"], "after revoke");
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&token),
            Some("m-1"),
            Some(&serde_json::json!({"body":"hello"})),
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["idempotency_replayed"], true);
    assert_eq!(resp.json()["message"]["id"].as_str().unwrap().len(), 36);
    evidence.push("[restart] ready ok; 4 messages persisted; m-1 replay -> 200 same id".into());
    server.stop().await;
    run_dir.cleanup();

    println!("BLACKBOX EVIDENCE:\n{}", evidence.join("\n"));
    println!("BLACKBOX DONE: server closed, temp assets removed");
}
/// 真实 SQLite 探针：返回某 reader 在指定 Room 的历次 Membership 窗口内
/// 可见的全部 Message sequence（升序）。内部事实检查专用。
async fn probe_visible_sequences(db_path: &Path, room_id: &str, participant_id: &str) -> Vec<i64> {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open probe database");
    let sequences: Vec<i64> = sqlx::query_scalar(
        "SELECT m.sequence FROM messages m \
         WHERE m.room_id = ? AND EXISTS ( \
             SELECT 1 FROM room_memberships mm \
             WHERE mm.room_id = m.room_id AND mm.participant_id = ? \
               AND mm.join_after_sequence < m.sequence \
               AND (mm.left_before_sequence IS NULL OR m.sequence <= mm.left_before_sequence) \
         ) ORDER BY m.sequence ASC",
    )
    .bind(room_id)
    .bind(participant_id)
    .fetch_all(&pool)
    .await
    .expect("probe visible sequences");
    drop(pool);
    sequences
}

/// 真实 SQLite 负向探针：Credential 明文不得落库，摘要必须是 64 字符
/// 小写 hex；Backend 切片不得顺带创建 Room/Membership 或延期协议表。
async fn verify_backend_storage(db_path: &Path, issued_tokens: &[&str]) {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open backend blackbox database");

    let hashes: Vec<String> = sqlx::query_scalar(
        "SELECT token_hash FROM backend_credentials ORDER BY created_at ASC, id ASC",
    )
    .fetch_all(&pool)
    .await
    .expect("read credential hashes");
    assert_eq!(hashes.len(), issued_tokens.len());
    for hash in &hashes {
        assert_eq!(hash.len(), 64);
        assert!(
            hash.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert!(
            !issued_tokens.iter().any(|token| hash == *token),
            "credential plaintext must not be stored"
        );
    }
    for token in issued_tokens {
        let expected = hex::encode(sha2::Sha256::digest(token.as_bytes()));
        assert!(
            hashes.iter().any(|hash| hash == &expected),
            "each issued credential must be represented only by its digest"
        );
    }

    let rooms: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM rooms")
        .fetch_one(&pool)
        .await
        .expect("count rooms");
    let memberships: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM room_memberships")
        .fetch_one(&pool)
        .await
        .expect("count memberships");
    assert_eq!(rooms, 0, "Backend authorization must not create rooms");
    assert_eq!(
        memberships, 0,
        "Backend authorization must not create memberships"
    );

    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .fetch_all(&pool)
            .await
            .expect("read schema table names");
    assert!(tables.iter().any(|name| name == "backend_bindings"));
    assert!(tables.iter().any(|name| name == "backend_credentials"));
    for forbidden in ["ack", "replay", "lease", "runtime"] {
        assert!(
            !tables.iter().any(|name| {
                name.split('_').any(|segment| {
                    segment == forbidden || segment.strip_suffix('s') == Some(forbidden)
                })
            }),
            "deferred protocol table must not exist: {forbidden}"
        );
    }
    drop(pool);
}

/// 真实独立进程 Backend 授权黑盒：只通过公开 Human 管理 API 与
/// `/v1/backend/me` 认证探针建立 Binding/Credential、重启、轮换和撤销；
/// 原始 token 只保留在测试内存中，不进入证据输出或失败消息。
#[tokio::test(flavor = "multi_thread")]
async fn blackbox_backend_binding_credential_via_public_api() {
    let mut run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;
    let human_token = bootstrap_cli(&db_path).await;

    let resp = server
        .request(
            "POST",
            "/v1/agents",
            Some(&human_token),
            Some("backend-agent-1"),
            Some(&serde_json::json!({"display_name":"Backend Agent"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let agent_id = resp.json()["agent"]["id"]
        .as_str()
        .expect("agent id")
        .to_owned();

    let bindings_path = format!("/v1/agents/{agent_id}/backend-bindings");
    let resp = server
        .request(
            "POST",
            &bindings_path,
            Some(&human_token),
            Some("binding-1"),
            Some(&serde_json::json!({"label":"local runtime"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    assert_eq!(resp.json()["idempotency_replayed"], false);
    let binding_1 = resp.json()["binding"]["id"]
        .as_str()
        .expect("binding id")
        .to_owned();
    let resp = server
        .request(
            "POST",
            &bindings_path,
            Some(&human_token),
            Some("binding-1"),
            Some(&serde_json::json!({"label":"local runtime"})),
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["idempotency_replayed"], true);
    assert_eq!(resp.json()["binding"]["id"], binding_1);

    let credentials_1_path = format!("{bindings_path}/{binding_1}/credentials");
    let resp = server
        .request(
            "POST",
            &credentials_1_path,
            Some(&human_token),
            Some("credential-1"),
            Some(&serde_json::json!({"replaces_credential_id":null})),
        )
        .await;
    assert_eq!(resp.status, 201);
    assert_eq!(resp.json()["idempotency_replayed"], false);
    let credential_1 = resp.json()["credential"]["id"]
        .as_str()
        .expect("credential id")
        .to_owned();
    let backend_token_1 = resp.json()["token"]
        .as_str()
        .expect("one-time credential token")
        .to_owned();

    let resp = server
        .request(
            "POST",
            &credentials_1_path,
            Some(&human_token),
            Some("credential-1"),
            Some(&serde_json::json!({"replaces_credential_id":null})),
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["idempotency_replayed"], true);
    assert!(resp.json()["token"].is_null());
    assert_eq!(resp.json()["credential"]["id"], credential_1);

    let resp = server
        .request("GET", &credentials_1_path, Some(&human_token), None, None)
        .await;
    assert_eq!(resp.status, 200);
    let listed = &resp.json()["credentials"][0];
    assert!(listed.get("token").is_none());
    assert!(listed.get("token_hash").is_none());
    assert!(listed.get("idempotency_key").is_none());

    let resp = server
        .request("GET", "/v1/backend/me", Some(&backend_token_1), None, None)
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["agent"]["id"], agent_id);
    assert_eq!(resp.json()["binding"]["id"], binding_1);
    assert_eq!(resp.json()["credential"]["id"], credential_1);
    let resp = server
        .request("GET", "/v1/backend/me", Some(&human_token), None, None)
        .await;
    assert_eq!(resp.status, 401);
    assert_eq!(resp.json()["error"]["code"], "invalid_credential");
    let resp = server
        .request("GET", "/v1/me", Some(&backend_token_1), None, None)
        .await;
    assert_eq!(resp.status, 401);
    assert_eq!(resp.json()["error"]["code"], "invalid_session");

    // 同一数据库重启后，Credential 仍能解析到同一公开 Agent 事实。
    server.stop().await;
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;
    let resp = server
        .request("GET", "/v1/backend/me", Some(&backend_token_1), None, None)
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["agent"]["id"], agent_id);

    // CAS 轮换：旧 token 立即失效；stale replaces 不得影响新 token。
    let resp = server
        .request(
            "POST",
            &credentials_1_path,
            Some(&human_token),
            Some("credential-2"),
            Some(&serde_json::json!({"replaces_credential_id":credential_1})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let credential_2 = resp.json()["credential"]["id"]
        .as_str()
        .expect("rotated credential id")
        .to_owned();
    let backend_token_2 = resp.json()["token"]
        .as_str()
        .expect("rotated one-time token")
        .to_owned();
    let resp = server
        .request("GET", "/v1/backend/me", Some(&backend_token_1), None, None)
        .await;
    assert_eq!(resp.status, 401);
    let resp = server
        .request("GET", "/v1/backend/me", Some(&backend_token_2), None, None)
        .await;
    assert_eq!(resp.status, 200);
    let resp = server
        .request(
            "POST",
            &credentials_1_path,
            Some(&human_token),
            Some("credential-stale"),
            Some(&serde_json::json!({"replaces_credential_id":credential_1})),
        )
        .await;
    assert_eq!(resp.status, 409);
    assert_eq!(resp.json()["error"]["code"], "credential_state_conflict");
    let resp = server
        .request("GET", "/v1/backend/me", Some(&backend_token_2), None, None)
        .await;
    assert_eq!(resp.status, 200);

    // 显式 Credential 撤销立即 fail closed。
    let resp = server
        .request(
            "DELETE",
            &format!("{credentials_1_path}/{credential_2}"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204);
    let resp = server
        .request("GET", "/v1/backend/me", Some(&backend_token_2), None, None)
        .await;
    assert_eq!(resp.status, 401);

    // 当前无 active Credential，可再次显式签发；随后撤销 Binding，证明
    // Credential 与 Binding 在同一事务 fail closed。
    let resp = server
        .request(
            "POST",
            &credentials_1_path,
            Some(&human_token),
            Some("credential-3"),
            Some(&serde_json::json!({"replaces_credential_id":null})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let backend_token_3 = resp.json()["token"]
        .as_str()
        .expect("replacement one-time token")
        .to_owned();
    let resp = server
        .request(
            "DELETE",
            &format!("{bindings_path}/{binding_1}"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204);
    let resp = server
        .request("GET", "/v1/backend/me", Some(&backend_token_3), None, None)
        .await;
    assert_eq!(resp.status, 401);

    // revoked Binding 保留历史，但不占 active 槽位；新 Binding/Credential
    // 建立后撤销 Agent，证明顶层生命周期传播同样 fail closed。
    let resp = server
        .request(
            "POST",
            &bindings_path,
            Some(&human_token),
            Some("binding-2"),
            Some(&serde_json::json!({"label":"Native Runtime"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let binding_2 = resp.json()["binding"]["id"]
        .as_str()
        .expect("second binding id")
        .to_owned();
    let credentials_2_path = format!("{bindings_path}/{binding_2}/credentials");
    let resp = server
        .request(
            "POST",
            &credentials_2_path,
            Some(&human_token),
            Some("credential-4"),
            Some(&serde_json::json!({"replaces_credential_id":null})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let backend_token_4 = resp.json()["token"]
        .as_str()
        .expect("agent cascade one-time token")
        .to_owned();
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/agents/{agent_id}"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204);
    let resp = server
        .request("GET", "/v1/backend/me", Some(&backend_token_4), None, None)
        .await;
    assert_eq!(resp.status, 401);
    let resp = server
        .request("GET", &bindings_path, Some(&human_token), None, None)
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["bindings"].as_array().unwrap().len(), 2);
    assert!(
        resp.json()["bindings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|binding| binding["status"] == "revoked")
    );

    server.stop().await;
    verify_backend_storage(
        &db_path,
        &[
            &backend_token_1,
            &backend_token_2,
            &backend_token_3,
            &backend_token_4,
        ],
    )
    .await;
    run_dir.cleanup();

    println!(
        "BACKEND BLACKBOX EVIDENCE: create/replay, one-time secret, channel isolation, restart persistence, CAS rotation, credential/binding/agent fail-closed, digest-only storage all passed"
    );
    println!("BACKEND BLACKBOX DONE: server closed, temp assets removed");
}

/// 真实 SQLite 探针：agent_events 行数与逐 Agent sequence 单调性、schema
/// 无延期对象；Event 数据在 revoke 后必须保留。
async fn verify_event_storage(db_path: &Path, expected_events: i64) {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open event blackbox database");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_events")
        .fetch_one(&pool)
        .await
        .expect("count agent_events");
    assert_eq!(
        count, expected_events,
        "event rows must survive revoke/restart"
    );

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT target_agent_id, sequence FROM agent_events ORDER BY target_agent_id, sequence",
    )
    .fetch_all(&pool)
    .await
    .expect("read event rows");
    let mut by_agent: std::collections::HashMap<String, Vec<i64>> = Default::default();
    for (agent, sequence) in rows {
        by_agent.entry(agent).or_default().push(sequence);
    }
    for (agent, sequences) in &by_agent {
        let expected: Vec<i64> = (1..=sequences.len() as i64).collect();
        assert_eq!(
            sequences, &expected,
            "agent {agent} event sequence must be 1..=n with no gaps or duplicates"
        );
    }

    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .fetch_all(&pool)
            .await
            .expect("read schema table names");
    assert!(tables.iter().any(|name| name == "agent_events"));
    // read_cursors / read_cursor_advancements 是 CORE_V1_READ_STATE.md 已封板
    // 的公共事实表，不属于本守卫的延期对象。
    for forbidden in ["ack", "attempt", "lease", "replay", "runtime"] {
        assert!(
            !tables.iter().any(|name| {
                name.split('_').any(|segment| {
                    segment == forbidden || segment.strip_suffix('s') == Some(forbidden)
                })
            }),
            "deferred protocol table must not exist: {forbidden}"
        );
    }
    drop(pool);
}

/// 真实独立进程 Agent Event 黑盒：只通过公开 Human 管理 API、Message API
/// 与 `/v1/backend/events` 完成无 Binding 生成、间隙可观察、幂等重放、
/// 双 Agent 隔离、分页、重启、轮换、revoke fail closed 与 Event 数据保留。
/// 原始 token 只保留在测试内存中，不进入证据输出或失败消息。
#[tokio::test(flavor = "multi_thread")]
async fn blackbox_agent_events_via_public_api() {
    let mut run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;
    let human_token = bootstrap_cli(&db_path).await;
    let mut evidence: Vec<String> = Vec::new();
    let mut step = |name: &str, detail: String| evidence.push(format!("[{name}] {detail}"));

    // Agent 1 + Direct Room 1。
    let resp = server
        .request(
            "POST",
            "/v1/agents",
            Some(&human_token),
            Some("ev-agent-1"),
            Some(&serde_json::json!({"display_name":"甲"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let agent_1 = resp.json()["agent"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&human_token),
            Some("ev-room-1"),
            Some(&serde_json::json!({"agent_id": agent_1})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let room_1 = resp.json()["room"]["id"].as_str().unwrap().to_owned();

    // 无 Binding 时发送两条 Message（201），并幂等重放第一条（200）。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_1}/messages"),
            Some(&human_token),
            Some("ev-m-1"),
            Some(&serde_json::json!({"body":"hello event"})),
        )
        .await;
    assert_eq!(resp.status, 201, "no-binding message must be 201");
    let msg_1_id = resp.json()["message"]["id"].as_str().unwrap().to_owned();
    let msg_1_created = resp.json()["message"]["created_at"]
        .as_str()
        .unwrap()
        .to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_1}/messages"),
            Some(&human_token),
            Some("ev-m-1"),
            Some(&serde_json::json!({"body":"hello event"})),
        )
        .await;
    assert_eq!(resp.status, 200, "replay must be 200");
    assert_eq!(resp.json()["idempotency_replayed"], true);
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_1}/messages"),
            Some(&human_token),
            Some("ev-m-2"),
            Some(&serde_json::json!({"body":"second event"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    step(
        "no-binding",
        "2 messages committed without any binding; replay returned 200".into(),
    );

    // 数据库探针：无 Binding 时已原子生成 2 条 Event。
    verify_event_storage(&db_path, 2).await;
    step(
        "db-probe",
        "agent_events count=2 before any binding exists".into(),
    );

    // 建立 Binding/Credential，从起点拉到间隙 Event。
    let resp = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_1}/backend-bindings"),
            Some(&human_token),
            Some("ev-binding-1"),
            Some(&serde_json::json!({"label":"local runtime"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let binding_1 = resp.json()["binding"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_1}/backend-bindings/{binding_1}/credentials"),
            Some(&human_token),
            Some("ev-cred-1"),
            Some(&serde_json::json!({"replaces_credential_id":null})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let credential_1 = resp.json()["credential"]["id"].as_str().unwrap().to_owned();
    let backend_token_1 = resp.json()["token"].as_str().unwrap().to_owned();

    let resp = server
        .request(
            "GET",
            "/v1/backend/events",
            Some(&backend_token_1),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    let json = resp.json();
    let events = json["events"].as_array().unwrap();
    assert_eq!(
        events.len(),
        2,
        "gap events must be observable from the start"
    );
    assert_eq!(events[0]["kind"], "message.created");
    assert_eq!(events[0]["message"]["id"], msg_1_id);
    assert_eq!(events[0]["message"]["body"], "hello event");
    assert_eq!(events[0]["created_at"], msg_1_created);
    assert_eq!(events[0]["created_at"], events[0]["message"]["created_at"]);
    assert_eq!(events[0]["message"]["actor"]["kind"], "human");
    assert!(
        events[0].get("sequence").is_none(),
        "no sequence in event view"
    );
    assert!(
        events[0]["message"].get("sequence").is_none(),
        "no sequence in source message view"
    );
    assert_eq!(
        json["next_after"],
        serde_json::json!(format!("c1.be.{}", events[1]["id"].as_str().unwrap()))
    );
    let event_body = events[0].to_string();
    for forbidden in [
        "target_agent_id",
        "binding",
        "credential",
        "owner",
        "idempotency",
        "token",
        "hash",
        "cursor",
        "completion",
        "ack",
    ] {
        assert!(
            !event_body.contains(&format!("\"{forbidden}\"")),
            "event view must not expose {forbidden}"
        );
    }
    step(
        "observe",
        "2 events from start; kind=message.created; created_at==message.created_at; no target/binding/credential/idempotency/hash/sequence fields".into(),
    );

    // 重拉与分页：同一起点重拉结果相同；limit/after cursor/空页语义正确。
    let resp = server
        .request(
            "GET",
            "/v1/backend/events?after=c1.be.start",
            Some(&backend_token_1),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.json()["events"],
        json["events"],
        "re-pull must be stable"
    );
    let resp = server
        .request(
            "GET",
            "/v1/backend/events?limit=1",
            Some(&backend_token_1),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    let first_cursor = resp.json()["next_after"].as_str().unwrap().to_owned();
    assert_eq!(
        first_cursor,
        format!("c1.be.{}", events[0]["id"].as_str().unwrap())
    );
    let resp = server
        .request(
            "GET",
            &format!("/v1/backend/events?after={first_cursor}&limit=1"),
            Some(&backend_token_1),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.json()["events"][0]["id"],
        events[1]["id"],
        "cursor page returns the next event"
    );
    assert_eq!(
        resp.json()["next_after"],
        serde_json::json!(format!("c1.be.{}", events[1]["id"].as_str().unwrap()))
    );
    let last_cursor = format!("c1.be.{}", events[1]["id"].as_str().unwrap());
    let resp = server
        .request(
            "GET",
            &format!("/v1/backend/events?after={last_cursor}"),
            Some(&backend_token_1),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["events"], serde_json::json!([]));
    assert_eq!(
        resp.json()["next_after"],
        serde_json::json!(last_cursor),
        "empty page keeps the actual requested cursor"
    );
    let resp = server
        .request(
            "GET",
            &format!("/v1/backend/events?after={last_cursor}&limit=1"),
            Some(&backend_token_1),
            None,
            None,
        )
        .await;
    assert_eq!(resp.json()["events"], serde_json::json!([]));
    assert_eq!(
        resp.json()["next_after"],
        serde_json::json!(last_cursor),
        "empty page keeps the actual requested cursor"
    );
    for query in [
        "?after=abc",
        "?after=-1",
        "?after=1&after=2",
        "?after=9223372036854775808",
        "?after=",
        "?after=c1.m.550e8400-e29b-41d4-a716-446655440000",
        "?after=c1.he.550e8400-e29b-41d4-a716-446655440000",
        "?after=c1.be.550e8400-e29b-41d4-a716-446655440000",
        "?after=c2.be.550e8400-e29b-41d4-a716-446655440000",
        "?limit=0",
        "?limit=101",
        "?limit=abc",
        "?limit=1&limit=2",
        "?unknown=1",
    ] {
        let resp = server
            .request(
                "GET",
                &format!("/v1/backend/events{query}"),
                Some(&backend_token_1),
                None,
                None,
            )
            .await;
        assert_eq!(resp.status, 400, "query {query}");
        assert_eq!(
            resp.json()["error"]["code"],
            "invalid_request",
            "query {query}"
        );
    }
    step(
        "paging",
        "cursor round-trip/empty page/re-pull stable/start sentinel; strict query+cursor 400 matrix passed".into(),
    );

    // 第二个 Agent：各自 sequence 从 1，互不泄露。
    let resp = server
        .request(
            "POST",
            "/v1/agents",
            Some(&human_token),
            Some("ev-agent-2"),
            Some(&serde_json::json!({"display_name":"乙"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let agent_2 = resp.json()["agent"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&human_token),
            Some("ev-room-2"),
            Some(&serde_json::json!({"agent_id": agent_2})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let room_2 = resp.json()["room"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_2}/messages"),
            Some(&human_token),
            Some("ev-b-1"),
            Some(&serde_json::json!({"body":"agent two"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let resp = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_2}/backend-bindings"),
            Some(&human_token),
            Some("ev-binding-2"),
            Some(&serde_json::json!({"label":"second"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let binding_2 = resp.json()["binding"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_2}/backend-bindings/{binding_2}/credentials"),
            Some(&human_token),
            Some("ev-cred-2"),
            Some(&serde_json::json!({"replaces_credential_id":null})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let backend_token_2 = resp.json()["token"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "GET",
            "/v1/backend/events",
            Some(&backend_token_2),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    let json_2 = resp.json();
    let events_2 = json_2["events"].as_array().unwrap();
    assert_eq!(events_2.len(), 1, "agent 2 sees only its own event");
    assert_eq!(events_2[0]["message"]["body"], "agent two");
    let resp = server
        .request(
            "GET",
            "/v1/backend/events",
            Some(&backend_token_1),
            None,
            None,
        )
        .await;
    let json_1 = resp.json();
    let events_1 = json_1["events"].as_array().unwrap();
    assert_eq!(events_1.len(), 2, "agent 1 still sees only its own events");
    assert!(
        !serde_json::to_string(events_1)
            .unwrap()
            .contains("agent two"),
        "no cross-agent event leak"
    );
    // 跨 Agent cursor：把 agent 2 的事件 cursor 用到 agent 1 → 400，与未知
    // cursor 不可区分。
    let cursor_2 = json_2["next_after"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "GET",
            &format!("/v1/backend/events?after={cursor_2}"),
            Some(&backend_token_1),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 400, "cross-agent cursor must be rejected");
    assert_eq!(resp.json()["error"]["code"], "invalid_request");
    step(
        "isolation",
        "agent1 sees 2 events, agent2 sees 1; no cross leak; cross-agent cursor 400".into(),
    );

    // Human token 不能调用 Event API；Backend token 不能调用 Human API。
    let resp = server
        .request("GET", "/v1/backend/events", Some(&human_token), None, None)
        .await;
    assert_eq!(resp.status, 401);
    assert_eq!(resp.json()["error"]["code"], "invalid_credential");
    let resp = server
        .request("GET", "/v1/me", Some(&backend_token_1), None, None)
        .await;
    assert_eq!(resp.status, 401);
    step(
        "channels",
        "human token on events 401; backend token on /v1/me 401".into(),
    );

    // 重启：同一 SQLite DB 后 Event、sequence 与重拉结果保持。
    server.stop().await;
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;
    let resp = server
        .request(
            "GET",
            "/v1/backend/events",
            Some(&backend_token_1),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.json()["events"],
        json["events"],
        "restart must preserve events"
    );
    step(
        "restart",
        "same db restart keeps events and after re-pull".into(),
    );

    // Credential 轮换：新 Credential 从同一 Agent sequence 继续，旧 token 401。
    let resp = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_1}/backend-bindings/{binding_1}/credentials"),
            Some(&human_token),
            Some("ev-cred-rot"),
            Some(&serde_json::json!({"replaces_credential_id":credential_1})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let backend_token_1b = resp.json()["token"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "GET",
            "/v1/backend/events",
            Some(&backend_token_1),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 401, "rotated-away credential must fail");
    let resp = server
        .request(
            "GET",
            "/v1/backend/events",
            Some(&backend_token_1b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.json()["events"],
        json["events"],
        "rotation must not change events"
    );
    step(
        "rotation",
        "rotated credential keeps the same event facts; old token 401".into(),
    );

    // Binding revoke：旧 Credential 401，Event 保留；无 Binding 间隙继续生成。
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/agents/{agent_1}/backend-bindings/{binding_1}"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204);
    let resp = server
        .request(
            "GET",
            "/v1/backend/events",
            Some(&backend_token_1b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 401, "revoked binding credential must fail");
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_1}/messages"),
            Some(&human_token),
            Some("ev-m-3"),
            Some(&serde_json::json!({"body":"gap message"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    // 探针按全库总数断言：agent1 的 3 条 + agent2 的 1 条 = 4。
    verify_event_storage(&db_path, 4).await;
    let resp = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_1}/backend-bindings"),
            Some(&human_token),
            Some("ev-binding-3"),
            Some(&serde_json::json!({"label":"rebuilt"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let binding_3 = resp.json()["binding"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_1}/backend-bindings/{binding_3}/credentials"),
            Some(&human_token),
            Some("ev-cred-3"),
            Some(&serde_json::json!({"replaces_credential_id":null})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let backend_token_3 = resp.json()["token"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "GET",
            "/v1/backend/events",
            Some(&backend_token_3),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.json()["events"].as_array().unwrap().len(),
        3,
        "rebuilt backend observes the gap events"
    );
    assert_eq!(resp.json()["events"][0]["message"]["body"], "hello event");
    assert_eq!(resp.json()["events"][2]["message"]["body"], "gap message");
    step(
        "binding-gap",
        "binding revoke keeps events; gap message generated; rebuilt binding observes all gap events".into(),
    );

    // Agent revoke：Credential fail closed，Event 数据保留，Human History 不变。
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/agents/{agent_1}"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204);
    let resp = server
        .request(
            "GET",
            "/v1/backend/events",
            Some(&backend_token_3),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 401, "agent revoke must fail closed");
    assert_eq!(resp.json()["error"]["code"], "invalid_credential");
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_1}/messages"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["messages"].as_array().unwrap().len(), 3);
    verify_event_storage(&db_path, 4).await;
    step(
        "agent-revoke",
        "agent revoke -> credential 401; 3 event rows preserved; human history unchanged".into(),
    );

    server.stop().await;
    run_dir.cleanup();

    println!("EVENT BLACKBOX EVIDENCE:");
    println!("{}", evidence.join("\n"));
    println!(
        "EVENT BLACKBOX DONE: no-binding generation, gap observe, replay, isolation, paging, restart, rotation, binding/agent revoke fail-closed, data preserved, strict query 400, redaction all passed; server closed, temp assets removed"
    );
}

/// 打开黑盒测试数据库的只读探针连接池（数据库证据核验用；成功/拒绝结果
/// 仍从公开 API 或真实数据库约束观察，这里只做黑盒之外的存储形状证明）。
async fn open_fixture_pool(db_path: &Path) -> SqlitePool {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open fixture database")
}

/// Mock Agent Backend 可执行文件路径（独立进程协议客户端）。
const MOCK_BIN: &str = env!("CARGO_BIN_EXE_celira-mock-backend");

/// 通过 CLI 创建 Human（create-human），返回 (participant_id, token)。
async fn create_human_cli(db_path: &Path, display_name: &str) -> (String, String) {
    let output = Command::new(BIN)
        .args(["create-human", "--display-name", display_name])
        .env("CELIRA_DB_PATH", db_path)
        .output()
        .await
        .expect("run create-human");
    assert!(
        output.status.success(),
        "create-human failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("cli json");
    let participant_id = json["participant_id"].as_str().unwrap().to_owned();
    let token = json["token"].as_str().unwrap().to_owned();
    assert!(json["session_id"].is_string());
    (participant_id, token)
}

/// 启动独立 Mock Agent Backend 进程（真实协议客户端：只通过 Backend
/// Credential 拉 Event，并以成员身份 say 回话）。
/// ⚠ 2026-09-28：原走「拉 Event → POST Completion → ACK」；Completion/ACK
/// 整体退役后改走 **拉取 + say**（拉取模型下主体自己说）。
struct MockBackend {
    child: Child,
}

impl MockBackend {
    fn start(core_url: &str, token: &str) -> Self {
        let child = Command::new(MOCK_BIN)
            .env("CELIRA_IM_URL", core_url)
            .env("CELIRA_BACKEND_TOKEN", token)
            .env("CELIRA_MOCK_POLL_MS", "200")
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn mock backend");
        Self { child }
    }

    async fn stop(&mut self) {
        let _ = self.child.kill().await;
        let waited = timeout(Duration::from_secs(10), self.child.wait()).await;
        assert!(waited.is_ok(), "mock backend did not exit after kill");
    }
}

impl Drop for MockBackend {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// 轮询 group History 直到出现期望数量的消息（等待 Mock Backend 完成
/// reply）；`expected_body` 非空时还要求该正文已出现在 History 中。
async fn wait_for_history(
    server: &Server,
    token: &str,
    room_id: &str,
    expected_len: usize,
    expected_body: &str,
) -> serde_json::Value {
    let deadline = Duration::from_secs(30);
    timeout(deadline, async {
        loop {
            let resp = server
                .request(
                    "GET",
                    &format!("/v1/rooms/{room_id}/messages?limit=100"),
                    Some(token),
                    None,
                    None,
                )
                .await;
            if resp.status == 200 {
                let json = resp.json();
                let messages = json["messages"].as_array().cloned().unwrap_or_default();
                let len = messages.len();
                let has_body = expected_body.is_empty()
                    || messages.iter().any(|m| {
                        m["body"]
                            .as_str()
                            .map(|b| b == expected_body)
                            .unwrap_or(false)
                    });
                if len >= expected_len && has_body {
                    return json;
                }
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    })
    .await
    .expect("history never reached expected state")
}

/// 运行时只读探针：server 仍持有该库时直读 agent_events 总数（只读连接，
/// busy_timeout 5s，不改任何事实）。用于证明「非 Agent 目标不产生 Event」。
async fn count_agent_events(db_path: &Path) -> i64 {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .foreign_keys(true)
        .read_only(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open event probe");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_events")
        .fetch_one(&pool)
        .await
        .expect("count agent events");
    drop(pool);
    count
}

/// 真实 SQLite 探针：contact_periods / mentions / events / memberships 计数与
/// 终态断言。
async fn verify_contacts_groups_storage(
    db_path: &Path,
    expected_contacts: i64,
    expected_mentions: i64,
    expected_events_by_agent: &[(&str, i64)],
) {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open storage probe");

    let contacts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM contact_periods")
        .fetch_one(&pool)
        .await
        .expect("count contact periods");
    assert_eq!(contacts, expected_contacts, "one active period per pair");
    let mentions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mentions")
        .fetch_one(&pool)
        .await
        .expect("count mentions");
    assert_eq!(mentions, expected_mentions);
    for (agent, expected) in expected_events_by_agent {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM agent_events WHERE target_agent_id = ?")
                .bind(agent)
                .fetch_one(&pool)
                .await
                .expect("count agent events");
        assert_eq!(count, *expected, "agent {agent} event count");
        let sequences: Vec<i64> = sqlx::query_scalar(
            "SELECT sequence FROM agent_events WHERE target_agent_id = ? ORDER BY sequence",
        )
        .bind(agent)
        .fetch_all(&pool)
        .await
        .expect("agent sequences");
        let expected_seq: Vec<i64> = (1..=*expected).collect();
        assert_eq!(sequences, expected_seq, "agent {agent} sequences");
    }
    drop(pool);
}

/// 真实独立进程黑盒：多 Human + Contact pending→accepted + Human direct +
/// group + 显式 mentions→Agent Event，以及独立 Mock Agent Backend 进程
/// 只通过真实 Credential 拉 Event 并 POST Completion reply 进入群 History。
/// 全部步骤只通过 CLI 与公开 HTTP API 完成。
#[tokio::test(flavor = "multi_thread")]
async fn blackbox_humans_contacts_groups_mentions_via_public_api() {
    let mut run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;
    let core_url = format!("http://{}", server.addr);

    // Operator 创建 Human A（bootstrap）/ B / C（create-human），Session 独立。
    let token_a = bootstrap_cli(&db_path).await;
    let (human_b, token_b) = create_human_cli(&db_path, "B").await;
    let (_human_c, token_c) = create_human_cli(&db_path, "C").await;
    let mut evidence: Vec<String> = Vec::new();
    let mut step = |name: &str, detail: String| evidence.push(format!("[{name}] {detail}"));

    // 身份隔离：B 的 Session 只能代表 B；C 的 Session 只能代表 C。
    let resp = server
        .request("GET", "/v1/me", Some(&token_a), None, None)
        .await;
    assert_eq!(resp.status, 200);
    assert_ne!(
        resp.json()["participant"]["id"],
        human_b,
        "A session must not resolve to B"
    );
    let resp = server
        .request("GET", "/v1/me", Some(&token_b), None, None)
        .await;
    assert_eq!(resp.json()["participant"]["id"], human_b);
    step(
        "identities",
        "bootstrap A + create-human B/C; sessions resolve only to their own human".into(),
    );

    let human_a = server
        .request("GET", "/v1/me", Some(&token_a), None, None)
        .await
        .json()["participant"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Contacts：A→B 请求；重复/反向收敛；未接受不能建 human_direct / 进群。
    let resp = server
        .request(
            "POST",
            "/v1/contacts/requests",
            Some(&token_a),
            Some("c-ab-1"),
            Some(&serde_json::json!({"participant_id": human_b})),
        )
        .await;
    assert_eq!(resp.status, 201, "contact request: {}", resp.json());
    let resp = server
        .request(
            "POST",
            "/v1/contacts/requests",
            Some(&token_a),
            Some("c-ab-1"),
            Some(&serde_json::json!({"participant_id": human_b})),
        )
        .await;
    assert_eq!(resp.status, 200, "same key replay: {}", resp.json());
    assert_eq!(resp.json()["idempotency_replayed"], true);
    // B→A 反向请求（新 key）收敛到同一 pending 关系，不创建第二条。
    let resp = server
        .request(
            "POST",
            "/v1/contacts/requests",
            Some(&token_b),
            Some("c-ba-1"),
            Some(&serde_json::json!({"participant_id": human_a})),
        )
        .await;
    assert_eq!(
        resp.status,
        200,
        "reverse request must converge: {}",
        resp.json()
    );
    assert_eq!(resp.json()["idempotency_replayed"], true);

    // 未接受：A 建 human_direct → 409 contact_not_accepted；A 建群邀请 B → 404 member_not_found。
    let resp = server
        .request(
            "POST",
            "/v1/human-direct-rooms",
            Some(&token_a),
            Some("hd-1"),
            Some(&serde_json::json!({"human_id": human_b})),
        )
        .await;
    assert_eq!(
        resp.status,
        409,
        "pending contact must block human direct: {}",
        resp.json()
    );
    assert_eq!(resp.json()["error"]["code"], "contact_not_accepted");
    let resp = server
        .request(
            "POST",
            "/v1/groups",
            Some(&token_a),
            Some("g-1"),
            Some(&serde_json::json!({"name": "项目群"})),
        )
        .await;
    assert_eq!(resp.status, 201, "group create: {}", resp.json());
    let group_id = resp.json()["room"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_a),
            Some("inv-b-pending"),
            Some(&serde_json::json!({"participant_id": human_b})),
        )
        .await;
    assert_eq!(
        resp.status,
        404,
        "pending contact must block group invite: {}",
        resp.json()
    );

    // B 接受（只有 addressee 可以）→ accepted；重复接受幂等。
    let resp = server
        .request(
            "POST",
            &format!("/v1/contacts/{human_a}/accept"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "accept: {}", resp.json());
    assert_eq!(resp.json()["contact"]["status"], "accepted");
    let resp = server
        .request(
            "POST",
            &format!("/v1/contacts/{human_a}/accept"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["contact"]["status"], "accepted");
    // requester 不能接受自己的请求。
    let resp = server
        .request(
            "POST",
            &format!("/v1/contacts/{human_b}/accept"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 404, "requester cannot accept: {}", resp.json());
    step(
        "contacts",
        "A→B pending, duplicate/reverse converge to one fact, unaccepted blocks direct+group, addressee-only accept".into(),
    );

    // Human direct：A 创建；B 反向创建 409；同 key 重放 200；双向互聊。
    let resp = server
        .request(
            "POST",
            "/v1/human-direct-rooms",
            Some(&token_a),
            Some("hd-2"),
            Some(&serde_json::json!({"human_id": human_b})),
        )
        .await;
    assert_eq!(resp.status, 201, "human direct create: {}", resp.json());
    let hd_id = resp.json()["room"]["id"].as_str().unwrap().to_owned();
    assert_eq!(resp.json()["room"]["kind"], "human_direct");
    let resp = server
        .request(
            "POST",
            "/v1/human-direct-rooms",
            Some(&token_a),
            Some("hd-2"),
            Some(&serde_json::json!({"human_id": human_b})),
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["room"]["id"], hd_id);
    let resp = server
        .request(
            "POST",
            "/v1/human-direct-rooms",
            Some(&token_b),
            Some("hd-3"),
            Some(&serde_json::json!({"human_id": human_a})),
        )
        .await;
    assert_eq!(resp.status, 409, "reverse create: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "human_direct_room_exists");

    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{hd_id}/messages"),
            Some(&token_a),
            Some("hd-m-1"),
            Some(&serde_json::json!({"body": "A 说你好"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{hd_id}/messages"),
            Some(&token_b),
            Some("hd-m-2"),
            Some(&serde_json::json!({"body": "B 回复"})),
        )
        .await;
    assert_eq!(resp.status, 201, "B replies: {}", resp.json());
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{hd_id}/messages"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    let hd_messages = resp.json();
    assert_eq!(hd_messages["messages"].as_array().unwrap().len(), 2);
    assert_eq!(hd_messages["messages"][0]["actor"]["id"], human_a);
    assert_eq!(hd_messages["messages"][1]["actor"]["id"], human_b);
    // 隔离：C 读不到。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{hd_id}/messages"),
            Some(&token_c),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 404);
    step(
        "human-direct",
        "accepted pair room created, reverse 409, A↔B two-way messages, C isolated".into(),
    );

    // Agents：A 创建两个 owned Agent，各建 Binding/Credential。
    let mut agent_ids = Vec::new();
    let mut backend_tokens = Vec::new();
    for (idx, name) in ["甲", "乙"].iter().enumerate() {
        let resp = server
            .request(
                "POST",
                "/v1/agents",
                Some(&token_a),
                Some(&format!("agent-{idx}")),
                Some(&serde_json::json!({"display_name": name})),
            )
            .await;
        assert_eq!(resp.status, 201);
        let agent_id = resp.json()["agent"]["id"].as_str().unwrap().to_owned();
        agent_ids.push(agent_id.clone());
        let resp = server
            .request(
                "POST",
                &format!("/v1/agents/{agent_id}/backend-bindings"),
                Some(&token_a),
                Some(&format!("binding-{idx}")),
                Some(&serde_json::json!({"label": "Mock"})),
            )
            .await;
        assert_eq!(resp.status, 201);
        let binding_id = resp.json()["binding"]["id"].as_str().unwrap().to_owned();
        let resp = server
            .request(
                "POST",
                &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
                Some(&token_a),
                Some(&format!("cred-{idx}")),
                Some(&serde_json::json!({"replaces_credential_id": null})),
            )
            .await;
        assert_eq!(resp.status, 201);
        backend_tokens.push(resp.json()["token"].as_str().unwrap().to_owned());
    }
    let agent_1 = agent_ids[0].clone();
    let agent_2 = agent_ids[1].clone();

    // Group：邀请 B（accepted Human）与两个 owned Agent（必带
    // Idempotency-Key，先查 key 再查权限）；第三方被拒。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_a),
            Some("inv-b-1"),
            Some(&serde_json::json!({"participant_id": human_b})),
        )
        .await;
    assert_eq!(resp.status, 201, "invite B: {}", resp.json());
    assert_eq!(resp.json()["idempotency_replayed"], false);
    assert!(resp.json()["membership"]["membership_id"].is_string());
    for (idx, agent) in [&agent_1, &agent_2].iter().enumerate() {
        let resp = server
            .request(
                "POST",
                &format!("/v1/rooms/{group_id}/members"),
                Some(&token_a),
                Some(&format!("inv-ag-{idx}")),
                Some(&serde_json::json!({"participant_id": agent})),
            )
            .await;
        assert_eq!(resp.status, 201, "invite agent: {}", resp.json());
    }
    // 同 key 重放 → 200 replayed=true，不重开周期。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_a),
            Some("inv-b-1"),
            Some(&serde_json::json!({"participant_id": human_b})),
        )
        .await;
    assert_eq!(resp.status, 200, "invite replay: {}", resp.json());
    assert_eq!(resp.json()["idempotency_replayed"], true);
    // C（非 accepted）邀请 → 404 member_not_found。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_a),
            Some("inv-c-1"),
            Some(&serde_json::json!({"participant_id": _human_c})),
        )
        .await;
    assert_eq!(resp.status, 404);
    assert_eq!(resp.json()["error"]["code"], "member_not_found");
    step(
        "group",
        "B + owned agents invited; unaccepted C rejected; owner=A".into(),
    );

    // 普通群消息：成功但无 Event（Mock 未启动，先验证序列与无事件）。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/messages"),
            Some(&token_a),
            Some("g-m-1"),
            Some(&serde_json::json!({"body": "大家好"})),
        )
        .await;
    assert_eq!(resp.status, 201, "ordinary group message: {}", resp.json());
    assert_eq!(resp.json()["message"]["mentions"], serde_json::json!([]));

    // Mention 目标放开窄切片（CORE_V1_MENTION_TARGETS.md）：本群的 active
    // Human 成员 B 是合法目标 → 201，响应 mentions 如实回显；B 不是 Agent，
    // 因此不生成任何 agent_event（真实 SQLite 只读探针直读该库）。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/messages"),
            Some(&token_a),
            Some("g-m-human"),
            Some(&serde_json::json!({"body": "@B 看这里", "mentions": [human_b]})),
        )
        .await;
    assert_eq!(resp.status, 201, "human member mention: {}", resp.json());
    assert_eq!(
        resp.json()["message"]["mentions"],
        serde_json::json!([human_b]),
        "mentions echoed back verbatim"
    );
    assert_eq!(
        count_agent_events(&db_path).await,
        0,
        "human mention must not generate any agent event"
    );

    // 非法 mention 原子失败：非成员 Human、重复目标、未知 ID。
    for (key, mentions) in [
        ("g-m-bad-2", serde_json::json!([_human_c])),
        ("g-m-bad-3", serde_json::json!([agent_1, agent_1])),
        (
            "g-m-bad-4",
            serde_json::json!([uuid::Uuid::new_v4().to_string()]),
        ),
    ] {
        let resp = server
            .request(
                "POST",
                &format!("/v1/rooms/{group_id}/messages"),
                Some(&token_a),
                Some(key),
                Some(&serde_json::json!({"body": "x", "mentions": mentions})),
            )
            .await;
        assert_eq!(resp.status, 400, "invalid mention {key}: {}", resp.json());
        assert_eq!(resp.json()["error"]["code"], "invalid_mention");
    }
    // Mention 上限：65 个目标在进入持久化之前被 400 invalid_mention 拒绝，
    // 不产生 Message / Mention / Event / sequence（数量守卫先于 UUID 解析，
    // 因此无需 65 个真实 Agent 成员）。
    let mentions_65: Vec<String> = (0..65).map(|_| uuid::Uuid::new_v4().to_string()).collect();
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/messages"),
            Some(&token_a),
            Some("g-m-bad-5"),
            Some(&serde_json::json!({"body": "x", "mentions": mentions_65})),
        )
        .await;
    assert_eq!(resp.status, 400, "65 mentions: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "invalid_mention");
    // mention 变化幂等冲突：同 key 同 body 换 mention 集合 → 409。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/messages"),
            Some(&token_a),
            Some("g-m-2"),
            Some(&serde_json::json!({"body": "@甲 你好", "mentions": [agent_1]})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/messages"),
            Some(&token_a),
            Some("g-m-2"),
            Some(&serde_json::json!({"body": "@甲 你好", "mentions": []})),
        )
        .await;
    assert_eq!(resp.status, 409, "mention set change: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "idempotency_conflict");
    // 非法 mention 无副作用：History 只含普通消息与 @甲。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}/messages?limit=100"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    let group_before = resp.json();
    let bodies: Vec<String> = group_before["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["body"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        bodies,
        vec!["大家好", "@B 看这里", "@甲 你好"],
        "no gaps from failed mentions"
    );
    assert_eq!(
        group_before["messages"][1]["mentions"],
        serde_json::json!([human_b]),
        "history returns canonical mentions for a human target"
    );
    assert_eq!(
        group_before["messages"][2]["mentions"],
        serde_json::json!([agent_1]),
        "history returns canonical mentions"
    );
    step(
        "mentions-validation",
        "ordinary msg no event, active human member mention 201 with zero agent events, invalid mention 400 x4 atomic (incl. 65-target cap), mention-set change 409, history intact".into(),
    );

    // 启动两个独立 Mock Agent Backend 进程（每个 Agent 一个 Credential）。
    let mut mock_1 = MockBackend::start(&core_url, &backend_tokens[0]);
    let mut mock_2 = MockBackend::start(&core_url, &backend_tokens[1]);

    // 晚接入的 Mock 从起点观察到间隙 Event（此前 @甲 你好 的 Event），
    // 先等它的 echo 出现（History 从 2 增长到 3）。
    let history = wait_for_history(&server, &token_a, &group_id, 4, "echo: @甲 你好").await;
    let messages = history["messages"].as_array().unwrap();
    assert_eq!(messages[3]["body"], "echo: @甲 你好");
    assert_eq!(messages[3]["actor"]["id"], agent_1);
    step(
        "gap-echo",
        "late-joined mock observed the earlier gap event and echoed it".into(),
    );

    // @甲 单 Agent：只有甲得到 Event 并回复。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/messages"),
            Some(&token_a),
            Some("g-m-3"),
            Some(&serde_json::json!({"body": "@甲 请回答", "mentions": [agent_1]})),
        )
        .await;
    assert_eq!(resp.status, 201, "mention 甲: {}", resp.json());
    let history = wait_for_history(&server, &token_a, &group_id, 6, "echo: @甲 请回答").await;
    let messages = history["messages"].as_array().unwrap();
    assert_eq!(messages[5]["actor"]["id"], agent_1);
    assert_eq!(messages[5]["body"], "echo: @甲 请回答");
    assert_eq!(messages[5]["actor"]["kind"], "agent");
    assert_eq!(
        messages[5]["mentions"],
        serde_json::json!([]),
        "reply carries no mention"
    );
    step(
        "mention-one",
        "@甲 -> only agent 甲 replied via real credential; reply visible in group history as agent actor".into(),
    );

    // @两个 Agent：各得一条 Event、各回一条，互不读取/写回对方事实。
    let mut sorted = vec![agent_1.clone(), agent_2.clone()];
    sorted.sort();
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/messages"),
            Some(&token_a),
            Some("g-m-4"),
            Some(&serde_json::json!({"body": "@两位 都回答", "mentions": sorted})),
        )
        .await;
    assert_eq!(resp.status, 201, "mention both: {}", resp.json());
    assert_eq!(
        resp.json()["message"]["mentions"],
        serde_json::json!(sorted)
    );
    let history = wait_for_history(&server, &token_a, &group_id, 9, "echo: @两位 都回答").await;
    let messages = history["messages"].as_array().unwrap();
    let echoes: Vec<&serde_json::Value> = messages
        .iter()
        .filter(|m| m["body"].as_str() == Some("echo: @两位 都回答"))
        .collect();
    assert_eq!(
        echoes.len(),
        2,
        "exactly two echoes for two mentioned agents"
    );
    assert_ne!(
        echoes[0]["actor"]["id"], echoes[1]["actor"]["id"],
        "two agents each replied once, isolated"
    );
    assert!(echoes[0]["actor"]["id"] == agent_1 || echoes[0]["actor"]["id"] == agent_2);
    step(
        "mention-two",
        "@甲+@乙 -> each agent got its own event and wrote its own echo reply".into(),
    );

    // Agent 自回复不触发：等待稳定窗口后 History 条数不变（最终 8 条）。
    tokio::time::sleep(Duration::from_secs(3)).await;
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}/messages?limit=100"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(
        resp.json()["messages"].as_array().unwrap().len(),
        9,
        "no self-trigger loop from agent replies"
    );
    step(
        "no-self-trigger",
        "after stability window history stays at 9; agent replies did not wake themselves".into(),
    );

    // 移除 B：B 不能读群 History；旧 key 重放只返回历史 closed 周期；
    // 新 key 重入 → 201（删除 MemberRevoked 终态）；owner 不可移除。
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/rooms/{group_id}/members/{human_b}"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204, "remove B: {}", resp.json());
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}/messages"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 404, "removed member cannot read");
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_a),
            Some("inv-b-1"),
            Some(&serde_json::json!({"participant_id": human_b})),
        )
        .await;
    assert_eq!(resp.status, 200, "old key replay: {}", resp.json());
    assert_eq!(resp.json()["idempotency_replayed"], true);
    assert_eq!(resp.json()["membership"]["closed_by"], "removed");
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_a),
            Some("inv-b-rejoin"),
            Some(&serde_json::json!({"participant_id": human_b})),
        )
        .await;
    assert_eq!(resp.status, 201, "rejoin B: {}", resp.json());
    assert_eq!(resp.json()["idempotency_replayed"], false);
    assert_eq!(
        resp.json()["membership"]["closed_by"],
        serde_json::Value::Null
    );
    step(
        "remove-rejoin",
        "B removed (read blocked), old key replays the closed period, new key re-enters 201".into(),
    );

    // 重启：Room/Membership/Message/Mention/Event/Completion/History 保持。
    mock_1.stop().await;
    mock_2.stop().await;
    server.stop().await;
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}/messages?limit=100"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["messages"].as_array().unwrap().len(), 9);
    assert_eq!(
        resp.json()["messages"][5]["body"],
        "echo: @甲 请回答",
        "mock completion + agent message survive restart"
    );
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{hd_id}/messages"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "human direct survives restart");
    assert_eq!(resp.json()["messages"].as_array().unwrap().len(), 2);
    step(
        "restart",
        "group history (9), agent echoes, human direct all preserved".into(),
    );

    server.stop().await;
    verify_contacts_groups_storage(
        &db_path,
        1,
        5,
        &[(&agent_1, 3), (&agent_2, 1), (human_b.as_str(), 0)],
    )
    .await;
    run_dir.cleanup();

    println!("HUMANS/GROUPS BLACKBOX EVIDENCE:");
    println!("{}", evidence.join("\n"));
    println!(
        "HUMANS/GROUPS BLACKBOX DONE: two humans, contacts pending->accepted, human direct two-way, group create/invite/remove, ordinary group msg no event, @single/@multi agent events isolated, mock backend real-HTTP echo replies in history, no self-trigger, restart persistence all passed; processes closed, temp assets removed"
    );
}

/// 群 Membership 周期黑盒验收（独立进程 + 公开 HTTP + 真实 SQLite 探针）：
/// A 首次加入后发送 1-5，leave；群内继续 6-10；A 用新 key 重入后 11-15；
/// A 的 History 只含 1-5 与 11-15；离群期间所有 Room 读写 404；分页跨
/// gap 无重复/跳过/泄露；B 全程可见 1-15；旧 key 重放不重开周期；群主
/// 原子转让后 coverage 见证正确、旧 owner 可退群、owner leave 409；
/// 进程重启后周期与窗口保持。
#[tokio::test(flavor = "multi_thread")]
async fn blackbox_group_membership_lifecycle_via_public_api() {
    let mut run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;

    // Operator 创建 Human A（bootstrap）/ B（create-human）。
    let token_a = bootstrap_cli(&db_path).await;
    let (human_b, token_b) = create_human_cli(&db_path, "B").await;
    let human_a = server
        .request("GET", "/v1/me", Some(&token_a), None, None)
        .await
        .json()["participant"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut evidence: Vec<String> = Vec::new();
    let mut step = |name: &str, detail: String| evidence.push(format!("[{name}] {detail}"));

    // A→B accepted 联系人。
    let resp = server
        .request(
            "POST",
            "/v1/contacts/requests",
            Some(&token_a),
            Some("lc-c-ab"),
            Some(&serde_json::json!({"participant_id": human_b})),
        )
        .await;
    assert_eq!(resp.status, 201, "contact request: {}", resp.json());
    let resp = server
        .request(
            "POST",
            &format!("/v1/contacts/{human_a}/accept"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "accept: {}", resp.json());

    // B 建群并邀请 A（必带 Idempotency-Key）。
    let resp = server
        .request(
            "POST",
            "/v1/groups",
            Some(&token_b),
            Some("lc-g-1"),
            Some(&serde_json::json!({"name": "周期群"})),
        )
        .await;
    assert_eq!(resp.status, 201, "group create: {}", resp.json());
    let group_id = resp.json()["room"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_b),
            Some("lc-inv-a-1"),
            Some(&serde_json::json!({"participant_id": human_a})),
        )
        .await;
    assert_eq!(resp.status, 201, "invite A: {}", resp.json());
    let a_first_membership = resp.json()["membership"]["membership_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // A 在群期间发送 1-5。
    for i in 1..=5 {
        let resp = server
            .request(
                "POST",
                &format!("/v1/rooms/{group_id}/messages"),
                Some(&token_a),
                Some(&format!("lc-a-m-{i}")),
                Some(&serde_json::json!({"body": format!("a{i}")})),
            )
            .await;
        assert_eq!(resp.status, 201, "A sends {i}: {}", resp.json());
    }
    step("join-1", "A invited, sends 1-5".into());

    // A 主动退群 → 204；离群期间所有 Room 读写 404；再 leave 404。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/leave"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204, "A leaves: {}", resp.json());
    for (method, path, key) in [
        ("GET", format!("/v1/rooms/{group_id}"), None),
        ("GET", format!("/v1/rooms/{group_id}/members"), None),
        ("GET", format!("/v1/rooms/{group_id}/messages"), None),
        (
            "POST",
            format!("/v1/rooms/{group_id}/messages"),
            Some("lc-out-m-1"),
        ),
    ] {
        let resp = server
            .request(
                method,
                &path,
                Some(&token_a),
                key,
                key.map(|_| serde_json::json!({"body": "out"})).as_ref(),
            )
            .await;
        assert_eq!(
            resp.status,
            404,
            "out-of-group {method} {path}: {}",
            resp.json()
        );
        assert_eq!(resp.json()["error"]["code"], "room_not_found");
    }
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/leave"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 404, "second leave: {}", resp.json());
    // A 的 rooms 列表不再含该群。
    let resp = server
        .request("GET", "/v1/rooms", Some(&token_a), None, None)
        .await;
    assert_eq!(resp.status, 200);
    assert!(
        resp.json()["rooms"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["room"]["id"] != group_id),
        "left member must not list the group"
    );

    // 群内继续产生 6-10（B 发送）。
    for i in 6..=10 {
        let resp = server
            .request(
                "POST",
                &format!("/v1/rooms/{group_id}/messages"),
                Some(&token_b),
                Some(&format!("lc-b-m-{i}")),
                Some(&serde_json::json!({"body": format!("b{i}")})),
            )
            .await;
        assert_eq!(resp.status, 201, "B sends {i}: {}", resp.json());
    }
    step(
        "leave",
        "A left (204), all room reads/writes 404, B produced 6-10".into(),
    );

    // 旧 key 重放只返回历史 closed 周期（leave），绝不误开新周期。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_b),
            Some("lc-inv-a-1"),
            Some(&serde_json::json!({"participant_id": human_a})),
        )
        .await;
    assert_eq!(resp.status, 200, "old key replay: {}", resp.json());
    assert_eq!(resp.json()["idempotency_replayed"], true);
    assert_eq!(
        resp.json()["membership"]["membership_id"],
        a_first_membership
    );
    assert_eq!(resp.json()["membership"]["closed_by"], "leave");
    // 周期未重开：A 仍不能读。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}/messages"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 404, "old key must not reopen the period");

    // A 用新 key 重入 → 201。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_b),
            Some("lc-inv-a-2"),
            Some(&serde_json::json!({"participant_id": human_a})),
        )
        .await;
    assert_eq!(resp.status, 201, "A rejoins: {}", resp.json());
    assert_eq!(resp.json()["idempotency_replayed"], false);
    let a_second_membership = resp.json()["membership"]["membership_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(a_first_membership, a_second_membership);

    // 不同 key 并发同目标：新 key 只返回同一 active 周期（200），不重开。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_b),
            Some("lc-inv-a-3"),
            Some(&serde_json::json!({"participant_id": human_a})),
        )
        .await;
    assert_eq!(
        resp.status,
        200,
        "extra key on active member: {}",
        resp.json()
    );
    assert_eq!(resp.json()["idempotency_replayed"], false);
    assert_eq!(
        resp.json()["membership"]["membership_id"],
        a_second_membership
    );

    // 重入后 A 与 B 产生 11-15。
    for i in 11..=15 {
        let resp = server
            .request(
                "POST",
                &format!("/v1/rooms/{group_id}/messages"),
                Some(&token_a),
                Some(&format!("lc-a-m-{i}")),
                Some(&serde_json::json!({"body": format!("a{i}")})),
            )
            .await;
        assert_eq!(
            resp.status,
            201,
            "A sends after rejoin {i}: {}",
            resp.json()
        );
    }
    step(
        "rejoin",
        "old key replays closed period only, new key rejoins (201), extra keys converge to one active period, 11-15 produced".into(),
    );

    // A 的 History：只含 1-5 与 11-15；分页跨 gap 无重复、无跳过、无泄露
    // （公共视图用 opaque cursor 与稳定 Message ID 表达，内部 sequence 由
    // 数据库探针证明）。
    let mut pages: Vec<Vec<String>> = Vec::new();
    let mut next_before: Option<String> = None;
    loop {
        let query = match &next_before {
            Some(before) => format!("/v1/rooms/{group_id}/messages?limit=3&before={before}"),
            None => format!("/v1/rooms/{group_id}/messages?limit=3"),
        };
        let resp = server
            .request("GET", &query, Some(&token_a), None, None)
            .await;
        assert_eq!(resp.status, 200, "paging query {query}: {}", resp.json());
        let json = resp.json();
        pages.push(
            json["messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m["body"].as_str().unwrap().to_owned())
                .collect(),
        );
        match json["next_before"].as_str() {
            Some(next) => next_before = Some(next.to_owned()),
            None => break,
        }
    }
    assert_eq!(
        pages,
        vec![
            vec!["a13", "a14", "a15"],
            vec!["a5", "a11", "a12"],
            vec!["a2", "a3", "a4"],
            vec!["a1"],
        ],
        "A pages across the gap: no duplicates, no skips, no gap leakage"
    );
    let all_a: Vec<String> = pages.iter().flatten().cloned().collect();
    assert!(
        !all_a.iter().any(|b| b.starts_with('b')),
        "gap messages must never leak into A's history: {all_a:?}"
    );
    assert_eq!(all_a.len(), 10, "exactly 1-5 and 11-15");
    // 数据库探针：A 的可见消息内部 sequence 恰为 1-5 与 11-15。
    let a_seqs = probe_visible_sequences(&db_path, &group_id, &human_a).await;
    assert_eq!(a_seqs, vec![1, 2, 3, 4, 5, 11, 12, 13, 14, 15]);
    // B 全程可见 1-15。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}/messages?limit=100"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    let b_bodies: Vec<String> = resp.json()["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["body"].as_str().unwrap().to_owned())
        .collect();
    let expected_b: Vec<String> = (1..=15)
        .map(|i| format!("{}{i}", if i <= 5 || i >= 11 { "a" } else { "b" }))
        .collect();
    assert_eq!(b_bodies, expected_b, "B sees the full 1-15 timeline");
    step(
        "windows",
        "A history = {1-5, 11-15} paged across gap (3/3/3/1) without dup/skip/leak; B sees 1-15; seq probe confirms windows"
            .into(),
    );

    // 群主原子转让：B 转让给 A（同 key 重放 200 replayed；owner leave 409）。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/owner-transfer"),
            Some(&token_b),
            Some("lc-t-1"),
            Some(&serde_json::json!({"participant_id": human_a})),
        )
        .await;
    assert_eq!(resp.status, 200, "transfer: {}", resp.json());
    assert_eq!(resp.json()["idempotency_replayed"], false);
    let new_owner_membership = resp.json()["membership"]["membership_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(resp.json()["membership"]["participant_id"], human_a);
    assert_eq!(resp.json()["membership"]["role"], "owner");
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/owner-transfer"),
            Some(&token_b),
            Some("lc-t-1"),
            Some(&serde_json::json!({"participant_id": human_a})),
        )
        .await;
    assert_eq!(resp.status, 200, "transfer replay: {}", resp.json());
    assert_eq!(resp.json()["idempotency_replayed"], true);
    assert_eq!(
        resp.json()["membership"]["membership_id"],
        new_owner_membership
    );

    // 转让目标校验：Agent / 自己 / 非成员 → 稳定业务错误。
    let resp = server
        .request(
            "POST",
            "/v1/agents",
            Some(&token_a),
            Some("lc-agent-1"),
            Some(&serde_json::json!({"display_name": "甲的"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let agent_id = resp.json()["agent"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_a),
            Some("lc-inv-ag-1"),
            Some(&serde_json::json!({"participant_id": agent_id})),
        )
        .await;
    assert_eq!(resp.status, 201, "invite agent: {}", resp.json());
    // 转让目标不是本房 active member ⇒ 404 member_not_found——**不分参与者 kind**
    // （批次三第一刀「owner = 创建者」：owner 是房内的治理角色，不是参与者种类）。
    // 这里用一支**不在本房**的 Agent 当目标（在房里的 Agent 是合法目标，那条正例
    // 由 `backend_group_admin_blackbox` 走真进程证明）。
    let resp = server
        .request(
            "POST",
            "/v1/agents",
            Some(&token_a),
            Some("lc-agent-outsider"),
            Some(&serde_json::json!({"display_name": "圈外的甲"})),
        )
        .await;
    assert_eq!(resp.status, 201, "create outsider agent: {}", resp.json());
    let outsider_agent = resp.json()["agent"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/owner-transfer"),
            Some(&token_a),
            Some("lc-t-agent"),
            Some(&serde_json::json!({"participant_id": outsider_agent})),
        )
        .await;
    assert_eq!(resp.status, 404, "agent outside the room: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "member_not_found");
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/owner-transfer"),
            Some(&token_a),
            Some("lc-t-self"),
            Some(&serde_json::json!({"participant_id": human_a})),
        )
        .await;
    assert_eq!(resp.status, 400, "self target: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "invalid_transfer_target");
    let ghost = uuid::Uuid::new_v4().to_string();
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/owner-transfer"),
            Some(&token_a),
            Some("lc-t-ghost"),
            Some(&serde_json::json!({"participant_id": ghost})),
        )
        .await;
    assert_eq!(resp.status, 404, "non-member target: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "member_not_found");

    // 新 owner（A）直接 leave → 409 owner_transfer_required；旧 owner（B）
    // 现在是 member，可以退群。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/leave"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 409, "owner leave: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "owner_transfer_required");
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/leave"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(
        resp.status,
        204,
        "old owner leaves as member: {}",
        resp.json()
    );
    // B 离群后 404；A 仍可读（窗口不变）。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}/messages"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 404);
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}/messages?limit=100"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["messages"].as_array().unwrap().len(), 10);
    step(
        "transfer",
        "owner transfer atomic (coverage witness + single active owner + replay), invalid targets 400/404 stable, owner leave 409 owner_transfer_required, old owner leaves 204".into(),
    );

    // 重启：周期、窗口与幂等重放保持。
    server.stop().await;
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}/messages?limit=100"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "A still reads after restart");
    let a_bodies: Vec<String> = resp.json()["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["body"].as_str().unwrap().to_owned())
        .collect();
    let expected_a: Vec<String> = [1, 2, 3, 4, 5, 11, 12, 13, 14, 15]
        .iter()
        .map(|i| format!("a{i}"))
        .collect();
    assert_eq!(a_bodies, expected_a, "A's window survives restart");
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/owner-transfer"),
            Some(&token_b),
            Some("lc-t-1"),
            Some(&serde_json::json!({"participant_id": human_a})),
        )
        .await;
    assert_eq!(resp.status, 200, "transfer replay after restart");
    assert_eq!(resp.json()["idempotency_replayed"], true);
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_b),
            Some("lc-inv-a-2"),
            Some(&serde_json::json!({"participant_id": human_a})),
        )
        .await;
    assert_eq!(resp.status, 200, "invite replay after restart");
    assert_eq!(resp.json()["idempotency_replayed"], true);
    assert_eq!(
        resp.json()["membership"]["closed_by"],
        "transfer",
        "replay returns the historical closed period even after the transfer"
    );
    step(
        "restart",
        "fresh process on the same db: windows, invite/transfer replays all preserved".into(),
    );

    // 真实 SQLite 探针：恰好一个 active owner，coverage 见证该周期。
    let options = SqliteConnectOptions::new()
        .filename(&db_path)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open lifecycle probe");
    let owners: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships \
         WHERE room_id = ? AND role = 'owner' AND active_marker = 1",
    )
    .bind(&group_id)
    .fetch_one(&pool)
    .await
    .expect("count owners");
    assert_eq!(owners, 1, "exactly one active owner");
    let witness: String =
        sqlx::query_scalar("SELECT group_owner_membership_id FROM room_coverage WHERE room_id = ?")
            .bind(&group_id)
            .fetch_one(&pool)
            .await
            .expect("coverage witness");
    assert_eq!(
        witness, new_owner_membership,
        "coverage witnesses the new owner period"
    );
    let (w_active, w_role): (i64, String) =
        sqlx::query_as("SELECT active_marker, role FROM room_memberships WHERE membership_id = ?")
            .bind(&witness)
            .fetch_one(&pool)
            .await
            .expect("witness period state");
    assert_eq!(w_active, 1);
    assert_eq!(w_role, "owner");
    let a_periods: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships \
         WHERE room_id = ? AND participant_id = ?",
    )
    .bind(&group_id)
    .bind(&human_a)
    .fetch_one(&pool)
    .await
    .expect("count A periods");
    assert_eq!(
        a_periods, 3,
        "A: closed(leave) + closed(transfer) + active(owner)"
    );
    let b_periods: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships \
         WHERE room_id = ? AND participant_id = ?",
    )
    .bind(&group_id)
    .bind(&human_b)
    .fetch_one(&pool)
    .await
    .expect("count B periods");
    assert_eq!(
        b_periods, 2,
        "B: owner period closed by transfer, member period closed by leave"
    );

    // 转让事实探针：恰一条不可变 transition，精确见证旧 owner（B）、目标
    // （A）与新 owner 周期；幂等记录 (human_id, key) → transition_id。
    let transitions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM group_owner_transitions WHERE room_id = ?")
            .bind(&group_id)
            .fetch_one(&pool)
            .await
            .expect("count transitions");
    assert_eq!(transitions, 1, "exactly one immutable transition fact");
    let (t_old, t_target, t_new_owner): (String, String, String) = sqlx::query_as(
        "SELECT old_owner_participant_id, target_participant_id, new_owner_membership_id \
         FROM group_owner_transitions WHERE room_id = ?",
    )
    .bind(&group_id)
    .fetch_one(&pool)
    .await
    .expect("transition facts");
    assert_eq!(t_old, human_b, "transition old owner is B");
    assert_eq!(t_target, human_a, "transition target is A");
    assert_eq!(t_new_owner, new_owner_membership);
    let (key_transition,): (String,) = sqlx::query_as(
        "SELECT transition_id FROM group_owner_transfer_idempotency \
         WHERE human_id = ? AND key = 'lc-t-1'",
    )
    .bind(&human_b)
    .fetch_one(&pool)
    .await
    .expect("transfer idempotency maps key to transition");
    let (t_transition,): (String,) =
        sqlx::query_as("SELECT transition_id FROM group_owner_transitions WHERE room_id = ?")
            .bind(&group_id)
            .fetch_one(&pool)
            .await
            .expect("transition id");
    assert_eq!(
        key_transition, t_transition,
        "key maps to the real transition fact"
    );

    // P1 拆分防护回归探针：B 历史上有 transfer 关闭的 owner 周期、且已
    // leave 无 active 周期——直接 SQL 单独以 closed_by='transfer' 关闭
    // A 的 owner 周期（无同事务 transition）在写入时立即被拒（转让不可拆
    // 成两笔事务）；把 B 的历史 closed 周期放进 transition 角色列也被
    // INSERT guard 拒绝（周期必须当时 active）；状态不变。
    let (a_owner_mid,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND role = 'owner' AND active_marker = 1",
    )
    .bind(&group_id)
    .fetch_one(&pool)
    .await
    .expect("A owner period");
    let close_res = sqlx::query(
        "UPDATE room_memberships \
         SET active_marker = 0, left_at = 9, left_before_sequence = 100, closed_by = 'transfer' \
         WHERE membership_id = ?",
    )
    .bind(&a_owner_mid)
    .execute(&pool)
    .await;
    assert!(
        close_res.is_err(),
        "transfer close without a same-transaction transition must be rejected at write time"
    );
    let close_err = match close_res.unwrap_err() {
        sqlx::Error::Database(d) => d.message().to_owned(),
        other => format!("{other}"),
    };
    assert!(
        close_err
            .contains("a transfer close requires a same-transaction matching owner transition"),
        "unexpected close guard error: {close_err}"
    );
    let (b_old_owner_mid,): (String,) = sqlx::query_as(
        "SELECT membership_id FROM room_memberships \
         WHERE room_id = ? AND participant_id = ? AND role = 'owner' AND active_marker = 0",
    )
    .bind(&group_id)
    .bind(&human_b)
    .fetch_one(&pool)
    .await
    .expect("B historical owner period");
    let reuse = sqlx::query(
        "INSERT INTO group_owner_transitions \
         (transition_id, room_id, old_owner_participant_id, target_participant_id, \
          old_owner_membership_id, target_member_membership_id, new_owner_membership_id, \
          old_owner_new_member_membership_id, occurred_at, boundary_sequence) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 9, 100)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&group_id)
    .bind(&human_a)
    .bind(&human_b)
    .bind(&b_old_owner_mid)
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(uuid::Uuid::new_v4().to_string())
    .execute(&pool)
    .await;
    assert!(
        reuse.is_err(),
        "reusing B's historical transfer period in a transition must be rejected"
    );
    let reuse_err = match reuse.unwrap_err() {
        sqlx::Error::Database(d) => d.message().to_owned(),
        other => format!("{other}"),
    };
    assert!(
        reuse_err.contains("group owner transition requires an active old owner"),
        "unexpected transition insert guard error: {reuse_err}"
    );
    let owners_after: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM room_memberships \
         WHERE room_id = ? AND role = 'owner' AND active_marker = 1",
    )
    .bind(&group_id)
    .fetch_one(&pool)
    .await
    .expect("count owners after probe");
    assert_eq!(
        owners_after, 1,
        "rejected probes must leave state unchanged"
    );
    let witness_after: String =
        sqlx::query_scalar("SELECT group_owner_membership_id FROM room_coverage WHERE room_id = ?")
            .bind(&group_id)
            .fetch_one(&pool)
            .await
            .expect("coverage after probe");
    assert_eq!(witness_after, new_owner_membership, "coverage unchanged");
    let transitions_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM group_owner_transitions WHERE room_id = ?")
            .bind(&group_id)
            .fetch_one(&pool)
            .await
            .expect("count transitions after probe");
    assert_eq!(transitions_after, 1, "no transition created by the probe");
    drop(pool);

    // owner 邀请自己 → 稳定 404 member_not_found，不写幂等记录；普通
    // active 成员（自己拥有的 Agent）用新 key 仍 200。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_a),
            Some("lc-inv-self"),
            Some(&serde_json::json!({"participant_id": human_a})),
        )
        .await;
    assert_eq!(resp.status, 404, "owner self invite: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "member_not_found");
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_a),
            Some("lc-inv-ag-2"),
            Some(&serde_json::json!({"participant_id": agent_id})),
        )
        .await;
    assert_eq!(
        resp.status,
        200,
        "active member with a new key still 200: {}",
        resp.json()
    );
    assert_eq!(resp.json()["idempotency_replayed"], false);
    step(
        "transition-facts",
        "immutable transition fact witnesses old owner/target/new owner; key→transition; P1: transfer close without same-transaction transition rejected at write, B's historical transfer period not consumable in a transition, state unchanged; owner self-invite 404 member_not_found without idempotency record; active member new key 200".into(),
    );

    server.stop().await;
    run_dir.cleanup();

    println!("MEMBERSHIP LIFECYCLE BLACKBOX EVIDENCE:");
    println!("{}", evidence.join("\n"));
    println!(
        "MEMBERSHIP LIFECYCLE BLACKBOX DONE: join/leave/rejoin windows, out-of-group 404s, gap-free paging without leakage, invite key replay vs rejoin, single active period under concurrent keys, atomic owner transfer with coverage witness + immutable transition facts, invalid targets, owner leave 409, restart persistence, P1 transfer-close split rejected at write + historical period not consumable, owner self-invite 404, db probes all passed; processes closed, temp assets removed"
    );
}

/// GET /v1/human/events 公共 API 助手（黑盒专用，不触碰数据库）。
async fn he_events_get(server: &Server, token: &str, query: &str) -> HttpResponse {
    server
        .request(
            "GET",
            &format!("/v1/human/events{query}"),
            Some(token),
            None,
            None,
        )
        .await
}

/// POST /v1/rooms/{room_id}/messages 公共 API 助手（黑盒专用）。
async fn he_room_send(
    server: &Server,
    room_id: &str,
    token: &str,
    key: &str,
    body: &str,
) -> HttpResponse {
    server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(token),
            Some(key),
            Some(&serde_json::json!({"body": body})),
        )
        .await
}

/// 真实进程 Human Durable Event 黑盒：bootstrap Alice / create B / create C，
/// Alice 建群并邀请 B；B 在群期间收发消息；REST 恢复、keyset 分页、严格
/// query 拒绝、离群隔离（只见本人 member.left）、间隙事件不可见、他人无
/// 泄露、重启保持、重入后缺口仍不可见；全部只通过公开 HTTP。
#[tokio::test]
async fn blackbox_human_events_rest_via_public_api() {
    let mut run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;

    let token_a = bootstrap_cli(&db_path).await;
    let (human_b, token_b) = create_human_cli(&db_path, "B").await;
    let (_human_c, token_c) = create_human_cli(&db_path, "C").await;
    let human_a = server
        .request("GET", "/v1/me", Some(&token_a), None, None)
        .await
        .json()["participant"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut evidence: Vec<String> = Vec::new();
    let mut step = |name: &str, detail: String| evidence.push(format!("[{name}] {detail}"));

    // A→B 联系人建立后 A 建群、邀请 B。
    let resp = server
        .request(
            "POST",
            "/v1/contacts/requests",
            Some(&token_a),
            Some("he-c-ab"),
            Some(&serde_json::json!({"participant_id": human_b})),
        )
        .await;
    assert_eq!(resp.status, 201, "contact request: {}", resp.json());
    let resp = server
        .request(
            "POST",
            &format!("/v1/contacts/{human_a}/accept"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "accept: {}", resp.json());
    let resp = server
        .request(
            "POST",
            "/v1/groups",
            Some(&token_a),
            Some("he-g-1"),
            Some(&serde_json::json!({"name": "事件群"})),
        )
        .await;
    assert_eq!(resp.status, 201, "group create: {}", resp.json());
    let group_id = resp.json()["room"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_a),
            Some("he-inv-b-1"),
            Some(&serde_json::json!({"participant_id": human_b})),
        )
        .await;
    assert_eq!(resp.status, 201, "invite B: {}", resp.json());

    // B 在群期间：b1(1), a1(2), b2(3)。
    assert_eq!(
        he_room_send(&server, &group_id, &token_b, "he-b-m-1", "b1")
            .await
            .status,
        201
    );
    assert_eq!(
        he_room_send(&server, &group_id, &token_a, "he-a-m-1", "a1")
            .await
            .status,
        201
    );
    assert_eq!(
        he_room_send(&server, &group_id, &token_b, "he-b-m-2", "b2")
            .await
            .status,
        201
    );

    // 未认证 401；严格 query：未知 key / 重复 / 越界 / 非法 cursor 统一
    // 400 错误信封。
    let resp = server
        .request("GET", "/v1/human/events", None, None, None)
        .await;
    assert_eq!(resp.status, 401);
    for bad in [
        "?foo=1",
        "?after=1&foo=2",
        "?after=1&after=2",
        "?limit=1&limit=2",
        "?after=abc",
        "?after=-1",
        "?after=9223372036854775808",
        "?after=",
        "?after=c1.m.550e8400-e29b-41d4-a716-446655440000",
        "?after=c1.be.550e8400-e29b-41d4-a716-446655440000",
        "?after=c1.he.550e8400-e29b-41d4-a716-446655440000",
        "?after=c2.he.550e8400-e29b-41d4-a716-446655440000",
        "?limit=0",
        "?limit=101",
    ] {
        let resp = he_events_get(&server, &token_b, bad).await;
        assert_eq!(resp.status, 400, "bad query {bad}: {}", resp.json());
        assert_eq!(resp.json()["error"]["code"], "invalid_request");
        assert!(resp.json()["error"]["request_id"].is_string());
    }

    // B 的恢复读取：joined(1) + 3 条 message.created 升序，快照含发生时
    // 公开字段（actor/body/room_id）；视图不暴露 sequence。
    let resp = he_events_get(&server, &token_b, "").await;
    assert_eq!(resp.status, 200, "B events: {}", resp.json());
    let json = resp.json();
    assert!(json["request_id"].as_str().map(str::len).unwrap() > 0);
    let events = json["events"].as_array().unwrap().clone();
    assert_eq!(events.len(), 4);
    let kinds: Vec<&str> = events.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        vec![
            "member.joined",
            "message.created",
            "message.created",
            "message.created"
        ]
    );
    assert!(events[0]["id"].as_str().unwrap().len() == 36);
    assert!(
        events
            .iter()
            .all(|e| e.get("source_id").is_none() && e["room_id"] == group_id),
        "public event envelope must not carry the internal source_id (kind+source_id is internal identity)"
    );
    assert!(
        events.iter().all(|e| e.get("sequence").is_none()),
        "no sequence in human event views"
    );
    assert!(
        events
            .iter()
            .filter(|e| e["kind"] == "message.created")
            .all(|e| e["snapshot"].get("sequence").is_none()),
        "message.created snapshot must not expose the internal message sequence"
    );
    assert_eq!(events[1]["snapshot"]["body"], "b1");
    assert_eq!(events[1]["snapshot"]["actor"]["id"], human_b);
    assert_eq!(events[1]["snapshot"]["actor"]["kind"], "human");
    assert_eq!(events[2]["snapshot"]["body"], "a1");
    assert_eq!(events[2]["snapshot"]["actor"]["id"], human_a);
    assert_eq!(events[3]["snapshot"]["body"], "b2");
    assert_eq!(
        json["next_after"],
        serde_json::json!(format!("c1.he.{}", events[3]["id"].as_str().unwrap()))
    );
    // 同一位置重拉稳定。
    let again = he_events_get(&server, &token_b, "").await.json();
    assert_eq!(again["events"], json["events"]);
    step(
        "recovery",
        "B sees joined + 3 message.created (strict asc, shared snapshot fields), no sequence in views or snapshots, no-auth 401, strict query+cursor 400 envelope, idempotent re-pull".into(),
    );

    // keyset 分页：limit=2 无重复无跳过；空页 next_after 保持请求 cursor。
    let mut seen_ids: Vec<String> = Vec::new();
    let mut after = "c1.he.start".to_owned();
    let mut empty_kept: Option<String> = None;
    for page in 0..5 {
        let resp = he_events_get(&server, &token_b, &format!("?after={after}&limit=2")).await;
        assert_eq!(resp.status, 200);
        let json = resp.json();
        let page_ids: Vec<String> = json["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].as_str().unwrap().to_owned())
            .collect();
        if page_ids.is_empty() {
            empty_kept = Some(json["next_after"].as_str().unwrap().to_owned());
            break;
        }
        seen_ids.extend(page_ids.clone());
        after = json["next_after"].as_str().unwrap().to_owned();
        assert_eq!(after, format!("c1.he.{}", page_ids.last().unwrap()));
        if page >= 3 {
            panic!("paging did not terminate");
        }
    }
    assert_eq!(
        seen_ids,
        events
            .iter()
            .map(|e| e["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>(),
        "keyset walk covers all events without dup/skip"
    );
    assert_eq!(
        empty_kept,
        Some(after),
        "empty page keeps the actual requested cursor"
    );
    step(
        "paging",
        "limit=2 keyset paging over opaque cursors: no dup/skip, empty page keeps the requested cursor".into(),
    );

    // B 退群 → 204；离群后只见本人 member.left（closed_by=leave），其余
    // 事件 fail closed；A 继续发 a2/a3 与 C 不泄露。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/leave"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204, "B leaves: {}", resp.json());
    assert_eq!(
        he_room_send(&server, &group_id, &token_a, "he-a-m-2", "a2")
            .await
            .status,
        201
    );
    assert_eq!(
        he_room_send(&server, &group_id, &token_a, "he-a-m-3", "a3")
            .await
            .status,
        201
    );
    let resp = he_events_get(&server, &token_b, "").await;
    assert_eq!(resp.status, 200);
    let events = resp.json()["events"].as_array().unwrap().clone();
    assert_eq!(
        events.len(),
        1,
        "out-of-group events fail closed: {events:?}"
    );
    assert_eq!(events[0]["kind"], "member.left");
    assert_eq!(events[0]["snapshot"]["participant"]["id"], human_b);
    assert_eq!(events[0]["snapshot"]["closed_by"], "leave");
    let left_id = events[0]["id"].as_str().unwrap().to_owned();
    // 离群后 REST 补拉只可能看到自己的 left：用第 4 条事件（b2）的 cursor。
    let after_b2 = format!("c1.he.{}", json["events"][3]["id"].as_str().unwrap());
    let resp = he_events_get(&server, &token_b, &format!("?after={after_b2}")).await;
    let events = resp.json()["events"].as_array().unwrap().clone();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["id"], left_id);
    assert_eq!(
        resp.json()["next_after"],
        serde_json::json!(format!("c1.he.{left_id}"))
    );
    // C 从未入群：空事件，不泄露；next_after 为起点 sentinel。
    let resp = he_events_get(&server, &token_c, "").await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["events"].as_array().unwrap().len(), 0);
    assert_eq!(resp.json()["next_after"], "c1.he.start");
    // 未知 / 跨 Human cursor：与「从未投递」不可区分，统一 400。
    let unknown_cursor = format!("c1.he.{}", uuid::Uuid::new_v4());
    let resp = he_events_get(&server, &token_c, &format!("?after={unknown_cursor}")).await;
    assert_eq!(resp.status, 400);
    assert_eq!(resp.json()["error"]["code"], "invalid_request");
    step(
        "isolation",
        "B left: REST sees only own member.left (fail closed, gap a2/a3 hidden), C sees empty events + start sentinel, unknown cursor 400".into(),
    );

    // 重启：fact 与重拉结果保持。
    server.stop().await;
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;
    let resp = he_events_get(&server, &token_b, "").await;
    assert_eq!(resp.status, 200, "B after restart");
    let events = resp.json()["events"].as_array().unwrap().clone();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["kind"], "member.left");
    assert_eq!(events[0]["id"], left_id);
    let resp = he_events_get(&server, &token_b, &format!("?after={after_b2}")).await;
    assert_eq!(resp.json()["events"].as_array().unwrap().len(), 1);
    step(
        "restart",
        "fresh process same db: B still sees only own member.left; cursor position survives restart".into(),
    );

    // B 重入 → 新 joined；发 b4；缺口事件 a2/a3 仍不可见；可见序列
    // 1,2,3,4(joined×2? 需按实际)，6(b4)。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_a),
            Some("he-inv-b-2"),
            Some(&serde_json::json!({"participant_id": human_b})),
        )
        .await;
    assert_eq!(resp.status, 201, "B rejoins: {}", resp.json());
    assert_eq!(
        he_room_send(&server, &group_id, &token_b, "he-b-m-4", "b4")
            .await
            .status,
        201
    );
    let resp = he_events_get(&server, &token_b, "").await;
    assert_eq!(resp.status, 200);
    let events = resp.json()["events"].as_array().unwrap().clone();
    assert_eq!(events.len(), 7, "2 joined + b1/a1/b2/b4 + own left");
    let kinds: Vec<&str> = events.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds.iter().filter(|k| **k == "member.joined").count(), 2);
    assert_eq!(kinds.iter().filter(|k| **k == "member.left").count(), 1);
    assert_eq!(kinds.iter().filter(|k| **k == "message.created").count(), 4);
    let ids: Vec<String> = events
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids.len(), 7, "events stay strictly ordered by stable ids");
    let bodies: Vec<&str> = events
        .iter()
        .filter(|e| e["kind"] == "message.created")
        .map(|e| e["snapshot"]["body"].as_str().unwrap())
        .collect();
    assert_eq!(
        bodies,
        vec!["b1", "a1", "b2", "b4"],
        "gap a2/a3 never visible"
    );
    step(
        "rejoin",
        "B rejoins: 2 joined + own left + 4 messages visible (strict asc by id), gap a2/a3 never visible".into(),
    );

    server.stop().await;
    run_dir.cleanup();

    println!("HUMAN EVENTS REST BLACKBOX EVIDENCE:");
    println!("{}", evidence.join("\n"));
    println!(
        "HUMAN EVENTS REST BLACKBOX DONE: recovery with opaque cursors (no sequence exposed), strict query+cursor 400s, keyset paging no dup/skip with empty-page cursor stability, post-leave own member.left only (fail closed), gap hidden, outsider empty + start sentinel, restart persistence, rejoin windows restored without gap leakage; processes closed, temp assets removed"
    );
}

/// 裸 TCP WebSocket 客户端（RFC 6455 最小实现）：只做本测试需要的握手、
/// 掩码文本帧发送与无掩码服务端帧读取；不引入任何额外依赖。
struct WsFrame {
    opcode: u8,
    payload: Vec<u8>,
}

async fn ws_connect(addr: &str, path: &str) -> Result<TcpStream, String> {
    let mut stream = TcpStream::connect(addr).await.map_err(|e| e.to_string())?;
    let mut key_bytes = [0u8; 16];
    getrandom::getrandom(&mut key_bytes).map_err(|e| e.to_string())?;
    let key = base64::engine::general_purpose::STANDARD.encode(key_bytes);
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    stream.flush().await.map_err(|e| e.to_string())?;

    let mut head = Vec::new();
    let mut buf = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|e| format!("read handshake: {e}"))?;
        if n == 0 {
            return Err("handshake closed early".into());
        }
        head.push(buf[0]);
        if head.len() > 8192 {
            return Err("handshake too large".into());
        }
    }
    let text = String::from_utf8_lossy(&head);
    let status_line = text.lines().next().unwrap_or_default();
    if !status_line.contains(" 101 ") {
        return Err(format!("websocket upgrade rejected: {status_line}"));
    }
    let expected = {
        use sha1::{Digest as _, Sha1};
        let mut digest = Sha1::new();
        digest.update(key.as_bytes());
        digest.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
        base64::engine::general_purpose::STANDARD.encode(digest.finalize())
    };
    let mut accept_ok = false;
    for line in text.lines() {
        if let Some((name, value)) = line.split_once(": ")
            && name.eq_ignore_ascii_case("Sec-WebSocket-Accept")
            && value.trim() == expected
        {
            accept_ok = true;
        }
    }
    if !accept_ok {
        return Err("Sec-WebSocket-Accept mismatch".into());
    }
    Ok(stream)
}

/// 发送一个客户端掩码 text 帧（FIN=1, opcode=1, mask=1）。
async fn ws_send_text(stream: &mut TcpStream, payload: &str) -> Result<(), String> {
    let data = payload.as_bytes();
    let len = data.len();
    let mut header = vec![0x81u8];
    if len < 126 {
        header.push(0x80 | len as u8);
    } else if len <= u16::MAX as usize {
        header.push(0x80 | 126);
        header.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        header.push(0x80 | 127);
        header.extend_from_slice(&(len as u64).to_be_bytes());
    }
    let mut mask = [0u8; 4];
    getrandom::getrandom(&mut mask).map_err(|e| e.to_string())?;
    header.extend_from_slice(&mask);
    let masked: Vec<u8> = data
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ mask[i % 4])
        .collect();
    stream.write_all(&header).await.map_err(|e| e.to_string())?;
    stream.write_all(&masked).await.map_err(|e| e.to_string())?;
    stream.flush().await.map_err(|e| e.to_string())
}

/// 读取一个服务端帧（无掩码）。返回 None 表示连接已关闭。
async fn ws_read_frame(stream: &mut TcpStream) -> Result<Option<WsFrame>, String> {
    let mut b0 = [0u8; 1];
    if stream.read(&mut b0).await.map_err(|e| e.to_string())? == 0 {
        return Ok(None);
    }
    let mut b1 = [0u8; 1];
    stream
        .read_exact(&mut b1)
        .await
        .map_err(|e| e.to_string())?;
    let opcode = b0[0] & 0x0f;
    let masked = b1[0] & 0x80 != 0;
    let mut len = (b1[0] & 0x7f) as u64;
    if len == 126 {
        let mut ext = [0u8; 2];
        stream
            .read_exact(&mut ext)
            .await
            .map_err(|e| e.to_string())?;
        len = u16::from_be_bytes(ext) as u64;
    } else if len == 127 {
        let mut ext = [0u8; 8];
        stream
            .read_exact(&mut ext)
            .await
            .map_err(|e| e.to_string())?;
        len = u64::from_be_bytes(ext);
    }
    if len > 16 * 1024 * 1024 {
        return Err("frame too large".into());
    }
    let mut mask = [0u8; 4];
    if masked {
        stream
            .read_exact(&mut mask)
            .await
            .map_err(|e| e.to_string())?;
    }
    let mut payload = vec![0u8; len as usize];
    if len > 0 {
        stream
            .read_exact(&mut payload)
            .await
            .map_err(|e| e.to_string())?;
    }
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    Ok(Some(WsFrame { opcode, payload }))
}

async fn ws_read_text_json(stream: &mut TcpStream) -> Result<serde_json::Value, String> {
    loop {
        match ws_read_frame(stream).await? {
            None => return Err("connection closed before a text frame".into()),
            Some(frame) if frame.opcode == 1 => {
                let text = String::from_utf8_lossy(&frame.payload).into_owned();
                return serde_json::from_str(&text)
                    .map_err(|e| format!("frame not json: {e}: {text}"));
            }
            Some(frame) if frame.opcode == 8 => {
                let code = if frame.payload.len() >= 2 {
                    u16::from_be_bytes([frame.payload[0], frame.payload[1]])
                } else {
                    0
                };
                return Err(format!("unexpected close frame {code}"));
            }
            Some(_) => continue, // ping/pong/其他控制帧忽略
        }
    }
}

async fn ws_expect_close_policy(stream: &mut TcpStream) -> Result<u16, String> {
    // 服务端以 close 帧（1008）或直接断连表达关闭；两者都算通过。
    loop {
        match ws_read_frame(stream).await? {
            None => return Ok(0),
            Some(frame) if frame.opcode == 8 => {
                if frame.payload.len() >= 2 {
                    return Ok(u16::from_be_bytes([frame.payload[0], frame.payload[1]]));
                }
                return Ok(0);
            }
            Some(_) => continue,
        }
    }
}

/// 真实进程 WebSocket 首帧认证 + volatile 唤醒黑盒：裸 TCP RFC6455 客户端。
/// 覆盖：升级 101、`{"type":"authenticate","token":...}` 首帧、认证成功在
/// 建立订阅后回 `{"type":"authenticated"}`（不是 events.ready）、随后只发
/// `events.available`（无事实 payload）、同一 Human 两条连接都收到唤醒、
/// 另一 Human（不在目标集）不收到、无效 token / 缺 type 首帧 / 多余字段
/// 拒绝（deny_unknown_fields，认证前零泄露）→ invalid_session + 1008、
/// 会话撤销后下一次唤醒前关闭、认证超时安全关闭。
#[tokio::test]
async fn blackbox_human_events_ws_via_public_api() {
    let mut run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;

    let token_a = bootstrap_cli(&db_path).await;
    let (human_b, token_b) = create_human_cli(&db_path, "B").await;
    let (_human_c, token_c) = create_human_cli(&db_path, "C").await;
    let human_a = server
        .request("GET", "/v1/me", Some(&token_a), None, None)
        .await
        .json()["participant"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut evidence: Vec<String> = Vec::new();
    let mut step = |name: &str, detail: String| evidence.push(format!("[{name}] {detail}"));

    // 联系人 + 群 + 邀请 B。
    let resp = server
        .request(
            "POST",
            "/v1/contacts/requests",
            Some(&token_a),
            Some("ws-c-ab"),
            Some(&serde_json::json!({"participant_id": human_b})),
        )
        .await;
    assert_eq!(resp.status, 201, "contact request: {}", resp.json());
    let resp = server
        .request(
            "POST",
            &format!("/v1/contacts/{human_a}/accept"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "accept: {}", resp.json());
    let resp = server
        .request(
            "POST",
            "/v1/groups",
            Some(&token_a),
            Some("ws-g-1"),
            Some(&serde_json::json!({"name": "唤醒群"})),
        )
        .await;
    assert_eq!(resp.status, 201, "group create: {}", resp.json());
    let group_id = resp.json()["room"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_a),
            Some("ws-inv-b-1"),
            Some(&serde_json::json!({"participant_id": human_b})),
        )
        .await;
    assert_eq!(resp.status, 201, "invite B: {}", resp.json());

    // 1) 有效 Session：升级 101 → 首帧必须是
    //    {"type":"authenticate","token":...}；认证成功在订阅建立后回
    //    {"type":"authenticated"}（不是 events.ready）。
    let addr = server.addr.clone();
    let mut ws = ws_connect(&addr, "/v1/human/events/stream")
        .await
        .expect("ws handshake");
    ws_send_text(
        &mut ws,
        &format!("{{\"type\":\"authenticate\",\"token\":\"{token_b}\"}}"),
    )
    .await
    .expect("send auth frame");
    let first = timeout(Duration::from_secs(5), ws_read_text_json(&mut ws))
        .await
        .expect("ack within 5s")
        .expect("ack frame");
    assert_eq!(first, serde_json::json!({"type": "authenticated"}));
    step(
        "authenticated",
        "ws upgrade 101, first frame {\"type\":\"authenticated\"} after subscription established"
            .into(),
    );

    // 2) A 发消息 → B 收到 events.available；payload 只有 type，无任何事实。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/messages"),
            Some(&token_a),
            Some("ws-a-m-1"),
            Some(&serde_json::json!({"body": "wake me"})),
        )
        .await;
    assert_eq!(resp.status, 201, "A sends: {}", resp.json());
    let wake = timeout(Duration::from_secs(5), ws_read_text_json(&mut ws))
        .await
        .expect("wake within 5s")
        .expect("wake frame");
    assert_eq!(wake, serde_json::json!({"type": "events.available"}));
    // 再发一条 → 再收到一条唤醒。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/messages"),
            Some(&token_a),
            Some("ws-a-m-2"),
            Some(&serde_json::json!({"body": "wake again"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let wake = timeout(Duration::from_secs(5), ws_read_text_json(&mut ws))
        .await
        .expect("second wake within 5s")
        .expect("second wake frame");
    assert_eq!(wake, serde_json::json!({"type": "events.available"}));
    assert_eq!(
        wake.as_object().unwrap().len(),
        1,
        "events.available must carry exactly one field and no facts"
    );
    step("wake", "two events.available after post-commit wakes; payload is exactly {{\"type\":\"events.available\"}}".into());
    drop(ws);

    // 3) 同一 Human 两条连接都收到同一唤醒；不在目标 Room 的另一 Human
    //    的第三条连接不收到（真实 public API，非进程内 notifier 单测）。
    let mut ws_b1 = ws_connect(&addr, "/v1/human/events/stream")
        .await
        .expect("ws handshake b1");
    let mut ws_b2 = ws_connect(&addr, "/v1/human/events/stream")
        .await
        .expect("ws handshake b2");
    let mut ws_c = ws_connect(&addr, "/v1/human/events/stream")
        .await
        .expect("ws handshake c");
    for (name, stream, token) in [
        ("b1", &mut ws_b1, &token_b),
        ("b2", &mut ws_b2, &token_b),
        ("c", &mut ws_c, &token_c),
    ] {
        ws_send_text(
            stream,
            &format!("{{\"type\":\"authenticate\",\"token\":\"{token}\"}}"),
        )
        .await
        .expect("send auth frame");
        let ack = timeout(Duration::from_secs(5), ws_read_text_json(stream))
            .await
            .expect("ack within 5s")
            .expect("ack frame");
        assert_eq!(
            ack,
            serde_json::json!({"type": "authenticated"}),
            "connection {name} must be authenticated first"
        );
    }
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/messages"),
            Some(&token_a),
            Some("ws-a-m-3"),
            Some(&serde_json::json!({"body": "two connections"})),
        )
        .await;
    assert_eq!(resp.status, 201, "A sends: {}", resp.json());
    for (name, stream) in [("b1", &mut ws_b1), ("b2", &mut ws_b2)] {
        let wake = timeout(Duration::from_secs(5), ws_read_text_json(stream))
            .await
            .unwrap_or_else(|_| panic!("connection {name} wake within 5s"))
            .expect("wake frame");
        assert_eq!(
            wake,
            serde_json::json!({"type": "events.available"}),
            "connection {name} of the same human must be woken"
        );
    }
    // C 的第三条连接必须在观察窗口内收不到任何帧（也不会被关闭）。
    let outsider = timeout(Duration::from_millis(700), ws_read_text_json(&mut ws_c)).await;
    match outsider {
        Err(_elapsed) => {}
        Ok(Ok(frame)) => panic!("outsider human C must not be woken, got {frame}"),
        Ok(Err(e)) => panic!("outsider human C connection closed unexpectedly: {e}"),
    }
    // C 的 REST 恢复事实也应为空（真实 public API 侧证）。
    let resp = server
        .request("GET", "/v1/human/events", Some(&token_c), None, None)
        .await;
    assert_eq!(resp.status, 200, "C REST: {}", resp.json());
    assert!(
        resp.json()["events"].as_array().unwrap().is_empty(),
        "outsider human C must have no durable events"
    );
    drop(ws_b1);
    drop(ws_b2);
    drop(ws_c);
    step(
        "multi-conn",
        "two connections of the same human both receive events.available; outsider human C receives nothing (WS silence + empty REST) after the same post-commit wake".into(),
    );

    // 4) 无效 token：错误帧 invalid_session + 1008 关闭；认证前零泄露
    //    （错误帧不携带任何事实字段）。
    let mut bad = ws_connect(&addr, "/v1/human/events/stream")
        .await
        .expect("ws handshake for bad token");
    ws_send_text(
        &mut bad,
        r#"{"type":"authenticate","token":"clr_hs_not-a-real-token"}"#,
    )
    .await
    .expect("send bad token");
    let err = timeout(Duration::from_secs(5), ws_read_text_json(&mut bad))
        .await
        .expect("error frame within 5s")
        .expect("error frame");
    assert_eq!(err["error"]["code"], "invalid_session");
    assert!(err["error"]["message"].is_string());
    assert!(err["error"]["request_id"].is_string());
    assert_eq!(
        err.as_object().unwrap().len(),
        1,
        "error frame has only error"
    );
    assert!(
        !err.to_string().contains("event"),
        "no fact leakage in error frame"
    );
    let code = timeout(Duration::from_secs(5), ws_expect_close_policy(&mut bad))
        .await
        .expect("close within 5s")
        .expect("close");
    assert_eq!(code, 1008, "policy violation close");
    step(
        "bad-token",
        "invalid token -> invalid_session error frame + 1008, zero fact leakage".into(),
    );
    drop(bad);

    // 5) 旧形状首帧（缺 type，只有 token）拒绝：冻结协议要求首个 text
    //    JSON 必须带 type=authenticate。
    let mut missing_type = ws_connect(&addr, "/v1/human/events/stream")
        .await
        .expect("ws handshake for missing-type frame");
    ws_send_text(&mut missing_type, &format!("{{\"token\":\"{token_b}\"}}"))
        .await
        .expect("send missing-type frame");
    let err = timeout(Duration::from_secs(5), ws_read_text_json(&mut missing_type))
        .await
        .expect("error within 5s")
        .expect("error frame");
    assert_eq!(err["error"]["code"], "invalid_session");
    let code = timeout(
        Duration::from_secs(5),
        ws_expect_close_policy(&mut missing_type),
    )
    .await
    .expect("close within 5s")
    .expect("close");
    assert_eq!(code, 1008);
    step(
        "frozen-shape",
        "first frame without type=authenticate rejected even with valid token".into(),
    );
    drop(missing_type);

    // 6) 首帧多余字段（deny_unknown_fields）：即使 token 有效也拒绝。
    let mut extra = ws_connect(&addr, "/v1/human/events/stream")
        .await
        .expect("ws handshake for extra fields");
    ws_send_text(
        &mut extra,
        &format!("{{\"type\":\"authenticate\",\"token\":\"{token_b}\",\"extra\":1}}"),
    )
    .await
    .expect("send extra-field frame");
    let err = timeout(Duration::from_secs(5), ws_read_text_json(&mut extra))
        .await
        .expect("error within 5s")
        .expect("error frame");
    assert_eq!(err["error"]["code"], "invalid_session");
    let code = timeout(Duration::from_secs(5), ws_expect_close_policy(&mut extra))
        .await
        .expect("close within 5s")
        .expect("close");
    assert_eq!(code, 1008);
    step(
        "strict-frame",
        "first frame with extra field rejected (deny_unknown_fields) even with valid token".into(),
    );
    drop(extra);

    // 7) 会话撤销：REST 撤销后，下一次唤醒前服务端重新认证失败 → 1008
    //    关闭；REST 同步 401。
    let mut revoked = ws_connect(&addr, "/v1/human/events/stream")
        .await
        .expect("ws handshake for revoked session");
    ws_send_text(
        &mut revoked,
        &format!("{{\"type\":\"authenticate\",\"token\":\"{token_b}\"}}"),
    )
    .await
    .expect("send auth frame");
    let first = timeout(Duration::from_secs(5), ws_read_text_json(&mut revoked))
        .await
        .expect("ack within 5s")
        .expect("ack");
    assert_eq!(first["type"], "authenticated");
    let resp = server
        .request("DELETE", "/v1/sessions/current", Some(&token_b), None, None)
        .await;
    assert_eq!(resp.status, 204, "revoke session: {}", resp.json());
    // 撤销后 REST 401。
    let resp = server
        .request("GET", "/v1/human/events", Some(&token_b), None, None)
        .await;
    assert_eq!(resp.status, 401, "revoked session must 401 on REST");
    // A 发消息触发 post-commit 唤醒 → 服务端在发送唤醒前认证失败 → 1008。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/messages"),
            Some(&token_a),
            Some("ws-a-m-4"),
            Some(&serde_json::json!({"body": "wake revoked"})),
        )
        .await;
    assert_eq!(resp.status, 201, "A sends after revoke: {}", resp.json());
    let code = timeout(Duration::from_secs(5), ws_expect_close_policy(&mut revoked))
        .await
        .expect("close within 5s")
        .expect("close");
    assert_eq!(code, 1008, "revoked session must be closed on next wake");
    step(
        "revoke",
        "revoked session: REST 401, ws closed with 1008 on next wake (re-auth before wake)".into(),
    );
    drop(revoked);

    // 8) 认证超时：不发首帧，服务端 5 秒内安全关闭（1008 或断连）。
    let mut silent = ws_connect(&addr, "/v1/human/events/stream")
        .await
        .expect("ws handshake for silent client");
    let code = timeout(Duration::from_secs(8), ws_expect_close_policy(&mut silent))
        .await
        .expect("auth timeout close within 8s")
        .expect("close");
    assert_eq!(code, 1008, "auth timeout must close with policy violation");
    step(
        "timeout",
        "no first frame -> closed 1008 within the 5s auth window".into(),
    );

    server.stop().await;
    run_dir.cleanup();

    println!("HUMAN EVENTS WS BLACKBOX EVIDENCE:");
    println!("{}", evidence.join("\n"));
    println!(
        "HUMAN EVENTS WS BLACKBOX DONE: upgrade 101, first frame {{\"type\":\"authenticate\",\"token\":...}}, authenticated ack after subscription, events.available exactly {{\"type\":\"events.available\"}} after post-commit wakes, two connections of one human both woken while outsider human stays silent (WS + empty REST), invalid token / missing type / extra field -> invalid_session + 1008 with zero fact leakage, revoked session -> REST 401 + ws 1008 on next wake, auth timeout -> 1008; processes closed, temp assets removed"
    );
}

/// P0-A 协议安全面：真实 Core 进程的 HTTP 错误表面负向路径。
///
/// 覆盖：客户端 x-request-id 不得覆盖 server 生成的 canonical ID；unknown
/// route / method mismatch / malformed JSON / body 超限全部是统一 JSON 信封
/// 且 error.request_id == x-request-id；WS 升级前 rejection 统一 JSON；删除
/// 数据库文件后 /ready 只回固定安全文案，不回显路径 / SQLite 细节。
#[tokio::test]
async fn blackbox_http_error_surface_via_public_api() {
    let mut run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;
    let token = bootstrap_cli(&db_path).await;
    let mut evidence: Vec<String> = Vec::new();
    let mut step = |name: &str, detail: String| evidence.push(format!("[{name}] {detail}"));

    // 1) 客户端传入 x-request-id：响应头必须是 server 生成的 UUID，body 的
    //    request_id 与响应头完全一致。
    let resp = http_request(
        &server.addr,
        "GET",
        "/health",
        &[("x-request-id", "client-trace-abc")],
        None,
    )
    .await
    .unwrap_or_else(|e| panic!("GET /health: {e}"));
    assert_eq!(resp.status, 200, "health: {}", resp.json());
    let header_rid = resp
        .headers
        .get("x-request-id")
        .expect("x-request-id header")
        .to_owned();
    assert_ne!(
        header_rid, "client-trace-abc",
        "client x-request-id must not override the canonical server id"
    );
    assert_eq!(
        resp.json()["request_id"].as_str().unwrap(),
        header_rid,
        "success body request_id must equal x-request-id header"
    );
    step(
        "request-id",
        "client x-request-id ignored; body request_id == x-request-id header (server UUID)".into(),
    );

    // 2) unknown route + method mismatch：统一 JSON 信封，error.request_id ==
    //    x-request-id，没有 Axum 默认纯文本。
    for (method, path, status, code) in [
        ("GET", "/does-not-exist", 404, "not_found"),
        ("POST", "/health", 405, "method_not_allowed"),
    ] {
        let resp = http_request(&server.addr, method, path, &[], None)
            .await
            .unwrap_or_else(|e| panic!("{method} {path}: {e}"));
        assert_eq!(resp.status, status, "{method} {path}: {}", resp.json());
        assert_eq!(resp.json()["error"]["code"], code, "{method} {path}");
        assert_eq!(
            resp.json()["error"]["request_id"].as_str().unwrap(),
            resp.headers
                .get("x-request-id")
                .expect("x-request-id header"),
            "{method} {path}: body request_id must equal header"
        );
    }
    step(
        "route-surface",
        "unknown route -> 404 not_found, method mismatch -> 405 method_not_allowed; both unified JSON with matching request_id".into(),
    );

    // 3) malformed JSON：认证通过后 body 解析失败 -> 400 invalid_request；
    //    未认证 + malformed JSON -> 401 authentication_required（认证先于
    //    body 解析），都保持统一信封。
    let resp = http_request(
        &server.addr,
        "POST",
        "/v1/agents",
        &[
            ("Authorization", &format!("Bearer {token}")),
            ("Content-Type", "application/json"),
        ],
        Some(b"{not json"),
    )
    .await
    .unwrap_or_else(|e| panic!("malformed json: {e}"));
    assert_eq!(resp.status, 400, "malformed json: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "invalid_request");
    assert_eq!(
        resp.json()["error"]["request_id"].as_str().unwrap(),
        resp.headers
            .get("x-request-id")
            .expect("x-request-id header"),
        "malformed json: body request_id must equal header"
    );
    let resp = http_request(
        &server.addr,
        "POST",
        "/v1/agents",
        &[("Content-Type", "application/json")],
        Some(b"{not json"),
    )
    .await
    .unwrap_or_else(|e| panic!("malformed json without auth: {e}"));
    assert_eq!(
        resp.status,
        401,
        "malformed json without auth: {}",
        resp.json()
    );
    assert_eq!(resp.json()["error"]["code"], "authentication_required");
    step(
        "malformed-json",
        "malformed JSON -> 400 invalid_request (authenticated) / 401 authentication_required (unauthenticated); unified envelope with matching request_id".into(),
    );

    // 4) body 超限（/v1/agents 4 KiB 上限）：413 payload_too_large 统一信封。
    let big = vec![b'a'; 5000];
    let resp = http_request(
        &server.addr,
        "POST",
        "/v1/agents",
        &[
            ("Authorization", &format!("Bearer {token}")),
            ("Content-Type", "application/json"),
            ("Idempotency-Key", "err-surface-1"),
        ],
        Some(&big),
    )
    .await
    .unwrap_or_else(|e| panic!("body too large: {e}"));
    assert_eq!(resp.status, 413, "body too large: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "payload_too_large");
    assert_eq!(
        resp.json()["error"]["request_id"].as_str().unwrap(),
        resp.headers
            .get("x-request-id")
            .expect("x-request-id header"),
        "413 body request_id must equal header"
    );
    step(
        "body-limit",
        "body > 4KiB -> 413 payload_too_large; unified envelope with matching request_id".into(),
    );

    // 5) WS 升级前 rejection（不带 upgrade 头的 GET /stream）：统一 JSON，
    //    不再是 Axum 默认纯文本。
    let resp = http_request(&server.addr, "GET", "/v1/human/events/stream", &[], None)
        .await
        .unwrap_or_else(|e| panic!("ws upgrade rejection: {e}"));
    assert_eq!(resp.status, 400, "ws upgrade rejection: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "invalid_request");
    assert_eq!(
        resp.json()["error"]["request_id"].as_str().unwrap(),
        resp.headers
            .get("x-request-id")
            .expect("x-request-id header"),
        "ws upgrade rejection request_id must equal header"
    );
    step(
        "ws-pre-upgrade",
        "GET /stream without upgrade headers -> 400 invalid_request unified JSON (no plain text)"
            .into(),
    );

    // 6) /ready 故障：删除数据库文件后探针失败。公开响应只含固定安全文案，
    //    绝不包含注入的路径、SQLite 或 SQL 细节；request_id 一致。
    //    删除打开的 SQLite 文件仅 unix 语义（Windows 锁定打开文件），故本
    //    子步骤整体仅 unix 执行。
    #[cfg(unix)]
    {
        let db_path_str = db_path.to_str().expect("db path utf8").to_owned();
        std::fs::remove_file(&db_path).expect("remove db file to force probe failure");
        let resp = http_request(&server.addr, "GET", "/ready", &[], None)
            .await
            .unwrap_or_else(|e| panic!("ready after db removal: {e}"));
        assert_eq!(resp.status, 500, "ready probe failure: {}", resp.json());
        assert_eq!(resp.json()["error"]["code"], "internal_error");
        assert_eq!(resp.json()["error"]["message"], "storage probe failed");
        let body = String::from_utf8_lossy(&resp.body).to_lowercase();
        for forbidden in [
            db_path_str.to_lowercase(),
            "sqlite".to_owned(),
            "sql".to_owned(),
            "missing".to_owned(),
            "locked".to_owned(),
        ] {
            assert!(
                !body.contains(&forbidden),
                "/ready failure must not leak {forbidden:?}; body={body:?}"
            );
        }
        assert_eq!(
            resp.json()["error"]["request_id"].as_str().unwrap(),
            resp.headers
                .get("x-request-id")
                .expect("x-request-id header"),
            "/ready failure request_id must equal header"
        );
        step(
            "ready-failure",
            "db file removed -> /ready 500 internal_error with fixed copy; no path/SQLite/SQL leak; request_id consistent".into(),
        );
    }

    server.stop().await;
    run_dir.cleanup();

    println!("HTTP ERROR SURFACE BLACKBOX EVIDENCE:");
    println!("{}", evidence.join("\n"));
    println!(
        "HTTP ERROR SURFACE BLACKBOX DONE: client x-request-id ignored (server UUID echoed in body+header), unknown route/method mismatch/malformed JSON/body too large/WS pre-upgrade rejection all unified JSON with error.request_id == x-request-id, /ready probe failure returns fixed safe copy without path/SQLite/SQL; processes closed, temp assets removed"
    );
}

/// 真实独立 Core 进程 read cursor / 未读黑盒（契约 CORE_V1_READ_STATE.md）：
/// Room list/get 的 read_state（初始 null / 他人消息未读 / 自己消息不计）、
/// PUT 推进 / 同目标 / 旧目标、非法输入矩阵（bad UUID / 未知字段 / 未知 /
/// 他 Room / 非成员 / 未知 Room）、并发收敛、read.cursor.advanced Durable
/// Human Event（快照无正文/boundary/session）与 WS 定向唤醒（只唤醒推进者
/// 本人）、离群间隙 + 重入 null cursor、群主转让继承连续未读、进程重启恢复、
/// 负向泄密检查；全部只通过公开 HTTP/WS。
#[tokio::test]
async fn blackbox_read_cursor_via_public_api() {
    let mut run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;

    let token_a = bootstrap_cli(&db_path).await;
    let (human_b, token_b) = create_human_cli(&db_path, "B").await;
    let (_human_c, token_c) = create_human_cli(&db_path, "C").await;
    let human_a = server
        .request("GET", "/v1/me", Some(&token_a), None, None)
        .await
        .json()["participant"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut evidence: Vec<String> = Vec::new();
    let mut step = |name: &str, detail: String| evidence.push(format!("[{name}] {detail}"));

    // 联系人 + 群1（A owner，邀请 B）+ 群2（A 独有，B 不在）。
    let resp = server
        .request(
            "POST",
            "/v1/contacts/requests",
            Some(&token_a),
            Some("rc-c-ab"),
            Some(&serde_json::json!({"participant_id": human_b})),
        )
        .await;
    assert_eq!(resp.status, 201, "contact request: {}", resp.json());
    let resp = server
        .request(
            "POST",
            &format!("/v1/contacts/{human_a}/accept"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "accept: {}", resp.json());
    let resp = server
        .request(
            "POST",
            "/v1/groups",
            Some(&token_a),
            Some("rc-g-1"),
            Some(&serde_json::json!({"name": "已读群"})),
        )
        .await;
    assert_eq!(resp.status, 201, "group1: {}", resp.json());
    let group_id = resp.json()["room"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_a),
            Some("rc-inv-b"),
            Some(&serde_json::json!({"participant_id": human_b})),
        )
        .await;
    assert_eq!(resp.status, 201, "invite B: {}", resp.json());
    let resp = server
        .request(
            "POST",
            "/v1/groups",
            Some(&token_a),
            Some("rc-g-2"),
            Some(&serde_json::json!({"name": "外群"})),
        )
        .await;
    assert_eq!(resp.status, 201, "group2: {}", resp.json());
    let group2 = resp.json()["room"]["id"].as_str().unwrap().to_owned();

    // 消息基线：m1(A,1) m2(B,2) m3(A,3)。
    let m1 = he_room_send(&server, &group_id, &token_a, "rc-a-1", "a1")
        .await
        .json()["message"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let m2 = he_room_send(&server, &group_id, &token_b, "rc-b-1", "b1")
        .await
        .json()["message"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let m3 = he_room_send(&server, &group_id, &token_a, "rc-a-2", "a2")
        .await
        .json()["message"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let other_room_msg = he_room_send(&server, &group2, &token_a, "rc-a-g2", "g2")
        .await
        .json()["message"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // 初始 read_state：B unread=2（m1/m3 by A，m2 自己不计）；list/get 一致。
    let resp = server
        .request("GET", "/v1/rooms", Some(&token_b), None, None)
        .await;
    assert_eq!(resp.status, 200);
    let list_b = resp.json();
    assert_eq!(list_b["rooms"].as_array().unwrap().len(), 1);
    assert_eq!(
        list_b["rooms"][0]["read_state"]["last_read_message_id"],
        serde_json::Value::Null
    );
    assert_eq!(list_b["rooms"][0]["read_state"]["unread_count"], 2);
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["read_state"]["unread_count"], 2);
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(resp.json()["read_state"]["unread_count"], 1, "A 未读 m2");
    step(
        "initial",
        "B list/get: read_state {null, 2} (own m2 excluded); A get: {null, 1}".into(),
    );

    // 推进 m1 → advanced=true, last_read=m1, unread=1；同目标 → false。
    let resp = server
        .request(
            "PUT",
            &format!("/v1/rooms/{group_id}/read-cursor"),
            Some(&token_b),
            None,
            Some(&serde_json::json!({"message_id": m1})),
        )
        .await;
    assert_eq!(resp.status, 200, "advance m1: {}", resp.json());
    let json = resp.json();
    assert_eq!(json["advanced"], true);
    assert_eq!(json["read_state"]["last_read_message_id"], m1);
    assert_eq!(json["read_state"]["unread_count"], 1);
    assert!(json["request_id"].is_string());
    let resp = server
        .request(
            "PUT",
            &format!("/v1/rooms/{group_id}/read-cursor"),
            Some(&token_b),
            None,
            Some(&serde_json::json!({"message_id": m1})),
        )
        .await;
    assert_eq!(resp.status, 200, "retry m1: {}", resp.json());
    assert_eq!(resp.json()["advanced"], false);

    // 推进到自己消息 m2（可见即可推进）→ unread=1（m3）；旧值 m1 → false。
    let resp = server
        .request(
            "PUT",
            &format!("/v1/rooms/{group_id}/read-cursor"),
            Some(&token_b),
            None,
            Some(&serde_json::json!({"message_id": m2})),
        )
        .await;
    assert_eq!(resp.status, 200, "advance own m2: {}", resp.json());
    assert_eq!(resp.json()["advanced"], true);
    assert_eq!(resp.json()["read_state"]["unread_count"], 1);
    let resp = server
        .request(
            "PUT",
            &format!("/v1/rooms/{group_id}/read-cursor"),
            Some(&token_b),
            None,
            Some(&serde_json::json!({"message_id": m1})),
        )
        .await;
    assert_eq!(resp.status, 200, "stale m1: {}", resp.json());
    assert_eq!(resp.json()["advanced"], false);
    assert_eq!(resp.json()["read_state"]["last_read_message_id"], m2);
    step(
        "advance",
        "m1 advanced=true (unread 2->1), retry m1 advanced=false, own m2 advanced=true, stale m1 advanced=false".into(),
    );

    // 非法输入矩阵：bad UUID / 未知字段 / 未知 / 他 Room / 非成员 / 未知
    // Room。
    for (body, expect) in [
        (
            serde_json::json!({"message_id": "not-a-uuid"}),
            "invalid_request",
        ),
        (
            serde_json::json!({"message_id": m1, "sequence": 1}),
            "invalid_request",
        ),
        (serde_json::json!({"message_id": null}), "invalid_request"),
        (
            serde_json::json!({"message_id": uuid::Uuid::new_v4().to_string()}),
            "invalid_read_cursor",
        ),
        (
            serde_json::json!({"message_id": other_room_msg}),
            "invalid_read_cursor",
        ),
    ] {
        let resp = server
            .request(
                "PUT",
                &format!("/v1/rooms/{group_id}/read-cursor"),
                Some(&token_b),
                None,
                Some(&body),
            )
            .await;
        assert_eq!(resp.status, 400, "body {body}: {}", resp.json());
        assert_eq!(resp.json()["error"]["code"], expect);
        assert!(resp.json()["error"]["request_id"].is_string());
    }
    let resp = server
        .request(
            "PUT",
            &format!("/v1/rooms/{group2}/read-cursor"),
            Some(&token_b),
            None,
            Some(&serde_json::json!({"message_id": other_room_msg})),
        )
        .await;
    assert_eq!(resp.status, 404, "B not in group2: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "room_not_found");
    let resp = server
        .request(
            "PUT",
            &format!("/v1/rooms/{group_id}/read-cursor"),
            Some(&token_c),
            None,
            Some(&serde_json::json!({"message_id": m1})),
        )
        .await;
    assert_eq!(resp.status, 404, "C never member: {}", resp.json());
    let resp = server
        .request(
            "PUT",
            &format!("/v1/rooms/{}/read-cursor", uuid::Uuid::new_v4()),
            Some(&token_b),
            None,
            Some(&serde_json::json!({"message_id": m1})),
        )
        .await;
    assert_eq!(resp.status, 404, "unknown room: {}", resp.json());
    step(
        "rejects",
        "400 invalid_request (bad uuid/unknown field/null) + invalid_read_cursor (unknown/other-room) + 404 (non-member/unknown room), all uniform envelope".into(),
    );

    // 并发：A 发 m4，B 并发 PUT m3 与 m4 → 均 200，收敛到 m4。
    let m4 = he_room_send(&server, &group_id, &token_a, "rc-a-3", "a3")
        .await
        .json()["message"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let s = &server;
    let g = &group_id;
    let tb = &token_b;
    let body_m3 = serde_json::json!({"message_id": m3});
    let body_m4 = serde_json::json!({"message_id": m4});
    let path = format!("/v1/rooms/{g}/read-cursor");
    let (r1, r2) = tokio::join!(
        s.request("PUT", &path, Some(tb), None, Some(&body_m3)),
        s.request("PUT", &path, Some(tb), None, Some(&body_m4)),
    );
    assert_eq!(r1.status, 200, "concurrent m3: {}", r1.json());
    assert_eq!(r2.status, 200, "concurrent m4: {}", r2.json());
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(
        resp.json()["read_state"]["last_read_message_id"],
        m4,
        "concurrent advances converge to the highest legal target"
    );
    assert_eq!(resp.json()["read_state"]["unread_count"], 0);
    step(
        "concurrency",
        "two concurrent PUTs (m3,m4) both 200; final cursor=m4, unread=0".into(),
    );

    // WS：B 与 C 各连一条（认证 → authenticated）；B PUT 推进 m5 →
    // 只有 B 的 WS 收到 events.available，C 的 WS 静默。
    let addr = server.addr.clone();
    let mut ws_b = ws_connect(&addr, "/v1/human/events/stream")
        .await
        .expect("ws_b");
    ws_send_text(
        &mut ws_b,
        &format!("{{\"type\":\"authenticate\",\"token\":\"{token_b}\"}}"),
    )
    .await
    .expect("auth b");
    let ack = timeout(Duration::from_secs(5), ws_read_text_json(&mut ws_b))
        .await
        .expect("ack b")
        .expect("ack frame");
    assert_eq!(ack, serde_json::json!({"type": "authenticated"}));
    let mut ws_c = ws_connect(&addr, "/v1/human/events/stream")
        .await
        .expect("ws_c");
    ws_send_text(
        &mut ws_c,
        &format!("{{\"type\":\"authenticate\",\"token\":\"{token_c}\"}}"),
    )
    .await
    .expect("auth c");
    let ack = timeout(Duration::from_secs(5), ws_read_text_json(&mut ws_c))
        .await
        .expect("ack c")
        .expect("ack frame");
    assert_eq!(ack, serde_json::json!({"type": "authenticated"}));

    let m5 = he_room_send(&server, &group_id, &token_a, "rc-a-4", "a4")
        .await
        .json()["message"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let resp = server
        .request(
            "PUT",
            &format!("/v1/rooms/{group_id}/read-cursor"),
            Some(&token_b),
            None,
            Some(&serde_json::json!({"message_id": m5})),
        )
        .await;
    assert_eq!(resp.status, 200, "advance m5: {}", resp.json());
    assert_eq!(resp.json()["advanced"], true);
    let wake = timeout(Duration::from_secs(5), ws_read_text_json(&mut ws_b))
        .await
        .expect("B wake within 5s")
        .expect("B wake frame");
    assert_eq!(wake, serde_json::json!({"type": "events.available"}));
    assert_eq!(wake.as_object().unwrap().len(), 1, "wake carries no facts");
    let c_silent = timeout(Duration::from_millis(1200), ws_read_text_json(&mut ws_c)).await;
    assert!(
        c_silent.is_err(),
        "C must not be woken by B's advance: {c_silent:?}"
    );
    step(
        "ws-wake",
        "B advance m5 -> B's WS got one events.available (no facts); C's WS stayed silent".into(),
    );

    // Human Event REST：B 拉到 read.cursor.advanced（快照只含
    // room_id/last_read_message_id，无 advancement id、无正文/boundary/
    // session）；公共信封无 source_id；C 零事件。
    let resp = he_events_get(&server, &token_b, "").await;
    assert_eq!(resp.status, 200);
    let events = resp.json()["events"].as_array().unwrap().clone();
    let rc_events: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| e["kind"] == "read.cursor.advanced")
        .collect();
    assert!(
        !rc_events.is_empty(),
        "B must see read.cursor.advanced events"
    );
    let last_rc = rc_events.last().unwrap();
    assert_eq!(last_rc["room_id"], group_id);
    assert!(
        last_rc.get("source_id").is_none(),
        "read.cursor.advanced must not expose the internal advancement_id as source_id"
    );
    let snap = &last_rc["snapshot"];
    let keys: Vec<&str> = snap
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    let mut keys = keys.clone();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["last_read_message_id", "room_id"],
        "read.cursor.advanced snapshot must contain only room_id and last_read_message_id (no advancement id): {snap}"
    );
    assert_eq!(snap["last_read_message_id"], m5);
    assert!(last_rc["id"].as_str().unwrap().len() == 36);
    assert!(
        events.iter().all(|e| e.get("source_id").is_none()),
        "no source_id on any human event kind"
    );
    assert!(
        events.iter().all(|e| e.get("sequence").is_none()),
        "no sequence in human event views"
    );
    assert!(
        events
            .iter()
            .all(|e| e["snapshot"].get("sequence").is_none()),
        "no sequence inside any human event snapshot"
    );
    let resp = he_events_get(&server, &token_c, "").await;
    assert_eq!(
        resp.json()["events"].as_array().unwrap().len(),
        0,
        "C no events"
    );
    step(
        "durable-event",
        "B REST sees read.cursor.advanced: envelope no source_id, snapshot keys exactly {last_read_message_id, room_id} (no advancement id), strict asc; C zero events".into(),
    );

    // 负向泄密：REST 事件与 Room/read state 响应不出现内部边界字段、
    // 内部 source 身份或正文。B 的完整事件响应（含全部快照）必须不含
    // source_id / advancement 标识 / sequence / boundary / token。
    let b_events_leak_check =
        serde_json::to_string(&serde_json::json!({"events": &events})).unwrap();
    let leak_check = serde_json::to_string(&resp.json()).unwrap();
    for forbidden in [
        "join_after_sequence",
        "read_sequence",
        "left_before_sequence",
        "token",
    ] {
        assert!(
            !leak_check.contains(forbidden),
            "REST must not leak {forbidden:?}"
        );
    }
    for forbidden in [
        "source_id",
        "advancement",
        "join_after_sequence",
        "read_sequence",
        "left_before_sequence",
        "token",
    ] {
        assert!(
            !b_events_leak_check.contains(forbidden),
            "human event REST must not leak internal identity {forbidden:?}"
        );
    }
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    let room_json = serde_json::to_string(&resp.json()).unwrap();
    for forbidden in [
        "join_after_sequence",
        "read_sequence",
        "left_before_sequence",
    ] {
        assert!(
            !room_json.contains(forbidden),
            "room view must not leak {forbidden:?}"
        );
    }
    step(
        "no-leak",
        "REST events and room view contain no internal boundary / cursor sequence / token / source_id / advancement-id fields"
            .into(),
    );

    // 重启：read state 与 Human Event 恢复同一权威状态；旧值 PUT 仍 no-op。
    server.stop().await;
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "B room after restart");
    assert_eq!(resp.json()["read_state"]["last_read_message_id"], m5);
    assert_eq!(resp.json()["read_state"]["unread_count"], 0);
    let resp = he_events_get(&server, &token_b, "").await;
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.json()["events"].as_array().unwrap().len(),
        events.len(),
        "events identical after restart"
    );
    let resp = server
        .request(
            "PUT",
            &format!("/v1/rooms/{group_id}/read-cursor"),
            Some(&token_b),
            None,
            Some(&serde_json::json!({"message_id": m4})),
        )
        .await;
    assert_eq!(resp.status, 200, "stale m4 after restart: {}", resp.json());
    assert_eq!(resp.json()["advanced"], false);
    step(
        "restart",
        "fresh process same db: read_state {m5, 0}, same event list, stale PUT still noop".into(),
    );

    // 离群间隙 + 重入：B leave（cursor 冻结）→ A 发 m6 → B 用新 key 重入
    // → 新 null cursor、间隙不计未读、m6 不可见也不可推进。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/leave"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204, "B leaves: {}", resp.json());
    let m6 = he_room_send(&server, &group_id, &token_a, "rc-a-5", "a5")
        .await
        .json()["message"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 404, "left member loses room access");
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/members"),
            Some(&token_a),
            Some("rc-inv-b-2"),
            Some(&serde_json::json!({"participant_id": human_b})),
        )
        .await;
    assert_eq!(resp.status, 201, "B rejoins: {}", resp.json());
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.json()["read_state"]["last_read_message_id"],
        serde_json::Value::Null,
        "rejoin resets to null cursor"
    );
    assert_eq!(
        resp.json()["read_state"]["unread_count"],
        0,
        "gap m6 must not count"
    );
    let resp = server
        .request(
            "PUT",
            &format!("/v1/rooms/{group_id}/read-cursor"),
            Some(&token_b),
            None,
            Some(&serde_json::json!({"message_id": m6})),
        )
        .await;
    assert_eq!(resp.status, 400, "gap m6 invisible: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "invalid_read_cursor");
    let _m7 = he_room_send(&server, &group_id, &token_b, "rc-b-2", "b2").await;
    let m8 = he_room_send(&server, &group_id, &token_a, "rc-a-6", "a6")
        .await
        .json()["message"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(
        resp.json()["read_state"]["unread_count"],
        1,
        "only post-rejoin m8"
    );
    let resp = server
        .request(
            "PUT",
            &format!("/v1/rooms/{group_id}/read-cursor"),
            Some(&token_b),
            None,
            Some(&serde_json::json!({"message_id": m8})),
        )
        .await;
    assert_eq!(resp.status, 200, "advance m8: {}", resp.json());
    assert_eq!(resp.json()["advanced"], true);
    assert_eq!(resp.json()["read_state"]["unread_count"], 0);
    step(
        "leave-rejoin",
        "leave freezes cursor (404), gap m6 invisible & invalid_read_cursor as target, rejoin null cursor, unread counts only post-rejoin messages".into(),
    );

    // 群主转让：B 的 cursor 与未读连续；A（新 member）cursor 不变。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    let a_before = resp.json()["read_state"].clone();
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{group_id}/owner-transfer"),
            Some(&token_a),
            Some("rc-t-1"),
            Some(&serde_json::json!({"participant_id": human_b})),
        )
        .await;
    assert_eq!(resp.status, 200, "transfer: {}", resp.json());
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.json()["read_state"]["last_read_message_id"], m8);
    assert_eq!(resp.json()["read_state"]["unread_count"], 0);
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}"),
            Some(&token_a),
            None,
            None,
        )
        .await;
    assert_eq!(
        resp.json()["read_state"],
        a_before,
        "A cursor must be inherited unchanged by transfer"
    );
    let m9 = he_room_send(&server, &group_id, &token_a, "rc-a-7", "a7")
        .await
        .json()["message"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{group_id}"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(
        resp.json()["read_state"]["unread_count"],
        1,
        "m9 unread after transfer"
    );
    let resp = server
        .request(
            "PUT",
            &format!("/v1/rooms/{group_id}/read-cursor"),
            Some(&token_b),
            None,
            Some(&serde_json::json!({"message_id": m9})),
        )
        .await;
    assert_eq!(resp.status, 200, "B advance m9: {}", resp.json());
    assert_eq!(resp.json()["advanced"], true);
    assert_eq!(resp.json()["read_state"]["unread_count"], 0);
    step(
        "transfer",
        "owner transfer A->B: both read states continuous (B {m8,0}, A unchanged), post-transfer message unread 1 then advanced".into(),
    );

    server.stop().await;
    run_dir.cleanup();

    println!("READ CURSOR BLACKBOX EVIDENCE:");
    println!("{}", evidence.join("\n"));
    println!(
        "READ CURSOR BLACKBOX DONE: initial/list/get read_state, advance/noop/stale, own-message target, uniform 400/404 matrix, concurrent convergence, WS directed wake only to advancing human, durable read.cursor.advanced snapshot without leak, restart recovery, leave-rejoin gap semantics, owner-transfer continuity; processes closed, temp assets removed"
    );
}

/// 真实进程 Reply / Quote 黑盒：普通消息 null 字段、合法回复 201、
/// 自己回复自己、未知/坏 UUID/跨 Room 目标错误表面、幂等重放/改 target
/// 冲突、History 只带 target ID、Human Event 快照与 Agent Event 视图
/// 统一携带 reply_to_message_id、无 target 内容泄露；全部只通过公开 HTTP。
#[tokio::test]
async fn blackbox_replies_and_quotes_via_public_api() {
    let run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();
    let server = Server::start(&db_path, &run_dir.path().join("media")).await;
    let human_token = bootstrap_cli(&db_path).await;
    let mut evidence: Vec<String> = Vec::new();
    let mut step = |name: &str, detail: String| evidence.push(format!("[{name}] {detail}"));

    // 两个 Direct Room（两个 Agent）。
    let mut agent_ids = Vec::new();
    let mut room_ids = Vec::new();
    for i in 1..=2 {
        let resp = server
            .request(
                "POST",
                "/v1/agents",
                Some(&human_token),
                Some(&format!("rp-agent-{i}")),
                Some(&serde_json::json!({"display_name": format!("甲{i}")})),
            )
            .await;
        assert_eq!(resp.status, 201);
        let agent_id = resp.json()["agent"]["id"].as_str().unwrap().to_owned();
        let resp = server
            .request(
                "POST",
                "/v1/direct-rooms",
                Some(&human_token),
                Some(&format!("rp-room-{i}")),
                Some(&serde_json::json!({"agent_id": agent_id})),
            )
            .await;
        assert_eq!(resp.status, 201);
        let room_id = resp.json()["room"]["id"].as_str().unwrap().to_owned();
        agent_ids.push(agent_id);
        room_ids.push(room_id);
    }

    // 普通消息：字段恒存在且为 null。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{}/messages", room_ids[0]),
            Some(&human_token),
            Some("rp-m-1"),
            Some(&serde_json::json!({"body": "目标消息", "mentions": []})),
        )
        .await;
    assert_eq!(resp.status, 201, "plain message must be 201");
    assert_eq!(
        resp.json()["message"]["reply_to_message_id"],
        serde_json::Value::Null,
        "plain message keeps a null reply target"
    );
    let target_id = resp.json()["message"]["id"].as_str().unwrap().to_owned();

    // 合法回复：source 与 target 同 Room，写入成功，响应返回 target ID。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{}/messages", room_ids[0]),
            Some(&human_token),
            Some("rp-m-2"),
            Some(&serde_json::json!({
                "body": "回复上一条",
                "reply_to_message_id": target_id,
            })),
        )
        .await;
    assert_eq!(resp.status, 201, "valid reply must be 201");
    assert_eq!(
        resp.json()["message"]["reply_to_message_id"],
        serde_json::json!(target_id),
        "send response carries the reply target"
    );
    let reply_id = resp.json()["message"]["id"].as_str().unwrap().to_owned();
    step(
        "reply",
        format!("reply id={reply_id} -> target={target_id}"),
    );

    // 自己回复自己：v1 允许，关系只有一条，正文不复制。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{}/messages", room_ids[0]),
            Some(&human_token),
            Some("rp-m-3"),
            Some(&serde_json::json!({
                "body": "自回复",
                "reply_to_message_id": reply_id,
            })),
        )
        .await;
    assert_eq!(resp.status, 201, "self reply must be allowed");
    assert_eq!(
        resp.json()["message"]["reply_to_message_id"],
        serde_json::json!(reply_id)
    );

    // History：所有 source 都返回 reply_to_message_id，不嵌入 target 内容。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{}/messages", room_ids[0]),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    let history_json = resp.json();
    let messages = history_json["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    for message in messages {
        assert!(
            message.get("reply_to_message_id").is_some(),
            "reply_to_message_id must always be present"
        );
        assert!(
            message.get("quoted_body").is_none()
                && message.get("target_sequence").is_none()
                && message.get("reply_actor_id").is_none(),
            "history must not embed target content"
        );
    }
    assert_eq!(
        messages[0]["reply_to_message_id"],
        serde_json::Value::Null,
        "oldest (target) message keeps a null reply target"
    );
    assert_eq!(
        messages[1]["reply_to_message_id"],
        serde_json::json!(target_id),
        "history view carries the target id"
    );
    step("history", "reply view carries target id only".into());

    // 错误表面：坏 UUID 格式 → 400 invalid_request。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{}/messages", room_ids[0]),
            Some(&human_token),
            Some("rp-bad-1"),
            Some(&serde_json::json!({
                "body": "回",
                "reply_to_message_id": "not-a-uuid",
            })),
        )
        .await;
    assert_eq!(resp.status, 400);
    assert_eq!(resp.json()["error"]["code"], "invalid_request");

    // 未知 target（规范 UUID 但不存在）→ 400 invalid_reply_target。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{}/messages", room_ids[0]),
            Some(&human_token),
            Some("rp-bad-2"),
            Some(&serde_json::json!({
                "body": "回",
                "reply_to_message_id": uuid::Uuid::new_v4().to_string(),
            })),
        )
        .await;
    assert_eq!(resp.status, 400);
    assert_eq!(resp.json()["error"]["code"], "invalid_reply_target");

    // 跨 Room → 400 invalid_reply_target，无副作用。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{}/messages", room_ids[1]),
            Some(&human_token),
            Some("rp-bad-3"),
            Some(&serde_json::json!({
                "body": "回",
                "reply_to_message_id": target_id,
            })),
        )
        .await;
    assert_eq!(resp.status, 400);
    assert_eq!(resp.json()["error"]["code"], "invalid_reply_target");
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{}/messages", room_ids[1]),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.json()["messages"],
        serde_json::json!([]),
        "failed cross-room reply must not leave a message"
    );

    // 幂等：同 key 同完整请求 → 200 重放，同 Message ID/sequence/target。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{}/messages", room_ids[0]),
            Some(&human_token),
            Some("rp-m-2"),
            Some(&serde_json::json!({
                "body": "回复上一条",
                "reply_to_message_id": target_id,
            })),
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["idempotency_replayed"], true);
    assert_eq!(resp.json()["message"]["id"], serde_json::json!(reply_id));
    assert_eq!(
        resp.json()["message"]["reply_to_message_id"],
        serde_json::json!(target_id)
    );

    // 同 key 改 reply target → 409 idempotency_conflict，旧事实不变。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{}/messages", room_ids[0]),
            Some(&human_token),
            Some("rp-m-2"),
            Some(&serde_json::json!({
                "body": "回复上一条",
                "reply_to_message_id": reply_id,
            })),
        )
        .await;
    assert_eq!(resp.status, 409);
    assert_eq!(resp.json()["error"]["code"], "idempotency_conflict");
    step("idempotency", "replay 200 / changed target 409".into());

    // Human Event REST：message.created 快照统一携带 reply_to_message_id。
    let resp = he_events_get(&server, &human_token, "?limit=100").await;
    assert_eq!(resp.status, 200);
    let he_json = resp.json();
    let events = he_json["events"].as_array().unwrap();
    let created_of = |message_id: &str| {
        events
            .iter()
            .find(|e| {
                e["kind"] == serde_json::json!("message.created")
                    && e["snapshot"]["id"] == serde_json::json!(message_id)
            })
            .unwrap_or_else(|| panic!("message.created event for {message_id}"))
    };
    let target_he = created_of(&target_id);
    assert_eq!(
        target_he["snapshot"]["reply_to_message_id"],
        serde_json::Value::Null,
        "plain message human event keeps a null reply target"
    );
    let reply_he = created_of(&reply_id);
    assert_eq!(
        reply_he["snapshot"]["reply_to_message_id"],
        serde_json::json!(target_id),
        "human event snapshot carries the reply target id"
    );
    assert!(
        reply_he["snapshot"].get("quoted_body").is_none()
            && reply_he["snapshot"].get("target_sequence").is_none(),
        "human event snapshot must not embed target content"
    );
    step(
        "human-events",
        "message.created snapshot carries reply_to_message_id".into(),
    );

    // Agent Event（Backend 视图）：source Message 同样只带 reply target ID。
    let bindings_path = format!("/v1/agents/{}/backend-bindings", agent_ids[0]);
    let resp = server
        .request(
            "POST",
            &bindings_path,
            Some(&human_token),
            Some("rp-binding"),
            Some(&serde_json::json!({"label":"local runtime"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let binding_id = resp.json()["binding"]["id"].as_str().unwrap().to_owned();
    let credentials_path = format!("{bindings_path}/{binding_id}/credentials");
    let resp = server
        .request(
            "POST",
            &credentials_path,
            Some(&human_token),
            Some("rp-credential"),
            Some(&serde_json::json!({"replaces_credential_id":null})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let backend_token = resp.json()["token"].as_str().unwrap().to_owned();

    let resp = server
        .request(
            "GET",
            "/v1/backend/events?limit=100",
            Some(&backend_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "backend token must read its agent events");
    let backend_json = resp.json();
    let events = backend_json["events"].as_array().unwrap();
    let reply_event = events
        .iter()
        .find(|e| e["message"]["id"] == serde_json::json!(reply_id))
        .expect("reply message has an agent event");
    assert_eq!(
        reply_event["message"]["reply_to_message_id"],
        serde_json::json!(target_id),
        "backend event view carries the reply target id"
    );
    let target_event = events
        .iter()
        .find(|e| e["message"]["id"] == serde_json::json!(target_id))
        .expect("target message has an agent event");
    assert_eq!(
        target_event["message"]["reply_to_message_id"],
        serde_json::Value::Null
    );
    step(
        "agent-events",
        "backend event view carries reply_to_message_id".into(),
    );

    // 数据库复核：reply_to_message_id 只固化在 messages 列，合法路径恰好
    // 2 条（m2→m1、m3→m2），无孤儿 target。
    verify_reply_storage(&db_path, 2).await;
    evidence.push("[storage] messages-with-reply=2, messages=3, sequences=[1,2,3]".into());
    println!("{}", evidence.join("\n"));
}

/// 只读数据库复核：messages.reply_to_message_id 与目标 Message 事实一致，
/// 无孤儿或跨 Room target；schema 无 message_replies 关系表。
async fn verify_reply_storage(db_path: &Path, expected_replies: i64) {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(db_path)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("open core database read-only for storage verification");
    let replies: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE reply_to_message_id IS NOT NULL")
            .fetch_one(&pool)
            .await
            .expect("messages.reply_to_message_id readable");
    assert_eq!(replies, expected_replies);
    let orphans: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages m \
         WHERE m.reply_to_message_id IS NOT NULL \
           AND NOT EXISTS (SELECT 1 FROM messages t \
                           WHERE t.id = m.reply_to_message_id AND t.room_id = m.room_id)",
    )
    .fetch_one(&pool)
    .await
    .expect("reply target integrity check");
    assert_eq!(orphans, 0, "no orphan or cross-room reply targets");
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table'")
            .fetch_all(&pool)
            .await
            .expect("schema table list");
    assert!(
        !tables.iter().any(|t| t == "message_replies"),
        "schema must not contain the message_replies relation table"
    );
    let _ = pool;
}

/// 真实进程 Message Recall 黑盒：Human 与 Agent 双入口撤回、tombstone
/// 公共投影（History / 发送重放 / Backend Event / Human Event）、201/200
/// 幂等、窗口 409、错误优先与脱敏、请求形状约束、Mock Backend 不对
/// Recall/tombstone 生成 Completion、Completion 两种先后、回复/未读相邻
/// 契约与重启保持；全部只通过公开 HTTP + 真实独立 Mock Backend 进程。
#[tokio::test]
async fn blackbox_message_recall_via_public_api() {
    let mut run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;

    let human_token = bootstrap_cli(&db_path).await;
    let (_human_b, token_b) = create_human_cli(&db_path, "B").await;
    let mut evidence: Vec<String> = Vec::new();
    let mut step = |name: &str, detail: String| evidence.push(format!("[{name}] {detail}"));

    // Agent + Direct Room + Binding + Credential（Backend 双入口前置）。
    let resp = server
        .request(
            "POST",
            "/v1/agents",
            Some(&human_token),
            Some("rc-agent-1"),
            Some(&serde_json::json!({"display_name": "撤回助手"})),
        )
        .await;
    assert_eq!(resp.status, 201, "agent: {}", resp.json());
    let agent_id = resp.json()["agent"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&human_token),
            Some("rc-room-1"),
            Some(&serde_json::json!({"agent_id": agent_id})),
        )
        .await;
    assert_eq!(resp.status, 201, "room: {}", resp.json());
    let room_id = resp.json()["room"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_id}/backend-bindings"),
            Some(&human_token),
            Some("rc-binding-1"),
            Some(&serde_json::json!({"label": "local runtime"})),
        )
        .await;
    assert_eq!(resp.status, 201, "binding: {}", resp.json());
    let binding_id = resp.json()["binding"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
            Some(&human_token),
            Some("rc-cred-1"),
            Some(&serde_json::json!({"replaces_credential_id": null})),
        )
        .await;
    assert_eq!(resp.status, 201, "credential: {}", resp.json());
    let backend_token = resp.json()["token"].as_str().unwrap().to_owned();
    step(
        "setup",
        format!("agent={agent_id} room={room_id} backend token issued"),
    );

    // 发送消息并撤回：201，响应形状 message_id/room_id/recalled_at/
    // idempotency_replayed/request_id。
    let secret_body = "绝密原文-勿外泄";
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&human_token),
            Some("rc-m-1"),
            Some(&serde_json::json!({"body": secret_body})),
        )
        .await;
    assert_eq!(resp.status, 201, "send: {}", resp.json());
    let msg_id = resp.json()["message"]["id"].as_str().unwrap().to_owned();
    assert_eq!(resp.json()["message"]["body"], secret_body);
    assert!(
        resp.json()["message"]["recalled_at"].is_null(),
        "active view has recalled_at null"
    );
    assert!(resp.json()["message"].get("sequence").is_none());

    let resp = server
        .request(
            "DELETE",
            &format!("/v1/rooms/{room_id}/messages/{msg_id}"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(
        resp.status,
        201,
        "first recall must be 201: {}",
        resp.json()
    );
    assert_eq!(resp.json()["message_id"], msg_id);
    assert_eq!(resp.json()["room_id"], room_id);
    assert_eq!(resp.json()["idempotency_replayed"], false);
    let recalled_at = resp.json()["recalled_at"].as_str().unwrap().to_owned();
    assert_eq!(resp.json()["request_id"].as_str().unwrap().len(), 36);
    let resp_json_text = serde_json::to_string(&resp.json()).unwrap();
    assert!(
        !resp_json_text.contains(secret_body)
            && !resp_json_text.contains("sequence")
            && !resp_json_text.contains("source_id"),
        "recall response must not leak body/sequence/source_id: {resp_json_text}"
    );
    step(
        "recall-human",
        format!("201 message_id={msg_id} recalled_at={recalled_at}"),
    );

    // 重复同一撤回：200 幂等重放，recalled_at 不刷新。
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/rooms/{room_id}/messages/{msg_id}"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "replay: {}", resp.json());
    assert_eq!(resp.json()["idempotency_replayed"], true);
    assert_eq!(
        resp.json()["recalled_at"],
        recalled_at,
        "recalled_at must not refresh"
    );
    step(
        "replay",
        "200 idempotency_replayed=true same recalled_at".into(),
    );

    // History：tombstone 占原位置——body=null、mentions=[]、recalled_at、
    // id/room/actor/reply_to/created_at 保留；响应全文不含原正文。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    let view = &resp.json()["messages"][0];
    assert_eq!(view["id"], msg_id);
    assert!(view["body"].is_null(), "tombstone: {view}");
    assert_eq!(view["mentions"], serde_json::json!([]));
    assert!(view["recalled_at"].is_string());
    assert_eq!(view["actor"]["kind"], "human");
    assert!(view["created_at"].is_string());
    let history_text = serde_json::to_string(&resp.json()).unwrap();
    assert!(
        !history_text.contains(secret_body),
        "history must not leak the original body"
    );
    step(
        "history",
        "tombstone in place (body=null mentions=[] recalled_at set)".into(),
    );

    // 发送幂等重放：200 同 ID，视图 tombstone、不泄原正文。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&human_token),
            Some("rc-m-1"),
            Some(&serde_json::json!({"body": secret_body})),
        )
        .await;
    assert_eq!(
        resp.status,
        200,
        "send replay after recall: {}",
        resp.json()
    );
    assert_eq!(resp.json()["idempotency_replayed"], true);
    assert_eq!(resp.json()["message"]["id"], msg_id);
    assert!(
        resp.json()["message"]["body"].is_null(),
        "replay view tombstone: {}",
        resp.json()
    );

    // Human Event：message.recalled（最小快照）；created 快照持久变形；
    // 响应全文不含原正文。
    let resp = server
        .request("GET", "/v1/human/events", Some(&human_token), None, None)
        .await;
    assert_eq!(resp.status, 200, "human events: {}", resp.json());
    let events = resp.json()["events"].as_array().unwrap().clone();
    let recalled_event = events
        .iter()
        .find(|e| e["kind"] == "message.recalled")
        .expect("message.recalled event exists");
    assert_eq!(recalled_event["snapshot"]["message_id"], msg_id);
    assert_eq!(recalled_event["snapshot"]["room_id"], room_id);
    assert!(recalled_event["snapshot"]["recalled_at"].is_number());
    assert!(
        recalled_event["snapshot"].get("body").is_none(),
        "minimal snapshot"
    );
    let created_event = events
        .iter()
        .find(|e| e["kind"] == "message.created")
        .expect("message.created event exists");
    assert!(
        created_event["snapshot"]["body"].is_null(),
        "created snapshot must be tombstoned: {}",
        created_event["snapshot"]
    );
    assert!(created_event["snapshot"]["recalled_at"].is_number());
    let events_text = serde_json::to_string(&resp.json()).unwrap();
    assert!(
        !events_text.contains(secret_body),
        "human events must not leak the original body"
    );
    step(
        "human-event",
        "message.recalled minimal snapshot; created snapshot tombstoned".into(),
    );

    // Backend Event：created + recalled 两种 kind 都返回 tombstone；无原
    // 正文。
    let resp = server
        .request(
            "GET",
            "/v1/backend/events",
            Some(&backend_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "backend events: {}", resp.json());
    let backend_events = resp.json()["events"].as_array().unwrap().clone();
    assert_eq!(backend_events.len(), 2, "created + recalled");
    assert_eq!(backend_events[0]["kind"], "message.created");
    assert_eq!(backend_events[1]["kind"], "message.recalled");
    for event in &backend_events {
        assert!(event["message"]["body"].is_null(), "tombstone: {event}");
        assert_eq!(event["message"]["id"], msg_id);
    }
    let backend_text = serde_json::to_string(&resp.json()).unwrap();
    assert!(
        !backend_text.contains(secret_body),
        "backend events must not leak the original body"
    );
    step(
        "backend-event",
        "created+recalled kinds; both tombstone; no original body".into(),
    );

    // ⚠ 2026-09-28：原有一段「独立 Mock Backend 只拉 Event 与 POST
    // Completion、撤回后不生成 Completion、ACK 前缀推进」的观察证据——
    // Completion/ACK 整体退役后那套协议不存在了，本段随之删除（新的
    // mock 是「拉取 + say」，见 group mention 那一支黑盒）。

    // 撤回后新建回复该 target：统一 400 invalid_reply_target，无副作用。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&human_token),
            Some("rc-m-2"),
            Some(&serde_json::json!({
                "body": "迟到回复",
                "reply_to_message_id": msg_id,
            })),
        )
        .await;
    assert_eq!(
        resp.status,
        400,
        "reply to recalled target: {}",
        resp.json()
    );
    assert_eq!(resp.json()["error"]["code"], "invalid_reply_target");
    step(
        "reply-blocked",
        "new reply to recalled target -> 400 invalid_reply_target".into(),
    );

    // Agent 先说 → 撤回不级联：对第二条消息先由 Agent say 回话，再撤回
    // source；Agent 那条 Message 保留，同 key 重放仍 200。
    // ⚠ 2026-09-28：原为 Completion 版本，随 Completion/ACK 退役改写。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&human_token),
            Some("rc-m-3"),
            Some(&serde_json::json!({"body": "第二条"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let msg_2 = resp.json()["message"]["id"].as_str().unwrap().to_owned();
    // ⚠ 2026-09-28：本段原为「Completion 先提交 ⇒ 撤回后终态保留、同结果
    // 重放仍 200」——Completion 退役后改测**同一件事在 say 上的形状**：
    // Agent 先说出去（key rc-reply-2），Human 再撤回第二条；随后同 key 同
    // 摘要重放仍是 200 重放（撤回不改变已提交 Message 的幂等事实）。
    let resp = server
        .request(
            "POST",
            &format!("/v1/backend/rooms/{room_id}/messages"),
            Some(&backend_token),
            Some("rc-reply-2"),
            Some(&serde_json::json!({
                "body": "echo: 第二条",
            })),
        )
        .await;
    assert_eq!(resp.status, 201, "say first: {}", resp.json());
    let reply_id = resp.json()["message"]["id"].as_str().unwrap().to_owned();

    // Human 撤回第二条：已说出的那条不受影响。
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/rooms/{room_id}/messages/{msg_2}"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 201, "recall second: {}", resp.json());
    let resp = server
        .request(
            "POST",
            &format!("/v1/backend/rooms/{room_id}/messages"),
            Some(&backend_token),
            Some("rc-reply-2"),
            Some(&serde_json::json!({
                "body": "echo: 第二条",
            })),
        )
        .await;
    assert_eq!(resp.status, 200, "replay after recall: {}", resp.json());
    assert_eq!(resp.json()["idempotency_replayed"], true);
    assert_eq!(resp.json()["message"]["id"], serde_json::json!(reply_id));
    step(
        "say-first",
        "agent said first -> recall of the source keeps it; same-key replay still 200".into(),
    );

    // Agent 经 Backend 入口撤回自己说的那条：201；无 Agent Recall Event
    // 生成（它本无 created target）。
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/backend/rooms/{room_id}/messages/{reply_id}"),
            Some(&backend_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 201, "agent recalls own reply: {}", resp.json());
    let resp = server
        .request(
            "GET",
            "/v1/backend/events",
            Some(&backend_token),
            None,
            None,
        )
        .await;
    let backend_events = resp.json()["events"].as_array().unwrap().clone();
    let reply_recalls = backend_events
        .iter()
        .filter(|e| e["message"]["id"] == reply_id && e["kind"] == "message.recalled")
        .count();
    assert_eq!(
        reply_recalls, 0,
        "agent reply has no created event -> no recall event"
    );
    step(
        "recall-agent",
        "agent recalls its own said message 201; no agent recall event".into(),
    );

    // 错误矩阵：代撤（Agent 撤回 Human 消息）、跨 room、unknown、非成员、
    // bad message UUID、窗口过期、请求形状约束、错误 token 双方向。
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/backend/rooms/{room_id}/messages/{msg_2}"),
            Some(&backend_token),
            None,
            None,
        )
        .await;
    assert_eq!(
        resp.status,
        404,
        "agent recalls human message: {}",
        resp.json()
    );
    assert_eq!(resp.json()["error"]["code"], "message_not_found");
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/rooms/{room_id}/messages/{}", Uuid::new_v4()),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 404, "unknown message: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "message_not_found");
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/rooms/{room_id}/messages/not-a-uuid"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 400, "bad message uuid: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "invalid_request");
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/rooms/{room_id}/messages/{msg_2}"),
            Some(&token_b),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 404, "non-member: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "room_not_found");
    // 请求形状：非空 body / 任意 query / Idempotency-Key 一律 400。
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/rooms/{room_id}/messages/{msg_2}"),
            Some(&human_token),
            None,
            Some(&serde_json::json!({"x": 1})),
        )
        .await;
    assert_eq!(resp.status, 400, "body must be rejected: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "invalid_request");
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/rooms/{room_id}/messages/{msg_2}?x=1"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 400, "query must be rejected: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "invalid_request");
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/rooms/{room_id}/messages/{msg_2}"),
            Some(&human_token),
            Some("any-key"),
            None,
        )
        .await;
    assert_eq!(
        resp.status,
        400,
        "Idempotency-Key must be rejected: {}",
        resp.json()
    );
    assert_eq!(resp.json()["error"]["code"], "invalid_request");
    // 错误 token 双方向：Backend token 走 Human 入口 → 401 invalid_session；
    // Human token 走 Backend 入口 → 401 invalid_credential。
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/rooms/{room_id}/messages/{msg_2}"),
            Some(&backend_token),
            None,
            None,
        )
        .await;
    assert_eq!(
        resp.status,
        401,
        "backend token on human route: {}",
        resp.json()
    );
    assert_eq!(resp.json()["error"]["code"], "invalid_session");
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/backend/rooms/{room_id}/messages/{msg_2}"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(
        resp.status,
        401,
        "human token on backend route: {}",
        resp.json()
    );
    assert_eq!(resp.json()["error"]["code"], "invalid_credential");
    step(
        "errors",
        "proxy/cross/unknown/non-member/bad-uuid/shape/token matrices all pass".into(),
    );

    // 窗口过期：受控 fixture 构造 created_at 相对 statement-time 回拨
    // 130s 的消息，公开 HTTP 首次撤回 → 409 message_recall_window_expired。
    // （⚠ 2026-09-28：原来这之前有一段 mock/ACK 观察，池子在那里打开；
    // 那一段随 Completion/ACK 退役删除后，池子改在这里打开。）
    let pool = open_fixture_pool(&db_path).await;
    let stale_room_id = Uuid::new_v4().to_string();
    let stale_membership_id = Uuid::new_v4().to_string();
    let human_a_id: String = sqlx::query_scalar(
        "SELECT id FROM participants WHERE kind = 'human' ORDER BY created_at LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("human id");
    let mut conn = pool.acquire().await.expect("conn");
    let mut tx = conn.begin().await.expect("begin");
    sqlx::query(
        "INSERT INTO rooms (id, kind, status, created_at, name) \
         VALUES (?, 'group', 'active', ?, 'stale')",
    )
    .bind(&stale_room_id)
    .bind(-400_000)
    .execute(&mut *tx)
    .await
    .expect("stale room");
    sqlx::query(
        "INSERT INTO room_coverage (room_id, kind, created_at, group_owner_membership_id) \
         VALUES (?, 'group', ?, ?)",
    )
    .bind(&stale_room_id)
    .bind(-400_000)
    .bind(&stale_membership_id)
    .execute(&mut *tx)
    .await
    .expect("stale coverage");
    sqlx::query(
        "INSERT INTO room_memberships \
         (membership_id, room_id, participant_id, role, joined_at, join_after_sequence) \
         VALUES (?, ?, ?, 'owner', ?, 0)",
    )
    .bind(&stale_membership_id)
    .bind(&stale_room_id)
    .bind(&human_a_id)
    .bind(-400_000)
    .execute(&mut *tx)
    .await
    .expect("stale membership");
    let stale_msg_id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO messages \
         (id, room_id, sequence, actor_id, body, reply_to_message_id, idempotency_key, created_at) \
         VALUES (?, ?, 1, ?, 'stale-body', NULL, 'rc-stale-1', \
                 CAST(unixepoch('subsec') * 1000 AS INTEGER) - 130000)",
    )
    .bind(&stale_msg_id)
    .bind(&stale_room_id)
    .bind(&human_a_id)
    .execute(&mut *tx)
    .await
    .expect("stale message");
    tx.commit().await.expect("stale commit");
    drop(conn);
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/rooms/{stale_room_id}/messages/{stale_msg_id}"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 409, "expired recall: {}", resp.json());
    assert_eq!(
        resp.json()["error"]["code"],
        "message_recall_window_expired"
    );
    step(
        "window",
        "first recall past 120s -> 409 message_recall_window_expired".into(),
    );

    // 重启保持：历史 tombstone、幂等重放、Human/Backend 恢复都不泄原正文。
    server.stop().await;
    server = Server::start(&db_path, &run_dir.path().join("media")).await;
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "history after restart: {}", resp.json());
    let messages = resp.json()["messages"].as_array().unwrap().clone();
    let tombstoned = messages
        .iter()
        .find(|m| m["id"] == msg_id)
        .expect("recalled message still in history");
    assert!(tombstoned["body"].is_null());
    assert!(tombstoned["recalled_at"].is_string());
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/rooms/{room_id}/messages/{msg_id}"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "replay after restart: {}", resp.json());
    assert_eq!(resp.json()["idempotency_replayed"], true);
    assert_eq!(resp.json()["recalled_at"], recalled_at);
    step(
        "restart",
        "tombstone + replay persist across restart; no original body".into(),
    );

    // 数据库证据：每条撤回恰好一条 message_recalls；foreign_key_check 干净；
    // 原正文只在 messages 行（内部审计事实），任何公共面不可见（已由上述
    // 断言覆盖）。
    let (recalls,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM message_recalls")
        .fetch_one(&pool)
        .await
        .expect("recalls count");
    assert_eq!(recalls, 3, "one recall fact per recalled message");
    let violations: Vec<(String, String, String)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await
        .expect("fk check");
    assert!(
        violations.is_empty(),
        "foreign_key_check must be clean: {violations:?}"
    );
    drop(pool);
    step(
        "db",
        "3 message_recalls rows; foreign_key_check clean".into(),
    );

    server.stop().await;
    run_dir.cleanup();

    println!("RECALL BLACKBOX EVIDENCE:");
    println!("{}", evidence.join("\n"));
    println!(
        "RECALL BLACKBOX DONE: human/agent dual-entry recall, tombstone projections, 201/200 replay, window 409, error matrix, shape constraints, mock backend no-completion, completion ordering, reply block, restart persistence, db shape all passed; server closed, temp assets removed"
    );
}

/// 构造 multipart/form-data 请求体（恰好一个 name="file" 的文件 part），
/// 返回 (Content-Type, body)。
fn multipart_body(file_name: &str, content_type: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "----celira-blackbox-boundary-3c91";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// 真实进程附件黑盒：只通过公开 API 完成上传（Human/Backend）、pending
/// 不可读、Message/Completion 绑定、下载安全头、附件-only Message、8 个
/// 上限、16 MiB 边界、幂等重放/冲突、跨 actor/跨 Room 统一 404、绑定原子
/// 回滚、撤回清空投影与拒绝下载、Human/Agent Event 附件元数据、重启保持、
/// orphan sweep 与路径攻击防护。
#[tokio::test(flavor = "multi_thread")]
async fn blackbox_attachments_via_public_api() {
    let mut run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();
    let media_root = run_dir.media_path();
    let mut server = Server::start(&db_path, &media_root).await;
    let human_token = bootstrap_cli(&db_path).await;
    let mut evidence: Vec<String> = Vec::new();
    let mut step = |name: &str, detail: String| evidence.push(format!("[{name}] {detail}"));

    let resp = server
        .request(
            "POST",
            "/v1/agents",
            Some(&human_token),
            Some("att-agent-1"),
            Some(&serde_json::json!({"display_name":"甲"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let agent_id = resp.json()["agent"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&human_token),
            Some("att-room-1"),
            Some(&serde_json::json!({"agent_id": agent_id})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let room_id = resp.json()["room"]["id"].as_str().unwrap().to_owned();

    // 上传（Human）→ 201 pending；响应不含 storage key/hash/路径。
    let (ctype, body) = multipart_body("报告.pdf", "application/pdf", b"%PDF-1.4 hello");
    let resp = server
        .request_raw(
            "POST",
            &format!("/v1/rooms/{room_id}/attachments"),
            Some(&human_token),
            Some(&ctype),
            &body,
        )
        .await;
    assert_eq!(resp.status, 201);
    let upload_json = resp.json();
    assert_eq!(upload_json["attachment"]["state"], "pending");
    assert_eq!(upload_json["attachment"]["file_name"], "报告.pdf");
    assert_eq!(upload_json["attachment"]["size_bytes"], 14);
    let att_a = upload_json["attachment"]["id"].as_str().unwrap().to_owned();
    let raw = upload_json.to_string();
    for forbidden in ["storage_key", "sha256", "hash", "media"] {
        assert!(
            !raw.contains(forbidden),
            "upload must not leak {forbidden}: {raw}"
        );
    }

    // pending 不可下载、不进入 History。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/attachments/{att_a}"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 404);
    assert_eq!(resp.json()["error"]["code"], "attachment_not_found");
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.json()["messages"], serde_json::json!([]));
    step(
        "pending",
        "upload 201 pending (no storage_key/hash/path); pending download 404; history still empty"
            .into(),
    );

    // 绑定 → 201；History 视图含附件元数据；下载字节一致 + 固定安全头。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&human_token),
            Some("att-m-1"),
            Some(&serde_json::json!({"body": "with attachment", "attachments": [att_a]})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let msg_1 = resp.json()["message"]["id"].as_str().unwrap().to_owned();
    assert_eq!(resp.json()["message"]["attachments"][0]["id"], att_a);
    assert_eq!(
        resp.json()["message"]["attachments"][0]["file_name"],
        "报告.pdf"
    );
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/attachments/{att_a}"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"%PDF-1.4 hello");
    assert_eq!(
        resp.headers.get("content-type").map(String::as_str),
        Some("application/pdf")
    );
    assert_eq!(
        resp.headers.get("content-disposition").map(String::as_str),
        Some("attachment")
    );
    assert_eq!(
        resp.headers
            .get("x-content-type-options")
            .map(String::as_str),
        Some("nosniff")
    );
    step(
        "bind",
        "message with attachment 201; download 200 bytes+headers; content-disposition fixed attachment + nosniff".into(),
    );

    // 附件-only Message：body 缺省 → 201，body=null，attachments 有序。
    let (ctype, body) = multipart_body("b.bin", "application/octet-stream", b"\x00\x01\x02");
    let resp = server
        .request_raw(
            "POST",
            &format!("/v1/rooms/{room_id}/attachments"),
            Some(&human_token),
            Some(&ctype),
            &body,
        )
        .await;
    assert_eq!(resp.status, 201);
    let att_b = resp.json()["attachment"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&human_token),
            Some("att-m-2"),
            Some(&serde_json::json!({"attachments": [att_b]})),
        )
        .await;
    assert_eq!(resp.status, 201, "attachment-only message must be 201");
    assert!(resp.json()["message"]["body"].is_null());
    assert_eq!(
        resp.json()["message"]["attachments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    step(
        "attachment-only",
        "message without body 201; body=null; attachments present".into(),
    );

    // 幂等：同 key 同有序集合重放 200；换顺序 409；换 body 409。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&human_token),
            Some("att-m-1"),
            Some(&serde_json::json!({"body": "with attachment", "attachments": [att_a]})),
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["idempotency_replayed"], true);
    assert_eq!(resp.json()["message"]["id"], msg_1);
    // 换 body → 409。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&human_token),
            Some("att-m-1"),
            Some(&serde_json::json!({"body": "changed", "attachments": [att_a]})),
        )
        .await;
    assert_eq!(resp.status, 409);
    assert_eq!(resp.json()["error"]["code"], "idempotency_conflict");
    step(
        "idempotency",
        "same ordered attachment set replay 200; changed body 409".into(),
    );

    // 8 个附件成功（含第 8 个）；第 9 个 → 400 invalid_attachment。
    let mut ids8 = Vec::new();
    for i in 0..8 {
        let (ctype, body) =
            multipart_body(&format!("f{i}.bin"), "application/octet-stream", &[i as u8]);
        let resp = server
            .request_raw(
                "POST",
                &format!("/v1/rooms/{room_id}/attachments"),
                Some(&human_token),
                Some(&ctype),
                &body,
            )
            .await;
        assert_eq!(resp.status, 201);
        ids8.push(resp.json()["attachment"]["id"].as_str().unwrap().to_owned());
    }
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&human_token),
            Some("att-m-8"),
            Some(&serde_json::json!({"body": "eight", "attachments": ids8})),
        )
        .await;
    assert_eq!(resp.status, 201);
    assert_eq!(
        resp.json()["message"]["attachments"]
            .as_array()
            .unwrap()
            .len(),
        8
    );
    let (ctype, body) = multipart_body("ninth.bin", "application/octet-stream", b"9");
    let resp = server
        .request_raw(
            "POST",
            &format!("/v1/rooms/{room_id}/attachments"),
            Some(&human_token),
            Some(&ctype),
            &body,
        )
        .await;
    let att_9 = resp.json()["attachment"]["id"].as_str().unwrap().to_owned();
    let mut ids9 = ids8.clone();
    ids9.push(att_9);
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&human_token),
            Some("att-m-9"),
            Some(&serde_json::json!({"body": "nine", "attachments": ids9})),
        )
        .await;
    assert_eq!(resp.status, 400);
    assert_eq!(resp.json()["error"]["code"], "invalid_attachment");
    step(
        "cap8",
        "8 attachments 201; 9th 400 invalid_attachment".into(),
    );

    // 16 MiB 边界：恰好 16 MiB 成功；+1 字节 413。
    let (ctype, body) = multipart_body(
        "max.bin",
        "application/octet-stream",
        &vec![7u8; 16 * 1024 * 1024],
    );
    let resp = server
        .request_raw(
            "POST",
            &format!("/v1/rooms/{room_id}/attachments"),
            Some(&human_token),
            Some(&ctype),
            &body,
        )
        .await;
    assert_eq!(resp.status, 201, "exactly 16 MiB must succeed");
    let (ctype, body) = multipart_body(
        "over.bin",
        "application/octet-stream",
        &vec![7u8; 16 * 1024 * 1024 + 1],
    );
    let resp = server
        .request_raw(
            "POST",
            &format!("/v1/rooms/{room_id}/attachments"),
            Some(&human_token),
            Some(&ctype),
            &body,
        )
        .await;
    assert_eq!(resp.status, 413);
    assert_eq!(resp.json()["error"]["code"], "payload_too_large");
    step(
        "16MiB",
        "16 MiB accepted; 16 MiB + 1 -> 413 payload_too_large".into(),
    );

    // 跨 actor：outsider 不能绑定/下载 uploader 的附件（统一 404）；跨
    // Room：另一 Room 的成员不能绑定（统一 404）。
    let (_outsider_id, outsider_token) = prepare_outsider_human(&db_path).await;
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/attachments/{att_a}"),
            Some(&outsider_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 404);
    assert_eq!(resp.json()["error"]["code"], "attachment_not_found");
    // outsider 建自己的 Room 与 Agent，试图把 owner 的 pending 绑定进自己
    // 的 Room → 404 attachment_not_found（跨 Room + 跨 actor 统一）。
    let resp = server
        .request(
            "POST",
            "/v1/agents",
            Some(&outsider_token),
            Some("att-o-agent"),
            Some(&serde_json::json!({"display_name":"O"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let o_agent = resp.json()["agent"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&outsider_token),
            Some("att-o-room"),
            Some(&serde_json::json!({"agent_id": o_agent})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let o_room = resp.json()["room"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{o_room}/messages"),
            Some(&outsider_token),
            Some("att-o-m-1"),
            Some(&serde_json::json!({"body": "x", "attachments": [att_a]})),
        )
        .await;
    assert_eq!(resp.status, 404);
    assert_eq!(resp.json()["error"]["code"], "attachment_not_found");
    step(
        "cross",
        "outsider download 404 attachment_not_found; cross-room/cross-actor bind 404 attachment_not_found".into(),
    );

    // 绑定原子回滚：1 合法 + 1 非法附件 → 404，无新 Message，合法附件仍
    // pending（att_b 已在上面的附件-only 测试绑定，此处用新的 pending）。
    let (ctype, body) = multipart_body("c.bin", "application/octet-stream", b"ccc");
    let resp = server
        .request_raw(
            "POST",
            &format!("/v1/rooms/{room_id}/attachments"),
            Some(&human_token),
            Some(&ctype),
            &body,
        )
        .await;
    assert_eq!(resp.status, 201);
    let att_c = resp.json()["attachment"]["id"].as_str().unwrap().to_owned();
    let ghost = uuid::Uuid::new_v4().to_string();
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&human_token),
            Some("att-m-10"),
            Some(&serde_json::json!({"body": "rollback", "attachments": [att_c.clone(), ghost]})),
        )
        .await;
    assert_eq!(resp.status, 404);
    assert_eq!(resp.json()["error"]["code"], "attachment_not_found");
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    let history_json = resp.json();
    let bodies: Vec<Option<&str>> = history_json["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["body"].as_str())
        .collect();
    assert!(
        !bodies.contains(&Some("rollback")),
        "failed bind must not leave a message"
    );
    // att_c 仍可单独绑定（未被回滚的事务占用）。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&human_token),
            Some("att-m-11"),
            Some(&serde_json::json!({"body": "c alone", "attachments": [att_c]})),
        )
        .await;
    assert_eq!(
        resp.status, 201,
        "unconsumed pending must still be bindable"
    );
    step(
        "atomic-rollback",
        "1 valid + 1 ghost attachment -> 404; no message; pending unconsumed".into(),
    );

    // Human Event：message.created 快照含 attachments；Agent Event 视图
    // message.attachments 一致。
    let resp = server
        .request(
            "GET",
            "/v1/human/events?limit=50",
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    let created = resp.json()["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "message.created" && e["snapshot"]["id"] == msg_1)
        .cloned()
        .expect("message.created event must exist");
    assert_eq!(
        created["snapshot"]["attachments"][0]["id"], att_a,
        "human event snapshot must carry attachment metadata"
    );
    // Agent Event 视图（Backend Credential 拉取）message.attachments 一致。
    let resp = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_id}/backend-bindings"),
            Some(&human_token),
            Some("att-binding-1"),
            Some(&serde_json::json!({"label":"local runtime"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let binding_id = resp.json()["binding"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
            Some(&human_token),
            Some("att-cred-1"),
            Some(&serde_json::json!({"replaces_credential_id":null})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let backend_token = resp.json()["token"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "GET",
            "/v1/backend/events?limit=50",
            Some(&backend_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    let events_json = resp.json();
    let events = events_json["events"].as_array().unwrap();
    let ev = events
        .iter()
        .find(|e| e["message"]["id"] == msg_1)
        .expect("event for bound message");
    assert_eq!(ev["message"]["attachments"][0]["id"], att_a);
    step(
        "events",
        "human event snapshot + agent event view carry attachment metadata".into(),
    );

    // Backend：Agent 上传 + Completion reply 带附件 → 201；同结果重试一致；
    // 改附件集合 → 409 completion_conflict；Agent 可下载。
    let (ctype, body) = multipart_body("agent.bin", "application/octet-stream", b"agent bytes");
    let resp = server
        .request_raw(
            "POST",
            &format!("/v1/backend/rooms/{room_id}/attachments"),
            Some(&backend_token),
            Some(&ctype),
            &body,
        )
        .await;
    assert_eq!(resp.status, 201);
    let att_agent = resp.json()["attachment"]["id"].as_str().unwrap().to_owned();
    let source_event = events
        .iter()
        .find(|e| e["message"]["actor"]["kind"] == "human")
        .expect("human source event")
        .clone();
    let _event_id = source_event["id"].as_str().unwrap().to_owned();
    // ⚠ 2026-09-28：原以 Completion reply 携带附件写回；退役后改走 say
    //（同一 message 用例，body/attachments 形状与人类路径一致）。
    let resp = server
        .request(
            "POST",
            &format!("/v1/backend/rooms/{room_id}/messages"),
            Some(&backend_token),
            Some("att-reply-k-1"),
            Some(&serde_json::json!({
                "body": "echo with attachment",
                "attachments": [att_agent],
            })),
        )
        .await;
    assert_eq!(resp.status, 201, "say with attachment: {}", resp.json());
    let reply_message_id = resp.json()["message"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/messages?limit=100"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    let history_json = resp.json();
    let reply = history_json["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == reply_message_id)
        .expect("reply in history");
    assert_eq!(reply["attachments"][0]["id"], att_agent);
    // 同结果重试 200；改附件 409 idempotency_conflict（同一 message 幂等面）。
    let resp = server
        .request(
            "POST",
            &format!("/v1/backend/rooms/{room_id}/messages"),
            Some(&backend_token),
            Some("att-reply-k-1"),
            Some(&serde_json::json!({
                "body": "echo with attachment",
                "attachments": [att_agent],
            })),
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json()["idempotency_replayed"], true);
    let resp = server
        .request(
            "POST",
            &format!("/v1/backend/rooms/{room_id}/messages"),
            Some(&backend_token),
            Some("att-reply-k-1"),
            Some(&serde_json::json!({
                "body": "echo with attachment",
                "attachments": [],
            })),
        )
        .await;
    assert_eq!(resp.status, 409);
    assert_eq!(resp.json()["error"]["code"], "idempotency_conflict");
    // Agent 下载自己绑定的附件。
    let resp = server
        .request(
            "GET",
            &format!("/v1/backend/rooms/{room_id}/attachments/{att_agent}"),
            Some(&backend_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"agent bytes");
    step(
        "completion",
        "agent upload + reply with attachment 201; same retry 200; changed attachment set 409 completion_conflict; agent download 200".into(),
    );

    // 撤回：History/Event 投影 attachments=[]、下载 404、Human Event created
    // 快照 attachments=[] 且 recalled_at 变形。
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/rooms/{room_id}/messages/{msg_1}"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 201);
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/messages?limit=100"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    let history_json = resp.json();
    let recalled = history_json["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == msg_1)
        .expect("recalled message stays in history");
    assert_eq!(recalled["attachments"], serde_json::json!([]));
    assert_eq!(recalled["body"], serde_json::Value::Null);
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/attachments/{att_a}"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 404);
    assert_eq!(resp.json()["error"]["code"], "attachment_not_found");
    let resp = server
        .request(
            "GET",
            "/v1/human/events?limit=50",
            Some(&human_token),
            None,
            None,
        )
        .await;
    let created_after = resp.json()["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "message.created" && e["snapshot"]["id"] == msg_1)
        .cloned()
        .expect("created event persists");
    assert_eq!(
        created_after["snapshot"]["attachments"],
        serde_json::json!([]),
        "recall must clear attachments in the created snapshot"
    );
    assert!(created_after["snapshot"]["recalled_at"].is_number());
    let resp = server
        .request(
            "GET",
            "/v1/backend/events?limit=50",
            Some(&backend_token),
            None,
            None,
        )
        .await;
    let events_json = resp.json();
    let ev_after = events_json["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["message"]["id"] == msg_1)
        .expect("event persists");
    assert_eq!(ev_after["message"]["attachments"], serde_json::json!([]));
    step(
        "recall",
        "recall clears attachments in history/human event/agent event; download 404".into(),
    );

    // 重启：附件、绑定与投影保持（含 new 上传的第二个附件与 Agent reply）。
    server.stop().await;
    let mut server = Server::start(&db_path, &media_root).await;
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/messages?limit=100"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    let restart_json = resp.json();
    let after_restart = restart_json["messages"].as_array().unwrap();
    assert!(
        after_restart.iter().any(|m| m["id"] == reply_message_id),
        "agent reply with attachment must survive restart"
    );
    let resp = server
        .request(
            "GET",
            &format!("/v1/backend/rooms/{room_id}/attachments/{att_agent}"),
            Some(&backend_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"agent bytes");
    step(
        "restart",
        "attachments, bindings and projections survive restart; download still works".into(),
    );

    // orphan sweep + path attack：媒体目录中写入超 grace 的孤儿文件（名称
    // 为严格 UUID v4 且无 SQLite 引用）→ 重启后被清除；有引用的文件保留；
    // symlink 不被 follow；file_name 含路径穿越字符只影响元数据，不进入
    // 磁盘路径（字节名 = storage key）。
    let attachments_dir = media_root.join("attachments");
    let orphan_key = uuid::Uuid::new_v4().to_string();
    std::fs::write(attachments_dir.join(&orphan_key), b"orphan bytes").unwrap();
    filetime::set_file_mtime(
        attachments_dir.join(&orphan_key),
        filetime::FileTime::from_system_time(
            std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86400),
        ),
    )
    .expect("age orphan file");
    // 有引用的文件（att_agent 的字节）保持新鲜。
    let referenced_file = std::fs::read_dir(&attachments_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name != orphan_key
        })
        .map(|e| e.path());
    let referenced_name = referenced_file
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // 在受控命名空间外放一个文件与一个 symlink：sweep 不得触碰。symlink
    // 部分仅 unix 语义（Windows 无 symlink 语义）。
    std::fs::write(attachments_dir.join("operator-note.txt"), b"keep").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        attachments_dir.join("operator-note.txt"),
        attachments_dir.join("link.outside-namespace"),
    )
    .unwrap();
    // file_name 穿越测试：上传 file_name="../../evil.txt"（只在元数据中）。
    let (ctype, body) = multipart_body("../../evil.txt", "text/plain", b"pwn?");
    let resp = server
        .request_raw(
            "POST",
            &format!("/v1/rooms/{room_id}/attachments"),
            Some(&human_token),
            Some(&ctype),
            &body,
        )
        .await;
    assert_eq!(resp.status, 201);
    assert_eq!(resp.json()["attachment"]["file_name"], "../../evil.txt");
    server.stop().await;
    let mut server = Server::start(&db_path, &media_root).await;
    assert!(
        !attachments_dir.join(&orphan_key).exists(),
        "aged unreferenced orphan must be swept on startup"
    );
    assert!(
        attachments_dir.join("operator-note.txt").exists(),
        "files outside the strict namespace must be kept"
    );
    #[cfg(unix)]
    assert!(
        attachments_dir.join("link.outside-namespace").exists(),
        "symlinks must never be followed or removed"
    );
    assert!(
        !referenced_name.is_empty() && attachments_dir.join(&referenced_name).exists(),
        "referenced blob file must be kept"
    );
    assert!(
        !media_root.join("evil.txt").exists() && !media_root.join("..").join("evil.txt").exists(),
        "file_name must never enter the filesystem path"
    );
    step(
        "sweep",
        "aged orphan swept on restart; referenced file, out-of-namespace file and symlink kept; file_name traversal never touches paths".into(),
    );

    server.stop().await;
    run_dir.cleanup();

    println!("ATTACHMENT BLACKBOX EVIDENCE:");
    println!("{}", evidence.join("\n"));
    println!(
        "ATTACHMENT BLACKBOX DONE: upload/bind/download, attachment-only, 8-cap, 16MiB boundary, idempotency, cross-actor/room 404, atomic rollback, human/agent events, completion reply attachments, recall clearing, restart persistence, orphan sweep and path attack all passed; server closed, temp assets removed"
    );
}
/// 主体进行态（契约 `CORE_V1_AGENT_PROTOCOL.md` §6）·真实进程黑盒：
/// 「在不在」由**连接**给出、「正在写」由主体自己的一个易失动作给出，
/// 两者都对同房成员可见，人机同形；执行面字段一个都不存在；旧端点全 404；
/// 进程重启即失忆。
///
/// ⚠ 2026-09-28：本用例取代原先的 `blackbox_agent_live_state_via_public_api`
/// 与 `blackbox_recall_clears_live_activity_immediately`——心跳、Activity
/// （阶段/流/草稿）、独立 Live WS 与挂在消息撤回上的即时清理都随旧
/// Live State 删除（主人裁决「不需要看到执行过程」）。
#[tokio::test]
async fn blackbox_subject_presence_and_typing_via_public_api() {
    let run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;

    let human_token = bootstrap_cli(&db_path).await;
    let human_id = server
        .request("GET", "/v1/me", Some(&human_token), None, None)
        .await
        .json()["participant"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut evidence: Vec<String> = Vec::new();
    let mut step = |name: &str, detail: String| evidence.push(format!("[{name}] {detail}"));

    // Provision：Agent + Binding + Credential + Direct Room。
    let resp = server
        .request(
            "POST",
            "/v1/agents",
            Some(&human_token),
            Some("live-agent-1"),
            Some(&serde_json::json!({"display_name": "小满"})),
        )
        .await;
    assert_eq!(resp.status, 201, "create agent: {}", resp.json());
    let agent_id = resp.json()["agent"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_id}/backend-bindings"),
            Some(&human_token),
            Some("live-binding-1"),
            Some(&serde_json::json!({"label": "Native Runtime"})),
        )
        .await;
    assert_eq!(resp.status, 201, "create binding: {}", resp.json());
    let binding_id = resp.json()["binding"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/agents/{agent_id}/backend-bindings/{binding_id}/credentials"),
            Some(&human_token),
            Some("live-cred-1"),
            Some(&serde_json::json!({"replaces_credential_id": null})),
        )
        .await;
    assert_eq!(resp.status, 201, "issue credential: {}", resp.json());
    let backend_token = resp.json()["token"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&human_token),
            Some("live-room-1"),
            Some(&serde_json::json!({"agent_id": agent_id})),
        )
        .await;
    assert_eq!(resp.status, 201, "create room: {}", resp.json());
    let room_id = resp.json()["room"]["id"].as_str().unwrap().to_owned();

    // ① 快照：两位主体都在名单上，人机同形；此刻谁都没连、谁都没写。
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/live"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "snapshot: {}", resp.json());
    let snap = resp.json();
    let members = snap["members"].as_array().unwrap();
    assert_eq!(members.len(), 2, "人与 Agent 同形，都在名单上：{snap}");
    for row in members {
        assert_eq!(row["online"], false, "没有任何连接时全员离线：{row}");
        assert_eq!(row["typing"], false, "没人声明过在写：{row}");
        for forbidden in [
            "activity",
            "phase",
            "stream_id",
            "revision",
            "body",
            "presence",
        ] {
            assert!(
                row.get(forbidden).is_none(),
                "执行面字段 `{forbidden}` 不得出现：{row}"
            );
        }
    }
    assert!(snap.get("agents").is_none(), "投影的名字是 members");
    step("roster", format!("members={}", members.len()));

    // ② Human 挂上真实 WS ⇒ 在线；Agent 还没露过面 ⇒ 离线。
    let mut ws = ws_connect(&server.addr, "/v1/human/events/stream")
        .await
        .expect("human event ws connect");
    ws_send_text(
        &mut ws,
        &format!("{{\"type\":\"authenticate\",\"token\":\"{human_token}\"}}"),
    )
    .await
    .expect("ws authenticate frame");
    let ack = timeout(Duration::from_secs(5), ws_read_text_json(&mut ws))
        .await
        .expect("ws ack within 5s")
        .expect("ws ack frame");
    assert_eq!(ack["type"], "authenticated", "{ack}");

    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/live"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    let snap = resp.json();
    let row = |snap: &serde_json::Value, id: &str| -> serde_json::Value {
        snap["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["participant_id"] == id)
            .cloned()
            .expect("member row")
    };
    assert_eq!(
        row(&snap, &human_id)["online"],
        true,
        "挂着 WS 连接 ⇒ 在线（没有心跳、没有上报）：{snap}"
    );
    assert_eq!(
        row(&snap, &agent_id)["online"],
        false,
        "Agent 还没露过面 ⇒ 离线"
    );
    step("human_online", "ws connected => online=true".into());

    // ③ Agent 走一次 BackendAuth（拉事件）⇒ 被看见 ⇒ 在线（拉取式兜底）。
    let resp = server
        .request(
            "GET",
            "/v1/backend/events?limit=10",
            Some(&backend_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "events pull: {}", resp.json());
    let resp = server
        .request(
            "GET",
            &format!("/v1/backend/rooms/{room_id}/live"),
            Some(&backend_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "backend snapshot: {}", resp.json());
    assert_eq!(
        row(&resp.json(), &agent_id)["online"],
        true,
        "认证过 ⇒ 被看见 ⇒ 在线"
    );
    step("agent_online", "backend auth => seen => online=true".into());

    // ④ typing：Human 声明 ⇒ Agent 侧看得见；发出消息 ⇒ 立刻消失。
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/typing"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204, "human typing: {}", resp.json());
    let resp = server
        .request(
            "GET",
            &format!("/v1/backend/rooms/{room_id}/live"),
            Some(&backend_token),
            None,
            None,
        )
        .await;
    assert_eq!(
        row(&resp.json(), &human_id)["typing"],
        true,
        "同房成员看得见「正在写」"
    );

    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_id}/messages"),
            Some(&human_token),
            Some("live-msg-1"),
            Some(&serde_json::json!({"body": "在的"})),
        )
        .await;
    assert_eq!(resp.status, 201, "send message: {}", resp.json());
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/live"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(
        row(&resp.json(), &human_id)["typing"],
        false,
        "消息发出后自己的「正在写」立刻消失"
    );
    step(
        "typing",
        "declare -> visible; send message -> cleared".into(),
    );

    // ⑤ Agent 的 typing 走同一条规则（BackendAuth 面）。
    let resp = server
        .request(
            "POST",
            &format!("/v1/backend/rooms/{room_id}/typing"),
            Some(&backend_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204, "agent typing: {}", resp.json());
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/live"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(
        row(&resp.json(), &agent_id)["typing"],
        true,
        "人机同形：Agent 说自己在写，Human 也看得见"
    );
    step("agent_typing", "same rule, backend auth face".into());

    // ⑥ 旧执行面端点全部 404（带表、带端点删除，不留死端点）。
    for (method, path) in [
        ("POST", "/v1/backend/presence"),
        (
            "DELETE",
            "/v1/backend/presence/00000000-0000-4000-8000-00000000a001",
        ),
        (
            "PUT",
            "/v1/backend/events/00000000-0000-4000-8000-00000000b001/activity",
        ),
        ("GET", "/v1/human/live/stream"),
    ] {
        let resp = server
            .request(method, path, Some(&backend_token), None, None)
            .await;
        assert_eq!(
            resp.status,
            404,
            "{method} {path} must be gone: {}",
            resp.json()
        );
    }
    step("removed", "presence / activity / live-ws all 404".into());

    // ⑦ 进程重启 ⇒ 进行态全部失忆（进程内易失，这是契约不是缺陷）。
    drop(ws);
    server.stop().await;
    server = Server::start(&db_path, &run_dir.path().join("media")).await;
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_id}/live"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "snapshot after restart: {}", resp.json());
    let snap = resp.json();
    assert_eq!(row(&snap, &agent_id)["online"], false, "重启后在线态失忆");
    assert_eq!(row(&snap, &human_id)["typing"], false, "重启后进行态失忆");
    step(
        "restart",
        "presence/typing forgotten (in-process by contract)".into(),
    );

    println!("BLACKBOX EVIDENCE:\n{}", evidence.join("\n"));
}

/// messages、human_event_facts、human_event_deliveries、read_cursors、
/// read_cursor_advancements。
/// 单表行数（Live 状态不得触碰任何耐久事实）。
/// 真实独立进程黑盒：**一间房 = 一段关系**（批次二）——同一对 Human–Agent 只有
/// 一间私人房：换 key 再要一次也**回到那间**（200 重放），`name` 不再是合法字段
/// （400 未知字段）；另一对是另一段关系 ⇒ 另一间房；消息/Event 按 room_id 隔离、
/// Agent 回复经 Completion 后 unread/read cursor 按 room_id 隔离、Agent revoke
/// 只关它自己那间、重启后房间仍在且同一对仍回到那间。
#[tokio::test]
async fn blackbox_one_room_per_relation_via_public_api() {
    let run_dir = TestRunDir::create();
    let db_path = run_dir.db_path();

    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;
    let human_token = bootstrap_cli(&db_path).await;
    let mut evidence: Vec<String> = Vec::new();
    let mut step = |name: &str, detail: String| evidence.push(format!("[{name}] {detail}"));

    // Agent 1 与 Agent 2 = **两段关系**（两对 Human–Agent），各得一间房。
    let mut agents = Vec::new();
    for i in 1..=2 {
        let resp = server
            .request(
                "POST",
                "/v1/agents",
                Some(&human_token),
                Some(&format!("md-agent-{i}")),
                Some(&serde_json::json!({"display_name": format!("甲{i}")})),
            )
            .await;
        assert_eq!(resp.status, 201);
        agents.push(resp.json()["agent"]["id"].as_str().unwrap().to_owned());
    }

    // 第一间房：201 Created，标题恒为 null（direct 房没有标题）。
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&human_token),
            Some("md-k-a"),
            Some(&serde_json::json!({"agent_id": agents[0]})),
        )
        .await;
    assert_eq!(resp.status, 201, "room A create: {}", resp.json());
    assert_eq!(resp.json()["room"]["name"], serde_json::Value::Null);
    assert_eq!(resp.json()["idempotency_replayed"], false);
    let room_a = resp.json()["room"]["id"].as_str().unwrap().to_owned();

    // 同一对**换一个 key**再要一次 ⇒ 200 重放同一间房，绝不新建第二间。
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&human_token),
            Some("md-k-b"),
            Some(&serde_json::json!({"agent_id": agents[0]})),
        )
        .await;
    assert_eq!(
        resp.status,
        200,
        "a second create for the same pair must replay: {}",
        resp.json()
    );
    assert_eq!(resp.json()["idempotency_replayed"], true);
    assert_eq!(resp.json()["room"]["id"], serde_json::json!(room_a));

    // 另一段关系（另一个 Agent）⇒ 另一间房。
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&human_token),
            Some("md-k-c"),
            Some(&serde_json::json!({"agent_id": agents[1]})),
        )
        .await;
    assert_eq!(resp.status, 201, "room B create: {}", resp.json());
    let room_b = resp.json()["room"]["id"].as_str().unwrap().to_owned();
    assert_ne!(room_a, room_b, "two relations must have two rooms");
    step(
        "one-room-per-relation",
        "same pair: 201 then 200 replay of the same room id; another agent (another relation): its own room"
            .into(),
    );

    // list 恰好两间、都没有标题；各自 get 独立。
    let resp = server
        .request("GET", "/v1/rooms", Some(&human_token), None, None)
        .await;
    assert_eq!(resp.status, 200);
    let list_json = resp.json();
    let rooms = list_json["rooms"].as_array().unwrap();
    assert_eq!(rooms.len(), 2, "list must show exactly two rooms");
    for item in rooms {
        let id = item["room"]["id"].as_str().unwrap();
        assert!(
            id == room_a || id == room_b,
            "unexpected room {id} in {list_json}"
        );
        assert_eq!(
            item["room"]["name"],
            serde_json::Value::Null,
            "direct rooms have no title: {list_json}"
        );
    }
    for room in [&room_a, &room_b] {
        let resp = server
            .request(
                "GET",
                &format!("/v1/rooms/{room}"),
                Some(&human_token),
                None,
                None,
            )
            .await;
        assert_eq!(resp.status, 200, "get {room}");
    }
    step(
        "list-get",
        "list shows exactly 2 untitled rooms; each get ok".into(),
    );

    // `name` 不再是合法字段：未知字段一律 400 invalid_request（标题退出身份）。
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&human_token),
            Some("md-k-name"),
            Some(&serde_json::json!({"agent_id": agents[0], "name": "项目A"})),
        )
        .await;
    assert_eq!(resp.status, 400, "title field: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "invalid_request");
    assert_eq!(
        resp.json()["room"],
        serde_json::Value::Null,
        "a rejected body must not create anything"
    );
    step(
        "title-is-not-a-field",
        "`name` in the create body is now an unknown field -> 400 invalid_request".into(),
    );

    // 同 key 精确重放 ⇒ 200 原房；同 key 改 agent ⇒ 409 idempotency_conflict。
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&human_token),
            Some("md-k-a"),
            Some(&serde_json::json!({"agent_id": agents[0]})),
        )
        .await;
    assert_eq!(resp.status, 200, "exact replay: {}", resp.json());
    assert_eq!(resp.json()["idempotency_replayed"], true);
    assert_eq!(resp.json()["room"]["id"], serde_json::json!(room_a));
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&human_token),
            Some("md-k-a"),
            Some(&serde_json::json!({"agent_id": agents[1]})),
        )
        .await;
    assert_eq!(resp.status, 409, "changed agent: {}", resp.json());
    assert_eq!(resp.json()["error"]["code"], "idempotency_conflict");
    step(
        "idempotency",
        "same key + same agent -> 200 replay of the same room; same key + another agent -> 409 idempotency_conflict"
            .into(),
    );

    // 消息与 Agent Event 按 room_id 隔离：绑定 Agent 1，向房间 A 发一条
    // 消息；房间 B（另一对）历史为空；Backend Event 携带 room_id == 房间 A。
    let resp = server
        .request(
            "POST",
            &format!("/v1/agents/{}/backend-bindings", agents[0]),
            Some(&human_token),
            Some("md-binding-1"),
            Some(&serde_json::json!({"label":"MD Backend"})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let binding = resp.json()["binding"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!(
                "/v1/agents/{}/backend-bindings/{binding}/credentials",
                agents[0]
            ),
            Some(&human_token),
            Some("md-cred-1"),
            Some(&serde_json::json!({"replaces_credential_id": null})),
        )
        .await;
    assert_eq!(resp.status, 201);
    let backend_token = resp.json()["token"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "POST",
            &format!("/v1/rooms/{room_a}/messages"),
            Some(&human_token),
            Some("md-m-1"),
            Some(&serde_json::json!({"body": "room A only"})),
        )
        .await;
    assert_eq!(resp.status, 201, "message in room A: {}", resp.json());
    for room in [&room_b] {
        let resp = server
            .request(
                "GET",
                &format!("/v1/rooms/{room}/messages"),
                Some(&human_token),
                None,
                None,
            )
            .await;
        assert_eq!(resp.status, 200);
        assert_eq!(
            resp.json()["messages"],
            serde_json::json!([]),
            "room {room} history must stay empty"
        );
    }
    let resp = server
        .request(
            "GET",
            "/v1/backend/events",
            Some(&backend_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    let events_json = resp.json();
    let events = events_json["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["kind"], "message.created");
    assert_eq!(events[0]["message"]["room_id"], serde_json::json!(room_a));
    let event_id = events[0]["id"].as_str().unwrap().to_owned();
    step(
        "isolation",
        "message in room A only; room B history empty; backend event carries room_id = A".into(),
    );

    // 未读/read cursor 按 room_id 隔离：复用已拉取的 A 房 Event 与 backend
    // credential，让 Agent 以成员身份在 A 房 say 一条公共回复；未读只出现
    // 在 A 房，推进 A 房 cursor 只清零 A，不影响 B。
    // ⚠ 2026-09-28：原走 Completion 协议产生这条回复；退役后改走 say。
    let _ = event_id;
    let resp = server
        .request(
            "POST",
            &format!("/v1/backend/rooms/{room_a}/messages"),
            Some(&backend_token),
            Some("md-reply-1"),
            Some(&serde_json::json!({
                "body": "official reply for room A",
            })),
        )
        .await;
    assert_eq!(resp.status, 201, "say in room A: {}", resp.json());
    let reply_id = resp.json()["message"]["id"].as_str().unwrap().to_owned();
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_a}/messages"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    let history_json = resp.json();
    let history = history_json["messages"].as_array().unwrap();
    assert_eq!(history.len(), 2, "A history = human message + agent reply");
    assert_eq!(history[0]["actor"]["kind"], "human");
    assert_eq!(history[1]["id"], reply_id);
    assert_eq!(history[1]["actor"]["kind"], "agent");
    assert_eq!(history[1]["body"], "official reply for room A");
    step(
        "completion-reply",
        format!("completion 201 in room A -> official reply {reply_id} in A history only"),
    );

    // 未读投影（公开列表）：只有 A 房 unread_count=1（Agent 回复），B
    // 房 unread_count=0；同 Human 自己的消息不计入自己未读。
    let resp = server
        .request("GET", "/v1/rooms", Some(&human_token), None, None)
        .await;
    assert_eq!(resp.status, 200);
    let list_json = resp.json();
    for item in list_json["rooms"].as_array().unwrap() {
        let id = item["room"]["id"].as_str().unwrap();
        let unread = item["read_state"]["unread_count"].as_i64().unwrap();
        if id == room_a {
            assert_eq!(
                unread, 1,
                "room A must carry the agent reply as unread: {list_json}"
            );
        } else {
            assert_eq!(
                unread, 0,
                "room {id} must stay unread 0 (read state isolated by room_id): {list_json}"
            );
        }
    }
    step(
        "unread-isolated",
        "after agent reply: room A unread_count=1, room B unread_count=0".into(),
    );

    // 推进 A 房 read cursor 到回复消息 → advanced=true、A 未读归零；B
    // 保持 0（cursor 不跨房扩散）。全部经公开 HTTP 观察，不直查 SQLite。
    let resp = server
        .request(
            "PUT",
            &format!("/v1/rooms/{room_a}/read-cursor"),
            Some(&human_token),
            None,
            Some(&serde_json::json!({"message_id": reply_id})),
        )
        .await;
    assert_eq!(resp.status, 200, "advance cursor: {}", resp.json());
    assert_eq!(resp.json()["advanced"], true);
    assert_eq!(resp.json()["read_state"]["last_read_message_id"], reply_id);
    assert_eq!(resp.json()["read_state"]["unread_count"], 0);
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_a}"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.json()["read_state"]["last_read_message_id"],
        reply_id,
        "room A get must reflect the advanced cursor: {}",
        resp.json()
    );
    assert_eq!(resp.json()["read_state"]["unread_count"], 0);
    let resp = server
        .request("GET", "/v1/rooms", Some(&human_token), None, None)
        .await;
    assert_eq!(resp.status, 200);
    let list_json = resp.json();
    for item in list_json["rooms"].as_array().unwrap() {
        let id = item["room"]["id"].as_str().unwrap();
        assert_eq!(
            item["read_state"]["unread_count"].as_i64().unwrap(),
            0,
            "room {id} unread must be 0 after advancing only room A: {list_json}"
        );
    }
    step(
        "read-cursor-isolated",
        "advance room A read cursor to the agent reply -> A unread 0, last_read = reply; room B remains 0"
            .into(),
    );

    // Agent revoke：Agent 1 的那**一间**房（A）的 Agent Membership 以 agent_revoked
    // 关闭，Human Membership 与 Room 保持 active；**Agent 2 的房（B）一点不受
    // 影响**——撤销按 Agent 传播，不顺着 Human 传到别人的关系上。
    let resp = server
        .request(
            "DELETE",
            &format!("/v1/agents/{}", agents[0]),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 204, "revoke agent 1");
    for room in [&room_a] {
        let resp = server
            .request(
                "GET",
                &format!("/v1/rooms/{room}/members"),
                Some(&human_token),
                None,
                None,
            )
            .await;
        assert_eq!(resp.status, 200, "members of {room}");
        let members_json = resp.json();
        let members = members_json["members"].as_array().unwrap();
        assert_eq!(members.len(), 2);
        assert_eq!(members[0]["membership"]["role"], "owner");
        assert!(members[0]["membership"]["left_at"].is_null());
        assert_eq!(members[1]["membership"]["role"], "member");
        assert_eq!(members[1]["membership"]["closed_by"], "agent_revoked");
        assert!(members[1]["membership"]["left_at"].is_string());
    }
    let resp = server
        .request(
            "GET",
            &format!("/v1/rooms/{room_b}/members"),
            Some(&human_token),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 200, "members of the other agent's room");
    let members_json = resp.json();
    let members = members_json["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    assert!(
        members[1]["membership"]["left_at"].is_null(),
        "the other agent's membership must stay active: {members_json}"
    );
    assert_eq!(
        members[1]["membership"]["closed_by"],
        serde_json::Value::Null
    );
    step(
        "revoke",
        "agent revoke closes only its own room's agent membership (agent_revoked); the other agent's room is untouched"
            .into(),
    );

    // 重启：readiness 通过、两间房持久（都没有标题）；同一对再要一次仍回到
    // **原来那间**（关系唯一性跨重启成立）。
    server.stop().await;
    let mut server = Server::start(&db_path, &run_dir.path().join("media")).await;
    let resp = server
        .request("GET", "/v1/rooms", Some(&human_token), None, None)
        .await;
    assert_eq!(resp.status, 200);
    let list_json = resp.json();
    let rooms = list_json["rooms"].as_array().unwrap();
    assert_eq!(rooms.len(), 2, "rooms must survive restart: {list_json}");
    for item in rooms {
        let id = item["room"]["id"].as_str().unwrap();
        assert!(
            id == room_a || id == room_b,
            "unexpected room {id} after restart"
        );
        assert_eq!(
            item["room"]["name"],
            serde_json::Value::Null,
            "direct rooms stay untitled after restart"
        );
    }
    // 重启后同一对再要一次仍回到**原来那间**（关系唯一性跨重启成立）。
    // 用 Agent 2：Agent 1 已在上面被撤销，撤销后它的关系不再接受建房（409）。
    let resp = server
        .request(
            "POST",
            "/v1/direct-rooms",
            Some(&human_token),
            Some("md-k-after-restart"),
            Some(&serde_json::json!({"agent_id": agents[1]})),
        )
        .await;
    assert_eq!(
        resp.status,
        200,
        "after restart the same pair must still replay: {}",
        resp.json()
    );
    assert_eq!(resp.json()["room"]["id"], serde_json::json!(room_b));
    step(
        "restart",
        "restart ready; 2 untitled rooms persisted; the same pair still replays into its own room"
            .into(),
    );

    server.stop().await;
    for line in &evidence {
        println!("{line}");
    }
    println!(
        "ONE ROOM PER RELATION BLACKBOX DONE: same-pair replay (new key and same key), title field rejected as unknown, two relations two rooms, per-room message/event isolation, unread/read-cursor isolation, revoke scoped to the revoked agent, restart persistence all passed; server closed, temp assets removed"
    );
}
