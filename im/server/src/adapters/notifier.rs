//! Volatile Human Event notifier：进程内 best-effort 定向唤醒 hub。
//!
//! 契约（CORE_V1_HUMAN_EVENTS.md 8.x）：notifier 只负责及时唤醒，不负责
//! 恢复正确性——耐久 REST Event 才是恢复来源。它：
//! - 是纯易失状态：订阅随连接 drop 而消失，唤醒丢失、无 receiver、慢
//!   消费、Core 重启均不影响耐久 Event；
//! - 不写 SQLite：不把 WebSocket 连接状态、服务端投递 ACK 或 cursor 写进
//!   数据库；
//! - 只接收 post-commit 由 HTTP 成功路径从已提交 delivery/fact 事实查得
//!   的 Human 列表，不在本模块复制 targeting 规则；
//! - 每个连接的唤醒通道是**有界容量 1**（tokio mpsc::channel(1)，禁止
//!   unbounded）：连续 notify 在通道已有待发送提示时合并/丢弃重复提示
//!   （try_send 返回 Full 是合法 best-effort，不报业务失败），同一连接
//!   至多一个待发送提示，绝不产生无界积压；通道已关闭（连接结束）才
//!   剪除订阅。
//!
//! 订阅键是 human_id：同一 Human 的多条连接（多设备）各自注册，全部收到
//! 同一唤醒；Session 撤销后的连接由 WS handler 在发送唤醒前重新认证并
//! 关闭，本模块不感知 Session。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::mpsc;

use crate::domain::participant::ParticipantId;
use crate::ports::event::BackendEventNotifier;
use crate::ports::human_event::HumanEventNotifier;
use crate::ports::live::ConnectionPresence;

/// 每个 Human 至多保留的订阅数上限（防异常膨胀；超出部分直接丢弃唤醒，
/// 不影响耐久事实）。
const MAX_SUBSCRIBERS_PER_HUMAN: usize = 64;

/// 每连接唤醒通道容量：1。容量大于 1 不会提升正确性，只掩盖「重复提示
/// 应被合并」这一契约事实；WS handler 与本模块测试都用同一容量创建通道。
pub const WAKE_CHANNEL_CAPACITY: usize = 1;

/// 订阅表：human_id → 该 Human 各连接唤醒通道的弱引用列表（有界、纯易失）。
type SubscriberMap = Arc<Mutex<HashMap<String, Vec<Weak<mpsc::Sender<()>>>>>>;

/// 进程内 volatile notifier。
#[derive(Clone, Default)]
pub struct VolatileHumanEventNotifier {
    subscribers: SubscriberMap,
}

impl VolatileHumanEventNotifier {
    pub fn new() -> Self {
        Self::default()
    }
}

impl HumanEventNotifier for VolatileHumanEventNotifier {
    fn subscribe(&self, human_id: &ParticipantId, wake: Arc<mpsc::Sender<()>>) {
        // Mutex poison 恢复：notifier 是纯易失提示，即使另一个调用方在持锁
        // 期间 panic，也绝不在这里再次 panic 或丢失订阅语义——锁内数据
        // 仍是完整的 HashMap，直接接管即可（安全降级：唤醒可能短暂延迟，
        // 耐久 Event 才是恢复来源）。
        let mut map = self
            .subscribers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let list = map.entry(human_id.as_str().to_owned()).or_default();
        list.retain(|weak| weak.strong_count() > 0);
        if list.len() < MAX_SUBSCRIBERS_PER_HUMAN {
            list.push(Arc::downgrade(&wake));
        }
    }

    fn notify(&self, human_ids: &[ParticipantId]) {
        if human_ids.is_empty() {
            return;
        }
        let mut map = self
            .subscribers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for human_id in human_ids {
            let Some(list) = map.get_mut(human_id.as_str()) else {
                continue;
            };
            list.retain(|weak| {
                let Some(sender) = weak.upgrade() else {
                    return false;
                };
                match sender.try_send(()) {
                    // 队列已有待发送提示：重复提示合并/丢弃，仍是同一
                    // 次「可能有新事件」的提示，不报业务失败。
                    Ok(()) | Err(mpsc::error::TrySendError::Full(())) => true,
                    // 接收端已关闭（连接结束）：剪除订阅。
                    Err(mpsc::error::TrySendError::Closed(())) => false,
                }
            });
        }
        // 惰性清理：没有订阅的 Human 键整体移除。
        map.retain(|_, list| !list.is_empty());
    }
}

/// 连接事实（`ports::live::ConnectionPresence`）由同一个 volatile hub 回答：
/// 「这位 Human 此刻还有活着的 WS 吗」——**在线就是连接在**，不需要任何
/// 一方上报心跳。判定用弱引用的存活（订阅方 drop 强引用即视为断开），
/// 顺带剪掉已死订阅，语义与 `notify` 的惰性清理一致。
///
/// ⚠ 2026-09-28：原先这里还实现 `LiveNotifier`（独立 Live WS 的
/// `live.available` 唤醒）。那条独立 WebSocket 已随旧 Live State 删除——
/// 进行态改由读面（两条 live 快照端点）拉取，不再需要第二条推送线。
impl ConnectionPresence for VolatileHumanEventNotifier {
    fn is_connected(&self, participant: &ParticipantId) -> bool {
        let mut map = self
            .subscribers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(list) = map.get_mut(participant.as_str()) else {
            return false;
        };
        list.retain(|weak| weak.strong_count() > 0);
        let connected = !list.is_empty();
        if !connected {
            map.remove(participant.as_str());
        }
        connected
    }
}

