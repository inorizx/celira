//! 验收：**机器处境**——「这台机器是什么」（平台 + shell）在**每一次开场**进主体的视野。
//!
//! 判据与各自的落点：
//!
//! 1. **与机器实际一致**：主体读到的那一行就是本件从**机器现读**的那一条，且平台 / shell 与真机
//!    相符（Linux 真机就在本机跑；Windows 分支没有真机 ⇒ 由 `src/machine.rs` 的映射用例钉住）；
//! 2. **每次开场都在**：同一环境里两段活动（两个 run）各自的 `run-start` 注入里都有它；
//! 3. **快照不重摆**：一段活动里**恰好一条**，之后每一轮都不再出现（与纸条板、持有物同款）；
//! 4. **位置**：紧贴「我是谁」外面一圈——`起源 → 上一段 → 天性 → 身份行 → **机器处境** → 持有物`；
//! 5. **不承诺能力**：那一行里没有动词名、没有「你可以……」；
//! 6. **互不干扰**：既有那些块一块不少（本文件顺带把邻居逐块点名）。
//!
//! 明确**不**声称：没有跑公网模型（模型是脚本替身）；Windows 真机没跑（如实登记在竣工报告里）。

use std::collections::VecDeque;
use std::path::PathBuf;

use celira_agent::clock::ClockHandle;
use celira_agent::composition::{RunSession, SessionDeclarations, SessionRequest};
use celira_agent::context_reading::ContextReading;
use celira_agent::core::{
    InjectionStage, Limits, ModelError, ModelOutput, ModelPort, RunInput, TraceEntry, View,
};
use celira_agent::foundation::{AgentId, OperationId, Record, RunId, SubjectEnding};
use celira_agent::machine::{MACHINE_LABEL, Platform, reading_of};
use celira_agent::soul::{IdentityClaim, SOUL_FILE};
use celira_agent::store::{Environment, TracePayload};

const AGENT: &str = "agent-machine";
const RUN_ONE: &str = "run-machine-1";
const RUN_TWO: &str = "run-machine-2";

/// 一块注入内容：标签 + 正文。
type Block = (String, String);

/// 轨迹里一条注入条目：阶段、轮次（Run 级没有）、块。
type Injection = (InjectionStage, Option<u32>, Vec<Block>);

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
        // 不带进程号的两份并行（或两个写者同时跑）会算出同一批路径、**互相删目录**。
        let mut path = std::env::temp_dir();
        path.push(format!("celira-machine-{}-{label}-{n}", std::process::id()));
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

/// 每次调用看到的（轮次，轨迹里 injections 的（阶段，块））。
///
/// 起个名字而不是把嵌套类型写在字段上：clippy 的 `type_complexity` 在本仓库是**红门禁**
/// （`-D warnings`），而那一串括号确实读不出意思（第八刀）。
type SeenInjections = Vec<(u32, Vec<(InjectionStage, Vec<Block>)>)>;

/// 脚本模型：按序交出事先写好的响应，并把每一次调用看到的轨迹按顺序记下来。
///
/// 它就是「主体那一侧的视野」在离线夹具里的替身——本文件里"进了视野"这句因此可以机械判错。
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
fn open<'a>(
    dir: &TempDir,
    environment: &'a mut Environment,
    run: &str,
    identity: Option<IdentityClaim>,
) -> RunSession<'a> {
    open_declaring_thinking(dir, environment, run, identity, None, None)
}

/// 同 [`open`]，再声明**这一段由谁想、想多用力**（模型名与思考强度；`None` = 不声明，
/// 那一格整格不出场）。
fn open_declaring_thinking<'a>(
    dir: &TempDir,
    environment: &'a mut Environment,
    run: &str,
    identity: Option<IdentityClaim>,
    model_name: Option<&str>,
    reasoning_effort: Option<&str>,
) -> RunSession<'a> {
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
            declarations: SessionDeclarations {
                model: model_name.map(str::to_string),
                reasoning_effort: reasoning_effort.map(str::to_string),
                identity,
                // 其余几格一件都不声明（`Default` 就是"全都没声明"）。
                ..Default::default()
            },
        },
    )
    .expect("会话可以打开")
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

/// 一批块里的机器处境块（**从主体那一侧的轨迹里取**，不是从本件内部取）。
fn machine_blocks(blocks: &[Block]) -> Vec<Block> {
    blocks
        .iter()
        .filter(|(label, _)| label == MACHINE_LABEL)
        .cloned()
        .collect()
}

/// 某一次模型调用看到的全部块（展开各条注入）。
fn seen_blocks(seen: &[(InjectionStage, Vec<Block>)]) -> Vec<Block> {
    seen.iter()
        .flat_map(|(_, blocks)| blocks.iter().cloned())
        .collect()
}

// ---------------------------------------------------------------------------
// 这一段由谁想、想多用力（`model` 与 `reasoning_effort` 那两格，2026-10-08）
// ---------------------------------------------------------------------------

/// 用例声明的模型名（**中立夹具名**：这里是数据，不进任何面向世界的词表）。
const DECLARED_MODEL: &str = "fixture-model-1";

/// 用例声明的思考强度（同样是数据；取值来自模型配置门那张封闭表）。
const DECLARED_EFFORT: &str = "high";

