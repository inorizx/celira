//! 唤醒源（TASK_75）：门铃与**本地钟源**，汇成一条等待。
//!
//! 门铃回答"世界有新动静了吗"（im 那条唤醒流，语义一个字没改）；本地钟源回答"主体给自己约的时刻
//! 到了吗"。两件事都只是"**何时**开始一次活动"，因此它们可以共用宿主那一条等待——但**来源不同**
//! 这件事必须留住：起源行要说清是谁把主体叫醒的（[`Woken`]）。
//!
//! 三件摆设：
//!
//! - [`WakePort`]：宿主只认这个（"下一次是什么"）。[`Doorbell`] 自己就实现了它——因此只跑门铃的
//!   那条路径（离线用例、没约过时间的机器）**一个字节都不用改**；
//! - [`TimerSource`]：本地钟源。**到点才投一次**（同一个 id 不重复投），表读不出来就安静等着
//!   （宿主那一轮会如实写 `unknown`，本件不冒充"没有到点"）；
//! - [`WakeHub`]：把两处合成一条等待。**启动时先补一次**：表里已有过期但还没响过的 `pending`
//!   ⇒ 立刻投一次 timer（离线错过这件事，进程一起来就要说，不能等主体下次被别的什么叫醒）。
//!
//! ⚠️ 本件**不认识** im 的协议，也不经 im 取时间：那个钟是宿主自己的（[`ClockHandle`]）。
//!
//! **合并发生在哪一层**（2026-10-06 修）：门铃那层本来就声明了"容量 1、满了就丢——那只表示
//! '还有动静'，不表示'响了几次'"。但那条语义从前**到不了宿主**：本件的转发线程投进一条
//! **无界**通道、比宿主快，于是门铃那条容量 1 的通道被立刻抽干 ⇒ **N 个铃 = N 次 Run**。
//! 今天那条通道是**有界**的，而且**铃满了就丢、别的阻塞等**（见 [`forward`]）——合并因此
//! 真正生效：**一批动静换一次唤醒，不换 N 次**。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::clock::ClockHandle;
use crate::doorbell::{Doorbell, DoorbellFailure, Signal};
use crate::timer_app::TimerApp;

/// 下一次唤醒**来自哪儿**（或唤醒通道被终结）。
///
/// 它是宿主选起源行的唯一依据。⚠ **2026-10-13：通道那一档分成两态**——接上与报变化是两件事，
/// 而它们进主体视野的那句话**不一样**（从前共用一句，那句把"可能有新的"写成了"有新事件"，
/// 真机代价是 15 段"以为有事"的空醒，见 `agent/README.md` §3.74）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Woken {
    /// 唤醒通道**刚接上**（那条流的第一帧）：覆盖订阅间隙——断着的那段时间可能发生过事情，
    /// 而这一帧**不声称**"刚刚有变化"。
    Opened,
    /// 唤醒通道**报了一次变化**（后续帧，服务端只在事件变化时才发；帧本身不带内容）。
    Changed,
    /// 本地钟源：表里那条 `pending` 到点了（含**离线错过**的补响）。
    Timer,
    /// 唤醒通道被终结（fail-closed，宿主据此退出并报分类）。
    Refused(DoorbellFailure),
}

/// 宿主等的东西：只需回答"下一次唤醒是什么"。
pub trait WakePort {
    /// 等下一次。**它永不返回"没有"**：通道结束本身也是一种读数（[`Woken::Refused`]），
    /// 报出来而不是让宿主永远阻塞。
    fn next(&self) -> Woken;
}

impl WakePort for Doorbell {
    /// 门铃的映射：两态**一一对应**（原样搬运，不在这一层做任何判断），终局 ⇒ [`Woken::Refused`]。
    fn next(&self) -> Woken {
        match self.recv() {
            Signal::Opened => Woken::Opened,
            Signal::Changed => Woken::Changed,
            Signal::Refused(failure) => Woken::Refused(failure),
        }
    }
}

