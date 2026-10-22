#!/usr/bin/env node
'use strict'
/**
 * **产品核心回路的真机探针**：人发一条 ⇒ im ⇒ 门铃把她唤醒 ⇒ 她想一次 ⇒ 把话写回世界。
 *
 * ## 为什么要有它（2026-10-05 记账）
 *
 * 这条链是产品的核心，而它**从来没有注册在案的真机门禁** —— 各组件自己的套件只证明各自那一半
 * （im 的契约、Runtime 的判定、页面的渲染），"**一条消息真的能把一支 Agent 叫醒并让它说回来**"
 * 只有真机跑得出来。2026-10-05 是拿一次性脚本在临时世界里验的：验过了，但脚本躺在临时目录里，
 * 下次就没了 —— 而仓库当天最大的教训正是"**真机门禁必须把触发条件写死，否则它自己会死掉**"
 * （`probe-agent-im.js` 就是活例子：住户化之后它在任何现代的家上都装配不起来，没人知道）。
 *
 * ## 它做什么（**全程不碰真家**）
 *
 * 1. 在系统临时目录里造一个**全新的家**，在那个家里 `node home.js up`（起世界 ＋ 铸主人 ＋
 *    默认 Agent ＋ 两道门）；
 * 2. 拿一支**那个家的** Human token（**绝不打印、绝不落盘**）；
 * 3. 世界里建一支 Agent（`POST /v1/agents`）＋ 与她的一间 `direct` 房（`POST /v1/direct-rooms`）；
 * 4. 把她安在**那个家**上（`node home.js up --agent <id>` ⇒ 环境 ＋ 凭据 ＋ 宿主）；
 * 5. **人发一条**（`POST /v1/rooms/<房>/messages`）—— 人那一半走**公共路径**，与页面同一条协议；
 * 6. **等她回**：轮询房里的消息，直到出现一条 `actor.id === 她` 的（**读到的每一条都原样打出来**，
 *    不替读数下结论）；超时 ⇒ **如实报"没等到"**，并把房里所有消息摆出来；
 * 7. **收尾**：`node home.js down` 那个家 ＋ **删掉整个临时家**（`finally` 里也删）。
 *
 * ## 用法与退出码
 *
 * ```text
 * node scripts/home/probe-loop.js [--timeout <秒>] [--keep] [--message <一句话>]
 *                                [--model-sources <清单路径>] [--model-source-id <条目 id>]
 * ```
 *
 * - `--timeout <秒>`：等她回的上限（缺省 240）；
 * - `--keep`：跑完**留着**那个临时家（排查用；正常跑一律删）；
 * - `--message <一句话>`：主人发的**那一句**（缺省是"看到就回我一句"）。**为什么要开这个口子**
 *   （2026-10-11 记账）：有些判据要**逼出特定的一条路**才量得到——例如"能力扩展那一下会不会
 *   撞坏缓存前缀"，就必须让她**不得不** `open_app`（比如去读一个文件），而"回我一句"永远逼不出来。
 *   ⚠ 它只改**输入**，不改判据：退出码与"这一轮算不算回了"一个字不变；
 * - `--model-sources <清单路径>`：**她用什么想**。缺省取**本机默认家**的清单
 *   （`<默认家>/model_sources.json`，与 `node home.js up` 交给宿主的是同一份）；
 * - `--model-source-id <id>`：清单里用**哪一条**（缺省用第一条）；
 * - 退出码：**0 全通** ／ **2 装配不起来**（含"这台机器上没有任何模型来源 ⇒ 她没法思考"这一档
 *   —— 那是**不适用**，不是失败）／ **3 确定的失败**（她没回）。
 *
 * ## 她用什么想（2026-10-09 改）
 *
 * 两条来路，**清单优先**：
 *
 * 1. **清单 ＋ 绑定**（今天正常家用的就是这条）：探针把**那一条**写进临时家的清单，并在那个
 *    临时家的环境根里**先放好绑定**——`up --agent` 认出这份"已经配好"的环境根（`seeded`），
 *    于是不要求 `DEEPSEEK_API_KEY`、并把清单交给宿主。⇒ 门禁跟着**这台机器真正的配置**走，
 *    换供应商/换型号都不用改探针；
 * 2. **用户环境变量 `DEEPSEEK_API_KEY`**（老形状，清单不在场时的兜底）：原样不动。
 *
 * ⚠ **钥匙不打印、不落盘到别处**：清单那一条被复制进**临时家**（跑完随家一起删）。
 * `--keep` 会把它留在盘上 ⇒ 排查完自己删掉（那一档会多打一行提醒）。
 *
 * ⚠ 它是**真机门禁**：会起真世界、真宿主，**并花掉一次真模型调用**（用这台机器上那把钥匙）。
 * 触发条件写在 `AGENTS.md`「验证与汇报」：**改动消息/唤醒那条链时必须跑它**。
 */

