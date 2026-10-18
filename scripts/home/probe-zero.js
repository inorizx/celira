#!/usr/bin/env node
'use strict'
/**
 * **从 0 那条真机门禁**：一个全新用户的第一小时，走一遍（2026-10-09）。
 *
 * ## 为什么要有它
 *
 * 另外两条真机门禁各量一半：`probe-loop.js` 量「人发一条 ⇒ 她回」（**模型已经配好了**），
 * `probe-ui.js` 量页面（**世界已经喂饱了**）。**没有一条量"从 0"**：起一个**全新的家**
 * （没有任何模型配置）→ 在**页面里配模型** → 发消息 → 她回话。
 *
 * 2026-10-09 主人让手工跑一遍，**当场抓出两个套件看不见的缺陷**：
 *   ① 配完**不重启吃不到**（宿主启动时"听进程侧"，而唤醒边界只对"已经听文件"的宿主重读）；
 *   ② **差一拍**（边界检查在循环顶上，而宿主阻塞在等铃里 ⇒ 配完第一条消息仍用旧的）。
 * 两条都只在"真起一个家 ＋ 页面里配一次 ＋ 发一条"的时候才暴露 ⇒ 它们必须有门禁，
 * 否则下一次还是靠人手工摸索（那次摸索了两个多小时，中间还夹着一条代理的假象）。
 *
 * ## 它做什么（**全程不碰真家**）
 *
 * 1. 临时目录里 `node home.js up`（世界 ＋ 铸主人 ＋ 常驻 Agent ＋ 两道门）；
 * 2. 起宿主——此刻家里**没有任何模型配置** ⇒ 她按**用户环境变量那一路**想（多半想不出来：
 *    那一路的钥匙在这台机器上可能是空的/失效的 ⇒ 这一段**故意**留着，它是"配之前"的读数）；
 * 3. 起静态页（构建产物 ＋ 代转）与模型配置门；浏览器登录；
 * 4. **页面里配模型**：左栏「自定义」→ 手填端点 → 贴钥匙 → 挑型号 → 手填窗口 → 保存；
 * 5. 核对落盘：`<家>/model_sources.json` 有那一条、`<环境根>/model_binding.json` 指向它；
 * 6. **页面里发一条** ⇒ 等她回（**不重启**）；**再发一条**（用来分辨"差一拍"与"根本不通"）；
 * 7. 收尾：`down` ＋ 删掉整个临时家。
 *
 * ## 它量什么、不量什么
 *
 * - **量**：页面保存真的落盘；宿主**不用重启**就换了装配；**下一段清醒期**就用新的（第 1 条
 *   就回话）。
 * - **不量**：模型答得好不好、页面长相与文案、别的 App、门铃的重连与退避（`probe-loop`）、
 *   **重启之后照旧能回**（同样是 `probe-loop`：它起的就是一个"已经配好"的家）。
 *
 * ⚠ **读数怎么读**（门铃是**合并**的：通道容量 1，"一次醒来可清一批"）：她可能**一次答掉两条**
 * 消息 ⇒ 第 2 条那一格读成 0 条**不代表她没答**——**看第 1 条那一格**。判决只认一件事：
 * **配完之后的第一段清醒期有没有用上新的**。
 *
 * ## ⚠ 2026-10-10（在场那一刀）之后，这条判据的**射程**要读准
 *
 * 「下一段清醒期」＝「她**下一次开新段**」。⚠ 而**一段在场如今会跨多声铃**（`agent/README.md`
 * §3.66）：装配在 Run 内钉住 ⇒ **她正在场时改配置，改动要等她这一段收场之后才生效**——
 * 那是**设计**，不是"差一拍"。
 *
 * 本探针的场景**恰好避开了那一格**：`配之前` 那条消息让她的那一段**收场**（没有模型 ⇒
 * `system:model-failed`），此后到 `配之后·第 1 条` 之间**没有唤醒** ⇒ 她**不在场** ⇒ 第 1 条
 * 必然开新段、必然用新装配。**因此本探针的判据与场景都照旧成立，一个字不用改。**
 *
 * ⚠ **它量不到的**（如实登记）：**"她在场时改了配置"**那一路——本探针的场景里她不在场。
 * 要量那一路得另起一条（让她先搁下、再改配置、再发消息，看她**这一段**用不用新的）。
 * **触发条件**：真出现"在场期间改了配置"的读数、或那一刀（把配置变化**告诉她**）落地时。
 *
 * ## 用法与退出码
 *
 * ```text
 * node scripts/home/probe-zero.js [--keep] [--timeout <秒>]
 *                                 [--model-sources <清单路径>] [--model-source-id <条目 id>]
 *                                 [--effort <档>]
 * ```
 *
 * - `--model-sources <清单路径>`：**页面上要填什么**从这一条来（缺省取本机默认家的清单）；
 * - `--model-source-id <id>`：用清单里哪一条（缺省第一条）；
 * - `--effort <档>`：配完顺手写进清单的思考强度（缺省 `max`；`--effort none` 表示不写这一格）；
 * - `--keep`：跑完留着临时家（排查用；⚠ 里面有那份清单的副本，含钥匙 ⇒ 自己删）；
 * - 退出码：**0** = 从 0 走通（配完**第 1 条**就回）／ **2** = 装配不起来、这台机器上没有任何
 *   模型来源（**不适用**，不是失败）、或**配之前她就回了**（前提不成立）／ **3** = 配完仍不回话，
 *   或**只有第 2 条回**（那正是"差一拍"这个缺陷的形状）。
 *
 * ## ⚠ 环境前置（跑之前先满足，否则红得看不懂）
 *
 * 1. **页面是构建产物**：`client/dist` 要在（`cd client && npm run verify` 会重建它）——
 *    本探针**不**替你构建；
 * 2. **这台机器上有一份清单**（`<默认家>/model_sources.json`，或用 `--model-sources` 指一份）：
 *    它是"用户在页面上要填什么"的来源（端点 / 型号 / 钥匙 / 窗口）。清单里那一条**不会被**
 *    写进临时家（那正是本门禁要验的：**页面**把它写进去）；
 * 3. **代理那一格**：若这台机器设了 `HTTP(S)_PROXY`，**宿主的模型请求会经代理**，而有些代理会
 *    截断大回信（症状：`transport-failure:io` / `transport-failure:timeout`，而 CLI 探针直连是好的）。
 *    这台机器实测过 ⇒ 跑之前给模型那家加一条 bypass（`NO_PROXY`），或摘掉这两个变量。
 *
 * ⚠ 它花掉**真模型调用**（三条消息 = 至多三次调用）。触发条件写在 `AGENTS.md`「验证与汇报」。
 *
 * 纪律：token 与钥匙**只在本进程里**（不打印、不落盘到别处）；页面 dump 一律打码。
 */

