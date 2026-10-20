import { useState } from 'react'
import { useNavigate } from 'react-router'
import { Check, ChevronRight, Plus, Server } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'
import type { OwnedAgent } from '@/api/domain'
import { useAppStore } from '@/api/store'
import { agentCreateErrorText } from '@/api/contractErrors'
import { agentRowStatusFromHomes, rowSameCauseLine } from '@/api/agentRowStatus'
import { agentHealthFromHomes } from '@/api/agentHealth'
import { shouldOfferAttach } from '@/api/agentMachine'
import {
  LOCAL_HOME_ID,
  mindReadoutFor,
  resolveAgentHome,
  type AgentHomeResolution,
  type HomeReading,
} from '@/api/homes'
import type { HomeStatusView } from '@/api/homeStatus'
import { firstRunPlan, type FirstRunExit, type FirstRunFrame, type FirstRunPlan, type FirstRunStepState } from '@/api/firstRun'
import { AGENT_REVOKED_CONSEQUENCE } from '@/api/agentRevoked'
import { AgentBadge } from '@/components/celira/AgentBadge'
import { ParticipantAvatar } from '@/components/celira/ParticipantAvatar'
import { agentHomeReadings, useHomeEntries } from '@/components/celira/use-home-entries'
// 读数四档 → 色类：**与 Agent 主页「机器」那一段共用同一份**（两处渲染的是同一句话）。
import { READOUT_TONE_DOT, READOUT_TONE_TEXT } from '@/components/celira/status-tone'
import { anjiaReadout, hasAnjiaBridge, type AnjiaResult } from '@/api/anjia'

/**
 * 「我的 Agent」**列表**（`/app/agents`）：一行一个 Agent，机器侧那一句 ＋ 三格读数 ＋ 内联创建
 * （前端填肉刀，2026-09-23；**家口远程访问刀**起按家条目认领）。
 *
 * 为什么从 `MyAgentsPage` 里搬成一个组件：列表要拉**家的读数**（状态口 + 模型门），
 * 而 Agent 的主页那一支也在拉同一份 —— 两处各拉一次，谁也不替谁多打一次口子。
 * hooks 也不能条件调用，所以整块搬进来。
 *
 * 三条纪律：
 * - **每一格都是读数，点击行为一字不变**：整行点下去仍然只是进 Agent 的主页；
 * - **一格读不到不连累别的格**（判据在 `api/agentRowStatus.ts` 与 `api/agentHealth.ts`，
 *   两层各有 `node --test` 用例）；
 * - ★ **一行只有一个状态点**（2026-10-20 T2 ③）：它只跟 `agentHealth` 的色档走
 *   （"在不在跑 / 正不正常"）；住址那一格是**纯文本**。从前住址那一格自带一颗点，
 *   于是"机器没在跑（黄字）"旁边挂着一颗绿点，读起来像"它在跑"。
 *
 * **认领**（家口远程访问刀）：哪台机器报告的就是这一行那个 Agent（`resolveAgentHome`），
 * 三格就用那台机器的读数；认领到远程那台时住址那一格如实写「在<名字>上」；
 * 谁都不是 ⇒ 「不在这台机器上」（不拿别的机器的读数顶）。
 *
 * **创建只在这一页**（2026-10-20）：创建是**所有权**，不是关系 —— 「新的朋友」只发关系申请。
 * 所以创建完的四态读数落在**就地面板**里（`CreateAgentInline`，它自己那张卡），不再有
 * "从别处创建完跳过来、读数要落在这一行"那条路（`createdAgentId` 随入口一起删掉）。
 *
 * ★ **第一次那三步**（2026-10-20 T1）：空态与创建面板顶部各摆一份 `FirstRunChecklist`
 * （模型源 / 建一支 / 接上一台机器）。状态全部来自这一页**已经在读**的那几份
 * （`useHomeEntries` 的配置读数 × `resolveAgentHome` × `anjiaByAgent`）——
 * **零新请求、零新持久面**；判据在 `api/firstRun.ts`（纯函数、有用例）。
 */

/**
 * 创建之后那句**事实**：Agent 不会回复。
 *
 * 为什么是一个常量而不是两处字面量：这句话在屏幕上出现两次（创建面板的**预期**句
 * `创建后` + 它，与创建成功横幅的**事实**句 = 它本身）。两处各写一遍就是会漂移的形状。
 *
 * ⚠ **横幅那半句由这条常量被传递覆盖**（`verify-ui.mjs` 断的是渲染出来的整句
 * `创建后它不会回复 —— 还没有模型，也没有机器在跑它`）：真机门禁**不真创建**（见该脚本
 * "只点不写"那条纪律），所以横幅那一支本没有判据能碰到；而它与预期句共用这一个字面量
 * ⇒ 它不可能与预期句漂移。**别把任一处改回内联字符串**——那会让这句事实失去唯一的守卫。
 *
 * ⚠ 2026-09-26：**导出**它，让「新的朋友」页的第二个创建入口也用它 ——
 * 那句事实（`client/README.md`「已知缺口」）**只能有一个权威位置**，而当时它只在
 * 这一个入口说，另一个入口的按钮还写着「创建并接入」（承诺不实）。
 * ⚠ **2026-10-20**：「新的朋友」那个创建入口已删（创建归这一页），导出的外部消费者
 * 随之消失。**仍然导出**：它是这句话的唯一权威位置，而唯一性不取决于外面还有几个人用。
 */
