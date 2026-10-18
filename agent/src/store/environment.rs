//! 身份与准入（2026-10-08 从 `store.rs` 拆出；**盘上格式一个字节没动**）。
//!
//! 这一段是**"环境"这个对象本身**：
//!
//! - **这是谁的环境**：身份记录的读写与归属校验（不静默新建、不重置数据）；
//! - **现在谁在写**：操作系统级排他写资格；
//! - **准入与健康**：`Admission` / `EnvironmentHealth` / 跨存储矛盾如实报告 / 开场读数一次解析；
//! - **事实视图** `FactView`（操作事实的只读投影）。
//!
//! **公开面原样留在 `crate::store::` 下**（父模块 `pub use environment::*`），消费方一处未改。

use super::*;

/// 环境里读取到的事实序列：操作事实的只读视图。
///
/// 它**不自动重放**任何旧 Run：读到的事实只用于如实报告状态。要不要按 Run 过滤已有决定权
/// 属于 `foundation::recover`，本模块不自行改它的定义。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FactView {
    rows: Vec<FactRow>,
}

impl FactView {
    /// 按追加顺序读回的事实行。
    pub fn rows(&self) -> &[FactRow] {
        &self.rows
    }

    /// 事实条数。
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// 是否没有任何事实。
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// 这次能不能在一个环境里开始执行。
///
/// 它**不是**"恢复成功"的布尔值，而是三个必须分开说的处境。判定只依据**已确认的耐久记录**，
/// 因此重开进程不能靠"进程是新的"绕过它，也不需要任何新的持久格式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    /// 没有留下任何未收尾的痕迹：可以开始新活动。
    Ready,
    /// 上一次因为保存失败而暂停，且没有被显式清除。
    ///
    /// 这是"**已经记录在案的**存储故障"：原因文本来自那条暂停记录。
    ///
    /// ⚠ **原因必定在场**（`String` 而不是 `Option`，2026-10-06 收）：暂停记录**总是**带着
    /// 它停下来的原因，而"没有原因、又没有被确认"那个组合在解码端被拒绝（见 `decode_part`）。
    /// 因此这里不需要 `Option`，渲染面也不必为它留一句空话。
    Paused {
        /// 记录下来的暂停原因（**必定在场**，见上）。
        reason: String,
    },
    /// 环境里存在**已登记开始、却没有可靠结束事实**的活动。
    ///
    /// 它**不**等于"发生了存储故障"：进程崩溃、被强杀，或者"暂停标记自己也写失败"都会留下
    /// 这个形状。因此这里只如实说"上一次活动没有收尾、原因没有被记录下来"，
    /// 不冒充存储故障结论。要开始新活动，必须由调用方**显式**确认（见 [`clear_paused`]）。
    Unfinished {
        /// 上一次没有收尾的活动。
        run: RunId,
    },
}

impl Admission {
    /// 是否允许开始新活动。
    pub fn is_ready(&self) -> bool {
        matches!(self, Admission::Ready)
    }

    /// 稳定分类标记。
    pub fn as_str(&self) -> &'static str {
        match self {
            Admission::Ready => "ready",
            Admission::Paused { .. } => "paused",
            Admission::Unfinished { .. } => "unfinished",
        }
    }
}

/// 一份存储与另一份存储之间**可判定**的矛盾。
///
/// 它不是损坏：两份存储各自都自洽，只是"同一个活动在两边的进度不一致"。这正是一次提交
/// 分几步写、只写成一部分时留下的形状，因此必须报告，而**不能**用相同 `RunId` 冒充事务一致。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossStoreIssue {
    run: RunId,
    kind: &'static str,
}

impl CrossStoreIssue {
    /// 涉及的活动。
    pub fn run(&self) -> &RunId {
        &self.run
    }

    /// 矛盾的机械描述。两个方向都可判定：
    ///
    /// - `start-without-fact`：经历里有"活动开始"，事实里没有对应事实；
    /// - `ending-without-fact`：经历里有"结束"，事实里没有对应事实；
    /// - `fact-without-start`：事实里有"活动开始"，经历里没有对应锚点；
    /// - `fact-without-ending`：事实里有"结束"，经历里没有对应锚点。
    ///
    /// 前两个方向正是"一次提交分几步写、只写成前几步"留下的形状；后两个方向本写入者
    /// 写不出来（经历先写、事实后写），所以它意味着文件被外部改动过——同样是可判定的事实，
    /// 因此同样报告，而不是当作"没有发现问题"。
    pub fn kind(&self) -> &'static str {
        self.kind
    }
}

/// 环境打开之后的只读自检结果：身份是否还在、历史前缀完整到什么程度、操作结果状态。
///
/// 这三件事**分别**展示，不压成"恢复成功"一个布尔值。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvironmentHealth {
    agent: AgentId,
    runs: Vec<RunSummary>,
    /// 经历存储的已确认前缀长度（条数）。
    trace_records: usize,
    /// 事实存储的已确认前缀长度（条数）。
    fact_records: usize,
    /// 上一次是否处于"保存失败暂停"状态。`true` 时不自动恢复工作。
    paused: bool,
    /// 两份存储之间可判定的矛盾（各自自洽，只是进度不一致）。
    cross_store: Vec<CrossStoreIssue>,
}

impl EnvironmentHealth {
    /// 这个环境的身份。
    pub fn agent(&self) -> &AgentId {
        &self.agent
    }

    /// 全部已知活动的摘要，按首次出现顺序。
    pub fn runs(&self) -> &[RunSummary] {
        &self.runs
    }

    /// 经历存储的已确认前缀条数。
    pub fn trace_records(&self) -> usize {
        self.trace_records
    }

    /// 事实存储的已确认前缀条数。
    pub fn fact_records(&self) -> usize {
        self.fact_records
    }

    /// 上一次是否留下了"保存失败暂停"标记。
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// 两份存储之间可判定的矛盾。空表示"没有发现"——它**不等于**"已经证明一致"。
    pub fn cross_store(&self) -> &[CrossStoreIssue] {
        &self.cross_store
    }

    /// 有没有留下未收尾的活动（有开始、没有可靠结束事实）。
    pub fn interrupted(&self) -> Vec<&RunSummary> {
        self.runs
            .iter()
            .filter(|run| run.status() == "interrupted")
            .collect()
    }
}

/// 一个已被打开、且当前由**本对象**独占写入的身份环境。
///
/// 它持有操作系统级锁：只有它还活着，这个环境目录才由本写者占用。`Drop` 时锁自动释放，
/// 不需要"清理任务"，也不依赖任何易被崩溃遗留物欺骗的存在性检查。
pub struct Environment {
    root: PathBuf,
    agent: AgentId,
    trace: File,
    facts: File,
    next_seq: u64,
    fact_seq: u64,
    /// 经历存储当前的**链值**（下一条记录的校验和从它续起，见 [`chain_checksum`]）。
    ///
    /// 它只在**写成功并确认耐久**之后前进；打开时由已确认前缀重新算出来（读的时候本来就要
    /// 逐条过一遍，因此这一步不多花一次读）。
    trace_chain: u32,
    /// 事实存储当前的链值（事实侧有自己的一条链，与经历那条互不影响）。
    facts_chain: u32,
    /// **封口的经历段**，按封口顺序（目录的内容）。
    ///
    /// 它们不可变、永不再写；开机不重读它们（目录里有每一卷的链值与活动摘要，
    /// 还有折到那一卷末尾的准入读数）。要重读全部旧卷只有两条路：显式全量读
    /// （[`read_trace_at`] / [`EnvironmentReader::trace`]），或目录与盘对不上时的重建。
    parts: Vec<SealedPart>,
    /// **封口的事实段**，按封口顺序（事实目录的内容）。
    ///
    /// 与 [`Environment::parts`] 同一判据：不可变、永不再写，开机不重读（目录里有每一卷的链值
    /// 与条数）。要重读全部旧卷只有两条路：显式全量读（[`read_facts`] 那条路，健康自检与诊断
    /// 走它），或目录与盘对不上时的重建。
    fact_parts: Vec<SealedFactPart>,
    trace_records: usize,
    fact_records: usize,
    /// 已经登记过「活动开始」的活动身份。
    ///
    /// 它让"同一个 `RunId` 只能登记一次"成为**写入前**的机械判断：重试保存不会重复插入，
    /// 新进程也不可能把一次旧活动悄悄重开成两份经历。
    known_runs: std::collections::BTreeSet<RunId>,
    /// 已经**封存**的活动身份。
    ///
    /// 封存是"到此为止"：封存之后再往这个活动追加经历必须被拒绝。新活动不受影响。
    sealed_runs: std::collections::BTreeSet<RunId>,
    /// 经历存储的追加句柄是否还停在一个**已确认的行边界**上。
    ///
    /// 一次写入或同步失败之后它变成 `false`：这时写者不知道自己停在哪一半，
    /// 因此**不再追加**——继续写只会把"写了一半"滚成更大的损坏，也会让后续写入找错位置。
    trace_tail_ok: bool,
    /// 事实存储的追加句柄是否还停在一个**已确认的行边界**上（同上）。
    facts_tail_ok: bool,
    /// 独占写资格的锁句柄。只为了持锁而存在；丢掉它就等于放弃写资格。
    _lock: File,
    /// **写这一行时的钟**（可选）。
    ///
    /// `Some` ⇒ 每一条记录都带 `at-ms`（unix 毫秒）；`None` ⇒ 那一格整个不出现
    /// （旧记录、离线夹具都是这一档）。装它的口是 [`Environment::with_clock`]。
    ///
    /// 它**不是**"环境的属性"，而是"**写这一行时**"的属性——因此它只在写侧用得到，
    /// 读侧读到的是**盘上那一格**（[`TraceRecord::at_millis`] / [`FactRow::at_millis`]）。
    clock: Option<ClockHandle>,
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("root", &self.root)
            .field("agent", &self.agent)
            .field("next_seq", &self.next_seq)
            .field("fact_seq", &self.fact_seq)
            .field("trace_records", &self.trace_records)
            .field("fact_records", &self.fact_records)
            .field("known_runs", &self.known_runs.len())
            .field("sealed_runs", &self.sealed_runs.len())
            .finish_non_exhaustive()
    }
}

impl Environment {
    /// **创建一个新环境**：这是唯一会写入身份记录的入口。
    ///
    /// 三条拒绝（一条都不覆盖、不重置、不销毁）：
    ///
    /// - 目标不是目录 ⇒ [`StoreError::WrongKind`]；
    /// - 目录已存在且**已经带有身份记录** ⇒ [`StoreError::AlreadyOpen`]（要打开已有环境请用
    ///   [`Environment::open`]）；
    /// - 身份不在、而**历史在**（两份存储已经存在）⇒ [`StoreError::HistoryWithoutIdentity`]：
    ///   那不是一个"还没有身份的空位置"，而是一个**丢了身份的既有环境**，按"创建"往下走会把
    ///   两份历史清空——同一个损坏形状，`open` 是拒绝、`create` 却是销毁。**看不懂的位置不动它。**
    ///
    /// 判据只覆盖**这一次操作自己的写集**（两份存储）：家侧的 `agent.conf`、主体的 `notes/` 等
    /// 既有内容不在判据里——`create` 不会动它们，而家侧按设计就把 `agent.conf` 写在这个目录里。
    ///
    /// **排他保护先于任何身份检查与写入**：先拿到排他写资格，再看身份在不在、才决定写不写。
    /// 反过来（先看存在再写、最后才加锁）会留下一个并发窗口：两个创建者可能都通过"身份不存在"
    /// 的检查，然后互相覆盖身份与存储。
    pub fn create(root: &Path, agent: AgentId) -> Result<Self, StoreError> {
        if root.exists() {
            let metadata = fs::symlink_metadata(root).map_err(io_error)?;
            if !metadata.is_dir() {
                return Err(StoreError::WrongKind);
            }
        } else {
            fs::create_dir_all(root).map_err(io_error)?;
        }

        // 先取排他写资格：这一步之后，同一个目录不可能再出现第二个创建者或写者。
        let lock = acquire_writer_lock(root)?;

        if root.join(IDENTITY_FILE).exists() {
            // 已经有身份：这是"打开"，不是"创建"。不静默采用，也不重置。
            return Err(StoreError::AlreadyOpen);
        }

        // ⚠ **2026-10-06 改（防静默销毁）**：身份不在，但这个位置里**已经有历史** ⇒ 拒绝。
        //
        // 为什么必须有这一步：身份记录丢了（被误删、被半途拷走、写了一半就断电）而两份存储还在
        // 时，下面那两行 `truncate(true)` 会把历史**清空**。判据与 `open` 对称——**看不懂的位置
        // 不动它**。
        //
        // 判据**只覆盖这一次操作自己的写集**：两份**活动段**，以及两个**封口目录**——封口段也是
        // 历史（旧卷），把它们留在原地再铸一个新身份，会造出一个"新身份 ＋ 旧历史"的环境。
        // 别的既有内容不在里面，因为 `create` 不会动它们，而家侧按设计就把 `agent.conf` 写在这个
        // 目录里（真机门禁 2026-10-06 抓到过一次"按'任何东西在场就拒绝'写"——那会让每一个新家都
        // 装不起来）。
        let mut history: Vec<&'static str> = [TRACE_FILE, FACTS_FILE]
            .into_iter()
            .filter(|name| root.join(name).exists())
            .collect();
        for name in [PARTS_DIR, FACTS_PARTS_DIR] {
            let occupied =
                fs::read_dir(root.join(name)).is_ok_and(|mut entries| entries.next().is_some());
            if occupied {
                history.push(name);
            }
        }
        if !history.is_empty() {
            return Err(StoreError::HistoryWithoutIdentity { files: history });
        }

        // 身份记录先写、先同步：它是"这个环境属于谁"的唯一权威位置。
        let identity = format!("{{\"agent\":\"{}\"}}\n", escape_json(agent.as_str()));
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(root.join(IDENTITY_FILE))
            .map_err(io_error)?;
        file.write_all(identity.as_bytes()).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        drop(file);

        // 两份空存储先建立并同步，之后只追加：这样后续追加不需要再依赖目录项耐久。
        for name in [TRACE_FILE, FACTS_FILE] {
            let file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(root.join(name))
                .map_err(io_error)?;
            file.sync_all().map_err(io_error)?;
        }
        sync_directory(root);

        Self::bind(root, agent, lock)
    }

