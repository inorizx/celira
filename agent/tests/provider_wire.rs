//! TASK_07 验收：**Provider Adapter 的翻译边界与模型请求收口**。
//!
//! 这一份文件用**真实适配层 + 回环 HTTP 服务**跑一次真正的请求：Core 给 View，Adapter 做一次
//! 外围翻译，得到一次中立响应。因此它能证明的，比"模型替身被调用了"强一层：
//!
//! ```text
//! View → Adapter → 一次 HTTP 请求 → wire 响应 → ModelOutput / ModelError → Core 继续判断
//! ```
//!
//! 失败那条路的分类**经事实系统一路可达**：终端一行 `model-failure=<分类>[:<细分>]`、
//! 盘上 `run-ended.failure` 同一个串、诊断命令读回同一个串（下面各格逐字节断言）。
//! 但"能到终端"不等于"泄漏细节"：状态码、响应体、供应商字段名一律不出门。
//!
//! 明确**不**声称的：
//!
//! - 回环只证明 Adapter 的协议路径，**不**证明公网服务可用，也**不**证明模型智能效果；
//! - 文本围栏只证明格式约定（`<<<DATA` 确实存在），**不**是提示注入的安全证明；
//! - 细分档只判**外部形状**（停在哪一步、什么状态码），**不**判根因：没有跨环境稳定判据的档
//!   （例如 TLS 握手）刻意不发布，那类失败会落到 `io` 或干脆不带细分。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::mpsc;

use celira_agent::core::{
    Action, ActionId, ActionOutcome, Agent, Capabilities, ContentBlock, Decision, Limits,
    ModelOutput, ModelPort, ProtocolData, Run, RunInput, TraceEntry, View,
};
use celira_agent::foundation::{AgentId, RunId, SubjectEnding};
use celira_agent::provider_adapter::DeepSeekModel;
mod common;
use common::binary;

/// 哨兵凭据：只用于回环认证，任何输出里出现它就说明越界了。
const SENTINEL_KEY: &str = "fixture-sentinel-credential";

/// 结尾是不是 `±hh:mm`（用例侧**独立**拼法，不调用被测的那个格式化点）。
///
/// 为什么需要它：状态行的 `time` 那一格现在带**本机偏移**，而偏移由**跑用例的机器**决定
/// ⇒ 判死一个具体偏移就会变成"只有 +08:00 的机器才绿"。偏移本身的逐字节判据在
/// `tests/status_bar.rs`（那里把偏移钉死了）。
fn has_offset_suffix(time: &str) -> bool {
    let bytes = time.as_bytes();
    if bytes.len() < 6 {
        return false;
    }
    let tail = &bytes[bytes.len() - 6..];
    (tail[0] == b'+' || tail[0] == b'-')
        && tail[1].is_ascii_digit()
        && tail[2].is_ascii_digit()
        && tail[3] == b':'
        && tail[4].is_ascii_digit()
        && tail[5].is_ascii_digit()
}

// ---------------------------------------------------------------------------
// 1. 一次调用 = 一次请求；请求由当前 View 重建
// ---------------------------------------------------------------------------

#[test]
fn one_complete_is_exactly_one_request_rebuilt_from_the_current_view() {
    let temp = TempDir::new("wire-one-request");
    let env_root = temp.env("agent-wire");
    let materials = temp.materials();
    let input = "这次活动的输入正文";

    let (endpoint, requests, extra) = spawn_wire_server(vec![(200, finish_body())]);
    let run = Fixture::new(&env_root, "agent-wire", &materials, "run-1", &endpoint)
        .input(input)
        .run();
    assert_eq!(code(&run), 0, "{}", stderr(&run));
    assert_eq!(field(&stdout(&run), "ending"), "subject:ended");

    let raw = first_request(&requests);
    // 认证发生了，但凭据不出现在任何 Agent 可见的地方。
    assert!(
        raw.to_ascii_lowercase()
            .contains(&format!("authorization: bearer {SENTINEL_KEY}").to_ascii_lowercase()),
        "请求必须带认证: {}",
        redacted(&raw)
    );
    assert!(!stdout(&run).contains(SENTINEL_KEY));
    assert!(!stderr(&run).contains(SENTINEL_KEY));

    // **自我介绍**（2026-10-06，接 opencode go 那一刀）：客户端必须**用自己的名字**自报身份。
    // 理由不是礼貌：那家按客户端身份做流量治理，而**实测**（本刀证伪时看到的请求原文）
    // 不设它的时候 reqwest 这一版**根本不发 `User-Agent`** —— 一个不自报身份的客户端正是
    // 那个监控要盯的形状。
    // ⚠ 断言的是**产品名**，不是完整取值：`CLIENT_USER_AGENT` 是 `pub(crate)` 共用件
    // （登记在 `minimal_agent.rs` 的 `ADAPTER_CRATE_SHARED`），集成测试看不见它 ——
    // 而绑死版本号会让每次发版都红一次，那不是这一条要守的东西。
    let lowered = raw.to_ascii_lowercase();
    assert!(
        lowered.contains("user-agent: celira-agent/"),
        "请求必须自报家门（`User-Agent`）: {}",
        redacted(&raw)
    );
    // 负对照：不许退回**库名**当身份（今天 reqwest 的默认是"根本不发"，这一条防的是
    // 它将来改成发库名 —— 两种都不是我们的身份）。
    assert!(
        !lowered.contains("user-agent: reqwest/"),
        "不许拿 HTTP 库的名字当我们的身份: {}",
        redacted(&raw)
    );

    // **那一段对话是谁**（2026-10-09）：opencode go 的 zen 网关**要求**这个头 —— 实测不带它
    // `POST …/chat/completions` 一律 **400**（响应体是空的），带上就是 200；而它是"should"
    // 还是"must"这一格，2026-10-06 的裁决是"**真被拒再加**"，今天真的被拒了。
    // 取值必须是**这一段活动**（`View::run`）：整段清醒期里逐字节相同，别家供应商忽略未知头。
    assert!(
        lowered.contains("x-opencode-session: run-1"),
        "请求必须带上'这一段对话是谁'那一格（值＝这一段活动的 id）: {}",
        redacted(&raw)
    );
    // 负对照：那一格**不是**空值（空值 = 网关照旧 400），也不是别的活动的 id。
    assert!(
        !lowered.contains("x-opencode-session: \r") && !lowered.contains("x-opencode-session: \n"),
        "那一格不许是空的: {}",
        redacted(&raw)
    );

    // **思考强度**（2026-10-08）：**没人声明 ⇒ 请求体里不许出现那个键**。
    //
    // 这是那一刀的**负向判据**，也是"不动的东西不许悄悄变"那条纪律的落点：我们动的是**协议面**
    // （请求体），而默认档（没人在清单里写 `reasoning_effort`）必须与从前**逐字节相同** ——
    // 谁把 `skip_serializing_if` 去掉、或者给它补一个默认值，这里当场红。
    // ⚠ 正向那一半（声明了值 ⇒ 只多这一个键、取值逐字节等于给的）在下一刀补：那需要一条
    // **带 `with_reasoning_effort` 的装配路径**，本文件现在的脚手架还没有那条路。
    assert!(
        !lowered.contains("reasoning_effort"),
        "没人声明思考强度 ⇒ 请求体里不许出现那个键: {}",
        redacted(&raw)
    );

    // 消息顺序：system 契约在最前，输入作为**非 system** 正文恰好一次，运行事实在最后。
    let roles = roles_of(&raw);
    assert_eq!(
        roles.first().map(String::as_str),
        Some("system"),
        "{roles:?}"
    );
    assert!(
        roles.iter().filter(|role| *role == "user").count() >= 2,
        "{roles:?}"
    );
    assert_eq!(
        raw.matches(input).count(),
        1,
        "初始输入必须在一次请求里逻辑上只出现一次"
    );
    let system_part = raw.split("\"role\":\"user\"").next().expect("有 system 段");
    assert!(!system_part.contains(input), "初始输入不得进入 system 契约");
    // 结构事实：这次活动、第几轮、轮数上限与余量都是机械读数。
    assert!(raw.contains("turn = 1"), "{}", redacted(&raw));
    // TASK_18 期望更新：这里曾是 `entries = 1`（首轮视野里只有那一条手动来源事实，因为 Turn 级
    // 注入返回空批次、不产生记录）。状态栏装配之后，每一轮都**恒摆**一条状态行 ⇒ 首轮视野里是
    // 两条：Run 级那条（内含来源事实 + 开场读数两块）与 Turn 级那条（本轮的读数）。
    // 数字变了，判据没变：`entries` 仍然是"轨迹里真实有几条"，不是推算出来的。
    assert!(
        raw.contains("entries = 2"),
        "首轮视野里应当是那条手动来源事实加一条恒摆的状态行: {}",
        redacted(&raw)
    );
    // 头部**恒定两件**（TASK_68）：一切动作经入口路由，能力清单不再直连 `tools` 声明。
    // 能力怎么被主体看见，走的是**注入的目录**（TASK_68 P2）；这里钉的是头部这一面。
    assert!(
        raw.contains("\"name\":\"invoke\""),
        "入口必须在头部: {}",
        redacted(&raw)
    );
    assert!(
        raw.contains("\"name\":\"finish_run\""),
        "结束类工具永远在: {}",
        redacted(&raw)
    );
    assert!(
        !system_part.contains("open_app"),
        "能力只出现在 tools 里，不再写进 system 契约: {}",
        redacted(&raw)
    );

    // **一次 `complete` 只发一次请求**：服务端在整个运行期间没有再收到第二条连接。
    assert_eq!(
        extra
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or(usize::MAX),
        0
    );
}

// ---------------------------------------------------------------------------
// 1c. 思考强度：清单里那一格**真的走到请求里**，也真的进账、真的进她那一眼（2026-10-08）
// ---------------------------------------------------------------------------
//
// 这一节走的是**配模型那条真路**（机器级清单 ＋ Agent 级绑定，没有任何进程侧旗标）：因此它同时
// 量到三件事——① 那一格**真的发出去**（wire 上只多这一个键，取值逐字节等于清单里那一个）；
// ② **进她那一眼**（开场那一行在请求体里，因为她看到的就是它）；③ **进账**（同一条注入落在
// `trace.ndjson` 里 ⇒ "哪一段是哪一档想的"查得出来）。
//
// 为什么必须走这条路而不是夹具直接声明：夹具的声明是**测试自己编的**，它证明不了"清单里写的
// 那一格真的生效"——而"页面上填了 high、请求里根本没有那个键"正是这一刀要堵的形状。

/// 清单里那一条声明的档位（取值在模型配置门的封闭表里）。
const DECLARED_EFFORT: &str = "high";

