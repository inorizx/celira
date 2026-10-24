//! TASK_18 验收：**系统服务层第一个件——状态栏**。
//!
//! 这一份文件只证明一件事：主体的处境（现在几点、轮次用到哪儿）**每一轮都在视野里**，而且这条
//! 读数没有制造第二份事实源。六条判据各有自己的落点：
//!
//! 1. 活动开场的处境行**恰好一条**、每一轮**恒摆一条**，全部走既有 `kind=injection` 形状落盘
//!    （没有新增持久种类，状态行也不进事实）；
//! 2. 它进入模型请求的那一段由 `tests/provider_wire.rs` 用**真实适配层 + 回环服务**证明
//!    （含"非 system、带围栏"与"与适配层机械运行事实两处渲染"）；
//! 3. 跨等待之后再驱动，轮次读数**从本段已确认记录接着算**，不重置（跨**进程**那一格随
//!    跨进程恢复一起退役：新的一段从第 1 轮开始，见判据 3 那一节的说明）；
//! 4. 固定钟下同输入**逐字节相同**输出，时间取 RFC 3339 UTC；
//! 5. 两处渲染（本件一个比例、适配层三个字段）同时存在，谁也不被静默删掉；
//! 6. 既有测试期望的更新逐项写在交付报告里。
//!
//! 明确**不**声称：没有跑公网模型（模型是脚本替身）；状态栏**不是**通知栏（增量窗、"一条只播
//! 一次"的纪律不属于本件）；屏幕占比、token/额度、deadline 这些真实联跑项本刀不做。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use celira_agent::activity_origin::ActivityOrigin;
use celira_agent::clock::{
    ClockHandle, LocalOffset, anchor_value, format_local, format_utc, localize_rfc3339,
    parse_rfc3339,
};
use celira_agent::composition::{RunSession, SessionDeclarations, SessionDrive, SessionRequest};
use celira_agent::context_reading::ContextReading;
use celira_agent::core::combine;
use celira_agent::core::{
    ContentBatch, ContentBlock, InjectionStage, Limits, ModelError, ModelOutput, ModelPort,
    RunInput, TraceEntry, View,
};
use celira_agent::foundation::{AgentId, RunId, SubjectEnding};
use celira_agent::self_concept::{IdentityClaim, SELF_CONCEPT_FILE, SELF_CONCEPT_LABEL};
use celira_agent::status_bar::{READING_GUIDE, READING_GUIDE_LABEL, STATUS_BAR_LABEL, StatusBar};
use celira_agent::store::Environment;

const AGENT: &str = "agent-status-bar";
const RUN: &str = "run-status-bar";

/// 用例**钉死**的本机偏移（`+08:00`）。
///
/// 为什么钉死：真实偏移来自**跑用例的那台机器**（`LocalOffset::of_machine`），
/// 而"同一时刻同一输入 ⇒ 逐字节相同输出"这条判据只有在偏移也被钉住时才可判错——
/// 这与"时间来源只从构造参数进来"是同一条理由（见 `clock.rs` 的文件头）。
const TEST_OFFSET: LocalOffset = LocalOffset::Minutes(8 * 60);

/// 一块注入内容：标签 + 正文。
type Block = (String, String);

/// 轨迹里一条注入条目：阶段、轮次（Run 级没有）、块。
type Injection = (InjectionStage, Option<u32>, Vec<Block>);

/// 模型某次调用看到的注入条目（轮次由那次调用本身给出）。
type SeenInjection = (InjectionStage, Vec<Block>);

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
        path.push(format!(
            "celira-task18-status-{}-{label}-{n}",
            std::process::id()
        ));
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

    fn trace_path(&self) -> PathBuf {
        self.env_root().join("trace.ndjson")
    }

    fn facts_path(&self) -> PathBuf {
        self.env_root().join("facts.ndjson")
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

/// 用例装配用的**时间来源**：固定钟——离线、可逐字节断言（`0` = 1970-01-01T00:00:00Z）。
fn fixed_clock() -> ClockHandle {
    ClockHandle::fixed_at_unix_seconds(0)
}

/// 用例声明的**上下文窗口**（宿主声明，必须正整数）：取一个小而固定的数字，便于逐字节断言。
///
/// 生产入口缺省用适配层的公开标称值（1M），用例一律显式声明——窗口是声明不是测量，用例自己
/// 声明一个"窗口有多小"才能把那一格钉住（TASK_74 起它的标签是 `context`）。
const CONTEXT_WINDOW: u32 = 1_000;

/// 用例装配用的**屏幕报告 cell 的读侧**：默认是**空 cell** ⇒ 状态行读作 `context = unknown/… token`。
///
/// 要模拟"适配层报过一次"的形态，用 `ContextReading::channel()` 拿写柄自己写（写柄在这一侧只是
/// 用例的替身；生产里它只交给适配层——这条纪律由源码守卫
/// `only_the_adapter_writes_the_reading_cell` 机械检查）。
fn reading_cell() -> ContextReading {
    ContextReading::default()
}

/// 脚本模型：按序交出事先写好的响应，并把每一次调用看到的轨迹按顺序记下来。
struct ScriptedModel {
    steps: VecDeque<ModelOutput>,
    /// 每次调用看到的（轮次，轨迹里 injections 的（阶段，块））。
    seen: Vec<(u32, Vec<SeenInjection>)>,
}

impl ScriptedModel {
    fn new(steps: Vec<ModelOutput>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
            seen: Vec::new(),
        }
    }

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
fn open<'a>(dir: &TempDir, environment: &'a mut Environment, limits: u32) -> RunSession<'a> {
    open_with_identity(dir, environment, limits, None)
}

