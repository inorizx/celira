// 「我的 Agent」列表那三格状态的纯逻辑测试（前端填肉刀，2026-09-23）。
//
// 最要紧的两条判据（都是踩出来的，不许压成一个）：
//  ① **`unknown` 不许说成「未接入」** —— 那是"把读不到说成一个事实"；
//  ② **`lastWake` 是原始行、不是时间串** —— 直接把那一行交给时间格式化会得到 `—`，
//    时间只能从活动标识里还原，且必须说清它从哪来。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import {
  agentRowStatus,
  agentRowStatusFromHomes,
  lastWakeReadout,
  machineSituation,
} from '../src/api/agentRowStatus.ts'
import { localHomeEntry, resolveAgentHome, type HomeEntry, type HomeReading } from '../src/api/homes.ts'
import type { HomeStatusView } from '../src/api/homeStatus.ts'
import type { HomeConfigView } from '../src/api/homeConfig.ts'

/** 状态口的一份合法读数（形状照 `tests/home-status.test.ts` 的 GOOD 抄）。 */
const STATUS: HomeStatusView = {
  readAt: '2026-09-23T04:00:00.000Z',
  stale: false,
  im: { health: 200 },
  agent: {
    state: 'loaded',
    agentId: 'agent-7',
    admission: 'ready',
    activities: 32,
    runs: [{ id: 'host-1-2-3', state: 'ended', outcome: 'subject:completed' }],
    runsTotal: 32,
  },
  // 活动标识里第 2 段是毫秒（宿主文档化的机械格式；`lib/agent-days.ts` 的 RUN_ID）。
  lastWake: 'wake run=host-9988-1789829905936-2 drives=1 ending=subject:completed notification=read',
}

/** 模型配置门的一份合法读数（清单一条 + 绑着它）。 */
const CONFIG: HomeConfigView = {
  readAt: '2026-09-23T04:00:00.000Z',
  state: 'configured',
  keyConfigured: true,
  endpoint: 'https://api.example.com',
  model: 'agent-model',
  contextWindow: 128000,
  protocol: 'chat-completions',
  reason: null,
  sourcesState: 'loaded',
  entries: [
    {
      id: 'ms-1',
      name: '主钥匙',
      endpoint: 'https://api.example.com',
      model: 'agent-model',
      contextWindow: 128000,
      protocol: 'chat-completions',
      keyConfigured: true,
    },
  ],
  bindingState: 'bound',
  bound: 'ms-1',
}

// ── 一、在线那一格：三态 ────────────────────────────────────────────────

test('在线：读数里的 id 就是我正在看的 Agent ⇒ 「在本机运行」（绿）', () => {
  const row = agentRowStatus({ view: STATUS, config: CONFIG, agentId: 'agent-7' })
  assert.equal(row.onlineLabel, '在本机运行')
  assert.equal(row.onlineTone, 'local')
})

test('在线：读数是**别人** ⇒ 「未接入」（灰，不是错误）', () => {
  const row = agentRowStatus({ view: STATUS, config: CONFIG, agentId: 'agent-9' })
  assert.equal(row.onlineLabel, '未接入')
  assert.equal(row.onlineTone, 'away')
})

test('★ 在线：读数里**没有 id** ⇒ 「状态未知」，**绝不冒充「未接入」**', () => {
  const noId: HomeStatusView = { ...STATUS, agent: { ...STATUS.agent, agentId: undefined } }
  const row = agentRowStatus({ view: noId, config: CONFIG, agentId: 'agent-7' })
  assert.equal(row.onlineLabel, '状态未知')
  assert.equal(row.onlineTone, 'unknown')
  assert.notEqual(row.onlineLabel, '未接入', '读不到 ≠ Agent 不在这台机器上')
  // 反方向：我不知道"我是谁"时同样是未知，不许说成别人在跑。
  const alsoUnknown = agentRowStatus({ view: STATUS, config: CONFIG, agentId: '' })
  assert.equal(alsoUnknown.onlineLabel, '状态未知')
})

test('在线：整份读数没读到 ⇒ 「状态未知」（与"是别人"分开）', () => {
  const row = agentRowStatus({ view: null, config: CONFIG, agentId: 'agent-7' })
  assert.equal(row.onlineLabel, '状态未知')
  assert.equal(row.onlineTone, 'unknown')
})

