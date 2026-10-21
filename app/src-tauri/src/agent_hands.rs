//! **这一支在这台机器上的手**（终端档）——壳 ↔ 页面那一格的读写面。
//!
//! ## 它是什么、不是什么
//!
//! `agent.conf` 的 `terminal=` 是**支级授权**（一支一张票），执行者只有一个：宿主起进程时
//! 读它（`anjia::host_process::terminal_tier`）。本模块**只读写那一行**——不解释档位、不停/起
//! 任何进程。生效点是**下一次启动**（页面把那句话如实印在开关旁边，不假装立刻生效）。
//!
//! 三条边界：
//!
//! - **只认本机**：手是这台机器给的（`home_facts::resolve_home_dir(None)`）——不走家钥匙、
//!   不走远程、不进世界。换一台机器，这一页说的是那一台的事（页面上这一格只在"住在这台"时出现）；
//! - **只动一行**：`agent_id` / `room_id` / `slot` / `display_name` / `max_turns` / `mcp_*`
//!   原样保留（写者唯一的权威是 [`crate::registration::set_agent_terminal`]）；
//! - **不住在这儿就不给**：环境根里没有 `agent.conf` ⇒ `homed: false`，写操作明确拒绝
//!   （不替一台没住的机器造一份登记）。
//!
//! ## 词表
//!
//! `full` / `none`（缺省 = 没写 = 不给）——与 `scripts/home/residents.js` 的 `resolveTerminal`
//! 同一条。**页面只认 `granted` 这个布尔**，不认档位字符串（档位是机器侧的事）。
//!
//! ## 这一层不做的事
//!
//! 不做"开完立刻生效"（那要停/起这一支，另是一格）；不做家级默认（`home.conf` 的 `terminal=`）；
//! 不做远程机器的档位（远程那一页要它自己的状态口，不是这条命令）。

use std::path::Path;

use serde::Serialize;
use tauri::command;

use crate::home_facts;
use crate::registration;

/// 档位的两个取值（v1 只有这两档；与 `residents.js` 的 `resolveTerminal` 同一条）。
const TERMINAL_FULL: &str = "full";
const TERMINAL_NONE: &str = "none";

/// **本平台有没有终端臂**（与 `anjia::host_process::HostPlatform::has_terminal_arm` 同一条：
/// 只有 Windows 与 Linux 有；别的平台上写了 `full` 也不会有手）。
const HAS_TERMINAL_ARM: bool = cfg!(any(target_os = "windows", target_os = "linux"));

/// 这一页要的那份读数（字段名 camelCase，页面直接吃）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandsReadout {
    pub agent_id: String,
    /// 这一支住不住在**这台**机器上（判据 = 它的环境根里有 `agent.conf`）。
    pub homed: bool,
    /// `agent.conf` 里 `terminal=` 的原文（没写 ⇒ `None`）——只作读数，页面不拿它判档。
    pub declared: Option<String>,
    /// 开了没有（原文归一后是 `full`）。
    pub granted: bool,
    /// 本平台有没有终端臂（没有 ⇒ 开关不给，只怕人以为开了有用）。
    pub arm: bool,
}

fn readout_for(home_dir: &Path, agent_id: &str) -> HandsReadout {
    let agent_id = agent_id.trim();
    let env_root = home_facts::agent_env_root(home_dir, agent_id);
    let homed = env_root.join(registration::AGENT_CONF).is_file();
    let declared = if homed {
        registration::read_agent_terminal(&env_root)
    } else {
        None
    };
    let granted = {
        // **支级 ⇒ 家级 ⇒ 不给**（与 `residents.js` 的 `resolveTerminal` 同一条）：这一支没写时，
        // 家级 `terminal=` 才是生效值。少了这一句，界面上会显示"关着"而主体其实有手。
        let effective = declared
            .clone()
            .or(home_facts::read_conf(home_dir).terminal);
        effective
            .as_deref()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case(TERMINAL_FULL))
    };
    HandsReadout {
        agent_id: agent_id.to_string(),
        homed,
        declared,
        granted,
        arm: HAS_TERMINAL_ARM,
    }
}

fn set_for(home_dir: &Path, agent_id: &str, granted: bool) -> Result<HandsReadout, String> {
    let agent_id = agent_id.trim();
    let env_root = home_facts::agent_env_root(home_dir, agent_id);
    if !env_root.join(registration::AGENT_CONF).is_file() {
        return Err("这一支不住在这台机器上（它的环境根里没有 agent.conf）——没有可改的档".into());
    }
    let value = if granted {
        TERMINAL_FULL
    } else {
        TERMINAL_NONE
    };
    registration::set_agent_terminal(&env_root, Some(value))?;
    Ok(readout_for(home_dir, agent_id))
}