/// 写一份**机器级清单**（一条）与**Agent 级绑定**（指向它）——"配模型"那条真路的两个文件。
///
/// `effort` / `protocol` 给 `None` 就是那一格**不写**（缺省形状：不发那个键 / 本家那一条协议）。
fn write_list_and_binding(
    home: &Path,
    env_root: &Path,
    endpoint: &str,
    effort: Option<&str>,
    protocol: Option<&str>,
) -> PathBuf {
    std::fs::create_dir_all(home).expect("建家目录");
    let effort_line = match effort {
        Some(value) => format!(",\n    \"reasoning_effort\": \"{value}\""),
        None => String::new(),
    };
    let protocol_line = match protocol {
        Some(value) => format!(",\n    \"protocol\": \"{value}\""),
        None => String::new(),
    };
    let list = home.join("model_sources.json");
    std::fs::write(
        &list,
        format!(
            "[\n  {{\n    \"id\": \"ms-1\",\n    \"name\": \"回环\",\n    \
             \"endpoint\": \"{endpoint}\",\n    \"api_key\": \"{SENTINEL_KEY}\",\n    \
             \"model\": \"fixture-model\",\n    \"context_window\": 4096{protocol_line}{effort_line}\n  }}\n]\n"
        ),
    )
    .expect("写清单");
    std::fs::write(
        env_root.join("model_binding.json"),
        "{ \"source\": \"ms-1\" }\n",
    )
    .expect("写绑定");
    list
}

/// 跑**真入口**（`agent_run`）：模型来源是清单 ＋ 绑定，因此**一个进程侧旗标都不给**。
fn run_entry_from_list(env_root: &Path, materials: &Path, list: &Path, run: &str) -> Output {
    Command::new(binary("agent_run"))
        .args([
            "run",
            "--root",
            &path_text(env_root),
            "--agent",
            "agent-wire",
            "--materials",
            &path_text(materials),
            "--run",
            run,
            "--max-turns",
            "4",
            "--model-sources",
            &path_text(list),
        ])
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        // 清单那一条自带钥匙：进程侧那两条凭据变量**不参与**这次装配（也不该在场）。
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("CELIRA_MODEL_API_KEY")
        .env_remove("CELIRA_IM_BACKEND_AUTH")
        .output()
        .expect("入口跑得起来")
}

#[test]
fn a_declared_reasoning_effort_reaches_the_request_the_account_and_her_view() {
    let temp = TempDir::new("effort-declared");
    let env_root = temp.env("agent-wire");
    let materials = temp.materials();
    let home = temp.path.join("home");

    let (endpoint, requests, _extra) = spawn_wire_server(vec![(200, finish_body())]);
    let list = write_list_and_binding(&home, &env_root, &endpoint, Some(DECLARED_EFFORT), None);
    let run = run_entry_from_list(&env_root, &materials, &list, "run-effort");
    assert_eq!(code(&run), 0, "{}", stderr(&run));

    let raw = first_request(&requests);
    // ① **真的发出去**：请求体里恰好一个这个键，取值**逐字节**等于清单里那一个。
    let wire = wire_of(&raw);
    assert_eq!(
        wire.get("reasoning_effort")
            .and_then(|value| value.as_str()),
        Some(DECLARED_EFFORT),
        "清单里那一格必须原样进请求: {}",
        redacted(&raw)
    );
    assert_eq!(
        raw.matches("\"reasoning_effort\":").count(),
        1,
        "只多这一个键，不许重复: {}",
        redacted(&raw)
    );

    // ② **进她那一眼**：开场那一行就在请求体里（她看到的就是它）——两格都在，逐字节。
    let tail = format!("model = fixture-model · reasoning_effort = {DECLARED_EFFORT}");
    assert!(
        raw.contains(&tail),
        "她读到的那一行必须带上档位: {}",
        redacted(&raw)
    );

    // ③ **进账**：同一条注入落在盘上（不是只在内存里）。
    let on_disk = std::fs::read_to_string(env_root.join("trace.ndjson")).expect("读经历");
    assert!(
        on_disk.contains(&tail),
        "那一行必须随 run-start 经历落在盘上: {on_disk}"
    );
}

/// **负向判据（文件那一档）**：清单里没写这一格 ⇒ 请求体里**不许出现那个键**，
/// 她那一行也**不多一格**（"没声明"与"读不出来"是两件事）。
#[test]
fn a_list_without_the_field_sends_no_key_and_shows_no_field() {
    let temp = TempDir::new("effort-absent");
    let env_root = temp.env("agent-wire");
    let materials = temp.materials();
    let home = temp.path.join("home");

    let (endpoint, requests, _extra) = spawn_wire_server(vec![(200, finish_body())]);
    let list = write_list_and_binding(&home, &env_root, &endpoint, None, None);
    let run = run_entry_from_list(&env_root, &materials, &list, "run-no-effort");
    assert_eq!(code(&run), 0, "{}", stderr(&run));

    let raw = first_request(&requests);
    assert!(
        !raw.to_ascii_lowercase().contains("reasoning_effort"),
        "没人声明 ⇒ 请求体里不许出现那个键: {}",
        redacted(&raw)
    );
    assert!(
        raw.contains("model = fixture-model"),
        "模型那一格照旧在: {}",
        redacted(&raw)
    );
}

/// **配对错误**：清单里声明了档位、线协议却选了那一条**没有这个键**的 ⇒ **明确拒绝**
/// （退出码 5），错误里点名是哪一件、给出两条出路——**不静默丢掉它**。
///
/// 静默丢掉的形状是"操作者以为她在按这一档想，而请求里根本没有这个键"。
#[test]
fn a_declared_effort_on_the_responses_wire_is_refused_with_a_way_out() {
    let temp = TempDir::new("effort-pairing");
    let env_root = temp.env("agent-wire");
    let materials = temp.materials();
    let home = temp.path.join("home");

    // 端点不必真的在：拒绝排在装配与任何网络调用**之前**。
    let list = write_list_and_binding(
        &home,
        &env_root,
        "https://example.invalid/v1/responses",
        Some(DECLARED_EFFORT),
        Some("responses"),
    );
    let run = run_entry_from_list(&env_root, &materials, &list, "run-pairing");
    assert_eq!(code(&run), 5, "{}", stderr(&run));
    let message = stderr(&run);
    assert!(message.contains("reasoning_effort"), "{message}");
    assert!(
        message.contains(DECLARED_EFFORT),
        "要点名是哪个取值: {message}"
    );
    assert!(message.contains("responses"), "{message}");
    assert!(
        message.contains("chat-completions"),
        "要给出出路（换回本家那一条协议）: {message}"
    );
}

// ---------------------------------------------------------------------------
// 1b. TASK_18：状态栏的行真的进了请求，而且与机械运行事实两处渲染并存
// ---------------------------------------------------------------------------

/// 状态栏那一条读数的**围栏开始**记号（与适配层同一份约定：只证明格式，不宣称防注入）。
const DATA_OPEN_MARK: &str = "<<<DATA";
/// 状态栏那一条读数的**围栏结束**记号。
const DATA_CLOSE_MARK: &str = ">>>";

#[test]
fn the_status_bar_reaches_the_real_request_beside_the_mechanical_facts() {
    let temp = TempDir::new("wire-status-bar");
    let env_root = temp.env("agent-wire-status");
    let materials = temp.materials();

    let (endpoint, requests, _extra) = spawn_wire_server(vec![(200, finish_body())]);
    let run = Fixture::new(
        &env_root,
        "agent-wire-status",
        &materials,
        "run-status",
        &endpoint,
    )
    .input("状态栏验收")
    // 固定钟：这一次请求因此可以逐字节断言（不给就是用真实钟的生产路径）。
    .clock(0)
    .run();
    assert_eq!(code(&run), 0, "{}", stderr(&run));

    let raw = first_request(&requests);
    let system_part = raw.split("\"role\":\"user\"").next().expect("有 system 段");

    // **判据 2：非 system、带围栏**。状态行是环境内容，因此留在非 system 正文里，围在数据围栏之间。
    assert!(
        !system_part.contains("status-bar"),
        "状态行不得写进 system 契约: {}",
        redacted(&raw)
    );
    assert!(
        raw.contains("status-bar:"),
        "状态行必须真的发出去: {}",
        redacted(&raw)
    );
    let segments: Vec<&str> = raw.split("status-bar:").skip(1).collect();
    assert_eq!(
        segments.len(),
        2,
        "开场一条 + 本轮一条，而且没有第三条: {}",
        redacted(&raw)
    );
    let mut readings = Vec::new();
    for segment in segments {
        assert!(
            segment.contains(DATA_OPEN_MARK),
            "状态行必须落在数据围栏里: {}",
            redacted(&raw)
        );
        let inside = segment
            .split(DATA_OPEN_MARK)
            .nth(1)
            .expect("有开始围栏")
            .split(DATA_CLOSE_MARK)
            .next()
            .expect("有结束围栏");
        readings.push(inside.to_string());
    }
    // ⚠ 那一格现在带**本机偏移**（`LocalOffset::of_machine`，跑用例的机器说了算）⇒ 这里判
    // **形状**（同一个瞬间的两种写法之一），不判死偏移——判死它就会变成"只有 +08:00 的机器
    // 才绿"。偏移本身的逐字节判据在 `tests/status_bar.rs`（那里把偏移钉死了）。
    // ⚠ 这一段是 **wire 里的 JSON 转义形态**：换行是**两个字符**（`\` ＋ `n`），不是真换行
    // ⇒ 要用那个转义串来切（`responses_protocol.rs` 里同款）。
    let time = readings[0]
        .split("\\n")
        .find_map(|field| field.strip_prefix("time = "))
        .expect("有 time 那一格");
    assert!(
        time.starts_with("1970-01-01T") && (time.ends_with('Z') || has_offset_suffix(time)),
        "固定钟下的时间是确定的读数: {readings:?}"
    );
    assert!(
        readings[0].contains("turn = 0/4"),
        "开场读的是「一次都还没消耗」: {readings:?}"
    );
    assert!(
        readings[1].contains("turn = 1/4"),
        "轮次读数取的是 Core 给的真实轮次与上限: {readings:?}"
    );
    // TASK_19 新增段：`context` 的分母是**宿主声明的窗口**；这一条没有传窗口，因此取适配层的
    // 公开标称值（缺省路径也有读数，不是"未声明分支"）。占用在这一轮还没有报告 ⇒ unknown。
    assert!(
        readings[0].contains("context = unknown/1000000 token"),
        "缺省窗口取适配层的公开标称值: {readings:?}"
    );
    assert!(
        readings[1].contains("context = unknown/1000000 token"),
        "本轮也还没有报告可读: {readings:?}"
    );

    // 开场那一轮：一条消息里两处内容都在（既有处境事实 + 开场读数，0/上限）。
    assert!(
        raw.contains("activity-origin:") && raw.contains("turn = 0/4"),
        "开场那条消息里应当既有来源事实、也有开场读数: {}",
        redacted(&raw)
    );

    // **判据 5：两处渲染都还在，形状不同、谁也不被静默删掉。**
    // 适配层那条机械运行事实（最后一条消息，三个字段）仍然完整。
    let facts = raw
        .split("ACTIVITY FACTS")
        .nth(1)
        .expect("机械运行事实仍然在请求里")
        .split("\"}")
        .next()
        .expect("事实那一段有结尾");
    assert!(facts.contains("turn = 1"), "{facts}");
    assert!(facts.contains("turn_limit = 4"), "{facts}");
    assert!(facts.contains("further_model_calls_at_most = 3"), "{facts}");
    // 而状态行是**另一个形状**：一个"已用/上限"的比例，且它不在机械事实那一段里。
    assert!(
        !facts.contains("turn = 1/4"),
        "比例式渲染只出现在状态行里，不与机械事实重合成同一段: {facts}"
    );
}

