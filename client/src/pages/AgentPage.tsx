import { useEffect, useState } from 'react'
import { useNavigate, useParams } from 'react-router'
import { ArrowLeft, Copy, MessageSquarePlus, Settings2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Separator } from '@/components/ui/separator'
import { getParticipant } from '@/api/participantDirectory'
import { AgentBadge } from '@/components/celira/AgentBadge'
import { OfficialBadge } from '@/components/celira/OfficialBadge'
import { ParticipantAvatar } from '@/components/celira/ParticipantAvatar'
import type { ProfileView } from '@/api/contracts'
import { getApi, useAppStore, useRooms } from '@/api/store'
import { supportsAgentProfile } from '@/api/uiPolicy'
import { resolveSelfId } from '@/api/selfIdentity'
import { directRoomOf } from '@/api/directRoom'
import { errorText } from '@/api/contractErrors'

/**
 * **世界侧的参与者页**（`/agent/:participantId`）。
 *
 * ## 这一页是什么、为什么在这儿（2026-09-26 重写）
 *
 * 归属判据（`api/routeRegistry.ts` 文件头）：**把 im 服务端换成微信/QQ，这一页还成立吗？**
 * 「点开一个人，看它是谁」——成立 ⇒ `world`。
 *
 * ⚠ **重写前它整页在 `FixtureRouteGuard` 之下**（`side: 'demo'`）——而**真实世界有三处
 * 入口链到它**（消息头像 / 右键菜单「查看主页」/ 联系人资料卡）⇒ 真实用户点得到、
 * 点进去撞门。这是审计里最重的一条硬伤（H9）。同一刀里还修掉两处：
 * 「发消息」按钮原先**跳写死的演示房号**（`r-guide` / `r-courier`，真实世界进不存在的房间），
 * 以及一处漏网的内行词（`Agent Backend`）。
 *
 * ## 它**不是** `/app/agents/:participantId`
 *
 * 那一页是**归属人管自己的 Agent**（接入 / 凭据 / 撤销）。这一页是**任何参与者
 * 在世界里的样子**——包括不是你的人、不是你的联系人、只在某个群里见过一次的人。
 * 两页**互链不合并**（判据同上：前者换成微信不成立，后者成立）。
 *
 * ## 它不显示什么
 *
 * 不显示「在不在线」（2026-09-26 主人裁决：IM 侧不展示身体态）、不显示它在哪台机器上跑、
 * 用什么模型（那是**执行端**的事实，在 `/app/runtimes`）。页脚把这件事说出来。
 */
