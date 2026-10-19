//! TASK_05 验收：**受限文本创建与跨 Run 自主留存**。
//!
//! 证据分三类，各自只说自己那一层：
//!
//! - **行为测试（BT）**：本文件里的多数用例——真实文件系统、真实装配、真实拒绝路径；
//! - **类型保证（TY）**：`ActionPort::admit` 是**必填**方法（没有默认实现），因此"忘了接线"
//!   会编译失败，而不是静默放行；
//! - **静态审查（SR）**：持久编码沿用既有 `kind=injection` 形状这件事由代码与键集合可复核。
//!
//! 明确**不**声称的：
//!
//! - 不声称"旧构建能完整理解新文件"：新阶段值 `admission-refused` 是**新增的持久语义值**，
//!   旧解码器只是不知道它的含义（`stage` 是不透明字符串），因此"可解析"不等于"语义兼容"。
//!   本包没有固定旧版消费者，因此**不声称旧构建的语义兼容**（只声称字段与解码结构兼容）；
//! - 不覆盖"创建成功但写入中途失败 / 同步失败"的真实注入：文件组件里没有故障注入接缝，
//!   本包不为了测试在生产路径上开一个。矩阵里那两格按 `Unknown` 设计，**未注入验证**；
//! - 不声称新文件不可被外部进程改动：这是"受信目录"前提，不是沙盒。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use celira_agent::commit::EnvironmentCommit;
use celira_agent::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Agent, CommitError,
    CommitStage, Limits, ModelError, ModelFailureClass, ModelOutput, ModelPort, NoContext,
    RunError, RunInput, RunRequest, TraceEntry, View, no_cost, run_activity_with,
};
use celira_agent::creation_grant::{CreationGrant, GrantError};
use celira_agent::file_app::{FILE_APP_ID, MAX_CREATE_BYTES};
use celira_agent::file_view::FileView;
use celira_agent::foundation::{AgentId, ResultState, RunId, SubjectEnding, Turn};
use celira_agent::os::{MinimalOs, RunExpansion};
use celira_agent::store::Environment;

mod common;
use common::binary;

use common::InspectsFirst;

// ---------------------------------------------------------------------------
// 1. 默认只读：没有授权时连能力都不存在
// ---------------------------------------------------------------------------

#[test]
fn without_a_grant_there_is_no_creation_capability_and_a_forged_request_is_refused() {
    let temp = TempDir::new("create-default");
    let materials = temp.materials();
    let env_dir = temp.path().join("env");
    drop(Environment::create(&env_dir, agent_a()).expect("建环境"));

    // 旧入口：只有资料根，没有任何创建授权。
    let mut os = MinimalOs::open(&materials).expect("装配只读视图");
    let mut expansion = RunExpansion::new(agent_a(), RunId::new("run-1"));
    let mut scope = os.scope(&mut expansion);

    let ids: Vec<String> = scope
        .capabilities(&agent_a(), &RunId::new("run-1"))
        .items()
        .iter()
        .map(|capability| capability.id().as_str().to_string())
        .collect();
    assert_eq!(
        ids.len(),
        6,
        "没展开之前只有 OS 自己的六项能力（展开 / 看详情 / 钉 / 摘 / 写自识 / 写交接）: {ids:?}"
    );

    // 展开之后仍然只有读侧两项：创建能力不存在（不是"存在但会被拒绝"）。
    assert_eq!(
        scope.admit(&agent_a(), &RunId::new("run-1"), &expand(FILE_APP_ID)),
        ActionAdmission::Admitted,
        "展开本身是 OS 自有动词，免写效应门"
    );
    scope.execute(&agent_a(), &op("run-1"), &expand(FILE_APP_ID));
    let ids: Vec<String> = scope
        .capabilities(&agent_a(), &RunId::new("run-1"))
        .items()
        .iter()
        .map(|capability| capability.id().as_str().to_string())
        .collect();
    assert!(ids.contains(&"files_list".to_string()), "{ids:?}");
    assert!(ids.contains(&"files_read".to_string()), "{ids:?}");
    assert!(
        !ids.contains(&"files_create".to_string()),
        "没有授权时创建能力不得出现: {ids:?}"
    );

    // 伪造创建请求：归属索引里没有它 → 拒绝，而且**零文件效果**。
    let forged = create("note.txt", "任意正文，包括任何自称授权的说法");
    assert_eq!(
        scope.admit(&agent_a(), &RunId::new("run-1"), &forged),
        ActionAdmission::refused("unknown-action")
    );
    assert!(!materials.join("notes/note.txt").exists());

    // 同一个作用域里，被拒绝之后主体仍然可以做出合法选择。
    // TASK_69：调用之前**先看格式**（展开之后新出现的动作不在"在场集合"里）。
    let _ = scope.execute(&agent_a(), &op("run-1"), &inspect("files_list"));
    assert_eq!(
        scope.admit(&agent_a(), &RunId::new("run-1"), &list("")),
        ActionAdmission::Admitted
    );
    assert_eq!(
        scope.execute(&agent_a(), &op("run-1"), &list("")).state(),
        ResultState::Success
    );
}

