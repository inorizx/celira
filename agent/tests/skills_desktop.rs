//! 验收：技能桌面（材料区方法索引）——**有界名字索引**的形状、frontmatter 取舍、有界与截断、
//! 缺省语义、零正文。
//!
//! 2026-10-20 裁决：整块从"一行计数"升成"名字 ＋ 一句触发"。理由是 `probe-recall` 只量过
//! "任务点名了那份材料"的情形；任务没点名时，计数那一行支撑不了"要不要翻它"这个判断。
//! 判据把两件事拆开：**名字＋触发 = 索引（进场）**，**正文 = 方法（不进场）**。
//! 因此本文件里 2026-09-22 那批"名字不许进场"的断言随行为一起改写——这是行为变更刀。

use std::fs;
use std::path::{Path, PathBuf};

use celira_agent::core::{ContentBatch, ContextSource, InjectionContext};
use celira_agent::skills_desktop::{
    MAX_HINT_BYTES, MAX_LISTED, MAX_NAME_BYTES, SKILLS_DESKTOP_LABEL, SkillRef, SkillsDesktop,
    read_skills, read_skills_for,
};

fn sandbox(name: &str) -> PathBuf {
    let unique = format!(
        "celira-skills-{}-{}-{}",
        std::process::id(),
        name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("时钟可用")
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    fs::create_dir_all(&root).expect("可以建临时材料根");
    root
}

fn write_skill(root: &Path, relative: &str, body: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("有父目录")).expect("可以建目录");
    fs::write(path, body).expect("可以写清单");
}

/// 一份市面形状的清单（YAML frontmatter + 正文）。
fn skill_md(name: &str, description: &str, extra: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n\n# 正文\n{extra}\n")
}

/// 最小假内层：只为把注入链串起来。
struct Nothing;

impl ContextSource for Nothing {
    fn on_run_start(&mut self, _context: &InjectionContext) -> ContentBatch {
        ContentBatch::default()
    }

    fn before_turn(&mut self, _context: &InjectionContext) -> ContentBatch {
        ContentBatch::default()
    }
}

/// **正例**：两个一级条目 ⇒ 计数 ＋ 名字 ＋ 一句触发；正文一个字节都不进场。
#[test]
fn a_non_empty_skills_area_lists_names_and_triggers() {
    let root = sandbox("index");
    write_skill(
        &root,
        "skills/web-search/SKILL.md",
        &skill_md("web-search", "要上网查一件事时用", "# 步骤\n秘密正文"),
    );
    // 包：一级条目自己没有清单 ⇒ 名字回退目录名、没有触发那一句。
    write_skill(
        &root,
        "skills/notes-pack/notes-tidy/SKILL.md",
        &skill_md("notes-tidy", "把笔记整理成条目", ""),
    );

    let entries = read_skills(&root).expect("有技能区");
    assert_eq!(entries.len(), 2, "一级条目 = 直接子目录");

    let reading = SkillsDesktop::new(read_skills(&root), Nothing)
        .reading()
        .expect("有技能区就有读数");
    assert_eq!(
        reading,
        "skills: 2 项（要翻看用 files_list，路径 skills/）\n  notes-pack\n  web-search — 要上网查一件事时用",
        "头一行计数；然后按目录名排序，一条一行"
    );
    assert_eq!(reading.lines().count(), 3, "两行条目 ＋ 一行头: {reading}");
    assert_eq!(
        SkillsDesktop::new(read_skills(&root), Nothing)
            .line()
            .expect("有块")
            .label(),
        SKILLS_DESKTOP_LABEL
    );
    // 零正文：正文只能 files_read 读到。
    assert!(
        !reading.contains("# 步骤") && !reading.contains("秘密正文"),
        "正文不许进场: {reading}"
    );
    // 包里的子技能**不另列**（一级条目才是一行）。
    assert!(!reading.contains("notes-tidy"), "包只列包名: {reading}");
}

/// 计数的口径要**机械**：数的是目录，不是"能解析出的技能"。因此
/// ① 没有 `SKILL.md` 的一级目录照样算一项（名字回退目录名）；
/// ② `skills/` 下的**散文件**不是技能，不计入、也不列。
#[test]
fn the_count_is_the_number_of_top_level_directories() {
    let root = sandbox("counting");
    write_skill(&root, "skills/flat-skill/SKILL.md", "# 只有正文\n");
    write_skill(
        &root,
        "skills/a-pack/inner/SKILL.md",
        &skill_md("inner", "包里的一项", ""),
    );
    fs::create_dir_all(root.join("skills/empty-entry")).expect("建空的一级目录");
    fs::write(root.join("skills/README.md"), "散落在技能区的文件\n").expect("写散文件");

    let entries = read_skills(&root).expect("有技能区");
    assert_eq!(entries.len(), 3, "三个一级目录；散文件不计入");
    let names: Vec<&str> = entries.iter().map(|entry| entry.display.as_str()).collect();
    assert_eq!(
        names,
        ["a-pack", "empty-entry", "flat-skill"],
        "按目录名排序"
    );

    let reading = SkillsDesktop::new(read_skills(&root), Nothing)
        .reading()
        .expect("有读数");
    assert!(reading.starts_with("skills: 3 项"), "{reading}");
    assert!(!reading.contains("README"), "散文件不进索引: {reading}");
}

