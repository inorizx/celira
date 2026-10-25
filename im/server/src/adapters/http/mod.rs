//! HTTP adapter：Resource API、错误映射、认证入口。
//!
//! handler 只调用 `application` 用例和 `ports` 端口，不直接操作 SQLite。

pub mod agent_routes;
pub mod attachment_routes;
pub mod attention_routes;
pub mod auth;
pub mod backend_auth;
pub mod backend_block_routes;
pub mod backend_contact_routes;
pub mod backend_directory_routes;
pub mod backend_group_routes;
pub mod backend_leave_routes;
pub mod backend_profile_routes;
pub mod backend_read_cursor_routes;
pub mod backend_read_routes;
pub mod backend_routes;
pub mod backend_stream_routes;
pub mod call_routes;
pub mod contact_routes;
pub mod event_routes;
pub mod home_routes;
pub mod human_event_routes;
mod human_profile_routes;
pub mod live_routes;
pub mod message_routes;
pub mod middleware;
pub mod participant_avatar_routes;
pub mod participant_presentation_routes;
pub mod room_profile_routes;
pub mod room_routes;
pub mod routes;

#[cfg(test)]
mod agent_routes_tests;
#[cfg(test)]
mod attachment_routes_tests;
#[cfg(test)]
mod attention_routes_tests;
#[cfg(test)]
mod backend_block_routes_tests;
#[cfg(test)]
mod backend_contact_routes_tests;
#[cfg(test)]
mod backend_directory_routes_tests;
#[cfg(test)]
mod backend_group_routes_tests;
#[cfg(test)]
mod backend_leave_routes_tests;
#[cfg(test)]
mod backend_read_cursor_routes_tests;
#[cfg(test)]
mod backend_read_routes_tests;
#[cfg(test)]
mod backend_routes_tests;
#[cfg(test)]
mod backend_stream_routes_tests;
#[cfg(test)]
mod call_routes_tests;
#[cfg(test)]
mod contact_routes_tests;
#[cfg(test)]
mod event_routes_tests;
#[cfg(test)]
mod home_routes_a2_tests;
#[cfg(test)]
mod human_profile_routes_tests;
#[cfg(test)]
mod live_routes_tests;
#[cfg(test)]
mod message_routes_tests;
#[cfg(test)]
mod participant_avatar_routes_tests;
#[cfg(test)]
mod participant_presentation_routes_tests;
#[cfg(test)]
mod recall_routes_tests;
#[cfg(test)]
mod room_profile_routes_tests;
#[cfg(test)]
mod room_routes_tests;
#[cfg(test)]
mod routes_tests;

use std::sync::Arc;

use axum::Router;

use crate::ports::agent::AgentStore;
use crate::ports::attachment::AttachmentStore;
use crate::ports::backend::BackendStore;
use crate::ports::blob::BlobStorage;
use crate::ports::call::CallStore;
use crate::ports::contact::ContactStore;
use crate::ports::event::{BackendEventNotifier, EventStore};
use crate::ports::home::HomeStore;
use crate::ports::human_event::{HumanEventNotifier, HumanEventStore};
use crate::ports::identity::IdentityStore;
use crate::ports::live::{ConnectionPresence, LivePresence};
use crate::ports::message::MessageStore;
use crate::ports::migration::MigrationProbe;
use crate::ports::participant_presentation::ParticipantPresentationStore;
use crate::ports::room::RoomStore;

