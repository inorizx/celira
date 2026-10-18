//! **壳 ↔ 前端的最小接口**（施工单 §三「壳↔前端最小接口」，2026-09-30 首刀步骤 2）。
//!
//! ## 这个面的形状（施工者定；三条边界照 §三）
//!
//! - **是壳的内部接口，不是 im 公共契约**：它只活在 webview ↔ Rust 之间（`invoke`），
//!   不在任何 HTTP/CORS 面上。世界的公共事实一个字节都不从这里过。
//! - **壳不存 Human token / 家钥匙**（护栏）：本面只回「安家」的读数，不搬运任何凭据
//!   （Agent 的后端凭据落**按身份分条**的本机存储，从不回给页面）。
//! - **前端按能力探测分支**（不按平台名，承 `shell.ts` 判据精神）：页面判的是
//!   「`invoke` 这个方法在不在」（`client/src/api/anjia.ts` 的 `hasAnjiaBridge`），
//!   web 侧**安静缺席**、构建不受污染（动态 import，同 `shell.ts` 那一刀）。
//!
//! ## 词表（**稳定字符串；页面只认它**）
//!
//! `kind` 是八态（[`super::AnjiaOutcome::kind()`]）＋本层新增的一态
//! **`in-flight`**（同一支 Agent 已有一轮安家在跑）。
//!
//! **人话不在这里**：本面回的是**读数**（`kind` ＋ 原样 `lines` ＋ `pid`/路径）。
//! 把 `kind` 翻成人话是页面那一步的事（`client/src/api/anjia.ts` 的 `anjiaReadout`，
//! 纯函数 ＋ 用例）——两处各写一套话，就是**两个答案**。
//!
//! ## `in-flight` 为什么必须有
//!
//! 创建成功之后页面会**自动**叫一次安家，而 React 的 StrictMode / 用户重试都会让
//! 同一次动作发两次；两次若都过了判据 3（都还没安家）就会**起出两个进程**跑同一支
//! Agent —— 与「一支一个环境根、每支 1 个进程」直接冲突。所以壳这一侧放一道闸。
//!
//! 闸是**进程内、易失**的（不落盘、不进任何持久层）：壳重启即失忆，重来一次即可，
//! 这符合安家「重入安全」的性质（判据 3 会在重来那次认出「已经安家」）。
//!
//! ## 这一层不做的事
//!
//! - **不重判、不吞错**：判据与结局全归 [`super::run_anjia`]；这里只把
//!   [`super::AnjiaReport`] 翻成页面认得的读数（`view_of`）。
//! - **不做进度流**：一次安家是**有先后顺序的一段**（秒级，最坏一个 20 秒预算），
//!   今天的页面只需要「在跑／结论」两件事 —— 有具体消费者时再加事件，不预建。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::State;

use super::{run_anjia, AnjiaReport, AnjiaRequest, RefusalReason};

/// `in-flight` 这一态的稳定词（本层新增；八态见 [`super::AnjiaOutcome::kind()`]）。
pub const KIND_IN_FLIGHT: &str = "in-flight";

// ---------------------------------------------------------------------------
// 闸：同一支 Agent 同时只许一轮安家
// ---------------------------------------------------------------------------

/// 闸门（可克隆：内层是 `Arc`，方便把一次占用**移进阻塞任务**）。
#[derive(Clone, Default)]
pub struct AnjiaGate {
    running: Arc<Mutex<HashSet<String>>>,
}

/// 一次占用。**`Drop` 即释放** —— 把它移进 `spawn_blocking` 的闭包里，
/// 就算命令的 future 被丢掉（页面刷新/导航），阻塞工作跑完照样释放，不会漏。
pub struct AnjiaSlot {
    gate: AnjiaGate,
    agent_id: String,
}

impl AnjiaGate {
    /// 占住这一支；已经在跑（或闸坏了）⇒ `None`（调用方按 `in-flight` 回报）。
    pub fn try_enter(&self, agent_id: &str) -> Option<AnjiaSlot> {
        let mut running = match self.running.lock() {
            Ok(guard) => guard,
            // 闸被人毒化了：宁可保守（当作在跑）也不放第二个进程进去。
            Err(poisoned) => poisoned.into_inner(),
        };
        if !running.insert(agent_id.to_string()) {
            return None;
        }
        Some(AnjiaSlot {
            gate: self.clone(),
            agent_id: agent_id.to_string(),
        })
    }