/// 在一条响应体里加上厂商报告的上下文占用（`usage.prompt_tokens`）。
///
/// 这些 body 都是单层 JSON 对象，因此在**最后一个** `}` 之前插入即可（不需要引库去重建 JSON）。
fn with_usage(body: &str, prompt: u64) -> String {
    let cut = body.rfind('}').expect("响应体是 JSON 对象");
    format!(
        "{},\"usage\":{{\"prompt_tokens\":{prompt}}}{}",
        &body[..cut],
        &body[cut..]
    )
}

/// 刀三（B）：适配层报告的三项用量**经收场行落盘** —— 从 wire 响应到 `run-ended` 的端到端一读。
///
/// 这条补的正是"屏幕读数"之外的第二条去向：同一份 wire 响应里的 `usage`，一个消费者是状态行
/// （下面的滞后读数），另一个是运行记录的成本格。**不是**往存储层直接写一行来验形状
/// （`durable_environment.rs` 已覆盖编解码），而是走完整路径：
///
/// ```text
/// 回环响应（带 usage） → 适配层写用量 cell → 收场读 cell → run-ended 事实行
/// ```
///
/// 同时钉住"不含就不写"：第 1 轮响应**不带** total ⇒ 落盘的必然是第 2 轮那次报告；两次刻意取
/// 不同的值，于是"落盘的到底是哪一次"有唯一答案。
#[test]
fn the_usage_a_wire_response_reports_lands_in_the_run_record() {
    let temp = TempDir::new("wire-usage-cost");
    let env_root = temp.env("agent-wire-usage");
    let materials = temp.materials();

    // 第 1 轮：纯正文 + 只报告 prompt（刻意给 1111）；第 2 轮：收场 + 报告三项（3333/444/3777）。
    let (endpoint, _requests, _extra) = spawn_wire_server(vec![
        (200, with_usage(&text_body("等", "stop"), 1_111)),
        (
            200,
            with_three_part_usage(&finish_body(), 3_333, 444, 3_777),
        ),
    ]);
    let run = Fixture::new(
        &env_root,
        "agent-wire-usage",
        &materials,
        "run-usage-cost",
        &endpoint,
    )
    .input("用量入账")
    .clock(0)
    .host_lines("\n")
    .run();
    assert_eq!(code(&run), 0, "{}", stderr(&run));
    assert_eq!(field(&stdout(&run), "ending"), "subject:ended");

    let facts = std::fs::read_to_string(env_root.join("facts.ndjson")).expect("读事实存储");
    let ended = facts
        .lines()
        .find(|line| line.contains("\"fact\":\"run-ended\""))
        .expect("必须有收场行");
    assert!(
        ended.contains("\"prompt-tokens\":3333"),
        "输入计数必须落盘，且是收场前最后一次报告的那个: {ended}"
    );
    assert!(
        ended.contains("\"completion-tokens\":444"),
        "输出计数必须落盘: {ended}"
    );
    assert!(
        ended.contains("\"total-tokens\":3777"),
        "总数（厂商真给了就不替它算）必须落盘: {ended}"
    );
    assert!(
        !ended.contains("1111"),
        "第 1 轮那次报告不得残留（它不是这次收场的读数）: {ended}"
    );
    // 时长由宿主侧量：`clock(0)` 固定钟 ⇒ 它落的是 `0`，是**读数**而不是缺省。
    assert!(
        ended.contains("\"elapsed-ms\":0"),
        "时长必须落盘（固定钟下就是 0，仍是读数）: {ended}"
    );
    // 分量不落盘：口径是"按读数记账"，金额是推出来的，不属于记录。
    assert!(!ended.contains("price"), "金额是推导量，不得入账: {ended}");
}

/// 一次**不带** `usage` 的响应：收场行里那些字段一个都不该出现（缺省 ≠ 零）。
///
/// 负对照与它成对：上面那条证明"有就落"，这条证明"没有就不落"——两条用的是同一个夹具、
/// 同一段路径，只有响应体差一个字段。少了这一条，"恒写三个 0"也能骗过上面那格。
#[test]
fn a_response_without_usage_leaves_no_cost_fields_at_all() {
    let temp = TempDir::new("wire-usage-absent");
    let env_root = temp.env("agent-wire-no-usage");
    let materials = temp.materials();

    let (endpoint, _requests, _extra) = spawn_wire_server(vec![(200, finish_body())]);
    let run = Fixture::new(
        &env_root,
        "agent-wire-no-usage",
        &materials,
        "run-no-usage",
        &endpoint,
    )
    .input("没有用量报告")
    .clock(0)
    .run();
    assert_eq!(code(&run), 0, "{}", stderr(&run));

    let facts = std::fs::read_to_string(env_root.join("facts.ndjson")).expect("读事实存储");
    let ended = facts
        .lines()
        .find(|line| line.contains("\"fact\":\"run-ended\""))
        .expect("必须有收场行");
    for absent in ["prompt-tokens", "completion-tokens", "total-tokens"] {
        assert!(
            !ended.contains(absent),
            "厂商没报告就不该有 {absent}（缺省不是零）: {ended}"
        );
    }
}

/// 同上，但把厂商报告的**三项**都写进去（chat 线协议的 `usage` 形状：prompt / completion / total）。
///
/// 与 [`with_usage`] 分开是因为它们的**语义**不同：那个造的是"只报了输入占用"的旧形状，这个造的是
/// 完整形状——两条判据要能各自单独红，就不能共用一个构造函数。
fn with_three_part_usage(body: &str, prompt: u64, completion: u64, total: u64) -> String {
    let cut = body.rfind('}').expect("响应体是 JSON 对象");
    format!(
        "{},\"usage\":{{\"prompt_tokens\":{prompt},\"completion_tokens\":{completion},\"total_tokens\":{total}}}{}",
        &body[..cut],
        &body[cut..]
    )
}

#[test]
fn a_reported_occupancy_shows_up_in_the_next_request_only() {
    let temp = TempDir::new("wire-reading-lag");
    let env_root = temp.env("agent-wire-reading");
    let materials = temp.materials();

    // 第 1 轮：纯正文（交回控制）+ 报告 4242；第 2 轮：收场 + 报告 9999。
    let (endpoint, requests, _extra) = spawn_wire_server(vec![
        (200, with_usage(&text_body("等", "stop"), 4_242)),
        (200, with_usage(&finish_body(), 9_999)),
    ]);
    let run = Fixture::new(
        &env_root,
        "agent-wire-reading",
        &materials,
        "run-reading-lag",
        &endpoint,
    )
    .input("屏幕验收")
    .clock(0)
    .window(4_096)
    // 一个空行 = 显式继续：驱动第二次（因此发出第二个请求），收场。
    .host_lines("\n")
    .run();
    assert_eq!(code(&run), 0, "{}", stderr(&run));
    assert_eq!(field(&stdout(&run), "ending"), "subject:ended");

    let raws = requests
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("两轮都真的发出过请求");
    assert_eq!(raws.len(), 2, "两轮 = 两个请求");

    // 第 1 个请求：此刻还没有任何报告 ⇒ unknown/窗口（窗口就是宿主声明的那个数）。
    assert!(
        raws[0].contains("context = unknown/4096 token"),
        "首拍读不到厂商报告: {}",
        redacted(&raws[0])
    );
    assert!(
        !raws[0].contains("4242") && !raws[0].contains("9999"),
        "首拍不得凭空出现占用数字: {}",
        redacted(&raws[0])
    );

    // 第 2 个请求：显示的是**第 1 次响应**报告的 4242 —— 滞后一回合，而且不是它自己那次的 9999。
    assert!(
        raws[1].contains("context = 4242/4096 token"),
        "第二拍必须显示上一次响应报告的读数: {}",
        redacted(&raws[1])
    );
    assert!(
        !raws[1].contains("9999"),
        "本轮响应里才产生的读数不得出现在本轮请求里: {}",
        redacted(&raws[1])
    );
}

// ---------------------------------------------------------------------------
// 2. 供应商失败分类与"零自动重试"
// ---------------------------------------------------------------------------

/// **一个名字不能用，是那一次调用的事，不是整段清醒期的事**（2026-10-13 按真机补）。
///
/// 真机抓到的缺陷：模型发出一个非法名 / 保留名 ⇒ 适配层用 `?` 直接判死**整段 Run**
/// （`unsupported-tool-name` / `reserved-tool-name`），而那条路**一个字都不留**——
/// trace 里连一行 `model-response` 都没有，操作者看不到是哪个名字，主体做对的事一起沉掉。
///
/// 现在的判据（与 2026-10-11「参数搬不动不报废这一段」同一条纪律）：它变成**那一次调用**的
/// 拒绝——主体看得见读数、当场能改，名字也进轨迹。这条测试钉三件：
/// ① 这一段**活着**（正常收场，而且真的又问了一次模型）；② 读数**点了那个名字**；
/// ③ 读数**不带那次调用的参数**（形状可以进视野，取值不行）。
#[test]
fn an_unusable_tool_name_is_one_calls_problem_not_the_whole_run() {
    let temp = TempDir::new("wire-bad-name");
    let env_root = temp.env("agent-wire");
    let materials = temp.materials();

    // 第一次：模型发出一个**非法工具名**（参数里带泄漏标记）；第二次：正常收场。
    let (endpoint, _requests, _extra) = spawn_wire_server(vec![
        (
            200,
            tool_call_body("call-1", "do.thing", "{\"note\":\"LeakMarkerExtra\"}"),
        ),
        (200, tool_call_body("call-2", "finish_run", "{}")),
    ]);
    let run = Fixture::new(&env_root, "agent-wire", &materials, "run-1", &endpoint).run();
    let report = stdout(&run);

    assert_eq!(field(&report, "ending"), "subject:ended", "{report}");
    assert_eq!(
        fixture_field(&report, "model-calls"),
        "2",
        "拒绝之后主体还得有机会改：{report}"
    );

    let trace = std::fs::read_to_string(env_root.join("trace.ndjson")).expect("读轨迹");
    // 读数点的是**那个名字**（找拒绝那一行，不是模型响应那一行）。
    let reading = trace
        .lines()
        .find(|line| line.contains("不是一个可用的工具名"))
        .expect("那次拒绝必须留下读数");
    assert!(reading.contains("do.thing"), "{reading}");
    assert!(
        !trace.contains("LeakMarkerExtra"),
        "形状读数不得携带参数取值"
    );
}

