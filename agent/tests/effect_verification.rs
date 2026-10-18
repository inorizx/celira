//! 写能力的**自证**：复核能力只回答"现在的真实状态是什么"，而且只读。
//!
//! 这一层证明四件事，全部离线、用测试自建的临时目录：
//!
//! 1. 复核是**现在时**：成功 = 现在真的符合那份参数；确定不存在 = 明确失败；存在但不一样 / 读不出来
//!    = 无法确认（**不**报成功、**不**报失败）；
//! 2. 复核**只读**：执行前后环境目录逐字节不变；
//! 3. 两类写的真跑判错：`exclusive-create` 同参数第二次**必失败**，`repeatable-write` 同参数
//!    第二次**必成功**（这也是"类别说的是真的"的唯一可判错证据）；
//! 4. 复核能力**只在有授权时存在**，而且类别与复核由 App 自己声明、装配期检查归属。

use std::fs;
use std::path::{Path, PathBuf};

use celira_agent::app_contract::{AppPort, EffectClass};
use celira_agent::core::{
    Action, ActionAdmission, ActionId, ActionParams, ActionPort, ContentBlock,
};
use celira_agent::creation_grant::CreationGrant;
use celira_agent::file_app::MAX_TEXT_BYTES;
use celira_agent::file_view::FileView;
use celira_agent::foundation::{AgentId, OperationId, ResultState, RunId, Turn};
use celira_agent::notes_app::NotesApp;

const CREATE_TEXT: &str = "files_create";
const VERIFY_TEXT: &str = "files_verify_text";
const EDIT: &str = "files_edit";
const VERIFY_EDIT: &str = "files_verify_edit";
const CREATE_NOTE: &str = "notes_create";
const VERIFY_NOTE: &str = "notes_verify_note";

/// 自建临时环境：`materials/`（授权资料根）+ `env/`（环境根）+ `workman/`（被授权的单层目录）。
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(label: &str) -> Self {
        let unique = format!(
            "celira-effect-verification-{}-{}-{}",
            std::process::id(),
            label,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("时钟可用")
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        fs::create_dir_all(root.join("materials").join("workman")).expect("建资料目录");
        fs::create_dir_all(root.join("env")).expect("建环境根");
        Self { root }
    }

    fn materials(&self) -> PathBuf {
        self.root.join("materials")
    }

    fn env(&self) -> PathBuf {
        self.root.join("env")
    }

    /// 环境目录的**逐字节快照**：相对路径 + 内容。复核的"只读"用它判错。
    fn snapshot(&self) -> Vec<(String, Vec<u8>)> {
        snapshot_of(&self.root)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// 递归取一份"相对路径 → 字节"的机械快照（按路径排序）。
fn snapshot_of(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .expect("在根之下")
                .to_string_lossy()
                .into_owned();
            out.push((relative, fs::read(&path).unwrap_or_default()));
        }
    }
    out.sort_by(|left, right| left.0.cmp(&right.0));
    out
}

fn agent() -> AgentId {
    AgentId::new("agent-a")
}

fn operation(run: &str) -> OperationId {
    OperationId::new(RunId::new(run), Turn::FIRST)
}

fn call(id: &str, params: &[(&str, &str)]) -> Action {
    let mut built = ActionParams::none();
    for (name, value) in params {
        built = built.with(*name, *value);
    }
    Action::with_params(ActionId::new(id), built)
}

fn file_view(sandbox: &Sandbox, run: &str) -> FileView {
    let grant = CreationGrant::new(agent(), RunId::new(run), "workman").expect("授权形状合法");
    FileView::with_history_and_creation(&sandbox.materials(), &sandbox.env(), Some(&grant))
        .expect("带创建授权的视图")
}

fn notes_app(sandbox: &Sandbox, _run: &str) -> NotesApp {
    // 笔记写在自己的环境里：装配即带写能力，不需要任何授权。
    NotesApp::open(sandbox.env()).expect("Notes App")
}