    /// **打开一个已有环境**：校验身份归属，取得排他写资格。
    ///
    /// 身份记录缺失、损坏或属于别的 Agent 时明确拒绝：不静默生成新身份、不重置数据。
    /// 已经有写者时直接拒绝。
    pub fn open(root: &Path, agent: AgentId) -> Result<Self, StoreError> {
        let metadata = fs::symlink_metadata(root).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => StoreError::NotFound,
            _ => StoreError::Io,
        })?;
        if !metadata.is_dir() {
            return Err(StoreError::WrongKind);
        }

        // 排他写资格先于读取：不先加锁就读，会把"另一个写者正在改"的内容当成稳定事实。
        let lock = acquire_writer_lock(root)?;
        Self::bind(root, agent, lock)
    }

    /// 在**已经持有排他写资格**的前提下，校验归属并把环境的读写句柄接起来。
    fn bind(root: &Path, agent: AgentId, lock: File) -> Result<Self, StoreError> {
        // 归属校验：读身份记录并比对，而不是相信调用者给了同一个字符串。
        // 判据与读者那一条**同一份实现**（见 `verify_identity_at`）。
        verify_identity_at(root, &agent)?;

        // 两份存储都必须**存在**且**完整可解码**：身份在，说明这是已建立的环境；
        // 缺文件是损坏，不能按空历史重建，也不能只数行数而不校验内容。
        // 分卷之后：**开机只读活动段 ＋ 目录**，不重读旧卷（见 [`load_parts`]）。
        let parts = load_parts(root)?;
        let chain_start = parts.last().map(|part| part.chain).unwrap_or(CHAIN_SEED);
        let (records, trace_chain) = read_trace_file(
            &root.join(TRACE_FILE),
            TailPolicy::RequireConfirmed,
            chain_start,
        )?;
        // 事实侧同一判据（2026-10-06 分卷之后）：**只读事实目录 ＋ 事实活动段**。下一序号、链值、
        // 条数三格都在目录里（每卷记着条数），因此旧卷不必重读——这一格从前是开机最大的一项
        // （实测 1200 段活动上占 85%）。
        let fact_parts = load_fact_parts(root)?;
        let fact_chain_start = fact_parts
            .last()
            .map(|part| part.chain)
            .unwrap_or(CHAIN_SEED);
        let (fact_rows, facts_chain) = read_fact_file(
            &root.join(FACTS_FILE),
            TailPolicy::RequireConfirmed,
            fact_chain_start,
        )?;

        // 下一个序号由**最后一条已确认记录的序号**决定，不是条数：这样即使前缀不是从 0 开始，
        // 也不会写出与既有记录冲突的序号。活动段空着时（刚封口），由最后一卷的末序号续起。
        let next_seq = records
            .last()
            .map(|record| record.seq() + 1)
            .or_else(|| parts.last().map(|part| part.seq_end + 1))
            .unwrap_or(0);
        let fact_seq = fact_rows
            .last()
            .map(|row| row.seq() + 1)
            .or_else(|| fact_parts.last().map(|part| part.seq_end + 1))
            .unwrap_or(0);
        let sealed_records: usize = parts.iter().map(|part| part.records).sum();
        let trace_records = sealed_records + records.len();
        // 已确认条数：**封口段那一半从目录里取**（每卷记着条数），活动段数一数。
        let sealed_fact_records: usize = fact_parts.iter().map(|part| part.records).sum();
        let fact_records = sealed_fact_records + fact_rows.len();

        // 已登记与已封存的活动：**旧卷从目录里折好的那两格取**（`runs_folded`：身份 ＋ 两个标志），
        // 活动段从记录里取。⚠ 这一格从前要从每卷的**完整摘要**里算——那是 O(全部活动) 的一串
        // 八字段对象，正是"开机那一拍随历史变慢"的最后一处。
        let known_runs: std::collections::BTreeSet<RunId> = parts
            .iter()
            .flat_map(|part| part.runs_folded.iter())
            .filter(|(_, flags)| flags & RUN_STARTED != 0)
            .map(|(run, _)| run.clone())
            .chain(
                records
                    .iter()
                    .filter(|record| record.entry().is_started())
                    .map(|record| record.run().clone()),
            )
            .collect();
        let sealed_runs: std::collections::BTreeSet<RunId> = parts
            .iter()
            .flat_map(|part| part.runs_folded.iter())
            .filter(|(_, flags)| flags & RUN_SEALED != 0)
            .map(|(run, _)| run.clone())
            .chain(
                records
                    .iter()
                    .filter(|record| record.entry().is_sealed())
                    .map(|record| record.run().clone()),
            )
            .collect();

        // 追加句柄：**不** create。文件不存在是上面已经拒绝过的损坏，不是"顺手建一份空的"。
        let trace = open_append(root, TRACE_FILE)?;
        let facts = open_append(root, FACTS_FILE)?;

        Ok(Self {
            root: root.to_path_buf(),
            agent,
            trace,
            facts,
            next_seq,
            fact_seq,
            trace_chain,
            facts_chain,
            trace_records,
            fact_records,
            known_runs,
            sealed_runs,
            parts,
            fact_parts,
            trace_tail_ok: true,
            facts_tail_ok: true,
            clock: None,
            _lock: lock,
        })
    }

    /// **给这个环境装上一个钟**：此后每一条记录都带上「它是什么时候写的」（`at-ms`）。
    ///
    /// ⚠ **默认没有钟**（构造出来就是 `None`）：直接走 [`Environment::append_trace`] 的那些路径
    /// （离线夹具、诊断、旧入口）因此**一个字节都不变**——那一大批"形状逐字节不变"的判据
    /// 一个字都不用改。
    ///
    /// **而每一次会话都会装上它**：[`crate::composition::RunSession::open_declared`] 在装配那一刻
    /// 把 [`crate::composition::SessionRequest::clock`] 交给环境。⇒ 生产读真时刻，夹具读固定钟
    /// （因此那些用例的 `at-ms` 是**确定值**，可以照常断言）。**没有"忘了装"这一档**。
    pub fn set_clock(&mut self, clock: ClockHandle) {
        self.clock = Some(clock);
    }

    /// **现在是什么时候**（unix 毫秒）；没有钟、或钟读不出来 ⇒ `None`（那一格不写）。
    ///
    /// ⚠ 钟读不出来（早于 1970）**不拒绝写这一行**：宁可这一条"没有时刻"，也不要因为时钟异常
    /// 丢掉一整条经历。这与"缺字段 = 这一格没有被记，**不编造**"是同一条规矩。
    fn at_millis(&self) -> Option<u64> {
        self.clock
            .as_ref()
            .and_then(|clock| unix_millis(clock.now()))
    }

    /// 这个环境的身份。
    pub fn agent(&self) -> &AgentId {
        &self.agent
    }

    /// 环境根目录。只用于诊断，不进任何 Model 可见内容。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 现在能不能在这个环境里开始一次新活动。
    ///
    /// 判定只读**已确认的耐久记录**，因此重开进程不能绕过它：进程是新的，但记录还是那些。
    ///
    /// 顺序是有意的：先看"有没有记录在案的保存失败暂停"，再看"有没有没收尾的活动"。
    /// 两者如实分开：前者是已知原因的存储故障，后者只说明上一次没有收尾、原因没有被记录。
    pub fn admission(&self) -> Result<Admission, StoreError> {
        // 写者的读法：**要求文件自洽**（最后一段没换行 ⇒ 报损坏，见 `TailPolicy`）。
        admission_at(&self.root, TailPolicy::RequireConfirmed)
    }

    /// 开场那一次读：准入 ＋ 历史上最后一次存储故障（**一次解析**，见 [`OpeningReadings`]）。
    pub fn opening_readings(&self) -> Result<OpeningReadings, StoreError> {
        opening_readings_at(&self.root, TailPolicy::RequireConfirmed)
    }

    /// 两份存储之间**可判定**的矛盾。
    ///
    /// 各自都自洽、只是进度不一致：这正是"一次提交分几步写、只写成了前几步"留下的形状。
    /// 因此这里**报告**而不是判损坏：它能被读到，就必须能被如实说清。
    /// 空结果只表示"没有发现"，不代表已经证明一致。
    pub fn cross_store_issues(&self) -> Result<Vec<CrossStoreIssue>, StoreError> {
        cross_store_issues_at(&self.root, TailPolicy::RequireConfirmed)
    }

    /// **这一段生命还能不能开始** —— `known_runs` 那一节说的"**写入前**的机械判断"就是它。
    ///
    /// ⚠ **为什么要有这个读（2026-09-26）**：那两张表的用途是"同一个 `RunId` 只能登记一次"，
    /// 但那条判断此前**只发生在写入那一刻** —— 而写入序列里排在最前面的是**装订留证**
    /// （`commit_static_head`：会话装配层发起的一条**见证**，代码自己写着"它不在 Core 的提交序列里"
    /// "只作见证，不进视野"）。于是重复的活动标识会**先撞上 `RunSealed`**、报成
    /// `stage=static-head`，而 store 里那个**专门为这件事准备**的 `RunAlreadyKnown`
    /// （`append_trace` 里 `is_start` 那一支）**永远到不了**。
    ///
    /// ⇒ 判决点落在"一份见证写不写得下去"上，而不是"这段生命能不能开始"上。那是反的：
    /// **见证是记录，不该有否决权。** 有了这个读，装配层可以在留证**之前**先问一句，
    /// 把判决点还给活动身份本身（那一问发生在任何模型调用之前，"写不下来就不驱动"一字未改）。
    ///
    /// 两张表都要看：重复登记看 `known_runs`，而"跑过并被封存"的那一段只落在 `sealed_runs`
    /// 里（封存不回头改登记表）。**只查一张会漏掉最常发生的那种重复。**
    pub fn run_is_taken(&self, run: &RunId) -> bool {
        self.known_runs.contains(run) || self.sealed_runs.contains(run)
    }

    /// 追加一条经历，返回它拿到的稳定序号。
    ///
    /// 追加**不覆盖**任何已确认前缀：序号只能前进。
    ///
    /// 两个**写入前**的判断：
    ///
    /// - "活动开始"只能登记一次：重试保存不会重复插入，新进程也不可能把一次旧活动重开成两份；
    /// - 已**封存**的活动不再接受追加：封存就是"到此为止"，之后的追加一律拒绝（新活动不受影响）。
    pub fn append_trace(&mut self, run: &RunId, payload: TracePayload) -> Result<u64, StoreError> {
        // 上一次追加没有停在已确认的行边界上：不再往同一个文件里写，避免把损坏滚大。
        if !self.trace_tail_ok {
            return Err(StoreError::TailUnconfirmed);
        }
        // 暂停标记是**环境级**记录：它可能在活动已经封存之后仍然需要被写下（保护的记录
        // 不该被活动级的封存挡住）。其余经历一律遵守"封存即到此为止"。
        let is_environment_marker = matches!(
            &payload,
            TracePayload::Injection { stage, .. } if stage == PAUSED_STAGE
        );
        if self.sealed_runs.contains(run) && !is_environment_marker {
            return Err(StoreError::RunSealed { run: run.clone() });
        }
        let is_start = payload.is_started();
        let is_sealed = payload.is_sealed();
        if is_start && self.known_runs.contains(run) {
            return Err(StoreError::RunAlreadyKnown { run: run.clone() });
        }
        let seq = self.next_seq;
        let (line, chain) = encode_trace(seq, run, &payload, self.trace_chain, self.at_millis())?;
        if let Err(error) = self.trace.write_all(line.as_bytes()) {
            self.trace_tail_ok = false;
            return Err(io_error(error));
        }
        if let Err(error) = self.trace.sync_all() {
            self.trace_tail_ok = false;
            return Err(io_error(error));
        }
        // 只有**确认耐久**之后才推进内存状态：写失败不得留下"已登记/已封存"的占用痕迹，
        // 也不得让链前进（链一旦前进，下一条的校验和就会把没写下去的那条算进去）。
        self.trace_chain = chain;
        // 写满就封口（另起一段）。封口本身失败**不算这次追加失败**：那条记录已经在盘上、
        // 已经被确认——它只是还没被封进某一卷（下次开机由目录重建路径补上）。
        if let Ok(metadata) = self.trace.metadata()
            && metadata.len() >= SEGMENT_ROLL_BYTES
            && let Err(error) = self.seal_active()
        {
            return Err(error);
        }
        if is_start {
            self.known_runs.insert(run.clone());
        }
        if is_sealed {
            self.sealed_runs.insert(run.clone());
        }
        self.next_seq = seq + 1;
        self.trace_records += 1;
        Ok(seq)
    }

    /// 活动段写满了：**封口**（搬进 `trace.parts/`，写一行目录 ＋ 一份卷内偏移索引），另起一段。
    ///
    /// 顺序是**先搬、后写索引**：崩在中间 ⇒ 卷在盘上、索引（或目录）少一份 ⇒ 下次**读它**时由
    /// 重建路径补上（**卷是权威，索引只是索引**）。反过来（先写索引再搬）会造出"索引说有、
    /// 盘上没有"，那一条只能靠猜。
    ///
    /// 封口时读一遍活动段（很稀有：每 1 MiB 一次），把那几格**折**进目录：链值、活动摘要、
    /// 以及折到这一卷末尾的准入读数（"最后一次存储故障"与"没有收尾的活动"）。开机只读目录
    /// ＋活动段就能答出"能不能开跑"，靠的就是这一折。**同一趟读**顺手逐行累加行长，
    /// 得到每段活动的字节区间与区间起点的链值 ⇒ 写成 `<卷名>.idx`（卷内偏移索引那一刀）。
    fn seal_active(&mut self) -> Result<(), StoreError> {
        let active = self.root.join(TRACE_FILE);
        let chain_start = self
            .parts
            .last()
            .map(|part| part.chain)
            .unwrap_or(CHAIN_SEED);
        let (records, chain, spans) =
            read_trace_file_detailed(&active, TailPolicy::RequireConfirmed, chain_start)?;
        let bytes = fs::metadata(&active).map(|meta| meta.len()).unwrap_or(0);
        let seq_start = records
            .first()
            .map(|record| record.seq())
            .unwrap_or(self.next_seq);
        let seq_end = records
            .last()
            .map(|record| record.seq())
            .unwrap_or(seq_start);
        let previous = self.parts.last();
        let acknowledged_through = last_acknowledged_seq(&records)
            .or_else(|| previous.and_then(|part| part.acknowledged_through));
        let paused = last_storage_failure_of(&records)?
            .or_else(|| previous.and_then(|part| part.paused.clone()));
        let unfinished = unfinished_after(
            &records,
            acknowledged_through,
            previous.and_then(|part| part.unfinished.clone()),
        );
        let runs = summaries_of(&records)?;
        let part = SealedPart {
            file: part_file_name(seq_start),
            seq_start,
            seq_end,
            bytes,
            chain,
            runs_folded: folded_of(&runs),
            records: records.len(),
            runs,
            paused,
            unfinished,
            acknowledged_through,
        };
        fs::create_dir_all(self.root.join(PARTS_DIR)).map_err(io_error)?;
        // 搬（同盘 rename，原子）：这一卷从此**永不再写**。
        fs::rename(&active, part.path(&self.root)).map_err(io_error)?;
        sync_directory(&self.root.join(PARTS_DIR));
        // 卷内偏移索引：**顺手**写下来（区间就在上面那趟读里算好了，不另读一遍）。
        write_part_index(&self.root, &part, &spans)?;
        let mut index: Vec<SealedPart> = self.parts.clone();
        index.push(part);
        write_parts_index(&self.root, &index)?;
        self.parts = index;
        // 新的活动段：这里**必须建**（刚把旧的搬走了）——与首次打开那条"缺文件即损坏、不 create"
        // 是两回事。
        self.trace = open_append_new(&self.root, TRACE_FILE)?;
        self.trace_chain = chain;
        Ok(())
    }

    /// 追加一条操作事实。
    ///
    /// 事实里**只有操作与结果身份**，没有正文：正文归经历存储，两者不互相冒充。
    ///
    /// 写满 [`FACTS_ROLL_BYTES`] 就**封口**（另起一段）——与经历那一刀同形。封口本身失败**不算
    /// 这次追加失败**：那条记录已经在盘上、已经被确认，它只是还没被封进某一卷（下次开机由目录
    /// 重建路径补上）。
    pub fn append_fact(&mut self, run: &RunId, record: &Record) -> Result<u64, StoreError> {
        if !self.facts_tail_ok {
            return Err(StoreError::TailUnconfirmed);
        }
        let seq = self.fact_seq;
        let (line, chain) = encode_fact(seq, run, record, self.facts_chain, self.at_millis())?;
        if let Err(error) = self.facts.write_all(line.as_bytes()) {
            self.facts_tail_ok = false;
            return Err(io_error(error));
        }
        if let Err(error) = self.facts.sync_all() {
            self.facts_tail_ok = false;
            return Err(io_error(error));
        }
        self.facts_chain = chain;
        if let Ok(metadata) = self.facts.metadata()
            && metadata.len() >= FACTS_ROLL_BYTES
            && let Err(error) = self.seal_active_facts()
        {
            return Err(error);
        }
        self.fact_seq = seq + 1;
        self.fact_records += 1;
        Ok(seq)
    }

    /// 事实的活动段写满了：**封口**（搬进 `facts.parts/`，写一行事实目录），另起一段。
    ///
    /// 顺序与经历那一刀**同一条**：**先搬、后写目录**——崩在中间 ⇒ 卷在盘上、目录少一行 ⇒
    /// 下次开机由重建路径补上（**卷是权威，目录只是索引**）。
    fn seal_active_facts(&mut self) -> Result<(), StoreError> {
        let active = self.root.join(FACTS_FILE);
        let chain_start = self
            .fact_parts
            .last()
            .map(|part| part.chain)
            .unwrap_or(CHAIN_SEED);
        let (rows, chain) = read_fact_file(&active, TailPolicy::RequireConfirmed, chain_start)?;
        let bytes = fs::metadata(&active).map(|meta| meta.len()).unwrap_or(0);
        let seq_start = rows.first().map(|row| row.seq()).unwrap_or(self.fact_seq);
        let seq_end = rows.last().map(|row| row.seq()).unwrap_or(seq_start);
        let part = SealedFactPart {
            file: part_file_name(seq_start),
            seq_start,
            seq_end,
            records: rows.len(),
            bytes,
            chain,
        };
        fs::create_dir_all(self.root.join(FACTS_PARTS_DIR)).map_err(io_error)?;
        // 搬（同盘 rename，原子）：这一卷从此**永不再写**。
        fs::rename(&active, part.path(&self.root)).map_err(io_error)?;
        sync_directory(&self.root.join(FACTS_PARTS_DIR));
        let mut index: Vec<SealedFactPart> = self.fact_parts.clone();
        index.push(part);
        write_facts_index(&self.root, &index)?;
        self.fact_parts = index;
        // 新的活动段：这里**必须建**（刚把旧的搬走了）——与首次打开那条"缺文件即损坏、不 create"
        // 是两回事。
        self.facts = open_append_new(&self.root, FACTS_FILE)?;
        self.facts_chain = chain;
        Ok(())
    }

    /// 已确认的经历前缀条数。
    pub fn trace_records(&self) -> usize {
        self.trace_records
    }

    /// 已确认的事实前缀条数。
    pub fn fact_records(&self) -> usize {
        self.fact_records
    }

    /// 读回全部经历（只读，不改变任何序号或句柄位置）。
    pub fn trace(&self) -> Result<Vec<TraceRecord>, StoreError> {
        read_trace(&self.root, TailPolicy::RequireConfirmed)
    }

    /// 读回全部操作事实（只读行视图）。
    pub fn facts(&self) -> Result<Vec<FactRow>, StoreError> {
        read_facts(&self.root, TailPolicy::RequireConfirmed)
    }

    /// 读回全部操作事实，包装成可整体查看的视图。
    pub fn fact_view(&self) -> Result<FactView, StoreError> {
        Ok(FactView {
            rows: self.facts()?,
        })
    }

    /// 按活动身份读一段经历，**分页**返回，并明确是否还有后续内容。
    ///
    /// 顺序是稳定序号；`from_seq` 是下界（含），`limit` 是这一页最多返回多少条。
    pub fn read_run(
        &self,
        run: &RunId,
        from_seq: Option<u64>,
        limit: usize,
    ) -> Result<TracePage, StoreError> {
        let all = self.trace()?;
        let mut records: Vec<TraceRecord> = all
            .into_iter()
            .filter(|record| record.run() == run)
            .filter(|record| from_seq.is_none_or(|from| record.seq() >= from))
            .collect();
        let has_more = records.len() > limit;
        records.truncate(limit);
        Ok(TracePage { records, has_more })
    }

    /// 按活动身份读一段操作事实。它不重放、不判断，只如实返回。
    pub fn read_run_facts(&self, run: &RunId) -> Result<Vec<FactRow>, StoreError> {
        let rows = self.facts()?;
        Ok(rows
            .into_iter()
            .filter(|row| row.run() == run.as_str())
            .collect())
    }

    /// 环境健康自检：身份、每个活动的前缀完整度、操作结果状态、保存失败暂停、跨存储矛盾
    /// **分别**报告。
    ///
    /// 它**不**自动恢复任何工作：已确认前缀只是能被读到，不代表可以继续写。
    ///
    /// `paused` 读的是**耐久记录**（见 [`paused_reason`]），不是内存字段——否则重启就能绕过保护。
    pub fn health(&self) -> Result<EnvironmentHealth, StoreError> {
        // 写者的读法：**要求文件自洽**（与 [`Environment::admission`] 同一条分界）。
        health_at(&self.root, TailPolicy::RequireConfirmed)
    }

    /// 记录"这个环境上次因为保存失败而暂停"的持久标记。
    ///
    /// 它必须**持久**，否则重启就能绕过保护。标记本身也走同一份只追加存储。
    ///
    /// **它是尽力而为，不是唯一防线**：如果存储已经坏到连这条标记也写不下去，调用方仍然
    /// 立即停止（不继续调用模型、不尝试新动作），而重开时由"环境里有没收尾的活动"兜底
    /// （见 [`Environment::admission`]）。所以"标记写失败"不会变成"保护失效"，
    /// 只是保护的理由从"已记录的故障"变成"未收尾、原因未被记录"。
    pub fn mark_paused(&mut self, run: &RunId, reason: &str) -> Result<u64, StoreError> {
        self.append_trace(
            run,
            TracePayload::Injection {
                stage: PAUSED_STAGE.to_string(),
                turn: None,
                blocks: vec![(PAUSED_LABEL.to_string(), reason.to_string())],
            },
        )
    }

    /// 取一份**只读句柄**（构造零成本：只有根与身份，不取任何锁）。
    ///
    /// 给"已经持有写资格、但要按读者读法读一遍"的调用方用（例如诊断的适配入口）。
    /// 读法固定为 [`TailPolicy::DropTornTail`]：**我们自己就是写者**，此刻写到一半的那一行
    /// 当然不算事实（不需要再探一次锁——那答案已经在我们手里）。
    pub fn reader(&self) -> EnvironmentReader {
        EnvironmentReader {
            root: self.root.clone(),
            agent: self.agent.clone(),
            tail: TailPolicy::DropTornTail,
        }
    }
}