/// **`invoke` 里包一个保留名**，同样是那一次调用的事（2026-10-13 按真机补完的那一半）。
///
/// 真机症状：dev 家两支 agent 各因 `reserved-tool-name` **整段清醒期判死**（20 分钟内两次），
/// 而那条路**一个字都不留**（trace 里连一行 `model-response` 都没有）。根因是**同一件事两处
/// 说法不同**：直调那条路早就按"一个名字不对，是那一次调用的事"处理了，而 `invoke` 包裹里
/// **同一个形状**在 `decode_invoke` 里归给了外层的 `Fatal` ⇒ 一次笔误换掉整段。
///
/// 判据（与上一条同一条纪律）：它变成**那一次调用**的拒绝——读数点着**内层那个名字**、
/// 说清该怎么改（收场用 `finish_run`、搁下用 `hold_run`），而这一段**继续活着**。
#[test]
fn a_reserved_name_inside_invoke_is_one_calls_problem_too() {
    let temp = TempDir::new("wire-reserved-inside");
    let env_root = temp.env("agent-wire");
    let materials = temp.materials();

    let (endpoint, _requests, _extra) = spawn_wire_server(vec![
        (
            200,
            tool_call_body("call-1", "invoke", "{\"name\":\"hold_run\",\"args\":{}}"),
        ),
        (200, tool_call_body("call-2", "finish_run", "{}")),
    ]);
    let run = Fixture::new(&env_root, "agent-wire", &materials, "run-1", &endpoint).run();
    let report = stdout(&run);

    assert_eq!(field(&report, "ending"), "subject:ended", "{report}");
    assert_eq!(
        fixture_field(&report, "model-calls"),
        "2",
        "拒绝之后主体还得有机会改：{report}"
    );

    let trace = std::fs::read_to_string(env_root.join("trace.ndjson")).expect("读轨迹");
    let reading = trace
        .lines()
        .find(|line| line.contains("是保留名"))
        .expect("那次拒绝必须留下读数（它从前一个字都不留）");
    assert!(
        reading.contains("hold_run"),
        "读数要点着内层那个名字：{reading}"
    );
}
/// **契约层**的失败：分类如实带出来，而且**不会被重采**（一条请求就够）。
///
/// ⚠ 名字与声称自 TASK_60 裁决 5 起**收窄**了：原来叫"永不重试"，那在**传输层瞬时族**上
/// 已经不成立（那一族现在会被立刻再采一次，见下一条用例）。这一条守的是**另一半**：
/// 这些失败都是"响应到手了但不合契约 / 对方按规矩拒了"——**再采一次不会变**，
/// 所以 `model-calls == 1` 这条判据一个字没改，只是它管的范围从"全部失败"收窄成"契约类失败"。
#[test]
fn contract_failures_keep_their_class_and_are_not_re_sampled() {
    // 每一行是一种失败 + 它**应当带出来的分类**：HTTP 非成功、响应不可解析、生成被截断、
    // 空响应、两个结束调用、非法工具名、参数不是 JSON、参数里多一个字段、结束取值不在封闭表里、
    // 工具调用没有标识（协议载荷缺件）。
    //
    // 第四列写的是**落盘的完整形状**——`<分类>` 或 `<分类>:<细分>`：HTTP 与传输这两类是外部形状，
    // 细分档由适配层折叠出来（500 系 ⇒ `server-side`），因此这里逐字节要求细分在场，
    // 不许只判前缀（只判前缀就分不出"细分丢了"和"细分对了"）。
    let cases: Vec<(&str, u16, String, &str)> = vec![
        (
            "http-status",
            500,
            "{\"error\":{\"message\":\"x\"}}".to_string(),
            "http-status-failure:server-side",
        ),
        (
            "unparsable",
            200,
            "LeakMarkerNotJson".to_string(),
            "unparsable-response",
        ),
        (
            "stopped-generation",
            200,
            text_body("LeakMarkerStopped", "length"),
            "stopped-generation",
        ),
        (
            "empty-reply",
            200,
            text_body("", "stop"),
            "contract-violation",
        ),
        ("two-calls", 200, two_calls_body(), "contract-violation"),
        (
            "arguments-not-json",
            200,
            tool_call_body("call-1", "finish_run", "not-json"),
            "contract-violation",
        ),
        (
            "arguments-extra-field",
            200,
            tool_call_body("call-1", "finish_run", "{\"note\":\"LeakMarkerExtra\"}"),
            "contract-violation",
        ),
        (
            // 从前这条叫 `unknown-ending`：那时结束工具**带一个枚举参数**。现在它不带参数
            // （2026-10-10 裁决），于是"塞一个 `ending` 进来"就是**多一个键**——同一条围栏。
            "finish-tool-takes-no-arguments",
            200,
            tool_call_body("call-1", "finish_run", "{\"ending\":\"ended\"}"),
            "contract-violation",
        ),
        (
            "call-without-id",
            200,
            tool_call_body("", "files_list", "{}"),
            "protocol-payload",
        ),
    ];

    for (name, status, body, expected_failure) in cases {
        let temp = TempDir::new(&format!("wire-{name}"));
        let env_root = temp.env("agent-wire");
        let materials = temp.materials();

        let (endpoint, requests, extra) = spawn_wire_server(vec![(status, body)]);
        let run = Fixture::new(&env_root, "agent-wire", &materials, "run-1", &endpoint).run();

        // 失败被 Core 如实收成"模型这一层终止"，不是主体收场，也不是行动结果。
        assert_eq!(
            code(&run),
            7,
            "{name}: 应当是系统终止\nstdout={}\nstderr={}",
            stdout(&run),
            stderr(&run)
        );
        let report = stdout(&run);
        assert_eq!(
            field(&report, "ending"),
            "system:model-failed",
            "{name}: {report}"
        );
        // 零自动重试：适配层只被调用一次、也只发出一次请求。
        assert_eq!(fixture_field(&report, "model-calls"), "1", "{name}");
        assert_eq!(
            extra
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap_or(usize::MAX),
            0,
            "{name}: 失败之后不得再发第二次请求"
        );
        let raw = first_request(&requests);
        assert!(
            !raw.is_empty(),
            "{name}: 必须真的发出过一次请求（否则这条证明不了）"
        );
        // **分类全程走事实系统**：终端一行稳定读数、stderr 没有侧信道、盘上收场行带同一个分类。
        assert!(
            report.contains(&format!("model-failure={expected_failure}")),
            "{name}: 终端必须给出分类 {expected_failure}: {report}"
        );
        assert!(
            !stderr(&run).contains("provider_adapter:"),
            "{name}: 适配层的 stderr 侧信道必须已经删掉: {}",
            stderr(&run)
        );
        let facts = std::fs::read_to_string(env_root.join("facts.ndjson")).expect("读事实存储");
        let ended = facts
            .lines()
            .find(|line| line.contains("\"fact\":\"run-ended\"") && line.contains("run-1"))
            .expect("必须有这次活动的收场行");
        assert!(
            ended.contains(&format!("\"failure\":\"{expected_failure}\"")),
            "{name}: 分类必须落盘: {ended}"
        );
        assert!(
            ended.contains("\"system\":\"model-failed\""),
            "{name}: 收场取值不得因载荷而改变: {ended}"
        );

        // 终端只看到"模型这一层失败"：没有状态码、没有响应体、没有供应商字段名。
        for leaked in [
            "LeakMarkerEnvelope",
            "LeakMarkerNotJson",
            "LeakMarkerExtra",
            "tool_calls",
            "choices",
            "Internal Server Error",
        ] {
            assert!(
                !report.contains(leaked) && !stderr(&run).contains(leaked),
                "{name}: 供应商细节泄漏到了输出: {leaked}"
            );
        }
    }
}

/// **一次调用的形状错误不报废这一段**（2026-10-11）。
///
/// 真机实测：她第一段活动以 `system:model-failed` / `contract-violation` 收场，一个字都说不出来
/// ——模型把 `invoke` 的载荷形状（`{"args": {…}}`）用在了**直调**上，而翻译层只会把整段判死。
/// 判据在 Core 里早就写着：**不把一次拒绝升级成整个活动的失败**。
///
/// 这一条走**生产路径**（真实适配层 ＋ 真实驱动循环 ＋ 回环线服务器）。三条判据缺一不可：
/// ① 这一段**继续活着**（真的发出了第二次请求，收场不是 `system:model-failed`）；
/// ② 那条读数**进了她的视野**（随单内容在下一次模型调用之前就位）——那是"她能不能改"的全部；
/// ③ 读数**只说形状、不说取值**（参数里藏的 marker 不许出现在任何地方）。
#[test]
fn a_malformed_call_is_refused_with_a_reading_instead_of_ending_the_run() {
    let temp = TempDir::new("wire-malformed-call");
    let env_root = temp.env("agent-wire-malformed");
    let materials = temp.materials();

    // 直调 `files_list`，却包了一层 `invoke` 的载荷形状；`args` 里再嵌一个对象 ⇒ 连消歧也
    // 救不了它，正是"搬不动"那一格（`path` 是一个 object）。
    let (endpoint, requests, _extra) = spawn_wire_server(vec![
        (
            200,
            tool_call_body(
                "call-1",
                "files_list",
                r#"{"args":{"path":{"LeakMarkerShape":1}}}"#,
            ),
        ),
        (200, finish_body()),
    ]);
    let run = Fixture::new(
        &env_root,
        "agent-wire-malformed",
        &materials,
        "run-malformed",
        &endpoint,
    )
    .input("形状验收")
    // 一个空行 = 显式继续：驱动第二次（因此发出第二个请求），收场。
    .host_lines("\n")
    .run();

    let report = stdout(&run);
    assert_eq!(code(&run), 0, "{}", stderr(&run));
    // ① 这一段活着，而且真的走到了下一轮。
    assert_eq!(
        field(&report, "ending"),
        "subject:ended",
        "一次形状错误不许升级成整段的失败：{report}"
    );
    assert_eq!(
        fixture_field(&report, "model-calls"),
        "2",
        "她该被叫醒第二次：{report}"
    );

    let raws = requests
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("两轮都真的发出过请求");
    assert_eq!(raws.len(), 2, "两轮 = 两个请求");
    // ② 读数进了她的视野：分类是**中立词**，而且说清是哪个键、该改成什么。
    assert!(
        raws[1].contains("invalid-argument"),
        "拒绝分类必须是那个中立词：{}",
        redacted(&raws[1])
    );
    assert!(
        raws[1].contains("这一次的参数里"),
        "读数必须说清哪里不对：{}",
        redacted(&raws[1])
    );
    // ③ 只说形状，不说取值（与泄漏守卫同一条纪律）。
    for raw in &raws {
        assert!(
            !raw.contains("LeakMarkerShape"),
            "读数不许泄漏取值：{}",
            redacted(raw)
        );
    }
}

