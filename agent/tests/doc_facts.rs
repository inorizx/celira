//! **文档事实守卫**：`README.md` 里点名的每一个"东西"都必须真的在树里。
//!
//! ## 它治的是哪一类病（2026-10-06 那一刀之前的真实形状）
//!
//! 权威层与工作树脱节，而且**没有任何东西会因此报红**：§2 的代码地图里还列着两个已经删掉的
//! 模块（`src/pending_effects.rs` / `src/screen.rs`），`lib.rs` 把已实现的东西写成"未实现"，
//! 守卫名单漏项、词表名字改过而文档没跟上。后果不是"文档难看"——是**每一轮开工都要重新核一遍
//! 事实**：预算花在复核上，不花在能用的东西上。
//!
//! ## 三条判据（都是机械的，都可判错）
//!
//! 1. **`src/` 下的每个 `.rs` 都在 §2 的代码地图里有一行**（`src/mcp/` 这种目录行覆盖它的子树）
//!    ——新增模块必须被人看见，删掉模块必须把那一行删掉；
//! 2. **正文里以 `src/<名字>.rs` 出现的路径必须存在**——历史提及走显式标记：写在 `~~…~~` 里
//!    的路径**不算**（这是全篇唯一的豁免，而且它必须被人看见，不是一条安静的忽略名单）。
//!    ⚠ **「以 `src/` 出现」是字面意思**：`app/src-tauri/src/home_doors.rs` 里的第二个 `src/`
//!    是**更长路径的一段**，不算（2026-10-18 修的一处假阳性——它把一份**写得完全正确**的
//!    文档判成了违规）；
//! 3. **§2 行里点名的代码符号必须在 `src/` 或 `tests/` 里找得到**——改名之后文档不改就会红。
//!
//! ## 它**证不到**什么（别把它当"文档说的都对"）
//!
//! 它只证明"**被点名的名字存在**"：一句话完全可以是假的（例如某行说"某某是唯一判据"而代码里
//! 有第二处判据），它一个字都管不了；它也不检查 §3 的散文（那是逐条人工核验的地盘，见 README
//! §4 的登记表）。**名字存在 ≠ 句子为真**——这条界线写在这里，免得下一个人把它当成完整保证。
//!
//! ## 判据怎么定义"代码符号"（没有白名单，规则可判错）
//!
//! 反引号里的一格，去掉尾部的 `()` 之后按 `::` 拆开，逐段判：
//!
//! - 全大写 + 下划线且**长度 ≥ 8** ⇒ 常量（`MAX_ID_BYTES`；`TASK_77` 这类标签因为长度不够被跳过）；
//! - 全小写 + 下划线且**长度 ≥ 4** ⇒ 函数 / 模块（`is_open` / `drive_run`）；
//! - 首字母大写且**含小写→大写驼峰** ⇒ 类型（`ModelOutput`；单个词 `Run` / `Turn` 是领域词，跳过）；
//! - 其余（含 `-`、`/`、`=`、空格、`<` 的）⇒ 不是符号，跳过。
//!
//! 命中判据的符号，用**词边界**在 `src/` ＋ `tests/` 的全部源码里找（裸子串会把 `Run` 匹配到
//! `Runner`，所以边界要判）。找不到就是一条违规，**报出文件与行号**，不静默通过。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// 读盘与清单
// ---------------------------------------------------------------------------

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("读不到 {}：{error}", path.display()))
}

/// `src/` 下**所有** `.rs`（含 `bin/` 与子目录），相对 `src/` 的路径统一成 `/` 分隔。
fn source_files(src_root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    collect(src_root, src_root, &mut found);
    found.sort();
    found
}

