//! 效果声明在**装配期**的机械检查。
//!
//! 这一层不证明"声明说的是真的"（那要行为测试与源码守卫，且无法机械封闭），只证明一件事：
//! **说出来的部分必须配得齐**——声明落在自己的能力里、写类别配了复核、复核也在自己的能力里。
//! 三条分类互不顶替，而且默认方向指向更少的能力：不声明 = 只读，仍然合法。
//!
//! 全部离线、无模型调用、无真实凭据。

use celira_agent::app_contract::{
    AppPort, AssemblyFailure, EffectClass, EffectDeclaration, SourceClass, SourceDeclaration,
};
use celira_agent::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionPort, Capabilities, Capability,
    ContentBlock, ParamSpec,
};
use celira_agent::foundation::{AgentId, OperationId, RunId};
use celira_agent::os::MinimalOs;

const WRITE: &str = "create_thing";
const VERIFY: &str = "verify_thing";
const READ: &str = "read_thing";

/// 可配置声明与能力的 App 替身：它没有业务，只回答"我声明了什么"。
struct DeclaringApp {
    id: &'static str,
    actions: Vec<&'static str>,
    effects: Vec<EffectDeclaration>,
}

impl DeclaringApp {
    fn new(id: &'static str, actions: &[&'static str]) -> Self {
        Self {
            id,
            actions: actions.to_vec(),
            effects: Vec::new(),
        }
    }

    fn declaring(mut self, effect: EffectDeclaration) -> Self {
        self.effects.push(effect);
        self
    }

    fn capabilities_of(&self) -> Capabilities {
        let items = self
            .actions
            .iter()
            .map(|action| {
                Capability::new(
                    ActionId::new(*action),
                    "测试替身：只回答自己声明了什么",
                    vec![ParamSpec::new("value", false, "替身不解释它")],
                )
            })
            .collect();
        Capabilities::of(items)
    }
}

impl ActionPort for DeclaringApp {
    fn capabilities(&self, _agent: &AgentId, _run: &RunId) -> Capabilities {
        self.capabilities_of()
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
        ActionOutcome::success(vec![ContentBlock::new("declared", "替身不执行任何东西")])
    }
}

impl AppPort for DeclaringApp {
    fn app_id(&self) -> &str {
        self.id
    }

    fn description(&self) -> &str {
        "测试替身 App（只声明效果，不做事）"
    }

    fn declared_capabilities(&self) -> Capabilities {
        self.capabilities_of()
    }

    fn declared_effects(&self) -> Vec<EffectDeclaration> {
        self.effects.clone()
    }

    /// 本文件只研究**效果**声明；来源那一格按"替身回执都是系统确认"填满，
    /// 免得每个效果用例都被 `capability-without-source` 抢答。
    fn declared_sources(&self) -> Vec<SourceDeclaration> {
        self.capabilities_of()
            .items()
            .iter()
            .map(|capability| SourceDeclaration::new(capability.id().clone(), SourceClass::System))
            .collect()
    }
}

fn declaration(action: &str, class: EffectClass, verification: Option<&str>) -> EffectDeclaration {
    EffectDeclaration::new(
        ActionId::new(action),
        class,
        verification.map(ActionId::new),
    )
}

/// 本文件的第一条规范：一个能力**要么**只读（不声明），**要么**声明写类别并配齐复核能力。
#[test]
fn a_write_declaration_without_verification_is_refused_at_assembly() {
    let app = DeclaringApp::new("things", &[WRITE]).declaring(declaration(
        WRITE,
        EffectClass::ExclusiveCreate,
        None,
    ));

    assert_eq!(
        MinimalOs::assemble(vec![Box::new(app)]).err(),
        Some(AssemblyFailure::WriteWithoutVerification {
            app: "things".to_string(),
            action: WRITE.to_string(),
        })
    );
    assert_eq!(
        AssemblyFailure::WriteWithoutVerification {
            app: "things".to_string(),
            action: WRITE.to_string(),
        }
        .as_str(),
        "write-without-verification"
    );
}

