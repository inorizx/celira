//! MCP resources 只读数据面（2026-10-25）：真进程验收。
//!
//! 三条判据：
//!
//! - 声明了 `resources` 的 server 展开后**多出两个只读动作**，普通工具照旧可用；
//! - 纯资源型（夹具 `--no-tools`）也能展开，而且**只有**那两个动作；
//! - 没声明 `resources` 的旧夹具**一个资源动作都不多出**（既有 tools-only 行为不变）。
//!
//! 另外两条用假通道钉住两件真进程不好造的事：名字冲突时合成动作**让位**给对面的工具
//! （不整台失败、也不把对面工具顶成只读），以及只读动作的 `Indeterminate` **不落成
//! `unknown`**（读不改变世界，没有"可能已经发生"这一格）。
//!
//! 这里不碰 Core / Run / Turn：两个动作仍是普通 `AppPort` 动作，走同一条 `execute`。

use std::time::Duration;

use serde_json::{Value, json};

use celira_agent::app_contract::{AppPort, EffectClass};
use celira_agent::core::{Action, ActionId, ActionOutcome, ActionParams, ActionPort};
use celira_agent::foundation::{AgentId, OperationId, ResultState, RunId, Turn};
use celira_agent::mcp::{
    CallFault, CallReply, McpAccess, McpApp, McpFailure, RawTool, ResourceBody, ResourceContent,
    ResourcePage, Transport, parse_declaration,
};

// ---------------------------------------------------------------------------
// 真进程：夹具就是 `src/bin/mcp_fixture_server.rs`
// ---------------------------------------------------------------------------

fn fixture() -> &'static str {
    env!("CARGO_BIN_EXE_mcp_fixture_server")
}

/// 一台指向真夹具的 server（`flags` 决定它是哪一档演员）。
fn access(name: &str, flags: &str) -> McpAccess {
    let text = format!("{name}=stdio:{} {flags}", fixture());
    McpAccess::new(parse_declaration(&text).expect("声明可以解析"), Vec::new())
}

fn agent() -> AgentId {
    AgentId::new("agent-a")
}

fn operation() -> OperationId {
    OperationId::new(RunId::new("run-1"), Turn::FIRST)
}

fn action(id: &str) -> Action {
    Action::with_params(ActionId::new(id), ActionParams::none())
}

fn with(id: &str, name: &str, value: &str) -> Action {
    Action::with_params(ActionId::new(id), ActionParams::none().with(name, value))
}

fn receipt(outcome: &ActionOutcome) -> String {
    outcome
        .content()
        .first()
        .map(|block| block.text().to_string())
        .unwrap_or_default()
}

fn failure(outcome: &ActionOutcome) -> String {
    outcome
        .failure_class()
        .unwrap_or("<no-failure>")
        .to_string()
}

fn ids(app: &McpApp) -> Vec<String> {
    app.declared_capabilities()
        .items()
        .iter()
        .map(|capability| capability.id().as_str().to_string())
        .collect()
}

fn class_of(app: &McpApp, action: &str) -> Option<EffectClass> {
    app.declared_effects()
        .iter()
        .find(|declared| declared.action().as_str() == action)
        .map(|declared| declared.class())
}

// ---------------------------------------------------------------------------
// 真进程验收
// ---------------------------------------------------------------------------