const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { spawn, spawnSync } = require('node:child_process')

const modelSources = require('./model-sources')
const { worldBaseOf } = require('./conf')
const paths = require('./paths')
const { chooseModelSource, describeModelSource } = require('./probe-loop')

const REPO_DIR = path.resolve(__dirname, '..', '..')
const HOME_JS = path.join(REPO_DIR, 'home.js')
const SERVE_CLIENT = path.join(REPO_DIR, 'scripts', 'serve_client.mjs')
const PLAYWRIGHT = path.join(REPO_DIR, 'client', 'node_modules', 'playwright')
// ⚠ **世界地址由那个家自己说了算**（2026-10-11）：`up` 建新家时会避开被占的口，并把挑到的
// 那一个写进**那个家自己的** `home.conf`（见 `conf.worldBaseOf`）。从前这里写死
// `127.0.0.1:8080` ⇒ 主人开着自己的家时这条门禁**根本跑不了**；而临时世界起在 8081、
// 探针却去跟 8080 上的**别人的世界**说话，是更糟的那一种形状。起来之后才赋值。
let BASE = null
const UI_PORT = 4173

/** 参数解析（未知选项**明确失败**，不静默忽略）。 */
function parseArguments(argv) {
  const options = {
    keep: false,
    timeoutSeconds: 90,
    modelSources: null,
    modelSourceId: null,
    effort: 'max',
  }
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index]
    if (flag === '--keep') {
      options.keep = true
    } else if (flag === '--timeout' || flag === '--model-sources' || flag === '--model-source-id' || flag === '--effort') {
      const value = argv[index + 1]
      if (typeof value !== 'string' || value.trim() === '' || value.startsWith('--')) {
        throw new Error(`${flag} 后面要给一个非空取值`)
      }
      if (flag === '--timeout') {
        if (!/^\d+$/.test(value.trim()) || Number(value.trim()) <= 0) {
          throw new Error('--timeout 后面要给一个正整数（秒）')
        }
        options.timeoutSeconds = Number(value.trim())
      } else if (flag === '--model-sources') {
        options.modelSources = value.trim()
      } else if (flag === '--model-source-id') {
        options.modelSourceId = value.trim()
      } else {
        options.effort = value.trim() === 'none' ? null : value.trim()
      }
      index += 1
    } else {
      throw new Error(`不认识的参数：${flag}`)
    }
  }
  return options
}

