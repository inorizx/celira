// 凭据读取面裁决（`src/api/credentials.ts`）的行为测试。
//
// 核心判据只有一条：**「确认没有」与「读不到」必须分开，键形状漂移必须响**。
// 收口前的形状（`credentialsByBinding[`${agentId}:${bindingId}`] ?? []`）作为负对照抄在
// 下面，并配一条能让它真红的输入：往表里塞一条**别的分隔符**写成的键（模拟生产侧改过、
// 消费侧没跟上），旧形状会平静地给出 `[]` —— 也就是"这个绑定下没有凭据"这条假话。

import assert from 'node:assert/strict'
import test from 'node:test'

import { credentialScopeKey, credentialsOf } from '../src/api/credentials.ts'
import type { CredentialView } from '../src/api/contracts.ts'

const AGENT = '3f1c0b2e-1111-4222-8333-444455556666'
const BINDING = '9a8b7c6d-aaaa-4bbb-8ccc-ddddeeeeffff'
const OTHER_AGENT = '00000000-2222-4333-8444-555566667777'
const OTHER_BINDING = '11111111-bbbb-4ccc-8ddd-eeeeffff0000'

function cred(id: string, status: 'active' | 'revoked'): CredentialView {
  return {
    id,
    binding_id: BINDING,
    status,
    created_at: '2026-09-25T10:00:00Z',
    revoked_at: null,
  } as CredentialView
}

/** 收口前的消费侧形状（`MyAgentsPage` 原文）。 */
function legacyCredentialsOf(
  byBinding: Record<string, CredentialView[]>,
  agentId: string,
  bindingId: string,
): CredentialView[] {
  return byBinding[`${agentId}:${bindingId}`] ?? []
}

test('键由唯一权威给出（两侧同 import，谁都不许再手写模板串）', () => {
  assert.equal(credentialScopeKey(AGENT, BINDING), `${AGENT}:${BINDING}`)
})

test('读到一份空列表 = **确认没有**（known 且 credentials 为空，不是"读不到"）', () => {
  const readout = credentialsOf({ [credentialScopeKey(AGENT, BINDING)]: [] }, AGENT, BINDING)
  assert.deepEqual(readout, { status: 'known', credentials: [] })
})

test('读到凭据 ⇒ known，数组原样带出（不重排、不过滤）', () => {
  const list = [cred('c-1', 'active'), cred('c-2', 'revoked')]
  const readout = credentialsOf({ [credentialScopeKey(AGENT, BINDING)]: list }, AGENT, BINDING)
  assert.deepEqual(readout, { status: 'known', credentials: list })
})

test('键缺失 ⇒ unknown（**不许**说成"没有凭据"）', () => {
  assert.deepEqual(credentialsOf({}, AGENT, BINDING), { status: 'unknown' })
})

test('★ 键形状漂移（换了分隔符）⇒ drifted；旧形状在同一输入下会说"没有凭据"', () => {
  const driftedKey = `${AGENT}_${BINDING}` // 生产侧若改成下划线，消费侧的模板串就查不到
  const table = { [driftedKey]: [cred('c-1', 'active')] }
  // 负对照：旧形状给出空数组 —— 页面会显示"没有凭据"，而表里明明有一条。
  assert.deepEqual(legacyCredentialsOf(table, AGENT, BINDING), [])
  // 新判据：显式报出漂移，而不是把漂移读成"没有"。
  assert.deepEqual(credentialsOf(table, AGENT, BINDING), { status: 'drifted', keys: [driftedKey] })
})

test('★ 键形状漂移（字段顺序颠倒）同样报出来', () => {
  const driftedKey = `${BINDING}:${AGENT}`
  assert.deepEqual(credentialsOf({ [driftedKey]: [] }, AGENT, BINDING), {
    status: 'drifted',
    keys: [driftedKey],
  })
})

test('★ 规范键与漂移键同时存在 ⇒ 漂移优先（两个写者并存的信号不许静默）', () => {
  const table = {
    [credentialScopeKey(AGENT, BINDING)]: [cred('c-1', 'active')],
    [`${AGENT}_${BINDING}`]: [cred('c-2', 'active')],
  }
  const readout = credentialsOf(table, AGENT, BINDING)
  assert.equal(readout.status, 'drifted')
})

test('别人的键不算漂移（只认包含同一个 bindingId 的键）', () => {
  const table = {
    [credentialScopeKey(OTHER_AGENT, OTHER_BINDING)]: [cred('c-9', 'active')],
    // 形状同样是"键"，但不属于这个绑定 ⇒ 与本次读取无关。
    [`${OTHER_AGENT}_${OTHER_BINDING}`]: [cred('c-8', 'active')],
  }
  assert.deepEqual(credentialsOf(table, AGENT, BINDING), { status: 'unknown' })
})

test('短形 id 不许因为子串关系误报漂移（锚点选 bindingId 的理由）', () => {
  // 演示/短形 id 有子串关系：`a-c` 是 `a-courier` 的前缀。
  const table = { [`${'a-courier'}:${OTHER_BINDING}`]: [] }
  assert.deepEqual(credentialsOf(table, 'a-c', BINDING), { status: 'unknown' })
})

test('参数不全 ⇒ unknown，且不去构造 `:binding` 这种残缺键', () => {
  const table = { [`${AGENT}:${BINDING}`]: [] }
  assert.deepEqual(credentialsOf(table, '', BINDING), { status: 'unknown' })
  assert.deepEqual(credentialsOf(table, AGENT, ''), { status: 'unknown' })
})
