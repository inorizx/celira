/**
 * **未读角标**（2026-10-08）：把"还有多少没看"报到壳上，让它在**窗口外**看得见
 * （托盘悬停那句 ＋ Windows 任务栏小红点／macOS·Linux 的系统角标）。
 *
 * ## 为什么只有壳里才做
 *
 * 浏览器页面碰不到任务栏与 Dock —— 未读在浏览器里本来就由**标签页标题/角标**之外的东西承担
 * （页面自己画的那个红点）。所以这条在浏览器形态是**空操作**，不是"还没实现"。
 *
 * ## 边界
 *
 * - 页面报的只是**一个总数**（`rooms[].unreadCount` 求和）；壳不解释它、更不自己算 ——
 *   世界里的 read state 仍是唯一权威；
 * - 壳那边怎么画（画什么颜色、什么图标）归 `app/src-tauri/src/unread.rs`，本文件只说"多少条"；
 * - 读数口：命令**返回拼好的那句悬停文案**，真机探针断言它（而不是去截屏看任务栏）。
 */
import { isTauriRuntime } from './shell'

/** 上一次报上去的数：**同一个数不重复过桥**（房间列表每变一次都调一次会很吵）。 */
let lastReported: number | null = null

/**
 * 把未读总数报给壳（浏览器里什么都不做）。
 *
 * ⚠ 失败**不抛给调用方**（角标是装饰，不该把连接或渲染带崩），但也不静默 —— 打 `console.error`，
 * 因为"角标不亮"这种问题只能靠这条线索查。
 */
export async function syncUnreadBadge(total: number): Promise<void> {
  const count = Math.max(0, Math.floor(total) || 0)
  if (!isTauriRuntime()) return
  if (lastReported === count) return
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const tooltip = await invoke<string>('set_unread_total', { count })
    lastReported = count
    // 成功也留一行：这条读数就是"壳到底收到没有"的证据（真机探针读同一句话）。
    console.info(`[unread] 已报给壳：${count} 条 —— ${tooltip}`)
  } catch (error) {
    console.error('[unread] 未读角标没报成（壳没收到）', error)
  }
}
