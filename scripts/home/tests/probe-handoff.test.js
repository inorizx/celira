'use strict'
/**
 * `probe-handoff.js` 的**判据**用例（真机部分不在这里跑 —— 那要真世界、真宿主、真模型调用）。
 *
 * 为什么这几条值得单独钉住（2026-10-19 设计时定下的三条）：
 *
 * 1. **主判据只数"第二段之后"她说的**：第一条任务里**就写着那个码** ⇒ 不从那一刻起数，
 *    "她报对了"会有一条假的路（照抄任务）；
 * 2. **"报对了"必须配"她没绕路"**：那一份就住在她的环境根里、她有 `terminal` ⇒ 只有"没绕路"
 *    时，"报对了"才只可能来自开场那一块；
 * 3. **`run-12` 不排在 `run-4` 前面**：段的先后按盘上的顺序，不按 id 排（与运行时同一条判据）。
 */

const test = require('node:test')
const assert = require('node:assert/strict')

const {
  parseArguments,
  makeMark,
  writeTask,
  REPORT_TASK,
  WRITE_ACTION,
  markReported,
  traceRows,
  actionsIn,
  openingInjections,
  runsIn,
  wroteHandoff,
  readByDetour,
  openingShowed,
  handoffVerdict,
  describeMessage,
} = require('../probe-handoff')

let cases = 0
function check(name, fn) {
  test(name, () => {
    cases += 1
    fn()
  })
}

const AGENT = 'aaaaaaaa-1111-2222-3333-444444444444'
const HUMAN = 'hhhhhhhh-1111-2222-3333-444444444444'
const MARK = 'HO-ABCDEFGH'

/** 世界回的那条消息**真实形状**（`MessageView`：`actor: {id, kind, display_name, status}`）。 */
function message(actorId, kind, body) {
  return {
    id: `m-${Math.random().toString(16).slice(2)}`,
    room_id: 'r-1',
    actor: { id: actorId, kind, display_name: 'x', status: 'active' },
    body,
    created_at: '2026-10-19T12:00:00.000Z',
  }
}

/** 经历账本的一行（真实形状：`{seq, run, kind, …}` ＋ 行尾 `\t<校验和>`）。 */
function row(value) {
  return `${JSON.stringify(value)}\t0123abcd`
}

check('参数：缺省 300 秒、不保留；--timeout / --keep 认；不认识的一律明确失败', () => {
  const base = parseArguments([])
  assert.equal(base.timeoutSeconds, 300)
  assert.equal(base.keep, false)
  assert.deepEqual(parseArguments(['--timeout', '60', '--keep']), { timeoutSeconds: 60, keep: true })
  for (const argv of [['--timeout'], ['--timeout', '0'], ['--timeout', '-1'], ['--timeout', 'x']]) {
    assert.throws(() => parseArguments(argv), /--timeout/, `${JSON.stringify(argv)} 必须报用法错`)
  }
  assert.throws(() => parseArguments(['--absent']), /不认识的参数/)
})

check('码不可猜、形状固定：前缀 `HO-` ＋ 8 位（不含 0/O/1/I 这类看不清的对）', () => {
  const mark = makeMark()
  assert.match(mark, /^HO-[ABCDEFGHJKLMNPQRSTUVWXYZ23456789]{8}$/)
  assert.notEqual(makeMark(), makeMark(), '两次生成不许撞（它是判据的全部）')
  // 固定随机源 ⇒ 可判错；末位不同的两份码不许相同。
  assert.equal(makeMark(() => Buffer.from([0, 1, 2, 3, 4, 5, 6, 7])), makeMark(() => Buffer.from([0, 1, 2, 3, 4, 5, 6, 7])))
})

check('第一段那条任务：码**逐字**进正文（她照抄它，才谈得上交付）', () => {
  const task = writeTask(MARK)
  assert.ok(task.includes(MARK), '码必须在任务正文里')
  assert.ok(!task.includes(WRITE_ACTION), '任务里**不许提动词名**——找不找得到它正是要量的事')
  assert.ok(!task.includes('handoff'), '也不许提那一格的英文名')
})

check('★ 主判据只数「第二段之后」她说的：第一条任务里就有那个码，照抄不算', () => {
  const messages = [
    message(HUMAN, 'human', writeTask(MARK)),
    message(AGENT, 'agent', `好，我写下了：${MARK}`), // 第一段的回话：**照抄任务** ⇒ 不算
    message(HUMAN, 'human', REPORT_TASK),
    message(AGENT, 'agent', '我开场看到的是空的，没有那一份。'), // 第二段：没报 ⇒ 不算
  ]
  assert.equal(markReported(messages, AGENT, MARK, 0), true, '从头数 ⇒ 会被照抄骗过去')
  assert.equal(markReported(messages, AGENT, MARK, 2), false, '从第二条任务起数 ⇒ 她没报')
  const good = [...messages.slice(0, 3), message(AGENT, 'agent', `是 ${MARK}`)]
  assert.equal(markReported(good, AGENT, MARK, 2), true, '第二段报对了才算')
})

check('经历账本：读不懂的行跳过、行尾校验和不进正文、缺字段不炸', () => {
  const text = [
    '',
    '不是 JSON',
    row({ seq: 1, run: 'run-1', kind: 'run-start' }),
    '   ',
    row({ seq: 2, run: 'run-1', kind: 'injection', stage: 'run-start', blocks: [{ label: 'handoff', text: `写下：${MARK}` }] }),
  ].join('\n')
  assert.equal(traceRows(text).length, 2)
  assert.equal(traceRows(null).length, 0)
  assert.equal(traceRows('{"a":1}').length, 1, '没有校验和那一截也读得出来')
})