const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { spawnSync } = require('node:child_process')

const { readUserEnv } = require('./userenv')
const { worldBaseOf } = require('./conf')
const modelSources = require('./model-sources')
const paths = require('./paths')

const REPO_DIR = path.resolve(__dirname, '..', '..')
const HOME_JS = path.join(REPO_DIR, 'home.js')
const MODEL_KEY_NAME = 'DEEPSEEK_API_KEY'

/** 参数解析（未知选项**明确失败**，不静默忽略）。 */
function parseArguments(argv) {
  const options = {
    timeoutSeconds: 240,
    keep: false,
    modelSources: null,
    modelSourceId: null,
    message: null,
  }
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
    } else if (flag === '--message') {
      const value = argv[index + 1]
      if (typeof value !== 'string' || value.trim() === '' || value.startsWith('--')) {
        throw new Error('--message 后面要给一句非空的话')
      }
      options.message = value.trim()
      index += 1
    } else if (flag === '--model-sources' || flag === '--model-source-id') {
      const value = argv[index + 1]
      if (typeof value !== 'string' || value.trim() === '' || value.startsWith('--')) {
        throw new Error(`${flag} 后面要给一个非空取值`)
      }
      if (flag === '--model-sources') options.modelSources = value.trim()
      else options.modelSourceId = value.trim()
      index += 1
    } else {
      throw new Error(`不认识的参数：${flag}`)
    }
  }
  return options
}

/**
 * **清单里用哪一条**（纯函数 ⇒ 可判错）。
 *
 * 判据三条，各自独立、不许互相顶替：清单必须是**非空数组**；给了 id 就必须**找得到**
 * （找不到是**用法错**，不是"回落到第一条"——静默选一条会让操作者以为跑的是他指的那条）；
 * 没给 id 就用**第一条**。选中的那一条还必须**四件齐**（端点 / 钥匙 / 型号 / 窗口）——
 * 缺一件就是那份清单坏了，如实说出来。
 */
function chooseModelSource(entries, wantedId) {
  const list = Array.isArray(entries) ? entries : []
  if (list.length === 0) {
    return { ok: false, reason: '清单里一条模型来源都没有（空的）—— 去「模型配置」页加一条' }
  }
  let entry = null
  if (wantedId) {
    entry = list.find((each) => each && each.id === wantedId) ?? null
    if (!entry) {
      const ids = list.map((each) => (each && each.id) || '?').join('、')
      return {
        ok: false,
        reason: `清单里没有 id 为 ${wantedId} 的那一条（有的：${ids}）—— 换一个 --model-source-id`,
      }
    }
  } else {
    entry = list[0]
  }
  const missing = ['endpoint', 'api_key', 'model'].filter(
    (field) => typeof entry[field] !== 'string' || entry[field].trim() === '',
  )
  if (missing.length > 0) {
    return { ok: false, reason: `清单里 ${entry.id || '第一条'} 缺 ${missing.join(' / ')}` }
  }
  return { ok: true, entry }
}

