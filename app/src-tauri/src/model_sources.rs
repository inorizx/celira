//! **模型源**——「这台机器上有哪些钥匙」那份**清单**（机器级）。
//!
//! ## 形状权威是 `scripts/home/model-sources.js`（照抄，不发明）
//!
//! | 文件 | 归属 | 形状 |
//! |---|---|---|
//! | `<家>/model_sources.json` | **机器** | `[{ id, name, endpoint, api_key, model, context_window, protocol? }]` |
//! | `<环境根>/model_binding.json` | **Agent** | `{ "source": "<清单条目 id>" }` |
//!
//! 一句话原理（JS 原文）：**钥匙串归房子，用哪把钥匙归住户**。
//!
//! ## 这一片只管**读**（写路径与探测是下一片）
//!
//! - **绑定那一格不在这里读**：它的读取权威已经是 [`crate::home_facts::read_model_binding`]
//!   （模型计划那条路在用）—— 两处各读一份就是两份权威，这里只**用**它；
//! - **`api_key` 只进内存、永不出门**：这一片把它读出来给写路径用（下一片），
//!   而载荷里一律只回 `keyConfigured` 布尔（照 JS：`api_key` 一个字节都不在载荷里）；
//! - **三态不许压成两态**：清单读不出来（`not_json` / `no_valid_entry`）与"清单是空的"
//!   是两件不同的事（前者要用户去修文件，后者只是还没配）。

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::home_facts;

/// 清单文件名（`model-sources.js` 的 `LIST_FILE`）。
pub const LIST_FILE: &str = "model_sources.json";

/// 绑定文件名（`model-sources.js` 的 `BINDING_FILE`）—— 它落在**环境根**下，不在家根下。
pub const BINDING_FILE: &str = "model_binding.json";

/// **型号那一格的字节上界**（`model-sources.js` 的 `MODEL_MAX_BYTES`，同一个数）。
pub const MODEL_MAX_BYTES: usize = 200;

/// **思考强度的封闭表**（支级覆盖那一格；**权威是运行时的读侧** `agent/src/model_source.rs`）。
///
/// ⚠ 三处各有一份、都删不掉：**运行时**（权威）、**家的门**（`scripts/home/model-sources.js`）、
/// **本文件**（壳那份门的形状层）。谁改一档就三处一起改 —— 对表由
/// `scripts/home/tests/model-sources-binding.test.js` 的一条用例读这三份源文件逐字比对。
pub const REASONING_EFFORTS: [&str; 7] =
    ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// 绑定里那两格"**她说的**"键名（2026-10-09 加宽的形状）。
///
/// 判据：**钥匙属机器，"用什么型号/什么档位"属她** —— 那两格可以写进绑定（支级）来覆盖条目
/// 里的同名字段。键名与 agent 侧 `model_source.rs` 的 `KEY_MODEL` / `KEY_REASONING` 逐字同名
/// （一个概念一个名称）。
const KEY_MODEL: &str = "model";
const KEY_REASONING_EFFORT: &str = "reasoning_effort";

/// 清单文件的路径（`<家>/model_sources.json`，与 `home.conf` 同级）。
pub fn list_path(home_dir: &Path) -> PathBuf {
    home_facts::model_sources_list_path(home_dir)
}

/// 一条清单条目（**含 `api_key`** —— 它只活在内存里，绝不上载荷）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSourceEntry {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub model: String,
    pub context_window: i64,
    pub protocol: Option<String>,
    /// **思考强度**（2026-10-08，可选）：这一条来源请求时带哪个档位（认得的取值由 agent 侧那个
    /// 唯一读侧的封闭表说了算；本层只收形状）。⚠ 它必须**原样带上**：漏了它，页面保存一次就把
    /// 用户填的那一格删掉。
    pub reasoning_effort: Option<String>,
    /// **这一份模型收不收得到媒体材料**（2026-10-17）：宿主声明，**fail-closed**（不写就是不支持）。
    ///
    /// ⚠ 与 `reasoning_effort` **同一个病、同一条药**：写者重建条目时漏了它，就等于把用户开的
    /// 那一格**删掉** —— 而这里删掉的后果是"她静默地重新读不动图片"（**没有任何报错**）。
    pub vision: bool,
    pub api_key: String,
}

