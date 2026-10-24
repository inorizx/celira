//! TASK_06 验收：**独立 Notes App 与第二个 App 的解耦**。
//!
//! 这一份文件要证明的不是"多了一个 App"，而是：**App 的独立性由第二个真实消费者证明**——
//! Notes 有自己的存储、身份、动作与失败词汇，只通过同一份窄契约接入，不认识文件 App，
//! 也不被 Core 认识。
//!
//! 证据分级：
//!
//! - **类型保证（TY）**：`ActionPort::admit` 是必填方法；两个 App 的**边界**彼此独立
//!   （材料目录的 `CreationGrant` 要授权、主体的笔记根本不需要授权——分界线是"写到谁的地盘"）；
//! - **源码守卫（SG）**：`notes_app.rs` 不得出现 `file_app` / `file_view`（`tests/minimal_agent.rs`）；
//! - **行为测试（BT）**：本文件；
//! - **静态审查（SR）**：Notes 的存储格式与事实格式无关，正文不进 `Record`。
//!
//! 明确**不**声称的：写入或同步失败的**部分效果**没有故障注入接缝（本包不为测试在生产路径上
//! 开一个），那两格按 `Unknown` 设计但**未注入验证**；`note-id-taken` 只在真实并发下可达。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use celira_agent::clock::ClockHandle;
use celira_agent::commit::EnvironmentCommit;
use celira_agent::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Agent, CommitError,
    CommitStage, Limits, ModelError, ModelFailureClass, ModelOutput, ModelPort, NoContext,
    RunError, RunInput, RunRequest, View, no_cost, run_activity_with,
};
use celira_agent::file_view::FileView;
use celira_agent::foundation::{AgentId, OperationId, ResultState, RunId, SubjectEnding, Turn};
use celira_agent::notes_app::{
    MAX_LISTED_NOTES, MAX_NOTE_BYTES, MAX_TITLE_BYTES, NOTES_APP_ID, NotesApp, NotesFailure,
};
use celira_agent::os::{MinimalOs, RunExpansion};
use celira_agent::store::{Environment, TracePayload};
mod common;
use common::binary;

// ---------------------------------------------------------------------------
// 1. 装配与独立性
// ---------------------------------------------------------------------------

#[test]
fn two_apps_assemble_through_the_same_contract_and_notes_needs_nobody_else() {
    let temp = TempDir::new("notes-assemble");
    let env_root = temp.env(&agent_a());
    let materials = temp.materials();

    // 两个 App 用**同一份**契约装配：OS 只知道标识、说明、声明能力与执行入口。
    let view = FileView::with_history(&materials, &env_root).expect("只读文件视图");
    let notes = NotesApp::open(&env_root).expect("Notes App");
    let os = MinimalOs::assemble(vec![Box::new(view), Box::new(notes)]).expect("两个 App 可以共存");
    let ids: Vec<String> = os.app_ids().iter().map(|id| (*id).to_string()).collect();
    assert_eq!(ids, vec!["files".to_string(), NOTES_APP_ID.to_string()]);

    // **拿掉文件 App 之后，Notes 照样工作**：这是"独立性"里最硬的一条——它不依赖谁在旁边。
    let mut only_notes = MinimalOs::assemble(vec![Box::new(
        NotesApp::open(&env_root).expect("Notes App"),
    )])
    .expect("只有 Notes 也能装配");
    let mut expansion = RunExpansion::new(agent_a(), RunId::new("run-1"));
    let mut scope = only_notes.scope(&mut expansion);
    // 先展开它自己，再创建；两步都只经过窄契约。
    assert_eq!(
        scope.admit(&agent_a(), &RunId::new("run-1"), &expand(NOTES_APP_ID)),
        ActionAdmission::Admitted
    );
    scope.execute(&agent_a(), &op("run-1"), &expand(NOTES_APP_ID));
    // **先看格式**（2026-10-10：没有任何动作开箱免查）⇒ 之后它才可调。
    assert_eq!(
        scope.admit(&agent_a(), &RunId::new("run-1"), &inspect("notes_create")),
        ActionAdmission::Admitted
    );
    scope.execute(&agent_a(), &op("run-1"), &inspect("notes_create"));
    assert_eq!(
        scope.admit(
            &agent_a(),
            &RunId::new("run-1"),
            &create_note("独立", "没有文件 App")
        ),
        ActionAdmission::Admitted
    );
    assert_eq!(
        scope
            .execute(
                &agent_a(),
                &op("run-1"),
                &create_note("独立", "没有文件 App")
            )
            .state(),
        ResultState::Success
    );
    assert!(
        std::fs::read_dir(env_root.join("notes"))
            .expect("Notes 自己的存储")
            .count()
            >= 1
    );

    // 装配期仍然拒绝重复的行动标识：第二个 App 声明同一件事是不允许的。
    struct FakeNotes;
    impl celira_agent::app_contract::AppPort for FakeNotes {
        fn app_id(&self) -> &str {
            "fake-notes"
        }
        fn description(&self) -> &str {
            "假装也能建笔记"
        }
        fn declared_capabilities(&self) -> celira_agent::core::Capabilities {
            celira_agent::core::Capabilities::of(vec![celira_agent::core::Capability::new(
                ActionId::new("notes_create"),
                "重复声明的创建动作",
                vec![],
            )])
        }
        fn declared_sources(&self) -> Vec<celira_agent::app_contract::SourceDeclaration> {
            use celira_agent::app_contract::{SourceClass, SourceDeclaration};
            self.declared_capabilities()
                .items()
                .iter()
                .map(|capability| {
                    SourceDeclaration::new(capability.id().clone(), SourceClass::System)
                })
                .collect()
        }
    }
    impl ActionPort for FakeNotes {
        fn capabilities(&self, _agent: &AgentId, _run: &RunId) -> celira_agent::core::Capabilities {
            celira_agent::app_contract::AppPort::declared_capabilities(self)
        }
        fn admit(&self, _agent: &AgentId, _run: &RunId, _action: &Action) -> ActionAdmission {
            ActionAdmission::Admitted
        }
        fn execute(
            &mut self,
            _agent: &AgentId,
            _operation: &OperationId,
            _action: &Action,
        ) -> ActionOutcome {
            ActionOutcome::failure("不该被调用")
        }
    }
    let conflict = MinimalOs::assemble(vec![
        Box::new(NotesApp::open(&env_root).expect("Notes App")),
        Box::new(FakeNotes),
    ]);
    assert!(
        conflict.is_err(),
        "两个 App 声明同一个动作标识必须在装配期被拒绝"
    );
}

// ---------------------------------------------------------------------------
// 2. 默认只读与授权隔离
// ---------------------------------------------------------------------------

#[test]
fn preinstalled_notes_are_open_from_the_first_turn_and_need_no_grant() {
    let temp = TempDir::new("notes-preinstalled");
    let env_root = temp.env(&agent_a());
    let mut os = notes_os(&env_root); // 预装常开，与生产装配一致
    let mut expansion2 = RunExpansion::new(agent_a(), RunId::new("run-1"));
    let mut scope = os.scope(&mut expansion2);

    // **首轮就在场**：不点任何东西，全套动词已经在能力面里。
    let ids = ids_of(&scope, &agent_a(), "run-1");
    for verb in [
        "notes_create",
        "notes_list",
        "notes_read",
        "notes_verify_note",
    ] {
        assert!(
            ids.contains(&verb.to_string()),
            "预装常开应当首轮即含 {verb}: {ids:?}"
        );
    }

    // 展开机制原样保留：对预装的 App 回一张门卡（已开、动作数、目录位置），
    // 不重读、不复制说明、不重复动词表。
    let expanded = scope.execute(&agent_a(), &op("run-1"), &expand(NOTES_APP_ID));
    assert_eq!(expanded.state(), ResultState::Success);
    assert_eq!(
        expanded.content()[0].label(),
        "already-expanded",
        "{:?}",
        expanded.content()
    );
    let door = expanded.content()[0].text();
    for expected in ["本次 Run 已经开着", "【notes】"] {
        assert!(door.contains(expected), "门卡缺 {expected}：{door}");
    }
    assert!(
        !door.contains("notes_create"),
        "重复展开不得回动词表：{door}"
    );

    // **免授权可写**：没有任何 flag、没有任何授权对象，创建照样接纳并成功。
    // ⚠ 2026-10-10：**先看一次格式**（没有任何动作开箱免查——常驻只能是主体自己 pin 的）。
    // 这一步与"授权"无关：它只是"先学再用"那道既有的门，被拒时随单附卡。
    scope.execute(&agent_a(), &op("run-1"), &inspect("notes_create"));
    assert_eq!(
        scope.admit(
            &agent_a(),
            &RunId::new("run-1"),
            &create_note("第一条", "x")
        ),
        ActionAdmission::Admitted
    );
    let created = scope.execute(&agent_a(), &op("run-1"), &create_note("第一条", "x"));
    assert_eq!(
        created.state(),
        ResultState::Success,
        "{:?}",
        created.content()
    );
    assert!(env_root.join("notes").exists(), "写过之后存储目录才出现");

    // **效果纪律的形状一根毛不动**：每一条写仍然是「可重复写 ＋ 配一个复核」（免授权只改
    // "谁点头"，不改类别、不改复核）。TASK_76 加的是**又一条**同形状的声明
    // （`notes_delete`），2026-10-13 又加了一条同形状的（`notes_append`）——都不是把哪一条的
    // 形状改松；2026-10-14 的 `notes_edit` 是**第一条不同形状的**：它与 `files_edit` 同类
    // （独占创建那一档：第一次消耗前置，第二次同参数必然明确失败），照样配复核。
    // ⚠ 这句原来还写着"不改未决门"——**未决效果门已退役**（门是指令，而世界只该给信息），
    // 声明这一侧一个字没松。
    let effects = app_effects(&env_root);
    assert_eq!(
        effects.len(),
        6,
        "六条写声明：notes_create / notes_pin / notes_delete / notes_unpin / notes_append / notes_edit"
    );
    assert_eq!(effects[0].action().as_str(), "notes_create");
    assert_eq!(
        effects[0].class(),
        celira_agent::app_contract::EffectClass::RepeatableWrite
    );
    assert_eq!(
        effects[0].verification().map(|id| id.as_str().to_string()),
        Some("notes_verify_note".to_string())
    );

    // 读仍然工作：现在有一条（写与读走同一条窄契约）。
    let listed = scope.execute(&agent_a(), &op("run-1"), &list_notes());
    assert_eq!(listed.state(), ResultState::Success);
    assert!(
        listed.content()[0]
            .text()
            .contains("notes = 1, truncated = false"),
        "{:?}",
        listed.content()
    );
}