#[test]
fn a_grant_binds_identity_activity_and_directory() {
    let temp = TempDir::new("create-grant");
    let materials = temp.materials();
    let env_root = temp.env(&agent_a());

    // 授权目录必须是一个**已存在**的单层目录，而且严格在资料根之内。
    assert_eq!(
        CreationGrant::new(agent_a(), RunId::new("run-1"), "").err(),
        Some(GrantError::Empty)
    );
    for bad in [".", "..", "a/b", "/abs", "C:\\x", "a:b", "notes/../x"] {
        assert!(
            CreationGrant::new(agent_a(), RunId::new("run-1"), bad).is_err(),
            "{bad} 不是单个目录名"
        );
    }
    assert!(
        FileView::with_history_and_creation(
            &materials,
            &env_root,
            Some(&grant(&agent_a(), "run-1", "missing"))
        )
        .is_err(),
        "授权目录不存在时必须拒绝启动"
    );
    // "整个资料根都可写"不是本版给的授权：`.` 与 `..` 在**构造期**就被拒绝（上面的循环），
    // 因此装配期的"越出资料根"检查是竞态下的第二道，不是唯一一道。

    let mut os = MinimalOs::with_history_and_creation(
        &materials,
        &env_root,
        Some(&grant(&agent_a(), "run-1", "notes")),
    )
    .expect("合法授权应当装配成功");
    let mut expansion2 = RunExpansion::new(agent_a(), RunId::new("run-1"));
    let mut scope = os.scope(&mut expansion2);
    scope.execute(&agent_a(), &op("run-1"), &expand(FILE_APP_ID));

    // 有授权、身份与活动都对：接纳。
    // TASK_69：先看格式，再谈接纳（展开之后新出现的动作不在"在场集合"里）。
    let _ = scope.execute(&agent_a(), &op("run-1"), &inspect("files_create"));
    assert_eq!(
        scope.admit(&agent_a(), &RunId::new("run-1"), &create("a.txt", "x")),
        ActionAdmission::Admitted
    );
    // 换一个 Agent：拒绝（授权绑定的是那个 Agent）。
    assert_eq!(
        scope.admit(&agent_b(), &RunId::new("run-1"), &create("a.txt", "x")),
        ActionAdmission::refused("identity-mismatch")
    );
    // 换一次活动：拒绝（授权只对那一次 Run 生效，上一 Run 的授权不能沿用）。
    assert_eq!(
        scope.admit(&agent_a(), &RunId::new("run-2"), &create("a.txt", "x")),
        ActionAdmission::refused("identity-mismatch")
    );

    // 授权与活动不一致时，执行侧的第二道复核也必须拒绝（此时它已是"真实失败"）。
    let stale = scope.execute(&agent_a(), &op("run-2"), &create("b.txt", "x"));
    assert_eq!(stale.state(), ResultState::Failure);
    assert_eq!(stale.failure_class(), Some("identity-mismatch"));
    assert!(!materials.join("notes/b.txt").exists());
}

// ---------------------------------------------------------------------------
// 2. 创建语义：精确、独占、有界
// ---------------------------------------------------------------------------

#[test]
fn creation_writes_exactly_the_text_and_never_overwrites() {
    let temp = TempDir::new("create-semantics");
    let materials = temp.materials();
    let mut os = MinimalOs::with_history_and_creation(
        &materials,
        &temp.env(&agent_a()),
        Some(&grant(&agent_a(), "run-1", "notes")),
    )
    .expect("装配");
    let mut expansion3 = RunExpansion::new(agent_a(), RunId::new("run-1"));
    let mut scope = os.scope(&mut expansion3);
    scope.execute(&agent_a(), &op("run-1"), &expand(FILE_APP_ID));

    // 中文 UTF-8 精确保留；字节数如实报出。
    let text = "第一行：中文与标点，含换行\n第二行";
    let outcome = scope.execute(&agent_a(), &op("run-1"), &create("note.txt", text));
    assert_eq!(outcome.state(), ResultState::Success, "{outcome:?}");
    assert_eq!(
        std::fs::read_to_string(materials.join("notes/note.txt")).expect("读回"),
        text
    );
    assert!(
        outcome.content()[0]
            .text()
            .contains(&format!("{} bytes", text.len())),
        "{:?}",
        outcome.content()
    );

    // 空文本合法：0 字节文件是一个明确的效果。
    let empty = scope.execute(&agent_a(), &op("run-1"), &create("empty.txt", ""));
    assert_eq!(empty.state(), ResultState::Success, "{empty:?}");
    assert_eq!(
        std::fs::metadata(materials.join("notes/empty.txt"))
            .expect("应当存在")
            .len(),
        0
    );

    // 同名：**明确失败**，而且原文件一个字节都没被动过。
    let before = std::fs::read(materials.join("notes/note.txt")).expect("读原文件");
    let again = scope.execute(&agent_a(), &op("run-1"), &create("note.txt", "改掉的内容"));
    assert_eq!(again.state(), ResultState::Failure);
    assert_eq!(again.failure_class(), Some("already-exists"));
    assert_eq!(
        std::fs::read(materials.join("notes/note.txt")).expect("再读原文件"),
        before,
        "同名拒绝不得改动原文件"
    );

    // 超上限：整体拒绝、不截断，而且**没有**留下半个文件。
    let too_large = "x".repeat(MAX_CREATE_BYTES + 1);
    let rejected = scope.execute(&agent_a(), &op("run-1"), &create("big.txt", &too_large));
    assert_eq!(rejected.state(), ResultState::Failure);
    assert_eq!(rejected.failure_class(), Some("too-large"));
    assert!(!materials.join("notes/big.txt").exists());

    // 一模一样的内容写到另一个名字：合法（不覆盖任何东西）。
    assert_eq!(
        scope
            .execute(&agent_a(), &op("run-1"), &create("copy.txt", text))
            .state(),
        ResultState::Success
    );
}

