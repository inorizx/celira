import { useState } from 'react'
import { SendHorizonal } from 'lucide-react'
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
import type { Attachment, Message } from '@/api/domain'
import { getApi, useAppStore, useRooms } from '@/api/store'
import { forwardMessage } from '@/lib/forwardMessage'
import { ParticipantAvatar } from './ParticipantAvatar'
import { AgentBadge } from './AgentBadge'
import { OfficialBadge } from './OfficialBadge'

interface ForwardSheetProps {
  open: boolean
  sourceRoomId: string
  /** 要转发的那些消息（**按时间线**排好——顺序就是"对面读到的顺序"）。空数组 = 没什么可转。 */
  messages: Message[]
  onOpenChange: (open: boolean) => void
}

export function ForwardSheet({ open, sourceRoomId, messages, onOpenChange }: ForwardSheetProps) {
  const rooms = useRooms()
  const send = useAppStore((s) => s.send)
  const [targetId, setTargetId] = useState<string | null>(null)
  const [forwarding, setForwarding] = useState(false)
  const [sent, setSent] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const candidates = rooms.filter((r) => r.id !== sourceRoomId)
  const attachmentCount = messages.reduce((sum, each) => sum + (each.attachments?.length ?? 0), 0)

  const close = () => {
    onOpenChange(false)
    setTargetId(null)
    setForwarding(false)
    setSent(false)
    setError(null)
  }

  /**
   * 转发（2026-10-08 刀：**附件也一起转**；同日补：**一批也能转**）。
   *
   * 编排全在 `lib/forwardMessage.ts`（纯逻辑、可 `node --test` 直测）；这里只做三件事：
   * 把真依赖接上、按时间线**一条一条**转、把结果如实说出来。
   *
   * 判据是**每条各自全有或全无**：某一条的附件没转成 ⇒ 那条不发，**后面的也不再发**
   * （已经发出去的那几条照旧在——它们是真的发出去了，不能假装没发生，所以失败那句
   * 要说清"前 N 条已经转过去了"）。
   *
   * ⚠ 发送那一半的失败**不在这里报**：`store.send` 自己把那条乐观消息标成失败（气泡上有重试），
   * 那是全仓统一的发送失败路径 —— 在这里再报一次就是第二处判据。
   */
  const doForward = async () => {
    if (!targetId || messages.length === 0 || forwarding) return
    setForwarding(true)
    setError(null)
    try {
      const api = getApi()
      // 有附件就得有字节通道（演示通道两条都没有）⇒ **先说清楚**，不假装转过去了。
      if (attachmentCount > 0 && (!api?.fetchAttachmentBlob || !api?.uploadAttachment)) {
        setError('这条连接转不了附件')
        return
      }
      const deps = {
        fetchBlob: (id: string) => {
          if (!api?.fetchAttachmentBlob) return Promise.reject(new Error('没有字节通道'))
          return api.fetchAttachmentBlob(sourceRoomId, id)
        },
        upload: async (file: File) => {
          if (!api?.uploadAttachment) throw new Error('没有上传通道')
          return (await api.uploadAttachment(targetId, file)).attachmentId
        },
        send: (input: { body: string; attachments: Attachment[] }) => send(targetId, input),
      }
      let done = 0
      for (const each of messages) {
        const outcome = await forwardMessage(
          { body: each.body, attachments: each.attachments ?? [] },
          deps,
        )
        if (!outcome.ok) {
          setError(done === 0 ? outcome.text : `前 ${done} 条已经转过去了；${outcome.text}`)
          return
        }
        done += 1
      }
      setSent(true)
      setTimeout(close, 700)
    } finally {
      setForwarding(false)
    }
  }

  return (
    <Sheet open={open} onOpenChange={(v) => !v && close()}>
      <SheetContent className="w-full gap-0 p-0 sm:max-w-md">
        <SheetHeader className="border-b border-border">
          <SheetTitle>转发消息</SheetTitle>
          <SheetDescription>
            {/* 一条时给正文摘要；多条时只说条数（摘要挤在标题下面读不出重点）。 */}
            {messages.length === 0
              ? ''
              : messages.length === 1
                ? `「${messages[0].body.slice(0, 40)}${messages[0].body.length > 40 ? '…' : ''}」将作为一条新消息发送`
                : `${messages.length} 条消息将**按时间顺序**逐条发送`}
          </SheetDescription>
        </SheetHeader>
        <ScrollArea className="min-h-0 flex-1 [&_[data-radix-scroll-area-viewport]>div]:!block">
          <ul className="space-y-0.5 p-3">
            {candidates.map((room) => (
              <li key={room.id}>
                <button
                  type="button"
                  onClick={() => setTargetId(room.id)}
                  className={cn(
                    'flex w-full cursor-pointer items-center gap-3 rounded-lg px-3 py-3 text-left transition-colors hover:bg-muted',
                    targetId === room.id && 'bg-accent',
                  )}
                >
                  {room.peer ? (
                    <ParticipantAvatar participant={room.peer} size="md" />
                  ) : (
                    <div className="flex size-8 shrink-0 items-center justify-center rounded-full bg-gradient-to-br from-bubble-self-from to-bubble-self-to text-sm font-medium text-bubble-self-foreground">
                      {room.name.slice(0, 1)}
                    </div>
                  )}
                  <span className="min-w-0 flex-1 truncate text-body">{room.name}</span>
                  {room.peer?.kind === 'agent' &&
                    (room.peer.official ? <OfficialBadge /> : <AgentBadge />)}
                </button>
              </li>
            ))}
            {candidates.length === 0 && (
              <li className="px-3 py-10 text-center text-caption text-muted-foreground">
                没有可转发的会话
              </li>
            )}
          </ul>
        </ScrollArea>
        <div className="space-y-2 border-t border-border p-4">
          <Button
            className="w-full gap-1.5"
            disabled={!targetId || forwarding || messages.length === 0}
            onClick={() => void doForward()}
          >
            <SendHorizonal className="size-4" />
            {sent ? '已转发' : forwarding ? '转发中…' : messages.length > 1 ? `转发 ${messages.length} 条` : '转发'}
          </Button>
          {attachmentCount > 0 && (
            <p className="text-center text-caption text-muted-foreground">
              附件会一起转过去（{attachmentCount} 个）
            </p>
          )}
          {error && (
            <p role="alert" className="text-center text-caption text-destructive">
              {error}
            </p>
          )}
        </div>
      </SheetContent>
    </Sheet>
  )
}
