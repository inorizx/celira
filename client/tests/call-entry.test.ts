// **通话入口的判据**（2026-10-09 接线时立，主人当次判断："**电话只人跟人**"）。
//
// 这一格判的不是"契约允许谁呼谁"（`CORE_V1_CALL.md` 允许任意两个 active participant 互呼），
// 而是**界面该不该给入口** —— 判据是"今天做得到什么"：
//   · 对端是**人** ⇒ 语音是真入口（人的"在不在电话旁"由连接派生，响铃/接听/挂断对人是成立的动作）；
//   · 对端是 **Agent** ⇒ 一个入口都不给（实时语音要求两端各有采集/播放那一面，而执行端今天
//     只会读事件、写文本消息 ⇒ 给了就是"按下去对面永远没人接"的假入口）；
//   · **群房** ⇒ 不给（Call P0 明确不做多方）；
//   · **视频** ⇒ 今天一律只是提示（壳只放行麦克风，Camera 一律拒）。
//
// 运行： cd client && npm test

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'

import { callEntryFor } from '../src/api/uiPolicy.ts'
import type { Participant, Room } from '../src/api/domain.ts'

const ME = 'human-me'
const FRIEND: Participant = { id: 'human-friend', kind: 'human', displayName: '阿元', initials: '阿' }
const AGENT: Participant = { id: 'agent-1', kind: 'agent', displayName: '小满', initials: '满' }

function room(overrides: Partial<Room>): Pick<Room, 'kind' | 'peer'> {
  return {
    kind: 'human_direct',
    peer: FRIEND,
    ...overrides,
  }
}

test('★ 人↔人：语音是**真入口**，视频只是提示', () => {
  const entry = callEntryFor(room({}), ME)
  assert.equal(entry.audio, 'call')
  assert.equal(entry.video, 'hint')
  assert.equal(entry.reason, '', '给了真入口就不该有"为什么没给"的理由')
})

test('★ 对端是 Agent：**一个入口都不给**（给了就是按下去没人接的假入口）', () => {
  const entry = callEntryFor(room({ kind: 'direct', peer: AGENT }), ME)
  assert.equal(entry.audio, 'none')
  assert.equal(entry.video, 'none')
  assert.match(entry.reason, /Agent/, '理由要点名"对端是 Agent"')
  assert.match(entry.reason, /执行端/, '并说清前置在哪：执行端还没有那一面')
})

test('★ 群房：不给（Call P0 不做多方）', () => {
  const entry = callEntryFor(room({ kind: 'group', peer: AGENT }), ME)
  assert.equal(entry.audio, 'none')
  assert.equal(entry.video, 'none')
  assert.match(entry.reason, /多方|群/)
})

test('没有对端 / 对端是自己：不给（并说得出为什么）', () => {
  const empty = callEntryFor(room({ peer: undefined }), ME)
  assert.equal(empty.audio, 'none')
  assert.match(empty.reason, /对端/)
  const self = callEntryFor(room({ peer: { ...FRIEND, id: ME } }), ME)
  assert.equal(self.audio, 'none')
})

test('★ 不变式：`none` 必须给得出理由；**视频今天永远不是真入口**', () => {
  const cases = [
    callEntryFor(room({}), ME),
    callEntryFor(room({ kind: 'direct', peer: AGENT }), ME),
    callEntryFor(room({ kind: 'group' }), ME),
    callEntryFor(room({ peer: undefined }), ME),
  ]
  for (const entry of cases) {
    if (entry.audio === 'none') assert.ok(entry.reason.length > 0, '不给入口必须说得出为什么')
    // 壳只放行麦克风（Camera 一律 Default = 拒）⇒ 视频真入口今天**不存在**。
    // 将来接通话的视频面时，这一条会红 —— 那时要一起改的是壳那一处 `on_permission_request`。
    assert.notEqual(entry.video, 'call', '视频真入口今天不该存在')
  }
})

test('★ 接线：两个按钮都按 `callEntry` 门控（不许无条件渲染通话入口）', () => {
  const source = readFileSync(new URL('../src/components/celira/ChatHeader.tsx', import.meta.url), 'utf8')
  assert.match(source, /callEntryFor\(room, myId \?\? ''\)/, '判据只有一处（`callEntryFor`）')
  assert.match(source, /\{callEntry\.audio !== 'none' && \(/, '语音按钮必须门控')
  assert.match(source, /\{callEntry\.video !== 'none' && \(/, '视频按钮必须门控')
  // 点了必须走 store 的动作（**不许**页面自己拼信令 —— 信令的唯一处理点在 store）。
  assert.match(source, /startCall\(room\.id, room\.peer\.id\)/)
  assert.match(source, /if \(callEntry\.audio !== 'call' \|\| !room\.peer\) return/, '真入口之外必须早退')
  assert.ok(!source.includes('/v1/calls'), '页面不许自己拼协议')
})

test('★ 接线：来电那一层挂在外壳上（**不挂在会话页里**）', () => {
  // 判据：来电不看你正停在哪一页 —— 挂进 ChatPage 就等于"只有打开那间房才收得到电话"。
  const app = readFileSync(new URL('../src/App.tsx', import.meta.url), 'utf8')
  assert.match(app, /<CallOverlay \/>/)
  assert.ok(app.indexOf('<CallOverlay />') > app.indexOf('<RouterProvider'), '该与 RouterProvider 同层')
  const chat = readFileSync(new URL('../src/pages/ChatPage.tsx', import.meta.url), 'utf8')
  assert.ok(!chat.includes('CallOverlay'), '会话页里不许再挂一层（会与外壳那层打架）')
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(true)
})
