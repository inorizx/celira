//! 终端名下那些**活到关家的事**：一本只追加的账 ＋ 一条读回来的版面。
//!
//! ## 它解决什么（2026-10-11 真机读数换来的）
//!
//! 终端的形状是"**一次调用 = 一次动作**"：跑完即散、没有会话、没有后台。于是主体起的东西
//! **下一轮就不存在了**——真机两轮实测：主体起了一个服务，回话时它在听、`parent=(父进程已不在)`；
//! 关家之后它还在听；要找它只能**按端口猜 pid**。裁决（2026-10-11 主人）：**主体起的东西活得
//! 跟住户一样久**，而且**回得来**（看得见、读得到输出、停得掉）。
//!
//! ## 形状：账只追加，版面是折出来的
//!
//! `<环境根>/terminal` 一行一个 JSON 对象，与 `<环境根>/timers`、notes 同款（一条一个对象、
//! `serde` 键就是契约）。两种行：
//!
//! - `start`：主体起了一件要留着的事（名字、原命令、pid、输出文件、时刻）；
//! - `stop`：主体把它停了（名字、时刻）。
//!
//! **只追加**：`stop` 不改写 `start` 那一行，只添一行（判据 22「账只追加，版面可重写」）。
//! 因此今天还剩什么，是**折出来的**（[`Held::entries`]），不是另存一份状态——没有第二份权威。
//!
//! ## 它**不**做的事
//!
//! - **不记"它还活着吗"**：那是读的时候向操作系统问一次的事（`resident_lifetime::process_is_alive`），
//!   不是盘上的事实——把它写进账里就等于把"当时"当成"现在"；
//! - **不收尸**：收口是**住户结束**（作业收容，见 [`crate::resident_lifetime`]）＋ 主体自己的
//!   `terminal_stop`。本件一个进程都不杀（它不是第二个收尸者）；
//! - ⚠ **死条目不会自己走**（2026-10-11 如实登记）：一件起来就崩了，账上它还留着——`state = gone`
//!   是**读数**，不是清账。本件**不自动清**：自动清就是又一条"没人看着也会改事实"的写者，而
//!   这一格要的是"**主体自己决定**什么时候把这一条划掉"（`terminal_stop` 追加一行 `stop` 即可）。
//!   代价如实说：那本账因此是"起过什么的流水"，不是"还活着什么的清单"——**要判断"还剩几件活着"，
//!   读的时候把 `state` 那一格数一遍**（那是读数，不是本件替主体下的结论）；
//! - ⚠ **日志不轮转、不清理**（同一条登记）：一件话多的东西会一直往
//!   `<环境根>/terminal-logs/<名字>.out` 写下去，本件**不动它**（那是主体自己的东西，删不删、
//!   留多久归主体）。今天没有上限、也没有按大小切分——**这是已知未覆盖**，真出现"盘被写满"的
//!   读数时再立；
//! - **不认识**历史、模型、账本、世界。它只持有环境根。

use std::io;
use std::path::{Path, PathBuf};

/// 自持物的文件名：`<环境根>/terminal`（一行一条 JSON 对象）。
pub const HELD_FILE: &str = "terminal";

/// 盘上那两个词（**契约**）：一条账行是哪一种。
pub const KIND_START: &str = "start";
/// 见 [`KIND_START`]。
pub const KIND_STOP: &str = "stop";

/// 名字的长度上限（它要进读数、进停它的那一条命令，不许长到读不动）。
pub const MAX_NAME_BYTES: usize = 60;

/// 一次最多渲染几行（多的如实报，与持有物、纸条板同一惯例）。
pub const MAX_RENDERED: usize = 20;

/// 盘上的一行：**一行一个 JSON 对象**，那几个键就是契约。
///
/// 编解码**手写**（与 timers、notes 同款，只走 `serde_json`）：
///
/// - `kind` / `name` / `at` **必须有且类型对**，否则这一行读不懂（上游如实说 `unknown`）；
/// - `command` / `pid` / `log` 可以缺席（**带默认值的那几个**：`stop` 行本来就没有它们，
///   将来加字段时照这条先例给默认值，因此老的账照旧读得动）；
/// - **多出来的键不看**（那是将来加的字段，不是错误）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldEntry {
    /// 机械身份（主体起它的时候拿到的那个名字）。
    pub name: String,
    /// 主体当时给的那条命令（原文；`stop` 行为空）。
    pub command: String,
    /// 起它的那条命令的直接子进程（`stop` 行为 0）。
    ///
    /// ⚠ 它是**当时**的读数：子进程可能早就退了，而它拉起的东西还活着（那是"脱离"那种形状）。
    /// 所以它只是"往哪儿收"的入口，不是"它还在不在"的答案。
    pub pid: u32,
    /// 输出落在哪儿（`<环境根>/terminal-logs/<名字>.out`；`stop` 行为空）。
    ///
    /// 主体**用已有的文件动词读它**——本件不另造一个"读输出"的动作（同一件事只有一个动作）。
    pub log: String,
    /// 记下这一行的时刻（unix 毫秒）。
    pub at_millis: u64,
    /// 这是 `start` 还是 `stop`。
    pub kind: String,
}

