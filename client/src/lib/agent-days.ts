/**
 * **活动记录**的纯逻辑（刀2a；2026-09-25 组织轴刀随房间改名）：把状态口那份机械读数
 * 按**一次清醒（Run）**分组成时光线。
 *
 * 为什么要有这一层：原来的「Agent 的经历」页给的是一列**裸 trace 行**（`seq` + `run-start`
 * 那种），那是审计日志，不是"它做过什么"。用户问「这些活动是谁的」时，那一列自己答不出来。
 * 这里把同一份读数重排成"这台机器上的 Agent 醒过几次、每次做了什么、怎么收场"。
 *
 * ⚠ 它服务的是**执行端**那一侧（`/app/runtimes/:entryId#activity`）：一次活动是**机器上**发生的事。
 *
 * 四条纪律：
 * - **数据只有一份来源**：`/home-status/status` 的 `experience.rows` + `agent.runs`。
 *   本刀**不改它的形状**、不新造字段、不为凑时光线写新 API；
 * - **缺什么就缺省**（`null`），绝不拿别的字段顶替。最要紧的一例：口子给的
 *   `notification=` **不是**"为什么醒"（它表示"这一轮通知快照读没读到"），
 *   所以时光线上**根本没有"起因"这一列** —— 而不是拿它冒充起因；
 * - **不收正文**：`rows` 在口子那一层已按白名单投影（客户端还有第二道行键白名单），
 *   这里只用机械字段（`run` / `kind` / `action` / `turn` / `subject` / `state` / `failure`）；
 * - **一件事只有一个来源**：每一场的"怎么收场"读**它自己那一行的 `kind === 'ending'`**，
 *   而不是拿 `agent.runs` 去对 —— 实测两者窗口可以**完全不重叠**（本机：8 条 `runs` 与
 *   窗口里的 40 行一条都不重合）。`agent.runs` 只在那一场没有 ending 行时**兜底**。
 * - **两条路各说各的**（2026-10-05）：明细在（`experience.rows`）走 `loaded`；明细**没给**
 *   （app 形态：壳不端那套投影）而收场那一层在（`agent.runs`）走 `runs-only` —— 后者
 *   **一格明细都不编**，页面按 `detail` 选句子（"没有动手的记录" ≠ "这台机器没给动手明细"）。
 */

import type { ExperienceRow, HomeStatusRun, HomeStatusView } from '@/api/homeStatus'

/** 时光线上的一行 = **一次清醒**（Run）。 */
export interface AgentDay {
  /** 活动标识（`host-<进程号>-<毫秒>-<序号>`，宿主机械生成）。 */
  run: string
  /**
   * 醒来的时刻（毫秒）。
   *
   * ⚠ **从活动标识里解出来的**，不是读数里的一格：那个格式是宿主的文档化产物
   * （`agent_host` 的用法说明写着它怎么生成），所以可解；解不出来就是 `null`
   * —— 如实缺省，**不拿别的时刻顶**。页面必须把这一点标出来。
   */
  wakeAtMs: number | null
  /** Agent**选择去做**的动作（按发生顺序去重）。 */
  actions: string[]
  /** 这一场走了几轮（最大 `turn`）；没有带 `turn` 的行就是 `null`。 */
  turns: number | null
  /** 怎么收场（`kind === 'ending'` 那一行的 `subject`）。 */
  ending: string | null
  /**
   * **为什么开始**（刀2b C②）：口子从 `run-start` 注入行的 `activity-origin` 块映射出来的
   * 稳定取值（`bell` / `timer` / `manual` / `unknown`）。**没有那一行就是 `null`**
   * —— 如实缺省，不拿别的字段顶（口子给不出时它就是给不出）。
   */
  origin: string | null
  /** 收场带出来的失败分类（有就是有，没有就是 `null`）。 */
  failure: string | null
  /** 封存行在不在（`kind === 'sealed'` ⇒ 这一场真的收场落盘了）。 */
  sealed: boolean
  /**
   * 这一场的**明细来源**（2026-10-05）：
   * - `rows`      = 这一场有自己的裸行（`experience.rows`）—— 页面那句"没有动手的记录"
   *                 是**看过的结论**；
   * - `runs-only` = 这一面只给到收场那一层（`agent.runs`）—— 明细**没给**，不是"没有"。
   *
   * ⚠ 两条路**不许互相冒充**：页面按它选句子（"没有"与"没给"是两件事）。
   */
  detail: 'rows' | 'runs-only'
  /**
   * 这一场的**判定类目**（五类互斥：`ended` / `unfinished` / `blocked` / `unknown` / `refused`）
   * —— 取自 `agent.runs[].state`，**两条路都取**（明细那条路要那一场恰好也在 runs 窗口里，
   * 两个窗口可以不重叠 ⇒ 取不到就是 `null`，不拿别的顶）。
   * ⚠ 它与 `sealed` **不是一个来源**：`sealed` 是"封存行在不在"（明细那条路的收场判据），
   * 判定类目是运行时对"这一个活动现在什么样"的结论 —— 两个都留着，页面按 `detail` 选一条说。
   */
  runState: string | null
  /** 这一场的裸行（机械字段；"展开详情"用）。⚠ `runs-only` 那条路一律是空数组。 */
  rows: ExperienceRow[]
}

