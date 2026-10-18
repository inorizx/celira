//! Human Event REST 恢复 + WebSocket volatile 唤醒 handler（契约
//! CORE_V1_HUMAN_EVENTS.md 6.x/7.x/8.x）。
//!
//! REST `GET /v1/human/events`：只接受 Human Session（CurrentAuth，Backend
//! Credential 一律 401）；严格 query 解析（复用 agent event 的同一套原始
//! query 解码）；纯读取无副作用；事件按当前 Human 的 sequence 严格升序，
//! 读取时按当前 Membership 过滤（唯一例外是读取者自己的 member.left）。
//!
//! WS `GET /v1/human/events/stream`：HTTP upgrade 后客户端必须在 5 秒内
//! 发送首个 text JSON 帧 `{"type":"authenticate","token":"<session-token>"}`
//! （deny_unknown_fields，`type` 必须恰好是 `authenticate`）；认证前不订阅、
//! 不发送唤醒、不泄露任何事实；认证成功后先建立该 Human 的订阅、再回
//! `{"type":"authenticated"}`，之后只发 `{"type":"events.available"}`（不带
//! event/sequence/room 或任何事实）。非法/超时认证发送安全错误帧
//! （invalid_session + request_id 语义）并以 policy violation（1008）关闭；
//! 认证存储失败同样只对外回安全文案，内部以 tracing error 记录脱敏原因与
//! request_id；token 绝不进入日志。Session 撤销后在下一次准备发送唤醒前
//! 重新认证并关闭；无事件时不要求轮询式 reauth。
//!
//! 每连接唤醒通道是有界容量 1 的合并提示：同一连接在已有待发送提示时，
//! 后续 notify 的重复提示被合并/丢弃（try_send Full 是合法 best-effort，
//! 不报业务失败，也不会产生无界积压）；通道已关闭（连接结束）才清理订阅。
//!
//! 本模块还提供 post-commit 定向唤醒助手 `wake_humans`：HTTP 成功路径在
//! 提交成功后用刚提交的完整 source 身份（kind + source_id）从已提交
//! delivery/fact 事实反查目标 Human 再定向通知；查询或通知失败只记录脱敏
//! warn，不得把已提交事务伪装失败。

use std::time::Duration;

use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade, close_code};
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::adapters::http::AppState;
use crate::adapters::http::auth::CurrentAuth;
use crate::adapters::http::event_routes::parse_raw_events_query;
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::routes::{error_response, store_error_response};
use crate::application::human_event::{HumanEventQueryError, list_human_events};
use crate::application::identity;
use crate::domain::human_event::HumanEventSource;
use crate::ports::human_event::HumanEventWithFact;
use crate::protocol::error::codes;
use crate::protocol::human_event::{
    HumanEventView, ListHumanEventsResponse, WS_AUTHENTICATE, WS_AUTHENTICATED,
    WS_EVENTS_AVAILABLE, WsAuthFrame, WsErrorFrame,
};

/// WS 首帧认证超时（HTTP upgrade 后 5 秒内必须发送首个 text JSON 帧）。
const WS_AUTH_TIMEOUT: Duration = Duration::from_secs(5);

/// WS 认证帧体上限（token 为一次性 Session token，4 KiB 足够）。
const WS_MAX_FRAME_BYTES: usize = 4096;

/// Human Event 路由：REST 恢复 + WS 唤醒。
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/human/events", get(list_events))
        .route("/v1/human/events/stream", get(stream_route))
}

/// GET /v1/human/events：当前认证 Human 的 `after`（opaque cursor）之后
/// Event，严格按该 Human 的 delivery sequence 升序，最多 limit 条；重复
/// 拉取稳定无副作用；读取时按当前 Membership 过滤（唯一例外是读取者
/// 自己的 member.left）。
/// `after` 是 opaque cursor（缺省表示起点），按当前 Human 作用域解析。
async fn list_events(
    State(state): State<AppState>,
    auth: CurrentAuth,
    RawQuery(raw_query): RawQuery,
    request_id: RequestId,
) -> Response {
    let request_id = request_id.0;
    let (after, limit) = match parse_raw_events_query(raw_query.as_deref()) {
        Ok(pair) => pair,
        Err(()) => {
            return error_response(
                StatusCode::BAD_REQUEST,
                codes::INVALID_REQUEST,
                "invalid human events query parameters",
                &request_id,
            );
        }
    };

    match list_human_events(
        state.human_events.as_ref(),
        &auth.participant.id,
        after.as_deref(),
        limit,
    )
    .await
    {
        Ok(outcome) => {
            let mut events = Vec::with_capacity(outcome.events.len());
            for item in &outcome.events {
                match human_event_view(item) {
                    Ok(view) => events.push(view),
                    Err(_) => {
                        tracing::error!(
                            operation = "human_event.list",
                            error_kind = "decode_or_corrupt",
                            request_id = %request_id,
                            "human event store data unparseable"
                        );
                        return error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            codes::INTERNAL_ERROR,
                            "human event store data unparseable",
                            &request_id,
                        );
                    }
                }
            }
            Json(ListHumanEventsResponse {
                events,
                next_after: outcome.next_after,
                request_id,
            })
            .into_response()
        }
        Err(HumanEventQueryError::InvalidRequest) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "invalid human events query parameters",
            &request_id,
        ),
        Err(HumanEventQueryError::Store(e)) => {
            store_error_response("human_event.list", &e, &request_id)
        }
    }
}

