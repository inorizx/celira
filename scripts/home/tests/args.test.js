'use strict'
// 开机编排的两样"形状"：**参数解析**与**交给子进程的命令行**。
//
// 为什么这两样值得各写几条：参数顺序就是语义（Windows 与 Linux 上 `--mcp` 必须夹在
// `--terminal full` 与 `--im-endpoint` 之间、没有终端臂的平台上根本没有终端那一对；
// 声明 0 台时一个参数都不许多），
// 而"旗标跨错一步"这类 bug 在只跑真活的时候极难发现（跑起来才发现 Agent 读不懂）。
// 平台分支一律**显式传参**（`isWindows`）钉住：本机是哪个平台都不影响两条分支各自被验。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const path = require('node:path')
const fs = require('node:fs')
const os = require('node:os')

const up = require('../up')
const residents = require('../residents')

let cases = 0
function argCase(name, fn) {
  test(name, () => {
    fn()
    cases += 1
  })
}

// ⚠ **这个夹具显式声明 `terminal: 'full'`**（2026-10-14）：下面这些用例量的是**给了手那一档**的
// argv 形状（`--mcp` 夹在 `--terminal full` 与 `--im-endpoint` 之间、0 台时一个参数都不多）。
// **缺省那一档（不给手）另有其钉**：`hands.test.js` 的 `withTerminal(undefined, …) === false`
// 与 `args.test.js` 末尾那条"平台默认不给手"的读数——两档各自被验，谁也不替谁说话。
const base = {
  envDir: 'D:\\celira-home\\env',
  agentId: 'agent-1',
  materialsDir: 'D:\\celira-home\\materials',
  listen: '127.0.0.1:8080',
  terminal: 'full',
}

argCase('起 Agent 的命令行（Windows）：声明 0 台 ⇒ 一个参数都不多（这段是与旧版逐字节一致的判据）', () => {
  assert.deepEqual(up.buildHostArgs({ ...base, declarations: [], isWindows: true }), [
    '--root', base.envDir,
    '--agent', base.agentId,
    '--materials', base.materialsDir,
    '--max-turns', '300',
    '--terminal', 'full',
    '--im-endpoint', 'http://127.0.0.1:8080',
  ])
  assert.ok(!up.buildHostArgs({ ...base, declarations: [], isWindows: true }).includes('--mcp'))
})

// **支级资源上界**（2026-10-05）：`max_turns` 原先由宿主写死（与壳的 `build_host_args` 各写一份）
// ⇒ 用户改不了、同一事实两个写者。现在唯一权威是 `agent.conf` 这一份文件，两个宿主只是读者。
// 这三条钉住：文件里的值进 argv、非法值**原样交出**（不在这一层兜底）、缺省是 300。
argCase('起 Agent 的命令行：`agent.conf` 的 `max_turns` 进 argv（缺省 300、非法值原样交出不兜底）', () => {
  const args = (extra) => up.buildHostArgs({ ...base, declarations: [], isWindows: true, ...extra })
  const value = (extra) => args(extra)[args(extra).indexOf('--max-turns') + 1]

  // 缺省（不给这个入参）⇒ 内置 300。
  assert.equal(value({}), '300')
  // 给了就照给。
  assert.equal(value({ maxTurns: '12' }), '12')
  // **非法值不许在这一层被改写成缺省** —— 校验的权威是宿主自己的旗标解析（非整数、0 都被拒，
  // 且有明确失败文本）；这里兜底就是"配置的值 ≠ 运行的值"的假绿。
  assert.equal(value({ maxTurns: '零' }), '零')
  assert.equal(value({ maxTurns: '0' }), '0')
})

