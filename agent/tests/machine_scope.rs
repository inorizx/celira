//! **机器档路径判据**（`sandbox::machine_path` / `resolve_machine`）的证明。
//!
//! 它服务的是文件 App 的**全开档**：主人把整台机器当文件面给了这一支（产品口径见
//! `docs/contracts/core-v1/CORE_V1_ACCEPTANCE.md` 那一族"支级授权"的裁决；实现口径见
//! `sandbox.rs` 的同名文档）。
//!
//! 与家档（相对路径 ＋ 一个根）的差别只有一条：**这里没有一个"根"把逃逸兜住** ——
//! 所以形状判据必须自己把命名空间关死：UNC、`\\?\`、`\\.\`、盘符相对（`C:foo`）、
//! 第二个 `:`（ADS）、`..`、Windows 保留设备名、以点/空格结尾的名字，一样不许过。
//!
//! 两档**互不接受对方的写法**：机器档收相对路径而拒之，家档收绝对路径而拒之。
//!
//! 沙盒等级与家档同档：受信目录、检查与打开之间有理论替换窗口，**不构成 OS 级安全沙盒**。

use std::fs;
use std::path::{Path, PathBuf};

use celira_agent::sandbox::{SandboxError, machine_path, resolve_machine, resolve_machine_parent};

/// 自建临时根（只在系统临时目录下，用完删掉自己那一份）。
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let unique = format!(
            "celira-machine-scope-{}-{}-{}",
            std::process::id(),
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("时钟可用")
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        fs::create_dir_all(&root).expect("可以建临时目录");
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn write(&self, relative: &str, text: &str) -> PathBuf {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("可以建子目录");
        }
        fs::write(&path, text).expect("可以写合成内容");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// 本机上一个**确定不存在**的卷（挑一个不在的盘符；Linux 上恒为 `None`）。
#[cfg(windows)]
fn absent_volume() -> Option<&'static str> {
    ["Q:\\", "X:\\", "Y:\\", "Z:\\"]
        .into_iter()
        .find(|candidate| fs::symlink_metadata(candidate).is_err())
}

