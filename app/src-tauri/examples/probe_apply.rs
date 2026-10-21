//! 真机跑一次「替换」：把 `<安装目录>` 换名成 `.old-probe`，把 payload 换名成原名。
//!
//! ```text
//! CELIRA_UPDATE_PROBE_INSTALL=D:\celira-test\app \
//! CELIRA_UPDATE_PROBE_PAYLOAD=D:\celira-test\app-new-0.1.3 \
//! cargo run --example probe_apply
//! ```
//!
//! ⚠ 只在**测试目录**上跑；它会改名两个目录（不删除）。去掉最后那句 `Start-Process`，
//! 探针不启动任何东西。

use celira_app_lib::update_apply::{apply_script, base64_utf16};

fn main() {
    let install = std::env::var("CELIRA_UPDATE_PROBE_INSTALL").expect("缺 CELIRA_UPDATE_PROBE_INSTALL");
    let payload = std::env::var("CELIRA_UPDATE_PROBE_PAYLOAD").expect("缺 CELIRA_UPDATE_PROBE_PAYLOAD");
    let script = apply_script(&install, &payload, "probe").replace(
        "Start-Process -FilePath (Join-Path $install 'celira-app.exe')",
        "",
    );
    let status = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &base64_utf16(&script),
        ])
        .status()
        .expect("powershell 该能起");
    println!("exit={status}");
    if !status.success() {
        std::process::exit(1);
    }
}
