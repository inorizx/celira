//! TASK_11 验收：**一轮多个行动**——模型一次请求几个动作，就忠实执行几个。
//!
//! 这一份文件走**生产路径**（真实环境 + `RunSession`），只把模型换成脚本替身。它证明四件事：
//!
//! 1. 一次回复里的多个动作**都执行**，顺序与模型给出的顺序一致，各自有自己的操作身份（子序号）；
//! 2. 其中某个动作**被准入拒绝**⇒ 它零操作身份、零行动事实，**其余动作照常**；
//! 3. 其中某个动作**明确失败**⇒ 那不是活动失败，**其余动作照常**；
//! 4. 其中某个动作**结果无法确认**⇒ 这一轮**剩下的动作一个都不发出**，并沿用既有规则收场。
//!
//! 还核两件盘上的事：**第 0 个动作不写子序号字段**（单动作轮次与从前逐字节相同），以及
//! **跳号是合法的**（第 0 个被拒只剩第 1 个有事实时，环境仍然读得出来）。

use std::collections::VecDeque;
use std::path::PathBuf;

use celira_agent::clock::ClockHandle;
use celira_agent::composition::{RunSession, SessionDeclarations, SessionDrive, SessionRequest};
use celira_agent::context_reading::ContextReading;
use celira_agent::core::{
    Action, ActionId, ActionParams, Limits, ModelError, ModelFailureClass, ModelOutput, ModelPort,
    RunInput, View,
};
use celira_agent::creation_grant::CreationGrant;
use celira_agent::file_app::FILE_APP_ID;
use celira_agent::foundation::{AgentId, Outcome, RunId, SubjectEnding, SystemEnding};
use celira_agent::recovery;
use celira_agent::store::Environment;

mod common;

use common::InspectsFirst;

const AGENT: &str = "agent-multi";
const RUN: &str = "run-multi";

/// 用例装配用的**时间来源**：固定钟——离线、可逐字节断言。
///
/// 生产装配给真实钟（`agent_run`），用例一律给固定钟：状态行的 `time` 因此在每次运行里都是同一个
/// 读数，任何"同输入同输出"的断言都不会被墙上时钟搅动。它只换时间来源，**不换**装配代码——
/// 这里与生产走的是同一个 `RunSession::open`、同一个格式化点。
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
/// 授权创建的目录（相对资料根的单层目录名）。
const GRANT_DIR: &str = "workman";

