'use strict'
// **世界归属**（`world.js`）与 **im 认领裁决**（`up.js` 的 `imListenerVerdict`）的机械判据。
//
// 这一刀要钉住的是 2026-09-25 真机踩到的那一格：同一份发布包 + 同一组默认端口时，"家事实"
// 两维同时退化（可执行件路径相同、端口也相同）⇒ 本家把**别家的世界**认成自己的，`down`
// 可能停掉别家的世界。判据分三层：
//   1. 三态裁决是**纯函数**（喂一份回执就够）⇒ 可机械判错；
//   2. 认领裁决的四条动作逐条钉住（含"判不了 ≠ 不是"）；
//   3. **真进程黑盒**：端口上真跑着一个世界、而本家证明不了它是自己的 ⇒ `down` **不许杀**。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { spawn, spawnSync } = require('node:child_process')

const { classifyOwnership, proveWorldOwnership } = require('../world')
const { imListenerVerdict } = require('../up')
const P = require('../paths')

let cases = 0
function worldCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

const ME = (agentId, status = 200) => ({
  status,
  body: status === 200 ? { agent: { id: agentId, display_name: 'Agent' } } : { error: 'invalid_credential' },
})

// ---------------------------------------------------------------------------
// 1) 三态裁决（纯函数）
// ---------------------------------------------------------------------------

worldCase('三态：认下凭据且代表本家的 Agent ⇒ ours', () => {
  assert.equal(classifyOwnership({ credential: 'c', agentId: 'agent-1', me: ME('agent-1') }), 'ours')
})

worldCase('三态：认下凭据却代表**别的** Agent ⇒ foreign（铁证：那是别家的世界）', () => {
  assert.equal(classifyOwnership({ credential: 'c', agentId: 'agent-1', me: ME('agent-2') }), 'foreign')
})

worldCase('三态：没凭据 / 没 agent_id / 不认这支凭据 / 回执里没有 Agent ⇒ 一律 unknown（**不是**"不是"）', () => {
  assert.equal(classifyOwnership({ credential: null, agentId: 'agent-1', me: ME('agent-1') }), 'unknown')
  assert.equal(classifyOwnership({ credential: 'c', agentId: null, me: ME('agent-1') }), 'unknown')
  assert.equal(classifyOwnership({ credential: 'c', agentId: 'agent-1', me: ME('agent-1', 401) }), 'unknown')
  assert.equal(classifyOwnership({ credential: 'c', agentId: 'agent-1', me: { status: 200, body: {} } }), 'unknown')
  assert.equal(classifyOwnership({ credential: 'c', agentId: 'agent-1', me: null }), 'unknown')
})

// ---------------------------------------------------------------------------
// 2) 真的问一句（注入 HTTP ⇒ 不起进程也能判）
// ---------------------------------------------------------------------------

worldCase('proveWorldOwnership：问的是 `/v1/backend/me`，带本家的凭据', async () => {
  const seen = []
  const request = async (base, method, pathname, options) => {
    seen.push({ base, method, pathname, bearer: options.bearer })
    return ME('agent-1')
  }
  const proved = await proveWorldOwnership({
    base: 'http://127.0.0.1:8080',
    credential: 'the-credential',
    agentId: 'agent-1',
    imRequest: request,
  })
  assert.equal(proved.state, 'ours')
  assert.deepEqual(seen, [
    { base: 'http://127.0.0.1:8080', method: 'GET', pathname: '/v1/backend/me', bearer: 'the-credential' },
  ])
})

worldCase('proveWorldOwnership：没凭据 ⇒ 连问都不问（省一次 PowerShell 与一次 HTTP）', async () => {
  let called = 0
  const proved = await proveWorldOwnership({
    base: 'http://127.0.0.1:8080',
    credential: null,
    agentId: 'agent-1',
    imRequest: async () => {
      called += 1
      return ME('agent-1')
    },
  })
  assert.equal(proved.state, 'unknown')
  assert.equal(proved.reason, 'no-credential')
  assert.equal(called, 0)
})

worldCase('proveWorldOwnership：连不上 / 401 各自成读数（"判不了"要说得清是哪种）', async () => {
  const unreachable = await proveWorldOwnership({
    base: 'http://127.0.0.1:1',
    credential: 'c',
    agentId: 'agent-1',
    imRequest: async () => ({ status: 0, body: null, error: 'ECONNREFUSED' }),
  })
  assert.equal(unreachable.state, 'unknown')
  assert.equal(unreachable.reason, 'unreachable')

  const rejected = await proveWorldOwnership({
    base: 'http://127.0.0.1:1',
    credential: 'c',
    agentId: 'agent-1',
    imRequest: async () => ME('agent-1', 401),
  })
  assert.equal(rejected.state, 'unknown')
  assert.equal(rejected.reason, 'status-401')
})

