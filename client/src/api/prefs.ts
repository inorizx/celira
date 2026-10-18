/**
 * 连接偏好（`celira.connection.prefs`）—— client 正门状态的唯一持久面。
 *
 * 纪律：
 * - 只存「最近一次的选择」：模式、token、服务器地址。
 * - **新字段一律带默认值**：`serverUrl` 空串 = 同源（走页面所在来源的 `/v1`
 *   转发）。旧 localStorage 里没有这个字段时解析结果就是空串，无需迁移。
 * - 未知/旧 `mode` 一律归一到 `core`（**TASK_63 起**）：Host 全托管路径已退役，
 *   旧数据里存的 `'host'` 是历史残留，读进来就是 core。**这不是 fail-open**——
 *   真正拦 fixtures 的是下面 `isRealWorldSession` 的凭据判据，不是这个字段。
 *
 * 本模块零运行期依赖（不 import 任何别的模块、不碰 window），因此可以在
 * `node --test` 的类型剥离模式下直接验。
 */

export const PREFS_KEY = 'celira.connection.prefs'

/**
 * 传输模式。**只有一个取值是当前事实**，不是"留个口子"：
 *
 * 保留这个类型与字段，是为了让「模式」仍然只有**一个权威位置**（本模块），
 * 并把旧持久值收敛到它。它不再参与任何真实分流——页面里的 `mode === 'core'`
 * 只用于区分演示通道与真实会话，而那条判据的权威位置是 `isRealWorldSession`。
 */
export type ConnectionMode = 'core'

/**
 * **这个会话是不是「真实世界」**（有真实数据在，fixtures 一律不许露面）。
 *
 * ---
 * ## 判据（TASK_56 立，TASK_63 收窄）
 *
 * 真实世界曾经有**两条**入口：本机 Host 会话、core 直连。而**演示通道也活在
 * `mode === 'core'` 里** —— 登录页那句「不连接，先看演示界面」走的是
 * `LoginGate.browseDemo()`：
 *
 * ```ts
 * setPrefs('core', '')      // ← 演示态的 mode 也是 'core'
 * navigate('/gallery')
 * ```
 *
 * 所以只判 `mode` 会**两头都错**：
 * - 把 `mode === 'host'` 当成「唯一的真实世界」⇒ core 直连（也是真实世界）
 *   不在判据里 ⇒ 真实用户看到 fixtures 假居民、点进不存在的房间（TASK_56 的真 bug）；
 * - 反过来把 `mode === 'core'` 一律当真实世界 ⇒ **演示通道被一起掐掉**。
 *
 * ## 收窄之后的判据
 *
 * Host 线退役（TASK_63）⇒ 只剩 core 一支，判据退化成**「手里有没有真实凭据」**：
 * `browseDemo()` 明确把 token 清空，而真正连过的人 token 一定在。于是
 * 「core + 空 token」就是演示通道。
 *
 * ⚠ **一处后效，如实记在这里**：退出登录后 prefs 被重置成 `{ mode: 'core',
 * token: '' }`，这个形状与 `browseDemo()` 完全一致 ⇒ 登出态在判据眼里就是
 * 演示通道（fixtures 页放行）。登出后第一屏是登录页，所以用户看不到差别；
 * 差别只在他从登出态深链接进 fixture 驱动页时。
 *
 * `mode` 参数**保留在签名里**（调用方零改动）：它不是判据，只是让调用点不必
 * 为了这次收窄而改写。哪天确认没人再传 `mode` 了，可以再收。
 */
export function isRealWorldSession(input: { mode: ConnectionMode; token: string }): boolean {
  return input.token !== ''
}

