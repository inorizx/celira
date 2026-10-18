'use strict'
// **一个家住 N 支 Agent**（2026-10-01 多支刀，承 PROJECT_SHAPE §8.0）—— 编排层的机械判据。
//
// 三件事在这里被钉住（都在 `node home.js up/status/down` 这条路上）：
//  ① `up` 的**目标裁决**（`planUpTargets`）：无参数 = 遍历住户（一支都没有 ⇒ 自建第一支）；
//     `--agent <id>` = 只给这一支安家（已有 ⇒ 重跑它；没有 ⇒ 认领成**新住户**，别人照旧）；
//  ② `status` **按支读数**：一个家住两支 ⇒ 两支各一行（身份 / 房 / 宿主 / 正忙 / 凭据 / 模型绑定）；
//  ③ `down --agent <id>` **只停这一支**：别的住户的账本一个字都不动，世界 / 页面 / 两个门不碰。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const P = require('../paths')
const { EXIT } = require('../codes')
const residents = require('../residents')
const up = require('../up')
const down = require('../down')
const { readUserEnv } = require('../userenv')
const status = require('../status')
const statusServer = require('../status-server')

let cases = 0
function mCase(name, fn) {
  test(name, async (t) => {
    // ⚠ 计数在**跑之前**加：这一格量的是"本文件真的执行了用例"（有没有空跑），不是"过了几条"。
    cases += 1
    await fn(t)
  })
}

/** 一个临时的家：端口挑**不像有人在听**的高位口（判据自己不许去碰真的 8080）。 */
function scratchHome(label, confLines = []) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), `celira-multi-${label}-`))
  const homeDir = path.join(root, 'home')
  fs.mkdirSync(path.join(homeDir, 'logs'), { recursive: true })
  const confFile = path.join(homeDir, 'home.conf')
  fs.writeFileSync(
    confFile,
    ['listen_addr=127.0.0.1:59321', 'client_port=59322', 'status_port=59323', 'config_port=59324', ...confLines, ''].join('\n'),
    'utf8',
  )
  return { root, homeDir, confFile }
}

/** 造一支住户：环境根 ＋ `agent.conf` ＋（可选）模型绑定 / 账本 pid 文件。 */
function makeResident(homeDir, agentId, { slot, roomId, binding = null, pid = null } = {}) {
  const envDir = P.agentEnvRoot(homeDir, agentId)
  fs.mkdirSync(envDir, { recursive: true })
  fs.writeFileSync(path.join(envDir, 'identity.json'), JSON.stringify({ agent: agentId }), 'utf8')
  residents.writeAgentConf(P.agentConfPath(homeDir, agentId), { agentId, roomId, slot })
  if (binding) {
    fs.writeFileSync(path.join(envDir, 'model_binding.json'), `${JSON.stringify({ source: binding }, null, 2)}\n`, 'utf8')
  }
  if (pid !== null) {
    const file = P.agentPidFile(homeDir, agentId)
    fs.mkdirSync(path.dirname(file), { recursive: true })
    fs.writeFileSync(file, `${pid}\n`, 'utf8')
  }
  return envDir
}

/**
 * 一支**已安家**的住户（环境根 ＋ 身份 ＋ `agent.conf`），形状与 `residents.residents()` 给的一致。
 *
 * 为什么要这个 helper：`planUpTargets` 的判据是"**这一支安没安家**"（有身份 **且** 有住户序号），
 * 所以夹具必须真的造出这两样 —— 拿 `{agentId, slot}` 这种裸对象当住户，测的就不是真实形状了。
 */
function residentEntry(homeDir, agentId, slot) {
  const envDir = makeResident(homeDir, agentId, { slot, roomId: `r-${slot}` })
  return { agentId, slot, roomId: `r-${slot}`, displayName: '', legacy: false, mismatch: null, envDir }
}

/** 把一段运行的读数抓下来（这几个命令都把读数打在 console 上）。 */
async function captureLog(fn) {
  const lines = []
  const original = console.log
  console.log = (...args) => {
    lines.push(args.join(' '))
  }
  try {
    await fn()
  } finally {
    console.log = original
  }
  return lines
}

