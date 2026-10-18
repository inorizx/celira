#!/usr/bin/env node
/**
 * **打包形态（产品形态）的真机页面级门禁** —— `verify:ui` 的姊妹。
 *
 * ## 为什么要有它（2026-10-05 真机撞出来的教训）
 *
 * `verify:ui` 跑的是 **web 形态**：页面由 vite（dev）或 `serve_client.mjs`（发布）供在
 * `http://localhost:5173`，登录走页面上的表单。而**用户真正会打开的那个东西**是
 * `tauri build` 出来的壳：页面走**内嵌产物**，来源是 Tauri 的自定义协议
 * `http://tauri.localhost`。两者**不是同一条路**：
 *
 * - 取资源：dev 走 vite 的 5173；产品走内嵌 dist ＋ 自定义协议；
 * - 出网：浏览器里是 `fetch`；壳里换成 Rust 侧（`tauri-plugin-http`，不经同源策略）；
 * - 本机那两个口：web 形态由 `node home.js doors` 供（页面同源相对路径 ＋ 代转）；
 *   壳里由**壳自己端**，页面按 loopback 绝对地址跨源打过去。
 *
 * ⇒ **"dev 绿、产品红"是结构性可能的**，而且真发生过：2026-10-05 打包形态里机器页
 * 「模型」那一间读不到 —— 配置门的来源守卫拒了壳自己的来源（`http://tauri.localhost`
 * 不是 loopback），而同一台机器上 web 形态一直是绿的。登记在 `app/README.md` §5 第 10 行，
 * 修在 2026-10-05（`home-key.js` 的 `SHELL_ORIGINS` ＋ 壳里那份镜像）。
 *
 * ## 它量什么（**只管产品形态特有的那一面**，不重复 `verify:ui` 的 85 条）
 *
 * 1. **形态判据本体**：页面真的是 `http://tauri.localhost`（否则这一颗二进制是 dev 形态
 *    ⇒ 报**不适用**并给出重编命令，绝不假装通过）；
 * 2. 落点：落进 `/app` 壳（不是登录页、不是 404 页、不是出错页）；
 * 3. **壳专属读数**：本机服务面板三行（世界 / Agent / 退出行为）真的读得到，
 *    且**不出现 web 专属那句**"这一格只有 app 里才有"。
 *    ⚠ **读数落定才算数**（2026-10-08 第二刀）：面板挂上来之后还有一帧「正在读本机服务…」，
 *    那一帧里"本机服务 / 世界"这两个字在**说明文字**里就有、「退出时：读不到」也成立 ⇒
 *    不并上"落定"这一格，这两条会**假绿**（实测冷启动那一次就是这样：17/18，只有 Agent 那一行红）；
 * 4. 机器页两间（运行 / 模型）＋「活动」明细那一层下钻在**打包形态**下读得到 —— 「模型」那一间
 *    **不许**出现"这个来源不被允许（只允许从本机访问）"（那正是上面那条缺陷的形状）；
 *    ★ **2026-10-11 加一条产品形态特有的**：机器页那张「第一次」卡（先配一条模型源）在**壳里**
 *    也出得来 —— 它的在场判据是 `modelSourceAbsent`，而**两个门的载荷形状不一样**
 *    （web 门给 `sourcesState: 'absent'`，壳门给 `'loaded'` ＋ 空表）⇒ 认 `'absent'` 的写法
 *    **在壳里恒不触发**。写成条件式：模型那一间说"还没有" ⇒ 卡必须在场；配好之后报不适用。
 * 5. **门的判据**（带壳自己的来源头打三个口）：状态口 / 配置门 / 绑定口都 200；
 *    **反例**：外域来源照旧 403（守卫没被放宽 —— 这一条是安全断言，不是形式）；
 * 6. **全路由渲染冒烟**（`routeRegistry.ts` 的 `ROUTES`，与 `probe sweep` 同一份清单）：
 *    逐条在打包形态里打开，正文非空、不是 404 页、不是出错页、没有 pageerror。
 *
 * ## 用法与环境前置
 *
 * ```text
 * npm run verify:ui:app [--home <家目录>] [--exe <壳的可执行文件>] [--port <CDP 口>] [--keep-open]
 * ```
 *
 * - **壳要打包形态的**：`cd app && npm run build -- --no-bundle`。
 *   ⚠ **不许用裸 `cargo build --release` 出壳**（2026-10-11 实测踩过）：Tauri 只在 `tauri build`
 *   下才关掉 dev cfg ⇒ 裸 `cargo build` 出来的是 **dev 形态**（它去找 vite 的 5173），
 *   页面是 `about:blank` —— 而这条门禁会**当场抓住**（「形态判据」与「壳里嵌的产物是最新的」
 *   两条同时红）。⚠ 旧提示写的 `cargo build --features tauri/custom-protocol` 是 **Tauri v1**
 *   的说法，本仓是 v2（没有这个 feature）—— 照它做只会得到同一颗 dev 壳。
 *   ⚠ 绕开 `beforeBuildCommand` 的正路：`npm run build -- --no-bundle --config <一份只把
 *   build.beforeBuildCommand 置空的 JSON>`（那两步编的是**壳不依赖**的 crate；dist 由
 *   `npm run verify` 重建，本门禁第一条断言会替你核它是不是最新）。
 *   缺省按 `target/release` → `target/debug` 找 `celira-app.exe`；找不到或那一颗是 dev 形态
 *   ⇒ 报**不适用**（退出 2）；
 * - **家**：缺省读 `CELIRA_HOME_DIR` / 平台的默认家（与 `home.js` 同一处取值）。
 *   ⚠ 它**会把这个家的世界起起来**（打开 app 本来就会这样），跑完**只把自己起的那些停掉**
 *   （进来之前世界就在跑 ⇒ 一个都不动）。⚠ **壳起的那份世界 `home.js down` 收不掉**
 *   （2026-10-08 实测：壳用自己那份 sidecar，家的认领判据只认仓根那份 ⇒ 按纪律拒绝杀）——
 *   所以收尾由本脚本按**差集 ＋ 身份**再扫一遍端口，且 `down` 失败时**把它的原话打出来**
 *   （原先被 `stdio: 'ignore'` 吞掉，孤儿世界会留到下一次 `tauri build` 才炸）；
 *   ⚠ **住户（`agent_host.exe`）也归本脚本收**（2026-10-08 第二刀）：世界收掉了、住户还在盘上，
 *   下一次冷启动它们会让这一次的世界与门起不来 —— 实测第二次跑 13/18，红的全是"读不到本机服务"，
 *   **看着像代码坏了**。判据与 `sweepWorldOrphan` 同形（差集 ＋ 种类 ＋ 家事实），见 `sweepResidentOrphans`；
 * - **不写任何配置**：只读页面与三个只读端点，**不点**「换绑」「保存」这类写按钮；
 * - **壳里若记着别的家的会话**（页面停在登录页）：本脚本**不清你的浏览器数据**，
 *   如实报**不适用**并给出那一步怎么做（在壳里登出一次，或换一颗干净的壳再跑）。
 *
 * ⚠ 它是**真机门禁**（要一颗能跑的壳 ＋ 一个家），进不了 `npm run verify`。触发条件写在
 * `AGENTS.md`「验证与汇报」：**改用户可见面、改壳的门与读数、改本机那两个口的来源判据时，
 * 除了 `verify:ui` 还要跑它**。
 */

