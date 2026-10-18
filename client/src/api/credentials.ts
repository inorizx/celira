// 凭据读取面（Agent Backend Credential Scope）的**唯一裁决处**。
//
// 「某个 Agent 在某个后端绑定下有哪些凭据」这份数据，在 store 里落成一张
// `Record<键, CredentialView[]>` 的表。收口前**生产与消费各写一遍键的拼法**：
//   - 生产：`store.ts` `credentialsByBinding: { ...s.credentialsByBinding, [`${agentId}:${bindingId}`]: credentials }`
//   - 消费：`MyAgentsPage.tsx`  `` credentialsByBinding[`${agent.id}:${activeBindingId}`] ?? [] ``
//
// 两处各写一遍模板串 = 任一侧改了分隔符或字段顺序，另一侧**不会报错、不会红**，只会
// 查不到键。而消费侧那一句 `?? []` 会把「查不到」变成 **`[]`（"没有凭据"）** ——
// 那不是崩溃，是**一条看着像事实的假话**：页面平静地显示"这个绑定下没有凭据"，
// 而事实可能是"我们根本没读到"或"读到的键形状已经漂了"。
//
// ⇒ 本模块给两件事各一个名字：**键**（`credentialScopeKey`，两侧同一个 import）与
// **读数**（`credentialsOf`，把「确认没有」与「读不到」分开，并且把键形状漂移显式报出来）。
//
// 纯函数、无 React、无 zustand ⇒ `node --test` 直测（见 `tests/credentials.test.ts`）。

import type { CredentialView } from './contracts'

/**
 * 凭据作用域键：**生产与消费必须调用同一个函数**（不许任何一侧再手写模板串）。
 * 形状（`agentId:bindingId`）是本模块的实现细节，不是契约 —— 谁都不该依赖它的字面量。
 */
export function credentialScopeKey(agentId: string, bindingId: string): string {
  return `${agentId}:${bindingId}`
}

/**
 * 一次凭据读取的**三态**（不许压成两态）：
 *   - `known`   —— 读到了一份权威读数（**空数组也算读到**：确认这个绑定下没有凭据）；
 *   - `unknown` —— 没读到（还没加载 / 读取失败 / 参数不全）：**这不是"没有凭据"**；
 *   - `drifted` —— 表里出现了**同一绑定的另一种键形状**：生产与消费已经不一致，
 *                  此时**停止显示**并把问题显式报出来，绝不把漂移读成"没有凭据"。
 */
export type CredentialReadout =
  | { status: 'known'; credentials: CredentialView[] }
  | { status: 'unknown' }
  | { status: 'drifted'; keys: string[] }

/**
 * 取某个 Agent 在某个后端绑定下的凭据读数。
 *
 * 漂移判据：表里存在**包含同一个 `bindingId` 却不是规范键**的键。锚点选 `bindingId`
 * 而不是 `agentId`，因为 binding id 是规范 UUID v4（不会被别的键偶然包含），而演示/短
 * 形 id 之间存在子串关系（`a-c` 与 `a-courier`），拿 `agentId` 当锚会**误报**。
 *
 * **漂移优先于命中**：若规范键与漂移键同时存在，说明有两个写者各按各的形状写过同一件
 * 事 —— 这种"两套写者并存"的信号必须响，不许因为规范键恰好有值就静默。
 */
export function credentialsOf(
  byBinding: Readonly<Record<string, CredentialView[]>>,
  agentId: string,
  bindingId: string,
): CredentialReadout {
  if (!agentId || !bindingId) return { status: 'unknown' }
  const key = credentialScopeKey(agentId, bindingId)
  const drifted = Object.keys(byBinding)
    .filter((k) => k !== key && k.includes(bindingId))
    .sort()
  if (drifted.length > 0) return { status: 'drifted', keys: drifted }
  const hit = byBinding[key]
  return hit ? { status: 'known', credentials: hit } : { status: 'unknown' }
}