// ── 二、模型那一格：绑定读数 ────────────────────────────────────────────

test('模型：有绑定 ⇒ 「模型：<那一条的名字>」', () => {
  const row = agentRowStatus({ view: STATUS, config: CONFIG, agentId: 'agent-7' })
  assert.equal(row.modelLabel, '模型：主钥匙')
})

test('模型：名字为空时拿模型名顶（与门侧同一口径）', () => {
  const nameless: HomeConfigView = {
    ...CONFIG,
    entries: [{ ...CONFIG.entries[0]!, name: '' }],
  }
  const row = agentRowStatus({ view: STATUS, config: nameless, agentId: 'agent-7' })
  assert.equal(row.modelLabel, '模型：agent-model')
})

test('模型：没有绑定 ⇒ 「模型：未指定」（这是一个事实，不是读不到）', () => {
  const unbound: HomeConfigView = { ...CONFIG, bindingState: 'absent', bound: null }
  const row = agentRowStatus({ view: STATUS, config: unbound, agentId: 'agent-7' })
  assert.equal(row.modelLabel, '模型：未指定')
})

test('★ 模型：绑定读不出来 ⇒ 「模型：未知」，**不说成「未指定」**', () => {
  const unreadable: HomeConfigView = { ...CONFIG, bindingState: 'unreadable', bound: null }
  assert.equal(
    agentRowStatus({ view: STATUS, config: unreadable, agentId: 'agent-7' }).modelLabel,
    '模型：未知',
  )
  // 悬空绑定（说 bound 但那一条不在清单里）也是"读不出来"那一档。
  const dangling: HomeConfigView = { ...CONFIG, bound: 'ms-404' }
  assert.equal(
    agentRowStatus({ view: STATUS, config: dangling, agentId: 'agent-7' }).modelLabel,
    '模型：未知',
  )
})

test('模型：这次没读到门 ⇒ 「模型：未知」（不是"未指定"）', () => {
  const row = agentRowStatus({ view: STATUS, config: null, agentId: 'agent-7' })
  assert.equal(row.modelLabel, '模型：未知')
})

// ── 二之二、模型那一格：**逐支**口径（2026-10-12）────────────────────────────
//
// 起因：这一格从前读的是**整机**那一格（`bindingState` / `bound`），而它在门那边是
// "**第一支**绑的那条" ⇒ 一个家住两支时**两行会显示同一条**（第一支的），第二行那句是假话。
// 上面那些用例走的是"老门没给逐支那一格"的退回路（与从前一字不变），下面这几条走逐支路。

/** 两支：`agent-7` 用 `ms-1`（主钥匙），`agent-9` 用 `ms-2`（备用钥匙）。 */
function twoResidents(): HomeConfigView {
  return {
    ...CONFIG,
    bindingDivergent: true,
    entries: [
      ...CONFIG.entries,
      { ...CONFIG.entries[0]!, id: 'ms-2', name: '备用钥匙' },
    ],
    boundPerResident: [
      { agentId: 'agent-7', state: 'bound', bound: 'ms-1' },
      { agentId: 'agent-9', state: 'bound', bound: 'ms-2' },
    ],
  }
}

test('★★ 模型：一个家住两支、各绑不同 ⇒ 每一行显示**自己**那一条', () => {
  const config = twoResidents()
  assert.equal(agentRowStatus({ view: STATUS, config, agentId: 'agent-7' }).modelLabel, '模型：主钥匙')
  assert.equal(agentRowStatus({ view: STATUS, config, agentId: 'agent-9' }).modelLabel, '模型：备用钥匙')
})

test('★ 模型：这一支**不在**逐支那一份里 ⇒ 「未知」，**不许拿第一支那条顶**', () => {
  const config: HomeConfigView = {
    ...CONFIG,
    boundPerResident: [{ agentId: 'agent-7', state: 'bound', bound: 'ms-1' }],
  }
  const other = agentRowStatus({ view: STATUS, config, agentId: 'agent-9' })
  assert.equal(other.modelLabel, '模型：未知')
  assert.notEqual(other.modelLabel, '模型：主钥匙', '拿别人的绑定当它的，比读不到更坏')
})