#[test]
fn writing_her_own_notes_needs_no_grant_and_the_line_is_whose_ground_it_is() {
    let temp = TempDir::new("notes-boundary");
    let env_root = temp.env(&agent_a());

    // **免授权**：装配起来就带写能力——笔记写在自己的环境里，是主体自持的字。
    let app = NotesApp::open(&env_root).expect("Notes App");
    assert_eq!(
        app.admit(&agent_a(), &RunId::new("run-1"), &create_note("t", "x")),
        ActionAdmission::Admitted
    );
    // 这一层不再按"谁在问"判：本 App 的边界是**目录**（它的存储挨着环境记录）。
    // "谁在问"由 OS 的使用作用域把住——下面用作用域证一次：换个身份一个字节都写不进去。
    assert_eq!(
        app.admit(&agent_b(), &RunId::new("run-1"), &create_note("t", "x")),
        ActionAdmission::Admitted
    );
    let mut os = notes_os(&env_root);
    let mut expansion = RunExpansion::new(agent_a(), RunId::new("run-1"));
    let mut scope = os.scope(&mut expansion);
    let foreign = scope.execute(&agent_b(), &op("run-1"), &create_note("t", "x"));
    assert_eq!(foreign.state(), ResultState::Failure);
    assert_eq!(foreign.failure_class(), Some("identity-mismatch"));
    assert!(!env_root.join("notes").exists());

    // **分界线：写到谁的地盘。** 材料目录是**别人的地盘**，写它仍然要授权——
    // 没有 `--create-dir` 那一类授权时，文件 App 连 `files_create` 都不在能力面里。
    let directory_grant =
        celira_agent::creation_grant::CreationGrant::new(agent_a(), RunId::new("run-1"), "notes")
            .expect("目录授权");
    assert_eq!(directory_grant.directory(), "notes");
    let materials = temp.materials();
    let mut view_os = MinimalOs::assemble(vec![Box::new(
        FileView::with_history(&materials, &env_root).expect("只读文件视图"),
    )])
    .expect("装配文件视图");
    let mut view_expansion = RunExpansion::new(agent_a(), RunId::new("run-1"));
    let view_scope = view_os.scope(&mut view_expansion);
    let view_ids = ids_of(&view_scope, &agent_a(), "run-1");
    assert!(
        !view_ids.contains(&"files_create".to_string()),
        "没有授权时材料目录连创建能力都不该出现: {view_ids:?}"
    );
    assert!(
        view_ids.contains(&"open_app".to_string()),
        "文件视图照旧要点开才展开: {view_ids:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. 创建语义与身份
// ---------------------------------------------------------------------------

#[test]
fn create_note_writes_exactly_the_text_and_never_overwrites() {
    let temp = TempDir::new("notes-semantics");
    let env_root = temp.env(&agent_a());
    let mut os = notes_os(&env_root);
    let mut expansion3 = RunExpansion::new(agent_a(), RunId::new("run-1"));
    let mut scope = os.scope(&mut expansion3);
    scope.execute(&agent_a(), &op("run-1"), &expand(NOTES_APP_ID));

    let title = "第一次清醒期记下的东西";
    let text = "第一行：中文与标点\n第二行";
    let created = scope.execute(&agent_a(), &op("run-1"), &create_note(title, text));
    assert_eq!(created.state(), ResultState::Success, "{created:?}");
    assert!(
        created.content()[0].text().contains("note-1"),
        "身份由 Notes App 生成: {:?}",
        created.content()
    );

    // 读回：标题与正文逐字一致。
    let read = scope.execute(&agent_a(), &op("run-1"), &read_note("note-1"));
    assert_eq!(read.state(), ResultState::Success);
    assert_eq!(read.content()[0].text(), title);
    assert_eq!(read.content()[1].text(), text);

    // 空正文合法。
    let empty = scope.execute(&agent_a(), &op("run-1"), &create_note("空的", ""));
    assert_eq!(empty.state(), ResultState::Success);
    let read_empty = scope.execute(&agent_a(), &op("run-1"), &read_note("note-2"));
    assert_eq!(read_empty.content()[1].text(), "");

    // **同名不覆盖**：标题不是身份，两条同名笔记各自存在、各自可读。
    let twin = scope.execute(&agent_a(), &op("run-1"), &create_note(title, "另一条正文"));
    assert_eq!(twin.state(), ResultState::Success);
    assert!(
        twin.content()[0].text().contains("note-3"),
        "{:?}",
        twin.content()
    );
    let first_again = scope.execute(&agent_a(), &op("run-1"), &read_note("note-1"));
    assert_eq!(first_again.content()[1].text(), text, "原笔记没有被覆盖");

    // 非法标题/正文：明确失败，而且**没有**留下任何文件。
    let before = std::fs::read_dir(env_root.join("notes"))
        .expect("存储")
        .count();
    for (bad_title, why) in [
        ("", "空标题"),
        ("带\n换行", "标题必须是单行"),
        (&"长".repeat(MAX_TITLE_BYTES), "标题超上限"),
    ] {
        let outcome = scope.execute(&agent_a(), &op("run-1"), &create_note(bad_title, "x"));
        assert_eq!(outcome.state(), ResultState::Failure, "{why}: {outcome:?}");
        assert_eq!(outcome.failure_class(), Some("invalid-parameter"), "{why}");
    }
    let too_large = scope.execute(
        &agent_a(),
        &op("run-1"),
        &create_note("太大", &"x".repeat(MAX_NOTE_BYTES + 1)),
    );
    assert_eq!(too_large.state(), ResultState::Failure);
    assert_eq!(too_large.failure_class(), Some("too-large"));
    assert_eq!(
        std::fs::read_dir(env_root.join("notes"))
            .expect("存储")
            .count(),
        before,
        "被拒绝的创建不得留下任何文件"
    );

    // 未知参数名也拒绝。
    let bogus = Action::with_params(
        ActionId::new("notes_create"),
        ActionParams::none().with("title", "t").with("body", "x"),
    );
    assert_eq!(
        scope
            .execute(&agent_a(), &op("run-1"), &bogus)
            .failure_class(),
        Some("invalid-parameter")
    );
}

#[test]
fn note_identity_is_generated_and_bounded() {
    let temp = TempDir::new("notes-identity");
    let env_root = temp.env(&agent_a());
    let mut os = notes_os(&env_root);
    let mut expansion4 = RunExpansion::new(agent_a(), RunId::new("run-1"));
    let mut scope = os.scope(&mut expansion4);
    scope.execute(&agent_a(), &op("run-1"), &expand(NOTES_APP_ID));

    // 身份是 `note-<n>`，逐条递增，而且**随文件稳定保存**。
    for _ in 0..3 {
        assert_eq!(
            scope
                .execute(&agent_a(), &op("run-1"), &create_note("t", "x"))
                .state(),
            ResultState::Success
        );
    }
    let listed = scope.execute(&agent_a(), &op("run-1"), &list_notes());
    let listed = listed.content()[0].text().to_string();
    assert!(listed.contains("notes = 3, truncated = false"), "{listed}");
    for id in ["note-1", "note-2", "note-3"] {
        assert!(listed.contains(id), "{listed}");
    }

    // 身份形状：只有 `note-<非零十进制>` 才算身份；别的形状连拼路径的机会都没有。
    for bad in [
        "../secret",
        "note-1/x",
        "note-",
        "note-01",
        "note-1.note",
        "",
        "NOTE-1",
    ] {
        let outcome = scope.execute(&agent_a(), &op("run-1"), &read_note(bad));
        assert_eq!(outcome.state(), ResultState::Failure, "{bad}");
        assert_eq!(outcome.failure_class(), Some("invalid-id"), "{bad}");
    }
    // 形状合法但不存在：明确 not-found，不冒充空笔记。
    let missing = scope.execute(&agent_a(), &op("run-1"), &read_note("note-99"));
    assert_eq!(missing.failure_class(), Some("not-found"));

    // 列表有界：超过上限时明确标记 truncated，不静默截断。
    for _ in 0..MAX_LISTED_NOTES {
        scope.execute(&agent_a(), &op("run-1"), &create_note("t", "x"));
    }
    let listed = scope
        .execute(&agent_a(), &op("run-1"), &list_notes())
        .content()[0]
        .text()
        .to_string();
    assert!(listed.contains("truncated = true"), "{listed}");
    assert_eq!(
        listed.lines().filter(|line| line.contains('\t')).count(),
        MAX_LISTED_NOTES,
        "有界就是有界：返回条数恰好是上限"
    );
}

// ---------------------------------------------------------------------------
// 4. 数据隔离与事实分离
// ---------------------------------------------------------------------------

#[test]
fn notes_stay_out_of_the_file_view_and_out_of_agent_b() {
    let temp = TempDir::new("notes-isolation");
    let env_root = temp.env(&agent_a());
    let materials = temp.materials();
    let secret = "只属于 A 的笔记正文";

    let mut os = MinimalOs::assemble(vec![
        Box::new(FileView::with_history(&materials, &env_root).expect("只读文件视图")),
        Box::new(NotesApp::open(&env_root).expect("Notes App")),
    ])
    .expect("装配两个 App");
    let mut expansion5 = RunExpansion::new(agent_a(), RunId::new("run-1"));
    let mut scope = os.scope(&mut expansion5);
    scope.execute(&agent_a(), &op("run-1"), &expand(NOTES_APP_ID));
    scope.execute(&agent_a(), &op("run-1"), &create_note("A 的笔记", secret));

    // 文件 App 看不到 Notes 的存储：资料根是另一个目录，Notes 存在环境根之内。
    scope.execute(&agent_a(), &op("run-1"), &expand("files"));
    let listed = scope.execute(&agent_a(), &op("run-1"), &list("")).content()[0]
        .text()
        .to_string();
    assert!(
        !listed.contains("notes"),
        "Notes 的存储不得出现在资料根列表里: {listed}"
    );
    let probe = scope.execute(&agent_a(), &op("run-1"), &read_text("notes/note-1.note"));
    assert_eq!(
        probe.state(),
        ResultState::Failure,
        "文件 App 读不到 Notes 存储"
    );

    // Agent B 有自己的环境，因此有自己的 Notes：读不到 A 的任何东西。
    let other_root = temp.other_env(&agent_b());
    let mut other_os = notes_os(&other_root);
    let mut expansion6 = RunExpansion::new(agent_b(), RunId::new("run-b"));
    let mut other_scope = other_os.scope(&mut expansion6);
    other_scope.execute(&agent_b(), &op("run-b"), &expand(NOTES_APP_ID));
    let listed = other_scope
        .execute(&agent_b(), &op("run-b"), &list_notes())
        .content()[0]
        .text()
        .to_string();
    assert!(
        listed.contains("notes = 0"),
        "B 的环境里没有 A 的笔记: {listed}"
    );
    assert_eq!(
        other_scope
            .execute(&agent_b(), &op("run-b"), &read_note("note-1"))
            .failure_class(),
        Some("not-found"),
        "B 读不到 A 的笔记身份"
    );

    // 事实侧：正文不进 Foundation 事实（用真实读写路径跑一次完整活动来证明）。
    let mut model = ScriptedModel::new(vec![
        ModelOutput::act(expand(NOTES_APP_ID)),
        ModelOutput::act(create_note("给下一次的", secret)),
        ModelOutput::finish(SubjectEnding::Completed),
    ]);
    let mut agent = Agent::new(agent_a());
    let mut ledger = celira_agent::foundation::Ledger::new(agent_a());
    let mut os = notes_os(&env_root);
    let mut expansion7 = RunExpansion::new(agent_a(), RunId::new("run-2"));
    let mut scope = os.scope(&mut expansion7);
    run_activity_with(
        &mut agent,
        RunRequest {
            cost: &no_cost,
            run: RunId::new("run-2"),
            limits: Limits::new(4),
            input: RunInput::empty(),
            source: &mut NoContext,
            commit: &mut celira_agent::core::NoCommit,
        },
        &mut model,
        &mut scope,
        &mut ledger,
    )
    .expect("活动应当收场");
    let facts = format!("{:?}", ledger.records());
    assert!(
        !facts.contains(secret),
        "笔记正文不得进入 Foundation 事实: {facts}"
    );
    assert!(!facts.contains("给下一次的"), "{facts}");
    // 事实里只留下动作身份与结果类别。
    assert!(facts.contains("ActionPrepared"), "{facts}");
    assert!(facts.contains("ActionResultRecorded"), "{facts}");
}

// ---------------------------------------------------------------------------
// 5. 跨 Run：独立进程主动读回
// ---------------------------------------------------------------------------

#[test]
fn a_note_written_in_one_run_is_read_back_by_a_later_independent_process() {
    let temp = TempDir::new("notes-cross-run");
    let env_root = temp.env(&agent_a());
    let materials = temp.materials();
    let title = "留给下一次的标题";
    let text = "留给下一次的正文（中文）";

    // Run A：宿主**什么都不声明**（笔记本来就不需要授权），Agent 自己选择创建一条笔记。
    // `open_app` 保留在这条脚本里：预装常开之后它仍然成立，回一张已开门的门卡。
    // ⚠ 2026-10-10：`notes_create` **不再开箱免查**（出厂精选已删——常驻只能是主体自己 pin 的）
    // ⇒ 先 `inspect_action` 看一次，再调。**门语义一个字没动**（TASK_69），只是它现在对**每一条**都成立。
    let a = Fixture::new(&env_root, "agent-alpha", &materials, "run-a")
        .script(&format!(
            "act:open_app?target={NOTES_APP_ID};act:inspect_action?action=notes_create;\
             act:notes_create?title={title}&text={text};finish"
        ))
        .run();
    assert_eq!(code(&a), 0, "{}", stderr(&a));
    let report = stdout(&a);
    assert!(
        report.contains("FIXTURE:saw-result=notes_create:success:none"),
        "{report}"
    );

    // Run B：**新进程、零 flag**：读回 A 的字，并且**再写一条**（免授权——主体的字一直在主体手上）。
    //
    // ⚠ 2026-10-10：**没有任何动作开箱免查**（出厂精选已删——常驻只能是主体自己 pin 的）
    // ⇒ 这一幕**三条**动词各先 `inspect_action` 看一次，再调。这不是给绿灯放宽校验，
    // 而是**照新纪律走**（TASK_69 的门语义一个字没动）；也正是那笔**价钱**：每 Run 头几轮多查几次。
    let b = Fixture::new(&env_root, "agent-alpha", &materials, "run-b")
        .script(
            "act:inspect_action?action=notes_list;act:notes_list;\
             act:inspect_action?action=notes_read;act:notes_read?id=note-1;\
             act:inspect_action?action=notes_create;\
             act:notes_create?title=第二天的标题&text=第二天写的;finish",
        )
        .max_turns(9)
        .echo("notes_read")
        .run();
    assert_eq!(code(&b), 0, "{}", stderr(&b));
    let report = stdout(&b);
    assert!(
        report.contains("FIXTURE:saw-result=notes_list:success:none"),
        "{report}"
    );
    assert!(
        report.contains("FIXTURE:saw-result=notes_create:success:none"),
        "新进程里没有 owner 的授权，主体也写得动自己的笔记: {report}"
    );
    assert!(
        report.contains("FIXTURE:saw-result=notes_read:success:none"),
        "{report}"
    );
    // 读回的**原文**：B 真的看见了 A 写下的标题与正文。
    let echo = fixture_field(&report, "echo");
    assert!(echo.contains(title), "读回内容必须包含 A 的标题: {echo}");
    assert!(echo.contains(text), "读回内容必须包含 A 的正文: {echo}");

    // 跨进程、跨 Run 的字**都在主体手上**（这就是"免授权"的意思）：B 这次活动里有自己的
    // 六次准备（看格式 ×3、列、读、写），一条都没少。
    let diagnosed = diagnose(&env_root, "agent-alpha", "run-b");
    assert!(
        diagnosed.contains("activity run=run-b") && diagnosed.contains("prepared=6"),
        "B 应当准备好「看格式 ×3 + 列、读、写」六次行动: {diagnosed}"
    );
    assert!(
        !diagnosed.contains("run=run-a"),
        "单活动诊断只回答被问的那一次: {diagnosed}"
    );
}

// ---------------------------------------------------------------------------
// 6. 失败语义：结果事实保存失败时不重做
// ---------------------------------------------------------------------------

#[test]
fn a_note_whose_result_cannot_be_saved_is_not_created_again() {
    let temp = TempDir::new("notes-result-save-fail");
    let env_root = temp.env(&agent_a());
    let mut env = Environment::open(&env_root, agent_a()).expect("打开环境");
    let text = "结果事实没写下去，但笔记已经在盘上";

    let mut model = ScriptedModel::new(vec![
        ModelOutput::act(expand(NOTES_APP_ID)),
        ModelOutput::act(create_note("t", text)),
        ModelOutput::finish(SubjectEnding::Completed),
    ]);
    // ⚠ 2026-10-10：`notes_create` 不再开箱免查 ⇒ 这一场先把它**钉住**
    // （常驻只能是主体自己 pin 的；写一份 pin 文件就是"她自己钉了它"）。
    // 这样这一场仍然只有"展开 → 创建"两次行动，`FailAtSecondResult` 照样落在**创建的结果**上。
    pin_actions(&env_root, &["notes_create"]);
    let mut agent = Agent::new(agent_a());
    let mut ledger = celira_agent::foundation::Ledger::new(agent_a());
    let result = {
        let mut commit = FailAtSecondResult::new(EnvironmentCommit::new(&mut env));
        let mut os = notes_os(&env_root);
        let mut expansion8 = RunExpansion::new(agent_a(), RunId::new("run-1"));
        let mut scope = os.scope(&mut expansion8);
        run_activity_with(
            &mut agent,
            RunRequest {
                cost: &no_cost,
                run: RunId::new("run-1"),
                limits: Limits::new(4),
                input: RunInput::empty(),
                source: &mut NoContext,
                commit: &mut commit,
            },
            &mut model,
            &mut scope,
            &mut ledger,
        )
    };
    let Err(RunError::Commit(error)) = result else {
        panic!("应当在结果事实上失败: {result:?}");
    };
    assert_eq!(error.stage(), CommitStage::ActionResult);

    // 效果发生了：笔记就在盘上。所以不能说"没有发生"，也不重新执行。
    let stored = std::fs::read_to_string(env_root.join("notes/note-1.note")).expect("笔记应当存在");
    assert!(stored.contains(text), "{stored}");
    assert_eq!(model.calls, 2, "保存失败不得触发重新执行");
    assert_eq!(
        std::fs::read_dir(env_root.join("notes"))
            .expect("存储")
            .count(),
        1,
        "只应有那一条笔记：不补写、不重试"
    );

    // 既有诊断把它读成"发出过、没有结果"——文件效果已经发生这件事不会被瞒掉。
    drop(env);
    let diagnosed = diagnose(&env_root, "agent-alpha", "run-1");
    assert!(
        diagnosed.contains("unconfirmed=1") && diagnosed.contains("dispatched-without-result=1"),
        "诊断必须如实报告未确认的效果: {diagnosed}"
    );
    assert!(
        !diagnosed.contains(text),
        "诊断不得把笔记正文当成事实读出来: {diagnosed}"
    );
}

// ---------------------------------------------------------------------------
// 夹具与工具
// ---------------------------------------------------------------------------

fn agent_a() -> AgentId {
    AgentId::new("agent-alpha")
}

fn agent_b() -> AgentId {
    AgentId::new("agent-beta")
}

/// 只装 Notes 的机器，**预装常开**（与生产装配一致）：能力从第一轮起就在场，
/// 不需要 `open_app`、也不需要任何授权。
fn notes_os(env_root: &Path) -> MinimalOs {
    let app = NotesApp::open(env_root).expect("Notes App");
    // ⚠ 2026-10-10：**带上环境根**——常驻只能是主体自己 pin 的，而 pin 文件住在环境根里
    // （`<环境根>/pinned_actions`）。不带根 ⇒ 读到空集 ⇒ 每一条都要先看格式。
    MinimalOs::assemble_preinstalled(vec![Box::new(app)], &[NOTES_APP_ID])
        .expect("装配 Notes")
        .with_environment_root(env_root)
}

/// 主体这台 Notes 自己声明出来的效果（免授权只改"谁点头"，不改类别与复核）。
fn app_effects(env_root: &Path) -> Vec<celira_agent::app_contract::EffectDeclaration> {
    use celira_agent::app_contract::AppPort;
    let app = NotesApp::open(env_root).expect("Notes App");
    app.declared_effects()
}

fn op(run: &str) -> OperationId {
    OperationId::new(RunId::new(run), Turn::FIRST)
}

fn ids_of(scope: &celira_agent::os::RunScope<'_>, agent: &AgentId, run: &str) -> Vec<String> {
    scope
        .capabilities(agent, &RunId::new(run))
        .items()
        .iter()
        .map(|capability| capability.id().as_str().to_string())
        .collect()
}

fn expand(target: &str) -> Action {
    Action::with_params(
        ActionId::new("open_app"),
        ActionParams::none().with("target", target),
    )
}

fn create_note(title: &str, text: &str) -> Action {
    Action::with_params(
        ActionId::new("notes_create"),
        ActionParams::none().with("title", title).with("text", text),
    )
}

/// **先看格式**（`inspect_action`）：2026-10-10 之后**没有任何动作开箱免查**——
/// 常驻只能是主体自己 pin 的（出厂默认集已删），所以"要调它就先看一次"是**每一条**的常态。
///
/// ⚠ 用例里必须走这一步：它**不是**为了绕过门，而是把新的常态如实摆出来
/// （从前这几条靠"出厂精选"免查，那一格已经不存在了）。
fn inspect(action: &str) -> Action {
    Action::with_params(
        ActionId::new("inspect_action"),
        ActionParams::none().with("action", action),
    )
}

/// **她自己钉住几个动作**：写一份 `<环境根>/pinned_actions`（一行一个标识）。
///
/// 这是**唯一**让动作免查的路（2026-10-10 之后常驻只剩这一个来源），因此用例要用它时
/// 就是在**扮演主体自己的那一笔**——不是绕过门，是走门后面那条正路。
fn pin_actions(env_root: &std::path::Path, ids: &[&str]) {
    let text: String = ids.iter().map(|id| format!("{id}\n")).collect();
    std::fs::write(env_root.join("pinned_actions"), text).expect("写 pin 文件");
}

/// 贴/摘/复核贴纸的三个动作（与 `notes_read` 同形：一个身份参数）。
fn pin(id: &str) -> Action {
    Action::with_params(
        ActionId::new("notes_pin"),
        ActionParams::none().with("id", id),
    )
}

fn unpin(id: &str) -> Action {
    Action::with_params(
        ActionId::new("notes_unpin"),
        ActionParams::none().with("id", id),
    )
}

fn verify_pinned(id: &str) -> Action {
    Action::with_params(
        ActionId::new("notes_verify_pinned"),
        ActionParams::none().with("id", id),
    )
}

fn list_notes() -> Action {
    Action::new(ActionId::new("notes_list"))
}

fn read_note(id: &str) -> Action {
    Action::with_params(
        ActionId::new("notes_read"),
        ActionParams::none().with("id", id),
    )
}

fn list(path: &str) -> Action {
    Action::with_params(
        ActionId::new("files_list"),
        ActionParams::none().with("path", path),
    )
}

fn read_text(path: &str) -> Action {
    Action::with_params(
        ActionId::new("files_read"),
        ActionParams::none().with("path", path),
    )
}

/// 让 `NotesFailure` 的稳定标记也被实际用到（它是这个 App 自己的失败词汇）。
#[test]
fn the_notes_app_failure_vocabulary_is_stable() {
    assert_eq!(NotesFailure::InvalidParameter.as_str(), "invalid-parameter");
    assert_eq!(NotesFailure::InvalidId.as_str(), "invalid-id");
    assert_eq!(NotesFailure::IdTaken.as_str(), "note-id-taken");
    assert_eq!(NotesFailure::NotFound.as_str(), "not-found");
    assert_eq!(NotesFailure::Unreadable.as_str(), "unreadable");
    assert_eq!(NotesFailure::TooLarge.as_str(), "too-large");
    assert_eq!(NotesFailure::Io.as_str(), "io");
}

/// 只在**第二次**结果提交上失败的接缝（第一次是展开）。
struct FailAtSecondResult<'a> {
    inner: EnvironmentCommit<'a>,
    results_committed: usize,
}

impl<'a> FailAtSecondResult<'a> {
    fn new(inner: EnvironmentCommit<'a>) -> Self {
        Self {
            inner,
            results_committed: 0,
        }
    }
}

impl celira_agent::core::CommitSeam for FailAtSecondResult<'_> {
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
        entry: &celira_agent::core::TraceEntry,
    ) -> Result<(), CommitError> {
        self.inner.commit_trace(run, turn, entry)
    }
    fn commit_action_selected(
        &mut self,
        run: &RunId,
        operation: &OperationId,
        action: &Action,
    ) -> Result<(), CommitError> {
        self.inner.commit_action_selected(run, operation, action)
    }
    fn commit_action_dispatched(
        &mut self,
        run: &RunId,
        operation: &OperationId,
    ) -> Result<(), CommitError> {
        self.inner.commit_action_dispatched(run, operation)
    }
    fn commit_action_result(
        &mut self,
        run: &RunId,
        operation: &OperationId,
        outcome: &ActionOutcome,
    ) -> Result<(), CommitError> {
        if self.results_committed == 1 {
            return Err(CommitError::at(CommitStage::ActionResult));
        }
        self.results_committed += 1;
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
    fn new(root: &Path, agent: &str, materials: &Path, run: &str) -> Self {
        Self {
            variables: vec![
                ("MANUAL_FIXTURE_ROOT", path_text(root)),
                ("MANUAL_FIXTURE_AGENT", agent.to_string()),
                ("MANUAL_FIXTURE_MATERIALS", path_text(materials)),
                ("MANUAL_FIXTURE_RUN", run.to_string()),
                ("MANUAL_FIXTURE_MAX_TURNS", "6".to_string()),
                ("MANUAL_FIXTURE_MODEL", "script".to_string()),
            ],
        }
    }

    fn script(mut self, script: &str) -> Self {
        self.set("MANUAL_FIXTURE_SCRIPT", script.to_string());
        self
    }

    /// 这一次活动的**轮次上限**（缺省 6）：脚本长到接近上限时显式给一个数，
    /// 免得"预算用完"被误读成"主体做不成这件事"。
    fn max_turns(mut self, turns: u32) -> Self {
        self.set("MANUAL_FIXTURE_MAX_TURNS", turns.to_string());
        self
    }

    fn echo(mut self, action: &str) -> Self {
        self.set("MANUAL_FIXTURE_ECHO", action.to_string());
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
        command.env_remove("DEEPSEEK_API_KEY");
        command.output().expect("启动夹具")
    }
}

