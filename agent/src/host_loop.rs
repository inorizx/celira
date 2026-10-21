//! 常驻宿主的**最小件**：门铃一响就**开一段在场**，这一段跑完回到监听。
//!
//! 它是"谁在叫醒主体"的第一件落地件，**不是**调度器：不排序、不批处理、不重试、不补偿、不做
//! 唤醒政策。三件事在这里定形：
//!
//! 1. **门铃不携带内容**：开段是**空输入**（主体要知道发生了什么，自己去看）；此后每一声音铃
//!    驱动的都是**同一段**（主体在场时的新动静由事件卡片那一格摆到眼前），**不是**每声铃一段。
//!    判据一句话：**铃只报告事实（「世界有动静」），不决定主体这一段在哪结束**——一段在场
//!    可以跨多声铃，收场只有一个来源：主体自己说结束（`finish_run`），或系统终止。
//!    「已经醒着的时候，一声铃不是再叫醒一次」就是这一条的落点。
//! 2. **活动一次一个**：门铃落在通道里等着（容量 1，重复铃合并），因此"Run 进行中铃不抢占"
//!    与"收场后恰好多一次"是**同一件事**的两面——本件只负责"等一次唤醒、跑一段到收场"，
//!    循环交给调用方（三行）。
//! 3. **起源如实**：通道接上、通道报变化、主体约的时间——三档各说各的话
//!    （[`WakeOrigin::Opened`] / [`WakeOrigin::Changed`] / [`WakeOrigin::Timer`]）。
//!
//! fail-closed 两路（都是**读数**，不是"重试一下就好了"）：
//!
//! - 唤醒通道被终结（[`HostFailure::Doorbell`]）：退出，报分类，不自动换凭据；
//! - 环境打不开 / 装配被拒（[`HostFailure::Environment`]）：退出报读数。
//!
//! ⚠ **准入那一路已经退役两次，今天不再是一路**（两刀的判据是同一条：**门是指令，而世界只该给信息**）：
//! `Unfinished`（有未收尾的活动）自 2026-09-26 起不是拒绝——"上一段没有收尾"由**开场那一轮**如实
//! 带给主体；`Paused`（已记录在案的保存失败）自 2026-10-06 起也不是拒绝——同一条故障由开场那一块
//! 纸条带给主体（`PreviousActivity` 的 `StorageFailure` 那一段），能不能继续由主体看着事实自己定。
//! 真写不下去时写路径照旧 fail-closed：那一轮在**第一次提交**（run-start，模型调用之前）就停住。
//!
//! **另有一路不是 fail-closed**：环境的**写资格**此刻被别人握着（[`HostFailure::EnvironmentBusy`]
//! 的成因）——那是**暂时**的，持有者总会放开，因此本件**等**它（见 [`WriteQualification`]），
//! 而不是把一次唤醒判死。判死的代价不对称：等一等最坏是这次唤醒晚几秒，
//! 判死是**常驻主体永久离线**（真机踩到过，见 [`WriteQualification`] 的文档）。
//!
//! **一个进程 = 一支 Agent = 一个环境**（`HostOptions` 的 `environment_root` 就是这一条）：多 Agent /
//! 多环境 / 宿主编排（启动、健康检查、回收）不在本件，那是 Host 那一层的横向部署编排
//! （`agent/README.md` §6「第一版明确不做」），**不是这一件的缺口**。
//!
//! ⚠ **如实登记的一格（未覆盖）**：世界**长期不可达**时，本件阻塞在 [`WakePort::next`] 里，这段时间
//! **一行读数都不产出**——它没有心跳，也没有有界等待（[`WakePort`] 只有"等下一次"，没有"等到某刻"）。
//! 操作者只能退回去看"进程在不在"。要那一行就得给 [`WakePort`] 加一个"等超时"的形状，那属于
//! **唤醒政策**（登记在 `agent/README.md` §4），不在本件。
//!
//! ⚠ **如实登记的第二格（未裁）**：一段在场可以跨很多声铃 ⇒ 进程被**硬杀**时丢掉的是**整段**：
//! 那一段没有收尾事实，下一段的开场会如实报「有一段没有正常收场」（见 [`crate::previous_activity`]，
//! 它**不冒充原因**）。在场变长之后这一档会从"罕见"变成"每次关机都有"。
//! **要不要让宿主在停机时写下一个系统终态，今天没有裁决**：词表里那一格
//! （[`SystemEnding::Crashed`]，"承载活动的进程崩溃"）**已经存在，但没有生产者**。
//! ⚠ **不拿主体的「搁下」顶它**：那是主体的词，不是系统替主体说的话。
//!
//! ## 真实读数（第一次全自动人机往返）
//!
//! 真 im ＋ 真模型端：人发一条消息 ⇒ 门铃响 ⇒ 自己醒、自己 `im_rooms` ＋ `im_read_history`、
//! 看到那条、`im_say` 回复（正文在盘上与 im 侧**逐字节相同**）、再自己 `im_verify_said` 复核
//! ⇒ 收场 ⇒ 回监听。期间一次真实的**传输级断连**（服务端被误杀）也由它自己接回来了：
//! 重连 ⇒ 又是一次 [`WakeOrigin::Opened`] 的唤醒（那正是"覆盖订阅间隙"）⇒ 空转 ⇒ 收 `silent`。

use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::composition::{
    CompositionError, RunOrigin, RunSession, SessionDeclarations, SessionDrive, SessionRequest,
};
use crate::core::{Limits, ModelPort, RunInput};
use crate::doorbell::DoorbellFailure;
use crate::foundation::{AgentId, Outcome, RunId, SystemEnding};
use crate::model_retry::ModelRetry;
use crate::timer_app::TimerApp;
use crate::wake::{WakePort, Woken};
use std::sync::{Arc, Mutex};

use crate::clock::{ClockHandle, unix_millis};
use crate::context_reading::ContextReading;
use crate::event_cards::{EventCardsReport, EventCardsWriter};
use crate::im_app::ImApp;
use crate::im_channel::ImAccess;
use crate::manual_entry::escape_field;
use crate::model_choice::ModelChoice;
use crate::store::{Admission, Environment, StoreError, WakeOrigin, outcome_label};

/// 宿主的声明：**全部来自命令行**，本件不加任何配置项（唤醒政策是主体的空间，不是配置）。
/// 「**现在有没有一次活动在跑**」这一格（宿主写、门铃的回调读）。
///
/// 它只做一件事：把**正在跑的那次活动的起点与它的细胞写柄**暂存起来，好让**铃回调**
/// （跑在读手线程上）知道该往哪儿写。活动收场即清空——细胞随之散掉（零持久）。
#[derive(Clone, Debug, Default)]
pub struct RunWatch {
    in_flight: Arc<Mutex<Option<(SystemTime, EventCardsWriter)>>>,
}

impl RunWatch {
    /// 造一格空的（没有活动在跑）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 一次活动开始：记下它的起点与本活动的细胞写柄，并交回一个 **RAII 句柄**。
    ///
    /// ⚠ **必须用返回的 [`WatchGuard`]**：收场由它的 `Drop` 兜底（见那边的文档）。丢掉句柄
    /// 就是立刻收场——这正是要的，"忘了 end"从此构造不出来。
    pub fn begin(&self, started: SystemTime, writer: EventCardsWriter) -> WatchGuard {
        // 新的一段：到达/渲染两格归零（它们与细胞同命——Run 级）。
        writer.reset_marks();
        *self.in_flight.lock().expect("在跑表只被短临界区持有") = Some((started, writer));
        WatchGuard {
            watch: self.clone(),
            ended: false,
        }
    }

