// 本机状态口读取层的纯逻辑测试（TASK_55）。
//
// 这一层的判据只有一条要紧的：**形状认不出就当读不到**。
// 半读半猜会把"没读到"画成"Agent 没在跑"——那比空白更坏。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { readdirSync, readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { createRequire } from 'node:module'
import {
  EXPERIENCE_ROW_FIELDS,
  fetchHomeStatus,
  agentAgentMatch,
  isHomeStatusView,
  HOME_STATUS_URL,
} from '../src/api/homeStatus.ts'
import { localServiceReading, readingLabel } from '../src/components/celira/reading-label.ts'

/** 读一份源码（接线守卫用；与其它用例同一个写法）。 */
const read = (relative: string) =>
  readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8')

const GOOD = {
  readAt: '2026-09-19T06:00:00.000Z',
  stale: false,
  im: { health: 200 },
  agent: {
    state: 'loaded',
    admission: 'ready',
    activities: 32,
    truncated: true,
    runs: [{ id: 'host-1-2-3', state: 'ended', outcome: 'subject:completed' }],
    runsTotal: 32,
    next: '活动已收场——无需动作，或开始新活动',
  },
  lastWake: 'wake run=host-1-2-3 drives=1 ending=subject:completed notification=read',
}

test('形状守卫：合法读数通过', () => {
  assert.equal(isHomeStatusView(GOOD), true)
})

test('形状守卫：宿主那一格（2026-10-06 补进契约）', () => {
  const withHost = (host: unknown) => ({ ...GOOD, host })
  // 四态都通过 —— 它是口子给的稳定词表。
  for (const state of ['running', 'running-orphan', 'stopped', 'unknown']) {
    assert.equal(isHomeStatusView(withHost({ state })), true, state)
  }
  // **缺这一格也通过**（旧版状态口 / 壳那条路）：缺 = 这一面没读数，
  // 而"缺"与"没在跑"在页面上必须长得不一样。
  assert.equal(isHomeStatusView(GOOD), true)
  // `pid` / `log` 可选（旧版可能只给 state）。
  assert.equal(isHomeStatusView(withHost({ state: 'running', pid: 4242 })), true)
  // **取值不认识不算形状坏**：`hostStateOf` 会把它读作"这一格没读数"，
  // 而整份拒绝会让这一页在最需要它的时候瞎掉（真机咬过三次，见 `hostStateOf` 的文档）。
  assert.equal(isHomeStatusView(withHost({ state: 'starting' })), true)
  // 形状坏（不是对象 / `state` 不是字符串）⇒ 整份拒绝。
  assert.equal(isHomeStatusView(withHost('running')), false)
  assert.equal(isHomeStatusView(withHost({ pid: 1 })), false)
})

test('形状守卫：家没开机时的 honest 读数也通过（不要求它"有数据"）', () => {
  assert.equal(
    isHomeStatusView({
      readAt: '2026-09-19T06:00:00.000Z',
      stale: false,
      im: { health: null },
      agent: { state: 'missing-binary' },
      lastWake: null,
    }),
    true,
  )
  assert.equal(
    isHomeStatusView({
      readAt: 'x',
      stale: false,
      im: { health: 200 },
      agent: { state: 'unreadable', reason: 'malformed-run-line' },
      lastWake: null,
    }),
    true,
  )
})

test('★ 形状守卫：零住户的诚实读数通过 ——「还没有 Agent」是**缺省**，不是缺件', () => {
  // 2026-10-05：这一态与 `missing-binary`（程序缺件）分开之后才可能出现；
  // 两者都必须在封闭表里，页面才敢把一句"这台机器上还没有 Agent"摆出来（而不是"缺少必要的程序"）。
  assert.equal(
    isHomeStatusView({
      readAt: '2026-10-05T00:00:00.000Z',
      stale: false,
      im: { health: 200 },
      agent: { state: 'no-resident' },
      lastWake: null,
    }),
    true,
  )
})

test('★ 形状守卫：老家形态那一格（`legacyHome`）—— 认不出的形状不许说成不存在', () => {
  const legacy = {
    readAt: '2026-10-05T00:00:00.000Z',
    stale: false,
    im: { health: 200 },
    agent: { state: 'no-resident' },
    lastWake: null,
    legacyHome: true,
  }
  assert.equal(isHomeStatusView(legacy), true, '老家那一家要读得进来（页面才说得出"老形态"）')
  assert.equal(isHomeStatusView({ ...legacy, legacyHome: false }), true, 'false 也是合法读数')
  // ⚠ 读不懂**不许**当"不是"（那是把缺省说成事实）——与 `stale` 同一条纪律。
  assert.equal(isHomeStatusView({ ...legacy, legacyHome: 'true' }), false)
  assert.equal(isHomeStatusView({ ...legacy, legacyHome: 1 }), false)
})

