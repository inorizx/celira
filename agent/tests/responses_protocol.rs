//! TASK_44 验收：**第二条线协议**（OpenAI Responses）的 wire 形状与全链闭合。
//!
//! 这一份文件用**真实适配层 + 回环 HTTP 服务**跑真正的请求，与 `provider_wire.rs`（chat 侧）
//! 同一套姿势：
//!
//! ```text
//! View → ResponsesModel → 一次 POST /responses → wire 响应 → ModelOutput → Core 继续判断
//! ```
//!
//! 它证明**协议这一层**的四件事（施工单 §4 的四条判据）：
//!
//! 1. **wire 断言**：`store: false`、tools 平铺、`instructions` 载契约、`input` 条目序列与
//!    `function_call` ↔ `function_call_output` **相邻配对**、reasoning 条目带 `encrypted_content`
//!    才原样回传（不带就不出现）；
//! 2. **全链**：主体调工具 → 夹具应答 → 回执进下一轮 → 下一轮正确——**回执里那个值只有对面知道**
//!    （夹具按测试给的一次性串生成），因此"带过去了"这件事是可判错的；
//! 3. **路由**：`responses` 没有端点 ⇒ 入口 exit 5（配对纪律，不猜）；缺省仍是 chat；
//! 4. **占用读数**：平铺的 `input_tokens` + `output_tokens` 进状态行；读不到如实 `unknown`。
//!
//! 明确**不**声称的：回环只证明本层这一条协议路径，不证明公网服务可用；夹具是照形状应答的替身，
//! 不证明任何模型的智能效果。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::mpsc;

use serde_json::{Value, json};
mod common;
use common::binary;

/// 哨兵凭据：只用于回环认证，任何输出里出现它就说明越界了。
const SENTINEL_KEY: &str = "fixture-sentinel-credential";

// ---------------------------------------------------------------------------
// 判据 1 + 2：wire 形状、相邻配对、推理条目回传、回执进下一轮
// ---------------------------------------------------------------------------

