// 房间头部读数（`src/api/roomHeadline.ts`）的行为测试。
//
// 这一套的核心判据只有一条：**同一间房，无论成员表加载没加载，读数必须一样**。
// 它是 2026-10-14 真机读数逼出来的（同一间 1:1 房先显示「1 位参与者 · 0 Human · 1 Agent」、
// 加载后又显示「2 位参与者 · 1 Human · 1 Agent」）——所以这里**两种来源各跑一遍、断言同一个答案**，
// 而不是只测好走的那一条。
import assert from 'node:assert/strict'
import test from 'node:test'

import type { RoomLiveSnapshot } from '../src/api/contracts.ts'
import {
  groupHeadline,
  memberRoll,
  membersWithSelf,
  peerPresence,
  presenceLabel,
  selfMember,
} from '../src/api/roomHeadline.ts'

const ME = { id: 'human-me', kind: 'human', displayName: 'Inori' }
const PEER = { id: 'agent-xiaoxin', kind: 'agent', displayName: '小欣' }
const OTHER = { id: 'agent-other', kind: 'agent', displayName: '小测' }

/** 世界的成员表（含我）。 */
const authoritative = [ME, PEER]
/** `direct` 房自带的 `participants`：**只有对端**（这就是那个洞）。 */
const peerOnly = [PEER]

function snapshot(
  members: Array<Partial<RoomLiveSnapshot['members'][number]> & { participant_id: string }>,
): RoomLiveSnapshot {
  return {
    room_id: 'room-1',
    members: members.map((m) => ({
      display_name: m.participant_id,
      kind: 'human',
      online: false,
      typing: false,
      ...m,
    })) as RoomLiveSnapshot['members'],
  }
}

test('★ 同一间 1:1 房：成员表加载与否，读数**必须一样**（这就是那条真机缺陷的反例）', () => {
  const loaded = memberRoll(authoritative, ME, true)
  const fallback = memberRoll(peerOnly, ME, false)

  assert.equal(loaded.total, 2)
  assert.equal(loaded.human, 1)
  assert.equal(loaded.agent, 1)
  // 来源不同 ⇒ 数字**一样**，只有"是不是权威"不同（措辞由它决定）。
  assert.deepEqual(
    { total: fallback.total, human: fallback.human, agent: fallback.agent },
    { total: loaded.total, human: loaded.human, agent: loaded.agent },
  )
  assert.equal(fallback.authoritative, false, '退回来源时要如实说"这只是已知"')
  assert.equal(
    memberRoll([], ME, false).human,
    1,
    '连对端都没读到时，房间里也只少了她一个——我自己永远算数（「0 Human」从此不可能出现）',
  )
})

test('★ 口径：人数**含我**；在线**不含我**（两个主语各自写出来）', () => {
  const roll = memberRoll([PEER, OTHER], ME, true)
  assert.deepEqual(
    { total: roll.total, human: roll.human, agent: roll.agent },
    { total: 3, human: 1, agent: 2 },
    '两个 Agent ＋ 我 ⇒ 3 位参与者、1 Human、2 Agent（0 Human 在结构上不可能）',
  )
  assert.equal(groupHeadline(roll, 2), '3 位参与者（含你） · 1 Human · 2 Agent · 2 位在线（不含你）')
  assert.equal(
    groupHeadline(memberRoll(peerOnly, ME, false), 1),
    '已知 2 位参与者（含你） · 1 Human · 1 Agent · 1 位在线（不含你）',
    '来源不全 ⇒ 说"已知"，不说"就这么多"',
  )
  assert.equal(
    groupHeadline(memberRoll([], ME, true), 0),
    '1 位参与者（含你） · 1 Human · 0 Agent',
    '没有别人在线时那一格整个不出现（现状不变：读不到就不提，不写"0 在线"）',
  )
})

test('含我：不在列表里就补上，已经在里面**不重复**（同一份事实只有一个数）', () => {
  assert.deepEqual(
    membersWithSelf(authoritative, ME).map((m) => m.id),
    ['human-me', 'agent-xiaoxin'],
  )
  assert.deepEqual(membersWithSelf(peerOnly, ME).map((m) => m.id), ['agent-xiaoxin', 'human-me'])
  assert.deepEqual(membersWithSelf(peerOnly, null).map((m) => m.id), ['agent-xiaoxin'])
  assert.deepEqual(membersWithSelf([], null), [])
})

test('我这一行从 `GET /v1/home` 的 participant 来（snake_case 只在这一处翻译）', () => {
  assert.deepEqual(
    selfMember({ participant: { id: 'p-1', kind: 'human', display_name: 'Inori' } }),
    { id: 'p-1', kind: 'human', displayName: 'Inori' },
  )
  assert.equal(selfMember(null), null, '没登录 ⇒ 没有我这一行（数字按来源算）')
  assert.equal(selfMember({ participant: null }), null)
  assert.equal(selfMember({ participant: { id: '', kind: 'human', display_name: 'x' } }), null)
})

test('对端在不在：三态**不许压成两态**（读不到 ≠ 不在线）', () => {
  assert.equal(peerPresence(undefined, PEER.id), 'unknown', '快照没到 ⇒ 读不到')
  assert.equal(peerPresence(snapshot([]), PEER.id), 'unknown', '快照里没有她那一行 ⇒ 读不到')
  assert.equal(peerPresence(snapshot([{ participant_id: PEER.id, online: true }]), PEER.id), 'online')
  assert.equal(
    peerPresence(snapshot([{ participant_id: PEER.id, online: false }]), PEER.id),
    'offline',
    '快照里真有那一行、写着 false ⇒ 才准说"不在线"',
  )
  assert.equal(peerPresence(snapshot([{ participant_id: PEER.id, online: true }]), null), 'unknown')
  assert.equal(presenceLabel('online'), '在线')
  assert.equal(presenceLabel('offline'), '不在线')
  assert.equal(presenceLabel('unknown'), '读不到在线状态')
})
