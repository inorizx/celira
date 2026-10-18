//! 进程内 Call adapter：实现 `ports::call::CallStore`。
//!
//! 权威契约：CORE_V1_CALL.md（Core v1 窄解冻切片）。
//!
//! 这是 Call 切片唯一允许存在的临时状态实现：
//! - 纯进程内（HashMap + Mutex），**绝不写 SQLite**：不新增任何表/列，
//!   不触碰 Message/History/Human Event/Completion/ACK/Membership/
//!   Permission/Binding/Credential 既有持久语义；
//! - Call 状态机（ringing → in_progress → ended）与信令中继（SDP/ICE 纯
//!   控制消息）都只存在于 Core 进程内；媒体由两端客户端 WebRTC P2P 直连，
//!   本实现永不接触媒体字节；
//! - 时间由调用方注入（`now: UtcMillis`），实现完全确定性、可测；
//! - 硬上限（激活 Call 数）达到上限 fail closed（`CallError::LimitReached`）；
//!   ring 超时由单一后台 sweep 以固定短间隔调用 `sweep_expired` 释放名额；
//! - Call-stream 订阅送达：每参与方每个连接一个有界容量的 channel（只持
//!   弱引用、断开惰性剪除），振铃/信令/状态 only 投给对端当前在线订阅；
//!   通道满时 best-effort 丢弃新事件（对端以 REST 重同步、迟发信令由 RTC
//!   重协商/客户端重发兜底）——绝不无界积压；
//! - 本实现不感知 Session/Backend Credential/Binding/token/room 成员；参与
//!   方身份与房间成员关系由 application 层经既有 Room 端口裁决，本模块只
//!   持有 Participant ID 键的临时状态，并按参与方 ID 定向送达最小公共投影
//!   （绝不 Serialize Credential/Binding/token/内部 sequence）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::mpsc;

use crate::domain::call::{
    Call, CallId, CallState, EndedReason, MAX_CALLS, SignalBody, SignalKind,
};
use crate::domain::participant::ParticipantId;
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::ports::call::{
    ActionOutcome, CallError, CallStore, CallStreamEvent, InitiateOutcome, SignalOutcome,
};

/// 每个参与方至多保留的 call-stream 订阅数上限（防异常膨胀；超出部分直接
/// 丢弃送达，不影响 REST 正确性）。
const MAX_SUBSCRIBERS_PER_PARTICIPANT: usize = 64;

/// 每连接 Call-stream 送达通道容量（有界）。振铃/信令/状态量很小、读取端
/// 及时消费，正常不会积压；通道满时丢弃新事件（best-effort，以 REST 重
/// 同步与 RTC 兜底），绝不产生无界积压。
pub const CALL_STREAM_CAPACITY: usize = 32;

/// CallInner：全部进程内临时状态（短临界区、无 await）。
struct CallInner {
    calls: HashMap<String, Call>,
    /// 参与方 → 其当前 active（ring|in_progress）Call 的索引。
    active: HashMap<String, String>,
}

impl CallInner {
    fn is_active(state: CallState) -> bool {
        matches!(state, CallState::Ringing | CallState::InProgress)
    }
}

/// 进程内 Call 存储。
pub struct InMemoryCallState {
    inner: Mutex<CallInner>,
    subscribers: Mutex<HashMap<String, Vec<Weak<mpsc::Sender<CallStreamEvent>>>>>,
    max_calls: usize,
}

impl InMemoryCallState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(CallInner {
                calls: HashMap::new(),
                active: HashMap::new(),
            }),
            subscribers: Mutex::new(HashMap::new()),
            max_calls: MAX_CALLS,
        }
    }

    /// 测试注入较小的激活 Call 上限。
    #[cfg(test)]
    pub fn with_max_calls(n: usize) -> Self {
        Self {
            inner: Mutex::new(CallInner {
                calls: HashMap::new(),
                active: HashMap::new(),
            }),
            subscribers: Mutex::new(HashMap::new()),
            max_calls: n,
        }
    }
}

