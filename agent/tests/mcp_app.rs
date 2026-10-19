//! MCP 适配的离线证明（TASK_42；Linux 臂那一刀补平台参数化与真执行用例）。
//!
//! 全部离线、零外网、零 `npx`：
//!
//! - **动词面**（名字、骨架卫生、参数映射、效果档）用**假通道**验证——通道被抽成三个方法的
//!   窄 trait 的全部意义就在这里：墙上每一条都能在没有进程的情况下逐条断言；
//! - **懒展开与真调用**用**真进程**：夹具就是一个最小的 stdio server（`src/bin/` 里的测试件），
//!   它带 `--marker`（被起起来那一刻写一行）与 `--secret`（只有真的调到那条动词才拿得到）。
//! - **平台参数化**（照 `tests/terminal_app.rs` 的形状）：真执行用例门控在
//!   `windows 或 linux`（今天两支臂），别的平台上判的是另一件事——**缝 fail-closed**
//!   （入口拒绝开启、件里不换近似行为）。两平台今天共用同一列夹具调用（它是「可执行件 ＋
//!   参数」）；只有 Linux 那一列有的那几条命令（环境转储、后台孙进程、非 UTF-8 字节）单独
//!   门控在 `target_os = "linux"` 上，并如实登记 Windows 侧未验证。
//!
//! **灵魂判据**是 `the_value_she_answers_can_only_come_from_a_real_call`：那台 server 知道一个
//! 值，主体只能通过一次真实调用拿到它——而那个值在主体的视野里**调用之前一个字节都没有**。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use celira_agent::app_contract::{AppPort, EffectClass, SourceClass};
use celira_agent::core::{
    Action, ActionId, ActionParams, ActionPort, Agent, Limits, ModelError, ModelOutput, ModelPort,
    View, run_activity,
};
use celira_agent::foundation::{
    AgentId, Ledger, OperationId, Outcome, ResultState, RunId, SubjectEnding, Turn,
};
use celira_agent::mcp::{
    CallFault, CallReply, MAX_TOOLS, McpAccess, McpApp, McpDeclaration, McpFailure, RawTool,
    StdioTransport, Transport, parse_declaration,
};
use celira_agent::os::{ExpansionFailure, MinimalOs, RunExpansion};

mod common;

use common::InspectsFirst;

const EXPAND: &str = "open_app";
const INSPECT: &str = "inspect_action";

// ---------------------------------------------------------------------------
// 自建临时沙盒（只用系统临时目录，用完自己删）
// ---------------------------------------------------------------------------

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let unique = format!(
            "celira-mcp-{}-{}-{}",
            std::process::id(),
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("时钟可用")
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&root).expect("可以建临时沙盒");
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// ---------------------------------------------------------------------------
// 假通道：把"对面会说什么"摆成脚本，并记下我们真的发了什么
// ---------------------------------------------------------------------------

/// 假通道的**读数**（测试持有一份，通道写另一份指针）。
#[derive(Clone, Default)]
struct Watch {
    connects: Arc<Mutex<usize>>,
    lists: Arc<Mutex<usize>>,
    calls: Arc<Mutex<Vec<(String, Value, Duration)>>>,
}

impl Watch {
    fn new() -> Self {
        Self::default()
    }

    fn connects(&self) -> usize {
        *self.connects.lock().expect("读数格只被短临界区持有")
    }

    fn lists(&self) -> usize {
        *self.lists.lock().expect("读数格只被短临界区持有")
    }

    fn calls(&self) -> Vec<(String, Value, Duration)> {
        self.calls.lock().expect("读数格只被短临界区持有").clone()
    }
}

/// 一条不起进程的通道。
struct Fake {
    tools: Vec<RawTool>,
    replies: Vec<Result<CallReply, CallFault>>,
    watch: Watch,
}

impl Fake {
    fn new(tools: Vec<RawTool>, replies: Vec<Result<CallReply, CallFault>>) -> (Self, Watch) {
        let watch = Watch::new();
        (
            Self {
                tools,
                replies,
                watch: watch.clone(),
            },
            watch,
        )
    }
}

impl Transport for Fake {
    fn connect(&mut self, _timeout: Duration) -> Result<(), CallFault> {
        *self.watch.connects.lock().expect("短临界区") += 1;
        Ok(())
    }

    fn list_tools(&mut self, _timeout: Duration) -> Result<Vec<RawTool>, CallFault> {
        *self.watch.lists.lock().expect("短临界区") += 1;
        Ok(self.tools.clone())
    }

