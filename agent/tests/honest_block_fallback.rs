//! 块级诚实回退：**内容装不下时，视野里要出现"它超限未给"这条读数**。
//!
//! ## 这一刀治的是什么
//!
//! `ContentBatch::of` 超限是**整体拒绝**（不产生半批，见 [`celira_agent::core::ContentBatch`]）。
//! 此前各调用点一律"退回空批次"（或"退回另一份"），于是那一整块内容**无声消失**：读主体的视野的人
//! 与自己都只看到"这里什么都没有"，分不清"没有"与"没读到"。本刀把它换成一条读数：
//! `<这个来源的标签> 超限未给（原始 N B）`。
//!
//! ## 这一份文件证明什么
//!
//! 证明的是**生产装配路径**上读数真的落进注入文本，不是"助手函数自己好用"：每一处**能真的
//! 超限**的来源，都用它自己的**真实入口**喂一个超限输入（写主体的自识文件、声明一段巨长的起源、
//! 声明一份巨长的投影、往事件卡片细胞里塞一个巨长的房名、把宿主输入顶到批次上限），
//! 然后读模型看得到的那一段。
//!
//! 结构上**不可能**超限的三处（纸条板 / 格位块 / 技能桌面：预览与条数都有上限，见交付报告里的
//! 界算术）在这里没有位置可构造，它们的证据是形状守卫
//! （`tests/minimal_agent.rs::a_batch_that_does_not_fit_is_never_silently_dropped`）。
//!
//! 时序（开场一次、之后不摆）等既有判据不在这里重复。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use celira_agent::clock::ClockHandle;
use celira_agent::composition::{RunOrigin, RunSession, SessionDeclarations, SessionRequest};
use celira_agent::context_reading::ContextReading;
use celira_agent::core::combine;
use celira_agent::core::{
    ContentBatch, ContentBatchError, ContentBlock, Limits, ModelError, ModelFailureClass,
    ModelOutput, ModelPort, RunInput, TraceEntry, View,
};
use celira_agent::event_cards::EventCardsReport;
use celira_agent::foundation::{AgentId, RunId};
use celira_agent::self_concept::SELF_CONCEPT_FILE;
use celira_agent::status_bar::STATUS_BAR_LABEL;
use celira_agent::store::{Environment, WakeOrigin};

const AGENT: &str = "agent-honest-fallback";
const RUN: &str = "run-honest-fallback";
/// 宿主声明的上下文窗口（状态行那一段的分母；与读数无关，取一个固定值）。
const WINDOW: u32 = 1_000;
/// 那一批的上限（拷自 `ContentBatch::MAX_BYTES`：本文件的算术围着它算）。
const CAP: usize = ContentBatch::MAX_BYTES;
/// 宿主输入那一块的标签（`composition.rs` 的端点契约；用例要按它算字节）。
const HOST_INPUT_LABEL: &str = "host-input";
/// 动作目录那一块的标签。
const DIRECTORY_LABEL: &str = "available-actions";
/// 一句读数的开头（与实现同源：`core.rs` 的 `NOT_DELIVERED`）。
const NOT_DELIVERED: &str = "超限未给";
/// "（原始 <数字> B）"那一段的**最短**形态（3 位数字），用来估一句读数的下限。
const READING_TAIL_MIN: usize = "（原始  B）".len() + 3;