argCase('读支级资源上界：`agent.conf` 有 `max_turns=` 就读出来，没有就是不在场（`null`）', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'celira-budget-'))
  const file = path.join(dir, 'agent.conf')
  try {
    // 文件不在 ⇒ 不在场（调用方给内置缺省）。
    assert.deepEqual(residents.readAgentBudget(file), { maxTurns: null })
    // 有键 ⇒ 读出来（值原样，不 trim 成别的意思）。
    fs.writeFileSync(file, 'agent_id=agent-1\nmax_turns=12\n')
    assert.deepEqual(residents.readAgentBudget(file), { maxTurns: '12' })
    // 空值 ⇒ 不在场（形状允许 `max_turns=`）。
    fs.writeFileSync(file, 'max_turns=\n')
    assert.deepEqual(residents.readAgentBudget(file), { maxTurns: null })
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
})

argCase('起 Agent 的命令行（Windows）：声明 2 台 ⇒ `--mcp` 在 `--terminal full` 之后、`--im-endpoint` 之前', () => {
  assert.deepEqual(
    up.buildHostArgs({
      ...base,
      declarations: ['browser=stdio:npx a', 'notes=stdio:cmd,read-only'],
      isWindows: true,
    }),
    [
      '--root', base.envDir,
      '--agent', base.agentId,
      '--materials', base.materialsDir,
      '--max-turns', '300',
      '--terminal', 'full',
      '--mcp', 'browser=stdio:npx a',
      '--mcp', 'notes=stdio:cmd,read-only',
      '--im-endpoint', 'http://127.0.0.1:8080',
    ],
  )
})

argCase('起 Agent 的命令行（Linux）：**给** `--terminal full` —— 这台机器上终端这只手在场（bash 臂）', () => {
  const args = up.buildHostArgs({ ...base, declarations: [], isWindows: false, isLinux: true })
  assert.deepEqual(args, [
    '--root', base.envDir,
    '--agent', base.agentId,
    '--materials', base.materialsDir,
    '--max-turns', '300',
    '--terminal', 'full',
    '--im-endpoint', 'http://127.0.0.1:8080',
  ])
  // 其余顺序一个字都不许动：`--mcp` / 清单 / 身份都照旧（终端那一对与 Windows 同一位置）。
  assert.deepEqual(
    up.buildHostArgs({
      ...base,
      declarations: ['a=stdio:x'],
      identity: { id: 'i-1', displayName: '名' },
      modelSourcesFile: '/h/model_sources.json',
      isWindows: false,
      isLinux: true,
    }),
    [
      '--root', base.envDir,
      '--agent', base.agentId,
      '--materials', base.materialsDir,
      '--max-turns', '300',
      '--terminal', 'full',
      '--mcp', 'a=stdio:x',
      '--im-endpoint', 'http://127.0.0.1:8080',
      '--model-sources', '/h/model_sources.json',
      '--identity-agent', 'i-1',
      '--identity-name', '名',
    ],
  )
})

argCase('起 Agent 的命令行（没有终端臂的平台）：**不给** `--terminal` —— 如实，不降级', () => {
  const args = up.buildHostArgs({ ...base, declarations: [], isWindows: false, isLinux: false })
  assert.deepEqual(args, [
    '--root', base.envDir,
    '--agent', base.agentId,
    '--materials', base.materialsDir,
    '--max-turns', '300',
    '--im-endpoint', 'http://127.0.0.1:8080',
  ])
  assert.ok(!args.includes('--terminal'), '没有臂的平台上不许传终端档位（那一档是有意的 fail-closed）')
})

argCase('终端读数：平台默认**不给手**（2026-10-14 起）—— 与平台无关，且说清怎么要手', () => {
  // ⚠ 这一条从前量的是"Linux 说手在场 / Windows 不宣告"。主人 2026-10-14 裁决之后，
  //   **平台默认是不给手**：这行读数因此与平台无关，它回答的是"没人声明过的时候给不给"。
  const line = up.terminalNotice()
  assert.ok(line && line.startsWith('[terminal]'), `要有一条稳定读数：${line}`)
  assert.match(line, /平台默认不给「终端」这只手/)
  assert.match(line, /2026-10-14 起/, '缺省变更的日期要看得见')
  assert.match(line, /terminal=full/, '要说清怎么要手（否则用户只看到"没有"）')
  assert.equal(up.terminalNotice(), line, '同一个事实只有一个来源：不许按平台给出不同的说法')
})

