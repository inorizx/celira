//! **可重算性门禁**：一份 View 交给**真实适配层**渲染两次，两次请求必须**逐字节相同**。
//!
//! 它守的是那条底线本身——"出问题了能查出来"的可测形式：**请求是 View 的纯函数**。
//! 同一份 View、同一份当次声明的静态头，渲染多少次、经几个独立的适配层实例，出去的字节
//! 必须一模一样；任何隐藏状态（钟、随机、进程级缓存、上一次请求的残留）都会当场让它红。
//!
//! ## ⚠ 旧规矩是什么、新规矩是什么（2026-09-26）
//!
//! **旧规矩**：这一份文件原来的形态是"**仅凭环境目录，能不能重建她当时看见的请求**"——
//! 真跑一次活动（进程 A）→ 改掉环境根里那一份静态头 → **另一个进程**从**已确认记录**水化
//! 一个 Run（`Run::resume`）→ 重建那一封请求 → 与真发出去的**逐字节比**；解释不了的差异必须
//! 登记在 `KNOWN_UNLOGGED` 里。它守的是"**记录足以解释请求**"。
//!
//! **那条路已退役**：承载活动的进程没了就是两个 Run——那一次连续
//! 就断了；新进程起来是**新的一段**，不是"顶着同一个 RunId 继续"。水化入口（`Run::resume`）
//! 随之删除，于是"从记录重建一个 Run"这件事**没有载体了**。
//!
//! **新规矩**：**不变量换到新的一段能承载的那一半上**——同一条"两次渲染必须逐字节相同"，
//! 但 View 由**正常驱动**长出来（`Run::begin_with_input` → `next_turn` → `accept_response`）。
//! ⇒ 记录侧那一半（"记录足以解释请求"）**随路退役**：它当时的消费者正是恢复那条路，而
//! 记录本身**一个字节都没变**（每一次装订照旧留证，见 `store::static_head_at`）。
//!
//! 跑：`cargo test --offline --test request_recomputability`

use std::io::{BufRead, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::sync::mpsc;

use celira_agent::core::{
    Action, ActionId, Agent, Capabilities, Decision, Limits, ModelOutput, ModelPort, ProtocolData,
    Run, RunInput,
};
use celira_agent::foundation::{AgentId, RunId};
use celira_agent::provider_adapter::DeepSeekModel;

const SENTINEL_KEY: &str = "sk-recomputability-sentinel";

/// 环境根里那一份静态头：两次渲染都声明**同一份**。
const HEAD: &str = "HEAD-8c41";

/// 这次活动的开场输入：它进请求，而且是**同一份 View** 的一部分。
const INPUT: &str = "开场：把这一份请求原样渲染两次。";

#[test]
fn the_same_view_renders_the_same_request_byte_for_byte() {
    // ── 一份**正常驱动**长出来的 View：开场输入 + 一条带续接载荷的模型响应。
    let mut agent = Agent::new(AgentId::new("agent-recompute"));
    let mut run = Run::begin_with_input(
        &mut agent,
        RunId::new("run-recompute"),
        Limits::new(4),
        RunInput::text(INPUT).expect("在上限内"),
    )
    .expect("开始活动");
    let turn = run.next_turn().expect("可以开始第一轮");
    // 续接载荷只能由适配层自己产出（`neutral_output` 那一族用例守的就是它），这里手工拼一份
    // 合法的：本用例判的是**渲染**，不是翻译。
    let protocol = ProtocolData::new("chat-completions", 1, "{\"call_id\":\"c-1\"}".to_string())
        .expect("载荷合法");
    let output = ModelOutput::new(
        "我先看一眼。",
        Some(protocol),
        Decision::Act(vec![Action::new(ActionId::new("files_list"))]),
    );
    run.accept_response(turn, output).expect("把响应记进轨迹");

    let view = run.view(Capabilities::none());

    // ── 两次渲染：**两个独立的适配层实例**、两个独立的回环服务（没有共享任何东西）。
    let first = rendered_request(&view, Some(HEAD));
    let second = rendered_request(&view, Some(HEAD));

    let first_body = body_of(&first);
    let second_body = body_of(&second);
    assert_eq!(
        first_body, second_body,
        "同一份 View 两次请求必须逐字节相同——差异意味着请求里混进了 View 之外的东西"
    );

    // ── 正对照：比较**不是空转**。换掉当次声明的那一份静态头，出去的字节必须真的变。
    let other_head = rendered_request(&view, Some("HEAD-另一个"));
    assert_ne!(
        body_of(&other_head),
        first_body,
        "换了静态头而请求一字不变 ⇒ 上面的'逐字节相同'是空转（那份头根本没进请求）"
    );
    assert!(
        first_body.contains(HEAD),
        "当次声明的那一份静态头必须真的在请求里：{first_body:.200}"
    );

    // ── 而这份 View 的**历史**也真的在请求里：开场输入与那条模型响应都要看得见。
    assert!(
        first_body.contains(INPUT),
        "开场输入必须进请求（它是 View 的一部分）"
    );
    assert!(
        first_body.contains("我先看一眼。"),
        "轨迹里那条模型响应必须进请求（重建请求靠的就是它）"
    );
}

// ---------------------------------------------------------------------------
// 回环：真发一次请求，取回**原始 HTTP 请求文本**
// ---------------------------------------------------------------------------

/// 把一份 View 交给**真实适配层**渲染成一次请求并真的发出去，返回原始 HTTP 请求文本。
fn rendered_request(view: &celira_agent::core::View, head: Option<&str>) -> String {
    let (endpoint, requests, _extra) = spawn_wire_server(vec![(200, text_body("重建"))]);
    let mut model =
        DeepSeekModel::at_endpoint(SENTINEL_KEY, "fixture-model", endpoint).expect("装配适配层");
    model.declare_static_head(head);
    model.complete(view).expect("这份历史必须能建成请求");
    first_request(&requests)
}

fn text_body(content: &str) -> String {
    format!(
        "{{\"choices\":[{{\"message\":{{\"content\":{}}},\"finish_reason\":\"stop\"}}]}}",
        serde_json::to_string(content).expect("可以序列化")
    )
}

fn body_of(raw: &str) -> &str {
    raw.split_once("\r\n\r\n")
        .map(|(_head, body)| body)
        .expect("请求含空行分隔的头与 body")
}

fn spawn_wire_server(
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
            let _ = write!(
                stream,
                "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.flush();
        }
        let _ = body_sender.send(raws);
        let _ = extra_sender.send(0usize);
    });

    (
        format!("http://127.0.0.1:{port}/chat/completions"),
        body_receiver,
        extra_receiver,
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

fn first_request(receiver: &mpsc::Receiver<Vec<String>>) -> String {
    receiver
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("应当收到请求")
        .into_iter()
        .next()
        .expect("至少一条请求")
}
