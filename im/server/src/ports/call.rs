//! Call 窄端口（进程内、有界、可过期、非持久实时协调）。
//!
//! 权威契约：CORE_V1_CALL.md（Core v1 窄解冻切片）。
//!
//! 本端口只声明 Call 切片的临时协调边界，与 Agent Live State 同质：
//! - 一切状态只存在于 Core 进程内（never SQLite / History / Message / Human
//!   Event），Core 重启即失忆（fail closed 404/流重置）；
//! - Call 状态机（ringing → in_progress → ended）与信令中继（SDP/ICE）都是
//!   临时、有界、可过期的控制事实，绝不跨入冻结持久契约；
//! - 媒体（音频）是两端客户端 WebRTC P2P 直连，**Core 永不接触媒体字节**；
//! - 时间由调用方注入（`UtcMillis`），实现保持确定性、可测；
//! - 硬上限（active Call 数）达到上限 fail closed（`LimitReached`）；
//! - Call-stream 每连接订阅有界容量的定向送达通道：信令/振铃/状态只投给
//!   对端当前已在线、持有效订阅的连接；对端无连接或通道满则 best-effort
//!   丢弃（fail closed，客户端以 REST 重同步、丢弃的迟发信令由 RTC 重协商/
//!   客户端重发兜底）。
//!
//! 本端口不感知 Human Session / Backend Credential / Binding / token；一切
//! 参与方身份与 room 成员关系由 application 层经既有 Room 端口裁决，本端口
//! 只持有 Participant ID 键的临时状态，并按参与方 ID 定向送达。

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::domain::call::{Call, CallId, CallMedia, SignalBody};
use crate::domain::participant::{DisplayName, ParticipantId};
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;

/// 一条要投递给 Call-stream 订阅的临时事件（振铃 / 对端信令 / 状态变更）。
/// 只在进程内、只投给当前在线的该参与方连接；绝不序列化进 SQLite/日志正文。
#[derive(Debug, Clone)]
pub enum CallStreamEvent {
    /// 本端被呼入（仅 callee 收到）。
    Ring { call: Call },
    /// 收到对端信令（仅对端方向，`from` 是发送方 Participant）。信令同步
    /// 即时中继：立即投递给对端当前在线订阅；对端无连接则静默丢弃。
    Signal {
        call_id: CallId,
        from: ParticipantId,
        body: SignalBody,
    },
    /// Call 状态/结束原因变更（caller 与 callee 双方收到）。
    StateChanged { call: Call },
}

/// 一次写操作的裁决结果。
#[derive(Debug, Clone)]
pub enum CallError {
    /// 未知 / 非本 Call 参与方 / 角色不匹配（对外统一 404 call_not_found，
    /// 不泄露存在性或归属）。
    NotFound,
    /// 当前状态不允许该动作（accept 非 ringing、已 ended 再操作等；409
    /// call_state_conflict）。
    StateConflict,
    /// Caller 或 Callee 已在另一 active Call（409 call_party_busy；仅同
    /// Room 成员间）。
    PartyBusy,
    /// 进程内激活 Call 达硬上限（503 call_state_limit_exceeded）。
    LimitReached,
}

/// 发起结果（沿用「同 (room, caller, callee) 已有 active Call 复用」近似
/// 无状态幂等，避免重复振铃）。
#[derive(Debug, Clone)]
pub struct InitiateOutcome {
    pub call: Call,
    pub created: bool,
}

/// 动作（accept/decline/hangup）结果。
#[derive(Debug, Clone)]
pub struct ActionOutcome {
    pub call: Call,
}

/// 信令中继结果：`delivered` 表示是否成功投递给对端当前在线订阅（false =
/// 对端无连接/通道满，best-effort 静默丢弃）。
#[derive(Debug, Clone)]
pub struct SignalOutcome {
    pub delivered: bool,
}

/// 进程内 Call 存储端口（窄接口）。实现是独立进程内 adapter，不扩
/// SQLite repository。
pub trait CallStore: Send + Sync {
    /// 发起新 Call（caller → callee，同 room）：新 Call 建立则生成
    /// `CallId`、置 `ringing` 并对 callee 发送 `Ring`；同 (room, caller,
    /// callee) 已有 active Call 则复用（created=false）。Caller 或 Callee
    /// 已在另一 active Call → PartyBusy。
    /// 参数多 = 一次发起所需的完整事实面；收敛参数结构体属设计变更，不在 lint 清零范围。
    #[allow(clippy::too_many_arguments)]
    fn initiate(
        &self,
        caller_id: &ParticipantId,
        caller_display: &DisplayName,
        callee_id: &ParticipantId,
        callee_display: &DisplayName,
        room_id: &RoomId,
        media: CallMedia,
        now: UtcMillis,
    ) -> Result<InitiateOutcome, CallError>;

    /// callee 接受：`ringing → in_progress`。非 callee 或未知 Call →
    /// NotFound；非 ringing → StateConflict。
    fn accept(
        &self,
        call_id: &CallId,
        actor_id: &ParticipantId,
        now: UtcMillis,
    ) -> Result<ActionOutcome, CallError>;

    /// callee 拒绝：`ringing → ended(decline_callee)`。
    fn decline(
        &self,
        call_id: &CallId,
        actor_id: &ParticipantId,
        now: UtcMillis,
    ) -> Result<ActionOutcome, CallError>;

    /// 任一方挂断/取消：`ringing | in_progress → ended(...)`。
    fn hangup(
        &self,
        call_id: &CallId,
        actor_id: &ParticipantId,
        now: UtcMillis,
    ) -> Result<ActionOutcome, CallError>;

    /// 中继信令至对方当前在线订阅。方向强制（契约 §4.2）：offer 只可
    /// caller→callee、answer 只可 callee→caller、ice 双向 trickle；参与方之一
    /// 才可发送，方向/角色错误或终态（ended）后发送一律 `StateConflict`。
    fn signal(
        &self,
        call_id: &CallId,
        from_id: &ParticipantId,
        body: &SignalBody,
        now: UtcMillis,
    ) -> Result<SignalOutcome, CallError>;

    /// 读取 Call（纯观察；非参与方不得经本端口取得 Call，存在性裁决在
    /// application 层完成）。
    fn get(&self, call_id: &CallId) -> Option<Call>;

    /// 单一后台清理器的全局 ring 超时 sweep：原子把过 TTL 的 `ringing`
    /// Call 置 `ended(timeout_ring)` 并向双方发送 `StateChanged`。返回本次
    /// 超时清除的 Call 数。
    fn sweep_expired(&self, now: UtcMillis) -> usize;

    /// 订阅某个参与方的 Call-stream 送达通道（一个连接一个有界容量的
    /// channel；只持有弱引用，连接的强引用由订阅方保持，断开时惰性清理）。
    fn subscribe(&self, participant_id: &ParticipantId, sender: Arc<mpsc::Sender<CallStreamEvent>>);
}
