//! TASK_26 验收：门铃 + 常驻宿主最小件（`doorbell` / `host_loop` / `agent_host`）。
//!
//! 这一份用例证明四件事，全部**离线**（假唤醒服务 + 脚本模型，零真实调用、零真实凭据）：
//!
//! 1. **线的形状**：连上就被唤醒一次（契约用它覆盖订阅间隙）、事件名算一次铃、保活注释不算是；
//!    **断掉就重连、重连又唤醒一次**；被拒（凭据级 / 超限 / 回错了媒体类型）⇒ 举着分类退出，
//!    **不重连、不自动换凭据**；
//! 2. **不抢占**：活动进行中到达的铃**合并成一格**，收场后**恰好多跑一次**（不是一次铃一次跑）；
//! 3. **起源如实**：门铃 Run 的起源块**逐字节**是门铃那句，而手动那句**不在**里面；
//! 4. **fail-closed 三路**：唤醒通道被拒 / 准入不放行 / 环境打不开，各自是**读数 + 退出码**，
//!    而且 `[next]` 指向真能走的下一步；
//! 5. **锚（TASK_27/29）**：它是**上一次去看投影的那一刻**——取在"去看之前"，读成了才推进，
//!    第一个 Run 播种（没有它就永远没有起点），因此相邻两次投影把时间**恰好切分**；
//! 6. **投影（TASK_28/29；TASK_73 起按事件定价）**：只带机械计数、**分母 ＋ 有新的房逐行
//!    （计数为 0 的房不点名，由分母与余数承担"没出现＝零新"）**、一行正文都没有；
//!    只在 Run 开场摆一次；读不回来时如实 `unknown`，且**不挡**这次活动。

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::{Duration, SystemTime};

use celira_agent::activity_origin::{
    MANUAL_ORIGIN_TEXT, WAKE_CHANGED_ORIGIN_TEXT, WAKE_OPENED_ORIGIN_TEXT,
};
use celira_agent::clock::{ClockHandle, anchor_value};
use celira_agent::composition::SessionDeclarations;
use celira_agent::context_reading::ContextReading;
use celira_agent::core::{
    Action, ActionId, Decision, ModelError, ModelFailureClass, ModelOutput, ModelPort, View,
};
use celira_agent::doorbell::{Doorbell, DoorbellAccess, DoorbellFailure, DoorbellPolicy, Signal};
use celira_agent::event_cards::EventCardsReport;
use celira_agent::foundation::{AgentId, Outcome, RunId, SubjectEnding};
use celira_agent::host_loop::{
    Host, HostFailure, HostOptions, RunNamer, RunWatch, WriteQualification,
};
use celira_agent::im_channel::ImAccess;
use celira_agent::model_choice::ModelChoice;
use celira_agent::provider_adapter::DeepSeekModel;
use celira_agent::store::{Environment, TracePayload};
use celira_agent::timer_app::TIMER_FILE;
use celira_agent::wake::{WakeHub, WakePort, Woken};
mod common;
use common::binary;

const AGENT: &str = "agent-host-fixture";

/// 这一份夹具**串行**跑：每个用例各自起两个回环假对端（门铃 + App 读面）并在同一进程里跑线程。
/// 串行是**资源纪律**（同一时刻只有一台夹具占用线程/端口/CPU），不是对"卡死"的修复——历史读数里
/// 串行同样会卡。卡死的根因在夹具与用例的交互里（铃通道容量 1 的合并语义被"预排队"踩中、过小的
/// 读窗口把流上静默误判成断线），本文件已在两处按纪律消除：见 [`WatchedBell`] 与
/// [`FakeRings::doorbell`] 的注释。
///
/// 这不是产品的性质——产品那边门铃有读窗口与重连、宿主有 fail-closed 三路，都由别的用例与真实
/// 联跑证明。这里只是让"夹具不撒谎"：`Fixture::new` 取到这把锁并持有到用例结束。
static SERIAL_HARNESS: Mutex<()> = Mutex::new(());
const SENTINEL: &str = "fixture-doorbell-sentinel-credential";

/// 夹具里所有"等别人"的等待上限：任何一条合法等待都在 1 秒量级，60 秒给足余量。
/// 超过它 ⇒ 带现场 panic（**卡住必须看得见，而不是无限等**——这是这次修复要立的那条纪律）。
const WAIT_LIMIT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// 假唤醒服务：一条连接一个"回合"，每一个字节都由用例驱动
// ---------------------------------------------------------------------------

/// 用例发给假服务的命令（**顺序驱动**，因此断言与行为之间没有竞态）。
enum Peer {
    /// 回状态行与头，进入数据阶段。
    Answer { status: u16, media: &'static str },
    /// 写一行（要写空行就写 `""`）。
    Write(&'static str),
    /// 断开这条连接。
    Close,
}

struct FakeRings {
    endpoint: String,
    to_peer: mpsc::Sender<Peer>,
    requests: mpsc::Receiver<String>,
    connections: Arc<AtomicUsize>,
}

impl FakeRings {
    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定回环端口");
        let port = listener.local_addr().expect("有地址").port();
        let (to_peer, peer) = mpsc::channel::<Peer>();
        let (requests, from_server) = mpsc::channel::<String>();
        let connections = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&connections);

        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                counter.fetch_add(1, Ordering::SeqCst);
                let request = read_request(&mut stream);
                let _ = requests.send(request);
                // 这条连接活着多久，由用例的命令决定。
                loop {
                    match peer.recv() {
                        Ok(Peer::Answer { status, media }) => {
                            let reason = if status == 200 { "OK" } else { "Refused" };
                            let _ = write!(
                                stream,
                                "HTTP/1.1 {status} {reason}\r\nContent-Type: {media}\r\n\r\n"
                            );
                            let _ = stream.flush();
                        }
                        Ok(Peer::Write(line)) => {
                            let _ = stream.write_all(format!("{line}\r\n").as_bytes());
                            let _ = stream.flush();
                        }
                        Ok(Peer::Close) => break,
                        // 用例走人了：整条线程收工。
                        Err(_) => return,
                    }
                }
            }
        });

        Self {
            endpoint: format!("http://127.0.0.1:{port}"),
            to_peer,
            requests: from_server,
            connections,
        }
    }

    fn doorbell(&self) -> WatchedBell {
        // **用生产同款的两个旋钮**（退避 1 秒、读窗口 60 秒）。
        //
        // 从前给的是 0ms/5s，理由是"把测试跑快"——但没有任何用例依赖"读窗口被触发"，而 5 秒的
        // 读窗口在负载下会被**误触发**：流上静默超过它，读手就弃掉这条连接去重连；假服务一次只
        // 伺候一条连接、旧连接不结束就不接新的 ⇒ 重连永远得不到服务，之后写到旧连接上的帧全部
        // 丢失、用例无限等（探针 C 实测卡住）。生产值不会在用例期间被触发，而"断开重连"这件事
        // 的覆盖不受影响——它由 `a_dead_connection_is_reconnected_and_wakes_again` 显式驱动。
        WatchedBell::open(
            DoorbellAccess::new(self.endpoint.clone(), SENTINEL).expect("声明可以建立"),
            DoorbellPolicy::production(),
        )
    }

    /// 回状态行与头；**成功的那一条还会立刻发首帧**——契约规定"建立成功即发一次初始唤醒
    /// （覆盖重连与订阅间隙）"，而门铃那一侧把**那一帧**当作"连接建立"那一次唤醒
    /// （`Signal::Opened`；2026-10-13 起：从前 `serve` 自己另发一个，于是每次连接响两次）。
    /// 假服务照契约发，因此下面那些用例里"连接建立 ⇒ 一次唤醒"照样成立。
    fn answer(&self, status: u16, media: &'static str) {
        let _ = self.to_peer.send(Peer::Answer { status, media });
        if (200..=299).contains(&status) && media == "text/event-stream" {
            self.ring_frame();
        }
    }

    fn write(&self, line: &'static str) {
        let _ = self.to_peer.send(Peer::Write(line));
    }

    /// 一个**完整的**唤醒事件帧（事件名 + 空 data + 帧结束的空行）。
    fn ring_frame(&self) {
        self.write("event: events.available");
        self.write("data:");
        self.write("");
    }

    fn close(&self) {
        let _ = self.to_peer.send(Peer::Close);
    }

    /// 给脚本模型一个"往线上写铃"的把手（见 `ScriptedModel::injecting`）。
    fn peer_sender(&self) -> mpsc::Sender<Peer> {
        self.to_peer.clone()
    }

    fn request(&self) -> String {
        self.requests
            .recv_timeout(Duration::from_secs(5))
            .expect("假服务应当收到一条连接")
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// 有上限的门铃等待：卡住要看得见，而不是无限等
// ---------------------------------------------------------------------------

/// 测试侧的门铃：把**无上限**的 `Doorbell::recv` 挪到一条转发线程上，用例线程用 `recv_timeout`
/// 等它——超限 ⇒ 带现场 panic。这是本次修复要立的那条纪律：夹具的任何"等别人"都必须**能自证卡住**。
///
/// 为什么不是直接给 `Doorbell` 加超时：那是产品的公共面（本次不动产品语义）。转发线程只搬信号，
/// **不改顺序、不去重、不合并**——铃的合并语义仍然只由门铃自己那口容量 1 的通道负责。
struct WatchedBell {
    signals: mpsc::Receiver<Signal>,
    /// 转发线程：`Doorbell::recv` 无上限，所以它可能一直挂着——**不 join**（drop 即分离）。
    _forwarder: std::thread::JoinHandle<()>,
}

impl WatchedBell {
    fn open(access: DoorbellAccess, policy: DoorbellPolicy) -> Self {
        let doorbell = Doorbell::open(access, policy);
        let (sender, signals) = mpsc::channel();
        let forwarder = std::thread::Builder::new()
            .name("test-bell".into())
            .spawn(move || {
                loop {
                    let signal = doorbell.recv();
                    // 终局之后读手就没了：搬完最后一条就收工（再读只会是 ChannelLost 噪声）。
                    let terminal = matches!(signal, Signal::Refused(_));
                    if sender.send(signal).is_err() || terminal {
                        return;
                    }
                }
            })
            .expect("起转发线程");
        Self {
            signals,
            _forwarder: forwarder,
        }
    }

    /// 等下一个信号；**有上限**（见 [`WAIT_LIMIT`]）。
    fn recv(&self) -> Signal {
        self.signals
            .recv_timeout(WAIT_LIMIT)
            .unwrap_or_else(|_| stall("门铃信号"))
    }

    /// **这段时间里安静吗**：用来钉"某个动作**没有**唤醒主体"。
    ///
    /// ⚠ 2026-10-14 加：寿命到点的换新**没有订阅间隙** ⇒ 它不许再叫一次（真机实测：不区分这两件
    /// 事 = 每 60 秒一次空醒、每小时几十次模型调用）。这条探法就是那一刀的判据。
    fn quiet_for(&self, span: Duration) -> bool {
        matches!(
            self.signals.recv_timeout(span),
            Err(mpsc::RecvTimeoutError::Timeout)
        )
    }
}

impl WakePort for WatchedBell {
    /// 与产品的映射逐字同款：两态**一一对应**，终局 ⇒ `Refused`（宿主据此 fail-closed）。
    fn next(&self) -> Woken {
        match self.recv() {
            Signal::Opened => Woken::Opened,
            Signal::Changed => Woken::Changed,
            Signal::Refused(failure) => Woken::Refused(failure),
        }
    }
}

/// 卡住时的**现场**（进程里每条线程在等什么），然后带着它 panic。
fn stall(context: &str) -> ! {
    let mut waits: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut threads = 0usize;
    if let Ok(entries) = std::fs::read_dir("/proc/self/task") {
        for entry in entries.flatten() {
            threads += 1;
            let wait = std::fs::read_to_string(entry.path().join("wchan"))
                .map(|text| text.trim().to_string())
                .unwrap_or_default();
            let wait = if wait.is_empty() {
                "running".to_string()
            } else {
                wait
            };
            *waits.entry(wait).or_insert(0) += 1;
        }
    }
    panic!(
        "夹具卡住：{context} 超过 {} 秒没有回来（现场：进程内 {threads} 条线程，等待点分布 {waits:?}）",
        WAIT_LIMIT.as_secs()
    );
}

/// **App 读面**的假对端（与门铃分开一个监听端：两条连接可以同时在跑）。
///
/// 每条连接回队列里的下一条回执，然后关掉；队列空了一律 404（"没脚本"= 读失败，**快而可见**，
/// 不会让用例挂着）。它顺便记下收到的请求原文与连接数，用来断言"每唤醒恰好一次"。
struct FakeImFace {
    endpoint: String,
    replies: Arc<Mutex<VecDeque<(u16, String)>>>,
    requests: Arc<Mutex<Vec<String>>>,
    connections: Arc<AtomicUsize>,
}

impl FakeImFace {
    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定回环端口");
        let port = listener.local_addr().expect("有地址").port();
        let replies = Arc::new(Mutex::new(VecDeque::<(u16, String)>::new()));
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let connections = Arc::new(AtomicUsize::new(0));

        let queue = Arc::clone(&replies);
        let seen = Arc::clone(&requests);
        let counter = Arc::clone(&connections);
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                counter.fetch_add(1, Ordering::SeqCst);
                let request = read_request(&mut stream);
                seen.lock().expect("锁").push(request);
                let reply = queue.lock().expect("锁").pop_front();
                let (status, body) =
                    reply.unwrap_or((404, "{\"error\":\"no-script\"}".to_string()));
                let reason = if status == 200 { "OK" } else { "Refused" };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.flush();
            }
        });

        Self {
            endpoint: format!("http://127.0.0.1:{port}"),
            replies,
            requests,
            connections,
        }
    }

    /// 排一条回执（按连接顺序）。
    fn reply(&self, status: u16, body: &str) {
        self.replies
            .lock()
            .expect("锁")
            .push_back((status, body.to_string()));
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("锁").clone()
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

/// 读一条请求（请求行 + 头），原样返回。
fn read_request(stream: &mut TcpStream) -> String {
    let mut reader = BufReader::new(stream.try_clone().expect("克隆流"));
    let mut raw = String::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if line.trim_end().is_empty() {
                    break;
                }
                raw.push_str(line.trim_end());
                raw.push('\n');
            }
            Err(_) => break,
        }
    }
    raw
}

