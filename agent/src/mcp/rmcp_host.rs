//! 用 `rmcp` 承载 MCP 协议，把异步 I/O 收在一条独立宿主线程里。
//!
//! 这一层只替换**通道**：上面的 [`super::app::McpApp`] 仍然只看见
//! `connect / list_tools / call_tool`，不知道对面是 stdio 还是 Streamable 网络，
//! 也不知道 SDK 的生命周期。Core、Run、Turn 仍然是同步状态机。
//!
//! ```text
//! 同步调用方（Agent / AppPort）
//!   └─ RmcpTransport（std::sync 通道 + 超时阻塞）
//!        └─ 宿主线程（tokio runtime）
//!             └─ rmcp::RunningService（Auto 生命周期：modern discover / legacy initialize）
//!                  └─ MCP server（stdio / Streamable 网络）
//! ```
//!
//! **协议正确性归 SDK，产品边界归 Celira。** 通知仍然只作为“变更信号”交给上层；
//! 正文必须由上层重新 `tools/list` 拉取，本层不往模型上下文里写任何东西。

use std::future::Future;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::{
    ClientHandler, ClientLifecycleMode, ClientServiceExt,
    model::{
        CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation, ProtocolVersion,
        ServerNotification, SubscriptionFilter, Tool,
    },
    service::{
        ClientInitializeError, MaybeSendFuture, NotificationContext, RoleClient, RunningService,
    },
    transport::{
        TokioChildProcess,
        streamable_http_client::{
            StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
        },
    },
};

use super::stdio::StdioTransport;
use super::wire::{describe_error, read_result};
use super::{
    CallFault, CallReply, McpDeclaration, McpEndpoint, McpFailure, McpNotification, RawTool,
    Transport, clip, sanitize,
};

/// `rmcp` 客户端的固定类型：handler 只做一件事——把通知收进共享队列。
type Client = RunningService<RoleClient, HostHandler>;

/// 宿主线程通过这条命令队列接受同步调用。
enum Command {
    ListTools {
        timeout: Duration,
        reply: Sender<Result<Vec<RawTool>, CallFault>>,
    },
    CallTool {
        name: String,
        arguments: serde_json::Value,
        timeout: Duration,
        reply: Sender<Result<CallReply, CallFault>>,
    },
    SubscribeTools {
        timeout: Duration,
        reply: Sender<Result<bool, CallFault>>,
    },
    Shutdown,
}

/// 一次连接尝试的结果。失败时把分类和可读原文一起交回同步侧。
struct Ready {
    result: Result<(), CallFault>,
    detail: Option<String>,
}

/// 通知接收器：legacy 路径由 handler 回调推入；现代订阅路径由单独任务推入。
#[derive(Clone)]
struct HostHandler {
    notifications: Arc<Mutex<Vec<McpNotification>>>,
}

impl HostHandler {
    fn signal_tools_changed(&self) {
        let mut queue = self
            .notifications
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !queue.contains(&McpNotification::ToolsListChanged) {
            queue.push(McpNotification::ToolsListChanged);
        }
    }
}

impl ClientHandler for HostHandler {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("celira-agent", env!("CARGO_PKG_VERSION")),
        )
    }

    fn on_tool_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        self.signal_tools_changed();
        std::future::ready(())
    }
}

/// 同步侧看见的 MCP 通道。线程在 `new` 时启动，`connect` 等它握手完成。
pub struct RmcpTransport {
    command: Option<tokio::sync::mpsc::UnboundedSender<Command>>,
    ready: Option<Receiver<Ready>>,
    join: Option<std::thread::JoinHandle<()>>,
    notifications: Arc<Mutex<Vec<McpNotification>>>,
    detail: Option<String>,
    is_stdio: bool,
    scrubbed: usize,
    unscrubbed: Vec<String>,
    connected: bool,
}

