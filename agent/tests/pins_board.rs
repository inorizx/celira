//! TASK_33 验收：**组件区**（主体策展、系统渲染）——渲染有界、缺省不等于零、永不代主体策展。
//!
//! 本文件只判**这一件的读数形状与边界**（`reading` / `line`）。
//! 时序（开场一次、之后不摆）与生产装配路径的判据在 `tests/run_session.rs`
//! （第一封请求体里就有这一块、第二轮没有）；动作侧的判据在 `tests/notes_app.rs`。

use celira_agent::activity_origin::ActivityOrigin;
use celira_agent::pins_board::{
    MAX_RENDERED, PINS_BOARD_LABEL, PREVIEW_BYTES, PinnedRow, PinsBoard,
};

/// 装配期读回来的行（身份 + 两个年龄 + 标题 + 正文原样）。
///
/// ⚠ 两格年龄答**两个问题**，因此都在：
/// **创建于**（权威在经历账本，由便签 App 折出来）与**最后动过**（盘上只有 mtime）。
/// 这里默认都给 `None`（读不出来 / 折不到）⇒ 两格都如实写 `age=unknown`，**不编一个数**。
fn row(id: &str, title: &str, text: &str) -> PinnedRow {
    row_at(id, None, title, text)
}

fn row_at(id: &str, touched_millis: Option<u64>, title: &str, text: &str) -> PinnedRow {
    PinnedRow {
        id: id.to_string(),
        created_millis: None,
        touched_millis,
        title: title.to_string(),
        text: text.to_string(),
    }
}

/// 一行两格年龄都给全（创建 ＋ 最后动过）。
fn row_both(
    id: &str,
    created: Option<u64>,
    touched: Option<u64>,
    title: &str,
    text: &str,
) -> PinnedRow {
    PinnedRow {
        created_millis: created,
        ..row_at(id, touched, title, text)
    }
}

/// 装配那一轮的钟**固定**——年龄读数用它（不另取系统钟），因此同一份素材逐字节相同。
const NOW: Option<u64> = Some(1_000_000);

fn board(rows: Option<&[PinnedRow]>) -> PinsBoard<ActivityOrigin> {
    PinsBoard::new(rows, NOW, ActivityOrigin::new())
}

#[test]
fn the_block_carries_its_own_label_and_the_identities() {
    let rows = vec![row("note-1", "词：青苔", "青苔——主人要求记住的一个词。")];
    let line = board(Some(&rows)).line();
    assert_eq!(line.label(), PINS_BOARD_LABEL);
    assert!(
        line.text().starts_with("pinned = 1, ~"),
        "条数后面跟着这一块的字节账单: {}",
        line.text()
    );
    assert!(line.text().contains("note-1"), "{}", line.text());
}

/// 账单是**主体的内容**的量：同样的纸条，正文写短一点，下一轮这个数就小一截。
#[test]
fn the_bill_follows_her_own_content() {
    fn bill(text: &str) -> usize {
        let rows = vec![row("note-1", "标题", text)];
        let reading = board(Some(&rows)).reading();
        let header = reading.lines().next().expect("有计数行");
        let (_, bytes) = header.rsplit_once('~').expect("计数行带账单");
        bytes
            .trim_end_matches('B')
            .parse()
            .unwrap_or_else(|_| panic!("账单是数字: {header}"))
    }
    assert!(
        bill("短") < bill(&"长".repeat(200)),
        "账单跟着主体写的内容走"
    );
}

#[test]
fn an_empty_wall_is_stated_as_zero_not_left_unsaid() {
    // 读成了、一条也没标 ⇒ 把零说出口（主体才知道是「空」，不是「没读到」）。
    assert_eq!(
        board(Some(&[])).reading(),
        "pinned = 0\n（一条也没标上）".to_string()
    );
}

#[test]
fn a_failed_read_is_unknown_and_never_pretends_the_wall_is_empty() {
    // 读失败（装配期 `pinned_material` 报错 ⇒ None）：如实说没读到，绝不冒充「墙上没有」。
    assert_eq!(board(None).reading(), "pinned = unknown".to_string());
}

fn parse_line(line: &str) -> (String, String, String, String, String) {
    let mut parts = line.split(" | ");
    (
        parts.next().unwrap_or_default().to_string(),
        parts.next().unwrap_or_default().to_string(),
        parts.next().unwrap_or_default().to_string(),
        parts.next().unwrap_or_default().to_string(),
        parts.next().unwrap_or_default().to_string(),
    )
}

