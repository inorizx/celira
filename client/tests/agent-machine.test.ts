// Agent 主页第二段「机器」的纯逻辑测试（2026-09-29）。
//
// 这一段治的是一个**死胡同**：`/app/agents/:id` 原先的出口只有「返回」与「撤销」，
// 而用户手里那个 Agent 在不在跑、住在哪台机器上，页面一个字都不说。
//
// 三条判据（都是不许压成别的形状的东西）：
//  ① **四态不许压成两态**：读不到 ≠ 没有机器在跑它 ≠ 说不准；
//  ② **不复述运行状态**：运行/活动/模型三间的唯一权威在机器页 —— 这一段只说"住哪台"；
//  ③ **读数文本不许给假路径**（2026-10-09 前提翻转）：从前 v1 没有"给已存在的 Agent 安家"
//     的路 ⇒ 那时任何指路都是假的；现在**有**那条路（`shouldOfferAttach` ＋ 面板上的出口），
//     但**读数只说事实、指路归出口** —— 两句混在一起就是两个答案。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { agentMachineReadout, modelChoiceMachine, shouldOfferAttach } from '../src/api/agentMachine.ts'
import { resolveAgentHome, type HomeEntry, type HomeReading } from '../src/api/homes.ts'
import type { HomeStatusView } from '../src/api/homeStatus.ts'

/** 状态口的一份合法读数（形状照 `tests/agent-row-status.test.ts` 的 `STATUS` 抄）。 */
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
  lastWake: null,
}

/** 预置的本机条目（`id` 固定是 `local`）。 */
const LOCAL: HomeEntry = {
  id: 'local',
  name: '本机',
  baseUrl: '',
  statusPort: 5190,
  configPort: 5191,
  key: '',
  local: true,
}

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

function reading(
  entry: HomeEntry,
  view: HomeStatusView | null,
  failure: string | null = null,
): HomeReading<HomeStatusView> {
  return { entry, view, failure }
}

/** 把读数交给**唯一**的归属裁决（`homes.ts`），再翻成人话 —— 与页面同一条链。 */
const readoutOf = (readings: HomeReading<HomeStatusView>[], agentId: string) =>
  agentMachineReadout({ resolution: resolveAgentHome(readings, agentId) })

// ── 一、四态各说各的话（一个都不许压成另一个）──────────────────────────────

test('★ 家条目还在读 ⇒ 「正在读…」，且**不给出口**（还没有可跳的目标）', () => {
  const readout = agentMachineReadout({ resolution: null })
  assert.equal(readout.kind, 'reading')
  assert.equal(readout.target, null)
  assert.equal(readout.text, '正在读…')
})

test('★ claimed（本机）⇒ 说得出住址，且**不写"本机本机"**重复', () => {
  const readout = readoutOf([reading(LOCAL, STATUS)], 'agent-7')
  assert.equal(readout.kind, 'claimed')
  assert.equal(readout.text, '它住在本机。')
  assert.deepEqual(readout.target, { entryId: 'local', label: '本机' })
})

test('★ claimed（远程）⇒ 住址是**那台的名字**，出口指向它自己的 entryId', () => {
  const view: HomeStatusView = { ...STATUS, agent: { ...STATUS.agent, agentId: 'agent-7' } }
  const readout = readoutOf([reading(LOCAL, null, '连不上本机服务（先开机：node home.js up）'), reading(REMOTE, view)], 'agent-7')
  assert.equal(readout.kind, 'claimed')
  assert.equal(readout.text, '它住在书房那台。')
  assert.deepEqual(readout.target, { entryId: 'home-1', label: '书房那台' })
})

test('★ unknown-id（那台没报上上面跑的是谁）⇒ **说不准**，不许说成"未接入"', () => {
  const noId: HomeStatusView = { ...STATUS, agent: { ...STATUS.agent, agentId: undefined } }
  const readout = readoutOf([reading(LOCAL, noId)], 'agent-7')
  assert.equal(readout.kind, 'no-id')
  assert.equal(readout.text, '本机没报上上面跑的是谁，说不准它住不住在那台。')
  // 说不准也**有可去的目标**：那台机器是唯一候选，用户该能去看它。
  assert.deepEqual(readout.target, { entryId: 'local', label: '本机' })
})

