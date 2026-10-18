interface UnreadDividerProps {
  label?: string
}

export function UnreadDivider({ label = '以下是新消息' }: UnreadDividerProps) {
  return (
    <div className="flex items-center gap-3 py-1" role="separator">
      <span className="h-px flex-1 bg-primary/40" />
      <span className="text-caption font-medium text-primary">{label}</span>
      <span className="h-px flex-1 bg-primary/40" />
    </div>
  )
}
