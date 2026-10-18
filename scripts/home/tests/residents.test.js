'use strict'
// **住户与家的形状迁移**（2026-10-01 多支刀，承 PROJECT_SHAPE §8.0）。
//
// 这一族钉住"一个家怎么摆"的两件事：
//  ① **住户表**：一支 = `<家>/agents/<支>/`（＋ `agent.conf`）—— 读得出来、序号分得对、
//     还没迁的老家那一支也列得出来（`runtimeResidents`）；
//  ② **老家迁移**（`migrateHomeShape`）：`<家>/env` ⇒ `<家>/agents/<旧 agent_id>/`，
//     **必须幂等**（重跑不重复搬、不丢文件、内容没变就不动 mtime），
//     并且两条纪律（旧那支还在跑 / 远端没有主人凭据）各自**当场停手、一个字都不改**。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const P = require('../paths')
const residents = require('../residents')

let cases = 0
function rCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

/** 一个临时的家（`<root>/home`）：conf 与目录按用例自己造。 */
function scratchHome(label, confLines = []) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), `celira-res-${label}-`))
  const homeDir = path.join(root, 'home')
  fs.mkdirSync(homeDir, { recursive: true })
  const confFile = path.join(homeDir, 'home.conf')
  fs.writeFileSync(confFile, ['listen_addr=127.0.0.1:8080', 'client_port=5173', ...confLines, ''].join('\n'), 'utf8')
  return { root, homeDir, confFile }
}

/** 造一支住户（新布局）：`<家>/agents/<支>/agent.conf` ＋ 一个 identity.json（形状与真实现同款）。 */
function makeResident(homeDir, agentId, { slot = 1, roomId = 'r-1', displayName = '' } = {}) {
  const envDir = P.agentEnvRoot(homeDir, agentId)
  fs.mkdirSync(envDir, { recursive: true })
  fs.writeFileSync(path.join(envDir, 'identity.json'), JSON.stringify({ agent: agentId }), 'utf8')
  residents.writeAgentConf(P.agentConfPath(homeDir, agentId), { agentId, roomId, slot, displayName })
  return envDir
}

/** 造一个**老家**（一个家一支）：`<家>/env` ＋ identity.json（＋ 可选的模型绑定）。 */
function makeLegacyEnv(homeDir, agentId, { binding = false } = {}) {
  const envDir = P.legacyEnvRoot(homeDir)
  fs.mkdirSync(envDir, { recursive: true })
  fs.writeFileSync(path.join(envDir, 'identity.json'), JSON.stringify({ agent: agentId }), 'utf8')
  if (binding) fs.writeFileSync(path.join(envDir, 'model_binding.json'), '{\n  "source": "ms-1"\n}\n', 'utf8')
  return envDir
}

function confMap(file) {
  const out = {}
  for (const line of fs.readFileSync(file, 'utf8').split('\n')) {
    const at = line.indexOf('=')
    if (at <= 0) continue
    out[line.slice(0, at)] = line.slice(at + 1)
  }
  return out
}

// ── ① 路径权威（与壳同一套布局）─────────────────────────────────────────────

rCase('★ 布局：一支一个环境根 `<家>/agents/<支>/`（与壳的 `agent_env_root` 同一套，不许造第三套）', () => {
  const home = '/tmp/some-home'
  assert.equal(P.agentsRoot(home), path.join(home, 'agents'))
  assert.equal(P.agentEnvRoot(home, 'a-1'), path.join(home, 'agents', 'a-1'))
  assert.equal(P.agentConfPath(home, 'a-1'), path.join(home, 'agents', 'a-1', 'agent.conf'))
  assert.equal(P.legacyEnvRoot(home), path.join(home, 'env'))
  // 运行账本**不进环境根**（环境根会跟着 Agent 换机器；pid 是机器侧的事实）。
  assert.equal(P.agentPidFile(home, 'a-1'), path.join(home, 'logs', 'agents', 'a-1', 'host.pid'))
  assert.equal(P.agentHostLog(home, 'a-1'), path.join(home, 'logs', 'agents', 'a-1', 'host.out'))
})

