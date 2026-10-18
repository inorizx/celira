'use strict'
// **认领一个世界里已有的 Agent**（2026-09-25）——两个平面之间那条缝的机器侧判据。
//
// 这一族钉三件事：
//  1. **裁决是纯的、且是支级的**（2026-10-01 起）：家里已经有**别的**住户不挡路（一台机器可以
//     跑多支）；同一个 id ⇒ "已经在住"；`--agent` 空值 ⇒ 用法错（**绝不回落成"自建一个新身份"**）。
//  2. **只能认领自己的**：`GET /v1/agents` 只列自己拥有的 ⇒ 不在里面就拒绝；
//     `status` 不是 `active` 也拒绝（撤销过的身份不许安家）。
//  3. **问不到就不动**：非 200 / 回执读不懂 ⇒ 拒绝，且**一行都不许写进 home.conf**。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')

const { planAdoption, verifyOwnedAgent, findExistingDirectRoom } = require('../adopt')

let cases = 0
function aCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

/** 假 imRequest：按次序回预置响应，并记下收到的参数。 */
function fakeIm(responses) {
  const calls = []
  const fn = async (base, method, url, options) => {
    calls.push({ base, method, url, options })
    const next = responses[calls.length - 1]
    if (!next) throw new Error(`假 imRequest 收到第 ${calls.length} 次调用，但没有预置响应`)
    return next
  }
  return { fn, calls }
}

const ARGS = { base: 'http://127.0.0.1:8080', token: 'human-token' }
const AGENTS_WITH_MINE = {
  status: 200,
  body: { agents: [{ agent: { id: 'a-mine', kind: 'agent', display_name: '小满', status: 'active' } }] },
}

aCase('裁决：空家里给一个 id ⇒ 认领；**没给旗标** ⇒ 照旧（自建那条路）', () => {
  assert.deepEqual(planAdoption({ residentAgentIds: [], requestedId: 'a-1' }), { kind: 'adopt', agentId: 'a-1' })
  assert.deepEqual(planAdoption({ residentAgentIds: [], requestedId: undefined }), { kind: 'as-is' })
  assert.deepEqual(planAdoption({ residentAgentIds: [], requestedId: null }), { kind: 'as-is' })
  assert.deepEqual(planAdoption({}), { kind: 'as-is' })
})

aCase('★ 裁决：判据是**支级**的 —— 家里有别的住户**不挡路**（一台机器可以跑多支）', () => {
  // 从前这里是 `refuse-occupied`（一个家只住一支）。2026-10-01 起（PROJECT_SHAPE §8.0）：
  // 问的是"**这个 id** 是不是已经是本家的住户"，别人住不住与它无关。
  assert.deepEqual(planAdoption({ residentAgentIds: ['a-old'], requestedId: 'a-new' }), {
    kind: 'adopt',
    agentId: 'a-new',
  })
  assert.deepEqual(
    planAdoption({ residentAgentIds: ['a-old', 'a-older'], requestedId: 'a-new' }),
    { kind: 'adopt', agentId: 'a-new' },
  )
  // 负对照：同一个 id ⇒ 不是"认领"，是"已经在住"（重跑幂等，不许报错、也不许再认一次）
  assert.deepEqual(planAdoption({ residentAgentIds: ['a-1'], requestedId: 'a-1' }), { kind: 'already', agentId: 'a-1' })
  assert.deepEqual(planAdoption({ residentAgentIds: [' a-1 '], requestedId: 'a-1' }), { kind: 'already', agentId: 'a-1' })
  // 住户表里有空串 / 非字符串 ⇒ 不许把它当成"已经有这一支"。
  assert.deepEqual(planAdoption({ residentAgentIds: ['', null], requestedId: 'a-1' }), {
    kind: 'adopt',
    agentId: 'a-1',
  })
})

aCase('裁决：`--agent` 给了个不是 id 的东西（空值/空白/带空格/过长）⇒ 用法错（绝不回落成自建）', () => {
  assert.deepEqual(planAdoption({ residentAgentIds: [], requestedId: '' }), { kind: 'refuse-bad-id' })
  assert.deepEqual(planAdoption({ residentAgentIds: [], requestedId: '   ' }), { kind: 'refuse-bad-id' })
  assert.deepEqual(planAdoption({ residentAgentIds: [], requestedId: 'a b' }), { kind: 'refuse-bad-id' })
  assert.deepEqual(planAdoption({ residentAgentIds: [], requestedId: 'x'.repeat(201) }), { kind: 'refuse-bad-id' })
})

