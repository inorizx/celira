'use strict'
// **终端三档：none | sandbox | full**（2026-10-23 沙箱刀，承 `docs/runtime/terminal/SANDBOX_PROVIDER_DECISIONS.md`
// §3/§5/§8）。这里钉住 home 侧的那一半契约（provider 探测在 runtime，不在 home）：
//
//  ① 配置解析：`terminal=sandbox` 必须被**认出来**（不许被当成 none），优先级仍是
//     **支级 > 家级 > 平台默认**，三档各自都能覆盖对方；
//  ② 启动装配：Linux 上 `sandbox` **原样**进 argv（`--terminal sandbox`）；Windows / 没有臂的平台
//     **一个参数都不多**（宿主缺省就是 none），尤其**绝不落成 full**（那是把「关起来」换成「整机全权」）；
//  ③ 读数：`status` / 启动读数里 sandbox **不是"没有手"**，并说清"由 runtime 探测 provider"；
//     不可用时说清**是哪一档**、**为什么**（与 full 的"没有终端臂"分开说）；
//  ④ 迁移：一次性迁移不许把 `terminal=sandbox` 补成 `terminal=full`（窄档不许被悄悄放宽）；
//  ⑤ 回归：`full` / `none` / 平台默认的每一句读数与每一个字节都不变。
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
function sCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

/** 一个临时的家：家级 `home.conf` ＋ 可选的住户。 */
function scratch(label, confLines = []) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), `celira-sandbox-${label}-`))
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

const PLATFORMS = {
  linux: { isWindows: false, isLinux: true },
  windows: { isWindows: true, isLinux: false },
  other: { isWindows: false, isLinux: false },
}

function argvOf(terminal, platform) {
  return up.buildHostArgs({
    envDir: '/tmp/whatever-env',
    agentId: 'a-1',
    materialsDir: '/tmp/m',
    listen: '127.0.0.1:1',
    terminal,
    ...platform,
  })
}

// ── ① 配置解析：三档都认得，优先级不变 ─────────────────────────────────────

sCase('★ 支级 sandbox 压过家级 full（支级 > 家级，且 sandbox 不被读成 none）', () => {
  const home = scratch('prio-own', ['terminal=full'])
  const resident = makeResident(home.homeDir, 'a-1', { lines: ['terminal=sandbox'] })
  const hands = handsOf(home.homeDir, home.confFile, resident)
  assert.equal(hands.terminal, 'sandbox', '支级写的就是它自己的档（覆盖家级）')
  assert.equal(hands.terminalSource, 'resident')

  const resolved = residents.resolveTerminal(hands, PLATFORMS.linux)
  assert.equal(resolved.mode, 'sandbox')
  assert.equal(resolved.requested, 'sandbox')
  assert.equal(resolved.unavailable, false)
  assert.equal(resolved.unavailableReason, null)
})

sCase('★ 家级 sandbox 当默认：支级没写就跟家走；支级 none / full 都压得过它', () => {
  const home = scratch('prio-home', ['terminal=sandbox'])
  const carried = makeResident(home.homeDir, 'a-1')
  const handsCarried = handsOf(home.homeDir, home.confFile, carried)
  assert.equal(handsCarried.terminal, 'sandbox')
  assert.equal(handsCarried.terminalSource, 'home', '支级没写 ⇒ 跟家走（来源要说清）')
  assert.equal(residents.resolveTerminal(handsCarried, PLATFORMS.linux).mode, 'sandbox')

  const denied = makeResident(home.homeDir, 'a-2', { slot: 2, roomId: 'r-2', lines: ['terminal=none'] })
  const handsDenied = handsOf(home.homeDir, home.confFile, denied)
  assert.equal(handsDenied.terminal, 'none', '支级 none 压过家级 sandbox（"这一支明确不要"）')
  assert.equal(handsDenied.terminalSource, 'resident')

  const wider = makeResident(home.homeDir, 'a-3', { slot: 3, roomId: 'r-3', lines: ['terminal=full'] })
  const handsWider = handsOf(home.homeDir, home.confFile, wider)
  assert.equal(handsWider.terminal, 'full', '支级 full 也压得过家级 sandbox（支级唯一权威）')
  assert.equal(residents.resolveTerminal(handsWider, PLATFORMS.linux).mode, 'full')
})

