import { useCallback, useEffect, useRef, useState } from 'react'
import { useLocation, useNavigate, useParams } from 'react-router'
import {
  ArrowLeft,
  Check,
  ClipboardCopy,
  ExternalLink,
  Image as ImageIcon,
  KeyRound,
  Link2,
  MessageSquarePlus,
  Server,
  ShieldCheck,
  UserRound,
} from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { cn } from '@/lib/utils'
import { scrollToAnchor } from '@/lib/anchorScroll'
import type { OwnedAgent } from '@/api/domain'
import type { BackendBindingView, CredentialView, ProfileView } from '@/api/contracts'
import { getApi, useAppStore, useConnection, useRooms } from '@/api/store'
import { credentialsOf } from '@/api/credentials'
import { bindingsOf } from '@/api/agentBindings'
import { AGENT_REVOKED_CONSEQUENCE, shouldShowAgentAdmin } from '@/api/agentRevoked'
import { agentRevokeErrorText, errorText } from '@/api/contractErrors'
import {
  supportsAgentBindings,
  supportsAgentBindingWrite,
  supportsAgentCredentials,
  supportsAgentCredentialWrite,
  supportsAgentDetail,
  supportsAgentRevoke,
} from '@/api/uiPolicy'
import { isRealWorldSession } from '@/api/prefs'
import { AgentBadge } from '@/components/celira/AgentBadge'
import { ParticipantAvatar } from '@/components/celira/ParticipantAvatar'
import { AgentsListPanel } from '@/components/celira/AgentsListPanel'
// ⚠ 这两个是**中性布局件**（`SectionFrame.tsx`）。它们原先住在执行端的 `Runtimes.tsx` 里
// 叫 `RuntimeFrame` / `RuntimesScroll`，而这一页是**世界侧**的参与者页 ——
// 一个世界侧页面从执行端组件文件里取布局件，是边界说不清（2026-09-26 抽出来）。
import { PageScroll, SectionFrame } from '@/components/celira/SectionFrame'
import { agentHomeReadings, useHomeEntries } from '@/components/celira/use-home-entries'
import { resolveAgentHome } from '@/api/homes'
import {
  attachAgentOnMachine,
  bindHomeSource,
  fetchHomeCatalog,
  HomeConfigError,
  REASONING_EFFORTS,
  residentModelLine,
  residentModelReadout,
  unhomedModelLine,
  type HomeConfigProbe,
  type HomeConfigView,
} from '@/api/homeConfig'
import {
  attachOutcomeLine,
  machineCandidates,
  ONE_MACHINE_NOTE,
  type MachineCandidate,
} from '@/api/remoteAttach'
import { agentMachineReadout, modelChoiceMachine, type AgentMachineKind } from '@/api/agentMachine'
import { agentHealthForClaimedMachine } from '@/api/agentHealth'
import { directRoomOf } from '@/api/directRoom'
import { resolveSelfId } from '@/api/selfIdentity'
import { READOUT_TONE_TEXT } from '@/components/celira/status-tone'

/** content_type → 人话。认不出的照原样显示（Core 才是权威，前端不替它归一）。 */
function avatarKindLabel(contentType: string): string {
  if (contentType === 'image/png') return 'PNG'
  if (contentType === 'image/jpeg') return 'JPEG'
  if (contentType === 'image/webp') return 'WebP'
  return contentType
}

/** 字节数 → 展示用读数（头像上限 2 MiB，所以只需要 KB/MB 两档）。 */
function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

/**
 * 接入那一格的徽章：**绑定是一个授权事实** —— 它说的是"有人被授权替这个 Agent 行动"，
 * **不是**"它此刻在不在"（契约原话：**Binding 不代表 Backend 在线**）。
 *
 * ⚠ 从前这里说的是「已连接」＋ 一颗 `presence-online` 绿点（按 2026-10-12 前的设计系统那一行做的）。
 * 那是**用在场那一套词答授权那个问题**：两者会分开 —— 真机上撞到过：绑定 `active`、而这台机器上
 * **一支住户都没有**（没有机器在跑它），页面照样绿着说"已连接"。⇒ 只说它自己的状态，
 * 用与下面「凭证」那一间**同一套词**（生效中 / 已解除），也不借在场那颗点。
 * **"它此刻在不在"的唯一权威是机器页**（本页下面那一段只给住址，不复述运行状态）。
 *
 * ⚠ **2026-10-20（T4 命名）**：撤销那一族拆成三个词，各归一件事 ——
 * **解除**接入（这个授权不再有效）／**作废**凭证（那串 token 当场失效）／**撤销** Agent（不可恢复）。
 * 所以这一格的终态叫「已解除」，不再与"撤销 Agent"共用一个"撤销"。
 */
function CoreBindingBadge({ status }: { status: string }) {
  return (
    <span className="inline-flex items-center text-caption text-muted-foreground">
      {status === 'active' ? '生效中' : '已解除'}
    </span>
  )
}

function OneTimeToken({ token, onDismiss }: { token: string; onDismiss: () => void }) {
  const [copied, setCopied] = useState(false)
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(token)
      setCopied(true)
      setTimeout(() => setCopied(false), 1600)
    } catch {
      // clipboard unavailable
    }
  }
  return (
    <div className="space-y-2 rounded-lg border border-border bg-muted p-3">
      <div className="text-caption text-muted-foreground">接入令牌（仅显示这一次，请立即复制保存）</div>
      <code className="block break-all rounded bg-card px-2 py-1.5 font-mono text-caption">{token}</code>
      <div className="flex gap-2">
        <Button size="sm" variant="outline" className="gap-1.5" onClick={() => void copy()}>
          {copied ? <Check className="size-3.5" /> : <ClipboardCopy className="size-3.5" />}
          {copied ? '已复制' : '复制 token'}
        </Button>
        <Button size="sm" variant="ghost" onClick={onDismiss}>
          关闭
        </Button>
      </div>
    </div>
  )
}

/**
 * 「我的 Agent」列表（`/app/agents`）：**只是列表**。
 *
 * 点一个 Agent 不在这里展开详情 —— 而是进**Agent 主页**（`/app/agents/:participantId`）。
 *
 * ⚠ **2026-09-25 组织轴刀**：Agent 主页从四间瘦成**两间**（资料 + 服务）——
 * 原先那三间（家 / 日子 / 脑子）读的是**机器级事实**，已搬到 `/app/runtimes/:entryId`。
 * 理由见 `navGroups.ts` 头上那段裁决：**轴与事实必须一致**。
 *
 * 两个路由共用这一个模块：它们要的是**同一份"选中哪个 Agent"的状态与数据面**
 * （`ownedAgents` / 绑定 / 凭据 / 资料 / 头像），拆成两份必然漂移。
 */
export function MyAgentsPage() {
  return <MyAgentsShell />
}

/** Agent 主页（`/app/agents/:participantId`）：资料 + 机器两段，一个 Agent。 */
export function AgentHomePage() {
  const { participantId } = useParams()
  return <MyAgentsShell participantId={participantId ?? ''} />
}

