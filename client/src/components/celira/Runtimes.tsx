import { useEffect, useState } from 'react'
import { AlertTriangle, Brain, House, Loader2, RefreshCw, Save } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { cn } from '@/lib/utils'
import { formatRuntimeTime } from '@/lib/time'
import { agentDays, wakeLineAtMs, type AgentDay } from '@/lib/agent-days'
import { legacyHomeNote, nextLabel, originLabel, resultLabel, runStateLabel, stateLabel } from '@/lib/plain-language'
import type { HomeStatusView } from '@/api/homeStatus'
import { bindHomeSource, fetchHomeCatalog, HomeConfigError, modelWitnessChainLine, modelWitnessLine, residentBindings, residentModelLine, residentModelReadout, type HomeConfigProbe, type HomeConfigView } from '@/api/homeConfig'
import type { HomeEntry } from '@/api/homes'
import { localServiceReading, readingLabel } from '@/components/celira/reading-label'
import { hostStateLabel } from '@/api/agentHealth'
import { LocalServicesPanel } from '@/components/celira/LocalServices'
import { STATUS_TONE_CLASS } from '@/components/celira/status-tone'
import type { ToneLevel } from '@/components/celira/status-tone'
import { SectionFrame } from '@/components/celira/SectionFrame'

/**
 * **执行端的三个房间**（2026-09-25 组织轴刀）。
 *
 * ## 为什么这三个房间归"机器"而不归"Agent"
 *
 * 它们读的**全是机器级事实**：那台机器上的 Agent 在不在跑、醒过几次、用哪条模型。
 * 而原先它们挂在一个 **Agent** 的主页下（`/app/agents/:id` 的「家 / 日子 / 脑子」）——
 * 于是页面得先"认领"（遍历机器找哪台报的是这个 Agent）才能说得出话，而"运行状态代表谁"
 * 这种问题页面自己答不出来。**轴错了，不是实现错了。**
 *
 * 现在主语是**执行端（那台机器）**：本文件的三个 section 各收一份**那台机器的读数**，
 * 不需要任何认领。Agent 与机器的关系由**上一层页面**说一次（"上面住的是 X"），
 * 不在每个房间里重复。
 *
 * ## 三条不许松的口径（照搬既有纪律，一个字都没改）
 *
 * - **读不到 ≠ Agent 停了**：联系不上那台机器时如实说，**不画成一盏灭掉的灯**；
 * - **活动只给机械字段**：动作 / 轮次 / 收场，**绝不渲染正文与参数值**（Agent 的私有世界）；
 * - **key 永不回浏览器**：模型只显示"配没配钥匙"这一级的读数（门本身也不回 key）。
 *
 * ## 房间名
 *
 * `运行` / `活动` / `模型` —— 都是名词，与 `DESIGN_SYSTEM.md` 的组件 `title` 一致。
 * 原先的「家 / 日子 / 脑子」是**比喻**，不是产品名（2026-09-25 一并清掉）。
 */

/**
 * 一个执行端这一轮的**处境**（三个房间共用一档形状）。
 *
 * - `loading`：还在读家条目与各条目的读数；
 * - `unavailable`：这台浏览器存不住家条目（本地存储读不了）⇒ 如实说；
 * - `ready`：条目在手，`reading` 是**那台机器**的两份读数（状态口 + 配置门），
 *   各自成败分开（`failure` 为 `null` 且 `view` 为 `null` = 还在读）。
 */
export type RuntimeState =
  | { kind: 'loading' }
  | { kind: 'unavailable'; message: string }
  | { kind: 'ready'; entry: HomeEntry; status: MachineReading<HomeStatusView>; config: MachineReading<HomeConfigView> }

/** 一份机器读数：读到了（`view`）或没读到（`failure` 是固定人话）。 */
export interface MachineReading<T> {
  view: T | null
  failure: string | null
}

/** 读数行（灯 + 标签 + 内容）——沿用运行状态页那套视觉语言，不新造展示范式。 */
function ReadingRow({
  label,
  tone,
  children,
}: {
  label: string
  tone: ToneLevel
  children: React.ReactNode
}) {
  return (
    <div className="flex items-baseline gap-2">
      <span className="flex w-28 shrink-0 items-center gap-1.5 text-muted-foreground">
        <span className={`size-1.5 rounded-full bg-current ${STATUS_TONE_CLASS[tone].text}`} aria-hidden />
        {label}
      </span>
      <span className="min-w-0 break-words">{children}</span>
    </div>
  )
}

/**
 * 联系不上那台机器时的诚实卡（运行 / 活动 / 模型共用）。
 *
 * 两件事分开说（**这是既有纪律的原话，只是不再用内行词**）：
 * - **那台机器没在跑 / 连不上** ⇒ 把原因（固定人话）摆出来，能重试就重试；
 * - **那台机器上没跑 Agent** ⇒ **不算故障**，由机器卡自己如实说一句
 *   （`pages/RuntimesPage.tsx` 的 `MachineCard`；2026-09-26 起它还会按
 *   `agentsReadable` 再分两支 —— 列表没读到就只说读不到，不许断言"不在这个世界里"）。
 *
 * `reason` 是 `homes.ts` 那份**固定人话**（如「钥匙不对，去那台机器上重新拿一把」）；
 * 本机那台联系不上时照旧给出开机命令（对远程的家，那两条命令毫无意义，所以不摆出来）。
 */
export function UnreachableCard({
  what,
  onRetry,
  reason,
  local,
}: {
  what: string
  onRetry?: () => void
  reason?: string | null
  local?: boolean
}) {
  return (
    <div className="space-y-3 rounded-xl border border-border bg-card p-5">
      <div className="flex items-center gap-2">
        <AlertTriangle className="size-4 text-presence-timeout" aria-hidden />
        <span className="text-body">读不到{what}</span>
      </div>
      <p className="text-caption text-muted-foreground">
        联系不上这台机器，所以这里没有数据 —— 这不是 Agent 停了。
      </p>
      {reason && <p className="text-caption text-muted-foreground">{reason}</p>}
      {local && (
        <div className="space-y-1 text-caption text-muted-foreground">
          <div>
            启动本机：<code className="rounded bg-muted px-1.5 py-0.5 font-mono">node home.js up</code>
          </div>
          <div>
            只起本机数据服务（读口 ＋ 写口，一个进程）：
            <code className="rounded bg-muted px-1.5 py-0.5 font-mono">node home.js doors</code>
          </div>
        </div>
      )}
      {onRetry && (
        <Button variant="outline" size="sm" onClick={onRetry}>
          <RefreshCw className="mr-1.5 size-3.5" />
          重试
        </Button>
      )}
    </div>
  )
}

