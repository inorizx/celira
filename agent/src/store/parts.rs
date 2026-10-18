//! 分卷与索引（2026-10-08 从 `store.rs` 拆出；**盘上格式一个字节没动**）。
//!
//! 判据是"谁在哪个文件里实现这套格式"：这一段自成一体——封口的段、一张目录、每卷一份
//! 卷内偏移索引、事实侧同一套形状，加上"整份读回"那个入口。
//!
//! **公开面原样留在 `crate::store::` 下**（父模块 `pub use parts::*`），所以消费方一处都不用改。
//! 它照旧受同一批封闭名单约束：文件系统与 `serde_json` 各自显式列入（见 `tests/minimal_agent.rs`）。

use super::*;

// ---------------------------------------------------------------------------
// 分卷：封口的段 ＋ 一张目录（市面标准形态第二步，2026-10-06）
// ---------------------------------------------------------------------------
//
// 判据（主人裁决："账本不该是一个无限长的文本，就该像日志那样分块"）：
// **活动段写满就封口，封口段永不再写；开机只仔细检查活动段 ＋ 目录，不重读旧卷。**
// 同族做法：Kafka 的段 ＋ 索引、KurrentDB 的 chunk ＋ checkpoint ＋ 显式哈希校验、
// Postgres 的 WAL 段 ＋ checkpoint、SQLite 的 WAL 帧 ＋ wal-index ＋ checkpoint。
//
// 为什么必须有**行尾校验和**（上一刀）才敢做这一件：封口段凭什么可信？答案是"它的蜡封
// 记在目录里"——没有逐条校验和，这一卷就是一段无法核对的字节。

/// 活动段写满多少字节就封口。
pub(super) const SEGMENT_ROLL_BYTES: u64 = 1024 * 1024;
/// 封口段放在这个子目录里（环境根只留活动段与目录）。
pub(super) const PARTS_DIR: &str = "trace.parts";
/// 目录：一行一卷，按封口顺序追加。
pub(super) const PARTS_INDEX: &str = "trace.parts.idx";

/// 一卷**封口的**经历。它不可变：封口之后再往它里面写是不允许的。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedPart {
    /// 段文件名（`<首序号 12 位>.ndjson`）。
    pub file: String,
    /// 这一卷覆盖的序号区间（闭区间）。
    pub seq_start: u64,
    pub seq_end: u64,
    /// 这一卷的字节数（封口那一刻记下的）。
    pub bytes: u64,
    /// 这一卷结束时的**链值**：下一卷（或活动段）从它续起。
    pub chain: u32,
    /// 这一卷里的**全部活动**：身份 ＋ 两个折好的标志（见 [`RUN_STARTED`] / [`RUN_SEALED`]）。
    ///
    /// ⚠ **折给开机用**（2026-10-06 第二刀）：`bind` 要建"已登记/已封存"两张表，而"这一段在不
    /// 在这一卷里"（[`SealedPart::holds`]）也要它——**它必须是全部活动**，不只是有标志的那些
    /// （一段被打断的活动可能两个标志都没有）。
    ///
    /// 它躺在目录的**骨架行**上，形状是"一串短字符串 ＋ 一串小整数"，因此开机解它很便宜；
    /// 完整摘要（[`SealedPart::runs`]）躺在**另一行**上，只有要它的路才解。
    pub runs_folded: Vec<(RunId, u8)>,
    /// 这一卷里的条数（折给开机算"已确认条数"）。
    pub records: usize,
    /// 这一卷的活动摘要（**完整那一份**：列表与跨存储判据都从它算）。
    ///
    /// ⚠ 开机**不**解它（那是这一刀省下的钱）：只有 [`summaries_at`] / [`health_at`] 这些要
    /// "一份完整读数"的路才用 [`with_part_runs`] 把它填上。开机之前它是空的。
    pub runs: Vec<RunSummary>,
    /// **折到这一卷末尾**的"历史上最后一次存储故障"。
    ///
    /// 目录就是**检查点**：开机只读它 ＋ 活动段，因此这两格必须在这里折好——
    /// 否则"主体上次为什么停"就得重读全部旧卷才答得出来。
    pub paused: Option<StorageFailure>,
    /// 折到这一卷末尾的"没有收尾的活动"（`None` = 到这一卷末尾为止没有）。
    pub unfinished: Option<RunId>,
    /// 折到这一卷末尾的"最后一次显式确认"的序号：下一卷继续用它判"确认之后"。
    pub acknowledged_through: Option<u64>,
}

/// [`SealedPart::runs_folded`] 那一个小整数的两个位：**登记过开始**。
pub(super) const RUN_STARTED: u8 = 1;
/// [`SealedPart::runs_folded`] 那一个小整数的两个位：**封存过**。
pub(super) const RUN_SEALED: u8 = 2;

impl SealedPart {
    /// 这一卷的盘上路径。
    pub(super) fn path(&self, root: &Path) -> PathBuf {
        root.join(PARTS_DIR).join(&self.file)
    }

    /// 这一卷里有没有这个活动（**只看折好的身份**，不需要完整摘要）。
    pub(super) fn holds(&self, run: &RunId) -> bool {
        self.runs_folded.iter().any(|(id, _)| id == run)
    }
}

