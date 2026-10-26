//! 唤醒通道那一层（`wake.rs` 的 [`WakeHub`]）：**一批动静换一次唤醒，不换 N 次**。
//!
//! **为什么单独有这个文件**：这一条链从前**没有任何测试消费者**——`tests/host_doorbell.rs`
//! 用 `ScriptedWake` 把 `WakeHub` 整个换掉（那是对的：它要钉"哪一轮被谁叫醒"，必须把合并
//! 变成确定的事），`WakeHub::bell_only` 全仓无消费者。于是"门铃那层声明的容量 1 到底有没有
//! 传到宿主"这件事**只有生产在跑**，而它当时是**没有传到**的（见下）。
//!
//! 量的是三件事：
//!
//! 1. **一批铃 = 一次唤醒**（不是 N 次）——从前 N 个铃会原样变成 N 次 Run；
//! 2. **终局一个都不许丢**——铃满了可以丢（它只是"还有动静"），终局不行；
//! 3. **取走之后世界又动了，还要再叫一次**——合并是"同一批说一次"，不是"一辈子说一次"。
//!
//! ⚠️ **本文件不量**：`WakeHub::new` 那条带**钟源**的装配（它要一个真环境根 ＋ 一个可调的钟），
//! 以及 `doorbell.rs` 自己那条容量 1 的通道——前者如实登记为未覆盖，后者由 `tests/host_doorbell.rs`
//! 从外面量。

use std::path::Path;
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use celira_agent::clock::ClockHandle;
use celira_agent::doorbell::DoorbellFailure;
use celira_agent::wake::{TimerSource, WakeHub, WakePort, Woken};

/// 一个**受控门铃**：测试往里推信号，它按序交给 [`WakePort::next`]。
///
/// 它是"一个**不丢**的来源"——真门铃那条容量 1 的通道在 `doorbell.rs` 里，本文件量的是
/// **本件有没有把合并接住**，所以来源这一侧故意做成"来多少给多少"：本件不合并，N 个输入
/// 就会原样变成 N 个唤醒。
struct ScriptedBell {
    incoming: Mutex<Receiver<Woken>>,
}

impl ScriptedBell {
    fn new(incoming: Receiver<Woken>) -> Self {
        Self {
            incoming: Mutex::new(incoming),
        }
    }
}

impl WakePort for ScriptedBell {
    fn next(&self) -> Woken {
        self.incoming
            .lock()
            .expect("短临界区")
            .recv()
            .unwrap_or(Woken::Refused(DoorbellFailure::ChannelLost))
    }
}

/// 把一批信号推给脚本门铃，然后**给转发线程一点时间**把该合并的合并掉。
fn push(incoming: &mpsc::Sender<Woken>, woken: Woken) {
    incoming.send(woken).expect("脚本门铃还在");
}

/// 宿主那一侧：把 `hub` 挪进一条线程连着取两次，把两次结果报回来。
///
/// `WakeHub` 持有 `Receiver`（`!Sync`），因此不能借出去跨线程——**挪进去**。
/// 第二次取是**阻塞**的：合并生效的话它永远等不到，于是 `recv_timeout` 会超时。
fn take_two(hub: WakeHub) -> Receiver<Woken> {
    let (report, outcomes) = mpsc::channel::<Woken>();
    thread::spawn(move || {
        report.send(hub.next()).ok();
        report.send(hub.next()).ok();
    });
    outcomes
}