/** 在一个临时的家上跑一段（`status` / `down` 从环境变量读家）。 */
async function withHome(homeDir, fn) {
  const previous = process.env.CELIRA_HOME_DIR
  process.env.CELIRA_HOME_DIR = homeDir
  try {
    return await fn()
  } finally {
    if (previous === undefined) delete process.env.CELIRA_HOME_DIR
    else process.env.CELIRA_HOME_DIR = previous
  }
}

// ── ① `up` 的目标裁决（纯函数）──────────────────────────────────────────────

mCase('★ `up` 无参数：家里一支都没有 ⇒ **自建第一支**（保住"一条命令就能用"）', () => {
  const plan = up.planUpTargets({ residentsList: [], adoptAgentId: null })
  assert.equal(plan.error, null)
  assert.equal(plan.targets.length, 1)
  assert.equal(plan.targets[0].resident, null, '还没有住户 ⇒ 交给布置去建')
  assert.equal(plan.targets[0].adoptId, null)
  assert.equal(plan.targets[0].slot, 1, '第一支的住户序号是 1（键形状无后缀）')
})

mCase('★ `up` 无参数：家里有住户 ⇒ **遍历每一支**（各起一个宿主）', () => {
  const home = scratchHome('plan-all')
  const list = [
    residentEntry(home.homeDir, 'a-1', 1),
    residentEntry(home.homeDir, 'a-2', 2),
  ]
  const plan = up.planUpTargets({ residentsList: list, adoptAgentId: null })
  assert.deepEqual(plan.targets.map((t) => t.resident.agentId), ['a-1', 'a-2'])
  assert.ok(plan.targets.every((t) => t.adoptId === null), '遍历不认领任何东西')
})

mCase('★ `up` 无参数：**半成品**（目录在、还没安家）不进遍历（别拿它去认领世界）', () => {
  const home = scratchHome('plan-half')
  const settled = residentEntry(home.homeDir, 'a-1', 1)
  // 半成品：只写了 agent.conf（手、显示名），**没有身份、没有住户序号**
  const halfDir = P.agentEnvRoot(home.homeDir, 'a-half')
  fs.mkdirSync(halfDir, { recursive: true })
  fs.writeFileSync(P.agentConfPath(home.homeDir, 'a-half'), 'mcp=none\nterminal=none\n', 'utf8')
  const plan = up.planUpTargets({
    residentsList: [settled, { agentId: 'a-half', slot: null, envDir: halfDir, legacy: false }],
    adoptAgentId: null,
  })
  assert.deepEqual(plan.targets.map((t) => t.resident.agentId), ['a-1'], '半成品不进遍历')
})

mCase('★ `up --agent <id>`：家里已经有这一支**且已安家** ⇒ **只重跑它**（不认领、不动别人）', () => {
  const home = scratchHome('plan-one')
  const list = [
    residentEntry(home.homeDir, 'a-1', 1),
    residentEntry(home.homeDir, 'a-2', 2),
  ]
  const plan = up.planUpTargets({ residentsList: list, adoptAgentId: 'a-2' })
  assert.equal(plan.targets.length, 1, '只动这一支')
  assert.equal(plan.targets[0].resident.agentId, 'a-2')
  assert.equal(plan.targets[0].adoptId, null, '已经是住户 ⇒ 不是"认领"')
})

mCase('★ `up --agent <id>`：只有**半成品**目录 ⇒ **把它安下来**（认领 ＋ 补住户序号，不再停手要用户手改）', () => {
  // 2026-10-01 第五刀真机发现的坑：先给第二支写 `agents/<支>/agent.conf`（手、显示名），
  // 再 `up --agent <它>` —— 从前这里把"目录在"读成"已安家"，于是**停手**并要用户手工补 `slot=`。
  const home = scratchHome('plan-half-adopt')
  const halfDir = P.agentEnvRoot(home.homeDir, 'a-half')
  fs.mkdirSync(halfDir, { recursive: true })
  fs.writeFileSync(P.agentConfPath(home.homeDir, 'a-half'), 'mcp=none\nterminal=none\n', 'utf8')
  const plan = up.planUpTargets({
    residentsList: [{ agentId: 'a-half', slot: null, envDir: halfDir, legacy: false }],
    adoptAgentId: 'a-half',
  })
  assert.equal(plan.targets.length, 1)
  assert.equal(plan.targets[0].resident.agentId, 'a-half', '认到那一支上（它的手与显示名要留着）')
  assert.equal(plan.targets[0].adoptId, 'a-half', '要认领世界里的那一支')
  assert.equal(plan.targets[0].slot, 1, '补一个住户序号（不猜、不空转）')
})