/// Backend Event 唤醒流端口（`ports::event::BackendEventNotifier`）由同一
/// volatile hub 类型实现：语义与 Human Event notifier 完全同构（按 target
/// Agent 订阅、每连接有界容量 1、弱引用、重复提示合并），但实例独立——
/// Agent Event 变化只唤醒 Backend SSE 门铃（`events.available`），绝不
/// 进入 Human Event 表或 Human WS 语义。composition 为该端口创建独立
/// 实例（与 Human/Live notifier 三者互不串扰）。
impl BackendEventNotifier for VolatileHumanEventNotifier {
    fn subscribe(&self, agent_id: &ParticipantId, wake: Arc<mpsc::Sender<()>>) {
        HumanEventNotifier::subscribe(self, agent_id, wake);
    }

    fn notify(&self, agent_ids: &[ParticipantId]) {
        HumanEventNotifier::notify(self, agent_ids);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn notify_reaches_subscribed_connections_only() {
        let hub = VolatileHumanEventNotifier::new();
        let human = ParticipantId("human-1".into());
        let other = ParticipantId("human-2".into());

        // 连接任务持强引用（与 WS handler 一致）；notifier 只存弱引用。
        let (tx1, mut rx1) = mpsc::channel::<()>(1);
        let (tx2, mut rx2) = mpsc::channel::<()>(1);
        let conn1 = Arc::new(tx1);
        let conn2 = Arc::new(tx2);
        HumanEventNotifier::subscribe(&hub, &human, conn1.clone());
        HumanEventNotifier::subscribe(&hub, &human, conn2.clone());

        // 其他 Human 的唤醒不影响 human-1。
        HumanEventNotifier::notify(&hub, std::slice::from_ref(&other));
        assert!(rx1.try_recv().is_err());
        assert!(rx2.try_recv().is_err());

        HumanEventNotifier::notify(&hub, &[human]);
        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
        assert!(rx1.try_recv().is_err(), "exactly one wake per notify");
    }

    #[tokio::test]
    async fn dropped_connections_are_pruned_and_send_failure_is_ignored() {
        let hub = VolatileHumanEventNotifier::new();
        let human = ParticipantId("human-1".into());

        // 模拟已断开的连接：强引用全部 drop 后，notify 惰性剪除。
        let (tx1, _rx1) = mpsc::channel::<()>(1);
        let conn1 = Arc::new(tx1);
        HumanEventNotifier::subscribe(&hub, &human, conn1.clone());
        drop(conn1);

        let (tx2, mut rx2) = mpsc::channel::<()>(1);
        let conn2 = Arc::new(tx2);
        HumanEventNotifier::subscribe(&hub, &human, conn2.clone());

        // 已关闭连接的发送失败被忽略，不影响其他连接。
        HumanEventNotifier::notify(&hub, std::slice::from_ref(&human));
        assert!(rx2.try_recv().is_ok());

        drop(conn2);
        HumanEventNotifier::notify(&hub, &[human]);
        // 无异常即可；订阅键惰性清理。
        let map = hub.subscribers.lock().unwrap();
        assert!(map.get("human-1").is_none_or(|l| l.is_empty()));
    }

    #[tokio::test]
    async fn repeated_notify_merges_to_at_most_one_pending_wake() {
        let hub = VolatileHumanEventNotifier::new();
        let human = ParticipantId("human-1".into());

        let (tx, mut rx) = mpsc::channel::<()>(1);
        let conn = Arc::new(tx);
        HumanEventNotifier::subscribe(&hub, &human, conn.clone());

        // 连续 notify：通道已有待发送提示时，重复提示被合并/丢弃（Full
        // 是合法 best-effort），不产生无界积压，也不报业务失败。
        for _ in 0..100 {
            HumanEventNotifier::notify(&hub, std::slice::from_ref(&human));
        }
        assert!(rx.try_recv().is_ok(), "first pending wake is delivered");
        assert!(
            rx.try_recv().is_err(),
            "repeated notifies must not accumulate: at most one pending wake"
        );

        // 消费后再 notify：新的提示仍能送达（通道未关闭、非丢弃状态）。
        HumanEventNotifier::notify(&hub, &[human]);
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err());
    }

    /// Mutex poison 恢复：持锁期间 panic 后，notify/subscribe 不再次 panic，
    /// 锁内数据完整接管，既有订阅仍能收到唤醒（安全降级，不写安全分类
    /// 之外任何内容）。
    #[test]
    fn poisoned_mutex_recovers_without_panic_and_keeps_waking() {
        let hub = VolatileHumanEventNotifier::new();
        let human = ParticipantId("human-1".into());

        let (tx, mut rx) = mpsc::channel::<()>(1);
        let conn = Arc::new(tx);
        HumanEventNotifier::subscribe(&hub, &human, conn.clone());

        // 模拟持锁 panic：另一个持有者在 drop 时留下 poisoned mutex。
        // 测试不得临时替换进程级 panic hook，避免与并行测试产生全局竞态。
        let injected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = hub.subscribers.lock().unwrap();
            panic!("injected panic while holding the notifier mutex");
        }));
        injected.expect_err("injected panic must have happened");

        // 恢复后仍能正常 notify 与 subscribe，不 panic。
        HumanEventNotifier::notify(&hub, std::slice::from_ref(&human));
        assert!(
            rx.try_recv().is_ok(),
            "wake must still be delivered after mutex poison recovery"
        );

        let (tx2, mut rx2) = mpsc::channel::<()>(1);
        let conn2 = Arc::new(tx2);
        HumanEventNotifier::subscribe(&hub, &human, conn2.clone());
        HumanEventNotifier::notify(&hub, &[human]);
        assert!(rx2.try_recv().is_ok());
    }
}
