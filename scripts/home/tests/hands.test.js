'use strict'
// **一支的手**（2026-10-01 第五刀，承 §8.0.1 触发条件 ②）：外部 server 与终端档从"家级事实"
// 升成"**支级事实**"—— 手长在身体上。
//
// 这里钉住的判据：
//  ① 三条规则：支级 `mcp_*` **覆盖**家级 ／ 支级 `mcp=none` ⇒ **一只都不要** ／ 都没写 ⇒ **跟家走**；
//  ② 终端档：支级 `terminal=` ⇒ 家级 `terminal=` ⇒ **平台默认**（**2026-10-14 主人裁：默认不给手**，
//     要手只能显式写 `terminal=full`）；`terminal=none` 与"没写"都不给这只手，且**真的不进 argv**；
//  ②b **缺省变更的一次性迁移**（`up.migrateTerminalDefault`）：老家那些在旧缺省下"有手"的支，
//     把现行的 full **显式写进它们自己的 `agent.conf`**（别的字节不动），幂等，且**新建的支不吃这一刀**；
//  ③ 读数**按支说、说清来源**（跟家走 / 它自己的 agent.conf / 明确不要），多支时点名；
//  ④ 手**不会被身份更新吃掉**（`writeAgentConf` 重写 `agent_id`/`room_id`/`slot` 时，`mcp_*`
//     与 `terminal=` 行必须原样留着）；
//  ⑤ 状态口载荷只给**名字与数量**：命令原文不进载荷（那是 argv 的事）。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const P = require('../paths')
const residents = require('../residents')
const up = require('../up')
const status = require('../status')
const statusServer = require('../status-server')

let cases = 0
function hCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

/** 一个临时的家：家级 `home.conf` ＋ 一支住户（`agents/<支>/`）。 */
function scratch(label, confLines = []) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), `celira-hands-${label}-`))
  const homeDir = path.join(root, 'home')
  fs.mkdirSync(path.join(homeDir, 'logs'), { recursive: true })
  const confFile = path.join(homeDir, 'home.conf')
  fs.writeFileSync(
    confFile,
    ['listen_addr=127.0.0.1:59421', 'client_port=59422', 'status_port=59423', 'config_port=59424', ...confLines, ''].join('\n'),
    'utf8',
  )
  return { root, homeDir, confFile }
}

function makeResident(homeDir, agentId, { slot = 1, roomId = 'r-1', lines = [] } = {}) {
  const envDir = P.agentEnvRoot(homeDir, agentId)
  fs.mkdirSync(envDir, { recursive: true })
  fs.writeFileSync(path.join(envDir, 'identity.json'), JSON.stringify({ agent: agentId }), 'utf8')
  residents.writeAgentConf(P.agentConfPath(homeDir, agentId), { agentId, roomId, slot })
  if (lines.length > 0) fs.appendFileSync(P.agentConfPath(homeDir, agentId), `${lines.join('\n')}\n`, 'utf8')
  return { agentId, envDir, slot, roomId, displayName: '', legacy: false, mismatch: null }
}

function handsOf(homeDir, confFile, resident) {
  return residents.residentHands({ homeDir, resident, homeConfFile: confFile })
}

// ── ① 三条规则 ──────────────────────────────────────────────────────────────

hCase('★ 手的三条规则：支级覆盖家级 ／ `mcp=none` 一只都不要 ／ 没写就跟家走', () => {
  const home = scratch('rules', ['mcp_a=stdio:node a.js', 'mcp_b=stdio:node b.js'])
  const one = makeResident(home.homeDir, 'a-1')
  const two = makeResident(home.homeDir, 'a-2', { slot: 2, roomId: 'r-2', lines: ['mcp_c=stdio:node c.js'] })
  const three = makeResident(home.homeDir, 'a-3', { slot: 3, roomId: 'r-3', lines: ['mcp=none'] })

  const carried = handsOf(home.homeDir, home.confFile, one)
  assert.deepEqual(carried.mcp, ['a=stdio:node a.js', 'b=stdio:node b.js'])
  assert.equal(carried.mcpSource, 'home', '没写支级 ⇒ 跟家走（今天的行为一字不变）')

  const own = handsOf(home.homeDir, home.confFile, two)
  assert.deepEqual(own.mcp, ['c=stdio:node c.js'], '支级有 mcp_* ⇒ 用它（覆盖，不是合并）')
  assert.equal(own.mcpSource, 'resident')

  const none = handsOf(home.homeDir, home.confFile, three)
  assert.deepEqual(none.mcp, [], '`mcp=none` ⇒ 一只外部 server 都不要（哪怕家级声明了）')
  assert.equal(none.mcpSource, 'none')
})