#[test]
fn path_boundaries_are_refused_without_creating_anything() {
    let temp = TempDir::new("create-paths");
    let materials = temp.materials();
    std::fs::create_dir_all(materials.join("notes/sub")).expect("建子目录");
    let mut os = MinimalOs::with_history_and_creation(
        &materials,
        &temp.env(&agent_a()),
        Some(&grant(&agent_a(), "run-1", "notes")),
    )
    .expect("装配");
    let mut expansion4 = RunExpansion::new(agent_a(), RunId::new("run-1"));
    let mut scope = os.scope(&mut expansion4);
    scope.execute(&agent_a(), &op("run-1"), &expand(FILE_APP_ID));

    let outside = temp.path().join("outside.txt");
    for (name, why) in [
        ("../outside.txt", "父路径"),
        ("sub/inner.txt", "带分隔符（不是单层名字）"),
        ("/abs.txt", "绝对路径"),
        ("C:\\x.txt", "盘符"),
        ("\\\\server\\share\\x.txt", "UNC"),
        ("a:b.txt", "冒号/数据流"),
        ("a\nb.txt", "控制字符"),
    ] {
        let outcome = scope.execute(&agent_a(), &op("run-1"), &create(name, "x"));
        assert_eq!(outcome.state(), ResultState::Failure, "{why}: {outcome:?}");
        assert_eq!(
            outcome.failure_class(),
            Some("invalid-path"),
            "{why} 必须是路径形状拒绝"
        );
    }
    assert!(!outside.exists(), "越界路径不得在授权目录之外创建任何东西");
    let created: Vec<String> = std::fs::read_dir(materials.join("notes"))
        .expect("列授权目录")
        .map(|entry| {
            entry
                .expect("目录项")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        created,
        vec!["sub".to_string()],
        "被拒绝的请求没有创建任何文件"
    );

    // 保留地址：授权目录名不能是资料根里已被 `history` 逻辑地址占用的真实条目。
    std::fs::create_dir_all(materials.join("history")).expect("占住保留地址");
    assert!(
        FileView::with_history_and_creation(
            &materials,
            &temp.env(&agent_a()),
            Some(&grant(&agent_a(), "run-1", "notes"))
        )
        .is_err(),
        "资料根里出现真实 history 条目时必须拒绝装配"
    );
}

// ---------------------------------------------------------------------------
// 3. 准入在 Prepared 之前：拒绝不留行动事实
// ---------------------------------------------------------------------------

#[test]
fn admission_precedes_prepared_and_a_refusal_leaves_no_action_facts() {
    let temp = TempDir::new("create-admission");
    let materials = temp.materials();
    let env_dir = temp.path().join("env");
    assert_eq!(fixture_init(&env_dir, "agent-creation", 0), 0, "初始化环境");

    // 授权给 `notes`，但脚本**先不展开**就请求创建：这一步必须被拒绝。
    let refused = fixture(
        &env_dir,
        "agent-creation",
        &materials,
        "run-refused",
        "script",
    )
    // ⚠️ 这条测的是**拒绝路径**（"拒绝不得留下任何行动事实"）⇒ **不补**"先查"：
    // 补了那次 inspect 本身会变成一条获准行动，把被测的语义破坏掉。
    .script("act:files_create?name=note.txt&text=x;finish")
    .create_dir("notes")
    .run();
    assert_eq!(code(&refused), 0, "{}", stderr(&refused));
    let report = stdout(&refused);
    assert!(
        report.contains("FIXTURE:saw-refusal=files_create:not-expanded"),
        "拒绝必须对下一次模型调用可见: {report}"
    );
    assert!(
        !materials.join("notes/note.txt").exists(),
        "拒绝不得创建文件"
    );

    // 事实侧：这次活动只有"开始"和"收场"，**一条行动事实都没有**。
    let diagnosed = diagnose(&env_dir, "agent-creation", "run-refused");
    let line = activity_line(&diagnosed, "run-refused");
    assert!(line.contains("prepared=0"), "{line}");
    assert!(line.contains("dispatched=0"), "{line}");
    assert!(line.contains("failure=0"), "{line}");
    assert!(line.contains("success=0"), "{line}");

    // 拒绝经历本身是可读的：它进了这次活动的经历（kind=injection + 阶段 admission-refused）。
    let trace = std::fs::read_to_string(env_dir.join("trace.ndjson")).expect("读经历");
    assert!(
        trace.contains("\"stage\":\"admission-refused\""),
        "拒绝经历必须按既定编码落盘: {trace}"
    );
    assert!(trace.contains("\"action\""), "{trace}");
    assert!(trace.contains("\"admission\""), "{trace}");
    assert!(
        !trace.contains("\"kind\":\"observed\""),
        "拒绝不得被写成已执行的结果: {trace}"
    );
}

#[test]
fn an_admitted_action_keeps_the_existing_confirmation_chain() {
    let temp = TempDir::new("create-admitted");
    let materials = temp.materials();
    let env_dir = temp.path().join("env");
    assert_eq!(fixture_init(&env_dir, "agent-creation", 0), 0);

    let text = "留给下一次清醒期的正文";
    let run = fixture(&env_dir, "agent-creation", &materials, "run-ok", "script")
        .script(&format!(
            "act:open_app?target=files;act:inspect_action?action=files_create;act:files_create?name=note.txt&text={text};finish"
        ))
        .create_dir("notes")
        .run();
    assert_eq!(code(&run), 0, "{}", stderr(&run));
    let report = stdout(&run);
    assert!(report.contains("ending=subject:ended"), "{report}");
    assert!(
        report.contains("creation=create_text@notes"),
        "报告必须说清这一次活动有没有写权限: {report}"
    );
    assert!(
        report.contains("FIXTURE:saw-result=files_create:success:none"),
        "{report}"
    );
    assert_eq!(
        std::fs::read_to_string(materials.join("notes/note.txt")).expect("读回"),
        text
    );

    // 获准的那一次仍然走完整确认链：准备、发出、结果各一条，且结果成功。
    let diagnosed = diagnose(&env_dir, "agent-creation", "run-ok");
    // **三次**获准行动：展开（OS 自有）＋ **看格式**（新形态下调用前那一轮）＋ 创建，各走一遍完整确认链。
    let line = activity_line(&diagnosed, "run-ok");
    assert!(line.contains("prepared=3"), "{line}");
    assert!(line.contains("dispatched=3"), "{line}");
    assert!(line.contains("success=3"), "{line}");
    assert!(line.contains("failure=0"), "{line}");
    assert!(line.contains("outcome=subject:ended"), "{line}");
}

// ---------------------------------------------------------------------------
// 4. 跨 Run：拒绝可读、延后的合法行动仍能被诊断还原
// ---------------------------------------------------------------------------

#[test]
fn many_refusals_then_a_late_admitted_action_survives_restart_and_diagnostics() {
    let temp = TempDir::new("create-late-turn");
    let materials = temp.materials();
    let env_dir = temp.path().join("env");
    assert_eq!(fixture_init(&env_dir, "agent-creation", 0), 0);

    // 八次被拒绝的创建请求（每次一个 Turn，零行动事实），之后才展开并合法创建。
    let mut script = "act:files_create?name=x.txt&text=x;".repeat(8); // 同上：测拒绝路径，不补先查
    script.push_str(
        "act:open_app?target=files;act:inspect_action?action=files_create;act:files_create?name=late.txt&text=late;finish", // 测拒绝路径（not-expanded），不补先查
    );
    let run = fixture(&env_dir, "agent-creation", &materials, "run-late", "script")
        .max_turns(12)
        .script(&script)
        .create_dir("notes")
        .run();
    assert_eq!(code(&run), 0, "{}", stderr(&run));
    assert!(
        stdout(&run).contains("FIXTURE:saw-refusal=files_create:not-expanded"),
        "{}",
        stdout(&run)
    );

    // 合法行动发生在第 9 个 Turn，而这次活动只有 5 行事实：
    // "事实行数 + 1"这种推导上界会把合法历史误判成越界，这里必须**原样**还原。
    let diagnosed = diagnose(&env_dir, "agent-creation", "run-late");
    let line = activity_line(&diagnosed, "run-late");
    // 11 行来自**三次**获准行动（展开 / **看格式** / 创建，各 3 条）+ 开始 + 收场；
    // 合法行动在第 11 个 Turn，大于"事实行数 + 1"，所以只有数字构造入口能准确还原它。
    assert!(line.contains("fact-rows=11"), "{line}");
    assert!(line.contains("last-action-turn=11"), "{line}");
    assert!(line.contains("prepared=3"), "{line}");
    assert!(line.contains("success=3"), "{line}");
    assert!(line.contains("outcome=subject:ended"), "{line}");

    // 独立的新进程读回历史：拒绝分类必须能在跨 Run 经历里被读到（走通用文件能力）。
    let read_back = fixture(
        &env_dir,
        "agent-creation",
        &materials,
        "run-reader",
        "read-back",
    )
    // TASK_69：**看格式要占一轮** —— 第 1 轮看 OS 动词、展开**之后**再一轮看 files 动作
    // （`inspect` 的范围是"此刻真能调的"，所以分两批），加上原 4 步 ⇒ 6 拍不够。
    // 这不是放宽校验，是承认"先看格式"确实要走两轮。
    .max_turns(8)
    .input("读回上一次的经历")
    .probe("admission-refused")
    .run();
    // 失败时**两边都报**：这个夹具的读数走 stdout（`FIXTURE:` 行），只报 stderr 会是一片空白。
    assert_eq!(
        code(&read_back),
        0,
        "读回那一次该正常收场；stdout={}；stderr={}",
        stdout(&read_back),
        stderr(&read_back)
    );
    let report = stdout(&read_back);
    assert_eq!(
        fixture_field(&report, "first-view-had-probe"),
        "false",
        "新 Run 首轮不得自动带入旧正文"
    );
    assert_eq!(fixture_field(&report, "read-back-had-probe"), "true");
}

// ---------------------------------------------------------------------------
// 5. Turn 的数字构造与超界拒绝
// ---------------------------------------------------------------------------

#[test]
fn turn_numbers_round_trip_exactly_and_oversize_stored_values_are_refused() {
    // 数字构造入口覆盖整个可表示范围（常数时间，不步进、不推算）。
    assert_eq!(Turn::from_number(0), None, "0 是活动开始之前不存在的 Turn");
    assert_eq!(Turn::from_number(1), Some(Turn::FIRST));
    assert_eq!(Turn::from_number(7).map(Turn::number), Some(7));
    assert_eq!(
        Turn::from_number(1_000_000).map(Turn::number),
        Some(1_000_000)
    );
    assert_eq!(
        Turn::from_number(u32::MAX).map(Turn::number),
        Some(u32::MAX),
        "上界必须精确保留，不能截断或饱和"
    );

    // 盘上出现超出 u32 的 Turn：整行读不出来，绝不截断成一个合法 Turn。
    let temp = TempDir::new("create-oversize-turn");
    let env_dir = temp.path().join("env");
    assert_eq!(fixture_init(&env_dir, "agent-oversize", 0), 0);
    let facts = env_dir.join("facts.ndjson");
    let mut rows = std::fs::read_to_string(&facts).expect("读事实");
    rows.push_str(&format!(
        "{{\"seq\":{},\"run\":\"run-oversize\",\"fact\":\"run-started\",\"agent\":\"agent-oversize\"}}\n",
        99_999
    ));
    rows.push_str(&format!(
        "{{\"seq\":{},\"run\":\"run-oversize\",\"fact\":\"action-prepared\",\"turn\":{}}}\n",
        100_000,
        u64::from(u32::MAX) + 1
    ));
    std::fs::write(&facts, &rows).expect("写事实");

    let diagnosed = Command::new(binary("environment_diagnose"))
        .args([path_text(&env_dir), "agent-oversize".to_string()])
        .output()
        .expect("启动诊断命令");
    assert_eq!(
        diagnosed.status.code(),
        Some(2),
        "超出 u32 的 Turn 必须让环境读不出来，而不是截断成合法值: stdout={} stderr={}",
        String::from_utf8_lossy(&diagnosed.stdout),
        String::from_utf8_lossy(&diagnosed.stderr)
    );
    assert!(
        String::from_utf8_lossy(&diagnosed.stdout).is_empty(),
        "读不出来时不得产出看起来正常的报告"
    );
}

// ---------------------------------------------------------------------------
// 6. 拒绝经历保存失败：不再有下一轮
// ---------------------------------------------------------------------------

#[test]
fn a_refusal_that_cannot_be_saved_stops_before_the_next_model_call() {
    let temp = TempDir::new("create-refusal-save-fail");
    let materials = temp.materials();
    let env_dir = temp.path().join("env");
    let mut env = Environment::create(&env_dir, agent_a()).expect("建环境");

    // 受控故障夹具：第 N 次 `commit_trace` 开始失败（每一次都发生在**拒绝经历**上）。
    let mut model = InspectsFirst::new(ScriptedModel::new(vec![
        // 第一次：未展开就请求创建 → 拒绝 → 保存这条拒绝经历时失败。
        ModelOutput::act(create("note.txt", "x")),
        ModelOutput::finish(SubjectEnding::Completed),
    ]));
    let mut agent = Agent::new(agent_a());
    let mut ledger = celira_agent::foundation::Ledger::new(agent_a());
    let mut commit = FailEveryCommit::new(EnvironmentCommit::new(&mut env));
    let mut os = os_with_grant(&materials, &env_dir, "notes", "run-save-fail");
    let mut expansion5 = RunExpansion::new(agent_a(), RunId::new("run-save-fail"));
    let mut scope = os.scope(&mut expansion5);
    let result = run_activity_with(
        &mut agent,
        RunRequest {
            run: RunId::new("run-save-fail"),
            limits: Limits::new(4),
            input: RunInput::empty(),
            source: &mut NoContext,
            commit: &mut commit,
            cost: &no_cost,
        },
        &mut model,
        &mut scope,
        &mut ledger,
    );

    assert!(
        matches!(result, Err(RunError::Commit(_))),
        "拒绝经历保存不下去时必须停在这条提交上: {result:?}"
    );
    assert_eq!(
        model.calls(),
        1,
        "保存失败之后不得再调用模型（下一次模型调用之前必须确认已经保存）"
    );
    assert_eq!(model.calls(), 1, "也不得自动重试");
    let facts = format!("{:?}", ledger.records());
    for fabricated in ["ActionPrepared", "ActionDispatched", "ActionResultRecorded"] {
        assert!(!facts.contains(fabricated), "{facts}");
    }
}

// ---------------------------------------------------------------------------
// 7. 文件效果已发生、结果事实保存失败：不重新执行
// ---------------------------------------------------------------------------

#[test]
fn a_created_file_whose_result_cannot_be_saved_is_not_created_again() {
    let temp = TempDir::new("create-result-save-fail");
    let materials = temp.materials();
    let env_dir = temp.path().join("env");
    let mut env = Environment::create(&env_dir, agent_a()).expect("建环境");

    let mut model = InspectsFirst::new(ScriptedModel::new(vec![
        ModelOutput::act(expand(FILE_APP_ID)),
        ModelOutput::act(create("keep.txt", "已经写在盘上了")),
        ModelOutput::finish(SubjectEnding::Completed),
    ]));
    let mut agent = Agent::new(agent_a());
    let mut ledger = celira_agent::foundation::Ledger::new(agent_a());
    // 这一段持有环境写资格：跑完就释放，后面才轮得到另一个进程（诊断命令）去读它。
    let result = {
        // TASK_69B → (b)：让**创建那个动作**的结果事实失败。
        //
        // ⚠️ 这里曾经写"第 3 个结果"——那是**实现巧合**：主体先展开、再按批量 inspect 看格式
        // （trace 里是 Turn 2 展开、Turn 3 一次 inspect 七个 files 动作），于是"第几个结果"
        // 随手数一变就错位；而 `InspectsFirst` 一次返回**整批** unseen ⇒
        // 结果数与轮数**根本不同步**。改成**按动作标识**：与主体查了几个**解耦**。
        //
        // ⚠️ 名字是 **`files_create`**（这个舞台装配的是文件视图；`notes/` 只是资料目录下的
        // 子目录名，不是 notes app 的动作）。
        let mut commit = FailAtResult::new(EnvironmentCommit::new(&mut env), "files_create");
        let mut os = os_with_grant(&materials, &env_dir, "notes", "run-result-fail");
        let mut expansion6 = RunExpansion::new(agent_a(), RunId::new("run-result-fail"));
        let mut scope = os.scope(&mut expansion6);
        run_activity_with(
            &mut agent,
            RunRequest {
                run: RunId::new("run-result-fail"),
                // TASK_69B：新形态下主体**多了一轮**（调用前先看格式）⇒ 预算相应放宽一格；
                // 这不是放宽校验，是承认"先查格式"确实要走一轮。
                limits: Limits::new(6),
                input: RunInput::empty(),
                source: &mut NoContext,
                commit: &mut commit,
                cost: &no_cost,
            },
            &mut model,
            &mut scope,
            &mut ledger,
        )
    };

    let Err(RunError::Commit(error)) = result else {
        panic!("应当在结果事实这一步失败: {result:?}");
    };
    assert_eq!(error.stage(), CommitStage::ActionResult);
    assert!(!error.after_confirmed_ending(), "活动还没有结束");

    // 效果确实发生了：文件就在盘上，而且内容完整——所以**不得**说"没有发生"。
    assert_eq!(
        std::fs::read_to_string(materials.join("notes/keep.txt")).expect("文件应当已经存在"),
        "已经写在盘上了"
    );
    // 也不重新执行：模型只被调用了两次（展开 + 创建），没有第三次尝试。
    assert_eq!(
        model.calls(),
        4,
        "保存失败不得触发重新执行（主体一共被叫 4 次：展开前查 / 展开 / 展开后查 / 创建）"
    );
    assert_eq!(
        std::fs::read_dir(materials.join("notes"))
            .expect("列授权目录")
            .count(),
        1,
        "只应有那一个文件：不补写、不重试"
    );
    // 盘上的事实才是权威：准备与发出都在，创建那一次的结果**没有**被确认。
    // （内存账本里那一条已经写下了，但"写进内存"与"确认为耐久"是两件事——正是这里要区分的。）
    let facts_on_disk = std::fs::read_to_string(env_dir.join("facts.ndjson")).expect("读事实");
    assert!(facts_on_disk.contains("action-prepared"), "{facts_on_disk}");
    assert!(
        facts_on_disk.contains("action-dispatched"),
        "{facts_on_disk}"
    );
    // 创建那一次的结果**没有**被确认——但判据**别写成"只有展开那一次"**：
    // 那是**旧形状**（一轮一个动作）的产物。现在主体开场会**批量** inspect
    // （本轮 trace：Turn 1 一次五个 OS 动作、Turn 3 一次七个 files 动作），
    // 那些 inspect 的结果**照样该被确认** ⇒ 总数随手数一变就错。
    // ⇒ 判据落在**目标动作**上：**恰好最后一次派发没有结果**（前面每一次都有）。
    let dispatched = facts_on_disk
        .matches("\"fact\":\"action-dispatched\"")
        .count();
    let results = facts_on_disk.matches("\"fact\":\"action-result\"").count();
    assert!(dispatched > 0, "至少要有一次派发才有得判: {facts_on_disk}");
    assert_eq!(
        results,
        dispatched - 1,
        "恰好**最后一次**派发没有结果——就是创建那一次: {facts_on_disk}"
    );

    // 既有诊断把它如实读成"发出过、但没有结果记录"——文件效果已经发生这件事不会被瞒掉。
    drop(env);
    let diagnosed = diagnose(&env_dir, "agent-creation", "run-result-fail");
    let line = activity_line(&diagnosed, "run-result-fail");
    assert!(line.contains("unconfirmed=1"), "{line}");
    assert!(line.contains("dispatched-without-result=1"), "{line}");
    assert!(line.contains("outcome=none"), "没有收尾事实: {line}");
}

// ---------------------------------------------------------------------------
// 8. 真实适配层回环：拒绝原因对模型可见
// ---------------------------------------------------------------------------

#[test]
fn a_real_loopback_request_can_see_the_refusal_reason() {
    let temp = TempDir::new("create-loopback");
    let materials = temp.materials();
    let env_dir = temp.path().join("env");
    assert_eq!(fixture_init(&env_dir, "agent-wire", 0), 0);

    // 服务依次应答：先请求一个**还没展开**的创建动作，再收场。
    let (endpoint, requests) = spawn_reply_server();
    let run = fixture(&env_dir, "agent-wire", &materials, "run-wire", "loopback")
        .max_turns(4)
        .endpoint(&endpoint, "fixture-sentinel-credential")
        .create_dir("notes")
        .run();
    assert_eq!(code(&run), 0, "{}", stderr(&run));

    let bodies = requests
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("回环服务应当收到两轮请求");
    let (first, second) = bodies
        .split_once("\n=====REQUEST=====\n")
        .expect("两轮请求");
    assert!(!first.contains("action-refused"), "{first}");
    assert!(
        !first.contains("\"role\":\"tool\""),
        "首轮请求还没有任何回执可配对: {first}"
    );
    // 第二轮：拒绝以**原生协议回执**的形状出现——与那次请求的调用标识相邻配对，
    // 而不是被摊平成普通正文里的一个"来源事实"。
    assert!(
        second.contains("\"role\":\"assistant\"") && second.contains("\"name\":\"files_create\""),
        "那次请求本身必须作为助手帧出现: {second}"
    );
    assert!(
        second.contains("\"role\":\"tool\"") && second.contains("\"call-create\""),
        "拒绝必须与原请求的调用标识配对: {second}"
    );
    assert!(
        second.contains("admission = not-expanded"),
        "真实请求里必须能看见拒绝原因: {second}"
    );
    assert!(
        second.contains("dispatched = false"),
        "拒绝说的是「没有派发过」: {second}"
    );
    assert!(
        !second.contains("state = "),
        "拒绝不得长得像行动结果: {second}"
    );
}

// ---------------------------------------------------------------------------
// 9. 旧入口保持只读
// ---------------------------------------------------------------------------

#[test]
fn the_old_read_only_entry_points_gain_nothing() {
    let temp = TempDir::new("create-old-entries");
    let materials = temp.materials();
    let mut os = MinimalOs::open(&materials).expect("旧入口");
    let mut expansion7 = RunExpansion::new(agent_a(), RunId::new("run-1"));
    let mut scope = os.scope(&mut expansion7);
    scope.execute(&agent_a(), &op("run-1"), &expand(FILE_APP_ID));
    let ids: Vec<String> = scope
        .capabilities(&agent_a(), &RunId::new("run-1"))
        .items()
        .iter()
        .map(|capability| capability.id().as_str().to_string())
        .collect();
    assert!(!ids.contains(&"files_create".to_string()), "{ids:?}");
    assert_eq!(
        scope.admit(&agent_a(), &RunId::new("run-1"), &create("a.txt", "x")),
        ActionAdmission::refused("unknown-action")
    );

    // 文件 App 直接装配时也一样：没有授权就没有创建能力。
    let app = celira_agent::file_app::FileApp::open(&materials).expect("只读 App");
    let declared: Vec<String> = app
        .capabilities(&agent_a(), &RunId::new("run-1"))
        .items()
        .iter()
        .map(|capability| capability.id().as_str().to_string())
        .collect();
    assert!(
        !declared.contains(&"files_create".to_string()),
        "{declared:?}"
    );
}

// ---------------------------------------------------------------------------
// 夹具与工具
// ---------------------------------------------------------------------------

fn agent_a() -> AgentId {
    AgentId::new("agent-creation")
}

fn agent_b() -> AgentId {
    AgentId::new("agent-other")
}

fn grant(agent: &AgentId, run: &str, directory: &str) -> CreationGrant {
    CreationGrant::new(agent.clone(), RunId::new(run), directory).expect("授权形状合法")
}

fn os_with_grant(materials: &Path, env_root: &Path, directory: &str, run: &str) -> MinimalOs {
    MinimalOs::with_history_and_creation(
        materials,
        env_root,
        Some(&grant(&agent_a(), run, directory)),
    )
    .expect("装配带授权的视图")
}

fn op(run: &str) -> celira_agent::foundation::OperationId {
    celira_agent::foundation::OperationId::new(RunId::new(run), Turn::FIRST)
}

fn expand(target: &str) -> Action {
    Action::with_params(
        ActionId::new("open_app"),
        ActionParams::none().with("target", target),
    )
}

/// `inspect_action` 的载荷（TASK_69：调用之前"先看格式"那一轮）。
fn inspect(name: &str) -> Action {
    Action::with_params(
        ActionId::new("inspect_action"),
        ActionParams::none().with("action", name),
    )
}

fn list(path: &str) -> Action {
    Action::with_params(
        ActionId::new("files_list"),
        ActionParams::none().with("path", path),
    )
}

fn create(name: &str, text: &str) -> Action {
    Action::with_params(
        ActionId::new("files_create"),
        ActionParams::none().with("name", name).with("text", text),
    )
}

/// 每一步提交都失败的接缝：用来把"拒绝经历保存不下去"变成可复现的形状。
struct FailEveryCommit<'a> {
    inner: EnvironmentCommit<'a>,
}

