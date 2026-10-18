import type { Participant } from '@/api/domain'
import { mentionCandidates } from '@/api/mentionTargets'
import { useAppStore } from '@/api/store'
import { cn } from '@/lib/utils'
import { AgentBadge } from './AgentBadge'
import { OfficialBadge } from './OfficialBadge'
import { ParticipantAvatar } from './ParticipantAvatar'

interface MentionPopoverProps {
  /** 建议源：**由调用方注入**的成员投影（唯一调用方是 `MessageInput`，注入的是
   * `roomMemberViews` 的产出）。组件自己**不再**走"三选一"那条链——旧实现这里还有
   * 一份 `room.participants ?? room.participantIds.map(getParticipant)`，而那条兜底
   * 从来没有人用到（调用方始终传 `members`），却带着一颗"查不到就抛"的地雷。 */
  members: Participant[]
  query: string
  activeIndex: number
  onSelect: (participantId: string) => void
}

export function MentionPopover({ members: resolved, query, activeIndex, onSelect }: MentionPopoverProps) {
  const meId = useAppStore((s) => s.me?.participant.id)
  // 候选 = 房间成员里除自己之外、名字匹配 query 的成员，**agent 优先**；Human 与 Agent
  // 都可 @（契约 `CORE_V1_MENTION_TARGETS.md`：group 房 active 成员均可为目标，且都进
  // 结构化 mentions）。判据与 `MessageInput` 的候选计数 / 回车选择**共用同一个函数**，
  // 不再三处各写一遍（旧实现里建议列表与回车选择是两份独立筛选）。
  // 行渲染仍用完整成员投影（头像 / 徽章要更多字段），故按 id 取回原条目。
  const byId = new Map(resolved.map((p) => [p.id, p]))
  const candidates = mentionCandidates(
    resolved.map((p) => ({ id: p.id, name: p.displayName, kind: p.kind })),
    { meId, query },
  )
    .map((m) => byId.get(m.id))
    .filter((p): p is Participant => p != null)

  if (candidates.length === 0) {
    return (
      <div className="absolute bottom-full left-0 right-0 mb-2 rounded-xl border border-border bg-popover px-4 py-3 text-caption text-muted-foreground shadow-lg">
        没有匹配的参与者
      </div>
    )
  }

  return (
    <ul className="absolute bottom-full left-0 right-0 mb-2 space-y-0.5 rounded-xl border border-border bg-popover p-1.5 shadow-lg">
      {candidates.map((p, i) => (
        <li key={p.id}>
          <button
            type="button"
            onMouseDown={(e) => {
              e.preventDefault()
              onSelect(p.id)
            }}
            className={cn(
              'flex w-full cursor-pointer items-center gap-2.5 rounded-lg px-2.5 py-2 text-left transition-colors',
              i === activeIndex ? 'bg-accent' : 'hover:bg-muted',
            )}
          >
            <ParticipantAvatar participant={p} size="sm" />
            <span className={cn('truncate text-body', p.kind === 'agent' && 'text-agent')}>
              {p.displayName}
            </span>
            {p.kind === 'agent' && (p.official ? <OfficialBadge /> : <AgentBadge />)}
          </button>
        </li>
      ))}
    </ul>
  )
}