#[test]
fn previews_are_bounded_and_never_split_a_character() {
    let even = "青".repeat(100); // 每字 3 字节 ⇒ 60 字节正好落在字符边界
    let odd = "苔".repeat(21); // 63 字节 ⇒ 截断必须回退到 60 之前的安全边界
    let rows = vec![row("note-1", &even, &odd)];
    let reading = board(Some(&rows)).reading();
    let (id, created, touched, title, text) =
        parse_line(reading.lines().nth(1).expect("第二行是第一张纸条"));

    assert_eq!(id, "note-1");
    assert_eq!(created, "age=unknown", "折不到就如实说读不出来");
    assert_eq!(touched, "age=unknown", "没给时间就如实说读不出来");
    assert_eq!(title, format!("{}…", "青".repeat(20))); // 60 字节 = 20 个汉字
    assert_eq!(text, format!("{}…", "苔".repeat(20)));
    assert!(title.len() <= PREVIEW_BYTES + '…'.len_utf8());
    assert!(text.len() <= PREVIEW_BYTES + '…'.len_utf8());
}

#[test]
fn control_characters_do_not_forge_another_note() {
    // 标题里塞换行会伪造出「另一张纸条」：预览必须把它压成空格。
    let rows = vec![row("note-1", "第一行\n第二行", "正文")];
    let reading = board(Some(&rows)).reading();
    assert_eq!(reading.lines().count(), 2, "{reading}");
    assert!(reading.contains("第一行 第二行"), "{reading}");
}

#[test]
fn overflow_is_stated_and_not_silently_cut() {
    let rows: Vec<PinnedRow> = (1..=MAX_RENDERED + 3)
        .map(|n| row(&format!("note-{n}"), "标题", "正文"))
        .collect();
    let reading = board(Some(&rows)).reading();
    assert!(
        reading.starts_with(&format!("pinned = {}, ~", MAX_RENDERED + 3)),
        "{reading}"
    );
    assert!(reading.contains("（还有 3 张未列出）"), "{reading}");
    // 计数行 + 上限张数 + 溢出那一行。
    assert_eq!(reading.lines().count(), 1 + MAX_RENDERED + 1, "{reading}");
}

#[test]
fn the_same_wall_renders_the_same_bytes() {
    let rows = vec![
        row("note-1", "词：青苔", "青苔——主人要求记住的一个词。"),
        row("note-2", "待办", "把 host 的 e2e 脚本改成只验 im + host。"),
    ];
    let first = board(Some(&rows)).reading();
    let second = board(Some(&rows)).reading();
    assert_eq!(first, second);
    assert!(first.starts_with("pinned = 2, ~"), "{first}");
    assert!(
        first.contains("note-1 | age=unknown | age=unknown | 词：青苔 | 青苔"),
        "{first}"
    );
}

/// **两格年龄答两个问题，因此都在**（判据 19 的"创建于" ＋ 盘上的"最后动过"）。
///
/// ⚠ 词**不共用**，而且各自对着各自的来源：
/// - `created-ago` ＝ 什么时候写下的（权威在**经历账本**，由便签 App 折出来）；
/// - `touched-ago` ＝ 最后动过（盘上只有 mtime，而 `notes_edit` / `notes_append` 会改写它）。
///
/// 这一条钉的是**措辞与来源的对应**：把 `created-ago` 接到 mtime 上、或把它写成
/// `touched-ago`，都是**编事实**，必须红。
#[test]
fn the_two_ages_answer_two_questions_and_never_share_a_word() {
    // 创建于 940_000，最后动过 970_000，钟在 1_000_000 ⇒ 60s / 30s。
    let rows = vec![row_both(
        "note-1",
        Some(940_000),
        Some(970_000),
        "标题",
        "正文",
    )];
    let reading = board(Some(&rows)).reading();
    assert!(
        reading.contains("note-1 | created-ago=60s | touched-ago=30s | 标题 | 正文"),
        "{reading}"
    );
    // 两格各自独立地"不编一个数"。
    let half = vec![row_both("note-1", None, Some(970_000), "标题", "正文")];
    let reading = board(Some(&half)).reading();
    assert!(
        reading.contains("age=unknown | touched-ago=30s"),
        "{reading}"
    );
    // 与持有物那一支的词**不共用**（那是两个件各自的词）。
    assert!(
        !reading.contains("due-in") && !reading.contains("fired-ago"),
        "{reading}"
    );
}