/// 同 [`open`]，但**带上宿主声明的身份**（TASK_70）：身份行那一块由它决定在场与否。
fn open_with_identity<'a>(
    dir: &TempDir,
    environment: &'a mut Environment,
    limits: u32,
    identity: Option<IdentityClaim>,
) -> RunSession<'a> {
    let materials = dir.materials();
    let declared = agent_id();
    RunSession::open(
        &materials,
        environment,
        SessionRequest {
            declarations: SessionDeclarations {
                identity,
                // 其余几格一件都不声明（`Default` 就是"全都没声明"）。
                ..Default::default()
            },
            declared_agent: &declared,
            run: RunId::new(RUN),
            limits: Limits::new(limits),
            input: RunInput::text("开场输入").expect("在上限内"),
            clock: fixed_clock(),
            context_window: CONTEXT_WINDOW,
            reading: reading_cell(),
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

/// 一条状态行**应当**长什么样（判据 4：同一时刻同一输入 ⇒ 逐字节相同）。
///
/// 四段：`time`（固定钟）/ `turn`（已用/上限）/ `context`（**最近一次报告**的占用，或 `unknown`）/
/// `since`（「新」的起点：**上一次去看投影的时刻**，装配没声明时读作 `unknown`）。
/// `context` 那一段的分母永远是宿主声明的窗口（[`CONTEXT_WINDOW`]），因为窗口是声明不是测量；
/// 它的**单位词**是给主体的那个词（TASK_74：见 `status_bar::CONTEXT_UNIT`）。
fn status_line(unix_seconds: u64, used: u32, limit: u32, reported: &str) -> String {
    format!(
        "time = {}\nturn = {used}/{limit}\ncontext = {reported}/{CONTEXT_WINDOW} token\nsince = unknown",
        format_local(
            SystemTime::UNIX_EPOCH + Duration::from_secs(unix_seconds),
            TEST_OFFSET
        )
    )
}

// ---------------------------------------------------------------------------
// 判据 1 + 2（进程内这一半）：开场一条，之后每轮一条，全部是既有 injection 形状
// ---------------------------------------------------------------------------

#[test]
fn the_activity_opens_with_one_reading_and_restates_it_before_every_turn() {
    let dir = TempDir::new("every-turn");
    let mut environment = dir.environment();
    let mut session = open(&dir, &mut environment, 4);
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等"), ModelOutput::say("再等")]);

    // 两次驱动 = 两轮真实模型调用（纯正文交回控制，活动没有收场）。
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());
    assert_eq!(model.calls(), 2);

    let injections = injections(&session);
    assert_eq!(
        injections.len(),
        3,
        "开场一条 + 两轮各一条: {injections:#?}"
    );

    // 开场那一轮：**七元素链 ＋ 说明书**——内层起源事实在前，然后是**机器处境**（这台机器是什么：
    // 平台 + shell，机器处境那一刀新增）、**持有物**（主体自己约下的时刻，持有物那一刀新增）、
    // **动作 pin**（TASK_68 P3：主体钉住的动作详情，出厂默认开箱即在）、
    // 组件区（主体的纸条）、通知栏（世界的账）四块**开场快照**，
    // 最外是**这一行的一次性说明书**（TASK_74）与状态行；轮次读数是"一次都还没消耗"。
    let (stage, turn, blocks) = &injections[0];
    assert_eq!(*stage, InjectionStage::RunStart);
    assert_eq!(*turn, None, "Run 级注入没有轮次");
    assert_eq!(
        blocks.len(),
        8,
        "七处批次 ＋ 说明书都递交（说明书写在最外，紧挨它解释的那一行）: {blocks:?}"
    );
    assert_eq!(blocks[0].0, "activity-origin", "最内层在前");
    // 机器处境紧贴"我是谁"外面一圈（这个舞台没声明身份、也没写过自识 ⇒ 那两块都不出场），
    // 因此它是起源之后的**第一个**块：主体先知道"这次为什么醒"，再立刻知道"我在哪台机器上"。
    assert_eq!(
        blocks[1].0, "machine",
        "机器处境在起源之后、持有物之前（这台机器是什么）: {blocks:?}"
    );
    assert!(
        blocks[1].1.starts_with("platform = ") && blocks[1].1.contains(" · shell = "),
        "一行两格：平台 + shell: {}",
        blocks[1].1
    );
    assert_eq!(
        blocks[1].1,
        celira_agent::machine::reading_of(celira_agent::machine::Platform::of_machine()),
        "那一行就是本件从机器现读的那一条（逐字节）"
    );
    // 持有物紧贴机器处境外面一圈（快照不是活状态）。
    assert_eq!(
        blocks[2].0, "holdings",
        "持有物在机器处境之后、工具面之前（主体自己约下的时刻）: {blocks:?}"
    );
    assert_eq!(
        blocks[2].1, "holdings = 0\n（没有约下的时刻）",
        "表里空着也要说出口——缺省不等于零"
    );
    assert_eq!(
        blocks[3].0, "pinned-actions",
        "动作 pin 在组件区之内（**她自己钉住的格位**——2026-10-10 起常驻只剩这一个来源）"
    );
    // **桌面是空的**：这个舞台没装配 im，也没有任何 pin 文件 ⇒ 一项都没钉。
    // 零被说出口，而且指路（怎么钉、上限几条、不钉的代价）——缺省不等于零，也不留空白。
    assert!(
        blocks[3].1.starts_with("pinned = 0\n")
            && blocks[3].1.contains("pin_action")
            && blocks[3].1.contains("先看一次格式"),
        "没钉过就说 0，并说清怎么钉与不钉的代价: {}",
        blocks[3].1
    );
    assert!(
        !blocks[3].1.contains("notes_list") && !blocks[3].1.contains("notes_create"),
        "**一条动作都不开箱即在**（出厂精选已删）: {}",
        blocks[3].1
    );
    assert!(
        !blocks[3].1.contains("im_say"),
        "**不在场的不渲染**（这个舞台没装配 im）——凭空造不出动作: {}",
        blocks[3].1
    );
    assert_eq!(
        blocks[4].0, "pinned-notes",
        "组件区在通知栏之内（主体自己布置的那一块）"
    );
    assert_eq!(
        blocks[4].1, "pinned = 0\n（一条也没标上）",
        "一条也没标也要说出口——缺省不等于零"
    );
    assert_eq!(blocks[5].0, "notification-bar", "通知栏在中间（快照）");
    assert_eq!(
        blocks[5].1, "since = unknown\ncounts = unknown",
        "**两件事各说各的**（2026-10-10）：没有锚（增量问不出来）与这一次没读到，都不是「没有新的」"
    );
    assert_eq!(
        blocks[6].0, READING_GUIDE_LABEL,
        "说明书在状态行前面（主体先读图例、再读读数）"
    );
    assert_eq!(
        blocks[6].1, READING_GUIDE,
        "说明书就是那一份常量（纯静态、一块封顶）"
    );
    assert_eq!(blocks[7].0, STATUS_BAR_LABEL, "状态行在最外");
    assert_eq!(
        blocks[7].1,
        status_line(0, 0, 4, "unknown"),
        "开场读数是 0/上限"
    );

    // 每一轮：状态行**每一轮都在**，轮次是**这一轮的真实序号**；
    // TASK_68 P2 起，**首轮另带一份动作目录**（只发一次，之后不再重发）。
    for (index, expected) in [(1u32, 2usize), (2, 3)] {
        let (stage, turn, blocks) = &injections[expected - 1];
        assert_eq!(*stage, InjectionStage::Turn);
        assert_eq!(*turn, Some(index), "注入带真实轮次");
        let expected_blocks = if index == 1 { 2 } else { 1 };
        assert_eq!(
            blocks.len(),
            expected_blocks,
            "第 {index} 轮的块数（首轮多一份目录）: {blocks:?}"
        );
        assert_eq!(blocks[0].0, STATUS_BAR_LABEL);
        assert_eq!(blocks[0].1, status_line(0, index, 4, "unknown"));
        if index == 1 {
            assert_eq!(
                blocks[1].0, "available-actions",
                "首轮带一份动作目录: {blocks:?}"
            );
            // TASK_69（真机四证补上的一格）：**免查的动作必须"免学"**。
            //
            // OS 自己那六件在准入面**直接放行**（`admit` 的 early-return）⇒ 调用之前
            // **没有别的地方**能知道它们的参数名：目录原本只给"名字 | 一句话"，而要看详情卡
            // 又得先知道 `inspect_action` 的参数名——一个自举死结。
            // 真机实测（2026-09-21）：模型对 `open_app` 的参数名连试三次都错（`name` / `app`），
            // 三个 Run 都卡在第一轮（一个 `model-failed`、一个 `declined`）。
            //
            // ⇒ 由**声明方**标了"免查"的条目，目录行必须自带参数名。
            // 判错即红：把 `os.rs` 里任何一个 `.exempt_from_inspection()` 删掉 ⇒ 这条立刻红。
            //
            // TASK_71 桌面 P0 起目录按 App 分组 ⇒ 动词行**在自己那扇门里**（组头零缩进、
            // 门内行缩进）。判据因此落在"**去掉缩进后的行首**"（不依赖缩进宽度），
            // 并额外要求它是一条**门内行**——这正是"它属于某扇门"的机械形态。
            for (id, param) in [
                ("open_app", "target"),
                ("inspect_action", "action"),
                ("pin_action", "action"),
                ("unpin_action", "action"),
                ("write_self_concept", "text"),
                ("write_handoff", "text"),
            ] {
                assert!(
                    blocks[1].1.lines().any(|line| line.starts_with(' ')
                        && line.trim_start().starts_with(&format!("{id} | "))
                        && line.contains(&format!("{param} = "))),
                    "免查动作 `{id}` 的参数名 `{param}` 必须在目录的**门内行**里可见: {}",
                    blocks[1].1
                );
            }
            // 反向：**走"先看格式"那条路的动作照旧不给参数**——别让"目录自带参数"被顺手扩成全部。
            // `notes_create` 虽然因出厂格位而免查，但它**不是**由声明方标的 ⇒ 目录行不该带参数。
            assert!(
                !blocks[1].1.lines().any(|line| line.starts_with(' ')
                    && line.trim_start().starts_with("notes_create | ")
                    && line.contains("（参数：")),
                "只有**声明方**标了免查的动作才自带参数名（pin 带来的免查照旧走格位块）: {}",
                blocks[1].1
            );
        }
    }

    // 模型自己看到的也是这两处：开场那轮看得见"来源 + 读数"，之后每轮都看得见读数。
    let (first_turn, first_injections) = &model.seen[0];
    assert_eq!(*first_turn, 1);
    let first_turn_blocks: Vec<&(String, String)> = first_injections
        .iter()
        .flat_map(|(_, blocks)| blocks.iter())
        .collect();
    assert_eq!(
        first_turn_blocks.len(),
        10,
        "开场那一轮：来源 + 机器处境 + 持有物 + 动作 pin + 组件区快照 + 通知栏快照 + **说明书** + 开场读数 + 本轮读数 + 动作目录（TASK_68 P2/P3 ／ TASK_74 说明书 ／ 持有物那一刀 ／ 机器处境那一刀）"
    );
    assert_eq!(
        first_turn_blocks[1].0, "machine",
        "机器处境也在这一轮（开场快照，之后不重摆）"
    );
    assert_eq!(
        first_turn_blocks[2].0, "holdings",
        "持有物也在这一轮（开场快照，之后不重摆）"
    );
    assert_eq!(
        first_turn_blocks[3].0, "pinned-actions",
        "动作 pin 也在这一轮（开场快照，之后不重摆）"
    );
    assert_eq!(
        first_turn_blocks[4].0, "pinned-notes",
        "组件区那块**只在这一轮**出现（快照不是活状态）"
    );
    assert_eq!(
        first_turn_blocks[5].0, "notification-bar",
        "通知栏同样只在这一轮出现"
    );
    assert_eq!(
        first_turn_blocks[6].0, READING_GUIDE_LABEL,
        "说明书也在这一轮的视野里（它属于开场那一块）"
    );
    assert_eq!(first_turn_blocks[8].1, status_line(0, 1, 4, "unknown"));
    let (second_turn, second_injections) = &model.seen[1];
    assert_eq!(*second_turn, 2);
    let second_turn_readings: Vec<&str> = second_injections
        .iter()
        .flat_map(|(_, blocks)| blocks.iter())
        .filter(|(label, _)| label == STATUS_BAR_LABEL)
        .map(|(_, text)| text.as_str())
        .collect();
    assert_eq!(
        second_turn_readings,
        vec![
            status_line(0, 0, 4, "unknown").as_str(),
            status_line(0, 1, 4, "unknown").as_str(),
            status_line(0, 2, 4, "unknown").as_str()
        ],
        "第二轮的视野里三条读数都在（历史只追加），最后一条是**当前值**"
    );

    // "登记恰好一次"这条既有的判据没有被破坏：Run 级注入仍然只有一条。
    let run_start = injections
        .iter()
        .filter(|(stage, ..)| *stage == InjectionStage::RunStart)
        .count();
    assert_eq!(run_start, 1);
}

// ---------------------------------------------------------------------------
// TASK_71 桌面 P0（①）——目录按 App 分组：**门牌是 App，门内是它的动词**
// ---------------------------------------------------------------------------

/// 目录顶部那行 **App 索引**的前缀（`Apps：…`）。它既不是组头，也不属于任何门。
const APPS_INDEX: &str = "Apps：";

/// 把目录正文切成**按门牌分的节**：每个 `【…】` 开头那一行是组头（零缩进），
/// 它之后到下一个组头之前的所有行是**门内行**。
///
/// 顶部那一行 App 索引（[`APPS_INDEX`] 开头）是**唯一**被跳过的前导行：它在第一个组头之前，
/// 但它不是任何门里的行。别的行出现在组头之前照旧 panic（形状变了就要有人看见）。
fn directory_sections(text: &str) -> Vec<(&str, Vec<&str>)> {
    let mut sections: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in text.lines() {
        if sections.is_empty() && line.starts_with(APPS_INDEX) {
            continue;
        }
        match line.starts_with('【') {
            true => sections.push((line, Vec::new())),
            false => sections
                .last_mut()
                .unwrap_or_else(|| panic!("门内行出现在任何组头之前: {text}"))
                .1
                .push(line),
        }
    }
    sections
}

/// 索引里某个 App 那一段（`<id>（…）`）。索引是一行：`Apps：a（…） · b（…）`。
fn index_entry<'a>(index: &'a str, app: &str) -> &'a str {
    index
        .strip_prefix(APPS_INDEX)
        .unwrap_or(index)
        .split(" · ")
        .find(|part| part.starts_with(&format!("{app}（")))
        .unwrap_or_else(|| panic!("索引里没有 {app} 那一段: {index}"))
}

