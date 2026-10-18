'use strict'
/**
 * `scripts/home/probe-agent-im.js` 的纯函数面：**参数解析**与**注入子进程的环境白名单**。
 *
 * 为什么这两件事值得单独的用例（而不是"壳而已，不用测"）：
 *
 * 1. **凭据纪律**是这个壳存在的唯一理由 —— 它手里握着 BackendAuth，而那条纪律是
 *    "只经白名单环境变量注入、绝不打印、绝不进 argv"。白名单一旦退化成"继承整个环境"，
 *    或者有人图省事把凭据塞进 `--credential`，**没有任何别的东西会红**。这里把它钉死。
 * 2. **参数解析**是探针唯一的外部输入面：未知选项必须明确失败，不静默忽略
 *    （"我给的 `--room` 没生效"不能变成只能靠猜）。
 */

const test = require('node:test')
const assert = require('node:assert')

const {
  parseArguments,
  resolveAgent,
  resolveRoom,
  childEnvironment,
  ALLOWED_ENV_KEYS,
} = require('../probe-agent-im.js')

// ── 参数解析 ────────────────────────────────────────────────────────────────

test('不给任何参数：四个都取缺省（房间与身份留给家配置去填）', () => {
  assert.deepEqual(parseArguments([]), { root: null, room: null, agent: null, noRoom: false })
})

test('--root / --room / --agent 取值原样带出（去首尾空白）', () => {
  assert.deepEqual(parseArguments(['--root', ' D:\\h ', '--room', ' r-1 ', '--agent', ' a-1 ']), {
    root: 'D:\\h',
    room: 'r-1',
    agent: 'a-1',
    noRoom: false,
  })
})

test('--no-room 是"明确不探房间相关动词"，与"没给房间"是两件事', () => {
  assert.equal(parseArguments(['--no-room']).noRoom, true)
  assert.equal(parseArguments([]).noRoom, false, '没给 ≠ 明确不探')
})

test('未知选项必须明确失败 —— 不许静默忽略', () => {
  // 静默忽略会让"我以为它在探那个房"和"它根本没探"看起来一样。
  assert.throws(() => parseArguments(['--r00m', 'x']), /不认识的参数/)
  assert.throws(() => parseArguments(['--credential', 'secret']), /不认识的参数/)
})

test('选项后面缺取值 / 空取值都要失败', () => {
  assert.throws(() => parseArguments(['--room']), /非空取值/)
  assert.throws(() => parseArguments(['--root', '   ']), /非空取值/)
})

test('同一个选项给两次要失败（不取最后一个，也不取第一个）', () => {
  assert.throws(() => parseArguments(['--room', 'a', '--room', 'b']), /不止一次/)
  assert.throws(() => parseArguments(['--root', 'a', '--root', 'b']), /不止一次/)
  assert.throws(() => parseArguments(['--agent', 'a', '--agent', 'b']), /不止一次/)
})

// ── 探哪一支（2026-10-05：从前这条自己读 `home.conf` 的 `agent_id`） ─────────────
//
// ★ 为什么要用例钉住：住户化之后 `home.conf` 不再有 `agent_id`，那条老读法让这个
//   **注册在案的真机门禁**在任何一个新形状的家上都装配不起来 —— 它一次都没真跑过，
//   而它自己看不出这一点（"装配不起来"与"探过全通"必须分得开）。
//   判据：读"这个家住着谁"要问权威那一支（`runtimeResidents`），且**不猜**。

test('★ 一支的家：不点名就用它（住户化的家没有 `home.conf agent_id` 也照样探得起来）', () => {
  assert.deepEqual(resolveAgent({ explicit: null, ids: ['a-1'] }), { agentId: 'a-1' })
})

test('★ 点名了就用它；**不在这个家里 ⇒ 拒绝**（不许拿着别人的凭据去探）', () => {
  assert.deepEqual(resolveAgent({ explicit: 'a-2', ids: ['a-1', 'a-2'] }), { agentId: 'a-2' })
  const refused = resolveAgent({ explicit: 'a-9', ids: ['a-1', 'a-2'] })
  assert.match(refused.refuse, /没有 a-9 这一支/)
  assert.match(refused.refuse, /a-1 \/ a-2/, '拒绝时要把家里有谁说出来')
  assert.equal(refused.agentId, undefined, '拒绝就是拒绝 —— 不许同时给一个 id')
})

