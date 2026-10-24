//! 真 Streamable HTTP：用 `rmcp` 的服务端在回环上起一台 MCP server，
//! 再由 [`RmcpTransport`] 走 HTTP 客户端连上它。
//!
//! 这里证明的不是「配置能构造」，而是：握手、`tools/list`、`tools/call` 都真的
//! 经过了现代 HTTP transport（POST + SSE 会话）。

use std::time::Duration;

use rmcp::{
    ErrorData, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use tokio::net::TcpListener;

use celira_agent::mcp::{RmcpTransport, Transport, parse_declaration};

#[derive(Clone)]
struct HttpFixture;

impl ServerHandler for HttpFixture {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "要回声的文本"}
            },
            "required": ["text"]
        });
        let schema = schema.as_object().cloned().expect("对象");
        Ok(ListToolsResult::with_all_items(vec![Tool::new(
            "http_echo",
            "HTTP 回声",
            schema,
        )]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let text = request
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get("text"))
            .and_then(|value| value.as_str())
            .unwrap_or("(空)");
        Ok(CallToolResult::success(vec![ContentBlock::text(format!("http:{text}"))]).into())
    }
}

async fn spawn_server() -> String {
    let service: StreamableHttpService<HttpFixture, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(HttpFixture),
            Default::default(),
            StreamableHttpServerConfig::default(),
        );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定回环端口");
    let address = listener.local_addr().expect("本地地址");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    format!("http://{address}/mcp")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_streamable_http_server_is_really_reached() {
    let address = spawn_server().await;
    let declaration = parse_declaration(&format!("lab={address}")).expect("回环地址可以声明");

    tokio::task::spawn_blocking(move || {
        let mut transport = RmcpTransport::new(&declaration, &[]).expect("宿主可以起");
        transport
            .connect(Duration::from_secs(20))
            .expect("HTTP 握手可以成");
        let tools = transport
            .list_tools(Duration::from_secs(20))
            .expect("清单可以拉");
        assert!(
            tools.iter().any(|tool| tool.name == "http_echo"),
            "HTTP server 的动词要在清单里：{tools:?}"
        );
        let arguments = serde_json::json!({"text": "hello"});
        let reply = transport
            .call_tool("http_echo", arguments, Duration::from_secs(20))
            .expect("调用可以回");
        let text = match reply {
            celira_agent::mcp::CallReply::Replied { text, .. } => text,
            other => panic!("应当是成功回执：{other:?}"),
        };
        assert!(text.contains("http:hello"), "真实 HTTP 回执：{text}");
    })
    .await
    .expect("阻塞任务可以完成");
}