// **技能引用**（2026-10-25）：支级事实，从 `agent.conf` 的 `skills=` 读；不跟家级走。
// 没写 = 空串 ⇒ 宿主缺省 all（不是「一项都不要」）。
hCase('★ 技能引用：支级 agent.conf 的 skills= 进 hands；没写就是缺省 all', () => {
  const home = scratch('skills', [])
  const declared = makeResident(home.homeDir, 'a-1', { lines: ['skills=release-note,ops'] })
  const silence = makeResident(home.homeDir, 'a-2', { slot: 2, roomId: 'r-2' })

  const hands = handsOf(home.homeDir, home.confFile, declared)
  assert.equal(hands.skills, 'release-note,ops', '支级声明原样读出来')
  assert.equal(handsOf(home.homeDir, home.confFile, silence).skills, '', '没写就是空（宿主缺省 all）')

  const lines = up.handNotices({
    who: '住户 a-1：',
    hands,
    terminal: residents.resolveTerminal(hands, { isWindows: false, isLinux: true }),
  })
  assert.ok(
    lines.some((line) => line.includes('[skills]') && line.includes('技能引用=release-note,ops')),
    `读数要说清技能引用：${lines.join('\n')}`,
  )

  const legacy = { agentId: 'a-old', envDir: '/tmp/legacy', slot: 1, roomId: 'r-old', legacy: true }
  assert.equal(handsOf(home.homeDir, home.confFile, legacy).skills, '', '老家没有支级声明位 ⇒ 缺省 all')
})

hCase('★ 老家那一支（还没迁）：手从 home.conf 读，来源如实报 `home`（不假装它有 agent.conf）', () => {
  const home = scratch('legacy', ['mcp_a=stdio:node a.js', 'agent_id=a-old', 'room_id=r-old'])
  const legacy = { agentId: 'a-old', envDir: P.legacyEnvRoot(home.homeDir), slot: 1, roomId: 'r-old', legacy: true }
  const hands = handsOf(home.homeDir, home.confFile, legacy)
  assert.deepEqual(hands.mcp, ['a=stdio:node a.js'])
  assert.equal(hands.mcpSource, 'home')
  assert.equal(hands.terminalSource, 'platform', '老家没有支级声明位 ⇒ 终端档仍是平台默认')
})

// ── ② 终端档 ────────────────────────────────────────────────────────────────

hCase('★ 终端档：支级/家级声明 ⇒ 进/不进 argv；没有臂的平台上声明 full 也**不带**（如实说）', () => {
  const envDir = '/tmp/whatever-env'
  const base = { envDir, agentId: 'a-1', materialsDir: '/tmp/m', listen: '127.0.0.1:1' }
  const withTerminal = (terminal, isWindows, isLinux) =>
    up.buildHostArgs({ ...base, terminal, isWindows, isLinux }).includes('--terminal')

  assert.equal(withTerminal('full', false, true), true, '有臂的平台 + full ⇒ 带 --terminal')
  assert.equal(withTerminal('none', false, true), false, '支级说不要 ⇒ **真的不带**（不是"少一个字节"）')
  assert.equal(withTerminal('full', false, false), false, '没有臂的平台：声明了也不带（有意的 fail-closed）')
  // ⚠ **2026-10-14 主人裁**：平台默认从"有臂就给 full"改成**不给手**。
  //    ⇒ 缺省那一档现在会打一行（那是新缺省，用户有权知道），而不是"照旧说手在场"。
  assert.equal(withTerminal(undefined, false, true), false, '缺省（平台默认）⇒ 不给手：不带 --terminal')

  const home = scratch('terminal', ['mcp_a=stdio:node a.js'])
  const resident = makeResident(home.homeDir, 'a-1', { lines: ['terminal=none'] })
  const hands = handsOf(home.homeDir, home.confFile, resident)
  const resolved = residents.resolveTerminal(hands, { isWindows: false, isLinux: true })
  assert.equal(resolved.mode, 'none')
  assert.equal(resolved.source, 'resident', '档位是**这一支**声明的（来源要说清）')
  assert.equal(resolved.unavailable, false, 'Linux 上有终端臂，不存在"装不进来"')

  const noArm = residents.resolveTerminal(
    { mcp: [], mcpSource: 'home', terminal: 'full', terminalSource: 'resident' },
    { isWindows: false, isLinux: false },
  )
  assert.equal(noArm.mode, 'none')
  assert.equal(noArm.unavailable, true, '声明了 full 但这台机器没有臂 ⇒ 读数要如实说"装不进来"')
})