function MyAgentsShell({ participantId }: { participantId?: string }) {
  /** 主页模式：地址里点了名，页面说的就是**那一个 Agent**（而不是"列表里选中的那个"）。 */
  const homeMode = participantId !== undefined && participantId !== ''
  const navigate = useNavigate()
  const { mode, token } = useConnection()
  const ownedAgents = useAppStore((s) => s.ownedAgents)
  const ownedAgentsLoading = useAppStore((s) => s.ownedAgentsLoading)
  const ownedAgentsError = useAppStore((s) => s.ownedAgentsError)
  const refreshOwnedAgents = useAppStore((s) => s.refreshOwnedAgents)
  const bindingsByAgent = useAppStore((s) => s.bindingsByAgent)
  const credentialsByBinding = useAppStore((s) => s.credentialsByBinding)
  const refreshAgent = useAppStore((s) => s.refreshAgent)
  const refreshBindings = useAppStore((s) => s.refreshBindings)
  const createBackendBinding = useAppStore((s) => s.createBackendBinding)
  const revokeBackendBinding = useAppStore((s) => s.revokeBackendBinding)
  const issueCredential = useAppStore((s) => s.issueCredential)
  const revokeCredential = useAppStore((s) => s.revokeCredential)
  const revokeAgent = useAppStore((s) => s.revokeAgent)
  // 列表模式下"点了哪个 Agent"仍然只是一次跳转；这里只留一个**兜底选中**（主页模式不用它）。
  const [fallbackSelectedId, setFallbackSelectedId] = useState('')
  const [revokeArmed, setRevokeArmed] = useState(false)
  const [revokeError, setRevokeError] = useState<string | null>(null)
  const [bindingLabel, setBindingLabel] = useState('')
  const [bindingBusy, setBindingBusy] = useState(false)
  const [credentialBusy, setCredentialBusy] = useState(false)
  const [oneTimeToken, setOneTimeToken] = useState<string | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)
  const [profile, setProfile] = useState<ProfileView | null>(null)
  const [profileLoaded, setProfileLoaded] = useState(false)
  const [profileLoading, setProfileLoading] = useState(false)
  const [profileError, setProfileError] = useState<string | null>(null)
  // ★ **首屏第一动作：去跟它说话**（2026-10-20 T4）。判据在 `api/directRoom.ts` 一处
  //   （`/agent/:id` 那颗「发消息」用的是同一个函数），写入口仍然只有 store 那一个。
  const allRooms = useRooms()
  const me = useAppStore((s) => s.me)
  const createDirectRoom = useAppStore((s) => s.createDirectRoom)
  const selfId = resolveSelfId(me?.participant.id)
  const [talkBusy, setTalkBusy] = useState(false)
  const [talkError, setTalkError] = useState<string | null>(null)
  /**
   * 「接线（高级）」的展开态。`null` = 人还没动过 ⇒ **按处境推导**：
   * - 有生效的接入 ⇒ 收起（日常看不到它，但它就在那儿）；
   * - **还没有生效的接入**（含读不到）⇒ 展开 —— 那正是"要人去同意"的那一步
   *   （2026-10-08 那条裁决的第①类），**默认折叠不许把要你动手的那一格藏起来**。
   * 一旦人点过，就听人的（不再被数据翻动）。
   */
  const [wiringChoice, setWiringChoice] = useState<boolean | null>(null)
  // ⚠ 2026-09-29 主人裁「这一页不再代笔」之后，这里**只留读**：原先那一套编辑态
  // （draft / saving / saved、头像的 busy/error/saved/preview 与 objectURL 回收）
  // 随编辑框一起删掉了 —— 它们唯一的消费者就是那个框。
  // ⚠ 判据是 `isRealWorldSession`（**唯一权威**，见 `api/prefs.ts`）：演示通道也是
  // `mode === 'core'`，拿 mode 当"真实世界"会把演示通道一起掐掉。
  const profileReal = isRealWorldSession({ mode, token })
  // 撤销能力同样是**能力探测**而不是模式表：core 直连实现了 `revokeAgent`，
  // mock 兜底没有 —— 没有实现就整块不渲染。
  const revokeSupported = supportsAgentRevoke(getApi())
  // TASK_72 ⑥：绑定区 / 凭据区 / 选中时拉详情，三处原本都拿 `mode === 'core'` 当判据。
  // `ConnectionMode` 只有一个取值，而**演示通道也活在 `mode === 'core'` 里** ⇒ 那三处
  // 在演示态照样按 core 渲染（空区）。改成与 `revokeSupported` 同形的**能力探测**：
  // 问的是"服务器有没有这个数据面"，不是"是不是真实世界"。
  const bindingsSupported = supportsAgentBindings(getApi())
  const credentialsSupported = supportsAgentCredentials(getApi())
  const agentDetailSupported = supportsAgentDetail(getApi())
  // ⚠ 2026-09-26 立：**读得到 ≠ 改得动**。上面两个探的是**读面**，而按钮走的是**写面** ——
  // 演示通道（`MockApi` 有读无写）里「创建/撤销接入」「签发/撤销凭证」四个按钮
  // 全都渲染出来、点下去什么也不发生（`store` 那几支当时是静默 `return`）。
  // 写能力单独探，按钮按它收口；`store` 那几支也改成明确抛错（双保险）。
  const bindingsWritable = supportsAgentBindingWrite(getApi())
  const credentialsWritable = supportsAgentCredentialWrite(getApi())

  const activeBinding = (list: BackendBindingView[] | undefined): BackendBindingView | undefined =>
    list?.find((b) => b.status === 'active')

  // `OwnedAgent`（= Participant ＋ `revoked`）：列表要能说出"这一支已经撤销了"，
  // 而软撤销后参与者行仍在（契约：不硬删除、含 revoked 历史）。
  const agentList: OwnedAgent[] = ownedAgents
  // 选中哪个 Agent：主页模式**只听地址**（`/app/agents/:id`）；列表模式取第一个（那一页不展开详情）。
  const selectedId = homeMode
    ? (participantId as string)
    : fallbackSelectedId || (agentList[0]?.id ?? '')
  // ⚠ 主页模式下**不许回落**到"列表里第一个" —— 认不出地址里那个 Agent 时，页面必须说"没有这个 Agent"，
  //    拿别的 Agent 说成 Agent 是这一刀要治的病（"运行状态代表谁"）。
  // ⚠ 类型显式写成 `OwnedAgent | null`（2026-10-20 起）：这一页要读 `revoked` 那一格
  //    （T3：已撤销的 Agent 不渲染管理面），而 `Participant` 上没有它。
  //    这个 tsconfig 没开 `noUncheckedIndexedAccess`，不写的话 `agentList[0] ?? null` 会被推成
  //    非空 —— 那个"非空"是假的（列表空时它确实是 undefined），下面的空判也就会被 TS
  //    当成死代码而**失去 narrowing**。
  const agent: OwnedAgent | null = homeMode
    ? (agentList.find((a) => a.id === participantId) ?? null)
    : (agentList.find((a) => a.id === selectedId) ?? agentList[0] ?? null)
  // 绑定读数走唯一权威：旧写法是 `bindingsByAgent[agent.id] ?? []` —— 键缺失（还没加载 /
  // 读失败）会被显示成「还没有接入」，并把创建表单递上去（契约是同一 Agent 至多一个
  // active Binding ⇒ 那是诱导撞限制）。
  const bindingsReadout = agent && bindingsSupported ? bindingsOf(bindingsByAgent, agent.id) : null
  const bindings = bindingsReadout?.status === 'known' ? bindingsReadout.bindings : []
  const bindingsSealed = bindingsReadout != null && bindingsReadout.status !== 'known'
  const activeBindingId = activeBinding(bindings)?.id
  // 凭据读数走唯一权威（`credentialScopeKey` 生产 / `credentialsOf` 消费同一个模块）：
  // 旧写法是 `` credentialsByBinding[`${agent.id}:${activeBindingId}`] ?? [] `` —— 键形状
  // 一漂移，`?? []` 就把「读不到」显示成「这个绑定下没有凭据」（一条假话）。
  const credentialsReadout =
    agent && credentialsSupported && activeBindingId
      ? credentialsOf(credentialsByBinding, agent.id, activeBindingId)
      : null
  const credentialsCore = credentialsReadout?.status === 'known' ? credentialsReadout.credentials : []
  // 读数不是 known 时（还没读到 / 键形状漂移）**不许签发**：在看不见既有凭据的情况下签发，
  // 可能给同一个绑定造出第二条 active 凭据。这是 fail-closed，不是禁用功能。
  const credentialsSealed = credentialsReadout != null && credentialsReadout.status !== 'known'

  /** 列表里点一个 Agent ⇒ **去 Agent 的主页**（不再在本页展开详情）。 */
  const openAgent = (id: string) => {
    setFallbackSelectedId(id)
    navigate(`/app/agents/${id}`)
  }

  const profileErrorMessage = (error: unknown): string => {
    const code = error && typeof error === 'object' && 'code' in error ? String(error.code) : ''
    if (code === 'profile_stale') return '资料已被其他设备更新，请重新加载后再看。'
    if (code === 'agent_not_active') return '这个 Agent 已失效，当前读不到它的资料。'
    if (code === 'agent_not_found') return '没有找到这个 Agent，可能已被撤销。'
    return '资料读取失败，请检查连接后重试。'
  }

  const loadProfile = useCallback(async (agentId: string) => {
    const currentApi = getApi()
    if (!profileReal || !currentApi?.getAgentProfile) return
    setProfileLoading(true)
    setProfileLoaded(false)
    setProfileError(null)
    try {
      const result = await currentApi.getAgentProfile(agentId)
      setProfile(result.profile)
      setProfileLoaded(true)
    } catch (error) {
      setProfileError(profileErrorMessage(error))
    } finally {
      setProfileLoading(false)
    }
  }, [profileReal])

  useEffect(() => {
    if (profileReal && ownedAgents.length > 0 && agent?.id) void loadProfile(agent.id)
  }, [mode, ownedAgents.length, agent?.id, loadProfile, profileReal])

  // 进 Agent 的主页那一刻：把 Agent 那一份数据拉齐（能力探测：有哪个数据面就拉哪个，与从前同一份判据），
  // 并把上一次那位 Agent 的编辑态清干净。
  // ⚠ 依赖用 `agent?.id`（不是 `selectedId`）：列表模式下没有"选中的 Agent"要拉。
  const primedRef = useRef<string | null>(null)
  useEffect(() => {
    const id = agent?.id
    if (!homeMode || !id || primedRef.current === id) return
    primedRef.current = id
    setRevokeArmed(false)
    setRevokeError(null)
    setActionError(null)
    setOneTimeToken(null)
    setProfile(null)
    setProfileLoaded(false)
    setProfileError(null)
    if (agentDetailSupported) void refreshAgent(id)
    if (bindingsSupported) void refreshBindings(id)
    // ⚠ 能力探测那两个值是**每次渲染现算**的（`getApi()` 上的方法在不在），
    //    放进依赖会让这个 effect 每渲染都重跑 —— 而 `primedRef` 已经保证了"同一个 Agent 只拉一次"，
    //    所以这里只跟着"是哪个 Agent"走。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [homeMode, agent?.id])

  if (!agent) {
    const loading = ownedAgentsLoading
    const failed = ownedAgentsError
    // 列表模式：列表为空也要给**列表本身** —— 空态里那颗「创建第一个 Agent」住在它里面。
    // ⚠ 从前这里对列表模式也返回占位页 ⇒ 那颗按钮**永远渲染不出来**（骨架刀留下的死代码；
    //    填肉时才发现：空账号连列表页都到不了，只能靠页头那颗「＋ 添加」）。
    if (!homeMode && !loading && !failed) {
      // `agentsKnown`：走到这里说明清单**已经落定且没读失败** ⇒ 空列表就是"真的一支都没有"
      // （三步清单的第二步据此判；读不到的另两支在下面那两个 else 里，不挂这块面板）。
      return <AgentsListPanel agents={agentList} agentsKnown onOpen={openAgent} />
    }
    // 主页模式下"找不到它"必须**明说没有这个 Agent**，绝不留在这儿等一个永不出现的它，
    // 更不许拿列表里第一个顶上去（那正是"页面说不清它说的是谁"）。
    return (
      <div className="flex h-full min-h-0 flex-col bg-background">
        <div className="flex items-center gap-2 border-b border-border bg-card px-4 py-4">
          <Button
            variant="ghost"
            size="icon"
            onClick={() => navigate(homeMode ? '/app/agents' : '/app/profile')}
            aria-label={homeMode ? '返回我的 Agent' : '返回我的'}
          >
            <ArrowLeft className="size-4" />
          </Button>
          <span className="text-title">{homeMode ? 'Agent 主页' : '我的 Agent'}</span>
        </div>
        <div className="flex flex-1 items-center justify-center px-6 text-center">
          <div className="space-y-2">
            {loading ? (
              <p className="text-body">正在读取 Agent…</p>
            ) : failed ? (
              <>
                <p className="text-body text-presence-failed">暂时无法读取 Agent</p>
                <p className="text-caption text-muted-foreground">{ownedAgentsError}</p>
                <Button variant="outline" size="sm" onClick={() => void refreshOwnedAgents()}>
                  重试
                </Button>
              </>
            ) : (
              <>
                <p className="text-body">没有这个 Agent</p>
                <p className="text-caption text-muted-foreground">
                  地址里那个 Agent 不在这份列表里（可能已被撤销，或地址写错了）。
                </p>
                <Button variant="outline" size="sm" onClick={() => navigate('/app/agents')}>
                  回到我的 Agent
                </Button>
              </>
            )}
          </div>
        </div>
      </div>
    )
  }

  /**
   * ★ **已撤销的 Agent 不该有一张"可操作"的页**（2026-10-20 T3）。
   *
   * 撤销是**软撤销**：那一支仍留在所有权清单里（历史与房间成员记录不硬删），于是
   * 深链接 `/app/agents/<已撤销 id>` 从前进来的是一张**完整的管理台** —— 接入、签发 /
   * 轮换凭证、接到机器上、选模型、撤销 Agent 全都在，而它们**一件也做不成**（世界那一侧
   * 早已不接受它）。列表那一行从 2026-09-29 起就只说后果了 ⇒ 两页对同一件事给出两个答案。
   *
   * 判据只有一条：**撤销之后还剩什么能改？** 什么都改不了。所以这一页剩下的**不是**
   * "管理台的只读版"，而是**页头 ＋ 那一句后果 ＋ 一个返回**（那句与列表**同一个常量**，
   * 见 `api/agentRevoked.ts`）。判据本身是纯函数，有用例。
   *
   * ⚠ **只对"这一支那一页"成立**（`homeMode`）：列表模式下 `agent` 也会被算出来
   * （`agentList[0]` 那个兜底）—— 少了这个门，**列表页会被换成一页"已撤销"**（第一支
   * 刚好是撤销的那支时，`/app/agents` 就不再有列表与「添加」）。这一条是真机撞出来的形状。
   */
  if (homeMode && !shouldShowAgentAdmin(agent)) {
    return (
      <div className="flex h-full min-h-0 flex-col bg-background">
        <div className="flex items-center gap-3 border-b border-border bg-card px-4 py-4 md:px-6">
          <Button
            variant="ghost"
            size="icon"
            onClick={() => navigate('/app/agents')}
            aria-label="返回我的 Agent"
          >
            <ArrowLeft className="size-4" />
          </Button>
          <ParticipantAvatar participant={agent} size="sm" />
          <span className="text-title">{agent.displayName} 的主页</span>
          <AgentBadge />
        </div>
        <div className="flex flex-1 items-center justify-center px-6 py-10">
          <div className="max-w-md space-y-3 text-center">
            <p className="text-body text-destructive">{AGENT_REVOKED_CONSEQUENCE}</p>
            <p className="text-caption text-muted-foreground">
              这一页上原来能做的事（接入、凭证、接到机器上、选模型）都随撤销一起失效了 ——
              所以这里不再显示它们。
            </p>
            <Button variant="outline" size="sm" onClick={() => navigate('/app/agents')}>
              回到我的 Agent
            </Button>
          </div>
        </div>
      </div>
    )
  }

  const handleCreateBinding = async () => {
    const label = bindingLabel.trim()
    if (!label) return
    setBindingBusy(true)
    setActionError(null)
    try {
      await createBackendBinding(agent.id, label)
      setBindingLabel('')
    } catch (e) {
      setActionError(errorText(e))
    } finally {
      setBindingBusy(false)
    }
  }

  const handleIssueCredential = async (binding: BackendBindingView, credentials: CredentialView[]) => {
    setCredentialBusy(true)
    setActionError(null)
    setOneTimeToken(null)
    try {
      const active = credentials.find((c) => c.status === 'active')
      const token = await issueCredential(agent.id, binding.id, active?.id ?? null)
      if (token) setOneTimeToken(token)
    } catch (e) {
      setActionError(errorText(e))
    } finally {
      setCredentialBusy(false)
    }
  }

  const handleRevokeBinding = async (bindingId: string) => {
    setActionError(null)
    try {
      await revokeBackendBinding(agent.id, bindingId)
    } catch (e) {
      setActionError(errorText(e))
    }
  }

  const handleRevokeCredential = async (binding: BackendBindingView, credentialId: string) => {
    setActionError(null)
    try {
      await revokeCredential(agent.id, binding.id, credentialId)
    } catch (e) {
      setActionError(errorText(e))
    }
  }

  /** 撤销（两次点击确认式）：错误文案来自 `contractErrors` 的共享稳定映射。 */
  const handleRevoke = async () => {
    if (!revokeArmed) {
      setRevokeArmed(true)
      setTimeout(() => setRevokeArmed((a) => a && false), 4000)
      return
    }
    setRevokeError(null)
    try {
      await revokeAgent(agent.id)
      // 撤销掉的那个 Agent **不再存在** ⇒ 回列表（留在它的主页上会一直显示"没有这个 Agent"，像出错）。
      setFallbackSelectedId('')
      navigate('/app/agents')
      setRevokeArmed(false)
    } catch (e) {
      setRevokeError(agentRevokeErrorText(e))
      setRevokeArmed(false)
    }
  }

  /**
   * ★ **首屏第一动作：去跟它说话**（2026-10-20 T4）。
   *
   * 判据在 `api/directRoom.ts` 的 `directRoomOf` **一处**（世界侧参与者页那颗「发消息」
   * 用的是同一个函数）；写入口也只有一处（store 的 `createDirectRoom`）。
   * 这里**不**自己找房、不自己建 —— 那会是同一条判据的第二份。
   */
  const handleTalk = async () => {
    if (talkBusy) return
    const existing = directRoomOf(allRooms, selfId, agent.id)
    if (existing) {
      navigate(`/app/chat/${existing.id}`)
      return
    }
    setTalkBusy(true)
    setTalkError(null)
    try {
      navigate(`/app/chat/${await createDirectRoom(agent.id)}`)
    } catch (e) {
      // 建失败（它已撤销 / 世界不认这一对 / 网络）⇒ 如实报错，不假装进了房间。
      setTalkError(errorText(e))
    } finally {
      setTalkBusy(false)
    }
  }

  /**
   * 「接线（高级）」的**一行读数**与**展开态**（2026-10-20 T4）。
   *
   * ⚠ 读数只说这一层自己的事实（几条接入 / 凭证在不在），而且**读不到就说读不到** ——
   * 不许把"读不到"写成"还没有接入"（`bindingsOf` 的三态在那里，这里只翻成人话）。
   */
  const activeWiring = activeBinding(bindings)
  const activeCredentialCount = credentialsCore.filter((each) => each.status === 'active').length
  const credentialPart =
    activeWiring === undefined
      ? ''
      : credentialsReadout === null
        ? ''
        : credentialsReadout.status !== 'known'
          ? ' · 凭证读不到'
          : activeCredentialCount > 0
            ? ` · ${activeCredentialCount} 张凭证生效中`
            : ' · 没有生效的凭证'
  const wiringLine =
    bindingsReadout === null
      ? '这一版没有接线这一层'
      : bindingsReadout.status !== 'known'
        ? '接线读不到'
        : activeWiring === undefined
          ? '还没有接入'
          : `1 条接入${credentialPart}`
  // 人没动过 ⇒ 按处境：有生效的接入就收起，没有（或读不到）就展开（那正是要人动手的一格）。
  const wiringOpen = wiringChoice ?? activeWiring === undefined

  // 列表模式：一行一个 Agent（三格读数见 `AgentsListPanel`）＋ 就地创建。
  const list = <AgentsListPanel agents={agentList} onOpen={openAgent} />

  // 「资料」房间的内容：世界侧公共身份（core 直连那几张卡，数据源一字未动）
  // ＋ Agent 的接线（接入 / 凭据 / 撤销 —— 它们说的也是"这个 Agent 是谁"）。
  const whoSections = (
    <>
      <div className="space-y-6">
          <section className="space-y-3 rounded-xl border border-border bg-card p-5">
            <div className="flex items-center gap-2 text-title">
              <ShieldCheck className="size-4 text-muted-foreground" />
              所有权
            </div>
            <div className="space-y-1.5 text-caption text-muted-foreground">
              <div>由你创建于 {agent.since ?? '当前连接未提供创建时间'}</div>
              <div>
                Agent ID <code className="rounded bg-muted px-1.5 py-0.5 font-mono">{agent.id}</code>
              </div>
            </div>
            <div className="space-y-1.5 border-t border-border pt-3">
              <div className="text-caption text-muted-foreground">名字</div>
              {/* ⚠ 2026-09-26：这里原先是一个 `<Input defaultValue={…} />` —— **纯假控件**：
                  它长得能改、也能输入，但没有 `onChange`、没有保存、`api/core.ts` 也没有
                  改名端点。用户改了名字，界面看起来接受了，实际丢弃。
                  现在如实画成只读文本（真做改名要先有一条契约路径，那是另一刀的事）。 */}
              <div className="text-body">{agent.displayName}</div>
            </div>
          </section>

          {profileReal && (
            <section className="space-y-3 rounded-xl border border-border bg-card p-5">
              <div className="flex items-center gap-2 text-title">
                <ShieldCheck className="size-4 text-muted-foreground" />
                公共资料
                {profile && <span className="ml-auto text-caption text-muted-foreground">版本 {profile.profile_version}</span>}
              </div>
              {profileLoading ? (
                <p className="text-caption text-muted-foreground">正在读取资料…</p>
              ) : profileError && !profileLoaded ? (
                <div className="space-y-3">
                  <p className="text-caption text-presence-failed">{profileError}</p>
                  <Button variant="outline" size="sm" onClick={() => void loadProfile(agent.id)}>
                    重新加载
                  </Button>
                </div>
              ) : profileLoaded ? (
                <>
                  {/* ⚠ **2026-09-29 主人裁：这一页不再代笔。**
                      契约允许 Owner 代笔（并集：Owner **或** 该 Agent 自己），这条冻结契约一字未改。
                      但按「**表达权归主体**」，资料是**它自己的话**——它自己用 `im_write_profile` /
                      `im_write_avatar` 写（那两件今天在真机上真的能用了）。
                      给人一个编辑框，等于把主体的表达权做成了**管理项**。
                      ⇒ 这里从"可编辑表单"改成**只读展示**：客户端不再提供 Owner 代笔的入口，
                      协议里的 Owner 写路径仍在（要恢复入口是 Git 里的一笔回退，不是协议变更）。 */}
                  <div className="space-y-3 rounded-lg border border-border bg-muted/40 p-3">
                    <div className="flex items-center gap-3">
                      <div className="flex size-12 shrink-0 items-center justify-center overflow-hidden rounded-full bg-muted text-body text-muted-foreground">
                        {profile?.avatar ? (
                          <ImageIcon className="size-5" aria-hidden />
                        ) : (
                          <span>{agent.displayName.slice(0, 1)}</span>
                        )}
                      </div>
                      <div className="min-w-0 flex-1">
                        <div className="text-body">头像</div>
                        <div className="text-caption text-muted-foreground">
                          {profile?.avatar
                            ? `已设置 · ${avatarKindLabel(profile.avatar.content_type)} · ${formatBytes(profile.avatar.size_bytes)}`
                            : '未设置 —— 房间与主页显示首字母'}
                        </div>
                      </div>
                    </div>
                    {/* 头像字节的授权只走 Room 成员路径，Owner 这一页没有房间上下文 ⇒
                        这里**从来只显示元数据**（原先那句注释也在说这件事）。 */}
                    <p className="text-caption text-muted-foreground">
                      这里只显示头像的元数据，字节在房间里按成员可见性读取。
                    </p>
                  </div>
                  <p className="text-caption text-muted-foreground">
                    其他参与者在主页和房间里能看到的资料。
                  </p>
                  <div className="space-y-1.5">
                    <span className="text-caption text-muted-foreground">一句话介绍</span>
                    <p className="text-body">
                      {profile?.short_description ?? (
                        <span className="text-muted-foreground">还没有写</span>
                      )}
                    </p>
                  </div>
                  <div className="space-y-1.5">
                    <span className="text-caption text-muted-foreground">公开介绍</span>
                    <p className="whitespace-pre-wrap text-body">
                      {profile?.public_description ?? (
                        <span className="text-muted-foreground">还没有写</span>
                      )}
                    </p>
                  </div>
                  <p className="text-caption text-muted-foreground">
                    {profile
                      ? '这是它在世界里的公开资料，由它自己写。'
                      : '它还没有写过自己的资料。这一页只读——资料由它自己写。'}
                  </p>
                </>
              ) : (
                <p className="text-caption text-muted-foreground">
                  还没有公共资料。这一页只读——资料由它自己写。
                </p>
              )}
            </section>
          )}

        </div>
    </>
  )

  /**
   * ★ **「接线（高级）」**（2026-10-20 T4）：接入 ＋ 凭据从「资料」里搬出来、**默认折叠**。
   *
   * 三条判据（都不新造事实）：
   * 1. **它们仍然在**：2026-10-08 那条裁决的第①类是「必须由人同意的（首次跑世界、模型 key、
   *    凭据 / 撤销）」—— 折叠是**顺序**问题，不是要不要有的问题；
   * 2. **折叠头带读数**（`wiringLine`）：藏起来的是细节，不是事实；
   * 3. **没有生效的接入时默认展开**：那是唯一要人动手的处境（`wiringOpen` 的推导在那里）。
   */
  const wiringSections = (
      <div className="space-y-6">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-caption text-muted-foreground">{wiringLine}</span>
          <Button
            variant="ghost"
            size="sm"
            className="ml-auto"
            aria-expanded={wiringOpen}
            onClick={() => setWiringChoice(!wiringOpen)}
          >
            {wiringOpen ? '收起' : '展开'}
          </Button>
        </div>
        {wiringOpen && (
          <>
          <section className="space-y-3 rounded-xl border border-border bg-card p-5">
            <div className="flex items-center gap-2 text-title">
              <Link2 className="size-4 text-muted-foreground" />
              接入
            </div>
            {bindingsReadout?.status === 'unknown' && (
              <div className="text-caption text-muted-foreground">
                绑定还没读到（服务器还没给出这份数据）。读到之前这里不显示"还没有绑定"。
              </div>
            )}
            {bindings.length > 0 ? (
              <div className="divide-y divide-border">
                {bindings.map((b) => (
                  <div key={b.id} className="flex items-center gap-3 py-3 first:pt-0 last:pb-0">
                    <div className="min-w-0 flex-1">
                      <div className="text-body">{b.label}</div>
                      <div className="text-caption text-muted-foreground">
                        接入与所有权是两回事：换接入方不改变归属
                      </div>
                    </div>
                    <CoreBindingBadge status={b.status} />
                    {b.status === 'active' ? (
                      <Button
                        variant="outline"
                        size="sm"
                        className="text-destructive"
                        onClick={() => void handleRevokeBinding(b.id)}
                      >
                        解除接入
                      </Button>
                    ) : (
                      <span className="text-caption text-muted-foreground">已解除</span>
                    )}
                  </div>
                ))}
              </div>
            ) : bindingsReadout?.status === 'known' ? (
              <div className="text-caption text-muted-foreground">还没有接入</div>
            ) : null}
            {bindingsWritable && !activeBinding(bindings) && !bindingsSealed && (
              <div className="flex items-center gap-2 border-t border-border pt-3">
                <Input
                  value={bindingLabel}
                  onChange={(e) => setBindingLabel(e.target.value)}
                  placeholder="接入方标签，如 本机 Runtime"
                  aria-label="接入方标签"
                  className="bg-muted"
                />
                <Button
                  size="sm"
                  disabled={!bindingLabel.trim() || bindingBusy}
                  onClick={() => void handleCreateBinding()}
                >
                  {bindingBusy ? '创建中…' : '绑定'}
                </Button>
              </div>
            )}
          </section>

          <section className="rounded-xl border border-border bg-card p-5">
            <div className="flex items-center gap-2 text-title">
              <KeyRound className="size-4 text-muted-foreground" />
              凭证
            </div>
            {activeBinding(bindings) ? (
              <>
                {credentialsReadout?.status === 'unknown' && (
                  <div className="text-caption text-muted-foreground">
                    凭据还没读到（服务器还没给出这份数据）。读到之前这里不显示"没有凭据"。
                  </div>
                )}
                {credentialsReadout?.status === 'drifted' && (
                  <p role="alert" className="text-caption text-presence-failed">
                    凭据读取面出现异常：同一个绑定的键出现了两种形状，读取侧与写入侧已经不一致。
                    已停止显示与签发，避免把"读不到"当成"没有凭据"。
                  </p>
                )}
                <div className="divide-y divide-border">
                  {credentialsCore.map((cred) => (
                    <div key={cred.id} className="flex items-center gap-3 py-3 first:pt-0 last:pb-0">
                      <div className="min-w-0 flex-1">
                        <div className="text-body">{cred.status === 'active' ? '生效中' : '已作废'}</div>
                        <div className="text-caption text-muted-foreground">
                          签发 {cred.created_at.slice(0, 16).replace('T', ' ')}
                        </div>
                      </div>
                      {cred.status === 'active' && credentialsWritable ? (
                        <Button
                          variant="outline"
                          size="sm"
                          className="text-destructive"
                          onClick={() => void handleRevokeCredential(activeBinding(bindings)!, cred.id)}
                        >
                          作废凭证
                        </Button>
                      ) : cred.status === 'active' ? (
                        <span className="text-caption text-muted-foreground">生效中</span>
                      ) : (
                        <span className="text-caption text-muted-foreground">已作废</span>
                      )}
                    </div>
                  ))}
                </div>
                {credentialsWritable ? (
                  <>
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={credentialBusy || credentialsSealed}
                      onClick={() => void handleIssueCredential(activeBinding(bindings)!, credentialsCore)}
                    >
                      {credentialBusy ? '签发中…' : credentialsCore.some((c) => c.status === 'active') ? '轮换凭证' : '签发凭证'}
                    </Button>
                    <div className="text-caption text-muted-foreground">
                      签发成功后 token 只显示一次。<strong>撤销与轮换都是当场生效</strong>
                      ：正在跑它的那台机器会认证失败、停下来 —— 要把新 token 交给那台机器才能继续。
                    </div>
                  </>
                ) : (
                  <div className="text-caption text-muted-foreground">
                    这一层只读：能看凭据，签发与撤销不在这一层。
                  </div>
                )}
              </>
            ) : (
              <div className="text-caption text-muted-foreground">先创建一条接入才能签发凭证</div>
            )}
            {credentialsSupported && oneTimeToken && (
              <OneTimeToken token={oneTimeToken} onDismiss={() => setOneTimeToken(null)} />
            )}
            {credentialsSupported && actionError && (
              <p className="text-caption text-presence-failed">{actionError}</p>
            )}
          </section>

          {/* ⚠ **刀2a 删掉了这一区的「模型」卡**：它读的是 fixtures 的供应商目录（真实世界里
              `supportsAgentModels` 恒假 ⇒ 从来不渲染），点了什么也不写 —— 一份**配不了任何东西
              的假界面**。Agent"用哪条模型源"现在归**执行端**那一页的「模型」房间
              （`/app/runtimes/:entryId#model`）：读那台机器的真值 + 换绑入口。
              原来那条"登记在施工报告里"的遗留（`ModelSelectSheet` / `modelId` /
              `modelSheetOpen` 三个"等数据面"）随之关闭：它们不再挂在这一页上。 */}

          {/* 接线（高级）的右括号（见上一条）：`wiringOpen` 收起时整块不渲染。 */}
          </>
        )}
      </div>
  )

  /**
   * ★ **危险操作**（2026-10-20 T4）：撤销 Agent 移到**页尾**，并单独起一节。
   *
   * 撤销是**不可恢复**的（软撤销之后它不再接收事件、绑定与凭证一并作废、也接不上机器）——
   * 这样的一件事不该挨着"看它一眼"。渲染判据照旧是能力探测（`revokeSupported`），不是模式表：
   * core 直连 `DELETE /v1/agents/{id}`（im 服务端既有软撤销端点）经 `revokeAgent` 到达；
   * 没有这个数据面的传输层上，放开只会得到一个点了必然报错的空区（TASK_72 ⑥ 同一律）。
   */
  const dangerSections = revokeSupported && (
            <section className="rounded-xl border border-destructive/40 bg-card p-5">
              {/* 后果说明**不在这一块里重写**：它就是这一节 `SectionFrame` 的 `subject`
                  （撤销 Agent 是不可恢复的，那句后果只该有一处）。这里只放动作。 */}
              <Button
                variant="destructive"
                className="w-full"
                onClick={() => void handleRevoke()}
              >
                {revokeArmed ? '再次点击确认撤销' : '撤销 Agent'}
              </Button>
              {revokeError && <p className="mt-2 text-caption text-presence-failed">{revokeError}</p>}
            </section>
  )

  // 列表模式：就一张列表（点一个 Agent ⇒ `openAgent` 带 Agent 去主页）。
  if (!homeMode) return list

  /**
   * **主页模式：这一支的「对象首页」**（2026-10-20 T4 改职责）。
   *
   * 从前这一页是**管理台**：一进来先看到"所有权 / 接入 / 凭据 / 撤销"，而"它现在怎么样"
   * 藏在最后一段。判据换了：**一个对象的主页，第一动作是去跟它说话**，其余按顺序退后 ——
   *
   * | 位置 | 说什么 | 为什么不更前/更后 |
   * |---|---|---|
   * | 页头 | 头像 ＋ 名字 ＋ **发消息 / 在世界里看** | 第一动作是"说话"；"它在世界里什么样"是链接，**不在这页重画一遍** |
   * | ① 「机器」 | 它住在哪台 ＋ 它现在有没有在跑 ＋ 接上 / 选模型 | 这是首屏那句 health 的**唯一出处**（同源、不复述） |
   * | ② 「资料」 | 所有权 ＋ 公开资料（只读） | 世界侧公共事实 |
   * | ③ 「接线（高级）」 | 接入 ＋ 凭据（**默认折叠**，头上有读数） | 2026-10-08 裁决第①类：必须在，但不必在最前 |
   * | ④ 「危险操作」 | 撤销 Agent（不可恢复） | 页尾，一次误点代价太大 |
   */
  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      <div className="flex flex-wrap items-center gap-3 border-b border-border bg-card px-4 py-4 md:px-6">
        <Button
          variant="ghost"
          size="icon"
          onClick={() => navigate('/app/agents')}
          aria-label="返回我的 Agent"
        >
          <ArrowLeft className="size-4" />
        </Button>
        <ParticipantAvatar participant={agent} size="sm" />
        <span className="text-title">{agent.displayName} 的主页</span>
        <AgentBadge />
        {/* 首屏那两个动作（都是**出口**，不是新的写入口）：发消息走 store 那一个建/进动作，
            「在世界里看」只是去 `/agent/:id`（那边才是"它在世界里什么样"的唯一版面）。 */}
        <div className="ml-auto flex items-center gap-2">
          <Button size="sm" className="gap-1.5" disabled={talkBusy} onClick={() => void handleTalk()}>
            <MessageSquarePlus className="size-4" aria-hidden />
            {talkBusy ? '正在建会话…' : '发消息'}
          </Button>
          <Button
            size="sm"
            variant="outline"
            className="gap-1.5"
            onClick={() => navigate(`/agent/${agent.id}`)}
          >
            <ExternalLink className="size-4" aria-hidden />
            在世界里看
          </Button>
        </div>
      </div>
      {talkError && (
        <p role="alert" className="border-b border-border bg-card px-4 pb-3 text-caption text-presence-failed md:px-6">
          {talkError}
        </p>
      )}
      <AgentHome
        name={agent.displayName}
        machine={<AgentMachine agentId={agent.id} />}
        who={whoSections}
        wiring={wiringSections}
        danger={dangerSections}
      />
    </div>
  )
}

