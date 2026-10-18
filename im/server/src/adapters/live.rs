//! 进程内主体进行态 adapter：实现 `ports::live::LivePresence`。
//!
//! 两张进程内表，别无其他：
//! - `last_seen`：主体 → 最近一次被看见的时刻（判"在不在"的兜底来源，
//!   只对**拉取式** Backend 有意义——挂门铃的看连接就够）；
//! - `typing`：`(房, 主体)` → 最近一次声明"我正在写"的时刻。
//!
//! 纪律（契约 §6）：
//! - **绝不写 SQLite**：Core 重启即失忆，这不是缺陷；
//! - **读是纯观察**：按时间过滤过期项并返回，绝不因为读而删除条目；
//! - **过期靠时间判断，不靠后台任务**：本切片没有任何后台 sweep——
//!   进行态没有人需要被"唤醒"（读面是拉取），所以不需要主动清理；
//! - **有界**：两张表各有硬上限，达上限按最旧优先淘汰。淘汰不报错、不
//!   产生错误码——丢一条易失信号不影响任何公共事实（这与"公共事实达到
//!   上限必须 fail closed"是两回事）；
//! - **无执行面**：没有阶段、没有生成流 id、没有 revision、没有草稿正文。

use std::collections::HashMap;
use std::sync::Mutex;

use crate::domain::live::{MAX_SEEN_ENTRIES, MAX_TYPING_ENTRIES, SEEN_TTL, TYPING_TTL};
use crate::domain::participant::ParticipantId;
use crate::domain::room::RoomId;
use crate::domain::time::UtcMillis;
use crate::ports::human_event::HumanEventNotifier;
use crate::ports::live::{ConnectionPresence, LivePresence};

/// 进行态接线束：composition 与测试夹具**共用的一次性构造**。
///
/// 它把"这一片到底由哪几个进程内对象组成"收在一处，避免调用方各自拼装
/// 出不一致的组合（尤其 Human 连接事实必须与唤醒 notifier 是**同一个
/// 对象**，否则"谁挂着 WS"会问错表）：
/// - `notifier`：Human Event 唤醒 + **连接事实**（同一对象两个面孔）；
/// - `presence`：主体进行态（`last_seen` / `typing`）；
/// - `streams`：Backend 门铃并发门闩，同时是 **Agent 连接事实**的来源。
#[derive(Clone)]
pub struct LiveWiring {
    notifier: std::sync::Arc<dyn HumanEventNotifier>,
    human_connections: std::sync::Arc<dyn ConnectionPresence>,
    presence: std::sync::Arc<dyn LivePresence>,
    streams: std::sync::Arc<crate::adapters::http::backend_stream_routes::BackendStreamGate>,
}

impl LiveWiring {
    /// 生产与测试默认组合：全是进程内易失对象，进程重启即失忆。
    /// Human 的唤醒端口与连接事实来自**同一个** notifier 对象。
    pub fn new_volatile() -> Self {
        let notifier =
            std::sync::Arc::new(crate::adapters::notifier::VolatileHumanEventNotifier::new());
        Self::from_parts(notifier.clone(), notifier)
    }

    /// 用**调用方给定的**两半组装（测试要注入桩时用）：唤醒端口可以是任意
    /// `HumanEventNotifier`，连接事实可以是任意 `ConnectionPresence`。
    pub fn from_parts(
        notifier: std::sync::Arc<dyn HumanEventNotifier>,
        human_connections: std::sync::Arc<dyn ConnectionPresence>,
    ) -> Self {
        Self {
            notifier,
            human_connections,
            presence: std::sync::Arc::new(VolatileLive::new()),
            streams: std::sync::Arc::new(
                crate::adapters::http::backend_stream_routes::BackendStreamGate::new(),
            ),
        }
    }

    pub fn human_notifier(&self) -> std::sync::Arc<dyn HumanEventNotifier> {
        self.notifier.clone()
    }

    pub fn presence(&self) -> std::sync::Arc<dyn LivePresence> {
        self.presence.clone()
    }

    pub fn human_connections(&self) -> std::sync::Arc<dyn ConnectionPresence> {
        self.human_connections.clone()
    }