function LoadingCard({ what }: { what: string }) {
  return (
    <div className="flex items-center justify-center gap-2 py-12 text-caption text-muted-foreground">
      <Loader2 className="size-4 animate-spin" aria-label={`正在读取${what}`} />
      正在读取{what}…
    </div>
  )
}

/** 这台浏览器**存不住家条目**时的诚实卡（读不了本地存储）——不许画成"你没配过机器"。 */
export function EntriesUnavailableCard({ message, onRetry }: { message: string; onRetry?: () => void }) {
  return (
    <div className="space-y-3 rounded-xl border border-border bg-card p-5">
      <div className="flex items-center gap-2">
        <AlertTriangle className="size-4 text-presence-timeout" aria-hidden />
        <span className="text-body">读不到你的机器列表</span>
      </div>
      <p className="text-caption text-muted-foreground">{message}</p>
      {onRetry && (
        <Button variant="outline" size="sm" onClick={onRetry}>
          <RefreshCw className="mr-1.5 size-3.5" />
          重试
        </Button>
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// 模型
// ---------------------------------------------------------------------------

/**
 * ★ **这条钥匙能调哪些型号**（2026-10-14）：一次**只读**的目录查询 —— 门拿清单里**存着的那把**
 * 钥匙去问对方一次（`POST /home-config/catalog`；key 永不回浏览器）。
 *
 * 三条纪律（与门那一侧同一句话）：
 * - **按需拉**：点一下才问一次，绝不在进页面时自动打（那会替你向供应商发请求）；
 * - **连不上是读数**：`ok: false` 照样是一份读数（"这家现在连不上/超时"），**不许**画成"没有型号"；
 * - **只读**：机器页的主语是"这台机器上有哪些钥匙"。要**改**型号去这条的配置页
 *   （那里点一个就是换成它）——否则这里会长出第二条写路径，两条迟早给出两个答案。
 */
function EntryCatalog({ id, entry }: { id: string; entry: HomeEntry | null }) {
  const [probe, setProbe] = useState<HomeConfigProbe | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = async () => {
    if (busy) return
    setBusy(true)
    setError(null)
    try {
      setProbe(await fetchHomeCatalog(id, entry ?? undefined))
    } catch (failure) {
      setError(
        failure instanceof HomeConfigError ? failure.message : '没能问到那一家（那台机器没有响应）',
      )
    } finally {
      setBusy(false)
    }
  }

  return (
    <>
      <Button
        variant="ghost"
        size="sm"
        className="shrink-0"
        disabled={busy}
        onClick={() => void load()}
      >
        {busy ? '正在问…' : probe === null ? '看支持的型号…' : '再问一次'}
      </Button>
      {probe !== null && (
        <div className="w-full space-y-1 border-t border-border pt-1.5">
          {probe.ok && probe.models.length > 0 ? (
            <div className="flex flex-wrap gap-1">
              {probe.models.map((name) => (
                <code
                  key={name}
                  className="rounded bg-muted px-1.5 py-0.5 font-mono text-micro text-muted-foreground"
                >
                  {name}
                </code>
              ))}
            </div>
          ) : (
            <p className="text-caption text-muted-foreground">
              {probe.ok
                ? '这家没列出任何型号（有的接口不给目录）—— 这不代表它不能用。'
                : `没能问到这家${probe.reason === 'timeout' ? '（超时）' : '（连不上）'}—— 已经配好的照旧能用。`}
            </p>
          )}
          <p className="text-micro text-muted-foreground">
            要换这一条用的型号，去它的配置页（那里点一个就是换成它）。
          </p>
        </div>
      )}
      {error && <p className="w-full text-caption text-presence-failed">{error}</p>}
    </>
  )
}

// ---------------------------------------------------------------------------
// 运行
// ---------------------------------------------------------------------------

/**
 * **运行**：这台机器跑得怎么样 —— **这一间给"当下的答案"**（在不在跑 / 能不能开始新活动 /
 * 上次那一次成没成 / 被什么挡住），活动明细是它下面**默认收起**的一层下钻（2026-10-12 主人裁）。
 *
 * 主语是机器（"这台机器上的 Agent 在不在跑"），所以这里**不需要认领**——
 * 那台机器报的是谁，由上一层页面说一次。⚠ 但**账本那几格是"第一支"的**（顶层那几格），
 * 多支时必须点名（见 `RunCard` 里那句），否则会被读成"这台机器没做完"。
 */
export function RuntimeRunSection({
  state,
  onRetry,
  agentNames,
}: {
  state: RuntimeState
  onRetry: () => void
  /** `agentId → 世界里的名字`：账本那一句要点名（多支时）。 */
  agentNames: Record<string, string>
}) {
  return (
    <SectionFrame
      id="run"
      title="运行"
      icon={<House className="size-4 text-muted-foreground" aria-hidden />}
      subject="这台机器上的 Agent 在不在跑、现在能不能开始新活动、醒过几次。"
    >
      {state.kind === 'loading' && <LoadingCard what="运行情况" />}
      {state.kind === 'unavailable' && <EntriesUnavailableCard message={state.message} onRetry={onRetry} />}
      {state.kind === 'ready' &&
        (state.status.view ? (
          <div className="space-y-3">
            <RunCard view={state.status.view} agentNames={agentNames} />
            {/* **本机服务那一格**（§8.3 的总开关）：只对**本机那台**出现 ——
                别的机器上的服务不归这颗 app 停，而它自己也只在壳里存在
                （组件内部判：不在壳里就整个不渲染）。 */}
            {state.entry.local && <LocalServicesPanel />}
          </div>
        ) : (
          <UnreachableCard
            what="运行情况"
            onRetry={onRetry}
            reason={state.status.failure}
            local={state.entry.local}
          />
        ))}
    </SectionFrame>
  )
}

/**
 * 「最近一次活动」那一行该写什么。
 *
 * ⚠ 从前这里摆的是**后端原话**（`wake run=host-…-2 drives=1 ending=subject:completed
 * notification=read`）—— 对普通用户那是一串天书。现在跟「活动」同一口径：
 * **从记录编号里还原时间**，并如实标明时间是怎么来的；还原不出来就说"没读到时间"。
 */
function lastWakeLabel(lastWake: string | null): string {
  if (!lastWake) return '—'
  // 「取 run → 还原时间」收在 `lib/her-days.ts` 一处（`wakeLineAtMs`）：`lastWake` 是**原始行**，
  // 不是时间串。列表页那一格用的是同一个判据，两处不许各写一遍正则。
  const ms = wakeLineAtMs(lastWake)
  if (ms === null) return '—（这次没有读到时间）'
  return `${formatRuntimeTime(new Date(ms).toISOString())}（推算）`
}

/** 一支住户的名字：**读不到名字 ≠ 这一支不存在**（两个事实分开说，与「模型」那一间同款）。 */
function residentName(agentId: string | null | undefined, agentNames: Record<string, string>): string {
  if (!agentId) return '（没有身份名的那一支）'
  return agentNames[agentId] ?? `${agentId.slice(0, 8)}…`
}

/** 账本那几格**是谁的**（顶层 = 第一支；它没报上 id 时就如实说"没报上"，不编一个名字）。 */
function nameOfLedger(view: HomeStatusView, agentNames: Record<string, string>): string {
  const id = view.agent.agentId
  if (!id) return '它没报上是谁的那一支'
  return residentName(id, agentNames)
}

function RunCard({ view, agentNames }: { view: HomeStatusView; agentNames: Record<string, string> }) {
  const { agent } = view
  /**
   * 活动明细展开着没有 —— **受控**（2026-10-12）。
   *
   * ⚠ 为什么不能用非受控的 `<details>`：这一页**会轮询重渲染**，重渲染会把非受控的展开状态
   * 丢掉（门禁实测：点开之后卡片又被收起来 ⇒ "8 秒内落地"那条断言红）。受控 = 状态在 React 里，
   * 重渲染不动它。
   */
  const [showActivities, setShowActivities] = useState(false)
  // 灯色跟判据走：「正在处理」是**正常态**（不是故障），只有真读不到才算 danger。
  // ⚠ **「还没有 Agent」也不是故障**（2026-10-05）：它是**缺省**（这个家刚建好、还没人住进来），
  // 给红灯会让新用户以为机器坏了 —— 与「本机服务」那一格同一口径（那里也写"还没有 Agent"）。
  const agentTone: ToneLevel =
    agent.state === 'loaded'
      ? agent.admission === 'ready'
        ? 'ok'
        : 'warn'
      : agent.state === 'busy' || agent.state === 'no-resident'
        ? 'muted'
        : 'danger'
  const agentLabel =
    agent.state === 'loaded'
      ? stateLabel(agent.admission)
      : agent.state === 'busy'
        ? '正在处理一件事'
        : agent.state === 'no-resident'
          ? // **老形态 ≠ 没有 Agent**（2026-10-05）：壳只认新布局，而老家（`<家>/env`）
            // 是 `up.js` 负责搬的（app 不做迁移）⇒ 认不出就说认不出，别把它说成不存在。
            view.legacyHome === true
            ? '这台机器的布局还是老的'
            : '这台机器上还没有 Agent'
          : agent.state === 'missing-binary'
            ? '读不到（本机缺少必要的程序）'
            : '读不到'

  // 「本地服务」那一行要说的话：**三态**（在跑 / 在跑但自报不健康 / 读不到），判据只有一处
  // （`reading-label.ts` 的 `localServiceReading`）—— 从前这里写的是 `listening ? '在跑' : '没在跑'`，
  // 而 `listening` 是 `health === 200` 派生的 ⇒ 能拼出「没在跑（HTTP 500）」那种自相矛盾的字面。
  const localService = localServiceReading(view.im.health)

  return (
    <div className="space-y-3 rounded-xl border border-border bg-card p-5">
      <div className="flex items-center gap-2">
        <span className="ml-auto text-caption text-muted-foreground">
          {readingLabel(formatRuntimeTime(view.readAt), view.stale)}
        </span>
      </div>

      <ReadingRow label="本地服务" tone={localService.tone}>
        {localService.text}
      </ReadingRow>

      <ReadingRow label="当前状态" tone={agentTone}>
        {agentLabel}
      </ReadingRow>

      {agent.state === 'loaded' && (
        <>
          <ReadingRow label="活动次数" tone="muted">
            {agent.activities ?? '未知'}
            {agent.truncated ? '（总数照给，明细只给最近几条）' : ''}
          </ReadingRow>
          <ReadingRow label="最近一次活动" tone="muted">
            {lastWakeLabel(view.lastWake)}
          </ReadingRow>
          <ReadingRow label="接下来" tone="muted">
            {nextLabel(agent.admission)}
          </ReadingRow>
        </>
      )}

      {agent.state === 'busy' && agent.note && (
        <p className="text-caption text-muted-foreground">它这一轮正在做事 —— 等这一轮过去再看。</p>
      )}

      {/* ★ **这台机器上住着谁、谁在跑**（2026-10-12 主人点头）：这是"**不问也该知道**"的那一档
          ——"它还活着"不能外包给问询（它不在跑的时候，恰恰问不了它）。
          ⚠ 数据是**载荷里现成的** `residents[].host`（`hostStateOf` 归一四态，人话在
          `agentHealth.ts` 的 `hostStateLabel`）⇒ **零新增探针**：这一格不是"再去问每一支"，
          而是把门**已经给了**的那一格逐支摆出来。 */}
      {view.residents !== undefined && view.residents.length > 0 && (
        <div className="space-y-2 border-t border-border pt-3">
          <div className="text-micro text-muted-foreground">
            这台机器上的 Agent（{view.residents.length}）—— 哪支在跑
          </div>
          <ul className="divide-y divide-border rounded-lg border border-border">
            {view.residents.map((resident, index) => (
              <li
                key={resident.agentId ?? `slot-${String(resident.slot ?? index)}`}
                className="flex flex-wrap items-center gap-2 px-3 py-2"
              >
                <span className="truncate text-body">
                  {residentName(resident.agentId, agentNames)}
                </span>
                <span className="ml-auto shrink-0 text-caption text-muted-foreground">
                  {hostStateLabel(resident.host)}
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}

      {/* ★ **账本那几格是谁的**（2026-10-12 点名）：顶层那几格是**第一支**的账本
          （`agentHealth.ts` 那条判据同源）。一支时它就是全部、不必点名；**多支时不许不说主语** ——
          否则"上一次没做完"会被读成"这台机器没做完"。 */}
      {view.residents !== undefined && view.residents.length > 1 && (
        <p className="text-caption text-muted-foreground">
          上面那几格说的是 <strong>{nameOfLedger(view, agentNames)}</strong> 这一支
          （这台机器上还有 {view.residents.length - 1} 支 —— 各自的账本要一支一支看）。
        </p>
      )}

      {/* ★ **活动明细降为下钻**（2026-10-12）：默认收起 —— 明细是"问了才知道"的那一档。
          内容（时间 / 来源 / 结果 / 失败分类 / 做了哪几件事）照旧；不再给 run id 与原始明细。
          ⚠ **不挂在 `agent.state === 'loaded'` 上**：卡片自己会如实说"读不到 / 这一段没有活动"
          （从前那一间就是这样 —— 挂上去会让"读不到"变成"没有这一格"）。
          ⚠ **定义句必须留在屏上**（`tests/page-subject-lines.test.ts` 钉着）：上面那一格写着
          「活动次数」⇒ 用户看到这个词，同一屏就得有一句话能跨过它（定义跟着词走，不跟着房间走）。 */}
      <p className="text-caption text-muted-foreground">
        它每做一件事就算一次活动 —— 明细里只记动作与结果，不记内容。
      </p>
      <details
        className="border-t border-border pt-2"
        open={showActivities}
        onToggle={(event) => setShowActivities(event.currentTarget.open)}
      >
        <summary className="cursor-pointer text-caption text-muted-foreground">
          最近的活动明细{agent.activities != null ? `（共 ${agent.activities} 次）` : ''}
        </summary>
        <div className="pt-3">
          <ActivityCard view={view} thin />
        </div>
      </details>

      {/* 老形态那一句：**认不出 ≠ 不存在**（与「运行」上面那几格同一句话）。 */}
      {agent.state === 'no-resident' && view.legacyHome === true && (
        <p className="text-caption text-muted-foreground">{legacyNoteFor(view)}</p>
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// 活动（**不再是一个房间** —— 2026-10-12 主人裁：明细降为「运行」那一间里的下钻）
// ---------------------------------------------------------------------------
//
// 判据（一句话）：**"不问也该知道的"留在页面上，"问了才知道的"交给问询** ——
// 这个 Agent 就在这台机器上、有整机全权的命令行，读自己的账本、查"我上次为什么没回"比任何页面都方便；
// 把账本常驻成"第三个房间"，等于把命令行能干的活做成按钮。
//
// ⚠ **但兜底不能外包**：它不在跑的时候，恰恰是最需要诊断的时候（问不了它）⇒
// 「运行」那一间留着**当下的答案**（在不在跑 / 能不能开始新活动 / 上次那一次成没成 / 被什么挡住），
// 明细（时间 / 来源 / 结果 / 失败分类 / 做了哪几件事）降为**默认收起**的一层下钻。
// 卡片本身（`ActivityCard` / `ActivityRow`）的判据**一个字没改** —— 变的是**摆放与默认可见性**。

/**
 * **老家形态**那一句话（2026-10-05；同日随「壳自己搬」改口）——「运行」与「活动」两间共用**同一句**。
 *
 * ⚠ 这一句**不能再写"这台 app 只认新形态"**：壳从 2026-10-05 起在**开机时自己搬**
 * （`home_shape.rs` 镜像 `residents.js` 的 `migrateHomeShape`，幂等）。所以这一格出现时，
 * 意思是**这一轮没搬成** —— 而**为什么**由壳给的稳定原因码说（`legacyBlocker`），
 * 人话在 `plain-language.ts` 的 `legacyHomeNote`（三种码三句话，认不出的走兜底）。
 */
function legacyNoteFor(view: HomeStatusView): string {
  return legacyHomeNote(view.legacyBlocker)
}

/**
 * 「这一面这次没读到」时那两句该说什么 —— **三种处境各说各的，不压成一句**（2026-10-05）：
 *
 * - **还没有 Agent**（`no-resident`）：这不是读不到 —— 还没有 Agent ⇒ 还没有活动，
 *   这句话本来就成立，照实说；
 * - **它这一轮正在做事**（`busy`）：门在那一轮不给活动数据（环境同时只有一个写者）⇒
 *   说清"为什么现在读不到、什么时候再看"，而不是笼统的"读不到"；
 * - **其余**：才是真的读不到 —— 那句老话照旧（缺省不等于零，读不到 ≠ 没有）。
 *
 * ⚠ 三条**只在真的没读到**（`days.state === 'missing'`）时才走：web 形态里
 * `experience` 与 Agent 忙不忙无关，所以正忙时那一路照旧有数据 ⇒ 这一支不会被走到。
 */
function activityMissingText(view: HomeStatusView): { title: string; note: string } {
  if (view.agent.state === 'no-resident') {
    // ⚠ **老形态 ≠ 没有 Agent**：先看那一格（"我认不出这个形状"与"它不存在"是两件事）。
    if (view.legacyHome === true) {
      return { title: '这台机器的布局还是老的', note: legacyNoteFor(view) }
    }
    return {
      title: '还没有活动记录',
      note: '这台机器上还没有 Agent —— 等它住进来，它做的每件事都会记在这里。',
    }
  }
  if (view.agent.state === 'busy') {
    return {
      title: '它这一轮正在做事',
      note: '正在做事的时候这一面读不出来 —— 等这一轮过去再看。',
    }
  }
  return {
    title: '这次没有读到数据',
    note: '这是「这一面这次读不到」，不是「它没有活动记录」。',
  }
}

function ActivityCard({ view, thin = false }: { view: HomeStatusView; thin?: boolean }) {
  const days = agentDays(view)
  if (days.state !== 'loaded' && days.state !== 'runs-only') {
    // 缺省不等于零：读不到这一面就说读不到，**绝不画一列空表**（那会被当成"它什么都没做"）。
    const { title, note } = activityMissingText(view)
    return (
      <div className="space-y-2 rounded-xl border border-border bg-card p-5">
        <div className="text-body">{title}</div>
        <p className="text-caption text-muted-foreground">{note}</p>
      </div>
    )
  }
  if (days.days.length === 0) {
    return (
      <div className="space-y-2 rounded-xl border border-border bg-card p-5">
        <div className="text-body">这一段记录里没有活动</div>
        <p className="text-caption text-muted-foreground">读到的记录是空的：这一段里它一次都没动过。</p>
      </div>
    )
  }
  return (
    <div className="space-y-3">
      <p className="text-caption text-muted-foreground">
        这一段记录里有 {days.days.length} 次活动
        {days.truncated ? '（更早的还有，只给最近这些）' : ''}
        {days.windowed ? '；只读了文件末尾那一段，更早的没读' : ''}
      </p>
      {days.state === 'runs-only' && (
        // **如实说清这一面少了什么**：这条路只有"怎么收场"那一层 —— 不许让"没给明细"
        // 看起来像"它没动手"（那是两句不同的话，页面分得开）。
        <p className="text-caption text-muted-foreground">
          这一面只读得到<strong>每一次怎么收场</strong>；动手明细（做了哪几件事、走了几轮）
          这台机器没给。
        </p>
      )}
      <ul className="space-y-2">
        {days.days.map((day) => (
          <ActivityRow key={day.run} day={day} thin={thin} />
        ))}
      </ul>
    </div>
  )
}

/** 「什么时候开始的」——⚠ 时间是从记录编号里还原出来的，页面必须把这一点说出来。 */
function wakeLabel(day: AgentDay): { text: string; derived: boolean } {
  if (day.wakeAtMs === null) return { text: '—（这次没有读到时间）', derived: false }
  return { text: formatRuntimeTime(new Date(day.wakeAtMs).toISOString()), derived: true }
}

/**
 * 一行右侧那几格读数 —— **两条路各说各的**（2026-10-05）：
 * - 明细那条路：`N 件事 · M 轮`（"做了几件"**只有这一条路说得出来**）；
 * - 只有收场那条路：**做了几件事不知道** ⇒ 一个字都不说（"0 件事"会是一句假话）。
 *
 * ⚠ **收场了没有不再放在这里**（2026-10-05 挪走）：它现在归「结果」那一行
 * （`resultText` 先看结果、再看判定）—— 同一件事**只在一处说**（一词一主）。
 */
function daySummary(day: AgentDay): string {
  if (day.detail === 'runs-only') return ''
  const parts = [`${day.actions.length} 件事`]
  if (day.turns !== null) parts.push(`${day.turns} 轮`)
  return parts.join(' · ')
}

/**
 * 「结果」那一行该写什么 —— **先看结果、再看判定、最后才说"没读到"**（2026-10-05）。
 *
 * 三档的**顺序是语义**：
 * 1. **有收场事实**（`ending`）⇒ 用它（`resultLabel` 那张表：完成 / 未回复 / 失败 / 原样）；
 * 2. 没有 ⇒ 看**判定类目**（`runStateLabel`：停在半路 / 被挡住 / **不明——它可能已经做了** /
 *    判定不出来）—— 从前这四类被压成同一句"这次没有读到结果"，而它们的真义差得很远；
 * 3. 两样都没有 ⇒ 只剩"这一场自己那一行封存了没有"（明细那条路的判据）——
 *    没封存就是"还没结束"，封存了却读不到结果才是那句老话。
 *
 * ⚠ **不许拿一档去顶另一档**：判定类目与收场结果是两件事（`agent/README.md` §3.10）。
 */
function resultText(day: AgentDay): string {
  const byOutcome = resultLabel(day.ending)
  if (byOutcome !== null) return byOutcome
  const byJudgement = runStateLabel(day.runState)
  if (byJudgement !== null) return byJudgement
  return day.detail === 'rows' && !day.sealed ? '还没结束' : '这次没有读到结果'
}

/**
 * 一次活动一行。
 *
 * `thin`（2026-10-12，下钻里用）：**只给人话那一层** —— 时间 / 来源 / 结果 / 失败分类 / 做了哪几件事；
 * 不再给 **run id**（那是账本内部的编号，"问了才知道"的那一档：问它本人就够了，CLI 也读得到）与
 * **原始明细**（四十行 `run-start` 那种）。
 */
function ActivityRow({ day, thin = false }: { day: AgentDay; thin?: boolean }) {
  const wake = wakeLabel(day)
  const summary = daySummary(day)
  return (
    <li className="rounded-xl border border-border bg-card p-4">
      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <span className="text-body">{wake.text}</span>
        {wake.derived && <span className="text-micro text-muted-foreground">（推算）</span>}
        {/* 为什么开始（刀2b C②）：口子映射出来的稳定取值 → 人话；没有这一格就**不写这一行**。 */}
        {originLabel(day.origin) && (
          <span className="text-caption text-muted-foreground">来源：{originLabel(day.origin)}</span>
        )}
        {summary && <span className="ml-auto text-caption text-muted-foreground">{summary}</span>}
      </div>

      {/* 动作那一行**只有明细那条路有**：`runs-only` 那条路上"做了几件事"根本没给，
          写"没有动手的记录"会把它说成"看过了，没有"（那是另一句话）。 */}
      {day.detail === 'rows' && (
        <div className="mt-2 flex flex-wrap items-baseline gap-x-2 gap-y-1 text-caption">
          {day.actions.length === 0 ? (
            <span className="text-muted-foreground">这次活动没有动手的记录</span>
          ) : (
            day.actions.map((action) => (
              <code key={action} className="rounded bg-muted px-1.5 py-0.5 font-mono text-micro">
                {action}
              </code>
            ))
          )}
        </div>
      )}

      <div className="mt-2 flex flex-wrap items-baseline gap-x-3 text-caption">
        <span className={day.failure ? 'text-presence-failed' : 'text-muted-foreground'}>
          结果：{resultText(day)}
          {day.failure ? `（${day.failure}）` : ''}
        </span>
        {!thin && <code className="ml-auto font-mono text-micro text-muted-foreground">{day.run}</code>}
      </div>

      {/* 原始明细降为**详情**：它还是机械字段（两道白名单各一道），只是一次活动该先给人看的
          是"这次做了什么"，而不是四十行 `run-start`。
          ⚠ **只有明细那条路有得可展**（`runs-only` 那条路一条裸行都没有 ⇒ 摆一个"展开 0 条"
          是拿空壳骗点击）。⚠ `thin`（下钻里那一档）**连这一层都不给**：那是"问了才知道"的深处。 */}
      {!thin && day.detail === 'rows' && (
      <details className="mt-3 border-t border-border pt-2">
        <summary className="cursor-pointer text-micro text-muted-foreground">
          展开这次活动的明细（{day.rows.length} 条 · 只给动作与状态，不给内容）
        </summary>
        <ul className="mt-2 space-y-1">
          {day.rows.map((row, index) => (
            <li
              key={`${String(row['seq'] ?? index)}`}
              className="flex flex-wrap items-baseline gap-x-3 gap-y-1 text-micro last:border-0"
            >
              <span className="w-14 shrink-0 font-mono text-muted-foreground">
                #{String(row['seq'] ?? index + 1)}
              </span>
              <span className="shrink-0 font-mono">{String(row['kind'])}</span>
              {row['action'] !== undefined && (
                <span className="font-mono text-foreground">{String(row['action'])}</span>
              )}
              {row['state'] !== undefined && (
                <span className="font-mono text-muted-foreground">{String(row['state'])}</span>
              )}
              {row['decision'] !== undefined && (
                <span className="text-muted-foreground">决定 {String(row['decision'])}</span>
              )}
              {row['turn'] !== undefined && (
                // **「轮」是词表里的名字**（`docs/runtime/AGENT_VOCABULARY.md`：`Turn` ＝ 一次「看＋行动」；
                // 页面上的词与概念 1:1，同义词不留）。这里原先写「拍」——同一个概念两套词，
                // 2026-10-05 按主人裁决收成一个。
                <span className="text-muted-foreground">第 {String(row['turn'])} 轮</span>
              )}
              {row['failure'] !== undefined && (
                <span className="text-presence-failed">{String(row['failure'])}</span>
              )}
            </li>
          ))}
        </ul>
      </details>
      )}
    </li>
  )
}

// ---------------------------------------------------------------------------
// 模型
// ---------------------------------------------------------------------------

/**
 * **模型**：这台机器上的模型源清单与"这一个 Agent 用哪一条"，以及**换成别的**。
 *
 * 清单（这台机器上有哪些钥匙）与绑定（这一个 Agent 用哪一条）是两件事：
 * 前者归机器、后者归住在机器上的那个 Agent —— 所以换绑在这里做，
 * 而"这台机器有几把钥匙"由**同一台机器的配置页**（`/app/runtimes/:entryId/config`；
 * 2026-10-11 起**按机器寻址**）管：**配置跟着它的对象走**，它不再住在"设置"里。
 *
 * ★ **2026-10-12：一支一支地换**（住户级覆盖）。从前这一间只有**家级**那一个动作
 * （"换成这条" = 这台机器上的**每一支**都换）—— 一个家住两支时，主人**没法**让 A 用 1 号、
 * B 用 2 号。所以这里列出**住户表**：每一支一行（名字 ＋ 它现在用哪条 ＋ 给它换）。
 *
 * ★ **2026-10-15：家级那两颗动作从这一间撤掉**（主人当场裁的边界，判据一句话：
 *   **机器是地方，Agent 是人 —— "有什么"归地方，"我用哪个"归人；"换"永远是给人换。**）
 *   撤掉的两颗是**没有主语**的：清单行那颗「换成这条」真实作用是"这台机器上每一支都换"
 *   （`switchTo(id)`，`agentId = null`），而"会把每一支都换过去"那句说明**只在多支时才渲染**
 *   ⇒ 单支的主人看到的是一颗**不知道换谁**、实际会动到所有人的按钮；底部那颗
 *   「解除指定（改用环境变量里的设置）」同款（`switchTo(null)` = 每一支都解绑）。
 *   能力没丢：家级绑定在**配置页保存时那颗「存完就用它」**上，逐支解绑在住户行里。
 *   ⇒ 这一间现在只回答两个问题：**这台机器上有哪些钥匙**（清单，读数）、
 *   **住着谁、各自拿哪把**（住户表，读数）；**每一颗动作都点名是哪一支**。
 *   主人当次的原话："啥叫换成这条，你不觉得很离谱吗"。
 *
 * 三条不许松的口径：
 * 1. **红路径不许静默**：换绑失败一律把服务那句人话摆出来（`role="alert"`），不假装换成了；
 * 2. **key 永不回浏览器**：只显示"配没配"这一级（服务本身也不回它）；
 * 3. **读不到 ≠ Agent 停了**：联系不上那台机器时如实说读不到，**不画成"它没有模型"**。
 */
export function RuntimeModelSection({
  state,
  onRetry,
  onOpenProviders,
  agentNames,
  agentsReadable,
}: {
  state: RuntimeState
  onRetry: () => void
  onOpenProviders: (entryId: string) => void
  /** `agentId → 世界里的名字`（住户表要用它把"那一支"说成人看得懂的名字）。 */
  agentNames: Record<string, string>
  /** 世界那份 Agent 清单读到了没有 —— **读不到时不许把"没名字"说成"不存在"**。 */
  agentsReadable: boolean
}) {
  /** 刚换完绑接上的那一份（服务回的正是三合一）；`null` = 用父层给的读数。 */
  const [override, setOverride] = useState<{ entryId: string; view: HomeConfigView } | null>(null)
  /** 正在换的那一条（`''` = 正在解除绑定）；用来禁按钮、防连点。 */
  const [busy, setBusy] = useState<string | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)

  const entryId = state.kind === 'ready' ? state.entry.id : null
  useEffect(() => {
    // 换了执行端 ⇒ 上一台机器的那份"刚换完绑"作废（它是别人的读数）。
    setOverride(null)
    setActionError(null)
  }, [entryId])

  const entry = state.kind === 'ready' ? state.entry : null
  const configReading = state.kind === 'ready' ? state.config : null
  const view = override && entry && override.entryId === entry.id ? override.view : (configReading?.view ?? null)

  /** 动作的钥匙（禁连点用）：家级是 `|ms-1`，住户级是 `<agentId>|ms-1`（右边空 = 解除）。 */
  const keyOf = (agentId: string | null, id: string | null) => `${agentId ?? ''}|${id ?? ''}`

  /** 住户表里的名字：**名字读不到 ≠ 这一支不存在**（两个事实分开说）。 */
  const nameOf = (agentId: string | null) => {
    if (agentId === null) return '（没有身份名的那一支）'
    const name = agentNames[agentId]
    if (name) return name
    return agentsReadable
      ? `${agentId.slice(0, 8)}…（不在你拥有的 Agent 里）`
      : `${agentId.slice(0, 8)}…（名字读不到）`
  }

  /** 这一支现在用哪条（**人话只有一处**：`homeConfig.ts` 的 `residentModelLine`）。 */
  const modelLineOf = (agentId: string | null): string =>
    view === null ? '读不到' : residentModelLine(residentModelReadout(view, agentId))

  /**
   * 换绑 / 解绑；成功了就把新的读数接上（**不重新拉一次**：服务回的正是三合一）。
   *
   * `agentId`：`null` = **家级**（这台机器上的每一支都换）；给了 = **只给这一支**（住户级覆盖）。
   */
  const switchTo = async (id: string | null, agentId: string | null = null) => {
    if (!entry) return
    setBusy(keyOf(agentId, id))
    setActionError(null)
    try {
      const next = await bindHomeSource(id, entry, agentId)
      setOverride({ entryId: entry.id, view: next })
    } catch (error) {
      setActionError(
        error instanceof HomeConfigError ? error.message : '没能换过来（那台机器没有响应）',
      )
    } finally {
      setBusy(null)
    }
  }

  /**
   * 整机那一格（门取的"**第一支**绑的那条"）：**只在"单支的家"里等于唯一事实**。
   * 多支的家它只是其中一支的 ⇒ 那几句全称句只在 `divergent` 为假时才说，而"谁在用"
   * 一律从**逐支读数**反查（`usersOf`，2026-10-15 起不再用 `inUseEntryIds` —— 它只回答
   * "这条有没有人在用"，而这一间要把**名字**写出来）。
   */
  const entries = view?.entries ?? []
  const bound = view?.bound ?? null
  const boundEntry = view ? (entries.find((each) => each.id === bound) ?? null) : null
  /** 各支绑得不一样吗（**门算的**）。为真时"正在用 X"那种全称句一律不许说。 */
  const divergent = view?.bindingDivergent === true
  /**
   * **这个家有没有住户**（`residentBindings`：`[]` = 门说一支都没有；`null` = 门没给这一格 ⇒ 不知道）。
   *
   * 为什么要它：这一页的主语是**机器**，而"哪一支用哪一条"与"给某一支换成这条"都是**住户级**的话。
   * 0 住户时那两句话**没有主语**（上面「活动」那一间刚说过"这台机器上还没有 Agent"）——
   * 于是这一间改说"还没有住户"，并且**不摆**那颗按钮：**动作的目标不存在时，动作就该收起来**。
   */
  const residents = view ? residentBindings(view) : null
  const noResident = residents !== null && residents.length === 0
  /**
   * ★ **这两份配置最后是谁改的**（2026-10-18）：门报过的**每一份**都列出来 ——
   * 清单那一份 ＋ 每一支的绑定（住户与"先选好还没接上"的**都在里面**：缺哪一份，
   * 那一份就成了看不见的一格）。判据与人话只有一处（`api/homeConfig.ts` 的 `modelWitnessLine`）。
   */
  const witnessRows = [
    ...(residents ?? []).map((row) => ({
      key: `homed-${row.agentId ?? 'legacy'}`,
      who: nameOf(row.agentId),
      reading: row.witness,
    })),
    ...(view?.boundUnhomed ?? []).map((row) => ({
      key: `unhomed-${row.agentId ?? 'legacy'}`,
      who: `${nameOf(row.agentId)}（还没接上这台机器）`,
      reading: row.witness,
    })),
  ]
  /** 账本链条断了 ⇒ 那一句（`null` = 没断，或老门不给这一格 ⇒ 不说）。 */
  const witnessChain = view ? modelWitnessChainLine(view.witnessChainBreakAt) : null
  /**
   * ★ **谁在用这一条**（2026-10-15）：清单那一列上写**名字**，不再写那颗没主语的「换成这条」。
   *
   * 判据（主人当场裁的边界）：**机器是地方，Agent 是人** —— 清单回答"这台机器上有哪些钥匙"，
   * 而"谁在用它"要把**名字**写出来（"正在使用"这种不点名的徽标，读者还得自己回上面那张表去对）。
   * 反查走的是**逐支读数**（`residentBindings`），与住户表**同一份来路** —— 不另算一遍。
   */
  const usersOf = (id: string): string[] =>
    (residents ?? [])
      .filter((row) => row.state === 'bound' && row.bound === id)
      .map((row) => nameOf(row.agentId))

  return (
    <SectionFrame
      id="model"
      title="模型"
      icon={<Brain className="size-4 text-muted-foreground" aria-hidden />}
      subject={
        <>
          这台机器上的模型源，以及它承载的 Agent 正在用哪一条。只显示配没配，
          <strong>不显示密钥内容</strong>。
        </>
      }
    >
      {state.kind === 'loading' && <LoadingCard what="模型配置" />}
      {state.kind === 'unavailable' && <EntriesUnavailableCard message={state.message} onRetry={onRetry} />}

      {state.kind === 'ready' && view === null && (
        <UnreachableCard what="模型配置" onRetry={onRetry} reason={configReading?.failure} local={entry?.local ?? false} />
      )}

      {state.kind === 'ready' && view !== null && (
        <div className="space-y-3 rounded-xl border border-border bg-card p-5">
          <div className="flex flex-wrap items-center gap-2">
            <Badge
              variant="outline"
              className={cn(
                view.state === 'configured' && 'border-presence-online/40 text-presence-online',
                view.state === 'unreadable' && 'border-destructive/40 text-destructive',
                view.state === 'absent' && 'text-muted-foreground',
              )}
            >
              {noResident
                ? '这台机器上还没有 Agent'
                : view.state === 'configured'
                  ? '已指定'
                  : view.state === 'unreadable'
                    ? '读不出来'
                    : '没指定'}
            </Badge>
            {boundEntry && !divergent && (
              <span className="text-body">
                这台机器上正在被用的：<strong>{boundEntry.name}</strong>
                {/* ★ 主语（2026-10-15）：从前的「正在用 X」读起来像**机器**在用模型 ——
                    而"用"这个动作的主语只能是 Agent。谁在用，把名字写出来。 */}
                <span className="text-muted-foreground">
                  {(() => {
                    const users = usersOf(boundEntry.id)
                    return users.length > 0 ? `（${users.join('、')}在用）` : '（还没有人在用）'
                  })()}
                </span>
              </span>
            )}
            {divergent && (
              <span className="text-body">
                住在这台机器上的几支<strong>用的不是同一条</strong> —— 每一支各自用的那条，在下面那一张表里。
              </span>
            )}
            <span className="ml-auto text-caption text-muted-foreground">
              更新时间 {formatRuntimeTime(view.readAt)}
            </span>
          </div>

          {actionError && (
            <p
              role="alert"
              className="rounded-lg border border-destructive/40 bg-card px-3 py-2 text-caption text-destructive"
            >
              {actionError}
            </p>
          )}

          {boundEntry ? (
            <>
              <ReadingRow label="接口地址" tone="muted">
                <code className="break-all font-mono">{boundEntry.endpoint}</code>
              </ReadingRow>
              <ReadingRow label="模型名" tone="muted">
                <code className="font-mono">{boundEntry.model}</code>
              </ReadingRow>
              <ReadingRow label="上下文长度" tone="muted">
                {boundEntry.contextWindow ?? '—'}
              </ReadingRow>
              <ReadingRow label="密钥" tone="muted">
                {boundEntry.keyConfigured ? '已保存（内容不显示）' : '没配'}
              </ReadingRow>
              {/* 接口类型是**进阶项**：默认收起。绝大多数人不用碰它（缺省就是最常见那一种），
                  而它摆在外面会让人以为"必须选一个"。 */}
              <details className="border-t border-border pt-2">
                <summary className="cursor-pointer text-micro text-muted-foreground">高级</summary>
                <div className="mt-2">
                  <ReadingRow label="接口类型" tone="muted">
                    {boundEntry.protocol ?? '默认（chat-completions）'}
                  </ReadingRow>
                </div>
              </details>
            </>
          ) : (
            <p className="text-caption text-muted-foreground">
              {view.state === 'unreadable'
                ? '那台机器上这一格配置读不出来 —— Agent 起不来就是因为这个。'
                : noResident
                  ? '这台机器上还没有 Agent —— 先在「我的 Agent」里创建一支，再在它自己的页面上点「接到这台机器上」；还没接上的那支也可以先在它的页面上把模型选好，接上就用它。'
                  : entries.length > 0
                    ? '这一支还没指定用哪一条。下面挑一条，或者先去配一条。'
                    : '那台机器上还没有模型配置 —— 先去配一条。'}
            </p>
          )}

          {/* ★ **一支一支地换**（2026-10-12；2026-10-15 起这是这一间**唯一**的换绑入口）：
              每一支一行 —— 名字、它现在用哪条、**给它换**。每颗动作都带着主语（这一行的名字）。 */}
          {residents !== null && residents.length > 0 && (
            <div className="space-y-2 border-t border-border pt-3">
              <div className="text-micro text-muted-foreground">
                这台机器上的 Agent（{residents.length}）—— 每一支各用哪一条
              </div>
              <ul className="divide-y divide-border rounded-lg border border-border">
                {residents.map((row) => (
                  <li
                    key={row.agentId ?? 'legacy'}
                    className="flex flex-wrap items-center gap-2 px-3 py-2"
                  >
                    <span className="truncate text-body">{nameOf(row.agentId)}</span>
                    <span className="truncate text-caption text-muted-foreground">
                      {modelLineOf(row.agentId)}
                    </span>
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button
                          variant="outline"
                          size="sm"
                          className="ml-auto shrink-0"
                          disabled={busy !== null || entries.length === 0}
                          // 名字进无障碍标签：**动作的主语不许只靠"在这一行里"暗示**
                          // （屏幕阅读器读不到"这一行是谁"）。
                          aria-label={`给 ${nameOf(row.agentId)} 换它用的那一条`}
                        >
                          {busy !== null && busy.startsWith(`${row.agentId ?? ''}|`)
                            ? '正在换…'
                            : '换这一支…'}
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        {entries.map((each) => (
                          <DropdownMenuItem
                            key={each.id}
                            disabled={row.state === 'bound' && row.bound === each.id}
                            onSelect={() => void switchTo(each.id, row.agentId)}
                          >
                            {each.name}（{each.model}）
                          </DropdownMenuItem>
                        ))}
                        {row.state === 'bound' && (
                          <DropdownMenuItem onSelect={() => void switchTo(null, row.agentId)}>
                            解除指定（改用环境变量）
                          </DropdownMenuItem>
                        )}
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </li>
                ))}
              </ul>
              {/* ★ **"只跑了小欣"那个疑问的出口**（2026-10-15）：这一页只列**住在这台机器上**的
                  Agent（主语是地方）—— 世界里别的 Agent 要不要住进来，是**它自己**那一页上的动作。
                  少了这一句，主人只会觉得"我的另一支怎么不见了"（主人当次问的就是这个）。 */}
              <p className="text-micro text-muted-foreground">
                别的 Agent 要住进这台机器，在它自己的页面上点「接到这台机器上」。
              </p>
            </div>
          )}

          {/* **那台机器上有哪些钥匙**（读数）：这一列**不给"换"的动作** —— 换永远要点名是哪一支
              （上面那张表），因为清单行那颗「换成这条」从前实际动的是**这台机器上的每一支**
              （2026-10-15 主人裁：没有主语的动作不该存在）。谁在用这条，这里把名字写出来。 */}
          {entries.length > 0 && (
            <div className="space-y-2 border-t border-border pt-3">
              <div className="text-micro text-muted-foreground">
                这台机器上配好的模型源（{entries.length}）—— 谁在用哪一条
              </div>
              <ul className="divide-y divide-border rounded-lg border border-border">
                {entries.map((each) => (
                  <li key={each.id} className="flex flex-wrap items-center gap-2 px-3 py-2">
                    <span className="truncate text-body">{each.name}</span>
                    <code className="truncate font-mono text-caption text-muted-foreground">
                      {each.model}
                    </code>
                    {/* ★ **这条钥匙能调哪些型号**（2026-10-14）：**只读**（机器页的主语是"这台机器上
                        有哪些钥匙"）。要**改**型号在配置页那一条里（模型目录点一个就改成它）——
                        这里点一个就改，会长出第二条写路径。 */}
                    <EntryCatalog id={each.id} entry={entry} />
                    <span className="ml-auto shrink-0 text-caption text-muted-foreground">
                      {(() => {
                        const users = usersOf(each.id)
                        return users.length > 0 ? `${users.join('、')} 在用` : '没人用'
                      })()}
                    </span>
                  </li>
                ))}
              </ul>
            </div>
          )}

          {/* ★ **这两份配置最后是谁改的**（2026-10-18）：**两档分开说，不合并** ——
              页面改的**有时刻**；不是页面改的，就**只知"变了"，不知是谁**。
              判据、边界（不是封印 · 判不了 ≠ 没改过）与人话只有一处：
              `api/homeConfig.ts` 的 `modelWitnessLine`。 */}
          <div className="space-y-2 border-t border-border pt-3">
            <div className="text-micro text-muted-foreground">
              这两份配置最后是谁改的 —— 页面改的，有时刻；不是页面改的，就只知"变了"，不知是谁。
            </div>
            <ul className="space-y-1">
              <li className="text-caption text-muted-foreground">
                清单：<span className="text-body">{modelWitnessLine(view.listWitness)}</span>
              </li>
              {witnessRows.map((row) => (
                <li key={row.key} className="text-caption text-muted-foreground">
                  {row.who} 的绑定：
                  <span className="text-body">{modelWitnessLine(row.reading)}</span>
                </li>
              ))}
            </ul>
            {witnessChain && (
              <p role="status" className="text-caption text-destructive">
                {witnessChain}
              </p>
            )}
          </div>

          <div className="flex flex-wrap items-center gap-2 border-t border-border pt-3">
            <Button
              variant="outline"
              size="sm"
              onClick={() => entry && onOpenProviders(entry.id)}
              className="gap-1.5"
            >
              <Save className="size-3.5" />
              去配置模型
            </Button>
          </div>

          <p className="text-micro text-muted-foreground">
            改动只影响<strong>下一次活动</strong>：正在进行的那一次不受影响。
          </p>
        </div>
      )}
    </SectionFrame>
  )
}

