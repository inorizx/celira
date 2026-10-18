//! Call 用例：授权编排 + 状态与信令协调。
//!
//! 权威契约：CORE_V1_CALL.md（Core v1 窄解冻切片）。
//!
//! 本层组合两个窄端口：
//! - `RoomStore::get_room_members`（只读裁决：Caller 为 Human 且必须存在
//!   active Membership、Callee 同 room 且为 active 成员；未知/非成员统一
//!   `RoomNotFound`，不泄露存在性）；
//! - `CallStore`（进程内、有界、可过期；不碰 SQLite；附带 call-stream 定向
//!   送达）。
//!
//! Caller 是本切片内唯一发起方（Human）；Callee 可为 Human 或 Agent。媒体
//! 是两端客户端 WebRTC P2P 直连，本层只为两方协调状态并中继纯信令控制消息
//! （SDP/ICE）。一切对外错误保持统一形状：未知/角色不匹配一律 `CallNotFound`
//! （不泄露其他 Call/参与方的存在性），忙碌为 `PartyBusy`，状态冲突为
//! `StateConflict`，载荷越界为 `InvalidPayload`，上限为 `LimitReached`。

use crate::domain::call::{Call, CallId, SignalBody};
use crate::domain::participant::{ParticipantId, ParticipantStatus};
use crate::domain::room::{MembershipStatus, RoomId};
use crate::domain::time::UtcMillis;
use crate::ports::call::{CallError as CallStoreError, CallStore};
use crate::ports::room::{RoomStore, RoomStoreError};
use crate::ports::storage::StorageError;

/// Call 用例错误。
#[derive(Debug)]
pub enum CallError {
    /// 请求形状/值非法（HTTP 400 invalid_request；如 callee == caller）。
    InvalidRequest(String),
    /// 发起：unknown/bad-id/非成员/Callee 非 active 成员，统一 404
    /// room_not_found（不泄露存在性）。
    RoomNotFound,
    /// 未知 / 非本 Call 参与方 / 角色不匹配：统一 404 call_not_found
    /// （不泄露存在性）。
    CallNotFound,
    /// 当前状态不允许该动作（accept 非 ringing、已 ended 再操作等；409
    /// call_state_conflict）。
    StateConflict,
    /// Caller 或 Callee 已在另一 active Call（409 call_party_busy）。
    PartyBusy,
    /// 进程内激活 Call 达硬上限（503 call_state_limit_exceeded）。
    LimitReached,
    /// 存储不可用（复用既有 500/503 分类）。
    Store(StorageError),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(m) => write!(f, "invalid call request: {m}"),
            Self::RoomNotFound => write!(f, "no such room for the caller/callee membership"),
            Self::CallNotFound => write!(f, "no such call for the authenticated participant"),
            Self::StateConflict => write!(f, "call state conflict"),
            Self::PartyBusy => write!(f, "call party is already in an active call"),
            Self::LimitReached => write!(f, "call store at capacity"),
            Self::Store(e) => write!(f, "call store failure: {e}"),
        }
    }
}

impl std::error::Error for CallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(e) => Some(e),
            _ => None,
        }
    }
}

fn room_store_error(e: RoomStoreError) -> CallError {
    match e {
        RoomStoreError::Database(storage) => CallError::Store(storage),
        other => CallError::Store(crate::application::unexpected_store_error(
            "call.room",
            other,
        )),
    }
}

fn map_store_error(e: CallStoreError) -> CallError {
    match e {
        CallStoreError::NotFound => CallError::CallNotFound,
        CallStoreError::StateConflict => CallError::StateConflict,
        CallStoreError::PartyBusy => CallError::PartyBusy,
        CallStoreError::LimitReached => CallError::LimitReached,
    }
}

/// 发起结果。
#[derive(Debug, Clone)]
pub struct Initiated {
    pub call: Call,
    pub created: bool,
}