// ---------------------------------------------------------------------------
// 自建测试目录 / 脚本模型（不指桌面、不指家目录、不指仓库根）
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
        path.push(format!("celira-honest-{}-{label}-{n}", std::process::id()));
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
        Environment::create(&self.env_root(), AgentId::new(AGENT)).expect("建立身份环境")
    }

    /// 往主体的地盘写一份自识（**自己的入口**：`<环境根>/self_concept`）。
    fn write_self_concept(&self, text: &str) {
        std::fs::create_dir_all(self.env_root()).expect("建环境根");
        std::fs::write(self.env_root().join(SELF_CONCEPT_FILE), text).expect("写自识");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// 脚本模型：按序交出事先写好的响应。这里只用它推进活动（不看它自己的状态）。
struct ScriptedModel {
    steps: VecDeque<ModelOutput>,
}

impl ScriptedModel {
    fn new(steps: Vec<ModelOutput>) -> Self {
        Self {
            steps: steps.into_iter().collect(),
        }
    }
}

impl ModelPort for ScriptedModel {
    fn complete(&mut self, _view: &View) -> Result<ModelOutput, ModelError> {
        self.steps.pop_front().ok_or(ModelError {
            class: ModelFailureClass::ContractViolation,
        })
    }
}

/// 打开一次会话（生产入口 `open_declared`，只有时间来源换成固定钟）。
fn session<'a>(
    dir: &TempDir,
    environment: &'a mut Environment,
    declared: RunOrigin<'_>,
    limits: u32,
) -> RunSession<'a> {
    let materials = dir.materials();
    let declared_agent = AgentId::new(AGENT);
    RunSession::open_declared(
        &materials,
        environment,
        declared,
        SessionRequest {
            declarations: SessionDeclarations::default(),
            declared_agent: &declared_agent,
            run: RunId::new(RUN),
            limits: Limits::new(limits),
            input: RunInput::text("开场输入").expect("在上限内"),
            clock: ClockHandle::fixed_at_unix_seconds(0),
            context_window: WINDOW,
            reading: ContextReading::default(),
        },
    )
    .expect("会话可以打开")
}

/// 轨迹里全部注入块：`(标签, 正文, 这一块的字节数)`，按发生顺序。
fn injected_blocks(run: &RunSession<'_>) -> Vec<(String, String, usize)> {
    run.trace()
        .iter()
        .flat_map(|entry| match entry {
            TraceEntry::Injection { batch, .. } => batch
                .blocks()
                .iter()
                .map(|block| {
                    (
                        block.label().to_string(),
                        block.text().to_string(),
                        block.label().len() + block.text().len(),
                    )
                })
                .collect::<Vec<_>>(),
            TraceEntry::Refused { .. }
            | TraceEntry::Observation(_)
            | TraceEntry::ModelResponse(_) => Vec::new(),
        })
        .collect()
}

/// 找第一块带这个标签的内容；找不到就是用例自己的前提没成立。
fn block(run: &RunSession<'_>, label: &str) -> (String, usize) {
    injected_blocks(run)
        .into_iter()
        .find(|(found, _, _)| found == label)
        .map(|(_, text, bytes)| (text, bytes))
        .unwrap_or_else(|| panic!("注入里没有 {label} 这一块"))
}

/// 某一块的字节大小（找不到就是前提没成立）。
fn size_of(run: &RunSession<'_>, label: &str) -> usize {
    block(run, label).1
}

/// 驱动一次，要求它以"等"收尾（活动还在）。
fn drive_waiting(session: &mut RunSession<'_>) {
    let mut model = ScriptedModel::new(vec![ModelOutput::say("等")]);
    assert!(
        session.drive(&mut model).expect("驱动成功").is_unsettled(),
        "这一次应当以等待收尾"
    );
}

/// 一句读数的形状：`超限未给（原始 N B）`——把 N 抠出来。
fn reading_bytes(text: &str) -> usize {
    let rest = text
        .strip_prefix(NOT_DELIVERED)
        .unwrap_or_else(|| panic!("这不是一句缺块读数: {text}"));
    let rest = rest
        .strip_prefix("（原始 ")
        .unwrap_or_else(|| panic!("缺了原始规模那一段: {text}"));
    let digits = rest
        .strip_suffix(" B）")
        .unwrap_or_else(|| panic!("缺了字节单位那一段: {text}"));
    digits
        .parse()
        .unwrap_or_else(|_| panic!("原始规模不是数字: {text}"))
}

