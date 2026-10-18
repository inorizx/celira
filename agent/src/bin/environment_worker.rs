//! 阶段八端到端验收用的**离线**工作进程：它可以扮演 A 或 B。
//!
//! 这**不是**产品的一部分，而是测试夹具：它让"两个独立进程共享一个身份环境"这件事
//! 可以被真的执行、真的观察，而不是在同一个进程里假装。
//!
//! - `a`：建立一个身份环境，跑一次活动，产出一条带正文的合成经历，正常收场后退出；
//! - `b`：独立打开同一身份的环境，开始新的一次活动，**照真实可见的能力目录**主动去查
//!   A 留下的经历，并把观察结果打到 stdout；
//! - `other-identity`：用别的身份打开同一环境（应当被拒绝）；
//! - `duplicate-run`：重复使用已经用过的活动标识（应当在任何模型/动作之前被拒绝）；
//! - `second-writer`：在上一个写者仍持有时尝试打开（应当被拒绝）；
//! - `pause-by-real-failure`：经**真实入口**制造一次保存失败，让环境留下暂停记录；
//! - `leave-unfinished`：制造"活动开始了、没有收尾、也没有暂停记录"的形状；
//! - `must-refuse`：在一个已经有异常痕迹的环境里尝试开始新活动（必须被拒绝，且不打模型）；
//! - `note-in-new-segment`：在一个已经留下未收尾活动的环境里开**新的一段**（会话路径），
//!   并回报开场那一轮有没有收到那块 `previous-activity` 纸条、纸条有没有点名那个未收尾的活动。
//! - `diagnose-seed`：建立一个可供**另一个进程**只读诊断的合成环境——两个正常活动、一个结果未知
//!   的活动、一个“结束已确认但没有封存”的活动、一个只在事实侧出现的活动，最后留下一个没有收尾
//!   的活动；并把两份存储的已确认条数打到 stdout，供另一个进程核对。
//!
//! 全程离线：不联网、不读密钥、只用测试自建的独立目录与合成文本。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use celira_agent::clock::ClockHandle;
use celira_agent::commit::EnvironmentCommit;
use celira_agent::composition::{
    CompositionError, EnvironmentActivity, RunSession, SessionDeclarations, SessionRequest,
    run_activity_in_environment,
};
use celira_agent::context_reading::ContextReading;
use celira_agent::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Agent,
    Capabilities, CommitError, CommitSeam, CommitStage, ContentBatch, ContentBlock, ContextSource,
    InjectionContext, InjectionStage, Limits, ModelError, ModelFailureClass, ModelOutput,
    ModelPort, NoContext, RunInput, RunRequest, TraceEntry, View, no_cost, run_activity_with,
};
use celira_agent::cost::RunCost;
use celira_agent::file_app::FILE_APP_ID;
use celira_agent::file_view::HISTORY_DIR;
use celira_agent::foundation::{
    AgentId, Ledger, OperationId, Outcome, Record, RunId, SubjectEnding, Turn,
};
use celira_agent::previous_activity::PREVIOUS_ACTIVITY_LABEL;
use celira_agent::store::{Environment, StoreError};

const ROLE_ENV: &str = "CELIRA_PHASE08_ROLE";
const ROOT_ENV: &str = "CELIRA_PHASE08_ROOT";

/// A 保存下来的那句合成原文——B 必须能查到它。
pub const A_SAVED_TEXT: &str = "A 在第一次清醒期里写下的合成经历正文";
/// A 的开场输入。
pub const A_INPUT: &str = "A 的初始输入";

fn agent_a() -> AgentId {
    AgentId::new("agent-alpha")
}

fn agent_b() -> AgentId {
    AgentId::new("agent-beta")
}

fn main() {
    let role = std::env::var(ROLE_ENV).expect("必须给出角色");
    let root = PathBuf::from(std::env::var(ROOT_ENV).expect("必须给出环境根"));
    let code = match role.as_str() {
        "a" => worker_a(&root),
        "b" => worker_b(&root),
        "other-identity" => worker_other_identity(&root),
        "duplicate-run" => worker_duplicate_run(&root),
        "second-writer" => worker_second_writer(&root),
        "pause-by-real-failure" => worker_pause_by_real_failure(&root),
        "leave-unfinished" => worker_leave_unfinished(&root),
        "must-run" => worker_must_run(&root),
        "note-in-new-segment" => worker_note_in_new_segment(&root),
        "diagnose-seed" => worker_diagnose_seed(&root),
        other => {
            eprintln!("unknown role: {other}");
            2
        }
    };
    std::process::exit(code);
}

// ---------------------------------------------------------------------------
// A：建立环境、产出经历、正常收场
// ---------------------------------------------------------------------------