/// 声明了 resources 的 server：两个通用只读动作 ＋ 原有工具，效果各按自己那一格。
#[test]
fn a_server_with_resources_gets_two_read_only_actions_and_keeps_its_tools() {
    let mut app = McpApp::new(access("lab", "--resources"));
    app.present().expect("声明了 resources 的 server 可以展开");

    let ids = ids(&app);
    assert!(ids.contains(&"lab_echo".to_string()), "{ids:?}");
    assert!(ids.contains(&"lab_resources_list".to_string()), "{ids:?}");
    assert!(ids.contains(&"lab_resources_read".to_string()), "{ids:?}");
    assert_eq!(
        ids.len(),
        15,
        "13 条工具 ＋ 2 条资源动作，一条不多一条不少：{ids:?}"
    );

    assert_eq!(
        class_of(&app, "lab_echo"),
        Some(EffectClass::UnverifiableWrite),
        "普通工具仍是宿主声明的那一档（默认最严）"
    );
    assert_eq!(
        class_of(&app, "lab_resources_list"),
        Some(EffectClass::ReadOnly),
        "资源清单是只读动作，与宿主给整台的档位无关"
    );
    assert_eq!(
        class_of(&app, "lab_resources_read"),
        Some(EffectClass::ReadOnly)
    );

    let echoed = app.execute(&agent(), &operation(), &with("lab_echo", "text", "hi"));
    assert_eq!(echoed.state(), ResultState::Success);
    assert!(
        receipt(&echoed).contains("echo: hi"),
        "{}",
        receipt(&echoed)
    );
}

/// 清单是一次一页的**元数据**：第一页带游标、第二页用游标翻到，正文一个字都不进来。
#[test]
fn the_list_action_pages_and_never_carries_a_body() {
    let mut app = McpApp::new(access("lab", "--resources"));
    app.present().expect("展开可以成功");

    let first = app.execute(&agent(), &operation(), &action("lab_resources_list"));
    assert_eq!(first.state(), ResultState::Success);
    let text = receipt(&first);
    assert!(text.contains("fixture://readme"), "{text}");
    assert!(text.contains("mime = text/markdown"), "{text}");
    assert!(text.contains("size = "), "{text}");
    assert!(text.contains("next-cursor = page-2"), "{text}");
    assert!(
        !text.contains("fixture 资源正文"),
        "清单只给元数据，正文要另调 resources_read：{text}"
    );
    assert!(
        !text.contains("fixture://logo"),
        "第二页的资源不该出现在第一页：{text}"
    );

    let second = app.execute(
        &agent(),
        &operation(),
        &with("lab_resources_list", "cursor", "page-2"),
    );
    assert_eq!(second.state(), ResultState::Success);
    let text = receipt(&second);
    assert!(text.contains("fixture://logo"), "{text}");
    assert!(text.contains("next-cursor = none"), "{text}");
}

/// 读取：文本进回执，blob 只报大小（base64 原文一个字节都不搬）。
#[test]
fn the_read_action_returns_text_and_only_reports_blobs() {
    let mut app = McpApp::new(access("lab", "--resources"));
    app.present().expect("展开可以成功");

    let read = app.execute(
        &agent(),
        &operation(),
        &with("lab_resources_read", "uri", "fixture://readme"),
    );
    assert_eq!(read.state(), ResultState::Success);
    let text = receipt(&read);
    assert!(text.contains("requested-uri = fixture://readme"), "{text}");
    assert!(text.contains("mime = text/markdown"), "{text}");
    assert!(text.contains("truncated = false"), "{text}");
    assert!(text.contains("fixture 资源正文：readme"), "{text}");

    let blob = app.execute(
        &agent(),
        &operation(),
        &with("lab_resources_read", "uri", "fixture://logo"),
    );
    assert_eq!(blob.state(), ResultState::Success);
    let text = receipt(&blob);
    assert!(text.contains("mime = image/png"), "{text}");
    assert!(text.contains("body = blob"), "{text}");
    assert!(
        !text.contains("iVBORw0KGgo"),
        "blob 不搬原文，只报类型与大小：{text}"
    );
}

/// 读取缺必填 `uri`：本地回绝（零转发），诊断指出是哪一项。
#[test]
fn reading_without_a_uri_is_refused_locally_with_a_diagnostic() {
    let mut app = McpApp::new(access("lab", "--resources"));
    app.present().expect("展开可以成功");

    let refused = app.execute(&agent(), &operation(), &action("lab_resources_read"));
    assert_eq!(refused.state(), ResultState::Failure);
    assert_eq!(failure(&refused), McpFailure::InvalidArgument.as_str());
    assert!(
        receipt(&refused).contains("parameter = uri"),
        "诊断要说清缺的是哪一个：{}",
        receipt(&refused)
    );
}