// ---------------------------------------------------------------------------
// 自建的独立测试目录与模型替身
// ---------------------------------------------------------------------------

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
            "celira-task11-multi-{label}-{}-{n}",
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
        std::fs::create_dir_all(materials.join(GRANT_DIR)).expect("建资料目录");
        std::fs::write(materials.join("alpha.txt"), "alpha").expect("写合成文件");
        materials
    }

    fn env_root(&self) -> PathBuf {
        self.path.join("env")
    }

    fn trace_path(&self) -> PathBuf {
        self.env_root().join("trace.ndjson")
    }

    fn facts_path(&self) -> PathBuf {
        self.env_root().join("facts.ndjson")
    }

    fn granted(&self, name: &str) -> PathBuf {
        self.materials().join(GRANT_DIR).join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn agent() -> AgentId {
    AgentId::new(AGENT)
}

fn run_id() -> RunId {
    RunId::new(RUN)
}

fn file_grant() -> CreationGrant {
    CreationGrant::new(agent(), run_id(), GRANT_DIR).expect("授权形状合法")
}

/// 按脚本作答的模型替身。
struct ScriptedModel {
    script: VecDeque<ModelOutput>,
    calls: usize,
}

impl ScriptedModel {
    fn new(script: Vec<ModelOutput>) -> Self {
        Self {
            script: VecDeque::from(script),
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

fn action(id: &str, params: &[(&str, &str)]) -> Action {
    let mut built = ActionParams::none();
    for (name, value) in params {
        built = built.with(*name, *value);
    }
    Action::with_params(ActionId::new(id), built)
}

fn expand(app_id: &str) -> ModelOutput {
    ModelOutput::act(action("open_app", &[("target", app_id)]))
}

fn list(path: &str) -> Action {
    action("files_list", &[("path", path)])
}

fn create_text(name: &str, body: &str) -> Action {
    action("files_create", &[("name", name), ("text", body)])
}

fn verify_text(name: &str, body: &str) -> Action {
    action("files_verify_text", &[("name", name), ("text", body)])
}

/// **一轮多个行动**：一次回复请求这几个动作，顺序就是给定的顺序。
fn batch(actions: Vec<Action>) -> ModelOutput {
    ModelOutput::act_many(actions).expect("至少一个动作")
}

fn finish(ending: SubjectEnding) -> ModelOutput {
    ModelOutput::finish(ending)
}

// ---------------------------------------------------------------------------
// 跑一次活动 + 读盘
// ---------------------------------------------------------------------------

/// 跑一次活动（生产路径：真环境 + 会话），返回这次驱动的结果。
fn drive(dir: &TempDir, script: Vec<ModelOutput>, with_grant: bool) -> (SessionDrive, usize) {
    let materials = dir.materials();
    let grant = file_grant();
    let mut environment = Environment::create(&dir.env_root(), agent()).expect("建立环境");
    let declared = agent();
    let mut session = RunSession::open(
        &materials,
        &mut environment,
        SessionRequest {
            declarations: SessionDeclarations {
                creation: if with_grant {
                    Some(grant.clone())
                } else {
                    None
                },
                // 其余几格一件都不声明（`Default` 就是"全都没声明"）——**逐格写全会让"加一格"
                // 重新变成二十处机械改动**，那正是 §3.49 还掉的那笔债。
                ..Default::default()
            },
            declared_agent: &declared,
            run: run_id(),
            limits: Limits::new(6),
            input: RunInput::text("开场输入").expect("在上限内"),
            clock: fixed_clock(),
            context_window: CONTEXT_WINDOW,
            reading: reading_cell(),
        },
    )
    .expect("会话可以打开");
    let mut model = InspectsFirst::new(ScriptedModel::new(script));
    let driven = session.drive(&mut model).expect("驱动不应失败");
    let calls = model.calls();
    drop(session);
    drop(environment);
    (driven, calls)
}

/// 一行的**正文**（存储行尾带校验和；这些用例读事实做断言，先把校验和剥掉）。
fn body_of(line: &str) -> &str {
    match line.rfind('\t') {
        Some(index) => &line[..index],
        None => line,
    }
}

fn facts_of(dir: &TempDir) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.facts_path())
        .expect("读事实存储")
        .lines()
        .map(|line| serde_json::from_str(body_of(line)).expect("每一行都是合法 JSON"))
        .collect()
}

/// 某一轮里某一类事实的子序号清单（`None` = 这一行没写子序号字段 = 第 0 个）。
///
/// 返回的是 `Option<u64>` 而不是 `u64`：**"没写字段"本身就是要被断言的事实**。
fn indices_of(dir: &TempDir, fact: &str, turn: u64) -> Vec<Option<u64>> {
    facts_of(dir)
        .iter()
        .filter(|row| row.get("fact").and_then(|v| v.as_str()) == Some(fact))
        .filter(|row| row.get("turn").and_then(|v| v.as_u64()) == Some(turn))
        .map(|row| row.get("index").and_then(|v| v.as_u64()))
        .collect()
}

fn trace_text(dir: &TempDir) -> String {
    std::fs::read_to_string(dir.trace_path()).expect("读经历存储")
}

/// 某一轮里"行动有结果"的经历条数。
///
/// 注意**展开也是一次行动**：它同样留下一条观察，所以"这一轮有几个动作"必须按轮次看。
fn observed_in_turn(dir: &TempDir, turn: u64) -> usize {
    trace_text(dir)
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(body_of(line)).expect("合法 JSON"))
        .filter(|row| row.get("kind").and_then(|v| v.as_str()) == Some("observed"))
        .filter(|row| row.get("turn").and_then(|v| v.as_u64()) == Some(turn))
        .count()
}

/// **业务动作**落在哪一轮：`actions` = 那一轮要求的业务动作个数。
///
/// TASK_69：轮次号不能再写死——主体开场那两批 `inspect_action`（"先看格式"）各占一轮，
/// 业务动作因此整体后移；按"这一轮要了几个业务动作"查回来，就与轮数解耦。
fn business_turn(dir: &TempDir, actions: usize) -> u64 {
    trace_text(dir)
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(body_of(line)).expect("合法 JSON"))
        .filter(|row| row.get("kind").and_then(|v| v.as_str()) == Some("model-response"))
        .filter(|row| row.get("decision").and_then(|v| v.as_str()) == Some("act"))
        .filter(|row| row.get("action").and_then(|v| v.as_str()) != Some("inspect_action"))
        .find(|row| {
            let extra = row
                .get("extra-actions")
                .and_then(|v| v.as_array())
                .map(|list| list.len())
                .unwrap_or(0);
            1 + extra == actions
        })
        .and_then(|row| row.get("turn").and_then(|v| v.as_u64()))
        .unwrap_or_else(|| panic!("找得到'要求 {actions} 个业务动作'的那一轮"))
}

