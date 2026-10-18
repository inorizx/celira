import { useState } from 'react'
import { UserPlus } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { cn } from '@/lib/utils'
import { useAppStore } from '@/api/store'
import { roomCandidates } from '@/api/uiPolicy'
import { roomActionErrorText } from '@/api/contractErrors'
import { AgentBadge } from './AgentBadge'
import { OfficialBadge } from './OfficialBadge'
import { ParticipantAvatar } from './ParticipantAvatar'

interface InviteMemberSheetProps {
  roomId: string
  currentMemberIds: string[]
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function InviteMemberSheet({
  roomId,
  currentMemberIds,
  open,
  onOpenChange,
}: InviteMemberSheetProps) {
  const contacts = useAppStore((s) => s.contacts)
  const ownedAgents = useAppStore((s) => s.ownedAgents)
  const inviteMember = useAppStore((s) => s.inviteMember)
  const [inviting, setInviting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // 候选源只来自既有投影（accepted contacts + owned agents），不自造目录、
  // 不回退 fixtures。
  //
  // ⚠ **按 id 去重**（2026-09-29 真机实测）：同一位 Agent 可以同时是 accepted 联系人与自有
  // Agent（屋主给自己的 Agent 发过好友申请并接受过），直接拼两个数组会让它列出**两行同一个
  // id**（React key 也重复）。规则与「发起新会话」共用 `uiPolicy.roomCandidates` 那一份。
  const candidates = roomCandidates(contacts, ownedAgents)
  const available = candidates.filter((p) => !currentMemberIds.includes(p.id))

  const invite = async (id: string) => {
    setInviting(true)
    setError(null)
    try {
      // 幂等 key 由 core 侧自生成（`RoomApi` 契约：可选参数，core 忽略调用方传的）；
      // Host 那套「按逻辑邀请复用并轮换内存 key」的纪律已随 TASK_64 删除。
      await inviteMember(roomId, id, undefined)
      onOpenChange(false)
    } catch (e) {
      setError(roomActionErrorText(e))
    } finally {
      setInviting(false)
    }
  }

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="w-full gap-0 p-0 sm:max-w-md">
        <SheetHeader className="border-b border-border">
          <SheetTitle>邀请成员</SheetTitle>
          <SheetDescription>从联系人和你拥有的 Agent 中邀请进群</SheetDescription>
        </SheetHeader>
        <ScrollArea className="min-h-0 flex-1 [&_[data-radix-scroll-area-viewport]>div]:!block">
          <ul className="space-y-0.5 p-3">
            {available.map((p) => (
              <li key={p.id}>
                <div className="flex items-center gap-3 rounded-lg px-3 py-3 transition-colors hover:bg-muted">
                  <ParticipantAvatar participant={p} size="md" />
                  <span className="min-w-0 flex-1">
                    <span className="flex items-center gap-2">
                      <span className={cn('truncate text-body', p.kind === 'agent' && 'text-agent')}>
                        {p.displayName}
                      </span>
                      {p.kind === 'agent' && (p.official ? <OfficialBadge /> : <AgentBadge />)}
                    </span>
                    <span className="text-caption text-muted-foreground">
                      {p.kind === 'agent' ? 'Agent' : 'Human'}
                    </span>
                  </span>
                  <Button
                    size="sm"
                    variant="outline"
                    className="gap-1"
                    disabled={inviting}
                    onClick={() => void invite(p.id)}
                  >
                    <UserPlus className="size-3.5" />
                    邀请
                  </Button>
                </div>
              </li>
            ))}
            {available.length === 0 && (
              <li className="px-3 py-10 text-center text-caption text-muted-foreground">
                没有可邀请的联系人或 Agent
              </li>
            )}
          </ul>
        </ScrollArea>
        {error && (
          <div className="border-t border-border p-4">
            <p className="text-caption text-presence-failed">{error}</p>
          </div>
        )}
      </SheetContent>
    </Sheet>
  )
}
