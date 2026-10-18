#!/usr/bin/env node
/**
 * **她看得见吗** —— 主体直接感知媒体材料的端到端探针（2026-10-17，提案 §11 的处方）。
 *
 * ## 要证的那一句
 *
 * **运行中的宿主真的把图片折进了请求，而模型看得见。**
 *
 * 此前分开证过三段：① 我们的 wire 形状供应商收（2026-10-17 一手实测 `deepseek-v4.1-flash`，
 * 红图 ⇒ 200 ＋ "This is a solid red square."）；② 我们的代码从一块材料产出那个形状（两条 wire
 * 的单元用例）；③ 材料从 `im_read_attachment` 走到块（`agent/tests/im_app.rs`）。
 * **没证的就是这三段在活着的宿主里接起来** —— 这一支探针只干这一件事。
 *
 * ## 路线与夹具
 *
 * - 走 **`im_read_attachment`（房间附件）**，**不走资料面**：资料面 `files_read` 同形是登记在案的
 *   **未做**项（`agent/README.md` §6 解冻 ④ 的「未做」①）⇒ 夹具是**人上传进房间的一张图**。
 * - 图里那个回执码是**画进像素**的（`RCPT-` ＋ 8 位不可猜）：**不在文件名里、不在正文里、不在
 *   任何文本里** —— 只写在文本里的话，文本就能答对，**证不到视觉**。
 *
 * ## 判据（四条，缺一不可）
 *
 * | # | 量什么 | 怎么量 |
 * |---|---|---|
 * | ① **主** | 她的回信里有那个码 | **完全黑盒**：走 im 公开协议读房里的消息（判据与 `probe-recall` 同款） |
 * | ② | **材料字节没有进她的环境根** | 逐文件按**字节**搜：PNG 签名 ＋ 文件中段 32 字节 ＋ 那一段的 base64 |
 * | ③ | 她那一次 `im_read_attachment` 的**回执**进了经历 | 账本里 `kind=observed` 的块出现 `form = media` |
 * | ④ | 回执给的**感知体量**与文件本身**逐数对上** | 回执的 `pixels = 宽x高` ≡ 那个 PNG 的 IHDR（提案 §7 判据 7 折进来） |
 *
 * ⚠ **②的射程如实**：它挡的是"**照直落盘**"（原始字节 / base64 两种形态）。**任意重编码**挡不住
 * —— 换一种编码就搜不到了 ⇒ 这一格不许读成"字节在任何表示下都不落盘"。
 * ⚠ **②扫的是她的环境根**（`<家>/agents/<支>/`：经历、记忆、笔记都在里面），**不是整个家**：
 * 世界那一份字节在 `<家>/im/media`，那是**世界的存储**（附件的权威），不是她的经历 —— 把它算进
 * 来就是把"世界存了附件"当成"她记了像素"。
 *
 * ## 退出码
 *
 * **0** ＝ 四条都成立 ／ **3** ＝ 没等到她回（**如实报，不假装**）／ **4** ＝ 她回了但没报对那个码
 * ／ **5** ＝ 报对了码，但 ②③④ 里有不成立的 ／ **2** ＝ 装配不起来（含"这台机器上没有任何模型
 * 来源"——那是**不适用**，不是失败）。
 *
 * ## 安全与凭据
 *
 * - **绝不碰真家**：只**读**真家那份清单（照抄一条来源），世界地址从**临时家自己的** `home.conf`
 *   读（读不到就拒绝跑，**不回落默认口**）；跑完删掉整个临时家（账本、凭据、历史一起走）。
 * - **钥匙不打印**：读数行只说 id / 名字 / 型号 / 端点主机（与 `probe-loop` 同一份判据）。
 * - 花掉的是一次**真模型调用**（2026-10-17 的读数是 `prompt_tokens ≈ 217` @64×64，**样本 = 1**）。
 *
 * ## 为什么不用 `temp-world.js` 那套装配
 *
 * 那个模块喂的是**页面**要的世界（它顺手建了一条 backend binding 并签发凭据），而这一支要的是
 * **她真的跑起来** —— 那条路 `probe-loop.js` 已经走通（`up --agent` 自己管凭据）。同一件事两个
 * 装配会漂，所以这里**照 `probe-loop` 的次序**走，只借它的两个读数函数与 `temp-world` 的
 * `api` / `runHome`（纯工具，不带装配判断）。
 */
