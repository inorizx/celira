//! 持有物验收：**主体自己安排的事，在他醒来时必须自己出现**。
//!
//! 只有瞬间记忆的主体要用上自己留下的东西，前提是**知道它存在**——而"记得"这条按前提不成立，
//! 所以"被摆出来"是必需的。本文件判的正是这一条，而且**全部在生产装配路径上**
//! （`RunSession::open_declared` + 真实处境链），不是另一条执行路径。
//!
//! ## 它治的是什么（两个形状，同一处病）
//!
//! 从前的形状是：宿主先 `consume_due` 把到点的从 `pending` 改成 `fired`（**落盘、不可逆**），
//! 再把"还没报过"的那几条放进**进程内存**里的 `owed`，最后把那一行**拼进 im 投影的字符串**。
//! 于是：
//!
//! - **崩在收掉与开活动之间** ⇒ `fired` 已经落盘、`owed` 随进程消失 ⇒ **永远不报**；
//! - **没有锚 / im 读不到**（第一个 Run、宿主刚重启、断线） ⇒ 投影是 `None` ⇒ 那一行
//!   **连坐消失**——而闹钟是主体的本地机制，与 im 毫无关系。
//!
//! 现在：开场由**持有物那一块整体摆出那张表**，于是"报过没有"不再是一个状态，没有可丢的东西。
//! 崩在收掉之前 ⇒ 表里是 `pending` + `overdue`；崩在之后 ⇒ 表里是 `fired`。**两种都看得见。**
//!
//! ## 判据的边界
//!
//! 本文件只判**开场看得见什么**。`timer_set` / `timer_list` / `timer_cancel` 自己的行为在
//! `tests/timer_app.rs`；宿主那一侧（到点响铃、离线补响、起源行）在 `tests/host_doorbell.rs`。

use std::path::PathBuf;

use celira_agent::clock::ClockHandle;
use celira_agent::composition::{RunOrigin, RunSession, SessionDeclarations, SessionRequest};
use celira_agent::context_reading::ContextReading;
use celira_agent::core::{
    Decision, Limits, ModelError, ModelOutput, ModelPort, RunInput, TraceEntry, View,
};
use celira_agent::foundation::{AgentId, RunId};
use celira_agent::holdings::HOLDINGS_LABEL;
use celira_agent::store::{Environment, WakeOrigin};
use celira_agent::timer_app::TIMER_FILE;

const AGENT: &str = "agent-holdings";

/// 用例的固定钟：自 1970 起的第 1_700_000_000 秒（⇒ unix 毫秒 1_700_000_000_000）。
///
/// 固定钟不是"另一种装配"：生产与这里走同一个会话入口、同一个格式化点，只是时间来源不同，
/// 因此年龄那一格的读数可以逐字节断言。
const NOW_SECONDS: u64 = 1_700_000_000;
const NOW_MILLIS: u64 = NOW_SECONDS * 1_000;

/// 用例声明的上下文窗口（宿主声明，必须正整数）。
const CONTEXT_WINDOW: u32 = 1_000;

// ---------------------------------------------------------------------------
// 装置（自建目录；不指桌面、不指家、不指仓库根）
// ---------------------------------------------------------------------------

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "celira-holdings-{label}-{}-{n}",
            std::process::id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("清掉同名残留目录");
        }
        std::fs::create_dir_all(&path).expect("建立测试目录");
        Self { path }
    }

    fn env_root(&self) -> PathBuf {
        let root = self.path.join("env");
        std::fs::create_dir_all(&root).expect("环境根");
        root
    }

    fn materials(&self) -> PathBuf {
        let root = self.path.join("materials");
        std::fs::create_dir_all(&root).expect("资料根");
        root
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn agent_id() -> AgentId {
    AgentId::new(AGENT)
}

fn fixed_clock() -> ClockHandle {
    ClockHandle::fixed_at_unix_seconds(NOW_SECONDS)
}

/// 往主体的持有物表里手写一行（**约是主体的动作**，行为判据在 `tests/timer_app.rs`；
/// 这里只判**装配与注入**：表里的东西有没有在开场出现）。
///
/// `state` 直接写成 `"fired"` 就是"宿主已经收掉、然后崩了"留在盘上的那个形状——正是本文件
/// 要判的输入。
fn seed_timer(root: &std::path::Path, id: &str, when_millis: u64, label: &str, state: &str) {
    let path = root.join(TIMER_FILE);
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!(
        "{{\"id\":\"{id}\",\"when\":{when_millis},\"label\":\"{label}\",\"state\":\"{state}\"}}\n"
    ));
    std::fs::write(&path, text).expect("写持有物表");
}