/// 一个环境的**只读句柄**：它**不取写资格**，因此写者正在跑活动时也读得到。
///
/// 为什么要有它（2026-10-06，真机体验那一刀）：[`Environment::open`] 的第一件事是取排他写资格
/// ——那条纪律不能松（"不先加锁就读，会把另一个写者正在改的内容当成稳定事实"），但**只读的
/// 消费者**（`agent_host_status` / `environment_diagnose`）从前走的正是同一条路，于是主体正在跑一次
/// 活动时，状态页拿到的是 `already-open`：一个"看一眼处境"的命令，**在最需要看处境的那一刻
/// 恰好读不到**。
///
/// 形状上它是"**读侧在构造时切开**"：这个类型**没有**任何写方法、也不持有任何追加句柄，
/// 因此"只读"是类型而不是约定；写资格仍然只有一个持有者（写者那一侧一字未改）。
///
/// 读法是**读者读法**（见 [`TailPolicy`] 与 [`reading_tail`]）：**有写者在写**时读**已确认前缀**
/// （写者此刻写到一半的那一行不算事实），**没有写者**时照旧要求文件自洽（无换行尾部 = 崩溃残留
/// = 明确失败，不许报告成一份更短的健康历史）。内容校验与写者完全一样——完整的一行读不出来、
/// 语义不合法、序号不前进，一律拒绝。
#[derive(Clone, Debug)]
pub struct EnvironmentReader {
    root: PathBuf,
    agent: AgentId,
    /// 打开那一刻定下的读法（见 [`reading_tail`]）。
    tail: TailPolicy,
}