/// 应用状态：由 composition root 组装并注入。
#[derive(Clone)]
pub struct AppState {
    /// 就绪探测端口。HTTP handler 通过它调用 readiness 用例。
    pub probe: Arc<dyn MigrationProbe>,
    /// 身份与认证端口。认证提取器与受保护 handler 通过它调用身份用例。
    pub identity: Arc<dyn IdentityStore>,
    /// Agent 生命周期端口。Agent handler 通过它调用 Agent 用例。
    pub agent: Arc<dyn AgentStore>,
    /// Room 端口。Room handler 通过它调用 Room 用例。
    pub room: Arc<dyn RoomStore>,
    /// Message 端口。Message handler 通过它调用 Message 用例。
    pub message: Arc<dyn MessageStore>,
    /// Backend Binding / Credential 端口。Backend handler 与 Backend
    /// 认证提取器通过它调用 Backend 用例。
    pub backend: Arc<dyn BackendStore>,
    /// Agent Event 端口。Backend Event handler 通过它调用 Event 用例。
    pub event: Arc<dyn EventStore>,
    /// Contact 端口。Contact handler 通过它调用 Contact 用例。
    pub contact: Arc<dyn ContactStore>,
    /// Human Event 端口。Human Event REST/WS handler 与 post-commit 唤醒
    /// 助手通过它读取耐久事实与唤醒目标。
    pub human_events: Arc<dyn HumanEventStore>,
    /// Volatile 唤醒 notifier。WS 连接订阅；HTTP 成功路径 post-commit 定向
    /// 唤醒；不写 SQLite。
    pub notifier: Arc<dyn HumanEventNotifier>,
    /// 进程内主体进行态（`last_seen` + `typing`；易失、非持久、有界，绝不
    /// 写 SQLite）。读面（两条 live 快照端点）与两条 typing 写入口共用。
    pub live: Arc<dyn LivePresence>,
    /// Human 连接事实（只读）：这位 Human 此刻还有没有活着的 Human Event
    /// WS。与 `notifier` 是**同一个对象**的窄接口句柄——在线就是连接在。
    pub human_connections: Arc<dyn ConnectionPresence>,
    /// Agent 连接事实（只读）：这位 Agent 的 Backend 此刻还挂着门铃 SSE 吗。
    /// 与 `backend_streams` 是同一个对象的窄接口句柄。
    pub backend_connections: Arc<dyn ConnectionPresence>,
    /// Backend SSE 唤醒流独立 notifier（窄解冻切片 §8；与 `notifier` 语义
    /// 同构但实例独立：Agent Event 变化只唤醒 Backend 门铃的
    /// `events.available`，绝不进入 Human Event 表或 Human WS 语义）。
    pub backend_notifier: Arc<dyn BackendEventNotifier>,
    /// Backend SSE 唤醒流并发门闩（进程内、纯易失：每 Agent ≤2、总量硬
    /// 上限；不写 SQLite）。
    pub backend_streams: Arc<backend_stream_routes::BackendStreamGate>,
    /// 附件元数据端口。附件上传/下载 handler 通过它建立 pending 行、裁决
    /// 下载授权与启动 sweep 引用集合。
    pub attachments: Arc<dyn AttachmentStore>,
    /// Blob 字节存储端口。附件上传/下载 handler 通过它读写字节；不接触
    /// SQLite。
    pub blob: Arc<dyn BlobStorage>,
    /// Home / Pairing Grant / Session 管理端口（A 包切片）。Home 身份
    /// 读取、Grant 签发/列表/撤销/兑换与 Session 列表/撤销 handler 通过
    /// 它调用对应用例。
    pub home: Arc<dyn HomeStore>,
    /// Participant Presentation 端口。Profile / Room Presentation 读写
    /// handler 通过它调用本切片用例（授权在事务内先于重放）。
    pub presentation: Arc<dyn ParticipantPresentationStore>,
    /// Call 端口。进程内、有界、可过期、非持久实时协调（Call 状态机 +
    /// 信令中继 + call-stream 定向送达）；绝不写 SQLite，媒体由两端
    /// WebRTC P2P 直连，Core 不接触媒体字节。
    pub call: Arc<dyn CallStore>,
    /// Pairing Grant TTL（分钟）：配置给定的签发默认，1..=60。
    pub grant_ttl_minutes: u32,
}

/// 组装 HTTP 路由。
///
/// 独立于监听与启动，便于测试直接对 Router 发起请求。
#[allow(clippy::too_many_arguments)]
pub fn build_router(
    probe: Arc<dyn MigrationProbe>,
    identity: Arc<dyn IdentityStore>,
    agent: Arc<dyn AgentStore>,
    room: Arc<dyn RoomStore>,
    message: Arc<dyn MessageStore>,
    backend: Arc<dyn BackendStore>,
    event: Arc<dyn EventStore>,
    contact: Arc<dyn ContactStore>,
    human_events: Arc<dyn HumanEventStore>,
    live: crate::adapters::live::LiveWiring,
    attachments: Arc<dyn AttachmentStore>,
    blob: Arc<dyn BlobStorage>,
    home: Arc<dyn HomeStore>,
    presentation: Arc<dyn ParticipantPresentationStore>,
    call: Arc<dyn CallStore>,
    grant_ttl_minutes: u32,
) -> Router {
    let state = AppState {
        probe,
        identity,
        agent,
        room,
        message,
        backend,
        event,
        contact,
        human_events,
        notifier: live.human_notifier(),
        live: live.presence(),
        human_connections: live.human_connections(),
        backend_connections: live.backend_connections(),
        // Backend SSE 门铃（窄解冻切片 §8；2026-09-28 起门闩由接线束给出，
        // 这样"谁挂着门铃流"这一事实只有一个来源）。
        backend_notifier: Arc::new(crate::adapters::notifier::VolatileHumanEventNotifier::new()),
        backend_streams: live.streams(),
        attachments,
        blob,
        home,
        presentation,
        call,
        grant_ttl_minutes,
    };
    Router::new()
        .merge(routes::router())
        .layer(axum::middleware::from_fn(middleware::request_context))
        .with_state(state)
}
