//! timer（TASK_75）：主体的**自持物**第三件——「过一会儿再来看」这件事的落点。
//!
//! 三件套的前两件是**记**（`notes`：写下来的字）与**钉**（`pinned_actions` / `pins_board`：留在
//! 视野里的格位与纸条）；缺的这一件是**时间**：主体今天没法给自己约一个时刻。
//!
//! 这一件解决的是**意图跨场存活**，它要两样东西同时在场：
//!
//! - **意图的载体**：`(id, when, label, state)` 四字段的一条记录——**标签是写给未来自己的信**。
//!   没有它，到点醒来的就是个**被叫醒的失忆者**：主体知道"有事"，却不知道"什么事"。
//! - **到点的唤醒**：宿主在**两场之间**读这张表，最早的一条 `pending` 到点就响一声（本地钟源，
//!   **不经 im**）——门铃的语义一个字没动，它照旧只管"何时开始一次活动"。
//!
//! ## 它是 App，不是 OS 的动词
//!
//! 与 `pin_action` / `write_self_concept`（OS 自己的动词、常驻在场）不同，这一件是**普通 App**：
//! 只有它被**展开**之后，三条动词才进场；没展开时动词根本不在场（那道门归 OS，本件不认识它、
//! 也不重复它那套分类词的名字）。
//! 拔掉它，全场照跑（没有 timer 的世界里，主体只是没法约时间）。
//!
//! ## 四字段（磁盘契约）
//!
//! `<环境根>/timers`：**一行一条 JSON 对象**（与 notes 同款：一条一个对象、`serde` 键就是契约）。
//!
//! ```text
//! {"id":"t-1","when":1758000000000,"label":"看便签","state":"pending"}
//! ```
//!
//! - `id`：**机械身份**（`t-<序号>`，序号取表里最大值 + 1）——`timer_cancel` 认它，**不靠时刻猜**；
//! - `when`：**unix 毫秒**，与状态行那句 `since` **同一个格式化点**（[`crate::clock::anchor_value`]）
//!   ⇒ 主体可以把那一行原文抄过来当参数，不必换算；
//! - `label`：**可选**的一行（上限 [`MAX_LABEL_BYTES`] 字节，超限**如实截断**并在回执里留读数；
//!   空就是空——**不设必填**：强制必填只会收到垃圾标签）；
//! - `state`：`pending` → `fired`。**`timer_cancel` 是除名**（从表里去掉）——Agent 的自持物主体做主。
//!
//! ## 三条不许越的线
//!
//! - **不预建**：不做 repeat / snooze / modify（取消再设就是它的组合）/ 必填标签 / top-N /
//!   长备注（那是 notes 的活。标签写「看便签」是自己的桥，机制不替主体搭）；
//! - **不替主体判断**：`when` 落在过去也照收（到点就响、如实显示**原定时刻**）——迟到是事实，
//!   **不漏报**是铁律；
//! - **不声明写效应**（[`TimerApp`] 不实现 `declared_effects`）：自持物写**不需要授权**
//!   （既有先例：`pin_action` / `write_self_concept` 的文档与 `notes` 的准入注释，`os.rs` 里那句
//!   「写自己的地盘」）。它与 notes 的 creation 授权族**无关**——那一族管的是**授权资料根**里的
//!   写（`files_create`），Agent 的自持物不经过它。

use std::io;
use std::path::{Path, PathBuf};

use crate::app_contract::AppPort;
use crate::clock::anchor_value;
use crate::core::{
    Action, ActionAdmission, ActionId, ActionOutcome, ActionParams, ActionPort, Capabilities,
    Capability, ContentBlock, ParamSpec,
};
use crate::foundation::{AgentId, OperationId, RunId};
use crate::holdings::{HoldingRow, HoldingState};

/// 这个 App 的稳定标识（装配时用它；展开那一格也认它）。
pub const TIMER_APP_ID: &str = "timer";

/// 自持物的文件名：`<环境根>/timers`（一行一条 JSON 对象）。
pub const TIMER_FILE: &str = "timers";

/// 三条动词（app 命名空间：`<app>_<动词>`）。
pub const CAPABILITY_SET: &str = "timer_set";
pub const CAPABILITY_LIST: &str = "timer_list";
pub const CAPABILITY_CANCEL: &str = "timer_cancel";
/// **两个写动词的复核**（2026-10-13）：与各自的写**同参数**、只读，回答"现在的真实状态符不符合
/// 当初那份参数"。它们的存在理由与归属见 [`AppPort::declared_effects`] 那一处。
pub const CAPABILITY_VERIFY_PENDING: &str = "timer_verify_pending";
pub const CAPABILITY_VERIFY_CANCELLED: &str = "timer_verify_cancelled";

