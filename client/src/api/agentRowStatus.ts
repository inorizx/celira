/**
 * 「我的 Agent」列表每一行那三格状态的**纯逻辑**（前端填肉刀，2026-09-23）。
 *
 * 三格都是**读数**，不是动作：整行点下去仍然只是进 Agent 的主页（点击行为不归本层）。
 * 三个数据源**各自独立、各自可能读不到** —— 一格读不到不许连累另外两格
 * （缺省不等于零，也不等于"整行没数据"）。
 *
 * | 那一格 | 读数来源 | 这一格读不到时 |
 * |---|---|---|
 * | 住址 | `/home-status` 的 `agent.agentId` 与这一行的 participant id 比（`agentAgentMatch`） | 「不知道在哪台机器上」 |
 * | 模型 | 模型配置门的**绑定**那一层，**逐支**读（`residentModelReadout`；老门缺那一格时退回整机 `bound`） | 「模型：未知」 |
 * | 最近活动 | `/home-status` 的 `lastWake` | 「最近活动：未知」 |
 *
 * 两条口径写死在判据里（都是踩出来的）：
 *
 * 1. **`unknown` 不冒充「不在这台机器上」**：`different` 才是"这台机器上跑的是别人"；
 *    `unknown` 是"这一格**没有读数**"（环境根记录读不出来 / 口子版本旧）⇒ 它只能说
 *    「不知道在哪台机器上」。把读不到说成一个事实，正是刀2a 治过的病。
 * 1b. ★ **住址那一格只答住址**（2026-10-20）：从前它写「在本机运行」——
 *    "在不在跑"另有其格（`api/agentHealth.ts`），一个词答两件事就会并排出两个相反的答案。
 * 2. **`lastWake` 不是时间串**：它是状态口那一行
 *    `wake run=host-<pid>-<毫秒>-<序号> drives=… ending=…` 的**原文**。
 *    直接交给 `formatRuntimeTime` 只会得到 `—`（解不出日期）⇒ 时间从活动标识里还原
 *    （`wakeLineAtMs`），并把"时间是推算的"这件事写在文案里（`（推算）`）。
 *    这一转换是**唯一一份**，已提升为导出的 `lastWakeReadout`（"最近活动"那一格与
 *    聊天页头部那条 Agent 家读数都调它 —— 本文件末尾那一节）。
 *
 * 纯函数、零副作用、只 `import type` 加几条值导入（`agentAgentMatch` / `resolveAgentHome` /
 * `wakeLineAtMs` 都是纯函数）—— 所以能被 `node --test` 直接钉住
 * （`tests/agent-row-status.test.ts`）。
 */

import { agentAgentMatch, hostStateOf, type HomeStatusView } from './homeStatus.ts'
import { residentModelReadout, type HomeConfigView } from './homeConfig.ts'
import { claimedHostOf, type AgentHomeResolution } from './homes.ts'
import { wakeLineAtMs } from '../lib/agent-days.ts'
import { formatRuntimeTime } from '../lib/time.ts'
import { machineMatchLabel } from '../lib/plain-language.ts'

/**
 * **机器列表一行**的处境文案（`/app/runtimes` 的 `RuntimeRow`）—— 纯逻辑，可直测。
 *
 * ## 为什么把它从页面里提出来
 *
 * 2026-09-26 审计在这里抓到一处**"把读不到说成事实"**：第四态原先只按"名字查不到"
 * 就断言「上面那支 Agent **不在这个世界里**」—— 而名字是从 `ownedAgents` 里查的，
 * 那份列表**读失败时是空数组**，于是**一次读取失败被页面断言成一句事实**
 * （仓库自己在 `lib/plain-language.ts` 写着：「把 `unknown` 说成「未接入」，
 * 就是把读不到说成一个事实」）。同一条纪律在本文件第 1 条口径里已经写过一遍。
 *
 * 提出来是为了让它**可被机械钉住**：页面里那段三目嵌套没法测，这里可以。
 *
 * ⚠ 四态**不压成两态**（读不到 ≠ 上面没 Agent），而且第四态**再分两支**：
 * 只有**真的读到过**那份列表，才有资格说"不在这个世界里"。
 */
