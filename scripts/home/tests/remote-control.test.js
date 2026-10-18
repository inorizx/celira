'use strict'
/**
 * ★ **远程接上一支 Agent**（2026-10-15 主人裁的那一刀）。
 *
 * 这一组守的是**四条边界**（每一条都有一个反例能把它打红）：
 *
 * 1. **准入 ≠ 授权**：家钥匙对、但 `home.conf` 里没开 `remote_control` ⇒ **拒**（`remote_control_off`），
 *    而且**那只手一次都不许跑**（"先动手再拒"是这一条最容易犯的错）；
 * 2. **谁付钱**：这一步**永不选模型** —— 没模型 ⇒ 接上但先不起它（`homed-not-started`），
 *    绝不替谁绑一条；
 * 3. **谁判起不起来**：结局从**那台机器自己的账本**读出来（住户登记在不在、宿主在不在跑），
 *    不从人话文本猜 —— 四种结局各一条；
 * 4. **留痕**：每次都在那台机器上写一行（谁 / 什么时候 / 从哪儿 / 接了哪一支 / 结局），
 *    而**钥匙一个字节都不进它**。
 *
 * 外加两条同形判据：那只手跑的是**既有的操作路径**（`up --agent … --attach-only`），
 * 结局词表与**壳那份逐字相同**。
 *
 * 运行： node --test "scripts/home/tests/*.test.js"
 */

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const remoteControl = require('../remote-control')
const modelSources = require('../model-sources')
const { createConfigServer, CONTROL_PATH, ERRORS } = require('../config-server')

let cases = 0
function remoteCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

function scratch(label) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `celira-remote-control-${label}-`))
}

/** 一个"已经接过一支"的家：环境根 + 住户登记 + 可选绑定。 */
function homeWithResident({ agentId = 'a-1', homed = true, binding = null } = {}) {
  const home = scratch('home')
  fs.mkdirSync(path.join(home, 'logs'), { recursive: true })
  const envRoot = path.join(home, 'agents', agentId)
  fs.mkdirSync(envRoot, { recursive: true })
  if (homed) {
    fs.writeFileSync(path.join(envRoot, 'identity.json'), JSON.stringify({ agent: agentId }))
    fs.writeFileSync(
      path.join(envRoot, 'agent.conf'),
      ['agent_id=' + agentId, 'slot=1', 'room_id=', 'display_name='].join('\n') + '\n',
    )
  }
  if (binding) modelSources.writeBinding(envRoot, binding)
  fs.writeFileSync(path.join(home, 'home.conf'), `home_id=test-home\nhuman_id=h-1\n`)
  return { home, envRoot, agentId }
}

/**
 * 一只**替身的手**：它只改"那台机器的事实"（住户登记 / 宿主账本），不真起进程。
 *
 * 为什么要这样测：这一层的判据是**结局怎么从事实里读出来**，而"真起一个宿主"是那条既有
 * 操作路径自己的事（它有自己的用例）。替身让四种结局都能被机械地摆出来。
 */
function fakeHand({ homed = true, running = false, pid = 4242 } = {}) {
  const calls = []
  const runner = async ({ homeDir, agentId }) => {
    calls.push({ homeDir, agentId })
    const envRoot = path.join(homeDir, 'agents', agentId)
    if (homed) {
      fs.mkdirSync(envRoot, { recursive: true })
      fs.writeFileSync(
        path.join(envRoot, 'agent.conf'),
        ['agent_id=' + agentId, 'slot=2', 'room_id=', 'display_name='].join('\n') + '\n',
      )
    }
    if (running) {
      const pidFile = path.join(homeDir, 'logs', 'agents', agentId, 'host.pid')
      fs.mkdirSync(path.dirname(pidFile), { recursive: true })
      fs.writeFileSync(pidFile, `${pid}\n`)
    }
    return { code: homed ? 0 : 4, stdout: '[home_up] 替身跑完了\n', stderr: '', timedOut: false }
  }
  return { runner, calls }
}