test('模型：逐支那一份里这一支**没指定** ⇒ 「未指定」（事实，不是读不到）', () => {
  const config: HomeConfigView = {
    ...CONFIG,
    bindingState: 'absent',
    bound: null,
    boundPerResident: [{ agentId: 'agent-7', state: 'absent', bound: null }],
  }
  assert.equal(agentRowStatus({ view: STATUS, config, agentId: 'agent-7' }).modelLabel, '模型：未指定')
})

// ── 三、最近活动那一格：原始行 → 时间 ──────────────────────────────────

test('★ 最近活动：从活动标识里还原时间，**绝不把那一行原话摆出来**', () => {
  const row = agentRowStatus({ view: STATUS, config: CONFIG, agentId: 'agent-7' })
  assert.ok(row.activityLabel.startsWith('最近活动：'), row.activityLabel)
  assert.ok(!row.activityLabel.includes('run='), '原始行（run=…）不许上屏')
  assert.ok(!row.activityLabel.includes('drives='), '原始行（drives=…）不许上屏')
  assert.ok(!row.activityLabel.includes('2026-09-23T'), '不许裸 ISO：时间要本地化后再出现')
  assert.ok(
    row.activityLabel.includes('（推算）'),
    '时间是**解出来的**，必须把这一点说出来',
  )
  // 正锚：那一格确实给出了一个时间（不是缺省占位）。
  assert.ok(!row.activityLabel.includes('—'), row.activityLabel)
})

test('最近活动：还一次都没醒过 / 标识解不出时间 / 读数没读到，三档如实分开', () => {
  const never: HomeStatusView = { ...STATUS, lastWake: null }
  assert.equal(agentRowStatus({ view: never, config: null, agentId: 'a' }).activityLabel, '最近活动：—')

  const weird: HomeStatusView = { ...STATUS, lastWake: 'wake run=not-a-run-id drives=0' }
  assert.equal(
    agentRowStatus({ view: weird, config: null, agentId: 'a' }).activityLabel,
    '最近活动：—（这次没有读到时间）',
  )

  assert.equal(
    agentRowStatus({ view: null, config: null, agentId: 'a' }).activityLabel,
    '最近活动：未知',
  )
})

// ── 四、一格读不到不连累另外两格 ────────────────────────────────────────

test('三格各自独立：读数只有一份时，另外那格如实说未知、有读数的那格照说', () => {
  const onlyStatus = agentRowStatus({ view: STATUS, config: null, agentId: 'agent-7' })
  assert.equal(onlyStatus.onlineLabel, '在本机运行')
  assert.equal(onlyStatus.modelLabel, '模型：未知')
  assert.ok(!onlyStatus.activityLabel.includes('未知'))

  const onlyConfig = agentRowStatus({ view: null, config: CONFIG, agentId: 'agent-7' })
  assert.equal(onlyConfig.onlineLabel, '状态未知')
  assert.equal(onlyConfig.modelLabel, '模型：主钥匙', '状态口读不到不该把模型那一格一起塌掉')

  const nothing = agentRowStatus({ view: null, config: null, agentId: 'agent-7' })
  assert.deepEqual(
    [nothing.onlineLabel, nothing.modelLabel, nothing.activityLabel],
    ['状态未知', '模型：未知', '最近活动：未知'],
  )
})

// ── 五、人话（设计词汇给设计者，界面语言给用户）─────────────────────────

test('★ 本刀新写的三格文案不出现工程黑话（「入籍」这类词不上屏）', () => {
  // 只钉**本刀新写的这几句**，不宣称全局词表（`Agent 的主页` 里「后端绑定」这类标题
  // 是既有文案，不归这条管）。词表取自 `verify-ui.mjs` 的人话自检那一行 ＋「入籍」。
  const samples = [
    agentRowStatus({ view: STATUS, config: CONFIG, agentId: 'agent-7' }),
    agentRowStatus({ view: null, config: null, agentId: 'agent-7' }),
  ]
  const text = samples
    .flatMap((row) => [row.onlineLabel, row.modelLabel, row.activityLabel])
    .join(' ')
  for (const word of ['入籍', '读数', '准入', '执行端', '在岗', '唤醒']) {
    assert.ok(!text.includes(word), `「${word}」不该出现在列表行文案里：${text}`)
  }
})

