//! **未读角标**（2026-10-08）：托盘悬停那句、任务栏上的小红点、非 Windows 的系统角标。
//!
//! ## 为什么是这一层
//!
//! 未读这件事**只有页面知道**（`rooms[].unreadCount` 来自世界的 read state），而"角标"只有壳画得出来
//! （任务栏 / Dock / 托盘都不是网页能碰的）。所以分工是：**页面报一个总数**（`set_unread_total`），
//! **壳负责让它在窗口外看得见**。壳不解释这个数、也不自己算 —— 它是纯粹的一条读数。
//!
//! ## 三处落点（各按平台能做到的来）
//!
//! - **托盘悬停那句话**：跨平台都有。⚠ 那句文案的**唯一出处**是 `lib.rs` 的 `tray_tooltip_now`
//!   （服务快照 ＋ 未读），本模块只提供**拼法**（[`tray_tooltip`]）—— 不许在别处再拼一遍。
//! - **Windows**：任务栏按钮上的**覆盖小图标**（`set_overlay_icon`），就是那个小红点。
//!   ⚠ 图标**在代码里画**（[`dot_rgba`]），不引入图片资源：一个 16×16 的点不值得一条资源管线，
//!   而纯函数的圆点是可测的。
//! - **macOS / Linux**：系统自己的角标计数（`set_badge_count`）。
//!
//! ## 不做什么
//!
//! - **不闪烁、不做动画**：角标不是"提醒你一下"，是"还有多少没看"；
//! - **不自己决定阈值**：`99+` 那档是展示上限（[`unread_suffix`]），不是把数改小；
//! - **不新增任何写者**：这个数由页面给，壳只读；世界里的 read state 仍然是唯一权威。

use std::sync::Mutex;

use tauri::Manager;

/// 红点的边长（像素）。16×16 是 Windows 覆盖图标的常规尺寸，系统会按 DPI 缩放。
pub const DOT_SIZE: u32 = 16;

/// 小红点的颜色：与应用内那个红**同一个值**（`client/src/styles/tokens.css` 的 `--destructive` 浅色档）。
///
/// ⚠ 这里重写了一遍字面量：壳不认识 CSS 令牌，而这个点必须跟应用里那个红**一眼是同一个红**。
/// 改了令牌就要改这里（`tokens.css` 是唯一权威，本常量是它在壳侧的影子）。
const DOT_RGBA: [u8; 4] = [0xd6, 0x45, 0x45, 0xff];

/// 页面报上来的未读总数（壳只读；`None` 等价于 0）。
#[derive(Default)]
pub struct UnreadTotal(pub Mutex<u32>);

/// 当前未读总数（锁坏了按 0 处理 —— 角标是装饰，**不许**因为一行装饰把别的路径带崩）。
pub fn current(app: &tauri::AppHandle) -> u32 {
    app.try_state::<UnreadTotal>()
        .and_then(|state| state.0.lock().ok().map(|guard| *guard))
        .unwrap_or(0)
}

/// 悬停那句话里"未读"那一段。**0 条时是空串** —— 于是没有未读时的文案与从前**逐字节相同**。
pub fn unread_suffix(unread: u32) -> String {
    match unread {
        0 => String::new(),
        // 99 以上只报"99+"：角标是给一眼看的，不是账本（真实条数在会话列表里逐间房都有）。
        1..=99 => format!(" · {unread} 条未读"),
        _ => " · 99+ 条未读".to_string(),
    }
}

/// 托盘悬停那句话的**唯一拼法**：服务快照 ＋ 未读。
pub fn tray_tooltip(summary: &str, unread: u32) -> String {
    format!("Celira —— {summary}{}", unread_suffix(unread))
}

