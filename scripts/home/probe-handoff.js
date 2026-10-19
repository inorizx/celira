#!/usr/bin/env node
'use strict'
/**
 * **「她写下的交接，下一段收得到吗」真机探针**。
 *
 * ## 它验的是哪一句
 *
 * 交接（`handoff`）那条线落成之后，判据是**两个半句**，而它们**从来没在真机上跑过**：
 *
 * > **她写下的字，下一段开场由她自己收到**（`agent/README.md` §3.22）。
 *
 * 两半各自都可能不成立，而**失败的样子完全不同**：
 *
 * - **写那一半**：她**找不找得到那个动词**（`write_handoff` 只在动作目录里，任务里**不提它的名字**）；
 * - **交付那一半**：那一份**进不进得了下一段的开场**（判据："上一次开场摆过它**且那一段真的想过**"）。
 *
 * ## 它怎么跑（**两次活动，一个临时家**）
 *
 * 1. 在系统临时目录里造一个**全新的家**（**全程不碰真家**）；
 * 2. 起世界、建一支 Agent、安在这个临时家上（照 `probe-recall.js` 那条路）；
 * 3. **第一段**：主人发一条 ——「我要收工了。你写一份交接留给你下一次开场的自己：
 *    里面写上这一串 —— `<码>`。写完就收场。」
 *    ⚠ **任务里不提动词名、不提路径**：她手上只有动作目录里那一行（`write_handoff` 的说明）；
 * 4. **等她收场**（经历里出现那一段的结束记录）——**她没开新的一段，这一轮就量不了**（见退出码 2）；
 * 5. **第二段**：主人再发一条 ——「你这一次开场时，自己上一次留给你的那份交接里写的是什么？
 *    把那串字符原样报给我（没有就说没有）。」
 * 6. 等她回；**把房里读到的每一条原样打出来**（不替读数下结论）；
 * 7. 收尾：`down` ＋ 删掉整个临时家（`finally` 里也删）。
 *
 * ## 判据（主判据完全黑盒；辅助判据只用来**排除另一条路**）
 *
 * | # | 判据 | 怎么读 | 用在哪 |
 * |---|---|---|---|
 * | **主** | **第二段**里她报没报出那个不可猜的码 | 走 **im 公开协议**读房里的消息 | **成功/失败的唯一判据** |
 * | 辅① | 第一段的经历里有没有 `write_handoff` | 读 `<环境根>/trace.ndjson` | **只用来分类失败**：分"她没写"与"写了没送到" |
 * | 辅② | 第二段她自己有没有去读那一份（`terminal` / `files_*`） | 同上 | **排除另一条路**：她若自己 `cat` 了那一份，"报对了"就不再是交付的证据 |
 *
 * ⚠ **为什么"报对了"必须配辅②**：那一份就住在她的环境根里（`<环境根>/handoff`），
 * 她**有** `terminal` ⇒ 她可以自己去读。**只有"她没绕路"时，"报对了"才只可能来自开场那一块。**
 * 这一格**必须**读经历——没有别的办法知道她走的是哪条路；而它**从不单独用来证明成功**
 * （读不到经历 ⇒ 报"不知道"，不报"交付了"）。
 *
 * ⚠ **为什么主判据只数"第二段之后"她说的**：第一段那条任务里**就写着那个码** ⇒ 她照抄一遍
 * 完全可能。判据从**发第二条任务那一刻**起数她的消息（这一条是 2026-10-19 设计时定下的，
 * 不这么做，"她报对了"会有第二条路）。
 *
 * ## 用法与退出码
 *
 * ```text
 * node scripts/home/probe-handoff.js [--timeout <秒>] [--keep]
 * ```
 *
 * - `--timeout <秒>`：每一段等她回的上限（缺省 300）；
 * - `--keep`：跑完**留着**那个临时家（排查用；正常跑一律删）；
 * - 退出码：
 *   - **0** 她**写了 ＋ 下一段报对了 ＋ 没绕路** ⇒ 这条线在真机上成立；
 *   - **3** 她**没写**（第一段的经历里没有 `write_handoff`）⇒ **写那一半不成立**；
 *   - **4** 她写了、但第二段**没报对**（交付没到 / 她没读出来）⇒ **不并进 3**；
 *   - **5** 她**报对了，但自己绕路读过那一份**（`terminal` / `files_*`）⇒ 交付那一半**不能算**；
 *   - **6** 她报对了、而第二段开场**没有摆过那一块**（两条判据打架）⇒ **如实报，人工看**；
 *   - **2** 装配不起来／**这台机器上没有模型钥匙**／钥匙在场但被服务端拒／**她没开新的一段**
 *     （前提不成立）——后四者是**不适用**，不是失败。
 *
 * ⚠ **它是真机探针**：会起真世界、真宿主，**并花掉真模型调用**（用这台机器上那把钥匙）。
 * ⚠ **样本 = 1 什么都证明不了**：单次结果只能说"这一次她写了/送到了"；要下结论必须重复 N 次。
 * ⚠ **它量不到**：① "她会不会**自己**想起来写交接"（这里是人叫她写的）；② 第三次活动
 * （"只摆一次"那一半）——那一条在离线套件里钉着（`agent/tests/handoff.rs`）。
 */