// ── 六、`lastWakeReadout`：三档缺省如实分开（没读数 / 没醒过 / 解不出时间）＋ 成功那档 ──
//
// ⚠ 2026-09-26：本节原先还有五组针对 `herLastWakeText`（聊天页头部那条家读数）的用例。
// 那条读数**已按第一性原理撤出 IM** —— IM 不展示 Agent 的身体态与宿主读数（那是执行过程，
// 归执行端；`ChatPage` 头部现在只报静态参与者统计）。函数与那五组用例一并删除，
// 不是"暂不跑"。`lastWakeReadout` 本身仍被机器页（`/app/runtimes`）使用，故留下。

/** 一台远程家（形状就是存进 IndexedDB 的样子）。 */
const REMOTE: HomeEntry = {
  id: 'home-1',
  name: '书房那台',
  baseUrl: 'https://192.168.1.20',
  statusPort: 5190,
  configPort: 5191,
  key: 'the-home-key',
  local: false,
}

function reading(entry: HomeEntry, view: HomeStatusView | null, failure: string | null = null): HomeReading<HomeStatusView> {
  return { entry, view, failure }
}

test('★ lastWake 读数四态：没读数 / 没醒过 / 解不出时间 / 解出时刻，一个都不许压成另一个', () => {
  assert.deepEqual(lastWakeReadout(null), { kind: 'unread' })
  assert.deepEqual(lastWakeReadout({ ...STATUS, lastWake: null }), { kind: 'never' })
  assert.deepEqual(lastWakeReadout({ ...STATUS, lastWake: 'wake run=not-a-run-id drives=0' }), {
    kind: 'no-time',
  })
  // 活动标识里第 2 段是毫秒（宿主文档化的机械格式）。
  assert.deepEqual(lastWakeReadout(STATUS), { kind: 'at', ms: 1789829905936 })
})

// ── 六之二、机器列表一行的处境（`machineSituation`）─────────────────────────────
//
// 这一节是 2026-09-26 审计的落点：`/app/runtimes` 的第四态原先只按"名字查不到"
// 就断言「上面那支 Agent **不在这个世界里**」—— 而名字来自 `ownedAgents`，
// 那份列表**读失败时是空数组** ⇒ **一次读取失败被页面断言成一句事实**。
// 下面是那条纪律的机械判据：**只有真的读到过，才有资格说"不在"。**

/** 一份读到了的机器读数（只用到 agentId 那一格）。 */
const MACHINE_VIEW = { agentId: 'agent-7' }

test('★ 读不到 ≠ 不在：Agent 列表读失败时，第四态**不许**说"不在这个世界里"', () => {
  const base = {
    pending: false,
    view: MACHINE_VIEW,
    failure: null,
    agentState: 'loaded',
    legacyHome: false,
    agentId: 'agent-7',
  }

  // 读到过列表、名字查不到 ⇒ 这才有资格说"不在这个世界里"（灰）。
  const readable = machineSituation({ ...base, agentName: null, agentsReadable: true })
  assert.equal(readable.tone, 'muted')
  assert.ok(readable.text.includes('不在这个世界里'), readable.text)

  // **列表本身没读到** ⇒ 只能说读不到（橙），且不许出现"不在"这个断言。
  const unreadable = machineSituation({ ...base, agentName: null, agentsReadable: false })
  assert.equal(unreadable.tone, 'warn', '读不到要跟"不在"分开档，不能一样灰')
  assert.ok(!unreadable.text.includes('不在这个世界里'), `不许把读不到说成事实：${unreadable.text}`)
  assert.ok(unreadable.text.includes('读不到'), unreadable.text)
})

test('★ 机器行四态：没开始读 / 这份读不到 / 它没报是谁 / 报的是谁 —— 一个都不许压成另一个', () => {
  const base = {
    view: MACHINE_VIEW,
    failure: null,
    agentState: 'loaded',
    legacyHome: false,
    agentId: 'agent-7',
    agentName: '小七',
    agentsReadable: true,
  }

  assert.deepEqual(machineSituation({ ...base, pending: true }).text, '正在读…')
  assert.equal(machineSituation({ ...base, pending: false, view: null, failure: '连不上这台机器' }).tone, 'warn')
  assert.ok(machineSituation({ ...base, pending: false, view: null, failure: null }).text.length > 0, '没给原因也要有话')
  assert.ok(machineSituation({ ...base, pending: false, agentId: null, agentName: null }).text.includes('没报上'))
  assert.equal(machineSituation({ ...base, pending: false }).tone, 'ok')
  assert.ok(machineSituation({ ...base, pending: false }).text.includes('小七'))
})