// ── ③ 读数说清来源 ──────────────────────────────────────────────────────────

hCase('★ 读数：按支说、说清来源（跟家走 / 它自己的 / 明确不要），多支时点名', () => {
  const home = scratch('notices', ['mcp_a=stdio:node a.js'])
  const own = makeResident(home.homeDir, 'a-1', { lines: ['mcp_b=stdio:node b.js'] })
  const none = makeResident(home.homeDir, 'a-2', { slot: 2, roomId: 'r-2', lines: ['mcp=none', 'terminal=none'] })

  const linesOwn = up.handNotices({
    who: '住户 a-1：',
    hands: handsOf(home.homeDir, home.confFile, own),
    terminal: residents.resolveTerminal(handsOf(home.homeDir, home.confFile, own), { isWindows: false, isLinux: true }),
  }).join('\n')
  assert.match(linesOwn, /住户 a-1：装了 1 台外部 server：b（来自它自己的 agent.conf）/, linesOwn)

  const linesNone = up.handNotices({
    who: '住户 a-2：',
    hands: handsOf(home.homeDir, home.confFile, none),
    terminal: residents.resolveTerminal(handsOf(home.homeDir, home.confFile, none), { isWindows: false, isLinux: true }),
  }).join('\n')
  assert.match(linesNone, /住户 a-2：这一支不要外部 server（agent.conf 里写了 mcp=none）/, linesNone)
  assert.match(linesNone, /住户 a-2：这一支没有「终端」这只手（agent.conf 里写了 terminal=none）/, linesNone)

  // 跟家走：措辞要说出来源（不点名时不留"住户"前缀 —— 单支的家与从前一样）
  const carried = makeResident(home.homeDir, 'a-3', { slot: 3, roomId: 'r-3' })
  const linesCarried = up.handNotices({
    who: '',
    hands: handsOf(home.homeDir, home.confFile, carried),
    terminal: residents.resolveTerminal(handsOf(home.homeDir, home.confFile, carried), { isWindows: false, isLinux: true }),
  }).join('\n')
  assert.match(linesCarried, /装了 1 台外部 server：a（来自跟家走（home.conf））/, linesCarried)
  assert.ok(!linesCarried.includes('住户 '), '单支的家不该多出"住户"前缀（与从前一致）')

  // 一台都没有、也没写 mcp=none ⇒ **一个字都不打**（默认状态不是新闻）。
  // ⚠ 终端那一行是另一回事：**2026-10-14 起平台默认就是"不给手"**，所以缺省那一档会打一行
  //   （新缺省、说清怎么要手）——它不再是"这台机器上有臂 ⇒ 手在场"那句话。
  const quiet = makeResident(home.homeDir, 'a-4', { slot: 4, roomId: 'r-4' })
  const quietLines = up.handNotices({
    who: '',
    hands: { mcp: [], mcpSource: 'home', terminal: '', terminalSource: 'platform' },
    terminal: residents.resolveTerminal(
      { mcp: [], mcpSource: 'home', terminal: '', terminalSource: 'platform' },
      { isWindows: false, isLinux: true },
    ),
    platform: { isWindows: false, isLinux: true },
  })
  assert.deepEqual(
    quietLines.filter((line) => line.startsWith('[mcp]')),
    [],
    `默认状态不该有 mcp 读数：${quietLines.join('\n')}`,
  )
  assert.equal(quietLines.length, 1, `默认状态只该有终端那一行：${quietLines.join('\n')}`)
  assert.match(
    quietLines[0],
    /^\[terminal\] 平台默认不给「终端」这只手（2026-10-14 起）；要手就在它自己的 agent\.conf 里写 terminal=full$/,
    quietLines.join('\n'),
  )
  assert.ok(fs.existsSync(P.agentConfPath(home.homeDir, quiet.agentId)))
})