/// **一批铃 = 一次唤醒。**
///
/// 这是"门铃那层声明的容量 1"要兑现的那件事，也是 2026-10-06 修掉的那个缺口：从前转发线程
/// 投进一条**无界**通道、比宿主快，于是门铃那条容量 1 的通道被立刻抽干 ⇒ N 个铃 = N 次 Run。
#[test]
fn a_burst_of_bells_becomes_one_wake_not_one_per_bell() {
    let (incoming, source) = mpsc::channel::<Woken>();
    let hub = WakeHub::bell_only(ScriptedBell::new(source));

    // 20 个铃，宿主**一个都不取**。
    for _ in 0..20 {
        push(&incoming, Woken::Changed);
    }
    thread::sleep(Duration::from_millis(150));

    let outcomes = take_two(hub);

    assert_eq!(
        outcomes
            .recv_timeout(Duration::from_secs(3))
            .expect("第一批铃总得叫醒一次"),
        Woken::Changed
    );
    assert!(
        outcomes.recv_timeout(Duration::from_millis(300)).is_err(),
        "20 个铃只该换**一次**唤醒；这里再来一个，就是「N 个铃 = N 次 Run」那个病回来了"
    );
}

/// **终局一个都不许丢**：铃满了可以丢（它只是"还有动静"），终局不行——丢了宿主就永远不知道
/// 通道断了，会一直阻塞在等待里。
#[test]
fn a_refusal_is_never_dropped_behind_a_waiting_bell() {
    let (incoming, source) = mpsc::channel::<Woken>();
    let hub = WakeHub::bell_only(ScriptedBell::new(source));

    // 先塞一个铃占住那一格，再塞终局——终局会**阻塞**在发送上，而不是被丢掉。
    push(&incoming, Woken::Changed);
    thread::sleep(Duration::from_millis(80));
    push(&incoming, Woken::Refused(DoorbellFailure::ChannelLost));
    thread::sleep(Duration::from_millis(80));

    let outcomes = take_two(hub);

    assert_eq!(
        outcomes
            .recv_timeout(Duration::from_secs(3))
            .expect("铃要先到"),
        Woken::Changed
    );
    assert_eq!(
        outcomes
            .recv_timeout(Duration::from_secs(3))
            .expect("终局一个都不许丢"),
        Woken::Refused(DoorbellFailure::ChannelLost)
    );
}

/// **取走之后世界又动了，还要再叫一次。**
///
/// 合并是"**同一批**说一次"，不是"一辈子说一次"——否则她就再也叫不醒了。
#[test]
fn activity_arriving_after_the_wake_is_taken_wakes_again() {
    let (incoming, source) = mpsc::channel::<Woken>();
    let hub = WakeHub::bell_only(ScriptedBell::new(source));

    push(&incoming, Woken::Changed);
    thread::sleep(Duration::from_millis(80));

    let outcomes = take_two(hub);

    assert_eq!(
        outcomes
            .recv_timeout(Duration::from_secs(3))
            .expect("第一个铃"),
        Woken::Changed
    );

    // 上一次唤醒已经被宿主取走了；**世界又动了** ⇒ 必须再叫一次。
    push(&incoming, Woken::Changed);
    assert_eq!(
        outcomes
            .recv_timeout(Duration::from_secs(3))
            .expect("新动静要再叫一次"),
        Woken::Changed
    );
}

/// **外部输入入口那一处也接在同一条等待上，而且不被"取一次"吃掉。**
///
/// 入口只交一条电平（容量 1、满了就丢）：这里证明它被搬成 `Woken::Channel`，并且第二次
/// 落盘还能再叫一次——不是"一辈子说一次"。
#[test]
fn a_channel_signal_wakes_the_host_and_stays_a_level() {
    let (bell, bell_source) = mpsc::channel::<Woken>();
    let (channels, channel_source) = mpsc::sync_channel::<()>(1);
    let hub = WakeHub::with_channels(
        ScriptedBell::new(bell_source),
        TimerSource::new(
            Path::new("/nonexistent-celira-wake-hub-timers"),
            ClockHandle::fixed_at_unix_seconds(1_800_000_000),
            Duration::from_millis(50),
        ),
        channel_source,
    );

    channels.send(()).expect("入口还在");
    assert_eq!(
        hub.next(),
        Woken::Channel,
        "外部输入必须被搬成 Channel 起源"
    );

    // 电平不是"一次性边沿"：又落一条，必须再叫一次。
    channels.send(()).expect("入口还在");
    assert_eq!(hub.next(), Woken::Channel, "第二次输入还要再叫一次");

    drop(bell);
}
