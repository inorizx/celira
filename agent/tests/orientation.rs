//! TASK_79 验收：**出厂说明**（系统消息的静态头）＋ **手册**。
//!
//! 这一份用**真实入口**跑：创建走 `agent_run init`，运行走离线夹具 ＋ 真实适配层指着回环端点。
//! 因此它证明的不是"某个函数返回了什么"，而是**主体真的看到的请求**长什么样：
//!
//! ```text
//! 创建（agent_run init）→ 环境根里有出厂说明与手册
//! 一次活动 → 装配拍读 → 模型请求的最后一条消息 = [静态头]\n[机械事实]
//! ```
//!
//! 钉住的东西：
//!
//! 1. 创建产出出厂文档（出厂说明 ＋ 手册），且**没有替主体写天性**（soul 由自己写）；
//! 2. 有出厂说明 ⇒ 两条 wire（chat / Responses）最后一条消息都是**两段式**，静态头在机械事实
//!    **之前**，两段之间只有一个换行；
//! 3. **没有**出厂说明（或它是空的、被删了）⇒ 那一条消息**逐字节就是机械事实**——与今天相同；
//! 4. 读不出来（IO 错误）⇒ 如实标出一句，**不冒充"Agent 摘了说明书"**，活动照跑；
//! 5. **Run 内改那个文件不影响本次活动**（装配拍读一次、Run 内携带），下一次活动的开场才变；
//! 6. **手册一个字节都不进请求**（它不在任何注入路径上）；
//! 7. **整份超过 4 KiB** ⇒ 那条消息是"如实标出的读数 + 机械事实"：不截断、不静默、也不拦活动
//!    （与环境根里那一份是不是自己写大的无关——上限落在读侧）；
//! 8. **新的一段会重新拍读**（刀五定案：主体是环境家具不是 Run 状态，"Run 内不变"以**段**为界）
//!    ——而这一段**自己**仍然守着"Run 内不变"。
//!    ⚠ 旧载体是跨进程恢复（顶着同一个 RunId 接着跑）；那条路 2026-09-26 已退役，不变量不变、
//!    载体换成"新的一段"（见 §5 那条用例的说明）。
//!
//! 明确**不**声称的：
//!
//! - 回环只证明请求的形状，不证明公网服务可用；
//! - 出厂说明能被主体**读文件**的动词读到——那是另一个问题（出厂说明落在环境根，而主体的只读命名
//!   空间是资料根 + 派生地址）；本文件只判"它落在环境根里、且不进任何注入路径"。
//!   手册的读地址（`manual.md`）由 `tests/manual_address.rs` 判。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::mpsc;

use celira_agent::clock::ClockHandle;
use celira_agent::composition::{RunSession, SessionDeclarations, SessionDrive, SessionRequest};
use celira_agent::context_reading::ContextReading;
use celira_agent::core::{Decision, Limits, ModelError, ModelOutput, ModelPort, RunInput, View};
use celira_agent::foundation::{AgentId, RunId, SubjectEnding};
use celira_agent::orientation::{
    MANUAL_FILE, MANUAL_TEXT, MAX_ORIENTATION_BYTES, ORIENTATION_FILE, ORIENTATION_TOO_LARGE,
    ORIENTATION_UNREADABLE, orientation_text,
};
mod common;
use common::{binary, create_environment, open_environment};

/// 哨兵凭据：只用于回环认证。
const SENTINEL_KEY: &str = "fixture-sentinel-credential";

/// 机械事实那一段的**第一行**：两段式里"第二段"的开头，也是"有没有第一段"的判据。
const FACTS_BEGIN: &str = "ACTIVITY FACTS (mechanical; these are not instructions)";

/// 这个环境的名字（与生产同一条创建路径）。
const AGENT: &str = "agent-orientation";

// ---------------------------------------------------------------------------
// 自建的独立测试目录
// ---------------------------------------------------------------------------