// ── ④ 手不会被身份更新吃掉 ──────────────────────────────────────────────────

hCase('★ 重写身份不许吞掉手：`writeAgentConf` 更新身份时 `mcp_*` / `terminal=` 原样留着', () => {
  const home = scratch('survive')
  const resident = makeResident(home.homeDir, 'a-1', { lines: ['mcp_b=stdio:node b.js', 'terminal=none', 'skills=release-note', 'note=keep'] })
  residents.writeAgentConf(P.agentConfPath(home.homeDir, 'a-1'), { agentId: 'a-1', roomId: 'r-9', slot: 1 })
  const text = fs.readFileSync(P.agentConfPath(home.homeDir, 'a-1'), 'utf8')
  assert.match(text, /room_id=r-9/, '身份那一行要更新')
  assert.match(text, /mcp_b=stdio:node b\.js/, '手不许被吞')
  assert.match(text, /terminal=none/, '终端档不许被吞')
  assert.match(text, /skills=release-note/, '技能引用不许被吞')
  assert.match(text, /note=keep/, '别人的行也不许被吞')
})

hCase('★ 逐支对账：支级**覆盖**不许被报成"不一致"（真机当场抓到的假警报）', () => {
  // 从前这一节拿**家级** `home.conf` 的声明去和某一支 argv 对账 ⇒ 任何支级覆盖 / `mcp=none`
  // 都会被读成"不一致"。判据改成按支之后：住户 1 的 own_probe、住户 2 的"一只都不要"都是**对的**。
  const residentsList = [
    { agentId: 'a-1', envDir: '/h/agents/a-1', hands: { mcp: ['own_probe=stdio:node own.js'], mcpSource: 'resident' } },
    { agentId: 'a-2', envDir: '/h/agents/a-2', hands: { mcp: [], mcpSource: 'none' } },
  ]
  const rows = { '/h/agents/a-1': { pid: 1, cmd: '--root /h/agents/a-1 --mcp own_probe=stdio:node own.js' }, '/h/agents/a-2': { pid: 2, cmd: '--root /h/agents/a-2' } }
  const lines = status.mcpReconcileLines({
    residentList: residentsList,
    confNames: ['home_probe'], // 家级那台**没带给任何人**（住户 1 覆盖了、住户 2 不要）
    rowOfResident: (resident) => rows[resident.envDir],
    namesOfRow: (row) => (row ? ['own_probe'].filter((name) => row.cmd.includes(name)) : []),
    hostCount: 2,
  }).join('\n')
  assert.match(lines, /住户 a-1：装了 1 台：own_probe（在跑的那支与这一支该有的手一致）/, lines)
  assert.ok(!lines.includes('不一致'), `有意的覆盖不许报成不一致：${lines}`)
  assert.ok(!lines.includes('住户 a-2'), `"一只都不要"且真的没装 ⇒ 整行不出现：${lines}`)
})

hCase('★ 逐支对账：真不一致要**说清差在哪**（这一支该有的 vs 在跑那支装的）', () => {
  const residentsList = [
    { agentId: 'a-1', envDir: '/h/agents/a-1', hands: { mcp: ['new_probe=stdio:node n.js'], mcpSource: 'resident' } },
  ]
  const lines = status.mcpReconcileLines({
    residentList: residentsList,
    confNames: [],
    rowOfResident: () => ({ pid: 1, cmd: '--root /h/agents/a-1 --mcp old_probe=stdio:node o.js' }),
    namesOfRow: () => ['old_probe'],
    hostCount: 1,
  }).join('\n')
  assert.match(lines, /这一支该装 1 台：new_probe；在跑那支装的是 1 台：old_probe——\*\*不一致\*\*/, lines)
  assert.match(lines, /只在"这一支该有的手"里：new_probe；只在在跑那支里：old_probe/, lines)
  assert.match(lines, /down \+ up/, lines)
})