export const CREATE_WONT_REPLY = '它不会回复 —— 还没有模型，也没有机器在跑它'

/**
 * 壳这一支的读数**就是「这台机器上还缺模型凭据」**那条拒绝吗（`refused` 的封闭子词之一）。
 *
 * ⚠ 只认**这一个**子词（`sub` 已经在 `api/anjia.ts` 收敛过封闭表）：其余子词的补救**不**在这台
 * 机器的「模型」那一间（缺程序 / 找不到仓根 / 家没布置 / 编号形状 / 绑定读不懂 / 凭据读不懂）
 * —— 指错门与指假路径同族。那句话在这一页**两处渲染**（列表行 ／ 创建成功横幅）⇒ 判据只有这一处。
 */
function refusedForMissingModelCredential(anjia: AnjiaResult | undefined): boolean {
  return anjia?.kind === 'refused' && anjia.sub === 'missing-model-credential'
}

/**
 * **那句拒绝的出口**（2026-10-08）。
 *
 * 那句话说的是"这台机器上还缺模型凭据（模型那一半还没配）"，而这件事**在这台机器上就能办**
 * —— 机器页的「模型」那一间（`/app/runtimes/local#model`：那台机器上有哪些钥匙 ＋ 这一支用
 * 哪一条）。从前它只被渲染成一句 `role="status"`、**没有出口**：用户读到了"缺什么"，却走不到
 * 能补它的地方（诊断句没有下一步，与死胡同同族）。
 *
 * 三条：
 * - **去处只有本机那一台**：这一整条路（壳的命令 `anjia_start`）说的就是**这台机器**，
 *   条目就是本机那条预置（`api/homes.ts` 的 `LOCAL_HOME_ID`）；
 * - **文案不发明新词**：说的就是 `api/anjia.ts` 那句里已经写着的那件事（"先在这台机器上配好
 *   模型源"），目的地那一间的名字也叫「模型源」（`components/celira/Runtimes.tsx`）；
 * - **只给这一条拒绝**（判据是上一个函数）：别的子词要办的是别的事，指过去就是假路径。
 */
function ModelSourceExit() {
  const navigate = useNavigate()
  return (
    <Button
      variant="outline"
      size="sm"
      className="gap-1.5"
      onClick={() => navigate('/app/runtimes/local#model')}
    >
      <Server className="size-3.5" aria-hidden />
      去配模型源
    </Button>
  )
}

