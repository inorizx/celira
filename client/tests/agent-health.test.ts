// 「这一支 Agent 现在正常吗」那一句人话的纯逻辑测试（2026-10-06）。
//
// 这一层的判据有两条要紧的：
//   ① **"没在跑"与"读不到"不许长得一样** —— 把缺省说成一个事实比空白更坏，
//      那会让人去修一台没坏的机器；
//   ② **产出的每一句都不许含内行词** —— `verify-ui` 拿 `scripts/jargon.mjs` 扫渲染后的正文，
//      而那份词表从前"规则一直写着、判据一直没人跑"。这里拿**同一份词表**离线扫一遍全矩阵。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import {
  agentHealthForClaimedMachine,
  agentHealthFromHomes,
  agentHealthReadout,
  hostStateLabel,
} from '../src/api/agentHealth.ts'
import type { HomeStatusView } from '../src/api/homeStatus.ts'
import type { AgentHomeResolution, HomeEntry } from '../src/api/homes.ts'
import { leakedJargon } from '../scripts/jargon.mjs'

/** 一份合法读数（只覆盖这一层用得到的那几格）。 */
function view(over: {
  host?: HomeStatusView['host']
  stale?: boolean
  agent?: Partial<HomeStatusView['agent']>
  legacyHome?: boolean
} = {}): HomeStatusView {
  return {
    readAt: '2026-10-06T06:00:00.000Z',
    stale: over.stale ?? false,
    im: { health: 200 },
    agent: {
      state: 'loaded',
      admission: 'ready',
      runs: [{ id: 'host-1-2-3', state: 'ended', outcome: 'subject:completed' }],
      ...over.agent,
    },
    lastWake: 'wake run=host-1-2-3 drives=1 ending=subject:completed',
    ...(over.host ? { host: over.host } : {}),
    ...(over.legacyHome === undefined ? {} : { legacyHome: over.legacyHome }),
  }
}

/**
 * 认领结果只在 `kind` / `view` / `ownReadings` / `entry` 四格上被这一层读，测试里按形状造。
 *
 * ⚠ `ownReadings` **缺省给 `true`**（= 认领靠顶层那一格：单支的家，或这个家的第一支）——
 * 本文件既有的用例说的都是那一档。第 2 支起那一档（认领靠住户清单）单开用例。
 * ⚠ `entry` 是**那一句话里点名的那台机器**（2026-10-20 起；缺省本机那条）。
 */
function resolution(
  kind: string,
  value?: HomeStatusView,
  ownReadings = true,
): AgentHomeResolution<HomeStatusView> {
  const entry: HomeEntry = {
    id: 'local',
    name: '本机',
    baseUrl: '',
    statusPort: 5190,
    configPort: 5191,
    key: '',
    local: true,
  }
  return (
    value ? { kind, entry, view: value, ownReadings } : { kind }
  ) as unknown as AgentHomeResolution<HomeStatusView>
}

/**
 * ★ **宿主那一格的短标签**（2026-10-12，机器页住户表用：一支一行"在跑 / 没在跑"）。
 *
 * 判据两条：① **四态各有各的话**（`stopped` 与 `unknown` 尤其不许长得一样 —— 那是"没在跑"与
 * "读不到"混成一格，本文件头一条判据的同一把尺子）；② 与那句长话**同一个来源**（`hostStateOf`）。
 */
test('★ 宿主四态各有一句短标签（"没在跑"与"读不到"不许长得一样）', () => {
  assert.equal(hostStateLabel({ state: 'running' }), '在跑')
  assert.equal(hostStateLabel({ state: 'running-orphan' }), '在跑（不是这次开机起的）')
  assert.equal(hostStateLabel({ state: 'stopped' }), '没在跑')
  assert.equal(hostStateLabel(undefined), '读不到')
  // 认不出的取值 ⇒ 读作"读不到"（**不编**，也不整份拒绝）。
  assert.equal(hostStateLabel({ state: 'something-new-from-a-newer-door' }), '读不到')
  // 不许把内行词带上屏（与这一层其余句子同一份词表）。
  for (const state of ['running', 'running-orphan', 'stopped', 'unknown'] as const) {
    assert.deepEqual(leakedJargon(hostStateLabel({ state })), [], `${state} 那一句漏了内行词`)
  }
})