argCase('起 Agent 的命令行（Linux）：**给** `--mcp` —— stdio 通道这只手在这台机器上在场（bash 臂）', () => {
  const args = up.buildHostArgs({
    ...base,
    declarations: ['fixture=stdio:/data/celira/agent/target/release/mcp_fixture_server', 'notes=stdio:cmd,read-only'],
    isWindows: false,
    isLinux: true,
  })
  assert.deepEqual(args, [
    '--root', base.envDir,
    '--agent', base.agentId,
    '--materials', base.materialsDir,
    '--max-turns', '300',
    '--terminal', 'full',
    '--mcp', 'fixture=stdio:/data/celira/agent/target/release/mcp_fixture_server',
    '--mcp', 'notes=stdio:cmd,read-only',
    '--im-endpoint', 'http://127.0.0.1:8080',
  ])
})

argCase('起 Agent 的命令行（没有 MCP 臂的平台）：**不给** `--mcp` —— 一行 mcp_* 不许把家弄停摆', () => {
  const args = up.buildHostArgs({
    ...base,
    declarations: ['fixture=stdio:x', 'notes=stdio:y,read-only'],
    isWindows: false,
    isLinux: false,
  })
  assert.deepEqual(args, [
    '--root', base.envDir,
    '--agent', base.agentId,
    '--materials', base.materialsDir,
    '--max-turns', '300',
    '--im-endpoint', 'http://127.0.0.1:8080',
  ])
  assert.ok(
    !args.includes('--mcp'),
    '没有臂的平台上不许传 --mcp（入口会拒绝开启这一档，传它等于让 Agent 每次开机都被拒）',
  )
})

argCase('MCP 读数：Windows / Linux 上不出现；没有臂的平台如实说「没有通道、这次不带」', () => {
  assert.equal(up.mcpNotice(true, false), null, 'Windows 上通道在场，不需要这一行')
  assert.equal(up.mcpNotice(false, true), null, 'Linux 上通道在场，不需要这一行')

  const bare = up.mcpNotice(false, false, 2, 'fixture,notes')
  assert.ok(bare && bare.startsWith('[mcp]'), `没有臂的平台要有一条稳定读数：${bare}`)
  assert.match(bare, /没有 stdio MCP 通道/)
  assert.match(bare, /有实现的平台：Windows、Linux/)
  assert.match(bare, /声明了 2 台（fixture,notes）/)
  assert.match(bare, /不静默降级/)
})

argCase('起 Agent 的命令行（Windows）：给了清单 ⇒ `--model-sources` 紧随 `--im-endpoint`，在身份两件之前', () => {
  assert.deepEqual(
    up.buildHostArgs({
      ...base,
      declarations: [],
      identity: { id: 'agent-1', displayName: 'Agent' },
      modelSourcesFile: 'D:\\celira-home\\model_sources.json',
      isWindows: true,
    }),
    [
      '--root', base.envDir,
      '--agent', base.agentId,
      '--materials', base.materialsDir,
      '--max-turns', '300',
      '--terminal', 'full',
      '--im-endpoint', 'http://127.0.0.1:8080',
      '--model-sources', 'D:\\celira-home\\model_sources.json',
      '--identity-agent', 'agent-1',
      '--identity-name', 'Agent',
    ],
  )
  // 回归判据：不给清单时**一个参数都不多**（与上一条"声明 0 台"同一条纪律）。
  assert.ok(!up.buildHostArgs({ ...base, declarations: [] }).includes('--model-sources'))
})

