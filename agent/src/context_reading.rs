//! **屏幕读数格**：最近一次厂商报告的占用读数（会话内暂存，不落盘）。
//!
//! ## 它为什么自己一个家
//!
//! 它与时钟同一个病：这两侧从前住在 `status_bar.rs` 里，于是**两个模型适配层**为了报一次读数，
//! 要 import 一个**显示区组件**——芯片认识状态栏。按手机模型，那是下层依赖上层。
//!
//! 它**不是**状态栏的一个字段：状态栏只是**读它**的那一处渲染。真正的形状是
//! **适配层写、显示区读**，而这一格夹在中间，属于手机本体——因此它有自己的家。
//!
//! ## 一个写者，一个读者
//!
//! 写柄与读侧在**构造时就分开**（[`ContextReading::channel`]）：写柄只交给装配模型的那一方，
//! 读侧只交给要渲染它的那一方。因此"只有一个写者"不是约定，是形状
//! （机械守卫见 `tests/minimal_agent.rs::only_the_adapter_writes_the_reading_cell`）。
//!
//! ## 它是**一段之内**的读数（2026-10-10 在场那一刀之后）
//!
//! ⚠ 这一格从前**不认段**：它只存"最近一个数"。于是**一段在场跨多声铃**之后，一段的
//! **开场那一行会显示上一段最后一次调用的占用**——而那一行的标签是 `context = 这一场送进模型的
//! 内容量`（见 `status_bar::READING_GUIDE`）。**那是拿另一段的数说这一段的事。**
//!
//! 现在读数**带着它属于哪一段**：写的时候记下 `RunId`，读的时候只认**同一段**的。
//! 别的段报的 ⇒ 读作 [`UNKNOWN_CONTEXT_READING`]——与"还没报过"同一个读数，因为对**这一段**
//! 而言它们确实是同一件事：**这一段还没有人报过。**

use crate::foundation::RunId;
use std::sync::{Arc, Mutex};

/// 这一格读不出来时的读数：这一次会话还没有收到过任何厂商报告（开场、会话首轮、恢复后首轮）。
///
/// 它与 [`crate::clock::UNKNOWN_TIME`] 值相同但**来源不同**，因此是两个常量而不是一个：
/// 一个是"宿主钟给不出表示"，一个是"还没有人报过"。将来其中一个变了不该牵连另一个。
pub const UNKNOWN_CONTEXT_READING: &str = "unknown";

/// 屏幕报告 cell 的**读**侧：最近一次厂商报告的占用读数。
///
/// ## 状态归属（六问）
///
/// | 问题 | 答案 |
/// |---|---|
/// | 权威所有者 | **没有持久权威**：它暂存"最近一次厂商报告的占用读数"，原文来自厂商响应，本进程不持久化 |
/// | 允许的写者 | **只有适配层**（它持有写柄 [`ContextReadingWriter`]） |
/// | 生命周期 | 会话（进程）内；会话销毁即消失，跨活动、跨进程都不带走。⚠ **读数本身是段级的**（见下面 `latest_for`） |
/// | 失败后状态 | **保持旧值**。语义就是"最近一次报告"，因此失败不需要另设失效机制 |
/// | 持久化 | **不持久化**：盘上零新增种类，事实系统里一个字都没有 |
/// | 重启解释 | 首轮 `unknown`，第一份响应成功之后恢复真实读数 |
///
/// 它**不是**第二事实源：读数只是"厂商说的最近一个数"，既不参与任何判断，也不被任何执行路径读取
/// ——只有状态栏那一行把它渲染出来。窗口是**宿主的声明**（不是这里算出来的），因此
/// `context = 占用/窗口` 里的分母从来不来自本 cell。
#[derive(Clone, Debug, Default)]
pub struct ContextReading {
    latest: Arc<Mutex<Option<(RunId, u64)>>>,
}

impl ContextReading {
    /// 新开一个空 cell，同时交出它的**唯一写柄**。
    ///
    /// 拿到的两半是分开的：写柄交给装配模型的那一方（生产入口），cell 交给装配声明的读取方。
    /// 因此"谁在写"在类型上就是看得见的。
    pub fn channel() -> (ContextReadingWriter, ContextReading) {
        let latest = Arc::new(Mutex::new(None));
        (
            ContextReadingWriter {
                latest: Arc::clone(&latest),
            },
            ContextReading { latest },
        )
    }

    /// **属于 `run` 那一段的**最近一次报告。
    ///
    /// - 最近一次报告就是这一段报的 ⇒ `Some(占用)`；
    /// - 最近一次报告是**别的段**报的（或者还没有人报过）⇒ `None`（渲染成
    ///   [`UNKNOWN_CONTEXT_READING`]）。⚠ 这两种情况在这里是**同一件事**：对这一段而言，
    ///   确实还没有人报过——而把上一段的数摆到这一段的开场那一行，是**拿另一段的数说这一段的事**。
    pub fn latest_for(&self, run: &RunId) -> Option<u64> {
        match &*self.latest.lock().expect("屏幕 cell 的锁只被短临界区持有") {
            Some((reported_in, used)) if reported_in == run => Some(*used),
            _ => None,
        }
    }
}

/// 屏幕报告 cell 的**写**侧：只有适配层持有它。
///
/// 它只做一件事——把厂商响应里报告的占用数字**原样**记下来，不换算、不累计、不推断。
/// 没有它的那一侧（状态栏、组合层、入口）读不到写入口，因此"唯一写者"不是约定而是形状。
#[derive(Clone, Debug)]
pub struct ContextReadingWriter {
    latest: Arc<Mutex<Option<(RunId, u64)>>>,
}

impl ContextReadingWriter {
    /// 记下这一次报告的占用读数（厂商响应里的原值），并记下**它是哪一段报的**。
    ///
    /// `run` 由适配层从这一次请求的视野里取（`View::run`）——它不需要额外知道任何东西，
    /// 那一段本来就在它手上。
    pub fn report(&self, run: &RunId, used: u64) {
        *self.latest.lock().expect("屏幕 cell 的锁只被短临界区持有") = Some((run.clone(), used));
    }
}
