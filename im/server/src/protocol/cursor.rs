//! 公共 opaque cursor 协议（契约 CORE_V1_MESSAGES.md 8.2 /
//! CORE_V1_HUMAN_EVENTS.md 6 / CORE_V1_AGENT_EVENTS.md 8.1）。
//!
//! 公共分页游标是「版本化 + 资源类型 + 稳定 ID」的服务端可解释字符串：
//!
//! - 版本前缀 `c1`：当前唯一支持版本；未知版本一律拒绝（400 invalid_request）；
//! - 资源标签：`m`（Message History 的 `before`）、`he`（Human Event 的
//!   `after`）、`be`（Backend/Agent Event 的 `after`）；
//! - 载荷：规范小写连字符 UUID v4，或起点哨兵 `start`（只对 `he`/`be`
//!   有效；`m` 只接受 UUID——Message 首屏以省略 `before` 表达，不需要
//!   可回传起点）。
//!
//! 全部字符都是 URL-safe（`[a-z0-9.-]`），无需 percent 编码，也无需签名：
//! 游标是服务端可解释、客户端不可依赖内部结构的版本化能力。它不包含
//! sequence、room、human、agent、membership 边界或任何凭据事实；服务端按
//! 当前 endpoint、认证身份与资源作用域把稳定 ID 解析回内部位置。
//! SQLite/ports 内部仍使用 i64 sequence，cursor 编码/解码只存在于
//! protocol/application 边界。

use crate::domain::agent_event::AgentEventId;
use crate::domain::human_event::HumanEventFactId;
use crate::domain::message::MessageId;

/// 当前游标协议版本前缀（唯一支持版本；结构变化时递增为新版本并放弃旧
/// 版本，旧版本一律 400 invalid_request）。
pub const CURSOR_VERSION: &str = "c1";

/// 起点哨兵载荷（`he`/`be` 专用）。
pub const CURSOR_START: &str = "start";

/// 游标资源类型：决定可使用的 endpoint 与解析作用域。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorResource {
    /// GET /v1/rooms/{room_id}/messages 的 `before`。
    Message,
    /// GET /v1/human/events 的 `after`。
    HumanEvent,
    /// GET /v1/backend/events 的 `after`。
    BackendEvent,
}

impl CursorResource {
    const fn tag(self) -> &'static str {
        match self {
            Self::Message => "m",
            Self::HumanEvent => "he",
            Self::BackendEvent => "be",
        }
    }
}

/// 游标载荷：稳定 UUID v4 或起点哨兵。
/// ⚠ 2026-09-28：原还有第三种「ACK 位置哨兵」（`c1.be.ack`），随
/// Completion/ACK 整体退役删除。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorPayload {
    /// 规范小写连字符 UUID v4（严格解析，非规范格式拒绝）。
    Id(String),
    /// 起点哨兵（`he`/`be` 专用；缺省 `after` 的确定性、可回传起点）。
    Start,
}

/// 已严格解析的公共游标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub resource: CursorResource,
    pub payload: CursorPayload,
}

impl Cursor {
    /// Message History `before` 游标：引用一条该 Room 内对读取者可见的
    /// 已提交 Message。
    pub fn message(id: &MessageId) -> Cursor {
        Cursor {
            resource: CursorResource::Message,
            payload: CursorPayload::Id(id.as_str().to_owned()),
        }
    }

    /// Human Event `after` 游标：引用该 Human 自己被投递的一条 fact。
    pub fn human_event(id: &HumanEventFactId) -> Cursor {
        Cursor {
            resource: CursorResource::HumanEvent,
            payload: CursorPayload::Id(id.as_str().to_owned()),
        }
    }

    /// Human Event 起点游标（缺省 `after` 时 next_after 返回的确定起点）。
    pub fn human_event_start() -> Cursor {
        Cursor {
            resource: CursorResource::HumanEvent,
            payload: CursorPayload::Start,
        }
    }

    /// Backend Event `after` 游标：引用该 Agent 自己的一条 Event。
    pub fn backend_event(id: &AgentEventId) -> Cursor {
        Cursor {
            resource: CursorResource::BackendEvent,
            payload: CursorPayload::Id(id.as_str().to_owned()),
        }
    }

    /// Backend Event 起点游标（缺省 `after` 时 next_after 返回的确定起点）。
    pub fn backend_event_start() -> Cursor {
        Cursor {
            resource: CursorResource::BackendEvent,
            payload: CursorPayload::Start,
        }
    }

    /// 严格解析：恰好三段（版本.资源.载荷）、版本恰为 `c1`、资源标签已
    /// 知、载荷为规范小写连字符 UUID v4（`m`）或 `start`（`he`/`be`）。
    /// 任何未知版本、未知标签、多余段、大小写/非规范 UUID 或格式偏差都
    /// 返回 None（HTTP 统一 400 invalid_request）。
    pub fn parse(raw: &str) -> Option<Cursor> {
        let mut parts = raw.split('.');
        let version = parts.next()?;
        let resource = parts.next()?;
        let payload = parts.next()?;
        if parts.next().is_some() {
            return None;
        }
        if version != CURSOR_VERSION {
            return None;
        }
        let resource = match resource {
            "m" => CursorResource::Message,
            "he" => CursorResource::HumanEvent,
            "be" => CursorResource::BackendEvent,
            _ => return None,
        };
        let payload = if payload == CURSOR_START {
            // 起点哨兵只对事件端点有效；Message 首屏以省略 before 表达。
            match resource {
                CursorResource::Message => return None,
                _ => CursorPayload::Start,
            }
        } else {
            if !is_canonical_uuid_v4(payload) {
                return None;
            }
            CursorPayload::Id(payload.to_owned())
        };
        Some(Cursor { resource, payload })
    }
}