argCase('★ client 入口：**两种形态同一条路**（同一个静态托管 ＋ 同一个 `--home` 家事实）', () => {
  // 2026-10-01 第六刀：源码形态不再走 `vite preview`（第三方 CLI 收不了我们的参数 ⇒ 那一支的
  // 认领判据只能退回"只看端口"）。两种形态现在都是 `scripts/serve_client.mjs`，
  // 只有 `--dist` 随形态走。
  //
  // ⚠ 这一条里路径用 `path.join` 算（**跨平台**）：`clientServeArgs` 自己拼路径，
  // 硬写 Windows 反斜杠会在 Linux 上把 `D:\celira/scripts/…` 当成"期望值"（实测踩过）。
  const repo = path.join('D:', 'celira')
  const base = {
    clientPort: '5173',
    listen: '127.0.0.1:8080',
    statusPort: '5190',
    configPort: '5191',
    homeDir: path.join('D:', 'celira-home'),
  }
  const release = up.clientServeArgs({ ...base, layout: { repo, clientDist: path.join(repo, 'client-dist') } })
  const source = up.clientServeArgs({ ...base, layout: { repo, clientDist: path.join(repo, 'client', 'dist') } })

  // 形状逐字（这一串是"页面不知道端口"与"这一支属于哪个家"两件事的实现手段）。
  assert.deepEqual(release, [
    path.join(repo, 'scripts', 'serve_client.mjs'),
    '--dist',
    path.join(repo, 'client-dist'),
    '--port',
    '5173',
    '--im',
    'http://127.0.0.1:8080',
    // TASK_61：代转要知道口子开在哪 ⇒ 这个旗标是"页面不知道端口"的实现手段。
    '--status-port',
    '5190',
    // TASK_77：模型配置门的代转同款。
    '--config-port',
    '5191',
    // 2026-09-25：**家事实**。进程表读得到命令行、读不到环境变量 —— 同一份发布包服务
    // 多个家时（可执行件路径相同、端口也可能相同），只有这一句认得出彼此。
    '--home',
    path.join('D:', 'celira-home'),
  ])

  // 源码形态：同一张形状，只有 `--dist` 换成本地构建的产物 —— **`--home` 也在**（这一条是本刀
  // 买到的：从前源码形态那一支带不了家事实）。
  assert.equal(source[0], path.join(repo, 'scripts', 'serve_client.mjs'))
  assert.equal(source[source.indexOf('--dist') + 1], path.join(repo, 'client', 'dist'))
  assert.equal(source[source.indexOf('--home') + 1], path.join('D:', 'celira-home'))
  assert.deepEqual(
    source.filter((part) => part !== path.join(repo, 'client', 'dist')),
    release.filter((part) => part !== path.join(repo, 'client-dist')),
    '两种形态只该差一个 --dist',
  )

  // 回归判据：`vite preview` 那条**已经退役**（少一条路 = 少一处会分叉的代理实现）。
  assert.equal(up.sourceClientArgs, undefined, 'vite preview 的启动参数不许再存在')
})

argCase('client 入口：非默认 status_port 原样传下去（不吃默认值）', () => {
  // 「页面自己跟着走」的机械判据在**参数这一层**：home.conf 写了 5191，就得原样到子进程手上。
  const args = up.releaseClientArgs({
    serveScript: 'D:\\celira\\scripts\\serve_client.mjs',
    clientDist: 'D:\\celira\\client-dist',
    clientPort: '5173',
    listen: '127.0.0.1:8080',
    statusPort: '5191',
    configPort: '5192',
    homeDir: 'D:\\celira-home',
  })
  assert.equal(args[args.indexOf('--status-port') + 1], '5191')
  assert.equal(args[args.indexOf('--config-port') + 1], '5192')
})

// ── TASK_77：模型来源在场 ⇒ 进程侧**一言不发** ────────────────────────────────
argCase('模型环境：文件不在场 ⇒ **原样交回**（现状分毫不动）', () => {
  const env = { Path: 'x', DEEPSEEK_API_KEY: 'sk-hand', CELIRA_MODEL_API_KEY: 'sk-other' }
  assert.deepEqual(up.modelEnvironment(env, false), env)
})