/// 参数名（对外就是这几个字，卡上原样写出来）。
pub const PARAM_WHEN: &str = "when";
pub const PARAM_LABEL: &str = "label";
pub const PARAM_ID: &str = "id";

/// 标签的**字节上限**（~60B 级：一行、够写「看便签」这类提示，装不下长备注）。
///
/// 超限**如实截断**（不拒绝、不悄悄丢）：截断发生在**字符边界**上，并在回执里留一条读数
/// （[`LABEL_TRUNCATED`] / [`LABEL_BYTES`] 两个字段），主体一眼看得出"它被剪过、原来多长"。
pub const MAX_LABEL_BYTES: usize = 60;

/// 截断读数里的两个字段名（回执用）。
pub const LABEL_TRUNCATED: &str = "label_truncated";
pub const LABEL_BYTES: &str = "label_bytes";

/// 状态的两个值（磁盘上就是这两个词）。
pub const STATE_PENDING: &str = "pending";
pub const STATE_FIRED: &str = "fired";

/// **这一件对外的失败词表**（封闭表：编码与解码只有这一份权威）。
///
/// ⚠ **2026-10-13 补上解码端**：这一族从前**只有裸常量、没有 `from_str`**——按纪律「稳定字符串
/// 是一份词汇表，必须有权威位置；新增取值必须**同一刀**补齐解码端校验（封闭表、未知取值明确
/// 拒绝），不许只写编码端」，加两个取值之前得先有这张表。⇒ 它同时进了
/// `tests/action_failure_wordlist.rs` 的封闭名单（**第九份**）。
///
/// ⚠ **标记的字面量写在这里**（不挂在常量上）：那条守卫按**调用形状**读词表——它扫 `as_str` 的
/// **字面量**，隔着常量一层就报"判据在空跑"（本刀当场被它拦下一次）。⇒ 权威收进枚举，原来那批
/// `CLASS_*` 常量**同刀删掉**（同一件事只留一个名字）。
///
/// `from_str` 认不出就返回 `None`（**fail-closed**，不静默降级成一个"别的失败"）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerFailure {
    /// 表读不出来。
    Unreadable,
    /// 表写不下去。
    Unwritable,
    /// `when` 不是一个非负整数毫秒。
    InvalidWhen,
    /// **参数面**：缺必填。
    MissingParameter,
    /// **参数面**：多了不认识的。
    UnknownParameter,
    /// **复核**：没有一条与那份参数相符、还在等的（`timer_verify_pending`）。
    NotPending,
    /// **复核**：那一条还在表里（`timer_verify_cancelled`）。
    StillPending,
}

impl TimerFailure {
    /// 回执与盘上的那个词（**契约**）。
    ///
    /// ⚠ **不写成 `const fn`**：那条词表守卫按 `pub fn as_str(` 这个签名抽方法体，加了 `const`
    /// 就抽不到——它会如实报"判据在空跑"（本刀当场被它拦了第二次）。它与另外那八份词表同形。
    pub fn as_str(self) -> &'static str {
        match self {
            TimerFailure::Unreadable => "timers-unreadable",
            TimerFailure::Unwritable => "timers-unwritable",
            TimerFailure::InvalidWhen => "invalid-when",
            TimerFailure::MissingParameter => "missing-parameter",
            TimerFailure::UnknownParameter => "unknown-parameter",
            TimerFailure::NotPending => "timers-not-pending",
            TimerFailure::StillPending => "timers-still-pending",
        }
    }

    /// 从那个词还原；**认不出返回 `None`**（封闭表，与 `NotesFailure::from_str` 同款）。
    ///
    /// 它**不**实现 `std::str::FromStr`：那个 trait 要求一个 `Err` 类型，会把"认不出"变成一个
    /// 错误**值**；这里只有"认得"与"认不得"两种，多出来的那个类型没有消费者。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Option<Self> {
        match text {
            "timers-unreadable" => Some(TimerFailure::Unreadable),
            "timers-unwritable" => Some(TimerFailure::Unwritable),
            "invalid-when" => Some(TimerFailure::InvalidWhen),
            "missing-parameter" => Some(TimerFailure::MissingParameter),
            "unknown-parameter" => Some(TimerFailure::UnknownParameter),
            "timers-not-pending" => Some(TimerFailure::NotPending),
            "timers-still-pending" => Some(TimerFailure::StillPending),
            _ => None,
        }
    }
}

