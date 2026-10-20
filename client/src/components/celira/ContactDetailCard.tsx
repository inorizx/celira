import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router'
import { Cable, Cpu, MessageSquarePlus, Settings2, UserRound } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Separator } from '@/components/ui/separator'
import { getModel, getProvider, myAgentConfig } from '@/fixtures/data'
import { resolveSelfId } from '@/api/selfIdentity'
import { relationActionAvailability } from '@/api/uiPolicy'
import type { Participant } from '@/api/domain'
import type { ProfileView } from '@/api/contracts'
import { getApi, useAppStore, useConnection, useRooms } from '@/api/store'
import { errorText } from '@/api/contractErrors'
import { isRealWorldSession } from '@/api/prefs'
import { AgentBadge } from './AgentBadge'
import { CopyIdButton } from './CopyIdButton'
import { OfficialBadge } from './OfficialBadge'
import { ParticipantAvatar } from './ParticipantAvatar'

/**
 * **销毁性动作的二次确认**：点一次进入「再次点击确认…」，4 秒后自动解除。
 *
 * 与「撤销 Agent」（`MyAgentsPage`）和「退出登录」（`SettingsGeneralPage`）**同一套做法**：
 * 这个仓库不用弹窗做确认。搬成钩子是因为这一页一次要用两个（断交、拉黑），再抄两遍就只剩
 * 三段一模一样的东西；判据、时长、文案形态三处保持一致。
 */
function useArmedConfirm(): [boolean, (run: () => void) => void] {
  const [armed, setArmed] = useState(false)
  return [
    armed,
    (run: () => void) => {
      if (!armed) {
        setArmed(true)
        setTimeout(() => setArmed(false), 4000)
        return
      }
      setArmed(false)
      run()
    },
  ]
}

function InfoRow({ label, value, truncate = false }: { label: string; value: string; truncate?: boolean }) {
  return (
    <div className="flex items-baseline gap-4 px-5 py-3">
      <span className="w-16 shrink-0 text-caption text-muted-foreground">{label}</span>
      <span className={truncate ? 'truncate text-body' : 'text-body'}>{value}</span>
    </div>
  )
}

