//! 行动面失败词表的机械判据：**编解码对称、封闭、形状稳定**。
//!
//! 为什么单独立文件：这一格落在盘上的**行动结果**里（`Observed.failure`）。它曾经
//! **只有编码端**——`as_str()` 是权威、却**没有 `from_str`**，于是"写得进去、读不出来"
//! （`core.rs` 那两个共用类别的说明里自己写着"解码端还没有收口"）。模型那一格早有
//! 自己的判据（`tests/model_failure_wordlist.rs`），这里管**行动面**。
//!
//! **标记表不在这里手抄**：它从**源码里那份 `as_str`** 解析出来，再拿它去喂 `from_str`。
//! 手抄一张表就会多一份真相；解析出来的表与枚举**不可能漂移**。
//!
//! 四条判据：
//! 1. **覆盖**：凡在 `ActionOutcome::failure*` / `ActionAdmission::refused*` 调用点上出现的
//!    词表，要么在 [`WORDLISTS`] 里，要么在 [`EXCUSED`] 里写明为什么（不许漏）；
//! 2. **轮转**：`as_str` 能拼出的每一个标记，`from_str` 都必须认回同一格（规范形态就是它自己）；
//! 3. **对称**：`from_str` 认的标记集合与 `as_str` 能拼的集合**逐条相等**（多一个少一个都红）；
//! 4. **封闭**：认不出的取值明确拒绝（`None`），不猜、不就近映射。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use celira_agent::file_app::FailureClass;
use celira_agent::im_channel::ImFailure;
use celira_agent::mcp::McpFailure;
use celira_agent::notes_app::NotesFailure;
use celira_agent::os::ExpansionFailure;
use celira_agent::pinned_actions::PinFailure;
use celira_agent::terminal_app::TerminalFailure;
use celira_agent::timer_app::TimerFailure;
use celira_agent::web_app::WebFailure;

/// 一份词表：枚举名 · 它所在的源文件 · 「认回原格并给出规范形态」的那个函数。
///
/// `decode` 返回的是**规范形态**（`from_str(…)` 之后再 `as_str()`），因此第 2 条判据
/// 同时量了"认得出"与"认回的是同一格"。
struct Wordlist {
    name: &'static str,
    file: &'static str,
    decode: fn(&str) -> Option<&'static str>,
}

/// **行动面上每一份能单值解码的词表**（九份；2026-10-13 补上 `TimerFailure`）。
///
/// ⚠ 它是**封闭名单**：第 1 条判据会把"调用点上出现、却不在这张表里"的名字揪出来。
const WORDLISTS: [Wordlist; 9] = [
    Wordlist {
        name: "ImFailure",
        file: "im_channel.rs",
        decode: |text| ImFailure::from_str(text).map(|value| value.as_str()),
    },
    Wordlist {
        name: "NotesFailure",
        file: "notes_app.rs",
        decode: |text| NotesFailure::from_str(text).map(NotesFailure::as_str),
    },
    Wordlist {
        name: "FailureClass",
        file: "file_app.rs",
        decode: |text| FailureClass::from_str(text).map(FailureClass::as_str),
    },
    Wordlist {
        name: "WebFailure",
        file: "web_app.rs",
        decode: |text| WebFailure::from_str(text).map(WebFailure::as_str),
    },
    Wordlist {
        name: "TerminalFailure",
        file: "terminal_app.rs",
        decode: |text| TerminalFailure::from_str(text).map(TerminalFailure::as_str),
    },
    Wordlist {
        name: "McpFailure",
        file: "mcp/mod.rs",
        decode: |text| McpFailure::from_str(text).map(McpFailure::as_str),
    },
    Wordlist {
        name: "ExpansionFailure",
        file: "os.rs",
        decode: |text| ExpansionFailure::from_str(text).map(ExpansionFailure::as_str),
    },
    Wordlist {
        name: "PinFailure",
        file: "pinned_actions.rs",
        decode: |text| PinFailure::from_str(text).map(PinFailure::as_str),
    },
    // ⚠ **2026-10-13 补的第九份**：timer 这一族从前**只有裸常量、没有解码端**——而扫描器是按
    // 调用形状抓的（`ActionOutcome::failure(CLASS_…)` 传的是**常量**，产不出名字），所以它一直
    // 在名单外。本刀给 timer 加两个复核用的失败取值，按纪律「新增取值必须同一刀补齐解码端校验」
    // 先把这张表补上：`TimerFailure` 就是那一份词表的唯一权威（`as_str` 指向那几个常量）。
    Wordlist {
        name: "TimerFailure",
        file: "timer_app.rs",
        decode: |text| TimerFailure::from_str(text).map(TimerFailure::as_str),
    },
];