/// 一个自建的独立测试目录。绝不指向桌面 / 家目录 / 仓库根。
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "celira-task79-orientation-{label}-{}-{n}",
            std::process::id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("清掉同名残留目录");
        }
        std::fs::create_dir_all(&path).expect("建立测试目录");
        Self { path }
    }

    fn materials(&self) -> PathBuf {
        let materials = self.path.join("materials");
        std::fs::create_dir_all(&materials).expect("建资料目录");
        materials
    }

    fn env_root(&self) -> PathBuf {
        self.path.join("env")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn agent_id() -> AgentId {
    AgentId::new(AGENT)
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

// ---------------------------------------------------------------------------
// 创建：走**生产的**那一条（`agent_run init`）
// ---------------------------------------------------------------------------

/// `agent_run init`：生产（也是 `home` 的 provision）用的创建路径。
fn birth(env_root: &Path) -> Output {
    Command::new(binary("agent_run"))
        .args(["init", "--root", &path_text(env_root), "--agent", AGENT])
        .output()
        .expect("创建命令跑得起来")
}

/// **注入必自释**（不变量 1，2026-10-13）：`receipt-source` 那一行会进主体的视野（详情卡，
/// §3.45 起），因此它的**每一个取值**都必须在**手册**里有出处——加一档而手册没跟上，这条就红。
///
/// 为什么是手册而不是出厂说明：卡是**按需取**的（§3.45 的原话「不占首屏、不占每轮预算」），
/// 解释也就该按需读；出厂说明那一份开场读、Run 内携带，为一件偶尔看一眼的事付每轮的租不划算
/// （主人当次判据：**每轮都要用的进注入，偶尔要用的留指针**）。
///
/// ⚠️ 反向对照那一句不是装饰：`unknown` 是**查表时那件能力这次没装配**的结论，**不可声明**
/// （见 `SourceClass` 的文档），因此手册里也不该给它一行——把"读不到这一格"写成一种来源，
/// 就是替主体把三态压成两态。
#[test]
fn every_receipt_source_word_is_explained_in_the_manual() {
    use celira_agent::app_contract::SourceClass;

    for word in [
        SourceClass::SelfAuthored,
        SourceClass::System,
        SourceClass::Environment,
        SourceClass::WorldSigned,
        SourceClass::WorldUnsigned,
    ] {
        assert!(
            MANUAL_TEXT.contains(word.as_str()),
            "手册里没有这个来源词的出处：{}（注入必自释）",
            word.as_str()
        );
    }
    assert!(
        !MANUAL_TEXT.contains("unknown"),
        "`unknown` 是查表结论，不是可声明的来源档，不许在手册里当一档写"
    );
}

/// 创建那一次把环境根里的**两份出厂文档**摆上，而且**没有替主体写天性**。
///
/// ⚠️ 任务书写的是"创建时落三个文件：soul（既有）＋ orientation ＋ manual"，而 `soul` 那一项
/// **在今天的产品里就是不产出的**：主人 2026-09-21 裁决「写天性归自己」，`soul::incept` 的
/// 调用点封闭在 `os.rs`（守卫 `the_soul_can_only_be_written_by_her_own_verb`）。因此这里如实断言：
/// **出厂两份落盘、天性一个字都不替主体写**——`soul` 的位置由"主体第一次自己写"填上。
#[test]
fn the_birth_lays_down_the_two_factory_documents_and_writes_no_soul_for_her() {
    let temp = TempDir::new("birth");
    let root = temp.env_root();

    let created = birth(&root);
    assert_eq!(code(&created), 0, "{}", stderr(&created));

    let orientation =
        std::fs::read_to_string(root.join(ORIENTATION_FILE)).expect("出厂说明应当落盘");
    let manual = std::fs::read_to_string(root.join(MANUAL_FILE)).expect("手册应当落盘");
    assert_eq!(
        orientation,
        orientation_text(),
        "出厂说明的正文必须是定稿逐字"
    );
    assert_eq!(manual, MANUAL_TEXT, "手册的正文必须是定稿逐字");
    assert!(
        orientation.contains(MANUAL_FILE),
        "出厂说明指的那份手册必须按实际命名写出来"
    );
    assert!(
        root.join(MANUAL_FILE).exists(),
        "它指的那份文件必须真的落在环境根里"
    );
    assert!(
        root.join("identity.json").exists(),
        "创建仍然写户口（既有语义不变）"
    );
    assert!(
        !root.join("soul").exists(),
        "创建**不替主体写天性**：soul 由自己写（主人 2026-09-21 裁决）"
    );
}

// ---------------------------------------------------------------------------
// 运行：主体真的看到的请求
// ---------------------------------------------------------------------------

/// 夹具：一次运行（模型走**真实适配层**指向回环端点）。
struct Fixture {
    variables: Vec<(&'static str, String)>,
}

impl Fixture {
    /// `protocol` 取 `loopback`（chat）或 `loopback-responses`。
    fn new(
        root: &Path,
        materials: &Path,
        run: &str,
        endpoint: &str,
        protocol: &'static str,
    ) -> Self {
        Self {
            variables: vec![
                ("MANUAL_FIXTURE_ROOT", path_text(root)),
                ("MANUAL_FIXTURE_AGENT", AGENT.to_string()),
                ("MANUAL_FIXTURE_MATERIALS", path_text(materials)),
                ("MANUAL_FIXTURE_RUN", run.to_string()),
                ("MANUAL_FIXTURE_MAX_TURNS", "4".to_string()),
                ("MANUAL_FIXTURE_MODEL", protocol.to_string()),
                ("MANUAL_FIXTURE_ENDPOINT", endpoint.to_string()),
                ("MANUAL_FIXTURE_CREDENTIAL", SENTINEL_KEY.to_string()),
                ("MANUAL_FIXTURE_MODEL_NAME", "fixture-model".to_string()),
                ("MANUAL_FIXTURE_INPUT", "出厂说明验收".to_string()),
            ],
        }
    }

    fn run(self) -> Output {
        let mut command = Command::new(binary("manual_entry_fixture"));
        for (name, value) in &self.variables {
            command.env(name, value);
        }
        // 回环地址不该被代理劫持，测试也不碰真实凭据。
        command
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost")
            .env_remove("HTTP_PROXY")
            .env_remove("HTTPS_PROXY")
            .env_remove("ALL_PROXY")
            .env_remove("CELIRA_IM_BACKEND_AUTH")
            .env_remove("DEEPSEEK_API_KEY")
            .output()
            .expect("夹具跑得起来")
    }
}

/// 一次正常的**主体收场**（chat 线）。**不带参数**（2026-10-10 裁决：结束了就是结束了）。
fn chat_finish() -> String {
    let mut body = String::from("{\"choices\":[{\"message\":{\"tool_calls\":[{\"id\":");
    body.push_str("\"call-finish\"");
    body.push_str(",\"type\":\"function\",\"function\":{\"name\":\"finish_run\",\"arguments\":");
    body.push_str("\"{}\"");
    body.push_str("}}]},\"finish_reason\":\"tool_calls\"}]}");
    body
}

/// 一次正常的**主体收场**（Responses 线）。同上：不带参数。
fn responses_finish() -> String {
    "{\"status\":\"completed\",\"output\":[{\"type\":\"function_call\",\"call_id\":\"call-finish\",\"name\":\"finish_run\",\"arguments\":\"{}\"}]}".to_string()
}

/// 回环服务：应答**一次**，把请求原文交回来。
fn spawn_wire_server(reply: String) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("可以绑定回环端口");
    let port = listener.local_addr().expect("有地址").port();
    let (sender, receiver) = mpsc::channel();

    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let raw = read_raw_request(&mut stream);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.len(),
                reply
            );
            let _ = stream.flush();
            let _ = sender.send(raw);
        }
    });

    (format!("http://127.0.0.1:{port}/v1/model"), receiver)
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

