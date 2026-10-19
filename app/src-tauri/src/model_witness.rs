//! **模型配置那两份文件的见证**（壳这一份；形状权威是 `scripts/home/model_witness.js`，逐条同形）。
//!
//! ## 它回答的唯一问题
//!
//! **这一份最后是「从页面改的」吗？如果是，什么时候改的？**
//!
//! ⚠ 它**答不出"是谁"**：答得出的是**两档**，而且**两档必须分开说**——
//! 「从页面那条路改的」**有时刻**；「别处改的」（主体走 terminal 写、直接改文件、别的工具）
//! **只知"变了"，不知是谁**。⇒ 判据一句话：**它答的是「页面有没有写过」，不是「有没有人写过」。**
//!
//! ## 缝收在**门上**，不在 `write_atomic` 上（这一条是承重的）
//!
//! `model_sources.rs` 的 `write_atomic` 也承载**不是页面**的写（`home.js up` 的迁移、
//! 布置时的绑定携带）。在那一个函数上记账，会把**布置**说成**"页面改的"** ——
//! 而这一层存在的全部理由就是不许把这两件事混成一件。
//! ⇒ 记账的调用点只有两处：这道门（`home_doors.rs`）与 JS 那道门（`config-server.js`）。
//!
//! ## 链值算在**一条规范行**上，不算在 JSON 序列化上
//!
//! 这本账有**两个写者、两种语言**。链值若算在各自的 JSON 序列化上，两边的转义必须逐字节
//! 相同 —— 那是一个没人守得住的约定。⇒ 链值 =
//! `sha256(utf8("<at-ms>\n<file>\n<size>\n<sha256|''>\n<prev|''>\n"))`，
//! 两个实现各钉**同一个测试向量**（本文件底部的用例与 JS 那份的 `VECTOR`）。
//!
//! ## 它抓的是「改动」，不是「篡改」（这一条必须说出口）
//!
//! 同机同用户 ⇒ 主体够得着那两份文件，也够得着这本账。⇒ **本地见证对拥有整机全权的主体不是
//! 封印**。它是**改动的可见性**，**不是密码学保证** —— 不许把它说成后者。
//!
//! ## 它不拒绝任何东西，也**绝不**让一笔写失败
//!
//! 记不下就**如实报一行**（调用方 `eprintln!`），那一笔配置写**照旧成功**。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// 见证账本的文件名（住在 `<家>/logs/`，与 `world-witness.ndjson` 同族）。
pub const WITNESS_NAME: &str = "model-witness.ndjson";

/// 记录里**恰好**这些键（顺序照 JS 那份排过；认不得的形状一律拒绝 —— fail-closed）。
pub const RECORD_KEYS: [&str; 5] = ["at-ms", "file", "prev", "sha256", "size"];

/// 一条记录（`sha256: None` = 那一刻这一份**不在**：门摘掉了它）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub at_ms: i64,
    pub file: String,
    pub size: i64,
    pub sha256: Option<String>,
    pub prev: Option<String>,
}

/// 见证账本的路径。
pub fn witness_path(home_dir: &Path) -> PathBuf {
    home_dir.join("logs").join(WITNESS_NAME)
}

/// 十六进制摘要（与 `home_facts::world_key` 同一个写法）。
fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// **哪一份** —— 相对家的路径，`/` 分隔，**可打印 ASCII**（说不成 ⇒ `None`，这一笔不记）。
///
/// 与 JS 的 `relativeFile` 逐条同形：家外 / 空 / 非可打印 ASCII ⇒ `None`。
pub fn relative_file(home_dir: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(home_dir).ok()?;
    let text = rel.to_string_lossy().replace('\\', "/");
    if text.is_empty() || text.starts_with('/') {
        return None;
    }
    if !text.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) {
        return None;
    }
    Some(text)
}

/// 一条记录（**纯函数**：喂摘要就够 ⇒ 可机械判错）。
pub fn make_record(
    at_ms: i64,
    file: &str,
    size: i64,
    sha256: Option<String>,
    prev: Option<String>,
) -> Result<Record, String> {
    if at_ms < 0 {
        return Err("model-witness: at-ms 要是非负整数".to_string());
    }
    if file.is_empty() || !file.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) {
        return Err("model-witness: file 要是可打印 ASCII 的相对路径".to_string());
    }
    if size < 0 {
        return Err("model-witness: size 要是非负整数".to_string());
    }
    if let Some(digest) = &sha256 {
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err("model-witness: sha256 要是 64 位小写十六进制".to_string());
        }
    }
    if let Some(link) = &prev {
        if link.len() != 64
            || !link
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err("model-witness: prev 要是 64 位小写十六进制".to_string());
        }
    }
    Ok(Record {
        at_ms,
        file: file.to_string(),
        size,
        sha256,
        prev,
    })
}