fn worker_a(root: &Path) -> i32 {
    let env_dir = root.join("env");
    let sandbox = root.join("sandbox");
    std::fs::create_dir_all(&sandbox).expect("建沙盒");

    let mut env = match Environment::create(&env_dir, agent_a()) {
        Ok(env) => env,
        Err(error) => {
            eprintln!("A 建立环境失败: {error:?}");
            return 1;
        }
    };

    let mut model = ScriptedModel::new(vec![
        ModelOutput::act(Action::with_params(
            ActionId::new("open_app"),
            ActionParams::none().with("target", "files"),
        )),
        ModelOutput::act(Action::with_params(
            ActionId::new("probe"),
            ActionParams::none().with("note", A_SAVED_TEXT),
        )),
        ModelOutput::finish(SubjectEnding::Completed),
    ]);

    match run_activity_in_environment(
        &sandbox,
        &mut env,
        EnvironmentActivity {
            declared_agent: &agent_a(),
            run: RunId::new("run-a"),
            limits: Limits::new(8),
            input: RunInput::text(A_INPUT).expect("在上限内"),
            source: &mut OneShotSource::new(A_SAVED_TEXT),
            creation: None,
        },
        &mut model,
    ) {
        Ok(_) => 0,
        Err(error) => {
            eprintln!("A 的活动失败: {error:?}");
            1
        }
    }
}

// ---------------------------------------------------------------------------
// B：独立打开同一身份，新活动，第一轮不带 A 正文，主动查 A
// ---------------------------------------------------------------------------

fn worker_b(root: &Path) -> i32 {
    let env_dir = root.join("env");
    let sandbox = root.join("sandbox");

    let mut env = match Environment::open(&env_dir, agent_a()) {
        Ok(env) => env,
        Err(error) => {
            eprintln!("B 打开环境失败: {error:?}");
            return 1;
        }
    };

    let mut model = InspectingModel::default();

    if let Err(error) = run_activity_in_environment(
        &sandbox,
        &mut env,
        EnvironmentActivity {
            declared_agent: &agent_a(),
            run: RunId::new("run-b"),
            limits: Limits::new(8),
            input: RunInput::text("B 的初始输入").expect("在上限内"),
            source: &mut NoContext,
            creation: None,
        },
        &mut model,
    ) {
        eprintln!("B 的活动失败: {error:?}");
        return 1;
    }

    println!(
        "B_REPORT:{}|{}|{}|{}|{}|{}|{}",
        model.first_view_had_a_text,
        model.saw_list,
        model.saw_read,
        model.read_back_contains_a_text,
        model.read_back_had_multiple,
        model.first_view_had_no_observations,
        model.observations_match_own_actions,
    );
    0
}

// ---------------------------------------------------------------------------
// 其它角色
// ---------------------------------------------------------------------------

fn worker_other_identity(root: &Path) -> i32 {
    match Environment::open(&root.join("env"), agent_b()) {
        Err(StoreError::IdentityMismatch { .. }) => 0,
        other => {
            eprintln!("别的身份竟然打开了环境: {other:?}");
            1
        }
    }
}

fn worker_duplicate_run(root: &Path) -> i32 {
    let mut env = match Environment::open(&root.join("env"), agent_a()) {
        Ok(env) => env,
        Err(error) => {
            eprintln!("打开失败: {error:?}");
            return 1;
        }
    };

    // 重复使用 A 已经用过的活动标识：必须在模型与动作之前就被拒绝。
    let mut model = CountingModel::default();
    let mut actions = CountingActions::default();
    let mut agent = Agent::new(agent_a());
    let mut ledger = Ledger::new(agent_a());
    let mut commit = EnvironmentCommit::new(&mut env);

    let result = run_activity_with(
        &mut agent,
        RunRequest {
            run: RunId::new("run-a"),
            limits: Limits::new(8),
            input: RunInput::empty(),
            source: &mut NoContext,
            commit: &mut commit,
            cost: &no_cost,
        },
        &mut model,
        &mut actions,
        &mut ledger,
    );

    if result.is_ok() {
        eprintln!("重复活动标识竟然被接受");
        return 1;
    }
    if model.calls != 0 || actions.calls != 0 {
        eprintln!(
            "重复活动标识被拒绝前就已经调用了模型({})或动作({})",
            model.calls, actions.calls
        );
        return 1;
    }
    0
}

fn worker_second_writer(root: &Path) -> i32 {
    match Environment::open(&root.join("env"), agent_a()) {
        Err(StoreError::AlreadyOpen) => 0,
        other => {
            eprintln!("第二个写者竟然打开了环境: {other:?}");
            1
        }
    }
}

// ---------------------------------------------------------------------------
// 准入：一次真实的保存失败 / 没有收尾的形状 / 必须被拒绝的新活动
// ---------------------------------------------------------------------------

