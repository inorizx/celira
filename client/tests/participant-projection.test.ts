// 参与者投影权威（`src/api/participantProjection.ts`）的行为测试。
//
// 这些用例是「收刀不改行为」的**结构证据**：把收刀前散在各处的实现（两份逐字相同的
// `toParticipant` ＋ 内联重写在调用点的对象字面量）作为**对照实现**写在测试里，用同一
// 组输入逐一比对产出。断言覆盖两类：
//   1. 与两份 `toParticipant` 逐字段相等（含 `signature` / `since`）；
//   2. 与 4 字段内联投影在**可见字段**上逐字段相等（多出的 `signature` / `since`
//      恒为 `''`，而 `''` 与缺省在所有消费点同形 —— 见 `ContactsPage` /
//      `ContactDetailCard` 的 `?? ''` 与 `&&` 用法）。

import assert from 'node:assert/strict'
import test from 'node:test'

import { toParticipant } from '../src/api/participantProjection.ts'
import type { Participant } from '../src/api/domain'

// 收刀前 `api/core.ts` 与 `api/store.ts` 里那份实现的**逐字副本**（两份完全相同）。
function legacyToParticipant(v: {
  id: string
  kind: string
  display_name: string
  status?: string
}): Participant {
  return {
    id: v.id,
    kind: v.kind === 'agent' ? 'agent' : 'human',
    displayName: v.display_name,
    initials: v.display_name.slice(0, 1),
    signature: '',
    since: '',
  }
}

// 收刀前 4 字段内联投影（store 的 `mergeSelf` / `registerLookup`、`ProfilePage`、
// `ChatHeader`、`MyServicesPage`（已随服务层退役删除，2026-09-28）、`RoomInfoDrawer`）
// 的形状副本。
function legacyFourFieldSelf(id: string, displayName: string): Participant {
  return { id, kind: 'human', displayName, initials: displayName.slice(0, 1) }
}

const samples: Array<{ id: string; kind: string; display_name: string; status?: string }> = [
  { id: 'hm_1', kind: 'human', display_name: 'Inori', status: 'active' },
  { id: 'ag_1', kind: 'agent', display_name: 'Courier', status: 'active' },
  { id: 'ag_2', kind: 'agent', display_name: '', status: 'revoked' },
  // 词表之外的取值：窄化成 human，不许透传
  { id: 'x_1', kind: 'AGENT', display_name: 'X' },
  { id: 'x_2', kind: '', display_name: '一个名字特别长的参与者' },
]

test('toParticipant: 与收刀前两份实现逐字段相等', () => {
  for (const s of samples) {
    assert.deepEqual(toParticipant(s), legacyToParticipant(s))
  }
})

test('toParticipant: kind 只认 agent，其余一律 human', () => {
  assert.equal(toParticipant({ id: 'a', kind: 'agent', display_name: 'A' }).kind, 'agent')
  assert.equal(toParticipant({ id: 'h', kind: 'human', display_name: 'H' }).kind, 'human')
  assert.equal(toParticipant({ id: 'x', kind: 'AGENT', display_name: 'X' }).kind, 'human')
  assert.equal(toParticipant({ id: 'y', kind: 'robot', display_name: 'Y' }).kind, 'human')
})

test('toParticipant: initials 取显示名首字符，空名得空串', () => {
  assert.equal(toParticipant({ id: 'a', kind: 'human', display_name: 'Inori' }).initials, 'I')
  assert.equal(toParticipant({ id: 'a', kind: 'human', display_name: '' }).initials, '')
})

test('toParticipant: 与 4 字段内联投影的可见字段逐一相等', () => {
  for (const s of samples.filter((x) => x.kind === 'human')) {
    const after = toParticipant(s)
    const before = legacyFourFieldSelf(s.id, s.display_name)
    assert.equal(after.id, before.id)
    assert.equal(after.kind, before.kind)
    assert.equal(after.displayName, before.displayName)
    assert.equal(after.initials, before.initials)
  }
})

test('toParticipant: 多出的 signature / since 恒为空串（与缺省在所有消费点同形）', () => {
  for (const s of samples) {
    const p = toParticipant(s)
    assert.equal(p.signature, '')
    assert.equal(p.since, '')
  }
  // 真实消费点的判别式：`''` 与 `undefined` 给出同一个渲染结果
  // `ContactsPage` / `MyAgentsPage` 的 `x ?? ''`
  const emptySince: string | undefined = ''
  const absentSince: string | undefined = undefined
  assert.equal(emptySince ?? '', absentSince ?? '')
  // `ContactDetailCard` 的 `{x && <InfoRow/>}`：两者都是 falsy ⇒ 都不渲染该行
  const emptySignature: string | undefined = ''
  const absentSignature: string | undefined = undefined
  assert.equal(Boolean(emptySignature && 'show'), false)
  assert.equal(Boolean(absentSignature && 'show'), false)
})

test('toParticipant: Presence/official 一律不写（不是本投影的事实）', () => {
  const p = toParticipant({ id: 'ag_1', kind: 'agent', display_name: 'Courier', status: 'active' })
  assert.equal('presence' in p, false)
  assert.equal('official' in p, false)
})