/// 一条记录的状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerState {
    /// 还没到点。
    Pending,
    /// 到过点了（**`when` 保留原值**：迟到是事实，[`Entry::when_millis`] 说的永远是"主体约的那个时刻"）。
    Fired,
}

impl TimerState {
    /// 磁盘上的那个词（**契约**）。
    pub fn as_str(&self) -> &'static str {
        match self {
            TimerState::Pending => STATE_PENDING,
            TimerState::Fired => STATE_FIRED,
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            STATE_PENDING => Some(TimerState::Pending),
            STATE_FIRED => Some(TimerState::Fired),
            _ => None,
        }
    }
}

/// 表里的一条（对宿主与自己都是同一份形状）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// 机械身份（`t-<序号>`）。
    pub id: String,
    /// 主体约的那个时刻（unix 毫秒）。
    pub when_millis: u64,
    /// 写给未来自己的那一行（可能为空）。
    pub label: String,
    /// 现在的状态。
    pub state: TimerState,
}

/// 磁盘上的一行：**一行一个 JSON 对象**，那四个键就是契约。
///
/// 编解码**手写**（与 notes 同款，只走 `serde_json`）：这一层只认那四个键——
///
/// - `id` / `when` / `state` **必须有且类型对**，否则这一行读不懂（上游如实说 `unknown`）；
/// - `label` 可以缺席（**唯一带默认值的字段**）：缺席就是空标签——将来加字段时照这条先例给默认值，
///   因此老的表照旧读得动；
/// - **多出来的键不看**（那是将来加的字段，不是错误）。
fn encode(entry: &Entry) -> String {
    serde_json::json!({
        "id": entry.id,
        "when": entry.when_millis,
        "label": entry.label,
        "state": entry.state.as_str(),
    })
    .to_string()
}

/// 一行读回一条。**形状不符 ⇒ `None`**（不猜、不当默认值）。
fn decode(line: &str) -> Option<Entry> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let object = value.as_object()?;
    Some(Entry {
        id: object.get("id")?.as_str()?.to_string(),
        when_millis: object.get("when")?.as_u64()?,
        label: match object.get("label") {
            Some(label) => label.as_str()?.to_string(),
            None => String::new(),
        },
        state: TimerState::parse(object.get("state")?.as_str()?)?,
    })
}

/// 主体的定时表。
///
/// 它**只**持有环境根（自持物住在那里），不持有历史、行动端口、模型端口或账本——与纸条板、格位
/// 同款：只把**读回来的那一份**变成动作与读数。
pub struct TimerApp {
    root: PathBuf,
}

impl TimerApp {
    /// 指向一个环境根。**不做任何 IO**（表可能还不存在；"不存在"是"主体还没约过"，不是故障）。
    pub fn new(environment_root: &Path) -> Self {
        Self {
            root: environment_root.to_path_buf(),
        }
    }

    /// 表的路径（`<环境根>/timers`）。
    pub fn path(&self) -> PathBuf {
        self.root.join(TIMER_FILE)
    }

