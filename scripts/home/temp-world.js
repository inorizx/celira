#!/usr/bin/env node
/**
 * **临时世界**：一个可以随便折腾、跑完就删的家（2026-10-08 刀）。
 *
 * 谁在用：`probe-ui.js`（让 `verify:ui` 真的跑起来）、`probe-forward.js`（转发那条真机驱动）。
 * 抽成一个模块的理由与仓里其它地方一样：**同一件事只有一个权威** —— 喂世界那份清单、
 * "8080 上已经有世界就别碰"这条安全判据、以及收尾（杀子进程 / `home.js down` / 删家）
 * 只该写一遍。
 *
 * ## 安全与凭据
 *
 * - **绝不碰真家**：家在系统临时目录里；跑完删掉（调用方决定，见 `stop()`）。
 * - **8080 上若已有世界在答，直接拒绝**（那可能是真家，不在它上面做实验）。
 * - **token 只在内存里**：本模块不打印它、不写文件；要交给子进程时由调用方走 argv。
 *
 * ## 喂的是什么（对着 `client/scripts/verify-ui.mjs` 文件头那份清单）
 *
 * 至少一支 Agent、至少一间房、**一个含 ≥2 个 Human 的群房**、名下至少一条接入绑定 ——
 * 全部走**公开协议**造（不改库、不塞数据）。第二个 Human 用邀请码铸（`new_human` 类）。
 */
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { spawn, spawnSync } = require('node:child_process')

const REPO_DIR = path.resolve(__dirname, '..', '..')
const { worldBaseOf } = require('./conf')
/** ⚠ 家 CLI 在**仓根**（`node home.js up`）。 */
const HOME_JS = path.join(REPO_DIR, 'home.js')
const SERVE_CLIENT = path.join(REPO_DIR, 'scripts', 'serve_client.mjs')
/** 默认世界口（**只在安全判据里当"默认那一格"用**；真正说话用的地址是 `BASE`）。
 *
 *  ⚠ 2026-10-11 改：`BASE` 从前是**写死的常量**，现在是**起完世界之后**从那个家自己的
 *  `home.conf` 读出来的（`conf.worldBaseOf`）。理由见 `startTempWorld` 里那段判据。
 */
const DEFAULT_BASE = 'http://127.0.0.1:8080'
/** 这一次要跟谁说话（`startTempWorld` 起来之后赋值）。 */
let BASE = null
const UI_PORT = 4173

function runHome(args, homeDir) {
  return spawnSync(process.execPath, [HOME_JS, ...args], {
    cwd: REPO_DIR,
    encoding: 'utf8',
    env: { ...process.env, CELIRA_HOME_DIR: homeDir },
  })
}

/** 从 `up --token` 的输出里取那支 token（**只回内存，绝不打印**）。 */
function tokenFromUpOutput(text) {
  const match = /\[token\]\s*(\S+)/.exec(String(text || ''))
  return match ? match[1] : null
}

async function api(base, token, method, url, body) {
  const headers = {}
  if (token) headers.Authorization = `Bearer ${token}`
  if (body !== undefined) {
    headers['content-type'] = 'application/json'
    headers['Idempotency-Key'] = `probe-${Date.now()}-${Math.random().toString(16).slice(2)}`
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

/** 某个地址上是不是**已经有世界在跑**（那可能是真家 ⇒ 绝不在它上面做实验）。 */
async function worldAlreadyRunning(at = DEFAULT_BASE) {
  try {
    const response = await fetch(`${at}/v1/home`, { signal: AbortSignal.timeout(1500) })
    return response.status > 0
  } catch {
    return false
  }
}

/** 等一个 HTTP 口答得上（等状态，不等时长）。 */
async function waitForHttp(url, timeoutMs = 20000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url, { signal: AbortSignal.timeout(1200) })
      if (response.status > 0) return true
    } catch {
      /* 还没起 */
    }
    await new Promise((resolve) => setTimeout(resolve, 400))
  }
  return false
}

/** 装配不起来时的统一说法（**不是**门禁失败）。 */
function assemblyFailed(step, result) {
  const detail = result && result.status !== undefined ? `HTTP ${result.status} ${String(result.text || '').slice(0, 160)}` : String(result)
  const error = new Error(`装配不起来 —— ${step}：${detail}`)
  error.assembly = true
  return error
}

/**
 * 起一个喂饱的临时世界。
 *
 * @returns `{ homeDir, base, uiBase, token, ids, stop }`；装配不起来时 **throw**（带 `.assembly`）。
 */