// ---------------------------------------------------------------------------
// 3. 凭据边界
// ---------------------------------------------------------------------------

#[test]
fn a_sentinel_credential_authenticates_but_never_reaches_output_or_history() {
    let temp = TempDir::new("wire-credential");
    let env_root = temp.env("agent-wire");
    let materials = temp.materials();

    // 成功一次、失败一次：两条路径都不许把凭据带出去。
    let (endpoint, requests, _extra) = spawn_wire_server(vec![
        (200, finish_body()),
        (500, "{\"error\":{\"message\":\"x\"}}".to_string()),
    ]);
    let ok = Fixture::new(&env_root, "agent-wire", &materials, "run-ok", &endpoint).run();
    assert_eq!(code(&ok), 0, "{}", stderr(&ok));
    let failed = Fixture::new(&env_root, "agent-wire", &materials, "run-failed", &endpoint).run();
    assert_eq!(code(&failed), 7, "{}", stderr(&failed));

    // 两次请求都真的带了认证。
    let raw = first_request(&requests);
    assert!(
        raw.to_ascii_lowercase()
            .contains(&format!("authorization: bearer {SENTINEL_KEY}").to_ascii_lowercase()),
        "{}",
        redacted(&raw)
    );

    // 但凭据不出现在任何输出里。
    for output in [&ok, &failed] {
        assert!(!stdout(output).contains(SENTINEL_KEY));
        assert!(!stderr(output).contains(SENTINEL_KEY));
        assert!(!stdout(output).to_lowercase().contains("authorization"));
        assert!(!stderr(output).to_lowercase().contains("authorization"));
    }
    // 也不进环境历史：Trace 与 Fact 里都没有它。
    for file in ["trace.ndjson", "facts.ndjson"] {
        let text = std::fs::read_to_string(env_root.join(file)).expect("读环境存储");
        assert!(
            !text.contains(SENTINEL_KEY) && !text.to_lowercase().contains("authorization"),
            "{file} 里出现了凭据"
        );
    }
    // 分类**不再**走 stderr 侧信道：终端在 stdout 上给一行稳定读数，stderr 里没有本层的前缀。
    let report = stdout(&failed);
    assert!(
        report.contains("model-failure=http-status-failure:server-side"),
        "终端必须给出失败分类（含细分档）: {report}"
    );
    assert!(
        !stderr(&failed).contains("provider_adapter:"),
        "侧信道必须已经删掉: {}",
        stderr(&failed)
    );
    assert!(!report.contains(SENTINEL_KEY) && !stderr(&failed).contains(SENTINEL_KEY));

    // 分类**落盘**：`run-ended` 行上多一个可选 `failure` 字段，而收场串一个字没变。
    let facts = std::fs::read_to_string(env_root.join("facts.ndjson")).expect("读事实存储");
    let ended = facts
        .lines()
        .find(|line| {
            line.contains("\"fact\":\"run-ended\"") && line.contains("\"run\":\"run-failed\"")
        })
        .expect("必须有失败那次活动的收场行");
    assert!(
        ended.contains("\"failure\":\"http-status-failure:server-side\""),
        "分类必须落盘（含细分档）: {ended}"
    );
    assert!(
        ended.contains("\"system\":\"model-failed\""),
        "收场取值不得因载荷而改变: {ended}"
    );

    // 诊断命令读回同一个分类（生产诊断路径、只读）。
    let diagnosed = Command::new(binary("environment_diagnose"))
        .arg(path_text(&env_root))
        .args(["agent-wire", "--run", "run-failed"])
        .output()
        .expect("跑诊断命令");
    let diagnosed_text = String::from_utf8_lossy(&diagnosed.stdout).to_string();
    assert!(
        diagnosed_text.contains("outcome=system:model-failed"),
        "诊断必须报出收场: {diagnosed_text}"
    );
    assert!(
        diagnosed_text.contains("failure=http-status-failure:server-side"),
        "诊断必须读回分类（含细分档）: {diagnosed_text}"
    );
}

// ---------------------------------------------------------------------------
// 4. 跨 Run：请求互不串上下文
// ---------------------------------------------------------------------------

#[test]
fn two_runs_rebuild_their_own_request_and_share_no_wire_context() {
    let temp = TempDir::new("wire-two-runs");
    let env_root = temp.env("agent-wire");
    let materials = temp.materials();

    let (endpoint, requests, _extra) =
        spawn_wire_server(vec![(200, finish_body()), (200, finish_body())]);
    let first = Fixture::new(&env_root, "agent-wire", &materials, "run-1", &endpoint)
        .input("第一个活动才有的输入")
        .run();
    assert_eq!(code(&first), 0, "{}", stderr(&first));
    let second = Fixture::new(&env_root, "agent-wire", &materials, "run-2", &endpoint)
        .input("第二个活动才有的输入")
        .run();
    assert_eq!(code(&second), 0, "{}", stderr(&second));

    let all = requests
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("两次请求");
    assert_eq!(all.len(), 2, "两次运行各发一次请求");
    // 每次请求都由**当前** View 重建：不携带上一次的输入、trace 或回应。
    assert!(
        all[0].contains("第一个活动才有的输入"),
        "{}",
        redacted(&all[0])
    );
    assert!(all[0].contains("run = run-1"), "{}", redacted(&all[0]));
    assert!(
        !all[0].contains("第二个活动才有的输入"),
        "{}",
        redacted(&all[0])
    );
    assert!(
        all[1].contains("第二个活动才有的输入"),
        "{}",
        redacted(&all[1])
    );
    assert!(all[1].contains("run = run-2"), "{}", redacted(&all[1]));
    assert!(
        !all[1].contains("第一个活动才有的输入"),
        "第二个 Run 不得看见第一个 Run 的正文: {}",
        redacted(&all[1])
    );
    assert!(
        !all[1].contains("run = run-1"),
        "第二个 Run 不得带着第一个 Run 的事实: {}",
        redacted(&all[1])
    );
}

// ---------------------------------------------------------------------------
// 回环服务与夹具
// ---------------------------------------------------------------------------

/// 一次「调用某个工具」的供应商响应。
///
/// 回程形状按供应商契约：`choices[].message.tool_calls[]`，其中 `arguments` 是**字符串里的 JSON**，
/// `finish_reason` 属于 **choice**，不是 message。
fn tool_call_body(call_id: &str, name: &str, arguments: &str) -> String {
    // 用拼接而不是 format!：这里写的是 JSON 的花括号，和格式占位符的转义混在一起最容易
    // 数错一个括号——而"少一个括号"会变成"响应不可解析"，那是另一件要单独验的事。
    let mut body = String::from("{\"choices\":[{\"message\":{\"tool_calls\":[{\"id\":");
    body.push_str(&quote(call_id));
    body.push_str(",\"type\":\"function\",\"function\":{\"name\":");
    body.push_str(&quote(name));
    body.push_str(",\"arguments\":");
    body.push_str(&quote(arguments));
    body.push_str("}}]},\"finish_reason\":\"tool_calls\"}]}");
    body
}

/// 一次正常收场：调用结束类工具（**不带参数**——2026-10-10 裁决：结束了就是结束了）。
fn finish_body() -> String {
    tool_call_body("call-finish", "finish_run", "{}")
}

/// 一次纯正文响应（没有任何工具调用）。
fn text_body(content: &str, finish_reason: &str) -> String {
    let mut body = String::from("{\"choices\":[{\"message\":{\"content\":");
    body.push_str(&quote(content));
    body.push_str("},\"finish_reason\":");
    body.push_str(&quote(finish_reason));
    body.push_str("}]}");
    body
}

/// 一次回复里出现**两个**工具调用：整体拒绝，任何一个都不执行。
fn two_calls_body() -> String {
    let mut body = String::from("{\"choices\":[{\"message\":{\"tool_calls\":[");
    body.push_str("{\"id\":\"call-1\",\"type\":\"function\",\"function\":{\"name\":\"finish_run\",\"arguments\":\"{}\"}},");
    body.push_str("{\"id\":\"call-2\",\"type\":\"function\",\"function\":{\"name\":\"finish_run\",\"arguments\":\"{}\"}}");
    body.push_str("]},\"finish_reason\":\"tool_calls\"}]}");
    body
}

fn quote(text: &str) -> String {
    serde_json::to_string(text).expect("可以序列化")
}

/// **头一次把响应截断、第二次照常作答**的回环夹具（"真瞬时"那一格的扳机）。
///
/// 第一次只发响应头 ＋ 半个体，停 200ms 再关 ⇒ 客户端**先收下头**、读体时撞上 EOF
/// ⇒ 折出来是带细分的 `transport-failure:io`（**在**重试族里）。
/// 分两段（头 → 停 → 半个体 → 关）是为了让"头先到手"成为**结构**而不是运气。
fn spawn_once_truncating_wire_server(
    reply: (u16, String),
) -> (String, mpsc::Receiver<Vec<String>>, mpsc::Receiver<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("可以绑定回环端口");
    let port = listener.local_addr().expect("有地址").port();
    let (body_sender, body_receiver) = mpsc::channel();
    let (extra_sender, extra_receiver) = mpsc::channel();

    std::thread::spawn(move || {
        let mut raws = Vec::new();
        let mut seen = 0usize;
        while seen < 2 {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            raws.push(read_raw_request(&mut stream));
            seen += 1;
            if seen == 1 {
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n0123456789"
                );
                let _ = stream.flush();
                std::thread::sleep(std::time::Duration::from_millis(200));
                continue;
            }
            let (status, body) = &reply;
            let reason = if *status == 200 {
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
        let _ = extra_sender.send(count_extra_connections(&listener));
    });

    (
        format!("http://127.0.0.1:{port}/chat/completions"),
        body_receiver,
        extra_receiver,
    )
}

/// **头一次连上就丢、第二次照常作答**的回环夹具（"未细分"那一格的扳机）。
///
/// 第一次**什么都不回**就把连接放掉（读侧拿到"连接断在响应之前"）——实测折出来是
/// **未细分**的 `transport-failure`，正是主体线上那两轮的形状 ⇒ 它**不在**重试族里。
fn spawn_once_dropping_wire_server(
    reply: (u16, String),
) -> (String, mpsc::Receiver<Vec<String>>, mpsc::Receiver<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("可以绑定回环端口");
    let port = listener.local_addr().expect("有地址").port();
    let (body_sender, body_receiver) = mpsc::channel();
    let (extra_sender, extra_receiver) = mpsc::channel();

    std::thread::spawn(move || {
        let mut raws = Vec::new();
        // 第一条**一定**来：读掉请求、什么都不回就把连接放掉。
        if let Ok((mut stream, _)) = listener.accept() {
            raws.push(read_raw_request(&mut stream));
        }
        // 再给一个**有界**窗口看有没有第二条（重试）。⚠ 别写成"等满两条才报数"——
        // 收窄之后这一格**本来就只有一条**，那样会让用例挂在超时上、而不是如实报 1 条。
        listener.set_nonblocking(true).expect("设成非阻塞");
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(800);
        while std::time::Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    raws.push(read_raw_request(&mut stream));
                    let (status, body) = &reply;
                    let reason = if *status == 200 {
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
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(_) => break,
            }
        }
        let _ = body_sender.send(raws);
        let _ = extra_sender.send(count_extra_connections(&listener));
    });

    (
        format!("http://127.0.0.1:{port}/chat/completions"),
        body_receiver,
        extra_receiver,
    )
}