argCase('模型环境：文件在场 ⇒ 那两条**摘掉**（不只是"不添加"：手边本来有的也要摘）', () => {
  const env = up.modelEnvironment(
    {
      Path: 'x',
      DEEPSEEK_API_KEY: 'sk-hand',
      CELIRA_MODEL_API_KEY: 'sk-other',
      CELIRA_IM_BACKEND_AUTH: 'clr_bc_hand',
    },
    true,
  )
  assert.deepEqual(Object.keys(env).sort(), ['CELIRA_IM_BACKEND_AUTH', 'Path'])
  for (const key of up.MODEL_ENV_KEYS) {
    assert.ok(!(key in env), `文件在场时不许把 ${key} 交给 Agent（进程侧与文件会打架，宿主会拒绝启动）`)
  }
  assert.equal(env.CELIRA_IM_BACKEND_AUTH, 'clr_bc_hand', 'im 那条凭据与模型源无关，照旧')
})

argCase('参数解析：无参 / --token / --status', () => {
  assert.deepEqual(up.parseUpArgs([]), {
    wantToken: false,
    wantStatus: false,
    // `--agent`（认领，2026-09-25）：`null` = **没给这个旗标**（照旧自建那条路）；
    // 给了但空值/像旗标 ⇒ 用法错（见下一条用例）。
    adoptAgent: null,
    // `--agent-name`（2026-10-12）：**自建那一支**的显示名。`null` = 没给 ⇒ 布置用占位
    // `未命名`（系统不替 Agent 起名 —— 显示名是公共呈现，归它自己或它的 Owner）。
    agentName: null,
    // **远端跑法**（2026-09-30）：`imEndpoint` = 规范化过的世界地址（`null` = 没给 ⇒ 本机全量）；
    // `remoteToken` = `--token` **跟的那个值**（主人凭据；`null` = 没跟值）。
    imEndpoint: null,
    remoteToken: null,
    // ★ `--attach-only`（2026-10-15）：**只接上、不要求模型**（没模型 ⇒ 接上但先不起它）。
    // 缺省 `false` ⇒ 既有口径（没模型就早失败）一字不变。
    attachOnly: false,
    error: null,
  })
  assert.equal(up.parseUpArgs(['--token']).wantToken, true)
  assert.equal(up.parseUpArgs(['--status']).wantStatus, true)
  assert.equal(up.parseUpArgs(['--attach-only']).attachOnly, true)
})

argCase('参数解析：--agent-name 带名字 / 不带名字 / 空值（后两种 ⇒ 用法错，绝不回落成占位）', () => {
  assert.equal(up.parseUpArgs(['--agent-name', '阿岩']).agentName, '阿岩')
  assert.equal(up.parseUpArgs(['--agent-name', '  阿岩  ']).agentName, '阿岩', '两侧空白去掉')
  for (const argv of [['--agent-name'], ['--agent-name', ''], ['--agent-name', '   '], ['--agent-name', '--token']]) {
    const bad = up.parseUpArgs(argv)
    assert.ok(bad.error && bad.error.includes('--agent-name'), `${JSON.stringify(argv)} 必须报用法错`)
    assert.equal(bad.agentName, null, '报错时不许留下半个名字')
  }
  // 与 `--agent` 一起给是合法的（认领时这个名字用不上，布置会**说**它没派上用场）。
  const both = up.parseUpArgs(['--agent', 'a-1', '--agent-name', '阿岩'])
  assert.equal(both.error, null)
  assert.equal(both.adoptAgent, 'a-1')
  assert.equal(both.agentName, '阿岩')
})

