'use strict'
// `node home.js down` 的**参数纪律**（2026-09-27 修的那个 bug）。
//
// 为什么要单独钉住这一条：`down` 从前**根本不看 argv**，而 `home.js` 里那一行又**没把 argv
// 传下去**（`up` 那一行传了、`down` 那一行没传）⇒ `node home.js down --help`（或任何打错的
// 旗标）被**静默忽略**、按**默认家**跑。真机撞到过：那次家本来就没跑，才侥幸是空操作 ——
// 家要是在跑，敲一次 `down --help` 就把主人的家关了。
// **`up` 对不认识的参数会拒绝，`down` 不会 —— 这个不对称就是 bug 本身**，
// 所以判据要把两边一起钉住。
//
// 判据分三层（少一层就挡不住这个 bug 复发）：
//   ① 纯函数层：`down` 的旗标面是**空集** —— 任何参数都是用法错；
//   ② 一致性层：`up` 拒的，`down` 也拒（`--home` 就在这一条里：指临时的家只有
//      `CELIRA_HOME_DIR` 一条路，两个命令都不收这个旗标）；
//   ③ 端到端层：真的起一个 `node home.js down --help` 子进程。`home.js` 那一行要是
//      又把 argv 漏了，只有这一层会红（①② 照样绿）—— 它才是这个 bug 的回归判据。
//
// ⚠ 端到端那几条**一律给一个不存在的家**（`CELIRA_HOME_DIR` 指到临时目录里一个没有的路径）：
// 万一将来有人把参数校验挪到 `P.homeDir()` 后面，这几条最坏也只是"报了个不存在的家"，
// **不会**真的去停默认家（`D:\celira-home`）的进程 —— 判据自己不许成为那个事故。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { spawnSync } = require('node:child_process')

const P = require('../paths')
const { EXIT } = require('../codes')
const down = require('../down')
const up = require('../up')

let cases = 0
function downCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

/** 一个**不存在**的家（端到端用例专用；见文件头那条 ⚠）。 */
function missingHome() {
  return path.join(os.tmpdir(), `celira-down-args-${process.pid}-${cases}`, 'not-here')
}

/** 真的敲一次 `node home.js <参数>` —— 与主人敲的是同一条路（不是内部函数）。 */
function runHome(args, homeDir) {
  const done = spawnSync(process.execPath, [path.join(P.REPO, 'home.js'), ...args], {
    env: { ...process.env, CELIRA_HOME_DIR: homeDir },
    encoding: 'utf8',
    windowsHide: true,
  })
  return { code: done.status, out: `${done.stdout ?? ''}${done.stderr ?? ''}` }
}

// ── ① 纯函数层：旗标面只有 `--agent <id>` ───────────────────────────────────
downCase('参数纪律：裸跑（一个参数都没有）⇒ 不是用法错 —— 关机那条路一字不变', () => {
  assert.deepEqual(down.parseDownArgs([]), { error: null, agentId: null })
  // 形状也要对：`parseUpArgs` 同款（`error` 非空 = 用法错）⇒ 两个命令读起来一样。
  assert.deepEqual(down.parseDownArgs(undefined), { error: null, agentId: null })
})

downCase('参数纪律：`--agent <id>` 是**唯一**认的旗标（只停这一支；2026-10-01 多支刀）', () => {
  // 依据：住户化之后**起停单位是 Agent**（PROJECT_SHAPE §8.0 的裁决 3）——
  // 一个家住 N 支，就必须能只停一支（不然想重启一支就得把整个家关掉）。
  assert.deepEqual(down.parseDownArgs(['--agent', 'a-1']), { error: null, agentId: 'a-1' })
  // 取值去空白（与 `up --agent` 同一份口径）。
  assert.deepEqual(down.parseDownArgs(['--agent', '  a-1  ']), { error: null, agentId: 'a-1' })
  // 缺值 / 值是另一个旗标 ⇒ 用法错（**绝不**静默当成"没给"⇒ 变成停整个家）。
  for (const argv of [['--agent'], ['--agent', '--help'], ['--agent', '   '], ['--agent', '--agent', 'a-1']]) {
    const bad = down.parseDownArgs(argv)
    assert.ok(bad.error, `${JSON.stringify(argv)} 必须报用法错`)
    assert.ok(bad.error.includes('--agent'), `读数里要点名 --agent：${bad.error}`)
    assert.equal(bad.agentId, null)
  }
  // 认了 `--agent` 也不等于"什么都收"：别的旗标照旧一律拒。
  const mixed = down.parseDownArgs(['--agent', 'a-1', '--force'])
  assert.ok(mixed.error && mixed.error.includes('--force'), `混进来的旗标照样拒：${mixed.error}`)
})