/// 索引里某个 App 的**动作数**（只对常开的门有意义）。
fn index_count(index: &str, app: &str) -> usize {
    let entry = index_entry(index, app);
    let rest = entry
        .strip_prefix(&format!("{app}（常开，"))
        .unwrap_or_else(|| panic!("{app} 那一段不是「常开，N 个动作」: {entry}"));
    let digits = rest
        .strip_suffix(" 个动作）")
        .unwrap_or_else(|| panic!("{app} 那一段缺动作数: {entry}"));
    digits
        .parse()
        .unwrap_or_else(|_| panic!("{app} 的动作数不是数字: {entry}"))
}

/// 门内行的**动作标识**（行格式 `<标识> | <骨架>`；去缩进后取 ` | ` 之前那一段）。
/// 组头（如 `读：`）没有 ` | `，返回 `None`——它不是动词。
fn verb_of(line: &str) -> Option<&str> {
    line.trim().split_once(" | ").map(|(verb, _)| verb)
}

/// **分组形状**（TASK_71 桌面 P0 第 1 件的验收②）：组头在场、门内动词归属正确、
/// **未展开的 App 也上榜**（且它的动词一条都不许出现）。
///
/// 这个舞台（`open`）装配了 `files` / `notes` / `web`，其中 notes 与 web **预装常开**、
/// files 是"要点开才展开"的材料窗 ⇒ 三扇门都在榜上，而只有 files 那扇门后面是空的。
#[test]
fn the_action_directory_groups_verbs_under_their_app_and_keeps_unopened_doors_on_the_board() {
    let dir = TempDir::new("directory-grouped");
    let mut environment = dir.environment();
    let mut session = open(&dir, &mut environment, 4);
    let mut model = ScriptedModel::new(vec![ModelOutput::say("看了看")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let injections = injections(&session);
    let (_, _, blocks) = injections
        .iter()
        .find(|(stage, turn, _)| *stage == InjectionStage::Turn && *turn == Some(1))
        .expect("首轮那一拍");
    assert_eq!(blocks.len(), 2, "首轮多一份目录: {blocks:?}");
    assert_eq!(blocks[1].0, "available-actions");
    let directory = blocks[1].1.as_str();

    // **最顶上一行是 App 索引**：一行说清榜上有哪几扇门、哪几扇常开、哪几扇还没打开。
    // 它排在【系统】之前，是这一块的**第一行**（也是 [`directory_sections`] 唯一跳过的前导行）。
    let index = directory.lines().next().expect("目录至少有一行");
    assert!(
        index.starts_with(APPS_INDEX),
        "目录第一行是 App 索引: {directory}"
    );
    assert!(
        directory
            .lines()
            .nth(1)
            .is_some_and(|line| line.starts_with("【系统】")),
        "索引之后紧接着就是【系统】那一组（索引不许插进别的行）: {directory}"
    );

    let sections = directory_sections(directory);
    assert_eq!(
        sections.len(),
        5,
        "一个「不归任何 App」组 + 四扇门（这个舞台有 files / notes / web / timer）: {directory}"
    );
    assert_eq!(
        sections[0].0, "【系统】不归任何 App 的动作",
        "头一组是门之外那组（OS 自有、不归任何 App）: {directory}"
    );
    // 门牌 = `【<App 标识>】<App 自己那句话>`：标识对，而且**带描述**（不是只有一个名字）。
    for (index, app) in ["files", "notes", "timer", "web"].iter().enumerate() {
        let header = sections[index + 1].0;
        assert!(
            header.starts_with(&format!("【{app}】")) && header.len() > format!("【{app}】").len(),
            "第 {} 扇门的门牌是「标识 + 描述」: {header}",
            index + 1
        );
    }

    // 组头零缩进、门内行有缩进 —— "在门里"这件事是**看得见**的（不依赖具体缩进宽度）。
    for (header, body) in &sections {
        assert!(!header.starts_with(' '), "组头不缩进: {header}");
        assert!(
            !body.is_empty(),
            "每一组都不是空的（未展开的门也有一句提示）: {header}"
        );
        for line in body {
            assert!(line.starts_with(' '), "门内行有缩进（它在门里）: {line}");
        }
    }

    // 门之外那组：恰好 OS 自有那六件，一个不多一个不少。
    let system_verbs: Vec<&str> = sections[0]
        .1
        .iter()
        .filter_map(|line| verb_of(line))
        .collect();
    assert_eq!(
        system_verbs,
        vec![
            "open_app",
            "inspect_action",
            "pin_action",
            "unpin_action",
            "write_self_concept",
            "write_handoff"
        ],
        "门之外那组 = OS 自有那六件（不归任何 App）: {directory}"
    );

    // 门内动词**归属正确**：notes 那 13 条落在【notes】门里、web 那 2 条落在【web】门里。
    let mut notes_verbs: Vec<&str> = sections[2]
        .1
        .iter()
        .filter_map(|line| verb_of(line))
        .collect();
    notes_verbs.sort_unstable();
    assert_eq!(
        notes_verbs,
        vec![
            // ⚠ 2026-10-13：追加与它的复核两条同刀进场（§3.79）；
            //   2026-10-14：就地改与它的复核同样两条同刀（§3.107）。
            "notes_append",
            "notes_create",
            "notes_delete",
            "notes_edit",
            "notes_list",
            "notes_pin",
            "notes_read",
            "notes_unpin",
            "notes_verify_appended",
            "notes_verify_deleted",
            "notes_verify_edited",
            "notes_verify_note",
            "notes_verify_pinned"
        ],
        "【notes】门里是它声明的全部动词（预装常开 ⇒ 一条都不缺）: {}",
        sections[2].1.join(" / ")
    );

    // **notes 的显示分组**：读 / 写 / 钉 / 复核。只改排列，13 条一条不少，每条只出现一次。
    let notes_body = sections[2].1.join("\n");
    let label_positions: Vec<usize> = ["  读：", "  写：", "  钉：", "  复核："]
        .iter()
        .map(|label| {
            notes_body
                .find(label)
                .unwrap_or_else(|| panic!("notes 目录缺组头 {label}: {notes_body}"))
        })
        .collect();
    assert!(
        label_positions.windows(2).all(|pair| pair[0] < pair[1]),
        "notes 组头顺序应是读 / 写 / 钉 / 复核: {notes_body}"
    );
    let ordered_notes: Vec<&str> = sections[2]
        .1
        .iter()
        .filter_map(|line| verb_of(line))
        .collect();
    assert_eq!(
        ordered_notes,
        vec![
            "notes_list",
            "notes_read",
            "notes_create",
            "notes_append",
            "notes_edit",
            "notes_delete",
            "notes_pin",
            "notes_unpin",
            "notes_verify_note",
            "notes_verify_appended",
            "notes_verify_edited",
            "notes_verify_deleted",
            "notes_verify_pinned",
        ],
        "notes 目录按读 / 写 / 钉 / 复核排列: {notes_body}"
    );

    let mut web_verbs: Vec<&str> = sections[4]
        .1
        .iter()
        .filter_map(|line| verb_of(line))
        .collect();
    web_verbs.sort_unstable();
    assert_eq!(
        web_verbs,
        vec!["web_fetch", "web_search"],
        "【web】门里是它声明的两条: {}",
        sections[4].1.join(" / ")
    );

    // **索引那一行的内容**（顺序照桌面原样 files / notes / timer / web；常开的给动作数、
    // 未打开的只给状态）。它是一句机械事实，不是推荐。
    assert!(
        index.contains("notes（常开，") && index.contains(" 个动作）"),
        "常开的门要带动作数: {index}"
    );
    assert!(index.contains("web（常开，"), "web 也预装常开: {index}");
    assert!(
        index.contains("files（未打开）"),
        "未打开的门只给状态: {index}"
    );
    assert!(index.contains("timer（未打开）"), "timer 也没打开: {index}");
    // 索引里的动作数**必须等于门内实际列出来的动词行数**（不写死 13 / 2）：
    // 门里多一条、索引少一条，就是同一件事有两处在说、而且说岔了。
    assert_eq!(
        index_count(index, "notes"),
        notes_verbs.len(),
        "notes 的动作数要与门内实际动词行数一致: {index}"
    );
    assert_eq!(
        index_count(index, "web"),
        web_verbs.len(),
        "web 的动作数要与门内实际动词行数一致: {index}"
    );
    // 反向：开没开由装配期事实决定，索引不许说反。
    assert!(
        !index.contains("notes（未打开）") && !index.contains("files（常开"),
        "常开的门不许被印成未打开，未打开的也不许被印成常开: {index}"
    );
    // **顺序照桌面原样**（files / notes / timer / web）：索引里各段的先后就是 `desktop.doors` 的
    // 先后，不排序、不按重要性重排。
    let positions: Vec<usize> = ["files", "notes", "timer", "web"]
        .iter()
        .map(|app| {
            index
                .find(&format!("{app}（"))
                .unwrap_or_else(|| panic!("索引里没有 {app}: {index}"))
        })
        .collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "索引顺序照桌面原样（不排序）: {index}"
    );

    // **门牌是招牌，不是说明书**：每张 ≤ 200 字节，且不点自己那节的动词名。
    // 这是“有需求时判断这里有没有我要的”那一条的机械守卫；谁再把动作卡抄进门牌，这里会红。
    for (header, body) in sections.iter().skip(1) {
        let (app, description) = header
            .strip_prefix('【')
            .and_then(|rest| rest.split_once('】'))
            .unwrap_or_else(|| panic!("门牌形如【app】描述: {header}"));
        assert!(
            description.len() <= 200,
            "{app} 的门牌超过 200 字节（{}）：{description}",
            description.len()
        );
        for line in body {
            if !line.contains(" | ") {
                continue;
            }
            let verb = line.trim().split(" | ").next().unwrap_or_default();
            assert!(
                !description.contains(verb),
                "{app} 的门牌不该点动词名 {verb}：{description}"
            );
        }
    }

    // **未展开的门也上榜**：门牌在，门后只有一句提示，而且它的**动词一条都不出现**。
    let files = &sections[1];
    assert_eq!(
        files.1.len(),
        1,
        "材料窗还没展开 ⇒ 门后只有一句提示: {directory}"
    );
    assert!(
        files.1[0].contains("还没打开") && files.1[0].contains("open_app"),
        "提示要**指名**开门那个动作（主体才知道拿什么去开）: {}",
        files.1[0]
    );
    // 判据只落在**动词行**上：`files_read` 这个名字在参数说明里当例子出现过（`inspect_action`
    // 那行），那不是"把它的动词摆出来" —— 摆出来的是**可调的动词行**。
    // TASK_75：**第二扇未展开的门**（timer：主体的第三件自持物）——门牌在、门后一句提示、
    // 动词一条都不出现。它与 files 同款（手动展开），因此这两扇门的行为必须一模一样。
    let timer = &sections[3];
    assert!(
        timer.0.starts_with("【timer】"),
        "门牌是 timer: {}",
        timer.0
    );
    assert_eq!(timer.1.len(), 1, "还没展开 ⇒ 门后只有一句提示: {directory}");
    assert!(
        timer.1[0].contains("还没打开"),
        "提示要说清「还没打开」: {}",
        timer.1[0]
    );
    assert!(
        !sections
            .iter()
            .flat_map(|(_, body)| body.iter())
            .filter_map(|line| verb_of(line))
            .any(|verb| verb.starts_with("files_") || verb.starts_with("timer_")),
        "没展开的 App 的**动词行**一条都不许出现（不把未展开的动作伪装成能调的）: {directory}"
    );

    // ①b：App 清单**只活在组头**（骨架里那句"当前装配了：…"已移走）⇒ 目录里不再重复一份。
    assert!(
        !directory.contains("当前装配了"),
        "App 清单只活在组头，骨架里那份已移走（信息量不增、只重排）: {directory}"
    );
}

#[test]
fn the_reading_is_persisted_in_the_existing_injection_shape_and_never_becomes_a_fact() {
    let dir = TempDir::new("persisted");
    let mut environment = dir.environment();
    let mut session = open(&dir, &mut environment, 4);
    let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Completed)]);
    assert!(matches!(
        session.drive(&mut model).expect("驱动"),
        SessionDrive::Ended(_)
    ));
    drop(session);
    drop(environment);

    // 经历里：状态行就是既有的 `kind=injection` 形状（`stage`/`turn`/`blocks`），没有新种类。
    // ⚠ **2026-10-06 改**（校验和那一刀）：这条判据读**盘上的种类**，因此走生产读路径
    // （`read_trace_at`）——不再手解每一行（行尾现在带校验和，手解会撞上 trailing characters）。
    let trace = std::fs::read_to_string(dir.trace_path()).expect("读经历");
    let records = celira_agent::store::read_trace_at(&dir.env_root()).expect("读经历");
    let kinds: Vec<&str> = records.iter().map(|record| record.entry().kind()).collect();
    let known = [
        "run-start",
        "run-input",
        "injection",
        "action-selected",
        "observed",
        "model-response",
        "ending",
        "sealed",
    ];
    for kind in &kinds {
        assert!(
            known.contains(kind),
            "出现了既有词汇之外的持久种类 {kind}: {kinds:?}"
        );
    }
    let injections = kinds.iter().filter(|kind| **kind == "injection").count();
    assert_eq!(injections, 2, "开场一条 + 收场那一轮一条: {kinds:?}");
    assert!(
        trace.contains(STATUS_BAR_LABEL) && trace.contains("turn = 1/4"),
        "盘点里必须留着那条状态行"
    );

    // 事实里：一条状态行都没有——它是经历（当前处境），不是事实（发生过什么）。
    // 判据取状态行的两个字段行本身，而不是"status-bar"这个词：身份标识里恰好含这个词。
    let facts = std::fs::read_to_string(dir.facts_path()).expect("读事实");
    assert!(
        !facts.contains("time = ") && !facts.contains("turn = "),
        "状态行不得进入事实系统: {facts}"
    );
}

