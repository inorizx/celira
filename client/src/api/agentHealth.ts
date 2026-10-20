/**
 * 「这一支 Agent 现在正常吗」——**机器侧那一句人话**（2026-10-06；2026-10-20 修主语）。
 *
 * ## 它治的是什么
 *
 * 「我的 Agent」那一行原本三格（在线 ／ 模型 ／ 最近活动）。三格都是**读数**，
 * 但拼起来回答不了那个真问题：**现在正常吗？** 人盯着「最近活动：12:03」只能猜：
 * 这是刚醒过，还是三个小时没动静了？机器上那个进程还活着吗？上一次是正常收场，还是出事了？
 *
 * 这一层把机器侧那几格合成**一句**（机器在不在跑 ＋ 账本该不该出手）：
 *
 * | 这一句 | 意思 |
 * |---|---|
 * | `它正在跑 · 上次：完成` | 正常 |
 * | `它正在跑 · 上次：未回复` | 正常 —— **主体选择不回**（沉默是一等终态，不是故障） |
 * | `它正在跑 · 上次：没正常结束` | 出过事；**为什么**在「运行」那一页的「活动」房间 |
 * | `它现在没在跑（…上没有它的进程） · …` | 那台机器上没有它的进程 |
 * | `机器这一面读不到` | 这一格没有读数（**不是**"没在跑"） |
 *
 * ⚠ **2026-10-20 修那两个主语的错**（真机撞到的一行：`机器没在跑 · 上次：完成`
 * 紧挨着绿的「在本机运行」—— 一句说"没在跑"、一句说"在跑"，两个相反的答案并排）：
 *
 * - **这一句的主语是它**，不是宿主进程：说的是"**它**现在有没有在跑"，宿主进程只是
 *   这件事的来路（`hostStateOf` 那四态没变，短标签 `hostStateLabel` 也没变）；
 * - **"在跑"这个词只归这一句**：住址那一格（`machineMatchLabel`）只说"在哪台机器上"，
 *   不再借"运行"两个字 —— 一个词只答一件事，两格才不会互相打脸。
 *
 * ## 三条纪律
 *
 * 1. **两个平面不许互相冒充**：这里的「它正在跑」是**执行端**的事实（宿主进程在不在），
 *    与 IM 里那个「在线」（连接派生的易失投影）**不是一件事** —— 页面把它们分开摆，
 *    这一层只讲前者；而家侧读数**不进世界**（它不上 im 服务端，也不进任何公共事实）；
 * 2. **粗粒度**：系统收场的**分类**（`credential-rejected` / `transport-failure` …）不上这一句
 *    —— 那是机器页「活动」房间的活（那里已经在显示）。这一层只说"正常 ／ 没正常收场 ／ 读不到"，
 *    同一份事实不许有两个说法；
 * 3. **缺省不等于零**：`view === null`（这一份读数没读到）与 `host` 缺那一格，
 *    都说"读不到"，**绝不说"没在跑"**（那是把缺省说成一个事实）。
 *
 * 纯函数、零副作用、无 React、无 store ⇒ 能 `node --test` 直测（`tests/agent-health.test.ts`）。
 *
 * ⚠️ **这一层产出的每一句都会被 `scripts/jargon.mjs` 那张内行词表扫**（`verify-ui` 读的是
 * **渲染后的正文**）：所以这里不许出现「收场」「读数」「准入」这类词，连复用的
 * `plain-language.ts` 也要当心（`runStateLabel('ended')` 给的是「已收场」⇒ 那一支本层绕开）。
 * `tests/agent-health.test.ts` 拿**同一份词表**离线扫一遍全矩阵 —— 规则写在词表里，
 * 判据就得有人跑。
 */

import { hostStateOf, type HomeStatusHost, type HomeStatusView } from './homeStatus.ts'
import { type AgentHomeResolution } from './homes.ts'
import { resultLabel, runStateLabel, stateLabel } from '../lib/plain-language.ts'

/** 这一句的色标（**只管语义，不管颜色**：Tailwind 类在组件那一层映）。 */
export type AgentHealthTone = 'ok' | 'warn' | 'bad' | 'muted'

export interface AgentHealth {
  tone: AgentHealthTone
  text: string
}

/**
 * 算那一句。
 *
 * `view` 为 `null` = **这一份读数这次没读到**（家条目读失败 ／ 加载中 ／ 认领不到）——
 * 与"读到了、但那一格没有"是两件事，两者在这一层各有各的说法。
 *
 * `machine` 只是**那句话里点谁的名**（缺省「这台机器」）：认领到远程那一台时，
 * "这台机器上没有它的进程"是句假话 —— 那里的事实是"**那台**上没有它的进程"。
 * ⚠ 它**不改判据**，只改主语的名字（长短可以两样，事实只能一处）。
 */
export function agentHealthReadout(
  view: HomeStatusView | null,
  opts: { machine?: string } = {},
): AgentHealth {
  if (view === null) return { tone: 'muted', text: '机器这一面读不到' }
  const cached = view.stale ? '（缓存）' : ''
  const where = opts.machine ?? '这台机器'
  return {
    tone: toneOf(view),
    text: `${cached}${machinePart(view, where)} · ${ledgerPart(view)}`,
  }
}