export interface AgentDaysView {
  /**
   * - `loaded`    = 明细在（`experience.rows`）；
   * - `runs-only` = **只有收场那一层**（`agent.runs`）—— 这台机器没给动手明细（app 形态就是这样）；
   * - `missing`   = 这一面这次读不到（**不是**"Agent 没有日子"）。
   */
  state: 'loaded' | 'runs-only' | 'missing'
  /** 最近的一次清醒在前。 */
  days: AgentDay[]
  /** 窗口内有更多（口子只给了最近这些）。 */
  truncated: boolean
  /** 窗口取自文件尾，更早的没读。⚠ `runs-only` 那条路上它没有意义（一律 `false`）。 */
  windowed: boolean
}

/** 活动标识的形状（宿主文档化的机械格式）。 */
const RUN_ID = /^host-(\d+)-(\d+)-(\d+)$/

/** 从活动标识里解出醒来时刻（毫秒）；格式不认识 ⇒ `null`。 */
export function wakeAtFromRunId(run: string): number | null {
  const match = RUN_ID.exec(run)
  if (!match) return null
  const ms = Number.parseInt(match[2] as string, 10)
  return Number.isSafeInteger(ms) && ms > 0 ? ms : null
}

/**
 * 从状态口那一行 `wake …` 的**原文**里解出醒来时刻（毫秒）；解不出 ⇒ `null`。
 *
 * ⚠ 存在的理由：`agent.lastWake` **不是时间串**，它是宿主那一行的原文
 * （`wake run=host-<pid>-<毫秒>-<序号> drives=… ending=… notification=…`）——
 * 直接交给 `formatRuntimeTime` 只会得到 `—`（它解不出日期）。时间只能从活动标识里还原。
 * 「运行」房间与「我的 Agent」列表都要这一格，所以"取 run → 还原时间"收在这里一处。
 */
export function wakeLineAtMs(lastWake: string | null): number | null {
  if (!lastWake) return null
  const run = /run=(\S+)/.exec(lastWake)?.[1] ?? ''
  return run ? wakeAtFromRunId(run) : null
}

/** 这一场最后一次出现的顺序号（排序用；窗口来自文件尾 ⇒ 序号大的新）。 */
function lastSeq(day: AgentDay): number {
  for (let index = day.rows.length - 1; index >= 0; index -= 1) {
    const seq = day.rows[index]?.['seq']
    if (typeof seq === 'number') return seq
  }
  return 0
}

