// 「我」的 participant id 归属纯逻辑（LOCAL_CLIENT_HOST.md Slice 2）。
//
// 本模块只含可独立测试的纯函数：给定当前连接身份（core 直连 `/v1/me`
// 返回的真实 Human participant id），判定一条消息是否属于「我」以及失败的
// 自己的 pending 是否可重试。无 React、无 DOM、无持久化依赖。
//
// 原则：带真实连接身份的模式（core 直连）优先使用当前认证 Human id；fixtures
// 的 SELF_ID 只在没有连接身份时兜底（Mock/Gallery/开发路径），绝不把 fixture
// 身份当作真实连接的自我归属。

import { SELF_ID } from '../fixtures/data.ts'
import { fixtureFallbackAllowed } from './participantDirectory.ts'
import type { Message } from './domain'

/**
 * 当前连接身份下的「我」participant id。
 * 传入真实认证 Human id 时返回它；空/缺失时**只在不属于真实世界时**回退 fixtures
 * SELF_ID（演示/Gallery/开发路径）。
 *
 * TASK_56：原实现无条件回退 SELF_ID —— 于是在**真实世界里**（core 直连）身份还没
 * 读出来 / 读失败时，会把 fixture 的 `p-inori` 当成"我"：
 * 自己的消息认不出、别人的消息被当成自己发的、直聊房按一个假 id 去找。
 * 真实世界下这里返回**空串** = 诚实地说"还不知道我是谁"，判定自然全部为假
 * （`'' === authorId` 不成立）——宁可不认，也不冒名。
 */
export function resolveSelfId(authenticatedHumanId?: string | null): string {
  if (typeof authenticatedHumanId === 'string' && authenticatedHumanId.length > 0) {
    return authenticatedHumanId
  }
  return fixtureFallbackAllowed() ? SELF_ID : ''
}

/** 消息 authorId 是否为当前连接身份（我）。 */
export function isSelfMessage(authorId: string, selfId?: string | null): boolean {
  return resolveSelfId(selfId) === authorId
}

/**
 * SystemMessage 撤回文案的「作者名 + 是否自己」纯逻辑（Slice 4 fail-closed）。
 *
 * - `resolveAuthorName`（真实 resolver，core 连接上下文）存在时：作者名只
 *   来自 resolver，未知作者显示中性「参与者」，**绝不回退 fixtures**（防 ID 碰撞
 *   把 fixture 身份当真实作者）；且 `selfId` 缺失时绝不当成「自己」。
 * - `resolveAuthorName` 缺省：只有在**不属于真实世界**时（演示/Gallery 通道）
 *   才允许 fixtures 回退作者名与 SELF_ID —— 保持既有展示行为。**真实世界下
 *   「没传 resolver」不能变成"那就用假作者"**（TASK_56：这条原先只靠调用方自觉）。
 */
export function systemMessageIdentity(
  authorId: string,
  selfId: string | undefined,
  resolveAuthorName: ((authorId: string) => string | undefined) | undefined,
  fixturesFallback: (authorId: string) => { displayName?: string } | null | undefined,
): { authorName: string; isSelfAuthor: boolean } {
  const useFixtures = resolveAuthorName == null && fixtureFallbackAllowed()
  if (useFixtures) {
    return {
      authorName: fixturesFallback(authorId)?.displayName ?? '参与者',
      isSelfAuthor: resolveSelfId(selfId) === authorId,
    }
  }
  return {
    authorName: resolveAuthorName?.(authorId) ?? '参与者',
    isSelfAuthor: typeof selfId === 'string' && selfId.length > 0 && selfId === authorId,
  }
}

/** 发送失败且属于「我」的消息（失败 pending 的可重试判定，含重试按钮）。 */
export function isRetriableSelfFailure(message: Message, selfId?: string | null): boolean {
  return isSelfMessage(message.authorId, selfId) && message.delivery === 'failed'
}

/**
 * 新消息到达后是否应自动跟随滚动到底部（ChatPage 滚动跟随策略）。
 * 「我」的消息按当前连接身份判定，无论当前位位置都滚到底；
 * 对方消息只在已接近底部时跟随，避免打断向上翻阅旧消息。
 */
export function shouldFollowToBottom(
  authorId: string,
  selfId: string | null | undefined,
  isNearBottom: boolean,
): boolean {
  return isSelfMessage(authorId, selfId) || isNearBottom
}

