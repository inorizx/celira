/**
 * **家条目**（家口远程访问刀）——"这个浏览器记得哪些家、怎么跟它们说话"的**纯逻辑层**。
 *
 * ## ⚠ 先读这条：本模块的 "home" 与 `api/worldIdentity.ts` 的 "home" 不是一回事
 *
 * 仓库里「Home」与「家」长期撞词，`docs/constitution/PRODUCT_VISION.md` 的「一词一主」
 * 已经裁过一次（**只在那里解释，别处不重复解释**）。落到前端这两个模块上：
 *
 * | 模块 | 它装的是裁决表里的哪一侧 | 权威在哪 |
 * |---|---|---|
 * | **本模块**（`HomeEntry` / 家钥匙） | **家**：**执行端机器**（`home.js` 管的那一整套本机环境：Agent 的环境、资料、日志、模型配置） | 本机 IndexedDB + 那台机器的状态口 / 配置门 |
 * | `api/worldIdentity.ts`（`home_id` → `worldId`） | **世界**：那个逻辑 im 服务端与它裁决的公共事实。契约语境里它写作专名 `Home`，那边读作**世界** | im 服务端 `GET /v1/home` |
 *
 * 两者**没有从属关系**：一个世界可以有 N 台「家」（执行点），一台「家」也可以被
 * 换去连别的世界。所以本模块**不 import `worldIdentity.ts`**，那边也不 import 本模块 ——
 * 两个事实、两套存储、两套凭据，撞的只是名字。
 *
 * ⚠ **认领用的 id 来自世界侧**（读数的 `agent.agentId` 是 im 服务端签发的 participant id），
 * 所以本模块要 import `homeStatus.ts` 的形状 —— 那是**形状**依赖，不是"两个 home 合成一个"。
 *
 * 职责一句话：把**存进 IndexedDB 的记录**（`idb.ts` 那层只搬字节）解释成**家条目**，
 * 并按条目把"状态口 / 模型配置门"的两族请求**分到两条路**上。
 *
 * ## 一件事实
 *
 * 每个 Agent 的照看数据住在**一台执行端**上（自己的家）。这台浏览器要去读它、配它，
 * 只有两条路：
 *
 * - **本机条目**（id `'local'`，**预置、钥匙为空**）：走**同源相对路径**（`/home-status`、
 *   `/home-config`），由当前页面的代转送到本机那两个口 —— 行为与今天完全一致，一个字不变；
 * - **远程条目**（用户加的，钥匙必填）：走**绝对地址 + Bearer 头**（`https://…:5190/status`）。
 *
 * 两族数据绝不混流的纪律延续：**分流只发生在请求构造这里** —— 下面那两族读数的
 * 形状守卫与错误分类不在本层（`homeStatus.ts` / `homeConfig.ts` 各自管）。
 *
 * ## 三态匹配
 *
 * 家条目连上后读到的状态口读数自带 `agent.agentId`（`<环境根>/identity.json` 那条既有链路）。
 * 与世界的 Agent 列表按 id **精确匹配** ⇒ 匹配显示住址与读数，不匹配如实「未接入」。
 * 匹配判据就是 `homeStatus.ts` 的 `agentAgentMatch`（三态：同一个人 / 是别人 / 读不到）。
 * **不做任何新协议**。
 *
 * ## 为什么有 `port` 这一格
 *
 * 家条目的形状是 `{ 名字, 家地址, 端口对(默认5190/5191), 家钥匙 }`（任务书 C7）。
 * 端口**缺省**住在 `paths.js` 那份默认值里（5190/5191）—— 用户不填就按默认，
 * 填了就按填的（与 `home.conf` 的可覆盖同一条纪律）。
 *
 * ## 纯逻辑、零运行期依赖
 *
 * 本模块不碰 `fetch`、不碰 `window`：解析 / 校验 / 分流构造全是纯函数，可以被
 * `node --test` 直接钉住。只有**存取**那三行会碰 IndexedDB —— 它们 import
 * `idb.ts` 的窄口（搬字节那层），**形状的解释仍在本层**。
 */

