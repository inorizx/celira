//! **真机验收**：在**当前身份**下把 `terminal=sandbox` 跑一遍，把判据打印成可复核的读数。
//!
//! 它不是单元测试的替代品，是那条测试证明不了的东西：**这一档在"住户自己的 uid"下到底
//! 兑现了什么**。因此它必须由**那个 uid** 来跑（`sudo -u celira ...`），而不是由写代码的人跑。
//!
//! ```text
//! cargo build --example sandbox_accept
//! sudo -u celira ./target/debug/examples/sandbox_accept /var/lib/celira/agents/<id>
//! ```
//!
//! 判据（任一条不成立 ⇒ 退出码 1，**不"大概通过"**）：
//!
//! 1. **入口**：`choose_for_entry` 在这台机器上选出一支后端，且环境根的 owner 是当前 uid；
//! 2. **能干活**：环境根里 `echo` 写得进去、退出码 0；
//! 3. **写不了系统**：`/etc` 下建文件必须失败；
//! 4. **造不出兄弟目录**：环境根的**父目录**里建目录必须失败（否则它能自己造一个"别人的家"）；
//! 5. **断网**：`/dev/tcp` 连 1.1.1.1 必须失败，且理由是 `Permission denied` 或 `Network is unreachable`；
//! 6. **拿不到调用方的 fd**：子进程只看得见 0/1/2（＋它读目录那一格）。
//!
//! ⚠ 第 4 条靠 **Landlock**（`src/landlock.rs`）：它把"写入"关进环境根，因此**同一个 uid**
//! 下也成立。没有 Landlock 的内核（< 5.13）上这一条会 FAIL——那正是这一档退回 DAC 的读数，
//! 脚本会把它印出来而不是藏起来。

#[cfg(target_os = "linux")]
fn main() {
    use celira_agent::app_contract::AppPort;
    use celira_agent::core::{Action, ActionId, ActionParams, ActionPort};
    use celira_agent::foundation::{AgentId, OperationId, RunId, Turn};
    use celira_agent::terminal_app::{CAPABILITY_RUN, TerminalAccess, TerminalApp, TerminalTier};
    use std::path::PathBuf;

    let Some(root) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("用法: sandbox_accept <环境根>");
        std::process::exit(2);
    };

    let mut failed = 0usize;
    let mut judge = |ok: bool, line: &str| {
        println!("{} {line}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            failed += 1;
        }
    };

    // 1. 入口：真探针 + 身份判据。
    let backend = match celira_agent::sandbox_backend::choose_for_entry(&root) {
        Ok(backend) => backend,
        Err(refusal) => {
            println!("REFUSED {}", refusal.as_str());
            println!("detail: {}", refusal.detail());
            std::process::exit(1);
        }
    };
    println!("backend = {}", backend.name());
    println!("guarantees = {}", backend.guarantees().reading());

    let access = TerminalAccess::new(TerminalTier::Sandbox, vec![]).with_sandbox_backend(backend);
    let mut app = TerminalApp::new(access, &root);
    let agent = AgentId::new("accept");
    let operation = OperationId::new(RunId::new("accept"), Turn::FIRST);

    let run = |app: &mut TerminalApp, command: &str| -> (bool, String) {
        let action = Action::with_params(
            ActionId::new(CAPABILITY_RUN),
            ActionParams::none().with("command", command),
        );
        let outcome = app.execute(&agent, &operation, &action);
        let text = outcome
            .content()
            .iter()
            .map(|block| block.text().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        (outcome.failure_class().is_none(), text)
    };

    // 2. 能干活。
    let (ran, receipt) = run(&mut app, "echo inside > inside.txt && cat inside.txt");
    let ok = ran && receipt.contains("exit = 0") && receipt.contains("inside");
    judge(ok, "环境根里写得进去（能干活）");
    println!("{receipt}");

    // 3. 写不了系统。
    let (_, receipt) = run(&mut app, "touch /etc/celira-accept-probe");
    judge(
        !receipt.contains("exit = 0") && receipt.contains("Permission denied"),
        "写不了 /etc",
    );

    // 4. 造不出兄弟目录（只有专用身份才成立）。
    // ⚠ 是**环境根的父目录**里，不是环境根里面：写在根里面只是"能写"的另一种读数，
    // 证明不了它造不出一个"别人的家"（2026-10-23 本机第一版就是这样假绿的）。
    let parent = root.parent().unwrap_or(&root);
    let sibling = format!("mkdir {}/celira-accept-sibling", parent.display());
    let (_, receipt) = run(&mut app, &sibling);
    println!("{receipt}");
    judge(!receipt.contains("exit = 0"), "造不出环境根的同级目录");

    // 5. 断网。
    let (_, receipt) = run(&mut app, "exec 3<>/dev/tcp/1.1.1.1/443");
    let denied =
        receipt.contains("Permission denied") || receipt.contains("Network is unreachable");
    judge(denied && !receipt.contains("exit = 0"), "连不出去");

    // 6. 拿不到调用方的 fd。
    let (_, receipt) = run(&mut app, "cd /proc/self/fd && echo *");
    // ⚠ 要读 `--- stdout ---` **那一段的正文**，不是 `stdout = … 字节` 那行读数：
    // 读数里的 `7 bytes` 会被当成一个 fd（2026-10-23 本机踩到，判据假红）。
    let listing = receipt
        .split("--- stdout ---\n")
        .nth(1)
        .unwrap_or("")
        .split("--- stderr ---")
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    println!("fds = {listing}");
    let leaked = listing
        .split_whitespace()
        .filter(|field| field.parse::<u32>().map(|fd| fd > 3).unwrap_or(true))
        .count();
    judge(leaked == 0, "子进程拿不到调用方的 fd");

    println!("{}", app.description());

    if failed > 0 {
        println!("{failed} 条判据不成立");
        std::process::exit(1);
    }
    println!("全部成立");
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("本平台没有 sandbox 档的实现（见 TerminalTier::available）");
    std::process::exit(2);
}
