//! **这台机器上的 app 设置**（机器级事实，一份文件一个权威位置）。
//!
//! ## 为什么有它，以及它只装什么
//!
//! `PROJECT_SHAPE.md` §8.6-1 裁的是「**第一次启动问一次**"要不要在这台机器上跑一个世界"
//! （它会写磁盘、占端口、常驻后台）；之后按面板的开关来」——**问一次**这件事需要一个地方
//! 记住"问过了、答复是什么"。那就是本文件唯一装的东西：
//!
//! ```json
//! { "runLocalWorld": true }
//! ```
//!
//! 三态是**有意的**，不是省事（`Option<bool>`）：
//!
//! | 文件 | `run_local_world()` | 界面该干什么 |
//! |---|---|---|
//! | 不在 / 读不懂 | `None` | **问一次**（还没表态） |
//! | `true` | `Some(true)` | 默认就在这台机器上跑 —— 打开 app 直接进自己的世界 |
//! | `false` | `Some(false)` | 不跑本机的；第一屏回登录页（连别的世界） |
//!
//! ## 它**不是**什么（护栏）
//!
//! - **不是家的配置**：家的位置、端口、身份三格在 `home.conf`（`home_facts` 那一套），
//!   本文件不重复任何一个 —— 一份事实一个权威位置。
//! - **不装凭据**：Human token、家钥匙、模型 key 一个字节都不进来（响应只回布尔）。
//! - **不是"家条目表"**：§8.4 的家条目（地址 + 钥匙，本机一条 + 远程 N 条）归页面那一侧
//!   （`client/src/api/homes.ts`）；这里只有"这台机器要不要跑一个世界"这一个开关。

use std::fs;
use std::path::{Path, PathBuf};

/// 设置文件名（住在 app 的**配置目录**里，与"家"分开：家可以被搬走，这个开关跟着 app）。
pub const SETTINGS_FILE: &str = "settings.json";

/// 这份设置的全部内容（**只装"这台机器上的 app 该怎么表现"**；一个事实一个键）。
///
/// | 键 | 三态 | 缺省（没表态）时的行为 |
/// |---|---|---|
/// | `runLocalWorld` | `Option<bool>` | **问一次**（§8.6-1）—— 不替用户表态 |
/// | `quitStopsServices` | `Option<bool>` | **停**（§8.3：托盘菜单那一项就写着"停止本机服务"；默认不替用户做隐藏的事） |
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MachineSettings {
    pub run_local_world: Option<bool>,
    pub quit_stops_services: Option<bool>,
}

impl MachineSettings {
    /// 「退出时停本机服务」的**生效值**（没表态 ⇒ 停；§8.3 的默认）。
    pub fn quit_stops_services_effective(&self) -> bool {
        self.quit_stops_services.unwrap_or(true)
    }
}

/// 设置文件的路径（`<app 配置目录>/settings.json`）。
pub fn settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join(SETTINGS_FILE)
}

/// 解析设置正文（**宽容**：旧文件缺字段、类型不符一律归"没表态"，**绝不猜**）。
pub fn parse_settings(text: &str) -> MachineSettings {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) else {
        return MachineSettings::default();
    };
    let flag = |key: &str| match parsed.get(key) {
        Some(serde_json::Value::Bool(value)) => Some(*value),
        _ => None,
    };
    MachineSettings {
        run_local_world: flag("runLocalWorld"),
        quit_stops_services: flag("quitStopsServices"),
    }
}

/// 解析"这台机器要不要跑一个世界"（三态；只有明确的 `true`/`false` 才算表态）。
pub fn parse_run_local_world(text: &str) -> Option<bool> {
    parse_settings(text).run_local_world
}

/// 渲染设置正文（键序稳定、带尾换行；**只写表过态的键** —— 没表态的键不写，
/// 那正是"没表态"与"表了 false"的区别，落盘也要保住）。
pub fn render_settings(settings: &MachineSettings) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Some(value) = settings.run_local_world {
        lines.push(format!(
            "  \"runLocalWorld\": {}",
            if value { "true" } else { "false" }
        ));
    }
    if let Some(value) = settings.quit_stops_services {
        lines.push(format!(
            "  \"quitStopsServices\": {}",
            if value { "true" } else { "false" }
        ));
    }
    if lines.is_empty() {
        return "{}\n".to_string();
    }
    format!("{{\n{}\n}}\n", lines.join(",\n"))
}

/// 读这份设置（文件不在 / 读不懂 ⇒ 全"没表态"）。
pub fn read_settings(config_dir: &Path) -> MachineSettings {
    match fs::read_to_string(settings_path(config_dir)) {
        Ok(text) => parse_settings(&text),
        Err(_) => MachineSettings::default(),
    }
}

/// 读"这台机器要不要跑一个世界"：文件不在 / 读不懂 / 没这一格 ⇒ `None`。
pub fn read_run_local_world(config_dir: &Path) -> Option<bool> {
    read_settings(config_dir).run_local_world
}

/// 读"退出时停不停本机服务"（**生效值**：没表态 ⇒ `true`）。
pub fn read_quit_stops_services(config_dir: &Path) -> bool {
    read_settings(config_dir).quit_stops_services_effective()
}