aCase('★ 认领：是我拥有的 active Agent ⇒ 成，读数点名它', async () => {
  const im = fakeIm([AGENTS_WITH_MINE])
  const out = await verifyOwnedAgent({ ...ARGS, agentId: 'a-mine', imRequest: im.fn })
  assert.equal(out.ok, true)
  assert.equal(out.name, '小满')
  assert.equal(out.status, 'active')
  assert.equal(im.calls.length, 1)
  assert.equal(im.calls[0].url, '/v1/agents')
  assert.equal(im.calls[0].method, 'GET')
  assert.match(out.lines.join('\n'), /是你的/)
})

aCase('★ 认领：不在我的列表里（别人的 / 不存在的）⇒ 拒绝，并说清"只列你自己的"', async () => {
  const im = fakeIm([AGENTS_WITH_MINE])
  const out = await verifyOwnedAgent({ ...ARGS, agentId: 'someone-else', imRequest: im.fn })
  assert.equal(out.ok, false)
  assert.equal(out.reason, 'not-owned')
  assert.match(out.lines.join('\n'), /不是你的/)
  assert.match(out.lines.join('\n'), /只列\*\*你拥有的\*\*/)
})

aCase('★ 认领：撤销过的身份（status≠active）⇒ 拒绝', async () => {
  const im = fakeIm([
    { status: 200, body: { agents: [{ agent: { id: 'a-dead', display_name: '小满', status: 'revoked' } }] } },
  ])
  const out = await verifyOwnedAgent({ ...ARGS, agentId: 'a-dead', imRequest: im.fn })
  assert.equal(out.ok, false)
  assert.equal(out.reason, 'not-active')
  assert.match(out.lines.join('\n'), /撤销过的身份不许安家/)
})

aCase('★ 认领：问不到世界（非 200 / 回执读不懂）⇒ 拒绝（什么都不改）', async () => {
  for (const answer of [{ status: 401, body: {} }, { status: 500, body: {} }, { status: 200, body: {} }]) {
    const im = fakeIm([answer])
    const out = await verifyOwnedAgent({ ...ARGS, agentId: 'a-1', imRequest: im.fn })
    assert.equal(out.ok, false, `status=${answer.status} 必须拒绝`)
    assert.equal(out.reason, 'unreachable')
    assert.match(out.lines.join('\n'), /什么都不改|什么都不改/)
  }
})

aCase('值班室：找得到她与主人那间 direct room ⇒ 沿用（不多开一间）', async () => {
  const im = fakeIm([
    {
      status: 200,
      body: {
        rooms: [
          { room: { id: 'r-group', kind: 'group' }, peer: { id: 'a-mine' } },
          { room: { id: 'r-direct', kind: 'direct' }, peer: { id: 'a-mine' } },
          { room: { id: 'r-other', kind: 'direct' }, peer: { id: 'a-other' } },
        ],
      },
    },
  ])
  const out = await findExistingDirectRoom({ ...ARGS, agentId: 'a-mine', imRequest: im.fn })
  assert.equal(out.roomId, 'r-direct', '群房与别人的 direct 房都不算')
  assert.equal(out.looked, true)
})

aCase('值班室：没有她那间 ⇒ null（照旧开一间）；列表读不到 ⇒ looked=false（如实说一句）', async () => {
  const im = fakeIm([
    { status: 200, body: { rooms: [{ room: { id: 'r-other', kind: 'direct' }, peer: { id: 'a-other' } }] } },
    { status: 503, body: {} },
  ])
  const none = await findExistingDirectRoom({ ...ARGS, agentId: 'a-mine', imRequest: im.fn })
  assert.deepEqual(none, { roomId: null, looked: true })
  const blind = await findExistingDirectRoom({ ...ARGS, agentId: 'a-mine', imRequest: im.fn })
  assert.deepEqual(blind, { roomId: null, looked: false })
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(cases > 0, '一条用例都没跑（空跑不算过）')
  assert.equal(cases, 9, `用例数 ${cases} 与声明的 9 不符`)
})
