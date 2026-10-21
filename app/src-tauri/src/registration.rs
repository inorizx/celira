//! **住户登记**——「这支 Agent 在这台机器上」这份**机械事实**的唯一写者与唯一形状。
//!
//! ## 为什么有这一片（2026-10-03 刀）
//!
//! 一天里出现过两只"起家的手"（`home.js` 与 app 的 [`crate::anjia`]），各写各的一半：
//! 壳那只手建了环境根、签了凭据、起了进程，**却没写 `agent.conf`** ⇒ `node home.js status`
//! 看那支住户是「住户 ?，房 无 …… 还没安家」，`up` 还会想**再安一次**（可能起出第二个宿主）。
//! 病根不是"少写一个文件"，而是**同一份机械事实没有唯一写者**。
//!
//! 这一片把那份登记**收成一处**：形状的定义、读、写、序号分配、账本路径。
//! ⚠ 它写的全是**编排事实**（这台机器上的第几支住户、跟哪间房绑着、它的进程账本在哪）——
//! **不是主体的记忆**（判据 9：系统补能力缺口、不替认知判断；主体的经历与判断一个字都不在这里）。
//!
//! ## 形状权威是 `scripts/home/residents.js`（照抄，不发明）
//!
//! | 事实 | 落在哪 | 权威 |
//! |---|---|---|
//! | 支级登记 | `<环境根>/agent.conf`（`agent_id` / `room_id` / `slot` / `display_name`） | `residents.js` 的 `renderAgentConf` / `readAgentConf` |
//! | 住户序号 | `max(已有 slot) + 1`（**单调、绝不回收**） | `residents.js` 的 `allocateSlot` |
//! | 住户账本 | `<家>/logs/agents/<支>/host.pid` ＋ `host.out` | `paths.js` 的 `agentPidFile` / `agentHostLog` |
//!
//! 三条纪律逐条对齐 JS：
//!
//! 1. **已知那几行按规范顺序，别的行原样保留**（不吞别人的行 —— 与 `home.conf` 同一条）；
//! 2. **内容没变就一个字都不动**（mtime 是读数的一部分；重跑不重复写）；
//! 3. **读不出来不猜**：缺文件/缺键给稳定缺省，畸形值（如 `slot=abc`）按"缺席"。
//!
//! ## 房（`room_id`）从哪来
//!
//! **问世界**，不猜：`GET /v1/rooms` 里找 `room.kind == "direct"` 且 `peer.id == <这支>` 的那间
//! （`adopt.js` 的 `findExistingDirectRoom` 原样）。一间都没有 ⇒ 写空（形状允许 `room_id=`）。

use std::fs;
use std::path::{Path, PathBuf};

/// 支级登记的文件名（`paths.js` 的 `AGENT_CONF`）。
pub const AGENT_CONF: &str = "agent.conf";
/// 身份文件（有它才算"建过了"；`paths.js` 的 `identity.json`）。
pub const IDENTITY: &str = "identity.json";

/// 登记的四个键（**字面量逐字对齐** `residents.js` 的 `AGENT_CONF_KEYS`）。
const KEY_AGENT: &str = "agent_id";
const KEY_ROOM: &str = "room_id";
const KEY_SLOT: &str = "slot";
const KEY_NAME: &str = "display_name";

/// 一份支级登记（缺键给稳定缺省，不抛）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentConf {
    pub agent_id: String,
    /// 值班室（那间 `direct` 房）；读不到 ⇒ 空串（形状允许 `room_id=`）。
    pub room_id: String,
    /// 住户序号（`parseSlot`：正整数才算数）。
    pub slot: Option<u32>,
    pub display_name: String,
}

/// 切行（`residents.js` 的 `linesOfText` 直译）：去行尾 CR、去掉结尾换行带来的空串。
fn lines_of(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();
    if lines.last().map(|line| line.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines
}

/// 取一个键（只认行首 `键=`，值里可含 `=`；与 `conf.js` 的 `confGet` 同款）。
fn conf_get(text: &str, key: &str) -> String {
    let prefix = format!("{key}=");
    for line in lines_of(text) {
        if let Some(rest) = line.strip_prefix(&prefix) {
            return rest.to_string();
        }
    }
    String::new()
}

/// 解析一份登记的正文（`readAgentConf` 直译）。
pub fn parse_agent_conf(text: &str) -> AgentConf {
    AgentConf {
        agent_id: conf_get(text, KEY_AGENT).trim().to_string(),
        room_id: conf_get(text, KEY_ROOM).trim().to_string(),
        slot: parse_slot(&conf_get(text, KEY_SLOT)),
        display_name: conf_get(text, KEY_NAME).trim().to_string(),
    }
}

/// 序号判据（`parseSlot`）：整数且 ≥ 1，否则"缺席"。
pub fn parse_slot(raw: &str) -> Option<u32> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || !trimmed.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    match trimmed.parse::<u32>() {
        Ok(value) if value >= 1 => Some(value),
        _ => None,
    }
}

