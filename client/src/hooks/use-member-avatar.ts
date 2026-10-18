import { useEffect, useState } from 'react'
import { getApi } from '@/api/store'

/**
 * 某个成员在**这一间房**里的头像 URL（`CORE_V1_AVATAR_P0.md` §3.4）。
 *
 * 契约只给这一条读取面：调用者与目标都必须是该 Room 的当前 active 成员。
 * 所以这个 hook **必须要房间上下文** —— 联系人页那种没有房间的地方拿不到，
 * 也不该拿（不发明第二条读取路径）。
 *
 * 读取策略：认证是 `Authorization: Bearer` 头，**`<img>` 带不了它** ⇒ 只能
 * `fetch` 回字节再 `URL.createObjectURL`，配一层内存缓存。
 *
 * 返回值只有两种意思：`string` = 可以画画看的 URL；`null` = 现在没有可画的字节
 * （没设头像 / 不是成员 / 还没取回来）。**"没设头像"是常态**，调用方照常画 fallback。
 */

/**
 * core 线的 blob URL 缓存：`roomId:participantId` → objectURL 或 `null`。
 *
 * - 存 `null` 也是缓存：**"确定没有头像"要记住**，否则每次渲染都会再打一次 404。
 * - 生命周期 = 页面。头像数量受房间成员数约束，且同一个成员在多个位置复用同一个
 *   URL（比每处各建一个 objectURL 省得多）；页面关闭时随进程回收。
 */
const coreAvatarUrls = new Map<string, string | null>()

/** 同一头像的并发请求合并：一屏里多个位置同时挂载时只发一次。 */
const coreAvatarInFlight = new Map<string, Promise<string | null>>()

function loadCoreAvatar(roomId: string, participantId: string): Promise<string | null> {
  const key = `${roomId}:${participantId}`
  const cached = coreAvatarUrls.get(key)
  if (cached !== undefined) return Promise.resolve(cached)
  const pending = coreAvatarInFlight.get(key)
  if (pending) return pending
  const request = (async () => {
    try {
      const api = getApi()
      if (!api?.fetchMemberAvatarBlob) throw new Error('no avatar path')
      const blob = await api.fetchMemberAvatarBlob(roomId, participantId)
      const url = URL.createObjectURL(blob)
      coreAvatarUrls.set(key, url)
      return url
    } catch {
      // 404（无头像 / 非成员 / 跨房间）与网络失败在展示上同一个结果：画 fallback。
      coreAvatarUrls.set(key, null)
      return null
    } finally {
      coreAvatarInFlight.delete(key)
    }
  })()
  coreAvatarInFlight.set(key, request)
  return request
}

export function useMemberAvatarUrl(
  roomId: string | null | undefined,
  participantId: string | null | undefined,
): string | null {
  const [coreUrl, setCoreUrl] = useState<string | null>(null)

  useEffect(() => {
    if (!roomId || !participantId) {
      setCoreUrl(null)
      return
    }
    let cancelled = false
    void loadCoreAvatar(roomId, participantId).then((url) => {
      if (!cancelled) setCoreUrl(url)
    })
    return () => {
      cancelled = true
    }
  }, [roomId, participantId])

  // fetch 回来的 blob URL（可能还没到，也可能确定没有）。
  return coreUrl
}
