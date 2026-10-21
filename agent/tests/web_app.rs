//! TASK_35 验收：web 窗的两条只读动词——线形状、五堵墙、失败词表、效果与能力面。
//!
//! 这一份用**离线回环假服务**（脚本化回执 + 请求原文捕获 + 连接计数）证明四件事：
//!
//! 1. **线形状**：搜索请求的路径、头与体逐项对得上；结果渲染有界（**地址除外**——它是把手，
//!    剪过的地址比少一行更坏）；哨兵凭据不进任何输出；
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
    CAPABILITY_FETCH, CAPABILITY_SEARCH, FetchAccess, FetchWire, MAX_ADDRESS_BYTES,
    MAX_FIELD_BYTES, MAX_QUERY_BYTES, MAX_RESULTS, SearchAccess, SearchWire, WEB_APP_ID, WebApp,
    WebPolicy, WebTier,
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

/// 把假站点当**搜索接口那一种方言**的搜索源接进来（2026-10-20）。
fn search_api_app(site: &Site) -> WebApp {
    let endpoint = format!("{}/v1/tools/search", site.endpoint);
    let access =
        SearchAccess::with_wire(SearchWire::SearchApi, endpoint, SENTINEL).expect("声明可以建立");
    WebApp::with_policy(Some(access), test_policy()).expect("可以装配 web 窗")
}

/// 把假站点当**抓取源**接进来（2026-10-20：抓取也走对面 API）。
fn fetch_app(site: &Site, wire: FetchWire) -> WebApp {
    let endpoint = format!("{}/v1/tools/fetch", site.endpoint);
    let access = FetchAccess::new(wire, endpoint, SENTINEL).expect("声明可以建立");
    WebApp::with_sources(None, Some(access), WebTier::Full, test_policy()).expect("可以装配 web 窗")
}

/// 没配任何来源的 web 窗（两个动词都该说 `credential-absent`）。
fn bare_app() -> WebApp {
    WebApp::with_policy(None, test_policy()).expect("可以装配 web 窗")
}

