'use strict'
/**
 * **清单的形状与 id 的单调性**（2026-10-23）。
 *
 * 为什么单独立一条：`ms-<序号>` 是**会进历史绑定**的稳定标识 —— 一支 Agent 的
 * `model_binding.json` 里写的就是它。所以 id **永不复用**：清单被删空之后，下一条也不许
 * 又是 `ms-1`（那会让一份历史绑定**静默指向一条新条目**，比"绑定悬空"更坏——悬空至少会
 * 明确报"模型源已被删除"）。判据落在**盘上的 `next` 计数器**上：它是分配计数器，
 * 不是事实；坏/偏小不许毁掉整份清单，但**任何写者都不许让它倒退**。
 */

const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const modelSources = require('../model-sources')

let cases = 0
function check(name, fn) {
  test(name, () => {
    cases += 1
    fn()
  })
}

function scratch(tag) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `celira-ms-list-${tag}-`))
}

function entry(id, extra = {}) {
  return {
    id,
    name: `名字-${id}`,
    endpoint: 'https://example.invalid/v1/chat/completions',
    api_key: `sk-${id}`,
    model: `model-${id}`,
    context_window: 8192,
    ...extra,
  }
}

/** 盘上那份 JSON 的**原样**（判形状用；不经模块归一）。 */
function raw(home) {
  return JSON.parse(fs.readFileSync(modelSources.listPath(home), 'utf8'))
}

check('★ 清单删空之后：下一个 id **不是** ms-1（计数器记得历史走到哪儿了）', () => {
  const home = scratch('no-reuse')
  modelSources.writeList(home, [entry('ms-1'), entry('ms-2')], 3)
  // 把两条都删掉（第二支不绑任何东西）。
  assert.equal(modelSources.removeEntry(home, [], 'ms-1').ok, true)
  assert.equal(modelSources.removeEntry(home, [], 'ms-2').ok, true)
  const emptied = modelSources.readList(home)
  assert.equal(emptied.state, 'loaded')
  assert.deepEqual(emptied.entries, [])
  assert.equal(emptied.next, 3, '删空之后计数器还在 3 —— 这正是"不回收"的落点')

  // 门那条路：id 由 `nextId` 给（这是 POST 新建时唯一给 id 的地方）。
  const id = modelSources.nextId(emptied)
  assert.equal(id, 'ms-3', '删空之后新增的那条**不许**又是 ms-1')
  assert.equal(modelSources.upsertEntry(home, entry(id)).ok, true)
  assert.deepEqual(
    modelSources.readList(home).entries.map((each) => each.id),
    ['ms-3'],
  )
  assert.equal(modelSources.readList(home).next, 4, '新增一条 ⇒ 计数器前进一格')

  // 再删空、再加一次：继续往前走。
  assert.equal(modelSources.removeEntry(home, [], 'ms-3').ok, true)
  assert.equal(modelSources.nextId(modelSources.readList(home)), 'ms-4')
  fs.rmSync(home, { recursive: true, force: true })
})

check('★ 老数组形状仍可读：next = max(现有 id 序号) + 1（空数组 ⇒ 1）', () => {
  const home = scratch('legacy-array')
  fs.mkdirSync(home, { recursive: true })
  fs.writeFileSync(
    modelSources.listPath(home),
    `${JSON.stringify([entry('ms-1'), entry('ms-3')], null, 2)}\n`,
  )
  const list = modelSources.readList(home)
  assert.equal(list.state, 'loaded')
  assert.deepEqual(list.entries.map((each) => each.id), ['ms-1', 'ms-3'])
  assert.equal(list.next, 4)

  fs.writeFileSync(modelSources.listPath(home), '[]\n')
  const empty = modelSources.readList(home)
  assert.equal(empty.state, 'loaded')
  assert.equal(empty.next, 1, '老形状的空数组没有历史可记 ⇒ 从 1 开始（那条路本来如此）')
  fs.rmSync(home, { recursive: true, force: true })
})