downCase('参数纪律：`down` 的旗标面除了 `--agent` 之外是**空集** —— 别的参数都是用法错', () => {
  // 依据（不是口味）：`home.js` 的 USAGE 里 `down` 那一行只列了 `--agent`；
  // 而手册的退出码表**已经**给 `home_down` 那一列写了 `1 = 用法错`
  // —— 契约早就发布了，只是代码里从来没有过这条路径（那一刀补上）。
  const rejects = [
    ['--help'],
    ['-h'],
    ['--home', 'D:/some-home'],
    ['--serve'],
    ['--force'],
    ['--token'],
    ['--resume', 'host-1-2-3'],
    ['--to', 'D:/backup'],
    ['--'],
    ['start'],
    ['D:/celira-home'],
  ]
  for (const argv of rejects) {
    const bad = down.parseDownArgs(argv)
    assert.ok(bad.error, `${JSON.stringify(argv)} 必须报用法错（down 的旗标面只有 --agent）`)
    // 读数要**带上那个参数**：否则用户看着"不认识的参数："后面空着，只能自己猜。
    assert.ok(bad.error.includes(argv[0]), `错误读数里要带上那个参数：${bad.error}`)
  }
  // 反例（正向对照）：**不是**"凡有参数就拒"这种粗暴判据 —— 拒的是参数本身，
  // 而裸跑这一支必须仍然是好的（上面那条用例）。
  assert.equal(down.parseDownArgs([]).error, null)
})

// ── ② 一致性层：与 `up` 同一份纪律 ─────────────────────────────────────────
downCase('一致性：`--home` 两边**同款拒**（指临时的家只有 CELIRA_HOME_DIR 一条路）', () => {
  const argv = ['--home', 'D:/some-home']
  const forUp = up.parseUpArgs(argv)
  const forDown = down.parseDownArgs(argv)
  assert.ok(forUp.error, '`up` 必须拒 `--home`（真机实测：不认识的参数 --home，退出码 1）')
  assert.ok(forDown.error, '`down` 也必须拒 —— 两个命令的参数纪律必须一致，这正是本刀要修的')
  for (const message of [forUp.error, forDown.error]) {
    assert.ok(message.includes('--home'), `读数里要说是哪个参数：${message}`)
  }
})

downCase('依据是已发布的契约：手册 `home_down` 那一列写着 `1 = 用法错`（这条路径从前没有）', () => {
  // 判据机械：退出码表里 `| 1 |` 那一行的**最后一格**就是 `home_down`。
  const readme = fs.readFileSync(path.join(P.REPO, 'scripts', 'HOME_MANUAL.md'), 'utf8')
  const row = readme.split('\n').find((line) => line.startsWith('| 1 |'))
  assert.ok(row, '手册的退出码表里找不到 `| 1 |` 那一行')
  assert.ok(
    row.trim().endsWith('| 用法错 |'),
    `home_down 那一格应当是"用法错"（改手册就等于改这条已发布的契约）：${row.trim()}`,
  )
})

