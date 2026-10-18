// 「日子」的纯逻辑测试（刀2a）。
//
// 这一层只做一件事：把状态口那份机械读数按**一次清醒（Run）**分组。判据里最要紧的三条：
//  1. **一件事只有一个来源**：怎么收场读**那一场自己**的 `ending` 行 —— 实测 `agent.runs`
//     与 experience 窗口可以**完全不重叠**，拿 runs 去对会把每一场都说成"收场未知"，
//     更糟的是拿**别的场**的收场顶上去；
//  2. **缺什么就缺省**（`null`），绝不拿别的字段顶。`notification=` 不是"为什么醒"，
//     所以时光线上根本没有"起因"这一列 —— 而不是拿它冒充；
//  3. **归不到哪一场的行就丢开**，不编一个"未知场"（那会把缺省说成事实）。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { agentDays, wakeAtFromRunId } from '../src/lib/agent-days.ts'
import type { HomeStatusRun, HomeStatusView } from '../src/api/homeStatus.ts'

/** 一份最小读数（只填这一层要用的字段）。 */
function view(rows: Record<string, unknown>[], runs: HomeStatusRun[] = []): HomeStatusView {
  return {
    readAt: '2026-09-22T15:00:00.000Z',
    stale: false,
    im: { health: 200 },
    agent: { state: 'loaded', admission: 'ready', runs, runsTotal: runs.length },
    lastWake: null,
    experience: { state: 'loaded', rows: rows as never, truncated: true, windowed: false },
  }
}

test('醒来的时刻是**从活动标识里解出来的**（解不出就是 null，不拿别的时刻顶）', () => {
  assert.equal(wakeAtFromRunId('host-17016-1790089509863-1'), 1790089509863)
  for (const bad of ['', 'run-1', 'host-abc-1-1', 'host-1-0-1', 'wake run=host-1-2-3 drives=1']) {
    assert.equal(wakeAtFromRunId(bad), null, bad)
  }
})

test('按活动标识分组：两场 ⇒ 两行，**最近的一场在前**（按最后一次出现的顺序号）', () => {
  const got = agentDays(
    view([
      { seq: 10, run: 'host-1-1000-1', kind: 'run-start' },
      { seq: 11, run: 'host-1-1000-1', kind: 'sealed' },
      { seq: 20, run: 'host-1-2000-2', kind: 'run-start' },
      { seq: 21, run: 'host-1-2000-2', kind: 'sealed' },
    ]),
  )
  assert.equal(got.state, 'loaded')
  assert.deepEqual(
    got.days.map((day) => day.run),
    ['host-1-2000-2', 'host-1-1000-1'],
  )
  assert.equal(got.truncated, true, '窗口内有更多要如实带出来')
  assert.equal(got.windowed, false)
})

test('做了几件事：只数 Agent**选择去做**的动作（action-selected），去重、按发生顺序', () => {
  const [day] = agentDays(
    view([
      { seq: 1, run: 'host-1-1000-1', turn: 1, kind: 'model-response', action: 'im_say', decision: 'act' },
      { seq: 2, run: 'host-1-1000-1', turn: 1, kind: 'action-selected', action: 'im_say' },
      { seq: 3, run: 'host-1-1000-1', turn: 1, kind: 'observed', action: 'im_say', state: 'success' },
      { seq: 4, run: 'host-1-1000-1', turn: 2, kind: 'action-selected', action: 'notes_pin' },
      { seq: 5, run: 'host-1-1000-1', turn: 2, kind: 'action-selected', action: 'im_say' },
      { seq: 6, run: 'host-1-1000-1', turn: 3, kind: 'action-selected', action: 'notes_pin' },
    ]),
  ).days
  assert.deepEqual(day?.actions, ['im_say', 'notes_pin'], '重复的动作只算一件，顺序照发生')
  assert.equal(day?.turns, 3, '走了几拍 = 最大 turn')
})

test('怎么收场：读**那一场自己**的 ending 行（含失败分类）', () => {
  const [day] = agentDays(
    view([
      { seq: 1, run: 'host-1-1000-1', kind: 'run-start' },
      { seq: 2, run: 'host-1-1000-1', kind: 'ending', subject: 'completed', failure: 'model-failed' },
      { seq: 3, run: 'host-1-1000-1', kind: 'sealed' },
    ]),
  ).days
  assert.equal(day?.ending, 'completed')
  assert.equal(day?.failure, 'model-failed')
  assert.equal(day?.sealed, true)
})