/// Human Event 公共视图：快照是发生时已授权的公开 JSON（触发器固化），
/// 解析失败视为持久事实无法解析。视图不返回任何 sequence（恢复位置由
/// next_after opaque cursor 表达），也不返回内部 source 身份（source_id
/// 是内部事实列，公共面以 event.id / snapshot.id 识别；read.cursor.advanced
/// 的 advancement_id 属内部 ID，绝不进入公共投影）。
fn human_event_view(item: &HumanEventWithFact) -> Result<HumanEventView, String> {
    let snapshot = serde_json::from_str(&item.fact.snapshot).map_err(|e| {
        format!(
            "stored human event snapshot unparseable (fact {}): {e}",
            item.fact.id.as_str()
        )
    })?;
    Ok(HumanEventView {
        id: item.fact.id.as_str().to_owned(),
        kind: item.fact.kind.as_str().to_owned(),
        room_id: item.fact.room_id.as_str().to_owned(),
        snapshot,
        created_at: item.fact.created_at.to_rfc3339(),
    })
}

/// GET /v1/human/events/stream：WebSocket 首帧认证 + volatile 唤醒。
async fn stream_route(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
    request_id: RequestId,
) -> Response {
    ws.max_message_size(WS_MAX_FRAME_BYTES)
        .on_upgrade(move |socket| stream_ws(socket, state, request_id.0))
}

