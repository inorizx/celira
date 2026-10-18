//! 主体进行态窄端口（进程内、易失、非持久）。
//!
//! 权威契约：`docs/contracts/core-v1/CORE_V1_AGENT_PROTOCOL.md` §6。
//!
//! 这一层只回答两个 IM 问题，**且只回答这两个**：
//! - **在不在**（online）：这位参与者此刻还有没有一条活着的连接；
//! - **正在写**（typing）：这位参与者此刻是不是正在这间房起草一条消息。
//!
//! 三条纪律：
//! 1. **不新增写者**：连接事实由**连接本身**给出（Human 的 WS 订阅表、
//!    Agent 的门铃流计数、以及"最近一次被看见"），不需要任何一方定时
//!    上报心跳、不需要 instance id、不需要容量词表；
//! 2. **进程内易失**：绝不写 SQLite、绝不进 Human Event 表、绝不进
//!    `events.available`；Core 重启即失忆，这是契约不是缺陷；
//! 3. **零执行面**：没有阶段（thinking/working/…）、没有生成流 id、没有
//!    revision、没有草稿正文——执行过程不是 IM 事实（2026-09-28 主人裁决）。

use crate::domain::participant::ParticipantId;
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;

/// 主体进行态的进程内读写端口（唯一实现：`adapters::live::VolatileLive`）。
///
/// 时间一律由调用方注入（`UtcMillis`），实现保持确定性、可测。
pub trait LivePresence: Send + Sync {
    /// 「这个主体刚被看见过」：任何一次成功的 BackendAuth 都算被看见。
    /// 这是**拉取式 Backend**（不挂门铃）判"在不在"的唯一来源——它不是
    /// 心跳上报，而是认证这条既有路径的副产品。
    fn mark_seen(&self, participant: &ParticipantId, now: UtcMillis);

    /// 「我正在这间房写」：易失信号，`domain::live::TYPING_TTL` 之后自动
    /// 消失；调用方必须已经过 active membership 裁决。
    fn mark_typing(&self, room_id: &RoomId, participant: &ParticipantId, now: UtcMillis);

    /// 清除该主体在这间房的 typing（发出消息即取代进行态；重复清除幂等）。
    fn clear_typing(&self, room_id: &RoomId, participant: &ParticipantId);

    /// 该房此刻仍在写的主体（纯只读观察：按时间过滤，过期项不返回，
    /// 也**不**因此被删除）。
    fn typing_in_room(&self, room_id: &RoomId, now: UtcMillis) -> Vec<ParticipantId>;

    /// 该主体是否在 `domain::live::SEEN_TTL` 内被看见过。
    fn seen_recently(&self, participant: &ParticipantId, now: UtcMillis) -> bool;
}

/// 连接事实的只读查询（进程内）：由 Human Event WS 的订阅表与 Backend
/// 门铃的并发门闩实现。
///
/// 它问的是**连接**，不是"这个主体想不想被算作在线"——所以没有写入口，
/// 也没有"上报离线"这种事：连接断了，条目自己就没了。
pub trait ConnectionPresence: Send + Sync {
    /// 该主体此刻是否至少有一条活着的连接（Human：WS 订阅；Agent：门铃
    /// SSE 流）。identity 未知/从未连接一律 false。
    fn is_connected(&self, participant: &ParticipantId) -> bool;
}
