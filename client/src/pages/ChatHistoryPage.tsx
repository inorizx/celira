import { useEffect, useMemo, useState } from 'react'
import { useNavigate, useParams } from 'react-router'
import { ArrowLeft, FileText, Film, Image as ImageIcon, Mic, Search } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'
import { tryGetParticipant } from '@/api/participantDirectory'
import { useAppStore, useConnection, useRooms } from '@/api/store'
import type { Message } from '@/api/domain'
import { attachmentMedia, type AttachmentMedia } from '@/lib/attachmentMedia'

const NO_MESSAGES: Message[] = []

// 类型页签 = **显示层归类**（`lib/attachmentMedia.ts` 那一份判据），不是第二份清单：
// 加一个渲染件就在这里多一个页签，两处不可能分叉。
type KindFilter = 'all' | AttachmentMedia

const KIND_TABS: Array<{ key: KindFilter; label: string }> = [
  { key: 'all', label: '全部' },
  { key: 'image', label: '图片' },
  { key: 'video', label: '视频' },
  { key: 'audio', label: '语音' },
  { key: 'file', label: '文件' },
]

function Highlighted({ text, query }: { text: string; query: string }) {
  if (!query) return <>{text}</>
  const lower = text.toLowerCase()
  const q = query.toLowerCase()
  const parts: Array<{ text: string; hit: boolean }> = []
  let i = 0
  for (;;) {
    const at = lower.indexOf(q, i)
    if (at === -1) {
      parts.push({ text: text.slice(i), hit: false })
      break
    }
    if (at > i) parts.push({ text: text.slice(i, at), hit: false })
    parts.push({ text: text.slice(at, at + q.length), hit: true })
    i = at + q.length
  }
  return (
    <>
      {parts.map((p, idx) =>
        p.hit ? (
          <mark key={idx} className="rounded-sm bg-agent-surface px-0.5 text-foreground">
            {p.text}
          </mark>
        ) : (
          <span key={idx}>{p.text}</span>
        ),
      )}
    </>
  )
}