/// 读清单的结局（**三态**：读得出来（可能是空表）/ 读不出来 / 文件不在）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListState {
    /// 读出来了（`entries` 可能是空表 —— 那只是"还没配"，不是故障）。
    Loaded { entries: Vec<ModelSourceEntry> },
    /// 读不出来（原因原样；**不静默回落**成"没配"）。
    Unreadable { reason: String },
}

impl ListState {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Loaded { .. } => "loaded",
            Self::Unreadable { .. } => "unreadable",
        }
    }

    pub fn entries(&self) -> &[ModelSourceEntry] {
        match self {
            Self::Loaded { entries } => entries,
            Self::Unreadable { .. } => &[],
        }
    }
}

/// 读清单（照 `model-sources.js` 的 `readList`：文件不在 ⇒ **空表**，畸形 ⇒ `unreadable`）。
///
/// ⚠ 条目**逐条严格**（照 `normalizeEntries`）：`endpoint` / `model` / `context_window` 三格
/// 缺一不可（`continue` 跳过），`id` 必须合 `ms-<序号>`，`api_key` 允许空串（空钥匙是
/// "还没贴"，不是畸形）。**一条都不合** ⇒ `no_valid_entry`（那不是"没配"，是文件坏了）。
pub fn read_list(home_dir: &Path) -> ListState {
    let file = list_path(home_dir);
    let Ok(text) = fs::read_to_string(&file) else {
        // 文件不在 ⇒ 空表（照 JS：`ENOENT` 是"还没配"）。
        return ListState::Loaded {
            entries: Vec::new(),
        };
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&text) else {
        return ListState::Unreadable {
            reason: "not_json".to_string(),
        };
    };
    let Some(items) = parsed.as_array() else {
        return ListState::Unreadable {
            reason: "not_array".to_string(),
        };
    };
    let entries = normalize_entries(items);
    if entries.is_empty() && !items.is_empty() {
        // 有东西、但一条都不合形状 ⇒ 读不出来（**不是**"没配"）。
        return ListState::Unreadable {
            reason: "no_valid_entry".to_string(),
        };
    }
    ListState::Loaded { entries }
}

/// `normalizeEntries` 的直译：逐条严格，不合形状的**跳过**（不猜、不补默认值）。
fn normalize_entries(items: &[Value]) -> Vec<ModelSourceEntry> {
    let mut out = Vec::new();
    for item in items {
        let text_of = |key: &str| {
            item.get(key)
                .and_then(|value| value.as_str())
                .map(str::trim)
                .unwrap_or("")
                .to_string()
        };
        let id = text_of("id");
        if !is_entry_id(&id) {
            continue;
        }
        let endpoint = text_of("endpoint");
        let model = text_of("model");
        // `context_window` 必须是数字（空 / 字符串都不算）。
        let Some(context_window) = item.get("context_window").and_then(|value| value.as_i64())
        else {
            continue;
        };
        if endpoint.is_empty() || model.is_empty() {
            continue;
        }
        let protocol = item
            .get("protocol")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_string());
        out.push(ModelSourceEntry {
            id,
            name: text_of("name"),
            endpoint,
            model,
            context_window,
            // 盘上这一格是 snake_case（与 JS 写者同格式）；本层只收形状、不判语义。
            reasoning_effort: item
                .get("reasoning_effort")
                .and_then(|value| value.as_str())
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty()),
            protocol,
            // **媒体材料**（2026-10-17）：与 JS 写者同一条判据 —— **只有显式 `true` 才算支持**
            // （缺席 / `false` / 非布尔一律落到"不支持"）。
            vision: item.get("vision").and_then(Value::as_bool).unwrap_or(false),
            api_key: text_of("api_key"),
        });
    }
    out
}