/// 不存在 / 读失败：走既有词表，而且**是明确失败**（只读动作没有 `unknown` 这一格）。
#[test]
fn a_missing_resource_is_a_plain_failure_not_an_unknown_write() {
    let mut app = McpApp::new(access("lab", "--resources"));
    app.present().expect("展开可以成功");

    let missing = app.execute(
        &agent(),
        &operation(),
        &with("lab_resources_read", "uri", "fixture://missing"),
    );
    assert_eq!(missing.state(), ResultState::Failure);
    assert_eq!(failure(&missing), McpFailure::ServerError.as_str());
}

/// 纯资源型：tools 为空也照常展开，动词面**恰好**是两个只读动作。
#[test]
fn a_resources_only_server_still_expands_with_exactly_two_actions() {
    let mut app = McpApp::new(access("lab", "--resources --no-tools"));
    app.present()
        .expect("tools 为空不该让纯资源型 server 展不开");

    let ids = ids(&app);
    assert_eq!(ids.len(), 2, "只有两条通用资源动作：{ids:?}");
    assert!(ids.contains(&"lab_resources_list".to_string()), "{ids:?}");
    assert!(ids.contains(&"lab_resources_read".to_string()), "{ids:?}");
    assert_eq!(
        class_of(&app, "lab_resources_list"),
        Some(EffectClass::ReadOnly)
    );
    assert_eq!(
        class_of(&app, "lab_resources_read"),
        Some(EffectClass::ReadOnly)
    );

    let listed = app.execute(&agent(), &operation(), &action("lab_resources_list"));
    assert_eq!(listed.state(), ResultState::Success);
    assert!(receipt(&listed).contains("fixture://readme"));
}

/// 回归：没有声明 resources 的旧夹具，一个资源动作都不多出。
#[test]
fn a_server_without_resources_gets_no_resource_actions() {
    let mut app = McpApp::new(access("lab", ""));
    app.present().expect("旧夹具照常展开");

    let ids = ids(&app);
    assert_eq!(ids.len(), 13, "旧夹具仍是那 13 条工具：{ids:?}");
    assert!(
        !ids.iter().any(|id| id.starts_with("lab_resources_")),
        "没声明 resources 的 server 不该多出任何资源动作：{ids:?}"
    );
}

// ---------------------------------------------------------------------------
// 假通道：两条真进程不好造的格子
// ---------------------------------------------------------------------------

fn tool(name: &str) -> RawTool {
    RawTool {
        name: name.to_string(),
        description: Some("对面自己的工具".to_string()),
        input_schema: json!({"type": "object", "properties": {}}),
    }
}

/// 一条**有资源面**的假通道：资源清单与读取都按字段给，读取可以给一个故障。
struct ResourceFake {
    tools: Vec<RawTool>,
    page: ResourcePage,
    read: Vec<ResourceContent>,
    read_fault: Option<CallFault>,
}

impl ResourceFake {
    fn new(tools: Vec<RawTool>) -> Self {
        Self {
            tools,
            page: ResourcePage::default(),
            read: Vec::new(),
            read_fault: None,
        }
    }
}

impl Transport for ResourceFake {
    fn connect(&mut self, _timeout: Duration) -> Result<(), CallFault> {
        Ok(())
    }

    fn list_tools(&mut self, _timeout: Duration) -> Result<Vec<RawTool>, CallFault> {
        Ok(self.tools.clone())
    }

    fn call_tool(
        &mut self,
        _tool: &str,
        _arguments: Value,
        _timeout: Duration,
    ) -> Result<CallReply, CallFault> {
        Err(CallFault::Refused(McpFailure::Transport))
    }

    fn supports_resources(&self) -> bool {
        true
    }

    fn list_resources(
        &mut self,
        _cursor: Option<String>,
        _timeout: Duration,
    ) -> Result<ResourcePage, CallFault> {
        Ok(self.page.clone())
    }

