//! **有没有新版本** —— 关于页那一颗「检查更新」。
//!
//! ## 这一层只回答一个问题
//!
//! 「渠道上最新是哪一版、那一版的包在哪、它的 sha256 是多少」——**只读，不下载、不替换、
//! 不动家**。下载与换文件夹是后面两刀的事；把它们混进来，会让"检查"变成一个能改磁盘的动作。
//!
//! ## 渠道顺序（2026-10-21 主人裁决）
//!
//! **GitHub 优先，失败退 Gitee。** 两个地址来自发布包 `VERSION` 里的 `repo` / `mirror`
//! （`make_release.ps1` 写的那两行）——包自己说得出从哪来，不靠 `.git`（下载回来的包没有它）。
//!
//! ## 哈希那一格为什么是 `Option`
//!
//! GitHub 的 release API 会给 `digest: "sha256:…"`；Gitee 的 release 对象**不给摘要**
//! （实测 2026-10-21：assets 只有 name 与 browser_download_url），只能从发布说明的
//! `sha256: <64 hex>` 行里读。**两处都读不到 ⇒ `None`**：一个答不出"是哪一份字节"的渠道，
//! 不能被后续的下载层当成可自动替换的源（`update.ps1` 的 fail-closed 同一条纪律）。
//!
//! ## 边界
//!
//! - **不读凭据**：GitHub/Gitee 的公开 release 端点都不需要 token（匿名限流够"偶尔点一下"）；
//! - **不写任何文件**：回来的都是内存里的读数；
//! - **不把「读不到」说成「已经最新」**：两个渠道都失败 ⇒ `state = "unknown"` ＋ `note` 里
//!   两条原因；只有拿到版本号并比过，才给 `latest` / `newer`。

use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

use crate::app_version;

/// 渠道上那个包（这一层只描述，不下载）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAsset {
    pub name: String,
    pub url: String,
    pub size: Option<u64>,
    /// 64 位小写十六进制；渠道没给就是 `None`。
    pub sha256: Option<String>,
}

/// 检查的读数。`state` 是封闭词表：`latest` / `newer` / `unknown`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateReadout {
    pub state: String,
    pub current: String,
    pub latest: Option<String>,
    /// `github` / `gitee`；`unknown` 时是 `None`。
    pub channel: Option<String>,
    pub asset: Option<UpdateAsset>,
    /// 读不到时的人才话依据（原样保留渠道给的原因，不翻译成乐观说法）。
    pub note: Option<String>,
}

/// 从 `VERSION` 里读 `repo = owner/name` 这种一行（读不到 ⇒ `None`）。
pub fn channel_of(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        if name.trim() == key {
            let value = value.trim();
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        } else {
            None
        }
    })
}

/// 版本比较：数字段按数值、其余按字典序（与 `scripts/probe-release-channel.mjs` 同一条规则）。
///
/// 返回 `<0` 表示 `a` 更旧（有新版）、`0` 相同、`>0` 更新。
pub fn compare_versions(a: &str, b: &str) -> i32 {
    fn parts(value: &str) -> Vec<String> {
        value
            .split(['.', '-', '+'])
            .map(str::to_string)
            .collect()
    }
    let left = parts(a);
    let right = parts(b);
    for i in 0..left.len().max(right.len()) {
        match (left.get(i), right.get(i)) {
            (None, Some(_)) => return -1,
            (Some(_), None) => return 1,
            (Some(x), Some(y)) => {
                let nx = x.parse::<u64>().ok();
                let ny = y.parse::<u64>().ok();
                match (nx, ny) {
                    (Some(nx), Some(ny)) if nx != ny => return if nx < ny { -1 } else { 1 },
                    (Some(_), Some(_)) => {}
                    _ if x != y => return if x < y { -1 } else { 1 },
                    _ => {}
                }
            }
            (None, None) => {}
        }
    }
    0
}

/// 发布说明里的 `sha256: <64 hex>`（Gitee 的附件对象不带摘要，只有这一行能读）。
pub fn hash_in_text(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let mut rest = lower.as_str();
    while let Some(pos) = rest.find("sha256:") {
        let after = rest[pos + "sha256:".len()..].trim_start();
        let candidate: String = after.chars().take(64).collect();
        if candidate.len() == 64 && candidate.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(candidate);
        }
        rest = &rest[pos + "sha256:".len()..];
    }
    None
}