const crypto = require('node:crypto')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { spawnSync } = require('node:child_process')

const { readUserEnv } = require('./userenv')
const { worldBaseOf } = require('./conf')

const REPO_DIR = path.resolve(__dirname, '..', '..')
const HOME_JS = path.join(REPO_DIR, 'home.js')
const MODEL_KEY_NAME = 'DEEPSEEK_API_KEY'

/** 那一块在显示区的标签（与 `agent/src/handoff.rs` 的 `HANDOFF_LABEL` **同一个词**）。 */
const HANDOFF_LABEL = 'handoff'
/** 她的写动词（**任务里不提它** —— 找不找得到它正是"写那一半"要量的事）。 */
const WRITE_ACTION = 'write_handoff'
/** 第一段那条任务的正文（`%s` 处填那个不可猜的码）。 */
const WRITE_TASK = '我要收工了。你写一份交接留给你下一次开场的自己：里面写上这一串 —— %s。写完就收场。'
/** 第二段那条任务的正文。 */
const REPORT_TASK =
  '你这一次开场时，自己上一次留给你的那份交接里写的是什么？把那串字符原样报给我（没有就说没有）。'
/** 她自己绕路去读那一份时会用到的动作（读到它们 ⇒ "报对了"不再算交付的证据）。 */
const DETOUR_ACTIONS = ['terminal', 'files_read', 'files_list', 'files_grep', 'files_write']

/** 参数解析（未知选项**明确失败**，不静默忽略）。 */
function parseArguments(argv) {
  const options = { timeoutSeconds: 300, keep: false }
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
    } else {
      throw new Error(`不认识的参数：${flag}`)
    }
  }
  return options
}

/**
 * 生成一个**不可猜**的码。
 *
 * 它必须不可猜，因为**主判据就是"她报出来的码对不对"**：码猜得到，判据就失效了。
 * 形状用大写字母数字（避免 `0/O`、`1/I` 这类看不清的对），前缀 `HO-` 便于在正文里认出来。
 */
function makeMark(random = crypto.randomBytes) {
  const alphabet = 'ABCDEFGHJKLMNPQRSTUVWXYZ23456789'
  const bytes = random(8)
  let out = ''
  for (let index = 0; index < 8; index += 1) {
    out += alphabet[bytes[index] % alphabet.length]
  }
  return `HO-${out}`
}

/** 第一段那条任务（**码必须逐字进正文**：她照抄它，才谈得上交付）。 */
function writeTask(mark) {
  return WRITE_TASK.replace('%s', mark)
}

/** 这条消息是她说的吗（判据只看**身份**，不看正文——与 `probe-loop.js` 同一份判据）。 */
function isHerMessage(message, agentId) {
  if (!message || typeof message !== 'object') return false
  const actor = message.actor
  if (!actor || typeof actor !== 'object') return false
  if (typeof agentId !== 'string' || agentId === '') return false
  return actor.id === agentId
}

/** 她说的那些条（按时间序）。 */
function herMessages(messages, agentId) {
  return (Array.isArray(messages) ? messages : []).filter((each) => isHerMessage(each, agentId))
}

