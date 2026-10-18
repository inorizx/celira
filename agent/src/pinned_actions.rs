//! 动作 pin（TASK_68 P3）：主体把某些动作的**详情**钉在自己的视野里。
//!
//! 与 [`crate::pins_board`]（主体贴在墙上的**笔记**）是**两件事**——那边是主体的字，这边是**工具面**；
//! 名字里带 `action` 就是为了不与纸条混（一词一主）。
//!
//! ## ⚠ 2026-10-10 收口（主人裁决）：**常驻只有一个来源——主体自己钉的那一份**
//!
//! 从前这里还有一张**出厂默认集**（`im_say` / `im_rooms` / `im_read_history` / `notes_create` /
//! `im_typing`）：**开箱即在**、**不落盘**、而且**主体摘不掉**。三件事合起来是同一个病：
//! **"常驻"有两个来源，而其中一个主体动不了**——那不只是"哪几个动作常驻"这个**关于主体习惯的判断**
//! 被系统替主体做了，更是**主体的桌面上有一块不属于主体**。
//!
//! 主人给的判据一句话：**默认可以给，特权不能给**；而更彻底的一步是**两样都不给**——
//! 常驻**只能**是主体自己 pin 的。⇒ 本刀删掉那张表，于是：
//!
//! - **一个来源**：生效的 pin 集**就是** `<环境根>/pinned_actions` 那一份；
//! - **上限数全部**（[`MAX_PINS`]）：桌面 **7 行封顶、全是主体的**（从前是"出厂 5 ＋ 主体自己 7"）；
//! - **看得见、摘得掉**：那几条（如果主体要）就在主体自己家里，与别的 pin 没有区别。
//!
//! **代价如实登记**：pin 过的动作**免"先看格式"**（[`crate::core::Capability::is_exempt_from_inspection`]
//! 那条路之外的都要先查一次，而**免查是一程之内的**）⇒ 新家一开始零格位，**每一段**为主体要用的
//! 动词各多一轮，直到主体自己 pin 上。**这是主体自己的学费，不是系统的待遇。**
//!
//! ## 其余纪律（一字未改）
//!
//! - **主体的 pin**：`pin_action` / `unpin_action` 改的那一份，写在自己的地盘上（自持物）。
//! - **只在 Run 边界生效**：这一份是**装配期读回**的 ⇒ 一个 Run 之内主体再 pin，也不改变这一次的开场。
//! - **永不自动删**：这里没有任何一段代码会替主体删格位（系统不代管主体的桌面）。
//! - **OS 自己那五件不在这里**：它们的参数名由**动作目录**自带（声明方在
//!   [`crate::core::Capability::is_exempt_from_inspection`] 上标了"免查"）。这一块只列
//!   "要**先查格式**、但已被钉住 ⇒ 免查"的动作；免查 ⇒ 必须"免学"，两处各管一段。
//!   ⚠ **"常驻"与"免查"是两件事**：前者是"我把它摆在桌上"（归主体，7 条）；后者是
//!   "这一条不需要先看格式"（归声明方的机械属性）。混起来就会出现"桌上有一行主体动不了"。
//!
//! 它不持有跨活动状态、不认识历史 / 行动 / 模型 / 账本——与纸条板同款：只把**读回来的那一份**摆出来。

use std::io;
use std::path::{Path, PathBuf};

use crate::core::combine;
use crate::core::{
    ActionId, Capabilities, ContentBatch, ContentBlock, ContextSource, InjectionContext,
};

/// 这一块在注入里的标签。
pub const PINNED_ACTIONS_LABEL: &str = "pinned-actions";
/// 「这一次没读到」的机械读数（与通知栏、纸条板的 `unknown` 同族）。
pub const UNKNOWN_PINNED: &str = "unknown";

/// 自持物里那个文件的名字：`<环境根>/pinned_actions`（一行一个动作标识）。
pub const SELF_HELD_FILE: &str = "pinned_actions";

/// **桌面的上限**（TASK_71 桌面 P0 第 2 件）：`<环境根>/pinned_actions` 里最多几条。
///
/// ⚠️ **2026-10-10 收口：它数的是全部**——因为常驻只剩一个来源（主体自己钉的那一份，见模块文档）。
/// 从前它只数"自己那一份"，理由是"出厂格位摘不掉，算进去会自锁"；那张表删掉之后没有自锁了，
/// 而"桌面有限"这件事因此**第一次是真的**：**7 行封顶、全是主体的**（从前最多 5 ＋ 7 = 12 行）。
///
/// 超限**明确拒绝**（稳定分类 `pinned-limit-reached`），**绝不自动淘汰**：出路是"先摘一个"，
/// 由自己挑。上限的规则与出路写在 `pin_action` 的**详情卡**里（`inspect_action` 取得到）。
pub const MAX_PINS: usize = 7;