export type MachineSituationTone = 'ok' | 'warn' | 'muted'

/**
 * **处境是哪一档**（★ 2026-10-11 加）。
 *
 * 为什么要有它：这一族从前只有 `text`，而**同一个 `text` 被两种完全不同的处境共用** ——
 * 一台**还没有 Agent 的新家**（正是第一次那条路）与一台**有 Agent 但没报上 id** 的机器，
 * 都走进 `agentId === null` 那一支，于是新用户读到「它没报上上面跑的是谁（可能是记录读不出来，
 * 或口子版本旧）」—— **假警报**（`status-server.js` 那条"缺省不等于故障"的注释写着同一类事故，
 * 当时修的是「运行」那一间、漏了这里）。⇒ **三态分开，谁也不冒充谁**：
 *
 * - `no-resident`：这台机器上还没有 Agent（**缺省**，不是故障）；
 * - `legacy-home`：这个家还是老形态（那一支住在旧布局里 ⇒ 报不出 id）；
 * - `no-id`：有读数、但那台**没报**是谁（记录读不出来 / 口子版本旧）。
 *
 * 页面按 `kind` 决定**自己那一段怎么说**（机器页那张卡说得更完整、家列表那行说"住"），
 * 但**判据只有这一处** —— 两个语域，一个判断。
 */
export type MachineSituationKind =
  | 'pending'
  | 'unreadable'
  | 'no-resident'
  | 'legacy-home'
  | 'no-id'
  | 'unknown-agent'
  | 'list-unreadable'
  | 'named'

export interface MachineSituation {
  kind: MachineSituationKind
  tone: MachineSituationTone
  /** 这一档在**列表行**那个语域里的一句话（页面要更完整的说法时按 `kind` 自己写）。 */
  text: string
}

export function machineSituation(input: {
  /** 这一格还没开始读（`reading` 为 undefined）。 */
  pending: boolean
  /** 那台机器的读数（`null` = 这一份没读到，`failure` 是固定人话）。本函数只用它判"有没有读到"。 */
  view: object | null
  failure: string | null
  /** 那台机器上报的 Agent 状态（`'no-resident'` = 一台都没有）。 */
  agentState: string | null
  /** 那台机器说这个家还是老形态（旧布局 ⇒ 报不出 id）。 */
  legacyHome: boolean
  /** 那台机器上报的 Agent id（`null` = 它没报）。 */
  agentId: string | null
  /** 在世界侧 Agent 列表里查到的名字（`null` = 查不到）。 */
  agentName: string | null
  /** **世界侧那份 Agent 列表这次读到了吗** —— 没读到就无权说"不在这个世界里"。 */
  agentsReadable: boolean
  /**
   * ★ **这台机器上报的**全部住户**（`residents[]`）—— 一支一项（2026-10-16 加）。
   *
   * 为什么必须收它：顶层那一格（`agentId` / `agentName`）是**第一支**（口子把它抬到顶层，
   * 为的是不破坏冻结前端）⇒ 一个家住两支时，那一句「上面跑的是 X」只说了**一半**，
   * 而读的人会以为**只有** X 在跑（真机撞上：小可明明也在跑，列表却只说小欣
   * —— 主人当次的原话："这里为啥还显示跑的是小欣，明明小可也在"）。
   *
   * ⚠ **可选**：老门 / 别的调用点不给这一格 ⇒ 退回顶层那一支（单支的家两者等价，
   * 一个字节都不变）。给了 ⇒ `agentId` / `agentName` 只当"第一支"用，说全话靠这一格。
   */
  residents?: {
    agentId: string | null
    /** 这一支在**世界侧**查到的名字（`null` = 查不到 —— 与"没有这一支"是两件事）。 */
    name: string | null
    /** 宿主那一档（`hostStateOf` 的四个取值之一；`null` = 没报/读不到）。 */
    hostState: string | null
  }[]
}): MachineSituation {
  if (input.pending) return { kind: 'pending', tone: 'muted', text: '正在读…' }
  if (input.view === null) {
    return { kind: 'unreadable', tone: 'warn', text: input.failure ?? '读不到' }
  }
  // ★ **缺省 / 老形态 / 读不到 id 三态**（2026-10-11）：先判"这台机器上有没有 Agent"，
  // 再判"报没报上来是谁" —— 顺序反了就会把新家说成"读不出来"。
  if (input.agentState === 'no-resident') {
    return input.legacyHome
      ? { kind: 'legacy-home', tone: 'muted', text: '已连上 · 这个家还是老形态' }
      : { kind: 'no-resident', tone: 'muted', text: '已连上 · 这台机器上还没有 Agent' }
  }
  // ★ **多支那一档**（2026-10-16）：顶层只有第一支 ⇒ 住户表在场且不止一支时，说**全**。
  //   判据（直觉版）：**"上面跑的是谁"这句话，机器上有几支就得说几支** —— 少说一支
  //   就是把"也在跑"读成"没在跑"（真机上主人当场撞上）。
  const many = manyResidentsLine(input)
  if (many !== null) return many
  if (input.agentId === null) {
    return { kind: 'no-id', tone: 'muted', text: '已连上 · 它没报上上面跑的是谁' }
  }
  if (input.agentName === null) {
    return input.agentsReadable
      ? { kind: 'unknown-agent', tone: 'muted', text: '已连上 · 上面那支 Agent 不在这个世界里' }
      : { kind: 'list-unreadable', tone: 'warn', text: '已连上 · 读不到你的 Agent 列表，说不出上面是谁' }
  }
  return { kind: 'named', tone: 'ok', text: `已连上 · 上面跑的是 ${input.agentName}` }
}

