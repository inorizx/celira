//! Backend Event 唤醒流（SSE 门铃；窄解冻切片 CORE_V1_AGENT_PROTOCOL.md §8）。
//!
//! `GET /v1/backend/events/stream`：与四端点同一 BackendAuth
//! （Authorization: Bearer 头），返回 `text/event-stream` 的易失「门铃」：
//! - 建立成功即发**一次初始唤醒**（`event: events.available`，覆盖重连/
//!   订阅间隙——客户端收到后 REST 补拉）；
//! - 此后每次该 Agent 有新 Agent Event 提交（`message.created` /
//!   `message.recalled`）时发 `event: events.available`（data 恒空），
//!   **不携带 cursor、Event id、正文或任何事实**；20s `: ping` 注释保活；
//! - **发送唤醒前重新认证**：Credential/Binding/Agent 任一级 revoked →
//!   流立即结束（fail closed，不泄露原因；客户端重连将得到 401）；
//!   认证存储失败同样结束流；
//! - 有界：每 Agent 并发流 ≤2、进程总流数有硬上限；超限 503
//!   `backend_stream_limit_exceeded`（不排队、不降级）；
//! - 唤醒可以全丢、可重复、可断；事实获取永远走 `GET /v1/backend/events`
//!   （REST 权威）；Core 不为 Backend 持久化连接状态、不写 SQLite、
//!   不进 Human Event 表 / Human WS 语义。轮询（§3.4）仍是完整合法路径。
//!
//! 本模块还提供 post-commit 定向唤醒助手 `wake_agents`：Human Message
//! 提交/撤回成功后用已提交 agent_events 事实反查 target Agent 再定向通知；
//! 查询或通知失败只记录脱敏 warn，不得把已提交事务伪装失败（与
//! `wake_humans` 同一纪律）。

use std::collections::HashMap;
use std::convert::Infallible;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::adapters::http::AppState;
use crate::adapters::http::auth::{BearerParse, parse_authorization};
use crate::adapters::http::backend_auth::{BackendAuthError, authenticate_token};
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::routes::error_response;
use crate::adapters::notifier::WAKE_CHANNEL_CAPACITY;
use crate::application::backend;
use crate::domain::agent_event::AgentEventKind;
use crate::domain::message::MessageId;
use crate::domain::participant::ParticipantId;
use crate::ports::live::ConnectionPresence;
use crate::protocol::error::codes;

/// 每个并发的唤醒事件名（data 恒空；不含任何事实）。
pub(crate) const BACKEND_STREAM_EVENT: &str = "events.available";

/// 每 Agent 并发流上限（同 Agent 多实例；超出 fail closed）。
const MAX_STREAMS_PER_AGENT: usize = 2;
/// 进程总流数硬上限（防异常膨胀；不影响耐久事实）。
const MAX_TOTAL_STREAMS: usize = 64;
/// SSE 保活注释间隔（与 Host 浏览器唤醒流同量级）。
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(20);
/// 唤醒转发任务检测响应体是否已被丢弃（客户端断开）的轮询间隔；
/// 超时未读到任何字节的判定由客户端读超时承担，这里只负责及时回收
/// 并发额度与订阅。
const LIVENESS_TICK: Duration = Duration::from_secs(5);

/// Backend Event 唤醒流路由。
pub fn router() -> Router<AppState> {
    Router::new().route("/v1/backend/events/stream", get(stream_route))
}

/// 连接事实的窄接口实现：门闩把「这位 Agent 此刻挂着几条门铃流」这一
/// 事实以只读方式借给进行态用例（在线 = 连接在；没有写入口，也没有
/// "上报离线"——流一断，条目自己就没了）。
impl ConnectionPresence for BackendStreamGate {
    fn is_connected(&self, participant: &ParticipantId) -> bool {
        self.is_connected(participant)
    }
}

/// 并发流门闩（进程内、纯易失；每 Agent ≤2、总量 ≤64）。
pub(crate) struct BackendStreamGate {
    inner: Mutex<GateInner>,
}

#[derive(Default)]
struct GateInner {
    total: usize,
    per_agent: HashMap<String, usize>,
}

/// 一条已获准的流占用：Drop 释放并发额度（由唤醒转发任务持有，任务
/// 退出即释放——包括客户端断开、撤销断流与发送失败各路径）。
pub(crate) struct StreamPermit {
    agent_id: String,
    gate: Arc<BackendStreamGate>,
}

