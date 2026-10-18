// **Call-stream 帧的解析判据**（2026-10-09 接线时立）。
//
// 为什么这一格值得单独立用例：这条流**带事实**（与只发门铃的 `events.available` 相反）——
// 一个读不懂的帧如果被当成某种已知事件，界面会显示**一通不存在的电话**（甚至振铃）。
// 判据三条：① 三种已知帧的形状必须逐字段过；② 形状不对**丢掉**；③ 不认识的 `type` **丢掉**
// （不许拿它当 `ring`），这就是 fail-closed 的全部含义。
//
// 运行： cd client && npm test

import assert from 'node:assert/strict'
import test from 'node:test'

import { parseCallStreamFrame, parseCallView } from '../src/api/callFrames.ts'

const CALL = {
  call_id: '11111111-1111-4111-8111-111111111111',
  room_id: '22222222-2222-4222-8222-222222222222',
  caller_id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',
  caller_display: '小满',
  callee_id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',
  callee_display: '主人',
  media: 'audio',
  state: 'ringing',
  ended_reason: null,
  created_at: '2026-10-09T00:00:00Z',
  accepted_at: null,
  ended_at: null,
}

test('三种已知帧：形状对 ⇒ 原样解出来', () => {
  const ring = parseCallStreamFrame(JSON.stringify({ type: 'call.ring', call: CALL }))
  assert.equal(ring?.type, 'call.ring')
  assert.equal(ring?.call?.call_id, CALL.call_id)
  assert.equal(ring?.call?.state, 'ringing')

  const state = parseCallStreamFrame(
    JSON.stringify({ type: 'call.state', call: { ...CALL, state: 'ended', ended_reason: 'timeout_ring' } }),
  )
  assert.equal(state?.call?.ended_reason, 'timeout_ring')

  const signal = parseCallStreamFrame(
    JSON.stringify({
      type: 'call.signal',
      call_id: CALL.call_id,
      from_id: CALL.caller_id,
      kind: 'offer',
      payload: { sdp: 'v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\n' },
    }),
  )
  assert.equal(signal?.kind, 'offer')
  assert.equal(signal?.payload?.sdp?.startsWith('v=0'), true)
})

test('★ 不认识的 `type` 一律丢掉（不许当成某种已知事件）', () => {
  for (const type of ['call.unknown', 'call.ringing', 'events.available', 'authenticated', '', null]) {
    assert.equal(parseCallStreamFrame(JSON.stringify({ type, call: CALL })), null, `${String(type)} 不该被认下`)
  }
  // 未来上游加了新事件：旧客户端"不认识"是对的 —— 但**不许**把它读成振铃。
  assert.equal(parseCallStreamFrame(JSON.stringify({ type: 'call.video.upgraded', call: CALL })), null)
})

test('★ 形状不对一律丢掉（不是 JSON、不是对象、缺字段）', () => {
  assert.equal(parseCallStreamFrame('not json'), null)
  assert.equal(parseCallStreamFrame('[]'), null)
  assert.equal(parseCallStreamFrame('null'), null)
  // ring/state 必须带一个**形状说得通**的 call。
  assert.equal(parseCallStreamFrame(JSON.stringify({ type: 'call.ring' })), null)
  assert.equal(parseCallStreamFrame(JSON.stringify({ type: 'call.ring', call: {} })), null)
  assert.equal(
    parseCallStreamFrame(JSON.stringify({ type: 'call.ring', call: { ...CALL, media: 'hologram' } })),
    null,
    '认不出的 media 不算合法',
  )
  assert.equal(
    parseCallStreamFrame(JSON.stringify({ type: 'call.state', call: { ...CALL, state: 'paused' } })),
    null,
    '认不出的 state 不许拿它当 ringing',
  )
})

test('★ `call.signal`：`kind` 与 `payload` 必须对得上（对不上丢掉整帧）', () => {
  const base = { type: 'call.signal', call_id: CALL.call_id, from_id: CALL.caller_id }
  assert.equal(parseCallStreamFrame(JSON.stringify({ ...base, kind: 'offer' })), null, 'offer 没有 sdp')
  assert.equal(parseCallStreamFrame(JSON.stringify({ ...base, kind: 'offer', payload: { sdp: '' } })), null)
  assert.equal(parseCallStreamFrame(JSON.stringify({ ...base, kind: 'ice', payload: { sdp: 'v=0' } })), null, 'ice 要 candidates')
  assert.equal(parseCallStreamFrame(JSON.stringify({ ...base, kind: 'ice', payload: { candidates: [] } })), null, '空数组不算')
  assert.equal(parseCallStreamFrame(JSON.stringify({ ...base, kind: 'bye', payload: { sdp: 'x' } })), null, '不认识的 kind')
  assert.equal(parseCallStreamFrame(JSON.stringify({ ...base, kind: 'ice' })), null)
  assert.equal(
    parseCallStreamFrame(JSON.stringify({ ...base, kind: 'ice', payload: { candidates: [{ sdp_mid: 'audio' }] } })),
    null,
    'candidate 缺文本',
  )
  const ok = parseCallStreamFrame(
    JSON.stringify({
      ...base,
      kind: 'ice',
      payload: { candidates: [{ candidate: 'candidate:1 1 udp 1 127.0.0.1 1 typ host', sdp_mid: 'audio', sdp_mline_index: 0 }] },
    }),
  )
  assert.equal(ok?.payload?.candidates?.length, 1)
  assert.equal(ok?.payload?.candidates?.[0]?.sdp_mline_index, 0)
})

test('`parseCallView`：可空字段照契约（`sdp_mid` / `ended_reason` / 时间戳）', () => {
  const view = parseCallView({ ...CALL, ended_reason: undefined, accepted_at: undefined })
  assert.equal(view?.ended_reason, null)
  assert.equal(view?.accepted_at, null)
  assert.equal(view?.caller_display, '小满')
  assert.equal(parseCallView({ ...CALL, caller_id: '' }), null)
  assert.equal(parseCallView({ ...CALL, ended_reason: 42 }), null)
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(true)
})