/// 读这一支的手（不住在这台机器上 ⇒ `homed: false`，页面据此不摆这一格）。
#[command]
pub fn hands_readout(agent_id: String) -> HandsReadout {
    readout_for(&home_facts::resolve_home_dir(None), &agent_id)
}

/// 给 / 收这一支的手：**只改 `agent.conf` 的 `terminal=` 那一行**。
///
/// ⚠ 生效在**下一次启动**（宿主起进程时读它）——本命令不停、也不起任何进程。
#[command]
pub fn hands_set(agent_id: String, granted: bool) -> Result<HandsReadout, String> {
    set_for(&home_facts::resolve_home_dir(None), &agent_id, granted)
}

// ---------------------------------------------------------------------------
// 用例：文件是唯一的证据（不 mock）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "celira-hands-{}-{}-{}",
            std::process::id(),
            tag,
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn home_with_conf(tag: &str, body: &str) -> (PathBuf, PathBuf) {
        let home = temp_home(tag);
        let env_root = home_facts::agent_env_root(&home, "a-1");
        fs::create_dir_all(&env_root).unwrap();
        fs::write(env_root.join(registration::AGENT_CONF), body).unwrap();
        (home, env_root)
    }

    #[test]
    fn granting_writes_exactly_one_line_and_keeps_every_other_line() {
        let (home, env_root) = home_with_conf(
            "grant",
            "agent_id=a-1\nroom_id=r-1\nslot=1\ndisplay_name=小可\nmax_turns=12\nmcp_browser=stdio:npx x\n",
        );
        assert!(!readout_for(&home, "a-1").granted, "没写就是不作为");
        let after = set_for(&home, "a-1", true).expect("开");
        assert!(after.granted);
        let text = fs::read_to_string(env_root.join(registration::AGENT_CONF)).unwrap();
        assert!(text.contains("terminal=full"), "{text}");
        // 别的行一个字节都不许丢（`mcp_*` 尤其：它是另一份授权）。
        for line in [
            "agent_id=a-1",
            "room_id=r-1",
            "slot=1",
            "display_name=小可",
            "max_turns=12",
            "mcp_browser=stdio:npx x",
        ] {
            assert!(text.contains(line), "丢了这一行：{line}\n{text}");
        }
        assert_eq!(text.matches("terminal=").count(), 1, "只许有一行");
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn revoking_writes_none_and_second_identical_write_touches_nothing() {
        let (home, env_root) = home_with_conf("revoke", "agent_id=a-1\nroom_id=\nslot=1\n");
        set_for(&home, "a-1", true).expect("开");
        let revoked = set_for(&home, "a-1", false).expect("收");
        assert!(!revoked.granted);
        let text = fs::read_to_string(env_root.join(registration::AGENT_CONF)).unwrap();
        assert!(text.contains("terminal=none"), "{text}");
        // 同一份内容再写一次 ⇒ 报告"没写"（mtime 是读数的一部分）。
        assert_eq!(
            registration::set_agent_terminal(&env_root, Some("none")).unwrap(),
            false,
            "内容没变就不许碰文件"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn the_home_default_counts_when_the_resident_says_nothing() {
        let (home, _env_root) = home_with_conf("home-default", "agent_id=a-1\nroom_id=\nslot=1\n");
        fs::write(home.join("home.conf"), "terminal=full\n").unwrap();
        assert!(
            readout_for(&home, "a-1").granted,
            "支级没写 ⇒ 家级 full 才是生效值（界面上不许显示成关着）"
        );
        // 支级写 none ⇒ 覆盖家级（"这一支不要"是显式的）。
        set_for(&home, "a-1", false).unwrap();
        assert!(!readout_for(&home, "a-1").granted);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn a_machine_without_the_registration_is_not_homed_and_the_write_is_refused() {
        let home = temp_home("unhomed");
        let readout = readout_for(&home, "a-1");
        assert!(!readout.homed, "没有 agent.conf 就不算住在这儿");
        assert!(!readout.granted);
        let refused = set_for(&home, "a-1", true).expect_err("不许替没住的机器造一份登记");
        assert!(refused.contains("不住在这台机器上"), "{refused}");
        assert!(
            !home_facts::agent_env_root(&home, "a-1").exists(),
            "拒绝之后什么都不许留下"
        );
        let _ = fs::remove_dir_all(&home);
    }
}
