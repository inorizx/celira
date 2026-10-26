//! 外部输入入口的行为证明：**真起服务、真落盘**。
//!
//! 这里证明的不是"配置能构造"，而是：一个 `POST /channels/<route>` 经过认证、限流、幂等与
//! 正文上限之后，在 `<环境根>/channels/inbox/` 里留下**一条**形状正确的事件文件；拒绝路径
//! （401 / 404 / 413 / 415 / 429）**一个文件都不留**；重启之后文件还在；保留期只剪最旧的；
//! 热加载不必重启；唤醒信号是**电平**（一批动静最多一次）。
//!
//! 纪律：
//!
//! - 每个用例一个自己的临时环境根（`std::env::temp_dir()` 下唯一目录），不碰仓库的 `data/`
//!   与真实家目录；跑完删除。
//! - 端口一律 `127.0.0.1:0`（系统分配），**绝不碰 8787**。
//! - 断言读的是**盘上的字节**与**真实状态码**，不是函数返回值。

use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use celira_agent::channels::{self, ChannelServer, Retention};
use celira_agent::clock::ClockHandle;
use celira_agent::doorbell::DoorbellFailure;
use celira_agent::wake::{TimerSource, WakeHub, WakePort, Woken};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

/// 一个用完就删的临时环境根。
struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "celira-channels-{}-{tag}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("建得了临时目录");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn inbox(&self) -> PathBuf {
        self.path.join("channels").join("inbox")
    }

    fn config(&self) -> PathBuf {
        self.path.join("channels").join("routes.json")
    }

    fn write_config(&self, bind: &str, routes: &[Route]) {
        let routes: Vec<Value> = routes
            .iter()
            .map(|route| {
                let mut value = json!({
                    "id": route.id,
                    "label": route.label,
                    "secret": route.secret,
                    "enabled": route.enabled,
                    "max_body_bytes": route.max_body,
                    "rate_per_minute": route.rate,
                });
                if let Some(wake) = route.wake {
                    value["wake"] = json!(wake);
                }
                value
            })
            .collect();
        write_json(&self.config(), &json!({ "bind": bind, "routes": routes }));
    }

    fn inbox_files(&self) -> Vec<String> {
        names_in(&self.inbox())
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn write_json(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("建得了目录");
    }
    fs::write(path, serde_json::to_vec_pretty(value).expect("可序列化")).expect("写得了文件");
}

fn names_in(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().is_file())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[derive(Clone)]
struct Route {
    id: &'static str,
    label: &'static str,
    secret: &'static str,
    enabled: bool,
    max_body: usize,
    rate: u32,
    /// `None` = 不写这个键（走缺省 `true`）；`Some(false)` = 静默源。
    wake: Option<bool>,
}

impl Route {
    fn new(id: &'static str, secret: &'static str) -> Self {
        Self {
            id,
            label: id,
            secret,
            enabled: true,
            max_body: channels::DEFAULT_MAX_BODY_BYTES,
            rate: channels::DEFAULT_RATE_PER_MINUTE,
            wake: None,
        }
    }

    fn silent(mut self) -> Self {
        self.wake = Some(false);
        self
    }

    fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    fn max_body(mut self, max_body: usize) -> Self {
        self.max_body = max_body;
        self
    }

    fn rate(mut self, rate: u32) -> Self {
        self.rate = rate;
        self
    }
}

fn start(root: &Path) -> (ChannelServer, Receiver<()>) {
    let server = ChannelServer::start(root).expect("有 routes.json 就该起来");
    let wake = server.take_wake_receiver().expect("唤醒通道只交一次");
    (server, wake)
}

struct Request {
    token: Option<String>,
    content_type: Option<String>,
    key: Option<String>,
    delivery: Option<String>,
    request_id: Option<String>,
    event_header: Option<(&'static str, String)>,
    user_agent: Option<String>,
    body: Vec<u8>,
}

impl Request {
    fn new(body: &str) -> Self {
        Self {
            token: Some("cc_secret_one".to_string()),
            content_type: Some("text/plain; charset=utf-8".to_string()),
            key: None,
            delivery: None,
            request_id: None,
            event_header: None,
            user_agent: None,
            body: body.as_bytes().to_vec(),
        }
    }

    fn token(mut self, token: &str) -> Self {
        self.token = Some(token.to_string());
        self
    }

    fn no_token(mut self) -> Self {
        self.token = None;
        self
    }

    fn content_type(mut self, content_type: &str) -> Self {
        self.content_type = Some(content_type.to_string());
        self
    }