/**
 * **这一轮的判决**（纯函数 ⇒ 可判错）：三段读数 ⇒ 一个结论 ＋ 退出码。
 *
 * 判据一句话：**配完之后的第一段清醒期就该用新的**（那是页面那句"改动只影响下一次活动：改完
 * 不用重启"的字面意思）。三段各自独立，不许互相顶替：
 *   · 配**之前**回了 ⇒ 这一轮测不出"配置生效"这件事（环境不对，报 2 让人看清，别当通过）；
 *   · 配之后**第 1 条**回 ⇒ 完全兑现（**这一条就够判**，别管第 2 条）；
 *   · 只有第 2 条回 ⇒ **差一拍**（那正是这个缺陷的形状，如实报 3）。
 */
function zeroVerdict({ before, first, second }) {
  if (before > 0) {
    return { code: 2, reading: '配**之前**她就回了 ⇒ 这一轮测不出"配置生效"（环境不对，不是通过）' }
  }
  if (first > 0) {
    return { code: 0, reading: '**配完第 1 条就回** ⇒ 不用重启、下一段清醒期就用新的（完全兑现）' }
  }
  if (second > 0) {
    return {
      code: 3,
      reading: `**差一拍**（第 1 条 ${first} 条、第 2 条 ${second} 条）——配完的下一段清醒期没用上新的`,
    }
  }
  return { code: 3, reading: '**两段都没回**（配之前 / 配之后都不回）' }
}

const redact = (text) =>
  String(text || '')
    .replace(/oc_sk_[A-Za-z0-9_-]+/g, '<钥匙已打码>')
    .replace(/sk-[A-Za-z0-9_-]{8,}/g, '<钥匙已打码>')
const say = (...args) => console.log('[probe-zero]', ...args)
const flat = (text, limit = 400) => redact(text).replace(/\s+/g, ' ').trim().slice(0, limit)

const runHome = (args, homeDir) =>
  spawnSync(process.execPath, [HOME_JS, ...args], {
    cwd: REPO_DIR,
    encoding: 'utf8',
    env: { ...process.env, CELIRA_HOME_DIR: homeDir },
  })

async function waitForHttp(url, timeoutMs = 25000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    try {
      if ((await fetch(url)).ok) return true
    } catch {
      /* 还没起来 */
    }
    await new Promise((resolve) => setTimeout(resolve, 400))
  }
  return false
}