'use strict'
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const { worldBaseOf } = require('./conf')
const { ledgerBody } = require('./ledger_line')
const modelSources = require('./model-sources')
const paths = require('./paths')
const { api, runHome } = require('./temp-world')
const { chooseModelSource, describeModelSource, describeMessage } = require('./probe-loop')
const { herMessages, makeReceiptMark, markReported } = require('./probe-recall')

const REPO_DIR = path.resolve(__dirname, '..', '..')

/** 图多大（画布尺寸 = PNG 的像素尺寸；判据 ④ 要拿它跟回执逐数对）。 */
const CANVAS = { width: 720, height: 200 }
/** 经历账本的文件名（与 `agent/src/store` 的 `TRACE_FILE` / `PARTS_DIR` 同形 —— 权威在那一侧）。 */
const TRACE_FILE = 'trace.ndjson'
const TRACE_PARTS_DIR = 'trace.parts'
/** 码在像素里怎么画（大、粗、等宽 —— 让"看不看得见"这件事不被字号搅浑）。 */
const GLYPH = 'font:700 60px/1.6 ui-monospace,Consolas,monospace;letter-spacing:6px;color:#000000'

/**
 * 给她的那一句（**只写事实与规矩**）。
 *
 * ⚠ 它**不含那个码**，也不含码的任何一段 —— 码只在像素里。提到 `RCPT-` 前缀是允许的：前缀是
 * 固定形状、猜得到，**不可猜的是那 8 位**。
 */
const TASK_LINE =
  '我在这条消息里给你带了一张图（就在附件里）。用你读附件那个动词把它读出来，' +
  '然后把图里那串字符原样报给我（连前面的 RCPT- 一起）—— 别的不加。'

function parseArguments(argv) {
  const options = { timeoutSeconds: 300, keep: false, modelSourceId: null }
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index]
    if (flag === '--keep') {
      options.keep = true
    } else if (flag === '--timeout') {
      const value = argv[index + 1]
      if (typeof value !== 'string' || !/^\d+$/.test(value.trim()) || Number(value) <= 0) {
        throw new Error('--timeout 后面要给一个正整数（秒）')
      }
      options.timeoutSeconds = Number(value.trim())
      index += 1
    } else if (flag === '--model-source-id') {
      const value = argv[index + 1]
      if (typeof value !== 'string' || value.trim() === '') {
        throw new Error('--model-source-id 后面要给一个清单里的 id（如 ms-1）')
      }
      options.modelSourceId = value.trim()
      index += 1
    } else {
      throw new Error(`不认识的参数：${flag}`)
    }
  }
  return options
}

/**
 * 一张 PNG 的**感知体量**：从 IHDR 读宽高。
 *
 * ⚠ 与 agent 侧 `im_app.rs` 的 `png_pixels` **同一件事**（读文件头、不解码像素）—— 判据 ④ 量的
 * 正是"两处读的是不是同一对数"，所以这里**不许**用"我知道我画了多大"顶替它。
 */
function pngSize(bytes) {
  if (!Buffer.isBuffer(bytes) || bytes.length < 24) return null
  if (bytes.readUInt32BE(0) !== 0x89504e47) return null
  if (bytes.readUInt32BE(4) !== 0x0d0a1a0a) return null
  if (bytes.toString('latin1', 12, 16) !== 'IHDR') return null
  return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) }
}

/**
 * 账本正文里**全部 `kind=observed` 的块**（label ＋ 正文）。
 *
 * 读法：一行一条 JSON、行尾带一个 CRC 尾 —— 按 `ledger_line.js` 切（家侧唯一读法）。
 * ⚠ 读不出来的行**不吞**：如实计数交出去（"没有这一块"与"这一行读不出来"是两件事）。
 */