/// **三条豁免**，每条都写明理由。两类：
///
/// **甲、单值解码不成立**（标记装不下那一格的全部）：
/// - `FileViewFailure` 是**信封**：它有两个变体把内层词表包在里面（`Materials(FailureClass)`、
///   `CreationTargetUnusable(FailureClass)`），单值解码重建不出内层那一个；
/// - `SoulWriteFailure` 是**带载荷的分类**：`soul-too-large` 那一格的 `wanted` / `current`
///   两个字节数在标记里**没有落脚处**，硬解码就会**编造两个 0**。
///
/// **乙、根本不属于行动面**：
/// - `MachineFailure` 是**装配/路由**那一层的失败（`composition::machine_refused` ⇒
///   `CompositionError`），它不进 `Observed.failure` 那一格；扫描器是按调用形状抓的，
///   抓到了它，这里如实说明它不是这一族。
///
/// ⇒ 甲类的解码端是"**读到哪一类**（标记）+ **那一类的读数另取**"，不是 `Option<Self>`。
const EXCUSED: [(&str, &str); 3] = [
    (
        "FileViewFailure",
        "信封：两个变体的标记来自内层词表，单值解码重建不出内层那一格",
    ),
    (
        "SoulWriteFailure",
        "带载荷：`soul-too-large` 的字节数不在标记里，硬解码会编造两个 0",
    ),
    (
        "MachineFailure",
        "不属于行动面：它是装配/路由那一层的失败（进 `CompositionError`），不落行动结果那一格",
    ),
];

/// 源码里那三个**共用常量**的写法 → 它的值（权威在各自那一处，这里只解析名字）。
const SHARED: [(&str, &str); 2] = [
    ("crate::core::INVALID_ARGUMENT", "invalid-argument"),
    ("crate::core::UNKNOWN_CAPABILITY", "unknown-capability"),
];

#[test]
fn every_action_failure_vocabulary_is_decodable_or_excused() {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (name, text) in sources() {
        for prefix in [
            "ActionOutcome::failure(",
            "ActionOutcome::failure_with_content(",
            "ActionAdmission::refused(",
            "ActionAdmission::refused_with(",
            // 两个**转发口**：有的 App 把"这一类怎么落成结果"收在本地小函数里
            // （`fn refused(failure: WebFailure) -> ActionOutcome`），于是词表只出现在它的
            // 调用点上。⚠ 扫描面就到这里为止：别的转发形状它看不见——那一条靠人登记。
            "refused(",
            "refused_with(",
        ] {
            for hit in call_argument_names(&text, prefix) {
                seen.insert(hit);
            }
        }
        // 让编译器盯着 `name`（扫描器自检用得上它）。
        assert!(!name.is_empty() || !text.is_empty());
    }
    // ⚠ **这条下界只是"判据没有空跑"**，不是覆盖率：扫描面能把这几处认出来就够了，
    // 真正的牙齿是下面那条"认出名字必须在名单里"。
    assert!(
        seen.len() >= 5,
        "调用点上至少该认出五份词表，只认出 {} 份——扫描器退化了",
        seen.len()
    );
    for name in &seen {
        let known = WORDLISTS.iter().any(|each| each.name == name)
            || EXCUSED.iter().any(|(excused, _)| excused == name);
        assert!(
            known,
            "`{name}` 出现在行动面的调用点上，却不在 WORDLISTS 里、也不在 EXCUSED 里：\n\
             它的标记会落进盘上的行动结果那一格——要么给它补 `from_str`，要么写明为什么不能。"
        );
    }
    // 反例自检：被豁免的那两份**确实**没有解码端（否则豁免就是摆设）。
    for (excused, _) in EXCUSED {
        assert!(
            !WORDLISTS.iter().any(|each| each.name == excused),
            "`{excused}` 既在豁免名单里又在词表里——两份名单不许重叠"
        );
    }
}

#[test]
fn wordlists_round_trip_every_mark() {
    let mut total = 0usize;
    for list in &WORDLISTS {
        let text = read(list.file);
        let encoder = marks_in(&as_str_body(&text, list.name));
        assert!(
            !encoder.is_empty(),
            "{} 的 `as_str` 里一个标记都没解析出来——判据在空跑",
            list.name
        );
        for mark in &encoder {
            assert_eq!(
                (list.decode)(mark),
                Some(mark.as_str()),
                "{}：`from_str` 认不出自己 `as_str` 拼出来的标记「{mark}」",
                list.name
            );
        }
        total += encoder.len();
    }
    assert_eq!(
        total, 88,
        "九份词表的标记总数变了（{total}）——加取值就要改这一行"
    );
}

#[test]
fn decoders_cover_exactly_the_encoder_tokens() {
    for list in &WORDLISTS {
        let text = read(list.file);
        let encoder: BTreeSet<String> = marks_in(&as_str_body(&text, list.name))
            .into_iter()
            .collect();
        let decoder: BTreeSet<String> = marks_in(&from_str_body(&text, list.name))
            .into_iter()
            .collect();
        assert_eq!(
            decoder, encoder,
            "{}：`from_str` 认的标记与 `as_str` 能拼的**不逐条对称**——\
             只有编码端或只有解码端都算缺一半（左边=解码端，右边=编码端）",
            list.name
        );
    }
}