    /// 这一支现在有没有一轮安家在跑（只读；给用例与将来的读数用）。
    pub fn is_running(&self, agent_id: &str) -> bool {
        match self.running.lock() {
            Ok(guard) => guard.contains(agent_id),
            Err(poisoned) => poisoned.into_inner().contains(agent_id),
        }
    }
}

impl Drop for AnjiaSlot {
    fn drop(&mut self) {
        let mut running = match self.gate.running.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        running.remove(&self.agent_id);
    }
}

// ---------------------------------------------------------------------------
// 读数：回给页面的形状
// ---------------------------------------------------------------------------

/// 一次安家回给页面的**读数**（字段名 camelCase，直接可用）。
///
/// `kind` 是稳定词表；`lines` 是壳自己那一串原样读数（页面拿它当"详情"，
/// **不当结论**——结论只有 `kind` 一份）。`sub` 是 `refused` 的**封闭子词**
/// （[`super::RefusalReason::sub()`]；别的结局恒 `None`）——页面按它给人话与下一步。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnjiaView {
    pub agent_id: String,
    /// 八态之一，或 [`KIND_IN_FLIGHT`]。
    pub kind: String,
    /// `refused` 的封闭子词；别的结局没有。
    pub sub: Option<String>,
    /// 进程号（起了才有；**只是报告**，不做 PID 认领）。
    pub pid: Option<u32>,
    /// 环境根（到了第 2 步才有；已安家时指向既有那一处）。
    pub env_root: Option<String>,
    /// 进程日志（起了才有）。
    pub log_file: Option<String>,
    /// 壳那一侧的原样读数。
    pub lines: Vec<String>,
}

/// 把壳的回报翻成页面读数（**纯函数**：可直测，不碰 IO）。
pub fn view_of(report: &AnjiaReport) -> AnjiaView {
    AnjiaView {
        agent_id: report.agent_id.clone(),
        kind: report.outcome.kind().to_string(),
        sub: report.refusal.map(|reason| reason.sub().to_string()),
        pid: report.pid,
        env_root: report
            .env_root
            .as_ref()
            .map(|path| path.display().to_string()),
        log_file: report
            .log_file
            .as_ref()
            .map(|path| path.display().to_string()),
        lines: report.lines.clone(),
    }
}

/// 已经在跑时回的那一份（不落任何东西、也不排第二次）。
pub fn in_flight_view(agent_id: &str) -> AnjiaView {
    AnjiaView {
        agent_id: agent_id.to_string(),
        kind: KIND_IN_FLIGHT.to_string(),
        sub: None,
        pid: None,
        env_root: None,
        log_file: None,
        lines: vec!["[安家] 这支 Agent 已经有一轮安家在进行中——这一次不重开".to_string()],
    }
}

/// 空 id 的拒绝读数（调用方的错；这一支不会动任何东西）。
pub fn empty_id_view(agent_id: &str) -> AnjiaView {
    AnjiaView {
        agent_id: agent_id.to_string(),
        kind: "refused".to_string(),
        sub: Some(RefusalReason::EmptyId.sub().to_string()),
        pid: None,
        env_root: None,
        log_file: None,
        lines: vec!["[安家] 收到一个空的 Agent id——什么都没动".to_string()],
    }
}

// ---------------------------------------------------------------------------
// 命令
// ---------------------------------------------------------------------------