fn text_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|each| !each.is_empty())
}

fn tagged_version(value: &str) -> String {
    value.trim().trim_start_matches(['v', 'V']).to_string()
}

/// 这一版的安装包：**版本前缀与平台后缀都要对上**。
///
/// 为什么不再回落到"第一个 celira zip"：同一个 release 会挂 win 与 linux 两个包，名字前缀
/// 相同；拿错平台的包，比"没找到"更坏。名字对不上就返回 `None`，由上层如实说"没有可下载的包"。
fn pick_asset<'a>(assets: &'a [Value], version: &str) -> Option<&'a Value> {
    let platform = if cfg!(target_os = "windows") {
        "win-x64"
    } else if cfg!(target_os = "linux") {
        "linux-x64"
    } else {
        ""
    };
    let wanted = format!("celira-{version}");
    assets.iter().find(|asset| {
        text_field(asset, "name")
            .map(|name| {
                name.starts_with(&wanted)
                    && name.ends_with(".zip")
                    && (platform.is_empty() || name.contains(platform))
            })
            .unwrap_or(false)
    })
}

fn asset_of(asset: &Value) -> UpdateAsset {
    let name = text_field(asset, "name").unwrap_or("(无名)").to_string();
    let url = text_field(asset, "browser_download_url")
        .or_else(|| text_field(asset, "download_url"))
        .unwrap_or("")
        .to_string();
    let size = asset.get("size").and_then(Value::as_u64);
    let sha256 = text_field(asset, "digest")
        .and_then(|digest| digest.strip_prefix("sha256:").or_else(|| digest.strip_prefix("SHA256:")))
        .map(|each| each.to_ascii_lowercase())
        .filter(|each| each.len() == 64 && each.chars().all(|c| c.is_ascii_hexdigit()));
    UpdateAsset {
        name,
        url,
        size,
        sha256,
    }
}

/// **纯函数**：GitHub 的 `releases/latest` 载荷 → 读数。
pub fn parse_github(body: &Value, current: &str) -> UpdateReadout {
    let tag = text_field(body, "tag_name").or_else(|| text_field(body, "name"));
    let Some(tag) = tag else {
        return unknown(current, "github 回信里没有 tag_name".to_string());
    };
    let latest = tagged_version(tag);
    let assets = body
        .get("assets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let asset = pick_asset(&assets, &latest).map(asset_of).filter(|each| !each.url.is_empty());
    let state = if compare_versions(current, &latest) < 0 {
        "newer"
    } else {
        "latest"
    };
    let note = if asset.is_none() {
        Some(format!("{latest} 这一版没有可下载的 zip"))
    } else {
        None
    };
    UpdateReadout {
        state: state.to_string(),
        current: current.to_string(),
        latest: Some(latest),
        channel: Some("github".to_string()),
        asset,
        note,
    }
}

/// **纯函数**：Gitee 的 `releases` 列表载荷 → 读数（取 `created_at` 最新的那一条）。
pub fn parse_gitee(body: &Value, current: &str) -> UpdateReadout {
    let Some(list) = body.as_array() else {
        return unknown(current, "gitee 回信不是一个列表".to_string());
    };
    let Some(newest) = list
        .iter()
        .max_by(|a, b| {
            text_field(a, "created_at")
                .unwrap_or("")
                .cmp(text_field(b, "created_at").unwrap_or(""))
        })
    else {
        return unknown(current, "gitee 上还没有发行版".to_string());
    };
    let Some(tag) = text_field(newest, "tag_name").or_else(|| text_field(newest, "name")) else {
        return unknown(current, "gitee 最新那一版没有 tag".to_string());
    };
    let latest = tagged_version(tag);
    let assets = newest
        .get("assets")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut asset = pick_asset(&assets, &latest).map(asset_of).filter(|each| !each.url.is_empty());
    // Gitee 的附件对象没有摘要；发布说明里的那一行是唯一来源（`publish-gitee.ps1` 写的）。
    if let Some(found) = asset.as_mut() {
        if found.sha256.is_none() {
            found.sha256 = text_field(newest, "body")
                .or_else(|| text_field(newest, "description"))
                .and_then(hash_in_text);
        }
    }
    let state = if compare_versions(current, &latest) < 0 {
        "newer"
    } else {
        "latest"
    };
    let note = if asset.is_none() {
        Some(format!("{latest} 这一版没有可下载的 zip"))
    } else {
        None
    };
    UpdateReadout {
        state: state.to_string(),
        current: current.to_string(),
        latest: Some(latest),
        channel: Some("gitee".to_string()),
        asset,
        note,
    }
}

fn unknown(current: &str, note: String) -> UpdateReadout {
    UpdateReadout {
        state: "unknown".to_string(),
        current: current.to_string(),
        latest: None,
        channel: None,
        asset: None,
        note: Some(note),
    }
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("HTTP 客户端建不起来：{error}"))
}

