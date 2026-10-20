'use strict'
// 布置的**可重入**判据（TASK_59）。
//
// 这一族为什么值得单独立文件：它是"同机第二个家会把第一个家变成不可启动"那条病的药，
// 而药本身有三个容易配错的地方 ——
//  1. `201` 与 `200` 的区别是"这次创建的"与"同 key 重放"，两个都算成功；
//  2. **凭据那一步不能"重放即接受"**：原文（token）只给一次 ⇒ 只收 200 会把 `null`
//     写进用户环境变量 ⇒ 从"响亮失败"变成"静默坏掉"；
//  3. **"凭据已存在"有两个触发面**（真机都见过，别只写一个）：
//     `200`（同 key 重放，响应里 `token: null`）与 **`409 credential_state_conflict`**
//     （状态变过，服务端连 id 都不给）。⇒ 统一"读一次现有凭据 → 用它的 id 轮换"。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')

const {
  acceptedCreate,
  obtainBackendCredential,
  provision,
  humanNameProblem,
  HUMAN_DISPLAY_NAME_PLACEHOLDER,
} = require('../provision')

let cases = 0
function pCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

// ── 首位 Human 的显示名：**由人给**（2026-10-20 主人裁决"必须要填"）────────────
//
// 判据：显示名是**公共事实**（那个世界的 Participant），用户名是**机器事实** ——
// 拿机器事实写进公共世界就是冒充公共事实，而且它常常不是你（Administrator / user /
// DESKTOP-XXXX），同机两个人也分不开。这与 Agent 那条（2026-10-12）是同一条：
// 系统补能力缺口，**不替谁认定身份**（判据 9）。
pCase('名字那一格：三条规则与 Core 同一套（禁 NUL / ≤80 个 scalar / trim 后非空）', () => {
  assert.equal(humanNameProblem('测试名字'), null)
  assert.equal(humanNameProblem('  测试名字  '), null, '两侧空白由布置削掉')
  assert.match(humanNameProblem(''), /不能是空的/)
  assert.match(humanNameProblem('   '), /不能是空的/)
  assert.match(humanNameProblem('a\u0000b'), /控制字符/)
  assert.equal(humanNameProblem('字'.repeat(80)), null)
  assert.match(humanNameProblem('字'.repeat(81)), /80/)
  // 按 **scalar value** 数，不按 UTF-16 单元：80 个 emoji 合格，81 个不合格。
  assert.equal(humanNameProblem('🐱'.repeat(80)), null)
  assert.match(humanNameProblem('🐱'.repeat(81)), /80/)
  // 占位是"没有名字"的诚实表达，不是默认值。
  assert.equal(HUMAN_DISPLAY_NAME_PLACEHOLDER, '未命名')
  // 反向判据（这一刀的全部理由）：**不再有任何从机器环境造名字的路**。
  const source = require('node:fs').readFileSync(
    require('node:path').join(__dirname, '..', 'provision.js'),
    'utf8',
  )
  assert.ok(!/USERNAME|process\.env\.USER\b/.test(source), '不再读机器用户名当显示名')
})

/** 造一个假的 `imRequest`：按调用次序返回预置响应，并把收到的参数记下来。 */
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

const ARGS = {
  base: 'http://127.0.0.1:8080',
  token: 'human-token',
  agentId: 'a1',
  bindingId: 'b1',
  // 布置键**按家取**（2026-10-01）：凭据那一步的签发与轮换都带上它们。
  keys: {
    agent: 'home-agent:H1',
    binding: 'home-binding:H1',
    credential: 'home-credential:H1',
    direct: 'home-direct:H1',
  },
}

/** 现有凭据清单（轮换前要先读它）。 */
const LIST_WITH_ACTIVE = {
  status: 200,
  body: { credentials: [{ id: 'c-old', binding_id: 'b1', status: 'active' }] },
}

pCase('可重入判据：201（这次建的）与 200（同 key 重放）都算成了', () => {
  assert.equal(acceptedCreate(201), true)
  assert.equal(acceptedCreate(200), true)
  // 负对照：别的码不许混进来（否则"重入"就成了"什么都接受"）。
  assert.equal(acceptedCreate(409), false)
  assert.equal(acceptedCreate(500), false)
  assert.equal(acceptedCreate(undefined), false)
})

pCase('凭据首次签发（201）⇒ 直接拿原文，只发一次请求', async () => {
  const im = fakeIm([{ status: 201, body: { credential: { id: 'c1' }, token: 'T-FIRST' } }])
  const out = await obtainBackendCredential({ ...ARGS, imRequest: im.fn })
  assert.equal(out.ok, true)
  assert.equal(out.token, 'T-FIRST')
  assert.equal(im.calls.length, 1, '201 时不该有第二次请求')
  assert.equal(im.calls[0].options.key, 'home-credential:H1', '签发带的是**这个家**的键')
  assert.deepEqual(im.calls[0].options.body, { replaces_credential_id: null })
})

pCase('★ 凭据重放（200 ＋ token:null）⇒ 读一次现有凭据、**轮换**拿新原文', async () => {
  const im = fakeIm([
    { status: 200, body: { credential: { id: 'c-old' }, token: null, idempotency_replayed: true } },
    LIST_WITH_ACTIVE,
    { status: 201, body: { credential: { id: 'c-new' }, token: 'T-ROTATED' } },
  ])
  const out = await obtainBackendCredential({ ...ARGS, imRequest: im.fn })
  assert.equal(out.ok, true)
  assert.equal(out.token, 'T-ROTATED', '拿到的必须是轮换后那支新的')
  assert.equal(im.calls.length, 3)
  // 幂等键**钉在被替换的那条凭据 id 上** —— 同一状态重试会重放（无害），
  // 一旦轮换成功状态就变、键也跟着变（不会连转两次）。**并且带上这个家**。
  assert.equal(im.calls[2].options.key, 'home-credential:H1:c-old')
  assert.deepEqual(im.calls[2].options.body, { replaces_credential_id: 'c-old' })
})

pCase('★★ 真机面：`409 credential_state_conflict`（状态变过，不给 id）⇒ 同样靠"读＋轮换"过去', async () => {
  // 这是 TASK_59 真机踩到的那个面：`{replaces_credential_id: null}` 在"已有一条凭据"时
  // **不匹配当前状态** ⇒ 服务端回 409 且响应里没有任何凭据 id。只认 200 的实现会在这里死。
  const im = fakeIm([
    {
      status: 409,
      body: {
        error: {
          code: 'credential_state_conflict',
          message: 'credential issue or rotation does not match current state',
        },
      },
    },
    LIST_WITH_ACTIVE,
    { status: 201, body: { credential: { id: 'c-new' }, token: 'T-FROM-409' } },
  ])
  const out = await obtainBackendCredential({ ...ARGS, imRequest: im.fn })
  assert.equal(out.ok, true, '409 是"要轮换"，不是"布置完了"')
  assert.equal(out.token, 'T-FROM-409')
  assert.equal(im.calls[1].method, 'GET', '要从现有凭据清单里取 id')
  assert.deepEqual(im.calls[2].options.body, { replaces_credential_id: 'c-old' })
})

pCase('★★ 判错：轮换之后仍拿不到原文 ⇒ 必须**失败**（这条把"只收 2xx"的静默坏路钉死）', async () => {
  // 若实现把 `body.token` 原样交出去，这里会拿到 ok:true + token:undefined ⇒ 断言当场红。
  const im = fakeIm([
    { status: 200, body: { credential: { id: 'c-old' }, token: null } },
    LIST_WITH_ACTIVE,
    { status: 200, body: { credential: { id: 'c-new' }, token: null } },
  ])
  const out = await obtainBackendCredential({ ...ARGS, imRequest: im.fn })
  assert.equal(out.ok, false, '没有原文就不许算"成了"')
  assert.equal(out.token, undefined, '失败路径上不许夹带一个 token 字段')
  assert.match(out.message, /轮换后端凭据失败/)
})

pCase('201 但 token 是空串 ⇒ 同样失败（空凭据 != 好凭据）', async () => {
  const im = fakeIm([{ status: 201, body: { credential: { id: 'c1' }, token: '' } }])
  const out = await obtainBackendCredential({ ...ARGS, imRequest: im.fn })
  assert.equal(out.ok, false)
})

pCase('重放/冲突了，但**没有可替换的凭据** ⇒ 明确失败（读不懂的状态不硬猜）', async () => {
  const im = fakeIm([
    { status: 409, body: { error: { code: 'credential_state_conflict' } } },
    { status: 200, body: { credentials: [] } },
  ])
  const out = await obtainBackendCredential({ ...ARGS, imRequest: im.fn })
  assert.equal(out.ok, false)
  assert.match(out.message, /没有可替换的后端凭据/)
  assert.equal(im.calls.length, 2, '没有可替换的就不该再发第三次')
})

pCase('读现有凭据失败 ⇒ 明确失败（别把读不到当成"没有"）', async () => {
  const im = fakeIm([
    { status: 200, body: { credential: { id: 'c-old' }, token: null } },
    { status: 500, body: { error: { code: 'internal_error' } } },
  ])
  const out = await obtainBackendCredential({ ...ARGS, imRequest: im.fn })
  assert.equal(out.ok, false)
  assert.match(out.message, /读现有后端凭据失败/)
})