/** 在跑（含"接管来的孤儿"——它确实在跑，只是账本没指向它）。 */
function runningHere(hostState: string | null): boolean {
  return hostState === 'running' || hostState === 'running-orphan'
}

/**
 * **多支那一句**（`null` = 这一档不适用 ⇒ 走下面单支那几条既有判据）。
 *
 * 三档都有各自的实话（**不许压成"上面跑的是 X"**）：
 * - 有几支在跑 ⇒ 点名那几支，并说出一共几支里有几支没在跑；
 * - 一支都没在跑 ⇒ 如实说"住着 N 支、现在都没在跑"（**灰点**，不是绿点）；
 * - 名字读不到 ⇒ 说得出几支、说得出哪几支的名字读不到（**不许**只列查得到的那些 ——
 *   那正是这一次的错：把"也在"吞掉）。
 *
 * ⚠ 住户**不足两支** ⇒ `null`（单支的家照旧一字不变：那是既有判据与既有断言的地盘）。
 */
function manyResidentsLine(input: {
  residents?: { agentId: string | null; name: string | null; hostState: string | null }[]
  agentsReadable: boolean
}): MachineSituation | null {
  const all = input.residents
  if (all === undefined || all.length < 2) return null
  const running = all.filter((each) => runningHere(each.hostState))
  const named = running.filter((each) => each.name !== null).map((each) => each.name)
  const head = named.length > 0 ? named.join('、') : null
  const missed = running.length - named.length
  const rest = all.length - running.length
  if (running.length === 0) {
    return {
      kind: 'no-id',
      tone: 'muted',
      text: `已连上 · 这台机器上住着 ${all.length} 支，现在都没在跑`,
    }
  }
  if (!input.agentsReadable || head === null) {
    return {
      kind: 'list-unreadable',
      tone: 'warn',
      text: `已连上 · 上面跑着 ${running.length} 支（共 ${all.length} 支），但名字读不到`,
    }
  }
  const who =
    missed > 0 ? `${head}（还有 ${missed} 支名字读不到）` : head
  const tail = rest > 0 ? ` —— 另外 ${rest} 支没在跑` : ''
  return {
    kind: 'named',
    tone: 'ok',
    text: `已连上 · 上面跑着 ${running.length} 支（共 ${all.length} 支）：${who}${tail}`,
  }
}

