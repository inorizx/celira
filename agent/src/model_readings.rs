//! **逐轮读数账**：每一次模型响应报告的读数，**一行一次**，追加在 `<环境根>/model_readings.ndjson`。
//!
//! ## 它为什么自己一个家
//!
//! 这一族读数今天**半存在**，而且是两个不同的形状：
//!
//! - **占用**那一格在 [`crate::context_reading`]：一格 = **最新值**，只给状态行渲染；
//! - **用量**那一格在 [`crate::cost`]：随 `RunEnded` 落盘，但那是**整段收场时的一条**。
//!
//! 两格都是"当下值"——**看完就没了**。而"这一段为什么贵""前缀有没有被复用"问的是**逐轮的形状**，
//! 不是最新值，也不是一条汇总。所以本件**不是第三格**：它是一份**账**。
//!
//! **两格各写各的**：占用仍归 [`crate::context_reading`]、用量仍归 [`crate::cost`]——本件不接管、
//! 不合并、也不改写它们任何一格（连"顺手补一个数"都不做）。它只回答"每一轮各报了什么"。
//!
//! 要它的理由是**给眼睛、不给判断**：操作者事后翻文件（2026-10-11 原话：「我后续看日志文件这种，
//! 我至少能翻出来」）。⇒ 没有任何执行路径读它。
//!
//! ⚠ **为什么不折进 [`crate::cost`]**：那是 **Core 认识**的中立件（Core 把它原样搬进事实行）；
//! 折进去等于让一个 Core 认识的中立模块去碰文件系统——**比多一个模块更坏**。
//!
//! ## 一个写者，一个读者，而且读者不在程序里
//!
//! 写柄与读侧在构造时就分开（[`ModelReadings::channel`]）：写柄只交给**装配模型的那一方**，
//! 读侧只交给**要报故障的装配入口**。因此"只有一个写者"不是约定，是形状。
//!
//! 这份账的**真正读者是操作者**（事后翻文件那一双眼睛），不是任何执行路径——没有任何代码
//! 读它做判断。读侧今天只回答一件事：**它坏过没有**。
//!
//! ⚠ **本模块一个字都不打**：报是入口的事（库不打印），所以"坏过没有"只在收尾那一处出现一次。
//! ⚠ **它不自己取锁**（没有第二把文件锁）：单写者由**环境那把写锁**派生——写只发生在真的调用
//! 模型时，而调用要过准入。
//!
//! ## 状态归属（六问）
//!
//! | 问题 | 答案 |
//! |---|---|
//! | 权威所有者 | **没有权威**：它记的是厂商原文的搬运，原文在**那一次响应**里；本件不裁决任何事实 |
//! | 允许的写者 | **只有适配层**（它持有写柄 [`ModelReadingsWriter`]） |
//! | 生命周期 | 文件跨活动、跨进程累积（追加，从不重写）；写柄随这一次进程 |
//! | 失败后状态 | **第一次失败记下来，之后不再尝试**（原因多半一样，重复只会拖慢每一次调用）；写失败**不返回给调用方**——那会把一次正常响应变成失败 |
//! | 持久化 | **落盘**：`<环境根>/model_readings.ndjson`，只追加 |
//! | 重启解释 | 文件留着（那是它的用处）；写柄重建，**不读回旧行** |
//!
//! ## 它**在第一次真的记一行时才出现**（这是刻意的）
//!
//! 构造**不碰文件系统**（[`ModelReadings::channel`] 只是把路径记下来）：**一次没写过，盘上就
//! 一份空的都没有**。
//!
//! 理由不是洁癖，是一条已经写死的纪律：**没有凭据的时刻不得改动环境**（用例
//! `manual_entry::the_creation_flag_is_explicit_and_never_implicit` 钉着它——一个停在装配模型
//! 那一步的 run，环境目录必须逐字节不动）。装配期先建一个空文件，正好违反那一条：那**是**改动。
//!
//! 代价是"打不开"这件事**发现得晚**（第一次要写的时候才知道），而它照旧被记下来、由读侧在收尾
//! 时报出——**不静默**。
//!
//! ⚠ **静默跳过才是最坏的那一条**：这一族不落盘之后，操作者会把"翻不到"读成"没发生"——而这两件
//! 事在诊断上正好相反。所以打不开**不拦流程**，但必须**说出来**。
//!
//! ## 它不是什么
//!
//! - **不是恢复输入**：`recovery` 一个字节都不读它。删掉整个文件，恢复与续跑照旧成立；
//! - **不进主体的视野**：它不在 `file_view` 的派生地址白名单里（那张表只有 `history/` 与
//!   `manual.md`），因此主体读不到它——这是"厂商怎么服务的"，不是主体的经历；
//! - **不进 Core 的判断**：不参与准入、不参与失败分类、不参与收场——**Core 根本不认识它**。
//!   这一族读数里，**既落盘、又在 Core 视野之外的只有本件**：占用那一格不落盘，用量那一格
//!   随 `RunEnded` 进 Core 的事实行；
//! - **不是第二份事实权威**：与 `trace` / `facts` 无关，也不与它们共享任何格式或协议。
//!
//! **负空间（这一件不碰的东西）**：不动 `RunEnded { cost }` 的落盘形状（**零持久格式改动**）·
//! 不进 Ledger / Trace / View / 通知栏 / 首屏 · **不新增判断读者** · 不改装配顺序
//! （**不违反 `RUNTIME_TERMINAL_FORM.md` §5**）· 不碰 im 服务端与任何冻结契约。
//!
//! ## 它只装"谁报的"，不装"谁算的"
//!
//! - **厂商报的**：四个计数，原样搬运；不报的字段**缺席**（不补零、不由别的字段推出来）；
//! - **我们记的**：活动、轮次、走的是哪一条线协议；
//! - **算出来的**：一个都不写。速率、命中率、金额都是**显示层现算**的东西——写进来就会让
//!   "厂商报的"与"我们算的"混成同一格，日后没人分得清哪个是原文。
//!
//! ## 为什么不记时刻
//!
//! 时间在这套系统里是**读数**不是事实：唯一的时间来源是装配注入的钟（[`crate::clock`]），
//! 而它**不落盘、不进事实**——两份追加存储里一个字都没有它。给本文件加一列时刻，等于在
//! 持久层新开一类"落盘的时间"，而那是**另一个决定**（谁拥有那个时刻、夹具怎么确定），
//! 不在这一刀里。行序与 `turn` 一起已经能回答"哪一轮"，要"几点"另议。
//!
//! ## 为什么不 `sync_all`
//!
//! 与两份追加存储**不同款**：那两份承诺"已确认前缀"，因此每次追加都同步。本件按**诊断账**
//! 那一档走——**可以丢失**，所以只保证"写出去的字节交给了操作系统"，不承诺掉电零损失。
//! 这不是省事，是**不许把它的可靠性说高**：它的用处是事后翻，不是恢复。
//!
//! ## 凭据与正文的边界
//!
//! **绝不写**：钥匙、授权头、请求体、响应体、正文。**只写**上面那两类机械读数。
//! 判据：**同一个故障两次发生，这一行必须逐字节相同**——所以这里没有一处动态错误详情。
//!
//! ## 怎么读这份账（判据，代码说不出）
//!
//! - **要判"前缀有没有被破坏"，看 `cached` 是不是单调递增；不要看命中率**——命中率会被
//!   "这一轮新内容多不多"主导；
//! - **要横向比两段或多段**，比的是**每轮落定的未命中量**（绝对量，不受分母大小影响），不是比值。
//!
//! ⚠ 这两条是**读法**，不是判据入口：今天没有任何代码按它们判事（本件不进判断）。
//!
//! ## 两条已量的结论（射程写在一起）
//!
//! - **能力扩展不会撞坏前缀**：`tools` 那一格变了（一次能力目录展开生效后的第一个请求一次涨了
//!   约 7 KB），缺口落在正常带内、`cached` 照旧**单调上涨** ⇒「同一次活动内只增不减」那条扩展
//!   策略**在缓存上没有代价**（能力目录变大是**追加在尾部**的，尾部变长不吃前缀）。
//!   ⚠ 射程：**一家厂商、一次跑**；
//! - **换一家之后还稳不稳，只在两家上量过**（本家直连 ＋ 一家网关）——**不是"所有厂商都这样"
//!   的证据**。
//!
//! ## 一条未裁的观察（本件不裁决）
//!
//! 把每轮与前一轮对齐，`cached(n)` 比 `prompt(n−1)` 少掉一段**固定长度**的尾部（那一次跑里是
//! 468，落在同一小段区间内）。对照消息形状：最后那条消息 ＝ **[静态头（出厂说明）] ＋
//! [机械事实]**，而机械事实很短（后三个读数**每轮都变**）⇒ **静态头是常量，却住在"每轮都要
//! 重写的那条消息"里**，它的位置每轮往后移，因此**永远不可能被命中**，每轮白算那一截。
//!
//! **要收掉它，唯一的办法是把静态头挪进稳定前缀——而那改的是主体开场读到的东西与顺序
//! （今天的顺序由主体自己定）。那是产品决定，不是优化题。** ⚠ 这一条**未裁**：本件只登记形状，
//! 不提议改装配；地界在 [`crate::orientation`]（`agent/README.md` §3.32）。
//!
//! ## 两条只观察到、不许当结论的
//!
//! - 两支 Agent 的第一轮**都恰好报 768** 命中——与"两段开场有一段共同前缀被缓存命中"相符，
//!   **两例不足以定论**（另一家第一轮报 **0** ⇒ 那个数是**厂商侧**的现象，不是这套装配攒出来的）；
//! - **跨 Run 的前缀也在厂商缓存里**（两段开场是同一份）——**一行证据**。
//!
//! 登记它们，是因为**这正是这份账被造出来要看的那类问题**。
//!
//! ## 覆盖缺口（如实列，不假装）
//!
//! - **跨进程续跑那一段的读数**没有真机证据（量的都是**一段之内**的几轮）；
//! - **写失败之后的行为**没有行为用例（句柄一旦拿到就有效，没有注入点）——覆盖到的只有
//!   **打不开**那一档 ⇒「第一次失败之后不再尝试」**只有代码形状，没有行为证明**；
//! - 两个进程同时写**不会发生**，但那是**别处**的保证（本件不自己取锁）；
//! - **崩溃中途的行数**未验证；**没有轮转与保留期限**；
//! - **`wire = responses` 那一格没有端到端证据**：字段形状已由直接问网关核过（与读取器认的
//!   那一个正好一致），但"账里真的落下一行 `wire: responses` 且带命中数"**还没跑到** ⇒
//!   这一格**收窄了，没有关掉**。

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::foundation::{RunId, Turn};

