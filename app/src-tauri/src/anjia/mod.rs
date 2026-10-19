//! **安家**——把「世界里的一支 Agent」接到「这台机器上的一个身体」的**执行**（壳侧）。
//!
//! ## 一句话原理（承施工单 §一）
//!
//! **创建给身份，安家给身体；两半两显式，不存在默认 Agent**（`PROJECT_SHAPE.md` §8.5）。
//! 世界侧能建 Agent（前端 `POST /v1/agents`），但那只建了**身份** —— 不写环境根、
//! 不发后端凭据、不起任何东西，所以**她不会回话**。这一层补的是机器侧那一次显式安家
//! 在 **app 里的自动化**：不发明新概念、不新造协议（im 服务端与 `scripts/home/` 一字不动）。
//!
//! ## 六步（§8.5）与三条判据（`adopt.js` 照抄，只改第三条）
//!
//! 1. **核实**「这个 id 是我的、且 active」——`GET /v1/agents`（Human Session 经既有
//!    operator CLI 补发，同一份库配置；token 只活在本次调用里，**壳不存它**）；
//! 2. **建它的环境根**（`agent_run init`：一支一个 —— `<家>/agents/<id>/`）；
//! 3. **写它的身份**（`identity.json`，同上一步一次完成）；
//! 4. **签后端凭据**（既有 HTTP 面，照 `provision.js` 的重入语义；落按身份分条的本地存储）；
//! 5. **起它的进程**（`agent_host`，每支 1 个；崩了不自动重启——§8.6-2）；
//! 6. **如实回报**（成功 / 失败 / 读不到；失败原因原样带上，不吞、不美化）。
//!
//! 三条判据：
//!
//! 1. **只能认领自己的**：id 必须在你拥有的列表里、`status=active`（撤销过的不许安家）；
//! 2. **问不到就不动**：世界连不上 / 回执读不懂 ⇒ **拒绝**（绝不先写上、回头再说）；
//! 3. **「这支 Agent 是否已经安家」**（承 §8.0：一台机器可跑多支；不是「这个家有没有主」）。
//!
//! ## 壳↔前端（首刀步骤 2）——看 [`commands`]
//!
//! 这一层只做**执行与回报**。页面那一头是**一个命令**（`anjia_start`）＋**一份读数**
//! （`kind` 稳定词表 ＋ 原样读数行 ＋ pid/路径）。**人话与四态归页面**
//! （`client/src/api/anjia.ts` 的 `anjiaReadout`，纯函数＋用例）——两处各写一套话就是两个答案。
//! `refused` 另带一个**封闭子词**（[`RefusalReason`]，2026-10-02 刀）：一个 `refused`
//! 说不出"缺什么"，子词让页面能给人话与具体下一步；未知子词页面按整词回退。
//!
//! ## 本刀**不做**的事（护栏；承施工单 §三 §四）
//!
//! - 不做同源代转、不代理、**不存 Human token / 家钥匙**；
//! - 不做 PID 认领（§8.7）、不做托盘与退出语义（§8.2-8.3）、崩了不自动重启（§8.6-2）；
//! - 不起世界（§8.6-1 不在本刀）、不写 `home.conf`（那是 `home.js` 的配置面）、
//!   不碰 `scripts/home/` 的任何文件；
//! - 远程家的管理模型（条目、钥匙）不在本刀 —— 本刀只解析**本机世界地址**
//!   （loopback + 约定端口、免钥匙，承 §8.4）。
//!
//! ## 一个有意加的前置（如实登记，见报告「疑点」）
//!
//! 第 5 步起进程需要**模型那一半**有人（宿主在装配模型之前就会拒绝启动——缺
//! `DEEPSEEK_API_KEY` 且无绑定 ⇒ 退出码 5）。本层照 `up.js` 的口径把这一问**提前**
//! 到「动手之前」：缺模型 ⇒ 拒绝、**什么都没建**（环境根还没建）——
//! 于是「配好模型再点一次安家」是一条走得通的路，不会留下
//! 「环境根建了、凭据签了、进程却起不来」的半截状态。

pub mod commands;
pub mod host_process;
mod user_env;

use std::path::{Path, PathBuf};

use crate::home_facts::{self as home, HomedAt, Plan};
use crate::model_sources::{self, ListState};
use crate::world_channel as world;

pub use crate::home_facts::Layout;

// ---------------------------------------------------------------------------
// 输入与回报（壳↔前端的「如实回报」形状——壳内部接口，不是 im 契约）
// ---------------------------------------------------------------------------

/// 一次安家的请求。
#[derive(Debug, Clone)]
pub struct AnjiaRequest<'a> {
    /// 世界侧那支 Agent 的 id（前端创建时拿到的那个）。
    pub agent_id: &'a str,
    /// 家的位置（不给 ⇒ `CELIRA_HOME_DIR` ⇒ 平台默认；测试与将来条目用）。
    pub home_dir: Option<&'a Path>,
    /// 仓库根/解压根（不给 ⇒ `CELIRA_REPO_DIR` ⇒ 从可执行件往上找；测试与开发用）。
    pub repo_dir: Option<&'a Path>,
}

/// 如实回报的**结局**（八态；`kind()` 是给上层分支用的稳定词表，不本地化）。
///
/// `RefusedPrecondition` 再带一个**封闭子词**（[`RefusalReason`]）——它压住了七个来路，
/// 一个 `refused` 说不出"缺什么"；子词让页面能给人话与下一步（人话仍在页面那一侧）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnjiaOutcome {
    /// 成功：环境根 + 身份 + 凭据 + 进程在跑（报到过）。
    Homed,
    /// ★ **接上了，但先没起它**（2026-10-15 主人裁）：这一支还没选模型（清单 0 条或多条）——
    /// **不是失败，也不是"跑起来了"**（第三种结局，如实报）。选好模型之后，
    /// 下次打开 app（`lib.rs` 那条"确保住户都在跑"）就会把它带起来。
    HomedNotStarted,
    /// 这台机器上已经有它的家——**什么都没动**（判据 3）。
    AlreadyHomed,
    /// 拒绝：**问不到世界**（连不上 / 回执读不懂）——什么都没动（判据 2）。
    RefusedWorldUnreadable,
    /// 拒绝：世界里没有这个 id，或者它不是你的。
    RefusedNotOwned,
    /// 拒绝：这个 id 不是 active（撤销过的不许安家）。
    RefusedNotActive,
    /// 拒绝：前置不满足（id 形状 / 家没布置 / 缺可执行件 / 缺模型凭据）——什么都没动。
    RefusedPrecondition,
    /// 失败：动手之后某一步没成（环境根 / 凭据 / 起进程），原因原样带上。
    Failed,
    /// 起了，但预算内没等到报到那一行——**不是失败，也不是成功**（如实第三态）。
    StartedNoReport,
}