// —— 负对照：认不出的形状一律拒 ——

test('负对照：缺必需字段 ⇒ 拒', () => {
  assert.equal(isHomeStatusView({ ...GOOD, readAt: undefined }), false)
  // `stale` 缺了就是「不知道这份新不新」——必须 fail-closed，**不许替它猜一个「新」**
  // （把缓存读数说成当下，正是 TASK_72 ③ 要防的那件事）。
  assert.equal(isHomeStatusView({ ...GOOD, stale: undefined }), false, '缺 stale ⇒ 拒')
  assert.equal(isHomeStatusView({ ...GOOD, stale: 'false' }), false, 'stale 必须是布尔，不是字符串')
  assert.equal(isHomeStatusView({ ...GOOD, im: undefined }), false)
  assert.equal(isHomeStatusView({ ...GOOD, agent: undefined }), false)
  assert.equal(isHomeStatusView({ ...GOOD, im: {} }), false, 'im.health 是必需读数（缺一格 = 形状不认识）')
  assert.equal(isHomeStatusView({ ...GOOD, im: { health: '200' } }), false, 'health 是字符串 ⇒ 拒（那是没转换的读数）')
  assert.equal(isHomeStatusView({ ...GOOD, im: { health: null } }), true, 'health: null 是**合法**读数（它没答话）')
  assert.equal(isHomeStatusView({ ...GOOD, im: { health: 500 } }), true, '非 200 也是**合法**读数（有人在答）')
})

test('形状守卫：stale = true 是**合法**读数（缓存读数不是读不到）', () => {
  assert.equal(isHomeStatusView({ ...GOOD, stale: true }), true)
})

test('负对照：Agent 的状态不在封闭集合里 ⇒ 拒（不猜成"未知"）', () => {
  assert.equal(isHomeStatusView({ ...GOOD, agent: { state: 'something-new' } }), false)
})

test('负对照：runs 形状畸形 ⇒ 拒（半条活动比没有更坏）', () => {
  assert.equal(isHomeStatusView({ ...GOOD, agent: { state: 'loaded', runs: [{ id: 'x' }] } }), false)
  assert.equal(isHomeStatusView({ ...GOOD, agent: { state: 'loaded', runs: 'nope' } }), false)
})

test('负对照：lastWake 既不是字符串也不是 null ⇒ 拒', () => {
  assert.equal(isHomeStatusView({ ...GOOD, lastWake: 42 }), false)
})

test('默认基址是**同源相对路径**（TASK_61：页面不知道端口）', () => {
  // 判据是"这根串里不许出现端口与主机" —— 只要它还是绝对地址，改 `home.conf` 的
  // `status_port` 就又要把前端改一遍（那正是本刀要消掉的对偶）。
  assert.equal(HOME_STATUS_URL, '/home-status')
  assert.ok(!/^https?:/i.test(HOME_STATUS_URL), '不许是绝对地址')
  assert.ok(!/:\d+/.test(HOME_STATUS_URL), '不许带端口')
})

async function withFetch(
  handler: (url: string) => Promise<Response>,
  run: () => Promise<void>,
): Promise<void> {
  const original = globalThis.fetch
  globalThis.fetch = ((input: RequestInfo | URL) =>
    handler(typeof input === 'string' ? input : input.toString())) as typeof fetch
  try {
    await run()
  } finally {
    globalThis.fetch = original
  }
}

test('fetchHomeStatus：打到 /status，合法形状原样返回', async () => {
  let seen = ''
  await withFetch(
    async (url) => {
      seen = url
      return new Response(JSON.stringify(GOOD), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      })
    },
    async () => {
      const view = await fetchHomeStatus()
      // 逐字钉住：后一段是**口子自己的路径**（代转不给端面改名）。
      assert.equal(seen, '/home-status/status')
      assert.equal(seen, `${HOME_STATUS_URL}/status`)
      assert.equal(view.agent.admission, 'ready')
      assert.equal(view.agent.runs?.length, 1)
    },
  )
})

test('负对照：非 2xx ⇒ 抛（由调用方决定怎么说"没读到"）', async () => {
  await withFetch(
    async () => new Response('{}', { status: 500 }),
    async () => {
      await assert.rejects(() => fetchHomeStatus())
    },
  )
})

test('负对照：形状不认识 ⇒ 抛（绝不当成"Agent 没在跑"画出来）', async () => {
  await withFetch(
    async () => new Response(JSON.stringify({ surprise: true }), { status: 200 }),
    async () => {
      await assert.rejects(() => fetchHomeStatus(), /unknown shape/)
    },
  )
})