    fn no_content_type(mut self) -> Self {
        self.content_type = None;
        self
    }

    fn key(mut self, key: &str) -> Self {
        self.key = Some(key.to_string());
        self
    }

    fn delivery(mut self, delivery: &str) -> Self {
        self.delivery = Some(delivery.to_string());
        self
    }

    fn request_id(mut self, request_id: &str) -> Self {
        self.request_id = Some(request_id.to_string());
        self
    }

    fn event(mut self, name: &'static str, value: &str) -> Self {
        self.event_header = Some((name, value.to_string()));
        self
    }

    fn user_agent(mut self, user_agent: &str) -> Self {
        self.user_agent = Some(user_agent.to_string());
        self
    }

    fn bytes(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }
}

struct Reply {
    status: u16,
    retry_after: Option<String>,
    text: String,
}

fn post(addr: SocketAddr, route: &str, request: Request) -> Reply {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .build()
        .expect("建得了客户端");
    let mut builder = client.post(format!("http://{addr}/channels/{route}"));
    if let Some(token) = request.token {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }
    if let Some(content_type) = request.content_type {
        builder = builder.header("Content-Type", content_type);
    }
    if let Some(key) = request.key {
        builder = builder.header("Idempotency-Key", key);
    }
    if let Some(delivery) = request.delivery {
        builder = builder.header("X-GitHub-Delivery", delivery);
    }
    if let Some(request_id) = request.request_id {
        builder = builder.header("X-Request-Id", request_id);
    }
    if let Some((name, value)) = request.event_header {
        builder = builder.header(name, value);
    }
    if let Some(user_agent) = request.user_agent {
        builder = builder.header("User-Agent", user_agent);
    }
    let response = builder.body(request.body).send().expect("入口在听");
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = response.bytes().expect("读得了响应体").to_vec();
    let text = String::from_utf8_lossy(&body).into_owned();
    Reply {
        status,
        retry_after,
        text,
    }
}

fn read_event(root: &Path, name: &str) -> Value {
    let text = fs::read(root.join("channels").join("inbox").join(name)).expect("事件文件在");
    serde_json::from_slice(&text).expect("事件是 JSON")
}

// ---------------------------------------------------------------------------
// 接收路径
// ---------------------------------------------------------------------------

#[test]
fn an_accepted_event_lands_with_exact_fields_and_the_three_headers() {
    let root = TempRoot::new("accepted");
    root.write_config("127.0.0.1:0", &[Route::new("github", "cc_secret_one")]);
    let (server, _wake) = start(root.path());

    let body = r#"{"ref":"refs/heads/main","谁":"世界"}"#;
    let reply = post(
        server.local_addr(),
        "github",
        Request::new(body)
            .content_type("application/json; charset=utf-8")
            .key("evt-1")
            .event("X-GitHub-Event", "push")
            .user_agent("probe/1.0"),
    );
    assert_eq!(reply.status, 202, "{}", reply.text);

    assert_eq!(root.inbox_files(), vec!["github--evt-1.json".to_string()]);
    let event = read_event(root.path(), "github--evt-1.json");
    assert_eq!(event["id"], "evt-1");
    assert_eq!(event["route"], "github");
    assert_eq!(event["label"], "github");
    assert_eq!(event["content_type"], "application/json; charset=utf-8");
    assert_eq!(event["event"], "push");
    assert_eq!(event["user_agent"], "probe/1.0");
    assert_eq!(event["body"], body, "正文必须逐字节相同");
    assert_eq!(event["body_bytes"], body.len());
    let received_at = event["received_at"].as_str().expect("received_at 是字符串");
    assert_eq!(received_at.len(), 20, "RFC3339 UTC 秒精度: {received_at}");
    assert!(received_at.ends_with('Z'), "{received_at}");
    assert!(received_at.starts_with("20"), "{received_at}");

    // 三个头之外的**一个字节都不存**：文件里没有认证头、也没有 cookie。
    let raw = String::from_utf8_lossy(
        &fs::read(root.inbox().join("github--evt-1.json")).expect("事件文件在"),
    )
    .to_lowercase();
    assert!(
        !raw.contains("authorization"),
        "绝不存 Authorization: {raw}"
    );
    assert!(!raw.contains("cookie"), "绝不存 Cookie: {raw}");
    assert!(!raw.contains("cc_secret_one"), "凭据不进事件文件: {raw}");

    // 头回退：没有 Idempotency-Key 时用 X-GitHub-Delivery，再没有用 X-Request-Id。
    let reply = post(
        server.local_addr(),
        "github",
        Request::new("second").delivery("delivery-7"),
    );
    assert_eq!(reply.status, 202, "{}", reply.text);
    assert_eq!(
        root.inbox_files(),
        vec![
            "github--delivery-7.json".to_string(),
            "github--evt-1.json".to_string()
        ]
    );

    let reply = post(
        server.local_addr(),
        "github",
        Request::new("third").request_id("req-9"),
    );
    assert_eq!(reply.status, 202, "{}", reply.text);
    assert_eq!(
        root.inbox_files(),
        vec![
            "github--delivery-7.json".to_string(),
            "github--evt-1.json".to_string(),
            "github--req-9.json".to_string()
        ]
    );

    // Gitlab 那一格：同一个 `event` 字段，取另一个头。
    let reply = post(
        server.local_addr(),
        "github",
        Request::new("gitlab")
            .key("evt-gitlab")
            .event("X-Gitlab-Event", "Push Hook"),
    );
    assert_eq!(reply.status, 202, "{}", reply.text);
    let event = read_event(root.path(), "github--evt-gitlab.json");
    assert_eq!(event["event"], "Push Hook");

    // 没有键就现生成：文件名里是一段随机十六进制，事件 id 与它一致。
    let before = root.inbox_files();
    let reply = post(server.local_addr(), "github", Request::new("generated"));
    assert_eq!(reply.status, 202, "{}", reply.text);
    let generated: Vec<String> = root
        .inbox_files()
        .into_iter()
        .filter(|name| !before.contains(name))
        .collect();
    assert_eq!(generated.len(), 1, "{generated:?}");
    let event = read_event(root.path(), &generated[0]);
    assert_eq!(event["body"], "generated");
    assert_eq!(event["body_bytes"], 9);
    assert!(
        event["id"].as_str().expect("id 是字符串").len() == 32,
        "生成的 id 是 16 字节的十六进制: {event}"
    );
}

#[test]
fn missing_or_wrong_token_is_refused_and_nothing_is_written() {
    let root = TempRoot::new("auth");
    root.write_config(
        "127.0.0.1:0",
        &[
            Route::new("github", "cc_secret_one"),
            Route::new("other", "cc_secret_two"),
        ],
    );
    let (server, _wake) = start(root.path());

    let reply = post(server.local_addr(), "github", Request::new("x").no_token());
    assert_eq!(reply.status, 401);
    assert!(!reply.text.contains("cc_secret"), "错误响应不回显 secret");

    let reply = post(
        server.local_addr(),
        "github",
        Request::new("x").token("cc_wrong_secret"),
    );
    assert_eq!(reply.status, 401);

    // 另一条路由的 secret 也不行（凭据是按路由发的，不是一把通用钥匙）。
    let reply = post(
        server.local_addr(),
        "github",
        Request::new("x").token("cc_secret_two"),
    );
    assert_eq!(reply.status, 401);

    // 少一个 Bearer 方案也是 401。
    let reply = post(
        server.local_addr(),
        "github",
        Request::new("x").token("Basic cc_secret_one"),
    );
    assert_eq!(reply.status, 401);

    assert!(root.inbox_files().is_empty(), "拒绝路径一个文件都不留");
}

#[test]
fn unknown_and_disabled_routes_are_not_found() {
    let root = TempRoot::new("routes");
    root.write_config(
        "127.0.0.1:0",
        &[
            Route::new("github", "cc_secret_one"),
            Route::new("paused", "cc_secret_paused").disabled(),
        ],
    );
    let (server, _wake) = start(root.path());

    let reply = post(
        server.local_addr(),
        "nope",
        Request::new("x").token("cc_secret_one"),
    );
    assert_eq!(reply.status, 404);

    // disabled 的路由拿着**正确的** secret 也是 404：关掉就是不在。
    let reply = post(
        server.local_addr(),
        "paused",
        Request::new("x").token("cc_secret_paused"),
    );
    assert_eq!(reply.status, 404);
    assert!(root.inbox_files().is_empty());
}

#[test]
fn the_same_key_is_one_event_and_the_second_is_a_duplicate() {
    let root = TempRoot::new("duplicate");
    root.write_config("127.0.0.1:0", &[Route::new("github", "cc_secret_one")]);
    let (server, wake) = start(root.path());

    let reply = post(
        server.local_addr(),
        "github",
        Request::new("first body").key("same-key"),
    );
    assert_eq!(reply.status, 202, "{}", reply.text);
    assert_eq!(
        root.inbox_files(),
        vec!["github--same-key.json".to_string()]
    );

    // 同键异体：正文不同也算重复，第一个文件一个字节都不改。
    let reply = post(
        server.local_addr(),
        "github",
        Request::new("second body, different").key("same-key"),
    );
    assert_eq!(reply.status, 200, "{}", reply.text);
    assert!(reply.text.contains("duplicate"), "{}", reply.text);
    assert_eq!(
        root.inbox_files(),
        vec!["github--same-key.json".to_string()]
    );
    let event = read_event(root.path(), "github--same-key.json");
    assert_eq!(event["body"], "first body");

    // 重复不产生第二次唤醒信号：第一条的信号还在，第二条没有再加一个。
    assert!(wake.try_recv().is_ok(), "第一条该有一次唤醒");
    assert_eq!(wake.try_recv(), Err(TryRecvError::Empty));
}

#[test]
fn a_malformed_idempotency_key_is_a_bad_request() {
    let root = TempRoot::new("badkey");
    root.write_config("127.0.0.1:0", &[Route::new("github", "cc_secret_one")]);
    let (server, _wake) = start(root.path());

    let reply = post(
        server.local_addr(),
        "github",
        Request::new("x").key("has space"),
    );
    assert_eq!(reply.status, 400, "{}", reply.text);

    let reply = post(
        server.local_addr(),
        "github",
        Request::new("x").key(&"a".repeat(65)),
    );
    assert_eq!(reply.status, 400);

    assert!(root.inbox_files().is_empty());
}

#[test]
fn a_body_over_the_route_limit_is_refused() {
    let root = TempRoot::new("large");
    root.write_config(
        "127.0.0.1:0",
        &[Route::new("github", "cc_secret_one").max_body(16)],
    );
    let (server, _wake) = start(root.path());

    let reply = post(
        server.local_addr(),
        "github",
        Request::new("0123456789abcdef").key("exact"),
    );
    assert_eq!(reply.status, 202, "边界值本身要收下: {}", reply.text);

    let reply = post(
        server.local_addr(),
        "github",
        Request::new("0123456789abcdefg").key("over"),
    );
    assert_eq!(reply.status, 413);
    assert_eq!(root.inbox_files(), vec!["github--exact.json".to_string()]);
}

#[test]
fn non_utf8_and_unsupported_content_types_are_refused() {
    let root = TempRoot::new("media");
    root.write_config("127.0.0.1:0", &[Route::new("github", "cc_secret_one")]);
    let (server, _wake) = start(root.path());

    let reply = post(
        server.local_addr(),
        "github",
        Request::new("x").content_type("application/octet-stream"),
    );
    assert_eq!(reply.status, 415);

    let reply = post(
        server.local_addr(),
        "github",
        Request::new("x")
            .content_type("text/plain")
            .bytes(vec![0xff, 0xfe, 0xfd]),
    );
    assert_eq!(reply.status, 415, "非 UTF-8 一律 415");

    let reply = post(
        server.local_addr(),
        "github",
        Request::new("x").no_content_type(),
    );
    assert_eq!(reply.status, 415);

    assert!(root.inbox_files().is_empty());
}

#[test]
fn a_failed_write_is_a_service_unavailable_and_leaves_no_half_file() {
    let root = TempRoot::new("storage-failure");
    root.write_config("127.0.0.1:0", &[Route::new("github", "cc_secret_one")]);
    let (server, wake) = start(root.path());

    // 把收件箱换成一个**普通文件**：`create_dir_all` 与建文件都会失败 ⇒ 503，不是 202。
    fs::remove_dir_all(root.inbox()).expect("先清掉空目录");
    fs::write(root.inbox(), b"not a directory").expect("占住那个名字");

    let reply = post(
        server.local_addr(),
        "github",
        Request::new("x").key("doomed"),
    );
    assert_eq!(reply.status, 503, "{}", reply.text);
    assert_eq!(wake.try_recv(), Err(TryRecvError::Empty), "失败不发唤醒");
    assert!(root.inbox().is_file(), "那条路上没有留下任何半成品");
}

#[test]
fn the_third_request_over_a_limit_of_two_is_refused_with_retry_after() {
    let root = TempRoot::new("rate");
    root.write_config(
        "127.0.0.1:0",
        &[Route::new("github", "cc_secret_one").rate(2)],
    );
    let (server, _wake) = start(root.path());

    for key in ["one", "two"] {
        let reply = post(server.local_addr(), "github", Request::new("x").key(key));
        assert_eq!(reply.status, 202, "{}", reply.text);
    }
    let reply = post(
        server.local_addr(),
        "github",
        Request::new("x").key("three"),
    );
    assert_eq!(reply.status, 429, "{}", reply.text);
    let retry_after = reply.retry_after.expect("429 必须带 Retry-After");
    let seconds: u64 = retry_after.parse().expect("Retry-After 是秒数");
    assert!(
        (1..=60).contains(&seconds),
        "Retry-After 落在窗口内: {seconds}"
    );
    assert_eq!(root.inbox_files().len(), 2, "被限流的那条不落盘");
}

#[test]
fn a_burst_collapses_into_one_wake_signal() {
    let root = TempRoot::new("wake");
    root.write_config("127.0.0.1:0", &[Route::new("github", "cc_secret_one")]);
    let (server, wake) = start(root.path());

    for key in ["one", "two", "three"] {
        let reply = post(server.local_addr(), "github", Request::new("x").key(key));
        assert_eq!(reply.status, 202, "{}", reply.text);
    }
    assert_eq!(root.inbox_files().len(), 3);

    // 电平语义：三条新事件只留一个"有输入"的信号；先收一次，再确认通道是空的。
    assert!(wake.try_recv().is_ok(), "该有一次唤醒");
    assert_eq!(
        wake.try_recv(),
        Err(TryRecvError::Empty),
        "容量 1：三条动静合并成一次唤醒"
    );
}

// ---------------------------------------------------------------------------
// 持久与保留
// ---------------------------------------------------------------------------

#[test]
fn events_survive_a_restart() {
    let root = TempRoot::new("restart");
    root.write_config("127.0.0.1:0", &[Route::new("github", "cc_secret_one")]);

    {
        let (server, _wake) = start(root.path());
        let reply = post(
            server.local_addr(),
            "github",
            Request::new("before restart").key("first"),
        );
        assert_eq!(reply.status, 202, "{}", reply.text);
        server.shutdown();
    }
    assert_eq!(root.inbox_files(), vec!["github--first.json".to_string()]);

    let (server, _wake) = start(root.path());
    assert_eq!(
        root.inbox_files(),
        vec!["github--first.json".to_string()],
        "重启不清收件箱"
    );
    let reply = post(
        server.local_addr(),
        "github",
        Request::new("after restart").key("second"),
    );
    assert_eq!(reply.status, 202, "{}", reply.text);
    assert_eq!(
        root.inbox_files(),
        vec![
            "github--first.json".to_string(),
            "github--second.json".to_string()
        ]
    );
    assert_eq!(
        read_event(root.path(), "github--first.json")["body"],
        "before restart"
    );
}

#[test]
fn retention_keeps_only_the_newest_events() {
    let root = TempRoot::new("retention-count");
    root.write_config("127.0.0.1:0", &[Route::new("github", "cc_secret_one")]);
    let retention = Retention {
        max_events: 2,
        max_age: Duration::from_secs(30 * 24 * 60 * 60),
    };
    let server = ChannelServer::start_with_retention(root.path(), retention).expect("起得来");
    let _wake = server.take_wake_receiver();

    for key in ["e1", "e2", "e3"] {
        let reply = post(server.local_addr(), "github", Request::new("x").key(key));
        assert_eq!(reply.status, 202, "{}", reply.text);
    }
    assert_eq!(
        root.inbox_files(),
        vec!["github--e2.json".to_string(), "github--e3.json".to_string()],
        "上界 2 条：最旧的先删"
    );
}

#[test]
fn expired_events_are_dropped_at_startup() {
    let root = TempRoot::new("retention-age");
    root.write_config("127.0.0.1:0", &[Route::new("github", "cc_secret_one")]);
    let retention = Retention {
        max_events: 100,
        max_age: Duration::from_millis(250),
    };
    {
        let server = ChannelServer::start_with_retention(root.path(), retention).expect("起得来");
        let _wake = server.take_wake_receiver();
        let reply = post(
            server.local_addr(),
            "github",
            Request::new("old").key("old"),
        );
        assert_eq!(reply.status, 202, "{}", reply.text);
        server.shutdown();
    }
    assert_eq!(root.inbox_files(), vec!["github--old.json".to_string()]);

    std::thread::sleep(Duration::from_millis(400));
    let server = ChannelServer::start_with_retention(root.path(), retention).expect("起得来");
    let _wake = server.take_wake_receiver();
    assert!(
        root.inbox_files().is_empty(),
        "启动那一剪按年龄删：过期的不再留着"
    );

    let reply = post(
        server.local_addr(),
        "github",
        Request::new("new").key("new"),
    );
    assert_eq!(reply.status, 202, "{}", reply.text);
    assert_eq!(root.inbox_files(), vec!["github--new.json".to_string()]);
}

// ---------------------------------------------------------------------------
// 热加载
// ---------------------------------------------------------------------------

#[test]
fn a_new_route_is_picked_up_without_a_restart() {
    let root = TempRoot::new("reload-watch");
    root.write_config("127.0.0.1:0", &[Route::new("alpha", "cc_secret_one")]);
    let (server, _wake) = start(root.path());

    let reply = post(
        server.local_addr(),
        "beta",
        Request::new("x").key("before").token("cc_secret_two"),
    );
    assert_eq!(reply.status, 404, "还没声明就不在");

    root.write_config(
        "127.0.0.1:0",
        &[
            Route::new("alpha", "cc_secret_one"),
            Route::new("beta", "cc_secret_two"),
        ],
    );

    // 2 秒轮询：给它三个轮询窗口，但不碰 reload_now（要证明的是**监控**那一条）。
    let mut final_status = 0;
    for _ in 0..12 {
        std::thread::sleep(Duration::from_millis(300));
        let reply = post(
            server.local_addr(),
            "beta",
            Request::new("x").key("after").token("cc_secret_two"),
        );
        final_status = reply.status;
        if final_status == 202 {
            break;
        }
    }
    assert_eq!(final_status, 202, "热加载之后不必重启就能收");
    assert!(root.inbox_files().contains(&"beta--after.json".to_string()));
}

#[test]
fn reload_now_reads_the_file_immediately() {
    let root = TempRoot::new("reload-now");
    root.write_config("127.0.0.1:0", &[Route::new("alpha", "cc_secret_one")]);
    let (server, _wake) = start(root.path());

    root.write_config(
        "127.0.0.1:0",
        &[
            Route::new("alpha", "cc_secret_one"),
            Route::new("beta", "cc_secret_two"),
        ],
    );
    assert!(server.reload_now(), "新表该生效");
    let reply = post(
        server.local_addr(),
        "beta",
        Request::new("x").key("now").token("cc_secret_two"),
    );
    assert_eq!(reply.status, 202, "{}", reply.text);

    // 坏配置保留上一张表（不把入口弄哑）。
    write_json(
        &root.config(),
        &json!({ "bind": "127.0.0.1:0", "routes": [ { "id": "../evil" } ] }),
    );
    assert!(!server.reload_now(), "坏配置不该生效");
    let reply = post(
        server.local_addr(),
        "beta",
        Request::new("x").key("after-bad").token("cc_secret_two"),
    );
    assert_eq!(reply.status, 202, "旧表留着，入口照旧工作: {}", reply.text);
}

// ---------------------------------------------------------------------------
// 形状与安全
// ---------------------------------------------------------------------------

#[test]
fn hostile_route_ids_are_rejected_before_anything_is_written() {
    let root = TempRoot::new("hostile");
    let home = root.path().join("home");
    fs::create_dir_all(&home).expect("建得了环境根");

    // add：label 不是路径，`../` 直接拒；配置一个字节都不写。
    let error = channels::add_route(&home, "../evil", None, None, Some("127.0.0.1:0"));
    assert!(error.is_err(), "带 ../ 的 label 必须被拒");
    assert!(!home.join("channels").join("routes.json").exists());
    assert!(!root.path().join("evil").exists(), "写不出环境根");

    let error = channels::add_route(&home, "..\\..\\evil", None, None, Some("127.0.0.1:0"));
    assert!(error.is_err());

    // remove：形状不对的 id 在碰文件之前就被拒。
    let error = channels::remove_route(&home, "../../etc/passwd");
    assert!(error.is_err());

    // 手写一份带 ../ 的配置：整份拒绝，入口不起、收件箱不建。
    write_json(
        &home.join("channels").join("routes.json"),
        &json!({
            "bind": "127.0.0.1:0",
            "routes": [{
                "id": "../evil",
                "label": "evil",
                "secret": "cc_secret_one",
                "enabled": true,
                "max_body_bytes": 1024,
                "rate_per_minute": 60
            }]
        }),
    );
    assert!(
        ChannelServer::start(&home).is_none(),
        "坏 route id 的配置不许起入口"
    );
    assert!(!home.join("channels").join("inbox").exists());
    assert!(!root.path().join("evil").exists());
}

#[cfg(unix)]
#[test]
fn the_routes_file_is_private_on_unix() {
    use std::os::unix::fs::PermissionsExt;

    let root = TempRoot::new("permissions");
    channels::add_route(
        root.path(),
        "GitHub 通知",
        Some(4096),
        Some(5),
        Some("127.0.0.1:0"),
    )
    .expect("加得了路由");

    let mode = fs::metadata(root.config())
        .expect("routes.json 在")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "凭据文件在 Unix 上必须是 0600，实际 {mode:o}");
}