/// 本地钟源：主体那张表里**最早一条 `pending` 到点**就叫一次。
///
/// `recheck` 是"多久回头看一次表"——它同时是两个动作的间隔：等一个将在很久以后到点的时刻，
/// 和"到点了但宿主还没把它收掉"时的再看一眼。生产给它一个短值（见 [`TimerSource::production`]）；
/// 离线用例给更小的值只是为了把测试跑快，它不是"要不要看表"的开关。
pub struct TimerSource {
    root: PathBuf,
    clock: ClockHandle,
    recheck: Duration,
}

impl TimerSource {
    /// 生产值：一秒看一次表（够快跟上主体刚约的时刻，也不值得更勤）。
    pub fn production(root: &Path, clock: ClockHandle) -> Self {
        Self::new(root, clock, Duration::from_secs(1))
    }

    pub fn new(root: &Path, clock: ClockHandle, recheck: Duration) -> Self {
        Self {
            root: root.to_path_buf(),
            clock,
            recheck,
        }
    }

    fn app(&self) -> TimerApp {
        TimerApp::new(&self.root)
    }

    fn now_millis(&self) -> u64 {
        self.clock
            .now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0)
    }

    /// 现在**有没有**到点还没响过的 `pending`（离线错过也在这里被认出来）。
    pub fn due_now(&self) -> bool {
        let now = self.now_millis();
        matches!(
            self.app().next_pending(),
            Ok(Some(entry)) if entry.when_millis <= now
        )
    }

    /// 起一条线程：按上面那条纪律投信号，直到进程结束。
    ///
    /// 它是 [`WakeHub`] 用的那一条，也**可以单独接**（谁想自己消化钟源的信号就自己接）——
    /// 因此它是公开的：本件不规定"钟源只能接到宿主那一条等待上"。
    ///
    /// ⚠️ **收信号的必须是一条有界通道**（`SyncSender`，容量 ≥1）。这不是形状洁癖：钟源的信号
    /// **一个都不许丢**，而"宿主还在跑上一场"这件事必须变成**背压**（等），不能变成**堆队列**
    /// （堆起来的每一个都会在收场后换来一次多余的 Run）。本件的 [`WakeHub`] 就是这么接的。
    ///
    /// **同一个 id 只投一次**：到点了但宿主还在跑上一场（铃排队照旧）时，本件不再重复投——
    /// 那一场收场后会立刻跑到表里把它收掉（`pending → fired`），于是下一条才轮到。
    pub fn run(self, signals: SyncSender<Woken>) -> JoinHandle<()> {
        thread::spawn(move || {
            let mut signalled: Option<String> = None;
            loop {
                let now = self.now_millis();
                let wait = match self.app().next_pending() {
                    Ok(Some(entry)) if entry.when_millis <= now => {
                        if signalled.as_deref() != Some(entry.id.as_str()) {
                            if signals.send(Woken::Timer).is_err() {
                                return; // 宿主已经不在了
                            }
                            signalled = Some(entry.id.clone());
                        }
                        self.recheck
                    }
                    Ok(Some(entry)) => {
                        signalled = None;
                        // 睡到"那一刻"或下一次看表，取先到的那个。
                        let remaining = entry.when_millis.saturating_sub(now);
                        Duration::from_millis(remaining).min(self.recheck)
                    }
                    // 一条 `pending` 都没有：等着主体下次约。
                    Ok(None) => {
                        signalled = None;
                        self.recheck
                    }
                    // 表读不出来（含有一行读不懂）：安静等着——宿主那一轮会如实写 `unknown`，
                    // 本件**不**冒充"没有到点"。
                    Err(_) => {
                        signalled = None;
                        self.recheck
                    }
                };
                thread::sleep(wait);
            }
        })
    }
}

/// 唤醒队列的容量。**它必须是 1**：这是"背压"传回门铃那一层的唯一途径——宿主忙着的时候，
/// 转发线程会因为通道满而停下（或丢弃铃），门铃那层"容量 1、满了就丢"的合并语义才真正生效。
const WAKE_QUEUE: usize = 1;