/// 这一轮模型响应那一条记录（解析成 JSON）。
fn model_response(dir: &TempDir, turn: u64) -> serde_json::Value {
    trace_text(dir)
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(body_of(line)).expect("合法 JSON"))
        .find(|row| {
            row.get("kind").and_then(|v| v.as_str()) == Some("model-response")
                && row.get("turn").and_then(|v| v.as_u64()) == Some(turn)
        })
        .expect("这一轮应当有一条模型响应记录")
}

// ---------------------------------------------------------------------------
// 1. 一轮两个只读动作：都执行，顺序一致，身份各自独立
// ---------------------------------------------------------------------------

#[test]
fn two_read_actions_in_one_reply_both_execute_in_order() {
    let dir = TempDir::new("two-reads");
    let (driven, _) = drive(
        &dir,
        vec![
            expand(FILE_APP_ID),
            batch(vec![list(""), list(GRANT_DIR)]),
            finish(SubjectEnding::Completed),
        ],
        false,
    );
    assert_eq!(
        driven,
        SessionDrive::Ended(Outcome::Subject(SubjectEnding::Completed)),
        "一轮多个动作不该变成模型失败"
    );

    // 两个业务轮：一轮展开（一个动作）、一轮两个动作。
    // TASK_69：轮次号按"这一轮要了几个业务动作"查回来，不写死——主体开场那两批"先看格式"各占一轮。
    let single = business_turn(&dir, 1);
    let pair = business_turn(&dir, 2);
    for fact in ["action-prepared", "action-dispatched", "action-result"] {
        assert_eq!(
            indices_of(&dir, fact, pair),
            vec![None, Some(1)],
            "{fact}：两个动作各一条，第 0 个**不写**子序号字段、第 1 个写 1"
        );
    }
    assert_eq!(
        indices_of(&dir, "action-prepared", single),
        vec![None],
        "只有一个动作的那一轮照旧不写字段（与从前逐字节相同）"
    );

    // 一条模型响应，带着**两个有序请求**。
    let response = model_response(&dir, pair);
    assert_eq!(response["decision"], "act");
    assert_eq!(response["action"], "files_list", "第 0 个沿用既有字段");
    let extra = response["extra-actions"]
        .as_array()
        .expect("第 1 个请求在 extra-actions 里");
    assert_eq!(extra.len(), 1, "只有第 1 个在追加字段里");
    assert_eq!(extra[0]["action"], "files_list");
    assert_eq!(
        extra[0]["params"][0]["text"], GRANT_DIR,
        "参数跟着它自己的那一项，顺序不打乱"
    );

    // 两条观察，顺序与请求一致。
    assert_eq!(
        observed_in_turn(&dir, pair),
        2,
        "这一轮的两个动作各留下一条观察"
    );
    assert_eq!(
        observed_in_turn(&dir, single),
        1,
        "只有一个动作的那一轮也是一次行动（它同样有观察）"
    );
}

