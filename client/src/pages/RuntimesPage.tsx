import { useEffect, useState } from 'react'
import { useLocation, useNavigate, useParams } from 'react-router'
import { ArrowLeft, ChevronRight, Cpu, KeyRound, Plus, RefreshCw, Server, Sparkles } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'
import { scrollToAnchor } from '@/lib/anchorScroll'
import { useAppStore } from '@/api/store'
import { useHomeEntries, type EntryReading } from '@/components/celira/use-home-entries'
import type { HomeEntry } from '@/api/homes'
// ★ 清单**只留一个门**（2026-10-12）：管理面从 `/settings/homes` 搬到这里 ——
//   「加一台」跟着清单走（下面那颗就地开表单）、「管某一台」跟着那台机器（机器卡里展开）。
import { AddMachineForm } from '@/components/celira/AddMachineForm'
import { MachineAdminPanel } from '@/components/celira/MachineAdminPanel'
import { machineSituation, type MachineSituation } from '@/api/agentRowStatus'
import { hostStateOf } from '@/api/homeStatus'
import { modelSourceAbsent } from '@/api/homeConfig'
import {
  EntriesUnavailableCard,
  RuntimeModelSection,
  RuntimeRunSection,
  type RuntimeState,
} from '@/components/celira/Runtimes'
import { PageScroll } from '@/components/celira/SectionFrame'

/**
 * **执行端**（`/app/runtimes` 列表、`/app/runtimes/:entryId` 详情）—— 2026-09-25 组织轴刀。
 * ## 这一页的主语是**机器**，不是 Agent
 *
 * 从前 Runtime 区以 **Agent** 为轴：「我的 Agent」是列表，点一个 Agent 进它的主页，
 * 而「家 / 日子 / 脑子」三个房间读的**全是机器级事实**（在不在跑、醒过几次、用哪条模型）。
 * 于是页面必须先"认领"（遍历机器找哪台报的是这个 Agent）才说得出话，
 * 而"运行状态代表谁"这种问题页面自己答不出来 —— **轴错了**。
 *
 * 现在：**一台机器一个地址**，机器是主语；「这台机器上住着谁」是机器的一个属性，
 * 由本页说一次（`AgentLine`），不在每个房间里重复。
 *
 * ⚠ **2026-09-26 补一句**：`/app/agents`（标题「我的 Agent」）**仍然存在** ——
 * 它是**Agent 索引的执行侧视图**（`side: 'home'`：每行拼世界侧所有权 + 家侧三格读数），
 * 不在壳层导航的任何分区里（⚠ **2026-09-29 已修**：它现在是「机器」组的第二个一级入口，
 * 见 `navGroups.ts`）。两页看的是同一批机器事实，**索引不同**（机器索引 / Agent 索引）。
 * 所以本页提到它时是**引用那一页的名字**，不是拿它当这一区的轴 —— 这一区的轴是机器。
 *
 * ## 与「世界侧」的分工（两个产品、两个轴）
 *
 * | | 组织轴 | 主语 | 在哪 |
 * |---|---|---|---|
 * | 世界（IM） | **Agent 即 Participant** | 一个 Agent = 一个联系人 | `/app/contacts`、`/app/agents/:id` |
 * | 机器（Agent 产品） | **执行端机器** | 一台机器 = 一行 | **本页** |
 *
 * ⚠ **Agent 侧因此没有独立列表页**：看一个 Agent 的运行与模型，得先有机器。
 * 这是机器优先的既定代价（轴与事实一致换来的）。
 *
 * ## 三条口径
 *
 * 1. **机器只报一支 Agent**（`identity.json` 只有 `{agent: <id>}` 一格）：所以"上面住着谁"
 *    仍由 id 精确匹配得出。将来一台机器承载多支 Agent 时，这里要跟着改（那是另一刀）。
 * 2. **读不到 ≠ 没在跑**：联系不上那台机器时如实说（`Runtimes.tsx` 的诚实卡）。
 * 3. **本机那条是预置的**（钥匙为空、走同源代转）：它排在第一条，删不得。
 */
