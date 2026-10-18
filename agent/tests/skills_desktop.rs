//! 验收：技能桌面（材料区资源索引）——**降级为一行读数**后的形状、计数口径、缺省语义、零正文。
//!
//! 2026-09-22 裁决：技能是"**有时**才需要"的信息 ⇒ 不再逐条列名字与说明，整块只留一行读数
//! （`skills: N 项（要翻看用 files_list，路径 skills/）`）。因此本文件里锁定旧行为的断言
//! （逐条渲染、预览截断、一次最多几条）随行为一起改写——这是行为变更刀，测试跟着行为走。

use std::fs;
use std::path::{Path, PathBuf};

use celira_agent::core::{ContentBatch, ContextSource, InjectionContext};
use celira_agent::skills_desktop::{SKILLS_DESKTOP_LABEL, SkillsDesktop, read_skills};

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

/// **正例（降级行）**：技能区里有两个一级条目 ⇒ 一行读数报出**真实条目数**，并给出翻看的
/// 动词名与路径；名字、说明、正文一律**不再进场**。
#[test]
fn a_non_empty_skills_area_is_one_reading_with_the_real_count() {
    let root = sandbox("downgrade");
    write_skill(
        &root,
        "skills/web-search/SKILL.md",
        &skill_md("web-search", "在网上搜东西并回答", ""),
    );
    write_skill(
        &root,
        "skills/notes-pack/notes-tidy/SKILL.md",
        &skill_md("notes-tidy", "把笔记整理成条目", ""),
    );
    write_skill(
        &root,
        "skills/notes-pack/notes-search/SKILL.md",
        &skill_md("notes-search", "在笔记里找东西", ""),
    );

    // 数的是**一级条目**（`skills/` 的直接子目录）：散 skill 一个 ＋ 包一个 = 2（包里的子技能不另算）。
    assert_eq!(read_skills(&root), Some(2), "一级条目 = 直接子目录");

    let desktop = SkillsDesktop::new(read_skills(&root), Nothing);
    let reading = desktop.reading().expect("有技能区就有读数");
    assert_eq!(
        reading, "skills: 2 项（要翻看用 files_list，路径 skills/）",
        "整块就是这一行——计数 ＋ 翻看的动词与路径（免查也免学）"
    );
    assert_eq!(reading.lines().count(), 1, "一行就是一行: {reading}");
    // **不再逐条**：名字与说明本来就在 `files` 的地盘，这里不抄第二份。
    assert!(
        !reading.contains("web-search")
            && !reading.contains("notes-pack")
            && !reading.contains("在网上搜东西并回答"),
        "逐条名字/说明不许再进场: {reading}"
    );
    assert!(
        !reading.contains('|'),
        "旧的一行一条形状不该留下: {reading}"
    );
    // 零正文：桌面块里没有任何清单正文。
    assert!(
        !reading.contains("# 正文") && !reading.contains("# 步骤"),
        "桌面块不许带正文: {reading}"
    );
    assert_eq!(desktop.line().expect("有块").label(), SKILLS_DESKTOP_LABEL);
}

/// 计数的口径要**机械**：数的是目录，不是"能解析出的技能"。因此
/// ① 没有 `SKILL.md` 的一级目录照样算一项（包、或主体刚建的空目录）；
/// ② `skills/` 下的**散文件**不是技能，不计入。
#[test]
fn the_count_is_the_number_of_top_level_directories() {
    let root = sandbox("counting");
    // ① 一级目录：散 skill（有清单）
    write_skill(&root, "skills/flat-skill/SKILL.md", "# 只有正文\n");
    // ① 一级目录：包（自己没有清单，下面有子技能）
    write_skill(
        &root,
        "skills/a-pack/inner/SKILL.md",
        &skill_md("inner", "包里的一项", ""),
    );
    // ① 一级目录：空目录（没有任何清单）——它**照样是一项**（数的是结构，不是可解析性）。
    fs::create_dir_all(root.join("skills/empty-entry")).expect("建空的一级目录");
    // ② 散文件不是技能。
    fs::write(root.join("skills/README.md"), "散落在技能区的文件\n").expect("写散文件");

    assert_eq!(
        read_skills(&root),
        Some(3),
        "三个一级目录（散 skill ＋ 包 ＋ 空目录）；散文件不计入"
    );

    let reading = SkillsDesktop::new(read_skills(&root), Nothing)
        .reading()
        .expect("有读数");
    assert_eq!(reading, "skills: 3 项（要翻看用 files_list，路径 skills/）");
}

