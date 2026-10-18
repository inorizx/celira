import { CloudOff, Loader2, RefreshCw, ServerOff } from 'lucide-react'
import { cn } from '@/lib/utils'

export type ConnectionState = 'connecting' | 'offline' | 'backend-offline'

const stateConfig: Record<
  ConnectionState,
  { icon: typeof CloudOff; text: (target?: string) => string; classes: string; retryable: boolean }
> = {
  connecting: {
    icon: Loader2,
    text: () => '连接中…',
    classes: 'bg-muted text-muted-foreground',
    retryable: false,
  },
  offline: {
    icon: CloudOff,
    text: () => '连接已断开，正在重试…',
    classes: 'bg-presence-timeout/15 text-presence-timeout',
    retryable: true,
  },
  'backend-offline': {
    icon: ServerOff,
    text: (target) => `${target ?? 'Agent'} 的执行后端已离线，消息将在其恢复后送达`,
    classes: 'bg-agent-surface text-agent',
    retryable: true,
  },
}

interface ConnectionBannerProps {
  state: ConnectionState
  target?: string
  onRetry?: () => void
}

export function ConnectionBanner({ state, target, onRetry }: ConnectionBannerProps) {
  const config = stateConfig[state]
  const Icon = config.icon
  return (
    <div
      role="status"
      className={cn('flex items-center justify-center gap-2 px-4 py-1.5 text-caption', config.classes)}
    >
      <Icon className={cn('size-3.5', state === 'connecting' && 'animate-spin')} />
      {config.text(target)}
      {config.retryable && onRetry && (
        <button
          type="button"
          onClick={onRetry}
          aria-label="立即重试连接"
          className="inline-flex cursor-pointer items-center gap-1 rounded-full border border-current/30 px-2 py-0.5 font-medium transition-opacity hover:opacity-75"
        >
          <RefreshCw className="size-3" />
          立即重试
        </button>
      )}
    </div>
  )
}
