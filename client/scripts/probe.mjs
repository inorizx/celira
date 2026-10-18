/**
 * **页面探针**（probe）—— 替人把前端点一遍。
 *
 * ---
 * ## 它治的是什么
 *
 * 本仓已有两支浏览器脚本，各管一件事，**中间空着一档**：
 *
 * | 脚本 | 管什么 | 前置 |
 * |---|---|---|
 * | `screenshot-catalog.mjs` | 陈列室**渲染健全性**（三条非像素判据） | `vite build` + `dist` |
 * | `verify-ui.mjs` | **产品语义**（导航分组、文案词表、能力探测、同源请求） | 真实 core + Human token |
 * | `probe.mjs`（本支） | **每条路由到底渲没渲染出来** | 什么都不用（自己起 dev server） |
 *
 * 空着的那一档的代价是具体的：`verify-ui.mjs` 要一台真服务器和一个真 token，
 * 于是"**这个页面白屏了吗 / 崩了吗**"这种问题，在日常改动里**没有任何机械判据** ——
 * 只能靠人开 dev server 一页页点。而白屏正是本仓最在意的那类失败
 * （`lib/renderFailure.ts` 的文件头：白屏 = 最彻底的红路径静默）。
 *
 * ## 判据：只钉"变了就一定是坏"的三条
 *
 * 1. **不是白屏** —— 落地后 `body` 有正文；
 * 2. **不是降级页** —— 页面上没有 `renderFailure` 那两句标题（渲染期抛错 / chunk 拿不到）；
 * 3. **没有未捕获异常** —— `pageerror` ＋ 页内记录器（见下「页内异常记录器」）一条都不许有。
 *
 * 三条都**不依赖像素、不依赖产品文案**，所以改样式、改措辞**不会**让它误报 ——
 * 这正是 2026-09-25「拆掉像素对齐」那条裁决要的形状（见 `DESIGN_SYSTEM.md`
 * §渲染健全性检查）。控制台里的网络失败（状态口没开、core 没起）**不算失败**：
 * 那是"这一页如实降级了"，不是"这一页坏了"—— 它只被记进报告，供人看。
 *
 * ## 页内异常记录器（2026-10-03 —— 修「未捕获异常」判据的一处假绿）
 *
 * ⚠ 本机唯一可用的引擎（CloakBrowser Chromium 146）**不发 CDP 的 `Runtime.exceptionThrown`**：
 * 裸 CDP 会话显式 `Runtime.enable` 也一条收不到（同一会话的 `Log` 域正常）⇒ Playwright 的
 * `page.on('pageerror')` 里**页面自己抛的异常永远不来**，上面第 3 条判据在本机上**恒假**。
 * 修法：`newProbePage` 用 `context.addInitScript` 在页面里装记录器（监听 `error` /
 * `unhandledrejection`，记进页内数组），`probeOne` 判分前 `page.evaluate` 读回、并入
 * `collected.pageErrors`。**判据文案不变**（`未捕获异常 ×N`）。
 *
 * 两条口径（改这里之前先读）：
 * - **读不到 ≠ 没有**：记录器读不回来（页面已关 / 记录器没装上）时**不许**当成「零异常」——
 *   如实降级，并在报告 `notes` 里可见；
 * - **与 CDP 事件的去重**：将来换到会发 `Runtime` 事件的引擎时两条路都在，同一桩异常
 *   **不许重复计数**。口径：剥掉通道前缀（`error:` / `unhandledrejection:` / `pageerror:`）
 *   与 `Uncaught [(in promise)]`、`Error:` 前缀后逐字比较，同一文本只计一条（页内记录器
 *   那份为准，CDP 流里多出来的才补进）。`selfcheck` 的 `uncaught` 桩（期望恰为
 *   `未捕获异常 ×1`）就是这条口径的**正对照**——重复计数会让它变红。
 *
 * ## 它刻意**不**做什么
 *
 * - 不比对像素、不留基线（那是被拆掉的那条路，别再修回来）；
 * - 不断产品语义（导航分组、文案词表、按钮该不该在）—— 那是 `verify-ui.mjs` 的活；
 * - 不写任何公共事实：**只读**，不点保存、不发消息、不建东西。
 *
 * ## 两条来路
 *
 * - **演示通道**（默认，`core` + 空 token）：不需要任何服务器。fixtures 驱动页放行，
 *   真实数据页各自走"读不到"的诚实分支 —— 崩溃与白屏在这一档照样会现形；
 * - **真实通道**（`--token <Human token>`）：与 `verify-ui.mjs` 同一档凭据纪律 ——
 *   **只从 argv / 环境变量进，不落盘、不进日志、不进报告**。
 *
 * ## 用法
 *
 * ```bash
 * node scripts/probe.mjs routes                  # 列出会走的路径（含动态段用的探针值）
 * node scripts/probe.mjs sweep                   # 全路由冒烟（自己起 dev server）
 * node scripts/probe.mjs sweep --only /settings  # 只跑匹配的路径
 * node scripts/probe.mjs sweep --base http://localhost:5173   # 用已经在跑的服务器
 * node scripts/probe.mjs open /app/runtimes --click 'role=button[name="运行"]'
 * node scripts/probe.mjs selfcheck               # 正对照：那三条判据真的会红吗
 * ```
 *
 * 退出码：`0` 全绿；`1` 有路由失败；`2` 用法/环境错误（起不来服务器等）。
 */