rCase('★ 旧账本：`logs/agents/` 下不属于本家任何一支的目录列得出来；没有孤儿 ⇒ 空（零噪音）', () => {
  // 为什么要有这一问（2026-10-01 第七刀）：一支 Agent 搬家 = 环境根整份搬走，而账本按设计
  // 留在老家 ⇒ `status` 该如实提示"本家没有这一支（可能搬走了）"。判据只认**目录名**。
  const home = scratchHome('orphan-ledger')
  makeResident(home.homeDir, 'a-1', { slot: 1 })
  makeResident(home.homeDir, 'a-2', { slot: 2 })
  // 还没起过的住户：账本目录不存在也算正常 ⇒ 没有孤儿。
  assert.deepEqual(residents.orphanLedgerDirs(home.homeDir, residents.residents(home.homeDir)), [])
  // 搬家后留下的：`logs/agents/<已搬走的支>/`（有账本文件）。
  fs.mkdirSync(P.agentLogDir(home.homeDir, 'a-moved'), { recursive: true })
  fs.writeFileSync(P.agentPidFile(home.homeDir, 'a-moved'), '12345\n', 'utf8')
  assert.deepEqual(residents.orphanLedgerDirs(home.homeDir, residents.residents(home.homeDir)), ['a-moved'])
  // 本家住户的账本目录**不算**孤儿（哪怕此刻没有宿主在跑）。
  fs.mkdirSync(P.agentLogDir(home.homeDir, 'a-1'), { recursive: true })
  assert.deepEqual(residents.orphanLedgerDirs(home.homeDir, residents.residents(home.homeDir)), ['a-moved'])
  // 目录根本不存在 / 读不出来 ⇒ 空数组（不许抛）。
  assert.deepEqual(residents.orphanLedgerDirs(path.join(home.root, 'no-such-home'), []), [])
})

// ── ② 住户表 ────────────────────────────────────────────────────────────────

rCase('住户表：只认 `<家>/agents/<支>/`（有 agent.conf 或 identity.json 才算住户）；按 slot 排序', () => {
  const home = scratchHome('read')
  makeResident(home.homeDir, 'a-two', { slot: 2, roomId: 'r-2' })
  makeResident(home.homeDir, 'a-one', { slot: 1, roomId: 'r-1' })
  // 杂物：没有 agent.conf、也没有 identity.json 的子目录不算住户（半成品不列）。
  fs.mkdirSync(path.join(P.agentsRoot(home.homeDir), 'junk'), { recursive: true })
  fs.mkdirSync(path.join(P.agentsRoot(home.homeDir), '.hidden'), { recursive: true })
  const list = residents.residents(home.homeDir)
  assert.deepEqual(list.map((r) => r.agentId), ['a-one', 'a-two'], '按住户序号排（顺序稳定）')
  assert.equal(list[0].roomId, 'r-1')
  assert.equal(list[0].envDir, P.agentEnvRoot(home.homeDir, 'a-one'))
  assert.equal(list[0].legacy, false)
  assert.equal(list[0].mismatch, null)
})

rCase('住户表：`agent.conf` 里那行与目录名不一致 ⇒ 挂 mismatch 读数（如实说，不静默选边）', () => {
  const home = scratchHome('mismatch')
  makeResident(home.homeDir, 'a-dir')
  fs.writeFileSync(P.agentConfPath(home.homeDir, 'a-dir'), 'agent_id=a-other\nroom_id=r-1\nslot=1\n', 'utf8')
  const list = residents.residents(home.homeDir)
  assert.equal(list[0].agentId, 'a-dir', '身份以**目录名**为准（壳的布局就是这样）')
  assert.equal(list[0].mismatch, 'a-other')
})

rCase('住户序号：`allocateSlot` = 最大 + 1（**绝不回收** —— 回收会让布置键撞上旧记录）', () => {
  assert.equal(residents.allocateSlot([]), 1)
  assert.equal(residents.allocateSlot([{ slot: 1 }]), 2)
  assert.equal(residents.allocateSlot([{ slot: 1 }, { slot: 3 }]), 4)
  // 没写 slot 的住户不参与 max（它自己就是"读不出来"那一档，不猜）。
  assert.equal(residents.allocateSlot([{ slot: null }, { slot: 2 }]), 3)
  assert.equal(residents.allocateSlot([{ slot: null }]), 1)
})

