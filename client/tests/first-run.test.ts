// **第一次那三步**（`src/api/firstRun.ts`）——纯逻辑直测（2026-10-20 T1 ②）。
//
// ## 这一层要钉的是什么
//
// 目标句：**新账号 + 一台空机器**，从 `/app/agents` 空态出发、不离开这块区域、不查文档，
// 能走到"它被接上且有模型源"。这条路散在三页（创建 / 那台机器的「模型」间 / Agent 主页的
// 「机器」段），所以判据必须**在页面之外**能被钉住 —— 页面那一层（React）不进 `node --test`。
//
// 三条要紧的：
//
// 1. **读不到 → 不摆**：模型那一格读不到时 `firstRunPlan` 返回 `null`（整块不渲染）。
//    摆一份三步清单却不知道第一步是什么，比不摆更坏（与机器页那张「先配一条模型源」卡
//    同一条在场判据：`modelSourceAbsent` 的调用方一律"读不到不摆"）。
// 2. **缺模型 → 摆那一步，且带出口**：那一步是 `todo` ＋ 一个能走的目的地
//    （`model-source`，与列表行那句拒绝同一个去处）。
// 3. **已接上 → 那步收成一行**：`done` ＋ 一句话 ＋ **不再给出口**。
//
// 另外两条边界：
//
// - **"读不到"不许说成"没有"**：所有权清单没读到时第二步是 `unknown`（不是 `todo`）；
//   机器一份都没答话时第三步也是 `unknown`（不是"它还没接上"——那是替系统断言）。
// - **一条机器都没有时只说前提**：出口指「我的机器」（加一台的入口在那儿），
//   不许指一条不存在的"就地接上"（`AGENTS.md` 的假路径纪律）。
//
// ⚠ 本文件逐条对齐**渲染出来的那几个词**：`tests/page-subject-lines.test.ts` 另有源级断言
// 盯着"这三步的名字只有一处、出口在面板里真的被渲染"（页面能跑 ≠ 判据能跑，两边都要有）。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import {
  FIRST_RUN_TITLES,
  firstRunPlan,
  type FirstRunInput,
  type FirstRunStep,
} from '../src/api/firstRun.ts'
import type { HomeConfigView } from '../src/api/homeConfig.ts'
import type { HomeEntry, AgentHomeResolution } from '../src/api/homes.ts'
import type { HomeStatusView } from '../src/api/homeStatus.ts'
import type { AnjiaResult } from '../src/api/anjia.ts'
import { leakedJargon } from '../scripts/jargon.mjs'

// ---------------------------------------------------------------------------
// 夹具：三份读数各按**它们自己的门会给的形状**造（不造空气）
// ---------------------------------------------------------------------------

/** 一台机器上**还没有任何模型源**（两台门共有那两格：空表 ＋ 没有任何一支在用）。 */
function configAbsent(): HomeConfigView {
  return {
    readAt: '2026-10-20T09:00:00.000Z',
    state: 'absent',
    keyConfigured: false,
    endpoint: null,
    model: null,
    contextWindow: null,
    protocol: null,
    reason: null,
    sourcesState: 'absent',
    entries: [],
    bindingState: 'absent',
    bound: null,
  }
}

/** 同一台机器上**已经有一条模型源**。 */
function configWithSource(): HomeConfigView {
  return {
    ...configAbsent(),
    state: 'loaded',
    keyConfigured: true,
    endpoint: 'https://api.example.invalid/chat/completions',
    model: 'some-model',
    sourcesState: 'loaded',
    entries: [
      {
        id: 'ms-1',
        name: '主钥匙',
        endpoint: 'https://api.example.invalid/chat/completions',
        model: 'some-model',
        contextWindow: 131072,
        protocol: null,
        keyConfigured: true,
      },
    ],
  }
}

function entry(over: Partial<HomeEntry> = {}): HomeEntry {
  return {
    id: 'local',
    name: '本机',
    baseUrl: '',
    statusPort: 5190,
    configPort: 5191,
    key: '',
    local: true,
    ...over,
  }
}

/** 一份合法状态读数（这一层只用得到 `host` 那一格，其余给最小合法值）。 */
function statusView(): HomeStatusView {
  return {
    readAt: '2026-10-20T09:00:00.000Z',
    stale: false,
    im: { health: 200 },
    agent: { state: 'loaded', admission: 'ready', runs: [] },
    lastWake: null,
  }
}

function claimed(at: HomeEntry = entry()): AgentHomeResolution<HomeStatusView> {
  return { kind: 'claimed', entry: at, view: statusView(), ownReadings: true }
}

