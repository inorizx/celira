//! **本机持久存储的文件后端** —— 前端 `api/storage.ts` 那道窄口在壳里的实现。
//!
//! ## 为什么要有它（2026-10-05，收 app §5 行 2／行 3）
//!
//! 前端状态层（连接偏好、主题、壁纸、草稿、房间偏好）原先**只住 webview 的 `localStorage`**。
//! 那是**浏览器内部**的东西：它不在家里、不跟着家走、用户清一次站点数据就没了，而 app 的
//! 目标形态（`client/README` 与 app §9.3 第 3 步）是"**存储换成文件实现**"。窄口早就留好了
//! （`StoragePort`：四个动作、全异步、零平台分支），缺的正是**壳这一侧的后端**。
//!
//! ## 判据与形状（照 `machine_settings.rs` 那套，不新造）
//!
//! - **落点**：`<app_config_dir>/local-store.json` —— 与机器级设置同一个目录（**机器级事实归机器**；
//!   键里已经带着世界/房间的作用域，所以这一个文件就够，不按家分）；
//! - **形状**：一个 JSON 对象 `{ "<键>": "<字符串>" }`。窄口的纪律是"**只放取值**"（不认识 JSON、
//!   不认识业务）⇒ 这里也只当它是"键 → 字符串"，**一个字节都不解结构**；
//! - **写**：先写 `local-store.json.tmp` 再 `rename`（与 `machine_settings` 同款：半截文件不会
//!   留在原地）；单值上限 [`MAX_VALUE_BYTES`]，超了**如实返回 false**（"没写进去"），不静默截断；
//!   ⚠ **跨进程**（2026-10-14）：`rename` 之外还持一把**文件级**建议锁（[`LOCK_FILE`]，有界等
//!   [`LOCK_WAIT`]）—— 同一台机器上**两个壳**因此也不会丢更新、也不会抢同一个 `.tmp`
//!   （2026-10-05 那把只护同一进程，跨进程那一半当时如实登记为"不在本刀范围"）；
//! - **读不懂**（文件坏了 / 不是对象）：`read` 一律**如实返回 `None`**（不抛、不猜）；
//!   而**写**遇到坏文件时把它**整份让位**（`local-store.json.broken-<时刻>`，rename 不删）再写新的
//!   —— 既不因为一个坏字节把存储永久锁死，也不毁掉那份可能还能救的内容；
//! - **失败不抛**：读写失败一律如实返回（`None` / `false`），由调用方区分"没有这一格"与"读不出来"
//!   —— 这与窄口的接口纪律逐字一致。
//!
//! ## 一条边界（如实说）
//!
//! 这是**本机**存储：换一台机器就换一份（与"家"不同 —— 家是能搬的，这一份不是）。它装的是
//! 界面偏好与草稿这类东西，不装任何凭据（凭据在 `%LOCALAPPDATA%\Celira\credentials\`，另有其主）。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// 存储文件名（与 `settings.json` 同目录）。
pub const STORE_FILE: &str = "local-store.json";

/// **跨进程那把锁**的同伴文件名（2026-10-14）：只被创建、**从不被换**（见 [`StoreFileLock`]）。
pub const LOCK_FILE: &str = "local-store.json.lock";

/// 等这把锁最多等多久（`try_lock` 是"试一次"，这里退让重试到它为止）。
/// 2 秒对"写一个键"这种毫秒级的事足够；超了**如实失败**，不无限等。
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(2);
const LOCK_RETRY: std::time::Duration = std::time::Duration::from_millis(25);

/// 单个值的上限（1 MiB）：草稿/偏好这类东西够用；超了**如实说没写进去**，不截断。
pub const MAX_VALUE_BYTES: usize = 1 << 20;

/// 存储文件的位置。
pub fn store_path(config_dir: &Path) -> PathBuf {
    config_dir.join(STORE_FILE)
}