// ---------------------------------------------------------------------------
// 脚本模型
// ---------------------------------------------------------------------------

/// 按脚本作答的模型；`seen` 记录每次调用看到的轮次（用来数"跑了几次活动"）。
struct ScriptedModel {
    script: VecDeque<ModelOutput>,
    seen: Vec<u32>,
    /// 第一次被调用时往假服务端写几条铃——这样"活动进行中到达的铃"是**真的**发生在活动里
    /// （模型调用就在活动内），不靠线程与睡眠去赌时序。
    inject: Option<(mpsc::Sender<Peer>, usize)>,
}

impl ScriptedModel {
    fn new(script: Vec<ModelOutput>) -> Self {
        Self {
            script: VecDeque::from(script),
            seen: Vec::new(),
            inject: None,
        }
    }

    /// 第一次调用时注入 `count` 条铃。
    fn injecting(mut self, to_peer: mpsc::Sender<Peer>, count: usize) -> Self {
        self.inject = Some((to_peer, count));
        self
    }

    fn calls(&self) -> usize {
        self.seen.len()
    }
}

impl ModelPort for ScriptedModel {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        self.seen.push(view.turn().number());
        if let Some((peer, count)) = self.inject.take() {
            for _ in 0..count {
                let _ = peer.send(Peer::Write("event: events.available"));
                let _ = peer.send(Peer::Write("data:"));
                let _ = peer.send(Peer::Write(""));
            }
        }
        self.script.pop_front().ok_or(ModelError {
            class: ModelFailureClass::ContractViolation,
        })
    }
}

/// 脚本用完之后的兜底：不结束、不请求行动（这样"多跑了一次"会表现为**多一次调用**，判据看得见）。
fn padding() -> Vec<ModelOutput> {
    vec![ModelOutput::finish(SubjectEnding::Silent); 8]
}

// ---------------------------------------------------------------------------
// 环境夹具
// ---------------------------------------------------------------------------

struct Fixture {
    /// 串行锁（活到用例结束；见 [`SERIAL_HARNESS`]）。
    _serial: MutexGuard<'static, ()>,
    root: PathBuf,
    environment_root: PathBuf,
    materials: PathBuf,
    agent: AgentId,
    /// 门铃那条唤醒流（用例逐字节驱动）。
    rings: FakeRings,
    /// **App 的读面**（投影读走这里）：与门铃分开一个端，因此两条连接互不干扰；
    /// 也正因为分开，"每唤醒恰好一次请求"可以单独数。
    im_face: FakeImFace,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = tempdir(name);
        let environment_root = root.join("env");
        let materials = root.join("materials");
        std::fs::create_dir_all(&materials).expect("建资料目录");
        drop(Environment::create(&environment_root, AgentId::new(AGENT)).expect("建环境"));
        Self {
            _serial: SERIAL_HARNESS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            root,
            environment_root,
            materials,
            agent: AgentId::new(AGENT),
            rings: FakeRings::spawn(),
            im_face: FakeImFace::spawn(),
        }
    }

    fn access(&self) -> ImAccess {
        // 主体的 im 动词与宿主的投影读都走**读面**那一个端；门铃是另一个端。
        ImAccess::new(self.im_face.endpoint.clone(), SENTINEL).expect("im 声明")
    }

    fn agent(&self) -> AgentId {
        self.agent.clone()
    }

    /// 一个宿主（clock 固定、屏幕 cell 空）。
    fn host<'a>(&'a self, access: &'a ImAccess) -> Host<'a> {
        self.host_with(access, ClockHandle::fixed_at_unix_seconds(1_800_000_000))
    }

    /// 同上，但**由用例给钟**——"锚取自哪一刻"这条判据需要一个**递增钟**才判得出来
    /// （固定钟下所有时刻都相等，"取在去看之前"与"取在收场之后"长得一模一样）。
    fn host_with<'a>(&'a self, access: &'a ImAccess, clock: ClockHandle) -> Host<'a> {
        self.host_with_policy(access, clock, WriteQualification::default())
    }

    /// 同上，但**写资格的耐心也由用例给**：那条路径的正常形状是"等几秒到十几秒"，
    /// 拿生产的 90 秒来验就是让用例空等，而空等既慢又证明不了上限。
    fn host_with_policy<'a>(
        &'a self,
        access: &'a ImAccess,
        clock: ClockHandle,
        write_qualification: WriteQualification,
    ) -> Host<'a> {
        let (_writer, report) = ContextReading::channel();
        Host::new(HostOptions {
            declarations: SessionDeclarations {
                usage: None,
                model: None,
                im: Some(access.clone()),
                identity: None,
                search: None,
                terminal: None,
                mcp: Vec::new(),
                ..Default::default()
            },
            environment_root: &self.environment_root,
            declared_agent: &self.agent,
            material_root: &self.materials,
            max_turns: 8,
            context_window: 128_000,
            write_qualification,
            clock,
            reading: report,
            watch: RunWatch::new(),
        })
    }

    fn trace(&self) -> String {
        std::fs::read_to_string(self.environment_root.join("trace.ndjson")).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// 0. 「在跑表」的对称性（T5）
// ---------------------------------------------------------------------------

/// [`RunWatch::begin`] 交回的句柄必须让"开始计数"与"收场"在**类型上**配对。
///
/// 只测对称性，**不测时序**：时间缝（`looked_at` 到投影读之间落进来的一声铃）在现有夹具里
/// 证不出来——那一半用类型堵（[`WatchGuard`] 的 `Drop`），不用测试堵。
#[test]
fn a_watch_guard_clears_the_running_table_when_dropped() {
    let watch = RunWatch::new();
    let (writer, _report) = EventCardsReport::channel();
    let guard = watch.begin(SystemTime::now(), writer);
    assert!(watch.running(), "begin 之后在跑表必须非空");
    drop(guard);
    assert!(
        !watch.running(),
        "drop 之后在跑表必须为空（早退路径不再把写柄留在表里）"
    );

    let (writer, _report) = EventCardsReport::channel();
    let guard = watch.begin(SystemTime::now(), writer);
    let _ = guard.end();
    assert!(!watch.running(), "end 之后同样必须为空");
}

// ---------------------------------------------------------------------------
// 1. 线的形状
// ---------------------------------------------------------------------------

#[test]
fn an_established_connection_is_already_a_wake() {
    let fixture = Fixture::new("established");
    let bell = fixture.rings.doorbell();
    fixture.rings.answer(200, "text/event-stream");

    // 第一次：**连接建立本身就是唤醒**（契约：覆盖订阅间隙）——假服务在 `answer` 里按契约
    // 发了首帧，而门铃把那条流的**第一帧**认作 `Opened`（不是另外补一个铃）。
    assert_eq!(bell.recv(), Signal::Opened);
    // 第二次：线上那个事件名 ⇒ `Changed`（此后每一帧都是"报了一次变化"）。
    // ⚠ **按序放帧**：铃通道容量是 1（重复铃按契约合并）——第二个铃必须在第一个**被消费之后**
    //    才放上线；预排队会让它被合并掉，这次等待就永远等不到（探针 A 实测卡住）。
    fixture.rings.ring_frame();
    assert_eq!(bell.recv(), Signal::Changed);

    let request = fixture.rings.request();
    assert!(
        request.starts_with("GET /v1/backend/events/stream "),
        "请求行要对得上契约: {request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer ")
            && request.contains(SENTINEL),
        "凭据进请求头: {request}"
    );
    assert!(
        request
            .to_ascii_lowercase()
            .contains("accept: text/event-stream"),
        "显式要事件流: {request}"
    );
}

#[test]
fn a_refusal_ends_the_doorbell_with_a_reading_and_no_reconnect() {
    for (status, expected) in [
        (401u16, DoorbellFailure::CredentialRejected),
        (403, DoorbellFailure::CredentialRejected),
        (503, DoorbellFailure::ChannelLimitExceeded),
    ] {
        let fixture = Fixture::new("refused");
        let bell = fixture.rings.doorbell();
        fixture.rings.answer(status, "text/event-stream");

        assert_eq!(
            bell.recv(),
            Signal::Refused(expected),
            "HTTP {status} 应当举着这个分类结束"
        );
        let _ = fixture.rings.request();
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(
            fixture.rings.connections(),
            1,
            "被拒之后**不重连**（HTTP {status}）"
        );
    }
}

#[test]
fn an_answer_that_is_not_an_event_stream_is_a_refusal_too() {
    let fixture = Fixture::new("wrong-media");
    let bell = fixture.rings.doorbell();
    fixture.rings.answer(200, "application/json");

    assert_eq!(
        bell.recv(),
        Signal::Refused(DoorbellFailure::UnexpectedMediaType)
    );
    let _ = fixture.rings.request();
    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(fixture.rings.connections(), 1, "不合用的答复也不重连");
}

/// **两态各说各的话**（2026-10-13）：同一条连接上，**首帧**是"连接建立"、后续帧是"报了一次变化"
/// ——而这两件事实在主体视野里是**两句不同的话**（起源块由词表值派生，见 `composition::origin_text`）。
///
/// 判错即红：把首帧标成"变化"（或反过来）⇒ 这里当场红；而让 `serve` 再补一个铃（旧行为）⇒
/// 第二次 `recv` 会先拿到那个补上的 `Opened`，也当场红。
#[test]
fn the_first_frame_is_a_connection_and_the_next_ones_are_changes() {
    let fixture = Fixture::new("two-states");
    let bell = fixture.rings.doorbell();
    fixture.rings.answer(200, "text/event-stream");

    assert_eq!(bell.recv(), Signal::Opened, "首帧 = 连接建立");
    fixture.rings.ring_frame();
    assert_eq!(bell.recv(), Signal::Changed, "后续帧 = 报了一次变化");
    fixture.rings.ring_frame();
    assert_eq!(bell.recv(), Signal::Changed, "再一帧还是变化");
    assert_eq!(fixture.rings.connections(), 1, "一条连接，两态分明");
}

#[test]
fn a_dead_connection_is_reconnected_and_wakes_again() {
    let fixture = Fixture::new("reconnect");
    let bell = fixture.rings.doorbell();

    // 第一条连接：建立（初始唤醒）→ 一个事件 → 断开。
    fixture.rings.answer(200, "text/event-stream");
    assert_eq!(bell.recv(), Signal::Opened);
    fixture.rings.ring_frame();
    assert_eq!(bell.recv(), Signal::Changed);
    let _ = fixture.rings.request();
    fixture.rings.close();

    // 顺序要紧：先关掉第一条 → 读手重连（假服务接受第二条，把它的请求交给我们）→ **然后**才回第二条的头。
    // 反过来的话，那条"Answer"会落到仍然活着的第一条连接上，第二条就永远等不到状态行。
    let second = fixture.rings.request();
    assert!(
        second.starts_with("GET /v1/backend/events/stream "),
        "重连走同一条路: {second}"
    );
    fixture.rings.answer(200, "text/event-stream");

    // 重连（退避 0）→ 又是一次初始唤醒（这就是"铃可以全丢"的兜底）。
    assert_eq!(bell.recv(), Signal::Opened);
    fixture.rings.ring_frame();
    assert_eq!(bell.recv(), Signal::Changed);
    assert_eq!(fixture.rings.connections(), 2, "恰好两条连接");
}

