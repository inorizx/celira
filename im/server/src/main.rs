//! Celira Core V1：单二进制服务入口。
//!
//! 职责只限：初始化日志 → 安装进程级脱敏 panic hook → 解析子命令 →
//! 读取配置 → 组装并执行。
//! - 无子命令：启动 HTTP server；
//! - `bootstrap-human --display-name <NAME>`：创建首位 Human + 初始 Session；
//! - `create-human --display-name <NAME>`：创建新的 active Human + 一次性
//!   Session（与 bootstrap-human 共用同一创建与 Session 签发用例）；
//! - `issue-human-session --human-id <ID>`：为 active Human 补发 Session；
//! - `issue-pairing-grant --human-id <ID> [--device-label <LABEL>]`：为
//!   active Human 签发一次性 Pairing Grant（初始配对路径，token 只输出
//!   一次；TTL 由 `CELIRA_GRANT_TTL_MINUTES` 配置）。
//!
//! stdout 纪律：CLI 成功时 stdout 只有一行机器可解析 JSON（含一次性
//! token）；日志统一走 stderr，原始 token 永不进入日志。

mod adapters;
mod application;
mod composition;
mod domain;
mod ports;
mod protocol;

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing::{error, info};
use tracing_subscriber::EnvFilter;

const USAGE: &str = "usage: celira-im [bootstrap-human --display-name <NAME> | \
                     create-human --display-name <NAME> | \
                     issue-human-session --human-id <ID> | \
                     issue-pairing-grant --human-id <ID> [--device-label <LABEL>]]";

enum CliCommand {
    Serve,
    BootstrapHuman {
        display_name: String,
    },
    CreateHuman {
        display_name: String,
    },
    IssueHumanSession {
        human_id: String,
    },
    IssuePairingGrant {
        human_id: String,
        device_label: Option<String>,
    },
}

impl CliCommand {
    fn parse(args: &[String]) -> Result<Self, String> {
        let Some(subcommand) = args.first().map(String::as_str) else {
            return Ok(Self::Serve);
        };
        match subcommand {
            "bootstrap-human" => {
                let flags =
                    parse_flags(args, &["--display-name"]).ok_or_else(|| USAGE.to_owned())?;
                let display_name =
                    required_flag(&flags, "--display-name").ok_or_else(|| USAGE.to_owned())?;
                Ok(Self::BootstrapHuman { display_name })
            }
            "create-human" => {
                let flags =
                    parse_flags(args, &["--display-name"]).ok_or_else(|| USAGE.to_owned())?;
                let display_name =
                    required_flag(&flags, "--display-name").ok_or_else(|| USAGE.to_owned())?;
                Ok(Self::CreateHuman { display_name })
            }
            "issue-human-session" => {
                let flags = parse_flags(args, &["--human-id"]).ok_or_else(|| USAGE.to_owned())?;
                let human_id =
                    required_flag(&flags, "--human-id").ok_or_else(|| USAGE.to_owned())?;
                Ok(Self::IssueHumanSession { human_id })
            }
            "issue-pairing-grant" => {
                let flags = parse_flags(args, &["--human-id", "--device-label"])
                    .ok_or_else(|| USAGE.to_owned())?;
                let human_id =
                    required_flag(&flags, "--human-id").ok_or_else(|| USAGE.to_owned())?;
                let device_label = optional_flag(&flags, "--device-label");
                if device_label
                    .as_ref()
                    .is_some_and(|label| label.trim().is_empty())
                {
                    return Err(USAGE.to_owned());
                }
                Ok(Self::IssuePairingGrant {
                    human_id,
                    device_label,
                })
            }
            other => Err(format!("unknown command {other:?}\n{USAGE}")),
        }
    }
}

