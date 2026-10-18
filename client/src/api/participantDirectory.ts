// 参与者目录（Participant Directory）。
//
// 这是**参与者查询链**的唯一权威位置：给定一个 id，按「真实投影 → 兜底来源」的顺序
// 解析出一个 Participant。它由三种**外部登记**组成，本模块自己不做任何判断：
//   1. `registerParticipantLookup` —— 谁登记真实投影（`api/store.ts` 从 core 连接填）；
//   2. `registerParticipantFallbackSource` —— 谁登记兜底来源（`fixtures/data.ts` 把
//      演示目录登记进来）；
//   3. `registerFixtureFallbackGate` —— 谁决定兜底**是否可用**（`api/store.ts` 读当前会话）。
//
// 它**不属于** `fixtures/`（那是演示数据）：本模块对 `fixtures/` 零依赖，也不知道
// 演示数据是什么。查询链住在这里（中立位置）的原因，正是「一个事实一个归属」——
// 演示数据只登记自己，能不能回落由闸门说了算。

import type { Participant } from './domain'

let participantLookup: ((id: string) => Participant | undefined) | null = null

export function registerParticipantLookup(fn: (id: string) => Participant | undefined) {
  participantLookup = fn
}

/** 兜底来源：查询 miss 且闸门允许时，用来捞一个 Participant 的那份来源。 */
let participantFallbackSource: ((id: string) => Participant | undefined) | null = null

export function registerParticipantFallbackSource(fn: (id: string) => Participant | undefined) {
  participantFallbackSource = fn
}

/**
 * **回落要不要允许**（TASK_56）。
 *
 * 这是「演示数据会不会被当成真实数据」的**总出口**：查询 miss 之后是不是还能
 * 在兜底来源里捞一个出来。默认允许（= 演示/开发路径的既有行为），
 * 由 `api/store.ts` 注册一个**读当前会话**的闸门函数；真实世界下闸门关闭 ⇒
 * 查不到就是查不到（`getParticipant` 抛、`tryGetParticipant` 返 null），
 * 绝不把 `p-inori` 这种演示身份当成真实用户。
 *
 * 为什么做成**函数**而不是布尔：判定依赖当前会话（mode / token），
 * 写成布尔就得在每个改会话的地方同步一次，漏一处就是一次造假。
 */
let fixtureFallbackGate: (() => boolean) | null = null

export function registerFixtureFallbackGate(fn: () => boolean) {
  fixtureFallbackGate = fn
}

/** 当前是否允许回落兜底来源（没注册闸门 = 允许，保持既有行为）。 */
export function fixtureFallbackAllowed(): boolean {
  return fixtureFallbackGate ? fixtureFallbackGate() : true
}

export function getParticipant(id: string): Participant {
  const fromLookup = participantLookup?.(id)
  if (fromLookup) return fromLookup
  // 真实世界：查不到就是查不到。**不许**在这里捞一个同名的演示身份出来
  // （那是"拿假身份冒名顶替真实 id"）。
  if (fixtureFallbackAllowed()) {
    const found = participantFallbackSource?.(id)
    if (found) return found
  }
  throw new Error(`unknown participant: ${id}`)
}

export function tryGetParticipant(id: string): Participant | null {
  try {
    return getParticipant(id)
  } catch {
    return null
  }
}
