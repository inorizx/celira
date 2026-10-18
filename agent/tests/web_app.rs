//! TASK_35 验收：web 窗的两条只读动词——线形状、五堵墙、失败词表、效果与能力面。
//!
//! 这一份用**离线回环假服务**（脚本化回执 + 请求原文捕获 + 连接计数）证明四件事：
//!
//! 1. **线形状**：搜索请求的路径、头与体逐项对得上；结果渲染有界；哨兵凭据不进任何输出；
//! 2. **五堵墙逐堵**：只取不写（协议白名单）/ 单次上限（超限拒绝、不截断）/ 超时 / 重定向
//!    上限与环 / 私网地址拒绝（含"放宽只对第一跳成立"那条）；
//! 3. **失败词表**是封闭九项，同一种故障两次逐字节相同；
//! 4. **两条动词声明为只读**——判据在**声明本身**（`declared_effects()`；它们不声明写效应）。
//!    ⚠ 原来这里写的是"它们的未决门放行是**声明的后果**，门那一族在 `tests/pending_effect_gate.rs`"
//!    ——**未决效果门已退役**（门是指令，而世界只该给信息），那份用例文件也已随门删除。
//!
//! 为什么离线用例要用一个显式策略：`WebPolicy` 把"多久算超时"与"要不要连本机/内网"两个
//! **操作旋钮**说出来（与门铃的 `DoorbellPolicy` 同一形状），生产给生产值、这里给能把回环
//! 假服务接进来的值。另外四堵墙没有旋钮，因此它们在这里是**原样**判的。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use celira_agent::app_contract::AppPort;
use celira_agent::app_contract::EffectClass;
use celira_agent::core::{Action, ActionId, ActionOutcome, ActionParams, ActionPort};
use celira_agent::foundation::{AgentId, OperationId, ResultState, RunId, Turn};
use celira_agent::web_app::{
    CAPABILITY_FETCH, CAPABILITY_SEARCH, MAX_FIELD_BYTES, MAX_PAGE_BYTES, MAX_QUERY_BYTES,
    MAX_RESULTS, SearchAccess, WEB_APP_ID, WebApp, WebPolicy,
};

/// 哨兵凭据：它只许进请求头，一个字节都不许出现在任何回执里。
const SENTINEL: &str = "tvly-sentinel-credential";

// ---------------------------------------------------------------------------
// 假服务：一次连接回一条脚本回执，并把请求原文交给用例
// ---------------------------------------------------------------------------

enum Reply {
    /// 回一个响应（状态 + 头 + 正文）。
    Answer {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    },
    /// 接住连接但一句话都不说（超时那一格用它）。
    Silent,
}

fn text(body: &str) -> Reply {
    with_kind(body, "text/plain; charset=utf-8")
}

fn html(body: &str) -> Reply {
    with_kind(body, "text/html")
}

fn with_kind(body: &str, media: &str) -> Reply {
    Reply::Answer {
        status: 200,
        headers: vec![("Content-Type".to_string(), media.to_string())],
        body: body.to_string(),
    }
}

fn code(status: u16) -> Reply {
    Reply::Answer {
        status,
        headers: vec![("Content-Type".to_string(), "application/json".to_string())],
        body: "{}".to_string(),
    }
}

fn body_only(status: u16, body: &str) -> Reply {
    Reply::Answer {
        status,
        headers: vec![("Content-Type".to_string(), "application/json".to_string())],
        body: body.to_string(),
    }
}

fn redirect(to: &str) -> Reply {
    Reply::Answer {
        status: 302,
        headers: vec![("Location".to_string(), to.to_string())],
        body: String::new(),
    }
}

/// 已绑定但还没开始应门的一个站点（重定向链要先把各跳的地址定下来）。
struct BoundSite {
    endpoint: String,
    listener: TcpListener,
}

/// 正在应门的一个站点。
struct Site {
    endpoint: String,
    requests: mpsc::Receiver<String>,
    connections: Arc<AtomicUsize>,
    _handle: JoinHandle<()>,
}