/// 一行（JSON ＋ 换行）。键序与 JS 的 `JSON.stringify` **逐字节相同**（同一个对象、同一个顺序）。
pub fn encode(record: &Record) -> String {
    let file = serde_json::Value::String(record.file.clone()).to_string();
    let sha = match &record.sha256 {
        Some(value) => serde_json::Value::String(value.clone()).to_string(),
        None => "null".to_string(),
    };
    let prev = match &record.prev {
        Some(value) => serde_json::Value::String(value.clone()).to_string(),
        None => "null".to_string(),
    };
    format!(
        "{{\"at-ms\":{},\"file\":{},\"size\":{},\"sha256\":{},\"prev\":{}}}\n",
        record.at_ms, file, record.size, sha, prev
    )
}

/// 链值算在**这条规范行**上（不是 JSON 序列化）—— 形状逐字钉死，两个写者各自实现它。
pub fn canonical_line(record: &Record) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n",
        record.at_ms,
        record.file,
        record.size,
        record.sha256.clone().unwrap_or_default(),
        record.prev.clone().unwrap_or_default()
    )
}

/// 一条记录的摘要 —— 下一条的 `prev` 就是它。
pub fn digest_record(record: &Record) -> String {
    hex_of(&Sha256::digest(canonical_line(record).as_bytes()))
}

/// 解码一行；形状认不得 ⇒ **明确拒绝**（不猜、不降级）。
pub fn decode(line: &str) -> Result<Record, String> {
    let parsed: serde_json::Value = serde_json::from_str(line)
        .map_err(|error| format!("model-witness: 这一行不是 JSON（{error}）"))?;
    let Some(object) = parsed.as_object() else {
        return Err("model-witness: 这一行不是对象".to_string());
    };
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    if keys != RECORD_KEYS {
        return Err(format!("model-witness: 认不得的键（{}）", keys.join(",")));
    }
    let at_ms = object["at-ms"]
        .as_i64()
        .ok_or_else(|| "model-witness: at-ms 要是非负整数".to_string())?;
    let file = object["file"]
        .as_str()
        .ok_or_else(|| "model-witness: file 要是字符串".to_string())?;
    let size = object["size"]
        .as_i64()
        .ok_or_else(|| "model-witness: size 要是非负整数".to_string())?;
    let sha256 = match &object["sha256"] {
        serde_json::Value::Null => None,
        serde_json::Value::String(text) => Some(text.clone()),
        _ => return Err("model-witness: sha256 要么是 64 位十六进制，要么是 null".to_string()),
    };
    let prev = match &object["prev"] {
        serde_json::Value::Null => None,
        serde_json::Value::String(text) => Some(text.clone()),
        _ => return Err("model-witness: prev 要么是 null，要么是 64 位十六进制".to_string()),
    };
    make_record(at_ms, file, size, sha256, prev)
}

/// 读回全部记录。文件不存在 ⇒ `Ok(vec![])`（**第一次**，不是"坏了"）。
///
/// ⚠ **一行读不出来 ⇒ `Err`**：账是**证据**，一本读不全的账不能拿来回答"这一份有没有被人动过"。
pub fn read_records(home_dir: &Path) -> Result<Vec<Record>, String> {
    let path = witness_path(home_dir);
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for line in text.split('\n') {
        if line.is_empty() {
            continue;
        }
        out.push(decode(line)?);
    }
    Ok(out)
}

/// 打开这本账（**只读**，一次读全）。
#[derive(Debug, Clone, Default)]
pub struct Ledger {
    pub records: Vec<Record>,
    /// 链的第一个断点（`None` = 完整）。
    pub chain_break_at: Option<usize>,
    /// `哪一份 ⇒ 最后一条记录`（**按份看**，不是按整本看）。
    pub last: HashMap<String, Record>,
    /// 账读不出来的原因（`None` = 读得出来）。**它与"账是空的"不是一回事**。
    pub broken: Option<String>,
}

