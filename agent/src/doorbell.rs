//! 门铃：im 服务端那条**唤醒流**的宿主侧客户端（权威契约 `CORE_V1_AGENT_PROTOCOL.md` §8）。
//!
//! 这一件只做一件事：把"世界有新动静了"这件事从一条长连接上**搬**给宿主。因此它的纪律是
//! **零事实**——线上除了一个事件名，什么都不带：
//!
//! - 建立连接成功**就是**一次唤醒（契约用它覆盖重连与订阅间隙）；
//! - 之后每来一个事件名（[`RING_EVENT`]）算一次铃；除事件名以外的每一行（包括保活的注释行）
//!   一律不看、不解释、不落任何东西；
//! - **没有 cursor、没有 Event id、没有正文**：它不产生任何事实，也不进任何存储。事实永远走
//!   REST 那条路（`GET /v1/backend/events`），而且**是主体的动作**，不是宿主的。
//!
//! 三条 fail-closed 纪律（本件自己的封闭词表 [`DoorbellFailure`]）：
//!
//! - 铃可以丢、可以重、可以断：**没答复**（连不上 / 读不出来 / 超时）⇒ **有界退避**重连
//!   （起点 1 秒、上限 30 秒；健康的连接断掉即复位——见 [`DoorbellPolicy::next_reconnect_delay`]），
//!   并把这件事记进**读数**（[`DoorbellReading`]）好让宿主打出来；
//! - **有答复但不合用**（凭据级被拒 / 超限 / 别的非 2xx / 回的媒体类型不是事件流）⇒ 报读数退出，
//!   不自动换凭据、不无限重试（重试一件注定失败的事只会变成噪声）；
//! - 铃与终局**不能互相挤掉**：铃走容量 1 的通道（满了就丢 = 合并），终局走保证送达的那条。

use std::io::{BufRead, BufReader};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// 唤醒流那条路（外部冻结形状：与四端点同一 BackendAuth 头部）。
const RING_PATH: &str = "/v1/backend/events/stream";
/// 唯一的唤醒事件名。**用它之外的名字**都不是铃（本件也不认识任何别的名字）。
const RING_EVENT: &str = "events.available";
/// 唤醒流的媒体类型（回别的类型就是"不合用"，见模块文档）。
const RING_MEDIA_TYPE: &str = "text/event-stream";

/// 宿主给门铃的**声明**：端点 + 凭据。
///
/// 它长得像 `im_app::ImAccess`，但**不是**同一件东西：那是"主体的工具面访问声明"，这是"宿主的
/// 唤醒通道声明"——两条连接、两种用途、各自的词表。凭据同样只进请求头，本类型**不派生
/// `Debug`**，因此它不可能被顺手打进日志或报告。
pub struct DoorbellAccess {
    endpoint: String,
    credential: String,
}

impl DoorbellAccess {
    /// 两样都必须显式给出且非空：缺一半不是"降级"，是没有门铃。
    pub fn new(endpoint: impl Into<String>, credential: impl Into<String>) -> Option<Self> {
        let endpoint = endpoint.into();
        let credential = credential.into();
        if endpoint.trim().is_empty() || credential.trim().is_empty() {
            return None;
        }
        Some(Self {
            endpoint,
            credential,
        })
    }

    fn ring_target(&self) -> String {
        format!("{}{RING_PATH}", self.endpoint)
    }
}