/// 条目 id 的形状：`ms-<序号>`（`model-sources.js` 的 `ID_SHAPE`）。
pub fn is_entry_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("ms-") else {
        return false;
    };
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

/// 下一个条目 id（`ms-<最大序号 + 1>`；与 JS 同款：**单调、不回收**——
/// 回收会让绑定指向一条别的条目）。
pub fn next_entry_id(entries: &[ModelSourceEntry]) -> String {
    let max = entries
        .iter()
        .filter_map(|entry| entry.id.strip_prefix("ms-"))
        .filter_map(|rest| rest.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("ms-{}", max + 1)
}

/// 载荷里那一条清单条目的形状（**没有 `api_key`**，只有 `keyConfigured`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryPayload {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub model: String,
    pub context_window: i64,
    pub protocol: Option<String>,
    /// 同上那一格（载荷是 camelCase ⇒ 键名 `reasoningEffort`）。
    pub reasoning_effort: Option<String>,
    /// **这一份模型收不收得到媒体材料**（2026-10-17）：页面据此初始化那一格（`entry.vision ?? false`）。
    /// ⚠ 它**在场就必须是布尔**（与别的格同一条：半读的形状比读不到更坏）。
    pub vision: bool,
    pub key_configured: bool,
}

impl From<&ModelSourceEntry> for EntryPayload {
    fn from(entry: &ModelSourceEntry) -> Self {
        Self {
            id: entry.id.clone(),
            name: entry.name.clone(),
            endpoint: entry.endpoint.clone(),
            model: entry.model.clone(),
            context_window: entry.context_window,
            protocol: entry.protocol.clone(),
            reasoning_effort: entry.reasoning_effort.clone(),
            vision: entry.vision,
            // **门的回答只有这一个布尔**（取值永不回来）。
            key_configured: !entry.api_key.trim().is_empty(),
        }
    }
}

// ---------------------------------------------------------------------------
// 写侧（形状照 `model-sources.js`：原子写、按 id 原地替换、单调不回收）
// ---------------------------------------------------------------------------

/// 一份清单的落盘形状（**键序照 JS**：`id,name,endpoint,api_key,model,context_window[,protocol]`）。
///
/// 为什么连键序都照抄：两份实现写的是**同一个文件**，逐字节同形才不会互相制造无谓 diff
/// （形状本身按键读，顺序不影响正确性 —— 这一条是"少一份噪音"，不是契约）。
fn render_list(entries: &[ModelSourceEntry]) -> String {
    let quote = |value: &str| serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string());
    let mut out = String::from("[");
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str("\n  {\n");
        out.push_str(&format!("    \"id\": {},\n", quote(&entry.id)));
        out.push_str(&format!("    \"name\": {},\n", quote(&entry.name)));
        out.push_str(&format!("    \"endpoint\": {},\n", quote(&entry.endpoint)));
        out.push_str(&format!("    \"api_key\": {},\n", quote(&entry.api_key)));
        out.push_str(&format!("    \"model\": {},\n", quote(&entry.model)));
        out.push_str(&format!("    \"context_window\": {}", entry.context_window));
        if let Some(protocol) = &entry.protocol {
            out.push_str(&format!(",\n    \"protocol\": {}", quote(protocol)));
        }
        // 思考强度：可选那一格（键序照 JS：排在 `protocol` 之后）。
        if let Some(reasoning_effort) = &entry.reasoning_effort {
            out.push_str(&format!(
                ",\n    \"reasoning_effort\": {}",
                quote(reasoning_effort)
            ));
        }
        // **媒体材料：总是写**（照 `writeList`：这一格落盘时是**显式布尔**；缺了它，等于把用户
        // 开着的那一格删掉 —— 而那是"她静默地重新读不动图片"，没有任何报错）。
        out.push_str(&format!(",\n    \"vision\": {}", entry.vision));
        out.push_str("\n  }");
    }
    out.push_str("\n]\n");
    out
}

