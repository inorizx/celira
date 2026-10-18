//! Agent Event 有界观察 HTTP handler（契约 CORE_V1_AGENT_EVENTS.md 8.x）。
//!
//! `GET /v1/backend/events` 只接受 Backend Credential（`BackendAuth`：
//! active Credential → active Binding → active Agent）；Agent 恒来自认证
//! 上下文，请求不能携带或覆盖 agent_id；Human Session token 一律 401。
//!
//! 严格 query 解析：只允许可选的 `after`（opaque cursor 字符串，缺省
//! 表示起点；语法与作用域解析在 application 层，`c1.be.ack` 从当前认证
//! Agent 的服务端 ACK 位置之后读取、无位置等价起点）与 `limit`（默认
//! 50、范围 1..=100）；未知、重复、非法或越界参数统一 400
//! invalid_request。GET 是纯读取，不建立或改写任何服务端状态。
//!
//! handler 只调用 `application::event` 用例与 `ports::event::EventStore`，
//! 不直接操作 SQLite。

use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use crate::adapters::http::AppState;
use crate::adapters::http::backend_auth::BackendAuth;
use crate::adapters::http::message_routes::project_message;
use crate::adapters::http::middleware::RequestId;
use crate::adapters::http::routes::{error_response, store_error_response};
use crate::application::event::{EventQueryError, list_agent_events};
use crate::domain::agent_event::AgentEventKind;
use crate::ports::event::EventWithMessage;
use crate::protocol::error::codes;
use crate::protocol::event::{
    BackendEventView, EVENT_KIND_MESSAGE_CREATED, EVENT_KIND_MESSAGE_RECALLED,
    ListBackendEventsResponse,
};

/// Agent Event 观察路由。
pub fn router() -> Router<AppState> {
    Router::new().route("/v1/backend/events", get(list_events))
}

/// 严格解析 events 查询参数：只允许可选的 `after` 与 `limit`，各自至多
/// 一次；未知 key、重复、空值统一 Err（HTTP 400 invalid_request）。
/// `after` 是 opaque cursor 字符串（非空；语义解析在 application 层）；
/// 与 Human Event REST 共用同一套原始 query 解码与参数裁决。
pub(crate) fn parse_events_query(pairs: &[(String, String)]) -> Result<(Option<String>, u32), ()> {
    let mut after: Option<String> = None;
    let mut limit: Option<u32> = None;
    for (key, value) in pairs {
        match key.as_str() {
            "after" => {
                if after.is_some() {
                    return Err(());
                }
                if value.is_empty() {
                    return Err(());
                }
                after = Some(value.clone());
            }
            "limit" => {
                if limit.is_some() {
                    return Err(());
                }
                let parsed: u32 = value.parse().map_err(|_| ())?;
                if !(1..=100).contains(&parsed) {
                    return Err(());
                }
                limit = Some(parsed);
            }
            _ => return Err(()),
        }
    }
    Ok((after, limit.unwrap_or(50)))
}

/// GET /v1/backend/events：当前认证 Agent 的 `after`（opaque cursor）之后
/// Event，严格按该 Agent 的 sequence 升序，最多 limit 条；重复拉取稳定
/// 无副作用。
async fn list_events(
    State(state): State<AppState>,
    auth: BackendAuth,
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
                "invalid events query parameters",
                &request_id,
            );
        }
    };

    match list_agent_events(
        state.event.as_ref(),
        &auth.view.agent.id,
        after.as_deref(),
        limit,
    )
    .await
    {
        Ok(outcome) => Json(ListBackendEventsResponse {
            events: outcome.events.iter().map(event_view).collect(),
            next_after: outcome.next_after,
            request_id,
        })
        .into_response(),
        Err(EventQueryError::InvalidRequest) => error_response(
            StatusCode::BAD_REQUEST,
            codes::INVALID_REQUEST,
            "invalid events query parameters",
            &request_id,
        ),
        Err(EventQueryError::Store(e)) => store_error_response("event.list", &e, &request_id),
    }
}

/// 严格解码原始 query，避免 Axum `Query` extractor 在 percent/UTF-8 解码
/// 失败时绕过 handler，产生没有统一错误信封与 request id 的框架默认响应。
pub(crate) fn parse_raw_events_query(raw_query: Option<&str>) -> Result<(Option<String>, u32), ()> {
    let mut pairs = Vec::new();
    for segment in raw_query.unwrap_or_default().split('&') {
        if segment.is_empty() {
            continue;
        }
        let (raw_key, raw_value) = segment.split_once('=').unwrap_or((segment, ""));
        pairs.push((
            decode_query_component(raw_key)?,
            decode_query_component(raw_value)?,
        ));
    }
    parse_events_query(&pairs)
}