/// 这一族读数落在这台机器上的文件名。
///
/// ⚠ **它不含供应商名**，也不含"厂商"那个英文词：这个名字进的是**被扫描的源文件**
/// （守卫明令那些词只许出现在适配层里），而"模型那一侧报的读数"本来就是这一族的中立主语。
pub const MODEL_READINGS_FILE: &str = "model_readings.ndjson";

/// 打不开 / 写不进。**两类，各自稳定分类**。
///
/// ⚠ 它不是**持久字段上的**词表：没有任何读者把它从盘上读回程序（文件里根本不写故障），
/// 因此本枚举**没有解码端**——不预建一个没有消费者的封闭表。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelReadingsError {
    /// 打不开（目录不在、权限、被占用）。**在第一次要写的时候才知道**（见模块文档）。
    Open(io::ErrorKind),
    /// 写不进（盘满、句柄失效）。
    Append(io::ErrorKind),
}

impl ModelReadingsError {
    /// 稳定分类标记（同一故障两次发生 ⇒ 同一个词）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open(_) => "open",
            Self::Append(_) => "append",
        }
    }

    /// 那一类下面的具体形状（**结构化的**，不是一段动态文本）。
    pub fn kind(&self) -> io::ErrorKind {
        match self {
            Self::Open(kind) | Self::Append(kind) => *kind,
        }
    }
}