export function RuntimesPage() {
  return <RuntimeList />
}

// ---------------------------------------------------------------------------
// 列表
// ---------------------------------------------------------------------------

function RuntimeList() {
  const navigate = useNavigate()
  const { state, retry } = useHomeEntries()
  const ownedAgents = useAppStore((s) => s.ownedAgents)
  // ⚠ **这一格是"读不到 ≠ 不在"的判据**（2026-09-26 审计修）：`ownedAgents` 读失败时是 `[]`，
  // 而列表行第四态原先把"名字查不到"一律说成「**上面那支 Agent 不在这个世界里**」——
  // 于是**一次读取失败被页面断言成一句事实**。仓库自己在别处反复警告这一类
  // （`lib/plain-language.ts`：「把 unknown 说成「未接入」，就是把读不到说成一个事实」）。
  // 只有**真的读到过**（`!error && !loading`）才有资格说"不在"。
  const ownedAgentsLoading = useAppStore((s) => s.ownedAgentsLoading)
  const ownedAgentsError = useAppStore((s) => s.ownedAgentsError)
  const agentsReadable = !ownedAgentsLoading && !ownedAgentsError
  /** 「添加」那块表单开着吗（**就地开** —— 见那颗按钮上的注释）。 */
  const [adding, setAdding] = useState(false)

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      <header className="flex items-center gap-2 border-b border-border bg-card px-4 py-4">
        {/* ★ **一级入口不带返回箭头**（2026-10-11）：这一页是导航里的一级入口
            （`navGroups.ts` 的「机器」组），原先却顶着一颗 `aria-label="返回我的"`
            —— 等于告诉用户"你在「我的」下面一层"，而导航里它就在自己那一层 ✗。
            （二级页那颗还在，见下面 `DetailHeader` 的「返回我的机器」。） */}
        <span className="text-title">我的机器</span>
        <span className="ml-2 text-caption text-muted-foreground">
          {state.kind === 'ready' ? `${state.entries.length} 台` : ''}
        </span>
        <div className="ml-auto flex items-center gap-1">
          <Button variant="ghost" size="icon" onClick={retry} aria-label="重新读取">
            <RefreshCw className="size-4" />
          </Button>
          {/* ★ **正门得能加**（2026-10-11 立，2026-10-12 就地开）：这一颗**就地**开那块表单
              —— 从前它把人送到 `/settings/homes` 去开（那意味着"加一台"住在设置里 ✗，
              而机器清单是**一级入口**）。落盘与校验只有 `AddMachineForm` 一处（`saveHomeEntry`），
              这边只负责"开/关"。 */}
          <Button
            variant="ghost"
            size="sm"
            className="gap-1.5"
            onClick={() => setAdding((open) => !open)}
          >
            <Plus className="size-4" />
            添加
          </Button>
        </div>
      </header>

      <ScrollArea className="min-h-0 flex-1">
        <div className="mx-auto w-full max-w-3xl space-y-2 px-4 py-6">
          {adding && (
            <AddMachineForm
              entries={state.kind === 'ready' ? state.entries : []}
              onSaved={() => {
                setAdding(false)
                retry()
              }}
              onCancel={() => setAdding(false)}
            />
          )}
          {state.kind === 'loading' && (
            <p className="py-12 text-center text-caption text-muted-foreground">正在读取你的机器…</p>
          )}
          {state.kind === 'unavailable' && <EntriesUnavailableCard message={state.message} onRetry={retry} />}
          {state.kind === 'ready' &&
            state.entries.map((entry) => (
              <RuntimeRow
                key={entry.id}
                entry={entry}
                reading={state.readings.find((each) => each.entry.id === entry.id)}
                agentNames={Object.fromEntries(ownedAgents.map((a) => [a.id, a.displayName]))}
                agentsReadable={agentsReadable}
                onOpen={() => navigate(`/app/runtimes/${entry.id}`)}
              />
            ))}
          {state.kind === 'ready' && (
            <p className="px-1 pt-2 text-micro text-muted-foreground">
              这里列的是机器：每台机器上跑着 Agent 的执行端（进程、模型源、活动记录都在那一台）。世界侧的联系人与会话在「消息」与「联系人」里。
            </p>
          )}
        </div>
      </ScrollArea>
    </div>
  )
}