fn collect(root: &Path, directory: &Path, found: &mut Vec<String>) {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("列不了 {}：{error}", directory.display()));
    for entry in entries {
        let path = entry.expect("目录项读得出来").path();
        if path.is_dir() {
            collect(root, &path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let relative = path.strip_prefix(root).expect("在根之下");
            found.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// `src/` ＋ `tests/` 的全部**代码**（行注释已剥）拼在一起——符号存在的判据面。
///
/// ⚠ **本文件自己被排除**：守卫自己的夹具字符串（例如反例里那个不存在的类型名）不是产品代码，
/// 留在判据面里会让"假符号"被自己的夹具匹配到——那是自指造成的假阴性，不是判据宽。
fn source_haystack(agent_root: &Path) -> String {
    let mut text = String::new();
    for directory in ["src", "tests"] {
        let root = agent_root.join(directory);
        let mut files = Vec::new();
        collect(&root, &root, &mut files);
        for relative in files {
            if relative == "doc_facts.rs" {
                continue;
            }
            for line in read_text(&root.join(&relative)).lines() {
                text.push_str(&strip_line_comment(line));
                text.push('\n');
            }
        }
    }
    text
}

/// 剥掉行注释（`//` 之后），**但字符串字面量里的 `//` 不算**（例如 URL）。
///
/// 为什么要剥：判据是"这个名字在**代码**里存在"。不剥的话，一个已经退役、只在注释里留名的
/// 符号会被读成"还在"——那正是这条守卫要抓的漂移。
///
/// **已知的保守方向**：生命周期记号（`&'a str`）会让本函数以为进了字符字面量，于是那一行
/// 的注释剥不掉（少剥 ⇒ 偏宽，不会误报）。宁可偏宽，也不误伤真代码。
fn strip_line_comment(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut index = 0;
    let mut in_string = false;
    let mut in_char = false;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' if in_string || in_char => {
                index += 2;
                continue;
            }
            b'"' if !in_char => in_string = !in_string,
            b'\'' if !in_string => in_char = !in_char,
            b'/' if !in_string && !in_char && bytes.get(index + 1) == Some(&b'/') => {
                return line[..index].to_string();
            }
            _ => {}
        }
        index += 1;
    }
    line.to_string()
}

// ---------------------------------------------------------------------------
// README 的解析
// ---------------------------------------------------------------------------

/// §2 代码地图的行：从表头 `| 文件 | 当前事实 |` 起，到第一行不以 `|` 开头为止。
fn code_map_rows(readme: &str) -> Vec<(usize, String)> {
    let mut rows = Vec::new();
    let mut inside = false;
    for (index, raw) in readme.lines().enumerate() {
        let line = raw.trim_end_matches('\r');
        if !inside {
            if line.starts_with("| 文件 |") {
                inside = true;
            }
            continue;
        }
        if !line.starts_with('|') {
            break;
        }
        if line.starts_with("|---") {
            continue;
        }
        rows.push((index + 1, line.to_string()));
    }
    rows
}

/// 去掉 `~~…~~` 段：**历史提及**的唯一豁免形状。
fn without_history(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut inside = false;
    let mut rest = line;
    while let Some(at) = rest.find("~~") {
        if !inside {
            // 只在"历史之外"保留：`~~` 之间的那一段整个丢掉。
            out.push_str(&rest[..at]);
        }
        inside = !inside;
        rest = &rest[at + 2..];
    }
    if !inside {
        out.push_str(rest);
    }
    out
}