/// 段文件名：`<首序号 12 位>.ndjson`（自描述、可排序）。
pub(super) fn part_file_name(seq_start: u64) -> String {
    format!("{seq_start:012}.ndjson")
}

/// 一卷的**骨架行**：段名 / 序号区间 / 字节数 / 链值 / 折好的活动与条数 / 折好的准入读数。
///
/// ⚠ **完整摘要不在这一行上**（见 [`encode_part_runs`]）：开机只解这一行，而那一串八字段的
/// 摘要对象是 O(全部活动) 的解析——它随历史变慢，正是这一刀要拿掉的那一项。
fn encode_part(part: &SealedPart) -> String {
    let folded: Vec<serde_json::Value> = part
        .runs_folded
        .iter()
        .map(|(run, flags)| serde_json::json!([run.as_str(), flags]))
        .collect();
    let paused = part.paused.as_ref().map(|failure| {
        serde_json::json!({
            "run": failure.run.as_str(),
            "reason": failure.reason,
            "acknowledged": failure.acknowledged,
        })
    });
    let object = serde_json::json!({
        "kind": "part",
        "file": part.file,
        "seq-start": part.seq_start,
        "seq-end": part.seq_end,
        "bytes": part.bytes,
        "chain": format!("{:08x}", part.chain),
        "records": part.records,
        "runs": folded,
        "paused": paused,
        "unfinished": part.unfinished.as_ref().map(|run| run.as_str()),
        "acknowledged-through": part.acknowledged_through,
    });
    let mut line = object.to_string();
    line.push('\n');
    line
}

/// 一卷的**摘要行**：这一卷的活动摘要（完整那一份）。
///
/// 与骨架行**分开一行**，因为要它的路（`list_runs` / 健康自检）与不要它的路（开机）不是同一条：
/// 混在一行里，开机就得付 O(全部活动) 的解析钱。两行**同一次整份重写**写出，因此不会各写一半。
fn encode_part_runs(part: &SealedPart) -> String {
    let runs: Vec<serde_json::Value> = part
        .runs
        .iter()
        .map(|summary| {
            serde_json::json!({
                "run": summary.run.as_str(),
                "records": summary.records,
                "first-seq": summary.first_seq,
                "last-seq": summary.last_seq,
                "ending": summary.ending,
                "failure": summary.failure,
                "sealed": summary.sealed,
                "started": summary.started,
            })
        })
        .collect();
    let object = serde_json::json!({
        "kind": "runs",
        "file": part.file,
        "runs": runs,
    });
    let mut line = object.to_string();
    line.push('\n');
    line
}

/// 解一行**骨架**。读不出来 ⇒ `None`（调用方据此**重建**目录，而不是猜）。
pub(super) fn decode_part(line: &str) -> Option<SealedPart> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("kind")?.as_str()? != "part" {
        return None;
    }
    let runs_folded = value
        .get("runs")?
        .as_array()?
        .iter()
        .map(|row| {
            let pair = row.as_array()?;
            let flags = pair.get(1)?.as_u64()?;
            // 标志是**封闭的两位**：认不出的取值只能通向一句猜的话 ⇒ 拒绝（交给重建）。
            if flags > u64::from(RUN_STARTED | RUN_SEALED) {
                return None;
            }
            Some((RunId::new(pair.first()?.as_str()?), flags as u8))
        })
        .collect::<Option<Vec<(RunId, u8)>>>()?;
    let paused = match value.get("paused").and_then(|paused| paused.as_object()) {
        Some(row) => {
            let reason = row
                .get("reason")
                .and_then(|reason| reason.as_str())
                .map(str::to_string);
            let acknowledged = row.get("acknowledged")?.as_bool()?;
            // ⚠ **"没有原因、又没有被确认"这个组合没有任何含义，因此整行读不出来**（2026-10-06 收）。
            //
            // 原因缺席只有**一种**已定义的含义：账本里只有"确认"、没有它确认的那条暂停
            // （见 [`StorageFailure::reason`]）——而那种形状里 `acknowledged` **必定为真**。
            // 反过来说，"还停着"却不说为什么，是一句谁也解释不了的话。
            //
            // 从前它被收下，然后在渲染端折成空串：`环境已暂停（）：…` ——**一对空括号**，
            // 而系统明明知道"原因没有被记录"这条事实。那正是"**能解析但无法解释**"该被拒绝的
            // 形状（同 [`StoreError::MalformedEntry`] 那条纪律）。
            //
            // 拒绝 ⇒ 整份目录不可信 ⇒ 由**卷**重建（卷是权威，目录只是索引）：重建会把整卷从头
            // 核一遍，因此暂停与它的原因**从记录里重新算出来**——错的是索引，真相照旧回得来。
            //
            // 两侧都成立才算数：**记录侧**唯一的产者 `last_storage_failure_of` 产不出这个组合
            // （`Paused(text)` 必带原因、`Acknowledged` 必把确认翻过来），**封口段**的暂停又继承
            // 自同一个函数 ⇒ 编码端写不出它；于是这一行只可能来自手改或别的构建写的目录。
            if reason.is_none() && !acknowledged {
                return None;
            }
            Some(StorageFailure {
                run: RunId::new(row.get("run")?.as_str()?),
                reason,
                acknowledged,
            })
        }
        None => None,
    };
    Some(SealedPart {
        file: value.get("file")?.as_str()?.to_string(),
        seq_start: value.get("seq-start")?.as_u64()?,
        seq_end: value.get("seq-end")?.as_u64()?,
        bytes: value.get("bytes")?.as_u64()?,
        chain: u32::from_str_radix(value.get("chain")?.as_str()?, 16).ok()?,
        runs_folded,
        records: value.get("records")?.as_u64()? as usize,
        // 摘要不在骨架行上：要它的路用 [`with_part_runs`] 填。
        runs: Vec::new(),
        paused,
        unfinished: value
            .get("unfinished")
            .and_then(|run| run.as_str())
            .map(RunId::new),
        acknowledged_through: value
            .get("acknowledged-through")
            .and_then(|seq| seq.as_u64()),
    })
}