pCase('别的状态码（如 403）⇒ 失败并原样带上读数，不去轮换', async () => {
  const im = fakeIm([{ status: 403, body: { error: { code: 'agent_not_active' } } }])
  const out = await obtainBackendCredential({ ...ARGS, imRequest: im.fn })
  assert.equal(out.ok, false)
  assert.match(out.message, /403/)
  assert.match(out.message, /agent_not_active/, '失败读数要带 body，别只给一个码')
  assert.equal(im.calls.length, 1)
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '一条用例都没跑（空跑不算过）')
  assert.ok(cases >= 9, `用例数 ${cases} 少于声明的 9`)
})

pCase('TASK_62：凭据该不该动——Agent 在跑就**绝不动**（三条判据）', () => {
  const { credentialAction } = require('../provision')
  // ① Agent**在跑** ⇒ 无论环境变量里那支可用与否，都绝不动钥匙。
  //    负对照：把 `if (agentRunning) return 'keep-agent-running'` 这一支去掉 ⇒ 下面两条立刻红。
  assert.equal(credentialAction({ agentRunning: true, backendUsable: true }), 'keep-agent-running')
  assert.equal(credentialAction({ agentRunning: true, backendUsable: false }), 'keep-agent-running')
  // ② Agent 不在跑 ＋ 那支可用且属于本家 ⇒ 不动（TASK_59 起就这样，本刀把它写成显式的）
  assert.equal(credentialAction({ agentRunning: false, backendUsable: true }), 'keep-existing')
  // ③ Agent 不在跑 ＋ 那支缺了/认不出 ⇒ 走 TASK_59 的重入语义（读现有 → 换新键轮换）
  assert.equal(credentialAction({ agentRunning: false, backendUsable: false }), 'issue-or-rotate')
})

pCase('TASK_62 补刀：跳过凭据时的读数——环境变量那支认不出时**必须多说一句**', () => {
  const { credentialSkipLines } = require('../provision')

  // ① 那支可用（本家、认得出）⇒ 一行就够，不该吓人
  const quiet = credentialSkipLines({ agentPid: 4321, backendUsable: true })
  assert.equal(quiet.length, 1)
  assert.match(quiet[0], /已在跑（pid 4321）——凭据未动/)
  assert.ok(!quiet.some((line) => line.startsWith('[warn]')), '那支可用时不该出 warn')

  // ② 那支认不出 ⇒ **必须多一句**，而且那句要把"怎么确认、怎么修"说出口。
  //    为什么这条判据重要：那一刻 Agent 可能就是聋的（Agent 不会自己换钥匙），
  //    只打"凭据未动"会被读成"没事"——那是把"不知道"说成了"好"。
  const loud = credentialSkipLines({ agentPid: 4321, backendUsable: false })
  assert.equal(loud.length, 2)
  assert.match(loud[0], /凭据未动/)
  assert.ok(loud[1].startsWith('[warn]'), loud[1])
  assert.match(loud[1], /不会换钥匙/)
  assert.match(loud[1], /down/)
  assert.match(loud[1], /up/)
})

// ── 2026-09-25：凭据的作用域是「Agent × 那个世界」⇒ 按身份分条存；环境变量只读（老路/手工路）──

/** 造一个"写存储"的替身（把写进去的东西记下来）。 */
function fakeWrite() {
  const writes = []
  const fn = (agentId, token) => {
    writes.push({ agentId, token })
    return `(假存储)/${agentId}.token`
  }
  return { fn, writes }
}

pCase('凭据裁决：存储里那份可用 ⇒ 不动（只问一次 `/v1/backend/me`，不写、不轮换）', async () => {
  const { resolveBackendCredential } = require('../provision')
  const im = fakeIm([{ status: 200, body: { agent: { id: 'a1' } } }])
  const write = fakeWrite()
  const out = await resolveBackendCredential({
    base: ARGS.base,
    agentId: 'a1',
    resolved: { token: 'T-STORE', source: 'store', state: 'present' },
    imRequest: im.fn,
    write: write.fn,
  })
  assert.equal(out.backend, 'T-STORE')
  assert.equal(out.backendUsable, true)
  assert.equal(im.calls.length, 1)
  assert.deepEqual(write.writes, [], '权威位置已经在存储里 ⇒ 不该再写一遍')
  assert.deepEqual(out.lines, [])
})

pCase('★ 迁移：只有环境变量那份、且是本家的 ⇒ 原文收进存储，**不轮换**', async () => {
  const { resolveBackendCredential } = require('../provision')
  const im = fakeIm([{ status: 200, body: { agent: { id: 'a1' } } }])
  const write = fakeWrite()
  const out = await resolveBackendCredential({
    base: ARGS.base,
    agentId: 'a1',
    resolved: { token: 'T-ENV', source: 'env', state: 'present' },
    imRequest: im.fn,
    write: write.fn,
  })
  assert.equal(out.backend, 'T-ENV')
  assert.equal(out.backendUsable, true)
  // ⚠ 键是「**世界 × 身份**」（2026-10-01 起）：同一个 agent id 在两个世界里不许撞在同一个文件上。
  assert.deepEqual(
    write.writes,
    [{ agentId: { agentId: 'a1', world: 'http://127.0.0.1:8080' }, token: 'T-ENV' }],
    '原文照搬',
  )
  assert.equal(im.calls.length, 1, '迁移**不发**任何轮换请求（轮换会撤销 Agent 手里那支）')
  assert.match(out.lines.join('\n'), /没有轮换/)
})

pCase('凭据裁决：不属于本家 ⇒ 判成"没有"（并点名来源）；Agent 在跑时读数不一样', async () => {
  const { resolveBackendCredential } = require('../provision')
  const im = fakeIm([
    { status: 200, body: { agent: { id: 'someone-else' } } },
    { status: 200, body: { agent: { id: 'someone-else' } } },
  ])
  const write = fakeWrite()
  const out = await resolveBackendCredential({
    base: ARGS.base,
    agentId: 'a1',
    resolved: { token: 'T-OTHER', source: 'env', state: 'present' },
    imRequest: im.fn,
    write: write.fn,
  })
  assert.equal(out.backend, null, '不是本家的凭据不许当"可用"')
  assert.equal(out.backendUsable, false)
  assert.deepEqual(write.writes, [], '不属于本家 ⇒ 一个字都不许写进存储')
  assert.match(out.lines.join('\n'), /用户环境变量/)
  assert.match(out.lines.join('\n'), /someone-else/)

  const running = await resolveBackendCredential({
    base: ARGS.base,
    agentId: 'a1',
    resolved: { token: 'T-OTHER', source: 'store', state: 'present' },
    agentRunning: true,
    imRequest: im.fn,
    write: write.fn,
  })
  assert.match(running.lines.join('\n'), /本地存储/)
  assert.match(running.lines.join('\n'), /正在跑，先不动它/)
})

pCase('凭据裁决：读不出来 ⇒ 当"没有"处理（**不回落**环境变量，且如实说一句）', async () => {
  const { resolveBackendCredential } = require('../provision')
  const im = fakeIm([])
  const out = await resolveBackendCredential({
    base: ARGS.base,
    agentId: 'a1',
    resolved: { token: null, source: null, state: 'unreadable', reason: 'empty' },
    imRequest: im.fn,
    write: fakeWrite().fn,
  })
  assert.equal(out.backend, null)
  assert.equal(out.backendUsable, false)
  assert.equal(im.calls.length, 0, '没有可用的凭据 ⇒ 一次请求都不该发')
  assert.match(out.lines.join('\n'), /读不出来/)
})

pCase('凭据裁决：迁移写失败 ⇒ 仍然用环境变量那支（如实说一句，不把它判成不可用）', async () => {
  const { resolveBackendCredential } = require('../provision')
  const im = fakeIm([{ status: 200, body: { agent: { id: 'a1' } } }])
  const out = await resolveBackendCredential({
    base: ARGS.base,
    agentId: 'a1',
    resolved: { token: 'T-ENV', source: 'env', state: 'present' },
    imRequest: im.fn,
    write: () => {
      throw new Error('磁盘满了')
    },
  })
  assert.equal(out.backend, 'T-ENV')
  assert.equal(out.backendUsable, true)
  assert.match(out.lines.join('\n'), /没成功/)
  assert.match(out.lines.join('\n'), /磁盘满了/)
})

// ─────────────────────────────────────────────────────────────────────────────
// **远端跑法**（2026-09-30）：`ctx.remote = { base, token }` —— 对着**那个世界**布置
//
// 三条要钉死的事实：
//  ① 每一句话都对**那个世界**说（`base` 是远端地址），一次都不许打回本机 listen；
//  ② **本机不碰 core 的 CLI**（runtime 子包没有它）—— 注入一个"一被调用就炸"的 runCli，
//     只有 agent_run 那一步能过；
//  ③ 缺主人凭据时**停手**，且停在任何写动作之前（不建 Agent、不建 binding、不开房）。
// ─────────────────────────────────────────────────────────────────────────────

const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

/** 造一个一次性的家（conf + 目录），返回 { homeDir, confFile, envDir, credsDir }。 */
function scratchHome(label, confLines = []) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), `celira-prov-${label}-`))
  const homeDir = path.join(root, 'home')
  const envDir = path.join(homeDir, 'agent-env')
  const credsDir = path.join(root, 'creds')
  fs.mkdirSync(envDir, { recursive: true })
  fs.mkdirSync(credsDir, { recursive: true })
  const confFile = path.join(homeDir, 'home.conf')
  fs.writeFileSync(confFile, ['listen_addr=127.0.0.1:8080', 'client_port=5173', ...confLines, ''].join('\n'), 'utf8')
  return { root, homeDir, envDir, credsDir, confFile }
}

/**
 * **按路由**应答的假世界（比"按次序"稳：布置的步骤顺序会变，路由不会）。
 *
 * 每个键是 `METHOD 路径`；值可以是回执，也可以是 `(call) => 回执`。没预置的那一句当场炸，
 * 并把"已经说过的每一句"带出来 —— 判错时一眼看清多说了什么。
 */
