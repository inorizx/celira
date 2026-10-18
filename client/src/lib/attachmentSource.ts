import { useEffect, useState } from 'react'
import type { Attachment } from '@/api/domain'
import { getApi } from '@/api/store'
import { createPreviewCache } from './previewCache'

/**
 * 附件可播放/可预览源：`localUrl` 与 `/mock/` 直出；真实会话经鉴权 fetch 拉 Blob。
 *
 * ## 为什么有一层缓存（2026-10-08 刀）
 *
 * 从前的写法是"挂载就取、卸载就 `revokeObjectURL`" ⇒ **滚回去就是重新下一遍**，而且中间那一下
 * 是空白的（图会闪）。图片/语音/视频三条路都走这个 hook，所以那一下是全局的。
 *
 * 现在：取回来的 object URL 进一块**有上限的会话内缓存**（[`createPreviewCache`]，条数与字节数
 * 两头都卡，LRU 逐出时撤销 URL），于是
 *
 * - 滚回去**不再重下**，也不再闪（命中是**同步**的：初值就从缓存里拿）；
 * - 同一份附件被两处同时要（气泡 ＋ 灯箱）**只取一次**（同键 in-flight 去重）；
 * - 它**不是第二份权威**：同一批字节、随时可逐出、**不落盘**，刷新即空。
 *
 * ## 不做什么（有意）
 *
 * **不做落盘的附件缓存**：家里已经存着字节，再往磁盘存一份就要配一整套配额、逐出与
 * "那条消息被撤回了怎么办"的失效策略，而浏览器形态本来还有 HTTP 缓存兜着。
 * 这条取舍登记在 `client/README.md`。
 */

/** 条数上限（小图很多时兜底）。 */
const MAX_ENTRIES = 40
/** 字节上限（大视频时兜底；单个附件上限 16 MiB，所以这里能容下几个视频）。 */
const MAX_BYTES = 64 * 1024 * 1024

const cache = createPreviewCache<{ url: string }>({
  maxEntries: MAX_ENTRIES,
  maxBytes: MAX_BYTES,
  dispose: (value) => URL.revokeObjectURL(value.url),
})

/** 同键只取一次的登记表（两个组件同时要同一张图时不许下两遍）。 */
const inFlight = new Map<string, Promise<string>>()

/** 缓存键：房间 ＋ 附件 id（附件 id 已经全局唯一，带上房间是为了读起来不歧义）。 */
function keyOf(roomId: string, attachmentId: string): string {
  return `${roomId}:${attachmentId}`
}

/**
 * 清空预览缓存（**换世界 / 登出时调**）：那些字节属于上一个身份看得见的东西，
 * 留在内存里没有用处，撤销 URL 也顺手把内存还回去。
 */
export function clearAttachmentPreviewCache(): void {
  cache.clear()
  inFlight.clear()
}

/** 取一个附件的预览源（命中同步、未命中取一次、同键去重）。 */
async function previewUrlFor(roomId: string, attachment: Attachment): Promise<string> {
  const key = keyOf(roomId, attachment.id)
  const hit = cache.get(key)
  if (hit) return hit.url
  const running = inFlight.get(key)
  if (running) return running
  const task = (async () => {
    const api = getApi()
    if (!api?.fetchAttachmentBlob) throw new Error('这条连接没有字节通道')
    const blob = await api.fetchAttachmentBlob(roomId, attachment.id)
    const url = URL.createObjectURL(blob)
    cache.set(key, { url }, blob.size)
    return url
  })().finally(() => inFlight.delete(key))
  inFlight.set(key, task)
  return task
}

export function useAttachmentSource(attachment: Attachment, roomId: string): {
  src: string | null
  failed: boolean
} {
  const direct =
    attachment.localUrl ?? (attachment.url?.startsWith('/mock/') ? attachment.url : null)
  // ⚠ 初值**先看缓存**：命中时这一格第一帧就有值 ⇒ 滚回去不会先空一下再跳出来。
  const [src, setSrc] = useState<string | null>(
    () => direct ?? cache.get(keyOf(roomId, attachment.id))?.url ?? null,
  )
  const [failed, setFailed] = useState(false)

  useEffect(() => {
    let cancelled = false
    if (direct) {
      setSrc(direct)
      setFailed(false)
      return
    }
    // 已经有源（缓存命中或上一次已经取回来）⇒ 不必再问一次。
    const cached = cache.get(keyOf(roomId, attachment.id))
    if (cached) {
      setSrc(cached.url)
      setFailed(false)
      return
    }
    const api = getApi()
    // ⚠ 原判据 `api?.mode === 'core' && …` —— 恒真，实际是**空值守卫**（审计 D18）。
    if (!api || !attachment.url || !api.fetchAttachmentBlob) return
    void (async () => {
      try {
        const url = await previewUrlFor(roomId, attachment)
        if (cancelled) return
        setSrc(url)
        setFailed(false)
      } catch {
        if (!cancelled) setFailed(true)
      }
    })()
    return () => {
      // ⚠ **不撤销 URL**：它还留在缓存里给下一次用（撤销只发生在逐出/清空时）。
      cancelled = true
    }
  }, [attachment.id, attachment.url, attachment.localUrl, roomId, direct])

  return { src, failed }
}