/** 一行一台机器：名字 + 地址 + **上面住着谁** + 那一格的处境。 */
function RuntimeRow({
  entry,
  reading,
  agentNames,
  agentsReadable,
  onOpen,
}: {
  entry: HomeEntry
  reading: EntryReading | undefined
  agentNames: Record<string, string>
  /** 世界侧的 Agent 列表**这次读到了吗**——没读到就无权说"不在这个世界里"。 */
  agentsReadable: boolean
  onOpen: () => void
}) {
  const view = reading?.status.view ?? null
  const agentId = view?.agent.agentId ?? null
  const agentName = agentId ? (agentNames[agentId] ?? null) : null
  // ★ **住户表**（2026-10-16）：顶层那一格只有**第一支** ⇒ 一个家住两支时，
  //   "上面跑的是谁"必须靠这一格说全（真机撞上：小可也在跑，列表只说小欣）。
  const residents = (view?.residents ?? []).map((each) => ({
    agentId: each.agentId ?? null,
    name: each.agentId ? (agentNames[each.agentId] ?? null) : null,
    hostState: hostStateOf(each.host),
  }))

  // 处境四态（**不压成两态**：读不到 ≠ 上面没 Agent）。判据已提成纯逻辑
  // （`api/agentRowStatus.ts` 的 `machineSituation`），所以它能被 `node --test` 钉住 ——
  // 这里只负责把 `reading` 摊成它要的那几个入参。
  const situation = machineSituation({
    pending: !reading,
    view,
    failure: reading?.status.failure ?? null,
    // ★ 两条新入参（2026-10-11）：**"这台机器上有没有 Agent"** 与 **"是不是老形态的家"** ——
    // 少了它们，一个新家会被说成"它没报上上面跑的是谁"（假警报）。
    agentState: view?.agent.state ?? null,
    legacyHome: view?.legacyHome === true,
    agentId,
    agentName,
    agentsReadable,
    residents,
  })

  const toneClass =
    situation.tone === 'ok'
      ? 'bg-presence-online'
      : situation.tone === 'warn'
        ? 'bg-presence-timeout'
        : 'bg-presence-silent'

  return (
    <button
      type="button"
      onClick={onOpen}
      className="flex w-full cursor-pointer items-center gap-3 rounded-xl border border-border bg-card px-4 py-3 text-left transition-colors hover:bg-muted"
    >
      <span className="flex size-9 shrink-0 items-center justify-center rounded-full bg-muted text-muted-foreground">
        <Server className="size-4" aria-hidden />
      </span>
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-2">
          <span className="truncate text-body">{entry.local ? '本机' : entry.name}</span>
          {entry.local && (
            <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-micro text-muted-foreground">
              预置
            </span>
          )}
        </span>
        <span className="mt-0.5 flex items-center gap-1.5 text-caption text-muted-foreground">
          <span className={cn('size-1.5 shrink-0 rounded-full', toneClass)} aria-hidden />
          <span className="truncate">{situation.text}</span>
        </span>
      </span>
      <ChevronRight className="size-4 shrink-0 text-muted-foreground" />
    </button>
  )
}

// ---------------------------------------------------------------------------
// 详情
// ---------------------------------------------------------------------------