/**
 * 在线那一格的色标（**只管语义，不管颜色**：Tailwind 类在组件那一层映）。
 *
 * - `local` = 读数说这台机器上跑的就是 Agent（绿点）；
 * - `away` = 读数说是别人（灰点）；
 * - `unknown` = 这一格没有读数（空心灰点 —— 与「确实是别人」长得不一样）。
 */
export type AgentRowTone = 'local' | 'away' | 'unknown'

export interface AgentRowStatus {
  /** 在线那一格：人话（`machineMatchLabel`）。 */
  onlineLabel: string
  onlineTone: AgentRowTone
  /** 模型那一格：已带前缀（「模型：…」）。 */
  modelLabel: string
  /** 最近活动那一格：已带前缀（「最近活动：…」）。 */
  activityLabel: string
}

/** 「模型」前缀与「最近活动」前缀各只在这一处出现（两处写法不许各写一遍）。 */
const MODEL_PREFIX = '模型：'
const ACTIVITY_PREFIX = '最近活动：'

/**
 * 三格一起算。
 *
 * ⚠ `view` / `config` 用 `null` 表示**那一份读数这次没读到**（加载中与读失败对这三格
 * 是同一件事：没有读数）。它们是两份**独立**的读数，所以也各自传：一份读不到只塌自己那一格。
 */
export function agentRowStatus(input: {
  /** `/home-status` 的读数；`null` = 这一次没读到。 */
  view: HomeStatusView | null
  /** 本机模型配置门的读数；`null` = 这一次没读到。 */
  config: HomeConfigView | null
  /** 这一行那个 Agent 的 participant id。 */
  agentId: string | null | undefined
}): AgentRowStatus {
  const online = onlineCell(input.view, input.agentId)
  return {
    onlineLabel: online.label,
    onlineTone: online.tone,
    modelLabel: modelCell(input.config, input.agentId),
    activityLabel: activityCell(input.view),
  }
}

/**
 * 按**家条目认领**算一行三格（家口远程访问刀）——列表页用的那一版。
 *
 * 认领结果（`resolveAgentHome`）与执行端那一页的「运行」房间是**同一份裁决**，这里只把它翻成三格：
 *
 * | 认领 | 住址那一格 | 模型 / 最近活动 |
 * |---|---|---|
 * | `claimed`（本机） | 「在这台机器上」（绿） | 那台机器的读数 |
 * | `claimed`（远程） | 「在<那台的名字>上」（绿） | 那台机器的读数 |
 * | `unknown-id`（读数没报 id） | 「不知道在哪台机器上」 | 那台机器的读数（**照旧显示**，不冒充不在这台） |
 * | `elsewhere`（都是别人） | 「不在这台机器上」（灰） | 未知（那些读数**不是 Agent**的 —— 不许拿别的机器的来顶） |
 * | `unreachable`（一份没读到） | 「不知道在哪台机器上」 | **调用方给的那一条的读数**（`mindEntryFor`：只配了一个家 ⇒ 没有歧义；没有 ⇒ 未知） |
 * | `null`（条目还在读） | 「不知道在哪台机器上」 | 未知 |
 *
 * `config` 是**认领到的那一条**（或 `mindEntryFor` 挑出的那一条）的配置读数（谁的绑定就是
 * 谁的，别台的钥匙不许上这一行）。调用方拿它就调 `mindReadoutFor` —— **不许自己拼**。
 *
 * ⚠ 本函数**不许把 `config` 丢掉**：`view` / `config` 是两份**独立**的读数，一份读不到只塌
 * 自己那一格（同一文件 `agentRowStatus` 那一版有用例钉着这条：「状态口读不到不该把模型那一格
 * 一起塌掉」）。旧实现这里对 `unreachable` 硬写 `config: null`，于是同一个输入下
 * 「Agent 的主页 · 模型」显示那台机器的读数、这一行说「模型：未知」—— 自己跟自己矛盾。
 */