#[test]
fn the_request_is_a_responses_request_and_a_call_is_paired_with_its_receipt() {
    let temp = TempDir::new("responses-wire");
    let env_root = temp.env("agent-responses");
    let materials = temp.materials();
    let input = "这次活动的输入正文";
    // 只有**对面**知道的一个值：它出现在夹具的第一次答复里，测试随后断言它进了下一轮。
    let nonce = format!("responses-fixture-{}", std::process::id());

    let (endpoint, requests, extra) = spawn_responses_server(vec![
        (
            200,
            call_body(
                "call-1",
                "open_app",
                "{\"target\":\"notes\"}",
                Some(json!({
                    "type": "reasoning",
                    "id": "rs_1",
                    "summary": [],
                    "encrypted_content": nonce,
                })),
            ),
        ),
        (200, finish_body()),
    ]);
    let run = Fixture::new(&env_root, "agent-responses", &materials, "run-1", &endpoint)
        .input(input)
        .run();
    assert_eq!(code(&run), 0, "{}", stderr(&run));
    assert_eq!(field(&stdout(&run), "ending"), "subject:ended");

    let seen = all_requests(&requests);
    let first = seen[0].clone();
    let wire = body_of(&first);

    // **两条 wire 都自报家门**（2026-10-06，接 opencode go 那一刀）：取值是**同一个
    // `pub(crate)` 共用件**（`provider_adapter::CLIENT_USER_AGENT`，源自 `Cargo.toml`），
    // chat 侧那条在 `tests/provider_wire.rs` 里有一模一样的断言 —— 这一条证明 Responses
    // 这条也没漏。断言的是**产品名**，不绑版本号（理由同上）。
    let lowered = first.to_ascii_lowercase();
    assert!(
        lowered.contains("user-agent: celira-agent/"),
        "Responses 这条 wire 也必须自报家门: {}",
        redacted(&first)
    );
    assert!(
        !lowered.contains("user-agent: reqwest/"),
        "不许拿 HTTP 库的名字当我们的身份: {}",
        redacted(&first)
    );

    // **那一段对话是谁**（2026-10-09）：与 chat 侧同一个头、同一个取值（共用件
    // `provider_adapter::SESSION_HEADER`）。实测（opencode go 的 zen 网关）**两条 wire 都要求它**、
    // 空值也算没带 —— 这条断言证明 Responses 这条没漏（chat 侧那条在 `tests/provider_wire.rs`）。
    assert!(
        lowered.contains("x-opencode-session: run-1"),
        "Responses 这条 wire 也必须带上'这一段对话是谁'那一格（值＝这一段活动的 id）: {}",
        redacted(&first)
    );
    assert!(
        !lowered.contains("x-opencode-session: \r") && !lowered.contains("x-opencode-session: \n"),
        "那一格不许是空的（空值对面照样 400）: {}",
        redacted(&first)
    );
    // —— 形状：这是 Responses 的请求，不是 chat 的 ——
    assert!(
        first.contains("POST /responses"),
        "端点路径就是宿主给的完整地址: {}",
        redacted(&first)
    );
    assert_eq!(wire["store"], json!(false), "状态主权：必带 store: false");
    assert!(wire.get("messages").is_none(), "它不该有 chat 的 messages");
    assert!(
        wire.get("instructions").is_some(),
        "系统指令走独立字段: {}",
        redacted(&first)
    );
    assert!(
        wire["instructions"]
            .as_str()
            .unwrap_or_default()
            .contains("finish_run"),
        "instructions 里要载着决定契约: {}",
        redacted(&first)
    );
    assert!(
        !wire["instructions"]
            .as_str()
            .unwrap_or_default()
            .contains(input),
        "初始输入不得进入 instructions"
    );
    // tools 平铺：`type` / `name` 在同一层，没有 chat 那层 `function` 嵌套。
    let tools = wire["tools"].as_array().expect("tools 是数组");
    let entry = tools
        .iter()
        .find(|tool| tool["name"] == json!("invoke"))
        .expect("头部恒定两件：入口在");
    assert_eq!(entry["type"], json!("function"));
    assert!(entry.get("function").is_none(), "平铺，没有 function 嵌套");
    assert_eq!(tools.len(), 3, "头部恰好三件（入口 ＋ 收场 ＋ 搁下）");
    assert!(
        !tools.iter().any(|tool| tool["name"] == json!("open_app")),
        "OS 动词与 app 动词都不再直连头部"
    );
    assert!(
        tools.iter().any(|tool| tool["name"] == json!("finish_run")),
        "结束类工具永远在"
    );
    assert!(
        tools
            .iter()
            .all(|tool| tool.get("strict").is_none() && tool.get("parameters").is_some()),
        "每条声明都带 parameters"
    );
    // input 条目：第一项是初始输入（**不是** system），最后一项是运行动静。
    let items = wire["input"].as_array().expect("input 是条目数组");
    assert!(
        items
            .first()
            .and_then(|item| item["content"].as_str())
            .unwrap_or_default()
            .contains(input),
        "初始输入作为**非 system** 正文排在最前: {}",
        redacted(&first)
    );
    assert!(
        items
            .last()
            .and_then(|item| item["content"].as_str())
            .unwrap_or_default()
            .contains("ACTIVITY FACTS"),
        "运行动静排在最末: {}",
        redacted(&first)
    );
    assert!(
        items.iter().all(|item| item.get("type").is_none()),
        "首轮只有正文条目（还没有调用、回执或推理）: {}",
        redacted(&first)
    );

    // —— 第二轮：相邻配对 + 推理条目原样回传 ——
    let second = seen[1].clone();
    let wire = body_of(&second);
    let items = wire["input"].as_array().expect("input 是条目数组");
    let kinds: Vec<&str> = items
        .iter()
        .map(|item| item["type"].as_str().unwrap_or("body"))
        .collect();
    let call = kinds
        .iter()
        .position(|kind| *kind == "function_call")
        .unwrap_or_else(|| panic!("要有一条调用: {kinds:?}"));
    // **相邻配对**：调用与它的回执紧挨着，中间没有别的东西（这正是摊平历史会丢掉的形状）。
    assert_eq!(
        kinds.get(call + 1),
        Some(&"function_call_output"),
        "{kinds:?}"
    );
    // 推理条目在调用**之前**（服务端本来就在前面），因此不会插进配对之间。
    assert_eq!(kinds.get(call - 1), Some(&"reasoning"), "{kinds:?}");
    assert_eq!(
        items[call - 1]["encrypted_content"],
        json!(nonce),
        "推理条目**原样**回传（连加密内容一起）: {}",
        redacted(&second)
    );
    assert_eq!(items[call]["call_id"], json!("call-1"));
    assert_eq!(items[call]["name"], json!("open_app"));
    assert_eq!(items[call]["arguments"], json!("{\"target\":\"notes\"}"));
    assert_eq!(
        items[call + 1]["call_id"],
        json!("call-1"),
        "回执用**同一个**调用标识配对"
    );
    assert!(
        items[call + 1]["output"]
            .as_str()
            .unwrap_or_default()
            .contains("open_app"),
        "回执正文要带上那次行动: {}",
        redacted(&second)
    );

    // **一次 complete 只发一次请求**：夹具脚本用完之后仍然盯着，多出来的连接会被算进去。
    assert_eq!(
        extra
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or(usize::MAX),
        0,
        "零自动重试"
    );
}