/// 解析存储正文（**容错**：读不懂 ⇒ 空表 —— 与"没有这一格"同一个意思，调用方按"读不到"处置）。
pub fn parse_store(text: &str) -> BTreeMap<String, String> {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) else {
        return BTreeMap::new();
    };
    let Some(object) = parsed.as_object() else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for (key, value) in object {
        if let Some(text) = value.as_str() {
            out.insert(key.clone(), text.to_string());
        }
    }
    out
}

/// 渲染存储正文（`BTreeMap` ⇒ 键有序 ⇒ 同样的内容写出同样的字节，diff 才好看）。
pub fn render_store(map: &BTreeMap<String, String>) -> String {
    let value = serde_json::Value::Object(
        map.iter()
            .map(|(key, value)| (key.clone(), serde_json::Value::String(value.clone())))
            .collect(),
    );
    format!("{value}\n")
}

/// 读整个存储（文件不在/读不懂 ⇒ 空表）。
pub fn read_store(config_dir: &Path) -> BTreeMap<String, String> {
    match fs::read_to_string(store_path(config_dir)) {
        Ok(text) => parse_store(&text),
        Err(_) => BTreeMap::new(),
    }
}

/// **写锁：读-改-写必须串行**（2026-10-05 真机日志抓出来的）。
///
/// 为什么必须有它：命令是并行跑的（`spawn_blocking`），而"写一个键"是**读整表 → 改一格 → 写回**。
/// 两个写者同时进来就会出两种病：
/// ① **丢更新** —— 各自读到同一份旧表，后写的那个把先写的整条覆盖掉；
/// ② **`os error 2`** —— 它们抢同一个 `.tmp` 名字，一个 `rename` 把 tmp 搬走之后，另一个的
///    `rename` 就"找不到指定的文件"（发布形态的日志里每 15 秒一条，正是这么来的）。
///
/// **进程内**那一半：同一进程里并行的命令（`spawn_blocking`）串行起来。
static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// **跨进程**那一半（2026-10-14）：同一台机器上**别的壳**也在写同一份存储。
///
/// 为什么必须有它：2026-10-05 那把锁只护**同一进程**，而"写一个键"是**读整表 → 改一格 → 写回**
/// ⇒ 两个**壳**同时进来照样丢更新、照样抢同一个 `.tmp`（发布形态日志里那条 `os error 2` 的
/// 跨进程版本）。⇒ 把同一把锁**抬到文件级**：`std::fs::File::try_lock`（**操作系统级**建议锁，
/// 与 runtime 护环境根那把 `.writer.lock` 是同一套机制、同一句理由："它不是'检查文件是否存在'
/// 这种可以被绕过的东西"）。
///
/// ⚠ **为什么锁一个同伴文件、不锁存储文件自己**：存储是"写临时文件再 `rename`" ⇒ `rename` 之后
/// 原来那个文件已经没人持有了，锁会**静默失效**。同伴文件只被创建、**从不被换**，所以锁得住。
///
/// ⚠ **有界等待**：`try_lock` 是"试一次" ⇒ 这里退让重试到 [`LOCK_WAIT`]；超了**如实返回失败**
/// （调用方按"没写进去"处置）——不无限等，也不假装成功。
struct StoreFileLock {
    _file: fs::File,
}

impl StoreFileLock {
    fn acquire(config_dir: &Path) -> Result<Self, String> {
        if let Err(error) = fs::create_dir_all(config_dir) {
            return Err(format!(
                "建配置目录失败（{}）：{error}",
                config_dir.display()
            ));
        }
        let path = config_dir.join(LOCK_FILE);
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|error| format!("打不开写锁文件（{}）：{error}", path.display()))?;
        let deadline = std::time::Instant::now() + LOCK_WAIT;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(fs::TryLockError::WouldBlock) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(format!(
                            "另一个壳正拿着这把写锁（等了 {} ms）：{} 没写进去",
                            LOCK_WAIT.as_millis(),
                            path.display()
                        ));
                    }
                    std::thread::sleep(LOCK_RETRY);
                }
                Err(fs::TryLockError::Error(error)) => {
                    return Err(format!("上写锁失败（{}）：{error}", path.display()));
                }
            }
        }
    }
}