/**
 * ★ **一个家住两支时，那一句话必须说全**（2026-10-16 真机撞上）。
 *
 * 病根：顶层那一格（`agentId` / `agentName`）是**第一支**（口子把它抬到顶层，为的是不破坏
 * 冻结前端）⇒ 一句话只说第一支，读的人会以为**只有**它在跑。主人当次的原话：
 * **"这里为啥还显示跑的是小欣，明明小可也在"** —— 小可确实在跑（第二位住户、宿主 running）。
 *
 * 判据三条（都能被反例打红）：
 * ① **一支时一字不变**（那是既有断言的地盘，也是最常见的家）；
 * ② 两支都在跑 ⇒ **点名两支**，而且说得出"共几支"；
 * ③ 住户在、**一支都没跑** ⇒ 灰点 ＋ "现在都没在跑"（**不许**沿用绿色的"上面跑的是 X"）。
 */
test('★ 多支：两支都在跑 ⇒ 点名两支（不许只说第一支）', () => {
  const base = {
    pending: false,
    view: MACHINE_VIEW,
    failure: null,
    agentState: 'loaded',
    legacyHome: false,
    agentId: 'a-1',
    agentName: '小欣',
    agentsReadable: true,
  }
  const two = machineSituation({
    ...base,
    residents: [
      { agentId: 'a-1', name: '小欣', hostState: 'running' },
      { agentId: 'a-2', name: '小可', hostState: 'running' },
    ],
  })
  assert.equal(two.tone, 'ok')
  assert.ok(two.text.includes('小欣') && two.text.includes('小可'), two.text)
  assert.ok(two.text.includes('2 支'), `要说得出几支：${two.text}`)
  assert.ok(!/上面跑的是 小欣$/.test(two.text), '不许退化成单支那一句')

  // 一支 ⇒ **一字不变**（既有断言照着它写）。
  const one = machineSituation({
    ...base,
    residents: [{ agentId: 'a-1', name: '小欣', hostState: 'running' }],
  })
  assert.equal(one.text, '已连上 · 上面跑的是 小欣')

  // 住户在、都没在跑 ⇒ 灰点 ＋ 实话（不许绿、不许说"上面跑的是"）。
  const none = machineSituation({
    ...base,
    residents: [
      { agentId: 'a-1', name: '小欣', hostState: 'stopped' },
      { agentId: 'a-2', name: '小可', hostState: 'stopped' },
    ],
  })
  assert.equal(none.tone, 'muted')
  assert.ok(none.text.includes('都没在跑'), none.text)

  // 一支在跑、一支没在跑 ⇒ 既点名在跑的，也说清还有几支没在跑。
  const some = machineSituation({
    ...base,
    residents: [
      { agentId: 'a-1', name: '小欣', hostState: 'running' },
      { agentId: 'a-2', name: '小可', hostState: 'stopped' },
    ],
  })
  assert.ok(some.text.includes('小欣'), some.text)
  assert.ok(some.text.includes('另外 1 支没在跑'), some.text)

  // 名字读不到**不许**把那一支吞掉（"也在"被吞掉正是这一次的错）。
  const unnamed = machineSituation({
    ...base,
    residents: [
      { agentId: 'a-1', name: '小欣', hostState: 'running' },
      { agentId: 'a-2', name: null, hostState: 'running' },
    ],
  })
  assert.ok(unnamed.text.includes('小欣'), unnamed.text)
  assert.ok(unnamed.text.includes('名字读不到'), unnamed.text)
  assert.ok(unnamed.text.includes('2 支'), unnamed.text)

  // "接管来的孤儿"确实在跑（账本没指向它）—— 不许当成没跑。
  const adopted = machineSituation({
    ...base,
    residents: [
      { agentId: 'a-1', name: '小欣', hostState: 'running-orphan' },
      { agentId: 'a-2', name: '小可', hostState: 'running' },
    ],
  })
  assert.ok(adopted.text.includes('2 支'), adopted.text)
})