/// 手动/自动触发一次安家（最窄面：本机那台家的默认位置；家与仓库根用默认解析）。
///
/// 返回值：
/// - `Ok(view)`：**任何有结论的结局**（成功、拒绝、失败都在这里——它们是结局，
///   不是异常；原因原样在 `kind`/`lines` 里）；
/// - `Err(text)`：**壳自己出了故障**（连读数都没能产出来：阻塞任务没能回来、
///   潜在的锁问题）。前端把这一支归到「读不到」。
///
/// ⚠ 安家是一段**有先后顺序的阻塞流程**（调 operator CLI、起进程、等报到），
/// 所以它在 `spawn_blocking` 里跑，**不占命令线程**。
#[tauri::command]
pub async fn anjia_start(
    agent_id: String,
    gate: State<'_, AnjiaGate>,
) -> Result<AnjiaView, String> {
    let agent_id = agent_id.trim().to_string();
    if agent_id.is_empty() {
        // 空 id 是调用方的错：说清即可（这一支不会动任何东西）。
        return Ok(empty_id_view(&agent_id));
    }

    // 先占闸（**在 await 之前**）：第二次调用立刻拿到 in-flight，不会走到起进程那一步。
    let Some(slot) = gate.try_enter(&agent_id) else {
        return Ok(in_flight_view(&agent_id));
    };

    let blocking_id = agent_id.clone();
    let joined = tauri::async_runtime::spawn_blocking(move || {
        // 占用跟着**阻塞工作**活：future 被丢掉也照样在结束时释放（见 `AnjiaSlot`）。
        let _held = slot;
        let req = AnjiaRequest {
            agent_id: &blocking_id,
            home_dir: None,
            repo_dir: None,
        };
        run_anjia(&req)
    })
    .await;

    match joined {
        Ok(report) => Ok(view_of(&report)),
        // 这一支只在壳内部真出事时出现（线程池关停/panic）——如实说"读不到"，
        // 不假装成功，也不假装是安家的失败。
        Err(error) => Err(format!("壳内部错误：安家那一轮没能回来（{error}）")),
    }
}