test('★ elsewhere（报的都是别人的 id）⇒ 那句**带作用域**；**读数文本**不夹带指路', () => {
  const other: HomeStatusView = { ...STATUS, agent: { ...STATUS.agent, agentId: 'someone-else' } }
  const readout = readoutOf([reading(LOCAL, other)], 'agent-7')
  assert.equal(readout.kind, 'elsewhere')
  assert.equal(readout.text, '能读到的执行端都没有报它在跑。')
  assert.equal(readout.target, null)
  // ⚠ **作用域不许丢**（2026-10-11）：这一档的判据是"**读到的**都不是它"（`homes.ts`：
  //    `readings.some(view !== null)` 且无一匹配 ⇒ 另有条目可能**没读到**）。
  //    它曾写作「还没有机器在跑它。」—— 那是把"我没看见"说成"没有"。边界是**两个可替换的
  //    平面**（世界侧会有别的执行端、执行端会连别的世界），那句话在别的通道在场时就是假话。
  //    判据一句话：**它是"我没看见"还是"没有"？**
  assert.ok(readout.text.includes('能读到'), `这一句必须带作用域（"我没看见"≠"没有"）：${readout.text}`)
  assert.ok(
    !readout.text.includes('没有机器在跑'),
    `不许写成不带作用域的"没有机器在跑"：${readout.text}`,
  )
  // ⚠ **这一条的前提在 2026-10-09 被主人裁 B 翻转了**：从前 v1 没有"给一个已存在的 Agent
  // 安家"的路径 ⇒ 那时任何"去接上/去安家"都是**假路径**（`client/README.md`「已知缺口」）。
  // 现在**有**那条路了（`shouldOfferAttach` ＋ 面板上那个出口）—— 但**读数文本**照旧不许夹带
  // 指路：指路归**出口**（按钮），读数只说事实（两句混在一起就是两个答案）。
  for (const promise of ['接上', '安家', '绑上', '配置']) {
    assert.ok(!readout.text.includes(promise), `读数不许夹带指路：「${promise}」：${readout.text}`)
  }
})

test('★ shouldOfferAttach：**本机没认领它**就给出口（不给的只有"已经有家"与"读不到"）', () => {
  const noId: HomeStatusView = { ...STATUS, agent: { ...STATUS.agent, agentId: undefined } }
  const other: HomeStatusView = { ...STATUS, agent: { ...STATUS.agent, agentId: 'someone-else' } }
  const cases: Array<[string, Parameters<typeof shouldOfferAttach>[0], boolean]> = [
    ['还没读回来（null）⇒ 不给（读不到时给一个会动手的按钮＝替用户赌一把）', null, false],
    ['claimed（已经有家了）⇒ 不给', resolveAgentHome([reading(LOCAL, STATUS)], 'agent-7'), false],
    // ⚠ **这一格第一版读错了**（写成 false ✗，而它恰恰是主人今天撞上的那一格）：
    // `unknown-id` = 有读数但**都没有 id 那一格** = "这台机器没报上面跑的是谁"
    // ⇒ 最常见的样子就是**一台还没有住户的机器**（`agent.state = no-resident`）⇒ 必须**给**。
    [
      'unknown-id（机器没报跑的是谁＝多半还没有住户）⇒ **给**',
      resolveAgentHome([reading(LOCAL, noId)], 'agent-7'),
      true,
    ],
    ['elsewhere（报的都是别人的 id）⇒ **给**', resolveAgentHome([reading(LOCAL, other)], 'agent-7'), true],
    ['unreachable（读不到那台机器）⇒ 不给', resolveAgentHome([reading(LOCAL, null, '连不上')], 'agent-7'), false],
  ]
  for (const [why, resolution, expected] of cases) {
    assert.equal(shouldOfferAttach(resolution), expected, why)
  }
  // 负向对照两条，各自堵一个方向：
  assert.ok(
    cases.filter(([, , expected]) => !expected).length >= 2,
    '至少两态必须**不给**出口（否则它就不是判据了）',
  )
  assert.ok(
    cases.filter(([, , expected]) => expected).length >= 2,
    '至少两态必须**给**出口 —— 只认一态就会漏掉"机器上还没有住户"那一格（**实测漏过一次**）',
  )
})