impl EnvironmentReader {
    /// **只读地打开一个环境**：校验身份与两份存储都在，**不取写资格**。
    ///
    /// 身份缺失、损坏或属于别的 Agent 时同样明确拒绝（不泄漏任何经历）；两份存储缺一份
    /// 同样是损坏（`storage-missing`）——"身份在，说明这是已建立的环境"这条判据两边一致。
    pub fn open(root: &Path, agent: AgentId) -> Result<Self, StoreError> {
        verify_identity_at(root, &agent)?;
        // 读法由**写资格那条证据**决定，不由调用方挑（见 `reading_tail`）。
        let tail = reading_tail(root);
        // ⚠ **2026-10-06 分卷之后：只核目录 ＋ 活动段**（与写侧 `bind` 同一判据）。
        // 从前这里 `read_trace` 读**全部**经历——分卷之后那就是"每开一次读都重读全部旧卷"，
        // 而这条路的调用方是家的只读状态口（每 15 秒一次）⇒ 它会随历史线性变慢。
        // 现在旧卷在**读到它**（`trace()` / `read_run_at`）或**显式全量读**时暴露。
        let parts = load_parts(root)?;
        let chain_start = parts.last().map(|part| part.chain).unwrap_or(CHAIN_SEED);
        read_trace_file(&root.join(TRACE_FILE), tail, chain_start)?;
        // 事实侧同一判据（分卷之后）：只读**事实目录 ＋ 事实活动段**。
        let fact_parts = load_fact_parts(root)?;
        let fact_chain_start = fact_parts
            .last()
            .map(|part| part.chain)
            .unwrap_or(CHAIN_SEED);
        read_fact_file(&root.join(FACTS_FILE), tail, fact_chain_start)?;
        Ok(Self {
            root: root.to_path_buf(),
            agent,
            tail,
        })
    }

    /// 这个环境的身份。
    pub fn agent(&self) -> &AgentId {
        &self.agent
    }

    /// 环境根目录。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 读回全部经历（读者读法）。
    pub fn trace(&self) -> Result<Vec<TraceRecord>, StoreError> {
        read_trace(&self.root, self.tail)
    }

    /// 读回**序号大于 `after` 的**经历（增量读；判据与 `read_trace_after` 同一份实现）。
    ///
    /// 它是给**派生物**用的：只读活动段 ＋ `seq_end > after` 的卷，**不重读旧卷**。
    /// 要整份重验走 [`EnvironmentReader::trace`]。
    pub fn trace_after(&self, after: u64) -> Result<Vec<TraceRecord>, StoreError> {
        read_trace_after(&self.root, self.tail, after)
    }

    /// 读回全部操作事实（读者读法）。
    pub fn facts(&self) -> Result<Vec<FactRow>, StoreError> {
        read_facts(&self.root, self.tail)
    }

    /// 现在这个环境处在什么准入状态（判据与 [`Environment::admission`] 同一份实现）。
    pub fn admission(&self) -> Result<Admission, StoreError> {
        admission_at(&self.root, self.tail)
    }

    /// 开场那一次读：准入 ＋ 历史上最后一次存储故障（判据与 [`Environment::opening_readings`]
    /// 同一份实现；**一次解析**）。
    pub fn opening_readings(&self) -> Result<OpeningReadings, StoreError> {
        opening_readings_at(&self.root, self.tail)
    }

    /// 健康自检（判据与 [`Environment::health`] 同一份实现）。
    pub fn health(&self) -> Result<EnvironmentHealth, StoreError> {
        health_at(&self.root, self.tail)
    }

    /// 两份存储之间可判定的矛盾（判据与 [`Environment::cross_store_issues`] 同一份实现）。
    pub fn cross_store_issues(&self) -> Result<Vec<CrossStoreIssue>, StoreError> {
        cross_store_issues_at(&self.root, self.tail)
    }
}

/// 只读地核对"这个位置属于谁"（**不取写资格**）：目录形状 + 身份记录比对。
///
/// 它与 [`Environment::open`] 的归属校验是**同一份判据**（`create` / `open` / 读者三条路都走它），
/// 因此"身份不匹配 ⇒ 拒绝、不泄漏经历"在三条路上逐字一致。
fn verify_identity_at(root: &Path, expected: &AgentId) -> Result<(), StoreError> {
    let metadata = fs::symlink_metadata(root).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => StoreError::NotFound,
        _ => StoreError::Io,
    })?;
    if !metadata.is_dir() {
        return Err(StoreError::WrongKind);
    }
    let recorded = read_identity(root)?;
    if recorded != *expected {
        return Err(StoreError::IdentityMismatch {
            expected: expected.clone(),
            found: recorded,
        });
    }
    Ok(())
}

/// 准入判据的**唯一实现**：写者与读者各自给一个 [`TailPolicy`]（见 [`Environment::admission`]）。
fn admission_at(root: &Path, tail: TailPolicy) -> Result<Admission, StoreError> {
    let parts = load_parts(root)?;
    let chain_start = parts.last().map(|part| part.chain).unwrap_or(CHAIN_SEED);
    let (records, _) = read_trace_file(&root.join(TRACE_FILE), tail, chain_start)?;
    admission_from(&parts, &records)
}

/// 准入判据：**目录里折好的旧卷状态 ＋ 活动段**（不重读旧卷）。
///
/// 分卷之后这一格必须这么算，否则"主体能不能开跑"要重读全部历史才答得出来——而那正是分卷要
/// 去掉的东西。折的规则：旧卷的暂停/确认/未收尾各带一格，活动段里的新痕迹盖在它们上面。
fn admission_from(parts: &[SealedPart], records: &[TraceRecord]) -> Result<Admission, StoreError> {
    let previous = parts.last();
    let acknowledged_through = last_acknowledged_seq(records)
        .or_else(|| previous.and_then(|part| part.acknowledged_through));
    let paused =
        last_storage_failure_of(records)?.or_else(|| previous.and_then(|part| part.paused.clone()));
    // `acknowledged` 为真 = 最后一次动的是"显式确认" ⇒ 不暂停（与 `paused_reason_of` 同一判据）。
    if let Some(failure) = &paused
        && !failure.acknowledged
    {
        // ⚠ **走到这里，原因必定在场，因此没有"缺了就编一个"的分支**（2026-10-06 收）。
        //
        // 不变量两侧都成立：**解码端**拒绝 `(没有原因 + 没有被确认)` 那个组合（见 `decode_part`），
        // **记录侧**唯一的产者 `last_storage_failure_of` 也产不出它（`Paused(text)` 必带原因）。
        // 从前这里写的是 `unwrap_or_default()`：一个宽泛兜底，把"没有原因"折成空串，
        // 于是渲染面输出一对空括号——**而系统明明知道"原因没有被记录"这条事实**。
        // 真缺了 = 上面那条不变量破了（逻辑缺陷，不是数据）⇒ 当场说出来，不编一个空串。
        let Some(reason) = failure.reason.clone() else {
            unreachable!("暂停必有原因：解码端与记录侧都不产出无原因的暂停");
        };
        return Ok(Admission::Paused { reason });
    }
    match unfinished_after(
        records,
        acknowledged_through,
        previous.and_then(|part| part.unfinished.clone()),
    ) {
        Some(run) => Ok(Admission::Unfinished { run }),
        None => Ok(Admission::Ready),
    }
}

/// 健康自检的**唯一实现**：写者与读者各自给一个 [`TailPolicy`]（见 [`Environment::health`]）。
///
/// ⚠ **一次解析**（2026-10-06，长跑读数那一刀）：这份自检要问五格读数（活动摘要、两份存储的
/// 条数、暂停、跨存储矛盾），它们全都从**同一份已确认前缀**里出来。从前每一格各读一遍 ⇒
/// **一次调用把同一份文件解析四遍**（实测：3 MB 的账本上自检 842 ms，是打开那一步的 3.8 倍）。
/// 现在两份文件各解析一次，五格都从解析结果里算。
fn health_at(root: &Path, tail: TailPolicy) -> Result<EnvironmentHealth, StoreError> {
    let parts = load_parts(root)?;
    let chain_start = parts.last().map(|part| part.chain).unwrap_or(CHAIN_SEED);
    let (records, _) = read_trace_file(&root.join(TRACE_FILE), tail, chain_start)?;
    let facts = read_facts(root, tail)?;
    let sealed_records: usize = parts.iter().map(|part| part.records).sum();
    // 活动摘要＝旧卷（目录的**摘要行**里带着，要它才解）＋ 活动段（现算）：跨存储判据与列表都从
    // 它算，不重读旧卷。
    let parts = with_part_runs(root, parts)?;
    let mut runs: Vec<RunSummary> = parts
        .iter()
        .flat_map(|part| part.runs.iter().cloned())
        .collect();
    runs.extend(summaries_of(&records)?);
    // 同一个活动只出现一次（横跨两卷时各卷给的是局部摘要，见 [`merge_summaries`]）。
    let runs = merge_summaries(runs);
    Ok(EnvironmentHealth {
        agent: read_identity(root)?,
        paused: matches!(admission_from(&parts, &records)?, Admission::Paused { .. }),
        cross_store: cross_store_issues_from(&runs, &facts)?,
        runs,
        trace_records: sealed_records + records.len(),
        fact_records: facts.len(),
    })
}

/// 暂停标记使用的注入阶段名（它是一条真实记录，不是隐藏状态）。
pub(super) const PAUSED_STAGE: &str = "storage-failure";
/// 暂停标记使用的标签。
const PAUSED_LABEL: &str = "paused";

/// 一个环境目录被上一个写者留下"保存失败暂停"标记了吗？
///
/// 这个判断读的是**已确认记录**，因此重启不能绕过保护。返回原因文本。
///
/// **清除也算一条记录**：`paused` 与 `acknowledged` 都是这个阶段下的真实标记，所以判断的是
/// **最后一条**标记——最后说了"暂停"就是暂停，最后说了"显式确认"才算确认。只认 `paused`
/// 会让显式确认永远无效。
///
/// **词表封闭**：这个阶段下的标记只认 [`PAUSED_LABEL`] 与 [`ACKNOWLEDGED_LABEL`] 两个取值，
/// 认不出的**明确拒绝**（见 [`pause_marker`]）——不猜、也不静默当成别的意思。
pub fn paused_reason(root: &Path) -> Result<Option<String>, StoreError> {
    paused_reason_with(root, TailPolicy::RequireConfirmed)
}