sCase('★ 支级 sandbox 压过家级 none；上档写成大写/带空格也认得', () => {
  const home = scratch('prio-none', ['terminal=none'])
  const resident = makeResident(home.homeDir, 'a-1', { lines: ['terminal= SandBox '] })
  const hands = handsOf(home.homeDir, home.confFile, resident)
  assert.equal(hands.terminal, 'sandbox', '大小写/空格归一后仍认得 sandbox')
  assert.equal(hands.terminalSource, 'resident')
  assert.equal(residents.resolveTerminal(hands, PLATFORMS.linux).mode, 'sandbox')
})

sCase('★ 老家那一支（还没迁）：档从 home.conf 读，sandbox 照样认', () => {
  const home = scratch('legacy', ['terminal=sandbox', 'agent_id=a-old', 'room_id=r-old'])
  const legacy = { agentId: 'a-old', envDir: P.legacyEnvRoot(home.homeDir), slot: 1, roomId: 'r-old', legacy: true }
  const hands = handsOf(home.homeDir, home.confFile, legacy)
  assert.equal(hands.terminal, 'sandbox')
  assert.equal(hands.terminalSource, 'home')
  assert.equal(residents.resolveTerminal(hands, PLATFORMS.linux).mode, 'sandbox')
})

sCase('★ 认不出的值仍按平台默认（不编一档出来）；三档清单只有这三个', () => {
  assert.deepEqual(residents.TERMINAL_TIERS.slice().sort(), ['full', 'none', 'sandbox'])
  const home = scratch('unknown', ['terminal=turbo'])
  const resident = makeResident(home.homeDir, 'a-1')
  const hands = handsOf(home.homeDir, home.confFile, resident)
  assert.equal(hands.terminal, '', '认不出的值不冒充任何档')
  assert.equal(hands.terminalSource, 'platform')
  const resolved = residents.resolveTerminal(hands, PLATFORMS.linux)
  assert.equal(resolved.mode, 'none')
  assert.equal(resolved.requested, 'none')
  assert.equal(resolved.unavailable, false, '"没写"不是"装不进来"（两件事）')
})

// ── ② 启动装配：sandbox 原样进 argv；Windows 不降级 ─────────────────────────

sCase('★ Linux 上 sandbox 原样进 argv：`--terminal sandbox` 成对、只一次、位置不变', () => {
  const args = argvOf('sandbox', PLATFORMS.linux)
  const at = args.indexOf('--terminal')
  assert.ok(at >= 0, `Linux 上该带 --terminal：${args.join(' ')}`)
  assert.equal(args[at + 1], 'sandbox', '原样传 sandbox（不换成 full / none）')
  assert.equal(args.filter((arg) => arg === '--terminal').length, 1, '只该出现一次')
  assert.equal(args.indexOf('--max-turns') + 2, at, '位置与从前一致（max-turns 之后、mcp 之前）')
  assert.ok(!args.includes('full'), `sandbox 的 argv 里不许出现 full：${args.join(' ')}`)
})

sCase('★ Windows 上 sandbox 不进 argv，且**绝不**落成 full（不静默降级）', () => {
  const args = argvOf('sandbox', PLATFORMS.windows)
  assert.ok(!args.includes('--terminal'), `Windows 上不该带这个旗标：${args.join(' ')}`)
  assert.ok(!args.includes('full'), `更不许换成 full：${args.join(' ')}`)
  assert.ok(!args.includes('sandbox'), `也不许把参数留在 argv 里：${args.join(' ')}`)
})