import { spawn } from 'node:child_process'
import { mkdirSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright'
// 路由清单**读仓内权威声明表**（`api/routeRegistry.ts`），不在这里抄第二份：
// 那张表由 `tests/route-admission.test.ts` 与 `App.tsx` 的真实路由表逐条对表，
// 所以"探针漏了一条新路由"这件事在源码层就不会发生。零运行期依赖，可直接 import。
import { ROUTES } from '../src/api/routeRegistry.ts'
// 降级页那两句标题同样**从权威模块读**（`lib/renderFailure.ts`）：
// 仓里已经因为"守卫抄死了被守卫对象的文案、改名后静默失效"吃过好几次
// （`verify-ui.mjs` 文件头自己就记着 TASK_58 那次），能 import 就别抄。
import { CHUNK_FAILURE_TITLE, RENDER_FAILURE_TITLE } from '../src/lib/renderFailure.ts'
// 内行词表：**唯一权威**（`scripts/jargon.mjs`），本脚本与 `verify-ui.mjs` 共用同一份。
import { leakedJargon } from './jargon.mjs'

const ROOT = fileURLToPath(new URL('..', import.meta.url))
const DEFAULT_PORT = Number(process.env.CELIRA_PROBE_PORT ?? 5197)
const VIEWPORT = { width: 1440, height: 900 }

/**
 * ⚠ 下面三个串**抄自组件**（`.tsx` 带 JSX，import 不进来）。**改那边就要同步改这里**，
 * 否则探针会**静默失效**（等不到落地 ⇒ 每页都判超时；或者认不出诚实门 ⇒ 把
 * "进错通道"当成正常页）。
 */
const ROUTE_FALLBACK_LABEL = '页面加载中' // App.tsx 的 RouteFallback
const STARTUP_TEXT = '正在准备连接' // StartupLoading.tsx
const FIXTURE_GATE_TITLE = '本页面尚未提供真实数据' // FixtureGuard.tsx 的 FixturePageUnavailable

/**
 * 动态段用哪些**探针值**。
 *
 * 动态路由（`:roomId` 这类）没有"一条地址"可扫，但**跳过就等于没覆盖** ——
 * 而恰恰是这些页最容易在"认不出 id"那一支上崩（`verify-ui.mjs` 记着
 * `/app/agents/<不存在的 id>` 那一支曾经转瞬即逝、扫不到）。所以给每个动态段
 * 一个**具体值**，并在报告里标明"这一条用的是探针值"。
 *
 * - `:participantId` / `:roomId` 用 fixtures 里真实存在的 id ⇒ 演示通道能真渲染；
 *   真实通道里它们不存在 ⇒ 走页面自己的"没有这个 / 读不到"诚实分支，同样可判；
 * - `:entryId` 用本机的家条目 id（`api/homes.ts` 的 `LOCAL_HOME_ID`，写死为 `local`；
 *   那是**预置**的那一条，永远在列表里）。
 */
const PROBE_VALUES = {
  ':participantId': 'p-inori',
  ':roomId': 'r-guide',
  ':entryId': 'local',
  '*': '/this-route-does-not-exist',
}

const USAGE = `用法:
  node scripts/probe.mjs routes
  node scripts/probe.mjs sweep [选项]
  node scripts/probe.mjs open <path> [选项]
  node scripts/probe.mjs selfcheck          # 正对照：证明那三条判据真的会红

sweep / open 通用选项:
  --base <url>       用已经在跑的服务器（默认自己起 vite dev，端口 ${DEFAULT_PORT}）
  --port <n>         自己起 dev server 时用的端口
  --token <tk>       真实通道的 Human token（默认演示通道：core + 空 token）
  --server <url>     真实通道的服务器地址（留空 = 同源）
  --only <substr>    只跑路径里含这个子串的（可重复）
  --theme <dark|light>  默认 dark
  --headed           显示浏览器窗口
  --out <dir>        报告与截图的落盘目录（默认在系统临时目录里，**不落进仓库**）
  --print-json       把报告 JSON 也打到 stdout

open 专有:
  --click <选择器>    点一下（Playwright 选择器语法，如 role=button[name="连接"]；可重复）
  --fill <选择器>=<值>  填一个输入框（可重复；**值不进日志**）
  --wait <文本>       等到页面上出现这段文本
  --keep-open        结束后不关浏览器（调试用）

凭据纪律：token 只从 --token / CELIRA_UI_TOKEN 进，不落盘、不进日志、不进报告。
退出码：0 全绿 / 1 有失败 / 2 用法或环境错误。`

// ---------------------------------------------------------------------------
// 参数
// ---------------------------------------------------------------------------

function parseArgs(argv) {
  const out = {
    verb: null,
    path: null,
    base: null,
    port: DEFAULT_PORT,
    token: null,
    server: null,
    only: [],
    theme: 'dark',
    headed: false,
    out: null,
    printJson: false,
    clicks: [],
    fills: [],
    wait: null,
    keepOpen: false,
  }
  const positional = []
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i]
    const next = () => {
      i += 1
      return argv[i] ?? ''
    }
    switch (arg) {
      case '--base':
        out.base = next().replace(/\/+$/, '')
        break
      case '--port':
        out.port = Number(next())
        break
      case '--token':
        out.token = next()
        break
      case '--server':
        out.server = next().replace(/\/+$/, '')
        break
      case '--only':
        out.only.push(next())
        break
      case '--theme':
        out.theme = next()
        break
      case '--out':
        out.out = next()
        break
      case '--click':
        out.clicks.push(next())
        break
      case '--fill': {
        const raw = next()
        const at = raw.indexOf('=')
        if (at <= 0) fail(`--fill 要写成 <选择器>=<值>，收到：${raw}`)
        out.fills.push({ selector: raw.slice(0, at), value: raw.slice(at + 1) })
        break
      }
      case '--wait':
        out.wait = next()
        break
      case '--headed':
        out.headed = true
        break
      case '--keep-open':
        out.keepOpen = true
        break
      case '--print-json':
        out.printJson = true
        break
      case '--help':
      case '-h':
        console.log(USAGE)
        process.exit(0)
        break
      default:
        positional.push(arg)
    }
  }
  out.verb = positional[0] ?? null
  out.path = positional[1] ?? null
  // 凭据兜底走环境变量（与 verify-ui.mjs 同款：**只有这两个入口**）。
  out.token = out.token || process.env.CELIRA_UI_TOKEN?.trim() || ''
  out.server = out.server || process.env.CELIRA_UI_SERVER?.trim().replace(/\/+$/, '') || ''
  if (out.theme !== 'dark' && out.theme !== 'light') fail(`--theme 只认 dark / light，收到：${out.theme}`)
  return out
}

