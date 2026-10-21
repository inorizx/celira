//! 真机跑一次「下载 → 校 sha256 → 解压」（不替换）。
//!
//! ```text
//! CELIRA_UPDATE_PROBE_DIR=<安装目录> \
//! CELIRA_UPDATE_VERSION=0.1.3 \
//! CELIRA_UPDATE_URL=https://…/celira-0.1.3-win-x64.zip \
//! CELIRA_UPDATE_SHA256=e2b5… \
//! cargo run --example probe_download
//! ```
//!
//! 成功时把 `PreparedUpdate` 打成 JSON；payload 落在 `<安装目录>-new-<版本>`，**不动安装目录**。
//! 失败就原样打印原因并以 1 退出（不要在脚本里把失败吞成绿）。

use std::path::Path;

fn main() {
    let install = std::env::var("CELIRA_UPDATE_PROBE_DIR").expect("缺 CELIRA_UPDATE_PROBE_DIR");
    let version = std::env::var("CELIRA_UPDATE_VERSION").expect("缺 CELIRA_UPDATE_VERSION");
    let url = std::env::var("CELIRA_UPDATE_URL").expect("缺 CELIRA_UPDATE_URL");
    let sha256 = std::env::var("CELIRA_UPDATE_SHA256").expect("缺 CELIRA_UPDATE_SHA256");

    let result = tauri::async_runtime::block_on(celira_app_lib::update_apply::download_into(
        Path::new(&install),
        version,
        url,
        Some(sha256),
    ));
    match result {
        Ok(prepared) => match serde_json::to_string_pretty(&prepared) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("读数序列化失败：{error}");
                std::process::exit(1);
            }
        },
        Err(why) => {
            eprintln!("失败：{why}");
            std::process::exit(1);
        }
    }
}