/**
 * **离底部近吗** —— `ChatPage` 的 `isNearBottom` 的判据本体（抽出来是因为它是**可判错的一句**）。
 *
 * ## 为什么它值一个函数 ＋ 用例（2026-10-05 主人真机反馈）
 *
 * 原先这一格写死在页面里、阈值 **64px**。对方回一段长话就把人推出 64px 之外 ⇒ 新消息到达时
 * **既不跟随滚动、也不推进读游标**（页面把这两件事写在同一个 `if` 里）⇒ 人明明在房里看着那条
 * 消息，未读却一直挂着、那颗「有 N 条新消息」也不消失，非要用鼠标去会话列表里点一下才清。
 *
 * ## 判据（主流做法）
 *
 * **一屏之内都算"在底部"**（Slack / Telegram 同款）：人还在读最近的内容 ⇒ 跟着走；
 * 翻到很上面看历史 ⇒ 不打断他，只亮那颗提示。阈值取 `max(64, 视口高度)`：视口很小
 * （或动态行高还没测出来）时至少留 64px 的宽容，视口正常时就是"一屏"。
 */
export function isNearBottomOf(input: {
  scrollHeight: number
  scrollTop: number
  clientHeight: number
}): boolean {
  const { scrollHeight, scrollTop, clientHeight } = input
  const slack = Math.max(64, clientHeight)
  return scrollHeight - scrollTop - clientHeight < slack
}

/**
 * **这一刻该不该把这一间标成读过**（新内容落位之后问一次）。
 *
 * ## 为什么它值一个函数 ＋ 用例（2026-10-14 主人真机反馈）
 *
 * 症状：人明明把那条消息读完了（消息尾巴和输入框都在屏上），「有 N 条新消息」那颗按钮
 * 和会话列表上的角标**一直挂着**，非要用鼠标去列表里点一下才清。
 *
 * 病根不在判据，在**什么时候问**：读游标原先只在三处推进（换房 / 新消息到达 / 点那颗按钮），
 * 而那两个滚动监听都写成"**只在过渡里**上报"——`readOnBottomRef` 是**滚动那一刻**拍的快照。
 * 长消息把底部推走是**布局**变化、**不发滚动事件** ⇒ 快照一直停在 `true`（其实已经不在底部），
 * 等人自己滚回底部时 `atBottom === ref` 被当成"没变化"**提前返回** ⇒ 游标永远不推进。
 *
 * ## 判据
 *
 * 两只眼各看一半，缺一不可：
 * - `nearBottom`：**现在**到底了吗（`isNearBottomOf`，落位之后重算，不是滚动那一刻的快照）；
 * - `unreadCount`：这一间**真有未读**吗（> 0 才动作 ⇒ 没有未读时一个请求都不发，不把世界问烦）。
 */
export function shouldMarkReadOnArrival(input: {
  nearBottom: boolean
  unreadCount: number
}): boolean {
  return input.nearBottom && input.unreadCount > 0
}

/**
 * ★ **进会话该落在哪一行**（2026-10-16 主人真机反馈："每次我进入会话的时候，默认不是聚焦在最新的那种"）。
 *
 * ## 两档，判据只有一条
 *
 * - **有「以下是新消息」那条分隔线** ⇒ 落在**分隔线**上（微信语义，2026-10-05 主人定过）——
 *   人一进来就知道"从这儿往后是新的"；
 * - **没有**（这一间全读过了）⇒ 落在**最后一条**上（`align: 'end'`）。
 *
 * ⚠ 从前这一档**没有走虚拟列表的落法**：页面直接 `viewport.scrollTo(top: scrollHeight)`，
 * 而虚拟列表的行高是**估值**、边测边长 ⇒ 滚一次就被后面的增长甩在后面，人停在**历史里** ✗
 * （这就是"进来不是最新那种"的机械原因）。所以落点必须走 `scrollToIndex`，而且**索引由这里算**。
 *
 * ⚠ 返回 `null` = 这一间**没得落**（还没消息）—— 调用方什么都不做，**不许**当成"落到第 0 行"。
 */
export function landingPlanFor(input: {
  /** 「以下是新消息」那一行的下标（`-1` = 没有）。 */
  unreadIndex: number
  /** 这一间现在有几行。 */
  rowCount: number
}): { kind: 'divider' | 'latest'; index: number } | null {
  if (input.rowCount <= 0) return null
  if (input.unreadIndex >= 0 && input.unreadIndex < input.rowCount) {
    return { kind: 'divider', index: input.unreadIndex }
  }
  return { kind: 'latest', index: input.rowCount - 1 }
}
