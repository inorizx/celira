/**
 * **第一次的那三步**：模型源 → 建一支 → 接上一台机器（2026-10-20 T1 主刀）。
 *
 * ## 它治的是什么
 *
 * 「建得出、活不了」：创建在 `/app/agents`，模型源在那台机器的「模型」那一间，
 * 「接上」一半在壳里（`anjia_start`）、另一半在 Agent 主页的「机器」那一段。
 * 三条路**散在三页、顺序没人说** ⇒ 新账号 + 一台空机器的人建得出 Agent，
 * 却说不清它为什么永远不回第二句话（`AgentsListPanel` 的 `CREATE_WONT_REPLY` 只是
 * 一句**事后**的事实，不是一条**可以走**的路）。
 *
 * 这一层把三步各自的**处境**从既有读数里派生出来，页面只负责摆清单 ＋ 渲染出口：
 *
 * | 这一步 | 判据（唯一权威） |
 * |---|---|
 * | 模型源 | `homeConfig.ts` 的 `modelSourceAbsent`（**读不到不摆**，见下） |
 * | 建一支 | 页面手里那份所有权清单（`ownedAgents`）—— 世界侧事实 |
 * | 接上一台机器 | `homes.ts` 的 `resolveAgentHome`（归属）＋ `anjia.ts` 的 `anjiaReadout`（壳那条读数） |
 *
 * ⇒ **零新请求、零新持久面、零新端点**：三个来源都是这一页已经在读的那几份。
 *
 * ## 四条纪律
 *
 * 1. **读不到不摆**：模型那一格读不到（`config === null`）时整块返回 `null` ——
 *    摆一份三步清单而不知道第一步是什么，比不摆更坏。同一把尺子在 2026-10-11 那张
 *    「先给这台机器配一条模型源」卡上用的是 `modelSourceAbsent`，同样的"读不到不摆"。
 * 2. **一个事实一个权威**：这里**不重算**任何裁决 —— 归属照旧 `resolveAgentHome`，
 *    "有没有模型源"照旧 `modelSourceAbsent`，"接上没有"照旧 `anjiaReadout`。本层只把它们
 *    排成三步、各配一句话与一个出口（与 `agentMachine.ts` / `agentRowStatus.ts` 同一条纪律）。
 * 3. **每步一个出口，且出口必须能走**：出口是一个**判别式联合**，页面照它渲染，页面
 *    不自己判"该给哪个出口"。出口的四个目的地都是既有页面：模型源 → 那台机器的「模型」
 *    那一间；建一支 → 这块面板自己；接上 → 它的 `#machine` 那一段（**那一段本来就能走远程家**，
 *    所以浏览器里也有真路）；一台机器都没有 → 「我的执行端」（那里才有加一台的入口）。
 * 4. **不复制写入口**：本层只给**目的地**，不给动作 —— "就地接上"仍然只有那一处
 *    （壳那六步 / `#machine` 那一段的按钮）。否则同一件事就有两个写入口。
 *
 * 纯函数、零副作用、无 React ⇒ `node --test` 直测（`tests/first-run.test.ts`）。
 */

import { modelSourceAbsent, type HomeConfigView } from './homeConfig.ts'
import type { AgentHomeResolution } from './homes.ts'
import type { HomeStatusView } from './homeStatus.ts'
import { anjiaReadout, type AnjiaResult } from './anjia.ts'

/** 三步的标识（顺序即屏上的顺序）。 */
export type FirstRunStepKey = 'model' | 'create' | 'attach'

/** 三步在屏上的名字（**只在这里写一次**；页面不再各写一套）。 */
export const FIRST_RUN_TITLES: Record<FirstRunStepKey, string> = {
  model: '模型源',
  create: '建一支',
  attach: '接上一台机器',
}

/**
 * 这一步此刻的处境。
 *
 * - `done`：已经成立（**收成一行**：一句话、不再给出口）；
 * - `todo`：还没成立 ⇒ 必须带一个能走的出口；
 * - `unknown`：**读不到** —— 不说成 todo（那会把"读不到"说成一个事实，本仓那条老判据）。
 */
export type FirstRunStepState = 'done' | 'todo' | 'unknown'