/// 盘上现存的封口段（按首序号排序）。
pub(super) fn part_files(root: &Path) -> Vec<(u64, PathBuf)> {
    let mut found: Vec<(u64, PathBuf)> = Vec::new();
    let Ok(entries) = fs::read_dir(root.join(PARTS_DIR)) else {
        return found;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".ndjson") else {
            continue;
        };
        let Ok(seq_start) = stem.parse::<u64>() else {
            continue;
        };
        found.push((seq_start, entry.path()));
    }
    found.sort_by_key(|(seq_start, _)| *seq_start);
    found
}

/// 读目录（**宽容一条尾巴**：最后一行没写完就当它没有——那一卷会被重建路径补上）。
///
/// 目录是**派生**的：它与盘上的卷对不上时，唯一的正解是**重读所有卷重建一次**
/// （见 [`load_parts`]），不是猜、也不是拒绝——主体的记忆在卷里，不在目录里。
///
/// 返回（骨架，**摘要行**的行数）；`None` = **这份目录不可信**（读不出来、或形状不是本构建写的），
/// 调用方据此**重建**。
///
/// ⚠ **任何一行读不出来都只是"重建"**（不是拒绝打开）：目录是**派生**的，而它的形状会随构建变
/// （这一刀就把完整摘要从骨架行挪到了摘要行上）——把它读成"环境坏了"，等于让一份派生文件**锁死**
/// 主体的记忆。卷是权威，重读一遍就什么都有了；真损坏会在重建那一步如实报出来。
///
/// 摘要行的行数要跟骨架对上：**每一卷都该有一条**——对不上说明这份目录不是本构建写下的形状
/// （例如早先那种把摘要塞在骨架行里的格式），或者尾部半行吃掉了它 ⇒ 同样重建。这一格只需数行，
/// **不必解**摘要。
fn read_parts_index(root: &Path) -> Result<Option<(Vec<SealedPart>, usize)>, StoreError> {
    let path = root.join(PARTS_INDEX);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Some((Vec::new(), 0)));
        }
        Err(error) => return Err(io_error(error)),
    };
    let mut parts = Vec::new();
    let mut summaries = 0usize;
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // 摘要行**不解**（那正是开机要省的钱）：按它自己的种类名跳过。
        if line.contains(RUNS_LINE_MARK) {
            summaries += 1;
            continue;
        }
        let Some(part) = decode_part(line) else {
            return Ok(None);
        };
        parts.push(part);
    }
    parts.sort_by_key(|part| part.seq_start);
    Ok(Some((parts, summaries)))
}

/// 摘要行上那个**只用于跳过**的记号（见 [`read_parts_index`]）。
pub(super) const RUNS_LINE_MARK: &str = "\"kind\":\"runs\"";

/// 打开时取目录：与盘上的卷**逐一对应**、而且**每卷都带着自己的摘要行**才用它，否则重读所有卷
/// 重建一次（慢一次，之后照旧）。
pub(super) fn load_parts(root: &Path) -> Result<Vec<SealedPart>, StoreError> {
    let Some((indexed, summaries)) = read_parts_index(root)? else {
        return rebuild_parts(root);
    };
    let on_disk = part_files(root);
    let matches = summaries == indexed.len()
        && indexed.len() == on_disk.len()
        && indexed
            .iter()
            .zip(on_disk.iter())
            .all(|(part, (seq_start, path))| {
                part.seq_start == *seq_start && part.path(root) == *path
            });
    if matches {
        return Ok(indexed);
    }
    rebuild_parts(root)
}

/// 把各卷的**完整摘要**填上（目录的摘要行）。
///
/// 只有要"一份完整读数"的路用它（[`summaries_at`] / [`health_at`]）：开机**不**走这一趟。
/// 摘要行读不出来、或某一卷没有摘要行 ⇒ 目录不可信 ⇒ 重读所有卷**重建**一次（同一条规矩）。
pub(super) fn with_part_runs(
    root: &Path,
    parts: Vec<SealedPart>,
) -> Result<Vec<SealedPart>, StoreError> {
    // 一卷都没有：没有摘要要填，**一个字节都不写**（这一条是只读的路，见 `environment_diagnostics`
    // 那条"命令不写东西"的用例）。
    if parts.is_empty() {
        return Ok(parts);
    }
    let Ok(text) = fs::read_to_string(root.join(PARTS_INDEX)) else {
        return rebuild_parts(root);
    };
    let mut by_file: BTreeMap<String, Vec<RunSummary>> = BTreeMap::new();
    for line in text.lines() {
        if !line.contains(RUNS_LINE_MARK) {
            continue;
        }
        let Some((file, runs)) = decode_part_runs(line) else {
            return rebuild_parts(root);
        };
        by_file.insert(file, runs);
    }
    let mut filled = Vec::with_capacity(parts.len());
    for mut part in parts {
        match by_file.remove(&part.file) {
            Some(runs) => {
                part.runs = runs;
                filled.push(part);
            }
            None => return rebuild_parts(root),
        }
    }
    Ok(filled)
}