    fn call_tool(
        &mut self,
        tool: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<CallReply, CallFault> {
        self.watch
            .calls
            .lock()
            .expect("短临界区")
            .push((tool.to_string(), arguments, timeout));
        if self.replies.is_empty() {
            return Err(CallFault::Refused(McpFailure::Transport));
        }
        self.replies.remove(0)
    }
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

fn declaration(name: &str) -> McpDeclaration {
    parse_declaration(&format!("{name}=stdio:fixture-server")).expect("声明可以解析")
}

fn tool(name: &str, description: Option<&str>, schema: Value) -> RawTool {
    RawTool {
        name: name.to_string(),
        description: description.map(str::to_string),
        input_schema: schema,
    }
}

/// 一条不吃参数的动词。
fn plain(name: &str) -> RawTool {
    tool(
        name,
        Some("一句话说明"),
        json!({"type": "object", "properties": {}}),
    )
}

fn schema(properties: Value, required: Value) -> Value {
    json!({"type": "object", "properties": properties, "required": required})
}

fn action(id: &str) -> Action {
    Action::with_params(ActionId::new(id), ActionParams::none())
}

fn with(id: &str, name: &str, value: &str) -> Action {
    Action::with_params(ActionId::new(id), ActionParams::none().with(name, value))
}

fn operation(run: &str) -> OperationId {
    OperationId::new(RunId::new(run), Turn::FIRST)
}

fn receipt(outcome: &celira_agent::core::ActionOutcome) -> String {
    outcome
        .content()
        .first()
        .map(|block| block.text().to_string())
        .unwrap_or_default()
}

fn failure(outcome: &celira_agent::core::ActionOutcome) -> String {
    outcome
        .failure_class()
        .unwrap_or("<no-failure>")
        .to_string()
}

// ---------------------------------------------------------------------------
// 动词面
// ---------------------------------------------------------------------------

/// 一条动词就是 `<server 名>_<对面的名字>`；骨架取对面的说明（过卫生）；效果档整台一个。
#[test]
fn a_verb_is_the_server_prefix_plus_the_tool_name() {
    let (fake, watch) = Fake::new(vec![plain("echo"), plain("where")], vec![]);
    let mut app = McpApp::new(McpAccess::with_transport(
        declaration("lab"),
        Vec::new(),
        Box::new(fake),
    ));

    assert!(
        app.declared_capabilities().is_empty(),
        "装配期**没有**动词：清单要等展开那一下才拿到"
    );
    app.present().expect("展开可以成功");
    assert_eq!(watch.connects(), 0, "假通道没有 connect（本身就在）");
    assert_eq!(watch.lists(), 1, "展开那一下拉一次清单");

    let ids: Vec<String> = app
        .declared_capabilities()
        .items()
        .iter()
        .map(|each| each.id().as_str().to_string())
        .collect();
    assert_eq!(ids, vec!["lab_echo".to_string(), "lab_where".to_string()]);

    let effects = app.declared_effects();
    assert_eq!(effects.len(), 2, "每条动词都有一条效果声明");
    for effect in &effects {
        assert_eq!(
            effect.class(),
            EffectClass::UnverifiableWrite,
            "不给 `,read-only` 就是最严那档"
        );
        assert!(
            effect.verification().is_none(),
            "外来工具的写没有同参复核可配"
        );
    }

    // 只读档：同一份清单，类别跟着宿主那句话走。
    let (fake, _) = Fake::new(vec![plain("echo")], vec![]);
    let mut read_only = McpApp::new(McpAccess::with_transport(
        parse_declaration("lab=stdio:fixture-server,read-only").expect("声明可以解析"),
        Vec::new(),
        Box::new(fake),
    ));
    read_only.present().expect("展开可以成功");
    assert_eq!(
        read_only.declared_effects()[0].class(),
        EffectClass::ReadOnly
    );
}

/// 外来文本不当可信：控制字符压成空格，超长截断**带标记**。
#[test]
fn a_foreign_description_is_sanitized_and_clipped_with_a_marker() {
    let noisy = format!("第一行\n第二行\t带制表{}", "很长".repeat(400));
    let (fake, _) = Fake::new(
        vec![tool("thing", Some(&noisy), json!({"type": "object"}))],
        vec![],
    );
    let mut app = McpApp::new(McpAccess::with_transport(
        declaration("lab"),
        Vec::new(),
        Box::new(fake),
    ));
    app.present().expect("展开可以成功");

    let skeleton = app.declared_capabilities().items()[0]
        .skeleton()
        .to_string();
    assert!(
        !skeleton.contains('\n') && !skeleton.contains('\t'),
        "控制字符要被压掉"
    );
    assert!(skeleton.ends_with("…（截断）"), "截断要显式：{skeleton}");
    assert!(
        skeleton.len() < 300,
        "骨架是每一轮都在的东西，必须小：{}",
        skeleton.len()
    );

    // 对面没给说明时如实说没给（不编一句）。
    let (fake, _) = Fake::new(vec![tool("quiet", None, json!({"type": "object"}))], vec![]);
    let mut app = McpApp::new(McpAccess::with_transport(
        declaration("lab"),
        Vec::new(),
        Box::new(fake),
    ));
    app.present().expect("展开可以成功");
    assert!(
        app.declared_capabilities().items()[0]
            .skeleton()
            .contains("没有给")
    );
}

/// 参数形状我们表达不了：**动词照旧在场**，但调用本地回绝，而且零转发。
///
/// ⚠ **2026-10-13 边界收窄**：从前这一格装的是"对象/数组"——那要求**对面符合我们的形状**，
/// 是拿我们的手型去量她的手。现在对象/数组走 [`JsonKind::Json`]（按一段 JSON 文本搬），
/// **仍然**表达不了的是：`null`、认不出的类型名，以及**两种不同类型拼起来的联合**。
/// 这条用例因此改用联合类型——判据没变，射程窄了。
#[test]
fn a_shape_we_cannot_express_is_visible_but_refuses_locally() {
    let (fake, watch) = Fake::new(
        vec![tool(
            "upload",
            Some("参数是一个两种类型拼起来的联合"),
            schema(
                json!({"paths": {"type": ["string", "integer"]}}),
                json!(["paths"]),
            ),
        )],
        vec![],
    );
    let mut app = McpApp::new(McpAccess::with_transport(
        declaration("lab"),
        Vec::new(),
        Box::new(fake),
    ));
    app.present().expect("展开可以成功");

    let capabilities = app.declared_capabilities();
    let capability = &capabilities.items()[0];
    assert_eq!(capability.id().as_str(), "lab_upload");
    assert!(capability.params().is_empty(), "表达不了就没有参数表");
    assert!(
        capability.details().contains("表达不了"),
        "详情里要说清是哪一格：{}",
        capability.details()
    );

    let outcome = app.execute(&AgentId::new("a"), &operation("r"), &action("lab_upload"));
    assert_eq!(failure(&outcome), "unsupported-schema");
    assert!(watch.calls().is_empty(), "本地回绝 = 零转发");
}

/// **对象与数组现在能用了**：她给一段 JSON 文本，对面收到**真正的**数组/对象。
///
/// 判据（2026-10-13）：Celira 的中立参数容器只装标量，而**对面的参数形状不是我们定的**。
/// 所以这一格不是"我们支持了对象参数"，是「**我们不再要求对面符合我们的形状**」。
#[test]
fn a_structured_parameter_is_expressible_and_reaches_the_server_as_json() {
    let (fake, watch) = Fake::new(
        vec![tool(
            "upload",
            Some("参数里有一个数组和一个对象"),
            schema(
                json!({
                    "paths": {"type": "array", "items": {"type": "string"}},
                    "filters": {"type": "object", "properties": {
                        "status": {"type": "string"},
                        "limit": {"type": "integer"},
                    }},
                }),
                json!(["paths"]),
            ),
        )],
        vec![Ok(CallReply::Replied {
            is_error: false,
            text: "好".to_string(),
            text_bytes: 3,
            truncated: false,
            unsupported: Vec::new(),
            noise: 0,
        })],
    );
    let mut app = McpApp::new(McpAccess::with_transport(
        declaration("lab"),
        Vec::new(),
        Box::new(fake),
    ));
    app.present().expect("展开可以成功");

    // 从前这一格让【整条动词调不了】；现在它照旧在场，而且参数表是满的。
    let capabilities = app.declared_capabilities();
    let capability = &capabilities.items()[0];
    let names: Vec<&str> = capability.params().iter().map(|spec| spec.name()).collect();
    assert!(
        names.contains(&"paths") && names.contains(&"filters"),
        "结构型参数现在有参数表：{names:?}"
    );
    assert_eq!(
        names.len(),
        3,
        "两个声明的参数 ＋ **我们那个超时旋钮**——从前 trouble 让旋钮挂不上，现在挂上了：{names:?}"
    );
    assert!(
        !capability.details().contains("表达不了"),
        "不该再有'表达不了'的读数：{}",
        capability.details()
    );

    // 她给的是**一段 JSON 文本**；对面收到的是**真正的数组/对象**。
    let params = ActionParams::none()
        .with("paths", r#"["a.txt","b.txt"]"#)
        .with("filters", r#"{"status":"open","limit":10}"#);
    let outcome = app.execute(
        &AgentId::new("a"),
        &operation("r"),
        &Action::with_params(ActionId::new("lab_upload"), params),
    );
    assert_eq!(outcome.state(), ResultState::Success);
    let calls = watch.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].1,
        json!({
            "paths": ["a.txt", "b.txt"],
            "filters": {"status": "open", "limit": 10},
        }),
        "对面看到的形状是对的——它不知道我们内部搬的是字符串"
    );

    // 给的不是合法 JSON ⇒ 本地回绝（零转发），与标量那三种同一条路。
    let bad = ActionParams::none().with("paths", "这不是 JSON");
    let outcome = app.execute(
        &AgentId::new("a"),
        &operation("r"),
        &Action::with_params(ActionId::new("lab_upload"), bad),
    );
    assert_eq!(failure(&outcome), "invalid-argument");
    assert_eq!(watch.calls().len(), 1, "本地回绝那一次零转发");
}

/// 结构型参数的**形状说明**：她要照它写那段 JSON，所以键名必须在里面。
///
/// 判据（2026-10-13）：只说"这是一段 JSON 文本"而不说里面有什么键，她**只能猜、只能试**。
/// 而它进的是**详情卡**（按需取），不是骨架（每轮都在）。
#[test]
fn a_structured_parameter_says_what_shape_to_write() {
    let (fake, _) = Fake::new(
        vec![tool(
            "upload",
            Some("对面自己那句话"),
            schema(
                json!({
                    "filters": {
                        "type": "object",
                        "description": "筛选条件那句话",
                        "properties": {
                            "status": {"type": "string"},
                            "limit": {"type": "integer"},
                            "dry": {"type": "boolean"},
                        },
                    },
                    "paths": {"type": "array", "items": {"type": "string"}},
                }),
                json!([]),
            ),
        )],
        vec![],
    );
    let mut app = McpApp::new(McpAccess::with_transport(
        declaration("lab"),
        Vec::new(),
        Box::new(fake),
    ));
    app.present().expect("展开可以成功");

    let capabilities = app.declared_capabilities();
    let capability = &capabilities.items()[0];
    let doc_of = |wanted: &str| -> String {
        capability
            .params()
            .iter()
            .find(|spec| spec.name() == wanted)
            .unwrap_or_else(|| panic!("有 `{wanted}` 那一条"))
            .description()
            .to_string()
    };

    let filters = doc_of("filters");
    for key in ["status", "limit", "dry"] {
        assert!(filters.contains(key), "形状说明里要有键 `{key}`：{filters}");
    }
    assert!(
        filters.contains("筛选条件那句话"),
        "对面自己那句话也要留着（它写在**这个属性**上）：{filters}"
    );

    let paths = doc_of("paths");
    assert!(
        paths.contains("[串"),
        "数组要说清元素类型（紧凑记法 `[串,…]`）：{paths}"
    );

    // 对面那句话很长时：**截断带标记**（静默截断违禁），而且**形状优先保住**。
    let (fake, _) = Fake::new(
        vec![tool(
            "wide",
            None,
            schema(
                json!({"filters": {
                    "type": "object",
                    "description": "很长的一句话，".repeat(30),
                    "properties": {"status": {"type": "string"}},
                }}),
                json!([]),
            ),
        )],
        vec![],
    );
    let mut app = McpApp::new(McpAccess::with_transport(
        declaration("lab"),
        Vec::new(),
        Box::new(fake),
    ));
    app.present().expect("展开可以成功");
    let doc = app.declared_capabilities().items()[0].params()[0]
        .description()
        .to_string();
    assert!(doc.contains("截断"), "超限要带标记：{doc}");
    assert!(
        doc.contains("status"),
        "形状优先——截断砍的是尾巴（对面那句话），不是形状：{doc}"
    );
}

/// **免查**：参数表短 ⇒ 目录行自带参数名；压成一行太长 ⇒ 老实走"先看格式"。
///
/// 判据（2026-10-13）："先看格式"那一步**吃模型能力**——`core.rs` 记着一次真机实测，
/// 模型对参数名**连试三次都错**，三个 Run 都卡在第一轮。而免查的代价是**目录行变长**，
/// 目录行**每一轮都在**前缀里 ⇒ 所以它是**字节预算**，不是"要不要"。
#[test]
fn a_short_parameter_list_earns_the_exemption_and_a_long_one_does_not() {
    let (fake, _) = Fake::new(
        vec![
            tool(
                "short",
                Some("参数少"),
                schema(json!({"a": {"type": "string"}}), json!([])),
            ),
            tool(
                "long",
                Some("对面写了一堆"),
                schema(
                    json!({
                        "a": {"type": "string", "description": "很长的一句话，".repeat(20)},
                        "b": {"type": "string", "description": "很长的一句话，".repeat(20)},
                    }),
                    json!([]),
                ),
            ),
        ],
        vec![],
    );
    let mut app = McpApp::new(McpAccess::with_transport(
        declaration("lab"),
        Vec::new(),
        Box::new(fake),
    ));
    app.present().expect("展开可以成功");

    let capabilities = app.declared_capabilities();
    let readings: Vec<(&str, bool)> = capabilities
        .items()
        .iter()
        .map(|capability| {
            (
                capability.id().as_str(),
                capability.is_exempt_from_inspection(),
            )
        })
        .collect();
    assert!(
        readings.iter().any(|(id, yes)| *id == "lab_short" && *yes),
        "参数少 ⇒ 免查（目录行自带参数名）：{readings:?}"
    );
    assert!(
        readings.iter().any(|(id, yes)| *id == "lab_long" && !*yes),
        "参数表压成一行太长 ⇒ **不**免查（老实走先看格式）：{readings:?}"
    );
}

/// 标量按对面声明的类型还原；还原不出来、参数名不认识、必填没到齐一律**本地回绝**。
#[test]
fn scalars_are_restored_to_the_types_the_server_declared() {
    let (fake, watch) = Fake::new(
        vec![tool(
            "numbers",
            Some("三种标量"),
            schema(
                json!({
                    "count": {"type": "integer"},
                    "ratio": {"type": "number"},
                    "flag": {"type": "boolean"},
                    "label": {"type": "string"},
                }),
                json!(["count"]),
            ),
        )],
        vec![Ok(CallReply::Replied {
            is_error: false,
            text: "好".to_string(),
            text_bytes: 3,
            truncated: false,
            unsupported: Vec::new(),
            noise: 0,
        })],
    );
    let mut app = McpApp::new(McpAccess::with_transport(
        declaration("lab"),
        Vec::new(),
        Box::new(fake),
    ));
    app.present().expect("展开可以成功");

    let params = ActionParams::none()
        .with("count", "7")
        .with("ratio", "0.5")
        .with("flag", "true")
        .with("label", " 留着空白 ");
    let outcome = app.execute(
        &AgentId::new("a"),
        &operation("r"),
        &Action::with_params(ActionId::new("lab_numbers"), params),
    );
    assert_eq!(outcome.state(), ResultState::Success);
    let calls = watch.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "numbers", "转发用的是**对面的名字**");
    assert_eq!(
        calls[0].1,
        json!({"count": 7, "ratio": 0.5, "flag": true, "label": " 留着空白 "}),
        "类型还原成对面声明的；字符串**不 trim**（空白可能是它的内容）"
    );

    // 下面三条都在本地判掉（零转发）。
    for (label, params) in [
        ("整数写不成整数", ActionParams::none().with("count", "七")),
        ("必填没到齐", ActionParams::none().with("label", "x")),
        (
            "参数名不认识",
            ActionParams::none().with("count", "1").with("nope", "x"),
        ),
    ] {
        let outcome = app.execute(
            &AgentId::new("a"),
            &operation("r"),
            &Action::with_params(ActionId::new("lab_numbers"), params),
        );
        assert_eq!(failure(&outcome), "invalid-argument", "{label}");
    }
    assert_eq!(watch.calls().len(), 1, "本地回绝的三次一次都没转发");
}

/// 超时旋钮是我们自己的：它进窗口、**不进**对面的参数；对面占了那个名字就不挂旋钮。
#[test]
fn the_timeout_knob_is_ours_and_never_forwarded() {
    let (fake, watch) = Fake::new(
        vec![
            plain("wait"),
            tool(
                "patience",
                Some("对面自己有一个 timeout"),
                schema(json!({"timeout": {"type": "integer"}}), json!([])),
            ),
        ],
        vec![
            Ok(CallReply::Replied {
                is_error: false,
                text: "好".to_string(),
                text_bytes: 3,
                truncated: false,
                unsupported: Vec::new(),
                noise: 0,
            }),
            Ok(CallReply::Replied {
                is_error: false,
                text: "好".to_string(),
                text_bytes: 3,
                truncated: false,
                unsupported: Vec::new(),
                noise: 0,
            }),
        ],
    );
    let mut app = McpApp::new(McpAccess::with_transport(
        parse_declaration("lab=stdio:fixture-server,timeout=120").expect("声明可以解析"),
        Vec::new(),
        Box::new(fake),
    ));
    app.present().expect("展开可以成功");

    let capabilities = app.declared_capabilities();
    let wait = &capabilities.items()[0];
    assert_eq!(wait.id().as_str(), "lab_wait");
    assert_eq!(
        wait.params().len(),
        1,
        "挂上我们那个旋钮：{:?}",
        wait.params()
    );
    assert_eq!(wait.params()[0].name(), "timeout");

    let outcome = app.execute(
        &AgentId::new("a"),
        &operation("r"),
        &with("lab_wait", "timeout", "7"),
    );
    assert_eq!(outcome.state(), ResultState::Success);
    let calls = watch.calls();
    assert_eq!(calls[0].2, Duration::from_secs(7), "旋钮改的是窗口");
    assert_eq!(calls[0].1, json!({}), "旋钮**不转发**给对面");

    // 对面自己占了 `timeout`：那条动词没有我们的旋钮，那个参数归它。
    let capabilities = app.declared_capabilities();
    let patience = &capabilities.items()[1];
    assert_eq!(patience.id().as_str(), "lab_patience");
    assert_eq!(patience.params().len(), 1);
    assert!(
        patience.details().contains("没有我们的超时旋钮"),
        "详情要说清为什么：{}",
        patience.details()
    );
    let outcome = app.execute(
        &AgentId::new("a"),
        &operation("r"),
        &with("lab_patience", "timeout", "9"),
    );
    assert_eq!(outcome.state(), ResultState::Success);
    let calls = watch.calls();
    assert_eq!(calls[1].1, json!({"timeout": 9}), "对面的同名参数照旧转发");
    assert_eq!(calls[1].2, Duration::from_secs(120), "窗口仍是声明的那个");
}

// ---------------------------------------------------------------------------
// 回执墙与读数
// ---------------------------------------------------------------------------

/// 正文超限：**显式截断**，读数照旧（原文一共多少字节）。
#[test]
fn an_over_long_body_is_truncated_with_an_explicit_marker() {
    let (fake, _) = Fake::new(
        vec![plain("big")],
        vec![Ok(CallReply::Replied {
            is_error: false,
            text: "x".repeat(1024),
            text_bytes: 300 * 1024,
            truncated: true,
            unsupported: Vec::new(),
            noise: 0,
        })],
    );
    let mut app = McpApp::new(McpAccess::with_transport(
        declaration("lab"),
        Vec::new(),
        Box::new(fake),
    ));
    app.present().expect("展开可以成功");

    let outcome = app.execute(&AgentId::new("a"), &operation("r"), &action("lab_big"));
    assert_eq!(outcome.state(), ResultState::Success);
    let text = receipt(&outcome);
    assert!(
        text.contains("text = 307200 bytes, truncated = true"),
        "{text}"
    );
    assert!(text.contains("（此处截断"), "截断必须带明确标记：{text}");
    assert!(text.contains("--- result ---"), "{text}");
}

/// 非文本内容：只报类型与条数、**不透传**；噪声读数也照报。
#[test]
fn non_text_content_is_reported_by_kind_and_never_passed_through() {
    let (fake, _) = Fake::new(
        vec![plain("mixed")],
        vec![Ok(CallReply::Replied {
            is_error: false,
            text: "这里有文本".to_string(),
            text_bytes: 15,
            truncated: false,
            unsupported: vec![("image".to_string(), 2)],
            noise: 3,
        })],
    );
    let mut app = McpApp::new(McpAccess::with_transport(
        declaration("lab"),
        Vec::new(),
        Box::new(fake),
    ));
    app.present().expect("展开可以成功");

    let outcome = app.execute(&AgentId::new("a"), &operation("r"), &action("lab_mixed"));
    let text = receipt(&outcome);
    assert!(text.contains("unsupported-content = image×2"), "{text}");
    assert!(
        text.contains("noise = 3"),
        "噪声要看得见，但不进协议：{text}"
    );
    assert!(text.contains("这里有文本"), "{text}");
    assert!(
        text.contains("scrubbed = n/a"),
        "不起子进程的通道如实说：{text}"
    );
}

/// 对面说"这条没成"（`isError`）是**读数**不是我们的失败分类——转发出去了、答复回来了就是成功。
#[test]
fn iserror_is_a_reading_not_a_failure_class() {
    let (fake, _) = Fake::new(
        vec![plain("fail")],
        vec![Ok(CallReply::Replied {
            is_error: true,
            text: "这条动词自己说它没成：示例原因".to_string(),
            text_bytes: 48,
            truncated: false,
            unsupported: Vec::new(),
            noise: 0,
        })],
    );
    let mut app = McpApp::new(McpAccess::with_transport(
        declaration("lab"),
        Vec::new(),
        Box::new(fake),
    ));
    app.present().expect("展开可以成功");

    let outcome = app.execute(&AgentId::new("a"), &operation("r"), &action("lab_fail"));
    assert_eq!(outcome.state(), ResultState::Success, "答复回来了就是成功");
    let text = receipt(&outcome);
    assert!(text.contains("state = replied"), "{text}");
    assert!(text.contains("is-error = true"), "{text}");
    assert!(text.contains("示例原因"), "对面的话要如实带回来：{text}");
}

/// 对面回**错误对象**：码只用来在我们两格之间选一个，原文只作携带。
#[test]
fn a_server_side_error_object_is_classified_by_code_and_carried_verbatim() {
    let (fake, _) = Fake::new(
        vec![plain("blowup"), plain("badparams")],
        vec![
            Ok(CallReply::Refused {
                code: Some(-32603),
                detail: "code=-32603: 内部错误示例".to_string(),
            }),
            Ok(CallReply::Refused {
                code: Some(-32602),
                detail: "code=-32602: 参数不合法示例".to_string(),
            }),
        ],
    );
    let mut app = McpApp::new(McpAccess::with_transport(
        declaration("lab"),
        Vec::new(),
        Box::new(fake),
    ));
    app.present().expect("展开可以成功");

    let blowup = receipt(&app.execute(&AgentId::new("a"), &operation("r"), &action("lab_blowup")));
    assert!(blowup.contains("state = refused-by-server"), "{blowup}");
    assert!(blowup.contains("error-class = server-error"), "{blowup}");
    assert!(blowup.contains("error-detail = code=-32603"), "{blowup}");

    let bad = receipt(&app.execute(
        &AgentId::new("a"),
        &operation("r"),
        &action("lab_badparams"),
    ));
    assert!(bad.contains("error-class = invalid-argument"), "{bad}");
    assert!(bad.contains("error-detail = code=-32602"), "{bad}");
}

// ---------------------------------------------------------------------------
// 参数**值**出错时带诊断：分类一个词，错在哪一项是旁边的读数
// ---------------------------------------------------------------------------

/// 参数**值**还原不成对面要的类型 ⇒ 回执里说清**哪一项、期望什么形状**。
///
/// 判据与 `os.rs::ParamShape` 治名字那一半**逐字相同**：分类照旧只有 `invalid-argument`
/// 一个词（逐字节稳定），"错在哪"是它**旁边**的结构化读数。
/// 从前四种本地拒绝（不认识的参数名 / 取值还原不成类型 / 必填没到齐 / 同一个参数给了两次）
/// 塌成同一个分类——主体当场不知道自己错在哪一项。
#[test]
fn a_wrong_kind_argument_says_which_one_and_what_was_expected() {
    let sandbox = Sandbox::new("wrong-kind");
    let access = real_access("lab", &sandbox, None, "");
    let mut app = McpApp::new(access);
    app.present().expect("展开可以成功");

    let outcome = app.execute(
        &AgentId::new("a"),
        &operation("r"),
        &Action::with_params(
            ActionId::new("lab_numbers"),
            ActionParams::none()
                .with("count", "abc")
                .with("label", "ok"),
        ),
    );

    // 分类照旧是**那一个词**：值不能揉进分类。
    assert_eq!(failure(&outcome), McpFailure::InvalidArgument.as_str());
    let said = receipt(&outcome);
    assert!(said.contains("parameter = count"), "{said}");
    assert!(said.contains("reason = wrong-kind"), "{said}");
    assert!(
        said.contains("expected = 整数"),
        "要说清期望什么形状（下一步才填得出来）：{said}"
    );
    assert!(
        !said.contains("abc"),
        "**不回显取值**（它可能敏感，而她本来就知道自己给了什么）：{said}"
    );
}

/// 不认识的参数名 ⇒ 说清是哪一个，并列出**这条动词认哪些名字**。
#[test]
fn an_unknown_argument_name_lists_what_the_verb_accepts() {
    let sandbox = Sandbox::new("unknown-name");
    let access = real_access("lab", &sandbox, None, "");
    let mut app = McpApp::new(access);
    app.present().expect("展开可以成功");

    let outcome = app.execute(
        &AgentId::new("a"),
        &operation("r"),
        &Action::with_params(
            ActionId::new("lab_numbers"),
            ActionParams::none().with("count", "1").with("boy", "x"),
        ),
    );

    assert_eq!(failure(&outcome), McpFailure::InvalidArgument.as_str());
    let said = receipt(&outcome);
    assert!(said.contains("parameter = boy"), "{said}");
    assert!(said.contains("reason = unknown-parameter"), "{said}");
    assert!(
        said.contains("count"),
        "接受名单要带上（它就是「该填哪些」的答案）：{said}"
    );
}

/// 必填没到齐 ⇒ 说清缺的是哪一个、它要什么形状。
#[test]
fn a_missing_required_argument_says_which_one_and_what_shape() {
    let sandbox = Sandbox::new("missing-required");
    let access = real_access("lab", &sandbox, None, "");
    let mut app = McpApp::new(access);
    app.present().expect("展开可以成功");

    let outcome = app.execute(
        &AgentId::new("a"),
        &operation("r"),
        &Action::with_params(ActionId::new("lab_numbers"), ActionParams::none()),
    );

    assert_eq!(failure(&outcome), McpFailure::InvalidArgument.as_str());
    let said = receipt(&outcome);
    assert!(said.contains("parameter = count"), "{said}");
    assert!(said.contains("reason = missing-required"), "{said}");
    assert!(said.contains("expected = 整数"), "{said}");
}

// ---------------------------------------------------------------------------
// 词表：同一故障两次，分类字符串逐字节相同
// ---------------------------------------------------------------------------

#[test]
fn the_same_fault_twice_gives_a_byte_identical_class() {
    let (fake, _) = Fake::new(
        vec![plain("echo")],
        vec![
            Err(CallFault::Refused(McpFailure::Transport)),
            Err(CallFault::Refused(McpFailure::Transport)),
        ],
    );
    let mut app = McpApp::new(McpAccess::with_transport(
        declaration("lab"),
        Vec::new(),
        Box::new(fake),
    ));
    app.present().expect("展开可以成功");

    let first = failure(&app.execute(&AgentId::new("a"), &operation("r"), &action("lab_echo")));
    // 第一次故障已经把通道收掉了 ⇒ 第二次是"通道不在"，同一个词。
    let second = failure(&app.execute(&AgentId::new("a"), &operation("r"), &action("lab_echo")));
    assert_eq!(first, "transport");
    assert_eq!(second, "transport");
    assert_eq!(first.as_bytes(), second.as_bytes());
}

/// **效果不明**的分叉：写档落进 `unknown`，只读档落进明确失败。
///
/// ⚠ 这里原来写的是"写档落进 `unknown`（**未决门要接的第三态**）"——**未决效果门已退役**
/// （门是指令，而世界只该给信息）；分叉判据本身一个字没变：它是**声明**出来的效果档在决定
/// "有没有『可能已经发生』这一格"。
#[test]
fn an_indeterminate_fault_splits_by_the_hosts_effect_declaration() {
    let verdict = |suffix: &str| {
        let (fake, _) = Fake::new(
            vec![plain("hangup")],
            vec![
                Err(CallFault::Indeterminate(McpFailure::Transport)),
                Err(CallFault::Indeterminate(McpFailure::Transport)),
            ],
        );
        let mut app = McpApp::new(McpAccess::with_transport(
            parse_declaration(&format!("lab=stdio:fixture-server{suffix}")).expect("声明可以解析"),
            Vec::new(),
            Box::new(fake),
        ));
        app.present().expect("展开可以成功");
        let first = app.execute(&AgentId::new("a"), &operation("r"), &action("lab_hangup"));
        let second = app.execute(&AgentId::new("a"), &operation("r"), &action("lab_hangup"));
        (first.state(), second.state())
    };

    let (write, write_again) = verdict("");
    assert_eq!(
        write,
        ResultState::Unknown,
        "写档：发出去之后通道没了 ⇒ 效果不明（那是「可能已经发生」这一格，不是失败）"
    );
    // 通道被收掉之后**不重连**：第二次是"压根没发出去"⇒ 明确失败，不再是效果不明。
    assert_eq!(
        write_again,
        ResultState::Failure,
        "通道已经收了：这次请求一个字节都没发出去"
    );

    let (read, read_again) = verdict(",read-only");
    assert_eq!(
        read,
        ResultState::Failure,
        "只读档没有「可能已经发生」这一格 ⇒ 明确失败"
    );
    assert_eq!(read_again, ResultState::Failure);
}

// ---------------------------------------------------------------------------
// 展开那一下：清单进归属索引 + 来源表（外部 App 的声明只有这一刻才存在）
// ---------------------------------------------------------------------------

/// 展开之后：那些动作**真的可以路由**，而且**进了 OS 的表**——`lab_push` 从"查不到"变成
/// "查得到"。这一条是"展开那一下真的并了表"的对照。
///
/// ⚠ **旧覆盖 → 新覆盖**（2026-09-26，两刀）——换的是可观察面，不是判据：
///
/// - **旧覆盖证明的是**：展开那一下把外部 server 的**效果声明**并进了 OS 的**效果声明表**
///   （`os.effects()` 的 `is_write` / `class_of`），缺了它 `lab_push` 会被读成只读 ⇒ **fail-open**；
///   再往前它还断言过"写类别在未决时被拒"（分类 `pending-effect-unverified`）。
/// - **新覆盖证明的是**：未决效果门随跨进程恢复退役（门是指令，而世界只该给信息），门读的那张
///   效果声明表也在本刀退役——**门是那张表唯一的生产读者**，门一走，它就只剩"装配期收集、之后
///   无人读"这一半。于是"展开那一下真的并了表"改由**来源表**这一面证明（`os.sources()`，
///   同一台 App、同一次 `absorb`）：它的方向相反——缺一格读成**不知道**（fail-closed），
///   而且外部 App 的清单同样**只在展开那一下**才拿得到，因此下面"展开前 `None` / 展开后 `Some`"
///   这一对就是那条并表路径的证据。**fail-open 那个面不再存在**：没有读者，也就没有"缺一格被
///   读成只读"这回事。
/// - **"类别跟着宿主背书走"**（默认写档 vs `,read-only` 档）由 App 自己那一面证明，见同文件的
///   `a_verb_is_the_server_prefix_plus_the_tool_name`（那里读的是 `declared_effects()`）。
#[test]
fn the_discovered_verbs_join_the_os_tables_after_the_expansion() {
    let build = |suffix: &str| {
        let (fake, _) = Fake::new(vec![plain("push")], vec![]);
        let access = McpAccess::with_transport(
            parse_declaration(&format!("lab=stdio:fixture-server{suffix}")).expect("声明可以解析"),
            Vec::new(),
            Box::new(fake),
        );
        let os = MinimalOs::assemble_preinstalled(vec![Box::new(McpApp::new(access))], &[])
            .expect("可以装配");
        let agent = AgentId::new("agent-a");
        let run = RunId::new("run-1");
        let expansion = RunExpansion::new(agent.clone(), run.clone());
        (os, agent, run, expansion)
    };

    // 默认档：`lab_push` 是写。**装配期它不在表里**——清单要等真的连上才拿得到。
    let (mut os, agent, run, mut expansion) = build("");
    assert_eq!(
        os.sources().class_if_assembled("lab_push"),
        None,
        "装配期还没有这份清单：这时它读成「不知道」，不是任何一档"
    );
    {
        let mut scope = os.scope(&mut expansion);
        let expanded = scope.execute(&agent, &operation("run-1"), &with(EXPAND, "target", "lab"));
        assert_eq!(
            expanded.state(),
            ResultState::Success,
            "{}",
            receipt(&expanded)
        );
    }
    // 展开那一下：交出来的清单**整批并进表**——外部 server 说的字是最严那一档。
    assert_eq!(
        os.sources().class_if_assembled("lab_push"),
        Some(SourceClass::WorldUnsigned),
        "展开之后那张表必须知道它的来源（不然这一格就是丢了一个事实）"
    );
    // TASK_69：调用之前**先看格式**——展开之后新出现的动作不在"在场集合"里，要先看一眼
    // （这是新形态下主体本来就要做的那一轮，不是绕过校验）。
    let mut scope = os.scope(&mut expansion);
    scope.execute(
        &agent,
        &operation("run-1"),
        &with(INSPECT, "action", "lab_push"),
    );
    assert!(
        matches!(
            scope.admit(&agent, &run, &action("lab_push")),
            celira_agent::core::ActionAdmission::Admitted
        ),
        "门退役之后写类别照常准入"
    );

    // 只读档：同一份清单，但类别跟着宿主的背书走（App 那一面见本文件上面那条用例）。
    let (mut os, agent, run, mut expansion) = build(",read-only");
    {
        let mut scope = os.scope(&mut expansion);
        let expanded = scope.execute(&agent, &operation("run-1"), &with(EXPAND, "target", "lab"));
        assert_eq!(
            expanded.state(),
            ResultState::Success,
            "{}",
            receipt(&expanded)
        );
    }
    assert_eq!(
        os.sources().class_if_assembled("lab_push"),
        Some(SourceClass::WorldUnsigned),
        "只读背书改的是**效果**档，不是来源档：来源照旧是最严那一档"
    );
    // TASK_69：同一份清单、另一次装配 ⇒ 先看格式那一轮要重来（在场集合是活动作用域自己的）。
    let mut scope = os.scope(&mut expansion);
    scope.execute(
        &agent,
        &operation("run-1"),
        &with(INSPECT, "action", "lab_push"),
    );
    assert!(matches!(
        scope.admit(&agent, &run, &action("lab_push")),
        celira_agent::core::ActionAdmission::Admitted
    ));
}

/// 展开那一下发现的撞名：**整台不上场**，而且一格都不落。
#[test]
fn a_discovered_name_that_would_shadow_an_os_verb_refuses_the_whole_server() {
    let (fake, _) = Fake::new(vec![plain("action")], vec![]);
    let access = McpAccess::with_transport(
        // `<server>_<tool>` = `inspect_action`：OS 自己的动词，谁都不许占。
        declaration("inspect"),
        Vec::new(),
        Box::new(fake),
    );
    let mut os = MinimalOs::assemble_preinstalled(vec![Box::new(McpApp::new(access))], &[])
        .expect("装配期还看不出来（清单要连上才有）");
    let agent = AgentId::new("agent-a");
    let run = RunId::new("run-1");
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = os.scope(&mut expansion);

    let outcome = scope.execute(
        &agent,
        &operation("run-1"),
        &with(EXPAND, "target", "inspect"),
    );
    assert_eq!(failure(&outcome), "conflicting-action-id");
    assert!(
        !scope.is_expanded("inspect"),
        "没并进索引就算没展开过（不留半截注册）"
    );
    assert!(
        scope
            .capabilities(&agent, &run)
            .find(&ActionId::new("inspect_action"))
            .is_some(),
        "OS 自己的动词照旧在场"
    );
}

// ---------------------------------------------------------------------------
// 真进程：懒展开 + 灵魂判据
// ---------------------------------------------------------------------------

fn fixture() -> &'static str {
    env!("CARGO_BIN_EXE_mcp_fixture_server")
}