worldCase('proveWorldOwnership：认下却是别人的 Agent ⇒ foreign，并把"它说是谁"带回来', async () => {
  const proved = await proveWorldOwnership({
    base: 'http://127.0.0.1:8080',
    credential: 'c',
    agentId: 'agent-1',
    imRequest: async () => ME('agent-2'),
  })
  assert.equal(proved.state, 'foreign')
  assert.equal(proved.represented, 'agent-2')
})

// ---------------------------------------------------------------------------
// 3) 认领裁决（四条动作逐条钉住）
// ---------------------------------------------------------------------------

worldCase('裁决：端口上什么都没有 ⇒ none（走后面的健康探/起一支）', () => {
  const verdict = imListenerVerdict({ ledgerPid: null, listener: null, proof: null, hasAgent: false, port: 8080 })
  assert.equal(verdict.action, 'none')
})

worldCase('裁决：账本里那支就是端口上那支（或端口空着但账本有）⇒ adopt，**不问证明**', () => {
  const same = imListenerVerdict({ ledgerPid: 111, listener: 111, proof: null, hasAgent: true, port: 8080 })
  assert.equal(same.action, 'adopt')
  assert.equal(same.pid, 111)
  const ledgerOnly = imListenerVerdict({ ledgerPid: 111, listener: null, proof: null, hasAgent: true, port: 8080 })
  assert.equal(ledgerOnly.action, 'adopt')
  assert.equal(ledgerOnly.pid, 111)
})

worldCase('裁决：账本陈旧但**证明是本家的** ⇒ adopt 端口上那支（接管，不重起）', () => {
  const verdict = imListenerVerdict({ ledgerPid: null, listener: 222, proof: 'ours', hasAgent: true, port: 8080 })
  assert.equal(verdict.action, 'adopt')
  assert.equal(verdict.pid, 222)
  assert.match(verdict.lines.join('\n'), /账本已对上真实/)
})

worldCase('裁决：**铁证不是本家**（foreign）⇒ stop，且读数点名"不动它"', () => {
  const verdict = imListenerVerdict({ ledgerPid: null, listener: 333, proof: 'foreign', hasAgent: true, port: 8080 })
  assert.equal(verdict.action, 'stop')
  assert.equal(verdict.pid, null)
  assert.match(verdict.lines.join('\n'), /不是本家的/)
})

worldCase('裁决：判不了 + 还没布置过（没有 agent_id）⇒ refuse-stop（那支不可能是本家的世界）', () => {
  const verdict = imListenerVerdict({ ledgerPid: null, listener: 444, proof: 'unknown', hasAgent: false, port: 8080 })
  assert.equal(verdict.action, 'refuse-stop')
  assert.match(verdict.lines.join('\n'), /还没布置过/)
})

worldCase('裁决：判不了 + 已布置过 ⇒ refuse-continue（不认领、不写账本，但保住凭据恢复路径）', () => {
  const verdict = imListenerVerdict({ ledgerPid: null, listener: 555, proof: 'unknown', hasAgent: true, port: 8080 })
  assert.equal(verdict.action, 'refuse-continue')
  const text = verdict.lines.join('\n')
  assert.match(text, /不认领、不写账本/)
  assert.match(text, /凭据格被别家占了/, '要说清这一档为什么不能停手')
})

worldCase('裁决：判不了但**账本里有本家那支** ⇒ 按账本算（adopt），并如实说端口上那支没证明', () => {
  const verdict = imListenerVerdict({ ledgerPid: 666, listener: 777, proof: 'unknown', hasAgent: true, port: 8080 })
  assert.equal(verdict.action, 'adopt')
  assert.equal(verdict.pid, 666)
  assert.match(verdict.lines.join('\n'), /证明不了是本家的/)
})

// ---------------------------------------------------------------------------
// 4) 真进程黑盒：`down` 不许杀掉"证明不了是本家的"那个世界
// ---------------------------------------------------------------------------