export function agentRowStatusFromHomes(input: {
  /** 认领结果；`null` = 家条目还没就绪。 */
  resolution: AgentHomeResolution<HomeStatusView> | null
  config: HomeConfigView | null
  agentId: string | null | undefined
}): AgentRowStatus {
  const { resolution, config, agentId } = input
  if (resolution === null) return agentRowStatus({ view: null, config: null, agentId })
  if (resolution.kind === 'claimed') {
    // ★ **认领靠住户清单时（第 2 支起），顶层那几格不是它的**（2026-10-08）：
    //   在线那一格不许拿顶层读数算（`agentAgentMatch` 会算出"不是这一支"——而它**就是**，
    //   只是住在同一个家的另一个环境根里）；最近活动那一格更不许拿第一支的 `lastWake` 顶。
    //   ⇒ 在线照旧绿（认领本身就是证据），活动如实说"读不到"。
    if (!resolution.ownReadings) {
      const host = claimedHostOf(resolution, agentId)
      const base: AgentRowStatus = {
        onlineLabel: machineMatchLabel('same'),
        onlineTone: 'local',
        modelLabel: modelCell(config, agentId),
        // ⚠ 2026-10-20：「读数」是内行词（`scripts/jargon.mjs`），而且这一句说的是
        //    "这一份里没有它的记录" —— 不是"没有记录"。
        activityLabel: `${ACTIVITY_PREFIX}读不到（这一份不是它的）`,
      }
      // 它**自己**那一项里有宿主读数 ⇒ 顺手说清"它的宿主在不在跑"（读不到就照旧不说）。
      // ⚠ "在不在跑"归健康那一句（`api/agentHealth.ts`），这一格只补一句并列的细节 ——
      //   两处说的是同一个事实，所以这里只许说"宿主"，不许再说一次"它在跑"。
      if (hostStateOf(host) === 'stopped') {
        return { ...base, onlineLabel: `${base.onlineLabel} · 它的宿主没在跑` }
      }
      return base
    }
    const base = agentRowStatus({ view: resolution.view, config, agentId })
    if (resolution.entry.local) return base
    // 远程那台：住址就是它的名字（⚠ 只说"在哪台"，不再借"运行"两个字 —— 那是健康那一句的活）。
    return { ...base, onlineLabel: `在${resolution.entry.name}上` }
  }
  if (resolution.kind === 'unknown-id') {
    return agentRowStatus({ view: resolution.view, config, agentId })
  }
  if (resolution.kind === 'elsewhere') {
    return {
      onlineLabel: machineMatchLabel('different'),
      onlineTone: 'away',
      modelLabel: `${MODEL_PREFIX}未知`,
      activityLabel: `${ACTIVITY_PREFIX}未知`,
    }
  }
  // unreachable：状态那一格塌成「状态未知」，但**模型那一格不跟着塌** —— 调用方给的 config
  // 是 `mindEntryFor` 挑出的那一条（一份都没读回来而只配了一个家 ⇒ 用它，没有歧义）。
  // 旧实现这里硬写 `config: null`：既违反本文件自己的原则，也让这一行与主页互相矛盾。
  return agentRowStatus({ view: null, config, agentId })
}