/** 三步的默认输入：**空账号 + 一台空机器**（正是目标那一格）。 */
function input(over: Partial<FirstRunInput> = {}): FirstRunInput {
  return {
    config: configAbsent(),
    machines: 1,
    agentsKnown: true,
    agentsCount: 0,
    agentId: null,
    resolution: null,
    anjia: undefined,
    ...over,
  }
}

/** 取某一步（找不到就算失败 —— 别让用例在空数组上静默通过）。 */
function step(plan: ReturnType<typeof firstRunPlan>, key: 'model' | 'create' | 'attach'): FirstRunStep {
  assert.ok(plan !== null, '这一份输入本该摆出清单')
  const found = plan.steps.find((each) => each.key === key)
  assert.ok(found, `清单里没有「${key}」那一步`)
  return found
}

// ---------------------------------------------------------------------------
// 一、读不到 ⇒ 整块不摆
// ---------------------------------------------------------------------------

test('★ 读不到模型那一格 ⇒ 整块不摆（`null`，不是三步都写"未知"）', () => {
  assert.equal(firstRunPlan(input({ config: null })), null)
})

test('★ 读不到 ≠ 没有：清单没读到 / 机器没答话 都不许说成"没有"', () => {
  // 所有权清单没读到 ⇒ 第二步 `unknown`（不是"这个世界里还没有你的 Agent"）。
  const blind = firstRunPlan(input({ agentsKnown: false }))
  assert.equal(step(blind, 'create').state, 'unknown')
  assert.equal(step(blind, 'create').exit, null, '读不到就不给出口（不知道对象是什么）')
  // 一台机器都没答话 ⇒ 第三步 `unknown`（不是"它还没接上"）。
  const unreachable = firstRunPlan(
    input({
      agentsCount: 1,
      agentId: 'agent-1',
      resolution: { kind: 'unreachable', entry: entry(), failure: '那台机器没开机' },
    }),
  )
  assert.equal(step(unreachable, 'attach').state, 'unknown')
})

// ---------------------------------------------------------------------------
// 二、缺模型 ⇒ 摆那一步，且带出口
// ---------------------------------------------------------------------------

test('★ 缺模型源 ⇒ 第一步是 todo ＋ 出口（去配模型源，与列表行同一个去处）', () => {
  const plan = firstRunPlan(input())
  const model = step(plan, 'model')
  assert.equal(model.state, 'todo')
  assert.equal(model.title, '模型源')
  assert.deepEqual(model.exit, { kind: 'model-source', label: '去配模型源' })
  // 「下一步」就是它（空态里第一步没走完，后面两步无法成立）。
  assert.equal(plan?.next, 'model')
})

test('★ 有模型源 ⇒ 第一步 done，且**不再给出口**（收成一行）', () => {
  const plan = firstRunPlan(input({ config: configWithSource() }))
  const model = step(plan, 'model')
  assert.equal(model.state, 'done')
  assert.equal(model.exit, null)
  assert.ok(model.detail.length > 0, 'done 也要能单独读（那一行就是它的全部）')
})

test('空账号：第二步 todo ＋ 「创建」出口；第三步说前提（先建一支，没有出口）', () => {
  const plan = firstRunPlan(input())
  const create = step(plan, 'create')
  assert.equal(create.state, 'todo')
  assert.deepEqual(create.exit, { kind: 'create', label: '创建' })
  const attach = step(plan, 'attach')
  assert.equal(attach.state, 'todo')
  assert.equal(attach.exit, null)
  assert.ok(attach.detail.includes('建好'), `还没有 Agent 时那句该说"先建一支"：${attach.detail}`)
})

test('已经有 Agent：第二步 done（说几支），不再给出口', () => {
  const create = step(firstRunPlan(input({ agentsCount: 2 })), 'create')
  assert.equal(create.state, 'done')
  assert.equal(create.exit, null)
  assert.ok(create.detail.includes('2'), `那句话该说清几支：${create.detail}`)
})

// ---------------------------------------------------------------------------
// 三、接上 ⇒ 收成一行；没接上 ⇒ 出口（浏览器里也有真路）
// ---------------------------------------------------------------------------

test('★ 已接上（某台机器报的就是它）⇒ 那一步 done、一句话、无出口', () => {
  const local = step(firstRunPlan(input({ agentsCount: 1, agentId: 'agent-1', resolution: claimed() })), 'attach')
  assert.equal(local.state, 'done')
  assert.equal(local.exit, null)
  assert.ok(local.detail.includes('这台机器'), `本机那档该说「这台机器」：${local.detail}`)
  // 远程那台：点名它的名字（不许说"这台机器"）。
  const remote = step(
    firstRunPlan(
      input({
        agentsCount: 1,
        agentId: 'agent-1',
        resolution: claimed(entry({ id: 'ranch', name: '书房那台', local: false, baseUrl: 'https://x.invalid' })),
      }),
    ),
    'attach',
  )
  assert.ok(remote.detail.includes('书房那台'), `远程那档该点名：${remote.detail}`)
})

