/**
 * **世界的身份**（`GET /v1/home` 的 `home_id`）—— 世界级持久事实的**归档键**。
 *
 * ## ⚠ 先读这条：本模块的 "home" 与 `api/homes.ts` 的 "home" 不是一回事
 *
 * 仓库里「Home」与「家」长期撞词，`docs/constitution/PRODUCT_VISION.md` 的「一词一主」
 * 已经裁过一次（**只在那里解释，别处不重复解释**）。落到前端这两个模块上：
 *
 * | 模块 | 它装的是裁决表里的哪一侧 | 权威在哪 |
 * |---|---|---|
 * | **本模块**（`home_id` → `worldId`） | **世界**：那个逻辑 im 服务端与它裁决的公共事实。契约语境里它写作专名 `Home`，本模块把它读作**世界** | im 服务端 `GET /v1/home` |
 * | `api/homes.ts`（`HomeEntry` / 家钥匙） | **家**：**执行端机器**（`home.js` 管的那一整套本机环境） | 本机 IndexedDB + 那台机器的状态口 |
 *
 * 两者**没有从属关系**：一个世界可以有 N 台「家」（执行点），一台「家」也可以被
 * 换去连别的世界。所以本模块**不 import `homes.ts`**，`homes.ts` 也不 import 本模块 ——
 * 两个事实、两套存储、两套凭据，撞的只是名字。
 *
 * ## 为什么需要它
 *
 * 客户端有几样东西是**世界级**的：公开事件流读到哪了（cursor）、系统消息、房间缓存。
 * 它们原来一律按 **Human token** 分键（`celira.events.cursor:<token>`、`celira.sys:<token>`、
 * IndexedDB 里那份 `token` 守卫）。token 是**凭据**，不是**世界**：备份/恢复出来的第二个世界
 * 可能带着同一份库、同一个 token（同一支 Human 的会话），于是两个世界会**互读**房间与消息。
 * 世界级的事实，键就该是**世界的身份**。
 *
 * ## 键的命名空间（2026-09-29 立）
 *
 * 每一个持久键都属于**恰好一个作用域**，作用域写在键名前缀里：
 *
 * | 作用域 | 键的形状 | 例子 | 为什么 |
 * |---|---|---|---|
 * | **世界级** | `celira.world:<世界身份或退回的凭据>:<名字>` | `celira.world:h-1:cursor` | 同一份数据换了世界就该换一份；app 里多世界并存不能互相覆盖 |
 * | **机器级** | `celira.machine:<名字>` | `celira.machine:wallpaper` | 它说的是**这台机器/这个壳**（外观、通知开关），跟世界无关 |
 *
 * **判据一句话**：换一个世界连，这个键该不该跟着变？该 ⇒ 世界级；不该 ⇒ 机器级。
 *
 * ## 退一步的规矩（不许因此把连接搞脆）
 *
 * 世界的身份要问一句才知道（`GET /v1/home`）。**问不到就退回按凭据分键**——老服务器、
 * 演示通道、离线、或者这一面还没实现的实现，行为与从前**一字不差**；键的形状也不冲突
 * （`home_id` 是 UUID，token 是另一套字符串）。
 *
 * 本模块是**策略面**（纯逻辑，零运行期依赖、不碰 window）⇒ 可以在 `node --test` 下直测。
 */

/** 世界级键的命名空间前缀。**唯一权威**在 `worldScopedKey`。 */
export const WORLD_NAMESPACE = 'celira.world'

/** 机器级键的命名空间前缀（外观 / 通知开关这类"属于这个壳"的东西）。 */
export const MACHINE_NAMESPACE = 'celira.machine'

/**
 * 世界级事实的键名（命名空间之内的"名字"）。
 *
 * 它们**不是**完整键——完整键由 `worldScopedKey` 拼；把名字与作用域分开，
 * 是因为"这个名字属于哪个作用域"是一条判据，不该散在每个调用点自己拼。
 */
export const CURSOR_KEY_NAME = 'cursor'
export const SYS_KEY_NAME = 'sys'
export const ROOM_PREFS_KEY_NAME = 'room-prefs'

/**
 * 世界级事实的完整键：`celira.world:<世界身份或退回的凭据>:<名字>`。
 *
 * 空名字是**编程错误**（不是运行期状态）——它会让同一个世界下所有事实挤进一个键，
 * 所以这里明确拒绝，而不是拼出一个看起来正常的键。
 */
export function worldScopedKey(worldKey: string, name: string): string {
  const scope = String(worldKey ?? '').trim()
  const leaf = String(name ?? '').trim()
  if (!leaf) throw new Error('worldScopedKey: 键名不能为空')
  return `${WORLD_NAMESPACE}:${scope}:${leaf}`
}

/** 机器级键的完整键：`celira.machine:<名字>`。同样是空名字即编程错误。 */
export function machineScopedKey(name: string): string {
  const leaf = String(name ?? '').trim()
  if (!leaf) throw new Error('machineScopedKey: 键名不能为空')
  return `${MACHINE_NAMESPACE}:${leaf}`
}

/**
 * 世界级事实归档用的键：**知道世界的身份就用它**，否则退回凭据。
 *
 * 空格子、空串、非字符串一律按"不知道"处理（旧数据里没有这一格）。
 */
export function worldKeyOf(worldId: string | null | undefined, token: string): string {
  const id = typeof worldId === 'string' ? worldId.trim() : ''
  return id !== '' ? id : token
}

/**
 * 从 `GET /v1/home` 的回执里取出世界的身份。**读不懂就是读不懂**（返回 `null`），
 * 绝不拿别的字段顶替、也不猜（与"认不出的形状明确失败"同一条纪律）。
 */
export function parseWorldIdentity(payload: unknown): string | null {
  if (typeof payload !== 'object' || payload === null) return null
  const raw = (payload as { home_id?: unknown }).home_id
  if (typeof raw !== 'string') return null
  const id = raw.trim()
  return id === '' ? null : id
}

/** 世界身份缺席时（老库/离线/演示），这一路的键与从前完全一致。 */
export function isWorldKnown(worldId: string | null | undefined): boolean {
  return typeof worldId === 'string' && worldId.trim() !== ''
}