fn capability_ids(app: &dyn AppPort) -> Vec<String> {
    app.declared_capabilities()
        .items()
        .iter()
        .map(|capability| capability.id().as_str().to_string())
        .collect()
}

fn action_ids(app: &dyn AppPort) -> Vec<String> {
    app.declared_effects()
        .iter()
        .map(|effect| effect.action().as_str().to_string())
        .collect()
}

/// 规范句 3/4 + 判据 4：复核是现在时，而且**只读**——执行前后环境目录逐字节不变。
#[test]
fn verifying_an_exclusive_creation_is_now_tense_and_changes_nothing() {
    let sandbox = Sandbox::new("verify-text-ok");
    let mut view = file_view(&sandbox, "run-1");
    let agent = agent();
    let operation = operation("run-1");

    let created = view.execute(
        &agent,
        &operation,
        &call(CREATE_TEXT, &[("name", "a.txt"), ("text", "hello")]),
    );
    assert_eq!(created.state(), ResultState::Success);

    let before = sandbox.snapshot();
    let verified = view.execute(
        &agent,
        &operation,
        &call(VERIFY_TEXT, &[("name", "a.txt"), ("text", "hello")]),
    );
    let after = sandbox.snapshot();

    assert_eq!(verified.state(), ResultState::Success);
    assert_eq!(before, after, "复核只读：环境必须逐字节不变");
    assert!(
        verified
            .content()
            .iter()
            .any(|block| block.label() == "verified"),
        "成功证据必须标记成复核（不是创建）"
    );
}

/// 规范句 5 / 判据 3：确定不存在 = **明确失败**，带稳定分类。
#[test]
fn verifying_a_missing_creation_is_a_definite_failure() {
    let sandbox = Sandbox::new("verify-text-absent");
    let mut view = file_view(&sandbox, "run-1");

    let outcome = view.execute(
        &agent(),
        &operation("run-1"),
        &call(VERIFY_TEXT, &[("name", "never.txt"), ("text", "hello")]),
    );

    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(outcome.failure_class(), Some("not-found"));
}

/// 规范句 5 / 判据 3 + 5：存在但字节不一致 = **无法确认**，不许报成功也不许报失败。
#[test]
fn verifying_a_mismatched_body_is_neither_success_nor_failure() {
    let sandbox = Sandbox::new("verify-text-mismatch");
    let mut view = file_view(&sandbox, "run-1");
    let agent = agent();
    let operation = operation("run-1");

    let created = view.execute(
        &agent,
        &operation,
        &call(CREATE_TEXT, &[("name", "b.txt"), ("text", "original")]),
    );
    assert_eq!(created.state(), ResultState::Success);

    let outcome = view.execute(
        &agent,
        &operation,
        &call(VERIFY_TEXT, &[("name", "b.txt"), ("text", "different")]),
    );

    assert_eq!(outcome.state(), ResultState::Unknown);
    assert_eq!(outcome.failure_class(), None, "无法确认不是失败分类");
}

/// 判据 2 的真跑对照：独占创建同参数第二次**必失败**；可重复写同参数第二次**必成功**。
///
/// 这一条是"类别说的是真的"唯一可判错的证据——类别不是文档承诺，是能被真跑反驳的断言。
#[test]
fn exclusive_create_fails_the_second_time_while_repeatable_write_succeeds() {
    let sandbox = Sandbox::new("write-classes");
    let agent = agent();
    let operation = operation("run-1");

    let mut view = file_view(&sandbox, "run-1");
    let params = [("name", "same.txt"), ("text", "same")];
    assert_eq!(
        view.execute(&agent, &operation, &call(CREATE_TEXT, &params))
            .state(),
        ResultState::Success
    );
    let second = view.execute(&agent, &operation, &call(CREATE_TEXT, &params));
    assert_eq!(
        second.state(),
        ResultState::Failure,
        "独占创建的同参数第二次必须失败"
    );

    let mut notes = notes_app(&sandbox, "run-1");
    let note = [("title", "same title"), ("text", "same body")];
    assert_eq!(
        notes
            .execute(&agent, &operation, &call(CREATE_NOTE, &note))
            .state(),
        ResultState::Success
    );
    let again = notes.execute(&agent, &operation, &call(CREATE_NOTE, &note));
    assert_eq!(
        again.state(),
        ResultState::Success,
        "可重复写的同参数第二次必须成功（身份是执行时抢的）"
    );
}

