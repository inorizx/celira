//! **Call-stream 帧的解析**（`GET /v1/call/stream`，`CORE_V1_CALL.md` §4.1）。
//!
//! 为什么单开一个文件、而且是**纯函数**：这条流是**带事实**的（与只发门铃的
//! `events.available` 相反）⇒ 一个坏帧可能让界面显示一通**不存在的电话**。
//! 判据因此要能被 `node --test` 直接量，而不是埋在 `core.ts` 的 `onmessage` 里。
//!
//! 三条纪律：
//! 1. **认不出的 `type` 一律丢掉**（`null`）—— 上游以后加事件类型时，旧客户端"不认识"是对的，
//!    但**不许**把不认识的东西当成某一种已知事件（fail closed）；
//! 2. **形状不合法也丢掉**：`call.ring` / `call.state` 必须带 `call` 且 `call_id`/`state` 说得通；
//!    `call.signal` 必须带 `call_id` / `from_id` / 合法 `kind` / 与 `kind` 对得上的 `payload`；
//! 3. **不解析 SDP/ICE 内容**（Core 也不解析）：只做"这一格在不在、是什么类型"的形状判据。

import type { CallIceCandidate, CallStreamEvent, CallView } from './contracts'

/** 信令的三种 `kind`（协议判别值，与 `im/server/src/protocol/call.rs` 的 `signal_kind` 同名）。 */
export const CALL_SIGNAL_KINDS = ['offer', 'answer', 'ice'] as const

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function nonEmptyString(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0
}

/**
 * 一条 `CallView` 的**形状**判据（不是语义判据）：三个 id/两个 display 非空、`media` 与 `state`
 * 在枚举里、`ended_reason` 是字符串或 null。
 *
 * ⚠ 认不出的 `media` / `state` **不算合法**（`null`）—— 上游将来加了新的状态取值，
 * 旧客户端应当如实说"这一格我读不懂"，而不是拿它当 `ringing` 去振铃。
 */
export function parseCallView(value: unknown): CallView | null {
  if (!isRecord(value)) return null
  const { call_id, room_id, caller_id, caller_display, callee_id, callee_display, media, state } = value
  if (
    !nonEmptyString(call_id) ||
    !nonEmptyString(room_id) ||
    !nonEmptyString(caller_id) ||
    !nonEmptyString(callee_id)
  ) {
    return null
  }
  if (media !== 'audio' && media !== 'video') return null
  if (state !== 'ringing' && state !== 'in_progress' && state !== 'ended') return null
  const endedReason = value.ended_reason
  if (endedReason !== null && endedReason !== undefined && typeof endedReason !== 'string') return null
  return {
    call_id,
    room_id,
    caller_id,
    caller_display: typeof caller_display === 'string' ? caller_display : '',
    callee_id,
    callee_display: typeof callee_display === 'string' ? callee_display : '',
    media,
    state,
    ended_reason: typeof endedReason === 'string' ? endedReason : null,
    created_at: typeof value.created_at === 'string' ? value.created_at : '',
    accepted_at: typeof value.accepted_at === 'string' ? value.accepted_at : null,
    ended_at: typeof value.ended_at === 'string' ? value.ended_at : null,
  }
}

/** 一条 ICE candidate 的形状（`sdp_mid` / `sdp_mline_index` 可空 —— 契约如此）。 */
function parseCandidate(value: unknown): CallIceCandidate | null {
  if (!isRecord(value)) return null
  if (!nonEmptyString(value.candidate)) return null
  const mid = value.sdp_mid
  const index = value.sdp_mline_index
  if (mid !== null && mid !== undefined && typeof mid !== 'string') return null
  if (index !== null && index !== undefined && typeof index !== 'number') return null
  return {
    candidate: value.candidate,
    sdp_mid: typeof mid === 'string' ? mid : null,
    sdp_mline_index: typeof index === 'number' ? index : null,
  }
}

/**
 * 把一条 WS 帧文本解成 `CallStreamEvent`；**任何读不懂的形状一律 `null`**。
 *
 * `kind` 与 `payload` 必须对得上（契约 §2.2）：`offer`/`answer` 要 `payload.sdp`（非空字符串）、
 * `ice` 要 `payload.candidates`（非空数组、逐条过形状）。对不上的**丢掉整帧**。
 */
export function parseCallStreamFrame(raw: string): CallStreamEvent | null {
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return null
  }
  if (!isRecord(parsed)) return null
  const type = parsed.type

  if (type === 'call.ring' || type === 'call.state') {
    const call = parseCallView(parsed.call)
    if (!call) return null
    return { type, call }
  }

  if (type === 'call.signal') {
    const { call_id, from_id, kind, payload } = parsed
    if (!nonEmptyString(call_id) || !nonEmptyString(from_id)) return null
    if (kind !== 'offer' && kind !== 'answer' && kind !== 'ice') return null
    if (!isRecord(payload)) return null
    if (kind === 'ice') {
      const raw = payload.candidates
      if (!Array.isArray(raw) || raw.length === 0) return null
      const candidates: CallIceCandidate[] = []
      for (const each of raw) {
        const candidate = parseCandidate(each)
        if (!candidate) return null
        candidates.push(candidate)
      }
      return { type, call_id, from_id, kind, payload: { candidates } }
    }
    if (!nonEmptyString(payload.sdp)) return null
    return { type, call_id, from_id, kind, payload: { sdp: payload.sdp } }
  }

  // ⚠ 不认识的 `type`：丢掉（**不许**当成某种已知事件）。
  return null
}