import { deleteHomeEntryRecord, listHomeEntryRecords, putHomeEntryRecord } from './idb.ts'

/** 本机条目的固定 id（"本机"是预置的第一条 —— 任务书 C7）。 */
export const LOCAL_HOME_ID = 'local'

/** 远程条目的 id：`home-<序号>`（单调不回收）。 */
export const REMOTE_HOME_ID_PREFIX = 'home-'

/** 远程条目的钥匙**必填**，否则连不了（本机那条钥匙为空 = 同源代转）。 */
export const HOME_KEY_ERROR = '家钥匙不能为空（在它那台机器上跑 node home.js issue-home-key 拿）'

/**
 * 家条目的**端口对**：缺省住在 `scripts/home/paths.js` 那份默认值（`DEFAULT_STATUS_PORT` /
 * `DEFAULT_CONFIG_PORT`）——与 `home.conf` 的可覆盖同一条纪律：**新字段一律带默认值**。
 */
export const DEFAULT_HOME_PORTS = { status: 5190, config: 5191 }

/**
 * 家地址的**归一化**：去空白与尾斜杠；没写 scheme 时补 `https://`
 * （与登录页那条"地址"同一条省事纪律：用户多半只敲 `192.168.1.20`）。
 * 远程的家只开在 TLS 上 ⇒ 补的是 **https**（不补 http —— 那会给出一个必然连不上的地址）。
 */
export function normalizeHomeAddress(input: string): string {
  const trimmed = String(input ?? '').trim()
  if (!trimmed) return ''
  const withScheme = /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(trimmed) ? trimmed : `https://${trimmed}`
  return withScheme.replace(/\/+$/, '')
}

/** 家地址的校验：必须是 https 绝对地址（模式二只开在 TLS 上）。返回人话或 null。 */
export function homeAddressError(input: string): string | null {
  const value = normalizeHomeAddress(input)
  if (!value) return '家地址不能为空'
  let url: URL
  try {
    url = new URL(value)
  } catch {
    return '家地址读不懂（示例：https://192.168.1.20）'
  }
  if (url.protocol !== 'https:') return '家地址只支持 https（远程的家必须走 TLS）'
  return null
}

/** 端口对校验：可空（用默认）或 1–65535 的正整数。 */
export function homePortError(raw: string): string | null {
  const value = String(raw ?? '').trim()
  if (!value) return null
  if (!/^[0-9]{1,5}$/.test(value)) return '端口得是 1–65535 的数字'
  const n = Number.parseInt(value, 10)
  if (n < 1 || n > 65535) return '端口得在 1–65535 之间'
  return null
}

/** 存进 IndexedDB 的那条**原样记录**（一层薄形状：分流的两个端点都从这里读）。 */
export interface StoredHomeEntry {
  id: string
  name: string
  /** 绝对地址（远程）或空（本机 ⇒ 同源）。 */
  baseUrl: string
  statusPort: number | null
  configPort: number | null
  /** 家钥匙原文（本机那条必为空）。 ⚠ 它就是用户贴进来的那把，原样存原样用。 */
  key: string
}

/** 解释出来、带着缺省的家条目（页面/请求构造用的形状）。 */
export interface HomeEntry {
  id: string
  name: string
  baseUrl: string
  statusPort: number
  configPort: number
  key: string
  /** 是不是本机那条（本机 = 同源代转，远程 = 绝对地址 + Bearer）。 */
  local: boolean
}

/**
 * 解析一条**存着的记录** ⇒ 家条目。**宽容**：缺字段一律按默认值收，
 * 不认识的内容归一成"读不懂"而不是抛出（旧数据必须开箱可用）；
 * 但**形状根本不对**（不是对象）⇒ 丢弃这一条。
 */
