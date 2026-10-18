// ⚠ 本模块**要被 `node --test` 直接测**（`tests/hidden-messages.test.ts`）⇒ **零 import**
//   （与 `roomPrefs.ts` 同款：命名空间字面量在这里重写一遍，形状权威仍在 `api/worldIdentity.ts`）。

/**
 * **本机隐藏的消息**（用户按的「删除 · 仅为我」）。
 *
 * ## 它是什么、不是什么
 *
 * 它**不是公共事实**：世界里那条消息**一动没动**（那是撤回，归 Core）。这里只是**这台机器上
 * 这个人的视图** —— 记下一串 id，渲染时滤掉。
 *
 * ## 三条代价（如实登记，也写进确认框）
 *
 * 1. **只在你这边隐藏**：对面照旧看得见；
 * 2. **不是删除**：换台机器、清了本地存储、重装之后，它照样回来；
 * 3. **不产生任何公共事件**：别的端不会跟着变（所以也没有"同步到我的其他设备"这回事）。
 *
 * ## 作用域
 *
 * 与 `roomPrefs` 同族：`celira.world:<worldKey>:hidden:<roomId>` —— **世界级 ＋ 房间级**。
 * 判据同 `worldIdentity.ts`：换个世界连，这份名单该不该跟着走？不该（房间 id 属于那个世界）。
 */

const WORLD_NAMESPACE = 'celira.world'

/** 某个世界里某一间房的隐藏名单键。 */
export function hiddenKey(worldKey: string, roomId: string): string {
  const world = String(worldKey ?? '').trim()
  const room = String(roomId ?? '').trim()
  return `${WORLD_NAMESPACE}:${world}:hidden:${room}`
}

/**
 * 宽容解析：读不懂一律归空名单（不抛）。只收非空字符串、并**去重**。
 *
 * 存的是**数组**（JSON 没有 Set），读回来仍是数组 —— 名单很短（一间房里人删掉的条数），
 * 不值得为它引入 Set 与它那一圈等价性判断。
 */
export function parseHidden(raw: string | null | undefined): string[] {
  if (!raw) return []
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return []
  }
  if (!Array.isArray(parsed)) return []
  const out: string[] = []
  for (const item of parsed) {
    if (typeof item === 'string' && item !== '' && !out.includes(item)) out.push(item)
  }
  return out
}

/** 序列化（给存储层用）。 */
export function serializeHidden(list: readonly string[]): string {
  return JSON.stringify(list)
}

/** 加一条（**幂等**：已在里面就原样返回）。 */
export function addHidden(list: readonly string[], id: string): string[] {
  if (!id || list.includes(id)) return [...list]
  return [...list, id]
}

/** 去掉一条（幂等）。留给"恢复显示 / 清空名单"那条路。 */
export function removeHidden(list: readonly string[], id: string): string[] {
  return list.filter((each) => each !== id)
}

/** 这条消息在这一间房里被隐藏了吗。 */
export function isHidden(list: readonly string[], id: string): boolean {
  return Boolean(id) && list.includes(id)
}