export function AgentsListPanel({
  agents,
  agentsKnown = true,
  onOpen,
}: {
  agents: OwnedAgent[]
  /**
   * 所有权清单**读到了没有**（`true` = 这份列表就是事实；空列表 = 真的一支都没有）。
   *
   * ⚠ 列表页只在"列表落定且没读失败"之后才挂载本组件（见 `MyAgentsPage` 那个
   * `!loading && !failed`），所以这里缺省 `true`。把它显式留一格，是因为
   * **"读不到"与"没有"不许在判据里长得一样**（`firstRunPlan` 的第二步就这么判）。
   */
  agentsKnown?: boolean
  onOpen: (id: string) => void
}) {
  const [createOpen, setCreateOpen] = useState(false)
  const { state } = useHomeEntries()
  // 每行现算认领（一份读数表 × N 行；表很小，且认领是**纯函数**）。
  const readings = state.kind === 'ready' ? agentHomeReadings(state.readings) : null
  // 「把它接到这台机器上」的读数（易失 store；创建成功时 store 自己落进去的）——
  // 这一页只**消费**它，**不发新请求**。
  const anjiaByAgent = useAppStore((s) => s.anjiaByAgent)
  // **随时可以建立"它 ↔ 这台机器"这段关系**（2026-10-09 主人裁 B）：调 store 里现成那条
  // （`attachAgent` ⇒ 壳的 `anjia_start` 六步），结果落回同一个读数 —— 本页不发别的新请求。
  const attachAgent = useAppStore((s) => s.attachAgent)
  const navigate = useNavigate()
  // 这一版页面**有没有壳**（与创建面板同一判据；承 `shell.ts` 判内部注入，不判平台名）。
  const hasShell = hasAnjiaBridge()
  /**
   * 三步清单的框架（与"是哪一支"无关的那一半）—— **只从这一页已经在读的那几份来**：
   * 本机那条条目的配置读数 ＋ 家条目数 ＋ 世界侧所有权清单。零新请求。
   *
   * ⚠ 模型那一格问**本机**：app 里"接上"只指本机（壳那六步），而这一步要办的事
   * 就是"这台机器上有没有模型源"（判据 `modelSourceAbsent`，与机器页那张卡同一个函数）。
   */
  const frame: FirstRunFrame = {
    config:
      state.kind === 'ready'
        ? (state.readings.find((each) => each.entry.local)?.config.view ?? null)
        : null,
    machines: state.kind === 'ready' ? state.entries.length : 0,
    agentsKnown,
    agentsCount: agents.length,
  }
  /**
   * 空态那一份（**还没有一支 Agent**）：三步清单。
   * ⚠ `null` = 模型那一格读不到（演示通道 / 门没开）⇒ 清单整块不摆，退回一句 ＋ 一颗按钮
   * —— 那时候"三步"的定义拿不出来，说"上面那三步"就是让人找不存在的东西。
   */
  const emptyPlan = firstRunPlan({ ...frame, agentId: null, resolution: null })

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      <div className="flex items-center gap-2 border-b border-border bg-card px-4 py-4">
        {/* ★ **一级入口不带返回箭头**（2026-10-11）：`/app/agents` 是导航里的一级入口，
            原先顶着一颗 `aria-label="返回我的"` ⇒ 一级内容拿二级待遇 ✗
            （这正是 `navGroups.ts` 当初把它加回导航时要治的那件事）。
            某一支 Agent 那一页的「返回我的 Agent」照旧在（那是二级页）。 */}
        <span className="text-title">我的 Agent</span>
        <span className="ml-2 text-caption text-muted-foreground">{agents.length} 个</span>
        {/* 高频动作直达（2026-09-23 裁决）：加号在列表右上角——所有软件教出来的肌肉记忆。
            填肉刀起它**不再跳走**：就地开一块内联面板（创建是这一页的事，不是「新的朋友」的事）。 */}
        <Button
          size="sm"
          className="ml-auto gap-1.5"
          aria-label="添加 Agent"
          aria-expanded={createOpen}
          onClick={() => setCreateOpen((open) => !open)}
        >
          <Plus className="size-4" />
          添加
        </Button>
      </div>
      <ScrollArea className="min-h-0 flex-1">
        <div className="mx-auto w-full max-w-3xl space-y-2 px-4 py-6">
          {/* **同一屏那句**（2026-10-03）：这份清单拼两件事，而其中一半来自**机器**。
              为什么必须有它：每一行摆着「状态 / 模型 / 最近活动」，用户看不出那三格不是服务器
              上的事实，而是"某台机器"上的读数（而且可能是"没有机器在跑它"）。判据不是"解释实现"，
              是**让人跨得过这个词**（与 `/app/runtimes` 底部那句同款）。
              ⚠ 放在行**之前**（那一页那句在列表末尾）：这里要先说清数字从哪来，再让人看数字。 */}
          <p className="px-1 pb-1 text-micro text-muted-foreground">
            这份清单拼两件事：世界里你拥有的 Agent，＋ 它们在哪台机器（执行端）上、在不在跑（机器上的数据）。
            点一行进去看它的公开资料。
          </p>
          {createOpen && (
            <CreateAgentInline
              onClose={() => setCreateOpen(false)}
              frame={frame}
              readings={readings}
            />
          )}
          {agents.map((participant) => {
            const resolution: AgentHomeResolution<HomeStatusView> | null = readings
              ? resolveAgentHome(readings, participant.id)
              : null
            // 「模型」那一格问**哪一条**：与「Agent 的主页 · 模型」同一手
            // （`mindReadoutFor` → `mindEntryFor`，一份权威）。旧写法在这里自己拼了一个
            // **少了"一份都没读回来、而只配了一个家"那一手**的局部判据 ⇒ 同一个输入下
            // 这一行说「模型：未知」而主页显示那台机器的读数（两条都在跑，门禁全绿）。
            const mind =
              state.kind === 'ready' && resolution
                ? mindReadoutFor({ resolution, entries: state.entries, readings: state.readings })
                : null
            const status = agentRowStatusFromHomes({
              resolution,
              config: mind?.view ?? null,
              agentId: participant.id,
            })
            // 「现在正常吗」那一句（**机器侧**）：宿主进程在不在跑 ＋ 账本该不该出手。
            // 与三格用**同一份认领裁决**（`agentHealthFromHomes` 内部照 `resolution.kind` 分派），
            // 判据与文案在 `api/agentHealth.ts`（纯函数 ＋ 用例）。
            const health = agentHealthFromHomes({ resolution })
            // ★ **四格同因 ⇒ 收成一句**（2026-10-20 T2 ④）：一份读数都没读回来时，
            // 不再并列四个"不知道"（判据与出口在 `api/agentRowStatus.ts` 的 `rowSameCauseLine`）。
            const sameCause = rowSameCauseLine({ resolution, config: mind?.view ?? null })
            // 这一支的机器侧结果（如果这一会话里问过）：四态 ＋ 人话在 `api/anjia.ts`，
            // 页面只把 `text` 原样渲染（两处各写一套话就是两个答案）。
            const anjia = anjiaByAgent[participant.id]
            const readout = anjia ? anjiaReadout(anjia.kind, anjia.sub) : null
            return (
              /* ⚠ **一行 = 一张卡，而卡里不止一个按钮**（2026-10-08）：拒绝那一句现在带一个出口
                 （`ModelSourceExit`），而交互元素不许套在按钮里（嵌套的按钮既不合规范，点内层
                 也会把外层那一下一起触发）⇒ 卡是 `div`，**点进主页那一条**才是按钮。
                 行的可访问名（`打开 X 的主页`）一字未动：真机门禁按它选行。 */
              <div
                key={participant.id}
                className="rounded-xl border border-border bg-card transition-colors hover:bg-muted"
              >
                <button
                  type="button"
                  onClick={() => onOpen(participant.id)}
                  aria-label={`打开 ${participant.displayName} 的主页`}
                  className="flex w-full cursor-pointer items-center gap-3 rounded-xl px-4 py-3 text-left"
                >
                  <ParticipantAvatar participant={participant} size="md" />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="truncate text-title">{participant.displayName}</span>
                      <AgentBadge />
                    </div>
                    {/* 机器侧那一句 ＋ 三格读数：Agent 现在正不正常（在不在跑、账本该不该出手）
                        ／ 在哪台机器上 ／ 思考用的是哪条钥匙 ／ 最近一次醒来的时间。
                        每一格各自独立 —— 哪一格没读到就说「读不到」或「未知」，不隐藏整格。
                        ⚠ **四格同因**（2026-10-20）：一句原因 ＋ 一个出口，不并列四个"不知道"。 */}
                    {participant.revoked ? (
                      // ⚠ **2026-09-29 真机实测修**：软撤销后参与者行仍在（契约：不硬删除、含已撤销
                      //    的历史），而这一行过去与"活着但没接入"长得一模一样。三格读数对一支已经
                      //    撤销的 Agent 也没有意义 —— 它不再接收事件、凭证与房间成员关系都已作废
                      //    ⇒ 就地改说这件事，不再画三格。那句后果说明与**深链接那一页同一个常量**
                      //    （`api/agentRevoked.ts`）：同一件事不许有两个说法。
                      <div className="mt-0.5 text-caption text-destructive">
                        {AGENT_REVOKED_CONSEQUENCE}
                      </div>
                    ) : sameCause !== null ? (
                      <div className="mt-0.5 text-caption text-muted-foreground">{sameCause.text}</div>
                    ) : (
                      <div className="mt-0.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-caption text-muted-foreground">
                        {/* 机器侧那一句在最前：它是"现在正常吗"的直接答案（其余三格是细节）。
                            颜色只映 tone，文字由 `api/agentHealth.ts` 原样给（两处各写一套话就是两个答案）。
                            ★ **一行只许一个状态点**（2026-10-20 T2 ③），且它只跟这一句的色档走
                            （"在不在跑 / 正不正常"）；住址那一格是**纯文本** —— 从前它自带一颗点，
                            于是"没在跑（黄字）"旁边挂着一颗绿点，读起来就是"它在跑"。 */}
                        <span className="inline-flex items-center gap-1.5">
                          <span
                            className={cn('size-1.5 shrink-0 rounded-full', READOUT_TONE_DOT[health.tone])}
                            aria-hidden
                          />
                          <span className={cn(READOUT_TONE_TEXT[health.tone])}>{health.text}</span>
                        </span>
                        <span>{status.onlineLabel}</span>
                        <span>{status.modelLabel}</span>
                        <span>{status.activityLabel}</span>
                      </div>
                    )}
                  </div>
                  <ChevronRight className="size-4 shrink-0 text-muted-foreground" />
                </button>
                {/* 四格同因那一句的**出口**（2026-10-20 T2 ④）：它在按钮外面（同一张卡里）——
                    `/app/runtimes` 才有"那台机器怎么了"的读数与改地址 / 换钥匙的入口。 */}
                {!participant.revoked && sameCause !== null && (
                  <div className="flex flex-wrap items-center gap-2 px-4 pb-3">
                    <Button
                      size="sm"
                      variant="outline"
                      className="gap-1.5"
                      onClick={() => navigate('/app/runtimes')}
                    >
                      <Server className="size-4" />
                      去我的执行端
                    </Button>
                  </div>
                )}
                {/* 机器侧结果（四态 ＋ 人话）就在这一行里 —— 从「新的朋友」创建完跳过来时，
                    「接上没有 / 缺什么」不用点进任何地方就能看见。数据是 store 里那份**易失**
                    读数（创建时壳报回来的），这一页不发新请求；人话来自 `api/anjia.ts`。
                    ⚠ 它在按钮**外面**（同一张卡里）：那句拒绝要带一个能走的出口，而出口不能
                    套在按钮里（见上面那条）。 */}
                {readout && (
                  <div className="flex flex-wrap items-center gap-2 px-4 pb-3">
                    <p role="status" className={cn('text-caption', READOUT_TONE_TEXT[readout.tone])}>
                      {readout.text}
                    </p>
                    {refusedForMissingModelCredential(anjia) && <ModelSourceExit />}
                  </div>
                )}
                {/* **本机没认领它**（`unknown-id` / `elsewhere`）⇒ 给一个出口（2026-10-09 主人裁 B）。
                    为什么要有这一格：创建那一刻会顺手接一次，而它可能被拒（缺模型源）——
                    从前那一拒就是死路（页面没有第二次 ✗，只能再造一支 ✗）。现在
                    **身份 ↔ 执行端的关系可以随时建立**：按钮调 store 里现成的 `attachAgent`
                    （壳的六步 `anjia_start` 一字未改），结果落回同一个读数。
                    ⚠ 那一句**对两态都成立**（"本机没认领它"是这两态的共同点）—— 别写成
                    `elsewhere` 专属那句「还没有机器在跑它」：`unknown-id` 是**说不准**，
                    写成断言就是替系统多说了一句（第一版正是这么错的 ✗）。
                    ⚠ **它不许被 readout 挡住**（2026-10-09 实测补）：第一次用的路径是
                    「创建被拒（缺模型源）⇒ 按出口去配好 ⇒ 回到这一页」——而那份拒绝读数是
                    **易失 store 里的旧话**，配好之后它照旧挂着 ⇒ 若出口要求"没有读数"，
                    用户配好了却**还是没得点**（只能重启 app 才恢复 ✗）。读数说**发生过什么**，
                    出口说**现在能做什么**，两件事不互相挡。
                    ⚠ 也别在"缺模型凭据"那一档把 `ModelSourceExit` 顶掉：那一档要的是
                    **先去配模型**（前置 ✓），配完回来这里照样给认领的出口 ✓。
                    ★ **2026-10-20（T1 ④）：这条出口在浏览器里也要有**。从前整块挂在
                    `hasShell` 上 ⇒ 没有壳的人读到"它还没接上"，却**没有任何出路**
                    （建得出、活不了 —— 这正是这一刀治的那件事）。
                    现在没有壳时给**深链** `/app/agents/<id>#machine`：那一段本来就能走远程家
                    （`/home-control/attach`，不需要壳），而且它**不复制**接上的逻辑 ——
                    写入口仍然只有那一段的按钮 ／ 壳那六步。
                    一条机器都没有时**不给按钮**，说清前提（要一台装了 Celira 的机器）并指
                    「我的执行端」：加一台的入口在那里，不指假路径。 */}
                {shouldOfferAttach(resolution) && (
                  <div className="flex flex-wrap items-center gap-2 px-4 pb-3">
                    <p role="status" className="text-caption text-muted-foreground">
                      它还没接到这台机器上 —— 接一下，它才能在这里回话。
                    </p>
                    {hasShell ? (
                      <Button
                        size="sm"
                        variant="outline"
                        className="gap-1.5"
                        onClick={() => void attachAgent(participant.id)}
                      >
                        <Server className="size-4" />
                        接到这台机器上
                      </Button>
                    ) : frame.machines > 0 ? (
                      <Button
                        size="sm"
                        variant="outline"
                        className="gap-1.5"
                        onClick={() => navigate(`/app/agents/${participant.id}#machine`)}
                      >
                        <Server className="size-4" />
                        去它的机器那一页
                      </Button>
                    ) : (
                      <p className="text-caption text-muted-foreground">
                        要一台装着 Celira 的机器 —— 在「我的执行端」里填上它的家地址和家钥匙。
                      </p>
                    )}
                  </div>
                )}
                {/* ⚠ 2026-10-20：这里原有的一格「刚创建那一支 · 浏览器里说 CREATE_WONT_REPLY」
                    随 `createdAgentId` 一起删除 —— 创建只发生在本页的就地面板里，而那句话
                    已经由面板自己说（`CreateAgentInline` 的成功横幅）。留着它就是一个**永远
                    不会成立的判断**（`justCreated` 恒 false），形状比事实多。 */}
              </div>
            )
          })}
          {agents.length === 0 && !createOpen && emptyPlan !== null && (
            /* 空态即引导（2026-09-23 裁决）：出口就在这里，不指路去别处。
               ★ 2026-10-20（T1 ①）：空态改成**三步清单**（模型源 / 建一支 / 接上一台机器）——
               空账号 + 空机器的人在这里一次看见"还差什么、各去哪"，而不是只看见一颗
               "创建"按钮（建得出、活不了）。第二步的出口就是那颗「创建」，所以不单摆一颗。 */
            <div className="space-y-3">
              <FirstRunChecklist plan={emptyPlan} onCreate={() => setCreateOpen(true)} />
              <p className="px-1 text-caption text-muted-foreground">
                还没有 Agent —— 上面那三步走完，它才会回你的第一句话。
              </p>
            </div>
          )}
          {agents.length === 0 && !createOpen && emptyPlan === null && (
            /* 读不到模型那一格（演示通道 / 门没开）⇒ **不摆三步清单**（判据在
               `api/firstRun.ts`：读不到不摆），退回一句 ＋ 一颗按钮 —— 那时候"三步"的定义
               拿不出来，说"上面那三步"就是让人找不存在的东西。 */
            <div className="flex flex-col items-center gap-3 px-3 py-10 text-center">
              <p className="text-caption text-muted-foreground">还没有 Agent</p>
              <Button size="sm" className="gap-1.5" onClick={() => setCreateOpen(true)}>
                <Plus className="size-4" />
                创建第一个 Agent
              </Button>
            </div>
          )}
        </div>
      </ScrollArea>
    </div>
  )
}