/// 接缝 E / 判据 12：复核能力**只在有授权时存在**——没有授权时连声明都没有。
#[test]
fn the_verification_capability_exists_exactly_when_the_grant_does() {
    let sandbox = Sandbox::new("verify-declared-with-grant");
    let with_grant = file_view(&sandbox, "run-1");
    let ids = capability_ids(&with_grant);
    assert!(ids.contains(&CREATE_TEXT.to_string()));
    assert!(ids.contains(&VERIFY_TEXT.to_string()));
    assert!(ids.contains(&EDIT.to_string()), "改文本与创建同挂授权");
    assert!(ids.contains(&VERIFY_EDIT.to_string()));

    let without_grant =
        FileView::with_history_and_creation(&sandbox.materials(), &sandbox.env(), None)
            .expect("只读视图");
    let ids = capability_ids(&without_grant);
    assert!(!ids.contains(&CREATE_TEXT.to_string()));
    assert!(!ids.contains(&VERIFY_TEXT.to_string()));
    assert!(
        !ids.contains(&EDIT.to_string()),
        "没有授权时改文本连能力都不出现（不是\"存在但会被拒绝\"）"
    );
    assert!(!ids.contains(&VERIFY_EDIT.to_string()));
    // ⚠ **作品面与授权无关**（2026-10-14 起）：她自己的那个 `projects/` 不需要任何授权，
    // 所以"没有授权"这一档剩下的正是**它**那一对——而不是"一条声明都没有"。
    assert_eq!(
        action_ids(&without_grant),
        vec!["projects_create".to_string(), "projects_edit".to_string()],
        "没有授权 ⇒ 资料面那两件不出现；作品面那两件照旧在场（她自己的东西不用申请）"
    );
}

/// 规范句 2 / 判据 1 的正向面：两个 App 各自声明**自己的**类别与复核，互不混用。
#[test]
fn the_two_write_capabilities_declare_their_own_classes_and_verifications() {
    let sandbox = Sandbox::new("declared-classes");

    let view = file_view(&sandbox, "run-1");
    let effects = view.declared_effects();
    assert_eq!(
        action_ids(&view),
        vec![
            CREATE_TEXT.to_string(),
            EDIT.to_string(),
            "projects_create".to_string(),
            "projects_edit".to_string(),
        ],
        "TASK_37 起文件 App 有两件写：创建与改文本；2026-10-14 起作品面再来两件（她自己的目录）"
    );
    assert_eq!(effects[0].class(), EffectClass::ExclusiveCreate);
    assert_eq!(
        effects[0].verification().map(|id| id.as_str()),
        Some(VERIFY_TEXT)
    );
    // 改文本与独占创建**同类**：判据是同参重放——第一次消耗掉前置（`old_text` 被换走），
    // 第二次同参数必然明确失败且无害。因此不新增效果类别、不动轴。
    assert_eq!(effects[1].class(), EffectClass::ExclusiveCreate);
    assert_eq!(
        effects[1].verification().map(|id| id.as_str()),
        Some(VERIFY_EDIT)
    );

    let notes = notes_app(&sandbox, "run-1");
    let effects = notes.declared_effects();
    assert_eq!(
        action_ids(&notes),
        vec![
            CREATE_NOTE.to_string(),
            "notes_pin".to_string(),
            "notes_delete".to_string(),
            "notes_unpin".to_string(),
            // ⚠ **2026-10-13 加的第五条**：追加也是写（只往后接、不改已有的字），
            // 因此照样「可重复写 ＋ 一个同参复核」（见 `agent/README.md` §3.79）。
            "notes_append".to_string(),
            // ⚠ **2026-10-14 加的第六条，而且它是 Notes 里第一条不同形状的**：就地改与
            // `files_edit` 同类（独占创建那一档）——同参数第二次必然明确失败（前置被换走了）。
            "notes_edit".to_string()
        ],
        "TASK_33 起 Notes 还有一对贴/摘（同一个 App 自己声明的动作）；2026-10-14 起再加一条就地改"
    );
    for (index, (action, verification)) in [
        (CREATE_NOTE, VERIFY_NOTE),
        ("notes_pin", "notes_verify_pinned"),
        ("notes_delete", "notes_verify_deleted"),
        ("notes_unpin", "notes_verify_pinned"),
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(effects[index].action().as_str(), *action);
        assert_eq!(
            effects[index].class(),
            EffectClass::RepeatableWrite,
            "Notes 的写都是可重复写（抢的是编号、或只动标记，不是内容）"
        );
        assert_eq!(
            effects[index].verification().map(|id| id.as_str()),
            Some(*verification),
            "写必须配复核；贴与摘**共用**同一个复核（无法归因到是哪一次，见 README §3.22）"
        );
    }
}