rCase('★ 运行时住户表：新布局 ＋（还没迁的）老家那一支；同一个身份在两边都出现 ⇒ 只列一次（以新布局为准）', () => {
  const home = scratchHome('runtime', ['agent_id=a-old', 'room_id=r-old'])
  makeLegacyEnv(home.homeDir, 'a-old')
  const list = residents.runtimeResidents(home.homeDir, home.confFile)
  assert.deepEqual(list.map((r) => r.agentId), ['a-old'])
  assert.equal(list[0].legacy, true, '还没迁 ⇒ 它的"支级配置"还在 home.conf 里')
  assert.equal(list[0].envDir, P.legacyEnvRoot(home.homeDir))
  assert.equal(residents.residentPidFile(home.homeDir, list[0]), path.join(home.homeDir, 'logs', 'host.pid'))
  assert.equal(residents.residentHostLog(home.homeDir, list[0]), path.join(home.homeDir, 'logs', 'host.out'))

  // 迁到一半（新布局里也有了同一支）⇒ 只列新布局那一份。
  makeResident(home.homeDir, 'a-old', { slot: 1, roomId: 'r-old' })
  const after = residents.runtimeResidents(home.homeDir, home.confFile)
  assert.deepEqual(after.map((r) => r.agentId), ['a-old'])
  assert.equal(after[0].legacy, false)
  assert.equal(after[0].envDir, P.agentEnvRoot(home.homeDir, 'a-old'))
})

rCase('支级配置：写一次、内容没变再写 ⇒ **一个字都不动**（mtime 是读数的一部分）', () => {
  const home = scratchHome('conf-idem')
  const file = P.agentConfPath(home.homeDir, 'a-1')
  const first = residents.writeAgentConf(file, { agentId: 'a-1', roomId: 'r-1', slot: 1 })
  assert.equal(first.changed, true)
  const before = fs.statSync(file).mtimeMs
  const again = residents.writeAgentConf(file, { agentId: 'a-1', roomId: 'r-1', slot: 1 })
  assert.equal(again.changed, false, '内容一样 ⇒ 不写盘')
  assert.equal(fs.statSync(file).mtimeMs, before, 'mtime 不许被无谓改动')
  // 显示名可选：有就写、没有就不写（老家的迁移不带名字）。
  residents.writeAgentConf(file, { agentId: 'a-1', roomId: 'r-1', slot: 1, displayName: '小满' })
  assert.match(fs.readFileSync(file, 'utf8'), /display_name=小满/)
  // 别人的行原样保留（与 home.conf 同一条"不吞别人的行"纪律）。
  fs.appendFileSync(file, 'note=keep-me\n', 'utf8')
  residents.writeAgentConf(file, { agentId: 'a-1', roomId: 'r-2', slot: 1 })
  assert.match(fs.readFileSync(file, 'utf8'), /note=keep-me/)
  assert.match(fs.readFileSync(file, 'utf8'), /room_id=r-2/)
})

// ── ③ 老家迁移 ──────────────────────────────────────────────────────────────

rCase('迁移：全新的家（没有 `<家>/env`、没有那两行）⇒ `none`，一个字都不写（幂等空转）', () => {
  const home = scratchHome('fresh')
  const before = fs.readFileSync(home.confFile, 'utf8')
  const out = residents.migrateHomeShape({
    homeDir: home.homeDir,
    confFile: home.confFile,
    layout: P.detectLayout(),
  })
  assert.equal(out.ok, true)
  assert.equal(out.action, 'none')
  assert.deepEqual(out.lines, [])
  assert.equal(fs.readFileSync(home.confFile, 'utf8'), before)
})

