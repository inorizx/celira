/**
 * **通知门控的唯一裁决面**（2026-10-23 立）。
 *
 * ## 为什么把它抽成纯函数
 *
 * 从前这条判据是一串写在 `store.ts` 的 `maybeNotify` 里的 `if`：全局开关、提及偏好、
 * 系统消息、作者是不是我、房间免打扰、结构化提及 —— 六件事挤在一处，**每加一档提醒
 * （这次是服务端的 `attention`）就得在那串 `if` 中间插一句**，而它的正确顺序只存在于
 * 作者的脑子里。抽出来之后：
 *
 * - **顺序**有了唯一实现（下面 §优先级 的四条），`node --test` 直测；
 * - `store.ts` 只负责**凑事实**（读偏好、查房间、判提及）并调用本函数，不再自己裁决；
 * - 界面里那句标题（「提及了你」）仍由调用方按 `mentionedMe` 决定 —— 门控只管
 *   **要不要弹**，不管弹什么。
 *
 * ## 优先级（自上而下，先命中先返回）
 *
 * 1. **两个开关都关** ⇒ 不弹（`enabled=false && mentions=false`：用户明确不想被打扰）；
 * 2. **系统消息 / 我自己的消息** ⇒ 不弹（撤回 tombstone 与自己的发送不是"新消息"）；
 * 3. **本机「消息免打扰」** ⇒ 不弹，**且压过一切**（含 `attention=all`、含 @ 我）；
 *    它是本机的"完全静音"（`api/roomPrefs.ts`），语义就是"这间房别吵我"；
 * 4. **服务端提醒档 `mentions`** ⇒ 只有**结构化 mentions 含我**才弹
 *    （`attention=all` / 缺省则照常往下走）；
 * 5. 最后按偏好收口：总开关开 ⇒ 弹；只开了"提及提醒" ⇒ 只有 @ 我时才弹。
 *
 * ## 两件不许混淆的事
 *
 * - `attention` 是**服务端事实**（换设备跟着走）；`muted` 是**本机偏好**。两者都在场时
 *   `muted` 优先 —— 这正是第 3 条排在第 4 条前面的原因。
 * - 这一档**不改消息主链**（契约 §9.1）：不弹通知 ≠ 收不到消息，未读与历史照旧。
 *   所以本函数只回答"弹不弹"，绝不参与"这条消息要不要入库 / 算不算未读"。
 */

import type { RoomAttention } from './domain'

/** 一次通知裁决要的全部事实（都是调用方手里现成的，函数本身零依赖）。 */
export interface NotificationFacts {
  /** 机器级偏好：新消息通知总开关。 */
  notificationsOn: boolean
  /** 机器级偏好：仅 @ 我的消息也提醒（总开关关闭时仍可生效）。 */
  mentionsOnly: boolean
  /** 本机偏好：这间房完全免打扰（`api/roomPrefs.ts` 的 `muted`）。 */
  muted: boolean
  /**
   * 服务端提醒档（`Room.attention`；`undefined` = 还不知道 / 缺行，按 `all` 处理
   * —— 与 `parseRoomAttention` 同一条纪律：宁可多响一声，不静默吞掉）。
   */
  attention?: RoomAttention
  /** 这条是不是系统消息（撤回 tombstone 等）。 */
  system: boolean
  /** 作者是不是我自己。 */
  fromSelf: boolean
  /** 这条消息的**结构化** `mentions` 里有没有我的 participant id。 */
  mentionedMe: boolean
}

/** 这条消息该不该弹通知。**纯函数**：不读存储、不看时间、不改任何东西。 */
export function shouldNotifyMessage(facts: NotificationFacts): boolean {
  // ① 两个开关都关：用户没要任何提醒。
  if (!facts.notificationsOn && !facts.mentionsOnly) return false
  // ② 系统消息 / 自己发的：不是"新消息"。
  if (facts.system || facts.fromSelf) return false
  // ③ 本机完全静音压过服务端提醒档（含 @ 我）。见文件头"两件不许混淆的事"。
  if (facts.muted) return false
  // ④ 服务端档 =「只接受 @」：不是结构化提及就到此为止。
  if (facts.attention === 'mentions' && !facts.mentionedMe) return false
  // ⑤ 偏好收口：总开关开 ⇒ 弹；否则只有"提及提醒"开着时 @ 我才弹。
  if (facts.notificationsOn) return true
  return facts.mentionsOnly && facts.mentionedMe
}