// ---------------------------------------------------------------------------
// 用例（纯逻辑；不需要壳在场）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anjia::AnjiaOutcome;
    use std::path::PathBuf;

    fn report(kind: AnjiaOutcome, pid: Option<u32>) -> AnjiaReport {
        AnjiaReport {
            agent_id: "agent-1".to_string(),
            outcome: kind,
            refusal: None,
            lines: vec!["[安家] 读数一行".to_string()],
            env_root: Some(PathBuf::from("/tmp/home/agents/agent-1")),
            log_file: Some(PathBuf::from("/tmp/home/logs/host-agent-1.out")),
            pid,
        }
    }

    #[test]
    fn view_carries_the_stable_kind_and_the_raw_paths() {
        let view = view_of(&report(AnjiaOutcome::Homed, Some(4242)));
        assert_eq!(view.kind, "homed");
        assert_eq!(view.agent_id, "agent-1");
        assert_eq!(view.pid, Some(4242));
        assert_eq!(
            view.env_root.as_deref(),
            Some("/tmp/home/agents/agent-1"),
            "环境根原样带出（页面只用它显示，不解析）"
        );
        assert_eq!(view.lines.len(), 1, "原样读数一行都不吞");
    }

    #[test]
    fn every_outcome_keeps_its_own_kind_word() {
        // 词表逐条对一遍：页面按它分流，串了就是"把拒绝说成成功"那一类错。
        let pairs = [
            (AnjiaOutcome::Homed, "homed"),
            // ★ 2026-10-15：接上了、但还没选模型 ⇒ 先没起它（**不是失败，也不是"跑起来了"**）。
            (AnjiaOutcome::HomedNotStarted, "homed-not-started"),
            (AnjiaOutcome::AlreadyHomed, "already-homed"),
            (AnjiaOutcome::RefusedWorldUnreadable, "world-unreadable"),
            (AnjiaOutcome::RefusedNotOwned, "not-owned"),
            (AnjiaOutcome::RefusedNotActive, "not-active"),
            (AnjiaOutcome::RefusedPrecondition, "refused"),
            (AnjiaOutcome::Failed, "failed"),
            (AnjiaOutcome::StartedNoReport, "started-no-report"),
        ];
        for (outcome, word) in pairs {
            assert_eq!(view_of(&report(outcome, None)).kind, word);
        }
    }

    #[test]
    fn in_flight_says_so_and_claims_nothing() {
        let view = in_flight_view("agent-9");
        assert_eq!(view.kind, KIND_IN_FLIGHT);
        assert_eq!(view.agent_id, "agent-9");
        assert!(view.pid.is_none(), "在跑不代表起了进程——不许报 pid");
        assert!(view.env_root.is_none(), "在跑不代表建了环境根");
        assert!(view.sub.is_none(), "在跑不是拒绝——不许带子词");
        assert!(!view.lines.is_empty(), "有话要说：不静默");
    }

    // ── 2026-10-02 刀：`refused` 的封闭子词 ─────────────────────────────────

    #[test]
    fn refused_carries_its_sub_word_and_other_outcomes_do_not() {
        let mut refused_report = report(AnjiaOutcome::RefusedPrecondition, None);
        refused_report.refusal = Some(RefusalReason::MissingModelCredential);
        let view = view_of(&refused_report);
        assert_eq!(view.kind, "refused");
        assert_eq!(view.sub.as_deref(), Some("missing-model-credential"));
        // 别的结局不许带子词（子词只在 refused 上有意义）。
        assert!(view_of(&report(AnjiaOutcome::Homed, Some(4242)))
            .sub
            .is_none());
        assert!(view_of(&report(AnjiaOutcome::Failed, None)).sub.is_none());
    }

    #[test]
    fn every_refusal_reason_has_a_distinct_stable_sub_word() {
        use crate::anjia::REFUSAL_SUBS;
        let all = [
            RefusalReason::MissingRepo,
            RefusalReason::MissingExecutable,
            RefusalReason::HomeNotSetUp,
            RefusalReason::BadIdShape,
            RefusalReason::ModelBindingUnreadable,
            RefusalReason::MissingModelCredential,
            RefusalReason::ModelNotChosen,
            RefusalReason::CredentialUnreadable,
            RefusalReason::EmptyId,
        ];
        let mut seen = std::collections::HashSet::new();
        for reason in all {
            assert!(seen.insert(reason.sub()), "子词重复：{}", reason.sub());
        }
        let words: Vec<&str> = all.iter().map(|reason| reason.sub()).collect();
        assert_eq!(
            words,
            REFUSAL_SUBS.to_vec(),
            "全量表与逐条子词必须一致（页面拿全量表对账）"
        );
    }

    #[test]
    fn empty_id_view_is_a_refused_with_the_empty_id_sub() {
        let view = empty_id_view("  ");
        assert_eq!(view.kind, "refused");
        assert_eq!(view.sub.as_deref(), Some("empty-id"));
        assert!(view.pid.is_none() && view.env_root.is_none() && view.log_file.is_none());
        assert!(!view.lines.is_empty(), "有话要说：不静默");
    }

    #[test]
    fn view_serializes_sub_as_a_camel_case_field() {
        // 页面读的是 `view.sub`（camelCase）——这个字段名是**线形状**的一部分，钉住它。
        let mut refused_report = report(AnjiaOutcome::RefusedPrecondition, None);
        refused_report.refusal = Some(RefusalReason::HomeNotSetUp);
        let json = serde_json::to_value(view_of(&refused_report)).unwrap();
        assert_eq!(json["kind"], "refused");
        assert_eq!(json["sub"], "home-not-set-up");
        // 非 refused 的结局：字段在、值为 null（页面读到的就是"没有子词"）。
        let json = serde_json::to_value(view_of(&report(AnjiaOutcome::Homed, None))).unwrap();
        assert!(json["sub"].is_null());
    }

    #[test]
    fn gate_refuses_the_second_round_for_the_same_agent() {
        let gate = AnjiaGate::default();
        let first = gate.try_enter("agent-1");
        assert!(first.is_some(), "第一轮应当占到闸");
        assert!(gate.is_running("agent-1"));
        assert!(
            gate.try_enter("agent-1").is_none(),
            "同一支的第二轮必须被拒（否则会起出第二个进程）"
        );
        assert!(gate.try_enter("agent-2").is_some(), "别的 Agent 不受牵连");
    }

    #[test]
    fn gate_releases_when_the_slot_drops() {
        let gate = AnjiaGate::default();
        {
            let _slot = gate.try_enter("agent-1").expect("第一轮");
            assert!(gate.is_running("agent-1"));
        }
        assert!(!gate.is_running("agent-1"), "占用随 Drop 释放");
        assert!(gate.try_enter("agent-1").is_some(), "释放之后可以再来一次");
    }
}