/// 经**真实入口**制造一次保存失败：先让一个活动正常收尾并封存，再用**同一个活动标识**开第二次。
///
/// 第二次会在"活动开始"这个确认点上被存储拒绝，走的全是生产路径（准入读数、提交接缝、
/// 失败处理都不是测试专用代码）。失败之后组合入口会尽力留下持久暂停记录。
///
/// 打出 `PAUSE_REPORT:<stage>|<ending_confirmed>|<pause_recorded>`。
fn worker_pause_by_real_failure(root: &Path) -> i32 {
    let env_dir = root.join("env");
    let sandbox = root.join("sandbox");
    std::fs::create_dir_all(&sandbox).expect("建沙盒");

    let mut env = match Environment::create(&env_dir, agent_a()) {
        Ok(env) => env,
        Err(error) => {
            eprintln!("建立环境失败: {error:?}");
            return 1;
        }
    };

    // 第一次：正常收尾并封存。
    let mut first = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Completed)]);
    if let Err(error) = run_activity_in_environment(
        &sandbox,
        &mut env,
        EnvironmentActivity {
            declared_agent: &agent_a(),
            run: RunId::new("run-pause"),
            limits: Limits::new(4),
            input: RunInput::empty(),
            source: &mut NoContext,
            creation: None,
        },
        &mut first,
    ) {
        eprintln!("第一次活动本应成功: {error:?}");
        return 1;
    }

    // 第二次：同一个标识再开一次 → 存储拒绝写入。
    let mut second = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Completed)]);
    match run_activity_in_environment(
        &sandbox,
        &mut env,
        EnvironmentActivity {
            declared_agent: &agent_a(),
            run: RunId::new("run-pause"),
            limits: Limits::new(4),
            input: RunInput::empty(),
            source: &mut NoContext,
            creation: None,
        },
        &mut second,
    ) {
        Err(CompositionError::SaveFailed {
            stage,
            ending_confirmed,
            pause_recorded,
        }) => {
            if second.calls() != 0 {
                eprintln!("保存失败之前就调用了模型");
                return 1;
            }
            println!(
                "PAUSE_REPORT:{}|{}|{}",
                stage.as_str(),
                ending_confirmed,
                pause_recorded
            );
            0
        }
        other => {
            eprintln!("本应是一次保存失败，却得到 {other:?}");
            1
        }
    }
}

/// 制造"活动开始了、**没有收尾**、而且**没有**暂停记录"的形状。
///
/// 这就是"暂停标记自己也写失败"留下的形状：进程在活动进行中被切断，除了已确认的前缀，
/// 什么都没留下。准入必须靠"没有收尾的活动"认出它，而不是靠暂停标记。
fn worker_leave_unfinished(root: &Path) -> i32 {
    let env_dir = root.join("env");
    let mut env = match Environment::create(&env_dir, agent_a()) {
        Ok(env) => env,
        Err(error) => {
            eprintln!("建立环境失败: {error:?}");
            return 1;
        }
    };

    // 前两个确认点成功（活动开始 + 输入），第三个确认点（Run 级注入）失败：
    // 活动和输入都在，唯独没有收尾——而且这一次失败发生在模型调用**之前**。
    let mut model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Completed)]);
    let mut actions = CountingActions::default();
    let mut agent = Agent::new(agent_a());
    let mut ledger = Ledger::new(agent_a());
    let mut commit = FailAfterCommit::new(EnvironmentCommit::new(&mut env), 2);

    let result = run_activity_with(
        &mut agent,
        RunRequest {
            run: RunId::new("run-unfinished"),
            limits: Limits::new(8),
            input: RunInput::text("没写完的输入").expect("在上限内"),
            source: &mut OneShotSource::new("没写完的开场注入"),
            commit: &mut commit,
            cost: &no_cost,
        },
        &mut model,
        &mut actions,
        &mut ledger,
    );

    match result {
        Err(celira_agent::core::RunError::Commit(_)) => {
            if model.calls() != 0 {
                eprintln!("失败之前就调用了模型");
                return 1;
            }
            0
        }
        other => {
            eprintln!("本应在提交时失败，却得到 {other:?}");
            1
        }
    }
}