// ---------------------------------------------------------------------------
// 判据 3：轮次读数**从本段自己的记录**算，不重置
// ---------------------------------------------------------------------------
//
// ⚠ **一条用例随路退役**（2026-09-26）：这里原来有
// `a_resumed_activity_reads_its_turns_from_the_records_instead_of_starting_over`——
// 它拿**跨进程恢复**（`RunSession::resume`：顶着同一个 RunId 接着跑）当载体，断言"恢复之后
// 第一次驱动是第 2 轮，不是第 1 轮"。
//
// **旧规矩**：那一次连续没断，恢复出来的活动**接着数**上一段已经花掉的轮次。
// **新规矩**：承载活动的进程没了就是两个 Run——那一次连续就断了；
// 新进程起来是**新的一段**，它**从第 1 轮开始**（"接着数"这件事已经没有主语了）。
// ⇒ 不变量随路退役，**不搬家**：没有一条新路需要"轮次接着数"。
//
// 本判据**留下的那一半**仍由下面这条用例守着：同一个会话里跨**等待**再驱动，轮次接着算。

#[test]
fn a_waiting_run_reads_its_turns_from_the_records_instead_of_starting_over() {
    let dir = TempDir::new("waiting-counts");
    let mut environment = dir.environment();
    let mut session = open(&dir, &mut environment, 4);
    let mut model = ScriptedModel::new(vec![
        ModelOutput::say("等"),
        ModelOutput::finish(SubjectEnding::Completed),
    ]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());
    assert!(matches!(
        session.drive(&mut model).expect("第二次驱动"),
        SessionDrive::Ended(_)
    ));

    let injections = injections(&session);
    let (stage, turn, blocks) = injections
        .iter()
        .find(|(stage, turn, _)| *stage == InjectionStage::Turn && *turn == Some(2))
        .expect("同一个会话里的第二次驱动是第 2 轮");
    assert_eq!(*stage, InjectionStage::Turn);
    assert_eq!(*turn, Some(2), "跨等待之后轮次接着算，不重置成 1");
    assert_eq!(
        blocks[0].1,
        status_line(0, 2, 4, "unknown"),
        "轮次从本段已确认记录接着算，不重置成 0 或 1"
    );
}