/// 一句读数**最多**占多少字节（标签 + 那句话 + 5 位数字）。用例用它算"塞不塞得下"。
fn reading_upper_bound(label: &str) -> usize {
    label.len() + NOT_DELIVERED.len() + READING_TAIL_MIN + 2
}

// ---------------------------------------------------------------------------
// 一、能真的超限的来源：走它们自己的真实入口，读注入文本
// ---------------------------------------------------------------------------

/// **自识**（自己写的、逐字整份）是这一批里唯一可能真超 64 KiB 的那一块：主体写多少就是多少。
///
/// 它必须**留名**：超限即自识静默消失，是这一刀里最贵的一种丢法。
#[test]
fn an_own_self_concept_over_the_batch_limit_leaves_a_reading_instead_of_vanishing() {
    const SELF_CONCEPT_BYTES: usize = 70_000;
    let dir = TempDir::new("self_concept");
    // ⚠ **顺序即事实**（2026-10-06）：环境先存在，自识才写得进去——`create` 只在一个空位置上创建
    // （身份不在却有别的东西 ⇒ 明确拒绝，见 README §3.3）。
    let mut environment = dir.environment();
    dir.write_self_concept(&"s".repeat(SELF_CONCEPT_BYTES));

    let mut session = session(&dir, &mut environment, RunOrigin::manual(), 2);
    drive_waiting(&mut session);

    let (text, bytes) = block(&session, "self_concept");
    assert_eq!(
        text,
        format!(
            "{NOT_DELIVERED}（原始 {} B）",
            "self_concept".len() + SELF_CONCEPT_BYTES
        ),
        "读数要写清那份自识原本多大"
    );
    assert_eq!(bytes, "self_concept".len() + text.len());

    // 也不许换个地方"半给"：整份自识一个字节都没进视野。
    for (label, body, _) in injected_blocks(&session) {
        assert!(
            body.len() < SELF_CONCEPT_BYTES,
            "{label} 里出现了超限自识的一部分（长度 {}）",
            body.len()
        );
    }
}

/// **起源那句现在是词表派生的**（刀三 A）：`RunOrigin::origin` 收成了封闭词表
/// （[`WakeOrigin`]），盘上/装配里存的是词表值，人话由渲染点当场派生
/// （[`WakeOrigin::text`]）。
///
/// 这带来一条**形状上的后果**，本条如实钉住它：起源那一块**不再可能超限**——正文是几个
/// 固定常量之一（最长的门铃那句也只有一百多字节，远在 [`ContentBatch`] 的 64 KiB 之下），
/// 因此它**总是**带着派生出来的人话落地，不会走"超限未给"那条路。
///
/// 换句话说：刀三之前，起源是宿主任意声明的一句字符串，所以"巨长的起源被如实报成读数"
/// 是一条要被证明的纪律；刀三之后这句话**构造不出来**——本条的判据从"超限留名"改成
/// "派生正文原样落地"。超限留名那条纪律没有被削弱：它由同文件里通知栏 / 事件卡片 / 自识
/// 三条**仍能造出超限内容**的用例继续守着。
#[test]
fn a_vocabulary_origin_always_lands_its_derived_text() {
    let dir = TempDir::new("origin");
    let mut environment = dir.environment();

    let declared = RunOrigin {
        origin: WakeOrigin::Manual,
        since: None,
        notification: None,
        cards: None,
    };
    let mut session = session(&dir, &mut environment, declared, 2);
    drive_waiting(&mut session);

    let (text, bytes) = block(&session, "activity-origin");
    assert_eq!(
        text,
        celira_agent::activity_origin::MANUAL_ORIGIN_TEXT,
        "起源那一块带的是**同一个词表值**派生出来的人话"
    );
    assert_eq!(
        bytes,
        "activity-origin".len() + celira_agent::activity_origin::MANUAL_ORIGIN_TEXT.len(),
        "读数如实：标签 + 派生正文的长度"
    );
    assert!(
        bytes < ContentBatch::MAX_BYTES,
        "词表派生的正文**构造不出**超限这一块：{bytes} 应远小于上限"
    );
}