impl RmcpTransport {
    /// 起一条宿主线程；真正的握手在 [`Transport::connect`] 里等待。
    pub fn new(declaration: &McpDeclaration, scrub: &[String]) -> Result<Self, McpFailure> {
        let (ready_tx, ready_rx) = mpsc::channel();
        let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
        let notifications = Arc::new(Mutex::new(Vec::new()));

        let is_stdio = matches!(declaration.endpoint(), McpEndpoint::Stdio { .. });
        let (scrubbed, unscrubbed) = if is_stdio {
            let (_, scrubbed, unscrubbed) = crate::platform::child_environment(scrub);
            (scrubbed, unscrubbed)
        } else {
            (0, Vec::new())
        };

        let declaration = declaration.clone();
        let scrub = scrub.to_vec();
        let thread_notifications = Arc::clone(&notifications);
        let join = std::thread::Builder::new()
            .name("celira-mcp-host".to_string())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_tx.send(Ready {
                            result: Err(CallFault::Refused(McpFailure::SpawnFailure)),
                            detail: Some(format!("MCP 宿主运行时没起来：{error}")),
                        });
                        return;
                    }
                };
                runtime.block_on(host_main(
                    declaration,
                    scrub,
                    thread_notifications,
                    command_rx,
                    ready_tx,
                ));
            })
            .map_err(|_| McpFailure::SpawnFailure)?;

        Ok(Self {
            command: Some(command_tx),
            ready: Some(ready_rx),
            join: Some(join),
            notifications,
            detail: None,
            is_stdio,
            scrubbed,
            unscrubbed,
            connected: false,
        })
    }

    /// 握手失败时面向展开回执的一行原文（已消毒、已截断）。
    pub fn refusal_detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    fn request<T>(
        &self,
        timeout: Duration,
        make: impl FnOnce(Sender<Result<T, CallFault>>) -> Command,
    ) -> Result<T, CallFault> {
        let Some(command) = self.command.as_ref() else {
            return Err(CallFault::Refused(McpFailure::Transport));
        };
        let (reply_tx, reply_rx) = mpsc::channel();
        command
            .send(make(reply_tx))
            .map_err(|_| CallFault::Refused(McpFailure::Transport))?;
        match reply_rx.recv_timeout(timeout) {
            Ok(reply) => reply,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                Err(CallFault::Indeterminate(McpFailure::Timeout))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(CallFault::Indeterminate(McpFailure::Transport))
            }
        }
    }
}

impl Transport for RmcpTransport {
    fn connect(&mut self, timeout: Duration) -> Result<(), CallFault> {
        if self.connected {
            return Ok(());
        }
        let Some(ready) = self.ready.take() else {
            return Err(CallFault::Refused(McpFailure::HandshakeFailure));
        };
        match ready.recv_timeout(timeout) {
            Ok(Ready { result, detail }) => {
                self.detail = detail.map(|text| clip(&sanitize(&text), 2048).0);
                result?;
                self.connected = true;
                Ok(())
            }
            Err(_) => Err(CallFault::Refused(McpFailure::HandshakeFailure)),
        }
    }

    fn list_tools(&mut self, timeout: Duration) -> Result<Vec<RawTool>, CallFault> {
        self.request(timeout, |reply| Command::ListTools { timeout, reply })
    }

    fn call_tool(
        &mut self,
        tool: &str,
        arguments: serde_json::Value,
        timeout: Duration,
    ) -> Result<CallReply, CallFault> {
        self.request(timeout, |reply| Command::CallTool {
            name: tool.to_string(),
            arguments,
            timeout,
            reply,
        })
    }

    /// 现代订阅在握手后已经由宿主线程发出；这里只报告这条通道支持通知。
    /// legacy server 不认 `subscriptions/listen` 时，handler 仍会收到旧式
    /// `notifications/tools/list_changed`。
    fn subscribe_tools_changed(&mut self, timeout: Duration) -> Result<bool, CallFault> {
        self.request(timeout, |reply| Command::SubscribeTools { timeout, reply })
    }

