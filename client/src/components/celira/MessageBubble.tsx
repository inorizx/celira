import { memo } from 'react'
import { Clock, AlertCircle } from 'lucide-react'
import { useNavigate } from 'react-router'
import { cn } from '@/lib/utils'
import { tryGetParticipant } from '@/api/participantDirectory'
import type { DeliveryState, Message } from '@/api/domain'
import { isSelfMessage, resolveSelfId } from '@/api/selfIdentity'
import { neutralParticipant } from '@/api/uiPolicy'
import { ParticipantAvatar } from './ParticipantAvatar'
import { AgentBadge } from './AgentBadge'
import { OfficialBadge } from './OfficialBadge'
import { ReplyQuote, type ReplyTargetState } from './ReplyQuote'
import { MarkdownText } from './MarkdownText'
import type { MentionMember } from '@/api/mentionTargets'
import { MessageContextMenu, type AuthorResolver } from './MessageContextMenu'
import { FileRow } from './FileRow'
import { AttachmentImage } from './AttachmentImage'
import { AudioAttachment } from './AudioAttachment'
import { VideoAttachment } from './VideoAttachment'
import { attachmentMedia } from '@/lib/attachmentMedia'

function DeliveryStatus({
  state,
  onRetry,
}: {
  state: DeliveryState
  onRetry?: () => void
}) {
  if (state === 'sending') {
    return (
      <span className="inline-flex items-center gap-1 text-caption text-muted-foreground">
        <Clock className="size-3 animate-pulse" /> 发送中
      </span>
    )
  }
  if (state === 'failed') {
    // **重试是真重试**（2026-09-25 补）：`store.retry` 复用首次那把 Idempotency-Key，
    // 契约 §6.3 保证同 key 同 payload 会重放**原** Message（200，`idempotency_replayed`）
    // ⇒ 即使首次其实已落库、只是响应丢了，这里也不会产生第二条公共消息。
    return (
      <button
        type="button"
        onClick={onRetry}
        aria-label="发送失败，点击重试"
        className="inline-flex cursor-pointer items-center gap-1 text-caption text-presence-failed transition-opacity hover:opacity-80"
      >
        <AlertCircle className="size-3" /> 发送失败，点击重试
      </button>
    )
  }
  // 系统没有「对端已确认」这个事实（Completion/ACK 已退役），所以 delivered 态
  // **什么都不渲染** —— 不给一个永远不会变绿的假回执灯。
  return null
}