/// 声明必须落在**自己的能力**里：说一个不属于自己的动作是另一类错误，分类不顶替。
#[test]
fn a_declaration_about_an_action_the_app_does_not_own_is_refused() {
    let app = DeclaringApp::new("things", &[WRITE]).declaring(declaration(
        "someone_elses_action",
        EffectClass::ExclusiveCreate,
        Some(VERIFY),
    ));

    assert_eq!(
        MinimalOs::assemble(vec![Box::new(app)]).err(),
        Some(AssemblyFailure::UnknownDeclaredEffect {
            app: "things".to_string(),
            action: "someone_elses_action".to_string(),
        })
    );
    assert_eq!(
        AssemblyFailure::UnknownDeclaredEffect {
            app: "things".to_string(),
            action: "someone_elses_action".to_string(),
        }
        .as_str(),
        "unknown-declared-effect"
    );
}

/// 配了复核、但复核能力不在自己的清单里：与"漏配复核"是两件事，分类不顶替。
#[test]
fn a_verification_that_the_app_does_not_own_is_refused() {
    let app = DeclaringApp::new("things", &[WRITE]).declaring(declaration(
        WRITE,
        EffectClass::ExclusiveCreate,
        Some(VERIFY),
    ));

    assert_eq!(
        MinimalOs::assemble(vec![Box::new(app)]).err(),
        Some(AssemblyFailure::VerificationNotDeclared {
            app: "things".to_string(),
            action: WRITE.to_string(),
            verification: VERIFY.to_string(),
        })
    );
    assert_eq!(
        AssemblyFailure::VerificationNotDeclared {
            app: "things".to_string(),
            action: WRITE.to_string(),
            verification: VERIFY.to_string(),
        }
        .as_str(),
        "verification-not-declared"
    );
}

/// 复核检查与类别无关：只读声明**也**不许把别人的动作当成自己的复核能力。
///
/// 这条是"分类不顶替"的反向证据：漏配复核的那条检查不会替它兜底。
#[test]
fn a_read_only_declaration_still_may_not_borrow_a_foreign_verification() {
    let app = DeclaringApp::new("things", &[READ]).declaring(declaration(
        READ,
        EffectClass::ReadOnly,
        Some("not_ours"),
    ));

    assert_eq!(
        MinimalOs::assemble(vec![Box::new(app)]).err(),
        Some(AssemblyFailure::VerificationNotDeclared {
            app: "things".to_string(),
            action: READ.to_string(),
            verification: "not_ours".to_string(),
        })
    );
}

/// 默认方向指向更少的能力：什么都不声明是合法状态，不是缺口。
#[test]
fn an_app_that_declares_nothing_is_read_only_and_still_assembles() {
    let app = DeclaringApp::new("things", &[READ, WRITE]);
    let os = MinimalOs::assemble(vec![Box::new(app)]).expect("不声明 = 只读，装配必须成功");
    assert_eq!(os.app_ids(), vec!["things"]);
}

/// 配得齐的写声明装配成功：写类别 + 自己的复核能力。
///
/// 两个类别（独占创建、可重复写）都走这一条，因为**类别本身不改变装配检查**——
/// 检查的是"配齐了没有"，不是"是哪一类"。
#[test]
fn a_conforming_write_declaration_assembles_with_both_write_classes() {
    for class in [EffectClass::ExclusiveCreate, EffectClass::RepeatableWrite] {
        let app = DeclaringApp::new("things", &[WRITE, VERIFY]).declaring(declaration(
            WRITE,
            class,
            Some(VERIFY),
        ));
        let os = MinimalOs::assemble(vec![Box::new(app)]).unwrap_or_else(|error| {
            panic!("{} 配齐了复核，装配必须成功：{error:?}", class.as_str())
        });
        assert_eq!(os.app_ids(), vec!["things"]);
    }
}

