// 房间头部那两行读数的**唯一裁决面**（客户端侧）：这间房里有多少人、各是什么、对端在不在。
//
// ## 为什么要有它（2026-10-14 真机读数逼出来的）
//
// 头部那一行原来是**两个组件各算一遍**（`ChatHeader` 与 `RoomInfoDrawer`），而且**口径会漂**：
// 世界的成员表（`membersByRoom`）**加载过** ⇒ 人数含我（「2 位参与者 · 1 Human · 1 Agent」）；
// **没加载** ⇒ 退回房间视图自带的 `participants`，而 `direct` 房那一份**只有对端**
// ⇒ 「1 位参与者 · **0 Human** · 1 Agent」。同一间房、同一句话，随**读取时机**改口径——
// 而且「0 Human」按"这个房间"读是**假的**（房间里就有一个人：我）。
//
// ## 三条纪律（本模块就是它们的唯一落点）
//
// 1. **人数含我**：1:1 显「2 位参与者」、群里「N 位」（与世界惯例一致：微信「群成员 N」含自己）。
//    推论：**「0 Human」在结构上不可能再出现**——我在这间房里，而我是人。
// 2. **在线不含我**（`liveHeadline` 那条理由：自己在线对自己没有信息量）——所以两处的主语
//    各自写出来，不让人猜。
// 3. **读不到 ≠ 没有**：成员表不在场时数字只是"**已知**"（`authoritative: false`），
//    页面据此写「已知 N 位」；进行态读不到时写「读不到在线状态」，**不写「不在线」**。
//
// 只读派生、无副作用、无 React、无 store ⇒ `node --test` 直测（见 `tests/room-headline.test.ts`）。

import { liveMemberOf } from './roomLive.ts'
import type { RoomLiveSnapshot } from './contracts'

/** 参与这间房读数的最小形状（真实调用方传的是带 role / official 的行，泛型原样保留）。 */
export interface RollMember {
  id: string
  kind: string
  displayName: string
}

/** 我（`GET /v1/home` 的 `participant`）→ 参与人数计算的那一行；没登录 / 形状不对 ⇒ `null`。 */
export function selfMember(
  me: { participant?: { id: string; kind: string; display_name: string } | null } | null | undefined,
): RollMember | null {
  const participant = me?.participant
  if (!participant || !participant.id) return null
  return {
    id: participant.id,
    kind: participant.kind || 'human',
    displayName: participant.display_name || '我',
  }
}

/**
 * 成员列表**含我**（不在就补上，返回新数组；已经在了原样返回）。
 *
 * 判据不是"我大概在里面"，而是 [`roomMembers`](../api/roomMembers.ts) 那句现成的话：
 * **你能看见的房间至少有你一个成员** —— 所以"成员表里没有我"只可能是**来源不全**
 * （`direct` 房那条来源只给对端），不是"我不在这间房"。
 */
export function membersWithSelf<T extends RollMember>(
  members: readonly T[],
  self: T | null | undefined,
): T[] {
  if (!self || members.some((m) => m.id === self.id)) return [...members]
  return [...members, self]
}

/** 这间房的人数与分解（**含我**）。 */
export interface MemberRoll {
  /** 人数（含我）。`authoritative` 为假时是"**已知**这么多"，不是"就这么多"。 */
  total: number
  human: number
  agent: number
  /** 世界的成员表在场吗（`membersByRoom` 非空）。不在场 ⇒ 数字只是"已知"。 */
  authoritative: boolean
}

/**
 * 人数与分解：**含我**，且只认一个口径。
 *
 * `authoritative` 由调用方按来源给（`membersByRoom` 非空 ⇒ 真）——它不改数字，只改**措辞**
 * （「N 位参与者」／「已知 N 位参与者」）。把它压成一个数字就会变成"来源不全时也说得斩钉截铁"。
 */
export function memberRoll(
  members: readonly RollMember[],
  self: RollMember | null | undefined,
  authoritative: boolean,
): MemberRoll {
  const all: RollMember[] = membersWithSelf<RollMember>(members, self)
  const human = all.filter((m) => m.kind === 'human').length
  return { total: all.length, human, agent: all.length - human, authoritative }
}

/**
 * 对端此刻的状态：三态**不许压成两态**（"读不到"不是"不在线"）。
 *
 * 与 [`isOnline`](./roomLive.ts) 的分工：那个函数服务于**一个点**（读不到就不点亮，够了）；
 * 这个函数服务于**一句话**（"不在线"是一句断言，只有快照里真有那一行、且写着 `online=false`
 * 时才准说）。
 */
export type PeerPresence = 'online' | 'offline' | 'unknown'

/** 快照不在场 / 那一行不在快照里 ⇒ `'unknown'`（读不到就是读不到）。 */
export function peerPresence(
  snapshot: RoomLiveSnapshot | undefined,
  peerId: string | null | undefined,
): PeerPresence {
  if (!peerId) return 'unknown'
  const row = liveMemberOf(snapshot, peerId)
  if (!row) return 'unknown'
  return row.online === true ? 'online' : 'offline'
}

/** 那一格的人话（**唯一一处措辞**：头部与房间信息两处共用）。 */
export function presenceLabel(state: PeerPresence): string {
  if (state === 'online') return '在线'
  if (state === 'offline') return '不在线'
  return '读不到在线状态'
}

/** 群房那一行的措辞：人数含我、在线不含我，两个主语都写出来。 */
export function groupHeadline(roll: MemberRoll, onlineCount: number): string {
  const head = roll.authoritative ? '' : '已知 '
  const line = `${head}${roll.total} 位参与者（含你） · ${roll.human} Human · ${roll.agent} Agent`
  return onlineCount > 0 ? `${line} · ${onlineCount} 位在线（不含你）` : line
}