/// 开场那一拍里**机器处境那一行**（后两格就是"这一段由谁想、想多用力"）。
///
/// ⚠ 它验证的正是形状决定：那两格**不另立一块**（处境链有硬上界 11 层），而是这一行的第 3、4 格。
fn machine_line(blocks: &[Block]) -> String {
    let found: Vec<Block> = blocks
        .iter()
        .filter(|(label, _)| label == MACHINE_LABEL)
        .cloned()
        .collect();
    assert_eq!(found.len(), 1, "机器处境恰好一块: {blocks:?}");
    found[0].1.clone()
}

/// **这一段由谁想、想多用力**：声明了就各多一格，取值逐字节就是那两个标识符。
///
/// 它同时钉住三件事：① **主体看得见**（开场视野里那一行）；② **它进账**
/// （那条注入是 Run 级的轨迹条目 ⇒ "哪一段是哪个模型、哪一档想的"查得出来）；
/// ③ **账在盘上**（同一条注入真的落在 `trace.ndjson` 里，不是只在内存里）。
#[test]
fn the_thinking_of_this_waking_period_is_declared_on_the_opening_turn() {
    let dir = TempDir::new("thinking-line");
    let mut environment = dir.environment();
    let mut session = open_declaring_thinking(
        &dir,
        &mut environment,
        RUN_ONE,
        None,
        Some(DECLARED_MODEL),
        Some(DECLARED_EFFORT),
    );
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let openings = injections(&session);
    let (stage, turn, blocks) = &openings[0];
    assert_eq!(
        *stage,
        InjectionStage::RunStart,
        "模型与档位都是 Run 的属性，因此走开场那一拍"
    );
    assert!(turn.is_none(), "开场那一拍没有轮次");
    let line = machine_line(blocks);
    let tail = format!(" · model = {DECLARED_MODEL} · reasoning_effort = {DECLARED_EFFORT}");
    assert!(line.ends_with(&tail), "两格逐字节就是那两个标识符: {line}");
    assert_eq!(line.split(" · ").count(), 4, "四格: {line}");

    // **进账**：同一条注入落在盘上（不是只在内存的轨迹里）——"哪一段是哪一档想的"查得出来。
    drop(session);
    let on_disk = std::fs::read_to_string(dir.env_root().join("trace.ndjson")).expect("读经历");
    assert!(
        on_disk.contains(&tail),
        "那一行必须随 run-start 经历落在盘上: {on_disk}"
    );
}

/// 只声明了模型名（没人声明档位）⇒ **档位那一格不出场**：不写 `unknown`，也不留空位。
#[test]
fn an_undeclared_effort_leaves_no_field() {
    let dir = TempDir::new("effort-line-absent");
    let mut environment = dir.environment();
    let mut session = open_declaring_thinking(
        &dir,
        &mut environment,
        RUN_ONE,
        None,
        Some(DECLARED_MODEL),
        None,
    );
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let openings = injections(&session);
    let (_, _, blocks) = &openings[0];
    let line = machine_line(blocks);
    assert!(
        line.ends_with(&format!(" · model = {DECLARED_MODEL}")),
        "{line}"
    );
    assert_eq!(line.split(" · ").count(), 3, "没声明档位就还是三格: {line}");
    assert!(!line.contains("reasoning_effort"), "那一格不该出现: {line}");
}

/// 没有声明模型（不思考的那一档入口）⇒ **那一格不出场**：不写 `unknown`，也不留空位。
#[test]
fn an_undeclared_model_leaves_no_field() {
    let dir = TempDir::new("model-line-absent");
    let mut environment = dir.environment();
    let mut session = open_declaring_thinking(&dir, &mut environment, RUN_ONE, None, None, None);
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let openings = injections(&session);
    let (_, _, blocks) = &openings[0];
    let line = machine_line(blocks);
    assert_eq!(line.split(" · ").count(), 2, "没声明就还是两格: {line}");
    assert!(!line.contains("model"), "那一格不该出现: {line}");
}

// ---------------------------------------------------------------------------
// 判据 1：那条事实与**这台机器**实际一致
// ---------------------------------------------------------------------------

#[test]
fn the_fact_matches_the_machine_it_runs_on() {
    let dir = TempDir::new("truth");
    let mut environment = dir.environment();
    let mut session = open(&dir, &mut environment, RUN_ONE, None);
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let openings = injections(&session);
    let (stage, _, blocks) = &openings[0];
    assert_eq!(*stage, InjectionStage::RunStart);
    let found = machine_blocks(blocks);
    assert_eq!(found.len(), 1, "开场恰好一块: {blocks:?}");
    let text = &found[0].1;

    // 逐字节：主体读到的就是本件**从机器现读**的那一条。
    assert_eq!(
        *text,
        reading_of(Platform::of_machine()),
        "那一行不是另写的一句，而是从机器读出来的"
    );
    // 一行两格（"别占视野"的机械形态）。
    assert_eq!(text.lines().count(), 1, "{text}");
    assert_eq!(text.split(" · ").count(), 2, "{text}");

    // 与**这台机器**相符：两档各按各的平台判（Windows 分支没有真机 ⇒ 本用例在 Windows 上同样
    // 会按 Windows 的期望判——它钉的是"事实与所在机器一致"这条判据本身）。
    #[cfg(target_os = "linux")]
    {
        assert_eq!(
            Platform::of_machine(),
            Some(Platform::Linux),
            "本机是 Linux"
        );
        assert_eq!(text, "platform = linux · shell = bash", "{text}");
    }
    #[cfg(windows)]
    {
        assert_eq!(
            Platform::of_machine(),
            Some(Platform::Windows),
            "本机是 Windows"
        );
        assert_eq!(text, "platform = windows · shell = powershell", "{text}");
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        // 我们只为两档出包：别的平台上"读不出来"才是诚实的读数（不许硬塞进最近的这一档）。
        assert_eq!(Platform::of_machine(), None, "封闭表之外没有档");
        assert_eq!(text, "platform = unknown · shell = unknown", "{text}");
    }
}