/// 原子写（照 `writeAtomic`：先写同目录临时文件再改名 —— 半截文件永不成为读数）。
fn write_atomic(file: &Path, text: &str) -> Result<(), String> {
    let dir = file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let name = file
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "model_sources.json".to_string());
    let temp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let write = || -> std::io::Result<()> {
        fs::create_dir_all(&dir)?;
        fs::write(&temp, text)?;
        fs::rename(&temp, file)
    };
    write().map_err(|error| {
        let _ = fs::remove_file(&temp);
        format!("写不进 {file:?}（{error}）")
    })
}

/// 写整份清单（**先按形状归一**：不合形状的条目不落盘，照 `writeList`）。
pub fn write_list(home_dir: &Path, entries: &[ModelSourceEntry]) -> Result<(), String> {
    let as_values: Vec<Value> = entries.iter().map(entry_as_value).collect();
    let normalized: Vec<ModelSourceEntry> = normalize_entries(&as_values);
    write_atomic(&list_path(home_dir), &render_list(&normalized))
}

/// 一条条目 → JSON（落盘与归一都从它走，免得两处各长一份形状判断）。
fn entry_as_value(entry: &ModelSourceEntry) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("id".to_string(), Value::String(entry.id.clone()));
    object.insert("name".to_string(), Value::String(entry.name.clone()));
    object.insert(
        "endpoint".to_string(),
        Value::String(entry.endpoint.clone()),
    );
    object.insert("api_key".to_string(), Value::String(entry.api_key.clone()));
    object.insert("model".to_string(), Value::String(entry.model.clone()));
    object.insert(
        "context_window".to_string(),
        Value::from(entry.context_window),
    );
    if let Some(protocol) = &entry.protocol {
        object.insert("protocol".to_string(), Value::String(protocol.clone()));
    }
    if let Some(reasoning_effort) = &entry.reasoning_effort {
        object.insert(
            "reasoning_effort".to_string(),
            Value::String(reasoning_effort.clone()),
        );
    }
    // 键序照 JS 的 `writeList`（`reasoning_effort` 之后、`protocol` 之前）。
    object.insert("vision".to_string(), Value::Bool(entry.vision));
    Value::Object(object)
}

/// 放一条进清单：**按 id 原地替换，新的追加在末尾**（照 `upsertEntry`；不重排、不排序）。
///
/// `vision` 那一格是**三态**（照 `upsertEntry` 的保险）：`None` = 来件**没说**（老页面）⇒
/// 从**旧条目原样带过来**；`Some(_)` = 说了（哪怕 `false`）⇒ 以它为准。
/// ⚠ 写成"缺省 `false`"就毁了这条：老页面保存一次就会把用户开着的视觉**静默关掉**。
pub fn upsert_entry(
    home_dir: &Path,
    entry: &ModelSourceEntry,
    vision: Option<bool>,
) -> Result<(), String> {
    let ListState::Loaded { entries } = read_list(home_dir) else {
        return Err("unreadable".to_string());
    };
    // 来件没说 ⇒ 从**旧条目**原样带过来（判据与 JS 那一侧同一句，位置也同一处：写侧）。
    let carried = vision.unwrap_or_else(|| {
        entries
            .iter()
            .find(|each| each.id == entry.id)
            .map(|each| each.vision)
            .unwrap_or(false)
    });
    let mut value = entry_as_value(entry);
    if let Some(object) = value.as_object_mut() {
        object.insert("vision".to_string(), Value::Bool(carried));
    }
    let Some(normalized) = normalize_entries(&[value]).into_iter().next() else {
        return Err("invalid_entry".to_string());
    };
    let mut next: Vec<ModelSourceEntry> = entries
        .iter()
        .map(|each| {
            if each.id == normalized.id {
                normalized.clone()
            } else {
                each.clone()
            }
        })
        .collect();
    if !next.iter().any(|each| each.id == normalized.id) {
        next.push(normalized);
    }
    write_list(home_dir, &next)
}