import { spawn, spawnSync } from 'node:child_process'
import { createRequire } from 'node:module'
import fs from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

import { chromium } from 'playwright'

import { ROUTES } from '../src/api/routeRegistry.ts'
// ★ **门判据的共用表**（唯一权威位置）：源码形态与壳都跑它 —— 见那个文件头。
import {
  SHELL_ORIGIN,
  describeDoorCases,
  doorCasesSayingWrongThing,
  runDoorCases,
} from './door-cases.mjs'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const CLIENT_DIR = path.resolve(HERE, '..')
const REPO_DIR = path.resolve(CLIENT_DIR, '..')
const APP_DIR = path.join(REPO_DIR, 'app')

/**
 * ⚠ **家的默认值不在这里拼**（2026-10-05 写这条时踩过）：权威是 `scripts/home/paths.js`
 * 的 `DEFAULT_HOME`（Windows 上是 `D:/celira-home`）＋ `CELIRA_HOME_DIR` 覆盖 ——
 * 我按"系统盘"猜过一次 `C:\celira-home`，结果门禁对着一个不存在的家报"环境没就绪"。
 * 同一类事实只有一处权威：这里 `require` 它，不抄一份。
 */
const homePaths = createRequire(import.meta.url)('../../scripts/home/paths.js')
/**
 * 家的编排件（**同一类事实只有一处权威**：端口读 `home.conf`、收进程用它自己的判据，
 * 这里不抄第二份）。`down` 收不掉壳起的那份世界的原因见文件末的 `sweepWorldOrphan`。
 */
const homeConf = createRequire(import.meta.url)('../../scripts/home/conf.js')
const homeProc = createRequire(import.meta.url)('../../scripts/home/proc.js')
/**
 * **住户表**（只读路径：`status` 用的同一份）—— 收尾那一格要的"家事实"从这里来：
 * `describeHostProcesses` 的认领判据第二维是 `home.envDirs`（一支一个环境根），
 * 而那是**家事实**，权威在 `residents.js`，这里不另拼一份。
 */
const homeResidents = createRequire(import.meta.url)('../../scripts/home/residents.js')

// ---------------------------------------------------------------------------
// 参数与环境前置
// ---------------------------------------------------------------------------

function parseArgs(argv) {
  const options = { home: null, exe: null, port: 9222, keepOpen: false }
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index]
    if (flag === '--keep-open') {
      options.keepOpen = true
    } else if (flag === '--home' || flag === '--exe' || flag === '--port') {
      const value = argv[index + 1]
      if (typeof value !== 'string' || value.trim() === '') {
        throw new Error(`${flag} 后面要给一个非空取值`)
      }
      if (flag === '--port') options.port = Number(value.trim())
      else options[flag === '--home' ? 'home' : 'exe'] = value.trim()
      index += 1
    } else {
      throw new Error(`不认识的参数：${flag}`)
    }
  }
  return options
}

/** 平台默认家（**权威在 `scripts/home/paths.js`**：`CELIRA_HOME_DIR` → 平台默认）。 */
function defaultHome() {
  const fromEnv = (process.env.CELIRA_HOME_DIR || '').trim()
  if (fromEnv !== '') return fromEnv
  return homePaths.DEFAULT_HOME
}

/** 打包形态的壳：release 优先，其次 debug（两颗都要**带内嵌产物**才作数，见下面那条形态判据）。 */
function findShellExe(explicit) {
  if (explicit) return fs.existsSync(explicit) ? explicit : null
  for (const candidate of [
    path.join(APP_DIR, 'src-tauri', 'target', 'release', 'celira-app.exe'),
    path.join(APP_DIR, 'src-tauri', 'target', 'debug', 'celira-app.exe'),
  ]) {
    if (fs.existsSync(candidate)) return candidate
  }
  return null
}

// ---------------------------------------------------------------------------
// 断言脚手架（与 `verify-ui.mjs` 同一套三态纪律）
// ---------------------------------------------------------------------------

const results = []
/**
 * `ok` **三态**：`true` 通过 / `false` 失败 / **`null` 不适用**。
 *
 * 判据与 `verify-ui.mjs` 逐字同款：**这条断言要的输入不在场 ⇒ N/A；输入在场而东西不对 ⇒ FAIL。**
 * 而"读不到"本身（页面空白、口子没答）是第三种 —— 它是这一轮真的坏了 ⇒ `false`。
 */
function check(name, ok, detail = '') {
  results.push({ name, ok })
  const tag = ok === null ? 'N/A ' : ok ? 'PASS' : 'FAIL'
  console.log(`${tag}  ${name}${detail ? `  — ${detail}` : ''}`)
}

function snippet(text, n = 160) {
  return String(text || '').replace(/\s+/g, ' ').trim().slice(0, n)
}

function notReady(message) {
  console.error(`\n环境没就绪：${message}`)
  console.error('（这不是代码坏了 —— 门禁按纪律报"不适用"，退出码 2。）')
  process.exit(2)
}

// ---------------------------------------------------------------------------
// 壳的进程与 CDP
// ---------------------------------------------------------------------------

let args
try {
  args = parseArgs(process.argv.slice(2))
} catch (error) {
  console.error(`[verify:ui:app] ${error.message}`)
  console.error(
    '用法: npm run verify:ui:app [--home <家目录>] [--exe <壳的可执行文件>] [--port <CDP 口>] [--keep-open]',
  )
  process.exit(2)
}

const home = args.home || defaultHome()
if (!fs.existsSync(home)) notReady(`家目录不在：${home}（用 --home 指一个，或先 node home.js up）`)

