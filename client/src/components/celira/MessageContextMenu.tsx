import { useRef, useState, type ReactNode } from 'react'
import { useNavigate } from 'react-router'
import { CheckSquare, Copy, Forward, Reply, Trash2, Undo2, UserRound } from 'lucide-react'
import { cn } from '@/lib/utils'
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from '@/components/ui/context-menu'
import { tryGetParticipant } from '@/api/participantDirectory'
import type { Message, Participant } from '@/api/domain'
import { isSelfMessage } from '@/api/selfIdentity'
import { neutralParticipant } from '@/api/uiPolicy'

/** 作者解析器：由调用方从当前连接的 `me + room.participants` 真实投影解析。
 * 返回 undefined 表示未知作者 → 显示中性「参与者」，绝不猜 human/agent。 */
export type AuthorResolver = (authorId: string) => Participant | undefined

interface MessageContextMenuProps {
  message: Message
  selfId?: string
  /** 真实作者解析器（真实世界必须传入；缺省时回退 fixtures Mock/Gallery）。 */
  resolveAuthor?: AuthorResolver
  onReply?: (message: Message) => void
  onRecall?: (message: Message) => void
  onForward?: (message: Message) => void
  /** 「删除 · 仅为我」：**本机隐藏**那条消息（不是撤回 —— 撤回是公共动作）。 */
  onHide?: (message: Message) => void
  /** 进入**多选**，并把这一条当作第一项选中。 */
  onSelectMode?: (message: Message) => void
  /** 已在多选态：点气泡 = 勾选/取消（这一下不再触发别的动作）。 */
  selectionMode?: boolean
  selected?: boolean
  onToggleSelect?: () => void
  children: ReactNode
}

export function MessageContextMenu({
  message,
  selfId,
  resolveAuthor,
  onReply,
  onRecall,
  onForward,
  onHide,
  onSelectMode,
  selectionMode,
  selected,
  onToggleSelect,
  children,
}: MessageContextMenuProps) {
  const navigate = useNavigate()
  // 作者投影：优先用调用方注入的真实解析器（真实世界绝不查 fixtures）；
  // 缺省（Gallery/Mock）保持既有 fixtures 回退。
  const author = resolveAuthor
    ? (resolveAuthor(message.authorId) ?? neutralParticipant(message.authorId))
    : (tryGetParticipant(message.authorId) ?? neutralParticipant(message.authorId))
  const isSelf = isSelfMessage(author.id, selfId)
  const isAgent = author.kind === 'agent'
  const [copied, setCopied] = useState(false)
  const pressTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const touchPoint = useRef({ x: 0, y: 0 })
  const bubbleRef = useRef<HTMLDivElement>(null)

  const startPress = (e: React.TouchEvent) => {
    const touch = e.touches[0]
    touchPoint.current = { x: touch.clientX, y: touch.clientY }
    pressTimer.current = setTimeout(() => {
      bubbleRef.current?.dispatchEvent(
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

  const copyBody = async () => {
    try {
      await navigator.clipboard.writeText(message.body)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      setCopied(false)
    }
  }

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>
        <div
          ref={bubbleRef}
          onTouchStart={startPress}
          onTouchEnd={cancelPress}
          onTouchMove={cancelPress}
          // 多选态里，点一下 = 勾选/取消（**不再触发别的动作**）；平时这一下不管事。
          onClick={
            selectionMode
              ? (event) => {
                  event.preventDefault()
                  event.stopPropagation()
                  onToggleSelect?.()
                }
              : undefined
          }
          data-message-id={message.id}
          data-selected={selected ? 'true' : undefined}
          className={cn(
            'inline-block',
            selectionMode && 'cursor-pointer rounded-2xl transition-shadow',
            selected && 'ring-2 ring-ring ring-offset-2 ring-offset-background',
          )}
        >
          {children}
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent className="min-w-36">
        <ContextMenuItem onSelect={copyBody}>
          <Copy className="size-3.5" />
          {copied ? '已复制' : '复制'}
        </ContextMenuItem>
        {typeof onSelectMode === 'function' && (
          <ContextMenuItem onSelect={() => onSelectMode(message)}>
            <CheckSquare className="size-3.5" />
            选择
          </ContextMenuItem>
        )}
        {typeof onReply === 'function' && (
          <ContextMenuItem onSelect={() => onReply(message)}>
            <Reply className="size-3.5" />
            回复
          </ContextMenuItem>
        )}
        {typeof onForward === 'function' && message.body && (
          <ContextMenuItem onSelect={() => onForward(message)}>
            <Forward className="size-3.5" />
            转发
          </ContextMenuItem>
        )}
        {isAgent && (
          <ContextMenuItem onSelect={() => navigate(`/agent/${author.id}`)}>
            <UserRound className="size-3.5" />
            查看主页
          </ContextMenuItem>
        )}
        {typeof onRecall === 'function' && isSelf && (
          <>
            <ContextMenuSeparator />
            <ContextMenuItem variant="destructive" onSelect={() => onRecall(message)}>
              <Undo2 className="size-3.5" />
              撤回
            </ContextMenuItem>
          </>
        )}
        {/*
          「删除 · 仅为我」（2026-10-08）：**本机隐藏**，与上面的「撤回」是两件事 ——
          撤回是**公共动作**（对面也看不见了，归 Core），这个是**只在你这边**。
          名字里带「仅为我」就是为了不让这两件事在同一个菜单里被读成一件。
        */}
        {typeof onHide === 'function' && (
          <>
            <ContextMenuSeparator />
            <ContextMenuItem variant="destructive" onSelect={() => onHide(message)}>
              <Trash2 className="size-3.5" />
              删除 · 仅为我
            </ContextMenuItem>
          </>
        )}
      </ContextMenuContent>
    </ContextMenu>
  )
}