// ---------------------------------------------------------------------------
// 判据 4：固定钟下逐字节相同；时间取 RFC 3339 UTC
// ---------------------------------------------------------------------------

#[test]
fn the_same_input_at_the_same_instant_is_byte_identical() {
    let first = TempDir::new("same-input-a");
    let second = TempDir::new("same-input-b");

    for dir in [&first, &second] {
        let mut environment = dir.environment();
        let mut session = open(dir, &mut environment, 4);
        let mut model = ScriptedModel::new(vec![
            ModelOutput::say("等"),
            ModelOutput::finish(SubjectEnding::Completed),
        ]);
        assert!(session.drive(&mut model).expect("驱动").is_unsettled());
        assert!(matches!(
            session.drive(&mut model).expect("驱动"),
            SessionDrive::Ended(_)
        ));
    }

    let a = std::fs::read(first.trace_path()).expect("读第一份经历");
    let b = std::fs::read(second.trace_path()).expect("读第二份经历");
    assert_eq!(a, b, "同一时刻同一输入必须是逐字节相同的经历");
}

#[test]
fn the_time_column_is_the_instant_the_clock_gave() {
    // RFC 3339 UTC（秒精度），闰年、世纪闰年与跨日边界都在里面。
    let table: [(u64, &str); 8] = [
        (0, "1970-01-01T00:00:00Z"),
        (86_399, "1970-01-01T23:59:59Z"),
        (86_400, "1970-01-02T00:00:00Z"),
        (951_782_400, "2000-02-29T00:00:00Z"),
        (1_000_000_000, "2001-09-09T01:46:40Z"),
        (1_709_208_000, "2024-02-29T12:00:00Z"),
        (1_758_000_000, "2025-09-16T05:20:00Z"),
        (4_102_444_800, "2100-01-01T00:00:00Z"),
    ];
    for (seconds, expected) in table {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
        assert_eq!(format_utc(at), expected, "{seconds} 秒");
    }

    // 读不出来的时刻读作 unknown：不编一个时间。
    let before_epoch = SystemTime::UNIX_EPOCH - Duration::from_secs(1);
    assert_eq!(format_utc(before_epoch), "unknown");
}

/// **本机时刻那一格**：同一个瞬间的**另一种写法**（带本机偏移）。
///
/// 三条判据一起判：① 偏移**真的在用**（换偏移 ⇒ 换结果）② 跨日 / 半时区 / 负偏移 / 跨年都对
/// ③ **与 [`format_utc`] 是同一个瞬间**——把偏移加回去，两种写法逐字节相等。
#[test]
fn the_local_time_column_carries_the_machine_offset_and_is_the_same_instant() {
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_758_000_000); // 2025-09-16T05:20:00Z

    // 判据 ①：同一个瞬间、不同偏移 ⇒ 不同结果（偏移**真的进了**那一格，不是摆设）。
    assert_eq!(
        format_local(at, LocalOffset::Minutes(8 * 60)),
        "2025-09-16T13:20:00+08:00"
    );
    assert_ne!(
        format_local(at, LocalOffset::Minutes(0)),
        format_local(at, LocalOffset::Minutes(8 * 60)),
        "偏移必须真的改变那一格"
    );

    // 判据 ②：跨日（西半球往回一天）、半时区、负偏移、跨年、闰日。
    let table: [(u64, i32, &str); 6] = [
        (0, 480, "1970-01-01T08:00:00+08:00"),
        (0, -300, "1969-12-31T19:00:00-05:00"),
        (0, 330, "1970-01-01T05:30:00+05:30"),
        (86_399, 480, "1970-01-02T07:59:59+08:00"),
        (1_709_208_000, -300, "2024-02-29T07:00:00-05:00"),
        (4_102_444_800, 480, "2100-01-01T08:00:00+08:00"),
    ];
    for (seconds, minutes, expected) in table {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
        assert_eq!(
            format_local(at, LocalOffset::Minutes(minutes)),
            expected,
            "{seconds} 秒 / {minutes} 分"
        );
    }

    // 判据 ③：**同一瞬间**——本地写法 ＝ 把同一段偏移加回那个瞬间之后的 UTC 写法。
    // ⚠ 这一条不靠肉眼：`offset_suffix` 是用例侧**独立**拼出来的对照写法。
    for (seconds, minutes, _) in table {
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
        let shifted = if minutes >= 0 {
            at + Duration::from_secs(minutes as u64 * 60)
        } else {
            at - Duration::from_secs((-minutes) as u64 * 60)
        };
        if shifted < SystemTime::UNIX_EPOCH {
            // ⚠ 这一格**故意留白**：`format_utc` 对纪元之前的时刻读作 `unknown`——那不是
            // "同一个瞬间的另一种写法"，是"读不出来" ⇒ 等价性没法用那种写法表达。
            // 边界那一行（本地日期落到 1969-12-31）因此只由判据 ② 的逐字节期望值钉住。
            continue;
        }
        assert_eq!(
            format_local(at, LocalOffset::Minutes(minutes)),
            format_utc(shifted).replace('Z', &offset_suffix(minutes)),
            "{seconds} 秒 / {minutes} 分 必须是同一瞬间"
        );
    }
}

/// 偏移的 `±hh:mm` 后缀——**用例侧独立**的对照写法（不调用被测的那个格式化点）。
fn offset_suffix(minutes: i32) -> String {
    let sign = if minutes < 0 { '-' } else { '+' };
    format!("{sign}{:02}:{:02}", minutes.abs() / 60, minutes.abs() % 60)
}

