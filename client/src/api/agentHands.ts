/**
 * **这一支在这台机器上的终端权限**（档：`full` / `none`）—— Agent 页那一格的接线。
 *
 * ⚠ **改名欠账（2026-10-20 如实登记）**：模块/组件/壳命令的标识符仍叫 `hands` / `Hands` /
 * `hands_readout`（字面是"手"）——那是比喻当名字，违反主人 2026-10-05 那条裁决
 * （「名字只有一个；比喻不占名字的位置」）。屏幕上的词已经改成**终端权限**（主人原话：
 * 「啥叫给它那只手……应该叫类似权限什么的」）；标识符没跟着改，是因为这几个文件
 * （加上壳侧 `app/src-tauri/src/agent_hands.rs` 与 `hands_readout` / `hands_set` 两条命令）
 * 是**另一条线正在施工的未提交产物** —— 改名会与那条线撞车。⇒ 谁收工谁改，改成
 * `terminalGrant` / `TerminalGrantRow` / `terminal_grant_*` 就行；那条改名不该动这里的语义。
 *
 * ## 它是什么
 *
 * 一支 Agent 在**这台机器**上有没有终端权限（＝能不能跑命令、读写文件）。它是**支级授权**
 * （一支一张票），落在它自己环境根的 `agent.conf`（`terminal=` 一行），执行者是宿主
 * （起进程时读它）。
 *
 * ## 三条纪律
 *
 * 1. **只认本机**：这份权限是这台机器给的，不走远程、不进世界；页面只在"这一支住在这台机器上"
 *    时摆这一格（壳回 `homed`）。
 * 2. **不假装立刻生效**：写入之后**下一次它启动**才生效 —— 页面把那句话原样印出来
 *    （`HANDS_APPLIES`），不显示"已生效"。
 * 3. **读不到 ≠ 关着**：没有壳 / 调用失败是 `missing` / `error` 两态，页面据此不显示开关，
 *    绝不把它渲染成"关着"。
 *
 * 人话只有这一处（`handsReadoutLine` / `HANDS_CONSEQUENCE`）—— 壳那边只有读数与布尔。
 */

import { isTauriRuntime } from './shell.ts'

/** 壳回的那份读数（字段名照 camelCase；`declared` 只作读数，页面不拿它判档）。 */
export interface HandsView {
  agentId: string
  homed: boolean
  declared: string | null
  granted: boolean
  arm: boolean
}

/** 读/写三态：读到 / 没有这条路 / 读不到（**后两者都不是"关着"**）。 */
export type HandsState =
  | { kind: 'ready'; view: HandsView }
  | { kind: 'missing' }
  | { kind: 'error'; message: string }

/** 一句话说清"开这一格等于给什么"（开关旁边常驻，不藏在确认框里）。 */
export const HANDS_CONSEQUENCE =
  '允许后，它能在这台机器上跑命令、读写文件——包括模型钥匙和其他住户的经历。'

/** 生效点说白：这一格不立刻生效，也不假装。 */
export const HANDS_APPLIES = '改完重开 Celira 生效。'

/** 壳在不在（判据照 `shell.ts`：看内部注入，不看平台名、不看构建变量）。 */
export function hasHandsBridge(
  scope: Record<string, unknown> = globalThis as unknown as Record<string, unknown>,
): boolean {
  return isTauriRuntime(scope)
}

/** 把壳回的读数收敛成封闭形状；形状不对 ⇒ `null`（调用方按"读不到"处理，不猜）。 */
export function normalizeHands(raw: unknown): HandsView | null {
  if (raw === null || typeof raw !== 'object') return null
  const view = raw as Record<string, unknown>
  if (
    typeof view.homed !== 'boolean' ||
    typeof view.granted !== 'boolean' ||
    typeof view.arm !== 'boolean'
  ) {
    return null
  }
  return {
    agentId: typeof view.agentId === 'string' ? view.agentId : '',
    homed: view.homed,
    declared: typeof view.declared === 'string' ? view.declared : null,
    granted: view.granted,
    arm: view.arm,
  }
}

/**
 * 那一行状态：三档，**顺序就是判据顺序**（不住、这台机器给不了、允许/未允许）。
 *
 * ⚠ 屏幕上的词是**权限的语言**（2026-10-20 主人：「应该叫类似权限什么的」）：
 * 「手」「终端臂」这类比喻不进用户可见面（那是机制的形状，不是他看到的那件事）。
 */
export function handsReadoutLine(view: HandsView): string {
  if (!view.homed) return '这一支不住在这台机器上'
  if (!view.arm) return '这台机器不提供终端权限（只有 Windows 与 Linux 有）'
  return view.granted ? '已允许 · 整台机器' : '未允许'
}

/** 读这一支的终端权限。**没有壳是 `missing`，调用失败是 `error`** —— 都不许落成"关着"。 */
export async function readHands(agentId: string): Promise<HandsState> {
  if (!hasHandsBridge()) return { kind: 'missing' }
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const view = normalizeHands(await invoke<unknown>('hands_readout', { agentId }))
    return view === null ? { kind: 'error', message: '没能读懂这台机器的回答' } : { kind: 'ready', view }
  } catch (error) {
    console.error('Celira：读这一支的手没成', error)
    return { kind: 'error', message: '读不到这一项的现状' }
  }
}

/** 给 / 收这一支的终端权限。失败**原样带回壳的话**（不吞、不美化）。 */
export async function setHands(agentId: string, granted: boolean): Promise<HandsState> {
  if (!hasHandsBridge()) return { kind: 'missing' }
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const view = normalizeHands(await invoke<unknown>('hands_set', { agentId, granted }))
    return view === null ? { kind: 'error', message: '没能读懂这台机器的回答' } : { kind: 'ready', view }
  } catch (error) {
    console.error('Celira：改这一支的手没成', error)
    const message = typeof error === 'string' && error.trim() !== '' ? error : '没能改过来'
    return { kind: 'error', message }
  }
}