/**
 * **主判据**：她在**第二段之后**说的那些话里，有没有那个码。
 *
 * ⚠ `since`（发第二条任务时房里已有几条）**不是可选项**：第一条任务里就写着那个码 ⇒ 不从那一刻
 * 起数，"她报对了"会有一条假的路（照抄任务）。
 */
function markReported(messages, agentId, mark, since) {
  const tail = (Array.isArray(messages) ? messages : []).slice(Math.max(0, Number(since) || 0))
  return herMessages(tail, agentId).some((each) => String((each && each.body) || '').includes(mark))
}

/** 经历账本的一行 → JSON（行尾那一截是 `\t<8位十六进制校验和>`，**不是正文**）。 */
function traceRow(raw) {
  const line = String(raw || '').trim()
  if (line === '') return null
  const tab = line.lastIndexOf('\t')
  const body = tab >= 0 ? line.slice(0, tab) : line
  try {
    const value = JSON.parse(body)
    return value && typeof value === 'object' ? value : null
  } catch {
    return null
  }
}

/** 经历账本里的每一行（读不懂的行**跳过**，不编造）。 */
function traceRows(traceText) {
  const rows = []
  for (const raw of String(traceText || '').split('\n')) {
    const row = traceRow(raw)
    if (row) rows.push(row)
  }
  return rows
}

/** 她在哪些**段**里调过哪些动作：`[{run, action}, …]`（按盘上的顺序）。 */
function actionsIn(traceText) {
  const out = []
  for (const row of traceRows(traceText)) {
    if (row.kind !== 'action-selected') continue
    if (typeof row.action !== 'string') continue
    out.push({ run: String(row.run || ''), action: row.action })
  }
  return out
}

/** **开场那一条注入**（`stage = run-start`）摆过哪些块：`[{run, blocks:[{label,text}]}, …]`。 */
function openingInjections(traceText) {
  const out = []
  for (const row of traceRows(traceText)) {
    if (row.kind !== 'injection') continue
    if (row.stage !== 'run-start') continue
    const blocks = Array.isArray(row.blocks) ? row.blocks : []
    out.push({
      run: String(row.run || ''),
      blocks: blocks
        .filter((each) => each && typeof each === 'object')
        .map((each) => ({ label: String(each.label || ''), text: String(each.text || '') })),
    })
  }
  return out
}

/**
 * **活动段的顺序**（经历里出现过 `run-start` 的那几段，按盘上的先后）。
 *
 * ⚠ **不按 id 排序**：`run-12` 按 id 排在 `run-4` 前面，那不是先后（与运行时同一条判据）。
 */
function runsIn(traceText) {
  const runs = []
  for (const row of traceRows(traceText)) {
    if (row.kind !== 'run-start') continue
    const run = String(row.run || '')
    if (run !== '' && !runs.includes(run)) runs.push(run)
  }
  return runs
}

/** 她在那一段里写过交接没有（`write_handoff`）。 */
function wroteHandoff(actions, run) {
  return (Array.isArray(actions) ? actions : []).some(
    (each) => each && each.run === run && each.action === WRITE_ACTION,
  )
}

/** 她在那一段里自己绕路去读那一份没有（`terminal` / `files_*`）。 */
function readByDetour(actions, run) {
  return (Array.isArray(actions) ? actions : []).some(
    (each) => each && each.run === run && DETOUR_ACTIONS.includes(each.action),
  )
}

/** 那一段的开场摆过这一份没有（标签对得上**且正文里带着那个码**）。 */
function openingShowed(injections, run, mark) {
  const found = (Array.isArray(injections) ? injections : []).find((each) => each && each.run === run)
  if (!found) return false
  return found.blocks.some((block) => block.label === HANDOFF_LABEL && block.text.includes(mark))
}

/**
 * **判决**（纯函数 ⇒ 六种结果可判错）。
 *
 * 六种结果各自说话，**不许互相顶替**：`3`（没写）与 `4`（写了没送到）是两件事，
 * `5`（她自己绕路读了）更不是"交付成立"——**那是另一条路**。
 *
 * ⚠ `runs.length < 2` ⇒ **前提不成立**（她没开新的一段 ⇒ 没有"下一段的开场"可量）——
 * 这一档**不适用**，不是失败：她搁下不搁下是她的事（§3.59/§3.60）。
 */
