//! 交接（handoff）②：**交付**——写了 ⇒ 下一段开场摆出来 ⇒ 再下一段不摆。
//!
//! 判据一句话（`RUNTIME_FOUNDATION_DESIGN.md` §B4.1 的交付那一步）：**交付过 ⟺ 紧邻上一次活动的
//! 开场里摆过它**。零新事实：那一次注入本来就记在账里（`trace.ndjson` 里那条 `stage=run-start`
//! 的 `blocks[].text`），装配期只把它读回来比一次正文——**零新写者、零新游标**。
//!
//! 这个文件判**交付这一层**（跨两段活动的行为）；三态与"一次播放"本身的判据在 `src/handoff.rs`，
//! "摆在哪一格"的判据在 `src/pins_board.rs`，那一格读回来的判据在 `src/store/environment.rs`。
//!
//! ⚠ **写动词是另一刀**：这里"她写下交接"走的是那一件的公开面（`handoff::write_from_subject`），
//! 因为今天她的动词面上还没有 `write_handoff`——本刀只接**交付**那一半。
//!
//! ⚠ **不声称**：没有跑真模型（模型是脚本替身）；没有跑真机宿主。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use celira_agent::clock::ClockHandle;
use celira_agent::composition::{RunSession, SessionDeclarations, SessionRequest};
use celira_agent::context_reading::ContextReading;
use celira_agent::core::{
    InjectionStage, Limits, ModelError, ModelOutput, ModelPort, RunInput, TraceEntry, View,
};
use celira_agent::foundation::{AgentId, RunId};
use celira_agent::handoff::{HANDOFF_FILE, HANDOFF_LABEL};
use celira_agent::pins_board::PINS_BOARD_LABEL;
use celira_agent::store::{Environment, TracePayload};

const AGENT: &str = "agent-handoff";
const RUN_ONE: &str = "run-handoff-1";
const RUN_TWO: &str = "run-handoff-2";
const RUN_THREE: &str = "run-handoff-3";

/// 一块注入内容：标签 + 正文。
type Block = (String, String);

/// 轨迹里一条注入条目：阶段、轮次（Run 级没有）、块。
type Injection = (InjectionStage, Option<u32>, Vec<Block>);

/// 每次模型调用看到的（轮次，轨迹里 injections 的（阶段，块））。
///
/// 起个名字而不是把嵌套类型写在字段上：clippy 的 `type_complexity` 在本仓库是**红门禁**
/// （`-D warnings`），而那一串括号确实读不出意思。
type SeenInjections = Vec<(u32, Vec<(InjectionStage, Vec<Block>)>)>;

// ---------------------------------------------------------------------------
// 自建的独立测试目录（绝不指向桌面 / 家目录 / 仓库根）
// ---------------------------------------------------------------------------

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        // 目录名**必须带进程号**：测试二进制是一份一个进程，而计数器只在进程内唯一 ⇒
        // 不带进程号的两份并行会算出同一批路径、互相删目录。
        let mut path = std::env::temp_dir();
        path.push(format!("celira-handoff-{}-{label}-{n}", std::process::id()));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("清掉同名残留目录");
        }
        std::fs::create_dir_all(&path).expect("建立测试目录");
        Self { path }
    }

    fn env_root(&self) -> PathBuf {
        self.path.join("env")
    }

    fn materials(&self) -> PathBuf {
        let materials = self.path.join("materials");
        std::fs::create_dir_all(&materials).expect("建资料目录");
        materials
    }

    fn environment(&self) -> Environment {
        Environment::create(&self.env_root(), agent_id()).expect("建立身份环境")
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

/// 用例装配用的**时间来源**：固定钟（离线、可逐字节断言）。
fn fixed_clock() -> ClockHandle {
    ClockHandle::fixed_at_unix_seconds(0)
}

/// 用例声明的上下文窗口（宿主声明，必须正整数）。
const CONTEXT_WINDOW: u32 = 1_000;

/// 脚本模型：按序交出事先写好的响应，并把每一次调用看到的轨迹按顺序记下来。
struct ScriptedModel {
    steps: VecDeque<ModelOutput>,
    seen: SeenInjections,
}

impl ScriptedModel {
    fn new(steps: Vec<ModelOutput>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            seen: Vec::new(),
        }
    }
}

