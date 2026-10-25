//! TASK_08 阶段 B 验收：**同进程会话、纯正文等待与显式宿主输入循环**。
//!
//! 这一份文件跑的是生产会话接口（`RunSession` + `manual_entry` 的宿主循环），不是另一条
//! 执行路径：装配一次、驱动多次；等待不结束活动；新输入恰好呈现一次；预算不重置。
//!
//! 全部离线、全合成数据、只用自建的独立测试目录（不扫任何个人数据、不读任何密钥）。
//! 回环服务只证明设备协议路径，**不**证明公网模型行为。
//!
//! 其中带真实回环的三条（等待、换实例重建、拒绝回执）是**同进程**的：它们用生产适配层指向
//! 本机回环端点，因此请求体、消息顺序与配对关系都是可断言的真实字节。CLI 夹具那一层的回环
//! 证据在 `provider_wire.rs`、`manual_entry.rs`、`authorized_creation.rs`。

use std::io::{BufRead as _, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::mpsc;

use celira_agent::clock::ClockHandle;
use celira_agent::composition::{
    CompositionError, RunOrigin, RunSession, SessionDeclarations, SessionDrive, SessionRequest,
};
use celira_agent::context_reading::ContextReading;
use celira_agent::core::{
    Action, ActionId, ActionParams, InjectionStage, Limits, ModelError, ModelFailureClass,
    ModelOutput, ModelPort, RunInput, TraceEntry, View,
};
use celira_agent::event_cards::EVENT_CARDS_LABEL;
use celira_agent::foundation::{
    AgentId, Outcome, ResultState, RunId, SubjectEnding, SystemEnding, Turn,
};
use celira_agent::manual_entry::{
    EXIT_UNSETTLED, ManualEnding, ManualInput, ManualRun, run_host_session_in,
};
use celira_agent::provider_adapter::DeepSeekModel;
use celira_agent::skills_desktop::SkillRef;
use celira_agent::store::{Admission, Environment};

mod common;

use common::InspectsFirst;

/// 哨兵凭据：只用于回环认证，任何输出或存储里出现它就说明越界了。
const SENTINEL_KEY: &str = "fixture-sentinel-credential";

/// 用例装配用的**时间来源**：固定钟——离线、可逐字节断言。
///
/// 生产装配给真实钟（`agent_run`），用例一律给固定钟：状态行的 `time` 因此在每次运行里都是同一个
/// 读数，任何"同输入同输出"的断言都不会被墙上时钟搅动。它只换时间来源，**不换**装配代码——
/// 这里与生产走的是同一个会话入口、同一个格式化点。
fn fixed_clock() -> ClockHandle {
    ClockHandle::fixed_at_unix_seconds(0)
}

/// 用例声明的**上下文窗口**（宿主声明，必须正整数）：取一个小而固定的数字，便于逐字节断言。
///
/// 生产入口缺省用适配层的公开标称值（1M），用例一律显式声明——窗口是声明不是测量，用例自己
/// 声明一个"屏幕有多小"才能把 `context` 那一段钉住。
const CONTEXT_WINDOW: u32 = 1_000;

/// 用例装配用的**屏幕报告 cell 的读侧**：默认是**空 cell** ⇒ 状态行读作 `context = unknown/… token`。
///
/// 要模拟"适配层报过一次"的形态，用 `ContextReading::channel()` 拿写柄自己写（写柄在这一侧只是
/// 用例的替身；生产里它只交给适配层——这条纪律由源码守卫
/// `only_the_adapter_writes_the_reading_cell` 机械检查）。
fn reading_cell() -> ContextReading {
    ContextReading::default()
}

// ---------------------------------------------------------------------------
// 自建的独立测试目录
// ---------------------------------------------------------------------------

/// 一个自建的独立测试目录。绝不指向桌面 / 家目录 / 仓库根。
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
            "celira-task08-session-{label}-{}-{n}",
            std::process::id()
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("清掉同名残留目录");
        }
        std::fs::create_dir_all(&path).expect("建立测试目录");
        Self { path }
    }

    fn materials(&self) -> PathBuf {
        let materials = self.path.join("materials");
        std::fs::create_dir_all(&materials).expect("建资料目录");
        std::fs::write(materials.join("alpha.txt"), "alpha").expect("写合成文件");
        materials
    }

    fn env_root(&self) -> PathBuf {
        self.path.join("env")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

const AGENT: &str = "agent-session";

fn agent_id() -> AgentId {
    AgentId::new(AGENT)
}

fn environment(dir: &TempDir) -> Environment {
    Environment::create(&dir.env_root(), agent_id()).expect("建立身份环境")
}

/// 手动运行参数（宿主循环那一侧用的就是这一份）。
fn manual_run(dir: &TempDir, run: &str, input: ManualInput) -> ManualRun {
    ManualRun {
        declarations: SessionDeclarations::default(),
        environment_root: dir.env_root(),
        expected_agent: agent_id(),
        material_root: dir.materials(),
        run: RunId::new(run),
        max_turns: 6,
        input,
        clock: fixed_clock(),
        context_window: CONTEXT_WINDOW,
        reading: reading_cell(),
    }
}

fn session_request<'a>(
    agent: &'a AgentId,
    run: &str,
    input: &str,
    limits: Limits,
) -> SessionRequest<'a> {
    SessionRequest {
        declarations: SessionDeclarations::default(),
        declared_agent: agent,
        run: RunId::new(run),
        limits,
        input: RunInput::text(input).expect("在上限内"),
        clock: fixed_clock(),
        context_window: CONTEXT_WINDOW,
        reading: reading_cell(),
    }
}

// ---------------------------------------------------------------------------
// 脚本模型：按序返回中立响应，并记下每次调用看到的轮次、初始输入与注入
// ---------------------------------------------------------------------------

struct ScriptedModel {
    steps: std::collections::VecDeque<ModelOutput>,
    /// 每次调用看到的（轮次，初始输入，轨迹里出现的注入正文）。
    seen: Vec<(Turn, String, Vec<String>)>,
}

impl ScriptedModel {
    fn new(steps: Vec<ModelOutput>) -> Self {
        Self {
            steps: steps.into(),
            seen: Vec::new(),
        }
    }

    /// 主体这一程一共调了几次模型。
    ///
    /// ⚠️ `#[allow(dead_code)]`：这是**测试侧的辅助读数**，不是每个用例都用到——
    /// 不用它的那些文件里它照样该在（想数的时候就在手边）。
    #[allow(dead_code)]
    fn calls(&self) -> usize {
        self.seen.len()
    }
}