/**
 * 一个出口：`label` 与目的地一起给（页面只渲染，不拼词）。
 *
 * ⚠ 四个目的地都是**既有页面**，一个都不新开：
 * - `model-source`：`/app/runtimes/<本机>#model`（那台机器的「模型」那一间）；
 * - `create`：这块面板自己（空态那一颗按钮 / 面板里的表单）；
 * - `open-agent`：`/app/agents/<id>#machine`（"它可以去哪台机器上跑"那一段）；
 * - `machines`：`/app/runtimes`（一台机器都没有时，加一台的入口在那儿）。
 */
export type FirstRunExit =
  | { kind: 'model-source'; label: string }
  | { kind: 'create'; label: string }
  | { kind: 'open-agent'; agentId: string; label: string }
  | { kind: 'machines'; label: string }

export interface FirstRunStep {
  key: FirstRunStepKey
  title: string
  state: FirstRunStepState
  /** 一句话（`done` 时也要能单独读：那一步就收成这一行）。 */
  detail: string
  /** 出口；`done` 与"还没有对象可动"时为 `null`（那一步的 detail 已经把前提说了）。 */
  exit: FirstRunExit | null
}

export interface FirstRunPlan {
  steps: FirstRunStep[]
  /** 现在缺的第一步（全 done ⇒ `null`）。 */
  next: FirstRunStepKey | null
}

/**
 * 这一页手里**已经有的**那几份读数（与"是哪一支"无关的那一半）。
 *
 * ⚠ `config` 说的是**本机**那条条目（app 里"接上"只指本机）：三步清单问的是
 * "这台机器上有没有模型源"，不是"某台远程机器上有没有" —— 后者等它真住到那台上再问
 * （那时 `modelChoiceMachine` 与「模型」那一间会答）。
 */
export interface FirstRunFrame {
  /** 本机那条条目的模型配置读数；`null` = 这一轮没读到 ⇒ **整块不摆**。 */
  config: HomeConfigView | null
  /** 这一页读到的家条目数（`0` = 一台都没有；读不到时上层也摆不出这块，见文件头第 1 条）。 */
  machines: number
  /** 世界侧所有权清单读到了没有。 */
  agentsKnown: boolean
  /** 世界侧一共有几支 Agent。 */
  agentsCount: number
}

/** 完整输入：框架 ＋ 这一支（可以还没有）。 */
export interface FirstRunInput extends FirstRunFrame {
  /** 这条路此刻说的是哪一支（刚建的那支）；`null` = 还没有 / 不确定是哪支。 */
  agentId: string | null
  /** 那一支的家侧裁决；`null` = 家条目还没就绪。 */
  resolution: AgentHomeResolution<HomeStatusView> | null
  /** 壳对这一支的最近一次读数（`undefined` = 这一会话里没问过）。 */
  anjia?: AnjiaResult | undefined
}

/** 条目在句子里的写法：本机那条不写它的名字（与 `agentMachine.ts` 的 `entryLabel` 同口径）。 */
function machineLabel(entry: { local: boolean; name: string }): string {
  return entry.local ? '这台机器' : entry.name
}

/**
 * 算出三步。**整块摆不摆**就在第一行：模型那一格读不到 ⇒ `null`。
 */
export function firstRunPlan(input: FirstRunInput): FirstRunPlan | null {
  const { config, machines, agentsKnown, agentsCount, agentId, resolution, anjia } = input
  if (config === null) return null
  const steps: FirstRunStep[] = [
    modelStep(config),
    createStep(agentsKnown, agentsCount),
    attachStep({ machines, agentId, resolution, anjia }),
  ]
  const next = steps.find((step) => step.state === 'todo')?.key ?? null
  return { steps, next }
}

/** 第一步：这台机器上有没有模型源（判据在 `homeConfig.ts`，这里只翻成人话）。 */
function modelStep(config: HomeConfigView): FirstRunStep {
  if (modelSourceAbsent(config)) {
    return {
      key: 'model',
      title: FIRST_RUN_TITLES.model,
      state: 'todo',
      detail: '这台机器上还没有模型源。',
      // 出口与列表行那句拒绝的出口**同一个目的地**（`AgentsListPanel` 的 `ModelSourceExit`）：
      // 这一步要办的事就是那一件事，两处说同一件事就该去同一个地方。
      exit: { kind: 'model-source', label: '去配模型源' },
    }
  }
  return {
    key: 'model',
    title: FIRST_RUN_TITLES.model,
    state: 'done',
    detail: '这台机器上已经有模型源。',
    exit: null,
  }
}

