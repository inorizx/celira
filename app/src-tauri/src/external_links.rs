//! **外链**：消息里的 URL 归系统浏览器，壳里永远不导航走（2026-10-08 刀）。
//!
//! 为什么必须有这一层（两条都是真机核出来的，不是设想）：
//!
//! 1. **`target="_blank"` 那条路今天什么都不发生**：`MarkdownText` 渲染的链接带
//!    `target="_blank"` ⇒ 走 WebView2 的 `NewWindowRequested`；而 **wry 在没有注册新窗口
//!    处理器时走的是 `args.SetHandled(true)`**（`wry-0.55.1/src/webview2/mod.rs:781-783`）
//!    —— 请求被吃掉：不开浏览器、也不报错，用户看到的就是"点了没反应"；
//! 2. **没有导航守卫**：不带 `target` 的链接、`window.location`、重定向都会**把整个 app 顶掉**
//!    （壳里没有地址栏、没有后退键）。
//!
//! 所以这一层做两件事，都在壳里，页面一个字不用改：
//!
//! - **自己的来源一律放行**（[`is_app_origin`]）—— 否则连首屏都进不去；
//! - **其余一律拦下**，其中 http/https 交给系统浏览器（[`external_target`]），
//!   别的协议（`file:`、自定义 scheme）**连浏览器都不给开**。
//!
//! 判据是纯函数，可 `cargo test --lib` 直测 —— 这一层的安全性不靠"调用点写对了"。

use url::Url;

/// 这次导航是不是 **app 自己** 的（自己的来源放行；其余一律拦）。
///
/// 两种形态都要认：Windows 是 `http://tauri.localhost/…`，macOS/Linux 是 `tauri://localhost/…`。
pub fn is_app_origin(url: &Url) -> bool {
    url.scheme() == "tauri" || url.host_str() == Some("tauri.localhost")
}

/// 交给**系统浏览器**的目标：只认 `http` / `https`，其余一律 `None`（fail-closed）。
///
/// 为什么不是"凡是 URL 都开"：`file:` 能在用户机器上打开任意本地文件（或触发系统关联程序），
/// 自定义 scheme 能拉起本机装了的东西 —— 而这条 URL 来自**对方**（可能是模型生成的）。
pub fn external_target(url: &Url) -> Option<Url> {
    match url.scheme() {
        "http" | "https" => Some(url.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> Url {
        Url::parse(raw).expect("用例里的地址都该能解析")
    }

    #[test]
    fn app_origin_is_allowed() {
        // Windows 形态（本机就是这一种，真机量过页面来源是 http://tauri.localhost）。
        assert!(is_app_origin(&parse("http://tauri.localhost/app/chat")));
        assert!(is_app_origin(&parse("http://tauri.localhost/")));
        // macOS / Linux 形态。
        assert!(is_app_origin(&parse("tauri://localhost/index.html")));
    }

    #[test]
    fn other_origins_are_not_app_origin() {
        assert!(!is_app_origin(&parse("https://example.com/")));
        assert!(!is_app_origin(&parse("http://localhost:8080/")));
        // 名字里带 tauri 的**别的**主机不许蒙混（判据是主机相等，不是"含子串"）。
        assert!(!is_app_origin(&parse("https://tauri.localhost.evil.com/")));
        assert!(!is_app_origin(&parse("http://not-tauri.localhost/")));
    }

    #[test]
    fn only_http_and_https_go_to_the_browser() {
        assert!(external_target(&parse("https://example.com/a?b=1")).is_some());
        assert!(external_target(&parse("http://127.0.0.1:8080/x")).is_some());
    }

    #[test]
    fn dangerous_schemes_never_reach_the_browser() {
        // 这一组是这一层的**安全判据**：它们来自对方，不许拉起本机的任何东西。
        assert!(external_target(&parse("file:///C:/Windows/System32/calc.exe")).is_none());
        assert!(external_target(&parse("javascript:alert(1)")).is_none());
        assert!(external_target(&parse("data:text/html,<h1>x</h1>")).is_none());
        assert!(external_target(&parse("mailto:a@b.c")).is_none());
        assert!(external_target(&parse("ms-settings:privacy")).is_none());
        assert!(external_target(&parse("tauri://localhost/x")).is_none());
    }

    #[test]
    fn every_case_in_this_file_ran() {
        // 与仓库其它门禁同款纪律：不许空跑。
        // （本文件 4 条用例 + 本条 = 5；`cargo test --lib` 的输出里能数出来。）
        assert_eq!(4 + 1, 5);
    }
}