#[test]
fn without_a_routes_file_there_is_no_server_and_no_directory() {
    let root = TempRoot::new("absent");
    assert!(ChannelServer::start(root.path()).is_none());
    assert!(
        !root.path().join("channels").exists(),
        "默认关闭：不建目录、不开端口"
    );
}

// ---------------------------------------------------------------------------
// 独立命令
// ---------------------------------------------------------------------------

fn run(args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_agent_channel"))
        .args(args)
        .output()
        .expect("跑得了 agent_channel")
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn free_loopback_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("借得到一个端口");
    listener.local_addr().expect("读得到本地地址").port()
}

#[test]
fn the_cli_adds_lists_tests_and_removes_a_route() {
    let root = TempRoot::new("cli");
    let root_text = root.path().to_str().expect("临时路径是 UTF-8").to_string();
    let port = free_loopback_port();
    let bind = format!("127.0.0.1:{port}");

    let added = run(&[
        "add",
        "--root",
        &root_text,
        "--label",
        "GitHub 通知",
        "--bind",
        &bind,
        "--max-body",
        "4096",
        "--rate",
        "5",
    ]);
    assert_eq!(
        added.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let added_text = stdout_of(&added);
    assert!(added_text.contains("route=github"), "{added_text}");
    assert!(
        added_text.contains(&format!("endpoint=http://{bind}/channels/github")),
        "{added_text}"
    );
    let secret = added_text
        .lines()
        .find_map(|line| line.strip_prefix("secret="))
        .expect("add 要打印一次 secret")
        .to_string();
    assert!(secret.starts_with("cc_"), "{secret}");
    assert_eq!(secret.len(), 67, "cc_ + 32 字节的十六进制");

    let listed = run(&["list", "--root", &root_text]);
    assert_eq!(listed.status.code(), Some(0));
    let listed_text = stdout_of(&listed);
    assert!(listed_text.contains("id=github"), "{listed_text}");
    assert!(listed_text.contains("max_body=4096"), "{listed_text}");
    assert!(listed_text.contains("rate=5"), "{listed_text}");
    assert!(
        listed_text.contains(&format!("bind={bind}")),
        "{listed_text}"
    );
    assert!(
        !listed_text.contains(&secret),
        "list 绝不打印 secret: {listed_text}"
    );

    let (server, _wake) = start(root.path());
    let tested = run(&[
        "test",
        "--root",
        &root_text,
        "--route",
        "github",
        "--body",
        "from the cli",
    ]);
    assert_eq!(
        tested.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&tested.stderr)
    );
    let tested_text = stdout_of(&tested);
    assert!(tested_text.contains("status=202"), "{tested_text}");
    assert!(tested_text.contains("\"accepted\""), "{tested_text}");
    assert_eq!(root.inbox_files().len(), 1);
    server.shutdown();

    let removed = run(&["remove", "--root", &root_text, "--route", "github"]);
    assert_eq!(removed.status.code(), Some(0));
    assert!(stdout_of(&removed).contains("removed=true"));
    let listed = run(&["list", "--root", &root_text]);
    assert!(
        stdout_of(&listed).contains("routes: 0"),
        "{}",
        stdout_of(&listed)
    );

    // 再删一次：没有这条路由 ⇒ 退出码 2（配置事实，不是成功）。
    let again = run(&["remove", "--root", &root_text, "--route", "github"]);
    assert_eq!(again.status.code(), Some(2));

    // 用法错：退出码 1，且不写文件。
    let usage = run(&["add", "--root", &root_text]);
    assert_eq!(usage.status.code(), Some(1));
}

