// Agent 后端绑定的读取面：**「还没读到」与「确认没有」必须分开**。
//
// store 把某个 Agent 的绑定落成 `bindingsByAgent[agentId]`，而 `refreshBindings` 只在
// **读成功**时写这个键（失败时 `catch { keep previous }`）。于是"键不在"有两个来源：
// 还没读、或读了但失败 —— 两者都**不是**"这个 Agent 没有绑定"。
//
// 收口前消费侧写的是 `bindingsByAgent[agent.id] ?? []`，页面据此渲染
// 「还没有后端绑定」**并把创建表单递上去**。那不是崩溃，是**一条看着像事实的假话**，
// 而且代价具体：Core 契约里**同一 Agent 至多一个 active Binding**，在看不见既有绑定的
// 情况下邀请创建，等于诱导用户去撞那条限制（或造出第二条）。
//
// 纯函数、无 React、无 zustand ⇒ `node --test` 直测（见 `tests/agent-bindings.test.ts`）。

import type { BackendBindingView } from './contracts'

/**
 * 一次绑定读取的**两态**（不许压成一态）：
 *   - `known`   —— 读到了一份权威读数（**空数组也算读到**：确认这个 Agent 还没有绑定）；
 *   - `unknown` —— 没读到（还没加载 / 读取失败 / 参数不全）：**这不是"没有绑定"**。
 */
export type BindingsReadout =
  | { status: 'known'; bindings: BackendBindingView[] }
  | { status: 'unknown' }

/** 取某个 Agent 的后端绑定读数。键就是 agentId（单键，不做任何拼接）。 */
export function bindingsOf(
  byAgent: Readonly<Record<string, BackendBindingView[]>>,
  agentId: string | null | undefined,
): BindingsReadout {
  if (!agentId) return { status: 'unknown' }
  const hit = byAgent[agentId]
  return hit ? { status: 'known', bindings: hit } : { status: 'unknown' }
}