sCase('★ 没有终端/沙箱臂的平台上 sandbox 也不进 argv；full 的旧行为一字不变', () => {
  const other = argvOf('sandbox', PLATFORMS.other)
  assert.ok(!other.includes('--terminal'), `别的平台上不带：${other.join(' ')}`)
  assert.ok(!other.includes('full'))

  // 回归：full 仍然只给 Windows / Linux。
  for (const platform of [PLATFORMS.linux, PLATFORMS.windows]) {
    const args = argvOf('full', platform)
    const at = args.indexOf('--terminal')
    assert.ok(at >= 0, `有臂的平台 + full ⇒ 带 --terminal：${args.join(' ')}`)
    assert.equal(args[at + 1], 'full')
  }
  assert.ok(!argvOf('full', PLATFORMS.other).includes('--terminal'), '没臂的平台：声明了 full 也不带')

  // 回归：none / 缺省 ⇒ 一个参数都不多（宿主的缺省档就是 none）。
  for (const terminal of ['none', undefined]) {
    const args = argvOf(terminal, PLATFORMS.linux)
    assert.ok(!args.includes('--terminal'), `${terminal} ⇒ 不带 --terminal：${args.join(' ')}`)
  }
})

// ── ③ 读数：sandbox 不是"没有手" ───────────────────────────────────────────

sCase('★ 启动读数：sandbox 说清来源与"由 runtime 探测"（不是"没有手"）', () => {
  const home = scratch('notice-sandbox', [])
  const own = makeResident(home.homeDir, 'a-1', { lines: ['terminal=sandbox'] })
  const ownHands = handsOf(home.homeDir, home.confFile, own)
  const linesOwn = up
    .handNotices({
      who: '住户 a-1：',
      hands: ownHands,
      terminal: residents.resolveTerminal(ownHands, PLATFORMS.linux),
      platform: PLATFORMS.linux,
    })
    .join('\n')
  assert.match(linesOwn, /住户 a-1：终端档 sandbox（来自它自己的 agent\.conf）/, linesOwn)
  assert.match(linesOwn, /由 runtime 探测 provider/, linesOwn)
  assert.match(linesOwn, /不降级成 full/, linesOwn)
  assert.ok(!linesOwn.includes('没有「终端」这只手'), `sandbox 不是"没有手"：${linesOwn}`)

  // 家级声明：来源如实报"家级 home.conf"。
  const home2 = scratch('notice-home', ['terminal=sandbox'])
  const carried = makeResident(home2.homeDir, 'a-1')
  const carriedHands = handsOf(home2.homeDir, home2.confFile, carried)
  const linesHome = up
    .handNotices({ who: '', hands: carriedHands, terminal: residents.resolveTerminal(carriedHands, PLATFORMS.linux), platform: PLATFORMS.linux })
    .join('\n')
  assert.match(linesHome, /终端档 sandbox（来自家级 home\.conf）/, linesHome)
  assert.ok(!linesHome.includes('住户 '), '单支的家不该多出"住户"前缀（与从前一致）')
})

sCase('★ Windows 上 sandbox：明确说不可用、不退成 full（与 full 的不可用分开说）', () => {
  const home = scratch('notice-win', [])
  const resident = makeResident(home.homeDir, 'a-1', { lines: ['terminal=sandbox'] })
  const hands = handsOf(home.homeDir, home.confFile, resident)
  const resolved = residents.resolveTerminal(hands, PLATFORMS.windows)
  assert.equal(resolved.mode, 'none', '这台机器上给不了 ⇒ 交给宿主的档是 none')
  assert.equal(resolved.requested, 'sandbox', '但"声明的是哪一档"必须留着（读数要分开说）')
  assert.equal(resolved.unavailable, true)
  assert.equal(resolved.unavailableReason, 'sandbox-provider-absent')

  const said = up.handNotices({ hands, terminal: resolved, platform: PLATFORMS.windows }).join('\n')
  assert.match(said, /声明了终端档 sandbox，但这台机器上没有沙箱 provider/, said)
  assert.match(said, /不会退成 full/, said)
  assert.match(said, /这次不带/, said)
  assert.ok(!said.includes('没有终端臂'), `sandbox 的不可用不是"没有终端臂"：${said}`)
  assert.ok(!said.includes('终端档 full'), `不许把 sandbox 说成 full：${said}`)
})