/// 在一个已经有异常痕迹的环境里尝试开始新活动：**必须放行**（2026-10-06 起），
/// 并如实报出这一次**到底跑没跑**（模型被调用了几次）。
///
/// 打出 `REFUSAL_REPORT:<kind>|calls=<n>`。
///
/// ⚠ **旧规矩 → 新规矩**（与 `worker_leave_unfinished` 同一条）：异常痕迹从前是**闸**
/// （"未收尾"与"已记录的保存失败"各拦一次），今天是**事实**——由开场那一块纸条带给主体，
/// 而**放行不等于没有记录**：准入读数照旧说 `paused` / `unfinished`。
fn worker_must_run(root: &Path) -> i32 {
    let env_dir = root.join("env");
    let sandbox = root.join("sandbox");
    std::fs::create_dir_all(&sandbox).expect("建沙盒");

    let mut env = match Environment::open(&env_dir, agent_a()) {
        Ok(env) => env,
        Err(error) => {
            eprintln!("打开环境失败: {error:?}");
            return 1;
        }
    };

    let mut model = CountingModel::default();
    match run_activity_in_environment(
        &sandbox,
        &mut env,
        EnvironmentActivity {
            declared_agent: &agent_a(),
            run: RunId::new("run-new"),
            limits: Limits::new(4),
            input: RunInput::empty(),
            source: &mut NoContext,
            creation: None,
        },
        &mut model,
    ) {
        // ⚠ **2026-10-06 改（旧规矩 → 新规矩）**：**已记录在案的保存失败暂停也不再拒绝**新活动
        // （见 `composition.rs` 装配点那一句：**门是指令，而世界只该给信息**）。
        // 这一支从"断言被拒、模型零调用"改成"**断言放行、模型真的被调用**"——而**放行不等于
        // 没有记录**：准入读数照旧说着那件事（因此把读数一起打出来）。
        Ok(_) => {
            println!(
                "REFUSAL_REPORT:none|calls={}|admission={}",
                model.calls,
                celira_agent::host_loop::admission_text(&env.admission().expect("准入读数"))
            );
            0
        }
        other => {
            eprintln!("本应放行，却得到 {other:?}");
            1
        }
    }
}

// ---------------------------------------------------------------------------
// 新的一段：开场那一轮带不带"上一段没有收场"那块纸条
// ---------------------------------------------------------------------------

/// 在一个**已经留下未收尾活动**的环境里开**新的一段**（会话路径），回报开场那一轮看见了什么。
///
/// 打出 `NOTE_REPORT:<有没有那块纸条>|<纸条有没有点名那个未收尾的活动>`。
///
/// 为什么必须走**会话路径**：那块 `previous-activity` 纸条是**处境链**的一层，只有
/// `RunSession` 才装它（一次性入口不装处境链，见 `composition.rs` 装配点那一段说明）。
/// 为什么必须**换一个进程**：这一段与上一段之间隔着一次进程消失——那正是这条用例要证明的形状
/// （旧规矩：未收尾拦住准入；新规矩：新的一段照开，而"上一段没有正常收场"由开场那一轮带给主体）。
fn worker_note_in_new_segment(root: &Path) -> i32 {
    let env_dir = root.join("env");
    let sandbox = root.join("sandbox");
    std::fs::create_dir_all(&sandbox).expect("建沙盒");

    let mut env = match Environment::open(&env_dir, agent_a()) {
        Ok(env) => env,
        Err(error) => {
            eprintln!("打开环境失败: {error:?}");
            return 1;
        }
    };

    let declared = agent_a();
    let mut session = match RunSession::open(
        &sandbox,
        &mut env,
        SessionRequest {
            declared_agent: &declared,
            run: RunId::new("run-new-segment"),
            limits: Limits::new(4),
            input: RunInput::empty(),
            clock: ClockHandle::system(),
            context_window: 1_000,
            reading: ContextReading::default(),
            // 夹具不声明模型（这一段由脚本化的执行端驱动）⇒ 开场那一块整块不出场；
            // 可选的那几格一件都不声明（本夹具只用必填那一半）。
            declarations: SessionDeclarations::default(),
        },
    ) {
        Ok(session) => session,
        Err(error) => {
            eprintln!("新的一段开不起来: {error:?}");
            return 1;
        }
    };

    let mut model = NoteRecorder::default();
    match session.drive(&mut model) {
        Ok(_) => {}
        Err(error) => {
            eprintln!("驱动失败: {error:?}");
            return 1;
        }
    }
    if !model.seen {
        eprintln!("模型一次都没被调用");
        return 1;
    }
    println!("NOTE_REPORT:{}|{}", model.saw_note, model.names_run);
    0
}

/// 只看**开场那一轮**的模型：回报处境链里有没有那块 `previous-activity` 纸条。
#[derive(Default)]
struct NoteRecorder {
    /// 已经看过开场那一轮（只看一次）。
    seen: bool,
    /// 开场注入里有没有那块纸条。
    saw_note: bool,
    /// 那块纸条有没有点名**上一段**那个未收尾的活动。
    names_run: bool,
}

impl ModelPort for NoteRecorder {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        if !self.seen {
            self.seen = true;
            for entry in view.trace() {
                let TraceEntry::Injection { stage, batch, .. } = entry else {
                    continue;
                };
                if *stage != InjectionStage::RunStart {
                    continue;
                }
                for block in batch.blocks() {
                    if block.label() == PREVIOUS_ACTIVITY_LABEL {
                        self.saw_note = true;
                        self.names_run = block.text().contains("run-unfinished");
                    }
                }
            }
        }
        Ok(ModelOutput::finish(SubjectEnding::Silent))
    }
}