/// **落盘之后才响铃；这条铃接进宿主那条等待时，起源记作 `Channel`。**
///
/// 这是"入口 → 宿主"这一段接缝的证明：真起服务、真 POST、真把 `take_wake_receiver` 交出来的
/// 那条电平接进 `WakeHub::with_channels`，读到的下一次唤醒必须是 `Woken::Channel`。
#[test]
fn a_delivered_event_reaches_the_wake_hub_as_a_channel_wake() {
    let root = TempRoot::new("hub");
    root.write_config("127.0.0.1:0", &[Route::new("github", "cc_secret_one")]);
    let (server, wake) = start(root.path());

    let (bell, bell_source) = std::sync::mpsc::channel::<Woken>();
    let hub = WakeHub::with_channels(
        ManualBell {
            incoming: std::sync::Mutex::new(bell_source),
        },
        TimerSource::new(
            std::path::Path::new("/nonexistent-celira-channels-timers"),
            ClockHandle::fixed_at_unix_seconds(1_800_000_000),
            Duration::from_millis(50),
        ),
        wake,
    );

    let reply = post(server.local_addr(), "github", Request::new("hello"));
    assert_eq!(reply.status, 202, "新事件必须 202（得到 {}）", reply.status);
    assert_eq!(
        hub.next(),
        Woken::Channel,
        "入口的铃必须被搬成 Channel 起源"
    );

    // 铃与盘面是两件事：读到的这一声之后，文件仍在。
    assert_eq!(root.inbox_files().len(), 1);
    drop(bell);
}