    /// **读数**：这一刻有没有活动在跑（门铃的回调与收场读的是同一张表）。
    ///
    /// 只回答"有没有"，不回答"是谁"——起点与写柄不离开本模块。
    pub fn running(&self) -> bool {
        self.in_flight
            .lock()
            .expect("在跑表只被短临界区持有")
            .is_some()
    }

    /// 一次活动收场：清空（细胞随之散掉），并如实回答**这一段收场时世界还有没有没摆到眼前的动静**。
    ///
    /// ⚠ 必须在**取走写柄之前**问：细胞一散，那两格计数就没人记得了。
    pub fn end(&self) -> bool {
        let taken = self
            .in_flight
            .lock()
            .expect("在跑表只被短临界区持有")
            .take();
        match taken {
            Some((_, writer)) => writer.unshown(),
            None => false,
        }
    }

    /// 造一个**铃回调**给门铃：跑着就拉一次 B1（`since = 本次活动的起点`）并覆盖式并进细胞。
    ///
    /// 没跑着 ⇒ 什么都不做（那一次铃交给宿主循环去开新活动）。读失败 ⇒ 静默（细胞留旧值）。
    pub fn sink(&self, access: ImAccess) -> Arc<dyn Fn() + Send + Sync> {
        let watch = self.clone();
        Arc::new(move || {
            // `ImApp::new` 在不完整装配时给 `None`（端点或凭据缺一半）—— 那就静静地不做事。
            let Some(app) = ImApp::new(access.clone()) else {
                return;
            };
            let in_flight = watch
                .in_flight
                .lock()
                .expect("在跑表只被短临界区持有")
                .clone();
            let Some((started, writer)) = in_flight else {
                return;
            };
            // **记一次到达**（2026-10-13）：跑着的时候来了一声铃 ⇒ 先记下"世界动过"这个机械事实。
            // 它**不等于**"主体看见了"——看见的机械证据是卡片进过一次注入（`mark_rendered`）。
            writer.ring();
            if let Ok(rows) = app.new_since_rows(started) {
                writer.merge(&rows);
            }
        })
    }
}

/// 一次活动在跑这一格的 **RAII 句柄**（[`RunWatch::begin`] 的返回值）。
///
/// ## 它治的是哪一类缺陷（2026-10-20）
///
/// 从前 `begin` 与 `end` 是一对手写的调用：开段之后只要有一条早退路径（装配被拒、唤醒通道
/// 终结、任何一个 `?`）走掉，"在跑表"就会停在一个**死写柄**上——`end()` 到不了，此后每一
/// 声音铃都往一张已经散掉的细胞里写。那不是"少了一次收尾"，是**这一类路径还会再犯**。
///
/// 现在"开始计数"与"收场"由类型配对：`Drop` 里一定调 [`RunWatch::end`]。正常收场要读
/// "还有没有没摆到眼前的动静"，调 [`WatchGuard::end`]（消费它）；不读就由 `Drop` 清空。
///
/// ⚠ 它**不改变铃的语义**：一声铃仍然不是"一次唤醒"，锚的推进规则与容量 1 的合并一个字未动。
#[must_use = "丢掉它就是立刻收场；要读 unshown 请调 end()"]
pub struct WatchGuard {
    /// 在跑表的另一份句柄（`RunWatch` 是 Arc，克隆只加一次计数）。
    ///
    /// 它**自有**而不是借用：开段那一段还要可变借用宿主（`self.since` 等），借住 `self.options`
    /// 会把借用关系搞复杂——而这张表本来就是共享的。
    watch: RunWatch,
    ended: bool,
}

impl WatchGuard {
    /// 收场：清空在跑表，并如实回答"这一段收场时世界还有没有没摆到眼前的动静"。
    ///
    /// ⚠ 必须在**取走写柄之前**问：细胞一散，那两格计数就没人记得了（与从前的 `end` 同一条）。
    pub fn end(mut self) -> bool {
        self.ended = true;
        self.watch.end()
    }
}

impl Drop for WatchGuard {
    fn drop(&mut self) {
        if !self.ended {
            // 早退路径：清空即可。没有读者在这里问 unshown——那一段没有正常收场。
            self.watch.end();
        }
    }
}

/// 取得环境**写资格**的耐心：别人此刻正握着（`already-open`）时**等**它放开，而不是把这次唤醒判死。
///
/// **为什么需要它**（2026-09-26 真机踩到，代价是常驻主体永久离线）：家的**只读状态口**
/// （`home.js status --serve`）每读一次本机状态就 spawn 一次 `agent_host_status`，而按既有规则
/// ——`agent/README.md` 登记的「按既有写锁规则打开……**没有共享读锁方案**」——它拿的是
/// **同一把排他写资格锁**，并且要把整份账本解码一遍。这个家的账本长到**一次读 11.3 秒**，
/// 而状态口的 TTL 是 15 秒 ⇒ 只要页面在读本机状态，那把锁有约四分之三的时间被一个**只读**的
/// 观察窗握着。于是"打开页面 → 发一条消息 → 门铃响"这条最普通的路径里，唤醒撞上读的概率极高，
/// 而唤醒撞上就 `environment-unusable` 退出 ⇒ **一个只读的观察窗可以永久杀死常驻主体**。
/// 真机读数：`[exit] reason=environment-unusable detail=already-open` 与那条消息的创建**同一秒**。
///
/// ⚠ **2026-10-06 更正两件事**（都来自长跑读数那一刀）：
/// ① **那个危险的观察窗没有了**：`agent_host_status` / `environment_diagnose` 改成**读者打开**
///    （只读句柄，不取写资格，见 `agent/README.md` §3.3）⇒ "只读窗口握锁"这条成因消失。
///    `WriteQualification` 仍然需要——它今天挡的是**另一个写者**（另一个宿主实例 / `agent_run` /
///    承认命令），那一类同样是暂时的。
/// ② **"11.3 秒"主要是等锁，不是解析**：合成历史实测（`tests/durability_reading.rs`）里
///    3 MB 的账本解析只要约 0.2 秒、约 60–70 ms/MB。⇒ 耐心那一头该按"**写者**会握多久"算，
///    不再按"一次读要多久"算（见下）。
///
/// **判据**：`already-open` 对**唤醒**而言是**暂时**的——持有者总会放开。它与"环境坏了 /
/// 不属于这个 Agent / 存储读不出来"是两类：那三类等多久都不会变（立刻返回，fail-closed），
/// 这一类的正确反应是等。代价不对称：等一等最坏是这次唤醒晚几秒，判死是主体永久离线。
///
/// **本件不动 `store` 的锁规则**：那条"没有共享读锁方案"是已登记的限制，要改它得先裁，
/// 不是这里顺手能改的。因此这一刀只在**写者那一侧**加耐心——读侧一个字不动。
#[derive(Clone, Copy, Debug)]
pub struct WriteQualification {
    /// 最多等多久（等满就如实报 [`HostFailure::EnvironmentBusy`]，不假装环境坏了）。
    pub patience: Duration,
    /// 两次尝试之间隔多久。尝试本身很便宜：撞锁时 `open` 在拿锁那一步就返回，不解码账本。
    pub retry: Duration,
}

impl Default for WriteQualification {
    fn default() -> Self {
        // 90 秒。⚠ **2026-10-06 更正理由**：原来这里写的是"≈ 八次'11.3 秒读'的窗口"，而那个
        // 11.3 秒**主要是等锁**、不是解析（合成历史实测：3 MB 账本解析约 0.2 秒、约 60–70 ms/MB）。
        // 今天读者不取锁了（见上），因此这一档挡的只有**另一个写者**——而写者一握就是**一整段活动**
        // （模型调用串起来，可能是分钟级）。90 秒给的是"它可能刚好在收尾"的余量：等满就如实报
        // `environment-busy`，让操作者去看**谁在里面**，而不是无限等下去。
        Self {
            patience: Duration::from_secs(90),
            retry: Duration::from_millis(500),
        }
    }
}

