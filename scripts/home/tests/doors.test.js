'use strict'
// **家的门**（2026-10-01 第六刀）：只读状态口（读端面）＋ 模型配置门（写端面），
// **一个进程、两个端面**。
//
// 这一层钉住的是"合并进程**不许**把端面也合掉"这件事，分四层：
//  1. **规划**：两个端面各读各的 `home.conf` 键（`status_port` / `config_port`，绑定键同款），
//     缺省值各是 5190 / 5191 —— 一个端面读不懂**不许**连坐另一个；
//  2. **准入各判各的**：非回环绑定 + 缺钥匙 ⇒ 那个端面 fail-closed（另一个仍可用）；
//  3. **归属判据**：`proc.ownsPid('doors', …)` 看的是"听没听这两个口中的任一个"，
//     而旧形状（`home.js status --serve` / `home.js config --serve`）**仍认得出**（迁移桥）；
//  4. **真进程**：一个 pid 绑两个口；`/home-config` 打在读口上是 404、`/status` 打在写口上是 404
//     —— 同一个进程**不是**"读口顺带能写"的理由。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { spawn, spawnSync } = require('node:child_process')

const P = require('../paths')
const proc = require('../proc')
const doors = require('../doors')
const homeKey = require('../home-key')
const modelSources = require('../model-sources')

let cases = 0
function doorsCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

function scratch(label) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `celira-doors-${label}-`))
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

async function portFree(port) {
  try {
    const res = await fetch(`http://127.0.0.1:${port}/healthz`)
    return !res.ok && res.status === 404 ? false : false
  } catch {
    return true
  }
}