/// **`refused` 的封闭子词表**（2026-10-02 刀：拒绝说得清）。
///
/// 为什么需要它：`refused` 一个词压住**七个来路 ＋ 空 id**（仓根找不到 / 缺可执行件 /
/// 家没布置 / id 形状 / 绑定读不懂 / 缺模型凭据 / 凭据读不懂 / 空 id），页面只能说
/// "还缺一样东西"——用户没法据此做下一步。子词是**稳定字符串，只加不换**；
/// 页面按它给人话与下一步，**未知子词页面按整词 `refused` 回退**（fail-closed，不瞎猜）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
    /// 找不到仓库根/解压根（可执行件在哪无从谈起）。
    MissingRepo,
    /// 缺可执行件（还没构建 / 安装）。
    MissingExecutable,
    /// 这个家还没布置过（`home.conf` 里没有 `human_id`）。
    HomeNotSetUp,
    /// id 形状不对（空 / 带空格 / 超长——`adopt.js` 的 `refuse-bad-id`）。
    BadIdShape,
    /// 环境根的模型绑定读不懂。
    ModelBindingUnreadable,
    /// 缺模型凭据（没有绑定，也没有 `DEEPSEEK_API_KEY`）。
    MissingModelCredential,
    /// 这台机器上有多条模型来源，而**这一支还没选用哪一条**（选哪条 = 花谁的钱 ⇒ 不替她决定）。
    ModelNotChosen,
    /// 本地存储里的凭据文件读不懂。
    CredentialUnreadable,
    /// 收到一个空的 Agent id（调用方的错）。
    EmptyId,
}

impl RefusalReason {
    /// 稳定子词（页面只认它；逐字对应 `client/src/api/anjia.ts` 的 `REFUSED_SUBS`）。
    pub fn sub(&self) -> &'static str {
        match self {
            RefusalReason::MissingRepo => "missing-repo",
            RefusalReason::MissingExecutable => "missing-executable",
            RefusalReason::HomeNotSetUp => "home-not-set-up",
            RefusalReason::BadIdShape => "bad-id-shape",
            RefusalReason::ModelBindingUnreadable => "binding-unreadable",
            RefusalReason::MissingModelCredential => "missing-model-credential",
            RefusalReason::ModelNotChosen => "model-not-chosen",
            RefusalReason::CredentialUnreadable => "credential-unreadable",
            RefusalReason::EmptyId => "empty-id",
        }
    }
}

/// 子词表全量（用例逐条对账；页面那份在 `client/src/api/anjia.ts`）。
pub const REFUSAL_SUBS: [&str; 9] = [
    "missing-repo",
    "missing-executable",
    "home-not-set-up",
    "bad-id-shape",
    "binding-unreadable",
    "missing-model-credential",
    "model-not-chosen",
    "credential-unreadable",
    "empty-id",
];

impl AnjiaOutcome {
    /// 稳定词表（壳内部接口；将来接前端时按它分支，不按文本）。
    pub fn kind(&self) -> &'static str {
        match self {
            AnjiaOutcome::Homed => "homed",
            AnjiaOutcome::HomedNotStarted => "homed-not-started",
            AnjiaOutcome::AlreadyHomed => "already-homed",
            AnjiaOutcome::RefusedWorldUnreadable => "world-unreadable",
            AnjiaOutcome::RefusedNotOwned => "not-owned",
            AnjiaOutcome::RefusedNotActive => "not-active",
            AnjiaOutcome::RefusedPrecondition => "refused",
            AnjiaOutcome::Failed => "failed",
            AnjiaOutcome::StartedNoReport => "started-no-report",
        }
    }
}

/// 一次安家的完整读数（`lines` 是给操作者/页面的原样读数）。
#[derive(Debug, Clone)]
pub struct AnjiaReport {
    pub agent_id: String,
    pub outcome: AnjiaOutcome,
    /// `refused` 的封闭子词（别的结局恒 `None`；见 [`RefusalReason`]）。
    pub refusal: Option<RefusalReason>,
    pub lines: Vec<String>,
    /// 环境根（到了第 2 步才会有；已安家时指向既有那一处）。
    pub env_root: Option<PathBuf>,
    /// 进程日志（起了进程才有）。
    pub log_file: Option<PathBuf>,
    /// 进程号（起了进程才有；**只是报告**，不做 PID 认领）。
    pub pid: Option<u32>,
}

// ---------------------------------------------------------------------------
// 主流程
// ---------------------------------------------------------------------------

fn report(
    agent_id: &str,
    outcome: AnjiaOutcome,
    lines: Vec<String>,
    env_root: Option<PathBuf>,
    log_file: Option<PathBuf>,
    pid: Option<u32>,
) -> AnjiaReport {
    AnjiaReport {
        agent_id: agent_id.to_string(),
        outcome,
        refusal: None,
        lines,
        env_root,
        log_file,
        pid,
    }
}

/// 一条**拒绝**（`refused` ＋ 子词）：什么都没动 —— 环境根 / 日志 / pid 全空。
fn refused(agent_id: &str, reason: RefusalReason, lines: Vec<String>) -> AnjiaReport {
    AnjiaReport {
        agent_id: agent_id.to_string(),
        outcome: AnjiaOutcome::RefusedPrecondition,
        refusal: Some(reason),
        lines,
        env_root: None,
        log_file: None,
        pid: None,
    }
}

fn homed_at_text(at: &HomedAt) -> String {
    match at {
        HomedAt::ShellRoot(path) => format!("{}（壳布局一支一个的环境根）", path.display()),
        HomedAt::HomeEnv(path) => format!("{}（home.js 那位住户的环境根）", path.display()),
        HomedAt::HomeConf => "home.conf 里记着它是这个家的住户（home.js 那一侧）".to_string(),
    }
}

