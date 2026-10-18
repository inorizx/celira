//! **原生通知**：壳里的消息提醒走**系统通知**（不是 Web 通知）。
//!
//! ## 为什么必须自己发（2026-10-08 真机读数）
//!
//! 壳里 WebView2 的 Web 通知**永远开不了**：
//!
//! 1. wry 注册的权限处理器**只放行剪贴板读**（`wry-0.55.1/src/webview2/mod.rs:500-513`）；
//! 2. WebView2 规范明写 `PermissionKind.Notification` 的 `Default` **一律当拒绝**；
//! 3. 真机实测：壳里 `Notification` 构造器**在**、`permission` 是 `default`，而
//!    `requestPermission()` 回 **`denied`**（用户看到的是「浏览器已拒绝通知权限」——在壳里是误导）。
//!
//! ⚠ **`@tauri-apps/plugin-notification` 的 JS 那一侧救不了这一格**：它在桌面端只是
//! `window.Notification` 的薄包装（`requestPermission()` 直接转发给 Web 那一套、
//! `sendNotification()` 直接 `new window.Notification(...)`）⇒ 在壳里照样是 denied。
//! 能用的是它的 **Rust 侧**（[`NotificationExt`]）—— Windows 上走系统 toast，**不经过 Web 权限**。
//!
//! ## 边界
//!
//! - **不判权限**：系统通知的开关归**系统设置**（应用问不到、也管不了）。所以页面那一侧
//!   壳里恒 `granted`，而这里 `Ok(())` 只说"**交给了系统**"，**不代表用户看见了**；
//! - **正文不是安全边界**：截断与措辞在页面那一侧（`api/notifications.ts`）已经做过；
//! - 失败**如实返回**（`Err` 带原文），绝不假装弹过。

use tauri::{AppHandle, Runtime};
use tauri_plugin_notification::NotificationExt;

/// 弹一条系统通知。`Ok(())` = 已交给系统（不代表用户看见了）；`Err` = 原文。
#[tauri::command]
pub fn notify_message<R: Runtime>(
    app: AppHandle<R>,
    title: String,
    body: String,
) -> Result<(), String> {
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|error| error.to_string())
}