async function api(token, method, url, body) {
  const headers = { Authorization: `Bearer ${token}` }
  if (body !== undefined) {
    headers['content-type'] = 'application/json'
    headers['Idempotency-Key'] = `probe-zero-${Date.now()}-${Math.random().toString(16).slice(2)}`
  }
  const response = await fetch(`${BASE}${url}`, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const text = await response.text()
  let json = null
  try {
    json = JSON.parse(text)
  } catch {
    json = null
  }
  return { status: response.status, json, text }
}

/**
 * 页面里发一条，然后等她**对这一段**回话（返回她新说的那几条）。
 *
 * ⚠ **只算"这一段之后她新说的"**（按消息 id 与发之前那份快照比）：门铃是**合并**的
 * （通道容量 1，"一次醒来可清一批"），所以一段清醒期可能一次答掉好几条消息 ⇒ 若按"房里她的
 * 全部消息"数，后面几段会读成"她没回"，其实是她已经答过了 —— 那是**读数**在骗人，不是她没回。
 */
async function talkAndWait(page, token, roomId, agentId, line, seconds) {
  const before = await api(token, 'GET', `/v1/rooms/${roomId}/messages?limit=50`)
  const known = new Set(
    ((before.json && before.json.messages) || []).filter((each) => each.actor && each.actor.id === agentId).map((each) => each.id),
  )
  const composer = page.locator('textarea').last()
  if (await composer.count()) {
    await composer.click()
    await composer.fill(line)
    await page.keyboard.press('Enter')
    say(`页面里发了：${line}`)
    // ⚠ **先验自己那条真的落进世界了没有**（2026-10-09 加：一次偶发红就是卡在这里 ——
    // 页面这一次没发出去，而探针只说"她没回"，把"我没发出去"读成了"她不回"✗）。
    let landed = false
    const landedBy = Date.now() + 20000
    while (Date.now() < landedBy) {
      const listed = await api(token, 'GET', `/v1/rooms/${roomId}/messages?limit=50`)
      const messages = (listed.json && listed.json.messages) || []
      if (messages.some((each) => each.body === line && each.actor && each.actor.id !== agentId)) {
        landed = true
        break
      }
      await page.waitForTimeout(2000)
    }
    if (!landed) {
      say('⚠ **我那条没落进世界**：页面这一次没发出去 ⇒ 后面读到 0 条**不算她的错**（这一轮没验到唤醒）')
    }
  } else {
    say('⚠ 输入区没找到 —— 这一条改走公开协议发（页面那一半这一轮没验到）')
    await api(token, 'POST', `/v1/rooms/${roomId}/messages`, {
      body: line,
      reply_to_message_id: null,
      attachments: [],
      mentions: [],
    })
  }
  const deadline = Date.now() + seconds * 1000
  let fresh = []
  while (Date.now() < deadline) {
    const listed = await api(token, 'GET', `/v1/rooms/${roomId}/messages?limit=50`)
    const messages = (listed.json && listed.json.messages) || []
    fresh = messages.filter((each) => each.actor && each.actor.id === agentId && !known.has(each.id))
    if (fresh.length > 0) break
    await page.waitForTimeout(4000)
  }
  return fresh
}

async function main() {
  let options
  try {
    options = parseArguments(process.argv.slice(2))
  } catch (error) {
    console.error(`[probe-zero] ${error.message}`)
    console.error(
      '用法: node scripts/home/probe-zero.js [--keep] [--timeout <秒>] [--model-sources <路径>] [--model-source-id <id>] [--effort <档>]',
    )
    return 2
  }

  // 前置：这台机器上**有一份清单**（页面里要填什么从它来；它**不会**被写进临时家）
  const listFile = options.modelSources || modelSources.listPath(paths.homeDir())
  if (!fs.existsSync(listFile)) {
    console.error(`[probe-zero] 这台机器上没有模型清单（${listFile}）⇒ 没有"页面上要填什么"⇒ **不适用**（不是失败）。`)
    console.error('[probe-zero] 在「模型配置」页加一条，或者用 --model-sources 指一份。')
    return 2
  }
  let entry
  try {
    const picked = chooseModelSource(
      modelSources.normalizeEntries(JSON.parse(fs.readFileSync(listFile, 'utf8'))),
      options.modelSourceId,
    )
    if (!picked.ok) {
      console.error(`[probe-zero] ${picked.reason}`)
      console.error(`[probe-zero] 清单：${listFile}`)
      return 2
    }
    entry = picked.entry
  } catch (error) {
    console.error(`[probe-zero] 清单读不出来（${listFile}）：${error.message}`)
    return 2
  }
  say(`页面上要填什么：${describeModelSource(entry)}（**钥匙不打印**）`)
  if (!fs.existsSync(path.join(REPO_DIR, 'client', 'dist', 'index.html'))) {
    console.error('[probe-zero] 没有 client/dist ⇒ 页面起不来（先 `cd client && npm run verify`）—— 不适用')
    return 2
  }

  const homeDir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-zero-probe-'))
  const children = []
  let browser = null
  say(`临时家 = ${homeDir}（**全程不碰真家**）`)
  try {
    // 1) 起世界 ＋ 铸主人 ＋ 常驻 Agent ＋ 两道门
    const up = runHome(['up'], homeDir)
    if (up.status !== 0) {
      console.error(`[probe-zero] 那个家起不来（exit=${up.status}）：${flat(up.stdout, 300)}`)
      return 2
    }
    // **世界地址现在才知道**：那个家刚把挑到的口写进它自己的 `home.conf`。
    // ⚠ 读不到就**拒绝跑**——绝不回落到默认的 8080（那上面可能是主人正开着的家）。
    BASE = worldBaseOf(homeDir)
    if (!BASE) {
      console.error(
        '[probe-zero] 读不到这个家自己的世界地址（`home.conf` 的 `listen_addr`）⇒ **不跑**：' +
          '默认的 8080 上可能是别人的家，拿它做实验不算数',
      )
      return 2
    }
    const minted = runHome(['up', '--token'], homeDir)
    const token = (/\[token\]\s*(\S+)/.exec(minted.stdout) || [])[1]
    if (!token) {
      console.error('[probe-zero] 取不到 Human token —— 装配不起来')
      return 2
    }
    const status0 = runHome(['status'], homeDir)
    const resident = /\[resident\]\s+([0-9a-f-]{36})（住户 1，房 ([0-9a-f-]{36})）/.exec(String(status0.stdout || ''))
    if (!resident) {
      console.error(`[probe-zero] 认不出常驻 Agent：${flat(status0.stdout, 300)}`)
      return 2
    }
    const [, agentId, roomId] = resident
    say(`常驻 Agent = ${agentId.slice(0, 8)}… ｜ 它的房 = ${roomId.slice(0, 8)}…（宿主随「up」已经起来）`)

    // 页面上要点的那个名字，**从世界里现场读**（`GET /v1/agents`）。
    // 这里从前写死字面量 `小满` —— 那是把门禁绑在"布置的默认名"上：默认名一改（2026-10-12，
    // 系统不再替 Agent 起名，没给 `--agent-name` 就是占位 `未命名`）门禁就红，而且红得看不懂。
    // 名字的权威在世界里那一支身上；本门禁只负责"点它"，不负责"它叫什么"。
    const agentsView = await api(token, 'GET', '/v1/agents')
    const mine = ((agentsView.json && agentsView.json.agents) || []).find(
      (each) => each && each.agent && each.agent.id === agentId,
    )
    const agentName = mine && mine.agent && mine.agent.display_name ? mine.agent.display_name : ''
    if (!agentName) {
      console.error(
        `[probe-zero] 读不到那一支的显示名（GET /v1/agents → ${agentsView.status}）—— 装配不起来`,
      )
      return 2
    }
    say(`它的显示名 = ${agentName}`)

    // 2) 页面（构建产物 ＋ 代转）与模型配置门
    const ui = spawn(
      process.execPath,
      [SERVE_CLIENT, '--dist', 'client/dist', '--port', String(UI_PORT), '--im', BASE, '--home', homeDir],
      { cwd: REPO_DIR, stdio: 'ignore' },
    )
    children.push(ui)
    if (!(await waitForHttp(`http://127.0.0.1:${UI_PORT}/`))) {
      console.error(`[probe-zero] 静态页服务没起来（:${UI_PORT}）—— 不适用`)
      return 2
    }
    const doors = spawn(process.execPath, [HOME_JS, 'config', '--serve'], {
      cwd: REPO_DIR,
      stdio: 'ignore',
      env: { ...process.env, CELIRA_HOME_DIR: homeDir },
    })
    children.push(doors)
    await new Promise((resolve) => setTimeout(resolve, 1500))

    const { chromium } = require(PLAYWRIGHT)
    browser = await chromium.launch()
    const page = await browser.newPage()
    page.setDefaultTimeout(10000)
    page.on('response', async (response) => {
      const url = response.url()
      if (!/home-config|home-binding/.test(url)) return
      let body = ''
      try {
        body = flat(await response.text(), 160)
      } catch {
        body = '<读不到回信>'
      }
      say(`  ⇄ HTTP ${response.status()} ${url.replace(/^https?:\/\/[^/]+/, '')} → ${body}`)
    })

    // 3) 登录
    await page.goto(`http://localhost:${UI_PORT}`)
    await page.waitForTimeout(1500)
    await page.locator('input[type="password"], input[name="token"], input#token').first().fill(token)
    await page.getByRole('button', { name: '连接', exact: true }).click()
    await page.waitForTimeout(2500)
    await page.getByText(agentName).first().click()
    await page.waitForTimeout(2000)

    // 4) **配之前**发一条（这一段是读数，不是判据）
    const before = await talkAndWait(page, token, roomId, agentId, '配之前先问一句：你在吗？', 45)
    say(`【配之前】她回了 ${before.length} 条（多半是 0：这一路还没配模型）`)

    // 5) 页面里配模型：「自定义」＝手填端点那条降级通道（预设那条路归 probe-ui 量）
    //
    // ⚠ **2026-10-14 修陈旧**：这一行从前跳 `/settings/providers`（不带机器上下文）——
    //    而 2026-10-11 起「模型配置」**按机器寻址**：旧地址**没有上下文时落执行端列表**
    //    （`routeRegistry.ts` 那一行原文："带 ?home= 落那台；没有上下文落执行端列表，不猜"）
    //    ⇒ 页面上根本没有「自定义」那一列，探针卡在 `locator.click` 超时（**不是产品坏了**）。
    //    带 `?home=local` 是那条路本来给的"老书签不破"入口（`verify-ui` 有判据钉着它）。
    await page.goto(`http://localhost:${UI_PORT}/settings/providers?home=local`)
    await page.waitForTimeout(2000)
    await page.getByText('自定义', { exact: true }).first().click()
    await page.waitForTimeout(800)
    await page.getByPlaceholder(/https:\/\/…\/v1\/chat\/completions/).fill(entry.endpoint)
    await page.getByPlaceholder(/粘贴这一家的 API Key/).fill(entry.api_key)
    await page.getByPlaceholder(/例如 deepseek-chat/).fill(entry.model)
    await page.getByPlaceholder(/留空 = 保存时自动获取/).fill(String(entry.context_window || 65536))
    await page.getByRole('button', { name: /^保存/ }).first().click()
    await page.waitForTimeout(3500)

    const listPath = path.join(homeDir, 'model_sources.json')
    const bindingPath = path.join(homeDir, 'agents', agentId, 'model_binding.json')
    const savedList = fs.existsSync(listPath) ? JSON.parse(fs.readFileSync(listPath, 'utf8')) : null
    const savedBinding = fs.existsSync(bindingPath) ? JSON.parse(fs.readFileSync(bindingPath, 'utf8')) : null
    say(`落盘：清单 ${savedList ? `${savedList.length} 条（${savedList[0].id}）` : '**没有**'} ｜ 绑定 ${savedBinding ? savedBinding.source : '**没有**'}`)
    if (!savedList || !savedBinding) {
      console.error('[probe-zero] 页面保存**没有落盘** ⇒ 这一格没验成（门或页面那条路）—— 如实报')
      return 3
    }
    // 思考强度：页面上没有那一格（门的写接口也不认它）⇒ 手写清单，如实说
    if (options.effort) {
      savedList[0].reasoning_effort = options.effort
      fs.writeFileSync(listPath, `${JSON.stringify(savedList, null, 2)}\n`)
      say(`思考强度 = ${options.effort}（**手写清单**：页面上没有那一格、门的写接口也不认它）`)
    }

    // 6) **配之后**：不重启，连发两条 —— 第 1 条是判据，第 2 条用来分辨"差一拍"与"根本不通"
    await page.goto(`http://localhost:${UI_PORT}`)
    await page.waitForTimeout(2500)
    await page.getByText(agentName).first().click()
    await page.waitForTimeout(2000)
    const first = await talkAndWait(page, token, roomId, agentId, '现在呢？看到就回我一句。', options.timeoutSeconds)
    say(`【配之后·第 1 条】她回了 ${first.length} 条${first.length === 0 ? '（还在等）' : ''}`)
    const second = await talkAndWait(page, token, roomId, agentId, '再问一次：你在吗？', options.timeoutSeconds)
    say(`【配之后·第 2 条】她回了 ${second.length} 条（⚠ 门铃是合并的：她一次答掉两条时这里是 0，看上一格）`)

    const hostLog = path.join(homeDir, 'logs', 'agents', agentId, 'host.out')
    if (fs.existsSync(hostLog)) {
      const tail = fs.readFileSync(hostLog, 'utf8').split('\n').slice(-10)
      say(`宿主日志尾：\n${tail.map((each) => `    ${flat(each, 180)}`).join('\n')}`)
    }
    for (const message of [...first, ...second]) say(`她说的：${flat(message.body, 200)}`)

    const verdict = zeroVerdict({ before: before.length, first: first.length, second: second.length })
    say(`结论：${verdict.reading}`)
    return verdict.code
  } finally {
    if (browser) await browser.close().catch(() => {})
    for (const child of children) {
      try {
        child.kill()
      } catch {
        /* 已退出 */
      }
    }
    runHome(['down'], homeDir)
    if (options.keep) {
      say(`--keep：临时家留着（${homeDir}）—— ⚠ 里面有那份清单的副本（含钥匙），排查完自己删`)
    } else {
      try {
        fs.rmSync(homeDir, { recursive: true, force: true })
        say('临时家已删')
      } catch (error) {
        say(`临时家没删掉：${error.message}`)
      }
    }
  }
}

if (require.main === module) {
  main()
    .then((code) => {
      process.exitCode = typeof code === 'number' ? code : 2
    })
    .catch((error) => {
      console.error(`[probe-zero] 意外失败：${error && error.stack ? error.stack : error}`)
      process.exitCode = 2
    })
}

// 只有这两个是给测试的（纯函数）；真机部分全是副作用，测试里不碰。
module.exports = { parseArguments, zeroVerdict }
