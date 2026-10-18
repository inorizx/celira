/**
 * **房间的用户侧偏好：置顶 / 免打扰**（2026-09-25 立）。
 *
 * ## 为什么它们住在本机、不进 Core
 *
 * 传统 IM 里置顶与免打扰都是**用户自己的看法**，不是公共事实：
 * 我把这个会话置顶了、我不想被它打扰——这两句的主语都是「我」，对方看不见、也不该看见。
 * 本仓已经有两条同类的先例，本模块与它们**同一档**：
 *
 * | 偏好 | 存哪 | 权威位置 |
 * |---|---|---|
 * | 聊天壁纸 | 机器级键 `celira.machine:wallpaper` | `lib/wallpaper.ts` |
 * | 通知开关 | 机器级键 `celira.machine:notifications` | `hooks/use-notifications.ts` |
 * | **置顶 / 免打扰** | 世界级键 `celira.world:<世界>:room-prefs` | **本模块** |
 *
 * ⚠ **这不是"Core 没端点所以做不了"** —— 那个结论是错的，而且已经错过一次：
 * 曾经这两个开关挂在 `ChatApi.togglePinned` / `toggleMuted` 上（两个**零实现**的方法，
 * 只服务早已不在工作树里的 `api/mock.ts`），于是拨了没反应、自己弹回去。
 * 正确的读法是：**它们本来就不该问 Core**。用户侧偏好归用户侧存储，
 * 既不需要 im 服务端端点，也不需要任何契约解冻。
 *
 * ## 作用域：按**世界**分键（2026-09-29 起并入统一命名空间）
 *
 * 房间 id 是世界内的标识（换一个 core 服务器就是另一个世界），所以偏好也按世界分。
 * 调用方把 `worldKey`（`api/worldIdentity.ts` 的 `worldKeyOf`）传进来拼键——
 * 本模块**不认识世界身份是怎么算出来的**，只接受一个字符串。
 *
 * 键的形状由 `roomPrefsKey` 唯一决定，而**命名空间的形状只在本文件里拼**：
 * 本模块不 import `worldIdentity.ts`（那条边界的理由是"用户侧偏好不许被接回传输层"，
 * 见下方纪律），所以两处拼的是同一个前缀字面量 `celira.world`，
 * 由 `tests/world-identity.test.ts` 交叉核对，改动时两边一起改。
 *
 * ## 纪律（与 `wallpaper.ts` / `prefs.ts` 逐字同款）
 *
 * - **纯逻辑 + 一个存储依赖**：只 import 存储窄口（`./storage.ts`），**绝不 import 任何世界侧
 *   模块**（`store` / `core` / `contracts` / `client`）—— 一旦 import 了，就说明有人又想把它
 *   接回传输层（那正是原病灶）。界面投电影射（`applyRoomPrefs`）与解析都是纯函数，
 *   可在 `node --test` 的类型剥离模式下直测；机械守卫在 `tests/room-prefs.test.ts`；
 * - **宽容解析**：读不懂、缺字段、类型不符一律归默认值，绝不抛错、绝不返回半个对象；
 * - **读写都走窄口**（2026-10-08 存储收尾第二刀）：从前本模块自己同步读 `globalThis.localStorage`
 *   —— 那是"文件存储接不进来"的最后一处（壳的 webview 不是家）。现在它与别处一样走
 *   `api/storage.ts`：壳里落 `<app_config_dir>/local-store.json`，浏览器里仍是 `localStorage`，
 *   而**首帧要有值**这件事由 `store.ts` 的补水负责（`hydrateLocalState`，挂载之前）；
 * - **写失败不静默**：`saveRoomPrefs` **返回是否落盘**（`false` = 写不进去）——
 *   由调用方决定怎么说，绝不吞成"已保存"。
 */

import { readThrough, storage } from './storage.ts'

/** 世界级键的命名空间前缀。**形状权威**在 `api/worldIdentity.ts`（本模块零 import，故此处重写字面量）。 */
const WORLD_NAMESPACE = 'celira.world'

/** 本模块在命名空间之内的键名。 */
const KEY_NAME = 'room-prefs'

/**
 * 拼出某个世界的存储键：`celira.world:<worldKey>:room-prefs`。
 *
 * `worldKey` 空串（还不知道是哪个世界：离线 / 演示 / 服务端没有这一面）时，作用域那一段
 * 为空 —— 与 `worldIdentity.ts` "拿不到世界身份就退回按凭据分键" 是同一条纪律：
 * **键的形状变了，行为不变**。此时键恰好是 `celira.world::room-prefs`，仍是
 * `celira.world` 命名空间下的键，所以"登出清整族"那条判据不会漏掉它。
 */
export function roomPrefsKey(worldKey: string): string {
  const trimmed = String(worldKey ?? '').trim()
  return `${WORLD_NAMESPACE}:${trimmed}:${KEY_NAME}`
}

export interface RoomPrefs {
  /** 置顶的房间 id。 */
  pinned: ReadonlySet<string>
  /** 免打扰的房间 id。 */
  muted: ReadonlySet<string>
}