hCase('★ 逐支对账：没在跑、还没有住户、宿主数多于住户——三种如实读数', () => {
  const stopped = status
    .mcpReconcileLines({
      residentList: [{ agentId: 'a-1', envDir: '/h/agents/a-1', hands: { mcp: ['p=stdio:node p.js'], mcpSource: 'home' } }],
      rowOfResident: () => null,
      namesOfRow: () => [],
    })
    .join('\n')
  assert.match(stopped, /这一支该装 1 台：p（没在跑，无从对账）/, stopped)

  const empty = status
    .mcpReconcileLines({ residentList: [], confNames: ['home_probe'], rowOfResident: () => null, namesOfRow: () => [] })
    .join('\n')
  assert.match(empty, /家级默认 1 台：home_probe（还没有住户，无从对账）/, empty)

  const weird = status
    .mcpReconcileLines({
      residentList: [{ agentId: 'a-1', envDir: '/h/agents/a-1', hands: { mcp: [], mcpSource: 'home' } }],
      rowOfResident: () => ({ pid: 1, cmd: '' }),
      namesOfRow: () => [],
      hostCount: 2,
    })
    .join('\n')
  assert.match(weird, /读到 2 支宿主，而家里只有 1 个住户——异常读数/, weird)
})

// ── ⑤ status 与状态口 ───────────────────────────────────────────────────────

hCase('★ `status` 一支一行里报手与终端（含来源），不给命令原文', async () => {
  const home = scratch('status', ['home_id=H-H', 'mcp_a=stdio:node a.js'])
  makeResident(home.homeDir, 'a-1', { lines: ['mcp=none', 'terminal=none'] })
  makeResident(home.homeDir, 'a-2', { slot: 2, roomId: 'r-2' })

  const lines = []
  const original = console.log
  console.log = (...args) => lines.push(args.join(' '))
  const previous = process.env.CELIRA_HOME_DIR
  process.env.CELIRA_HOME_DIR = home.homeDir
  try {
    await status.status()
  } finally {
    console.log = original
    if (previous === undefined) delete process.env.CELIRA_HOME_DIR
    else process.env.CELIRA_HOME_DIR = previous
  }
  const said = lines.join('\n')
  assert.match(said, /\[resident\] a-1（住户 1，房 r-1）：.*手 0 台（这一支不要外部 server）；终端 没有（它自己的声明）/, said)
  assert.match(
    said,
    /\[resident\] a-2（住户 2，房 r-2）：.*手 1 台（跟家走）；终端 没有（平台默认；要手就在它自己的 agent\.conf 里写 terminal=full）/,
    said,
  )
  assert.ok(!said.includes('stdio:node'), '读数只给名字，不给命令原文')
})

hCase('★ `status` 端到端：对账用的是**这一支该有的手**（裸住户表会把覆盖报成"不一致"）', async () => {
  // 真机抓到的第二个假警报：`status` 把**裸住户表**交给对账 ⇒ "这一支该装几台"永远读成 0
  // ⇒ 住户 1 自己的 `own` 被报成"不一致"。这里用一行**假的宿主进程行**把整条链走完。
  const home = scratch('status-reconcile', ['home_id=H-SR', 'mcp_home_probe=stdio:node h.js'])
  const a1 = makeResident(home.homeDir, 'a-1', { lines: ['mcp_own=stdio:node own.js'] })
  const a2 = makeResident(home.homeDir, 'a-2', { slot: 2, roomId: 'r-2', lines: ['mcp=none'] })

  const proc = require('../proc')
  const original = proc.describeHostProcesses
  proc.describeHostProcesses = () => [
    { pid: 4242, argv: ['agent_host', '--root', a1.envDir, '--agent', 'a-1', '--mcp', 'own=stdio:node own.js'] },
    { pid: 4243, argv: ['agent_host', '--root', a2.envDir, '--agent', 'a-2'] },
  ]
  const lines = []
  const originalLog = console.log
  console.log = (...args) => lines.push(args.join(' '))
  const previous = process.env.CELIRA_HOME_DIR
  process.env.CELIRA_HOME_DIR = home.homeDir
  try {
    await status.status()
  } finally {
    console.log = originalLog
    proc.describeHostProcesses = original
    if (previous === undefined) delete process.env.CELIRA_HOME_DIR
    else process.env.CELIRA_HOME_DIR = previous
  }
  const said = lines.join('\n')
  assert.match(said, /住户 a-1：装了 1 台：own（在跑的那支与这一支该有的手一致）/, said)
  assert.ok(!said.includes('**不一致**'), `支级覆盖不该被报成不一致：${said}`)
})