test('正常：它正在跑 + 主体自己收场 ⇒ 绿，且两个平面各说各的', () => {
  const readout = agentHealthReadout(view({ host: { state: 'running' } }))
  assert.equal(readout.tone, 'ok')
  assert.equal(readout.text, '它正在跑 · 上次：完成')
})

test('沉默不是故障：`subject:silent` 照旧是绿的', () => {
  const readout = agentHealthReadout(
    view({
      host: { state: 'running' },
      agent: { runs: [{ id: 'r', state: 'ended', outcome: 'subject:silent' }] },
    }),
  )
  assert.equal(readout.tone, 'ok')
  assert.equal(readout.text, '它正在跑 · 上次：未回复')
})

test('主体选择拒绝 / 等待：也各有中文，不露英文词', () => {
  for (const [outcome, label] of [
    ['subject:declined', '拒绝'],
    ['subject:waiting', '等外部结果'],
  ] as const) {
    const readout = agentHealthReadout(
      view({ host: { state: 'running' }, agent: { runs: [{ id: 'r', state: 'ended', outcome }] } }),
    )
    assert.equal(readout.text, `它正在跑 · 上次：${label}`)
    assert.equal(readout.tone, 'ok')
  }
})

test('系统结束 ⇒ 黄，且**分类不上这一句**（它在机器页的「活动」房间）', () => {
  const readout = agentHealthReadout(
    view({
      host: { state: 'running' },
      agent: {
        runs: [
          { id: 'r', state: 'ended', outcome: 'system:model-failed', failure: 'credential-rejected' },
        ],
      },
    }),
  )
  assert.equal(readout.tone, 'warn')
  assert.equal(readout.text, '它正在跑 · 上次：没正常结束')
  assert.ok(!readout.text.includes('credential-rejected'), '分类不上这一句')
})

test('它现在没在跑（这台机器上没有它的进程） ⇒ 黄；这一句必须与"读不到"长得不一样', () => {
  const stopped = agentHealthReadout(view({ host: { state: 'stopped' } }))
  assert.equal(stopped.tone, 'warn')
  assert.equal(stopped.text, '它现在没在跑（这台机器上没有它的进程） · 上次：完成')
  // `host` 缺那一格 = 这一面没读数 ⇒ **不许**说"没在跑"。
  const missing = agentHealthReadout(view())
  assert.equal(missing.tone, 'muted')
  assert.equal(missing.text, '它现在跑没跑读不到 · 上次：完成')
  assert.ok(!missing.text.includes('没在跑'), '缺省不许说成事实')
  // 取值认不出 ⇒ 同一句话（归一成"这一格没读数"）。
  const unknownValue = agentHealthReadout(view({ host: { state: 'starting' } }))
  assert.equal(unknownValue.text, '它现在跑没跑读不到 · 上次：完成')
  // `running-orphan`：在跑，但**不是这次开机起的** —— 如实说。
  assert.equal(
    agentHealthReadout(view({ host: { state: 'running-orphan' } })).text,
    '它正在跑（不是这次开机起的） · 上次：完成',
  )
})

test('该不该出手优先：已暂停 ⇒ 红；上一次没做完 ⇒ 黄', () => {
  const paused = agentHealthReadout(
    view({ host: { state: 'running' }, agent: { admission: 'paused(reason=disk)' } }),
  )
  assert.equal(paused.tone, 'bad')
  assert.equal(paused.text, '它正在跑 · 已暂停，需要你确认')
  const unfinished = agentHealthReadout(
    view({ host: { state: 'running' }, agent: { admission: 'unfinished(run=host-1-2-3)' } }),
  )
  assert.equal(unfinished.tone, 'warn')
  assert.equal(unfinished.text, '它正在跑 · 上一次没做完')
})