/// 严格解析一个子命令允许的旗标集合：只接受 `--flag value` 或
/// `--flag=value`，拒绝位置参数、未知旗标、缺值与任一重复旗标。多个
/// 白名单旗标可同时使用（如 issue-pairing-grant 的 human-id + label）。
fn parse_flags(args: &[String], allowed: &[&str]) -> Option<Vec<(String, String)>> {
    let mut parsed = Vec::new();
    let mut index = 1;
    while index < args.len() {
        let raw = &args[index];
        let (name, value, consumed_next) = match raw.split_once('=') {
            Some((name, value)) if !name.is_empty() && !value.is_empty() => {
                (name, value.to_owned(), false)
            }
            Some(_) => return None,
            None => {
                let value = args.get(index + 1)?;
                if value.starts_with("--") {
                    return None;
                }
                (raw.as_str(), value.clone(), true)
            }
        };
        if !allowed.contains(&name) || parsed.iter().any(|(seen, _)| seen == name) {
            return None;
        }
        parsed.push((name.to_owned(), value));
        index += if consumed_next { 2 } else { 1 };
    }
    Some(parsed)
}

fn required_flag(flags: &[(String, String)], name: &str) -> Option<String> {
    flags
        .iter()
        .find(|(seen, _)| seen == name)
        .map(|(_, value)| value.clone())
}

fn optional_flag(flags: &[(String, String)], name: &str) -> Option<String> {
    required_flag(flags, name)
}

#[tokio::main]
async fn main() {
    init_tracing();
    install_redacted_panic_hook();

    let config = match composition::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            error!(
                operation = "config.load",
                error_kind = e.error_kind(),
                "invalid configuration"
            );
            std::process::exit(1);
        }
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match CliCommand::parse(&args) {
        Ok(c) => c,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };

    match command {
        CliCommand::Serve => {
            // 启动 info 不记录 db_path（路径是本地拓扑细节，不进生产日志）。
            info!(listen_addr = %config.listen_addr, "celira-im starting");
            if let Err(e) = composition::run(&config).await {
                error!(
                    operation = "startup.run",
                    error_kind = e.error_kind(),
                    "startup failed"
                );
                std::process::exit(1);
            }
        }
        CliCommand::BootstrapHuman { display_name } => {
            if let Err(e) = composition::cli::bootstrap_human(&config, &display_name).await {
                error!(
                    operation = "cli.bootstrap_human",
                    error_kind = e.error_kind(),
                    "bootstrap-human failed"
                );
                std::process::exit(1);
            }
        }
        CliCommand::CreateHuman { display_name } => {
            if let Err(e) = composition::cli::create_human(&config, &display_name).await {
                error!(
                    operation = "cli.create_human",
                    error_kind = e.error_kind(),
                    "create-human failed"
                );
                std::process::exit(1);
            }
        }
        CliCommand::IssueHumanSession { human_id } => {
            if let Err(e) = composition::cli::issue_human_session(&config, &human_id).await {
                error!(
                    operation = "cli.issue_human_session",
                    error_kind = e.error_kind(),
                    "issue-human-session failed"
                );
                std::process::exit(1);
            }
        }
        CliCommand::IssuePairingGrant {
            human_id,
            device_label,
        } => {
            if let Err(e) =
                composition::cli::issue_pairing_grant(&config, &human_id, device_label.as_deref())
                    .await
            {
                error!(
                    operation = "cli.issue_pairing_grant",
                    error_kind = e.error_kind(),
                    "issue-pairing-grant failed"
                );
                std::process::exit(1);
            }
        }
    }
}

/// 日志统一写 stderr：CLI 的 stdout 只保留机器可解析的输出行，
/// 保证 `bootstrap-human` / `issue-human-session` 可被可靠解析。
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}

/// 进程级脱敏 panic hook 的固定输出文案：编译期常量，hook 绝不拼接任何
/// 动态内容。结构字段固定为 `operation=process.panic, error_kind=panic`；
/// 进程级无 request_id 概念，固定文案不记录任何请求上下文。
const REDACTED_PANIC_HOOK_LINE: &[u8] =
    b"ERROR celira-im: process panicked; operation=process.panic error_kind=panic\n";

/// 安装幂等保护：整个进程只允许安装一次（由 main 调用）。后续重复调用
/// 直接返回，不替换已安装 hook，避免任何并发 take_hook/set_hook 竞态。
static PANIC_HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