/**
 * 内联创建面板：**两条路，本刀只通一条**。
 *
 * - **默认路「创建」**：建**世界侧身份**（`POST /v1/agents`），然后按**有没有壳**分流 ——
 *   - **在壳里**（首刀步骤 2，2026-09-30）：创建成功 ⇒ **自动**把它接到这台机器上
 *     （壳的命令 `anjia_start`），四态如实显示（进行中 / 成功 / 失败 / 读不到）；
 *   - **在浏览器里**：没有这条路径（壳不在），横幅照旧说 `CREATE_WONT_REPLY` —— 那是事实。
 *   「安家」「入籍」是**设计词汇，不上屏**：界面语言只叫「创建」／「接到这台机器上」
 *   （人话刀裁决：设计词汇给设计者，界面语言给用户）。
 * - **进阶路「只创建本地」**：本刀**只留入口不留功能** —— 禁用态 ＋ 明说「尚未支持」。
 *   诚实降级：按钮点了没反应比"看起来能用"好，但比"看上去像坏掉的按钮"差 ——
 *   所以文案里把「尚未支持」说出来，而不是留一个哑按钮。
 *
 * 边界：创建表单**就一个名字**（不加头像 / self_concept / 模型选择 —— 「创建 ≠ 配置」是既有裁决）；
 * 名字的长度与形状规则**不在这里判**（Core 是唯一裁决者，`agentCreateErrorText` 会把它的拒绝说成人话）。
 *
 * 四态的**判据与文案**不在这里：`api/anjia.ts` 的 `anjiaReadout`（纯函数 ＋ 用例），
 * 页面只负责把 `state` 映成颜色、把 `text` 原样渲染。
 */

