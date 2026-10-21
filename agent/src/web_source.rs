//! **web 源的清单 + 绑定**（2026-10-20）：这台机器上有哪些外网凭据，与**这一支**用哪一条。
//!
//! 形状照模型那两份（[`crate::model_source`]），"钥匙串归房子、用哪把钥匙归住户"也照抄。
//! 与它不同的是**条目里有两个能力**——一条源＝**一份凭据 ＋ 查（search）＋ 取（fetch）**：
//!
//! | 事实 | 归属 | 文件 |
//! |---|---|---|
//! | 这台机器上有哪些外网凭据（**清单**） | **机器** | `<家>/web_sources.json` |
//! | 这一支用哪一条（**绑定**） | **Agent** | `<环境根>/web_binding.json` |
//!
//! ```json
//! [{ "id": "ss-1", "name": "深度求索", "api_key": "sk-…",
//!    "search": { "wire": "search-api", "endpoint": "…/v1/tools/search" },
//!    "fetch":  { "wire": "fetch-doc",  "endpoint": "…/v1/tools/fetch" } }]
//! ```
//!
//! `wire` 是**方言**（对面回执的形状），**不是厂商名**——厂商名只在写侧
//! （`scripts/home/web-sources.js`）与入口的凭据变量名里，本模块一个都不认识。
//!
//! 四条纪律：
//!
//! 1. **一个权威**：条目的形状归写侧，绑定与解析归本模块；本模块是**唯一读侧**，**不缓存**。
//! 2. **fail-closed**：读不出来、形状不对、缺哪个能力、绑到一个不存在的条目 ⇒ **如实拒绝并点名**
//!    （哪个文件、第几条、哪个键），不猜、不回落、不静默另选一条。
//! 3. **两个能力都必须在**：一条源只声明了一半（比如只有 search）是**形状不对**，不是"取不可用"——
//!    今天接的两家都能查能取，留一个半条就是给"某个 App 悄悄不在场"开门。
//! 4. **凭据不出门**：本模块不打印它、不回执、`Entry` 不派生 `Debug`（只 `Clone`）。

use std::path::Path;

use crate::web_app::{FetchAccess, FetchWire, SearchAccess, SearchWire};

/// 清单文件名（机器级，住在家的根下）。
pub const LIST_FILE: &str = "web_sources.json";

/// 绑定文件名（Agent 级，住在环境根下）。
pub const BINDING_FILE: &str = "web_binding.json";

const KEY_ID: &str = "id";
const KEY_NAME: &str = "name";
const KEY_CREDENTIAL: &str = "api_key";
const KEY_SEARCH: &str = "search";
const KEY_FETCH: &str = "fetch";
const KEY_WIRE: &str = "wire";
const KEY_ENDPOINT: &str = "endpoint";

/// 清单里的一条 web 源：**一份凭据 ＋ 两个能力**。
///
/// ⚠ `credential` 在这里**只是一个值**：本模块不打印它、不落盘、不给它任何输出通道；
/// 唯一读它的是那两个 `*_access`（把值交给 web 窗，只进请求头）。
#[derive(Clone)]
pub struct Entry {
    /// 稳定短标识（写侧分配，绑定靠它指过来）。
    pub id: String,
    /// 人起的名字（读数用；缺省 = id）。
    pub name: String,
    credential: String,
    /// 「查」那个能力的方言与端点。
    search: (SearchWire, String),
    /// 「取」那个能力的方言与端点。
    fetch: (FetchWire, String),
}

impl Entry {
    /// 交给 web 窗的**搜索**声明；任何一格不成立 ⇒ `None`（调用方 fail-closed）。
    pub fn search_access(&self) -> Option<SearchAccess> {
        SearchAccess::with_wire(
            self.search.0,
            self.search.1.clone(),
            self.credential.clone(),
        )
    }

    /// 交给 web 窗的**抓取**声明；任何一格不成立 ⇒ `None`。
    pub fn fetch_access(&self) -> Option<FetchAccess> {
        FetchAccess::new(self.fetch.0, self.fetch.1.clone(), self.credential.clone())
    }
}

/// 读一次：`Ok(None)` = **这一支没有绑定**（没配）；`Ok(Some(..))` = 清单里那一条；
/// `Err` = **有绑定但读不出来**——这条不许被当成"没配"吞掉。
pub fn read(list: &Path, env_root: &Path) -> Result<Option<Entry>, String> {
    let binding_path = env_root.join(BINDING_FILE);
    let Some(text) = read_if_present(&binding_path)? else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("{}：读不成 JSON（{error}）", binding_path.display()))?;
    let id = text_field(&value, "source").ok_or_else(|| {
        format!(
            "{}：缺 `source`（这一支用清单里的哪一条）",
            binding_path.display()
        )
    })?;
    let entries = read_list(list)?;
    let known: Vec<String> = entries.iter().map(|entry| entry.id.clone()).collect();
    let entry = entries
        .into_iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| {
            format!(
                "{} 里没有绑定点名的 `{id}`（清单里的是：{}）",
                list.display(),
                if known.is_empty() {
                    "一条都没有".to_string()
                } else {
                    known.join(" / ")
                }
            )
        })?;
    if entry.search_access().is_none() || entry.fetch_access().is_none() {
        return Err(format!(
            "{} 里 `{}` 这一条不成立（端点或凭据不成立）",
            list.display(),
            entry.id
        ));
    }
    Ok(Some(entry))
}