export function parseStoredEntry(record: unknown): HomeEntry | null {
  if (typeof record !== 'object' || record === null) return null
  const raw = record as Record<string, unknown>
  const id = typeof raw.id === 'string' && raw.id ? raw.id : ''
  if (!id) return null
  const name = typeof raw.name === 'string' && raw.name.trim() ? raw.name.trim() : '未命名'
  const baseUrl = typeof raw.baseUrl === 'string' ? raw.baseUrl.trim() : ''
  const statusPort =
    typeof raw.statusPort === 'number' && Number.isInteger(raw.statusPort) && raw.statusPort > 0
      ? raw.statusPort
      : DEFAULT_HOME_PORTS.status
  const configPort =
    typeof raw.configPort === 'number' && Number.isInteger(raw.configPort) && raw.configPort > 0
      ? raw.configPort
      : DEFAULT_HOME_PORTS.config
  const key = typeof raw.key === 'string' ? raw.key : ''
  return { id, name, baseUrl, statusPort, configPort, key, local: id === LOCAL_HOME_ID }
}

/** 远程条目的新 id（在现有条目上取 `home-N` 最大 N + 1，保证不撞）。 */
export function nextRemoteId(entries: HomeEntry[]): string {
  let max = 0
  for (const entry of entries) {
    if (!entry.id.startsWith(REMOTE_HOME_ID_PREFIX)) continue
    const n = Number.parseInt(entry.id.slice(REMOTE_HOME_ID_PREFIX.length), 10)
    if (Number.isInteger(n) && n > max) max = n
  }
  return `${REMOTE_HOME_ID_PREFIX}${max + 1}`
}

/**
 * 本机条目（预置的那条）。
 *
 * **两种形状，一条判据**（`PROJECT_SHAPE.md` §9.3 那条纪律：*app 里只有一条路——家条目统一为
 * "地址 + 钥匙"，本机那条只是地址为 loopback 的一条*）：
 *
 * - **web 形态**（默认）：`baseUrl` 空 ⇒ 走**同源相对路径**（`/home-status`、`/home-config`），
 *   由页面所在的那台服务器代转。**同源代转是 web 专属的实现细节**（§9.3 原话），
 *   只在浏览器里成立；
 * - **壳里**（[`setLocalHomeBase`] 装过壳解析出来的 loopback 地址）：`baseUrl` = loopback、
 *   钥匙仍为空（loopback 免钥匙，`home-key.js` 模式一）⇒ 与远程条目**走同一个绝对地址分支**。
 *   壳里没有"页面来源"可解析（`api/shell.ts` 的 `resolveShellUrl`），相对路径在那儿是**响亮失败**。
 */
export function localHomeEntry(): HomeEntry {
  const base = localBase
  return {
    id: LOCAL_HOME_ID,
    name: '本机',
    baseUrl: base ? base.host : '',
    statusPort: base ? base.statusPort : DEFAULT_HOME_PORTS.status,
    configPort: base ? base.configPort : DEFAULT_HOME_PORTS.config,
    key: '',
    local: true,
  }
}

/** 壳解析出来的本机基址（`local_world_readout` 的两格读数；`null` = web 形态）。 */
export interface LocalHomeBase {
  /** loopback 主机（`http://127.0.0.1`）。 */
  host: string
  statusPort: number
  configPort: number
}

let localBase: LocalHomeBase | null = null

/**
 * 装上"本机家的 loopback 基址"（**唯一入口**；壳的桥 `api/shellLocal.ts` 装它）。
 *
 * 为什么要有它：壳里没有代转层，页面拿到的相对路径发不出去（§8.4 推论 2 的原话：
 * *本机地址由 app 解析*）。装上之后本机条目与远程条目走同一条绝对地址分支。
 * `null` = 恢复 web 形态（浏览器里从没装过，所以 web 行为一字不变）。
 */
export function setLocalHomeBase(next: LocalHomeBase | null): void {
  localBase = next
}

/** 当前装着的本机基址（读数用；`null` = 网页形态）。 */
export function localHomeBase(): LocalHomeBase | null {
  return localBase
}