/**
 * 四态的色档 → 语义令牌类（颜色只许来自 `tokens.css`，`scripts/check-tokens.mjs` 守着）。
 *
 * ⚠ **2026-10-06 搬到 `status-tone.ts`**：Agent 主页「机器」那一段现在渲染的是**逐字相同的
 * 同一句话**（同一个 `agentHealthReadout`）⇒ 映射必须只有一处，否则就是"同一句话、两种颜色"。
 */
function CreateAgentInline({
  onClose,
  frame,
  readings,
}: {
  onClose: () => void
  /** 三步清单的框架（与"是哪一支"无关的那一半）—— 由面板传下来（**不在这里再取一次数**）。 */
  frame: FirstRunFrame
  /** 家条目读数（认领用）；`null` = 这一轮没读到。 */
  readings: HomeReading<HomeStatusView>[] | null
}) {
  const createAgent = useAppStore((s) => s.createAgent)
  const refreshOwnedAgents = useAppStore((s) => s.refreshOwnedAgents)
  const anjiaByAgent = useAppStore((s) => s.anjiaByAgent)
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [createdName, setCreatedName] = useState<string | null>(null)
  /** 刚创建**那一支**的 id：读数按它取（没有它就不显示读数，只显示诚实横幅）。 */
  const [createdId, setCreatedId] = useState<string | null>(null)
  const trimmed = name.trim()
  /** 这一版页面**有没有壳**：判据是内部注入（承 `shell.ts`），不是平台名。 */
  const hasShell = hasAnjiaBridge()
  /** 壳对这一支的最近一次读数（`undefined` = 还没问过／不在壳里）。 */
  const reading = createdId ? anjiaByAgent[createdId] : undefined
  const readout = reading ? anjiaReadout(reading.kind, reading.sub) : null
  /**
   * ★ **创建之前就说清"还差什么"**（2026-10-20 T1 ①/③）：
   * 判据全在 `api/firstRun.ts`（纯函数、有用例）—— 这里只把这一页**已经在读**的三份
   * 交给它：本机配置读数（`frame.config`）× 这一支的认领（`resolveAgentHome`）×
   * 壳最近一次读数（`anjiaByAgent`）。**零新请求**（读数由面板传下来，不在这里再取一次）。
   *
   * ⚠ `onCreate` 传 `null`：创建这颗按钮**就在下面**（同一个面板），不必在清单里再摆一颗
   * —— 同一件事两个按钮只会让人猜哪个才算。
   */
  const plan = firstRunPlan({
    ...frame,
    agentId: createdId,
    resolution: readings ? resolveAgentHome(readings, createdId) : null,
    anjia: reading,
  })

  const submit = async () => {
    if (!trimmed || busy) return
    setBusy(true)
    setError(null)
    try {
      // 幂等 key：**不传** —— `core.ts` 的 `createAgent` 每次自己生成一把
      // （`newIdempotencyKey()`）。⚠ 任务书说的"每个逻辑创建一个内存 key、复用/轮换"
      // 是 **Host 时代**的纪律，已随 TASK_64 整体删除；既有实现（`NewContactPage`）也不传。
      // 在这里另造一套 key 就是**第二份纪律**，两处迟早说不同的话。
      const agentId = await createAgent(trimmed, undefined)
      // 权威重读列表：store 里那次是 fire-and-forget，这里**等它落地**再报成功 ——
      // 否则"已创建"和列表里那一行可能差一轮（看着像没建成）。
      await refreshOwnedAgents()
      setCreatedName(trimmed)
      setCreatedId(agentId || null)
      setName('')
      // 没拿到 id 说明后端没有这条路径（`createAgent` 在无 api 时返回空串）——
      // 那时**不报成功**（报成功才是撒谎）。
      if (!agentId) setError('这台后端没有创建 Agent 的接口，请联系提供方。')
    } catch (e) {
      // 红路径不许静默：走 Agent Create 那一族的稳定文案（绝不出示服务端原文）。
      setError(agentCreateErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-3 rounded-xl border border-border bg-card p-4">
      {/* ★ **创建之前就说缺什么**（2026-10-20 T1 ③）：三步清单在面板**顶部**，
          缺模型源那一步自带出口（「去配模型源」）—— 不是只印一句"它不会回话"。
          ⚠ 模型那一格读不到时 `plan` 是 `null`（**读不到不摆**，判据在 `api/firstRun.ts`）。 */}
      {plan !== null && <FirstRunChecklist plan={plan} onCreate={null} />}
      <div className="text-body">创建 Agent</div>

      {/* 默认路（已选中）：本刀唯一能走通的那条。 */}
      <div className="flex items-start gap-3 rounded-lg border border-primary/60 bg-muted px-3 py-2.5">
        <Check className="mt-0.5 size-4 shrink-0 text-primary" aria-hidden />
        <div className="min-w-0">
          <div className="text-body">创建</div>
          {/* ⚠ 这句是**预期**，与创建后横幅那句**事实**共用同一个字面量（`CREATE_WONT_REPLY`）
              —— 创建只建公共身份：Agent 既没有模型、也没有机器在跑它（给一支已有身份**安家**在
              **app 里**没有路径——机器侧只有手册里那条窄路 `up --agent`，且只通"这个家还没有住户"
              的场合，所以这里不能指一扇走不通的门）。两句**已退役**的话点在这里：
              git 里那句「创建后即可与你聊天」是**过度承诺**；本次工作树里被推翻的
              「创建后还要给 Agent 配模型、在这台机器上安家，它才会回话」是**假路径**。 */}
          <div className="text-caption text-muted-foreground">
            {hasShell
              ? /* 壳里：创建完会**自动**接一次 —— 正路是真的，所以不再说"没有机器在跑它"。
                   ⚠ 预期句只说"会自动接"；**接成没接成由读数如实给**（四态）。 */
                '创建后会自动把它接到这台机器上（这台机器上还没配好模型的话，它接上了也暂时不会回话）。'
              : /* 浏览器里：没有壳就没有这条路径 —— 那两句**仍是事实**，一个字不改。 */
                `创建后${CREATE_WONT_REPLY}。`}
          </div>
        </div>
      </div>

      {/* 进阶路：禁用态 ＋ 明说「尚未支持」（本刀只留入口不留功能）。
          ⚠ 措辞纪律：**不承诺"即将提供"** —— "尚未"是当下的事实，"即将"是对未来的承诺
          （同 `ChatHeader` 的通话提示、`FixturePageUnavailable`）。 */}
      <button
        type="button"
        disabled
        title="尚未支持"
        className="flex w-full cursor-not-allowed items-start gap-3 rounded-lg border border-border px-3 py-2.5 text-left opacity-60"
      >
        <span className="mt-0.5 size-4 shrink-0 rounded-full border border-border" aria-hidden />
        <span className="min-w-0 flex-1">
          <span className="text-body">只创建本地</span>
          <span className="block text-caption text-muted-foreground">不接入服务器，只在本机创建</span>
        </span>
        <span className="shrink-0 rounded-full border border-border px-2 py-0.5 text-micro text-muted-foreground">
          尚未支持
        </span>
      </button>

      <label className="block space-y-1.5">
        <span className="text-caption text-muted-foreground">名字</span>
        <Input
          value={name}
          onChange={(event) => {
            setName(event.target.value)
            setError(null)
          }}
          placeholder="给你的 Agent 起个名字…"
          disabled={busy}
          className="bg-muted"
        />
      </label>

      <div className="flex items-center gap-2">
        <Button size="sm" disabled={!trimmed || busy} onClick={() => void submit()}>
          {busy ? '创建中…' : '创建'}
        </Button>
        <Button size="sm" variant="ghost" onClick={onClose}>
          取消
        </Button>
      </div>

      {error && (
        <p role="alert" className="text-caption text-presence-failed">
          {error}
        </p>
      )}

      {createdName && (
        <div className="space-y-1.5 rounded-lg border border-border bg-muted px-3 py-2.5">
          <div className="text-body text-presence-online">已创建「{createdName}」</div>
          {hasShell && readout ? (
            /* **壳里**：这一支的机器侧结果（四态里的一条）。文案来自 `anjiaReadout`
               ——与壳的词表同源；页面**不自己拼话**（两处各写一套就是两个答案）。
               ⚠ 读数在场时**不**再挂 `CREATE_WONT_REPLY`：它在壳里已经不是事实了。
               ⚠ "缺模型凭据"那一条**带出口**（与列表行同一件东西、同一个去处）：那句话说的是
               这台机器上还缺什么，而补它的地方就是这台机器的「模型」那一间。 */
            <div className="flex flex-wrap items-center gap-2">
              <p role="status" className={cn('text-caption', READOUT_TONE_TEXT[readout.tone])}>
                {readout.text}
              </p>
              {refusedForMissingModelCredential(reading) && <ModelSourceExit />}
            </div>
          ) : (
            /* **浏览器里**（没有壳）：这条诚实横幅一个字不改 —— 在这个运行环境里它是事实。
               ⚠ 它渲染的就是 `CREATE_WONT_REPLY` 那一个常量（与上面的预期句同源），
               所以两者不可能漂移；**别在这里内联字符串**。
               ⚠ 也不指「设置 → 模型配置」：模型绑定是"某台机器上那个 Agent"的事，
               一个没有家的 Agent 在 Agent 的模型房间就是「未接入」，根本绑不上（假路径，2026-09-24 刀删）。 */
            <p className="text-caption text-muted-foreground">{CREATE_WONT_REPLY}</p>
          )}
        </div>
      )}
    </div>
  )
}

/**
 * ★ **三步清单**（2026-10-20 T1 ①②③）：模型源 ／ 建一支 ／ 接上一台机器。
 *
 * ## 它为什么是这个形状
 *
 * 目标句：**新账号 + 一台空机器**，从 `/app/agents` 空态出发、不离开这块区域、不查文档，
 * 就能走到"它被接上且有模型源"。所以这一块只做三件事：说清**还差哪一步**、给**一个能走的出口**、
 * 已成立的那步**收成一行**（不再占地方）。
 *
 * ## 判据不在这里
 *
 * 三步的处境与出口全由 `api/firstRun.ts` 的 `firstRunPlan` 给（纯函数、有用例）——
 * 本层只把 `exit.kind` 映成导航（四个目的地都是既有页面，一个都不新开）：
 *
 * | exit | 去哪 |
 * |---|---|
 * | `model-source` | `/app/runtimes/local#model`（那台机器「模型」那一间，与列表行同一处） |
 * | `machines` | `/app/runtimes`（加一台机器的入口在那儿） |
 * | `open-agent` | `/app/agents/<id>#machine`（"可以让它去哪台机器上跑"那一段） |
 * | `create` | 这块面板自己（空态那颗按钮；面板里已经有表单 ⇒ `onCreate === null` 时不摆） |
 *
 * ⚠ **`plan === null` 就什么都不渲染**（模型那一格读不到）—— 与"读不到不摆"同一条纪律。
 * ⚠ 勾/叉是**文字**不是图标：真机门禁按 `✓` / `✗` 判"三步在不在"（形状比图标稳）。
 */
function FirstRunChecklist({
  plan,
  onCreate,
}: {
  plan: FirstRunPlan | null
  /** 空态那颗「创建」；面板里传 `null`（表单就在下面，不再摆第二颗）。 */
  onCreate: (() => void) | null
}) {
  const navigate = useNavigate()
  if (plan === null) return null
  const go = (exit: FirstRunExit) => {
    if (exit.kind === 'model-source') navigate(`/app/runtimes/${LOCAL_HOME_ID}#model`)
    else if (exit.kind === 'machines') navigate('/app/runtimes')
    else if (exit.kind === 'open-agent') navigate(`/app/agents/${exit.agentId}#machine`)
    else onCreate?.()
  }
  const mark = (state: FirstRunStepState) => (state === 'done' ? '✓' : state === 'todo' ? '✗' : '…')
  const allDone = plan.next === null && plan.steps.every((step) => step.state === 'done')
  return (
    <div className="space-y-2 rounded-xl border border-border bg-card p-4">
      <div className="flex flex-wrap items-baseline gap-2">
        <span className="text-body">三步把它跑起来</span>
        <span className="text-micro text-muted-foreground">
          {allDone ? '三步都齐了 —— 它下次醒来就能回话。' : '三步都齐了，它才会回你的第一句话。'}
        </span>
      </div>
      <ul className="space-y-1.5">
        {plan.steps.map((step) => {
          const exit = step.exit
          // 面板里不摆第二颗「创建」（`onCreate === null`）；其余出口照摆。
          const showExit = exit !== null && !(exit.kind === 'create' && onCreate === null)
          return (
            <li key={step.key} className="flex flex-wrap items-center gap-2">
              <span
                aria-hidden
                className={cn(
                  'w-3 shrink-0 text-center text-caption',
                  step.state === 'done' ? 'text-presence-online' : 'text-muted-foreground',
                )}
              >
                {mark(step.state)}
              </span>
              <span
                className={cn(
                  'shrink-0 text-caption',
                  step.state === 'done' ? 'text-muted-foreground' : 'text-foreground',
                )}
              >
                {step.title}
              </span>
              <span className="min-w-0 flex-1 text-caption text-muted-foreground">{step.detail}</span>
              {showExit &&
                (exit.kind === 'create' ? (
                  <Button size="sm" className="gap-1.5" onClick={() => go(exit)}>
                    <Plus className="size-4" />
                    {exit.label}
                  </Button>
                ) : (
                  <Button size="sm" variant="outline" className="gap-1.5" onClick={() => go(exit)}>
                    <Server className="size-4" />
                    {exit.label}
                  </Button>
                ))}
            </li>
          )
        })}
      </ul>
    </div>
  )
}