pub struct HostOptions<'a> {
    /// 环境根（一次一环境）。
    pub environment_root: &'a Path,
    /// 调用方声明的身份：与环境里记录的权威身份不一致就拒绝启动。
    pub declared_agent: &'a AgentId,
    /// 授权资料根（主体的资料边界）。
    pub material_root: &'a Path,
    /// 这一次活动最多几次模型调用。
    pub max_turns: u32,
    /// 宿主声明的上下文窗口（状态行 `context` 段的分母）。
    pub context_window: u32,
    /// 取得环境**写资格**的耐心（见 [`WriteQualification`]）。
    ///
    /// 它是**本件的策略**、不是命令行配置：生产走 `Default`，用例给短的。
    /// 放在这里而不是写成常量，是因为"等多久算够"必须能被用例注入——
    /// 否则那条路径只能靠真机 11 秒级的等待来验。
    pub write_qualification: WriteQualification,
    /// 时间来源：每一次活动各持一份（本件从同一句柄克隆）。
    pub clock: ClockHandle,
    /// 屏幕报告 cell 的读侧（与适配层共用的那一格）。
    pub reading: ContextReading,
    /// **宿主声明的可选那几格**（一个包，见 [`SessionDeclarations`]）：模型名（与窗口同源，
    /// 由入口从模型配置门的裁决里拿，**本件不猜**）／用量接线／im 访问／身份行／搜索／
    /// 终端档位／MCP。
    ///
    /// ⚠ **im 那一格在本层是必需的**（门铃与主体的 im 动词用的是**同一份声明**：端点与凭据
    /// 都只有那一份），但类型上与别的声明同款：装着它的包是可选的。入口
    /// （`bin/agent_host.rs`）总是把它填上；`None` 的含义仍是"这一次装配没有 im 动词"。
    /// 每一次活动**克隆一整包**交给会话（包是会话规格里可选的那一半）。
    pub declarations: SessionDeclarations,
    /// **在跑的那次活动**（`None` = 现在没跑）：宿主在跑着时听到铃就往它的细胞里写计数。
    /// 它与门铃的铃回调是**同一条**（bin 用 `RunWatch::sink(access)` 造回调）。
    pub watch: RunWatch,
}

/// 宿主**退出**的原因。常驻进程只在 fail-closed 时退出，因此退出码就是"为什么退"。
#[derive(Debug)]
pub enum HostFailure {
    /// 环境打不开 / 装配被拒（根不在、身份不一致、资料根不可用、存储读不出来）。
    Environment(String),
    /// 环境的**写资格**被别人握着，等满耐心仍没放开。
    ///
    /// **与 [`HostFailure::Environment`] 是两类，不能合并**：那里是"这个环境坏了 / 不属于这个
    /// Agent / 存储读不出来"——重试无用，动作是查环境；这里是"环境好着，只是此刻有别人在里面"
    /// ——它**暂时**，只是已经超出这次唤醒愿意等的上限，动作是查**谁在里面**。
    /// 合并会让操作者去修一个没坏的环境，而那正是这条读数唯一要说清的事。
    ///
    /// 之所以还会走到"等满"：持锁的那个只读窗口（`agent_host_status`）本身要几秒到十几秒，
    /// 而耐心是按账本增长给的（见 [`WriteQualification`]）。等满说明**里面那个持有者活得比
    /// 一次读还久**——那是另一个写者，是真的需要人看一眼的事。
    EnvironmentBusy { waited_ms: u64 },
    /// 唤醒通道被服务端终结（凭据级被拒 / 超限 / 不可用的答复）。
    Doorbell(DoorbellFailure),
}

impl HostFailure {
    /// 退出码是**读数语义**：`2` 环境不可用 ／ `4` 唤醒通道终结 ／ `6` 环境写资格被别人握着。
    ///
    /// `5` 不在这里：它由入口自己用（模型配置读不出来），不属于"宿主为什么退"这一族。
    ///
    /// ⚠ **2026-10-06 改**：`3`（需要用户动作）**随暂停那道闸一起退役**——暂停不再是拒绝，
    /// 它是**事实**（由开场那一块纸条带给主体）。今天没有"需要你动手"这一档：
    /// 装配失败一律归 `2`（环境不可用），而"环境已暂停"照旧由 `[state] admission=` 那一行说出来。
    pub fn code(&self) -> i32 {
        match self {
            HostFailure::Environment(_) => 2,
            HostFailure::Doorbell(_) => 4,
            HostFailure::EnvironmentBusy { .. } => 6,
        }
    }

    /// 稳定分类标记（进读数）。
    pub fn as_str(&self) -> &'static str {
        match self {
            HostFailure::Environment(_) => "environment-unusable",
            HostFailure::EnvironmentBusy { .. } => "environment-busy",
            HostFailure::Doorbell(DoorbellFailure::ChannelLost) => "channel-lost",
            HostFailure::Doorbell(failure) => failure.as_str(),
        }
    }

    /// `[next]`：**恰好一行**，而且**只指向真能走的下一步**。它**不**自动重试、不代敲命令。
    pub fn next_line(&self, root: &Path, agent: &AgentId) -> String {
        let root = escape_field(&root.display().to_string());
        let agent = escape_field(agent.as_str());
        match self {
            HostFailure::Environment(message) => format!(
                "环境不可用（{}）：先用 `environment_diagnose --root {} --agent {}` 看清原因，再重启本进程",
                escape_field(message),
                root,
                agent
            ),
            HostFailure::EnvironmentBusy { waited_ms } => format!(
                "环境的写资格被别人握着（等了 {waited_ms} ms）：先用 `agent_host_status {} --agent {}` 看清此刻是谁在里面（它读得到就说明已经放开），确认没有第二个写者之后再重启本进程",
                root, agent
            ),
            HostFailure::Doorbell(DoorbellFailure::CredentialRejected) => "唤醒通道被拒（凭据级）：检查 im 的 BackendAuth 凭据是否还有效（变量名在入口的说明里），然后重启本进程（本进程不自动换凭据）".to_string(),
            HostFailure::Doorbell(DoorbellFailure::ChannelLimitExceeded) => "唤醒通道超限：这个 Agent 的并发唤醒通道已达上限，先停掉别的宿主再重启本进程".to_string(),
            HostFailure::Doorbell(failure) => format!(
                "唤醒通道终结（{}）：确认 `--im-endpoint` 指向的是 Celira IM，然后重启本进程（本进程不自动重试）",
                failure.as_str()
            ),
        }
    }
}

/// 等到**写资格**空出来再打开环境：别人此刻正握着就**等**，而不是把这次唤醒判死。
///
/// **只有 `already-open` 值得等**——它是"此刻有别人在里面"，暂时。其余错误立刻返回：根不在、
/// 身份不一致、存储读不出来，那些等多久都不会变，等它们就是拿一次唤醒换一次空转。
///
/// 启动准入（`agent_host` 的 `[state]` 那一次）与每一次唤醒都走这里：**同一个成因只有一个修法**。
/// 不复制执行链——两处调用同一个函数。
pub fn open_when_free(
    root: &Path,
    agent: &AgentId,
    policy: WriteQualification,
) -> Result<Environment, HostFailure> {
    let started = Instant::now();
    loop {
        match Environment::open(root, agent.clone()) {
            Ok(environment) => return Ok(environment),
            Err(StoreError::AlreadyOpen) => {
                let waited = started.elapsed();
                if waited >= policy.patience {
                    return Err(HostFailure::EnvironmentBusy {
                        waited_ms: waited.as_millis() as u64,
                    });
                }
                std::thread::sleep(policy.retry);
            }
            Err(error) => return Err(HostFailure::Environment(error.as_str().to_string())),
        }
    }
}