/// **通知栏**的正文是宿主读回来的投影（每间房几条）：本层只搬运、不解析、不预览。
///
/// 它超限时是"这一段内容真的没有"，不是"没有通知"。
#[test]
fn a_notification_projection_over_the_batch_limit_leaves_a_reading() {
    const HUGE: usize = 80_000;
    let small = "rooms = 1";

    // 先用一条**同形状的小投影**量出固定前缀（`since = <锚>\n`），再把投影撑大：
    // 期望的原始规模 = 标签 + 前缀 + 投影，用例里因此不写死那句前缀。
    //
    // `since` 必须给：它是"新"的起点，缺了它通知栏就读作 `since = unknown`（**如实说不知道**，
    // 于是那一段投影根本不会被渲染）。
    let since = Some(std::time::SystemTime::UNIX_EPOCH);
    let control_dir = TempDir::new("notification-control");
    let mut control_env = control_dir.environment();
    let mut control = session(
        &control_dir,
        &mut control_env,
        RunOrigin {
            origin: WakeOrigin::Manual,
            since,
            notification: Some(small),
            cards: None,
        },
        2,
    );
    drive_waiting(&mut control);
    let (control_text, control_bytes) = block(&control, "notification-bar");
    assert_eq!(control_bytes, "notification-bar".len() + control_text.len());
    let prefix = control_text.len() - small.len();

    let dir = TempDir::new("notification");
    let mut environment = dir.environment();
    let projection = "r".repeat(HUGE);
    let mut session = session(
        &dir,
        &mut environment,
        RunOrigin {
            origin: WakeOrigin::Manual,
            since,
            notification: Some(&projection),
            cards: None,
        },
        2,
    );
    drive_waiting(&mut session);

    let (text, _) = block(&session, "notification-bar");
    assert_eq!(
        reading_bytes(&text),
        "notification-bar".len() + prefix + HUGE,
        "投影的原始字节要如实报出来: {text}"
    );
}

/// **事件卡片**里的房名由宿主并进细胞（本件不预览它）：一个巨长的房名就能把这一块顶出上限。
#[test]
fn event_cards_over_the_batch_limit_leave_a_reading() {
    const NAME_BYTES: usize = 80_000;
    let dir = TempDir::new("cards");
    let mut environment = dir.environment();

    let (writer, report) = EventCardsReport::channel();
    writer.merge(&[("r".repeat(NAME_BYTES), 3_i64)]);

    let mut session = session(
        &dir,
        &mut environment,
        RunOrigin {
            origin: WakeOrigin::Manual,
            since: None,
            notification: None,
            cards: Some(report),
        },
        2,
    );
    drive_waiting(&mut session);

    let (text, _) = block(&session, "event-cards");
    let reported = reading_bytes(&text);
    assert!(
        reported > NAME_BYTES,
        "读数要报出那块原始内容（至少含那个房名）: {reported} vs {NAME_BYTES}"
    );
    assert!(
        !text.contains(&"r".repeat(1_000)),
        "超限的卡片不许被截一半递出去"
    );
}

// ---------------------------------------------------------------------------
// 二、自身读数有界、但会**让位**的那两处：让位时要留下自己的名字
// ---------------------------------------------------------------------------

/// 量出首轮那两块的真实字节数：`(状态行, 动作目录)`。用例据此算"刚好顶到上限"的输入长度，
/// 不写死魔法常量。
fn measure_opening_blocks(dir: &TempDir) -> (usize, usize) {
    let mut environment = dir.environment();
    let mut session = session(dir, &mut environment, RunOrigin::manual(), 2);
    drive_waiting(&mut session);
    (
        size_of(&session, STATUS_BAR_LABEL),
        size_of(&session, DIRECTORY_LABEL),
    )
}