fn decode_query_component(raw: &str) -> Result<String, ()> {
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = hex_value(bytes[index + 1])?;
                let low = hex_value(bytes[index + 2])?;
                decoded.push((high << 4) | low);
                index += 3;
            }
            b'%' => return Err(()),
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).map_err(|_| ())
}

fn hex_value(byte: u8) -> Result<u8, ()> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(()),
    }
}

/// Event 公共视图：source Message 复用既有 MessageView 形状（含
/// reply_to_message_id，普通消息为 null；不返回 target 正文、actor、
/// sequence、Room 或内部边界）与同一 tombstone 投影（message.created 与
/// message.recalled 在 source Message 已撤回时都呈现 tombstone，绝不回放
/// 原正文）；actor 是读取时当前 Participant 公共资料投影；kind 是协议
/// 判别值（message.created / message.recalled）。不返回 target Agent、
/// Binding、Credential、Owner、幂等键或任何私有事实；视图不返回任何
/// sequence。
fn event_view(item: &EventWithMessage) -> BackendEventView {
    let actor = &item.actor;
    let message = &item.message;
    BackendEventView {
        id: item.event.id.as_str().to_owned(),
        kind: match item.event.kind {
            AgentEventKind::Created => EVENT_KIND_MESSAGE_CREATED,
            AgentEventKind::Recalled => EVENT_KIND_MESSAGE_RECALLED,
        }
        .to_owned(),
        message: project_message(
            message,
            crate::protocol::message::MessageActorView {
                id: actor.id.as_str().to_owned(),
                kind: actor.kind.as_str().to_owned(),
                display_name: actor.display_name.as_str().to_owned(),
                status: actor.status.as_str().to_owned(),
            },
        ),
        created_at: item.event.created_at.to_rfc3339(),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_events_query, parse_raw_events_query};

    /// 与 serde_urlencoded 对 `Vec<(String, String)>` 的输出保持一致的
    /// 测试用解析（无 '+' 转义、无 percent 解码干扰的普通查询）。
    fn pairs(query: &str) -> Vec<(String, String)> {
        query
            .split('&')
            .filter(|part| !part.is_empty())
            .map(|part| {
                if let Some((key, value)) = part.split_once('=') {
                    (key.to_owned(), value.to_owned())
                } else {
                    (part.to_owned(), String::new())
                }
            })
            .collect()
    }

    #[test]
    fn strict_query_accepts_only_optional_after_and_limit() {
        assert_eq!(parse_events_query(&pairs("")).unwrap(), (None, 50));
        assert_eq!(
            parse_events_query(&pairs("after=c1.be.start")).unwrap(),
            (Some("c1.be.start".to_owned()), 50)
        );
        assert_eq!(
            parse_events_query(&pairs(
                "after=c1.be.550e8400-e29b-41d4-a716-446655440000&limit=7"
            ))
            .unwrap(),
            (
                Some("c1.be.550e8400-e29b-41d4-a716-446655440000".to_owned()),
                7
            )
        );
        assert_eq!(
            parse_events_query(&pairs("limit=100&after=c1.he.start")).unwrap(),
            (Some("c1.he.start".to_owned()), 100)
        );
        // after 只做形状校验：cursor 语义解析在 application 层。
        assert_eq!(
            parse_events_query(&pairs("after=anything")).unwrap(),
            (Some("anything".to_owned()), 50)
        );
    }

    #[test]
    fn strict_query_rejects_unknown_duplicate_illegal_and_empty_after() {
        for bad in [
            "foo=1",
            "after=1&foo=2",
            "after=1&after=2",
            "limit=1&limit=2",
            "after=1&limit=1&limit=2",
            "after=",
            "after=1&after",
            "limit=0",
            "limit=101",
            "limit=abc",
            "limit=",
            "limit=-1",
            "limit=4294967296",
        ] {
            assert!(
                parse_events_query(&pairs(bad)).is_err(),
                "must reject {bad:?}"
            );
        }
    }

    #[test]
    fn raw_query_rejects_invalid_percent_and_utf8_sequences() {
        for bad in [Some("after=%"), Some("after=%GG"), Some("after=%FF")] {
            assert!(parse_raw_events_query(bad).is_err(), "must reject {bad:?}");
        }
        assert_eq!(parse_raw_events_query(Some("after%3D7&limit%3D2")), Err(()));
        assert_eq!(
            parse_raw_events_query(Some("after=c1.be.start&limit=2")).unwrap(),
            (Some("c1.be.start".to_owned()), 2)
        );
    }
}