test('★ 兜底只在**这一场没有 ending 行**时用 agent.runs；认不出来就缺省，绝不拿别的场顶', () => {  // 实测形状：窗口里的行属于 run A，而 agent.runs 只列着 run B（两者可以不重叠）。
  const got = agentDays(
    view(
      [
        { seq: 1, run: 'host-1-1000-1', kind: 'run-start' },
        { seq: 2, run: 'host-1-1000-1', kind: 'action-selected', action: 'im_say' },
      ],
      [{ id: 'host-9-9999-9', state: 'ended', outcome: 'subject:silent', failure: null }],
    ),
  )
  assert.equal(got.days.length, 1)
  assert.equal(
    got.days[0]?.ending,
    null,
    '认不出这一场 ⇒ 必须缺省（页面照实说"这一场的收场不在本次读数窗口里"），不许拿 B 的收场冒充',
  )
  assert.equal(got.days[0]?.failure, null)

  // 正对照：runs 里**有**这一条（没有 ending 行时才有资格兜底）。
  const withRun = agentDays(
    view([{ seq: 1, run: 'host-1-1000-1', kind: 'run-start' }], [
      { id: 'host-1-1000-1', state: 'ended', outcome: 'subject:silent', failure: null },
    ]),
  )
  assert.equal(withRun.days[0]?.ending, 'subject:silent')
})

test('缺省纪律：没有活动标识的行**丢开**，不编一个"未知场"', () => {
  const got = agentDays(
    view([
      { seq: 1, kind: 'noise' },
      { seq: 2, run: '', kind: 'noise' },
      { seq: 3, run: 'host-1-1000-1', kind: 'sealed' },
    ]),
  )
  assert.deepEqual(
    got.days.map((day) => day.run),
    ['host-1-1000-1'],
    '归不到哪一场的行不该凭空长出一场',
  )
})

test('这一面读不到 ⇒ missing（**不是**"Agent 没有日子"），一行都不编', () => {
  const missing = view([])
  delete (missing as { experience?: unknown }).experience
  assert.deepEqual(agentDays(missing), {
    state: 'missing',
    days: [],
    truncated: false,
    windowed: false,
  })
  const unreadable = view([])
  unreadable.experience = { state: 'unreadable' }
  assert.equal(agentDays(unreadable).state, 'missing')
})

// ⚠ 这条判据在刀2b C② **翻了一次面**（如实记下）：刀2a 时「起因不在读数里」，所以那时断言的是
// `!('origin' in day)`；刀2b 起口子把起源**映射成稳定取值**送出来了 ⇒ 现在它有、但**只认那一格**。
test('起源：只从 `run-start` 那一行取（口子映射出来的稳定取值），没有就是 null', () => {
  const withOrigin = agentDays(
    view([
      { seq: 1, run: 'host-1-1000-1', kind: 'injection', stage: 'run-start', origin: 'bell' },
      { seq: 2, run: 'host-1-1000-1', kind: 'sealed' },
    ]),
  ).days[0]
  assert.equal(withOrigin?.origin, 'bell')
  // 认不出的取值也**照原样带过去**（页面负责说"不知道"；这一层不替它编）。
  const unknown = agentDays(
    view([{ seq: 1, run: 'host-1-1000-1', kind: 'injection', stage: 'run-start', origin: 'unknown' }]),
  ).days[0]
  assert.equal(unknown?.origin, 'unknown')
  // 没有那一行 ⇒ null（**不是**编一个）。
  const none = agentDays(view([{ seq: 1, run: 'host-1-1000-1', kind: 'run-start' }])).days[0]
  assert.equal(none?.origin, null)
})

test('★ 仍然**不拿 `notification=` 当起因**（那条老纪律还在）', () => {
  // 它的真义是"这一拍通知快照读没读到"，与"谁把 Agent 叫醒"是两件事：就算那一行在旁边，也不许采它。
  const [day] = agentDays(
    view([
      { seq: 1, run: 'host-1-1000-1', kind: 'injection', stage: 'turn', notification: 'read' },
      { seq: 2, run: 'host-1-1000-1', kind: 'sealed' },
    ]),
  ).days
  assert.equal(day?.origin, null, '`notification` 不是起源，采它就等于编')
  assert.ok(!('notification' in (day as object)), '这一层也不带那一格')
})

// ---------------------------------------------------------------------------
// 只有收场那一层（2026-10-05）—— app 形态那条路（壳不端 `experience`）
// ---------------------------------------------------------------------------
//
// 判据一句话：**明细"没给" ≠ "没有动手"**。这条路一格明细都不编
// （`actions` / `turns` / `origin` / `sealed` 全部缺省），只说那一层**本来就有**的
// （`ending` / `failure` / `state`），并如实标出来源（`detail: 'runs-only'`）。

/** 一份"只有收场"的读数：`experience` 不给（或由调用方指定），`agent.runs` 在。 */
function runsOnlyView(
  runs: HomeStatusRun[],
  { experience, truncated }: { experience?: unknown; truncated?: boolean } = {},
): HomeStatusView {
  const base = view([], runs)
  base.agent = {
    state: 'loaded',
    runs,
    runsTotal: runs.length,
    truncated,
  } as HomeStatusView['agent']
  if (experience === undefined) delete (base as { experience?: unknown }).experience
  else base.experience = experience as HomeStatusView['experience']
  return base
}