argCase('参数解析：--agent 带 id / 不带 id / 空值（后两种 ⇒ 用法错，绝不回落成自建）', () => {
  assert.equal(up.parseUpArgs(['--agent', 'a-1']).adoptAgent, 'a-1')
  assert.equal(up.parseUpArgs(['--agent', '  a-1  ']).adoptAgent, 'a-1', '两侧空白去掉')
  for (const argv of [['--agent'], ['--agent', ''], ['--agent', '   '], ['--agent', '--token']]) {
    const bad = up.parseUpArgs(argv)
    assert.ok(bad.error && bad.error.includes('--agent'), `${JSON.stringify(argv)} 必须报用法错`)
    assert.equal(bad.adoptAgent, null, '报错时不许留下半个 id')
  }
})

argCase('参数解析：--resume 已退役 ⇒ 用法错，且读数要说出真相与出路（不许静默忽略）', () => {
  // 跨进程恢复退役于 2026-09-26（一段清醒被一段不清醒隔开就是两段）。这条钉住的是**退役之后**的行为：
  // 它不是"不认识的参数"，而是"这条路没了 + 你现在该敲哪条" —— 静默忽略会让用户以为它跑了。
  for (const argv of [['--resume', 'host-9-9-9'], ['--resume'], ['--resume', '--token']]) {
    const bad = up.parseUpArgs(argv)
    assert.ok(bad.error, `${JSON.stringify(argv)} 必须报用法错`)
    assert.ok(bad.error.includes('已退役'), `读数要说清这条路没了：${bad.error}`)
    assert.ok(bad.error.includes('node home.js up'), `读数要给出出路：${bad.error}`)
    assert.equal(bad.resumeRun, undefined, '解析结果里不该再有 resumeRun 这个字段')
  }
})

argCase('参数解析：不认识的参数 ⇒ 用法错', () => {
  assert.ok(up.parseUpArgs(['--nope']).error)
  assert.ok(up.parseUpArgs(['start']).error)
})

// ── 远端跑法（2026-09-30）：`--im-endpoint` 与 `--token` 带值的纪律 ─────────────────
argCase('参数解析（远端）：--im-endpoint 带地址 ⇒ 规范化（去尾斜杠）并记下；不带值/带旗标 ⇒ 用法错', () => {
  assert.equal(up.parseUpArgs(['--im-endpoint', 'http://127.0.0.1:8100']).imEndpoint, 'http://127.0.0.1:8100')
  assert.equal(up.parseUpArgs(['--im-endpoint', ' http://host:80/ ']).imEndpoint, 'http://host:80', '两侧空白与尾斜杠都去掉')
  assert.equal(up.parseUpArgs(['--im-endpoint', 'https://world.example.com']).imEndpoint, 'https://world.example.com')
  // **缺值一律是用法错**（绝不回落成"没给这个旗标" ⇒ 悄悄变成本机全量）。
  for (const argv of [['--im-endpoint'], ['--im-endpoint', ''], ['--im-endpoint', '  '], ['--im-endpoint', '--token']]) {
    const bad = up.parseUpArgs(argv)
    assert.ok(bad.error && bad.error.includes('--im-endpoint'), `${JSON.stringify(argv)} 必须报用法错：${bad.error}`)
    assert.equal(bad.imEndpoint, null, '报错时不许留下半个地址')
  }
})

argCase('参数解析（远端）：地址形状不对（缺协议头 / 带路径 / 带查询）⇒ 用法错，且读数说清要什么', () => {
  for (const [value, hint] of [
    ['127.0.0.1:8100', 'http://'],
    ['http://', '主机'],
    ['http://host:8100/path', '路径'],
    ['http://host:8100?x=1', '查询'],
    ['http://ho st:8100', '空格'],
  ]) {
    const bad = up.parseUpArgs(['--im-endpoint', value])
    assert.ok(bad.error, `「${value}」必须报用法错`)
    assert.ok(bad.error.includes(hint), `读数要说清缺什么（期望含「${hint}」）：${bad.error}`)
  }
})