/// 建立连接与保活的参数。
///
/// 生产值见 [`DoorbellPolicy::production`]；离线用例给更小的值只是**为了把测试跑快**——它们是
/// 真实的旋钮，不是"要不要保活"的开关。
#[derive(Clone, Copy, Debug)]
pub struct DoorbellPolicy {
    /// 第一次重连要等的间隔（**起点**，不是固定值）。
    pub reconnect_backoff: Duration,
    /// 重连间隔的**上限**。
    ///
    /// 它是这一件里唯一与**花费**直接相关的数字：契约规定"建立连接就是一次唤醒"，而一次唤醒
    /// 就是一次完整 Run（一次真模型调用）。没有上限时，一个 1 秒节奏抖动的世界会把唤醒频率
    /// 拉到每分钟六十次——那不是"多花一点"，是把钱烧在重连上。有了它，最坏情况被封在
    /// `max_backoff` 一条上。
    pub max_backoff: Duration,
    /// "这条连接算健康"的门槛：活过它才把间隔**复位**。
    ///
    /// 取一个完整的空闲窗口（= [`DoorbellPolicy::read_timeout`]，也就是 3 × 服务端保活）：
    /// 活过那么久的连接断掉是正常事（代理会回收空闲长连接），不该被罚；活不过它的就是抖动，
    /// 那才是要退避的形状。
    pub stable_after: Duration,
    /// **一次读操作**的空闲上限（不是整条流的总时长）：它必须独立于普通请求那面 30s 墙——
    /// 长连接不因墙误杀，生死由服务端的保活注释判定（取 3 倍保活间隔：保活每 20 秒一次，
    /// 这里 60 秒没有收到任何一行就当这条流已经死了）。
    ///
    /// 本版 reqwest 的 blocking `timeout` 正是"连接 / 读 / 写各自的操作窗口"，因此它不会给
    /// 一条活着的长连接设总时长上限。
    pub read_timeout: Duration,
    /// **一条连接的最长寿命**（2026-10-13）：到点就**主动换新**，不等它自己断。
    ///
    /// 判据是**这一层在系统里的位置**：门铃是"及时性"的派生物（契约：「正确性来自耐久 REST
    /// 事实，及时性来自易失唤醒」「唤醒可以全丢、可重复、**可断**」）。而 `read_timeout` 只能判
    /// "对面还说不说话"——服务端的**保活注释**会让一条**活着但不再响铃**的流永远不超时。
    /// 真机读数：一个主体因此静默 **73 分钟**，而日志里一个字都没有（进程在、连接"健康"、
    /// 门铃就是不响）。
    ///
    /// 换新是**无损**的：重连后第一帧就是 `Opened`，而它的定义恰好是「**覆盖订阅间隙**：
    /// 断着的那段时间可能发生过事情」。于是"最坏多久一定会核一次持久事实"变成一个**可说的
    /// 物理量**——就是这个数（生产 60 秒：与 `SEEN_TTL`、与 3 × 保活同量级）。
    ///
    /// 它**不决定"要不要叫醒"**：换新只会让那一次唤醒来得晚一点，铃的取舍照旧归主体
    /// （见 `wake.rs` 那条"不要加门"的注）。
    pub max_age: Duration,
}

impl DoorbellPolicy {
    /// 生产值：起点 1 秒、上限 30 秒、健康门槛 60 秒（= 一个空闲窗口）、读超时 60 秒。
    pub fn production() -> Self {
        Self {
            reconnect_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(30),
            stable_after: Duration::from_secs(60),
            read_timeout: Duration::from_secs(60),
            max_age: Duration::from_secs(60),
        }
    }

    /// 下一次重连该等多久（**纯函数**：判据能单测，不必真起线程、也不必真断网）。
    ///
    /// 两条规则，各自对应一种真实形状：
    ///
    /// - **活够了的连接**（`lived >= stable_after`）或**送到过铃的连接**（`rang`）⇒ 复位成起点：
    ///   这条通道刚刚证明过它能用，下一次断掉该快点回来（一条真消息可能正等着）；
    /// - 否则 ⇒ **翻倍，封顶** `max_backoff`：抖动形状（连不上、或刚连上就断）会被越推越远。
    ///
    /// 它**不判断"要不要叫醒"**——那是唤醒政策，归门与主人；这里只决定"隔多久再试一次"。
    pub fn next_reconnect_delay(&self, current: Duration, lived: Duration, rang: bool) -> Duration {
        if rang || lived >= self.stable_after {
            return self.reconnect_backoff;
        }
        (current * 2).min(self.max_backoff)
    }
}

/// 门铃给宿主看的**读数**（进程内、不落盘、零事实）：这条唤醒通道现在什么样。
///
/// 它是**读数不是政策**：宿主拿它打一行日志，好让人看得出"这条通道一直在重连"——
/// 在此之前，传输级失败在读手线程里只表现为"睡一会儿再试"，**宿主一个字都看不到**。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DoorbellReading {
    /// 从进程开始到现在重连过几次（连不上 ＋ 流断掉，两个形状都算）。
    pub reconnects: u64,
    /// 现在这一次重连要等多久；正连着时是 `None`。
    pub backoff: Option<Duration>,
    /// 现在连着没有。
    pub connected: bool,
}