/// 同 [`paused_reason`]，但把"最后一段没有换行"怎么算交给调用方（见 [`TailPolicy`]）。
fn paused_reason_with(root: &Path, tail: TailPolicy) -> Result<Option<String>, StoreError> {
    paused_reason_of(&read_trace(root, tail)?)
}

/// 现在**还停着**吗（从已经解析过的记录算）：最后一条标记说了什么，就是什么。
fn paused_reason_of(records: &[TraceRecord]) -> Result<Option<String>, StoreError> {
    let mut reason = None;
    for record in records {
        if let TracePayload::Injection { stage, blocks, .. } = record.entry()
            && stage == PAUSED_STAGE
        {
            reason = match pause_marker(record.seq(), blocks)? {
                // 显式确认：这条之后的最后标记是"已确认过"。
                PauseMarker::Acknowledged => None,
                PauseMarker::Paused(text) => Some(text),
            };
        }
    }
    Ok(reason)
}

/// `storage-failure` 阶段下那一条标记说的是什么。
enum PauseMarker {
    /// 记下一次保存失败暂停（载荷是原因文本）。
    Paused(String),
    /// 记下一次**显式确认**：调用方已经看过此前的异常。
    Acknowledged,
}

/// 认出这条暂停标记——**词表封闭，认不出就拒绝**。
///
/// 取值只有两个：`paused`（记下暂停）与 `acknowledged`（记下显式确认）。
///
/// ⚠ **2026-09-26 改（旧规矩 → 新规矩）**：第二个取值原来叫 **`resumed`**。改名理由：
/// `resume` 在本仓库已特指刚退役的那条路（**跨进程继续同一段活动**），而这个标记说的是
/// "**这条异常已经被看过了**"——宿主的一次行政确认（`agent_host_acknowledge`），与"继续"无关。
/// 留着它就是一个词两个意思。
///
/// **旧取值明确拒绝，不静默当成别的意思**：从前这里认不出就落到 `Some("unspecified")`，
/// 那是把"我不知道这条标记是什么"说成"暂停、原因未说明"——一句假话；而旧构建写下的
/// `resumed` 正好落进那一支（一个**已经被显式确认过**的环境会被读成又一次暂停）。
/// 现在认不出 ⇒ [`StoreError::UnknownPauseMarker`]。项目没上线、不存在数据迁移，
/// 因此旧环境由这条拒绝**如实报出来**。
///
/// **方向是不对称的，如实登记**：新构建认不出旧取值 ⇒ 明确拒绝；旧构建认不出新取值 ⇒
/// 它那一版**没有标签词表**（只有 `paused` / `resumed` 两支加一个 `unspecified` 兜底），
/// 会把新标记读成"暂停、原因未说明"。那一侧修不了（改不动旧构建），只登记。
fn pause_marker(seq: u64, blocks: &[(String, String)]) -> Result<PauseMarker, StoreError> {
    if blocks.iter().any(|(label, _)| label == ACKNOWLEDGED_LABEL) {
        return Ok(PauseMarker::Acknowledged);
    }
    if let Some((_, text)) = blocks.iter().find(|(label, _)| label == PAUSED_LABEL) {
        return Ok(PauseMarker::Paused(text.clone()));
    }
    Err(StoreError::UnknownPauseMarker { seq })
}

/// 环境**历史上最后一次**存储故障（那一段写不下去时留下的记录）。
///
/// 它是给**主体**看的读数：它只说出"我上次为什么停、是哪一段、有没有被看过"这几条事实——
/// **没有指令**（要不要继续、怎么写，是它自己的事）。这条记录一直就在账本里，只是从前
/// **只有操作者的命令读得到**（2026-10-06 起开场那一块纸条也带给它）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageFailure {
    /// 哪一段活动停下来的（**不是**后来确认那一段：确认说的是"这条异常已经被看过了"）。
    pub run: RunId,
    /// 那次停下的原因（稳定分类标记）。
    ///
    /// `None` 只出现在一种形状里：账本里只有"确认"而没有它确认的那条暂停（旧环境或被裁剪的
    /// 历史）——那时如实说"原因没有被记录"，不编一个。
    pub reason: Option<String>,
    /// 这条异常之后有没有被**显式确认**过（`agent_host_acknowledge` 那一次行政确认）。
    pub acknowledged: bool,
}

/// 开场那**一次读**的读数：准入 ＋ 历史上最后一次存储故障。
///
/// 为什么合成一次读（2026-10-06）：这两格都要走同一份已确认前缀，而那份账本可能很长
/// （合成历史实测：解析约 60–70 ms/MB，线性——见 `tests/durability_reading.rs`）——各读一遍
/// 就是把整份账本白读一遍。顺带把准入本身也从"读两遍"（暂停标记一遍、记录一遍）收成一遍。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpeningReadings {
    /// 现在能不能开始一次新活动。
    pub admission: Admission,
    /// 历史上最后一次存储故障（从来没有过 ⇒ `None`）。
    pub last_storage_failure: Option<StorageFailure>,
}

/// 开场读数（**一次解析**，见 [`OpeningReadings`]）。
fn opening_readings_at(root: &Path, tail: TailPolicy) -> Result<OpeningReadings, StoreError> {
    let parts = load_parts(root)?;
    let chain_start = parts.last().map(|part| part.chain).unwrap_or(CHAIN_SEED);
    let (records, _) = read_trace_file(&root.join(TRACE_FILE), tail, chain_start)?;
    let previous = parts.last();
    Ok(OpeningReadings {
        admission: admission_from(&parts, &records)?,
        last_storage_failure: last_storage_failure_of(&records)?
            .or_else(|| previous.and_then(|part| part.paused.clone())),
    })
}

/// 历史上最后一次存储故障（从已经解析过的记录算）。
///
/// 走一遍记录、**只留最后一条**：确认那一条不重写原因（它说的是"看过了"），因此原因由它
/// 前面那条暂停带过来。
pub(crate) fn last_storage_failure_of(
    records: &[TraceRecord],
) -> Result<Option<StorageFailure>, StoreError> {
    let mut found: Option<StorageFailure> = None;
    for record in records {
        if let TracePayload::Injection { stage, blocks, .. } = record.entry()
            && stage == PAUSED_STAGE
        {
            let run = record.run().clone();
            match pause_marker(record.seq(), blocks)? {
                PauseMarker::Paused(text) => {
                    found = Some(StorageFailure {
                        run,
                        reason: Some(text),
                        acknowledged: false,
                    });
                }
                // **确认不重写原因**：只把"被看过"这一格翻过来；前面没有暂停可确认时，
                // 原因如实留空（不编一个）。
                PauseMarker::Acknowledged => {
                    found = Some(match found.take() {
                        Some(mut failure) => {
                            failure.acknowledged = true;
                            failure
                        }
                        None => StorageFailure {
                            run,
                            reason: None,
                            acknowledged: true,
                        },
                    });
                }
            }
        }
    }
    Ok(found)
}

/// 清掉暂停标记，并**显式确认**调用方已经看过了此前的异常。
///
/// 只有**显式**排除损坏并确认可用之后才允许调用。清标记本身是一条新经历（`acknowledged`），
/// 不删除旧标记——旧标记是真实发生过的事实。
///
/// 它的作用有两层，都由同一条 `acknowledged` 记录兑现：
///
/// - 清掉"保存失败暂停"；
/// - 让**此前**留下、没有收尾的活动不再拦住新活动（调用方已经确认过那些痕迹）。
pub fn clear_paused(env: &mut Environment, run: &RunId) -> Result<u64, StoreError> {
    env.append_trace(
        run,
        TracePayload::Injection {
            stage: PAUSED_STAGE.to_string(),
            turn: None,
            blocks: vec![(
                ACKNOWLEDGED_LABEL.to_string(),
                "explicitly cleared".to_string(),
            )],
        },
    )
}

/// 暂停标记被显式确认（清除）时使用的标签。
///
/// ⚠ **2026-09-26 改**：取值从 `resumed` 改成 `acknowledged`——它说的是"这条异常已经被看过了"，
/// 不是"接着跑同一段活动"（`resume` 那个词已随跨进程恢复退役）。
const ACKNOWLEDGED_LABEL: &str = "acknowledged";

/// **静态头装订**那一条经历的阶段名（持久字段取值）。
///
/// 它记的是**装配在某一刻把哪一份出厂说明装订给了模型**。出厂说明的家在环境根（它是**环境
/// 家具**，不是 Run 状态），但"当时看见的是哪一份"是**历史**——而历史的家在这里。
/// 两个事实、两个家，谁也不冒充谁。
///
/// ⚠️ **它只作见证，不进视野**：Core 不认识静态头（它由适配层在渲染那一刻拼进请求的尾巴），
/// 因此 [`rehydrate`] 对它返回"**合法地不是**轨迹条目"——与 `action-selected` 同族。
///
/// **旧构建边界**：这个取值不在旧构建的阶段表里 ⇒ 旧构建的 [`rehydrate`] 会拒绝那个活动
/// （`unknown-stage`）。**拒绝而不是读错**——方向与 `index` 那一次相反（那一次旧构建会忽略
/// 字段、把同一轮的两个动作认成同一个身份）。已知边界，如实登记。
pub const STATIC_HEAD_STAGE: &str = "static-head";

/// 静态头装订记录里那一段正文的标签（与 `status-bar` / `pinned-notes` 同族）。
pub const STATIC_HEAD_LABEL: &str = "orientation";

/// 最后一条"显式确认"记录的位置。
///
/// 它之后的痕迹才算"还没有被确认过"：这样显式确认不会失效，也不会把已经确认过的旧异常
/// 永远当成拦截理由（那会变成"永久封锁"）。
pub(super) fn last_acknowledged_seq(records: &[TraceRecord]) -> Option<u64> {
    records
        .iter()
        .filter(|record| match record.entry() {
            TracePayload::Injection { stage, blocks, .. } => {
                stage == PAUSED_STAGE && blocks.iter().any(|(label, _)| label == ACKNOWLEDGED_LABEL)
            }
            _ => false,
        })
        .map(|record| record.seq())
        .max()
}

/// 一条记录是不是"某个活动开始了"。
fn is_run_start(record: &TraceRecord) -> bool {
    record.entry().is_started()
}

/// 一条记录是不是"某个活动有可靠结束事实"。
pub(super) fn is_run_ending(record: &TraceRecord) -> bool {
    record.entry().is_ending()
}

/// 在 `acknowledged_through` **之后**，有没有"已登记开始、却没有可靠结束事实"的活动。
///
/// 找到了就返回它。**它不判断原因**：崩溃、强杀，或"暂停标记自己也写失败"都会留下这个形状，
/// 因此这里只能说"上一次没有收尾"，不能冒充"发生了存储故障"。
pub(crate) fn unfinished_run_after(
    records: &[TraceRecord],
    acknowledged_through: Option<u64>,
) -> Option<RunId> {
    let fresh: Vec<&TraceRecord> = records
        .iter()
        .filter(|record| acknowledged_through.is_none_or(|through| record.seq() > through))
        .collect();

    let mut order: Vec<RunId> = Vec::new();
    let mut started: std::collections::BTreeSet<RunId> = std::collections::BTreeSet::new();
    let mut ended: std::collections::BTreeSet<RunId> = std::collections::BTreeSet::new();
    for record in fresh {
        let run = record.run().clone();
        if !order.contains(&run) {
            order.push(run.clone());
        }
        if is_run_start(record) {
            started.insert(run.clone());
        }
        if is_run_ending(record) {
            ended.insert(run);
        }
    }
    // 按出现顺序返回第一个没有收尾的活动：稳定、可解释。
    order
        .into_iter()
        .find(|run| started.contains(run) && !ended.contains(run))
}