/**
 * **Agent 主页第二段：「机器」——主语是机器**（2026-09-29 补）。
 *
 * ## 为什么补这一段
 *
 * 在这一段之前，这一页是个**死胡同**：出口只有「返回我的 Agent」与「撤销 Agent」。
 * 而「我的 Agent」列表每一行上摆着「状态 / 模型 / 最近活动」三格，点进来**一个字都不说**
 * 它在哪台机器上 —— 用户要问"它现在在跑吗"，得退回去、自己想起来去「我的执行端」、
 * 再从机器那一头找。机器页开口就说「上面跑的是 X」：**答案一直在，只是只从另一端说**。
 *
 * ## 边界（两条，都不是审美）
 *
 * 1. **主语是机器**：这一段说的事实（哪台机器在跑它）来自**家**，与上面「资料」那一段
 *    （世界侧的公共事实，来自 core）**来源不同**。判据是 `routeRegistry.ts` 文件头那条
 *    「判据管**页面主体**，不管有没有引用对方」—— 机器页引用 Agent 的名字是正当的，
 *    反过来同样正当：**"哪台机器在跑它"是机器的属性**。
 * 2. **不复述运行状态**：在不在跑 / 醒过几次，**唯一权威**是机器页
 *    （`/app/runtimes/:entryId`）那几个房间。在这里再算一遍就是第二个答案
 *    （`homes.ts` 的 `mindReadoutFor` 头上记着这条教训）。所以本段只回答"住哪台" ＋ 给出口。
 *
 *    ⚠ **2026-10-12 修订（本条第三个例子被摘掉：`用哪条模型` 不再属于这一条）**：主人当次点头 ——
 *    "模型"这一格要**在这一页选**（绑定是**这一支自己的事实**，而这一页就是"这一支"的控制面：
 *    接入、凭证都在这儿）。**判据不是"同一句话只能出现在一个页面上"，而是"不许有第二个答案"** ——
 *    所以这一块照 2026-10-06 那条修订的三条纪律落地：① **归属判据不变**（认领到哪台就用哪台）；
 *    ② **读数只有一处来路**（`useHomeEntries` 那份 `config.view`，与机器页同一份）；
 *    ③ **拼句只有一处**（`api/homeConfig.ts` 的 `residentModelLine`，与机器页同一个函数）。
 *    机器页那一间留着的是**另一件事**："这台机器上各支分别用哪条"（多支并看），不是第二个答案。
 *
 *    ⚠ **2026-10-14 修订（这一格的"什么时候能选"改了：选择先于接上）**：主人当次撞上的是
 *    **互等** —— 机器上有两条来源，接它时壳按"选哪一条 = 花谁的钱，不替它决定"停下来；
 *    而"选"的那一格要求"某台机器已认领它"（`claimed`），它还没被接上 ⇒ 那一格根本不摆。
 *    ⇒ 判据改成 **`modelChoiceMachine` 那三档**（纯逻辑、有用例）：认领到就那台；没认领
 *    就指**本机**（app 里"接上"只指本机），并且要求**本机那台的清单读得到**（读不到就没得选）；
 *    还没裁决（`resolution === null`）不摆。**"写在哪一支上"没变**（永远是它自己那一支），
 *    "这一笔发给哪台机器"由那个函数一处裁决 —— 页面不自己判一次。
 *    ⚠ **张冠李戴那一条照旧成立**：本页仍**不读**任何"别人的机器读数"（第二句那条负对照
 *    盯着的是**账本/健康**那一句，它仍然只有 `claimed` 才渲染；`scripts/verify-ui.mjs` 里那条
 *    负对照没有放宽）。
 *
 *    ⚠ **2026-10-06 修订（本条第二句被收窄，第一句不动）**：`claimed` 那一档现在多一句
 *    **"机器在不在跑 ＋ 上次怎么样"**（主人当次点头）。它**不是第二个答案**，三条判据：
 *    ① 拼句只有一处（`api/agentHealth.ts` 的 `agentHealthReadout`）——**与「我的 Agent」列表
 *    那一行是同一个函数**，本页不重写一个 if；② 读数只有一处来路（`useHomeEntries`）；
 *    ③ **逐次明细仍然只有机器页有**（本页给一句汇总，不给活动列表、不给系统分类）。
 *    要防的是"两个调用点各拼一遍 ⇒ 两个答案"（`mindReadoutFor` 那条教训），不是"同一句话
 *    只能出现在一个页面上"——列表那一行早就在说同一句话了。
 *    ⚠ 还有一条**归属**判据：**只有裁决过"这台机器跑的就是它"（`claimed`）时才渲染这一句**。
 *    `unknown-id` / `elsewhere` / `unreachable` 三档不给 —— 那台的账本读数**未必是这一支的**，
 *    摆上来就是张冠李戴（`scripts/verify-ui.mjs` 有一条负对照盯着这三档）。
 *
 * 四态（读不到 / 说不准 / 没有机器在跑它 / 说得出住址）的**人话都在 `api/agentMachine.ts`**
 * —— 那是纯逻辑，有 `node --test` 用例钉着；这里只做组合与跳转，不在这里就地写 if。
 */