fn real_access(name: &str, sandbox: &Sandbox, secret: Option<&str>, suffix: &str) -> McpAccess {
    let marker = sandbox.path().join("child-started.txt");
    let mut text = format!("{name}=stdio:{} --marker {}", fixture(), marker.display());
    if let Some(secret) = secret {
        text.push_str(&format!(" --secret {secret}"));
    }
    text.push_str(suffix);
    McpAccess::new(parse_declaration(&text).expect("声明可以解析"), Vec::new())
}

/// **懒展开**：声明 `--mcp` 不等于起了子进程——主体不动那台 server，它就一个字节都不花。
#[test]
fn the_child_is_not_started_until_she_opens_the_app() {
    let sandbox = Sandbox::new("lazy");
    let marker = sandbox.path().join("child-started.txt");
    let access = real_access("lab", &sandbox, None, "");
    let mut os = MinimalOs::assemble_preinstalled(vec![Box::new(McpApp::new(access))], &[])
        .expect("可以装配");
    let agent = AgentId::new("agent-a");
    let run = RunId::new("run-1");
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    {
        let mut scope = os.scope(&mut expansion);
        let available = scope.capabilities(&agent, &run);
        assert!(
            available.find(&ActionId::new("lab_echo")).is_none(),
            "没展开：Agent 的能力面里没有那台 server 的动词"
        );
        assert!(
            available
                .find(&ActionId::new(EXPAND))
                .expect("展开在场")
                .details()
                .contains("lab"),
            "入口里要说清装了哪一台"
        );
        assert!(!marker.exists(), "只是声明了，子进程不该起");

        // 主体什么都不做：整场活动不碰那台 server。
        let mut model = ScriptedModel {
            script: vec![ModelOutput::finish(SubjectEnding::Silent)],
        };
        let mut state = Agent::new(agent.clone());
        let mut ledger = Ledger::new(agent.clone());
        run_activity(
            &mut state,
            run.clone(),
            Limits::new(2),
            &mut model,
            &mut scope,
            &mut ledger,
        )
        .expect("活动可以跑完");
    }

    assert!(!marker.exists(), "一次都没展开 ⇒ 子进程一次都没起");
}