/// 解一行**摘要**：返回（段名，这一卷的活动摘要）。读不出来 ⇒ `None`（交给重建）。
fn decode_part_runs(line: &str) -> Option<(String, Vec<RunSummary>)> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("kind")?.as_str()? != "runs" {
        return None;
    }
    let runs = value
        .get("runs")?
        .as_array()?
        .iter()
        .map(|row| {
            Some(RunSummary {
                run: RunId::new(row.get("run")?.as_str()?),
                records: row.get("records")?.as_u64()? as usize,
                first_seq: row.get("first-seq")?.as_u64()?,
                last_seq: row.get("last-seq")?.as_u64()?,
                ending: row
                    .get("ending")
                    .and_then(|ending| ending.as_str())
                    .map(str::to_string),
                failure: row
                    .get("failure")
                    .and_then(|failure| failure.as_str())
                    .map(str::to_string),
                sealed: row.get("sealed")?.as_bool()?,
                started: row.get("started")?.as_bool()?,
            })
        })
        .collect::<Option<Vec<RunSummary>>>()?;
    Some((value.get("file")?.as_str()?.to_string(), runs))
}

/// 重读所有封口段，把目录重算出来（卷是权威，目录只是索引）。
fn rebuild_parts(root: &Path) -> Result<Vec<SealedPart>, StoreError> {
    let mut parts: Vec<SealedPart> = Vec::new();
    let mut chain = CHAIN_SEED;
    let facts = read_facts(root, TailPolicy::RequireConfirmed)?;
    for (seq_start, path) in part_files(root) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (records, next_chain) = read_trace_file(&path, TailPolicy::RequireConfirmed, chain)?;
        let bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        let runs = summaries_of(&records)?;
        let acknowledged_through = last_acknowledged_seq(&records)
            .or(parts.last().and_then(|part| part.acknowledged_through));
        let paused = last_storage_failure_of(&records)?
            .or_else(|| parts.last().and_then(|part| part.paused.clone()));
        let unfinished = unfinished_after(
            &records,
            acknowledged_through,
            parts.last().and_then(|part| part.unfinished.clone()),
        );
        parts.push(SealedPart {
            file: name,
            seq_start,
            seq_end: records
                .last()
                .map(|record| record.seq())
                .unwrap_or(seq_start),
            bytes,
            chain: next_chain,
            runs_folded: folded_of(&runs),
            records: records.len(),
            runs,
            paused,
            unfinished,
            acknowledged_through,
        });
        chain = next_chain;
    }
    // 重建出来的目录里，跨存储判据也要能算：这里只做**结构**核对，判据本身在读的时候算。
    let _ = facts;
    write_parts_index(root, &parts)?;
    Ok(parts)
}

/// 把完整摘要折成骨架行要的那一串（身份 ＋ 两个标志）。
pub(super) fn folded_of(runs: &[RunSummary]) -> Vec<(RunId, u8)> {
    runs.iter()
        .map(|summary| {
            let mut flags = 0u8;
            if summary.started {
                flags |= RUN_STARTED;
            }
            if summary.sealed {
                flags |= RUN_SEALED;
            }
            (summary.run.clone(), flags)
        })
        .collect()
}

/// 把目录**整份重写**（封口时追加；重建时整份写）：每一卷**两行**——骨架行 ＋ 摘要行。
pub(super) fn write_parts_index(root: &Path, parts: &[SealedPart]) -> Result<(), StoreError> {
    fs::create_dir_all(root.join(PARTS_DIR)).map_err(io_error)?;
    let mut text = String::new();
    for part in parts {
        text.push_str(&encode_part(part));
        text.push_str(&encode_part_runs(part));
    }
    let path = root.join(PARTS_INDEX);
    let temporary = root.join(format!("{PARTS_INDEX}.new"));
    fs::write(&temporary, text.as_bytes()).map_err(io_error)?;
    // 原子换名：目录要么是旧的、要么是新的，不会半份。
    fs::rename(&temporary, &path).map_err(io_error)?;
    sync_directory(root);
    Ok(())
}