impl Default for InMemoryCallState {
    fn default() -> Self {
        Self::new()
    }
}

impl CallStore for InMemoryCallState {
    fn initiate(
        &self,
        caller_id: &ParticipantId,
        caller_display: &crate::domain::participant::DisplayName,
        callee_id: &ParticipantId,
        callee_display: &crate::domain::participant::DisplayName,
        room_id: &RoomId,
        media: crate::domain::call::CallMedia,
        now: UtcMillis,
    ) -> Result<InitiateOutcome, CallError> {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());

        // 同 (room, caller, callee) 已有 active Call → 复用（避免重复振铃，
        // 近似无状态幂等）。绝不新建第二条。
        if let Some(existing) = inner.calls.values().find(|c| {
            CallInner::is_active(c.state)
                && c.room_id == *room_id
                && c.caller_id == *caller_id
                && c.callee_id == *callee_id
        }) {
            return Ok(InitiateOutcome {
                call: existing.clone(),
                created: false,
            });
        }

        // 任一参与方已在另一 active Call → busy。复用情形已在上方返回，因此
        // 这里剩下的 active 索引都属于「其他 Call」。
        if inner.active.contains_key(caller_id.as_str())
            || inner.active.contains_key(callee_id.as_str())
        {
            return Err(CallError::PartyBusy);
        }

        // 硬上限：激活 Call 数满则 fail closed（ring 超时/结束后释放名额）。
        let active_calls = inner
            .calls
            .values()
            .filter(|c| CallInner::is_active(c.state))
            .count();
        if active_calls >= self.max_calls {
            return Err(CallError::LimitReached);
        }

        let call_id = CallId::new_v4();
        let call = Call {
            id: call_id.clone(),
            room_id: room_id.clone(),
            caller_id: caller_id.clone(),
            caller_display: caller_display.clone(),
            callee_id: callee_id.clone(),
            callee_display: callee_display.clone(),
            media,
            state: CallState::Ringing,
            ended_reason: None,
            created_at: now,
            accepted_at: None,
            ended_at: None,
        };
        inner
            .calls
            .insert(call_id.as_str().to_owned(), call.clone());
        inner
            .active
            .insert(caller_id.as_str().to_owned(), call_id.as_str().to_owned());
        inner
            .active
            .insert(callee_id.as_str().to_owned(), call_id.as_str().to_owned());
        drop(inner);