/// **一段在场跑完之后的读数**：那一段**已经收场**。
///
/// ⚠ **名字为什么不是 `Wake`**（2026-10-10 在场那一刀）：开这一段的是**一次唤醒**，而这一段
/// 此后还会**吸收后来的每一声音铃**（每一声音铃驱动**同一段**）。因此这个值描述的是**一段**，
/// 不是"一次唤醒"——用 `Wake` 当它的名字会让读数说一件不是它的事。
///
/// ⚠ **名字也不许用旧 Runtime 那个词**：它只是 `Run` 的**解释**，不是它的名字（见
/// `docs/runtime/terminal/RUNTIME_TERMINAL_DECISIONS.md` §9），而且它在**中立层是排除词**
/// ——词表守卫会红（那一条守卫扫的正是中立层源码里的旧术语与供应商语言）。
#[derive(Debug)]
pub struct RunReading {
    /// 这一段活动的标识（宿主机械生成的）。
    pub run: RunId,
    /// 收场结果。
    pub outcome: Outcome,
    /// 这一段被驱动了几次（`1` = 开段那一次就收场；大于 1 = 中间停过等待，被后来的唤醒再驱动过）。
    pub drives: usize,
    /// **这一段收场的那一刻**（宿主钟给的）。常规路它就是**下一次唤醒的起点**（下一次的投影从
    /// 这一刻算起，状态行与通知栏都渲染它）：相邻两次投影因此把时间切成互不重叠、也不留缝的窗口。
    ///
    /// ⚠ **不是"去看投影的那一刻"**（那一版 2026-10-10 改掉了，见 [`next_anchor`]）；而这一段
    /// **还有没摆到眼前的动静**时锚只推到这一段**开头**——那一档的权威是 [`next_anchor`]，
    /// 不是本字段。
    ///
    /// 注意它**不等于**"这一刻主体眼前那个 `since`"：主体看到的是**上一次**取下的那个（本次窗口的起点）。
    pub anchor_at: SystemTime,
    /// **这一段里发生过几次模型重试**（TASK_60 裁决 5）。
    ///
    /// 为什么由宿主打这一行、而不是库自己打：宿主日志就是这个进程的 stdout/stderr，
    /// 而库打印会变成 TASK_51 删过的那种"侧信道"形状 ⇒ 记录**交上来**，由这里（入口层）说出去。
    /// 它是**诊断**不是事实：不进 `facts.ndjson`、不参与任何判断。
    pub model_retries: Vec<ModelRetry>,
    /// 这一段**读到投影了吗**（宿主读数行里就是 `notification=read|none` 这一格）。
    ///
    /// `false` = **这一次没读到**：读失败了（宿主侧因此如实写 `counts = unknown`），或这一次
    /// 装配没有 im（不完整装配 ⇒ 没有可读的投影）。⚠ **"没有锚"不算**——2026-10-10 起没锚
    /// **也**读（增量那一半问不出来，未读那一半照给，见 [`ImApp::new_since`]）；从前那句
    /// "锚未知就**不读**（⇒ 零请求）"是已退役的形状，别照着它写用例。
    pub notification: bool,
    /// **这一段收场时，世界还有没有没摆到眼前的动静**（2026-10-13）。
    ///
    /// `true` = 这一段开场之后世界动过，而那次动静**一次注入都没进过**（细胞随这一段散掉）
    /// ⇒ 它既不在本段的卡片里、也不在下一段的窗口里（锚已经推到收场时刻）。
    /// 宿主据此**补响一段**（`bin/agent_host.rs` 的循环顶上），并把锚只推到场开头那一刻。
    pub unshown: bool,
}

impl RunReading {
    /// 一段在场的**稳定读数行**（宿主每跑完一段打一行）。
    ///
    /// ⚠ **它从前以 `wake ` 开头**（2026-10-10 在场那一刀改）：一次唤醒不再等于一段，
    /// 所以那一行说的不是"唤醒"。**`run=<标识>` 那一格两版都在**——前端解时间用的正是
    /// `run=(\S+)`（`client/src/lib/agent-days.ts`），因此那条读法一字未改。
    pub fn reading(&self) -> String {
        format!(
            "run={} drives={} ending={} notification={} unshown={}",
            escape_field(self.run.as_str()),
            self.drives,
            ending_text(self.outcome),
            if self.notification { "read" } else { "none" },
            if self.unshown { "yes" } else { "no" }
        )
    }
}

/// **锚推进到哪里**（2026-10-13 抽成纯函数：它是判据，必须可判错）。
///
/// 三档，按"这一段到底看没看过"分：
///
/// - **没读成**（`read == false`，且本来就有锚）⇒ **不动**：窗口继续延伸，下次一并报出来
///   （"不漏报"那一侧）；
/// - **读成过，但这段期间的动静还有没摆到眼前的**（`unshown`）⇒ 只推到**这一段开头那一刻**
///   （`looked_at` ——那一次它确实看过）：这段期间的动静留给下一段；
/// - **其余**（读成过、且没有没摆到眼前的）⇒ 推到场收场时刻（常规路）。
///
/// ⚠ 抽出来是因为它判错过一次：从前只看 `read`，等于假设"读过了就是看完了"——那在一段活动
/// 死在第 1 轮（模型失败 / 崩溃）时不成立，代价是那条消息**两个窗口都不落**（真机实测）。
fn next_anchor(
    read: bool,
    previous: Option<SystemTime>,
    unshown: bool,
    looked_at: SystemTime,
    settled_at: SystemTime,
) -> Option<SystemTime> {
    if !read && previous.is_some() {
        previous
    } else if unshown {
        Some(looked_at)
    } else {
        Some(settled_at)
    }
}

/// 唤醒源的答复 → 宿主的下一步：铃/到点就是一次唤醒，通道终结就是一条**退出读数**。
///
/// **一处拥有这个判据**：等第一声铃（[`Host::next_wake`]）与在场期间等下一声铃
/// （[`Host::run_segment`]）走的是同一个函数——两处各写一遍迟早会分叉，而分叉的那一天
/// 症状是"开机起得来、在场却把通道断了当没事"。
fn accept(woken: Woken) -> Result<Woken, HostFailure> {
    match woken {
        Woken::Refused(failure) => Err(HostFailure::Doorbell(failure)),
        woken => Ok(woken),
    }
}

/// 结局的稳定文本（`subject:*` / `system:*`）。
///
/// 词与形状的权威在 `store`（那里有盘上的封闭表），这里只做**一件事**：模型失败时在那份文本
/// 后面**加上分类**（`system:model-failed model-failure=<class>`）——因为分类是这一层才有的读数，
/// 而基础那份文本不该被抄第二遍。
fn ending_text(outcome: Outcome) -> String {
    let label = outcome_label(outcome);
    match outcome {
        Outcome::System(SystemEnding::ModelFailed(Some(class))) => {
            format!("{label} model-failure={class}")
        }
        _ => label,
    }
}

/// 活动标识的**机械**生成器：`host-<进程号>-<毫秒>-<序号>`。
///
/// 唯一性由三段一起给：同一个进程里序号只增；跨进程重启时进程号与毫秒都对上才可能撞，而那是
/// **假设**、不是保证。真撞了也不会静默——同一个标识再登记会被存储拒绝（那条路径已有用例），
/// 宿主把它当成一次环境读数退出，**不换名、不重试**。
pub struct RunNamer {
    process: u32,
    next: u64,
}

impl Default for RunNamer {
    fn default() -> Self {
        Self::new()
    }
}