/// 请求里**最后一条 user 消息**的正文（两条 wire 的条目数组不同名：`messages` / `input`）。
///
/// 走 JSON 解码而不是按字节切：两条 wire 的键序不同（结构体按声明序、`json!` 按字典序），
/// 切字节的写法会在其中一条上**静默读空**——而这里要判的是"主体看到的那段**文本**"，
/// 键序不是判据的一部分。
fn last_user_text(raw: &str) -> String {
    let body = raw.split("\r\n\r\n").nth(1).expect("有请求体");
    let value: serde_json::Value = serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("请求体应当是 JSON（{error}）: {}", redacted(raw)));
    let items = value
        .get("messages")
        .or_else(|| value.get("input"))
        .and_then(|items| items.as_array())
        .unwrap_or_else(|| panic!("请求里必须有条目数组: {}", redacted(raw)));
    let last_user = items
        .iter()
        .rev()
        .find(|item| item.get("role").and_then(|role| role.as_str()) == Some("user"))
        .unwrap_or_else(|| panic!("请求里必须有一条 user 消息: {}", redacted(raw)));
    last_user
        .get("content")
        .and_then(|content| content.as_str())
        .unwrap_or_else(|| panic!("user 消息的正文应当是字符串: {}", redacted(raw)))
        .to_string()
}