impl<'a> FailEveryCommit<'a> {
    fn new(inner: EnvironmentCommit<'a>) -> Self {
        Self { inner }
    }
}

impl celira_agent::core::CommitSeam for FailEveryCommit<'_> {
    fn commit_run_start(&mut self, agent: &AgentId, run: &RunId) -> Result<(), CommitError> {
        self.inner.commit_run_start(agent, run)
    }
    fn commit_input(&mut self, run: &RunId, input: &RunInput) -> Result<(), CommitError> {
        self.inner.commit_input(run, input)
    }
    fn commit_trace(
        &mut self,
        _run: &RunId,
        _turn: Option<Turn>,
        _entry: &TraceEntry,
    ) -> Result<(), CommitError> {
        Err(CommitError::at(CommitStage::Trace))
    }
    fn commit_action_selected(
        &mut self,
        run: &RunId,
        operation: &celira_agent::foundation::OperationId,
        action: &Action,
    ) -> Result<(), CommitError> {
        self.inner.commit_action_selected(run, operation, action)
    }
    fn commit_action_dispatched(
        &mut self,
        run: &RunId,
        operation: &celira_agent::foundation::OperationId,
    ) -> Result<(), CommitError> {
        self.inner.commit_action_dispatched(run, operation)
    }
    fn commit_action_result(
        &mut self,
        run: &RunId,
        operation: &celira_agent::foundation::OperationId,
        outcome: &ActionOutcome,
    ) -> Result<(), CommitError> {
        self.inner.commit_action_result(run, operation, outcome)
    }
    fn commit_ending(
        &mut self,
        run: &RunId,
        outcome: celira_agent::foundation::Outcome,
        cost: celira_agent::cost::RunCost,
    ) -> Result<(), CommitError> {
        self.inner.commit_ending(run, outcome, cost)
    }
    fn seal(&mut self, run: &RunId) -> Result<(), CommitError> {
        self.inner.seal(run)
    }
}