/// **通道读数**（门铃那一刀）：连不上时，重连次数与**当前退避**都写进读数——从前它只发生在
/// 读手线程内部（睡一会儿再试），宿主一个字都看不到，而每一次重连都换来一次完整 Run。
///
/// 这里用**很小的策略值**把三件事在毫秒级里走完：起点 → 翻倍 → **封顶**。
#[test]
fn a_channel_that_cannot_connect_shows_up_in_the_reading_with_a_capped_backoff() {
    let policy = DoorbellPolicy {
        reconnect_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(40),
        stable_after: Duration::from_secs(60),
        read_timeout: Duration::from_secs(5),
        // 这条用例只走"连不上"那条路：界与它无关，给生产同款值。
        max_age: Duration::from_secs(60),
    };
    // `127.0.0.1:1` 上没有人监听：连不上 = 传输级（与 `--im-endpoint` 那条参数用例同一个地址）。
    let access = DoorbellAccess::new("http://127.0.0.1:1", SENTINEL).expect("声明可以建立");
    let bell = Doorbell::open(access, policy);
    let reading = bell.reading_handle();

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let latest = reading.latest();
        assert!(!latest.connected, "连不上就不该说连着：{latest:?}");
        if latest.reconnects >= 4 {
            assert_eq!(
                latest.backoff,
                Some(policy.max_backoff),
                "退避必须封顶（这一格里唯一与花费相关的数字）：{latest:?}"
            );
            assert!(!latest.is_healthy(), "重连过的通道不是健康基线");
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "等不到读数长大：{latest:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// **一条活着但不响铃的流，必须在界内换新**（2026-10-13：真机 73 分钟静默换来的缺陷）。
///
/// 形状：流一直活着、服务端的保活照来（于是 `read_timeout` **永不触发**——它只判"对面还说
/// 不说话"），就是**不再响铃**。今天之前，这种流会把主体冻在无限期的静默里：进程在、读数说
/// "健康"、日志一个字没有，而消息进来没人理。
///
/// 判据：门铃是"及时性"的派生物（契约：唤醒可以全丢、可重复、可断），而重连后第一帧就是
/// `Opened`——它的定义恰好是「覆盖订阅间隙」。⇒ 换新无损，拿回的是"**最坏多久一定核一次持久
/// 事实**"这个可说的量。
///
/// 反例：把寿命界摘掉（回到"睡到它自己断"）⇒ 这里永远等不到换新，卡在 `recv` 上由
/// [`WAIT_LIMIT`] 当场判红。
#[test]
fn a_live_but_silent_stream_is_renewed_within_the_bound() {
    let fixture = Fixture::new("renew");
    let policy = DoorbellPolicy {
        // 只把这一格压到毫秒级：界本身是纯函数判据，与它多大无关。
        max_age: Duration::from_millis(300),
        ..DoorbellPolicy::production()
    };
    let bell = WatchedBell::open(
        DoorbellAccess::new(fixture.rings.endpoint.clone(), SENTINEL).expect("声明可以建立"),
        policy,
    );

    fixture.rings.answer(200, "text/event-stream");
    assert_eq!(bell.recv(), Signal::Opened, "首帧 = 连接建立");

    // 活着、保活照来、就是不响铃：60 条 × 10ms = 600ms > max_age(300ms)。
    for _ in 0..60 {
        fixture.rings.write(": keepalive");
        std::thread::sleep(Duration::from_millis(10));
    }

    // 换新走的是同一条路，**顺序照 `a_dead_connection_is_reconnected_and_wakes_again`**：假服务
    // 一次只伺候一条连接，旧的那条要由用例显式收掉，它才会回头 accept 新的；新的那条也要等我们
    // 回了头，客户端才会送出 `Opened`。
    fixture.rings.close();
    let renewed = fixture.rings.request();
    assert!(
        renewed.starts_with("GET /v1/backend/events/stream "),
        "换新走同一条路: {renewed}"
    );
    fixture.rings.answer(200, "text/event-stream");
    // **先等换新真的发生**：`request()` 取的是队列里那条（第一条连接的请求），它**不会**等到新连接；
    // 客户端还要睡一个退避间隔才去连（生产值 1 秒）。不等就量，量到的是"还没连上"，不是"连上了
    // 却没叫"——本用例第一次就是这么红的（`left: 1`）。从前的写法是被下面那句无上限的 `recv()`
    // 顺带等到的；现在那一趟不再有信号可等，所以这份等待必须显式写出来。
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while fixture.rings.connections() < 2 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fixture.rings.connections(), 2, "必须真的换了新连接");
    // ⚠ **2026-10-14 改判据**：换新**没有订阅间隙**（旧连接一直活着、只是沉默）⇒ 它
    // **不是一次唤醒**。从前这里断言"换新后的第一帧是 Opened"，那正是真机那条缺陷：
    // 每 60 秒一次空醒（主体每小时醒 60 次、每次 4~8 次模型调用，真消息淹在噪声里）。
    //
    // ⚠ 这个等待还必须**短于 `max_age`**（150ms < 300ms）：等久了，等待本身就会触发下一次换新。
    assert!(
        bell.quiet_for(Duration::from_millis(150)),
        "换新只是换一条管道，不许再唤醒主体一次"
    );

    // 但换新之后**真的来动静** ⇒ 照旧唤醒（抑制的只是"订阅重开"那一帧，不是这条流上的铃）。
    fixture.rings.write("event: events.available");
    fixture.rings.write("");
    assert_eq!(bell.recv(), Signal::Changed, "换新之后真的来动静，照旧唤醒");
}
// ---------------------------------------------------------------------------
// 2. 不抢占 / 合并 / 起源
// ---------------------------------------------------------------------------

#[test]
fn bells_during_a_run_merge_into_exactly_one_more_run() {
    let fixture = Fixture::new("no-preempt");
    let access = fixture.access();
    let bell = fixture.rings.doorbell();
    let mut host = fixture.host(&access);

    // 这次活动的第一次模型调用里，往线上放**三条铃**：它们到达时活动**正在进行**。
    let mut model = ScriptedModel::new(vec![
        ModelOutput::finish(SubjectEnding::Silent),
        ModelOutput::finish(SubjectEnding::Silent),
    ])
    .injecting(fixture.rings.peer_sender(), 3);

    fixture.rings.answer(200, "text/event-stream");
    let first = host
        .wake_and_run(&bell, &mut model)
        .expect("连接建立即唤醒");
    assert!(first.run.as_str().starts_with("host-"), "标识是机械生成的");
    assert_eq!(first.drives, 1, "脚本一次就收场");

    // 活动进行中到达的三条铃：**不抢占**，收场后合并成**恰好一次**活动。
    let second = host
        .wake_and_run(&bell, &mut model)
        .expect("合并后的那一格");
    assert_ne!(second.run, first.run, "两次活动不同名");
    assert_eq!(
        model.calls(),
        2,
        "三条铃合并成一次活动（不是三次）：总调用数必须是 2"
    );
}

#[test]
fn pings_are_not_bells_and_the_origin_says_doorbell() {
    let fixture = Fixture::new("pings");
    let access = fixture.access();
    let bell = fixture.rings.doorbell();
    let mut host = fixture.host(&access);
    let mut model = ScriptedModel::new(padding());

    fixture.rings.answer(200, "text/event-stream");

    // 第一次唤醒 = 连接建立（**先消费掉它，再放后面的帧**——铃通道容量 1 的按序纪律，
    // 理由与 `an_established_connection_is_already_a_wake` 那条相同）。
    let first = host.wake_and_run(&bell, &mut model).expect("初始唤醒");
    let _ = first;

    // 三条保活注释：它们不是铃。
    fixture.rings.write(": ping");
    fixture.rings.write("");
    fixture.rings.write(": ping");
    fixture.rings.write("");
    fixture.rings.write(": ping");
    fixture.rings.write("");
    // 之后才来一个真事件。
    fixture.rings.ring_frame();

    // 第二次唤醒 = 那个真事件。保活没有多叫醒一次。
    let second = host.wake_and_run(&bell, &mut model).expect("事件唤醒");
    let _ = second;
    assert_eq!(model.calls(), 2, "三次保活不该叫醒任何一次");

    // **起源如实（两态各说各的话）**：接上那句与报变化那句**各出现一次**，手动那句**不在**
    // ——而且**存下去的是两个新词表值**（旧账那个 `bell` 照读不写）。
    let trace = fixture.trace();
    assert!(
        trace.contains(WAKE_OPENED_ORIGIN_TEXT),
        "连接建立那一次：起源块必须逐字节是「接上」那句"
    );
    assert!(
        trace.contains(WAKE_CHANGED_ORIGIN_TEXT),
        "报变化那一次：起源块必须逐字节是「有变化」那句"
    );
    assert!(
        trace.contains("\"origin\":\"wake-opened\"")
            && trace.contains("\"origin\":\"wake-changed\""),
        "盘上存的是两个新词表值: {trace}"
    );
    assert!(
        !trace.contains(MANUAL_ORIGIN_TEXT),
        "门铃 Run 不许被记成宿主手动发起"
    );
}

#[test]
fn run_names_are_mechanical_and_never_repeat() {
    let mut namer = RunNamer::new();
    let first = namer.next_run_id();
    let second = namer.next_run_id();
    assert_ne!(first, second);
    assert!(
        first
            .as_str()
            .starts_with(&format!("host-{}-", std::process::id())),
        "标识是机械的: {}",
        first.as_str()
    );
    assert!(
        second
            .as_str()
            .starts_with(&format!("host-{}-", std::process::id()))
    );
}

// ---------------------------------------------------------------------------
// 3. fail-closed 三路
// ---------------------------------------------------------------------------

#[test]
fn a_refused_channel_is_a_host_failure_with_a_next_step() {
    let fixture = Fixture::new("refused-host");
    let access = fixture.access();
    let bell = fixture.rings.doorbell();
    let mut host = fixture.host(&access);
    let mut model = ScriptedModel::new(padding());

    fixture.rings.answer(401, "text/event-stream");
    let failure = host
        .wake_and_run(&bell, &mut model)
        .expect_err("被拒的通道不该被当成一次唤醒");
    assert_eq!(failure.code(), 4);
    assert_eq!(failure.as_str(), "credential-rejected");
    let next = failure.next_line(&fixture.environment_root, &fixture.agent());
    assert!(next.contains("BackendAuth"), "{next}");
    assert_eq!(model.calls(), 0, "通道被拒时一次模型都不该调用");
}

/// 已记录的保存失败**不再需要用户动作**（2026-10-06 新规矩，主人裁决）。
///
/// ⚠ **旧规矩**：这里原来断言 `failure.code() == 3`、失败分类是 `NeedsUserAction`、
/// 而 `next` 指向 `agent_host_acknowledge`（"操作者去看清并显式确认"）。
/// **新规矩**：**门是指令，而世界只该给信息**——那条故障由开场那一块纸条带给主体
/// （`PreviousActivity` 的 `StorageFailure` 那一段：哪一段停的、什么原因、有没有被确认过），
/// 能不能继续由她看着事实自己定。⇒ 照常被叫醒、照常跑一次。
///
/// **放行不等于没有记录**：那条故障照旧在准入读数里（`paused(reason=…)`），也照旧进开场纸条。
#[test]
fn a_paused_environment_is_woken_and_runs() {
    let fixture = Fixture::new("paused-host");
    {
        let mut environment =
            Environment::open(&fixture.environment_root, fixture.agent()).expect("打开环境");
        environment
            .mark_paused(&RunId::new("run-old"), "save-failed:run-start")
            .expect("写下暂停");
    }
    let access = fixture.access();
    let bell = fixture.rings.doorbell();
    let mut host = fixture.host(&access);
    let mut model = ScriptedModel::new(padding());

    fixture.rings.answer(200, "text/event-stream");
    let outcome = host.wake_and_run(&bell, &mut model);
    assert!(
        outcome.is_ok(),
        "暂停环境照常跑起来（它是事实不是闸），却得到 {:?}",
        outcome.err()
    );
    assert!(model.calls() > 0, "它真的跑了：模型被调用");
    // 那条记录照旧在盘上（这里直接读准入，不靠内存状态）。
    let reader =
        celira_agent::store::EnvironmentReader::open(&fixture.environment_root, fixture.agent())
            .expect("读者");
    assert!(
        matches!(
            reader.admission().expect("准入读数"),
            celira_agent::store::Admission::Paused { .. }
        ),
        "放行不等于没有记录"
    );
}

/// 有未收尾的活动**不再需要用户动作**（2026-09-26 新规矩）。
///
/// ⚠ **旧规矩**：这里原来断言 `failure.code() == 3`、且 next 指向 `agent_host_status`。
/// **新规矩**：进程没了就是那一段断了，新进程起来是**新的一段** ⇒ 不再拒绝；
/// 而"上一段没有收尾"由**开场那一轮**如实带给主体（见 `PreviousActivity`）。
///
/// **放行不等于解决**：旧痕迹仍然如实留在盘上——那半句由 `durable_environment.rs`
/// 里那两条准入用例守着（准入读数仍然说 `unfinished`、`list_runs` 仍然报 `interrupted`）。
#[test]
fn an_unfinished_environment_no_longer_needs_a_user_action() {
    let fixture = Fixture::new("unfinished-host");
    {
        let mut environment =
            Environment::open(&fixture.environment_root, fixture.agent()).expect("打开环境");
        environment
            .append_trace(
                &RunId::new("run-left"),
                TracePayload::RunStarted { origin: None },
            )
            .expect("登记一个没收尾的活动");
    }
    let access = fixture.access();
    let bell = fixture.rings.doorbell();
    let mut host = fixture.host(&access);
    let mut model = ScriptedModel::new(padding());

    fixture.rings.answer(200, "text/event-stream");
    let outcome = host.wake_and_run(&bell, &mut model);
    assert!(
        outcome.is_ok(),
        "未收尾环境不再拦住新活动，却得到 {:?}",
        outcome.err()
    );
}

#[test]
fn an_unreadable_environment_is_a_failure_not_a_pause() {
    let fixture = Fixture::new("bad-root");
    let access = fixture.access();
    let bell = fixture.rings.doorbell();
    let missing = fixture.root.join("not-here");
    let (_writer, report) = ContextReading::channel();
    let agent = fixture.agent();
    let mut host = Host::new(HostOptions {
        declarations: SessionDeclarations {
            usage: None,
            model: None,
            im: Some(access.clone()),
            identity: None,
            search: None,
            terminal: None,
            mcp: Vec::new(),
            ..Default::default()
        },
        environment_root: &missing,
        declared_agent: &agent,
        material_root: &fixture.materials,
        max_turns: 4,
        context_window: 128_000,
        write_qualification: WriteQualification::default(),
        clock: ClockHandle::fixed_at_unix_seconds(1_800_000_000),
        reading: report,
        watch: RunWatch::new(),
    });
    let mut model = ScriptedModel::new(padding());

    fixture.rings.answer(200, "text/event-stream");
    let failure = host
        .wake_and_run(&bell, &mut model)
        .expect_err("打不开就不该跑");
    assert_eq!(failure.code(), 2);
    assert_eq!(failure.as_str(), "environment-unusable");
    assert_eq!(model.calls(), 0);
}

// ---------------------------------------------------------------------------
// 3b. 写资格被别人握着：等，不判死（2026-09-26 真机那一刀）
// ---------------------------------------------------------------------------

/// 把环境的**写资格**从用例这边握住（模拟"此刻有别人在里面"）。
///
/// 直接用操作系统锁、**不经 `Environment`**：要的就是"别人"这个位置。如果这里也用
/// `Environment::open`，握住的其实是**同一把**锁，那证明的就不是争用而是自锁。
fn hold_write_qualification(environment_root: &std::path::Path) -> std::fs::File {
    let holder = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(environment_root.join(".writer.lock"))
        .expect("打开写资格锁文件");
    holder.try_lock().expect("用例应当能握住写资格");
    holder
}

/// **唤醒撞上"别人正握着写资格" ⇒ 等它放开，照常跑完**（不判死）。
///
/// 这一格就是真机踩到的那个死法：家的**只读状态口**按既有规则拿同一把排他锁、一次读十几秒
/// （账本越大越久），于是"打开页面 → 发一条消息 → 门铃响"这条最普通的路径里，唤醒撞上读的
/// 概率极高。从前撞上就 `environment-unusable` 退出 ⇒ **一个只读的观察窗可以永久杀死常驻主体**。
#[test]
fn a_wake_waits_for_the_write_qualification_instead_of_dying() {
    let fixture = Fixture::new("wq-wait");
    let access = fixture.access();
    let bell = fixture.rings.doorbell();

    let holder = hold_write_qualification(&fixture.environment_root);
    // 握住一会儿再放开：唤醒必须**等**过去，而不是当场判死。
    let released = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        drop(holder);
    });

    let mut host = fixture.host_with_policy(
        &access,
        ClockHandle::fixed_at_unix_seconds(1_800_000_000),
        WriteQualification {
            patience: Duration::from_secs(10),
            retry: Duration::from_millis(20),
        },
    );
    let mut model = ScriptedModel::new(padding());

    fixture.rings.answer(200, "text/event-stream");
    let wake = host
        .wake_and_run(&bell, &mut model)
        .expect("等到了就该跑完——写资格被暂时握着不是环境不可用");
    assert!(wake.run.as_str().starts_with("host-"), "标识是机械生成的");
    assert_eq!(model.calls(), 1, "等到之后照常跑一次活动");
    released.join().expect("放锁线程不应 panic");
}