function handoffVerdict({ runs, actions, injections, mark, reported, traceReadable }) {
  if (!Array.isArray(runs) || runs.length < 2) {
    return { kind: 'no-second-activity', runs: Array.isArray(runs) ? runs.length : 0 }
  }
  const first = runs[0]
  const second = runs[1]
  const wrote = wroteHandoff(actions, first)
  const detour = readByDetour(actions, second)
  const shown = traceReadable ? openingShowed(injections, second, mark) : null
  if (!wrote) return { kind: 'never-wrote', first, second, wrote, detour, shown }
  if (reported && detour) return { kind: 'by-detour', first, second, wrote, detour, shown }
  if (reported && shown === false) return { kind: 'contradiction', first, second, wrote, detour, shown }
  if (reported) return { kind: 'delivered', first, second, wrote, detour, shown }
  return { kind: 'not-reported', first, second, wrote, detour, shown }
}

/** 一条消息的一行人话（读数用：谁、什么时候、说了什么）。 */
function describeMessage(message) {
  const actor = message && message.actor ? message.actor : {}
  const who = `${actor.kind || '?'}:${String(actor.id || '?').slice(0, 8)}`
  const when = message && message.created_at ? message.created_at : '?'
  const body = String((message && message.body) || '').replace(/\s+/g, ' ').slice(0, 160)
  return `${when}  [${who}]  ${body}`
}

// ---------------------------------------------------------------------------
// 真机部分（起进程、发请求、收尾）—— 副作用都在这里
// ---------------------------------------------------------------------------

function runHome(args, homeDir, extraEnv = {}) {
  return spawnSync(process.execPath, [HOME_JS, ...args], {
    cwd: REPO_DIR,
    encoding: 'utf8',
    env: { ...process.env, ...extraEnv, CELIRA_HOME_DIR: homeDir },
  })
}

/** 从 `up --token` 的输出里取那支 token（**只回内存，绝不打印**）。 */
function tokenFromUpOutput(text) {
  const match = /\[token\]\s*(\S+)/.exec(String(text || ''))
  return match ? match[1] : null
}