async fn fetch_github(http: &reqwest::Client, repo: &str) -> Result<Value, String> {
    let response = http
        .get(format!("https://api.github.com/repos/{repo}/releases/latest"))
        .header("User-Agent", "celira-app-update")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|error| format!("连不上：{error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    response
        .json::<Value>()
        .await
        .map_err(|error| format!("回信读不懂：{error}"))
}

async fn fetch_gitee(http: &reqwest::Client, repo: &str) -> Result<Value, String> {
    let response = http
        .get(format!(
            "https://gitee.com/api/v5/repos/{repo}/releases?per_page=5"
        ))
        .header("User-Agent", "celira-app-update")
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|error| format!("连不上：{error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    response
        .json::<Value>()
        .await
        .map_err(|error| format!("回信读不懂：{error}"))
}

/// 真正的检查：给定"exe 在哪个目录"，问渠道。
///
/// 与命令分开一层，是为了能**不经过 UI**验证它（`examples/probe_update.rs`）：
/// 一条只肯在窗口里跑的网络逻辑，出问题时只能靠猜。
pub async fn check_from_dir(exe_dir: Option<&Path>) -> UpdateReadout {
    let current = app_version::readout_from(exe_dir).version;
    let version_text = exe_dir
        .and_then(|dir| std::fs::read_to_string(dir.join("VERSION")).ok())
        .unwrap_or_default();
    let repo = channel_of(&version_text, "repo");
    let mirror = channel_of(&version_text, "mirror");

    if repo.is_none() && mirror.is_none() {
        return unknown(
            &current,
            "这个包没说它属于哪个渠道（VERSION 里没有 repo / mirror）".to_string(),
        );
    }

    let http = match client() {
        Ok(http) => http,
        Err(why) => return unknown(&current, why),
    };
    let mut notes: Vec<String> = Vec::new();

    if let Some(repo) = repo {
        match fetch_github(&http, &repo).await {
            Ok(body) => return parse_github(&body, &current),
            Err(why) => notes.push(format!("github: {why}")),
        }
    }
    if let Some(mirror) = mirror {
        match fetch_gitee(&http, &mirror).await {
            Ok(body) => return parse_gitee(&body, &current),
            Err(why) => notes.push(format!("gitee: {why}")),
        }
    }

    unknown(&current, notes.join("；"))
}

/// 壳给页面的那一格：**GitHub 优先，失败退 Gitee**。
#[tauri::command]
pub async fn update_check() -> UpdateReadout {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    check_from_dir(exe_dir.as_deref()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn channel_lines_are_read_by_key_only() {
        let text = "version = 0.1.3\nrepo    = inorizx/celira\nmirror  = myloveinori/celira\n";
        assert_eq!(channel_of(text, "repo"), Some("inorizx/celira".to_string()));
        assert_eq!(channel_of(text, "mirror"), Some("myloveinori/celira".to_string()));
        // 空值 / 没有这个键 ⇒ None（不回落成空串，也不猜）。
        assert_eq!(channel_of("repo    =   \n", "repo"), None);
        assert_eq!(channel_of("version = 0.1.3\n", "repo"), None);
    }

    #[test]
    fn version_compare_follows_the_probe_rule() {
        assert_eq!(compare_versions("0.1.2", "0.1.3"), -1);
        assert_eq!(compare_versions("0.1.3", "0.1.3"), 0);
        assert_eq!(compare_versions("0.1.4", "0.1.3"), 1);
        assert_eq!(compare_versions("0.1.10", "0.1.9"), 1);
    }

    #[test]
    fn hash_line_is_read_from_prose_and_garbage_is_refused() {
        let hex = "977a3367a2f1bff395bb311a80a8fb07f369170a3af6eda1bf0d5ddd1efd7652";
        assert_eq!(hash_in_text(&format!("changes\n\nsha256: {hex}\n")), Some(hex.to_string()));
        assert_eq!(
            hash_in_text(&format!("sha256: {}", hex.to_uppercase())),
            Some(hex.to_string())
        );
        assert_eq!(hash_in_text("sha256: deadbeef"), None);
        assert_eq!(hash_in_text("no digest here"), None);
    }

    #[test]
    fn github_latest_reads_tag_asset_and_digest() {
        let hex = "e2b56326702af450dcfba5bd9fbb5a3d6df9caf4cc25bdae0d93d0bbf9954d81";
        let platform = if cfg!(target_os = "windows") { "win-x64" } else { "linux-x64" };
        let zip = format!("celira-0.1.3-{platform}.zip");
        let body = json!({
            "tag_name": "v0.1.3",
            "assets": [
                { "name": zip, "size": 29804949,
                  "browser_download_url": "https://example/celira-0.1.3.zip",
                  "digest": format!("sha256:{hex}") },
                { "name": format!("{zip}.sha256"), "size": 92,
                  "browser_download_url": "https://example/sums" }
            ]
        });
        let read = parse_github(&body, "0.1.2");
        assert_eq!(read.state, "newer");
        assert_eq!(read.latest.as_deref(), Some("0.1.3"));
        assert_eq!(read.channel.as_deref(), Some("github"));
        let asset = read.asset.expect("zip");
        assert_eq!(asset.size, Some(29804949));
        assert_eq!(asset.sha256.as_deref(), Some(hex));
    }

    #[test]
    fn github_same_version_is_latest_not_newer() {
        let body = json!({ "tag_name": "v0.1.3", "assets": [] });
        let read = parse_github(&body, "0.1.3");
        assert_eq!(read.state, "latest");
        assert_eq!(read.latest.as_deref(), Some("0.1.3"));
        // 没有包 ⇒ 明说，不假装能更新。
        assert!(read.asset.is_none());
        assert!(read.note.is_some());
    }

    #[test]
    fn gitee_reads_digest_from_the_release_body() {
        let hex = "e2b56326702af450dcfba5bd9fbb5a3d6df9caf4cc25bdae0d93d0bbf9954d81";
        let platform = if cfg!(target_os = "windows") { "win-x64" } else { "linux-x64" };
        let zip = format!("celira-0.1.3-{platform}.zip");
        let body = json!([{
            "tag_name": "v0.1.3",
            "created_at": "2026-10-21T13:31:00+08:00",
            "body": format!("release notes\n\nsha256: {hex}"),
            "assets": [
                { "name": zip,
                  "browser_download_url": "https://gitee.com/x/y/releases/download/v0.1.3/celira.zip" }
            ]
        }]);
        let read = parse_gitee(&body, "0.1.2");
        assert_eq!(read.state, "newer");
        assert_eq!(read.channel.as_deref(), Some("gitee"));
        let asset = read.asset.expect("zip");
        // Gitee 的 assets 没有 size —— 这里必须是 None，不是 0。
        assert_eq!(asset.size, None);
        assert_eq!(asset.sha256.as_deref(), Some(hex));
    }

    #[test]
    fn empty_or_malformed_channels_are_unknown_not_latest() {
        assert_eq!(parse_gitee(&json!([]), "0.1.3").state, "unknown");
        assert_eq!(parse_gitee(&json!({}), "0.1.3").state, "unknown");
        assert_eq!(parse_github(&json!({}), "0.1.3").state, "unknown");
    }
}