/// 判据 4：复核自己也是只读的（不新增、不删除、不改动笔记文件）。
#[test]
fn verifying_a_note_is_now_tense_and_changes_nothing() {
    let sandbox = Sandbox::new("verify-note-ok");
    let mut notes = notes_app(&sandbox, "run-1");
    let agent = agent();
    let operation = operation("run-1");
    let params = [("title", "标题"), ("text", "正文")];

    assert_eq!(
        notes
            .execute(&agent, &operation, &call(CREATE_NOTE, &params))
            .state(),
        ResultState::Success
    );

    let before = sandbox.snapshot();
    let outcome = notes.execute(&agent, &operation, &call(VERIFY_NOTE, &params));
    let after = sandbox.snapshot();

    assert_eq!(outcome.state(), ResultState::Success);
    assert_eq!(before, after, "复核只读：笔记目录必须逐字节不变");
}

/// 判据 3：笔记的"确定一条都没有"也是明确失败，不是无法确认。
#[test]
fn verifying_a_missing_note_is_a_definite_failure() {
    let sandbox = Sandbox::new("verify-note-absent");
    let mut notes = notes_app(&sandbox, "run-1");

    let outcome = notes.execute(
        &agent(),
        &operation("run-1"),
        &call(VERIFY_NOTE, &[("title", "没写过"), ("text", "没写过")]),
    );

    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(outcome.failure_class(), Some("not-found"));
}

/// **如实登记可重复写复核的弱化**：它无法把结论归因到那一次操作。
///
/// 这里把"那一次创建"的产物删掉，再手工放一条同标题同正文、但编号不同的笔记——复核仍然报成功。
/// 这不是缺陷，而是类别本身带来的边界（身份是执行时抢的、不在参数里）；本用例的作用是让它
/// **可判错**，而不是靠文档措辞掩盖。
#[test]
fn a_repeatable_writes_verification_cannot_attribute_the_effect() {
    let sandbox = Sandbox::new("verify-note-attribution");
    let mut notes = notes_app(&sandbox, "run-1");
    let agent = agent();
    let operation = operation("run-1");
    let params = [("title", "same"), ("text", "same")];

    assert_eq!(
        notes
            .execute(&agent, &operation, &call(CREATE_NOTE, &params))
            .state(),
        ResultState::Success
    );

    // 删掉那一次创建的产物，手工放一条**同内容、不同编号**的笔记。
    let storage = sandbox.env().join("notes");
    fs::remove_file(storage.join("note-1.note")).expect("删掉那一条");
    fs::write(
        storage.join("note-9.note"),
        "{\"text\":\"same\",\"title\":\"same\"}\n",
    )
    .expect("手工放一条同样内容的笔记");

    let outcome = notes.execute(&agent, &operation, &call(VERIFY_NOTE, &params));
    assert_eq!(
        outcome.state(),
        ResultState::Success,
        "复核只能断言\"现在至少存在一条\"，无法归因到那一次操作"
    );
    assert!(
        outcome
            .content()
            .iter()
            .any(|block: &ContentBlock| block.text().contains("at least one")),
        "证据文本必须说的是\"至少存在一条\"，不是\"那一次成功了\""
    );
}