check('★ 新对象形状：读 next；**更新条目不动 next**，新增才 +1', () => {
  const home = scratch('object-shape')
  fs.mkdirSync(home, { recursive: true })
  fs.writeFileSync(
    modelSources.listPath(home),
    `${JSON.stringify({ next: 9, entries: [entry('ms-2')] }, null, 2)}\n`,
  )
  const list = modelSources.readList(home)
  assert.equal(list.state, 'loaded')
  assert.equal(list.next, 9)
  assert.equal(modelSources.nextId(list), 'ms-9')

  // 更新已有条目（id 没变）⇒ 计数器原样（更新不消耗序号）。
  assert.equal(modelSources.upsertEntry(home, entry('ms-2', { name: '改过的名字' })).ok, true)
  assert.equal(raw(home).next, 9, '更新已有条目：next 不许动')
  assert.equal(raw(home).entries[0].name, '改过的名字')

  // 新增 ⇒ 计数器前进一格，落盘仍是新形状。
  assert.equal(modelSources.upsertEntry(home, entry('ms-9')).ok, true)
  assert.equal(raw(home).next, 10)
  assert.deepEqual(
    raw(home).entries.map((each) => each.id),
    ['ms-2', 'ms-9'],
  )
  assert.equal(Array.isArray(raw(home)), false, '写侧只产出新形状')
  fs.rmSync(home, { recursive: true, force: true })
})

check('★ next 偏小 / 坏 / 缺席 ⇒ 取 max+1（**不**把整份清单判成 unreadable）', () => {
  const home = scratch('clamp')
  fs.mkdirSync(home, { recursive: true })
  const write = (body) => fs.writeFileSync(modelSources.listPath(home), `${JSON.stringify(body)}\n`)

  write({ next: 1, entries: [entry('ms-4')] })
  const small = modelSources.readList(home)
  assert.equal(small.state, 'loaded', '坏/偏小的 next 只是分配计数器坏了，不是清单坏了')
  assert.equal(small.next, 5, '偏小 ⇒ max+1')
  assert.equal(modelSources.nextId(small), 'ms-5')

  write({ next: 'seven', entries: [entry('ms-4')] })
  assert.equal(modelSources.readList(home).next, 5, '类型不对 ⇒ 按 max+1')

  write({ entries: [entry('ms-4')] })
  assert.equal(modelSources.readList(home).next, 5, '缺席 ⇒ 按 max+1')

  write({ next: -3, entries: [] })
  const negative = modelSources.readList(home)
  assert.equal(negative.state, 'loaded')
  assert.equal(negative.next, 1, '空清单 + 坏 next ⇒ 至少 1（正整数）')
  fs.rmSync(home, { recursive: true, force: true })
})

check('形状不对 ⇒ fail-closed（只有数组 / {next, entries} 两种形状读得出来）', () => {
  const home = scratch('bad-shape')
  fs.mkdirSync(home, { recursive: true })
  const write = (text) => fs.writeFileSync(modelSources.listPath(home), text)
  for (const text of ['"x"', '{}', '{"entries": 5}', '{"next": 2}', '42']) {
    write(text)
    const list = modelSources.readList(home)
    assert.equal(list.state, 'unreadable', `这种情况应当读不出来：${text}`)
    assert.equal(list.reason, 'not_array', text)
    assert.equal(list.next, 1, 'unreadable 也给一个正整数 next（调用方不许据此分配）')
  }
  fs.rmSync(home, { recursive: true, force: true })
})

check('★ 写者漏带 next ⇒ 计数器也**不许倒退**（盘上的 next 是底线）', () => {
  const home = scratch('no-regress')
  modelSources.writeList(home, [entry('ms-5')], 6)
  // 模拟一个老调用点（探针/脚本）：只给条目、不给 next。
  modelSources.writeList(home, [entry('ms-5', { name: '改过' })])
  assert.equal(raw(home).next, 6, '没给 next ⇒ 守住盘上的 6，不退回 max+1 以外的更低值')
  // 删掉最高那条之后，计数器仍不回落（否则下一条会复用 ms-6 的历史）。
  assert.equal(modelSources.removeEntry(home, [], 'ms-5').ok, true)
  assert.equal(raw(home).next, 6)
  fs.rmSync(home, { recursive: true, force: true })
})