function fakeWorld(routes = {}) {
  const calls = []
  const fn = async (base, method, url, options) => {
    calls.push({ base, method, url, options })
    const hit = routes[`${method} ${url}`]
    if (!hit) {
      throw new Error(`假世界没有预置「${method} ${url}」；已经说过：\n${calls.map((c) => `  ${c.method} ${c.url}`).join('\n')}`)
    }
    return typeof hit === 'function' ? hit({ base, method, url, options }) : hit
  }
  return { fn, calls }
}

/** 一次成功的远端布置要用的那套路由（世界里还没有这个 Agent 的那条路）。 */
function happyRemoteWorld(agentId = 'a-remote') {
  return {
    'POST /v1/agents': { status: 201, body: { agent: { id: agentId } } },
    'GET /v1/backend/me': { status: 200, body: { agent: { id: agentId, display_name: '小满' } } },
    [`POST /v1/agents/${agentId}/backend-bindings`]: { status: 201, body: { binding: { id: `b-${agentId}` } } },
    [`POST /v1/agents/${agentId}/backend-bindings/b-${agentId}/credentials`]: {
      status: 201,
      body: { credential: { id: `c-${agentId}` }, token: 'clr_bc_first' },
    },
    'POST /v1/direct-rooms': { status: 201, body: { room: { id: `r-${agentId}` } } },
    // **世界的现状**（2026-10-13）：布置现在**无条件先读一次**再决定要不要写。
    // 「世界里还没有这一对的房」⇒ 照旧去写（与从前逐字相同的行为）。
    'GET /v1/rooms': { status: 200, body: { rooms: [] } },
  }
}

pCase('★ 远端布置（给了主人凭据）：每一句都对**那个世界**说，本机不碰 core 的 CLI', async () => {
  const home = scratchHome('remote-ok')
  process.env.CELIRA_CREDENTIALS_DIR = home.credsDir
  try {
    const im = fakeWorld(happyRemoteWorld())
    const cli = []
    const out = await provision({
      confFile: home.confFile,
      homeDir: home.homeDir,
      // **故意指向不存在的东西**：远端跑法下本机根本没有 core（runtime 子包就是缺它）。
      imExe: path.join(home.root, 'no-such-celira-im'),
      runExe: '/fake/agent_run',
      imEnv: {},
      // 全新的家、第一支住户 ⇒ 住户序号 1（键形状：住户 1 **无后缀**）。
      slot: 1,
      remote: { base: 'http://world.example:8080', token: 'clr_hs_owner' },
      imRequest: im.fn,
      runCli: (exe, args) => {
        cli.push({ exe, args })
        if (exe !== '/fake/agent_run') throw new Error(`远端跑法不该起这个 CLI：${exe}`)
        // agent_run init 的替身：把环境"建"出来（真实现写 identity.json）。
        fs.writeFileSync(path.join(args[args.indexOf('--root') + 1], 'identity.json'), '{}', 'utf8')
        return { code: 0, stdout: '', stderr: '' }
      },
    })
    assert.equal(out.ok, true, JSON.stringify(out))

    // ① 对谁说：全部请求都打远端，一次都没打回本机。
    assert.deepEqual(
      im.calls.map((c) => `${c.method} ${c.url}`),
      [
        'POST /v1/agents',
        'POST /v1/agents/a-remote/backend-bindings',
        'POST /v1/agents/a-remote/backend-bindings/b-a-remote/credentials',
        // ⚠ **2026-10-13 多这一句**：开房之前**先读一次世界的现状**（"这一对有没有房"是世界的事实，
        // 而键是附注）——从前只有"认领"那条路读，首装／重跑那条路不看就写，于是被**上一次半途而废
        // 的尝试**占住的键会把家判死（真机两例）。读到了就不写，读不到才写（下一句）。
        'GET /v1/rooms',
        'POST /v1/direct-rooms',
      ],
      '对着世界说的每一句话都要能一眼看懂（多一句少一句都算错）',
    )
    for (const call of im.calls) assert.equal(call.base, 'http://world.example:8080')
    assert.ok(!im.calls.some((c) => c.base.includes('127.0.0.1:8080')), '本机 listen 一次都不许出现')

    // ② 用哪支凭据：主人凭据从参数进（不是本机铸的）。
    assert.equal(im.calls[0].options.bearer, 'clr_hs_owner')

    // ③ 本机 CLI：只该有 agent_run 那一次（`bootstrap-human` / `issue-human-session` 全跳过）。
    assert.deepEqual(cli.map((c) => c.exe), ['/fake/agent_run'])
    assert.deepEqual(cli[0].args.slice(0, 1), ['init'])

    // ④ 记账：世界里那三样落进**支级配置**（`<环境根>/agent.conf`）；`home.conf` 只留机器级事实。
    const agentConf = fs.readFileSync(path.join(envRootOf(home, 'a-remote'), 'agent.conf'), 'utf8')
    assert.match(agentConf, /agent_id=a-remote/)
    assert.match(agentConf, /room_id=r-a-remote/)
    assert.match(agentConf, /slot=1/)
    const conf = fs.readFileSync(home.confFile, 'utf8')
    assert.ok(!/human_id/.test(conf), `远端跑法不许在本地记人：${conf}`)
    assert.ok(!/agent_id=/.test(conf), `身份 / 房不许留在 home.conf（住户化之后它们是支级事实）：${conf}`)

    // ⑤ 后端凭据落在**本地存储**（按「世界 × 身份」分条）——世界里那支原文只出现一次。
    assert.match(credentialOf(home, 'a-remote'), /clr_bc_first/)
    const stored = fs.readdirSync(home.credsDir)
    assert.equal(stored.length, 1, `凭据存储应当只有"世界摘要"这一层：${JSON.stringify(stored)}`)
    assert.equal(fs.readdirSync(path.join(home.credsDir, stored[0])).length, 1, '一个世界一个子目录，一支一个文件')
  } finally {
    delete process.env.CELIRA_CREDENTIALS_DIR
  }
})

pCase('★ 远端布置（**没给**主人凭据）⇒ 停在动手之前：不建 Agent、不建 binding、不开房，一个字都不写', async () => {
  const home = scratchHome('remote-notoken')
  process.env.CELIRA_CREDENTIALS_DIR = home.credsDir
  try {
    const im = fakeIm([])
    const before = fs.readFileSync(home.confFile, 'utf8')
    const out = await provision({
      confFile: home.confFile,
      envDir: home.envDir,
      homeDir: home.homeDir,
      imExe: path.join(home.root, 'no-such-celira-im'),
      runExe: '/fake/agent_run',
      imEnv: {},
      remote: { base: 'http://world.example:8080', token: null },
      imRequest: im.fn,
      runCli: () => {
        throw new Error('没有主人凭据 ⇒ 连 agent_run 都不该起')
      },
    })
    assert.equal(out.ok, false)
    assert.match(String(out.message), /还需要主人凭据/)
    assert.match(String(out.message), /CELIRA_IM_HUMAN_AUTH/, '要给出第二来源（环境变量）')
    assert.equal(im.calls.length, 0, '一次请求都不该发')
    assert.equal(fs.readFileSync(home.confFile, 'utf8'), before, 'home.conf 一个字都不该动')
  } finally {
    delete process.env.CELIRA_CREDENTIALS_DIR
  }
})

pCase('★ 远端布置（重跑：家里已经布置好、凭据也在）⇒ 不给主人凭据也走得完，且一次世界写入都没有', async () => {
  // ⚠ 这里必须带上 `home_id`：**布置好的家**现在都有家 id（2026-10-01 起）。
  // "没有家 id"的那种形状叫**老家**——它的重跑行为是另一族用例（见 `residents.test.js`），别混。
  //
  // 住户化之后（本刀）"家里已经布置好"的形状 = **一支住户**（`<家>/agents/<支>/agent.conf`）
  // ⇒ 重跑时把它作为 `ctx.resident` 交进来（`up` 就是这么做的），身份 / 房一个字都不用问世界。
  const home = scratchHome('remote-rerun', ['home_id=H-rerun'])
  process.env.CELIRA_CREDENTIALS_DIR = home.credsDir
  try {
    const envRoot = envRootOf(home, 'a-remote')
    fs.mkdirSync(envRoot, { recursive: true })
    fs.writeFileSync(
      path.join(envRoot, 'agent.conf'),
      'agent_id=a-remote\nroom_id=r-remote\nslot=1\n',
      'utf8',
    )
    const credPath = credentialPathOf(home, 'a-remote')
    fs.mkdirSync(path.dirname(credPath), { recursive: true })
    fs.writeFileSync(credPath, 'clr_bc_existing\n', 'utf8')
    const im = fakeWorld({ 'GET /v1/backend/me': { status: 200, body: { agent: { id: 'a-remote' } } } }) // 只有"问一句凭据代不代表本家"
    const out = await provision({
      confFile: home.confFile,
      homeDir: home.homeDir,
      imExe: path.join(home.root, 'no-such-celira-im'),
      runExe: '/fake/agent_run',
      imEnv: {},
      resident: residentOf(home, 'a-remote'),
      remote: { base: 'http://world.example:8080', token: null },
      imRequest: im.fn,
      runCli: (exe, args) => {
        fs.writeFileSync(path.join(args[args.indexOf('--root') + 1], 'identity.json'), '{}', 'utf8')
        return { code: 0, stdout: '', stderr: '' }
      },
    })
    assert.equal(out.ok, true, JSON.stringify(out))
    assert.equal(out.agentId, 'a-remote', '重跑必须还是同一支（住户说了算，不问世界）')
    assert.equal(out.roomId, 'r-remote')
    assert.equal(im.calls.length, 1, `重跑只该问一句：${im.calls.map((c) => c.method + ' ' + c.url).join(' / ')}`)
    assert.equal(im.calls[0].method, 'GET')
  } finally {
    delete process.env.CELIRA_CREDENTIALS_DIR
  }
})