/// 打开这本账（**只读**）。
pub fn open(home_dir: &Path) -> Ledger {
    let records = match read_records(home_dir) {
        Ok(records) => records,
        Err(reason) => {
            return Ledger {
                broken: Some(reason),
                ..Ledger::default()
            }
        }
    };
    let mut chain_break_at = None;
    for (index, record) in records.iter().enumerate() {
        let expect = if index == 0 {
            None
        } else {
            Some(digest_record(&records[index - 1]))
        };
        if record.prev != expect {
            chain_break_at = Some(index);
            break;
        }
    }
    let mut last = HashMap::new();
    for record in &records {
        last.insert(record.file.clone(), record.clone());
    }
    Ledger {
        records,
        chain_break_at,
        last,
        broken: None,
    }
}

/// 一个文件的 `(size, sha256)`；读不出来 ⇒ `None`（**判不了 ≠ 没变**）。
pub fn digest_file(file: &Path) -> Option<(i64, String)> {
    let bytes = fs::read(file).ok()?;
    Some((bytes.len() as i64, hex_of(&Sha256::digest(&bytes))))
}

/// 这一份此刻的读数（四态；`detail` 是更细的那一格）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub state: &'static str,
    pub at_ms: Option<i64>,
    pub detail: Option<&'static str>,
    pub delta_bytes: Option<i64>,
}

impl Reading {
    fn none() -> Self {
        Reading {
            state: "no-witness",
            at_ms: None,
            detail: None,
            delta_bytes: None,
        }
    }
    /// 上载荷的那一份（键名与 JS 门那一侧**逐字相同**）。
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "state": self.state,
            "atMs": self.at_ms,
            "detail": self.detail,
            "deltaBytes": self.delta_bytes,
        })
    }
}

/// 拿"这一份最后一次被门写过那一刻"与"此刻"比（判据与 JS 的 `readingFor` **逐条同形**）。
pub fn reading_for(ledger: &Ledger, home_dir: &Path, file: &Path) -> Reading {
    if ledger.broken.is_some() {
        return Reading {
            state: "unreadable",
            at_ms: None,
            detail: Some("ledger"),
            delta_bytes: None,
        };
    }
    // 说不成"哪一份"⇒ 查不到它的见证 ⇒ **判不了**（不是"没改过"）。
    let Some(rel) = relative_file(home_dir, file) else {
        return Reading::none();
    };
    let last = ledger.last.get(&rel);
    let exists = file.exists();
    let current = if exists { digest_file(file) } else { None };
    let Some((size, sha256)) = current else {
        if !exists {
            let Some(last) = last else {
                return Reading::none();
            };
            if last.sha256.is_none() {
                return Reading {
                    state: "same",
                    at_ms: Some(last.at_ms),
                    detail: None,
                    delta_bytes: None,
                };
            }
            return Reading {
                state: "changed",
                at_ms: Some(last.at_ms),
                detail: Some("missing"),
                delta_bytes: None,
            };
        }
        return Reading {
            state: "unreadable",
            at_ms: last.map(|record| record.at_ms),
            detail: Some("file"),
            delta_bytes: None,
        };
    };
    let Some(last) = last else {
        return Reading::none();
    };
    if last.sha256.is_none() {
        // 见证记着"门摘掉过它"，而它现在在 ⇒ 有人放回来了。
        return Reading {
            state: "changed",
            at_ms: Some(last.at_ms),
            detail: Some("appeared"),
            delta_bytes: None,
        };
    }
    if last.sha256.as_deref() != Some(sha256.as_str()) {
        return Reading {
            state: "changed",
            at_ms: Some(last.at_ms),
            detail: Some("content"),
            delta_bytes: Some(size - last.size),
        };
    }
    Reading {
        state: "same",
        at_ms: Some(last.at_ms),
        detail: None,
        delta_bytes: None,
    }
}

/// 追加一条（调用方已经过形状判断）。**只追加**（`append` 模式：绝不改写已有的行）。
fn append(home_dir: &Path, record: Record) -> Result<Record, String> {
    use std::io::Write;
    let path = witness_path(home_dir);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|error| format!("建不了见证目录 {dir:?}（{error}）"))?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("打不开见证账本 {path:?}（{error}）"))?;
    file.write_all(encode(&record).as_bytes())
        .map_err(|error| format!("写不进见证账本 {path:?}（{error}）"))?;
    Ok(record)
}

/// 记一条"**门写过这一份**"（那一刻它在，摘要就是它此刻的字节）。**绝不 panic**。
pub fn record_write(home_dir: &Path, file: &Path, at_ms: i64) -> Result<Record, String> {
    let Some(rel) = relative_file(home_dir, file) else {
        return Err("unnamed".to_string());
    };
    let Some((size, sha256)) = digest_file(file) else {
        return Err("unreadable".to_string());
    };
    let records = read_records(home_dir)?;
    let prev = records.last().map(digest_record);
    let record = make_record(at_ms, &rel, size, Some(sha256), prev)?;
    append(home_dir, record)
}