check('★ removeEntry：**第二支**绑着也拒（一个家住 N 支就有 N 份绑定）', () => {
  const home = scratch('multi-resident')
  modelSources.writeList(home, [entry('ms-1'), entry('ms-2')], 3)
  const first = path.join(home, 'agents', 'a-1')
  const second = path.join(home, 'agents', 'b-2')
  modelSources.writeBinding(first, 'ms-1')
  modelSources.writeBinding(second, 'ms-2')

  const blocked = modelSources.removeEntry(home, [first, second], 'ms-2')
  assert.equal(blocked.ok, false)
  assert.equal(blocked.reason, 'bound')
  assert.deepEqual(blocked.boundTo, [second], '要点名是**这一支**绑着它')
  assert.deepEqual(
    modelSources.readList(home).entries.map((each) => each.id),
    ['ms-1', 'ms-2'],
    '被拒的删除一个字节都不动',
  )

  // 第一支也绑着它 ⇒ 两个环境根都报出来。
  modelSources.writeBinding(first, 'ms-2')
  assert.deepEqual(modelSources.removeEntry(home, [first, second], 'ms-2').boundTo, [first, second])

  // 都不绑 ⇒ 删得掉；**单个字符串**的老调用形状仍然认。
  modelSources.clearBinding(first)
  modelSources.clearBinding(second)
  assert.equal(modelSources.removeEntry(home, first, 'ms-1').ok, true)
  assert.deepEqual(
    modelSources.readList(home).entries.map((each) => each.id),
    ['ms-2'],
  )
  fs.rmSync(home, { recursive: true, force: true })
})

check('★ 迁移：写新形状，`ms-1` 保留，`next` 至少 2（已有更高的 next 不许被打回 2）', () => {
  const home = scratch('migrate-shape')
  const envDir = path.join(home, 'env')
  fs.mkdirSync(envDir, { recursive: true })
  fs.writeFileSync(
    path.join(envDir, 'model_source.json'),
    `${JSON.stringify({
      endpoint: 'https://legacy.invalid/v1/chat',
      api_key: 'sk-legacy',
      model: 'legacy-model',
      context_window: 8192,
    })}\n`,
  )
  const got = modelSources.migrateIfNeeded(home, envDir)
  assert.equal(got.action, 'migrated', JSON.stringify(got))
  const document = raw(home)
  assert.equal(Array.isArray(document), false, '迁移也写新形状')
  assert.equal(document.next, 2)
  assert.deepEqual(document.entries.map((each) => each.id), ['ms-1'])
  assert.equal(modelSources.readList(home).next, 2)

  // 第二个家：清单已经有 `next: 7`，迁移只往前不往后。
  const other = scratch('migrate-keep-next')
  const otherEnv = path.join(other, 'env')
  modelSources.writeList(other, [entry('ms-3')], 7)
  fs.mkdirSync(otherEnv, { recursive: true })
  fs.writeFileSync(
    path.join(otherEnv, 'model_source.json'),
    `${JSON.stringify({
      endpoint: 'https://legacy.invalid/v1/chat',
      api_key: 'sk-legacy',
      model: 'legacy-model',
      context_window: 8192,
    })}\n`,
  )
  assert.equal(modelSources.migrateIfNeeded(other, otherEnv).action, 'migrated')
  assert.equal(raw(other).next, 7, '已有的 next 更高 ⇒ 保留它（至少 2 是下限，不是重启）')

  fs.rmSync(home, { recursive: true, force: true })
  fs.rmSync(other, { recursive: true, force: true })
})

check('用例数 > 0（空跑守卫）', () => {
  assert.ok(cases > 0, '这一份必须真的跑过用例')
})