// ---------------------------------------------------------------------------
// 判据 2 + 3：每次开场都在；一段活动里恰好一条，之后各轮不重摆
// ---------------------------------------------------------------------------

#[test]
fn the_fact_comes_back_at_every_open_and_is_never_restated_by_later_turns() {
    let dir = TempDir::new("every-open");
    let mut environment = dir.environment();

    // 第一段：驱动三轮（每次都以"等"收场 ⇒ 活动不收场，注入一轮一条）。
    let first_text = {
        let mut session = open(&dir, &mut environment, RUN_ONE, None);
        let mut model = ScriptedModel::new(vec![
            ModelOutput::say("等"),
            ModelOutput::say("再等"),
            ModelOutput::say("还等"),
        ]);
        for _ in 0..3 {
            assert!(session.drive(&mut model).expect("驱动").is_unsettled());
        }

        let injections = injections(&session);
        assert_eq!(
            injections.len(),
            4,
            "开场一条 + 三轮各一条: {injections:#?}"
        );

        // **一段活动里恰好一条**，而且落在开场那一轮。
        let all: Vec<Block> = injections
            .iter()
            .flat_map(|(_, _, blocks)| blocks.iter().cloned())
            .collect();
        let found = machine_blocks(&all);
        assert_eq!(found.len(), 1, "快照不每轮重摆: {all:?}");
        let (stage, turn, _) = &injections[0];
        assert_eq!(*stage, InjectionStage::RunStart);
        assert_eq!(*turn, None);
        assert!(
            machine_blocks(&injections[0].2).len() == 1,
            "它在开场那一拍: {injections:#?}"
        );
        // 之后每一轮都没有它（逐轮点名，不是"总数对得上"）。
        for (index, (_, _, blocks)) in injections.iter().enumerate().skip(1) {
            assert!(
                machine_blocks(blocks).is_empty(),
                "第 {index} 条注入里不该再有它: {blocks:?}"
            );
        }
        // 模型最后一轮看到的视野里，它**仍然在**（历史只追加）且**只有一份**。
        let (turn, seen) = &model.seen[2];
        assert_eq!(*turn, 3);
        let in_view = machine_blocks(&seen_blocks(seen));
        assert_eq!(in_view.len(), 1, "视野里只有开场那一份: {seen:#?}");
        in_view[0].1.clone()
    };

    // 第二段（同一个环境、新的一段活动）：**开场又有一条**，而且与上一段逐字节相同。
    {
        let mut session = open(&dir, &mut environment, RUN_TWO, None);
        let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
        assert!(session.drive(&mut model).expect("驱动").is_unsettled());

        let openings = injections(&session);
        let found = machine_blocks(&openings[0].2);
        assert_eq!(found.len(), 1, "每一段开场都在: {openings:#?}");
        assert_eq!(found[0].1, first_text, "同一台机器 ⇒ 同一条事实");
        // 上一段没有正常收场 ⇒ 那张纸条也在（它没有被新来的一块挤掉）。
        let note = openings[0]
            .2
            .iter()
            .find(|(label, _)| label == "previous-activity")
            .unwrap_or_else(|| panic!("邻居照旧在场: {openings:#?}"));
        // ⚠ **指路只能指到它能走的路**（2026-10-06）：视图**刻意不给**环境内部那几个名字
        // （`the_view_never_exposes_environment_internals` 钉着），因此纸条里点名它们就是
        // "照着做会撞墙"——模型拿到的不是去哪里看，而是一个读不到的名字。
        for internal in ["identity.json", "trace.ndjson", "facts.ndjson"] {
            assert!(
                !note.1.contains(internal),
                "开场纸条不许点名环境内部文件（视图里它一律 not-found）: {internal} in {}",
                note.1
            );
        }
        assert!(
            note.1.contains("history/"),
            "指路要指到它真能走的那条（派生经历目录）: {}",
            note.1
        );
    }
}

