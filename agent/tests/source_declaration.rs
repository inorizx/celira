//! **来源声明**的行为证据（本文件只读代码、不碰网络、不碰 im）。
//!
//! 这一格回答的是**"这段回执是谁说的"**——它不是可信度评级，也不是权限。本文件要钉住四件事：
//!
//! 1. **正常**：一台真实 App 的每个能力都查得出自己的档位，且**按能力分别声明**（不是整台一个档）；
//! 2. **拒绝**：多声明一个不属于自己的行动、漏声明一个自己的能力，两种都在装配期失败；
//! 3. **失败**：漏项时**点名漏的是哪一个**，不是笼统报一句"声明不齐"；
//! 4. **未知**：这次装配里没有的能力查出来是 `None`（**不知道**），绝不兜底成一个档——
//!    "这段没有署名"与"我不知道这段从哪来"必须可判错地分开。
//!
//! 另外钉住 OS 自己的五个动词：它们**不归任何 App**，因此来源由 OS 自己那份清单声明，
//! 不许在表里留下"系统自己的动作来源不知道"这种读数。

use std::path::PathBuf;

use celira_agent::app_contract::{
    AppPort, AssemblyFailure, SourceClass, SourceDeclaration, SourceTable,
};
use celira_agent::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionPort, Capabilities, Capability,
};
use celira_agent::foundation::{AgentId, OperationId, RunId};
use celira_agent::os::MinimalOs;
use celira_agent::timer_app::TimerApp;

// ---------------------------------------------------------------------------
// 替身：只声明，不做事
// ---------------------------------------------------------------------------

