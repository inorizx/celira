/**
 * **提醒档（attention）在客户端的唯一裁决面**（2026-10-23 立）。
 *
 * ## 它是什么、归谁
 *
 * 契约 `CORE_V1_IM_CONTRACT.md` §9.1：群房里「这间房要不要提醒我」是**个人事实** ——
 * 只有本人能写、只有本人可见、换设备跟着走。`all` = 普通群消息也提醒；`mentions` = 只接受
 * **结构化** @（`message.mentions` 含我的 id）。⇒ 它**不是**本机偏好：
 *
 * | 这件事 | 事实所有者 | 住哪 |
 * |---|---|---|
 * | 置顶 / 消息免打扰（完全静音） | **本机**（我的看法，对方看不见） | `api/roomPrefs.ts` |
 * | **提醒档 `all` / `mentions`** | **世界**（im 服务端的个人事实） | Core `PUT /v1/rooms/{id}/attention`，投影在本模块 |
 *
 * 两者同时在场时 **`muted` 优先**（本机完全静音压过服务端档）—— 判据不在本模块，
 * 在 `api/notificationGate.ts`（通知门控的唯一一处）。
 *
 * ## 纪律
 *
 * - **宽容解析**：缺字段 / `null` / 读不懂一律归 `all`（契约：缺行投影成 `all`），绝不抛；
 * - **纯逻辑**：无 React、无 zustand、无 fixtures、无网络。可被 `node --test` 的类型剥离
 *   模式直测（`tests/room-attention.test.ts`）；
 * - **投影不改入参**：`withRoomAttention` 返回新数组、新对象，值没变时原样返回同一个对象
 *   （避免无意义的引用变化引发重渲染 —— 与 `roomPrefs.applyRoomPrefs` 同一条纪律）。
 *
 * ⚠ 这里**没有** `setRoomAttention` 的网络实现：写入路径是「store 的乐观更新 +
 * `RoomApi.setRoomAttention`（core 的 PUT）」。本模块只回答「这个值是什么」「开关该画成
 * 什么样」「投影后房间列表长什么样」。
 */

import type { RoomAttention } from './domain'

/**
 * 缺行 / 缺字段的投影值（契约 §9.1：不预建行，缺行 = `all`）。
 *
 * ⚠ 它是**契约语义**，不是"默认偏好"：服务端没写过这一行时，提醒行为就是普通的
 * 「群消息也提醒」，客户端不许把它当"未知"去猜第三种状态。
 */
export const DEFAULT_ROOM_ATTENTION: RoomAttention = 'all'

/** 这个值是不是契约里的两个档之一（其余一律不算，含大小写变体）。 */
export function isRoomAttention(value: unknown): value is RoomAttention {
  return value === 'all' || value === 'mentions'
}

/**
 * 线协议 `attention`（`string`）→ 视图模型的档。**宽容、fail-soft**：
 * 只认契约原样的两个字面量，其余（缺字段 / `null` / 空串 / 大写变体 / 数字）一律归 `all`。
 *
 * 为什么不"猜"成 `mentions`：猜错的方向决定后果 —— 猜成 `mentions` 会**静默吞掉**本该
 * 提醒的群消息，猜成 `all` 最多多提醒一次（用户能再点一下）。宁可多响一声。
 */
export function parseRoomAttention(value: unknown): RoomAttention {
  return isRoomAttention(value) ? value : DEFAULT_ROOM_ATTENTION
}

/** 开关的 `checked` 投影：只有 `mentions` 是开。 */
export function attentionSwitchChecked(attention: RoomAttention | undefined): boolean {
  return attention === 'mentions'
}

/** 开关拨动 → 要写的档：开 = `mentions`（只接受 @），关 = `all`。 */
export function attentionForChecked(checked: boolean): RoomAttention {
  return checked ? 'mentions' : 'all'
}

/**
 * 把某一间房的档投影到房间列表上（**纯函数**，不改入参）。
 *
 * 两处调用（同一个判据，不许各写一遍）：
 * ① store 的 `setRoomAttention` **乐观**先落一次；
 * ② 失败回退再落一次（回原值）、成功回读再落一次（服务端权威值）。
 *
 * 值没变时**返回同一个对象**；一间都没变时**返回原数组**（不制造无谓的重渲染）。
 */
export function withRoomAttention<T extends { id: string; attention?: RoomAttention }>(
  rooms: readonly T[],
  roomId: string,
  level: RoomAttention,
): T[] {
  let changed = false
  const next = rooms.map((room) => {
    if (room.id !== roomId || room.attention === level) return room
    changed = true
    return { ...room, attention: level }
  })
  return changed ? next : (rooms as T[])
}