/// 把"读-改-写"整段关进两把锁：**先是文件级**（跨进程），再是进程内。
///
/// 顺序**只有这一处**（同一个入口 ⇒ 不可能出现"你锁我、我锁你"）；且文件锁是**有界**的
/// ⇒ 最坏情况是"如实失败"，不是死等。
fn with_write_lock<T>(config_dir: &Path, body: impl FnOnce() -> T) -> Result<T, String> {
    let _file = StoreFileLock::acquire(config_dir)?;
    // 中毒（别的线程 panic 过）也照旧进去：这里护的是一段纯文件读写，没有跨调用的不变量会因此
    // 半途而废 —— 卡死比继续更糟。
    let guard = WRITE_LOCK.lock().unwrap_or_else(|each| each.into_inner());
    let out = body();
    drop(guard);
    Ok(out)
}

/// 写整个存储（tmp ＋ rename；坏文件先**整份让位**，不删）。**整个写是串行的**（见 [`WRITE_LOCK`]）。
pub fn write_store(config_dir: &Path, map: &BTreeMap<String, String>) -> Result<(), String> {
    with_write_lock(config_dir, || write_store_locked(config_dir, map))?
}

fn write_store_locked(config_dir: &Path, map: &BTreeMap<String, String>) -> Result<(), String> {
    if let Err(error) = fs::create_dir_all(config_dir) {
        return Err(format!(
            "建配置目录失败（{}）：{error}",
            config_dir.display()
        ));
    }
    let path = store_path(config_dir);
    // 坏文件让位：它读不懂 ⇒ 这次写会把它覆盖掉 ⇒ 先整份挪开（rename，不删），并留一条日志。
    if let Ok(text) = fs::read_to_string(&path) {
        if serde_json::from_str::<serde_json::Value>(&text).is_err() {
            let aside = free_broken_path(&path);
            if let Err(error) = fs::rename(&path, &aside) {
                return Err(format!(
                    "存储文件读不懂、又让不开位（{} ⇒ {}：{error}）—— 不覆盖它",
                    path.display(),
                    aside.display()
                ));
            }
            log::warn!(
                "[本机存储] {} 读不懂 ⇒ 整份让位到 {}（不删），这次写一份新的",
                path.display(),
                aside.display()
            );
        }
    }
    let temporary = config_dir.join(format!("{STORE_FILE}.tmp"));
    fs::write(&temporary, render_store(map))
        .map_err(|error| format!("写临时存储失败（{}）：{error}", temporary.display()))?;
    fs::rename(&temporary, &path)
        .map_err(|error| format!("存储落盘失败（{}）：{error}", path.display()))
}

/// 坏文件让位时那个空位（被占就 `-2`…`-99`，最后退回时间戳）。
fn free_broken_path(path: &Path) -> PathBuf {
    let base = PathBuf::from(format!("{}.broken", path.display()));
    if !base.exists() {
        return base;
    }
    for n in 2..100 {
        let candidate = PathBuf::from(format!("{}-{n}", base.display()));
        if !candidate.exists() {
            return candidate;
        }
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|each| each.as_millis())
        .unwrap_or(0);
    PathBuf::from(format!("{}-{stamp}", base.display()))
}

// ---------------------------------------------------------------------------
// 窄口的四个动作（**读不到 / 写失败一律如实返回**，不抛）
// ---------------------------------------------------------------------------

/// 读一个键（没有 ⇒ `None`；读不出来 ⇒ 也是 `None` —— 窄口的形状就是这两态）。
pub fn read_key(config_dir: &Path, key: &str) -> Option<String> {
    read_store(config_dir).get(key).cloned()
}