pCase('远端布置（认领）：先问清"世界里那个身份在不在、是不是你的"，再认领 —— 都在**那个世界**上问', async () => {
  const home = scratchHome('remote-adopt')
  process.env.CELIRA_CREDENTIALS_DIR = home.credsDir
  try {
    const im = fakeWorld({
      // verifyOwnedAgent：GET /v1/agents 找在场（认领的判据在 adopt.js）
      'GET /v1/agents': { status: 200, body: { agents: [{ agent: { id: 'a-old', display_name: '小满', status: 'active' } }] } },
      'GET /v1/backend/me': { status: 200, body: { agent: { id: 'a-old', display_name: '小满' } } },
      // findExistingDirectRoom：那间**已有**的房（认领要沿用它；形状 = {room:{kind,id}, peer:{id}}）
      'GET /v1/rooms': {
        status: 200,
        body: { rooms: [{ room: { id: 'r-existing', kind: 'direct' }, peer: { id: 'a-old' } }] },
      },
      'POST /v1/agents/a-old/backend-bindings': { status: 201, body: { binding: { id: 'b-old' } } },
      'POST /v1/agents/a-old/backend-bindings/b-old/credentials': {
        status: 201,
        body: { credential: { id: 'c-old' }, token: 'clr_bc_adopted' },
      },
    })
    const out = await provision({
      confFile: home.confFile,
      homeDir: home.homeDir,
      imExe: path.join(home.root, 'no-such-celira-im'),
      runExe: '/fake/agent_run',
      imEnv: {},
      adoptAgentId: 'a-old',
      // 认领进来的身份成为**第一支住户**（键形状无后缀）。
      slot: 1,
      remote: { base: 'http://world.example:8080', token: 'clr_hs_owner' },
      imRequest: im.fn,
      runCli: (exe, args) => {
        fs.writeFileSync(path.join(args[args.indexOf('--root') + 1], 'identity.json'), '{}', 'utf8')
        return { code: 0, stdout: '', stderr: '' }
      },
    })
    assert.equal(out.ok, true, JSON.stringify(out))
    for (const call of im.calls) assert.equal(call.base, 'http://world.example:8080')
    const agentConf = fs.readFileSync(path.join(envRootOf(home, 'a-old'), 'agent.conf'), 'utf8')
    assert.match(agentConf, /agent_id=a-old/)
    assert.match(agentConf, /room_id=r-existing/, '认领要用世界里**已有**那间房（别多开一间）')
  } finally {
    delete process.env.CELIRA_CREDENTIALS_DIR
  }
})

pCase('★ 半成品住户（写了 agent.conf 的手、没身份没住户序号）⇒ 认领 ＋ 补序号，**手原样留着**', async () => {
  // 2026-10-01 第五刀真机发现的坑：用户先给第二支写 `agents/<支>/agent.conf`（手、显示名），
  // 再 `up --agent <它>`。从前 provision 按"目录在"判成"已经安过家" ⇒ 接着读不到 slot 就停手，
  // 要求用户手工补 `slot=`。正确动作：**把它安下来**（问世界 ⇒ 写身份 ⇒ 补住户序号），
  // 已经写在 agent.conf 里的手与显示名一个字都不许丢。
  const home = scratchHome('half-built')
  process.env.CELIRA_CREDENTIALS_DIR = home.credsDir
  try {
    const envDir = path.join(home.homeDir, 'agents', 'a-half')
    fs.mkdirSync(envDir, { recursive: true })
    fs.writeFileSync(
      path.join(envDir, 'agent.conf'),
      ['display_name=第二支', 'mcp=none', 'terminal=none', 'note=keep-me', ''].join('\n'),
      'utf8',
    )
    const im = fakeWorld({
      'GET /v1/agents': { status: 200, body: { agents: [{ agent: { id: 'a-half', display_name: '第二支', status: 'active' } }] } },
      'GET /v1/backend/me': { status: 200, body: { agent: { id: 'a-half', display_name: '第二支' } } },
      'GET /v1/rooms': { status: 200, body: { rooms: [] } },
      'POST /v1/agents/a-half/backend-bindings': { status: 201, body: { binding: { id: 'b-half' } } },
      'POST /v1/agents/a-half/backend-bindings/b-half/credentials': {
        status: 201,
        body: { credential: { id: 'c-half' }, token: 'clr_bc_half' },
      },
      'POST /v1/direct-rooms': { status: 201, body: { room: { id: 'r-half' } } },
    })
    const out = await provision({
      confFile: home.confFile,
      homeDir: home.homeDir,
      imExe: path.join(home.root, 'no-such-celira-im'),
      runExe: '/fake/agent_run',
      imEnv: {},
      resident: { agentId: 'a-half', envDir, slot: null, roomId: '', displayName: '第二支', legacy: false },
      adoptAgentId: 'a-half',
      slot: 2,
      remote: { base: 'http://world.example:8080', token: 'clr_hs_owner' },
      imRequest: im.fn,
      runCli: (exe, args) => {
        fs.writeFileSync(path.join(args[args.indexOf('--root') + 1], 'identity.json'), '{}', 'utf8')
        return { code: 0, stdout: '', stderr: '' }
      },
    })
    assert.equal(out.ok, true, JSON.stringify(out))
    assert.equal(out.agentId, 'a-half', '认领的是世界里那一支（不新建身份）')
    assert.ok(
      im.calls.some((call) => call.url === '/v1/agents'),
      `要先问世界"这一支在不在、是不是你的"：${im.calls.map((c) => `${c.method} ${c.url}`).join(' / ')}`,
    )
    const agentConf = fs.readFileSync(path.join(envDir, 'agent.conf'), 'utf8')
    assert.match(agentConf, /agent_id=a-half/, '身份写进这一支的 agent.conf')
    assert.match(agentConf, /slot=2/, '住户序号补上了（不再停手要用户手改）')
    assert.match(agentConf, /mcp=none/, '这一支的手不许被吞')
    assert.match(agentConf, /terminal=none/, '终端档不许被吞')
    assert.match(agentConf, /note=keep-me/, '别人的行也不许被吞')
    assert.match(agentConf, /display_name=第二支/, '显示名沿用用户写的那一行')
  } finally {
    delete process.env.CELIRA_CREDENTIALS_DIR
  }
})

pCase('远端布置（认领 + 没凭据）⇒ 停手（问都别问：认领也要主人凭据）', async () => {
  const home = scratchHome('remote-adopt-notoken')
  process.env.CELIRA_CREDENTIALS_DIR = home.credsDir
  try {
    const im = fakeIm([])
    const out = await provision({
      confFile: home.confFile,
      envDir: home.envDir,
      homeDir: home.homeDir,
      imExe: path.join(home.root, 'no-such-celira-im'),
      runExe: '/fake/agent_run',
      imEnv: {},
      adoptAgentId: 'a-old',
      remote: { base: 'http://world.example:8080', token: null },
      imRequest: im.fn,
    })
    assert.equal(out.ok, false)
    assert.match(String(out.message), /还需要主人凭据/)
    assert.equal(im.calls.length, 0)
  } finally {
    delete process.env.CELIRA_CREDENTIALS_DIR
  }
})

// ─────────────────────────────────────────────────────────────────────────────
// **幂等键按「家」取**（2026-10-01）：一个世界住多个家
//
// 三条要钉死的事实（都是真机症状的机制层）：
//  ① 两个家接同一个世界 ⇒ 各建各的（两支 Agent / 两间房 / 两支凭据）；
//  ② 同一个家重跑 ⇒ 幂等（同一支 agent_id，不新建；凭据丢了也只轮换自己那支）；
//  ③ 老家（home.conf 里没有家 id）⇒ 生成一个、世界里**另建一支 Agent**（旧那支留着不动）；
//     旧那支还在跑 / 远端没有主人凭据 ⇒ 拒绝，一个字都不改。
//
// 判据的关键在假世界：它把世界侧那几条幂等语义照搬进来（按**提交原文**比较、按 Human 记账；
// binding 跨 Agent 全局；凭据按 Human × key；direct 房一对一间）。键若还是世界级常量，
// 下面第①条会当场红（B 与 C 拿到同一支 Agent —— 那正是旧行为）。
// ─────────────────────────────────────────────────────────────────────────────

/**
 * **有状态的假世界**：只实现布置用到的那几条路由，语义照世界侧。
 * 返回 `{ fn, calls, agentsByKey, rooms, ... }`（`calls` 是每一句的原样记录）。
 */