// ---------------------------------------------------------------------------
// 2. 被拒的动作不阻断同一轮其余动作（而且**跳号**仍然读得出来）
// ---------------------------------------------------------------------------

#[test]
fn a_refused_action_does_not_stop_the_rest_of_the_turn() {
    let dir = TempDir::new("refused-then-read");
    let (driven, _) = drive(
        &dir,
        vec![
            expand(FILE_APP_ID),
            // 没有创建授权 ⇒ `files_create` 在归属索引里没有主人 ⇒ 被拒；
            // 后面的只读动作照常执行。
            batch(vec![create_text("x.txt", "body"), list("")]),
            finish(SubjectEnding::Completed),
        ],
        false,
    );
    assert_eq!(
        driven,
        SessionDrive::Ended(Outcome::Subject(SubjectEnding::Completed)),
        "一次准入拒绝不升级成活动失败"
    );

    let trace = trace_text(&dir);
    assert_eq!(
        trace.matches("admission-refused").count(),
        1,
        "被拒的动作只留一条拒绝经历"
    );
    // TASK_69：轮次号按"这一轮要了几个业务动作"查回来（不写死）。
    let pair = business_turn(&dir, 2);
    assert_eq!(
        observed_in_turn(&dir, pair),
        1,
        "同一轮里另一个动作照常执行（只有它留下观察）"
    );

    // 第 0 个被拒 ⇒ 事实里只剩第 1 个：**跳号是合法的**。
    assert_eq!(
        indices_of(&dir, "action-prepared", pair),
        vec![Some(1)],
        "只有被派发的那个动作有事实"
    );
    assert!(!dir.granted("x.txt").exists(), "被拒的动作不得触碰任何 App");

    // 这条环境仍然**读得出来**（跳号不是损坏）：判定跑得完，而且结论是"已收场"。
    let environment = Environment::open(&dir.env_root(), agent()).expect("重开环境");
    let state = recovery::inspect(&environment.reader(), &run_id());
    assert_eq!(
        state.as_str(),
        "ended",
        "跳号的环境必须照样可读（不是 refused）"
    );
}

// ---------------------------------------------------------------------------
// 3. 明确失败不阻断同一轮其余动作
// ---------------------------------------------------------------------------

#[test]
fn a_failed_action_does_not_stop_the_rest_of_the_turn() {
    let dir = TempDir::new("failure-then-write");
    let (driven, _) = drive(
        &dir,
        vec![
            expand(FILE_APP_ID),
            // 同一个名字两次：第 0 个抢到（成功），第 1 个必然失败（已存在）。
            // 失败**不是**活动失败，所以这一轮两个动作都留下事实。
            batch(vec![
                create_text("same.txt", "v"),
                create_text("same.txt", "v"),
            ]),
            finish(SubjectEnding::Completed),
        ],
        true,
    );
    assert_eq!(
        driven,
        SessionDrive::Ended(Outcome::Subject(SubjectEnding::Completed))
    );

    // TASK_69：轮次号按"这一轮要了几个业务动作"查回来（不写死）。
    let pair = business_turn(&dir, 2);
    for fact in ["action-prepared", "action-dispatched", "action-result"] {
        assert_eq!(
            indices_of(&dir, fact, pair),
            vec![None, Some(1)],
            "{fact}：两个动作都留下了自己的事实"
        );
    }
    let facts = facts_of(&dir);
    let states: Vec<&str> = facts
        .iter()
        .filter(|row| row.get("fact").and_then(|v| v.as_str()) == Some("action-result"))
        .filter(|row| row.get("turn").and_then(|v| v.as_u64()) == Some(pair))
        .map(|row| row.get("state").and_then(|v| v.as_str()).unwrap_or("?"))
        .collect();
    assert_eq!(
        states,
        vec!["success", "failure"],
        "第 0 个成功、第 1 个明确失败；失败是**结果**，不是活动失败"
    );
}

// ---------------------------------------------------------------------------
// 4. 结果无法确认 ⇒ 这一轮剩下的动作一个都不发出
// ---------------------------------------------------------------------------