impl fmt::Display for ModelReadingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({:?})", self.as_str(), self.kind())
    }
}

/// 一次模型响应报告的读数（**搬运**：不做加法、不做换算、不推断）。
///
/// 字段名是**中立名**：两条线协议各自把自家的字段映射到这一组（映射在适配层里，那里才是
/// 认识自家协议的地方）。四个计数各自可选——**缺的那一格如实缺着**，"没报"与"报了零"是两件事。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnReadings {
    /// 这一次响应属于哪一段清醒期。
    pub run: RunId,
    /// 这一次响应属于那一段的第几轮。
    pub turn: Turn,
    /// 走的是哪一条线协议（适配层自己的公开标识）。
    pub wire: &'static str,
    /// 这一次送进去的上下文量。
    pub prompt_tokens: Option<u64>,
    /// 这一次生成量。
    pub completion_tokens: Option<u64>,
    /// 这一次的总量。
    pub total_tokens: Option<u64>,
    /// 送进去的那一段里**命中缓存**的那部分（口径见适配层的映射函数）。
    pub cached_prompt_tokens: Option<u64>,
}

/// 盘上那一行的形状。**只在这里**：`TurnReadings` 是中立的载荷，落盘格式是本件自己的事。
///
/// 编码走 `serde_json` 的**值**与**映射**，不引入派生宏——与两份追加存储的编码同款
/// （那份格式也是手写字段，理由相同：字段的**缺席**必须是我们自己决定的，不能由派生规则代劳）。
fn encode(readings: &TurnReadings) -> Option<String> {
    let mut row = serde_json::Map::new();
    row.insert(
        "run".to_string(),
        serde_json::Value::from(readings.run.as_str()),
    );
    row.insert(
        "turn".to_string(),
        serde_json::Value::from(readings.turn.number()),
    );
    row.insert("wire".to_string(), serde_json::Value::from(readings.wire));
    // **没报的字段不插进去**：缺席、`null`、`0` 是三件事（见模块文档）。
    for (name, value) in [
        ("prompt_tokens", readings.prompt_tokens),
        ("completion_tokens", readings.completion_tokens),
        ("total_tokens", readings.total_tokens),
        ("cached_prompt_tokens", readings.cached_prompt_tokens),
    ] {
        if let Some(value) = value {
            row.insert(name.to_string(), serde_json::Value::from(value));
        }
    }
    serde_json::to_string(&serde_json::Value::Object(row)).ok()
}

