import { BadgeCheck, BellOff } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { Room } from '@/api/domain'
import { ParticipantAvatar } from './ParticipantAvatar'
import { AgentBadge } from './AgentBadge'
import { ConversationContextMenu } from './ConversationContextMenu'

interface ConversationItemProps {
  room: Room
  active?: boolean
  onSelect?: (roomId: string) => void
  // Agent 聚合行：同一 Agent 的全部直聊房（列表序 = 活跃序，首个即最近）。
  // 一间房 = 一段关系（Core 批次二）之后，一对 Human–Agent 的**私人房至多一间**，
  // 所以聚合行在常见情形下就一行一房；它仍然有用的是**跨关系周期**的服务房。
  groupRooms?: Room[]
  // 未读窗口内有 @ 我的消息（由列表层按真实 mentions 计算）
  mentioned?: boolean
}

// 会话行解剖（DESIGN_SYSTEM.md「会话行配方」）：
// 名字区 = 头像 + 名称 + inline 身份标记；右侧上槽永远时间、下槽永远动态状态（未读/免打扰）
export function ConversationItem({ room, active = false, onSelect, groupRooms, mentioned = false }: ConversationItemProps) {
  const counterpart = room.peer ?? null
  const isAggregate = groupRooms != null && groupRooms.length > 0

  const titled =
    !isAggregate &&
    counterpart != null &&
    room.participantIds.length === 2 &&
    room.name !== counterpart.displayName
  // 双前缀去重：预览自带的「作者：」与对端名相同时不重复拼接
  const stripAuthorPrefix = (preview: string, authorName: string) =>
    preview.startsWith(`${authorName}：`) ? preview.slice(authorName.length + 1) : preview

  const name = isAggregate && counterpart ? counterpart.displayName : room.name
  const unreadCount = isAggregate
    ? groupRooms.reduce((sum, r) => sum + r.unreadCount, 0)
    : room.unreadCount
  const lastActivityAt = isAggregate ? (groupRooms[0]?.lastActivityAt ?? '') : room.lastActivityAt

  let previewText: string
  if (isAggregate && counterpart) {
    // 聚合行：副行 = 最近那间房的预览（去掉作者前缀）+ 房间数
    const first = groupRooms[0]
    const p = first ? stripAuthorPrefix(first.lastMessagePreview ?? '', counterpart.displayName) : ''
    previewText = (p || '还没有消息') + (groupRooms.length > 1 ? ` · ${groupRooms.length} 个会话` : '')
  } else {
    const rawPreview = room.lastMessagePreview ?? ''
    const strippedPreview =
      titled && counterpart ? stripAuthorPrefix(rawPreview, counterpart.displayName) : rawPreview
    previewText = titled
      ? strippedPreview
        ? `${counterpart.displayName} · ${strippedPreview}`
        : counterpart.displayName
      : rawPreview
  }

  const targetRoomId = isAggregate ? groupRooms[0].id : room.id

  const row = (
    <button
      type="button"
      onClick={() => onSelect?.(targetRoomId)}
      aria-label={`${name}${unreadCount > 0 ? `，${unreadCount > 99 ? '99+' : unreadCount} 条未读` : ''}${room.pinned ? '，已置顶' : ''}${room.muted ? '，已免打扰' : ''}`}
      aria-current={active ? 'true' : undefined}
      className={cn(
        'flex w-full cursor-pointer items-center gap-3 rounded-lg px-3 py-3 text-left transition-colors',
        active ? 'bg-accent' : room.pinned ? 'bg-muted/60 hover:bg-muted' : 'hover:bg-muted',
      )}
    >
      {counterpart ? (
        <ParticipantAvatar participant={counterpart} />
      ) : (
        <div className="flex size-10 shrink-0 items-center justify-center rounded-full bg-gradient-to-br from-bubble-self-from to-bubble-self-to text-sm font-medium text-bubble-self-foreground">
          {room.avatarLabel ?? name.slice(0, 1)}
        </div>
      )}
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline justify-between gap-2">
          <span className="flex min-w-0 items-baseline gap-1.5">
            <span className="truncate text-title">{name}</span>
            {/* 官方 = 信任标记：inline 小勾；AGENT pill = 种类标记（Agent 聚合行与无标题直聊保留） */}
            {counterpart?.official && (
              <BadgeCheck className="size-3.5 shrink-0 self-center text-primary" aria-label="官方" />
            )}
            {counterpart?.kind === 'agent' && !counterpart.official && (isAggregate || !titled) && (
              <AgentBadge />
            )}
          </span>
          <span className={cn('shrink-0 text-caption', active ? 'text-primary' : 'text-muted-foreground')}>
            {lastActivityAt}
          </span>
        </div>
        <div className="flex items-center gap-1.5">
          <div className="min-w-0 flex-1 truncate text-caption text-muted-foreground">
            {mentioned && unreadCount > 0 && (
              <span className="text-destructive">[有人@我] </span>
            )}
            {previewText}
          </div>
          {room.muted && <BellOff className="size-3 shrink-0 text-muted-foreground" aria-label="已免打扰" />}
          {unreadCount > 0 &&
            (room.muted ? (
              <span className="size-2 shrink-0 rounded-full bg-muted-foreground/50" aria-hidden />
            ) : (
              <span className="flex h-[18px] min-w-[18px] shrink-0 items-center justify-center rounded-full bg-primary px-1 text-micro text-primary-foreground">
                {unreadCount > 99 ? '99+' : unreadCount}
              </span>
            ))}
        </div>
      </div>
    </button>
  )

  // 聚合行没有单房间语义，不挂置顶/免打扰上下文菜单
  if (isAggregate) return row
  return <ConversationContextMenu room={room}>{row}</ConversationContextMenu>
}