/// **「搁下」不再是收场**（2026-10-10 在场那一刀）。
///
/// 判据一句话：**收场只有一个来源，就是主体说"结束了"**。「我停在这儿，有事叫我」不是结束——
/// 她**仍在场**，只是此刻没事。
///
/// 这一刀之前，`hold_run` 会走 `settle`（写终态、释放活动槽、封存），于是**每一声音铃都开一段
/// 新活动**、上下文每段从零起。那不是她的边界，是**门铃的边界**。
#[test]
fn holding_is_not_an_ending_any_more() {
    let dir = TempDir::new("hold-not-ending");
    let mut environment = dir.environment();
    let mut session = open(&dir, &mut environment, RUN_ONE, None);
    let mut model = ScriptedModel::new(vec![
        // 她点名「搁下」。
        ModelOutput::finish(SubjectEnding::Held),
        // 同一段里再说一句（第二声铃把她叫回来）。
        ModelOutput::say("还有一句"),
    ]);

    let first = session.drive(&mut model).expect("驱动");
    assert!(
        first.is_unsettled(),
        "「搁下」= 仍在场，不是收场（从前它会走 settle）: {first:?}"
    );
    assert_eq!(first.outcome(), None, "它**不**产生终态");

    // 同一段**再驱动一次**：轮次接着算、轨迹只追加，不重开一段。
    let second = session.drive(&mut model).expect("再驱动");
    assert!(
        second.is_unsettled(),
        "第二声铃驱动的是**同一段**: {second:?}"
    );
    assert_eq!(model.seen.len(), 2, "搁下之后这一段照旧能被再驱动");

    let starts = injections(&session)
        .iter()
        .filter(|(stage, _, _)| *stage == InjectionStage::RunStart)
        .count();
    assert_eq!(
        starts, 1,
        "两次驱动**只登记一次** run-start（同一段，不是两段）"
    );
}