/// **等满耐心仍被握着 ⇒ `environment-busy`，不是 `environment-unusable`。**
///
/// 两类的动作不同：前者是"去看谁在里面"，后者是"去修环境"。合并成一条读数会让操作者
/// 去重建一个**没坏**的环境——而那正是这条读数唯一要说清的事。
#[test]
fn a_write_qualification_held_past_patience_is_busy_not_unusable() {
    let fixture = Fixture::new("wq-busy");
    let access = fixture.access();
    let bell = fixture.rings.doorbell();

    // 全程握着：这次唤醒等不到。
    let _holder = hold_write_qualification(&fixture.environment_root);

    let mut host = fixture.host_with_policy(
        &access,
        ClockHandle::fixed_at_unix_seconds(1_800_000_000),
        WriteQualification {
            patience: Duration::from_millis(150),
            retry: Duration::from_millis(20),
        },
    );
    let mut model = ScriptedModel::new(padding());

    fixture.rings.answer(200, "text/event-stream");
    let failure = host
        .wake_and_run(&bell, &mut model)
        .expect_err("一直握着就不该跑");
    assert_eq!(failure.as_str(), "environment-busy");
    assert_eq!(
        failure.code(),
        6,
        "退出码是读数语义，与 environment-unusable 的 2 分开"
    );
    assert_eq!(model.calls(), 0, "没拿到写资格就不许碰模型");
    match failure {
        HostFailure::EnvironmentBusy { waited_ms } => {
            assert!(
                waited_ms >= 150,
                "等待时长要如实报出来，得到 {waited_ms} ms"
            );
        }
        other => panic!("分类必须是 EnvironmentBusy，得到 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 3c. 换模型那一刻：**模型那一组声明一起换**（名字 / 思考强度 / 窗口，2026-10-08）
// ---------------------------------------------------------------------------

/// 一次唤醒里开场那一行（机器处境块）——从**盘上的经历**里取（进账的同一份）。
fn opening_machine_line(fixture: &Fixture) -> String {
    for record in trace_records(fixture) {
        let TracePayload::Injection { stage, blocks, .. } = record.entry() else {
            continue;
        };
        if stage != "run-start" {
            continue;
        }
        for (label, text) in blocks {
            if label == "machine" {
                return text.clone();
            }
        }
    }
    panic!("盘上应当有一条 run-start 注入，里面应当有机器处境那一块");
}

/// 盘上每一条状态行读数（`(run, 正文)`）——"窗口跟着换"那条判据看它。
fn status_lines(fixture: &Fixture) -> Vec<String> {
    let mut out = Vec::new();
    for record in trace_records(fixture) {
        let TracePayload::Injection { blocks, .. } = record.entry() else {
            continue;
        };
        for (label, text) in blocks {
            if label == "status-bar" {
                out.push(text.clone());
            }
        }
    }
    out
}

/// **换模型那一刻三格一起换**：名字 / 思考强度 / 窗口。
///
/// 为什么是一个用例钉三格：这三格**都随模型走**（窗口是模型自己的标称值、档位是清单那一条声明的），
/// 而权威是**同一个对象**。分开换会留下"名字换了、档位还是上一份"的读数——主体读到的那一行
/// 与真正发出去的请求对不上，而那一行正是她判断"我现在用的是什么"的唯一依据。
#[test]
fn switching_the_model_switches_the_whole_declared_group_at_once() {
    let fixture = Fixture::new("model-switch");
    let access = fixture.access();
    let bell = fixture.rings.doorbell();
    let (_writer, report) = ContextReading::channel();
    let agent = fixture.agent();

    let mut host = Host::new(HostOptions {
        environment_root: &fixture.environment_root,
        declared_agent: &agent,
        material_root: &fixture.materials,
        max_turns: 4,
        context_window: 1_000,
        write_qualification: WriteQualification::default(),
        clock: ClockHandle::fixed_at_unix_seconds(1_800_000_000),
        reading: report,
        declarations: SessionDeclarations {
            model: Some("old-model".to_string()),
            reasoning_effort: Some("low".to_string()),
            im: Some(access.clone()),
            ..Default::default()
        },
        watch: RunWatch::new(),
    });
    let mut model = ScriptedModel::new(padding());

    // 换一套：名字、档位、窗口**一起**换（同一个对象给的三个值）。
    let next = ModelChoice::Chat(
        DeepSeekModel::at_endpoint(
            SENTINEL,
            "next-model",
            "http://127.0.0.1:1/v1/chat/completions",
        )
        .expect("装配")
        .with_reasoning_effort(Some("high".to_string())),
    );
    host.declare_model_assembly(&next, 2_048);

    fixture.rings.answer(200, "text/event-stream");
    host.wake_and_run(&bell, &mut model)
        .expect("这一次唤醒该跑完");

    let line = opening_machine_line(&fixture);
    assert!(
        line.ends_with(" · model = next-model · reasoning_effort = high"),
        "三格必须一起换成新的那一套: {line}"
    );
    let readings = status_lines(&fixture);
    assert!(
        readings.iter().any(|text| text.contains("/2048")),
        "窗口也要跟着换（它同样是随模型走的声明）: {readings:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. 入口（bin）：参数、帮助、读数
// ---------------------------------------------------------------------------

/// 跑一次宿主入口。`model_key` 与 `im_credential` 分开给：这样"缺哪一样"是可以分开判的。
fn host_bin(arguments: &[&str], model_key: Option<&str>, im_credential: bool) -> Output {
    let mut command = Command::new(binary("agent_host"));
    for argument in arguments {
        command.arg(argument);
    }
    command
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("CELIRA_IM_BACKEND_AUTH")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if im_credential {
        command.env(
            "CELIRA_IM_BACKEND_AUTH",
            "fixture-backend-credential-not-a-real-one",
        );
    }
    if let Some(key) = model_key {
        command.env("DEEPSEEK_API_KEY", key);
    }
    // **有上限地等它退出**：这条入口是 fail-closed 的（读不出来就带码退出），正常都在 1 秒内收工；
    // 超过上限 ⇒ 带现场 panic，而不是把整个目标挂住（"等别人"必须有上限）。
    let mut child = command.spawn().expect("启动 agent_host");
    let deadline = std::time::Instant::now() + WAIT_LIMIT;
    loop {
        if child.try_wait().expect("等 agent_host").is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            stall("agent_host 子进程退出");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().expect("收 agent_host 的输出")
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn the_help_path_and_the_argument_errors_are_readings() {
    let help = host_bin(&["--help"], None, false);
    assert_eq!(code(&help), 0);
    assert!(stdout(&help).contains("用法:"));

    for arguments in [
        vec![],
        vec!["--root", "env"],
        vec!["--root", "env", "--agent", AGENT],
        vec!["--root", "env", "--agent", AGENT, "--materials", "m"],
        // 有端点但**没有**凭据：门铃与 im 都要它，缺了就是参数错。
        vec![
            "--root",
            "env",
            "--agent",
            AGENT,
            "--materials",
            "m",
            "--max-turns",
            "4",
            "--im-endpoint",
            "http://127.0.0.1:1",
        ],
    ] {
        let output = host_bin(&arguments, None, false);
        assert_eq!(code(&output), 1, "参数错: {arguments:?}");
    }
}

/// 起一个**会一直听着**的 `agent_host`，读到它开始听（或它退出 / 超时），然后杀掉它。
///
/// 为什么需要它（2026-10-06）：暂停不再是闸之后，这条路**不再 fail-closed 退出**——它照常起来、
/// 照常听。因此判据从"退出码 3"改成"**它没退出、而且开始听了**"，而"等它"照旧**有上限**
/// （读线程 + 截止时间 + 杀掉，见本仓库那条纪律：等别人必须有上限）。
fn host_bin_listening(arguments: &[&str]) -> String {
    use std::sync::{Arc, Mutex};

    let mut command = Command::new(binary("agent_host"));
    for argument in arguments {
        command.arg(argument);
    }
    command
        .env_remove("DEEPSEEK_API_KEY")
        .env_remove("CELIRA_IM_BACKEND_AUTH")
        .env(
            "CELIRA_IM_BACKEND_AUTH",
            "fixture-backend-credential-not-a-real-one",
        )
        .env("DEEPSEEK_API_KEY", "fixture-key")
        .env("NO_PROXY", "127.0.0.1,localhost")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("启动 agent_host");
    let stdout = child.stdout.take().expect("取 stdout");
    let seen = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&seen);
    let reader = std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        while let Ok(read) = reader.read_line(&mut line) {
            if read == 0 {
                break;
            }
            sink.lock().expect("读到的输出").push_str(&line);
            line.clear();
        }
    });

    let deadline = std::time::Instant::now() + WAIT_LIMIT;
    loop {
        if seen.lock().expect("读到的输出").contains("[listening]") {
            break;
        }
        if child.try_wait().expect("等 agent_host").is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();
    seen.lock().expect("读到的输出").clone()
}

/// **已记录的保存失败不再让宿主提前退出**（2026-10-06 新规矩，主人裁决）。
///
/// ⚠ **旧规矩**：这里断言宿主**在开始听之前**就以码 3 退出（"需要你动手"）、`next` 指向
/// `agent_host_acknowledge`、并且**没有** `[listening]`。
/// **新规矩**：**门是指令，而世界只该给信息**——暂停是**事实**不是闸，由开场那一块纸条带给
/// 主体（`PreviousActivity` 的 `StorageFailure` 那一段）。⇒ 它照常起来、照常听，
/// 而那格读数照旧打出来（**放行不等于没有记录**）。
#[test]
fn a_paused_environment_no_longer_makes_the_host_exit_before_it_listens() {
    let fixture = Fixture::new("bin-paused");
    {
        let mut environment =
            Environment::open(&fixture.environment_root, fixture.agent()).expect("打开环境");
        environment
            .mark_paused(&RunId::new("run-old"), "save-failed:run-start")
            .expect("写下暂停");
    }

    let text = host_bin_listening(&[
        "--root",
        fixture.environment_root.to_str().expect("路径"),
        "--agent",
        AGENT,
        "--materials",
        fixture.materials.to_str().expect("路径"),
        "--max-turns",
        "4",
        "--im-endpoint",
        &fixture.rings.endpoint,
    ]);
    assert!(
        text.contains("admission=paused(reason=save-failed:run-start)"),
        "那条记录照旧打出来: {text}"
    );
    assert!(
        text.contains("[listening]"),
        "暂停不再是闸：它照常起来听着: {text}"
    );
    assert!(
        !text.contains("needs-user-action"),
        "那一档已经退役（今天没有'需要你动手'）: {text}"
    );
}

#[test]
fn a_missing_model_credential_is_exit_five() {
    let fixture = Fixture::new("bin-nokey");
    let output = host_bin(
        &[
            "--root",
            fixture.environment_root.to_str().expect("路径"),
            "--agent",
            AGENT,
            "--materials",
            fixture.materials.to_str().expect("路径"),
            "--max-turns",
            "4",
            "--im-endpoint",
            &fixture.rings.endpoint,
        ],
        None,
        true,
    );
    assert_eq!(code(&output), 5, "{}", stdout(&output));
}

fn tempdir(name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "celira-host-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("时钟")
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).expect("建临时目录");
    path
}

// ---------------------------------------------------------------------------
// 5. 锚：给「新」一个起点（TASK_27 S1 / TASK_29 口径）
// ---------------------------------------------------------------------------

/// 一个**递增钟**：每读一次往前一秒。
///
/// "锚取自哪一刻"只有在钟会走的时候才判得出来——固定钟下 `looked_at`、开场读数里的 `time`、
/// 收场时刻全是同一个数，任何"取在哪一步"的断言都会恒真（这正是判据必须可判错的那条纪律）。
fn ticking_clock(start_seconds: u64) -> ClockHandle {
    let counter = Arc::new(AtomicUsize::new(start_seconds as usize));
    ClockHandle::of(move || {
        let tick = counter.fetch_add(1, Ordering::SeqCst) as u64;
        std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(tick)
    })
}

/// 一个时刻在**这台机器上**的可读形状（与状态行那一格**同一个形态**）。
///
/// ⚠ 为什么不能直接用 UTC 的形状去比：状态行的 `time` 现在带**本机偏移**，而**字符串序即时序**
/// 只在两边偏移相同时才成立（西半球的机器上，`…T19:00:00-05:00` 会排在同一天的 UTC 写法之前）
/// ⇒ 比较的两边必须是**同一个形态**。
fn readable(at: std::time::SystemTime) -> String {
    celira_agent::clock::format_local(at, celira_agent::clock::LocalOffset::of_machine())
}

/// 某一轮活动里所有状态行读数（按出现顺序）。
fn readings_of(fixture: &Fixture, run: &RunId) -> Vec<String> {
    status_readings(fixture)
        .into_iter()
        .filter(|(each, _)| each == run.as_str())
        .map(|(_, text)| text)
        .collect()
}

/// **第一次唤醒播种、之后每次的锚都是"上一次去看的那一刻"**，而且那一取**在开场之前**。
///
/// 三样一起判：
/// 1. 第一个 Run 没有可比对的边界 ⇒ 状态行与通知栏都如实 `unknown`（**没锚也照读**：增量那一半
///    问不出来，未读那一半按世界记着的读位置算——2026-10-10 起不再是"不读"）；
/// 2. 该 Run 仍要**种下锚**，否则下一次还是没有起点；
/// 3. 第二次的 `since` **恰好**等于第一次取下的 `anchor_at`——用递增钟判，所以它
///    与"开场读数的时刻"、"收场的时刻"都不同值，取错任何一处都会红。
#[test]
fn the_first_wake_of_a_process_has_no_anchor_and_later_ones_do() {
    let fixture = Fixture::new("anchor");
    let access = fixture.access();
    let bell = fixture.rings.doorbell();
    let mut host = fixture.host_with(&access, ticking_clock(1_800_000_000));
    let mut model = ScriptedModel::new(pad(3));

    fixture.rings.answer(200, "text/event-stream");
    let first = host.wake_and_run(&bell, &mut model).expect("第一次唤醒");

    // 第二次：由一条真铃叫醒（判据落在"第二次的锚 = 第一次去看的时刻"上）。
    fixture.rings.ring_frame();
    let second = host.wake_and_run(&bell, &mut model).expect("第二次唤醒");

    // 每次活动都会摆多次状态行（开场一条 + 每轮一条），因此按**活动**分组看。
    let first_run = readings_of(&fixture, &first.run);
    let second_run = readings_of(&fixture, &second.run);
    assert!(
        !first_run.is_empty() && !second_run.is_empty(),
        "两次活动都要有状态读数"
    );
    assert!(
        first_run
            .iter()
            .all(|text| text.ends_with("since = unknown")),
        "新进程的第一个清醒期**如实说不知道**，不编造时刻: {first_run:?}"
    );

    // 取钟**在开场之前**：那一刻一定早于第一次活动开场读数里的 `time`（递增钟因此可判）。
    let opening = first_run
        .last()
        .and_then(|text| text.lines().next())
        .and_then(|line| line.strip_prefix("time = "))
        .expect("开场读数第一行是 time");
    // 比的是**时刻**、不是形状：锚那一栏现在是十进制 unix 毫秒（主体动词吃的形状），
    // 这里拿**同一个时刻**的可读形态与开场读数比——同一口钟、同一精度、**同一个形态**
    // （两边都带本机偏移），字符串序即时序。
    //
    // ⚠ **2026-10-10 改：锚取在"这一段收场之后"，不再取在"去看之前"**（判据反过来）。
    // 真机实测（主体自己报的）：锚取在段**开头**时，她**在场**时到的消息落进**下一次**的
    // 增量窗口 ⇒ 下一段的通知栏仍数着她**已经答过的**那些 ⇒ 她为它们白醒一段（"补叫"）。
    // 现在增量那一半的窗口 = **她不在场的那段时间**（"我睡着的时候有多少"），
    // 而她醒着时到的消息归**事件卡片**那一格；"不漏"由**未读**那一半兜底（与锚无关）。
    assert!(
        readable(first.anchor_at).as_str() > opening,
        "锚取在**收场之后**（这一段结束了才算切分点）：锚 {} 应当晚于开场那一刻 {opening}",
        readable(first.anchor_at)
    );
    // 更强的判据（同一条纪律）：锚**不早于这一段的最后一轮**——也就是"收场"那一侧，
    // 而不是"开场"那一侧。判据落在**时刻**上：最后一次注入读数第一行那个 `time`。
    let last_turn = first_run
        .last()
        .and_then(|text| text.lines().next())
        .and_then(|line| line.strip_prefix("time = "))
        .expect("最后一次注入读数第一行是 time");
    assert!(
        readable(first.anchor_at).as_str() >= last_turn,
        "锚必须落在**这一段收场之后**（不早于最后一轮）：锚 {} vs 最后一轮 {last_turn}",
        readable(first.anchor_at)
    );

    // 第二次 Run 的起点 = 第一次取下的锚（同一个格式化点，形状是 unix 毫秒）。
    let expected = format!("since = {}", anchor_value(first.anchor_at));
    assert!(
        second_run.iter().all(|text| text.ends_with(&expected)),
        "第二次清醒期的起点必须是第一次去看的时刻（{expected}）: {second_run:?}"
    );
    assert!(
        second.anchor_at > first.anchor_at,
        "钟只往前走（递增钟下这条断言才不是恒真）"
    );
}

/// **读没成 ⇒ 锚不动**：窗口继续延伸，下次一并报出来（宁可多报，不可漏报）。
///
/// 判据落在"下一次请求带的 `since`"上：它必须仍是**第一次**那个锚，而不是这一次去看的时刻。
#[test]
fn a_failed_projection_does_not_advance_the_anchor() {
    let fixture = Fixture::new("anchor-failed-read");
    // **三场各读一次**（2026-10-10：没有锚的那一场**也读**——未读那一半不需要锚）：
    // 第 1 场成功（拿锚），第 2 场失败（500），第 3 场成功。
    fixture.im_face.reply(200, &rooms_body());
    fixture.im_face.reply(500, "{\"error\":\"synthetic\"}");
    fixture.im_face.reply(200, &rooms_body());
    let access = fixture.access();
    let bell = fixture.rings.doorbell();
    let mut host = fixture.host_with(&access, ticking_clock(1_800_000_000));
    let mut model = ScriptedModel::new(pad(4));

    fixture.rings.answer(200, "text/event-stream");
    let first = host.wake_and_run(&bell, &mut model).expect("第一次唤醒");
    fixture.rings.ring_frame();
    let second = host.wake_and_run(&bell, &mut model).expect("第二次唤醒");
    fixture.rings.ring_frame();
    let third = host.wake_and_run(&bell, &mut model).expect("第三次唤醒");

    assert!(!second.notification, "500 ⇒ 这一次没读到");
    assert!(third.notification, "下一次读到了");
    assert_ne!(
        second.anchor_at, first.anchor_at,
        "钟往前走 ⇒ 下面那条断言不是恒真"
    );

    let third_run = readings_of(&fixture, &third.run);
    assert!(
        third_run
            .iter()
            .all(|text| text.ends_with(&format!("since = {}", anchor_value(first.anchor_at)))),
        "读失败**不推进**锚：窗口继续延伸（起点仍是第一次那个）: {third_run:?}"
    );

    // 三场都真去读了；**第 1 场没有锚**（不带 `since=`），第 2 场失败不算推进，
    // 因此第 3 场带的仍是第 1 场那个起点——被跳过的那一段仍然落在窗口里。
    let requests = fixture.im_face.requests();
    assert_eq!(requests.len(), 3, "三场各读一次（第 1 场没有锚，照读）");
    assert!(
        !requests[0].contains("since="),
        "第一场没有锚 ⇒ 不带那个参数（未读那一半不需要锚）: {}",
        requests[0]
    );
    let since_of = |request: &str| {
        request
            .split("since=")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .map(str::to_string)
            .expect("请求带 since")
    };
    assert_eq!(
        since_of(&requests[1]),
        since_of(&requests[2]),
        "两次带锚的请求用**同一个**起点（中间那次失败没有把窗口切短）"
    );
    assert_eq!(
        since_of(&requests[1]),
        (first
            .anchor_at
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .expect("正时刻")
            .as_millis())
        .to_string(),
        "起点就是第一次去看那一刻（毫秒十进制，与端点约定一致）"
    );
}

/// 宿主声明了起点 ⇒ 状态行照它渲染；没声明 ⇒ 如实 `unknown`（**走真实装配路径**，
/// 因为 `InjectionContext` 没有公开构造器——Core 不给，本刀也不改 Core）。
#[test]
fn a_declared_since_is_rendered_and_an_absent_one_stays_unknown() {
    use celira_agent::composition::{RunOrigin, RunSession, SessionDeclarations, SessionRequest};
    use celira_agent::core::{Limits, RunInput};

    fn reading_with_since(name: &str, since: Option<std::time::SystemTime>) -> String {
        let fixture = Fixture::new(name);
        let access = fixture.access();
        let run = celira_agent::foundation::RunId::new("run-anchor");
        let request = SessionRequest {
            declarations: SessionDeclarations {
                im: Some(access.clone()),
                // 其余几格一件都不声明（`Default` 就是"全都没声明"）。
                ..Default::default()
            },
            declared_agent: &fixture.agent,
            run: run.clone(),
            limits: Limits::new(4),
            input: RunInput::empty(),
            clock: ClockHandle::fixed_at_unix_seconds(1_800_000_100),
            context_window: 1_000,
            reading: ContextReading::channel().1,
        };
        let mut environment =
            Environment::open(&fixture.environment_root, fixture.agent.clone()).expect("打开环境");
        let mut session = RunSession::open_declared(
            &fixture.materials,
            &mut environment,
            RunOrigin {
                cards: None,
                origin: celira_agent::store::WakeOrigin::Changed,
                since,
                notification: None,
            },
            request,
        )
        .expect("装配");
        let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
        let _ = session.drive(&mut model).expect("驱动");
        drop(session);
        status_readings(&fixture)
            .into_iter()
            .map(|(_, text)| text)
            .next()
            .expect("有状态读数")
    }

    let declared =
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000);
    let with_anchor = reading_with_since("anchor-declared", Some(declared));
    assert!(
        // 声明的时刻照**同一个格式化点**渲染成十进制 unix 毫秒（主体可以原样递给 im_rooms）：
        // 1_800_000_000 秒 = 1_800_000_000_000 毫秒。
        with_anchor.ends_with("since = 1800000000000"),
        "声明的时刻照同一个格式化点渲染: {with_anchor}"
    );
    let without = reading_with_since("anchor-absent", None);
    assert!(
        without.ends_with("since = unknown"),
        "没声明就是不知道（默认值不动既有调用点）: {without}"
    );
}

/// **`open()` 是委托，不是第二份执行链**（TASK_26 裁决的前提条件）。
///
/// 判据是机械的：同一份输入、同一份声明（`RunOrigin::manual()`）分别走两个入口，
/// 两次留下的经历**逐字节相同**——如果 `open` 自己走了一条路，这里就会不一样。
#[test]
fn open_delegates_instead_of_running_a_second_chain() {
    use celira_agent::composition::{RunOrigin, RunSession, SessionDeclarations, SessionRequest};
    use celira_agent::core::{Limits, RunInput};

    fn drive_once(name: &str, via_declared: bool) -> String {
        let fixture = Fixture::new(name);
        let access = fixture.access();
        let run = celira_agent::foundation::RunId::new("run-delegation");
        let request = || SessionRequest {
            declarations: SessionDeclarations {
                im: Some(access.clone()),
                // 其余几格一件都不声明（`Default` 就是"全都没声明"）。
                ..Default::default()
            },
            declared_agent: &fixture.agent,
            run: run.clone(),
            limits: Limits::new(4),
            input: RunInput::empty(),
            clock: ClockHandle::fixed_at_unix_seconds(1_800_000_000),
            context_window: 1_000,
            reading: ContextReading::channel().1,
        };
        let mut environment =
            Environment::open(&fixture.environment_root, fixture.agent.clone()).expect("打开环境");
        let mut session = if via_declared {
            RunSession::open_declared(
                &fixture.materials,
                &mut environment,
                RunOrigin::manual(),
                request(),
            )
            .expect("装配")
        } else {
            RunSession::open(&fixture.materials, &mut environment, request()).expect("装配")
        };
        let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
        assert!(matches!(
            session.drive(&mut model).expect("驱动"),
            celira_agent::composition::SessionDrive::Ended(_)
        ));
        drop(session);
        std::fs::read_to_string(fixture.environment_root.join("trace.ndjson")).expect("读经历")
    }

    let via_open = drive_once("delegation-a", false);
    let via_declared = drive_once("delegation-b", true);
    assert!(
        !via_open.is_empty() && !via_declared.is_empty(),
        "两次都要真的跑起来"
    );
    assert_eq!(
        via_open, via_declared,
        "两个入口必须留下**逐字节相同**的经历：open 只是委托"
    );
}

/// 盘上的全部经历：走**生产读路径**（`read_trace_at`），不手解存储格式。
///
/// ⚠ **2026-10-06 改**（校验和那一刀）：从前这些帮手各自 `serde_json::from_str(line)` 手解每一行——
/// 而行尾现在带校验和（`\t<8 位十六进制>`），手解会撞上"trailing characters"。
/// 判据是：**夹具不该认识存储格式**——它读经历就走与生产同一个入口（顺带让这些用例
/// 也覆盖真实读路径）。
fn trace_records(fixture: &Fixture) -> Vec<celira_agent::store::TraceRecord> {
    celira_agent::store::read_trace_at(&fixture.environment_root).expect("读经历")
}

/// 盘上每一条状态读数（`(run, 正文)`，按出现顺序）。
fn status_readings(fixture: &Fixture) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for record in trace_records(fixture) {
        let TracePayload::Injection { blocks, .. } = record.entry() else {
            continue;
        };
        let run = record.run().as_str().to_string();
        for (label, text) in blocks {
            if label != "status-bar" {
                continue;
            }
            if text.starts_with("time = ") {
                out.push((run.clone(), text.clone()));
            }
        }
    }
    out
}

/// 一个只够驱动一次的脚本（多余的调用会以"脚本用完"失败，因此"多跑一次"会被看得见）。
fn pad(count: usize) -> Vec<ModelOutput> {
    vec![ModelOutput::finish(SubjectEnding::Silent); count]
}

// ---------------------------------------------------------------------------
// 6. 通知栏：投影（计数、零正文）+ 快照时机 + 失败软着陆
// ---------------------------------------------------------------------------

/// 一份 B1 回执：两间有新的、**一间零新**，而且**每条都带预览**（用来证明投影一个字节正文都不带）。
fn rooms_body() -> String {
    serde_json::json!({
        "rooms": [
            {
                "room_id": "r-direct",
                "kind": "direct",
                "display_name": "值班室",
                "last_activity_at": "2026-09-16T14:41:55.455Z",
                "preview": "主人，你在吗？这条正文绝不该出现在通知栏里",
                "since_count": 2,
                "read_state": {"last_read_message_id": null, "unread_count": 0}
            },
            {
                "room_id": "r-group",
                "kind": "group",
                "display_name": "项目群",
                "last_activity_at": "2026-09-16T14:40:00.000Z",
                "preview": "群里也说一声：今晚对一下进度。",
                "since_count": 1,
                "read_state": {"last_read_message_id": null, "unread_count": 0}
            },
            {
                "room_id": "r-quiet",
                "kind": "group",
                "display_name": "清静间",
                "last_activity_at": "2026-09-16T10:00:00.000Z",
                "preview": "很久以前的一句话",
                "since_count": 0,
                "read_state": {"last_read_message_id": null, "unread_count": 0}
            }
        ],
        "request_id": "req-rooms"
    })
    .to_string()
}

/// 一块读数：`(run, turn, 正文)`。
type BlockReadings = Vec<(String, Option<u32>, String)>;

/// 盘上某一块读数（按出现顺序）。
///
/// 按**标签**取，因为"哪一块里有它"本身就是要判的东西：通知栏（世界的账）与持有物
/// （主体自己约下的时刻）是两块不同的东西——从前闹钟那一行寄生在通知栏的字符串里，
/// 于是 im 一断线、或第一个 Run 没有锚，闹钟就跟着一起哑；现在各走各的块。
fn block_readings(fixture: &Fixture, label: &str) -> BlockReadings {
    let mut out = Vec::new();
    for record in trace_records(fixture) {
        let TracePayload::Injection { turn, blocks, .. } = record.entry() else {
            continue;
        };
        let run = record.run().as_str().to_string();
        for (block_label, text) in blocks {
            if block_label != label {
                continue;
            }
            out.push((run.clone(), *turn, text.clone()));
        }
    }
    out
}

/// 盘上每一条通知栏读数。
fn notification_readings(fixture: &Fixture) -> Vec<(String, Option<u32>, String)> {
    block_readings(fixture, "notification-bar")
}

/// 盘上每一条**持有物**读数。
fn holdings_readings(fixture: &Fixture) -> Vec<(String, Option<u32>, String)> {
    block_readings(fixture, celira_agent::holdings::HOLDINGS_LABEL)
}

/// 投影说**分母 ＋ 有新的那几间**：两间有新的各一行、**余数一行 `其余 N−k 间 = 0`**、
/// **一行正文都没有**；而且这一次唤醒对读面**恰好一次请求**。
///
/// **契约变更（TASK_73，按事件定价）**：零新的房不再逐间点名（安静的一天不该为「无事」按房间数
/// 付税），缺席消歧改由**分母**承担——名字没出现在名单里的房就是零新。意图一条不丢。
#[test]
fn the_projection_states_the_denominator_and_lists_only_the_rooms_with_news() {
    let fixture = Fixture::new("projection");
    // **两场各读一次**（2026-10-10：第 1 场没有锚**也读**——未读那一半不需要锚；
    // 第 2 场带锚，读回来的是"自锚以来有几条新的"）。
    fixture.im_face.reply(200, &rooms_body());
    fixture.im_face.reply(200, &rooms_body());
    let access = fixture.access();
    let bell = fixture.rings.doorbell();
    let mut host = fixture.host(&access);
    let mut model = ScriptedModel::new(pad(4));

    fixture.rings.answer(200, "text/event-stream");
    let first = host.wake_and_run(&bell, &mut model).expect("第一次唤醒");
    // ⚠ **2026-10-10 改**：从前是"没有锚就不读"——而那一句让刚重启 / 第一次开机的那一段
    // **整块失明**。现在照读：增量那一半确实问不出来（不编一个 0），而**未读那一半按世界记着的
    // 读位置算，它不需要锚**（契约 §3.1：`since` 可选）。
    assert!(first.notification, "没有锚**也读**——未读那一半不需要锚");

    fixture.rings.ring_frame();
    let second = host.wake_and_run(&bell, &mut model).expect("第二次唤醒");
    assert!(second.notification, "有锚就该读到投影");

    let readings: Vec<String> = notification_readings(&fixture)
        .into_iter()
        .filter(|(run, _, _)| run == second.run.as_str())
        .map(|(_, _, text)| text)
        .collect();
    assert_eq!(readings.len(), 1, "一次唤醒恰好一块通知栏读数");
    let text = &readings[0];
    assert!(
        text.contains("可见 3 间 · 有新 2 间"),
        "分母与有新的间数都要如实: {text}"
    );
    assert!(
        text.contains("值班室（r-direct） = 2") && text.contains("项目群（r-group） = 1"),
        "有新的房**一个不漏**、条数如实，而且**带上房号**（名字给人读、号码给手用——她要拿它去调 `im_read_history`）: {text}"
    );
    // **缺席消歧**：Agent 是成员、这一轮没有新的那间房**由余数承担**（分母在场 ⇒ 没出现的＝零新）。
    assert!(
        text.contains("其余 1 间 = 0"),
        "零新由余数那句话承担（分母在场 ⇒ 「没出现」＝零新）: {text}"
    );
    assert!(
        !text.contains("清静间"),
        "零新的房不再点名（契约变更：按事件定价）: {text}"
    );
    // 顺序确定：条数降序、同数按房名升序 ⇒ 名单之后紧跟余数那一行（逐字节可复现）。
    assert!(
        text.contains("其余 1 间 = 0"),
        "余数行在（条数降序、名单之后）: {text}"
    );
    // **持有物那一刀起，通知栏只说自己**：im 那几行之后**不再**跟着主体时间那一行。
    // 闹钟是"手机的"（主体自己约的时刻），世界投影是"世界的"（im 那几行）——从前两者被拼成
    // 同一个字符串，于是 im 一断线、或第一个 Run 没有锚，闹钟就跟着一起哑。现在各走各的块
    // （持有物自己那一块见 `holdings_readings`，判据在 `tests/holdings.rs`）。
    assert!(
        !text.lines().any(|line| line.starts_with("timer: ")),
        "通知栏里不许再捎带闹钟那一行: {text}"
    );
    assert!(
        !text.contains("主人，你在吗") && !text.contains("预览") && !text.contains("进度"),
        "投影里**一个字节正文都没有**: {text}"
    );

    // 读面：**两次唤醒各读一次**（2026-10-10：第一次没有锚**也读**——未读那一半不需要锚），
    // 走的都是**同一端点**；只有带锚的那一次带 `since=`。
    assert_eq!(
        fixture.im_face.connections(),
        2,
        "两次唤醒各读一次（第一次没有锚，读的是未读那一半）"
    );
    let requests = fixture.im_face.requests();
    assert_eq!(requests.len(), 2, "一次唤醒对读面恰好一次请求");
    assert!(
        !requests[0].contains("since="),
        "第一次没有锚 ⇒ 不带那个参数: {}",
        requests[0]
    );
    assert!(
        requests[1].starts_with("GET /v1/backend/rooms?since="),
        "带锚的那一次走主体的同一条只读路径、带上锚: {}",
        requests[1]
    );

    // **同一份事实的两处渲染**：通知栏那句 `since` 与同一次活动状态行里的 `since` 是同一个时刻
    // （同一个声明、同一个格式化点）——改名之后这里就是它的守卫。
    let status = readings_of(&fixture, &second.run);
    assert!(!status.is_empty(), "同一次活动要有状态读数");
    let since_line = text.lines().next().expect("第一行是 since").to_string();
    assert!(
        status.iter().all(|line| line.ends_with(&since_line)),
        "通知栏的 `{since_line}` 与状态行的起点必须是同一个时刻: {status:?}"
    );
}

/// 投影读不出来 ⇒ 通知栏如实 `unknown`，而**这次活动照常收场**（便利坏了不挡清醒）。
#[test]
fn a_failed_projection_is_unknown_and_the_run_still_settles() {
    let fixture = Fixture::new("projection-failed");
    fixture.im_face.reply(500, "{\"error\":\"synthetic\"}");
    let access = fixture.access();
    let bell = fixture.rings.doorbell();
    let mut host = fixture.host(&access);
    let mut model = ScriptedModel::new(pad(4));

    fixture.rings.answer(200, "text/event-stream");
    let _ = host.wake_and_run(&bell, &mut model).expect("第一次唤醒");
    fixture.rings.ring_frame();
    let second = host.wake_and_run(&bell, &mut model).expect("第二次唤醒");

    assert!(!second.notification, "读失败 ⇒ 没有投影");
    assert_eq!(
        second.outcome,
        celira_agent::foundation::Outcome::Subject(SubjectEnding::Silent),
        "通知栏读不到**不**影响活动的收场"
    );
    let readings: Vec<String> = notification_readings(&fixture)
        .into_iter()
        .filter(|(run, _, _)| run == second.run.as_str())
        .map(|(_, _, text)| text)
        .collect();
    assert!(
        readings[0].ends_with("counts = unknown"),
        "如实说这一次没读到（不冒充「没有新的」）: {}",
        readings[0]
    );
    assert!(
        readings[0].starts_with("since = ") && !readings[0].starts_with("since = unknown"),
        "起点照旧说出来（锚是知道的，形状是十进制毫秒）: {}",
        readings[0]
    );
}

/// **交回控制 ⇒ 这一段仍然开着；收场由她自己说**（2026-10-10 在场那一刀）。
///
/// 两件事一起钉，因为它们容易被混成一件：
///
/// - **不催**（更早那一刀）：她交回控制之后**不再被追问**——从前会拿空输入问到轮次预算用尽，
///   账上写 `system:timeout`（"预算用尽"），而真实发生的是**她自己停了下来**。那是拿假话记她的账；
/// - **不封**（这一刀）：`Unsettled` 从前在这里被 `session.hold()` **封掉**，于是
///   **每一声铃都开一段新活动**、上下文每段从零起。那不是她的边界，是**门铃的边界**——
///   铃在传输层上只说"有新动静"，它不该顺手切断她的在场。
///
/// 判据：第二声铃驱动的是**同一段**（run-start 只登记一次、模型被问第二次），
/// 而收场是**她自己说的**（`subject:ended`），不是宿主替她说的 `subject:held`。
#[test]
fn handing_control_back_keeps_the_segment_open_until_the_subject_finishes_it() {
    let fixture = Fixture::new("hold-on-yield");
    let access = fixture.access();
    let mut host = fixture.host(&access);
    // 第一轮纯正文（交回控制）⇒ 这一段**没有结束**；第二声铃把她叫回来，这一次她收场。
    let mut model = ScriptedModel::new(vec![
        ModelOutput::say("先这样"),
        ModelOutput::finish(SubjectEnding::Ended),
    ]);

    let presence = host
        .wake_and_run(
            &ScriptedWake::of(vec![Woken::Changed, Woken::Changed]),
            &mut model,
        )
        .expect("两声音铃跑成一段");

    assert_eq!(
        model.calls(),
        2,
        "交回控制之后**不再被追问**（从前会问到轮次预算用尽）"
    );
    assert_eq!(
        presence.drives, 2,
        "这一段被驱动两次：开段那一声铃 ＋ 后来那一声铃"
    );
    assert_eq!(
        presence.outcome,
        Outcome::Subject(SubjectEnding::Ended),
        "收场是**她自己说的**，不是宿主替她说的「搁下」"
    );
    // 账上：这一段**恰好一条** run-start（第二声铃驱动的是同一段，不是新开一段），
    // 而且**没有**那个替她写下的终态（宿主不再替她封这一段）。
    let run = presence.run.as_str().to_string();
    let records = trace_records(&fixture);
    let starts = records
        .iter()
        .filter(|record| {
            record.run().as_str() == run
                && matches!(record.entry(), TracePayload::RunStarted { .. })
        })
        .count();
    assert_eq!(
        starts, 1,
        "两声音铃驱动的是**同一段**：run-start 只登记一次"
    );
    let endings = records
        .iter()
        .filter(|record| {
            record.run().as_str() == run && matches!(record.entry(), TracePayload::Ending { .. })
        })
        .count();
    assert_eq!(endings, 1, "这一段**恰好一个**终态");
}

/// **快照不是活状态**：一块读数出现在**开场**那一轮（`turn = None`），此后每一轮都没有。
///
/// ⚠ **2026-10-10 改（在场那一刀）**：这次活动的两轮**不靠"交回控制"**——交回控制现在是
/// "仍在场，等下一声铃"，不会在同一次 `wake_and_run` 里再问她。两轮靠**她自己做一件事**：
/// 第一轮读一次文件（行动 ⇒ 观察 ⇒ 下一轮），第二轮收场。
/// ⇒ 判据一字未变（读数只在开场那一轮），变的只是"怎么让这一段有两轮"。
#[test]
fn the_snapshot_lands_at_the_opening_and_never_repeats_per_turn() {
    let fixture = Fixture::new("snapshot");
    // **两场各读一次**（2026-10-10：没有锚的那一场也读）。
    fixture.im_face.reply(200, &rooms_body());
    fixture.im_face.reply(200, &rooms_body());
    let access = fixture.access();
    let bell = fixture.rings.doorbell();
    let mut host = fixture.host(&access);
    // 第一次唤醒先跑一次（拿锚）；第二次**先做一件事、再收场** ⇒ 这次活动有两轮。
    let mut model = ScriptedModel::new(vec![
        ModelOutput::finish(SubjectEnding::Silent),
        ModelOutput::new(
            String::new(),
            None,
            Decision::Act(vec![Action::new(ActionId::new("files_list"))]),
        ),
        ModelOutput::finish(SubjectEnding::Silent),
    ]);

    fixture.rings.answer(200, "text/event-stream");
    let _ = host.wake_and_run(&bell, &mut model).expect("第一次唤醒");
    fixture.rings.ring_frame();
    let second = host.wake_and_run(&bell, &mut model).expect("第二次唤醒");
    assert_eq!(second.drives, 1, "脚本第二轮就收场 ⇒ 这一段只被驱动一次");
    assert_eq!(
        model.calls(),
        3,
        "第二次活动确实有两轮（读一次文件 ＋ 收场）"
    );

    let readings: Vec<(Option<u32>, String)> = notification_readings(&fixture)
        .into_iter()
        .filter(|(run, _, _)| run == second.run.as_str())
        .map(|(_, turn, text)| (turn, text))
        .collect();
    assert_eq!(readings.len(), 1, "**Runs 内只有一块**通知栏读数");
    assert_eq!(readings[0].0, None, "它在**开场**那一拍（turn = None）");
    assert!(
        readings[0].1.contains("可见 3 间 · 有新 2 间") && readings[0].1.contains("其余 1 间 = 0"),
        "内容照新契约（分母 ＋ 有新的 ＋ 余数）: {}",
        readings[0].1
    );
}

// ---------------------------------------------------------------------------
// TASK_75：本地钟源——到点响铃 / 离线补响 / 投影那一行 / 起源如实
// ---------------------------------------------------------------------------

/// 按剧本交信号的唤醒源：**用例自己当那个钟**——不起门铃、不起线程，每一步都可判。
///
/// 真实的合并通道（`WakeHub`：门铃 ＋ 钟源）在 `wake.rs` 里；这里换掉它只为把"哪一轮被谁叫醒"
/// 变成确定的事（门铃那条线自己的形状由本文件上面那些用例钉着）。
struct ScriptedWake {
    steps: std::sync::Mutex<std::collections::VecDeque<celira_agent::wake::Woken>>,
}

impl ScriptedWake {
    fn of(steps: Vec<celira_agent::wake::Woken>) -> Self {
        Self {
            steps: std::sync::Mutex::new(steps.into()),
        }
    }
}

impl celira_agent::wake::WakePort for ScriptedWake {
    fn next(&self) -> celira_agent::wake::Woken {
        self.steps.lock().expect("短临界区").pop_front().unwrap_or(
            celira_agent::wake::Woken::Refused(
                celira_agent::doorbell::DoorbellFailure::ChannelLost,
            ),
        )
    }
}

/// 每一次活动开场那块**起源事实**（按出现顺序给 `(run, 正文)`）。
fn activity_origins(fixture: &Fixture) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for record in trace_records(fixture) {
        let TracePayload::Injection { blocks, .. } = record.entry() else {
            continue;
        };
        let run = record.run().as_str().to_string();
        for (label, text) in blocks {
            if label != "activity-origin" {
                continue;
            }
            out.push((run.clone(), text.clone()));
        }
    }
    out
}

/// 每一条 `run-start` 经历登记里的**结构化唤醒缘由**（`None` = 那一行没有这个字段）。
///
/// 它与 [`activity_origins`] 读的是**两处不同的东西**：这条读活动开始那一行自己的字段
/// （「谁叫醒的」结构化入账），那条读同一次开场注入里渲染出来的 `activity-origin` 块。
/// 两者都该有，且该说同一件事——本文件下面两条判据各管一边。
fn run_start_origins(fixture: &Fixture) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    for record in trace_records(fixture) {
        let TracePayload::RunStarted { origin } = record.entry() else {
            continue;
        };
        out.push((
            record.run().as_str().to_string(),
            origin.map(|origin| origin.as_str().to_string()),
        ));
    }
    out
}

