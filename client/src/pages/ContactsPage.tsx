import { useState } from 'react'
import { ArrowLeft, Check, ChevronDown, ChevronRight, UserPlus } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'
import { useNavigate, useParams } from 'react-router'
import { cn } from '@/lib/utils'
import { participants, SELF_ID } from '@/fixtures/data'
import type { Participant } from '@/api/domain'
import { useAppStore, useConnection } from '@/api/store'
import { errorText } from '@/api/contractErrors'
import { isRealWorldSession } from '@/api/prefs'
import { AgentBadge } from '@/components/celira/AgentBadge'
import { OfficialBadge } from '@/components/celira/OfficialBadge'
import { ParticipantAvatar } from '@/components/celira/ParticipantAvatar'
import { ContactDetailCard } from '@/components/celira/ContactDetailCard'

function ContactRow({
  participant,
  active,
  onSelect,
}: {
  participant: Participant
  active: boolean
  onSelect: (id: string) => void
}) {
  return (
    <li>
      <button
        type="button"
        onClick={() => onSelect(participant.id)}
        className={cn(
          'flex w-full cursor-pointer items-center gap-3 rounded-lg px-3 py-2.5 text-left transition-colors',
          active ? 'bg-accent' : 'hover:bg-muted',
        )}
      >
        <ParticipantAvatar participant={participant} size="md" />
        <span className="min-w-0 flex-1">
          <span className="flex items-center gap-2">
            <span
              className={cn('truncate text-body', participant.kind === 'agent' && 'text-agent')}
            >
              {participant.displayName}
            </span>
            {participant.kind === 'agent' &&
              (participant.official ? <OfficialBadge /> : <AgentBadge />)}
          </span>
          <span className="block truncate text-caption text-muted-foreground">
            {participant.signature ?? ''}
          </span>
        </span>
      </button>
    </li>
  )
}

function Group({
  title,
  members,
  activeId,
  onSelect,
}: {
  title: string
  members: Participant[]
  activeId?: string
  onSelect: (id: string) => void
}) {
  const [open, setOpen] = useState(true)
  if (members.length === 0) return null
  return (
    <section>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className="flex w-full cursor-pointer items-center gap-1.5 px-3 py-1.5 text-micro tracking-wider text-muted-foreground uppercase transition-colors hover:text-foreground"
      >
        {open ? <ChevronDown className="size-3.5" /> : <ChevronRight className="size-3.5" />}
        {title} — {members.length}
      </button>
      {open && (
        <ul className="space-y-0.5">
          {members.map((m) => (
            <ContactRow key={m.id} participant={m} active={m.id === activeId} onSelect={onSelect} />
          ))}
        </ul>
      )}
    </section>
  )
}

