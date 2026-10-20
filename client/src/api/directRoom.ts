/**
 * **「去跟它说话」要进的那一间房**（2026-10-20 T4）。
 *
 * ## 它治的是什么
 *
 * 「跟它说话」这件事今天有两处入口（世界侧的参与者页 `/agent/:id` 的「发消息」、以及这里要加的
 * Agent 主页首屏），而**判据**只有一条：**已经有那间直聊房就进它，没有才建**。把这条判据抄成
 * 两份，两页迟早说不同的话（一处先建了房、另一处还按旧列表找不到，于是又建一间 —— 而契约里
 * 「同一 Human–Agent 对**至多一间** `direct` 房」是硬约束，重复建会撞 409）。
 *
 * ⇒ 判据住在这里（纯函数、有 `tests/direct-room.test.ts`）；**动作**仍旧只有一处：
 * `store` 的 `createDirectRoom`（本模块不建任何东西、不发任何请求）。
 *
 * ## 判据（**不看 `kind`**，这一条是踩出来的）
 *
 * 只看两件事：**房里正好两个人**，且**这两个人正是我和你**。
 * 不看 `kind` 是因为人↔人那间房在契约里叫 `human_direct`（`CORE_V1_IM_CONTRACT.md` 的房间类型），
 * 而"跟这个人有没有那间两人房"这个问题的答案**两族同形** —— 认 `kind === 'direct'` 会让
 * 人↔人那一支永远找不到房、然后每次点都去建一间。
 *
 * ⚠ **读不到就返回 `null`**（`selfId` 还没解析出来 / 没有房列表）：调用方据此**去建**，
 * 而建这条动作自己是幂等的（契约保证至多一间）⇒ 最坏情况是一次多余的请求，不是两条事实。
 */

/** 这一层只看得到的两格（房 id 与会者 id 表；`Room` 的其余格子与判据无关）。 */
export interface DirectRoomView {
  id: string
  participantIds: string[]
}

/**
 * 找那间「我和它」的两人房；没有 ⇒ `null`。
 *
 * 边界：`selfId` / `peerId` 任一为空（我还没解析出来 / 地址里没有它）⇒ `null` ——
 * 那时"哪间是我们的房"这个问题**问不出来**，不许拿列表里第一间两人房顶上去。
 */
export function directRoomOf<T extends DirectRoomView>(
  rooms: readonly T[],
  selfId: string,
  peerId: string,
): T | null {
  if (!selfId || !peerId || selfId === peerId) return null
  return (
    rooms.find(
      (room) =>
        room.participantIds.length === 2 &&
        room.participantIds.includes(selfId) &&
        room.participantIds.includes(peerId),
    ) ?? null
  )
}
