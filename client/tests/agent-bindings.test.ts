// 绑定读取面（`src/api/agentBindings.ts`）的行为测试。
//
// 核心判据一条：**「确认没有绑定」与「还没读到」必须分开**。收口前的消费侧形状
// （`bindingsByAgent[agentId] ?? []`）作为负对照抄在下面，并配一条让它真红的输入 ——
// 键缺失时它会给出 `[]`，页面据此说「还没有后端绑定」并递上创建表单。

import assert from 'node:assert/strict'
import test from 'node:test'

import { bindingsOf } from '../src/api/agentBindings.ts'
import type { BackendBindingView } from '../src/api/contracts.ts'

const AGENT = 'agent-7'
const OTHER_AGENT = 'agent-9'

function binding(id: string, status: 'active' | 'revoked'): BackendBindingView {
  return { id, label: `绑定 ${id}`, status, created_at: '2026-09-25T10:00:00Z' } as BackendBindingView
}

/** 收口前的消费侧形状（`MyAgentsPage` 原文）。 */
function legacyBindingsOf(
  byAgent: Record<string, BackendBindingView[]>,
  agentId: string,
): BackendBindingView[] {
  return byAgent[agentId] ?? []
}

test('读到空列表 = **确认没有绑定**（known 且为空，不是"读不到"）', () => {
  assert.deepEqual(bindingsOf({ [AGENT]: [] }, AGENT), { status: 'known', bindings: [] })
})

test('读到绑定 ⇒ known，数组原样带出', () => {
  const list = [binding('b-1', 'active'), binding('b-2', 'revoked')]
  assert.deepEqual(bindingsOf({ [AGENT]: list }, AGENT), { status: 'known', bindings: list })
})

test('★ 键缺失 ⇒ unknown；旧形状在同一输入下说"没有绑定"（页面据此递创建表单）', () => {
  const table = { [OTHER_AGENT]: [binding('b-9', 'active')] }
  // 负对照：旧形状给 `[]` —— 页面显示「还没有后端绑定」，而事实是"还没读到/读失败"。
  assert.deepEqual(legacyBindingsOf(table, AGENT), [])
  // 新判据：如实说没读到。
  assert.deepEqual(bindingsOf(table, AGENT), { status: 'unknown' })
})

test('agentId 缺省 / 空串 ⇒ unknown，且不构造空键去查表', () => {
  const table = { '': [binding('b-0', 'active')] }
  assert.deepEqual(bindingsOf(table, ''), { status: 'unknown' })
  assert.deepEqual(bindingsOf(table, null), { status: 'unknown' })
  assert.deepEqual(bindingsOf(table, undefined), { status: 'unknown' })
})

test('别的 Agent 的键不影响读数（单键，不做任何拼接）', () => {
  assert.deepEqual(bindingsOf({ [OTHER_AGENT]: [] }, AGENT), { status: 'unknown' })
  // 前缀相近的键也不许误命中。
  assert.deepEqual(bindingsOf({ 'agent-70': [] }, AGENT), { status: 'unknown' })
})