impl RunNamer {
    pub fn new() -> Self {
        Self {
            process: std::process::id(),
            next: 0,
        }
    }

    /// 铸一个新的活动标识。
    pub fn next_run_id(&mut self) -> RunId {
        self.next += 1;
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or(0);
        RunId::new(format!("host-{}-{millis}-{}", self.process, self.next))
    }
}

/// 常驻宿主的循环**体**：**等一次唤醒，跑一次活动**。
///
/// 它自己不循环（调用方三行）：这样"Run 进行中铃不抢占"与"收场后恰好多一次"就是通道容量与
/// 顺序调用的**机械后果**，而不是本件里的调度策略。
pub struct Host<'a> {
    options: HostOptions<'a>,
    names: RunNamer,
    /// **「新」的起点**（本进程内记的账；还没跑过任何活动就是 `None`）。
    ///
    /// 门铃只说「有新的」，主体知道「新」从哪个时刻起算，才能用 `im_rooms(since=…)` 只问增量。
    /// ⚠ **它不一定是"去看投影的那一刻"**：往哪儿推由 [`next_anchor`] 按三档决定——常规路推到
    /// **这一段收场**那一刻（2026-10-10 改；从前推到去看那一刻，于是主体在场时到的消息落到下一次
    /// 窗口里，它为已经答过的消息白醒一段），没读成则**不动**，读成过但这段还有没摆到眼前的则只推
    /// 到**这一段开头**。判据（"不漏报"那一侧）与那次实测见 [`next_anchor`]。
    /// 跨进程重启后如实回到 `None`（不编造）。
    since: Option<SystemTime>,
    /// **合并窗口**（2026-10-19）：头一声铃之后，再等这么久才开段，把同一批动静合成一次。
    ///
    /// `ZERO` ＝ **不合并**——[`Host::new`] 的默认值，既有行为一个字节不变，离线用例也不因此变慢。
    /// 生产入口显式打开它（`bin/agent_host.rs`）：没有它，"默认收到群消息"就等于每条消息一次
    /// 模型调用。
    coalesce: std::time::Duration,
}

impl<'a> Host<'a> {
    pub fn new(options: HostOptions<'a>) -> Self {
        Self {
            options,
            names: RunNamer::new(),
            since: None,
            coalesce: std::time::Duration::ZERO,
        }
    }

    /// 打开**合并窗口**：头一声铃之后最多再等这么久，把同一批动静合成一次开段。
    ///
    /// 默认关着（见字段文档）：只有常驻入口该打开它。用例给几十毫秒只为把窗口本身量出来。
    pub fn with_coalesce(mut self, window: std::time::Duration) -> Self {
        self.coalesce = window;
        self
    }

    /// 换一次**模型那一组声明**：模型名 / 思考强度 / 窗口（`--context-window` 那一件在内）。
    ///
    /// 为什么是**一组**、为什么要有它（TASK_77 的模型配置门）：模型源可以在**唤醒边界**换
    /// （下一次清醒用新配置），而这三格**都随模型走**——窗口是模型自己的标称值、思考强度是
    /// 清单那一条声明的档、名字就是它本身。**分开换**会留下"名字换了、档位还是上一份"这种
    /// 自相矛盾的读数（主体读到的那一行与真正发出去的请求对不上）。
    ///
    /// 值**从将要发请求的那个对象取**（[`ModelChoice::model`] / [`ModelChoice::reasoning_effort`]）：
    /// 启动那一处（`bin/agent_host.rs` 构造 `HostOptions`）取的是同一个来源，两处不会分家。
    ///
    /// **换一次装配，只在开段时生效**：生效面只有一处——[`Host::run_segment`] 开段那一刻从
    /// `options` 现读一次，而后整段**钉住** ⇒ 进行中的活动拿的是它开始那一刻的值，**一次活动内部
    /// 不会中途换档**。调用方因此只该在**两次活动之间**调它（一段返回之后、下一次之前）。
    ///
    /// ⚠ **唤醒边界那个钩子换不掉它**，而且是**结构上**换不掉（[`Host::run_woken_with`]）：调它
    /// 那一刻宿主已被 `&mut self` 借住，钩子**拿不到宿主**。⇒ 「在场期间改配置换不掉正在跑的那
    /// 一段」不是调用方的自觉，是这条借用关系；那时能做的只有**读出来给操作者**——主体看不到
    /// （那一半欠着，见 `agent/README.md` §3.66 的如实登记第 1 条）。
    pub fn declare_model_assembly(&mut self, model: &ModelChoice, window: u32) {
        self.options.declarations.model = Some(model.model().to_string());
        self.options.declarations.reasoning_effort = model.reasoning_effort().map(str::to_string);
        self.options.context_window = window;
    }

    /// **等一次唤醒**（连接建立或铃响）——**只等，不跑**。
    ///
    /// 为什么把"等"与"跑"拆开（2026-10-09 真机实测换来的）：模型配置的**边界重判**从前在
    /// 调用方循环的**顶上**，而那一刻宿主**正阻塞在等铃里** ⇒ 用户在页面上改完模型、紧接着的
    /// **那一段清醒期仍然用旧的**（第二条消息才换）——而页面写着"改完不用重启"。拆开之后，
    /// "**铃已到、还没开跑**"就是那个检查点 ⇒ 改完之后的**下一段清醒期**就用新的。
    pub fn next_wake(&mut self, wake: &impl WakePort) -> Result<Woken, HostFailure> {
        let first = accept(wake.next())?;
        Ok(self.coalesce_window(first, wake))
    }