    pub fn backend_connections(&self) -> std::sync::Arc<dyn ConnectionPresence> {
        self.streams.clone()
    }

    pub fn streams(
        &self,
    ) -> std::sync::Arc<crate::adapters::http::backend_stream_routes::BackendStreamGate> {
        self.streams.clone()
    }
}

/// 进程内主体进行态（唯一实例由 composition 持有）。
#[derive(Debug, Default)]
pub struct VolatileLive {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    seen: HashMap<ParticipantId, UtcMillis>,
    typing: HashMap<(RoomId, ParticipantId), UtcMillis>,
}

impl VolatileLive {
    pub fn new() -> Self {
        Self::default()
    }

    /// 测试与诊断用：当前表深（不暴露内容）。
    #[cfg(test)]
    fn depths(&self) -> (usize, usize) {
        let inner = self.inner.lock().unwrap();
        (inner.seen.len(), inner.typing.len())
    }
}

impl LivePresence for VolatileLive {
    fn mark_seen(&self, participant: &ParticipantId, now: UtcMillis) {
        let mut inner = self.inner.lock().unwrap();
        if inner.seen.len() >= MAX_SEEN_ENTRIES && !inner.seen.contains_key(participant) {
            evict_oldest(&mut inner.seen);
        }
        inner.seen.insert(participant.clone(), now);
    }

    fn mark_typing(&self, room_id: &RoomId, participant: &ParticipantId, now: UtcMillis) {
        let key = (room_id.clone(), participant.clone());
        let mut inner = self.inner.lock().unwrap();
        if inner.typing.len() >= MAX_TYPING_ENTRIES && !inner.typing.contains_key(&key) {
            evict_oldest(&mut inner.typing);
        }
        inner.typing.insert(key, now);
    }

    fn clear_typing(&self, room_id: &RoomId, participant: &ParticipantId) {
        let key = (room_id.clone(), participant.clone());
        self.inner.lock().unwrap().typing.remove(&key);
    }

    fn typing_in_room(&self, room_id: &RoomId, now: UtcMillis) -> Vec<ParticipantId> {
        let window = TYPING_TTL.as_millis() as i64;
        let inner = self.inner.lock().unwrap();
        let mut out: Vec<ParticipantId> = inner
            .typing
            .iter()
            .filter(|((room, _), at)| room == room_id && !expired(at, now, window))
            .map(|((_, participant), _)| participant.clone())
            .collect();
        // 稳定顺序：调用方（投影）按成员顺序再排，这里只保证同一输入
        // 得到同一输出，避免测试与日志抖动。
        out.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        out
    }

    fn seen_recently(&self, participant: &ParticipantId, now: UtcMillis) -> bool {
        let window = SEEN_TTL.as_millis() as i64;
        let inner = self.inner.lock().unwrap();
        inner
            .seen
            .get(participant)
            .is_some_and(|at| !expired(at, now, window))
    }
}

/// 过期判据：`now >= at + window`（与既有 Live State 的边界一致）。
fn expired(at: &UtcMillis, now: UtcMillis, window_millis: i64) -> bool {
    now.millis() >= at.millis().saturating_add(window_millis)
}