// ---------------------------------------------------------------------------
// 一、形状：只认"盘符 ＋ 分隔符"开头的那一种绝对路径
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn windows_shape_splits_drive_absolute_into_root_and_names() {
    let target = machine_path(r"C:\Users\x\notes\a.txt").expect("盘符绝对路径合法");
    assert_eq!(target.root(), Path::new("C:\\"));
    assert_eq!(target.names(), ["Users", "x", "notes", "a.txt"]);
    assert!(!target.is_volume_root());

    // 大小写与分隔符风格都归一：盘符进根（大写），名字段原样保留。
    let mixed = machine_path("d:/data/File.TXT").expect("正斜杠也合法（Windows 两种都吃）");
    assert_eq!(mixed.root(), Path::new("D:\\"));
    assert_eq!(mixed.names(), ["data", "File.TXT"]);

    // 卷根本身是合法目标（列目录要它）。
    let root = machine_path(r"C:\").expect("卷根合法");
    assert!(root.is_volume_root());
    assert_eq!(root.root(), Path::new("C:\\"));

    // 连续分隔符不产生空名字。
    let doubled = machine_path(r"C:\a\\b").expect("连续分隔符合法");
    assert_eq!(doubled.names(), ["a", "b"]);
}

#[cfg(windows)]
#[test]
fn windows_shape_refuses_everything_that_is_not_a_drive_absolute() {
    for refused in [
        "",                   // 空
        "x",                  // 相对
        r"x\y",               // 相对（带分隔符）
        r".\x",               // 当前目录
        r"\x",                // 没有盘符的根相对
        "C:x",                // **盘符相对**（当前盘上的 x）——不是绝对路径
        r"C:\a\..\b",         // ..
        r"C:\..\x",           // ..
        r"C:\.",              // .
        r"\\server\share\x",  // UNC
        r"\\?\C:\x",          // 逐字前缀
        r"\\.\COM1",          // 设备命名空间
        r"C:\a\b.txt:stream", // ADS（第二个冒号）
        r"C:\a\b.txt::$DATA", // 具名数据流
        r"C:\con\x",          // 保留设备名
        r"C:\NUL.txt",        // 带扩展名的保留名还是设备
        "C:\\trailing. ",     // 以点/空格结尾（Win32 会悄悄剪掉）
        r"C:\a\b:alt",        // 再一个 ADS 形状
    ] {
        assert_eq!(
            machine_path(refused).unwrap_err(),
            SandboxError::InvalidPath,
            "这个形状必须拒：{refused:?}",
        );
    }
}

#[cfg(unix)]
#[test]
fn unix_shape_splits_absolute_into_root_and_names() {
    let target = machine_path("/home/u/notes/a.txt").expect("绝对路径合法");
    assert_eq!(target.root(), Path::new("/"));
    assert_eq!(target.names(), ["home", "u", "notes", "a.txt"]);
    assert!(machine_path("/").expect("根本身合法").is_volume_root());

    for refused in [
        "",
        "x",
        "x/y",
        "./x",
        "/home/../etc",
        "/.",
        r"C:\x",
        "/a//../b",
    ] {
        assert_eq!(
            machine_path(refused).unwrap_err(),
            SandboxError::InvalidPath,
            "这个形状必须拒：{refused:?}",
        );
    }
}

// ---------------------------------------------------------------------------
// 二、落实：逐段确认存在、逐段拒绝链接；卷不在是 NotFound（不是 InvalidPath）
// ---------------------------------------------------------------------------

#[test]
fn resolution_walks_the_real_chain_and_refuses_missing_segments() {
    let scratch = Scratch::new("resolve");
    let file = scratch.write("sub/a.txt", "hello");
    let absolute = file.display().to_string();

    let target = machine_path(&absolute).expect("临时文件是绝对路径");
    let resolved = resolve_machine(&target).expect("逐段都在，应当解析得出来");
    // 不规范化：逐段拼回去，与输入同形（与家档 `resolve_within` 同一口径）。
    assert_eq!(resolved, scratch.path().join("sub").join("a.txt"));

    // 中途缺一段 ⇒ NotFound（**不是** InvalidPath：形状没问题，是它不在）。
    let missing = scratch
        .path()
        .join("nope")
        .join("a.txt")
        .display()
        .to_string();
    assert_eq!(
        resolve_machine(&machine_path(&missing).expect("形状仍合法")).unwrap_err(),
        SandboxError::NotFound,
    );

    // 根在、但整条路径不存在 ⇒ 同上。
    let ghost = scratch.path().join("ghost").display().to_string();
    assert_eq!(
        resolve_machine(&machine_path(&ghost).expect("形状仍合法")).unwrap_err(),
        SandboxError::NotFound,
    );
}

#[test]
fn create_shape_resolves_only_the_parent_and_leaves_the_last_name_alone() {
    let scratch = Scratch::new("parent");
    scratch.write("sub/existing.txt", "hello");

    // 父在、末段不在 ⇒ 这正是"新建"要的形状：交回（父目录, 名字），**不要求末段存在**。
    let fresh = scratch.path().join("sub").join("brand-new.txt");
    let fresh_text = fresh.display().to_string();
    let (dir, name) = resolve_machine_parent(&machine_path(&fresh_text).expect("形状合法"))
        .expect("父目录在，应当解析得出来");
    assert_eq!(dir, scratch.path().join("sub"));
    assert_eq!(name, "brand-new.txt");
    assert!(!fresh.exists(), "这一步只解析，不创建任何东西");

    // 父不在 ⇒ NotFound（父目录要先存在；这一层不替主体建目录）。
    let orphan = scratch
        .path()
        .join("ghost")
        .join("a.txt")
        .display()
        .to_string();
    assert_eq!(
        resolve_machine_parent(&machine_path(&orphan).expect("形状合法")).unwrap_err(),
        SandboxError::NotFound,
    );

    // 父是一个**文件** ⇒ WrongKind（不是"新建失败"，是不该拿它当目录）。
    let not_a_dir = scratch
        .path()
        .join("sub")
        .join("existing.txt")
        .join("a.txt")
        .display()
        .to_string();
    assert_eq!(
        resolve_machine_parent(&machine_path(&not_a_dir).expect("形状合法")).unwrap_err(),
        SandboxError::WrongKind,
    );

    // 卷根本身不是可以新建文件的地方（没有末段）。
    let volume = machine_path(if cfg!(windows) { "C:\\" } else { "/" }).expect("形状合法");
    assert_eq!(
        resolve_machine_parent(&volume).unwrap_err(),
        SandboxError::InvalidPath,
    );
}

#[test]
fn an_absent_volume_is_not_found_not_invalid_path() {
    #[cfg(windows)]
    {
        let Some(absent) = absent_volume() else {
            // 这台机器每个候选盘符都在：本用例**未覆盖**，如实报。
            eprintln!("未覆盖：Q/X/Y/Z 四个盘符都真实存在，没有可以拿来判「卷不在」的目标");
            return;
        };
        let probe = format!("{absent}celira-nonexistent");
        let target = machine_path(&probe).expect("形状合法（那个卷只是不在）");
        assert_eq!(
            resolve_machine(&target).unwrap_err(),
            SandboxError::NotFound
        );
    }
    #[cfg(unix)]
    {
        let target = machine_path("/celira-nonexistent-root").expect("形状合法");
        assert_eq!(
            resolve_machine(&target).unwrap_err(),
            SandboxError::NotFound
        );
    }
}

/// 链上出现链接 ⇒ `link`（能建就验，建不了就**明确报告未覆盖** —— 与 `tests/file_app.rs`
/// 的同一条纪律；它不假装跑过）。
#[test]
fn a_link_in_the_chain_is_refused_or_reported_as_uncovered() {
    let scratch = Scratch::new("link");
    let real = scratch.write("real/a.txt", "hello");
    let link = scratch.path().join("link.txt");
    #[cfg(windows)]
    let created = std::os::windows::fs::symlink_file(&real, &link).is_ok();
    #[cfg(unix)]
    let created = std::os::unix::fs::symlink(&real, &link).is_ok();
    if !created {
        eprintln!("未覆盖：这个环境建不了符号链接（Windows 需要开发者模式或相应权限）");
        return;
    }

    let absolute = link.display().to_string();
    let target = machine_path(&absolute).expect("链接路径形状合法");
    assert_eq!(
        resolve_machine(&target).unwrap_err(),
        SandboxError::Link,
        "机器档与家档同一条：链上任何一段是链接都拒",
    );
}
