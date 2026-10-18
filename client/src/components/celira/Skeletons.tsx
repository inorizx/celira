import { cn } from '@/lib/utils'

function Bone({ className }: { className?: string }) {
  return <div className={cn('animate-pulse rounded-md bg-muted', className)} />
}

export function ChatSkeleton() {
  return (
    <div className="space-y-1 p-2" aria-hidden>
      {Array.from({ length: 6 }).map((_, i) => (
        <div key={i} className="flex items-center gap-3 px-3 py-3">
          <Bone className="size-10 rounded-full" />
          <div className="flex-1 space-y-2">
            <Bone className={cn('h-3.5', i % 3 === 0 ? 'w-2/5' : 'w-3/5')} />
            <Bone className="h-3 w-4/5" />
          </div>
          <Bone className="h-3 w-8" />
        </div>
      ))}
    </div>
  )
}

export function MessageSkeleton() {
  return (
    <div className="space-y-6 px-4 py-6" aria-hidden>
      {[0, 1, 2, 3, 4].map((i) => {
        const self = i % 2 === 1
        return (
          <div key={i} className={cn('flex gap-3', self && 'flex-row-reverse')}>
            <Bone className="size-9 shrink-0 rounded-full" />
            <Bone
              className={cn(
                'rounded-2xl',
                self ? 'rounded-tr-sm' : 'rounded-tl-sm',
                i % 3 === 0 ? 'h-16 w-64' : 'h-11 w-48',
              )}
            />
          </div>
        )
      })}
    </div>
  )
}