impl DoorbellReading {
    /// 宿主日志那一行里的稳定字段（**零动态文本**：固定词与数字，同一形状两次逐字节相同）。
    pub fn as_fields(&self) -> String {
        format!(
            "reconnects={} connected={} backoff-ms={}",
            self.reconnects,
            if self.connected { "yes" } else { "no" },
            self.backoff.map(|wait| wait.as_millis()).unwrap_or(0)
        )
    }

    /// 健康基线：**此刻连着、且不在退避里**。基线不打印（零动静不出行）。
    ///
    /// ⚠ **2026-10-13 改**：从前是「一次都没重连过」。有了定期换新（[`DoorbellPolicy::max_age`]）
    /// 之后 `reconnects` **必然**会涨——那是**常态**，不是病态。病态的形状是"连着又断、正在退避里等"。
    pub fn is_healthy(&self) -> bool {
        self.connected && self.backoff.is_none()
    }
}

/// 读数的**读侧**：只有一个读法，没有写入口——写入口只属于读手线程（形状，不是约定）。
#[derive(Clone, Debug, Default)]
pub struct DoorbellReadingHandle {
    latest: Arc<Mutex<DoorbellReading>>,
}

impl DoorbellReadingHandle {
    /// 最近一次写下的读数。
    pub fn latest(&self) -> DoorbellReading {
        *self.latest.lock().expect("门铃读数只在短临界区里写")
    }
}

/// 门铃自己的**封闭失败词表**：它是"这条唤醒通道怎么了"，**不是**模型失败，也不是一次工具调用
/// 的失败——三者归属不同层，谁的词表说谁的事。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoorbellFailure {
    /// 凭据 / 绑定 / Agent 任一级被拒（401 或 403）：**不自动换凭据、不重试**。
    CredentialRejected,
    /// 服务端明确超限（503）：它拒绝开这条通道。
    ChannelLimitExceeded,
    /// 其它非 2xx：服务端给了答复，而这条答复不在这件东西的契约里。
    UnexpectedResponse,
    /// 答复的媒体类型不是事件流：它答了我们一句话，但不是我们要的那条。
    UnexpectedMediaType,
    /// 读手结束了却没有留下任何终局（**不应发生**的形状）。
    ChannelLost,
}

impl DoorbellFailure {
    /// 稳定分类标记（进读数，给宿主看）。
    pub fn as_str(&self) -> &'static str {
        match self {
            DoorbellFailure::CredentialRejected => "credential-rejected",
            DoorbellFailure::ChannelLimitExceeded => "channel-limit-exceeded",
            DoorbellFailure::UnexpectedResponse => "unexpected-response",
            DoorbellFailure::UnexpectedMediaType => "unexpected-media-type",
            DoorbellFailure::ChannelLost => "channel-lost",
        }
    }
}

/// 门铃交给宿主的两样东西。
///
/// ⚠ **2026-10-13：`Ring` 拆成两态**——它从前把两件不同的事合成一个值（连接建立 / 线上来了一帧），
/// 而这两件事在**起源那一句话**上是分开的（见 [`crate::activity_origin`] 与 `composition::origin_text`）：
/// 共用一句时那句话只能二者取一，真机上它的代价是 15 段"以为有事"的空醒。
/// ⚠ **拆的只是标签，行为一个字节没改**：两态都照旧唤醒（"连接建立即一次唤醒"是契约里的
/// fail-safe，覆盖订阅间隙；收窄它会把断线窗口里的事件变成永远不响）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// **通道刚接上**（那条流的第一帧）：覆盖订阅间隙——断着的那段时间里可能发生过事情，
    /// 而这一帧**不声称**"刚刚有变化"。
    Opened,
    /// **线上来了一帧**：服务端只在事件有变化时才发（帧本身不带内容、不带事实）。
    Changed,
    /// 通道终结：宿主必须 **fail-closed** 退出并报这个分类。
    Refused(DoorbellFailure),
}