/**
 * **一台机器那一页**（`/app/runtimes/:entryId`）。
 *
 * ⚠ **旧锚点要认**：`/app/runtime` 与 `/app/experience` 是刀2a 之前的一级页面，
 * 它们的深链接写作 `#home` / `#days` / `#mind`（当时那三间叫「家 / 日子 / 脑子」）。
 * 后来那三间叫**运行 / 活动 / 模型**，锚点改成 `#run` / `#activity` / `#model`
 * —— 这里做一次**旧名 → 新名**的映射，老书签照旧滚到对的那一间。
 *
 * ⚠ **2026-10-12：`#activity` 不再是一个房间**（主人裁：明细降为「运行」那一间里的下钻）
 * ⇒ `days` 与 `activity` **都映射到 `#run`**（老书签照旧落到"对的那一间"：内容就在它下面）。
 *
 * ⚠ **2026-09-26 修 H12**：这张表**只对带 `entryId` 的地址生效**（它住在 `RuntimeDetail` 里）。
 * 而 `App.tsx` 的 `LegacyRuntimeRedirect` 原先重定向到 `/app/runtimes#home`（**列表页**，
 * 没有 entryId）⇒ **这张表一次都没被跑到**，文档却承诺"旧锚点由 `RuntimeDetail` 映射"。
 * 已改成重定向不带 hash。**直接深链到机器页的旧书签**（`/app/runtimes/<entryId>#home`）
 * 仍然有效 —— 那才是这张表真正服务的东西。
 */
const LEGACY_ANCHORS: Record<string, string> = {
  home: 'run',
  days: 'run',
  activity: 'run',
  mind: 'model',
}

export function RuntimeDetailPage() {
  const { entryId } = useParams()
  return <RuntimeDetail entryId={entryId ?? ''} />
}

