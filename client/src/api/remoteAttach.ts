/**
 * ★ **"让它去哪台机器上跑"**（2026-10-15 远程接上那一刀的客户端那一半）。
 *
 * ## 这一格的主语是**机器**，不是 Agent、也不是本机（归属）
 *
 * - **它是谁、属于谁** ⇒ 世界的 Agent 列表（不在这里判）；
 * - **它现在在哪台跑** ⇒ **各台机器自己报**（状态口的 `residents[]`）—— 一支一报，
 *   页面不替它们合成一个"当前机器"（那会把"某台没开机"读成"它不在这台"）；
 * - **允不允许从外面接上来** ⇒ **那一台机器自己报的那一格**（`remoteControl`）；
 * - **动手的是哪台** ⇒ 点到哪一行，就是**那一台**的手（本机走壳那只手，外地走它自己的
 *   `/home-control/attach`）。
 *
 * ⇒ 所以这一格的文案**必须点名机器**：「接到 <机器名> 上」。而「接到**这台**机器上」这句
 * 只许留给**本机那一档** —— 在远程上下文里"这台"是歧义（哪台？）。
 *
 * ## 三种"不能"要分开说（边界）
 *
 * `already-there`（它已经在那台跑）／`not-allowed`（那台没允许，附开它的命令）／
 * `unsupported`（那扇门不提供这条路：壳形态的机器只在本机接）／`unreadable`（那台读不到）
 * —— 四档四句话，**不压成"不能"**：它们要用户做的事完全不同。
 *
 * ## 一条硬事实写在旁边（直觉）
 *
 * V1：**同一支 Agent 同时至多一台机器在跑** ⇒ "接上第二台"是**搬家**，不是"多开一份"。
 * 这一句由调用方摆在那一格上（`oneMachineNote`），不许让人自己猜。
 */

import type { HomeEntry } from './homes.ts'
import { remoteAttachLine, remoteAttachReadout, type HomeConfigView } from './homeConfig.ts'

/** 一台机器在这一格上的处境（**读数**，不是判据）。 */
export interface MachineReading {
  entry: HomeEntry
  /** 那一台的模型配置读数（`null` = 读不到）。 */
  config: HomeConfigView | null
  /** 那一台报的住户 id 列表（`null` = 那一台的状态读不到 —— **不许**当成"没有它"）。 */
  residents: string[] | null
}

/** 一行（页面直接渲染；判断全在上面那个函数里）。 */
export interface MachineCandidate {
  entry: HomeEntry
  /** 那一台报的：这一支在不在它上面跑（`null` = 那台的状态读不到）。 */
  runningThere: boolean | null
  /** 那台机器允不允许从外面接上来（本机那台走壳那只手，永远允许）。 */
  allowed: boolean
  /** 这一行的话（**一处拼句**）。 */
  line: string
  /** 摆不摆那颗按钮；摆的话按钮上的字（**带机器名**）。 */
  action: { label: string } | null
}

/**
 * **哪些机器能让这一支去跑**（纯函数：可 `node --test` 直测）。
 *
 * 判据顺序是有意的：
 * 1. **那台读不到** ⇒ 如实说读不到（按钮不摆 —— 不指一条走不通的路）；
 * 2. **它已经在那台跑** ⇒ 说"在这台跑"，按钮不摆（那一档有幂等读数兜底，但摆一个无意义的
 *    按钮只会让人以为点了会"再开一份"）；
 * 3. **本机** ⇒ 允许（那颗按钮走壳那只手，与从前一字不变）；
 * 4. **外地** ⇒ 只有那台**自己报"允许"**时才摆按钮；没允许/不提供/没报，三档三句话。
 */
export function machineCandidates(input: {
  machines: MachineReading[]
  agentId: string | null | undefined
}): MachineCandidate[] {
  const id = typeof input.agentId === 'string' && input.agentId.trim() !== '' ? input.agentId : null
  return input.machines.map((machine) => {
    const name = machine.entry.name || machine.entry.id
    const runningThere =
      machine.residents === null ? null : id !== null && machine.residents.includes(id)
    const readout = remoteAttachReadout(machine.config)
    const allowed = machine.entry.local || readout.kind === 'on'
    const base = { entry: machine.entry, runningThere, allowed }

    if (id === null) {
      return { ...base, allowed: false, line: '还没读到它是谁 —— 读不到就不摆动作', action: null }
    }
    if (machine.config === null && machine.residents === null) {
      return { ...base, allowed: false, line: '这台机器读不到（它没开机，或地址不对）', action: null }
    }
    if (runningThere === true) {
      return { ...base, line: '它现在在这台机器上跑', action: null }
    }
    if (machine.entry.local) {
      return { ...base, line: '本机 —— 接上之后它就在这台机器上跑', action: { label: '接到这台机器上' } }
    }
    if (readout.kind === 'on') {
      return {
        ...base,
        line: `允许从别的机器接 Agent 上来（接上之后它就在 ${name} 上跑）`,
        action: { label: `接到 ${name} 上` },
      }
    }
    return { ...base, allowed: false, line: remoteAttachLine(readout), action: null }
  })
}

/**
 * 那一格上必须摆着的一句硬事实（V1）：**同一支 Agent 同时至多一台机器在跑**。
 *
 * 为什么必须写出来：不然"接上第二台"会被读成"多开一份" —— 而它是**搬家**
 * （世界那一侧同一支同时只认一条活跃后端连接；旧那台要处理，读数里会如实报）。
 */
export const ONE_MACHINE_NOTE =
  '同一支 Agent 同时只在一台机器上跑 —— 接到另一台上，就是从原来那台搬过去。'

/**
 * 一次接上的结局 → 一句人话（**一处拼句**）。
 *
 * 三档各说各的（**不许压成"成功/失败"两档**）：`homed-not-started` 是"接上了但还没起它"，
 * 它既不是失败，也不是"跑起来了" —— 说错这一句，用户会去等一个不会回话的 Agent。
 */
export function attachOutcomeLine(readout: { outcome: string; pid: number | null }): string {
  switch (readout.outcome) {
    case 'homed':
      return '接上了 —— 它已经在那台机器上跑起来了。'
    case 'homed-not-started':
      return '接上了 —— 但它还没选模型，所以先没起它（去它的页面上选一条，选好之后下次打开就会带起来）。'
    case 'already-homed':
      return '那台机器上本来就有它的家 —— 这次什么都没动。'
    default:
      return '那台机器报了"没接上" —— 上面那几行是它自己说的话。'
  }
}