function statefulWorld({ human = 'h1' } = {}) {
  const calls = []
  const agentsByKey = new Map() // `${human}|${key}` -> { id }
  const bindingsByKey = new Map() // `${human}|${key}` -> { id, agentId }
  const bindingsByAgent = new Map() // agentId -> binding id（一个 Agent 至多一条 active binding）
  const credsByKey = new Map() // `${human}|${key}` -> credential（签发记录）
  const credsByToken = new Map() // token -> credential
  const credsByBinding = new Map() // bindingId -> [credential]
  const roomsByKey = new Map() // `${human}|${key}` -> { id, agentId }
  const roomsByPair = new Map() // `${human}|${agentId}` -> room id
  const rooms = new Map() // roomId -> { id, agentId }
  let seq = 0
  const nextId = (prefix) => `${prefix}-${++seq}`
  const conflict = () => ({ status: 409, body: { error: { code: 'idempotency_conflict' } } })

  const fn = async (base, method, url, options) => {
    calls.push({ base, method, url, options })
    const key = options?.key ?? null
    if (method === 'POST' && url === '/v1/agents') {
      const existing = agentsByKey.get(`${human}|${key}`)
      if (existing) return { status: 200, body: { agent: { id: existing.id }, idempotency_replayed: true } }
      const id = nextId('a')
      agentsByKey.set(`${human}|${key}`, { id })
      return { status: 201, body: { agent: { id } } }
    }
    if (method === 'GET' && url === '/v1/backend/me') {
      const cred = credsByToken.get(options?.bearer)
      if (!cred || cred.status !== 'active') return { status: 401, body: { error: { code: 'invalid_credential' } } }
      return { status: 200, body: { agent: { id: cred.agentId } } }
    }
    const binding = /^\/v1\/agents\/([^/]+)\/backend-bindings$/.exec(url)
    if (method === 'POST' && binding) {
      const agentId = binding[1]
      const existing = bindingsByKey.get(`${human}|${key}`)
      if (existing) {
        if (existing.agentId !== agentId) return conflict()
        return { status: 200, body: { binding: { id: existing.id }, idempotency_replayed: true } }
      }
      if (bindingsByAgent.has(agentId)) return { status: 409, body: { error: { code: 'binding_exists' } } }
      const id = nextId('b')
      bindingsByKey.set(`${human}|${key}`, { id, agentId })
      bindingsByAgent.set(agentId, id)
      return { status: 201, body: { binding: { id } } }
    }
    const credentials = /^\/v1\/agents\/([^/]+)\/backend-bindings\/([^/]+)\/credentials$/.exec(url)
    if (credentials) {
      const [, agentId, bindingId] = credentials
      const list = credsByBinding.get(bindingId) ?? []
      if (method === 'GET') {
        return { status: 200, body: { credentials: list.map((c) => ({ id: c.id, status: c.status })) } }
      }
      if (method === 'POST') {
        const replaces = options?.body?.replaces_credential_id ?? null
        const existing = credsByKey.get(`${human}|${key}`)
        if (existing) {
          const same = existing.bindingId === bindingId && existing.agentId === agentId && existing.replaces === replaces
          if (!same) return conflict()
          return { status: 200, body: { credential: { id: existing.id }, token: null, idempotency_replayed: true } }
        }
        if (replaces === null) {
          if (list.some((c) => c.status === 'active')) {
            return { status: 409, body: { error: { code: 'credential_state_conflict' } } }
          }
        } else {
          const target = list.find((c) => c.id === replaces && c.status === 'active')
          if (!target) return { status: 409, body: { error: { code: 'credential_state_conflict' } } }
          target.status = 'revoked'
        }
        const id = nextId('c')
        const token = `clr_bc_${id}`
        const cred = { id, bindingId, agentId, token, status: 'active', replaces }
        credsByKey.set(`${human}|${key}`, cred)
        credsByToken.set(token, cred)
        list.push(cred)
        credsByBinding.set(bindingId, list)
        return { status: 201, body: { credential: { id }, token } }
      }
    }
    if (method === 'GET' && url === '/v1/rooms') {
      // 世界的现状：谁跟谁的那间 direct room（`findExistingDirectRoom` 在返回里按
      // `room.kind` ＋ `peer.id` 匹，所以这里给全量，过滤归它）。
      const entries = [...rooms.values()].map((r) => ({
        room: { id: r.id, kind: 'direct', status: 'active', name: null },
        peer: { id: r.agentId, kind: 'agent', status: 'active' },
      }))
      return { status: 200, body: { rooms: entries } }
    }
    if (method === 'POST' && url === '/v1/direct-rooms') {
      const agentId = options?.body?.agent_id
      const existing = roomsByKey.get(`${human}|${key}`)
      if (existing) {
        if (existing.agentId !== agentId) return conflict()
        return { status: 200, body: { room: { id: existing.id }, idempotency_replayed: true } }
      }
      const held = roomsByPair.get(`${human}|${agentId}`)
      if (held) return { status: 200, body: { room: { id: held }, idempotency_replayed: true } }
      const id = nextId('r')
      roomsByKey.set(`${human}|${key}`, { id, agentId })
      roomsByPair.set(`${human}|${agentId}`, id)
      rooms.set(id, { id, agentId })
      return { status: 201, body: { room: { id } } }
    }
    throw new Error(`假世界没有这条路由：${method} ${url}（已经说过：${calls.map((c) => c.method + ' ' + c.url).join(' / ')}）`)
  }
  return { fn, calls, agentsByKey, bindingsByKey, bindingsByAgent, rooms, roomsByKey, credsByBinding, credsByToken }
}

/** `home.conf` 读成一张表（测试自己的读数，不动 conf.js 的口径）。 */
function confMap(file) {
  const out = {}
  for (const line of fs.readFileSync(file, 'utf8').split('\n')) {
    const at = line.indexOf('=')
    if (at > 0) out[line.slice(0, at)] = line.slice(at + 1)
  }
  return out
}

/** 跑一次这个家的布置（远端跑法；凭据目录按家切换）。 */
async function provisionHome(
  world,
  home,
  { homeId, token = 'clr_hs_owner', agentRunning = false, resident = null, slot = 1, carryBindingFrom = null, agentName = null } = {},
) {
  const previous = process.env.CELIRA_CREDENTIALS_DIR
  process.env.CELIRA_CREDENTIALS_DIR = home.credsDir
  try {
    return await provision({
      confFile: home.confFile,
      homeDir: home.homeDir,
      imExe: path.join(home.root, 'no-such-celira-im'),
      runExe: '/fake/agent_run',
      imEnv: {},
      agentRunning,
      // **住户化之后**（2026-10-01）：环境根由布置按住户算（`<家>/agents/<支>/`，与壳同一套布局），
      // `ctx.envDir` 不再被读；**住户序号由调用方给**（这里是"第一支" = 1）。
      resident,
      slot,
      carryBindingFrom,
      agentName,
      remote: { base: 'http://world.example:8080', token },
      imRequest: world.fn,
      newHomeId: homeId === undefined ? undefined : () => homeId,
      runCli: (exe, args) => {
        // `agent_run init` 的替身：真实现会**建出环境根**再写身份 —— 替身也照做
        // （老家那一族用例里环境根刚被整份让位 ⇒ 这一步得真的把它建回来）。
        const root = args[args.indexOf('--root') + 1]
        fs.mkdirSync(root, { recursive: true })
        fs.writeFileSync(path.join(root, 'identity.json'), '{}', 'utf8')
        return { code: 0, stdout: '', stderr: '' }
      },
    })
  } finally {
    if (previous === undefined) delete process.env.CELIRA_CREDENTIALS_DIR
    else process.env.CELIRA_CREDENTIALS_DIR = previous
  }
}

/** 一支住户的**环境根**（`<家>/agents/<支>/`；与 `paths.agentEnvRoot` 同一份口径）。 */
function envRootOf(home, agentId) {
  return path.join(home.homeDir, 'agents', agentId)
}

/** 一支住户的**支级配置**（`<环境根>/agent.conf`：身份 / 值班室 / 住户序号 / 显示名）。 */
function agentConfMap(home, agentId) {
  return confMap(path.join(envRootOf(home, agentId), 'agent.conf'))
}

/** 一支住户的**后端凭据**路径（「世界 × 身份」分条；同一个口径在 `credentials.js`）。 */
function credentialPathOf(home, agentId, world = 'http://world.example:8080') {
  const previous = process.env.CELIRA_CREDENTIALS_DIR
  process.env.CELIRA_CREDENTIALS_DIR = home.credsDir
  try {
    return require('../credentials').credentialFile({ world, agentId })
  } finally {
    if (previous === undefined) delete process.env.CELIRA_CREDENTIALS_DIR
    else process.env.CELIRA_CREDENTIALS_DIR = previous
  }
}

/** 读一支住户的凭据原文（判据用；**不打印**）。 */
function credentialOf(home, agentId, world = 'http://world.example:8080') {
  return fs.readFileSync(credentialPathOf(home, agentId, world), 'utf8').trim()
}

/** 把 `home.conf` 里那几行（住户化之后只剩机器级事实）读成一张表。 */
function homeConfMap(home) {
  return confMap(home.confFile)
}

/** 一支**已有住户**（`up` 读到的形状）：交给 `provision` 的 `ctx.resident`。 */
function residentOf(home, agentId) {
  const { runtimeResidents } = require('../residents')
  const list = runtimeResidents(home.homeDir, home.confFile)
  const hit = list.find((resident) => resident.agentId === agentId)
  if (!hit) throw new Error(`住户表里没有 ${agentId}：${list.map((r) => r.agentId).join(' / ')}`)
  return hit
}

/** 把一段运行的读数抓下来（布置把读数打在 console 上）。 */
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