async function startTempWorld({ quiet = false, withUi = true, uiPort = UI_PORT } = {}) {
  const say = (...args) => {
    if (!quiet) console.log(...args)
  }

  // **"别在别人的世界上做实验"这条判据，2026-10-11 换了个形状**（原判据："8080 上有世界在答
  // 就直接拒绝跑"）。原判据是一条**代理**，而它两头都不对：
  //   · **太宽**：主人正开着自己的家时，临时世界本来可以起在别的口上 —— 门禁却被整条拦掉；
  //   · **太窄**：它只看 8080，管不住"我最后到底跟哪个口说了话"。
  // 真正的不变量是——**我绝不跟我没起的那个世界说话**。现在直接按它来：
  //   ① 地址从**我刚起的那个家自己的** `home.conf` 读（`up` 建新家时本来就会避开被占的口）；
  //   ② **读不到就拒绝跑**，绝不回落到默认口；
  //   ③ 另加一条 fail-closed 断言：万一我拿到的**正是默认那一格**、而它起世界之前就在答，
  //      那就说明"避开口"这件事坏了 ⇒ 拒绝跑（这是防上游退化的，不是防主人的）。
  const defaultWasBusy = await worldAlreadyRunning(DEFAULT_BASE)
  if (defaultWasBusy) {
    say(`[temp-world] ${DEFAULT_BASE} 上有一个世界在跑（**可能是真家**）——本探针不碰它，自己另起一个口`)
  }

  const homeDir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-temp-world-'))
  const children = []
  say(`[temp-world] 临时家 = ${homeDir}（**全程不碰真家**）`)

  const stop = () => {
    for (const child of children) {
      try {
        child.kill()
      } catch {
        /* 已退出 */
      }
    }
    runHome(['down'], homeDir)
  }

  try {
    const up = runHome(['up'], homeDir)
    if (up.status !== 0) {
      throw assemblyFailed('那个家起不来（node home.js up 非 0）', String(up.stdout || '').split('\n').slice(-4).join(' / '))
    }
    // **世界地址现在才知道**：那个家刚把挑到的口写进它自己的 `home.conf`。
    BASE = worldBaseOf(homeDir)
    if (!BASE) {
      throw assemblyFailed(
        '读不到这个家自己的世界地址（`home.conf` 的 `listen_addr`）',
        '默认的 8080 上可能是别人的家 ⇒ 拿它做实验不算数（本探针**不**回落到默认口）',
      )
    }
    if (defaultWasBusy && BASE === DEFAULT_BASE) {
      throw assemblyFailed(
        `这个家挑到的口正是 ${DEFAULT_BASE}，而它起世界之前就在答`,
        '「新家避开口」这件事坏了 ⇒ 拒绝跑（不在别人的世界上做实验）',
      )
    }
    const minted = runHome(['up', '--token'], homeDir)
    const token = tokenFromUpOutput(minted.stdout)
    if (!token) throw assemblyFailed('取不到 Human token（`up --token` 没回那一行）', '')
    say('[temp-world] 世界起来了 · token 到手（**不打印取值**）')

    // ---- 喂世界（全走公开协议）----------------------------------------------
    const agent = await api(BASE, token, 'POST', '/v1/agents', { display_name: '探针小测' })
    const agentId = agent.json && agent.json.agent ? agent.json.agent.id : null
    if (!agentId) throw assemblyFailed('建 Agent', agent)

    const room = await api(BASE, token, 'POST', '/v1/direct-rooms', { agent_id: agentId })
    const directRoomId = room.json && room.json.room ? room.json.room.id : null
    if (!directRoomId) throw assemblyFailed('建与它的私人房', room)

    const binding = await api(BASE, token, 'POST', `/v1/agents/${agentId}/backend-bindings`, {
      label: '探针绑定',
    })
    if (binding.status !== 201) throw assemblyFailed('建接入绑定', binding)
    // **这支 Agent 的 BackendAuth 凭据**（2026-10-10 加）：凭据由**另一条端点**签发、
    // **只在 201 那一刻回一次**（`POST …/backend-bindings/{id}/credentials`，契约 §2.3）
    // ⇒ 夹具留着，交给门禁去**推进那支 Agent 的读游标**（判「对方已读」那一支要用它）。
    // ⚠ 凭据纪律：只经 env 交子进程、**不打印、不写文件、不进日志**（与 Human token 同规）。
    const bindingId = binding.json && binding.json.binding ? binding.json.binding.id : null
    let agentCredential = null
    if (bindingId) {
      const issued = await api(
        BASE,
        token,
        'POST',
        `/v1/agents/${agentId}/backend-bindings/${bindingId}/credentials`,
        // 首次签发：**没有前驱**（契约 §2.3 的体就是这一格；空体是 400，实测过）。
        { replaces_credential_id: null },
      )
      agentCredential =
        (issued.json && ((issued.json.credential || {}).token || issued.json.token)) || null
      // 拿不到时**只打印形状**（状态码 + 顶层键名 + credential 的键名）——**绝不打印取值**。
      if (!agentCredential) {
        say(
          `[temp-world] 凭据签发读数：status=${issued.status} 顶层键=${issued.json ? Object.keys(issued.json).join(',') : '(非 JSON)'}` +
            (issued.json && issued.json.credential
              ? ` credential 键=${Object.keys(issued.json.credential).join(',')}`
              : ''),
        )
      }
    }
    say(`[temp-world] 那支 Agent 的凭据：${agentCredential ? '在场' : '没拿到'}（不打印取值）`)

    const grant = await api(BASE, token, 'POST', '/v1/home/pairing-grants', { grants: 'new_human' })
    const code = grant.json ? grant.json.token : null
    if (!code) throw assemblyFailed('签发邀请码', grant)

    const exchanged = await api(BASE, code, 'POST', '/v1/home/pairing-grants/exchange', {
      display_name: '探针第二位',
    })
    const secondToken = exchanged.json && (exchanged.json.token || (exchanged.json.session || {}).token)
    const secondId =
      exchanged.json &&
      ((exchanged.json.session || {}).human_id ||
        exchanged.json.human_id ||
        (exchanged.json.participant || {}).id)
    if (!secondToken || !secondId) throw assemblyFailed('兑换邀请码（第二个 Human）', exchanged)

    const me = await api(BASE, token, 'GET', '/v1/me')
    const myId = me.json && (me.json.participant ? me.json.participant.id : me.json.id)
    if (!myId) throw assemblyFailed('读我自己是谁', me)

    const request = await api(BASE, token, 'POST', '/v1/contacts/requests', { participant_id: secondId })
    if (request.status !== 201 && request.status !== 200) throw assemblyFailed('发关系申请', request)
    const accepted = await api(BASE, secondToken, 'POST', `/v1/contacts/${myId}/accept`)
    if (accepted.status !== 204 && accepted.status !== 200) throw assemblyFailed('第二个 Human 接受关系', accepted)

    const group = await api(BASE, token, 'POST', '/v1/groups', { name: '探针群' })
    const groupId = group.json && group.json.room ? group.json.room.id : null
    if (!groupId) throw assemblyFailed('建群房', group)
    const invited = await api(BASE, token, 'POST', `/v1/rooms/${groupId}/members`, {
      participant_id: secondId,
    })
    if (invited.status !== 201 && invited.status !== 200) throw assemblyFailed('把第二个 Human 拉进群', invited)
    say('[temp-world] 世界喂好了：1 支 Agent · 1 间私人房 · 1 个含 2 个 Human 的群 · 1 条绑定')

    const ids = { agentId, directRoomId, groupId, myId, secondId, secondToken, agentCredential }

    // ---- 静态页服务（构建产物 + /v1 代转）＋ 模型配置门 ------------------------
    let uiBase = null
    if (withUi) {
      const ui = spawn(
        process.execPath,
        [SERVE_CLIENT, '--dist', 'client/dist', '--port', String(uiPort), '--im', BASE, '--home', homeDir],
        { cwd: REPO_DIR, stdio: 'ignore' },
      )
      children.push(ui)
      if (!(await waitForHttp(`http://127.0.0.1:${uiPort}/`))) {
        throw assemblyFailed(`静态页服务没起来（:${uiPort}）`, '先跑一次 npm run build 出 client/dist')
      }
      uiBase = `http://localhost:${uiPort}`
      const config = spawn(process.execPath, [HOME_JS, 'config', '--serve'], {
        cwd: REPO_DIR,
        stdio: 'ignore',
        env: { ...process.env, CELIRA_HOME_DIR: homeDir },
      })
      children.push(config)
      say(`[temp-world] 页面与两个口就位（${uiBase}）`)
    }

    return { homeDir, base: BASE, uiBase, token, ids, homeDirCleanup: () => fs.rmSync(homeDir, { recursive: true, force: true }), stop }
  } catch (error) {
    stop()
    fs.rmSync(homeDir, { recursive: true, force: true })
    throw error
  }
}

// ⚠ **`BASE` 不在导出里**（2026-10-11）：它从前是常量，现在是**起完世界之后**才赋值的
// 模块级变量 ⇒ 导出它等于导出一个"读到的时候还是 `null`"的陷阱。世界地址由
// `startTempWorld()` 的返回值 `base` 交给调用方（那才是它该走的唯一一条路）。
module.exports = { startTempWorld, api, runHome, DEFAULT_BASE, REPO_DIR }