/**
 * 端口空着吗（起真服务之前先问一句，免得撞上主人自己跑着的东西）。
 *
 * 判据是"**有没有人在听**（LISTEN）"，不是"有没有任何套接字"：TIME-WAIT / ESTABLISHED
 * 不是占用，而 Linux 的 netstat 会列出 TIME-WAIT —— 迁移前那版会让刚跑过一轮的用例
 * 无故跳过（看着绿，其实什么都没验）。读不到表 ⇒ 按"占着"处理（不假装空着）。
 */
function portFree(port) {
  if (process.platform === 'win32') {
    const out = spawnSync('netstat', ['-ano'], { encoding: 'utf8', windowsHide: true })
    return !String(out.stdout ?? '')
      .split('\n')
      .some((line) => line.includes('LISTENING') && line.includes(`:${port} `))
  }
  const out = spawnSync('ss', ['-ltn'], { encoding: 'utf8' })
  if (out.error || typeof out.stdout !== 'string') return false
  return !out.stdout
    .split('\n')
    .some((line) => (line.trim().split(/\s+/)[3] ?? '').endsWith(`:${port}`))
}

worldCase('真进程：端口上的世界证明不了是本家的 ⇒ `node home.js down` **不杀**它，并如实报出来', async (t) => {
  const layout = P.detectLayout()
  if (!P.isFile(layout.imExe)) {
    // 离线套件不强制先构建：没有可执行件就如实跳过（不许假装验过）。
    console.log('      （跳过：还没有构建 celira-im —— 先 cargo build --release）')
    return
  }
  const imPort = 41517
  const clientPort = 45173
  const statusPort = 45190
  const configPort = 45191
  if (!portFree(imPort)) {
    console.log(`      （跳过：端口 ${imPort} 上已经有东西在跑）`)
    return
  }

  const homeDir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-world-down-'))
  fs.mkdirSync(path.join(homeDir, 'im', 'media'), { recursive: true })
  fs.writeFileSync(
    path.join(homeDir, 'home.conf'),
    `listen_addr=127.0.0.1:${imPort}\nclient_port=${clientPort}\nstatus_port=${statusPort}\nconfig_port=${configPort}\n`,
    'utf8',
  )
  const world = spawn(layout.imExe, [], {
    env: {
      ...process.env,
      CELIRA_LISTEN_ADDR: `127.0.0.1:${imPort}`,
      CELIRA_DB_PATH: path.join(homeDir, 'im', 'core.db'),
      CELIRA_MEDIA_ROOT: path.join(homeDir, 'im', 'media'),
    },
    stdio: 'ignore',
    windowsHide: true,
  })
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
  try {
    let up = false
    for (let attempt = 0; attempt < 30 && !up; attempt += 1) {
      try {
        const answer = await fetch(`http://127.0.0.1:${imPort}/health`)
        up = answer.ok
      } catch {
        await sleep(500)
      }
    }
    assert.ok(up, '那个世界应当先起来（否则这条用例什么也没验）')

    // 本家：**没有 agent_id、没有凭据** ⇒ 证明不了（unknown）⇒ 这一支不许被收掉。
    const done = spawnSync(process.execPath, [path.join(P.REPO, 'home.js'), 'down'], {
      encoding: 'utf8',
      windowsHide: true,
      timeout: 60000,
      env: {
        SystemRoot: process.env.SystemRoot,
        windir: process.env.windir,
        PATH: process.env.PATH,
        CELIRA_HOME_DIR: homeDir,
      },
    })
    const text = `${done.stdout ?? ''}${done.stderr ?? ''}`
    assert.match(text, /证明不了是本家的/, `读数要说清为什么不杀：\n${text}`)
    assert.equal(done.status, 4, '端口还占着 ⇒ "只起来了一部分"（如实，不假装全停了）')
    assert.ok(world.exitCode === null, '那个世界**必须还活着**（这条就是本刀要钉住的跨家误杀）')
  } finally {
    world.kill()
    await sleep(300)
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

worldCase('真进程：`--home` 真的进得了进程表（那是这一维存在的**唯一**理由）', async (t) => {
  const homeDir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-homeflag-'))
  const port = 45190
  if (!portFree(port)) {
    console.log(`      （跳过：端口 ${port} 上已经有东西在跑）`)
    fs.rmSync(homeDir, { recursive: true, force: true })
    return
  }
  fs.writeFileSync(
    path.join(homeDir, 'home.conf'),
    `listen_addr=127.0.0.1:45180\nstatus_port=${port}\n`,
    'utf8',
  )
  // 2026-10-01 第六刀：两个门合成"一个进程、两个端面" ⇒ 起门那条路是 `home.js doors`。
  const door = spawn(
    process.execPath,
    [path.join(P.REPO, 'home.js'), 'doors', '--home', homeDir],
    { stdio: 'ignore', windowsHide: true, env: { ...process.env, CELIRA_HOME_DIR: homeDir } },
  )
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
  try {
    let answered = false
    for (let attempt = 0; attempt < 20 && !answered; attempt += 1) {
      try {
        answered = (await fetch(`http://127.0.0.1:${port}/healthz`)).ok
      } catch {
        await sleep(300)
      }
    }
    assert.ok(answered, '读端面应当起来（否则这条用例什么也没验）')

    const layout = P.detectLayout()
    const info = require('../proc').describeProcess(door.pid)
    assert.ok(info, '进程表里应当读得到这支')
    assert.match(info.cmdline, /--home/, `命令行里应当有 --home：${info.cmdline}`)
    // 判据两向：本家认它；**另一家**（哪怕端口一样）不认它 —— 这就是同包多家分得开的原因。
    // 家的门：家事实是**一组端口**（读口 ＋ 写口，听其中任一个就算本家的）。
    const mine = { homeDir, envDir: path.join(homeDir, 'env'), clientPort: '5173', imPort: '45180', statusPort: String(port), configPort: '5191', doorsPorts: [String(port), '5191'], listening: new Map([[door.pid, new Set([port])]]) }
    const theirs = { ...mine, homeDir: path.join(homeDir, 'other'), listening: mine.listening }
    assert.ok(require('../proc').ownsPid('doors', info, layout, mine), '本家应当认领')
    assert.ok(!require('../proc').ownsPid('doors', info, layout, theirs), '别家不许认领（同端口也不行）')
    // 旧形状那两条指纹**还在**（迁移桥）：老家还跑着旧门时要认得出、收得掉。
    assert.ok(require('../proc').ownsPid('status', info, layout, mine) === false, '新形状不是旧的 status 支')
  } finally {
    door.kill()
    await sleep(300)
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

worldCase('真进程：`node home.js status` 认得出带 `--home` 的常驻件（**不许炸**）', async (t) => {
  const homeDir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-status-claim-'))
  const port = 45290
  if (!portFree(port)) {
    console.log(`      （跳过：端口 ${port} 上已经有东西在跑）`)
    fs.rmSync(homeDir, { recursive: true, force: true })
    return
  }
  fs.writeFileSync(
    path.join(homeDir, 'home.conf'),
    [
      'listen_addr=127.0.0.1:45280',
      'client_port=45273',
      `status_port=${port}`,
      'config_port=45291',
      'agent_id=agent-probe',
      'human_id=human-probe',
      'room_id=room-probe',
      '',
    ].join('\n'),
    'utf8',
  )
  const door = spawn(
    process.execPath,
    [path.join(P.REPO, 'home.js'), 'doors', '--home', homeDir],
    { stdio: 'ignore', windowsHide: true, env: { ...process.env, CELIRA_HOME_DIR: homeDir } },
  )
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
  try {
    let answered = false
    for (let attempt = 0; attempt < 20 && !answered; attempt += 1) {
      try {
        answered = (await fetch(`http://127.0.0.1:${port}/healthz`)).ok
      } catch {
        await sleep(300)
      }
    }
    assert.ok(answered, '状态口应当起来')
    // 这一条是**真踩过的回归**：`status.js` 的家事实少了 `homeDir` ⇒ 一遇到带 `--home` 的
    // 常驻件就抛（"家事实缺 homeDir"）。判据：它得认领，且不许有栈。
    const done = spawnSync(process.execPath, [path.join(P.REPO, 'home.js'), 'status'], {
      encoding: 'utf8',
      windowsHide: true,
      timeout: 60000,
      env: {
        SystemRoot: process.env.SystemRoot,
        windir: process.env.windir,
        PATH: process.env.PATH,
        CELIRA_HOME_DIR: homeDir,
      },
    })
    const out = `${done.stdout ?? ''}${done.stderr ?? ''}`
    assert.match(out, /\[status\] 在跑/, `应当认领那个口：\n${out}`)
    assert.doesNotMatch(out, /家事实缺|TypeError|at Object\./, `不许抛：\n${out}`)
  } finally {
    door.kill()
    await sleep(300)
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '本文件一条用例都没跑（空跑不算过）')
  assert.ok(cases >= 12, `用例数 ${cases} 少于声明的 12`)
})