pCase('布置键的形状：`<基础名>:<家 id>` —— 一眼读得出属于哪个家，且符合世界的键纪律', () => {
  const { provisioningKeys } = require('../provision')
  const homeId = '3f2504e0-4f89-41d3-9a0c-0305e82c3301'
  const keys = provisioningKeys(homeId)
  assert.deepEqual(keys, {
    agent: `home-agent:${homeId}`,
    binding: `home-binding:${homeId}`,
    credential: `home-credential:${homeId}`,
    direct: `home-direct:${homeId}`,
  })
  for (const value of Object.values(keys)) {
    // 世界侧的形状纪律：1–128 个可见 ASCII（0x21–0x7E）、不含逗号、无首尾空白。
    assert.ok(/^[\x21-\x7E]+$/.test(value), `可见 ASCII：${value}`)
    assert.ok(!value.includes(','), `不含逗号：${value}`)
    assert.ok(value.length <= 128, `不超过 128 字节：${value.length}`)
    assert.ok(value.endsWith(`:${homeId}`), '键里要读得出它属于哪个家')
  }
})

pCase('家 id：已有 ⇒ 逐字读回（只读）；没有 ⇒ 裁决"等第一次用键时再生成"（本地一个字不动）', () => {
  const { ensureHomeId } = require('../provision')
  const home = scratchHome('hid-existing', ['home_id=H-EXIST', 'agent_id=a-x'])
  const before = fs.readFileSync(home.confFile, 'utf8')
  const out = ensureHomeId({ confFile: home.confFile })
  assert.equal(out.homeId, 'H-EXIST')
  assert.equal(out.refuse, null)
  assert.deepEqual(out.lines, [])
  assert.equal(fs.readFileSync(home.confFile, 'utf8'), before, '只读：一个字都不该动')

  // 没有家 id、也没有世界侧身份（全新的家）⇒ 裁决为"还没有"：生成推迟到第一次用键时，
  // 这里（以及"停在动手之前"那些路）连本地都不写。
  const fresh = scratchHome('hid-fresh')
  const freshBefore = fs.readFileSync(fresh.confFile, 'utf8')
  const pending = ensureHomeId({ confFile: fresh.confFile })
  assert.equal(pending.homeId, null)
  assert.equal(pending.refuse, null)
  assert.deepEqual(pending.lines, [])
  assert.equal(fs.readFileSync(fresh.confFile, 'utf8'), freshBefore, '全新的家：这一步一个字都不该写')
})

pCase('★★ 两个家接同一个世界：各建各的（两支 Agent / 两间房 / 两支凭据），键里带得出家', async () => {
  const world = statefulWorld()
  const homeB = scratchHome('multi-b')
  const homeC = scratchHome('multi-c')
  const outB = await provisionHome(world, homeB, { homeId: 'H-B' })
  const outC = await provisionHome(world, homeC, { homeId: 'H-C' })
  assert.equal(outB.ok, true, JSON.stringify(outB))
  assert.equal(outC.ok, true, JSON.stringify(outC))
  const confB = homeConfMap(homeB)
  const confC = homeConfMap(homeC)
  assert.equal(confB.home_id, 'H-B')
  assert.equal(confC.home_id, 'H-C')
  assert.equal(confB.agent_id, undefined, 'home.conf 里不该再有身份（住户化之后是支级事实）')
  const agentB = agentConfMap(homeB, outB.agentId)
  const agentC = agentConfMap(homeC, outC.agentId)
  assert.notEqual(agentB.agent_id, agentC.agent_id, '两个家必须拿到**不同**的 Agent 身份')
  assert.notEqual(agentB.room_id, agentC.room_id, '两间值班室必须不同')
  assert.equal(agentB.slot, '1')
  assert.equal(agentC.slot, '1')
  // 两支后端凭据：都在、各自代表本家、互不相同。
  const tokenB = credentialOf(homeB, agentB.agent_id)
  const tokenC = credentialOf(homeC, agentC.agent_id)
  assert.notEqual(tokenB, tokenC, '两支后端凭据必须不同')
  assert.equal((await world.fn('w', 'GET', '/v1/backend/me', { bearer: tokenB })).body.agent.id, agentB.agent_id)
  assert.equal((await world.fn('w', 'GET', '/v1/backend/me', { bearer: tokenC })).body.agent.id, agentC.agent_id)
  // **同一个世界、两支不同的身份 ⇒ 凭据不撞车**（本刀：键加了世界那一维）：
  // 世界摘要那一段由**世界地址**决定（两家对的是同一个世界 ⇒ 同一段），
  // 文件名那一段由**身份**决定（两支不同 ⇒ 两个文件）。两个家的凭据目录本来就不同（各自的机器级目录）。
  assert.equal(
    path.basename(path.dirname(credentialPathOf(homeB, agentB.agent_id))),
    path.basename(path.dirname(credentialPathOf(homeC, agentC.agent_id))),
    '同一个世界 ⇒ 同一个世界摘要',
  )
  assert.notEqual(
    path.basename(credentialPathOf(homeB, agentB.agent_id)),
    path.basename(credentialPathOf(homeC, agentC.agent_id)),
    '两支身份 ⇒ 两个文件',
  )
  // 键：每个家发出去的每一把都带得出它属于哪个家；裸的世界级常量**不许再出现**。
  const usedKeys = new Set(world.calls.map((c) => c.options?.key).filter(Boolean))
  for (const base of ['home-agent', 'home-binding', 'home-credential', 'home-direct']) {
    assert.ok(usedKeys.has(`${base}:H-B`), `B 家应当发过 ${base}:H-B`)
    assert.ok(usedKeys.has(`${base}:H-C`), `C 家应当发过 ${base}:H-C`)
    assert.ok(!usedKeys.has(base), `裸的 ${base} 不许再出现（那是世界级常量）`)
  }
  // 负对照：两家各有各的 Agent（世界里两条记录，不共用）。
  assert.equal(world.agentsByKey.size, 2)
})

pCase('★★ 同一个家重跑：幂等（同一支 agent_id，不新建；凭据丢了也只轮换自己那支）', async () => {
  const world = statefulWorld()
  const homeB = scratchHome('rerun-b')
  const first = await provisionHome(world, homeB, { homeId: 'H-B' })
  assert.equal(first.ok, true, JSON.stringify(first))
  // 凭据丢了（重跑时会被重新签发）—— 这正是旧行为里"轮换"那条路的入口。
  fs.rmSync(credentialPathOf(homeB, first.agentId))
  const out = await provisionHome(world, homeB, { homeId: 'H-B' })
  assert.equal(out.ok, true, JSON.stringify(out))
  const second = out
  assert.equal(second.agentId, first.agentId, '重跑必须还是同一支 agent_id')
  assert.equal(second.roomId, first.roomId, '重跑必须还是同一间房')
  assert.equal(world.agentsByKey.size, 1, '世界里不许多出 Agent')
  const agentPosts = world.calls.filter((c) => c.method === 'POST' && c.url === '/v1/agents')
  assert.equal(agentPosts.length, 2, '第二次那句是同键重放（世界里不许真的多一支）')
  assert.equal(world.agentsByKey.size, 1)
  // 重新签发的那支凭据：还是代表本家（换的是凭据，不是身份）。
  const token = credentialOf(homeB, second.agentId)
  assert.equal((await world.fn('w', 'GET', '/v1/backend/me', { bearer: token })).body.agent.id, second.agentId)
  // 键还是同一个家的那几把（重放）；轮换那把钉在被替换的凭据 id 上。
  const usedKeys = world.calls.map((c) => c.options?.key).filter(Boolean)
  assert.ok(usedKeys.includes('home-agent:H-B'))
  assert.ok(usedKeys.includes('home-binding:H-B'))
  assert.ok(usedKeys.includes('home-credential:H-B'))
  assert.ok(
    usedKeys.some((key) => key.startsWith('home-credential:H-B:')),
    `轮换键要钉在凭据 id 上、并带上家：${usedKeys.join(' / ')}`,
  )
})

pCase('★★ 住户 2 起：键带**住户序号**（`home-agent:<家>:2`），住户 1 照旧无后缀 —— 不对称是有意的', async () => {
  // 为什么必须不对称：住户 1 在世界里的记录是**旧键形状**（`home-agent:<家>`）建的 ——
  // 给它加后缀会让世界**另建一支**（静默重复）；住户 2 起本来就没有旧记录可沿用，
  // 带上序号才能让同一个家的两支各有各的键（不然第二支会重放第一支的记录）。
  const world = statefulWorld()
  const home = scratchHome('slots')
  const one = await provisionHome(world, home, { homeId: 'H-S' })
  assert.equal(one.ok, true, JSON.stringify(one))
  assert.equal(one.slot, 1)
  const two = await provisionHome(world, home, { homeId: 'H-S', slot: 2 })
  assert.equal(two.ok, true, JSON.stringify(two))
  assert.equal(two.slot, 2)
  assert.notEqual(two.agentId, one.agentId, '第二支必须是**另一支** Agent（键不同）')
  assert.notEqual(two.roomId, one.roomId, '第二支有自己的值班室')
  assert.equal(world.agentsByKey.size, 2, '同一个家的两支 = 世界里两条记录')
  const usedKeys = new Set(world.calls.map((c) => c.options?.key).filter(Boolean))
  assert.ok(usedKeys.has('home-agent:H-S'), '住户 1 沿用旧键形状（无后缀）')
  assert.ok(usedKeys.has('home-agent:H-S:2'), '住户 2 带住户序号')
  assert.ok(usedKeys.has('home-direct:H-S:2'))
  assert.ok(usedKeys.has('home-credential:H-S:2'))
  // 两支的支级配置各写各的（同一支重跑不许改写另一支的文件）。
  assert.equal(agentConfMap(home, one.agentId).slot, '1')
  assert.equal(agentConfMap(home, two.agentId).slot, '2')
})