// ---------------------------------------------------------------------------
// 卷内偏移索引：每卷一份 `<卷名>.idx`，按**活动**记边界（2026-10-06，市面标准形态第三步）
// ---------------------------------------------------------------------------
//
// 判据（README §4 那一行，"卷内偏移索引"）：**读一段旧活动不该读整卷**。目录（上一刀）已经把
// 成本封顶在一卷上，但"一卷"仍要整份读进来再筛；而一段活动只占其中约 31 KB。同族做法：
// Kafka 的 `.index` 就是"偏移 → 字节位置"的**稀疏**索引；这里按**活动**记边界，比稀疏字节索引
// 更省也更准（一段活动就是一次要读的全部）。
//
// **链值是这一设计的关键**：每条区间带上"**区间起点**的链值"，于是读一个切片可以**独立校验**
// （从那个链值起逐行核校验和），不必从头折叠整卷——否则省下的时间又花回校验上。
//
// **它是派生的**：与卷对不上（丢了、半行、链值不对、区间没对准行）⇒ 重读该卷重建。卷是权威。

/// 一卷里**一段活动**占的字节区间（`<卷名>.idx` 的一行）。
///
/// `first` 是这一段第一条记录的起始字节，`end` 是它最后一条记录的**末尾之后**（开区间）——
/// 因此 `[first, end)` 一定装着这一段的行，也可能顺带装着**夹在中间**的别人的行（环境级标记
/// 会在活动封口之后才落盘）。读的人照旧按活动身份再筛一次，因此正确性**不依赖**"这一段在卷里
/// 连续"；不连续时区间只是个更大的超集，读得多一点、不会读错。
///
/// `chain` 是**区间起点**的链值（这一段第一条记录之前的链）。
pub(crate) struct RunSpan {
    pub(super) run: RunId,
    pub(super) first: u64,
    pub(super) end: u64,
    pub(super) chain: u32,
}

/// 一卷的偏移索引文件名：`<卷名>.idx`（贴着卷放，一起搬、一起删）。
fn part_index_name(part: &SealedPart) -> String {
    format!("{}.idx", part.file)
}

/// 一卷的偏移索引路径。
fn part_index_path(root: &Path, part: &SealedPart) -> PathBuf {
    root.join(PARTS_DIR).join(part_index_name(part))
}

/// 索引的一行（与存储记录同样是**追加**的 JSON 行）。
fn encode_span(span: &RunSpan) -> String {
    let object = serde_json::json!({
        "kind": "span",
        "run": span.run.as_str(),
        "first-offset": span.first,
        "end-offset": span.end,
        "chain-at-start": format!("{:08x}", span.chain),
    });
    let mut line = object.to_string();
    line.push('\n');
    line
}

/// 解一行索引。读不出来 ⇒ `None`（调用方据此**重建**，而不是猜）。
fn decode_span(line: &str) -> Option<RunSpan> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("kind")?.as_str()? != "span" {
        return None;
    }
    let span = RunSpan {
        run: RunId::new(value.get("run")?.as_str()?),
        first: value.get("first-offset")?.as_u64()?,
        end: value.get("end-offset")?.as_u64()?,
        chain: u32::from_str_radix(value.get("chain-at-start")?.as_str()?, 16).ok()?,
    };
    // 区间必须**非空且向前**：反过来的区间不可能是本写入者算出来的。
    if span.end <= span.first {
        return None;
    }
    Some(span)
}

/// 读一卷的偏移索引。
///
/// **尾部半行不算损坏**（封口那一刻崩在写索引中间就是这一形状）：那半行丢掉、前面照用——
/// 被丢掉的那个区间由"读不到它 ⇒ 重建"那条路补上，因此它**自愈**，不需要谁去修。
/// 中间的行读不出来则是另一回事：整份索引都不可信 ⇒ `None`（交给重建）。
pub(super) fn read_part_index(
    root: &Path,
    part: &SealedPart,
) -> Result<Option<Vec<RunSpan>>, StoreError> {
    let path = part_index_path(root, part);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut spans = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match decode_span(line) {
            Some(span) => spans.push(span),
            // 只有**最后一行**可以当"没写完"；中间的不行。
            None if index + 1 == lines.len() => break,
            None => return Ok(None),
        }
    }
    Ok(Some(spans))
}

/// 写一卷的偏移索引（整份写、原子换名：索引要么是旧的、要么是新的，不会半份）。
pub(super) fn write_part_index(
    root: &Path,
    part: &SealedPart,
    spans: &[RunSpan],
) -> Result<(), StoreError> {
    let directory = root.join(PARTS_DIR);
    fs::create_dir_all(&directory).map_err(io_error)?;
    let mut text = String::new();
    for span in spans {
        text.push_str(&encode_span(span));
    }
    let path = part_index_path(root, part);
    let temporary = directory.join(format!("{}.new", part_index_name(part)));
    fs::write(&temporary, text.as_bytes()).map_err(io_error)?;
    fs::rename(&temporary, &path).map_err(io_error)?;
    sync_directory(&directory);
    Ok(())
}

/// 重读**这一卷**，把它的偏移索引重算出来（卷是权威，索引只是索引）。
///
/// 顺带把**整卷的记录**还给调用方：会走到这里的人手上正好缺这一段（索引对不上），而整卷刚在
/// 这里从头核过一遍，不必再读第二遍。
///
/// 链值从**目录**里拿（"这一卷起点"＝它前面那些卷封口时的链值）：这一趟本来就要重读整卷，
/// 多读一次目录不算什么；而**快路不读目录**，正是这一刀要省的钱。目录一定列着这一卷——
/// [`load_parts`] 的核对就是"目录与盘上的卷逐一对应"，而封口段从不被删除。
pub(super) fn rebuild_part_index(
    root: &Path,
    part: &SealedPart,
) -> Result<(Vec<RunSpan>, Vec<TraceRecord>), StoreError> {
    let chain_start = load_parts(root)?
        .iter()
        .take_while(|candidate| candidate.file != part.file)
        .fold(CHAIN_SEED, |_chain, candidate| candidate.chain);
    let (records, _, spans) =
        read_trace_file_detailed(&part.path(root), TailPolicy::RequireConfirmed, chain_start)?;
    write_part_index(root, part, &spans)?;
    Ok((spans, records))
}