/// 跑一次安家。全程同步（调 CLI、收发 HTTP、起进程、等报到）——
/// 将来接 Tauri 命令时用 `tauri::async_runtime::spawn_blocking` 托底。
pub fn run_anjia(req: &AnjiaRequest<'_>) -> AnjiaReport {
    let agent_id = req.agent_id.trim().to_string();
    let mut lines: Vec<String> = Vec::new();

    // ── 0) 家与形态（本机条目最小片：loopback + 约定端口、免钥匙）──────────
    let home_dir = home::resolve_home_dir(req.home_dir);
    let conf = home::read_conf(&home_dir);
    let listen = home::listen_addr(&conf);
    let base = format!("http://{listen}");
    lines.push(format!(
        "[安家] 家：{}；世界：{base}（本机、免钥匙）",
        home_dir.display()
    ));

    let Some(repo_dir) = home::resolve_repo_dir(req.repo_dir) else {
        lines.push(
            "[安家] 找不到仓库根/解压根（可执行件在哪无从谈起）—— 什么都没动。\
             显式指一个：CELIRA_REPO_DIR=<仓库根>"
                .to_string(),
        );
        return refused(&agent_id, RefusalReason::MissingRepo, lines);
    };
    let layout = home::detect_layout(&repo_dir);
    let missing: Vec<String> = [&layout.im_exe, &layout.run_exe, &layout.host_exe]
        .iter()
        .filter(|exe| !exe.is_file())
        .map(|exe| exe.display().to_string())
        .collect();
    if !missing.is_empty() {
        lines.push(format!(
            "[安家] 缺可执行件：{} —— 什么都没动。先构建：\
             (cd \"{}\" && cargo build --release) 以及 (cd \"{}/agent\" && cargo build --bins --release)",
            missing.join("、"),
            repo_dir.display(),
            repo_dir.display()
        ));
        return refused(&agent_id, RefusalReason::MissingExecutable, lines);
    }
    let Some(human_id) = conf
        .human_id
        .clone()
        .filter(|value| !value.trim().is_empty())
    else {
        lines.push(
            "[安家] 这个家还没布置过（home.conf 里没有 human_id）—— 什么都没动。\
             先用 node home.js up 把家点亮，再来安家"
                .to_string(),
        );
        return refused(&agent_id, RefusalReason::HomeNotSetUp, lines);
    };

    // ── 1) 判据 3：这支 Agent 在这台机器上已经有家吗？（本地，不碰世界）────
    let agent_id = match home::plan_anjia(&home_dir, &conf, &agent_id) {
        Plan::RefuseBadId => {
            lines.push(
                "[安家] 这不是一个 Agent id 的形状（空 / 带空格 / 超过 200 字符）—— 什么都没动"
                    .to_string(),
            );
            return refused(&agent_id, RefusalReason::BadIdShape, lines);
        }
        Plan::Already { agent_id, at } => {
            lines.push(format!(
                "[安家] 已经安家：{agent_id} —— 这台机器上已经有它的家：{}；这次什么都不动",
                homed_at_text(&at)
            ));
            let env_root = match &at {
                HomedAt::ShellRoot(path) | HomedAt::HomeEnv(path) => Some(path.clone()),
                // 只有住户登记对得上时，它该住的地方是**今天**的环境根（`<家>/agents/<支>/`，
                // 可能还没建完）。
                //
                // ⚠ **2026-10-18 更正**：原写 `home_dir.join("env")` —— 那是 **2026-09-30 以前
                // 的老家形状**（`scripts/home/paths.js` 明写它只是迁移输入），与壳**自己的**
                // `agent_env_root`（`home_facts.rs`）自相矛盾 ⇒ 这一格是**给前端显示的读数**，
                // 会显示一个**不存在、也不会被创建**的路径。⇒ **委托给那一份权威**，不手搓路径。
                HomedAt::HomeConf => Some(home::agent_env_root(&home_dir, &agent_id)),
            };
            return report(
                &agent_id,
                AnjiaOutcome::AlreadyHomed,
                lines,
                env_root,
                None,
                None,
            );
        }
        Plan::Adopt { agent_id } => agent_id,
    };

    // ── 2) 判据 1/2：问世界（问不到就不动）────────────────────────────────
    let human_token =
        match world::issue_human_session(&layout.im_exe, &listen, &home_dir, &human_id) {
            Ok(token) => token,
            Err(detail) => {
                lines.push(format!(
                    "[安家] 问不到世界（{detail}）—— 认领要先能问世界，这次什么都不改"
                ));
                return report(
                    &agent_id,
                    AnjiaOutcome::RefusedWorldUnreadable,
                    lines,
                    None,
                    None,
                    None,
                );
            }
        };
    let world_client = match world::World::open(&base, human_token, agent_id.clone()) {
        Ok(client) => client,
        Err(detail) => {
            lines.push(format!("[安家] 问不到世界（{detail}）—— 这次什么都不改"));
            return report(
                &agent_id,
                AnjiaOutcome::RefusedWorldUnreadable,
                lines,
                None,
                None,
                None,
            );
        }
    };
    let name = match world_client.verify_owned() {
        world::OwnedVerdict::Ok { name } => name,
        world::OwnedVerdict::Unreachable { detail } => {
            lines.push(format!(
                "[安家] 问不到世界（{detail}）—— 认领要先能问世界，这次什么都不改"
            ));
            return report(
                &agent_id,
                AnjiaOutcome::RefusedWorldUnreadable,
                lines,
                None,
                None,
                None,
            );
        }
        world::OwnedVerdict::NotOwned { total } => {
            lines.push(format!(
                "[安家] 世界里没有这个 Agent，或者它不是你的：{agent_id}\
                 （GET /v1/agents 只列**你拥有的**，共 {total} 个）—— 换一个 id，\
                 或先在客户端把它建出来；这次什么都没改"
            ));
            return report(
                &agent_id,
                AnjiaOutcome::RefusedNotOwned,
                lines,
                None,
                None,
                None,
            );
        }
        world::OwnedVerdict::NotActive { status } => {
            lines.push(format!(
                "[安家] 这个 Agent 不是 active（status={status}）—— 撤销过的身份不许安家，这次什么都没改"
            ));
            return report(
                &agent_id,
                AnjiaOutcome::RefusedNotActive,
                lines,
                None,
                None,
                None,
            );
        }
    };
    lines.push(format!(
        "[安家] 问过世界了：{name}（{agent_id}）是你的，status=active"
    ));

    // ── 2.5) 模型那一半（照 `up.js` 的早失败口径；见文件头）──
    //
    // ⚠ 2026-10-03：这一段的**口径收进了 `host_process::model_plan`**（安家那只手与
    // "打开 app 起她"共用同一处）—— 从前两边各写一份，迟早漂移。
    //
    // ★ 2026-10-15：**没有模型计划不再拦住"接上"**（`plan = None` ⇒ 照旧建家、只是先不起）。
    //   判据与四态见 [`choose_entry_if_unbound`]：接上是"存在"，选模型是"花钱"，存在不等花钱。
    let env_root = home::agent_env_root(&home_dir, &agent_id);
    let plan = match host_process::model_plan(&home_dir, &env_root) {
        Ok(plan) => Some(plan),
        Err(host_process::ModelPlanError::BindingUnreadable(reason)) => {
            lines.push(format!(
                "[安家] 环境根的模型绑定读不懂（{reason}）—— 先去「模型配置」页重绑一次再来；\
                 这次什么都没动"
            ));
            return refused(&agent_id, RefusalReason::ModelBindingUnreadable, lines);
        }
        Err(host_process::ModelPlanError::MissingBinding) => {
            // **这一支还没有绑定**（见 [`choose_entry_if_unbound`] 的判据与 2026-10-09 / 2026-10-15 两次裁决）。
            match choose_entry_if_unbound(&home_dir, &env_root) {
                Ok(Some((chosen, line))) => {
                    lines.push(line);
                    match host_process::model_plan(&home_dir, &env_root) {
                        Ok(plan) => Some(plan),
                        Err(failure) => {
                            lines.push(format!(
                                "[安家] 绑到 {chosen} 之后仍然算不出模型计划（{failure:?}）——\
                                 去「模型配置」页看一眼那一条；这次什么都没起"
                            ));
                            return refused(
                                &agent_id,
                                RefusalReason::ModelBindingUnreadable,
                                lines,
                            );
                        }
                    }
                }
                // ★ **没得绑（0 条 / 多条）：照旧接上，只是先不起它**（2026-10-15 主人裁）。
                Ok(None) => None,
                Err((reason, extra)) => {
                    lines.extend(extra);
                    return refused(&agent_id, reason, lines);
                }
            }
        }
    };
    // 凭据本地存储**读不出来** ⇒ 提前挑明（照 `credentials.js` 的纪律：那多半是别人的/坏的文件，
    // 覆盖它等于替主人做决定）。这里提前挡还有一个更硬的理由：那一步的「换一份」会**在世界侧轮换**，
    // 而轮换出来的原文紧接着会写不进本地存储 ⇒ 世界与机器同时丢掉那一支（谁也没有新钥匙）。
    let credential_dir = home::credentials_dir();
    if let home::CredentialRead::Unreadable(reason) =
        home::read_credential(&credential_dir, &base, &agent_id)
    {
        lines.push(format!(
            "[安家] 本地存储里的凭据文件读不出来（{reason}）—— 先看清它再重试；这一次什么都没动\
             （世界 {}，身份 {}）",
            base, agent_id
        ));
        return refused(&agent_id, RefusalReason::CredentialUnreadable, lines);
    }

    // ── 3) 建它的环境根 + 写它的身份（一支一个）──────────────────────────
    for dir in [home_dir.join("materials"), home_dir.join("logs")] {
        if let Err(error) = std::fs::create_dir_all(&dir) {
            lines.push(format!(
                "[安家] 家目录写不进去（{}：{error}）—— 停手",
                dir.display()
            ));
            return report(&agent_id, AnjiaOutcome::Failed, lines, None, None, None);
        }
    }
    let init = world::run_cli(
        &layout.run_exe,
        &[
            "init",
            "--root",
            &env_root.display().to_string(),
            "--agent",
            &agent_id,
        ],
        &world::im_env(&listen, &home_dir),
    );
    if init.code != Some(0) {
        lines.push(format!(
            "[安家] 建环境根这一步没成（agent_run init 退出码 {:?}）—— 环境根留在：{}",
            init.code,
            env_root.display()
        ));
        push_failure_detail(&mut lines, &init.stderr, init.error.as_deref());
        return report(
            &agent_id,
            AnjiaOutcome::Failed,
            lines,
            Some(env_root),
            None,
            None,
        );
    }
    lines.push(format!(
        "[安家] 环境根已建、身份已写：{}（一支一个）",
        env_root.display()
    ));

    // ── 4) 它的后端凭据（既有 HTTP 面；照 `provision.js` 的现做法）──────────
    //
    // 现做法三条（`provision.js` 的 `resolveBackendCredential` + `credentialAction`）：
    // ① 本地存储里那份**世界认它、且认的是本家** ⇒ **不动**（沿用——那是权威位置，没有轮换的理由）；
    // ② 只在老路（`CELIRA_IM_BACKEND_AUTH`）里那份、且确实是本家的 ⇒ **原文照搬**进本地存储（**不轮换**）；
    // ③ 都没有 / 那支不属于本家 ⇒ 重入语义：读现有凭据 → 换新键轮换（`obtainBackendCredential`）。
    //
    // ⚠ 壳**不做**「Agent 在不在跑」那一问（那是 `provision.js` 的第一条优先级）：认领 PID / 查进程表
    // 是 §8.7 整块不要的；而且它在本刀**不可能有真实的输入**——「已经安家」在判据 3 就被挡在前面了。
    let mut backend: Option<String> = None;
    let stored = home::read_credential(&credential_dir, &base, &agent_id);
    let (candidate, source) = match &stored {
        home::CredentialRead::Present(token) => (Some(token.clone()), "本地存储"),
        home::CredentialRead::Missing => (
            user_env::read_user_env("CELIRA_IM_BACKEND_AUTH")
                .filter(|value| !value.trim().is_empty()),
            "老路环境变量",
        ),
        // 已在 2.5 挡下（读不出来**不许**回落到老路——`credentials.js` 的纪律）。
        home::CredentialRead::Unreadable(_) => (None, "本地存储"),
    };
    if let Some(token) = candidate {
        match world_client.identity(&token) {
            Ok(Some((id, name))) => {
                if source == "老路环境变量" {
                    match home::write_credential(&credential_dir, &base, &agent_id, &token) {
                        Ok(file) => lines.push(format!(
                            "[安家] 老路里那支凭据确实是本家的，已收进本地存储（没有轮换）：{}",
                            file.display()
                        )),
                        Err(detail) => lines.push(format!(
                            "[安家] ⚠ 收进本地存储没成功（{detail}）—— 这一次照旧用老路里那支"
                        )),
                    }
                } else {
                    lines.push(format!(
                        "[安家] 本地存储里已有一支它的后端凭据，世界认它（{name}，{id}）—— 沿用，不轮换"
                    ));
                }
                backend = Some(token);
            }
            Ok(None) => lines.push(format!(
                "[安家] {source}里那支凭据问不出身份（me 没给出 id 与名字）—— 换一份"
            )),
            Err(detail) => lines.push(format!(
                "[安家] {source}里那支凭据不归本家（{detail}）—— 换一份"
            )),
        }
    }
    let backend = match backend {
        Some(token) => token,
        None => {
            let fresh = match world_client.sign_backend_credential() {
                Ok(token) => token,
                Err(detail) => {
                    lines.push(format!("[安家] 签后端凭据没成：{detail}"));
                    return report(
                        &agent_id,
                        AnjiaOutcome::Failed,
                        lines,
                        Some(env_root),
                        None,
                        None,
                    );
                }
            };
            match home::write_credential(&credential_dir, &base, &agent_id, &fresh) {
                Ok(file) => lines.push(format!(
                    "[安家] 后端凭据已签发、已落盘（按「世界 × 身份」分条）：{}",
                    file.display()
                )),
                Err(detail) => {
                    lines.push(format!("[安家] 凭据写不进本地存储：{detail}"));
                    return report(
                        &agent_id,
                        AnjiaOutcome::Failed,
                        lines,
                        Some(env_root),
                        None,
                        None,
                    );
                }
            }
            fresh
        }
    };
    let identity = match world_client.identity(&backend) {
        Ok(identity) => identity,
        Err(detail) => {
            lines.push(format!(
                "[安家] 新签的凭据没通过世界那一关（{detail}）—— 进程没起；这把凭据已经签出去了"
            ));
            return report(
                &agent_id,
                AnjiaOutcome::Failed,
                lines,
                Some(env_root),
                None,
                None,
            );
        }
    };
    match &identity {
        Some((id, name)) => lines.push(format!("[安家] 验过凭据：它代表 {name}（{id}）")),
        None => {
            lines.push("[安家] 世界没报出这份凭据代表谁 —— 起进程时不带「是谁」那两行".to_string())
        }
    }

    // ── 4.5) 写**住户登记**（2026-10-03 刀：同一份机械事实只有一个写者，而写者就是安家这只手）
    //
    // 病根（真机读数）：从前壳建了环境根、签了凭据、起了进程，却**没写 `agent.conf`** ⇒
    // `node home.js status` 看这支住户是「住户 ?，房 无 …… 还没安家」，`up` 还会想再安一次。
    // 形状权威是 `scripts/home/residents.js`（照抄，不发明）：`agent_id` / `room_id` / `slot` /
    // `display_name`；序号 `max(已有) + 1` 单调不回收；房**问世界**（`GET /v1/rooms` 找
    // `kind=direct` 且 `peer.id` 就是她的那间——`adopt.js` 原样），问不到就写空。
    //
    // ⚠ 这里写的全是**编排事实**（这台机器上的第几支、跟哪间房绑着）——**主体自己的经历与判断
    // 一个字都不在这里**（判据 9：系统补能力缺口、不替认知判断）。
    let room_id = match world_client.find_own_direct_room() {
        Ok(Some(id)) => id,
        Ok(None) => {
            lines.push(
                "[安家] 世界里还没有她与主人的那间 direct 房 —— 登记里房那一格先写空".to_string(),
            );
            String::new()
        }
        Err(detail) => {
            // **问不到就不猜**：房那一格写空，并如实说（不是失败 —— 登记里那一格本来就允许空）。
            lines.push(format!(
                "[安家] 房那一格没问出来（{detail}）—— 先写空，不猜一间出来"
            ));
            String::new()
        }
    };
    let existing = crate::registration::read_agent_conf(&env_root);
    let registration = crate::registration::AgentConf {
        agent_id: agent_id.clone(),
        room_id,
        // 已经有序号 ⇒ 沿用（重跑不改身份）；没有才分配（单调，不回收）。
        slot: existing
            .slot
            .or_else(|| Some(crate::registration::allocate_slot(&home_dir))),
        display_name: name.clone(),
    };
    match crate::registration::write_agent_conf(&env_root, &registration) {
        Ok(changed) => lines.push(format!(
            "[安家] 住户登记已写{}：{}（住户 {}，房 {}）",
            if changed {
                ""
            } else {
                "（内容没变，一个字都没动）"
            },
            env_root.join(crate::registration::AGENT_CONF).display(),
            registration.slot.unwrap_or(0),
            if registration.room_id.is_empty() {
                "无"
            } else {
                registration.room_id.as_str()
            }
        )),
        Err(detail) => {
            lines.push(format!("[安家] 住户登记写不进去：{detail}"));
            return report(
                &agent_id,
                AnjiaOutcome::Failed,
                lines,
                Some(env_root),
                None,
                None,
            );
        }
    }

    // ── 5) 起它的进程（每支 1 个；崩了不自动重启——本层不承诺生命周期）────
    //
    // ★ 2026-10-15：**没有模型计划就不起**（`plan = None`，即 0 条或多条来源、这一支还没选）。
    //   这不是失败，也不是"跑起来了"——**接上了，先没起它**（第三种结局，如实报）。
    //   起了它也答不了话（`local_residents::start_resident` 本来就如实拒绝没模型的住户），
    //   所以这里**不起**：早失败那条纪律照旧（不是"起起来再看着她认证不过"）。
    let Some(plan) = plan else {
        lines.push(
            "[安家] 接上了，但**先没起它**：这一支还没选模型 —— 去它自己的页面上选一条\
             （或「模型配置」里给这一支选一条），选好之后下次打开 app 就会把它带起来"
                .to_string(),
        );
        return report(
            &agent_id,
            AnjiaOutcome::HomedNotStarted,
            lines,
            Some(env_root),
            None,
            None,
        );
    };
    // 账本按 layout 那条路走（`logs/agents/<支>/host.out` ＋ `host.pid`）：**谁起的谁记账**，
    // 这样 `home.js status/down` 认得出它（不是 PID 认领 —— §8.7 撤掉的是"认领别人的进程"）。
    let log_file = crate::registration::host_log_file(&home_dir, &agent_id);
    let spec = host_process::HostSpec {
        home_dir: home_dir.clone(),
        env_root: env_root.clone(),
        agent_id: agent_id.clone(),
        listen: listen.clone(),
        identity,
        model_sources_file: plan.sources_file(),
        platform: host_process::HostPlatform::current(),
    };
    let launch = host_process::HostLaunch {
        exe: &layout.host_exe,
        spec: &spec,
        backend_credential: &backend,
        model_api_key: plan.env_key(),
        log_file: &log_file,
    };
    match host_process::spawn_agent_host(&launch) {
        Err(detail) => {
            lines.push(format!(
                "[安家] 起进程没成：{detail}（日志 {}）",
                log_file.display()
            ));
            report(
                &agent_id,
                AnjiaOutcome::Failed,
                lines,
                Some(env_root),
                Some(log_file),
                None,
            )
        }
        Ok((spawned, _child)) => {
            // 用起进程时**实际用的那一份**日志路径（不是这里再算一遍）。
            let host_log = spawned.log_file.clone();
            // 账本照写（谁起的谁记账）：`logs/agents/<支>/host.pid`。
            let pid_file = crate::registration::host_pid_file(&home_dir, &agent_id);
            match crate::registration::write_pid(&pid_file, spawned.pid) {
                Ok(()) => lines.push(format!(
                    "[安家] 账本已记：{}（pid {}）",
                    pid_file.display(),
                    spawned.pid
                )),
                Err(detail) => lines.push(format!(
                    "[安家] ⚠ 账本没记上（{detail}）—— 进程在跑，但 `home.js status/down` 这一轮认不出它"
                )),
            }
            lines.push(format!(
                "[安家] 进程已起：pid {}（日志 {}）",
                spawned.pid,
                host_log.display()
            ));

            // ── 6) 等报到（如实三态；20 秒预算与 up.js 同款）────────────────
            match host_process::wait_for_report(&host_log, 40, 500) {
                host_process::HostReport::Listening => {
                    lines.push(format!(
                        "[安家] 它报到了（[listening]）—— 安家完成：{name}（{agent_id}）在这台机器上已经住下"
                    ));
                    report(
                        &agent_id,
                        AnjiaOutcome::Homed,
                        lines,
                        Some(env_root),
                        Some(host_log),
                        Some(spawned.pid),
                    )
                }
                host_process::HostReport::Exited { lines: exit_lines } => {
                    lines.push("[安家] 它起来后自己退了，原因（日志原文）：".to_string());
                    for line in exit_lines {
                        lines.push(format!("       {line}"));
                    }
                    report(
                        &agent_id,
                        AnjiaOutcome::Failed,
                        lines,
                        Some(env_root),
                        Some(host_log),
                        Some(spawned.pid),
                    )
                }
                host_process::HostReport::NotYet => {
                    lines.push(format!(
                        "[安家] 进程在（pid {}），但 20 秒内没等到报到那一行——\
                         不是失败，也不是成功；看日志：{}",
                        spawned.pid,
                        host_log.display()
                    ));
                    for line in host_process::log_tail(&host_log, 5) {
                        lines.push(format!("       {line}"));
                    }
                    report(
                        &agent_id,
                        AnjiaOutcome::StartedNoReport,
                        lines,
                        Some(env_root),
                        Some(host_log),
                        Some(spawned.pid),
                    )
                }
            }
        }
    }
}