/// 渲染一份登记（`renderAgentConf` 直译，**逐字节对齐**）：
/// 已知四行按规范顺序在前（`display_name` 空则整行不写），**其余行原样保留**，末尾一个换行。
pub fn render_agent_conf(current: Option<&str>, values: &AgentConf) -> String {
    let known = [KEY_AGENT, KEY_ROOM, KEY_SLOT, KEY_NAME].map(|key| format!("{key}="));
    let kept: Vec<String> = lines_of(current.unwrap_or(""))
        .into_iter()
        .filter(|line| !known.iter().any(|prefix| line.starts_with(prefix.as_str())))
        .collect();
    let mut out = vec![
        format!("{KEY_AGENT}={}", values.agent_id),
        format!("{KEY_ROOM}={}", values.room_id),
        format!("{KEY_SLOT}={}", values.slot.unwrap_or(0)),
    ];
    if !values.display_name.is_empty() {
        out.push(format!("{KEY_NAME}={}", values.display_name));
    }
    out.extend(kept);
    format!("{}\n", out.join("\n"))
}

/// 读一支住户的登记（文件不在 ⇒ 缺省，不抛）。
pub fn read_agent_conf(env_root: &Path) -> AgentConf {
    match fs::read_to_string(env_root.join(AGENT_CONF)) {
        Ok(text) => parse_agent_conf(&text),
        Err(_) => AgentConf::default(),
    }
}

/// 支级资源上界的键（**与命令行旗标同名**：一个概念一个名称；`residents.js` 的
/// `AGENT_BUDGET_KEYS` 逐字对齐）。
const KEY_MAX_TURNS: &str = "max_turns";