/**
 * **四格同因 ⇒ 收成一句**（2026-10-20 T2 ④）。
 *
 * ## 它治的是什么
 *
 * 一份读数都没读回来时（那台机器没开机 / 地址不对 / 配置门也没开），那一行从前并列摆着
 * 四句"不知道"：`机器这一面读不到` ／ `不知道在哪台机器上` ／ `模型：未知` ／
 * `最近活动：未知`。四个词、一个原因 —— 用户读到的是"四件不同的事都坏了"，
 * 而事实是**一件事坏了**（那一份读数没来）。⇒ 同因就收成一句 ＋ 一个出口。
 *
 * ## 判据（两条一起看，缺一条都不收）
 *
 * - **模型那一格也没读到**（`config === null`）：只要它还读得到，这一行就有真事实可说，
 *   不该被一句"读不到"盖掉（那会把真读数藏起来）；
 * - **状态那一面是 `unreachable`**：`claimed` / `unknown-id` / `elsewhere` 三档都
 *   至少有一份读数在场，它们各有各的话（见 `agentRowStatusFromHomes` 那张表）。
 *
 * ⚠ **"正在读"不收**：`resolution === null` 是**还没裁决**，不是"读不到" ——
 * 那时候收成一句"读不到"就是拿一段延迟说成一个事实（而且首屏会闪一下再回来）。
 *
 * ⚠ 出口是「我的执行端」（`/app/runtimes`）：那一页才有"那台机器怎么了"的读数与
 * 改地址 / 换钥匙的入口 —— 本行不指别的门（同 `agentMachine.ts` 那条"不指假路径"）。
 */
export function rowSameCauseLine(input: {
  resolution: AgentHomeResolution<HomeStatusView> | null
  config: HomeConfigView | null
}): { text: string; exit: 'machines' } | null {
  const { resolution, config } = input
  if (config !== null) return null
  if (resolution === null || resolution.kind !== 'unreachable') return null
  const why = resolution.failure ? `（${resolution.failure}）` : ''
  return { text: `读不到它现在怎么样${why}。`, exit: 'machines' }
}

/** 在线那一格：三态**不许压成两态**（见文件头第 1 条）。 */
function onlineCell(
  view: HomeStatusView | null,
  agentId: string | null | undefined,
): { label: string; tone: AgentRowTone } {
  if (view === null) return { label: machineMatchLabel('unknown'), tone: 'unknown' }
  const match = agentAgentMatch(view, agentId)
  if (match === 'same') return { label: machineMatchLabel('same'), tone: 'local' }
  if (match === 'different') return { label: machineMatchLabel('different'), tone: 'away' }
  return { label: machineMatchLabel('unknown'), tone: 'unknown' }
}

/**
 * 模型那一格：读的是**绑定**那一层（"这一个 Agent 用哪一条"），不是清单有几条。
 *
 * ★ **2026-10-12：按这一支读**。从前它读的是**整机**那一格（`bindingState` / `bound`），
 * 而那一格在门那边是"**第一支**绑的那条" ⇒ 一个家住两支时，**两行会显示同一条**（第一支的），
 * 第二行那句话是假的。现在走 `residentModelReadout`（逐支那一格），四态各有各的话：
 *
 * - 这一支绑了一条、那一条还在清单里 ⇒「模型：<名字>」（名字空的话拿模型名顶，与门侧同口径）；
 * - 这一支**没指定** ⇒「模型：未指定」（**这是一个事实**，不是读不到）；
 * - 这一支的绑定**读不出来** / 绑的那条**不在清单里**（悬空）⇒「模型：未知」
 *   ——**不许说成"未指定"**（那会把"读不出来"说成"没配"，让人去重配一条本来在的）；
 * - **这一支不在这份读数里** ⇒「模型：未知」（不许拿整机那格顶：那是别人的）；
 * - **老门没给逐支那一格** ⇒ 退回整机口径（与从前**一字不变**，见 `machineWideModelLabel`）。
 */
function modelCell(config: HomeConfigView | null, agentId: string | null | undefined): string {
  if (config === null) return `${MODEL_PREFIX}未知`
  const mine = residentModelReadout(config, agentId)
  if (mine.kind === 'unlisted') return machineWideModelLabel(config)
  if (mine.kind === 'bound') return `${MODEL_PREFIX}${mine.entry.name || mine.entry.model}`
  if (mine.kind === 'absent') return `${MODEL_PREFIX}未指定`
  return `${MODEL_PREFIX}未知`
}