    /// **合并窗口**（2026-10-19）：头一声之后，在窗口内继续收，把同一批动静合成**一次**开段。
    ///
    /// 为什么需要它：默认改成"群里有人说话也会叫醒主体"之后，一声一声地开段就等于**一条消息一次
    /// 模型调用**。窗口买的不是"数铃"，是"**等一小会儿**"——窗口内的动静要么已经在通道里
    /// （容量 1，多半已被合并），要么已经在世界里（下一次读账看得见）。
    ///
    /// 三条不退让的：
    /// 1. **钟必须赴**：窗口里到了主体自己约的时刻 ⇒ **立刻**开，起源记 `Timer`（不被铃压住）；
    /// 2. **通道终结立刻报**：`Refused` 不等到窗口结束——那只会让操作者晚知道；
    /// 3. **起源取最早的**：窗口里又来的铃只说明"还有动静"，不改"是谁叫醒的"。
    fn coalesce_window(&self, first: Woken, wake: &impl WakePort) -> Woken {
        let window = self.coalesce;
        if window.is_zero() || !matches!(first, Woken::Opened | Woken::Changed) {
            return first;
        }
        let deadline = std::time::Instant::now() + window;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return first;
            }
            match wake.next_within(remaining) {
                None => return first,
                Some(Woken::Timer) => return Woken::Timer,
                Some(Woken::Refused(failure)) => return Woken::Refused(failure),
                // 同一批里的又一声铃：丢掉——"还有动静"这件事已经说过了。
                Some(Woken::Opened) | Some(Woken::Changed) => {}
            }
        }
    }

    /// 用**已经到手**的那次唤醒开一段在场（[`Host::next_wake`] 之后调它）。
    ///
    /// ⚠️ 它**不返回于第一次驱动**：开这一段的那次唤醒之后，**每一声音铃都驱动同一段**
    /// （见 [`Host::run_segment`]），因此它返回时这一段已经**收场**。
    /// 在场期间要等下一次唤醒，所以调用方要把**唤醒源**也交进来。
    ///
    /// ⚠️ **它不带唤醒边界的钩子**：调用方若要在"铃已到、下一轮还没开始"那一刻做一件事
    /// （宿主二进制要做的正是那件事：重判模型来源），用 [`Host::run_woken_with`]。
    pub fn run_woken<M: ModelPort>(
        &mut self,
        woken: Woken,
        wake: &impl WakePort,
        model: &mut M,
    ) -> Result<RunReading, HostFailure> {
        self.run_woken_with(woken, wake, model, &mut |_| Ok(()))
    }

    /// 同 [`Host::run_woken`]，外加**每一次唤醒边界**上的钩子。
    ///
    /// 钩子在"**铃已到、下一轮还没开始**"那一刻被调一次（开段那一次**不调**——调用方在调本函数
    /// 之前刚判过）。它拿得到模型对象，因此调用方能在那里做它自己那一件事；⚠️ **但它拿不到宿主**
    /// ⇒ 那里面**换不了这一段的装配**（装配在开段那一刻钉住，见 [`Host::declare_model_assembly`]）。
    ///
    /// ⚠️ **它不决定这一段收不收场**：钩子的返回值只有两种——`Ok`（继续驱动同一段）与
    /// `Err(HostFailure)`（宿主退出）。**"要不要收场"归主体**，钩子没有那个权力。
    pub fn run_woken_with<M: ModelPort>(
        &mut self,
        woken: Woken,
        wake: &impl WakePort,
        model: &mut M,
        boundary: &mut dyn FnMut(&mut M) -> Result<(), HostFailure>,
    ) -> Result<RunReading, HostFailure> {
        self.run_segment(woken, wake, model, boundary)
    }

    /// 等一次唤醒（连接建立或铃响），开一段在场并跑到它收场。
    ///
    /// ⚠️ **2026-10-10 改（在场那一刀）**：从前这里"一次唤醒 = 一次活动"，而主体一交回控制
    /// 宿主就把这一段封掉 ⇒ **每一声铃都开一段新活动**、上下文每段从零起。那不是主体的边界，
    /// 是**门铃的边界**：铃在传输层上只说"有新动静"，它不该顺手切断主体的在场。
    /// 现在开这一段的那次唤醒之后，**后来每一声音铃都驱动同一段**（在场期间的新动静由
    /// 事件卡片那一格摆到眼前），收场只有一个来源：主体自己说结束，或系统终止。
    ///
    /// ⚠️ 等一次唤醒——**来自哪个源都行**：im 门铃（[`crate::doorbell::Doorbell`]）或本地钟源（[`crate::wake::WakeHub`]）。
    /// 两者都只回答"何时开始一次活动"，而"是谁叫醒的"由 [`Woken`] 带给起源行。
    ///
    /// ⚠️ 只跑门铃的路径（离线用例、没约过时间的机器）直接传 `&Doorbell`：它自己就是
    /// [`WakePort`]，因此那条路径**一个字节都不用改**。
    ///
    /// ⚠️ **它只是 [`Host::next_wake`] ＋ [`Host::run_woken`] 的组合**：中间不留缝隙，因此
    /// **不**适合"在活动开始之前还要做一件事"的调用方（宿主二进制要做的正是那件事：重判模型来源）
    /// —— 那种调用方直接调那两个，别调这个。
    pub fn wake_and_run<M: ModelPort>(
        &mut self,
        wake: &impl WakePort,
        model: &mut M,
    ) -> Result<RunReading, HostFailure> {
        let woken = self.next_wake(wake)?;
        self.run_woken(woken, wake, model)
    }

    /// **一段在场**：开这一段 → 驱动 → 未收场就**等下一次唤醒再驱动同一段** → 直到收场。
    ///
    /// ## 它为什么是一个循环（2026-10-10 在场那一刀）
    ///
    /// 从前这里是"一次唤醒跑一次活动"，而主体交回控制（`DriveOutcome::Unsettled`）时宿主
    /// **立刻把这一段封掉**（`session.hold()` ⇒ `subject:held` ⇒ 释放活动槽）。后果是：
    /// **每一声铃都开一段新活动**，上下文每段从零起——真机那一晚 10 段、平均 44 秒，
    /// 上一段说过的话一个字都带不过来。
    ///
    /// 那不是主体的边界，是**门铃的边界**。铃在传输层上只是一个 SSE 事件，它该说的只有
    /// 「有新的」；让它顺手切断主体的在场，等于让电线决定主体什么时候睡。
    ///
    /// ## 三条纪律
    ///
    /// - **收场只有一个来源**：主体自己说结束（`finish_run` ⇒ `Ended`），或系统终止
    ///   （模型失败 / 取消 / 超时 / 崩溃）。**宿主不再替主体说"搁下"**；
    /// - **在场期间不动**：`Unsettled` ⇒ 等**下一次唤醒**（门铃或主体自己约的时刻）再驱动同一段。
    ///   新动静由事件卡片那一格摆到眼前（宿主跑着时听到铃就拉一次并覆盖式并进细胞）；
    /// - **唤醒通道终结 ⇒ 如实退出，不替这一段收场**：通道断了是宿主走了，不是主体结束了
    ///   —— 那一段因此**没有收尾事实**，下一次开机的开场纸条会如实报"有一段没有正常收场"。
    ///   给系统终止补一个终态会让那句纸条变成假话。
    ///
    /// ⚠ 写资格那一步**会等**（[`WriteQualification`]）：别人此刻正握着不是"环境不可用"，
    /// 而是一次唤醒晚几秒。从前这里直接把 `already-open` 判死，于是家的只读状态口
    /// ——它按既有规则拿同一把排他锁、一次读十几秒——能把常驻主体永久杀死。
    fn run_segment<M: ModelPort>(
        &mut self,
        woken: Woken,
        wake: &impl WakePort,
        model: &mut M,
        boundary: &mut dyn FnMut(&mut M) -> Result<(), HostFailure>,
    ) -> Result<RunReading, HostFailure> {
        let options = &self.options;
        let run = self.names.next_run_id();
        let mut environment = open_when_free(
            options.environment_root,
            options.declared_agent,
            options.write_qualification,
        )?;
        let request = SessionRequest {
            declared_agent: options.declared_agent,
            run: run.clone(),
            limits: Limits::new(options.max_turns),
            // 门铃**不携带内容**：这一次活动的开场就是空的。
            input: RunInput::empty(),
            clock: options.clock.clone(),
            context_window: options.context_window,
            reading: options.reading.clone(),
            // **可选的那几格整包交给会话**：模型名（这一段由谁想）、im 访问、身份行、搜索、
            // 终端档位、MCP、用量接线各格的语义在包自己身上——本处只搬运，不解释。
            //
            // **外部工具的实际面**（2026-10-20）：宿主声明 ＋（**开了终端档时才读**）主体自己的
            // `<环境根>/mcp.conf`。判据与理由在 [`crate::mcp::declarations_for_run`]：自配 MCP
            // 不新增权力（有终端档时主体本来就能跑任意命令），没开时它会是绕过终端档的后门。
            // ⇒ 生效点是**每一段活动**，不是进程重启——主体改自己的清单，下一段就多一台。
            declarations: {
                let (mcp, notes) = crate::mcp::declarations_for_run(
                    &options.declarations.mcp,
                    options.environment_root,
                    options.declarations.terminal.is_some(),
                );
                for note in notes {
                    eprintln!("[mcp] {note}");
                }
                let mut declarations = options.declarations.clone();
                declarations.mcp = mcp;
                declarations
            },
        };
        // **锚：取钟在读之前。**
        //
        // `looked_at` 是"我们去看的那一刻"：它**不晚于**服务端处理这次请求的时刻 ⇒ 本次漏掉的消息
        // 一定落在下一次的窗口里（**不漏报**）。
        // ⚠ 而锚**最终**推到哪一刻不一定是它：常规路推到**这一段收场**（见收场处的 `next_anchor`），
        // 这一段还有没摆到眼前的动静时才只推到这里。相邻两次投影因此仍把时间**恰好切分**：每条消息
        // 落在且只落在一个窗口里、被且只被一次计数。
        let previous = self.since;
        let looked_at = options.clock.now();
        // **事件卡片的计数必须与"看的那一刻"同刻开始**（2026-10-20）：铃回调只在"有活动在跑"
        // 时才记数（[`RunWatch::sink`]），而从 `looked_at` 到读完世界投影之间是一段网络往返
        // （最长几十秒）。从前 `begin` 排在投影**之后** ⇒ 落在这段窗口里的一声铃两格都不数
        // （卡片不记；下一段的增量窗口又从收场起算）。现在把通道与 `begin` 前移到 `looked_at`
        // 紧后面：**"看的那一刻"与"开始计数的那一刻"是同一刻**。
        //
        // 句柄是 RAII 的（[`WatchGuard`]）：装配被拒 / 通道终结这些早退路径由 `Drop` 收场，
        // 在跑表不会再停在一个死写柄上。
        let (cards_writer, cards) = EventCardsReport::channel();
        let watch_guard = options.watch.begin(looked_at, cards_writer);
        // **两场之间**：把这张表里**已经到点**的收掉（含离线错过）——它发生在开活动**之前**，
        // 因此"Run 里到点不抢占、下一场记账"是这条次序的机械后果（与 im 动静同律）。
        //
        // **返回值不要了**：那一份"待报清单"从前住在本件里（`owed`），而它正是"崩一次就永远不报"
        // 的来源——`fired` 已经落盘，清单却随进程消失。现在开场由**持有物那一块整体摆出整张表**
        // （`composition::read_opening_materials`），于是"报过没有"不再是一个状态，也就没有可丢
        // 的东西：崩在收掉**之前** ⇒ 表里是 `pending` + `overdue`；崩在**之后** ⇒ 表里是 `fired`。
        // **两种都看得见**，没有第三种（静默消失）的形状。
        //
        // **失败不在这里兜底**：收不掉时那一条留在 `pending`，钟源下一次仍会认它（**再醒一次**，
        // 不是静默吃掉）；"这一次没读到"由持有物那一块自己读同一张表、如实写 `unknown`。
        // 钟读不出来（1970 之前）⇒ 什么都不收：条目留在 `pending`，同样是 fail-safe 的那一侧。
        if let Some(now) = unix_millis(options.clock.now()) {
            let _ = TimerApp::new(options.environment_root).consume_due(now);
        }
        // **通知栏那块投影：每开一段恰好一次**（一段在场跨多声铃，那些铃归事件卡片，不再重读）。
        // 读失败**不**影响这次活动：它只是"这次没看见"，
        // 如实写 unknown。
        //
        // ⚠ **2026-10-10 改**：从前是"**没锚就不读**"（不知道"新"从哪儿算起，读了也没有意义）
        // ——而那一句让**刚重启 / 第一次开机的那一段通知栏整块失明**。现在照读：增量那一半确实
        // 问不出来（**不编一个 0**），而**未读那一半按世界记着的读位置算，它不需要锚**。
        //
        // **它不再捎带闹钟**：持有物是**手机的**，世界投影是**世界的**，两者从前被拼成同一个
        // 字符串（`format!("{counts}\n{timer_line}")`），于是 im 一断线、或第一个 Run 没有锚，
        // 闹钟就跟着一起哑了。现在各走各的块：投影这一块只说自己，持有物那一块自己出场。
        // ⚠ 那一行从前还有第二处看不见：它摆不出**「过期却没响成」**那一档
        // ——今天由持有物如实摆出 `pending + overdue`（`holdings.rs` 的 `//!` 第 3 条纪律）。
        let projection = self.project(options, previous);
        let read = projection.is_some();
        // **锚的推进**（⚠ 2026-10-10 改：推进到"**这一段收场**"那一刻，见下面收场处的赋值）。
        //
        // 从前这里推的是 `looked_at`（**去看的那一刻**，即这一段的**开头**）⇒ 窗口 = （上一段开头，
        // 这一段开头]，于是**主体在场时**到的消息落在**下一次**的窗口里。真机实测的形状
        // （2026-10-10，主体自己报上来的）：主体一段里答完 5 条 → 搁下 → 三秒后铃响 →
        // 下一段的通知栏「新动静」仍数着那 5 条 ⇒ 主体为**已经答过的**消息**白醒一段**（"补叫"）。
        //
        // 现在锚推到场**收场**的时刻 ⇒ 增量那一半的窗口 = **主体不在场的那段时间**，语义一句话：
        // **「我睡着的时候有多少」**。而主体**醒着**时到的消息归**事件卡片**那一格
        // （Run 级增量，宿主跑着时听到铃就写）——两格各管一段，不重叠也不留缝。
        // ⚠ **"不漏"由未读那一半兜底**：它按世界记着的读位置算（与锚无关）⇒ 主体没读过的永远算得出来。
        let mut session = RunSession::open_declared(
            options.material_root,
            &mut environment,
            RunOrigin {
                // **起源如实**：是谁叫醒的就说谁——通道接上 / 通道报变化 / 主体约的时间
                // （词表值；那句人话由渲染点用同一个值现查现派生，不在盘上存一句话）。
                //
                // ⚠ **三档一一对应，没有兜底支**（2026-10-13）：`Refused` **走不到这里**
                // （它是唤醒通道的终局，由 [`accept`] 在开段之前就变成 `HostFailure`；
                // 这里照抄同一个映射，只为让 match 穷尽而不替未来的档位猜一个起源）。
                origin: match woken {
                    Woken::Opened => WakeOrigin::Opened,
                    Woken::Changed => WakeOrigin::Changed,
                    Woken::Timer => WakeOrigin::Timer,
                    Woken::Refused(failure) => return Err(HostFailure::Doorbell(failure)),
                },
                since: previous,
                notification: projection.as_deref(),
                cards: Some(cards),
            },
            request,
        )
        .map_err(classify)?;

        // **这一段被驱动几次，由铃决定；它什么时候收场，由主体决定**（2026-10-10 在场那一刀）。
        //
        // ⚠ **从前这里是"催"**（更早）：主体交回控制（纯正文、没调工具）时 Core 说"这一段没有结束"，
        // 而宿主**立刻再驱动一次**（空输入、零新信息），一直到轮次预算用尽 ⇒ 账上写
        // `system:timeout`（"预算用尽"），而真实发生的是**主体自己停了下来**——**拿假话记主体的账**。
        // 那一刀把它改成"停在搁下上"（`session.hold()` ⇒ `subject:held`）：假话没了，
        // 但**每一声音铃仍然开一段新活动**、上下文每段从零起。
        //
        // 现在两件都对了：**不催**（不再拿空输入反复问主体），**也不封**（不再替主体把这一段结束）。
        // `Unsettled` ⇒ 仍在场，等下一次唤醒再驱动**同一段**。
        //
        // ⚠ **落地的是宪法那三句**：§7.5「等待、沉默和不行动，都是合法的 Agent 状态」、
        // §4「不强迫 Agent 每次醒来都必须行动」、§5「不能拒绝工作 = 在上班；不能发起工作 = 在待命」
        // —— 后一句要的是"主体能决定自己还在不在场"，而那正是这一段。
        // ⚠ **显式继续那条路一字未动**：它是**操作者**的动作（`drive_with_input`），
        // 与"主体交回控制"不是同一件事，因此不并进这一档。
        let mut drives = 0usize;
        let outcome = loop {
            drives += 1;
            match session.drive(model).map_err(classify)? {
                SessionDrive::Ended(outcome) => break outcome,
                SessionDrive::Unsettled => {
                    // **在场不动**：等下一次唤醒（门铃或主体自己约的时刻）。
                    //
                    // 通道被终结 ⇒ 如实退出，**不替这一段收场**：断了的是唤醒通道，
                    // 不是主体结束了。那一段因此没有收尾事实，下一次开机的开场纸条会
                    // 如实报"有一段没有正常收场"——给系统终止补一个终态会让那句话变成假话。
                    //
                    // 这一步**不用** `self.next_wake`（那是 `&mut self`），因为 `options`
                    // 此刻还借着 `self`：同一个判据由 [`accept`] 一处拥有，两处都走它。
                    accept(wake.next())?;
                    // **唤醒边界**：铃已到、下一轮还没开始。调用方在这里做它自己那一件事
                    // （宿主二进制：重判模型来源）。
                    // ⚠ 它**不决定这一段收不收场**：那是主体的判断。
                    // ⚠ 它也**换不了装配**：这一段由开段那一刻现读的声明驱动（那条借用关系见
                    // [`Host::declare_model_assembly`]），变了只能在这里**读出来给操作者**，
                    // 主体看不到。
                    boundary(model)?;
                }
            }
        };
        // 这一段收场：细胞散掉（下一段是新的一格）。**先问一句"还有没有没摆到眼前的动静"**。
        let unshown = watch_guard.end();
        // **锚在这里推进**（2026-10-10 改）：推到场**收场**的时刻，不是场开头那一刻——
        // 见上面那段。三条纪律：
        //
        // - **读成了** ⇒ 锚推到场收场时刻（下一次的窗口 = 这一段收场之后到现在）；
        // - **没读成** ⇒ 锚**不动**（窗口继续延伸，下次一并报出来——"不漏报"那一侧）；
        // - **第一个 Run 播种**：还没有可比对的边界时也要种下锚，否则永远没有起点。
        //
        // ⚠ 在场跨多声铃时这条纪律**一字不变**：增量那一半量的仍然是"**主体不在场的那段时间**"
        // （在场期间的新动静归**事件卡片**那一格，两格不重叠也不留缝）。
        let settled_at = options.clock.now();
        // **锚推进到哪里，取决于"这段期间的动静有没有被摆到主体眼前"**（2026-10-13 改）。
        //
        // 从前只看 `read`（"这一段读到过投影吗"）⇒ 读成过就推到场收场时刻。那等于假设
        // **读过了就是看完了**，而它在"活动跑到一半就死了"时不成立：一段死在第 1 轮
        // （模型失败 / 崩溃 / 断电），它开场之后到达的动静**两个窗口都不落**——上一段的卡片
        // 随它散掉，下一段的窗口又从它的收场时刻起算 ⇒ 那条消息谁都不会再提（真机实测：
        // 14:09:45 到的消息，到 14:16 都没人再看它一眼）。
        //
        // 现在：这段期间的动静**都渲染进过某一次注入**（`unshown == false`）⇒ 锚推到场收场时刻
        // （常规路，一字不变）；**还有没渲染过的** ⇒ 锚只推到**这一段开头那一刻**（那一次它确实
        // 看过），把这段期间的动静留给下一段（"不漏报"那一侧）。补响由宿主循环负责。
        self.since = next_anchor(read, previous, unshown, looked_at, settled_at);

        Ok(RunReading {
            run,
            outcome,
            drives,
            unshown,
            anchor_at: settled_at,
            notification: read,
            // 取走（不是"看一眼"）：一段里驱动好几轮，每轮都可能重试——
            // 取走让宿主**每条恰好打一次**。
            model_retries: session.take_model_retries(),
        })
    }

    /// 读一次**只读投影**（自 `since` 以来每间房几条新的 ＋ 每间房有几条未读）。
    /// 返回 `None` = 没读成（读失败）。
    ///
    /// 它走的是 App 的**只读投影入口**（`im_app::new_since`）——wire 形状仍然只在 App 那一处，
    /// 本层只拿结果字符串。**一次调用恰好一次请求**（App 那边的纪律）。
    ///
    /// **为什么读在这里**：网络往返因此**不进 Core 的注入路径**，也不多一个网络源文件——
    /// Core 只看见"外层又给了一块"（与事件卡片同一条分工，见 `event_cards.rs` 的 `//!`）。
    /// 读失败只让**这一次没看见**（`counts = unknown`），**不挡这一次活动**：便利坏了不挡清醒。
    ///
    /// `since = None` = **没有锚**：照样读——增量那一半问不出来，未读那一半照给
    /// （见 [`ImApp::new_since`]）。
    fn project(&self, options: &HostOptions<'_>, since: Option<SystemTime>) -> Option<String> {
        // 读只读投影要用 im 的访问声明：`None` = 这一次装配没有 im ⇒ 没有可读的投影（如实返回 `None`）。
        let app = ImApp::new(options.declarations.im.clone()?)?;
        app.new_since(since).ok()
    }
}

