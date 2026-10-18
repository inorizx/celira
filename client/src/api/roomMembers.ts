// 房间成员投影（Room Member Views）的**唯一裁决面**。
//
// 「这个房间里有哪些成员」这件事，在客户端原先有**三份各写一遍**的实现，而且三份对
// 「解析不出来」的处理**互相矛盾**：
//   - `ChatHeader.tsx`：`core?.map(toParticipant) ?? room.participants ?? participantIds.map(getParticipant)`
//     —— 第三条用 `getParticipant`，**查不到就抛**（异常进渲染路径 = 白屏，TASK_56 治过的病）；
//   - `MessageInput.tsx`：同一条链，但第三条用 `tryGetParticipant` 静默跳过；
//   - `ChatPage.tsx`（「上次醒于」）：同一条链，第三条是 `tryGetParticipant(id)?.kind ?? 'human'`
//     —— 解析不出就**给它编一个种类**（把不知道是谁的 id 断言成人类）。
//
// 还有一处更隐蔽的不一致：core 成员投影**存在但是空数组**时，`ChatHeader` 把它当
// 「这个房没有成员」（`??` 不穿透空数组），另外两处当「还没加载」。同一个输入，三处三种答案。
//
// 本模块把这条链收成**一个纯函数 + 一条失败策略**：
//   ① 世界侧成员投影（权威，非空即用）→ ② 房间视图自带成员（direct 房的 peer）
//   → ③ 参与者目录（**唯一允许回落演示数据的一跳**，由 `fixtureFallbackAllowed()` 闸门决定）；
//   解析不出 ⇒ **跳过**：成员表里不列一个不知道是谁的人，更不给他编一个种类，也绝不抛。
//
// 纯函数、无 React、无 zustand、对 `fixtures/` 零依赖（兜底数据由 fixtures 自己登记进
// 目录，本模块只经闸门查询）⇒ `node --test` 直测（见 `tests/room-members.test.ts`）。

import type { Participant } from './domain'
// ⚠ 扩展名必须带 `.ts`：本模块会被 `node --test --experimental-strip-types` 直接加载
// （与 `api/store.ts` 的既有纪律一致），无扩展名的相对导入在 Node 的 ESM 解析下会找不到。
import { toParticipant } from './participantProjection.ts'
import { tryGetParticipant } from './participantDirectory.ts'

/**
 * 世界侧成员投影的形状子集（`membersByRoom[roomId]` 的元素：members 端点的成员行）。
 * 只声明本模块真正读的字段，不绑定 `contracts.ts` 的具体视图类型。
 */
export interface RoomMemberRow {
  participant: { id: string; kind: string; display_name: string }
}

/** 房间视图自带的成员来源（`Room` 的形状子集）。 */
export interface RoomMemberFallback {
  participants?: readonly Participant[] | undefined
  participantIds?: readonly string[] | undefined
}

export interface RoomMemberSources {
  /** 世界侧成员投影（`membersByRoom[roomId]`）。**缺省或空数组 = 还没加载**，不是「没有成员」。 */
  core?: readonly RoomMemberRow[] | undefined
  /** 房间视图自带的成员（core 直连下 direct 房是 peer，群房通常为空）。 */
  room?: RoomMemberFallback | null | undefined
}

/**
 * 房间成员视图：给出该房间**已知**的成员列表（顺序即来源顺序）。
 *
 * 判据顺序（前者覆盖后者）：
 *   ① `core` 非空 ⇒ 它就是权威（`toParticipant` 投影 + 统一窄化），**不再看后面的来源**；
 *   ② `room.participants` 非空 ⇒ 用它（core 直连下这是 direct 房的 peer）；
 *   ③ `room.participantIds` 逐个查参与者目录 —— 查不到就**跳过**（真实世界下目录闸门关闭，
 *      查不到就是查不到，绝不从演示数据里捞一个同名的顶上来）。
 *
 * 「空数组按未加载处理」是有意的：你能看见的房间至少有你一个成员，**空成员列表不可能是
 * 一条公共事实**，它只可能是"还没读到"。按事实处理会让一个尚未加载的群房显示成"0 名成员"，
 * 而按未加载处理会继续往下找，最终至少能落到目录里的「我」。
 */
export function roomMemberViews(sources: RoomMemberSources): Participant[] {
  const core = sources.core
  if (core && core.length > 0) return core.map((row) => toParticipant(row.participant))

  const roomParticipants = sources.room?.participants
  if (roomParticipants && roomParticipants.length > 0) return [...roomParticipants]

  return (sources.room?.participantIds ?? []).flatMap((id) => {
    const found = tryGetParticipant(id)
    return found ? [found] : []
  })
}