/** 判"宿主在不在跑"的替身（这一层只问那两把尺子；尺子本身有它自己的用例）。 */
function fakeProc({ runningPids = [], orphans = [] } = {}) {
  return {
    trustedLedgerPid: () => (runningPids.length > 0 ? runningPids[0] : null),
    hostOrphanPids: () => orphans,
  }
}

const deps = (over = {}) => ({
  detectLayout: () => ({ kind: 'source', hostExe: '/nowhere/agent_host' }),
  residents: require('../residents'),
  proc: fakeProc(over.proc ?? {}),
})

// ── 一、开关：缺省关，且只认 `on`（fail-closed）────────────────────────────────

remoteCase('开关：缺省**不许**（家钥匙到手 ≠ 允许起进程）；只认 `on`，别的值一律当没开', () => {
  const { home } = homeWithResident()
  assert.equal(remoteControl.remoteControlEnabled(home), false, '缺省必须是关的')
  // 手改文件成别的值 ⇒ 仍然算关（fail-closed：不是"非 off 即 on"）。
  fs.writeFileSync(path.join(home, 'home.conf'), 'home_id=test-home\nremote_control=yes\n')
  assert.equal(remoteControl.remoteControlEnabled(home), false, '只认 on')
  assert.equal(remoteControl.setRemoteControl(home, true), true)
  assert.equal(remoteControl.remoteControlEnabled(home), true)
  assert.equal(remoteControl.setRemoteControl(home, false), false)
  fs.rmSync(home, { recursive: true, force: true })
})

remoteCase('★ 开关关着 ⇒ **拒，而且那只手一次都不许跑**（不许"先动手再拒"）', async () => {
  const { home, agentId } = homeWithResident({ homed: false })
  const hand = fakeHand()
  const result = await remoteControl.attachRemote({
    homeDir: home,
    agentId,
    runner: hand.runner,
    deps: deps(),
  })
  assert.equal(result.ok, false)
  assert.equal(result.error, ERRORS.REMOTE_CONTROL_OFF)
  assert.equal(hand.calls.length, 0, '拒的时候不许已经动过手')
  assert.equal(fs.existsSync(path.join(home, 'agents', agentId, 'agent.conf')), false, '什么都没动')
  fs.rmSync(home, { recursive: true, force: true })
})

// ── 二、结局：从**那台机器自己的账本**读出来（四种各一条）─────────────────────

remoteCase('★ 结局：接上了且在跑 ⇒ `homed`（这一次真的把它带起来了）', async () => {
  const { home, agentId } = homeWithResident({ homed: false })
  remoteControl.setRemoteControl(home, true)
  const hand = fakeHand({ homed: true, running: true })
  const result = await remoteControl.attachRemote({
    homeDir: home,
    agentId,
    runner: hand.runner,
    deps: deps({ proc: { runningPids: [777] } }),
  })
  assert.equal(result.ok, true, JSON.stringify(result))
  assert.equal(result.outcome, 'homed')
  assert.equal(result.pid, 777)
  assert.equal(hand.calls.length, 1)
  assert.equal(hand.calls[0].agentId, agentId)
  fs.rmSync(home, { recursive: true, force: true })
})

remoteCase('★ 结局：本来就在跑 ⇒ `already-homed`（什么都没动）', async () => {
  const { home, agentId } = homeWithResident({ homed: true })
  remoteControl.setRemoteControl(home, true)
  const hand = fakeHand({ homed: true, running: true })
  const result = await remoteControl.attachRemote({
    homeDir: home,
    agentId,
    runner: hand.runner,
    deps: deps({ proc: { runningPids: [777] } }),
  })
  assert.equal(result.outcome, 'already-homed', JSON.stringify(result))
  fs.rmSync(home, { recursive: true, force: true })
})

