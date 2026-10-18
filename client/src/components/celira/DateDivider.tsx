interface DateDividerProps {
  label: string
}

export function DateDivider({ label }: DateDividerProps) {
  return (
    <div className="flex items-center gap-3 py-1" role="separator">
      <span className="h-px flex-1 bg-border" />
      <span className="rounded-full bg-muted px-3 py-1 text-caption text-muted-foreground">{label}</span>
      <span className="h-px flex-1 bg-border" />
    </div>
  )
}