doorsCase('规划：两个端面各读各的键（缺省 5190 / 5191，绑定缺省回环）', () => {
  const homeDir = scratch('plan')
  try {
    fs.writeFileSync(path.join(homeDir, 'home.conf'), 'status_port=5301\n', 'utf8')
    const plan = doors.planEndfaces({ homeDir })
    const status = plan.endfaces.find((e) => e.kind === 'status')
    const config = plan.endfaces.find((e) => e.kind === 'config')
    assert.equal(status.port, 5301)
    assert.equal(status.bind, homeKey.DEFAULT_BIND)
    assert.equal(status.launch.ok, true)
    assert.equal(status.launch.access.mode, 'local', '回环 ⇒ 模式一')
    assert.equal(config.port, Number(P.DEFAULT_CONFIG_PORT), '没写就是家的默认口')
    assert.equal(config.error, undefined)
  } finally {
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

doorsCase('规划：一个端面的端口读不懂 ⇒ **只坏它一个**（另一个照旧可用）', () => {
  const homeDir = scratch('badport')
  try {
    fs.writeFileSync(path.join(homeDir, 'home.conf'), 'status_port=5190\nconfig_port=not-a-port\n', 'utf8')
    const plan = doors.planEndfaces({ homeDir })
    const status = plan.endfaces.find((e) => e.kind === 'status')
    const config = plan.endfaces.find((e) => e.kind === 'config')
    assert.equal(status.port, 5190, '读端面不受影响')
    assert.equal(status.error, undefined)
    assert.match(config.error ?? '', /config_port/)
  } finally {
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

doorsCase('准入各判各的：写端面绑非本机 + 缺钥匙 ⇒ 只它 fail-closed', () => {
  const homeDir = scratch('access')
  try {
    fs.writeFileSync(path.join(homeDir, 'home.conf'), 'status_port=5190\nconfig_port=5191\nconfig_bind=0.0.0.0\n', 'utf8')
    const plan = doors.planEndfaces({ homeDir })
    const status = plan.endfaces.find((e) => e.kind === 'status')
    const config = plan.endfaces.find((e) => e.kind === 'config')
    assert.equal(status.launch.ok, true, '读端面还是模式一')
    assert.equal(config.launch.ok, false, '写端面缺钥匙 ⇒ 开口之前就拒')
    assert.match(config.launch.message, /issue-home-key|钥匙/)
  } finally {
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

doorsCase('归属：听读口或写口**任一个**都算本家的门；两个口都在听也算', () => {
  const layout = P.detectLayout()
  const info = { pid: 4321, name: 'node.exe', cmdline: 'node /data/celira/home.js doors --home /home/me' }
  const base = {
    homeDir: '/home/me',
    clientPort: '5173',
    imPort: '8080',
    statusPort: '5190',
    configPort: '5191',
    doorsPorts: ['5190', '5191'],
  }
  const onStatus = { ...base, listening: new Map([[4321, new Set([5190])]]) }
  const onConfig = { ...base, listening: new Map([[4321, new Set([5191])]]) }
  const onBoth = { ...base, listening: new Map([[4321, new Set([5190, 5191])]]) }
  const elsewhere = { ...base, listening: new Map([[4321, new Set([9999])]]) }
  assert.ok(proc.ownsPid('doors', info, layout, onStatus), '听读口 = 本家的')
  assert.ok(proc.ownsPid('doors', info, layout, onConfig), '听写口 = 本家的')
  assert.ok(proc.ownsPid('doors', info, layout, onBoth), '两个口都在听 = 本家的')
  assert.ok(!proc.ownsPid('doors', info, layout, elsewhere), '别的口 ⇒ 不是本家的')
  // 别家（同一份包、同样的口）不许认领：第二维是 `--home`。
  assert.ok(!proc.ownsPid('doors', info, layout, { ...onBoth, homeDir: '/home/other' }), '别家不许认领')
})

doorsCase('归属：旧形状那两条指纹**还在**（迁移桥：老家还跑着旧门时要认得出、收得掉）', () => {
  const layout = P.detectLayout()
  const home = {
    homeDir: '/home/me',
    clientPort: '5173',
    imPort: '8080',
    statusPort: '5190',
    configPort: '5191',
    doorsPorts: ['5190', '5191'],
    listening: new Map([
      [111, new Set([5190])],
      [222, new Set([5191])],
    ]),
  }
  const oldStatus = { pid: 111, name: 'node.exe', cmdline: 'node /data/celira/home.js status --serve --home /home/me' }
  const oldConfig = { pid: 222, name: 'node.exe', cmdline: 'node /data/celira/home.js config --serve --home /home/me' }
  assert.ok(proc.ownsPid('status', oldStatus, layout, home), '旧状态口仍认得出')
  assert.ok(proc.ownsPid('config', oldConfig, layout, home), '旧配置门仍认得出')
  // 新形状**不许**被旧指纹认领（反过来也一样）：认错进程会去杀错人。
  const newDoors = { pid: 333, name: 'node.exe', cmdline: 'node /data/celira/home.js doors --home /home/me' }
  const listening = { ...home, listening: new Map([[333, new Set([5190, 5191])]]) }
  assert.ok(!proc.ownsPid('status', newDoors, layout, listening), '新的门不是旧的 status 支')
  assert.ok(!proc.ownsPid('config', newDoors, layout, listening), '新的门不是旧的 config 支')
  assert.ok(proc.ownsPid('doors', newDoors, layout, listening), '新的门就是 doors 支')
})

doorsCase('退役：`status --serve` / `config --serve` 如实说退役 + 指路（退出码 1，不是静默）', () => {
  const homeDir = scratch('retired')
  try {
    for (const sub of ['status', 'config']) {
      const done = spawnSync(process.execPath, [path.join(P.REPO, 'home.js'), sub, '--serve'], {
        encoding: 'utf8',
        windowsHide: true,
        timeout: 20000,
        env: { ...process.env, CELIRA_HOME_DIR: homeDir },
      })
      assert.equal(done.status, 1, `${sub} --serve 应当以用法错退出`)
      assert.match(done.stdout, /已退役/, '要说清它退役了')
      assert.match(done.stdout, /node home\.js doors/, '要给出去处')
    }
  } finally {
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

doorsCase('真进程：一个 pid 绑两个口；两个端面**不串门**（读口上没有写路由，写口上没有读路由）', async () => {
  const homeDir = scratch('live')
  const statusPort = 45390
  const configPort = 45391
  if (!(await portFree(statusPort)) || !(await portFree(configPort))) {
    console.log(`      （跳过：端口 ${statusPort}/${configPort} 上已经有东西在跑）`)
    fs.rmSync(homeDir, { recursive: true, force: true })
    return
  }
  // 一个"布置过"的最小家（`status` 见到**一个住户**才会往下走读门那两行）：
  // 老家形状的那一支（`home.conf` 里的 `agent_id` / `room_id`）就够 —— 本用例不看 Agent，只看门。
  fs.writeFileSync(
    path.join(homeDir, 'home.conf'),
    [
      `status_port=${statusPort}`,
      `config_port=${configPort}`,
      'agent_id=agent-probe',
      'human_id=human-probe',
      'room_id=room-probe',
      '',
    ].join('\n'),
    'utf8',
  )
  const door = spawn(process.execPath, [path.join(P.REPO, 'home.js'), 'doors', '--home', homeDir], {
    stdio: 'ignore',
    windowsHide: true,
    env: { ...process.env, CELIRA_HOME_DIR: homeDir },
  })
  try {
    let up = false
    for (let attempt = 0; attempt < 30 && !up; attempt += 1) {
      try {
        const [a, b] = await Promise.all([
          fetch(`http://127.0.0.1:${statusPort}/healthz`),
          fetch(`http://127.0.0.1:${configPort}/home-config`),
        ])
        up = a.ok && b.ok
      } catch {
        await sleep(300)
      }
    }
    assert.ok(up, '两个端面都应当起来（否则这条用例什么也没验）')

    // **一个 pid**：两个口上的监听者必须是同一支进程。
    const snapshot = proc.listenerSnapshot()
    const holders = [statusPort, configPort].map((port) => {
      const found = []
      for (const [pid, ports] of snapshot) {
        if (ports.has(port)) found.push(pid)
      }
      return found
    })
    assert.deepEqual(holders[0], [door.pid], `读口上的监听者应当是它：${JSON.stringify(holders[0])}`)
    assert.deepEqual(holders[1], [door.pid], `写口上的监听者应当是它：${JSON.stringify(holders[1])}`)

    // 两个端面各答各的。
    const status = await fetch(`http://127.0.0.1:${statusPort}/status`)
    assert.equal(status.status, 200)
    const payload = await status.json()
    assert.ok(payload.residents !== undefined, '读端面的载荷里应当有住户表')
    const config = await fetch(`http://127.0.0.1:${configPort}/home-config`)
    assert.equal(config.status, 200)

    // **不串门**：读口上没有写路由、写口上没有读路由。
    const crossA = await fetch(`http://127.0.0.1:${statusPort}/home-config`)
    assert.equal(crossA.status, 404, '读口上不许有写端面的路由')
    const crossB = await fetch(`http://127.0.0.1:${configPort}/status`)
    assert.equal(crossB.status, 404, '写口上不许有读端面的路由')
    const crossC = await fetch(`http://127.0.0.1:${configPort}/healthz`)
    assert.equal(crossC.status, 404, '写口上也不许有读端面的探活路由')

    // `status` 读数认得出它，并且**说清那是一个进程的两个端面**。
    const done = spawnSync(process.execPath, [path.join(P.REPO, 'home.js'), 'status'], {
      encoding: 'utf8',
      windowsHide: true,
      timeout: 30000,
      env: { ...process.env, CELIRA_HOME_DIR: homeDir },
    })
    const text = `${done.stdout ?? ''}${done.stderr ?? ''}`
    assert.match(text, new RegExp(`\\[status\\] 在跑 .*:${statusPort}/status`), `status 应当报读端面：${text}`)
    assert.match(text, new RegExp(`\\[config\\] 在跑 .*:${configPort}/home-config`), `status 应当报写端面：${text}`)
    assert.match(text, /同一个进程/, 'status 要说清这两个口是一个进程的两个端面')
    // ⚠ 判据要**点名**：这个 scratch 家里世界/宿主本来就没起（那几行"没在跑"是真的），
    // 所以只钉门那两行 —— 泛泛地 `!/没在跑/` 会把自己的判据写成假红。
    assert.ok(!/\[status\] 没在跑/.test(text), '不许把在跑的读端面说成没跑')
    assert.ok(!/\[config\] 没在跑/.test(text), '不许把在跑的写端面说成没跑')
  } finally {
    door.kill()
    await sleep(400)
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

// ★ **门跑着的时候安的那一支，也算数**（2026-10-12 真机 drill 撞出来的）。
//
// 症状（真机）：门在 20:34 起来、第二支 21:2x 才安家 ⇒ `GET /home-config` 的逐支读数里
// **永远没有它**（只报一支），而同一刻起一个新鲜进程（`node home.js status`）看得到两支 ——
// 也就是**门把自己开口那一刻的世界当成了现在**；反过来，删掉一支之后门还会一直报着那一行。
// 根因：`doors.js` 把一份**数组**交给两个端面，那份数组到门退出为止都不会变。
//
// 判据（真进程黑盒）：起真门 ⇒ 数一次 ⇒ **在它跑着的时候**往盘上加一支（新布局：`agents/<id>/`
// 里有 `agent.conf` 或 `identity.json` 就算一支）⇒ 再数一次，**必须多出它**，而且它那一行的
// 绑定要读得出刚写下去的那份文件。权威是磁盘上那份住户表，门只是通道。
doorsCase('★ 住户表**每次现问**：门跑着的时候安一支 ⇒ 那两个端面的读数里当场就有它', async () => {
  const homeDir = scratch('live-residents')
  const statusPort = 59333
  const configPort = 59334
  const makeResident = (id, { slot, source }) => {
    const envDir = path.join(homeDir, 'agents', id)
    fs.mkdirSync(envDir, { recursive: true })
    fs.writeFileSync(
      path.join(envDir, 'agent.conf'),
      `agent_id=${id}\nroom_id=\nslot=${slot}\n`,
      'utf8',
    )
    fs.writeFileSync(path.join(envDir, 'identity.json'), JSON.stringify({ agent: id }), 'utf8')
    modelSources.writeBinding(envDir, source)
  }
  if (!(await portFree(statusPort)) || !(await portFree(configPort))) {
    console.log(`      （跳过：端口 ${statusPort}/${configPort} 上已经有东西在跑）`)
    fs.rmSync(homeDir, { recursive: true, force: true })
    return
  }
  fs.writeFileSync(
    path.join(homeDir, 'home.conf'),
    [`status_port=${statusPort}`, `config_port=${configPort}`, ''].join('\n'),
    'utf8',
  )
  modelSources.writeList(homeDir, [
    {
      id: 'ms-1',
      name: '主钥匙',
      endpoint: 'https://a.invalid/v1/chat/completions',
      api_key: 'k1',
      model: 'm1',
      context_window: 8192,
    },
    {
      id: 'ms-2',
      name: '备用',
      endpoint: 'https://b.invalid/v1/chat/completions',
      api_key: 'k2',
      model: 'm2',
      context_window: 4096,
    },
  ])
  makeResident('a-1', { slot: 1, source: 'ms-1' })

  const door = spawn(process.execPath, [path.join(P.REPO, 'home.js'), 'doors', '--home', homeDir], {
    stdio: 'ignore',
    windowsHide: true,
    env: { ...process.env, CELIRA_HOME_DIR: homeDir },
  })
  try {
    let up = false
    for (let attempt = 0; attempt < 30 && !up; attempt += 1) {
      try {
        const [a, b] = await Promise.all([
          fetch(`http://127.0.0.1:${statusPort}/healthz`),
          fetch(`http://127.0.0.1:${configPort}/home-config`),
        ])
        up = a.ok && b.ok
      } catch {
        await sleep(300)
      }
    }
    assert.ok(up, '两个端面都应当起来（否则这条用例什么也没验）')

    const before = await (await fetch(`http://127.0.0.1:${configPort}/home-config`)).json()
    assert.equal(before.boundPerResident.length, 1, '前提：门起来时只有一支')
    assert.equal(before.bindingDivergent, false)

    // **门跑着的时候**安第二支（这一步就是真机上做过的那一步）。
    makeResident('a-2', { slot: 2, source: 'ms-2' })

    const after = await (await fetch(`http://127.0.0.1:${configPort}/home-config`)).json()
    assert.equal(after.boundPerResident.length, 2, '第二支必须在读数里（从前它到门退出都不出现）')
    assert.deepEqual(
      after.boundPerResident.map((row) => row.bound),
      ['ms-1', 'ms-2'],
      '两支各绑不同：新那一支的绑定要**当场**读得出来',
    )
    assert.equal(after.bindingDivergent, true, '各支不一致 ⇒ 如实标出来')
    assert.equal(after.bound, 'ms-1', '整机那一格仍是第一支那条（口径未变）')

    // 读端面那份住户表也要跟着长（它是**逐支**读数：运行 / 活动都按它给）。
    //
    // ⚠ **"当场"这三个字对它不成立**：读面有自己的 **TTL（15 秒）**缓存（`status-server.js`
    // 顶部那段：开机先热一次，页面第一眼不必等 3~4 秒；载荷里 `readAt` / `stale` 如实标出）。
    // 那是**有界且已经说出口**的滞后 —— 与这一刀治的那个病（**无界**：从门开口那一刻冻到进程
    // 退出为止）不是同一件事。所以这里**等**它一个 TTL，而不是把它也写成"当场"。
    let statusIds = []
    for (let attempt = 0; attempt < 20 && statusIds.length < 2; attempt += 1) {
      const payload = await (await fetch(`http://127.0.0.1:${statusPort}/status`)).json()
      statusIds = payload.residents.map((each) => each.agentId)
      if (statusIds.length < 2) await sleep(1000)
    }
    assert.deepEqual(statusIds, ['a-1', 'a-2'], '读端面的住户表最多滞后一个 TTL（15 秒），不许冻住')
  } finally {
    door.kill()
    await sleep(400)
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

// ★ **账本丢了、人还在**：`down` 照旧收得掉（2026-10-11 补的**真用例**）。
//
// 判据在 `down.js` 最后那一道（`sweepPort`）：账本（`logs/doors.pid`）是**更弱的来源** ——
// 它可能被清掉、可能是陈旧号；**端口 ＋ 家事实**（`ownsPid('doors', …)`）才是硬的。
// 所以"账本没了"**不许**变成"本来就没跑" —— 那会把孤儿留在盘上占着两个口。
// ⚠ 这不是理论：2026-10-11 真撞上（一个临时家的门活着占住 5190/5191，账本没了 ⇒
// `down` 说"本来就没跑" ⇒ 后面每一次产品形态门禁量的都是它，看着像代码坏了）。
// 补这一条之前，那一段行为**只写在注释里**（注释还比实现宽：门的两个口当时根本没被扫）。
doorsCase('★ 账本被清掉之后 `down` 照旧收掉自己的门（认端口 ＋ 家事实，不认账本）', async () => {
  const homeDir = scratch('orphan')
  const statusPort = 59331
  const configPort = 59332
  fs.mkdirSync(path.join(homeDir, 'logs'), { recursive: true })
  fs.writeFileSync(
    path.join(homeDir, 'home.conf'),
    ['listen_addr=127.0.0.1:59330', `status_port=${statusPort}`, `config_port=${configPort}`, ''].join('\n'),
    'utf8',
  )
  const door = spawn(process.execPath, [path.join(P.REPO, 'home.js'), 'doors', '--home', homeDir], {
    stdio: 'ignore',
    windowsHide: true,
    env: { ...process.env, CELIRA_HOME_DIR: homeDir },
  })
  try {
    let up = false
    for (let attempt = 0; attempt < 30 && !up; attempt += 1) {
      try {
        const [a, b] = await Promise.all([
          fetch(`http://127.0.0.1:${statusPort}/healthz`),
          fetch(`http://127.0.0.1:${configPort}/home-config`),
        ])
        up = a.ok && b.ok
      } catch {
        await sleep(300)
      }
    }
    assert.ok(up, '两个端面都应当起来（否则这条用例什么也没验）')

    // **把账本清掉**（模拟"账本丢了 / 被清了"）—— 进程照旧活着、两个口照旧在答。
    fs.rmSync(path.join(homeDir, 'logs', 'doors.pid'), { force: true })
    assert.ok(proc.isAlive(door.pid), '清账本不该把进程也清掉（否则这条用例验的不是它）')

    const done = spawnSync(process.execPath, [path.join(P.REPO, 'home.js'), 'down'], {
      encoding: 'utf8',
      windowsHide: true,
      timeout: 30000,
      env: { ...process.env, CELIRA_HOME_DIR: homeDir },
    })
    const text = `${done.stdout ?? ''}${done.stderr ?? ''}`
    // ① 必须**认出**那是自己的孤儿（而不是"已经在停着（本来就没跑）"）。
    assert.match(text, /\[doors\] 端口 \d+ 上还有孤儿/, `down 应当认出账本之外的孤儿：${text}`)
    // ② 认出来就得**收掉**：进程没了，两个口都空出来。
    await sleep(700)
    assert.ok(!proc.isAlive(door.pid), `孤儿应当被收掉：${text}`)
    assert.ok(
      !proc.portInUse(statusPort) && !proc.portInUse(configPort),
      '两个口都该空出来（否则下一次门禁量的还是它）',
    )
  } finally {
    try {
      door.kill()
    } catch {
      /* 已退出 */
    }
    await sleep(300)
    fs.rmSync(homeDir, { recursive: true, force: true })
  }
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '本文件一条用例都没跑（空跑不算过）')
  assert.ok(cases >= 7, `用例数 ${cases} 少于声明的 7`)
})