remoteCase('★ 结局：接上了、但没起它（没模型）⇒ `homed-not-started`（**不是失败**）', async () => {
  const { home, agentId } = homeWithResident({ homed: false })
  remoteControl.setRemoteControl(home, true)
  const hand = fakeHand({ homed: true, running: false })
  const result = await remoteControl.attachRemote({
    homeDir: home,
    agentId,
    runner: hand.runner,
    deps: deps({ proc: {} }),
  })
  assert.equal(result.ok, true, '接上这件事**做成了**')
  assert.equal(result.outcome, 'homed-not-started')
  assert.equal(result.pid, null)
  assert.ok(
    result.lines.some((line) => line.includes('替身跑完了')),
    '原样读数要带出去（它是"为什么没起"的唯一来路）',
  )
  // **不许偷偷绑**：这一层一个字节的绑定都不写。
  assert.equal(
    fs.existsSync(path.join(home, 'agents', agentId, 'model_binding.json')),
    false,
    '这一步永不选模型',
  )
  fs.rmSync(home, { recursive: true, force: true })
})

remoteCase('★ 结局：没接上 ⇒ `failed`（＋封闭码 `attach_failed`，读数原样带出去）', async () => {
  const { home, agentId } = homeWithResident({ homed: false })
  remoteControl.setRemoteControl(home, true)
  const hand = fakeHand({ homed: false })
  const result = await remoteControl.attachRemote({
    homeDir: home,
    agentId,
    runner: hand.runner,
    deps: deps(),
  })
  assert.equal(result.ok, false)
  assert.equal(result.error, ERRORS.ATTACH_FAILED)
  assert.equal(result.outcome, 'failed')
  fs.rmSync(home, { recursive: true, force: true })
})

// ── 三、留痕：一行，且**钥匙不进它**────────────────────────────────────────────

remoteCase('★ 留痕：每次写一行（谁/何时/从哪/哪一支/结局），**钥匙一个字节都不进它**', async () => {
  const { home, agentId } = homeWithResident({ homed: false })
  remoteControl.setRemoteControl(home, true)
  const hand = fakeHand({ homed: true, running: true })
  const canary = 'hk-canary-2f9c1d' // 家钥匙原文（这条路上根本不该出现它）
  const result = await remoteControl.attachRemote({
    homeDir: home,
    agentId,
    from: 'https://192.168.1.20',
    runner: hand.runner,
    clock: () => '2026-10-15T02:00:00.000Z',
    deps: deps({ proc: { runningPids: [777] } }),
  })
  assert.equal(result.outcome, 'homed')
  const file = path.join(home, 'logs', remoteControl.AUDIT_FILE)
  assert.equal(result.auditFile, file)
  const text = fs.readFileSync(file, 'utf8')
  const rows = text.trim().split('\n').map((line) => JSON.parse(line))
  assert.equal(rows.length, 1)
  assert.deepEqual(rows[0], {
    at: '2026-10-15T02:00:00.000Z',
    from: 'https://192.168.1.20',
    agentId,
    outcome: 'homed',
  })
  assert.ok(!text.includes(canary), `留痕里不许有钥匙：${text}`)
  fs.rmSync(home, { recursive: true, force: true })
})

// ── 四、那只手跑的是**既有的操作路径**（不新写第二只手）──────────────────────

remoteCase('★ 那只手 = 既有的操作路径：`up --agent <id> --attach-only`（参数形状可判）', () => {
  const args = remoteControl.upArgsFor('agent-9')
  assert.deepEqual(args, ['up', '--agent', 'agent-9', '--attach-only'])
  // 判据不是"有这两个词"，而是**它就是操作者在终端里敲的那条命令**：
  // 少 `--attach-only` ⇒ 没模型时会早失败（接不上）；少 `--agent` ⇒ 会去动**整个家**。
  assert.equal(args[0], 'up')
  assert.ok(args.includes('--agent') && args.includes('--attach-only'))
})

// ── 五、门那条路：形状 / 分流 / 封闭码 ────────────────────────────────────────

