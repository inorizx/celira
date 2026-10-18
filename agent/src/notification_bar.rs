//! 通知栏：系统服务层的**第二个件**——把「世界有什么新的」摆成一块**开场快照**。
//!
//! 它与 [`crate::status_bar`] 是**同族**，但两者管的不是同一件事：
//!
//! | | 状态栏 | 通知栏 |
//! |---|---|---|
//! | 管什么 | **自己的处境**（几点、预算、屏幕、上次去看） | **外面世界的动静**（哪几间房有新的） |
//! | 什么性质 | **活状态**：每轮重摆一次（当前值） | **快照**：Run 开场摆一次，之后不再变 |
//! | 谁给的数 | 装配声明（钟 / 窗口 / 锚） | **宿主读回来的投影**（谁读见下；**两半各有各的锚**） |
//!
//! 两条读数各说一句话，但**同一个 `since`**：状态行那行说「新从哪一刻起算」，本件说「那一刻以来
//! 有哪些房、各几条新的」（安静的不逐行点名，见 [`NotificationBar::reading`]）。它们是**同一份事实
//! 的两处渲染**（同一个时刻、同一个来路、**同一个格式**——
//! 十进制 unix 毫秒，见 [`crate::clock::anchor_value`]），不是两个来源。
//! 形状收在一处是有意的：那个值的唯一用处是递给 `im_rooms(since=…)`，所以两处都得是它吃的形状。
//!
//! 三条纪律（都是这一刀裁定的）：
//!
//! 1. **它不读、不解析、不判断**：拿到什么摆什么。读是**宿主**的事（每唤醒恰好一次，
//!    走 App 的只读投影入口），本件只负责"哪里显示、怎么显示"。
//! 2. **它是快照**：只在 `on_run_start` 摆一次，`before_turn` 返回**空批次**——
//!    Run 内不刷新。理由：把不变的几行每轮重灌是持续成本（8 轮就是几十行陈旧重复），
//!    而"现在又有什么新的"这条**回忆路径已经存在**（自己有 `im_rooms` + 锚）。
//! 3. **`unknown` 是合法读数，而且它**不**挡主体的 Run**：投影没读回来，如实写 `unknown`。
//!    这与门铃刻意相反——门铃是唤醒通道（断了要 fail-closed），
//!    通知栏只是便利（看不见就自己去看），因此它**永不**让一次活动失败。
//!
//!    ⚠ **2026-10-10 补**：**"没有锚"与"没读到"是两件事，各说各的**。投影那一份里有两半——
//!    增量那一半要锚（没锚就问不出来，**不编一个 0**），**未读那一半不需要锚**（它按世界记着的
//!    读位置算，见 `im_app::render_new_since` 与契约 §3.2 的 D3）。⇒ **没有锚也读**：从前
//!    "没锚就不读"让刚重启 / 第一次开机的那一段**整块失明**。

use std::time::SystemTime;

use crate::clock::{UNKNOWN_SINCE, anchor_value};
use crate::core::combine;
use crate::core::{ContentBatch, ContentBlock, ContextSource, InjectionContext};

/// 通知栏内容块的标签（呈现时的区域名）。
pub const NOTIFICATION_BAR_LABEL: &str = "notification-bar";

/// 「投影没读回来」时的机械读数（与 `unknown` 同族但来源不同：**这一次读失败了**）。
pub const UNKNOWN_COUNTS: &str = "unknown";

/// 通知栏：一块开场快照 + 内层来源。
///
/// 它**不持有**任何跨活动状态（摆完就完了）、不认识历史 / 行动 / 模型 / 账本——
/// 因此它不可能替主体判断"该不该去看"。
#[derive(Debug)]
pub struct NotificationBar<Inner> {
    /// 宿主读回来的投影（`None` = 这一次没有投影：没锚、或读失败）。
    projection: Option<String>,
    /// 投影的起点（与状态行第四行**同一个声明**）：上一次是**什么时候去看的**。
    /// `None` = 宿主不知道「新」该从哪个时刻算起。
    since: Option<SystemTime>,
    inner: Inner,
}

impl<Inner> NotificationBar<Inner> {
    /// 用宿主的声明装配：投影正文（可缺）+ 起点（可缺）+ 内层来源。
    pub fn new(projection: Option<&str>, since: Option<SystemTime>, inner: Inner) -> Self {
        Self {
            projection: projection.map(str::to_string),
            since,
            inner,
        }
    }

    /// 本件那一条读数。
    ///
    /// **零正文**：投影的全部内容就是"自某刻以来**哪几间房各有几条新的**"（安静的房不点名）——一行材料正文、预览或发送者都没有。
    /// 深度读永远是**主体**的动作。
    ///
    /// **折叠由投影那一侧定，本件只搬运、不解析、不重排**：计数为 0 的房**不逐行点名**，改由
    /// 分母（可见几间）与余数那两处承担"没出现的就是零新"（按事件定价，形状见
    /// [`crate::im_app`] 的 `render_new_since`）。因此安静的世界在视野里是一行，不随房间数长。
    pub fn reading(&self) -> String {
        let since = match self.since {
            Some(at) => anchor_value(at),
            None => UNKNOWN_SINCE.to_string(),
        };
        match (&self.projection, self.since) {
            // 有投影就摆（**有锚没锚都算**）：起点的时刻 + 投影正文。
            //
            // ⚠ **2026-10-10 改**：从前这里是"有锚才摆"，而**没锚时整块只写 `unknown`**
            // ——刚重启 / 第一次开机的那一段因此失明。投影里那一半"未读"按**世界记着的读位置**
            // 算，**它不需要锚**（契约 §3.2 的 D3：两个锚，不合并）。
            (Some(counts), _) => format!("since = {since}\n{counts}"),
            // 没有投影、但有起点：如实说**这一次没读到**（不冒充"没有新的"）。
            (None, Some(_)) => format!("since = {since}\ncounts = {UNKNOWN_COUNTS}"),
            // 既没投影、也没起点：**两件事各说各的**（"没锚"与"没读到"不是同一件事）。
            (None, None) => format!("since = {UNKNOWN_SINCE}\ncounts = {UNKNOWN_COUNTS}"),
        }
    }

    /// 此刻的内容块（标签是本件的身份）。
    pub fn line(&self) -> ContentBlock {
        ContentBlock::new(NOTIFICATION_BAR_LABEL, self.reading())
    }

    fn own_batch(&self) -> ContentBatch {
        ContentBatch::of_or_not_delivered(vec![self.line()], NOTIFICATION_BAR_LABEL)
    }

    /// 内层来源（只读）。
    pub fn inner(&self) -> &Inner {
        &self.inner
    }
}

impl<Inner: ContextSource> ContextSource for NotificationBar<Inner> {
    /// **开场一次**：本件在这里摆出它唯一那条读数，与内层的开场事实一起递交。
    fn on_run_start(&mut self, context: &InjectionContext) -> ContentBatch {
        combine(self.inner.on_run_start(context), self.own_batch())
    }

    /// **这里不产生通知栏块**：快照不每轮重摆（见模块文档第 2 条）。内层照旧。
    fn before_turn(&mut self, context: &InjectionContext) -> ContentBatch {
        self.inner.before_turn(context)
    }
}