argCase('参数解析（远端）：--token 跟值 = 主人凭据（要配 --im-endpoint）；不跟值 = 老路补发', () => {
  const both = up.parseUpArgs(['--im-endpoint', 'http://127.0.0.1:8100', '--token', 'clr_hs_x'])
  assert.equal(both.imEndpoint, 'http://127.0.0.1:8100')
  assert.equal(both.remoteToken, 'clr_hs_x')
  assert.equal(both.wantToken, false, '跟了值就不该同时是"补发"')
  assert.equal(both.error, null)

  // 顺序无关：反过来读也一样。
  const reversed = up.parseUpArgs(['--token', ' clr_hs_x ', '--im-endpoint', 'http://h:1'])
  assert.equal(reversed.remoteToken, 'clr_hs_x')
  assert.equal(reversed.imEndpoint, 'http://h:1')

  // `--token` 跟值但不给世界地址 ⇒ 用法错（那是远端跑法的输入，绝不静默吞掉）。
  const lonely = up.parseUpArgs(['--token', 'clr_hs_x'])
  assert.ok(lonely.error && lonely.error.includes('--im-endpoint'), `要指出去哪用：${lonely.error}`)

  // 远端模式里 `--token` 不带值 ⇒ 用法错（补发是**世界那台机器**上的事）。
  const bare = up.parseUpArgs(['--im-endpoint', 'http://h:1', '--token'])
  assert.ok(bare.error && bare.error.includes('世界那台机器'), `要说清补发在哪边做：${bare.error}`)

  // `--status` 与 `--im-endpoint` 是两件事 ⇒ 用法错。
  const mixed = up.parseUpArgs(['--status', '--im-endpoint', 'http://h:1'])
  assert.ok(mixed.error && mixed.error.includes('--status'), `要说清哪两个不能一起用：${mixed.error}`)
})

argCase('起 Agent 的命令行（远端）：`--im-endpoint` 的值换成**远端世界地址**，位置一字不动', () => {
  assert.deepEqual(
    up.buildHostArgs({
      ...base,
      declarations: [],
      imEndpoint: 'http://10.0.0.5:8080',
      isWindows: false,
      isLinux: true,
    }),
    [
      '--root', base.envDir,
      '--agent', base.agentId,
      '--materials', base.materialsDir,
      '--max-turns', '300',
      '--terminal', 'full',
      '--im-endpoint', 'http://10.0.0.5:8080',
    ],
  )
  // 回归判据：**不给远端地址**时与从前逐字节一致（本机全量那条路一个字都没动）。
  assert.deepEqual(
    up.buildHostArgs({ ...base, declarations: [], isWindows: false, isLinux: true }),
    up.buildHostArgs({ ...base, declarations: [], imEndpoint: null, isWindows: false, isLinux: true }),
  )
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '本文件一条用例都没跑（空跑不算过）')
  assert.ok(cases >= 7, `用例数 ${cases} 少于声明的 7`)
})

