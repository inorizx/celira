import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/utils'

export function OfficialBadge({ className }: { className?: string }) {
  return (
    <Badge
      variant="outline"
      className={cn(
        'border-primary/40 bg-accent px-1.5 py-0.5 text-micro tracking-wider text-primary',
        className,
      )}
    >
      官方
    </Badge>
  )
}