impl ModelPort for ScriptedModel {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        let injections = view
            .trace()
            .iter()
            .filter_map(|entry| match entry {
                TraceEntry::Injection { batch, .. } => Some(
                    batch
                        .blocks()
                        .iter()
                        .map(|block| block.text().to_string())
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>();
        self.seen
            .push((view.turn(), view.input().as_str().to_string(), injections));
        self.steps.pop_front().ok_or(scripted_model_failure())
    }
}

fn expand(target: &str) -> Action {
    Action::with_params(
        ActionId::new("open_app"),
        ActionParams::none().with("target", target),
    )
}

fn list() -> Action {
    Action::with_params(ActionId::new("files_list"), ActionParams::none())
}

fn observations(session: &RunSession<'_>) -> Vec<(String, ResultState)> {
    session
        .trace()
        .iter()
        .filter_map(|entry| match entry {
            TraceEntry::Observation(observation) => Some((
                observation.action().as_str().to_string(),
                observation.state(),
            )),
            _ => None,
        })
        // TASK_69：只看**真实业务动作**——"先看格式"那一轮（`inspect_action`）也是行动结果，
        // 混进来会让"这条轨迹上一共发生过什么"随手数一变就错位。
        .filter(|(action, _)| action != "inspect_action")
        .collect()
}

// ---------------------------------------------------------------------------
// 1. 真实回环：纯正文 → 等待 → 新输入 → 仅按协议继续（§8.3 / §8.13 的核心）
// ---------------------------------------------------------------------------

/// 一次纯正文响应**保存之后**停下驱动：活动不结束、请求数不增长；新输入之后按协议继续。
#[test]
fn a_pure_body_reply_waits_and_the_next_input_continues_the_same_run() {
    let temp = TempDir::new("loopback-wait");
    let materials = temp.materials();
    let mut environment = environment(&temp);
    let (endpoint, bodies) = spawn_reply_server(vec![
        // 第一轮：纯正文，没有任何工具调用。
        text_reply("我先说一句，等宿主给下一段"),
        // 第二轮：主体自己收场。
        finish_reply(),
    ]);

    let declared = agent_id();
    let mut model = loopback_model(&endpoint);
    let mut session = RunSession::open(
        &materials,
        &mut environment,
        session_request(&declared, "run-wait", "开场输入", Limits::new(6)),
    )
    .expect("会话可以打开");

    // 第一次驱动：模型只说了正文 → 停下等宿主。**不**是收场。
    let first = session.drive(&mut model).expect("驱动不应失败");
    assert!(first.is_unsettled(), "纯正文必须映射成等待");
    assert!(
        session.outcome().is_none(),
        "等待不是收场：不得出现任何结局"
    );
    assert_eq!(session.run().as_str(), "run-wait", "还是同一个活动");

    // 新输入：同一活动内继续。
    let second = session
        .drive_with_input(RunInput::text("第二段").expect("在上限内"), &mut model)
        .expect("再次驱动不应失败");
    assert_eq!(
        second,
        SessionDrive::Ended(Outcome::Subject(SubjectEnding::Ended)),
        "第二轮是主体自己收场（**不带值**：结束了就是结束了）"
    );

    let bodies = receive_bodies(&bodies);
    assert_eq!(
        bodies.len(),
        2,
        "等待期间不得出现第三次请求：等待不自动再驱动一次"
    );
    assert!(bodies[0].contains("开场输入"), "首轮带开场输入");
    assert!(
        !bodies[0].contains("第二段"),
        "首轮不得预先带上还没给出的输入"
    );
    assert_eq!(
        bodies[1].matches("第二段").count(),
        1,
        "新输入必须恰好出现一次"
    );
    assert_eq!(
        bodies[1].matches("我先说一句，等宿主给下一段").count(),
        1,
        "上一轮正文按协议作为历史回传，而且不重复渲染"
    );
    // 等待的代价：中间那段没有任何请求，因此服务端只收到两条。
    assert!(bodies[1].contains("开场输入"), "初始输入不因新输入而消失");
}

// ---------------------------------------------------------------------------
// 2. 换一个新的适配层实例：只用持久记录重建，配对相邻（§8.7）
// ---------------------------------------------------------------------------

/// 一个**全新的** Adapter 实例从已确认记录重建请求：调用标识与它的回执**相邻**，
/// 正文一次，原生 `arguments` 是字符串里的 JSON。
#[test]
fn a_fresh_adapter_instance_rebuilds_the_paired_request_from_persisted_records() {
    let temp = TempDir::new("fresh-instance");
    let materials = temp.materials();
    let env_root = temp.env_root();
    let mut environment = Environment::create(&env_root, agent_id()).expect("建立身份环境");

    // 第一个实例（服务 A）：第 1 轮请求展开、第 2 轮纯正文 → 停下等待。
    let (endpoint_a, bodies_a) = spawn_reply_server(vec![
        tool_call_reply("call-alpha", "open_app", "{\"target\":\"files\"}"),
        text_reply("展开好了"),
    ]);
    // 第二个实例（服务 B）：第 3 轮收场。它**没有**见过前两轮的任何东西。
    let (endpoint_b, bodies_b) = spawn_reply_server(vec![finish_reply()]);

    let declared = agent_id();
    let mut first_instance = loopback_model(&endpoint_a);
    let mut session = RunSession::open(
        &materials,
        &mut environment,
        session_request(&declared, "run-fresh", "开场", Limits::new(6)),
    )
    .expect("会话可以打开");

    assert!(
        session
            .drive(&mut first_instance)
            .expect("第一次驱动")
            .is_unsettled(),
        "第 2 轮是纯正文 → 等待"
    );
    assert_eq!(
        observations(&session),
        vec![("open_app".to_string(), ResultState::Success)],
        "展开真的执行过"
    );

    // 丢弃第一个实例，换一个同协议兼容的新实例接着驱动同一个会话。
    drop(first_instance);
    let mut second_instance = loopback_model(&endpoint_b);
    let ended = session
        .drive_with_input(
            RunInput::text("接着来").expect("在上限内"),
            &mut second_instance,
        )
        .expect("新实例应当能接着驱动");
    assert_eq!(
        ended,
        SessionDrive::Ended(Outcome::Subject(SubjectEnding::Ended))
    );

    let a = receive_bodies(&bodies_a);
    let b = receive_bodies(&bodies_b);
    assert_eq!(a.len(), 2, "第一个实例发了两条请求");
    assert_eq!(b.len(), 1, "第二个实例只发了一条：重建不靠旧实例的历史");

    // 第 2 条请求（同一个实例、下一轮）：助手帧与它的回执**相邻**。
    assert_paired(&a[1], "call-alpha", "open_app", "success");
    // 第 3 条请求（**新实例**）：同样的配对关系由已确认记录重建出来。
    assert_paired(&b[0], "call-alpha", "open_app", "success");
    assert_eq!(b[0].matches("展开好了").count(), 1, "上一轮正文只出现一次");
    // 原始 `arguments` 是**字符串里的 JSON**（供应商契约），不是嵌套对象。
    let wire = parse_body(&b[0]);
    let call = first_call(&wire);
    let arguments = call["function"]["arguments"]
        .as_str()
        .expect("arguments 必须是字符串");
    let decoded: serde_json::Value = serde_json::from_str(arguments).expect("字符串里是 JSON");
    assert_eq!(decoded["target"], "files", "参数按原样回传");
    assert!(!b[0].contains(SENTINEL_KEY), "凭据不进请求体");
}

// ---------------------------------------------------------------------------
// 3. 真实回环：准入拒绝用原调用标识回执，零行动事实（§8.8）
// ---------------------------------------------------------------------------

/// 被拒绝的请求不是结果：它带着**原调用标识**回执"拒绝、未执行"，而且不写任何行动事实。
#[test]
fn a_refused_request_is_receipted_with_its_own_call_id_and_leaves_no_action_facts() {
    let temp = TempDir::new("refusal-pair");
    let materials = temp.materials();
    let env_root = temp.env_root();
    let mut environment = Environment::create(&env_root, agent_id()).expect("建立身份环境");

    // 第 1 轮：请求一个**还没展开**的文件能力 → 执行端拒绝；
    // 第 2 轮：纯正文 → 停下等待。
    let (endpoint, bodies) = spawn_reply_server(vec![
        tool_call_reply("call-refused", "files_list", "{}"),
        text_reply("知道了，我不再试"),
    ]);

    let declared = agent_id();
    let mut model = loopback_model(&endpoint);
    let mut session = RunSession::open(
        &materials,
        &mut environment,
        session_request(&declared, "run-refused", "开场", Limits::new(6)),
    )
    .expect("会话可以打开");
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    // 轨迹侧：只有一条拒绝经历，没有任何结果。
    let refusals: Vec<(String, String)> = session
        .trace()
        .iter()
        .filter_map(|entry| match entry {
            TraceEntry::Refused { action, class, .. } => {
                Some((action.as_str().to_string(), class.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        refusals,
        vec![("files_list".to_string(), "not-expanded".to_string())],
        "未展开的请求必须留下一条可见的拒绝经历"
    );
    assert!(
        observations(&session).is_empty(),
        "被拒绝的请求**不是**一次结果: {:?}",
        observations(&session)
    );

    // 事实侧：零 Prepared / Dispatched / ActionResult。
    let facts = std::fs::read_to_string(env_root.join("facts.ndjson")).expect("读事实存储");
    for fabricated in ["action-prepared", "action-dispatched", "action-result"] {
        assert!(
            !facts.contains(fabricated),
            "被拒绝的请求不得产生 {fabricated}: {facts}"
        );
    }

    // 协议侧：第 2 条请求里，助手帧的调用标识与它的回执相邻，回执明说未派发。
    let bodies = receive_bodies(&bodies);
    assert_eq!(bodies.len(), 2);
    assert_paired(&bodies[1], "call-refused", "files_list", "refused");
    let wire = parse_body(&bodies[1]);
    let receipt = paired_receipt(&wire).expect("有配对回执");
    let content = receipt["content"].as_str().expect("回执有正文");
    assert!(
        content.contains("dispatched = false"),
        "回执必须明说没有派发过: {content}"
    );
    assert!(
        !content.contains("state ="),
        "拒绝不是结果，不得给出结果类别: {content}"
    );
    assert!(
        !content.contains("operation ="),
        "拒绝没有操作身份: {content}"
    );
}

// ---------------------------------------------------------------------------
// 4. 展开、轨迹与预算跨等待保留（§8.4）
// ---------------------------------------------------------------------------

/// 等待前展开的能力，等待之后仍然可用；轨迹、轮次与预算都还是同一份。
#[test]
fn expansion_trace_and_budget_survive_a_wait() {
    let temp = TempDir::new("keep-expansion");
    let materials = temp.materials();
    let mut environment = environment(&temp);

    let mut model = InspectsFirst::new(ScriptedModel::new(vec![
        // 第 1 轮：展开文件视图。
        ModelOutput::act(expand("files")),
        // 第 2 轮：纯正文 → 等待。
        ModelOutput::say("等宿主的下一段"),
        // 第 3 轮：**直接**用文件能力（不再展开）；展开状态丢了就会被拒。
        ModelOutput::act(list()),
        // 第 4 轮：收场。
        ModelOutput::finish(SubjectEnding::Completed),
    ]));

    let declared = agent_id();
    let mut session = RunSession::open(
        &materials,
        &mut environment,
        session_request(&declared, "run-keep", "开场", Limits::new(6)),
    )
    .expect("可以打开");
    assert_eq!(
        session.agent(),
        &declared,
        "会话绑定的身份就是环境里的权威身份"
    );

    // 一次驱动会一直走到"停下"或"收场"：展开那一轮之后紧接着就是纯正文。
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());
    assert_eq!(
        model.calls(),
        4,
        "等待之前用掉四轮：先看格式一轮、展开一轮、纯正文一轮…"
    );
    assert!(session.outcome().is_none(), "等待不是收场：轨迹与预算还在");

    // 等待之后**不再展开**：直接用文件能力。
    let ended = session
        .drive_with_input(RunInput::text("继续").expect("在上限内"), &mut model)
        .expect("驱动");
    assert_eq!(
        ended,
        SessionDrive::Ended(Outcome::Subject(SubjectEnding::Completed))
    );
    assert_eq!(
        observations(&session),
        vec![
            ("open_app".to_string(), ResultState::Success),
            ("files_list".to_string(), ResultState::Success),
        ],
        "等待之后没再展开就能直接用文件能力"
    );
    // 轮次跨越等待继续递增，没有回到 1。
    //
    // TASK_69：主体开场那两批"先看格式"各占一轮（那些轮次里内层不被调）⇒ 内层看到的轮次不再逐连。
    // 本判据真正要守的是**同一份编号一直往上走、没有回到 1**；再对一次总数（没有跳号、没有重号）。
    let turns: Vec<u32> = model
        .inner()
        .seen
        .iter()
        .map(|(turn, _, _)| turn.number())
        .collect();
    assert!(
        turns.windows(2).all(|pair| pair[0] < pair[1]),
        "每次驱动一轮，等待不耗轮：轮次必须严格递增、没有回到 1: {turns:?}"
    );
    assert_eq!(
        *turns.last().expect("有调用"),
        model.calls() as u32,
        "轮次编号与总调用数一致（没有跳号、没有重号）"
    );
}

// ---------------------------------------------------------------------------
// 5. 新输入恰好一次；登记恰好一次；等待不耗轮；预算不重置（§8.5）
// ---------------------------------------------------------------------------

#[test]
fn new_input_appears_once_and_the_run_is_registered_once() {
    let temp = TempDir::new("input-once");
    let materials = temp.materials();
    let mut environment = environment(&temp);

    let mut model = InspectsFirst::new(ScriptedModel::new(vec![
        ModelOutput::say("第一次等"),
        ModelOutput::say("第二次等"),
        ModelOutput::finish(SubjectEnding::Silent),
    ]));

    let declared = agent_id();
    let mut session = RunSession::open(
        &materials,
        &mut environment,
        session_request(&declared, "run-input", "开场", Limits::new(6)),
    )
    .expect("可以打开");

    assert!(session.drive(&mut model).expect("驱动").is_unsettled());
    assert!(
        session
            .drive_with_input(RunInput::text("只有一次").expect("在上限内"), &mut model)
            .expect("驱动")
            .is_unsettled()
    );
    // 第三次不给新输入：显式继续。
    assert!(matches!(
        session.drive(&mut model).expect("驱动"),
        SessionDrive::Ended(_)
    ));

    let turns: Vec<u32> = model
        .inner()
        .seen
        .iter()
        .map(|(turn, _, _)| turn.number())
        .collect();
    assert_eq!(
        turns,
        vec![2, 3, 4],
        "每次驱动一轮，等待不耗轮（第 1 拍先看格式）"
    );

    // 新输入在随后每一轮里恰好出现一次（它已经进了轨迹，因此之后也读得到）。
    //
    // TASK_69：主体开场"先看格式"那一轮占掉第 1 轮，所以**第一条**内层调用（第 2 轮）本来就早于
    // 新输入——与其写死"从第几轮起"，不如按语义来：从**第一次看见它**的那一轮起，往后每轮恰好一次。
    let mut delivered = false;
    for (turn, _, injections) in &model.inner().seen {
        let count = injections
            .iter()
            .filter(|text| text.as_str() == "只有一次")
            .count();
        if delivered {
            assert_eq!(
                count, 1,
                "第 {turn:?} 轮里新输入必须恰好一次: {injections:?}"
            );
        } else if count > 0 {
            assert_eq!(count, 1, "送达那一轮起就必须恰好一次: {injections:?}");
            delivered = true;
        }
    }
    assert!(delivered, "新输入必须在随后某一轮里出现过");
    // 初始输入仍然是初始输入：它没有因为新输入而消失。
    assert!(
        model.inner().seen.last().expect("有调用").1 == "开场",
        "视野里的初始输入不被新输入覆盖"
    );

    // 登记恰好一次：Run 级注入只有一条。
    let run_start_injections = session
        .trace()
        .iter()
        .filter(|entry| {
            matches!(
                entry,
                TraceEntry::Injection {
                    stage: InjectionStage::RunStart,
                    ..
                }
            )
        })
        .count();
    assert_eq!(run_start_injections, 1, "Run 级注入恰好一次");
    // Turn 级注入必须带真实轮次（"哪一轮"是 Core 标记的，不是外层推的）。
    for entry in session.trace() {
        if let TraceEntry::Injection { stage, turn, .. } = entry {
            assert_eq!(
                *stage == InjectionStage::Turn,
                turn.is_some(),
                "轮次级注入必须带真实轮次: {entry:?}"
            );
        }
    }
}

/// 预算用完之后再驱动：**零**新增请求，也不冒充主体完成。
#[test]
fn an_exhausted_budget_makes_no_extra_request() {
    let temp = TempDir::new("budget");
    let materials = temp.materials();
    let mut environment = environment(&temp);

    let mut model = InspectsFirst::new(ScriptedModel::new(vec![ModelOutput::say("等")]));
    let declared = agent_id();
    let mut session = RunSession::open(
        &materials,
        &mut environment,
        // TASK_69：预算要装得下"先看格式"那一轮（第 1 轮）＋自己的"等"（第 2 轮）——
        // 1 拍会被开场查阅吃光，这个舞台就测不到"预算耗尽之后"那一步了。
        session_request(&declared, "run-budget", "开场", Limits::new(2)),
    )
    .expect("可以打开");

    assert!(session.drive(&mut model).expect("驱动").is_unsettled());
    assert_eq!(model.calls(), 2, "先看格式一轮 + 主体的'等'一轮");

    // 再驱动：预算已经用完，不得绕过上限多请求一次，也不得冒充主体完成。
    assert_eq!(
        session.drive(&mut model).expect("驱动"),
        SessionDrive::Ended(Outcome::System(SystemEnding::Timeout)),
        "耗尽预算如实报成系统终局"
    );
    assert_eq!(model.calls(), 2, "不得多请求一次");
}

// ---------------------------------------------------------------------------
// 6. 拒绝：环境错配、第二个 Run、收场后再驱动、失败不降级成等待（§8.6）
// ---------------------------------------------------------------------------

#[test]
fn a_mismatched_identity_is_refused_without_assembling_anything() {
    let temp = TempDir::new("mismatch");
    let materials = temp.materials();
    let mut environment = environment(&temp);

    let wrong = AgentId::new("agent-other");
    match RunSession::open(
        &materials,
        &mut environment,
        session_request(&wrong, "run-mismatch", "开场", Limits::new(2)),
    ) {
        Err(CompositionError::Environment(reason)) => {
            assert_eq!(reason, "identity-mismatch", "沿用存储层自己的稳定标记");
        }
        Err(other) => panic!("身份错配必须被拒绝，却得到 {other:?}"),
        Ok(_) => panic!("身份错配必须被拒绝，却成功打开了会话"),
    }
}

/// 等待期间同一个环境里不能开始第二个活动：留下的是"未收尾"，不是"存储故障"。
#[test]
fn a_second_activity_is_refused_while_the_first_one_waits() {
    let temp = TempDir::new("second-run");
    let materials = temp.materials();
    let mut environment = environment(&temp);

    let declared = agent_id();
    {
        let mut model =
            InspectsFirst::new(ScriptedModel::new(vec![ModelOutput::say("等宿主的下一段")]));
        let mut session = RunSession::open(
            &materials,
            &mut environment,
            session_request(&declared, "run-first", "开场", Limits::new(4)),
        )
        .expect("可以打开");
        assert!(session.drive(&mut model).expect("驱动").is_unsettled());
        // 会话在这里被丢弃：等待中的活动**没有**结束。
    }

    // 环境自己的准入判定说得很清楚：有已登记开始、却没有可靠结束事实的活动。
    match environment.admission().expect("可以判定准入") {
        Admission::Unfinished { run } => assert_eq!(run.as_str(), "run-first"),
        other => panic!("等待中的活动必须留下未收尾痕迹，却得到 {other:?}"),
    }
    // ⚠ **2026-09-26 改（旧规矩 → 新规矩）**：第二个活动**不再被拒绝**——进程没了就是那一段断了，
    // 新进程起来是**新的一段**。而**放行不等于解决**：等待中那一段的未收尾痕迹一个字没少，
    // 上面那条准入断言守的就是这半句。
    RunSession::open(
        &materials,
        &mut environment,
        session_request(&declared, "run-second", "另一段", Limits::new(4)),
    )
    .expect("没有收尾的旧活动不再拦住新活动");
    match environment.admission().expect("可以判定准入") {
        Admission::Unfinished { run } => assert_eq!(run.as_str(), "run-first"),
        other => panic!("放行不等于解决：旧痕迹必须仍然在，却得到 {other:?}"),
    }
}

/// 收场之后再驱动是明确的拒绝，不是又一次等待。
#[test]
fn a_session_cannot_be_driven_after_it_ends() {
    let temp = TempDir::new("after-end");
    let materials = temp.materials();
    let mut environment = environment(&temp);

    let mut model = InspectsFirst::new(ScriptedModel::new(vec![ModelOutput::finish(
        SubjectEnding::Completed,
    )]));
    let declared = agent_id();
    let mut session = RunSession::open(
        &materials,
        &mut environment,
        session_request(&declared, "run-after-end", "开场", Limits::new(4)),
    )
    .expect("可以打开");

    assert!(matches!(
        session.drive(&mut model).expect("第一次驱动"),
        SessionDrive::Ended(_)
    ));
    // TASK_69：主体开场"先看格式"那一轮也是一次调用 ⇒ 不写死"一共 1 次"，
    // 改成**在拒绝前后对账**：被拒绝的那几个动作一个字节都不许多调。
    let calls_before = model.calls();
    assert_eq!(
        session.drive(&mut model),
        Err(CompositionError::SessionNotDrivable),
        "收场之后不得再被驱动"
    );
    assert_eq!(
        session.provide(RunInput::text("太晚了").expect("在上限内")),
        Err(CompositionError::SessionNotDrivable)
    );
    assert_eq!(model.calls(), calls_before, "被拒绝的驱动不得调用模型");
}

/// 保存没被确认：这次驱动**以失败告终**，错误路径不降级成等待，也不伪造终态。
///
/// 故障注入是真的、不是静态推断：先用同一个 RunId 完整跑完一次活动并封存，那个活动标识从此
/// 是**已登记**的；再用同一个标识驱动，登记那一步的写入必然被环境自己拒绝——于是这次驱动在
/// **第一次模型调用之前**就以"保存没被确认"失败。
#[test]
fn a_failed_drive_never_becomes_a_waiting_session() {
    let temp = TempDir::new("failed-drive");
    let materials = temp.materials();
    let mut environment = environment(&temp);
    let declared = agent_id();

    {
        let mut model = InspectsFirst::new(ScriptedModel::new(vec![ModelOutput::finish(
            SubjectEnding::Silent,
        )]));
        let mut first = RunSession::open(
            &materials,
            &mut environment,
            session_request(&declared, "run-taken", "开场", Limits::new(2)),
        )
        .expect("第一个活动可以打开");
        assert!(matches!(
            first.drive(&mut model).expect("第一个活动应当正常收场"),
            SessionDrive::Ended(_)
        ));
    }

    let mut model = InspectsFirst::new(ScriptedModel::new(vec![ModelOutput::say("等")]));
    let mut session = RunSession::open(
        &materials,
        &mut environment,
        session_request(&declared, "run-taken", "开场", Limits::new(2)),
    )
    .expect("会话本身可以打开：登记那一刻才失败");
    let failure = session.drive(&mut model).expect_err("写不下去就必须失败");
    assert!(
        matches!(
            failure,
            CompositionError::SaveFailed { .. }
                | CompositionError::Run(celira_agent::core::RunError::Commit(_))
        ),
        "必须是保存没被确认那一类: {failure:?}"
    );
    assert_eq!(
        model.calls(),
        0,
        "失败发生在第一次模型调用之前：登记没被确认，就没有任何请求"
    );
    assert!(
        session.outcome().is_none(),
        "失败不是收场：不得出现任何终态"
    );
    assert_eq!(
        session.drive(&mut model),
        Err(CompositionError::SessionNotDrivable),
        "失败之后不得再被驱动，更不能被再驱动一次抹成等待"
    );
    assert_eq!(model.calls(), 0, "被拒绝的驱动不得调用模型");
}

// ---------------------------------------------------------------------------
// 6b. 组件区（TASK_33）：主体贴的纸条在**第一封请求**里就摆着，而第二轮不重摆
// ---------------------------------------------------------------------------

/// 往主体的笔记存储里手写一条（贴是主体的动作，行为判据在 `tests/notes_app.rs`；
/// 这里只判**装配与注入**：写进去的东西有没有在第一封请求里出现）。
fn seed_note(env_root: &std::path::Path, id: &str, title: &str, text: &str, pinned: bool) {
    let directory = env_root.join("notes");
    std::fs::create_dir_all(&directory).expect("notes 目录");
    std::fs::write(
        directory.join(format!("{id}.note")),
        format!("{{\"title\":\"{title}\",\"text\":\"{text}\",\"pinned\":{pinned}}}\n"),
    )
    .expect("写笔记文件");
}

/// **判据（生产装配路径）**：主体贴在墙上的纸条，在**第一封请求**里就摆着——**主体没做任何动作**；
/// 没贴的不出现；组件区**只在开场摆一次**（第二轮不再重摆）。
#[test]
fn the_first_request_of_a_real_session_already_carries_her_pinned_notes() {
    let temp = TempDir::new("session-pins");
    let env_root = temp.env_root();
    // ⚠ **顺序即事实**（2026-10-06）：环境先存在，笔记才写得进去——`create` 只在一个空位置上
    // 创建（身份不在却有别的东西 ⇒ 明确拒绝，见 README §3.3）。
    let mut environment = environment(&temp);
    seed_note(
        &env_root,
        "note-1",
        "词：青苔",
        "青苔——主人要求记住的一个词。",
        true,
    );
    seed_note(&env_root, "note-2", "没贴的", "这条没贴。", false);

    let (endpoint, bodies) = spawn_reply_server(vec![text_reply("等我一下"), finish_reply()]);
    let run = manual_run(&temp, "run-pins", ManualInput::Text("醒着".to_string()));
    let mut model = loopback_model(&endpoint);
    // 一次显式继续 ⇒ 第二轮（用来判「快照只在开场摆一次」）。
    let mut host: &[u8] = "\n".as_bytes();
    run_host_session_in(
        &mut environment,
        &run,
        RunInput::text("醒着").expect("在上限内"),
        &mut host,
        &mut model,
    )
    .expect("循环本身不该失败");

    let bodies = receive_bodies(&bodies);
    assert!(bodies.len() >= 2, "应当驱动两轮：{}", bodies.len());
    let first = &bodies[0];
    assert!(
        first.contains("pinned-notes"),
        "第一封请求里就该有组件区块：{}",
        redacted(first)
    );
    assert!(first.contains("note-1"), "{}", redacted(first));
    assert!(
        first.contains("青苔——主人要求记住的一个词。"),
        "预览要在里面（全文仍走 notes_read）：{}",
        redacted(first)
    );
    assert!(
        !first.contains("note-2"),
        "没贴的不该出现在墙上：{}",
        redacted(first)
    );

    let second = &bodies[1];
    // 请求体是**全历史**（第二封会把开场那条一起带上），所以「不再摆」不能判成「不含」——
    // 正确的机械判据是：**第二封里没有新增**这一块（出现次数不增长），而且第二轮那一段
    // 注入批（`[before turn 2]`）里没有它。
    let openings = first.matches("pinned-notes").count();
    assert_eq!(
        second.matches("pinned-notes").count(),
        openings,
        "组件区是开场快照，第二轮不许新增一块：{}",
        redacted(second)
    );
    let second_batch = second
        .split("[before turn 2]")
        .nth(1)
        .expect("第二轮应当有自己的注入批");
    assert!(
        !second_batch.contains("pinned-notes"),
        "第二轮那一段注入批里不许有它：{}",
        redacted(second_batch)
    );
}

/// **缺省不等于零（反向同律）**：墙上空着也要**说出口**——主体才知道是「空」而不是「没读到」。
#[test]
fn an_empty_wall_is_still_stated_in_the_first_request() {
    let temp = TempDir::new("session-pins-empty");
    let mut environment = environment(&temp);
    let (endpoint, bodies) = spawn_reply_server(vec![finish_reply()]);
    let run = manual_run(
        &temp,
        "run-pins-empty",
        ManualInput::Text("醒着".to_string()),
    );
    let mut model = loopback_model(&endpoint);
    let mut host: &[u8] = b"";
    run_host_session_in(
        &mut environment,
        &run,
        RunInput::text("醒着").expect("在上限内"),
        &mut host,
        &mut model,
    )
    .expect("循环本身不该失败");

    let bodies = receive_bodies(&bodies);
    let first = &bodies[0];
    assert!(
        first.contains("pinned = 0"),
        "墙上空着也要说出口：{}",
        redacted(first)
    );
}

// ---------------------------------------------------------------------------
// 6c. 事件卡片（TASK_34）：Run 内增量、一条只播一次、零动静不出块、零正文
// ---------------------------------------------------------------------------

/// 判据（生产装配路径）：跑一次真会话、看**每一封请求体**里那块卡片。
///
/// 细胞是 Run 级内存：这里预先把它填成"某间房 2 条"，于是**第 1 轮**就该看见它，
/// 而**第 2 轮**不许再播（一条只播一次）。
#[test]
fn an_event_card_is_played_once_at_the_next_turn_and_never_again() {
    let temp = TempDir::new("session-cards");
    let mut environment = environment(&temp);
    let (writer, report) = celira_agent::event_cards::EventCardsReport::channel();
    // 模拟"宿主在主体跑着的时候收到铃、拉了一次 B1"（两铃合并成同一张卡：后到的覆盖式并进）。
    writer.merge(&[("值班室".to_string(), 1)]);
    writer.merge(&[("值班室".to_string(), 2), ("清静间".to_string(), 1)]);

    let (endpoint, bodies) = spawn_reply_server(vec![
        text_reply("等我一下"),
        text_reply("接着说"),
        finish_reply(),
    ]);
    let agent = agent_id();
    let request = session_request(&agent, "run-cards", "醒着", Limits::new(6));
    let mut model = loopback_model(&endpoint);
    let mut session = RunSession::open_declared(
        &temp.materials(),
        &mut environment,
        RunOrigin {
            origin: celira_agent::store::WakeOrigin::Manual,
            since: None,
            notification: None,
            cards: Some(report),
        },
        request,
    )
    .expect("会话");
    while let Ok(drive) = session.drive(&mut model) {
        match drive {
            SessionDrive::Ended(_) => break,
            SessionDrive::Unsettled => {}
        }
    }

    let bodies = receive_bodies(&bodies);
    assert!(bodies.len() >= 2, "至少两轮：{}", bodies.len());
    let first = &bodies[0];
    // 请求体是**全历史**：判"开场不许有"要只看**开场那一段**（`[before turn 1]` 之前），
    // 而不是看整封里有没有出现过（那是本刀通用的那条口径：数次数、不数出现）。
    let opening = first.split("[before turn 1]").next().unwrap_or_default();
    assert!(
        !opening.contains(EVENT_CARDS_LABEL),
        "开场那一轮不该有卡片（那是通知栏的活）：{}",
        redacted(opening)
    );
    assert!(
        first.contains("event-cards"),
        "第 1 轮该看见它（细胞里已经有了）：{}",
        redacted(first)
    );
    assert!(first.contains("值班室 +2"), "{}", redacted(first));
    assert!(first.contains("清静间 +1"), "{}", redacted(first));

    // 一条只播一次：后面的每一封里，它都**没有新增**（出现次数不增长）。
    let played = first.matches(EVENT_CARDS_LABEL).count();
    for body in &bodies[1..] {
        assert_eq!(
            body.matches(EVENT_CARDS_LABEL).count(),
            played,
            "卡片只播一次，后面不许新增：{}",
            redacted(body)
        );
    }

    // 零正文：卡片**逐字**只有「那一句话 + 每间房一行计数」（增量多在前、同量按房名）。
    //
    // 这是本刀最硬的一条形状判据，负对照是回执：回执带着预览，卡片一个字节都不许有。
    let card = card_block(first).expect("第 1 轮该有卡片");
    assert_eq!(
        card, "自上轮以来：\n值班室 +2\n清静间 +1",
        "卡片只许有房名与计数，不许有正文"
    );
}

/// 零动静不出块：细胞空着 ⇒ 每一封请求里都没有卡片（与快照那两件刻意相反）。
#[test]
fn no_ring_no_card() {
    let temp = TempDir::new("session-cards-quiet");
    let mut environment = environment(&temp);
    let (_writer, report) = celira_agent::event_cards::EventCardsReport::channel();
    let (endpoint, bodies) = spawn_reply_server(vec![finish_reply()]);
    let agent = agent_id();
    let request = session_request(&agent, "run-quiet", "醒着", Limits::new(4));
    let mut model = loopback_model(&endpoint);
    let mut session = RunSession::open_declared(
        &temp.materials(),
        &mut environment,
        RunOrigin {
            origin: celira_agent::store::WakeOrigin::Manual,
            since: None,
            notification: None,
            cards: Some(report),
        },
        request,
    )
    .expect("会话");
    while let Ok(drive) = session.drive(&mut model) {
        if let SessionDrive::Ended(_) = drive {
            break;
        }
    }

    let bodies = receive_bodies(&bodies);
    for body in &bodies {
        assert!(
            !body.contains(EVENT_CARDS_LABEL),
            "没有动静就不许出卡片：{}",
            redacted(body)
        );
    }
}

/// 同卡同形：**同一份细胞**（两铃以不同顺序并进来）⇒ 卡片逐字节相同，且只出一张。
///
/// 判的是那条机械排序纪律（增量多在前、同量按房名）：顺序若随"读到的先后"变，
/// 同一件事在两次醒里就长得不一样，主体每次都得重新认这张卡。
/// 顺带把施工单那条"两铃合并成一张卡"落成读数：两次 `merge` 只出一块、两行计数。
#[test]
fn the_same_cell_renders_the_same_card_bytes() {
    fn first_card(report: celira_agent::event_cards::EventCardsReport, label: &str) -> String {
        let temp = TempDir::new(label);
        let mut environment = environment(&temp);
        let (endpoint, bodies) = spawn_reply_server(vec![finish_reply()]);
        let agent = agent_id();
        let request = session_request(&agent, &format!("run-{label}"), "醒着", Limits::new(3));
        let mut model = loopback_model(&endpoint);
        let mut session = RunSession::open_declared(
            &temp.materials(),
            &mut environment,
            RunOrigin {
                origin: celira_agent::store::WakeOrigin::Manual,
                since: None,
                notification: None,
                cards: Some(report),
            },
            request,
        )
        .expect("会话");
        while let Ok(drive) = session.drive(&mut model) {
            if let SessionDrive::Ended(_) = drive {
                break;
            }
        }
        let bodies = receive_bodies(&bodies);
        card_block(&bodies[0]).expect("第一轮该有卡片")
    }

    let (writer_a, report_a) = celira_agent::event_cards::EventCardsReport::channel();
    writer_a.merge(&[("乙室".to_string(), 3)]);
    writer_a.merge(&[("甲室".to_string(), 2), ("乙室".to_string(), 3)]);
    let (writer_b, report_b) = celira_agent::event_cards::EventCardsReport::channel();
    writer_b.merge(&[("甲室".to_string(), 2), ("乙室".to_string(), 3)]);
    writer_b.merge(&[("甲室".to_string(), 2)]);

    let left = first_card(report_a, "cards-shape-a");
    let right = first_card(report_b, "cards-shape-b");
    assert_eq!(left, right, "同一份细胞必须长成同一张卡");
    assert_eq!(
        left, "自上轮以来：\n乙室 +3\n甲室 +2",
        "两铃合并成一张 + 机械排序"
    );
}

// ---------------------------------------------------------------------------
// 7. 显式宿主输入循环：输入 → 行动 → 纯正文等待 → 新输入 → 收场（§8.13）
// ---------------------------------------------------------------------------

/// **判据 1（生产装配路径）**：一次真实会话的**第一封请求**里，主体的动词已经全在——
/// notes 四件、im 六件与 web 两件**都不需要 `open_app`**，而材料窗（files）照旧要点开。
///
/// 后两件是**出厂内置**的（TASK_35）：web 窗不依赖宿主逐次递内容，因此它从第一轮起就在场。
///
/// 判的是请求体（模型实际看到的那份 tools），不是我们自己拼的目录：只有这条路径能证明
/// "装配真的这么装了"。
#[test]
fn the_first_request_of_a_real_session_already_carries_her_own_verbs() {
    let temp = TempDir::new("first-request-tools");
    let env_root = temp.env_root();
    let mut environment = Environment::create(&env_root, agent_id()).expect("建立身份环境");

    // TASK_41 夹具：材料区里放两个**真形状**的技能（一个散 skill、一个包）。
    for (relative, body) in [
        (
            "skills/web-search/SKILL.md",
            "---\nname: web-search\ndescription: 在网上搜东西并回答\n---\n\n# 步骤\n",
        ),
        (
            "skills/notes-pack/notes-tidy/SKILL.md",
            "---\nname: notes-tidy\ndescription: 把笔记整理成条目\n---\n\n# 步骤\n",
        ),
        (
            "skills/notes-pack/notes-search/SKILL.md",
            "---\nname: notes-search\ndescription: 在笔记里找东西\n---\n\n# 步骤\n",
        ),
    ] {
        let path = temp.materials().join(relative);
        std::fs::create_dir_all(path.parent().expect("有父目录")).expect("建目录");
        std::fs::write(path, body).expect("写清单");
    }
    let (endpoint, bodies) = spawn_reply_server(vec![finish_reply()]);
    let mut run = manual_run(&temp, "run-tools", ManualInput::Text("醒着".to_string()));
    // 声明 im：与生产一样，端点与凭据成对给（这一轮主体不会用到它，因此不需要真的服务端）。
    run.declarations.im = Some(
        celira_agent::im_channel::ImAccess::new("http://127.0.0.1:1/", "fixture-secret")
            .expect("端点与凭据非空"),
    );
    let mut model = loopback_model(&endpoint);
    let mut host: &[u8] = b"";
    run_host_session_in(
        &mut environment,
        &run,
        RunInput::text("醒着").expect("在上限内"),
        &mut host,
        &mut model,
    )
    .expect("循环本身不该失败");

    let bodies = receive_bodies(&bodies);
    assert_eq!(bodies.len(), 1, "一次收场只驱动一轮");
    let first = &bodies[0];
    for verb in [
        "notes_create",
        "notes_list",
        "notes_read",
        "notes_verify_note",
        "im_rooms",
        "im_read_history",
        "im_say",
        "im_verify_said",
        "im_recall",
        "im_verify_recalled",
        // web 窗（TASK_35）：出厂内置、预装常开——主体第一轮就看得见这两条。
        "web_search",
        "web_fetch",
    ] {
        // TASK_68：能力**仍在**（RunScope 与能力模型一字未改），但**不再直连 `tools` 声明**——
        // 头部恒两件，这些动词改由入口 `invoke` 路由；主体怎么看见它们走**注入的目录**（P2）。
        assert!(
            !first.contains(&format!("\"{verb}\"")),
            "「{verb}」不再直连头部（它经入口路由）: {}",
            redacted(first)
        );
    }
    assert!(
        first.contains("\"name\":\"invoke\"") && first.contains("\"name\":\"finish_run\""),
        "首轮头部恒定两件: {}",
        redacted(first)
    );
    assert!(
        !first.contains("\"files_create\""),
        "材料窗照旧要点开，不该在首轮就出现: {}",
        redacted(first)
    );

    // TASK_39（骨架/详情分离）：`tools` 数组里每个动词**只有骨架**（一句话 + 参数表），
    // 上限、机械读数、失败形状这类详情不进每一轮的前缀——它们走 `inspect_action` 的尾部。
    assert!(
        !first.contains("64 KiB"),
        "详情（上限）不该进 tools 前缀: {}",
        redacted(first)
    );
    assert!(
        !first.contains("truncated"),
        "机械读数说明不该进 tools 前缀: {}",
        redacted(first)
    );
    // TASK_68 P2：骨架不再进**工具声明**（头部恒两件）——它改由**注入的动作目录**露面
    // （首轮一份，标签 `available-actions`）；**要走"先看格式"那条路的**动作，参数靠自己去看
    // （`inspect_action`）；**声明方标了"免查"的**那几条（OS 自己那六件）目录行自带参数名。
    assert!(
        first.contains("available-actions"),
        "首轮的目录必须在: {}",
        redacted(first)
    );
    assert!(
        first.contains("notes_read | 读一条自己笔记的标题与正文"),
        "目录里要有那一行动作: {}",
        redacted(first)
    );
    // 2026-10-24：**可调就必须看得见参数名**。读动作不受"先看格式"约束，
    // 调用前没有别的地方能知道参数叫什么——目录行必须自带（真模型 soak 里 41% 的
    // `notes_read` 调用都在猜参数名）。
    assert!(
        first.contains("notes_read | 读一条自己笔记的标题与正文（参数：id"),
        "读动作的参数名必须在目录行里: {}",
        redacted(first)
    );
    // TASK_71 桌面 P0（①）：目录按 App 分组 —— 门牌行（`【<App 标识>】<描述>`）在，门内动词行在。
    // ①b：`open_app` 骨架里那份"当前装配了：…"**移进了组头** ⇒ 首轮请求体里不该再出现它
    //（信息量不增、只重排；清单只活在组头一处）。
    assert!(
        first.contains("【notes】") && first.contains("【files】"),
        "目录要有按 App 分的门牌行: {}",
        redacted(first)
    );
    assert!(
        !first.contains("当前装配了"),
        "App 清单只活在组头，骨架里那份已移走: {}",
        redacted(first)
    );
    assert!(
        !first.contains("\"description\":\"读一条自己笔记的标题与正文\""),
        "骨架不进工具声明: {}",
        redacted(first)
    );

    // TASK_41（灵魂判据："主体什么也不做"的舞台）：材料区里有两个一级条目 ⇒ **第一轮上下文里
    // 技能桌面块就在场**，而主体这一轮什么都没做——线索不依赖主体的任何动作。
    // 2026-10-20 升成有界索引：**名字 ＋ 一句触发进场**（那是 `files_list` 之前就要知道的东西）；
    // 正文仍然零进场。
    assert!(
        first.contains("skills: 2 项（要翻看用 files_list，路径 skills/）"),
        "技能桌面块必须在首轮在场（计数 ＋ 路径）: {}",
        redacted(first)
    );
    assert!(
        first.contains("web-search") && first.contains("notes-pack"),
        "名字要进场（索引的那一半）: {}",
        redacted(first)
    );
    assert!(
        !first.contains("# 步骤"),
        "桌面块不许带清单正文（零正文）: {}",
        redacted(first)
    );
}

/// **引用过滤开场索引**（2026-10-25）：同一份材料区，两支不同的 `SkillRef` 看到不同的一级条目。
/// 这是装配路径的真实验收，不只是 `read_skills_for` 的单元测试。
#[test]
fn a_skill_reference_filters_the_opening_index_in_a_real_session() {
    let temp = TempDir::new("skills-reference");
    let env_root = temp.env_root();
    let mut environment = Environment::create(&env_root, agent_id()).expect("建立身份环境");

    for (relative, body) in [
        (
            "skills/web-search/SKILL.md",
            "---\nname: web-search\ndescription: 在网上搜东西并回答\n---\n\n# 步骤\n",
        ),
        (
            "skills/notes-pack/notes-tidy/SKILL.md",
            "---\nname: notes-tidy\ndescription: 把笔记整理成条目\n---\n\n# 步骤\n",
        ),
    ] {
        let path = temp.materials().join(relative);
        std::fs::create_dir_all(path.parent().expect("有父目录")).expect("建目录");
        std::fs::write(path, body).expect("写清单");
    }

    let (endpoint, bodies) = spawn_reply_server(vec![finish_reply()]);
    let mut run = manual_run(
        &temp,
        "run-skills-ref",
        ManualInput::Text("醒着".to_string()),
    );
    run.declarations.skills = SkillRef::Only(vec!["web-search".to_string()]);
    let mut model = loopback_model(&endpoint);
    let mut host: &[u8] = b"";
    run_host_session_in(
        &mut environment,
        &run,
        RunInput::text("醒着").expect("在上限内"),
        &mut host,
        &mut model,
    )
    .expect("循环本身不该失败");

    let bodies = receive_bodies(&bodies);
    assert_eq!(bodies.len(), 1, "一次收场只驱动一轮");
    let first = &bodies[0];
    assert!(
        first.contains("skills: 1 项（已启用；技能区共 2 项；要翻看用 files_list，路径 skills/）"),
        "索引头要同时说启用数与技能区总数: {}",
        redacted(first)
    );
    assert!(
        first.contains("web-search"),
        "启用的那一项要在: {}",
        redacted(first)
    );
    assert!(
        !first.contains("notes-pack") && !first.contains("notes-tidy"),
        "没启用的包不许进开场索引: {}",
        redacted(first)
    );
}

/// TASK_39：**先看卡片、再调动作**——详情走尾部回执，之后照常调用。
///
/// 两条合法路径都要绿：一条是"骨架足够 ⇒ 直调"（既有用例一直在证明），另一条是这一条
/// ——"先 `inspect_action` 拿详情，回执进历史，然后正确调用"。
#[test]
fn the_model_can_read_a_card_and_then_call_the_action() {
    let temp = TempDir::new("card-then-call");
    let env_root = temp.env_root();
    let mut environment = Environment::create(&env_root, agent_id()).expect("建立身份环境");

    // TASK_41 夹具：材料区里放两个**真形状**的技能（一个散 skill、一个包）。
    for (relative, body) in [
        (
            "skills/web-search/SKILL.md",
            "---\nname: web-search\ndescription: 在网上搜东西并回答\n---\n\n# 步骤\n",
        ),
        (
            "skills/notes-pack/notes-tidy/SKILL.md",
            "---\nname: notes-tidy\ndescription: 把笔记整理成条目\n---\n\n# 步骤\n",
        ),
        (
            "skills/notes-pack/notes-search/SKILL.md",
            "---\nname: notes-search\ndescription: 在笔记里找东西\n---\n\n# 步骤\n",
        ),
    ] {
        let path = temp.materials().join(relative);
        std::fs::create_dir_all(path.parent().expect("有父目录")).expect("建目录");
        std::fs::write(path, body).expect("写清单");
    }
    let (endpoint, bodies) = spawn_reply_server(vec![
        tool_call_reply(
            "call-inspect",
            "inspect_action",
            "{\"action\":\"notes_list\"}",
        ),
        tool_call_reply("call-list", "notes_list", "{}"),
        finish_reply(),
    ]);
    let run = manual_run(&temp, "run-card", ManualInput::Text("醒着".to_string()));
    let mut model = loopback_model(&endpoint);
    let mut host: &[u8] = b"";
    run_host_session_in(
        &mut environment,
        &run,
        RunInput::text("醒着").expect("在上限内"),
        &mut host,
        &mut model,
    )
    .expect("循环本身不该失败");

    let bodies = receive_bodies(&bodies);
    assert_eq!(bodies.len(), 3, "三次驱动各一轮");

    // 第 2 轮的请求里**看得到那张卡片**：骨架、详情、参数表都在历史尾部。
    let second = &bodies[1];
    assert!(
        second.contains("notes_list: ") && second.contains("params:"),
        "卡片必须回到历史里: {}",
        redacted(second)
    );
    assert!(
        second.contains("100 条") && second.contains("truncated"),
        "详情（上限与读数说明）应当在卡片里出现: {}",
        redacted(second)
    );
    // 而入口照旧在场（没被卡片挤掉），第 2 轮真的经它调了那个动作。
    assert!(
        second.contains("\"name\":\"invoke\""),
        "入口照旧在场: {}",
        redacted(second)
    );
    assert!(
        !second.contains("\"name\":\"notes_list\""),
        "那个动作不直连头部（它经入口调）: {}",
        redacted(second)
    );
    // 第 3 轮的请求里看得到那个动作的真实回执。
    assert!(
        bodies[2].contains("notes_list"),
        "动作回执也在历史里: {}",
        redacted(&bodies[2])
    );

    // TASK_41：技能桌面是**开场一次**的快照。读法是「每一轮请求里恰好出现一次」——
    // 那一次在**历史**里（第一封请求的那个块被逐轮重放）。要是每轮都重摆一份，第 2 轮就会
    // 同时有「历史里那份 + 新摆那份」，计数会翻倍。所以判据是 == 请求数，不是 <= 请求数。
    let occurrences: usize = bodies
        .iter()
        .map(|body| body.matches("skills: 2 项").count())
        .sum();
    assert_eq!(
        occurrences,
        bodies.len(),
        "技能桌面只在开场摆一次（每轮只见历史里那一份，不重摆）"
    );
}

/// 生产宿主循环 + 真实适配层跑完整条路，而且逐条请求都可断言。
#[test]
fn the_host_loop_runs_input_action_pure_body_wait_input_and_finish() {
    let temp = TempDir::new("host-full");
    let env_root = temp.env_root();
    let mut environment = Environment::create(&env_root, agent_id()).expect("建立身份环境");

    let (endpoint, bodies) = spawn_reply_server(vec![
        // 第 1 轮：行动（展开文件视图）。
        tool_call_reply("call-open", "open_app", "{\"target\":\"files\"}"),
        // 第 2 轮：纯正文 → 停下等宿主。
        text_reply("展开好了，等你给下一段"),
        // 第 3 轮：主体自己收场。
        finish_reply(),
    ]);

    let run = manual_run(&temp, "run-host", ManualInput::Text("开场输入".to_string()));
    let mut model = loopback_model(&endpoint);
    // 宿主只给一行非空文本，然后流结束：显式输入一次，之后不再继续。
    // 第三行**不该被读到**——收场之后循环必须停下。
    let mut host: &[u8] = "宿主第二段\n不该被读到的第三段\n".as_bytes();
    let ending = run_host_session_in(
        &mut environment,
        &run,
        RunInput::text("开场输入").expect("在上限内"),
        &mut host,
        &mut model,
    )
    .expect("循环本身不该失败");
    assert_eq!(ending, ManualEnding::Subject(SubjectEnding::Ended));

    let bodies = receive_bodies(&bodies);
    assert_eq!(bodies.len(), 3, "输入一次驱动一轮，加上开场轮一共三轮");
    assert!(
        !bodies[0].contains("宿主第二段"),
        "开场轮不得带上还没给的输入"
    );
    assert!(
        !bodies[1].contains("宿主第二段"),
        "第 2 轮是纯正文那一轮，宿主输入还没被交出去: {}",
        redacted(&bodies[1])
    );
    assert_eq!(
        bodies[2].matches("宿主第二段").count(),
        1,
        "显式输入在它真正启用的那一轮里恰好出现一次"
    );
    assert!(
        bodies[2].contains("展开好了，等你给下一段"),
        "上一轮正文按协议作为历史回传"
    );
    assert!(
        bodies[1].contains("\"role\":\"tool\""),
        "行动的真相按工具回执回到请求里: {}",
        redacted(&bodies[1])
    );
    assert!(
        bodies
            .iter()
            .all(|body| !body.contains("不该被读到的第三段")),
        "收场之后宿主循环不得再读下一行"
    );
    assert!(
        std::fs::read_to_string(env_root.join("trace.ndjson"))
            .expect("读经历存储")
            .contains("model-response"),
        "模型响应是**语义明确**的顶层记录，不借注入伪装"
    );
}

/// 一个空行是**显式继续**：不给新内容，但仍然驱动下一轮。
#[test]
fn an_empty_line_is_an_explicit_continue() {
    let temp = TempDir::new("continue");
    let mut environment = environment(&temp);

    let (endpoint, bodies) = spawn_reply_server(vec![text_reply("等"), finish_reply()]);
    let mut model = loopback_model(&endpoint);
    // 一个空行 → 显式继续；后面那一行**不该被读到**。
    let mut host: &[u8] = "\n后一行不该被读到\n".as_bytes();
    let ending = run_host_session_in(
        &mut environment,
        &manual_run(&temp, "run-continue", ManualInput::Empty),
        RunInput::empty(),
        &mut host,
        &mut model,
    )
    .expect("循环本身不该失败");

    assert_eq!(ending, ManualEnding::Subject(SubjectEnding::Ended));
    let bodies = receive_bodies(&bodies);
    assert_eq!(bodies.len(), 2, "显式继续驱动了一轮，没有多出来的一次");
    assert!(
        !bodies[1].contains("后一行不该被读到"),
        "收场之后不得再读宿主输入"
    );
}

/// EOF 之后活动**没有**结束：如实报成等待，环境里留下一个未收尾的活动。
#[test]
fn eof_leaves_the_activity_unfinished_and_is_reported_as_waiting() {
    let temp = TempDir::new("eof");
    let env_root = temp.env_root();
    let mut environment = Environment::create(&env_root, agent_id()).expect("建立身份环境");

    let (endpoint, bodies) = spawn_reply_server(vec![text_reply("等")]);
    let mut model = loopback_model(&endpoint);
    // 空流：第一轮之后立刻 EOF。
    let mut host: &[u8] = b"";
    let ending = run_host_session_in(
        &mut environment,
        &manual_run(&temp, "run-eof", ManualInput::Text("开场".to_string())),
        RunInput::text("开场").expect("在上限内"),
        &mut host,
        &mut model,
    )
    .expect("循环本身不该失败");

    assert_eq!(ending, ManualEnding::Unsettled, "停在等待，不假装收场");
    assert_eq!(ending.exit_code(), EXIT_UNSETTLED);
    assert_ne!(ending.exit_code(), 0, "等待绝不能报成 0");
    // 它与主体自己选择的 `waiting` 收场是两件不同的事。
    assert_ne!(ending, ManualEnding::Subject(SubjectEnding::Waiting));
    assert_eq!(receive_bodies(&bodies).len(), 1, "EOF 之后不得再驱动一次");
    // 后果如实留在环境里：下一次运行会按**未收尾**处理，而不是"上一次完成了"。
    match environment.admission().expect("可以判定准入") {
        Admission::Unfinished { run } => assert_eq!(run.as_str(), "run-eof"),
        other => panic!("EOF 之后必须留下未收尾痕迹，却得到 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 回环服务与断言夹具
// ---------------------------------------------------------------------------

/// 一次纯正文应答（没有任何工具调用）。
fn text_reply(content: &str) -> String {
    let mut body = String::from("{\"choices\":[{\"message\":{\"content\":");
    body.push_str(&quote(content));
    body.push_str("},\"finish_reason\":\"stop\"}]}");
    body
}

/// 一次「调用某个工具」的供应商应答：`arguments` 是**字符串里的 JSON**，`finish_reason` 属于 choice。
fn tool_call_reply(call_id: &str, name: &str, arguments: &str) -> String {
    let mut body = String::from("{\"choices\":[{\"message\":{\"tool_calls\":[{\"id\":");
    body.push_str(&quote(call_id));
    body.push_str(",\"type\":\"function\",\"function\":{\"name\":");
    body.push_str(&quote(name));
    body.push_str(",\"arguments\":");
    body.push_str(&quote(arguments));
    body.push_str("}}]},\"finish_reason\":\"tool_calls\"}]}");
    body
}

/// 一次收场应答：调用结束类工具（**不带参数**——2026-10-10 裁决：结束了就是结束了）。
fn finish_reply() -> String {
    tool_call_reply("call-finish", "finish_run", "{}")
}

fn quote(text: &str) -> String {
    serde_json::to_string(text).expect("可以序列化")
}

fn loopback_model(endpoint: &str) -> DeepSeekModel {
    DeepSeekModel::at_endpoint(SENTINEL_KEY, "fixture-model", endpoint.to_string())
        .expect("可以装配适配层")
}

/// 按脚本应答的回环服务：返回（端点、全部请求体的接收端）。
///
/// 脚本用完之后服务**继续观察**一小段时间，任何多出来的连接都会被标成 `EXTRA:` 一起发回来：
/// 那个标记就是"零自动重试、等待期间零新请求"的证据。
fn spawn_reply_server(replies: Vec<String>) -> (String, mpsc::Receiver<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("可以绑定回环端口");
    let port = listener.local_addr().expect("有地址").port();
    let (sender, receiver) = mpsc::channel();

    std::thread::spawn(move || {
        let mut bodies = Vec::new();
        for reply in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            bodies.push(read_body(&mut stream));
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.len(),
                reply
            );
            let _ = stream.flush();
        }
        // 脚本用完之后继续观察：任何多出来的连接都会被算进去。
        listener.set_nonblocking(true).expect("设成非阻塞");
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
        while std::time::Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    bodies.push(format!("EXTRA:{}", read_body(&mut stream)));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(_) => break,
            }
        }
        let _ = sender.send(bodies);
    });

    (
        format!("http://127.0.0.1:{port}/chat/completions"),
        receiver,
    )
}

fn read_body(stream: &mut std::net::TcpStream) -> String {
    let mut reader = BufReader::new(stream.try_clone().expect("克隆流"));
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        if line.to_ascii_lowercase().starts_with("content-length:") {
            length = line
                .split(':')
                .nth(1)
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0);
        }
        if line == "\r\n" {
            break;
        }
    }
    let mut buffer = vec![0u8; length];
    if reader.read_exact(&mut buffer).is_ok() {
        return String::from_utf8_lossy(&buffer).into_owned();
    }
    String::new()
}

/// 取回全部请求体；出现"多出来的连接"就直接失败。
fn receive_bodies(receiver: &mpsc::Receiver<Vec<String>>) -> Vec<String> {
    let bodies = receiver
        .recv_timeout(std::time::Duration::from_secs(15))
        .expect("应当收到请求");
    for body in &bodies {
        assert!(
            !body.starts_with("EXTRA:"),
            "出现了脚本之外的多余请求（等待不得自动再驱动、失败不得自动重试）: {body}"
        );
    }
    bodies
}

fn parse_body(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).expect("请求体是合法 JSON")
}

/// 一封请求体里那一块**卡片**的正文（标签行与数据围栏都剥掉；`None` = 这一封里没有卡片）。
///
/// 请求体是**原始 JSON 文本**（换行在里面是 `\n` 两个字面字符），所以先解析出真正文，
/// 再按**真换行**看形状：这样"一行只有一个计数"才是真的在判形状，而不是在判转义。
fn card_block(raw: &str) -> Option<String> {
    let wire = parse_body(raw);
    let text = wire["messages"]
        .as_array()?
        .iter()
        .filter_map(|message| message["content"].as_str())
        .find(|content| content.contains(EVENT_CARDS_LABEL))?;
    let block = text
        .split(EVENT_CARDS_LABEL)
        .nth(1)?
        .split(">>>")
        .next()?
        .split_once('\n')
        .map(|(_, rest)| rest)?;
    Some(
        block
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with("<<<") && !line.starts_with(">>>"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// 请求里的第一个工具调用（助手帧里的那一个）。
fn first_call(wire: &serde_json::Value) -> &serde_json::Value {
    wire["messages"]
        .as_array()
        .expect("messages 是数组")
        .iter()
        .find_map(|message| {
            message
                .get("tool_calls")
                .and_then(|calls| calls.as_array())
                .and_then(|calls| calls.first())
        })
        .expect("请求里必须有一次工具调用")
}

/// 紧跟在助手帧之后的那条工具回执。
fn paired_receipt(wire: &serde_json::Value) -> Option<&serde_json::Value> {
    let messages = wire["messages"].as_array()?;
    let index = messages.iter().position(|message| {
        message
            .get("tool_calls")
            .and_then(|calls| calls.as_array())
            .is_some_and(|calls| !calls.is_empty())
    })?;
    messages
        .get(index + 1)
        .filter(|next| next["role"] == "tool")
}

/// 断言"助手帧的调用标识与它的回执**相邻**"这件事在真实请求体里成立。
///
/// `expected_state` 是回执里必须出现的那个词：真实结果给 `success` / `failure` / `unknown`，
/// 被拒绝的请求给 `refused`（它不说结果类别，只说"未派发"）。
fn assert_paired(raw: &str, call_id: &str, name: &str, expected_state: &str) {
    let wire = parse_body(raw);
    let messages = wire["messages"].as_array().expect("messages 是数组");
    let index = messages
        .iter()
        .position(|message| {
            message
                .get("tool_calls")
                .and_then(|calls| calls.as_array())
                .is_some_and(|calls| {
                    calls
                        .iter()
                        .any(|call| call["id"].as_str() == Some(call_id))
                })
        })
        .unwrap_or_else(|| panic!("请求里必须有标识为 {call_id} 的调用: {}", redacted(raw)));
    let assistant = &messages[index];
    assert_eq!(assistant["role"], "assistant");
    assert_eq!(
        assistant["tool_calls"][0]["function"]["name"], name,
        "工具名就是能力标识"
    );
    let receipt = &messages[index + 1];
    assert_eq!(
        receipt["role"],
        "tool",
        "回执必须**紧邻**它的助手帧，中间不得插入别的消息: {}",
        redacted(raw)
    );
    assert_eq!(receipt["tool_call_id"], call_id, "回执带着原调用标识");
    let content = receipt["content"].as_str().expect("回执有正文");
    if expected_state == "refused" {
        assert!(
            content.contains("dispatched = false"),
            "拒绝回执必须明说未派发: {content}"
        );
    } else {
        assert!(
            content.contains(&format!("state = {expected_state}")),
            "真实结果回执必须给出结果类别: {content}"
        );
    }
}

/// 断言失败信息里打印请求时先把凭据遮掉：测试输出不该成为泄漏渠道。
fn redacted(raw: &str) -> String {
    raw.replace(SENTINEL_KEY, "<redacted>")
}

/// 这一份用例里的"模型失败"分类：脚本用尽 = 这次响应没有履约。
///
/// 这是**测试替身**的失败，不是某个真实服务行为；它只用来把"失败分类"这条路径走通。
/// 生产路径的分类由适配层按九类如实给出。
const SCRIPTED_MODEL_FAILURE: ModelFailureClass = ModelFailureClass::ContractViolation;

/// 这一份用例里的"模型失败"（带上上面那个分类）。
fn scripted_model_failure() -> ModelError {
    ModelError {
        class: SCRIPTED_MODEL_FAILURE,
    }
}

/// **档位门控**（TASK_36）：声明了 `--terminal full` 才是「主体的手在房间里」；不声明 ⇒ 那个动词
/// **一个字节都不出现**（不是"在但用不了"）。对照：web 两件在两次装配里都在——证明"不在"是
/// 特定的，不是装配坏了。
#[test]
fn the_tier_declaration_is_what_puts_her_hand_in_the_room() {
    let mut observed: Vec<String> = Vec::new();
    for declared in [false, true] {
        let temp = TempDir::new(if declared {
            "tier-declared"
        } else {
            "tier-absent"
        });
        let env_root = temp.env_root();
        let mut environment = Environment::create(&env_root, agent_id()).expect("建立身份环境");

        let (endpoint, bodies) = spawn_reply_server(vec![finish_reply()]);
        let mut run = manual_run(&temp, "run-tier", ManualInput::Text("醒着".to_string()));
        if declared {
            // 档位就是授权本体：声明了它，这只手才在。
            run.declarations.terminal = Some(celira_agent::terminal_app::TerminalAccess::new(
                celira_agent::terminal_app::TerminalTier::Full,
                Vec::new(),
            ));
        }
        let mut model = loopback_model(&endpoint);
        let mut host: &[u8] = b"";
        run_host_session_in(
            &mut environment,
            &run,
            RunInput::text("醒着").expect("在上限内"),
            &mut host,
            &mut model,
        )
        .expect("循环本身不该失败");

        let bodies = receive_bodies(&bodies);
        assert_eq!(bodies.len(), 1, "一次收场只驱动一轮");
        observed.push(bodies[0].clone());
    }

    let absent = &observed[0];
    let present = &observed[1];
    // TASK_68：档位仍然决定**能力面**（RunScope 与能力模型一字未改），但它不再改 `tools` 前缀——
    // 两种档位下头部都是同样两件。所以这一格在 wire 层只能说"头部照旧恒定"，
    // "档位真的把手放进了房间"改由**注入的目录**观测（TASK_68 P2）。
    assert!(
        !absent.contains("\"terminal_run\"") && !present.contains("\"terminal_run\""),
        "两种档位下，动作都不直连头部: {} / {}",
        redacted(absent),
        redacted(present)
    );
    assert!(
        present.contains("\"name\":\"invoke\"") && present.contains("\"name\":\"finish_run\""),
        "声明了档位 ⇒ 头部照旧是那两件: {}",
        redacted(present)
    );
    for body in [absent, present] {
        // TASK_68：两条都不再直连头部（web 的能力仍在，只是经入口路由）。
        assert!(
            !body.contains("\"web_fetch\"") && !body.contains("\"web_search\""),
            "对照：web 窗的能力不直连头部（两种档位都一样）: {}",
            redacted(body)
        );
    }
}