/// 往主体那张表里放一条（用例直接写文件：磁盘契约就是那四个键）。
fn put_timer(fixture: &Fixture, id: &str, when_millis: u64, label: &str, state: &str) {
    let path = fixture.environment_root.join(TIMER_FILE);
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!(
        "{{\"id\":\"{id}\",\"when\":{when_millis},\"label\":\"{label}\",\"state\":\"{state}\"}}\n"
    ));
    std::fs::write(&path, text).expect("写表");
}

/// 表里的那一条现在是什么状态（`None` = 表里没有它）。
fn timer_state(fixture: &Fixture, id: &str) -> Option<String> {
    let text = std::fs::read_to_string(fixture.environment_root.join(TIMER_FILE)).ok()?;
    for line in text.lines() {
        let row: serde_json::Value = serde_json::from_str(line).ok()?;
        if row.get("id").and_then(|value| value.as_str()) == Some(id) {
            return row
                .get("state")
                .and_then(|value| value.as_str())
                .map(str::to_string);
        }
    }
    None
}

/// **验收 2（到点响铃）**：钟走到 `when` ⇒ 那一场**被钟源叫醒**、起源行如实说 timer、
/// 表里那条转 `fired`，而且 **`when` 保留原定值**。
#[test]
fn a_due_timer_wakes_the_host_and_the_origin_says_timer() {
    let fixture = Fixture::new("timer-due");
    fixture.im_face.reply(200, &rooms_body());
    let access = fixture.access();
    // 固定钟落在 1_800_000_000 秒；这一条约在**同一刻** ⇒ 开局就是"到点了"。
    put_timer(&fixture, "t-1", 1_800_000_000_000, "看便签", "pending");
    let mut host = fixture.host(&access);
    let mut model = ScriptedModel::new(pad(4));

    let wake = host
        .wake_and_run(
            &ScriptedWake::of(vec![celira_agent::wake::Woken::Timer]),
            &mut model,
        )
        .expect("钟源那一拍要跑起来");

    let origins = activity_origins(&fixture)
        .into_iter()
        .filter(|(run, _)| run == wake.run.as_str())
        .collect::<Vec<_>>();
    assert_eq!(origins.len(), 1, "开场恰好一块起源事实");
    assert_eq!(
        origins[0].1,
        celira_agent::activity_origin::TIMER_ORIGIN_TEXT,
        "起源如实说**是钟源**叫醒的（不是门铃那句）"
    );
    assert_eq!(
        timer_state(&fixture, "t-1").as_deref(),
        Some("fired"),
        "到点了就转 fired"
    );
    let text = std::fs::read_to_string(fixture.environment_root.join(TIMER_FILE)).expect("读表");
    assert!(
        text.contains("1800000000000"),
        "`when` 保留**原定时刻**（迟到不改成「现在」）: {text}"
    );
}

