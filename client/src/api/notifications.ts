/**
 * **通知**：开关偏好 + 权限 + 弹出（2026-09-29 存储收口时从 `hooks/use-notifications.ts` 挪来）。
 *
 * ## 为什么它住在 `api/` 而不是 `hooks/`
 *
 * 从前的 `hooks/use-notifications.ts` **一个 React hook 都没有**——它是"存储 + 权限 + 平台弹窗"。
 * 而 `api/store.ts`（数据面）要读通知开关，于是 import 了 `@/hooks/...` ——
 * **数据面依赖 UI 层，方向是反的**。
 *
 * 现在平台部分归本模块（数据面可以放心依赖），`hooks/use-notifications.ts` 只留 React 那层。
 *
 * ## 作用域：**机器级**
 *
 * 判据（`api/worldIdentity.ts` 文件头）：换一个世界连，这个键该不该跟着变？
 * "要不要给我弹通知"是**这台机器上这个人**的选择，跟连哪个世界无关 ⇒ 机器级。
 *
 * ## 两种形态（2026-10-08 刀：壳里接上原生通知）
 *
 * | | 浏览器 | 壳（app） |
 * |---|---|---|
 * | 能力 | Web Notification API | **系统通知**（壳的命令 `notify_message`，见 `app/src-tauri/src/native_notify.rs`） |
 * | 为什么不用 Web 那一套 | —— | **壳里它永远开不了**：wry 注册的权限处理器只放行剪贴板读（`wry-0.55.1/src/webview2/mod.rs:500-513`），而 WebView2 规范明写 `PermissionKind.Notification` 的 `Default` **一律当拒绝**（真机实测：`requestPermission()` 回 `denied`，用户看到的是「浏览器已拒绝通知权限」——在壳里是误导） |
 * | 权限 | 浏览器自己记（要用户点一次） | **没有这一格**：系统通知的开关归**系统设置**（应用问不到也管不了）⇒ 恒 `granted` |
 * | 点击跳房 | `notification.onclick` ⇒ 派发 `celira:focus-room` | ⚠ **还没有**（系统通知的点击回调这条链上没接，见下方"已知缺口"） |
 *
 * 换的就是本模块：调用方（含 `store.ts`）**一行不改**。
 *
 * ## 已知缺口（如实登记，不假装）
 *
 * 1. **壳里点通知不会跳到那间房**：浏览器那条靠 `onclick`，壳里那套没有等价物
 *    （`tauri-plugin-notification` 的 `onAction` 在桌面端这条链上拿不到点击）；
 * 2. **壳里不能"替换"同房的那条通知**：浏览器靠 `tag` + `close()` 做到，原生通知没有句柄
 *    ⇒ 聚合窗口内每来一条就再弹一条（标题里带「（N 条新消息）」）。
 */
import { isTauriRuntime } from './shell'
import { machineScopedKey } from './worldIdentity'
import { storage } from './storage'

const NOTIFY_KEY = machineScopedKey('notifications')

export interface NotifyPrefs {
  enabled: boolean
  mentions: boolean
}

const DEFAULT_NOTIFY_PREFS: NotifyPrefs = { enabled: false, mentions: false }

/** 通知权限的**本平台**状态。`unsupported` = 这个平台两条路都没有。 */
export type NotificationPermissionState = 'granted' | 'denied' | 'default' | 'unsupported'

/** 读出开关偏好。**宽容**：读不懂一律归默认（与 `prefs.ts` / `roomPrefs.ts` 同款纪律）。 */
export async function loadNotifyPrefs(): Promise<NotifyPrefs> {
  const raw = await storage.read(NOTIFY_KEY)
  if (!raw) return { ...DEFAULT_NOTIFY_PREFS }
  try {
    const parsed = JSON.parse(raw) as Partial<NotifyPrefs>
    if (typeof parsed !== 'object' || parsed === null) return { ...DEFAULT_NOTIFY_PREFS }
    return {
      enabled: parsed.enabled === true,
      mentions: parsed.mentions === true,
    }
  } catch {
    return { ...DEFAULT_NOTIFY_PREFS }
  }
}

/** 写开关偏好。写不进去返回 `false`（不抛）—— 调用方要让用户看见"没保存住"。 */
export async function saveNotifyPrefs(next: NotifyPrefs): Promise<boolean> {
  return storage.write(NOTIFY_KEY, JSON.stringify(next))
}

