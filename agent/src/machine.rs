//! 机器处境（Machine）：**这台机器是什么**——平台 + shell，开场一行，只陈述事实。
//!
//! ## 为什么必须告诉主体
//!
//! 我们分平台发包（win-x64 / linux-x64 各一份），**编译期**决定「用哪套实现去执行」。但
//! **命令是模型写的，不是代码写的**——编译期常量决定不了模型生成什么文本。今天它看不到平台，
//! 只能从环境根路径的风格（`/home/...` 还是 `D:\...`）去**猜**；猜错的表现是它按自己更习惯的
//! 语言写命令（Linux 上写 PowerShell 语法、Windows 上写 bash），用户看到的是「这 Agent 连列个
//! 目录都不会」。
//!
//! 命令词法也**不能**靠工具层「翻译」（谁翻译谁是坑：静默换 shell 是错的——用户指定 bash 却被
//! 换成另一种 shell，命令会被另一种语法错误解析）。所以只有一条正路：**把事实告诉主体，让它按
//! 事实写**。
//!
//! ## 事实从机器读，不从别处读
//!
//! 环境根可以**搬机器**（一支 Agent 换机器 = 搬环境根）⇒ 平台不是这个 Agent 的属性，而是
//! **它此刻所在那台机器的处境**。因此：
//!
//! - 这条事实**每次开场都要在**（处境，不是一次性出厂说明）；
//! - 取值**每次开场从机器现读**（[`Platform::of_machine`]）：读的是**机器自己的报告**——
//!   Linux 内核在 procfs 里自报的名字、Windows 系统设进进程环境的那个值。两条都不是我们写死的
//!   字面量，也不是环境根里带着走的字符串；读不出来（或报告不在封闭表里）⇒ 如实说 `unknown`，
//!   **不猜**（不默认到某一档，也不拿别处的字符串顶替）。
//!
//! ## 判据：环境事实 ≠ 能力事实（本件最重要的一条）
//!
//! 本件只说「这台机器是什么」（平台、shell 是什么），**绝不承诺「你能干什么」**——终端有没有、
//! 档位是什么，是别的刀的事。两者混在一起就会出现「我被告知有 shell，却根本没有那只手」。
//! 因此这一行里**不出现任何动词名、不出现「你可以……」**：它是一句关于机器的陈述，不是一张
//! 能力清单。
//!
//! ## 平台真理源一处
//!
//! 平台名与 shell 名的映射**只写一份**（[`Platform::name`] / [`Platform::shell`]），两个平台
//! 共用同一份渲染（[`reading_of`]）——不许在两个平台各写一份文案（两份文案就是两个事实源，
//! 迟早对不上）。
//!
//! ## 为什么是**新的一件**，而不是塞进邻居
//!
//! - **状态栏**是**每轮重摆的活状态**（时间 + 轮次）：机器在一次 Run 里不会换，一条不变的
//!   事实每轮重摆只是吃视野预算（判据 2）；
//! - **通知栏**说「世界有什么新的」，素材由宿主投影上来：本件说的是机器，**素材只能从机器读**；
//! - **出厂说明**（`orientation`）住在**环境根**里、是主体的地盘（可删可改可迁移）：平台事实
//!   恰恰**不能**跟着环境根走——搬一次家就会带走一条过时的平台；
//! - **事件卡片**是 Run 内的增量（每轮判一次），本件是**开场快照**。
//!
//! 所以它是一件新的开场快照件（与纸条板、持有物同款）：只在开场摆一次，之后各轮不重摆。
//!
//! ## 第三格：**这一段由谁想**（2026-10-08）
//!
//! 这一行本来是两格（平台 + shell），现在可以多两格 —— `model` 与 `reasoning_effort`（这一段
//! 由谁想、想多用力）：它们都是**宿主声明**的那一份（模型配置门的裁决），**不是从机器读的**。
//! 几件事实同处一行，是因为它们回答同一个问题：**这一段是在什么样的执行体里想**（在哪台机器上、
//! 由谁想、多用力）。来源不同这件事写在每一格的词里：`platform` / `shell` 从机器现读，
//! `model` / `reasoning_effort` 由装配交给本件。
//!
//! ⚠ **本件仍然不读环境根一个字节**（那条判据没变）：那两格是**装配交进来的值**，不是本件去读的。
//! 没有声明时它们**整格不出场**（不是 `unknown`）——"没声明"与"读不出来"是两件事。
//!
//! ⚠ **为什么不另立一块**：处境链有**硬上界 11 层**（C8：要加第 12 层，先删一层）。这两格不是
//! 新的一件快照，它们就是"执行体"那一行的第 3、4 格。
//!
//! ## 它不是什么
//!
//! 它不读环境根、不读网络、不落盘、不认识任何 App、也不解释内容；它**永不让活动失败**——机器
//! 读不出来只是如实说 `unknown`。