/// **旧账里「搁下」的那一段，开场纸条说的是「搁下」，不是「没有正常收场」**。
///
/// ⚠ **2026-10-10 在场那一刀之后，这条判据只剩「照读」这一半**：`subject:held` 在**线缆上
/// 再也产不出来**（见上一条用例），但**旧账里有**——真机 2026-10-10 那一段里就有 7 段是这么
/// 收的场。盘上那张封闭表**照读不写**，所以这里**直接写一条旧形状的账**来验它。
///
/// 判据一句话：**两者在账上长得不一样**——搁下的那一段**有**收尾事实（`subject:held`），
/// 没走完的那一段**根本没有**。而下一段只能看到这张纸条 ⇒ 把「主体自己搁下的」读成「断了」
/// 就是**拿假话吓人**（反过来也一样：把断了读成「你搁下的」就是替主体认领一个它没做过的决定）。
#[test]
fn a_legacy_held_segment_is_still_reported_as_held_and_not_as_a_broken_one() {
    let dir = TempDir::new("held-note");
    let mut environment = dir.environment();
    let legacy = RunId::new(RUN_ONE);

    // 一条**旧形状**的账：那时「搁下」**是**收场的一种（经历侧 ＋ 事实侧两半都写）。
    environment
        .append_trace(&legacy, TracePayload::RunStarted { origin: None })
        .expect("登记活动");
    environment
        .append_trace(
            &legacy,
            TracePayload::Ending {
                subject: Some("held".to_string()),
                system: None,
                failure: None,
            },
        )
        .expect("写下收场");
    for record in [
        Record::RunStarted {
            agent: agent_id(),
            run: legacy.clone(),
        },
        Record::RunEnded {
            run: legacy.clone(),
            outcome: celira_agent::foundation::Outcome::Subject(SubjectEnding::Held),
            cost: Default::default(),
        },
    ] {
        environment.append_fact(&legacy, &record).expect("写下事实");
    }

    // 下一段开场：纸条必须说「搁下」，而且**不许**说「没有正常收场」。
    let mut session = open(&dir, &mut environment, RUN_TWO, None);
    let mut model = ScriptedModel::new(vec![ModelOutput::say("接着来")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let openings = injections(&session);
    let note = openings[0]
        .2
        .iter()
        .find(|(label, _)| label == "previous-activity")
        .unwrap_or_else(|| panic!("搁下的那一段要有纸条: {openings:#?}"));
    assert!(note.1.contains("搁下"), "纸条要说「搁下」: {}", note.1);
    assert!(
        !note.1.contains("hold_run"),
        "⚠ 措辞只说机械事实：那一格说的是**账上怎么收的场**，不是**她当时调了什么**\
         ——对纯正文那一次，「你调了 `hold_run`」是假话: {}",
        note.1
    );
    assert!(
        !note.1.contains("没有正常收场"),
        "**不许**把「主体自己搁下的」读成「断了」（那是假话）: {}",
        note.1
    );
    assert!(
        note.1.contains("history/"),
        "指路照旧指到它能走的那条: {}",
        note.1
    );
}

/// **点名搁下之后进程没了：纸条说清「你自己点名停下的」**（2026-10-13）。
///
/// 这一格与上一条（旧账 `subject:held`）覆盖的是**同一件事的两个时代**：有收尾事实的那个时代
/// （上一条，照读旧账），与"搁下不再收场"（§3.66）之后的这个时代。**今天真机上跑的就是这一条路**
/// ——2026-10-12 那一夜，她搁下之后进程没了，而下一段读到的是「有一段没有正常收场」。
///
/// ⚠ **上面那句「没有正常收场」照旧在，而且它是对的**：那一段**确实没有收尾事实**（它不是收场，
/// 是断了）。本文这一条限定句补的是**真相的另一层**——**她先停下来了**。
/// ⇒ 不许靠"把它的名字从名单里拿掉"来暗示（剔掉它会让总数变成假的，见
/// `previous_activity::PreviousReading::unfinished_text`）；两句都对，**限定句紧跟在它后面**
/// （句子里那个"它"就是上面点名的那一段）。
///
/// ⚠ **判据是"读账"，不是"认领"**：只有那一段**最后一条答复**确实是"停在这儿"时才这么说。
/// ⚠ 与上一条那句"不许出现 `hold_run`"**不冲突**：那里读的是**收尾事实**（那个值有两个来源
/// ——点名与纯正文——因此说出 `hold_run` 就是假话）；这里读的是**账**（点名那一次多一条工具
/// 调用记录 ⇒ 说得出来）。**同一句话在两处真假相反，判据就是"它是不是读出来的"。**
#[test]
fn a_segment_that_stopped_in_place_before_the_process_died_says_so() {
    let dir = TempDir::new("stopped-named");
    let mut environment = dir.environment();
    {
        // 第一段：她**点名**搁下（`hold_run`）⇒ `Unsettled` ⇒ 这一**段还在场、没有收尾事实**。
        let mut session = open(&dir, &mut environment, RUN_ONE, None);
        let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Held)]);
        assert!(session.drive(&mut model).expect("驱动").is_unsettled());
    } // ← 会话在这里丢掉：**进程没了**，而那一**段没有收场**。

    // 下一段的开场纸条：两句都要在，而且**限定句紧跟在那句之后**。
    let mut session = open(&dir, &mut environment, RUN_TWO, None);
    let mut model = ScriptedModel::new(vec![ModelOutput::say("接着来")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let openings = injections(&session);
    let note = openings[0]
        .2
        .iter()
        .find(|(label, _)| label == "previous-activity")
        .unwrap_or_else(|| panic!("那一格要有纸条: {openings:#?}"));
    assert!(
        note.1.contains("你自己点名停下的"),
        "纸条要说清是**她自己停下的**: {}",
        note.1
    );
    assert!(
        note.1.contains("`hold_run`"),
        "点名那一次**账上读得出来** ⇒ 说得出来（这是读账，不是认领）: {}",
        note.1
    );
    assert!(
        note.1.contains("此后这一段活动就断了"),
        "那一段**确实是断的**（不是干完了、也不是她收的场）⇒ 这句也要说: {}",
        note.1
    );
    let plain = note
        .1
        .find("没有正常收场")
        .expect("「没有正常收场」这一句是真的（那一段没有收尾事实）⇒ 不许拿掉它");
    let qualifier = note
        .1
        .find("你自己点名停下的")
        .expect("限定句要在（它才是这一刀加的那一层）");
    assert!(
        qualifier > plain,
        "限定句必须**紧跟**在那一句之后——它的主语就是上面点名的那一段: {}",
        note.1
    );
    assert!(
        note.1.contains("history/"),
        "指路照旧指到它能走的那条: {}",
        note.1
    );
}

/// **纯正文之后进程没了：纸条说的是「你自己停下的」，并且**说清那一次没有点名****（2026-10-13）。
///
/// 「搁下」有**两个来源**、而它们在账上**分得开**：点名（`hold_run`，多一条工具调用记录）与
/// 纯正文（把那一条答复交回控制）。用一句话盖住两种情形，会让主体**以为自己点过名**——
/// 那就是替它认领一个它没做过的动作。
#[test]
fn a_segment_that_stopped_with_plain_text_does_not_claim_a_named_stop() {
    let dir = TempDir::new("stopped-silent");
    let mut environment = dir.environment();
    {
        let mut session = open(&dir, &mut environment, RUN_ONE, None);
        let mut model = ScriptedModel::new(vec![ModelOutput::say("我先歇会儿")]);
        assert!(session.drive(&mut model).expect("驱动").is_unsettled());
    }

    let mut session = open(&dir, &mut environment, RUN_TWO, None);
    let mut model = ScriptedModel::new(vec![ModelOutput::say("在")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let openings = injections(&session);
    let note = openings[0]
        .2
        .iter()
        .find(|(label, _)| label == "previous-activity")
        .unwrap_or_else(|| panic!("那一格要有纸条: {openings:#?}"));
    assert!(
        note.1.contains("你自己停下的"),
        "两种来源都是「她自己停下的」: {}",
        note.1
    );
    assert!(
        note.1.contains("没有点名"),
        "纯正文那一次**没点名**，这句话要说出来（否则等于替它认领一个动作）: {}",
        note.1
    );
    assert!(
        !note.1.contains("hold_run"),
        "没有点名就**不许**提 `hold_run`（那是假话）: {}",
        note.1
    );
    let plain = note
        .1
        .find("没有正常收场")
        .expect("「没有正常收场」这一句是真的 ⇒ 照旧在");
    let qualifier = note.1.find("你自己停下的").expect("限定句要在");
    assert!(
        qualifier > plain,
        "限定句紧跟在那一句之后（主语同一段）: {}",
        note.1
    );
}

/// **"没走完"不止一段时，如实报数**（2026-10-06 主人裁决；本刀补上它的行为证明）。
///
/// 那一刀自己写了四条判据，这里逐条钉住：
///
/// ① **一段与多段分开说**——多段时不许压成"有一段"（那是假话，而"更早那些"正是最容易被漏掉的一格）；
/// ② **按写入序号排，不按活动 id**——所以这里故意让 id 的字面顺序与写入顺序**相反**：
///    `run-b` 先写、`run-a` 后写 ⇒ 按 id 排会得到 `run-a、run-b`（**错**），按序号排才是 `run-b、run-a`；
/// ③ **有上限、超量报数**（`MAX_UNFINISHED`）：没列出来的那几段只报数，不假装没有；
/// ④ **它不把自己算进去**——开场时这一段**还在跑**，把它写成"没有正常收场"就是假话。
#[test]
fn the_opening_note_counts_every_unfinished_run_in_write_order() {
    let dir = TempDir::new("unfinished-note");
    let mut environment = dir.environment();

    // 六段都没走完：每段都开、都驱动到"没结算"，然后丢下它（进程没了 = 那一段断了）。
    for run in ["run-b", "run-a", "run-c", "run-d", "run-e", "run-f"] {
        let mut session = open(&dir, &mut environment, run, None);
        let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
        assert!(session.drive(&mut model).expect("驱动").is_unsettled());
    }

    // 第七段开场：纸条要说清"有 6 段"，按**写入顺序**列前 5 段，并如实报出还有几段没列。
    let mut session = open(&dir, &mut environment, "run-g", None);
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());
    let openings = injections(&session);
    let note = openings[0]
        .2
        .iter()
        .find(|(label, _)| label == "previous-activity")
        .unwrap_or_else(|| panic!("开场纸条应当在: {:#?}", openings[0].2));

    assert!(
        note.1.contains("有 6 段没有正常收场"),
        "多段必须报总数，不许压成「有一段」: {}",
        note.1
    );
    assert!(
        note.1.contains("run-b、run-a、run-c、run-d、run-e"),
        "按写入序号排，不按活动 id: {}",
        note.1
    );
    assert!(
        !note.1.contains("run-f"),
        "超过上限的那一段只报数、不列出: {}",
        note.1
    );
    assert!(note.1.contains("还有 1 段未列出"), "{}", note.1);
    assert!(
        !note.1.contains("run-g"),
        "开场这一段还在跑，不许把自己写成「没有正常收场」: {}",
        note.1
    );
}

/// **"我上次为什么停"也归主体**（2026-10-06）：存储故障那条记录一直就在账本里，但从前的读者
/// **只有操作者的命令**——主体既看不到、也说不了话（那时它连跑都跑不起来）。现在开场那一块
/// 纸条把它带给主体：哪一段、什么原因、有没有被看过，一条事实不少，一句指令没有。
///
/// 为什么这一格值得单独钉：它是"至少知道哪里出了问题"的最小落地——她动不了编译产物，
/// 但**知道哪里出问题**这一半必须成立。
#[test]
fn the_opening_note_also_carries_the_last_storage_failure() {
    let dir = TempDir::new("storage-failure-note");
    let mut environment = dir.environment();
    let broken = RunId::new("run-broken");
    // 一次保存失败暂停 + 一次显式确认（有人看过之后放行）——两段都留在账本里。
    environment
        .mark_paused(&broken, "save-failed:run-start")
        .expect("写下暂停");
    celira_agent::store::clear_paused(&mut environment, &broken).expect("显式确认");

    let mut session = open(&dir, &mut environment, RUN_ONE, None);
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let injections = injections(&session);
    let blocks = &injections[0].2;
    let note = blocks
        .iter()
        .find(|(label, _)| label == "previous-activity")
        .unwrap_or_else(|| panic!("开场纸条应当在: {blocks:#?}"));
    assert!(note.1.contains("存储出过一次故障"), "{}", note.1);
    assert!(note.1.contains("run-broken"), "哪一段要说清: {}", note.1);
    assert!(
        note.1.contains("save-failed:run-start"),
        "原因要说清: {}",
        note.1
    );
    assert!(
        note.1.contains("已经被显式确认过一次"),
        "看过没有也要说（它是事实的一部分）: {}",
        note.1
    );
    // **零指令**：与"没有正常收场"那条同一条边界——世界给信息，决定权在它。
    for word in ["你应该", "你必须", "建议你", "请你"] {
        assert!(
            !note.1.contains(word),
            "不许带指令口气: {word} in {}",
            note.1
        );
    }
}

// ---------------------------------------------------------------------------
// 判据 4 + 6：位置（紧贴身份行外面一圈）与邻居的完整性
// ---------------------------------------------------------------------------

#[test]
fn the_fact_sits_between_the_identity_and_the_holdings_without_displacing_anyone() {
    let dir = TempDir::new("position");
    let mut environment = dir.environment();
    std::fs::write(dir.env_root().join(SOUL_FILE), "我是阿默。\n").expect("写下天性");

    let mut session = open(
        &dir,
        &mut environment,
        RUN_ONE,
        Some(IdentityClaim {
            id: "core-1".to_string(),
            display_name: "小满".to_string(),
        }),
    );
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let openings = injections(&session);
    let blocks = &openings[0].2;
    let names: Vec<&str> = blocks.iter().map(|(label, _)| label.as_str()).collect();

    // 位置：`起源 → 天性 → 身份行 → **机器处境** → 持有物`。
    let at = |label: &str| {
        names
            .iter()
            .position(|name| *name == label)
            .unwrap_or_else(|| panic!("{label} 应当在开场: {names:?}"))
    };
    assert_eq!(at("soul"), at("activity-origin") + 1, "{names:?}");
    assert_eq!(at("identity"), at("soul") + 1, "{names:?}");
    assert_eq!(
        at(MACHINE_LABEL),
        at("identity") + 1,
        "机器处境紧跟身份行（你是谁、你在哪台机器上）: {names:?}"
    );
    assert_eq!(
        at("holdings"),
        at(MACHINE_LABEL) + 1,
        "持有物紧贴机器处境外面一圈: {names:?}"
    );

    // 互不干扰：既有那些块一块不少（逐块点名，缺一块就红）。
    for label in [
        "activity-origin",
        "soul",
        "identity",
        MACHINE_LABEL,
        "holdings",
        "pinned-actions",
        "pinned-notes",
        "notification-bar",
        "reading-guide",
        "status-bar",
    ] {
        assert!(names.contains(&label), "{label} 不见了: {names:?}");
    }
    // 而且**没有多出第二块**机器处境（一块，不是两块）。
    assert_eq!(machine_blocks(blocks).len(), 1, "{names:?}");
}

// ---------------------------------------------------------------------------
// 判据 5：不承诺能力
// ---------------------------------------------------------------------------

#[test]
fn the_fact_never_promises_a_capability() {
    let dir = TempDir::new("no-promise");
    let mut environment = dir.environment();
    let mut session = open(&dir, &mut environment, RUN_ONE, None);
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let openings = injections(&session);
    let found = machine_blocks(&openings[0].2);
    assert_eq!(found.len(), 1);
    let text = &found[0].1;

    // 只陈述机器是什么：不出现动词名、不出现「你可以……」、不提任何能力标识。
    for word in [
        "terminal",
        "terminal_run",
        "命令",
        "可以",
        "能力",
        "capability",
        "工具",
    ] {
        assert!(!text.contains(word), "「{word}」出现在：{text}");
    }
    // 反向（正向那一半）：它**必须**说清平台与 shell 是什么——两格都在、都非空。
    for field in ["platform = ", "shell = "] {
        let value = text
            .split(" · ")
            .find_map(|part| part.strip_prefix(field))
            .unwrap_or_else(|| panic!("缺一格 {field}：{text}"));
        assert!(!value.trim().is_empty(), "空格子：{text}");
    }
}

/// **「哪一件事没结算」也归主体**（2026-10-06 主人裁决，第一性那条）。
///
/// 终局要的四件里这一件从前没有落点（`agent/README.md` 那句"因此这一格不立项"已按裁决翻掉）。
/// 本用例把它钉在四件上：
///
/// 1. **机械事实照给** —— 哪一轮、那一轮的第几个、能力名、什么时候；
/// 2. **参数一个都不进开场** —— 判据是**主体性**（`target` 是解释，解释归主体），
///    **不是"私密"**：原文在 `history/` 里逐条原样，主体自己能看；
/// 3. **是信息，不是指令** —— 一个"该"字都没有；
/// 4. **作用域如实登记** —— 只覆盖最近那一段，不许让主体以为只有这一段。
#[test]
fn the_opening_note_also_carries_the_unsettled_operations() {
    let dir = TempDir::new("unsettled-note");
    let mut environment = dir.environment();
    let broken = RunId::new("run-half-done");
    let turn = celira_agent::foundation::Turn::from_number(2).expect("第 2 轮");
    let operation = OperationId::at(broken.clone(), turn, 0);
    // 一次"选过、发出去了、而没有结果"的操作：经历侧有选择，事实侧只有派发。
    environment
        .append_trace(&broken, TracePayload::RunStarted { origin: None })
        .expect("登记活动");
    environment
        .append_trace(
            &broken,
            TracePayload::ActionSelected {
                turn: 2,
                index: 0,
                action: "terminal_run".to_string(),
                // ⚠ 这个取值**不许**出现在开场里 —— 下面拿它当反例。
                params: vec![("command".to_string(), "echo SECRET-MARKER".to_string())],
            },
        )
        .expect("写下选择");
    // ⚠ **事实侧那条 `run-started` 也必须有**：只有经历侧有它 ⇒ `start-without-fact`，
    // 于是既有解释器给的是 `Blocked` 而不是 `Unknown`（同族：只写 `dispatched` 会触发
    // "派发过、没准备过"那条矛盾）。这一档的判据一个字都不改，夹具照它的形状搭。
    environment
        .append_fact(
            &broken,
            &Record::RunStarted {
                agent: agent_id(),
                run: broken.clone(),
            },
        )
        .expect("登记活动事实");
    environment
        .append_fact(
            &broken,
            &Record::ActionPrepared {
                run: broken.clone(),
                operation: operation.clone(),
            },
        )
        .expect("写下准备");
    environment
        .append_fact(
            &broken,
            &Record::ActionDispatched {
                run: broken.clone(),
                operation,
            },
        )
        .expect("写下派发");

    let mut session = open(&dir, &mut environment, RUN_ONE, None);
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());
    let openings = injections(&session);
    let note = openings[0]
        .2
        .iter()
        .find(|(label, _)| label == "previous-activity")
        .unwrap_or_else(|| panic!("那张纸条要在场: {openings:#?}"))
        .1
        .clone();

    // ① **机械事实照给**。
    assert!(note.contains("turn 2 index 0"), "要说清是哪一件: {note}");
    assert!(note.contains("terminal_run"), "要说清是什么能力: {note}");
    // ② **参数一个都不进开场**（判据是主体性，不是"私密"）。
    assert!(
        !note.contains("SECRET-MARKER"),
        "开场不许出现能力参数 —— 原文在 `history/` 里，主体自己看: {note}"
    );
    assert!(!note.contains("command"), "参数的名字也不出现: {note}");
    // ③ **是信息，不是指令**。
    for banned in ["该", "建议", "重要"] {
        assert!(!note.contains(banned), "零指令（「{banned}」）: {note}");
    }
    // ④ **作用域如实登记**。
    assert!(
        note.contains("更早的段"),
        "不许让主体以为只有这一段: {note}"
    );
}

/// **结束了 ≠ 结算了**（2026-10-10 裁决）：上一段**正常收场**了，而它派发过的某一件结果
/// 没有回来 —— 那几件**照样**要摆在她眼前。
///
/// 判据是那两件事本来就独立：一次活动可以**带着未确认的效果收场**（`system:indeterminate`
/// 说的就是这个）。旧作用域是"最近那一段**没走完**"，于是这种段里的那几件**一个字都看不到**，
/// 而纸条还会读成"**确实没有**"（那是假话，不是"读不到"）。
///
/// 本用例与上一条的差别只有一处：**这一段有收场**。其余判据（机械事实照给 / 参数不进开场 /
/// 零指令）照旧逐条钉。
#[test]
fn the_opening_note_reports_unsettled_operations_of_a_settled_run_too() {
    let dir = TempDir::new("unsettled-settled");
    let mut environment = dir.environment();
    let broken = RunId::new("run-settled-with-hole");
    let turn = celira_agent::foundation::Turn::from_number(1).expect("第 1 轮");
    let operation = OperationId::at(broken.clone(), turn, 0);
    // 一段**正常收场**的活动：经历侧有开始 / 选择 / 收场，事实侧有开始 / 准备 / 派发 / 收场，
    // **唯独那一件的结果没有回来**。
    environment
        .append_trace(&broken, TracePayload::RunStarted { origin: None })
        .expect("登记活动");
    environment
        .append_trace(
            &broken,
            TracePayload::ActionSelected {
                turn: 1,
                index: 0,
                action: "terminal_run".to_string(),
                // ⚠ 这个取值**不许**出现在开场里 —— 下面拿它当反例。
                params: vec![("command".to_string(), "echo SECRET-MARKER".to_string())],
            },
        )
        .expect("写下选择");
    environment
        .append_trace(
            &broken,
            TracePayload::Ending {
                subject: Some("silent".to_string()),
                system: None,
                failure: None,
            },
        )
        .expect("写下收场");
    for record in [
        Record::RunStarted {
            agent: agent_id(),
            run: broken.clone(),
        },
        Record::ActionPrepared {
            run: broken.clone(),
            operation: operation.clone(),
        },
        Record::ActionDispatched {
            run: broken.clone(),
            operation,
        },
        Record::RunEnded {
            run: broken.clone(),
            outcome: celira_agent::foundation::Outcome::Subject(
                celira_agent::foundation::SubjectEnding::Silent,
            ),
            cost: Default::default(),
        },
    ] {
        environment.append_fact(&broken, &record).expect("写下事实");
    }

    let mut session = open(&dir, &mut environment, RUN_ONE, None);
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());
    let openings = injections(&session);
    let note = openings[0]
        .2
        .iter()
        .find(|(label, _)| label == "previous-activity")
        .unwrap_or_else(|| panic!("上一段结束了的段，它未结算的那几件照样要在场: {openings:#?}"))
        .1
        .clone();

    // ① **机械事实照给**。
    assert!(note.contains("turn 1 index 0"), "要说清是哪一件: {note}");
    assert!(note.contains("terminal_run"), "要说清是什么能力: {note}");
    // ② **参数一个都不进开场**（判据是主体性，不是"私密"）。
    assert!(
        !note.contains("SECRET-MARKER"),
        "开场不许出现能力参数 —— 原文在 `history/` 里，主体自己看: {note}"
    );
    // ③ **是信息，不是指令**。
    for banned in ["该", "建议", "重要"] {
        assert!(!note.contains(banned), "零指令（「{banned}」）: {note}");
    }
    // ④ **不许把它写成"没有正常收场"**：它收场了 —— 这一格说的是"哪一件没结算"，
    //    不是"哪一段没走完"。两件事混在一起，下一段就分不出"我被打断了"和"我走完了、但有件没回音"。
    assert!(
        !note.contains("没有正常收场"),
        "它收场了：这两件事必须分得开: {note}"
    );
}