/// 未声明的名字与未展开的动词都路由不到。
#[test]
fn an_undeclared_server_and_an_unopened_verb_are_both_refused() {
    let sandbox = Sandbox::new("refused");
    let access = real_access("lab", &sandbox, None, "");
    let mut os = MinimalOs::assemble_preinstalled(vec![Box::new(McpApp::new(access))], &[])
        .expect("可以装配");
    let agent = AgentId::new("agent-a");
    let run = RunId::new("run-1");
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = os.scope(&mut expansion);

    let undeclared = scope.execute(
        &agent,
        &operation("run-1"),
        &with(EXPAND, "target", "not-declared"),
    );
    assert_eq!(
        failure(&undeclared),
        ExpansionFailure::UnknownTarget.as_str()
    );

    let unopened = scope.execute(&agent, &operation("run-1"), &action("lab_echo"));
    assert_eq!(failure(&unopened), ExpansionFailure::UnknownAction.as_str());
}

/// 按脚本作答的模型（不读视野）。
struct ScriptedModel {
    script: Vec<ModelOutput>,
}

impl ModelPort for ScriptedModel {
    fn complete(&mut self, _view: &View) -> Result<ModelOutput, ModelError> {
        Ok(self.script.remove(0))
    }
}

/// **灵魂判据**：那台 server 知道一个值，主体只能通过一次真实调用拿到它。
///
/// 三件事一起证明它是"真调用"：
/// 1. 那个值在**调用之前**一个字节都没进过主体的视野（含处境注入与能力面）；
/// 2. 主体读到的值**就是**声明里那一个（独立核对）；
/// 3. 子进程**真的起过**（标记文件），而且只在主体展开那一下之后。
struct SelfConceptModel {
    secret: String,
    called: bool,
    answered: String,
    saw_secret_too_early: bool,
    calls: usize,
}