// ★ 2026-10-11：**"缺省"与"读不到"必须分开** —— 这一条是那次假警报的机械判据。
//
// 一个**全新的家**（还没有 Agent）载荷是 `agent: { state: 'no-resident' }`，**没有 `agentId`**
// ⇒ 从前那一支只看 `agentId === null`，于是第一次来的用户读到
// 「它没报上上面跑的是谁（可能是记录读不出来，或口子版本旧）」——**假警报**。
// 同一格事实有**三条来路**，谁也不许冒充谁（`scripts/home/status-server.js` 那条
// "缺省不等于故障"的注释写着同一类事故：当时修的是「运行」那一间，漏了这里）。
test('★ 机器行·三条来路分开：零住户（缺省）/ 老形态 / 有读数但没报 id', () => {
  const common = {
    pending: false,
    view: {},
    failure: null,
    agentId: null,
    agentName: null,
    agentsReadable: true,
  }

  const fresh = machineSituation({ ...common, agentState: 'no-resident', legacyHome: false })
  assert.equal(fresh.kind, 'no-resident')
  assert.ok(fresh.text.includes('还没有 Agent'), fresh.text)
  assert.ok(!fresh.text.includes('没报上'), `缺省不许说成读不出来：${fresh.text}`)

  const legacy = machineSituation({ ...common, agentState: 'no-resident', legacyHome: true })
  assert.equal(legacy.kind, 'legacy-home')
  assert.ok(legacy.text.includes('老形态'), legacy.text)
  assert.ok(
    !legacy.text.includes('还没有 Agent'),
    `老形态不许说成"还没有 Agent"（那一支明明在，只是住在旧布局里）：${legacy.text}`,
  )

  const noId = machineSituation({ ...common, agentState: 'loaded', legacyHome: false })
  assert.equal(noId.kind, 'no-id')
  assert.ok(noId.text.includes('没报上'), `这一档才是"读不出来"：${noId.text}`)
})

// ── 七、「我的 Agent」列表行按**家条目认领**算三格（`agentRowStatusFromHomes`）──────
//
// ⚠ 这一段是本刀补的**空白**：此前 `agentRowStatusFromHomes` 全仓只有一个调用点，
// **一行测试都没有** —— 而它渲染的是「我的 Agent」这个主界面。于是下面这条自相矛盾
// 活了下来：同一个输入（一份读数都没回来 + 只配了一个家）下，Agent 的主页显示那台机器的
// 模型读数，而列表行说「模型：未知」，两条都在跑、门禁全绿。
//
// 判据分两半：靠认领结果选标签的那一半（claimed 的本机/远程、unknown-id、elsewhere、
// unreachable、null），以及**模型那一格不许被状态那一格连累**那一半。

test('行·认领 claimed（本机）：绿点说「在本机运行」，模型用那台机器的读数', () => {
  const local = localHomeEntry()
  const resolution = resolveAgentHome([reading(local, STATUS)], 'agent-7')
  const row = agentRowStatusFromHomes({ resolution, config: CONFIG, agentId: 'agent-7' })
  assert.equal(row.onlineLabel, '在本机运行')
  assert.equal(row.onlineTone, 'local')
  assert.equal(row.modelLabel, '模型：主钥匙', '有绑定时显示的是那台机器上**绑定的名字**')
})

test('★ 行·认领 claimed（远程）：**不许说"本机"** —— 住址是它的名字', () => {
  const resolution = resolveAgentHome([reading(REMOTE, STATUS)], 'agent-7')
  const row = agentRowStatusFromHomes({ resolution, config: CONFIG, agentId: 'agent-7' })
  assert.equal(row.onlineLabel, '在书房那台上运行')
  assert.ok(!row.onlineLabel.includes('本机'), row.onlineLabel)
})