/// 摘掉一条的结局（照 `removeEntry` 的三个原因）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveOutcome {
    Removed,
    NotFound,
    /// 这一支绑着它 ⇒ **不许删**（先换绑）。
    Bound {
        bound_to: String,
    },
    /// 清单读不出来 / 写不进去。
    Failed {
        reason: String,
    },
}

/// 摘掉一条（照 `removeEntry`：不在场 ⇒ `NotFound`；被绑定挡住 ⇒ `Bound`，绝不悄悄删）。
pub fn remove_entry(home_dir: &Path, id: &str, bound_to: Option<&str>) -> RemoveOutcome {
    let ListState::Loaded { entries } = read_list(home_dir) else {
        return RemoveOutcome::Failed {
            reason: "unreadable".to_string(),
        };
    };
    if !entries.iter().any(|entry| entry.id == id) {
        return RemoveOutcome::NotFound;
    }
    if let Some(bound_to) = bound_to {
        return RemoveOutcome::Bound {
            bound_to: bound_to.to_string(),
        };
    }
    let next: Vec<ModelSourceEntry> = entries.into_iter().filter(|entry| entry.id != id).collect();
    match write_list(home_dir, &next) {
        Ok(()) => RemoveOutcome::Removed,
        Err(_) => RemoveOutcome::Failed {
            reason: "save_failed".to_string(),
        },
    }
}

/// 绑定这一支用清单里的哪一条（`{ "source": "<id>" }`，原子写）。
///
/// ⚠ **2026-10-09：改写绑定时要把"它说的那两格"原样带上**（`model` / `reasoning_effort`）。
/// 判据：**钥匙属机器，"用什么型号/什么档位"归主体** —— 那两格是**支级**事实，而本函数缺省只
/// 拿到 `source`。漏了这一步，页面上重新绑一次就把那两格**悄悄抹掉**（与 `reasoning_effort`
/// 那次"页面保存抹掉清单那一格"是**同族事故**）。缺那两格时写出来的仍是老形状（一字不变）。
pub fn write_binding(env_root: &Path, source: &str) -> Result<(), String> {
    write_binding_with(env_root, source, &BindingOverrides::default())
}

/// **写绑定时那两格给什么**（三态，一个都不许含糊；`scripts/home/model-sources.js` 的
/// `overrides` **逐条同形**）：
///
/// | 字段 | 写出来的是 |
/// |---|---|
/// | `None` | **沿用旧值**（老行为一字不变：重新绑定不抹掉它） |
/// | `Some(None)` | **清掉这一格**（回到"用条目里的型号/档位"）——写 `null`，与运行时的"没写"同义 |
/// | `Some(Some(v))` | 写成 `v`（形状由调用方先过 [`ModelBinding`](crate::home_facts) 那一侧的同款预检） |
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BindingOverrides {
    pub model: Option<Option<String>>,
    pub reasoning_effort: Option<Option<String>>,
}

/// [`write_binding`] 的加宽版（2026-10-14 支级覆盖那一刀）：那两格可以**显式写**。
///
/// 在此之前运行时一直认它们（`apply_binding_overrides`），而**没有任何写者**能写它们 ——
/// 功能有、出口没有；这一刀把出口补上（页面上的"选具体型号 / 思考档"）。
pub fn write_binding_with(
    env_root: &Path,
    source: &str,
    overrides: &BindingOverrides,
) -> Result<(), String> {
    let file = env_root.join(BINDING_FILE);
    let existing = fs::read_to_string(&file)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let mut object = serde_json::Map::new();
    object.insert("source".to_string(), Value::String(source.to_string()));
    // 一格的三态：不给 ⇒ 沿用旧的（`null`/不写都算"没有"）；给了 ⇒ 照给的写（`None` = 清掉）。
    let mut decide = |key: &str, given: &Option<Option<String>>| {
        let value = match given {
            Some(Some(text)) => Some(Value::String(text.clone())),
            Some(None) => None,
            None => existing
                .as_ref()
                .and_then(|each| each.get(key))
                .filter(|each| !each.is_null())
                .cloned(),
        };
        if let Some(value) = value {
            object.insert(key.to_string(), value);
        }
    };
    decide(KEY_MODEL, &overrides.model);
    decide(KEY_REASONING_EFFORT, &overrides.reasoning_effort);
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&Value::Object(object))
            .map_err(|error| format!("绑定序列化不出来（{error}）"))?
    );
    write_atomic(&file, &text)
}