export function ContactDetailCard({ participant }: { participant: Participant }) {
  const navigate = useNavigate()
  const me = useAppStore((s) => s.me)
  const isAgent = participant.kind === 'agent'
  const ownedAgents = useAppStore((s) => s.ownedAgents)
  const { mode, token } = useConnection()
  // 真实分支：归属用真实 ownedAgents；mock 才用 fixtures 判定。
  //
  // ⚠ 判据是 `isRealWorldSession`（**唯一权威**）：演示通道也是 `mode === 'core'`，
  // 拿 mode 当"真实世界"会把演示通道一起掐掉。
  const isReal = isRealWorldSession({ mode, token })
  const isMine = isReal
    ? ownedAgents.some((a) => a.id === participant.id)
    : participant.id === myAgentConfig.agentId
  const model = isMine ? getModel(myAgentConfig.modelId) : undefined
  const modelProvider = model ? getProvider(model.providerId) : undefined
  const allRooms = useRooms()
  const createDirectRoom = useAppStore((s) => s.createDirectRoom)
  // 关系面（关联可逆性切片）：这三个判据来自 store 的 contact / block 两份读数。
  const contacts = useAppStore((s) => s.contacts)
  const blocks = useAppStore((s) => s.blocks)
  const removeContact = useAppStore((s) => s.removeContact)
  const blockParticipant = useAppStore((s) => s.blockParticipant)
  const unblockParticipant = useAppStore((s) => s.unblockParticipant)
  const [removeArmed, runRemoveArmed] = useArmedConfirm()
  const [blockArmed, runBlockArmed] = useArmedConfirm()
  const [relationError, setRelationError] = useState<string | null>(null)
  // 「发消息」建首房的创建中/失败提示（单窗模型；不新增状态机）
  const [creating, setCreating] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  // Agent 公共资料（CORE_V1_PARTICIPANT_PRESENTATION.md）：只对「当前 Human
  // 拥有的 Agent」在真实模式（core）读取展示（short_description 卡片位 +
  // public_description 详情位）；无 Profile（版本 0）时展示诚实的空态。非拥有
  // 的 Agent 不在本卡片读取（归属判定复用既有 /v1/agents 投影，绝不伪造资料）。
  const [agentProfile, setAgentProfile] = useState<ProfileView | null>(null)
  const [agentProfileLoaded, setAgentProfileLoaded] = useState(false)
  const [agentProfileFailed, setAgentProfileFailed] = useState(false)

  useEffect(() => {
    if (!isAgent || !isMine || !isReal) return
    const currentApi = getApi()
    if (!currentApi?.getAgentProfile) return
    let active = true
    void currentApi
      .getAgentProfile(participant.id)
      .then((result) => {
        if (!active) return
        setAgentProfile(result.profile)
        setAgentProfileLoaded(true)
      })
      .catch(() => {
        if (active) setAgentProfileFailed(true)
      })
    return () => {
      active = false
    }
  }, [isAgent, isMine, isReal, participant.id])

  const isOwned = isMine
  // 自 ID：真实世界用真实 me；演示通道才用 fixtures。
  // TASK_56：判据收敛到 `resolveSelfId`（真实世界无身份时返空 ⇒ 找不到直聊房，
  // 而不是拿 `p-inori` 去撞一串真实 id）。
  const selfId = resolveSelfId(me?.participant.id)
  const directRooms = allRooms.filter(
    (r) =>
      r.participantIds.length === 2 &&
      r.participantIds.includes(selfId) &&
      r.participantIds.includes(participant.id),
  )
  const directRoom = directRooms[0]

  // 单窗模型：发消息 = 自然首房路径。已有直聊房 → 原样进入；真实模式 +
  // 自有 Agent + 无房 → 建首房后进入（期间 loading、失败就地展示）；真实
  // 模式 + 非自有 + 无房 → 按钮禁用；mock 维持 r-guide 演示路径。
  const handleSendMessage = async () => {
    if (directRoom) {
      navigate(`/app/chat/${directRoom.id}`)
      return
    }
    if (isReal) {
      if (creating) return
      setCreating(true)
      setActionError(null)
      try {
        const roomId = await createDirectRoom(participant.id)
        navigate(`/app/chat/${roomId}`)
      } catch (e) {
        setActionError(errorText(e))
      } finally {
        setCreating(false)
      }
      return
    }
    navigate('/app/chat/r-guide')
  }

  // ── 关系可逆性（`CORE_V1_CONTACT_REVERSIBILITY.md`）────────────────────────
  //
  // 三个动作的**可用性**由 `relationActionAvailability` 一个纯函数裁决（有直测），这一层只管
  // "点了之后发生什么"。断交与拉黑都会**关掉你们那间房**（契约：段结束时双方成员周期同事务
  // 关闭）⇒ 落定之后必须离开这一页：留着它只会显示一份已经不存在的资料。
  const isContact = contacts.some((c) => c.id === participant.id)
  const blocked = blocks.some((b) => b.id === participant.id)
  const relation = relationActionAvailability({ isContact, blocked })

  const runRelationAction = async (action: () => Promise<void>, leave: boolean) => {
    setRelationError(null)
    try {
      await action()
      if (leave) navigate('/app/contacts')
    } catch (e) {
      setRelationError(errorText(e))
    }
  }

  return (
    <div className="mx-auto w-full max-w-xl space-y-6 px-4 py-8 md:py-12">
      <div className="flex items-center gap-4">
        <ParticipantAvatar participant={participant} size="xl" />
        <div className="min-w-0 space-y-1">
          <div className="flex items-center gap-2">
            <span className="truncate text-display">{participant.displayName}</span>
            {isAgent && (participant.official ? <OfficialBadge /> : <AgentBadge />)}
          </div>
        </div>
      </div>

      <div className="divide-y divide-border rounded-xl border border-border bg-card">
        <div className="flex items-baseline gap-4 px-5 py-3">
          <span className="w-16 shrink-0 text-caption text-muted-foreground">身份 ID</span>
          <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-caption">{participant.id}</code>
          <CopyIdButton value={participant.id} className="self-center" />
        </div>
        {participant.signature && (
          <InfoRow label="签名" value={participant.signature} truncate />
        )}
        {participant.since && <InfoRow label="加入时间" value={participant.since} />}
        <InfoRow
          label="来源"
          value={
            isAgent
              ? participant.official
                ? 'Celira 平台内置'
                : isMine
                  ? '我把它接进来的'
                  : '他人接入的 Agent'
              : '朋友验证'
          }
        />
      </div>

      {/* 公共资料（真实模式 + 本人拥有）：short_description（卡片位） +
          public_description（详情位）；无 Profile（版本 0）诚实空态。 */}
      {isAgent && isMine && isReal && (
        <div className="space-y-3 rounded-xl border border-border bg-card p-5">
          <div className="text-micro tracking-wider text-muted-foreground uppercase">公共资料</div>
          {agentProfileFailed ? (
            <p className="text-caption text-muted-foreground">当前无法读取这位 Agent 的公共资料。</p>
          ) : !agentProfileLoaded ? (
            <p className="text-caption text-muted-foreground">正在读取公共资料…</p>
          ) : agentProfile ? (
            <div className="space-y-2">
              {agentProfile.short_description && (
                <p className="text-body">{agentProfile.short_description}</p>
              )}
              {agentProfile.public_description && (
                <p className="whitespace-pre-wrap text-caption text-muted-foreground">
                  {agentProfile.public_description}
                </p>
              )}
              {!agentProfile.short_description && !agentProfile.public_description && (
                <p className="text-caption text-muted-foreground">这位 Agent 还没有填写公共资料。</p>
              )}
            </div>
          ) : (
            <p className="text-caption text-muted-foreground">这位 Agent 还没有填写公共资料。</p>
          )}
        </div>
      )}

      {isAgent && !participant.official && (
        <div className="divide-y divide-border rounded-xl border border-border bg-card">
          <div className="flex items-center gap-2 px-5 py-3">
            <Cable className="size-3.5 shrink-0 text-muted-foreground" />
            <span className="text-body">
              {isMine
                ? isReal
                  ? '由你这台机器驱动'
                  : myAgentConfig.backendLabel
                : '接入方由其归属人管理'}
            </span>
          </div>
          {/* 模型来自 fixtures，真实模式（core）不展示假配置 */}
          {isMine && !isReal && (
            <div className="flex items-center gap-2 px-5 py-3 text-caption text-muted-foreground">
              <Cpu className="size-3.5 shrink-0" />
              模型 · {model ? model.label : '未选择'}
              {modelProvider && ` · ${modelProvider.name}`}
            </div>
          )}
        </div>
      )}

      {isAgent && (directRooms.length > 0 || isOwned) && (
        <div className="space-y-2">
          <div className="px-1 text-micro tracking-wider text-muted-foreground uppercase">
            和 TA 的对话
          </div>
          <div className="divide-y divide-border rounded-xl border border-border bg-card">
            {directRooms.map((room) => (
              <button
                key={room.id}
                type="button"
                onClick={() => navigate(`/app/chat/${room.id}`)}
                aria-label={`打开对话 ${room.name}`}
                className="flex w-full cursor-pointer items-center gap-3 px-5 py-3 text-left transition-colors hover:bg-muted"
              >
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-body">{room.name}</span>
                  <span className="block truncate text-caption text-muted-foreground">
                    {room.lastMessagePreview || '还没有消息'}
                  </span>
                </span>
                <span className="shrink-0 text-caption text-muted-foreground">
                  {room.lastActivityAt}
                </span>
                {/* ⚠ 2026-09-26 摘掉这里原先的未读角标（`room.unreadCount > 0` → `bg-primary` 圆点）。
                    `DESIGN_SYSTEM.md` 的配方写着「**未读角标只在两处**：会话列表行内 + AppShell
                    消息 tab 汇总」—— 这是**第三处**，而且它出现在「某位联系人的资料卡」里，
                    与那两处的语义（"哪些会话有未读"）也不是同一件事。
                    未读数仍由那两处如实呈现；这一页只说"这个人是谁"。 */}
              </button>
            ))}
          </div>
        </div>
      )}

      {actionError && (
        <p className="px-1 text-center text-caption text-presence-failed">{actionError}</p>
      )}
      <div className="flex items-center justify-center gap-3">
        <Button
          className="gap-1.5"
          disabled={isReal && !directRoom && !isOwned}
          onClick={() => void handleSendMessage()}
        >
          <MessageSquarePlus className="size-4" />
          {creating ? '创建中…' : '发消息'}
        </Button>
        {isMine && (
          <Button variant="outline" className="gap-1.5" onClick={() => navigate('/app/profile')}>
            <Settings2 className="size-4" />
            配置
          </Button>
        )}
        {participant.official && (
          <Button
            variant="outline"
            className="gap-1.5"
            onClick={() => navigate(`/agent/${participant.id}`)}
          >
            <UserRound className="size-4" />
            主页
          </Button>
        )}
      </div>

      {/* 关系面。**后果写成常驻说明，不藏在确认里**：断交会永久封存你们那间房，
          那是这一页上唯一一个不可逆的动作，写清楚比让人点两次更重要。
          每一段说明只在它真的适用时才出现（没有关系段时不谈"断交"，没拉黑时不谈"解除"）。 */}
      {(relation.remove || relation.block || relation.unblock) && (
        <div className="space-y-3 rounded-xl border border-border bg-card p-5">
          <div className="text-micro tracking-wider text-muted-foreground uppercase">关系</div>
          <ul className="space-y-1 text-caption text-muted-foreground">
            {relation.remove && (
              <>
                <li>断交后你们那间房永久封存：双方都不能再发也不能再读，历史只留在服务器上。</li>
                <li>之后重新加回来是另建一间新房，旧房永久不可达；对方不会收到通知。</li>
              </>
            )}
            {relation.block && (
              <li>
                拉黑同时就是断交（上面那两条一样适用）。它不影响群聊，你们在同一个群里照常互相
                看得见发言。
              </li>
            )}
            {relation.unblock && <li>解除拉黑只是放你过去，不会恢复关系——你们仍然是陌生人。</li>}
          </ul>
          {relationError && (
            <p className="text-caption text-presence-failed">{relationError}</p>
          )}
          <div className="flex flex-wrap items-center gap-3">
            {relation.remove && (
              <Button
                variant="outline"
                className="text-presence-failed"
                onClick={() =>
                  runRemoveArmed(() => void runRelationAction(() => removeContact(participant.id), true))
                }
              >
                {removeArmed ? '再次点击确认断交' : '断交'}
              </Button>
            )}
            {relation.block && (
              <Button
                variant="outline"
                className="text-presence-failed"
                onClick={() =>
                  runBlockArmed(() =>
                    void runRelationAction(() => blockParticipant(participant.id), true),
                  )
                }
              >
                {blockArmed ? '再次点击确认拉黑' : '拉黑'}
              </Button>
            )}
            {relation.unblock && (
              <Button
                variant="outline"
                onClick={() =>
                  void runRelationAction(() => unblockParticipant(participant.id), false)
                }
              >
                解除拉黑
              </Button>
            )}
          </div>
        </div>
      )}

      <Separator />
      <p className="text-center text-caption text-muted-foreground">
        它跑在哪台机器上、用什么模型，在「我的机器」那一页。
      </p>
    </div>
  )
}