check('★ 段的先后按盘上的顺序，**不按 id 排**（`run-12` 不许排到 `run-4` 前面）', () => {
  const text = [
    row({ seq: 1, run: 'run-4', kind: 'run-start' }),
    row({ seq: 9, run: 'run-12', kind: 'run-start' }),
  ].join('\n')
  assert.deepEqual(runsIn(text), ['run-4', 'run-12'])
})

check('辅助判据：她写过交接没有 / 她绕没绕路 —— 都**按段**算', () => {
  const text = [
    row({ seq: 1, run: 'run-1', kind: 'action-selected', action: 'write_handoff' }),
    row({ seq: 2, run: 'run-1', kind: 'action-selected', action: 'say' }),
    row({ seq: 3, run: 'run-2', kind: 'action-selected', action: 'terminal' }),
  ].join('\n')
  const actions = actionsIn(text)
  assert.equal(actions.length, 3)
  assert.equal(wroteHandoff(actions, 'run-1'), true)
  assert.equal(wroteHandoff(actions, 'run-2'), false, '第二段没写 ⇒ 不算')
  assert.equal(readByDetour(actions, 'run-2'), true)
  assert.equal(readByDetour(actions, 'run-1'), false)
  // `files_*` 那一族也算绕路（她可能用 `files_read` 去读那一份）。
  const files = actionsIn(row({ seq: 1, run: 'run-2', kind: 'action-selected', action: 'files_read' }))
  assert.equal(readByDetour(files, 'run-2'), true)
})

check('开场那一块：标签对得上**且正文里带着那个码**才算摆过', () => {
  const text = [
    row({
      seq: 1,
      run: 'run-1',
      kind: 'injection',
      stage: 'run-start',
      blocks: [{ label: 'status-bar', text: '时间 …' }],
    }),
    row({
      seq: 2,
      run: 'run-2',
      kind: 'injection',
      stage: 'run-start',
      blocks: [
        { label: 'status-bar', text: '时间 …' },
        { label: 'handoff', text: `上一次留下：${MARK}` },
      ],
    }),
    // 段内那次 turn 级注入**不算开场**（判据只认 `stage = run-start`）。
    row({
      seq: 3,
      run: 'run-2',
      kind: 'injection',
      stage: 'turn',
      blocks: [{ label: 'handoff', text: MARK }],
    }),
  ].join('\n')
  const injections = openingInjections(text)
  assert.equal(injections.length, 2, 'turn 级那条不算开场注入')
  assert.equal(openingShowed(injections, 'run-2', MARK), true)
  assert.equal(openingShowed(injections, 'run-1', MARK), false)
  assert.equal(openingShowed(injections, 'run-2', 'HO-ZZZZZZZZ'), false, '码不对 ⇒ 不是那一份')
})

check('★ 判决：六档各自独立，不许互相顶替', () => {
  const base = { runs: ['run-1', 'run-2'], mark: MARK, traceReadable: true }
  const wrote = { run: 'run-1', action: WRITE_ACTION }
  const detour = { run: 'run-2', action: 'terminal' }
  const shown = [{ run: 'run-2', blocks: [{ label: 'handoff', text: `上一次：${MARK}` }] }]
  const empty = [{ run: 'run-2', blocks: [{ label: 'status-bar', text: '时间 …' }] }]

  assert.equal(
    handoffVerdict({ ...base, actions: [wrote], injections: shown, reported: true }).kind,
    'delivered',
  )
  assert.equal(
    handoffVerdict({ ...base, actions: [], injections: shown, reported: false }).kind,
    'never-wrote',
  )
  assert.equal(
    handoffVerdict({ ...base, actions: [wrote], injections: shown, reported: false }).kind,
    'not-reported',
  )
  assert.equal(
    handoffVerdict({ ...base, actions: [wrote, detour], injections: shown, reported: true }).kind,
    'by-detour',
    '她自己读了那一份 ⇒ 交付不能算',
  )
  assert.equal(
    handoffVerdict({ ...base, actions: [wrote], injections: empty, reported: true }).kind,
    'contradiction',
    '报对了而开场没摆 ⇒ 打架，如实报',
  )
  // 只有一段 ⇒ 前提不成立（不适用），**不是**"没写"。
  assert.equal(
    handoffVerdict({ ...base, runs: ['run-1'], actions: [wrote], injections: [], reported: false }).kind,
    'no-second-activity',
  )
  // 经历读不到 ⇒ `shown` 是 `null`（不知道）⇒ 按黑盒判，不许当成"没摆"。
  assert.equal(
    handoffVerdict({
      ...base,
      actions: [],
      injections: [],
      reported: true,
      traceReadable: false,
    }).kind,
    'never-wrote',
    '读不到经历时仍按"没写"分类（那一档本来就靠经历）',
  )
})

check('读数那一行：谁 / 什么时候 / 说了什么（正文压成一行、有长度上限；缺字段不炸）', () => {
  const line = describeMessage(message(AGENT, 'agent', '第一行\n第二行'))
  assert.match(line, /2026-10-19T12:00:00\.000Z/)
  assert.match(line, /\[agent:aaaaaaaa\]/)
  assert.match(line, /第一行 第二行/)
  assert.ok(!describeMessage({}).includes('undefined'))
  assert.equal(typeof describeMessage(null), 'string')
  assert.ok(describeMessage(message(AGENT, 'agent', 'x'.repeat(400))).length < 240)
})

test('用例数 > 0（空跑守卫）', () => {
  assert.ok(cases > 0, '这一片一条都没跑 —— 那是空跑，不是通过')
})