/// 一行里出现的 `src/…` 记号（`src/` 后面的合法路径字符：字母数字 `_` `.` `/` `-`）。
///
/// **目录记号也返回**（`src/mcp/`）：§2 允许用一行目录行覆盖它的整棵子树。
fn src_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut rest = line;
    while let Some(at) = rest.find("src/") {
        let tail = &rest[at..];
        let end = tail
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-')))
            .map(|(index, _)| index)
            .unwrap_or(tail.len());
        // ⚠ 只在 `src/` **起始一个词**时才收（2026-10-18 修一处假阳性）。
        //
        // **原约束**（判据 2 的原话）：「正文里**以 `src/<名字>.rs` 出现**的路径必须存在」。
        // **真实消费者**：`every_src_path_named_in_the_readme_exists`。
        // **修的是什么**：`app/src-tauri/src/home_doors.rs` 里的第二个 `src/` **不是**
        // 「以 `src/` 出现」——它是**更长路径里的一段**（`app/src-tauri/` 的 `src` 后面是 `-`，
        // 所以 `find("src/")` 命中的是后面那一个）。照旧实现，它被截成 `src/home_doors.rs`、
        // 拿去 `agent/` 下查存在性 ⇒ **一份写得完全正确的文档被判成违规**。
        // **新边界**：**前一个字符若是路径字符**（字母 / 数字 / `_` / `.` / `/` / `-`），
        // 这个 `src/` 属于更长的路径，**不收**。
        // **越界反例照旧要红**：真的写了 `src/foo.rs` 而 `agent/src/foo.rs` 不在 ⇒ 仍然报违规
        // （钉在 `the_doc_scanners_report_violations_when_they_exist` 的 ⑤ 里）。
        let absolute = line.len() - rest.len() + at;
        let starts_token = match line[..absolute].chars().next_back() {
            None => true,
            Some(c) => !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-')),
        };
        if starts_token {
            tokens.push(tail[..end].to_string());
        }
        rest = &tail[end.max(1)..];
    }
    tokens
}

/// 一行里的反引号格子（`` `…` ``）。
fn backticked(line: &str) -> Vec<String> {
    let mut spans = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else { break };
        spans.push(after[..close].to_string());
        rest = &after[close + 1..];
    }
    spans
}

/// 一格是不是"代码符号"（判据见文件头：没有白名单）。
fn is_code_symbol(token: &str) -> bool {
    let token = token.trim_end_matches("()");
    if token.is_empty() || !token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return false;
    }
    let has_underscore = token.contains('_');
    let letters: Vec<char> = token.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    let all_caps = !letters.is_empty() && letters.iter().all(|c| c.is_ascii_uppercase());
    let all_lower = letters.iter().all(|c| c.is_ascii_lowercase());
    let camel = token.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && letters.iter().any(|c| c.is_ascii_lowercase())
        && token
            .chars()
            .zip(token.chars().skip(1))
            .any(|(a, b)| a.is_ascii_lowercase() && b.is_ascii_uppercase());
    if has_underscore && all_caps {
        // 常量：长度门槛把 `TASK_77` 这类标签挡在外面。
        return token.len() >= 8;
    }
    if has_underscore && all_lower {
        return token.len() >= 4;
    }
    camel
}

/// 一个符号在不在源码里（**词边界**匹配：`Run` 不许匹配到 `Runner`）。
fn symbol_exists(token: &str, haystack: &str) -> bool {
    let token = token.trim_end_matches("()");
    let bytes = haystack.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut start = 0;
    while let Some(found) = haystack[start..].find(token) {
        let at = start + found;
        let before_ok = at == 0 || !is_word(bytes[at - 1]);
        let after = at + token.len();
        let after_ok = after >= bytes.len() || !is_word(bytes[after]);
        if before_ok && after_ok {
            return true;
        }
        start = at + 1;
    }
    false
}