test('★ unreachable + 原因 ⇒ 把原因**原样带上**（不吞掉"读不到"的证据）', () => {
  const readout = readoutOf([reading(LOCAL, null, '连不上本机服务（先开机：node home.js up）')], 'agent-7')
  assert.equal(readout.kind, 'unreachable')
  assert.equal(readout.text, '读不到它住在哪台机器上 —— 连不上本机服务（先开机：node home.js up）。')
  assert.equal(readout.target, null)
})

test('★ unreachable 无原因（没有条目 / 没读到原因）⇒ 只说不确定，**不编原因**', () => {
  const readout = readoutOf([], 'agent-7')
  assert.equal(readout.kind, 'unreachable')
  assert.equal(readout.text, '读不到它住在哪台机器上。')
  assert.equal(readout.target, null)
})

// ── 一·B、★「它用哪一条模型」这一笔该发给哪台机器（2026-10-14「选择先于接上」）─────
//
// 症状（主人当次撞上）：机器上有**两条**模型来源，接她时壳按"选哪一条 = 花谁的钱，不替她决定"
// 停下来；而"选"的那一格从前要求**某台机器已认领她**（`claimed`）—— 她还没被接上 ⇒ 那一格
// 根本不摆 ⇒ 两边互等。判据改成三档（`modelChoiceMachine`）：认领到就那台；没认领就指**本机**
// （app 里"接上"只指本机）；还没裁决不摆。**写在哪一支上没变**（永远是她自己那一支）。

test('★ 认领到了 ⇒ 就写在那一台上（远程也照旧，旧口径一字未改）', () => {
  const view: HomeStatusView = { ...STATUS, agent: { ...STATUS.agent, agentId: 'agent-7' } }
  const resolution = resolveAgentHome([reading(LOCAL, null, '连不上'), reading(REMOTE, view)], 'agent-7')
  const choice = modelChoiceMachine({ resolution, local: { entry: LOCAL, configReadable: true } })
  assert.deepEqual(choice, { entry: REMOTE, homed: true })
})

test('★ 没认领 + 本机清单读得到 ⇒ 指**本机**，且 `homed: false`（"先给她选好"）', () => {
  const other: HomeStatusView = { ...STATUS, agent: { ...STATUS.agent, agentId: 'someone-else' } }
  const resolution = resolveAgentHome([reading(LOCAL, other)], 'agent-7')
  assert.deepEqual(modelChoiceMachine({ resolution, local: { entry: LOCAL, configReadable: true } }), {
    entry: LOCAL,
    homed: false,
  })
  // `unknown-id`（那台没报上面跑的是谁）同档：她没被任何机器认领 ⇒ 也是"先给她选好"。
  const noId: HomeStatusView = { ...STATUS, agent: { ...STATUS.agent, agentId: undefined } }
  assert.deepEqual(
    modelChoiceMachine({
      resolution: resolveAgentHome([reading(LOCAL, noId)], 'agent-7'),
      local: { entry: LOCAL, configReadable: true },
    }),
    { entry: LOCAL, homed: false },
  )
})

test('★ 没认领但**本机清单读不到**（或压根没有本机条目）⇒ 不摆：读不到就别让人对着空气选', () => {
  const other: HomeStatusView = { ...STATUS, agent: { ...STATUS.agent, agentId: 'someone-else' } }
  const resolution = resolveAgentHome([reading(LOCAL, other)], 'agent-7')
  assert.equal(modelChoiceMachine({ resolution, local: { entry: LOCAL, configReadable: false } }), null)
  assert.equal(modelChoiceMachine({ resolution, local: null }), null)
})