export function MessageBubble({
  message,
  selfId,
  compact = false,
  onReply,
  onRecall,
  onRetry,
  onPreviewImage,
  onJumpToMessage,
  onForward,
  onHide,
  onSelectMode,
  selectionMode,
  selected,
  onToggleSelect,
  resolveReplyTarget,
  onLoadReplyTarget,
  resolveAuthor,
  mentionMembers,
  highlighted = false,
}: {
  message: Message
  selfId?: string
  compact?: boolean
  onReply?: (message: Message) => void
  onRecall?: (message: Message) => void
  onRetry?: () => void
  onPreviewImage?: (attachmentId: string) => void
  onJumpToMessage?: (messageId: string) => void
  onForward?: (message: Message) => void
  /** 「删除 · 仅为我」：**本机隐藏**（不是撤回 —— 撤回是公共动作，归 Core）。 */
  onHide?: (message: Message) => void
  /** 进入**多选**（把这一条当第一项）。 */
  onSelectMode?: (message: Message) => void
  /** 已在多选态：气泡上点一下 = 勾选/取消。 */
  selectionMode?: boolean
  selected?: boolean
  onToggleSelect?: () => void
  /** ReplyQuote 的目标解析器（引用当前已加载的权威 timeline）。 */
  resolveReplyTarget?: (messageId: string) => ReplyTargetState
  /** 目标未加载时按 opaque `before` 分页加载更早消息。 */
  onLoadReplyTarget?: (messageId: string) => void
  /** 真实作者解析器（真实世界必须传入 me + room.participants 投影；缺省回退
   * fixtures Mock/Gallery）。作者 name/kind/isAgent/self 归属与「查看主页」
   * 门禁全部经此解析，真实世界绝不猜未知作者。 */
  resolveAuthor?: AuthorResolver
  /** 房间成员名快照（agent/human）：@ 渲染高亮用（真实成员投影，缺省回退
   * fixtures）。 */
  mentionMembers?: MentionMember[]
  highlighted?: boolean
}) {
  const navigate = useNavigate()
  // 作者投影：调用方注入的解析器优先（真实 me + room.participants，
  // 未知作者 → 中性「参与者」，不猜 human/agent）；缺省回退 fixtures。
  const author = resolveAuthor
    ? (resolveAuthor(message.authorId) ?? neutralParticipant(message.authorId))
    : (tryGetParticipant(message.authorId) ?? neutralParticipant(message.authorId))
  const isSelf = isSelfMessage(author.id, selfId)
  const self = resolveSelfId(selfId)
  const isAgent = author.kind === 'agent'
  // 附件的显示归类**只有一处判据**（`lib/attachmentMedia.ts`）：图片、语音、视频各有
  // 自己的渲染件，其余保持文件卡片。别在这里再写一遍 mime 前缀判断（那就是第二处判据）。
  const attachments = message.attachments ?? []
  const images = attachments.filter((a) => attachmentMedia(a) === 'image')
  const videos = attachments.filter((a) => attachmentMedia(a) === 'video')
  const audioFiles = attachments.filter((a) => attachmentMedia(a) === 'audio')
  const plainFiles = attachments.filter((a) => attachmentMedia(a) === 'file')
  // 回复/撤回只对**已提交**（非发送中/失败 pending）的消息开放：发送中/失败的
  // 临时消息不得成为真实 reply target，也不得被撤回（无 Core id）。
  const committed = message.delivery === 'delivered'
  const replyAction = committed ? onReply : undefined
  const recallAction = committed ? onRecall : undefined

  return (
    <div
      className={cn(
        'group/msgline flex max-w-[85%] gap-3 md:max-w-[70%]',
        isSelf && 'ml-auto flex-row-reverse',
      )}
    >
      {compact ? (
        <div className="flex w-9 shrink-0 items-start justify-center pt-1.5">
          <span className="text-micro text-muted-foreground opacity-0 transition-opacity group-hover/msgline:opacity-100">
            {message.sentAt}
          </span>
        </div>
      ) : (
        <button
          type="button"
          onClick={() => isAgent && navigate(`/agent/${author.id}`)}
          aria-label={isAgent ? `查看 ${author.displayName} 的主页` : undefined}
          className={cn('shrink-0 self-start rounded-full', isAgent && 'cursor-pointer')}
        >
          <ParticipantAvatar participant={author} size="md" />
        </button>
      )}
      <div className={cn(isSelf && 'text-right')}>
        {!compact && (
          <div className={cn('mb-1 flex items-baseline gap-2', isSelf && 'justify-end')}>
            {isSelf && <span className="text-caption text-muted-foreground">{message.sentAt}</span>}
            <span className={cn('text-title', isAgent ? 'text-agent' : isSelf ? 'text-foreground' : 'text-accent-foreground')}>
              {author.displayName}
            </span>
            {isAgent && (author.official ? <OfficialBadge /> : <AgentBadge />)}
            {!isSelf && <span className="text-caption text-muted-foreground">{message.sentAt}</span>}
          </div>
        )}
        <div className={cn('inline-block', isSelf && 'text-left')}>
          <MessageContextMenu
            message={message}
            selfId={self}
            resolveAuthor={resolveAuthor}
            onReply={replyAction}
            onRecall={recallAction}
            onForward={onForward}
            onHide={onHide}
            onSelectMode={onSelectMode}
            selectionMode={selectionMode}
            selected={selected}
            onToggleSelect={onToggleSelect}
          >
            <div
              className={cn(
                'flex flex-col gap-3 rounded-2xl',
                highlighted && 'ring-2 ring-ring ring-offset-2 ring-offset-background',
              )}
            >
              {images.length === 1 && (
                <AttachmentImage
                  attachment={images[0]}
                  roomId={message.roomId}
                  alt={images[0].name}
                  imgLabel={`预览图片 ${images[0].name}`}
                  onClick={() => onPreviewImage?.(images[0].id)}
                  className="max-w-56 cursor-pointer self-start rounded-xl border border-border transition-opacity hover:opacity-85 md:max-w-72"
                />
              )}
              {images.length > 1 && (
                <div
                  className={cn(
                    'grid max-w-56 gap-1 self-start md:max-w-72',
                    images.length === 4 ? 'grid-cols-2' : 'grid-cols-3',
                  )}
                >
                  {images.map((a) => (
                    <AttachmentImage
                      key={a.id}
                      attachment={a}
                      roomId={message.roomId}
                      alt={a.name}
                      imgLabel={`预览图片 ${a.name}`}
                      onClick={() => onPreviewImage?.(a.id)}
                      className="aspect-square w-full cursor-pointer rounded-lg border border-border object-cover transition-opacity hover:opacity-85"
                    />
                  ))}
                </div>
              )}
              {videos.map((a) => (
                <VideoAttachment key={a.id} attachment={a} roomId={message.roomId} />
              ))}
              {!message.body &&
                audioFiles.map((a) => (
                  <div
                    key={a.id}
                    className="flex w-40 items-center gap-2.5 self-start rounded-xl border border-border bg-card px-3 py-2.5"
                  >
                    <AudioAttachment attachment={a} roomId={message.roomId} />
                  </div>
                ))}
              {!message.body &&
                plainFiles.map((a) => (
                  <div
                    key={a.id}
                    className="flex w-56 items-center gap-2.5 self-start rounded-xl border border-border bg-card px-3 py-2.5"
                  >
                    <FileRow attachment={a} roomId={message.roomId} />
                  </div>
                ))}
              {message.body && (
                <div
                  className={cn(
                    'self-start px-4 py-3 text-left text-body',
                    compact ? 'rounded-xl' : 'rounded-2xl',
                    (audioFiles.length > 0 || plainFiles.length > 0) && 'min-w-56',
                    isSelf
                      ? 'rounded-tr-sm bg-bubble-self-from text-bubble-self-foreground'
                      : 'rounded-tl-sm bg-bubble-peer text-bubble-peer-foreground border border-border',
                  )}
                >
                  {audioFiles.map((a) => (
                    <div
                      key={a.id}
                      className={cn(
                        'mb-2 flex items-center gap-2.5 rounded-lg px-2.5 py-2',
                        isSelf ? 'bg-bubble-self-foreground/15' : 'bg-muted',
                      )}
                    >
                      <AudioAttachment attachment={a} roomId={message.roomId} onSelfBubble={isSelf} />
                    </div>
                  ))}
                  {plainFiles.map((a) => (
                    <div
                      key={a.id}
                      className={cn(
                        'mb-2 flex items-center gap-2.5 rounded-lg px-2.5 py-2',
                        isSelf ? 'bg-bubble-self-foreground/15' : 'bg-muted',
                      )}
                    >
                      <FileRow attachment={a} roomId={message.roomId} onSelfBubble={isSelf} />
                    </div>
                  ))}
                  {message.replyToId && (
                    <ReplyQuote
                      replyToId={message.replyToId}
                      onSelfBubble={isSelf}
                      onJump={onJumpToMessage}
                      resolveTarget={resolveReplyTarget ?? (() => ({ status: 'unloaded' as const }))}
                      onLoadOlder={onLoadReplyTarget}
                    />
                  )}
                  <MarkdownText
                    text={message.body}
                    onSelf={isSelf}
                    mentions={message.mentions}
                    // 结构化目标按 **id** 解析成成员（名字 + 种类）：目标离开房间后仍按
                    // 它自己的种类上色，绝不因为"名表里没有"就当成 Agent。
                    resolveMention={
                      resolveAuthor
                        ? (id) => {
                            const p = resolveAuthor(id)
                            return p ? { id: p.id, name: p.displayName, kind: p.kind } : undefined
                          }
                        : undefined
                    }
                    mentionMembers={mentionMembers}
                  />
                </div>
              )}
            </div>
          </MessageContextMenu>
        </div>
        {isSelf && (
          <div className="mt-1">
            <DeliveryStatus state={message.delivery} onRetry={onRetry} />
          </div>
        )}
      </div>
    </div>
  )
}

export const MemoizedMessageBubble = memo(MessageBubble, (prev, next) => {
  return (
    prev.message === next.message &&
    prev.compact === next.compact &&
    prev.highlighted === next.highlighted &&
    prev.selfId === next.selfId &&
    prev.onReply === next.onReply &&
    prev.onRecall === next.onRecall &&
    prev.resolveReplyTarget === next.resolveReplyTarget &&
    prev.onLoadReplyTarget === next.onLoadReplyTarget &&
    prev.resolveAuthor === next.resolveAuthor
  )
})