/// 规范字符串形式（与服务端签发时完全一致；客户端应原样回传，通过
/// `to_string()` 获取）。
impl std::fmt::Display for Cursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.payload {
            CursorPayload::Id(id) => {
                write!(f, "{}.{}.{}", CURSOR_VERSION, self.resource.tag(), id)
            }
            CursorPayload::Start => {
                write!(
                    f,
                    "{}.{}.{}",
                    CURSOR_VERSION,
                    self.resource.tag(),
                    CURSOR_START
                )
            }
        }
    }
}

/// 严格规范 UUID v4：可解析、版本位为 4（随机）、且原文恰为小写连字符
/// 规范形式（拒绝大写、无连字符、非 v4 等其他表示）。
fn is_canonical_uuid_v4(raw: &str) -> bool {
    match uuid::Uuid::parse_str(raw) {
        Ok(uuid) => {
            (uuid.get_version() == Some(uuid::Version::Random))
                && uuid.hyphenated().to_string() == raw
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg_id() -> MessageId {
        MessageId("550e8400-e29b-41d4-a716-446655440000".into())
    }

    #[test]
    fn round_trip_formats_all_resource_types() {
        let message = Cursor::message(&msg_id());
        assert_eq!(
            message.to_string(),
            "c1.m.550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(Cursor::parse(&message.to_string()), Some(message.clone()));

        let human = Cursor::human_event(&HumanEventFactId(
            "550e8400-e29b-41d4-a716-446655440001".into(),
        ));
        assert_eq!(
            human.to_string(),
            "c1.he.550e8400-e29b-41d4-a716-446655440001"
        );
        assert_eq!(Cursor::parse(&human.to_string()), Some(human.clone()));

        let backend =
            Cursor::backend_event(&AgentEventId("550e8400-e29b-41d4-a716-446655440002".into()));
        assert_eq!(
            backend.to_string(),
            "c1.be.550e8400-e29b-41d4-a716-446655440002"
        );
        assert_eq!(Cursor::parse(&backend.to_string()), Some(backend.clone()));
    }

    #[test]
    fn start_sentinel_round_trips_for_events_only() {
        assert_eq!(Cursor::human_event_start().to_string(), "c1.he.start");
        assert_eq!(
            Cursor::parse("c1.he.start"),
            Some(Cursor::human_event_start())
        );
        assert_eq!(Cursor::backend_event_start().to_string(), "c1.be.start");
        assert_eq!(
            Cursor::parse("c1.be.start"),
            Some(Cursor::backend_event_start())
        );
        assert_eq!(Cursor::parse("c1.m.start"), None);
    }

    /// ⚠ 2026-09-28：原用例名 `ack_sentinel_round_trips_for_backend_events_only`
    /// ——ACK 位置哨兵随 Completion/ACK 整体退役删除，这里只剩「`ack` 不再是
    /// 合法载荷」这一条反向断言（哨兵不许复活）。
    #[test]
    fn ack_sentinel_is_no_longer_a_valid_payload() {
        assert_eq!(Cursor::parse("c1.be.ack"), None);
        assert_eq!(Cursor::parse("c1.he.ack"), None);
        assert_eq!(Cursor::parse("c1.m.ack"), None);
        for bad in ["c1.be.ackx", "c1.be.ACK", "c1.be.ack.extra", "ack"] {
            assert_eq!(Cursor::parse(bad), None, "must reject {bad:?}");
        }
    }

    #[test]
    fn parse_rejects_malformed_and_unknown_forms() {
        for bad in [
            "",
            "c1",
            "c1.m",
            "c1.m.550e8400-e29b-41d4-a716-446655440000.extra",
            "c1.he.start.extra",
            "c2.m.550e8400-e29b-41d4-a716-446655440000",
            "c0.m.550e8400-e29b-41d4-a716-446655440000",
            "v1.m.550e8400-e29b-41d4-a716-446655440000",
            "c1.x.550e8400-e29b-41d4-a716-446655440000",
            "c1.M.550e8400-e29b-41d4-a716-446655440000",
            "c1.m.not-a-uuid",
            "c1.m.550e8400e29b41d4a716446655440000",
            "c1.m.550E8400-E29B-41D4-A716-446655440000",
            "c1.m.{550e8400-e29b-41d4-a716-446655440000}",
            "c1.m.550e8400-e29b-51d4-a716-446655440000",
            "c1.m.550e8400-e29b-31d4-a716-446655440000",
            "c1.m.00000000-0000-0000-0000-000000000000",
            "c1.he.startt",
            "c1.be.START",
            " c1.he.start",
            "c1.he.start ",
            "c1.m.550e8400-e29b-41d4-a716-44665544000",
            "c1.m.550e8400-e29b-41d4-a716-4466554400000",
        ] {
            assert_eq!(Cursor::parse(bad), None, "must reject {bad:?}");
        }
    }

    #[test]
    fn parse_accepts_distinct_ids_and_rejects_tampered_version() {
        let a = Cursor::message(&MessageId("11111111-1111-4111-8111-111111111111".into()));
        let b = Cursor::message(&MessageId("22222222-2222-4222-8222-222222222222".into()));
        assert_ne!(a, b);
        assert_eq!(Cursor::parse(&a.to_string()), Some(a.clone()));
        assert_eq!(Cursor::parse(&b.to_string()), Some(b.clone()));
        // 版本不可伪造：改写版本段后必须拒绝。
        let tampered = format!("c2{}", &a.to_string()[2..]);
        assert_eq!(Cursor::parse(&tampered), None);
    }
}