/// 失败细节：stderr 末尾几行原样带出（起不来时带 `error`）。
fn push_failure_detail(lines: &mut Vec<String>, stderr: &str, error: Option<&str>) {
    let detail = match error {
        Some(message) if stderr.trim().is_empty() => message.to_string(),
        Some(message) => format!("{message}\n{}", world::tail_text(stderr, 3)),
        None => world::tail_text(stderr, 3),
    };
    for line in detail.lines() {
        if !line.trim().is_empty() {
            lines.push(format!("       {line}"));
        }
    }
}

/// **补登记**：给一支**已经安家**的住户把机械事实补齐（`agent.conf` ＋ 凭据的世界分条）。
///
/// 为什么要有它（2026-10-03 刀）：在壳这只手学会写登记**之前**安下来的住户，
/// 环境根与凭据都在、登记却缺 —— 于是 `home.js` 那个视角永远说它"还没安家"。
/// 补登记**只补机械事实**：
///
/// - **不碰环境根**（她的经历、笔记、轨迹一个字都不动）；
/// - **不碰进程**（不起、不停、不认领 PID）；
/// - **幂等**（登记内容没变就一个字都不写）。
pub fn ensure_registration(
    agent_id: &str,
    home_dir: Option<&Path>,
    repo_dir: Option<&Path>,
) -> RegisterReport {
    let mut lines: Vec<String> = Vec::new();
    let agent_id = agent_id.trim().to_string();
    if agent_id.is_empty() || agent_id.contains(' ') {
        lines.push("[登记] 这不是一个 Agent id（空 / 带空格）—— 什么都没动".to_string());
        return RegisterReport {
            outcome: RegisterOutcome::RefusedBadId,
            lines,
        };
    }
    let facts = crate::local_world::LocalFacts::load(home_dir, repo_dir);
    let home_dir = facts.home_dir.clone();
    let env_root = home_dir.join("agents").join(&agent_id);
    if !env_root.join("identity.json").is_file() {
        lines.push(format!(
            "[登记] 这个家没有她的环境根（{} 里没有 identity.json）—— 补登记只补**已经安家**的住户；\
             要安一支新的： node home.js up --agent {agent_id}",
            env_root.display()
        ));
        return RegisterReport {
            outcome: RegisterOutcome::RefusedNoEnvRoot,
            lines,
        };
    }
    let base = facts.world_base();

    // 主人的一支会话（只活在这一次调用里；世界没在跑就停手 —— 房与显示名都要问它）。
    let human_token = match crate::local_world::local_session(&facts) {
        crate::local_world::SessionOutcome::Ok {
            token, human_id, ..
        } => {
            lines.push(format!("[登记] 以主人身份问世界（{human_id}）"));
            token
        }
        crate::local_world::SessionOutcome::WorldDown => {
            lines.push(
                "[登记] 世界没在跑 —— 先起它（app 里点一下，或 node home.js up），再补登记"
                    .to_string(),
            );
            return RegisterReport {
                outcome: RegisterOutcome::RefusedWorldUnreadable,
                lines,
            };
        }
        crate::local_world::SessionOutcome::NoCore => {
            lines.push(format!(
                "[登记] 缺可执行件：{}",
                facts.layout.im_exe.display()
            ));
            return RegisterReport {
                outcome: RegisterOutcome::Failed,
                lines,
            };
        }
        crate::local_world::SessionOutcome::Failed { detail } => {
            lines.push(format!("[登记] 取一支主人会话没成：{detail}"));
            return RegisterReport {
                outcome: RegisterOutcome::Failed,
                lines,
            };
        }
    };

    let world_client = match world::World::open(&base, human_token, agent_id.clone()) {
        Ok(client) => client,
        Err(detail) => {
            lines.push(format!("[登记] 问不到世界（{detail}）—— 什么都没动"));
            return RegisterReport {
                outcome: RegisterOutcome::RefusedWorldUnreadable,
                lines,
            };
        }
    };
    let display_name = match world_client.verify_owned() {
        world::OwnedVerdict::Ok { name } => name,
        world::OwnedVerdict::Unreachable { detail } => {
            lines.push(format!("[登记] 问不到世界（{detail}）—— 什么都没动"));
            return RegisterReport {
                outcome: RegisterOutcome::RefusedWorldUnreadable,
                lines,
            };
        }
        world::OwnedVerdict::NotOwned { total } => {
            lines.push(format!(
                "[登记] 世界里没有这个 Agent，或者它不是你的（你拥有 {total} 个）—— 什么都没动"
            ));
            return RegisterReport {
                outcome: RegisterOutcome::RefusedNotOwned,
                lines,
            };
        }
        world::OwnedVerdict::NotActive { status } => {
            lines.push(format!(
                "[登记] 这个 Agent 不是 active（status={status}）—— 什么都没动"
            ));
            return RegisterReport {
                outcome: RegisterOutcome::RefusedNotActive,
                lines,
            };
        }
    };
    let room_id = match world_client.find_own_direct_room() {
        Ok(Some(id)) => id,
        Ok(None) => {
            lines.push("[登记] 世界里还没有她与主人的那间 direct 房 —— 房那一格写空".to_string());
            String::new()
        }
        Err(detail) => {
            lines.push(format!(
                "[登记] 房那一格没问出来（{detail}）—— 先写空，不猜一间出来"
            ));
            String::new()
        }
    };

    let existing = crate::registration::read_agent_conf(&env_root);
    let slot = existing
        .slot
        .unwrap_or_else(|| crate::registration::allocate_slot(&home_dir));
    let registration = crate::registration::AgentConf {
        agent_id: agent_id.clone(),
        room_id: room_id.clone(),
        slot: Some(slot),
        display_name: display_name.clone(),
    };
    let changed = match crate::registration::write_agent_conf(&env_root, &registration) {
        Ok(changed) => changed,
        Err(detail) => {
            lines.push(format!("[登记] 写不进去：{detail}"));
            return RegisterReport {
                outcome: RegisterOutcome::Failed,
                lines,
            };
        }
    };
    lines.push(format!(
        "[登记] {}（住户 {slot}，房 {}，显示名 {}）",
        if changed {
            "登记已补齐"
        } else {
            "登记本来就对 —— 一个字都没动"
        },
        if room_id.is_empty() { "无" } else { &room_id },
        if display_name.is_empty() {
            "无"
        } else {
            &display_name
        }
    ));

    // 凭据：老形状（扁平那份）⇒ 搬到**按「世界 × 身份」分条**的新形状。
    // 判据：新形状在 ⇒ 不动；只有老形状在 ⇒ 搬（**同一份凭据不该有两份**）。
    let credentials_dir = home::credentials_dir();
    if let Some(legacy) = home::legacy_credential_path(&credentials_dir, &agent_id) {
        if legacy.is_file() {
            match home::credential_file(&credentials_dir, &base, &agent_id) {
                Some(target) if !target.is_file() => {
                    if let Some(parent) = target.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    match std::fs::rename(&legacy, &target) {
                        Ok(()) => lines.push(format!(
                            "[登记] 凭据已搬进按世界分条的那一格：{}",
                            target.display()
                        )),
                        Err(error) => lines.push(format!(
                            "[登记] ⚠ 凭据搬不过去（{error}）—— 老形状那份还在，读得到（不必慌）"
                        )),
                    }
                }
                Some(_) => lines.push(
                    "[登记] 凭据已经在按世界分条的那一格（老形状那份也在 —— 读的是新的那份）"
                        .to_string(),
                ),
                None => lines.push("[登记] ⚠ 世界地址算不出摘要 —— 凭据没搬".to_string()),
            }
        }
    }

    RegisterReport {
        outcome: RegisterOutcome::Registered { changed },
        lines,
    }
}