/// 连接任务：等待首帧认证（5 秒）→ 订阅 → authenticated ack → 唤醒循环。
async fn stream_ws(mut socket: WebSocket, state: AppState, request_id: String) {
    // 1. 首个 text JSON 帧必须在 5 秒内到达且必须恰好是
    //    `{"type":"authenticate","token":...}`（deny_unknown_fields，type
    //    值不匹配同样视为格式失败）；超时/关闭/非文本/解析失败/格式失败
    //    一律安全错误 + policy violation 关闭，认证前零泄露。格式类失败
    //    不是存储故障，不产生 error 日志。
    let frame = match timeout(WS_AUTH_TIMEOUT, socket.recv()).await {
        Ok(Some(Ok(Message::Text(text)))) => text,
        _ => {
            auth_fail_and_close(&mut socket, &request_id).await;
            return;
        }
    };
    let token = match serde_json::from_str::<WsAuthFrame>(frame.as_str()) {
        Ok(frame) if frame.kind == WS_AUTHENTICATE && !frame.token.is_empty() => frame.token,
        _ => {
            auth_fail_and_close(&mut socket, &request_id).await;
            return;
        }
    };

    // 2. 认证：token 只用于认证，绝不进入日志。存储故障对外同样只回安全
    //    invalid_session/1008，但内部必须以 error 级记录脱敏原因与
    //    request_id（可观察）；格式失败/未知 token 是认证失败而非存储
    //    故障，不需要 error 日志。
    let context = match identity::authenticate(state.identity.as_ref(), &token).await {
        Ok(context) => context,
        Err(identity::AuthenticateError::Store(_)) => {
            tracing::error!(
                operation = "human_event.ws.authenticate",
                error_kind = "storage_unavailable",
                request_id = %request_id,
                "identity store failure during WS human session authentication"
            );
            auth_fail_and_close(&mut socket, &request_id).await;
            return;
        }
        Err(_) => {
            auth_fail_and_close(&mut socket, &request_id).await;
            return;
        }
    };

    // 3. 先建立该 Human 的订阅，再回 authenticated ack：客户端收到 ack 后
    //    开始 REST 补拉，不会留下订阅竞态（订阅前发生的唤醒由 REST 恢复
    //    覆盖）。每连接唤醒通道有界容量 1：notifier 只持有弱引用，连接
    //    存活期内本任务保持强引用，任务结束时强引用 drop，订阅在下一次
    //    notify 惰性清理。
    let (wake_tx, mut wake_rx) =
        mpsc::channel::<()>(crate::adapters::notifier::WAKE_CHANNEL_CAPACITY);
    let wake = std::sync::Arc::new(wake_tx);
    state
        .notifier
        .subscribe(&context.participant.id, wake.clone());
    let ack = format!("{{\"type\":\"{WS_AUTHENTICATED}\"}}");
    if socket
        .send(Message::Text(Utf8Bytes::from(ack)))
        .await
        .is_err()
    {
        return;
    }

    // 4. 唤醒循环：只发 events.available（不携带任何事实）；发送前重新
    //    认证（Session 撤销后在下一次唤醒前关闭，无事件时不轮询 reauth）；
    //    存储失败同第 2 步：对外安全文案、内部 error 日志带 request_id；
    //    客户端关闭/断开即退出，订阅随 sender drop 惰性清理。
    loop {
        tokio::select! {
            _ = wake_rx.recv() => {
                match identity::authenticate(state.identity.as_ref(), &token).await {
                    Ok(_) => {
                        let wake = format!("{{\"type\":\"{WS_EVENTS_AVAILABLE}\"}}");
                        if socket
                            .send(Message::Text(Utf8Bytes::from(wake)))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(identity::AuthenticateError::Store(_)) => {
                        tracing::error!(
                            operation = "human_event.ws.wake_reauth",
                            error_kind = "storage_unavailable",
                            request_id = %request_id,
                            "identity store failure during WS wake re-authentication"
                        );
                        auth_fail_and_close(&mut socket, &request_id).await;
                        return;
                    }
                    Err(_) => {
                        close_policy(&mut socket).await;
                        return;
                    }
                }
            }
            frame = socket.recv() => match frame {
                None | Some(Err(_)) => return,
                Some(Ok(Message::Close(_))) => return,
                // 认证后的其他帧无契约语义，忽略（不泄露任何事实）。
                Some(Ok(_)) => {}
            },
        }
    }
}

/// 认证失败：先发送安全错误帧（invalid_session + request_id），再以
/// policy violation（1008）关闭；任何失败都 best-effort。Live WS 复用。
pub(crate) async fn auth_fail_and_close(socket: &mut WebSocket, request_id: &str) {
    let error = serde_json::to_string(&WsErrorFrame::invalid_session(request_id))
        .unwrap_or_else(|_| "{\"error\":{}}".to_owned());
    let _ = socket.send(Message::Text(Utf8Bytes::from(error))).await;
    close_policy(socket).await;
}

/// 以 policy violation（1008）关闭。Live WS 复用。
pub(crate) async fn close_policy(socket: &mut WebSocket) {
    let _ = socket
        .send(Message::Close(Some(axum::extract::ws::CloseFrame {
            code: close_code::POLICY,
            reason: Utf8Bytes::from("policy violation"),
        })))
        .await;
}

/// post-commit 定向唤醒（契约 8.x）：用刚提交的完整 source 身份
/// （kind + source_id）从**已提交**的 per-Human delivery / shared fact
/// 事实反查目标 Human 再定向通知；不在 Rust 层复制 targeting 规则。
/// 同一 source_id 在不同 kind 下是不同 fact（migration 009 的
/// `(kind, source_id)` 唯一），查询必须同时匹配 kind 与 source_id，否则
/// 可能错误唤醒拥有同值 source_id 的其他 Human。查询或通知失败只记录
/// 带 canonical request_id 的脱敏 warn，已提交的事务与响应不受影响
/// （唤醒是 best-effort 提示，耐久 Event 才是恢复来源）。notifier panic
/// 用最小隔离捕获（不得引入任务框架）：panic 不翻转已提交成功，只留下
/// 固定安全告警。
pub(crate) async fn wake_humans(state: &AppState, sources: &[HumanEventSource], request_id: &str) {
    if sources.is_empty() {
        return;
    }
    let targets = match state.human_events.delivery_targets(sources).await {
        Ok(targets) => targets,
        Err(_) => {
            tracing::warn!(
                operation = "human_event.wake.delivery_targets",
                error_kind = "storage_unavailable",
                request_id = %request_id,
                "post-commit human event wake target lookup failed; durable events unaffected"
            );
            return;
        }
    };
    if !targets.is_empty() {
        // notifier 是纯易失提示：即使实现方意外 panic，也绝不允许它翻转
        // 已提交成功；catch_unwind 只负责隔离并记录固定安全分类。
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.notifier.notify(&targets);
        }));
        if result.is_err() {
            tracing::warn!(
                operation = "human_event.wake.notify",
                error_kind = "notifier_panic",
                request_id = %request_id,
                "post-commit wake notifier panicked; durable events unaffected"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::adapters::http::event_routes::parse_events_query;

    #[test]
    fn parse_events_query_rejects_unknown_duplicate_illegal_and_empty_after() {
        for bad in [
            "foo=1",
            "after=1&foo=2",
            "after=1&after=2",
            "limit=1&limit=2",
            "after=",
            "after=1&limit=2&limit=3",
            "limit=0",
            "limit=101",
            "limit=abc",
            "limit=4294967296",
        ] {
            let pairs: Vec<(String, String)> = bad
                .split('&')
                .filter(|p| !p.is_empty())
                .map(|p| {
                    if let Some((k, v)) = p.split_once('=') {
                        (k.to_owned(), v.to_owned())
                    } else {
                        (p.to_owned(), String::new())
                    }
                })
                .collect();
            assert!(parse_events_query(&pairs).is_err(), "must reject {bad:?}");
        }
        assert_eq!(parse_events_query(&[]).unwrap(), (None, 50));
    }
}