/// 折出来的版面：**今天还剩什么**（按起的先后，老的在前）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    /// 还留着的（`start` 之后没有对应的 `stop`）。
    pub entries: Vec<HeldEntry>,
    /// 已经被主体停掉的条数（如实报，不静默丢）。
    pub stopped: usize,
}

impl Held {
    /// 从账行折出版面。
    ///
    /// **同名后起的覆盖先起的**（主体可以用同一个名字再起一件，那就是"换掉旧的"）——
    /// 判据一句话：**一个名字在任何时刻只指一件事**，否则 `terminal_stop` 就有歧义。
    pub fn fold(lines: &[HeldEntry]) -> Self {
        let mut entries: Vec<HeldEntry> = Vec::new();
        let mut stopped = 0usize;
        for line in lines {
            match line.kind.as_str() {
                KIND_START => {
                    if let Some(index) = entries.iter().position(|each| each.name == line.name) {
                        entries.remove(index);
                    }
                    entries.push(line.clone());
                }
                KIND_STOP => {
                    if let Some(index) = entries.iter().position(|each| each.name == line.name) {
                        entries.remove(index);
                        stopped += 1;
                    }
                }
                // 读不懂的行在 `decode` 那一层就被拒了；走到这里说明词表被绕过（不当默认值）。
                _ => {}
            }
        }
        Held { entries, stopped }
    }
}

/// 主体名下那些活到关家的事（一本账）。
///
/// 它**只**持有环境根（自持物住在那里），与时钟 App、纸条板同款：只把**读回来的那一份**
/// 变成读数与动作。
pub struct HeldThings {
    root: PathBuf,
}

impl HeldThings {
    /// 指向一个环境根。**不做任何 IO**（账可能还不存在；"不存在"是"主体还没起过要留的东西"，
    /// 不是故障）。
    pub fn new(environment_root: &Path) -> Self {
        Self {
            root: environment_root.to_path_buf(),
        }
    }

    /// 账的路径（`<环境根>/terminal`）。
    pub fn path(&self) -> PathBuf {
        self.root.join(HELD_FILE)
    }

    /// 它指向的那个环境根（**只读地交出去**）。
    ///
    /// 谁要它：**手**——`terminal_run` 不传 `cwd` 时在**这里**执行（2026-10-14）。环境根只
    /// 存了**一份**（本件持有），因此不存在"两处各记一个家"的可能；本函数不碰文件系统。
    pub fn environment_root(&self) -> &Path {
        &self.root
    }

    /// 输出落在哪儿（`<环境根>/terminal-logs/<名字>.out`）。
    ///
    /// ⚠ 与账分开放：账是**主体的安排**，输出是**那件事吐出来的东西**，两者的寿命不同
    /// （停掉一件不等于要删掉它吐过的话——主体可能正是为了看那段才停它的）。
    pub fn log_path(&self, name: &str) -> PathBuf {
        self.root.join("terminal-logs").join(format!("{name}.out"))
    }