/// 写一份设置（原子性靠"先写临时文件再改名"——与 `home-key.js` 落摘要同一种处置：
/// 半个文件被读到比"还没写"糟得多）。
pub fn write_settings(config_dir: &Path, settings: &MachineSettings) -> Result<(), String> {
    fs::create_dir_all(config_dir)
        .map_err(|error| format!("建配置目录失败（{}）：{error}", config_dir.display()))?;
    let target = settings_path(config_dir);
    let temporary = config_dir.join(format!("{SETTINGS_FILE}.tmp"));
    fs::write(&temporary, render_settings(settings))
        .map_err(|error| format!("写设置失败（{}）：{error}", temporary.display()))?;
    fs::rename(&temporary, &target)
        .map_err(|error| format!("设置落地失败（{}）：{error}", target.display()))
}

/// 写下表态（**读-改-写**：只动这一格，别的格一个字都不碰）。
pub fn write_run_local_world(config_dir: &Path, value: bool) -> Result<(), String> {
    let mut settings = read_settings(config_dir);
    settings.run_local_world = Some(value);
    write_settings(config_dir, &settings)
}

/// 写下"退出时停不停本机服务"（**读-改-写**）。
pub fn write_quit_stops_services(config_dir: &Path, value: bool) -> Result<(), String> {
    let mut settings = read_settings(config_dir);
    settings.quit_stops_services = Some(value);
    write_settings(config_dir, &settings)
}

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_states_are_distinct() {
        // 没这一格 / 文件读不懂 / 不是布尔 ⇒ 一律「还没表态」，绝不默认成某一支。
        assert_eq!(parse_run_local_world(""), None);
        assert_eq!(parse_run_local_world("not json at all"), None);
        assert_eq!(parse_run_local_world("{}"), None);
        assert_eq!(parse_run_local_world("{\"runLocalWorld\": \"true\"}"), None);
        assert_eq!(parse_run_local_world("{\"other\": true}"), None);
        // 明确的表态才算是表态。
        assert_eq!(
            parse_run_local_world("{\"runLocalWorld\": true}"),
            Some(true)
        );
        assert_eq!(
            parse_run_local_world("{\"runLocalWorld\": false, \"x\": 1}"),
            Some(false)
        );
    }

    #[test]
    fn render_is_stable_and_round_trips() {
        for (run, quit) in [
            (Some(true), None),
            (Some(false), None),
            (Some(true), Some(false)),
            (None, Some(true)),
        ] {
            let settings = MachineSettings {
                run_local_world: run,
                quit_stops_services: quit,
            };
            let text = render_settings(&settings);
            assert!(text.ends_with('\n'));
            assert_eq!(parse_settings(&text), settings);
            assert_eq!(
                text,
                render_settings(&settings),
                "同一件事两次渲染必须逐字节相同"
            );
            // **没表态的键不落盘**（"没表态"与"表了 false"是两件事）。
            assert_eq!(text.contains("runLocalWorld"), run.is_some());
            assert_eq!(text.contains("quitStopsServices"), quit.is_some());
        }
        assert_eq!(render_settings(&MachineSettings::default()), "{}\n");
    }

    #[test]
    fn quit_behaviour_defaults_to_stopping() {
        // §8.3：默认不替用户做隐藏的事 ⇒ 没表态时"退出"就是**停本机服务**。
        assert!(MachineSettings::default().quit_stops_services_effective());
        assert!(!MachineSettings {
            quit_stops_services: Some(false),
            ..Default::default()
        }
        .quit_stops_services_effective());
        assert!(MachineSettings {
            quit_stops_services: Some(true),
            ..Default::default()
        }
        .quit_stops_services_effective());
    }

    #[test]
    fn writers_touch_only_their_own_key() {
        let dir = std::env::temp_dir().join(format!("celira-settings-rw-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        write_run_local_world(&dir, true).unwrap();
        write_quit_stops_services(&dir, false).unwrap();
        // 后写的那一格不许把前一格抹掉（读-改-写）。
        assert_eq!(read_run_local_world(&dir), Some(true));
        assert!(!read_quit_stops_services(&dir));
        write_run_local_world(&dir, false).unwrap();
        assert_eq!(read_run_local_world(&dir), Some(false));
        assert!(
            !read_quit_stops_services(&dir),
            "写另一格时这一格要原样留着"
        );
        let text = fs::read_to_string(settings_path(&dir)).unwrap();
        assert!(text.contains("runLocalWorld") && text.contains("quitStopsServices"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_then_read_round_trips_and_leaves_no_temp() {
        let dir = std::env::temp_dir().join(format!("celira-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(read_run_local_world(&dir), None, "目录都不在 ⇒ 还没表态");

        write_run_local_world(&dir, true).unwrap();
        assert_eq!(read_run_local_world(&dir), Some(true));
        write_run_local_world(&dir, false).unwrap();
        assert_eq!(read_run_local_world(&dir), Some(false));

        let leftovers: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "改名之后不该留下临时文件：{leftovers:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
