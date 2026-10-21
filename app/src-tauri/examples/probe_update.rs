//! 真实渠道的一次读数（不经过 UI）。
//!
//! `CELIRA_UPDATE_PROBE_DIR=<含 VERSION 的目录> cargo run --example probe_update`
//! 打印 `update_check` 的 JSON —— 用来在真机上证明"GitHub 优先、失败退 Gitee"这条链
//! 真的能拿到东西，而不是只在单元测试的假载荷里成立。
//!
//! 不设那个变量 ⇒ 用 examples 二进制自己的目录（没有 VERSION ⇒ `unknown`，如实说）。

use std::path::Path;

fn main() {
    let dir = std::env::var("CELIRA_UPDATE_PROBE_DIR").unwrap_or_default();
    let exe_dir = if dir.trim().is_empty() {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
    } else {
        Some(Path::new(dir.trim()).to_path_buf())
    };
    let readout = tauri::async_runtime::block_on(celira_app_lib::update::check_from_dir(
        exe_dir.as_deref(),
    ));
    match serde_json::to_string_pretty(&readout) {
        Ok(text) => println!("{text}"),
        Err(error) => eprintln!("读数序列化失败：{error}"),
    }
}