/// 一次装配声明（与生产同一个会话入口）。
fn session_request<'a>(agent: &'a AgentId, run: &str) -> SessionRequest<'a> {
    SessionRequest {
        declarations: SessionDeclarations::default(),
        declared_agent: agent,
        run: RunId::new(run),
        limits: Limits::new(4),
        input: RunInput::empty(),
        clock: fixed_clock(),
        context_window: CONTEXT_WINDOW,
        reading: ContextReading::default(),
    }
}

/// **新进程那一次醒来**的处境声明：没有锚、没有投影、没有细胞。
///
/// 它就是宿主刚重启（或这一段经历不是本进程开的头）时给出的形状——也正是"崩在收掉与开活动
/// 之间"之后，下一次醒来的真实形状。
fn fresh_process_origin() -> RunOrigin<'static> {
    RunOrigin {
        origin: WakeOrigin::Timer,
        since: None,
        notification: None,
        cards: None,
    }
}

// ---------------------------------------------------------------------------
// 记录模型：把每一轮真正收到的注入块原样记下来
// ---------------------------------------------------------------------------

struct Recorder {
    /// 每次调用收到的（块标签，块正文）。
    seen: Vec<Vec<(String, String)>>,
}

impl Recorder {
    fn new() -> Self {
        Self { seen: Vec::new() }
    }

    /// 第一轮那一批里的某一块（按标签找）。
    fn block(&self, turn: usize, label: &str) -> Option<&str> {
        self.seen
            .get(turn)?
            .iter()
            .find(|(each, _)| each == label)
            .map(|(_, text)| text.as_str())
    }

    /// 这一轮看到的**全部历史注入**里，某个标签出现了几次。
    ///
    /// 判"快照只摆一次"必须用**计数**而不是"在不在"：视野是**全历史**，第二轮必然把开场那一批
    /// 一起带上（与 `tests/run_session.rs` 判组件区同一条判据）。
    fn count(&self, turn: usize, label: &str) -> usize {
        self.seen
            .get(turn)
            .map(|blocks| blocks.iter().filter(|(each, _)| each == label).count())
            .unwrap_or(0)
    }
}