// ---------------------------------------------------------------------------
// 供另一个进程只读诊断的合成环境
// ---------------------------------------------------------------------------
/// 建立一个多活动的合成环境，并把两份存储的已确认条数打到 stdout。
///
/// 六个活动按写入顺序：两个正常收尾并封存、一个结果未知（系统以 Indeterminate 收场）、
/// 一个“结束已确认但没有封存”、一个**只在事实侧**出现，最后留下一个没有收尾的活动。
///
/// 打出 `SEED_REPORT:facts=<n>|trace=<n>`：另一个进程的只读诊断必须**独立**读到同样的条数。
fn worker_diagnose_seed(root: &Path) -> i32 {
    let env_dir = root.join("env");
    let sandbox = root.join("sandbox");
    std::fs::create_dir_all(&sandbox).expect("建沙盒");

    let mut env = match Environment::create(&env_dir, agent_a()) {
        Ok(env) => env,
        Err(error) => {
            eprintln!("建立环境失败: {error:?}");
            return 1;
        }
    };

    // 1) 一个正常收场并封存的活动：经**真实环境入口**，先展开文件视图再收场。
    let mut first = ScriptedModel::new(vec![
        ModelOutput::act(Action::with_params(
            ActionId::new("open_app"),
            ActionParams::none().with("target", FILE_APP_ID),
        )),
        ModelOutput::finish(SubjectEnding::Completed),
    ]);
    if let Err(error) = run_activity_in_environment(
        &sandbox,
        &mut env,
        EnvironmentActivity {
            declared_agent: &agent_a(),
            run: RunId::new("run-basic-1"),
            limits: Limits::new(6),
            input: RunInput::text("第一次清醒期的输入").expect("在上限内"),
            source: &mut OneShotSource::new("第一次清醒期的注入"),
            creation: None,
        },
        &mut first,
    ) {
        eprintln!("run-basic-1 本应正常收场: {error:?}");
        return 1;
    }

    // 2) 第二个正常收场的活动：结局与上一个不同，便于逐活动核对不串线。
    let mut second = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
    if let Err(error) = run_activity_in_environment(
        &sandbox,
        &mut env,
        EnvironmentActivity {
            declared_agent: &agent_a(),
            run: RunId::new("run-basic-2"),
            limits: Limits::new(4),
            input: RunInput::empty(),
            source: &mut NoContext,
            creation: None,
        },
        &mut second,
    ) {
        eprintln!("run-basic-2 本应正常收场: {error:?}");
        return 1;
    }

    // 3) 结果未知的活动：外部是否真的发生过无法确认，系统以 Indeterminate 收场并封存。
    let mut unknown_model =
        ScriptedModel::new(vec![ModelOutput::act(Action::new(ActionId::new("probe")))]);
    let mut unknown_actions = UnknownActions;
    let mut unknown_agent = Agent::new(agent_a());
    let mut unknown_ledger = Ledger::new(agent_a());
    let mut unknown_commit = EnvironmentCommit::new(&mut env);
    let unknown = run_activity_with(
        &mut unknown_agent,
        RunRequest {
            run: RunId::new("run-unknown"),
            limits: Limits::new(4),
            input: RunInput::empty(),
            source: &mut NoContext,
            commit: &mut unknown_commit,
            cost: &no_cost,
        },
        &mut unknown_model,
        &mut unknown_actions,
        &mut unknown_ledger,
    );
    if unknown.is_err() {
        eprintln!("run-unknown 本应正常收场: {unknown:?}");
        return 1;
    }

    // 4) 结束已确认、封存没确认：终态在盘上，但“到此为止”的封存标记不在。
    // 确认序列是 run-start → input → trace（响应）→ ending → seal：前四次成功，第五次（封存）失败。
    let mut unsealed_model = ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Silent)]);
    let mut unsealed_actions = CountingActions::default();
    let mut unsealed_agent = Agent::new(agent_a());
    let mut unsealed_ledger = Ledger::new(agent_a());
    let mut unsealed_commit = FailAfterCommit::new(EnvironmentCommit::new(&mut env), 4);
    let unsealed = run_activity_with(
        &mut unsealed_agent,
        RunRequest {
            run: RunId::new("run-unsealed"),
            limits: Limits::new(4),
            input: RunInput::empty(),
            source: &mut NoContext,
            commit: &mut unsealed_commit,
            cost: &no_cost,
        },
        &mut unsealed_model,
        &mut unsealed_actions,
        &mut unsealed_ledger,
    );
    if !matches!(unsealed, Err(celira_agent::core::RunError::Commit(_))) {
        eprintln!("run-unsealed 本应在封存时停下: {unsealed:?}");
        return 1;
    }

    // 5) **只在事实侧**出现的活动：经历侧没有任何锚点（跨存储矛盾必须被如实报告）。
    let only_run = RunId::new("run-facts-only");
    if let Err(error) = env.append_fact(
        &only_run,
        &Record::RunStarted {
            agent: agent_a(),
            run: only_run.clone(),
        },
    ) {
        eprintln!("写事实侧孤立活动失败: {error:?}");
        return 1;
    }

    // 6) 最后留下一个**没有收尾**的活动：准入会认出它，而且原因没有被记录成存储故障。
    let mut unfinished_model =
        ScriptedModel::new(vec![ModelOutput::finish(SubjectEnding::Completed)]);
    let mut unfinished_actions = CountingActions::default();
    let mut unfinished_agent = Agent::new(agent_a());
    let mut unfinished_ledger = Ledger::new(agent_a());
    let mut unfinished_commit = FailAfterCommit::new(EnvironmentCommit::new(&mut env), 2);
    let unfinished = run_activity_with(
        &mut unfinished_agent,
        RunRequest {
            run: RunId::new("run-unfinished"),
            limits: Limits::new(8),
            input: RunInput::text("没写完的输入").expect("在上限内"),
            source: &mut OneShotSource::new("没写完的开场注入"),
            commit: &mut unfinished_commit,
            cost: &no_cost,
        },
        &mut unfinished_model,
        &mut unfinished_actions,
        &mut unfinished_ledger,
    );
    if !matches!(unfinished, Err(celira_agent::core::RunError::Commit(_))) {
        eprintln!("run-unfinished 本应在提交时停下: {unfinished:?}");
        return 1;
    }

    let facts = match env.facts() {
        Ok(facts) => facts,
        Err(error) => {
            eprintln!("读回事实失败: {error:?}");
            return 1;
        }
    };
    let trace = match env.trace() {
        Ok(trace) => trace,
        Err(error) => {
            eprintln!("读回经历失败: {error:?}");
            return 1;
        }
    };
    println!("SEED_REPORT:facts={}|trace={}", facts.len(), trace.len());
    0
}