/// 写柄与读侧共享的那一份状态。
#[derive(Debug)]
struct LogState {
    /// 账落在哪儿。**构造时只记路径**，不碰文件系统（见模块文档）。
    path: PathBuf,
    /// 追加句柄：**第一次真的要写一行时才开**。`None` = 这一份账今天还没被写过。
    file: Option<File>,
    /// **第一次**失败。只留第一条：后面的失败多半是同一个原因。
    first_failure: Option<ModelReadingsError>,
}

/// 逐轮读数的**读**侧：装配入口持有它，用来在收尾时报出"这份账坏过没有"。
///
/// 它**不给**任何执行路径用：这里没有"读回某一行"的入口——回读是**操作者翻文件**的事，
/// 不是程序的事。程序只需要知道它是不是好的。
#[derive(Clone, Debug)]
pub struct ModelReadings {
    state: Arc<Mutex<LogState>>,
}

impl ModelReadings {
    /// 建一份逐轮读数账，同时交出**唯一写柄**与读侧。
    ///
    /// ⚠ **它不碰文件系统、也没有失败分支**：文件在**第一次真的记一行**时才出现。这不是省事——
    /// "没有凭据的时刻不得改动环境"是一条写死的纪律，装配期先建一个空文件正好违反它。
    pub fn channel(root: &Path) -> (ModelReadingsWriter, ModelReadings) {
        let state = Arc::new(Mutex::new(LogState {
            path: root.join(MODEL_READINGS_FILE),
            file: None,
            first_failure: None,
        }));
        (
            ModelReadingsWriter {
                state: Arc::clone(&state),
            },
            ModelReadings { state },
        )
    }