/** 平台这一侧有没有 Web 通知能力（`Notification` 构造器在不在）。 */
function notificationCtor(): typeof Notification | null {
  return typeof globalThis.Notification !== 'undefined' ? globalThis.Notification : null
}

/**
 * 壳里的权限**没有这一格**：系统通知的开关归**系统设置**（应用问不到、也管不了）
 * ⇒ 恒 `granted`；真的没弹出去时由 [`showMessageNotification`] 如实回 `false`。
 *
 * ⚠ 为什么不去问插件：`@tauri-apps/plugin-notification` 的 **JS** 那一侧在桌面端只是
 * `window.Notification` 的薄包装（`requestPermission()` 直接转发给 Web 那一套）⇒ 在壳里照样
 * 回 `denied`，问它等于**把一句假话当真话**。能用的是它的 Rust 侧，也就是壳那条命令。
 */
function shellPermission(): NotificationPermissionState {
  return 'granted'
}

export function notificationPermission(): NotificationPermissionState {
  if (isTauriRuntime()) return shellPermission()
  const ctor = notificationCtor()
  return ctor ? (ctor.permission as NotificationPermissionState) : 'unsupported'
}

/**
 * 请求通知权限。**权限归平台**（浏览器自己记；壳里没有这一格）。
 * 两条路都没有 ⇒ `false`（明确不可用，不假装）。
 */
export async function ensureNotificationPermission(): Promise<boolean> {
  if (isTauriRuntime()) return true
  const ctor = notificationCtor()
  if (!ctor) return false
  if (ctor.permission === 'granted') return true
  if (ctor.permission === 'denied') return false
  try {
    return (await ctor.requestPermission()) === 'granted'
  } catch {
    return false
  }
}

/** 同一个房间的通知聚合窗口（30 秒内合并成"（N 条新消息）"）。 */
const AGGREGATE_MS = 30_000
const lastNotifyAtByRoom = new Map<string, number>()
const lastCountByRoom = new Map<string, number>()
/** 浏览器那条要留着上一条的句柄才能"替换"它；壳里没有句柄（见文件头的已知缺口）。 */
const lastBrowserNotificationByRoom = new Map<string, Notification>()

/**
 * 弹一条消息通知。**返回是否真的弹了**（没有能力、没授权、窗口在前台都不弹）。
 *
 * 判据"窗口在前台就不弹"两种形态**共用**：`document.hidden` / `document.hasFocus()` ——
 * 壳里也是 Chromium（WebView2），这两个读数一样成立；缺 `document` 时**按"不知道就不弹"**处理
 * （宁可少弹一次，也不要在用户正看着的时候弹）。
 */
export async function showMessageNotification(opts: {
  roomId: string
  title: string
  body: string
}): Promise<boolean> {
  const doc = typeof globalThis.document !== 'undefined' ? globalThis.document : null
  if (!doc) return false
  if (!doc.hidden && doc.hasFocus()) return false

  const now = Date.now()
  const isAggregate = lastNotifyAtByRoom.has(opts.roomId) && now - (lastNotifyAtByRoom.get(opts.roomId) ?? 0) < AGGREGATE_MS
  lastNotifyAtByRoom.set(opts.roomId, now)
  const count = isAggregate ? (lastCountByRoom.get(opts.roomId) ?? 1) + 1 : 1
  lastCountByRoom.set(opts.roomId, count)
  const title = isAggregate ? `${opts.title}（${count} 条新消息）` : opts.title

  if (isTauriRuntime()) {
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      // 交给**系统通知**（壳那条命令）。`Ok` 只说"交给了系统"，**不代表用户看见了**。
      await invoke('notify_message', { title, body: opts.body })
      return true
    } catch (error) {
      console.error('Celira：壳里的通知没弹出去', error)
      return false
    }
  }

  const ctor = notificationCtor()
  if (!ctor || ctor.permission !== 'granted') return false
  const last = lastBrowserNotificationByRoom.get(opts.roomId)
  if (isAggregate) last?.close()
  const notification = new ctor(title, {
    body: opts.body,
    tag: `celira-${opts.roomId}`,
    silent: isAggregate,
  })
  Object.defineProperty(notification, 'data', { value: { count }, writable: true, configurable: true })
  notification.onclick = () => {
    globalThis.window?.focus()
    notification.close()
    globalThis.window?.dispatchEvent(
      new CustomEvent('celira:focus-room', { detail: { roomId: opts.roomId } }),
    )
  }
  lastBrowserNotificationByRoom.set(opts.roomId, notification)
  return true
}