/// 写一个键（成功 ⇒ `true`；超上限 / 写不进去 ⇒ `false`，**如实说没写进去**）。
///
/// ⚠ **读-改-写整段在锁里**（2026-10-05，真机日志抓出来的那个 bug）：只锁"写"是不够的 ——
/// 两个写者会各自读到同一份旧表，后写的那个把先写的整条覆盖掉（实测：8 线程并发写 48 个键，
/// 修之前丢键）。所以这里**不调** [`write_store`]（那个自己也上锁 ⇒ 会死锁），而是调不加锁的内层。
pub fn write_key(config_dir: &Path, key: &str, value: &str) -> bool {
    if key.is_empty() || value.len() > MAX_VALUE_BYTES {
        return false;
    }
    match with_write_lock(config_dir, || {
        let mut map = read_store(config_dir);
        map.insert(key.to_string(), value.to_string());
        match write_store_locked(config_dir, &map) {
            Ok(()) => true,
            Err(error) => {
                log::warn!("[本机存储] 写 {key} 失败：{error}");
                false
            }
        }
    }) {
        Ok(written) => written,
        // 拿不到锁（别的壳正写着）⇒ **如实说没写进去**（窄口的形状就是这两态）。
        Err(error) => {
            log::warn!("[本机存储] 写 {key} 失败：{error}");
            false
        }
    }
}

/// 删一个键（删不掉就删不掉 —— 这不是调用方要处理的事）。**读-改-写整段在锁里**，同 [`write_key`]。
pub fn remove_key(config_dir: &Path, key: &str) {
    let outcome = with_write_lock(config_dir, || {
        let mut map = read_store(config_dir);
        if map.remove(key).is_none() {
            return;
        }
        if let Err(error) = write_store_locked(config_dir, &map) {
            log::warn!("[本机存储] 删 {key} 失败：{error}");
        }
    });
    if let Err(error) = outcome {
        log::warn!("[本机存储] 删 {key} 失败：{error}");
    }
}

/// 以 `prefix` 开头的键（用于清理）。
pub fn keys_with_prefix(config_dir: &Path, prefix: &str) -> Vec<String> {
    read_store(config_dir)
        .into_keys()
        .filter(|key| key.starts_with(prefix))
        .collect()
}

// ---------------------------------------------------------------------------
// 命令（壳↔前端的最窄面）
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn local_store_read(
    config: tauri::State<'_, crate::MachineConfigDir>,
    key: String,
) -> Result<Option<String>, String> {
    let config_dir = config.0.clone();
    Ok(
        tauri::async_runtime::spawn_blocking(move || read_key(&config_dir, &key))
            .await
            .unwrap_or(None),
    )
}

#[tauri::command]
pub async fn local_store_write(
    config: tauri::State<'_, crate::MachineConfigDir>,
    key: String,
    value: String,
) -> Result<bool, String> {
    let config_dir = config.0.clone();
    Ok(
        tauri::async_runtime::spawn_blocking(move || write_key(&config_dir, &key, &value))
            .await
            .unwrap_or(false),
    )
}

#[tauri::command]
pub async fn local_store_remove(
    config: tauri::State<'_, crate::MachineConfigDir>,
    key: String,
) -> Result<(), String> {
    let config_dir = config.0.clone();
    let _ = tauri::async_runtime::spawn_blocking(move || remove_key(&config_dir, &key)).await;
    Ok(())
}