fn diagnose(env_root: &Path, agent: &str, run: &str) -> String {
    let output = Command::new(binary("environment_diagnose"))
        .args([
            path_text(env_root),
            agent.to_string(),
            "--run".to_string(),
            run.to_string(),
        ])
        .output()
        .expect("启动诊断命令");
    if output.status.code() != Some(0) {
        return String::new();
    }
    String::from_utf8_lossy(&output.stdout).into_owned()
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
        path.push(format!("celira-task06-{label}-{}-{n}", std::process::id()));
        if path.exists() {
            std::fs::remove_dir_all(&path).expect("清掉同名残留目录");
        }
        std::fs::create_dir_all(&path).expect("建立测试目录");
        Self { path }
    }

    /// 一个**真实存在**的身份环境目录（本包自建的隔离夹具）。
    fn env(&self, agent: &AgentId) -> PathBuf {
        let env_root = self.path.join("env");
        if !env_root.exists() {
            drop(Environment::create(&env_root, agent.clone()).expect("建环境"));
        }
        env_root
    }

    /// 另一个 Agent 的环境：它有自己的 Notes，读不到别人的。
    fn other_env(&self, agent: &AgentId) -> PathBuf {
        let env_root = self.path.join("env-other");
        if !env_root.exists() {
            drop(Environment::create(&env_root, agent.clone()).expect("建环境"));
        }
        env_root
    }

    /// 资料根（与环境根分开：Notes 的存储不该出现在它下面）。
    fn materials(&self) -> PathBuf {
        let materials = self.path.join("materials");
        std::fs::create_dir_all(&materials).expect("建资料目录");
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
// 6. 贴纸条 / 摘纸条（TASK_33）：只动标记、幂等、共用复核
// ---------------------------------------------------------------------------

#[test]
fn pinning_moves_only_the_marker_and_is_idempotent() {
    let temp = TempDir::new("notes-pin");
    let env_root = temp.env(&agent_a());
    let mut os = notes_os(&env_root);
    let mut expansion = RunExpansion::new(agent_a(), RunId::new("run-1"));
    let mut scope = os.scope(&mut expansion);

    // 创建与贴都**免授权**（宿主要什么 flag 都没给）。
    let created = scope.execute(&agent_a(), &op("run-1"), &create_note("词：青苔", "青苔"));
    assert_eq!(created.state(), ResultState::Success);
    let path = env_root.join("notes/note-1.note");
    let before = std::fs::read_to_string(&path).expect("笔记文件");
    assert!(before.contains("\"pinned\":false"), "{before}");

    let pinned = scope.execute(&agent_a(), &op("run-1"), &pin("note-1"));
    assert_eq!(
        pinned.state(),
        ResultState::Success,
        "{:?}",
        pinned.content()
    );
    assert_eq!(pinned.content()[0].label(), "pinned");
    let after = std::fs::read_to_string(&path).expect("笔记文件");
    assert!(after.contains("\"pinned\":true"), "{after}");
    // **内容一字不动**：只多了标记。
    assert!(after.contains("\"title\":\"词：青苔\""), "{after}");
    assert!(after.contains("\"text\":\"青苔\""), "{after}");

    // 幂等：再贴一次 = 仍然贴着（成功，而且是 already-pinned，不重写文件）。
    let again = scope.execute(&agent_a(), &op("run-1"), &pin("note-1"));
    assert_eq!(again.state(), ResultState::Success);
    assert_eq!(again.content()[0].label(), "already-pinned");
    assert_eq!(std::fs::read_to_string(&path).expect("笔记文件"), after);

    // 摘：同样只动标记；再摘一次 = 仍然没贴。
    let unpinned = scope.execute(&agent_a(), &op("run-1"), &unpin("note-1"));
    assert_eq!(unpinned.state(), ResultState::Success);
    assert_eq!(unpinned.content()[0].label(), "unpinned");
    assert!(
        std::fs::read_to_string(&path)
            .expect("笔记文件")
            .contains("\"pinned\":false")
    );
    let again = scope.execute(&agent_a(), &op("run-1"), &unpin("note-1"));
    assert_eq!(again.content()[0].label(), "already-unpinned");
}

#[test]
fn pin_on_an_unknown_note_fails_clearly_without_any_grant_or_side_effect() {
    let temp = TempDir::new("notes-pin-unknown");
    let env_root = temp.env(&agent_a());
    let mut os = notes_os(&env_root);
    let mut expansion = RunExpansion::new(agent_a(), RunId::new("run-1"));
    let mut scope = os.scope(&mut expansion);

    // 首轮能力面里就有这一对（预装常开 + 自持物写免授权）。
    let ids = ids_of(&scope, &agent_a(), "run-1");
    for verb in ["notes_pin", "notes_unpin", "notes_verify_pinned"] {
        assert!(
            ids.contains(&verb.to_string()),
            "首轮应当含 {verb}: {ids:?}"
        );
    }

    let missing = scope.execute(&agent_a(), &op("run-1"), &pin("note-9"));
    assert_eq!(missing.state(), ResultState::Failure);
    assert_eq!(missing.failure_class(), Some("not-found"));
    assert!(
        !env_root.join("notes").exists(),
        "只读的活动连存储目录都不该建出来"
    );

    // 非法身份连路径都拼不出来 ⇒ invalid-id（不是 not-found）。
    let bad = scope.execute(&agent_a(), &op("run-1"), &pin("note-1/../x"));
    assert_eq!(bad.failure_class(), Some("invalid-id"));
}

#[test]
fn the_shared_pin_verification_reports_the_current_state_not_the_history() {
    let temp = TempDir::new("notes-pin-verify");
    let env_root = temp.env(&agent_a());
    let mut os = notes_os(&env_root);
    let mut expansion = RunExpansion::new(agent_a(), RunId::new("run-1"));
    let mut scope = os.scope(&mut expansion);
    scope.execute(&agent_a(), &op("run-1"), &create_note("词：青苔", "青苔"));
    scope.execute(&agent_a(), &op("run-1"), &pin("note-1"));

    let verified = scope.execute(&agent_a(), &op("run-1"), &verify_pinned("note-1"));
    assert_eq!(
        verified.state(),
        ResultState::Success,
        "{:?}",
        verified.content()
    );

    scope.execute(&agent_a(), &op("run-1"), &unpin("note-1"));
    let verified = scope.execute(&agent_a(), &op("run-1"), &verify_pinned("note-1"));
    assert_eq!(verified.state(), ResultState::Failure);
    assert_eq!(verified.failure_class(), Some("not-pinned"));

    // 笔记不在 ⇒ 那是 not-found，不是 not-pinned（两件事不混）。
    let missing = scope.execute(&agent_a(), &op("run-1"), &verify_pinned("note-8"));
    assert_eq!(missing.failure_class(), Some("not-found"));
}

#[test]
fn pinned_material_reads_only_pinned_rows_in_identity_order_and_tolerates_old_files() {
    let temp = TempDir::new("notes-pinned-read");
    let env_root = temp.env(&agent_a());
    let mut os = notes_os(&env_root);
    let mut expansion = RunExpansion::new(agent_a(), RunId::new("run-1"));
    let mut scope = os.scope(&mut expansion);
    for (title, text) in [("一", "甲"), ("二", "乙"), ("三", "丙")] {
        scope.execute(&agent_a(), &op("run-1"), &create_note(title, text));
    }
    scope.execute(&agent_a(), &op("run-1"), &pin("note-1"));
    scope.execute(&agent_a(), &op("run-1"), &pin("note-3"));

    // 手写一条**旧格式**文件（TASK_33 之前写的，没有 `pinned` 键）：宽容解码 ⇒ 算没贴。
    std::fs::write(
        env_root.join("notes/note-4.note"),
        "{\"title\":\"旧\",\"text\":\"旧格式\"}\n",
    )
    .expect("写旧格式文件");

    let rows = celira_agent::notes_app::pinned_material(&env_root).expect("读得回来");
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0].id, "note-1");
    assert_eq!(rows[1].id, "note-3");
    assert_eq!(rows[0].text, "甲");
    // 读面**只读**：旧格式那条还在，一个字节没被改写。
    assert!(
        std::fs::read_to_string(env_root.join("notes/note-4.note"))
            .expect("旧文件还在")
            .contains("旧格式")
    );
}

