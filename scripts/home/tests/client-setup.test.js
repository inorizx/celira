'use strict'
// **源码形态的一键前端**（2026-09-30）：`up` 自己装依赖 + 构建的机械判据。
//
// 这一层钉住四条：
//  1. **谁准备前端**（平台缝）：非 Windows = `up` 自己弄；Windows = 原样（手工命令）
//     —— 红线「不改变 Windows 行为」钉在这里，两条分支都被机械判错；
//  2. **缺什么补什么**：依赖缺 ⇒ 装；产物缺 ⇒ 构；两样都在 ⇒ 一步都不跑；
//  3. **进度与失败如实**：失败读数说清哪一步 / 什么错 / 去哪儿看；失败**不改变** up 的退出码
//     （"前端没起来不影响 Agent 在线"是既有判据——这里钉读数形状，不重复钉那条）；
//  4. **真机跑得动**：`runClientStep` 用真进程验四支（退出 0 / 退出非 0 / 超时 / 起不来）。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const up = require('../up')
const proc = require('../proc')

let cases = 0
function clientCase(name, fn) {
  test(name, async () => {
    // ⚠ 计数在**跑之前**加：这一格量的是"本文件真的执行了用例"（有没有空跑），
    // 不是"过了几条"。从前加在 `await fn()` 之后 ⇒ 一条失败会**连带**把空跑守卫也弄红
    // （2026-10-05 实测：3 条真红 + 1 条假红，一条问题报成两条）。
    cases += 1
    await fn()
  })
}

function scratch(label) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `celira-client-setup-${label}-`))
}

// ---------------------------------------------------------------------------
// §1 平台缝与"缺什么补什么"（纯函数）
// ---------------------------------------------------------------------------

clientCase('谁准备前端：非 Windows = up 自己弄；Windows = 原样（红线：不改变 Windows 行为）', () => {
  assert.equal(up.clientSetupOwner(false), 'up', '非 Windows 上由 up 自己装 + 构')
  assert.equal(up.clientSetupOwner(true), 'user', 'Windows 上保持原样（依赖缺 ⇒ 手工命令）')
})

clientCase('缺什么补什么：依赖缺 ⇒ install；产物缺 ⇒ build；两样都在 ⇒ 一步都不跑', () => {
  assert.deepEqual(up.clientSetupSteps({ hasDeps: true, hasDist: true }), [], '都齐了 ⇒ 一步都不多')
  assert.deepEqual(up.clientSetupSteps({ hasDeps: false, hasDist: true }), ['install'], '只缺依赖')
  assert.deepEqual(up.clientSetupSteps({ hasDeps: true, hasDist: false }), ['build'], '只缺产物')
  assert.deepEqual(
    up.clientSetupSteps({ hasDeps: false, hasDist: false }),
    ['install', 'build'],
    '两样都缺 ⇒ 先装再构（顺序是语义）',
  )
})

clientCase('用时的人话：秒 / 整分钟 / 分加秒（进度行与收尾读数共用一处）', () => {
  assert.equal(up.formatDuration(45_400), '45 秒')
  assert.equal(up.formatDuration(180_000), '3 分钟')
  assert.equal(up.formatDuration(192_000), '3 分 12 秒')
})

// ---------------------------------------------------------------------------
// §2 真机：runClientStep 的四支（真进程，可复核）
// ---------------------------------------------------------------------------

clientCase('真进程：退出 0 ⇒ ok，输出落日志（可复核），用时如实', async () => {
  const dir = scratch('ok')
  const logFile = path.join(dir, 'client.out')
  const done = await up.runClientStep({
    step: 'install',
    command: process.execPath,
    // ⚠ 这一段脚本必须**对 shell 安全**（2026-10-05 修）：Windows 上 `runClientStep` 走
    //   `shell: true`（`npm` 是 `.cmd` 垫片，Node 不许直接 exec `.cmd`）⇒ 命令行交给 **cmd**，
    //   而 cmd 会**吃掉双引号**、把 `>` 当重定向 ⇒ `console.log("装好了")` 到 node 手里成了
    //   `console.log(装好了)` ⇒ `装好了 is not defined` ⇒ **exit 1**（这条用例因此红了很久）。
    //   判据：这段脚本里**不许有引号、`>`、`&`、`|`**。这里用 `String.fromCharCode` 打一个词。
    args: ['-e', 'console.log(String.fromCharCode(67,76,73,69,78,84))'],
    cwd: dir,
    logFile,
    timeoutMs: 15_000,
  })
  assert.equal(done.ok, true, `这一步该成：${JSON.stringify(done)}`)
  assert.equal(done.code, 0)
  assert.ok(done.elapsedMs >= 0)
  const log = fs.readFileSync(logFile, 'utf8')
  assert.ok(log.includes('CLIENT'), '子进程输出要原样落日志（失败时可复核）')
  assert.ok(log.includes('开始（'), '起跑线要写进日志（哪一段是这一步的）')
})

clientCase('真进程：退出码非 0 ⇒ 不 ok，reason 说清是哪一个码（不静默）', async () => {
  const dir = scratch('fail')
  const logFile = path.join(dir, 'client.out')
  const done = await up.runClientStep({
    step: 'build',
    command: process.execPath,
    args: ['-e', 'process.exit(3)'],
    cwd: dir,
    logFile,
    timeoutMs: 15_000,
  })
  assert.equal(done.ok, false)
  assert.equal(done.timedOut, false)
  assert.equal(done.reason, '退出码 3')
})