/// 两份存储之间**可判定**的矛盾。
///
/// 判据是机械的、可核实的：两个活动锚点（"开始"与"结束"）在经历侧与事实侧必须成对出现，
/// 缺任何一侧都是矛盾，两个方向都报告。
///
/// 空结果只表示**没有发现**矛盾，不代表已经证明两份存储一致：能判定的只是这几个对应关系。
fn cross_store_issues_at(
    root: &Path,
    tail: TailPolicy,
) -> Result<Vec<CrossStoreIssue>, StoreError> {
    cross_store_issues_from(&summaries_at(root, tail)?, &read_facts(root, tail)?)
}

/// 两份存储之间**可判定**的矛盾（**从活动摘要算**）。
///
/// 分卷之后判据必须从摘要来：旧卷只在目录里留摘要，不该为了这一格重读它们。
/// 判据本身一个字没变：两个活动锚点（"开始"与"结束"）在两侧必须成对出现，缺任何一侧都是矛盾，
/// 两个方向都报告；空结果只表示**没有发现**矛盾。
fn cross_store_issues_from(
    runs: &[RunSummary],
    facts: &[FactRow],
) -> Result<Vec<CrossStoreIssue>, StoreError> {
    // ⚠ **两遍扫、各建一份查表**（2026-10-06 长跑读数第四档暴露的二次方）：从前这里是
    // "每个活动扫一遍全部事实、每条事实再扫一遍全部活动"——1200 段活动上健康自检要 **650 ms**，
    // 而它随两边**乘积**增长。判据一个字没变，只是把"查表"变成真的查表。
    let facts_of: std::collections::BTreeSet<(&str, &str)> =
        facts.iter().map(|row| (row.run(), row.fact())).collect();
    let has_fact = |run: &RunId, fact: &str| facts_of.contains(&(run.as_str(), fact));
    let mut issues = Vec::new();
    for summary in runs {
        if summary.started && !has_fact(&summary.run, "run-started") {
            issues.push(CrossStoreIssue {
                run: summary.run.clone(),
                kind: "start-without-fact",
            });
        }
        if summary.ending.is_some() && !has_fact(&summary.run, "run-ended") {
            issues.push(CrossStoreIssue {
                run: summary.run.clone(),
                kind: "ending-without-fact",
            });
        }
    }
    // 反向：事实侧有锚点、经历侧没有。正常写入顺序（经历先、事实后）产生不出这个方向，
    // 因此它同样是一条可判定的矛盾，不能用"另一方向没发现问题"把它吞掉。
    let trace_of: std::collections::BTreeMap<&str, &RunSummary> = runs
        .iter()
        .map(|summary| (summary.run.as_str(), summary))
        .collect();
    for row in facts {
        let run = row.run();
        let summary = trace_of.get(run).copied();
        match row.fact() {
            "run-started" if summary.is_none_or(|summary| !summary.started) => {
                issues.push(CrossStoreIssue {
                    run: RunId::new(run),
                    kind: "fact-without-start",
                });
            }
            "run-ended" if summary.is_none_or(|summary| summary.ending.is_none()) => {
                issues.push(CrossStoreIssue {
                    run: RunId::new(run),
                    kind: "fact-without-ending",
                });
            }
            _ => {}
        }
    }
    Ok(issues)
}

/// 从环境根读回**这是谁的环境**（中立入口）。
///
/// 给"需要一个只读读者、而手里只有环境根"的调用方——例如某个 App 要折**自己那一份**
/// 来源索引（它得先能 [`EnvironmentReader::open`]）。
/// ⚠ 它是**中立的**：身份是 Foundation 的概念，**不是任何 App 的词**。
pub fn identity_at(root: &Path) -> Result<AgentId, StoreError> {
    read_identity(root)
}

/// 读身份记录。
fn read_identity(root: &Path) -> Result<AgentId, StoreError> {
    let path = root.join(IDENTITY_FILE);
    if !path.exists() {
        return Err(StoreError::IdentityMissing);
    }
    let text = fs::read_to_string(&path).map_err(io_error)?;
    parse_identity(&text).ok_or(StoreError::IdentityCorrupt)
}

/// 从一行身份记录里取出 AgentId。格式错误一律是"损坏"，不猜。
fn parse_identity(text: &str) -> Option<AgentId> {
    let value = extract_string_field(text, "agent")?;
    if value.is_empty() {
        return None;
    }
    Some(AgentId::new(value))
}

/// 取得环境的排他写资格（真实的操作系统级文件锁）。
///
/// 它是**排他保护的唯一入口**：`create` 与 `open` 都先经过它，因此"先检查后写入"那种
/// 依赖时序的写法不会出现。持有者进程退出时操作系统自动释放锁，不需要清理任务，
/// 也不依据锁文件时间戳猜测死进程并抢占。
///
/// ⚠ **"被别人握着"与"锁这一步坏了"是两类，不许压成一个**（2026-10-06 收）：宿主对
/// [`StoreError::AlreadyOpen`] 的策略是**等**（见 `host_loop::open_when_free`），而对别的错误
/// **立刻报出**。把锁自己的 I/O 故障也说成 `AlreadyOpen`，代价是双向的——白等满耐心（真机
/// 耐心按账本增长），然后报一句"环境忙"，把操作者指向**查谁在里面**这条走不通的路。
/// 分类照 `StoreError::Io`（"一般 I/O 失败"）走：它的下一步与别的 I/O 失败相同（去看诊断）。
///
/// 两支**穷尽**（`TryLockError` 今天只有这两个取值）：因此这里没有兜底臂——将来 std 新增取值
/// 会**编译不过**，而不是悄悄并进"环境忙"。
fn acquire_writer_lock(root: &Path) -> Result<File, StoreError> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(WRITER_LOCK_FILE))
        .map_err(io_error)?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        // 此刻有别人在里面：**暂时**，宿主等它放开。
        Err(fs::TryLockError::WouldBlock) => Err(StoreError::AlreadyOpen),
        // 锁本身失败（不是"被别人拿着"）：真 I/O 故障，**不许**说成"环境忙"。
        Err(fs::TryLockError::Error(_)) => Err(StoreError::Io),
    }
}

/// 打开一个**已经存在**的只追加文件的追加句柄。
///
/// 刻意**不** create：文件缺失是已经建立的环境的损坏（见 [`StoreError::StorageMissing`]），
/// 不是"顺手建一份空的"的理由。
fn open_append(root: &Path, name: &'static str) -> Result<File, StoreError> {
    OpenOptions::new()
        .append(true)
        .open(root.join(name))
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => StoreError::StorageMissing { file: name },
            _ => io_error(error),
        })
}

/// 追加句柄（**建**文件）：只用在封口之后——那时活动段刚被搬走，必须新建一个空的。
///
/// 与 [`open_append`] 分开正是为了"缺文件即损坏"那条规矩：首次打开**不许**顺手建空账本。
fn open_append_new(root: &Path, name: &'static str) -> Result<File, StoreError> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join(name))
        .map_err(io_error)
}

/// 把一个只追加文件切成**已确认的行**。
///
/// 已确认前缀的边界不是"有多少行"，而是"写到哪一个换行符为止"：
///
/// - 文件为空 → 没有任何已确认行；
/// - 最后一个字节不是 `\n` → 最后一段没有写完（也没有被确认为耐久），属于未知尾部；
/// - 中间出现空行 → 文件不自洽。
///
/// 用"按行切分"来数行会**丢掉换行信息**，因此无法区分"完整 JSON 带换行"与"完整 JSON 不带换行"
/// ——后者恰恰是"写了一半但 JSON 碰巧合法"的形状。这里按字节边界判断，才认得出它。
/// 读两份存储时，**最后一段没有换行**该怎么算。
///
/// 两种答案都是对的，取决于**有没有写者正在写**（2026-10-06 立；判据由 [`reading_tail`] 从
/// 写资格那条证据推出来，不是调用方各自挑一个）：
///
/// - [`TailPolicy::RequireConfirmed`]（**没有写者**）：那一段是**没人收尾的残留**（崩在半路），
///   照旧**拒绝**——不许把未知尾部报告成一份更短的健康历史。
/// - [`TailPolicy::DropTornTail`]（**有写者在写**）：那一段是它**此刻写到一半**的那一行
///   （写完才 sync），**丢掉它、读前一条已确认的记录**——读者不需要文件自洽，他只需要
///   "读到的每一条都是已确认的"。
///
/// 这条分界就是"**读者读已确认前缀，写者要文件自洽**"：同一个文件、同一时刻，两种人各读各的。
/// 它**不**放宽内容校验：完整的一行读不出来、语义不合法、序号不前进，两种读法都照旧拒绝
/// （那是真的损坏，不是"写了一半"）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TailPolicy {
    RequireConfirmed,
    DropTornTail,
}

/// **这一刻该按哪种读法读**：有没有写者正在写这个环境（判据见 [`TailPolicy`]）。
///
/// 读者构造时问一次，答案就固定在这次读取里——"打开那一刻有没有写者"是一个**读数**，
/// 不是保证（问完到读完之间写者可能开始或结束，那一小段窗口里读到的仍只是已确认前缀）。
pub(crate) fn reading_tail(root: &Path) -> TailPolicy {
    if writer_is_active(root) {
        TailPolicy::DropTornTail
    } else {
        TailPolicy::RequireConfirmed
    }
}

/// 现在有没有写者持有这个环境的写资格。
///
/// **探一下就走**：拿到锁立刻释放（`File` 一丢操作系统就解锁），因此读者不占写资格、也不会
/// 把后来的写者挡在外面。锁文件不在场 ⇒ 从来没有写者 ⇒ 没有人在写。
///
/// 拿不到锁（包括任何错误）一律当作"有写者"：那一边的读法只是**丢掉一段没写完的尾部**，
/// 而猜错另一边的代价是把崩溃残留读成健康历史——两害相权，取前者。
fn writer_is_active(root: &Path) -> bool {
    let Ok(probe) = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(WRITER_LOCK_FILE))
    else {
        return false;
    };
    probe.try_lock().is_err()
}

/// 一份存储里**已确认的一行**：正文 ＋ 它在文件里的字节位置。
///
/// 位置是**派生读数**（2026-10-06 卷内偏移索引那一刀）：封口时本来就要把这一卷读完，顺手把
/// 每一行的起点与长度累加起来，就能给"读一段旧活动"留下一个可 `seek` 的区间。它不改行的内容、
/// 也不参与校验——校验和仍然算在**正文**上（见 [`line_sum`]），位置只是"这一行从哪一字节开始"。
struct ConfirmedLine {
    /// 这一行第一个字节在文件里的偏移。
    offset: u64,
    /// 这一行的正文（不含行尾校验和与换行；`\r` 已剥）。
    text: String,
    /// 这一行在盘上占的字节数（含换行，以及被剥掉的那个 `\r`）。
    bytes: u64,
}

fn confirmed_lines(
    path: &Path,
    name: &'static str,
    tail: TailPolicy,
) -> Result<Vec<ConfirmedLine>, StoreError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(StoreError::StorageMissing { file: name });
        }
        Err(error) => return Err(io_error(error)),
    };
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let body = match text.strip_suffix('\n') {
        Some(body) => body,
        // 最后一段没有换行符：它没有被确认为耐久，属于未知尾部。
        None => match tail {
            TailPolicy::RequireConfirmed => {
                return Err(StoreError::CorruptTail {
                    seq: text.matches('\n').count() as u64,
                });
            }
            // 只读的人：丢掉这一段（它可能正是写者此刻写到一半的那一行），读它前面那些。
            TailPolicy::DropTornTail => match text.rfind('\n') {
                Some(index) => &text[..index],
                // 整个文件就只有这一段没写完的行：确认前缀是**空的**，不是损坏。
                None => return Ok(Vec::new()),
            },
        },
    };
    let mut lines = Vec::new();
    let mut offset: u64 = 0;
    for raw in body.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.is_empty() {
            return Err(StoreError::CorruptTail {
                seq: lines.len() as u64,
            });
        }
        // 盘上这一行 = `raw` ＋ 换行（`\r` 若在，本来就在 `raw` 里）。
        let bytes = raw.len() as u64 + 1;
        lines.push(ConfirmedLine {
            offset,
            text: line.to_string(),
            bytes,
        });
        offset += bytes;
    }
    Ok(lines)
}