/// 宿主侧的门铃：一个后台读手 + 一条容量 1 的铃通道 + 一格**读数**。
pub struct Doorbell {
    signals: Receiver<Signal>,
    _reader: JoinHandle<()>,
    reading: DoorbellReadingHandle,
}

impl Doorbell {
    /// 起一个门铃。它**不等**连接建立（连接失败属于传输级，读手自己重连），因此本函数不失败。
    pub fn open(access: DoorbellAccess, policy: DoorbellPolicy) -> Self {
        Self::open_with_ring_sink(access, policy, None)
    }

    /// 同上，外加一个**铃回调**：每一次铃（含建立连接那一次）都会叫它一次。
    ///
    /// 它是**宿主**准备的（TASK_34：宿主在活动**跑着**的时候收到铃，就拉一次 B1 写进事件
    /// 卡片的细胞）。门铃自己**不认识**它做什么——回调是宿主的动作，门铃只负责"按铃"。
    pub fn open_with_ring_sink(
        access: DoorbellAccess,
        policy: DoorbellPolicy,
        sink: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        // 容量 1：**铃按容量合并**（连着来五个铃，对一个正在跑活动的宿主来说就是"还有动静"），
        // 而终局走 `send` 而不是 `try_send` —— 它绝不能被满通道挤掉。
        let (signals, receiver) = mpsc::sync_channel::<Signal>(1);
        let cell = Arc::new(Mutex::new(DoorbellReading::default()));
        let reading = DoorbellReadingHandle {
            latest: Arc::clone(&cell),
        };
        let reader = std::thread::spawn(move || serve(access, policy, signals, sink, cell));
        Self {
            signals: receiver,
            _reader: reader,
            reading,
        }
    }

    /// 取一格**读数**的读柄（可克隆；**没有写入口**——写只发生在读手线程里）。
    ///
    /// 宿主在把门铃交出去之前取一份留着，就能在它自己那一行
    /// 日志里说出"这条通道一直在重连"——那件事以前只发生在读手线程内部，外面一个字都看不到。
    pub fn reading_handle(&self) -> DoorbellReadingHandle {
        self.reading.clone()
    }

    /// 等下一个信号。**它永不返回"没有"**：读手是唯一发送方，通道结束就意味着"终局丢了"，
    /// 那也是一种读数（[`DoorbellFailure::ChannelLost`]），报出来而不是让宿主永远阻塞。
    pub fn recv(&self) -> Signal {
        self.signals
            .recv()
            .unwrap_or(Signal::Refused(DoorbellFailure::ChannelLost))
    }
}