/// 只读**一个字节区间**：从 `span.first` seek 过去，读 `end - first` 个字节，链从 `span.chain` 起。
///
/// 区间没对准行、或这一刀里有任何一行读不出来 ⇒ `Ok(None)`：**这一刀不算数**，调用方据此读作
/// "索引与卷对不上"并重读该卷重建（见 [`part_run_records`]）。这里**不吞错**：重建会把整卷从头
/// 核一遍，真损坏会在那一步如实报出来。只有真正的 I/O 失败才返回 `Err`。
pub(super) fn read_trace_slice(
    path: &Path,
    span: &RunSpan,
    run: &RunId,
) -> Result<Option<Vec<TraceRecord>>, StoreError> {
    if span.end <= span.first {
        return Ok(None);
    }
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) => return Err(io_error(error)),
    };
    let mut bytes = vec![0u8; (span.end - span.first) as usize];
    let read = file
        .seek(SeekFrom::Start(span.first))
        .and_then(|_| file.read_exact(&mut bytes));
    match read {
        Ok(()) => {}
        // 区间伸到文件末尾之外 ⇒ 索引比卷长：这一刀不算数（重建）。
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(io_error(error)),
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return Ok(None);
    };
    // 区间必须**整行对齐**：以换行结尾，且每一段都不是空的（空行不是本写入者的形状）。
    let Some(body) = text.strip_suffix('\n') else {
        return Ok(None);
    };
    if body.is_empty() {
        return Ok(None);
    }
    let mut records = Vec::new();
    let mut chain = span.chain;
    for (index, raw) in body.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.is_empty() {
            return Ok(None);
        }
        let Ok((line_body, expected)) = verified_line(chain, line, index) else {
            return Ok(None);
        };
        let Ok(record) = decode_trace_line(line_body, index, run) else {
            return Ok(None);
        };
        if let Some(record) = record {
            records.push(record);
        }
        chain = expected;
    }
    Ok(Some(records))
}

// ---------------------------------------------------------------------------
// 事实侧分卷：同一套形状，另一份存储（2026-10-06，市面标准形态第二步的对称那一半）
// ---------------------------------------------------------------------------
//
// 判据（**读数说话**）：**开机那一拍不该随全部事实线性**。事实侧从前整份读，而它随每一次行动
// 增长——实测 1200 段活动上 `bind` 的 163 ms 里 **139 ms（85%）**花在把 1.19 MB 事实整份解析
// 上；家的只读状态口（每 15 秒一次）同样付 131 ms。形状与经历那一刀**完全同形**（同一族做法：
// 段 ＋ 目录 ＋ 检查点 ＋ 链续起），差别只有两点：
//
// - 事实**没有**卷内偏移索引：今天没有"按活动读事实"的生产消费者（`read_run_facts` 全仓没有
//   调用点），因此每卷只记骨架；
// - 事实目录**不折准入读数**："暂停""没有收尾""最后一次确认"都是**经历侧**的概念。

/// 事实的活动段写满多少字节就封口。
///
/// 判据与经历那条**同一条**：**开机只读活动段**，所以这个数就是"开机要读多少"的上限。事实约
/// **1 KB 一段活动**（实测 1200 段活动 ⇒ 1.19 MB），因此 **128 KiB ≈ 130 段活动**；按 100 段/天
/// 算，一天不到一个卷、一年约 300 个（比经历侧一年约千个还少）。
pub(super) const FACTS_ROLL_BYTES: u64 = 128 * 1024;
/// 事实的封口段放在这个子目录里。
pub(super) const FACTS_PARTS_DIR: &str = "facts.parts";
/// 事实的目录：一行一卷，按封口顺序追加。
const FACTS_PARTS_INDEX: &str = "facts.parts.idx";

/// 一卷**封口的**事实。它不可变：封口之后再往它里面写是不允许的。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedFactPart {
    /// 段文件名（`<首序号 12 位>.ndjson`，与经历侧同一套命名）。
    pub file: String,
    /// 这一卷覆盖的序号区间（闭区间）。
    pub seq_start: u64,
    pub seq_end: u64,
    /// 这一卷里的条数：开机用它算"已确认条数"，**不必重读这一卷**。
    pub records: usize,
    /// 这一卷的字节数（封口那一刻记下的）。
    pub bytes: u64,
    /// 这一卷结束时的**链值**：下一卷（或活动段）从它续起。
    pub chain: u32,
}

impl SealedFactPart {
    /// 这一卷的盘上路径。
    pub(super) fn path(&self, root: &Path) -> PathBuf {
        root.join(FACTS_PARTS_DIR).join(&self.file)
    }
}