    fn read_resource(
        &mut self,
        _uri: &str,
        _timeout: Duration,
    ) -> Result<Vec<ResourceContent>, CallFault> {
        match self.read_fault.clone() {
            Some(fault) => Err(fault),
            None => Ok(self.read.clone()),
        }
    }
}

fn app_with(fake: ResourceFake) -> McpApp {
    McpApp::new(McpAccess::with_transport(
        parse_declaration("lab=stdio:fixture-server").expect("声明可以解析"),
        Vec::new(),
        Box::new(fake),
    ))
}

/// 对面真的有一个叫 `resources_list` 的工具：合成动作**让位**，不整台失败，
/// 也不会把对面的工具悄悄顶成只读。
#[test]
fn a_real_tool_named_like_the_synthetic_action_keeps_its_own_effect() {
    let mut app = app_with(ResourceFake::new(vec![tool("resources_list")]));
    app.present().expect("名字冲突不该让整台失败");

    let ids = ids(&app);
    assert_eq!(
        ids.iter()
            .filter(|id| id.as_str() == "lab_resources_list")
            .count(),
        1,
        "同名条目只该有一条（对面的工具）：{ids:?}"
    );
    assert!(
        ids.contains(&"lab_resources_read".to_string()),
        "另一条合成动作照旧挂上：{ids:?}"
    );
    assert_eq!(
        class_of(&app, "lab_resources_list"),
        Some(EffectClass::UnverifiableWrite),
        "那个名字是对面的工具，不能被合成动作顶成只读"
    );
    assert_eq!(
        class_of(&app, "lab_resources_read"),
        Some(EffectClass::ReadOnly)
    );
}

/// 只读动作遇到"发出去之后不知道"：如实落进明确失败，不是 `unknown`。
#[test]
fn an_indeterminate_resource_read_is_not_an_unknown_write() {
    let mut fake = ResourceFake::new(Vec::new());
    fake.read_fault = Some(CallFault::Indeterminate(McpFailure::Transport));
    let mut app = app_with(fake);
    app.present().expect("展开可以成功");

    let read = app.execute(
        &agent(),
        &operation(),
        &with("lab_resources_read", "uri", "fixture://whatever"),
    );
    assert_eq!(
        read.state(),
        ResultState::Failure,
        "读不改变世界：没有「可能已经发生」这一格"
    );
    assert_eq!(failure(&read), McpFailure::Transport.as_str());
}

/// 空答复（对面说"有这条资源，但一个内容块都没给"）：如实说 0 块，不编正文。
#[test]
fn an_empty_read_reply_says_zero_blocks() {
    let mut fake = ResourceFake::new(Vec::new());
    fake.read = Vec::new();
    let mut app = app_with(fake);
    app.present().expect("展开可以成功");

    let read = app.execute(
        &agent(),
        &operation(),
        &with("lab_resources_read", "uri", "fixture://empty"),
    );
    assert_eq!(read.state(), ResultState::Success);
    let text = receipt(&read);
    assert!(text.contains("content = 0 块"), "{text}");
}

/// 文本正文的读取形状（假通道直给一块文本）：这一条钉住回执渲染本身，
/// 与真夹具那条（`the_read_action_returns_text_and_only_reports_blobs`）互补。
#[test]
fn a_text_body_is_rendered_under_an_explicit_heading() {
    let mut fake = ResourceFake::new(Vec::new());
    fake.read = vec![ResourceContent {
        uri: "fixture://note".to_string(),
        mime_type: Some("text/plain".to_string()),
        body: ResourceBody::Text {
            text: "第一行\n第二行\n".to_string(),
            text_bytes: 16,
            truncated: false,
        },
        omitted: 0,
    }];
    let mut app = app_with(fake);
    app.present().expect("展开可以成功");

    let read = app.execute(
        &agent(),
        &operation(),
        &with("lab_resources_read", "uri", "fixture://note"),
    );
    let text = receipt(&read);
    assert!(text.contains("--- content ---\n第一行\n第二行"), "{text}");
    assert!(
        text.contains("text = 16 bytes, truncated = false"),
        "{text}"
    );
}