/// 推理条目**不带** `encrypted_content` 时：**不**回传，也不伪造一条空的。
#[test]
fn a_reasoning_item_without_encrypted_content_is_not_replayed() {
    let temp = TempDir::new("responses-plain-reasoning");
    let env_root = temp.env("agent-plain");
    let materials = temp.materials();

    let (endpoint, requests, _extra) = spawn_responses_server(vec![
        (
            200,
            call_body(
                "call-1",
                "open_app",
                "{\"target\":\"notes\"}",
                Some(
                    json!({"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "想了一下"}]}),
                ),
            ),
        ),
        (200, finish_body()),
    ]);
    let run = Fixture::new(&env_root, "agent-plain", &materials, "run-1", &endpoint)
        .input("输入")
        .run();
    assert_eq!(code(&run), 0, "{}", stderr(&run));

    let second = all_requests(&requests)[1].clone();
    let wire = body_of(&second);
    let items = wire["input"].as_array().expect("input 是条目数组");
    let kinds: Vec<&str> = items
        .iter()
        .map(|item| item["type"].as_str().unwrap_or("body"))
        .collect();
    assert!(
        !kinds.contains(&"reasoning"),
        "没有加密内容就没有可回传的推理链——不伪造一条空的: {kinds:?}"
    );
    let call = kinds
        .iter()
        .position(|kind| *kind == "function_call")
        .unwrap_or_else(|| panic!("要有一条调用: {kinds:?}"));
    assert_eq!(
        kinds.get(call + 1),
        Some(&"function_call_output"),
        "{kinds:?}"
    );
}

// ---------------------------------------------------------------------------
// 判据 4：占用读数（平铺字段）
// ---------------------------------------------------------------------------

#[test]
fn the_input_side_of_the_flat_usage_reaches_the_context_line_and_a_missing_usage_reads_unknown() {
    let temp = TempDir::new("responses-usage");
    let env_root = temp.env("agent-usage");
    let materials = temp.materials();

    // 第一轮：报输入侧 5、输出侧 7（输出侧不计入占用）；第二轮：**不报** usage（读不到就如实 unknown）。
    let with_usage = "{\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":5,\"output_tokens\":7}}";
    let without_usage = "{\"status\":\"completed\",\"output\":[]}";
    let (endpoint, requests, _extra) = spawn_responses_server(vec![
        // 第一轮：一次纯正文（交出控制，活动继续到下一轮宿主输入）。
        (200, with_usage.replace("\"output\":[]", "\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"说一句\"}]}]")),
        (200, finish_body()),
    ]);
    let run = Fixture::new(&env_root, "agent-usage", &materials, "run-1", &endpoint)
        .input("输入")
        .window(1000)
        .host_lines("\n")
        .run();
    assert_eq!(code(&run), 0, "{}", stderr(&run));

    let second = all_requests(&requests)[1].clone();
    assert!(
        second.contains("context = 5/1000 token"),
        "输入侧那一项要进状态行（输出侧 7 不计入占用）: {}",
        redacted(&second)
    );

    // 第三轮前的那一次请求：夹具**没有**报 usage ⇒ 状态行如实 unknown。
    let temp2 = TempDir::new("responses-usage-missing");
    let env2 = temp2.env("agent-usage2");
    let materials2 = temp2.materials();
    let (endpoint2, requests2, _extra2) = spawn_responses_server(vec![
        (200, without_usage.replace("\"output\":[]", "\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"说一句\"}]}]").to_string()),
        (200, finish_body()),
    ]);
    let run2 = Fixture::new(&env2, "agent-usage2", &materials2, "run-1", &endpoint2)
        .input("输入")
        .window(1000)
        .host_lines("\n")
        .run();
    assert_eq!(code(&run2), 0, "{}", stderr(&run2));
    let second2 = all_requests(&requests2)[1].clone();
    assert!(
        second2.contains("context = unknown/1000 token"),
        "读不到就如实 unknown（不伪造读数）: {}",
        redacted(&second2)
    );
}

/// 非 2xx：**分类与 chat 侧同款**（同一份九类 ＋ 同一份细分档），而且要如实落到事实里——
/// 不是笼统的"模型失败"，也不是丢了细分的裸类名。两个协议走同一张折叠表，这一格就是那条的判据。
#[test]
fn an_http_status_failure_carries_its_class_into_the_facts() {
    let temp = TempDir::new("responses-status");
    let env_root = temp.env("agent-status");
    let materials = temp.materials();

    let (endpoint, _requests, _extra) = spawn_responses_server(vec![(500, "{}".to_string())]);
    let run = Fixture::new(&env_root, "agent-status", &materials, "run-1", &endpoint)
        .input("输入")
        .run();
    assert_eq!(code(&run), 7, "{}", stderr(&run));
    let report = stdout(&run);
    assert_eq!(field(&report, "ending"), "system:model-failed", "{report}");
    assert!(
        report.contains("model-failure=http-status-failure:server-side"),
        "终端必须给出分类（含细分档）: {report}"
    );

    let facts = std::fs::read_to_string(env_root.join("facts.ndjson")).expect("读事实存储");
    let ended = facts
        .lines()
        .find(|line| line.contains("\"fact\":\"run-ended\""))
        .expect("必须有收场行");
    assert!(
        ended.contains("\"failure\":\"http-status-failure:server-side\""),
        "分类必须落盘（含细分档）: {ended}"
    );
}

// ---------------------------------------------------------------------------
// 判据 3：路由（配对纪律）
// ---------------------------------------------------------------------------

/// `responses` 只在**换家档**有效：没有 `--model-endpoint` 就是 exit 5，而且**一次模型都没装配**。
#[test]
fn responses_without_an_endpoint_is_refused_by_the_entry() {
    let temp = TempDir::new("responses-routing");
    let env_root = temp.env("agent-routing");
    let materials = temp.materials();

    let refused = run_entry(&[
        "run",
        "--root",
        &path_text(&env_root),
        "--agent",
        "agent-routing",
        "--materials",
        &path_text(&materials),
        "--run",
        "run-1",
        "--max-turns",
        "1",
        "--model-protocol",
        "responses",
    ]);
    assert_eq!(code(&refused), 5, "{}", stderr(&refused));
    assert!(
        stderr(&refused).contains("只在换家档有效"),
        "要说清为什么: {}",
        stderr(&refused)
    );

    // 不认识的协议名同样是入口拒绝（不落到"随便挑一个"）。
    let unknown = run_entry(&[
        "run",
        "--root",
        &path_text(&env_root),
        "--agent",
        "agent-routing",
        "--materials",
        &path_text(&materials),
        "--run",
        "run-2",
        "--max-turns",
        "1",
        "--model-protocol",
        "not-a-protocol",
    ]);
    assert_eq!(code(&unknown), 5, "{}", stderr(&unknown));
    assert!(stderr(&unknown).contains("不认识"), "{}", stderr(&unknown));
}

/// 缺省仍是 chat：显式写 `chat-completions` 与**不写**这条旗标，请求体逐字节同形。
#[test]
fn the_default_protocol_is_still_chat_completions() {
    let explicit = chat_request_body(&["--model-protocol", "chat-completions"]);
    let implicit = chat_request_body(&[]);
    // ⚠ 比之前要**先把钟抹平**：状态行里那个 `time = <RFC3339>` 读的是**墙上时钟**，
    // 两次真入口跑只要跨过一秒就会差一个字（本机实测撞到过 `11:48:59Z` vs `11:49:00Z`，
    // 于是这条判据随机变红——它与"线协议选了哪条路"毫无关系）。
    // 抹平只动这一个字段，其余仍逐字节比；再加一条"两边的钟各出现一次"防止顺手抹掉别的东西。
    assert_eq!(
        explicit.matches("time = ").count(),
        implicit.matches("time = ").count(),
        "抹平前先确认结构没变（状态行数量一致）"
    );
    assert_eq!(
        without_clock(&explicit),
        without_clock(&implicit),
        "缺省与显式 chat-completions 必须逐字节一致（零迁移，钟除外）"
    );
    assert!(
        explicit.contains("\"messages\"") && explicit.contains("\"stream\":false"),
        "走的是 chat 那条路径（messages + 嵌套 tools）: {}",
        redacted(&explicit)
    );
}

/// 把状态行里的**钟**抹成一个固定记号（只在上面那条判据里用）。
///
/// 取值从 `time = ` 之后一直取到 JSON 转义的换行（`\n` 是**两个字符**）为止——状态行是
/// `time = …\nturn = …\n…` 的形态，所以"这一行"的右边界就是那个反斜杠。
fn without_clock(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(at) = rest.find("time = ") {
        let (before, after) = rest.split_at(at + "time = ".len());
        out.push_str(before);
        out.push_str("<clock>");
        let end = after.find('\\').unwrap_or(after.len());
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// 跑一次**真入口**（`agent_run`）并把第一份请求体原样取回来。
///
/// 两种线协议各一次，用来判"缺省没变"；凭据与端点都走换家档（回环夹具）。
fn chat_request_body(extra: &[&str]) -> String {
    let temp = TempDir::new("responses-default");
    let env_root = temp.env("agent-default");
    let materials = temp.materials();
    // chat 侧的夹具应答：一条结束帧。
    let (endpoint, requests) = spawn_wire_server(vec![(
        200,
        "{\"choices\":[{\"message\":{\"content\":null,\"tool_calls\":[{\"id\":\"c1\",\"type\":\"function\",\"function\":{\"name\":\"finish_run\",\"arguments\":\"{}\"}}]},\"finish_reason\":\"tool_calls\"}]}".to_string(),
    )]);

    let mut arguments: Vec<String> = vec![
        "run".to_string(),
        "--root".to_string(),
        path_text(&env_root),
        "--agent".to_string(),
        "agent-default".to_string(),
        "--materials".to_string(),
        path_text(&materials),
        "--run".to_string(),
        "run-1".to_string(),
        "--max-turns".to_string(),
        "1".to_string(),
        // 换家档的五件套：端点 / 模型名 / 标称窗口（+ 凭据走环境变量）——协议缺省即可。
        "--model-endpoint".to_string(),
        endpoint,
        "--model".to_string(),
        "fixture-model".to_string(),
        "--context-window".to_string(),
        "1000".to_string(),
    ];
    arguments.extend(extra.iter().map(|each| (*each).to_string()));
    let out = run_entry(&arguments.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let raw = all_requests(&requests)[0].clone();
    raw.split("\r\n\r\n").nth(1).expect("有请求体").to_string()
}

// ---------------------------------------------------------------------------
// 回环夹具：按脚本应答，然后继续盯着（零自动重试的证据）
// ---------------------------------------------------------------------------

fn spawn_responses_server(
    replies: Vec<(u16, String)>,
) -> (String, mpsc::Receiver<Vec<String>>, mpsc::Receiver<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("可以绑定回环端口");
    let port = listener.local_addr().expect("有地址").port();
    let (body_sender, body_receiver) = mpsc::channel();
    let (extra_sender, extra_receiver) = mpsc::channel();

    std::thread::spawn(move || {
        let mut raws = Vec::new();
        for (status, body) in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            raws.push(read_raw_request(&mut stream));
            let reason = if status == 200 {
                "OK"
            } else {
                "Internal Server Error"
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.flush();
        }
        let _ = body_sender.send(raws);

        listener.set_nonblocking(true).expect("设成非阻塞");
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        let mut extra = 0usize;
        while std::time::Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    extra += 1;
                    let _ = read_raw_request(&mut stream);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(_) => break,
            }
        }
        let _ = extra_sender.send(extra);
    });

    (
        format!("http://127.0.0.1:{port}/responses"),
        body_receiver,
        extra_receiver,
    )
}

/// chat 侧的回环服务（只为"缺省没变"那一对拍用）。
fn spawn_wire_server(replies: Vec<(u16, String)>) -> (String, mpsc::Receiver<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("可以绑定回环端口");
    let port = listener.local_addr().expect("有地址").port();
    let (body_sender, body_receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut raws = Vec::new();
        for (status, body) in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            raws.push(read_raw_request(&mut stream));
            let _ = write!(
                stream,
                "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.flush();
        }
        let _ = body_sender.send(raws);
    });
    (
        format!("http://127.0.0.1:{port}/chat/completions"),
        body_receiver,
    )
}

fn read_raw_request(stream: &mut std::net::TcpStream) -> String {
    let mut reader = BufReader::new(stream.try_clone().expect("克隆流"));
    let mut raw = String::new();
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        raw.push_str(&line);
        if line.to_ascii_lowercase().starts_with("content-length:") {
            length = line
                .split(':')
                .nth(1)
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0);
        }
        if line == "\r\n" {
            break;
        }
    }
    let mut buffer = vec![0u8; length];
    if reader.read_exact(&mut buffer).is_ok() {
        raw.push_str(&String::from_utf8_lossy(&buffer));
    }
    raw
}