// ---------------------------------------------------------------------------
// TASK_76（B）：notes_delete——删得掉，而且删错没有后悔药
// ---------------------------------------------------------------------------

/// 一个"只吃 `id` 一个参数"的动作（`notes_read` / `notes_delete` / `notes_verify_deleted` 同款）。
fn by_id(id: &str, param: &str) -> Action {
    Action::with_params(ActionId::new(id), ActionParams::none().with("id", param))
}

/// `notes_append` / `notes_verify_appended`（两个动词**同参数**：`id` ＋ `text`）。
fn with_text(id: &str, verb: &str, text: &str) -> Action {
    Action::with_params(
        ActionId::new(verb),
        ActionParams::none().with("id", id).with("text", text),
    )
}

/// **往一条笔记后面接一段**（2026-10-13，§3.79）：它**只往后接**——逐字节、**不替你加分隔符**、
/// 标题与贴纸标记一字不动；四条拒绝都在动手之前（空增量 / 超限 / 未知身份 / 参数不齐）；
/// 复核与它**同参数**，只认"结尾"。
#[test]
fn appending_keeps_every_existing_byte_and_moves_nothing_else() {
    let temp = TempDir::new("notes-append");
    let env_root = temp.env(&agent_a());
    let agent = agent_a();
    let run = "run-append";
    let mut os = notes_os(&env_root);
    let mut expansion = RunExpansion::new(agent.clone(), RunId::new(run));
    let mut scope = os.scope(&mut expansion);

    let id = create_one(&mut scope, &env_root, run, &agent);
    // 先贴上：追加**不许**动这个标记。
    let pinned_before = scope.execute(&agent, &op(run), &pin(&id));
    assert_eq!(pinned_before.state(), ResultState::Success);

    // 复核在追加之前就说"没有"（不是"读不出来"）。
    let not_yet = scope.execute(
        &agent,
        &op(run),
        &with_text(&id, "notes_verify_appended", "、鸡蛋"),
    );
    assert_eq!(not_yet.failure_class(), Some("not-found"));

    // 追加：接在**后面**，一个分隔符都不加（旧正文没有换行 ⇒ 结果就是"西红柿、鸡蛋"）。
    let appended = scope.execute(&agent, &op(run), &with_text(&id, "notes_append", "、鸡蛋"));
    assert_eq!(
        appended.state(),
        ResultState::Success,
        "{:?}",
        appended.content()
    );
    let read_back = scope.execute(&agent, &op(run), &read_note(&id));
    let blocks = read_back.content();
    assert_eq!(blocks[0].label(), "title");
    assert_eq!(blocks[0].text(), "买菜", "标题一字不动");
    assert_eq!(
        blocks[1].text(),
        "西红柿、鸡蛋",
        "逐字节接在后面（**不替你加分隔符**）"
    );

    // 贴纸标记也一字不动。
    let pinned_after = scope.execute(&agent, &op(run), &verify_pinned(&id));
    assert_eq!(
        pinned_after.state(),
        ResultState::Success,
        "追加不动 pinned: {pinned_after:?}"
    );

    // 复核现在说"是"，而且**同参数**：换一段就说不符合。
    let matched = scope.execute(
        &agent,
        &op(run),
        &with_text(&id, "notes_verify_appended", "、鸡蛋"),
    );
    assert_eq!(matched.state(), ResultState::Success);
    let mismatched = scope.execute(
        &agent,
        &op(run),
        &with_text(&id, "notes_verify_appended", "、牛奶"),
    );
    assert_eq!(mismatched.failure_class(), Some("not-found"));

    // **空增量拒绝**：那是一次什么都没写的动作，不许给成功回执。
    let empty = scope.execute(&agent, &op(run), &with_text(&id, "notes_append", ""));
    assert_eq!(empty.failure_class(), Some("invalid-parameter"));
    // 身份不存在 ⇒ 明确失败 `not-found`（不是"读不出来"）。
    let ghost = scope.execute(&agent, &op(run), &with_text("note-99", "notes_append", "x"));
    assert_eq!(ghost.failure_class(), Some("not-found"));

    // **上限只量正文**（2026-10-13 统一口径）：正文**贴着**上限的那一条，写得进去、**也读得回来**。
    // 从前读那一侧判的是**整份文件**（含 JSON 外壳）⇒ 那种笔记写得进去、读不回来（同一上限量了两件事）。
    let full = scope.execute(&agent, &op(run), &create_note("满", &"x".repeat(64 * 1024)));
    assert_eq!(full.state(), ResultState::Success);
    let full_id = {
        let mut ids: Vec<String> = std::fs::read_dir(env_root.join("notes"))
            .expect("读笔记目录")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter_map(|name| name.strip_suffix(".note").map(str::to_string))
            .collect();
        ids.sort();
        ids.pop().expect("至少落了一条")
    };
    let read_full = scope.execute(&agent, &op(run), &read_note(&full_id));
    assert_eq!(
        read_full.state(),
        ResultState::Success,
        "正文贴着上限的那一条读得回来: {read_full:?}"
    );
    assert_eq!(read_full.content()[1].text().len(), 64 * 1024);

    // **超限 ⇒ 拒绝，而且旧正文原样保留**（不截断、不写半截）。
    let over = scope.execute(&agent, &op(run), &with_text(&full_id, "notes_append", "y"));
    assert_eq!(over.failure_class(), Some("too-large"));
    let still_full = scope.execute(&agent, &op(run), &read_note(&full_id));
    assert_eq!(
        still_full.content()[1].text().len(),
        64 * 1024,
        "超限那次一个字节都没写进去"
    );

    // **正文超限的笔记照样列得出来**（列表只要身份与标题）：手写一条 70 KiB 正文的笔记，
    // 它在 `notes_list` 里不许**静默消失**（"读不出来"与"没有"是两件事），而读它时如实说 `too-large`。
    let big_body = format!(
        "{{\"title\":\"太大\",\"text\":\"{}\",\"pinned\":false}}\n",
        "z".repeat(70 * 1024)
    );
    std::fs::write(env_root.join("notes").join("note-99.note"), big_body)
        .expect("手写一条超限笔记");
    let listed = scope.execute(&agent, &op(run), &list_notes());
    assert_eq!(listed.state(), ResultState::Success);
    let listing = listed.content()[0].text().to_string();
    assert!(
        listing.contains("note-99"),
        "正文超限的笔记不许在列表里消失: {listing}"
    );
    let too_big = scope.execute(&agent, &op(run), &read_note("note-99"));
    assert_eq!(
        too_big.failure_class(),
        Some("too-large"),
        "读它时如实说太大（不是「读不出来」、也不是「没有」）"
    );
}