/// 只报“结果未知”的执行端：用来制造“发出过、但外部是否真的发生无法确认”的形状。
struct UnknownActions;

impl ActionPort for UnknownActions {
    fn capabilities(&self, _agent: &AgentId, _run: &RunId) -> Capabilities {
        Capabilities::none()
    }

    // 准入策略：本替身**不**模拟授权判断，一律接纳——它的用途不是检验这一层。
    fn admit(&self, _agent: &AgentId, _run: &RunId, _action: &Action) -> ActionAdmission {
        ActionAdmission::Admitted
    }

    fn execute(
        &mut self,
        _agent: &AgentId,
        _operation: &OperationId,
        _action: &Action,
    ) -> ActionOutcome {
        ActionOutcome::unknown()
    }
}

/// 转交给真实适配器、但在第 `ok_calls + 1` 次调用开始失败的接缝。
///
/// 它只用来制造"写到一半就停"的形状；失败发生之前写下的记录都是真实落盘的。
struct FailAfterCommit<'a> {
    inner: EnvironmentCommit<'a>,
    remaining_ok: usize,
}

impl<'a> FailAfterCommit<'a> {
    fn new(inner: EnvironmentCommit<'a>, ok_calls: usize) -> Self {
        Self {
            inner,
            remaining_ok: ok_calls,
        }
    }

    fn gate(&mut self) -> Result<(), CommitError> {
        if self.remaining_ok == 0 {
            return Err(CommitError::at(CommitStage::Trace));
        }
        self.remaining_ok -= 1;
        Ok(())
    }
}

impl CommitSeam for FailAfterCommit<'_> {
    fn commit_run_start(&mut self, agent: &AgentId, run: &RunId) -> Result<(), CommitError> {
        self.gate()?;
        self.inner.commit_run_start(agent, run)
    }

    fn commit_input(&mut self, run: &RunId, input: &RunInput) -> Result<(), CommitError> {
        self.gate()?;
        self.inner.commit_input(run, input)
    }

    fn commit_trace(
        &mut self,
        run: &RunId,
        turn: Option<Turn>,
        entry: &TraceEntry,
    ) -> Result<(), CommitError> {
        self.gate()?;
        self.inner.commit_trace(run, turn, entry)
    }

    fn commit_action_selected(
        &mut self,
        run: &RunId,
        operation: &OperationId,
        action: &Action,
    ) -> Result<(), CommitError> {
        self.gate()?;
        self.inner.commit_action_selected(run, operation, action)
    }

    fn commit_action_dispatched(
        &mut self,
        run: &RunId,
        operation: &OperationId,
    ) -> Result<(), CommitError> {
        self.gate()?;
        self.inner.commit_action_dispatched(run, operation)
    }

    fn commit_action_result(
        &mut self,
        run: &RunId,
        operation: &OperationId,
        outcome: &ActionOutcome,
    ) -> Result<(), CommitError> {
        self.gate()?;
        self.inner.commit_action_result(run, operation, outcome)
    }

    fn commit_ending(
        &mut self,
        run: &RunId,
        outcome: Outcome,
        cost: RunCost,
    ) -> Result<(), CommitError> {
        self.gate()?;
        self.inner.commit_ending(run, outcome, cost)
    }

    fn seal(&mut self, run: &RunId) -> Result<(), CommitError> {
        self.gate()?;
        self.inner.seal(run)
    }
}