sCase('★ 回归：full 的不可用 / full 的声明 / none 的三种来路 / 平台默认，逐句不变', () => {
  const hands = (terminal, source) => ({ mcp: [], mcpSource: 'home', terminal, terminalSource: source })

  const fullNoArm = up
    .handNotices({
      hands: hands('full', 'resident'),
      terminal: residents.resolveTerminal(hands('full', 'resident'), PLATFORMS.other),
      platform: PLATFORMS.other,
    })
    .join('\n')
  assert.equal(
    fullNoArm,
    '[terminal] 声明了终端档 full，但这台机器上没有终端臂（有实现的平台：Windows、Linux）—— 这次不带；不静默降级',
  )

  const fullOwn = up
    .handNotices({
      who: '住户 a-1：',
      hands: hands('full', 'resident'),
      terminal: residents.resolveTerminal(hands('full', 'resident'), PLATFORMS.linux),
      platform: PLATFORMS.linux,
    })
    .join('\n')
  assert.equal(fullOwn, '[terminal] 住户 a-1：终端档 full（来自它自己的 agent.conf）')

  const noneOwn = up
    .handNotices({
      who: '住户 a-1：',
      hands: hands('none', 'resident'),
      terminal: residents.resolveTerminal(hands('none', 'resident'), PLATFORMS.linux),
      platform: PLATFORMS.linux,
    })
    .join('\n')
  assert.equal(noneOwn, '[terminal] 住户 a-1：这一支没有「终端」这只手（agent.conf 里写了 terminal=none）')

  const noneHome = up
    .handNotices({
      hands: hands('none', 'home'),
      terminal: residents.resolveTerminal(hands('none', 'home'), PLATFORMS.linux),
      platform: PLATFORMS.linux,
    })
    .join('\n')
  assert.equal(noneHome, '[terminal] 这一支没有「终端」这只手（家级写了 terminal=none）')

  const platformDefault = up
    .handNotices({
      hands: hands('', 'platform'),
      terminal: residents.resolveTerminal(hands('', 'platform'), PLATFORMS.linux),
      platform: PLATFORMS.linux,
    })
    .join('\n')
  assert.equal(
    platformDefault,
    '[terminal] 平台默认不给「终端」这只手（2026-10-14 起）；要手就在它自己的 agent.conf 里写 terminal=full',
  )
})

sCase('★ `status` 那一格：sandbox 有档、说清"由 runtime 探测"；不可用时不被说成"没有"', () => {
  const hands = { mcp: [], mcpSource: 'home', terminal: 'sandbox', terminalSource: 'resident' }
  const onLinux = status.terminalLabel({ hands }, PLATFORMS.linux)
  assert.match(onLinux, /^sandbox（它自己的声明；/, onLinux)
  assert.match(onLinux, /由 runtime 探测 provider/, onLinux)
  assert.match(onLinux, /不降级成 full/, onLinux)
  assert.ok(!onLinux.includes('没有'), `sandbox 不是"没有手"：${onLinux}`)

  const fromHome = status.terminalLabel({ hands: { ...hands, terminalSource: 'home' } }, PLATFORMS.linux)
  assert.match(fromHome, /^sandbox（家级声明；/, fromHome)

  const onWindows = status.terminalLabel({ hands }, PLATFORMS.windows)
  assert.match(onWindows, /声明了 \*\*sandbox\*\*，但这台机器上没有沙箱 provider/, onWindows)
  assert.match(onWindows, /也不降级成 full/, onWindows)

  // 回归：full / none / 读不出来 三格一字不变。
  const full = status.terminalLabel({ hands: { ...hands, terminal: 'full' } }, PLATFORMS.linux)
  assert.equal(full, '在场')
  const none = status.terminalLabel({ hands: { ...hands, terminal: 'none' } }, PLATFORMS.linux)
  assert.equal(none, '没有（它自己的声明）')
  const platformDefault = status.terminalLabel({ hands: { ...hands, terminal: '', terminalSource: 'platform' } }, PLATFORMS.linux)
  assert.equal(platformDefault, '没有（平台默认；要手就在它自己的 agent.conf 里写 terminal=full）')
  assert.equal(status.terminalLabel({ hands: null }, PLATFORMS.linux), '读不出来')
})

