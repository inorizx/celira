//! TASK_75 验收：**timer**——主体的第三件自持物（约时间）。
//!
//! 这一份管 App 自己那一层：表的形状与生命周期、标签的三种情形、跨进程持久、展开门、
//! 写效应归类。**宿主那一层**（到点响铃、离线补响、投影那一行、起源行）在 `host_doorbell.rs`
//! 的 TASK_75 一节里——那里有宿主夹具，而这里只需要一个环境根。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use celira_agent::clock::ClockHandle;
use celira_agent::core::{
    Action, ActionAdmission, ActionId, ActionParams, ActionPort, Capabilities,
};
use celira_agent::foundation::{AgentId, OperationId, RunId, Turn};
use celira_agent::os::{MinimalOs, RunExpansion};
use celira_agent::timer_app::{
    CAPABILITY_CANCEL, CAPABILITY_LIST, CAPABILITY_SET, CAPABILITY_VERIFY_CANCELLED,
    CAPABILITY_VERIFY_PENDING, MAX_LABEL_BYTES, PARAM_ID, PARAM_LABEL, PARAM_WHEN, TIMER_APP_ID,
    TIMER_FILE, TimerApp, TimerFailure, TimerState,
};

// ---------------------------------------------------------------------------
// 装置（自建目录；不指桌面、不指家、不指仓库根）
// ---------------------------------------------------------------------------

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("celira-timer-{}-{label}-{n}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).expect("清掉同名残留");
        }
        std::fs::create_dir_all(&root).expect("建临时根");
        Self { root }
    }

    fn app(&self) -> TimerApp {
        TimerApp::new(&self.root)
    }

    /// 表文件的原文（判磁盘契约的那四个键）。
    fn raw(&self) -> String {
        std::fs::read_to_string(self.root.join(TIMER_FILE)).expect("表应当已经落盘")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// 一次动词调用（App 自己那一层：不走 OS、不走准入）。
fn call(app: &mut TimerApp, id: &str, params: &[(&str, &str)], index: u16) -> String {
    let mut built = ActionParams::none();
    for (name, value) in params {
        built = built.with(*name, *value);
    }
    let action = Action::with_params(ActionId::new(id), built);
    let operation = OperationId::at(RunId::new("run-timer"), Turn::FIRST, index);
    let agent = AgentId::new("agent-timer");
    let outcome = app.execute(&agent, &operation, &action);
    assert_eq!(
        outcome.state(),
        celira_agent::foundation::ResultState::Success,
        "这一步应当成功（{id}）"
    );
    outcome
        .content()
        .iter()
        .map(|block| block.text().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// **验收 1（生命周期）**：约一条 → 表里看得见 → 除名 → 表里没有了。
#[test]
fn a_timer_is_set_listed_and_cancelled_away() {
    let sandbox = Sandbox::new("lifecycle");
    let mut app = sandbox.app();

    let set = call(
        &mut app,
        CAPABILITY_SET,
        &[(PARAM_WHEN, "1758000000000"), (PARAM_LABEL, "看便签")],
        0,
    );
    assert!(set.contains("id = t-1"), "回执要给出机械身份: {set}");
    assert!(set.contains("state = pending"), "{set}");
    assert!(set.contains("when = 1758000000000"), "{set}");
    assert!(set.contains("label = 看便签"), "{set}");

    let list = call(&mut app, CAPABILITY_LIST, &[], 1);
    assert!(list.contains("count = 1, pending = 1, fired = 0"), "{list}");
    assert!(list.contains("t-1"), "{list}");

    let cancelled = call(&mut app, CAPABILITY_CANCEL, &[(PARAM_ID, "t-1")], 2);
    assert!(cancelled.contains("cancelled = true"), "{cancelled}");

    let after = call(&mut app, CAPABILITY_LIST, &[], 3);
    assert!(after.contains("count = 0"), "除名了就不再是那条: {after}");
    assert!(!after.contains("t-1"), "{after}");
    assert!(
        after.contains("（还没有约过时间）"),
        "空也要说出口: {after}"
    );

    // 取消一个**表里没有**的 id：明确的成功结果，不报错（与摘一个没钉过的动作同款）。
    let missing = call(&mut app, CAPABILITY_CANCEL, &[(PARAM_ID, "t-99")], 4);
    assert!(missing.contains("cancelled = false"), "{missing}");
}

/// **验收 5（标签）**：可选（不填就是空）、空如实空、超限**如实截断并留读数**。
#[test]
fn a_label_is_optional_and_an_overlong_one_is_clipped_with_a_reading() {
    let sandbox = Sandbox::new("label");
    let mut app = sandbox.app();

    // ① 不填：空标签照收（不设必填——强制必填只收获垃圾标签）。
    let bare = call(&mut app, CAPABILITY_SET, &[(PARAM_WHEN, "1")], 0);
    assert!(
        bare.ends_with("label = "),
        "空就是空（那一行就停在等号后）: {bare:?}"
    );
    assert!(!bare.contains("label_truncated"), "没剪过就不报: {bare}");

    // ② 填空串：一样是空（不做"多半是忘了填"的猜测）。
    let empty = call(
        &mut app,
        CAPABILITY_SET,
        &[(PARAM_WHEN, "2"), (PARAM_LABEL, "")],
        1,
    );
    assert!(empty.ends_with("label = "), "{empty:?}");

    // ③ 超限：截断**在字符边界上**，并留两条读数（剪过没有、原来多少字节）。
    let long = "看".repeat(MAX_LABEL_BYTES); // 每个汉字 3 字节 ⇒ 远超上限
    let clipped = call(
        &mut app,
        CAPABILITY_SET,
        &[(PARAM_WHEN, "3"), (PARAM_LABEL, &long)],
        2,
    );
    assert!(clipped.contains("label_truncated = true"), "{clipped}");
    assert!(
        clipped.contains(&format!("label_bytes = {}", long.len())),
        "原来多少字节要说出来: {clipped}"
    );
    assert!(
        clipped.contains(&format!("label_bytes_limit = {MAX_LABEL_BYTES}")),
        "{clipped}"
    );
    let stored = app.entries().expect("表读得出来");
    let entry = stored
        .iter()
        .find(|entry| entry.id == "t-3")
        .expect("那条在");
    assert!(
        entry.label.len() <= MAX_LABEL_BYTES,
        "{}",
        entry.label.len()
    );
    assert!(!entry.label.is_empty(), "截断不是清空");
}

/// **验收 6（持久）**：跨"进程重启"表还在；磁盘上就是那四个键；后续加字段带默认值。
#[test]
fn the_table_survives_a_restart_and_its_disk_keys_are_the_contract() {
    let sandbox = Sandbox::new("persist");
    let mut first = sandbox.app();
    call(
        &mut first,
        CAPABILITY_SET,
        &[(PARAM_WHEN, "1758000000000"), (PARAM_LABEL, "明早八点检查")],
        0,
    );

    // 磁盘上：一行一个 JSON 对象，键就是那四个（顺序由 `json!` 的插入序给，内容才是契约）。
    let raw = sandbox.raw();
    assert_eq!(raw.lines().count(), 1, "一行一条: {raw}");
    for key in ["\"id\"", "\"when\"", "\"label\"", "\"state\""] {
        assert!(raw.contains(key), "磁盘契约里要有 {key}: {raw}");
    }

    // "重启"：换一个实例读同一个根 ⇒ 表照旧。
    let second = sandbox.app();
    let entries = second.entries().expect("重启后读得出来");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].when_millis, 1_758_000_000_000);
    assert_eq!(entries[0].label, "明早八点检查");
    assert_eq!(entries[0].state, TimerState::Pending);

    // **后续加字段带默认值**：`label` 缺席 ⇒ 空标签（唯一带默认值的字段）；多出来的键不看。
    std::fs::write(
        sandbox.root.join(TIMER_FILE),
        "{\"id\":\"t-9\",\"when\":1758000000001,\"state\":\"pending\",\"later\":\"将来的字段\"}\n",
    )
    .expect("写一条老形状的行");
    let forward = sandbox.app().entries().expect("老形状照旧读得动");
    assert_eq!(forward.len(), 1);
    assert_eq!(forward[0].label, "", "缺席的 label 是空标签");
    assert_eq!(forward[0].id, "t-9");

    // 反过来：**缺了必须有的键 ⇒ 读不懂**（不猜、不当默认值）。
    std::fs::write(sandbox.root.join(TIMER_FILE), "{\"id\":\"t-9\"}\n").expect("写一条坏行");
    assert!(
        sandbox.app().entries().is_err(),
        "缺 `when` / `state` 的行读不懂，而不是当成零"
    );
}

/// **验收 7（展开门）**：没展开之前三条动词根本不在场（被**既有**那道门拒），展开之后才进场。
///
/// 这一条判的是**门没有被放松**：timer 与 files 同款——手动展开，不是预装常开。
#[test]
fn the_verbs_are_out_of_play_until_the_window_is_opened() {
    let sandbox = Sandbox::new("expand");
    let agent = AgentId::new("agent-expand");
    let run = RunId::new("run-expand");
    let mut os =
        MinimalOs::assemble(vec![Box::new(TimerApp::new(&sandbox.root))]).expect("可以装配");
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = os.scope(&mut expansion);
    let operation = OperationId::new(run.clone(), Turn::FIRST);

    let set = Action::with_params(
        ActionId::new(CAPABILITY_SET),
        ActionParams::none().with(PARAM_WHEN, "1758000000000"),
    );

    // 没展开：动词不在场 ⇒ **既有那道门**拒（分类词仍归 OS，本件不认识它）。
    assert_eq!(
        scope.admit(&agent, &run, &set),
        ActionAdmission::refused("not-expanded"),
        "没打开这一格之前，约时间那三条不在场"
    );
    assert!(
        !scope
            .capabilities(&agent, &run)
            .items()
            .iter()
            .any(|capability| capability.id().as_str() == CAPABILITY_SET),
        "不在场就不该出现在能力表里"
    );

    // 展开那一下（OS 自有动作）：三条动词进场。
    let opened = scope.execute(
        &agent,
        &operation,
        &Action::with_params(
            ActionId::new("open_app"),
            ActionParams::none().with("target", TIMER_APP_ID),
        ),
    );
    assert_eq!(
        opened.state(),
        celira_agent::foundation::ResultState::Success,
        "展开要成功"
    );
    let ids: Vec<String> = scope
        .capabilities(&agent, &run)
        .items()
        .iter()
        .map(|capability| capability.id().as_str().to_string())
        .collect();
    for verb in [
        CAPABILITY_SET,
        CAPABILITY_LIST,
        CAPABILITY_CANCEL,
        CAPABILITY_VERIFY_PENDING,
        CAPABILITY_VERIFY_CANCELLED,
    ] {
        assert!(
            ids.contains(&verb.to_string()),
            "展开后 {verb} 要在场: {ids:?}"
        );
    }
    // 展开之后**不再**是 `not-expanded`；它今天**照旧过"先看形状"那一关**——因为那一关问的是
    // **这个动作会不会改外面的世界**（`EffectClass`），而 timer 的写**现在声明了**写效应
    // （2026-10-13：`timer_set` / `timer_cancel` 各配一个同参复核，见下一条用例）。
    //
    // ⚠ **这一格 2026-10-13 改过一次，来回都要说清**：§3.73 收窄那一关之后，timer **因为没声明
    // 而悄悄免了那一关**——同一个动作类（`timer_set` 与 `notes_create`）行为不同，而那个不同
    // **没有人打算过**。本刀把声明补齐 ⇒ 两者同待遇。
    let refused = scope.admit(&agent, &run, &set);
    match &refused {
        ActionAdmission::Refused { class, content } => {
            assert_eq!(class, "needs-inspection", "展开门放行之后轮到既有的那一关");
            assert_eq!(content.len(), 1, "随单恰好一块：那张卡");
            assert_eq!(content[0].label(), CAPABILITY_SET, "卡带着它自己的标签");
        }
        other => panic!("这一步应当是「先看格式」那一关: {other:?}"),
    }
    // 卡随拒绝给过了 ⇒ **同一份载荷直接可调**（既有语义，一个字没动）。
    assert_eq!(
        scope.admit(&agent, &run, &set),
        ActionAdmission::Admitted,
        "卡进过视野之后照旧放行"
    );
    // **形状也照旧拿得到**（按需那条路一个字没动）：主体想知道它要什么，`inspect_action` 随时给。
    let card = scope.execute(
        &agent,
        &operation,
        &Action::with_params(
            ActionId::new("inspect_action"),
            ActionParams::none().with("action", CAPABILITY_SET),
        ),
    );
    assert_eq!(
        card.state(),
        celira_agent::foundation::ResultState::Success,
        "形状按需仍然拿得到"
    );
}

/// **验收 8（写效应归类，2026-10-13 改口）**：自持物写**声明**它的写效应与复核——
/// 而"声明"**不是权限**：它不需要 Grant，也不需要"先复核上一笔"。
///
/// ⚠ 本用例原名 `..._needs_no_grant_and_no_verification`，判据是"`declared_effects()` 是空的"。
/// 那一条**把两件事混成了一件**：`app_contract` 写着「**声明不是权限**：它不授予、不撤销、也不替代
/// 准入；它只是'系统这一格能不能机械判断'的依据」。⇒ 自持物写**照样是写**（它改的是进程之外的
/// 东西），因此照样该说清**类别**与**复核**；"不需要授权"与"不需要说清"是两件事。
///
/// 本用例因此改成钉三件：①两个写各声明一次、各配一个复核；②**复核自己不被声明为写**
/// （否则那一格就成了递归）；③连着写两笔照旧都成功（**不需要先复核上一笔**——声明不产生门）。
#[test]
fn a_self_held_write_declares_its_effect_but_needs_no_grant() {
    use celira_agent::app_contract::{AppPort, EffectClass};

    let sandbox = Sandbox::new("effects");
    let app = TimerApp::new(&sandbox.root);

    let declared = app.declared_effects();
    assert_eq!(declared.len(), 2, "两个写各声明一次: {declared:?}");
    for write in [CAPABILITY_SET, CAPABILITY_CANCEL] {
        let one = declared
            .iter()
            .find(|each| each.action().as_str() == write)
            .unwrap_or_else(|| panic!("{write} 要声明写效应: {declared:?}"));
        assert_eq!(one.class(), EffectClass::RepeatableWrite, "{write} 的类别");
        let verification = one
            .verification()
            .expect("写类别必须配一个复核（装配期会拒没配的）");
        assert!(
            !declared.iter().any(|each| each.action() == verification),
            "复核自己**不许**被声明成写（{} 只读）——否则那一格就成了递归",
            verification.as_str()
        );
    }
    // 两条复核**在场**（可调）：五条动词都在声明面上。
    let ids: Vec<String> = app
        .declared_capabilities()
        .items()
        .iter()
        .map(|capability| capability.id().as_str().to_string())
        .collect();
    for verb in [CAPABILITY_VERIFY_PENDING, CAPABILITY_VERIFY_CANCELLED] {
        assert!(
            ids.contains(&verb.to_string()),
            "复核动词要在场: {verb} in {ids:?}"
        );
    }

    let agent = AgentId::new("agent-effects");
    let run = RunId::new("run-effects");
    let mut os = MinimalOs::assemble_preinstalled(
        vec![Box::new(TimerApp::new(&sandbox.root))],
        &[TIMER_APP_ID],
    )
    .expect("装配成**预装常开**，只为让动词直接在场（本用例不看展开门）");
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = os.scope(&mut expansion);

    let write = |params: ActionParams| Action::with_params(ActionId::new(CAPABILITY_SET), params);
    // **声明带来的是"先看形状"那一关**（§3.73 的判据：会不会改外面的世界）——不是一道授权门。
    let first = write(ActionParams::none().with(PARAM_WHEN, "1758000000000"));
    assert!(
        matches!(
            scope.admit(&agent, &run, &first),
            ActionAdmission::Refused { .. }
        ),
        "声明了写效应 ⇒ 这一程第一次调用先看形状（与 notes 同待遇）"
    );
    // 卡进过视野 ⇒ 此后这一程直接可调；**连着写两笔照旧都成功**（不需要先复核上一笔）。
    for (index, when) in ["1758000000000", "1758000000001"].into_iter().enumerate() {
        let action = write(ActionParams::none().with(PARAM_WHEN, when));
        assert_eq!(
            scope.admit(&agent, &run, &action),
            ActionAdmission::Admitted,
            "自持物写不需要授权"
        );
        let outcome = scope.execute(
            &agent,
            &OperationId::at(run.clone(), Turn::FIRST, index as u16),
            &action,
        );
        assert_eq!(
            outcome.state(),
            celira_agent::foundation::ResultState::Success,
            "第 {index} 笔不需要先复核上一笔（声明不产生门；复核是给系统机械判断用的）"
        );
    }
    assert_eq!(sandbox.app().entries().expect("表读得出来").len(), 2);

    // 参数面是**封闭**的：缺必填、多了不认识的名字，各给稳定的分类与差异。
    let missing = scope.execute(
        &agent,
        &operation_of(&run, 20),
        &Action::new(ActionId::new(CAPABILITY_SET)),
    );
    assert_eq!(
        missing.failure_class(),
        Some(TimerFailure::MissingParameter.as_str())
    );
    let unknown = scope.execute(
        &agent,
        &operation_of(&run, 21),
        &write(ActionParams::none().with(PARAM_WHEN, "1").with("oops", "x")),
    );
    assert_eq!(
        unknown.failure_class(),
        Some(TimerFailure::UnknownParameter.as_str())
    );
    // 值不是非负整数毫秒 ⇒ 明确拒绝（不退回"现在"）。
    let bad = scope.execute(
        &agent,
        &operation_of(&run, 22),
        &write(ActionParams::none().with(PARAM_WHEN, "马上")),
    );
    assert_eq!(
        bad.failure_class(),
        Some(TimerFailure::InvalidWhen.as_str())
    );

    // 声明面与执行面同源：三条动词都在声明的能力表里。
    let Capabilities { .. } = app.declared_capabilities();
    assert!(
        app.description().contains("约时间"),
        "{}",
        app.description()
    );
}

fn operation_of(run: &RunId, index: u16) -> OperationId {
    OperationId::at(run.clone(), Turn::FIRST, index)
}

/// **两个写动词各自的同参复核**（2026-10-13）：三分（符合 / 明确不符合 / 读不出来），
/// 而且复核与写**用同一把截断尺**（约的时候标签被剪过，复核也要剪过之后再比）。
///
/// 这是 `declared_effects()` 那两个 `verification` 的行为证明：声明说"这一格系统能机械判断"，
/// 这条用例就是那句话的读数。
#[test]
fn the_two_writes_have_a_same_parameter_verification() {
    let sandbox = Sandbox::new("verify");
    let agent = AgentId::new("agent-verify");
    let run = RunId::new("run-verify");
    let mut os = MinimalOs::assemble_preinstalled(
        vec![Box::new(TimerApp::new(&sandbox.root))],
        &[TIMER_APP_ID],
    )
    .expect("装配成预装常开");
    let mut expansion = RunExpansion::new(agent.clone(), run.clone());
    let mut scope = os.scope(&mut expansion);

    let mut call = |index: u16, verb: &str, params: ActionParams| {
        scope.execute(
            &agent,
            &operation_of(&run, index),
            &Action::with_params(ActionId::new(verb), params),
        )
    };
    let when = "1758000000000";
    // 一条**超过上限**的标签：写的时候会被如实截断（约 60 字节）。
    let long = "看".repeat(40);

    // ① 约上之前：复核说"没有" ⇒ **明确失败**（不是成功、也不是读不出来）。
    let before = call(
        0,
        CAPABILITY_VERIFY_PENDING,
        ActionParams::none().with(PARAM_WHEN, when),
    );
    assert_eq!(
        before.failure_class(),
        Some(TimerFailure::NotPending.as_str()),
        "还没约过就复核 ⇒ 明确说没有"
    );

    // ② 约上（标签被截断）⇒ 复核**同一份参数照旧成功**：两边用同一把尺。
    let set = call(
        1,
        CAPABILITY_SET,
        ActionParams::none()
            .with(PARAM_WHEN, when)
            .with(PARAM_LABEL, &long),
    );
    assert_eq!(set.state(), celira_agent::foundation::ResultState::Success);
    let after = call(
        2,
        CAPABILITY_VERIFY_PENDING,
        ActionParams::none()
            .with(PARAM_WHEN, when)
            .with(PARAM_LABEL, &long),
    );
    assert_eq!(
        after.state(),
        celira_agent::foundation::ResultState::Success,
        "复核必须把**被剪过**的那份参数也认出来: {after:?}"
    );
    assert!(
        after.content()[0].text().contains("pending = true"),
        "回执说的是**现在**的状态: {:?}",
        after.content()[0].text()
    );

    // ③ 别的时刻 ⇒ 明确失败（同参复核只认同一份参数）。
    let other = call(
        3,
        CAPABILITY_VERIFY_PENDING,
        ActionParams::none().with(PARAM_WHEN, "1758000000001"),
    );
    assert_eq!(
        other.failure_class(),
        Some(TimerFailure::NotPending.as_str())
    );

    // ④ 除名的复核：**先**说"还在"（明确失败，回执带上它现在那一行），**后**说"不在了"。
    let id = sandbox.app().entries().expect("表读得出来")[0].id.clone();
    let still = call(
        4,
        CAPABILITY_VERIFY_CANCELLED,
        ActionParams::none().with(PARAM_ID, &id),
    );
    assert_eq!(
        still.failure_class(),
        Some(TimerFailure::StillPending.as_str()),
        "还没除名 ⇒ 明确说还在"
    );
    let cancel = call(
        5,
        CAPABILITY_CANCEL,
        ActionParams::none().with(PARAM_ID, &id),
    );
    assert_eq!(
        cancel.state(),
        celira_agent::foundation::ResultState::Success
    );
    let gone = call(
        6,
        CAPABILITY_VERIFY_CANCELLED,
        ActionParams::none().with(PARAM_ID, &id),
    );
    assert_eq!(gone.state(), celira_agent::foundation::ResultState::Success);

    // ⑤ **读不出来 ⇒ `unknown`**（既不说"没有"，也不说"还在"）：把表那个名字用目录占住。
    std::fs::remove_file(sandbox.app().path()).expect("先摘掉那份表");
    std::fs::create_dir(sandbox.app().path()).expect("用目录占住那个名字");
    let unreadable = call(
        7,
        CAPABILITY_VERIFY_PENDING,
        ActionParams::none().with(PARAM_WHEN, when),
    );
    assert_eq!(
        unreadable.state(),
        celira_agent::foundation::ResultState::Unknown,
        "读不出来就是读不出来（三分里的第三档）: {unreadable:?}"
    );
}

// ---------------------------------------------------------------------------
// 本地钟源那一层（`wake.rs` 的 TimerSource）：到点才投一次，收掉之后重新武装
// ---------------------------------------------------------------------------

/// 一个**可以拨动**的钟（用例当时间的主人）：固定钟看不出"到点前后"，递增钟又对不上表里的
/// `when`——只有能拨的钟才能把"到点"这件事判成确定的一步。
fn adjustable_clock(at: SystemTime) -> (ClockHandle, Arc<Mutex<SystemTime>>) {
    let cell = Arc::new(Mutex::new(at));
    let handle = ClockHandle::of({
        let cell = cell.clone();
        move || *cell.lock().expect("短临界区")
    });
    (handle, cell)
}

fn at_unix_millis(millis: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(millis)
}

/// **验收 2／3 的源头那一层**：钟源**到点才投**、**同一条不重复投**、收掉之后**重新武装**。
///
/// 判错即红的三处：没到点就投（骚扰）、到点了重复投（一场变几场）、收掉了还投（对着空表响）。
#[test]
fn the_local_source_signals_once_per_due_timer_and_rearms_after_consumption() {
    use celira_agent::wake::{TimerSource, Woken};
    use std::sync::mpsc;
    use std::time::Duration;

    let sandbox = Sandbox::new("source");
    let start = 1_800_000_000_000u64;
    let (clock, cell) = adjustable_clock(at_unix_millis(start));
    let mut app = sandbox.app();
    call(
        &mut app,
        CAPABILITY_SET,
        &[
            (PARAM_WHEN, &(start + 10_000).to_string()),
            (PARAM_LABEL, "十秒后"),
        ],
        0,
    );

    let (sender, signals) = mpsc::sync_channel::<Woken>(1);
    let _running = TimerSource::new(&sandbox.root, clock, Duration::from_millis(10)).run(sender);

    // ① 还没到点：**一个信号都不投**。
    assert!(
        signals.recv_timeout(Duration::from_millis(80)).is_err(),
        "没到点就不该叫醒任何人"
    );

    // ② 把钟拨到那一刻：投**一次**。
    *cell.lock().expect("短临界区") = at_unix_millis(start + 10_000);
    assert_eq!(
        signals
            .recv_timeout(Duration::from_secs(2))
            .expect("到点要投一次"),
        Woken::Timer
    );

    // ③ 宿主还没把它收掉 ⇒ **同一条不重复投**（铃合并、排队都由宿主那条通道管）。
    assert!(
        signals.recv_timeout(Duration::from_millis(60)).is_err(),
        "同一条到点只投一次，不重复叫"
    );

    // ④ 收掉它 ⇒ 表里没有 pending 了 ⇒ 不再投（重新武装成"等主体下次约"）。
    let consumed = sandbox.app().consume_due(start + 10_000).expect("收得掉");
    assert_eq!(consumed.len(), 1, "到点的那条被收掉");
    assert!(
        signals.recv_timeout(Duration::from_millis(60)).is_err(),
        "收掉之后不该再响"
    );
}