#[test]
fn an_unconfirmed_result_stops_the_rest_of_the_turn() {
    let dir = TempDir::new("unknown-stops-turn");
    // 盘上内容与复核参数不一致 ⇒ 复核既不能报成功、也不能报失败，只能报"无法确认"。
    std::fs::create_dir_all(dir.granted("draft.txt").parent().expect("有父目录")).expect("建目录");
    std::fs::write(dir.granted("draft.txt"), "被人改过的正文").expect("写盘上内容");

    let (driven, _) = drive(
        &dir,
        vec![
            expand(FILE_APP_ID),
            batch(vec![
                verify_text("draft.txt", "当初的正文"),
                create_text("later.txt", "v2"),
            ]),
            finish(SubjectEnding::Completed),
        ],
        true,
    );
    assert_eq!(
        driven,
        SessionDrive::Ended(Outcome::System(SystemEnding::Indeterminate)),
        "结果无法确认时按既有规则收场"
    );

    // 第 1 个（复核）有事实；第 2 个**一条都没有**——不准备、不派发。
    // TASK_69：轮次号按"这一轮要了几个业务动作"查回来（不写死）。
    let pair = business_turn(&dir, 2);
    for fact in ["action-prepared", "action-dispatched", "action-result"] {
        assert_eq!(
            indices_of(&dir, fact, pair),
            vec![None],
            "{fact}：这一轮只该有第 0 个动作的事实"
        );
    }
    assert!(
        !dir.granted("later.txt").exists(),
        "这一轮剩下的动作一个都不许发出"
    );
}

// ---------------------------------------------------------------------------
// 5. 子序号的形状自洽检查**可判错**：重复或倒退的序号不是本写入者写出的形状
// ---------------------------------------------------------------------------

/// 把某一轮最后一条 `action-prepared` 的序号改成与前面**重复**：读取必须明确拒绝。
///
/// 它守的是"序号是身份的一部分"：两个不同请求塌成同一个身份时，**不许猜**，也不许静默合并。
#[test]
fn a_duplicated_sub_index_is_refused_when_reading() {
    let dir = TempDir::new("duplicated-index");
    let (driven, _) = drive(
        &dir,
        vec![
            expand(FILE_APP_ID),
            batch(vec![list(""), list(GRANT_DIR)]),
            finish(SubjectEnding::Completed),
        ],
        false,
    );
    assert_eq!(
        driven,
        SessionDrive::Ended(Outcome::Subject(SubjectEnding::Completed))
    );
    // 正常读得出来（对照组）。
    {
        let environment = Environment::open(&dir.env_root(), agent()).expect("重开环境");
        assert_eq!(
            recovery::inspect(&environment.reader(), &run_id()).as_str(),
            "ended",
            "改动之前，这份环境是健康读得出来的"
        );
    }

    // 把第 2 轮第 1 个动作（写 `index: 1` 的那一条）的序号改成 `0` ⇒ 与第 0 个重复。
    let rows: Vec<serde_json::Value> = facts_of(&dir);
    let mut text = String::new();
    let mut changed = false;
    for row in &rows {
        let mut row = row.clone();
        if !changed
            && row.get("fact").and_then(|v| v.as_str()) == Some("action-prepared")
            && row.get("index").and_then(|v| v.as_u64()) == Some(1)
        {
            row["index"] = serde_json::Value::from(0u64);
            changed = true;
        }
        text.push_str(&row.to_string());
        text.push('\n');
    }
    assert!(changed, "夹具前提：确实有一条写着 index 1 的 prepared 事实");
    std::fs::write(dir.facts_path(), text).expect("写回事实存储");

    let environment = Environment::open(&dir.env_root(), agent()).expect("重开环境");
    let state = recovery::inspect(&environment.reader(), &run_id());
    assert_eq!(
        state.as_str(),
        "refused",
        "重复的序号是读不出来的形状，不许猜"
    );
    assert_eq!(
        state.reason(),
        Some("fact-row:action-index-out-of-order"),
        "原因必须指出是子序号的问题"
    );
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