/// 读清单并逐条校验；任何一条不成立就整份拒绝（**不跳过坏条目**：静默少一条等于换了一家）。
fn read_list(list: &Path) -> Result<Vec<Entry>, String> {
    let text = std::fs::read_to_string(list)
        .map_err(|error| format!("{}：读不出来（{error}）", list.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("{}：读不成 JSON（{error}）", list.display()))?;
    let rows = value
        .as_array()
        .ok_or_else(|| format!("{}：清单应当是一个数组", list.display()))?;
    rows.iter()
        .enumerate()
        .map(|(index, row)| parse_entry(row, list, index))
        .collect()
}

/// 一条条目的形状校验（错误文本点名"第几条、哪个键"——操作者照着改）。
fn parse_entry(row: &serde_json::Value, list: &Path, index: usize) -> Result<Entry, String> {
    let at = format!("{} 的第 {} 条", list.display(), index + 1);
    let id = text_field(row, KEY_ID).ok_or_else(|| format!("{at}：缺 `{KEY_ID}`"))?;
    let name = text_field(row, KEY_NAME).unwrap_or_else(|| id.clone());
    let credential = text_field(row, KEY_CREDENTIAL)
        .ok_or_else(|| format!("{at}（{id}）：缺 `{KEY_CREDENTIAL}`"))?;

    let search = capability(row, list, &id, KEY_SEARCH)?;
    let (search_wire_text, search_endpoint) = (search.0, search.1);
    let search_wire = SearchWire::parse(&search_wire_text).ok_or_else(|| {
        format!(
            "{at}（{id}）：`{KEY_SEARCH}` 里认不得的 `{KEY_WIRE}` `{search_wire_text}`（只有 `{}` / `{}`）",
            SearchWire::ResultsJson.as_str(),
            SearchWire::SearchApi.as_str()
        )
    })?;

    let fetch = capability(row, list, &id, KEY_FETCH)?;
    let (fetch_wire_text, fetch_endpoint) = (fetch.0, fetch.1);
    let fetch_wire = FetchWire::parse(&fetch_wire_text).ok_or_else(|| {
        format!(
            "{at}（{id}）：`{KEY_FETCH}` 里认不得的 `{KEY_WIRE}` `{fetch_wire_text}`（只有 `{}` / `{}`）",
            FetchWire::Doc.as_str(),
            FetchWire::Raw.as_str()
        )
    })?;

    Ok(Entry {
        id,
        name,
        credential,
        search: (search_wire, search_endpoint),
        fetch: (fetch_wire, fetch_endpoint),
    })
}

/// 取一条能力（`search` / `fetch` 两个对象同形）：`{ wire, endpoint }`。
fn capability(
    row: &serde_json::Value,
    list: &Path,
    id: &str,
    key: &str,
) -> Result<(String, String), String> {
    let at = format!("{} 里 `{id}` 的 `{key}`", list.display());
    let Some(object) = row.get(key) else {
        return Err(format!(
            "{at}：缺这一格（一条 web 源要两个能力都在：查与取）"
        ));
    };
    let wire = text_field(object, KEY_WIRE).ok_or_else(|| format!("{at}：缺 `{KEY_WIRE}`"))?;
    let endpoint = text_field(object, KEY_ENDPOINT)
        .ok_or_else(|| format!("{at}：缺 `{KEY_ENDPOINT}`（完整地址）"))?;
    Ok((wire, endpoint))
}

/// 读一份**可缺**的文件：不在 ⇒ `Ok(None)`；在但读不动 ⇒ `Err`（"读不到"不是"没有"）。
fn read_if_present(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}：读不出来（{error}）", path.display())),
    }
}

/// 取一个非空文本字段（去首尾空白；缺、空、不是字符串都算"没有"）。
fn text_field(value: &serde_json::Value, name: &str) -> Option<String> {
    value
        .get(name)
        .and_then(|field| field.as_str())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// 清单文件的路径（`<家>/web_sources.json`，与 `model_sources.json` 同级）。
pub fn list_path(home_dir: &Path) -> std::path::PathBuf {
    home_dir.join(LIST_FILE)
}

/// 装配面要的那一个结论（两个入口共用同一份判据）。
pub enum Resolved {
    /// 清单 + 绑定解析出的一条（**两个能力都在**）。
    Bound(Entry),
    /// **这一支没有绑定** ⇒ 两个动词都如实说没配。
    Unbound,
    /// 有绑定但读不出来 ⇒ 调用方**如实报一行**，两个动词都说没配（**不许回落**）。
    Broken(String),
}

/// 装配期的判据：先看绑定，再按绑定去清单里取那一条。
pub fn resolve(list: &Path, env_root: &Path) -> Resolved {
    match read(list, env_root) {
        Ok(Some(entry)) => Resolved::Bound(entry),
        Ok(None) => Resolved::Unbound,
        Err(message) => Resolved::Broken(message),
    }
}
