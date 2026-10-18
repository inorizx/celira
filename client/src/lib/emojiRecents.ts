// 「最近使用」表情：纯客户端事实（不进任何公共契约），最多 16 个、最近使用在前；
// 损坏数据一律降级为空。
//
// ## 作用域：**机器级**
//
// 判据（`api/worldIdentity.ts` 文件头）：**换一个世界连，这个键该不该跟着变？**
// 表情的"最近使用"是**这台机器上这个人**的习惯，跟连的是哪个世界无关 ⇒ 机器级。
//
// ## 存储走 `api/storage.ts`（2026-09-29 收口）
//
// 本模块不再直接碰 `localStorage`：那样会让"能不能存东西"跟浏览器绑死，
// 第二个壳（桌面/手机 app）来了就得改这里。现在**换平台只换 `storage` 的实现**。
//
// 接口因此是**异步**的——`localStorage` 同步、文件不同步，这条缝必须在第一天就是异步的。
//
// ⚠ **有意的行为变化**：从前的函数是同步的。改成异步之后，`EmojiPicker` 那一侧
// 必须在 `await` 之后再 `setState`（否则竞态下会把"刚记录的那一个"覆盖掉）。
import { machineScopedKey } from '@/api/worldIdentity'
import { storage } from '@/api/storage'

const KEY = machineScopedKey('recent-emoji')
const MAX = 16

export async function loadRecentEmoji(): Promise<string[]> {
  const raw = await storage.read(KEY)
  if (!raw) return []
  try {
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed)) return []
    return parsed.filter((e): e is string => typeof e === 'string').slice(0, MAX)
  } catch {
    return []
  }
}

/**
 * 记一个表情，返回**新的最近列表**（调用方拿它更新界面）。
 *
 * 写失败**不抛**：存储不可用（隐私模式、配额满）时只影响"下次开应用还记得"，
 * 本次会话内由调用方的 state 保证 —— 为它崩掉一个表情面板是不值当的。
 */
export async function recordRecentEmoji(emoji: string): Promise<string[]> {
  const next = [emoji, ...(await loadRecentEmoji()).filter((e) => e !== emoji)].slice(0, MAX)
  await storage.write(KEY, JSON.stringify(next))
  return next
}