/// **清单内容不再参与**（旧纪律的接替者）：一级目录里的 `SKILL.md` 长什么样**都不影响**这一行读数
/// ——坏 frontmatter、超长描述、根本没有清单，计数都一样。
///
/// 旧件里有"名字优先 frontmatter、回退目录名"与"预览按字节截断、控制字符压成空格"两条纪律，
/// 它们守着的是**逐条渲染**；逐条渲染降级后没有消费者 ⇒ 那两条随之退场。这条用例是它们的接替者：
/// 它证明**解析器不再在链上**（因此没有"渲染卫生"可失守，也没有名字可回退）。
#[test]
fn the_manifest_content_no_longer_changes_the_reading() {
    let root = sandbox("manifest-ignored");
    // ① frontmatter 坏掉（没有闭合的 `---`）、描述里带控制字符与换行。
    write_skill(
        &root,
        "skills/broken/SKILL.md",
        "---\nname: 坏清单\ndescription: 第一行\n第二行\t带控制字符\n",
    );
    // ② 描述超长（旧件会截断并加 `…`）。
    write_skill(
        &root,
        "skills/wordy/SKILL.md",
        &skill_md("wordy", &"描".repeat(5_000), ""),
    );
    // ③ 一级目录里压根没有清单。
    fs::create_dir_all(root.join("skills/silent")).expect("建没有清单的一级目录");

    assert_eq!(read_skills(&root), Some(3), "计数不依赖清单能不能解析");

    let reading = SkillsDesktop::new(read_skills(&root), Nothing)
        .reading()
        .expect("有读数");
    assert_eq!(reading, "skills: 3 项（要翻看用 files_list，路径 skills/）");
    assert!(
        !reading.contains('…') && !reading.contains("坏清单") && !reading.contains('描'),
        "清单内容一个字节都不许渗进这一行: {reading}"
    );
}

/// **降级的机械版**：读数不随技能数增长。三百个一级条目仍然**一行**。
///
/// 这就是"按需要定价"在本件的落法：常量成本在"有多少"那一个数上，不在"逐条"上。
#[test]
fn the_reading_does_not_grow_with_the_number_of_skills() {
    let root = sandbox("many");
    for index in 0..300 {
        fs::create_dir_all(root.join(format!("skills/pack-{index:03}"))).expect("建一级条目目录");
    }

    assert_eq!(read_skills(&root), Some(300));
    let reading = SkillsDesktop::new(read_skills(&root), Nothing)
        .reading()
        .expect("有读数");
    assert_eq!(reading.lines().count(), 1, "三百个技能也是一行: {reading}");
    assert!(reading.contains("skills: 300 项"), "{reading}");
    assert!(!reading.contains("pack-000"), "逐条名字不再进场: {reading}");
}

/// **缺省不等于零**（两个负对照）：
/// ① 材料区**没有** `skills/` ⇒ 整块不出场（不是空标题、也不是 `0 项`）；
/// ② 技能区在、但一个一级条目都没有 ⇒ 如实说"为空"。
#[test]
fn a_missing_skills_directory_has_no_block_and_an_empty_one_says_it_is_empty() {
    // ① 负对照：没有 skills/。
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

    // ① 的另一半：`skills` 在，但**不是目录**（同名文件）⇒ 同样没有技能区。
    let file = sandbox("not-a-dir");
    fs::write(file.join("skills"), "这不是目录\n").expect("写同名文件");
    assert_eq!(read_skills(&file), None, "同名的文件不是技能区");

    // ② 负对照：技能区在、但空的。
    let empty = sandbox("empty");
    fs::create_dir_all(empty.join("skills")).expect("建技能区");
    assert_eq!(read_skills(&empty), Some(0));
    let reading = SkillsDesktop::new(read_skills(&empty), Nothing)
        .reading()
        .expect("有块（存在但空）");
    assert_eq!(reading, "skills: 0 项（技能区为空）", "空就如实说空");
}