/** 远程条目的**校验**（添加表单用）：地址必须 https、钥匙必须给。端口可空（用默认）。 */
export interface HomeEntryDraft {
  name: string
  baseUrl: string
  statusPort: string
  configPort: string
  key: string
}

export interface HomeEntryDraftError {
  baseUrl?: string
  key?: string
  statusPort?: string
  configPort?: string
}

export function homeEntryDraftErrors(draft: HomeEntryDraft): HomeEntryDraftError {
  const errors: HomeEntryDraftError = {}
  const addressError = homeAddressError(draft.baseUrl)
  if (addressError) errors.baseUrl = addressError
  if (!String(draft.key ?? '').trim()) errors.key = HOME_KEY_ERROR
  const statusError = homePortError(draft.statusPort)
  if (statusError) errors.statusPort = statusError
  const configError = homePortError(draft.configPort)
  if (configError) errors.configPort = configError
  return errors
}

// ---------------------------------------------------------------------------
// 请求构造：两族数据绝不混流（分流只在这里发生一次）
// ---------------------------------------------------------------------------

/**
 * 状态口那一族（`fetchHomeStatus` 用）：**按条目把地址算出来**。
 *
 * - **本机 + 空地址**（web 形态）⇒ `/home-status/status`、`/home-status/healthz` —— 同源代转
 *   **会改名**到口子自己的 `/status`、`/healthz`（那两条路由是口子的契约，代转只做搬运）；
 * - **其余**（远程，以及**壳里的本机**——地址是 loopback）⇒ `http://主机:端口/status`
 *   —— 直连口子，**用它自己的路由名**。
 *
 * ⚠ 两条路的路径**长得不一样**是事实（代理替你改名 vs 直连），不许把一边的写法套到另一边。
 * ⚠ "空地址 ⇒ 同源"这一支**只在浏览器里成立**（§9.3：同源代转是 web 专属实现细节）；
 *   壳里本机那条由 `setLocalHomeBase` 装上地址，于是自动走下面那条绝对分支。
 */
export function statusRequestUrl(entry: HomeEntry, path: '/status' | '/healthz'): string {
  if (entry.local && !entry.baseUrl) return `/home-status${path}`
  return `${stripSlash(entry.baseUrl)}:${entry.statusPort}${path}`
}

/**
 * 模型配置门那一族（`fetchHomeConfig` 用）：同样按条目分流。
 * 两边的路径**同名**（`/home-config` 那几条就是门自己的路由）——本机在 web 形态下只是多一层同源代转。
 */
export function configRequestUrl(entry: HomeEntry, path: string): string {
  if (entry.local && !entry.baseUrl) return path
  return `${stripSlash(entry.baseUrl)}:${entry.configPort}${path}`
}

/** 远程请求要带的头（Bearer 家钥匙）；本机 ⇒ `{}`（同源代转，不带任何头）。 */
export function homeRequestHeaders(entry: HomeEntry): Record<string, string> {
  if (entry.local || !entry.key) return {}
  return { authorization: `Bearer ${entry.key}` }
}

function stripSlash(url: string): string {
  return String(url ?? '').replace(/\/+$/, '')
}

// ---------------------------------------------------------------------------
// 错误分类：固定人话（contractErrors 的纪律：只认稳定分类、绝不回显服务端原文）
// ---------------------------------------------------------------------------

/**
 * 一次请求失败的**稳定分类**（鸭子的样子，由本层的 `HomeRequestError` 或
 * `homeConfig.ts` 的 `HomeConfigError` 生产 —— 两族读数各有各的错误类型，
 * 这里只认那几个**字段**，不引入任何传输层依赖）。
 */
export type HomeRequestFailure = 'unauthorized' | 'network' | 'http' | 'shape' | 'server'

export class HomeRequestError extends Error {
  readonly failure: HomeRequestFailure
  readonly status: number | null

  constructor(failure: HomeRequestFailure, status: number | null = null, message = '') {
    super(message || `home request failed: ${failure}${status === null ? '' : ` (${status})`}`)
    this.name = 'HomeRequestError'
    this.failure = failure
    this.status = status
  }
}