/// 读回全部经历。
///
/// 每一行都要**完整解码**并且**语义可解释**：顺序不前进、读不出来、或者读出来却无法解释，
/// 一律拒绝。校验只覆盖"本写入者可能写出的形状"，不猜、不修、不改写文件。
///
/// `tail` 只决定"最后一段没有换行"怎么算（见 [`TailPolicy`]），内容校验两种读法完全一样。
pub(crate) fn read_trace(root: &Path, tail: TailPolicy) -> Result<Vec<TraceRecord>, StoreError> {
    read_trace_with_chain(root, tail).map(|(records, _)| records)
}

/// 读回**序号大于 `after` 的**经历（增量读）。
///
/// ## 它为什么存在（判据）
///
/// 有些派生物只需要"**上次折到之后**又发生了什么"（例如来源索引：哪条字是什么时候、因哪次
/// 活动而起的）。而 [`read_trace`] **每次都重读全部旧卷** —— 那让这类派生物的成本**随历史
/// 线性增长**，而那正是本仓判过的定时炸弹（只在几年后才显形，而那时没人在看）。
///
/// ## 判据：**只读它该读的**
///
/// 读**活动段 ＋ `seq_end > after` 的那些卷**；整卷都已折过的**一个字节都不读**。
/// 跳过一卷时，链值从**目录里记着的封口链值**续起（[`SealedPart::chain`]）——
/// 因此"链把全部历史绑在一起"在增量读里照旧成立，而**不需要读那一卷**。
///
/// ⚠ **它不校验被跳过的那些卷**（那些字节根本没被读）。要整份重验就走 [`read_trace`]。
/// 这不是偷工减料，是分工：`read_trace` 是"**整份重验**"，这个是"**接着上次往下读**"。
///
/// ⚠ 它是**中立的**：只说"序号大于 N 的轨迹条目"，**不认识任何 App、任何业务词**——
/// 谁要折什么由上层决定（`store` 不认识 App，见本模块头注释）。
pub(crate) fn read_trace_after(
    root: &Path,
    tail: TailPolicy,
    after: u64,
) -> Result<Vec<TraceRecord>, StoreError> {
    let parts = load_parts(root)?;
    let mut chain = CHAIN_SEED;
    let mut records = Vec::new();
    for part in &parts {
        if part.seq_end <= after {
            // 整卷都折过了：**不读它**，链接着它的封口值往下走。
            chain = part.chain;
            continue;
        }
        let (part_records, next) = read_trace_file(&part.path(root), tail, chain)?;
        records.extend(
            part_records
                .into_iter()
                .filter(|record| record.seq() > after),
        );
        chain = next;
    }
    let (active, _) = read_trace_file(&root.join(TRACE_FILE), tail, chain)?;
    records.extend(active.into_iter().filter(|record| record.seq() > after));
    Ok(records)
}

/// 读回全部经历，**并把链值一起带出来**（打开时要用它继续往下追加）。
///
/// 每一行都过校验和：**正文逐字节核**（见 [`chain_checksum`]）——这既是"这一条没被改过"，
/// 也是"它前面那些旧记录没被改过"（旧记录自己没有校验和，但它们进了链）。旧记录**不校验、
/// 只进链**（旧环境不迁移）。校验和对不上 ⇒ [`StoreError::ChecksumMismatch`]，
/// 与序号不自洽、解释不了同属**可判定的损坏**（见 `is_structural_damage`）。
fn read_trace_with_chain(
    root: &Path,
    tail: TailPolicy,
) -> Result<(Vec<TraceRecord>, u32), StoreError> {
    // **显式全量读**：逐卷核校验和，再读活动段。分卷之后开机**不走**这条路（见 `bind`），
    // 因此它同时就是"整份重验"那条路——要重验全部历史，读它。
    let parts = load_parts(root)?;
    let mut chain = CHAIN_SEED;
    let mut records = Vec::new();
    for part in &parts {
        let (part_records, next) = read_trace_file(&part.path(root), tail, chain)?;
        records.extend(part_records);
        chain = next;
    }
    let (active, next) = read_trace_file(&root.join(TRACE_FILE), tail, chain)?;
    records.extend(active);
    Ok((records, next))
}

/// 读**一个经历文件**（活动段或某一卷封口段），链值从 `chain_start` 续起。
///
/// 分卷之后这一层是必需的：活动段不是从 [`CHAIN_SEED`] 起算，而是从**上一卷封口时的链值**
/// 续起——否则"链把全部历史绑在一起"这句话在分卷之后就断了。
pub(super) fn read_trace_file(
    path: &Path,
    tail: TailPolicy,
    chain_start: u32,
) -> Result<(Vec<TraceRecord>, u32), StoreError> {
    let (records, chain, _) = read_trace_file_detailed(path, tail, chain_start)?;
    Ok((records, chain))
}

/// 同 [`read_trace_file`]，但**把每一段活动占的字节区间一起带出来**（封口时用它写卷内偏移索引）。
///
/// 区间不是另读一遍算的：它就在这次读的循环里**逐行累加行长**得到（那一刀本来就要读完这一卷）。
pub(crate) fn read_trace_file_detailed(
    path: &Path,
    tail: TailPolicy,
    chain_start: u32,
) -> Result<(Vec<TraceRecord>, u32, Vec<RunSpan>), StoreError> {
    let lines = confirmed_lines(path, TRACE_FILE, tail)?;
    let mut records = Vec::new();
    let mut spans: Vec<RunSpan> = Vec::new();
    let mut slot_of: BTreeMap<String, usize> = BTreeMap::new();
    let mut previous: Option<u64> = None;
    let mut chain = chain_start;
    for (index, line) in lines.iter().enumerate() {
        let corrupt = StoreError::CorruptTail { seq: index as u64 };
        let (body, sum) = match line_sum(&line.text) {
            LineSum::Absent(body) => (body, None),
            LineSum::Present(body, sum) => (body, Some(sum)),
            LineSum::Unreadable => {
                return Err(StoreError::ChecksumMismatch { seq: index as u64 });
            }
        };
        // 这一行的期望链值（算在**正文**上；旧记录没有校验和，只进链）。
        let expected = chain_checksum(chain, body.as_bytes());
        // **一行一次解析**：`seq` / `run` / 正文都从同一个 `Value` 里取（从前各解析一遍）。
        let value: serde_json::Value = serde_json::from_str(body).ok().ok_or(corrupt.clone())?;
        let seq = value
            .get("seq")
            .and_then(|seq| seq.as_u64())
            .ok_or(corrupt.clone())?;
        // 序号必须严格前进：不然"顺序"就不再是唯一依据。倒退与重复都落在这里。
        if let Some(previous) = previous
            && seq != previous + 1
        {
            return Err(StoreError::OutOfOrder {
                previous,
                found: seq,
            });
        }
        let run = value
            .get("run")
            .and_then(|run| run.as_str())
            .ok_or(corrupt.clone())?
            .to_string();
        let entry = decode_trace_value(&value).ok_or(corrupt)?;
        if !trace_entry_is_interpretable(&entry) {
            return Err(StoreError::MalformedEntry { seq: index as u64 });
        }
        // **校验和放在结构检查之后**：结构说得通、却与写下去的那串字节不同，才是校验和的活。
        // 这样"哪一类损坏"的读数不会被抢走——例如重复一行仍然报"序号不自洽"（那更具体），
        // 而"文本字段里翻了一个 bit"只有这里看得出来。
        if let Some(sum) = sum
            && sum != expected
        {
            return Err(StoreError::ChecksumMismatch { seq: index as u64 });
        }
        // 区间：这一段**第一次出现**时记下起点与**起点处的链值**，之后只把末尾往后推
        // （`chain` 此刻仍是"这一行之前"的链值，因为推进在下面）。
        match slot_of.get(run.as_str()) {
            Some(slot) => spans[*slot].end = line.offset + line.bytes,
            None => {
                slot_of.insert(run.clone(), spans.len());
                spans.push(RunSpan {
                    run: RunId::new(run.clone()),
                    first: line.offset,
                    end: line.offset + line.bytes,
                    chain,
                });
            }
        }
        chain = expected;
        records.push(TraceRecord {
            seq,
            run: RunId::new(run),
            at_millis: value.get(AT_FIELD).and_then(|at| at.as_u64()),
            entry,
        });
        previous = Some(seq);
    }
    Ok((records, chain, spans))
}

/// 一行过**校验和**：返回（正文，这一行的链值）。
///
/// 它只做**便宜**的那一半（CRC 与一次十六进制比较），不解 JSON——读**切片**那条路要用它把区间
/// 里的每一行都折进链（见 [`read_trace_slice`]），而**解 JSON** 只花在这一段自己的行上。
pub(crate) fn verified_line(
    chain: u32,
    line: &str,
    index: usize,
) -> Result<(&str, u32), StoreError> {
    let (body, sum) = match line_sum(line) {
        LineSum::Absent(body) => (body, None),
        LineSum::Present(body, sum) => (body, Some(sum)),
        LineSum::Unreadable => return Err(StoreError::ChecksumMismatch { seq: index as u64 }),
    };
    let expected = chain_checksum(chain, body.as_bytes());
    if let Some(sum) = sum
        && sum != expected
    {
        return Err(StoreError::ChecksumMismatch { seq: index as u64 });
    }
    Ok((body, expected))
}

/// 把一行的**正文**解成一条记录；不是 `want` 这一段就返回 `None`（**不报错**——那不是损坏）。
///
/// 它**只解 JSON**，不碰校验和：调用它的两条路各自决定要不要先折链（切片折，活动段不折——
/// 见 [`read_trace_file_for_run`]）。⚠ 三条路的**判据顺序故意不同**：全量读把结构检查放在校验和
/// 之前，为的是"哪一类损坏"的读数不被抢走（见 [`read_trace_file_detailed`]）；切片先过校验和、
/// 再解 JSON，因为"命不中的行一个字节都不解码"正是它省下钱的地方。它们拒绝的是同一批字节，
/// 只是**报哪一类**可能不同。
pub(super) fn decode_trace_line(
    body: &str,
    index: usize,
    want: &RunId,
) -> Result<Option<TraceRecord>, StoreError> {
    let corrupt = StoreError::CorruptTail { seq: index as u64 };
    let value: serde_json::Value = serde_json::from_str(body).ok().ok_or(corrupt.clone())?;
    let seq = value
        .get("seq")
        .and_then(|seq| seq.as_u64())
        .ok_or(corrupt.clone())?;
    let run = value
        .get("run")
        .and_then(|run| run.as_str())
        .ok_or(corrupt.clone())?;
    if run != want.as_str() {
        return Ok(None);
    }
    let entry = decode_trace_value(&value).ok_or(corrupt)?;
    if !trace_entry_is_interpretable(&entry) {
        return Err(StoreError::MalformedEntry { seq: index as u64 });
    }
    Ok(Some(TraceRecord {
        seq,
        run: RunId::new(run),
        at_millis: value.get(AT_FIELD).and_then(|at| at.as_u64()),
        entry,
    }))
}

/// 读**一个经历文件**，但只**解码**属于 `run` 的行（其余行只做一次字节预筛）。
///
/// 它是"读一段旧活动"落在**活动段**上的那一半：活动段还在长，因此没有卷内偏移索引（索引只在
/// 封口那一刻写）。
///
/// ⚠ **它与 [`read_trace_slice`] 有意不同**：切片有索引给的**区间起点链值**，因此能**独立校验**
/// 那一刀；活动段没有那个链值，要核它就得把整段从头折叠一遍——那正是这一刀要省的钱
/// （同一条判据见 [`RunSpan`] 的文档）。因此这一段只保证：**返回的每一条都解码得出来、都能被
/// 解释、都属于这一段**，而且它们之间序号**严格递增**；它**不**声称活动段自洽。
/// 要"整份前缀都自洽"的读数，走 [`read_trace_at`] 或 [`EnvironmentReader::trace`]。
///
/// ⚠ 这省掉的不是"没人看过"，是"重复再看一遍"：活动段的已确认前缀在**打开那一刻**已经由写者
/// （`bind`）或读者（`open`）全量校验过，之后只有写者往里追加**它自己逐条校验过**的行。
pub(super) fn read_trace_file_for_run(
    path: &Path,
    tail: TailPolicy,
    run: &RunId,
) -> Result<Vec<TraceRecord>, StoreError> {
    let lines = confirmed_lines(path, TRACE_FILE, tail)?;
    // 预筛的针：写侧用 `serde_json` 序列化那一格，因此**同一个序列化器**生成的针与盘上的字节
    // 逐字节同形（不是手写转义）。再加一个"活动身份原样出现"的针取**并集**——宁可多解码一行，
    // 不可漏一行；命中错了无害（解码后按 `run` 再筛一次）。
    let needle = format!("\"run\":{}", serde_json::Value::from(run.as_str()));
    let mut records = Vec::new();
    let mut previous: Option<u64> = None;
    for (index, line) in lines.iter().enumerate() {
        if !line.text.contains(&needle) && !line.text.contains(run.as_str()) {
            continue;
        }
        // 命中的行要**带得出正文**：校验和那一格读不出来，就是这一行不成立（它是不是"对"的，
        // 归折叠链管——见上）。
        let body = match line_sum(&line.text) {
            LineSum::Absent(body) => body,
            LineSum::Present(body, _) => body,
            LineSum::Unreadable => {
                return Err(StoreError::ChecksumMismatch { seq: index as u64 });
            }
        };
        let Some(record) = decode_trace_line(body, index, run)? else {
            continue;
        };
        if let Some(previous) = previous
            && record.seq() <= previous
        {
            return Err(StoreError::OutOfOrder {
                previous,
                found: record.seq(),
            });
        }
        previous = Some(record.seq());
        records.push(record);
    }
    Ok(records)
}

