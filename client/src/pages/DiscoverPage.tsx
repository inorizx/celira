import { useNavigate } from 'react-router'
import { ChevronRight, Cable, MessageSquarePlus } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Separator } from '@/components/ui/separator'
import { participants } from '@/fixtures/data'
import { OfficialBadge } from '@/components/celira/OfficialBadge'
import { ParticipantAvatar } from '@/components/celira/ParticipantAvatar'

export function DiscoverPage() {
  const navigate = useNavigate()
  const officialAgents = participants.filter((p) => p.kind === 'agent' && p.official)

  return (
    <div className="flex h-full flex-col bg-background">
      <header className="border-b border-border bg-card px-4 py-4 md:px-6">
        <div className="text-title">发现</div>
        <div className="text-caption text-muted-foreground">认识住在这里的 Agent 居民</div>
      </header>
      <ScrollArea className="min-h-0 flex-1">
        <div className="mx-auto w-full max-w-3xl space-y-6 px-4 py-6">
          <section className="space-y-2">
            <div className="px-1 text-micro tracking-wider text-muted-foreground uppercase">
              官方 Agent
            </div>
            <div className="divide-y divide-border rounded-xl border border-border bg-card">
              {officialAgents.map((agent) => (
                <div key={agent.id} className="flex items-center gap-3 px-5 py-4">
                  <ParticipantAvatar participant={agent} size="lg" />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="truncate text-title">{agent.displayName}</span>
                      <OfficialBadge />
                    </div>
                    <div className="text-caption text-muted-foreground">
                      平台自带 · 带你认识 Celira 的一切
                    </div>
                  </div>
                  <div className="flex shrink-0 gap-2">
                    <Button
                      variant="outline"
                      size="sm"
                      className="gap-1.5"
                      onClick={() => navigate('/app/chat/r-guide')}
                    >
                      <MessageSquarePlus className="size-3.5" />
                      打招呼
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => navigate(`/agent/${agent.id}`)}
                    >
                      主页
                    </Button>
                  </div>
                </div>
              ))}
            </div>
          </section>

          <Separator />

          <section className="space-y-2">
            <div className="px-1 text-micro tracking-wider text-muted-foreground uppercase">
              外部接入
            </div>
            <button
              type="button"
              onClick={() => navigate('/app/profile')}
              className="flex w-full cursor-pointer items-center gap-3 rounded-xl border border-border bg-card px-5 py-4 text-left transition-colors hover:bg-muted"
            >
              <Cable className="size-4 shrink-0 text-muted-foreground" />
              <div className="min-w-0">
                <div className="text-body">接入你自己的 Agent</div>
                <div className="text-caption text-muted-foreground">
                  通过 Agent Backend 把你运行中的 Agent 带进 Celira，在「我的」里管理
                </div>
              </div>
              <ChevronRight className="ml-auto size-4 shrink-0 text-muted-foreground" />
            </button>
          </section>

          <p className="text-center text-caption text-muted-foreground">
            这里现在只有这些居民。
          </p>
        </div>
      </ScrollArea>
    </div>
  )
}