rCase('★ 迁移（有家 id）：`<家>/env` 整份迁成住户 1 的环境根 —— 身份不变，home.conf 那两行让位', () => {
  const home = scratchHome('migrate', ['home_id=H-1', 'agent_id=a-old', 'room_id=r-old'])
  makeLegacyEnv(home.homeDir, 'a-old', { binding: true })
  fs.mkdirSync(path.join(home.homeDir, 'logs'), { recursive: true })
  fs.writeFileSync(path.join(home.homeDir, 'logs', 'host.pid'), '4242\n', 'utf8')
  fs.writeFileSync(path.join(home.homeDir, 'logs', 'host.out'), '[listening]\n', 'utf8')

  const out = residents.migrateHomeShape({
    homeDir: home.homeDir,
    confFile: home.confFile,
    layout: P.detectLayout(),
    hostRunning: () => false,
  })
  assert.equal(out.ok, true, out.refuse ?? '')
  assert.equal(out.action, 'migrated')
  assert.equal(out.agentId, 'a-old')
  // 环境**整份**迁过去（不是复制）：旧位置没了、新位置在，内容还在。
  assert.ok(!fs.existsSync(P.legacyEnvRoot(home.homeDir)), '旧环境根让位')
  const envRoot = P.agentEnvRoot(home.homeDir, 'a-old')
  assert.ok(fs.existsSync(path.join(envRoot, 'identity.json')), '身份记录跟着走')
  assert.equal(
    fs.readFileSync(path.join(envRoot, 'model_binding.json'), 'utf8'),
    '{\n  "source": "ms-1"\n}\n',
    '模型绑定也在环境根里 ⇒ 整份迁（不许丢）',
  )
  // 支级配置：住户 1（键形状**无后缀**那一支）。
  const agentConf = confMap(P.agentConfPath(home.homeDir, 'a-old'))
  assert.equal(agentConf.agent_id, 'a-old')
  assert.equal(agentConf.room_id, 'r-old')
  assert.equal(agentConf.slot, '1')
  // 运行账本跟着搬（rename ⇒ 内容与 mtime 不变）。
  assert.ok(fs.existsSync(P.agentPidFile(home.homeDir, 'a-old')))
  assert.ok(fs.existsSync(P.agentHostLog(home.homeDir, 'a-old')))
  assert.ok(!fs.existsSync(path.join(home.homeDir, 'logs', 'host.pid')))
  // home.conf 只留机器级事实：那两行让位。
  const conf = confMap(home.confFile)
  assert.equal(conf.agent_id, undefined)
  assert.equal(conf.room_id, undefined)
  assert.equal(conf.home_id, 'H-1', '家 id 是机器级事实，留在 home.conf')
  assert.match(out.lines.join('\n'), /住户 1：a-old/)
  assert.match(out.lines.join('\n'), /身份不变/)

  // **重跑**：幂等（旧位置不在 ⇒ none；一个字都不动）。
  const confBefore = fs.readFileSync(home.confFile, 'utf8')
  const confMtime = fs.statSync(P.agentConfPath(home.homeDir, 'a-old')).mtimeMs
  const again = residents.migrateHomeShape({
    homeDir: home.homeDir,
    confFile: home.confFile,
    layout: P.detectLayout(),
    hostRunning: () => false,
  })
  assert.equal(again.ok, true)
  assert.equal(again.action, 'none', '重跑不许再搬一次')
  assert.deepEqual(again.lines, [])
  assert.equal(fs.readFileSync(home.confFile, 'utf8'), confBefore)
  assert.equal(fs.statSync(P.agentConfPath(home.homeDir, 'a-old')).mtimeMs, confMtime, '重跑不许动 agent.conf 的 mtime')
})

rCase('★ 迁移（有家 id）＋ 旧那支**还在跑** ⇒ 拒绝：先 down 再 up，一个字都不改', () => {
  const home = scratchHome('migrate-running', ['home_id=H-1', 'agent_id=a-old', 'room_id=r-old'])
  makeLegacyEnv(home.homeDir, 'a-old')
  const before = fs.readFileSync(home.confFile, 'utf8')
  const out = residents.migrateHomeShape({
    homeDir: home.homeDir,
    confFile: home.confFile,
    layout: P.detectLayout(),
    hostRunning: () => true,
  })
  assert.equal(out.ok, false)
  assert.equal(out.action, 'refuse')
  assert.match(out.refuse, /先 node home\.js down/)
  assert.ok(fs.existsSync(P.legacyEnvRoot(home.homeDir)), '拒绝时旧环境一动不动')
  assert.equal(fs.readFileSync(home.confFile, 'utf8'), before, '拒绝时 home.conf 一个字都不改')
})

