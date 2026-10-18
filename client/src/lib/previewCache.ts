/**
 * **有上限的预览缓存**（2026-10-08 刀）：附件预览用的 object URL 复用那一层。
 *
 * ## 它是什么
 *
 * 一块**会话内的内存**：键 → 值 ＋ 占多少字节，**两头都有上限**（条数与总字节），
 * 超了就按"最久没用过的先走"（LRU）逐出，逐出时调 [`dispose`] 把 object URL 撤销掉。
 *
 * ## 它不是什么（这一层最容易越界的地方）
 *
 * - **不是第二份权威**：它存的只是"这一批字节已经取回来过一次"这件事，随时可以整个丢掉，
 *   丢了下次照旧去取真的那份（家里那份）；
 * - **不落盘**：不碰 IndexedDB / 文件系统 —— 刷新即空。落盘的附件缓存是**另一件事**，
 *   要在"家里已经存着字节"的前提下再存一份、并配一整套配额与失效策略，
 *   2026-10-08 判定**有意不做**（理由登记在 `client/README.md`）；
 * - **不自己造值**：怎么造（`URL.createObjectURL`）与怎么销毁（`revokeObjectURL`）都由调用方
 *   注入 —— 这一层只管"留着几个、什么时候扔"，于是它可以脱离浏览器直测。
 *
 * ## 为什么必须有上限
 *
 * 没有上限的预览缓存就是内存泄漏：视频一个 16 MiB，在一个热闹的会话里滚十分钟就能把
 * 标签页撑爆。所以这里**两头都卡**：条数（小图很多时兜底）与字节数（大视频时兜底）。
 */

export interface PreviewCacheOptions<T> {
  /** 最多留几条。 */
  maxEntries: number
  /** 最多留多少字节（按值的大小累加）。 */
  maxBytes: number
  /** 逐出/清空/覆盖时怎么把值还回去（浏览器里就是 `URL.revokeObjectURL`）。 */
  dispose: (value: T) => void
}

export interface PreviewCache<T> {
  /** 取一个。**命中会刷新它的"最近用过"位置**。 */
  get(key: string): T | null
  /** 放一个进去（同键覆盖：旧的先 dispose）。 */
  set(key: string, value: T, bytes: number): void
  /** 主动丢掉一个（例如那条消息被撤回了）。 */
  delete(key: string): void
  /** 全清（换世界/登出时用）。 */
  clear(): void
  /** 现在留了几条 / 多少字节（给测试与读数用）。 */
  stats(): { entries: number; bytes: number }
}

export function createPreviewCache<T>(options: PreviewCacheOptions<T>): PreviewCache<T> {
  const { maxEntries, maxBytes, dispose } = options
  /** `Map` 的插入顺序就是 LRU 顺序：命中时删了再插，等于"挪到最新"。 */
  const entries = new Map<string, { value: T; bytes: number }>()
  let bytes = 0

  const drop = (key: string) => {
    const hit = entries.get(key)
    if (!hit) return
    entries.delete(key)
    bytes -= hit.bytes
    dispose(hit.value)
  }

  const evictToFit = () => {
    // 先按条数、再按字节数，直到两头都不超（`maxEntries` 至少留 1，否则永远放不进去）。
    while (entries.size > Math.max(1, maxEntries) || bytes > maxBytes) {
      const oldest = entries.keys().next()
      if (oldest.done) break
      // 只剩一条时别再逐出自己（它是刚放进来的那一条）。
      if (entries.size === 1) break
      drop(oldest.value)
    }
  }

  return {
    get(key) {
      const hit = entries.get(key)
      if (!hit) return null
      entries.delete(key)
      entries.set(key, hit)
      return hit.value
    },
    set(key, value, size) {
      if (entries.has(key)) drop(key)
      entries.set(key, { value, bytes: Math.max(0, size) })
      bytes += Math.max(0, size)
      evictToFit()
    },
    delete(key) {
      drop(key)
    },
    clear() {
      for (const key of [...entries.keys()]) drop(key)
    },
    stats() {
      return { entries: entries.size, bytes }
    },
  }
}