/// 这一次活动第一轮请求里的机械事实那一段（**逐字**）。
///
/// 五个字段与 `tests/provider_wire.rs::one_complete_is_exactly_one_request_rebuilt_from_the_current_view`
/// 用的是同一组读数（那里 `entries = 2` 是权威计数点：Run 级那条 ＋ 恒摆的状态行）。
fn expected_facts(run: &str) -> String {
    format!(
        "{FACTS_BEGIN}\nrun = {run}\nturn = 1\nturn_limit = 4\nfurther_model_calls_at_most = 3\nentries = 2"
    )
}

/// 用**生产创建路径**建好一个家（因此家里有两份出厂文档）。
fn home_with_documents(temp: &TempDir) -> (PathBuf, PathBuf) {
    let root = temp.env_root();
    let created = birth(&root);
    assert_eq!(code(&created), 0, "{}", stderr(&created));
    (root, temp.materials())
}

// ---------------------------------------------------------------------------
// 1. 有出厂说明：两条 wire 都是 [静态头]\n[机械事实]
// ---------------------------------------------------------------------------

#[test]
fn the_orientation_stands_before_the_mechanical_facts_on_the_chat_wire() {
    let temp = TempDir::new("chat-two-part");
    let (root, materials) = home_with_documents(&temp);
    let run = "run-orientation";

    let (endpoint, requests) = spawn_wire_server(chat_finish());
    let ran = Fixture::new(&root, &materials, run, &endpoint, "loopback").run();
    assert_eq!(code(&ran), 0, "{}", stderr(&ran));

    let raw = requests
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("应当收到请求");
    let message = last_user_text(&raw);

    let document = orientation_text();
    let head = document.trim_end();
    assert_eq!(
        message,
        format!("{head}\n{}", expected_facts(run)),
        "最后一条消息必须是「静态头 ✚ 一个换行 ✚ 机械事实」（静态在前、动态在后）"
    );
    assert_eq!(
        raw.matches("这份视野是你自己的身体和家").count(),
        1,
        "静态头在一次请求里只出现一次（不重复渲染）"
    );
    // **手册零注入**：这个环境的手册一个字节都不许进请求（那是自己读的文件）。
    assert!(
        !raw.contains(MANUAL_TEXT.trim()),
        "手册不得进任何注入路径: {}",
        redacted(&raw)
    );
    assert!(
        !raw.contains("想给自己做事的时候"),
        "手册不得进任何注入路径: {}",
        redacted(&raw)
    );
}

#[test]
fn the_orientation_stands_before_the_mechanical_facts_on_the_responses_wire() {
    let temp = TempDir::new("responses-two-part");
    let (root, materials) = home_with_documents(&temp);
    let run = "run-orientation-responses";

    let (endpoint, requests) = spawn_wire_server(responses_finish());
    let ran = Fixture::new(&root, &materials, run, &endpoint, "loopback-responses").run();
    assert_eq!(code(&ran), 0, "{}", stderr(&ran));

    let raw = requests
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("应当收到请求");
    let message = last_user_text(&raw);

    let document = orientation_text();
    let head = document.trim_end();
    assert_eq!(
        message,
        format!("{head}\n{}", expected_facts(run)),
        "第二条 wire 的末条也必须两段式，顺序与 chat 侧逐字节相同"
    );
    assert!(
        !raw.contains("想给自己做事的时候"),
        "手册不得进任何注入路径: {}",
        redacted(&raw)
    );
}