/// 只在"结果事实"这一步失败的接缝。
///
/// ⚠️ **按动作标识失败**（TASK_69，(b) 裁决）。这里曾经是 `remaining_ok: usize`（"还允许几次成功"）——
/// 那是**实现巧合**：一轮的**批量** inspect（`InspectsFirst` 在"有没看过的动作"时一次返回整批）
/// 会让**结果数 ≠ 轮数**（展开后新出现的 K 个动作，那一轮就产生 K 个结果），于是
/// `remaining_ok = 3` 在那批 inspect 里就耗光 ⇒ 轮到 create 时直接失败 ⇒ **create 从未执行**。
/// ⇒ ⇒ "第 N 个结果"根本表达不了"**某个动作**的结果"。改成**按名字**：与"主体查了几个"**解耦**。
struct FailAtResult<'a> {
    inner: EnvironmentCommit<'a>,
    /// **要失败的那个动作标识**：当结果属于它时，这一步失败。
    target: String,
    /// 最近一次 `commit_action_selected` 记下的（操作身份 ＋ 动作标识）。
    ///
    /// 为什么记它：`OperationId` 只有 `{run, turn, index}`，**不含动作名**；而
    /// `commit_action_selected` 带着 `action` ⇒ 在这里把两者绑起来，
    /// `commit_action_result` 才判得出"这次结果是不是那个动作的"。
    /// 选择与结果**成对紧邻**（selected → result），所以"最近一次"就够——一轮多动作时也照样成对。
    selected: Option<(celira_agent::foundation::OperationId, String)>,
}