/// 脚本用完之后再观察 500ms，**报出多出来的连接数**（`0` = 没有"偷偷多采一次"）。
fn count_extra_connections(listener: &TcpListener) -> usize {
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
    extra
}

/// 回环服务：按脚本应答，然后**继续盯着**看有没有多出来的请求。
///
/// 返回（端点、两轮以内的原始请求文本、**额外连接数**）。
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

        // 脚本用完之后继续观察：多出来的连接 = 有人偷偷重试了。
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
        format!("http://127.0.0.1:{port}/chat/completions"),
        body_receiver,
        extra_receiver,
    )
}

/// 读一条完整的 HTTP 请求（请求行 + 头 + body），原样返回。
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
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("应当收到请求")
        .into_iter()
        .next()
        .expect("至少一条请求")
}

/// 断言失败信息里打印请求时，先把凭据遮掉：测试输出不该成为泄漏渠道。
fn redacted(raw: &str) -> String {
    raw.replace(SENTINEL_KEY, "<redacted>")
}

/// 请求里的角色序列。
fn roles_of(raw: &str) -> Vec<String> {
    raw.split("\"role\":\"")
        .skip(1)
        .map(|tail| tail.split('"').next().unwrap_or_default().to_string())
        .collect()
}

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
                ("MANUAL_FIXTURE_MODEL", "loopback".to_string()),
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

    /// 给状态栏一个**固定钟**（自 1970-01-01T00:00:00Z 起的秒数）。
    ///
    /// 不调用它就与生产命令一样用真实钟；调用它之后请求体里的 `time` 每次都是同一个读数，
    /// 于是"这一行走的是同一条装配路径"这件事可以逐字节断言。
    fn clock(mut self, unix_seconds: u64) -> Self {
        self.variables
            .push(("MANUAL_FIXTURE_CLOCK", unix_seconds.to_string()));
        self
    }

    /// 声明一个**上下文窗口**（状态行 `context` 段的分母）。
    ///
    /// 不调用它就用适配层的公开标称值；调用它之后分母是确定的，`context = …/<这个数> <单位>` 可以逐字节断言。
    fn window(mut self, window: u32) -> Self {
        self.variables
            .push(("MANUAL_FIXTURE_CONTEXT_WINDOW", window.to_string()));
        self
    }

    /// 显式启用**宿主输入循环**并给出后续几个宿主动作（`\n` 分隔：空行 = 显式继续）。
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
        // 显式清掉代理与真实凭据：回环地址不该被代理劫持，测试也不碰真实 Key。
        command
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost")
            .env_remove("HTTP_PROXY")
            .env_remove("HTTPS_PROXY")
            .env_remove("ALL_PROXY")
            .env_remove("http_proxy")
            .env_remove("https_proxy")
            .env_remove("all_proxy")
            .env_remove("DEEPSEEK_API_KEY");
        command.output().expect("启动夹具")
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
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
    let needle = format!("{key}=");
    for line in text.lines() {
        for token in line.split(' ') {
            if let Some(value) = token.strip_prefix(&needle) {
                return value.to_string();
            }
        }
    }
    panic!("报告里没有字段 {key}: {text}");
}