use crate::core::combine;
use crate::core::{ContentBatch, ContentBlock, ContextSource, InjectionContext};

/// 这一块的标签（与 `status-bar` / `holdings` / `previous-activity` 同族）。
pub const MACHINE_LABEL: &str = "machine";

/// 机器读不出来时的机械读数（与通知栏、纸条板、持有物同族：**缺省不等于零**）。
pub const UNKNOWN_MACHINE: &str = "unknown";

/// Linux 内核在 procfs 里自报操作系统名的那个文件（**机器自己的报告**）。
const KERNEL_OSTYPE_REPORT: &str = "/proc/sys/kernel/ostype";

/// 内核报告里的那个名字（逐字节比较——不做模糊匹配、不做前缀匹配）。
const KERNEL_REPORT_VALUE: &str = "Linux";

/// Windows 由系统设进每个进程环境的那个变量（名字短得像个别名，但它是系统设的）。
const SYSTEM_VARIABLE: &str = "OS";

/// 那个变量的取值（逐字节比较）。
const SYSTEM_VARIABLE_VALUE: &str = "Windows_NT";

/// 平台：**从机器读到的**那个（封闭两档）。
///
/// 两档就是两种**命令词法**（也是我们出包的两个平台）；我们没有为别的平台出包 ⇒ 别的报告一律
/// 读不出来（`None`），不许硬塞进最近的这一档。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// 这台机器是 Windows。
    Windows,
    /// 这台机器是 Linux。
    Linux,
}

impl Platform {
    /// **从机器读**：这个进程所在平台的报告。
    ///
    /// 两条读法都是**运行时**读这台机器自己的报告，没有一条是我们写死的字面量：
    ///
    /// 1. Linux：内核在 [`KERNEL_OSTYPE_REPORT`] 里报自己的名字；
    /// 2. Windows：系统在进程环境里设 [`SYSTEM_VARIABLE`]。
    ///
    /// 两条都不成立（或报告不在封闭表里）⇒ `None`：如实说不知道，**不猜**。
    pub fn of_machine() -> Option<Self> {
        if let Ok(report) = std::fs::read_to_string(KERNEL_OSTYPE_REPORT)
            && report.trim() == KERNEL_REPORT_VALUE
        {
            return Some(Platform::Linux);
        }
        if std::env::var(SYSTEM_VARIABLE).is_ok_and(|value| value == SYSTEM_VARIABLE_VALUE) {
            return Some(Platform::Windows);
        }
        None
    }

    /// 平台名（**给主体看的稳定词**；封闭表）。
    pub fn name(self) -> &'static str {
        match self {
            Platform::Windows => "windows",
            Platform::Linux => "linux",
        }
    }

    /// **本平台默认的 shell 名**（平台 → shell 的**唯一一份映射**）。
    ///
    /// 判据是「命令词法是谁的」：Windows 上是 PowerShell，Linux 上是 bash。两个平台共用同一份
    /// 渲染（[`reading_of`]），取值差异只在这里一处。
    ///
    /// ⚠ 这只是**shell 的名字**，不是「你有终端可用」——那只手是别的刀的事（见模块文档那条判据）。
    /// 它是**运行时**从这台机器现读的处境；终端那侧的「**本构建里有没有能用的臂**」是**编译期**判断
    /// （`terminal_app::TerminalTier::available`）——两者不是一回事，不能互相顶替：一个说"这台机器
    /// 的默认命令语法是什么"，另一个说"这份二进制里编进了哪些手"。今天 Linux 上两者恰好同词
    /// （都是 `bash`），那是**事实一致**，不是同一条判据（由用例逐词钉住）。
    pub fn shell(self) -> &'static str {
        match self {
            Platform::Windows => "powershell",
            Platform::Linux => "bash",
        }
    }
}

/// 那一行读数：**平台 + shell**（唯一一份渲染，两个平台共用）。
///
/// 一行两格：`platform = <平台名> · shell = <shell 名>`。机器读不出来时两格都如实写
/// [`UNKNOWN_MACHINE`]——不冒充（不猜一个平台，也不给一个默认 shell）。
///
/// 路径风格（`/home/...` 还是 `D:\...`）**不另写一格**：它是平台名的蕴含，同一件事不写两遍。
pub fn reading_of(platform: Option<Platform>) -> String {
    match platform {
        Some(platform) => format!(
            "platform = {} · shell = {}",
            platform.name(),
            platform.shell()
        ),
        None => format!("platform = {UNKNOWN_MACHINE} · shell = {UNKNOWN_MACHINE}"),
    }
}