export function ChatHistoryPage() {
  const { roomId } = useParams()
  const navigate = useNavigate()
  const { me } = useConnection()
  const rooms = useRooms()
  const room = rooms.find((r) => r.id === roomId)
  const messages = useAppStore((s) => s.messagesByRoom[roomId ?? '']) ?? NO_MESSAGES
  const nextBefore = useAppStore((s) => s.nextBeforeByRoom[roomId ?? ''])
  const loadOlder = useAppStore((s) => s.loadOlder)
  const loadRoomMessages = useAppStore((s) => s.loadRoomMessages)
  const loadError = useAppStore((s) => (roomId ? (s.messagesLoadErrorByRoom[roomId] ?? null) : null))
  const { connection } = useConnection()
  const [query, setQuery] = useState('')
  const [kind, setKind] = useState<KindFilter>('all')
  const [loadingOlder, setLoadingOlder] = useState(false)
  const [loadingHistory, setLoadingHistory] = useState(false)

  /**
   * **进这一页就把这个会话的历史读一次**（2026-09-29 真机实测修）。
   *
   * 这一页原先**只读已经加载过的消息**（`messagesByRoom[roomId]`）。从房间里点「查找聊天记录」
   * 进来时那些消息在手，所以搜得到；而**直接打开 / 刷新这个地址**时那个格子还是空的 ⇒
   * 页面把"还没加载"直接说成了「没有匹配的消息」——一句**关于世界的断言**，而事实是它自己
   * 没去取。这里补上触发点，并把三种处境分开说（见下面那段空态）。
   */
  useEffect(() => {
    if (!roomId) return
    if (connection !== 'connected' && connection !== 'reconnecting') return
    // 已经有东西了就不重复拉（从房间内点进来走的就是这一支）。
    if ((useAppStore.getState().messagesByRoom[roomId] ?? []).length > 0) return
    setLoadingHistory(true)
    void loadRoomMessages(roomId).finally(() => setLoadingHistory(false))
  }, [roomId, connection, loadRoomMessages])

  const nameOf = (authorId: string): string => {
    if (me?.participant?.id === authorId) return me.participant.display_name
    const p = room?.participants?.find((x) => x.id === authorId)
    if (p) return p.displayName
    const fp = tryGetParticipant(authorId)
    if (fp) return fp.displayName
    return '参与者'
  }

  const q = query.trim()
  const results = useMemo(() => {
    let list = messages.filter((m) => m.recalledAt == null && !m.system)
    if (kind !== 'all') list = list.filter((m) => m.attachments?.some((a) => attachmentMedia(a) === kind))
    if (q) list = list.filter((m) => m.body.toLowerCase().includes(q.toLowerCase()))
    return [...list].reverse()
  }, [messages, kind, q])

  const kindIcon = (k: Exclude<KindFilter, 'all'>) =>
    k === 'image' ? (
      <ImageIcon className="size-3.5" />
    ) : k === 'video' ? (
      <Film className="size-3.5" />
    ) : k === 'audio' ? (
      <Mic className="size-3.5" />
    ) : (
      <FileText className="size-3.5" />
    )

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      <header className="flex h-14 shrink-0 items-center gap-1 border-b border-border px-2 md:px-4">
        <Button variant="ghost" size="icon" aria-label="返回" onClick={() => navigate(-1)}>
          <ArrowLeft className="size-5" />
        </Button>
        <h1 className="text-title">查找聊天记录</h1>
        {room && <span className="truncate text-caption text-muted-foreground">· {room.name}</span>}
      </header>

      <div className="shrink-0 space-y-2 border-b border-border px-4 py-3 md:px-6">
        <div className="relative mx-auto w-full max-w-3xl">
          <Search className="absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="搜索消息内容…"
            aria-label="搜索消息内容"
            className="bg-muted border-transparent pl-9 focus-visible:border-input"
          />
        </div>
        <div className="mx-auto flex w-full max-w-3xl gap-1.5">
          {KIND_TABS.map((t) => (
            <button
              key={t.key}
              type="button"
              onClick={() => setKind(t.key)}
              className={cn(
                'cursor-pointer rounded-full px-3 py-1 text-caption transition-colors',
                kind === t.key ? 'bg-agent-surface text-foreground' : 'text-muted-foreground hover:text-foreground',
              )}
            >
              {t.label}
            </button>
          ))}
        </div>
      </div>

      <ScrollArea className="min-h-0 flex-1">
        <div className="mx-auto w-full max-w-3xl px-4 py-2 md:px-6">
          {results.length === 0 ? (
            <p className="py-10 text-center text-caption text-muted-foreground">
              {loadingHistory
                ? '正在读取这个会话的消息…'
                : loadError
                  ? `这个会话的消息没读到：${loadError}`
                  : q || kind !== 'all'
                    ? '没有匹配的消息'
                    : '输入关键词搜索，或按类型筛选'}
            </p>
          ) : (
            results.map((m) => {
              const att = m.attachments?.find((a) => kind === 'all' || attachmentMedia(a) === kind)
              return (
                <button
                  key={m.id}
                  type="button"
                  onClick={() => navigate(`/app/chat/${roomId}`, { state: { targetId: m.id } })}
                  className="flex w-full cursor-pointer flex-col gap-0.5 rounded-lg px-2 py-2.5 text-left transition-colors hover:bg-muted"
                >
                  <div className="flex items-baseline justify-between gap-3">
                    <span className="truncate text-caption font-medium text-agent">{nameOf(m.authorId)}</span>
                    <span className="shrink-0 text-caption text-muted-foreground">{m.sentAt}</span>
                  </div>
                  <div className="flex items-center gap-1.5 text-body text-foreground">
                    {att && <span className="shrink-0 text-muted-foreground">{kindIcon(attachmentMedia(att))}</span>}
                    <span className="line-clamp-2 break-all">
                      {m.body ? <Highlighted text={m.body} query={q} /> : (att?.name ?? '[附件]')}
                    </span>
                  </div>
                </button>
              )
            })
          )}
          {nextBefore && (
            <div className="flex justify-center py-4">
              <Button
                variant="ghost"
                size="sm"
                disabled={loadingOlder}
                onClick={() => {
                  setLoadingOlder(true)
                  void loadOlder(roomId ?? '').finally(() => setLoadingOlder(false))
                }}
              >
                加载更早的消息以继续搜索
              </Button>
            </div>
          )}
        </div>
      </ScrollArea>
    </div>
  )
}