rCase('★★ 迁移（**没有**家 id）⇒ 旧环境整份让位 ＋ 那两行让位（世界里会另建一支）', () => {
  const home = scratchHome('aside', ['agent_id=a-old', 'room_id=r-old'])
  makeLegacyEnv(home.homeDir, 'a-old', { binding: true })
  const out = residents.migrateHomeShape({
    homeDir: home.homeDir,
    confFile: home.confFile,
    layout: P.detectLayout(),
    hostRunning: () => false,
    canTouchWorld: true,
  })
  assert.equal(out.ok, true)
  assert.equal(out.action, 'aside')
  assert.ok(out.envAside, '要给出让位到哪')
  // 旧环境**整份让位**（改名，不删）：旧那支的经历还在盘上。
  assert.ok(!fs.existsSync(P.legacyEnvRoot(home.homeDir)))
  assert.ok(fs.existsSync(path.join(out.envAside, 'identity.json')))
  assert.equal(
    fs.readFileSync(path.join(out.envAside, 'model_binding.json'), 'utf8'),
    '{\n  "source": "ms-1"\n}\n',
    '让位的那份**一个字都不动**（模型绑定留给新住户搬运）',
  )
  const conf = confMap(home.confFile)
  assert.equal(conf.agent_id, undefined)
  assert.equal(conf.room_id, undefined)
  assert.equal(conf.home_id, undefined, '这一步**不生成**家 id（生成推迟到第一次真的用键时）')
  const said = out.lines.join('\n')
  assert.match(said, /另建一支 Agent/)
  assert.match(said, /旧那支留着不动/)
  assert.match(said, /旧环境整份让位/)
  // 幂等：再跑一次 ⇒ 旧环境已经不在 ⇒ `none`（不会第二次改名）。
  const again = residents.migrateHomeShape({
    homeDir: home.homeDir,
    confFile: home.confFile,
    layout: P.detectLayout(),
    hostRunning: () => false,
  })
  assert.equal(again.action, 'none')
  assert.ok(fs.existsSync(path.join(out.envAside, 'identity.json')), '让位目录一直在（我们不删它）')
})

rCase('★ 迁移（没有家 id）＋ 旧那支还在跑 / 远端没有主人凭据 ⇒ 两条纪律各自停手，本地一个字都不改', () => {
  const home = scratchHome('aside-refuse', ['agent_id=a-old', 'room_id=r-old'])
  makeLegacyEnv(home.homeDir, 'a-old')
  const before = fs.readFileSync(home.confFile, 'utf8')

  const running = residents.migrateHomeShape({
    homeDir: home.homeDir,
    confFile: home.confFile,
    layout: P.detectLayout(),
    hostRunning: () => true,
    canTouchWorld: true,
  })
  assert.equal(running.ok, false)
  assert.match(running.refuse, /先 node home\.js down/)

  const noToken = residents.migrateHomeShape({
    homeDir: home.homeDir,
    confFile: home.confFile,
    layout: P.detectLayout(),
    hostRunning: () => false,
    canTouchWorld: false,
  })
  assert.equal(noToken.ok, false)
  assert.match(noToken.refuse, /停在动手之前/)
  assert.match(noToken.refuse, /--token/)

  assert.ok(fs.existsSync(P.legacyEnvRoot(home.homeDir)), '两次拒绝之后旧环境都还在')
  assert.equal(fs.readFileSync(home.confFile, 'utf8'), before, '两次拒绝之后 home.conf 一个字都不改')
})

rCase('迁移（迁到一半）：环境已经在 `<家>/agents/` 里，而 home.conf 还留着那两行 ⇒ 收尾（只删那两行）', () => {
  const home = scratchHome('finish', ['home_id=H-1', 'agent_id=a-old', 'room_id=r-old'])
  const envRoot = makeResident(home.homeDir, 'a-old', { slot: 1, roomId: 'r-old' })
  const out = residents.migrateHomeShape({
    homeDir: home.homeDir,
    confFile: home.confFile,
    layout: P.detectLayout(),
    hostRunning: () => false,
  })
  assert.equal(out.ok, true)
  assert.equal(out.action, 'finish')
  const conf = confMap(home.confFile)
  assert.equal(conf.agent_id, undefined)
  assert.equal(conf.room_id, undefined)
  assert.equal(conf.home_id, 'H-1')
  assert.equal(confMap(P.agentConfPath(home.homeDir, 'a-old')).slot, '1', '住户那一份不许被改写')
  assert.ok(fs.existsSync(path.join(envRoot, 'identity.json')))
  assert.match(out.lines.join('\n'), /收尾/)
})

rCase('迁移（空 env）：`<家>/env` 在，但没有身份可归属 ⇒ 什么都不动（不猜它属于谁）', () => {
  const home = scratchHome('empty-env')
  fs.mkdirSync(P.legacyEnvRoot(home.homeDir), { recursive: true })
  const out = residents.migrateHomeShape({
    homeDir: home.homeDir,
    confFile: home.confFile,
    layout: P.detectLayout(),
    hostRunning: () => false,
  })
  assert.equal(out.ok, true)
  assert.equal(out.action, 'none')
  assert.ok(fs.existsSync(P.legacyEnvRoot(home.homeDir)))
  assert.deepEqual(residents.residents(home.homeDir), [], '空 env 不是住户（也没有身份可归属）')
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '本文件一条用例都没跑（空跑不算过）')
  assert.ok(cases >= 12, `用例数 ${cases} 少于声明的 12`)
})
