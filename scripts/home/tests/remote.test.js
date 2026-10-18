'use strict'
// **远端跑法**（`node home.js up --im-endpoint <世界地址> [--token <主人凭据>]`，2026-09-30）。
//
// 这一族钉住的是本刀新增的三层：
//   1. **形状层**（纯函数）：世界地址只收"协议头 + 主机[:端口]"；主人凭据的来源顺序
//      （旗标 → 环境变量 → 没有）；`GET /v1/agents` 回执的四态；"可认领"的三态；
//      "记着世界的家裸跑 up"的裁决 —— 全部可机械判错；
//   2. **预检层**（注入 imRequest）：三种卡点（不可达 / 凭据被拒 / 没有可认领的身份）
//      各自的读数分开说，且都在动手之前停手；
//   3. **真进程黑盒**：`node home.js up` 的参数纪律与"远端家裸跑"的拒绝 —— 与主人敲的是
//      同一条路（不是内部函数）。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')
const { spawnSync } = require('node:child_process')

const remote = require('../remote')
const P = require('../paths')

let cases = 0
function remoteCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

// ---------------------------------------------------------------------------
// 1) 形状层（纯函数）
// ---------------------------------------------------------------------------

remoteCase('世界地址：只收「协议头 + 主机[:端口]」，尾斜杠去掉；别的形状当场拒（不修复、不猜）', () => {
  assert.deepEqual(remote.parseImEndpoint('http://127.0.0.1:8080'), { ok: true, base: 'http://127.0.0.1:8080' })
  assert.deepEqual(remote.parseImEndpoint('  http://host:80/  '), { ok: true, base: 'http://host:80' })
  assert.deepEqual(remote.parseImEndpoint('https://world.example.com'), { ok: true, base: 'https://world.example.com' })

  for (const value of ['', '   ', 'host:8080', 'http://', 'http://host:8080/x', 'http://host:8080?q=1', 'http://ho st:1', 'http://host:1#f']) {
    const bad = remote.parseImEndpoint(value)
    assert.equal(bad.ok, false, `「${value}」必须拒`)
    assert.ok(bad.error, '拒的时候要说清缺什么')
  }
})

remoteCase('主人凭据的来源：--token 的值 → 环境变量 CELIRA_IM_HUMAN_AUTH → 没有（三态分开说）', () => {
  const flag = remote.remoteTokenFrom({ flagToken: 'clr_hs_from_flag', readEnv: () => 'clr_hs_from_env' })
  assert.deepEqual(flag, { token: 'clr_hs_from_flag', source: 'flag' }, '旗标优先')

  const env = remote.remoteTokenFrom({ flagToken: null, readEnv: (name) => (name === remote.HUMAN_ENV ? 'clr_hs_from_env' : null) })
  assert.deepEqual(env, { token: 'clr_hs_from_env', source: 'env' })

  const none = remote.remoteTokenFrom({ flagToken: null, readEnv: () => null })
  assert.deepEqual(none, { token: null, source: null })

  // 读环境这一步抛错 ⇒ 如实按"没有"处理（不炸、也不编一支出来）。
  const broken = remote.remoteTokenFrom({ flagToken: null, readEnv: () => { throw new Error('powershell 起不来') } })
  assert.deepEqual(broken, { token: null, source: null })
})

remoteCase('GET /v1/agents 的回执四态：连不上 / 被拒（401、403）/ 好（200 + 列表）/ 读不懂', () => {
  assert.equal(remote.classifyWorldAnswer(null), 'unreachable')
  assert.equal(remote.classifyWorldAnswer({ status: 0 }), 'unreachable')
  assert.equal(remote.classifyWorldAnswer({ status: 401, body: {} }), 'refused')
  assert.equal(remote.classifyWorldAnswer({ status: 403, body: {} }), 'refused')
  assert.equal(remote.classifyWorldAnswer({ status: 200, body: { agents: [] } }), 'ok')
  assert.equal(remote.classifyWorldAnswer({ status: 200, body: {} }), 'unreadable', '200 但没有列表 ⇒ 读不懂，不硬猜')
  assert.equal(remote.classifyWorldAnswer({ status: 500, body: {} }), 'unreadable')
})

remoteCase('可认领的三态：在场且 active / 不在列表里 / 不是 active', () => {
  const list = [
    { agent: { id: 'a-1', display_name: '小满', status: 'active' } },
    { agent: { id: 'a-2', display_name: '别的', status: 'revoked' } },
  ]
  assert.deepEqual(remote.findClaimableIdentity(list, 'a-1'), { ok: true, name: '小满', status: 'active' })
  assert.equal(remote.findClaimableIdentity(list, 'a-9').reason, 'missing')
  assert.equal(remote.findClaimableIdentity(list, 'a-9').count, 2, '读数要带上"你一共有几个"（给用户换 id 的判据）')
  assert.equal(remote.findClaimableIdentity(list, 'a-2').reason, 'not-active')
  // 没有名字时用 id 顶（不写空）。
  assert.equal(remote.findClaimableIdentity([{ agent: { id: 'a-3', status: 'active' } }], 'a-3').name, 'a-3')
})