    /// 打开（并清空）一件的输出文件，交给子进程去写。
    ///
    /// **为什么这一手住在本件里**：输出的落点是本件说的事（[`Self::log_path`]），因此"开它、
    /// 看它多大"这些 IO 也归本件——`terminal_app` 只把拿到的那份交出去，自己不碰文件系统
    /// （源码守卫 `boundary_apis_stay_in_their_own_layer` 钉着这条分层）。
    pub fn open_log(&self, name: &str) -> io::Result<std::fs::File> {
        let path = self.log_path(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::File::create(path)
    }

    /// 那件到现在吐了多少字节（读不到 ⇒ `None`：**"读不到"与"0 字节"是两件事**）。
    pub fn log_bytes(&self, name: &str) -> Option<u64> {
        std::fs::metadata(self.log_path(name))
            .ok()
            .map(|meta| meta.len())
    }

    /// 那件**最近吐出来的那几个字节**（读不到 ⇒ `None`：**"读不到"与"还没说话"是两件事**）。
    ///
    /// 只给尾部：一件东西"起成没起成、是不是在报错"都在最后几行里。⚠ 窗口大小由调用方给
    /// （那是**读数预算**的事，不是本件的），本件只保证一件事：**不切开一个不完整的行**
    /// （半个行会伪造出一条不存在的输出）。
    pub fn log_tail_bytes(&self, name: &str, max_bytes: usize) -> Option<Vec<u8>> {
        let bytes = std::fs::read(self.log_path(name)).ok()?;
        let start = bytes.len().saturating_sub(max_bytes);
        let tail = &bytes[start..];
        match tail.iter().position(|each| *each == b'\n') {
            // 只从窗口里的第一个换行之后开始（窗口起点落在半行中间时，那半行不算数）。
            Some(index) if start > 0 => Some(tail[index + 1..].to_vec()),
            _ => Some(tail.to_vec()),
        }
    }

    /// 读整本账（**没折**：一行一条，按盘上的顺序）。
    ///
    /// **文件不存在 = 主体还没起过** ⇒ 空账；
    /// **读不动 / 有一行读不懂** ⇒ `Err`（上游如实说 `unknown`，绝不冒充"一件都没有"）。
    pub fn lines(&self) -> io::Result<Vec<HeldEntry>> {
        let text = match std::fs::read_to_string(self.path()) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut lines = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match decode(line) {
                Some(entry) => lines.push(entry),
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "terminal ledger row is not readable",
                    ));
                }
            }
        }
        Ok(lines)
    }

    /// 折出来的版面：今天还剩什么。
    pub fn held(&self) -> io::Result<Held> {
        Ok(Held::fold(&self.lines()?))
    }

    /// **追加**一行（起了一件）。
    ///
    /// 只追加：不改写任何既有行（判据 22）。
    pub fn append_start(
        &self,
        name: &str,
        command: &str,
        pid: u32,
        log: &str,
        at_millis: u64,
    ) -> io::Result<()> {
        self.append(&HeldEntry {
            name: name.to_string(),
            command: command.to_string(),
            pid,
            log: log.to_string(),
            at_millis,
            kind: KIND_START.to_string(),
        })
    }

    /// **追加**一行（停了一件）。
    pub fn append_stop(&self, name: &str, at_millis: u64) -> io::Result<()> {
        self.append(&HeldEntry {
            name: name.to_string(),
            command: String::new(),
            pid: 0,
            log: String::new(),
            at_millis,
            kind: KIND_STOP.to_string(),
        })
    }

    fn append(&self, entry: &HeldEntry) -> io::Result<()> {
        use std::io::Write;
        if let Some(parent) = self.path().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path())?;
        writeln!(file, "{}", encode(entry))
    }
}

/// 名字合法吗：非空、不超上限、**不含路径分隔与控制字符**（它要变成一个文件名）。
///
/// 返回 `None` 表示合法；否则返回稳定词（失败分类由调用方那一层定义）。
pub fn name_problem(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        return Some("empty");
    }
    if name.len() > MAX_NAME_BYTES {
        return Some("too-long");
    }
    if name
        .chars()
        .any(|each| each.is_control() || each == '/' || each == '\\' || each == ':')
    {
        return Some("not-a-name");
    }
    None
}

/// 盘上的一行（**契约**：那六个键）。
fn encode(entry: &HeldEntry) -> String {
    serde_json::json!({
        "kind": entry.kind,
        "name": entry.name,
        "command": entry.command,
        "pid": entry.pid,
        "log": entry.log,
        "at": entry.at_millis,
    })
    .to_string()
}

/// 一行读回一条。**形状不符 ⇒ `None`**（不猜、不当默认值）。
fn decode(line: &str) -> Option<HeldEntry> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let object = value.as_object()?;
    let kind = object.get("kind")?.as_str()?.to_string();
    // 词表是封闭的：认不出的取值**明确拒绝**，不就近猜（与持久字段上那条纪律同款）。
    if kind != KIND_START && kind != KIND_STOP {
        return None;
    }
    Some(HeldEntry {
        name: object.get("name")?.as_str()?.to_string(),
        command: match object.get("command") {
            Some(command) => command.as_str()?.to_string(),
            None => String::new(),
        },
        pid: match object.get("pid") {
            Some(pid) => u32::try_from(pid.as_u64()?).ok()?,
            None => 0,
        },
        log: match object.get("log") {
            Some(log) => log.as_str()?.to_string(),
            None => String::new(),
        },
        at_millis: object.get("at")?.as_u64()?,
        kind,
    })
}