function RuntimeDetail({ entryId }: { entryId: string }) {
  const navigate = useNavigate()
  const { hash } = useLocation()
  const { state, retry } = useHomeEntries()
  const ownedAgents = useAppStore((s) => s.ownedAgents)
  const ownedAgentsLoading = useAppStore((s) => s.ownedAgentsLoading)
  const ownedAgentsError = useAppStore((s) => s.ownedAgentsError)
  const agentsReadable = !ownedAgentsLoading && !ownedAgentsError

  // 深链接带锚点要**滚到那一间**。⚠ 判据不止"滚一次"：**那一节会晚出现、出现之后还会被顶走**
  // （上面几节分批长高）—— 2026-10-11 真机量过：滚了 171px 却仍停在顶上。所以交给
  // `lib/anchorScroll.ts` 那份共用实现（盯住一段时间、用户一动就交还、有界）。
  // 旧名（家 / 日子 / 脑子）经 `LEGACY_ANCHORS` 映射到新名。
  useEffect(() => {
    return scrollToAnchor(hash, { legacy: LEGACY_ANCHORS })
    // ⚠ `state.kind` 有意留在依赖里：数据到位时**重开一个窗口**（那时上面几节刚长高）。
  }, [hash, state.kind])

  const entry = state.kind === 'ready' ? (state.entries.find((e) => e.id === entryId) ?? null) : null
  const reading = state.kind === 'ready' ? (state.readings.find((r) => r.entry.id === entryId) ?? null) : null

  // 找不到这一条：如实说（**不拿第一条顶上** —— 那正是"页面说不清它说的是谁"）。
  if (state.kind === 'ready' && !entry) {
    return (
      <div className="flex h-full min-h-0 flex-col bg-background">
        <DetailHeader title="机器" onBack={() => navigate('/app/runtimes')} />
        <div className="flex flex-1 items-center justify-center px-6 text-center">
          <div className="space-y-2">
            <p className="text-body">没有这台机器</p>
            <p className="text-caption text-muted-foreground">
              地址里那台机器不在这份列表里（可能已被删除，或地址写错了）。
            </p>
            <Button variant="outline" size="sm" onClick={() => navigate('/app/runtimes')}>
              回到我的机器
            </Button>
          </div>
        </div>
      </div>
    )
  }

  // 三个房间共用**一份**状态形状（机器为主语，不需要认领）。
  const machine: RuntimeState =
    state.kind === 'ready' && entry && reading
      ? {
          kind: 'ready',
          entry,
          status: { view: reading.status.view, failure: reading.status.failure },
          config: { view: reading.config.view, failure: reading.config.failure },
        }
      : state.kind === 'unavailable'
        ? { kind: 'unavailable', message: state.message }
        : { kind: 'loading' }

  const agentId = reading?.status.view?.agent.agentId ?? null
  const agent = agentId ? (ownedAgents.find((a) => a.id === agentId) ?? null) : null
  // ★ 与列表行**同一份住户表**（2026-10-16）：机器页那张卡也说全 —— 判据仍然是
  //   `machineSituation` 一处（"这一页与那一页不会各说各话"）。
  const residents = (reading?.status.view?.residents ?? []).map((each) => ({
    agentId: each.agentId ?? null,
    name: each.agentId
      ? (ownedAgents.find((a) => a.id === each.agentId)?.displayName ?? null)
      : null,
    hostState: hostStateOf(each.host),
  }))

  /**
   * **第一次那条路的第一步要不要摆**（2026-10-11）。
   *
   * 判据是**现成的纯函数**（`api/homeConfig.ts` 的 `modelSourceAbsent`，有离线用例）：
   * 这台机器上**没有模型来源**时才摆。两条纪律：
   * - **读不到不摆**（`view === null` ⇒ 不摆）——「读不到 ≠ 没有模型来源」，那是这一族的头号病；
   * - **零新状态**：配好之后那份读数自然变 ⇒ 卡自己消失（不落任何持久面、不加任何偏好）。
   */
  const configView = reading?.config.view ?? null
  const showFirstModelStep = configView !== null && modelSourceAbsent(configView)

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      <DetailHeader
        title={entry ? (entry.local ? '本机' : entry.name) : '机器'}
        onBack={() => navigate('/app/runtimes')}
      />
      <PageScroll>
        {/* ★ **动作排在读数之前**（2026-10-11 主人裁）：这一屏原先只有读数（运行 / 本机服务 /
            活动），而第一次来的人要的是"我该做什么"。三个房间回答的是"现在怎么样"，
            所以这张卡必须排在它们**前面** —— 落点带 `#model` 那次滚动只是补丁。 */}
        {entry && showFirstModelStep && (
          <FirstModelStep
            entryId={entry.id}
            onConfigure={() => navigate(`/app/runtimes/${encodeURIComponent(entry.id)}/config`)}
            onBuildAgent={() => navigate('/app/agents')}
          />
        )}
        <MachineCard
          entry={entry}
          // 与列表行**同一份判据**（`machineSituation`）—— 这一页与那一页不会各说各话。
          situation={machineSituation({
            pending: state.kind === 'loading',
            view: reading?.status.view ?? null,
            failure: reading?.status.failure ?? null,
            agentState: reading?.status.view?.agent.state ?? null,
            legacyHome: reading?.status.view?.legacyHome === true,
            agentId,
            agentName: agent?.displayName ?? null,
            agentsReadable,
            residents,
          })}
          agentId={agentId}
          agentName={agent?.displayName ?? null}
          onRemoved={retry}
        />
        <RuntimeRunSection
          state={machine}
          onRetry={retry}
          agentNames={Object.fromEntries(ownedAgents.map((a) => [a.id, a.displayName]))}
        />
        <RuntimeModelSection
          state={machine}
          onRetry={retry}
          // ★ **按机器寻址**（2026-10-11）：配置页挂在**这台机器**的路径下（不再住"设置"里）——
          //   它管的是这一台上的钥匙清单。地址里点名谁，页面就配谁。
          onOpenProviders={(id) => navigate(`/app/runtimes/${encodeURIComponent(id)}/config`)}
          // ★ 住户表要用名字（2026-10-12）：`agentId → 世界里的名字`；清单读不到时如实说
          //   "名字读不到"（**不许**把读不到说成"没有这一支"）。
          agentNames={Object.fromEntries(ownedAgents.map((a) => [a.id, a.displayName]))}
          agentsReadable={agentsReadable}
        />
      </PageScroll>
    </div>
  )
}