function fail(message) {
  console.error(message)
  process.exit(2)
}

const args = parseArgs(process.argv.slice(2))
if (!args.verb || !['routes', 'sweep', 'open', 'selfcheck'].includes(args.verb)) {
  console.error(USAGE)
  process.exit(2)
}
if (args.verb === 'open' && !args.path) fail('open 要一个路径，例如：open /app/runtimes')

/** 真实通道 = 手里有凭据（与 `api/prefs.ts` 的 `isRealWorldSession` 同一条判据）。 */
const realChannel = args.token !== ''

/**
 * 浏览器引擎：**优先用本机既有的引擎**（`CELIRA_BROWSER_EXECUTABLE`，与 `verify-ui.mjs`
 * 同一入口）。本机没有 playwright 自带的下载件、而红线不许下载时，指一下现有 chromium 即可
 * （2026-10-02 补：`npm run verify` 在没有下载件的机器上的前置）。
 */
const launchBrowser = (headed) =>
  chromium.launch({
    headless: !headed,
    args: ['--no-proxy-server'],
    executablePath: process.env.CELIRA_BROWSER_EXECUTABLE?.trim() || undefined,
  })

// ---------------------------------------------------------------------------
// 服务器
// ---------------------------------------------------------------------------

/** 已经在跑就直接用；没给 `--base` 就自己起一支 vite dev，收工时杀掉。 */
async function ensureServer() {
  if (args.base) {
    const alive = await reachable(args.base)
    if (!alive) fail(`打不开 ${args.base} —— 先确认服务在跑，或去掉 --base 让探针自己起。`)
    return { base: args.base, stop: () => {} }
  }
  const base = `http://localhost:${args.port}`
  if (await reachable(base)) {
    console.log(`（端口 ${args.port} 上已经有服务器，直接用它：${base}）`)
    return { base, stop: () => {} }
  }
  const child = spawn(
    process.execPath,
    [join(ROOT, 'node_modules', 'vite', 'bin', 'vite.js'), '--port', String(args.port), '--strictPort'],
    { cwd: ROOT, stdio: 'ignore' },
  )
  child.on('exit', (code) => {
    if (code !== null && code !== 0) console.error(`vite dev 提前退出（code ${code}）`)
  })
  const deadline = Date.now() + 60000
  while (Date.now() < deadline) {
    if (await reachable(base)) return { base, stop: () => child.kill() }
    await sleep(300)
  }
  child.kill()
  fail(`dev server 60 秒内没起来（${base}）`)
}