/// 连与读的循环（在读手线程里跑）。
///
/// **它是这一格里唯一的读数写者**：每一次重连的形状都先写进 `cell`、再睡——因此宿主看到的
/// 读数与这条线程真正在做的事同步（不会出现"读数说在等 1 秒、其实在等 30 秒"）。
fn serve(
    access: DoorbellAccess,
    policy: DoorbellPolicy,
    signals: SyncSender<Signal>,
    sink: Option<Arc<dyn Fn() + Send + Sync>>,
    cell: Arc<Mutex<DoorbellReading>>,
) {
    let mut backoff = policy.reconnect_backoff;
    let mut reconnects: u64 = 0;
    // 下一次连接接的是不是**一条活着的订阅**（寿命到点换新来的）⇒ 那次连接**不算新订阅、不发铃**。
    // 断掉（没答复 / 服务端拒绝）之后一律复位：那才是契约说的"订阅间隙"。
    let mut renewal = false;
    loop {
        match connect(&access, &policy) {
            // 没答复：连不上。算一次重连，按当前间隔等，然后翻倍（封顶）。
            Err(Refusal::Transport) => {
                renewal = false;
                reconnects += 1;
                publish(&cell, reconnects, Some(backoff), false);
                std::thread::sleep(backoff);
                backoff = policy.next_reconnect_delay(backoff, Duration::ZERO, false);
            }
            Err(Refusal::Refused(failure)) => {
                // 终局：保证送达（通道满了就在这里等宿主取走）。**不重连、不换凭据**。
                publish(&cell, reconnects, None, false);
                let _ = signals.send(Signal::Refused(failure));
                return;
            }
            Ok(response) => {
                let connected_at = Instant::now();
                publish(&cell, reconnects, None, true);
                // 契约：**建立连接成功就是一次唤醒**（覆盖重连与订阅间隙）——而**那一次唤醒
                // 就是服务端随即发的首帧**，由 [`follow`] 作为 [`Signal::Opened`] 送出去。
                //
                // ⚠ **2026-10-13 改**：这里从前**另发**一个铃（`ring_with_sink`）——于是每次连接
                // 都会响两次：一次在这里、一次是首帧。容量 1 通常把第二个吞掉，而宿主手快时它会
                // 变成**多出来的一次空醒**（真机"每次重启 ≈ 1~2 个空铃"里那个 2）。现在只发一次，
                // 而且发的是**带标签的那一次**。
                //
                // ⚠ **2026-10-14 更正**：上面那句契约说的是**订阅间隙**；寿命到点的换新**没有间隙**
                // ⇒ 那次连接的首帧**不是一次唤醒**（真机实测：不区分这两件事 = 每 60 秒一次空醒）。
                // 判据由 [`FollowEnd::renewed`] 带给下一次连接。
                //
                // 读下去直到它断掉：一条流**断掉**永远是传输级（没答复）⇒ 重连。
                let end = follow(response, &signals, sink.as_ref(), policy.max_age, renewal);
                renewal = end.renewed;
                let lived = connected_at.elapsed();
                reconnects += 1;
                // **先算再睡**：活够了的连接（或送到过铃的连接）断掉 ⇒ 间隔复位；刚连上就断的
                // ⇒ 越推越远。顺序反了就会拿"上一次抖动攒下的 30 秒"去罚一条健康的连接。
                backoff = policy.next_reconnect_delay(backoff, lived, end.rang);
                publish(&cell, reconnects, Some(backoff), false);
                std::thread::sleep(backoff);
            }
        }
    }
}

/// 写一格读数：**唯一的写者**就是上面那条线程。
fn publish(
    cell: &Arc<Mutex<DoorbellReading>>,
    reconnects: u64,
    backoff: Option<Duration>,
    connected: bool,
) {
    *cell.lock().expect("门铃读数只在短临界区里写") = DoorbellReading {
        reconnects,
        backoff,
        connected,
    };
}

/// 一次唤醒（容量 1：满了就丢——那只表示"还有动静"，不表示"响了几次"）。
/// 线上来事件时也照样叫一次回调（宿主自己决定"现在跑着没有"）。
///
/// ⚠ **两态之间不做替换**（2026-10-13）：铃通道满了就丢，**不比较**谁更具体。丢掉的只是**标签**
/// ——世界里的那件事还在（**下一次醒来一拉就看得见**），而"哪一态先进了通道"不是需要维护的事实。
fn ring_with_sink(
    signals: &SyncSender<Signal>,
    sink: Option<&Arc<dyn Fn() + Send + Sync>>,
    signal: Signal,
) {
    if let Some(sink) = sink {
        sink();
    }
    ring(signals, signal);
}

fn ring(signals: &SyncSender<Signal>, signal: Signal) {
    match signals.try_send(signal) {
        Ok(()) | Err(TrySendError::Full(_)) => {}
        // 宿主已经不在了：读手没事可做。
        Err(TrySendError::Disconnected(_)) => {}
    }
}

/// 连接层的结果：**没答复**（重连）还是**有答复但不合用**（退出）。
enum Refusal {
    Transport,
    Refused(DoorbellFailure),
}

/// 连一次。**一次调用恰好一次连接**：不重试、不换端点。
fn connect(
    access: &DoorbellAccess,
    policy: &DoorbellPolicy,
) -> Result<reqwest::blocking::Response, Refusal> {
    let client = reqwest::blocking::Client::builder()
        .timeout(policy.read_timeout)
        .build()
        .map_err(|_| Refusal::Transport)?;
    let request = client
        .get(access.ring_target())
        .header("Authorization", format!("Bearer {}", access.credential))
        .header("Accept", RING_MEDIA_TYPE);
    let response = request.send().map_err(|_| Refusal::Transport)?;

    let status = response.status().as_u16();
    if !(200..=299).contains(&status) {
        return Err(Refusal::Refused(match status {
            401 | 403 => DoorbellFailure::CredentialRejected,
            503 => DoorbellFailure::ChannelLimitExceeded,
            _ => DoorbellFailure::UnexpectedResponse,
        }));
    }

    let media_type = response
        .headers()
        .get("Content-Type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !media_type.to_ascii_lowercase().starts_with(RING_MEDIA_TYPE) {
        return Err(Refusal::Refused(DoorbellFailure::UnexpectedMediaType));
    }

    Ok(response)
}

