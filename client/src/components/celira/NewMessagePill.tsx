import { ArrowDown } from 'lucide-react'

interface NewMessagePillProps {
  count: number
  onClick?: () => void
}

export function NewMessagePill({ count, onClick }: NewMessagePillProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="flex cursor-pointer items-center gap-1.5 rounded-full border border-border bg-popover px-3.5 py-1.5 text-caption font-medium text-primary shadow-lg transition-colors hover:bg-accent"
    >
      有 {count} 条新消息
      <ArrowDown className="size-3.5" />
    </button>
  )
}