/// 新建一条笔记，回它**落盘后**的身份（从盘上取：那才是真的）。
fn create_one(
    scope: &mut celira_agent::os::RunScope<'_>,
    env_root: &Path,
    run: &str,
    agent: &AgentId,
) -> String {
    let created = scope.execute(
        agent,
        &op(run),
        &Action::with_params(
            ActionId::new("notes_create"),
            ActionParams::none()
                .with("title", "买菜")
                .with("text", "西红柿"),
        ),
    );
    assert_eq!(created.state(), ResultState::Success, "{created:?}");
    let mut ids: Vec<String> = std::fs::read_dir(env_root.join("notes"))
        .expect("读笔记目录")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter_map(|name| name.strip_suffix(".note").map(str::to_string))
        .collect();
    ids.sort();
    ids.pop().expect("至少落了一条")
}

/// **删得掉、查得出来、再删会被拒**：create → list 有 → delete（回执声明不可恢复）
/// → list 没有、read 拒 → 再删 ⇒ **明确拒绝**（不装成功）并把"拿 list 核对"带上。
#[test]
fn a_note_is_created_deleted_and_then_refused_on_both_read_and_delete() {
    let temp = TempDir::new("notes-delete");
    let env_root = temp.env(&agent_a());
    let agent = agent_a();
    let run = "run-delete";
    let mut os = notes_os(&env_root);
    let mut expansion = RunExpansion::new(agent.clone(), RunId::new(run));
    let mut scope = os.scope(&mut expansion);

    let id = create_one(&mut scope, &env_root, run, &agent);
    let listed = scope.execute(&agent, &op(run), &Action::new(ActionId::new("notes_list")));
    assert!(
        listed
            .content()
            .iter()
            .any(|block| block.text().contains(&id)),
        "刚建的要在 list 里: {listed:?}"
    );

    let deleted = scope.execute(&agent, &op(run), &by_id("notes_delete", &id));
    assert_eq!(deleted.state(), ResultState::Success, "{deleted:?}");
    let receipt: String = deleted
        .content()
        .iter()
        .map(|block| block.text().to_string())
        .collect();
    assert!(
        receipt.contains("已删除、不可恢复"),
        "回执要如实声明不可逆: {receipt}"
    );

    let listed = scope.execute(&agent, &op(run), &Action::new(ActionId::new("notes_list")));
    assert!(
        !listed
            .content()
            .iter()
            .any(|block| block.text().contains(&id)),
        "删掉的不能再出现在 list 里: {listed:?}"
    );
    let read = scope.execute(&agent, &op(run), &by_id("notes_read", &id));
    assert_eq!(read.failure_class(), Some("not-found"), "{read:?}");

    // 再删一次：**明确拒绝**（删除是破坏性的，`not-found` 不许装成功），
    // 而且随拒绝告诉主体去哪儿核对。
    let again = scope.execute(&agent, &op(run), &by_id("notes_delete", &id));
    assert_eq!(again.state(), ResultState::Failure, "{again:?}");
    assert_eq!(again.failure_class(), Some("not-found"));
    assert!(
        again
            .content()
            .iter()
            .any(|block| block.text().contains("notes_list")),
        "拒绝要指名核对的路: {again:?}"
    );
}