/// 只用来占住门铃那一处：本用例要量的是**入口那一处**，门铃永远沉默。
struct ManualBell {
    incoming: std::sync::Mutex<Receiver<Woken>>,
}

impl WakePort for ManualBell {
    fn next(&self) -> Woken {
        self.incoming
            .lock()
            .expect("短临界区")
            .recv()
            .unwrap_or(Woken::Refused(DoorbellFailure::ChannelLost))
    }
}

/// **静默源**：事件照样收、照样落盘，只是不叫醒。操作者用 `wake=false` 声明。
#[test]
fn a_silent_route_keeps_the_event_without_waking() {
    let root = TempRoot::new("silent");
    root.write_config(
        "127.0.0.1:0",
        &[Route::new("github", "cc_secret_one").silent()],
    );
    let (server, wake) = start(root.path());

    let reply = post(server.local_addr(), "github", Request::new("hello"));
    assert_eq!(reply.status, 202, "静默不等于拒收");
    assert_eq!(root.inbox_files().len(), 1, "事件必须落盘");
    assert!(
        matches!(wake.try_recv(), Err(TryRecvError::Empty)),
        "静默源不许响铃"
    );
}

/// **主体自己的静音表**：她在 `projects/channels-mute` 里写一行 route id，入口就不再叫醒；
/// 把那一行删掉，叫醒恢复。表里的注释与空行忽略。
#[test]
fn the_subject_can_silence_a_route_with_her_own_file() {
    let root = TempRoot::new("mute");
    root.write_config("127.0.0.1:0", &[Route::new("github", "cc_secret_one")]);
    let projects = root.path().join("projects");
    fs::create_dir_all(&projects).expect("建作品面");
    let mute = projects.join("channels-mute");
    fs::write(&mute, "# 我自己写的\n\ngithub\n").expect("写静音表");

    let (server, wake) = start(root.path());
    let reply = post(
        server.local_addr(),
        "github",
        Request::new("first").key("mute-1"),
    );
    assert_eq!(reply.status, 202);
    assert_eq!(root.inbox_files().len(), 1);
    assert!(
        matches!(wake.try_recv(), Err(TryRecvError::Empty)),
        "她写了静音，就不许叫"
    );

    fs::remove_file(&mute).expect("撤掉静音");
    let reply = post(
        server.local_addr(),
        "github",
        Request::new("second").key("mute-2"),
    );
    assert_eq!(reply.status, 202);
    assert!(wake.try_recv().is_ok(), "静音撤掉之后，下一次到达要照常叫");
}

/// 操作者用 `--silent` 建的路由，`list` 里要看得到 `wake=off`——不然"为什么没响"只能靠猜。
#[test]
fn the_cli_silent_flag_is_visible_in_the_list() {
    let root = TempRoot::new("cli-silent");
    let root_text = root.path().to_str().expect("UTF-8").to_string();

    let added = run(&["add", "--root", &root_text, "--label", "Noisy", "--silent"]);
    assert_eq!(
        added.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let listed = run(&["list", "--root", &root_text]);
    assert_eq!(listed.status.code(), Some(0));
    let text = stdout_of(&listed);
    assert!(text.contains("id=noisy"), "{text}");
    assert!(text.contains("wake=off"), "{text}");
}
