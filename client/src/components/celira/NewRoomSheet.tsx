import { useState } from 'react'
import { Check, MessageSquarePlus, Users } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { cn } from '@/lib/utils'
import { participants } from '@/fixtures/data'
import { resolveSelfId } from '@/api/selfIdentity'
import { getApi, useAppStore, useRooms } from '@/api/store'
import { directRoomRouteFor, roomCandidates, supportsRoomCreation } from '@/api/uiPolicy'
import { roomActionErrorText } from '@/api/contractErrors'
import { AgentBadge } from './AgentBadge'
import { OfficialBadge } from './OfficialBadge'
import { ParticipantAvatar } from './ParticipantAvatar'

interface NewRoomSheetProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  onStart: (participantIds: string[]) => void
}

export function NewRoomSheet({ open, onOpenChange, onStart }: NewRoomSheetProps) {
  const [selected, setSelected] = useState<string[]>([])
  const [groupName, setGroupName] = useState('')
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // 真实建房的两条路（私聊 / 群聊）都在才走真实分支 —— 能力探测，不是模式表（TASK_72 ⑥）。
  const roomCreationSupported = supportsRoomCreation(getApi())
  const contacts = useAppStore((s) => s.contacts)
  const ownedAgents = useAppStore((s) => s.ownedAgents)
  const createHumanDirectRoom = useAppStore((s) => s.createHumanDirectRoom)
  const createDirectRoom = useAppStore((s) => s.createDirectRoom)
  const createGroup = useAppStore((s) => s.createGroup)
  const allRooms = useRooms()
  const me = useAppStore((s) => s.me)
  // 直聊房判定口径（与 ContactDetailCard 一致）：2 人房 + 含 selfId + 含该 agent。
  // TASK_56：判据收敛到 `resolveSelfId`（真实世界无身份时返空 ⇒ 找不到已有的
  // 直聊房、于是不去重；**不能**拿 `p-inori` 去撞真实 id 假装找到了）。
  const selfId = resolveSelfId(me?.participant.id)
  const existingDirectRoomId = (agentId: string): string | undefined =>
    allRooms.find(
      (r) =>
        r.participantIds.length === 2 &&
        r.participantIds.includes(selfId) &&
        r.participantIds.includes(agentId),
    )?.id

  // ⚠ **必须按 id 去重**（2026-09-29 真机实测）：同一位 Agent 可以**同时**是 accepted 联系人与
  // 自有 Agent（屋主给自己的 Agent 发过好友申请并接受了 —— 那是批次三允许的常态），
  // 直接拼两个数组会让它出现**两行同一个 id**（React key 也重复）。规则收在
  // `uiPolicy.roomCandidates`（纯函数、有 `node --test` 用例），这里不重写一遍。
  const candidates = roomCandidates(contacts, ownedAgents)

  const toggle = (id: string) => {
    // 换一个候选人 ⇒ 上一轮那句红字（「找不到这位用户」之类）不该继续挂着说这一轮的事。
    setError(null)
    setSelected((ids) => (ids.includes(id) ? ids.filter((x) => x !== id) : [...ids, id]))
  }

  const start = async () => {
    setCreating(true)
    setError(null)
    try {
      // TASK_72 ⑥ 收尾：原判据 `mode === 'core'` **恒真**（`ConnectionMode` 只有一个取值）
      // ⇒ 演示通道也会走「真实建房」这一支，而那条路上 `getApi()` 是 null：私聊/群聊会拿到
      // fixtures 的占位房号。改成能力探测（与 `revokeSupported` 同形）：那两条路真的在，
      // 才按真实数据（联系人 / 自有 Agent）建；否则走 fixtures 那条路。
      if (roomCreationSupported) {
        if (selected.length === 1) {
          // **按人选路，不按名单来源选路**（2026-09-29 真机实测修）：原先先
          // `contacts.find(...)` 再 `ownedAgents.find(...)`，于是"既是联系人又是我的 Agent"
          // 的那一支永远走 Human 那条路 ⇒ 404。判据只该有一个：**它是什么**。
          const target = candidates.find((p) => p.id === selected[0])
          if (target && directRoomRouteFor(target) === 'agent') {
            // 一间房 = 一段关系（Core 批次二）：同一对 Human–Agent 只有一间私人房，
            // 点它就是**回到你们那间**——已有房直接进，没有则由 Core 建首房（同一
            // 条路径、同一个结果：Core 对"同一对再要一次"回放原房）。
            const roomId = existingDirectRoomId(target.id) ?? (await createDirectRoom(target.id))
            onStart([roomId])
            reset()
            return
          }
          if (target) {
            const roomId = await createHumanDirectRoom(target.id)
            onStart([roomId])
            reset()
            return
          }
        }
        if (groupName.trim()) {
          const roomId = await createGroup(groupName.trim())
          onStart([roomId])
          reset()
          return
        }
        setError('单选联系人创建私聊、单选 Agent 发起直聊，或填写群名创建群聊')
        return
      }
      if (selected.length === 1) {
        const target = participants.find((p) => p.id === selected[0])
        if (target?.kind === 'agent') {
          // 同一口径（演示通道）：已有那间就进去，没有则建首房
          const roomId = existingDirectRoomId(target.id) ?? (await createDirectRoom(target.id))
          onStart([roomId])
          reset()
          return
        }
        const roomId = await createHumanDirectRoom(selected[0])
        onStart([roomId])
        reset()
        return
      }
      onStart(selected)
      reset()
    } catch (e) {
      setError(roomActionErrorText(e))
    } finally {
      setCreating(false)
    }
  }

  const reset = () => {
    setSelected([])
    setGroupName('')
    setError(null)
    onOpenChange(false)
  }

  const singleAgentSelected =
    selected.length === 1 &&
    candidates.find((p) => p.id === selected[0])?.kind === 'agent'

  const canStart = selected.length === 1 || groupName.trim().length > 0

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="w-full gap-0 p-0 sm:max-w-md">
        <SheetHeader className="border-b border-border">
          <SheetTitle>发起新会话</SheetTitle>
          <SheetDescription>选择联系人创建私聊、Agent 发起直聊，或命名一个群</SheetDescription>
        </SheetHeader>
        <ScrollArea className="min-h-0 flex-1 [&_[data-radix-scroll-area-viewport]>div]:!block">
          <ul className="space-y-0.5 p-3">
            {candidates.map((p) => {
              const checked = selected.includes(p.id)
              return (
                <li key={p.id}>
                  <button
                    type="button"
                    onClick={() => toggle(p.id)}
                    aria-pressed={checked}
                    // 真机门禁用它**按 id** 数候选行（`verify-ui.mjs`）：同名不同人不会误判，
                    // 同一人列两行则必然被抓到（2026-09-29 那条真机实测缺陷）。
                    data-candidate-id={p.id}
                    className={cn(
                      'flex w-full cursor-pointer items-center gap-3 rounded-lg px-3 py-3 text-left transition-colors',
                      checked ? 'bg-accent' : 'hover:bg-muted',
                    )}
                  >
                    <ParticipantAvatar participant={p} size="md" />
                    <span className="min-w-0 flex-1">
                      <span className="flex items-center gap-2">
                        <span
                          className={cn(
                            'truncate text-body',
                            p.kind === 'agent' && 'text-agent',
                          )}
                        >
                          {p.displayName}
                        </span>
                        {p.kind === 'agent' && (p.official ? <OfficialBadge /> : <AgentBadge />)}
                      </span>
                      <span className="text-caption text-muted-foreground">
                        {p.kind === 'agent' ? 'Agent' : 'Human'}
                      </span>
                    </span>
                    <span
                      className={cn(
                        'flex size-5 shrink-0 items-center justify-center rounded-full border border-border transition-colors',
                        checked && 'border-primary bg-primary text-primary-foreground',
                      )}
                    >
                      {checked && <Check className="size-3" />}
                    </span>
                  </button>
                </li>
              )
            })}
            {candidates.length === 0 && (
              <li className="px-3 py-10 text-center text-caption text-muted-foreground">
                还没有可邀请的联系人或 Agent
              </li>
            )}
          </ul>
        </ScrollArea>
        <div className="border-t border-border p-4">
          <div className="flex items-center gap-2">
            <Users className="size-4 shrink-0 text-muted-foreground" />
            <Input
              value={groupName}
              onChange={(e) => setGroupName(e.target.value)}
              placeholder="群聊名称（可选）"
              aria-label="群聊名称"
              className="bg-muted"
            />
          </div>
        </div>
        <div className="border-t border-border p-4">
          {error && <p className="mb-2 text-caption text-presence-failed">{error}</p>}
          <Button
            className="w-full gap-1.5"
            disabled={!canStart || creating}
            onClick={() => void start()}
          >
            <MessageSquarePlus className="size-4" />
            {creating
              ? '创建中…'
              : singleAgentSelected
                ? '发起直聊'
                : selected.length === 1
                  ? '发起直聊'
                  : '创建群聊'}
          </Button>
        </div>
      </SheetContent>
    </Sheet>
  )
}
