import { useEffect, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { MessageSquare, Plus } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import type { Message, Room } from '@/api/domain'
import { useAppStore, useConnection } from '@/api/store'
import { CeliraLogo } from './CeliraLogo'
import { ConversationItem } from './ConversationItem'
import { tryGetParticipant } from '@/api/participantDirectory'
import { resolveSelfId } from '@/api/selfIdentity'

interface ConversationListProps {
  rooms: Room[]
  activeRoomId: string
  onSelectRoom: (roomId: string) => void
  onCreateRoom?: () => void
}

export function ConversationList({ rooms, activeRoomId, onSelectRoom, onCreateRoom }: ConversationListProps) {
  const [query, setQuery] = useState('')
  const [searchFocused, setSearchFocused] = useState(false)
  const { me } = useConnection()
  const trimmed = query.trim().toLowerCase()
  const messagesByRoom = useAppStore((s) => s.messagesByRoom)
  // Agent 直聊房聚合为一行（列表序 = 活跃序，首次出现即最近）；Human/群聊保持一行一房间。
  // 一间房 = 一段关系（Core 批次二）之后，同一对的**私人房至多一间**：聚合在这里
  // 通常是一对一，剩下的用处是把跨关系周期的服务房收成同一行。
  const items = useMemo(() => {
    const agentGroups = new Map<string, Room[]>()
    const list: { key: string; room: Room; groupRooms?: Room[] }[] = []
    for (const r of rooms) {
      const peer = r.peer
      if (peer?.kind === 'agent' && r.participantIds.length === 2) {
        const g = agentGroups.get(peer.id)
        if (g) {
          g.push(r)
          continue
        }
        const ng = [r]
        agentGroups.set(peer.id, ng)
        list.push({ key: `agent-${peer.id}`, room: r, groupRooms: ng })
      } else {
        list.push({ key: r.id, room: r })
      }
    }
    return list
  }, [rooms])
  const filtered = items
    .filter((it) => {
      const name = it.groupRooms ? (it.room.peer?.displayName ?? it.room.name) : it.room.name
      const preview = it.groupRooms
        ? (it.groupRooms[0]?.lastMessagePreview ?? '')
        : it.room.lastMessagePreview
      return (
        name.toLowerCase().includes(trimmed) || preview.toLowerCase().includes(trimmed)
      )
    })
    .sort(
      (a, b) =>
        Number(Boolean(b.room.pinned) && !b.groupRooms) - Number(Boolean(a.room.pinned) && !a.groupRooms),
    )
  // 「有人@我」：未读窗口（最近 unreadCount 条已加载消息）内存在 mentions 含我的
  // 消息即为真。未加载的更早消息无从判断，宁可不显示也不虚构。
  // TASK_56：判据收敛到 `resolveSelfId` —— 真实世界无身份时它返空，
  // 于是"这条 @"是不是给我的"如实变成"不知道"，不再拿 fixture 身份顶替。
  const mentionedRoomIds = useMemo(() => {
    const selfId = resolveSelfId(me?.participant?.id)
    const flagged = new Set<string>()
    for (const r of rooms) {
      if (!r.unreadCount) continue
      const msgs = messagesByRoom[r.id]
      if (!msgs || msgs.length === 0) continue
      const unreadWindow = msgs.slice(-r.unreadCount)
      if (unreadWindow.some((m) => !m.system && m.mentions?.includes(selfId))) flagged.add(r.id)
    }
    return flagged
  }, [rooms, messagesByRoom, me])
  const scrollRef = useRef<HTMLDivElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)
  const rowVirtualizer = useVirtualizer({
    count: filtered.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 72,
    overscan: 8,
  })
  const messageHits = useMemo(() => {
    if (trimmed.length < 2) return []
    return Object.values(messagesByRoom)
      .flat()
      .filter((m) => !m.system && m.body.toLowerCase().includes(trimmed))
      .slice(-60)
      .reverse()
      .slice(0, 20)
  }, [messagesByRoom, trimmed])
  const showMessageResults = searchFocused && trimmed.length >= 2 && messageHits.length > 0

  const jumpToMessage = (roomId: string, messageId: string) => {
    window.dispatchEvent(new CustomEvent('celira:focus-message', { detail: { roomId, messageId } }))
  }

  useEffect(() => {
    const onFocusSearch = () => searchRef.current?.focus()
    window.addEventListener('celira:focus-search', onFocusSearch)
    return () => window.removeEventListener('celira:focus-search', onFocusSearch)
  }, [])

  return (
    <aside className="flex h-full w-full flex-col border-r border-border bg-card md:w-72 md:shrink-0">
      <div className="flex items-center gap-3 border-b border-border px-5 py-4">
        <CeliraLogo className="size-7 md:hidden" />
        <span className="text-title tracking-wide md:hidden">Celira</span>
        <span className="hidden text-title tracking-wide md:block">消息</span>
        <Button
          variant="ghost"
          size="icon"
          className="ml-auto"
          onClick={onCreateRoom}
          aria-label="发起新会话"
        >
          <Plus className="size-4" />
        </Button>
      </div>
      <div className="px-4 py-3">
        <Input
          ref={searchRef}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onFocus={() => setSearchFocused(true)}
          onBlur={() => window.setTimeout(() => setSearchFocused(false), 150)}
          onKeyDown={(e) => {
            if (e.key === 'Escape') setQuery('')
          }}
          placeholder="搜索会话或消息…"
          className="bg-muted border-transparent focus-visible:border-input"
        />
      </div>
      <div ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto">
        {showMessageResults ? (
          <div className="px-2 py-1">
            <div className="px-3 py-2 text-micro tracking-wider text-muted-foreground uppercase">
              消息（已加载范围）
            </div>
            <ul className="space-y-0.5">
              {messageHits.map((m) => (
                <MessageHitRow key={m.id} message={m} rooms={rooms} onJump={jumpToMessage} />
              ))}
            </ul>
          </div>
        ) : filtered.length > 0 ? (
          <div style={{ height: rowVirtualizer.getTotalSize(), position: 'relative' }}>
            {rowVirtualizer.getVirtualItems().map((vi) => {
              const item = filtered[vi.index]
              return (
                <div
                  key={item.key}
                  data-index={vi.index}
                  ref={rowVirtualizer.measureElement}
                  style={{
                    position: 'absolute',
                    top: 0,
                    left: 0,
                    width: '100%',
                    transform: `translateY(${vi.start}px)`,
                  }}
                >
                  <div className="px-2 pb-1">
                    <ConversationItem
                      room={item.room}
                      groupRooms={item.groupRooms}
                      mentioned={
                        item.groupRooms
                          ? item.groupRooms.some((r) => mentionedRoomIds.has(r.id))
                          : mentionedRoomIds.has(item.room.id)
                      }
                      active={
                        item.groupRooms
                          ? item.groupRooms.some((r) => r.id === activeRoomId)
                          : item.room.id === activeRoomId
                      }
                      onSelect={onSelectRoom}
                    />
                  </div>
                </div>
              )
            })}
          </div>
        ) : (
          <div className="px-3 py-10 text-center text-caption text-muted-foreground">
            {trimmed ? '没有匹配的会话或消息' : '没有会话'}
          </div>
        )}
      </div>
    </aside>
  )
}

function MessageHitRow({
  message,
  rooms,
  onJump,
}: {
  message: Message
  rooms: Room[]
  onJump: (roomId: string, messageId: string) => void
}) {
  const room = rooms.find((r) => r.id === message.roomId)
  const author = tryGetParticipant(message.authorId)
  return (
    <li>
      <button
        type="button"
        onClick={() => onJump(message.roomId, message.id)}
        className="flex w-full cursor-pointer items-center gap-2.5 rounded-lg px-3 py-2 text-left transition-colors hover:bg-muted"
      >
        <MessageSquare className="size-3.5 shrink-0 text-muted-foreground" />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-caption text-muted-foreground">
            {room?.name ?? '会话'} · {author?.displayName ?? '参与者'}
          </span>
          <span className="block truncate text-body">{message.body.slice(0, 60)}</span>
        </span>
      </button>
    </li>
  )
}