/// 一个只会声明、不会执行的替身 App。
struct Declaring {
    id: &'static str,
    verbs: Vec<&'static str>,
    sources: Vec<(&'static str, SourceClass)>,
}

impl Declaring {
    fn new(
        id: &'static str,
        verbs: &[&'static str],
        sources: &[(&'static str, SourceClass)],
    ) -> Self {
        Self {
            id,
            verbs: verbs.to_vec(),
            sources: sources.to_vec(),
        }
    }
}

impl ActionPort for Declaring {
    fn capabilities(&self, _agent: &AgentId, _run: &RunId) -> Capabilities {
        AppPort::declared_capabilities(self)
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
        ActionOutcome::failure("替身不执行任何东西")
    }
}

impl AppPort for Declaring {
    fn app_id(&self) -> &str {
        self.id
    }

    fn description(&self) -> &str {
        "来源声明替身"
    }

    fn declared_capabilities(&self) -> Capabilities {
        Capabilities::of(
            self.verbs
                .iter()
                .map(|verb| Capability::new(ActionId::new(*verb), "替身动词", vec![]))
                .collect(),
        )
    }

    fn declared_sources(&self) -> Vec<SourceDeclaration> {
        self.sources
            .iter()
            .map(|(action, class)| SourceDeclaration::new(ActionId::new(*action), *class))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// 自建临时目录（TimerApp 要一个环境根）
// ---------------------------------------------------------------------------

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(label: &str) -> Self {
        let unique = format!(
            "celira-source-declaration-{}-{}-{}",
            std::process::id(),
            label,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("时钟可用")
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&root).expect("建临时目录");
        Self { root }
    }
}

// ---------------------------------------------------------------------------
// 正常
// ---------------------------------------------------------------------------

/// 一台真实 App 的**每个能力**都查得出自己的档位，而且**按能力分别声明**。
///
/// `timer` 正好是混合的一台：清单读的是**自己约下的时刻**，两个写动词回报的是
/// **系统对这次动作的确认**。如果来源被做成"整台一个档"，这条用例会立刻失败。
#[test]
fn a_real_app_declares_one_class_per_capability() {
    let sandbox = Sandbox::new("timer");
    let os = MinimalOs::assemble(vec![Box::new(TimerApp::new(&sandbox.root))]).expect("可以装配");
    let sources = os.sources();

    assert_eq!(
        sources.class_if_assembled("timer_list"),
        Some(SourceClass::SelfAuthored)
    );
    assert_eq!(
        sources.class_if_assembled("timer_set"),
        Some(SourceClass::System)
    );
    assert_eq!(
        sources.class_if_assembled("timer_cancel"),
        Some(SourceClass::System)
    );
    assert!(!sources.is_empty());
}

/// OS 自己的五个动词**不归任何 App**，但它们在表里，而且都是 `system`。
///
/// 这一条防的是"系统自己的动作来源不知道"：那五个动词是主体能点的，回执进主体的视野。
#[test]
fn the_os_own_verbs_are_declared_by_the_os_itself() {
    let sandbox = Sandbox::new("os-owned");
    let os = MinimalOs::assemble(vec![Box::new(TimerApp::new(&sandbox.root))]).expect("可以装配");
    let sources = os.sources();

    for action in [
        "open_app",
        "inspect_action",
        "pin_action",
        "unpin_action",
        "write_self_concept",
    ] {
        assert_eq!(
            sources.class_if_assembled(action),
            Some(SourceClass::System),
            "{action} 应当由 OS 自己声明为 system"
        );
    }
}

/// 稳定字符串是一份**词汇表**：五个取值逐字节稳定，谁改谁负责。
#[test]
fn the_five_classes_are_stable_strings() {
    assert_eq!(SourceClass::SelfAuthored.as_str(), "self-authored");
    assert_eq!(SourceClass::System.as_str(), "system");
    assert_eq!(SourceClass::Environment.as_str(), "environment");
    assert_eq!(SourceClass::WorldSigned.as_str(), "world-signed");
    assert_eq!(SourceClass::WorldUnsigned.as_str(), "world-unsigned");
}

/// 读数按行动标识排序，顺序不承载任何判断。
#[test]
fn the_reading_is_sorted_by_action_id() {
    let sandbox = Sandbox::new("sorted");
    let os = MinimalOs::assemble(vec![Box::new(TimerApp::new(&sandbox.root))]).expect("可以装配");
    let actions: Vec<&str> = os.sources().actions().into_iter().map(|(a, _)| a).collect();
    let mut expected = actions.clone();
    expected.sort_unstable();
    assert_eq!(actions, expected);
}

// ---------------------------------------------------------------------------
// 未知（不是"未署名"）
// ---------------------------------------------------------------------------

/// 这次装配里**没有**的能力查出来是 `None`——**不知道**，不是一个档。
///
/// "这段没有署名"（`world-unsigned`）与"我不知道这段从哪来"（`None`）是两句不同的事实。
/// 把后者说成前者，就是**系统制造的混淆**——本用例存在的全部理由。
#[test]
fn an_unassembled_capability_is_unknown_and_never_falls_back_to_a_class() {
    let sandbox = Sandbox::new("unknown");
    let os = MinimalOs::assemble(vec![Box::new(TimerApp::new(&sandbox.root))]).expect("可以装配");

    assert_eq!(os.sources().class_if_assembled("web_fetch"), None);
    assert_eq!(os.sources().class_if_assembled("terminal_run"), None);
    assert_eq!(os.sources().class_if_assembled("im_read_history"), None);
}

// ---------------------------------------------------------------------------
// 拒绝
// ---------------------------------------------------------------------------

/// **多声明**了一个不属于自己的行动 ⇒ 装配期拒绝。
#[test]
fn declaring_someone_elses_action_is_refused() {
    let app = Declaring::new(
        "borrower",
        &["mine"],
        &[
            ("mine", SourceClass::System),
            ("not-mine", SourceClass::SelfAuthored),
        ],
    );
    let failure = MinimalOs::assemble(vec![Box::new(app)]).expect_err("必须拒绝");
    assert_eq!(
        failure,
        AssemblyFailure::UnknownDeclaredSource {
            app: "borrower".to_string(),
            action: "not-mine".to_string(),
        }
    );
    assert_eq!(failure.as_str(), "unknown-declared-source");
}

/// **漏声明**一个自己的能力 ⇒ 装配期拒绝，而且**点名漏的是哪一个**。
#[test]
fn a_capability_without_a_source_is_refused_and_named() {
    let app = Declaring::new(
        "partial",
        &["declared", "forgotten"],
        &[("declared", SourceClass::Environment)],
    );
    let failure = MinimalOs::assemble(vec![Box::new(app)]).expect_err("必须拒绝");
    assert_eq!(
        failure,
        AssemblyFailure::CapabilityWithoutSource {
            app: "partial".to_string(),
            action: "forgotten".to_string(),
        }
    );
    assert_eq!(failure.as_str(), "capability-without-source");
}

/// 一个都不声明同样拒绝——**空声明不是"默认只读"那种合法状态**。
///
/// 效果那一格有安全的默认值（默认只读 ⇒ 更少的能力），来源这一格**没有**：
/// 默认成任何一档都是替主体说一句主体没说过的实话。
#[test]
fn declaring_nothing_is_refused() {
    let app = Declaring::new("silent", &["something"], &[]);
    let failure = MinimalOs::assemble(vec![Box::new(app)]).expect_err("必须拒绝");
    assert_eq!(
        failure,
        AssemblyFailure::CapabilityWithoutSource {
            app: "silent".to_string(),
            action: "something".to_string(),
        }
    );
}

// ---------------------------------------------------------------------------
// 并表（展开那一下走的路）
// ---------------------------------------------------------------------------

/// 把一台 App 的声明并进表里，**不动**已经在表里的那些。
///
/// 外部来源的 App（MCP server）的清单要等真的连上才拿得到，走的就是这条路：
/// 同一份声明、同一个收集动作，只是时机晚了一点。
#[test]
fn absorbing_one_app_merges_it_without_touching_the_rest() {
    let first = Declaring::new("first", &["a"], &[("a", SourceClass::SelfAuthored)]);
    let second = Declaring::new("second", &["b"], &[("b", SourceClass::WorldUnsigned)]);

    let mut table = SourceTable::of(&[Box::new(first)]);
    assert_eq!(
        table.class_if_assembled("a"),
        Some(SourceClass::SelfAuthored)
    );
    assert_eq!(table.class_if_assembled("b"), None);

    table.absorb(&second);
    assert_eq!(
        table.class_if_assembled("a"),
        Some(SourceClass::SelfAuthored)
    );
    assert_eq!(
        table.class_if_assembled("b"),
        Some(SourceClass::WorldUnsigned)
    );
}