// ── ③ 端到端层：`home.js` 那一行到底传没传 argv ─────────────────────────────
downCase('端到端：`node home.js down <打错的旗标>` ⇒ 用法错（退出码 1），且**连家都不解析**', () => {
  const home = missingHome()
  for (const flags of [['--help'], ['--nope'], ['--home', 'D:/some-home'], ['start']]) {
    const done = runHome(['down', ...flags], home)
    assert.equal(
      done.code,
      EXIT.USAGE,
      `node home.js down ${flags.join(' ')} 必须是用法错（退出码 1），实际 ${done.code}：\n${done.out}`,
    )
    assert.ok(
      done.out.includes(`[home_down] 不认识的参数：${flags[0]}`),
      `读数要说清是哪个参数不认得：\n${done.out}`,
    )
    assert.ok(done.out.includes('用法: node home.js down'), `用法错要把用法打出来：\n${done.out}`)
    // **这一条才是本 bug 的要害**：参数错的时候，`P.homeDir()` 都不该被走到 ——
    // 从前它会被走完、并且按默认家一路停下去（"没有这个家"这一行就是走到了的证据）。
    assert.ok(!done.out.includes('没有这个家'), `参数错时不该走到"看家在不在"那一步：\n${done.out}`)
    assert.ok(!done.out.includes(home), `参数错时连家目录都不该被解析出来：\n${done.out}`)
  }
})

downCase('端到端（回归）：裸跑 `node home.js down` **照旧走正常流程** —— 参数这一刀没动它', () => {
  const home = missingHome()
  const done = runHome(['down'], home)
  assert.equal(done.code, EXIT.ENV_UNAVAILABLE, `没有这个家 ⇒ 环境不可用（2），实际 ${done.code}：\n${done.out}`)
  assert.ok(done.out.includes('没有这个家'), `裸跑要照旧走到"看家在不在"那一步：\n${done.out}`)
})

downCase('端到端：`down --agent <id>` 被认下（走到看家那一步），`--agent` 缺值照旧是用法错', () => {
  const home = missingHome()
  // 认下了 ⇒ 流程继续（这个家不存在 ⇒ 环境不可用）—— 参数**没有**被静默忽略。
  const ok = runHome(['down', '--agent', 'a-1'], home)
  assert.equal(ok.code, EXIT.ENV_UNAVAILABLE, `实际 ${ok.code}：\n${ok.out}`)
  assert.ok(ok.out.includes('没有这个家'), `认下 --agent 之后要照常往下走：\n${ok.out}`)
  // 缺值 ⇒ 用法错，而且**连家都不解析**（与其它参数错同一条纪律）。
  const bad = runHome(['down', '--agent'], home)
  assert.equal(bad.code, EXIT.USAGE, `实际 ${bad.code}：\n${bad.out}`)
  assert.ok(bad.out.includes('`--agent` 后面要跟一个世界里的 Agent id'), `读数：\n${bad.out}`)
  assert.ok(!bad.out.includes('没有这个家'), `参数错时不该走到"看家在不在"那一步：\n${bad.out}`)
})

downCase('端到端（回归）：`node home.js up --help` 照旧被拒 —— 别把 `up` 弄坏', () => {
  const done = runHome(['up', '--help'], missingHome())
  assert.equal(done.code, EXIT.USAGE, `up --help 必须是用法错，实际 ${done.code}：\n${done.out}`)
  assert.ok(done.out.includes('[home_up] 不认识的参数：--help'), `读数：\n${done.out}`)
})

downCase('现状不动：`node home.js --help`（没有子命令）仍然落进 up 的参数解析并报用法错', () => {
  // 这是**现状**，本刀特意不动它（`home.js` 里"不带子命令 = up，老写法也认"那条兼容路）。
  const done = runHome(['--help'], missingHome())
  assert.equal(done.code, EXIT.USAGE, `实际 ${done.code}：\n${done.out}`)
  assert.ok(done.out.includes('[home_up] 不认识的参数：--help'), `读数：\n${done.out}`)
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '本文件一条用例都没跑（空跑不算过）')
  assert.ok(cases >= 9, `用例数 ${cases} 少于声明的 9`)
})