const cdpUrl = `http://127.0.0.1:${args.port}`
const cdpAlive = async () => {
  try {
    const response = await fetch(`${cdpUrl}/json/version`)
    return response.ok
  } catch {
    return false
  }
}

/** 进来之前世界在跑吗（决定跑完要不要把它停掉 —— **只收自己起的那些**）。 */
// ⚠ 端口**不写 8080**：它是家的家事实（`home.conf` 的 `listen_addr`），权威在 `paths.js`。
//   写死端口会让"世界本来在不在跑"在非默认端口的家上判错 —— 而这一格决定收尾动不动手。
const confFile = path.join(home, 'home.conf')
const imPort = homeConf.portOf(homeConf.confGet(confFile, 'listen_addr', homePaths.DEFAULT_LISTEN))
/** 远端跑法：`home.conf` 记着 `im_endpoint` ⇒ 本机这个口与这个家无关（去收它就是跨家误杀）。 */
const imEndpoint = (homeConf.confGet(confFile, 'im_endpoint', '') || '').trim()

const worldWasRunning = await (async () => {
  if (imEndpoint !== '') return false
  try {
    const response = await fetch(`http://127.0.0.1:${imPort}/health`, {
      signal: AbortSignal.timeout(1500),
    })
    return response.ok
  } catch {
    return false
  }
})()

/**
 * 进来时**世界的端口上站着谁**（收尾只收差集）。
 *
 * 为什么不能只看 `worldWasRunning`：那是一个健康读数，而收尾要的是"这个 pid 是不是我造成的"。
 * 差集（跑完 − 进来时）＋ 身份判据（见 `sweepWorldOrphan`）才既收得掉孤儿、又不误杀。
 */
const worldPidsBefore = imEndpoint === '' ? homeProc.listenersOnPort(imPort) : []

let shellProcess = null
let attachedToRunningShell = false
/**
 * 壳自己那份 sidecar 的布局 ＋ 进来时**本家的住户**有哪些（收尾只收差集）。
 *
 * 为什么要有住户这一格（2026-10-08 第二刀实测）：收尾原先只扫**世界**（`celira-im`），
 * 壳带起来的**住户**（`agent_host.exe`）留在盘上 —— 下一次冷启动时它们让这一次的世界/门起不来，
 * 于是门禁自己读成"读不到本机服务"（实测第二次跑 13/18，看着像代码坏了）。判据见 `sweepResidentOrphans`。
 *
 * ⚠ 基线必须在 `spawn` **之前**取：壳一起来就会带起住户，晚一步就把"它起的"算进基线 ⇒ 永远收不掉。
 */
let shellLayout = null
let hostPidsBefore = []