/// **读不出来 ≠ 就在 UTC**：没有臂 / 系统调用失败时退回 `Z`，**不写 `+00:00`**。
#[test]
fn an_unreadable_offset_falls_back_to_utc_and_never_pretends_to_be_local() {
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_758_000_000);
    assert_eq!(format_local(at, LocalOffset::Unreadable), format_utc(at));
    assert_eq!(
        format_local(at, LocalOffset::Unreadable),
        "2025-09-16T05:20:00Z"
    );
    assert!(
        !format_local(at, LocalOffset::Unreadable).contains('+'),
        "读不出来时不许出现偏移"
    );
    // 真的在 UTC 的机器是**另一回事**：那是读出来了、值是 0 ⇒ 写 `+00:00`（一个真陈述）。
    assert_eq!(
        format_local(at, LocalOffset::Minutes(0)),
        "2025-09-16T05:20:00+00:00"
    );
    // 早于纪元的时刻照旧读作 unknown（与 format_utc 同一条纪律）。
    let before_epoch = SystemTime::UNIX_EPOCH - Duration::from_secs(1);
    assert_eq!(
        format_local(before_epoch, LocalOffset::Minutes(480)),
        "unknown"
    );
}

/// **读世界给的时刻**：它真发过的两种形状都要读得出来；读不出来就 `None`（**不猜**）。
///
/// ⚠ 顺带把两个民用日期算法的**互逆性**也验了：`parse_rfc3339` 走 `days_from_civil`、
/// `format_utc` 走 `civil_from_days` ⇒ 读回来再格式化出去，逐字节相同即互逆。
#[test]
fn the_world_timestamp_is_read_in_the_shapes_it_really_arrives_in() {
    // 真机形状（`Z` ＋ 毫秒）与契约/夹具形状（`+00:00`）。
    assert_eq!(
        format_utc(parse_rfc3339("2026-10-09T17:47:12.345Z").expect("真机形状")),
        "2026-10-09T17:47:12Z"
    );
    assert_eq!(
        format_utc(parse_rfc3339("2026-09-16T11:00:00+00:00").expect("零偏移形状")),
        "2026-09-16T11:00:00Z"
    );
    // **偏移真的被算进去**：`+08:00` 的 19:00 就是 UTC 的 11:00。
    assert_eq!(
        format_utc(parse_rfc3339("2026-09-16T19:00:00+08:00").expect("带偏移")),
        "2026-09-16T11:00:00Z"
    );
    // 互逆（纪元、闰日、世纪闰年、2100 都在里面）。
    for text in [
        "1970-01-01T00:00:00Z",
        "2000-02-29T12:00:00Z",
        "2024-02-29T23:59:59Z",
        "2100-01-01T00:00:00Z",
    ] {
        assert_eq!(format_utc(parse_rfc3339(text).expect(text)), text, "{text}");
    }
    // 读不出来就 `None`：空、占位符、缺偏移、形状错、月份越界、纪元之前。
    for text in [
        "",
        "-",
        "unknown",
        "2026-09-16 11:00:00Z",
        "2026-09-16T11:00:00",
        "2026-13-01T00:00:00Z",
        "1969-12-31T23:59:59Z",
    ] {
        assert_eq!(parse_rfc3339(text), None, "{text}");
    }
}

/// **摆给她的世界时刻**：换成本机刻度之后**仍是同一个瞬间**（这条判据与机器无关）。
#[test]
fn the_world_timestamp_is_shown_in_the_machines_clock_and_stays_the_same_instant() {
    for text in ["2026-10-09T17:47:12Z", "2026-09-16T11:00:00+00:00"] {
        let shown = localize_rfc3339(text);
        // 把显示的偏移读回来 ⇒ 必须还是那个瞬间。
        assert_eq!(
            format_utc(parse_rfc3339(&shown).expect(&shown)),
            format_utc(parse_rfc3339(text).expect(text)),
            "{text} → {shown}"
        );
        // 形状只有两种：**原样**（本机偏移读不出来）或**带 `±hh:mm`**。
        if shown != text {
            let tail = &shown[shown.len().saturating_sub(6)..];
            assert!(
                (tail.starts_with('+') || tail.starts_with('-')) && tail.as_bytes()[3] == b':',
                "换过之后必须是 ±hh:mm: {shown}"
            );
        }
    }
    // **读不出来的原文原样返回**：宁可摆一个没换过、但完整的原文，也不吞掉一个时刻。
    for text in ["t", "-", "unknown", ""] {
        assert_eq!(localize_rfc3339(text), text);
    }
}

/// **锚那一栏的值**：十进制 unix 毫秒（2026-09-17 裁决——主体要能把它原样递给自己的动词）。
///
/// 形状本身（裸的十进制、无装饰）与"递回去能用"是两件事，判在一起：
/// 这里判形状与两个边界，`tests/im_app.rs` 里那条判**可用性**（同一个值过参数网关进查询串）。
#[test]
fn the_anchor_column_is_a_bare_millisecond_value() {
    let at = SystemTime::UNIX_EPOCH + Duration::from_millis(1_789_625_378_407);
    assert_eq!(anchor_value(at), "1789625378407");
    // 秒的整倍数也要是**毫秒**（不是"秒 + 三个零"被省掉的形态）：纪元、整秒、亚秒。
    assert_eq!(anchor_value(SystemTime::UNIX_EPOCH), "0");
    assert_eq!(
        anchor_value(SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
        "1800000000000"
    );
    assert_eq!(
        anchor_value(SystemTime::UNIX_EPOCH + Duration::from_millis(1)),
        "1"
    );
    // 前纪元读作 unknown（`since` 不收负数：与其给一个自己的动词会拒绝的数，
    // 不如如实说读不出来——与 format_utc 同一条纪律）。
    assert_eq!(
        anchor_value(SystemTime::UNIX_EPOCH - Duration::from_secs(1)),
        "unknown"
    );
}

#[test]
fn the_clock_is_read_every_turn_and_not_frozen_at_the_assembly() {
    // 一个**递增钟**：每一次取值都比上一次晚一小时。它证明 `time` 是每轮现算的，而不是
    // 装配时取一次、之后照抄（固定钟看不出这两者的差别）。
    struct Advancing {
        calls: std::sync::atomic::AtomicU64,
    }
    impl celira_agent::clock::Clock for Advancing {
        fn now(&self) -> SystemTime {
            let step = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            SystemTime::UNIX_EPOCH + Duration::from_secs(step * 3_600)
        }
    }

    let dir = TempDir::new("advancing-clock");
    let mut environment = dir.environment();
    let materials = dir.materials();
    let declared = agent_id();
    let mut session = RunSession::open(
        &materials,
        &mut environment,
        SessionRequest {
            declarations: SessionDeclarations::default(),
            declared_agent: &declared,
            run: RunId::new(RUN),
            limits: Limits::new(4),
            input: RunInput::text("开场输入").expect("在上限内"),
            clock: ClockHandle::of(Advancing {
                calls: std::sync::atomic::AtomicU64::new(0),
            }),
            context_window: CONTEXT_WINDOW,
            reading: reading_cell(),
        },
    )
    .expect("会话可以打开");
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等"), ModelOutput::say("再等")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let texts: Vec<String> = injections(&session)
        .iter()
        .flat_map(|(_, _, blocks)| blocks.iter().map(|(_, text)| text.clone()))
        .filter(|text| text.starts_with("time = "))
        .collect();
    // ⚠️ **2026-10-06 改了这里的断言【方式】，理由必须写清楚** —— 改断言是重罪，
    // 所以要说清它为什么**不是**"替实现过关"。
    //
    // 本条要证的**不变量**是「`time` 每轮现算，不是装配时取一次照抄」。原来它用一个**更强的**
    // 断言来表达：三行读数的**具体值**（01:00 / 02:00 / 03:00）。
    //
    // 那些具体值之所以正好是"每轮 +1h"，靠的是一个**与本条无关的巧合**：夹具钟
    // （[`Advancing`]）**每被读一次就前进一小时**，而当时**每一轮恰好只读一次钟**。
    //
    // 自 2026-10-06 起，**每写一条记录也读一次钟**（`at-ms`，「这一行是什么时候写的」，
    // 见 `agent/src/store.rs` 的 `AT_FIELD`）—— ⇒ 那个巧合没了：具体值变成"取决于这一轮
    // 写了几条记录"，而**那是一个与本条无关的量**。继续钉具体值，等于把一条**关于时间的判据**
    // 绑死在"每轮写几条记录"上——以后每加一种记录都会让它假红一次。
    //
    // 所以改成**直接断言那条不变量本身**：三行读数**严格递增**（每轮现算 ⇒ 必递增；
    // 冻在装配 ⇒ 三行逐字相同）。**没有放宽**：形状（轮次、上下文读数、`since`）仍逐字钉死。
    //
    // （会话打开那一刻仍会读一次钟 —— `clock_at_open`，算时长用；它消耗递增钟的第 0 轮。
    //  这不影响本条：本条只问"三行之间是否递增"。）
    let times: Vec<&str> = texts
        .iter()
        .map(|text| text.lines().next().expect("状态行有第一行"))
        .collect();
    for pair in times.windows(2) {
        // RFC 3339 的零填充定长写法下，**字典序就是时序** ⇒ 直接比字符串。
        assert!(
            pair[0] < pair[1],
            "每一轮都重新取一次时间（读数必须递增）：{} → {}",
            pair[0],
            pair[1]
        );
    }
    // 形状仍然逐字钉死：轮次与上下文读数一个都不许变。
    let shape: Vec<String> = texts
        .iter()
        .map(|text| text.lines().skip(1).collect::<Vec<_>>().join("\n"))
        .collect();
    assert_eq!(
        shape,
        vec![
            "turn = 0/4\ncontext = unknown/1000 token\nsince = unknown",
            "turn = 1/4\ncontext = unknown/1000 token\nsince = unknown",
            "turn = 2/4\ncontext = unknown/1000 token\nsince = unknown",
        ],
        "每一轮都重新取一次时间"
    );
}

// ---------------------------------------------------------------------------
// 两处批次都递交这条纪律：合成的唯一规则可判错（两条例外都指向"内容不被静默吞掉"）
// ---------------------------------------------------------------------------

/// **验收复核补的一条**：宿主输入接近批次上限时，让位的必须是**本件自己的读数**。
///
/// 这是那条"超限交回内层/别人的内容"规则在生产路径上的可达性探针：`provide` 允许的输入与
/// 状态行的块合起来正好会越过 `ContentBatch::MAX_BYTES`，此时若让位的是宿主输入，那段文本会被
/// **静默吞掉**（`pending` 已经在 `take` 里消耗掉了，宿主不会得到任何错误）。
#[test]
fn a_host_input_near_the_batch_limit_is_still_delivered() {
    let text = "x".repeat(65_500);
    let dir = TempDir::new("host-input-limit");
    let mut environment = dir.environment();
    let mut session = open(&dir, &mut environment, 2);
    session
        .provide(RunInput::text(text.clone()).expect("在输入上限内"))
        .expect("排入应当被接受");
    let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Completed)]);
    assert!(matches!(
        session.drive(&mut model).expect("驱动"),
        SessionDrive::Ended(_)
    ));

    let (_, injections) = &model.seen[0];
    let delivered: Vec<&str> = injections
        .iter()
        .flat_map(|(_, blocks)| blocks.iter())
        .map(|(_, text)| text.as_str())
        .collect();
    assert!(
        delivered.iter().any(|item| *item == text),
        "宿主显式交出的文本必须真的递进视野（它只有这一份）"
    );
    // 状态是当前值：这一个极端用例里它可以让位，下一轮还会重摆。
    assert!(
        delivered.iter().any(|item| item.starts_with("time = ")),
        "开场那条读数仍然在: {delivered:?}"
    );
}