impl SelfConceptModel {
    fn new(secret: &str) -> Self {
        Self {
            secret: secret.to_string(),
            called: false,
            answered: String::new(),
            saw_secret_too_early: false,
            calls: 0,
        }
    }

    /// 主体眼前的一切（注入、能力、经历）——用一条最短的机械投影。
    fn everything_in_sight(view: &View) -> String {
        let mut text = format!("{:?}", view.capabilities());
        for entry in view.trace() {
            text.push_str(&format!("{entry:?}"));
        }
        text
    }
}

impl ModelPort for SelfConceptModel {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        self.calls += 1;
        if !self.called && Self::everything_in_sight(view).contains(&self.secret) {
            self.saw_secret_too_early = true;
        }

        // 展开那台 server：它的动词在此之前**不在**Agent 的能力面里。
        if view
            .capabilities()
            .find(&ActionId::new("lab_reveal"))
            .is_none()
        {
            return Ok(ModelOutput::act(with(EXPAND, "target", "lab")));
        }
        // 先看一眼详情卡片（详情是尾部那一半，走 OS 的元工具）。
        if view
            .observed()
            .iter()
            .all(|observation| observation.action().as_str() != INSPECT)
        {
            return Ok(ModelOutput::act(with(INSPECT, "action", "lab_reveal")));
        }
        // 真的调一次。
        if !self.called {
            self.called = true;
            return Ok(ModelOutput::act(action("lab_reveal")));
        }
        // 把读到的东西记下来（它只能来自那次调用）。
        self.answered = view
            .observed()
            .iter()
            .rev()
            .find(|observation| observation.action().as_str() == "lab_reveal")
            .map(|observation| receipt(observation.outcome()))
            .unwrap_or_default();
        Ok(ModelOutput::finish(SubjectEnding::Completed))
    }
}