export function ContactsPage() {
  const { participantId } = useParams()
  const navigate = useNavigate()
  const [query, setQuery] = useState('')
  const { mode, token } = useConnection()
  const contacts = useAppStore((s) => s.contacts)
  const pendingContacts = useAppStore((s) => s.pendingContacts)
  const ownedAgents = useAppStore((s) => s.ownedAgents)
  const acceptContact = useAppStore((s) => s.acceptContact)
  const declineContact = useAppStore((s) => s.declineContact)
  const unblockParticipant = useAppStore((s) => s.unblockParticipant)
  const blocks = useAppStore((s) => s.blocks)
  // 待确认行上的就地失败提示（接受/拒绝各自的错都落这里，键是对方的 id）。
  const [rowError, setRowError] = useState<{ id: string; text: string } | null>(null)

  const q = query.trim().toLowerCase()
  const match = (p: Participant) => p.displayName.toLowerCase().includes(q)
  const others = participants.filter((p) => p.id !== SELF_ID)
  // 真实数据分支。官方 Agent 是 fixtures（Discover 切片），
  // 真实模式下绝不混入假数据。
  //
  // ⚠ 判据是 `isRealWorldSession`（**唯一权威**，见 `api/prefs.ts`）：演示通道也是
  // `mode === 'core'`，所以拿 mode 当"真实世界"会把它一起掐掉（演示通道进这一页会看到空列表）。
  const isReal = isRealWorldSession({ mode, token })
  const officialAgents = isReal ? [] : others.filter((p) => p.kind === 'agent' && p.official && match(p))
  const myAgents = isReal ? ownedAgents.filter(match) : others.filter((p) => p.kind === 'agent' && !p.official && match(p))
  // **联系人不再只是 Human**（批次三第二刀「关系对主体开放」）：`GET /v1/contacts`
  // 的对端从此可以是 Agent。所以这里按 `peer.kind` 分两件事：
  //   1. 我自己拥有的 Agent 走「我的 Agent」那一组（**按 id 去重**：一支我拥有的
  //      Agent 若同时与我有关系段，不该在两组里各出现一次）；
  //   2. 其余联系人（Human + 我不拥有的 Agent）同属「联系人」——标准 IM 里好友
  //      就是好友，不因为对面是账号还是服务号分家。
  const ownedIds = new Set(ownedAgents.map((p) => p.id))
  const contactPeers = contacts.filter((p) => !ownedIds.has(p.id))
  const contactsGroup = isReal
    ? contactPeers.filter(match)
    : others.filter((p) => p.kind === 'human' && match(p))
  const pendings = isReal ? pendingContacts : []

  const selected = isReal
    ? [...contacts, ...ownedAgents, ...pendingContacts].find((p) => p.id === participantId)
    : participants.find((p) => p.id === participantId)
  const select = (id: string) => navigate(`/app/contacts/${id}`)

  return (
    <div className="flex h-full min-h-0 w-full overflow-hidden bg-background">
      <aside
        className={cn(
          'flex w-full flex-col border-r border-border bg-card md:w-80 md:shrink-0',
          participantId && 'hidden md:flex',
        )}
      >
        <header className="border-b border-border px-5 py-4">
          <div className="text-title">联系人</div>
          <div className="text-caption text-muted-foreground">
            Human 与 Agent 都是这里的居民
          </div>
        </header>
        <div className="px-4 py-3">
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="搜索联系人…"
            className="border-transparent bg-muted focus-visible:border-input"
          />
        </div>
        <ScrollArea className="min-h-0 flex-1 [&_[data-radix-scroll-area-viewport]>div]:!block">
          <div className="space-y-2 px-2 pb-4">
            {/* 新的朋友：写路径（发起好友请求/接受 pending）已接通 Core 端点
                （CLIENT_CONTACT_REQUEST P1）；真实世界可用，不再拦截。 */}
            <button
              type="button"
              onClick={() => navigate('/app/contacts/new')}
              className="flex w-full cursor-pointer items-center gap-3 rounded-lg px-3 py-2.5 text-left transition-colors hover:bg-muted"
            >
              <span className="flex size-9 shrink-0 items-center justify-center rounded-full bg-accent text-accent-foreground">
                <UserPlus className="size-4" />
              </span>
              <span className="text-body">新的朋友</span>
              <ChevronRight className="ml-auto size-4 text-muted-foreground" />
            </button>
            {pendings.length > 0 && (
              <section>
                <div className="px-3 py-1.5 text-micro tracking-wider text-muted-foreground uppercase">
                  待确认 — {pendings.length}
                </div>
                <ul className="space-y-0.5">
                  {pendings.map((p) => (
                    <li key={p.id} className="rounded-lg px-3 py-2.5">
                      <div className="flex items-center gap-2.5">
                        <ParticipantAvatar participant={p} size="md" />
                        <span className="min-w-0 flex-1 truncate text-body">{p.displayName}</span>
                        {/* 拒绝与接受成对：「这条在等我回答」的两种回答都得给，
                            只给一种等于逼人先接受再断交（那会多留下一段已闭合的关系段）。 */}
                        <Button
                          size="sm"
                          variant="ghost"
                          aria-label={`拒绝 ${p.displayName} 的联系人请求`}
                          onClick={() => {
                            setRowError(null)
                            void declineContact(p.id).catch((e) =>
                              setRowError({ id: p.id, text: errorText(e) }),
                            )
                          }}
                        >
                          拒绝
                        </Button>
                        <Button
                          size="sm"
                          variant="secondary"
                          aria-label={`接受 ${p.displayName} 的联系人请求`}
                          onClick={() => {
                            setRowError(null)
                            void acceptContact(p.id).catch((e) =>
                              setRowError({ id: p.id, text: errorText(e) }),
                            )
                          }}
                        >
                          <Check className="size-3.5" />
                          接受
                        </Button>
                      </div>
                      {rowError?.id === p.id && (
                        <p className="pt-1 pl-11 text-caption text-presence-failed">
                          {rowError.text}
                        </p>
                      )}
                    </li>
                  ))}
                </ul>
              </section>
            )}
            <Group title="官方 Agent" members={officialAgents} activeId={participantId} onSelect={select} />
            <Group title="我的 Agent" members={myAgents} activeId={participantId} onSelect={select} />
            {/* 这一组标题从「Human」改成「联系人」：关系对主体开放之后，这一组里
                可能坐着我不拥有的 Agent（批次三第二刀）——旧标题会是一句假话。 */}
            <Group title="联系人" members={contactsGroup} activeId={participantId} onSelect={select} />
            {/* 已拉黑：只列**我拉黑的**那一半（契约：拉黑不泄露，没有"谁拉黑了我"的读取面）。
                做成可展开的一组，与小节标题一致；每行的动作是唯一的出口——解除拉黑。 */}
            {isReal && blocks.length > 0 && (
              <section>
                <div className="px-3 py-1.5 text-micro tracking-wider text-muted-foreground uppercase">
                  已拉黑 — {blocks.length}
                </div>
                <ul className="space-y-0.5">
                  {blocks.map((b) => (
                    <li key={b.id} className="flex items-center gap-2.5 rounded-lg px-3 py-2.5">
                      <ParticipantAvatar participant={b} size="md" />
                      <span className="min-w-0 flex-1 truncate text-body">{b.displayName}</span>
                      <Button
                        size="sm"
                        variant="ghost"
                        aria-label={`解除对 ${b.displayName} 的拉黑`}
                        onClick={() => void unblockParticipant(b.id)}
                      >
                        解除拉黑
                      </Button>
                    </li>
                  ))}
                </ul>
              </section>
            )}
            {officialAgents.length + myAgents.length + contactsGroup.length === 0 && blocks.length === 0 && (
              <div className="px-3 py-10 text-center text-caption text-muted-foreground">
                没有匹配的联系人
              </div>
            )}
          </div>
        </ScrollArea>
      </aside>
      <main className={cn('min-w-0 flex-1 flex-col', participantId ? 'flex' : 'hidden md:flex')}>
        {selected ? (
          <>
            <header className="flex shrink-0 items-center gap-1 border-b border-border bg-card px-2 py-1.5 md:hidden">
              <Button
                variant="ghost"
                size="icon"
                onClick={() => navigate('/app/contacts')}
                aria-label="返回联系人"
              >
                <ArrowLeft className="size-4" />
              </Button>
              <span className="truncate text-title">{selected.displayName}</span>
            </header>
            <ScrollArea className="h-full [&_[data-radix-scroll-area-viewport]>div]:!block">
              <ContactDetailCard participant={selected} />
            </ScrollArea>
          </>
        ) : (
          <div className="flex h-full items-center justify-center px-6 text-center text-body text-muted-foreground">
            选择一位联系人，看看TA的主页。
          </div>
        )}
      </main>
    </div>
  )
}