impl ModelPort for ScriptedModel {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        let injections = view
            .trace()
            .iter()
            .filter_map(|entry| match entry {
                TraceEntry::Injection { stage, batch, .. } => Some((
                    *stage,
                    batch
                        .blocks()
                        .iter()
                        .map(|block| (block.label().to_string(), block.text().to_string()))
                        .collect::<Vec<_>>(),
                )),
                TraceEntry::Refused { .. }
                | TraceEntry::Observation(_)
                | TraceEntry::ModelResponse(_) => None,
            })
            .collect();
        self.seen.push((view.turn().number(), injections));
        self.steps.pop_front().ok_or(ModelError {
            class: celira_agent::core::ModelFailureClass::ContractViolation,
        })
    }
}

/// 打开一次会话（生产入口，只有时间来源换成了固定钟）。
fn open<'a>(dir: &TempDir, environment: &'a mut Environment, run: &str) -> RunSession<'a> {
    let materials = dir.materials();
    let declared = agent_id();
    RunSession::open(
        &materials,
        environment,
        SessionRequest {
            declared_agent: &declared,
            run: RunId::new(run),
            limits: Limits::new(4),
            input: RunInput::text("开场输入").expect("在上限内"),
            clock: fixed_clock(),
            context_window: CONTEXT_WINDOW,
            reading: ContextReading::default(),
            declarations: SessionDeclarations::default(),
        },
    )
    .expect("会话可以打开")
}

/// 驱动一段：她只说一句"等"（活动不收场，注入一轮一条）。
fn drive_once(session: &mut RunSession<'_>) {
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());
}

/// 轨迹里全部 injection 条目，按发生顺序：`(阶段, 轮次, 块)`。
fn injections(run: &RunSession<'_>) -> Vec<Injection> {
    run.trace()
        .iter()
        .filter_map(|entry| match entry {
            TraceEntry::Injection { stage, turn, batch } => Some((
                *stage,
                turn.map(|turn| turn.number()),
                batch
                    .blocks()
                    .iter()
                    .map(|block| (block.label().to_string(), block.text().to_string()))
                    .collect(),
            )),
            TraceEntry::Refused { .. }
            | TraceEntry::Observation(_)
            | TraceEntry::ModelResponse(_) => None,
        })
        .collect()
}

/// 一批块里的交接块（**从主体那一侧的轨迹里取**，不是从本件内部取）。
fn handoff_blocks(blocks: &[Block]) -> Vec<Block> {
    blocks
        .iter()
        .filter(|(label, _)| label == HANDOFF_LABEL)
        .cloned()
        .collect()
}

/// 一段活动开场那一拍的块（第 0 条注入就是开场那一拍）。
fn opening_blocks(injections: &[Injection]) -> &[Block] {
    let (stage, turn, blocks) = &injections[0];
    assert_eq!(*stage, InjectionStage::RunStart, "第一条就是开场那一拍");
    assert!(turn.is_none(), "开场那一拍没有轮次");
    blocks
}

/// 环境根里现在有哪些**条目名**（用来判"开场没有新落一个文件"）。
fn env_root_entries(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root)
        .expect("读环境根")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names
}

// ---------------------------------------------------------------------------
// 判据：写了 ⇒ 下一段摆出来 ⇒ 再下一段不摆
// ---------------------------------------------------------------------------

