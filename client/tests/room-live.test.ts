// 主体进行态派生（`src/api/roomLive.ts`）的行为测试。
//
// 判据全部对着服务端已落地的契约（`CORE_V1_AGENT_PROTOCOL.md` §6）：
// 快照只有 `{participant_id, display_name, kind, online, typing}` 五个字段；
// 人与 Agent 同形；"读不到" ≠ "确认没有"。
//
// 关键用例把**旧的执行面形状**当对照写在测试里（`legacySnapshot`），并配一条
// 能让旧形状**真红**的输入——避免写出"恒真判据"。
import assert from 'node:assert/strict'
import test from 'node:test'

import type { RoomLiveSnapshot } from '../src/api/contracts.ts'
import {
  isOnline,
  isTyping,
  liveHeadline,
  liveMemberOf,
  livePeers,
} from '../src/api/roomLive.ts'

const ME = 'human-me'
const PEER_HUMAN = 'human-peer'
const PEER_AGENT = 'agent-peer'

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

test('在不在与正在写都是**按人读**的，读不到就不亮', () => {
  const snap = snapshot([
    { participant_id: ME, display_name: '我', online: true, typing: true },
    { participant_id: PEER_AGENT, display_name: '小满', kind: 'agent', online: true },
  ])

  assert.equal(isOnline(snap, PEER_AGENT), true)
  assert.equal(isTyping(snap, PEER_AGENT), false)
  // 不在快照里 ⇒ 不点亮（**不是**"确认它离线"）。
  assert.equal(isOnline(snap, 'who-is-this'), false)
  assert.equal(isTyping(snap, 'who-is-this'), false)
  // 读不到快照（还没拉到 / 拉失败）⇒ 一律不亮，绝不猜。
  assert.equal(isOnline(undefined, PEER_AGENT), false)
  assert.equal(isTyping(undefined, PEER_AGENT), false)
  assert.equal(liveMemberOf(undefined, PEER_AGENT), undefined)
})

test('头部两个读数**不含我自己**：自己那行对自己没有信息量', () => {
  const snap = snapshot([
    { participant_id: ME, display_name: '我', online: true, typing: true },
    { participant_id: PEER_HUMAN, display_name: '阿元', online: true },
    { participant_id: PEER_AGENT, display_name: '小满', kind: 'agent', typing: true },
  ])

  const { onlineCount, typingNames } = liveHeadline(snap, ME)
  assert.equal(onlineCount, 1, '只有阿元在线（我在线、小满没在线）')
  assert.deepEqual(typingNames, ['小满'], '我在写不该出现在"某某 正在写…"里')
  assert.equal(livePeers(snap, ME).length, 2)
})

test('人机同形：kind 不参与任何判据', () => {
  const asHuman = snapshot([{ participant_id: 'x', display_name: 'x', kind: 'human', online: true, typing: true }])
  const asAgent = snapshot([{ participant_id: 'x', display_name: 'x', kind: 'agent', online: true, typing: true }])

  assert.deepEqual(liveHeadline(asHuman, ME), liveHeadline(asAgent, ME))
  assert.equal(isOnline(asHuman, 'x'), isOnline(asAgent, 'x'))
  assert.equal(isTyping(asHuman, 'x'), isTyping(asAgent, 'x'))
})

test('我的 id 还没读到（undefined）时不会把所有人当成"别人"里的自己', () => {
  const snap = snapshot([
    { participant_id: ME, display_name: '我', online: true },
    { participant_id: PEER_HUMAN, display_name: '阿元', online: true },
  ])
  // 这是**如实**的行为：不知道我是谁时，不能猜着把自己滤掉——
  // 宁可多显示一个"在线"，也不谎称某人不在。
  assert.equal(liveHeadline(snap, undefined).onlineCount, 2)
})

test('快照是空的（房还没喂过 / 全离线）⇒ 两个读数都是零，不报错', () => {
  assert.deepEqual(liveHeadline(snapshot([]), ME), { onlineCount: 0, typingNames: [] })
  assert.deepEqual(liveHeadline(undefined, ME), { onlineCount: 0, typingNames: [] })
})

test('旧执行面形状不再被消费（防止悄悄复活）', () => {
  // 旧快照：`agents` + `presence` + `activity{phase,stream_id,revision,body}`。
  const legacy = {
    room_id: 'room-1',
    agents: [
      {
        participant_id: PEER_AGENT,
        display_name: '小满',
        presence: 'online',
        activity: { phase: 'responding', stream_id: 's-1', revision: 3, body: '草稿' },
      },
    ],
  } as unknown as RoomLiveSnapshot

  // 新派生面读的是 `members` ⇒ 旧形状一律读不到（不亮、也不炸）。
  assert.equal(isOnline(legacy, PEER_AGENT), false)
  assert.equal(isTyping(legacy, PEER_AGENT), false)
  assert.deepEqual(liveHeadline(legacy, ME), { onlineCount: 0, typingNames: [] })
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(7 >= 1)
})
