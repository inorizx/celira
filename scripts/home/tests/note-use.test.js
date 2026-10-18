'use strict'
// `probe-note-use`（判据 3 的真机读数）：**她写下的字，后来被自己读过吗**。
//
// 这一族要钉住的是**判据**，不是"能数数"：
//  1. **年龄从账里折，不从盘上猜** —— 笔记文件只有 `{pinned,text,title}`，没有时间字段；
//     文件 mtime 是"最后动过"（`notes_edit` / `notes_append` 会改写）⇒ **mtime 一变都不许影响读数**；
//  2. **"用过"的判据是"此后被点名读过"** —— 创建**之前**的读不算（那是别的条），
//     创建**之后**的读才算；
//  3. **口径要说准**：读数说的是「从未被 `notes_read` 点名读过」，不是"从未出现在她视野里"；
//  4. **不适用与判不了分开**：没有住户 / 没有笔记 / 账本还没建 ⇒ 退出 **2**；
//     账本在场但一行都读不出来 ⇒ 退出 **3**（既不算通过也不算失败那条线不许糊）；
//  5. **不打印正文**：只有身份、时刻与计数。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const probe = require('../probe-note-use')

let cases = 0
function nCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

/** 一行账本记录：`{json}\t<链值>`（真机形状 —— 链值不是 JSON 的一部分）。 */
function line(rec) {
  return `${JSON.stringify(rec)}\tdeadbeef\n`
}

const created = (seq, run, turn, at, noteId) =>
  line({ kind: 'observed', seq, run, turn, 'at-ms': at, action: 'notes_create', blocks: [{ label: 'created', text: noteId }] })
const createCall = (seq, run, turn, at) =>
  line({ kind: 'action-selected', seq, run, turn, 'at-ms': at, action: 'notes_create', params: [] })
const readCall = (seq, run, turn, at, noteId) =>
  line({
    kind: 'action-selected',
    seq,
    run,
    turn,
    'at-ms': at,
    action: 'notes_read',
    params: [{ label: 'note', text: noteId }],
  })

/** 造一个临时家：一支住户，带 notes/ 与一本合成账本。 */
function makeHome({ notes = ['note-1', 'note-2'], ledger = null } = {}) {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), `celira-noteuse-${process.pid}-`))
  const envDir = path.join(home, 'agents', 'agent-a')
  fs.mkdirSync(path.join(envDir, 'notes'), { recursive: true })
  fs.writeFileSync(
    path.join(envDir, 'agent.conf'),
    'agent_id=agent-a\nslot=1\ndisplay_name=测试住户\n'
  )
  for (const id of notes) {
    fs.writeFileSync(path.join(envDir, 'notes', `${id}.note`), JSON.stringify({ pinned: false, text: 'x', title: 't' }))
  }
  if (ledger !== null) fs.writeFileSync(path.join(envDir, 'trace.ndjson'), ledger)
  return { home, envDir }
}
function drop(home) {
  fs.rmSync(home, { recursive: true, force: true })
}

/** 跑 main 并把 stdout/stderr 收起来（用例判的是退出码与折出来的事实，不是打印格式）。 */
function runMain(argv) {
  const out = []
  const err = []
  const log = console.log
  const error = console.error
  console.log = (...a) => out.push(a.join(' '))
  console.error = (...a) => err.push(a.join(' '))
  try {
    return { code: probe.main(argv), out: out.join('\n'), err: err.join('\n') }
  } finally {
    console.log = log
    console.error = error
  }
}

nCase('★ 年龄从账里折：创建时刻来自 notes_create 的事实，不是文件 mtime', () => {
  const { home, envDir } = makeHome({
    ledger:
      createCall(1, 'r1', 1, 1000) +
      created(2, 'r1', 1, 1000, 'note-1') +
      readCall(3, 'r1', 2, 2000, 'note-1') +
      createCall(4, 'r2', 1, 3000) +
      created(5, 'r2', 1, 3000, 'note-2'),
  })
  try {
    const { rows } = probe.readLedger(envDir)
    const { born } = probe.fold(rows)
    assert.equal(born.get('note-1'), 1000)
    assert.equal(born.get('note-2'), 3000)
  } finally {
    drop(home)
  }
})