/**
 * 那一条的**读数行**（给人看的）：id · 名字 · 型号 · 端点主机。**永不打印钥匙**。
 *
 * 单测里有一条负向断言钉着它（取值里塞一个哨兵钥匙 ⇒ 读数行里不许出现）。
 */
function describeModelSource(entry) {
  const host = (() => {
    try {
      return new URL(entry.endpoint).host
    } catch {
      return '<端点不是合法 URL>'
    }
  })()
  const name = entry.name ? ` · ${entry.name}` : ''
  return `${entry.id}${name} · 型号 ${entry.model} · ${host}`
}

/**
 * **这条消息是她说的吗** —— 判据只看**身份**，不看正文（正文是她的，探针不替她判断说了什么）。
 *
 * ⚠ 2026-10-05 的教训（这个函数就是为它写的）：第一次我按 `sender.kind` 过滤 —— 而消息载荷里
 * 那一格叫 **`actor`**（`{id, kind, display_name, status}`）⇒ 过滤器把她的回信当成不认识的行，
 * 于是**她已经回了，脚本却报"没等到"**。判据落在这里、被单测钉住，就是为了不再犯。
 */
function isHerMessage(message, agentId) {
  if (!message || typeof message !== 'object') return false
  const actor = message.actor
  if (!actor || typeof actor !== 'object') return false
  if (typeof agentId !== 'string' || agentId === '') return false
  return actor.id === agentId
}

/** 她回的那几条（按时间序，原样交给调用方打印）。 */
function herMessages(messages, agentId) {
  return (Array.isArray(messages) ? messages : []).filter((each) => isHerMessage(each, agentId))
}

/**
 * **这一轮的判决**（纯函数 ⇒ 可判错）：`replied` ／ `not-yet` ／ `absent`。
 * 三条判据各自独立，不许互相顶替。
 */
function loopVerdict({ messages, agentId }) {
  const hers = herMessages(messages, agentId)
  if (hers.length > 0) return { kind: 'replied', hers }
  return { kind: 'not-yet', hers: [] }
}