/// 取回这一次会话收到的**全部**请求原文。
///
/// ⚠ 一个连接里只能取一次：`Receiver` 收到的是一条消息（装着所有请求的那个 `Vec`），
/// 取走之后通道就空了（第二次再取会得到 `Disconnected`，看起来像"服务端没答复"）。
fn all_requests(receiver: &mpsc::Receiver<Vec<String>>) -> Vec<String> {
    receiver
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("应当收到请求")
}

/// 断言失败信息里打印请求时先把凭据遮掉：测试输出不该成为泄漏渠道。
fn redacted(raw: &str) -> String {
    raw.replace(SENTINEL_KEY, "<redacted>")
}

/// 请求体 → JSON。
fn body_of(raw: &str) -> Value {
    let body = raw.split("\r\n\r\n").nth(1).expect("有请求体");
    serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("请求体应当是 JSON（{error}）: {body}"))
}

// ---------------------------------------------------------------------------
// 应答脚本
// ---------------------------------------------------------------------------

/// 一条 responses 应答。
fn responses_body(output: Value, status: &str, usage: Option<(u64, u64)>) -> String {
    let mut body = json!({
        "id": "resp_1",
        "object": "response",
        "status": status,
        "output": output,
    });
    if let Some((input, output)) = usage {
        body["usage"] = json!({"input_tokens": input, "output_tokens": output});
    }
    body.to_string()
}