test('★ 没接上（认领到的是别人 / 读到的机器都没报它）⇒ todo ＋ 深链出口（不复制接上的逻辑）', () => {
  for (const resolution of [
    { kind: 'elsewhere' } as AgentHomeResolution<HomeStatusView>,
    { kind: 'unknown-id', entry: entry(), view: statusView() } as AgentHomeResolution<HomeStatusView>,
  ]) {
    const attach = step(firstRunPlan(input({ agentsCount: 1, agentId: 'agent-1', resolution })), 'attach')
    assert.equal(attach.state, 'todo')
    assert.deepEqual(attach.exit, {
      kind: 'open-agent',
      agentId: 'agent-1',
      label: '去它的机器那一页',
    })
  }
})

test('壳刚接过一次（成功那一档）⇒ 就地收成一行，且那句话**原样来自** `anjiaReadout`', () => {
  const homed: AnjiaResult = {
    agentId: 'agent-1',
    kind: 'homed',
    sub: null,
    pid: 4242,
    envRoot: '/tmp/env/agent-1',
    logFile: null,
  }
  const attach = step(
    firstRunPlan(
      input({
        agentsCount: 1,
        agentId: 'agent-1',
        // 归属还没翻过来（读数下一轮才到）—— 这一格由壳那句话先答。
        resolution: { kind: 'elsewhere' },
        anjia: homed,
      }),
    ),
    'attach',
  )
  assert.equal(attach.state, 'done')
  assert.equal(attach.exit, null)
  assert.ok(attach.detail.includes('接'), attach.detail)
})

test('一条机器都没有 ⇒ 只说前提 ＋ 指「我的机器」，不摆"接上"的按钮', () => {
  const attach = step(
    firstRunPlan(input({ machines: 0, agentsCount: 1, agentId: 'agent-1', resolution: { kind: 'elsewhere' } })),
    'attach',
  )
  assert.equal(attach.state, 'todo')
  assert.deepEqual(attach.exit, { kind: 'machines', label: '去我的机器' })
  assert.ok(attach.detail.includes('装着 Celira 的机器'), `没有机器时该说清前提：${attach.detail}`)
})

// ---------------------------------------------------------------------------
// 四、全齐 ⇒ 没有"下一步"；且没有一句内行词（`verify-ui` 同一份词表）
// ---------------------------------------------------------------------------

test('三步都齐 ⇒ `next === null`（页面据此说"三步都齐了"）', () => {
  const plan = firstRunPlan(
    input({
      config: configWithSource(),
      agentsCount: 1,
      agentId: 'agent-1',
      resolution: claimed(),
    }),
  )
  assert.ok(plan)
  assert.equal(plan.next, null)
  assert.ok(plan.steps.every((each) => each.state === 'done'))
})

test('三步的每一句话都不含内行词（拿 `verify-ui` 那份词表离线扫一遍矩阵）', () => {
  const plans = [
    firstRunPlan(input()),
    firstRunPlan(input({ config: configWithSource() })),
    firstRunPlan(input({ agentsKnown: false })),
    firstRunPlan(input({ agentsCount: 3, agentId: 'agent-1', resolution: claimed() })),
    firstRunPlan(input({ agentsCount: 1, agentId: 'agent-1', resolution: { kind: 'elsewhere' } })),
    firstRunPlan(
      input({ agentsCount: 1, agentId: 'agent-1', resolution: { kind: 'unreachable', entry: null, failure: '没开机' } }),
    ),
    firstRunPlan(input({ machines: 0, agentsCount: 1, agentId: 'agent-1', resolution: { kind: 'elsewhere' } })),
  ]
  const texts = plans.flatMap((plan) => (plan === null ? [] : plan.steps.map((each) => each.detail)))
  const labels = plans.flatMap((plan) =>
    plan === null ? [] : plan.steps.flatMap((each) => (each.exit ? [each.exit.label] : [])),
  )
  assert.ok(texts.length >= 18, `矩阵要够宽，现在只有 ${texts.length} 句`)
  for (const text of [...texts, ...labels, ...Object.values(FIRST_RUN_TITLES)]) {
    assert.deepEqual(leakedJargon(text), [], `这一句漏了内行词：${text}`)
  }
})
