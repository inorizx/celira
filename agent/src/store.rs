//! 身份环境与只追加经历存储：让"确认保存的经历"跨进程存在。
//!
//! 这一层只回答四个问题，不回答其他：
//!
//! 1. **这是谁的环境**——一个环境目录里的稳定身份记录；打开时校验归属，不静默新建、不重置数据；
//! 2. **现在谁在写**——同时只允许一个写者，机制是不可靠的"文件存在"判断之外的真实排他；
//! 3. **经历保存到哪**——两份只追加存储：经历正文（Trace）与操作事实（Fact）；
//! 4. **保存到哪里为止**——只承诺**已确认耐久的前缀**，损坏尾部必须被检测到，不伪装完整成功。
//!
//! 本模块是**允许碰文件系统的封闭名单里的一员**——名单（13 项）的唯一权威是
//! `tests/minimal_agent.rs` 的 `FILE_SYSTEM_SOURCES`，这里刻意不复述（从前这里写的是
//! "除文件 App 与组合入口之外唯一"，那句话早已不成立：第二个 App 起，拥有自己存储的 App
//! 自己碰文件系统，而那正是"App 有自己的数据"应有的样子）。
//! 它不认识 Core 的循环，也不认识任何 App。Core 只通过中立接缝说话，见 `core::CommitSeam`；
//! 边界模块清单见 `lib.rs`，这里刻意不复述，以免守卫只能靠豁免维持。
//!
//! ## 承诺与不承诺
//!
//! 承诺：只有**成功调用过 `sync_all`（`fsync`）的整行**才被当作"已确认"。返回值返回之后，
//! 内存序号与已登记/已封存集合才会推进，因此"已确认"意味着"这一行确实被提交给了操作系统"。
//!
//! **不**承诺：
//!
//! - 掉电零丢失——`sync_all` 只是把数据交给存储设备，不证明设备自身的写缓存能在掉电后保住它；
//!   `sync_all` 之前崩溃的那一行也可能"看起来在"也可能"不在"。可承诺的只有"已确认前缀"；
//! - 目录项耐久——目录项在环境**创建**时建立并同步一次，之后只追加，不再重复同步目录；
//! - 两份存储之间的跨存储原子性——经历与事实各自独立确认，矛盾时**如实报告**，不用相同
//!   `RunId` 冒充事务一致；
//! - 拷贝两个环境目录后的**全局**同身份排他——只保证**该环境目录**的排他打开。
//!
//! ## 排他机制
//!
//! 用 `std::fs::File::try_lock`（操作系统级文件锁）。它不是"检查文件是否存在"这种可以被
//! 崩溃遗留物欺骗的写法，也**不**依据锁文件时间戳猜测死进程并抢占：持有者进程退出时，
//! 操作系统自动释放锁，下一个打开者自然拿到。

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::clock::{ClockHandle, unix_millis};
use crate::core::{
    Action, ActionId, ActionOutcome, ActionParams, ContentBatch, ContentBlock, Decision,
    ModelOutput, ModelResponse, Observation, ProtocolData, TraceEntry,
};
use crate::cost::{RunCost, Usage};
use crate::foundation::{
    ActionResult, AgentId, OperationId, Outcome, Record, ResultState, RunId, SubjectEnding,
    SystemEnding, Turn,
};

/// 环境目录里身份记录的文件名。
const IDENTITY_FILE: &str = "identity.json";
/// 排他写资格的锁文件名（内容无意义，只用于持有操作系统锁）。
const WRITER_LOCK_FILE: &str = ".writer.lock";
/// 经历正文（Trace）的文件名。
const TRACE_FILE: &str = "trace.ndjson";
/// 操作事实（Fact）的文件名。
const FACTS_FILE: &str = "facts.ndjson";
/// 经历存储里封存条目的动作名。
pub const TRACE_SEALED: &str = "sealed";