/// 类别字符串是稳定的封闭取值，而且只有写类别算写。
#[test]
fn effect_class_labels_are_stable_and_only_writes_are_writes() {
    assert_eq!(EffectClass::ReadOnly.as_str(), "read-only");
    assert_eq!(EffectClass::ExclusiveCreate.as_str(), "exclusive-create");
    assert_eq!(EffectClass::RepeatableWrite.as_str(), "repeatable-write");
    assert_eq!(
        EffectClass::UnverifiableWrite.as_str(),
        "unverifiable-write"
    );
    assert!(!EffectClass::ReadOnly.is_write());
    assert!(EffectClass::ExclusiveCreate.is_write());
    assert!(EffectClass::RepeatableWrite.is_write());
    // 第四类**仍然是写**：它是"没法复核"，不是"不算写"。
    assert!(EffectClass::UnverifiableWrite.is_write());
}

/// **第四类是显式的**：它豁免的只是"必须配复核"那一条，别的写规矩一条不少。
///
/// 两边的对照都在这一条里：同一个替身声明 `UnverifiableWrite` 而不配复核 ⇒ **装配成功**；
/// 把它换成 `RepeatableWrite`（同样的"没配复核"）⇒ 照旧被拒。没有后一半，前一半什么都证明不了。
#[test]
fn an_unverifiable_write_declares_itself_and_needs_no_verification() {
    let app = DeclaringApp::new("things", &[WRITE]).declaring(declaration(
        WRITE,
        EffectClass::UnverifiableWrite,
        None,
    ));
    // ⚠ **旧规矩 → 新规矩**（2026-09-26，两刀）：这条用例原来在装配之后读**效果声明表**
    // （`os.effects()` 的 `is_write` / `class_if_assembled` / `verification_of`）。那张表随
    // **未决效果门**退役——门是它唯一的生产读者（门自己也在上一刀随跨进程恢复退役）。读数回到
    // **权威源**：App 自己那句声明（表本来就是它的机械收集）。
    let declared = app.declared_effects();
    assert_eq!(declared.len(), 1, "恰好一条声明");
    assert_eq!(declared[0].action().as_str(), WRITE);
    assert_eq!(declared[0].class(), EffectClass::UnverifiableWrite);
    // 它在声明里仍然是**写**：第四类豁免的只是"必须配复核"那一条，不是"不算写"。
    assert!(declared[0].class().is_write());
    assert!(
        declared[0].verification().is_none(),
        "它自成一类的理由就是「没有同参复核可配」"
    );

    let os = MinimalOs::assemble(vec![Box::new(app)])
        .unwrap_or_else(|error| panic!("不可验证的写可以不配复核：{error:?}"));
    assert_eq!(os.app_ids(), vec!["things"]);

    // 对照：同样的"没配复核"，换成可重复写就必须被拒。
    let old = DeclaringApp::new("things", &[WRITE]).declaring(declaration(
        WRITE,
        EffectClass::RepeatableWrite,
        None,
    ));
    assert_eq!(
        MinimalOs::assemble(vec![Box::new(old)]).err(),
        Some(AssemblyFailure::WriteWithoutVerification {
            app: "things".to_string(),
            action: WRITE.to_string(),
        }),
        "有路可复核的写类别，没配复核照旧拒绝"
    );
}

/// 第四类的装配检查与前三类**各自独立**：它不放过"声明不属于自己的动作"。
#[test]
fn the_unverifiable_class_still_requires_the_action_to_be_its_own() {
    let app = DeclaringApp::new("things", &[WRITE]).declaring(declaration(
        "someone_elses_action",
        EffectClass::UnverifiableWrite,
        None,
    ));
    assert_eq!(
        MinimalOs::assemble(vec![Box::new(app)]).err(),
        Some(AssemblyFailure::UnknownDeclaredEffect {
            app: "things".to_string(),
            action: "someone_elses_action".to_string(),
        })
    );
}