export const EMPTY_ROOM_PREFS: RoomPrefs = { pinned: new Set(), muted: new Set() }

/** 一个 id 集合的宽容解析：只收非空字符串，其余一律丢掉（不抛）。 */
function cleanIdSet(value: unknown): Set<string> {
  const out = new Set<string>()
  if (!Array.isArray(value)) return out
  for (const item of value) {
    if (typeof item === 'string' && item) out.add(item)
  }
  return out
}

/**
 * 解析 persisted 偏好。**宽容**：任何读不懂的内容都归默认值。
 *
 * 存的是**两个 id 数组**（不是 Set —— JSON 没有 Set）；读回来再变成 Set。
 */
export function parseRoomPrefs(raw: string | null | undefined): RoomPrefs {
  if (!raw) return { pinned: new Set(), muted: new Set() }
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return { pinned: new Set(), muted: new Set() }
  }
  if (typeof parsed !== 'object' || parsed === null) return { pinned: new Set(), muted: new Set() }
  const record = parsed as { pinned?: unknown; muted?: unknown }
  return { pinned: cleanIdSet(record.pinned), muted: cleanIdSet(record.muted) }
}

export function serializeRoomPrefs(prefs: RoomPrefs): string {
  return JSON.stringify({ pinned: [...prefs.pinned], muted: [...prefs.muted] })
}

/**
 * 读出某个世界的偏好（走**存储窄口**；读不到 ⇒ 归默认，不抛）。
 *
 * 它是**异步**的（窄口从第一天起就是异步的：`localStorage` 同步、文件不同步），
 * 所以调用方必须在渲染**之前**补一次水，见 `store.ts` 的 `hydrateLocalState`。
 * 顺带做**读穿式迁移**（老位置还留着这一格时搬进窄口）—— 判据在 `storage.ts`，一处。
 */
export async function loadRoomPrefs(worldKey: string): Promise<RoomPrefs> {
  return parseRoomPrefs(await readThrough(roomPrefsKey(worldKey)))
}

/**
 * 写入某个世界的偏好（同一个窄口）。
 *
 * **返回是否落盘**（`false` = 写不进去：隐私模式、配额满、壳那头失败）。
 * 调用方**不许把 `false` 吞成"已保存"**——这一格的用户可见反馈归调用方决定。
 */
export async function saveRoomPrefs(worldKey: string, prefs: RoomPrefs): Promise<boolean> {
  return storage.write(roomPrefsKey(worldKey), serializeRoomPrefs(prefs))
}

/** 翻转一个 id 在集合里的存在性（返回**新集合**，不改入参）。 */
export function toggleId(set: ReadonlySet<string>, id: string): Set<string> {
  const next = new Set(set)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  return next
}

/** 这份偏好里**一个 id 都没有**（= 用户还没对任何房间表过态）。 */
export function isEmptyRoomPrefs(prefs: RoomPrefs): boolean {
  return prefs.pinned.size === 0 && prefs.muted.size === 0
}

/**
 * 把偏好投影到房间列表上（**纯函数**，不改入参、不写任何存储）。
 *
 * 这是"列表该怎么排、徽标该怎么画"的**唯一判据**：`pinned` / `muted` 两个字段
 * 在真实世界的**唯一写入者**就是这里（其余地方一律只读）。
 *
 * ## 一条不许松的判据：**空偏好 = 没意见，不是"全部取消"**
 *
 * 偏好整体为空时，本函数**一个字段都不碰**（原样返回同一个对象）。
 * 为什么这条必须有：演示通道的 `rooms` 自带 `pinned` / `muted`（fixtures 数据里就有），
 * 而那时用户的偏好是空的 —— 若不判空就投影，`prefs.pinned.has(id)` 算出 `false`，
 * 会把 fixtures 那两个值**抹成 undefined**，演示里"已置顶"的会话当场失去标记。
 *
 * ⚠ 这条曾经写错过一次（`has(id) || undefined` 直接投影）：当时正是上面那条用例
 * 把它抓出来的 —— **测试比实现先想对了语义**。改实现，不改断言。
 *
 * 偏好**非空**时它就是这一格的权威：不在集合里就是"没置顶"（`undefined`），
 * 不再沿用 fixtures 的初始值 —— 与传统 IM 一致：置顶是我的看法，我说了算。
 */
export function applyRoomPrefs<T extends { id: string; pinned?: boolean; muted?: boolean }>(
  rooms: readonly T[],
  prefs: RoomPrefs,
): T[] {
  if (isEmptyRoomPrefs(prefs)) return [...rooms]
  return rooms.map((room) => {
    const pinned = prefs.pinned.has(room.id) || undefined
    const muted = prefs.muted.has(room.id) || undefined
    // 值没变就**原样返回同一个对象**（避免无意义的引用变化引发重渲染）。
    if (room.pinned === pinned && room.muted === muted) return room
    return { ...room, pinned, muted }
  })
}