/// 装配被拒 → 宿主退出原因（**准入**那一档要单独认出来：它是操作者的动作空间）。
///
/// ⚠ **2026-09-26 起只剩 `Paused` 一档**：`Unfinished` 不再是一种拒绝——进程没了就是那一段断了，
/// 新进程起来是**新的一段**（见 `composition.rs` 装配点那一句）。而"上一段没有收尾"由**开场那一轮**
/// 如实带给主体，不用拒绝来挡。
/// ⚠ **2026-10-06 起那一档也没了**（主人裁决）：`Paused` 同样是**事实**不是闸——它由开场那一块
/// 纸条带给主体（`PreviousActivity` 的 `StorageFailure` 那一段）。因此**装配失败一律归
/// [`HostFailure::Environment`]**（"环境不可用，去看清原因"），不再有"需要你动手"这一档。
fn classify(error: CompositionError) -> HostFailure {
    HostFailure::Environment(detail(&error))
}

/// 装配错误 → 一个**不含配置细节**的稳定读数（照既有各层各自渲染的先例：这里只说清"哪一类"，
/// 存储格式、资料根取值、机器内部细节都不外泄）。
///
/// ⚠ **活动面那一格不许再塌成一个词**（2026-10-06 收）：从前 `CompositionError::Run(_)` 一律写成
/// `run-refused`，于是**事实写失败 / 保存未确认 / 活动槽被占 / 轮次用尽**在操作者读数里长得一模一样
/// （退出码也一律相同）——那正是"必须保留影响调用方决策的类别、阶段和已确认边界"要挡的形状。
/// 分类归 Core（[`RunError::as_str`] 是那张词表的唯一权威），这里只**委托**并保留既有前缀
/// （`run-refused:` 这个记号照旧可搜，后面多出是哪一类）。
fn detail(error: &CompositionError) -> String {
    match error {
        CompositionError::FileAppRefused(class) => format!("materials-refused:{}", class.as_str()),
        CompositionError::MachineRefused(class) => format!("machine-refused:{class}"),
        CompositionError::Environment(reason) => format!("environment:{reason}"),
        CompositionError::Run(error) => format!("run-refused:{}", error.as_str()),
        CompositionError::SaveFailed { stage, .. } => format!("save-failed:{}", stage.as_str()),
        CompositionError::SessionNotDrivable => "session-not-drivable".to_string(),
    }
}

/// 让 `Admission` 的渲染只此一处：读数行里那句"准入是什么"。
pub fn admission_text(admission: &Admission) -> String {
    match admission {
        Admission::Ready => "ready".to_string(),
        Admission::Paused { reason } => format!("paused(reason={})", escape_field(reason)),
        Admission::Unfinished { run } => format!("unfinished(run={})", escape_field(run.as_str())),
    }
}