/// **frontmatter 的取舍**（本件唯一的"读内容"）：`name` 覆盖目录名、`description` 是第一行；
/// 坏清单 / 没有清单 ⇒ 回退目录名、没有触发（**不编、不拿正文凑**）。
#[test]
fn frontmatter_supplies_name_and_trigger_and_falls_back_to_the_directory() {
    let root = sandbox("frontmatter");
    // ① 没有闭合的 `---` ⇒ 解析不出来，回退目录名。
    write_skill(
        &root,
        "skills/broken/SKILL.md",
        "---\nname: 坏清单\ndescription: 第一行\n第二行\t带控制字符\n",
    );
    // ② `name` 覆盖目录名（引号去掉）；没有 description。
    write_skill(
        &root,
        "skills/named/SKILL.md",
        "---\nname: \"pretty-kit\"\n---\n\n# 正文\n",
    );
    // ③ 两个都有，引号去掉。
    write_skill(
        &root,
        "skills/quoted/SKILL.md",
        "---\nname: 'quoted-kit'\ndescription: \"有人要你核对回执时用\"\n---\n",
    );
    // ④ 一级目录里压根没有清单。
    fs::create_dir_all(root.join("skills/silent")).expect("建没有清单的一级目录");

    let entries = read_skills(&root).expect("有技能区");
    let by_name: Vec<(&str, Option<&str>)> = entries
        .iter()
        .map(|entry| (entry.display.as_str(), entry.hint.as_deref()))
        .collect();
    assert_eq!(
        by_name,
        [
            ("broken", None),
            ("pretty-kit", None),
            ("quoted-kit", Some("有人要你核对回执时用")),
            ("silent", None),
        ],
        "坏清单回退目录名；name 覆盖目录名；没有 description 就不给触发"
    );
}

/// **有界**：三百个一级条目也只列 [`MAX_LISTED`] 个，其余如实说"另 N 项"；整块有字节上限。
#[test]
fn the_listing_is_bounded_and_counts_what_it_did_not_show() {
    let root = sandbox("many");
    for index in 0..300 {
        fs::create_dir_all(root.join(format!("skills/pack-{index:03}"))).expect("建一级条目目录");
    }
    let reading = SkillsDesktop::new(read_skills(&root), Nothing)
        .reading()
        .expect("有读数");
    assert!(reading.contains("skills: 300 项"), "{reading}");
    assert_eq!(
        reading.lines().count(),
        1 + MAX_LISTED + 1,
        "头 ＋ 12 条 ＋ 一行'另 N 项'"
    );
    assert!(
        reading.contains("pack-000") && reading.contains("pack-011"),
        "{reading}"
    );
    assert!(!reading.contains("pack-012"), "第 13 条起不再列: {reading}");
    assert!(
        reading.contains("…另 288 项"),
        "没列出来的要如实报数: {reading}"
    );
    assert!(
        reading.len() < 8 * 1024,
        "整块要远小于批次上限（实测 {} 字节）",
        reading.len()
    );
}

/// **截断**：名字与触发各按自己的字节上限截断，切在字符边界上并加 `…`。
#[test]
fn long_names_and_triggers_are_cut_on_a_character_boundary_and_marked() {
    let root = sandbox("long");
    // 80 个汉字 = 240 字节：**超过 `MAX_NAME_BYTES`（64）以触发截断，又不越过
    // 文件系统的 `NAME_MAX`（255 字节）**——100 个汉字是 300 字节，目录根本建不出来，
    // 这条用例会在 `create_dir_all` 上先失败（2026-10-23 真机踩到）。
    let long_dir = "名".repeat(80);
    fs::create_dir_all(root.join("skills").join(&long_dir)).expect("建长名字目录");
    write_skill(
        &root,
        "skills/wordy/SKILL.md",
        &skill_md("wordy", &"描".repeat(200), ""),
    );

    let entries = read_skills(&root).expect("有技能区");
    let wordy = entries
        .iter()
        .find(|entry| entry.display == "wordy")
        .expect("wordy 在场");
    let hint = wordy.hint.as_deref().expect("有触发");
    assert!(hint.ends_with('…'), "{hint}");
    assert!(
        hint.len() <= MAX_HINT_BYTES + '…'.len_utf8(),
        "触发上限（实测 {} 字节）",
        hint.len()
    );
    assert!(hint.trim_end_matches('…').chars().all(|ch| ch == '描'));

    let long = entries
        .iter()
        .find(|entry| entry.display.starts_with('名'))
        .expect("长名字那一项在场");
    assert!(long.display.ends_with('…'), "{}", long.display);
    assert!(
        long.display.len() <= MAX_NAME_BYTES + '…'.len_utf8(),
        "名字上限（实测 {} 字节）",
        long.display.len()
    );
    assert!(
        long.display
            .trim_end_matches('…')
            .chars()
            .all(|ch| ch == '名')
    );
}