remoteCase('"记着世界的家裸跑 up"的裁决：没记 ⇒ 过；记了 ⇒ 停手（补发那一路的读数不一样）', () => {
  assert.deepEqual(remote.localStartVerdict({ confImEndpoint: '' }), { ok: true, lines: [] })

  const plain = remote.localStartVerdict({ confImEndpoint: 'http://h:1', wantToken: false })
  assert.equal(plain.ok, false)
  const text = plain.lines.join('\n')
  assert.match(text, /远端跑法/)
  assert.match(text, /--im-endpoint http:\/\/h:1/, '要给出出路：带上那个世界')
  assert.match(text, /im_endpoint/, '要给出"改回本机全量"的那一步')
  assert.match(text, /agent_id/, '要提醒身份那三行也是那个世界的事实')

  const reissue = remote.localStartVerdict({ confImEndpoint: 'http://h:1', wantToken: true })
  assert.equal(reissue.ok, false)
  assert.match(reissue.lines.join('\n'), /世界那台机器/, '补发 token 是那边的事')
})

// ---------------------------------------------------------------------------
// 2) 预检层（注入 imRequest ⇒ 不起进程、不出网也能判）
// ---------------------------------------------------------------------------

/** 一个按"调用次序"返回预置响应的假 imRequest（照 provision.test.js 的 fakeIm 先例）。 */
function fakeIm(responses) {
  const calls = []
  const fn = async (base, method, url, options) => {
    calls.push({ base, method, url, options })
    const next = responses[calls.length - 1]
    if (!next) throw new Error(`假 imRequest 收到第 ${calls.length} 次调用，但没有预置响应`)
    return next
  }
  return { fn, calls }
}

const AGENTS_OK = {
  status: 200,
  body: { agents: [{ agent: { id: 'a-1', display_name: '小满', status: 'active' } }] },
}

remoteCase('预检（不可达）：读数说"不可达"、停在动手之前，且**一次请求都不发第二下**', async () => {
  const im = fakeIm([{ status: 0, body: null, error: 'ECONNREFUSED' }])
  const out = await remote.preflightWorld({ base: 'http://127.0.0.1:1', token: 'clr_hs_x', imRequest: im.fn })
  assert.equal(out.ok, false)
  assert.equal(out.reason, 'unreachable')
  const text = out.lines.join('\n')
  assert.match(text, /不可达/)
  assert.match(text, /ECONNREFUSED/, '读数要带上网层的原话（可复核）')
  assert.match(text, /停手/)
  assert.equal(im.calls.length, 1)
  assert.equal(im.calls[0].options.bearer, 'clr_hs_x', '预检也要带上主人凭据（问的就是"它认不认"）')
})

remoteCase('预检（凭据被拒）：读数说"世界拒绝了这支凭据"，并给出补发的路', async () => {
  const im = fakeIm([{ status: 401, body: { error: 'invalid_credential' } }])
  const out = await remote.preflightWorld({ base: 'http://h:1', token: 'clr_hs_bad', imRequest: im.fn })
  assert.equal(out.ok, false)
  assert.equal(out.reason, 'refused')
  const text = out.lines.join('\n')
  assert.match(text, /拒绝/)
  assert.match(text, /401/)
  assert.match(text, /up --token/, '出路：在世界那台机器上补发一支')
  assert.match(text, /停手/)
})

remoteCase('预检（没有可认领的身份）：--agent 给的 id 不在（或不是你的）⇒ 停手，读数带上"你共有几个"', async () => {
  const im = fakeIm([AGENTS_OK])
  const out = await remote.preflightWorld({
    base: 'http://h:1',
    token: 'clr_hs_x',
    adoptAgentId: 'a-404',
    imRequest: im.fn,
  })
  assert.equal(out.ok, false)
  assert.equal(out.reason, 'not-claimable')
  const text = out.lines.join('\n')
  assert.match(text, /没有这个 Agent，或者它不是你的/)
  assert.match(text, /共 1 个/)
  assert.match(text, /停手/)
})

remoteCase('预检（撤销过的身份不许安家）：status != active ⇒ 停手', async () => {
  const im = fakeIm([{ status: 200, body: { agents: [{ agent: { id: 'a-2', display_name: 'x', status: 'revoked' } }] } }])
  const out = await remote.preflightWorld({ base: 'http://h:1', token: 'clr_hs_x', adoptAgentId: 'a-2', imRequest: im.fn })
  assert.equal(out.ok, false)
  assert.equal(out.reason, 'not-active')
  assert.match(out.lines.join('\n'), /撤销过的身份不许安家/)
})