/// 补登记的结局（**只补机械事实**；拒绝的来路各有其名）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterOutcome {
    /// 登记已就位（`changed` = 这一次真的写了吗；`false` = 本来就对）。
    Registered { changed: bool },
    /// 拒绝：id 形状不对。
    RefusedBadId,
    /// 拒绝：这个家没有她的环境根（补登记只管已经安家的住户）。
    RefusedNoEnvRoot,
    /// 拒绝：问不到世界（这一点**不猜**）。
    RefusedWorldUnreadable,
    /// 拒绝：世界里没有这个 id，或者它不是你的。
    RefusedNotOwned,
    /// 拒绝：这个 id 不是 active。
    RefusedNotActive,
    /// 失败：动手之后某一步没成。
    Failed,
}

impl RegisterOutcome {
    /// 稳定子词（给脚本判读用；与 `AnjiaOutcome::kind` 同一条纪律：只加不换）。
    pub fn kind(self) -> &'static str {
        match self {
            Self::Registered { .. } => "registered",
            Self::RefusedBadId => "refused-bad-id",
            Self::RefusedNoEnvRoot => "refused-no-env-root",
            Self::RefusedWorldUnreadable => "refused-world-unreadable",
            Self::RefusedNotOwned => "refused-not-owned",
            Self::RefusedNotActive => "refused-not-active",
            Self::Failed => "failed",
        }
    }
}