/// **写了 ⇒ 下一段开场摆出来（原文，一个字不加）⇒ 再下一段不摆。**
///
/// 同时钉住三件事：① 没写过 ⇒ 开场**一个字都不说、连标签都不出现**；② 摆出来的就是原文；
/// ③ 它**只在开场那一拍**（之后每一轮都不重摆——与组件区其它块同款）。
#[test]
fn a_handoff_written_in_one_activity_is_shown_once_at_the_next_opening() {
    let dir = TempDir::new("delivered-once");
    let mut environment = dir.environment();
    let text = "我干到第三步：正在等对面回话。下一段接着问。";

    // **第一段**：开场里还没有交接（她还没写过）——写发生在这一段**里面**。
    {
        let mut session = open(&dir, &mut environment, RUN_ONE);
        drive_once(&mut session);
        let openings = injections(&session);
        assert!(
            handoff_blocks(opening_blocks(&openings)).is_empty(),
            "还没写过 ⇒ 一个字都不说、连标签都不出现: {openings:#?}"
        );
        // 她在这一段里写下交接（写动词是另一刀，这里走那一件的公开面）。
        celira_agent::handoff::write_from_subject(&dir.env_root(), text).expect("写下交接");
    }

    // **第二段**：开场摆出原文；此后每一轮都不再摆。
    {
        let mut session = open(&dir, &mut environment, RUN_TWO);
        drive_once(&mut session);
        drive_once(&mut session);
        let openings = injections(&session);
        let found = handoff_blocks(opening_blocks(&openings));
        assert_eq!(found.len(), 1, "写了 ⇒ 下一段开场摆出来: {openings:#?}");
        assert_eq!(found[0].1, text, "原文摆出，前后不加东西");
        for (index, (_, _, blocks)) in openings.iter().enumerate().skip(1) {
            assert!(
                handoff_blocks(blocks).is_empty(),
                "第 {index} 条注入里不该再有它（快照不重摆）: {blocks:?}"
            );
        }
    }

    // **第三段**：交付过 ⇒ 不摆；而组件区那一格照旧在场（pin 投影每段都摆）。
    {
        let mut session = open(&dir, &mut environment, RUN_THREE);
        drive_once(&mut session);
        let openings = injections(&session);
        assert!(
            handoff_blocks(opening_blocks(&openings)).is_empty(),
            "交付过就不再摆: {openings:#?}"
        );
        assert!(
            opening_blocks(&openings)
                .iter()
                .any(|(label, _)| label == PINS_BOARD_LABEL),
            "那一格照旧在场（不是整格消失）: {openings:#?}"
        );
    }
}

/// **再写 ⇒ 新的那一份下一次开场照样摆**（B4.1 的「再写」那一支）。
///
/// 这一条钉的是判据的**身份**那一半：交付判据比的是**正文**，不是"上一次摆过一块交接没有"。
/// 后者会把重写后的新的一份读成"已经交付过" ⇒ 那一份**永远不摆**。
#[test]
fn a_rewritten_handoff_is_shown_again_at_the_next_opening() {
    let dir = TempDir::new("rewritten");
    let mut environment = dir.environment();
    let first = "第一份：接着做 X。";
    let second = "第二份：X 做完了，接着做 Y。";

    // 第一段：写下第一份。
    {
        let mut session = open(&dir, &mut environment, RUN_ONE);
        drive_once(&mut session);
        celira_agent::handoff::write_from_subject(&dir.env_root(), first).expect("写第一份");
    }

    // 第二段：开场摆出第一份；她在这一段里改写成第二份。
    {
        let mut session = open(&dir, &mut environment, RUN_TWO);
        drive_once(&mut session);
        let openings = injections(&session);
        let found = handoff_blocks(opening_blocks(&openings));
        assert_eq!(found.len(), 1, "第一份摆出来: {openings:#?}");
        assert_eq!(found[0].1, first);
        celira_agent::handoff::write_from_subject(&dir.env_root(), second).expect("改写");
    }

    // 第三段：**新的那一份照样摆**（不是"已经交付过 ⇒ 什么都不摆"）。
    {
        let mut session = open(&dir, &mut environment, RUN_THREE);
        drive_once(&mut session);
        let openings = injections(&session);
        let found = handoff_blocks(opening_blocks(&openings));
        assert_eq!(found.len(), 1, "改写之后是新的一份: {openings:#?}");
        assert_eq!(found[0].1, second);
    }
}