// —— Agent 的经历（TASK_72 ④）：第二道白名单 + 两侧对表 ——

test('经历：合形状的机械读数通过（只有白名单字段）', () => {
  const experience = {
    state: 'loaded',
    rows: [
      { seq: 6, run: 'host-1-1-1', turn: 1, kind: 'observed', action: 'notes_create', state: 'success' },
      { seq: 7, run: 'host-1-1-1', kind: 'ending', subject: 'completed' },
    ],
    truncated: true,
    windowed: false,
  }
  assert.equal(isHomeStatusView({ ...GOOD, experience }), true)
  assert.equal(
    isHomeStatusView({ ...GOOD, experience: { state: 'missing' } }),
    true,
    '这一面读不到也是合法读数（页面据此说"读不到"，不画空表）',
  )
})

/**
 * 泄漏样本 = **排除清单里"会带正文"的那 6 个**。
 *
 * **只有这一份**：抽成常量的原因正是它曾经有过两份 —— `:175` 那份是 5 个、`:207` 那份是
 * 6 个（差 `extra-actions`），而两份手抄的清单迟早漂移（本仓已有先例：`contractErrors.ts`
 * 那张手抄的分类清单里藏过 4 条死码）。下面两条测试都只读这一个常量。
 */
const LEAKY_FIELDS = ['params', 'blocks', 'text', 'content', 'payload', 'extra-actions']

test('经历：**行里出现正文类字段 ⇒ 整个读数判成"读不到"**（第二道白名单）', () => {
  // 口子那层已经按白名单投影过；这一条是**第二道**：万一口子漏了，客户端也不许画出来。
  for (const leaked of LEAKY_FIELDS) {
    const experience = {
      state: 'loaded',
      rows: [{ seq: 6, kind: 'observed', action: 'notes_create', [leaked]: 'Agent 说的话' }],
    }
    assert.equal(
      isHomeStatusView({ ...GOOD, experience }),
      false,
      `行里带了 ${leaked} ⇒ fail-closed`
    )
  }
})

test('经历：窗口标记必须是布尔；行必须是对象', () => {
  assert.equal(
    isHomeStatusView({ ...GOOD, experience: { state: 'loaded', rows: [], truncated: 'yes' } }),
    false,
  )
  assert.equal(isHomeStatusView({ ...GOOD, experience: { state: 'loaded', rows: ['x'] } }), false)
  assert.equal(isHomeStatusView({ ...GOOD, experience: { state: 'weird' } }), false)
})

test('两侧白名单**逐字相同**（防漂移：客户端那份 vs 口子那份）', async () => {
  // 两份清单住在两个运行时里（浏览器 TS / 家的 node 脚本），没法共用一份代码；
  // 于是用这条**对表**守住"改一边忘另一边"——那正是泄漏最容易钻进来的缝。
  const require = createRequire(import.meta.url)
  const server = require('../../scripts/home/status-server.js') as { EXPERIENCE_FIELDS: string[] }
  assert.deepEqual(
    [...EXPERIENCE_ROW_FIELDS].sort(),
    [...server.EXPERIENCE_FIELDS].sort(),
    '客户端与口子的白名单必须一模一样',
  )
  for (const name of LEAKY_FIELDS) {
    assert.ok(!server.EXPERIENCE_FIELDS.includes(name), `${name} 不许在口子那份白名单里`)
  }
})

// —— 更新那一行：时刻 + **缓存标注**（TASK_72 ③；人话刀改名「读数时刻」→「更新时间」）——
//
// 这两条是「代价必须被标注」的判据：TTL 命中时**不能只给时刻**，也不能把时刻换成"当下"。
// ⚠ 词变了（给用户看），**判据一条没变**：时刻照旧是那次的、陈旧照旧要标出来。

test('更新行：非缓存时只给时刻', () => {
  assert.equal(readingLabel('2026-09-19 14:00:00', false), '更新时间 2026-09-19 14:00:00')
})

test('更新行：缓存命中时必须明说"正在刷新"，时刻仍是那次的时刻', () => {
  const label = readingLabel('2026-09-19 14:00:00', true)
  assert.match(label, /正在刷新/, '陈旧要被说出口，不许冒充当下')
  assert.ok(
    label.includes('2026-09-19 14:00:00'),
    '时刻还是**那次读数**的时刻（这一层不许自己取当前时间）',
  )
})