async function api(base, token, method, url, body) {
  const headers = { Authorization: `Bearer ${token}` }
  if (body !== undefined) {
    headers['content-type'] = 'application/json'
    headers['Idempotency-Key'] = `probe-handoff-${Date.now()}-${Math.random().toString(16).slice(2)}`
  }
  const response = await fetch(`${base}${url}`, {
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

/// 读临时家里宿主写下的**模型失败分类**（没有 ⇒ `null`）。
///
/// ⚠ 为什么要它：前置那条"有没有钥匙"只看**用户环境**——钥匙**在场但被服务端拒**时它照样放行，
/// 于是宿主 401、她一个字想不出来，而结论会写成"**她没写**"。**分不开这两件事的读数会骗人**
/// （与 `probe-recall.js` 同一处坑，2026-10-14 实测过）。
function readModelFailure(homeDir) {
  const agentsDir = path.join(homeDir, 'logs', 'agents')
  let names = []
  try {
    names = fs.readdirSync(agentsDir)
  } catch {
    return null
  }
  for (const name of names) {
    let text = ''
    try {
      text = fs.readFileSync(path.join(agentsDir, name, 'host.out'), 'utf8')
    } catch {
      continue
    }
    const hit = text.match(/model-failure=(\S+)/)
    if (hit) return hit[1]
  }
  return null
}

/** 读她的经历账本（读不到 ⇒ `null`，**如实说不知道**，不编造）。 */
function readTrace(envDir) {
  try {
    return fs.readFileSync(path.join(envDir, 'trace.ndjson'), 'utf8')
  } catch {
    return null
  }
}

async function main() {
  let options
  try {
    options = parseArguments(process.argv.slice(2))
  } catch (error) {
    console.error(`[probe-handoff] ${error.message}`)
    console.error('用法: node scripts/home/probe-handoff.js [--timeout <秒>] [--keep]')
    return 2
  }

  // 前置：这台机器上有没有那把钥匙 —— **没有就是"不适用"**（她没法思考，不是链坏了）。
  const modelKey = readUserEnv(MODEL_KEY_NAME)
  if (!modelKey) {
    console.error(
      `[probe-handoff] 这台机器的用户环境里没有 ${MODEL_KEY_NAME} ⇒ 她没法思考 ⇒ 这一轮**不适用**（不是失败）。`,
    )
    return 2
  }

  const mark = makeMark()
  // ⚠ **模型那一家要绕过代理**（`AGENTS.md` 的真机探针前置；2026-10-14 实测的坑）：宿主经代理时
  // 大回信会被截断（`transport-failure:io` / `timeout`）⇒ **假红**。补的是那把钥匙对应的那一家。
  const modelHost = 'api.deepseek.com'
  const current = String(process.env.NO_PROXY || process.env.no_proxy || '')
  const bare = current
    .split(',')
    .map((each) => each.trim().toLowerCase())
    .filter(Boolean)
  if (!bare.includes(modelHost)) {
    const merged = [current, modelHost, `.${modelHost}`].filter(Boolean).join(',')
    process.env.NO_PROXY = merged
    process.env.no_proxy = merged
    console.log(
      `[probe-handoff] 代理绕过：把 ${modelHost} 补进 NO_PROXY（宿主经代理时大回信会被截断 ⇒ 假红）`,
    )
  }
  const homeDir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-handoff-probe-'))
  let base = null
  console.log(`[probe-handoff] 临时家 = ${homeDir}（**全程不碰真家**）`)
  console.log(`[probe-handoff] 这一次的码 = ${mark}（随机生成，**猜不到**）`)

  try {
    // 1) 起世界（那个家里）＋ 2) 拿 token
    const up = runHome(['up'], homeDir)
    if (up.status !== 0) {
      console.error('[probe-handoff] 那个家起不来（node home.js up 非 0）：')
      console.error(String(up.stdout || '').split('\n').slice(-8).join('\n'))
      return 2
    }
    // **世界地址现在才知道**：那个家刚把挑到的口写进它自己的 `home.conf`。
    // ⚠ 读不到就**拒绝跑**——绝不回落到默认的 8080（那上面可能是主人正开着的家）。
    base = worldBaseOf(homeDir)
    if (!base) {
      console.error(
        '[probe-handoff] 读不到这个家自己的世界地址（`home.conf` 的 `listen_addr`）⇒ **不跑**：' +
          '默认的 8080 上可能是别人的家，拿它做实验不算数',
      )
      return 2
    }
    const minted = runHome(['up', '--token'], homeDir)
    const token = tokenFromUpOutput(minted.stdout)
    if (!token) {
      console.error('[probe-handoff] 取不到 Human token（`up --token` 没回那一行）—— 装配不起来')
      return 2
    }
    console.log('[probe-handoff] 世界起来了 · token 到手（**不打印取值**）')

    // 3) 世界里建一支 Agent ＋ 一间 direct 房
    const created = await api(base, token, 'POST', '/v1/agents', { display_name: '探针小交' })
    const agentId = created.json && created.json.agent ? created.json.agent.id : null
    if (created.status !== 201 || !agentId) {
      console.error(
        `[probe-handoff] 建 Agent 没成：HTTP ${created.status} ${created.text.slice(0, 160)}`,
      )
      return 2
    }
    const room = await api(base, token, 'POST', '/v1/direct-rooms', { agent_id: agentId })
    const roomId = room.json && room.json.room ? room.json.room.id : null
    if (room.status !== 201 || !roomId) {
      console.error(`[probe-handoff] 建房没成：HTTP ${room.status} ${room.text.slice(0, 160)}`)
      return 2
    }
    console.log(`[probe-handoff] Agent = ${agentId.slice(0, 8)}… ｜ 房 = ${roomId.slice(0, 8)}…`)

    // 4) 把她安在**这个临时家**上（环境 ＋ 凭据 ＋ 宿主）
    const adopted = runHome(['up', '--agent', agentId], homeDir)
    if (adopted.status !== 0) {
      console.error('[probe-handoff] 安家没成（up --agent 非 0）：')
      console.error(String(adopted.stdout || '').split('\n').slice(-8).join('\n'))
      return 2
    }
    console.log('[probe-handoff] 她安在这个临时家上了（环境 ＋ 凭据 ＋ 宿主）')

    const envDir = path.join(homeDir, 'agents', agentId)
    const listMessages = async () => {
      const listed = await api(base, token, 'GET', `/v1/rooms/${roomId}/messages?limit=50`)
      return (listed.json && listed.json.messages) || []
    }
    const waitForHer = async (since, seconds) => {
      const deadline = Date.now() + seconds * 1000
      let messages = []
      let lastCount = -1
      while (Date.now() < deadline) {
        messages = await listMessages()
        if (messages.length !== lastCount) {
          lastCount = messages.length
          console.log(`[probe-handoff] （房里 ${messages.length} 条）`)
          for (const each of messages) console.log(`                 ${describeMessage(each)}`)
        }
        if (herMessages(messages.slice(since), agentId).length > 0) return messages
        await new Promise((resolve) => setTimeout(resolve, 5000))
      }
      return messages
    }

    // 5) 第一段：叫她写一份交接、写完收场
    const firstTask = writeTask(mark)
    const sentFirst = await api(base, token, 'POST', `/v1/rooms/${roomId}/messages`, {
      body: firstTask,
      reply_to_message_id: null,
      attachments: [],
      mentions: [],
    })
    if (sentFirst.status !== 201) {
      console.error(
        `[probe-handoff] 发第一条没成：HTTP ${sentFirst.status} ${sentFirst.text.slice(0, 160)}`,
      )
      return 2
    }
    console.log(`[probe-handoff] 主人发了（第一段）：${firstTask}`)
    await waitForHer(0, options.timeoutSeconds)

    // 6) 等她**收场**：经历里出现那一段的结束记录 ⇒ 下一条消息才会开**新的一段**。
    //    ⚠ 她搁下不搁下是她的事（§3.59/§3.60）⇒ 等不到**不报失败**，照发第二条，由判决分档。
    const settleDeadline = Date.now() + Math.min(options.timeoutSeconds, 120) * 1000
    let traceText = readTrace(envDir)
    let runs = traceText === null ? [] : runsIn(traceText)
    let ended = traceText !== null && /"kind":"ending"/.test(traceText)
    while (!ended && Date.now() < settleDeadline) {
      await new Promise((resolve) => setTimeout(resolve, 5000))
      traceText = readTrace(envDir)
      ended = traceText !== null && /"kind":"ending"/.test(traceText)
    }
    console.log(
      ended
        ? '[probe-handoff] 第一段收场了（经历里有结束记录）⇒ 下一条消息会开新的一段'
        : '[probe-handoff] ⚠ 没等到第一段收场 —— 下一条消息可能落回**同一段**（判决里会如实分档）',
    )

    // 7) 第二段：问她"上一次留给自己的那一份里写的是什么"
    const before = await listMessages()
    const sentSecond = await api(base, token, 'POST', `/v1/rooms/${roomId}/messages`, {
      body: REPORT_TASK,
      reply_to_message_id: null,
      attachments: [],
      mentions: [],
    })
    if (sentSecond.status !== 201) {
      console.error(
        `[probe-handoff] 发第二条没成：HTTP ${sentSecond.status} ${sentSecond.text.slice(0, 160)}`,
      )
      return 2
    }
    console.log(`[probe-handoff] 主人发了（第二段）：${REPORT_TASK}`)
    const messages = await waitForHer(before.length, options.timeoutSeconds)

    // 8) 辅助判据：读她这一次的经历（**只用于分类与排除**）
    traceText = readTrace(envDir)
    const traceReadable = traceText !== null
    const actions = traceReadable ? actionsIn(traceText) : []
    const injections = traceReadable ? openingInjections(traceText) : []
    runs = traceReadable ? runsIn(traceText) : []
    const reported = markReported(messages, agentId, mark, before.length)
    console.log(
      traceReadable
        ? `[probe-handoff] 经历：${runs.length} 段（${runs.join(' / ') || '—'}）· ` +
            `${actions.length} 个动作 · ${injections.length} 条开场注入`
        : '[probe-handoff] ⚠ 读不到她的经历 ⇒ 辅助判据缺一格，如实说不知道',
    )

    const verdict = handoffVerdict({
      runs,
      actions,
      injections,
      mark,
      reported,
      traceReadable,
    })
    console.log('')
    // **她的原话要留在结论旁边**（判据只回答"报没报对"；要回答"为什么"就得有她的原话）。
    const hers = herMessages(messages.slice(before.length), agentId)
    if (hers.length > 0) {
      console.log('[probe-handoff] 她第二段说的（判据之外，供人读）：')
      for (const each of hers) console.log(`                 ${describeMessage(each)}`)
    } else {
      console.log('[probe-handoff] 她第二段一句话都没说')
    }

    const modelFailure = readModelFailure(homeDir)
    if (modelFailure) {
      console.log(
        `[probe-handoff] 宿主的模型调用失败：${modelFailure} ⇒ 她没法思考 ⇒ 这一轮**不适用**（不是失败）`,
      )
      if (modelFailure.includes('credential')) {
        console.log(
          '[probe-handoff] 那把钥匙在用户环境里**在场但被服务端拒**（401）——换一把、或摘掉它再跑',
        )
      }
      return 2
    }

    switch (verdict.kind) {
      case 'delivered':
        console.log(
          '[probe-handoff] 结论：**她写了，而且下一段报对了** ⇒ 这条线在真机上成立' +
            `（第二段开场摆过那一块${verdict.shown === true ? '，经历里对得上' : '；经历读不到，只按黑盒判'}` +
            '；她**没有**绕路去读那一份）',
        )
        return 0
      case 'never-wrote':
        console.log(
          '[probe-handoff] 结论：**她没写**（第一段的经历里没有 `write_handoff`）' +
            ' ⇒ **写那一半不成立**：动作目录里那一行没能让她把这一份写下来',
        )
        console.log('[probe-handoff] ⚠ 这一档要人看她的原话与动作：是没找到那个动词，还是没理解任务')
        return 3
      case 'by-detour':
        console.log(
          '[probe-handoff] 结论：**她报对了，但自己绕路读过那一份**（第二段调过 ' +
            `${DETOUR_ACTIONS.join(' / ')}）⇒ **交付那一半不能算**：那一串可能来自她自己翻到的文件，` +
            '不是开场那一块',
        )
        return 5
      case 'contradiction':
        console.log(
          '[probe-handoff] 结论：**两条判据打架** —— 她报对了，而第二段的开场**没有摆过那一块**' +
            '（经历里那一段的 `run-start` 注入里没有带这个码的 `handoff` 块）⇒ **如实报，人工看**',
        )
        return 6
      case 'not-reported':
        console.log(
          '[probe-handoff] 结论：**她写了，但第二段没报对** ⇒ **交付那一半不成立**（不并进"没写"）：' +
            '看她的原话——是开场没摆，还是摆了没读出来',
        )
        return 4
      default:
        console.log(
          `[probe-handoff] 结论：**她没开新的一段**（经历里只有 ${verdict.runs} 段）⇒ **前提不成立**` +
            '（不适用，不是失败）：没有"下一段的开场"可量 —— 她搁下不搁下是她的事',
        )
        return 2
    }
  } finally {
    runHome(['down'], homeDir)
    if (options.keep) {
      console.log(`[probe-handoff] --keep：临时家留着（${homeDir}）`)
    } else {
      try {
        fs.rmSync(homeDir, { recursive: true, force: true })
        console.log('[probe-handoff] 临时家已删（那个世界的账本、凭据与历史一起走）')
      } catch (error) {
        console.error(`[probe-handoff] ⚠ 临时家没删掉（${error.message}）：${homeDir}`)
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
      console.error(`[probe-handoff] 意外失败：${error && error.message ? error.message : error}`)
      process.exitCode = 2
    })
}

// 只有这些是给测试的（纯函数）；真机部分全是副作用，测试里不碰。
module.exports = {
  parseArguments,
  makeMark,
  writeTask,
  WRITE_TASK,
  REPORT_TASK,
  HANDOFF_LABEL,
  WRITE_ACTION,
  DETOUR_ACTIONS,
  isHerMessage,
  herMessages,
  markReported,
  traceRows,
  actionsIn,
  openingInjections,
  runsIn,
  wroteHandoff,
  readByDetour,
  openingShowed,
  handoffVerdict,
  describeMessage,
  readModelFailure,
}