/// 一卷事实的目录行（一行一个 JSON 对象；与存储记录同样是**追加**的）。
fn encode_fact_part(part: &SealedFactPart) -> String {
    let object = serde_json::json!({
        "kind": "fact-part",
        "file": part.file,
        "seq-start": part.seq_start,
        "seq-end": part.seq_end,
        "records": part.records,
        "bytes": part.bytes,
        "chain": format!("{:08x}", part.chain),
    });
    let mut line = object.to_string();
    line.push('\n');
    line
}

/// 解一行事实目录。读不出来 ⇒ `None`（调用方据此**重建**，而不是猜）。
fn decode_fact_part(line: &str) -> Option<SealedFactPart> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    if value.get("kind")?.as_str()? != "fact-part" {
        return None;
    }
    Some(SealedFactPart {
        file: value.get("file")?.as_str()?.to_string(),
        seq_start: value.get("seq-start")?.as_u64()?,
        seq_end: value.get("seq-end")?.as_u64()?,
        records: value.get("records")?.as_u64()? as usize,
        bytes: value.get("bytes")?.as_u64()?,
        chain: u32::from_str_radix(value.get("chain")?.as_str()?, 16).ok()?,
    })
}

/// 盘上现存的事实封口段（按首序号排序）。
fn fact_part_files(root: &Path) -> Vec<(u64, PathBuf)> {
    let mut found: Vec<(u64, PathBuf)> = Vec::new();
    let Ok(entries) = fs::read_dir(root.join(FACTS_PARTS_DIR)) else {
        return found;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".ndjson") else {
            continue;
        };
        let Ok(seq_start) = stem.parse::<u64>() else {
            continue;
        };
        found.push((seq_start, entry.path()));
    }
    found.sort_by_key(|(seq_start, _)| *seq_start);
    found
}

/// 读事实目录（**宽容一条尾巴**：最后一行没写完就当它没有——那一卷会被重建路径补上）。
///
/// 与经历目录同一条规矩：目录是**派生**的，与盘上的卷对不上时唯一的正解是**重读所有卷重建**
/// （见 [`load_fact_parts`]），不是猜、也不是拒绝——事实在卷里，不在目录里。
fn read_facts_index(root: &Path) -> Result<Vec<SealedFactPart>, StoreError> {
    let path = root.join(FACTS_PARTS_INDEX);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error(error)),
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut parts = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match decode_fact_part(line) {
            Some(part) => parts.push(part),
            // 读不出来：只有**最后一行**可以当"没写完"（封口那一刻崩了）；中间的不行。
            None if index + 1 == lines.len() => {}
            None => {
                return Err(StoreError::MalformedEntry {
                    seq: parts.len() as u64,
                });
            }
        }
    }
    parts.sort_by_key(|part| part.seq_start);
    Ok(parts)
}

/// 打开时取事实目录：与盘上的卷**逐一对应**才用它，否则重读所有卷重建一次（慢一次，之后照旧）。
pub(super) fn load_fact_parts(root: &Path) -> Result<Vec<SealedFactPart>, StoreError> {
    let indexed = read_facts_index(root)?;
    let on_disk = fact_part_files(root);
    let matches = indexed.len() == on_disk.len()
        && indexed
            .iter()
            .zip(on_disk.iter())
            .all(|(part, (seq_start, path))| {
                part.seq_start == *seq_start && part.path(root) == *path
            });
    if matches {
        return Ok(indexed);
    }
    rebuild_fact_parts(root)
}

/// 重读所有事实封口段，把事实目录重算出来（卷是权威，目录只是索引）。
fn rebuild_fact_parts(root: &Path) -> Result<Vec<SealedFactPart>, StoreError> {
    let mut parts: Vec<SealedFactPart> = Vec::new();
    let mut chain = CHAIN_SEED;
    for (seq_start, path) in fact_part_files(root) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (rows, next_chain) = read_fact_file(&path, TailPolicy::RequireConfirmed, chain)?;
        let bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        parts.push(SealedFactPart {
            file: name,
            seq_start,
            seq_end: rows.last().map(|row| row.seq()).unwrap_or(seq_start),
            records: rows.len(),
            bytes,
            chain: next_chain,
        });
        chain = next_chain;
    }
    write_facts_index(root, &parts)?;
    Ok(parts)
}

/// 把事实目录**整份重写**（封口时追加一行；重建时整份写）。
pub(super) fn write_facts_index(root: &Path, parts: &[SealedFactPart]) -> Result<(), StoreError> {
    fs::create_dir_all(root.join(FACTS_PARTS_DIR)).map_err(io_error)?;
    let mut text = String::new();
    for part in parts {
        text.push_str(&encode_fact_part(part));
    }
    let path = root.join(FACTS_PARTS_INDEX);
    let temporary = root.join(format!("{FACTS_PARTS_INDEX}.new"));
    fs::write(&temporary, text.as_bytes()).map_err(io_error)?;
    // 原子换名：目录要么是旧的、要么是新的，不会半份。
    fs::rename(&temporary, &path).map_err(io_error)?;
    sync_directory(root);
    Ok(())
}