/// 环境目录操作失败的原因。每一类保留自己发生的层级，不压成一个字符串。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// 环境目录不存在。
    NotFound,
    /// 目标不是目录（或路径形状不可用作环境根）。
    WrongKind,
    /// 环境目录里没有身份记录：这是一个"还没有身份"的位置，不能当成已有环境打开。
    IdentityMissing,
    /// **创建**时发现这个位置里有**历史**（两份存储），却没有身份。
    ///
    /// 它不是"还没有身份的空位置"（那才是可以创建的），而是一个**丢了身份的既有环境**——
    /// 按"创建"往下走会把那两份历史清空，那是**静默销毁**。因此**拒绝，一个字节都不动**。
    ///
    /// **判据贴着这一次操作自己的写集**：`create` 唯一会清空的就是 [`TRACE_FILE`] 与
    /// [`FACTS_FILE`]，因此只有它们在场才算"会销毁东西"。别的既有内容（家侧的 `agent.conf`、
    /// 主体的 `notes/` / `self_concept` / `timers` …）**不在判据里**——它们不会被 `create` 动，
    /// 而且家侧按设计就把 `agent.conf` 写在这个目录里（2026-10-06 真机门禁抓到：按"任何东西在场
    /// 就拒绝"写会让**每一个新家**都装不起来，也会把一个半成品家永久锁死）。
    ///
    /// 它与 [`StoreError::IdentityMissing`] 分开：那条说"这里没有身份"（`open` 的判据），
    /// 这条说"这里没有身份、**却有历史**"（`create` 的判据）——分开才知道该看哪一边；它与
    /// [`StoreError::StorageMissing`] 正好是同一件事的两面（那是"身份在、存储不在"）。
    HistoryWithoutIdentity { files: Vec<&'static str> },
    /// 身份记录存在但读不出来或内容不合法。**不**因此新建身份或重置数据。
    IdentityCorrupt,
    /// 身份记录属于另一个 Agent：拒绝打开，不泄漏任何经历。
    IdentityMismatch { expected: AgentId, found: AgentId },
    /// 已经有别的写者持有这个环境时拒绝打开。
    AlreadyOpen,
    /// 存储尾部里的一行读不出来：不是本写入者格式的合法条目（缺字段、JSON 不合法、
    /// 种类/取值不认识，或最后一段没有停在换行边界上）。
    ///
    /// 已确认前缀仍然有效，不自动丢弃未知尾部。
    CorruptTail { seq: u64 },
    /// 条目的序号不单调：文件被外部改写成不自洽的顺序。
    OutOfOrder { previous: u64, found: u64 },
    /// 这一行**能解析**，但内容不合法：解出来也没有一个可解释的含义（例如一条收场事实
    /// 既没有主体收场也没有系统收场，或者两种都有）。
    ///
    /// 它与 [`StoreError::CorruptTail`] 是两类不同的问题，必须分开报告：
    /// 前者是"格式不成立"，这里是"格式成立、语义不成立"。两者都**拒绝打开**，
    /// 都不用"就近猜一个含义"把非法内容读成有效历史。
    MalformedEntry { seq: u64 },
    /// 要写下的那一行带着一个**词表之外**的模型失败分类：**拒绝写**。
    ///
    /// 它是 [`StoreError::MalformedEntry`] 的**另一半**——那条管"读进来的一行不可解释"，
    /// 这条管"写出去的一行会变得不可解释"。两条合起来才是那条纪律的全文：
    /// **持久字段上的词表一律 fail-closed**。读侧早就做到了（认不出的取值 ⇒ 整行拒绝），
    /// **写侧从前一个字都不查**。
    ///
    /// 为什么写侧也会不合法：`SystemEnding::ModelFailed` 的载荷在拆掉 Foundation ↔ Core
    /// 那个环之后是一根**裸字符串**（见 `SystemEnding` 的文档），**类型不再保证它在词表之内**。
    /// 一个词表之外的值会在盘上落下一行**谁也读不回来**的记录——那正是本条要挡住的。
    ///
    /// 拒绝发生在**写之前**：文件一个字节都不动，内存里的序号也不推进。
    ///
    /// ⚠️ 它只管**模型**失败分类（`Ending` 与 `run-ended` 两处）。行动结果那一格
    /// （`Observed.failure`）是**另一套词表**（`ActionOutcome::failure_class`），
    /// 归它自己的层，本层不解释、也不校验。
    UnknownFailureClass,
    /// 一个**已经有身份**的环境里缺少某份存储文件。
    ///
    /// 这不等于"这个环境还没有历史"：身份在，说明这是一个已建立的环境，缺文件是损坏。
    /// 因此**不**按空历史重建，也不静默新建一份空文件。
    StorageMissing { file: &'static str },
    /// 一条**暂停标记**记录带着词表之外的标签：**拒绝判读**。
    ///
    /// `storage-failure` 阶段下的标签是**封闭词表**（[`PAUSED_LABEL`] / [`ACKNOWLEDGED_LABEL`]）。
    /// 认不出的取值只能通向一句假话——"暂停、原因未说明"——而它可能恰恰相反（例如旧构建写下的
    /// `resumed`：那是一个**已经被显式确认过**的环境）。因此这里**拒绝**，与
    /// [`StoreError::UnknownFailureClass`] 同一条纪律：**持久字段上的词表一律 fail-closed**。
    ///
    /// 它和 [`StoreError::MalformedEntry`] 分开：那条管"整行不可解释"，这条管"整行成立、
    /// 只是这一格不认识"——分开才知道该去看哪个字段。
    UnknownPauseMarker { seq: u64 },
    /// 这个活动身份在这个环境里已经被登记过了：不得再开第二次。
    ///
    /// 它保证"重试保存不会重复插入"：同一个 `RunId` 的 `RunStarted` 只能有一条，
    /// 因此新进程不可能悄悄把一次旧活动重开成两份经历。
    RunAlreadyKnown { run: RunId },
    /// 这个活动已经封存：它的经历到此为止，不再接受新的追加。
    ///
    /// 封存是"给历史一个明确的到此为止"，因此封存之后的追加必须被拒绝；**新**的活动不受影响。
    RunSealed { run: RunId },
    /// 这个写者的上一次写入没有停在已确认的行边界上，因此**不再继续追加**。
    ///
    /// 它不是文件损坏，而是写者自己的状态：继续往一个"不知道停在哪一半"的句柄里写，
    /// 只会把损坏滚大。重新打开环境会重新校验整份前缀（可能报损坏，也可能读出干净前缀）。
    TailUnconfirmed,
    /// 一条记录的内容与它自己的校验和对不上（或校验和读不出来）。
    ///
    /// 它是**可判定的损坏**：那串字节与写下去时不是同一串。旧记录（加校验和之前写下的）
    /// 没有校验和，因此不会落进这里——它们由**后面**那条记录的链护着（见 [`chain_checksum`]）。
    ChecksumMismatch {
        /// 第几行（从 0 起）。
        seq: u64,
    },
    /// 一般 I/O 失败。
    Io,
}

impl StoreError {
    /// 稳定分类标记。
    pub fn as_str(&self) -> &'static str {
        match self {
            StoreError::NotFound => "not-found",
            StoreError::WrongKind => "wrong-kind",
            StoreError::IdentityMissing => "identity-missing",
            StoreError::HistoryWithoutIdentity { .. } => "history-without-identity",
            StoreError::IdentityCorrupt => "identity-corrupt",
            StoreError::IdentityMismatch { .. } => "identity-mismatch",
            StoreError::AlreadyOpen => "already-open",
            StoreError::CorruptTail { .. } => "corrupt-tail",
            StoreError::OutOfOrder { .. } => "out-of-order",
            StoreError::MalformedEntry { .. } => "malformed-entry",
            StoreError::ChecksumMismatch { .. } => "checksum-mismatch",
            StoreError::UnknownFailureClass => "unknown-failure-class",
            StoreError::StorageMissing { .. } => "storage-missing",
            StoreError::UnknownPauseMarker { .. } => "unknown-pause-marker",
            StoreError::RunAlreadyKnown { .. } => "run-already-known",
            StoreError::RunSealed { .. } => "run-sealed",
            StoreError::TailUnconfirmed => "tail-unconfirmed",
            StoreError::Io => "io",
        }
    }

    /// 这是不是"结构损坏"（文件不自洽），而不是"某一步没有被完整确认"。
    ///
    /// 两者必须分开报告：损坏说明**格式**不成立，未完成确认说明**格式**成立、只是某一步没走完。
    /// "能解析但内容不合法"同样属于结构损坏：它也不是"某一步没走完"，而是这一行本身就不可解释
    /// （[`StoreError::UnknownPauseMarker`] 同属这一支：那一行读得出来，只是这一格的取值
    /// 不在词表里——盘上那份东西按本构建解释不了）。
    pub fn is_structural_damage(&self) -> bool {
        matches!(
            self,
            StoreError::CorruptTail { .. }
                | StoreError::OutOfOrder { .. }
                | StoreError::MalformedEntry { .. }
                | StoreError::ChecksumMismatch { .. }
                | StoreError::UnknownPauseMarker { .. }
                | StoreError::IdentityCorrupt
                | StoreError::StorageMissing { .. }
        )
    }
}

