//! **这个 app 自己在跑哪一版** —— 关于页那一行。
//!
//! ## 判据：号从哪儿来（两格都要说出来）
//!
//! - **发布包**：`VERSION` 就在 exe 旁边（`make_release.ps1` 放的那一份，三个发布单元同一个号）
//!   ⇒ 读它，`source = "package"`；
//! - **源码形态**：没有那个文件 ⇒ 退回**编译期**的 `CARGO_PKG_VERSION`，`source = "build"`。
//!
//! 为什么要 `source`：**关于页上写一个猜出来的号，比不写更坏**。源码形态下那个号是编译期的，
//! 它未必等于你手里那个包的名字 —— 如实标出来，读者才知道该怎么读。
//!
//! ⚠ 这一格从前是前端里**写死的** `v0.1.0`（`client/src/pages/settings/SettingsAboutPage.tsx`）：
//! **发一版就撒一次谎**。与 `scripts/home/provision.js` 里那个写死的 Agent 名同一族
//! —— 系统借一个值替事实说话。
//!
//! 只读、不写、不联网；读不到就是读不到（不猜、不回落成某个常量）。

use std::path::Path;

/// 版本读数：`version` 是号，`source` 说它是怎么来的。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppVersionReadout {
    pub version: String,
    /// 封闭词表：`package`（exe 旁边的 `VERSION`）／`build`（编译期那个号）。
    pub source: String,
}

/// 发布包 `VERSION` 里 `version = 0.1.0` 那一行（读不到／空的 ⇒ `None`，**不猜**）。
fn version_line(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| {
            let (key, value) = line.split_once('=')?;
            if key.trim() == "version" {
                Some(value.trim().to_string())
            } else {
                None
            }
        })
        .filter(|value| !value.is_empty())
}

/// 与 `readout_from` 配对的那一半 IO：exe 旁边那份 `VERSION`。
fn packaged_version(exe_dir: &Path) -> Option<String> {
    version_line(&std::fs::read_to_string(exe_dir.join("VERSION")).ok()?)
}

/// **纯函数**：给定"exe 在哪个目录"，给出读数（用例直接量它，不需要真的有个壳）。
pub fn readout_from(exe_dir: Option<&Path>) -> AppVersionReadout {
    if let Some(version) = exe_dir.and_then(packaged_version) {
        return AppVersionReadout {
            version,
            source: "package".to_string(),
        };
    }
    AppVersionReadout {
        version: env!("CARGO_PKG_VERSION").to_string(),
        source: "build".to_string(),
    }
}

/// 壳给页面的一格读数（关于页用它显示"我在跑哪一版"）。
#[tauri::command]
pub fn app_version_readout() -> AppVersionReadout {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    readout_from(exe_dir.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_takes_the_version_row_only() {
        let text = "version = 0.1.0\ngit rev = 7c4b6a8\nplatform = windows-x64\n";
        assert_eq!(version_line(text), Some("0.1.0".to_string()));
        // 没有那一行 / 那一行是空的 / 只有别的行 ⇒ 一个都不猜。
        assert_eq!(version_line("git rev = abc\n"), None);
        assert_eq!(version_line("version =   \n"), None);
        assert_eq!(version_line(""), None);
    }

    #[test]
    fn packaged_version_wins_and_says_so() {
        let dir = std::env::temp_dir().join(format!("celira-appver-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("VERSION"), "version = 9.9.9\ngit rev = ab\n")
            .expect("write VERSION");

        let readout = readout_from(Some(&dir));
        assert_eq!(readout.version, "9.9.9");
        assert_eq!(readout.source, "package");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn without_a_package_it_falls_back_to_the_build_number_and_says_so() {
        // 一个空的目录 = 源码形态（exe 旁边没有 VERSION）⇒ 编译期那个号，且 source=build。
        let readout = readout_from(Some(Path::new("no-such-dir-celira-appver")));
        assert_eq!(readout.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(readout.source, "build");
        // 连 exe 目录都不知道（current_exe 失败）也走同一条回落。
        assert_eq!(readout_from(None).source, "build");
    }
}
