/**
 * **通知的 React 面**（2026-09-29 收口后只剩这一层）。
 *
 * 平台与存储部分（开关偏好、权限、弹出）已挪到 **`@/api/notifications`** ——
 * 原因是数据面（`api/store.ts`）要读开关，而从前的 `@/hooks/use-notifications` 让
 * **数据面依赖了 UI 层**，方向是反的。现在数据面依赖 `@/api/notifications`，
 * 页面继续从这里取（本模块把它们**原样转出**，页面不必改 import 路径）。
 *
 * ## 唯一的钩子（有真实消费者：`SettingsNotificationsPage`）
 *
 * 开关偏好的读取现在是**异步**的（存储口异步，见 `api/storage.ts`），所以页面不能再用
 * `useState(loadPrefs())` 那种"初始值即读"的写法。`useNotificationSettings` 把它包成
 * "默认值上屏 → 读到之后补上 → 写入后回填"，页面只消费结果。
 *
 * ## 存储写失败不静默
 *
 * `setEnabled` / `setMentions` 返回**实际写入的值**：写不进去（隐私模式、配额满）就返回
 * `false`，页面据此把开关拨回去 —— **不许把失败吞成"已保存"**。
 */
import { useCallback, useEffect, useState } from 'react'
import {
  ensureNotificationPermission,
  loadNotifyPrefs,
  notificationPermission,
  saveNotifyPrefs,
  showMessageNotification,
  type NotificationPermissionState,
} from '@/api/notifications'

export { ensureNotificationPermission, notificationPermission, showMessageNotification }
export type { NotificationPermissionState }

export interface NotificationSettings {
  /** 开关偏好的**已落盘**值（读之前先给默认值，不阻塞首屏）。 */
  enabled: boolean
  mentions: boolean
  /** 这个平台的权限状态（`unsupported` = 这个平台没有浏览器通知那一套）。 */
  permission: NotificationPermissionState
  /** 偏好是否已经从存储读回来（首帧为 false）。 */
  loaded: boolean
  /** 问平台要权限；**问完界面上的 `permission` 会自己更新**。 */
  requestPermission: () => Promise<boolean>
  /** 写回两个开关；返回**实际落盘**的值（写不进去就是 `false`）。 */
  setEnabled: (enabled: boolean) => Promise<boolean>
  setMentions: (mentions: boolean) => Promise<boolean>
}

export function useNotificationSettings(): NotificationSettings {
  const [enabled, setEnabledState] = useState(false)
  const [mentions, setMentionsState] = useState(false)
  const [loaded, setLoaded] = useState(false)
  // 权限是平台侧的事实，不进存储；进页面时读一次，**用户拨开关之后再读一次**
  // （授权请求会改变它，界面上的说明文案要跟着变）。
  const [permission, setPermission] = useState<NotificationPermissionState>(() => notificationPermission())

  useEffect(() => {
    let alive = true
    void loadNotifyPrefs().then((p) => {
      if (!alive) return
      setEnabledState(p.enabled)
      setMentionsState(p.mentions)
      setLoaded(true)
    })
    return () => {
      alive = false
    }
  }, [])

  /**
   * 问平台要权限（会弹系统框），**问完把权限状态同步进界面**。
   *
   * 这是本模块对 `@/api/notifications` 同名函数的唯一包装理由：权限是 React state，
   * 页面不该自己去 `Notification.permission`（那正是从前 `SettingsNotificationsPage`
   * 直接碰平台 API 的原因）。
   */
  const requestPermission = useCallback(async () => {
    const granted = await ensureNotificationPermission()
    setPermission(notificationPermission())
    return granted
  }, [])

  const setEnabled = useCallback(async (next: boolean) => {
    const ok = await saveNotifyPrefs({ enabled: next, mentions: (await loadNotifyPrefs()).mentions })
    const stored = ok ? next : (await loadNotifyPrefs()).enabled
    setEnabledState(stored)
    return stored
  }, [])

  const setMentions = useCallback(async (next: boolean) => {
    const ok = await saveNotifyPrefs({ enabled: (await loadNotifyPrefs()).enabled, mentions: next })
    const stored = ok ? next : (await loadNotifyPrefs()).mentions
    setMentionsState(stored)
    return stored
  }, [])

  return {
    enabled,
    mentions,
    permission,
    loaded,
    requestPermission,
    setEnabled,
    setMentions,
  }
}