/// 自持物的路径。
fn self_held_path(environment_root: &Path) -> PathBuf {
    environment_root.join(SELF_HELD_FILE)
}

/// 读**生效的那一份格位**（2026-10-10 之后它是**唯一**来源：常驻只能 pin）。
///
/// **"文件不存在" = 一项都没钉**（出厂那张表已经删掉，见模块文档）——于是"没有"与"读不到"
/// 仍然分得开：读不出来 ⇒ `Err` ⇒ 上游渲染 `unknown`，绝不冒充"主体没钉过"。
pub fn pinned_actions(environment_root: &Path) -> io::Result<Vec<String>> {
    match std::fs::read_to_string(self_held_path(environment_root)) {
        Ok(text) => Ok(text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

/// **唯一写入口**：把"主体改过的那一份"写进自持物。
///
/// 空集 ⇒ 写成一个**空文件**（不删文件）：文件在、内容空 = "主体一项都没改过"。读取时它与
/// "从来没写过"本就是同一件事（[`pinned_actions`] 对两者都返回空集），所以不必区分；
/// 而删文件反而会造出一种"下次读成 `unknown`"的形状——把正常状态说成故障。
///
/// ⚠️ 写的是**自己的地盘**（自持物，不是跨边界效应）：按 notes 的先例不需要授权、也不需要复核。
/// （原来这里写的是"不走未决门"——**未决效果门已退役**，门是指令而世界只该给信息；理由本身与门
/// 无关：这一笔改的是主体自己的格位。）
/// 失败**如实返回 `Err`**——调用方必须把它当失败报出去，不许假装成功。
pub fn write(mine: &[String], environment_root: &Path) -> io::Result<()> {
    let mut text = String::new();
    for id in mine {
        text.push_str(id);
        text.push('\n');
    }
    std::fs::write(self_held_path(environment_root), text)
}

/// 改主体自己那一份格位时的**失败分类**（封闭；稳定字符串进主体的回执）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinFailure {
    /// 自持物读不出来：**绝不用空集去覆盖**——那会静默抹掉主体钉过的东西。
    Unreadable,
    /// 自己那一份已经满 [`MAX_PINS`] 项：**明确拒绝、不自动淘汰**。
    LimitReached,
    /// 写不进去（权限、盘满…）：旧的照旧，下一次开场用的还是它。
    Unwritable,
}

impl PinFailure {
    /// 稳定分类标记（进读数）。
    pub fn as_str(self) -> &'static str {
        match self {
            PinFailure::Unreadable => "pinned-actions-unreadable",
            PinFailure::LimitReached => "pinned-limit-reached",
            PinFailure::Unwritable => "pinned-actions-unwritable",
        }
    }

    /// 从稳定标记还原（解码用）。认不出的取值就是 `None`——**不猜**，也不就近映射
    /// （调用方按 fail-closed 处理）。
    ///
    /// **标记表就是权威**：`as_str` 与它**逐条对称**，有测试钉住（`tests/action_failure_wordlist.rs`）。
    /// 新增取值必须**同一刀**补在这里——这一格落在盘上的**行动结果**里，缺了解码端就是
    /// 「写得进去、读不出来」。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Option<Self> {
        Some(match text {
            "pinned-actions-unreadable" => PinFailure::Unreadable,
            "pinned-limit-reached" => PinFailure::LimitReached,
            "pinned-actions-unwritable" => PinFailure::Unwritable,
            _ => return None,
        })
    }
}

/// **改主体自己那一份格位**：钉住 / 摘掉一项。
///
/// 这一个入口就是那一份事实的**唯一写者**，所以域规则全在这里：读不到就拒（不拿空集去覆盖）、
/// 满了就拒（[`MAX_PINS`]，且**不自动淘汰**——腾哪个格位是主体的决策）、写失败如实报。
/// 成功时交回**给主体的那一块回执**：改了什么、现在几项、什么时候生效，由本件自己说。
///
/// ⚠ **不在这里的两件**（那是**路由事实**，归 `os`）：① 名字在不在**这一次的能力面**里；
/// ② 这一次装配**有没有环境根**。两件都是"能不能调"，不是"格位是什么"。
pub fn set_pinned(
    environment_root: &Path,
    name: &str,
    pinned: bool,
) -> Result<ContentBlock, PinFailure> {
    let mut mine = pinned_actions(environment_root).map_err(|_| PinFailure::Unreadable)?;
    let already = mine.iter().any(|id| id == name);
    // 上限数**全部**（常驻只剩一个来源，见 [`MAX_PINS`]）。重复钉与摘掉都不受上限影响
    // （它们不增加条数）。
    if pinned && !already && mine.len() >= MAX_PINS {
        return Err(PinFailure::LimitReached);
    }
    if pinned {
        if !already {
            mine.push(name.to_string());
        }
    } else {
        mine.retain(|id| id != name);
    }
    write(&mine, environment_root).map_err(|_| PinFailure::Unwritable)?;
    Ok(ContentBlock::new(
        PINNED_ACTIONS_LABEL,
        receipt(&mine, name, pinned, already),
    ))
}

