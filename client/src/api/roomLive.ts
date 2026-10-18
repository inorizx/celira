// 主体进行态的**唯一裁决面**（客户端侧）：从 `GET /v1/rooms/{id}/live` 的
// 快照里派生出界面要显示的那两句话与那几个点。
//
// 契约：`docs/contracts/core-v1/CORE_V1_AGENT_PROTOCOL.md` §6。
// **只读派生、无副作用、无 React、无 store**——所以能 `node --test` 直测
// （见 `tests/room-live.test.ts`）。页面与组件只消费这里的函数，不各自
// 再写一遍"怎么算在线、怎么算在写"。
//
// 三条纪律（照抄契约，避免客户端自己发明）：
// 1. **执行过程不进 IM**：这里没有阶段、没有草稿、没有生成流——快照里
//    本来就没有这些字段（2026-09-28 主人裁决「不需要看到执行过程」）；
// 2. **人机同形**：`kind` 只用于呈现，不参与任何判据；
// 3. **易失投影**：读不到就说读不到（返回空/undefined），**绝不**把它当成
//    "它离线了"或"它没在写"——那是把"没读到"读成"确认没有"。

import type { RoomLiveMember, RoomLiveSnapshot } from './contracts'

/** 快照里某位成员那一行（没有就是 `undefined`——**不猜**）。 */
export function liveMemberOf(
  snapshot: RoomLiveSnapshot | undefined,
  participantId: string,
): RoomLiveMember | undefined {
  // `members` 也走可选链：契约说它恒在，但**收到的形状不对时不许炸**——
  // 客户端对陌生载荷的唯一正确反应是"读不到"，不是白屏。
  return snapshot?.members?.find((m) => m.participant_id === participantId)
}

/** 该成员此刻是否在线（快照里没有这个人 ⇒ `false`，即"读不到就不点亮"）。 */
export function isOnline(
  snapshot: RoomLiveSnapshot | undefined,
  participantId: string,
): boolean {
  return liveMemberOf(snapshot, participantId)?.online === true
}

/** 该成员此刻是否正在写（同上：读不到就不亮）。 */
export function isTyping(
  snapshot: RoomLiveSnapshot | undefined,
  participantId: string,
): boolean {
  return liveMemberOf(snapshot, participantId)?.typing === true
}

/** 除我之外的成员——"别人在不在"才是我要看的，我自己那行对自己没有信息量。 */
export function livePeers(
  snapshot: RoomLiveSnapshot | undefined,
  myId: string | undefined,
): RoomLiveMember[] {
  return (snapshot?.members ?? []).filter((m) => m.participant_id !== myId)
}

/** 头部那一行的两个读数：`N 在线` 与 `某某、某某 正在写…`。 */
export function liveHeadline(
  snapshot: RoomLiveSnapshot | undefined,
  myId: string | undefined,
): { onlineCount: number; typingNames: string[] } {
  const peers = livePeers(snapshot, myId)
  return {
    onlineCount: peers.filter((m) => m.online).length,
    typingNames: peers.filter((m) => m.typing).map((m) => m.display_name),
  }
}