remoteCase('预检（换了世界）：home.conf 记着的身份不在这个世界 ⇒ 两条出路都说清（不是含糊的 404）', async () => {
  const im = fakeIm([AGENTS_OK])
  const out = await remote.preflightWorld({
    base: 'http://h:1',
    token: 'clr_hs_x',
    confAgentId: 'a-old-world',
    imRequest: im.fn,
  })
  assert.equal(out.ok, false)
  assert.equal(out.reason, 'stale-identity')
  const text = out.lines.join('\n')
  assert.match(text, /不在这个世界里/)
  assert.match(text, /--agent/, '出路①：认领那个世界里的一个')
  assert.match(text, /清掉 home.conf 的 agent_id/, '出路②：清掉旧世界的身份再新建')
})

remoteCase('预检（过）：世界可达、凭据被认下；要认领时把"认领目标在场"也说出来', async () => {
  const im = fakeIm([AGENTS_OK])
  const plain = await remote.preflightWorld({ base: 'http://h:1', token: 'clr_hs_x', imRequest: im.fn })
  assert.equal(plain.ok, true)
  assert.match(plain.lines.join('\n'), /世界可达、主人凭据被认下/)

  const im2 = fakeIm([AGENTS_OK])
  const adopt = await remote.preflightWorld({ base: 'http://h:1', token: 'clr_hs_x', adoptAgentId: 'a-1', imRequest: im2.fn })
  assert.equal(adopt.ok, true)
  assert.match(adopt.lines.join('\n'), /认领目标在场：小满（a-1）status=active/)
})

remoteCase('预检（无主人凭据的重跑路）：拿已存的后端凭据问 /v1/backend/me —— 三态分开说', async () => {
  // ① 认下且代表本家 ⇒ 过
  const ok = await remote.preflightStoredCredential({
    base: 'http://h:1',
    agentId: 'a-1',
    credential: 'clr_bc_x',
    imRequest: fakeIm([{ status: 200, body: { agent: { id: 'a-1', display_name: '小满' } } }]).fn,
  })
  assert.equal(ok.ok, true)

  // ② 连不上 ⇒ 不可达
  const down = await remote.preflightStoredCredential({
    base: 'http://h:1',
    agentId: 'a-1',
    credential: 'clr_bc_x',
    imRequest: fakeIm([{ status: 0, body: null, error: 'timeout' }]).fn,
  })
  assert.equal(down.ok, false)
  assert.equal(down.reason, 'unreachable')

  // ③ 不认这支 ⇒ 说清"可能被撤销/轮换过"，并指出重签要带主人凭据
  const rejected = await remote.preflightStoredCredential({
    base: 'http://h:1',
    agentId: 'a-1',
    credential: 'clr_bc_x',
    imRequest: fakeIm([{ status: 401, body: {} }]).fn,
  })
  assert.equal(rejected.ok, false)
  assert.equal(rejected.reason, 'credential-rejected')
  assert.match(rejected.lines.join('\n'), /不认这支后端凭据/)
  assert.match(rejected.lines.join('\n'), /--token/)

  // ④ 代表别人 ⇒ 对不上，停手
  const wrong = await remote.preflightStoredCredential({
    base: 'http://h:1',
    agentId: 'a-1',
    credential: 'clr_bc_x',
    imRequest: fakeIm([{ status: 200, body: { agent: { id: 'a-other' } } }]).fn,
  })
  assert.equal(wrong.ok, false)
  assert.equal(wrong.reason, 'wrong-identity')
  assert.match(wrong.lines.join('\n'), /代表 a-other，本家是 a-1/)
})

// ---------------------------------------------------------------------------
// 3) 真进程黑盒：与主人敲的是同一条路
// ---------------------------------------------------------------------------

function scratch(label) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `celira-remote-${label}-`))
}

/** 真的敲一次 `node home.js <参数>`（不是内部函数）。 */
function runHome(args, homeDir, extraEnv = {}) {
  const done = spawnSync(process.execPath, [path.join(P.REPO, 'home.js'), ...args], {
    env: {
      ...process.env,
      CELIRA_HOME_DIR: homeDir,
      CELIRA_CREDENTIALS_DIR: path.join(homeDir, '..', 'creds-never-touched'),
      // 预检跑在模型检查之后 ⇒ 占位钥匙是必须的（本文件**没有**任何真钥匙）。
      DEEPSEEK_API_KEY: 'sk-placeholder-not-a-real-key',
      ...extraEnv,
    },
    encoding: 'utf8',
    windowsHide: true,
  })
  return { code: done.status, out: `${done.stdout ?? ''}${done.stderr ?? ''}` }
}