/// 那一条回执的正文（措辞归本件：它知道这一次动的是哪一份事实）。
///
/// ⚠ **2026-10-10**：出厂那张表删掉之后，"出厂格位不受影响"那两句话**没有对象了**——
/// 现在主体钉的就是全部，摘掉就是真的摘掉（**如实说**，不许留一句会让主体误以为还有别处的旧话）。
fn receipt(mine: &[String], name: &str, pinned: bool, already: bool) -> String {
    let verb = if pinned { "钉住" } else { "摘掉" };
    // ⚠ 「**你自己钉着**」这句话是**这一份事实的所有者标记**：`tests/minimal_agent.rs` 的
    // `the_router_owns_no_domain_words` 断言它**必须留在这里**（路由那一层不许持有域词）。
    // 2026-10-10 之后它**更准了**：出厂那张表删掉，主体钉的就是全部。
    let mut text = if mine.is_empty() {
        format!("{verb}了 `{name}`。你现在自己钉着 0 项（`pinned_actions` 是空的）。")
    } else {
        format!(
            "{verb}了 `{name}`。你现在自己钉着 {} 项：{}。",
            mine.len(),
            mine.join("、")
        )
    };
    if already {
        text.push_str("\n（它本来就在里面：重复钉是明确的成功结果，不报错）");
    } else if pinned && mine.len() >= MAX_PINS {
        text.push_str(&format!(
            "\n（已经满了：{MAX_PINS} 项。要换别的进来，得先摘一个——淘汰哪个是你的决策。）"
        ));
    }
    text.push_str("\n下一次开场生效：这一次的视野已经定了，改动不回头改它。");
    text
}

/// 动作 pin 的来源：**开场一次**（与纸条板同款——快照不是活状态，之后不重摆）。
#[derive(Debug)]
pub struct PinnedActions<Inner> {
    /// 装配期读回来的那一份格位（2026-10-10 之后它是**唯一**来源）。
    ///
    /// `None` = 这一次**没读成**；`Some(vec![])` = 读成了、**主体一项都没钉**——
    /// 两层不是一回事（缺省不等于零）。
    mine: Option<Vec<String>>,
    inner: Inner,
}

impl<Inner> PinnedActions<Inner> {
    /// 用装配期读回来的那一份装配（可缺）+ 内层来源。
    pub fn new(mine: Option<&[String]>, inner: Inner) -> Self {
        Self {
            mine: mine.map(|rows| rows.to_vec()),
            inner,
        }
    }

    /// 生效的 pin 集：**就是读回来的那一份**（按标识去重，重复写同一个只算一次）。
    ///
    /// ⚠ 2026-10-10 之前这里还有"出厂默认在前"那一段——删掉之后，**顺序就是主体文件里的顺序**，
    /// 同一份自持物两次装配渲染逐字节相同。
    fn effective(&self) -> Vec<String> {
        let mut ordered: Vec<String> = Vec::new();
        if let Some(mine) = &self.mine {
            for id in mine {
                if !ordered.iter().any(|known| known == id) {
                    ordered.push(id.clone());
                }
            }
        }
        ordered
    }