async function reachable(base) {
  try {
    const res = await fetch(base, { redirect: 'manual' })
    return res.status < 500
  } catch {
    return false
  }
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

// ---------------------------------------------------------------------------
// 路由
// ---------------------------------------------------------------------------

/** 把声明表里的动态段换成探针值，得到一条**真的打得开**的地址。 */
function resolvePath(routePath) {
  let out = routePath
  const used = []
  for (const [segment, value] of Object.entries(PROBE_VALUES)) {
    if (segment === '*') continue
    if (out.includes(segment)) {
      out = out.replace(segment, value)
      used.push(`${segment}→${value}`)
    }
  }
  if (routePath === '*') return { path: PROBE_VALUES['*'], dynamic: ['*→404 兜底'] }
  return { path: out, dynamic: used }
}

function selectRoutes() {
  const picked = []
  for (const route of ROUTES) {
    const { path, dynamic } = resolvePath(route.path)
    if (args.only.length > 0 && !args.only.some((needle) => route.path.includes(needle))) continue
    picked.push({ declared: route.path, path, side: route.side, what: route.what, dynamic })
  }
  return picked
}

// ---------------------------------------------------------------------------
// 通道
// ---------------------------------------------------------------------------

/**
 * 每个页面在**载入前**钉住连接偏好与主题（与 `screenshot-catalog.mjs` 同一手法）：
 * 钉住才可复现 —— 否则上一页留下的 localStorage 会让这一页走另一条分支。
 */
function initScriptFor(channel) {
  const prefs = realChannel
    ? { mode: 'core', token: channel.token, serverUrl: args.server, worldId: '' }
    : { mode: 'core', token: '', serverUrl: '', worldId: '' }
  return {
    prefs: JSON.stringify(prefs),
    theme: args.theme,
  }
}

/**
 * ★ 页内异常记录器（2026-10-03）：为什么必须用它、读不到怎么办、去重口径，见文件头
 * 「页内异常记录器」一节。挂的是**页面自己的** `error` / `unhandledrejection`
 * （addInitScript 在页面脚本之前跑），不依赖引擎的 CDP Runtime 事件。
 * 资源加载失败不会进这里：error 事件默认不冒泡到 window（诊断报告里实测过）。
 */
const IN_PAGE_ERROR_RECORDER = () => {
  const bucket = []
  Object.defineProperty(window, '__celiraProbeErrors', { value: bucket, configurable: true })
  window.addEventListener('error', (event) => {
    bucket.push(`error: ${event?.message ?? String(event?.error ?? event)}`)
  })
  window.addEventListener('unhandledrejection', (event) => {
    const reason = event?.reason
    bucket.push(`unhandledrejection: ${String(reason?.message ?? reason)}`)
  })
}

/**
 * 读回页内记录器。**读不到返回 `null`**（记录器没装上 / 页面已关 / 导航把 world 换了）——
 * 调用方不许把它当成「零异常」（仓库纪律：读不到 ≠ 没有），要如实降级并让它可见。
 */
async function readRecordedErrors(page) {
  return page
    .evaluate(() => (Array.isArray(window.__celiraProbeErrors) ? window.__celiraProbeErrors.slice() : null))
    .catch(() => null)
}

/** 归一化口径（去重只认它）：剥掉通道前缀与 `Uncaught [(in promise)]`、`Error:` 前缀。 */
function normalizeUncaughtLine(line) {
  return String(line)
    .replace(/^(?:pageerror|error|unhandledrejection):\s*/, '')
    .replace(/^Uncaught(?:\s+\(in promise\))?\s*/, '')
    .replace(/^Error:\s*/, '')
    .trim()
}

/**
 * 合并两条来路的未捕获异常并**去重**（口径见文件头）。页内记录器那份是基准
 * （本机引擎下它是唯一来路）；CDP 流里逐条对得上的丢掉、对不上的补进来 ——
 * 同一条异常只算一条，但两边各自的多重出现照各自的份数保留（`×N` 的 N 要准）。
 */
function mergeUncaughtExceptions(cdpErrors, recordedErrors) {
  const pool = recordedErrors.map(normalizeUncaughtLine)
  const unmatched = []
  for (const line of cdpErrors) {
    const index = pool.indexOf(normalizeUncaughtLine(line))
    if (index >= 0) {
      pool.splice(index, 1) // 这条异常已由记录器计过 ⇒ 不再重复计数
      continue
    }
    unmatched.push(line)
  }
  return [...recordedErrors, ...unmatched]
}

async function newProbePage(browser, channel) {
  const context = await browser.newContext({
    viewport: VIEWPORT,
    colorScheme: args.theme,
    reducedMotion: 'reduce',
  })
  const init = initScriptFor(channel)
  await context.addInitScript((payload) => {
    localStorage.setItem('celira.connection.prefs', payload.prefs)
    localStorage.setItem('celira-theme', payload.theme)
  }, init)
  // 页内异常记录器（见文件头「页内异常记录器」）：挂 context 上 ⇒ 本页每个新文档都装。
  await context.addInitScript(IN_PAGE_ERROR_RECORDER)
  const page = await context.newPage()
  page.setDefaultTimeout(20000)
  const collected = { pageErrors: [], consoleErrors: [], httpFailures: [], requestFailures: [] }
  page.on('pageerror', (error) => collected.pageErrors.push(String(error?.message ?? error)))
  page.on('console', (message) => {
    if (message.type() !== 'error') return
    const where = message.location()?.url
    collected.consoleErrors.push(where ? `${message.text()} ← ${where}` : message.text())
  })
  page.on('response', (response) => {
    if (response.status() < 400) return
    collected.httpFailures.push({ status: response.status(), url: response.url() })
  })
  page.on('requestfailed', (request) => {
    collected.requestFailures.push(`${request.failure()?.errorText ?? 'failed'} ← ${request.url()}`)
  })
  return { context, page, collected }
}

/**
 * 等页面**落到终态**——等一个状态，不等一个时长。
 *
 * 三条路各自是一个状态（`RouteFallback` 的转圈没了 / 有正文了 / 壳层的「正在准备连接」
 * 过去了）；三者同时成立才算落地。`networkidle` 只给 3 秒兜底：状态口那一次探测要
 * 3~4 秒，抢不到就算了 —— 它在报告里以"网络失败"的形式如实出现，不该把整页判成超时。
 */
async function waitForSettled(page, timeoutMs = 20000) {
  const ok = await page
    .waitForFunction(
      ({ fallbackLabel, startupText }) => {
        const text = (document.body.innerText || '').trim()
        const loading = document.querySelector(`[aria-label="${fallbackLabel}"]`) !== null
        return !loading && text.length > 0 && !text.includes(startupText)
      },
      { fallbackLabel: ROUTE_FALLBACK_LABEL, startupText: STARTUP_TEXT },
      { timeout: timeoutMs },
    )
    .then(() => true)
    .catch(() => false)
  await page.waitForLoadState('networkidle', { timeout: 3000 }).catch(() => {})
  return ok
}

function classify(text, collected, settled, routePath = null) {
  const problems = []
  if (!settled) problems.push('没落地（等到超时仍是加载态或空白）')
  if (text.trim().length === 0) problems.push('白屏（落地后正文为空）')
  if (text.includes(RENDER_FAILURE_TITLE) || text.includes(CHUNK_FAILURE_TITLE)) {
    problems.push('撞了渲染降级页')
  }
  if (collected.pageErrors.length > 0) problems.push(`未捕获异常 ×${collected.pageErrors.length}`)
  // **内行词不许上屏**（词表唯一权威 = `scripts/jargon.mjs`）。
  // ⚠ 2026-09-26 搬到这里：它原先**只在 `verify-ui.mjs` 里**，而那支脚本要真机 core + token、
  //    **不在 `verify` 里**、且很久没跑过 ⇒ 实测漏了 13 处（B2 一次扫出来的）。
  //    `sweep` 本来就在 `verify` 里、本来就逐条渲染全部路由、本来就握着渲染后的正文。
  const jargon = leakedJargon(text, routePath)
  if (jargon.length > 0) problems.push(`内行词上屏：${JSON.stringify(jargon)}`)
  const notes = []
  if (collected.recorderUnavailable) {
    notes.push('页内异常记录器读不到（「未捕获异常」这一条本轮降级：读不到 ≠ 没有异常）')
  }
  if (text.includes(FIXTURE_GATE_TITLE)) notes.push('诚实门（真实通道里的 fixture 页）')
  if (collected.consoleErrors.length > 0) notes.push(`控制台错误 ×${collected.consoleErrors.length}`)
  if (collected.httpFailures.length > 0) notes.push(`HTTP ≥400 ×${collected.httpFailures.length}`)
  return { problems, notes }
}

// ---------------------------------------------------------------------------
// 动词
// ---------------------------------------------------------------------------

function slug(routePath) {
  // `*` 与 `/` 都会落到 'root' 上（`/` 去掉斜杠后是空串）—— 显式分开，
  // 否则同一份报告里两张图同名，看的人分不清哪张是 404 兜底页。
  if (routePath === '*') return 'not-found'
  return (
    routePath
      .replace(/^\//, '')
      .replace(/[^a-zA-Z0-9]+/g, '-')
      .replace(/^-|-$/g, '') || 'root'
  )
}

function outDir() {
  if (args.out) return args.out
  const stamp = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19)
  return join(tmpdir(), 'celira-probe', stamp)
}

function doRoutes() {
  const picked = selectRoutes()
  const dynamicCount = picked.filter((r) => r.dynamic.length > 0).length
  for (const route of picked) {
    const tail = route.dynamic.length > 0 ? `   [探针值 ${route.dynamic.join(' ')}]` : ''
    console.log(`${route.path.padEnd(38)} ${route.side.padEnd(9)} ${route.what}${tail}`)
  }
  console.log(
    `\n共 ${picked.length} 条（其中 ${dynamicCount} 条用了探针值），来源：src/api/routeRegistry.ts`,
  )
}

/**
 * 探一条路由：开一个**独立 context**（localStorage / 网络 / cookie 都隔离，
 * 上一页不会把状态漏给这一页）→ 导航 → 等落地 → 采正文/截图/异常 → 判。
 *
 * ⚠ **判据只有这一份**：`sweep` 与 `selfcheck` 都走它 —— 自检若另写一套判据，
 * 它证明的就不是线上跑的那份，那正是"守卫写死被守卫对象"的同族病。
 */
async function probeOne(browser, base, target, { shotPath = null, settleTimeout = 20000 } = {}) {
  const { context, page, collected } = await newProbePage(browser, { token: args.token })
  const started = Date.now()
  let settled = false
  let text = ''
  let finalPath = target.path
  try {
    await page.goto(`${base}${target.path}`, { waitUntil: 'domcontentloaded', timeout: 25000 })
    settled = await waitForSettled(page, settleTimeout)
    text = await page.locator('body').innerText().catch(() => '')
    finalPath = new URL(page.url()).pathname
    if (shotPath) await page.screenshot({ path: shotPath })
  } catch (error) {
    collected.pageErrors.push(`导航失败：${String(error?.message ?? error).split('\n')[0]}`)
  }
  // 判分前读回页内记录器，接到「未捕获异常」判据上（见文件头；本机引擎不发 CDP 异常事件，
  // 这里是唯一来路）。读不到 ⇒ 标记降级（classify 会写进 notes），**不许**当成「零异常」。
  const recorded = await readRecordedErrors(page)
  if (recorded === null) collected.recorderUnavailable = true
  else collected.pageErrors = mergeUncaughtExceptions(collected.pageErrors, recorded)
  const { problems, notes } = classify(text, collected, settled, target.path)
  await context.close()
  return {
    declared: target.declared,
    path: target.path,
    finalPath,
    side: target.side,
    dynamic: target.dynamic ?? [],
    ms: Date.now() - started,
    settled,
    textLength: text.trim().length,
    textHead: text.replace(/\s+/g, ' ').trim().slice(0, 200),
    problems,
    notes,
    pageErrors: collected.pageErrors,
    consoleErrors: collected.consoleErrors.slice(0, 10),
    httpFailures: dedupe(collected.httpFailures).slice(0, 10),
    requestFailures: collected.requestFailures.slice(0, 10),
  }
}

async function sweep() {
  const picked = selectRoutes()
  const dir = outDir()
  mkdirSync(dir, { recursive: true })
  const { base, stop } = await ensureServer()
  const browser = await launchBrowser(args.headed)
  const results = []
  try {
    console.log(
      `\n探针：${base} · 通道=${realChannel ? '真实（有凭据）' : '演示（core + 空 token）'} · ` +
        `主题=${args.theme} · ${picked.length} 条路由\n`,
    )
    for (const [index, route] of picked.entries()) {
      // 图名用**落地的那条路径**（不是声明里的 `:roomId`）：翻文件夹的人要的是
      // "这张是哪一页"，不是"它由哪条模板来的"。
      const shot = join(dir, `${String(index + 1).padStart(2, '0')}-${slug(route.path)}.png`)
      const result = await probeOne(browser, base, route, { shotPath: shot })
      results.push(result)
      const mark = result.problems.length === 0 ? '✓' : '✗'
      const tail =
        result.problems.length > 0
          ? `  ← ${result.problems.join('；')}`
          : result.notes.length > 0
            ? `  (${result.notes.join('，')})`
            : ''
      console.log(`${mark} ${route.path.padEnd(38)} ${String(result.ms).padStart(6)}ms${tail}`)
    }
  } finally {
    await browser.close()
    stop()
  }

  const failed = results.filter((r) => r.problems.length > 0)
  const report = {
    at: new Date().toISOString(),
    base: args.base ?? `http://localhost:${args.port}`,
    channel: realChannel ? 'real' : 'demo',
    theme: args.theme,
    total: results.length,
    failed: failed.length,
    results,
  }
  const reportPath = join(dir, 'report.json')
  writeFileSync(reportPath, JSON.stringify(report, null, 2), 'utf8')

  console.log(
    `\n${results.length} 条路由：${results.length - failed.length} 条渲染正常，${failed.length} 条有问题。`,
  )
  if (failed.length > 0) {
    console.log('\n有问题的：')
    for (const item of failed) console.log(`  ${item.path} — ${item.problems.join('；')}`)
  }
  console.log(`\n截图与报告：${dir}`)
  if (args.printJson) console.log(JSON.stringify(report))
  // 空跑不许假装通过：路由清单为空 ⇒ 非零退出（仓规：门禁必须断言用例数 > 0）。
  if (results.length === 0) {
    console.error('\n一条路由都没跑到（--only 过滤掉了？）—— 按空跑处理，判红。')
    process.exit(1)
  }
  process.exit(failed.length > 0 ? 1 : 0)
}

function dedupe(items) {
  const seen = new Set()
  return items.filter((item) => {
    const key = `${item.status} ${item.url}`
    if (seen.has(key)) return false
    seen.add(key)
    return true
  })
}

/**
 * `selfcheck` —— **正对照**：证明上面那三条判据真的会红。
 *
 * ---
 * ## 为什么必须有它
 *
 * **一次全绿的报告，与一次"判据恒假"的报告长得一模一样。** 本仓对这件事有明确纪律
 * （门禁必须断言用例数 > 0；词表守卫要带正反例）—— 一条永远不红的判据不是判据，是装饰。
 *
 * 所以这里喂**六个桩页面**（一支临时 http 服务器，不碰真实应用），逐个核对判据
 * **逐字**命中：正常页必须绿，白屏 / 降级页 / 抛错页 / 内行词上屏页必须各自红在**那一条**上，
 * 而"没有内行词的正常页"必须绿。用的判据就是 `probeOne` —— 与 `sweep` 同一份，不是另写一套。
 */
async function selfcheck() {
  const cases = [
    { name: 'healthy', html: '<h1>正常</h1><p>这一页渲染出来了。</p>', expect: [] },
    { name: 'blank', html: '<div id="root"></div>', expect: ['白屏（落地后正文为空）'] },
    {
      name: 'failure-screen',
      html: `<h1>${RENDER_FAILURE_TITLE}</h1><p>界面在渲染这一步出了问题。</p>`,
      expect: ['撞了渲染降级页'],
    },
    {
      name: 'uncaught',
      html: '<h1>抛错页</h1><script>throw new Error("probe selfcheck")</script>',
      expect: ['未捕获异常 ×1'],
    },
    // ⚠ **2026-09-26 补两个内行词桩**（判据搬进来就必须同时补正反例）：
    // 一个**必须红**（页面把内行词摆上了屏），一个**必须绿**（同一个词只在注释里 ——
    // 但注释不在渲染后的正文里，所以这一支其实证明的是"判据读的是正文，不是源码"）。
    // 没有这两支，`sweep` 的全绿与"判据恒假"长得一模一样。
    {
      name: 'jargon-on-screen',
      html: '<h1>本机受管 Runtime</h1><p>准入：允许</p>',
      expect: ['内行词上屏：["准入","Runtime"]'],
    },
    {
      name: 'jargon-free',
      html: '<h1>执行端</h1><p>这台机器上的 Agent 在不在跑。</p>',
      expect: [],
    },
  ]
  const server = createServer((req, res) => {
    const name = (req.url ?? '/').replace(/^\//, '').split('?')[0]
    const found = cases.find((c) => c.name === name)
    res.writeHead(found ? 200 : 404, { 'content-type': 'text/html; charset=utf-8' })
    res.end(found ? `<!doctype html><html><body>${found.html}</body></html>` : 'not found')
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const base = `http://127.0.0.1:${server.address().port}`
  const browser = await launchBrowser(args.headed)
  const failures = []
  try {
    console.log(`\n正对照：${cases.length} 个桩页面（判据与 sweep 同一份：probeOne）\n`)
    for (const item of cases) {
      // 白屏那一页永远等不到落地 ⇒ 给短超时，别让自检耗掉 20 秒。
      const result = await probeOne(
        browser,
        base,
        { declared: item.name, path: `/${item.name}`, side: 'selfcheck', dynamic: [] },
        { settleTimeout: 3000 },
      )
      const hit = item.expect.every((expected) => result.problems.includes(expected))
      const ok = item.expect.length === 0 ? result.problems.length === 0 : hit
      if (!ok) failures.push({ name: item.name, expect: item.expect, got: result.problems })
      console.log(
        `${ok ? '✓' : '✗'} ${item.name.padEnd(16)} 期望 ${JSON.stringify(item.expect)} · 实得 ${JSON.stringify(result.problems)}`,
      )
    }
  } finally {
    await browser.close()
    server.close()
  }
  if (failures.length > 0) {
    console.error(`\n正对照失败：判据没有按预期变红 —— 它现在拦不住东西，别信 sweep 的绿。`)
    for (const f of failures) console.error(`  ${f.name}：期望 ${JSON.stringify(f.expect)}，实得 ${JSON.stringify(f.got)}`)
    process.exit(1)
  }
  console.log(`\n正对照通过：${cases.length}/${cases.length}（判据真的会红，也有一个真的绿）。`)
}

/**
 * `open` —— 单页探针，可点可填。这是"**替我点一下这个**"那条路：
 * 不做断言，只把**落地后的正文、截图、控制台**交出来给人看。
 */
async function open() {
  const dir = outDir()
  mkdirSync(dir, { recursive: true })
  const { base, stop } = await ensureServer()
  const browser = await launchBrowser(args.headed)
  const { page, collected } = await newProbePage(browser, { token: args.token })
  try {
    await page.goto(`${base}${args.path}`, { waitUntil: 'domcontentloaded', timeout: 25000 })
    await waitForSettled(page)
    for (const selector of args.clicks) {
      console.log(`点击 ${selector}`)
      await page.locator(selector).first().click()
      await waitForSettled(page)
    }
    for (const fill of args.fills) {
      // ⚠ 值**只报长度**：填 token 那种用法下，值进日志就是泄露。
      console.log(`填写 ${fill.selector}（值 ${fill.value.length} 字符，不回显）`)
      await page.locator(fill.selector).first().fill(fill.value)
    }
    if (args.wait) {
      await page.getByText(args.wait).first().waitFor({ timeout: 20000 })
      console.log(`等到了「${args.wait}」`)
    }
    const text = await page.locator('body').innerText()
    const shot = join(dir, `${slug(args.path)}.png`)
    await page.screenshot({ path: shot })
    console.log(`\n落地地址：${page.url()}`)
    console.log(`\n--- 正文（${text.trim().length} 字符）---\n${text.trim()}`)
    // 与 probeOne 同一读法：页内记录器是「未捕获异常」在本机引擎上的唯一来路。
    const recorded = await readRecordedErrors(page)
    if (recorded === null) {
      console.log(`\n未捕获异常：页内记录器读不到（读不到 ≠ 没有 —— 这一项本轮没有读数）`)
    } else {
      const uncaught = mergeUncaughtExceptions(collected.pageErrors, recorded)
      if (uncaught.length > 0) console.log(`\n未捕获异常：\n  ${uncaught.join('\n  ')}`)
    }
    if (collected.consoleErrors.length > 0) {
      console.log(`\n控制台错误：\n  ${collected.consoleErrors.slice(0, 10).join('\n  ')}`)
    }
    const http = dedupe(collected.httpFailures)
    if (http.length > 0) {
      console.log(`\nHTTP ≥400：\n  ${http.slice(0, 10).map((h) => `${h.status} ${h.url}`).join('\n  ')}`)
    }
    console.log(`\n截图：${shot}`)
    if (args.keepOpen) {
      console.log('（--keep-open：浏览器留着，Ctrl+C 结束）')
      await new Promise(() => {})
    }
  } finally {
    if (!args.keepOpen) {
      await browser.close()
      stop()
    }
  }
}

if (args.verb === 'routes') doRoutes()
else if (args.verb === 'sweep') await sweep()
else if (args.verb === 'selfcheck') await selfcheck()
else await open()