/// 淘汰时间最旧的一项（仅在表满且要写新键时调用）。空表不调用。
fn evict_oldest<K: Clone + std::hash::Hash + Eq>(map: &mut HashMap<K, UtcMillis>) {
    if let Some(key) = map
        .iter()
        .min_by_key(|(_, at)| at.millis())
        .map(|(k, _)| k.clone())
    {
        map.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn participant(raw: &str) -> ParticipantId {
        ParticipantId(raw.to_owned())
    }

    fn room(raw: &str) -> RoomId {
        RoomId::parse_canonical(raw).expect("canonical uuid")
    }

    const ROOM_A: &str = "11111111-1111-4111-8111-111111111111";
    const ROOM_B: &str = "22222222-2222-4222-8222-222222222222";

    #[test]
    fn typing_expires_by_its_own_ttl_and_is_per_room() {
        let live = VolatileLive::new();
        let me = participant("agent-1");
        let t0 = UtcMillis::from_millis(1_000_000);
        live.mark_typing(&room(ROOM_A), &me, t0);

        assert_eq!(
            live.typing_in_room(&room(ROOM_A), t0),
            vec![me.clone()],
            "刚声明就在写"
        );
        assert!(
            live.typing_in_room(&room(ROOM_B), t0).is_empty(),
            "进行态按房隔离：A 房的写作不泄漏到 B 房"
        );

        let just_before = UtcMillis::from_millis(1_000_000 + TYPING_TTL.as_millis() as i64 - 1);
        assert_eq!(
            live.typing_in_room(&room(ROOM_A), just_before),
            vec![me.clone()],
            "TTL 边界前仍算在写"
        );

        let at_expiry = UtcMillis::from_millis(1_000_000 + TYPING_TTL.as_millis() as i64);
        assert!(
            live.typing_in_room(&room(ROOM_A), at_expiry).is_empty(),
            "到 TTL 边界即视为没在写"
        );
    }

    #[test]
    fn sending_a_message_supersedes_typing_and_clear_is_idempotent() {
        let live = VolatileLive::new();
        let me = participant("human-1");
        let now = UtcMillis::from_millis(2_000_000);
        live.mark_typing(&room(ROOM_A), &me, now);
        live.clear_typing(&room(ROOM_A), &me);
        live.clear_typing(&room(ROOM_A), &me);
        assert!(
            live.typing_in_room(&room(ROOM_A), now).is_empty(),
            "消息发出后自己的进行态立刻消失，重复清除幂等"
        );
    }

    #[test]
    fn read_never_deletes_entries() {
        let live = VolatileLive::new();
        let me = participant("agent-1");
        let t0 = UtcMillis::from_millis(3_000_000);
        live.mark_typing(&room(ROOM_A), &me, t0);
        live.mark_seen(&me, t0);

        // 读一次远未来的时间：全部过期，但条目仍在表里（读是纯观察）。
        let far = UtcMillis::from_millis(3_999_999_999);
        assert!(live.typing_in_room(&room(ROOM_A), far).is_empty());
        assert!(!live.seen_recently(&me, far));
        assert_eq!(
            live.depths(),
            (1, 1),
            "读不删除条目：过期只影响判断，不影响存量"
        );

        // 时间"回到"有效区间内仍能读到（因为条目没被读掉）。
        assert_eq!(live.typing_in_room(&room(ROOM_A), t0), vec![me.clone()]);
        assert!(live.seen_recently(&me, t0));
    }

    #[test]
    fn seen_window_is_wider_than_typing_window() {
        let live = VolatileLive::new();
        let me = participant("agent-1");
        let t0 = UtcMillis::from_millis(4_000_000);
        live.mark_seen(&me, t0);
        let after_typing_ttl =
            UtcMillis::from_millis(4_000_000 + TYPING_TTL.as_millis() as i64 + 1);
        assert!(
            live.seen_recently(&me, after_typing_ttl),
            "「在不在」比「在不在写」宽：轮询式 Backend 两次拉取之间仍算在线"
        );
        let after_seen_ttl = UtcMillis::from_millis(4_000_000 + SEEN_TTL.as_millis() as i64);
        assert!(
            !live.seen_recently(&me, after_seen_ttl),
            "超过被看见窗口即离线"
        );
    }

    #[test]
    fn tables_are_bounded_and_evict_the_oldest() {
        let live = VolatileLive::new();
        for i in 0..(MAX_TYPING_ENTRIES + 8) {
            let me = participant(&format!("p-{i}"));
            live.mark_typing(&room(ROOM_A), &me, UtcMillis::from_millis(1_000 + i as i64));
        }
        let (_, typing_depth) = live.depths();
        assert_eq!(typing_depth, MAX_TYPING_ENTRIES, "typing 表有硬上限");

        // 最旧的（p-0）被淘汰，最新的仍在——且淘汰不报错、不产生错误码。
        let now = UtcMillis::from_millis(1_000 + MAX_TYPING_ENTRIES as i64 + 7);
        let live_now = live.typing_in_room(&room(ROOM_A), now);
        assert!(!live_now.contains(&participant("p-0")), "淘汰最旧");
        assert!(
            live_now.contains(&participant(&format!("p-{}", MAX_TYPING_ENTRIES + 7))),
            "最新写入的仍在"
        );
    }
}