/// **首轮目录不是“下一轮还会重摆”的读数**：它和接近上限的宿主输入撞在一起时，
/// 这一拍先让位，但必须记下待补；下一轮装得下时补一次，不能静默丢。
#[test]
fn a_directory_that_does_not_fit_on_the_opening_turn_is_delivered_next_turn() {
    let host_text = "x".repeat(64_500);
    let dir = TempDir::new("directory-deferred");
    let mut environment = dir.environment();
    let mut session = open(&dir, &mut environment, 2);
    session
        .provide(RunInput::text(host_text.clone()).expect("在输入上限内"))
        .expect("排入应当被接受");
    let mut model = ScriptedModel::new(vec![
        ModelOutput::say("先看看"),
        ModelOutput::finish(SubjectEnding::Completed),
    ]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());
    assert!(matches!(
        session.drive(&mut model).expect("驱动"),
        SessionDrive::Ended(_)
    ));

    let (_, first_injections) = &model.seen[0];
    let first: Vec<(&str, &str)> = first_injections
        .iter()
        .flat_map(|(_, blocks)| blocks.iter())
        .map(|(label, text)| (label.as_str(), text.as_str()))
        .collect();
    assert!(
        first.iter().any(|(_, text)| *text == host_text.as_str()),
        "宿主输入这一份必须真的递进视野"
    );

    let (_, second_injections) = &model.seen[1];
    let second: Vec<(&str, &str)> = second_injections
        .iter()
        .flat_map(|(_, blocks)| blocks.iter())
        .map(|(label, text)| (label.as_str(), text.as_str()))
        .collect();
    assert!(
        !first
            .iter()
            .any(|(label, body)| *label == "available-actions" && body.contains("notes_list")),
        "这一拍装不下 ⇒ 完整目录先让位: {first:?}"
    );
    assert!(
        first
            .iter()
            .any(|(label, body)| *label == "available-actions" && !body.contains("notes_list")),
        "让位不能无声：原地要留一句读数: {first:?}"
    );
    assert!(
        second
            .iter()
            .any(|(label, body)| *label == "available-actions" && body.contains("notes_list")),
        "下一轮装得下 ⇒ 目录补一次: {second:?}"
    );
}

#[test]
fn combining_two_batches_keeps_both_sides_in_order() {
    let inner = ContentBatch::of(vec![ContentBlock::new("activity-origin", "来源")]).expect("合法");
    let own = ContentBatch::of(vec![ContentBlock::new(STATUS_BAR_LABEL, "读数")]).expect("合法");

    let combined = combine(inner, own);
    let labels: Vec<&str> = combined
        .blocks()
        .iter()
        .map(|block| block.label())
        .collect();
    assert_eq!(
        labels,
        vec!["activity-origin", STATUS_BAR_LABEL],
        "内层先、外层后"
    );
    assert!(
        !combined.is_source_failure(),
        "两处都成功 ⇒ 整批不是失败读数"
    );

    // 例外一：内层报告"来源读不到"时原样交回——一条成功的读数不得被并进失败读数。
    let failure = ContentBatch::source_failure("activity-origin", "读不到").expect("合法");
    let combined = combine(
        failure,
        ContentBatch::of(vec![ContentBlock::new(STATUS_BAR_LABEL, "读数")]).expect("合法"),
    );
    assert!(combined.is_source_failure());
    assert_eq!(combined.blocks().len(), 1, "失败读数不被稀释");

    // 空批次是合法的：两处都空 ⇒ 空批次（Core 不因此产生任何记录）。
    assert!(combine(ContentBatch::empty(), ContentBatch::empty()).is_empty());

    // 例外二：拼接后超过批次上限时交回内层——状态行下一轮还有，别人的内容只有这一份。
    //
    // 这一批**正好顶到块数上限**，因此连"让位者留名"的那句读数都塞不进去：这是例外二里
    // 退无可退的那个角。读数塞得下时交回的是"内层 + 一句读数"，两条都在
    // `tests/honest_block_fallback.rs` 里钉着。
    let full = ContentBatch::of(
        (0..ContentBatch::MAX_BLOCKS)
            .map(|index| ContentBlock::new(format!("block-{index}"), "x"))
            .collect(),
    )
    .expect("正好在上限内");
    let combined = combine(
        full,
        ContentBatch::of(vec![ContentBlock::new("x", "y")]).expect("合法"),
    );
    assert_eq!(
        combined.blocks().len(),
        ContentBatch::MAX_BLOCKS,
        "读数都塞不进去时，交回内层那一批、一个块都不动"
    );
    assert!(combined.blocks()[0].label() == "block-0", "交回的是内层");
}

// ---------------------------------------------------------------------------
// 件的形状：委托链、只插注入口、状态行正文的字段
// ---------------------------------------------------------------------------

#[test]
fn the_component_is_a_two_element_chain_over_the_existing_source() {
    let bar = StatusBar::new(
        fixed_clock(),
        TEST_OFFSET,
        CONTEXT_WINDOW,
        reading_cell(),
        ActivityOrigin::new(),
    );
    // 内层就是既有的处境来源（只读可查），说明合成形状是"外层持内层"而不是并列的两个件。
    let _inner: &ActivityOrigin = bar.inner();

    // 状态行正文的样子（判据 4 的期望形状）：时间 + 轮次比例 + 上下文占用（含单位词）+ 「新」的起点。
    assert_eq!(
        status_line(0, 3, 12, "unknown"),
        "time = 1970-01-01T08:00:00+08:00\nturn = 3/12\ncontext = unknown/1000 token\nsince = unknown"
    );
    assert_eq!(STATUS_BAR_LABEL, "status-bar");
}