test('没有结束结果：判定类目说话，`unknown` 是"可能已经做了"', () => {
  const unknown = agentHealthReadout(
    view({
      host: { state: 'running' },
      agent: { runs: [{ id: 'r', state: 'unknown', outcome: null }] },
    }),
  )
  assert.equal(unknown.tone, 'warn')
  assert.equal(unknown.text, '它正在跑 · 上次：不明 —— 它可能已经做了')
  const unfinished = agentHealthReadout(
    view({
      host: { state: 'running' },
      agent: { runs: [{ id: 'r', state: 'unfinished', outcome: null }] },
    }),
  )
  assert.equal(unfinished.text, '它正在跑 · 上次：还没结束')
  // 自相矛盾的读数（口子说收过场、却没给结果）：说"读不到结果"，
  // **不**复用 `runStateLabel('ended')` —— 那个词是「已收场」，含内行词。
  const contradictory = agentHealthReadout(
    view({
      host: { state: 'running' },
      agent: { runs: [{ id: 'r', state: 'ended', outcome: null }] },
    }),
  )
  assert.equal(contradictory.text, '它正在跑 · 上次：读不到结果')
})

test('一场活动都没有 / 记录那一面读不到：各说各的（缺省不等于零）', () => {
  assert.equal(
    agentHealthReadout(view({ host: { state: 'running' }, agent: { runs: [] } })).text,
    '它正在跑 · 还没有过活动',
  )
  assert.equal(
    agentHealthReadout(
      view({ host: { state: 'running' }, agent: { state: 'unreadable', runs: [] } }),
    ).text,
    '它正在跑 · 它的记录读不到',
  )
  assert.equal(
    agentHealthReadout(
      view({ host: { state: 'running' }, agent: { state: 'no-resident', runs: [] } }),
    ).text,
    '它正在跑 · 这台机器上还没有 Agent',
  )
  // ★ 2026-10-11：**老形态的家 ≠ 没有 Agent** —— 那一支**明明在**，只是住在旧布局里、壳认不出来。
  // 同一句判据在机器页「运行」那一间从 2026-10-05 起就有，这一处（「我的 Agent」列表每一行）
  // 漏了：`emptyLedgerText` 拿到 `no-resident` 就直说"还没有 Agent"——一句假话。
  assert.equal(
    agentHealthReadout(
      view({ host: { state: 'running' }, agent: { state: 'no-resident', runs: [] }, legacyHome: true }),
    ).text,
    '它正在跑 · 这台机器的布局还是老的',
  )
  assert.equal(
    agentHealthReadout(
      view({ host: { state: 'running' }, agent: { state: 'missing-binary', runs: [] } }),
    ).text,
    '它正在跑 · 这台机器上找不到它的程序',
  )
})

test('缓存读数必须说出来（不把缓存画成当下）', () => {
  const readout = agentHealthReadout(view({ host: { state: 'running' }, stale: true }))
  assert.equal(readout.text, '（缓存）它正在跑 · 上次：完成')
})

test('整份读数没读到 ⇒ 灰，且**不说**"没在跑"', () => {
  const readout = agentHealthReadout(null)
  assert.equal(readout.tone, 'muted')
  assert.equal(readout.text, '机器这一面读不到')
  assert.ok(!readout.text.includes('没在跑'))
})