/// "没有收尾的活动"折到下一卷：本卷里出现的收场要把它清掉，否则它继续带着。
pub(super) fn unfinished_after(
    records: &[TraceRecord],
    acknowledged_through: Option<u64>,
    carried: Option<RunId>,
) -> Option<RunId> {
    let ended: std::collections::BTreeSet<RunId> = records
        .iter()
        .filter(|record| is_run_ending(record))
        .map(|record| record.run().clone())
        .collect();
    let carried = carried.filter(|run| !ended.contains(run));
    carried.or_else(|| unfinished_run_after(records, acknowledged_through))
}

/// 按**环境目录**读全部经历（只读；读法见 [`reading_tail`]）。
pub fn read_trace_at(root: &Path) -> Result<Vec<TraceRecord>, StoreError> {
    read_trace(root, reading_tail(root))
}

/// 按环境目录算出各活动的机械摘要。
pub(super) fn summaries_at(root: &Path, tail: TailPolicy) -> Result<Vec<RunSummary>, StoreError> {
    let parts = load_parts(root)?;
    let chain_start = parts.last().map(|part| part.chain).unwrap_or(CHAIN_SEED);
    let (records, _) = read_trace_file(&root.join(TRACE_FILE), tail, chain_start)?;
    // 旧卷那一半从目录的**摘要行**里取（开机不解它，这一条路要）。
    let parts = with_part_runs(root, parts)?;
    let mut runs: Vec<RunSummary> = parts
        .iter()
        .flat_map(|part| part.runs.iter().cloned())
        .collect();
    runs.extend(summaries_of(&records)?);
    // ⚠ **同一个活动只出现一次**：横跨两卷 ＋ 活动段时，各卷给的是**局部**摘要，必须合并成整段
    // （见 [`merge_summaries`]）。
    Ok(merge_summaries(runs))
}

/// 把同一活动的多份**局部**摘要合并成一份（跨卷 ＋ 活动段的读数合成整段）。
///
/// ⚠ **为什么必须有它**（2026-10-06 查出来的真形状）：封口按**字节**触发，一段活动可以横跨两卷
/// ＋ 活动段，于是同一个活动会有好几份局部摘要。从前它们被直接拼起来 ⇒ 同一个地址在列表里出现
/// 好几次，而且每一份都是**半截**读数（条数只有那一卷的、`started` 只出现在第一份、`ending` 只
/// 出现在最后一份）。**活动是主体认得的地址**（`history/<活动>/`）：它必须只出现一次，而且读数
/// 是**整段**的。
///
/// 合并规则：条数相加、序号区间取并、收场与失败分类取**最后一份有的**（它们一旦写下就不再改）、
/// 两个标志取或。
pub(super) fn merge_summaries(summaries: Vec<RunSummary>) -> Vec<RunSummary> {
    let mut merged: Vec<RunSummary> = Vec::new();
    let mut slot_of: BTreeMap<String, usize> = BTreeMap::new();
    for summary in summaries {
        match slot_of.get(summary.run.as_str()) {
            Some(slot) => {
                let into = &mut merged[*slot];
                into.records += summary.records;
                into.first_seq = into.first_seq.min(summary.first_seq);
                into.last_seq = into.last_seq.max(summary.last_seq);
                if summary.ending.is_some() {
                    into.ending = summary.ending;
                }
                if summary.failure.is_some() {
                    into.failure = summary.failure;
                }
                into.sealed |= summary.sealed;
                into.started |= summary.started;
            }
            None => {
                slot_of.insert(summary.run.as_str().to_string(), merged.len());
                merged.push(summary);
            }
        }
    }
    merged
}

/// 各活动的机械摘要（**从已经解析过的记录算**）。
///
/// 拆出这一层与 [`admission_of`] 同一个理由：同一份账本可能被问好几格读数，各读一遍就是
/// 把整份账本白读一遍（真机读数见 `tests/durability_reading.rs`：健康自检一度是打开的四倍，
/// 因为它在一**次**调用里把同一份文件解析了四遍）。
pub(super) fn summaries_of(trace: &[TraceRecord]) -> Result<Vec<RunSummary>, StoreError> {
    let mut runs: Vec<RunSummary> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();

    for record in trace {
        let key = record.run().as_str().to_string();
        let slot = match index.get(&key) {
            Some(slot) => *slot,
            None => {
                index.insert(key, runs.len());
                runs.push(RunSummary {
                    run: record.run().clone(),
                    records: 0,
                    first_seq: record.seq(),
                    last_seq: record.seq(),
                    ending: None,
                    failure: None,
                    sealed: false,
                    started: false,
                });
                runs.len() - 1
            }
        };
        let summary = &mut runs[slot];
        summary.records += 1;
        summary.last_seq = summary.last_seq.max(record.seq());
        summary.first_seq = summary.first_seq.min(record.seq());
        match record.entry() {
            TracePayload::RunStarted { .. } => summary.started = true,
            TracePayload::Ending {
                subject,
                system,
                failure,
            } => {
                summary.ending = Some(
                    ending_label(subject.as_deref(), system.as_deref())
                        .unwrap_or_else(|| "malformed".to_string()),
                );
                // 分类与结局分开存：`ending` 是"哪一类收场"，`failure` 是"那次失败具体怎么了"。
                summary.failure = failure.clone();
            }
            TracePayload::Sealed => summary.sealed = true,
            _ => {}
        }
    }
    Ok(runs)
}