/// 画那个小红点（RGBA，行优先）。透明底 ＋ 一个居中的实心圆。
///
/// 纯函数（不碰任何系统接口）⇒ 可测：中心是那个红、四角是透明的。
pub fn dot_rgba(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size as usize) * (size as usize) * 4];
    if size == 0 {
        return out;
    }
    let center = (size as f32 - 1.0) / 2.0;
    // 留一圈透明边：Windows 会把这图标贴到按钮右下角，挨着边缘画会显得被裁掉。
    let radius = (size as f32 / 2.0 - 1.0).max(1.0);
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            if (dx * dx + dy * dy).sqrt() <= radius {
                let i = ((y as usize) * (size as usize) + x as usize) * 4;
                out[i..i + 4].copy_from_slice(&DOT_RGBA);
            }
        }
    }
    out
}

/// 把角标落到窗口上（按平台走能走的那条路）。
pub fn apply_window_badge(app: &tauri::AppHandle, unread: u32) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    #[cfg(windows)]
    {
        let icon = (unread > 0)
            .then(|| tauri::image::Image::new_owned(dot_rgba(DOT_SIZE), DOT_SIZE, DOT_SIZE));
        let _ = window.set_overlay_icon(icon);
    }
    #[cfg(not(windows))]
    {
        let count = (unread > 0).then_some(i64::from(unread.min(999)));
        let _ = window.set_badge_count(count);
    }
}

/// 页面报未读总数（`0` = 清掉角标）。
///
/// 返回**拼好的那句悬停文案** —— 不是为了好看，是为了让"壳到底收到没有"有一个**可核对的读数**
/// （真机探针就断言这个返回值，而不是去截屏看任务栏）。
#[tauri::command]
pub fn set_unread_total(app: tauri::AppHandle, count: u32) -> Result<String, String> {
    let state = app
        .try_state::<UnreadTotal>()
        .ok_or_else(|| "未读状态没装配上".to_string())?;
    {
        let mut guard = state.0.lock().map_err(|_| "未读状态锁坏了".to_string())?;
        *guard = count;
    }
    // 托盘那句要跟着变（唯一出处仍是 `refresh_tray`）。
    crate::refresh_tray(&app);
    apply_window_badge(&app, count);
    Ok(crate::tray_tooltip_now(&app))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ⚠ 这里**没有**"本文件真的跑了 N 条"那种计数器断言：Rust 的用例**并行**跑，
    //   全局计数与最后一条断言之间是竞态（实测会假红）。用例数由 `cargo test` 自己的
    //   `test result: N passed` 那行给出 —— 那才是它的权威读数。

    #[test]
    fn unread_suffix_is_empty_at_zero_and_caps_at_99() {
        // ⚠ 0 条必须是**空串**：没有未读时那句悬停文案与从前逐字节相同（不许悄悄多一个空格）。
        assert_eq!(unread_suffix(0), "");
        assert_eq!(unread_suffix(1), " · 1 条未读");
        assert_eq!(unread_suffix(99), " · 99 条未读");
        // 100 以上只报 99+（角标是给一眼看的，不是账本）。
        assert_eq!(unread_suffix(100), " · 99+ 条未读");
        assert_eq!(unread_suffix(4321), " · 99+ 条未读");
    }

    #[test]
    fn tray_tooltip_keeps_the_old_string_when_nothing_is_unread() {
        assert_eq!(tray_tooltip("1 支在跑", 0), "Celira —— 1 支在跑");
        assert_eq!(tray_tooltip("都停了", 3), "Celira —— 都停了 · 3 条未读");
    }

    #[test]
    fn dot_rgba_paints_a_center_and_leaves_corners_transparent() {
        let size = DOT_SIZE;
        let buffer = dot_rgba(size);
        assert_eq!(buffer.len(), (size * size * 4) as usize);
        let at = |x: u32, y: u32| {
            let i = ((y * size + x) * 4) as usize;
            [buffer[i], buffer[i + 1], buffer[i + 2], buffer[i + 3]]
        };
        // 中心：那个红，且不透明。
        assert_eq!(at(size / 2, size / 2), DOT_RGBA);
        // 四角：透明（留了一圈边）。
        for (x, y) in [(0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)] {
            assert_eq!(at(x, y)[3], 0, "角上应该是透明的");
        }
        // 尺寸为 0 时不许 panic（退化输入）。
        assert!(dot_rgba(0).is_empty());
    }
}