mCase('★ `up --agent <id>`：有身份**但没住户序号** ⇒ 也算半成品（补序号，不空转）', () => {
  const home = scratchHome('plan-noslot')
  const envDir = P.agentEnvRoot(home.homeDir, 'a-1')
  fs.mkdirSync(envDir, { recursive: true })
  fs.writeFileSync(path.join(envDir, 'identity.json'), JSON.stringify({ agent: 'a-1' }), 'utf8')
  residents.writeAgentConf(P.agentConfPath(home.homeDir, 'a-1'), { agentId: 'a-1', roomId: '', slot: null })
  const plan = up.planUpTargets({
    residentsList: [{ agentId: 'a-1', slot: null, envDir, legacy: false }],
    adoptAgentId: 'a-1',
  })
  assert.equal(plan.targets[0].adoptId, 'a-1')
  assert.equal(plan.targets[0].slot, 1)
})

mCase('★ `up --agent <id>`：家里没有这一支 ⇒ **认领成新住户**（序号接着排，别人照旧）', () => {
  const list = [
    { agentId: 'a-1', slot: 1 },
    { agentId: 'a-3', slot: 3 },
  ]
  const plan = up.planUpTargets({ residentsList: list, adoptAgentId: 'a-new' })
  assert.equal(plan.targets.length, 1)
  assert.equal(plan.targets[0].resident, null)
  assert.equal(plan.targets[0].adoptId, 'a-new')
  assert.equal(plan.targets[0].slot, 4, '住户序号 = 最大 + 1（绝不回收）')
})

// ── ② `status` 按支读数 ─────────────────────────────────────────────────────

mCase('★★ `status`：一个家住两支 ⇒ **两支各一行**（身份 / 房 / 宿主 / 正忙 / 凭据 / 模型绑定）', async () => {
  const home = scratchHome('status', ['home_id=H-S'])
  makeResident(home.homeDir, 'a-1', { slot: 1, roomId: 'r-1', binding: 'ms-1' })
  makeResident(home.homeDir, 'a-2', { slot: 2, roomId: 'r-2' })

  const lines = await withHome(home.homeDir, () => captureLog(() => status.status()))
  const residentLines = lines.filter((line) => line.startsWith('[resident] '))
  assert.equal(residentLines.length, 2, `一支一行，实际：\n${lines.join('\n')}`)
  assert.match(residentLines[0], /^\[resident\] a-1（住户 1，房 r-1）：/)
  assert.match(residentLines[0], /宿主 没在跑/, '没有宿主在跑 ⇒ 如实说')
  assert.match(residentLines[0], /正忙 否/)
  assert.match(residentLines[0], /凭据 (在场|没有|读不出来)/, '凭据那一格只给形状（**不给值**）')
  assert.match(residentLines[0], /模型绑定 在场（ms-1）/)
  assert.match(residentLines[1], /^\[resident\] a-2（住户 2，房 r-2）：/)
  assert.match(residentLines[1], /模型绑定 没有/, '这一支没绑 ⇒ 如实说"没有"（不是"读不出来"）')
  // 支级细读也按支来（多支时带上 id）。
  assert.ok(lines.some((line) => line.startsWith('[agent] a-1：没在跑')), `缺 a-1 的细读：\n${lines.join('\n')}`)
  assert.ok(lines.some((line) => line.startsWith('[agent] a-2：没在跑')), `缺 a-2 的细读：\n${lines.join('\n')}`)
  // 读数里**不许**出现凭据原文（只给形状）。
  assert.ok(!lines.some((line) => /\.token|clr_/.test(line)), '读数不许带凭据的形状之外的东西')
})