/// **删一条贴着的**：板上下一场就不再有它（无孤儿）；而且这一笔删除是**可复核的写**
/// ——复核与它配得上（同参数）⇒ 永远解得开，不是死锁。
#[test]
fn deleting_a_pinned_note_takes_it_off_the_board_and_the_verifier_pairing_holds() {
    let temp = TempDir::new("notes-delete-pinned");
    let env_root = temp.env(&agent_a());
    let agent = agent_a();
    let run = "run-delete-pinned";
    let mut os = notes_os(&env_root);
    let mut expansion = RunExpansion::new(agent.clone(), RunId::new(run));
    let mut scope = os.scope(&mut expansion);

    let id = create_one(&mut scope, &env_root, run, &agent);
    let pinned = scope.execute(&agent, &op(run), &by_id("notes_pin", &id));
    assert_eq!(pinned.state(), ResultState::Success, "{pinned:?}");
    let board = celira_agent::notes_app::pinned_material(&env_root).expect("读得回来");
    assert!(board.iter().any(|note| note.id == id), "先确认它确实在板上");

    let deleted = scope.execute(&agent, &op(run), &by_id("notes_delete", &id));
    assert_eq!(deleted.state(), ResultState::Success, "{deleted:?}");
    // **无孤儿**：纸条板是装配期一次性只读的那一份 ⇒ 下一场读回来就没有它了。
    let board = celira_agent::notes_app::pinned_material(&env_root).expect("读得回来");
    assert!(
        board.iter().all(|note| note.id != id),
        "删掉的不能还挂在板上: {board:?}"
    );

    // **写声明配了复核**（在事实层证明）：那一笔删除声明的复核**就是** `notes_verify_deleted`，
    // 而两者吃的参数**同一个名字**（`id`）。
    //
    // ⚠ **旧规矩 → 新规矩**（2026-09-26，两刀）：这里原来判的是「复核能力对得上 ＋ 参数完全相同
    // ⇒ 未决解得开」（门的注册与拦截在 Core 那一侧，由当时的 `pending_effect_gate.rs` 钉着）。
    // **未决效果门**随跨进程恢复退役，它读的那张**效果声明表**也在本刀退役（门是那张表唯一的
    // 生产读者），因此现在判的是**声明自己**：删除声明的复核就是它、类别是写。参数同名这件事
    // 由能力表本身保证（复核能力的参数表与它逐字相同），不再经门。
    let fresh = NotesApp::open(&env_root).expect("Notes App");
    let declared = celira_agent::app_contract::AppPort::declared_effects(&fresh);
    let deletion = declared
        .iter()
        .find(|effect| effect.action().as_str() == "notes_delete")
        .expect("notes_delete 必须声明效果");
    assert_eq!(
        deletion.verification().map(|id| id.as_str()),
        Some("notes_verify_deleted"),
        "删除的复核配得上"
    );
    assert_eq!(
        deletion.class(),
        celira_agent::app_contract::EffectClass::RepeatableWrite,
        "删除是**写**，且与贴/摘同一档"
    );
    // 复核照旧可用：删掉的那条 ⇒ 成功（这一次删除生效）；还在的那条 ⇒ 明确失败。
    let verified = scope.execute(&agent, &op(run), &by_id("notes_verify_deleted", &id));
    assert_eq!(verified.state(), ResultState::Success, "{verified:?}");
    let alive = create_one(&mut scope, &env_root, run, &agent);
    let still = scope.execute(&agent, &op(run), &by_id("notes_verify_deleted", &alive));
    assert_eq!(still.failure_class(), Some("still-present"), "{still:?}");
}