// ---------------------------------------------------------------------------
// 测试替身
// ---------------------------------------------------------------------------

/// Run 级注入一次合成正文的来源。
struct OneShotSource {
    text: String,
}

impl OneShotSource {
    fn new(text: &str) -> Self {
        Self {
            text: text.to_string(),
        }
    }
}

impl ContextSource for OneShotSource {
    fn on_run_start(&mut self, _context: &InjectionContext) -> ContentBatch {
        ContentBatch::of(vec![ContentBlock::new("note", self.text.clone())]).expect("在上限内")
    }

    fn before_turn(&mut self, _context: &InjectionContext) -> ContentBatch {
        ContentBatch::empty()
    }
}

/// 按脚本作答的模型。
struct ScriptedModel {
    script: VecDeque<ModelOutput>,
    /// 被调用了几次：用来断言"该被拒绝时一次都没被叫到"。
    calls: usize,
}

impl ScriptedModel {
    fn new(script: Vec<ModelOutput>) -> Self {
        Self {
            script: VecDeque::from(script),
            calls: 0,
        }
    }

    fn calls(&self) -> usize {
        self.calls
    }
}

impl ModelPort for ScriptedModel {
    fn complete(&mut self, _view: &View) -> Result<ModelOutput, ModelError> {
        self.calls += 1;
        self.script.pop_front().ok_or(scripted_model_failure())
    }
}

/// B 用的模型：**照真实可见的能力目录**决定下一步，并核对查回的内容。
///
/// 这是本测试最重要的一处：如果 B 只是照剧本盲目发动作，就证明不了"能力目录真的可见"。
/// 所以它每一步都先读 `view.capabilities()`，只有目录里出现对应能力才发那个动作。
/// 它**只用通用文件动作**（`files_list` / `files_read`），不调用任何历史专用动作：
/// 旧经历在同一个只读文件视图里是保留逻辑目录 [`HISTORY_DIR`] 下的派生文件。
///
/// 它还顺便盯住"不自动重放 A 的行动"：B 第一次清醒、还没做任何事时，视野里**不得**已经
/// 有观察；跑完时观察条数必须**正好**等于 B 自己发出的动作数——多出来的就只能是别人塞的。
#[derive(Default)]
struct InspectingModel {
    first_view_had_a_text: bool,
    saw_list: bool,
    saw_read: bool,
    read_back_contains_a_text: bool,
    read_back_had_multiple: bool,
    first_view_had_no_observations: bool,
    observations_match_own_actions: bool,
    step: usize,
    /// 从真实目录列表里读出来的活动目录名与分段名（每一步都来自能力返回，不写死）。
    run_dir: Option<String>,
    segment: Option<String>,
}

/// 这个模型一共会自己发出几个**业务**动作（展开 → 列历史目录 → 列活动目录 → 读一段）。
///
/// TASK_69：调用之前"先看格式"那两轮（`inspect_action`）不计入——它们是我自己发的动作，
/// 与"有没有自动重放 A 的行动"无关（见 `_` 分支里的对账）。
const B_OWN_ACTIONS: usize = 4;

/// 通用文件动作的标识（由文件视图 App 声明；夹具按名字引用）。
const LIST: &str = "files_list";
const READ_TEXT: &str = "files_read";
/// 看格式那个动作的标识（OS 自有动词；夹具按名字引用）。
const INSPECT: &str = "inspect_action";

/// 从一段目录列表正文里机械地取第一个某种种类的条目名。
fn first_entry_of_kind(text: &str, kind: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (name, entry_kind) = line.split_once('\t')?;
        (entry_kind == kind).then(|| name.to_string())
    })
}