function AgentMachine({ agentId }: { agentId: string }) {
  const navigate = useNavigate()
  // 与「我的 Agent」列表**同一份**读数（`useHomeEntries`）：两处各拉一次，谁也不替谁多打口子
  // —— 但列表与主页**不会同时在场**（列表模式在下面提前 return），所以没有重复取数。
  const { state, retry } = useHomeEntries()
  const readings = state.kind === 'ready' ? agentHomeReadings(state.readings) : null
  const resolution = readings === null ? null : resolveAgentHome(readings, agentId)
  const readout = agentMachineReadout({ resolution })
  // 只有"某台机器报的就是它"时才读那台的账本 —— 判据在 `agentHealthForClaimedMachine`
  // 里（有离线用例），页面不自己判一次：那读数未必是这一支的（见上面那条归属判据）。
  const health = agentHealthForClaimedMachine({ resolution })
  // ★ **它用哪一条模型**（2026-10-12 主人点头：这一格回到 Agent 页 —— 绑定是**这一支自己的事实**，
  //   而"换"这件事本来就该在"这一支"的控制面上做；2026-10-14 再往前挪一格：**选择先于接上**）。
  //   三条纪律照旧：
  //   ① **归属判据不变**：认领到哪一台就用哪一台的读数（`claimed` 之外一律指**本机**，
  //      因为 app 里"接上"只指本机）；② **读数只有一处来路**（`useHomeEntries` 的那份
  //      `config.view`，与机器页同一份）；③ **人话只有一处**（`api/homeConfig.ts` 的
  //      `residentModelLine` / `unhomedModelLine`，按它住没住下分档）。
  const localEntry = state.kind === 'ready' ? (state.entries.find((each) => each.local) ?? null) : null
  const localReading =
    state.kind === 'ready' && localEntry !== null
      ? (state.readings.find((each) => each.entry.id === localEntry.id) ?? null)
      : null
  const choice = modelChoiceMachine({
    resolution,
    local:
      localEntry === null
        ? null
        : { entry: localEntry, configReadable: localReading?.config.view != null },
  })
  const choiceEntry = choice?.entry ?? null
  const homed = choice?.homed ?? false
  const configReading =
    choiceEntry === null
      ? null
      : state.kind === 'ready'
        ? (state.readings.find((each) => each.entry.id === choiceEntry.id) ?? null)
        : null
  const [modelOverride, setModelOverride] = useState<HomeConfigView | null>(null)
  const [modelBusy, setModelBusy] = useState(false)
  const [modelError, setModelError] = useState<string | null>(null)
  /** **解除指定**的确认门口（第一下只亮后果，不直接动手）—— 2026-10-15 从配置页搬过来。 */
  const [unbindConfirm, setUnbindConfirm] = useState(false)
  const choiceId = choiceEntry?.id ?? null
  useEffect(() => {
    // 换了机器（或它刚被接上）⇒ 上一份"刚换完"的读数作废（它是别的处境的读数）。
    setModelOverride(null)
    setModelError(null)
  }, [choiceId, homed])
  const configView = modelOverride ?? configReading?.config.view ?? null
  const modelEntry = configReading?.entry ?? choiceEntry
  const modelReadout = configView === null ? null : residentModelReadout(configView, agentId, homed)
  const modelLine =
    modelReadout === null ? null : homed ? residentModelLine(modelReadout) : unhomedModelLine(modelReadout)
  /** 换这一支用的那条（`null` = 解除，回到环境变量那一路）。**只动这一支**（`agentId`）。 */
  const switchModel = async (id: string | null) => {
    if (!modelEntry || modelBusy) return
    setModelBusy(true)
    setModelError(null)
    try {
      setModelOverride(await bindHomeSource(id, modelEntry, agentId))
    } catch (error) {
      setModelError(
        error instanceof HomeConfigError ? error.message : '没能换过来（那台机器没有响应）',
      )
    } finally {
      setModelBusy(false)
    }
  }
  /**
   * ★ **具体用哪个型号 / 什么思考档**（2026-10-14 支级覆盖那一刀）。
   *
   * 三态照 `bindHomeSource` 那份：**不给** = 不动、**`null`** = 清掉（回到条目里的）、
   * **字符串** = 写成它。写入的是**这一支自己的那两格**（钥匙仍归机器）。
   */
  const setOverride = async (patch: { model?: string | null; reasoningEffort?: string | null }) => {
    if (!modelEntry || modelReadout?.kind !== 'bound' || modelBusy) return
    setModelBusy(true)
    setModelError(null)
    try {
      setModelOverride(
        await bindHomeSource(modelReadout.entry.id, modelEntry, agentId, patch),
      )
    } catch (error) {
      setModelError(
        error instanceof HomeConfigError ? error.message : '没能改过来（那台机器没有响应）',
      )
    } finally {
      setModelBusy(false)
    }
  }
  // 这一支**现在选的是哪条**（用于候选行上标出"正在用 / 已经选好"）。
  const chosenId = modelReadout?.kind === 'bound' ? modelReadout.entry.id : null
  // ★ **"这条钥匙能调哪些型号"**：**按需拉**（点一下才问对方一次）—— 绝不在进页面时自动打。
  //   拉的是**当前选的那条来源**（`chosenId` 是清单条目 id；`modelEntry` 是"哪台机器"）。
  const [catalog, setCatalog] = useState<{ forEntry: string; probe: HomeConfigProbe } | null>(null)
  const [catalogBusy, setCatalogBusy] = useState(false)
  const [catalogError, setCatalogError] = useState<string | null>(null)
  const loadCatalog = async (sourceId: string) => {
    if (!modelEntry || catalogBusy) return
    setCatalogBusy(true)
    setCatalogError(null)
    try {
      setCatalog({ forEntry: sourceId, probe: await fetchHomeCatalog(sourceId, modelEntry) })
    } catch (error) {
      // 门拒绝（条目刚被删 / 门读不出来）⇒ 如实说；**连不上供应商是读数**（`probe.ok = false`），
      // 那一种不走这里（它照样是一份读数，页面照原样显示）。
      setCatalogError(
        error instanceof HomeConfigError ? error.message : '没能问到那一家（那台机器没有响应）',
      )
    } finally {
      setCatalogBusy(false)
    }
  }
  const catalogProbe = catalog !== null && catalog.forEntry === chosenId ? catalog.probe : null
  /**
   * ★ **可以让它去哪台机器上跑**（2026-10-15 远程接上那一刀）。
   *
   * 判据全在 `api/remoteAttach.ts` 的 `machineCandidates`（纯函数、有用例）—— 这一页只渲染。
   * 三件事在这一格里同时成立，缺一条就会混：
   * ① **归属**：它现在在哪台跑，由**各台自己报**（那台的状态口 `residents`）；"哪台愿意让它去"
   *    由**那台自己报的那一格**（`remoteControl`）—— 本页不替任何一台说话；
   * ② **边界**：点到哪一行，动的就是**那一台**（本机走壳那只手，外地走它自己的 `/home-control/attach`）；
   * ③ **直觉**：按钮上的字**带机器名**（「接到 <名字> 上」），而「接到**这台**机器上」只留给本机那一档。
   */
  const machines =
    state.kind === 'ready'
      ? machineCandidates({
          agentId,
          machines: state.readings.map((each) => ({
            entry: each.entry,
            config: each.config.view,
            // ⚠ 那台报的住户里，`agentId` **可能读不到**（`null`/缺）—— 那是"这一行读不出来"，
            // 不是"这一支不在这儿"：所以只把**读得出的 id** 收进来，`null` 一格留在原处
            // （整格的 null ── "那台的状态读不到" ── 与它不同，判据见 `machineCandidates`）。
            residents:
              each.status.view?.residents === undefined
                ? null
                : each.status.view.residents
                    .map((resident) => resident.agentId)
                    .filter((id): id is string => typeof id === 'string' && id !== ''),
          })),
        })
      : []
  const [attachBusy, setAttachBusy] = useState<string | null>(null)
  const [attachNote, setAttachNote] = useState<{
    entryId: string
    text: string
    bad: boolean
  } | null>(null)
  /** 在那**一台**上接上这一支（动作的主语是那一行指名的机器）。 */
  const attachOn = async (candidate: MachineCandidate) => {
    if (attachBusy !== null) return
    setAttachBusy(candidate.entry.id)
    setAttachNote(null)
    try {
      const done = await attachAgentOnMachine(agentId, candidate.entry)
      setAttachNote({ entryId: candidate.entry.id, text: attachOutcomeLine(done), bad: false })
      // 接完重读一遍：那一行自己会翻成"它现在在这台机器上跑"（读数只有一处来路）。
      retry()
    } catch (error) {
      const text =
        error instanceof HomeConfigError
          ? [error.message, ...error.lines].join('\n')
          : '没能接上（那台机器没有响应）'
      setAttachNote({ entryId: candidate.entry.id, text, bad: true })
    } finally {
      setAttachBusy(null)
    }
  }
  // 语义（`kind`）→ 令牌类只在这一处映射（组件层唯一映射，与 `status-tone` 同款纪律）。
  const TONE: Record<AgentMachineKind, string> = {
    reading: 'text-muted-foreground',
    claimed: 'text-foreground',
    'no-id': 'text-muted-foreground',
    elsewhere: 'text-muted-foreground',
    // 「读不到」是**看不见**，不是"它停了" ⇒ 橙档（与执行端那一侧的判据同一档）。
    unreachable: 'text-presence-timeout',
  }
  return (
    <SectionFrame
      id="machine"
      title="机器"
      icon={<Server className="size-4 text-muted-foreground" aria-hidden />}
      subject="它住在哪台机器上、那台机器现在怎么样（机器上的数据，不在服务器）。"
    >
      <div className="space-y-3 rounded-xl border border-border bg-card p-5">
        <p className={cn('text-body', TONE[readout.kind])}>{readout.text}</p>
        {/* 第二句：**机器在不在跑 ＋ 上次怎么样**（`claimed` 才有，见上面那条归属判据）。
            文案与色档都来自别处（`agentHealth.ts` ＋ `status-tone.ts`），这里只渲染。 */}
        {health && (
          <p role="status" className={cn('text-caption', READOUT_TONE_TEXT[health.tone])}>
            {health.text}
          </p>
        )}
        {/* ★ **它用哪一条模型**（2026-10-12；2026-10-14「选择先于接上」）：
            这一支一行、就在这一支的页面上选 —— **它还没被接到这台机器上时也摆**
            （那时选的是"接上之后用哪条"：绑定的落点就是它的环境根，选择可以先落盘）。
            钥匙清单归机器（那一页管增删改），**用哪把归这一支** —— 所以这里只做"选"。 */}
        {choiceEntry !== null && (
          <div className="space-y-2 border-t border-border pt-3">
            <div className="text-micro text-muted-foreground">
              {homed
                ? '它用哪一条模型（这台机器上配好的钥匙里选）'
                : '它还接在这台机器外面 —— 先给它选好用哪一条，接上就用它'}
            </div>
            {configView === null ? (
              <p className="text-caption text-muted-foreground">
                {configReading?.config.failure ?? '这一格读不到'} —— 读不到不等于"没配"。
              </p>
            ) : (
              <>
                <div className="flex flex-wrap items-center gap-2">
                  <span className="text-body">{modelLine}</span>
                  {modelReadout?.kind === 'bound' &&
                    (unbindConfirm ? (
                      /* ★ **解除要先亮警示再动手**（2026-10-15：这一族动作从配置页搬到**这一支**
                         的页面上 —— 那三件事跟着搬，**约束不变**）：缺模型就停下、app 不继承这台
                         机器的环境变量、以及怎么恢复。 */
                      <div className="w-full space-y-2 rounded-lg border border-destructive/40 bg-card px-3 py-2">
                        <p className="text-caption text-destructive">
                          解除之后，这一支下次运行会因为没有模型来源而停下（app 形态不继承这台机器的
                          环境变量：没指定就是没配）；重新给它指定一条就能恢复。
                        </p>
                        <div className="flex flex-wrap items-center gap-2">
                          <Button
                            variant="destructive"
                            size="sm"
                            disabled={modelBusy}
                            onClick={() => {
                              setUnbindConfirm(false)
                              void switchModel(null)
                            }}
                          >
                            确认解除
                          </Button>
                          <Button
                            variant="ghost"
                            size="sm"
                            disabled={modelBusy}
                            onClick={() => setUnbindConfirm(false)}
                          >
                            取消
                          </Button>
                        </div>
                      </div>
                    ) : (
                      <Button
                        variant="ghost"
                        size="sm"
                        className="ml-auto shrink-0"
                        disabled={modelBusy}
                        onClick={() => setUnbindConfirm(true)}
                      >
                        {homed ? '解除指定' : '取消这条选择'}
                      </Button>
                    ))}
                </div>
                {/* ★ **候选直接摆出来**（2026-10-14 主人裁）：这台机器上配好的钥匙一条一行 ——
                    名字 ＋ **型号** ＋ 窗口，点哪行的「用它」就用哪条。不再让人先点下拉才看得见
                    有什么（主人当次的原话："这也没说，选择模型什么的"）。
                    ⚠ 这一列**不是**机器页那一列的复制：这里的主语是"**它**用哪条"，那一列的主语是
                    "这台机器上有哪些钥匙"（写入口也只有一个 —— 都走 `bindHomeSource`）。 */}
                <ul className="divide-y divide-border rounded-lg border border-border">
                  {configView.entries.map((each) => {
                    const current = chosenId === each.id
                    return (
                      <li key={each.id} className="flex flex-wrap items-center gap-2 px-3 py-2">
                        <span className="truncate text-body">{each.name}</span>
                        <code className="truncate font-mono text-caption text-muted-foreground">
                          {each.model}
                        </code>
                        {each.contextWindow !== null && (
                          <span className="text-micro text-muted-foreground">
                            窗口 {each.contextWindow}
                          </span>
                        )}
                        {current ? (
                          <Badge
                            variant="outline"
                            className="ml-auto shrink-0 border-presence-online/40 text-presence-online"
                          >
                            {homed ? '正在用' : '已经选好'}
                          </Badge>
                        ) : (
                          <Button
                            variant="outline"
                            size="sm"
                            className="ml-auto shrink-0"
                            disabled={modelBusy}
                            onClick={() => void switchModel(each.id)}
                          >
                            用它
                          </Button>
                        )}
                      </li>
                    )
                  })}
                </ul>
                {/* ★ **具体用哪个型号**（2026-10-14 支级覆盖）：一把钥匙常常能调好几个型号，
                    "用哪个"是**这一支**的事实（运行时一直这么认）。默认 = 用这条来源配的那个；
                    要换就从"这条钥匙能调哪些型号"里挑一个（**按需拉**：点一下才问对方一次）。 */}
                {modelReadout?.kind === 'bound' && (
                  <div className="space-y-1.5 rounded-lg border border-border bg-muted/30 p-3">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="text-caption text-muted-foreground">用哪个型号</span>
                      <code className="font-mono text-caption">
                        {modelReadout.model ?? modelReadout.entry.model}
                      </code>
                      {modelReadout.model === null && (
                        <span className="text-micro text-muted-foreground">（跟这条来源一样）</span>
                      )}
                      <div className="ml-auto flex shrink-0 items-center gap-1.5">
                        {modelReadout.model !== null && (
                          <Button
                            variant="ghost"
                            size="sm"
                            disabled={modelBusy}
                            onClick={() => void setOverride({ model: null })}
                          >
                            跟来源一样
                          </Button>
                        )}
                        <Button
                          variant="outline"
                          size="sm"
                          disabled={catalogBusy}
                          onClick={() => void loadCatalog(modelReadout.entry.id)}
                        >
                          {catalogBusy ? '正在问…' : '看这家的型号…'}
                        </Button>
                      </div>
                    </div>
                    {catalogProbe !== null && (
                      <div className="flex flex-wrap items-center gap-1.5 border-t border-border pt-1.5">
                        {catalogProbe.ok && catalogProbe.models.length > 0 ? (
                          catalogProbe.models.map((name) => (
                            <Button
                              key={name}
                              variant={modelReadout.model === name ? 'secondary' : 'ghost'}
                              size="sm"
                              className="font-mono"
                              disabled={modelBusy}
                              onClick={() => void setOverride({ model: name })}
                            >
                              {name}
                            </Button>
                          ))
                        ) : (
                          <span className="text-caption text-muted-foreground">
                            {catalogProbe.ok
                              ? '这家没列出任何型号（不是它的错：有的接口不给目录）—— 那就按它配的那个用。'
                              : `没能问到这家${
                                  catalogProbe.reason === 'timeout' ? '（超时）' : '（连不上）'
                                }—— 这不影响已经配好的。`}
                          </span>
                        )}
                      </div>
                    )}
                    {catalogError && (
                      <p className="text-caption text-presence-failed">{catalogError}</p>
                    )}
                  </div>
                )}
                {/* ★ **思考档**（同一刀）：七档封闭表 ＋ "跟来源一样"。 */}
                {modelReadout?.kind === 'bound' && (
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="text-caption text-muted-foreground">思考档</span>
                    <span className="text-caption">
                      {modelReadout.reasoningEffort ?? '跟来源一样'}
                    </span>
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button variant="ghost" size="sm" className="ml-auto shrink-0" disabled={modelBusy}>
                          改思考档…
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        <DropdownMenuItem
                          disabled={modelReadout.reasoningEffort === null}
                          onSelect={() => void setOverride({ reasoningEffort: null })}
                        >
                          跟来源一样
                        </DropdownMenuItem>
                        {REASONING_EFFORTS.map((effort) => (
                          <DropdownMenuItem
                            key={effort}
                            disabled={modelReadout.reasoningEffort === effort}
                            onSelect={() => void setOverride({ reasoningEffort: effort })}
                          >
                            {effort}
                          </DropdownMenuItem>
                        ))}
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>
                )}
                {/* ★ **"还没有配好的钥匙"这句必须带一个能走的出口**（2026-10-20 T1 ⑤）。
                    从前它是两句**指路的话**（"去那台机器的「模型」那一间配一条"），
                    没有可点的东西 —— 读到"缺什么"却走不到能补它的地方（诊断句没有下一步
                    与死胡同同族，与列表行 `ModelSourceExit` 那条判据是同一件事）。
                    目的地与那个出口**同一个**：这台机器的「模型」那一间（`#model`）；
                    文案也不新造词 —— 按钮上就是列表行那一句「去配模型源」。 */}
                {configView.entries.length === 0 && (
                  <div className="flex flex-wrap items-center gap-2">
                    <p className="text-caption text-muted-foreground">
                      {homed
                        ? '这台机器上还没有配好的钥匙。'
                        : '这台机器上还没有配好的钥匙 —— 先配一条，回来就能选。'}
                    </p>
                    {modelEntry !== null && (
                      <Button
                        variant="outline"
                        size="sm"
                        className="gap-1.5"
                        onClick={() => navigate(`/app/runtimes/${modelEntry.id}#model`)}
                      >
                        <Server className="size-3.5" aria-hidden />
                        去配模型源
                      </Button>
                    )}
                  </div>
                )}
                <p className="text-micro text-muted-foreground">
                  {!homed ? (
                    <>接上它之后，它第一次活动就用这一条。</>
                  ) : modelReadout?.kind === 'absent' ? (
                    /* ★ 2026-10-15：**接上与选模型是两件事** —— 它可能已经被接在这台机器上、
                       但还没选过模型（那一档在 app 里起不来）。这里指下一步，不替它断言形态。 */
                    <>它还没选模型 —— 在下面挑一条；选好之后它才会起来。</>
                  ) : (
                    <>
                      改动只影响<strong>下一次活动</strong>：正在进行的那一次不受影响。
                    </>
                  )}
                </p>
              </>
            )}
            {modelError && (
              <p role="alert" className="text-caption text-presence-failed">
                {modelError}
              </p>
            )}
          </div>
        )}
        {/* ★ **可以让它去哪台机器上跑**（2026-10-15 远程接上那一刀）。
            主语在每一行上（机器名），动作只动那一台；本机那一档与从前一字不变。 */}
        {machines.length > 0 && readout.kind !== 'reading' && (
          <div className="mt-3 space-y-2 border-t border-border pt-3">
            <div className="text-caption text-muted-foreground">可以让它去哪台机器上跑</div>
            <ul className="divide-y divide-border rounded-lg border border-border">
              {machines.map((candidate) => (
                <li key={candidate.entry.id} className="flex flex-wrap items-center gap-2 px-3 py-2">
                  <span className="truncate text-body">
                    {candidate.entry.name || candidate.entry.id}
                  </span>
                  <span className="truncate text-caption text-muted-foreground">
                    {candidate.line}
                  </span>
                  {candidate.action !== null && (
                    <Button
                      variant="outline"
                      size="sm"
                      className="ml-auto shrink-0"
                      disabled={attachBusy !== null}
                      onClick={() => void attachOn(candidate)}
                    >
                      {attachBusy === candidate.entry.id ? '正在接…' : candidate.action.label}
                    </Button>
                  )}
                </li>
              ))}
            </ul>
            <p className="text-micro text-muted-foreground">{ONE_MACHINE_NOTE}</p>
            {attachNote !== null && (
              <p
                role={attachNote.bad ? 'alert' : undefined}
                className={
                  attachNote.bad ? 'text-caption text-presence-failed' : 'text-caption text-presence-online'
                }
              >
                {attachNote.text}
              </p>
            )}
          </div>
        )}
        {/* 还在读的时候不给按钮：还没有可跳的目标，给了就是让用户点一个不知道去哪的东西。 */}
        {readout.kind !== 'reading' && (
          <Button
            variant="outline"
            size="sm"
            className="gap-1.5"
            onClick={() =>
              navigate(
                readout.target ? `/app/runtimes/${readout.target.entryId}` : '/app/runtimes',
              )
            }
          >
            <Server className="size-3.5" />
            {readout.target ? '去那台机器' : '去我的执行端'}
          </Button>
        )}
      </div>
    </SectionFrame>
  )
}