function failureOf(error: unknown): HomeRequestFailure {
  if (typeof error === 'object' && error !== null) {
    const shaped = error as { failure?: unknown; code?: unknown; status?: unknown }
    if (shaped.code === 'unauthorized' || shaped.failure === 'unauthorized') return 'unauthorized'
    if (shaped.failure === 'network') return 'network'
    if (shaped.failure === 'shape') return 'shape'
    if (shaped.status === 401) return 'unauthorized'
    if (typeof shaped.status === 'number') return 'http'
  }
  // `fetch` 的断网/证书/拒绝连接一律是 `TypeError`（浏览器不给更细的因）。
  if (error instanceof TypeError) return 'network'
  return 'server'
}

/**
 * 一次失败的**分类**（固定几种；给测试留一个可判的钩子）：
 *
 * - `key` / `local-key`：钥匙不对（远程条目）／这台机器的口要钥匙而页面没带（本机条目）；
 * - `unreachable`：连不上（本机服务没起 / 远程地址端口不对）；
 * - `tls`：**远程 + https + 连不上** —— 里面可能是地址问题，也可能是证书浏览器不认。
 *   ⚠ 这两件事在浏览器里**没法机械分开**（`fetch` 失败一律给 `TypeError`，没有原因码）
 *   ⇒ 这一档的人话**如实把两种可能都说出来**，不假装分辨得出（诚实高于好看）。
 * - `shape`：答的话读不懂（版本不一致）；
 * - `http`：口子**答了**，但答的是个错误码（服务在，这一面不对）；
 * - `other`：别的（人话由调用方从错误自己的 `message` 里取 —— 那是封闭表给的）。
 */
export type HomeFailureKind = 'key' | 'local-key' | 'unreachable' | 'tls' | 'shape' | 'http' | 'other'

export function classifyHomeFailure(entry: HomeEntry, error: unknown): HomeFailureKind {
  const failure = failureOf(error)
  if (failure === 'unauthorized') return entry.local ? 'local-key' : 'key'
  if (failure === 'network') {
    if (entry.local) return 'unreachable'
    return stripSlash(entry.baseUrl).toLowerCase().startsWith('https:') ? 'tls' : 'unreachable'
  }
  if (failure === 'shape') return 'shape'
  // ⚠ **这一档必须自己有人话**（2026-09-25 修）：从前它落进 `other`，而 `other` 取的是
  // `error.message` —— 对 `HomeRequestError` 来说那句是**开发向英文**
  // （`home request failed: http (502)`）。实测：本机服务没起时，执行端那一行就把这串
  // 英文当人话渲染给了用户。`failureOf` 只认"鸭子字段"、认不出这是自家的错误类型，
  // 于是分类信息在手边却没用上。**稳定分类 → 固定人话**是 `contractErrors` 的纪律，
  // 这里补上漏掉的那一格。
  if (failure === 'http') return 'http'
  return 'other'
}

/** 分类 → 固定人话（每一条都是"为什么连不上/进不去"的直译，不带工程黑话）。 */
export function homeFailureText(entry: HomeEntry, error: unknown): string {
  switch (classifyHomeFailure(entry, error)) {
    case 'key':
      return '钥匙不对，去那台机器上重新拿一把'
    case 'local-key':
      return '这台机器的家口要钥匙了 —— 把它的地址和钥匙添成一条家条目就能连上'
    case 'unreachable':
      return entry.local
        ? '连不上本机服务（先开机：node home.js up）'
        : '连不上这台机器 —— 地址或端口不对'
    case 'tls':
      return '连不上这台机器 —— 地址、端口，或那张证书浏览器还没认（先在浏览器里直接打开一次它的地址，把证书放行）'
    case 'shape':
      return '那台机器答的话读不懂（版本对不上？）'
    case 'http':
      // ⚠ **不带状态码**：用户不需要知道 502/500 的区别，那是我们的事。
      // 但要说清"服务在、这一面不对"，因为那与"连不上"是**两件事**（不许合并成一句"连接失败"）。
      return '那台机器答了，但这一面它给的是个错误（服务在跑，状态口那一面没通）'
    case 'other': {
      // 别的错误（如模型配置门封闭表里的拒绝）自己带着一句人话 ⇒ 用它；没有就说不知道。
      const message = error instanceof Error ? error.message : ''
      return message || '这台机器出错了'
    }
  }
}

