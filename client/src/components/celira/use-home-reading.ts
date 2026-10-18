import { useCallback, useEffect, useState } from 'react'
import { hasRealWorldTransport } from '@/api/store'
import { fetchHomeStatus, type HomeStatusView } from '@/api/homeStatus'

/**
 * 本机状态口的一份读数。
 *
 * **家口远程访问刀起，"运行 / 活动"那两个房间不再走这里** —— 它们按家条目取数
 * （`use-home-entries.ts`，本机或远程都可能是那台机器）。本 hook 留在原处服务**只问本机**的
 * 那些面（`SettingsServerPage` 的"本机服务端"那一行）：它读的就是同源代转那条路。
 */

/** 三种态：正在读 / 读不到 / 读到了。**"读不到"不是"Agent 停了"**（口径在页面文案里）。 */
export type HomeReading =
  | { kind: 'loading' }
  | { kind: 'unreachable' }
  | { kind: 'loaded'; view: HomeStatusView }

/**
 * 拉一次本机状态口的读数。
 *
 * 失败一律归「没读到」，**不是**"Agent 没在跑" —— 三种失败（口子没开 / 超时 / 形状不认识）
 * 对用户是同一件事：这一面没有读数。
 */
export function useHomeReading(): { reading: HomeReading; retry: () => void } {
  const [reading, setReading] = useState<HomeReading>({ kind: 'loading' })
  const [attempt, setAttempt] = useState(0)
  useEffect(() => {
    let cancelled = false
    setReading({ kind: 'loading' })
    void (async () => {
      // 演示通道没有家在跑（见 `use-home-entries.ts` 同一道闸的说明）：
      // 不打 `/home-status`，如实"没读到"。
      if (!hasRealWorldTransport()) {
        if (!cancelled) setReading({ kind: 'unreachable' })
        return
      }
      try {
        const view = await fetchHomeStatus()
        if (!cancelled) setReading({ kind: 'loaded', view })
      } catch {
        if (!cancelled) setReading({ kind: 'unreachable' })
      }
    })()
    return () => {
      cancelled = true
    }
  }, [attempt])
  return { reading, retry: useCallback(() => setAttempt((n) => n + 1), []) }
}