/// 补登记的一次回报（读数行 ＋ 结局）。
#[derive(Debug, Clone)]
pub struct RegisterReport {
    pub outcome: RegisterOutcome,
    pub lines: Vec<String>,
}

/// **替它绑的那一条**（条目 id ＋ 给操作者看的一行）。
type ChosenBinding = (String, String);

/// 拒绝的回执：原因 ＋ 要交代的那几行。
type BindingRefusal = (RefusalReason, Vec<String>);

/// **这一支还没有绑定时的补救**（2026-10-09 修：主人第一次用 app 就撞上的死锁）。
///
/// ★ **接上之前那一格模型：只在"没有选择"时才替它绑**（2026-10-09 主人裁 ＋ 2026-10-15 修订）。
///
/// 判据一句话：**清单是机器级事实，绑定是支级事实**。"这台机器上有哪些钥匙"由页面写进
/// `<家>/model_sources.json`（机器级），"这一支用哪一条"才写进 `<环境根>/model_binding.json`
/// （支级）。于是一支 Agent 都还没有的机器上，**绑定没地方挂** —— 而从前的做法是当场拒绝
/// （`MissingModelCredential`），于是「创建 ⇒ 要绑定；绑定 ⇒ 要 Agent」谁也不动，
/// 页面还把这句话读成"模型那一半还没配"，可主人明明在「模型配置」里配过、清单里就躺着那一条。
///
/// ## ★ 2026-10-15 修订：**接上 ≠ 选模型**（主人当场裁）
///
/// 从前的四态是"恰一条 ⇒ 绑上并接；**多条 ⇒ 拒绝整个接上动作**；空 ⇒ 同样拒绝"。
/// 主人当次的原话：**"接到这个机器上，跟模型不冲突啊，接到这个机器上后才能选这个机器上的模型吧"**
/// —— 判据成立，而且**运行时早就把"接了但没选"当合法状态**：宿主那一侧
/// （`agent/src/model_source.rs` 的 `decide`）写着「没有绑定 ⇒ `Source::Process`」，
/// **它不会去清单里挑第一条** ⇒ 不绑**不会偷偷花钱**。
/// 那条拒绝的理由只有一句"选哪一条 = 花谁的钱，不替它决定" —— 它只证明**不许偷偷绑**，
/// 证明不了**不许接**。拒绝的射程超出了它的理由，于是：
///
/// | 清单 | 从前 | 现在 |
/// |---|---|---|
/// | **恰一条** | 绑上 ＋ 起 | **照旧**（"没有选择"不算替它决定） |
/// | **多条** | ⛔ 拒绝接上 | ✅ **接上，一个字节都不绑** —— 先不起它，选好再起 |
/// | **空** | ⛔ 拒绝接上 | ✅ **接上，不绑** —— 同上（"还没配"是个如实的状态，不是拦路的理由） |
/// | **读不出来** | ⛔ 拒绝 | **照旧拒绝**（fail-closed：真坏了，不猜） |
///
/// ⚠ **不违反 2026-10-07 那条裁决**（app 形态不继承模型来源）：来源仍然**只从页面来**
/// （页面写的清单），一个环境变量都不读；这一次动的只是"**要不要因为还没选而拒绝接上**"。
/// ⚠ **也不违反"早失败"那条纪律**：没模型就**不起进程**（不是"起起来再看着她认证不过"）——
/// 起不起的判据在 `local_residents::start_resident`（它本来就如实拒绝没模型的住户），
/// 这一步只是**不再把"接上"也一起挡掉**。
///
/// 返回：
/// - `Ok(Some((id, 一行)))` ＝ 替它绑上了这一条（只有"恰一条"会走到这里）；
/// - `Ok(None)` ＝ **没绑，也不该绑**（0 条 / 多条）—— 调用方**照旧接上，只是先不起**；
/// - `Err((原因, 几行))` ＝ 真拒绝（清单读不出来 / 绑定写不下去）。
fn choose_entry_if_unbound(
    home_dir: &Path,
    env_root: &Path,
) -> Result<Option<ChosenBinding>, BindingRefusal> {
    let entries = match model_sources::read_list(home_dir) {
        ListState::Loaded { entries } => entries,
        ListState::Unreadable { reason } => {
            return Err((
                RefusalReason::ModelBindingUnreadable,
                vec![format!(
                    "[安家] 这台机器的模型清单读不出来（{reason}）—— 去「模型配置」页重存一次再来；这次什么都没动"
                )],
            ))
        }
    };
    let chosen = match entries.len() {
        // **空清单不再拦接上**（2026-10-15）：如实说"还没配"，然后照旧把它接上（先不起它）。
        0 => {
            return Ok(None);
        }
        // **恰一条 ⇒ 替它绑上**（"没有选择"不算替它决定）。
        1 => &entries[0],
        // **多条 ⇒ 不绑也不拦**（2026-10-15）：先接上，选好再起。
        _ => return Ok(None),
    };
    // 环境根这会儿多半还没建（安家第 2 步才建它）。先建空目录再写绑定：写不成时留下的空目录
    // **不算住户**（`residents()` 要 `agent.conf` 或 `identity.json`），所以拒绝路径照样是"什么都没动"。
    if let Err(reason) = std::fs::create_dir_all(env_root)
        .map_err(|error| error.to_string())
        .and_then(|()| model_sources::write_binding(env_root, &chosen.id))
    {
        return Err((
            RefusalReason::ModelBindingUnreadable,
            vec![format!(
                "[安家] 这一支的模型绑定写不下去（{reason}）—— 检查这个家能不能写；这次什么都没起"
            )],
        ));
    }
    Ok(Some((
        chosen.id.clone(),
        format!(
            "[安家] 这一支还没有模型来源 ⇒ 这台机器上只有一条，就绑给它：{}（{} · {}）—— 想换去「模型配置」页",
            chosen.id, chosen.name, chosen.model
        ),
    )))
}