// ---------------------------------------------------------------------------
// 按 agentId 认领（三态：同一个人 / 读不到 id / 都不是）
// ---------------------------------------------------------------------------

/** 一份读数的**最小形状**：这里只要"这台机器上那个 Agent 的 id"那一格。 */
export interface AgentHomeView {
  agent: { agentId?: string }
  /**
   * ★ **这一台机器上住着哪几支**（`residents[]`；2026-10-08 加）。
   *
   * 可选：老门（或旧版口子）不给这一格 ⇒ 认领只按顶层那一格，行为与从前一字不变。
   * ⚠ 本层只用 `agentId` 那一格做认领；`host` / `slot` 归各自的消费者（见 `claimedHostOf`）。
   */
  residents?: readonly { agentId?: string | null; host?: { state: string; pid?: number | null; log?: string } }[]
  /** ★ 顶层那一格宿主读数（`ownReadings` 为真时它才是这一支的）。 */
  host?: { state: string; pid?: number | null; log?: string }
}

/**
 * 一个条目这一次的**状态读数**（`null` = 没读到；`failure` 是固定人话）。
 *
 * 泛型 `T` 让调用方**保住自己那份读数的具体形状**（`HomeStatusView`）——
 * 本层只按 `agent.agentId` 那一格做认领，但交给页面时不许被削成最小形状
 * （页面要渲染的东西远不止那一格）。
 */
export interface HomeReading<T extends AgentHomeView = AgentHomeView> {
  entry: HomeEntry
  view: T | null
  failure: string | null
}

/**
 * 哪个条目报告的就是这个 Agent（**两个判据，顺序是有意的**）。
 *
 * 1. **顶层那一格**（`agent.agentId`）：它是**第一支**（口子把第一支抬到顶层，为的是不破坏
 *    冻结前端）⇒ 单支的家与从前**一字不变**；
 * 2. ★ **住户清单**（`residents[]`，2026-10-08 加）：第 2 支起**只在这份名单里** ——
 *    从前不比它 ⇒ 它们被判成 `elsewhere`，页面于是写「还没有机器在跑它」：**假话**
 *    （它们就在这台机器上跑着，只是各住一个环境根）。
 *
 * ⚠ **两个判据的"读数归属"不同**（`ownReadings`）：① 认领的那一份顶层读数**就是它的**；
 * ② 认领靠名单时，顶层那几格是**第一支**的 ⇒ 住址对，**读数不是它的**（不许张冠李戴）。
 *
 * 没有匹配 ⇒ `null`（不匹配不许说成匹配；"读不到 id"更不许说成"不是它"）。
 */
export function claimEntry<T extends AgentHomeView>(
  readings: HomeReading<T>[],
  agentId: string | null | undefined,
): { entry: HomeEntry; view: T; ownReadings: boolean } | null {
  if (typeof agentId !== 'string' || agentId === '') return null
  for (const reading of readings) {
    if (reading.view && reading.view.agent.agentId === agentId) {
      return { entry: reading.entry, view: reading.view, ownReadings: true }
    }
  }
  for (const reading of readings) {
    const view = reading.view
    if (!view) continue
    if (view.residents?.some((resident) => resident.agentId === agentId)) {
      return { entry: reading.entry, view, ownReadings: false }
    }
  }
  return null
}

/**
 * 「家」这一面的**归属裁决**（三态不压成两态，与 `agentAgentMatch` 同一口径）：
 *
 * - `claimed`：某个条目报告的就是 Agent ⇒ 用那一条的读数（**住址＝那个条目**）；
 * - `unknown-id`：有读数但都没有 id 那一格 ⇒ **照旧显示**（像今天一样），不许说成"未接入"；
 * - `elsewhere`：都读了、报的都是**别人**的 id ⇒ 「未接入」（不是错误，是事实）；
 * - `unreachable`：一份都没读回来 ⇒ 「读不到」（把原因交出去，不冒充"未接入"）。
 */