mCase('★ `status`：**有 home.conf、但一支住户都没有** ⇒ 说"一支都没有"，**别的东西照旧读**', async () => {
  // ⚠ **2026-10-11 改判据**（原来这条钉的是错话）：从前 `status` 把"没有 home.conf"与
  //   "一个住户都没有"合成一句「家里还没有布置过」并**提前 return** ⇒ 一个**已经有世界**的家，
  //   连"世界在不在跑、门在不在听"都读不到（**人不许比系统知道得更少**），给的下一步（`up`）也不对。
  //   现在两件事分开：没有 conf ⇒ 还没布置过（退出码 2）；**有 conf、没住户** ⇒ 照旧读完各节，
  //   只在住户那一节如实说"一支都没有"。
  const home = scratchHome('status-empty', ['home_id=H-E'])
  const lines = await withHome(home.homeDir, () => captureLog(() => status.status()))
  assert.ok(
    !lines.some((line) => line.includes('家里还没有布置过')),
    `这个家有 home.conf ⇒ 不许说"还没有布置过"（那是另一档）：\n${lines.join('\n')}`,
  )
  assert.ok(
    lines.some((line) => line.includes('[住户] 一支都没有')),
    `住户那一节要如实说"一支都没有"：\n${lines.join('\n')}`,
  )
  // ★ 这一半才是重点：**别的东西照旧读**（世界 / 两个门 / client 各节都在）。
  for (const section of ['[im]', '[status]', '[config]']) {
    assert.ok(
      lines.some((line) => line.startsWith(section)),
      `少了 ${section} 那一节 —— "没住户"不该把别的读数一起吞掉：\n${lines.join('\n')}`,
    )
  }
  // 而"不编一支出来"这半边照旧（一条 `[resident]` 都不许有）。
  assert.equal(lines.filter((line) => line.startsWith('[resident] ')).length, 0)
})

mCase('★ `status`：**没有 home.conf** ⇒ 才是"还没布置过"（退出码 2，别的什么都不读）', async () => {
  const home = scratchHome('status-no-conf', [])
  // ⚠ `scratchHome` **总会写一份 `home.conf`** ⇒ 这一档要自己把它删掉（否则量的是上面那一档）。
  fs.rmSync(home.confFile, { force: true })
  const lines = await withHome(home.homeDir, () => captureLog(() => status.status()))
  assert.ok(
    lines.some((line) => line.includes('家里还没有布置过（没有 home.conf）')),
    `实际读数：\n${lines.join('\n')}`,
  )
  assert.equal(lines.filter((line) => line.startsWith('[resident] ')).length, 0)
})

mCase('★ `status`：搬家后留在老家的**旧账本**只提示孤儿（本家住户的不提示；没有孤儿 ⇒ 零噪音）', async () => {
  // 2026-10-01 第七刀：一支 Agent 搬家 = 环境根整份搬走，而账本按设计留在老家
  // （`logs/agents/<支>/`）。`up` / `down` 都不碰它；`status` **仅当**它不属于本家任何一支时
  // 提示一行（"可能搬走了 —— 可以删"），否则一个字都不多打。
  const home = scratchHome('status-orphan', ['home_id=H-O'])
  makeResident(home.homeDir, 'a-1', { slot: 1, roomId: 'r-1' })
  // 本家住户的账本目录（在，但不是孤儿）＋ 一支搬走后留下的账本目录（孤儿）。
  fs.mkdirSync(P.agentLogDir(home.homeDir, 'a-1'), { recursive: true })
  fs.mkdirSync(P.agentLogDir(home.homeDir, 'a-moved'), { recursive: true })

  const lines = await withHome(home.homeDir, () => captureLog(() => status.status()))
  const orphans = lines.filter((line) => line.startsWith('[旧账本] '))
  assert.deepEqual(
    orphans,
    ['[旧账本] logs/agents/a-moved/：本家没有这一支（可能搬走了）—— 可以删'],
    `孤儿提示只该有搬走那支：\n${lines.join('\n')}`,
  )

  // 负对照（零噪音）：把那支孤儿目录删掉 ⇒ 这一行一个字都不出现。
  fs.rmSync(P.agentLogDir(home.homeDir, 'a-moved'), { recursive: true })
  const clean = await withHome(home.homeDir, () => captureLog(() => status.status()))
  assert.ok(
    !clean.some((line) => line.startsWith('[旧账本]')),
    `没有孤儿时读数一字不变：\n${clean.join('\n')}`,
  )
})

// ── ③ `down --agent` 只停一支 ───────────────────────────────────────────────