/// **灵魂判据**（门控在 `windows 或 linux`：它要真的起进程，今天两支臂；别的平台上判的是
/// 缝 fail-closed，见本文件末尾的缝用例）。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn the_value_she_answers_can_only_come_from_a_real_call() {
    let sandbox = Sandbox::new("self_concept");
    let marker = sandbox.path().join("child-started.txt");
    // 只有那台 server 知道的一个值：它不在主体的任何别处（不是文件、不是记忆、不是配置）。
    let secret = format!(
        "nonce-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("时钟可用")
            .as_nanos()
    );
    let access = real_access("lab", &sandbox, Some(&secret), "");
    let mut os = MinimalOs::assemble_preinstalled(vec![Box::new(McpApp::new(access))], &[])
        .expect("可以装配");
    assert!(!marker.exists(), "声明 ≠ 起进程");

    let agent = AgentId::new("agent-a");
    let run = RunId::new("run-1");
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = os.scope(&mut expansion);
    let mut model = InspectsFirst::new(SelfConceptModel::new(&secret));
    let mut state = Agent::new(agent.clone());
    let mut ledger = Ledger::new(agent.clone());
    let run = run_activity(
        &mut state,
        run.clone(),
        Limits::new(12),
        &mut model,
        &mut scope,
        &mut ledger,
    )
    .expect("活动可以跑完");

    assert_eq!(
        run.outcome(),
        Some(Outcome::Subject(SubjectEnding::Completed)),
        "主体应当能走完这一趟"
    );
    assert!(
        !model.inner().saw_secret_too_early,
        "那个值在调用之前不该出现在主体的视野里"
    );
    assert!(
        model.inner().answered.contains(&secret),
        "主体读到的必须是那台 server 知道的那个值：{}",
        model.inner().answered
    );
    assert!(marker.exists(), "展开那一下之后子进程真的起过");

    // 独立核对：那个值**只**从那一条动词的观察里进过主体的视野。
    let carried = run
        .observed()
        .iter()
        .filter(|observation| receipt(observation.outcome()).contains(&secret))
        .map(|observation| observation.action().as_str().to_string())
        .collect::<Vec<String>>();
    assert_eq!(carried, vec!["lab_reveal".to_string()]);
    assert!(model.calls() >= 4, "展开 → 看详情 → 调用 → 收场，至少四轮");
}

/// 对照拍：**没声明**那台 server 时，同一个问题拿不到那个值（那它就不在主体的世界里）。
#[test]
fn without_the_declaration_the_value_is_out_of_her_world() {
    let secret = format!("nonce-unreachable-{}", std::process::id());
    let sandbox = Sandbox::new("control");
    // 只装配文件视图（没有任何外部来源的 App）：`open_app("lab")` 必然被拒。
    let mut os = MinimalOs::open(sandbox.path()).expect("可以装配");
    let agent = AgentId::new("agent-a");
    let run_id = RunId::new("run-1");
    let mut expansion = RunExpansion::new(agent.clone(), run_id.clone());
    let mut scope = os.scope(&mut expansion);

    let refused = scope.execute(&agent, &operation("run-1"), &with(EXPAND, "target", "lab"));
    assert_eq!(failure(&refused), ExpansionFailure::UnknownTarget.as_str());

    let mut model = ScriptedModel {
        script: vec![ModelOutput::finish(SubjectEnding::Silent)],
    };
    let mut state = Agent::new(agent.clone());
    let mut ledger = Ledger::new(agent.clone());
    let run = run_activity(
        &mut state,
        run_id.clone(),
        Limits::new(2),
        &mut model,
        &mut scope,
        &mut ledger,
    )
    .expect("活动可以跑完");

    let everything = format!("{:?}{:?}", scope.capabilities(&agent, &run_id), run.trace());
    assert!(
        !everything.contains(&secret),
        "没装那台 server，那个值就不可能出现在主体的世界里"
    );
}

// ---------------------------------------------------------------------------
// 平台缝：本构建里有没有臂 / 别的平台上 fail-closed
// ---------------------------------------------------------------------------

/// **本构建里有臂的平台上**，这一档必须可用（今天两支臂：Windows、Linux）。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn the_platform_has_the_stdio_channel_where_an_arm_exists() {
    assert!(
        celira_agent::mcp::stdio_available(),
        "本构建里编进了这一档的臂（Windows 或 Linux）"
    );
}

/// 没有臂的平台：入口会拒绝开启 `--mcp`（`stdio_available` 为假），件里也不换近似行为。
#[cfg(not(any(windows, target_os = "linux")))]
#[test]
fn the_platform_seam_is_fail_closed_elsewhere() {
    assert!(
        !celira_agent::mcp::stdio_available(),
        "没有臂的平台必须 fail-closed"
    );
    let outcome = StdioTransport::spawn(&declaration("lab"), &[]);
    assert!(
        matches!(outcome, Err(McpFailure::SpawnFailure)),
        "没有实现就不许起进程、也不许换近似行为"
    );
}

// ---------------------------------------------------------------------------
// 真进程走整条通道（平台参数化：今天两支臂共用同一列夹具调用）
// ---------------------------------------------------------------------------

/// 起夹具 server 的那条命令（两平台同形：可执行件 ＋ 参数；词法交给各自的臂）。
#[cfg(any(windows, target_os = "linux"))]
fn fixture_command(marker: &Path, secret: Option<&str>) -> String {
    let mut text = format!("{} --marker {}", fixture(), marker.display());
    if let Some(secret) = secret {
        text.push_str(&format!(" --secret {secret}"));
    }
    text
}

/// 起夹具 server、并点名它的**协议世代**（给"探测对面是哪一代"当演员）。
#[cfg(any(windows, target_os = "linux"))]
fn fixture_command_with_era(marker: &Path, era: &str) -> String {
    format!("{} --marker {} --era {era}", fixture(), marker.display())
}

/// **对面只会说新版协议** ⇒ celira 改用**现代框子接着说**（不是报错）。
///
/// 判据（2026-10-13）：celira 是**纯消费者**——只会说一门话就会丢掉另一门那边的所有 server。
/// ⇒ 这里是"**多会一门**"，**不是升级**（`2024-11-05` 那条路一个字没动）。
///
/// ⚠ 夹具在 modern 档会**拒不带 `_meta` 的 `tools/list`**（回 `-32602`）⇒ "清单拉得到"
/// 这一件事就**证明**我们真的说了现代话，而不是靠"没报错"倒推。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_modern_only_server_is_spoken_to_in_its_own_era() {
    let sandbox = Sandbox::new("modern-only");
    let marker = sandbox.path().join("child-started.txt");
    let command = fixture_command_with_era(&marker, "modern-only");
    let declaration = parse_declaration(&format!("lab=stdio:{command}")).expect("声明可以解析");

    let mut transport = StdioTransport::spawn(&declaration, &[]).expect("通道可以起");
    transport
        .connect(Duration::from_secs(20))
        .expect("它会说新版 ⇒ 接得上（改用现代框子；不发 legacy 那个收尾通知）");
    let tools = transport
        .list_tools(Duration::from_secs(20))
        .expect("清单拉得到——夹具拒不带 `_meta` 的请求，所以这一条证明了我们**真的发了** `_meta`");
    assert!(
        tools.iter().any(|tool| tool.name == "reveal"),
        "清单里应当有 reveal"
    );
}

/// **对面是 modern，但它也说我们会的那一版** ⇒ 照样接得上（两边说得到一起）。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_modern_server_that_also_speaks_our_version_still_connects() {
    let sandbox = Sandbox::new("modern-and-ours");
    let marker = sandbox.path().join("child-started.txt");
    let command = fixture_command_with_era(&marker, "modern-and-ours");
    let declaration = parse_declaration(&format!("lab=stdio:{command}")).expect("声明可以解析");

    let mut transport = StdioTransport::spawn(&declaration, &[]).expect("通道可以起");
    transport
        .connect(Duration::from_secs(20))
        .expect("说得到一起 ⇒ 接得上");
    transport
        .list_tools(Duration::from_secs(20))
        .expect("清单拉得到");
}