export type AgentHomeResolution<T extends AgentHomeView = AgentHomeView> =
  | {
      kind: 'claimed'
      entry: HomeEntry
      view: T
      /**
       * ★ **这份 `view` 的支级读数是不是它自己的**（2026-10-08）。
       *
       * `true` = 认领靠**顶层那一格**（单支的家，或这个家的第一支）⇒ `agent` / `lastWake` /
       * `host` 那几格**就是它的**，照旧用。
       * `false` = 认领靠**住户清单**（第 2 支起）⇒ **住址对**（这台机器确实跑着它），
       * 但顶层那几格是**第一支**的 ⇒ **不许当它的读数用**：账本 / 最近活动 / 经历一律不给，
       * 宿主那一格要读**它自己那一项**里的（`residents[].host`）。
       *
       * ⚠ 为什么必须显式：不写这一格，消费者会照旧读 `view.agent.runs` —— 那是**别人的**账本，
       * 比留空更坏（留空用户会去问，错值没人核对）。
       */
      ownReadings: boolean
    }
  | { kind: 'unknown-id'; entry: HomeEntry; view: T }
  | { kind: 'elsewhere' }
  | { kind: 'unreachable'; entry: HomeEntry | null; failure: string | null }

export function resolveAgentHome<T extends AgentHomeView>(
  readings: HomeReading<T>[],
  agentId: string | null | undefined,
): AgentHomeResolution<T> {
  const claimed = claimEntry(readings, agentId)
  if (claimed) {
    return {
      kind: 'claimed',
      entry: claimed.entry,
      view: claimed.view,
      ownReadings: claimed.ownReadings,
    }
  }
  const unknownId = readings.find((reading) => reading.view && !reading.view.agent.agentId)
  if (unknownId && unknownId.view) {
    return { kind: 'unknown-id', entry: unknownId.entry, view: unknownId.view }
  }
  if (readings.some((reading) => reading.view !== null)) return { kind: 'elsewhere' }
  const failed = readings.find((reading) => reading.failure !== null)
  return { kind: 'unreachable', entry: failed?.entry ?? null, failure: failed?.failure ?? null }
}

/**
 * ★ **认领到的那一支自己的宿主那一格**（2026-10-08）——多支时它与顶层那一格**不是同一支的**。
 *
 * - 认领靠顶层（`ownReadings`）⇒ 就是顶层那一格；
 * - 认领靠住户清单 ⇒ 读**它自己那一项**里的 `host`（逐支判：一支一个进程）；
 * - 认不出（老门不给清单 / 那一项没给 host）⇒ `undefined` = **这一格没读数**
 *   （页面照旧说"读不到"，**不编一个"没在跑"**）。
 */
export function claimedHostOf<T extends AgentHomeView>(
  resolution: AgentHomeResolution<T>,
  agentId: string | null | undefined,
): { state: string; pid?: number | null; log?: string } | undefined {
  if (resolution.kind !== 'claimed') return undefined
  if (resolution.ownReadings) return resolution.view.host
  return resolution.view.residents?.find((resident) => resident.agentId === agentId)?.host
}

/**
 * 「Agent 的模型」这一面该问**哪个条目**（认领之外的最后一手，判据只有一条）：
 *
 * - 认领到了（`claimed` / `unknown-id`）⇒ 就是那一条；
 * - 一份读数都没回来（`unreachable`）、而**只配了一个家** ⇒ 用它 —— 只有一个的时候没有
 *   任何歧义（本机是预置的第一条 ⇒ 家里只有本机的人，这一手让"状态口没开、配置门开着"
 *   这个角落照旧看得到模型读数，与从前一致）；
 * - 别的（谁都不是 / 多个家里读不回来）⇒ `null`：**这一面不知道该问谁**，
 *   如实说读不到/未接入，不拿某一台顶上去。
 */