test('★ 还没裁决（家条目还在读）⇒ 不摆（那时连"是哪台"都还没有）', () => {
  assert.equal(
    modelChoiceMachine({ resolution: null, local: { entry: LOCAL, configReadable: true } }),
    null,
  )
})

test('★ 两台机器、都没认领它 ⇒ 仍指**本机**（"接上"在 app 里只指本机，不是拿第一台顶）', () => {
  const other: HomeStatusView = { ...STATUS, agent: { ...STATUS.agent, agentId: 'someone-else' } }
  const otherRemote: HomeStatusView = { ...STATUS, agent: { ...STATUS.agent, agentId: 'yet-another' } }
  const resolution = resolveAgentHome(
    [reading(LOCAL, other), reading(REMOTE, otherRemote)],
    'agent-7',
  )
  assert.deepEqual(modelChoiceMachine({ resolution, local: { entry: LOCAL, configReadable: true } }), {
    entry: LOCAL,
    homed: false,
  })
})

// ── 二、这一段的主语是机器：不复述运行状态、不变成 IM 的存在态 ────────────────
test('★ 不复述运行状态，也不写成 Agent 在世界里的存在态（红线）', () => {
  const noId: HomeStatusView = { ...STATUS, agent: { ...STATUS.agent, agentId: undefined } }
  const other: HomeStatusView = { ...STATUS, agent: { ...STATUS.agent, agentId: 'someone-else' } }
  const samples = [
    agentMachineReadout({ resolution: null }),
    readoutOf([reading(LOCAL, STATUS)], 'agent-7'),
    readoutOf([reading(LOCAL, noId)], 'agent-7'),
    readoutOf([reading(LOCAL, other)], 'agent-7'),
    readoutOf([reading(LOCAL, null, '连不上这台机器 —— 地址或端口不对')], 'agent-7'),
  ]
  const text = samples.map((each) => each.text).join(' ')
  // 「机器在不在跑 / 醒过几次 / 用哪条模型」的唯一权威是机器页那三个房间
  // （`/app/runtimes/:entryId`）—— 这一段再算一遍就是第二个答案。
  for (const duplicated of ['醒过', '最近活动', '能不能开始']) {
    assert.ok(!text.includes(duplicated), `不许复述机器页的读数：「${duplicated}」：${text}`)
  }
  // 世界侧的存在态（IM 的 presence）不许出现在这里 —— 换成微信这一段就不成立了。
  for (const presence of ['在线', '正在写', '正在输入']) {
    assert.ok(!text.includes(presence), `不许写成 IM 的存在态：「${presence}」：${text}`)
  }
})

test('★ 本段文案不出现内行词与称谓病句（与 `scripts/jargon.mjs` 同一张表）', () => {
  const noId: HomeStatusView = { ...STATUS, agent: { ...STATUS.agent, agentId: undefined } }
  const samples = [
    agentMachineReadout({ resolution: null }),
    readoutOf([reading(LOCAL, STATUS)], 'agent-7'),
    readoutOf([reading(LOCAL, noId)], 'agent-7'),
    readoutOf([reading(REMOTE, null, '连不上这台机器 —— 地址或端口不对')], 'agent-7'),
  ]
  const text = samples.map((each) => each.text).join(' ')
  // 内行词表（`scripts/jargon.mjs` 是唯一权威，这里只取与本段有关的几条 ——
  // 别把整张表抄过来，抄过来就是第二个权威）。
  for (const word of ['读数', '准入', '唤醒', '清醒', 'Core', 'Runtime']) {
    assert.ok(!text.includes(word), `内行词不该上屏：「${word}」：${text}`)
  }
  // 称谓刀（`verify-ui.mjs` 的 `BANNED_SUBJECT`）：名字不在手时不许写「Agent 的主页」这类。
  assert.ok(!text.includes('Agent 的主页'), `称谓病句：${text}`)
})