hCase('★ 状态口载荷：`hands` 每支一份（数量 ＋ 名字 ＋ 来源），命令原文不进载荷', async () => {
  const home = scratch('door', ['home_id=H-HD', 'mcp_a=stdio:node secret-path.js'])
  const list = [makeResident(home.homeDir, 'a-1', { lines: ['terminal=none'] }), makeResident(home.homeDir, 'a-2', { slot: 2, roomId: 'r-2' })]

  const { EventEmitter } = require('node:events')
  const { PassThrough } = require('node:stream')
  const fakeSpawn = () => {
    const child = new EventEmitter()
    child.stdout = new PassThrough()
    child.stderr = new PassThrough()
    setImmediate(() => {
      child.stdout.write('[state]\nadmission=ready\n')
      child.stdout.end()
      child.emit('close', 0)
    })
    return child
  }

  const payload = await statusServer.buildStatusPayload({
    homeDir: home.homeDir,
    layout: P.detectLayout(),
    imListen: '127.0.0.1:59421',
    residents: () => list,
    confFile: home.confFile,
    spawn: fakeSpawn,
    proc: { httpCode: async () => '000', trustedLedgerPid: () => null, hostOrphanPids: () => [] },
  })
  assert.deepEqual(payload.residents[0].hands, {
    mcp: { count: 1, names: ['a'], source: 'home' },
    terminal: { mode: 'none', source: 'resident', unavailable: false },
  })
  assert.deepEqual(payload.residents[1].hands.mcp, { count: 1, names: ['a'], source: 'home' })
  assert.ok(!JSON.stringify(payload).includes('secret-path.js'), '载荷里不许出现命令原文')
})

// ── ⑤ 缺省变更的一次性迁移（2026-10-14 主人裁）────────────────────────────────

hCase('★ 迁移：老家那些在旧缺省下"有手"的支 ⇒ 现行的 full 显式写进它自己的 agent.conf（别的行不动）', () => {
  const home = scratch('migrate')
  const one = makeResident(home.homeDir, 'a-1', { lines: ['note=keep-me'] })
  const two = makeResident(home.homeDir, 'a-2', { slot: 2, roomId: 'r-2', lines: ['terminal=none'] })
  const list = residents.residents(home.homeDir)
  assert.equal(list.length, 2)

  const before = fs.readFileSync(P.agentConfPath(home.homeDir, 'a-1'), 'utf8')
  const lines = up.migrateTerminalDefault({ homeDir: home.homeDir, confFile: home.confFile, residents: list })
  assert.equal(lines.length, 2, '一支一条 ＋ 一条总结')
  assert.match(lines[0], /住户 a-1 在旧缺省下是「有手」/)

  const after = fs.readFileSync(P.agentConfPath(home.homeDir, 'a-1'), 'utf8')
  assert.match(after, /^terminal=full$/m, '现行的 full 显式写进去了')
  assert.ok(after.startsWith(before.trimEnd()), '原有内容一个字节不动，只追加那一行')
  assert.match(after, /note=keep-me/, '别人的行不许被吞')

  assert.equal(
    fs.readFileSync(P.agentConfPath(home.homeDir, 'a-2'), 'utf8').includes('terminal=full'),
    false,
    '已经表过态的支（terminal=none）不许被覆盖 —— 那是别人已经说过的态',
  )

  // 迁移过的家：档位来源从"平台默认"变成"它自己的声明"，读数里因此看得见。
  const hands = handsOf(home.homeDir, home.confFile, list.find((r) => r.agentId === 'a-1'))
  assert.equal(hands.terminal, 'full')
  assert.equal(hands.terminalSource, 'resident', '迁移之后来源是**它自己的声明**，不再是平台默认')

  // 幂等：再跑一次什么都不做（一次性的历史补偿不许变成每次开机都写）。
  const again = up.migrateTerminalDefault({ homeDir: home.homeDir, confFile: home.confFile, residents: list })
  assert.deepEqual(again, [], '标记在场 ⇒ 一个字都不打、一个字节都不写')
  assert.equal(fs.readFileSync(P.agentConfPath(home.homeDir, 'a-1'), 'utf8'), after)
})