export function mindEntryFor<T extends AgentHomeView>(
  resolution: AgentHomeResolution<T>,
  entries: HomeEntry[],
): HomeEntry | null {
  if (resolution.kind === 'claimed' || resolution.kind === 'unknown-id') return resolution.entry
  if (resolution.kind === 'unreachable' && entries.length === 1) return entries[0] ?? null
  return null
}

/** 「Agent 的模型」这一面的读数：问的是哪一条 + 那条的配置读数（各自成败，不互相冒充）。 */
export interface MindReadout<C> {
  entry: HomeEntry
  /** 那一条的配置读数；`null` = **这一次没读到**（不是"没配"）。 */
  view: C | null
  /** 没读到时的固定人话（分类与文案都归 `homes.ts` 一处）。 */
  failure: string | null
}

/**
 * 「Agent 的模型」这一面**唯一**的一次判断：该问哪一条 ＋ 那一条读到了什么。
 *
 * 为什么必须收成一处：这条链有三步（认领裁决 → 该问谁 → 那条的读数），**两个调用点各拼
 * 一遍就等于两个答案**。事实正是如此 —— Agent 的主页拼的是 `mindEntryFor`，而「我的 Agent」
 * 列表行自己写了一个**少了"一份都没读回来、而只配了一个家"那一手**的局部判据，于是同一个
 * 输入下：主页显示那台机器的模型，列表行说「模型：未知」。**两条都在跑，门禁全绿。**
 *
 * 输入用**结构形状**而不是组件层的 `EntryReading`：本模块在 `api/`，不该反向依赖组件层。
 * 读数缺失（那条 entry 没有对应读数）⇒ `view` / `failure` 都为 `null`，**不抛、不猜**。
 */
export function mindReadoutFor<T extends AgentHomeView, C>(input: {
  resolution: AgentHomeResolution<T>
  entries: HomeEntry[]
  readings: ReadonlyArray<{
    entry: HomeEntry
    config: { view: C | null; failure: string | null }
  }>
}): MindReadout<C> | null {
  const entry = mindEntryFor(input.resolution, input.entries)
  if (!entry) return null
  const reading = input.readings.find((each) => each.entry.id === entry.id)
  return { entry, view: reading?.config.view ?? null, failure: reading?.config.failure ?? null }
}

// ---------------------------------------------------------------------------
// 存取（住哪儿：IndexedDB 的 `homes` store —— 这是**唯一**落盘的地方）
// ---------------------------------------------------------------------------

/**
 * 读出全部家条目：**本机那条永远是第一条**（预置），后面是存着的远程条目。
 *
 * 读不到库（不支持 / 坏了）⇒ **抛**：调用方（`use-home-entries`）据此如实说
 * "这台浏览器存不住家条目"，不许把它静默画成"你一个家都没配过"。
 */
export async function loadHomeEntries(): Promise<HomeEntry[]> {
  const local = localHomeEntry()
  const records = await listHomeEntryRecords()
  const stored = records
    .map(parseStoredEntry)
    .filter((entry): entry is HomeEntry => entry !== null && entry.id !== LOCAL_HOME_ID)
  return [local, ...stored]
}

/** 存一条家条目（同 id 覆盖）。**本机那条不落盘**（它是预置形状，不是用户数据）。 */
export async function saveHomeEntry(entry: HomeEntry): Promise<void> {
  if (entry.local) return
  await putHomeEntryRecord({
    id: entry.id,
    name: entry.name,
    baseUrl: entry.baseUrl,
    statusPort: entry.statusPort,
    configPort: entry.configPort,
    key: entry.key,
  })
}

/** 删一条家条目。**本机那条删不得**（预置的第一条；它是"同源代转"的化身）。 */
export async function removeHomeEntry(id: string): Promise<void> {
  if (id === LOCAL_HOME_ID) return
  await deleteHomeEntryRecord(id)
}