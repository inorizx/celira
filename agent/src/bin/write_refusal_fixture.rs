//! 夹具：在 Unix 上制造一次**真实的写入拒绝**（进程级文件大小上限），供"写入被真拒绝之后
//! 会怎样"这条契约取证。
//!
//! 这**不是**产品的一部分，而是测试夹具。为什么需要它：那条契约的 Windows 证据靠"另一个句柄
//! 持排他字节范围锁"制造真拒绝，而 Unix 上字节范围锁只是**建议性**的（不产生故障，写出来的是
//! 假证据）。Unix 的等价手段是 `RLIMIT_FSIZE` ＋ 忽略 `SIGXFSZ`：写超出上限的那一次由**内核**
//! 拒绝（`EFBIG`），已写下的前缀照旧在盘上——这正是那条契约要的形状。
//!
//! 为什么必须是**独立进程**：`RLIMIT_FSIZE` 是**进程级**的。在测试进程里设它，会把并行的别的
//! 用例一起拖下水（`cargo test` 默认并行）——所以拒绝由本夹具产生，父进程只断言它打出来的读数
//! 与它留在盘上的东西。
//!
//! 职责边界：夹具只做**注入 ＋ 读数**（打开环境、设上限、试三次、打印）；"盘上只剩可证实前缀"
//! 与"重开靠未收尾兜住"由父进程在夹具退出后自己断言（那时环境已被放开）。
//!
//! 全程离线：不联网、不读密钥、只碰父进程给的那个独立环境根。
//!
//! 用法：`CELIRA_WRITE_REFUSAL_ROOT=<环境根> CELIRA_WRITE_REFUSAL_AGENT=<身份>
//! CELIRA_WRITE_REFUSAL_RUN=<活动>`。退出码 0 = 三条读数都如预期；2 = 有一条不如预期。

// 下面这些**只被 Unix 臂用**：非 Unix 上 `main` 只打印一句就退出（那台机器上没有等价手段）。
// 不加 `cfg` 的话，Windows 构建里它们全是死代码，而本 crate 的门禁带 `-D warnings`（会红）。
#[cfg(unix)]
use std::path::PathBuf;

#[cfg(unix)]
use celira_agent::foundation::{AgentId, RunId};
#[cfg(unix)]
use celira_agent::store::{Environment, StoreError, TracePayload};

#[cfg(unix)]
const ROOT_ENV: &str = "CELIRA_WRITE_REFUSAL_ROOT";
#[cfg(unix)]
const AGENT_ENV: &str = "CELIRA_WRITE_REFUSAL_AGENT";
#[cfg(unix)]
const RUN_ENV: &str = "CELIRA_WRITE_REFUSAL_RUN";
/// `limit`（默认）= 真的装上那道上限；`control` = **正对照**：别的都一样，只是不装上限。
#[cfg(unix)]
const MODE_ENV: &str = "CELIRA_WRITE_REFUSAL_MODE";

#[cfg(unix)]
fn main() {
    std::process::exit(unix_main());
}

#[cfg(not(unix))]
fn main() {
    eprintln!("这个夹具只在 Unix 上成立：它用的是进程级文件大小上限（RLIMIT_FSIZE）");
    std::process::exit(3);
}