/// 把门铃的信号搬进那条**有界**通道。**合并真正发生的地方就是这里**：
///
/// - **两态铃都用 `try_send`，满了就丢**——铃说的是"**还有动静**"，不是"响了几次"（门铃那一层
///   自己就是这么写的）。通道里已经有一个信号，就表示"还有动静"这件事**已经被记下了**；
///   再塞一个只是把同一件事说第二遍。丢的是**次数**，不是这件事。
///   ⚠ **两态之间也不做替换**（2026-10-13）：丢掉的只是**标签**——世界里的那件事还在
///   （**下一次醒来一拉就看得见**）。要比谁更具体，就得在这里维护一件不需要维护的事实。
/// - **别的用阻塞 `send`，一个都不许丢**——终局（`Refused`）丢了宿主就永远不知道通道断了；
///   钟源丢了就是"主体给自己约的时刻"没响。
///
/// ⚠️ **为什么不能让铃也阻塞**：一阻塞，转发线程就会**手里攥着一个铃**等宿主，于是它不再去
/// 读门铃那条容量 1 的通道——合并反而失效（在途最多会攒到 3 个）。丢，才是这里的正确动作。
///
/// ⚠️ **起源永不合并**：本函数一个信号进、一个信号出，从不把两个来源揉成一个——起源行说
/// "是谁叫醒的"，揉了就说不清了（见 [`Woken`]）。
fn forward(bell: impl WakePort + Send + 'static, signals: SyncSender<Woken>) -> JoinHandle<()> {
    thread::spawn(move || {
        loop {
            match bell.next() {
                bell @ (Woken::Opened | Woken::Changed) => match signals.try_send(bell) {
                    Ok(()) | Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => return,
                },
                other => {
                    if signals.send(other).is_err() {
                        return;
                    }
                }
            }
        }
    })
}

/// 两处来源汇成一条等待。
pub struct WakeHub {
    signals: Receiver<Woken>,
    _threads: Vec<JoinHandle<()>>,
}

impl WakeHub {
    /// 造一条合并的等待：门铃（im）＋ 本地钟源（timer）。
    ///
    /// **启动时先补一次**：表里已有过期但还没响过的 `pending` ⇒ 立刻投一次 [`Woken::Timer`]。
    /// 于是"进程重启后补响"就是这条通道的**第一个**信号——主体醒来的第一场，起源行就说 timer。
    ///
    /// 第一个参数是**任何** [`WakePort`]——门铃自己就是它，因此"只跑门铃"那条路**一个字节
    /// 不改**。
    ///
    /// ⚠️ 这里**曾经**可以装一道「门」（`crate::gate::Gate`，2026-10-06 已删）——**不要加回来**：
    /// "一次铃要不要叫醒主体"不归系统裁决（`RUNTIME_TERMINAL_DECISIONS.md` §7）。
    pub fn new(bell: impl WakePort + Send + 'static, timers: TimerSource) -> Self {
        let (sender, signals) = mpsc::sync_channel::<Woken>(WAKE_QUEUE);
        // ① 离线错过（含"上一次进程死在到点之后"）：先补，再说别的。
        if timers.due_now() {
            let _ = sender.send(Woken::Timer);
        }
        // ② 门铃转发（合并语义见 `forward`）。
        let forwarder = forward(bell, sender.clone());
        // ③ 本地钟源。
        let timer_thread = timers.run(sender);
        Self {
            signals,
            _threads: vec![forwarder, timer_thread],
        }
    }

    /// 只跑门铃的那条路径（没有本地钟源：离线用例与"这一台没约过时间"都用得上）。
    ///
    /// 与 [`WakeHub::new`] 同款：第一个参数是**任何** [`WakePort`]。
    pub fn bell_only(bell: impl WakePort + Send + 'static) -> Self {
        let (sender, signals) = mpsc::sync_channel::<Woken>(WAKE_QUEUE);
        let forwarder = forward(bell, sender);
        Self {
            signals,
            _threads: vec![forwarder],
        }
    }
}

impl WakePort for WakeHub {
    fn next(&self) -> Woken {
        // 两条来源线程都是发送方，通道结束就意味着"两边都丢了"——那也是一种读数。
        self.signals
            .recv()
            .unwrap_or(Woken::Refused(DoorbellFailure::ChannelLost))
    }
}