/**
 * **Agent 主页**（`/app/agents/:participantId`）——2026-09-25 组织轴刀之后只剩**两间**，
 * 2026-09-28 **服务层退役**之后只剩**一间**。
 *
 * ## 为什么瘦了
 *
 * 原先这里有四间（是谁 / 家 / 日子 / 脑子），而其中三间读的是**机器级事实**
 * （在不在跑、醒过几次、用哪条模型）。轴错了：那三样属于**执行端**，不属于 Agent。
 * 它们已搬到 `/app/runtimes/:entryId`（见 `pages/RuntimesPage.tsx`）。
 *
 * 然后「服务」那一间也退场了（2026-09-28：Publication / Access / 服务房整体退役
 * —— 服务声明层是**工作态**，不是 IM 事实，违反「执行过程不经过 IM」那条红线）。
 *
 * 现在这一页是**世界侧的 Agent** —— 一个 Agent 在这里就是一个 Participant：
 * 主体是它的公开资料与接线（身份、接入、凭证、撤销）。
 *
 * ⚠ **2026-09-29 修订**：本条原写「**它不读家条目**，所以也不需要"认领"（`resolveAgentHome`
 * 那套是机器优先的反方向）」，并把这句结论挂在一句**只活在注释里**的话上 ——
 * 「看它的运行与模型 ⇒ 去 `/app/runtimes`（那台机器那一页）」。那句注释**从未在 UI 里存在**：
 * 用户点进来只看得到「返回」与「撤销」，运行状态一个字都没有（页面上真正的死胡同）。
 * 现在补了**第二段**（`AgentMachine`，`#machine`）：它**读家**、**主语是机器**、
 * 只回答"它住在哪台机器上"＋一个去机器页的出口。所以"不读家条目"这句已不成立，
 * 而"认领"仍然在 `homes.ts` 一处裁决 —— 本页只是**消费**它，不重判一次。
 *
 * 看它的运行与模型 ⇒ 那一段给出口，答案在 `/app/runtimes/:entryId` 三个房间里。
 */