#[cfg(unix)]
fn unix_main() -> i32 {
    let root = PathBuf::from(std::env::var(ROOT_ENV).expect("要给环境根"));
    let agent = AgentId::new(std::env::var(AGENT_ENV).expect("要给身份"));
    let run = RunId::new(std::env::var(RUN_ENV).expect("要给活动"));

    let mode = std::env::var(MODE_ENV).unwrap_or_else(|_| "limit".to_string());
    let mut environment = Environment::open(&root, agent).expect("打开环境");
    let trace = root.join("trace.ndjson");
    let confirmed = std::fs::metadata(&trace).expect("读已确认前缀的大小").len();

    // 上限就设成**已确认前缀的大小**：此后任何一个字节都写不进去（内核拒绝，不是我们假装）。
    // 正对照（`control`）别的都不动，**只**不装上限——于是同一支夹具、同一个环境形状，
    // 唯一差别就是那道上限；这样"拒绝了"才不可能来自别的原因。
    let limited = match mode.as_str() {
        "limit" => {
            install_file_size_limit(confirmed);
            true
        }
        "control" => false,
        other => {
            eprintln!("不认识的方式：{other}（要 limit 或 control）");
            return 3;
        }
    };

    let refused = environment.append_trace(
        &run,
        TracePayload::RunInput {
            text: "这一行写不进去".into(),
        },
    );
    let second = environment.append_trace(
        &run,
        TracePayload::RunInput {
            text: "第二次".into(),
        },
    );
    let paused = environment.mark_paused(&run, "save-failed:trace");

    println!("mode = {mode}");
    println!("confirmed-bytes = {confirmed}");
    println!("refused-append = {}", token(&refused));
    println!("second-append = {}", token(&second));
    println!("mark-paused = {}", token(&paused));

    // 夹具自己也核一遍（父进程还会再核一次）：三条读数与预期不符就报 2。
    let expected = if limited {
        [
            ("refused-append", "io"),
            ("second-append", "tail-unconfirmed"),
            ("mark-paused", "tail-unconfirmed"),
        ]
    } else {
        // 正对照：没有上限时，这三笔都该成功。
        [
            ("refused-append", "ok"),
            ("second-append", "ok"),
            ("mark-paused", "ok"),
        ]
    };
    let actual = [
        ("refused-append", token(&refused)),
        ("second-append", token(&second)),
        ("mark-paused", token(&paused)),
    ];
    for ((name, want), (_, got)) in expected.iter().zip(actual.iter()) {
        if want != got {
            eprintln!("{name}：预期 {want}，实际 {got}");
            return 2;
        }
    }
    0
}

/// 三条读数用同一套记号：父进程按记号断言，不解析 Debug 文本。
#[cfg(unix)]
fn token(result: &Result<u64, StoreError>) -> String {
    match result {
        Ok(_) => "ok".to_string(),
        Err(StoreError::Io) => "io".to_string(),
        Err(StoreError::TailUnconfirmed) => "tail-unconfirmed".to_string(),
        Err(other) => format!("other:{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 手写 FFI（零新依赖）：把**软**上限设成给定大小，并让超出时返回错误而不是杀进程。
// 只动软上限、不动硬上限——硬上限一旦降下去，非特权进程就抬不回来了。
// ---------------------------------------------------------------------------

#[cfg(unix)]
const RLIMIT_FSIZE: std::os::raw::c_int = 1;
#[cfg(unix)]
const SIGXFSZ: std::os::raw::c_int = 25;
/// `signal(SIGXFSZ, SIG_IGN)` 里的 `SIG_IGN`（POSIX 规定为 1）。
#[cfg(unix)]
const SIG_IGN: usize = 1;

#[cfg(unix)]
#[repr(C)]
struct RLimit {
    rlim_cur: std::os::raw::c_ulong,
    rlim_max: std::os::raw::c_ulong,
}

#[cfg(unix)]
unsafe extern "C" {
    fn getrlimit(resource: std::os::raw::c_int, limits: *mut RLimit) -> std::os::raw::c_int;
    fn setrlimit(resource: std::os::raw::c_int, limits: *const RLimit) -> std::os::raw::c_int;
    fn signal(signum: std::os::raw::c_int, handler: usize) -> usize;
}

/// 忽略 `SIGXFSZ`（否则超出上限时进程**直接被杀**，而不是让 `write` 返回 `EFBIG`），
/// 再把软上限设成 `bytes`。
#[cfg(unix)]
fn install_file_size_limit(bytes: u64) {
    // SAFETY: 三个调用都是本进程自用的 POSIX 接口，参数按各自的 C 约定给；`RLimit` 的
    // 布局与 C 的 `struct rlimit` 一致（两个 `rlim_t`，在 Unix 上是 `unsigned long`）。
    unsafe {
        signal(SIGXFSZ, SIG_IGN);
        let mut limits = RLimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        assert_eq!(
            getrlimit(RLIMIT_FSIZE, &mut limits),
            0,
            "读得到当前的文件大小上限"
        );
        limits.rlim_cur = bytes;
        assert_eq!(
            setrlimit(RLIMIT_FSIZE, &limits),
            0,
            "设得上软上限（硬上限不动：{:?}）",
            limits.rlim_max
        );
    }
}