// ---------------------------------------------------------------------------
// TASK_37：`files_edit` 的行为面（唯一替换 + 复核三分 + 边界）
// ---------------------------------------------------------------------------

/// 唯一出现 ⇒ 换掉那一处；复核是**现在时**而且**只读**。
#[test]
fn editing_replaces_the_one_occurrence_and_its_verification_is_now_tense_and_read_only() {
    let sandbox = Sandbox::new("edit-ok");
    let mut view = file_view(&sandbox, "run-1");
    let workman = sandbox.materials().join("workman");
    fs::write(workman.join("draft.txt"), "alpha BETA gamma").expect("预置材料");

    let edited = view.execute(
        &agent(),
        &operation("run-1"),
        &call(
            EDIT,
            &[
                ("path", "draft.txt"),
                ("old_text", "BETA"),
                ("new_text", "beta"),
            ],
        ),
    );
    assert_eq!(edited.state(), ResultState::Success);
    assert_eq!(
        fs::read_to_string(workman.join("draft.txt")).expect("读回"),
        "alpha beta gamma",
        "只换那一处，前后原样保留"
    );

    let before = sandbox.snapshot();
    let verified = view.execute(
        &agent(),
        &operation("run-1"),
        &call(
            VERIFY_EDIT,
            &[
                ("path", "draft.txt"),
                ("old_text", "BETA"),
                ("new_text", "beta"),
            ],
        ),
    );
    let after = sandbox.snapshot();

    assert_eq!(verified.state(), ResultState::Success);
    assert_eq!(before, after, "复核只读：环境必须逐字节不变");
}

/// `old_text` 一次都没出现 / 出现多次 ⇒ 都**明确失败**，而且盘上一个字节都不动。
#[test]
fn editing_refuses_an_absent_or_ambiguous_old_text_and_changes_nothing() {
    let sandbox = Sandbox::new("edit-unique");
    let mut view = file_view(&sandbox, "run-1");
    let workman = sandbox.materials().join("workman");
    fs::write(workman.join("two.txt"), "same same").expect("预置材料");
    let before = sandbox.snapshot();

    let absent = view.execute(
        &agent(),
        &operation("run-1"),
        &call(
            EDIT,
            &[
                ("path", "two.txt"),
                ("old_text", "nothing"),
                ("new_text", "x"),
            ],
        ),
    );
    assert_eq!(absent.state(), ResultState::Failure, "没找到就是明确失败");
    assert_eq!(absent.failure_class(), Some("not-found"));

    let ambiguous = view.execute(
        &agent(),
        &operation("run-1"),
        &call(
            EDIT,
            &[("path", "two.txt"), ("old_text", "same"), ("new_text", "x")],
        ),
    );
    assert_eq!(
        ambiguous.state(),
        ResultState::Failure,
        "多处出现是歧义，不许换第一处、也不许全换"
    );
    assert_eq!(ambiguous.failure_class(), Some("invalid-argument"));

    assert_eq!(sandbox.snapshot(), before, "两种拒绝都不许动盘上一个字节");
}