impl<'a> FailAtResult<'a> {
    /// `target` 是**动作标识**（如 `notes_create`），不是"第几个结果"。
    fn new(inner: EnvironmentCommit<'a>, target: &str) -> Self {
        Self {
            inner,
            target: target.to_string(),
            selected: None,
        }
    }
}

impl celira_agent::core::CommitSeam for FailAtResult<'_> {
    fn commit_run_start(&mut self, agent: &AgentId, run: &RunId) -> Result<(), CommitError> {
        self.inner.commit_run_start(agent, run)
    }
    fn commit_input(&mut self, run: &RunId, input: &RunInput) -> Result<(), CommitError> {
        self.inner.commit_input(run, input)
    }
    fn commit_trace(
        &mut self,
        run: &RunId,
        turn: Option<Turn>,
        entry: &TraceEntry,
    ) -> Result<(), CommitError> {
        self.inner.commit_trace(run, turn, entry)
    }
    fn commit_action_selected(
        &mut self,
        run: &RunId,
        operation: &celira_agent::foundation::OperationId,
        action: &Action,
    ) -> Result<(), CommitError> {
        // 把（操作身份 → 动作标识）绑起来：`commit_action_result` 只拿得到前者。
        self.selected = Some((operation.clone(), action.id().as_str().to_string()));
        self.inner.commit_action_selected(run, operation, action)
    }
    fn commit_action_dispatched(
        &mut self,
        run: &RunId,
        operation: &celira_agent::foundation::OperationId,
    ) -> Result<(), CommitError> {
        self.inner.commit_action_dispatched(run, operation)
    }
    fn commit_action_result(
        &mut self,
        run: &RunId,
        operation: &celira_agent::foundation::OperationId,
        outcome: &ActionOutcome,
    ) -> Result<(), CommitError> {
        // **按动作标识判**：这次结果属于那个动作 ⇒ 就失败在这一步。
        let is_target = self
            .selected
            .as_ref()
            .is_some_and(|(selected_op, id)| selected_op == operation && *id == self.target);
        if is_target {
            return Err(CommitError::at(CommitStage::ActionResult));
        }
        self.inner.commit_action_result(run, operation, outcome)
    }
    fn commit_ending(
        &mut self,
        run: &RunId,
        outcome: celira_agent::foundation::Outcome,
        cost: celira_agent::cost::RunCost,
    ) -> Result<(), CommitError> {
        self.inner.commit_ending(run, outcome, cost)
    }
    fn seal(&mut self, run: &RunId) -> Result<(), CommitError> {
        self.inner.seal(run)
    }
}