fn fixture_field(text: &str, key: &str) -> String {
    let needle = format!("FIXTURE:{key}=");
    text.lines()
        .find_map(|line| line.strip_prefix(&needle))
        .unwrap_or_else(|| panic!("夹具报告里没有 {key}: {text}"))
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
        // 标签里可能有连字符，只保留字面量即可。
        // 目录名**必须带进程号**：测试二进制是一份一个进程，而计数器只在进程内唯一 ⇒
        // 不带进程号的两份并行（或两个写者同时跑）会算出同一批路径、**互相删目录**。
        let mut path = std::env::temp_dir();
        path.push(format!(
            "celira-task07-{}-{}-{n}",
            std::process::id(),
            label.replace('\n', "-")
        ));
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

// ---------------------------------------------------------------------------
// 5. 缺回执的历史：边界冲刷与合成回执
// ---------------------------------------------------------------------------
//
// **请求可以没有回执**：一次被派发过、结果却从未被确认的调用（崩溃窗口留下的真实形状）
// 在经历侧就是"只有请求、没有回执"。这种残余在三个边界上被冲刷成一条**合成回执**
// （`state = unconfirmed`），而不是让整份历史变成不可翻译的。
//
// 这一节在**真实适配层 + 回环服务**上验证它，并且把两个**不放松**的方向钉在同一处：
// 没有对应请求的回执、以及插在请求与回执之间的注入，仍然在发请求之前就失败。
//
// 明确**不**声称：合成回执之后服务端一定接受（回环只证明 wire 合法，真实复验另有其人）；
// 也不声称"缺回执只能来自崩溃"——那由存储层与恢复层各自负责。

/// 一次回复里给出**若干**工具调用（原生协议允许一轮多个），可带思考续接原文。
fn calls_body(calls: &[(&str, &str, &str)], reasoning: Option<&str>) -> String {
    let items: Vec<String> = calls
        .iter()
        .map(|(id, name, arguments)| {
            format!(
                "{{\"id\":{},\"type\":\"function\",\"function\":{{\"name\":{},\"arguments\":{}}}}}",
                quote(id),
                quote(name),
                quote(arguments)
            )
        })
        .collect();
    let mut message = String::from("{");
    if let Some(reasoning) = reasoning {
        message.push_str("\"reasoning_content\":");
        message.push_str(&quote(reasoning));
        message.push(',');
    }
    message.push_str("\"tool_calls\":[");
    message.push_str(&items.join(","));
    message.push_str("]}");
    format!("{{\"choices\":[{{\"message\":{message},\"finish_reason\":\"tool_calls\"}}]}}")
}

/// 真实适配层把一条 wire 响应翻译成中立响应（续接载荷只能由适配层自己产出，不能手工拼）。
fn neutral_output(body: String) -> ModelOutput {
    let (endpoint, _requests, _extra) = spawn_wire_server(vec![(200, body)]);
    let mut model =
        DeepSeekModel::at_endpoint(SENTINEL_KEY, "fixture-model", endpoint).expect("装配适配层");
    let mut agent = Agent::new(AgentId::new("agent-wire"));
    let run =
        Run::begin(&mut agent, RunId::new("run-translate"), Limits::new(8)).expect("开始活动");
    model
        .complete(&run.view(Capabilities::none()))
        .expect("这条响应必须可翻译")
}

/// 把一份 View 交给**真实适配层**渲染成一次请求并真的发出去，返回原始 HTTP 请求文本。
///
/// 它自己不判成功失败：渲染不出来时适配层会交回 `ModelError`，这里让它 panic——"请求建成"
/// 正是这些用例要证明的事。
fn rendered_request(view: &View) -> String {
    let (endpoint, requests, _extra) = spawn_wire_server(vec![(200, finish_body())]);
    let mut model =
        DeepSeekModel::at_endpoint(SENTINEL_KEY, "fixture-model", endpoint).expect("装配适配层");
    model
        .complete(view)
        .expect("这份历史必须能建成请求（缺回执不构成失败）");
    first_request(&requests)
}

/// 把一份 View 交给真实适配层，**并且**观察它在失败前有没有真的发出请求。
///
/// 返回 `(失败分类, 有没有发出请求)`：`None` = 这次渲染成功了。两个方向都要判——失败是必须的，
/// 而"失败发生在发请求**之前**"是这条规则的实质（不是"发出去了再被服务端拒"）。
fn refused_before_sending(view: &View) -> (Option<String>, bool) {
    let (endpoint, requests, _extra) = spawn_wire_server(vec![(200, finish_body())]);
    let mut model =
        DeepSeekModel::at_endpoint(SENTINEL_KEY, "fixture-model", endpoint).expect("装配适配层");
    let class = model
        .complete(view)
        .err()
        .map(|error| error.class.as_str().to_string());
    let sent = requests
        .recv_timeout(std::time::Duration::from_millis(300))
        .map(|raws| !raws.is_empty())
        .unwrap_or(false);
    (class, sent)
}

/// 一份原始请求的 body（JSON 文本）。
fn body_of(raw: &str) -> &str {
    raw.split_once("\r\n\r\n")
        .map(|(_head, body)| body)
        .expect("请求含空行分隔的头与 body")
}

/// 一份原始请求解析成 JSON。
fn wire_of(raw: &str) -> serde_json::Value {
    serde_json::from_str(body_of(raw)).expect("请求体是 JSON")
}

/// 一条消息里的正文取值（没有 content 字段时是空串）。
fn content_of(message: &serde_json::Value) -> String {
    message
        .get("content")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

/// 消息的角色序列（按发生顺序）。
fn message_roles(wire: &serde_json::Value) -> Vec<String> {
    wire["messages"]
        .as_array()
        .expect("messages 是数组")
        .iter()
        .map(|message| message["role"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// 一条消息在 `messages` 里的下标。
fn message_index(wire: &serde_json::Value, index: usize) -> &serde_json::Value {
    &wire["messages"][index]
}

/// 手工拼轨迹：每一步都走 Core 的真实面（不另造捷径），响应由适配层自己翻译出来。
struct Trace {
    run: Run,
}

impl Trace {
    fn new(run: &str) -> Self {
        let mut agent = Agent::new(AgentId::new("agent-wire"));
        let run = Run::begin(&mut agent, RunId::new(run), Limits::new(8)).expect("开始活动");
        Self { run }
    }

    /// 开一轮，接受一条真实适配层翻译出来的响应，并把这一轮请求的行动交回。
    fn accept(&mut self, body: String) -> Vec<Action> {
        self.run.next_turn().expect("可以开始 Turn");
        let output = neutral_output(body);
        let actions = output.decision().actions().to_vec();
        self.run
            .accept_response(self.run.last_turn(), output)
            .expect("可以记响应");
        actions
    }

    /// 准备 + 派发 + 结算：这一条请求得到了它的回执。
    fn settle(&mut self, action: &Action, index: u16) {
        let operation = self.run.prepare(action, index).expect("可以准备");
        self.run.mark_dispatched(&operation).expect("可以发出");
        self.run
            .observe(action, ActionOutcome::success(Vec::new()))
            .expect("可以结算");
    }

    /// 只准备 + 派发，**不结算**：留下的正是"已派发无结果"那个形状。
    fn dispatch_only(&mut self, action: &Action, index: u16) {
        let operation = self.run.prepare(action, index).expect("可以准备");
        self.run.mark_dispatched(&operation).expect("可以发出");
    }

    /// 一次未被接纳的请求：只留一条拒绝回执，不产生操作身份。
    ///
    /// 这条夹具只造**不带随单内容**的拒绝（形状与从前逐字节相同）；带随单内容的那种由
    /// `refuse_bringing` 造（TASK_72）。
    fn refuse(&mut self, action: &Action, class: &str) {
        self.refuse_bringing(action, class, Vec::new());
    }

    /// 同 [`Fixture::refuse`]，但随单带上内容（TASK_72）。
    fn refuse_bringing(&mut self, action: &Action, class: &str, content: Vec<ContentBlock>) {
        self.run
            .accept_refusal(
                self.run.last_turn(),
                action.id(),
                class.to_string(),
                content,
            )
            .expect("可以记拒绝");
    }

    fn view(&self) -> View {
        self.run.view(Capabilities::none())
    }
}

/// §6.1：轨迹**末尾**缺回执（真崩溃形状）⇒ 请求建成，而且带一条合成回执。
#[test]
fn a_trailing_request_without_a_receipt_is_rebuilt_with_a_synthesised_receipt() {
    let mut trace = Trace::new("run-dangling");
    let actions = trace.accept(calls_body(
        &[("c-1", "files_create", "{\"name\":\"x.txt\"}")],
        None,
    ));
    trace.dispatch_only(&actions[0], 0);

    let raw = rendered_request(&trace.view());
    let wire = wire_of(&raw);
    assert_eq!(
        message_roles(&wire),
        vec!["system", "assistant", "tool", "user"],
        "缺回执的请求必须只有一个助手帧、一条回执，再加机械事实"
    );
    assert_eq!(message_index(&wire, 1)["tool_calls"][0]["id"], "c-1");
    assert_eq!(message_index(&wire, 2)["tool_call_id"], "c-1");
    let receipt = content_of(message_index(&wire, 2));
    assert!(receipt.contains("state = unconfirmed"), "{receipt}");
    assert!(receipt.contains("action = files_create"), "{receipt}");
}

/// §6.1：一轮请求**两个**行动、回执全缺 ⇒ 两条合成回执**按请求顺序**逐条出现。
///
/// 形状说明：Core 的循环同一时刻只有一个在途尝试，因此"两条都缺"在真实崩溃里只可能出现在
/// **这一轮一条回执都没写成**的时候（这一轮的响应确已确认，之后崩溃）。这里就是用这个形状。
#[test]
fn two_missing_receipts_are_synthesised_in_request_order() {
    let mut trace = Trace::new("run-two-dangling");
    let actions = trace.accept(calls_body(
        &[
            ("c-1", "files_create", "{\"name\":\"x.txt\"}"),
            ("c-2", "files_list", "{}"),
        ],
        None,
    ));
    assert_eq!(actions.len(), 2);

    let raw = rendered_request(&trace.view());
    let wire = wire_of(&raw);
    assert_eq!(
        message_roles(&wire),
        vec!["system", "assistant", "tool", "tool", "user"]
    );
    let calls = message_index(&wire, 1)["tool_calls"]
        .as_array()
        .expect("两个调用")
        .iter()
        .map(|call| call["id"].as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert_eq!(calls, vec!["c-1", "c-2"], "请求顺序不得被打乱");
    for (index, (call_id, action)) in [("c-1", "files_create"), ("c-2", "files_list")]
        .into_iter()
        .enumerate()
    {
        let receipt = message_index(&wire, 2 + index);
        assert_eq!(receipt["tool_call_id"], call_id);
        let text = content_of(receipt);
        assert!(text.contains("state = unconfirmed"), "{text}");
        assert!(text.contains(&format!("action = {action}")), "{text}");
    }
}

/// §6.2：**部分缺** ⇒ 缺的那条合成，有的那条逐字节原样。
#[test]
fn only_the_missing_receipt_is_synthesised() {
    let mut trace = Trace::new("run-partial");
    let actions = trace.accept(calls_body(
        &[
            ("c-1", "files_create", "{\"name\":\"x.txt\"}"),
            ("c-2", "files_list", "{}"),
        ],
        None,
    ));
    trace.settle(&actions[0], 0);
    trace.dispatch_only(&actions[1], 1);

    let raw = rendered_request(&trace.view());
    let wire = wire_of(&raw);
    assert_eq!(
        message_roles(&wire),
        vec!["system", "assistant", "tool", "tool", "user"]
    );
    let first = content_of(message_index(&wire, 2));
    let second = content_of(message_index(&wire, 3));
    // 有的那条：仍是一条完整的**行动结果**回执（有操作身份、有真实结果类别），不是合成形。
    assert!(first.contains("operation = "), "{first}");
    assert!(first.contains("state = success"), "{first}");
    assert!(!first.contains("unconfirmed"), "{first}");
    // 缺的那条：合成形，只有行动与"未确认"。
    assert_eq!(
        second,
        "action = files_list\nstate = unconfirmed\nnote = dispatched but result never confirmed",
        "缺回执处只渲染如实标注的合成回执"
    );
}

/// §6.3（反方向之一）：**没有对应请求**的回执 ⇒ 仍在发请求之前失败。
#[test]
fn an_orphan_receipt_is_still_refused_before_anything_is_sent() {
    let mut trace = Trace::new("run-orphan");
    trace.run.next_turn().expect("可以开始 Turn");
    let action = Action::new(ActionId::new("probe"));
    trace.settle(&action, 0);

    let (class, sent) = refused_before_sending(&trace.view());
    assert_eq!(
        class.as_deref(),
        Some("unpaired-history"),
        "没有对应请求的回执必须明确失败，而且分类要如实过边界"
    );
    assert!(!sent, "失败必须发生在发出请求之前");
}

/// §6.4：**正常历史**一个字都不改——多动作轮次、准入拒绝回执、纯正文、收尾全在。
#[test]
fn a_normal_history_is_rebuilt_without_any_synthesised_receipt() {
    let mut trace = Trace::new("run-normal");
    // 第 1 轮：一次请求两个行动，两条回执都在（含思考续接原文）。
    let actions = trace.accept(calls_body(
        &[
            ("c-1", "open_app", "{\"target\":\"files\"}"),
            ("c-2", "files_list", "{}"),
        ],
        Some("先展开再列目录"),
    ));
    trace.settle(&actions[0], 0);
    trace.settle(&actions[1], 1);
    // 第 2 轮：一个动作被**准入拒绝**（拒绝回执也是回执）。
    let refused = trace.accept(calls_body(
        &[("c-3", "files_read", "{\"path\":\"x.txt\"}")],
        None,
    ));
    trace.refuse(&refused[0], "not-expanded");
    // 第 3 轮：纯正文（交回控制）。
    trace.run.next_turn().expect("可以开始 Turn");
    let yield_output = neutral_output(text_body("先说一句", "stop"));
    trace
        .run
        .accept_response(trace.run.last_turn(), yield_output)
        .expect("可以记响应");
    // 第 4 轮：主体自己收场。
    trace.run.next_turn().expect("可以开始 Turn");
    let finish_output = neutral_output(calls_body(&[("c-4", "finish_run", "{}")], None));
    trace
        .run
        .accept_response(trace.run.last_turn(), finish_output)
        .expect("可以记响应");

    let raw = rendered_request(&trace.view());
    let wire = wire_of(&raw);
    assert_eq!(
        message_roles(&wire),
        vec![
            "system",
            "assistant",
            "tool",
            "tool",
            "assistant",
            "tool",
            "assistant",
            "assistant",
            "user"
        ],
        "回执全在的历史不得多出任何一条消息"
    );
    assert!(
        !raw.contains("unconfirmed"),
        "正常历史里不得出现合成回执: {raw}"
    );
    // 真的两条回执：一条是行动结果、一条是准入拒绝，形状各自保持原样。
    let first = content_of(message_index(&wire, 2));
    assert!(first.contains("operation = "), "{first}");
    assert!(first.contains("state = success"), "{first}");
    let second = content_of(message_index(&wire, 3));
    assert!(second.contains("operation = "), "{second}");
    assert!(second.contains("state = success"), "{second}");
    let refused = content_of(message_index(&wire, 5));
    assert!(refused.contains("admission = not-expanded"), "{refused}");
    assert!(refused.contains("dispatched = false"), "{refused}");
    // 纯正文那一轮没有产生任何工具调用，也没有回执。
    assert_eq!(message_index(&wire, 6)["role"], "assistant");
    assert!(message_index(&wire, 6).get("tool_calls").is_none());
    assert!(content_of(message_index(&wire, 6)).contains("先说一句"));
    // 收尾那一轮同样不产生工具调用。
    assert!(message_index(&wire, 7).get("tool_calls").is_none());
}

/// §6.5：合成回执的正文只陈述"发出过、结果未确认"——不编结果、不编操作身份。
#[test]
fn a_synthesised_receipt_states_only_the_action_and_the_unconfirmed_state() {
    let mut trace = Trace::new("run-shape");
    let actions = trace.accept(calls_body(
        &[("c-1", "files_create", "{\"name\":\"x.txt\"}")],
        None,
    ));
    trace.dispatch_only(&actions[0], 0);

    let raw = rendered_request(&trace.view());
    let wire = wire_of(&raw);
    let receipt = content_of(message_index(&wire, 2));
    assert_eq!(
        receipt,
        "action = files_create\nstate = unconfirmed\nnote = dispatched but result never confirmed",
        "合成回执的形状是定死的三行"
    );
    for forbidden in [
        "success",
        "failure",
        "operation =",
        "admission =",
        "dispatched = false",
        "<<<DATA",
    ] {
        assert!(
            !receipt.contains(forbidden),
            "合成回执不得出现 {forbidden}: {receipt}"
        );
    }
}

/// §6.8：空正文的助手帧有两种情形，必须分开渲染。
///
/// - **没有工具调用**（空正文的收尾）⇒ 必须是**合法的空正文**。实测服务端对这种帧直接回 400
///   `Invalid assistant message: content or tool_calls must be set`（记录型代理重放，脱凭据）；
/// - **有工具调用** ⇒ 保持 `content = null`（真实运行里一直是这个形状，动它没有理由）。
#[test]
fn an_empty_finish_frame_becomes_a_legal_empty_assistant_message() {
    let mut trace = Trace::new("run-empty-finish");
    // 第 1 轮：一次行动（空正文 + 工具调用），两条回执都在。
    let actions = trace.accept(calls_body(
        &[
            ("c-1", "open_app", "{\"target\":\"files\"}"),
            ("c-2", "files_list", "{}"),
        ],
        None,
    ));
    trace.settle(&actions[0], 0);
    trace.settle(&actions[1], 1);
    // 第 2 轮：纯正文。
    trace.run.next_turn().expect("可以开始 Turn");
    let plain = neutral_output(text_body("先说一句", "stop"));
    trace
        .run
        .accept_response(trace.run.last_turn(), plain)
        .expect("可以记响应");
    // 第 3 轮：**空正文的收尾**——没有工具调用。
    trace.run.next_turn().expect("可以开始 Turn");
    let empty_finish = ModelOutput::finish(SubjectEnding::Completed);
    assert!(empty_finish.content().is_empty(), "这条收尾必须没有正文");
    trace
        .run
        .accept_response(trace.run.last_turn(), empty_finish)
        .expect("可以记响应");

    let raw = rendered_request(&trace.view());
    let wire = wire_of(&raw);
    assert_eq!(
        message_roles(&wire),
        vec![
            "system",
            "assistant",
            "tool",
            "tool",
            "assistant",
            "assistant",
            "user"
        ]
    );
    // 有工具调用的空正文帧：仍是 `null`（正常形状，不动）。
    assert!(
        message_index(&wire, 1)["tool_calls"].is_array(),
        "第 1 轮那条助手帧带工具调用"
    );
    assert_eq!(
        message_index(&wire, 1)["content"],
        serde_json::Value::Null,
        "有工具调用的空正文帧保持 null"
    );
    // 没有工具调用的空正文帧：合法的空正文，而不是 null。
    assert!(message_index(&wire, 5).get("tool_calls").is_none());
    assert_eq!(
        content_of(message_index(&wire, 5)),
        "",
        "空正文收尾必须渲染成空字符串，而不是 null"
    );
}

/// 传输层**瞬时**失败（落在带细分的档上）：立刻再采一次 ⇒ 第二次够着了 ⇒ 这一轮**照常收场**。
///
/// 为什么用"读体断"这个形状当扳机：它折出来是**带细分**的 `transport-failure:io`
/// （实测：`is_decode` ＋ `UnexpectedEof`），因此它**在收窄后的族里**——
/// 这正是本刀要救的那一格。判据落在**服务端数了几条连接**上：恰好 **2** 条
/// （第一次被截断、第二次拿到答复），且**没有第三条**（有界）。
/// 适配层之上的 `model-calls` **看不见**重试（它数的是 `complete` 被调用几次），也不该看见。
#[test]
fn a_transient_transport_failure_is_sampled_once_more_and_then_lands_normally() {
    let temp = TempDir::new("wire-retry-transient");
    let env_root = temp.env("agent-wire");
    let materials = temp.materials();

    let (endpoint, requests, extra) = spawn_once_truncating_wire_server((200, finish_body()));
    let run = Fixture::new(&env_root, "agent-wire", &materials, "run-retry", &endpoint).run();

    assert_eq!(
        code(&run),
        0,
        "第二次采成功了 ⇒ 这一拍应当照常收场（不是 system:model-failed）\nstdout={}\nstderr={}",
        stdout(&run),
        stderr(&run)
    );
    assert_eq!(field(&stdout(&run), "ending"), "subject:ended");

    let raws = requests
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("服务端应当收到请求");
    assert_eq!(
        raws.len(),
        2,
        "恰好再采一次（第一次被截断、第二次拿到答复）"
    );
    assert_eq!(
        extra
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap_or(usize::MAX),
        0,
        "有界：不许有第三次"
    );
    // 两次请求是**同一个视野**（零参数的"立即再采"）：逐字节相同。
    assert_eq!(raws[0], raws[1], "重试必须原样再采，不带任何新东西");
}

/// **未细分**的传输失败：**不重采**（TASK_60 收窄判据的端到端一读）。
///
/// 这一格就是"收窄"的落点，也是**本刀的代价**：主体至今观测到的失败**全是未细分**
/// （§10.1 的时间线）⇒ 收窄之后**那些拍不会被重试**。为什么要这么收：认不出可能正是
/// "有持久原因"的那一类（证书／TLS 刻意没发布，它落在这里），那种东西重采一次不会变。
/// 形状要靠**形态指纹**那一行取证（§6），量出稳定形状之后再议并族。
#[test]
fn an_unclassified_transport_failure_is_not_re_sampled() {
    let temp = TempDir::new("wire-retry-unclassified");
    let env_root = temp.env("agent-wire");
    let materials = temp.materials();

    // 「连上了、读到了请求、然后什么都不回就关掉」——实测折出来是**未细分**的
    // `transport-failure`（这正是主体线上那两轮的形状）。
    let (endpoint, requests, extra) = spawn_once_dropping_wire_server((200, finish_body()));
    let run = Fixture::new(
        &env_root,
        "agent-wire",
        &materials,
        "run-unclassified",
        &endpoint,
    )
    .run();

    assert_eq!(code(&run), 7, "{}", stderr(&run));
    assert_eq!(field(&stdout(&run), "ending"), "system:model-failed");
    let facts = std::fs::read_to_string(env_root.join("facts.ndjson")).expect("读事实存储");
    let ended = facts
        .lines()
        .find(|line| line.contains("\"fact\":\"run-ended\""))
        .expect("有收场行");
    assert!(
        ended.contains("\"failure\":\"transport-failure\""),
        "未细分就该如实落成不带细分的那个词: {ended}"
    );

    // TASK_60 §6：**认不出**的形态留下了一行形态指纹（夹具与宿主同一条通路）。
    // 这一条同时把"只进宿主日志、不进领域与事实"钉住：它在 stderr 上，`facts.ndjson` 里没有它。
    let stderr_text = stderr(&run);
    assert!(
        stderr_text.contains("transport-shape stage=send"),
        "认不出的形态必须留下形态指纹: {stderr_text}"
    );
    assert!(
        !facts.contains("transport-shape"),
        "指纹只进诊断面，不许进事实: {facts}"
    );
    assert!(
        stderr_text.contains("timeout=") && stderr_text.contains("io="),
        "指纹要带五个布尔与错误链里的 io 种类: {stderr_text}"
    );

    let raws = requests
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("服务端应当收到请求");
    assert_eq!(raws.len(), 1, "未细分**不重采** ⇒ 只该有一条连接");
    assert_eq!(
        extra
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap_or(usize::MAX),
        0,
        "也不许有第二条"
    );
}

/// 传输失败也走同一条事实路径：终端一行分类、stderr 没有侧信道、盘上收场行带
/// `transport-failure:refused`。
///
/// 这一格**没有回环服务**：端点指向一个刚被放掉的端口，连接必然失败——正好是"够不着模型服务"。
/// 夹具显式清掉代理并设 `NO_PROXY`（见 `Fixture::run`），所以这里是**直连被拒**；
/// 经代理的同一条路会给 HTTP 502、折进 `http-status-failure:*`，那是另一格的事。
#[test]
fn a_transport_failure_also_carries_its_class_into_the_facts() {
    let temp = TempDir::new("wire-transport");
    let env_root = temp.env("agent-wire");
    let materials = temp.materials();

    let dead = TcpListener::bind("127.0.0.1:0").expect("可以绑定回环端口");
    let port = dead.local_addr().expect("有地址").port();
    drop(dead);
    let endpoint = format!("http://127.0.0.1:{port}/chat/completions");

    let run = Fixture::new(&env_root, "agent-wire", &materials, "run-1", &endpoint).run();
    assert_eq!(code(&run), 7, "{}", stderr(&run));
    let report = stdout(&run);
    assert_eq!(field(&report, "ending"), "system:model-failed", "{report}");
    assert!(
        report.contains("model-failure=transport-failure:refused"),
        "终端必须给出分类（含细分档）: {report}"
    );
    assert!(
        !stderr(&run).contains("provider_adapter:"),
        "侧信道必须已经删掉: {}",
        stderr(&run)
    );
    let facts = std::fs::read_to_string(env_root.join("facts.ndjson")).expect("读事实存储");
    let ended = facts
        .lines()
        .find(|line| line.contains("\"fact\":\"run-ended\"") && line.contains("\"run\":\"run-1\""))
        .expect("必须有收场行");
    assert!(
        ended.contains("\"failure\":\"transport-failure:refused\""),
        "分类必须落盘（含细分档）: {ended}"
    );
}

/// **保留工具名进了账，回放照样原样引用**（2026-10-13 第二刀改判决）。
///
/// ⚠ **原约束 → 新边界**。从前这条钉的是"轨迹里若真出现以结束类工具名为标识的行动，重建必须
/// 明确失败"（`reserved-tool-name`），依据是**"这一格在生产流程里不可达"**。那个前提当晚被真机
/// 推翻：`invoke` 里包一个保留名**就是**一条被拒绝的调用（一次名字搬不动不再判死整段，见
/// `a_reserved_name_inside_invoke_is_one_calls_problem_too`），它的标识会进账 ⇒ 那道闸把整段
/// 又判死了（dev 家两支 agent 20 分钟内各一次，都是整段清醒期报销）。
///
/// **新边界**：回放**一律原样引用**——那次调用**从未派发**（准入会拒它），紧跟着它的就是我们
/// 那条拒绝读数；而**名字的合法性属于声明侧**（`tools_for` 仍然拒、分类仍可达，
/// `an_illegal_or_reserved_tool_name_is_rejected` 守着那一半），不属于回放侧。
/// **真实消费者**：配对（wire 要求每个 `tool_call` 都有一个结果）。
///
/// ⚠ **旧规矩 → 新规矩**（2026-09-26）：这条用例原来用 `Run::resume` 把手工拼的轨迹**灌进**
/// 一个 Run（那是"跨进程恢复"的顺带用法）。那条路已退役，于是轨迹改由**正常驱动**长出来：
/// `Run::begin_with_input` → `next_turn` → `accept_response` 得到的正是同一条
/// `TraceEntry::ModelResponse`。**要守的不变量一个字没变**（保留工具名必须带分类被拒），
/// 换的只是"这份历史从哪来"。
#[test]
fn a_persisted_reserved_name_is_replayed_verbatim_and_does_not_refuse_the_run() {
    let action = Action::new(ActionId::new("finish_run"));
    let protocol = ProtocolData::new("chat-completions", 1, "{\"call_id\":\"c-1\"}".to_string())
        .expect("载荷合法");
    let output = ModelOutput::new(String::new(), Some(protocol), Decision::Act(vec![action]));
    let mut agent = Agent::new(AgentId::new("agent-wire"));
    let mut run = Run::begin_with_input(
        &mut agent,
        RunId::new("run-reserved"),
        Limits::new(4),
        RunInput::empty(),
    )
    .expect("开始活动");
    let turn = run.next_turn().expect("可以开始第一轮");
    run.accept_response(turn, output)
        .expect("把这次响应记进轨迹");
    assert!(
        matches!(run.trace(), [TraceEntry::ModelResponse(_)]),
        "前提：这份历史就是那一条模型响应（请求了那个保留工具名）"
    );

    let (class, _sent) = refused_before_sending(&run.view(Capabilities::none()));
    assert!(
        class.is_none(),
        "保留名不再让请求重建失败：那次调用从未派发，回放它对配对是必需的、且无损"
    );
}