/// 顺着一条已经建立的流读下去，**直到它断掉**；返回**这条连接有没有送到过铃**。
///
/// 逐行读，只认一件事：**帧里有没有那个唤醒事件名**。注释行（服务端的保活）与其它任何字段
/// 一律不看——门铃不携带事实，因此这里连解析都不需要，也就不可能把事实漏进事实系统。
///
/// ⚠ **首帧是"连接建立"那一次唤醒**（2026-10-13）：契约规定"建立成功即发一次初始唤醒
/// （覆盖重连与订阅间隙）"⇒ **那一次唤醒就是这一帧**，标签是 [`Signal::Opened`]；此后每一帧
/// 都是 [`Signal::Changed`]。⇒ 每次连接**恰好一次**唤醒（从前 `serve` 另发一个，见那里的注）。
///
/// ⚠ 射程如实说：如果对面**不发**首帧（不合契约），这一次连接就没有 `Opened`——而**第一帧真变化
/// 会被标成 `Opened`**（标签偏一点，但**不丢唤醒**）。这一侧刻意选"标签偏"而不是"吞掉一次"
/// （反过来做，一个不合契约的对面就会让断线窗口里的事件永远不响）。
///
/// 它**没有**「读了一半判定为终局」的出口：一条已经建立的流断掉，永远是「没答复」（EOF /
/// 超时 / 连接重置），那就是重连；「有答复但不合用」只可能发生在开流那一刻（见 [`connect`]）。
///
/// 返回值的用处只有一处：**送到过铃**证明这条通道真的能用，于是下一次重连不必退避
/// （见 [`DoorbellPolicy::next_reconnect_delay`]）。
/// 一条流**为什么结束**——决定下一次连接算不算"新订阅"。
///
/// ⚠ **2026-10-14 加**（真机抓到的缺陷）：`max_age` 换新**没有间隙**（旧连接一直活着、只是沉默），
/// 所以它**不是一次"开"**；而"断掉之后接续"才是（契约：建立连接成功就是一次唤醒，覆盖重连与
/// 订阅间隙）。不区分这两件事的代价是真机实测的：**每 60 秒一次无谓唤醒**（主体每小时醒来 60 次、
/// 每次烧 4~8 次模型调用，真正的消息淹在噪声里）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FollowEnd {
    /// 因为**寿命到点**主动换新（订阅没有间隙）。
    renewed: bool,
    /// 这条连接里**送到过铃**（退避判据用，语义与从前一样）。
    rang: bool,
}