    /// 读整张表。
    ///
    /// **文件不存在 = 主体还没约过** ⇒ 空表；
    /// **读不动 / 有一行读不懂** ⇒ `Err`（上游如实说 `unknown`，绝不冒充"一条都没有"）。
    pub fn entries(&self) -> io::Result<Vec<Entry>> {
        let text = match std::fs::read_to_string(self.path()) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut entries = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match decode(line) {
                Some(entry) => entries.push(entry),
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "timer row is not readable",
                    ));
                }
            }
        }
        entries.sort_by(|left, right| {
            left.when_millis
                .cmp(&right.when_millis)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(entries)
    }

    /// **交素材**给显示区（装配期用，不是主体的动词）。
    ///
    /// 方向是 `app → 显示区`：手机模型 §3 的分层自下而上是 **1 循环＋插槽 / 2 显示区 / 3 app /
    /// 4 Agent**，而第 3 条写着"**app：往显示区交素材**"——上层依赖下层，合法。
    ///
    /// 因此本 App **依赖 [`crate::holdings`]**、把它自己的记录填进 [`HoldingRow::Timer`]：
    ///
    /// - 交的是**结构化内容**（约了什么、什么状态），**不是本 App 的记录类型**；
    /// - **怎么摆成一行归显示区**（手机模型 §5：「内容归 app 交，怎么显示手机说了算」）；
    /// - 盘上那个状态词（[`TimerState::as_str`]）在这里**映射**成显示区的词
    ///   （[`HoldingState`]）——两份词值相同、来源不同，各自有主，谁也不解析谁的。
    ///
    /// ⚠️ 反过来的形状（显示区 import 本 App、装配层直接调 [`TimerApp::entries`] 喂给它）是
    /// **下层认识上层**：那等于"锁屏去翻时钟 App 的数据库"。真实手机里 `Notification` 那个类
    /// 是**平台**定义的、app 填内容——本函数就是"填"这一步。
    pub fn holdings_material(&self) -> io::Result<Vec<HoldingRow>> {
        Ok(self
            .entries()?
            .into_iter()
            .map(|entry| HoldingRow::Timer {
                id: entry.id,
                state: match entry.state {
                    TimerState::Pending => HoldingState::Pending,
                    TimerState::Fired => HoldingState::Fired,
                },
                when_millis: entry.when_millis,
                label: entry.label,
            })
            .collect())
    }

    /// 最早的 `pending`（宿主钟源据此排下一次睡眠）。没有 ⇒ `None`。
    pub fn next_pending(&self) -> io::Result<Option<Entry>> {
        Ok(self
            .entries()?
            .into_iter()
            .find(|entry| entry.state == TimerState::Pending))
    }

    /// **把已经到点的收掉**：`state == pending && when <= now` 全部转 `fired` 并落盘，返回它们
    /// （按 `when` 升序）。
    ///
    /// 这一下**不改 `when`**：主体约的是那个时刻，晚响是迟到、不是重约（"如实显示原定时刻"）。
    /// 没有到点的 ⇒ 空表、**不写盘**（读一次不动一个字节）。
    pub fn consume_due(&self, now_millis: u64) -> io::Result<Vec<Entry>> {
        let mut entries = self.entries()?;
        let mut consumed = Vec::new();
        for entry in entries.iter_mut() {
            if entry.state == TimerState::Pending && entry.when_millis <= now_millis {
                entry.state = TimerState::Fired;
                consumed.push(entry.clone());
            }
        }
        if !consumed.is_empty() {
            self.write(&entries)?;
        }
        Ok(consumed)
    }

    /// 整表落盘（一行一条）。**就地重写**（与 `pinned_actions` / `self_concept` 同款：自持物是主体的，
    /// 不引入"临时文件 + 改名"的第二套写路径；写坏了的后果是"下一次读不出来 ⇒ 如实说 unknown"，
    /// 而不是静默丢一条）。
    fn write(&self, entries: &[Entry]) -> io::Result<()> {
        let mut text = String::new();
        for entry in entries {
            text.push_str(&encode(entry));
            text.push('\n');
        }
        std::fs::write(self.path(), text)
    }

    /// 下一个机械身份：表里 `t-<n>` 的最大 `n` + 1（表里没有 ⇒ `t-1`）。
    ///
    /// **取消过的号不复用**：已 `fired` 的条目照旧在表里，因此序号只增——主体手里那个 id 不会
    /// 在不知不觉间指向另一条。
    fn next_id(entries: &[Entry]) -> String {
        let max = entries
            .iter()
            .filter_map(|entry| entry.id.strip_prefix("t-"))
            .filter_map(|digits| digits.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        format!("t-{}", max + 1)
    }

    /// `timer_set`：约一个时刻。
    fn set(&self, action: &Action) -> ActionOutcome {
        if let Err(shape) = require_params(action.params(), &[PARAM_WHEN], &[PARAM_LABEL]) {
            return shape;
        }
        let raw = action.params().get(PARAM_WHEN).unwrap_or_default();
        let when_millis = match raw.trim().parse::<u64>() {
            Ok(value) => value,
            // 值不是非负整数 ⇒ 明确拒绝（**不**退回"现在"：那会把主体的话改成别的事）。
            Err(_) => return ActionOutcome::failure(TimerFailure::InvalidWhen.as_str()),
        };
        let (label, truncation) = clip_label(action.params().get(PARAM_LABEL).unwrap_or_default());
        let mut entries = match self.entries() {
            Ok(entries) => entries,
            Err(_) => return ActionOutcome::failure(TimerFailure::Unreadable.as_str()),
        };
        let entry = Entry {
            id: Self::next_id(&entries),
            when_millis,
            label,
            state: TimerState::Pending,
        };
        entries.push(entry.clone());
        if self.write(&entries).is_err() {
            return ActionOutcome::failure(TimerFailure::Unwritable.as_str());
        }
        let mut text = entry_lines(&entry);
        if let Some(bytes) = truncation {
            text.push('\n');
            text.push_str(&format!(
                "{LABEL_TRUNCATED} = true\n{LABEL_BYTES} = {bytes}\n{LABEL_LIMIT} = {MAX_LABEL_BYTES}"
            ));
        }
        ActionOutcome::success(vec![ContentBlock::new(TIMER_APP_ID, text)])
    }

    /// `timer_list`：机械读数（按 `when` 升序，同刻按 id）。
    fn list(&self) -> ActionOutcome {
        let entries = match self.entries() {
            Ok(entries) => entries,
            Err(_) => return ActionOutcome::failure(TimerFailure::Unreadable.as_str()),
        };
        let pending = entries
            .iter()
            .filter(|entry| entry.state == TimerState::Pending)
            .count();
        let mut text = format!(
            "count = {}, {STATE_PENDING} = {}, {STATE_FIRED} = {}",
            entries.len(),
            pending,
            entries.len() - pending
        );
        if entries.is_empty() {
            // 空也要说出口（"没有"与"没读到"是两句话）。
            text.push_str("\n（还没有约过时间）");
        }
        for entry in &entries {
            text.push('\n');
            text.push_str(&entry_lines(entry).replace('\n', "\t"));
        }
        ActionOutcome::success(vec![ContentBlock::new(TIMER_APP_ID, text)])
    }

    /// `timer_cancel`：**从表里除名**（Agent 的自持物主体做主）。
    ///
    /// 取消一个表里没有的 id 是**明确的成功结果**（与 `unpin_action` 同款：摘一个本来就没钉的
    /// 名字不报错）——把它当失败会逼主体先查一遍表，而"它本来就不在"已经是答案。
    fn cancel(&self, action: &Action) -> ActionOutcome {
        if let Err(shape) = require_params(action.params(), &[PARAM_ID], &[]) {
            return shape;
        }
        let id = action.params().get(PARAM_ID).unwrap_or_default().trim();
        let mut entries = match self.entries() {
            Ok(entries) => entries,
            Err(_) => return ActionOutcome::failure(TimerFailure::Unreadable.as_str()),
        };
        let before = entries.len();
        entries.retain(|entry| entry.id != id);
        if entries.len() == before {
            return ActionOutcome::success(vec![ContentBlock::new(
                TIMER_APP_ID,
                format!("{PARAM_ID} = {id}\ncancelled = false\n（表里本来就没有这个 id）"),
            )]);
        }
        if self.write(&entries).is_err() {
            return ActionOutcome::failure(TimerFailure::Unwritable.as_str());
        }
        ActionOutcome::success(vec![ContentBlock::new(
            TIMER_APP_ID,
            format!(
                "{PARAM_ID} = {id}\ncancelled = true\ncount = {}",
                entries.len()
            ),
        )])
    }

    /// `timer_verify_pending`：同参数问"现在表里有没有一条与它相符、**还在等的**"（只读）。
    ///
    /// ⚠ **与 `set` 用同一把截断尺**（[`clip_label`]）：`set` 落盘的是**截断之后**的标签，复核
    /// 当然也要截断之后再比——否则"约的时候被剪过"的那一条永远复核不上（两处同一个函数 ⇒ 不会漂）。
    ///
    /// ⚠ **三分**（与 notes 的复核同款）：有 ⇒ 成功；**确定没有** ⇒ 明确失败
    /// （[`TimerFailure::NotPending.as_str()`]）；**读不出来 ⇒ `unknown`**——"没读到"绝不说成"确定没有"。
    fn verify_pending(&self, action: &Action) -> ActionOutcome {
        if let Err(shape) = require_params(action.params(), &[PARAM_WHEN], &[PARAM_LABEL]) {
            return shape;
        }
        let raw = action.params().get(PARAM_WHEN).unwrap_or_default();
        let Ok(when_millis) = raw.trim().parse::<u64>() else {
            return ActionOutcome::failure(TimerFailure::InvalidWhen.as_str());
        };
        let (label, _) = clip_label(action.params().get(PARAM_LABEL).unwrap_or_default());
        let entries = match self.entries() {
            Ok(entries) => entries,
            Err(_) => return ActionOutcome::unknown(),
        };
        let count = entries
            .iter()
            .filter(|entry| {
                entry.when_millis == when_millis
                    && entry.label == label
                    && entry.state == TimerState::Pending
            })
            .count();
        if count == 0 {
            return ActionOutcome::failure(TimerFailure::NotPending.as_str());
        }
        ActionOutcome::success(vec![ContentBlock::new(
            TIMER_APP_ID,
            format!("{PARAM_WHEN} = {when_millis}\npending = true\ncount = {count}"),
        )])
    }

    /// `timer_verify_cancelled`：同参数问"那条 id 现在**还在不在**表里"（只读）。
    ///
    /// ⚠ **三分**：不在 ⇒ 成功；**还在** ⇒ 明确失败（[`TimerFailure::StillPending.as_str()`]，回执里带上它现在
    /// 那一行——"还在"是个可复核的事实，不是"好像没成"）；读不出来 ⇒ `unknown`。
    fn verify_cancelled(&self, action: &Action) -> ActionOutcome {
        if let Err(shape) = require_params(action.params(), &[PARAM_ID], &[]) {
            return shape;
        }
        let id = action.params().get(PARAM_ID).unwrap_or_default().trim();
        let entries = match self.entries() {
            Ok(entries) => entries,
            Err(_) => return ActionOutcome::unknown(),
        };
        if let Some(entry) = entries.iter().find(|entry| entry.id == id) {
            return ActionOutcome::failure_with_content(
                TimerFailure::StillPending.as_str(),
                vec![ContentBlock::new(TIMER_APP_ID, entry_lines(entry))],
            );
        }
        ActionOutcome::success(vec![ContentBlock::new(
            TIMER_APP_ID,
            format!("{PARAM_ID} = {id}\ncancelled = true"),
        )])
    }
}

/// 截断读数里的第三个字段名（上限本身）。
pub const LABEL_LIMIT: &str = "label_bytes_limit";

/// 参数面**封闭**：缺必填、多了不认识的，都说清是哪一个（**不猜近似名**）。
fn require_params(
    params: &ActionParams,
    required: &[&str],
    optional: &[&str],
) -> Result<(), ActionOutcome> {
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|name| params.get(name).is_none())
        .collect();
    if !missing.is_empty() {
        return Err(ActionOutcome::failure_with_content(
            TimerFailure::MissingParameter.as_str(),
            vec![ContentBlock::new(
                "parameters",
                format!("missing = {}", missing.join(", ")),
            )],
        ));
    }
    let unexpected: Vec<&str> = params
        .entries()
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| !required.contains(name) && !optional.contains(name))
        .collect();
    if !unexpected.is_empty() {
        return Err(ActionOutcome::failure_with_content(
            TimerFailure::UnknownParameter.as_str(),
            vec![ContentBlock::new(
                "parameters",
                format!(
                    "unexpected = {}\nallowed = {}",
                    unexpected.join(", "),
                    required
                        .iter()
                        .chain(optional.iter())
                        .copied()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )],
        ));
    }
    Ok(())
}