#[test]
fn marks_are_unique_and_shaped_stably() {
    for list in &WORDLISTS {
        let text = read(list.file);
        let encoder = marks_in(&as_str_body(&text, list.name));
        let unique: BTreeSet<&String> = encoder.iter().collect();
        assert_eq!(
            unique.len(),
            encoder.len(),
            "{}：有两个取值共用同一个标记——那会让历史里出现两种事实",
            list.name
        );
        for mark in &encoder {
            assert!(
                mark.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{}：标记里有不该出现的字符「{mark}」（只许小写字母、数字与连字符）",
                list.name
            );
        }
    }
}

#[test]
fn unknown_marks_are_rejected_fail_closed() {
    for list in &WORDLISTS {
        for bad in [
            "",
            "nope",
            "NOT-A-CLASS",
            "invalid-argument-extra",
            "not found",
        ] {
            assert_eq!(
                (list.decode)(bad),
                None,
                "{}：认不出的取值必须明确拒绝（不许就近映射）：「{bad}」",
                list.name
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 源码扫描（判据的取证部分）
// ---------------------------------------------------------------------------

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn read(name: &str) -> String {
    fs::read_to_string(root().join(name)).unwrap_or_else(|error| panic!("读不到 {name}: {error}"))
}

/// `src/` 下全部 `.rs`（含子目录；`bin/` 里的入口夹具也扫——它们也构造分类）。
fn sources() -> Vec<(String, String)> {
    let mut paths = Vec::new();
    collect(&root(), &mut paths);
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let name = path
                .strip_prefix(root())
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let text = fs::read_to_string(&path).unwrap_or_default();
            (name, text)
        })
        .collect()
}

fn collect(directory: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, into);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            into.push(path);
        }
    }
}

/// 一个调用点实参里的**类型名**：`ActionOutcome::failure(NotesFailure::NotFound…)` ⇒ `NotesFailure`。
///
/// 传变量或 `.as_str()` 的调用点不产出名字（那不是"某一份词表"的证据）。
fn call_argument_names(text: &str, prefix: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(prefix) {
        let after = &rest[at + prefix.len()..];
        if let Some(colon) = after.find("::") {
            let candidate = &after[..colon];
            let shaped = !candidate.is_empty()
                && candidate
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_uppercase())
                && candidate.chars().all(|c| c.is_alphanumeric() || c == '_');
            if shaped {
                names.push(candidate.to_string());
            }
        }
        rest = &rest[at + prefix.len()..];
    }
    names
}

/// `impl <Name>` 里某个函数的**函数体**（去掉注释行）。
fn method_body(text: &str, name: &str, signature: &str) -> String {
    let marker = format!("impl {name}");
    let at = text
        .find(&marker)
        .unwrap_or_else(|| panic!("{name} 的 impl 不见了"));
    let rest = &text[at..];
    let fn_at = rest
        .find(signature)
        .unwrap_or_else(|| panic!("{name} 里找不到 `{signature}`"));
    let rest = &rest[fn_at..];
    let open = rest.find('{').expect("函数体");
    let rest = &rest[open + 1..];
    let close = rest
        .find("\n    }")
        .unwrap_or_else(|| panic!("{name} 的函数体没有收尾"));
    rest[..close]
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn as_str_body(text: &str, name: &str) -> String {
    method_body(text, name, "pub fn as_str(")
}

fn from_str_body(text: &str, name: &str) -> String {
    method_body(text, name, "pub fn from_str(")
}

/// 一段 match 体里的**全部标记**：引号里的字面量 ＋ 那两个共用常量（按名字解析成值）。
fn marks_in(body: &str) -> Vec<String> {
    let mut marks = Vec::new();
    let mut quotes = body.match_indices('"');
    while let Some((open, _)) = quotes.next() {
        if let Some((close, _)) = quotes.next() {
            marks.push(body[open + 1..close].to_string());
        }
    }
    for (written, value) in SHARED {
        if body.contains(written) {
            marks.push(value.to_string());
        }
    }
    marks
}

/// 扫描器自检：解析出来的标记必须是**真的标记**，而且共用常量认得出。
#[test]
fn the_wordlist_scanner_reports_when_it_exists() {
    let sample = r#"
            ImFailure::Unreachable => "unreachable",
            ImFailure::InvalidArgument => crate::core::INVALID_ARGUMENT,
    "#;
    let marks = marks_in(sample);
    assert_eq!(marks, vec!["unreachable", "invalid-argument"]);

    // 注释行里的引号不算（否则一句"它回 `not-found`"就能伪造一个标记）。
    let commented = "\n            // 这里会回 \"not-found\"\n            X::Y => \"real\",\n";
    let stripped: String = commented
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(marks_in(&stripped), vec!["real"]);

    // 调用点解析：类型名认得出，变量与 `.as_str()` 不产出名字。
    assert_eq!(
        call_argument_names(
            "ActionOutcome::failure(NotesFailure::NotFound)",
            "ActionOutcome::failure("
        ),
        vec!["NotesFailure"]
    );
    assert!(
        call_argument_names(
            "ActionOutcome::failure(class.as_str())",
            "ActionOutcome::failure("
        )
        .is_empty()
    );
}