/**
 * **整机那一格**的老口径（只在门没给逐支表时用）—— 一字不变地留在这里：
 * 单支的家它与逐支读法等价；多支的家它答的是"第一支"，**所以不许拿它当这一支的答案**。
 */
function machineWideModelLabel(config: HomeConfigView): string {
  if (config.bindingState === 'absent') return `${MODEL_PREFIX}未指定`
  if (config.bindingState !== 'bound') return `${MODEL_PREFIX}未知`
  const entry = config.entries.find((candidate) => candidate.id === config.bound)
  if (!entry) return `${MODEL_PREFIX}未知`
  return `${MODEL_PREFIX}${entry.name || entry.model}`
}

/**
 * 最近活动那一格：`lastWake` 那一行原文里**只有**活动标识能解出时间。
 *
 * 三档如实分开（都是缺省，但不是同一件事）—— 判据在 `lastWakeReadout`（**唯一一份**转换）：
 * - 这一份读数本身没读到 ⇒「未知」；
 * - 读数在、但这一家还一次都没醒过（`lastWake` 为 null）⇒「—」（没有这个事实）；
 * - 有那一行、但标识解不出时间 ⇒「—（这次没有读到时间）」。
 */
function activityCell(view: HomeStatusView | null): string {
  const readout = lastWakeReadout(view)
  if (readout.kind === 'unread') return `${ACTIVITY_PREFIX}未知`
  if (readout.kind === 'never') return `${ACTIVITY_PREFIX}—`
  if (readout.kind === 'no-time') return `${ACTIVITY_PREFIX}—（这次没有读到时间）`
  return `${ACTIVITY_PREFIX}${formatRuntimeTime(new Date(readout.ms).toISOString())}（推算）`
}

// ---------------------------------------------------------------------------
// `lastWake` 那一行原文 ⇒ 四态（"最近活动"那一格的唯一转换）
// ---------------------------------------------------------------------------

/**
 * `lastWake` 那一行原文 ⇒ **四态读数**（三档缺省如实分开）：
 *
 * - `unread`：这一份读数**这次没读到** ⇒ 未知；
 * - `never`：读数在、但这一家**一次都没醒过**（`lastWake` 为 null）⇒ 没有这个事实；
 * - `no-time`：有那一行、但活动标识**解不出时间** ⇒ 这次没读到时间；
 * - `at`：解出了 Agent 上次醒来的时刻（毫秒）。
 *
 * 为什么它是一个**导出**的纯函数：判据要能被 `node --test` 直测（`tests/agent-row-status.test.ts`
 * 第六节整节直接调它），而**绝不许写第二份**（那是复制执行链）⇒ 生产侧只有一处调用。
 *
 * ⚠ **它今天唯一的生产消费者是 `activityCell`**（"最近活动"那一格）。本节原写「聊天页头部那条
 * Agent 家读数（"Agent 还在不在"刀）」—— **那个消费者已经不存在了**：2026-09-26 主人裁「IM 侧
 * 连 presence 投影也不放」，聊天面从此只剩静态参与者统计 ＋ 进行态（`ChatHeader.tsx` 逐行可核，
 * 一个宿主读数都没有）＋ 已读那一格（2026-10-20 起挂在**我最后一条气泡下面**，不在头部：
 * `MessageBubble` 的 `peerRead`）。**留着那句陈旧注释，下一个人就会照着往 IM 面再塞一条宿主读数**
 * —— 而那正是 `routeRegistry.ts` 文件头那条红线（「IM 侧不能有宿主读数」）。
 */
export type LastWakeReadout =
  | { kind: 'unread' }
  | { kind: 'never' }
  | { kind: 'no-time' }
  | { kind: 'at'; ms: number }

export function lastWakeReadout(view: HomeStatusView | null): LastWakeReadout {
  if (view === null) return { kind: 'unread' }
  if (!view.lastWake) return { kind: 'never' }
  const ms = wakeLineAtMs(view.lastWake)
  if (ms === null) return { kind: 'no-time' }
  return { kind: 'at', ms }
}