mCase('★★ `down --agent <id>`：只停这一支（账本清掉），**别的住户一个字都不动**', async () => {
  const home = scratchHome('down-one', ['home_id=H-D'])
  // 两支都记着一个**已经不在的 pid**（真进程一个都不起：本用例只判"谁被动了"）。
  makeResident(home.homeDir, 'a-1', { slot: 1, roomId: 'r-1', pid: 999999 })
  makeResident(home.homeDir, 'a-2', { slot: 2, roomId: 'r-2', pid: 999998 })
  const pidOne = P.agentPidFile(home.homeDir, 'a-1')
  const pidTwo = P.agentPidFile(home.homeDir, 'a-2')

  const lines = await withHome(home.homeDir, () => captureLog(() => down.down(['--agent', 'a-1'])))
  const said = lines.join('\n')
  assert.ok(!fs.existsSync(pidOne), `这一支的账本要清掉：\n${said}`)
  assert.ok(fs.existsSync(pidTwo), `**别的住户的账本一个字都不许动**：\n${said}`)
  assert.equal(fs.readFileSync(pidTwo, 'utf8'), '999998\n')
  assert.match(said, /只停这一支住户：a-1/)
  // 世界 / 页面 / 两个门**不碰**（那三支的读数一行都不该出现）。
  for (const tag of ['[im]', '[client]', '[status]', '[config]']) {
    assert.ok(!said.includes(tag), `只停一支时不许碰 ${tag}：\n${said}`)
  }
  assert.match(said, /只停了这一支（a-1）/)
})

mCase('★ `down`（不带 --agent）：整个家 ⇒ 每一支都停（各清各的账本）', async () => {
  const home = scratchHome('down-all', ['home_id=H-D2'])
  makeResident(home.homeDir, 'a-1', { slot: 1, roomId: 'r-1', pid: 999999 })
  makeResident(home.homeDir, 'a-2', { slot: 2, roomId: 'r-2', pid: 999998 })
  const pidOne = P.agentPidFile(home.homeDir, 'a-1')
  const pidTwo = P.agentPidFile(home.homeDir, 'a-2')
  const lines = await withHome(home.homeDir, () => captureLog(() => down.down([])))
  const said = lines.join('\n')
  assert.ok(!fs.existsSync(pidOne), `a-1 的账本要清掉：\n${said}`)
  assert.ok(!fs.existsSync(pidTwo), `a-2 的账本要清掉：\n${said}`)
  assert.match(said, /\[agent\] 住户 a-1：/, '多支时读数带上 id（一行读数不许让主人猜）')
  assert.match(said, /\[agent\] 住户 a-2：/)
  assert.ok(said.includes('[client]'), `整个家要走到常驻件那一步：\n${said}`)
})

mCase('★ `down --agent <id>`：家里没有这一支 ⇒ 用法错，并列出家里住着谁（绝不静默不动）', async () => {
  const home = scratchHome('down-missing', ['home_id=H-D3'])
  makeResident(home.homeDir, 'a-1', { slot: 1, roomId: 'r-1', pid: 999999 })
  const pidOne = P.agentPidFile(home.homeDir, 'a-1')
  let code = null
  const lines = await withHome(home.homeDir, () =>
    captureLog(async () => {
      code = await down.down(['--agent', 'a-nope'])
    }),
  )
  assert.equal(code, EXIT.USAGE)
  const said = lines.join('\n')
  assert.match(said, /没有这支住户：a-nope/)
  assert.match(said, /家里住着：a-1/)
  assert.ok(fs.existsSync(pidOne), '认不出目标 ⇒ 谁都不许被动')
})

// ── ④ 状态口的载荷：住户列表（每支一份）────────────────────────────────────