/// 发起：Caller（Human 或 Agent）在共享 Room 向 Callee（Human 或 Agent）发起
/// 通话。Caller 是 Human 或 Agent 由路由层认证决定（Human Session →
/// `/v1/calls`；Backend Credential → `/v1/backend/calls`）；两侧都必须在该
/// Room 有 active Membership 且 Participant active；未知/非成员统一
/// RoomNotFound。返回 Call（复用同 (room, caller, callee) 既有 active Call
/// 时 created=false）。
pub async fn initiate(
    call: &dyn CallStore,
    room: &dyn RoomStore,
    caller_id: &ParticipantId,
    room_id: &RoomId,
    callee_id: &ParticipantId,
    media: crate::domain::call::CallMedia,
    now: UtcMillis,
) -> Result<Initiated, CallError> {
    if caller_id == callee_id {
        return Err(CallError::InvalidRequest(
            "caller and callee must be distinct".into(),
        ));
    }
    let members = room
        .get_room_members(caller_id, room_id)
        .await
        .map_err(room_store_error)?
        .ok_or(CallError::RoomNotFound)?;

    // 两侧 active Membership + Participant active（未知/非成员统一
    // RoomNotFound，不泄露存在性）。同时取两方最小公共展示名快照。
    let mut caller_member: Option<crate::domain::participant::DisplayName> = None;
    let mut callee_member: Option<crate::domain::participant::DisplayName> = None;
    for member in &members {
        let active = member.membership.status == MembershipStatus::Active
            && member.participant.status == ParticipantStatus::Active;
        if !active {
            continue;
        }
        if member.participant.id == *caller_id {
            caller_member = Some(member.participant.display_name.clone());
        }
        if member.participant.id == *callee_id {
            callee_member = Some(member.participant.display_name.clone());
        }
    }
    let (Some(caller_display), Some(callee_display)) = (caller_member, callee_member) else {
        return Err(CallError::RoomNotFound);
    };

    let outcome = call
        .initiate(
            caller_id,
            &caller_display,
            callee_id,
            &callee_display,
            room_id,
            media,
            now,
        )
        .map_err(map_store_error)?;
    Ok(Initiated {
        call: outcome.call,
        created: outcome.created,
    })
}

/// callee 接受（只可由 callee）；`actor == callee` 才可，否则 CallNotFound。
pub fn accept(
    call: &dyn CallStore,
    call_id: &CallId,
    actor_id: &ParticipantId,
    now: UtcMillis,
) -> Result<Call, CallError> {
    call.accept(call_id, actor_id, now)
        .map(|o| o.call)
        .map_err(map_store_error)
}

/// callee 拒绝（只可由 callee）。
pub fn decline(
    call: &dyn CallStore,
    call_id: &CallId,
    actor_id: &ParticipantId,
    now: UtcMillis,
) -> Result<Call, CallError> {
    call.decline(call_id, actor_id, now)
        .map(|o| o.call)
        .map_err(map_store_error)
}

/// 任一方挂断/取消。
pub fn hangup(
    call: &dyn CallStore,
    call_id: &CallId,
    actor_id: &ParticipantId,
    now: UtcMillis,
) -> Result<Call, CallError> {
    call.hangup(call_id, actor_id, now)
        .map(|o| o.call)
        .map_err(map_store_error)
}

/// 中继信令至对端当前在线订阅。
pub fn signal(
    call: &dyn CallStore,
    call_id: &CallId,
    from_id: &ParticipantId,
    body: &SignalBody,
    now: UtcMillis,
) -> Result<bool, CallError> {
    call.signal(call_id, from_id, body, now)
        .map(|o| o.delivered)
        .map_err(map_store_error)
}

/// 读取 Call（存在性裁决在调用前完成；非参与方不得调本用例）。
pub fn get(call: &dyn CallStore, call_id: &CallId) -> Option<Call> {
    call.get(call_id)
}