/**
 * **第一次那条路的第一步**（2026-10-11；主人裁：这一屏原先只有读数）。
 *
 * ## 它治的是什么
 *
 * 第一次那条路（本机世界刚立起来、这台机器上还没有模型源）的落点是
 * `/app/runtimes/local#model` —— 靠**滚动**把人送到「模型」那一间。那条路真机量出来是
 * **没到位**的（滚了 171px、目标仍停在 641px：上面几节分批长高把它顶下去了），而且
 * **就算到位了，那个形状也是错的** —— 那是把"下一步要做的事"藏在折叠线下、再用手势拽出来。
 *
 * ⇒ 正解：**动作排在读数之前**。这张卡就是那句话，并且**把两步一起说清**
 * （先配一条模型源，再建一支 Agent）—— 后一步的入口在**世界侧**（`/app/agents`），
 * 不在这一页上，所以必须写出来，否则这条路走到一半就断了。
 *
 * ## 三条纪律
 *
 * - **在场判据只有一处**：`modelSourceAbsent`（`api/homeConfig.ts` 的纯函数，有离线用例）；
 *   调用点（`RuntimeDetail`）额外守一条：**读不到不摆**（「读不到 ≠ 没有模型源」）。
 * - **零新状态**：判据取自页面上**已有**的那份读数（`useHomeEntries` 的 `config`）——
 *   不新开请求、不落偏好 ⇒ 配好之后它自己消失。
 * - ⚠ **它消失之后"建一支 Agent"那个指针也跟着消失** —— **那是有意的**：它只服务"第一次"
 *   那一段路，一次就够；再要找那个入口，它在壳层导航「机器」组的「我的 Agent」里。
 */
function FirstModelStep({
  entryId,
  onConfigure,
  onBuildAgent,
}: {
  entryId: string
  onConfigure: () => void
  onBuildAgent: () => void
}) {
  return (
    <section
      data-first-model-step={entryId}
      className="space-y-3 rounded-xl border border-primary/40 bg-card p-5"
    >
      <div className="flex items-center gap-2">
        <KeyRound className="size-4 text-primary" aria-hidden />
        <span className="text-body">先给这台机器配一条模型源</span>
      </div>
      <p className="text-caption text-muted-foreground">
        Agent 要用一条模型源才能开始做事；这台机器上还没有。
        <strong>先配一条，再建一支 Agent。</strong>
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" className="gap-1.5" onClick={onConfigure}>
          <KeyRound className="size-3.5" />
          去配置模型
        </Button>
        <Button variant="outline" size="sm" className="gap-1.5" onClick={onBuildAgent}>
          <Sparkles className="size-3.5" />
          我的 Agent（在那里建一支）
        </Button>
      </div>
    </section>
  )
}

function DetailHeader({ title, onBack }: { title: string; onBack: () => void }) {
  return (
    <header className="flex items-center gap-2 border-b border-border bg-card px-4 py-4">
      <Button variant="ghost" size="icon" onClick={onBack} aria-label="返回我的机器">
        <ArrowLeft className="size-4" />
      </Button>
      <span className="text-title">{title}</span>
    </header>
  )
}

/**
 * 机器卡：**主语写在这里**（这台机器是什么、上面跑着谁）。
 *
 * 为什么单独一张卡而不是塞进「运行」房间：`运行` 房间说的是"跑得怎么样"，
 * 而"这是哪台机器、上面是谁"是**页面级的定位**——三个房间都靠它才知道自己在说谁。
 */