/// 摘掉绑定（回到"这台机器上没绑模型源"那一路）。**查无也成功**（撤销式）。
pub fn clear_binding(env_root: &Path) -> Result<(), String> {
    let file = env_root.join(BINDING_FILE);
    match fs::remove_file(&file) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("摘不掉 {file:?}（{error}）")),
    }
}

/// ★ **还没住户化、但已经先选好了**的那几支（2026-10-14「选择先于接上」那一刀）。
///
/// 形状与判据的权威是 `scripts/home/model-sources.js` 的 `unhomedBindings`（**逐条同形**）：
/// `<家>/agents/<id>/` 里有 `model_binding.json`，而它**不是住户** —— 住户判据直接复用
/// [`crate::registration::resident_env_roots`] 那一份（**不另写第二把尺子**）。
///
/// 为什么它与绑定同属本模块：绑定的**落点**就是"她的环境根"，而环境根要等接她第 2 步才建
/// ⇒ "先选好"只多出一个**环境根先于住户出现**的形状，绑定本身的形状一字未改。
///
/// ⚠ **它们不是住户**：调用方不许把它们并进"这台机器上的住户"那一格（那是假话）。
/// 返回**按 id 排序**（稳定，与住户表的次序无关）。
pub fn unhomed_bindings(home_dir: &Path) -> Vec<(String, home_facts::ModelBinding)> {
    let live: std::collections::HashSet<PathBuf> =
        crate::registration::resident_env_roots(home_dir)
            .into_iter()
            .collect();
    let mut out: Vec<(String, home_facts::ModelBinding)> = Vec::new();
    let Ok(entries) = fs::read_dir(home_dir.join("agents")) else {
        // 没有 `agents/` 目录 = 一支都没有（含"先选好的"）——**空是一个真答案**，不是读不到。
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || live.contains(&path) {
            continue;
        }
        if !path.join("model_binding.json").is_file() {
            continue;
        }
        out.push((name, home_facts::read_model_binding(&path)));
    }
    out.sort_by(|left, right| left.0.cmp(&right.0));
    out
}