        // 只对 callee 发送振铃。
        self.deliver(callee_id, CallStreamEvent::Ring { call: call.clone() }, now);
        Ok(InitiateOutcome {
            call,
            created: true,
        })
    }

    fn accept(
        &self,
        call_id: &CallId,
        actor_id: &ParticipantId,
        now: UtcMillis,
    ) -> Result<ActionOutcome, CallError> {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let Some(call) = inner.calls.get_mut(call_id.as_str()) else {
            return Err(CallError::NotFound);
        };
        // 只有 callee 可接受；非 callee 一律视作不存在（no oracle）。
        if call.callee_id != *actor_id {
            return Err(CallError::NotFound);
        }
        if call.state != CallState::Ringing {
            return Err(CallError::StateConflict);
        }
        call.state = CallState::InProgress;
        call.accepted_at = Some(now);
        let call = call.clone();
        let caller = call.caller_id.clone();
        let callee = call.callee_id.clone();
        drop(inner);
        // state / ended_reason 变更投给 caller 与 callee 双方的流（契约
        // CORE_V1_CALL.md 4.1）。callee 是动作方，HTTP 200 已携带新状态；
        // 仍按契约把投影投给双方，保持两端 UI 收敛一致。
        let event = CallStreamEvent::StateChanged { call: call.clone() };
        self.deliver(&caller, event.clone(), now);
        self.deliver(&callee, event, now);
        Ok(ActionOutcome { call })
    }

    fn decline(
        &self,
        call_id: &CallId,
        actor_id: &ParticipantId,
        now: UtcMillis,
    ) -> Result<ActionOutcome, CallError> {
        self.terminal_action(call_id, actor_id, now, true)
    }

    fn hangup(
        &self,
        call_id: &CallId,
        actor_id: &ParticipantId,
        now: UtcMillis,
    ) -> Result<ActionOutcome, CallError> {
        // hangup 允许 caller 或 callee。
        {
            let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            let Some(call) = inner.calls.get(call_id.as_str()) else {
                return Err(CallError::NotFound);
            };
            if call.caller_id != *actor_id && call.callee_id != *actor_id {
                return Err(CallError::NotFound);
            }
        }
        self.terminal_action(call_id, actor_id, now, false)
    }

    fn signal(
        &self,
        call_id: &CallId,
        from_id: &ParticipantId,
        body: &SignalBody,
        now: UtcMillis,
    ) -> Result<SignalOutcome, CallError> {
        let delivered = {
            let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            let Some(call) = inner.calls.get(call_id.as_str()) else {
                return Err(CallError::NotFound);
            };
            if call.caller_id != *from_id && call.callee_id != *from_id {
                return Err(CallError::NotFound);
            }
            // 终态后不再接受任何信令（`ended` 是终态；后续动作一律
            // call_state_conflict，契约 CORE_V1_CALL.md §7）。
            if call.state == CallState::Ended {
                return Err(CallError::StateConflict);
            }
            // 中继方向强制（契约 §4.2）：offer 只可 caller→callee、answer
            // 只可 callee→caller、ice 双向 trickle。方向/角色错误一律
            // StateConflict，绝不把信令投递给错误的对端或当作普通中继。
            let is_caller = call.caller_id == *from_id;
            let is_callee = call.callee_id == *from_id;
            let allowed = match body.kind {
                SignalKind::Offer => is_caller,
                SignalKind::Answer => is_callee,
                SignalKind::Ice => is_caller || is_callee,
            };
            if !allowed {
                return Err(CallError::StateConflict);
            }
            let peer = if is_caller {
                call.callee_id.clone()
            } else {
                call.caller_id.clone()
            };
            drop(inner);
            self.deliver(
                &peer,
                CallStreamEvent::Signal {
                    call_id: call_id.clone(),
                    from: from_id.clone(),
                    body: body.clone(),
                },
                now,
            )
        };
        Ok(SignalOutcome { delivered })
    }

    fn get(&self, call_id: &CallId) -> Option<Call> {
        let inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.calls.get(call_id.as_str()).cloned()
    }

    fn sweep_expired(&self, now: UtcMillis) -> usize {
        let (timeouted, deliveries) = {
            let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            let ttl = crate::domain::call::RING_TTL.as_millis() as i64;
            let ids: Vec<String> = inner
                .calls
                .values()
                .filter(|c| {
                    c.state == CallState::Ringing && now.millis() >= c.created_at.millis() + ttl
                })
                .map(|c| c.id.as_str().to_owned())
                .collect();
            let mut deliveries = Vec::new();
            for id in &ids {
                let (snapshot, caller, callee) = {
                    let Some(call) = inner.calls.get_mut(id) else {
                        continue;
                    };
                    call.state = CallState::Ended;
                    call.ended_reason = Some(EndedReason::TimeoutRing);
                    call.ended_at = Some(now);
                    (call.clone(), call.caller_id.clone(), call.callee_id.clone())
                };
                inner.active.remove(caller.as_str());
                inner.active.remove(callee.as_str());
                let event = CallStreamEvent::StateChanged {
                    call: snapshot.clone(),
                };
                deliveries.push((caller, event.clone()));
                deliveries.push((callee, event));
            }
            (ids.len(), deliveries)
        };
        for (participant, event) in deliveries {
            self.deliver(&participant, event, now);
        }
        timeouted
    }

    fn subscribe(
        &self,
        participant_id: &ParticipantId,
        sender: Arc<mpsc::Sender<CallStreamEvent>>,
    ) {
        let mut map = self.subscribers.lock().unwrap_or_else(|p| p.into_inner());
        let list = map.entry(participant_id.as_str().to_owned()).or_default();
        list.retain(|weak| weak.strong_count() > 0);
        if list.len() < MAX_SUBSCRIBERS_PER_PARTICIPANT {
            list.push(Arc::downgrade(&sender));
        }
    }
}