#[tauri::command]
pub async fn local_store_keys(
    config: tauri::State<'_, crate::MachineConfigDir>,
    prefix: String,
) -> Result<Vec<String>, String> {
    let config_dir = config.0.clone();
    Ok(
        tauri::async_runtime::spawn_blocking(move || keys_with_prefix(&config_dir, &prefix))
            .await
            .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "celira-store-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|each| each.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn a_value_round_trips_and_survives_a_new_read() {
        let dir = scratch("roundtrip");
        assert!(
            read_key(&dir, "prefs:connection").is_none(),
            "还没有 ⇒ 读不到（不是空串）"
        );
        assert!(write_key(
            &dir,
            "prefs:connection",
            "{\"server\":\"http://x\"}"
        ));
        assert_eq!(
            read_key(&dir, "prefs:connection").as_deref(),
            Some("{\"server\":\"http://x\"}"),
            "**一个字节都不解结构**：存什么读什么"
        );
        // 第二个键进来，第一个还在（读-改-写）。
        assert!(write_key(&dir, "theme", "dark"));
        assert_eq!(
            read_key(&dir, "prefs:connection").as_deref(),
            Some("{\"server\":\"http://x\"}")
        );
        assert_eq!(read_key(&dir, "theme").as_deref(), Some("dark"));
        // 落盘的是**一个 JSON 对象**（人看得懂、能救）；`serde_json` 渲染是紧凑的。
        let text = fs::read_to_string(store_path(&dir)).unwrap();
        assert!(text.contains("\"theme\":\"dark\""), "{text}");
        assert!(text.contains("\"prefs:connection\""), "{text}");
        assert!(
            !dir.join(format!("{STORE_FILE}.tmp")).exists(),
            "临时文件不许留下"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_are_filtered_by_prefix_and_remove_is_exact() {
        let dir = scratch("keys");
        for key in [
            "room:world-a:r1",
            "room:world-a:r2",
            "room:world-b:r1",
            "theme",
        ] {
            assert!(write_key(&dir, key, "x"));
        }
        let mut mine = keys_with_prefix(&dir, "room:world-a:");
        mine.sort();
        assert_eq!(mine, vec!["room:world-a:r1", "room:world-a:r2"]);
        assert_eq!(keys_with_prefix(&dir, "没有这个前缀"), Vec::<String>::new());
        remove_key(&dir, "room:world-a:r1");
        assert!(read_key(&dir, "room:world-a:r1").is_none());
        assert_eq!(
            read_key(&dir, "room:world-a:r2").as_deref(),
            Some("x"),
            "别的键不许被误删"
        );
        // 删一个不存在的键：什么都不做（也不报错），别的键一个都不许动。
        remove_key(&dir, "从来没写过");
        assert_eq!(
            read_key(&dir, "theme").as_deref(),
            Some("x"),
            "这一支写的就是 x"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_or_unreadable_file_reads_as_absent_not_as_an_error() {
        let dir = scratch("absent");
        assert!(read_key(&dir, "任何键").is_none(), "文件不在 ⇒ 读不到");
        fs::write(store_path(&dir), "{ 这不是 JSON").unwrap();
        assert!(
            read_key(&dir, "任何键").is_none(),
            "读不懂 ⇒ 也读不到（如实，不猜）"
        );
        fs::write(store_path(&dir), "[1,2,3]").unwrap();
        assert!(read_key(&dir, "任何键").is_none(), "不是对象 ⇒ 一样读不到");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_steps_aside_instead_of_being_overwritten() {
        let dir = scratch("broken");
        fs::write(store_path(&dir), "{ 坏掉了").unwrap();
        // 坏文件在场时**写得进去**（存储不许被一个坏字节永久锁死）……
        assert!(write_key(&dir, "theme", "dark"));
        assert_eq!(read_key(&dir, "theme").as_deref(), Some("dark"));
        // ……而那份坏的**整份让位**（rename，不删）：内容还在，人能去救。
        let aside = PathBuf::from(format!("{}.broken", store_path(&dir).display()));
        assert!(aside.exists(), "坏文件该让位到 {}", aside.display());
        assert_eq!(fs::read_to_string(&aside).unwrap(), "{ 坏掉了");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_writes_do_not_lose_updates_or_race_on_the_temp_file() {
        // ★ 2026-10-05：真机日志里每 15 秒一条 `os error 2`，根因就是并发（见 `WRITE_LOCK`）。
        // 这个用例**故意**让 N 个线程同时写**不同的键**：修好之前它必然红（要么丢更新、要么
        // 抢 `.tmp` 抢到 rename 找不到源），修好之后每个键都在。
        let dir = scratch("concurrent");
        let threads = 8;
        let per_thread = 6;
        let mut handles = Vec::new();
        for t in 0..threads {
            let dir = dir.clone();
            handles.push(std::thread::spawn(move || {
                for n in 0..per_thread {
                    let key = format!("cursor:world-{t}:{n}");
                    assert!(
                        write_key(&dir, &key, &format!("值-{t}-{n}")),
                        "写 {key} 该成功"
                    );
                }
            }));
        }
        for handle in handles {
            handle.join().expect("线程不许 panic");
        }
        let map = read_store(&dir);
        for t in 0..threads {
            for n in 0..per_thread {
                let key = format!("cursor:world-{t}:{n}");
                assert_eq!(
                    map.get(&key).map(String::as_str),
                    Some(format!("值-{t}-{n}").as_str()),
                    "并发写之后 {key} 丢了（读-改-写没串行）"
                );
            }
        }
        assert_eq!(
            map.len(),
            threads * per_thread,
            "一共该有这么多键，一个不多一个不少"
        );
        assert!(
            !dir.join(format!("{STORE_FILE}.tmp")).exists(),
            "临时文件不许留下"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_value_cap_is_reported_not_silently_truncated() {
        let dir = scratch("cap");
        let big = "x".repeat(MAX_VALUE_BYTES + 1);
        assert!(!write_key(&dir, "draft", &big), "超上限 ⇒ 如实说没写进去");
        assert!(
            read_key(&dir, "draft").is_none(),
            "没写进去就是没写进去（不许截断存半截）"
        );
        assert!(
            write_key(&dir, "draft", &"x".repeat(MAX_VALUE_BYTES)),
            "正好到上限 ⇒ 写得进"
        );
        assert!(!write_key(&dir, "", "值"), "空键不是键");
        let _ = fs::remove_dir_all(&dir);
    }

    /// **跨进程那一半**（2026-10-14）：同一台机器上**两个壳**（各自一个句柄）不能同时进
    /// "读-改-写"。
    ///
    /// 判据与 2026-10-05 那条同源，只是抬了一层——`WRITE_LOCK` 护不住**别的壳**：两个壳会各自
    /// 读到同一份旧表（丢更新），也会抢同一个 `.tmp`（`os error 2` 的跨进程版本）。
    ///
    /// ⚠ **为什么一个进程里两个句柄就够证明**：`File::try_lock` 是**操作系统级**建议锁，按**句柄**
    /// 判（Windows 的 `LockFileEx` / Unix 的 `flock` 都落在"这一次 open"上）⇒ 两个句柄与两个进程
    /// 同形。⇒ 这条用例不需要真起第二个进程。
    ///
    /// 三件一起钉：① 第二个持有者**被拒**；② 它是**有界等待**之后才拒的（不是立刻放弃）；③ 放掉
    /// 之后**立刻**能再拿（锁真的还回来了——这一条防的是"加了锁却忘了放"）。
    #[test]
    fn a_second_holder_is_refused_and_the_lock_comes_back_when_dropped() {
        let dir = scratch("cross-process-lock");
        let first = StoreFileLock::acquire(&dir).expect("第一个持有者该拿到");
        let started = std::time::Instant::now();
        let second = StoreFileLock::acquire(&dir);
        assert!(
            second.is_err(),
            "同一份存储的第二个持有者必须被拒（这就是跨进程那一半）"
        );
        assert!(
            started.elapsed() >= LOCK_WAIT,
            "要有界**等待**，不是立刻放弃：{:?}",
            started.elapsed()
        );
        drop(first);
        assert!(
            StoreFileLock::acquire(&dir).is_ok(),
            "放掉之后必须立刻能再拿（加了锁就得放）"
        );
        // 写一个键照样通：锁没有把正常路径堵死。
        assert!(write_key(&dir, "theme", "dark"));
        assert_eq!(read_key(&dir, "theme").as_deref(), Some("dark"));
        let _ = fs::remove_dir_all(&dir);
    }
}