/** 第二步：世界里有没有一支 Agent（**所有权**，不是关系）。 */
function createStep(agentsKnown: boolean, agentsCount: number): FirstRunStep {
  if (!agentsKnown) {
    return {
      key: 'create',
      title: FIRST_RUN_TITLES.create,
      state: 'unknown',
      detail: '读不到你的 Agent 清单。',
      exit: null,
    }
  }
  if (agentsCount > 0) {
    return {
      key: 'create',
      title: FIRST_RUN_TITLES.create,
      state: 'done',
      detail: `已经有 ${agentsCount} 支 Agent。`,
      exit: null,
    }
  }
  return {
    key: 'create',
    title: FIRST_RUN_TITLES.create,
    state: 'todo',
    detail: '这个世界里还没有你的 Agent。',
    exit: { kind: 'create', label: '创建' },
  }
}

/**
 * 第三步：它接在哪台机器上。
 *
 * 顺序是有意的：**归属裁决先说**（`claimed` 是"某台机器报的就是它"—— 最硬的一条）；
 * 壳那条读数只在"还没有归属"时补一句（它是"这一次接的结果"，比裁决更新、但只说"接上了"，
 * 说不出住址）。两处都不许冒充对方：`claimed` 不给"正在接…"，`anjia` 不给住址。
 */
function attachStep(input: {
  machines: number
  agentId: string | null
  resolution: AgentHomeResolution<HomeStatusView> | null
  anjia: AnjiaResult | undefined
}): FirstRunStep {
  const { machines, agentId, resolution, anjia } = input
  const title = FIRST_RUN_TITLES.attach
  const base = { key: 'attach' as const, title }

  if (resolution !== null && resolution.kind === 'claimed') {
    return {
      ...base,
      state: 'done',
      detail: `它已经接在${machineLabel(resolution.entry)}上。`,
      exit: null,
    }
  }
  // 壳刚接过一次（成功那一档）：接上了，但读数的归属还没翻过来。
  // 那句话**原样来自** `anjiaReadout`（与列表行那一句同源），本层不另写一套。
  const readout = anjia ? anjiaReadout(anjia.kind, anjia.sub) : null
  if (readout !== null && readout.state === 'done') {
    return { ...base, state: 'done', detail: readout.text, exit: null }
  }
  if (agentId === null) {
    return {
      ...base,
      state: 'todo',
      detail: '建好之后才能把它接上。',
      exit: null,
    }
  }
  if (readout !== null && readout.state === 'working') {
    return { ...base, state: 'unknown', detail: readout.text, exit: null }
  }
  if (resolution === null) {
    return {
      ...base,
      state: 'unknown',
      detail: '正在读它接在哪台机器上…',
      exit: null,
    }
  }
  // 还没有机器可以被接（一条家条目都没有 / 一条都读不到）⇒ **说前提，不指假路径**：
  // 这一刻"接上"这件事在这台浏览器里没有落点，能走的只有"先有一台机器"。
  if (machines < 1) {
    return {
      ...base,
      state: 'todo',
      detail: '这需要一台装着 Celira 的机器 —— 在「我的执行端」里填上它的家地址和家钥匙。',
      exit: { kind: 'machines', label: '去我的执行端' },
    }
  }
  // ⚠ 失败 / 进行中那几档**不复述**壳那句话：它在同一张卡的下方由 `anjiaReadout` 渲染
  // （列表行那一块），两处各说一遍就是"同一个结果、两句话"。
  return {
    ...base,
    state: resolution.kind === 'unreachable' ? 'unknown' : 'todo',
    detail:
      resolution.kind === 'unreachable'
        ? '读不到它接没接上。'
        : '它还没接到任何一台机器上 —— 接上之后它才能回话。',
    // 目的地是**它的机器那一段**（`#machine`）：那一段本来就能走远程家，也是唯一点得亮
    // "接到 <机器名> 上"的地方。本层不给动作 —— 写入口仍然只有那一处。
    exit: { kind: 'open-agent', agentId, label: '去它的机器那一页' },
  }
}
