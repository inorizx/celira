// ★ **见证那一格的人话**（2026-10-18）——「这两份配置最后是谁改的」。
//
// 这一件要钉住的是**判据**，不是措辞：
//  1. **两档分开，不合并**：页面改的**有时刻**；不是页面改的，**只知"变了"，不知是谁**
//     （那一句里必须**说不出**是谁 —— 编一个"可能是她"就是把读数说成推断）；
//  2. **判不了 ≠ 没改过**：`no-witness` / `unreadable` 两档都要把"判不了"说出口；
//  3. **门没给这一格**（老门）⇒ 如实说"读不到"，**不许**说成"没改过"；
//  4. **时刻读不出来时不许兜底成"现在"**（那会把一个缺失的事实说成一个假的事实）。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { modelWitnessChainLine, modelWitnessLine, type ModelWitnessReading } from '../src/api/homeConfig.ts'

/** 造一份读数（缺省：门改过、有时刻）。 */
function reading(over: Partial<ModelWitnessReading> = {}): ModelWitnessReading {
  return { state: 'same', atMs: 1760000000000, detail: null, deltaBytes: null, ...over }
}

const AT = new Date(1760000000000).toLocaleString()

test('★ 门改过 ⇒ 有时刻；之后没变过就说没变过', () => {
  const line = modelWitnessLine(reading())
  assert.equal(line, `页面在 ${AT} 改过，之后没变过`)
})

test('★★ 门外那只手改的 ⇒ 说得出"有人动过"，而**说不出是谁**', () => {
  const line = modelWitnessLine(reading({ state: 'changed', detail: 'content', deltaBytes: 12 }))
  assert.ok(line.includes('页面之后有人动过它'), line)
  assert.ok(line.includes(`页面最后写于 ${AT}`), '知道的是**门最后写于何时**')
  assert.ok(line.includes('+12 字节'), line)
  assert.ok(line.includes('是谁、什么时候，这条路上答不出来'), line)
  // 少一个字节也如实带符号（不许把"少"说成"多"）。
  assert.ok(modelWitnessLine(reading({ state: 'changed', detail: 'content', deltaBytes: -3 })).includes('-3 字节'))
  // 字节差算不出来 ⇒ 那一截**整个不说**（不编一个 0）。
  const noDelta = modelWitnessLine(reading({ state: 'changed', detail: 'content', deltaBytes: null }))
  assert.ok(!noDelta.includes('字节'), noDelta)
})

test('★ 不见了 / 又出现了：两档都说得出，且都不猜是谁', () => {
  const missing = modelWitnessLine(reading({ state: 'changed', detail: 'missing' }))
  assert.ok(missing.includes('不在了') && missing.includes(AT), missing)
  assert.ok(missing.includes('答不出来'), missing)
  const appeared = modelWitnessLine(reading({ state: 'changed', detail: 'appeared' }))
  assert.ok(appeared.includes('又出现了') && appeared.includes('摘掉过它'), appeared)
})

test('★ 判不了的三档各自说自己的处境（**都不是**"没改过"）', () => {
  assert.equal(modelWitnessLine(reading({ state: 'no-witness', atMs: null })), '没有见证 ⇒ 判不了（不是"没改过"）')
  assert.equal(
    modelWitnessLine(reading({ state: 'unreadable', atMs: null, detail: 'file' })),
    '这一份读不出来 ⇒ 判不了（不是"没改过"）',
  )
  assert.equal(
    modelWitnessLine(reading({ state: 'unreadable', atMs: null, detail: 'ledger' })),
    '见证账本读不出来 ⇒ 判不了（不是"没改过"）',
  )
})

test('★ 门没给这一格（老门）⇒ 说"读不到"，不许说成"没改过"', () => {
  for (const value of [null, undefined]) {
    const line = modelWitnessLine(value)
    assert.ok(line.includes('旧版'), line)
    assert.ok(!line.includes('没改过'), '不许把"读不到"说成"没改过"')
  }
})

test('★ 时刻读不出来 ⇒ 如实说读不出来，**绝不**兜底成"现在"', () => {
  const line = modelWitnessLine(reading({ atMs: null }))
  assert.ok(line.includes('一个读不出来的时刻'), line)
  assert.ok(!line.includes(AT), '不许拿另一个时刻顶上来')
})

test('★ 账本链条断了 ⇒ 那一句（`null` = 没断 / 老门不给 ⇒ 不说）', () => {
  assert.equal(modelWitnessChainLine(null), null)
  assert.equal(modelWitnessChainLine(undefined), null)
  const line = modelWitnessChainLine(2)
  assert.ok(line !== null && line.includes('第 3 条'), String(line))
  assert.ok(line !== null && line.includes('不是封印'), '边界要跟着这一句一起说出口')
})