export interface ConnectionPrefs {
  mode: ConnectionMode
  token: string
  /** 直连 Core 的服务器地址；空串 = 同源（页面自身来源的 `/v1`）。 */
  serverUrl: string
  /**
   * **这个连接的世界的身份**（`GET /v1/home` 的 `home_id`；空串 = 还不知道）。
   *
   * 为什么它在这里：它是**世界级持久事实的归档键**（事件 cursor / 系统消息 / 房间缓存
   * 都按它分键，见 `api/worldIdentity.ts`）。凭据（token）回答"我是谁"，它回答
   * "我在哪个世界"——两件事，两个格子。**连上之前不知道**（要问一句），所以空串是常态。
   */
  worldId: string
}

/** 首访 / 读不到 prefs 时的缺省：不预设任何服务器。
 *
 * 它同时就是**演示通道**的形状（`core` + 空 token）——第一屏归谁由
 * `api/entry.ts` 按"手里有没有凭据"判：没有 ⇒ 登录页。启动探测已随 host 模块
 * 删除（TASK_64），所以这里不再"交给探测决定"。 */
export const DEFAULT_PREFS: ConnectionPrefs = { mode: 'core', token: '', serverUrl: '', worldId: '' }

/**
 * 解析 persisted prefs。**宽容**：任何缺字段、类型不符或读不懂的内容都归一成
 * 该字段的默认值，绝不抛错、绝不返回半个对象（旧数据必须开箱可用）。
 */
export function parsePrefs(raw: string | null | undefined): ConnectionPrefs {
  if (!raw) return { ...DEFAULT_PREFS }
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return { ...DEFAULT_PREFS }
  }
  if (typeof parsed !== 'object' || parsed === null) return { ...DEFAULT_PREFS }
  const record = parsed as { token?: unknown; serverUrl?: unknown; worldId?: unknown }
  return {
    // 旧数据里的 `'host'`／未知取值一律归一为 core（唯一取值，见 `ConnectionMode`）。
    mode: 'core',
    token: typeof record.token === 'string' ? record.token : '',
    serverUrl: typeof record.serverUrl === 'string' ? record.serverUrl : '',
    // 旧数据里没有这一格 ⇒ 空串 = "还不知道"，一切按凭据分键（与从前一字不差）。
    worldId: typeof record.worldId === 'string' ? record.worldId : '',
  }
}

export function serializePrefs(prefs: ConnectionPrefs): string {
  return JSON.stringify(prefs)
}

/**
 * 归一化服务器地址：去空白与尾斜杠；没写 scheme 时补 `http://`
 * （用户多半只会敲 `127.0.0.1:8080`）。空串保持空串 = 同源。
 */
export function normalizeServerUrl(input: string): string {
  const trimmed = input.trim()
  if (!trimmed) return ''
  const withScheme = /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(trimmed) ? trimmed : `http://${trimmed}`
  const stripped = withScheme.replace(/\/+$/, '')
  // 只剩 scheme 时（如 `http://`）不把 `//` 也削掉，否则会被改造成
  // `http://http:` 这种「能解析但根本不是它」的假地址——留给校验如实报错。
  return /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\/.+$/.test(stripped) ? stripped : withScheme
}

/**
 * 地址校验：空串合法（= 同源）；否则必须是可解析的 http/https 绝对地址。
 * 返回给用户看的一句话，或 null 表示通过。
 */
export function serverUrlError(input: string): string | null {
  const normalized = normalizeServerUrl(input)
  if (!normalized) return null
  let url: URL
  try {
    url = new URL(normalized)
  } catch {
    return '服务器地址读不懂，示例：http://127.0.0.1:8080'
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    return '服务器地址只支持 http/https'
  }
  return null
}

// ⚠ **读穿式迁移的判据不在这里**（2026-10-08 存储收尾第二刀搬走）：它现在是
// `api/storage.ts` 的 `preferStoredRaw` ＋ `readThrough` —— 因为要按同一条判据搬的
// 不止连接偏好这一族（还有主题/壁纸/草稿/房间偏好），而"老位置（webview 的 `localStorage`）
// 只被一处读"这条纪律要求它住在窄口那一层。本模块回到它文件头写的样子：零运行期依赖。
