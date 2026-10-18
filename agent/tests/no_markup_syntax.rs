//! **给非渲染读者的文本里不许有标记语法**（2026-10-13）。
//!
//! 判据一句话：**标记语法只在会被渲染的地方才有意义**——主体读到的是**原样文本**（没有渲染器），
//! 操作者读到的是**控制台**（同样不渲染）⇒ `**上限**` 里的星号不是"加粗"，而是**多出来的两个字节**，
//! 还可能被读成正文的一部分。这与"门牌 = 标识 + 一句人话"是同一条纪律的另一面：
//! **给主体的文本用标识符与结构化字段**。
//!
//! ## 射程
//!
//! 它扫 `agent/src/**.rs`，判据是**机械的**：一行里出现 `**` 且**落在字符串字面量里**
//! （逐字符跟踪引号配对，见 [`stars_in_literals`]）⇒ 违规。**注释里的 `**` 不管**：那是给写代码
//! 的人看的，那里本来就用标记语法。
//!
//! ⚠ **它没有豁免名单**（2026-10-13 第二刀撤掉的）：第一版把"读者是操作者"的那几个文件列名豁免，
//! 理由是"那些行的判据是人不许比系统知道得更少，而不是主体读到的是不是原样文本"。**那个理由站不住**
//! ——控制台也不渲染标记语法，那 46 处同样是噪音。⇒ 清掉之后**名单空了**，守卫因此**更强**：
//! 从今往后 `agent/src` 里任何一处字面量出现 `**` 都红，**不必再判断"这段文本是给谁的"**。
//!
//! ⚠ 它也**不量**"这句话是不是人话"、"有没有比喻"：那些判据在 `minimal_agent.rs` 的声音纪律那几条。

use std::path::{Path, PathBuf};

/// 这一行里**落在字符串字面量内**的 `**` 出现次数。
///
/// ⚠ 判据必须是"**在不在字面量里**"，不能图省事写成"这一行有没有星号"：Rust 代码里 `**name` 是
/// **两次解引用**（`mcp/app.rs` 的 `contains_key(**name)` 就是），把它算成标记语法会让守卫误报，
/// 而且会让"按同一判据清理"的那一步**改坏真代码**——本刀实测过一次，已还原重做。
fn stars_in_literals(line: &str) -> usize {
    let mut inside = false;
    let mut count = 0usize;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            // 转义：连同下一个字符一起跳过（`\"` 不翻转配对）。
            '\\' => {
                chars.next();
            }
            '"' => inside = !inside,
            '*' if inside && chars.peek() == Some(&'*') => {
                chars.next();
                count += 1;
            }
            _ => {}
        }
    }
    count
}

/// 递归收集 `agent/src` 下的全部 `.rs`（相对路径用 `/` 归一，报违规时好定位）。
fn sources(root: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            found.push((relative, path));
        }
    }
    found.sort();
    found
}

/// **每一个字面量都不含标记语法**：一次报**全部**违规（不是撞到第一条就停），
/// 这样修的时候能一刀清完。
#[test]
fn no_markup_syntax_in_any_literal() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders: Vec<String> = Vec::new();
    let mut scanned = 0usize;

    for (relative, path) in sources(&root) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        scanned += 1;
        for (index, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                // 注释：给写代码的人看的，标记语法在这里合法。
                continue;
            }
            if stars_in_literals(line) > 0 {
                offenders.push(format!("{relative}:{}: {}", index + 1, line.trim()));
            }
        }
    }

    assert!(scanned > 0, "一个文件都没扫到——判据在空跑");
    assert!(
        offenders.is_empty(),
        "字面量里出现标记语法（{} 处）：\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}