    /// 这份账落在哪个文件上（给报故障那一处说清"是哪一份"）。
    ///
    /// ⚠ 它是**打算落在哪儿**，不等于"它已经在那儿"——今天还没写过就还没有这个文件。
    pub fn path(&self) -> PathBuf {
        self.state
            .lock()
            .expect("逐轮读数账的锁只被短临界区持有")
            .path
            .clone()
    }

    /// **第一次**失败；一次都没坏过时是 `None`。
    ///
    /// `None` 只说明"没有记到失败"，**不等于**"每一轮都写成功了"——一份从来没被写过的账
    /// 也读作 `None`。要判"写没写"，看文件本身。
    pub fn first_failure(&self) -> Option<ModelReadingsError> {
        self.state
            .lock()
            .expect("逐轮读数账的锁只被短临界区持有")
            .first_failure
    }
}

/// 逐轮读数的**写**侧：只有适配层持有它。
///
/// 它只做一件事——把这一次响应报告的读数**原样**追加一行，不换算、不累计、不推断。
/// 没有它的那一侧（组合层、入口、显示区）拿不到写入口，因此"唯一写者"不是约定而是形状
/// （机械守卫见 `tests/minimal_agent.rs::only_the_adapter_writes_the_readings_log`）。
#[derive(Clone, Debug)]
pub struct ModelReadingsWriter {
    state: Arc<Mutex<LogState>>,
}

impl ModelReadingsWriter {
    /// 记下一次响应报告的读数：**一行一次**，追加。
    ///
    /// 写失败**不返回给调用方**：那会把一次本来正常的响应变成失败（遥测不是契约）。
    /// 失败被记在状态里，由读侧在收尾时报出来——**不静默**，但也不打断这一轮。
    pub fn record(&self, readings: &TurnReadings) {
        let mut state = self.state.lock().expect("逐轮读数账的锁只被短临界区持有");
        // 已经坏过就不再重复尝试：原因多半一样，而重复只会让每一次调用都多付一次失败的代价。
        if state.first_failure.is_some() {
            return;
        }
        let mut encoded = match encode(readings) {
            Some(encoded) => encoded,
            // 三个标识与四个计数都编码不了，现实里不存在；但"写不进"必须有一条出路，
            // 不许 panic 在调用链上。
            None => {
                state.first_failure = Some(ModelReadingsError::Append(io::ErrorKind::InvalidData));
                return;
            }
        };
        encoded.push('\n');
        // **第一次真的要写，才开这一份账**（在那之前盘上什么都没有）。
        if state.file.is_none() {
            match OpenOptions::new()
                .create(true)
                .append(true)
                .open(&state.path)
            {
                Ok(file) => state.file = Some(file),
                Err(error) => {
                    state.first_failure = Some(ModelReadingsError::Open(error.kind()));
                    return;
                }
            }
        }
        // **只写，不 `sync_all`**：这一档按"可以丢失"走（见模块文档）。
        let file = state
            .file
            .as_mut()
            .expect("上一步保证句柄在场（要么开好了，要么已经记下失败并返回）");
        if let Err(error) = file.write_all(encoded.as_bytes()) {
            state.first_failure = Some(ModelReadingsError::Append(error.kind()));
        }
    }
}