/// 一格里所有"代码符号"候选（按 `::` 拆开逐段判）。
fn symbol_candidates(span: &str) -> Vec<String> {
    span.split("::")
        .map(|part| part.trim())
        .filter(|part| is_code_symbol(part))
        .map(|part| part.trim_end_matches("()").to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// 三条判据
// ---------------------------------------------------------------------------

/// 违规 1：`src/` 下有文件没有 §2 的行。
fn uncovered_files(readme: &str, files: &[String]) -> Vec<String> {
    let rows = code_map_rows(readme);
    let mut covered: BTreeSet<String> = BTreeSet::new();
    let mut directories: Vec<String> = Vec::new();
    for (_, row) in &rows {
        let first_cell = row.split('|').nth(1).unwrap_or_default();
        for token in src_tokens(first_cell) {
            let relative = token.trim_start_matches("src/").to_string();
            if relative.ends_with(".rs") {
                covered.insert(relative);
            } else {
                directories.push(relative);
            }
        }
    }
    files
        .iter()
        .filter(|file| {
            !covered.contains(*file)
                && !directories.iter().any(|dir| file.starts_with(dir.as_str()))
        })
        .cloned()
        .collect()
}

/// 违规 2：正文里点名了一条不存在的路径（只查 `.rs` 文件；目录记号不是文件）。
fn missing_paths(readme: &str, agent_root: &Path) -> Vec<(usize, String)> {
    let mut violations = Vec::new();
    for (index, raw) in readme.lines().enumerate() {
        for token in src_tokens(&without_history(raw)) {
            if token.ends_with(".rs") && !agent_root.join(&token).exists() {
                violations.push((index + 1, token));
            }
        }
    }
    violations
}

/// 违规 3：§2 行里点名了一个源码里找不到的符号（`~~…~~` 里的历史提及不算）。
fn unknown_symbols(readme: &str, haystack: &str) -> Vec<(usize, String)> {
    let mut violations = Vec::new();
    for (line, row) in code_map_rows(readme) {
        for span in backticked(&without_history(&row)) {
            for candidate in symbol_candidates(&span) {
                if !symbol_exists(&candidate, haystack) {
                    violations.push((line, candidate));
                }
            }
        }
    }
    violations
}

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

#[test]
fn every_source_file_has_a_row_in_the_code_map() {
    let agent_root = manifest_dir();
    let readme = read_text(&agent_root.join("README.md"));
    let files = source_files(&agent_root.join("src"));

    // ⚠️ 这两个数是**下限**，不是"文件该有多少"——它们的用处只有一个：**证明扫描器真的扫到了
    // 东西**（扫错地方会返回 0，于是下面的 `uncovered` 空得毫无意义）。
    //
    // 因此：**合法地删掉模块时，把它们一起调下来是正常的**（2026-10-06 删 `gate.rs` 之后源码
    // 文件数正好落到 60，就是这一次把它写清楚的来由）。但**不许为了过测试把它们调低**——
    // 它们今天离真实数量很近（源码 60 / 地图 58），红了先问一句"是真少了，还是扫描器坏了"。
    assert!(
        files.len() >= 60,
        "扫到的源码文件太少（{}）——守卫可能读错了地方。\
         ⚠ 若确实是合法删了模块：**同一刀把这个下限一起调下来**，别去动别的断言",
        files.len()
    );
    let rows = code_map_rows(&readme);
    assert!(
        rows.len() >= 50,
        "§2 代码地图只有 {} 行——地图本身可能没被解析到。\
         ⚠ 若确实是合法删了行：**同一刀把这个下限一起调下来**",
        rows.len()
    );

    let uncovered = uncovered_files(&readme, &files);
    assert!(
        uncovered.is_empty(),
        "这些源文件在 README §2 的代码地图里没有行（新增模块要被人看见，删模块要把行删掉）：{uncovered:#?}"
    );
}

#[test]
fn every_src_path_named_in_the_readme_exists() {
    let agent_root = manifest_dir();
    let readme = read_text(&agent_root.join("README.md"));

    let violations = missing_paths(&readme, &agent_root);
    assert!(
        violations.is_empty(),
        "README 点名了不存在的路径（历史提及请写进 `~~…~~`，那是唯一的豁免形状）：{violations:#?}"
    );
}

#[test]
fn every_code_symbol_named_in_the_code_map_exists() {
    let agent_root = manifest_dir();
    let readme = read_text(&agent_root.join("README.md"));
    let haystack = source_haystack(&agent_root);

    // 先自证"真的在扫"：§2 里应当有足够多的符号候选。
    let candidates: usize = code_map_rows(&readme)
        .iter()
        .flat_map(|(_, row)| backticked(row))
        .flat_map(|span| symbol_candidates(&span))
        .count();
    assert!(
        candidates >= 50,
        "§2 里只认出 {candidates} 个符号候选——判据可能写窄了"
    );

    let violations = unknown_symbols(&readme, &haystack);
    assert!(
        violations.is_empty(),
        "§2 点名了源码里找不到的符号（改名之后文档没跟上）：{violations:#?}"
    );
}

/// **正反例自检**：三条判据各自都要能在"真的有违例"时报出来，也要在"没有违例"时安静。
#[test]
fn the_doc_scanners_report_violations_when_they_exist() {
    // 一份**假 README**：一行地图，一个真文件、一个假文件、一个真符号、一个假符号。
    let fake = "\
| 文件 | 当前事实 |
|---|---|
| `src/core.rs` | 中立响应 `ModelOutput` 与 `NoSuchType` |
| `src/gone.rs` | 已经删掉的模块 |

正文里点名 `src/also-gone.rs`，历史提及写在这里：~~`src/old-name.rs`~~。
";
    let agent_root = manifest_dir();
    let files = vec!["core.rs".to_string(), "lib.rs".to_string()];
    let haystack = source_haystack(&agent_root);

    // ① 覆盖：`lib.rs` 没有行、地图里的 `gone.rs` 也不在文件里。
    let uncovered = uncovered_files(fake, &files);
    assert_eq!(uncovered, vec!["lib.rs".to_string()], "{uncovered:#?}");

    // ② 路径：`src/gone.rs`（地图行）与 `src/also-gone.rs`（正文）都要报；`~~` 里那条**不报**。
    let missing = missing_paths(fake, &agent_root);
    let names: Vec<&str> = missing.iter().map(|(_, token)| token.as_str()).collect();
    assert!(names.contains(&"src/gone.rs"), "{missing:#?}");
    assert!(names.contains(&"src/also-gone.rs"), "{missing:#?}");
    assert!(
        !names.contains(&"src/old-name.rs"),
        "`~~…~~` 里的历史提及不该报：{missing:#?}"
    );

    // ③ 符号：真符号（`ModelOutput`）安静，假符号（`NoSuchType`）报出来。
    assert!(
        !symbol_exists("NoSuchType", &haystack),
        "夹具名不该出现在判据面里（本文件自己被排除在 haystack 之外）"
    );
    let symbols = unknown_symbols(fake, &haystack);
    let symbol_names: Vec<&str> = symbols.iter().map(|(_, token)| token.as_str()).collect();
    assert_eq!(symbol_names, vec!["NoSuchType"], "{symbols:#?}");

    // ④ 判据本身的正反例：领域词与标签**不是**符号候选。
    for not_a_symbol in ["Run", "Turn", "TASK_77", "read-only", "src/core.rs", "a=b"] {
        assert!(
            symbol_candidates(not_a_symbol).is_empty(),
            "{not_a_symbol} 不该被当成代码符号"
        );
    }
    for a_symbol in ["ModelOutput", "is_open", "MAX_ID_BYTES", "MinimalOs::open"] {
        assert!(
            !symbol_candidates(a_symbol).is_empty(),
            "{a_symbol} 应当被当成代码符号"
        );
    }

    // ⑤ 判据 2 的边界（2026-10-18 修的那处假阳性）：**只有起始一个词的 `src/` 才算**。
    // 正例：更长路径里那一段不收；反例：真的写了 `src/<不存在>.rs` 照旧要报。
    let nested = "\
正文里点名 `app/src-tauri/src/home_doors.rs`（**更长路径的一段**，不是「以 `src/` 出现」），
以及 `src/really-gone.rs`（真的不存在）。
";
    let nested_missing = missing_paths(nested, &agent_root);
    let nested_names: Vec<&str> = nested_missing
        .iter()
        .map(|(_, token)| token.as_str())
        .collect();
    assert!(
        !nested_names.contains(&"src/home_doors.rs"),
        "更长路径里那一段 `src/` 不该被当成 agent 相对路径：{nested_missing:#?}"
    );
    assert!(
        nested_names.contains(&"src/really-gone.rs"),
        "真的不存在的 `src/<名字>.rs` 仍然要报（反例）：{nested_missing:#?}"
    );
}