/// **对面是 modern，但它说的版本我们一个都不会** ⇒ **只有这一格**才叫"版本不支持"。
///
/// 它是上面两条的**反例守卫**：没有它，"探出 modern 就一律报 `unsupported-version`"
/// 那种偷懒写法也能全绿——而那种写法会把**接得上的** server 说成接不上（记录撒谎）。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_modern_server_that_speaks_none_of_our_versions_is_named_unsupported() {
    let sandbox = Sandbox::new("modern-other");
    let marker = sandbox.path().join("child-started.txt");
    let command = fixture_command_with_era(&marker, "modern-other");
    let declaration = parse_declaration(&format!("lab=stdio:{command}")).expect("声明可以解析");

    let mut transport = StdioTransport::spawn(&declaration, &[]).expect("通道可以起");
    let fault = transport
        .connect(Duration::from_secs(20))
        .expect_err("它说的版本我们一个都不会");
    assert_eq!(
        fault.class(),
        McpFailure::UnsupportedVersion,
        "要**点名**是哪一格：{fault:?}"
    );
    assert_eq!(fault.class().as_str(), "unsupported-version");
}

/// **拒握手时对面那句话要留下来**——展开失败的回执里，分类与原文**各占一格**。
///
/// 判据不是"信任纪律"，是**记录诚实**（`outputs/IRREPLACEABLE_RECORD_RULING_DRAFT_2026-10-13.md` §3）：
/// 外来的话若不加标明地进记录，它看起来就像我们自己的读数。
///
/// ⚠ 它钉的是**这一格曾经整个丢掉的那句话**：从前 `connect` 把 `Reply::Refused(_)` 的 payload
/// 直接丢弃，记录里只剩一个分类——**对面为什么拒**因此永远读不回来。
/// 同一条解法在"调用"那条路径上早就有了（`error-detail`），这一刀只是把它补到展开这条路径上。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_refused_handshake_keeps_what_the_server_said() {
    let sandbox = Sandbox::new("refusal-detail");
    let command =
        fixture_command_with_era(&sandbox.path().join("child-started.txt"), "modern-other");
    let access = McpAccess::new(
        parse_declaration(&format!("lab=stdio:{command}")).expect("声明可以解析"),
        Vec::new(),
    );
    let mut os = MinimalOs::assemble_preinstalled(vec![Box::new(McpApp::new(access))], &[])
        .expect("可以装配");
    let agent = AgentId::new("agent-a");
    let run = RunId::new("run-1");
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = os.scope(&mut expansion);

    let outcome = scope.execute(&agent, &operation("run-1"), &with(EXPAND, "target", "lab"));

    // 分类照旧是**那一个词**：它可能落盘，因此逐字节稳定，对面的话一个字都不许揉进来。
    assert_eq!(failure(&outcome), McpFailure::UnsupportedVersion.as_str());
    // 对面那句话**有落脚处**：它是旁边那一块内容，不是分类的一部分。
    let carried = receipt(&outcome);
    assert!(
        carried.contains("fixture: 不支持这个协议版本"),
        "对面自己那句话必须留下来：{carried}"
    );
    assert!(
        carried.contains("-32022"),
        "错误码也在 `describe_error` 的原文里（它是对面说了什么的一部分）：{carried}"
    );
    assert!(
        !scope.is_expanded("lab"),
        "握手没成就不算展开过（回到未展开的样子）"
    );
}

/// **对面拒了但一个字的 `message` 都没给** ⇒ 回执里**只报码**，不许编话、也不许把结构摊成文本。
///
/// 它是上面那条的**负向守卫**：没有它，"失败就一律贴一块内容"或"没话就把整个 error 对象
/// JSON 化"那两种写法也能全绿——而它们都会让记录**替对面说话**。
///
/// 判据两条：
/// 1. **「没有」与「读不到」分得开**：缺 `message` ⇒ 如实只给 `code=<码>`（那是对面真给的事实），
///    而不是空块、也不是编一句话；
/// 2. **结构不进"原文"那一格**：对面给的 `data`（这里是 `supported` 版本表）**一个字节都不许**
///    出现在这句话里——它是结构，该有自己的格子；摊平成文本就是让"携带原文"的格子去承载结构。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_refusal_without_a_message_carries_only_the_code() {
    let sandbox = Sandbox::new("refusal-silent");
    let command = fixture_command_with_era(
        &sandbox.path().join("child-started.txt"),
        "modern-other-silent",
    );
    let access = McpAccess::new(
        parse_declaration(&format!("lab=stdio:{command}")).expect("声明可以解析"),
        Vec::new(),
    );
    let mut os = MinimalOs::assemble_preinstalled(vec![Box::new(McpApp::new(access))], &[])
        .expect("可以装配");
    let agent = AgentId::new("agent-a");
    let run = RunId::new("run-1");
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = os.scope(&mut expansion);

    let outcome = scope.execute(&agent, &operation("run-1"), &with(EXPAND, "target", "lab"));

    // 分类照旧在（拒了就是拒了）。
    assert_eq!(failure(&outcome), McpFailure::UnsupportedVersion.as_str());
    // 只报码：对面没留话，我们也不替它说话。
    let carried = receipt(&outcome);
    assert_eq!(
        carried, "code=-32022",
        "缺 message ⇒ 如实只给码，不编话、不摊结构"
    );
    assert!(
        !carried.contains("supported") && !carried.contains("data"),
        "对面的 `data` 是结构，不许摊进原文那一格：{carried}"
    );
    assert!(
        !scope.is_expanded("lab"),
        "握手没成就不算展开过（回到未展开的样子）"
    );
}

/// **对面回 `input_required`（MRTR）** ⇒ 读成**如实的拒绝**，不许读成成功。
///
/// 判据（丙1，2026-10-13 裁）：本版**不实现** MRTR——那要引入"回头再问一次"的**执行语义**，
/// 不是 `mcp/` 一层能定的。⇒ 但**不许**把"对面没说完"读成"对面说完了"。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn an_input_required_reply_is_a_refusal_not_a_success() {
    let sandbox = Sandbox::new("input-required");
    let marker = sandbox.path().join("child-started.txt");
    let command = format!(
        "{} --marker {} --input-required",
        fixture(),
        marker.display()
    );
    let declaration = parse_declaration(&format!("lab=stdio:{command}")).expect("声明可以解析");

    let mut transport = StdioTransport::spawn(&declaration, &[]).expect("通道可以起");
    transport
        .connect(Duration::from_secs(20))
        .expect("握手可以成");
    let reply = transport
        .call_tool("reveal", json!({}), Duration::from_secs(20))
        .expect("这一次交换本身完成了");
    match reply {
        CallReply::Refused { detail, .. } => {
            assert!(detail.contains("input_required"), "说清是哪一格：{detail}");
        }
        _ => panic!("不许读成成功——那是把「对面没说完」读成「对面说完了」"),
    }
}

/// 真进程走完整条通道：握手 → 拉清单 → 调一条动词；回执来自**真的那次调用**。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_real_server_answers_a_real_call_over_the_channel() {
    let sandbox = Sandbox::new("roundtrip");
    let marker = sandbox.path().join("child-started.txt");
    let secret = format!("nonce-channel-{}", std::process::id());
    let command = fixture_command(&marker, Some(&secret));
    let declaration = parse_declaration(&format!("lab=stdio:{command}")).expect("声明可以解析");

    let mut transport = StdioTransport::spawn(&declaration, &[]).expect("有臂的平台：通道可以起");
    transport
        .connect(Duration::from_secs(20))
        .expect("握手可以成");
    let tools = transport
        .list_tools(Duration::from_secs(20))
        .expect("清单可以拉");
    assert!(
        tools.iter().any(|tool| tool.name == "reveal"),
        "清单里应当有 reveal"
    );
    let reply = transport
        .call_tool("reveal", json!({}), Duration::from_secs(20))
        .expect("调用可以成");
    match reply {
        CallReply::Replied { is_error, text, .. } => {
            assert!(!is_error);
            assert!(
                text.contains(&secret),
                "回执里必须是那台 server 知道的那个值（真调用的证据）：{text}"
            );
        }
        other => panic!("预期一条答复：{other:?}"),
    }
    assert!(marker.exists(), "通道真的起了子进程");
}

/// 真 server 的清单过一遍**机械检查**（"整台或全无"的那一套判据，在真清单上逐条过）。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn the_real_catalogue_passes_the_mechanical_checks() {
    let sandbox = Sandbox::new("catalogue");
    let marker = sandbox.path().join("child-started.txt");
    let command = fixture_command(&marker, None);
    let declaration = parse_declaration(&format!("lab=stdio:{command}")).expect("声明可以解析");
    let mut transport = StdioTransport::spawn(&declaration, &[]).expect("通道可以起");
    transport
        .connect(Duration::from_secs(20))
        .expect("握手可以成");
    let tools = transport
        .list_tools(Duration::from_secs(20))
        .expect("清单可以拉");

    assert!(!tools.is_empty(), "清单不许是空的");
    assert!(tools.len() <= MAX_TOOLS, "条数不超上限：{}", tools.len());
    let mut names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
    let mut unique = names.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), names.len(), "同一个动词名不许出现两次");
    for tool in &tools {
        assert!(!tool.name.is_empty(), "名字必须有");
        assert!(
            !tool
                .name
                .chars()
                .any(|character| character.is_whitespace() || character.is_control()),
            "名字里不许带空白或控制字符：{:?}",
            tool.name
        );
        assert!(
            "lab".len() + 1 + tool.name.len() <= celira_agent::provider_adapter::TOOL_NAME_MAX,
            "lab_{} 放不下工具名上限",
            tool.name
        );
    }
    // 这台夹具的动词面是**它自己说的名字**：逐条对上（加一条动词就该被人看见）。
    names.sort_unstable();
    let expected = [
        "badparams",
        "big",
        "blowup",
        "echo",
        "fail",
        "hangup",
        "mixed",
        "nested",
        "noisy",
        "numbers",
        "reveal",
        "slow",
        "where",
    ];
    assert_eq!(names, expected, "夹具的清单与预期逐条对上");
}