function MachineCard({
  entry,
  situation,
  agentId,
  agentName,
  onRemoved,
}: {
  entry: HomeEntry | null
  /** 「上面跑的是谁」的处境 —— **判据在 `api/agentRowStatus.ts`**，这里只按 `kind` 说完整那句话。 */
  situation: MachineSituation
  agentId: string | null
  agentName: string | null
  /** 那一台被移除了（父层据此重读列表）。 */
  onRemoved: () => void
}) {
  const navigate = useNavigate()
  /** 「这台机器的设置」展开着吗（管理面就在这一页 —— 见那颗按钮上的注释）。 */
  const [managing, setManaging] = useState(false)
  return (
    <section className="space-y-3 rounded-xl border border-border bg-card p-5">
      <div className="flex items-center gap-2">
        <Server className="size-4 text-muted-foreground" aria-hidden />
        <span className="text-body">{entry ? (entry.local ? '本机' : entry.name) : '这台机器'}</span>
        {entry && !entry.local && (
          <code className="truncate font-mono text-caption text-muted-foreground">{entry.baseUrl}</code>
        )}
      </div>
      <div className="text-caption text-muted-foreground">
        {/* **判据只有一处**：与列表行、家条目那一行同一份（`machineSituation`）——
            这里只是它的**完整语域**（列表行那句短）。★ 2026-10-11 修的那处假警报：
            从前这一支只看 `agentId === null` ⇒ 一个**还没有 Agent 的新家**（第一次那条路）
            被说成"没报上…（可能是记录读不出来，或口子版本旧）"，而真相是"还没有 Agent"。 */}
        {situation.kind === 'pending' && '正在读…'}
        {/* ⚠ 这一句原写作「这台机器**的读数**这一次没读到」—— 「读数」在**内行词表**上
            （`scripts/jargon.mjs`），被 `npm run verify` 的路由 sweep 当场抓住。 */}
        {situation.kind === 'unreadable' && '这台机器这次没读到。'}
        {situation.kind === 'no-resident' && '这台机器上还没有 Agent。'}
        {situation.kind === 'legacy-home' &&
          '这台机器的布局还是老的 —— 上面那一支住在旧布局里，所以这一格读不出是谁。'}
        {situation.kind === 'no-id' &&
          '这台机器没报上上面跑的是谁（可能是记录读不出来，或口子版本旧）。'}
        {situation.kind === 'unknown-agent' &&
          '这台机器上报的 Agent 不在这个世界里（「我的 Agent」那一页的清单里没有它）。'}
        {situation.kind === 'list-unreadable' &&
          '读不到「我的 Agent」那一页的清单，所以说不出上面跑的是谁——这不等于它不在这个世界里。'}
        {situation.kind === 'named' && (
          <>
            上面跑的是{' '}
            <button
              type="button"
              className="cursor-pointer text-foreground underline decoration-dotted underline-offset-2"
              onClick={() => agentId && navigate(`/app/agents/${agentId}`)}
            >
              {agentName}
            </button>
            。它的公开资料与接入方式在那一页。
          </>
        )}
      </div>
      <div className="flex flex-wrap gap-2">
        <Button
          variant="outline"
          size="sm"
          className="gap-1.5"
          onClick={() => setManaging((open) => !open)}
        >
          <Cpu className="size-3.5" />
          这台机器的设置
        </Button>
      </div>
      {/* ★ **某一台的管理面就在这一页**（2026-10-12）：从前这颗按钮把人送去
          `/settings/homes`（同一份清单的第二个门 ✗）。现在清单只有一处，管理跟着那台机器。
          ⚠ 这里只说**管理**（地址 / 端口 / 钥匙 / 移除）；"它现在怎么样"由上面那张卡与
          三个房间说 —— **一处说一次**。 */}
      {managing && entry && (
        <MachineAdminPanel
          entry={entry}
          onRemoved={() => {
            setManaging(false)
            onRemoved()
          }}
        />
      )}
    </section>
  )
}