test('★ 只有收场那一层 ⇒ runs-only：每一场都在，明细一格不编（"没给"不是"没有"）', () => {
  const got = agentDays(
    runsOnlyView([
      { id: 'host-1-1000-1', state: 'ended', outcome: 'subject:completed' },
      { id: 'host-1-2000-2', state: 'unfinished', outcome: null },
    ]),
  )
  assert.equal(got.state, 'runs-only')
  assert.deepEqual(
    got.days.map((day) => day.run),
    ['host-1-2000-2', 'host-1-1000-1'],
    '最近的一场在前',
  )
  const [newest, older] = got.days
  assert.equal(newest?.detail, 'runs-only', '页面靠它选句子')
  assert.equal(newest?.runState, 'unfinished', '收场了没有：那一层本来就有')
  assert.equal(newest?.ending, null, '未收尾的活动没有结果 —— 不编')
  assert.equal(newest?.wakeAtMs, 2000, '时刻仍然从活动标识里解出来')
  assert.deepEqual(newest?.actions, [], '做了几件事：这一层没有 ⇒ 空，不是"零件事"')
  assert.equal(newest?.turns, null)
  assert.equal(newest?.origin, null)
  assert.equal(newest?.sealed, false, '封存行只有明细那一层说得出来')
  assert.deepEqual(newest?.rows, [])
  assert.equal(older?.ending, 'subject:completed')
  assert.equal(older?.runState, 'ended')
})

test('★ runs-only 与"读不到"分得开：收场那一层也没有 ⇒ 才是 missing', () => {
  assert.equal(agentDays(runsOnlyView([])).state, 'missing')
  // 口子**明说** `experience: missing`（不是没给）时也一样：收场那一层在就走 runs-only。
  const stated = runsOnlyView([{ id: 'host-1-1000-1', state: 'ended', outcome: null }], {
    experience: { state: 'missing' },
  })
  assert.equal(agentDays(stated).state, 'runs-only')
})

test('runs-only：口子说"还有更多"就如实带出来；`windowed` 那条路上没有意义', () => {
  const got = agentDays(
    runsOnlyView([{ id: 'host-1-1000-1', state: 'ended', outcome: null }], { truncated: true }),
  )
  assert.equal(got.truncated, true)
  assert.equal(got.windowed, false)
})

test('runs-only：有解不出时刻的场 ⇒ **不按时刻排**（不拿缺省当零，一场都不丢）', () => {
  const got = agentDays(
    runsOnlyView([
      { id: 'host-1-1000-1', state: 'ended', outcome: null },
      { id: 'not-a-run-id', state: 'ended', outcome: null },
      { id: 'host-1-3000-3', state: 'ended', outcome: null },
    ]),
  )
  assert.deepEqual(
    got.days.map((day) => day.run),
    ['host-1-3000-3', 'not-a-run-id', 'host-1-1000-1'],
    '口子给的是旧的在前 ⇒ 倒过来；解不出时刻的不参与排序',
  )
  assert.equal(got.days[1]?.wakeAtMs, null, '解不出就是 null，不拿别的时刻顶')
})

// ★ 2026-10-05：**判定类目**（五类之一）在**明细那条路**上也要取得到 ——
// 页面靠它把"停在半路 / 被挡住 / **不明（可能已经做了）** / 判定不出来"分开说，
// 而不是把四类压成同一句"这次没有读到结果"。
test('★ 明细那条路也取判定类目：那一场恰好也在 `agent.runs` 窗口里就取，取不到就是 null', () => {
  const rows = [
    { seq: 1, run: 'host-1-1000-1', kind: 'sealed' },
    { seq: 2, run: 'host-1-2000-2', kind: 'run-start' },
  ]
  // ① 两场都在 runs 窗口里 ⇒ 各自的类目取得到（`ending` 那一格也照旧兜底）。
  const both = agentDays(
    view(rows, [
      { id: 'host-1-1000-1', state: 'ended', outcome: 'subject:completed' },
      { id: 'host-1-2000-2', state: 'unknown', outcome: null },
    ]),
  )
  const byRun = Object.fromEntries(both.days.map((day) => [day.run, day]))
  assert.equal(byRun['host-1-1000-1']?.runState, 'ended')
  assert.equal(byRun['host-1-1000-1']?.ending, 'subject:completed')
  assert.equal(byRun['host-1-2000-2']?.runState, 'unknown', '这一类必须取得出来')
  assert.equal(byRun['host-1-2000-2']?.ending, null, '它没有收场结果 —— 不编')

  // ② 两个窗口**完全不重叠**（真机实测过这种）：类目取不到 ⇒ null，不拿别的顶。
  const disjoint = agentDays(view(rows, [{ id: 'host-9-9000-9', state: 'ended', outcome: 'x' }]))
  assert.deepEqual(
    disjoint.days.map((day) => day.runState),
    [null, null],
    '窗口不重叠 ⇒ 判定类目缺席（而不是拿别场的顶）',
  )
  // ③ 连 runs 都没有 ⇒ 同样是 null（缺省不等于"没有这一场"）。
  assert.deepEqual(
    agentDays(view(rows)).days.map((day) => day.runState),
    [null, null],
  )
})
