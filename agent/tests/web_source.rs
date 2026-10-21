//! **web 源的清单 + 绑定**（2026-10-20）：读侧判据。
//!
//! 这一份只判四件事：
//!
//! 1. **没有绑定 ≠ 读不出来**：前者 `Ok(None)`（两个动词都说没配），后者 `Err`；
//! 2. **一条源两个能力**：`search` / `fetch` 都得在，缺一格是**形状不对**（不是"那条动词不可用"）；
//! 3. **绑到不存在的条目** ⇒ 如实拒绝（**绝不**回落到清单里的第一条）；
//! 4. **一份清单里有一条坏，整份拒绝**——静默跳过去等于换了一家（花的是另一把钥匙）。

use std::fs;
use std::path::{Path, PathBuf};

use celira_agent::web_app::{FetchWire, SearchWire};
use celira_agent::web_source::{BINDING_FILE, LIST_FILE, Resolved, resolve};

fn home(label: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("celira-web-source-{}-{label}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("agents").join("a-1")).expect("建立临时家");
    root
}

fn write_list(home: &Path, text: &str) {
    fs::write(home.join(LIST_FILE), text).expect("写清单");
}

fn write_binding(home: &Path, text: &str) {
    fs::write(home.join("agents").join("a-1").join(BINDING_FILE), text).expect("写绑定");
}

fn env_root(home: &Path) -> PathBuf {
    home.join("agents").join("a-1")
}

const GOOD: &str = r#"[
  {
    "id": "ss-1",
    "name": "深度求索",
    "api_key": "sk-sentinel-1",
    "search": { "wire": "search-api", "endpoint": "https://example.test/v1/tools/search" },
    "fetch":  { "wire": "fetch-doc",  "endpoint": "https://example.test/v1/tools/fetch" }
  },
  {
    "id": "ss-2",
    "name": "备用",
    "api_key": "tvly-sentinel-2",
    "search": { "wire": "results-json", "endpoint": "https://example.test/search" },
    "fetch":  { "wire": "fetch-raw",     "endpoint": "https://example.test/extract" }
  }
]"#;

#[test]
fn without_a_binding_there_is_no_web_source_even_when_the_list_exists() {
    let home = home("unbound");
    write_list(&home, GOOD);
    let read = celira_agent::web_source::read(&home.join(LIST_FILE), &env_root(&home));
    assert!(
        matches!(read, Ok(None)),
        "没有绑定就是没配（清单在不在都一样）"
    );
}

#[test]
fn a_bound_entry_resolves_both_capabilities() {
    let home = home("bound");
    write_list(&home, GOOD);
    write_binding(&home, r#"{"source":"ss-2"}"#);
    let entry = celira_agent::web_source::read(&home.join(LIST_FILE), &env_root(&home))
        .expect("读得出来")
        .expect("绑定了");
    assert_eq!(entry.id, "ss-2");
    assert_eq!(entry.name, "备用");

    let search = entry.search_access().expect("查那一条要成立");
    assert_eq!(search.wire(), SearchWire::ResultsJson);
    let fetch = entry.fetch_access().expect("取那一条要成立");
    assert_eq!(fetch.wire(), FetchWire::Raw);
}

#[test]
fn a_broken_binding_is_an_error_not_an_empty_configuration() {
    let home = home("broken-binding");
    write_list(&home, GOOD);
    write_binding(&home, "{ 这不是 JSON");
    let message = celira_agent::web_source::read(&home.join(LIST_FILE), &env_root(&home))
        .err()
        .expect("坏绑定必须如实失败");
    assert!(message.contains(BINDING_FILE), "点名哪一个文件：{message}");
}

#[test]
fn a_binding_that_points_nowhere_is_refused_by_name() {
    let home = home("dangling");
    write_list(&home, GOOD);
    write_binding(&home, r#"{"source":"ss-9"}"#);
    let message = celira_agent::web_source::read(&home.join(LIST_FILE), &env_root(&home))
        .err()
        .expect("悬空绑定必须拒绝");
    assert!(message.contains("ss-9"), "点名那一格：{message}");
    assert!(
        message.contains("ss-1") && message.contains("ss-2"),
        "也要说清单里到底有哪几条：{message}"
    );
}

#[test]
fn an_unknown_wire_is_refused_with_the_closed_table() {
    let home = home("unknown-wire");
    write_list(
        &home,
        r#"[{"id":"ss-1","api_key":"k",
             "search":{"wire":"whatever","endpoint":"https://example.test/a"},
             "fetch":{"wire":"fetch-doc","endpoint":"https://example.test/b"}}]"#,
    );
    write_binding(&home, r#"{"source":"ss-1"}"#);
    let message = celira_agent::web_source::read(&home.join(LIST_FILE), &env_root(&home))
        .err()
        .expect("认不得的方言必须拒绝");
    assert!(message.contains("whatever"), "{message}");
    assert!(
        message.contains("results-json") && message.contains("search-api"),
        "要把封闭表的取值说出来：{message}"
    );
}

#[test]
fn a_source_missing_one_capability_is_refused() {
    let home = home("half");
    write_list(
        &home,
        r#"[{"id":"ss-1","api_key":"k",
             "search":{"wire":"search-api","endpoint":"https://example.test/a"}}]"#,
    );
    write_binding(&home, r#"{"source":"ss-1"}"#);
    let message = celira_agent::web_source::read(&home.join(LIST_FILE), &env_root(&home))
        .err()
        .expect("少一个能力是形状不对");
    assert!(message.contains("fetch"), "要点名缺的是哪一个：{message}");
}

