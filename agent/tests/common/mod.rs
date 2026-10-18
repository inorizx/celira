//! TASK_69 的测试装置：**主体先看格式，再调**。
//!
//! 旧形态的舞台模型直接调用动作（那时 `tools` 是全表，不需要"先看格式"这一轮）；
//! 新形态下模型只看到**在场集合**，展开后新出现的动作必须先看格式。这个包装器把那一轮
//! 补在**模型**身上（不是补在夹具上）——所以测试仍然证明"自己会先查"。
//!
//! 两条纪律：
//! - 它不认识任何具体动作，按**当前视野**里在场的能力去查，因此对每个舞台都成立；
//! - 视野里的能力面**变了**（有展开）就再查一轮——否则刚展开出来的动作仍旧没被看过。
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use celira_agent::core::{
    Action, ActionId, ActionParams, Decision, ModelError, ModelOutput, ModelPort, View,
};
use celira_agent::foundation::AgentId;
use celira_agent::host_loop::{WriteQualification, open_when_free};
use celira_agent::store::{Environment, StoreError};

/// 把任意舞台模型包一层：**先看格式，再调**。
pub struct InspectsFirst<M> {
    inner: M,
    /// 上一次查过的能力面（用于发现"视野变了"）。
    seen: Vec<String>,
    /// 被调用过几次（**含**包装器自己补的那些"先看格式"拍）。
    calls: usize,
}

impl<M: ModelPort> InspectsFirst<M> {
    pub fn new(inner: M) -> Self {
        Self {
            inner,
            seen: Vec::new(),
            calls: 0,
        }
    }

    /// 内层模型（舞台要读它自己的状态时用这个，例如"脚本走到第几条"）。
    pub fn inner(&self) -> &M {
        &self.inner
    }

    /// 被调用过几次——**含**包装器补的那些"先看格式"拍。
    ///
    /// 舞台原来统计的是"模型被调了几次"；新形态下"先看格式"也是主体的一次调用，
    /// 所以那份断言现在该读这个计数。
    pub fn calls(&self) -> usize {
        self.calls
    }

    /// 当前视野里在场的动作标识。
    fn present(view: &View) -> Vec<String> {
        view.capabilities()
            .items()
            .iter()
            .map(|each| each.id().as_str().to_string())
            .collect()
    }

    /// 这一轮要不要先看格式：视野里出现了**没看过**的动作。
    fn unseen(view: &View, seen: &[String]) -> Vec<String> {
        Self::present(view)
            .into_iter()
            .filter(|name| !seen.contains(name))
            .collect()
    }
}

impl<M: ModelPort> ModelPort for InspectsFirst<M> {
    /// **透明转发**（TASK-79）：这一层只是"先看格式"的外壳，它不该改变请求的内容——
    /// 不转发的话，装在它里面的模型就收不到装配装订的静态头。
    fn declare_static_head(&mut self, head: Option<&str>) {
        self.inner.declare_static_head(head);
    }

    fn complete(&mut self, view: &View) -> Result<ModelOutput, ModelError> {
        self.calls += 1;
        let unseen = Self::unseen(view, &self.seen);
        if !unseen.is_empty() {
            self.seen = Self::present(view);
            let actions: Vec<Action> = unseen
                .into_iter()
                .map(|name| {
                    Action::with_params(
                        ActionId::new("inspect_action"),
                        ActionParams::none().with("action", name),
                    )
                })
                .collect();
            if let Some(decision) = Decision::act(actions) {
                return Ok(ModelOutput::new(String::new(), None, decision));
            }
        }
        self.inner.complete(view)
    }
}

/// 夹具二进制的路径：**这个平台上的可执行件后缀，只有这一处知道**。
///
/// 它与测试二进制由同一次 `cargo test` 构建，所以就在测试可执行文件旁边
/// （`deps/` 那一层要退掉：测试可执行件在 `deps/` 里，可执行件在上一层）。
///
/// 为什么收在这里：仓库里曾有十来份各自拼 `.exe` 的副本，其中**一份漏了平台判断**
/// （`responses_protocol`），于是它在 Linux 上拼出 `agent_run.exe`（不存在）⇒
/// 那一支 6 条用例全死在"夹具跑得起来"（ENOENT），**在 Linux 上从来没跑过真东西**。
/// 判据一句话：**平台形状只许有一份**。
pub fn binary(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().expect("拿到测试可执行文件路径");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.join(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    })
}

/// 测试进程里取得环境**写资格**的耐心：与生产的 [`WriteQualification`] 同形，用例给短的。
///
/// 这里的等待只用来跨过一个**毫秒级窗口**（见 [`open_environment`] 的说明），不是等另一个
/// 真写者；10 秒上限只为「宁可明确失败、不无限等」。健康运行里一次就过，零开销。
pub const WRITE_QUALIFICATION: WriteQualification = WriteQualification {
    patience: Duration::from_secs(10),
    retry: Duration::from_millis(5),
};

/// 打开环境：写资格被**瞬时**握着时等一等（测试基础设施，2026-10-03）。
///
/// **为什么用例也要等**（真因，已由 strace 与最小实验双重证实）：测试二进制是多线程的。
/// 某条用例线程**持有环境写资格**（`flock` 落在打开文件描述上，见 `store.rs`）时，另一条
/// 用例线程 spawn 子进程 —— `fork` 把整张 fd 表复制给子进程，子进程在 `exec` 之前**继承了
/// 这把锁的 fd 副本**；`O_CLOEXEC` 要等 exec 那一刻才关掉它。于是有一个毫秒级的窗口：
/// 持有者已经 close、子进程还没 exec —— 此刻重新 open+flock 会得到 `AlreadyOpen`
/// （`store.rs` 把 `try_lock` 的任何失败都读作它）。窗口随并发子进程 exec 而消失。
///
/// 这是**测试进程的形状**（多线程 + fork），不是产品语义问题：单线程进程在 fork→exec 期间
/// 不会跑自己的代码（`spawn` 阻塞到 exec），宿主内部也没有「close 之后重开同一个环境」的路径。
///
/// 规则与生产 [`open_when_free`] 相同（同一个成因只有一个修法）：**只有 `already-open` 值得等**
/// （它是暂时的）；其余错误立刻报出；等满耐心则明确失败、带现场。
pub fn open_environment(root: &Path, agent: AgentId) -> Environment {
    open_when_free(root, &agent, WRITE_QUALIFICATION).unwrap_or_else(|failure| {
        panic!(
            "打开环境: {failure:?}（写资格耐心 {:?} 已满；root={}）",
            WRITE_QUALIFICATION.patience,
            root.display()
        )
    })
}

/// 创建环境：同一条写资格规则（`Environment::create` 没有生产版的等待入口，这里镜像一份）。
///
/// 瞬时 `AlreadyOpen` 的成因与 [`open_environment`] 完全相同；其余错误立刻报出。
pub fn create_environment(root: &Path, agent: AgentId) -> Environment {
    let started = std::time::Instant::now();
    loop {
        match Environment::create(root, agent.clone()) {
            Ok(environment) => return environment,
            Err(StoreError::AlreadyOpen) => {
                if started.elapsed() >= WRITE_QUALIFICATION.patience {
                    panic!(
                        "创建环境: AlreadyOpen（写资格耐心 {:?} 已满；root={}）",
                        WRITE_QUALIFICATION.patience,
                        root.display()
                    );
                }
                std::thread::sleep(WRITE_QUALIFICATION.retry);
            }
            Err(error) => panic!("创建环境: {error:?}（root={}）", root.display()),
        }
    }
}