// ---------------------------------------------------------------------------
// 2. 没有 / 空了 / 删了 ⇒ 那一条消息逐字节就是机械事实
// ---------------------------------------------------------------------------

/// **没有那一份**（这个环境是直接建的环境，不带出厂文档）⇒ 与今天逐字节相同。
#[test]
fn without_the_document_the_system_message_is_exactly_the_mechanical_facts() {
    let temp = TempDir::new("without");
    let root = temp.env_root();
    let materials = temp.materials();
    // 直接建环境（不经出厂文档那一步）⇒ 家里没有 orientation.md。
    create_environment(&root, agent_id());
    assert!(
        !root.join(ORIENTATION_FILE).exists(),
        "这一格的前提是没有那一份"
    );
    let run = "run-without";

    let (endpoint, requests) = spawn_wire_server(chat_finish());
    let ran = Fixture::new(&root, &materials, run, &endpoint, "loopback").run();
    assert_eq!(code(&ran), 0, "{}", stderr(&ran));

    let raw = requests
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("应当收到请求");
    assert_eq!(
        last_user_text(&raw),
        expected_facts(run),
        "没有出厂说明 ⇒ 那一条消息**逐字节**就是机械事实（与今天相同）"
    );
    assert!(
        !raw.contains("这份视野是你自己的身体和家"),
        "缺席就是缺席：一点静态头的痕迹都不许有"
    );
}

/// **空了 / 被删了**（主体摘掉说明书，合法）⇒ 同样缺席，而且**不报错**。
#[test]
fn an_empty_or_removed_document_is_a_missing_head_and_not_an_error() {
    let temp = TempDir::new("empty-then-removed");
    let (root, materials) = home_with_documents(&temp);

    // 空了：一个空文件不是说明书。
    std::fs::write(root.join(ORIENTATION_FILE), "  \n").expect("把出厂说明清空");
    let (endpoint, requests) = spawn_wire_server(chat_finish());
    let ran = Fixture::new(&root, &materials, "run-empty", &endpoint, "loopback").run();
    assert_eq!(code(&ran), 0, "空文件不是错误: {}", stderr(&ran));
    let raw = requests
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("应当收到请求");
    assert_eq!(last_user_text(&raw), expected_facts("run-empty"));

    // 删了：Agent 摘了说明书。
    std::fs::remove_file(root.join(ORIENTATION_FILE)).expect("删掉出厂说明");
    let (endpoint, requests) = spawn_wire_server(chat_finish());
    let ran = Fixture::new(&root, &materials, "run-removed", &endpoint, "loopback").run();
    assert_eq!(code(&ran), 0, "文件不在不是错误: {}", stderr(&ran));
    let raw = requests
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("应当收到请求");
    assert_eq!(last_user_text(&raw), expected_facts("run-removed"));
}

/// **读不出来**（IO 错误）⇒ 如实标出一句，**不冒充"Agent 摘了说明书"**，活动照跑。
#[test]
fn an_unreadable_document_is_reported_instead_of_silently_skipped() {
    let temp = TempDir::new("unreadable");
    let (root, materials) = home_with_documents(&temp);
    // 用目录占住那个名字：读它必然失败（与天性那条用例同一个造法）。
    std::fs::remove_file(root.join(ORIENTATION_FILE)).expect("先摘掉那一份");
    std::fs::create_dir(root.join(ORIENTATION_FILE)).expect("用目录占住那个名字");
    let run = "run-unreadable";

    let (endpoint, requests) = spawn_wire_server(chat_finish());
    let ran = Fixture::new(&root, &materials, run, &endpoint, "loopback").run();
    assert_eq!(code(&ran), 0, "读不出来不拦活动: {}", stderr(&ran));

    let raw = requests
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("应当收到请求");
    assert_eq!(
        last_user_text(&raw),
        format!("{ORIENTATION_UNREADABLE}\n{}", expected_facts(run)),
        "读不出来要如实标出（不静默、也不冒充「Agent 摘了说明书」）"
    );
}