/// 一次工具调用的应答（可带一条推理条目在它前面）。
fn call_body(call_id: &str, name: &str, arguments: &str, reasoning: Option<Value>) -> String {
    let mut items = Vec::new();
    if let Some(item) = reasoning {
        items.push(item);
    }
    items.push(json!({
        "type": "function_call",
        "call_id": call_id,
        "name": name,
        "arguments": arguments,
    }));
    responses_body(Value::Array(items), "completed", None)
}

/// 一个结束帧（**不带参数**——2026-10-10 裁决：结束了就是结束了）。
fn finish_body() -> String {
    responses_body(
        json!([{
            "type": "function_call",
            "call_id": "call-finish",
            "name": "finish_run",
            "arguments": "{}",
        }]),
        "completed",
        None,
    )
}

// ---------------------------------------------------------------------------
// 夹具与进程
// ---------------------------------------------------------------------------

/// 夹具：一次运行的全部参数（模型走**真实适配层**指向回环端点）。
struct Fixture {
    variables: Vec<(&'static str, String)>,
}

impl Fixture {
    fn new(root: &Path, agent: &str, materials: &Path, run: &str, endpoint: &str) -> Self {
        Self {
            variables: vec![
                ("MANUAL_FIXTURE_ROOT", path_text(root)),
                ("MANUAL_FIXTURE_AGENT", agent.to_string()),
                ("MANUAL_FIXTURE_MATERIALS", path_text(materials)),
                ("MANUAL_FIXTURE_RUN", run.to_string()),
                ("MANUAL_FIXTURE_MAX_TURNS", "4".to_string()),
                ("MANUAL_FIXTURE_MODEL", "loopback-responses".to_string()),
                ("MANUAL_FIXTURE_ENDPOINT", endpoint.to_string()),
                ("MANUAL_FIXTURE_CREDENTIAL", SENTINEL_KEY.to_string()),
                ("MANUAL_FIXTURE_MODEL_NAME", "fixture-model".to_string()),
            ],
        }
    }