/// **状态行**的读数很小（有界），但它在"宿主输入顶到上限"那一轮会让位——让位**也要留名**。
///
/// 这一条是既有舞台 `a_host_input_near_the_batch_limit_is_still_delivered` 的另一半：那边钉
/// "别人的内容不许被吞"，这边钉"让掉的那一份不许无声消失"。
#[test]
fn the_status_bar_leaves_a_reading_when_the_host_input_fills_the_batch() {
    let measure_dir = TempDir::new("status-yield-measure");
    let (status_bytes, _) = measure_opening_blocks(&measure_dir);

    // 输入块离上限差 `gap`：下界是"补上状态行就顶出上限"（要 `gap < status_bytes`），
    // 上界是"还得放得下那句读数"（要 `gap >= 读数上限`）。两条都由量出来的数当场断言。
    let gap = (reading_upper_bound(STATUS_BAR_LABEL) + status_bytes) / 2;
    assert!(
        gap < status_bytes,
        "前提：宿主输入加一块状态行要能顶出上限（状态行 {status_bytes} 字节）"
    );
    assert!(
        gap >= reading_upper_bound(STATUS_BAR_LABEL),
        "留出的空当要放得下那句读数"
    );
    let payload = CAP - gap - HOST_INPUT_LABEL.len();
    assert!(payload < 65_526, "宿主输入必须在上限内: {payload}");

    let dir = TempDir::new("status-yield");
    let mut environment = dir.environment();
    let text = "x".repeat(payload);
    let mut session = session(&dir, &mut environment, RunOrigin::manual(), 2);
    session
        .provide(RunInput::text(text.clone()).expect("在输入上限内"))
        .expect("排入应当被接受");
    drive_waiting(&mut session);

    let injected = injected_blocks(&session);
    // 别人的内容只有这一份：它必须原样递进视野。
    assert!(
        injected.iter().any(|(_, body, _)| *body == text),
        "宿主显式交出的文本必须真的递进视野"
    );
    // 让位的那一份留名，而且报出它原本多大（它顶到了上限附近）。
    let notice = injected
        .iter()
        .find(|(label, body, _)| label == STATUS_BAR_LABEL && body.contains(NOT_DELIVERED))
        .unwrap_or_else(|| {
            panic!(
                "状态行让位时必须在原地留下读数: {:?}",
                injected
                    .iter()
                    .map(|(label, body, _)| (label.as_str(), body.len()))
                    .collect::<Vec<_>>()
            )
        });
    assert!(
        reading_bytes(&notice.1) == status_bytes,
        "读数要报出让位那一份原本多大（{status_bytes} 字节）: {}",
        notice.1
    );
}