async function withDoor(homeDir, ctx = {}) {
  const server = createConfigServer({ homeDir, residents: () => [], ...ctx })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const port = server.address().port
  const call = async (method, target, body) => {
    const res = await fetch(`http://127.0.0.1:${port}${target}`, {
      method,
      headers: { 'content-type': 'application/json', origin: 'http://127.0.0.1:5190' },
      body: body === undefined ? undefined : JSON.stringify(body),
    })
    const text = await res.text()
    let json = null
    try {
      json = JSON.parse(text)
    } catch {
      json = null
    }
    return { status: res.status, json, text, allow: res.headers.get('allow') }
  }
  return { port, call, close: () => new Promise((resolve) => server.close(resolve)) }
}

remoteCase('★ 门：开关关着 ⇒ 403 `remote_control_off`（**不是 404** —— 不装成不存在）', async () => {
  const { home, agentId } = homeWithResident({ homed: false })
  const door = await withDoor(home, { attach: remoteControl.attachRemote })
  try {
    const res = await door.call('POST', CONTROL_PATH, { agentId })
    assert.equal(res.status, 403, res.text)
    assert.equal(res.json.error, ERRORS.REMOTE_CONTROL_OFF)
    assert.equal(fs.existsSync(path.join(home, 'agents', agentId, 'agent.conf')), false)
  } finally {
    await door.close()
    fs.rmSync(home, { recursive: true, force: true })
  }
})

remoteCase('★ 门：id 形状不合法 ⇒ 400 `bad_agent_id`（它会变成目录名，绝不拼路径）', async () => {
  const { home } = homeWithResident({ homed: false })
  remoteControl.setRemoteControl(home, true)
  let called = 0
  const door = await withDoor(home, {
    attach: async () => {
      called += 1
      return { ok: true, outcome: 'homed', agentId: 'x', envRoot: '/x', pid: 1, lines: [] }
    },
  })
  try {
    for (const bad of ['', '   ', '../evil', 'a/b', 'a b', 'a:b']) {
      const res = await door.call('POST', CONTROL_PATH, { agentId: bad })
      assert.equal(res.status, 400, `${bad} 该被拒：${res.text}`)
      assert.equal(res.json.error, ERRORS.BAD_AGENT_ID)
    }
    assert.equal(called, 0, '形状不合法时那只手一次都不许跑')
    assert.equal(fs.existsSync(path.join(home, 'evil')), false)
  } finally {
    await door.close()
    fs.rmSync(home, { recursive: true, force: true })
  }
})

remoteCase('★ 门：接上了 ⇒ 200 带结局与原样读数；**响应里没有钥匙**', async () => {
  const { home } = homeWithResident({ homed: false })
  remoteControl.setRemoteControl(home, true)
  const canary = 'sk-model-key-canary-77aa'
  const door = await withDoor(home, {
    attach: async ({ homeDir, agentId, from }) => ({
      ok: true,
      outcome: 'homed-not-started',
      agentId,
      envRoot: path.join(homeDir, 'agents', agentId),
      pid: null,
      lines: [`[model] 接上了，先没起它`, `（from=${from}）`],
    }),
  })
  try {
    const res = await door.call('POST', CONTROL_PATH, { agentId: 'a-1' })
    assert.equal(res.status, 200, res.text)
    assert.equal(res.json.outcome, 'homed-not-started')
    assert.equal(res.json.agentId, 'a-1')
    assert.equal(res.json.pid, null)
    assert.equal(res.json.lines.length, 2, '原样读数一行都不吞')
    assert.ok(!res.text.includes(canary), '响应里不许出现任何钥匙')
    // **结局词必须是封闭表里那几个**（页面按它分流；串了就是把拒绝说成成功）。
    assert.ok(remoteControl.ATTACH_OUTCOMES.includes(res.json.outcome))
  } finally {
    await door.close()
    fs.rmSync(home, { recursive: true, force: true })
  }
})

