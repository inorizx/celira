// 房间头部那一行的**唯一裁决面**（客户端侧）：这间房里还有谁、此刻能不能找到人。
//
// ## 2026-10-20：句子不再报"账"（主人真机点名）
//
// 主人截了一张 2 人房（人 ＋ 1 个 Agent）的头部，那一行是：
//
//   「2 位参与者（含你） · 1 Human · 1 Agent · 1 位在线（不含你）」
//   原话：「说了一大堆，是不是很逆天」
//
// 病不在算错了（数字是对的），在于**把一张账贴到了标题上**：
// - 「（含你）」「（不含你）」是**对着读者解释自己的口径** —— 人话句子不这么写；
// - 「1 Human · 1 Agent」是**分类学的计数**，而这间房里只有一个对端，分类没有信息量；
// - 判据：**把 im 服务端换成微信/QQ，这句话还成立吗？** 不成立 ⇒ 它不该长在这儿。
//
// ⇒ 现在的措辞只有三种情形（都不带括号、不带英文分类名）：
// - **除我之外至多一人**：「小紫 · 在线」（1:1 房的标题已经是对方的名字 ⇒ 只写「在线」）；
// - **不止一人**：「3 位成员 · 2 位在线」；
// - **只有我**：「只有你」（成员表还没到 ⇒ 「成员还没读到」，**不写"只有你"**）。
//
// 两条口径**写进计算、不写进句子**：
// 1. **人数含我**（与世界惯例一致：微信「群成员 N」含自己；含我的直接后果是
//    「0 Human」这种在结构上不可能出现的读数再也不会出现在屏幕上）；
// 2. **在线不含我**（自己在线对自己没有信息量）。
// ⇒ 判据是"这间房几个人 / 别人里谁在线"这两个自然问句，而不是给读者加括注。
//
// **分解（几个人、几个 Agent）不进句子**：它的家在成员列表（每一行一枚徽标，抽屉里还按
// 「Agent · N」「人 · N」分组）。同一份事实写两遍，就是两份迟早打架的语料 ——
// 2026-10-14 那次「同一间房、同一句话，随读取时机改口径」正是这么长出来的。
//
// ## 它最早解决什么（2026-10-14 真机读数逼出来的）
//
// 头部那一行原来两个组件各算一遍（`ChatHeader` 与 `RoomInfoDrawer`），口径会漂：世界的成员表
// **加载过** ⇒ 人数含我（「2 位参与者」）；**没加载** ⇒ 退回房间自带的那份 `participants`，
// 而 `direct` 房那一份**只有对端** ⇒ 「1 位参与者 · 0 Human · 1 Agent」。同一间房、同一句话，
// 随读取时机改口径 —— 所以「含我」这条判据与 `membersWithSelf` 一起留到了今天。
//
// 只读派生、无副作用、无 React、无 store ⇒ `node --test` 直测（见 `tests/room-headline.test.ts`）。

import { liveHeadline, liveMemberOf } from './roomLive.ts'
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

/** `roomHeadline` 要的全部输入（两个调用方各自把手里那份交进来，不各算一遍）。 */
export interface RoomHeadlineInput {
  /**
   * 房间种类：只用来回答"标题是不是已经是对方的名字"（`direct` / `human_direct` 是）。
   * 契约里它可能存在也可能缺（演示通道 / 老载荷）—— 缺了按"不是 1:1"处理：多写一个名字
   * 比把群房当成 1:1 房**少错一格**（后者会把"谁在线"写成一句对整间房都不成立的断言）。
   */
  kind?: string
  /** 成员（**不含我也行** —— 本函数按 `membersWithSelf` 补上）。 */
  members: readonly RollMember[]
  /** 我那一行（`selfMember(me)`；没登录 ⇒ `null`）。 */
  self: RollMember | null | undefined
  /** 成员表在场吗（`membersByRoom` 非空 ⇒ 真）。不在场 ⇒ 人数只是"已知"。 */
  authoritative: boolean
  /** 主体进行态快照（不在场 ⇒ 读不到，此时**不提**在线，更不写"不在线"）。 */
  snapshot: RoomLiveSnapshot | undefined
}

/**
 * 头部那一行（头部与房间信息**共用这一句**，别在组件里各写一份）。
 *
 * 三支见文件头：至多一个对端 ⇒ 名字 ＋ 状态；不止一个 ⇒ 人数 ＋ 在线数；只有我 ⇒ 「只有你」。
 */
export function roomHeadline({
  kind,
  members,
  self,
  authoritative,
  snapshot,
}: RoomHeadlineInput): string {
  const all = membersWithSelf(members, self)
  const myId = self?.id
  const others = all.filter((m) => m.id !== myId)
  const titleIsPeer = kind === 'direct' || kind === 'human_direct'

  // 只有我（或成员表还没到）：不报数，也不假装"就我一个人"。
  if (others.length === 0) return authoritative ? '只有你' : '成员还没读到'

  if (others.length === 1) {
    const peer = others[0]
    const state = presenceLabel(peerPresence(snapshot, peer.id))
    // 1:1 房的标题就是对方的名字 ⇒ 再写一遍名字是同一件事画两遍。
    return titleIsPeer ? state : `${peer.displayName} · ${state}`
  }

  const head = authoritative ? '' : '已知 '
  const online = liveHeadline(snapshot, myId).onlineCount
  // 没有别人在线（或读不到进行态）时那一格整个不出现：读不到就不提，不写"0 位在线"。
  return online > 0 ? `${head}${all.length} 位成员 · ${online} 位在线` : `${head}${all.length} 位成员`
}
