// ⚠ 本模块**要被 `node --test` 直接测**（`tests/message-selection.test.ts`）⇒ 相对路径 ＋
//   带 `.ts` 后缀（Node 的 ESM 解析器不认 `@/` 别名、也不认省略后缀的相对路径）。
import type { Message } from '../api/domain.ts'

/**
 * **多选**（一批消息的选择状态与批量动作的纯逻辑，2026-10-08 刀）。
 *
 * ## 为什么要有这一层
 *
 * 批量动作里最容易错、也最看不出来的一件事是**顺序**：用户可能先点第 3 条、再点第 1 条，
 * 而"转发/复制出去的那一份"必须按**时间线**排（先发生的在前）—— 按点击顺序发出去，
 * 对面读到的就是一段被打乱的对话，而且**没人会当场发现**。
 *
 * 所以判据收在这里：选择状态是一串 id，**要动那批消息时一律回时间线里取**
 * （[`selectedInOrder`]），调用方不许自己 `map` 一遍。
 */

/** 点一下：已选 ⇒ 取消；没选 ⇒ 选上（幂等，且不动原数组）。 */
export function toggleSelected(ids: readonly string[], id: string): string[] {
  if (!id) return [...ids]
  return ids.includes(id) ? ids.filter((each) => each !== id) : [...ids, id]
}

/** 这条消息在选中集合里吗。 */
export function isSelected(ids: readonly string[], id: string): boolean {
  return Boolean(id) && ids.includes(id)
}

/**
 * 把选中集合**按时间线**取出来（这是本模块存在的理由，见文件头）。
 *
 * 时间线里已经没有的那些 id 会被自然丢掉（比如那条消息刚被撤回成系统行、
 * 或用户切了房间）——**不许**因此报错，也不许凭空造一条出来。
 */
export function selectedInOrder(messages: readonly Message[], ids: readonly string[]): Message[] {
  if (ids.length === 0) return []
  const wanted = new Set(ids)
  return messages.filter((each) => wanted.has(each.id))
}

/**
 * 批量复制的文本：**按时间线**取，逐条正文用换行连接，**跳过空正文**
 * （纯附件消息没有正文；连着两个换行看起来像有个空消息）。
 */
export function copyTextOf(messages: readonly Message[]): string {
  return messages
    .map((each) => (typeof each.body === 'string' ? each.body : ''))
    .filter((each) => each !== '')
    .join('\n')
}
