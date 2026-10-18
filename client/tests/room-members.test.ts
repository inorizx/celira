// 房间成员投影裁决面（`src/api/roomMembers.ts`）的行为测试。
//
// 三处旧实现各写一遍这条链，且**失败策略互相矛盾** —— 本文件把三份旧形状作为**负对照**
// 抄进来，并各配一条能让旧形状真红的输入（避免写出恒真判据）：
//   1. `ChatHeader` 的形状：core 为空数组时**不穿透**（`??` 不认空数组）⇒ 显示 0 名成员；
//   2. `ChatHeader` 第三条来源：`getParticipant` —— **查不到就抛**；
//   3. `ChatPage` 第三条来源：`tryGetParticipant(id)?.kind ?? 'human'` —— **编一个种类**。

import assert from 'node:assert/strict'
import test from 'node:test'

import { roomMemberViews, type RoomMemberRow } from '../src/api/roomMembers.ts'
import {
  getParticipant,
  registerFixtureFallbackGate,
  tryGetParticipant,
} from '../src/api/participantDirectory.ts'
import type { Participant } from '../src/api/domain'
// 副作用导入：fixtures 自己把演示目录登记进参与者目录（本模块只经闸门查询它）。
import { participants, SELF_ID } from '../src/fixtures/data.ts'

function row(id: string, kind: string, display_name: string): RoomMemberRow {
  return { participant: { id, kind, display_name } }
}

const aria = participants.find((p) => p.id === 'p-aria') as Participant
const courier = participants.find((p) => p.id === 'a-courier') as Participant

/** 闸门是模块级状态：用完一律恢复默认（开）。 */
function withGate(allow: boolean, run: () => void) {
  registerFixtureFallbackGate(() => allow)
  try {
    run()
  } finally {
    registerFixtureFallbackGate(() => true)
  }
}

const ids = (list: Participant[]): string[] => list.map((p) => p.id)

// ── 收口前的三份形状（负对照，逐字抄自收刀前的源码）─────────────────────────

/** `ChatHeader.tsx` 旧形状：core 投影非空即用，空数组**不穿透**。 */
function legacyHeaderMembers(
  core: RoomMemberRow[] | undefined,
  room: { participants?: Participant[]; participantIds?: string[] } | undefined,
): Participant[] {
  return (
    core?.map((r) => ({ id: r.participant.id }) as Participant) ??
    room?.participants ??
    (room?.participantIds ?? []).map(getParticipant)
  )
}

/** `ChatPage.tsx` 旧形状：解析不出就编一个种类。 */
function legacyPageKind(id: string): 'human' | 'agent' {
  return (tryGetParticipant(id)?.kind ?? 'human') as 'human' | 'agent'
}

// ── ① 权威来源与优先级 ──────────────────────────────────────────────────────

test('core 非空 ⇒ 只用 core，不看房间自带成员', () => {
  const members = roomMemberViews({
    core: [row('p-x', 'human', 'X')],
    room: { participants: [aria], participantIds: [aria.id] },
  })
  assert.deepEqual(ids(members), ['p-x'])
})

test('core 投影走 toParticipant 统一窄化（词表外的 kind 不透传）', () => {
  const members = roomMemberViews({
    core: [row('a-y', 'agent', 'Y'), row('p-z', 'AGENT', 'Z'), row('p-w', 'robot', 'W')],
  })
  assert.deepEqual(
    members.map((m) => [m.id, m.kind]),
    [
      ['a-y', 'agent'],
      ['p-z', 'human'],
      ['p-w', 'human'],
    ],
  )
})

// ── ② 空数组 = 还没加载（★ 行为与 ChatHeader 旧形状不同）─────────────────────

test('★ core 是空数组 ⇒ 按「未加载」继续往下找（旧 ChatHeader 会显示 0 名成员）', () => {
  const room = { participants: [aria], participantIds: [aria.id] }
  // 负对照：旧形状在同一输入下给出空列表。
  assert.deepEqual(ids(legacyHeaderMembers([], room)), [])
  // 新判据：空成员列表不可能是公共事实 ⇒ 继续用房间自带成员。
  assert.deepEqual(ids(roomMemberViews({ core: [], room })), [aria.id])
})

test('core 缺省 ⇒ 用房间自带成员', () => {
  assert.deepEqual(ids(roomMemberViews({ room: { participants: [aria, courier] } })), [aria.id, courier.id])
})

// ── ③ 目录兜底：闸门说了算 ──────────────────────────────────────────────────

test('两处都没有 ⇒ 按 id 走目录；演示兜底可用时解析出演示身份', () => {
  const members = roomMemberViews({ room: { participantIds: [SELF_ID] } })
  assert.deepEqual(ids(members), [SELF_ID])
  assert.equal(members[0]?.displayName, 'Inori')
})

test('room 为 null / undefined / 空 ⇒ 空列表，绝不抛', () => {
  assert.deepEqual(roomMemberViews({}), [])
  assert.deepEqual(roomMemberViews({ room: null }), [])
  assert.deepEqual(roomMemberViews({ room: { participantIds: [] } }), [])
})

// ── ④ ★ 解析不出：跳过，绝不编种类、绝不抛 ──────────────────────────────────

test('★ 查不到的人不编种类（旧 ChatPage 会把未知 id 断言成人类）', () => {
  withGate(false, () => {
    // 负对照：旧形状把「不知道是谁」说成「人类」——一条凭空的事实。
    assert.equal(legacyPageKind('p-nobody'), 'human')
    // 新判据：这个人不进成员表。
    assert.deepEqual(roomMemberViews({ room: { participantIds: ['p-nobody'] } }), [])
  })
})

test('★ 查不到的人不抛异常（旧 ChatHeader 第三条来源会抛，异常进渲染路径 = 白屏）', () => {
  withGate(false, () => {
    assert.throws(() => getParticipant('p-nobody'), /unknown participant/)
    assert.deepEqual(roomMemberViews({ room: { participantIds: ['p-nobody'] } }), [])
  })
})

test('真实世界（闸门关闭）下演示身份不被当成真实成员', () => {
  withGate(false, () => {
    assert.deepEqual(roomMemberViews({ room: { participantIds: [SELF_ID] } }), [])
  })
  // 闸门恢复后同一个 id 又能解析（证明上一条是闸门的作用，不是解析器坏了）。
  assert.deepEqual(ids(roomMemberViews({ room: { participantIds: [SELF_ID] } })), [SELF_ID])
})

test('部分解析得出时，只列解析得出的那些（保序）', () => {
  withGate(false, () => {
    // 闸门关着 ⇒ 演示名一个都解析不出，两个 id 全跳过。
    assert.deepEqual(roomMemberViews({ room: { participantIds: [SELF_ID, 'p-nobody'] } }), [])
  })
  withGate(true, () => {
    const members = roomMemberViews({ room: { participantIds: [SELF_ID, 'p-nobody'] } })
    assert.deepEqual(ids(members), [SELF_ID])
  })
})