// ---------------------------------------------------------------------------
// 就地改（`notes_edit`，2026-10-14）：**整理那一侧**，与追加是两条路
//
// 判据来源：主体自己那份清单的第 3 条（「笔记只能新建 ＋ 往后接，不能就地改、不能压缩」）
// ＋ 主人在 2026-10-14 的裁决。它与 `notes_append` **同时在场**：
// 追加是"原始记录"那一侧（已有的字一个都不动），这一件是"我现在记着的东西"那一侧。
// ---------------------------------------------------------------------------

/// 就地改的动作（`id` ＋ `old_text` ＋ `new_text`）。
fn edit_note_action(action: &str, id: &str, old_text: &str, new_text: &str) -> Action {
    Action::with_params(
        ActionId::new(action),
        ActionParams::none()
            .with("id", id)
            .with("old_text", old_text)
            .with("new_text", new_text),
    )
}

/// 把一条笔记的正文读回来（`notes_read` 的第二块）。
fn body_of(
    scope: &mut celira_agent::os::RunScope<'_>,
    agent: &AgentId,
    run: &str,
    id: &str,
) -> String {
    let read = scope.execute(agent, &op(run), &read_note(id));
    assert_eq!(read.state(), ResultState::Success, "{read:?}");
    read.content()[1].text().to_string()
}

/// **恰好一处** ⇒ 换掉；标题与贴纸标记一字不动；同参数再来一次 ⇒ 明确失败（前置已被消耗）。
#[test]
fn editing_replaces_exactly_one_occurrence_and_moves_nothing_else() {
    let temp = TempDir::new("notes-edit-one");
    let env_root = temp.env(&agent_a());
    let agent = agent_a();
    let run = "run-edit-one";
    let mut os = notes_os(&env_root);
    let mut expansion = RunExpansion::new(agent.clone(), RunId::new(run));
    let mut scope = os.scope(&mut expansion);

    // 一条长正文的笔记：这一次要"整理"的正是这种东西。
    let id = {
        let created = scope.execute(
            &agent,
            &op(run),
            &create_note("买菜", "西红柿、鸡蛋（这一段是后来加长的细节，用来占地方）"),
        );
        assert_eq!(created.state(), ResultState::Success, "{created:?}");
        created
            .content()
            .first()
            .map(|block| {
                block
                    .text()
                    .split(' ')
                    .next()
                    .unwrap_or_default()
                    .to_string()
            })
            .unwrap_or_default()
    };
    let pinned = scope.execute(&agent, &op(run), &by_id("notes_pin", &id));
    assert_eq!(pinned.state(), ResultState::Success, "{pinned:?}");

    // 改之前复核：要被换掉的那段**还在** ⇒ `still-present`（不是"读不出来"）。
    let before = scope.execute(
        &agent,
        &op(run),
        &edit_note_action("notes_verify_edited", &id, "西红柿", "黄瓜"),
    );
    assert_eq!(before.failure_class(), Some("still-present"), "{before:?}");

    // 就地改：唯一一处 ⇒ 成功。**只动那一段**。
    let edited = scope.execute(
        &agent,
        &op(run),
        &edit_note_action("notes_edit", &id, "西红柿", "黄瓜"),
    );
    assert_eq!(edited.state(), ResultState::Success, "{edited:?}");
    let read_back = scope.execute(&agent, &op(run), &read_note(&id));
    assert_eq!(read_back.content()[0].text(), "买菜", "标题一字不动");
    assert_eq!(
        read_back.content()[1].text(),
        "黄瓜、鸡蛋（这一段是后来加长的细节，用来占地方）",
        "只换了点名的那一段"
    );
    let pinned_after = scope.execute(&agent, &op(run), &verify_pinned(&id));
    assert_eq!(
        pinned_after.state(),
        ResultState::Success,
        "贴纸标记一字不动"
    );

    // 复核现在是"成功"，而且**同参数**。
    let after = scope.execute(
        &agent,
        &op(run),
        &edit_note_action("notes_verify_edited", &id, "西红柿", "黄瓜"),
    );
    assert_eq!(after.state(), ResultState::Success, "{after:?}");

    // **同参数再来一次 ⇒ 明确失败**：前置（`old_text`）已经被换走了。这条钉住它声明的类别
    // （与 `files_edit` 同类：第一次消耗前置，第二次必然明确失败且无害）。
    let again = scope.execute(
        &agent,
        &op(run),
        &edit_note_action("notes_edit", &id, "西红柿", "黄瓜"),
    );
    assert_eq!(again.failure_class(), Some("not-found"), "{again:?}");

    // 空 `old_text` ⇒ 明确拒绝（空串会在每个位置匹配，那不是"换掉某一段"）；
    // 身份不存在 ⇒ `not-found`；复核一个从没出现过的对子 ⇒ **无法确认**（不许冒充成功）。
    let empty_old = scope.execute(
        &agent,
        &op(run),
        &edit_note_action("notes_edit", &id, "", "x"),
    );
    assert_eq!(empty_old.failure_class(), Some("invalid-parameter"));
    let ghost = scope.execute(
        &agent,
        &op(run),
        &edit_note_action("notes_edit", "note-99", "甲", "乙"),
    );
    assert_eq!(ghost.failure_class(), Some("not-found"));
    let never = scope.execute(
        &agent,
        &op(run),
        &edit_note_action("notes_verify_edited", &id, "从没出现过", "也没出现过"),
    );
    assert_eq!(never.state(), ResultState::Unknown, "{never:?}");
}

/// **0 处 ／ 不止一处 ⇒ 都明确失败，而且盘上一个字节都不动**（含糊的替换不做）。
#[test]
fn editing_refuses_zero_or_several_matches_without_writing_anything() {
    let temp = TempDir::new("notes-edit-ambiguous");
    let env_root = temp.env(&agent_a());
    let agent = agent_a();
    let run = "run-edit-ambiguous";
    let mut os = notes_os(&env_root);
    let mut expansion = RunExpansion::new(agent.clone(), RunId::new(run));
    let mut scope = os.scope(&mut expansion);

    let created = scope.execute(&agent, &op(run), &create_note("同一段", "甲甲"));
    assert_eq!(created.state(), ResultState::Success, "{created:?}");
    let id = created.content()[0]
        .text()
        .split(' ')
        .next()
        .unwrap_or_default()
        .to_string();

    // 0 处 ⇒ `not-found`（"没有被换掉的那一段"，不是"读不出来"）。
    let missing = scope.execute(
        &agent,
        &op(run),
        &edit_note_action("notes_edit", &id, "乙", "丙"),
    );
    assert_eq!(missing.failure_class(), Some("not-found"), "{missing:?}");

    // 两处 ⇒ `invalid-parameter`：**不猜"你想改哪一处"**。
    let ambiguous = scope.execute(
        &agent,
        &op(run),
        &edit_note_action("notes_edit", &id, "甲", "乙"),
    );
    assert_eq!(
        ambiguous.failure_class(),
        Some("invalid-parameter"),
        "{ambiguous:?}"
    );

    // 正文原样——两次拒绝都没写进任何东西。
    assert_eq!(body_of(&mut scope, &agent, run, &id), "甲甲");

    // **上限**：正文贴着 64 KiB 的那一条，把它**唯一**的那一段改长一个字节 ⇒ 整体拒绝、
    // 旧正文原样保留。（⚠ 这里不能用"一长串相同的字"来凑上限：那样会先撞上**唯一性**那一关
    // ——`invalid-parameter` 也对，但它证的不是上限这条。）
    let full_text = format!("{}{}", "x".repeat(64 * 1024 - 3), "尾");
    let full = scope.execute(&agent, &op(run), &create_note("满", &full_text));
    assert_eq!(full.state(), ResultState::Success, "{full:?}");
    let full_id = full.content()[0]
        .text()
        .split(' ')
        .next()
        .unwrap_or_default()
        .to_string();
    let over = scope.execute(
        &agent,
        &op(run),
        &edit_note_action("notes_edit", &full_id, "尾", "尾巴"),
    );
    assert_eq!(over.failure_class(), Some("too-large"), "{over:?}");
    assert_eq!(
        body_of(&mut scope, &agent, run, &full_id).len(),
        64 * 1024,
        "超限那一次一个字节都没写进去"
    );

    // 同一份参数里"不止一处"先于上限判：一长串相同的字 ⇒ `invalid-parameter`（不猜改哪一处）。
    let ambiguous_full = scope.execute(
        &agent,
        &op(run),
        &edit_note_action("notes_edit", &full_id, "x", "xx"),
    );
    assert_eq!(
        ambiguous_full.failure_class(),
        Some("invalid-parameter"),
        "{ambiguous_full:?}"
    );
}

