import { cn } from '@/lib/utils'
import type { Message } from '@/api/domain'

// ReplyQuote 的三种投影（由调用方传入**当前已加载的真实目标消息/参与者信息**，
// 组件绝不直接查 fixtures、绝不复制被引用目标正文）：
// - loaded：目标已加载 → 展示当前公共作者资料与正文预览；
// - recalled：目标已撤回 → 只显示「[该消息已撤回]」，绝不泄露撤销前正文；
// - unloaded：目标尚未加载 → 显示稳定占位，点击后由调用方按 opaque `before`
//   分页逐页加载更早消息直到找到或没有更旧页（不猜 cursor、不无限循环）。
export type ReplyTargetState =
  | { status: 'loaded'; message: Message; authorName: string }
  | { status: 'recalled'; authorName?: string }
  | { status: 'unloaded' }

interface ReplyQuoteProps {
  replyToId: string
  /** 调用方注入的目标解析器（引用当前已加载的权威 timeline 与参与者投影）。 */
  resolveTarget: (messageId: string) => ReplyTargetState
  onSelfBubble?: boolean
  onJump?: (messageId: string) => void
  /** 目标未加载时的点击出口：调用方按 opaque `before` 分页加载更早消息。 */
  onLoadOlder?: (messageId: string) => void
}

export function ReplyQuote({
  replyToId,
  resolveTarget,
  onSelfBubble = false,
  onJump,
  onLoadOlder,
}: ReplyQuoteProps) {
  const state = resolveTarget(replyToId)
  const authorName = state.status === 'unloaded' ? undefined : state.authorName
  const recalled = state.status === 'recalled'
  const unloaded = state.status === 'unloaded'
  const preview = recalled
    ? '[该消息已撤回]'
    : unloaded
      ? '引用消息未加载'
      : state.message.body.length > 40
        ? `${state.message.body.slice(0, 40)}…`
        : state.message.body

  return (
    <div
      onClick={() => {
        if (unloaded) onLoadOlder?.(replyToId)
        else onJump?.(replyToId)
      }}
      className={cn(
        'mb-2 cursor-pointer rounded-lg border-l-2 px-2.5 py-1.5 text-caption transition-opacity hover:opacity-75',
        onSelfBubble
          ? 'border-bubble-self-foreground/45 bg-white/10 text-bubble-self-foreground'
          : 'border-primary/60 bg-muted text-muted-foreground',
        recalled && 'opacity-60',
        unloaded && 'opacity-70',
      )}
    >
      {authorName && (
        <>
          <span className="font-medium">{authorName}</span>
          <span className="mx-1.5 opacity-50">·</span>
        </>
      )}
      <span className="opacity-80">{preview}</span>
    </div>
  )
}