#[test]
fn a_source_without_a_credential_is_refused() {
    let home = home("no-key");
    write_list(
        &home,
        r#"[{"id":"ss-1",
             "search":{"wire":"search-api","endpoint":"https://example.test/a"},
             "fetch":{"wire":"fetch-doc","endpoint":"https://example.test/b"}}]"#,
    );
    write_binding(&home, r#"{"source":"ss-1"}"#);
    let message = celira_agent::web_source::read(&home.join(LIST_FILE), &env_root(&home))
        .err()
        .expect("缺凭据必须拒绝");
    assert!(message.contains("api_key"), "{message}");
}

#[test]
fn one_broken_entry_refuses_the_whole_list() {
    let home = home("one-bad");
    write_list(
        &home,
        r#"[
          {"id":"ss-1","api_key":"k",
           "search":{"wire":"search-api","endpoint":"https://example.test/a"},
           "fetch":{"wire":"fetch-doc","endpoint":"https://example.test/b"}},
          {"id":"ss-2","api_key":"",
           "search":{"wire":"search-api","endpoint":"https://example.test/c"},
           "fetch":{"wire":"fetch-doc","endpoint":"https://example.test/d"}}
        ]"#,
    );
    write_binding(&home, r#"{"source":"ss-1"}"#);
    let message = celira_agent::web_source::read(&home.join(LIST_FILE), &env_root(&home))
        .err()
        .expect("一条坏就整份拒绝（静默跳过等于换了一家）");
    assert!(message.contains("第 2 条"), "点名是第几条：{message}");
    assert!(message.contains("api_key"), "也要说缺的是哪个键：{message}");
}

#[test]
fn a_missing_list_with_a_binding_is_an_error() {
    let home = home("no-list");
    write_binding(&home, r#"{"source":"ss-1"}"#);
    let message = celira_agent::web_source::read(&home.join(LIST_FILE), &env_root(&home))
        .err()
        .expect("绑定了却没有清单 ⇒ 读不出来");
    assert!(message.contains(LIST_FILE), "{message}");
}

#[test]
fn resolve_says_which_of_the_three_situations_this_is() {
    let home = home("resolve");
    assert!(matches!(
        resolve(&home.join(LIST_FILE), &env_root(&home)),
        Resolved::Unbound
    ));
    write_list(&home, GOOD);
    write_binding(&home, r#"{"source":"ss-1"}"#);
    assert!(matches!(
        resolve(&home.join(LIST_FILE), &env_root(&home)),
        Resolved::Bound(_)
    ));
    write_binding(&home, "{ 坏");
    assert!(matches!(
        resolve(&home.join(LIST_FILE), &env_root(&home)),
        Resolved::Broken(_)
    ));
}