/**
 * ★ **组合断言**（2026-10-20 T2 的必加用例）：同一行输出**不可能同时**说"它正在跑"与
 * "它现在没在跑"。
 *
 * 为什么这条必须单独钉：真机上撞到的那一行是 `机器没在跑 · 上次：完成` **紧挨着**
 * 绿的「在本机运行」—— 两个相反的答案并排，而当时**两个用例各断一半**（一个只断健康那句、
 * 一个只断住址那句），于是全绿。修法就是把两半放在**同一个输出**里断：
 *
 * - 机器那一半只能落在 `machinePart` 的四句之一（第 ① 条）；
 * - 把那四句里的两句相反的话凑在一起就红（第 ② 条）；
 * - 旧的主语（「机器在跑」/「机器没在跑」）一个都不许回来（第 ③ 条）。
 */
test('★ 组合：同一句里不可能既说"它正在跑"又说"它现在没在跑"（两个答案不许并排）', () => {
  const forms = {
    running: '它正在跑',
    orphan: '它正在跑（不是这次开机起的）',
    stopped: '它现在没在跑（这台机器上没有它的进程）',
    unknown: '它现在跑没跑读不到',
  } as const
  const ledgers = [
    view({ agent: { runs: [{ id: 'r', state: 'ended', outcome: 'subject:completed' }] } }),
    view({ agent: { runs: [] } }),
    view({ agent: { admission: 'paused(reason=disk)' } }),
  ]
  for (const base of ledgers) {
    for (const [host, expected] of [
      ['running', forms.running],
      ['running-orphan', forms.orphan],
      ['stopped', forms.stopped],
      // 取值认不出 ⇒ 归一成"这一格没读数"（**不许**说成没在跑）。
      ['starting', forms.unknown],
    ] as const) {
      const { text } = agentHealthReadout({ ...base, host: { state: host } })
      assert.ok(
        text.startsWith(expected),
        `机器那一半该是「${expected}」，实际：${text}`,
      )
      const saysRunning = text.includes('它正在跑')
      const saysStopped = text.includes('它现在没在跑')
      assert.ok(!(saysRunning && saysStopped), `这一句同时说了两个相反的答案：${text}`)
      // 旧的主语（宿主进程）不许回来 —— 那是 T2 治的那件事。
      assert.ok(
        !text.includes('机器在跑') && !text.includes('机器没在跑'),
        `宿主那套主语又回来了：${text}`,
      )
    }
  }
})

test('按认领算：认到别人那台时不许拿它的读数顶', () => {
  assert.equal(
    agentHealthFromHomes({ resolution: resolution('claimed', view({ host: { state: 'running' } })) }).text,
    '它正在跑 · 上次：完成',
  )
  // `unknown-id`（读数没报它是谁）：与三格同一条口径，照旧显示那台机器的读数。
  assert.equal(
    agentHealthFromHomes({ resolution: resolution('unknown-id', view({ host: { state: 'stopped' } })) })
      .text,
    '它现在没在跑（这台机器上没有它的进程） · 上次：完成',
  )
  const other = agentHealthFromHomes({ resolution: resolution('elsewhere') })
  assert.equal(other.tone, 'muted')
  // ★ 2026-10-11 **带作用域**：`elsewhere` 说的是"**读得到的**执行端都没有报它在跑"，
  // 不是"**这台机器**上跑的是别的 Agent"——后者把"我没看见"说成了"没有"。
  assert.equal(other.text, '未接入 —— 能读到的机器都没有报它在跑')
  // 一份都没读到：说"读不到"，不说"没在跑"。
  assert.equal(agentHealthFromHomes({ resolution: resolution('unreachable') }).text, '机器这一面读不到')
  // 家条目还没就绪：说"正在读"。
  assert.equal(agentHealthFromHomes({ resolution: null }).text, '正在读…')
})