/// 一条**声明出来的**值：空白（或空串）不算声明 ⇒ `None`（那一格不出场）。
///
/// 判据是"**没声明**"与"**读不出来**"分得开：本件对声明的那几格**不写 `unknown`**——
/// `unknown` 是机器读不出来时的读数（[`reading_of`]），不是"宿主没说"的读数。
fn declared(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

/// 机器处境：一块开场快照 + 内层来源。
///
/// `model` 与 `reasoning_effort` 是**宿主声明**的那两格（这一段由谁想、想多用力）：`None`
/// 或缺省值 ⇒ 那一格**不出场**（本件不从环境根读它们，也不写 `unknown`）。两格**各自独立**
/// 出场：今天它们同源（都从将要发请求的那个对象取），因此"只有档位没有名字"不可达——
/// 但本件的判据**不靠**这一点（各自按自己那一格判）。
#[derive(Debug)]
pub struct Machine<Inner> {
    model: Option<String>,
    reasoning_effort: Option<String>,
    inner: Inner,
}

impl<Inner> Machine<Inner> {
    /// 用内层来源装配本件，并带上宿主声明的**模型名**与**思考强度**（都可以不给）。
    pub fn new(model: Option<String>, reasoning_effort: Option<String>, inner: Inner) -> Self {
        Self {
            model,
            reasoning_effort,
            inner,
        }
    }

    /// 内层来源（只读）。
    pub fn inner(&self) -> &Inner {
        &self.inner
    }

    /// 本件那一条读数——**每一次开场都从机器现读一次**（后两格来自装配声明）。
    pub fn reading(&self) -> String {
        let mut line = reading_of(Platform::of_machine());
        if let Some(model) = declared(&self.model) {
            line.push_str(" · model = ");
            line.push_str(model);
        }
        if let Some(effort) = declared(&self.reasoning_effort) {
            line.push_str(" · reasoning_effort = ");
            line.push_str(effort);
        }
        line
    }

    /// 此刻的内容块（标签是本件的身份）。
    pub fn line(&self) -> ContentBlock {
        ContentBlock::new(MACHINE_LABEL, self.reading())
    }

    /// 本件自己的批次（超限时留一句读数，**不退空批次**——与纸条板、持有物同律）。
    fn own_batch(&self) -> ContentBatch {
        ContentBatch::of_or_not_delivered(vec![self.line()], MACHINE_LABEL)
    }
}

impl<Inner: ContextSource> ContextSource for Machine<Inner> {
    /// **开场一次**：本件在这里摆出它唯一那条读数，与内层的开场事实一起递交。
    fn on_run_start(&mut self, context: &InjectionContext) -> ContentBatch {
        combine(self.inner.on_run_start(context), self.own_batch())
    }

    /// **这里不产生机器块**：机器在一次 Run 里不会换，快照不每轮重摆。内层照旧。
    fn before_turn(&mut self, context: &InjectionContext) -> ContentBatch {
        self.inner.before_turn(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity_origin::ActivityOrigin;

    /// 三档读数（两档已知 + 一档读不出来）——形状与取值逐字节钉住。
    fn all_readings() -> Vec<String> {
        vec![
            reading_of(Some(Platform::Linux)),
            reading_of(Some(Platform::Windows)),
            reading_of(None),
        ]
    }

    /// **平台 → shell 的映射只写一份**：两档各自的名字与 shell 逐字节钉住。
    ///
    /// （Windows 真机这一刀没有，映射靠这条用例守——见 `tests/machine.rs`。）
    #[test]
    fn the_platform_to_shell_mapping_is_one_table_for_both_platforms() {
        assert_eq!(Platform::Linux.name(), "linux");
        assert_eq!(Platform::Linux.shell(), "bash");
        assert_eq!(Platform::Windows.name(), "windows");
        assert_eq!(Platform::Windows.shell(), "powershell");
        assert_eq!(
            reading_of(Some(Platform::Linux)),
            "platform = linux · shell = bash"
        );
        assert_eq!(
            reading_of(Some(Platform::Windows)),
            "platform = windows · shell = powershell"
        );
    }

    /// **读不出来不许猜**：两格都如实写 `unknown`，不默认到某一档、也不给默认 shell。
    #[test]
    fn an_unreadable_machine_is_said_out_loud_and_never_guessed() {
        assert_eq!(reading_of(None), "platform = unknown · shell = unknown");
    }

    /// 形状：**一行**、两格、`名 = 值`（一行两格是"别占视野"的机械保证）。
    #[test]
    fn the_reading_is_exactly_one_line_of_two_fields() {
        for text in all_readings() {
            assert_eq!(text.lines().count(), 1, "{text}");
            let fields: Vec<&str> = text.split(" · ").collect();
            assert_eq!(fields.len(), 2, "{text}");
            assert!(fields[0].starts_with("platform = "), "{text}");
            assert!(fields[1].starts_with("shell = "), "{text}");
            assert!(!text.contains('\n'), "{text}");
        }
    }

    /// **长度边界**：这一块在结构上不可能超限（最坏输入下也远小于上限），因此永不需要回退。
    #[test]
    fn the_reading_is_bounded_so_it_can_never_need_the_fallback() {
        for text in all_readings() {
            assert!(text.len() < 256, "{} 字节：{}", text.len(), text);
            assert!(
                text.len() * 4 < ContentBatch::MAX_BYTES,
                "{} 字节：{}",
                text.len(),
                text
            );
        }
    }

    /// **不承诺能力**：这一行里不出现动词名，也不出现「你可以」。
    #[test]
    fn the_fact_never_promises_a_capability() {
        for text in all_readings() {
            for word in [
                "terminal",
                "命令",
                "可以",
                "能力",
                "shell_run",
                "terminal_run",
            ] {
                assert!(!text.contains(word), "{word} 出现在：{text}");
            }
        }
    }

    /// 件是**普通的委托链**：内层只读可查；读数与"从机器现读"逐字节一致。
    #[test]
    fn the_piece_is_a_plain_chain_over_its_inner_source() {
        let machine = Machine::new(None, None, ActivityOrigin::new());
        let _inner: &ActivityOrigin = machine.inner();
        assert_eq!(machine.reading(), reading_of(Platform::of_machine()));
        assert_eq!(machine.line().label(), MACHINE_LABEL);
        assert_eq!(machine.line().text(), machine.reading());
    }

    /// **声明的那两格**：声明了就多一格、逐字节等于给的那个标识符；没声明（或只给了空白）⇒
    /// 那一格不出场，**不写 `unknown`**（"没声明"与"读不出来"是两件事）。
    ///
    /// 两格**各自独立**：今天它们同源（都从将要发请求的那个对象取），但本件的判据不靠那一点。
    #[test]
    fn the_declared_fields_are_optional_and_never_faked() {
        let declared = Machine::new(
            Some("fixture-model-1".to_string()),
            Some("high".to_string()),
            ActivityOrigin::new(),
        );
        let line = declared.reading();
        assert!(
            line.ends_with(" · model = fixture-model-1 · reasoning_effort = high"),
            "{line}"
        );
        assert_eq!(line.split(" · ").count(), 4, "{line}");

        // 只有名字那一格：三格，档位那一段一个字节都不出现。
        let only_model = Machine::new(
            Some("fixture-model-1".to_string()),
            None,
            ActivityOrigin::new(),
        );
        let line = only_model.reading();
        assert!(line.ends_with(" · model = fixture-model-1"), "{line}");
        assert_eq!(line.split(" · ").count(), 3, "{line}");
        assert!(!line.contains("reasoning_effort"), "{line}");

        // 一格都没有（含"只给了空白"）：还是两格，两段的词都不出现。
        for (model, effort) in [
            (None, None),
            (Some(String::new()), Some(String::new())),
            (Some("  ".to_string()), Some("\t".to_string())),
        ] {
            let machine = Machine::new(model, effort, ActivityOrigin::new());
            let line = machine.reading();
            assert_eq!(line.split(" · ").count(), 2, "没有声明就还是两格: {line}");
            assert!(!line.contains("model"), "{line}");
            assert!(!line.contains("reasoning_effort"), "{line}");
        }
    }

    /// **块级诚实回退**：这一块自己超限时留下一句读数，**不退空批次**（与纸条板同律）。
    ///
    /// 这里用一件"假装读数很长"的同形状件来触发（本件真实读数有上一条用例钉住）：
    /// 回退路径属于 `ContentBatch`，本件只是照律使用它。
    #[test]
    fn an_oversize_block_leaves_a_reading_instead_of_vanishing() {
        let huge = ContentBlock::new(MACHINE_LABEL, "字".repeat(70_000));
        let expected = MACHINE_LABEL.len() + huge.text().len();
        assert!(expected > ContentBatch::MAX_BYTES, "前提：这一块真的超限");
        let batch = ContentBatch::of_or_not_delivered(vec![huge], MACHINE_LABEL);
        assert_eq!(batch.blocks().len(), 1, "留下一块读数，不是空批次");
        assert_eq!(batch.blocks()[0].label(), MACHINE_LABEL);
        assert_eq!(
            batch.blocks()[0].text(),
            format!("超限未给（原始 {expected} B）")
        );
        assert!(!batch.is_source_failure(), "它不是来源失败");
    }
}
