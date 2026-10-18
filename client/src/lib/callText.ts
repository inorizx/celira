//! **通话那几句话**（封闭表）：这一通怎么结束的、媒体面为什么没起来。
//!
//! 为什么单开一处、而且是纯函数：这一格的判据是**"从我的角度看发生了什么"** ——
//! 同一个 `hangup_caller`，我是发起方时是"你挂断了"，我是被叫方时是"对方挂断了"。
//! 写错就是**说反了**（本仓对"说反了"的容忍度比"没说"低得多），所以它必须能被直测。
//!
//! 与 `api/contractErrors.ts` 的分工：那边是**服务端的稳定码 → 人话**（401/404/409…），
//! 这边是**通话这个域自己的状态 → 人话**（`ended_reason` 四种 ＋ 媒体面三种失败）。

import type { CallView } from '@/api/contracts'
import type { CallMediaFailure } from './callMedia.ts'

/** 通话结束的四种原因（协议判别值，`CORE_V1_CALL.md` §2.1）。 */
export const CALL_ENDED_REASONS = [
  'hangup_caller',
  'hangup_callee',
  'decline_callee',
  'timeout_ring',
] as const

export type CallEndedReason = (typeof CALL_ENDED_REASONS)[number]

function isEndedReason(value: string | null): value is CallEndedReason {
  return value !== null && (CALL_ENDED_REASONS as readonly string[]).includes(value)
}

/**
 * **这一通是怎么结束的**（从"我"的角度说）。
 *
 * ⚠ 认不出的 `ended_reason` **不编**：说"通话结束了"就够 —— 编一个"对方挂断"是**说反了**的
 * 风险（本仓那条"原因没有被记录，不许渲染成某个具体原因"）。
 */
export function callEndedNotice(call: Pick<CallView, 'ended_reason' | 'caller_id'>, selfId: string): string {
  const reason = call.ended_reason
  if (!isEndedReason(reason)) return '通话已结束'
  const iAmCaller = call.caller_id === selfId
  switch (reason) {
    case 'hangup_caller':
      return iAmCaller ? '你挂断了' : '对方挂断了'
    case 'hangup_callee':
      return iAmCaller ? '对方挂断了' : '你挂断了'
    case 'decline_callee':
      // 拒接只可能由 callee 做出 ⇒ 我是 callee 时是我拒的（正常路径不会走到这句）。
      return iAmCaller ? '对方拒接了' : '你拒接了'
    case 'timeout_ring':
      return '没人接（响铃 45 秒后自动结束）'
  }
}

/** 媒体面三种失败的人话（**封闭表**：逐格都有出口）。 */
export const CALL_MEDIA_FAILURE_TEXT: Record<CallMediaFailure, string> = {
  unsupported: '拿不到麦克风 —— 这台机器上的应用可能还没放行它（见「语音消息」那一格）',
  sdp_out_of_bounds: '通话协商数据越界了，没能发出去',
  peer_failed: '没能和对端建立直连（网络环境可能挡了 P2P）',
}

/**
 * **我现在能不能打这一通**（纯判据）：这一格判的是"按下去之后有没有意义"，
 * 不是"服务端会不会拒"（服务端那五种码由 `contractErrors.ts` 说）。
 *
 * 返回 `null` = 可以；否则是**为什么不行**那句人话。
 */
export function callBlockedReason(options: {
  selfId: string
  roomKind: string | undefined
  peerId: string | undefined
  activeCall: CallView | null
}): string | null {
  const { selfId, roomKind, peerId, activeCall } = options
  if (activeCall && activeCall.state !== 'ended') {
    return activeCall.caller_id === selfId ? '你已经在通话里了' : '你正在接一通电话'
  }
  if (!peerId) return '这个房间里没有可通话的对象'
  if (peerId === selfId) return '不能打给自己'
  // 契约：至少一方在调用时刻是该 Room 的 active 成员、双方 active；群房（group）在 P0 里
  // **不做多方**（一次 Call 严格只有两个参与者）—— 群房里能不能打是**产品判断**，
  // 而 `CORE_V1_CALL.md` §6 只排除了"群呼/多方"，没排除"群房里点一个人私聊式地打"。
  // ⇒ 这里**不拦群房**（契约允许任意两个 active participant 互呼），只要求房间里有对端。
  void roomKind
  return null
}