/// 标签裁剪：**在字符边界上**截到 [`MAX_LABEL_BYTES`]；被剪过就回报原字节数。
fn clip_label(raw: &str) -> (String, Option<usize>) {
    let bytes = raw.len();
    if bytes <= MAX_LABEL_BYTES {
        return (raw.to_string(), None);
    }
    let mut end = MAX_LABEL_BYTES;
    while end > 0 && !raw.is_char_boundary(end) {
        end -= 1;
    }
    (raw[..end].to_string(), Some(bytes))
}

/// 一条记录的机械渲染（`key = value` 四行；`when` 走**与锚同一个格式化点**）。
fn entry_lines(entry: &Entry) -> String {
    let when = std::time::UNIX_EPOCH + std::time::Duration::from_millis(entry.when_millis);
    format!(
        "{PARAM_ID} = {}\n{STATE_KEY} = {}\n{PARAM_WHEN} = {}\n{PARAM_LABEL} = {}",
        entry.id,
        entry.state.as_str(),
        anchor_value(when),
        entry.label
    )
}

/// 状态那个字段在读数里的键名（与参数名同一张表：**主体看到的词只有一处定义**）。
pub const STATE_KEY: &str = "state";

impl TimerApp {
    /// 这三条动词的声明面。**它是静态的**（不依赖身份、也不依赖活动）——因此
    /// [`AppPort::declared_capabilities`] 与 [`ActionPort::capabilities`] 直接用同一份，
    /// **不需要为了走一条带 Run 作用域的接缝去伪造一个空的活动标识**。
    ///
    /// ⚠️ 2026-10-06：这里从前写的是 `ActionPort::capabilities(self, &AgentId::new(""),
    /// &RunId::new(""))`——**全 crate 唯一一处 `RunId::new("")`**：装配期（根本没有 Run）
    /// 为了走那条接缝塞了个空值，而实现本来就忽略参数。同族的 App 走的是不依赖 Run 的入口
    /// （`notes_app` 的 `available_capabilities`、`web_app` / `terminal_app` / `im_*` 的
    /// `own_capabilities`）——这一处向它们看齐。
    fn own_capabilities() -> Capabilities {
        Capabilities::of(vec![
            Capability::new(
                ActionId::new(CAPABILITY_SET),
                "给自己约一个时刻：到点前没事发生，到点后你会被叫醒一次（标签是写给未来自己的信）",
                vec![
                    ParamSpec::new(
                        PARAM_WHEN,
                        true,
                        "约在什么时候：unix 毫秒（状态行 `since = …` 那个数就是这个形状，可原样抄）",
                    ),
                    ParamSpec::new(
                        PARAM_LABEL,
                        false,
                        "写给未来自己的一行（可空；上限约 60 字节，超了如实截断）",
                    ),
                ],
            )
            .with_details(
                "到点这件事发生在两场之间：宿主读这张表，最早的一条 `pending` 到点就响一次\
                 ——被叫醒的那一场，起源行会说 timer 是你约的时间到了，而这条记录的 `when` \
                 **保留原样**（晚上响也算数：迟到是事实，不会漏报）。\
                 你约在过去的时刻也照收：它下一拍就会被收掉。\
                 想改时间：先 cancel 再 set（这一件不做 modify、不做 repeat、不做 snooze）。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_LIST),
                "看自己约过哪些时刻（机械读数：几条待响、几条响过，逐条给 id / 状态 / 时刻 / 标签）",
                Vec::new(),
            )
            .with_details(
                "顺序确定（时刻升序、同刻按 id），因此同一份表两次读出来逐字节相同。\
                 标签为空的那条，`label = ` 后面就是空的——空是如实，不是缺。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_CANCEL),
                "把一条约的时间除名（认 id，不猜时刻）",
                vec![ParamSpec::new(
                    PARAM_ID,
                    true,
                    "要除名的那一条的 id（`timer_list` 里逐条给出来）",
                )],
            )
            .with_details(
                "取消一个表里没有的 id 也是成功（回执写 `cancelled = false`）：\
                 `id` 认的是那一条记录，不是时刻——同一刻可以有好几条。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_VERIFY_PENDING),
                "复核一次约时间现在的状态：表里至少有一条同参数还在等的 = 成功（只读）",
                vec![
                    ParamSpec::new(PARAM_WHEN, true, "当初约的那个时刻（unix 毫秒）"),
                    ParamSpec::new(
                        PARAM_LABEL,
                        false,
                        "当初写的那行标签（可空；与 timer_set 完全同参数）",
                    ),
                ],
            )
            .with_details(
                "三分：现在至少有一条同参数、还在等的 = 成功；确定一条都没有 = 明确失败\
                 （timers-not-pending）；读不出来 = 无法确认。弱在归因：记录身份（t-序号）是执行时\
                 取的、不在参数里，因此它无法断言「就是那一次约下的那一条」——同一时刻同一标签的\
                 另一条会让它同样报成功。参数与 timer_set 完全相同；它回答的是现在，不是当时。",
            ),
            Capability::new(
                ActionId::new(CAPABILITY_VERIFY_CANCELLED),
                "复核一次除名现在的状态：那条 id 已经不在表里了 = 成功（只读）",
                vec![ParamSpec::new(PARAM_ID, true, "当初除名的那一条的 id")],
            )
            .with_details(
                "三分：现在不在表里 = 成功；还在 = 明确失败（timers-still-pending）；读不出来 = \
                 无法确认。弱在归因：它只回答「这条现在在不在」，不声称「那一次除名生效了」\
                 ——同参数再约一次会拿到新的身份，因此两者不会混。",
            ),
        ])
    }
}

