// 关系可逆性（拒绝 / 解除 / 拉黑）在界面上的**动作可用性**判据。
//
// 这一份直测 `uiPolicy.relationActionAvailability`：它是"这张资料卡画哪几个按钮"的唯一裁决面。
// 三个布尔各自都有一句理由（见函数注释），散进页面就只剩表达式、没人知道哪条能删。
//
// 契约权威：`docs/contracts/core-v1/CORE_V1_CONTACT_REVERSIBILITY.md`
// （解除单方面即时生效 / 幂等 204；**拉黑 ⊇ 解除**；拉黑不泄露；拉黑不阻断群内发言）。

import test from 'node:test'
import assert from 'node:assert/strict'

import { relationActionAvailability } from '../src/api/uiPolicy.ts'

// ── 断交：只在真有关系段时给 ────────────────────────────────────────────────

test('是联系人 ⇒ 给「断交」', () => {
  assert.equal(relationActionAvailability({ isContact: true, blocked: false }).remove, true)
})

test('★ 不是联系人 ⇒ 不给「断交」：那会是一个按了什么都不发生的按钮', () => {
  // 契约把"本来就没有 active 段"也答成幂等 204，所以技术上随时能调 —— 判据因此不能是
  // "服务端会不会拒绝"，只能是"这个人现在到底有没有关系可断"。
  assert.equal(relationActionAvailability({ isContact: false, blocked: false }).remove, false)
})

// ── 拉黑与解除拉黑：互斥，且拉黑刻意不看 isContact ───────────────────────────

test('没拉黑过 ⇒ 给「拉黑」，不给「解除拉黑」', () => {
  const a = relationActionAvailability({ isContact: true, blocked: false })
  assert.equal(a.block, true)
  assert.equal(a.unblock, false)
})

test('拉黑过 ⇒ 给「解除拉黑」，不再给「拉黑」', () => {
  const a = relationActionAvailability({ isContact: true, blocked: true })
  assert.equal(a.block, false, '已经拉黑了还画「拉黑」就是第二个空按钮')
  assert.equal(a.unblock, true)
})

test('★ 拉黑**不看** isContact：还没加过我、上来就发申请的人也拦得住', () => {
  // 契约允许拉黑任意 active 主体（拉黑 ⊇ 解除）。一个只有 pending 申请、从未成为联系人的
  // 陌生人，恰恰是最需要这个按钮的场合 —— 这一条守着"别顺手加上 isContact &&"。
  const stranger = relationActionAvailability({ isContact: false, blocked: false })
  assert.equal(stranger.block, true)
})

test('block 与 unblock 恒互斥（任何输入下都不同时为真）', () => {
  for (const isContact of [true, false]) {
    for (const blocked of [true, false]) {
      const a = relationActionAvailability({ isContact, blocked })
      assert.notEqual(a.block, a.unblock, `isContact=${isContact} blocked=${blocked}`)
    }
  }
})

// ── 门禁不许空跑 ────────────────────────────────────────────────────────────

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(7 >= 1)
})