// ---------------------------------------------------------------------------
// 格式与校验和：**另一份文件**（`store/format.rs`，2026-10-08 拆）
// ---------------------------------------------------------------------------
//
// 最后一块，也是所有东西都依赖的那一块：编码/解码（一行一个 JSON 对象 + 换行）、
// **CRC-32 链**（每条记录带行尾校验和并串成一条链）、以及盘上那几类记录的形状。
// 它搬走之后，本文件只剩**自述 ＋ 错误词表 ＋ 接线**——那正是门面该有的样子。
mod format;
pub use format::*;

// ---------------------------------------------------------------------------
// 身份与准入：**另一份文件**（`store/environment.rs`，2026-10-08 拆）
// ---------------------------------------------------------------------------
//
// 同款判据：换「谁在哪个文件里实现这套格式」，**盘上格式一个字节不动**。这一段是"环境"这个
// 对象本身——身份记录、排他写资格、两份追加存储的准入与健康、开场读数、事实视图。
mod environment;
pub use environment::*;

// ---------------------------------------------------------------------------
// 分卷与索引：**另一份文件**（`store/parts.rs`，2026-10-08 拆）
// ---------------------------------------------------------------------------
//
// 拆的判据是「**谁在哪个文件里实现这套格式**」，不是"格式不好"：这一段自成一体（封口的段、
// 一张目录、每卷一份卷内偏移索引、事实侧同一套形状），搬出去**不动盘上格式一个字节**。
mod parts;
pub use parts::*;