test('★ 行·第 2 支（认领靠住户清单）：在线照旧绿，但**活动那一格不许拿第一支的顶**', () => {
  // 2026-10-08：顶层那几格是**第一支**的 ⇒ 第 2 支这一行如果照旧读 `view.lastWake`，
  // 显示的就是**别人的**最近活动（比留空更坏：留空用户会去问，错值没人核对）。
  const multi: HomeStatusView = {
    ...STATUS,
    // ⚠ 顶层那一格是**第一支**的（这里必须是 `agent-1`）—— 若照抄 `STATUS` 的 `agent-7`，
    //   认领会走"顶层那一格"那条判据，这一条用例就量不到"靠清单认领"那一档了。
    agent: { ...STATUS.agent, agentId: 'agent-1' },
    host: { state: 'running' },
    residents: [
      { agentId: 'agent-1', slot: 1, host: { state: 'running' } },
      { agentId: 'agent-7', slot: 2, host: { state: 'stopped' } },
    ],
  }
  const resolution = resolveAgentHome([reading(localHomeEntry(), multi)], 'agent-7')
  assert.equal(resolution.kind, 'claimed')
  assert.equal(resolution.kind === 'claimed' ? resolution.ownReadings : null, false, '认领靠清单那一档')
  const row = agentRowStatusFromHomes({ resolution, config: CONFIG, agentId: 'agent-7' })
  assert.equal(row.onlineLabel, '在本机 · 它的宿主没在跑', '它**自己**那一项里的宿主读数说了算')
  assert.equal(row.onlineTone, 'local')
  assert.equal(row.modelLabel, '模型：主钥匙', '模型那一格是那台机器的读数，照旧给')
  assert.ok(row.activityLabel.includes('读不到'), row.activityLabel)
  assert.ok(!row.activityLabel.includes('推算'), `不许拿第一支的 lastWake 顶：${row.activityLabel}`)
})

test('行·认领 unknown-id：状态那格说未知，**模型照旧显示**（不冒充"未接入"）', () => {
  const noId: HomeStatusView = { ...STATUS, agent: { ...STATUS.agent, agentId: undefined } }
  const resolution = resolveAgentHome([reading(localHomeEntry(), noId)], 'agent-7')
  assert.equal(resolution.kind, 'unknown-id')
  const row = agentRowStatusFromHomes({ resolution, config: CONFIG, agentId: 'agent-7' })
  assert.equal(row.onlineLabel, '状态未知')
  assert.equal(row.modelLabel, '模型：主钥匙')
})

test('★ 行·认领 elsewhere：那些读数**不是 Agent**的 ⇒ 别台的 config 塞进来也不许上这一行', () => {
  const resolution = resolveAgentHome([reading(localHomeEntry(), STATUS)], 'agent-9')
  const row = agentRowStatusFromHomes({ resolution, config: CONFIG, agentId: 'agent-7' })
  assert.equal(row.onlineLabel, '未接入')
  assert.equal(row.modelLabel, '模型：未知', '认领到别人名下时，模型那一格不许拿别台的来顶')
})

test('★ 行·认领 unreachable：状态塌成未知，**模型那一格不跟着塌**（调用方给的那一条）', () => {
  const local = localHomeEntry()
  const resolution = resolveAgentHome([reading(local, null, '连不上本机服务')], 'agent-7')
  assert.equal(resolution.kind, 'unreachable')
  // 负对照（旧形状）：调用方给不出 config（旧 AgentsListPanel 自己拼的局部判据在
  // unreachable 时给 null）⇒ 这一格就是「模型：未知」，与主页的读数相反。
  assert.equal(
    agentRowStatusFromHomes({ resolution, config: null, agentId: 'agent-7' }).modelLabel,
    '模型：未知',
  )
  // 新判据：`mindEntryFor` 挑出的那一条（只配了一个家 ⇒ 没有歧义）算出来的 config
  // 必须**照旧上这一行**（旧实现这里硬写 `config: null`，把它丢掉了）。
  const row = agentRowStatusFromHomes({ resolution, config: CONFIG, agentId: 'agent-7' })
  assert.equal(row.onlineLabel, '状态未知', '状态口读不到 ⇒ 状态那格如实说未知')
  assert.equal(row.modelLabel, '模型：主钥匙', '状态口读不到不该把模型那一格一起塌掉')
  assert.equal(row.activityLabel, '最近活动：未知')
})

test('行·条目还没就绪（resolution = null）：三格全未知，不猜', () => {
  const row = agentRowStatusFromHomes({ resolution: null, config: CONFIG, agentId: 'agent-7' })
  assert.deepEqual(
    [row.onlineLabel, row.modelLabel, row.activityLabel],
    ['状态未知', '模型：未知', '最近活动：未知'],
  )
})