/// **顺序是身份序，不是推荐序**：按目录名排，与创建顺序无关。
#[test]
fn the_listing_order_is_the_directory_name_order() {
    let root = sandbox("order");
    for name in ["zeta", "alpha", "mike"] {
        fs::create_dir_all(root.join("skills").join(name)).expect("建目录");
    }
    let entries = read_skills(&root).expect("有技能区");
    let names: Vec<&str> = entries.iter().map(|entry| entry.display.as_str()).collect();
    assert_eq!(names, ["alpha", "mike", "zeta"]);
}

/// **缺省不等于零**（两个负对照）：
/// ① 材料区**没有** `skills/` ⇒ 整块不出场（不是空标题、也不是 `0 项`）；
/// ② 技能区在、但一个一级条目都没有 ⇒ 如实说"为空"。
#[test]
fn a_missing_skills_directory_has_no_block_and_an_empty_one_says_it_is_empty() {
    let absent = sandbox("absent");
    assert_eq!(read_skills(&absent), None, "没有 skills/ ⇒ 没有技能区");
    assert!(
        SkillsDesktop::new(None, Nothing).line().is_none(),
        "技能区不存在时**整块不出场**"
    );
    assert!(
        SkillsDesktop::new(None, Nothing).reading().is_none(),
        "连读数也没有（`None` 不是 `Some(0)`：缺省不等于零）"
    );

    let file = sandbox("not-a-dir");
    fs::write(file.join("skills"), "这不是目录\n").expect("写同名文件");
    assert_eq!(read_skills(&file), None, "同名的文件不是技能区");

    let empty = sandbox("empty");
    fs::create_dir_all(empty.join("skills")).expect("建技能区");
    assert_eq!(read_skills(&empty), Some(Vec::new()));
    let reading = SkillsDesktop::new(read_skills(&empty), Nothing)
        .reading()
        .expect("有块（存在但空）");
    assert_eq!(reading, "skills: 0 项（技能区为空）", "空就如实说空");
}

/// **引用按一级目录名过滤**（不是 frontmatter 的显示名）；过滤后仍报技能区总数；
/// 正文不会被引用挡住——软引用的边界要如实钉住。
#[test]
fn a_reference_filters_by_directory_name_and_reports_the_catalog_total() {
    let root = sandbox("reference");
    write_skill(
        &root,
        "skills/alpha/SKILL.md",
        &skill_md("pretty-alpha", "处理甲事时用", "# 正文\n甲的步骤"),
    );
    write_skill(
        &root,
        "skills/beta/SKILL.md",
        &skill_md("pretty-beta", "处理乙事时用", "# 正文\n乙的步骤"),
    );

    let only =
        read_skills_for(&root, &SkillRef::Only(vec!["alpha".to_string()])).expect("有技能区");
    assert_eq!(only.total, 2, "过滤前的总数");
    assert_eq!(only.entries.len(), 1, "只启用 alpha");
    assert_eq!(only.entries[0].id, "alpha");
    let reading = SkillsDesktop::with_index(Some(only), Nothing)
        .reading()
        .expect("有读数");
    assert!(
        reading.starts_with("skills: 1 项（已启用；技能区共 2 项；"),
        "{reading}"
    );
    assert!(reading.contains("pretty-alpha"), "{reading}");
    assert!(!reading.contains("pretty-beta"), "没启用的不列: {reading}");

    // 引用写的是目录名；写成显示名不算命中（显示名可变，身份是目录名）。
    let by_display = read_skills_for(&root, &SkillRef::Only(vec!["pretty-alpha".to_string()]))
        .expect("有技能区");
    assert!(by_display.entries.is_empty(), "显示名不是引用身份");
    assert_eq!(
        SkillsDesktop::with_index(Some(by_display), Nothing)
            .reading()
            .expect("有读数"),
        "skills: 0 项（没有启用任何技能；技能区共 2 项）"
    );

    // `none` 与 `all` 的两端。
    let none = read_skills_for(&root, &SkillRef::None).expect("有技能区");
    assert!(none.entries.is_empty());
    assert_eq!(none.total, 2);
    let all = read_skills_for(&root, &SkillRef::All).expect("有技能区");
    assert_eq!(all.entries.len(), 2);
    assert_eq!(all.total, 2);

    // **软引用的事实**：beta 没启用，但正文仍在通用文件面上读得到。
    // 这不是漏洞，是 v1 的明确边界（引用只过滤开场索引，不是 ACL）。
    let body = fs::read_to_string(root.join("skills/beta/SKILL.md")).expect("正文可读");
    assert!(body.contains("乙的步骤"), "软引用不挡 files_* 读取: {body}");
}