/// **刀一判据 A（唤醒通道）**：通道叫醒的那一场，`run-start` 行**自己**带 `origin` 字段，
/// 内容是**词表值**。它验的是"为什么醒"已**结构化入账**，不必去解析整批注入块。
///
/// ⚠ **2026-10-13**：通道那一档分成两态 ⇒ 取值是 `wake-changed`（这一场喂的是"报了一次变化"）；
/// 旧账那个 `bell` **照读不写**（认得出来，而生产线再也写不出它）。
#[test]
fn the_run_start_line_records_the_wake_origin_as_a_structured_field() {
    let fixture = Fixture::new("origin-bell");
    fixture.im_face.reply(200, &rooms_body());
    let access = fixture.access();
    let mut host = fixture.host(&access);
    let mut model = ScriptedModel::new(pad(4));

    let wake = host
        .wake_and_run(
            &ScriptedWake::of(vec![celira_agent::wake::Woken::Changed]),
            &mut model,
        )
        .expect("门铃那一拍要跑起来");

    let starts = run_start_origins(&fixture)
        .into_iter()
        .filter(|(run, _)| run == wake.run.as_str())
        .collect::<Vec<_>>();
    assert_eq!(starts.len(), 1, "这一次活动恰好一条 run-start 登记");
    assert_eq!(
        starts[0].1.as_deref(),
        Some("wake-changed"),
        "run-start 行必须**结构化**记下「谁叫醒的」——取值是**词表值**，不是一句话"
    );
    assert_ne!(
        starts[0].1.as_deref(),
        Some("timer"),
        "门铃这一档与钟源那一档必须分得开"
    );
}