clientCase('真进程：超时 ⇒ 停掉它、如实报超时（不假装完成），且不留野进程', async () => {
  const dir = scratch('timeout')
  const logFile = path.join(dir, 'client.out')
  const done = await up.runClientStep({
    step: 'install',
    command: process.execPath,
    // ⚠ 同样要**对 shell 安全**：`() => {}` 里的 `>` 会被 cmd 当成重定向 ⇒ 子进程立刻退场，
    //   根本活不到超时那一刻（这条用例从前就是这么红的）。写成 `function(){}` 就没有 `>` 了。
    args: ['-e', 'setTimeout(function(){},60000)'],
    cwd: dir,
    logFile,
    timeoutMs: 700,
  })
  assert.equal(done.ok, false)
  assert.equal(done.timedOut, true)
  assert.match(done.reason, /超时/)
  assert.equal(proc.isAlive(done.pid), false, '超时之后那支进程要真的退场（不留野进程）')
})

clientCase('真进程：命令起不来（不存在）⇒ 不 ok，reason 说"起不来"（不抛）', async () => {
  const dir = scratch('enoent')
  const logFile = path.join(dir, 'client.out')
  const done = await up.runClientStep({
    step: 'install',
    command: 'celira-definitely-not-a-command-xyz',
    args: [],
    cwd: dir,
    logFile,
    timeoutMs: 5_000,
  })
  // ⚠ **按平台判**（2026-10-05 修）：Windows 上 `runClientStep` 走 `shell: true`
  //   （`npm` 是 `.cmd` 垫片）⇒ "命令不存在"由 **cmd** 报出来（它自己 exit 1 ＋ 一行
  //   "不是内部或外部命令"），**拿不到 ENOENT** ⇒ 从前那条 `/起不来/` 在这台机器上
  //   永远不成立。两档都如实钉住：**not ok 是共同的那一条**。
  assert.equal(done.ok, false)
  if (proc.IS_WINDOWS) {
    assert.match(done.reason, /退出码 1/, 'Windows 上这是 cmd 的退出码')
    assert.ok(fs.readFileSync(logFile, 'utf8').trim().length > 0, 'cmd 那句话要落进日志（可复核）')
  } else {
    assert.match(done.reason, /起不来/, '没有 shell 时才是 ENOENT 那一档')
  }

  // 另一档（注入 `spawnFn`）：**起不来时不抛**、如实回一句"起不来" —— 与平台无关的那一半钉在这里。
  const threw = await up.runClientStep({
    step: 'install',
    command: 'x',
    args: [],
    cwd: dir,
    logFile,
    timeoutMs: 1_000,
    spawnFn: () => {
      throw new Error('boom')
    },
  })
  assert.equal(threw.ok, false)
  assert.match(threw.reason, /起不来/)
})

// ---------------------------------------------------------------------------
// §3 失败读数（纯函数 + 真日志尾巴）
// ---------------------------------------------------------------------------

clientCase('失败读数：说清哪一步、什么错、去哪儿看、怎么续；并明说"不影响 Agent 在线"', async () => {
  const dir = scratch('lines')
  const logFile = path.join(dir, 'client.out')
  fs.writeFileSync(logFile, 'npm error code ECONNREFUSED\nnpm error 后面还有几行\n', 'utf8')
  const lines = up.clientStepFailureLines('install', { reason: '退出码 1' }, logFile, '5173')
  const text = lines.join('\n')
  assert.ok(text.includes('npm install 没成功 —— 退出码 1'), `要说清哪一步 + 什么错：\n${text}`)
  assert.ok(text.includes('不影响 Agent 在线'), '要明说不影响 Agent 在线（既有判据）')
  assert.ok(text.includes('ECONNREFUSED'), '要把日志尾巴原样交出来（可复核的原因）')
  assert.ok(text.includes('node home.js up'), '要给续上的路（幂等重跑）')
  assert.ok(text.includes('--port 5173'), '手动那条路要带本家的 client 口')
  assert.ok(up.clientStepFailureLines('build', { reason: '超时（超过 10 分钟 还没结束）' }, logFile, '5173')[0].includes('npm run build'))
})

// ---------------------------------------------------------------------------
// §4 平台缝的源码形状（红线钉在源码里：Windows 那两条手工读数仍在）
// ---------------------------------------------------------------------------

clientCase('红线钉住：Windows 分支的两条手工读数仍在源码里逐字保留（平台缝两条分支都被判错）', () => {
  const source = fs.readFileSync(path.join(__dirname, '..', 'up.js'), 'utf8')
  assert.ok(source.includes('这个 client 还没装依赖'), 'Windows 的依赖缺读数不许改')
  assert.ok(
    source.includes('手动： cd client && npm install && npm run build && node ../scripts/serve_client.mjs --dist client/dist --port'),
    // 2026-10-01 第六刀：手工那条路的**最后一步**换了（源码形态不再走 vite preview）——
    // 前两步（装依赖 / 构建）逐字未动，平台缝本身（谁准备前端）也没动。
    'Windows 的三条手工命令不许改（第三步 = 与 up 同一个静态托管）',
  )
  assert.ok(source.includes('clientSetupOwner()'), '平台缝要在编排里真的被用到')
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '本文件一条用例都没跑（空跑不算过）')
  assert.ok(cases >= 7, `用例数 ${cases} 少于声明的 7`)
})