/**
 * **Agent 主页的外壳：四段，顺序有判据**（2026-10-20 T4 改）。
 *
 * | 段 | 锚点 | 说什么 | 为什么在这个位置 |
 * |---|---|---|---|
 * | ① 机器 | `#machine` | 住址 ＋ **它现在有没有在跑** ＋ 接上 / 选模型 | 首屏那句 health 的**唯一出处**（主语是机器，判据在 `agentMachine.ts` / `agentHealth.ts`） |
 * | ② 资料 | `#who` | 所有权 ＋ 公开资料（只读） | 世界侧的公共事实；**不重画**"它在世界里什么样"（那在 `/agent/:id`） |
 * | ③ 接线（高级） | `#wiring` | 接入 ＋ 凭据 | 2026-10-08 裁决第①类：必须在，但不必在最前；**默认折叠、头上有读数** |
 * | ④ 危险操作 | `#danger` | 撤销 Agent | 不可恢复 ⇒ 页尾 |
 *
 * ⚠ 顺序与「不复述」是同一件事：这一页**只有一处**说 health（①），
 * ②③④ 一个字节都不重复它 —— 复述就是"同一件事两个答案"。
 */
function AgentHome({
  name,
  who,
  machine,
  wiring,
  danger,
}: {
  name: string
  who: React.ReactNode
  machine: React.ReactNode
  wiring: React.ReactNode
  danger: React.ReactNode
}) {
  const { hash } = useLocation()
  // 深链接带锚点（`/app/agents/<id>#machine`）要**滚到那一间** —— 交给共用实现
  // （`lib/anchorScroll.ts`：那一节会**晚出现**、出现之后还会被上面那一段撑高**顶走**；
  //   它盯住一段时间、用户一动就交还、且有界）。读不到那个 id 就什么都不做（不报错、不跳走）。
  useEffect(() => {
    return scrollToAnchor(hash)
  }, [hash])

  return (
    <PageScroll>
      {machine}
      <SectionFrame
        id="who"
        title="资料"
        icon={<UserRound className="size-4 text-muted-foreground" aria-hidden />}
        subject={<>{name} 在世界里的资料与归属（服务器上的数据，不在本机）；想看成什么样，去「在世界里看」。</>}
      >
        {who}
      </SectionFrame>
      <SectionFrame
        id="wiring"
        title="接线（高级）"
        icon={<Link2 className="size-4 text-muted-foreground" aria-hidden />}
        subject="谁被授权替它行动、凭证是什么时候签发的（服务器上的数据）。日常不用看，接不上或要换接入方时才来。"
      >
        {wiring}
      </SectionFrame>
      <SectionFrame
        id="danger"
        title="危险操作"
        icon={<KeyRound className="size-4 text-destructive" aria-hidden />}
        subject="这一步不可恢复：撤销之后它就退出这个世界 —— 不再接收事件，接入与凭证随之失效，也接不上机器。"
      >
        {danger}
      </SectionFrame>
    </PageScroll>
  )
}