function observedBlocks(ledgerText) {
  const blocks = []
  const kinds = {}
  let unreadable = 0
  let oddBlocks = 0
  let lines = 0
  for (const line of String(ledgerText ?? '').split('\n')) {
    if (line.trim() === '') continue
    lines += 1
    let value = null
    try {
      value = JSON.parse(ledgerBody(line))
    } catch {
      unreadable += 1
      continue
    }
    const kind = value && typeof value.kind === 'string' ? value.kind : '(没有 kind)'
    kinds[kind] = (kinds[kind] ?? 0) + 1
    if (kind !== 'observed') continue
    for (const block of Array.isArray(value.blocks) ? value.blocks : []) {
      // ⚠ 块的形状以**权威读法**为准：`{label, text}`（`status-server.js` 的 `projectOrigin`
      //   就是这么读的）。认不出的块**如实计数**，不当成"没有这一块" —— 那两者是两件事
      //   （2026-10-17 实测踩过：第一版按 `[label, text]` 两元组读 ⇒ 15 条 observed 全成了 0 块，
      //   读数看着像"回执不在场"，其实是**探针读错了形状**）。
      if (block && typeof block === 'object' && typeof block.text === 'string') {
        blocks.push({ label: typeof block.label === 'string' ? block.label : '', text: block.text })
      } else {
        oddBlocks += 1
      }
    }
  }
  return { blocks, kinds, unreadable, oddBlocks, lines }
}

/** 媒体回执的读数（判据 ③ ＋ ④ 的那一格）：`form = media` 与 `pixels = 宽x高`。 */
function mediaReading(blocks) {
  const hit = (Array.isArray(blocks) ? blocks : []).find((each) =>
    String(each && each.text).includes('form = media'),
  )
  if (!hit) return { present: false, pixels: null, label: null }
  const match = /pixels = ([0-9]+x[0-9]+|unknown)/.exec(hit.text)
  return { present: true, pixels: match ? match[1] : null, label: hit.label }
}

/**
 * 材料字节的**指纹**（判据 ② 用的那几样）。
 *
 * 三样一起搜，因为"落盘"可能有三种样子：整份原样、只落了一段、被 base64 过。
 * ⚠ 射程见文件头：**任意重编码挡不住** —— 这一条量的是"照直落盘"。
 */
function imageFingerprints(pngBytes) {
  const signature = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])
  const middle = pngBytes.subarray(
    Math.floor(pngBytes.length / 2),
    Math.floor(pngBytes.length / 2) + 32,
  )
  return [
    { label: 'PNG 签名', needle: signature },
    { label: '文件中段 32 字节', needle: middle },
    { label: '中段的 base64', needle: Buffer.from(middle.toString('base64'), 'utf8') },
  ]
}

/**
 * 读一个文件，**锁住时重试**（宿主正在写 `model_readings.ndjson` 那一类文件时，Windows 上会短暂
 * 占用 ⇒ 单次读会漏掉一个文件，而"漏读"在判据 ② 里与"干净"长得一模一样）。
 *
 * 返回 `Buffer` 或 `null`（读不出来）。两次之间同步睡一小会儿 —— 这两个读法都是同步的，
 * 而重试是这里唯一需要的异步性。
 */
function readFileRetry(file, attempts = 3) {
  for (let index = 0; index < attempts; index += 1) {
    try {
      return fs.readFileSync(file)
    } catch {
      if (index < attempts - 1) {
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 150)
      }
    }
  }
  return null
}

/**
 * 读不出来的文件怎么算 —— **纯函数**（它决定的是"判据 ② 有没有洞"）。
 *
 * - **0 字节** ⇒ `empty`：里面不可能藏着任何指纹 ⇒ 算**搜过了**（读不出来是因为它被占着，
 *   不是因为它有内容）；
 * - 其余（含**大小也读不出来**）⇒ `unreadable`：**点名交出去**，不许静默当成干净。
 */
function classifyUnreadable(size) {
  return size === 0 ? 'empty' : 'unreadable'
}