// —— 「本地服务」那一行：**三态**（2026-10-11 立的判据）——
//
// 病根：这一行从前读的是派生的 `listening`（`health === 200`）⇒ 「世界活着、只是自报 500」
// 被写成**「没在跑（HTTP 500）」**：自相矛盾的字面（把"在跑"和"没在跑"同时说了）。
// 判据两条：① **有应答（任何码）⇒ 那个口上有人在 ⇒ 在跑**；
//           ② **`null` ⇒ 不许说成"没在跑"** —— 探不到**证明不了**它停了（读不到 ≠ 没有）。

test('本地服务读数：答了 200 ⇒ 「在跑」，且是正常灯', () => {
  assert.deepEqual(localServiceReading(200), { text: '在跑', tone: 'ok' })
})

test('★ 本地服务读数：答了**非 200** ⇒ 仍然是「在跑」（它自己说不健康），不是"没在跑"', () => {
  const reading = localServiceReading(500)
  assert.match(reading.text, /在跑/, '有应答就证明那个口上有人在 —— 说成"没在跑"就是自相矛盾')
  assert.ok(!reading.text.includes('没在跑'), '禁止出现"没在跑"')
  assert.match(reading.text, /500/, '它自己报的那个码要摆出来（那是证据，不是黑话）')
  assert.equal(reading.tone, 'danger', '确定的故障 ⇒ 红灯')
})

test('★ 本地服务读数：没答话（null）⇒ 说「读不到」，**不说"没在跑"**，且灯是灰的', () => {
  const reading = localServiceReading(null)
  assert.ok(!reading.text.includes('没在跑'), '探不到证明不了它停了 —— 不许说成事实')
  assert.match(reading.text, /读不到/, '要如实说读不到')
  assert.equal(reading.tone, 'muted', '不确定不许画成故障（画成故障 = 把"我不知道"说成"它坏了"）')
})

// —— 接线守卫（2026-10-11）：这条判据只许有一份实现，派生字段不许复活 ——
//
// 病根的形状：门那一侧给一个派生布尔（`listening = health === 200`），页面拿它当主词 ⇒
// 同一行同时说了"在跑"和"没在跑"。判据是仓库那条**一个事实一个权威位置**：
// 探活的读数就是 `health`；要说的话由 `localServiceReading` 一处产出。

test('★ 两个页面都用那一份判据（不许各写一份 if）', () => {
  const machine = read('../src/components/celira/Runtimes.tsx')
  const settings = read('../src/pages/settings/SettingsServerPage.tsx')
  for (const [name, source] of [
    ['机器页「运行」那一间', machine],
    ['设置「服务器」那一页', settings],
  ] as const) {
    assert.ok(source.includes('localServiceReading'), `${name} 没有用共用判据`)
  }
  // ⚠ 这里**不**断言"文件里没有『没在跑』"：那个词在别处是**合法**的
  //（宿主/账本那一族有账本证据，"没在跑"说得出）—— 探活那一行才不许说。判据见上面三条用例。
})

/** 去掉注释再扫（这条守卫找的是**代码里的字段**；注释里可以提它 —— 否则下次改注释就假红）。 */
const stripComments = (source: string) =>
  source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '')

test('★ 派生字段 `listening` 在 client 的**代码**里不许复活（探活只有一个事实：`health`）', () => {
  const root = new URL('../src/', import.meta.url)
  const offenders: string[] = []
  const walk = (dir: URL) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.isDirectory()) walk(new URL(`${entry.name}/`, dir))
      else if (/\.tsx?$/.test(entry.name)) {
        const file = fileURLToPath(new URL(entry.name, dir))
        if (stripComments(readFileSync(file, 'utf8')).includes('listening')) {
          offenders.push(file.slice(fileURLToPath(root).length).replace(/\\/g, '/'))
        }
      }
    }
  }
  walk(root)
  assert.deepEqual(offenders, [], `这些文件的代码里又出现了 listening：${offenders.join(', ')}`)
})

// —— 刀2b C①：这份本机读数**是不是这个 Agent 的**（闭合刀2a 登记的判据缺口）——

test('本机读数归属：同一个人 / 不是同一个人 / 不知道（三态不许压成两态）', () => {
  const withId = { ...GOOD, agent: { ...GOOD.agent, agentId: 'agent-7' } }
  assert.equal(agentAgentMatch(withId, 'agent-7'), 'same')
  assert.equal(agentAgentMatch(withId, 'agent-9'), 'different', '别人 ⇒ 页面说"此 agent 未在本机运行"')
  // ⚠ 读不到 id ⇒ **不知道**：这时候不许说"Agent 不在这台机器上"（那是把读不到说成一个事实）。
  assert.equal(agentAgentMatch(GOOD, 'agent-7'), 'unknown')
  assert.equal(agentAgentMatch(withId, ''), 'unknown')
  assert.equal(agentAgentMatch(withId, null), 'unknown')
})