hCase('★★ 新家不吃这一刀：**先建家再建支** ⇒ 那支拿到的是新缺省（没有手），不是被迁到 full', () => {
  // 这个家的第一次开机时**一支住户都没有**：迁移跑过一次、标记写下、没人可迁。
  const home = scratch('migrate-fresh')
  const first = up.migrateTerminalDefault({ homeDir: home.homeDir, confFile: home.confFile, residents: [] })
  assert.equal(first.length, 1, '空家那一次只打一条总结（没有谁可迁）')
  assert.match(
    fs.readFileSync(home.confFile, 'utf8'),
    new RegExp(`^${up.TERMINAL_DEFAULT_KEY}=${up.TERMINAL_DEFAULT_MIGRATED}$`, 'm'),
    '标记必须写下：否则下一次开机就会把**新建的支**当成老住户',
  )

  // 之后才建的支：它**没有** `terminal=` 那一行 —— 判据若是"它跑过没有"，它第二次开机就会被迁到 full。
  const late = makeResident(home.homeDir, 'a-late')
  const lines = up.migrateTerminalDefault({
    homeDir: home.homeDir,
    confFile: home.confFile,
    residents: residents.residents(home.homeDir),
  })
  assert.deepEqual(lines, [], '标记在场 ⇒ 新建的支吃**新缺省**（不给手），不许被迁到 full')
  assert.equal(
    fs.readFileSync(P.agentConfPath(home.homeDir, late.agentId), 'utf8').includes('terminal='),
    false,
    '它的 agent.conf 里不该多出 terminal 行',
  )
  const hands = handsOf(home.homeDir, home.confFile, residents.residents(home.homeDir)[0])
  assert.equal(hands.terminalSource, 'platform')
  assert.equal(residents.resolveTerminal(hands, { isWindows: true, isLinux: false }).mode, 'none', '新缺省＝不给手')
})

hCase('迁移：老家那一支（档在 home.conf 里）⇒ 只如实说一句，不动它的档', () => {
  const home = scratch('migrate-legacy', ['agent_id=a-old', 'room_id=r-old'])
  const legacy = { agentId: 'a-old', envDir: P.legacyEnvRoot(home.homeDir), slot: 1, roomId: 'r-old', legacy: true }
  const lines = up.migrateTerminalDefault({ homeDir: home.homeDir, confFile: home.confFile, residents: [legacy] })
  assert.match(lines[0], /老家那一支的档写在 home.conf 里/, lines.join('\n'))
  const after = fs.readFileSync(home.confFile, 'utf8')
  assert.ok(!after.includes('terminal=full'), '它的档不许被迁移悄悄改掉（要手是人自己写的一句话）')
  assert.match(after, /^agent_id=a-old$/m, '原本的行照旧')
  assert.match(after, /^room_id=r-old$/m, '原本的行照旧')
  assert.match(after, new RegExp(`^${up.TERMINAL_DEFAULT_KEY}=${up.TERMINAL_DEFAULT_MIGRATED}$`, 'm'))
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '本文件一条用例都没跑（空跑不算过）')
  assert.ok(cases >= 10, `用例数 ${cases} 少于声明的 10`)
})