    /// 本件那一条读数：pin 集里**在场**的动作逐条给参数表（名字 + 说明），末尾一句机械字节读数。
    ///
    /// 字节读数是**这一块自己占了多少上下文**的诚实说法（预算是读数，不是机制）；
    /// 不在场的动作**不渲染**（凭空造不出动作），一个都不在场就把零说出口。
    pub fn reading(&self, capabilities: &Capabilities) -> String {
        let Some(_) = &self.mine else {
            return format!("pinned = {UNKNOWN_PINNED}\n（这一次没读到；不冒充「没有 pin 过」）");
        };

        let mut listed = String::new();
        let mut rendered = 0usize;
        for id in self.effective() {
            let Some(capability) = capabilities.find(&ActionId::new(id.clone())) else {
                continue;
            };
            listed.push_str(&id);
            listed.push('\t');
            // 参数表的紧凑一行问 **`Capability` 自己**——一份权威：与动作目录里
            // 免查条目的写法同源，不在两处各写一遍连接串。
            listed.push_str(&capability.params_line());
            listed.push('\n');
            rendered += 1;
        }

        if rendered == 0 {
            // 一个都不在场：把零说出口（主体才知道是"空"，不是"没读到"）。
            // ⚠ 2026-10-10：这里从前还写着"出厂默认都在常量里"——那句话**没有对象了**。
            // ⚠ 2026-10-13：从前写的是"每段第一次要先看一次格式"（一切动作）。那一关今天
            // 只挡**会改东西的动作**（`EffectClass`，见 `os.rs::admit`）⇒ 这句跟着改窄，
            // 否则它就成了那句"免查必须同时免学"的反面：说了半天，主体照旧不知道读的动作不必等。
            return format!(
                "pinned = 0\n（你还没钉过任何动作：用 `pin_action` 钉住一个、`unpin_action` 摘掉；\
                 上限 {MAX_PINS} 项。不钉也能调——会改东西的动作每段第一次要先看一次格式，\
                 读的动作不必。）\n"
            );
        }
        let bytes = listed.len();
        format!("{listed}pinned = {rendered}, ~{bytes}B\n")
    }

    /// 此刻的内容块（标签是本件的身份）。
    pub fn line(&self, capabilities: &Capabilities) -> ContentBlock {
        ContentBlock::new(PINNED_ACTIONS_LABEL, self.reading(capabilities))
    }

    fn own_batch(&self, capabilities: &Capabilities) -> ContentBatch {
        ContentBatch::of_or_not_delivered(vec![self.line(capabilities)], PINNED_ACTIONS_LABEL)
    }

    /// 内层来源（只读）。
    pub fn inner(&self) -> &Inner {
        &self.inner
    }
}

impl<Inner: ContextSource> ContextSource for PinnedActions<Inner> {
    /// **开场一次**：与内层的开场事实一起递交（快照不是活状态，之后不重摆）。
    fn on_run_start(&mut self, context: &InjectionContext) -> ContentBatch {
        combine(
            self.inner.on_run_start(context),
            self.own_batch(context.capabilities()),
        )
    }