/** 把一份读数重排成时光线。 */
export function agentDays(view: HomeStatusView): AgentDaysView {
  const experience = view.experience
  if (!experience || experience.state !== 'loaded') {
    // **明细这一面没给**（app 形态就是这样：壳不端那套投影）—— 但**收场那一层口子一直给**
    // （`agent.runs`）⇒ 从它降级投影，别把"本来能说的"也说成"读不到"。
    //
    // ⚠ **这不是造第二个权威**：`runs` 是口子已经给了的读数，本层只是**换一种排法**；
    //   明细那一半（做了哪几件事 / 走了几轮 / 为什么醒 / 封存没有）**一格都不编** ——
    //   那些只有 `experience.rows` 说得出来，说不出来就说"没给"（`detail: 'runs-only'`）。
    const runs = view.agent.runs
    if (!Array.isArray(runs) || runs.length === 0) {
      return { state: 'missing', days: [], truncated: false, windowed: false }
    }
    // 口子给的次序是**旧的在前**（账本原序）⇒ 倒过来，最近的一场在前。
    const days = runs.map(runsOnlyDay).reverse()
    // 每一场都解得出时刻 ⇒ 按时刻排（同一份读数内部的排序，不引入新事实）；
    // 有解不出的就**不排** —— 那会拿缺省当零，把"没读到时刻"的那些挤到最后。
    if (days.every((day) => day.wakeAtMs !== null)) {
      days.sort((left, right) => (right.wakeAtMs as number) - (left.wakeAtMs as number))
    }
    return {
      state: 'runs-only',
      days,
      // 口子自己说的"还有更多"（`agent.truncated`）——原样带出来，不自己算。
      truncated: view.agent.truncated === true,
      windowed: false,
    }
  }
  const order: string[] = []
  const buckets = new Map<string, ExperienceRow[]>()
  for (const row of experience.rows ?? []) {
    const run = row['run']
    // 没有活动标识的行归不到哪一场 ⇒ 丢开，**不编一个"未知场"**（那会把缺省说成事实）。
    if (typeof run !== 'string' || run === '') continue
    const bucket = buckets.get(run)
    if (bucket) bucket.push(row)
    else {
      buckets.set(run, [row])
      order.push(run)
    }
  }
  const days = order.map((run) => summarize(run, buckets.get(run) as ExperienceRow[], view.agent.runs))
  days.sort((left, right) => lastSeq(right) - lastSeq(left))
  return {
    state: 'loaded',
    days,
    truncated: experience.truncated === true,
    windowed: experience.windowed === true,
  }
}

/** 把一场的行收成一件事：做了几件、走了几轮、怎么收场。 */
function summarize(run: string, rows: ExperienceRow[], runs?: HomeStatusRun[]): AgentDay {
  const actions: string[] = []
  let turns: number | null = null
  let ending: string | null = null
  let failure: string | null = null
  let origin: string | null = null
  let sealed = false
  for (const row of rows) {
    // 只数 Agent**选择去做**的动作（`action-selected`），不是"看到的结果" —— 一次行动算一件。
    if (row['kind'] === 'action-selected' && typeof row['action'] === 'string') {
      if (!actions.includes(row['action'])) actions.push(row['action'])
    }
    if (typeof row['turn'] === 'number') turns = Math.max(turns ?? 0, row['turn'])
    if (row['kind'] === 'ending') {
      ending = typeof row['subject'] === 'string' ? row['subject'] : null
      failure = typeof row['failure'] === 'string' ? row['failure'] : null
    }
    // 起源只在 `run-start` 那一行上（口子已把它映射成稳定取值）。
    if (row['kind'] === 'injection' && row['stage'] === 'run-start' && typeof row['origin'] === 'string') {
      origin = row['origin']
    }
    if (row['kind'] === 'sealed') sealed = true
  }
  // 兜底与判定**共用同一份 `agent.runs`**（两个窗口可以不重叠 ⇒ 找不到就是缺省，不拿别的顶）：
  // - `ending` 那一格：这一场**没有** ending 行时才去看它；
  // - `runState` 那一格：判定类目（五类之一）**只有 `agent.runs` 说得出来** —— 明细那条路
  //   也照取（2026-10-05：页面要靠它把"停在半路 / 被挡住 / 结果不明 / 判定不出来"分开说）。
  const listed = runs?.find((each) => each.id === run)
  if (ending === null && listed) {
    ending = listed.outcome
    failure = listed.failure ?? null
  }
  return {
    run,
    wakeAtMs: wakeAtFromRunId(run),
    actions,
    turns,
    ending,
    failure,
    origin,
    sealed,
    detail: 'rows',
    runState: listed?.state ?? null,
    rows,
  }
}

/**
 * **只有收场那一层**的一场（`agent.runs` 里的一条）—— app 形态那条路（2026-10-05）。
 *
 * 一格都不编：`actions` / `turns` / `origin` / `sealed` 在那一层根本不存在 ⇒ 全部缺省，
 * 页面按 `detail === 'runs-only'` 选句子（"没给" ≠ "没有"）。**唯一多说的一句**是
 * `runState`（那一场自己的 `ended` / `unfinished`）—— 它是那一层**本来就有**的读数。
 */
function runsOnlyDay(run: HomeStatusRun): AgentDay {
  return {
    run: run.id,
    wakeAtMs: wakeAtFromRunId(run.id),
    actions: [],
    turns: null,
    ending: run.outcome,
    failure: run.failure ?? null,
    origin: null,
    // ⚠ `sealed` 在这条路上**没有意义**（封存行只有明细那一层说得出来）—— 页面看 `runState`。
    sealed: false,
    detail: 'runs-only',
    runState: run.state,
    rows: [],
  }
}
