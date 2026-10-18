/**
 * **通话那一层**（2026-10-09 接线，Call P0 窄解冻切片）。
 *
 * 为什么挂在外壳上（`App.tsx` 里 `RouterProvider` 旁边）而不是会话页里：**来电不看你正停在
 * 哪一页** —— 挂进 ChatPage 就等于"只有打开那间房才收得到电话"，那是一个**静默丢来电**的形状。
 *
 * 三种脸（都由 store 那一格推出来，组件自己不判业务）：
 *
 * | 状态 | 脸 |
 * |---|---|
 * | 我是 callee、`ringing` | **来电**：对方名字 ＋ 接听 / 拒接 |
 * | 我是 caller、`ringing` | **正在呼叫**：对方名字 ＋ 取消 |
 * | `in_progress` | **通话中**：对方名字 ＋ 媒体态（正在连接 / 已接通）＋ 挂断 |
 *
 * ⚠ 两条纪律：
 * 1. **`callNotice`（"对方拒接了"这类话）单独一条**：`call` 在 `ended` 之后就清空了，
 *    而那句话得留在屏幕上让用户看见（它是一次性的，用户点掉或下一次动作时清掉）；
 * 2. **媒体态如实说**：`failed` 时说清"没能建立直连"，不许把"正在连接"一直挂着 ——
 *    那正是"界面比系统知道得少"的形状。
 */

import { Phone, PhoneOff, PhoneCall, X } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { useAppStore } from '@/api/store'

export function CallOverlay() {
  const call = useAppStore((s) => s.call)
  const role = useAppStore((s) => s.callRole)
  const mediaState = useAppStore((s) => s.callMediaState)
  const notice = useAppStore((s) => s.callNotice)
  const me = useAppStore((s) => s.me?.participant.id ?? '')
  const acceptCall = useAppStore((s) => s.acceptCall)
  const declineCall = useAppStore((s) => s.declineCall)
  const hangupCall = useAppStore((s) => s.hangupCall)
  const clearCallNotice = useAppStore((s) => s.clearCallNotice)

  if (!call && !notice) return null

  const peerName = call ? (call.caller_id === me ? call.callee_display : call.caller_display) : ''
  const incoming = call?.state === 'ringing' && role === 'callee'
  const outgoing = call?.state === 'ringing' && role === 'caller'
  const mediaText =
    mediaState === 'connected'
      ? '已接通'
      : mediaState === 'connecting'
        ? '正在连接…'
        : mediaState === 'failed'
          ? '没能建立直连'
          : '准备中…'

  return (
    <div
      role="dialog"
      aria-label="通话"
      className="fixed inset-x-0 bottom-4 z-50 mx-auto flex w-[min(28rem,calc(100%-1.5rem))] flex-col gap-2"
    >
      {notice && (
        <div
          role="status"
          className="flex items-center gap-2 rounded-xl border border-border bg-popover px-3 py-2 text-body shadow-lg"
        >
          <span className="min-w-0 flex-1 truncate">{notice}</span>
          <Button variant="ghost" size="icon" aria-label="知道了" onClick={clearCallNotice}>
            <X className="size-4" />
          </Button>
        </div>
      )}
      {call && (
        <div className="flex items-center gap-3 rounded-xl border border-border bg-card px-3 py-2.5 shadow-lg">
          <span className="flex size-9 shrink-0 items-center justify-center rounded-full bg-accent text-accent-foreground">
            {incoming || outgoing ? <PhoneCall className="size-4" /> : <Phone className="size-4" />}
          </span>
          <span className="min-w-0 flex-1">
            <span className="block truncate text-body">
              {incoming ? `${peerName} 来电` : outgoing ? `正在呼叫 ${peerName}` : `与 ${peerName} 通话中`}
            </span>
            {/* 媒体态只在通话中那一栏有意义（振铃时还没采集）。 */}
            {!incoming && !outgoing && <span className="block text-caption text-muted-foreground">{mediaText}</span>}
          </span>
          {incoming ? (
            <span className="flex shrink-0 gap-1.5">
              <Button size="sm" onClick={() => void acceptCall()} aria-label="接听">
                接听
              </Button>
              <Button size="sm" variant="ghost" onClick={() => void declineCall()} aria-label="拒接">
                拒接
              </Button>
            </span>
          ) : (
            <Button
              size="sm"
              variant="ghost"
              className="shrink-0 text-presence-failed"
              onClick={() => void hangupCall()}
              aria-label={outgoing ? '取消呼叫' : '挂断'}
            >
              <PhoneOff className="size-4" />
              {outgoing ? '取消' : '挂断'}
            </Button>
          )}
        </div>
      )}
    </div>
  )
}