/// **交付判据读的是账，不是新落的一格**（零新事实、零新写者）。
///
/// 三件事：① 装配期要的那一份事实**就是**上一段开场那条注入本身（`run-start` 的
/// `blocks[].text`，从 `trace.ndjson` 读回来）；② 这一份原文在账里**只**出现那一次
/// （没有"另写一条已交付"）；③ 环境根里没有为交付另落一格，那一份自己也没被改过
/// （没有游标、没有"已交付"标记）。
#[test]
fn the_delivery_judgement_reads_the_ledger_and_writes_nothing_new() {
    let dir = TempDir::new("zero-new-facts");
    let mut environment = dir.environment();
    let text = "零新事实：这一份只从账里认出来。";

    // 第一段：写下交接，让它在这一段之后的下一次开场被摆出来。
    let shown_at_run_one = {
        let mut session = open(&dir, &mut environment, RUN_ONE);
        drive_once(&mut session);
        celira_agent::handoff::write_from_subject(&dir.env_root(), text).expect("写下交接");
        // 这一段自己的开场里没有它（写在这一段里面）。
        let openings = injections(&session);
        assert!(handoff_blocks(opening_blocks(&openings)).is_empty());
        text.to_string()
    };

    // **开场读数**：上一次活动的开场摆了哪几块 —— 那就是账里那条注入原样带回来的。
    let readings = environment.opening_readings().expect("开场读数");
    let found: Vec<&Block> = readings
        .last_opening_blocks
        .iter()
        .filter(|(label, _)| label == HANDOFF_LABEL)
        .collect();
    assert!(
        found.is_empty(),
        "上一段（第一段）开场没摆过交接: {:?}",
        readings.last_opening_blocks
    );
    // 账里那一批块与"主体那一侧看到的"是同一份事实（标签逐字节相同）。
    assert!(
        readings
            .last_opening_blocks
            .iter()
            .any(|(label, _)| label == PINS_BOARD_LABEL),
        "开场那一批块就是账里那一条: {:?}",
        readings.last_opening_blocks
    );

    // 第二段：开场摆出它 ⇒ 这一次**该**被认成"交付过"。
    {
        let mut session = open(&dir, &mut environment, RUN_TWO);
        drive_once(&mut session);
        let openings = injections(&session);
        let found = handoff_blocks(opening_blocks(&openings));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, shown_at_run_one);
    }

    // **交付没有新落一条记录**：这一份原文在账里**只**出现在它被摆出去的那条开场注入里
    // （不是"另写一条已交付"，也不是"在里面记一笔"）。
    let ledger = std::fs::read_to_string(dir.env_root().join("trace.ndjson")).expect("读经历账本");
    let carrying: Vec<&str> = ledger
        .lines()
        .filter(|line| line.contains(shown_at_run_one.as_str()))
        .collect();
    assert_eq!(
        carrying.len(),
        1,
        "那一份在账里只该出现一次（就是摆出它的那条开场注入）"
    );
    assert!(
        carrying[0].contains("\"stage\":\"run-start\""),
        "而且就是开场那条注入: {}",
        carrying[0]
    );

    // **环境根里也没有多出一格**：交付没有游标文件、那一份自己一个字节没被改。
    let entries = env_root_entries(&dir.env_root());
    let handoff_named: Vec<&String> = entries
        .iter()
        .filter(|name| name.contains("handoff"))
        .collect();
    assert_eq!(
        handoff_named.len(),
        1,
        "交付不在环境根里另落一格（没有游标）: {entries:?}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.env_root().join(HANDOFF_FILE)).expect("读那一份"),
        shown_at_run_one,
        "那一份自己一个字节没被改（没有在里面记'已交付'）"
    );

    // 交付判据要的那一格现在**就在账里**：上一段开场摆过的那一份，原文逐字节在。
    let readings = environment.opening_readings().expect("开场读数");
    let found: Vec<&Block> = readings
        .last_opening_blocks
        .iter()
        .filter(|(label, _)| label == HANDOFF_LABEL)
        .collect();
    assert_eq!(found.len(), 1, "上一段开场摆过的那一块在账里: {readings:?}");
    assert_eq!(found[0].1, shown_at_run_one, "原文逐字节");
}