// ---------------------------------------------------------------------------
// 3. 开场拍读一次、Run 内携带：Run 里改文件，本次不变，下次才变
// ---------------------------------------------------------------------------

/// 记下**每一次被装订的静态头**的模型替身（替身不发请求 ⇒ 只判装订这件事本身）。
///
/// 前几轮返回纯正文 ⇒ 一次驱动停在**等待**（"Run 中途"），最后一轮收场（活动必须有结局，
/// 否则下一次开场会被"未收尾的旧活动"挡住——那正是准入。）
struct HeadRecorder {
    heads: Vec<Option<String>>,
    views: usize,
    /// 第几轮收场（1 起算）。
    finish_at: usize,
}

impl HeadRecorder {
    fn new(finish_at: usize) -> Self {
        Self {
            heads: Vec::new(),
            views: 0,
            finish_at,
        }
    }
}

impl ModelPort for HeadRecorder {
    fn declare_static_head(&mut self, head: Option<&str>) {
        self.heads.push(head.map(str::to_string));
    }

    fn complete(&mut self, _view: &View) -> Result<ModelOutput, ModelError> {
        self.views += 1;
        let decision = if self.views >= self.finish_at {
            Decision::Finish(SubjectEnding::Completed)
        } else {
            Decision::Yield
        };
        Ok(ModelOutput::new("（只看一眼）", None, decision))
    }
}

/// 用例装配用的固定钟（生产给真实钟，用例给固定钟：同一个装配入口）。
fn fixed_clock() -> ClockHandle {
    ClockHandle::fixed_at_unix_seconds(0)
}

fn session_request<'a>(agent: &'a AgentId, run: &str) -> SessionRequest<'a> {
    SessionRequest {
        declarations: SessionDeclarations::default(),
        declared_agent: agent,
        run: RunId::new(run),
        limits: Limits::new(4),
        input: RunInput::text("出厂说明验收").expect("在上限内"),
        clock: fixed_clock(),
        context_window: 1_000,
        reading: ContextReading::default(),
    }
}

#[test]
fn the_head_is_read_at_the_opening_and_survives_an_edit_inside_the_run() {
    let temp = TempDir::new("opening-read");
    let root = temp.env_root();
    let materials = temp.materials();
    create_environment(&root, agent_id());
    // 开场那一刻环境根里有一份（手工写：这一条只判拍读的时机，不判创建）。
    std::fs::write(root.join(ORIENTATION_FILE), "第一版：开场读到的那一份。\n").expect("写第一版");

    let agent = agent_id();
    // 第一轮停（等待，Run 还在），第二轮收场（活动必须有结局才能开下一次）。
    let mut model = HeadRecorder::new(2);
    {
        let mut environment = open_environment(&root, agent.clone());
        let mut session = RunSession::open(
            &materials,
            &mut environment,
            session_request(&agent, "run-opening"),
        )
        .expect("打开会话");

        assert!(matches!(
            session.drive(&mut model).expect("第一次驱动"),
            SessionDrive::Unsettled
        ));
        assert_eq!(
            model.heads,
            vec![Some("第一版：开场读到的那一份。\n".to_string())],
            "开场拍读一次，驱动前装订给模型"
        );

        // **Run 内改文件**：主体可以改自己家的文件，但这一次活动的请求已经定了。
        std::fs::write(root.join(ORIENTATION_FILE), "第二版：Run 中途改的。\n").expect("写第二版");
        assert!(matches!(
            session.drive(&mut model).expect("第二次驱动"),
            SessionDrive::Ended(_)
        ));
        assert_eq!(
            model.heads,
            vec![
                Some("第一版：开场读到的那一份。\n".to_string()),
                Some("第一版：开场读到的那一份。\n".to_string()),
            ],
            "Run 内改文件**不改**本次活动的静态头（装配拍读一次、Run 内携带）"
        );
    }

    // **下一次活动的开场才变**（上一步的活动已收场、写资格已交回）。
    {
        let mut environment = open_environment(&root, agent.clone());
        let mut next = RunSession::open(
            &materials,
            &mut environment,
            session_request(&agent, "run-next"),
        )
        .expect("打开下一次会话");
        let mut model = HeadRecorder::new(1);
        assert!(matches!(
            next.drive(&mut model).expect("下一次活动的驱动"),
            SessionDrive::Ended(_)
        ));
        assert_eq!(
            model.heads,
            vec![Some("第二版：Run 中途改的。\n".to_string())],
            "下一次清醒的开场重新拍读 ⇒ 换成了新的那一份"
        );
    }
}