/** 在一个目录里**逐文件按字节**搜那些指纹（二进制不解码；读不出来的文件**点名**交出去）。 */
function scanTree(root, needles) {
  const hits = []
  const unreadableFiles = []
  let files = 0
  let emptyFiles = 0
  const walk = (dir) => {
    let items
    try {
      items = fs.readdirSync(dir, { withFileTypes: true })
    } catch {
      return
    }
    for (const item of items) {
      const full = path.join(dir, item.name)
      if (item.isDirectory()) {
        walk(full)
        continue
      }
      if (!item.isFile()) continue
      const bytes = readFileRetry(full)
      if (bytes === null) {
        // 读不出来时再看一眼大小（实测：被宿主占着的 `.writer.lock` 就是这一档）。
        let size = null
        try {
          size = fs.statSync(full).size
        } catch {
          size = null
        }
        if (classifyUnreadable(size) === 'empty') {
          files += 1
          emptyFiles += 1
        } else {
          unreadableFiles.push(path.relative(root, full))
        }
        continue
      }
      files += 1
      for (const { label, needle } of needles) {
        if (needle.length > 0 && bytes.includes(needle)) {
          hits.push({ file: path.relative(root, full), label })
        }
      }
    }
  }
  walk(root)
  return { hits, files, emptyFiles, unreadableFiles }
}

/**
 * 读**经历账本**：`<环境根>/trace.ndjson` ＋ `<环境根>/trace.parts/*.ndjson`。
 *
 * ⚠ 只读经历，**不读 `facts.ndjson`**：事实与经历是两个存储，混在一起数行数，读的人就分不清
 * "这一行没有 kind"是"它不是经历"还是"它坏了"。（文件名与 `agent/src/store` 的
 * `TRACE_FILE` / `PARTS_DIR` 同形 —— 权威在那一侧。）
 */
function readLedgers(root) {
  const parts = []
  const unreadableFiles = []
  const read = (file, label) => {
    const bytes = readFileRetry(file)
    if (bytes === null) unreadableFiles.push(label)
    else parts.push(bytes.toString('utf8'))
  }
  read(path.join(root, TRACE_FILE), TRACE_FILE)
  let items = []
  try {
    items = fs.readdirSync(path.join(root, TRACE_PARTS_DIR), { withFileTypes: true })
  } catch {
    items = []
  }
  for (const item of items) {
    if (item.isFile() && item.name.endsWith('.ndjson')) {
      read(path.join(root, TRACE_PARTS_DIR, item.name), `${TRACE_PARTS_DIR}/${item.name}`)
    }
  }
  return { text: parts.join('\n'), unreadableFiles }
}

/**
 * **主判据**：她报出了那个码吗。
 *
 * 判据是**不可猜的那一段**（`RCPT-` 后面那 8 位），不是整串：前缀是固定形状、猜得到，而她可能
 * 只把那 8 位报回来 —— 那也算报对了。8 位取自 32 个字符 ⇒ 撞上的概率约 1/32⁸，可忽略。
 */
function reportedMark(messages, agentId, mark) {
  if (markReported(messages, agentId, mark)) return true
  const tail = String(mark).replace(/^RCPT-/, '')
  if (tail.length < 8) return false
  return herMessages(messages, agentId).some((each) =>
    String((each && each.body) || '').includes(tail),
  )
}

/**
 * 四条判据的裁决（**纯函数**，可判错）。
 *
 * 输入全是已经量好的读数 —— 这一层不做 IO，也不替读数说情。
 */
function visionVerdict({ replied, reported, media, expectedPixels, scan }) {
  const bytesAbsent = scan.hits.length === 0
  const receipt = media.present === true
  const pixelsMatch = receipt && media.pixels === expectedPixels
  return {
    replied,
    reported,
    bytesAbsent,
    receipt,
    pixelsMatch,
    ok: reported && bytesAbsent && receipt && pixelsMatch,
  }
}