impl ModelPort for Recorder {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        let blocks = view
            .trace()
            .iter()
            .filter_map(|entry| match entry {
                TraceEntry::Injection { batch, .. } => Some(
                    batch
                        .blocks()
                        .iter()
                        .map(|block| (block.label().to_string(), block.text().to_string()))
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>();
        self.seen.push(blocks);
        // **只说正文、不行动** ⇒ 停下等待（活动没结束，因此还能显式继续到第二轮）。
        Ok(ModelOutput::new(
            "我看看".to_string(),
            None,
            Decision::Yield,
        ))
    }
}

// ---------------------------------------------------------------------------
// 判据
// ---------------------------------------------------------------------------

/// **崩在收掉与开活动之间**：那条闹钟已经标成 `fired` 落盘，而"已通知"随进程没了。
///
/// 判据：下一次醒来（新进程、没有锚、**没有投影**）开场仍然看得见它——身份、原定时刻、标签
/// 都在。**这正是从前永远不报的那一格。**
#[test]
fn an_alarm_that_fired_before_the_crash_still_shows_up_in_the_next_waking() {
    let temp = TempDir::new("fired-before-crash");
    let env_root = temp.env_root();
    // ⚠ **顺序即事实**（2026-10-06）：环境先存在，闹钟才写得进去——`create` 只在一个空位置上
    // 创建（身份不在却有别的东西 ⇒ 明确拒绝，见 README §3.3）。从前这条夹具先把闹钟写进去，
    // 演示的是一个产品里不存在的顺序。
    let mut environment = Environment::create(&env_root, agent_id()).expect("建立身份环境");
    seed_timer(&env_root, "t-1", NOW_MILLIS - 1_000_000, "看便签", "fired");
    let agent = agent_id();
    let request = session_request(&agent, "run-holdings-1");
    let mut session = RunSession::open_declared(
        &temp.materials(),
        &mut environment,
        fresh_process_origin(),
        request,
    )
    .expect("装配应当成功");

    let mut model = Recorder::new();
    session.drive(&mut model).expect("驱动本身不该失败");

    let block = model
        .block(0, HOLDINGS_LABEL)
        .unwrap_or_else(|| panic!("开场就该有持有物那一块：{:?}", model.seen));
    assert!(block.contains("t-1"), "身份要在里面：{block}");
    assert!(block.contains("fired"), "状态要在里面：{block}");
    assert!(
        block.contains("看便签"),
        "**写给未来自己的那一行**要在里面：{block}"
    );
    assert!(
        block.contains(&format!("when={}", NOW_MILLIS - 1_000_000)),
        "原定时刻要在里面（迟到是事实，不改写成「现在」）：{block}"
    );
    assert!(block.contains("fired-ago=1000s"), "年龄要在里面：{block}");
}

/// **另一种崩法**：到点了，但宿主还没跑到 `consume_due` 就死了。
///
/// 判据：表里那条照旧是 `pending`，而年龄那一格如实说 `overdue`——**"这一次没响成"看得见**，
/// 不会被折进任何别的说法，也不会被 `fired` 那些行挤掉。
#[test]
fn an_alarm_that_is_overdue_but_never_consumed_shows_up_as_overdue() {
    let temp = TempDir::new("overdue");
    let env_root = temp.env_root();
    // 环境先存在，闹钟才写得进去（同上）。
    let mut environment = Environment::create(&env_root, agent_id()).expect("建立身份环境");
    seed_timer(&env_root, "t-9", NOW_MILLIS - 60_000, "该响了", "pending");
    let agent = agent_id();
    let request = session_request(&agent, "run-holdings-2");
    let mut session = RunSession::open_declared(
        &temp.materials(),
        &mut environment,
        fresh_process_origin(),
        request,
    )
    .expect("装配应当成功");

    let mut model = Recorder::new();
    session.drive(&mut model).expect("驱动本身不该失败");

    let block = model
        .block(0, HOLDINGS_LABEL)
        .unwrap_or_else(|| panic!("开场就该有持有物那一块：{:?}", model.seen));
    assert!(block.contains("t-9"), "{block}");
    assert!(block.contains("pending"), "{block}");
    assert!(block.contains("overdue=60s"), "{block}");
}

/// **持有物不寄生于世界投影**：没有锚、没有 im（`notification = None`）时它照旧出场。
///
/// 这是从前那个连坐的负对照：同一份处境声明下，通知栏如实空着，而持有物必须在。
#[test]
fn the_holdings_block_does_not_depend_on_the_world_projection() {
    let temp = TempDir::new("no-projection");
    let env_root = temp.env_root();
    // 环境先存在，闹钟才写得进去（同上）。
    let mut environment = Environment::create(&env_root, agent_id()).expect("建立身份环境");
    seed_timer(
        &env_root,
        "t-5",
        NOW_MILLIS + 3_600_000,
        "一小时后",
        "pending",
    );
    let agent = agent_id();
    let request = session_request(&agent, "run-holdings-3");
    let mut session = RunSession::open_declared(
        &temp.materials(),
        &mut environment,
        fresh_process_origin(),
        request,
    )
    .expect("装配应当成功");

    let mut model = Recorder::new();
    session.drive(&mut model).expect("驱动本身不该失败");

    let labels: Vec<&str> = model.seen[0]
        .iter()
        .map(|(label, _)| label.as_str())
        .collect();
    assert!(
        labels.contains(&HOLDINGS_LABEL),
        "没有投影时持有物照样出场：{labels:?}"
    );
    let block = model.block(0, HOLDINGS_LABEL).expect("上面刚判过");
    assert!(block.contains("due-in=3600s"), "{block}");
}

/// 往**终端那一本账**里手写一行 `start`（**起是主体的动作**，行为判据在 `tests/terminal_held.rs`；
/// 这里只判**装配与注入**：账里的东西有没有在开场出现）。
///
/// `pid` 给一个**确定不存在的**大数：那一行的 `state` 因此是 `gone`（读数），用例不依赖
/// 这台机器上跑着什么——**判据是"装配有没有把它摆出来"，不是"它活着没有"**。
fn seed_held(root: &std::path::Path, name: &str, started_millis: u64) {
    let path = root.join("terminal");
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!(
        "{{\"kind\":\"start\",\"name\":\"{name}\",\"command\":\"run {name}\",\"pid\":4294900000,\"log\":\"logs/{name}.out\",\"at\":{started_millis}}}\n"
    ));
    std::fs::write(&path, text).expect("写终端那本账");
}

/// **第二支成员真的摆进开场了**（2026-10-11）：账里那一件"活到关家"的东西，在**新进程那一次
/// 醒来**的开场块里看得到——名字、三态读数、起的时刻与年龄、输出落在哪儿、当时那条命令。
///
/// 这一条判的是**装配**（`app → 显示区` 那条线通不通），不是渲染（渲染的判据在 `holdings.rs`
/// 自己的单测里）。
#[test]
fn a_thing_she_started_shows_up_in_the_opening_block() {
    let temp = TempDir::new("held-member");
    let env_root = temp.env_root();

    let mut environment = Environment::create(&env_root, agent_id()).expect("建立身份环境");
    seed_held(&env_root, "server", NOW_MILLIS - 5_000);
    let agent = agent_id();
    let request = session_request(&agent, "run-holdings-5");
    let mut session = RunSession::open_declared(
        &temp.materials(),
        &mut environment,
        fresh_process_origin(),
        request,
    )
    .expect("装配应当成功");

    let mut model = Recorder::new();
    session.drive(&mut model).expect("驱动本身不该失败");

    let block = model
        .block(0, HOLDINGS_LABEL)
        .unwrap_or_else(|| panic!("开场就该有持有物那一块：{:?}", model.seen));
    assert!(
        block.starts_with("holdings = 1 (pending 0 / fired 0 / held 1)"),
        "账单那一行要如实说这一件是从哪儿来的：{block}"
    );
    assert!(
        block.contains("server | gone | started="),
        "名字、三态读数、起的时刻都要摆出来：{block}"
    );
    assert!(
        block.contains("started-ago=5s"),
        "年龄那一格走装配那一轮的钟（这一支自己的词）：{block}"
    );
    assert!(
        block.contains("output=logs/server.out"),
        "输出落在哪儿要摆出来（主体读得到那一份）：{block}"
    );
    assert!(
        block.contains("run server"),
        "当时那条命令要摆出来：{block}"
    );
}

/// **表在、一条也没有 ⇒ 把零说出口**（主体才知道是"空"，不是"没读到"）。
#[test]
fn a_waking_with_nothing_arranged_says_zero_out_loud() {
    let temp = TempDir::new("empty");
    let env_root = temp.env_root();

    let mut environment = Environment::create(&env_root, agent_id()).expect("建立身份环境");
    let agent = agent_id();
    let request = session_request(&agent, "run-holdings-4");
    let mut session = RunSession::open_declared(
        &temp.materials(),
        &mut environment,
        fresh_process_origin(),
        request,
    )
    .expect("装配应当成功");

    let mut model = Recorder::new();
    session.drive(&mut model).expect("驱动本身不该失败");

    let block = model
        .block(0, HOLDINGS_LABEL)
        .unwrap_or_else(|| panic!("开场就该有持有物那一块：{:?}", model.seen));
    assert!(block.starts_with("holdings = 0"), "{block}");
    assert!(block.contains("没有约下的时刻"), "{block}");
}

/// **开场快照，不是每轮重摆**：第二轮不再新增这一块（与纸条板同律）。
#[test]
fn the_block_is_an_opening_snapshot_and_is_not_relaid_every_turn() {
    let temp = TempDir::new("snapshot-once");
    let env_root = temp.env_root();
    // 环境先存在，闹钟才写得进去（同上）。
    let mut environment = Environment::create(&env_root, agent_id()).expect("建立身份环境");
    seed_timer(&env_root, "t-2", NOW_MILLIS + 1_000, "马上", "pending");
    let agent = agent_id();
    let request = session_request(&agent, "run-holdings-5");
    let mut session = RunSession::open_declared(
        &temp.materials(),
        &mut environment,
        fresh_process_origin(),
        request,
    )
    .expect("装配应当成功");

    let mut model = Recorder::new();
    session.drive(&mut model).expect("驱动本身不该失败");
    assert_eq!(model.seen.len(), 1, "一轮驱动只调一次模型");

    // 第二轮（显式继续）：把这一段的新输入排进去再驱动一次。
    session
        .provide(RunInput::text("接着看").expect("在上限内"))
        .expect("排得进去");
    session.drive(&mut model).expect("第二次驱动");

    assert!(
        model.seen.len() >= 2,
        "应当驱动了两轮：{}",
        model.seen.len()
    );
    // **判据是计数不增长**（视野是全历史，第二轮必然带上开场那一批——见 `Recorder::count`）。
    let openings = model.count(0, HOLDINGS_LABEL);
    assert_eq!(openings, 1, "开场恰好摆一次");
    assert_eq!(
        model.count(1, HOLDINGS_LABEL),
        openings,
        "持有物是开场快照，第二轮不许新增一块：{:?}",
        model.seen[1]
    );
}