/// 记一条"**门摘掉了这一份**"（那一刻它不在 ⇒ `sha256: null`）。
///
/// 为什么"清"也算一笔写：它决定主体下一拍还醒不醒得过来。不记它，页面上的"解除指定"
/// 就会在读数里变成"别处有人动过它" —— 把第一档报成第二档。
pub fn record_clear(home_dir: &Path, file: &Path, at_ms: i64) -> Result<Record, String> {
    let Some(rel) = relative_file(home_dir, file) else {
        return Err("unnamed".to_string());
    };
    let records = read_records(home_dir)?;
    let prev = records.last().map(digest_record);
    let record = make_record(at_ms, &rel, 0, None, prev)?;
    append(home_dir, record)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("celira-model-witness-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 一个**多字节**字符（非可打印 ASCII），**写成转义**——两条用例要拿它试"路径必须可打印
    /// ASCII"那条判据，而源码里不必出现那个字（声音纪律）。
    const NON_ASCII: char = '\u{5979}';

    /// ★ **跨语言向量**：与 JS 那份 `tests/model-witness.test.js` 的 `VECTOR` **钉同一个值**。
    /// 改任何一边的规范行，两处里必有一处红。
    #[test]
    fn the_chain_digest_is_the_same_vector_as_the_js_writer() {
        let first = make_record(
            1760000000000,
            "model_sources.json",
            412,
            Some("a".repeat(64)),
            None,
        )
        .unwrap();
        assert_eq!(
            canonical_line(&first),
            format!(
                "1760000000000\nmodel_sources.json\n412\n{}\n\n",
                "a".repeat(64)
            )
        );
        assert_eq!(
            digest_record(&first),
            "28d6c89ea3d3f0ddfafc178d675b1de2d453f4255cc6ee37f7f2d1ca0278208f"
        );
        let second = make_record(
            1760000000001,
            "agents/a-1/model_binding.json",
            0,
            None,
            Some(digest_record(&first)),
        )
        .unwrap();
        assert_eq!(
            digest_record(&second),
            "9b48432c475ee76105f35dcb4ca436c1456a71c4edbbe9b72ddeba70fd66e496"
        );
        // 落盘那一行与 JS 的 `JSON.stringify` **逐字节相同**（同一个键序）。
        assert_eq!(
            encode(&first),
            format!(
                "{{\"at-ms\":1760000000000,\"file\":\"model_sources.json\",\"size\":412,\"sha256\":\"{}\",\"prev\":null}}\n",
                "a".repeat(64)
            )
        );
        assert_eq!(decode(encode(&first).trim_end()).unwrap(), first);
    }

    #[test]
    fn unknown_shapes_are_refused_instead_of_guessed() {
        assert!(decode(
            "{\"at-ms\":1,\"file\":\"x\",\"size\":0,\"sha256\":null,\"prev\":null,\"extra\":1}"
        )
        .is_err());
        assert!(decode("{\"at-ms\":1,\"file\":\"x\",\"size\":0,\"sha256\":null}").is_err());
        assert!(decode("不是 JSON").is_err());
        assert!(decode("[1,2]").is_err());
        // ⚠ **这里必须用一个多字节字符**：这一格测的是"路径要是**可打印 ASCII**"，
        // 换成任何 ASCII 名字（`not-an-id.json`）都走不到那条判据上（实测吃过一次：
        // 守卫那一刀换掉了它，用例当场变绿——**判据被换没了**）。
        // 字符写成转义，源码里因此不出现它（声音纪律：`no_person_pronouns_in_shell_sources`）。
        assert!(make_record(
            1,
            &format!("agents/{NON_ASCII}/model_binding.json"),
            0,
            None,
            None
        )
        .is_err());
        assert!(make_record(1, "x", 0, Some("xyz".to_string()), None).is_err());
        assert!(make_record(1, "x", 0, None, Some("A".repeat(64))).is_err());
        assert!(make_record(-1, "x", 0, None, None).is_err());
    }

    #[test]
    fn a_path_outside_the_home_cannot_be_named() {
        let home = temp_home("naming");
        let inside = home.join("agents").join("a-1").join("model_binding.json");
        assert_eq!(
            relative_file(&home, &inside).as_deref(),
            Some("agents/a-1/model_binding.json")
        );
        assert_eq!(
            relative_file(&home, &home.join("model_sources.json")).as_deref(),
            Some("model_sources.json")
        );
        let outside = home.parent().unwrap().join("elsewhere.json");
        assert_eq!(relative_file(&home, &outside), None);
        // 住户目录下**只有那两份**认得出；别的一律 `None`。这里取一个**多字节**名字——
        // 判据是"相对路径要可打印 ASCII"（与上一条同一条，见那里的注）。
        assert_eq!(
            relative_file(
                &home,
                &home.join("agents").join(format!("{NON_ASCII}.json"))
            ),
            None
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn the_four_readings_keep_cannot_tell_apart_from_unchanged() {
        let home = temp_home("readings");
        let list = home.join("model_sources.json");
        fs::write(&list, "[]\n").unwrap();
        // ① 从没被门碰过 ⇒ no-witness（**不是**"没改过"）
        assert_eq!(reading_for(&open(&home), &home, &list).state, "no-witness");
        // ② 门写过 ⇒ same，且**有时刻**
        record_write(&home, &list, 1000).unwrap();
        let same = reading_for(&open(&home), &home, &list);
        assert_eq!((same.state, same.at_ms), ("same", Some(1000)));
        // ③ 门外的手改的（主体走 terminal）⇒ changed/content，时刻仍是"门最后写于何时"
        fs::write(&list, "[] \n").unwrap();
        let changed = reading_for(&open(&home), &home, &list);
        assert_eq!(
            (changed.state, changed.detail, changed.delta_bytes),
            ("changed", Some("content"), Some(1))
        );
        // ④ 账本里有一行读不出来 ⇒ unreadable/ledger（整本判不了）
        fs::write(witness_path(&home), "{\"半行\"\n").unwrap();
        let broken = open(&home);
        assert!(broken.broken.is_some());
        assert_eq!(reading_for(&broken, &home, &list).detail, Some("ledger"));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn clearing_is_a_write_of_its_own_and_the_chain_links_every_record() {
        let home = temp_home("clearing");
        let binding = home.join("agents").join("a-1").join("model_binding.json");
        fs::create_dir_all(binding.parent().unwrap()).unwrap();
        fs::write(&binding, "{\n  \"source\": \"ms-1\"\n}\n").unwrap();
        record_write(&home, &binding, 1000).unwrap();
        fs::remove_file(&binding).unwrap();
        let missing = reading_for(&open(&home), &home, &binding);
        assert_eq!(
            (missing.state, missing.detail),
            ("changed", Some("missing"))
        );
        // 门自己摘的 ⇒ 记一条"清"：那一刻不在、此刻也不在 ⇒ **一致**
        record_clear(&home, &binding, 2000).unwrap();
        let cleared = reading_for(&open(&home), &home, &binding);
        assert_eq!((cleared.state, cleared.at_ms), ("same", Some(2000)));
        // 有人放回来了
        fs::write(&binding, "{\n  \"source\": \"ms-1\"\n}\n").unwrap();
        let appeared = reading_for(&open(&home), &home, &binding);
        assert_eq!(
            (appeared.state, appeared.detail),
            ("changed", Some("appeared"))
        );
        // 链：删掉中间一条 ⇒ 报出断点
        record_write(&home, &binding, 3000).unwrap();
        let ledger = open(&home);
        assert_eq!(ledger.chain_break_at, None);
        let records = read_records(&home).unwrap();
        fs::write(
            witness_path(&home),
            format!("{}{}", encode(&records[0]), encode(&records[2])),
        )
        .unwrap();
        assert_eq!(open(&home).chain_break_at, Some(1));
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn recording_never_breaks_the_write_it_witnesses() {
        let home = temp_home("never-throws");
        let outside = home
            .parent()
            .unwrap()
            .join(format!("outside-{}.json", std::process::id()));
        fs::write(&outside, "{}").unwrap();
        assert_eq!(record_write(&home, &outside, 1000).unwrap_err(), "unnamed");
        assert!(!witness_path(&home).exists(), "一个字节都没写");
        let _ = fs::remove_file(&outside);
        // 这一份在、但读不动（目录）⇒ 也回一个 reason，不 panic
        let as_dir = home.join("model_sources.json");
        fs::create_dir_all(&as_dir).unwrap();
        assert_eq!(
            record_write(&home, &as_dir, 1000).unwrap_err(),
            "unreadable"
        );
        let _ = fs::remove_dir_all(&home);
    }
}