/// 读一支住户的资源上界：`agent.conf` 的 `max_turns`。**缺省 = `None`**（调用方给内置缺省）。
///
/// 2026-10-05：这个值原先由两个宿主**各自写死**（`scripts/home/up.js` 与这里的
/// `build_host_args` 各写一个 `8`）⇒ 用户改不了，而且同一事实两个写者。现在**唯一权威是
/// `agent.conf` 这一份文件**，两个宿主只是读者。
///
/// ⚠ **值原样返回，不在这一层校验**：校验的权威是 `agent_host` 自己的旗标解析（非整数与 0
/// 都被拒，且有明确失败文本）。在这一层"顺手兜底成 8"会让**配置的值**与**运行的值**悄悄
/// 不一致 —— 那正是假绿；宁可让宿主带着原文去拒绝。
pub fn read_agent_budget(env_root: &Path) -> Option<String> {
    let text = fs::read_to_string(env_root.join(AGENT_CONF)).ok()?;
    let value = conf_get(&text, KEY_MAX_TURNS).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// 键名（`residents.js` 的 `HAND_KEYS.terminal`，**字面量逐字对齐**）。
const KEY_TERMINAL: &str = "terminal";

/// 读一支的**终端档声明**：`agent.conf` 的 `terminal=`（原文；没写/空 ⇒ `None`）。
///
/// ⚠ **规则不在这里**（本函数只读文件）：`full` 才给那只手、`none` 与"没写"都不给 ——
/// 判据的权威是 `scripts/home/residents.js` 的 `residentHands` ＋ `resolveTerminal`，
/// 消费方在 `anjia::host_process::terminal_tier`（那里把支级 ⇒ 家级 ⇒ 不给连起来）。
/// 值**原样**返回（大小写与空白由消费方按同一条规矩归一），认不出的取值当"没写"。
pub fn read_agent_terminal(env_root: &Path) -> Option<String> {
    let text = fs::read_to_string(env_root.join(AGENT_CONF)).ok()?;
    let value = conf_get(&text, KEY_TERMINAL).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// 写一支的**终端档声明**：**只动 `terminal=` 这一行**，其余行（`mcp_*` / `max_turns` /
/// 登记那几行）原样保留。
///
/// `Some("full" | "none")` ⇒ 文件里有这一行；`None` ⇒ 删掉它（回到"没写"＝平台缺省）。
/// 返回"这一次真的写了吗"（内容没变就一字不动 —— mtime 是读数的一部分）。
pub fn set_agent_terminal(env_root: &Path, value: Option<&str>) -> Result<bool, String> {
    let file = env_root.join(AGENT_CONF);
    let current = fs::read_to_string(&file)
        .map_err(|error| format!("这一支的登记读不出来（{}）：{error}", file.display()))?;
    let prefix = format!("{KEY_TERMINAL}=");
    let mut lines: Vec<String> = lines_of(&current)
        .into_iter()
        .filter(|line| !line.starts_with(&prefix))
        .collect();
    if let Some(value) = value {
        lines.push(format!("{KEY_TERMINAL}={value}"));
    }
    let text = if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    };
    if text == current {
        return Ok(false);
    }
    fs::write(&file, &text)
        .map_err(|error| format!("写登记失败（{}）：{error}", file.display()))?;
    Ok(true)
}

/// 写一份登记：**内容没变就一个字都不动**（`writeAgentConf` 直译）。
/// 返回"这一次真的写了吗"。
pub fn write_agent_conf(env_root: &Path, values: &AgentConf) -> Result<bool, String> {
    if values.agent_id.trim().is_empty() {
        return Err("登记的 agent_id 是空的 —— 不写（那会让这支住户认不出自己）".to_string());
    }
    if values.slot.is_none() {
        return Err("登记的 slot 缺席 —— 不写（序号是住户身份的一半，猜不得）".to_string());
    }
    let file = env_root.join(AGENT_CONF);
    let current = fs::read_to_string(&file).ok();
    let text = render_agent_conf(current.as_deref(), values);
    if current.as_deref() == Some(text.as_str()) {
        return Ok(false);
    }
    fs::create_dir_all(env_root)
        .map_err(|error| format!("建环境根失败（{}）：{error}", env_root.display()))?;
    fs::write(&file, text).map_err(|error| format!("写登记失败（{}）：{error}", file.display()))?;
    Ok(true)
}

// ---------------------------------------------------------------------------
// 序号分配与账本路径（`residents.js` / `paths.js`）
// ---------------------------------------------------------------------------

/// 这个家住着哪几支（**只认新布局** `<家>/agents/<支>/`：目录里**有 `agent.conf` 或
/// `identity.json`** 才算住户 —— 半成品/杂物不列）。
pub fn resident_env_roots(home_dir: &Path) -> Vec<PathBuf> {
    let root = home_dir.join("agents");
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(&root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.join(AGENT_CONF).is_file() || path.join(IDENTITY).is_file() {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// 下一个住户序号：`max(已有 slot) + 1`（**单调、绝不回收** —— `allocateSlot` 原话：
/// 回收会让布置键撞上旧记录）。老家的单支环境根（`<家>/env`）算 1。
pub fn allocate_slot(home_dir: &Path) -> u32 {
    let mut max = 0u32;
    if home_dir.join("env").join(IDENTITY).is_file() {
        max = 1;
    }
    for env_root in resident_env_roots(home_dir) {
        if let Some(slot) = read_agent_conf(&env_root).slot {
            max = max.max(slot);
        }
    }
    max + 1
}

/// 一支住户的账本目录：`<家>/logs/agents/<支>/`（`paths.js` 的 `agentLogDir`）。
pub fn host_log_dir(home_dir: &Path, agent_id: &str) -> PathBuf {
    home_dir.join("logs").join("agents").join(agent_id.trim())
}

/// 一支住户的宿主 pid 文件（`paths.js` 的 `agentPidFile`）。
pub fn host_pid_file(home_dir: &Path, agent_id: &str) -> PathBuf {
    host_log_dir(home_dir, agent_id).join("host.pid")
}

/// 一支住户的宿主日志（`paths.js` 的 `agentHostLog`）。
pub fn host_log_file(home_dir: &Path, agent_id: &str) -> PathBuf {
    host_log_dir(home_dir, agent_id).join("host.out")
}

/// 记一笔账（**谁起的谁记账**，不是 PID 认领）：`logs/agents/<支>/host.pid`。
pub fn write_pid(path: &Path, pid: u32) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("建账本目录失败（{}）：{error}", parent.display()))?;
    }
    fs::write(path, format!("{pid}\n"))
        .map_err(|error| format!("写账本失败（{}）：{error}", path.display()))
}

// ---------------------------------------------------------------------------
// 用例（形状与 JS 逐条对齐；不需要壳在场）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("celira-reg-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn render_matches_residents_js_line_for_line() {
        // 规范顺序：agent_id → room_id → slot → [display_name]，末尾一个换行。
        let conf = AgentConf {
            agent_id: "a-1".into(),
            room_id: "r-1".into(),
            slot: Some(2),
            display_name: "小月".into(),
        };
        assert_eq!(
            render_agent_conf(None, &conf),
            "agent_id=a-1\nroom_id=r-1\nslot=2\ndisplay_name=小月\n"
        );
        // display_name 空 ⇒ 整行不写（不是写空值）。
        let anonymous = AgentConf {
            display_name: String::new(),
            ..conf.clone()
        };
        assert_eq!(
            render_agent_conf(None, &anonymous),
            "agent_id=a-1\nroom_id=r-1\nslot=2\n"
        );
        // 房缺席 ⇒ `room_id=`（形状允许），不是把那一行丢掉。
        let roomless = AgentConf {
            room_id: String::new(),
            ..conf.clone()
        };
        assert_eq!(
            render_agent_conf(None, &roomless),
            "agent_id=a-1\nroom_id=\nslot=2\ndisplay_name=小月\n"
        );
        // **别人的行原样保留**（`note=` 那类），且重排只发生在我们认识的四行上。
        let existing = "note=这是主人留的\nslot=1\nagent_id=old\nhand=terminal\n";
        assert_eq!(
            render_agent_conf(Some(existing), &conf),
            "agent_id=a-1\nroom_id=r-1\nslot=2\ndisplay_name=小月\nnote=这是主人留的\nhand=terminal\n"
        );
        // CRLF 容错：读得懂，写回统一 LF。
        assert_eq!(
            render_agent_conf(Some("agent_id=old\r\nnote=x\r\n"), &conf),
            "agent_id=a-1\nroom_id=r-1\nslot=2\ndisplay_name=小月\nnote=x\n"
        );
    }

    #[test]
    fn write_is_idempotent_and_refuses_without_slot() {
        let home = temp_home("write");
        let env_root = home.join("agents").join("a-1");
        let conf = AgentConf {
            agent_id: "a-1".into(),
            room_id: "r-1".into(),
            slot: Some(1),
            display_name: "小月".into(),
        };
        assert!(write_agent_conf(&env_root, &conf).unwrap(), "第一次要写");
        assert!(
            !write_agent_conf(&env_root, &conf).unwrap(),
            "内容没变 ⇒ 一个字都不动"
        );
        // 序号缺席 ⇒ 拒绝（不写半个身份）。
        let no_slot = AgentConf {
            slot: None,
            ..conf.clone()
        };
        assert!(write_agent_conf(&env_root, &no_slot).is_err());
        // 空 id ⇒ 拒绝。
        let no_id = AgentConf {
            agent_id: String::new(),
            ..conf.clone()
        };
        assert!(write_agent_conf(&env_root, &no_id).is_err());

        let read = read_agent_conf(&env_root);
        assert_eq!(read, conf);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn slot_parsing_is_strict_and_allocation_is_monotonic() {
        assert_eq!(parse_slot("1"), Some(1));
        assert_eq!(parse_slot(" 12 "), Some(12));
        assert_eq!(parse_slot("0"), None, "0 不是住户序号");
        assert_eq!(parse_slot("-1"), None);
        assert_eq!(parse_slot("abc"), None);
        assert_eq!(parse_slot(""), None);
        assert_eq!(parse_slot("2.5"), None);

        let home = temp_home("slot");
        assert_eq!(allocate_slot(&home), 1, "一支都没有 ⇒ 第一支是 1");
        for (id, slot) in [("a", 1u32), ("b", 2), ("c", 3)] {
            let env_root = home.join("agents").join(id);
            fs::create_dir_all(&env_root).unwrap();
            write_agent_conf(
                &env_root,
                &AgentConf {
                    agent_id: id.into(),
                    room_id: String::new(),
                    slot: Some(slot),
                    display_name: String::new(),
                },
            )
            .unwrap();
        }
        assert_eq!(allocate_slot(&home), 4, "max + 1");
        // 序号**不回收**：删掉中间那支，下一个仍然是 max+1。
        fs::remove_dir_all(home.join("agents").join("a")).unwrap();
        assert_eq!(allocate_slot(&home), 4);
        // 半成品目录（既无 agent.conf 也无 identity.json）不算住户。
        fs::create_dir_all(home.join("agents").join("junk")).unwrap();
        assert_eq!(allocate_slot(&home), 4);
        // 老家单支（`<家>/env` 有 identity.json）算 1 ⇒ 连它一起 max。
        fs::create_dir_all(home.join("env")).unwrap();
        fs::write(home.join("env").join(IDENTITY), "{}").unwrap();
        assert_eq!(allocate_slot(&home), 4);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn ledger_paths_follow_paths_js() {
        let home = Path::new("/home/u/celira-home");
        let expected_dir = home.join("logs").join("agents").join("a-1");
        assert_eq!(host_log_dir(home, "a-1"), expected_dir);
        assert_eq!(host_pid_file(home, "a-1"), expected_dir.join("host.pid"));
        assert_eq!(host_log_file(home, "a-1"), expected_dir.join("host.out"));
        // 前后空白一律对齐（id 是目录名，脏 id 会把账本写到别处）。
        assert_eq!(
            host_pid_file(home, "  a-1  "),
            expected_dir.join("host.pid")
        );
    }
}