    fn poll_notifications(&mut self) -> Vec<McpNotification> {
        let mut queue = self
            .notifications
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        std::mem::take(&mut *queue)
    }

    fn credential_reading(&self) -> Option<(usize, Vec<String>)> {
        self.is_stdio
            .then(|| (self.scrubbed, self.unscrubbed.clone()))
    }
}

impl Drop for RmcpTransport {
    fn drop(&mut self) {
        if let Some(command) = self.command.take() {
            let _ = command.send(Command::Shutdown);
        }
        if let Some(join) = self.join.take() {
            if self.connected {
                let _ = join.join();
            } else {
                // 还没握手完成就丢弃时，宿主线程可能正卡在 connect；不 join，
                // 让它按自己的失败/取消路径收尾，避免把一个同步调用拖死。
                drop(join);
            }
        }
    }
}

async fn host_main(
    declaration: McpDeclaration,
    scrub: Vec<String>,
    notifications: Arc<Mutex<Vec<McpNotification>>>,
    mut commands: tokio::sync::mpsc::UnboundedReceiver<Command>,
    ready: Sender<Ready>,
) {
    let handler = HostHandler {
        notifications: Arc::clone(&notifications),
    };

    let client = match connect(&declaration, &scrub, handler).await {
        Ok(client) => client,
        Err((fault, detail)) => {
            let _ = ready.send(Ready {
                result: Err(fault),
                detail,
            });
            return;
        }
    };

    // 现代 server：显式订阅工具清单变化。legacy server 不认这个方法时返回错误，
    // 不影响连接；旧式通知仍可能从 client handler 到达。
    // ⚠ 订阅由上层在拿到初始清单之后显式触发（`subscribe_tools_changed`），
    // 不能在 `connect` 里抢跑：那会让初始清单变成“订阅后的清单”。
    let mut subscribed = false;
    let _ = ready.send(Ready {
        result: Ok(()),
        detail: None,
    });

    while let Some(command) = commands.recv().await {
        match command {
            Command::ListTools { timeout, reply } => {
                let result = match tokio::time::timeout(timeout, client.list_all_tools()).await {
                    Ok(Ok(tools)) => Ok(tools.into_iter().map(raw_tool).collect()),
                    Ok(Err(error)) => Err(list_fault(error)),
                    Err(_) => Err(CallFault::Refused(McpFailure::Timeout)),
                };
                let _ = reply.send(result);
            }
            Command::CallTool {
                name,
                arguments,
                timeout,
                reply,
            } => {
                let result = call_tool(&client, &name, arguments, timeout).await;
                let _ = reply.send(result);
            }
            Command::SubscribeTools { timeout, reply } => {
                let result = if subscribed {
                    Ok(true)
                } else {
                    let result = subscribe_tools(&client, &notifications, timeout).await;
                    if result {
                        subscribed = true;
                    }
                    Ok(result)
                };
                let _ = reply.send(result);
            }
            Command::Shutdown => break,
        }
    }

    let _ = client.cancel().await;
}

async fn connect(
    declaration: &McpDeclaration,
    scrub: &[String],
    handler: HostHandler,
) -> Result<Client, (CallFault, Option<String>)> {
    match declaration.endpoint() {
        McpEndpoint::Stdio { .. } => connect_stdio(declaration, scrub, handler).await,
        McpEndpoint::Remote {
            address,
            credential_env,
        } => {
            let mut config = StreamableHttpClientTransportConfig::with_uri(address.as_str())
                .reinit_on_expired_session(true);
            if let Some(name) = credential_env {
                match std::env::var(name) {
                    Ok(credential) if !credential.is_empty() => {
                        config = config.auth_header(credential);
                    }
                    _ => {
                        return Err((
                            CallFault::Refused(McpFailure::HandshakeFailure),
                            Some("远程 MCP 的凭据环境变量没有值".to_string()),
                        ));
                    }
                }
            }
            let transport = StreamableHttpClientTransport::from_config(config);
            handler
                .serve_with_lifecycle(transport, lifecycle())
                .await
                .map_err(init_failure)
        }
    }
}