    fn input(mut self, text: &str) -> Self {
        self.variables
            .push(("MANUAL_FIXTURE_INPUT", text.to_string()));
        self
    }

    fn window(mut self, window: u32) -> Self {
        self.variables
            .push(("MANUAL_FIXTURE_CONTEXT_WINDOW", window.to_string()));
        self
    }

    fn host_lines(mut self, lines: &str) -> Self {
        self.variables
            .push(("MANUAL_FIXTURE_HOST_LINES", lines.to_string()));
        self
    }

    fn run(self) -> Output {
        let mut command = Command::new(binary("manual_entry_fixture"));
        for (name, value) in &self.variables {
            command.env(name, value);
        }
        command
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost")
            .env_remove("HTTP_PROXY")
            .env_remove("HTTPS_PROXY")
            .env_remove("CELIRA_IM_BACKEND_AUTH")
            .output()
            .expect("夹具跑得起来")
    }
}

/// 跑**真入口**（`agent_run`）：凭据与端点都从命令行与环境给，不碰主人的家。
fn run_entry(arguments: &[&str]) -> Output {
    Command::new(binary("agent_run"))
        .args(arguments)
        .env("CELIRA_MODEL_API_KEY", SENTINEL_KEY)
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("CELIRA_IM_BACKEND_AUTH")
        .output()
        .expect("入口跑得起来")
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn field(text: &str, key: &str) -> String {
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
        .unwrap_or_default()
        .to_string()
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        // 目录名**必须带进程号**：测试二进制是一份一个进程，而计数器只在进程内唯一 ⇒
        // 不带进程号的两份并行（或两个写者同时跑）会算出同一批路径、**互相删目录**。
        let mut path = std::env::temp_dir();
        path.push(format!("celira-task44-{}-{label}-{n}", std::process::id()));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("清掉同名残留目录");
        }
        std::fs::create_dir_all(&path).expect("建立测试目录");
        Self { path }
    }

    fn env(&self, agent: &str) -> PathBuf {
        let env_root = self.path.join("env");
        if !env_root.exists() {
            drop(
                celira_agent::store::Environment::create(
                    &env_root,
                    celira_agent::foundation::AgentId::new(agent),
                )
                .expect("建环境"),
            );
        }
        env_root
    }

    fn materials(&self) -> PathBuf {
        let materials = self.path.join("materials");
        std::fs::create_dir_all(&materials).expect("建资料目录");
        materials
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
