// **通话那几句话的判据**（2026-10-09 接线时立）。
//
// 这一格最容易"说反了"：同一个 `hangup_caller`，我是发起方时是「你挂断了」，我是被叫方时是
// 「对方挂断了」。本仓对"说反了"的容忍度比"没说"低得多 ⇒ 判据必须逐格钉住。
// 另外两条：**认不出的原因不编**（说"通话已结束"就够）、**能不能打**的判据只有一处。
//
// 运行： cd client && npm test

import assert from 'node:assert/strict'
import test from 'node:test'

import {
  CALL_ENDED_REASONS,
  CALL_MEDIA_FAILURE_TEXT,
  callBlockedReason,
  callEndedNotice,
} from '../src/lib/callText.ts'
import type { CallView } from '../src/api/contracts.ts'

const CALLER = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'
const CALLEE = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'

function call(overrides: Partial<CallView> = {}): CallView {
  return {
    call_id: '11111111-1111-4111-8111-111111111111',
    room_id: '22222222-2222-4222-8222-222222222222',
    caller_id: CALLER,
    caller_display: '小满',
    callee_id: CALLEE,
    callee_display: '主人',
    media: 'audio',
    state: 'ended',
    ended_reason: null,
    created_at: '2026-10-09T00:00:00Z',
    accepted_at: null,
    ended_at: '2026-10-09T00:00:10Z',
    ...overrides,
  }
}

test('★ 结束原因：**从我的角度**说（同一个原因，两侧说法相反）', () => {
  assert.equal(callEndedNotice(call({ ended_reason: 'hangup_caller' }), CALLER), '你挂断了')
  assert.equal(callEndedNotice(call({ ended_reason: 'hangup_caller' }), CALLEE), '对方挂断了')
  assert.equal(callEndedNotice(call({ ended_reason: 'hangup_callee' }), CALLER), '对方挂断了')
  assert.equal(callEndedNotice(call({ ended_reason: 'hangup_callee' }), CALLEE), '你挂断了')
  assert.equal(callEndedNotice(call({ ended_reason: 'decline_callee' }), CALLER), '对方拒接了')
  assert.equal(callEndedNotice(call({ ended_reason: 'timeout_ring' }), CALLER), '没人接（响铃 45 秒后自动结束）')
  assert.equal(callEndedNotice(call({ ended_reason: 'timeout_ring' }), CALLEE), '没人接（响铃 45 秒后自动结束）')
})

test('★ 认不出的原因**不编**（说"通话已结束"就够）', () => {
  assert.equal(callEndedNotice(call({ ended_reason: null }), CALLER), '通话已结束')
  assert.equal(callEndedNotice(call({ ended_reason: 'carrier_lost' }), CALLER), '通话已结束')
  assert.equal(callEndedNotice(call({ ended_reason: '' }), CALLER), '通话已结束')
  assert.equal(CALL_ENDED_REASONS.length, 4, '契约里就这四种（改了这里要一起改）')
})

test('媒体面三种失败：封闭表，逐格一句人话（不许空括号）', () => {
  for (const text of Object.values(CALL_MEDIA_FAILURE_TEXT)) {
    assert.ok(text.length > 0)
    assert.ok(!text.includes('（）'))
  }
  assert.match(CALL_MEDIA_FAILURE_TEXT.unsupported, /麦克风/, '拿不到麦克风那一格要说清是权限/应用的事')
})

test('★ 能不能打：判据只有一处（已在通话里 / 没有对端 / 打给自己）', () => {
  const idle = { selfId: CALLER, roomKind: 'direct', peerId: CALLEE, activeCall: null }
  assert.equal(callBlockedReason(idle), null, '空闲 + 有对端 ⇒ 可以打')

  assert.match(String(callBlockedReason({ ...idle, activeCall: call({ state: 'in_progress' }) })), /已经在通话里/)
  assert.match(
    String(callBlockedReason({ ...idle, activeCall: call({ state: 'ringing' }) })),
    /已经在通话里/,
    '振铃中也算"在通话里"（契约：同一 Participant 同时至多一个 active Call）',
  )
  // `ended` 的那一通不算"在通话里"（它已经不是当前这一通了）。
  assert.equal(callBlockedReason({ ...idle, activeCall: call({ state: 'ended' }) }), null)
  assert.match(String(callBlockedReason({ ...idle, peerId: undefined })), /没有可通话的对象/)
  assert.match(String(callBlockedReason({ ...idle, peerId: CALLER })), /不能打给自己/)
})

test('被叫方的"我在通话里"那句与主叫不同（同一格，两种处境）', () => {
  const incoming = call({ state: 'ringing', caller_id: CALLEE, callee_id: CALLER })
  const blocked = callBlockedReason({ selfId: CALLER, roomKind: 'direct', peerId: CALLEE, activeCall: incoming })
  assert.match(String(blocked), /正在接一通电话/)
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(true)
})