/// **刀一判据 B（钟源）**：钟源叫醒的那一场，`run-start` 行记的是**钟源那句**——
/// 与门铃那句**不同**。这条同时是 A 的负对照：如果实现写死了门铃那句，这里会红。
#[test]
fn the_run_start_line_records_the_timer_origin_distinctly_from_the_bell() {
    let fixture = Fixture::new("origin-timer");
    fixture.im_face.reply(200, &rooms_body());
    let access = fixture.access();
    // 钟落在 1_800_000_000 秒；这一条约在同一刻 ⇒ 开局就是"到点了"。
    put_timer(&fixture, "t-1", 1_800_000_000_000, "看便签", "pending");
    let mut host = fixture.host(&access);
    let mut model = ScriptedModel::new(pad(4));

    let wake = host
        .wake_and_run(
            &ScriptedWake::of(vec![celira_agent::wake::Woken::Timer]),
            &mut model,
        )
        .expect("钟源那一拍要跑起来");

    let starts = run_start_origins(&fixture)
        .into_iter()
        .filter(|(run, _)| run == wake.run.as_str())
        .collect::<Vec<_>>();
    assert_eq!(starts.len(), 1, "这一次活动恰好一条 run-start 登记");
    assert_eq!(
        starts[0].1.as_deref(),
        Some("timer"),
        "run-start 行记的是**钟源**那一档（不是门铃那一档）"
    );
    assert_ne!(
        starts[0].1.as_deref(),
        Some("bell"),
        "钟源与门铃必须不同——否则两个来源分不开"
    );
}