sCase('★ 状态口载荷：sandbox 原样是 `mode`（形状仍是冻结的三键，不加字段）', async () => {
  const home = scratch('payload', ['home_id=H-P'])
  const resident = makeResident(home.homeDir, 'a-1', { lines: ['terminal=sandbox'] })
  const view = await statusServer.buildResidentView({
    resident,
    ctx: {
      layout: { statusExe: '/nonexistent/celira-agent-status' },
      homeDir: home.homeDir,
      confFile: home.confFile,
      proc: {},
    },
  })
  const terminal = view.hands.terminal
  assert.equal(terminal.mode, 'sandbox', 'Linux 上真跑的这支是 sandbox 档（不是 none / 不是 full）')
  assert.equal(terminal.source, 'resident')
  assert.equal(terminal.unavailable, false)
  assert.deepEqual(
    Object.keys(terminal).sort(),
    ['mode', 'source', 'unavailable'],
    '载荷形状是冻结的：加 `requested` 就是改一份没有消费者的契约（见 status-server 的注释）',
  )
})

// ── ④ 迁移：窄档不许被放宽成 full ──────────────────────────────────────────

sCase('★ 一次性迁移不动 sandbox：不许把 `terminal=sandbox` 补成 `terminal=full`', () => {
  const home = scratch('migrate', [])
  const resident = makeResident(home.homeDir, 'a-1', { lines: ['terminal=sandbox'] })
  const lines = up.migrateTerminalDefault({ homeDir: home.homeDir, confFile: home.confFile, residents: [resident] })

  const text = fs.readFileSync(P.agentConfPath(home.homeDir, 'a-1'), 'utf8')
  assert.ok(!/^terminal=full$/m.test(text), `sandbox 那一行不许被改成 full：\n${text}`)
  assert.match(text, /^terminal=sandbox$/m, '声明原样留着')
  assert.equal(
    text.match(/^terminal=/gm).length,
    1,
    `不许再补一行 terminal=：\n${text}`,
  )
  assert.equal(
    handsOf(home.homeDir, home.confFile, resident).terminal,
    'sandbox',
    '迁移之后这一支仍是 sandbox（来源仍是它自己的声明）',
  )
  // 迁移本身**跑过了**（家级标记写了）—— 别用"提前 return"换一个假绿。
  const conf = fs.readFileSync(home.confFile, 'utf8')
  assert.match(conf, /terminal_default=migrated-2026-10-14/, `迁移该照常记账：\n${conf}`)
  assert.ok(
    !lines.some((line) => line.includes('已将 terminal=full 显式写进它自己的 agent.conf')),
    `迁移读数不许声称给 sandbox 那一支补了 full：${lines.join('\n')}`,
  )
})

sCase('★ 回归：迁移仍给"没写档"的支补 full（旧缺省下它有手）', () => {
  const home = scratch('migrate-legacy-default', [])
  const untouched = makeResident(home.homeDir, 'a-1')
  up.migrateTerminalDefault({ homeDir: home.homeDir, confFile: home.confFile, residents: [untouched] })
  const text = fs.readFileSync(P.agentConfPath(home.homeDir, 'a-1'), 'utf8')
  assert.match(text, /^terminal=full$/m, '没表过态的支照旧补 full（老住户不掉东西）')
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases >= 12, `用例数 ${cases} 少于声明的 12`)
})