/**
 * 按**家条目认领**算这一句（与三格同一份裁决：`agentRowStatusFromHomes`）。
 *
 * 认领到别人那台（`elsewhere`）时**不许拿那台的读数顶** —— 那会说出"另一支 Agent 很健康"
 * 而这一行是这一支。这一格因此只说"未接入"。
 */
export function agentHealthFromHomes(input: {
  /** 认领结果；`null` = 家条目还没就绪。 */
  resolution: AgentHomeResolution<HomeStatusView> | null
}): AgentHealth {
  const { resolution } = input
  if (resolution === null) return { tone: 'muted', text: '正在读…' }
  if (resolution.kind === 'claimed' && !resolution.ownReadings) {
    // ★ **认领靠住户清单**（第 2 支起，2026-10-08）：住址对（这台机器确实跑着它），
    // 但顶层那几格是**第一支**的 ⇒ 这一句**不许**拿它算（那会说出"另一支的账本很健康"
    // 而这一行是这一支）。如实说这一支的读数这里读不到。
    // ⚠ 2026-10-20：这一句从前写「在本机 · 这一支的运行读数不在这一份里」—— 住址与
    //    "没有它的账本"混在一格，而「读数」是内行词（`scripts/jargon.mjs` 会红）。
    //    住址那一格另有其格（`machineMatchLabel`），这一句只说"它的记录不在这儿"。
    return { tone: 'muted', text: '这一支的记录不在这一份里（这一份只有第一支的）' }
  }
  if (resolution.kind === 'claimed' || resolution.kind === 'unknown-id') {
    // `unknown-id`（读数没报它是谁）照旧显示那台机器的读数 —— 与三格同一条口径。
    return agentHealthReadout(resolution.view, { machine: machineOf(resolution.entry) })
  }
  if (resolution.kind === 'elsewhere') {
    // ⚠ **带作用域**（2026-10-11）：`elsewhere` 说的是"**读得到的**执行端都没有报它在跑"，
    // 不是"**这台机器**上跑的是别的 Agent" —— 后者把"我没看见"说成了"没有"
    // （跨平面的话必须带作用域；说法与 `agentMachine.ts` 那一句一致）。
    return { tone: 'muted', text: '未接入 —— 能读到的执行端都没有报它在跑' }
  }
  // `unreachable`：一份都没读到。
  return agentHealthReadout(null)
}

/**
 * **Agent 主页「机器」那一段用的那一句**：只有裁决过"这台机器跑的就是它"（`claimed`）才给，
 * 其余三档一律 `null`（页面据此决定渲不渲染）。
 *
 * 为什么这条判据住在这里而不是页面里：它是**归属**判据——"那台的账本读数算不算这一支的"。
 * 页面那一层没有用例（React 组件不进 `node --test`），放在这里才钉得住：
 *
 * - `unknown-id`：那台**没报上上面跑的是谁** ⇒ 它的账本可能是**别人**的；
 * - `elsewhere`：那台跑的是**别的** Agent ⇒ 同理；
 * - `unreachable` / `null`：没有可归属的读数。
 *
 * ★ **2026-10-08 加一条**：`claimed` 但 `ownReadings === false`（认领靠住户清单 ⇒ 第 2 支起）
 * 也一律 `null` —— 顶层那几格是**第一支**的账本，摆在它名下同样是张冠李戴。
 *
 * ⇒ 把别台的读数摆在它名下就是**张冠李戴**，而它比留空更坏：留空用户会去问，错值没人核对。
 */
export function agentHealthForClaimedMachine(input: {
  resolution: AgentHomeResolution<HomeStatusView> | null
}): AgentHealth | null {
  const { resolution } = input
  if (resolution === null || resolution.kind !== 'claimed') return null
  if (!resolution.ownReadings) return null
  // ⚠ 2026-10-20：这一句的**正文**与列表那一行逐字相同 ⇒ 那一句里点了哪台机器的名，
  // 这一页也得点同一台（否则同一句话在两个页面上说两个机器）。
  return agentHealthReadout(resolution.view, { machine: machineOf(resolution.entry) })
}

/**
 * 宿主那一格的**短标签**（机器页住户表用：一支一行"在跑 / 没在跑"）。
 *
 * ⚠ 与 [`machinePart`] 那句长话**同一个来源**（`hostStateOf` 的四态）：这里只是短说法，
 * 不是第二个答案 —— 判据一句话：**长短可以两样，事实只能一处**。
 */
export function hostStateLabel(host: HomeStatusHost | undefined | null): string {
  switch (hostStateOf(host)) {
    case 'running':
      return '在跑'
    case 'running-orphan':
      return '在跑（不是这次开机起的）'
    case 'stopped':
      return '没在跑'
    case 'unknown':
      return '读不到'
  }
}

/**
 * 那个条目在句子里的写法（本机那条不写它的名字 —— 它叫「本机」，拼进句子会变成
 * 「本机这台机器…」；与 `agentMachine.ts` 的 `entryLabel` 同一口径）。
 */
