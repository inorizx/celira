//! Human Event 恢复协议模型（契约 CORE_V1_HUMAN_EVENTS.md 6.x / 7.x）。
//!
//! REST 响应每条事件携带发生时已授权的公开快照；快照是 SQLite 触发器在
//! 同一事务内固化的 JSON，本模块只定义协议信封形状，不建立第二套
//! Message/Membership/Owner DTO，也不验证快照内部结构（快照 schema 由
//! 契约文档逐 kind 定义，解析失败属于持久事实无法解析）。
//!
//! 公共协议不暴露任何 sequence：`after`/`next_after` 是 `protocol::cursor`
//! 定义的 opaque cursor（缺省 `after` 表示起点），`HumanEventView` 不返回
//! sequence。公共信封也不暴露内部 source 身份：`HumanEventFact.source_id`
//! 是内部事实列（advancement_id 等内部 ID 绝不出现在公共 HTTP JSON），
//! 客户端以 `id`（fact_id）形成 opaque cursor、以 `snapshot.id` 识别
//! Message/Membership/Transition 公开资源；`read.cursor.advanced` 快照
//! 只含 room_id 与 last_read_message_id，不含任何内部 ID。
//!
//! WebSocket 帧模型同文件定义：首帧 `authenticate` 认证、
//! `authenticated` ack 与 `events.available` 唤醒（只表达「可能有新事
//! 件」，不携带任何事实）。

use serde::{Deserialize, Serialize};

/// GET /v1/human/events 成功响应里的一条 Event。不返回任何 sequence
/// （恢复位置由 `next_after` opaque cursor 表达）；不返回内部 source
/// 身份（source_id 是内部事实列，公共面以 event.id / snapshot.id 识别）。
#[derive(Debug, Clone, Serialize)]
pub struct HumanEventView {
    /// 共享 fact ID（UUID v4）：也是 `after` opaque cursor 的载荷。
    pub id: String,
    /// 判别值：message.created / member.joined / member.left /
    /// owner.transferred / read.cursor.advanced。
    pub kind: String,
    /// source occurrence 所在 Room。
    pub room_id: String,
    /// 发生时已授权的公开快照（逐 kind schema 见
    /// CORE_V1_HUMAN_EVENTS.md 4.3；read.cursor.advanced 不含任何
    /// advancement ID）。
    pub snapshot: serde_json::Value,
    pub created_at: String,
}

/// GET /v1/human/events 成功响应：events 严格按该 Human 的 sequence
/// 升序；next_after 为 opaque cursor 字符串（非空页为最后一条 Event 的
/// cursor、空页为实际采用的 after cursor、缺省为起点 sentinel）；
/// next_after 只属于调用方本地分页状态，Core 不持久化，也不表示 ACK。
#[derive(Debug, Clone, Serialize)]
pub struct ListHumanEventsResponse {
    pub events: Vec<HumanEventView>,
    pub next_after: String,
    pub request_id: String,
}

/// WS 首帧认证请求（已冻结）：升级成功后客户端必须在 5 秒内发送
/// `{"type":"authenticate","token":"<human session token>"}`；`type` 必须
/// 恰好是 `authenticate`，`deny_unknown_fields` 拒绝任何多余字段。token
/// 只用于认证，绝不进入日志。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WsAuthFrame {
    #[serde(rename = "type")]
    pub kind: String,
    pub token: String,
}

/// WS 首帧认证帧的固定 `type` 值。
pub const WS_AUTHENTICATE: &str = "authenticate";

/// WS 认证成功后的 ack：服务端先建立该 Human 的订阅再发送本帧；客户端
/// 收到后即可开始 REST 补拉，不会留下订阅竞态。
pub const WS_AUTHENTICATED: &str = "authenticated";

/// WS 唤醒帧：只表达「该 Human 可能有新 Event，请 REST 补拉」，不携带
/// event、sequence、room 或其他事实；断线/漏唤醒/重启后仍由 REST 恢复。
pub const WS_EVENTS_AVAILABLE: &str = "events.available";

/// WS 认证失败/超时错误帧：复用统一错误信封（invalid_session + request_id）
/// 语义；发送后以 policy violation（1008）关闭。
#[derive(Debug, Clone, Serialize)]
pub struct WsErrorFrame<'a> {
    pub error: WsErrorDetail<'a>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WsErrorDetail<'a> {
    pub code: &'static str,
    pub message: &'a str,
    pub request_id: &'a str,
}

impl<'a> WsErrorFrame<'a> {
    pub fn invalid_session(request_id: &'a str) -> WsErrorFrame<'a> {
        WsErrorFrame {
            error: WsErrorDetail {
                code: crate::protocol::error::codes::INVALID_SESSION,
                message: "invalid session",
                request_id,
            },
        }
    }
}