if (await cdpAlive()) {
  attachedToRunningShell = true
  console.log(`[verify:ui:app] 接上**已经在跑**的那颗壳（CDP ${cdpUrl}）—— 跑完不动它。`)
} else {
  // ★ **先问一句：这两个口现在有没有人在答**（2026-10-11 立，判据见 `doorSquatter`）：
  //   源码形态的门（`node home.js doors`）留在盘上时会占着它们，而壳会**复用它** ⇒
  //   这一轮量到的不是壳。宁可报"量不到"，也不拿别人的回答当壳的读数。
  const squatter = await doorSquatter()
  if (squatter) {
    notReady(
      `本机那两个口现在有人在答：${squatter} —— 这一轮量到的不会是壳。` +
        `多半是源码形态的门留着（\`node home.js doors\`）：先 \`node home.js down\`（那个家自己的），` +
        `或关掉正在跑的 app；然后再跑这一条。`,
    )
  }
  const exe = findShellExe(args.exe)
  if (!exe) {
    notReady(
      `找不到壳的可执行文件（找过 target/release 与 target/debug）—— 先 cd app && npm run build -- --no-bundle`,
    )
  }
  console.log(`[verify:ui:app] 壳 = ${exe}`)
  console.log(`[verify:ui:app] 家 = ${home}`)
  // 进来时本家的住户（收尾只收差集）—— 见上面那两个变量的说明，**必须在 spawn 之前**。
  shellLayout = shellLayoutFor(exe)
  hostPidsBefore = residentPidsNow(shellLayout)
  shellProcess = spawn(exe, [], {
    cwd: APP_DIR,
    // ⚠ `stdio: 'ignore'`：壳的输出走它自己的日志（`%APPDATA%` 那一份），
    // 而且**管道 stdio 在受限沙箱里会 EPERM** —— 这里不碰它。
    stdio: 'ignore',
    env: {
      ...process.env,
      CELIRA_HOME_DIR: home,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${args.port}`,
    },
  })
  const deadline = Date.now() + 60000
  while (Date.now() < deadline && !(await cdpAlive())) {
    await new Promise((resolve) => setTimeout(resolve, 500))
  }
  if (!(await cdpAlive())) notReady(`壳起来了但 CDP 没开（${cdpUrl}）—— 60 秒没等到`)
}

const browser = await chromium.connectOverCDP(cdpUrl)
const page = browser.contexts()[0].pages()[0]

/**
 * 等页面**落到一个状态**，别等一个时长。
 *
 * ⚠ 这一条是照 `verify-ui.mjs` 的 `waitForBoundaryText` 抄的教训（2026-10-05 我自己又踩了一次）：
 * 壳起来时页面先渲染「正在准备连接…」（会话自举），睡固定 4 秒会在那一刻读正文 ⇒
 * 把"还在启动"误报成"没落进应用"。**终态判据**：正文非空、且不再是那几个过渡态。
 *
 * ⚠ 2026-10-08 第二刀补一格：**本机服务面板自己那个过渡态**（「正在读本机服务…」）也进清单 ——
 * 它是**页内的一块**在等读数（壳命令 `local_services_readout`），与"整页还在自举"是两回事，
 * 但同一条纪律：**过渡态不当终态判**。见下面面板那三条断言前的 `panelSettled`。
 */
const TRANSIENT_TEXTS = ['正在准备连接', '正在读取', '正在准备…', '正在读本机服务']
async function waitForSettled(page, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs
  let body = ''
  while (Date.now() < deadline) {
    body = await page.locator('body').innerText()
    const transient = TRANSIENT_TEXTS.some((each) => body.includes(each))
    if (!transient && body.trim().length > 0) return body
    await page.waitForTimeout(400)
  }
  return body
}

/** 页面稳定下来（不是过渡态）再看正文。 */
async function settle(ms = 2500) {
  await page.waitForTimeout(ms)
  return page.locator('body').innerText()
}

let text = await waitForSettled(page)

// ---- 1. 形态判据本体 ------------------------------------------------------

const pageUrl = page.url()
const isPackagedForm = /^https?:\/\/tauri\.localhost(:|\/|$)/.test(pageUrl)
/**
 * ⚠ 两种"不是打包形态"要**分开说**（2026-10-05 记账：第一版把两者写成一句，害我多绕一轮）：
 * - 页面**根本没加载起来**（`chrome-error://` / 连不上 / 空白）⇒ 那是**壳没起来或它是 dev 形态**
 *   （dev 形态的壳会去连 vite 的 5173，没有 vite 在跑就是"拒绝连接"）—— 读数是"加载失败"，
 *   不是"形态不对"；
 * - 页面**加载了、但地址不是** `tauri.localhost`（例如 `localhost:5173`）⇒ 那才是"这一颗是 dev 形态"。
 */
const pageFailedToLoad = !/^https?:/.test(pageUrl)
check(
  '★ 形态判据：页面走的是**内嵌产物**（`http://tauri.localhost`），不是 vite 的 dev 地址',
  isPackagedForm,
  `URL=${pageUrl}` +
    (isPackagedForm
      ? ''
      : pageFailedToLoad
        ? ' ⇒ **页面根本没加载起来**：壳多半没起来，或那颗二进制是 dev 形态（它会去连 vite 的 5173）' +
          ' ⇒ cd app && npm run build -- --no-bundle（**别用裸 cargo build**：那样出来的永远是 dev 壳）'
        : ' ⇒ 这一颗是 dev 形态：cd app && npm run build -- --no-bundle（**别用裸 cargo build**）'),
)

// ---- 1b. **壳里嵌的那份产物是最新的吗**（2026-10-05 记账）------------------
//
// 打包形态把 `client/dist` **在编译期嵌进二进制**（`tauri/custom-protocol`）⇒ 改了页面只重建
// dist **不生效**：壳照旧供旧的那份，页面看起来"没改"。2026-10-05 我为这件事白验了两轮
// （页面照旧显示旧那句，我以为是代码没生效）。判据：**页面上跑的那个入口脚本名，必须与磁盘上
// `client/dist/index.html` 里写的一致** —— Vite 的产物名带内容哈希 ⇒ 名字不一致就是"嵌的是旧的"。
const distIndex = path.join(CLIENT_DIR, 'dist', 'index.html')
/**
 * ⚠ 只比**入口 `<script>` 标签**里那个产物名（2026-10-05 第一版比错过）：`page.content()` 给的是
 * **运行中的 DOM** —— Vite 的 preload 帮手会在跑起来之后往 `<head>` 里插一堆
 * `<link rel="modulepreload">`（懒加载那些 chunk）⇒ 拿"页面里出现过的所有 assets/*.js"去比，
 * 会把它们全算进来，**同一个入口也会被判成不一致**。静态入口只有一个，就比那一个。
 */
const ENTRY_PATTERN = /<script[^>]+src="([^"]*assets\/[^"]+\.js)"/g
const entriesOf = (html) => {
  const found = []
  for (const match of String(html).matchAll(ENTRY_PATTERN)) found.push(match[1])
  return found.map((each) => each.replace(/^.*\//, '')).sort()
}
if (!fs.existsSync(distIndex)) {
  check(
    '★ 壳里嵌的产物是最新的（页面入口脚本 == 磁盘上的 dist）',
    null,
    `没有 ${distIndex} —— 这一条无从判断（先 npm run build）`,
  )
} else {
  const onDisk = entriesOf(fs.readFileSync(distIndex, 'utf8'))
  const served = entriesOf(await page.content())
  const same = onDisk.length > 0 && served.length > 0 && onDisk.join('|') === served.join('|')
  check(
    '★ 壳里嵌的产物是最新的（页面入口脚本 == 磁盘上的 dist）',
    same,
    `页面=${served.join(',') || '(没找到入口脚本)'} · 磁盘=${onDisk.join(',') || '(没找到入口脚本)'}` +
      (same
        ? ''
        : ' ⇒ 嵌进去的是**旧产物**：cd app && npm run build -- --no-bundle（它会先重建 dist 再嵌）'),
  )
}

// ---- 2. 落点 --------------------------------------------------------------

/** 页面停在登录页 / 启动页 / 404 页 —— 三种都不是"落进应用"。 */
const LOGIN_TITLE = '连接你的世界'
const FIRST_RUN_TITLE = '你自己的世界'
const NOT_FOUND_TEXT = '这个地址还没有居民入住'
const RENDER_FAILURE_TITLE = '这个页面出错了'

if (text.includes(LOGIN_TITLE)) {
  check(
    '落进应用（不是登录页）',
    null,
    '这颗壳里记着**别的家**的会话（页面停在登录页）。本脚本不清你的浏览器数据 —— 在壳里登出一次，或换一颗干净的壳再跑',
  )
} else if (text.includes(FIRST_RUN_TITLE)) {
  check(
    '落进应用（不是首启那一问）',
    null,
    '这个家还没表过态（页面停在"你自己的世界"那一问）。本脚本**不替你按**那颗按钮（它会铸一位 Human、写机器级表态）—— 先手动打开 app 点一次再跑',
  )
} else {
  check(
    '落进应用：URL 在 /app 下 ＋ 主导航在场 ＋ 不是登录页 / 404 页 / 出错页',
    pageUrl.includes('/app') &&
      (await page.locator('nav[aria-label="主导航"]').count()) > 0 &&
      !text.includes(NOT_FOUND_TEXT) &&
      !text.includes(RENDER_FAILURE_TITLE),
    `URL=${pageUrl} · ${snippet(text, 80)}`,
  )
}

// ---- 3. 壳专属读数：本机服务面板 ------------------------------------------

async function goto(target) {
  await page.goto(`http://tauri.localhost${target}`, { waitUntil: 'domcontentloaded', timeout: 20000 })
  // ⚠ **状态等待，不是时长等待**（2026-10-08 修）。原先这里睡固定 3.5 秒，而本文件
  // 253 行那条 doctrine 就是"**等页面落到一个状态，别等一个时长**"——固定时长在这里
  // **按构造会误报**：状态口自己写着"实测约 3.7 秒"（`src/api/homeStatus.ts` 的超时注释），
  // 页面在那之前必然停在「正在读取运行情况…」⇒ 3.5 秒那一刻读到的正是过渡态，
  // 于是"读得慢"被报成"读不到"（2026-10-08 实测：机器页那几条就是这样红的）。
  // 换成状态等待**不削弱任何断言**：读数真坏了，等满 20 秒照样是过渡态 ⇒ 该红还是红；
  // 它去掉的只有"因为读得慢而误报"这一种假红。
  return waitForSettled(page, 20000)
}

/**
 * **先等两个口起来，再读机器页**（顺序是语义）：壳按 世界 → 状态口 → 配置门 的顺序起，
 * 而页面比它们先出现。若不先等，"门还没起"会被下面「模型」那一条读成"读不到模型配置"
 * ⇒ 把**启动慢**误报成**坏了**（2026-10-05 第一次跑就是这个形状）。
 * 等的是**状态**（端口答得上 200 就算起来）；等不到 ⇒ 后面几条照旧 FAIL（那是真坏）。
 */
const doorDeadline = Date.now() + 20000
while (Date.now() < doorDeadline) {
  if ((await probeDoor('http://127.0.0.1:5191/home-config', null)) === 200) break
  await new Promise((resolve) => setTimeout(resolve, 700))
}

let machineText = await goto('/app/runtimes/local')

/**
 * ⚠ **面板级的状态等待**（2026-10-08 补，同日第二刀**收紧**）。
 *
 * 本机服务面板有**两个**过渡态，判"读得到"之前两个都得过去：
 *  ① **还没挂上来**（`phase !== 'ready'` 之前什么都不渲染）；
 *  ② **挂上来了、读数还在路上** —— 那一帧正文里写着「正在读本机服务…」。
 *
 * 从前只等①（判据是 `/本机服务/` 出现），于是②那一帧会被**当成终态**判。2026-10-08 实测
 * （冷启动那一次，读数还没落定）：头一条「面板真的读得到」**假绿**（它要的 `本机服务` 与 `世界`
 * 两个字，面板的**说明文字**里就有），「退出时：读不到」那一格**在加载态也成立** ⇒ 也假绿，
 * 只有「Agent 那一行」如实红了 ⇒ 整条门禁读成 17/18，看着像代码坏了。
 *
 * 等的是**状态**（读数落定），不是时长；等不到 ⇒ 下面三条照旧 FAIL（**那是真坏**）。
 */
const panelDeadline = Date.now() + 20000
while (Date.now() < panelDeadline) {
  if (/本机服务/.test(machineText) && !/正在读本机服务/.test(machineText)) break
  machineText = await page.locator('body').innerText()
  await page.waitForTimeout(400)
}

/**
 * ★ **读数落定了吗** —— 并进下面三条断言（缺了它，加载态能蒙过两条，见上面那段实测）。
 *
 * 判据只有一条：面板那行过渡态文字不在了。**等不到落定就是 FAIL**（不是 N/A）：
 * 这一格的输入是"壳的命令答不答得上"，答不上来就是这一轮真的坏了。
 */
const panelSettled = !/正在读本机服务/.test(machineText)

/** **web 专属的那一句**：壳里出现它，就说明页面没走壳那条路（形态判据的反面）。 */
const WEB_ONLY_LINE = '这一格只有 app 里才有'

check(
  '★ 壳专属读数：本机服务面板真的读得到（且**不出现** web 专属那句"这一格只有 app 里才有"）',
  panelSettled && !machineText.includes(WEB_ONLY_LINE) && /本机服务/.test(machineText) && /世界/.test(machineText),
  !panelSettled
    ? '读数还没落定（面板还停在「正在读本机服务…」）'
    : machineText.includes(WEB_ONLY_LINE)
      ? '出现 web 专属那句 ⇒ 页面没走壳那条路'
      : snippet((machineText.match(/本机服务[\s\S]{0,120}/) || [''])[0], 120),
)
check(
  '本机服务面板：Agent 那一行读得到（住户数 / 老形态 / 还没有 Agent 三态之一）',
  panelSettled && /Agent/.test(machineText) && /(支在跑|这个家还是老形态|这台机器上还没有 Agent)/.test(machineText),
  // ⚠ 这里原先是按 `indexOf('Agent')` 取 120 字 —— 那**锚在面板的说明文字上**，红的时侯
  //   看不到真正那一行，于是没人查得动（2026-10-08 实测三次：一红一绿一红，读数都没用）。
  //   取**含 Agent 的那一行**整行：断言一个字没动，只是让红的那次说得出它在读什么。
  panelSettled
    ? snippet((machineText.match(/[^\n]*Agent[^\n]*/) || [''])[0], 140)
    : '读数还没落定（面板还停在「正在读本机服务…」）',
)
check(
  '本机服务面板：退出行为那一行读得到（停止 / 保持运行 / 读不到 三态之一）',
  panelSettled && /退出时：(停止本机服务|保持运行|读不到)/.test(machineText),
  // ⚠ 「读不到」本身是**终态**之一（那一格读不到就照实说读不到）—— 但它**在加载态也成立**，
  //   所以这一条必须并上 `panelSettled`，否则它会靠加载态假绿（2026-10-08 第二刀实测）。
  panelSettled
    ? snippet((machineText.match(/退出时：[^\n]*/) || [''])[0], 80)
    : '读数还没落定（面板还停在「正在读本机服务…」）',
)

/**
 * **未读角标**（2026-10-08）：真机上叫一次壳命令，读它回的**那句悬停文案**。
 *
 * 为什么这条只能在打包形态里验：托盘、任务栏红点、系统角标都是**壳**的东西，web 形态结构性没有。
 * 为什么断言返回值而不是截图：那句文案就是"壳到底收到没有"的**可核对读数** ——
 * 截任务栏只能看个大概，还随 DPI 变。
 *
 * ⚠ 两头都验（报 3 条要能看见、报回 0 要能消失），而且验完就**报回 0** —— 不留状态给后面的断言。
 */
const unreadProbe = await page.evaluate(async () => {
  const invoke = window.__TAURI_INTERNALS__?.invoke
  if (typeof invoke !== 'function') return { ok: false, reason: '页面里没有内建桥' }
  try {
    const withUnread = await invoke('set_unread_total', { count: 3 })
    const cleared = await invoke('set_unread_total', { count: 0 })
    return { ok: true, withUnread, cleared }
  } catch (error) {
    return { ok: false, reason: String(error && error.message ? error.message : error) }
  }
})
check(
  '★ 壳专属读数：未读角标（报 3 条 ⇒ 悬停那句说得出「3 条未读」；报回 0 ⇒ 它消失）',
  unreadProbe.ok === true &&
    typeof unreadProbe.withUnread === 'string' &&
    unreadProbe.withUnread.includes('3 条未读') &&
    typeof unreadProbe.cleared === 'string' &&
    !unreadProbe.cleared.includes('条未读') &&
    unreadProbe.cleared.startsWith('Celira —— '),
  unreadProbe.ok
    ? `报 3 = ${snippet(unreadProbe.withUnread, 60)} ｜ 报 0 = ${snippet(unreadProbe.cleared, 60)}`
    : `没验成：${unreadProbe.reason}`,
)

// ---- 4. 机器页两间（打包形态）＋「活动」明细的那一层下钻 ----------------------
//
// ⚠ **2026-10-12**：「活动」**不再是一个房间**（主人裁：不问也该知道的留在页面上，问了才知道的
//   交给问询）—— 明细降为「运行」那一间里**默认收起**的一层下钻。判据**不放松**：
//   仍然"或读到、或如实说读不到"，只是它在折叠的那一层里 ⇒ 这一条量**下钻在场 ＋
//   「运行」那一间说得出活动读数**（折叠着也能读到"活动次数"那一格）。
check(
  '机器页「运行」：当前状态读得到，且不是 web 专属那句',
  /当前状态/.test(machineText) && !machineText.includes(WEB_ONLY_LINE),
  snippet((machineText.match(/当前状态[\s\S]{0,60}/) || [''])[0], 100),
)

const activityOk =
  /最近的活动明细/.test(machineText) &&
  (/活动次数/.test(machineText) ||
    /还没有 Agent/.test(machineText) ||
    /它这一拍正在做事/.test(machineText))
check(
  '机器页「运行」：活动明细那一层下钻在场（默认收起）＋ 活动读数说得出（不许空白）',
  activityOk,
  snippet((machineText.match(/活动[\s\S]{0,80}/) || [''])[0], 100),
)

/** 今天那条缺陷的形状：配置门拒了壳自己的来源 ⇒ 页面只能说"这个来源不被允许"。 */
const ORIGIN_REFUSED_LINE = '这个来源不被允许'
/**
 * ⚠ 这一条**要的是"读到了"**，不是"没出现那句来源拒绝"（2026-10-05 第一次跑把它写松了）：
 * 只要门的判据是绿的，这一间就**必须**给出一份配置读数（清单 / 绑定 / "没有配置 ⇒ 走环境变量"）；
 * 说「读不到模型配置」就是坏 —— 无论它给的理由是"来源不被允许"还是"连不上那台机器"
 * （后者在真机上出现过：同一轮里配置门根本没答上来）。
 */
const MODEL_READ_LINES = ['那台机器上没有模型配置', '模型源', '清单', '绑定', '已配', '没有配']
const modelRead = MODEL_READ_LINES.some((each) => machineText.includes(each))
check(
  '★ 机器页「模型」：**读得到**（既不许"这个来源不被允许"，也不许"读不到模型配置"）',
  /模型/.test(machineText) && !machineText.includes(ORIGIN_REFUSED_LINE) && !machineText.includes('读不到模型配置') && modelRead,
  machineText.includes(ORIGIN_REFUSED_LINE)
    ? '出现「这个来源不被允许」⇒ 壳自己的来源又被自己的门拒了'
    : snippet((machineText.match(/模型[\s\S]{0,120}/) || [''])[0], 140),
)

/**
 * ★ **第一次那条路的第一步**（2026-10-11）—— 这一条**是产品形态特有的**，所以住在这里：
 *
 * 那张卡的在场判据是 `modelSourceAbsent`，而**两个门的载荷形状不一样**（`App.tsx` 那段注释
 * 记着这个坑）：web 门在文件不在时给 `sourcesState: 'absent'`，**壳门给 `'loaded'` ＋ 空表**
 * ⇒ 早先那个认 `'absent'` 的判据**在壳里恒不触发**（web 形态假绿）。所以"它在壳里也出得来"
 * 只有真机打包形态量得到。
 *
 * 判据写成**条件式**：模型那一间说"还没有" ⇒ 卡**必须**在场；主人配好之后那一间不再那么说，
 * 卡本就该消失 ⇒ 那一档报**不适用**（既不算通过也不算失败，与仓库同一条纪律）。
 *
 * ⚠ 前提字符串取自**页面自己那句话**（`Runtimes.tsx` 的 `模型` 空态：
 * 「那台机器上**还**没有模型配置 ⇒ Agent 起不来，先去配一条。」）—— 不在这里重写一份
 * `modelSourceAbsent`（那是第二份判据，仓库不许）。
 */
const noModelYet = machineText.includes('那台机器上还没有模型配置')
check(
  '★ 机器页：壳门说"这台机器上还没有模型源" ⇒ 「第一次」那张卡在场（动作排在读数之前）',
  !noModelYet || machineText.includes('先给这台机器配一条模型源'),
  noModelYet
    ? snippet(machineText, 120)
    : '这一轮模型那一间没说"还没有" ⇒ 不适用（配好之后它按设计消失）',
)

// ---- 5. 门的判据（带壳自己的来源头；含安全反例） --------------------------

/** 带一个来源头打一个只读端点，回状态码（连不上 ⇒ 0）。 */
async function probeDoor(url, origin) {
  const headers = origin ? { origin } : {}
  try {
    const response = await fetch(url, { headers, signal: AbortSignal.timeout(8000) })
    return response.status
  } catch {
    return 0
  }
}

/**
 * ★ **这两个口现在有没有人在答**（2026-10-11 立）：有 ⇒ 说一句人话，调用方据此拒绝跑。
 *
 * 为什么必须问这一句：源码形态的门（`node home.js doors` —— `home.js up` 与 web 探针都会起它）
 * 如果留在盘上，它会**占着 5190/5191**，而壳起来时会**复用它** ⇒ 这一轮量到的**根本不是壳**。
 * 实测（2026-10-11）：那条"外域来源打 PUT-only 端点 ⇒ 403（不是 405）"读到 **405** ——
 * 不是代码坏了，是答话的是**源码形态的门**，而两个生产者当时对"来源与方法谁先判"的顺序
 * **还不一样**（JS 先判方法、壳先判来源；那一刀已把 JS 改成与壳同形）。
 * ⇒ 判据：**读数要可信** —— 口被谁占着说不清时，宁可报"这一轮量不到壳"，
 * 也不许拿别人的回答当壳的读数（那正是"把读不到说成一个事实"）。
 */
async function doorSquatter() {
  const probes = [
    [5190, 'status'],
    [5191, 'home-config'],
  ]
  for (const [port, path] of probes) {
    const status = await probeDoor(`http://127.0.0.1:${port}/${path}`, null)
    if (status !== 0) return `127.0.0.1:${port} 已经在答（HTTP ${status}）`
  }
  return null
}

// ★ **门判据：跑那一份共用表**（2026-10-12）。
//
// 表在 `client/scripts/door-cases.mjs` —— **唯一权威位置**：源码形态的门
// （`node home.js doors`）与壳各有一份实现（两种形态各需要一个宿主，合不成一份），
// 所以判据落在"**同形**"上；这份表两个门禁都跑（`verify-ui.mjs` 量源码形态、
// 这里量壳），谁想加一条判据都只能加在那里 ⇒ **两边同时生效**，再也不会一边有一边没有。
// ⚠ 2026-10-11 真撞过一次不同形：来源那道与路由谁先判两边反着 ⇒ 外域来源打 `/home-binding`
//   在壳那边 403、在 JS 那边 405 ＋ `Allow: PUT`（JS 那一侧当天已改成与壳同形）。
const doorResults = await runDoorCases(
  { status: 'http://127.0.0.1:5190', config: 'http://127.0.0.1:5191' },
  SHELL_ORIGIN,
)
const doorWrong = doorCasesSayingWrongThing(doorResults)
check(
  '★ 门的六条判据（共用表：读口/写口认自己的来源 · PUT-only 405 ＋ Allow · 三条外域反例）',
  doorWrong.length === 0,
  doorWrong.length ? doorWrong.join(' · ') : describeDoorCases(doorResults),
)

// ---- 6. 全路由渲染冒烟（与 `probe sweep` 同一份清单） ---------------------

const pageErrors = []
page.on('pageerror', (error) => pageErrors.push(error.message))

/** 动态段用的探针值（照 `probe.mjs` 的 `selectRoutes`：只挑不需要真 id 的形状）。 */
function concretePath(routePath) {
  return routePath.replace(/:[A-Za-z]+/g, 'probe-value')
}

const sweep = ROUTES.map((route) => route.path).filter((each) => !each.includes('*'))
let sweepBad = 0
for (const routePath of sweep) {
  const target = concretePath(routePath)
  let body = ''
  try {
    await page.goto(`http://tauri.localhost${target}`, { waitUntil: 'domcontentloaded', timeout: 20000 })
    body = await settle(700)
  } catch (error) {
    sweepBad += 1
    console.log(`FAIL  路由 ${routePath} — 打不开：${error.message.split('\n')[0]}`)
    continue
  }
  const blank = body.trim().length === 0
  const crashed = body.includes(RENDER_FAILURE_TITLE)
  if (blank || crashed) {
    sweepBad += 1
    console.log(`FAIL  路由 ${routePath} — ${blank ? '正文空白' : '渲染出错页'}：${snippet(body, 100)}`)
  }
}
check(
  `全路由渲染冒烟（打包形态，${sweep.length} 条）：正文非空、不是出错页`,
  sweepBad === 0,
  sweepBad === 0 ? `${sweep.length} 条都渲染出来了` : `${sweepBad} 条有问题`,
)
check('全路由冒烟期间没有未捕获的页面错误（pageerror）', pageErrors.length === 0, pageErrors.slice(0, 2).join(' | '))

// ---------------------------------------------------------------------------
// 收尾
// ---------------------------------------------------------------------------

const passed = results.filter((each) => each.ok === true).length
const failed = results.filter((each) => each.ok === false)
const na = results.filter((each) => each.ok === null)

console.log('')
console.log(`=== 打包形态：${passed}/${results.length} 通过 · ${na.length} 条不适用 · ${failed.length} 条失败 ===`)
if (na.length > 0) {
  console.log('不适用（环境没就绪，不是代码问题 —— 别当成通过）：')
  for (const each of na) console.log(`  ~ ${each.name}`)
}

// 门禁不许空跑（仓库纪律：`PASSED` 但 `running 0 tests` 是空跑）。
if (results.length === 0) {
  console.error('★ 一条断言都没跑 —— 这是空跑，按失败处理。')
  process.exitCode = 1
}

if (!args.keepOpen) {
  await browser.close()
  if (shellProcess && !attachedToRunningShell) {
    shellProcess.kill()
    console.log('[verify:ui:app] 停掉本脚本起的壳（接上别人那颗时不动它）。')
  }
  // 只收**自己起的**那些：进来之前世界没在跑 ⇒ 把它停回去（住户另有一条，见下面那次调用）。
  if (!worldWasRunning && !attachedToRunningShell) {
    // ⚠ `home.js down` **收不掉壳起的那份世界**（2026-10-08 真机实测）：壳用的是**它自己那份
    //   sidecar**（`app/src-tauri/target/<profile>/bin/celira-im.exe`），而家的认领判据只认
    //   仓根那份 ⇒ `down` 按纪律**拒绝杀**（"不是我们的进程 —— 不杀"），如实报"端口仍被占着"、
    //   退出 3。**那条拒绝本身是对的**（跨家误杀是这套编排里最大的爆炸半径），所以这里不动家的
    //   判据，改由本脚本收**自己造成的那个差集**（判据见 `sweepWorldOrphan`）。
    const down = spawnSync('node', ['home.js', 'down'], { cwd: REPO_DIR, encoding: 'utf8' })
    if (down.status === 0) {
      console.log('[verify:ui:app] 世界本来没在跑 ⇒ 已把它停回去。')
    } else {
      // **不许静默**：这一次失败原先被 `stdio: 'ignore'` ＋「只在成功时打一行」吞掉，
      // 于是孤儿世界一声不响地留到下一次 `tauri build` 才炸（它锁着那份 sidecar 二进制）。
      const said = `${down.stdout ?? ''}${down.stderr ?? ''}`.trim().split('\n')
      console.log(`[verify:ui:app] home.js down 没成功（退出码 ${down.status}），它说：`)
      for (const line of said.slice(-6)) console.log(`    ${line.trim()}`)
    }
    await sweepWorldOrphan()
  }
  // 住户收尾：**与"世界本来在不在跑"无关** —— 只要这颗壳是本脚本起的，它带起来的住户就归本脚本收
  // （世界本来在跑时，壳照样会为这个家带起住户）。判据见 `sweepResidentOrphans`。
  await sweepResidentOrphans()
}

/**
 * **壳自己那份 sidecar 的布局** —— 与 `paths.js` 的 `detectLayout()` **不是同一份**。
 *
 * 为什么必须另造：`detectLayout()` 给的是**源码形态**的布局（住户在 `<仓根>/agent/target/release/`），
 * 而壳用的是**它自己那份** sidecar（`app/src-tauri/target/<profile>/bin/` —— 与 `sweepWorldOrphan`
 * 里那条"壳的 exe 在壳自己的 target 下"同一个理由）。
 * 而 `describeHostProcesses` 的指纹要求**可执行件路径逐字相等** ⇒ 不换这一格，本家的住户
 * 一支都认不出来，收尾会以为"没有孤儿"—— 那正是**假绿**。
 */
function shellLayoutFor(exe) {
  return {
    ...homePaths.detectLayout(),
    hostExe: path.join(path.dirname(path.resolve(exe)), 'bin', 'agent_host.exe'),
  }
}

/**
 * **家事实**：认领判据的第二维（`describeHostProcesses` 要的是对象，不是家目录那个字符串）。
 *
 * 形状照 `status.js` 那一份（同一类事实只有一处权威）：`{ homeDir, envDirs }` —— `envDirs` 是
 * **一支一个环境根**那一组（住户化之后一支一个进程），来源是 `residents.js` 的只读住户表。
 * ⚠ 缺了它 `proc.js` 会**炸**（明文写着"认领判据不许静默退回只看可执行件"）—— 那是对的：
 * 退回只看 exe 就会跨家误杀。这里如实让它炸、由调用方接住并说出来。
 */
function residentHomeFacts(homeDir) {
  const confFile = path.join(homeDir, 'home.conf')
  const list = homeResidents.runtimeResidents(homeDir, confFile)
  return { homeDir, envDirs: list.map((resident) => resident.envDir) }
}

/**
 * 这一刻**本家的住户** pid。
 *
 * **读不到就返回 `null`**（不是空表）：空表会被当成"一支都没有"，于是收尾以为"没有孤儿"
 * —— 与"读不到"混成同一件事，是这一格最容易出的假绿。
 */
function residentPidsNow(layout) {
  try {
    return homeProc.describeHostProcesses(layout, residentHomeFacts(home)).map((row) => row.pid)
  } catch (error) {
    console.log(
      `[verify:ui:app] ⚠ 住户清单读不出来（${String(error && error.message ? error.message : error)}）` +
        '—— 这一格这次收不了尾，如实说（下一次冷启动可能因此起不来）。',
    )
    return null
  }
}

/**
 * 收掉**本脚本造成的**那些孤儿住户（`agent_host.exe`）。
 *
 * 三条判据一起成立才杀（与 `sweepWorldOrphan` 同形；少一条就不杀 —— 「认不出就不杀」是这仓库的纪律）：
 * 1. 它的 pid **不在**进来时的基线里（基线 = spawn 之前那一刻本家的住户）；
 * 2. 它是**壳自己那份** sidecar（exe 逐字等于 `<壳目录>/bin/agent_host.exe`）；
 * 3. **家事实**对得上：它的 `--root` 落在这个家里（②③ 两条都在 `describeHostProcesses` 的指纹里）。
 *
 * ⚠ 基线读不到（`null`）⇒ **一支都不杀**：那时"哪些是我造成的"没有判据，宁可留着并说出来。
 */
async function sweepResidentOrphans() {
  if (!shellProcess || attachedToRunningShell || !shellLayout) return
  if (hostPidsBefore === null) return
  const before = hostPidsBefore
  let rows = []
  try {
    rows = homeProc.describeHostProcesses(shellLayout, residentHomeFacts(home))
  } catch (error) {
    console.log(
      `[verify:ui:app] ⚠ 住户清单读不出来（${String(error && error.message ? error.message : error)}）` +
        '—— 没敢动手（认不出就不杀）。',
    )
    return
  }
  const doomed = rows.filter((row) => !before.includes(row.pid))
  for (const row of doomed) {
    await homeProc.killPid(row.pid, { force: true })
    console.log(
      `[verify:ui:app] 收掉壳起的孤儿住户（pid ${row.pid}，${row.name}）—— 壳被硬杀时它不会自己走。`,
    )
  }
  console.log(
    doomed.length === 0
      ? `[verify:ui:app] 住户收尾：进来时 ${before.length} 支，这一轮没有多出来的。`
      : `[verify:ui:app] 住户收尾：进来时 ${before.length} 支，收掉 ${doomed.length} 支。`,
  )
}

/**
 * 收掉**本脚本造成的**那个世界孤儿。
 *
 * 三条判据一起成立才杀（少一条就不杀 —— 「认不出就不杀」是这个仓库的纪律）：
 * 1. 它的 pid **不在**进来时的基线里（基线 = 本家 im 端口上原来站着的那些）；
 * 2. 它的可执行件在**壳自己的** target 目录下 —— 这一条把"用户自己 `node home.js up` 起的
 *    那份"排除在外（那份在仓根 `target/` 下），于是收尾不会碰到别人的世界；
 * 3. 进程名是 `celira-im`。
 */
async function sweepWorldOrphan() {
  if (imEndpoint !== '') {
    console.log(`[verify:ui:app] 这个家用的是远端世界（${imEndpoint}）⇒ 本机端口不动。`)
    return
  }
  const shellTarget = path.join(APP_DIR, 'src-tauri', 'target')
  for (const pid of homeProc.listenersOnPort(imPort)) {
    if (worldPidsBefore.includes(pid)) continue
    const info = homeProc.describeProcess(pid)
    const name = String(info?.name ?? '')
    const exe = String(info?.exePath ?? '')
    const ours = /^celira-im/i.test(name) && isUnder(exe, shellTarget)
    if (!ours) {
      console.log(
        `[verify:ui:app] 世界端口上多了一支（pid ${pid}，${name || '认不出'}${exe ? `（${exe}）` : ''}）——` +
          '认不出是壳起的那份 ⇒ 不杀（要停它请到那个家去停）',
      )
      continue
    }
    await homeProc.killPid(pid, { force: true })
    console.log(`[verify:ui:app] 收掉壳起的那个孤儿世界（pid ${pid}）—— 壳被硬杀时它不会自己走。`)
  }
  await homeProc.sleep(300)
  if (homeProc.portInUse(imPort)) {
    console.log(
      `[verify:ui:app] ⚠ 世界端口 ${imPort} 仍被占着 —— 下一次 tauri build 可能撞「failed to remove file」。`,
    )
  }
}

/** `child` 是不是在 `dir` 底下（大小写不敏感：Windows 上路径大小写不构成区分）。 */
function isUnder(child, dir) {
  if (!child || !dir) return false
  const relative = path.relative(path.resolve(dir), path.resolve(child))
  return relative !== '' && !relative.startsWith('..') && !path.isAbsolute(relative)
}

if (failed.length > 0) process.exitCode = 1
