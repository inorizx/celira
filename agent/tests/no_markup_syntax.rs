//! **给非渲染读者的文本里不许有标记语法**（2026-10-13；2026-10-19 改成**跨行**认字面量）。
//!
//! 判据一句话：**标记语法只在会被渲染的地方才有意义**——主体读到的是**原样文本**（没有渲染器），
//! 操作者读到的是**控制台**（同样不渲染）⇒ `**上限**` 里的星号不是"加粗"，而是**多出来的两个字节**，
//! 还可能被读成正文的一部分。这与"门牌 = 标识 + 一句人话"是同一条纪律的另一面：
//! **给主体的文本用标识符与结构化字段**。
//!
//! ## 射程
//!
//! 它扫 `agent/src/**.rs`，判据是**机械的**：一个 `**` **落在字符串字面量里** ⇒ 违规。
//! **注释里的 `**` 不管**：那是给写代码的人看的，那里本来就用标记语法。
//!
//! ⚠ **它没有豁免名单**（2026-10-13 第二刀撤掉的）：第一版把"读者是操作者"的那几个文件列名豁免，
//! 理由是"那些行的判据是人不许比系统知道得更少，而不是主体读到的是不是原样文本"。**那个理由站不住**
//! ——控制台也不渲染标记语法，那 46 处同样是噪音。⇒ 清掉之后**名单空了**，守卫因此**更强**：
//! 从今往后 `agent/src` 里任何一处字面量出现 `**` 都红，**不必再判断"这段文本是给谁的"**。
//!
//! ## ⚠ 2026-10-19：旧版是**逐行**扫的，多行字面量的**续行**全漏
//!
//! 旧版的 [`stars_in_literals`] 在**一行之内**配对引号 ⇒ 一个长字面量写成
//! `"...第一行\\` ＋ 续行时，**续行上的 `**` 根本不在它的视野里**（那一行的引号数为 0）。
//! 于是"名单空了、判据更强"只对**起头那一行**成立：实测**续行上还留着 879 处**
//! （18 个文件，`im_app.rs` 一处就 311）。⇒ 本刀把判据改成**跨行**（一次走完整份源码，
//! 引号配对跨行接得上），并把那 879 处**同一刀清干净**——判据因此名副其实：
//! **一处也不许有**（没有债务账、没有豁免名单）。
//!
//! ⚠ 它也**不量**"这句话是不是人话"、"有没有比喻"：那些判据在 `minimal_agent.rs` 的声音纪律那几条。

use std::path::{Path, PathBuf};