/// **动作目录**只有首轮那一份，同样是"自己的读数"——超容时让位，但要在原地留名。
#[test]
fn the_action_directory_leaves_a_reading_when_it_cannot_fit() {
    let measure_dir = TempDir::new("directory-yield-measure");
    let (status_bytes, directory_bytes) = measure_opening_blocks(&measure_dir);

    // 让 `输入 + 状态行` 离上限只差这么多：小到两个人都进得来、大到目录挤不进来。
    // 下界要 `> reading_upper_bound(目录)`（读数放得下），上界要 `< directory_bytes`（目录挤不进来）。
    let gap = 100;
    assert!(
        directory_bytes > gap,
        "前提：目录本身要比留出的空当大（目录 {directory_bytes} 字节）"
    );
    assert!(
        gap > reading_upper_bound(DIRECTORY_LABEL),
        "留出的空当要放得下那句读数（标签 {} 字节）",
        DIRECTORY_LABEL.len()
    );
    let payload = CAP - gap - status_bytes - HOST_INPUT_LABEL.len();
    assert!(payload < 65_526, "宿主输入必须在上限内: {payload}");

    let dir = TempDir::new("directory-yield");
    let mut environment = dir.environment();
    let text = "x".repeat(payload);
    let mut session = session(&dir, &mut environment, RunOrigin::manual(), 2);
    session
        .provide(RunInput::text(text.clone()).expect("在输入上限内"))
        .expect("排入应当被接受");
    drive_waiting(&mut session);

    let injected = injected_blocks(&session);
    assert!(
        injected.iter().any(|(_, body, _)| *body == text),
        "宿主显式交出的文本必须真的递进视野"
    );
    // 状态行这一组还进得来（没到上限），因此它照旧在；挤不进来的是目录。
    assert!(
        injected
            .iter()
            .any(|(label, body, _)| label == STATUS_BAR_LABEL && !body.contains(NOT_DELIVERED)),
        "这一拍状态行不该让位"
    );
    let notice = injected
        .iter()
        .find(|(label, body, _)| label == DIRECTORY_LABEL && body.contains(NOT_DELIVERED))
        .unwrap_or_else(|| {
            panic!(
                "目录让位时必须在原地留下读数: {:?}",
                injected
                    .iter()
                    .map(|(label, body, _)| (label.as_str(), body.len()))
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(
        reading_bytes(&notice.1),
        directory_bytes,
        "读数要报出目录那一块的原始字节（含它自己的标签）"
    );
}

// ---------------------------------------------------------------------------
// 三、合并那一处：被让掉的是**另一半**，它也要留名
// ---------------------------------------------------------------------------

/// 两批各自都在上限内、合起来超限 ⇒ 留下的那一批里要有一句写清"另一半是什么、原本多大"。
#[test]
fn combining_past_the_limit_names_the_half_that_yielded() {
    let other = ContentBatch::of(vec![ContentBlock::new(
        HOST_INPUT_LABEL,
        "x".repeat(40_000),
    )])
    .expect("在上限内");
    let own = ContentBatch::of(vec![ContentBlock::new(
        STATUS_BAR_LABEL,
        "y".repeat(30_000),
    )])
    .expect("在上限内");

    let combined = combine(other, own);

    assert_eq!(combined.blocks().len(), 2, "别人的内容 + 一句读数");
    assert_eq!(
        combined.blocks()[0].text().len(),
        40_000,
        "别人的内容原样交回（它只有这一份）"
    );
    let notice = &combined.blocks()[1];
    assert_eq!(notice.label(), STATUS_BAR_LABEL, "读数带着让位者的标签");
    assert_eq!(
        notice.text(),
        format!(
            "{NOT_DELIVERED}（原始 {} B）",
            STATUS_BAR_LABEL.len() + 30_000
        ),
        "而且报出它原本多大"
    );
}

/// 多块的那一半让位时，标签要**都**写出来（不挑一块代表）。
#[test]
fn combining_names_every_label_of_the_half_that_yielded() {
    let other = ContentBatch::of(vec![ContentBlock::new(
        HOST_INPUT_LABEL,
        "x".repeat(40_000),
    )])
    .expect("在上限内");
    let own = ContentBatch::of(vec![
        ContentBlock::new("self_concept", "s".repeat(20_000)),
        ContentBlock::new("identity", "i".repeat(20_000)),
    ])
    .expect("在上限内");

    let combined = combine(other, own);
    let notice = combined.blocks().last().expect("有读数");
    assert_eq!(
        notice.label(),
        "self_concept+identity",
        "让位的两块都要留名"
    );
    assert_eq!(
        notice.text(),
        format!(
            "{NOT_DELIVERED}（原始 {} B）",
            "self_concept".len() + 20_000 + "identity".len() + 20_000
        )
    );
}

/// 退无可退的那个角：别人的内容**一个人就占满整批预算**，连一行读数都塞不进去。
///
/// 这时交回的仍是别人的那一批（它只有这一份）；这一条把它写成**看得见的**行为，
/// 免得"读数一定留得下"变成一句没人验过的想当然。
#[test]
fn combining_keeps_the_other_side_intact_when_it_fills_the_batch_itself() {
    let full = ContentBatch::of(
        (0..ContentBatch::MAX_BLOCKS)
            .map(|index| ContentBlock::new(format!("block-{index}"), "x"))
            .collect(),
    )
    .expect("正好在上限内");
    let own = ContentBatch::of(vec![ContentBlock::new(STATUS_BAR_LABEL, "读数")]).expect("合法");

    let combined = combine(full, own);
    assert_eq!(
        combined.blocks().len(),
        ContentBatch::MAX_BLOCKS,
        "连读数都塞不进去时，别人的内容一个块都不动"
    );
    assert_eq!(combined.blocks()[0].label(), "block-0");
}

// ---------------------------------------------------------------------------
// 四、读数自己的形状
// ---------------------------------------------------------------------------

/// 读数是一句常数 + 一个封顶的标签：**它自己一定装得下**（哪怕调用方给了个荒唐的标签）。
#[test]
fn the_reading_itself_always_fits() {
    let absurd = "标".repeat(100_000);
    let notice =
        ContentBatch::not_delivered(&absurd, ContentBatchError::TooLarge { bytes: 70_000 });
    assert_eq!(notice.blocks().len(), 1, "它是一块，而且只有一块");
    assert!(
        notice.blocks()[0].label().len() <= 64,
        "标签按字符边界截断到封顶: {}",
        notice.blocks()[0].label().len()
    );
    assert!(
        notice.blocks()[0].label().len() + notice.blocks()[0].text().len() < CAP,
        "读数自己必须在上限内"
    );
    assert!(notice.blocks()[0].text().contains(NOT_DELIVERED));
    assert!(
        !notice.is_source_failure(),
        "它不是来源失败：这次交付是成功的"
    );
}

/// 块数超限是另一句话（报块数，不报字节）：两种拒绝各自的读数都要如实。
#[test]
fn a_block_count_refusal_is_reported_as_a_block_count() {
    let notice = ContentBatch::not_delivered(
        "some-source",
        ContentBatchError::TooManyBlocks { blocks: 70 },
    );
    assert_eq!(
        notice.blocks()[0].text(),
        format!("{NOT_DELIVERED}（原始 70 块）")
    );
}

/// 没超限时它**一个字节都不改**（这一刀不许把正常路径也变成带读数）。
#[test]
fn a_batch_that_fits_is_delivered_untouched() {
    let blocks = vec![ContentBlock::new("a", "1"), ContentBlock::new("b", "2")];
    let delivered = ContentBatch::of_or_not_delivered(blocks.clone(), "a+b");
    assert_eq!(delivered.blocks(), blocks.as_slice());
    assert!(!delivered.is_empty());

    // 空批次照旧是空批次：没内容与装不下是两件事。
    assert!(ContentBatch::of_or_not_delivered(Vec::new(), "nothing").is_empty());
}

/// 那句读数的**措辞**只有一个装配点（`core.rs` 的 `NOT_DELIVERED`）：别的模块只带自己的标签
/// 来用它，不各写一遍——否则几处措辞迟早分叉，读主体的视野的人就得记住不止一种说法。
#[test]
fn the_reading_has_exactly_one_implementation() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut owners: Vec<String> = Vec::new();
    for path in rust_sources(&root) {
        let text = std::fs::read_to_string(&path).expect("读源码");
        if text.contains("NOT_DELIVERED") {
            owners.push(
                path.strip_prefix(&root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    assert_eq!(
        owners,
        vec!["core.rs".to_string()],
        "那句读数的措辞只该在 core.rs 里装配"
    );
}

/// 递归收集 `src/` 下的 `.rs`（与 `tests/minimal_agent.rs` 的枚举同一口径）。
fn rust_sources(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![directory.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = std::fs::read_dir(&current).expect("目录可读");
        for entry in entries {
            let path = entry.expect("目录项可读").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}