pCase('★ 新住户的**模型绑定**跟着家走（`carryBindingFrom` / `envAside` 两条来路）', async () => {
  const world = statefulWorld()
  const home = scratchHome('carry-binding')
  const one = await provisionHome(world, home, { homeId: 'H-CB' })
  assert.equal(one.ok, true, JSON.stringify(one))
  // 家里第一支已经绑好一条模型 ⇒ 第二支的环境建起来时应当**跟着用同一条**。
  const firstRoot = envRootOf(home, one.agentId)
  fs.writeFileSync(path.join(firstRoot, 'model_binding.json'), '{\n  "source": "ms-1"\n}\n', 'utf8')
  const two = await provisionHome(world, home, {
    homeId: 'H-CB',
    slot: 2,
    carryBindingFrom: firstRoot,
  })
  assert.equal(two.ok, true, JSON.stringify(two))
  assert.equal(
    fs.readFileSync(path.join(envRootOf(home, two.agentId), 'model_binding.json'), 'utf8'),
    '{\n  "source": "ms-1"\n}\n',
    '新住户的环境里要有同一条绑定（不然它起不来）',
  )
  // 负对照：绑定**不许**被带进一个已经自己有绑定的环境（不覆盖既有事实）。
  fs.writeFileSync(path.join(envRootOf(home, two.agentId), 'model_binding.json'), '{"source":"ms-9"}\n', 'utf8')
  const three = await provisionHome(world, home, {
    homeId: 'H-CB',
    slot: 3,
    carryBindingFrom: path.join(envRootOf(home, two.agentId)),
  })
  assert.equal(three.ok, true, JSON.stringify(three))
  assert.equal(
    fs.readFileSync(path.join(envRootOf(home, three.agentId), 'model_binding.json'), 'utf8'),
    '{"source":"ms-9"}\n',
  )
})

// ─────────────────────────────────────────────────────────────────────────────
// **已安家住户的显式读数**（2026-10-01 第七刀补读数）：`up` 重跑她时，布置要**显式**说一句
// "这就是她、认出来了"（`[布置] 住户 <id>（<显示名>）已安家：房 <room_id> —— 不新建、不认领`）。
// 从前的这条路**一句话不说** —— 用户只能从"没有新建读数 ＋ [state] 带着旧经历"反推。
// 数据全从**手边就有**的事实来（支级 `agent.conf` ＋ 她的经历账本）；**不为了读数去起宿主**。

pCase('★ 已安家住户的显式读数：`settledNotice` —— 显示名 / 房 / 经历段数全从手边的事实来，拿不到就不硬凑', () => {
  const { settledNotice, countTraceRuns } = require('../provision')
  const home = scratchHome('settled-notice')
  const envDir = envRootOf(home, 'a-1')
  fs.mkdirSync(envDir, { recursive: true })
  fs.writeFileSync(
    path.join(envDir, 'trace.ndjson'),
    [
      '{"kind":"run-start","run":"host-1-1-1","seq":1}',
      '{"kind":"observed","run":"host-1-1-1","seq":2}',
      '{"kind":"run-start","run":"host-2-2-2","seq":1}',
      '',
    ].join('\n'),
    'utf8',
  )
  assert.equal(countTraceRuns(envDir), 2, '段数＝经历账本里不同的 run（与 agent_host_status 的 of= 同一口径）')
  assert.equal(
    settledNotice({ agentId: 'a-1', envDir, roomId: 'r-1', displayName: '小满' }),
    '[布置] 住户 a-1（小满）已安家：房 r-1 —— 不新建、不认领（经历 2 段）',
  )
  // 显示名没有 / 房没有 / 账本不在 ⇒ 那几段**整个省略**（拿不到就不硬凑，也不拿别处顶替）。
  assert.equal(
    settledNotice({ agentId: 'a-2', envDir: path.join(envDir, '..', 'a-none'), roomId: '', displayName: '' }),
    '[布置] 住户 a-2 已安家：房 无 —— 不新建、不认领',
  )
  // 账本读不懂（有一行不是 JSON）⇒ 段数省略（宁可不说，也不报一个错的数）。
  fs.writeFileSync(path.join(envDir, 'trace.ndjson'), '{"run":"x"}\nnot-json\n', 'utf8')
  assert.equal(countTraceRuns(envDir), null)
})

pCase('★ 经历段数的**两种行形状、两处段**都算得出来：带校验列（今天）／不带（旧记录）× 封口段／活动段', () => {
  const { countTraceRuns } = require('../provision')
  const { splitLedgerLine, ledgerBody } = require('../ledger_line')
  const home = scratchHome('trace-line-shapes')
  const envDir = envRootOf(home, 'a-1')
  fs.mkdirSync(path.join(envDir, 'trace.parts'), { recursive: true })

  // ⚠ 这一条是**真机换来的**（2026-10-14），它钉两件事：
  //   ① 2026-10-06 起每行行尾多了一列校验和，而从前对整行 `JSON.parse` ⇒ 段数对**每一支**
  //      都读不出来（`null` ⇒ 整段省略），用例却一直绿（fixture 写的是旧形状）；
  //   ② 口径是**总次数**（封口段 ＋ 活动段，`agent 侧 summaries_at`），而从前只读活动段
  //      ⇒ 分卷之后少报一大截（真机：小欣 4 段 vs 真实的 41 段）。
  const today = [
    { kind: 'run-start', run: 'host-9-9-9', seq: 1 },
    { kind: 'sealed', run: 'host-9-9-9', seq: 2 },
  ]
  const signed = today.map((row, index) => `${JSON.stringify(row)}\t${index.toString(16).padStart(8, '0')}`)
  const legacy = ['{"kind":"run-start","run":"host-8-8-8","seq":1}']
  fs.writeFileSync(path.join(envDir, 'trace.parts', '000000000000.ndjson'), '{"kind":"run-start","run":"host-7-7-7","seq":1}\n', 'utf8')
  fs.writeFileSync(path.join(envDir, 'trace.ndjson'), `${[...signed, ...legacy].join('\n')}\n`, 'utf8')
  assert.equal(countTraceRuns(envDir), 3, '封口段 1 段 ＋ 活动段 2 段（两种行形状混在一本账里也数得对）')

  // 三态不许压成两态：制表符之后不是十六进制 ⇒ `unreadable`，**不是**旧记录。
  assert.equal(splitLedgerLine(signed[0]).state, 'present')
  assert.equal(splitLedgerLine(legacy[0]).state, 'absent')
  assert.equal(splitLedgerLine('{"a":1}\tZZ').state, 'unreadable')
  assert.equal(ledgerBody(signed[0]), JSON.stringify(today[0]), '正文就是制表符之前那一段')
})

pCase('★★ 已安家的住户重跑：布置**显式**打一行「已安家」（这就是她）—— 新建 / 认领那两种读数不走这条路', async () => {
  const world = statefulWorld()
  const home = scratchHome('settled-rerun')
  const first = await provisionHome(world, home, { homeId: 'H-R', agentName: '测试住户' })
  assert.equal(first.ok, true, JSON.stringify(first))
  // 给她留两段经历 —— 重跑时读数里应当带着段数（从她的经历账本读，**不起宿主**）。
  fs.writeFileSync(
    path.join(envRootOf(home, first.agentId), 'trace.ndjson'),
    '{"run":"host-1-1-1"}\n{"run":"host-1-1-1"}\n{"run":"host-2-2-2"}\n',
    'utf8',
  )
  const resident = residentOf(home, first.agentId)
  const lines = await captureLog(() =>
    provisionHome(world, home, { homeId: 'H-R', resident, slot: resident.slot, agentName: '测试住户' }),
  )
  const said = lines.join('\n')
  assert.match(
    said,
    new RegExp(
      `\\[布置\\] 住户 ${first.agentId}（测试住户）已安家：房 ${first.roomId} —— 不新建、不认领（经历 2 段）`,
    ),
    `重跑要把"这就是她、认出来了"显式说出来：\n${said}`,
  )
  assert.ok(!said.includes('已建'), `重跑不许出现「新建」读数：\n${said}`)
  assert.ok(!said.includes('认领了世界里'), `重跑不许出现「认领」读数：\n${said}`)
  // **不许静默忽略**：这次给了名字，但家里已经有住户 ⇒ 说清它没派上用场。
  assert.match(said, /--agent-name 没派上用场/, `给了名字却没用上，必须说出来：\n${said}`)
})

// ─────────────────────────────────────────────────────────────────────────────
// **系统不替 Agent 起名**（2026-10-12）：从前这里写死 `小满` ——
// 一个具体的名字被当成"默认"，等于系统借一个昵称替那支 Agent 认定了身份（判据 9），
// 而且它是一个人的名字躺在开源仓库里。现在的规矩与"首位 Human 的默认显示名"**同一条**：
// 人给了就用人的（`--agent-name`），没给就是**占位**（明确是占位，不是称呼）。
// ─────────────────────────────────────────────────────────────────────────────

pCase('★★ 系统不替 Agent 起名：没给 --agent-name ⇒ 显示名是占位「未命名」，而且读数说出来', async () => {
  const world = statefulWorld()
  const home = scratchHome('agent-name-placeholder')
  const lines = await captureLog(() => provisionHome(world, home, { homeId: 'H-N' }))
  const said = lines.join('\n')
  const posts = world.calls.filter((c) => c.method === 'POST' && c.url === '/v1/agents')
  assert.equal(posts.length, 1, `只该建一支：\n${said}`)
  assert.equal(
    posts[0].options.body.display_name,
    '未命名',
    '没给名字 ⇒ 送的是**占位**，不是一个具体的人名',
  )
  assert.match(said, /\[布置\] Agent 已建：a-\d+（显示名 未命名 —— 占位，不是称呼/, `读数要说清它是占位：\n${said}`)
  assert.match(said, /名字归它自己或你/, '并说清名字归谁（这才是它的路）')
})