argCase('TASK_62：Agent"在不在跑"的四支——账本活着 / 孤儿接管 / 都不在 / 账本记的是**别家**的', () => {
  // 这一问是 TASK_62 的前提判据（凭据那一步按它决定动不动），所以几支都要钉住。
  // 判据复用既有两条（账本 → ownsPid 指纹；账本不活时找**本家**的孤儿），本用例只判组合，
  // 另加两条本刀的事实：**家事实必须传下去**；**别家的账本不是"Agent 在跑"**（只进 foreign 读数）。
  const layout = { kind: 'source' }
  const home = { envDir: 'D://celira-home//env', clientPort: '5173', imPort: '8080', statusPort: '5190', configPort: '5191', listening: new Map() }
  const pidFile = 'D://celira-home//logs//host.pid'
  const writes = []

  // ① 账本里活着 ⇒ 直接用账本，**不去进程表里找**、也不动 pid 文件
  let askedOrphans = 0
  let seenLedgerArgs = null
  assert.deepEqual(
    up.agentRunningNow({
      hostPidFile: pidFile,
      layout,
      home,
      ledgerPid: (...args) => {
        seenLedgerArgs = args
        return { pid: 4321, state: 'ours' }
      },
      orphansOf: () => {
        askedOrphans += 1
        return []
      },
      writePid: () => {
        throw new Error('账本活着时不该写 pid 文件')
      },
    }),
    { running: true, pid: 4321, adopted: false, foreign: null },
  )
  assert.equal(askedOrphans, 0, '账本活着就不该去进程表里找')
  assert.deepEqual(seenLedgerArgs, ['agent', pidFile, layout, home], '家事实必须原样传给账本判据')

  // ② 账本不活、进程表里有对得上的孤儿 ⇒ **接管**（把 pid 文件重写指向它），不是重起
  let seenOrphanArgs = null
  assert.deepEqual(
    up.agentRunningNow({
      hostPidFile: pidFile,
      layout,
      home,
      ledgerPid: () => ({ pid: null, state: 'gone' }),
      orphansOf: (...args) => {
        seenOrphanArgs = args
        return [777]
      },
      writePid: (file, pid) => writes.push([file, pid]),
    }),
    { running: true, pid: 777, adopted: true, foreign: null },
  )
  assert.deepEqual(writes, [[pidFile, 777]])
  assert.deepEqual(seenOrphanArgs, [layout, home], '孤儿扫描同样要拿家事实（只收本家的）')

  // ③ 两样都没有 ⇒ **不在跑**（只有这一支才轮到"起 Agent"，也才轮到 TASK_59 的轮换）
  assert.deepEqual(
    up.agentRunningNow({
      hostPidFile: pidFile,
      layout,
      home,
      ledgerPid: () => ({ pid: null, state: 'gone' }),
      orphansOf: () => [],
      writePid: () => {
        throw new Error('不在跑时不该写 pid 文件')
      },
    }),
    { running: false, pid: null, adopted: false, foreign: null },
  )

  // ④ 账本里那支是**别家**的（家事实对不上）⇒ **不算 Agent 在跑**（起本家那支），
  //    但那个 pid 要进 `foreign` —— up 的读数靠它说清"为什么还是起了一支新的"。
  assert.deepEqual(
    up.agentRunningNow({
      hostPidFile: pidFile,
      layout,
      home,
      ledgerPid: () => ({ pid: 33828, state: 'not-ours' }),
      orphansOf: () => [],
      writePid: () => {
        throw new Error('别家的账本不该被写进 pid 文件')
      },
    }),
    { running: false, pid: null, adopted: false, foreign: 33828 },
  )
  // 边界：别家账本在场、同时本家有一支孤儿 ⇒ 接管本家那支，foreign 照旧报出来。
  assert.deepEqual(
    up.agentRunningNow({
      hostPidFile: pidFile,
      layout,
      home,
      ledgerPid: () => ({ pid: 33828, state: 'not-ours' }),
      orphansOf: () => [777],
      writePid: (file, pid) => writes.push([file, pid]),
    }),
    { running: true, pid: 777, adopted: true, foreign: 33828 },
  )
})

argCase('起 Agent 的命令行（Windows）：**给了身份** ⇒ 两个旗标成对出现在 `--im-endpoint` 之后（TASK_70）', () => {
  assert.deepEqual(
    up.buildHostArgs({
      ...base,
      declarations: [],
      identity: { id: 'core-1', displayName: '小满' },
      isWindows: true,
    }),
    [
      '--root', base.envDir,
      '--agent', base.agentId,
      '--materials', base.materialsDir,
      '--max-turns', '300',
      '--terminal', 'full',
      '--im-endpoint', 'http://127.0.0.1:8080',
      '--identity-agent', 'core-1',
      '--identity-name', '小满',
    ],
  )
})

argCase('起 Agent 的命令行：**问不到身份** ⇒ 一个参数都不多（开场照旧跳过那一行）', () => {
  const args = up.buildHostArgs({ ...base, declarations: [], identity: null })
  assert.deepEqual(args, up.buildHostArgs({ ...base, declarations: [] }))
  assert.ok(!args.includes('--identity-agent'))
  assert.ok(!args.includes('--identity-name'))
})