/// **缺席也是明确装订一次**：同一台宿主跨活动复用同一个模型，上一个活动的头不许留在那里。
#[test]
fn a_later_run_without_the_document_clears_the_head() {
    let temp = TempDir::new("clears");
    let root = temp.env_root();
    let materials = temp.materials();
    create_environment(&root, agent_id());
    std::fs::write(root.join(ORIENTATION_FILE), "临时写一份。\n").expect("写一份");
    let agent = agent_id();

    // 同一个模型替身跨两个活动用（模拟常驻宿主：模型比会话活得久）。
    let mut model = HeadRecorder::new(1);
    {
        let mut environment = open_environment(&root, agent.clone());
        let mut session = RunSession::open(
            &materials,
            &mut environment,
            session_request(&agent, "run-with-head"),
        )
        .expect("打开会话");
        assert!(matches!(
            session.drive(&mut model).expect("驱动"),
            SessionDrive::Ended(_)
        ));
    }
    // 主体摘掉了那一份，下一个活动照同一个模型跑。
    std::fs::remove_file(root.join(ORIENTATION_FILE)).expect("摘掉那一份");
    {
        let mut environment = open_environment(&root, agent.clone());
        let mut session = RunSession::open(
            &materials,
            &mut environment,
            session_request(&agent, "run-without-head"),
        )
        .expect("打开会话");
        assert!(matches!(
            session.drive(&mut model).expect("驱动"),
            SessionDrive::Ended(_)
        ));
    }

    assert_eq!(
        model.heads,
        vec![Some("临时写一份。\n".to_string()), None],
        "第二个活动明确装订 `None`：缺席 ≠ 沿用上一次的头"
    );
}

// ---------------------------------------------------------------------------
// 4. 上限：整份超过 4 KiB ⇒ 如实标出，不截断、也不拦活动（刀五）
// ---------------------------------------------------------------------------

/// 自己把自己那一份写大了（在自己地盘上怎么写都行）⇒ 静态头**整条不摆**：
/// 那条消息是"如实标出的读数 ＋ 机械事实"，**一个字节正文都没有**（截断比不给更坏）。
#[test]
fn an_oversized_document_is_marked_instead_of_truncated_or_blocking_the_run() {
    let temp = TempDir::new("oversized");
    let (root, materials) = home_with_documents(&temp);
    let oversized = "x".repeat(MAX_ORIENTATION_BYTES + 1);
    std::fs::write(root.join(ORIENTATION_FILE), &oversized).expect("写一份超限的出厂说明");
    let run = "run-oversized";

    let (endpoint, requests) = spawn_wire_server(chat_finish());
    let ran = Fixture::new(&root, &materials, run, &endpoint, "loopback").run();
    assert_eq!(code(&ran), 0, "超限不拦活动: {}", stderr(&ran));

    let raw = requests
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("应当收到请求");
    let message = last_user_text(&raw);
    assert_eq!(
        message,
        format!("{ORIENTATION_TOO_LARGE}\n{}", expected_facts(run)),
        "超限 ⇒ 如实标出的读数 ＋ 机械事实（活动照跑）"
    );
    assert!(
        message.starts_with("orientation-too-large"),
        "稳定分类标记在句首: {message}"
    );
    assert!(
        !raw.contains("xxx"),
        "不截断：正文一个字节都不许进请求（半截说明书比没有更坏）: {}",
        redacted(&raw)
    );
}

// ---------------------------------------------------------------------------
// 5. 新的一段：**重新拍读**（刀五定案为故意），而这一段自己仍"Run 内不变"
// ---------------------------------------------------------------------------