remoteCase('★ 门：那只手说没成 ⇒ 500 `attach_failed`（读数照样带出去）', async () => {
  const { home } = homeWithResident({ homed: false })
  remoteControl.setRemoteControl(home, true)
  const door = await withDoor(home, {
    attach: async () => ({ ok: false, error: ERRORS.ATTACH_FAILED, lines: ['[home_up] 缺凭据'] }),
  })
  try {
    const res = await door.call('POST', CONTROL_PATH, { agentId: 'a-1' })
    assert.equal(res.status, 500)
    assert.equal(res.json.error, ERRORS.ATTACH_FAILED)
    assert.deepEqual(res.json.lines, ['[home_up] 缺凭据'], '失败时读数更要带出去')
  } finally {
    await door.close()
    fs.rmSync(home, { recursive: true, force: true })
  }
})

remoteCase('门：这条路只收 POST（id 走请求体，**绝不进 URL** —— URL 会进日志、进历史）', async () => {
  const { home } = homeWithResident({ homed: false })
  const door = await withDoor(home)
  try {
    // 这条路径认的那四条动词里，除 POST 之外的三条 ⇒ 405 + `Allow: POST`。
    for (const method of ['GET', 'PUT', 'DELETE']) {
      const res = await door.call(method, CONTROL_PATH, undefined)
      assert.equal(res.status, 405, `${method} 该是 405`)
      assert.equal(res.allow, 'POST')
    }
    // 四条之外的动词走这一层既有那条判据（`Allow` 如实列出这一层认的四条）。
    const patch = await door.call('PATCH', CONTROL_PATH, undefined)
    assert.equal(patch.status, 405)
    assert.equal(patch.allow, 'GET, POST, PUT, DELETE')
  } finally {
    await door.close()
    fs.rmSync(home, { recursive: true, force: true })
  }
})

// ── 六、同形：结局词与壳那份**逐字相同** ──────────────────────────────────────

remoteCase('★ 词表同形：结局词在壳的源码里逐字出现（`AnjiaOutcome::kind()` 那一份）', () => {
  const source = fs.readFileSync(
    path.join(__dirname, '..', '..', '..', 'app', 'src-tauri', 'src', 'anjia', 'mod.rs'),
    'utf8',
  )
  for (const word of remoteControl.ATTACH_OUTCOMES) {
    assert.ok(source.includes(`"${word}"`), `壳侧源码里找不到这个结局词：${word}`)
  }
  assert.equal(remoteControl.ATTACH_OUTCOMES.length, 4, '四种结局，一个都不许少（也不许多）')
})

// ── 七、真进程那一条：门自己起、自己答（**自选端口**，绝不碰默认那两颗）──────────────

/** 一个刚放掉的端口（几乎必然没人听）——用它把这条真机判据与默认那两颗门隔开。 */
async function freePort() {
  const net = require('node:net')
  const probe = net.createServer()
  await new Promise((resolve) => probe.listen(0, '127.0.0.1', resolve))
  const port = probe.address().port
  await new Promise((resolve) => probe.close(resolve))
  return port
}