nCase('★ mtime 一变都不许影响读数（这条就是"不许拿 mtime 冒充年龄"）', () => {
  const { home, envDir } = makeHome({
    ledger:
      createCall(1, 'r1', 1, 1000) +
      created(2, 'r1', 1, 1000, 'note-1') +
      readCall(3, 'r1', 2, 2000, 'note-1') +
      createCall(4, 'r2', 1, 3000) +
      created(5, 'r2', 1, 3000, 'note-2'),
  })
  try {
    const before = probe.fold(probe.readLedger(envDir).rows)
    // 把 note-2 摸成"刚刚动过"（模拟 notes_edit / notes_append 改写文件）
    const future = new Date(Date.now() + 86400000)
    fs.utimesSync(path.join(envDir, 'notes', 'note-2.note'), future, future)
    const after = probe.fold(probe.readLedger(envDir).rows)
    assert.deepEqual([...after.born.entries()], [...before.born.entries()], '创建时刻不受 mtime 影响')
    assert.equal(after.readsAfter.get('note-2'), undefined, 'note-2 仍然从未被点名读过')
  } finally {
    drop(home)
  }
})

nCase('★ "用过"的判据：创建**之后**的读才算，创建**之前**的不算', () => {
  const { home, envDir } = makeHome({
    notes: ['note-1'],
    ledger:
      readCall(1, 'r0', 1, 500, 'note-1') + // 这条还没写出来时的读 —— 不算
      createCall(2, 'r1', 1, 1000) +
      created(3, 'r1', 1, 1000, 'note-1'),
  })
  try {
    const { born, readsAfter } = probe.fold(probe.readLedger(envDir).rows)
    assert.equal(born.get('note-1'), 1000)
    assert.equal(readsAfter.get('note-1'), undefined, '创建之前的那次读不许算进来')
  } finally {
    drop(home)
  }
})

nCase('★ 真机形状的整条读数：一条用过、一条从未用过 ⇒ 1 / 2 = 50%', () => {
  const { home } = makeHome({
    ledger:
      createCall(1, 'r1', 1, 1000) +
      created(2, 'r1', 1, 1000, 'note-1') +
      readCall(3, 'r1', 2, 2000, 'note-1') +
      createCall(4, 'r2', 1, 3000) +
      created(5, 'r2', 1, 3000, 'note-2'),
  })
  try {
    const r = runMain(['--root', home])
    assert.equal(r.code, 0)
    assert.match(r.out, /1 \/ 2 = 50%/)
    assert.match(r.out, /从未被点名：note-2/)
    assert.match(r.out, /不判：读数说的是「从未被 notes_read 点名读过」/, '口径必须打出来')
  } finally {
    drop(home)
  }
})

nCase('★ 不适用 ⇒ 退出 2：一支笔记都没有 / 账本还没建', () => {
  const empty = makeHome({ notes: [] })
  try {
    assert.equal(runMain(['--root', empty.home]).code, 2, 'notes/ 空 ⇒ 不适用')
  } finally {
    drop(empty.home)
  }
  const noLedger = makeHome({ ledger: null })
  try {
    assert.equal(runMain(['--root', noLedger.home]).code, 2, '账本还没建 ⇒ 不适用')
  } finally {
    drop(noLedger.home)
  }
  const noResident = fs.mkdtempSync(path.join(os.tmpdir(), `celira-noteuse-${process.pid}-`))
  try {
    assert.equal(runMain(['--root', noResident]).code, 2, '一支住户都没有 ⇒ 不适用')
  } finally {
    drop(noResident)
  }
})

nCase('★ 判不了 ⇒ 退出 3：账本在场但一行都读不出来（不许与"不适用"糊在一起）', () => {
  const { home } = makeHome({ ledger: '这不是 JSON\n也不是\n' })
  try {
    const r = runMain(['--root', home])
    assert.equal(r.code, 3)
    assert.match(r.out, /一行都读不出来/)
  } finally {
    drop(home)
  }
})

nCase('★ 读不懂的行要如实报数，不许静默跳过', () => {
  const { home, envDir } = makeHome({
    ledger:
      createCall(1, 'r1', 1, 1000) +
      created(2, 'r1', 1, 1000, 'note-1') +
      'garbage\n' +
      readCall(3, 'r1', 2, 2000, 'note-1'),
  })
  try {
    const { rows, unparsable } = probe.readLedger(envDir)
    assert.equal(unparsable, 1)
    assert.equal(rows.length, 3)
  } finally {
    drop(home)
  }
})

nCase('★ 用法错 ⇒ 退出 1（认不得的选项 / 重复给同一个选项）', () => {
  assert.equal(runMain(['--nope']).code, 1)
  assert.equal(runMain(['--root', 'a', '--root', 'b']).code, 1)
  assert.equal(runMain(['--root']).code, 1)
})

test('★ 这一套用例真的跑了（空跑不算通过）', () => {
  assert.ok(cases >= 8, `跑到的用例数 ${cases} —— 少于 8 说明有整块没执行`)
})