/// 进程级脱敏 panic hook（P0 收口）：init_tracing 之后、解析任何配置/
/// 子命令之前由 main 调用一次。它与请求内 `catch_unwind`（post-commit
/// notifier panic）职责分离：
/// - `catch_unwind` 负责请求内最小隔离，只保留固定 warn + canonical
///   request_id（见 CORE_V1_ERROR_HANDLING.md §8.1）；
/// - 本 hook 负责进程级「panic 原文不进日志」红线：只输出固定分类文案，
///   绝不读取 `PanicHookInfo` 的 payload/location，绝不打印 backtrace，
///   绝不保存或调用默认 hook。
///   hook 直接写固定字节到 stderr，不经过 tracing 派发；写入失败被显式忽略，
///   最坏情况只丢失这条固定分类，绝不会因日志失败引发二次 panic。
fn install_redacted_panic_hook() {
    if PANIC_HOOK_INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::panic::set_hook(Box::new(|_| {
        let _ = std::io::stderr().write_all(REDACTED_PANIC_HOOK_LINE);
    }));
}

#[cfg(test)]
mod tests {
    use super::install_redacted_panic_hook;
    use std::process::Command;

    /// 子进程探针的环境变量：仅当被父测试以该环境变量启动时才安装生产
    /// 同一脱敏 hook 并注入带唯一敏感标记的 panic。普通测试套件里运行
    /// （无该环境变量）时不做任何全局副作用，避免共享测试进程永久替换
    /// 全局 hook。
    const CHILD_PROBE_ENV: &str = "CELIRA_IM_PANIC_HOOK_CHILD_PROBE";

    /// 父测试用于筛选子进程测试名的唯一子串；不得与任何其他测试名重叠。
    const CHILD_PROBE_TEST: &str = "celira_core_panic_hook_child_probe";

    /// 唯一敏感标记：断言它绝不进入子进程 stderr。
    const SECRET_MARKER: &str = "CELIRA_IM_SECRET_PANIC_PAYLOAD_9f4c2d7e";

    /// 子进程探针：以 `CHILD_PROBE_ENV` 启动时，安装与 main 完全相同的
    /// 脱敏 hook 后注入 panic（payload 为唯一敏感标记），进程以非零退出。
    #[test]
    fn celira_core_panic_hook_child_probe() {
        if std::env::var_os(CHILD_PROBE_ENV).is_none() {
            return;
        }
        install_redacted_panic_hook();
        panic!("{SECRET_MARKER}");
    }

    /// 真实子进程负向测试：子进程安装与 main 同一脱敏 hook 后注入 panic。
    /// 断言非零退出；stderr 含固定安全分类与固定文案；且不含 panic
    /// payload、源码位置、`panicked at`、backtrace。
    #[test]
    fn process_panic_hook_redacts_payload_in_real_child_process() {
        let exe = std::env::current_exe().expect("current test binary");
        let output = Command::new(&exe)
            .arg(CHILD_PROBE_TEST)
            .arg("--nocapture")
            .env(CHILD_PROBE_ENV, "1")
            .env_remove("RUST_BACKTRACE")
            .env_remove("RUST_LIB_BACKTRACE")
            .output()
            .expect("spawn child test process");
        assert!(
            !output.status.success(),
            "child process must exit non-zero after uncaught panic, got {:?}",
            output.status
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("operation=process.panic") && stderr.contains("error_kind=panic"),
            "fixed structured classification must appear in child stderr: {stderr}"
        );
        assert!(
            stderr.contains("process panicked"),
            "fixed message must appear in child stderr: {stderr}"
        );
        assert!(
            !stderr.contains(SECRET_MARKER),
            "panic payload must never reach child stderr: {stderr}"
        );
        assert!(
            !stderr.contains("panicked at"),
            "default panic formatting must be replaced by the redacted hook: {stderr}"
        );
        assert!(
            !stderr.contains("backtrace") && !stderr.contains("RUST_BACKTRACE"),
            "no backtrace may reach child stderr: {stderr}"
        );
        assert!(
            !stderr.contains("src/main.rs"),
            "panic source location must never reach child stderr: {stderr}"
        );
    }
}