impl ActionPort for TimerApp {
    /// 声明面只有一份：见 [`TimerApp::own_capabilities`]。
    fn capabilities(&self, _agent: &AgentId, _run: &RunId) -> Capabilities {
        Self::own_capabilities()
    }

    /// 五条动词一视同仁：约时间、看表、除名、两条复核全是**自己的地盘**（与 notes 的准入注释
    /// 同款——自持物写不需要授权；**复核是只读的，同样不需要授权**。原来那句"不走未决门"里的
    /// **未决效果门已退役**，理由本身与门无关：改的是主体自己的格位）；身份与活动由 OS 的使用
    /// 作用域把住，这里不重复判。
    fn admit(&self, _agent: &AgentId, _run: &RunId, action: &Action) -> ActionAdmission {
        match action.id().as_str() {
            CAPABILITY_SET
            | CAPABILITY_LIST
            | CAPABILITY_CANCEL
            | CAPABILITY_VERIFY_PENDING
            | CAPABILITY_VERIFY_CANCELLED => ActionAdmission::Admitted,
            _ => ActionAdmission::refused(crate::core::UNKNOWN_CAPABILITY),
        }
    }

    fn execute(
        &mut self,
        _agent: &AgentId,
        _operation: &OperationId,
        action: &Action,
    ) -> ActionOutcome {
        match action.id().as_str() {
            CAPABILITY_SET => self.set(action),
            CAPABILITY_LIST => self.list(),
            CAPABILITY_CANCEL => self.cancel(action),
            CAPABILITY_VERIFY_PENDING => self.verify_pending(action),
            CAPABILITY_VERIFY_CANCELLED => self.verify_cancelled(action),
            // 声明面与执行面不同源就会走到这里（准入面已经把没声明的 ID 拒了）。
            _ => ActionOutcome::failure(crate::core::UNKNOWN_CAPABILITY),
        }
    }
}