remoteCase('★ 真进程：门起在**自选端口**上，开关关着 ⇒ 403；开了 ⇒ 形状错 400（都不动手）', async () => {
  const { home } = homeWithResident({ homed: false })
  const statusPort = await freePort()
  const configPort = await freePort()
  fs.writeFileSync(
    path.join(home, 'home.conf'),
    `home_id=test-home\nhuman_id=h-1\nstatus_port=${statusPort}\nconfig_port=${configPort}\n`,
  )
  const { spawn } = require('node:child_process')
  const child = spawn(
    process.execPath,
    [path.join(__dirname, '..', '..', '..', 'home.js'), 'doors', '--home', home],
    { cwd: home, windowsHide: true, env: { ...process.env, NO_PROXY: '127.0.0.1,localhost' } },
  )
  const call = async (body) => {
    const res = await fetch(`http://127.0.0.1:${configPort}${CONTROL_PATH}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', origin: 'http://127.0.0.1:5190' },
      body: JSON.stringify(body),
    })
    return { status: res.status, json: await res.json().catch(() => null) }
  }
  try {
    // 等门开口（最多 15 秒）。
    let up = false
    for (let i = 0; i < 60 && !up; i += 1) {
      try {
        await fetch(`http://127.0.0.1:${configPort}/home-config`)
        up = true
      } catch {
        await new Promise((resolve) => setTimeout(resolve, 250))
      }
    }
    assert.ok(up, '门没在 15 秒内开口（自选端口）')
    const off = await call({ agentId: 'a-1' })
    assert.equal(off.status, 403, JSON.stringify(off))
    assert.equal(off.json.error, ERRORS.REMOTE_CONTROL_OFF)
    // 在那台机器上开开关（**本地命令**）⇒ 同一颗门**不用重启**就该认（每请求现读）。
    remoteControl.setRemoteControl(home, true)
    const bad = await call({ agentId: '../evil' })
    assert.equal(bad.status, 400, JSON.stringify(bad))
    assert.equal(bad.json.error, ERRORS.BAD_AGENT_ID)
    assert.equal(fs.existsSync(path.join(home, 'evil')), false, '不许拼出 home 之外的路径')
    assert.equal(
      fs.existsSync(path.join(home, 'agents', 'a-1', 'agent.conf')),
      false,
      '这两笔都不许动过手',
    )
  } finally {
    child.kill()
    // ⚠ **等它真的走**：那个门进程的 cwd 就是这个家 ⇒ 它还在时 Windows 上删不掉那个目录
    // （实测 `EBUSY: resource busy or locked`）—— 收尾要等，不是"发了信号就当它走了"。
    await new Promise((resolve) => {
      if (child.exitCode !== null || child.signalCode !== null) return resolve()
      child.once('close', resolve)
      setTimeout(resolve, 5000)
    })
    fs.rmSync(home, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 })
  }
})

// ── 八、读数那一格：门如实报"这台机器允不允许"（三态不许压成两态）────────────────────

remoteCase('★ 读数：门那一格缺省报 `off`、开了报 `on`（**读的那一刻现问**，不用重启门）', async () => {
  const { home } = homeWithResident({ homed: false })
  const door = await withDoor(home)
  try {
    const before = await door.call('GET', '/home-config', undefined)
    assert.equal(before.status, 200, before.text)
    assert.equal(before.json.remoteControl, 'off', '缺省必须是 off（fail-closed）')
    remoteControl.setRemoteControl(home, true)
    const after = await door.call('GET', '/home-config', undefined)
    assert.equal(after.json.remoteControl, 'on', '开了就该报 on —— 且不用重启门')
  } finally {
    await door.close()
    fs.rmSync(home, { recursive: true, force: true })
  }
})

/**
 * ★ **同形：壳那一侧必须报 `null`（不提供这条路），不许报 `off`**。
 *
 * 判据（三态不许压成两态）：`off` 是"这台有那条路、只是没开"（用户跑一条命令就能开）；
 * `null` 是"这条路**在这台机器上不存在**"（去开开关也没用）。壳自己端的两颗口只绑 loopback
 * ⇒ 它只能报 `null`。报成 `off` 就是**指一条走不通的路**（人会照着去开一个不存在的开关）。
 */
remoteCase('★ 同形：壳那份门报 `null`（不提供），源码这份门报 `on`/`off` —— 两态不许混', () => {
  const shell = fs.readFileSync(
    path.join(__dirname, '..', '..', '..', 'app', 'src-tauri', 'src', 'home_doors.rs'),
    'utf8',
  )
  assert.ok(
    /"remoteControl":\s*Value::Null/.test(shell),
    '壳那份门必须如实报 null（它不提供远程接上这一条）',
  )
  assert.ok(
    !/"remoteControl":\s*json!\(\s*"(on|off)"/.test(shell),
    '壳那份门不许报 on/off —— 那是指一条走不通的路',
  )
})

test('（用例计数）', () => {
  assert.ok(cases > 0, '远程接上这一组必须有用例（空跑不算通过）')
  console.log(`      remote-control 用例数：${cases}`)
})