impl BackendStreamGate {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(GateInner::default()),
        }
    }

    /// 连接事实（`ports::live::ConnectionPresence`）：这位 Agent 的 Backend
    /// 此刻还挂着门铃流吗？——**在线就是连接在**，不需要 Backend 上报心跳。
    pub(crate) fn is_connected(&self, agent_id: &ParticipantId) -> bool {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner
            .per_agent
            .get(agent_id.as_str())
            .is_some_and(|count| *count > 0)
    }

    /// 获准一条流占用；超限返回 None（fail closed，不排队）。
    /// 对 crate 内可见：连接事实测试要能真的"挂一条流"再验在线。
    pub(crate) fn acquire(self: &Arc<Self>, agent_id: &ParticipantId) -> Option<StreamPermit> {
        // Mutex poison 恢复：门闩是纯易失提示，锁内数据仍完整，直接接管
        // （安全降级：额度计数可能短暂偏差，不影响耐久事实）。
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = inner.per_agent.get(agent_id.as_str()).copied().unwrap_or(0);
        if inner.total >= MAX_TOTAL_STREAMS || current >= MAX_STREAMS_PER_AGENT {
            return None;
        }
        *inner
            .per_agent
            .entry(agent_id.as_str().to_owned())
            .or_insert(0) += 1;
        inner.total += 1;
        Some(StreamPermit {
            agent_id: agent_id.as_str().to_owned(),
            gate: self.clone(),
        })
    }
}

impl Drop for StreamPermit {
    fn drop(&mut self) {
        let mut inner = self
            .gate
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.total = inner.total.saturating_sub(1);
        if let Some(count) = inner.per_agent.get_mut(&self.agent_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                inner.per_agent.remove(&self.agent_id);
            }
        }
    }
}

/// 一条无事实唤醒事件（SSE wire：`event: events.available` + 空 data）。
fn wake_event() -> Event {
    Event::default().event(BACKEND_STREAM_EVENT).data("")
}

/// GET /v1/backend/events/stream：BackendAuth（头部 Bearer，与四端点同一
/// 认证与 401 语义）→ SSE 唤醒流。
async fn stream_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;

    // 1. Authorization 解析（与四端点同一 Bearer 纪律：一个且仅一个
    //    header、非空、拒绝逗号合并/重复；缺 header 与其余失败分类不同）。
    let token = match parse_authorization(&headers) {
        BearerParse::Ok(token) => token,
        BearerParse::Missing => {
            return BackendAuthError::MissingHeader { request_id }.into_response();
        }
        BearerParse::Invalid => {
            return BackendAuthError::InvalidCredential { request_id }.into_response();
        }
    };

    // 2. 认证（复用与 extractor 完全一致的共享 helper；token 不进日志）。
    let auth = match authenticate_token(&state, &token, &request_id).await {
        Ok(auth) => auth,
        Err(error) => return error.into_response(),
    };
    let agent_id = auth.view.agent.id.clone();

    // 3. 并发上限（fail closed：不排队、不降级；轮询仍是完整路径）。
    let Some(permit) = state.backend_streams.acquire(&agent_id) else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            codes::BACKEND_STREAM_LIMIT_EXCEEDED,
            "backend event stream limit exceeded",
            &request_id,
        );
    };

    // 4. 订阅该 Agent 的唤醒通道（容量 1：重复提示合并；notifier 只持有
    //    弱引用，强引用由转发任务持有，任务退出即惰性剪除订阅）。
    let (wake_tx, mut wake_rx) = mpsc::channel::<()>(WAKE_CHANNEL_CAPACITY);
    let wake = Arc::new(wake_tx);
    state.backend_notifier.subscribe(&agent_id, wake.clone());

    // 5. 唤醒转发任务：初始唤醒 → 循环（每次发送前重新认证）。任务持有
    //    permit 与 wake 强引用——退出（客户端断开 / 撤销断流 / 发送失败）
    //    即释放并发额度并剪除订阅。响应体被丢弃时 body_tx.is_closed()
    //    变真（保活注释保证服务端定期写、hyper 定期感知断开）。
    let (body_tx, body_rx) = mpsc::channel::<Result<Event, Infallible>>(1);
    let backend_store = state.backend.clone();
    tokio::spawn(async move {
        let _permit = permit;
        let _wake = wake;
        // 初始唤醒：建立即发一次（覆盖重连/订阅间隙；客户端收到即补拉）。
        if body_tx.send(Ok(wake_event())).await.is_err() {
            return;
        }
        let mut tick = tokio::time::interval(LIVENESS_TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if body_tx.is_closed() {
                        break;
                    }
                }
                signal = wake_rx.recv() => {
                    let Some(()) = signal else { break };
                    match backend::authenticate_backend(backend_store.as_ref(), &token).await {
                        Ok(_) => {
                            if body_tx.send(Ok(wake_event())).await.is_err() {
                                break;
                            }
                        }
                        Err(backend::AuthenticateBackendError::Store(_)) => {
                            tracing::error!(
                                operation = "backend_stream.wake_reauth",
                                error_kind = "storage_unavailable",
                                request_id = %request_id,
                                "backend store failure during stream wake re-authentication"
                            );
                            break;
                        }
                        // revoked / invalid → 结束流（fail closed，零泄露；
                        // 客户端重连将得到 401）。
                        Err(_) => break,
                    }
                }
            }
        }
    });

    // 6. SSE 响应：事件流 + 20s 保活注释（由 axum 直接发出，不经过
    //    唤醒通道）；no-store 防中间层缓存易失唤醒。
    let stream = ReceiverStream::new(body_rx);
    let mut response = Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(KEEPALIVE_INTERVAL).text("ping"))
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