    /// 轮级照旧交给内层：这一件只在开场露面。
    fn before_turn(&mut self, context: &InjectionContext) -> ContentBatch {
        self.inner.before_turn(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Capability, ParamSpec};

    /// 临时根：按进程号分开，避免并行测试互相踩。
    fn temp_root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("celira-pinned-{}", std::process::id()))
    }

    /// 往返：写进去什么，读出来就是什么；**没写过**读成空集（不是故障）。
    #[test]
    fn what_is_pinned_is_what_comes_back() {
        let root = temp_root();
        std::fs::create_dir_all(&root).expect("可以建临时根");
        let _ = std::fs::remove_file(self_held_path(&root));

        // 没写过 ⇒ 空集：这是"主体没改过"，不是"读不到"（出厂默认在常量里，不需要落盘）。
        assert_eq!(pinned_actions(&root).expect("可以读"), Vec::<String>::new());

        write(&["notes_list".to_string(), "im_say".to_string()], &root).expect("可以写");
        assert_eq!(
            pinned_actions(&root).expect("可以读"),
            vec!["notes_list".to_string(), "im_say".to_string()]
        );

        // 写空集 ⇒ 空文件 ⇒ 读回空集（**不是 unknown**：不删文件就没有"读不到"这一态）。
        write(&[], &root).expect("可以写");
        assert_eq!(pinned_actions(&root).expect("可以读"), Vec::<String>::new());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 生效集：**就是读回来的那一份**（按标识去重，重复写同一个只算一次），顺序照文件里的顺序。
    #[test]
    fn the_effective_set_is_exactly_what_was_pinned_deduped() {
        let mine = vec![
            "im_say".to_string(),
            "notes_extra".to_string(),
            "im_say".to_string(),
        ];
        let board = PinnedActions::new(Some(&mine), ());
        let effective = board.effective();

        assert_eq!(
            effective,
            vec!["im_say".to_string(), "notes_extra".to_string()],
            "一个来源：主体写的那一份；重复的只算一次；顺序照主体写的"
        );
    }

    /// 一条合成能力（只为喂渲染：名字 + 一项参数）。
    fn capability(id: &str) -> Capability {
        Capability::new(
            ActionId::new(id),
            "合成的一句话",
            vec![ParamSpec::new("p", true, "参数说明")],
        )
    }

    /// **块级诚实回退**：格位块自己超限时留下一句读数，**不退空批次**。
    ///
    /// 生产里进不到这里（在场动作的参数表都是声明出来的，而且按 [`MAX_PINS`] 只有几条），
    /// 因此这条用例**构造**一个超限块：一条参数表巨大的合成能力。它证明的是"这一处不是静默的"
    /// ——将来哪个 App 声明出一张撑爆批次的参数表，主体会读到那句读数，而不是"格位区什么都没有"。
    #[test]
    fn an_oversize_block_leaves_a_reading_instead_of_vanishing() {
        let params: Vec<ParamSpec> = (0..200)
            .map(|n| ParamSpec::new(format!("p{n}"), false, "参数说明".repeat(40)))
            .collect();
        let in_play = Capabilities::of(vec![Capability::new(
            ActionId::new("huge"),
            "合成的一句话",
            params,
        )]);
        let board = PinnedActions::new(Some(&["huge".to_string()]), ());

        // 那一块原本多大（标签 + 正文），读数要原样报出来。
        let expected = PINNED_ACTIONS_LABEL.len() + board.reading(&in_play).len();
        assert!(expected > ContentBatch::MAX_BYTES, "前提：这一块真的超限");

        let batch = board.own_batch(&in_play);
        assert_eq!(batch.blocks().len(), 1, "留下一块读数，不是空批次");
        assert_eq!(batch.blocks()[0].label(), PINNED_ACTIONS_LABEL);
        assert_eq!(
            batch.blocks()[0].text(),
            format!("超限未给（原始 {expected} B）")
        );
        assert!(!batch.is_source_failure(), "它不是来源失败");
    }

    /// **常驻只有一个来源**（2026-10-10 裁决）：渲染的就是**主体钉的那一份**——
    /// 在场但没被钉的动词**一条都不进**格位块（它们走既有的 `inspect_action` 路径），
    /// 末尾那句读数报的是**真渲染条数**。
    ///
    /// ⚠ 出厂那张表删掉之后，"精选集"这条判据**没有对象了**：从前这里断言的是"精选 5 条 ∧ 在场"，
    /// 现在断言的是"**主体钉了什么就渲染什么**"——而"桌面是空的"那一态也由这里钉住。
    #[test]
    fn only_what_is_pinned_gets_rendered() {
        // 在场的有 8 条，主体只钉了 2 条。
        let in_play = Capabilities::of(vec![
            capability("im_say"),
            capability("im_rooms"),
            capability("im_read_history"),
            capability("notes_create"),
            capability("im_typing"),
            capability("notes_list"),
            capability("notes_read"),
            capability("im_recall"),
        ]);
        let mine = vec!["notes_list".to_string(), "im_recall".to_string()];
        let board = PinnedActions::new(Some(&mine), ());
        let reading = board.reading(&in_play);

        let rendered: Vec<&str> = reading
            .lines()
            .filter(|line| line.contains('\t'))
            .map(|line| line.split('\t').next().expect("制表符前是标识"))
            .collect();
        assert_eq!(
            rendered,
            vec!["notes_list", "im_recall"],
            "只有主体钉的、且在场的才渲染，次序照主体写的: {reading}"
        );
        assert!(
            reading
                .lines()
                .filter(|line| line.contains('\t'))
                .all(|line| line.contains(" = ")),
            "每一条都自带参数表（格位块的既有形状）: {reading}"
        );
        let count = reading.lines().last().expect("末行是机械读数");
        assert!(
            count.starts_with("pinned = 2, ~") && count.ends_with('B'),
            "末尾那句读数报的是真渲染条数（6 条在场但没钉的不算）: {count}"
        );

        // **桌面是空的**：主体一项都没钉 ⇒ 把零说出口，而且指路（怎么摆上来、上限几条）。
        let empty = PinnedActions::new(Some(&[]), ()).reading(&in_play);
        assert!(empty.starts_with("pinned = 0\n"), "{empty}");
        assert!(empty.contains("pin_action"), "要说清怎么摆上来: {empty}");
        assert!(empty.contains("先看一次格式"), "要说清不摆的代价: {empty}");

        // **没读到**与"没钉"分得开：`None` ⇒ `unknown`，绝不冒充空桌面。
        let unknown = PinnedActions::new(None, ()).reading(&in_play);
        assert!(unknown.starts_with("pinned = unknown\n"), "{unknown}");
    }
}