/// 按脚本作答的模型（记录被调用了多少次）。
struct ScriptedModel {
    script: std::collections::VecDeque<ModelOutput>,
    calls: usize,
}

impl ScriptedModel {
    fn new(script: Vec<ModelOutput>) -> Self {
        Self {
            script: std::collections::VecDeque::from(script),
            calls: 0,
        }
    }
}

impl ModelPort for ScriptedModel {
    fn complete(&mut self, _view: &View) -> Result<ModelOutput, ModelError> {
        self.calls += 1;
        self.script.pop_front().ok_or(scripted_model_failure())
    }
}

/// 夹具：一次运行的全部参数。
struct Fixture {
    variables: Vec<(&'static str, String)>,
}

impl Fixture {
    fn new(root: &Path, agent: &str, materials: &Path, run: &str, model: &str) -> Self {
        Self {
            variables: vec![
                ("MANUAL_FIXTURE_ROOT", path_text(root)),
                ("MANUAL_FIXTURE_AGENT", agent.to_string()),
                ("MANUAL_FIXTURE_MATERIALS", path_text(materials)),
                ("MANUAL_FIXTURE_RUN", run.to_string()),
                ("MANUAL_FIXTURE_MAX_TURNS", "6".to_string()),
                ("MANUAL_FIXTURE_MODEL", model.to_string()),
            ],
        }
    }

    fn max_turns(mut self, turns: u32) -> Self {
        self.set("MANUAL_FIXTURE_MAX_TURNS", turns.to_string());
        self
    }

    fn script(mut self, script: &str) -> Self {
        self.set("MANUAL_FIXTURE_SCRIPT", script.to_string());
        self
    }

    fn input(mut self, text: &str) -> Self {
        self.set("MANUAL_FIXTURE_INPUT", text.to_string());
        self
    }

    fn probe(mut self, text: &str) -> Self {
        self.set("MANUAL_FIXTURE_PROBE", text.to_string());
        self
    }

    fn create_dir(mut self, directory: &str) -> Self {
        self.set("MANUAL_FIXTURE_CREATE_DIR", directory.to_string());
        self
    }

    fn endpoint(mut self, endpoint: &str, credential: &str) -> Self {
        self.set("MANUAL_FIXTURE_ENDPOINT", endpoint.to_string());
        self.set("MANUAL_FIXTURE_CREDENTIAL", credential.to_string());
        self.set("MANUAL_FIXTURE_MODEL_NAME", "fixture-model".to_string());
        self
    }