/// 扫一份源码，交出**落在字符串字面量里的 `**`**：`(行号, 那一行的原文)`。
///
/// ## 为什么不是"这一行有没有星号"
///
/// 判据必须是"**在不在字面量里**"，不能图省事写成"这一行有没有星号"：Rust 代码里 `**name` 是
/// **两次解引用**（`mcp/app.rs` 的 `contains_key(**name)` 就是），把它算成标记语法会让守卫误报，
/// 而且会让"按同一判据清理"的那一步**改坏真代码**——2026-10-13 实测过一次，已还原重做。
///
/// ## 它认得出什么（每一条都有反例自检，见下面的用例）
///
/// - **跨行字面量**：引号配对跨行接得上（2026-10-19 修的那一格——旧版逐行扫，续行全漏）；
/// - `//` 行注释与 `/* */` 块注释（含嵌套）：注释里的标记语法合法，**不参与**；
/// - 转义：`\"` 不结束字面量；`\` ＋ 换行是**续行**（换行本身照旧算一行）；
/// - **字符字面量**（`'"'` 是三个字符，里面的引号**不开始**一个字符串）与**生命周期**（`'a`）；
/// - **原始字符串**：`r"…"` / `r#"…"#` / `br#"…"#`（里面的引号不结束它）。
fn stars_in_literals(text: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let lines: Vec<&str> = text.lines().collect();
    let at = |index: usize| chars.get(index).copied();
    let mut offenders: Vec<(usize, String)> = Vec::new();
    // 在字符串字面量里吗；原始字符串的 `#` 个数（0 = 普通字面量）；块注释的嵌套深度。
    let mut inside = false;
    let mut raw_hashes = 0usize;
    let mut block_depth = 0usize;
    let mut line = 1usize;
    let mut index = 0usize;
    while index < chars.len() {
        let ch = chars[index];
        if ch == '\n' {
            line += 1;
            index += 1;
            continue;
        }
        if block_depth > 0 {
            if ch == '/' && at(index + 1) == Some('*') {
                block_depth += 1;
                index += 2;
                continue;
            }
            if ch == '*' && at(index + 1) == Some('/') {
                block_depth -= 1;
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }
        if inside {
            if raw_hashes > 0 {
                // 原始字符串：只有 `"` 后面跟着**同样多**的 `#` 才算结束。
                let closes =
                    ch == '"' && (0..raw_hashes).all(|offset| at(index + 1 + offset) == Some('#'));
                if closes {
                    index += 1 + raw_hashes;
                    inside = false;
                    raw_hashes = 0;
                    continue;
                }
                // ⚠ 原始字符串**也是字面量**：里面的 `**` 一样要报（与别的字面量同一条判据）。
                if ch == '*' && at(index + 1) == Some('*') {
                    offenders.push((line, lines.get(line - 1).unwrap_or(&"").to_string()));
                    index += 2;
                    continue;
                }
                index += 1;
                continue;
            }
            if ch == '\\' {
                // 转义：连下一个字符一起跳过。⚠ **换行是例外**（续行）：它照旧算一行。
                index += if at(index + 1) == Some('\n') { 1 } else { 2 };
                continue;
            }
            if ch == '"' {
                inside = false;
                index += 1;
                continue;
            }
            if ch == '*' && at(index + 1) == Some('*') {
                offenders.push((line, lines.get(line - 1).unwrap_or(&"").to_string()));
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }
        // 不在字面量里。
        if ch == '/' && at(index + 1) == Some('/') {
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if ch == '/' && at(index + 1) == Some('*') {
            block_depth = 1;
            index += 2;
            continue;
        }
        if ch == '"' {
            inside = true;
            index += 1;
            continue;
        }
        if ch == '\'' {
            // 三种写法分得开：`'\''` 这类**转义字符字面量**、`'a'` 这类**字符字面量**、
            // 以及 `'a` 这类**生命周期**（它没有收尾引号，不能一路吞到下一个 `'`）。
            if at(index + 1) == Some('\\') {
                let mut end = index + 2;
                while end < chars.len() && chars[end] != '\'' {
                    end += 1;
                }
                index = end + 1;
                continue;
            }
            if at(index + 2) == Some('\'') {
                index += 3;
                continue;
            }
            index += 1;
            continue;
        }
        if ch == 'r' || ch == 'b' {
            // 原始字符串（`r"…"` / `r#"…"#`；`br#"…"#` 从 `b` 起算）。
            let mut cursor = index + 1;
            if ch == 'b' {
                if at(cursor) != Some('r') {
                    index += 1;
                    continue;
                }
                cursor += 1;
            }
            let mut hashes = 0usize;
            while at(cursor) == Some('#') {
                hashes += 1;
                cursor += 1;
            }
            if at(cursor) == Some('"') {
                inside = true;
                raw_hashes = hashes;
                index = cursor + 1;
                continue;
            }
            index += 1;
            continue;
        }
        index += 1;
    }
    offenders
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
        for (line, source) in stars_in_literals(&text) {
            offenders.push(format!("{relative}:{line}: {}", source.trim()));
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

/// **判据自检**：它认得出真违规，也**不许误报**——每一条都对应真代码里出现过的一种写法。
///
/// ⚠ 数的是 **`**` 这个两字符序列出现了几次**：`**粗**` 是**两次**（起头一次、收尾一次），
/// 因此下面真违规那一组期望的都是 `2`。
/// ⚠ 第一条就是 2026-10-19 修的那一格：**续行上的 `**`**（旧版逐行扫，看不见）。
#[test]
fn the_markup_scanner_recognises_real_literals() {
    let hits = |text: &str| stars_in_literals(text).len();

    // 真违规：起头那一行、以及**续行**（旧版漏的就是这一格）。
    assert_eq!(hits("let a = \"**粗**\";"), 2);
    assert_eq!(
        hits("let a = \"第一行\\\n**第二行**\";"),
        2,
        "续行也要认出来"
    );
    assert_eq!(
        hits("let a = \"第一行\\\n第二行\\\n**第三行**\";"),
        2,
        "跨两行的续行"
    );
    // 转义引号不结束字面量 ⇒ 后面的 `**` 仍在里面。
    assert_eq!(hits("let a = \"她说\\\"**粗**\\\"\";"), 2);
    // 原始字符串里的 `**` 也在里面（它里面的引号不结束它）。
    assert_eq!(hits("let a = r#\"{\"k\":\"**粗**\"}\"#;"), 2);
    // 字符字面量里的引号**不开始**一个字符串 ⇒ 后面那个字面量照旧被认出来。
    assert_eq!(hits("let q = '\"'; let a = \"**粗**\";"), 2);
    // 生命周期同理（`'a` 不能一路吞到下一个 `'`）。
    assert_eq!(hits("fn f<'a>(x: &'a str) -> &'a str { \"**粗**\" }"), 2);

    // 不许误报：注释（行注释与块注释）、两次解引用、以及干净的字面量。
    assert_eq!(hits("// **注释里的标记语法合法**"), 0);
    assert_eq!(hits("/* **块注释也一样** */"), 0);
    assert_eq!(hits("let x = **name;"), 0, "两次解引用不是标记语法");
    assert_eq!(hits("let a = \"干净的\";"), 0);
    assert_eq!(hits("let q = '\"';"), 0);
}