/// 读回全部操作事实（**整份**：逐卷 ＋ 活动段）。
///
/// 与经历同样**完整解码并校验语义**：只数行数不算校验，"打开时读过一遍内容"才是。
/// ⚠ **2026-10-06 起也带校验和**：事实侧有**自己的一条链**（与经历那条互不影响），
/// 判据与读法完全同 [`read_trace_with_chain`]。
pub(super) fn read_facts(root: &Path, tail: TailPolicy) -> Result<Vec<FactRow>, StoreError> {
    read_facts_with_chain(root, tail).map(|(rows, _)| rows)
}

/// 读回全部操作事实，**并把链值一起带出来**（同 [`read_trace_with_chain`]）。
///
/// ⚠ **2026-10-06 分卷之后**：这是**整份**那条路（逐卷核一遍，再读活动段）——开机**不走**它
/// （见 [`Environment::bind`]），它服务的是健康自检、跨存储判据与诊断这些"要一份完整读数"的
/// 地方。
fn read_facts_with_chain(root: &Path, tail: TailPolicy) -> Result<(Vec<FactRow>, u32), StoreError> {
    let parts = load_fact_parts(root)?;
    let mut chain = CHAIN_SEED;
    let mut rows = Vec::new();
    for part in &parts {
        let (part_rows, next) = read_fact_file(&part.path(root), tail, chain)?;
        rows.extend(part_rows);
        chain = next;
    }
    let (active, chain) = read_fact_file(&root.join(FACTS_FILE), tail, chain)?;
    rows.extend(active);
    Ok((rows, chain))
}

/// 读**一个事实文件**（活动段或某一卷封口段），链值从 `chain_start` 续起。
///
/// 判据与 [`read_trace_file_detailed`] 完全同形（一行一次解析、结构检查在校验和之前、序号必须
/// 严格前进），只是这一份存的是一条**操作事实**而不是一条经历。
pub(crate) fn read_fact_file(
    path: &Path,
    tail: TailPolicy,
    chain_start: u32,
) -> Result<(Vec<FactRow>, u32), StoreError> {
    let lines = confirmed_lines(path, FACTS_FILE, tail)?;
    let mut rows = Vec::new();
    let mut previous: Option<u64> = None;
    let mut chain = chain_start;
    for (index, line) in lines.iter().enumerate() {
        let corrupt = StoreError::CorruptTail { seq: index as u64 };
        let (body, sum) = match line_sum(&line.text) {
            LineSum::Absent(body) => (body, None),
            LineSum::Present(body, sum) => (body, Some(sum)),
            LineSum::Unreadable => {
                return Err(StoreError::ChecksumMismatch { seq: index as u64 });
            }
        };
        let expected = chain_checksum(chain, body.as_bytes());
        // **一行一次解析**（同 [`read_trace`]）：`seq` 与事实正文从同一个 `Value` 里取。
        let value: serde_json::Value = serde_json::from_str(body).ok().ok_or(corrupt.clone())?;
        let seq = value
            .get("seq")
            .and_then(|seq| seq.as_u64())
            .ok_or(corrupt.clone())?;
        if let Some(previous) = previous
            && seq != previous + 1
        {
            return Err(StoreError::OutOfOrder {
                previous,
                found: seq,
            });
        }
        let row = decode_fact_row_value(&value).ok_or(corrupt)?;
        if !fact_row_is_interpretable(&row) {
            return Err(StoreError::MalformedEntry { seq: index as u64 });
        }
        // 结构检查之后才看校验和（同 [`read_trace_with_chain`]）。
        if let Some(sum) = sum
            && sum != expected
        {
            return Err(StoreError::ChecksumMismatch { seq: index as u64 });
        }
        chain = expected;
        rows.push(row);
        previous = Some(seq);
    }
    Ok((rows, chain))
}

/// 主体收场的已知取值（与 `subject_str` 一一对应）。
///
/// 这是**封闭词汇表**：`SubjectEnding` 是一个枚举，所以一个不在表里的取值无法被解释成任何
/// 一种收场。读到一个不认识的取值时，宁可拒绝打开，也不能把它当成有效历史读出来。
///
/// ⚠ **2026-10-10**：今天会写下去的是 `ended`（收场）与 `held`（搁下）两个——**区别只在动词，
/// 两个都不带值**；四个旧值是 2026-10-10 之前的历史，**照读不写**。
/// 表因此有六个值而**写侧只有两个**。
const SUBJECT_ENDINGS: [&str; 6] = [
    "ended",
    "held",
    "completed",
    "silent",
    "declined",
    "waiting",
];
/// 系统收场的已知取值（与 `system_str` 一一对应）。
const SYSTEM_ENDINGS: [&str; 6] = [
    "model-failed",
    "action-failed",
    "cancelled",
    "timeout",
    "crashed",
    "indeterminate",
];
/// 事实种类的已知取值（与 `encode_fact` 写出的名字一一对应）。
const KNOWN_FACTS: [&str; 5] = [
    "run-started",
    "action-prepared",
    "action-dispatched",
    "action-result",
    "run-ended",
];

/// 模型响应的**决定种类**已知取值（与 `response_payload` 写出的名字一一对应）。
///
/// 与 [`SUBJECT_ENDINGS`] 同一个道理：决定是一个枚举，所以一个不在表里的取值无法被解释成
/// 任何一种决定。宁可拒绝打开，也不把它当成有效历史读出来。
const KNOWN_DECISIONS: [&str; 3] = ["act", "finish", "yield"];

/// 一条收场是否**恰好**是主体选择或系统收场之一，而且取值在封闭词汇表里。
fn ending_is_interpretable(subject: Option<&str>, system: Option<&str>) -> bool {
    match (subject, system) {
        (Some(value), None) => SUBJECT_ENDINGS.contains(&value),
        (None, Some(value)) => SYSTEM_ENDINGS.contains(&value),
        // 两种都有是自相矛盾，两种都没有是什么也没说：都不是本写入者可能写出的形状。
        _ => false,
    }
}

/// 一条模型响应的**形状**是否可解释。
///
/// 校验的是"本写入者不可能写出的形状"，与解码成功与否是两件事：
///
/// - 决定种类必须在封闭词汇表里；
/// - `act` 必须带一个非空行动标识，而且不得同时带结束取值（那是两份互相矛盾的意图）；
/// - `finish` 必须带一个封闭词汇表里的主体收场，而且不得带行动或参数；
/// - `yield` 既不带行动也不带结束，也不带参数（参数只属于一次行动）；
/// - 续接数据的三个字段要么一起在、要么一起不在，而且协议名与载荷都非空。
fn model_response_is_interpretable(
    decision: &str,
    action: Option<&str>,
    params: &[(String, String)],
    extra: &ExtraActions,
    ending: Option<&str>,
    protocol: Option<&(String, u32, String)>,
) -> bool {
    // 先过封闭词汇表：一个不在表里的取值无法被解释成任何一种决定。表是唯一来源，
    // 下面那一段只补充"这一种决定还要求哪些字段"。
    if !KNOWN_DECISIONS.contains(&decision) {
        return false;
    }
    // `extra-actions` 里每一项都必须是一件**能表达的动作**：空标识不是动作。
    let extra_ok = extra.iter().all(|(id, _)| !id.is_empty());
    let decision_ok = match decision {
        // 一轮可以请求多个动作：第 0 个在 `action`，其余在 `extra`。至少一个。
        "act" => action.is_some_and(|id| !id.is_empty()) && ending.is_none() && extra_ok,
        "finish" => {
            action.is_none()
                && params.is_empty()
                && extra.is_empty()
                && ending.is_some_and(|value| SUBJECT_ENDINGS.contains(&value))
        }
        "yield" => action.is_none() && params.is_empty() && extra.is_empty() && ending.is_none(),
        _ => false,
    };
    if !decision_ok {
        return false;
    }
    protocol.is_none_or(|(name, _, payload)| !name.is_empty() && !payload.is_empty())
}

/// 一条**已经解出来**的经历，内容是否可解释。
///
/// 解码成功不等于形状合法：这里只校验"本写入者不可能写出的形状"，不猜、不放宽。
fn trace_entry_is_interpretable(entry: &TracePayload) -> bool {
    match entry {
        TracePayload::Ending {
            subject,
            system,
            failure,
        } => {
            if !ending_is_interpretable(subject.as_deref(), system.as_deref()) {
                return false;
            }
            // 分类只属于模型失败，而且取值必须在封闭词汇表里 —— 与事实侧 `run-ended` 的
            // `failure` 逐字同一条规矩。缺字段是**合法**的：写入端"没有分类就不写这个字段"，
            // 旧记录就是这一形态。两种形状不可解释：分类名不认识（fail-closed）；
            // 结局不是模型失败却带着分类（自相矛盾）。
            match failure {
                None => true,
                Some(class) => {
                    system.as_deref() == Some("model-failed")
                        && crate::core::ModelFailureClass::from_str(class).is_some()
                }
            }
        }
        TracePayload::ModelResponse {
            decision,
            action,
            params,
            extra,
            ending,
            protocol,
            ..
        } => model_response_is_interpretable(
            decision,
            action.as_deref(),
            params,
            extra,
            ending.as_deref(),
            protocol.as_ref(),
        ),
        _ => true,
    }
}

/// 一条**已经解出来**的事实行，内容是否可解释。
fn fact_row_is_interpretable(row: &FactRow) -> bool {
    if !KNOWN_FACTS.contains(&row.fact()) {
        return false;
    }
    // 只有收场事实带收场；它必须恰好带一种，而且取值在封闭词汇表里。
    if row.fact() == "run-ended" {
        let (subject, system) = match row.outcome() {
            Some(text) => match text.split_once(':') {
                Some(("subject", value)) => (Some(value), None),
                Some(("system", value)) => (None, Some(value)),
                _ => return false,
            },
            None => (None, None),
        };
        if !ending_is_interpretable(subject, system) {
            return false;
        }
        // 分类只属于模型失败，而且取值必须在封闭词汇表里。
        //
        // 缺字段是**合法**的：写入端"没有分类就不写这个字段"，所以旧记录就是这一形态。
        // 两种形状不可解释：分类名不认识（fail-closed，与 outcome 的取值同一纪律）；
        // 结局不是模型失败却带着分类（自相矛盾）。
        return match row.failure() {
            None => true,
            Some(class) => {
                system == Some("model-failed")
                    && crate::core::ModelFailureClass::from_str(class).is_some()
            }
        };
    }
    // 非收场事实不得携带收场字段：那会是"同一行说了两件互相矛盾的事"。
    if row.outcome().is_some() {
        return false;
    }
    // 分类同属收场事实：别的行带着它同样是"说了两件互相矛盾的事"。
    if row.failure().is_some() {
        return false;
    }
    // 代价同属收场事实：别的行带着它（时长或用量）同样是"说了两件互相矛盾的事"。
    if row.cost().elapsed_millis.is_some() || row.cost().usage.is_some() {
        return false;
    }
    // 子序号只属于**行动事实**：它答的是"这一轮里的第几个动作"，别的行没有这个问题可答。
    if row.index().is_some() && !is_action_fact(row.fact()) {
        return false;
    }
    true
}