impl AppPort for TimerApp {
    fn app_id(&self) -> &str {
        TIMER_APP_ID
    }

    fn description(&self) -> &str {
        "给自己约时间：到点之后你会被叫醒一次（自持物，住在你自己的环境根里）"
    }

    fn declared_capabilities(&self) -> Capabilities {
        // 声明必须与实例的真实能力一致：OS 的归属索引就是按这份声明建的（与 notes 同款）。
        Self::own_capabilities()
    }

    /// **两个写动词各配一个复核**（2026-10-13）。
    ///
    /// ⚠ **更正一处站不住的理由**：这一格从前写的是"自持物写**不需要授权、也不需要复核**"。
    /// 前半句对，**后半句把两件事混成了一件**：`app_contract.rs` 的 `declared_effects` 写着
    /// 「**声明不是权限**：它不授予、不撤销、也不替代准入；它只是'系统这一格能不能机械判断'
    /// 的依据」。⇒ 自持物写**照样是写**（它改的是进程之外的东西），因此照样该说清类别与复核
    /// ——这与"要不要授权"无关。notes 那边就是这么做的（四个写各配一个复核，注释原话：
    /// 「删除也是写（自己的地盘上的写，不需要授权），因此**照样**配一个复核」）。
    ///
    /// ⚠ **它还有一个机械后果**（§3.73 起）：那一关（"动它之前先看形状"）问的正是 `is_write()`
    /// ⇒ **不声明就是悄悄免了那一关**。于是同一个动作类（`timer_set` 与 `notes_create`）在那之前
    /// 行为不同，而那个不同**没有人打算过**——两条测试里都登记着它。
    fn declared_effects(&self) -> Vec<crate::app_contract::EffectDeclaration> {
        use crate::app_contract::{EffectClass, EffectDeclaration};
        vec![
            // **可重复写**：身份（`t-<序号>`）是**执行时**取的、不在参数里 ⇒ 同参数第二次**会成功**
            // （多出一条同刻同标签的）。与 `notes_create` 同类，因此复核天生**弱在归因**。
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_SET),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_VERIFY_PENDING)),
            ),
            // 除名也是写：同参数第二次"表里没有这个 id"照旧成功（与 `notes_delete` 的类别同款）。
            EffectDeclaration::new(
                ActionId::new(CAPABILITY_CANCEL),
                EffectClass::RepeatableWrite,
                Some(ActionId::new(CAPABILITY_VERIFY_CANCELLED)),
            ),
        ]
    }

    /// 来源档 = **这一段回执里携带的内容从哪个口子进来**。
    ///
    /// `timer_list` 带回的是**自己约下的时刻**（自持物），因此是
    /// [`crate::app_contract::SourceClass::SelfAuthored`]；`timer_set` / `timer_cancel` 只回报系统对这次
    /// 动作的确认，因此是 [`crate::app_contract::SourceClass::System`]。
    fn declared_sources(&self) -> Vec<crate::app_contract::SourceDeclaration> {
        use crate::app_contract::{SourceClass, SourceDeclaration};
        self.declared_capabilities()
            .items()
            .iter()
            .map(|capability| {
                let class = match capability.id().as_str() {
                    CAPABILITY_SET | CAPABILITY_CANCEL => SourceClass::System,
                    _ => SourceClass::SelfAuthored,
                };
                SourceDeclaration::new(capability.id().clone(), class)
            })
            .collect()
    }
}