test('★ 第 2 支起（认领靠住户清单）：住址对，但**顶层那几格不是它的** ⇒ 不许拿它算', () => {
  // 2026-10-08：一个家住多支时，顶层那几格是**第一支**的（口子把它抬到顶层给冻结前端读）。
  // 第 2 支的页面上，把这些读数摆在它名下就是张冠李戴 —— 比留空更坏（留空用户会去问）。
  const second = resolution('claimed', view({ host: { state: 'running' } }), false)
  assert.equal(agentHealthFromHomes({ resolution: second }).tone, 'muted')
  assert.equal(
    agentHealthFromHomes({ resolution: second }).text,
    '这一支的记录不在这一份里（这一份只有第一支的）',
  )
  // ⚠ 2026-10-20：这一句从前是「在本机 · 这一支的运行读数不在这一份里」——
  //    住址与"没有它的记录"混在一格，而「读数」是内行词（`scripts/jargon.mjs` 会红）。
  //    住址那一格另有其格（`machineMatchLabel`），这一句只说记录在不在。
  // 主页那一段同理：**不给**这一句（页面据此决定渲不渲染）。
  assert.equal(agentHealthForClaimedMachine({ resolution: second }), null)
})

test('Agent 主页那一段：**只有认到它自己那台**才给这一句（别台的读数不许顶）', () => {
  // 归属判据：这一句说的是"这一支的机器 + 这一支的账本"。认不准时留空 —— 留空用户会去问，
  // 错值没人核对（`agent-machine` 那条四态纪律的同一句话）。
  assert.equal(
    agentHealthForClaimedMachine({
      resolution: resolution('claimed', view({ host: { state: 'running' } })),
    })?.text,
    '它正在跑 · 上次：完成',
  )
  for (const kind of ['unknown-id', 'elsewhere', 'unreachable']) {
    assert.equal(
      agentHealthForClaimedMachine({ resolution: resolution(kind, view()) }),
      null,
      `${kind} 那一档不许给这一句（那读数未必是这一支的）`,
    )
  }
  assert.equal(agentHealthForClaimedMachine({ resolution: null }), null, '还在读时也不给')
})

test('每一句都不含内行词（拿 `verify-ui` 那份词表离线扫全矩阵）', () => {
  const views: HomeStatusView[] = [
    view(),
    view({ host: { state: 'running' } }),
    view({ host: { state: 'running-orphan' } }),
    view({ host: { state: 'stopped' } }),
    view({ host: { state: 'starting' } }),
    view({ host: { state: 'running' }, stale: true }),
    ...['subject:completed', 'subject:silent', 'subject:declined', 'subject:waiting', 'weird:new'].map(
      (outcome) =>
        view({ host: { state: 'running' }, agent: { runs: [{ id: 'r', state: 'ended', outcome }] } }),
    ),
    ...['system:model-failed', 'system:indeterminate', 'system:timeout', 'system:crashed'].map(
      (outcome) =>
        view({ host: { state: 'running' }, agent: { runs: [{ id: 'r', state: 'ended', outcome }] } }),
    ),
    ...['unfinished', 'blocked', 'unknown', 'refused', 'ended', 'something-new'].map((state) =>
      view({ host: { state: 'running' }, agent: { runs: [{ id: 'r', state, outcome: null }] } }),
    ),
    ...['loaded', 'no-resident', 'missing-binary', 'busy', 'unreadable'].map((state) =>
      view({ host: { state: 'running' }, agent: { state: state as 'loaded', runs: [] } }),
    ),
    view({ host: { state: 'running' }, agent: { admission: 'paused(reason=disk)' } }),
    view({ host: { state: 'running' }, agent: { admission: 'unfinished(run=r)' } }),
  ]
  const texts = [
    ...views.map((item) => agentHealthReadout(item).text),
    agentHealthReadout(null).text,
    agentHealthFromHomes({ resolution: null }).text,
    agentHealthFromHomes({ resolution: resolution('elsewhere') }).text,
    agentHealthFromHomes({ resolution: resolution('unreachable') }).text,
  ]
  assert.ok(texts.length >= 28, `矩阵要够宽，现在只有 ${texts.length} 句`)
  for (const text of texts) {
    assert.deepEqual(leakedJargon(text), [], `这一句漏了内行词：${text}`)
  }
})