// ---------------------------------------------------------------------------
// 记录 ↔ 轨迹：**另一份文件**（`store/projection.rs`，2026-10-08 拆）
// ---------------------------------------------------------------------------
//
// 同款判据：换「谁在哪个文件里实现这套搬运」，**盘上格式一个字节不动**。它管两个方向：
// `*_payload`（轨迹条目 → 盘上一行）、`rehydrate` / `outcome_of`（盘上一行 → 轨迹条目），
// 外加四个**标签**（→ 人话）与**按活动读回**（`list_runs` / `read_run_at`）。
mod projection;
pub use projection::*;

// 父模块自己要用到的那些名字：**显式列出来**，不靠 glob（两个子模块都 `use super::*` 时，
use environment::{
    PAUSED_STAGE, TailPolicy, decode_trace_line, is_run_ending, last_acknowledged_seq, read_facts,
    read_trace_file, read_trace_file_for_run,
};
use parts::{
    FACTS_PARTS_DIR, FACTS_ROLL_BYTES, PARTS_DIR, PARTS_INDEX, RUN_SEALED, RUN_STARTED,
    RUNS_LINE_MARK, RunSpan, SEGMENT_ROLL_BYTES, decode_part, folded_of, load_fact_parts,
    load_parts, merge_summaries, part_file_name, part_files, read_part_index, read_trace_slice,
    rebuild_part_index, summaries_at, summaries_of, unfinished_after, with_part_runs,
    write_facts_index, write_part_index, write_parts_index,
};
use projection::{
    blocks_value, decode_blocks, decode_extra, decode_pairs, decode_state, escape_json,
    extra_value, extract_string_field, io_error, pairs_value, state_str, stored_index, subject_str,
    sync_directory, system_str,
};