/** 一条消息的一行人话（读数用：谁、什么时候、说了什么）。 */
function describeMessage(message) {
  const actor = message && message.actor ? message.actor : {}
  const who = `${actor.kind || '?'}:${String(actor.id || '?').slice(0, 8)}`
  const when = message && message.created_at ? message.created_at : '?'
  const body = String((message && message.body) || '').replace(/\s+/g, ' ').slice(0, 120)
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
    headers['Idempotency-Key'] = `probe-loop-${Date.now()}-${Math.random().toString(16).slice(2)}`
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

async function main() {
  let options
  try {
    options = parseArguments(process.argv.slice(2))
  } catch (error) {
    console.error(`[probe-loop] ${error.message}`)
    console.error('用法: node scripts/home/probe-loop.js [--timeout <秒>] [--keep]')
    return 2
  }

  // 前置：**她用什么想** —— 清单优先（今天正常家的形状），用户环境变量兜底（老形状）。
  //
  // ⚠ 判据是"这台机器上有没有**任何一个**模型来源"，不是"有没有那个环境变量"：清单在场而
  // 环境变量不在，是**今天的正常形状**（`node home.js up` 本来就把清单交给宿主）。
  const listFile = options.modelSources || modelSources.listPath(paths.homeDir())
  let chosen = null
  if (fs.existsSync(listFile)) {
    let entries
    try {
      entries = modelSources.readEntriesAt(listFile)
    } catch (error) {
      console.error(`[probe-loop] 清单读不出来（${listFile}）：${error.message}`)
      return 2
    }
    const picked = chooseModelSource(entries, options.modelSourceId)
    if (!picked.ok) {
      console.error(`[probe-loop] ${picked.reason}`)
      console.error(`[probe-loop] 清单：${listFile}`)
      return 2
    }
    chosen = picked.entry
    console.log(`[probe-loop] 她用什么想：本机清单 ${listFile}`)
    console.log(`             ${describeModelSource(chosen)}（**钥匙不打印**）`)
  } else {
    const modelKey = readUserEnv(MODEL_KEY_NAME)
    if (!modelKey) {
      console.error(
        `[probe-loop] 这台机器上没有任何模型来源 ⇒ 她没法思考 ⇒ 这一轮**不适用**（不是失败）。`,
      )
      console.error(`[probe-loop] 两条来路任选其一：① 在「模型配置」页加一条（写成 ${listFile}）；`)
      console.error(`[probe-loop] ② 或者配一次用户环境变量 ${MODEL_KEY_NAME}。`)
      return 2
    }
    console.log(`[probe-loop] 她用什么想：用户环境变量 ${MODEL_KEY_NAME}（**不打印取值**）`)
  }

  const homeDir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-loop-probe-'))
  // ⚠ **世界地址由那个家自己说了算**（2026-10-11）：`up` 建新家时会避开被占的口，并把挑到的
  // 那一个写进**那个家自己的** `home.conf`（见 `conf.worldBaseOf`）。从前这里写死
  // `127.0.0.1:8080` ⇒ 主人开着自己的家时这条门禁**根本跑不了**；而临时世界起在 8081、
  // 探针却去跟 8080 上的**别人的世界**说话，是更糟的那一种形状。
  let base = null
  console.log(`[probe-loop] 临时家 = ${homeDir}（**全程不碰真家**）`)

  try {
    // 1) 起世界（那个家里）＋ 2) 拿 token
    const up = runHome(['up'], homeDir)
    if (up.status !== 0) {
      console.error('[probe-loop] 那个家起不来（node home.js up 非 0）：')
      console.error(String(up.stdout || '').split('\n').slice(-8).join('\n'))
      return 2
    }
    // **世界地址现在才知道**：那个家刚把挑到的口写进它自己的 `home.conf`。
    // ⚠ 读不到就**拒绝跑**——绝不回落到默认的 8080（那上面可能是主人正开着的家）。
    base = worldBaseOf(homeDir)
    if (!base) {
      console.error(
        '[probe-loop] 读不到这个家自己的世界地址（`home.conf` 的 `listen_addr`）⇒ **不跑**：' +
          '默认的 8080 上可能是别人的家，拿它做实验不算数',
      )
      return 2
    }
    const minted = runHome(['up', '--token'], homeDir)
    const token = tokenFromUpOutput(minted.stdout)
    if (!token) {
      console.error('[probe-loop] 取不到 Human token（`up --token` 没回那一行）—— 装配不起来')
      return 2
    }
    console.log('[probe-loop] 世界起来了 · token 到手（**不打印取值**）')

    // 3) 世界里建一支 Agent ＋ 一间 direct 房
    const created = await api(base, token, 'POST', '/v1/agents', { display_name: '探针小测' })
    const agentId = created.json && created.json.agent ? created.json.agent.id : null
    if (created.status !== 201 || !agentId) {
      console.error(`[probe-loop] 建 Agent 没成：HTTP ${created.status} ${created.text.slice(0, 160)}`)
      return 3
    }
    const room = await api(base, token, 'POST', '/v1/direct-rooms', { agent_id: agentId })
    const roomId = room.json && room.json.room ? room.json.room.id : null
    if (room.status !== 201 || !roomId) {
      console.error(`[probe-loop] 建房没成：HTTP ${room.status} ${room.text.slice(0, 160)}`)
      return 3
    }
    console.log(`[probe-loop] Agent = ${agentId.slice(0, 8)}… ｜ 房 = ${roomId.slice(0, 8)}…`)

    // 3b) 把**那一份模型来源**交给这个临时家：清单一条 ＋ 环境根里先放好绑定。
    //
    // 为什么要"先放好"：`up --agent` 认这份环境根已经配过（`seeded`）⇒ 它**不要求**
    // `DEEPSEEK_API_KEY`，并把清单交给宿主。⇒ 探针跟着这台机器**真正的配置**走。
    if (chosen) {
      modelSources.writeList(homeDir, [chosen])
      const envDir = paths.agentEnvRoot(homeDir, agentId)
      fs.mkdirSync(envDir, { recursive: true })
      modelSources.writeBinding(envDir, chosen.id)
      console.log(
        '[probe-loop] 那个临时家拿到了同一份模型来源（清单一条 ＋ 绑定指向它）——钥匙只留在临时家里，跑完随家一起删',
      )
    }

    // 4) 把她安在**这个临时家**上（环境 ＋ 凭据 ＋ 宿主）
    const adopted = runHome(['up', '--agent', agentId], homeDir)
    if (adopted.status !== 0) {
      console.error('[probe-loop] 安家没成（up --agent 非 0）：')
      console.error(String(adopted.stdout || '').split('\n').slice(-8).join('\n'))
      return 2
    }
    console.log('[probe-loop] 她安在这个临时家上了（环境 ＋ 凭据 ＋ 宿主）')

    // 5) 人发一条（公共路径，与页面同一条协议）
    const line = options.message || `探针消息 ${new Date().toISOString()}：看到就回我一句。`
    const sent = await api(base, token, 'POST', `/v1/rooms/${roomId}/messages`, {
      body: line,
      reply_to_message_id: null,
      attachments: [],
      mentions: [],
    })
    if (sent.status !== 201) {
      console.error(`[probe-loop] 发消息没成：HTTP ${sent.status} ${sent.text.slice(0, 160)}`)
      return 3
    }
    console.log(`[probe-loop] 主人发了：${line}`)

    // 6) 等她回（**把读到的每一条原样打出来** —— 不替读数下结论）
    const deadline = Date.now() + options.timeoutSeconds * 1000
    let lastCount = -1
    let verdict = { kind: 'not-yet', hers: [] }
    while (Date.now() < deadline) {
      const listed = await api(base, token, 'GET', `/v1/rooms/${roomId}/messages?limit=50`)
      const messages = (listed.json && listed.json.messages) || []
      if (messages.length !== lastCount) {
        lastCount = messages.length
        console.log(`[probe-loop] （房里 ${messages.length} 条）`)
        for (const each of messages) console.log(`             ${describeMessage(each)}`)
      }
      verdict = loopVerdict({ messages, agentId })
      if (verdict.kind === 'replied') break
      await new Promise((resolve) => setTimeout(resolve, 5000))
    }

    if (verdict.kind === 'replied') {
      console.log('')
      console.log(`[probe-loop] 结论：**她回了**（${verdict.hers.length} 条，等她 ${options.timeoutSeconds} 秒以内）`)
      for (const each of verdict.hers) console.log(`             ${describeMessage(each)}`)
      return 0
    }
    console.log('')
    console.log(`[probe-loop] 结论：**没等到她回**（等了 ${options.timeoutSeconds} 秒）—— 如实报，不假装`)
    console.log('[probe-loop] 房里读到的每一条都在上面（谁发的、什么时候、说了什么）')
    return 3
  } finally {
    runHome(['down'], homeDir)
    if (options.keep) {
      console.log(`[probe-loop] --keep：临时家留着（${homeDir}）`)
      if (chosen) {
        console.log(
          '[probe-loop] ⚠ 那个临时家里有**你那份清单那一条的副本**（含钥匙）——排查完请自己删掉它',
        )
      }
    } else {
      try {
        fs.rmSync(homeDir, { recursive: true, force: true })
        console.log('[probe-loop] 临时家已删（那个世界的账本、凭据与历史一起走）')
      } catch (error) {
        console.error(`[probe-loop] ⚠ 临时家没删掉（${error.message}）：${homeDir}`)
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
      console.error(`[probe-loop] 意外失败：${error && error.message ? error.message : error}`)
      process.exitCode = 2
    })
}

// 只有这几个是给测试的（纯函数）；真机部分全是副作用，测试里不碰。
module.exports = {
  parseArguments,
  isHerMessage,
  herMessages,
  loopVerdict,
  describeMessage,
  chooseModelSource,
  describeModelSource,
}