fn follow(
    response: reqwest::blocking::Response,
    signals: &SyncSender<Signal>,
    sink: Option<&Arc<dyn Fn() + Send + Sync>>,
    max_age: Duration,
    renewal: bool,
) -> FollowEnd {
    let mut reader = BufReader::new(response);
    let mut ringing = false;
    let mut rang = false;
    let mut opened = renewal;
    // **换新之后的首个 Wake 帧是"订阅重开"的标记，不是新动静** ⇒ 不响（见 [`FollowEnd`]）。
    let mut first = renewal;
    let started = Instant::now();
    loop {
        // **寿命到点 ⇒ 主动换新**（2026-10-13）。检查放在每次读之前：服务端的保活每 20 秒
        // 一行（`: ...`，由下面那条 `starts_with(':')` 吃掉），因此这个判据**至少每个保活
        // 间隔被看一次** ⇒ 实际生效的上界是 `max_age` ＋ 一个保活间隔。
        //
        // 为什么必须有它：只有 `read_timeout` 时，"活着但不响铃"的流（保活照来、铃声不来）
        // 会把主体冻在**无限期**的静默里——真机 73 分钟。换新拿回的是"**最坏多久一定核一次
        // 持久事实**"这个可说的量。
        //
        // ⚠ **2026-10-14 更正**：这里从前靠"重连后的 `Opened`"覆盖丢掉的那点唤醒——**那是错的**：
        // 换新**没有间隙**，于是每一次换新都真的叫醒了主体（真机：每 60 秒一次）。现在换新**不响**，
        // 由调用方带着 [`FollowEnd::renewed`] 告诉下一次连接"你接的是一条活着的订阅"。
        if started.elapsed() >= max_age {
            return FollowEnd {
                renewed: true,
                rang,
            };
        }
        let mut line = String::new();
        match reader.read_line(&mut line) {
            // EOF、超时、连接重置：都是「没答复」⇒ 返回，由调用方重连。
            Ok(0) | Err(_) => {
                return FollowEnd {
                    renewed: false,
                    rang,
                };
            }
            Ok(_) => {}
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            // 帧结束：本帧里出现过 Wake 事件名就是一次铃——**首帧**是"连接建立"那一次。
            if ringing {
                if first {
                    // 换新后的首铃：那只说明订阅重开了，**不发信号**。
                    first = false;
                } else if opened {
                    ring_with_sink(signals, sink, Signal::Changed);
                    rang = true;
                } else {
                    ring_with_sink(signals, sink, Signal::Opened);
                    rang = true;
                    opened = true;
                }
                ringing = false;
            }
            continue;
        }
        if line.starts_with(':') {
            // 注释（服务端的保活注释走这里）：不看、不解释。
            continue;
        }
        if let Some(name) = line.strip_prefix("event:") {
            ringing = ringing || name.trim() == RING_EVENT;
        }
        // `data:` 与其它任何字段：这一层**不看**（门铃没有内容可读）。
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 抖动形状：**翻倍、封顶**——上限必须真的封住（那是这一格里唯一与花费相关的数字）。
    #[test]
    fn a_flapping_channel_backs_off_up_to_the_cap() {
        let policy = DoorbellPolicy::production();
        let mut wait = policy.reconnect_backoff;
        let mut seen = vec![wait];
        for _ in 0..6 {
            wait = policy.next_reconnect_delay(wait, Duration::ZERO, false);
            seen.push(wait);
        }
        assert_eq!(
            seen,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
                Duration::from_secs(30),
                Duration::from_secs(30),
            ],
            "起点、翻倍与封顶：{seen:?}"
        );
    }

    /// 健康的连接（活够了一个空闲窗口）或**送到过铃**的连接断掉 ⇒ 间隔复位成起点；
    /// 活不过门槛又没有铃的 ⇒ 照旧翻倍封顶。
    #[test]
    fn a_healthy_connection_comes_back_at_once() {
        let policy = DoorbellPolicy::production();
        let grown = Duration::from_secs(30);
        assert_eq!(
            policy.next_reconnect_delay(grown, policy.stable_after, false),
            policy.reconnect_backoff,
            "活够了的连接不该被罚"
        );
        assert_eq!(
            policy.next_reconnect_delay(grown, Duration::from_secs(1), true),
            policy.reconnect_backoff,
            "送到过铃的连接证明通道能用，也该复位"
        );
        assert_eq!(
            policy.next_reconnect_delay(grown, Duration::from_secs(1), false),
            policy.max_backoff,
            "刚连上就断：照旧封顶"
        );
    }

    /// 读数的字段是**稳定文本**（同一形状两次逐字节相同），且"健康基线"认得出。
    #[test]
    fn the_reading_line_is_stable_and_the_healthy_baseline_is_recognisable() {
        let healthy = DoorbellReading {
            reconnects: 0,
            backoff: None,
            connected: true,
        };
        assert!(healthy.is_healthy());
        assert_eq!(
            healthy.as_fields(),
            "reconnects=0 connected=yes backoff-ms=0"
        );

        let flapping = DoorbellReading {
            reconnects: 7,
            backoff: Some(Duration::from_millis(30_000)),
            connected: false,
        };
        assert!(!flapping.is_healthy());
        assert_eq!(flapping.as_fields(), flapping.as_fields());
        assert_eq!(
            flapping.as_fields(),
            "reconnects=7 connected=no backoff-ms=30000"
        );
    }
}