fn all_view_text(view: &View) -> String {
    view.trace()
        .iter()
        .flat_map(|entry| match entry {
            TraceEntry::Injection { batch, .. } => batch
                .blocks()
                .iter()
                .map(|block| block.text().to_string())
                .collect::<Vec<_>>(),
            TraceEntry::Refused { .. } => Vec::new(),
            // 模型自己说过的话不是"视野文本"：它由结构化历史承载，这份扁平文本只汇总注入与行动结果。
            TraceEntry::ModelResponse(_) => Vec::new(),
            TraceEntry::Observation(observation) => observation
                .outcome()
                .content()
                .iter()
                .map(|block| block.text().to_string())
                .collect::<Vec<_>>(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 最后一条观察返回的正文（连续几步里，"刚刚读到了什么"）。
fn last_observation_text(view: &View) -> String {
    view.observed()
        .last()
        .map(|observation| {
            observation
                .outcome()
                .content()
                .iter()
                .map(|block| block.text())
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn capability_ids(view: &View) -> Vec<String> {
    view.capabilities()
        .items()
        .iter()
        .map(|capability| capability.id().as_str().to_string())
        .collect()
}

fn list_path(path: &str) -> Action {
    Action::with_params(ActionId::new(LIST), ActionParams::none().with("path", path))
}

fn read_path(path: &str) -> Action {
    Action::with_params(
        ActionId::new(READ_TEXT),
        ActionParams::none().with("path", path),
    )
}

/// 看一个动作的格式卡（TASK_69：调用之前"先看格式"那一轮）。
fn inspect(name: &str) -> Action {
    Action::with_params(
        ActionId::new(INSPECT),
        ActionParams::none().with("action", name),
    )
}

impl ModelPort for InspectingModel {
    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        let ids = capability_ids(view);

        match self.step {
            0 => {
                // 第一轮：A 的正文**不得**出现在视野里，而且 B 还没做过任何事。
                let text = all_view_text(view);
                self.first_view_had_a_text = text.contains(A_SAVED_TEXT);
                self.first_view_had_no_observations = view.observed().is_empty();
                self.step = 1;
                // 先展开文件视图——这是文件能力（含派生经历目录）出现的前提。
                Ok(ModelOutput::act(Action::with_params(
                    ActionId::new("open_app"),
                    ActionParams::none().with("target", FILE_APP_ID),
                )))
            }
            1 => {
                // 第二轮：两项通用文件能力必须**真的**出现在目录里。
                self.saw_list = ids.iter().any(|id| id == LIST);
                self.saw_read = ids.iter().any(|id| id == READ_TEXT);
                if !self.saw_list || !self.saw_read {
                    return Err(scripted_model_failure());
                }
                self.step = 2;
                // TASK_69：**调用之前先看格式**——没查过的动作在准入面就被拒（拒绝的语义是
                // "先看格式"，不是"你没有这个能力"）。两项一起查（一轮多动作），与真实包装器
                // "整批先看"的行为同形。
                ModelOutput::act_many(vec![inspect(LIST), inspect(READ_TEXT)])
                    .ok_or(scripted_model_failure())
            }
            2 => {
                // 第三轮：列历史目录。
                self.step = 3;
                Ok(ModelOutput::act(list_path(HISTORY_DIR)))
            }
            3 => {
                // 第四轮：上一轮必须真的列出了一个活动目录（名字来自能力返回）。
                let text = last_observation_text(view);
                let Some(run_dir) = first_entry_of_kind(&text, "dir") else {
                    return Err(scripted_model_failure());
                };
                self.run_dir = Some(run_dir.clone());
                self.step = 4;
                Ok(ModelOutput::act(list_path(&format!(
                    "{HISTORY_DIR}/{run_dir}"
                ))))
            }
            4 => {
                // 第五轮：活动目录里必须有分段文件，读第一段。
                let text = last_observation_text(view);
                let Some(segment) = first_entry_of_kind(&text, "file") else {
                    return Err(scripted_model_failure());
                };
                let run_dir = self.run_dir.clone().ok_or(scripted_model_failure())?;
                self.segment = Some(segment.clone());
                self.step = 5;
                Ok(ModelOutput::act(read_path(&format!(
                    "{HISTORY_DIR}/{run_dir}/{segment}"
                ))))
            }
            _ => {
                // 第六轮：核对刚刚查回来的内容，并核对观察条数。
                let text = last_observation_text(view);
                self.read_back_contains_a_text = text.contains(A_SAVED_TEXT);
                self.read_back_had_multiple = text.contains("run = run-a")
                    && text.lines().filter(|line| line.starts_with('#')).count() >= 2;
                // 按**业务动作**对账（滤掉"先看格式"那两轮）：这一条要证明的是
                // "不自动重放 A 的行动"；查阅格式是我自己发的动作，与 A 无关。
                self.observations_match_own_actions = view
                    .observed()
                    .iter()
                    .filter(|observation| observation.action().as_str() != INSPECT)
                    .count()
                    == B_OWN_ACTIONS;
                Ok(ModelOutput::finish(SubjectEnding::Completed))
            }
        }
    }
}

/// 只数调用次数的模型。
#[derive(Default)]
struct CountingModel {
    calls: usize,
}

impl ModelPort for CountingModel {
    fn complete(&mut self, _view: &View) -> Result<ModelOutput, ModelError> {
        self.calls += 1;
        Ok(ModelOutput::finish(SubjectEnding::Silent))
    }
}

/// 只数调用次数的执行端。
#[derive(Default)]
struct CountingActions {
    calls: usize,
}

impl ActionPort for CountingActions {
    fn capabilities(&self, _agent: &AgentId, _run: &RunId) -> Capabilities {
        Capabilities::none()
    }

    // 准入策略：本替身**不**模拟授权判断，一律接纳——它的用途不是检验这一层。
    fn admit(&self, _agent: &AgentId, _run: &RunId, _action: &Action) -> ActionAdmission {
        ActionAdmission::Admitted
    }

    fn execute(
        &mut self,
        _agent: &AgentId,
        _operation: &OperationId,
        _action: &Action,
    ) -> ActionOutcome {
        self.calls += 1;
        ActionOutcome::success(vec![ContentBlock::new("result", "不该发生")])
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