/// **新的一段会重新拍读**——这不是偶然，是 2026-09-23 的定案：出厂说明是**环境家具**（环境根里
/// 的一份文件），不是 Run 状态；Run 状态的家在已确认记录里，而它不在。同族先例是 `reading`：
/// 新进程不知道上一个进程拍到了什么，于是照**当次的真实事实**重新拍。
///
/// 同一条用例的另一半：**这一段自己**照旧"Run 内不变"——不重读盘。
///
/// ⚠ **旧规矩是什么、新规矩是什么**（2026-09-26）。这条用例从前叫
/// `a_cross_process_resume_reads_the_document_again_on_purpose`，载体是**跨进程恢复**
/// （`RunSession::resume`：顶着**同一个 RunId** 接着跑那一段没走完的生命）。
/// **那条路已退役**：承载活动的进程没了，就是两个 Run——那一次连续
/// 就断了；新进程起来是**新的一段**，不是"顶着同一个 RunId 继续"。
/// ⇒ **不变量一个字没变，换的是载体**：新进程 ⇒ **新的一段**（新 RunId、新的
/// [`RunSession::open`]）⇒ 开场照样重新拍读那一份环境家具。因此这条用例留下，只把
/// "恢复同一个 RunId"换成"开新的一段"。
#[test]
fn a_new_segment_reads_the_document_again_on_purpose() {
    let temp = TempDir::new("new-segment-reread");
    let root = temp.env_root();
    let materials = temp.materials();
    create_environment(&root, agent_id());
    std::fs::write(root.join(ORIENTATION_FILE), "第一版：上一段拍到的。\n").expect("写第一版");
    let agent = agent_id();

    // 第一段：开场拍一次、驱动一次**停在等待**（活动未收尾），然后整个进程消失（句柄散掉）。
    {
        let mut environment = open_environment(&root, agent.clone());
        let mut session = RunSession::open(
            &materials,
            &mut environment,
            session_request(&agent, "run-first"),
        )
        .expect("打开会话");
        // 永远只 yield ⇒ 这一段停在半路（正是"进程被切断"留下的形状）。
        let mut model = HeadRecorder::new(usize::MAX);
        assert!(matches!(
            session.drive(&mut model).expect("驱动"),
            SessionDrive::Unsettled
        ));
        assert_eq!(
            model.heads,
            vec![Some("第一版：上一段拍到的。\n".to_string())]
        );
    }

    // 中间自己改了那一份（那是环境根里的文件；改它不需要任何授权）。
    std::fs::write(root.join(ORIENTATION_FILE), "第二版：新的一段读到的。\n").expect("写第二版");

    // **新的一段**：新进程、新的 RunId、新的会话——不是"接着跑上一段"。
    let mut environment = open_environment(&root, agent.clone());
    let mut session = RunSession::open(
        &materials,
        &mut environment,
        session_request(&agent, "run-second"),
    )
    .expect("新的一段可以开始");
    let mut model = HeadRecorder::new(2);
    assert!(matches!(
        session.drive(&mut model).expect("新的一段的第一次驱动"),
        SessionDrive::Unsettled
    ));
    let first = "第二版：新的一段读到的。\n".to_string();
    assert_eq!(
        model.heads,
        vec![Some(first.clone())],
        "新的一段**重新拍读**：装订的是当次读回来的那一份，不是上一段记忆里的"
    );

    // 而这一段**自己**照旧"Run 内不变"：中途改文件不影响这次活动。
    std::fs::write(root.join(ORIENTATION_FILE), "第三版：这一段中途改的。\n").expect("写第三版");
    assert!(matches!(
        session.drive(&mut model).expect("第二次驱动"),
        SessionDrive::Ended(_)
    ));
    assert_eq!(
        model.heads,
        vec![Some(first.clone()), Some(first)],
        "「Run 内不变」以**段**为界：这一段不重读盘"
    );
}

/// 断言失败信息里打印请求时先把凭据遮掉：测试输出不该成为泄漏渠道。
fn redacted(raw: &str) -> String {
    raw.replace(SENTINEL_KEY, "<redacted>")
}
