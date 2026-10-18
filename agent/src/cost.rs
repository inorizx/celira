//! **代价读数**：这一次活动花了多久、用了多少量。
//!
//! 它是四要素蒸馏里「代价」那一格的取材：时长与用量**各自有唯一的来源**，本件只把它们
//! 收在同一处，好让收场那一刻一次入账。
//!
//! ## 为什么是两个独立的来源，而不是一个
//!
//! | 那一格 | 谁独占它 | 为什么 |
//! |---|---|---|
//! | **时长** | **宿主循环** | 起止时刻只有它拿得到：活动什么时候开始、什么时候收场是它的事实 |
//! | **用量** | **适配层** | 厂商响应只有它读得到（`usage` 就在那一次响应里） |
//!
//! 两边都**只写原文、不换算**：时长是"从起点到收场过了多少毫秒"这一个数，用量是厂商报的
//! 那几个字段。本层不做加法、不做换算、不判断多少算多——Core 的纪律是**搬运不解释**。
//!
//! ## 它不是什么
//!
//! - **不是金额**：金额是用量×价目派生出来的，而价目表在机器配置里会变。账本记事实、不记汇率。
//! - **不是第二事实源**：它是一次活动**内**的暂存，收场时随 `RunEnded` 一起落盘。
//! - **不进 Core**：Core 只把这份读数原样搬进事实行，不读它的含义（见 `commit.rs`）。
//!
//! ## 三个 writer 的边界（形状而不是约定）
//!
//! - 用量：只有适配层拿得到 [`UsageWriter`]（与状态栏那一格的写柄同款形状）；
//! - 时长：只有宿主填得进 [`RunCost::elapsed_millis`]——它在收场那一刻现算，不走 cell。

use std::sync::{Arc, Mutex};

/// 这一次活动的**用量读数**：厂商响应里报的那几个数，原样搬运。
///
/// **取值只挑这三个字段**（`prompt` / `completion` / `total`）：两条 wire 都报这几个，
/// 而"报了什么记什么"会让同一格在不同厂商下形状不同——所以只收**共同的那三个**，
/// 其余字段不进来（它们不是同一件事，硬塞进同一行会让蒸馏端读到互相矛盾的口径）。
///
/// 三个字段各自可选：厂商漏报其中之一是**它的问题**，不是我们要编的数——
/// 缺的那一格如实缺着，不补零（补零会说"用了 0"，那是假话）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Usage {
    /// 送进去的上下文量（chat 与 Responses 两条 wire 各自的原文字段，落到这里同一口径）。
    pub prompt_tokens: Option<u64>,
    /// 本次生成量（同上）。
    pub completion_tokens: Option<u64>,
    /// 厂商报的总量。
    pub total_tokens: Option<u64>,
}

impl Usage {
    /// 三个字段一个都没有 ⇒ 这一轮什么都没报，调用方**不该**写下这一格。
    pub fn is_empty(&self) -> bool {
        self.prompt_tokens.is_none()
            && self.completion_tokens.is_none()
            && self.total_tokens.is_none()
    }
}

/// 一次活动的**代价读数**（时长 ＋ 用量）。
///
/// 两格各自独立可选：没有时长（宿主没记）与没有用量（厂商没报）是两件事，各缺各的。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct RunCost {
    /// **时长**：从活动起点到收场那一刻的毫秒数。
    ///
    /// 归**宿主**填：它独占那两个时刻。`None` = 这一格没有被记（恢复路径、离线夹具）。
    pub elapsed_millis: Option<u64>,
    /// **用量**：厂商响应报的那几个数（可能整格都没有）。
    pub usage: Option<Usage>,
}

/// 用量 cell 的**写**侧：只有适配层持有它。
///
/// 它只做一件事——把厂商响应里报的那几个数**原样**记下来，不换算、不累计、不推断。
/// 与状态栏那一格同款形状：没有它的那一侧拿不到写入口，
/// 因此"唯一写者"是形状而不是约定。
#[derive(Clone, Debug)]
pub struct UsageWriter {
    latest: Arc<Mutex<Option<Usage>>>,
}

impl UsageWriter {
    /// 记下这一次响应报告的用量（厂商原文，缺的字段不补）。
    pub fn report(&self, usage: Usage) {
        *self.latest.lock().expect("用量 cell 的锁只被短临界区持有") = Some(usage);
    }
}

/// 用量 cell 的**读**侧：会话与宿主各持一份。
#[derive(Clone, Debug, Default)]
pub struct UsageReport {
    latest: Arc<Mutex<Option<Usage>>>,
}

impl UsageReport {
    /// 新开一个空 cell，同时交出它的**唯一写柄**。
    ///
    /// 拿到的两半是分开的：写柄交给装配模型的那一方，cell 交给收场那一方。
    pub fn channel() -> (UsageWriter, UsageReport) {
        let latest = Arc::new(Mutex::new(None));
        (
            UsageWriter {
                latest: Arc::clone(&latest),
            },
            UsageReport { latest },
        )
    }

    /// 最近一次报告的用量；一条都没报过时是 `None`（不伪造读数）。
    pub fn latest(&self) -> Option<Usage> {
        *self.latest.lock().expect("用量 cell 的锁只被短临界区持有")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空 cell 读作 `None`：**不伪造**一个"用了零"。
    #[test]
    fn an_empty_cell_reads_as_none() {
        let (_writer, report) = UsageReport::channel();
        assert_eq!(report.latest(), None);
    }

    /// 写进去的就是读出来的：本层不做任何换算或补全。
    #[test]
    fn what_the_adapter_reports_is_what_comes_back() {
        let (writer, report) = UsageReport::channel();
        // 只报两个字段：第三个**如实缺着**，不补零。
        writer.report(Usage {
            prompt_tokens: Some(11),
            completion_tokens: Some(7),
            total_tokens: None,
        });
        assert_eq!(
            report.latest(),
            Some(Usage {
                prompt_tokens: Some(11),
                completion_tokens: Some(7),
                total_tokens: None,
            })
        );
    }

    /// **三个字段全缺**是可表达的读数：那一格"这次什么都没报"，而不是"用了零"。
    ///
    /// 它与"cell 还空着"（`latest() == None`）是两件事，因此 `is_empty` 不能拿来当作
    /// "没有读数"的判据——调用方先看 `latest()`，再看它自己要不要写这一格。
    #[test]
    fn an_all_absent_usage_is_a_reading_not_a_zero() {
        let all_absent = Usage::default();
        assert!(all_absent.is_empty());
        assert_eq!(all_absent.prompt_tokens, None);
        assert_eq!(all_absent.total_tokens, None);
    }
}