/// 绑定 `count` 个回环端口（每个站点一个）。
fn bind(count: usize) -> Vec<BoundSite> {
    (0..count)
        .map(|_| {
            let listener = TcpListener::bind("127.0.0.1:0").expect("可以绑定回环端口");
            let port = listener.local_addr().expect("有地址").port();
            BoundSite {
                endpoint: format!("http://127.0.0.1:{port}"),
                listener,
            }
        })
        .collect()
}

impl BoundSite {
    fn serve(self, replies: Vec<Reply>) -> Site {
        let (sender, requests) = mpsc::channel();
        let connections = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&connections);
        let handle = std::thread::spawn(move || {
            let mut queue = replies.into_iter();
            while let Ok((stream, _)) = self.listener.accept() {
                counter.fetch_add(1, Ordering::SeqCst);
                let mut peek = stream.try_clone().expect("克隆流");
                let request = read_request(&mut peek);
                let _ = sender.send(request);
                match queue.next() {
                    // 一句话都不说，但把连接**握在手里**（挂到用例的超时窗口之外）。
                    Some(Reply::Silent) => {
                        let _held = stream;
                        std::thread::sleep(Duration::from_millis(1_500));
                    }
                    Some(Reply::Answer {
                        status,
                        headers,
                        body,
                    }) => respond(stream, status, &headers, &body),
                    // 脚本用完之后再多来的连接一律 500 并计数（用来抓"偷偷重试/多余请求"）。
                    None => respond(stream, 500, &[], "{}"),
                }
            }
        });
        Site {
            endpoint: self.endpoint,
            requests,
            connections,
            _handle: handle,
        }
    }
}

impl Site {
    fn next_request(&self) -> String {
        self.requests
            .recv_timeout(Duration::from_secs(5))
            .expect("这一次调用应当发出一个请求")
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut reader = BufReader::new(stream);
    let mut raw = String::new();
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim_end().to_string();
                if trimmed.is_empty() {
                    break;
                }
                if let Some(value) = trimmed.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
                raw.push_str(&trimmed);
                raw.push('\n');
            }
            Err(_) => break,
        }
    }
    if length > 0 {
        let mut body = vec![0u8; length];
        let mut filled = 0usize;
        while filled < length {
            match reader.read(&mut body[filled..]) {
                Ok(0) => break,
                Ok(read) => filled += read,
                Err(_) => break,
            }
        }
        raw.push_str(&String::from_utf8_lossy(&body));
    }
    raw
}

fn respond(mut stream: TcpStream, status: u16, headers: &[(String, String)], body: &str) {
    let reason = match status {
        200 => "OK",
        302 => "Found",
        401 => "Unauthorized",
        403 => "Forbidden",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    };
    let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    head.push_str("Connection: close\r\n\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}

// ---------------------------------------------------------------------------
// 用例辅助
// ---------------------------------------------------------------------------

/// 把假站点当**搜索厂商**接进来（端点是配置，所以这里给的是完整的那一个地址）。
fn search_app(site: &Site) -> WebApp {
    with_policy(site, test_policy())
}

fn with_policy(site: &Site, policy: WebPolicy) -> WebApp {
    let endpoint = format!("{}/search", site.endpoint);
    let access = SearchAccess::new(endpoint, SENTINEL).expect("声明可以建立");
    WebApp::with_policy(Some(access), policy).expect("可以装配 web 窗")
}

/// 只接"取页"那一条（没有搜索凭据）。
fn fetch_app(policy: WebPolicy) -> WebApp {
    WebApp::with_policy(None, policy).expect("可以装配 web 窗")
}

/// 用例策略：窗口调小（超时那一格不用等 30 秒）＋ 允许**第一跳**落在回环（才接得上假服务）。
fn test_policy() -> WebPolicy {
    WebPolicy {
        request_timeout: Duration::from_millis(400),
        // 只担保回环：假服务住在那里。别的私网地址（比如内网）**不在**这份清单里，
        // 因此"重定向把我们往内网指"那一格仍然会被拒——这一条在下面有用例。
        allowed_private_hosts: vec!["127.0.0.1".to_string()],
    }
}

fn agent() -> AgentId {
    AgentId::new("agent-t35")
}

fn operation() -> OperationId {
    OperationId::new(RunId::new("web"), Turn::FIRST)
}

fn call(app: &mut WebApp, action: Action) -> ActionOutcome {
    app.execute(&agent(), &operation(), &action)
}

fn search(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_SEARCH), params)
}

