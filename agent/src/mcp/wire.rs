//! MCP 的**wire 形状**：JSON-RPC 的答复/清单/错误 → 我们的有界形状。
//!
//! 两条通道共用这一份：stdio（子进程逐行收发）与 remote（网络请求/事件流）。
//! 分开写两遍就等于"应答的截断、非文本块的处理、噪声计数"各有一套——那三件事
//! 一旦两套，两边的回执就会慢慢长成两种读数。
//!
//! 这里的函数**不认识通道**：只吃 `serde_json::Value`，吐出 [`CallReply`] / [`RawTool`]。

use serde_json::{Value, json};

use super::{CallReply, MAX_TEXT_BYTES, RawTool, sanitize};

/// 把一段文本读成一个**对象**（不是一个对象就当噪声：剥掉可能的前导字节序标记与空白再解析）。
pub(crate) fn as_object(text: &str) -> Option<Value> {
    let cleaned = text.trim_start_matches(['\u{feff}', ' ', '\t', '\r', '\n']);
    let value: Value = serde_json::from_str(cleaned).ok()?;
    value.is_object().then_some(value)
}

/// 错误对象 → 一句**只作携带**的原文（已消毒、已截断）。
///
/// ⚠ **只搬运"对面说出来的话"**：
/// - 有 `message` ⇒ `code=<码>: <那句话>`（码是附带事实，不是我们编的）；
/// - **没有 `message`** ⇒ 只给 `code=<码>`。
///
/// **不把整个错误对象 JSON 化当原文**：那不是"对面的话"，是我们把对面的结构序列化了一遍——
/// 记进"原文"那一格就是记录在替对面说话。
pub(crate) fn describe_error(error: &Value) -> String {
    let code = error.get("code").and_then(Value::as_i64);
    let raw = match error.get("message").and_then(Value::as_str) {
        Some(message) => match code {
            Some(code) => format!("code={code}: {message}"),
            None => message.to_string(),
        },
        None => match code {
            Some(code) => format!("code={code}"),
            None => String::new(),
        },
    };
    let clean = sanitize(&raw);
    let (clipped, truncated) = super::clip(&clean, 400);
    if truncated {
        format!("{clipped}…（截断）")
    } else {
        clipped
    }
}

/// 清单里的一条工具。名字是**必须有**的；说明与参数模式可以缺。
pub(crate) fn read_tool(item: &Value) -> Option<RawTool> {
    let name = item.get("name").and_then(Value::as_str)?.to_string();
    if name.is_empty() {
        return None;
    }
    Some(RawTool {
        name,
        description: item
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        input_schema: item
            .get("inputSchema")
            .cloned()
            .unwrap_or_else(|| json!({})),
    })
}

/// 一次调用的答复 → 有界的回执形状。
///
/// **有界**是这里做的：正文只留到 [`MAX_TEXT_BYTES`]（读的时候就不再多收），原文一共多少字节
/// 照旧报出来。非文本的块**不搬**，只按类型记条数。
pub(crate) fn read_result(result: &Value, noise: usize) -> CallReply {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut text = String::new();
    let mut text_bytes = 0usize;
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();

    if let Some(blocks) = result.get("content").and_then(Value::as_array) {
        for block in blocks {
            let kind = block
                .get("type")
                .and_then(Value::as_str)
                .map(sanitize)
                .filter(|each| !each.is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            if kind != "text" {
                *counts.entry(kind).or_insert(0) += 1;
                continue;
            }
            let part = block.get("text").and_then(Value::as_str).unwrap_or("");
            text_bytes += part.len();
            add_bounded(&mut text, part);
        }
    }
    // 结构化的那一份我们也不搬（v1 只搬文本）——它出现就要看得见。
    if result.get("structuredContent").is_some() {
        *counts.entry("structured-content".to_string()).or_insert(0) += 1;
    }

    CallReply::Replied {
        is_error,
        truncated: text_bytes > MAX_TEXT_BYTES,
        text,
        text_bytes,
        unsupported: counts.into_iter().collect(),
        noise,
    }
}

/// 往正文里追加，**只追加到上限为止**（不再多收）。
pub(crate) fn add_bounded(text: &mut String, part: &str) {
    if text.len() >= MAX_TEXT_BYTES {
        return;
    }
    if !text.is_empty() {
        text.push('\n');
    }
    let room = MAX_TEXT_BYTES.saturating_sub(text.len());
    let (piece, _) = super::clip(part, room);
    text.push_str(&piece);
}