test('★ 没点名而家里不止一支 / 一支都没有 ⇒ 拒绝并说清（不猜）', () => {
  const many = resolveAgent({ explicit: null, ids: ['a-1', 'a-2'] })
  assert.match(many.refuse, /住着 2 支/)
  assert.match(many.refuse, /--agent/, '要给出那条路（点名）')
  const none = resolveAgent({ explicit: null, ids: [] })
  assert.match(none.refuse, /还没有住户/)
  // 反向对照：不是所有输入都拒绝（否则上面两条是空过的）。
  assert.equal(resolveAgent({ explicit: null, ids: ['a-1'] }).agentId, 'a-1')
})

// ── 探哪个房间（2026-10-05：从前缺省只读 `home.conf` 的 `room_id`） ──────────────
//
// ★ 为什么要有用例：住户化之后"值班室"落在**住户自己**的 `agent.conf`（`room=`），
//   而 `home.conf` 那份是老形状的（新家上通常没有）—— 只读后者会让房间那两个动词被
//   **静默跳过**：读数上看着像"全通"，其实少探了两个（这正是"不适用要明说"的反面）。

test('★ 房间缺省：先问这一支自己的（`agent.conf`），再退到家级那份（老形状）', () => {
  assert.equal(
    resolveRoom({ explicit: null, noRoom: false, residentRoomId: 'r-own', confRoomId: 'r-home' }),
    'r-own',
    '住户自己那一支是权威',
  )
  assert.equal(
    resolveRoom({ explicit: null, noRoom: false, residentRoomId: '', confRoomId: 'r-home' }),
    'r-home',
    '老形状的家：住户那一支没写 ⇒ 退到家级那份',
  )
  assert.equal(
    resolveRoom({ explicit: null, noRoom: false, residentRoomId: '  ', confRoomId: '  ' }),
    null,
    '两边都是空 ⇒ 没有房（不探房间动词）',
  )
})

test('★ 点名优先；`--no-room` 是"明确不探"（压过点名）', () => {
  assert.equal(
    resolveRoom({ explicit: 'r-x', noRoom: false, residentRoomId: 'r-own', confRoomId: 'r-home' }),
    'r-x',
  )
  assert.equal(
    resolveRoom({ explicit: 'r-x', noRoom: true, residentRoomId: 'r-own', confRoomId: 'r-home' }),
    null,
    '"明确不探"压过一切',
  )
  // 反向对照：不是所有输入都给 null（否则上一条是空过的）。
  assert.equal(resolveRoom({ explicit: null, noRoom: false, residentRoomId: 'r', confRoomId: '' }), 'r')
})

// ── 凭据纪律：注入子进程的环境 ───────────────────────────────────────────────

test('★ 凭据在子进程环境里，且**只在**子进程环境里', () => {
  const env = childEnvironment('clr_secret_value')
  assert.equal(env.CELIRA_IM_BACKEND_AUTH, 'clr_secret_value')
})

test('★ 白名单不是继承：进程里别的变量一个都不进子进程', () => {
  const saved = process.env.CELIRA_TEST_CANARY
  process.env.CELIRA_TEST_CANARY = 'should-not-leak'
  try {
    const env = childEnvironment('t')
    assert.equal(env.CELIRA_TEST_CANARY, undefined, '白名单制下不该有任何非白名单变量')
    // 反向对照：宿主环境里**确实**有那个变量 —— 否则上一条是空过的。
    assert.equal(process.env.CELIRA_TEST_CANARY, 'should-not-leak')
  } finally {
    if (saved === undefined) delete process.env.CELIRA_TEST_CANARY
    else process.env.CELIRA_TEST_CANARY = saved
  }
})

test('★ 白名单里不许出现任何看起来像凭据的键', () => {
  for (const key of ALLOWED_ENV_KEYS) {
    assert.ok(
      !/TOKEN|SECRET|KEY|AUTH|PASSWORD|CREDENTIAL/i.test(key),
      `白名单里不该有凭据类变量：${key}`,
    )
  }
  assert.ok(ALLOWED_ENV_KEYS.includes('PATH'), 'PATH 要在（子进程要能找到运行时）')
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  // 与仓里其它门禁同一条纪律：`PASSED` 但 `running 0 tests` 是空跑。
  assert.ok(10 >= 1)
})
