import { useCallback, useEffect, useState } from 'react'
import { hasRealWorldTransport } from '@/api/store'
import { fetchHomeStatus, type HomeStatusView } from '@/api/homeStatus'
import { fetchHomeConfig, type HomeConfigView } from '@/api/homeConfig'
import {
  homeFailureText,
  loadHomeEntries,
  localHomeEntry,
  type HomeEntry,
  type HomeReading,
} from '@/api/homes'

/**
 * 家条目 + 每个条目的两份读数（家口远程访问刀）。
 *
 * 为什么单开一个 hook：**「运行」与「活动」说的是同一轮的事实**（都读
 * `fetchHomeStatus`），而它们现在可能问的是**任一**条目（本机或远程）——
 * 所以取数只做一次、各处共用；**每个条目的成败各自成档**（一份读不到不许连累另一份，
 * 也不许连累另一个条目 —— 与「哪一格有读数就说哪一格」同一条纪律）。
 *
 * ⚠ 消费者是**执行端**那一侧（`pages/RuntimesPage.tsx` 的列表与详情、
 * `SettingsHomesPage`、「我的 Agent」列表行）：机器是主语，**不做 Agent 认领**。
 *
 * 三档状态（**不许压成两档**）：
 *
 * - `loading`：还在读（先读 IndexedDB 里的条目，再并行读每个条目的状态/配置门）；
 * - `unavailable`：这台浏览器**存不住家条目**（本地存储读不出来）⇒ 如实说，
 *   不许把它画成"你一个家都没配过"（那是把读不到说成一个事实）；
 * - `ready`：条目就绪（**第一条永远是预置的「本机」**），且每份读数的成败都在 `readings` 里
 *   —— 全部失败也是一种 `ready`（页面照"读不到 + 原因"显示，不必再分一档）。
 */

export type HomeEntryState =
  | { kind: 'loading' }
  | { kind: 'unavailable'; message: string }
  | { kind: 'ready'; entries: HomeEntry[]; readings: EntryReading[] }

/** 一个条目这一次的读数（状态口 + 模型门；各自成败，失败带固定人话）。 */
export interface EntryReading {
  entry: HomeEntry
  status: { view: HomeStatusView | null; failure: string | null }
  config: { view: HomeConfigView | null; failure: string | null }
}

/** 把一次失败接成"该条目那一族读数的人话"（分类与人话都在 `homes.ts` 一处）。 */
function fail(entry: HomeEntry, error: unknown): string {
  return homeFailureText(entry, error)
}

/** 把逐条目读数压成**认领**要的那份形状（同一批数据，只留"归属"那一格）。 */
export function agentHomeReadings(readings: EntryReading[]): HomeReading<HomeStatusView>[] {
  return readings.map((reading) => ({
    entry: reading.entry,
    view: reading.status.view,
    failure: reading.status.failure,
  }))
}

/** 只要**条目列表**（不探读数）—— 配置页的"配哪台机器"选择用它：
 * 那一页自己读"选中的那台"的配置，不该替**每一台**都打一遍读数（那是替它多干活）。 */
export function useHomeEntryList(): { entries: HomeEntry[]; settled: boolean; reload: () => void } {
  const [entries, setEntries] = useState<HomeEntry[] | null>(null)
  const [attempt, setAttempt] = useState(0)
  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const list = await loadHomeEntries()
        if (!cancelled) setEntries(list)
      } catch {
        // 读不到本地存储 ⇒ 只剩预置的「本机」（页面的写路径会各自如实报错，不吞）。
        if (!cancelled) setEntries([localHomeEntry()])
      }
    })()
    return () => {
      cancelled = true
    }
  }, [attempt])
  return {
    entries: entries ?? [],
    settled: entries !== null,
    reload: useCallback(() => setAttempt((n) => n + 1), []),
  }
}

export function useHomeEntries(): { state: HomeEntryState; retry: () => void } {
  const [state, setState] = useState<HomeEntryState>({ kind: 'loading' })
  const [attempt, setAttempt] = useState(0)

  useEffect(() => {
    let cancelled = false
    setState({ kind: 'loading' })
    void (async () => {
      // **演示通道里只留"本机"那一条，且不发任何请求**（2026-09-25）。
      //
      // 为什么要有这一格：`/home-status/*` 与 `/home-config` 是**同源代转**到本机那两个口子
      // （`:5190` / `:5191`）的。演示通道里家根本没在跑，于是每次进 `/app/runtimes`
      // 或 `/app/agents` 都会打出一串 502 —— 控制台红一片，而那不是"演示通道坏了"，
      // 是**这一层本来就不该问**。
      //
      // ⚠ **只挡"远程条目"，不挡"本机条目"**：本机那条走的是同源相对路径，
      // 请求同样会 502，但**它必须显示出来** —— 否则执行端列表在演示通道恒为「0 台」，
      // 那正是这一页存在的意义（一台机器都没有，页面就没什么可看的了）。
      // 它这次的读数如实给 `null` + `failure: null`（= 没有读数），
      // 页面于是渲染「读不到运行情况」那张诚实卡 —— 那是**如实**的：演示通道确实没有那台机器。
      //
      // 判据是 `hasRealWorldTransport()`（传输层是不是 `CoreApi`）。
      // ⚠ 别改成 `getApi() === null`：演示通道 `connect()` 之后拿到的是 `MockApi`，
      // 那个判据**在演示态不成立** —— 2026-09-25 就是这么漏掉的。
      const realTransport = hasRealWorldTransport()
      let entries: HomeEntry[]
      try {
        entries = await loadHomeEntries()
      } catch {
        if (!cancelled) {
          setState({
            kind: 'unavailable',
            message: '这台浏览器存不住家条目（读不了本地存储）—— 只能照看本机这一条',
          })
        }
        return
      }
      if (!realTransport) {
        const localOnly = entries.filter((entry) => entry.local)
        if (!cancelled) {
          setState({
            kind: 'ready',
            entries: localOnly,
            readings: localOnly.map((entry) => ({
              entry,
              status: { view: null, failure: null },
              config: { view: null, failure: null },
            })),
          })
        }
        return
      }
      const readings = await Promise.all(
        entries.map(async (entry): Promise<EntryReading> => {
          let status: EntryReading['status'] = { view: null, failure: null }
          let config: EntryReading['config'] = { view: null, failure: null }
          await Promise.all([
            (async () => {
              try {
                status = { view: await fetchHomeStatus(entry), failure: null }
              } catch (error) {
                status = { view: null, failure: fail(entry, error) }
              }
            })(),
            (async () => {
              try {
                config = { view: await fetchHomeConfig(undefined, entry), failure: null }
              } catch (error) {
                config = { view: null, failure: fail(entry, error) }
              }
            })(),
          ])
          return { entry, status, config }
        }),
      )
      if (!cancelled) setState({ kind: 'ready', entries, readings })
    })()
    return () => {
      cancelled = true
    }
  }, [attempt])

  return { state, retry: useCallback(() => setAttempt((n) => n + 1), []) }
}