function machineOf(entry: { local: boolean; name: string }): string {
  return entry.local ? '这台机器' : entry.name
}

/**
 * 机器那一半：**它现在有没有在跑**（`host` 缺那一格 ⇒ 读不到，不说"没在跑"）。
 *
 * ⚠ **2026-10-20 修主语**：从前这三句是「机器在跑 / 机器没在跑 / 机器在不在读不到」——
 * 主语是**宿主进程**，而这一行问的是**它**。同一屏上「机器没在跑」与「在本机运行」
 * （住址那一格）并排时，读起来就是两个相反的答案（真机撞到的那一行）。
 * ⇒ 主语换成"它"，"在不在跑"这件事只说一次；住址那一格不再借"运行"两个字。
 *
 * `where` 是按认领到的那台机器点名（远程那台不许说"这台机器"）。
 */
function machinePart(view: HomeStatusView, where: string): string {
  const state = hostStateOf(view.host)
  // 短标签（`hostStateLabel`）与这三句**同一个来源**：长短可以两样，事实只能一处。
  if (state === 'running') return '它正在跑'
  if (state === 'running-orphan') return '它正在跑（不是这次开机起的）'
  if (state === 'stopped') return `它现在没在跑（${where}上没有它的进程）`
  return '它现在跑没跑读不到'
}

/** 账本那一半：**该不该出手**优先于"上次结局"（准入那一格说的是"现在能不能开始新活动"）。 */
function ledgerPart(view: HomeStatusView): string {
  const admission = stateLabel(view.agent.admission)
  if (admission === '已暂停') return '已暂停，需要你确认'
  if (admission === '上一次没做完') return '上一次没做完'
  const run = view.agent.runs?.[0]
  if (!run) return emptyLedgerText(view.agent.state, view.legacyHome === true)
  if (run.outcome === null) {
    // 判定类目（还没结束 ／ 被挡住 ／ 不明——可能已经做了 ／ 记录读不出来）。
    // ⚠ `ended` 而没有结果这一格是**读数自相矛盾**（口子说收过场、却没给结局）：
    // 那时说"读不到结果"，**不**复用 `runStateLabel('ended')`——那个词是「已收场」，
    // 而"收场"是内行词（`scripts/jargon.mjs`），不许上屏。
    if (run.state === 'ended') return '上次：读不到结果'
    return `上次：${runStateLabel(run.state) ?? '读不到结果'}`
  }
  if (run.outcome.startsWith('subject:')) {
    return `上次：${resultLabel(run.outcome) ?? '读不到结果'}`
  }
  // 系统收的场：这一层只说"没正常结束"，**分类不上这一句**（它在「运行」页的「活动」房间）。
  return '上次：没正常结束'
}

/**
 * 记录读得到、但一场活动都没有（或这一面根本没读出来）时的说法。
 *
 * ⚠ **老形态的家不许被说成"还没有 Agent"**（2026-10-11）：老家（`<家>/env` 那一档）里的那一支
 * **明明在**，只是住在旧布局里、壳认不出来 ⇒ 说"还没有 Agent"就是一句**假话**
 * （`homeStatus.ts` 那条注释点名过这句话不许说；机器页「运行」那一间从 2026-10-05 起就判了
 * `legacyHome`，**这里漏了** —— 而它正是「我的 Agent」列表每一行显示的那句）。
 */
function emptyLedgerText(state: string, legacyHome: boolean): string {
  if (state === 'loaded') return '还没有过活动'
  if (state === 'no-resident') return legacyHome ? '这个家还是老形态' : '这台机器上还没有 Agent'
  if (state === 'missing-binary') return '这台机器上找不到它的程序'
  if (state === 'busy') return '它的记录这一面正忙'
  return '它的记录读不到'
}

/** 色标：**先看该不该出手，再看机器在不在，最后看上一场**。 */
function toneOf(view: HomeStatusView): AgentHealthTone {
  const admission = view.agent.admission ?? ''
  // 准入那一格说"现在能不能开始新活动"：它说该出手，色标就必须跟着它 ——
  // 否则会出现"文字说上一次没做完、颜色说一切正常"这种自相矛盾的一行。
  if (admission === 'paused' || admission.startsWith('paused(')) return 'bad'
  if (admission === 'unfinished' || admission.startsWith('unfinished(')) return 'warn'
  const machine = hostStateOf(view.host)
  if (machine === 'stopped') return 'warn'
  if (machine === 'unknown') return 'muted'
  const run = view.agent.runs?.[0]
  if (!run) return 'ok'
  if (run.outcome !== null) {
    // 主体自己收的场（完成 ／ 未回复 ／ 拒绝 ／ 等外部结果）都是正常；系统收的场是"出过事"。
    return run.outcome.startsWith('subject:') ? 'ok' : 'warn'
  }
  if (run.state === 'ended') return 'ok'
  if (
    run.state === 'unfinished' ||
    run.state === 'blocked' ||
    run.state === 'unknown' ||
    run.state === 'refused'
  ) {
    return 'warn'
  }
  // 判定类目认不出：文案把它原样带出去，**色标不编**。
  return 'ok'
}