mCase('★★ 状态口载荷：`residents[]` **每支一份**（身份 / 序号 / 房 / 宿主 / 模型绑定），顶层照旧是第一支', async () => {
  const home = scratchHome('door', ['home_id=H-P'])
  makeResident(home.homeDir, 'a-1', { slot: 1, roomId: 'r-1', binding: 'ms-1', pid: 999999 })
  makeResident(home.homeDir, 'a-2', { slot: 2, roomId: 'r-2' })
  const list = residents.runtimeResidents(home.homeDir, home.confFile)
  assert.equal(list.length, 2)

  // 假 spawn：把"探测了几次"变成可判据（真二进制在本机存在，但用例不该依赖它）。
  // ⚠ 替身必须是**异步孩子**的形状（带流的 stdout/stderr ＋ close 事件）——同步形状喂进来
  // 只会得到假读数（那一课写在 `status-server.js` 的注入点注释里）。
  const spawned = []
  const fakeSpawn = () => {
    spawned.push(1)
    const { EventEmitter } = require('node:events')
    const { PassThrough } = require('node:stream')
    const child = new EventEmitter()
    child.stdout = new PassThrough()
    child.stderr = new PassThrough()
    setImmediate(() => {
      child.stdout.write(
        'celira-agent-host-status v1\nenvironment-root=x\nagent=a\nactivities-page=1 of=1 scope=latest\n\n[state]\nadmission=ready\n\n[next]\n活动已收场\n',
      )
      child.stdout.end()
      child.emit('close', 0)
    })
    return child
  }

  const payload = await statusServer.buildStatusPayload({
    homeDir: home.homeDir,
    layout: P.detectLayout(),
    imListen: '127.0.0.1:59321',
    residents: () => list,
    spawn: fakeSpawn,
    proc: {
      httpCode: async () => '000',
      // 宿主那一格要用的两把（生产由 `serve()` 传真的 proc 模块）。
      trustedLedgerPid: (kind, pidFile) => (fs.existsSync(pidFile) ? 999999 : null),
      hostOrphanPids: () => [],
    },
  })
  assert.equal(payload.residents.length, 2, '一支一份')
  assert.deepEqual(payload.residents.map((r) => r.agentId), ['a-1', 'a-2'])
  assert.equal(payload.residents[0].slot, 1)
  assert.equal(payload.residents[0].roomId, 'r-1')
  assert.equal(payload.residents[0].host.state, 'running')
  assert.equal(payload.residents[0].host.log, path.join('logs', 'agents', 'a-1', 'host.out'))
  assert.equal(payload.residents[1].host.state, 'stopped')
  assert.deepEqual(payload.residents[0].modelBinding, { state: 'bound', source: 'ms-1' })
  assert.deepEqual(payload.residents[1].modelBinding, { state: 'absent', source: null })
  // 顶层那三格 = **第一支**的读数（冻结的前端照旧读它；本刀只加不改）。
  assert.equal(payload.agent.agentId, 'a-1')
  assert.equal(payload.agent.state, 'loaded')
  assert.ok(typeof payload.readAt === 'string')
  // 端面纪律：载荷里**不许**有凭据那一格（口上无秘密）。
  assert.ok(!('credential' in payload.residents[0]), '状态口载荷不许带凭据（连"有没有"也不报）')
  assert.ok(!JSON.stringify(payload).includes('clr_'), '载荷里不许出现凭据形状')
  assert.equal(spawned.length, 2, '一支探一次（不是"整个家探一次"）')
})

/** 真的敲一次 `node home.js <参数>`（黑盒：与主人敲的是同一条路）。 */
function runHome(args, homeDir, extraEnv = {}) {
  const { spawnSync } = require('node:child_process')
  const done = spawnSync(process.execPath, [path.join(P.REPO, 'home.js'), ...args], {
    env: {
      ...process.env,
      CELIRA_HOME_DIR: homeDir,
      CELIRA_CREDENTIALS_DIR: path.join(homeDir, '..', 'creds-never-touched'),
      ...extraEnv,
    },
    encoding: 'utf8',
    windowsHide: true,
  })
  return { code: done.status, out: `${done.stdout ?? ''}${done.stderr ?? ''}` }
}