/// post-commit 定向唤醒：用已提交 agent_events 事实反查 `(kind, source
/// message)` 的 target Agent 再定向通知（不复制 targeting 规则）。查询或
/// 通知失败只记录带 canonical request_id 的脱敏 warn，已提交的事务与响应
/// 不受影响（唤醒是 best-effort 提示，耐久 Event 才是恢复来源）；notifier
/// panic 用最小隔离捕获（与 wake_humans 同一纪律）。
pub(crate) async fn wake_agents(
    state: &AppState,
    kind: AgentEventKind,
    message_id: &str,
    request_id: &str,
) {
    let Some(message_id) = MessageId::parse_canonical(message_id) else {
        tracing::warn!(
            operation = "backend_stream.wake_agents",
            error_kind = "domain_id_parse",
            request_id = %request_id,
            "post-commit agent wake skipped: message id failed canonical parse; durable events unaffected"
        );
        return;
    };
    let targets = match state.event.event_targets(&kind, &message_id).await {
        Ok(targets) => targets,
        Err(_) => {
            tracing::warn!(
                operation = "backend_stream.wake.targets",
                error_kind = "storage_unavailable",
                request_id = %request_id,
                "post-commit agent event wake target lookup failed; durable events unaffected"
            );
            return;
        }
    };
    if targets.is_empty() {
        return;
    }
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        state.backend_notifier.notify(&targets);
    }));
    if result.is_err() {
        tracing::warn!(
            operation = "backend_stream.wake.notify",
            error_kind = "notifier_panic",
            request_id = %request_id,
            "post-commit backend stream wake notifier panicked; durable events unaffected"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 门闩额度语义：同 Agent 第 3 条拒绝、跨 Agent 互不影响、Drop 精确回收。
    #[test]
    fn gate_counts_per_agent_and_releases_on_drop() {
        let gate = Arc::new(BackendStreamGate::new());
        let agent_a = ParticipantId("agent-a".into());
        let agent_b = ParticipantId("agent-b".into());

        let first = gate.acquire(&agent_a).expect("first stream allowed");
        let _second = gate.acquire(&agent_a).expect("second stream allowed");
        assert!(
            gate.acquire(&agent_a).is_none(),
            "third concurrent stream for same agent must be rejected"
        );
        assert!(
            gate.acquire(&agent_b).is_some(),
            "other agent is unaffected"
        );

        drop(first);
        assert!(
            gate.acquire(&agent_a).is_some(),
            "released slot is reusable"
        );
        // 语句结束即释放：此刻存活只有 _second（agent-b 临时与复检临时
        // 都已 drop 回收）。
        let inner = gate.inner.lock().unwrap();
        assert_eq!(inner.total, 1);
        assert_eq!(inner.per_agent.get("agent-a"), Some(&1));
        assert!(!inner.per_agent.contains_key("agent-b"));
    }

    /// 唤醒事件构造（wire 断言见 backend_stream_routes_tests：无事实）。
    #[test]
    fn wake_event_is_constructible() {
        let _event = wake_event();
    }
}
