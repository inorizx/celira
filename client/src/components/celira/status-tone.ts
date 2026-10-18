/**
 * 状态灯色语义 → 视觉类（**组件层唯一映射**）。
 *
 * 灯色语义在**呈现层自持**：谁该点哪一档由调用方按自己的判据决定
 * （「运行」那个房间按本机状态口的读数分档，见 `Runtimes.tsx`），本模块只管「档 → 类」。
 * 具体用哪个 token 只在这里决定一次，调用方共用，避免各写一份而漂移。
 * 只用 token 派生的工具类（不写 hex / 不写裸色阶）——`check-tokens.mjs` 把关。
 *
 * ⚠ **名字与它的来历**（TASK_69 名字收尾）：原名 `liveness-tone.ts`，服务的是「Agent 的存活」那个
 * 口径——那口径的输入只有 Host 线提供，已随 TASK_63（Host 删除刀 C）删掉。标识符改名不属于那一
 * 刀的范围，当时**登记为待办**；这里一并收掉。顺带改正原文里一处**过期指针**：它写着"同族还有
 * `store.ts` 的几个 `host*` 状态残留"，而实测 `store.ts` 里 `host` **零命中**（早已清干净）
 * ⇒ 那句不再指路，删掉，免得下一个读者照着去找不存在的东西。
 */

import type { AgentHealthTone } from '@/api/agentHealth'
export type ToneLevel = 'ok' | 'warn' | 'danger' | 'muted'

export const STATUS_TONE_CLASS: Record<ToneLevel, { dot: string; text: string }> = {
  ok: { dot: 'bg-presence-online', text: 'text-presence-online' },
  warn: { dot: 'bg-presence-timeout', text: 'text-presence-timeout' },
  danger: { dot: 'bg-destructive', text: 'text-destructive' },
  muted: { dot: 'bg-muted-foreground', text: 'text-muted-foreground' },
}

/**
 * **读数那一句话的四档** → 文本色类。
 *
 * 与 [`STATUS_TONE_CLASS`] 分开列，因为两者是**不同的东西**：那一份是**状态灯**（带圆点，
 * 最重的一档是 `danger` → `text-destructive`）；这一份是**一句读数的文字色**，档名取自
 * `api/agentHealth.ts` 的 `AgentHealthTone`（**同一个词汇表，不另起一套**）。
 *
 * ⚠ **必须只有一处**：`AgentsListPanel` 那一行与 Agent 主页「机器」那一段渲染的是
 * **逐字相同的同一句话**（同一个纯函数给的）⇒ 各写一份就是"同一句话、两种颜色"的漂移，
 * 而且没人会去比对两页的颜色。颜色只许来自 `tokens.css`（`check-tokens.mjs` 守着）。
 */
export const READOUT_TONE_TEXT: Record<AgentHealthTone, string> = {
  ok: 'text-presence-online',
  warn: 'text-presence-timeout',
  bad: 'text-presence-failed',
  muted: 'text-muted-foreground',
}