impl InMemoryCallState {
    /// 共用终态裁决：`decline`（仅 callee）或挂断（caller/callee）。先校验
    /// Call 存在与角色，再统一落到 ended，释放 active 索引并通知对方。
    fn terminal_action(
        &self,
        call_id: &CallId,
        actor_id: &ParticipantId,
        now: UtcMillis,
        decline: bool,
    ) -> Result<ActionOutcome, CallError> {
        let snapshot = {
            let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
            let Some(call) = inner.calls.get_mut(call_id.as_str()) else {
                return Err(CallError::NotFound);
            };
            if decline {
                if call.callee_id != *actor_id {
                    return Err(CallError::NotFound);
                }
            } else if call.caller_id != *actor_id && call.callee_id != *actor_id {
                return Err(CallError::NotFound);
            }
            if call.state == CallState::Ended {
                return Err(CallError::StateConflict);
            }
            let reason = if decline {
                EndedReason::DeclineCallee
            } else if call.caller_id == *actor_id {
                EndedReason::HangupCaller
            } else {
                EndedReason::HangupCallee
            };
            call.state = CallState::Ended;
            call.ended_reason = Some(reason);
            call.ended_at = Some(now);
            call.clone()
        };
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        inner.active.remove(snapshot.caller_id.as_str());
        inner.active.remove(snapshot.callee_id.as_str());
        drop(inner);
        // state / ended_reason 变更投给 caller 与 callee 双方的流（契约
        // CORE_V1_CALL.md 4.1）。
        let event = CallStreamEvent::StateChanged {
            call: snapshot.clone(),
        };
        self.deliver(&snapshot.caller_id, event.clone(), now);
        self.deliver(&snapshot.callee_id, event, now);
        Ok(ActionOutcome { call: snapshot })
    }

