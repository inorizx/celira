import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/utils'

export function AgentBadge({ className }: { className?: string }) {
  return (
    <Badge
      variant="outline"
      className={cn(
        'border-agent/40 bg-agent-surface text-agent text-micro px-1.5 py-0.5 tracking-wider',
        className,
      )}
    >
      AGENT
    </Badge>
  )
}