/// 空串语义：`old_text` 空串**拒绝**（空串匹配一切，那不是替换）；`new_text` 空串**合法**（删掉那一段）。
#[test]
fn an_empty_old_text_is_refused_while_an_empty_new_text_deletes_the_snippet() {
    let sandbox = Sandbox::new("edit-empty");
    let mut view = file_view(&sandbox, "run-1");
    let workman = sandbox.materials().join("workman");
    fs::write(workman.join("cut.txt"), "keep DROP keep").expect("预置材料");

    let refused = view.execute(
        &agent(),
        &operation("run-1"),
        &call(
            EDIT,
            &[("path", "cut.txt"), ("old_text", ""), ("new_text", "x")],
        ),
    );
    assert_eq!(refused.state(), ResultState::Failure);
    assert_eq!(refused.failure_class(), Some("invalid-argument"));
    assert_eq!(
        fs::read_to_string(workman.join("cut.txt")).expect("读回"),
        "keep DROP keep"
    );

    let deleted = view.execute(
        &agent(),
        &operation("run-1"),
        &call(
            EDIT,
            &[("path", "cut.txt"), ("old_text", "DROP "), ("new_text", "")],
        ),
    );
    assert_eq!(deleted.state(), ResultState::Success);
    assert_eq!(
        fs::read_to_string(workman.join("cut.txt")).expect("读回"),
        "keep keep",
        "换成空 = 把那一段删掉"
    );
}

/// 替换后全文超过 64 KiB ⇒ **整体拒绝**，不截断、不写半个字。
#[test]
fn an_oversized_edit_is_refused_whole_and_leaves_the_file_untouched() {
    let sandbox = Sandbox::new("edit-toolarge");
    let mut view = file_view(&sandbox, "run-1");
    let workman = sandbox.materials().join("workman");
    fs::write(workman.join("big.txt"), "seed").expect("预置材料");
    let before = sandbox.snapshot();

    let oversized = "x".repeat(MAX_TEXT_BYTES as usize + 1);
    let outcome = view.execute(
        &agent(),
        &operation("run-1"),
        &call(
            EDIT,
            &[
                ("path", "big.txt"),
                ("old_text", "seed"),
                ("new_text", oversized.as_str()),
            ],
        ),
    );
    assert_eq!(outcome.state(), ResultState::Failure);
    assert_eq!(outcome.failure_class(), Some("too-large"));
    assert_eq!(
        sandbox.snapshot(),
        before,
        "超限要整体拒绝：不截断、不写半个字"
    );
}

/// 同参重放：第一次消耗掉前置（`old_text` 被换走），第二次同参数**必然明确失败且无害**。
///
/// 这就是它归 `ExclusiveCreate` 而不是某个新类别的**可判错证据**。
#[test]
fn the_same_edit_parameter_set_fails_the_second_time() {
    let sandbox = Sandbox::new("edit-replay");
    let mut view = file_view(&sandbox, "run-1");
    let workman = sandbox.materials().join("workman");
    fs::write(workman.join("once.txt"), "one two").expect("预置材料");

    let params = [("path", "once.txt"), ("old_text", "one"), ("new_text", "1")];
    let first = view.execute(&agent(), &operation("run-1"), &call(EDIT, &params));
    assert_eq!(first.state(), ResultState::Success, "第一次消耗掉前置");

    let second = view.execute(&agent(), &operation("run-1"), &call(EDIT, &params));
    assert_eq!(
        second.state(),
        ResultState::Failure,
        "同参第二次必然明确失败"
    );
    assert_eq!(second.failure_class(), Some("not-found"));
    assert_eq!(
        fs::read_to_string(workman.join("once.txt")).expect("读回"),
        "1 two",
        "第二次失败还必须是无害的"
    );
}