/// 一个只报告"读不到"的来源形状无法在本文件里走完接缝（`ContextSource` 的两个方法都要一个
/// `InjectionContext`，而用例侧构造不出它——Core 不提供公开构造器，本刀也不许改 Core）。
/// 因此那条纪律由 [`combine`] 的直接用例覆盖：下面这一条给的就是**同一个函数**会拿到的那一批。
#[test]
fn a_failing_source_reading_is_never_diluted_by_a_healthy_one() {
    let failure = ContentBatch::source_failure("activity-origin", "坏掉").expect("合法");
    let own = ContentBatch::of(vec![ContentBlock::new(STATUS_BAR_LABEL, "读数")]).expect("合法");
    let combined = combine(failure, own);
    assert!(combined.is_source_failure(), "失败读数保持");
    assert_eq!(combined.blocks().len(), 1, "成功读数不并进失败读数");
    assert_eq!(combined.blocks()[0].label(), "activity-origin");
}

#[test]
fn every_injected_reading_is_four_field_lines_and_nothing_else() {
    let dir = TempDir::new("shape");
    let mut environment = dir.environment();
    let mut session = open(&dir, &mut environment, 2);
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let texts: Vec<String> = injections(&session)
        .iter()
        .flat_map(|(_, _, blocks)| blocks.iter().map(|(_, text)| text.clone()))
        .filter(|text| text.starts_with("time = "))
        .collect();
    assert_eq!(texts.len(), 2, "开场一条 + 本轮一条");
    for text in texts {
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "机械读数只有四行: {text}");
        let fields: Vec<&str> = lines
            .iter()
            .map(|line| line.split(" = ").next().expect("有字段名"))
            .collect();
        assert_eq!(
            fields,
            vec!["time", "turn", "context", "since"],
            "字段只有时间、轮次、上下文与「新」的起点: {text}"
        );
        // TASK_74：那一格的标签要**见名知意**——旧的工程比喻词（本文件之外的那个词）从主体的
        // 视野里消失，换成这个标签 ＋ 模型有原生先验的单位词。
        assert!(
            text.contains("context = ") && text.contains("token"),
            "上下文那一格要带标签与单位: {text}"
        );
        assert!(
            !text.contains("reading"),
            "工程比喻词不许再进主体的视野: {text}"
        );
        for line in lines {
            let (name, value) = line.split_once(" = ").expect("字段行形状是 `名 = 值`");
            assert!(
                !value.is_empty() && !name.contains(' '),
                "字段行不得空值或带空格: {text}"
            );
        }
    }
}

/// **说明书**（TASK_74）：开场**恰好一份**、Run 内不重复；四字段各一句 ＋ 撞墙后果 ＋ since 消费链接。
///
/// 判错即红：说明书写漏一个字段 ⇒ 红；它在轮级重摆里又冒出来一次 ⇒ 红（"说明书读一次，读数看千遍"）。
#[test]
fn the_reading_guide_lands_once_at_the_opening_and_never_repeats() {
    let dir = TempDir::new("guide");
    let mut environment = dir.environment();
    let mut session = open(&dir, &mut environment, 4);
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等"), ModelOutput::say("再等")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let guides: Vec<(InjectionStage, Option<u32>, String)> = injections(&session)
        .into_iter()
        .filter_map(|(stage, turn, blocks)| {
            blocks
                .into_iter()
                .find(|(label, _)| label == READING_GUIDE_LABEL)
                .map(|(_, text)| (stage, turn, text))
        })
        .collect();
    assert_eq!(guides.len(), 1, "整场只有一份说明书: {guides:?}");
    let (stage, turn, text) = &guides[0];
    assert_eq!(*stage, InjectionStage::RunStart, "它在开场那一拍");
    assert_eq!(*turn, None, "Run 级注入没有轮次");

    // 四字段各一句（每个字段名都作为行首出现）。
    for field in ["time", "turn", "context", "since"] {
        assert!(
            text.lines()
                .any(|line| line.starts_with(&format!("{field} = "))),
            "说明书要逐字段解释（缺 `{field}`）: {text}"
        );
    }
    // 撞墙后果：说清"撞上它这一场会断"，不是只给一个数。
    assert!(
        text.contains("撞上它这一场就会断"),
        "要说明撞墙后果: {text}"
    );
    // since 的消费链接：指名那个动词与那个参数（卡片已述参数，说明书只给链接）。
    assert!(
        text.contains("im_rooms(since=…)"),
        "要给出 since 怎么花的那条链接: {text}"
    );
    // **只解释、不报警**：没有阈值数字、没有"警告"这类替主体决定何时担心的话（已裁的三段式第一段）。
    assert!(
        !text.contains("警告") && !text.contains("注意") && !text.contains('%'),
        "说明书只解释、不报警: {text}"
    );
    assert_eq!(text, READING_GUIDE, "说明书就是那一份常量（逐字节）");
}

/// **自识**（TASK_70）：`<环境根>/self_concept` 里那一篇，开场**原文**摆出来。
/// 判错即红：写进去的没进开场 ⇒ 红；正文被加料/裁剪 ⇒ 红。
#[test]
fn the_self_concept_stands_in_the_opening_exactly_as_written() {
    let dir = TempDir::new("self_concept-opening");
    let mut environment = dir.environment();
    let text = "我是阿默：先看事实，再说话。\n";
    std::fs::write(dir.env_root().join(SELF_CONCEPT_FILE), text).expect("写下自识");

    let mut session = open(&dir, &mut environment, 4);
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let injections = injections(&session);
    let (_, _, blocks) = &injections[0];
    let found = blocks
        .iter()
        .find(|(label, _)| label == SELF_CONCEPT_LABEL)
        .expect("自识那一块应当在开场");
    assert_eq!(found.1, text, "原文摆出——一个字都不加、一个字都不减");
}

/// **没有自识**（新生儿）：开场**优雅跳过**——不注入空壳、连标签都不出现。
#[test]
fn an_environment_without_a_self_concept_says_nothing_at_all() {
    let dir = TempDir::new("self_concept-absent");
    let mut environment = dir.environment();

    let mut session = open(&dir, &mut environment, 4);
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let injections = injections(&session);
    let (_, _, blocks) = &injections[0];
    assert!(
        blocks.iter().all(|(label, _)| label != SELF_CONCEPT_LABEL),
        "没有那一份就一个字都不说（不注入空壳）: {blocks:?}"
    );
}

/// **身份行**（TASK_70）：宿主声明的两个值原样摆出来，**紧跟自识**。
#[test]
fn the_identity_line_stands_after_the_self_concept_in_the_opening() {
    let dir = TempDir::new("identity-opening");
    let mut environment = dir.environment();
    std::fs::write(dir.env_root().join(SELF_CONCEPT_FILE), "我是阿默。\n").expect("写下自识");

    let mut session = open_with_identity(
        &dir,
        &mut environment,
        4,
        Some(IdentityClaim {
            id: "core-1".to_string(),
            display_name: "小满".to_string(),
        }),
    );
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let injections = injections(&session);
    let (_, _, blocks) = &injections[0];
    let names: Vec<&str> = blocks.iter().map(|(label, _)| label.as_str()).collect();
    let self_concept_at = names
        .iter()
        .position(|n| *n == SELF_CONCEPT_LABEL)
        .expect("自识在场");
    let identity_at = names
        .iter()
        .position(|n| *n == "identity")
        .expect("身份行在场");
    assert_eq!(
        identity_at,
        self_concept_at + 1,
        "身份行紧跟在自识之后: {names:?}"
    );
    assert_eq!(
        blocks[identity_at].1, "你是 core-1，在此世界名为 小满",
        "两个值**原样**摆出来（不是本机身份、也不经过任何改写）"
    );
}

/// **问不到身份**：那一行整块跳过（不注入空壳）——缺席不占位。
#[test]
fn without_a_claim_the_identity_line_is_skipped() {
    let dir = TempDir::new("identity-absent");
    let mut environment = dir.environment();

    let mut session = open(&dir, &mut environment, 4);
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
    assert!(session.drive(&mut model).expect("驱动").is_unsettled());

    let injections = injections(&session);
    let (_, _, blocks) = &injections[0];
    assert!(
        blocks.iter().all(|(label, _)| label != "identity"),
        "没声明就没有那一行: {blocks:?}"
    );
}
