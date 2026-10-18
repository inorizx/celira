/**
 * **本机隐藏的消息**（用户按的「删除 · 仅为我」）—— 存取那一半。
 *
 * 纯逻辑（解析 / 加 / 去 / 判据 / 键的形状）在 `lib/hiddenMessages.ts`，那边零 import、可
 * `node --test` 直测；**这里只做"读一个字符串、写一个字符串"**，以及把它挂到**世界作用域**上。
 *
 * ## 为什么是世界作用域
 *
 * 判据与 `api/worldIdentity.ts` 同款：换一个世界连，这份名单该不该跟着走？**不该** ——
 * 房间 id 属于那个世界，跟着走只会把 A 世界的隐藏名单套到 B 世界的同一间房上。
 * 键形与 `roomPrefs` 同族（`celira.world:<worldKey>:…`），所以"登出清整族"那条判据不会漏掉它。
 *
 * ## 它是视图，不是事实
 *
 * 世界里那条消息**一动没动**（那是撤回，归 Core）。这里失手最多是"名单读不出来" ——
 * 那按**空名单**处理（= 什么都不隐藏），**绝不**因为一份坏数据让一整间房渲染不出来。
 */
import { hiddenKey, parseHidden, serializeHidden } from '@/lib/hiddenMessages'
import { parsePrefs, PREFS_KEY } from './prefs'
import { storage } from './storage'
import { worldKeyOf } from './worldIdentity'

/** 当前连接所属世界的 key（读连接偏好 —— 与 `store.ts` 的 `worldKey()` 同一判据）。 */
async function currentWorldKey(): Promise<string> {
  const prefs = parsePrefs(await storage.read(PREFS_KEY))
  return worldKeyOf(prefs.worldId, prefs.token)
}

/** 读这一间房的隐藏名单。读不到/读不懂 ⇒ 空名单（**不抛**）。 */
export async function loadHiddenMessages(roomId: string): Promise<string[]> {
  if (!roomId) return []
  return parseHidden(await storage.read(hiddenKey(await currentWorldKey(), roomId)))
}

/** 写这一间房的隐藏名单。**写不进去返回 `false`**（调用方要让用户看见"没保存住"）。 */
export async function saveHiddenMessages(roomId: string, ids: readonly string[]): Promise<boolean> {
  if (!roomId) return false
  return storage.write(hiddenKey(await currentWorldKey(), roomId), serializeHidden(ids))
}