// ---------------------------------------------------------------------------
// 用例（形状；不碰真家）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("celira-ms-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_is_an_empty_list_not_a_failure() {
        let home = temp_home("missing");
        assert_eq!(
            read_list(&home),
            ListState::Loaded {
                entries: Vec::new()
            },
            "还没配 ≠ 读不出来"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn malformed_file_is_unreadable_never_silently_empty() {
        let home = temp_home("malformed");
        let file = list_path(&home);
        fs::write(&file, "not json at all").unwrap();
        assert_eq!(
            read_list(&home),
            ListState::Unreadable {
                reason: "not_json".to_string()
            }
        );
        fs::write(&file, "{\"a\":1}").unwrap();
        assert_eq!(
            read_list(&home),
            ListState::Unreadable {
                reason: "not_array".to_string()
            }
        );
        // 有东西、但一条都不合形状 ⇒ no_valid_entry（**不是**"没配"）。
        fs::write(&file, "[{\"id\":\"nope\"}]").unwrap();
        assert_eq!(
            read_list(&home),
            ListState::Unreadable {
                reason: "no_valid_entry".to_string()
            }
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn entries_are_strict_and_keep_their_key_in_memory_only() {
        let home = temp_home("entries");
        let file = list_path(&home);
        fs::write(
            &file,
            r#"[
              {"id":"ms-1","name":"深度求索","endpoint":"https://api.deepseek.com/chat/completions",
               "api_key":"sk-secret","model":"deepseek-chat","context_window":65536},
              {"id":"ms-2","name":"没窗口","endpoint":"https://x/v1","api_key":"k","model":"m"},
              {"id":"bad","name":"坏 id","endpoint":"https://x/v1","api_key":"k","model":"m","context_window":1},
              {"id":"ms-3","name":"没模型","endpoint":"https://x/v1","api_key":"k","context_window":1},
              {"id":"ms-4","name":"协议那条","endpoint":"https://x/v1","api_key":"","model":"m2",
               "context_window":32768,"protocol":"openai"}
            ]"#,
        )
        .unwrap();
        let ListState::Loaded { entries } = read_list(&home) else {
            panic!("该读得出来");
        };
        // ms-2（缺窗口）/ bad（id 不合法）/ ms-3（缺模型）三条被跳过，**不猜**。
        assert_eq!(
            entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            vec!["ms-1", "ms-4"]
        );
        assert_eq!(entries[0].api_key, "sk-secret");
        assert_eq!(entries[0].context_window, 65536);
        assert_eq!(entries[0].protocol, None, "没写协议就是 None");
        assert_eq!(entries[1].protocol.as_deref(), Some("openai"));
        assert_eq!(entries[1].api_key, "", "空钥匙是「还没贴」，不是畸形");

        // 载荷里**没有 key 的取值**，只有布尔。
        let payload = EntryPayload::from(&entries[0]);
        let text = serde_json::to_string(&payload).unwrap();
        assert!(
            !text.contains("sk-secret"),
            "key 一个字节都不许进载荷：{text}"
        );
        assert!(text.contains("\"keyConfigured\":true"));
        // ⚠ 逐字形状：这一格里 `vision` **在场**（页面据此初始化那一格；缺了它，页面只能当"没说"）。
        assert_eq!(
            serde_json::to_string(&EntryPayload::from(&entries[1])).unwrap(),
            "{\"id\":\"ms-4\",\"name\":\"协议那条\",\"endpoint\":\"https://x/v1\",\"model\":\"m2\",\"contextWindow\":32768,\"protocol\":\"openai\",\"reasoningEffort\":null,\"vision\":false,\"keyConfigured\":false}"
        );
        let _ = fs::remove_dir_all(&home);
    }

    /// ★ **`vision` 那一格：三态 ＋ 落盘是显式布尔**（2026-10-17）。
    ///
    /// 判据三条（每条都能被一个反例打红）：
    /// 1. 盘上开着 ⇒ **来件没说**（老页面）时**原样带过来** —— 否则老页面保存一次就把用户开着的
    ///    视觉**静默关掉**（这是这一格最容易写错的地方，也是 JS 那一侧同一条保险）；
    /// 2. 来件**说了** `false` ⇒ 以它为准（**关得掉**）；
    /// 3. 落盘**总是**带这一格（照 `writeList`）：缺了键 = 把用户开的那一格删掉。
    #[test]
    fn vision_is_three_state_and_lands_as_an_explicit_boolean() {
        let home = temp_home("vision");
        let file = list_path(&home);
        fs::write(
            &file,
            r#"[{"id":"ms-1","name":"n","endpoint":"https://x/v1","api_key":"k","model":"m",
                 "context_window":1024,"vision":true}]"#,
        )
        .unwrap();
        let ListState::Loaded { entries } = read_list(&home) else {
            panic!("该读得出来");
        };
        assert!(entries[0].vision, "前提：盘上那一格开着");
        let entry = entries[0].clone();

        // ① 没说 ⇒ 从旧条目原样带过来。
        upsert_entry(&home, &entry, None).unwrap();
        let ListState::Loaded { entries } = read_list(&home) else {
            panic!("该读得出来");
        };
        assert!(entries[0].vision, "来件没说 ⇒ **不许**静默关掉");

        // ② 说了 `false` ⇒ 以它为准。
        upsert_entry(&home, &entry, Some(false)).unwrap();
        let ListState::Loaded { entries } = read_list(&home) else {
            panic!("该读得出来");
        };
        assert!(!entries[0].vision, "明确说不支持 ⇒ 关得掉");

        // ③ 落盘是显式布尔（`true` 那一档也验一遍：写回去要看得见）。
        upsert_entry(&home, &entry, Some(true)).unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(
            text.contains("\"vision\": true"),
            "落盘必须带这一格（缺了它 = 删事实）：{text}"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn ids_are_monotonic_and_never_reused() {
        assert!(is_entry_id("ms-1"));
        assert!(is_entry_id("ms-42"));
        for bad in ["ms-", "ms-a", "ms-1a", "1", "", "MS-1"] {
            assert!(!is_entry_id(bad), "{bad} 不是合法 id");
        }
        let entries = vec![
            ModelSourceEntry {
                id: "ms-1".to_string(),
                name: String::new(),
                endpoint: "e".to_string(),
                model: "m".to_string(),
                context_window: 1,
                protocol: None,
                reasoning_effort: None,
                vision: false,
                api_key: String::new(),
            },
            ModelSourceEntry {
                id: "ms-7".to_string(),
                name: String::new(),
                endpoint: "e".to_string(),
                model: "m".to_string(),
                context_window: 1,
                protocol: None,
                reasoning_effort: None,
                vision: false,
                api_key: String::new(),
            },
        ];
        assert_eq!(next_entry_id(&entries), "ms-8", "max + 1（**不回收**）");
        assert_eq!(next_entry_id(&[]), "ms-1");
    }

    /// **重新绑定时，她那两格不许被抹掉**（2026-10-09 加宽：钥匙属机器，用什么想属她）。
    ///
    /// 为什么单独立一条：写者每次只拿到 `source`，而绑定里可能有 `model` / `reasoning_effort`
    /// —— 漏了保留这一步，页面上重新绑一次就把她那两格**悄悄抹掉**（与 `reasoning_effort`
    /// 那次"页面保存抹掉清单那一格"是同族事故，实测吃过一次）。
    #[test]
    fn rebinding_keeps_what_she_said_about_model_and_effort() {
        let home = temp_home("binding-keep");
        let env_root = home.join("agents").join("a-1");
        fs::create_dir_all(&env_root).unwrap();
        fs::write(
            env_root.join("model_binding.json"),
            "{\n  \"source\": \"ms-1\",\n  \"model\": \"her-model\",\n  \"reasoning_effort\": \"max\"\n}\n",
        )
        .unwrap();

        write_binding(&env_root, "ms-2").unwrap();
        let text = fs::read_to_string(env_root.join("model_binding.json")).unwrap();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            parsed.get("source").and_then(Value::as_str),
            Some("ms-2"),
            "来源要换成新那一条"
        );
        assert_eq!(
            parsed.get("model").and_then(Value::as_str),
            Some("her-model"),
            "她那两格不许被抹掉：{text}"
        );
        assert_eq!(
            parsed.get("reasoning_effort").and_then(Value::as_str),
            Some("max")
        );

        // 没有那两格时写出来的仍是老形状（只有 `source` 一格）—— 盘上不多一个字。
        let fresh = home.join("agents").join("a-2");
        fs::create_dir_all(&fresh).unwrap();
        write_binding(&fresh, "ms-1").unwrap();
        let text = fs::read_to_string(fresh.join("model_binding.json")).unwrap();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            parsed.as_object().map(serde_json::Map::len),
            Some(1),
            "只有 source 一格：{text}"
        );
    }
}
