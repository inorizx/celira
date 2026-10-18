import { useRef, type ReactNode } from 'react'
import { Bell, BellOff, Pin, PinOff } from 'lucide-react'
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from '@/components/ui/context-menu'
import type { Room } from '@/api/domain'
import { useAppStore } from '@/api/store'

interface ConversationContextMenuProps {
  room: Room
  children: ReactNode
}

/**
 * 会话行右键菜单。
 *
 * **置顶 / 免打扰**：用户侧偏好，只写本机存储（`api/roomPrefs.ts` 经 `api/storage.ts` 窄口），
 * 不发任何请求 —— 见那个文件头部为什么它们**不该**问 Core。
 *
 * ⚠ 这里治过一次真实的静默失效（2026-09-25）：这两个动作原先问
 * `ChatApi.togglePinned` / `toggleMuted`——**两个零实现的方法**（只服务早已不在工作树里的
 * `api/mock.ts`）⇒ 点了没有任何反应。根因不是"Core 缺端点所以做不了"，是**问错了地方**：
 * 用户自己的看法本来就归用户侧存储。
 *
 * ⚠ **「删除会话」不在这一档**：房间是公共事实，删除它需要 Core 端点，而 Core v1 没有、
 * 架构红线也明写「不引入房间删除/状态/改名」⇒ 那一项已从菜单移除（原先它调一条空动作
 * 之后还把用户导航走，会话**看起来**被删了、刷新又回来）。退出房间走详情抽屉的
 * 「离开房间」——那是 Core 真有的 `leave`。
 */
export function ConversationContextMenu({ room, children }: ConversationContextMenuProps) {
  const togglePinned = useAppStore((s) => s.togglePinned)
  const toggleMuted = useAppStore((s) => s.toggleMuted)
  const pressTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const touchPoint = useRef({ x: 0, y: 0 })
  const rowRef = useRef<HTMLDivElement>(null)

  const startPress = (e: React.TouchEvent) => {
    const touch = e.touches[0]
    touchPoint.current = { x: touch.clientX, y: touch.clientY }
    pressTimer.current = setTimeout(() => {
      rowRef.current?.dispatchEvent(
        new MouseEvent('contextmenu', {
          bubbles: true,
          cancelable: true,
          clientX: touchPoint.current.x,
          clientY: touchPoint.current.y,
        }),
      )
    }, 500)
  }
  const cancelPress = () => {
    if (pressTimer.current) clearTimeout(pressTimer.current)
    pressTimer.current = null
  }

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          ref={rowRef}
          onTouchStart={startPress}
          onTouchEnd={cancelPress}
          onTouchMove={cancelPress}
        >
          {children}
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent className="min-w-36">
        <ContextMenuItem onSelect={() => void togglePinned(room.id)}>
          {room.pinned ? <PinOff className="size-3.5" /> : <Pin className="size-3.5" />}
          {room.pinned ? '取消置顶' : '置顶'}
        </ContextMenuItem>
        <ContextMenuItem onSelect={() => void toggleMuted(room.id)}>
          {room.muted ? <Bell className="size-3.5" /> : <BellOff className="size-3.5" />}
          {room.muted ? '取消免打扰' : '免打扰'}
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  )
}