/// stdio 的 modern 探测失败时，旧式 `initialize` 还要再试一次：
/// ① 旧 server 不认 `server/discover`，但 `initialize` 能接；
/// ② 只会新版的 server 在 `initialize` 里通常会留下它拒绝的版本表（展开回执里的那句话）。
///
/// 两次尝试各起一个子进程，避免复用一条已经半死的管道。
async fn connect_stdio(
    declaration: &McpDeclaration,
    scrub: &[String],
    handler: HostHandler,
) -> Result<Client, (CallFault, Option<String>)> {
    let first = spawn_stdio(declaration, scrub)?;
    let first = handler
        .clone()
        .serve_with_lifecycle(
            first,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![
                    ProtocolVersion::V_2026_07_28,
                    ProtocolVersion::V_2025_11_25,
                ],
            },
        )
        .await;
    match first {
        Ok(client) => Ok(client),
        Err(_first_error) => {
            let second = spawn_stdio(declaration, scrub)?;
            let second = handler
                .clone()
                .serve_with_lifecycle(second, ClientLifecycleMode::Initialize)
                .await;
            match second {
                Ok(client) => Ok(client),
                Err(second_error) => {
                    let primary = init_failure(second_error);
                    // rmcp 在「服务端拒绝 initialize」这条路上会把服务端的错误响应
                    // 当成一条无 method 的消息回一个 `-32600`，最后报成 `-32601`；
                    // 这不是对端的话。旧通道的握手诊断只在这条失败路径上跑一次，
                    // 用它的分类与原文把真实拒绝留下（成功路径仍然只走 rmcp）。
                    if let Some((class, detail)) = diagnose_stdio(declaration, scrub) {
                        return Err((CallFault::Refused(class), detail));
                    }
                    Err(primary)
                }
            }
        }
    }
}

fn diagnose_stdio(
    declaration: &McpDeclaration,
    scrub: &[String],
) -> Option<(McpFailure, Option<String>)> {
    let mut transport = StdioTransport::spawn(declaration, scrub).ok()?;
    let fault = transport.connect(Duration::from_secs(20)).err()?;
    Some((
        fault.class(),
        transport.refusal_detail().map(str::to_string),
    ))
}

fn spawn_stdio(
    declaration: &McpDeclaration,
    scrub: &[String],
) -> Result<TokioChildProcess, (CallFault, Option<String>)> {
    let command = declaration
        .stdio_command()
        .ok_or((CallFault::Refused(McpFailure::SpawnFailure), None))?;
    let (mut environment, _, _) = crate::platform::child_environment(scrub);
    let mut builder =
        crate::platform::mcp_command(crate::platform::MCP_SHELL, command, &mut environment);
    builder.stdin(std::process::Stdio::piped());
    builder.stdout(std::process::Stdio::piped());
    builder.stderr(std::process::Stdio::null());
    crate::platform::apply_environment(&mut builder, environment);
    let command = tokio::process::Command::from(builder);
    TokioChildProcess::new(command).map_err(|error| {
        (
            CallFault::Refused(McpFailure::SpawnFailure),
            Some(format!("MCP 子进程没起来：{error}")),
        )
    })
}

fn init_failure(error: ClientInitializeError) -> (CallFault, Option<String>) {
    match error {
        ClientInitializeError::JsonRpcError(error) => {
            let class = if error.code.0 == -32022 {
                McpFailure::UnsupportedVersion
            } else {
                McpFailure::HandshakeFailure
            };
            let value = serde_json::to_value(&error).unwrap_or(serde_json::Value::Null);
            (CallFault::Refused(class), Some(describe_error(&value)))
        }
        ClientInitializeError::NoCompatibleProtocolVersion { .. } => (
            CallFault::Refused(McpFailure::UnsupportedVersion),
            Some("没有双方都支持的协议版本".to_string()),
        ),
        ClientInitializeError::LegacyFallbackFailed { fallback, .. } => init_failure(*fallback),
        other => (
            CallFault::Refused(McpFailure::HandshakeFailure),
            Some(other.to_string()),
        ),
    }
}