    /// best-effort 定向送达：只投给 target 的当前在线订阅。每连接有界容量；
    /// 通道已满（消费者慢）丢弃新事件（with REST resync + RTC fallback），
    /// 接收端关闭则剪除订阅。绝不无界积压，也不因送达失败翻转业务裁决。
    fn deliver(
        &self,
        participant_id: &ParticipantId,
        event: CallStreamEvent,
        _now: UtcMillis,
    ) -> bool {
        let mut map = self.subscribers.lock().unwrap_or_else(|p| p.into_inner());
        let Some(list) = map.get_mut(participant_id.as_str()) else {
            return false;
        };
        let mut delivered = false;
        list.retain(|weak| {
            let Some(sender) = weak.upgrade() else {
                return false;
            };
            match sender.try_send(event.clone()) {
                Ok(()) => {
                    delivered = true;
                    true
                }
                // 通道已有待发送事件（消费者慢）：丢弃本次，to be resynced
                // 由客户端以 REST / RTC 兜底（best-effort，不报业务失败）。
                Err(mpsc::error::TrySendError::Full(_)) => true,
                // 接收端已关闭：剪除订阅。
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            }
        });
        map.retain(|_, list| !list.is_empty());
        delivered
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::call::{CallMedia, CallState, EndedReason, RING_TTL, SignalKind};
    use crate::domain::participant::DisplayName;
    use crate::domain::room::RoomId;
    use crate::domain::time::UtcMillis;

    fn alice() -> ParticipantId {
        ParticipantId("00000000-0000-4000-8000-0000000000a1".into())
    }
    fn bob() -> ParticipantId {
        ParticipantId("00000000-0000-4000-8000-0000000000a2".into())
    }
    fn carol() -> ParticipantId {
        ParticipantId("00000000-0000-4000-8000-0000000000a3".into())
    }
    fn dave() -> ParticipantId {
        ParticipantId("00000000-0000-4000-8000-0000000000a4".into())
    }
    fn erin() -> ParticipantId {
        ParticipantId("00000000-0000-4000-8000-0000000000a5".into())
    }
    fn dname(s: &str) -> DisplayName {
        DisplayName::parse(s).unwrap()
    }
    fn room() -> RoomId {
        RoomId("00000000-0000-4000-8000-0000000000bb".into())
    }
    fn t0() -> UtcMillis {
        UtcMillis::from_millis(1_000_000)
    }
    fn at(base: UtcMillis, offset_ms: i64) -> UtcMillis {
        UtcMillis::from_millis(base.millis() + offset_ms)
    }
    fn sdp(s: &str) -> SignalBody {
        SignalBody::parse(SignalKind::Offer, vec![s.to_owned()]).unwrap()
    }

    #[tokio::test]
    async fn initiate_rings_callee_and_reuses_same_triple() {
        let store = InMemoryCallState::new();
        // 订阅 callee 的送达通道（连接任务持强引用，store 只存弱引用）。
        let (tx, mut rx) = tokio::sync::mpsc::channel::<CallStreamEvent>(CALL_STREAM_CAPACITY);
        let conn = Arc::new(tx);
        store.subscribe(&bob(), conn.clone());

        let out = store
            .initiate(
                &alice(),
                &dname("A"),
                &bob(),
                &dname("B"),
                &room(),
                CallMedia::Audio,
                t0(),
            )
            .unwrap();
        assert!(out.created);
        assert_eq!(out.call.state, CallState::Ringing);
        assert_eq!(out.call.caller_display, dname("A"));
        assert_eq!(out.call.callee_display, dname("B"));
        assert_eq!(store.get(&out.call.id).unwrap().state, CallState::Ringing);
        // callee 收到振铃。
        let ev = rx.recv().await.unwrap();
        assert!(matches!(ev, CallStreamEvent::Ring { .. }));

        // 同 (room, caller, callee) 复用，created=false。
        let again = store
            .initiate(
                &alice(),
                &dname("A"),
                &bob(),
                &dname("B"),
                &room(),
                CallMedia::Audio,
                t0(),
            )
            .unwrap();
        assert!(!again.created);
        assert_eq!(again.call.id, out.call.id);
    }

    #[tokio::test]
    async fn busy_party_rejected_when_in_another_active_call() {
        let store = InMemoryCallState::new();
        // Carol 已在与 Bob 的通话中。
        store
            .initiate(
                &carol(),
                &dname("C"),
                &bob(),
                &dname("B"),
                &room(),
                CallMedia::Audio,
                t0(),
            )
            .unwrap();
        // Alice 尝试呼 Bob（Bob 忙） → PartyBusy。
        let err = store
            .initiate(
                &alice(),
                &dname("A"),
                &bob(),
                &dname("B"),
                &room(),
                CallMedia::Audio,
                t0(),
            )
            .unwrap_err();
        assert!(matches!(err, CallError::PartyBusy));
        // Carol 忙：Alice 呼 Carol → PartyBusy。
        let err = store
            .initiate(
                &alice(),
                &dname("A"),
                &carol(),
                &dname("C"),
                &room(),
                CallMedia::Audio,
                t0(),
            )
            .unwrap_err();
        assert!(matches!(err, CallError::PartyBusy));
    }

    #[tokio::test]
    async fn accept_only_by_callee_and_state_conflicts_after_end() {
        let store = InMemoryCallState::new();
        let out = store
            .initiate(
                &alice(),
                &dname("A"),
                &bob(),
                &dname("B"),
                &room(),
                CallMedia::Audio,
                t0(),
            )
            .unwrap();
        // 非 callee（caller）accept → NotFound（no oracle）。
        assert!(matches!(
            store.accept(&out.call.id, &alice(), t0()),
            Err(CallError::NotFound)
        ));
        // callee accept → in_progress。
        let acc = store.accept(&out.call.id, &bob(), at(t0(), 1)).unwrap();
        assert_eq!(acc.call.state, CallState::InProgress);
        assert!(acc.call.accepted_at.is_some());
        // hangup by caller → ended(hangup_caller)。
        let h = store.hangup(&out.call.id, &alice(), at(t0(), 2)).unwrap();
        assert_eq!(h.call.state, CallState::Ended);
        assert_eq!(h.call.ended_reason, Some(EndedReason::HangupCaller));
        // 结束后 accept → StateConflict。
        assert!(matches!(
            store.accept(&out.call.id, &bob(), at(t0(), 3)),
            Err(CallError::StateConflict)
        ));
    }

    #[tokio::test]
    async fn decline_and_signal_delivery_with_ttl() {
        let store = InMemoryCallState::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<CallStreamEvent>(CALL_STREAM_CAPACITY);
        // caller 订阅以便收到 answer/state（连接任务持强引用）。
        let conn = Arc::new(tx);
        store.subscribe(&alice(), conn.clone());
        let out = store
            .initiate(
                &alice(),
                &dname("A"),
                &bob(),
                &dname("B"),
                &room(),
                CallMedia::Audio,
                t0(),
            )
            .unwrap();

        // caller 发 offer → callee 收到信号（未订阅 → delivered=false 但明确）。
        let sent = store
            .signal(&out.call.id, &alice(), &sdp("v=0"), t0())
            .unwrap();
        assert!(!sent.delivered, "callee has no own subscriber yet");

        // callee 订阅并 decline → caller 收到 StateChanged(decline_callee)。
        let (tx2, mut _rx2) = tokio::sync::mpsc::channel::<CallStreamEvent>(CALL_STREAM_CAPACITY);
        let bob_sub = Arc::new(tx2);
        store.subscribe(&bob(), bob_sub.clone());
        store.decline(&out.call.id, &bob(), at(t0(), 1)).unwrap();
        let ev = rx.recv().await.unwrap();
        match ev {
            CallStreamEvent::StateChanged { call } => {
                assert_eq!(call.state, CallState::Ended);
                assert_eq!(call.ended_reason, Some(EndedReason::DeclineCallee));
            }
            other => panic!("expected state changed, got {other:?}"),
        }

        // 信令同步即时中继：活跃 Call 中，caller 再发 offer 应投递成功
        // （对端当前在线订阅存在，delivered=true）。
        let out2 = store
            .initiate(
                &alice(),
                &dname("A"),
                &carol(),
                &dname("C"),
                &room(),
                CallMedia::Audio,
                at(t0(), 2),
            )
            .unwrap();
        let (tx3, mut _rx3) = tokio::sync::mpsc::channel::<CallStreamEvent>(CALL_STREAM_CAPACITY);
        let carol_sub = Arc::new(tx3);
        store.subscribe(&carol(), carol_sub.clone());
        let sent = store
            .signal(&out2.call.id, &alice(), &sdp("v=0"), at(t0(), 2))
            .unwrap();
        assert!(
            sent.delivered,
            "peer with a live subscription receives the signal"
        );
        // 终态（ended）后任何信令一律 call_state_conflict（契约 §7 终态纪律）。
        assert!(matches!(
            store.signal(&out.call.id, &alice(), &sdp("v=0"), at(t0(), 2)),
            Err(CallError::StateConflict)
        ));
    }

    #[tokio::test]
    async fn signal_direction_enforced_offer_answer_ice() {
        let store = InMemoryCallState::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<CallStreamEvent>(CALL_STREAM_CAPACITY);
        let conn = Arc::new(tx);
        store.subscribe(&bob(), conn.clone());
        let out = store
            .initiate(
                &alice(),
                &dname("A"),
                &bob(),
                &dname("B"),
                &room(),
                CallMedia::Audio,
                t0(),
            )
            .unwrap();

        // offer：只可 caller(Alice)→callee(Bob)。
        // 先消费 initiate 时投给 callee 的 Ring 事件。
        let ring = rx.recv().await.unwrap();
        assert!(matches!(ring, CallStreamEvent::Ring { .. }));
        let ok = store
            .signal(&out.call.id, &alice(), &sdp("offer"), t0())
            .unwrap();
        assert!(ok.delivered, "caller offer delivered to callee");
        let ev = rx.try_recv().unwrap();
        match ev {
            CallStreamEvent::Signal { from, body, .. } => {
                assert_eq!(from, alice());
                assert_eq!(body.kind, SignalKind::Offer);
            }
            other => panic!("expected signal, got {other:?}"),
        }
        // answer 方向错误（caller 发 answer）→ call_state_conflict。
        let answer_req = SignalBody::parse(SignalKind::Answer, vec!["v=0".into()]).unwrap();
        assert!(matches!(
            store.signal(&out.call.id, &alice(), &answer_req, t0()),
            Err(CallError::StateConflict)
        ));
        // offer 方向错误（callee 发 offer）→ call_state_conflict。
        let offer_req = SignalBody::parse(SignalKind::Offer, vec!["v=0".into()]).unwrap();
        assert!(matches!(
            store.signal(&out.call.id, &bob(), &offer_req, t0()),
            Err(CallError::StateConflict)
        ));
        // ICE 双向均可。
        let ice_req = SignalBody::parse(SignalKind::Ice, vec!["c".into()]).unwrap();
        assert!(store.signal(&out.call.id, &alice(), &ice_req, t0()).is_ok());
        assert!(store.signal(&out.call.id, &bob(), &ice_req, t0()).is_ok());
    }

    #[tokio::test]
    async fn sweep_expired_times_out_rings_and_delivers_state() {
        let store = InMemoryCallState::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<CallStreamEvent>(CALL_STREAM_CAPACITY);
        let conn = Arc::new(tx);
        store.subscribe(&alice(), conn.clone());
        let out = store
            .initiate(
                &alice(),
                &dname("A"),
                &bob(),
                &dname("B"),
                &room(),
                CallMedia::Audio,
                t0(),
            )
            .unwrap();
        // 未到 TTL：不超时。
        assert_eq!(
            store.sweep_expired(at(t0(), RING_TTL.as_millis() as i64 - 1)),
            0
        );
        // 到 TTL：超时并通知双方（caller 订阅收到 StateChanged timeout_ring）。
        let timed = store.sweep_expired(at(t0(), RING_TTL.as_millis() as i64));
        assert_eq!(timed, 1);
        let got = store.get(&out.call.id).unwrap();
        assert_eq!(got.state, CallState::Ended);
        assert_eq!(got.ended_reason, Some(EndedReason::TimeoutRing));
        let ev = rx.recv().await.unwrap();
        assert!(matches!(ev, CallStreamEvent::StateChanged { .. }));
    }

    #[tokio::test]
    async fn hard_limit_fails_closed_and_frees_after_end() {
        let store = InMemoryCallState::with_max_calls(1);
        let out = store
            .initiate(
                &alice(),
                &dname("A"),
                &bob(),
                &dname("B"),
                &room(),
                CallMedia::Audio,
                t0(),
            )
            .unwrap();
        // 已满：用两个全新（非 busy）参与者 → 只有上限可拒绝 → LimitReached。
        let err = store
            .initiate(
                &dave(),
                &dname("D"),
                &erin(),
                &dname("E"),
                &room(),
                CallMedia::Audio,
                t0(),
            )
            .unwrap_err();
        assert!(matches!(err, CallError::LimitReached));
        // 结束后释放名额。
        store.hangup(&out.call.id, &alice(), at(t0(), 1)).unwrap();
        let out2 = store
            .initiate(
                &dave(),
                &dname("D"),
                &erin(),
                &dname("E"),
                &room(),
                CallMedia::Audio,
                at(t0(), 2),
            )
            .unwrap();
        assert!(out2.created);
    }
}