/// **刀一判据 C（无起源 ⇒ 不写字段）**：没有声明起源的登记（旧形状／离线夹具）在盘上
/// **逐字节不含** `origin` 字段——这是"append-only、旧记录读作 None、不编造"那一侧。
#[test]
fn a_run_started_without_a_declared_origin_writes_no_origin_field() {
    let fixture = Fixture::new("origin-absent");
    {
        let mut env =
            Environment::open(&fixture.environment_root, fixture.agent()).expect("开环境");
        env.append_trace(
            &RunId::new("run-plain"),
            TracePayload::RunStarted { origin: None },
        )
        .expect("追加登记");
        env.append_trace(
            &RunId::new("run-named"),
            TracePayload::RunStarted {
                origin: Some(celira_agent::store::WakeOrigin::Timer),
            },
        )
        .expect("追加登记");
    }

    let starts = run_start_origins(&fixture);
    let plain = starts
        .iter()
        .find(|(run, _)| run == "run-plain")
        .expect("有 run-plain 那条");
    assert_eq!(plain.1, None, "没有起源 ⇒ 行里没有 origin 字段（旧形状）");

    // 正对照：声明了起源的那一条**真的**写进去了（否则"没写字段"可能只是解码没读）。
    let named = starts
        .iter()
        .find(|(run, _)| run == "run-named")
        .expect("有 run-named 那条");
    assert_eq!(
        named.1.as_deref(),
        Some("timer"),
        "声明了起源 ⇒ 行里写的是**词表值**（不是那句话）"
    );

    // 盘上原文核对：无字段那一行**逐字节**不含 `"origin"`。
    let raw = fixture.trace();
    let plain_line = raw
        .lines()
        .find(|line| line.contains(r#""run":"run-plain""#))
        .expect("原文件里有 run-plain 那行");
    assert!(
        !plain_line.contains("origin"),
        "无起源那一行不得出现 origin 字样: {plain_line}"
    );
}

/// **验收 3（离线补响）＋ 持有物出场**：进程重启后发现过期 pending ⇒ 补响＋转 fired；
/// 而**原定时刻 ＋ 标签**在**第一个 Run**（没有锚、没有投影、没有细胞）就摆出来了。
///
/// ⚠ **这一条断言的是持有物那一刀改掉的行为**（不是让实现去迁就旧断言）。从前那一行是拼进
/// im 投影字符串里的，于是第一个 Run 没有锚 ⇒ 投影是 `None` ⇒ 那一行**连坐消失**；而"报过没有"
/// 又住在宿主进程的 `owed` 里，崩一次就**永远不报**。现在开场摆的是**整张表**：
/// "报过"不再是一个状态，因此每一场都看得见，也就没有可丢的东西。
///
/// 它护住的仍是原来那条不变量——**不漏报**；放掉的只有"不重复报"，因为那一格正是病根。
#[test]
fn an_overdue_timer_catches_up_and_shows_its_original_time_in_the_very_first_waking() {
    let fixture = Fixture::new("timer-catch-up");
    // **三场各读一次投影**（2026-10-10：没有锚的那一场**也读**——未读那一半不需要锚）。
    fixture.im_face.reply(200, &rooms_body());
    fixture.im_face.reply(200, &rooms_body());
    fixture.im_face.reply(200, &rooms_body());
    let access = fixture.access();
    // 过期五分钟（离线错过）：现在 = 1_800_000_000_000，原定 = 1_799_999_700_000。
    put_timer(
        &fixture,
        "t-7",
        1_799_999_700_000,
        "明早八点检查",
        "pending",
    );
    let mut host = fixture.host(&access);
    let mut model = ScriptedModel::new(pad(8));

    /// 某一场里某一块的正文（多块时拼起来，便于逐条断言）。
    fn block_of(fixture: &Fixture, run: &RunId, readings: fn(&Fixture) -> BlockReadings) -> String {
        readings(fixture)
            .into_iter()
            .filter(|(each, _, _)| each == run.as_str())
            .map(|(_, _, text)| text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    // 第一场：钟源把它收掉，而持有物**这一场就摆出来了**——不需要锚、不需要投影。
    let first = host
        .wake_and_run(
            &ScriptedWake::of(vec![celira_agent::wake::Woken::Timer]),
            &mut model,
        )
        .expect("补响那一场");
    assert_eq!(timer_state(&fixture, "t-7").as_deref(), Some("fired"));
    let first_holdings = block_of(&fixture, &first.run, holdings_readings);
    assert!(first_holdings.contains("t-7"), "{first_holdings}");
    assert!(first_holdings.contains("fired"), "{first_holdings}");
    assert!(
        first_holdings.contains("when=1799999700000"),
        "**原定时刻**如实显示（迟到是事实，不改成「现在」）: {first_holdings}"
    );
    assert!(
        first_holdings.contains("明早八点检查"),
        "写给未来自己的那一行要在: {first_holdings}"
    );
    // 而通知栏**不再捎带**它：那是世界的账，不是主体的持有物。
    let first_notification = block_of(&fixture, &first.run, notification_readings);
    assert!(
        !first_notification.contains("timer"),
        "通知栏只说世界那几行: {first_notification}"
    );

    // 第二场（门铃叫醒）：表是权威 ⇒ 照旧看得见——**不需要"再报一次"这件事存在**。
    let second = host
        .wake_and_run(
            &ScriptedWake::of(vec![celira_agent::wake::Woken::Changed]),
            &mut model,
        )
        .expect("第二次唤醒");
    let second_holdings = block_of(&fixture, &second.run, holdings_readings);
    assert!(
        second_holdings.contains("when=1799999700000"),
        "{second_holdings}"
    );
    assert!(
        second_holdings.contains("明早八点检查"),
        "{second_holdings}"
    );
    // im 那一块照旧是世界的账（这一场有锚 ⇒ 有投影），两块**各说各的**。
    let second_notification = block_of(&fixture, &second.run, notification_readings);
    assert!(
        second_notification.contains("可见"),
        "{second_notification}"
    );
    assert!(
        !second_notification.contains("timer"),
        "{second_notification}"
    );

    // 第三场：还是看得见——**没有"已报"状态，因此也没有可丢的东西**（这正是那一刀的目的）。
    let third = host
        .wake_and_run(
            &ScriptedWake::of(vec![celira_agent::wake::Woken::Changed]),
            &mut model,
        )
        .expect("第三次唤醒");
    let third_holdings = block_of(&fixture, &third.run, holdings_readings);
    assert!(
        third_holdings.contains("when=1799999700000"),
        "表是权威 ⇒ 崩在哪一侧都看得见: {third_holdings}"
    );
}

/// **同一个到点的时刻，两种处境、两种后果**（2026-10-10 在场那一刀）。
///
/// 这一条钉的是**刀① 新造出来的那条判据**，也是"她自己重开一段"那条**组合路**的承重点
/// ——她手上已有的两个动词就够：`timer_set(when=现在)` ＋ `finish_run`，**不需要第四个动词**。
///
/// | 那一刻她在不在场 | 后果 |
/// |---|---|
/// | **在场**（还没收场） | 驱动**同一段**——那正是"到点提醒我"该有的样子 |
/// | **不在场**（她收场了） | 开**新的一段**（干净的上下文）——那正是"重开" |
///
/// ⚠ 刀① 之前这两格是**同一格**（每一声唤醒都开新段），所以"重开"根本不需要组合；
/// 今天它需要，而它的前提是**"她收场了"**——那个前提是新的，因此值得单独一条。
#[test]
fn the_same_due_timer_drives_the_open_segment_and_opens_a_new_one_after_it_ends() {
    let fixture = Fixture::new("timer-two-ways");
    fixture.im_face.reply(200, &rooms_body());
    fixture.im_face.reply(200, &rooms_body());
    let access = fixture.access();
    let mut host = fixture.host(&access);
    // 三声钟：第一段里两声（她先说一句、再收场），第二段一声（新段开起来就收）。
    let mut model = ScriptedModel::new(vec![
        ModelOutput::say("先停一下"),
        ModelOutput::finish(SubjectEnding::Ended),
        ModelOutput::finish(SubjectEnding::Ended),
    ]);
    let wake = ScriptedWake::of(vec![Woken::Timer, Woken::Timer, Woken::Timer]);

    // 第一段：她**在场**时第二声钟到点 ⇒ 驱动的是**同一段**（同一个 run、两次驱动）。
    let first = host.wake_and_run(&wake, &mut model).expect("第一段");
    assert_eq!(
        first.drives, 2,
        "在场时那一声钟驱动同一段：这一段被驱动两次"
    );
    assert_eq!(
        first.outcome,
        Outcome::Subject(SubjectEnding::Ended),
        "收场是**她自己说的**"
    );

    // 第二段：她已经**不在场**了 ⇒ 同一声钟开的是**新的一段**。
    let second = host.wake_and_run(&wake, &mut model).expect("第二段");
    assert_ne!(second.run, first.run, "不在场时那一声钟开新段");
    assert_eq!(second.drives, 1, "新段开起来就收");
    assert_eq!(model.calls(), 3, "两段一共三次模型调用");

    // 两段各自恰好一条 run-start，而且起源**如实说 timer**（"她自己约的时刻"那一档）——
    // 组合路走完，账上留下的是一条**说得通**的轨迹，不是一句含糊的"她又醒了"。
    let starts = run_start_origins(&fixture);
    for run in [&first.run, &second.run] {
        let origins: Vec<Option<String>> = starts
            .iter()
            .filter(|(each, _)| each == run.as_str())
            .map(|(_, origin)| origin.clone())
            .collect();
        assert_eq!(
            origins,
            vec![Some("timer".to_string())],
            "{} 的起源必须如实说 timer",
            run.as_str()
        );
    }
}

/// **唤醒边界上的钩子：在场期间每一声铃调一次，开段那一次不调**（2026-10-10 在场那一刀）。
///
/// 它钉的是宿主交给调用方的那一格：**"铃已到、下一轮还没开始"**。宿主二进制在那里做它自己那一
/// 件事（重判模型来源，见 `agent_host`），而**它不决定这一段收不收场**——那是主体的判断。
///
/// ⚠ 判据两条一起：① **开段那一次不调**（调用方在开段之前刚判过，再调一次是重复）；
/// ② 在场期间**每一声铃恰好一次**（不多不少——多了就是替调用方做了它没要求的事）。
#[test]
fn the_wake_boundary_hook_runs_once_per_bell_inside_the_segment() {
    let fixture = Fixture::new("boundary-hook");
    fixture.im_face.reply(200, &rooms_body());
    let access = fixture.access();
    let mut host = fixture.host(&access);
    let mut model = ScriptedModel::new(vec![
        ModelOutput::say("先停一下"),
        ModelOutput::finish(SubjectEnding::Ended),
    ]);
    let mut calls = 0usize;
    let mut boundary = |_: &mut ScriptedModel| -> Result<(), HostFailure> {
        calls += 1;
        Ok(())
    };

    let reading = host
        .run_woken_with(
            Woken::Changed,
            &ScriptedWake::of(vec![Woken::Changed]),
            &mut model,
            &mut boundary,
        )
        .expect("一段在场");

    assert_eq!(reading.drives, 2, "两声音铃驱动同一段");
    assert_eq!(
        calls, 1,
        "钩子在**唤醒边界**上恰好一次：开段那一次不调，后来那一声铃调一次"
    );
}

/// **静也要说出口**：一条都没到点 ⇒ 持有物那一块如实写 `holdings = N (pending N / fired 0)`，
/// 而不是没有那一块（主体才知道是"空"，不是"没读到"）；而且没到点的那条**不许**被收掉。
///
/// ⚠ 从前这一条判的是通知栏里那句 `timer: 0`——持有物那一刀起，闹钟不再寄生在世界投影上。
#[test]
fn a_quiet_waking_states_her_pending_holdings_and_never_consumes_them() {
    let fixture = Fixture::new("timer-quiet");
    fixture.im_face.reply(200, &rooms_body());
    let access = fixture.access();
    put_timer(&fixture, "t-1", 1_900_000_000_000, "还没到", "pending");
    let mut host = fixture.host(&access);
    let mut model = ScriptedModel::new(pad(4));

    let _ = host
        .wake_and_run(
            &ScriptedWake::of(vec![celira_agent::wake::Woken::Timer]),
            &mut model,
        )
        .expect("第一场");
    let second = host
        .wake_and_run(
            &ScriptedWake::of(vec![celira_agent::wake::Woken::Changed]),
            &mut model,
        )
        .expect("第二场");
    let text: String = holdings_readings(&fixture)
        .into_iter()
        .filter(|(run, _, _)| run == second.run.as_str())
        .map(|(_, _, text)| text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("holdings = 1 (pending 1 / fired 0)"),
        "静也要说出口: {text}"
    );
    assert!(text.contains("t-1"), "{text}");
    assert!(text.contains("还没到"), "写给未来自己的那一行要在: {text}");
    assert_eq!(
        timer_state(&fixture, "t-1").as_deref(),
        Some("pending"),
        "没到点的那条**不许**被收掉"
    );
}

/// **合并窗口**（2026-10-19）：头一声铃之后要等满窗口才开段；默认（`ZERO`）不等。
///
/// 这一格是"默认收到群消息"能不能上线的前提：没有它，群里每一条消息都换来一次模型调用。
/// 两条断言成对——**关着时快、开着时慢**——只量一条的话，"慢"也可能是别的原因。
#[test]
fn a_ring_waits_for_the_coalesce_window_only_when_it_is_open() {
    // ⚠ 150ms → 600ms（2026-10-24 夜间巡检抓到）：这条判据要区分的是"等满窗口"与"不等窗口"，
    // 而 150ms 的对照在满载机器上会被调度抖动吃掉——实测"窗口关着"那一支跑了 70.68ms 就红了，
    // 它量到的是 CPU 排期，不是合并语义。对照窗口拉大之后，"误睡"仍然会被抓：真睡了 600ms
    // 就绝不可能落在下面的 300ms 以内。
    for (window, must_wait) in [
        (std::time::Duration::ZERO, false),
        (std::time::Duration::from_millis(600), true),
    ] {
        let fixture = Fixture::new(if must_wait {
            "coalesce-on"
        } else {
            "coalesce-off"
        });
        fixture.im_face.reply(200, &rooms_body());
        let access = fixture.access();
        let bell = fixture.rings.doorbell();
        let mut host = fixture.host(&access).with_coalesce(window);
        fixture.rings.answer(200, "text/event-stream");

        // ⚠ 用**生产那条合并通道**：`small` 的门铃包装自己不带窗口语义，
        // 合并发生在 `WakeHub` 上（这也正是生产装配的样子）。
        let hub = WakeHub::bell_only(bell);
        let started = std::time::Instant::now();
        let woken = host.next_wake(&hub).expect("一声铃");
        let waited = started.elapsed();

        assert!(
            matches!(woken, Woken::Opened | Woken::Changed),
            "开段那一档必须是铃: {woken:?}"
        );
        if must_wait {
            assert!(
                waited >= window,
                "窗口开着就必须等满（等的是 {window:?}，实际 {waited:?}）"
            );
        } else {
            assert!(
                waited < window.max(std::time::Duration::from_millis(300)),
                "窗口关着就不该等（实际 {waited:?}）"
            );
        }
    }
}