fn fetch(params: ActionParams) -> Action {
    Action::with_params(ActionId::new(CAPABILITY_FETCH), params)
}

fn query(text: &str) -> ActionParams {
    ActionParams::none().with("query", text)
}

fn target(address: &str) -> ActionParams {
    ActionParams::none().with("url", address)
}

fn text_of(outcome: &ActionOutcome) -> String {
    outcome
        .content()
        .iter()
        .map(|block| block.text().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn failure_of(outcome: &ActionOutcome) -> String {
    outcome.failure_class().unwrap_or("<none>").to_string()
}

/// 一份搜索回执体（形状照厂商：`results[].{title,url,content}`）。
fn results_body(rows: &[(&str, &str, &str)]) -> String {
    let results: Vec<serde_json::Value> = rows
        .iter()
        .map(|(title, address, content)| {
            serde_json::json!({"title": title, "url": address, "content": content})
        })
        .collect();
    serde_json::json!({"results": results}).to_string()
}

// ---------------------------------------------------------------------------
// 1. 线形状
// ---------------------------------------------------------------------------

#[test]
fn the_search_request_carries_the_query_and_the_key_only_in_the_header() {
    let site = bind(1).remove(0).serve(vec![with_kind(
        &results_body(&[("标题甲", "https://a.example/1", "摘要甲")]),
        "application/json",
    )]);
    let mut app = search_app(&site);

    let outcome = call(&mut app, search(query("青苔")));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );

    let request = site.next_request();
    assert!(
        request.starts_with("POST /search "),
        "查询走的是配置里那一个端点：{request}"
    );
    assert!(
        request.contains(&format!("authorization: Bearer {SENTINEL}"))
            || request.contains(&format!("Authorization: Bearer {SENTINEL}")),
        "凭据只进请求头：{request}"
    );
    assert!(
        request.contains("\"query\":\"青苔\""),
        "查询原样进请求体：{request}"
    );
    assert!(
        request.contains(&format!("\"max_results\":{MAX_RESULTS}")),
        "条数上限由我们定，不交给主体：{request}"
    );
    // 凭据不进回执。
    assert!(
        !text_of(&outcome).contains(SENTINEL),
        "凭据不许出现在回执里：{}",
        text_of(&outcome)
    );
    assert_eq!(site.connections(), 1, "一次调用恰好一次出网请求");
}

#[test]
fn search_results_are_bounded_and_each_field_is_truncated() {
    let long = "长".repeat(MAX_FIELD_BYTES); // 每字 3 字节 ⇒ 远超上限
    let rows: Vec<(String, String, String)> = (0..MAX_RESULTS + 1)
        .map(|index| {
            (
                format!("标题{index}"),
                format!("https://example.test/{index}"),
                long.clone(),
            )
        })
        .collect();
    let borrowed: Vec<(&str, &str, &str)> = rows
        .iter()
        .map(|(title, address, content)| (title.as_str(), address.as_str(), content.as_str()))
        .collect();
    let site = bind(1).remove(0).serve(vec![with_kind(
        &results_body(&borrowed),
        "application/json",
    )]);
    let mut app = search_app(&site);

    let outcome = call(&mut app, search(query("上限")));
    assert_eq!(outcome.state(), ResultState::Success);
    let reading = text_of(&outcome);
    let mut lines = reading.lines();
    assert_eq!(
        lines.next(),
        Some(format!("results = {MAX_RESULTS}, truncated = true").as_str()),
        "多出来的如实报截断：{reading}"
    );
    let body_lines: Vec<&str> = lines.collect();
    assert_eq!(body_lines.len(), MAX_RESULTS, "只列上限条数：{reading}");
    for line in &body_lines {
        let fields: Vec<&str> = line.split(" | ").collect();
        assert_eq!(fields.len(), 3, "每条三段：{line}");
        for field in &fields {
            assert!(
                field.len() <= MAX_FIELD_BYTES + '…'.len_utf8(),
                "每一栏都要有界（字符边界上截断）：{field} ({})",
                field.len()
            );
        }
        assert!(fields[2].ends_with('…'), "被截断的那一栏要说出口：{line}");
    }
}

#[test]
fn a_result_row_with_a_missing_field_is_stated_as_dash() {
    let site = bind(1).remove(0).serve(vec![with_kind(
        r#"{"results":[{"title":"只有标题","url":"https://example.test/x"}]}"#,
        "application/json",
    )]);
    let mut app = search_app(&site);
    let outcome = call(&mut app, search(query("缺栏")));
    assert_eq!(outcome.state(), ResultState::Success);
    let reading = text_of(&outcome);
    assert!(
        reading.contains("只有标题 | https://example.test/x | -"),
        "缺的一栏照实说 `-`，不静默省略：{reading}"
    );
}

#[test]
fn an_empty_or_overlong_query_is_refused_without_a_request() {
    let site = bind(1).remove(0).serve(vec![]);
    let mut app = search_app(&site);
    for params in [
        ActionParams::none(),
        query(""),
        query("   "),
        query(&"长".repeat(MAX_QUERY_BYTES)),
    ] {
        let outcome = call(&mut app, search(params));
        assert_eq!(outcome.state(), ResultState::Failure);
        assert_eq!(failure_of(&outcome), "invalid-argument");
    }
    assert_eq!(site.connections(), 0, "参数网关先于出网：一次都不许发");
}

// ---------------------------------------------------------------------------
// 2. 凭据缺失与失败词表
// ---------------------------------------------------------------------------

#[test]
fn a_missing_credential_fails_search_but_fetch_still_works() {
    let mut site = bind(1);
    let page = site.remove(0).serve(vec![text("外面的字")]);
    let mut app = fetch_app(test_policy());

    // 查：没有凭据 ⇒ 如实失败（动词在场，不是"用不了的能力"）。
    // ⚠ **2026-10-13**：报的是 `credential-absent`（**这台机器没配**，本地事实、一次出网都不发），
    // 不是 `credential-rejected`（对面拒了）。两者从前压成一档，代价是主人看到"被拒"会去查凭据
    // 有效性，而真正该做的是**配一个**。
    let failed = call(&mut app, search(query("随便")));
    assert_eq!(failed.state(), ResultState::Failure);
    assert_eq!(failure_of(&failed), "credential-absent");
    assert_eq!(page.connections(), 0, "没配凭据时一次出网请求都不发");

    // 取：与厂商无关，照常可用。
    let fetched = call(&mut app, fetch(target(&page.endpoint)));
    assert_eq!(
        fetched.state(),
        ResultState::Success,
        "{}",
        text_of(&fetched)
    );
    assert!(
        text_of(&fetched).contains("外面的字"),
        "{}",
        text_of(&fetched)
    );
}

#[test]
fn the_search_failure_vocabulary_is_the_closed_table() {
    let cases: Vec<(Reply, &str)> = vec![
        (code(401), "credential-rejected"),
        (code(403), "credential-rejected"),
        (code(429), "server-error"),
        (code(500), "server-error"),
        (code(503), "server-error"),
        (body_only(200, "{ 这不是一份读得懂的回执"), "server-error"),
        (body_only(200, "{}"), "server-error"),
    ];
    for (reply, expected) in cases {
        let site = bind(1).remove(0).serve(vec![reply]);
        let mut app = search_app(&site);
        let outcome = call(&mut app, search(query("故障")));
        assert_eq!(outcome.state(), ResultState::Failure, "{expected}");
        assert_eq!(failure_of(&outcome), expected);
        assert!(
            !text_of(&outcome).contains(SENTINEL),
            "失败回执里也不许出现凭据"
        );
        assert_eq!(site.connections(), 1, "零重试：一次调用恰好一次请求");
    }
}

#[test]
fn the_same_failure_twice_is_byte_identical_and_a_dead_endpoint_is_transport() {
    let mut classes = Vec::new();
    for _ in 0..2 {
        let site = bind(1).remove(0).serve(vec![code(500)]);
        let mut app = search_app(&site);
        classes.push(failure_of(&call(&mut app, search(query("同一种故障")))));
    }
    assert_eq!(classes[0], classes[1], "同一种故障两次逐字节相同");

    // 连不上：地址本身就发不出去（端口越界 ⇒ 请求构造失败），走 `send()` 的同一个错误分支——
    // 与"连接被拒 / 域名解析不了"同一条。**不**用"绑一个端口再放掉"的写法：本机进程带着
    // 环境变量代理，那样请求会被代理接走并回一个非 2xx（那是环境的形状，不是这一层的行为）。
    let access =
        SearchAccess::new("http://127.0.0.1:99999/not-a-port", SENTINEL).expect("声明可以建立");
    let mut app = WebApp::with_policy(Some(access), test_policy()).expect("可以装配");
    let outcome = call(&mut app, search(query("没人应门")));
    assert_eq!(failure_of(&outcome), "transport-unreachable");
}

// ---------------------------------------------------------------------------
// 3. 五堵墙（取页）
// ---------------------------------------------------------------------------

#[test]
fn a_plain_page_comes_back_raw_with_its_source_and_size() {
    let body = "第一行\n第二行";
    let site = bind(1).remove(0).serve(vec![text(body)]);
    let mut app = fetch_app(test_policy());

    let outcome = call(&mut app, fetch(target(&site.endpoint)));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    let reading = text_of(&outcome);
    assert_eq!(
        reading,
        format!(
            "source = {}/\nbytes = {}\n{body}",
            site.endpoint,
            body.len()
        ),
        "取页回执 = 取自哪里 + 多大 + 正文原文"
    );
}

#[test]
fn markup_is_extracted_mechanically() {
    let site = bind(1).remove(0).serve(vec![html(
        "<html><head><style>.a{color:red}</style><script>var secret=1;</script></head>\
         <body><h1>标题</h1><p>正文 &amp; 更多</p><ul><li>甲</li><li>乙</li></ul></body></html>",
    )]);
    let mut app = fetch_app(test_policy());
    let outcome = call(&mut app, fetch(target(&site.endpoint)));
    assert_eq!(outcome.state(), ResultState::Success);
    let reading = text_of(&outcome);

    assert!(reading.contains("标题"), "{reading}");
    assert!(reading.contains("正文 & 更多"), "实体要解码：{reading}");
    assert!(reading.contains("甲"), "{reading}");
    assert!(reading.contains("乙"), "{reading}");
    assert!(!reading.contains("secret"), "脚本整段丢掉：{reading}");
    assert!(!reading.contains("color:red"), "样式整段丢掉：{reading}");
    assert!(!reading.contains('<'), "标记本身不进正文：{reading}");
    assert!(
        reading.lines().count() > 3,
        "块级标记是「一段完了」的记号 ⇒ 至少分几行：{reading}"
    );
}

#[test]
fn the_private_address_wall_refuses_before_connecting() {
    let page = bind(1).remove(0).serve(vec![text("不该到这里")]);
    let mut app = fetch_app(WebPolicy::production());
    let private = [
        format!("{}/", page.endpoint),
        "http://10.1.2.3/".to_string(),
        "http://192.168.1.1/".to_string(),
        "http://169.254.10.10/".to_string(),
        "http://[::1]/".to_string(),
        // IPv6 那一族：**字面量按解析判**之后才有这些格子。此前它们全落进
        // `transport-unreachable`——因为带方括号的串在 Linux 上解析不了（见 `ensure_public`）。
        "http://[fe80::1]/".to_string(),
        "http://[fd00::1]/".to_string(),
        "http://[::ffff:10.1.2.3]/".to_string(),
        "http://[::]/".to_string(),
    ];
    for address in private {
        let outcome = call(&mut app, fetch(target(&address)));
        assert_eq!(outcome.state(), ResultState::Failure, "{address}");
        assert_eq!(failure_of(&outcome), "private-address-refused", "{address}");
    }
    assert_eq!(page.connections(), 0, "拒绝发生在**连接之前**");
}

/// vouch 清单按**规范化后的主机文本**比：清单里写 `::1` 要认得出带方括号的写法。
///
/// 从前比的是 `host_str()`（IPv6 字面量带方括号）⇒ 清单里的 `::1` 永远认不出 ⇒
/// **部署点过名的内网 IPv6 服务用不了**。这条与上面那条是同一件事的两面：
/// 上面的地址被拒，是因为**没人点过名**；这里的地址放行，是因为**点过名了**。
#[test]
fn a_vouched_ipv6_literal_gets_through_the_wall() {
    let mut policy = test_policy();
    policy.allowed_private_hosts.push("::1".to_string());
    let mut app = fetch_app(policy);
    let outcome = call(&mut app, fetch(target("http://[::1]:9/")));
    assert_eq!(outcome.state(), ResultState::Failure, "回环上没有服务");
    assert_ne!(
        failure_of(&outcome),
        "private-address-refused",
        "清单点过名的字面量不该被墙拒（这里该是连不上）"
    );
}

#[test]
fn only_the_two_page_schemes_are_accepted() {
    let site = bind(1).remove(0).serve(vec![]);
    let mut app = fetch_app(test_policy());
    for address in [
        "file:///C:/windows/win.ini",
        "ftp://example.test/x",
        "gopher://example.test/",
        "这不是一个地址",
        "",
    ] {
        let outcome = call(&mut app, fetch(target(address)));
        assert_eq!(outcome.state(), ResultState::Failure, "{address}");
        assert_eq!(failure_of(&outcome), "invalid-argument", "{address}");
    }
    // 没有给参数也一样。
    let missing = call(&mut app, fetch(ActionParams::none()));
    assert_eq!(failure_of(&missing), "invalid-argument");
    assert_eq!(site.connections(), 0, "协议墙也在出网之前");
}

#[test]
fn an_overlarge_page_is_refused_not_truncated() {
    let exact = "x".repeat(MAX_PAGE_BYTES);
    let over = "x".repeat(MAX_PAGE_BYTES + 1);
    let site = bind(1).remove(0).serve(vec![text(&exact), text(&over)]);
    let mut app = fetch_app(test_policy());

    let ok = call(&mut app, fetch(target(&site.endpoint)));
    assert_eq!(ok.state(), ResultState::Success, "正好等于上限是允许的");
    assert!(
        text_of(&ok).contains(&format!("bytes = {MAX_PAGE_BYTES}")),
        "{}",
        text_of(&ok)
    );

    let refused = call(&mut app, fetch(target(&site.endpoint)));
    assert_eq!(refused.state(), ResultState::Failure);
    assert_eq!(
        failure_of(&refused),
        "too-large",
        "超限**拒绝**，不给半截（不截断）"
    );
}

#[test]
fn redirects_are_followed_up_to_the_limit_and_then_refused() {
    // 五跳：允许（每一跳都由我们自己跟）。先把地址定下来，再写各跳的回执。
    let sites = bind(6);
    let hop: Vec<String> = sites.iter().map(|site| site.endpoint.clone()).collect();
    // 逐个 serve：第 0..4 跳回 302 指向下一跳，第 5 跳回正文。
    let mut handles = Vec::new();
    for (index, site) in sites.into_iter().enumerate() {
        let reply = if index < 5 {
            redirect(&hop[index + 1])
        } else {
            text("终点")
        };
        handles.push(site.serve(vec![reply]));
    }

    let mut app = fetch_app(test_policy());
    let outcome = call(&mut app, fetch(target(&hop[0])));
    assert_eq!(outcome.state(), ResultState::Success, "五跳内应当跟完");
    let reading = text_of(&outcome);
    assert!(
        reading.contains(&format!("source = {}", hop[5])),
        "回执说的是**终点**（我们跟到哪儿了）：{reading}"
    );
    assert!(reading.contains("终点"), "{reading}");

    // 六跳：拒绝。
    let many = bind(7);
    let chain: Vec<String> = many.iter().map(|site| site.endpoint.clone()).collect();
    let mut hops = Vec::new();
    for (index, site) in many.into_iter().enumerate() {
        let reply = if index < 6 {
            redirect(&chain[index + 1])
        } else {
            text("不该到这里")
        };
        hops.push(site.serve(vec![reply]));
    }
    let mut app = fetch_app(test_policy());
    let refused = call(&mut app, fetch(target(&chain[0])));
    assert_eq!(refused.state(), ResultState::Failure);
    assert_eq!(failure_of(&refused), "redirect-loop", "超过上限即拒绝");
    drop(handles);
    drop(hops);
}

#[test]
fn a_redirect_loop_is_detected() {
    let mut sites = bind(2);
    let first = sites[0].endpoint.clone();
    let second = sites[1].endpoint.clone();
    let a = sites
        .remove(0)
        .serve(vec![redirect(&second), redirect(&second)]);
    let b = sites.remove(0).serve(vec![redirect(&first)]);
    let mut app = fetch_app(test_policy());

    let outcome = call(&mut app, fetch(target(&first)));
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(failure_of(&outcome), "redirect-loop");
    drop((a, b));
}

#[test]
fn a_redirect_into_the_private_range_is_refused_even_when_the_first_hop_was_allowed() {
    // 第一跳是调用方点名的目标（策略允许落在回环），它却把我们往内网指 ⇒ 严格面拒绝。
    let page = bind(1)
        .remove(0)
        .serve(vec![redirect("http://10.1.2.3/inside")]);
    let mut app = fetch_app(test_policy());
    let outcome = call(&mut app, fetch(target(&page.endpoint)));
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(
        failure_of(&outcome),
        "private-address-refused",
        "放宽只对第一跳成立：重定向的目标一律按严格面看"
    );
    assert_eq!(page.connections(), 1, "只发了第一跳那一次请求");
}

#[test]
fn a_relative_redirect_is_resolved_against_the_current_address() {
    // 相对地址按**当前地址**解析：`/next` 落在同一台服务器上，所以同一个站点给两条回执。
    let site = bind(1).remove(0);
    let origin = site.endpoint.clone();
    let served = site.serve(vec![redirect("/next"), text("相对跳转也跟得上")]);
    let mut app = fetch_app(test_policy());

    let outcome = call(&mut app, fetch(target(&origin)));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    assert!(
        text_of(&outcome).contains(&format!("source = {origin}/next")),
        "相对地址按当前地址解析（同一台服务器、同一个端口）：{}",
        text_of(&outcome)
    );
    assert_eq!(served.connections(), 2, "两跳：先 302、再取到正文");
}

#[test]
fn non_text_content_is_refused() {
    let site = bind(1).remove(0).serve(vec![
        with_kind("不是文本", "application/pdf"),
        with_kind("不是文本", "image/png"),
        Reply::Answer {
            status: 200,
            headers: vec![],
            body: "没说是什么".to_string(),
        },
    ]);
    let mut app = fetch_app(test_policy());
    for expected in ["application/pdf", "image/png", "没有类型"] {
        let outcome = call(&mut app, fetch(target(&site.endpoint)));
        assert_eq!(outcome.state(), ResultState::Failure, "{expected}");
        assert_eq!(failure_of(&outcome), "wrong-kind", "{expected}");
    }
}

#[test]
fn a_silent_site_is_a_timeout() {
    let site = bind(1).remove(0).serve(vec![Reply::Silent]);
    let mut app = fetch_app(test_policy());
    let outcome = call(&mut app, fetch(target(&site.endpoint)));
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(failure_of(&outcome), "timeout");
}

// ---------------------------------------------------------------------------
// 4. 能力面与效果声明
// ---------------------------------------------------------------------------

#[test]
fn both_verbs_are_declared_read_only_and_preinstalled() {
    let app = fetch_app(WebPolicy::production());
    assert_eq!(app.app_id(), WEB_APP_ID);

    let capabilities = app.declared_capabilities();
    let ids: Vec<String> = capabilities
        .items()
        .iter()
        .map(|capability| capability.id().as_str().to_string())
        .collect();
    assert_eq!(
        ids,
        vec![CAPABILITY_SEARCH, CAPABILITY_FETCH],
        "两条，顺序固定"
    );

    let effects = app.declared_effects();
    assert_eq!(effects.len(), 2, "两条动词都声明");
    for effect in &effects {
        assert_eq!(effect.class(), EffectClass::ReadOnly, "两条都是只读");
        assert!(
            effect.verification().is_none(),
            "只读没有可配的复核（复核是写效应的配对物）"
        );
    }
}

/// **两条动词是显式只读**：读它们自己的声明，并与一件**真的写**能力对照。
///
/// ⚠ **旧规矩 → 新规矩**（2026-09-26，两刀）：这条用例原来把各 App 的声明收进一张**效果声明表**
/// 再问它（"未决效果门读的就是这张表"）。门随跨进程恢复退役，它读的那张表也在本刀退役——**门是
/// 那张表唯一的生产读者**，门一走它就只剩"装配期收集、之后无人读"这一半。读数回到**权威源**：
/// 各 App 自己的 `declared_effects()`。**对照那一半一个字没松**——它才是这条断言承重的地方
/// （如果只读声明写错了，两次都会说"不是写"，那样它就永远绿）。
#[test]
fn the_two_verbs_are_read_only_and_the_contrast_is_a_real_write() {
    // 对照那半边用 im（它声明了一件真的写：`im_say`）——它不需要碰文件系统。
    let access = celira_agent::im_channel::ImAccess::new("http://127.0.0.1:1/", SENTINEL)
        .expect("端点与凭据非空");
    let im = celira_agent::im_app::ImApp::new(access).expect("可以装配 im app");
    let web = fetch_app(WebPolicy::production());

    let web_declared = web.declared_effects();
    for verb in [CAPABILITY_SEARCH, CAPABILITY_FETCH] {
        let declared = web_declared
            .iter()
            .find(|effect| effect.action().as_str() == verb)
            .unwrap_or_else(|| panic!("{verb} 必须自己说出来（不声明也是只读，但这两条是显式的）"));
        assert_eq!(
            declared.class(),
            EffectClass::ReadOnly,
            "{verb} 声明的是只读"
        );
        assert!(
            declared.verification().is_none(),
            "只读没有可配的复核（复核是写效应的配对物）"
        );
    }
    // 对照：一件真的写能力必须说「是写」。
    assert!(
        im.declared_effects()
            .iter()
            .any(|effect| effect.action().as_str() == "im_say" && effect.class().is_write()),
        "对照：写能力必须说「是写」（否则上面两条断言什么都不证明）"
    );
}

/// **出网档决定"在不在场"**（2026-10-13）：`off` ⇒ 这一档**不摆这件 App**（她的门里连这扇门都
/// 没有）；`fetch` ⇒ 摆，但**「查」不进声明面**；`full` ⇒ 两条都在场（出厂以来的样子）。
///
/// ⚠ 与**凭据**那件事分得开：凭据管"能不能执行"（没配 ⇒ 动词照旧在场、执行时如实说没配），
/// 档位管"在不在场"（不在 ⇒ 她读到的那扇门里就没有它）。两者是两件事，不许混成一格。
#[test]
fn the_tier_decides_whether_the_window_and_the_search_verb_are_in_play() {
    use celira_agent::app_contract::AppPort;
    use celira_agent::web_app::WebTier;

    let declared = |tier: WebTier| -> Option<Vec<String>> {
        let app = WebApp::with_tier(None, tier, test_policy())?;
        let mut ids: Vec<String> = app
            .declared_capabilities()
            .items()
            .iter()
            .map(|capability| capability.id().as_str().to_string())
            .collect();
        ids.sort();
        Some(ids)
    };

    assert_eq!(declared(WebTier::Off), None, "off ⇒ 不摆这件 App");
    assert_eq!(
        declared(WebTier::Fetch),
        Some(vec!["web_fetch".to_string()]),
        "fetch 档里「查」不在场"
    );
    assert_eq!(
        declared(WebTier::Full),
        Some(vec!["web_fetch".to_string(), "web_search".to_string()]),
        "full 档 = 出厂以来的样子"
    );
    // 三个词是**封闭**的：不认识的档位没有解析结果（调用方据此 fail-closed，不猜一个近似档）。
    assert_eq!(WebTier::parse("sometimes"), None);
    for word in ["off", "fetch", "full"] {
        assert_eq!(
            WebTier::parse(word).map(WebTier::as_str),
            Some(word),
            "词表逐条对称"
        );
    }
}