remoteCase('真进程：`--im-endpoint` 缺值 / `--token` 跟值却不给世界 / `--status` 混世界 ⇒ 都是用法错（1）', () => {
  const home = scratch('usage')
  const bare = runHome(['up', '--im-endpoint'], home)
  assert.equal(bare.code, 1, bare.out)
  assert.match(bare.out, /--im-endpoint 要一个世界地址/)

  const lonelyToken = runHome(['up', '--token', 'clr_hs_x'], home)
  assert.equal(lonelyToken.code, 1, lonelyToken.out)
  assert.match(lonelyToken.out, /远端跑法/)

  const mixed = runHome(['up', '--status', '--im-endpoint', 'http://127.0.0.1:1'], home)
  assert.equal(mixed.code, 1, mixed.out)
  assert.match(mixed.out, /--status 与 --im-endpoint/)
})

remoteCase('真进程：远端家**没给**主人凭据（全新家 / 只给 --agent）⇒ 停手（2），不是内部错', () => {
  const home = scratch('need-token')
  const bare = runHome(['up', '--im-endpoint', 'http://127.0.0.1:1'], home)
  assert.equal(bare.code, 2, bare.out)
  assert.match(bare.out, /还需要主人凭据/)
  assert.match(bare.out, /clr_hs_/, '要说清凭据长什么样')
  assert.match(bare.out, /停手：这一次没有对任何世界做任何改动/)
  assert.ok(!bare.out.includes('意料之外的错'), `不许是内部错：${bare.out}`)

  const adopt = runHome(['up', '--im-endpoint', 'http://127.0.0.1:1', '--agent', 'a-1'], home)
  assert.equal(adopt.code, 2, adopt.out)
  assert.match(adopt.out, /认领世界里已有的一个身份要主人凭据/)
  assert.ok(!adopt.out.includes('意料之外的错'), adopt.out)
})

remoteCase('真进程：记着世界的家**裸跑 up 一律停手**（退出码 2）—— 不许静默回落成本机全量', () => {
  const home = scratch('refuse')
  fs.writeFileSync(
    path.join(home, 'home.conf'),
    ['listen_addr=127.0.0.1:8080', 'client_port=5173', 'im_endpoint=http://world.example:8080', ''].join('\n'),
    'utf8',
  )
  const done = runHome(['up'], home)
  assert.equal(done.code, 2, done.out)
  assert.match(done.out, /这个家是\*\*远端跑法\*\*/)
  assert.match(done.out, /--im-endpoint http:\/\/world.example:8080/)
  assert.ok(!done.out.includes('缺可执行件'), '拒绝要发生在任何"起东西"的判据之前')

  // 补发那一路（`up --token`）：同款停手，且说清"那是世界那台机器上的事"。
  const reissue = runHome(['up', '--token'], home)
  assert.equal(reissue.code, 2, reissue.out)
  assert.match(reissue.out, /世界那台机器/)
})

remoteCase('真进程：世界不可达 ⇒ fail-closed（退出码 2、读数说清卡在哪、什么都没改）', async () => {
  const layout = P.detectLayout()
  if (!P.isFile(layout.hostExe) || !P.isFile(layout.runExe)) {
    // 离线套件不强制先构建：没有可执行件就如实跳过（照 world.test.js 的先例，不假装验过）。
    console.log('      （跳过：还没有构建 agent_host / agent_run —— 先 cd agent && cargo build --bins --release）')
    return
  }
  const home = scratch('unreachable')
  // 找一个**关着**的口：先绑 0 号口拿一个空闲号，再关掉它（小窗口内不会被别人抢走）。
  const net = require('node:net')
  const port = await new Promise((resolve) => {
    const probe = net.createServer()
    probe.listen(0, '127.0.0.1', () => {
      const picked = probe.address().port
      probe.close(() => resolve(picked))
    })
  })
  assert.ok(port, '没拿到空闲口')

  const run = runHome(['up', '--im-endpoint', `http://127.0.0.1:${port}`, '--token', 'clr_hs_not-a-real-token'], home)
  assert.equal(run.code, 2, run.out)
  assert.match(run.out, /世界\*\*不可达\*\*/, run.out)
  assert.match(run.out, /停手/, run.out)
  assert.ok(!run.out.includes('起不来（30 秒没回应）'), '本机 core 那一段根本不该走到')
  assert.ok(!fs.existsSync(path.join(home, 'logs', 'im.pid')), '远端跑法不许写 im 的账本（本机没有 core）')
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '本文件一条用例都没跑（空跑不算过）')
  assert.ok(cases >= 10, `用例数 ${cases} 少于声明的 10`)
})