/// 用例策略：窗口调小（超时那一格不用等 30 秒）。
fn test_policy() -> WebPolicy {
    WebPolicy {
        request_timeout: Duration::from_millis(400),
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
fn search_results_are_bounded_and_the_display_columns_are_truncated() {
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
        Some(
            format!("results = {MAX_RESULTS}, truncated = true, addresses_over_limit = 0").as_str()
        ),
        "多出来的如实报截断：{reading}"
    );
    let body_lines: Vec<&str> = lines.collect();
    assert_eq!(body_lines.len(), MAX_RESULTS, "只列上限条数：{reading}");
    for line in &body_lines {
        let fields: Vec<&str> = line.split(" | ").collect();
        assert_eq!(fields.len(), 4, "每条四段：{line}");
        // 展示栏（标题 / 摘要 / 日期）有界；**地址那一栏不参与截断**（见下一条用例）。
        for field in [fields[0], fields[2], fields[3]] {
            assert!(
                field.len() <= MAX_FIELD_BYTES + '…'.len_utf8(),
                "每一展示栏都要有界（字符边界上截断）：{field} ({})",
                field.len()
            );
        }
        assert!(fields[2].ends_with('…'), "被截断的那一栏要说出口：{line}");
        assert_eq!(fields[3], "-", "厂商没给日期就照实说没有：{line}");
    }
}

/// **地址是把手，不参与展示截断**（2026-10-20）：能力说明写着"把那一行的地址原样交给
/// `web_fetch`"——剪过的地址要么解析不了、要么取到别处。这一条拿一个远超展示预算的地址
/// （真实世界里带跟踪参数的那种）钉住：**逐字节原样**回来。
///
/// ⚠ 旧用例只把摘要那一栏弄长，地址那一栏从来没量过——那正是它被截断而没人发现的原因。
#[test]
fn an_address_is_never_cut_even_when_it_exceeds_the_display_budget() {
    let address = format!(
        "https://example.test/{}?{}",
        "a".repeat(300),
        "k=v&".repeat(200)
    );
    assert!(
        address.len() > MAX_FIELD_BYTES,
        "这一条要量的是**超过展示预算**的地址"
    );
    let site = bind(1).remove(0).serve(vec![with_kind(
        &results_body(&[("标题甲", address.as_str(), "摘要甲")]),
        "application/json",
    )]);
    let mut app = search_app(&site);

    let outcome = call(&mut app, search(query("长地址")));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    let reading = text_of(&outcome);
    assert!(reading.contains(&address), "地址一个字都不许少：{reading}");
    assert!(
        !reading.lines().skip(1).any(|line| line.contains('…')),
        "地址那一栏不该带省略号：{reading}"
    );
}

/// 地址超过**把手上限**：那一行**整个不列**、首行点数——不剪成半截。
/// 判据：少一行（她还能用其余几行）比给一个假地址（她会去取别的页）诚实。
#[test]
fn a_row_whose_address_exceeds_the_handle_cap_is_counted_not_listed() {
    let too_long = format!("https://example.test/{}", "x".repeat(MAX_ADDRESS_BYTES));
    let body = results_body(&[
        ("能用的那一条", "https://example.test/ok", "摘要甲"),
        ("地址太长的那一条", too_long.as_str(), "摘要乙"),
    ]);
    let site = bind(1)
        .remove(0)
        .serve(vec![with_kind(&body, "application/json")]);
    let mut app = search_app(&site);

    let outcome = call(&mut app, search(query("超限地址")));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    let reading = text_of(&outcome);
    assert!(
        reading.starts_with("results = 1, truncated = false, addresses_over_limit = 1"),
        "首行要把丢掉的那几行说出来：{reading}"
    );
    assert!(reading.contains("https://example.test/ok"), "{reading}");
    assert!(
        !reading.contains("地址太长"),
        "超限的那一行整个不列（不是剪一半）：{reading}"
    );
}

/// 日期那一栏：厂商回执里带就读出来，没带就是 `-`——**不猜、不从正文推断**。
#[test]
fn the_date_column_carries_only_what_the_vendor_says() {
    let body = serde_json::json!({
        "results": [
            {"title": "有日期", "url": "https://example.test/dated", "content": "摘要甲", "published_date": "2026-10-20"},
            {"title": "没日期", "url": "https://example.test/undated", "content": "摘要乙"},
        ]
    })
    .to_string();
    let site = bind(1)
        .remove(0)
        .serve(vec![with_kind(&body, "application/json")]);
    let mut app = search_app(&site);

    let outcome = call(&mut app, search(query("日期")));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    let reading = text_of(&outcome);
    assert!(
        reading.contains("有日期 | https://example.test/dated | 摘要甲 | 2026-10-20"),
        "{reading}"
    );
    assert!(
        reading.contains("没日期 | https://example.test/undated | 摘要乙 | -"),
        "缺的那一栏照实说 `-`：{reading}"
    );
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
        reading.contains("只有标题 | https://example.test/x | - | -"),
        "缺的栏照实说 `-`，不静默省略：{reading}"
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

// ---------------------------------------------------------------------------
// 4. 能力面与效果声明
// ---------------------------------------------------------------------------

#[test]
fn both_verbs_are_declared_read_only_and_preinstalled() {
    let app = bare_app();
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
    let web = bare_app();

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

// ---------------------------------------------------------------------------
// 1b. 第二个方言：一家的**搜索接口**（2026-10-20）
// ---------------------------------------------------------------------------

/// 方言二说自己的形状：请求体 `{query, count, need_content}`，回执是 `results[]`——
/// 摘要取 `summary`（给模型读的长摘要），缺了退回 `snippet`；时间取 `date`。
#[test]
fn the_search_api_dialect_sends_its_own_body_and_reads_the_official_fields() {
    let body = serde_json::json!({
        "results": [
            {
                "title": "甲",
                "url": "https://a.example/1",
                "snippet": "短甲",
                "summary": "长甲",
                "date": "2026-06-01T00:00:00+00:00"
            },
            {
                "title": "乙",
                "url": "https://b.example/2",
                "snippet": "只有短摘要"
            }
        ]
    })
    .to_string();
    let site = bind(1)
        .remove(0)
        .serve(vec![with_kind(&body, "application/json")]);
    let mut app = search_api_app(&site);

    let outcome = call(&mut app, search(query("今天发生了什么")));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    let reading = text_of(&outcome);
    assert!(
        reading.starts_with("results = 2, truncated = false, addresses_over_limit = 0"),
        "{reading}"
    );
    assert!(
        reading.contains("甲 | https://a.example/1 | 长甲 | 2026-06-01T00:00:00+00:00"),
        "摘要取 summary、时间取 date：{reading}"
    );
    assert!(
        reading.contains("乙 | https://b.example/2 | 只有短摘要 | -"),
        "没有 summary 就退回 snippet；没有 date 就是 `-`：{reading}"
    );

    let request = site.next_request().to_ascii_lowercase();
    assert!(
        request.starts_with("post /v1/tools/search "),
        "端点就是配置的那一个：{request}"
    );
    assert!(
        request.contains("authorization: bearer tvly-sentinel-credential"),
        "凭据进请求头：{request}"
    );
    assert!(request.contains("\"count\":5"), "{request}");
    assert!(request.contains("\"need_content\":false"), "{request}");
    assert!(request.contains("今天发生了什么"), "{request}");
    assert!(
        !reading.contains("tvly-sentinel-credential"),
        "凭据不进回执"
    );
    assert_eq!(site.connections(), 1, "一次调用恰好一次出网请求");
}

/// 没匹配到是**正常结束**（对面明文写着返回空数组），不是失败——如实给一行 `results = 0`。
#[test]
fn no_matches_is_an_empty_reading_not_a_failure() {
    let body = serde_json::json!({ "results": [] }).to_string();
    let site = bind(1)
        .remove(0)
        .serve(vec![with_kind(&body, "application/json")]);
    let mut app = search_api_app(&site);

    let outcome = call(&mut app, search(query("一个没有结果的词")));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    assert_eq!(
        text_of(&outcome),
        "results = 0, truncated = false, addresses_over_limit = 0\n"
    );
    assert_eq!(site.connections(), 1);
}

/// `search-api` 方言**自己的**字数上限：129 个字符本地就拒、零出网。
///
/// 判据：放过去只会换来对面一个 400，而 400 在这里会被归成 `server-error` —— 同一件事分成两格，
/// 操作者会去查"服务端怎么了"，而真正该做的是**把查询写短**。
#[test]
fn the_search_api_dialect_refuses_an_overlong_query_before_going_out() {
    let site = bind(1)
        .remove(0)
        .serve(vec![with_kind("{}", "application/json")]);
    let mut app = search_api_app(&site);

    let outcome = call(&mut app, search(query(&"a".repeat(129))));
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(failure_of(&outcome), "invalid-argument");
    assert_eq!(site.connections(), 0, "本地拒绝：一次出网请求都不发");
}

// ---------------------------------------------------------------------------
// 5. 抓取也走对面 API（2026-10-20：与搜索同形，**没有自己抓的回退**）
// ---------------------------------------------------------------------------

/// 方言一（`fetch-doc`）：请求 `{url}`，回执 `{title, content}`；读数首行三格，
/// 其中 `via` 说明这一页是**谁取的**。
#[test]
fn a_vendor_fetch_comes_back_with_source_via_and_bytes() {
    let body = serde_json::json!({ "title": "标题", "content": "# 正文\n第二行" }).to_string();
    let site = bind(1)
        .remove(0)
        .serve(vec![with_kind(&body, "application/json")]);
    let mut app = fetch_app(&site, FetchWire::Doc);

    let outcome = call(&mut app, fetch(target("https://example.test/a")));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    let reading = text_of(&outcome);
    assert!(
        reading.starts_with("source = https://example.test/a\nvia = fetch-doc\nbytes = "),
        "首行三格：{reading}"
    );
    assert!(reading.contains("# 正文"), "正文原样搬回来：{reading}");

    let request = site.next_request().to_ascii_lowercase();
    assert!(
        request.starts_with("post /v1/tools/fetch "),
        "端点就是配置的那一个：{request}"
    );
    assert!(
        request.contains("authorization: bearer tvly-sentinel-credential"),
        "凭据进请求头：{request}"
    );
    assert!(
        request.contains("\"url\":\"https://example.test/a\""),
        "{request}"
    );
    assert!(
        !reading.contains("tvly-sentinel-credential"),
        "凭据不进回执"
    );
    assert_eq!(site.connections(), 1, "一次调用恰好一次请求");
}

/// 方言二（`fetch-raw`）：请求 `{urls:[url], format:"markdown"}`，回执取 `results[0].raw_content`。
/// ⚠ **200 ＋ 空 `results` 是失败**（对面没取到），不是"这一页是空的"。
#[test]
fn a_raw_fetch_reads_the_first_result_and_empty_results_is_a_failure() {
    let ok = serde_json::json!({
        "results": [{ "url": "https://example.test/a", "raw_content": "# 正文" }],
        "failed_results": []
    })
    .to_string();
    let site = bind(1)
        .remove(0)
        .serve(vec![with_kind(&ok, "application/json")]);
    let mut app = fetch_app(&site, FetchWire::Raw);

    let outcome = call(&mut app, fetch(target("https://example.test/a")));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "{}",
        text_of(&outcome)
    );
    let reading = text_of(&outcome);
    assert!(reading.contains("via = fetch-raw"), "{reading}");
    assert!(reading.contains("# 正文"), "{reading}");
    let request = site.next_request().to_ascii_lowercase();
    assert!(
        request.contains("\"urls\":[\"https://example.test/a\"]"),
        "{request}"
    );
    assert!(request.contains("\"format\":\"markdown\""), "{request}");

    let empty = serde_json::json!({
        "results": [],
        "failed_results": [{ "url": "https://example.test/a", "error": "Failed to retrieve content" }]
    })
    .to_string();
    let site = bind(1)
        .remove(0)
        .serve(vec![with_kind(&empty, "application/json")]);
    let mut app = fetch_app(&site, FetchWire::Raw);
    let outcome = call(&mut app, fetch(target("https://example.test/a")));
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(failure_of(&outcome), "server-error");
    assert_eq!(site.connections(), 1, "失败也不重试");
}

/// **没配来源**：两个动词都如实说 `credential-absent`——与搜索同形，没有"自己抓"这一条。
#[test]
fn without_a_source_both_verbs_say_credential_absent() {
    let mut app = bare_app();
    let searching = call(&mut app, search(query("青苔")));
    assert_eq!(failure_of(&searching), "credential-absent");
    let fetching = call(&mut app, fetch(target("https://example.test/a")));
    assert_eq!(failure_of(&fetching), "credential-absent");
    assert_eq!(
        failure_of(&fetching),
        failure_of(&searching),
        "两个动词同一句话"
    );
}