/// 转发窗口到点：**先收树、再判定**——收干净了（击杀报成功 ＋ 窗口内等到它消失）才敢说
/// `timeout`；通道从此不可再用（下一次是"确定没发出去"）。
///
/// ⚠ **这一条的读数由构造保证**（2026-10-09）：判的是"收干净了才是确定的那一格"，而收树那一手
/// 的成败**不在我们手里**（`taskkill` 没找到进程、或它自己起不来，都返回 false ⇒ 实现如实读成
/// 保守的 `Indeterminate(Timeout)`，于是用例偶发假红——§4 登记过）。所以：**窗口**给 30 秒
/// （生产缺省 10 秒）、**那一手的答案**由用例给确定值——⚠ 注入的是**读数不是行为**：真进程照旧
/// 由平台臂收掉，"窗口内等到它消失"那一半照旧量真的。规则本身（两个输入四种组合）另有一条
/// 与进程无关的单元用例钉着（`src/mcp/stdio.rs`）。
#[cfg(any(windows, target_os = "linux"))]
#[test]
fn a_slow_call_times_out_only_after_the_tree_is_collected() {
    let sandbox = Sandbox::new("slow");
    let marker = sandbox.path().join("child-started.txt");
    let command = fixture_command(&marker, None);
    let declaration = parse_declaration(&format!("lab=stdio:{command}")).expect("声明可以解析");
    let mut transport = StdioTransport::spawn(&declaration, &[])
        .expect("通道可以起")
        .with_kill_window(Duration::from_secs(30))
        .with_kill_answer(true);
    transport
        .connect(Duration::from_secs(20))
        .expect("握手可以成");

    let fault = transport.call_tool("slow", json!({"seconds": 30}), Duration::from_secs(1));
    assert!(
        matches!(fault, Err(CallFault::Refused(McpFailure::Timeout))),
        "收干净了才是确定的那一格：{fault:?}"
    );
    // 收掉之后不重连：这一次请求一个字节都没发出去（确定）。
    let again = transport.call_tool("echo", json!({"text": "x"}), Duration::from_secs(5));
    assert!(
        matches!(again, Err(CallFault::Refused(McpFailure::Transport))),
        "{again:?}"
    );
}

// ---------------------------------------------------------------------------
// Linux 臂的真执行（只有那一列才有的命令；Windows 侧未验证，如实登记）
// ---------------------------------------------------------------------------

/// **进程组击杀的证据**：超时之后**孙进程也死**。
///
/// 声明命令自己先起一个后台子 shell（孙进程），它两秒后落记号，随后 `exec` 夹具——于是夹具
/// 与那个后台子 shell 同属一个进程组（臂给子进程 `process_group(0)`）；`timeout = 1` 到点
/// 按组发 `SIGKILL`。若只杀了直接子进程，记号会在两秒时出现。
#[cfg(target_os = "linux")]
#[test]
fn a_timeout_kills_the_grandchildren_too() {
    let sandbox = Sandbox::new("grandchild");
    let marker = sandbox.path().join("grandchild-done.txt");
    let command = format!(
        "( sleep 2; touch '{}' ) & exec {}",
        marker.display(),
        fixture()
    );
    let declaration = parse_declaration(&format!("lab=stdio:{command}")).expect("声明可以解析");
    let mut transport = StdioTransport::spawn(&declaration, &[]).expect("通道可以起");
    transport
        .connect(Duration::from_secs(20))
        .expect("握手可以成");

    let fault = transport.call_tool("slow", json!({"seconds": 30}), Duration::from_secs(1));
    assert!(
        matches!(fault, Err(CallFault::Refused(McpFailure::Timeout))),
        "{fault:?}"
    );
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        !marker.exists(),
        "被按组击杀的孙进程不该再往下走一步：{}",
        marker.display()
    );
}

/// **凭据洗**（真进程 ＋ **正对照**）：测试进程环境里放两个哨兵——一个 `CELIRA_` 前缀
/// （系统自有前缀那族），一个**宿主点名**的键名；声明命令先把**交出去的那份环境**的键名
/// 转储到文件、再 `exec` 夹具。读数与转储两侧都判：洗掉几个如实报、被洗的键名一个都不在
/// 子进程环境里、没点名的照旧在、**取值一个字节都不落**。
///
/// ⚠ 正对照改的是**测试进程自己的环境**（`set_var`）：串行跑时是确定的（全量套件就是串行）。
/// Windows 侧未验证（那一列要 cmd 的转储语法，本机没有 Windows 工具链）。
#[cfg(target_os = "linux")]
#[test]
fn the_credentials_are_scrubbed_from_the_child_environment() {
    let sandbox = Sandbox::new("scrub");
    let dump = sandbox.path().join("env-names.txt");
    let unique = std::process::id();
    let prefixed = format!("CELIRA_KNIFE6_SENTINEL_{unique}");
    let named = format!("KNIFE6_NAMED_SENTINEL_{unique}");
    let prefixed_value = format!("value-of-{prefixed}");
    let named_value = format!("value-of-{named}");

    // 正对照：**直接**起一支 bash 跑同一条转储命令（不经过这只手）⇒ 两个哨兵真的会被转储出来。
    unsafe {
        std::env::set_var(&prefixed, &prefixed_value);
        std::env::set_var(&named, &named_value);
    }
    let control = std::process::Command::new("bash")
        .arg("-c")
        .arg(format!("env | cut -d= -f1 > '{}'", dump.display()))
        .status()
        .expect("起对照用的 bash");
    assert!(control.success(), "对照用的 bash 自己得跑成");
    let seen = std::fs::read_to_string(&dump).expect("对照的转储可读");
    assert!(
        seen.contains(&prefixed) && seen.contains(&named),
        "正对照失败：转储这个动作本身没生效——先查它，别把用例坏当成墙生效"
    );
    std::fs::remove_file(&dump).expect("清掉对照的转储");

    // 经过这只手：同一条转储命令（先转储、再 exec 夹具）；宿主点名要洗的键名。
    let command = format!(
        "env | cut -d= -f1 > '{}'; exec {}",
        dump.display(),
        fixture()
    );
    let declaration = parse_declaration(&format!("lab=stdio:{command}")).expect("声明可以解析");
    let scrub = vec![named.clone()];
    let mut transport = StdioTransport::spawn(&declaration, &scrub).expect("通道可以起");
    let reading = transport
        .credential_reading()
        .expect("stdio 通道有这份读数");
    unsafe {
        std::env::remove_var(&prefixed);
        std::env::remove_var(&named);
    }

    transport
        .connect(Duration::from_secs(20))
        .expect("握手可以成");
    let reply = transport
        .call_tool(
            "echo",
            json!({"text": "still-alive"}),
            Duration::from_secs(20),
        )
        .expect("调用可以成");

    let seen = std::fs::read_to_string(&dump).expect("子进程的转储可读");
    assert!(
        !seen.contains(&prefixed),
        "系统自有前缀那族要被洗掉：{seen}"
    );
    assert!(!seen.contains(&named), "宿主点名的键名要被洗掉：{seen}");
    assert!(
        seen.contains("PATH"),
        "没被点名的照旧在（对照：不是把环境清空了）：{seen}"
    );

    let (scrubbed, unscrubbed) = reading;
    assert!(scrubbed >= 2, "洗掉几个要如实报（这里是 {scrubbed}）");
    assert!(
        unscrubbed.is_empty(),
        "洗不掉的要如实报（这里是空的）：{unscrubbed:?}"
    );
    if let CallReply::Replied { text, .. } = reply {
        assert!(text.contains("still-alive"), "通道照旧能用：{text}");
        assert!(
            !text.contains(&prefixed_value) && !text.contains(&named_value),
            "哨兵的**取值**一个字节都不许出现：{text}"
        );
    } else {
        panic!("预期一条答复：{reply:?}");
    }
}

/// **坏行与非 UTF-8**：协议之外的行只进**噪声读数**（不静默当没发生），通道照旧能用；
/// 非 UTF-8 的字节按有损读进来、当噪声跳过——**不假装它是协议**。
#[cfg(target_os = "linux")]
#[test]
fn a_bad_line_and_non_utf8_bytes_are_counted_as_noise() {
    let sandbox = Sandbox::new("noise");
    let marker = sandbox.path().join("child-started.txt");
    // 先吐一行非 UTF-8 的字节（GBK 的「本地连接」四个字）、再吐一行普通垃圾，然后 exec 夹具。
    let command = format!(
        "printf '\\xB1\\xBE\\xB5\\xD8\\xC1\\xAC\\xBD\\xD3\\n'; printf 'not-a-protocol-line\\n'; exec {} --marker {}",
        fixture(),
        marker.display()
    );
    let declaration = parse_declaration(&format!("lab=stdio:{command}")).expect("声明可以解析");
    let mut transport = StdioTransport::spawn(&declaration, &[]).expect("通道可以起");
    transport
        .connect(Duration::from_secs(20))
        .expect("握手可以成（噪声不挡协议）");

    let reply = transport
        .call_tool(
            "echo",
            json!({"text": "after-noise"}),
            Duration::from_secs(20),
        )
        .expect("通道照旧能用");
    match reply {
        CallReply::Replied { text, noise, .. } => {
            assert!(text.contains("after-noise"), "{text}");
            assert!(noise >= 2, "协议之外的两行都要进噪声读数：noise = {noise}");
        }
        other => panic!("预期一条答复：{other:?}"),
    }
}