/// **两条路都在场**：追加照旧（只往后接、已有的字一个都不动），就地改是另一件。
/// 还有一条事实：改完的正文里，**原来那段还在经历账本里**（那不是这一层能证明的，
/// 这里只钉"这一层同时给了两条路"）。
#[test]
fn appending_and_editing_are_two_roads_and_both_are_here() {
    let temp = TempDir::new("notes-two-roads");
    let env_root = temp.env(&agent_a());
    let agent = agent_a();
    let run = "run-two-roads";
    let mut os = notes_os(&env_root);
    let mut expansion = RunExpansion::new(agent.clone(), RunId::new(run));
    let mut scope = os.scope(&mut expansion);

    let created = scope.execute(&agent, &op(run), &create_note("两条路", "第一段"));
    assert_eq!(created.state(), ResultState::Success, "{created:?}");
    let id = created.content()[0]
        .text()
        .split(' ')
        .next()
        .unwrap_or_default()
        .to_string();

    // 追加：只往后接。
    let appended = scope.execute(
        &agent,
        &op(run),
        &with_text(&id, "notes_append", "＋第二段"),
    );
    assert_eq!(appended.state(), ResultState::Success, "{appended:?}");
    assert_eq!(body_of(&mut scope, &agent, run, &id), "第一段＋第二段");

    // 就地改：换掉点名的那一处（旧的字**在这里**消失——原文在他的经历里，不在这条笔记里）。
    let edited = scope.execute(
        &agent,
        &op(run),
        &edit_note_action("notes_edit", &id, "第一段", "开头"),
    );
    assert_eq!(edited.state(), ResultState::Success, "{edited:?}");
    assert_eq!(body_of(&mut scope, &agent, run, &id), "开头＋第二段");

    // 能力表里两条路的动词都在（一支同时拿得到"接"和"改"）。
    let fresh = NotesApp::open(&env_root).expect("Notes App");
    let ids: Vec<String> = celira_agent::app_contract::AppPort::declared_capabilities(&fresh)
        .items()
        .iter()
        .map(|capability| capability.id().as_str().to_string())
        .collect();
    for expected in ["notes_append", "notes_edit", "notes_verify_edited"] {
        assert!(
            ids.contains(&expected.to_string()),
            "缺 {expected}：{ids:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 来源索引：这份字**是什么时候、因哪次活动**写下的（判据 19）
// ---------------------------------------------------------------------------

/// **权威在经历账本，不在盘上**——`notes_edit` / `notes_append` 会改写文件，
/// 因此 mtime 只是"最后动过"，答不了"什么时候写下的"。
///
/// 这一条钉四件事：
/// ① 折出来的「创建于」来自账本里那条 `notes_create` 的**动作事实**；
/// ② 索引**只记引用**（身份 ＋ 时刻 ＋ 活动），**不记正文**（判据 19 的后半句）；
/// ③ 索引住在**环境根**、与 `notes/` 平级（它不是笔记存储的一部分）；
/// ④ **缺钟 ⇒ 那一格是 `None`**（不编造）：环境没给钟时 `at-ms` 那一格本来就没有。
#[test]
fn the_origin_index_folds_the_creation_time_out_of_the_ledger() {
    let root = std::env::temp_dir().join(format!("celira-origins-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let agent = AgentId::new("agent-origins");
    let mut env = Environment::create(&root, agent).expect("创建环境");
    // **给环境一只固定钟**：`at-ms` 由它来。
    env.set_clock(ClockHandle::fixed_at_unix_seconds(1_700_000_000));
    // 账本里那一条：`notes_create` 的动作事实，回执块 `created` 带着身份。
    env.append_trace(
        &RunId::new("run-1"),
        TracePayload::Observed {
            turn: 1,
            index: 0,
            action: "notes_create".to_string(),
            params: Vec::new(),
            state: ResultState::Success,
            failure: None,
            blocks: vec![(
                "created".to_string(),
                "note-1 (title 3 bytes, text 6 bytes)".to_string(),
            )],
        },
    )
    .expect("登记动作事实");
    drop(env);
    // 盘上那张纸条（贴着）。
    std::fs::create_dir_all(root.join("notes")).expect("建存储目录");
    std::fs::write(
        root.join("notes").join("note-1.note"),
        r#"{"pinned":true,"title":"词","text":"青苔"}"#,
    )
    .expect("写笔记");

    let rows = celira_agent::notes_app::pinned_material(&root).expect("交素材");
    assert_eq!(rows.len(), 1, "贴着的只有一张：{rows:?}");

    // **先看索引有没有落下来**：它不在 ⇒ 折叠那一步自己失败了（而不是"折到了但没时间"）。
    let index = std::fs::read_to_string(root.join("notes-origins.ndjson"))
        .unwrap_or_else(|error| panic!("索引没落下来（折叠那一步失败了）：{error}"));

    assert_eq!(
        rows[0].created_millis,
        Some(1_700_000_000_000),
        "创建时刻应当从账里折出来（固定钟），而不是盘上猜：{rows:?}"
    );
    assert!(index.contains("note-1"), "记着那条身份：{index}");
    assert!(
        index.contains("run-1"),
        "只记引用（因哪次活动而起）：{index}"
    );
    assert!(!index.contains("青苔"), "**只记引用，不记正文**：{index}");

    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// 真机读数（不进常规套件）
// ---------------------------------------------------------------------------

/// **来源索引在真家那份账上折得出来吗**——判据是 **32/32**（真机读数）。
///
/// **触发条件**（本仓纪律：凡要真环境、进不了常规套件的门禁，必须把触发条件写死）：
///
/// ```text
/// $env:CELIRA_REAL_HOME = "<真家的环境根>"      # 例如 D:\celira-home\agents\<agent-id>
/// cargo test --test notes_app -- --ignored the_origin_index_on_a_real_home --nocapture
/// ```
///
/// **为什么忽略**：它要**真家那份账**（真机 17.9 MB），常规套件里没有。
/// **它只读**：唯一的写入是那份**派生**的索引文件（丢了就重折，与装配期本来就会做的事同一件）。
/// **它不启世界、不花模型调用**——折的是账，不是跑一次活动。
#[test]
#[ignore = "为什么：它要真家那份账（真机 17.9 MB），常规套件里没有真家；什么时候重跑：来源索引的提取规则或账本格式一变就跑（设 CELIRA_REAL_HOME 指向真家环境根后加 --ignored）"]
fn the_origin_index_on_a_real_home() {
    let Ok(root) = std::env::var("CELIRA_REAL_HOME") else {
        panic!("触发条件没满足：CELIRA_REAL_HOME 没设（见本用例的文档注释）");
    };
    let root = PathBuf::from(root);
    assert!(root.is_dir(), "CELIRA_REAL_HOME 不是一个目录：{root:?}");

    let started = std::time::Instant::now();
    let rows = celira_agent::notes_app::pinned_material(&root).expect("交素材");
    let elapsed = started.elapsed();

    let with_created = rows
        .iter()
        .filter(|row| row.created_millis.is_some())
        .count();
    let with_touched = rows
        .iter()
        .filter(|row| row.touched_millis.is_some())
        .count();

    // 盘上有多少条笔记（`.note`）——覆盖率的分母。
    let on_disk = std::fs::read_dir(root.join("notes"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".note"))
                .count()
        })
        .unwrap_or(0);

    let index = std::fs::read_to_string(root.join("notes-origins.ndjson")).unwrap_or_default();
    let edges = index
        .lines()
        .filter(|line| line.starts_with("edge\t"))
        .count();
    let cursors = index
        .lines()
        .filter(|line| line.starts_with("cursor\t"))
        .count();
    let last_cursor = index
        .lines()
        .filter_map(|line| line.strip_prefix("cursor\t"))
        .next_back()
        .unwrap_or("(没有)");

    println!("---- 来源索引 · 真机读数 ----");
    println!("环境根          = {root:?}");
    println!("贴着的纸条      = {} 张", rows.len());
    println!("盘上 .note      = {on_disk} 条");
    println!("折到创建时刻    = {with_created} 条");
    println!("有最后动过      = {with_touched} 条");
    println!("索引边          = {edges} 条 · 游标行 {cursors} 条 · 末游标 seq={last_cursor}");
    println!("折这一趟花了    = {elapsed:?}");
    if let Some(row) = rows.first() {
        println!(
            "首行            = {} | created={:?} | touched={:?}",
            row.id, row.created_millis, row.touched_millis
        );
    }
    println!(
        "索引前 3 行     = {:?}",
        index.lines().take(3).collect::<Vec<_>>()
    );
    println!("-----------------------------");

    // **判据**：盘上每一条都折得出来（真机读数是 32/32）——比的是**索引的边**，不是贴着的行
    // （贴着的只有一部分，那是另一件事）。
    assert_eq!(
        edges, on_disk,
        "盘上 {on_disk} 条，索引里 {edges} 条边——**账里应当一条不落**"
    );
    // 每一张**贴着的**都拿到了创建时刻（它才是那一行要显示的东西）。
    assert_eq!(
        with_created,
        rows.len(),
        "贴着的 {} 张，只有 {with_created} 张折到了创建时刻",
        rows.len()
    );
    assert!(edges > 0, "索引一条边都没落下来");
}