pCase('★★ 人给了名字就用人的：--agent-name 一路送到世界，读数不再说占位', async () => {
  const world = statefulWorld()
  const home = scratchHome('agent-name-given')
  const lines = await captureLog(() => provisionHome(world, home, { homeId: 'H-G', agentName: '阿岩' }))
  const said = lines.join('\n')
  const posts = world.calls.filter((c) => c.method === 'POST' && c.url === '/v1/agents')
  assert.equal(posts[0].options.body.display_name, '阿岩', '人给的名字要原样送出去')
  assert.match(said, /\[布置\] Agent 已建：a-\d+（显示名 阿岩）/, `读数里就是人给的那个名字：\n${said}`)
  assert.ok(!said.includes('占位'), `人给了名字就别说占位：\n${said}`)
})

// ─────────────────────────────────────────────────────────────────────────────
// **「同一支、没带凭据、换机器」那条死路的读数**（2026-10-02 第九刀）：撞 `409 backend_binding_exists`
// 时不许再抛裸 JSON —— 读数要说清**这是什么** ＋ **两条出路**（把凭据带到本机同一位置 / 按手册
// 「最后手段」重签并说清后果）；仍然 fail-closed：不动文件、不起宿主、不静默重建。
// ─────────────────────────────────────────────────────────────────────────────

pCase('★★ 撞 `409 backend_binding_exists` 的读数（纯函数）：是什么 ＋ 两条出路 ＋ 不泄漏凭据', () => {
  const { backendBindingExistsReadout } = require('../provision')
  const lines = backendBindingExistsReadout({ credentialFile: '/creds/abc123/agent-9.token' }).split('\n')
  assert.equal(lines.length, 3, `三行：是什么 / 出路一 / 出路二：\n${lines.join('\n')}`)
  assert.ok(lines[0].includes('409 backend_binding_exists'), '机器可读的原因要带上')
  assert.ok(lines[0].includes('已有一条后端连接'), '说清这是什么（本刀的关键句）')
  assert.ok(lines[0].includes('可能来自另一台机器'), '"可能来自另一台机器"要说出口')
  assert.ok(lines[1].includes('把那份凭据带到本机同一位置'), '出路一：把凭据带过去')
  assert.ok(lines[1].includes('＜凭据目录＞/＜世界摘要＞/＜agent id＞.token'), '给出路径形状')
  assert.ok(lines[1].includes('/creds/abc123/agent-9.token'), '本机的落点要能给到（它是路径，不是凭据）')
  assert.ok(lines[2].includes('旧的那支会**立即失效**'), '出路二：重签的后果（旧的那支立即失效）')
  assert.ok(lines[2].includes('会被踢下线'), '另一台机器上正在跑的她会被踢下线')
  assert.ok(lines[2].includes('先在那台机器 down'), '纪律：先 down 那边')
  assert.ok(!lines.join('\n').includes('clr_'), '读数里不许出现凭据形状')
  // 拿不到本机落点（理论上不该发生）⇒ 退回只说形状，不硬编一个路径、也不出现 undefined。
  const fallback = backendBindingExistsReadout({})
  assert.ok(fallback.includes('＜凭据目录＞/＜世界摘要＞/＜agent id＞.token'))
  assert.ok(!fallback.includes('undefined'))
})

pCase('★★ 同一支、没带凭据、换机器：布置撞 409 时读数说清两条出路，且仍然 fail-closed', async () => {
  // 场景 = 真机第七刀 5a-ii：她**已安家**（住户化形状：身份/房/序号都在支级 agent.conf 里），
  // 但本机没有她那份后端凭据（凭据在另一台机器上）⇒ 建 binding 撞 409 backend_binding_exists。
  const home = scratchHome('move-no-cred', ['home_id=H-MOVE'])
  process.env.CELIRA_CREDENTIALS_DIR = home.credsDir
  try {
    const envDir = envRootOf(home, 'a-moved')
    fs.mkdirSync(envDir, { recursive: true })
    fs.writeFileSync(path.join(envDir, 'agent.conf'), 'agent_id=a-moved\nroom_id=r-old\nslot=1\n', 'utf8')
    fs.writeFileSync(path.join(envDir, 'identity.json'), '{}', 'utf8')
    const confBefore = fs.readFileSync(home.confFile, 'utf8')
    const im = fakeWorld({
      'POST /v1/agents/a-moved/backend-bindings': {
        status: 409,
        body: { error: { code: 'backend_binding_exists', message: 'agent already has an active backend binding' } },
      },
    })
    const out = await provision({
      confFile: home.confFile,
      homeDir: home.homeDir,
      imExe: path.join(home.root, 'no-such-celira-im'),
      runExe: '/fake/agent_run',
      imEnv: {},
      resident: residentOf(home, 'a-moved'),
      remote: { base: 'http://world.example:8080', token: 'clr_hs_owner' },
      imRequest: im.fn,
      runCli: () => {
        throw new Error('撞 409 之后不许再起任何本机 CLI（agent_run 也不例外）')
      },
    })
    assert.equal(out.ok, false, '仍然 fail-closed：这条路没有"自动走通"')
    const message = String(out.message)
    assert.ok(message.includes('已有一条后端连接'), `读数要说清这是什么：\n${message}`)
    assert.ok(message.includes('把那份凭据带到本机同一位置'), `出路一：\n${message}`)
    assert.ok(message.includes('旧的那支会**立即失效**'), `出路二（并说清后果）：\n${message}`)
    assert.ok(message.includes(credentialPathOf(home, 'a-moved')), `给出本机的落点：\n${message}`)
    // fail-closed 的机械判据：只发了这一句请求；文件一个字没动；凭据没被静默重签。
    assert.deepEqual(
      im.calls.map((c) => `${c.method} ${c.url}`),
      ['POST /v1/agents/a-moved/backend-bindings'],
      '撞 409 之前只该有这一句（不多问、不绕路）',
    )
    assert.equal(fs.readFileSync(home.confFile, 'utf8'), confBefore, 'home.conf 一个字都不该动（已有家 id）')
    assert.ok(!fs.existsSync(credentialPathOf(home, 'a-moved')), '凭据没有被静默重签')
  } finally {
    delete process.env.CELIRA_CREDENTIALS_DIR
  }
})

// ─────────────────────────────────────────────────────────────────────────────
// **老家（一个家一支、环境根 `<家>/env`）的形状迁移**：不在本文件 —— 它是
// `residents.js` 的 `migrateHomeShape`（由 `up` 排在**读住户之前**调用；2026-10-01 多支刀）。
//
// 为什么挪走：形状迁移是"这个家怎么摆"的事，而 `provision` 只管**一支**（住户化之后它连
// `home.conf` 里的身份都不读了 —— 那些是支级事实）。两处都判形状 = 两份权威，迟早对不上。
// 那一族用例（有/没有家 id、旧那支在跑、远端没有主人凭据、幂等重跑、绑定跟着走）见
// `scripts/home/tests/residents.test.js`。

test('门禁不许空跑：本文件真的执行了用例（含"按家取键"那一族）', () => {
  assert.ok(cases >= 17, `用例数 ${cases} 少于声明的 17`)
})

pCase('★★ 世界里已经有那间房 ⇒ **不写**，直接用（世界的现状是权威，键是附注）', async () => {
  // 真机两例（真家与 dev 家）：世界里那间房早就在，而布置用的幂等键被**上一次半途而废的尝试**
  // 占住 ⇒ 从前直接去写 ⇒ 409 ⇒ `up` 退出 4 ⇒ **家永久起不来**。
  const world = fakeWorld({
    ...happyRemoteWorld('a-existing'),
    'GET /v1/rooms': {
      status: 200,
      body: {
        rooms: [
          { room: { id: 'r-existing', kind: 'direct', status: 'active', name: null }, peer: { id: 'a-existing' } },
          { room: { id: 'r-other', kind: 'direct', status: 'active', name: null }, peer: { id: 'a-somebody-else' } },
        ],
      },
    },
  })
  const home = scratchHome('room-existing')
  const out = await provisionHome(world, home, { homeId: 'H-EX', token: 'clr_hs_owner' })
  assert.equal(out.ok, true, JSON.stringify(out))
  assert.equal(out.roomId, 'r-existing', '要用世界里那一间（并且按 peer 匹到**自己**那支）')
  assert.ok(
    !world.calls.some((c) => c.method === 'POST' && c.url === '/v1/direct-rooms'),
    '世界里已经有了就**一次都不许写**',
  )
})

pCase('★★ 写被拒（409 键被占过）⇒ **再读一次世界**，读到就用它（写被拒 ≠ 事实不存在）', async () => {
  let looked = 0
  const world = fakeWorld({
    ...happyRemoteWorld('a-late'),
    // 第一次读：还没有（照旧去写）；第二次读：有了（世界在两次之间给了它）。
    'GET /v1/rooms': () => ({
      status: 200,
      body: {
        rooms:
          looked++ === 0
            ? []
            : [{ room: { id: 'r-late', kind: 'direct', status: 'active', name: null }, peer: { id: 'a-late' } }],
      },
    }),
    'POST /v1/direct-rooms': { status: 409, body: { error: { code: 'idempotency_conflict' } } },
  })
  const home = scratchHome('room-409')
  const out = await provisionHome(world, home, { homeId: 'H-409', token: 'clr_hs_owner' })
  assert.equal(out.ok, true, JSON.stringify(out))
  assert.equal(out.roomId, 'r-late', '409 之后要用世界里那一间，而不是把家判死')
})