export function AgentPage() {
  const { participantId } = useParams()
  const navigate = useNavigate()
  const allRooms = useRooms()
  const me = useAppStore((s) => s.me)
  const ownedAgents = useAppStore((s) => s.ownedAgents)
  const contacts = useAppStore((s) => s.contacts)
  const createDirectRoom = useAppStore((s) => s.createDirectRoom)
  const createHumanDirectRoom = useAppStore((s) => s.createHumanDirectRoom)

  const participant = participantId
    ? (() => {
        try {
          return getParticipant(participantId)
        } catch {
          return undefined
        }
      })()
    : undefined

  const isAgent = participant?.kind === 'agent'
  // 归属判据：真实世界只信真实 ownedAgents；演示通道由目录回落自己解决。
  const isMine = participant ? ownedAgents.some((a) => a.id === participant.id) : false
  const isContact = participant ? contacts.some((c) => c.id === participant.id) : false

  // 公共资料：判据是**这台后端有没有这个数据面**（能力探测），不是"是不是真实世界"
  // —— `mode` 只有一个取值，拿它当判据在演示通道里也会成立。
  const profileSupported = supportsAgentProfile(getApi())
  const [profile, setProfile] = useState<ProfileView | null>(null)
  const [profileLoaded, setProfileLoaded] = useState(false)
  const [profileError, setProfileError] = useState(false)

  useEffect(() => {
    if (!isAgent || !participantId) return
    const api = getApi()
    if (!api?.getAgentProfile) return
    let active = true
    void api
      .getAgentProfile(participantId)
      .then((response) => {
        if (!active) return
        setProfile(response.profile)
        setProfileLoaded(true)
      })
      .catch(() => {
        if (active) setProfileError(true)
      })
    return () => {
      active = false
    }
  }, [isAgent, participantId])

  // ── 发消息：**真的**建/进一个直聊房，不再跳写死的演示房号 ──────────────────
  //
  // 「哪间是我们的房」这条判据**不在本页**（2026-10-20 T4）：它现在也是 Agent 主页首屏
  // 那颗「发消息」的判据 ⇒ 抽到 `api/directRoom.ts` 的 `directRoomOf`（纯函数、有用例），
  // 本页只消费它。**写入口仍然只有一处**：`store` 的 `createDirectRoom` / `createHumanDirectRoom`。
  // 建失败（对 Human 而言，契约要求先有 accepted 联系人关系）**如实报错**，
  // 不静默、不假装成功。
  const selfId = resolveSelfId(me?.participant.id)
  const directRoom = participant ? directRoomOf(allRooms, selfId, participant.id) : null
  const [creating, setCreating] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)

  const handleSendMessage = async () => {
    if (!participant) return
    if (directRoom) {
      navigate(`/app/chat/${directRoom.id}`)
      return
    }
    if (creating) return
    setCreating(true)
    setActionError(null)
    try {
      const roomId = isAgent
        ? await createDirectRoom(participant.id)
        : await createHumanDirectRoom(participant.id)
      navigate(`/app/chat/${roomId}`)
    } catch (e) {
      setActionError(errorText(e))
    } finally {
      setCreating(false)
    }
  }

  if (!participant) {
    return (
      <div className="flex h-full min-h-0 flex-col items-center justify-center gap-3 bg-background px-6">
        <div className="text-body">没有这位参与者的资料</div>
        {/* ⚠ 不许把它说成"这个人不存在"：读不到 ≠ 不存在（这一页的目录只认
            "和你同处过一个房间"的参与者，读不到是**我们**的视野边界，不是它的事实）。 */}
        <p className="max-w-sm text-center text-caption text-muted-foreground">
          这里只读得到和你同处过一个房间的参与者。
        </p>
        <Button variant="outline" onClick={() => navigate(-1)}>
          返回
        </Button>
      </div>
    )
  }

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      <header className="flex items-center gap-2 border-b border-border bg-card px-4 py-4">
        <Button variant="ghost" size="icon" onClick={() => navigate(-1)} aria-label="返回">
          <ArrowLeft className="size-4" />
        </Button>
        <div className="text-title">主页</div>
      </header>
      <ScrollArea className="min-h-0 flex-1">
        <div className="mx-auto w-full max-w-3xl space-y-6 px-4 py-6">
          <section className="space-y-4 rounded-xl border border-border bg-card p-5">
            <div className="flex items-center gap-4">
              <ParticipantAvatar participant={participant} size="xl" />
              <div className="min-w-0 space-y-1">
                <div className="flex items-center gap-2">
                  <span className="truncate text-title">{participant.displayName}</span>
                  {isAgent && (participant.official ? <OfficialBadge /> : <AgentBadge />)}
                </div>
                <div className="text-caption text-muted-foreground">
                  {isAgent
                    ? participant.official
                      ? '官方 Agent · 由 Celira 平台提供'
                      : 'Agent'
                    : 'Human'}
                </div>
              </div>
            </div>

            <div className="flex items-baseline gap-3 rounded-lg bg-muted px-3 py-2">
              <span className="shrink-0 text-caption text-muted-foreground">身份 ID</span>
              <code className="min-w-0 flex-1 truncate font-mono text-caption">{participant.id}</code>
              <Copy className="size-3 shrink-0 text-muted-foreground" aria-hidden />
            </div>

            <Button className="w-full gap-1.5" disabled={creating} onClick={() => void handleSendMessage()}>
              <MessageSquarePlus className="size-4" />
              {creating ? '正在建会话…' : '发消息'}
            </Button>
            {actionError && <p className="text-caption text-presence-failed">{actionError}</p>}
          </section>

          {profileSupported && isAgent && (
            <section className="space-y-3 rounded-xl border border-border bg-card p-5">
              <div className="text-micro tracking-wider text-muted-foreground uppercase">公共资料</div>
              {profileError ? (
                <p className="text-caption text-muted-foreground">当前无法读取这位 Agent 的公共资料。</p>
              ) : !profileLoaded ? (
                <p className="text-caption text-muted-foreground">正在读取公共资料…</p>
              ) : profile?.short_description || profile?.public_description ? (
                <div className="space-y-2">
                  {profile.short_description && <p className="text-body">{profile.short_description}</p>}
                  {profile.public_description && (
                    <p className="whitespace-pre-wrap text-caption text-muted-foreground">
                      {profile.public_description}
                    </p>
                  )}
                </div>
              ) : (
                <p className="text-caption text-muted-foreground">这位 Agent 还没有填写公共资料。</p>
              )}
            </section>
          )}

          {/* ── 去别处：这一页只说"它在世界里是什么样"，管理与接线各有其页 ────────── */}
          {(isMine || isContact) && (
            <section className="space-y-2">
              {isMine && (
                <Button
                  variant="outline"
                  className="w-full justify-start gap-2"
                  onClick={() => navigate(`/app/agents/${participant.id}`)}
                >
                  <Settings2 className="size-4" />
                  它是你的 Agent —— 接入与凭据在它的主页
                </Button>
              )}
              {isContact && (
                <Button
                  variant="outline"
                  className="w-full justify-start gap-2"
                  onClick={() => navigate(`/app/contacts/${participant.id}`)}
                >
                  <Copy className="size-4" />
                  在你的联系人里
                </Button>
              )}
            </section>
          )}

          <Separator />
          <p className="text-center text-caption text-muted-foreground">
            这一页只说它在世界里是什么样。它跑在哪台机器上、用什么模型，在「我的机器」那一页。
          </p>
        </div>
      </ScrollArea>
    </div>
  )
}