fn lifecycle() -> ClientLifecycleMode {
    ClientLifecycleMode::Auto {
        preferred_versions: vec![ProtocolVersion::V_2026_07_28, ProtocolVersion::V_2025_11_25],
        legacy_version: Some(ProtocolVersion::V_2024_11_05),
    }
}

async fn subscribe_tools(
    client: &Client,
    notifications: &Arc<Mutex<Vec<McpNotification>>>,
    timeout: Duration,
) -> bool {
    let filter = SubscriptionFilter::builder().tools_list_changed().build();
    let Ok(Ok(mut subscription)) =
        tokio::time::timeout(timeout, client.peer().listen(filter)).await
    else {
        return false;
    };
    // `listen` 返回时只说明服务端收了订阅；紧接着到达的那条通知可能已经在通道里。
    // 先取一次（很短），让展开回执返回时变更已经落在 Runtime 的事实面；随后交给后台任务。
    if let Ok(Ok(Some(ServerNotification::ToolListChangedNotification(_)))) =
        tokio::time::timeout(Duration::from_millis(20), subscription.next()).await
    {
        let mut queue = notifications
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !queue.contains(&McpNotification::ToolsListChanged) {
            queue.push(McpNotification::ToolsListChanged);
        }
    }
    let notifications = Arc::clone(notifications);
    tokio::spawn(async move {
        while let Ok(Some(notification)) = subscription.next().await {
            if matches!(
                notification,
                ServerNotification::ToolListChangedNotification(_)
            ) {
                let mut queue = notifications
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                if !queue.contains(&McpNotification::ToolsListChanged) {
                    queue.push(McpNotification::ToolsListChanged);
                }
            }
        }
    });
    true
}

async fn call_tool(
    client: &Client,
    name: &str,
    arguments: serde_json::Value,
    timeout: Duration,
) -> Result<CallReply, CallFault> {
    let Some(arguments) = arguments.as_object().cloned() else {
        return Err(CallFault::Refused(McpFailure::InvalidArgument));
    };
    let params = CallToolRequestParams::new(name.to_string()).with_arguments(arguments);
    match tokio::time::timeout(timeout, client.call_tool(params)).await {
        Ok(Ok(result)) => {
            let value = serde_json::to_value(&result)
                .map_err(|_| CallFault::Refused(McpFailure::ServerError))?;
            Ok(read_result(&value, 0))
        }
        Ok(Err(error)) => match error {
            rmcp::ServiceError::McpError(error) => Ok(CallReply::Refused {
                code: Some(error.code.0 as i64),
                detail: clip(&sanitize(&error.message), 2048).0,
            }),
            rmcp::ServiceError::Timeout { .. } => {
                Err(CallFault::Indeterminate(McpFailure::Timeout))
            }
            _ => Err(CallFault::Indeterminate(McpFailure::Transport)),
        },
        Err(_) => Err(CallFault::Indeterminate(McpFailure::Timeout)),
    }
}

fn raw_tool(tool: Tool) -> RawTool {
    RawTool {
        name: tool.name.to_string(),
        description: tool.description.as_ref().map(|text| text.to_string()),
        input_schema: serde_json::to_value(&*tool.input_schema)
            .unwrap_or_else(|_| serde_json::json!({"type": "object", "properties": {}})),
    }
}

fn list_fault(error: rmcp::ServiceError) -> CallFault {
    match error {
        rmcp::ServiceError::Timeout { .. } => CallFault::Refused(McpFailure::Timeout),
        _ => CallFault::Refused(McpFailure::ToolsFailed),
    }
}