/// 复核三分：`old_text` 还在 = 明确失败；但**读不出来**或**存在一份不一样的东西**都只能报无法确认。
#[test]
fn editing_verification_is_definite_only_when_the_old_text_is_still_there() {
    let sandbox = Sandbox::new("verify-edit-three");
    let mut view = file_view(&sandbox, "run-1");
    let workman = sandbox.materials().join("workman");
    fs::write(workman.join("still.txt"), "before").expect("预置材料");

    let still = view.execute(
        &agent(),
        &operation("run-1"),
        &call(
            VERIFY_EDIT,
            &[
                ("path", "still.txt"),
                ("old_text", "before"),
                ("new_text", "after"),
            ],
        ),
    );
    assert_eq!(
        still.state(),
        ResultState::Failure,
        "旧文本还在 ⇒ 那次替换没留在盘上"
    );
    assert_eq!(still.failure_class(), Some("not-found"));

    let missing = view.execute(
        &agent(),
        &operation("run-1"),
        &call(
            VERIFY_EDIT,
            &[
                ("path", "never.txt"),
                ("old_text", "before"),
                ("new_text", "after"),
            ],
        ),
    );
    assert_eq!(
        missing.state(),
        ResultState::Unknown,
        "读不出来（不存在）⇒ 无法确认"
    );

    fs::write(workman.join("other.txt"), "something else").expect("预置材料");
    let other = view.execute(
        &agent(),
        &operation("run-1"),
        &call(
            VERIFY_EDIT,
            &[
                ("path", "other.txt"),
                ("old_text", "before"),
                ("new_text", "after"),
            ],
        ),
    );
    assert_eq!(
        other.state(),
        ResultState::Unknown,
        "旧文本不在但新文本也不在 ⇒ 存在一份不一样的东西 ⇒ 不能断言成功也不能断言失败"
    );
}

/// 写范围就是**被授权的那一个目录**：其下任意深度可以走，越出它一律拒绝；没有授权时连门都进不去。
#[test]
fn editing_stays_inside_the_granted_directory_and_needs_the_grant() {
    let sandbox = Sandbox::new("edit-boundary");
    let workman = sandbox.materials().join("workman");
    fs::write(sandbox.materials().join("outside.txt"), "outside").expect("预置资料根材料");

    fs::create_dir_all(workman.join("deep")).expect("建子目录");
    fs::write(workman.join("deep").join("inner.txt"), "in inner").expect("预置");

    let mut view = file_view(&sandbox, "run-1");
    let nested = view.execute(
        &agent(),
        &operation("run-1"),
        &call(
            EDIT,
            &[
                ("path", "deep/inner.txt"),
                ("old_text", "inner"),
                ("new_text", "INNER"),
            ],
        ),
    );
    assert_eq!(nested.state(), ResultState::Success, "授权目录之下任意深度");
    assert_eq!(
        fs::read_to_string(workman.join("deep").join("inner.txt")).expect("读回"),
        "in INNER"
    );

    let escape = view.execute(
        &agent(),
        &operation("run-1"),
        &call(
            EDIT,
            &[
                ("path", "../outside.txt"),
                ("old_text", "outside"),
                ("new_text", "hacked"),
            ],
        ),
    );
    assert_eq!(escape.state(), ResultState::Failure);
    assert_eq!(escape.failure_class(), Some("invalid-path"));
    assert_eq!(
        fs::read_to_string(sandbox.materials().join("outside.txt")).expect("读回"),
        "outside",
        "授权目录之外的材料一个字节都不许动"
    );

    let readonly = FileView::with_history_and_creation(&sandbox.materials(), &sandbox.env(), None)
        .expect("只读视图");
    assert_eq!(
        readonly.admit(
            &agent(),
            &RunId::new("run-1"),
            &call(
                EDIT,
                &[
                    ("path", "deep/inner.txt"),
                    ("old_text", "INNER"),
                    ("new_text", "x"),
                ],
            ),
        ),
        ActionAdmission::refused("creation-not-granted"),
        "没有授权：准入就拒，连执行都到不了"
    );
}