/** 把回执码**画进像素**，返回一张 PNG 的字节。⚠ 码只在像素里。 */
async function renderMarkPng(mark) {
  const { createRequire } = require('node:module')
  // ⚠ playwright 装在 **client** 里（本脚本在 scripts/home/ 下，从仓根 require 不到）。
  const clientRequire = createRequire(path.join(REPO_DIR, 'client', 'package.json'))
  const { chromium } = clientRequire('playwright')
  const browser = await chromium.launch({
    headless: true,
    args: ['--no-proxy-server'],
    executablePath: process.env.CELIRA_BROWSER_EXECUTABLE?.trim() || undefined,
  })
  try {
    const page = await browser.newPage({
      viewport: { width: CANVAS.width, height: CANVAS.height },
      deviceScaleFactor: 1,
    })
    await page.setContent(
      '<body style="margin:0;background:#ffffff">' +
        `<div style="${GLYPH};text-align:center;padding-top:40px">${mark}</div>` +
        '</body>',
      { waitUntil: 'load' },
    )
    const shot = await page.screenshot({
      type: 'png',
      clip: { x: 0, y: 0, width: CANVAS.width, height: CANVAS.height },
    })
    return Buffer.from(shot)
  } finally {
    await browser.close()
  }
}

async function main() {
  let options
  try {
    options = parseArguments(process.argv.slice(2))
  } catch (error) {
    console.error(`[probe-vision] ${error.message}`)
    console.error('用法: node scripts/home/probe-vision.js [--timeout <秒>] [--keep] [--model-source-id <id>]')
    return 2
  }

  // ---- 前置：她用什么想（**照抄真家那一条**，只读；钥匙永不打印）------------------------
  const realList = modelSources.listPath(paths.homeDir())
  if (!fs.existsSync(realList)) {
    console.error(`[probe-vision] 这台机器上没有任何模型来源（${realList} 不在）⇒ 她没法思考 ⇒`)
    console.error('[probe-vision] 这一轮**不适用**（不是失败）。去「模型配置」页加一条再来。')
    return 2
  }
  let parsedList
  try {
    parsedList = JSON.parse(fs.readFileSync(realList, 'utf8'))
  } catch (error) {
    console.error(`[probe-vision] 真家那份清单读不出来（${realList}）：${error.message}`)
    return 2
  }
  const picked = chooseModelSource(modelSources.normalizeEntries(parsedList), options.modelSourceId)
  if (!picked.ok) {
    console.error(`[probe-vision] ${picked.reason}`)
    return 2
  }
  // ★ **被测的就是这一格**：在自己的临时家里显式声明 `vision: true`（真家那一条有没有声明都不
  //   影响这一轮 —— 这一轮量的是"声明了之后，她到底看不看得见"）。
  const source = { ...picked.entry, vision: true }
  console.log(`[probe-vision] 她用什么想：照抄真家 ${realList} 的那一条`)
  console.log(`               ${describeModelSource(source)}（**钥匙不打印**）`)
  console.log(
    `[probe-vision] 临时家里把这一条的 \`vision\` 声明成 true（真家那一条现在是 ${
      picked.entry.vision === true ? 'true' : '没声明/不是 true'
    }）`,
  )

  // ⚠ **模型那一家要绕过代理**（2026-10-14 实测的坑，见 `AGENTS.md` 的真机探针前置）：宿主经代理时
  //   大回信会被截断（`transport-failure:io` / `timeout`）⇒ **假红**。主机名从**这一条自己的端点**
  //   派生 —— 不让这一轮的成败取决于"跑它的人环境里恰好有没有那一行"。
  try {
    const host = new URL(source.endpoint).hostname.toLowerCase()
    const current = String(process.env.NO_PROXY || process.env.no_proxy || '')
    const bare = current
      .split(',')
      .map((each) => each.trim().toLowerCase().replace(/^\*\./, '').replace(/^\./, ''))
      .filter(Boolean)
    if (host && !bare.includes(host)) {
      const merged = [current, host, `.${host}`].filter(Boolean).join(',')
      process.env.NO_PROXY = merged
      process.env.no_proxy = merged
      console.log(`[probe-vision] 代理绕过：把 ${host} 补进 NO_PROXY（宿主经代理时大回信会被截断 ⇒ 假红）`)
    }
  } catch {
    /* 端点不是合法 URL：门那一侧会拒它，这里不猜 */
  }

  // ---- 图（码画进像素）----------------------------------------------------------------
  const mark = makeReceiptMark()
  let png
  try {
    png = await renderMarkPng(mark)
  } catch (error) {
    console.error(`[probe-vision] 画不出那张图（playwright / 浏览器起不来）：${error.message}`)
    console.error('[probe-vision] 装配不起来 ⇒ 不适用（不是失败）。')
    return 2
  }
  const size = pngSize(png)
  if (!size) {
    console.error('[probe-vision] 画出来的东西不是一张能读的 PNG —— 装配不起来')
    return 2
  }
  const expectedPixels = `${size.width}x${size.height}`
  const fingerprints = imageFingerprints(png)
  console.log(
    `[probe-vision] 图：${expectedPixels} · ${png.length} 字节 · 码画在**像素**里（不进文件名、不进正文、不进日志）`,
  )

  const homeDir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-vision-probe-'))
  console.log(`[probe-vision] 临时家 = ${homeDir}（**全程不碰真家**）`)

  try {
    // 1) 起世界（那个家里）＋ 2) 拿 token
    const up = runHome(['up'], homeDir)
    if (up.status !== 0) {
      console.error('[probe-vision] 那个家起不来（node home.js up 非 0）：')
      console.error(String(up.stdout || '').split('\n').slice(-8).join('\n'))
      return 2
    }
    const base = worldBaseOf(homeDir)
    if (!base) {
      console.error(
        '[probe-vision] 读不到这个家自己的世界地址（`home.conf` 的 `listen_addr`）⇒ **不跑**：' +
          '默认的 8080 上可能是别人的家，拿它做实验不算数',
      )
      return 2
    }
    const minted = runHome(['up', '--token'], homeDir)
    const tokenMatch = /\[token\]\s*(\S+)/.exec(String(minted.stdout || ''))
    const token = tokenMatch ? tokenMatch[1] : null
    if (!token) {
      console.error('[probe-vision] 取不到 Human token（`up --token` 没回那一行）—— 装配不起来')
      return 2
    }
    console.log(`[probe-vision] 世界 = ${base}（从这个家自己的 home.conf 读）· token 到手（不打印取值）`)

    // 3) 世界里建一支 Agent ＋ 一间 direct 房
    const created = await api(base, token, 'POST', '/v1/agents', { display_name: '探针小测' })
    const agentId = created.json && created.json.agent ? created.json.agent.id : null
    if (created.status !== 201 || !agentId) {
      console.error(`[probe-vision] 建 Agent 没成：HTTP ${created.status} ${created.text.slice(0, 160)}`)
      return 2
    }
    const room = await api(base, token, 'POST', '/v1/direct-rooms', { agent_id: agentId })
    const roomId = room.json && room.json.room ? room.json.room.id : null
    if (room.status !== 201 || !roomId) {
      console.error(`[probe-vision] 建房没成：HTTP ${room.status} ${room.text.slice(0, 160)}`)
      return 2
    }
    console.log(`[probe-vision] Agent = ${agentId.slice(0, 8)}… ｜ 房 = ${roomId.slice(0, 8)}…`)

    // 3b) 那一份模型来源交给这个临时家（清单一条 ＋ 环境根里先放好绑定）
    modelSources.writeList(homeDir, [source])
    const envRoot = paths.agentEnvRoot(homeDir, agentId)
    fs.mkdirSync(envRoot, { recursive: true })
    modelSources.writeBinding(envRoot, source.id)

    // 4) 把她安在**这个临时家**上（环境 ＋ 凭据 ＋ 宿主）
    const adopted = runHome(['up', '--agent', agentId], homeDir)
    if (adopted.status !== 0) {
      console.error('[probe-vision] 安家没成（up --agent 非 0）：')
      console.error(String(adopted.stdout || '').split('\n').slice(-8).join('\n'))
      return 2
    }
    console.log('[probe-vision] 她安在这个临时家上了（环境 ＋ 凭据 ＋ 宿主）')

    // 5) 人把那张图**传进房间**，再发一条消息把它绑上去（公共路径，与页面同一条协议）
    const form = new FormData()
    form.append('file', new Blob([png], { type: 'image/png' }), 'vision-probe.png')
    const uploaded = await fetch(`${base}/v1/rooms/${roomId}/attachments`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${token}` },
      body: form,
    })
    const uploadedText = await uploaded.text()
    let uploadedJson = null
    try {
      uploadedJson = JSON.parse(uploadedText)
    } catch {
      uploadedJson = null
    }
    const attachment = uploadedJson && uploadedJson.attachment ? uploadedJson.attachment : null
    if (uploaded.status !== 201 || !attachment) {
      console.error(`[probe-vision] 上传没成：HTTP ${uploaded.status} ${uploadedText.slice(0, 160)}`)
      return 2
    }
    console.log(
      `[probe-vision] 上传：attachment = ${String(attachment.id).slice(0, 8)}… · ` +
        `content_type = ${attachment.content_type} · size = ${attachment.size_bytes}` +
        `（本地 ${png.length} 字节${attachment.size_bytes === png.length ? '，一致' : '，**不一致**'}）`,
    )
    const sent = await api(base, token, 'POST', `/v1/rooms/${roomId}/messages`, {
      body: TASK_LINE,
      reply_to_message_id: null,
      attachments: [attachment.id],
      mentions: [],
    })
    if (sent.status !== 201) {
      console.error(`[probe-vision] 发消息没成：HTTP ${sent.status} ${sent.text.slice(0, 160)}`)
      return 2
    }
    console.log(`[probe-vision] 主人发了：${TASK_LINE}`)

    // 6) 等她回（**把读到的每一条原样打出来** —— 不替读数下结论）
    const deadline = Date.now() + options.timeoutSeconds * 1000
    let messages = []
    let reported = false
    let lastCount = -1
    while (Date.now() < deadline) {
      const listed = await api(base, token, 'GET', `/v1/rooms/${roomId}/messages?limit=50`)
      messages = (listed.json && listed.json.messages) || []
      if (messages.length !== lastCount) {
        lastCount = messages.length
        console.log(`[probe-vision] （房里 ${messages.length} 条）`)
        for (const each of messages) console.log(`               ${describeMessage(each)}`)
      }
      if (herMessages(messages, agentId).length > 0) {
        reported = reportedMark(messages, agentId, mark)
        if (reported) break
        // 她说了话但还没报对：再等等（她可能正在第二次读/第二次答）
      }
      await new Promise((resolve) => setTimeout(resolve, 5000))
    }

    const hers = herMessages(messages, agentId)
    const ledger = readLedgers(envRoot)
    const { blocks, kinds, unreadable, oddBlocks, lines } = observedBlocks(ledger.text)
    const media = mediaReading(blocks)
    const scan = scanTree(envRoot, fingerprints)
    const verdict = visionVerdict({
      replied: hers.length > 0,
      reported,
      media,
      expectedPixels,
      scan,
    })
    /** 账本里到底有什么（**给排查用的读数**，不是判据）：块数、各类 kind、observed 的 label。 */
    const kindLine = Object.entries(kinds)
      .map(([kind, count]) => `${kind} ${count}`)
      .join(' · ')
    const labelLine = [...new Set(blocks.map((each) => each.label))].join('、')

    console.log('')
    console.log(
      `[probe-vision] ① 主判据（黑盒）：她的回信里有那个码（RCPT- ＋ 8 位不可猜）⇒ ${
        verdict.reported ? '成立' : '**不成立**'
      }（她说了 ${hers.length} 条）`,
    )
    console.log(
      `[probe-vision] ② 材料字节：在她的环境根里逐文件搜了 ${scan.files} 个文件` +
        (scan.emptyFiles > 0 ? `（其中 ${scan.emptyFiles} 个是 0 字节 —— 里面不可能有指纹）` : '') +
        (scan.unreadableFiles.length > 0
          ? `（**读不出来 ${scan.unreadableFiles.length} 个：${scan.unreadableFiles.join('、')}** —— 那几个没搜到）`
          : '（全都搜到了）') +
        `⇒ 命中 ${scan.hits.length}` +
        (scan.hits.length > 0
          ? `：${scan.hits.map((each) => `${each.file}（${each.label}）`).join(' · ')}`
          : '（世界那一份字节在 <家>/im/media —— 那是世界的存储，不在这一条的射程里）'),
    )
    console.log(
      `[probe-vision] ③ 回执：账本 ${lines} 行（读不出来 ${unreadable}${
        ledger.unreadableFiles.length > 0
          ? `；**没读到的账本文件：${ledger.unreadableFiles.join('、')}**`
          : ''
      }）里 \`form = media\` 那一块 ⇒ ${verdict.receipt ? `在场（label = ${media.label}）` : '**不在场**'}`,
    )
    console.log(
      `[probe-vision]    （账本里有什么：observed 块 ${blocks.length} 块` +
        `${oddBlocks > 0 ? `（**形状认不出 ${oddBlocks} 块**）` : ''}` +
        `${labelLine ? `（label：${labelLine}）` : ''} · 各类 kind：${kindLine || '(一行都没读出来)'}）`,
    )
    console.log(
      `[probe-vision] ④ 感知体量：回执写 \`pixels = ${media.pixels ?? '(没有这一行)'}\`，` +
        `文件头是 ${expectedPixels} ⇒ ${verdict.pixelsMatch ? '逐数对上' : '**对不上**'}`,
    )

    if (verdict.ok) {
      console.log('')
      console.log('[probe-vision] 结论：**她看得见** —— 活着的宿主把图片折进了请求，四条判据都成立')
      return 0
    }
    if (!verdict.replied) {
      console.log('')
      console.log(`[probe-vision] 结论：**没等到她回**（等了 ${options.timeoutSeconds} 秒）—— 如实报，不假装`)
      console.log('[probe-vision] [next] 房里读到的每一条都在上面；她起没起来看 `<家>/logs/agents/<支>/host.out`')
      return 3
    }
    if (!verdict.reported) {
      console.log('')
      console.log('[probe-vision] 结论：**她回了，但没报对那个码** ⇒ 视觉这一条没成立（如实报）')
      console.log('[probe-vision] [next] 先看 ③ 那条回执在不在：不在 ⇒ 她根本没读附件；在 ⇒ 字节交出去了，但她没认出来')
      return 4
    }
    console.log('')
    console.log('[probe-vision] 结论：**她报对了码**，但 ②③④ 里有不成立的 ⇒ 见上面那三行的读数')
    console.log('[probe-vision] [next] ②命中 ⇒ 材料落进了她的环境根（那是形状问题，去查 store 那条路）')
    return 5
  } finally {
    runHome(['down'], homeDir)
    if (options.keep) {
      console.log(`[probe-vision] --keep：临时家留着（${homeDir}）`)
      console.log('[probe-vision] ⚠ 那个临时家里有**你那份清单那一条的副本**（含钥匙）——排查完请自己删掉它')
    } else {
      try {
        fs.rmSync(homeDir, { recursive: true, force: true })
        console.log('[probe-vision] 临时家已删（那个世界的账本、凭据与历史一起走）')
      } catch (error) {
        console.error(`[probe-vision] ⚠ 临时家没删掉（${error.message}）：${homeDir}`)
      }
    }
  }
}

// 只有这几个是给测试的（纯函数）；真机那一段全是副作用，测试里不碰。
module.exports = {
  parseArguments,
  pngSize,
  observedBlocks,
  mediaReading,
  imageFingerprints,
  scanTree,
  classifyUnreadable,
  readLedgers,
  reportedMark,
  visionVerdict,
  CANVAS,
  TASK_LINE,
}

if (require.main === module) {
  main()
    .then((code) => {
      process.exitCode = typeof code === 'number' ? code : 2
    })
    .catch((error) => {
      console.error(`[probe-vision] 意外失败：${error && error.message ? error.message : error}`)
      process.exitCode = 2
    })
}