mCase('★★ 新住户不再被逼要模型钥匙：家里已经绑好 / 这一支先放好绑定 ⇒ 不报"缺凭据"', (t) => {
  // 世界一律给一个**不可达**的地址：模型那一关排在预检之前，所以"退出码是 2（世界不可达）而不是
  // 3（缺凭据）"正是这一条的判据；同时它保证用例不起任何进程、不出网。
  const remote = ['--im-endpoint', 'http://127.0.0.1:1', '--token', 'clr_hs_x']

  // ① 家里别的住户已经绑好模型 ⇒ 新住户跟着用同一条（不该再逼用户去配钥匙）
  const carried = scratchHome('keyless-carried', ['home_id=H-K1'])
  makeResident(carried.homeDir, 'a-1', { slot: 1, roomId: 'r-1', binding: 'ms-1' })
  const one = runHome(['up', ...remote, '--agent', 'a-new'], carried.homeDir, { DEEPSEEK_API_KEY: '' })
  assert.equal(one.code, EXIT.ENV_UNAVAILABLE, `应当是"世界不可达"而不是"缺凭据"：\n${one.out}`)
  assert.ok(!one.out.includes('缺凭据'), `不该再要模型钥匙：\n${one.out}`)

  // ② 这一支的环境根里已经先放好了绑定（它还没进住户表）⇒ 同样不要钥匙
  const seeded = scratchHome('keyless-seeded', ['home_id=H-K2'])
  const envDir = P.agentEnvRoot(seeded.homeDir, 'a-new')
  fs.mkdirSync(envDir, { recursive: true })
  fs.writeFileSync(path.join(envDir, 'model_binding.json'), '{\n  "source": "ms-1"\n}\n', 'utf8')
  const two = runHome(['up', ...remote, '--agent', 'a-new'], seeded.homeDir, { DEEPSEEK_API_KEY: '' })
  assert.equal(two.code, EXIT.ENV_UNAVAILABLE, `应当是"世界不可达"：\n${two.out}`)
  assert.ok(!two.out.includes('缺凭据'), `不该再要模型钥匙：\n${two.out}`)

  // ③ **半成品住户**（目录在、还没安家：没有身份 / 没有住户序号）＋ 家里别的支已绑好
  //    ⇒ 这次布置会把它的环境**建起来**、绑定跟着带进去 ⇒ 同样不该逼用户再配一次钥匙
  //    （2026-10-01 第五刀真机发现的坑：`up --agent <半成品>` 曾在这里以退出码 3 停住）。
  const half = scratchHome('keyless-half', ['home_id=H-K3'])
  makeResident(half.homeDir, 'a-1', { slot: 1, roomId: 'r-1', binding: 'ms-1' })
  const halfDir = P.agentEnvRoot(half.homeDir, 'a-half')
  fs.mkdirSync(halfDir, { recursive: true })
  fs.writeFileSync(P.agentConfPath(half.homeDir, 'a-half'), 'display_name=第二支\nmcp=none\nterminal=none\n', 'utf8')
  const three = runHome(['up', ...remote, '--agent', 'a-half'], half.homeDir, { DEEPSEEK_API_KEY: '' })
  assert.equal(three.code, EXIT.ENV_UNAVAILABLE, `半成品应当是"世界不可达"而不是"缺凭据"：\n${three.out}`)
  assert.ok(!three.out.includes('缺凭据'), `半成品不该再要模型钥匙（家里已经绑过了）：\n${three.out}`)

  // ④ 反面对照：**要布置一支、而它既没有绑定、家里也没有别的支可跟** ⇒ 仍然如实要求钥匙（退出码 3）。
  //    ⚠ 2026-10-05 修两处：
  //    ① 从前这里是"一支都没有的全新家"，但**没有要布置的 Agent 时根本不需要模型钥匙**
  //       （钥匙是给"要思考的那一支"的）⇒ `needsApiKey` 为假、直接走到远端预检去了；
  //    ② 这一格的**前置**是"这台机器的用户环境里没有那把钥匙" —— 而 `readUserEnv` 读的是
  //       `HKCU\Environment`（**不是进程环境**，子进程 env 覆盖不了它）⇒ 机器上有钥匙时
  //       这一档**判不动**。按仓库纪律：输入不在场 ⇒ **如实报不适用**，绝不假装通过。
  const fresh = scratchHome('keyless-fresh')
  if (readUserEnv('DEEPSEEK_API_KEY')) {
    t.skip('这台机器的用户环境里有 DEEPSEEK_API_KEY ⇒ "缺凭据"那一档这一轮没法验（不是通过）')
    return
  }
  const bare = runHome(['up', ...remote, '--agent', 'a-fresh'], fresh.homeDir, { DEEPSEEK_API_KEY: '' })
  assert.equal(bare.code, EXIT.NEEDS_YOU, `要布置一支而它没绑定 ⇒ 该如实说缺凭据：\n${bare.out}`)
  assert.match(bare.out, /缺凭据/)
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '本文件一条用例都没跑（空跑不算过）')
  assert.ok(cases >= 13, `用例数 ${cases} 少于声明的 13`)
})