// ---------------------------------------------------------------------------
// 用例：**真拒绝路径**（跑的是 `run_anjia` 本体，不是拼出来的报告）
// ---------------------------------------------------------------------------
//
// 为什么要有这一组：`refusal` 子词是**在真路径上被填进去的**——只测 `view_of` 拼装
// 证明不了"哪条路给哪个子词"。这一组用**显式 scratch 目录**（`AnjiaRequest` 的
// `home_dir` / `repo_dir` 两个显式入口）把真路径走到拒绝那一支：不碰真家、不碰真凭据、
// 不连世界（这三条路都在"问世界"之前就拒绝）。跑完即删。

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// 一支**独立**的临时目录（照 `home.rs` 用例的形状；跑完即删）。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "celira-anjia-run-test-{}-{}-{}",
            std::process::id(),
            tag,
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 三支可执行件的**替身**（空文件即可——`detect_layout` 只认"在不在"）。
    fn fake_executables(repo: &Path) {
        for rel in [
            "target/release/celira-im",
            "agent/target/release/agent_host",
            "agent/target/release/agent_run",
        ] {
            let file = repo.join(rel);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, "stub").unwrap();
        }
    }

    /// 跑一次 `run_anjia`（显式目录，全离线）。
    fn run(home: &Path, repo: &Path, agent_id: &str) -> AnjiaReport {
        run_anjia(&AnjiaRequest {
            agent_id,
            home_dir: Some(home),
            repo_dir: Some(repo),
        })
    }

    fn cleanup(dirs: &[&Path]) {
        for dir in dirs {
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_real_refusal_path_carries_the_missing_executable_sub() {
        let home = temp_dir("missing-exe-home");
        let repo = temp_dir("missing-exe-repo"); // 空目录 ⇒ 三支可执行件都缺
        let report = run(&home, &repo, "agent-x");
        assert_eq!(report.outcome, AnjiaOutcome::RefusedPrecondition);
        assert_eq!(report.refusal, Some(RefusalReason::MissingExecutable));
        // 什么都没动：环境根 / 日志 / pid 全空。
        assert!(report.env_root.is_none() && report.log_file.is_none() && report.pid.is_none());
        assert!(
            report.lines.iter().any(|line| line.contains("缺可执行件")),
            "原样读数要说清缺什么：{:?}",
            report.lines
        );
        cleanup(&[&home, &repo]);
    }

    #[test]
    fn a_real_refusal_path_carries_the_home_not_set_up_sub() {
        let home = temp_dir("home-noconf"); // 没有 home.conf ⇒ 家还没布置
        let repo = temp_dir("home-noconf-repo");
        fake_executables(&repo);
        let report = run(&home, &repo, "agent-x");
        assert_eq!(report.outcome, AnjiaOutcome::RefusedPrecondition);
        assert_eq!(report.refusal, Some(RefusalReason::HomeNotSetUp));
        assert!(report.env_root.is_none() && report.log_file.is_none() && report.pid.is_none());
        cleanup(&[&home, &repo]);
    }

    #[test]
    fn a_real_refusal_path_carries_the_bad_id_shape_sub() {
        let home = temp_dir("bad-id-home");
        fs::write(home.join("home.conf"), "human_id=hum-not-real\n").unwrap();
        let repo = temp_dir("bad-id-repo");
        fake_executables(&repo);
        // 带空格的 id（`adopt.js` 的 refuse-bad-id 形状）——这一支也在问世界之前拒绝。
        let report = run(&home, &repo, "a b");
        assert_eq!(report.outcome, AnjiaOutcome::RefusedPrecondition);
        assert_eq!(report.refusal, Some(RefusalReason::BadIdShape));
        assert!(report.env_root.is_none() && report.log_file.is_none() && report.pid.is_none());
        cleanup(&[&home, &repo]);
    }

    /// ★ 「已经安家」时报出的**环境根**：只有 `home.conf` 的住户登记对得上时，她住的地方是
    /// **今天**的环境根（`<家>/agents/<id>/`），**不是** `<家>/env`（那是 2026-09-30 以前的
    /// 老家形状、只是迁移输入）。
    ///
    /// ⚠ **2026-10-18 加**：这一格此前**没有任何测试**——`home_facts.rs` 的 ③ 只断言
    /// `Plan::Already { .. }`、**不看路径** ⇒ 那条路径一直写着老家形状而没人发现。
    /// 判据一句话：**读数说"她住哪"，就必须是她真的住哪**（那是给用户看的）。
    #[test]
    fn already_homed_by_conf_reports_todays_env_root_not_the_old_shape() {
        let home = temp_dir("already-conf-home");
        // 只有住户登记：没有 `agents/<id>/identity.json`，也没有 `env/identity.json`。
        fs::write(
            home.join("home.conf"),
            "human_id=hum-not-real\nagent_id=resident\n",
        )
        .unwrap();
        let repo = temp_dir("already-conf-repo");
        fake_executables(&repo);
        let report = run(&home, &repo, "resident");
        assert_eq!(report.outcome, AnjiaOutcome::AlreadyHomed);
        assert_eq!(
            report.env_root,
            Some(home.join("agents").join("resident")),
            "住户登记那一路报出的环境根必须是**今天**的形状（<家>/agents/<id>/）"
        );
        cleanup(&[&home, &repo]);
    }

    // -----------------------------------------------------------------------
    // 还没有绑定时那一格（2026-10-09：主人第一次用 app 撞上的死锁）
    // -----------------------------------------------------------------------

    /// 写一份清单进这个家（只给判据要的那几格）。
    fn write_list(home: &Path, entries: &[(&str, &str)]) {
        let body: Vec<String> = entries
            .iter()
            .map(|(id, model)| {
                format!(
                    "{{\"id\":\"{id}\",\"name\":\"那一家\",\"endpoint\":\"https://example.invalid/v1/chat/completions\",\"api_key\":\"k\",\"model\":\"{model}\",\"context_window\":1000}}"
                )
            })
            .collect();
        fs::write(
            home.join("model_sources.json"),
            format!("[{}]", body.join(",")),
        )
        .unwrap();
    }

    fn binding_of(env_root: &Path) -> Option<String> {
        fs::read_to_string(env_root.join("model_binding.json")).ok()
    }

    #[test]
    fn a_machine_with_one_source_binds_it_to_the_new_agent_instead_of_refusing() {
        let home = temp_dir("unbound-one-home");
        write_list(&home, &[("ms-1", "deepseek-v4.1-flash")]);
        let env_root = home.join("agents").join("a-1");

        let (chosen, line) = choose_entry_if_unbound(&home, &env_root)
            .expect("清单有一条不是拒绝")
            .expect("清单有一条就该绑上");
        assert_eq!(chosen, "ms-1");
        assert!(line.contains("ms-1"), "那一行要点名绑的是哪一条：{line}");
        let written = binding_of(&env_root).expect("绑定必须真的落盘");
        assert!(written.contains("\"source\": \"ms-1\""), "{written}");
        // 落盘之后 `model_plan` 就该认得出这一支的来源（这才是"点一次安家能走通"的判据）。
        assert!(host_process::model_plan(&home, &env_root).is_ok());
        cleanup(&[&home]);
    }

    /// ★ **2026-10-15 主人裁：多条来源不再拦"接上"**（判据：接上 ≠ 选模型；不绑就不会偷偷花钱）。
    ///
    /// 从前这一格是 `Err(ModelNotChosen)` ⇒ **整个接上动作被拒**。现在：**一个字节都不绑**，
    /// 但也不拒 —— 接上照做，先不起它；选择权仍在它自己的页面上。
    #[test]
    fn several_sources_no_longer_block_homing_they_just_leave_it_unchosen() {
        let home = temp_dir("unbound-many-home");
        write_list(&home, &[("ms-1", "first-model"), ("ms-2", "second-model")]);
        let env_root = home.join("agents").join("a-1");

        let chosen = choose_entry_if_unbound(&home, &env_root).expect("多条来源不是拒绝");
        assert!(chosen.is_none(), "多条来源 ⇒ 一个字节都不绑（不替它决定）");
        assert!(
            binding_of(&env_root).is_none(),
            "**不许偷偷绑**：绑定文件不该出现"
        );
        assert!(
            !env_root.exists(),
            "这一步只判「绑不绑」，建家是后面那几步的事"
        );
        // 没绑定 ⇒ 模型计划仍然算不出来（起不起由 `start_resident` 如实拒绝，见那边）。
        assert!(host_process::model_plan(&home, &env_root).is_err());
        cleanup(&[&home]);
    }

    #[test]
    fn an_empty_list_no_longer_refuses_either_it_just_leaves_it_unchosen() {
        let home = temp_dir("unbound-empty-home");
        // 清单文件不在 ⇒ 空表（照 JS：ENOENT 是"还没配"，不是畸形）。
        let env_root = home.join("agents").join("a-1");
        let chosen = choose_entry_if_unbound(&home, &env_root).expect("空清单不是拒绝");
        assert!(chosen.is_none(), "空清单 ⇒ 不绑（也没得绑）");
        assert!(binding_of(&env_root).is_none(), "不许写绑定");
        assert!(!env_root.exists());
        cleanup(&[&home]);
    }

    #[test]
    fn an_unreadable_list_refuses_instead_of_guessing() {
        let home = temp_dir("unbound-broken-home");
        fs::write(home.join("model_sources.json"), "{ 这不是 JSON").unwrap();
        let env_root = home.join("agents").join("a-1");
        let (reason, lines) =
            choose_entry_if_unbound(&home, &env_root).expect_err("读不出来就得拒");
        assert_eq!(reason, RefusalReason::ModelBindingUnreadable);
        assert!(
            lines.iter().any(|each| each.contains("读不出来")),
            "{lines:?}"
        );
        assert!(binding_of(&env_root).is_none() && !env_root.exists());
        cleanup(&[&home]);
    }
}