    fn set(&mut self, name: &'static str, value: String) {
        if let Some(entry) = self.variables.iter_mut().find(|entry| entry.0 == name) {
            entry.1 = value;
        } else {
            self.variables.push((name, value));
        }
    }

    fn run(self) -> Output {
        let mut command = Command::new(binary("manual_entry_fixture"));
        for (name, value) in &self.variables {
            command.env(name, value);
        }
        command
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost")
            .env_remove("DEEPSEEK_API_KEY");
        command.output().expect("启动夹具")
    }
}

fn fixture(root: &Path, agent: &str, materials: &Path, run: &str, model: &str) -> Fixture {
    Fixture::new(root, agent, materials, run, model)
}

/// 用生产命令初始化环境（顺带证明 init 路径不碰模型）。
fn fixture_init(env_dir: &Path, agent: &str, _unused: u8) -> i32 {
    let output = Command::new(binary("agent_run"))
        .args(["init", "--root", &path_text(env_dir), "--agent", agent])
        .env_remove("DEEPSEEK_API_KEY")
        .output()
        .expect("启动生产命令");
    output.status.code().unwrap_or(-1)
}

fn diagnose(env_dir: &Path, agent: &str, run: &str) -> String {
    let output = Command::new(binary("environment_diagnose"))
        .args([
            path_text(env_dir),
            agent.to_string(),
            "--run".to_string(),
            run.to_string(),
        ])
        .output()
        .expect("启动诊断命令");
    assert_eq!(
        output.status.code(),
        Some(0),
        "诊断应当成功: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn activity_line(text: &str, run: &str) -> String {
    let needle = format!("run={run} ");
    text.lines()
        .find(|line| line.starts_with("activity ") && line.contains(&needle))
        .unwrap_or_else(|| panic!("诊断报告里没有 {run} 的活动行: {text}"))
        .to_string()
}

/// 回环服务：第一轮请求创建（未展开 → 拒绝），第二轮收场。
fn spawn_reply_server() -> (String, std::sync::mpsc::Receiver<String>) {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("可以绑定回环端口");
    let port = listener.local_addr().expect("有地址").port();
    let (sender, receiver) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        // 两轮应答都按**原生工具调用**的形状：`tool_calls[].function` + 字符串里的 JSON 参数。
        const CREATE: &str = r#"{"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"call-create","type":"function","function":{"name":"files_create","arguments":"{\"name\":\"note.txt\",\"text\":\"x\"}"}}]},"finish_reason":"tool_calls"}]}"#;
        const FINISH: &str = r#"{"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"call-finish","type":"function","function":{"name":"finish_run","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#;
        let mut bodies = Vec::new();
        for reply in [CREATE, FINISH] {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
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
            bodies.push(if reader.read_exact(&mut buffer).is_ok() {
                String::from_utf8_lossy(&buffer).to_string()
            } else {
                String::new()
            });
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.len(),
                reply
            );
            let _ = stream.flush();
        }
        let _ = sender.send(bodies.join("\n=====REQUEST=====\n"));
    });

    (
        format!("http://127.0.0.1:{port}/chat/completions"),
        receiver,
    )
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn fixture_field(text: &str, key: &str) -> String {
    let needle = format!("FIXTURE:{key}=");
    text.lines()
        .find_map(|line| line.strip_prefix(&needle))
        .unwrap_or_else(|| panic!("夹具报告里没有 {key}: {text}"))
        .to_string()
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut path = std::env::temp_dir();
        path.push(format!("celira-task05-{label}-{}-{n}", std::process::id()));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("清掉同名残留目录");
        }
        std::fs::create_dir_all(&path).expect("建立测试目录");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// 一个**真实存在**的身份环境目录：身份环境是别的容器，装配要读它，所以不能假装它在。
    fn env(&self, agent: &AgentId) -> PathBuf {
        let env_dir = self.path.join("env");
        if !env_dir.exists() {
            drop(Environment::create(&env_dir, agent.clone()).expect("建环境"));
        }
        env_dir
    }

    /// 资料根（本包自建的隔离夹具，绝不碰用户资料）。
    fn materials(&self) -> PathBuf {
        let materials = self.path.join("materials");
        std::fs::create_dir_all(materials.join("notes")).expect("建授权子目录");
        materials
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
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

// ---------------------------------------------------------------------------
// TASK_37 改名回归：工具名 = App 名 `_` 动词，旧名一个都不许留在表里
// ---------------------------------------------------------------------------

/// 工具表里只有**带 App 前缀的新名**，旧的一批裸名/半裸名一个都不在。
///
/// 判据落在**机制**上（表里有哪些名字），不落在任何行为上：动词名不落盘、Run 装配时注入，
/// 所以改名零迁移——主体的词汇表一次切换，不存在"兼容旧名"的双轨期。
#[test]
fn the_tool_table_carries_only_the_prefixed_names_and_keeps_no_legacy_alias() {
    const LEGACY: [&str; 9] = [
        "list",
        "read_text",
        "find_text",
        "create_text",
        "verify_text",
        "create_note",
        "read_note",
        "list_notes",
        "verify_note",
    ];
    // 三件只读 + 两件写 + 各自复核。
    const FILES: [&str; 7] = [
        "files_list",
        "files_read",
        "files_grep",
        "files_create",
        "files_verify_text",
        "files_edit",
        "files_verify_edit",
    ];

    let temp = TempDir::new("task37-naming");
    let materials = temp.path().join("materials");
    let env_dir = temp.path().join("env");
    std::fs::create_dir_all(materials.join("notes")).expect("建资料目录");
    drop(Environment::create(&env_dir, agent_a()).expect("建环境"));

    let mut os = os_with_grant(&materials, &env_dir, "notes", "run-1");
    let mut expansion = RunExpansion::new(agent_a(), RunId::new("run-1"));
    let mut scope = os.scope(&mut expansion);
    scope.execute(&agent_a(), &op("run-1"), &expand(FILE_APP_ID));
    let ids: Vec<String> = scope
        .capabilities(&agent_a(), &RunId::new("run-1"))
        .items()
        .iter()
        .map(|capability| capability.id().as_str().to_string())
        .collect();

    for name in FILES {
        assert!(ids.contains(&name.to_string()), "{name} 必须在场: {ids:?}");
    }
    for name in LEGACY {
        assert!(
            !ids.contains(&name.to_string()),
            "旧名 {name} 不许残留（没有兼容旧名的分支）: {ids:?}"
        );
    }
}
