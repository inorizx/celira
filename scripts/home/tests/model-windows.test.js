'use strict'

const test = require('node:test')
const assert = require('node:assert/strict')
const path = require('node:path')

const { createWindowTable, loadWindowTable } = require('../model-windows')

/** 夹具：照真表的形状手写小份（`家 → { 模型名 → 窗口 }`）。 */
const fixture = createWindowTable({
  deepseek: { 'deepseek-flash': 1000000, 'deepseek-chat': 131072 },
  mistral: { 'mistral-large-latest': 262144 },
})

test('限家 + 逐字命中：拿得到就给值', () => {
  assert.equal(fixture.windowFor('deepseek', 'deepseek-flash'), 1000000)
  assert.equal(fixture.windowFor('mistral', 'mistral-large-latest'), 262144)
})

test('★ 限家隔离：同一个名字拿去别的家查，必须 miss（窗口是「(家, 模型) 对」的属性）', () => {
  assert.equal(fixture.windowFor('deepseek', 'mistral-large-latest'), null)
  assert.equal(fixture.windowFor('mistral', 'deepseek-flash'), null)
})

test('★★ 负判据：模糊匹配必须不生效 —— 只差一点就该 miss，不许「猜一个」', () => {
  // 表里是 `mistral-large-latest`。现实教训：模糊匹配会把它命中成**别的条目**、
  // 给出一个看着正常的错值（32000，真值 256K）⇒ 所以这里必须逐字。
  assert.equal(fixture.windowFor('mistral', 'mistral-large'), null, '少个后缀不许命中')
  assert.equal(fixture.windowFor('mistral', 'large-latest'), null, '多个前缀不许命中')
  assert.equal(fixture.windowFor('deepseek', 'deepseek'), null, '前缀不许命中')
})

test('大小写不折算（逐字就是逐字）', () => {
  assert.equal(fixture.windowFor('deepseek', 'DeepSeek-Flash'), null)
  assert.equal(fixture.windowFor('DeepSeek', 'deepseek-flash'), null)
})

test('原型上的名字不算命中（不许把 toString / constructor 当成模型）', () => {
  assert.equal(fixture.windowFor('deepseek', 'toString'), null)
  assert.equal(fixture.windowFor('deepseek', 'constructor'), null)
  assert.equal(fixture.windowFor('deepseek', 'hasOwnProperty'), null)
  assert.equal(fixture.windowFor('__proto__', 'deepseek-flash'), null)
})

test('空 / 歪输入一律 null（不抛、不回落）', () => {
  assert.equal(fixture.windowFor('', 'deepseek-flash'), null)
  assert.equal(fixture.windowFor(null, 'deepseek-flash'), null)
  assert.equal(fixture.windowFor(undefined, 'deepseek-flash'), null)
  assert.equal(fixture.windowFor('deepseek', ''), null)
  assert.equal(fixture.windowFor('deepseek', '   '), null)
  assert.equal(fixture.windowFor('deepseek', null), null)
  assert.equal(fixture.windowFor('deepseek', 123), null)
  assert.equal(fixture.windowFor('unknown-provider', 'deepseek-flash'), null)
})

test('表读不动 ⇒ 当成空表（退回「让用户填」，而不是把保存流程挂掉）', () => {
  const empty = loadWindowTable(path.join(__dirname, 'no-such-file.json'))
  assert.equal(empty.windowFor('deepseek', 'deepseek-flash'), null)
  assert.deepEqual(empty.providerIds(), [])
})

test('随包的那张真表：格式对、抽查值对、探测拿得到的家确实不在', () => {
  const real = loadWindowTable()
  assert.ok(real.providerIds().length > 0, '真表应当有家')
  assert.ok(real.windowFor('deepseek', 'deepseek-flash') === 1000000)
  assert.ok(real.windowFor('openai', 'gpt-4o') === 128000)
  // `openrouter` 的 `/models` 自带 `context_length` ⇒ 探测拿得到 ⇒ 不该占兜底表的位置
  for (const provider of ['openrouter', 'together', 'moonshot', 'mistral']) {
    assert.equal(real.hasProvider(provider), false, `${provider} 探测本来就拿得到窗口，不该摘进兜底表`)
  }
})