/// **读不到 ⇒ 如实说，而且不冒充"交付过"**（三态不许压成两态）。
///
/// `handoff` 那个位置被一个目录占着 ⇒ 读失败 ⇒ 开场摆出"这一次没读到"，**下一段照样说**
/// （它一次也没有被交付过：没读到的那一句不是"她的字"）。
#[test]
fn an_unreadable_handoff_is_stated_and_never_counted_as_delivered() {
    let dir = TempDir::new("unreadable");
    let mut environment = dir.environment();
    std::fs::create_dir_all(dir.env_root().join(HANDOFF_FILE)).expect("造目录顶替那一份");

    for run in [RUN_ONE, RUN_TWO] {
        let mut session = open(&dir, &mut environment, run);
        drive_once(&mut session);
        let openings = injections(&session);
        let found = handoff_blocks(opening_blocks(&openings));
        assert_eq!(found.len(), 1, "读不到要如实说读不到: {openings:#?}");
        assert!(
            found[0].1.contains("这一次没读到"),
            "不冒充'主体没有交接': {}",
            found[0].1
        );
    }
}

/// **封口之后看不见那一次开场 ⇒ 至多重播一次，绝不静默跳过**（B4.2 的方向）。
///
/// 活动段封口是 1 MiB 一次；封口之后，上一次活动的开场落在上一卷里，而开场读数只读活动段
/// ⇒ 那一格为空 ⇒ 装配点读作"没交付过" ⇒ 这一段**再摆一次**。
/// **重复可容忍、丢失不可接受**——这一条钉的正是那个方向（不是"读不到就跳过"）。
#[test]
fn an_opening_that_is_sealed_away_is_repeated_rather_than_skipped() {
    let dir = TempDir::new("sealed-repeat");
    let mut environment = dir.environment();
    let text = "封口之后那一份还会再摆一次。";
    // 写在第一段开场**之前** ⇒ 第一段开场就摆出它。
    celira_agent::handoff::write_from_subject(&dir.env_root(), text).expect("写下交接");
    {
        let mut session = open(&dir, &mut environment, RUN_ONE);
        drive_once(&mut session);
        let openings = injections(&session);
        let found = handoff_blocks(opening_blocks(&openings));
        assert_eq!(found.len(), 1, "第一段开场摆出它: {openings:#?}");
        assert_eq!(found[0].1, text);
    }

    // 把活动段顶过封口线：那一段经历（含它开场那条注入）搬进上一卷 ⇒ 开场读数看不见它。
    let mut appended = 0;
    while !dir.env_root().join("trace.parts").exists() {
        environment
            .append_trace(
                &RunId::new(RUN_ONE),
                TracePayload::RunInput {
                    text: "填".repeat(100_000),
                },
            )
            .expect("追加填充记录");
        appended += 1;
        assert!(appended < 40, "填充没把活动段顶过封口线");
    }

    // 第二段：那一份**再摆一次**（看不见 ⇒ 不假装交付过）。
    {
        let mut session = open(&dir, &mut environment, RUN_TWO);
        drive_once(&mut session);
        let openings = injections(&session);
        let found = handoff_blocks(opening_blocks(&openings));
        assert_eq!(found.len(), 1, "看不见就重播，不静默跳过: {openings:#?}");
        assert_eq!(found[0].1, text, "重播的是同一份原文");
    }
}
