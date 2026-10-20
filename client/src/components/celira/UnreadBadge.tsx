import { cn } from '@/lib/utils'

/**
 * **未读朵数**（2026-10-20 收成一处）。
 *
 * 三处调用原先各写一遍 `h-[18px] min-w-[18px] …`（`AppShell` 侧栏一处在图标右上、移动端底栏
 * 一处在图标右上、`ConversationItem` 一处在行尾）—— 想让它们一起变，得改三个文件。
 * 这就是"一改全局"做不到的样子，所以抽成组件：**尺寸只在这个文件里**。
 *
 * 两档而已，别再加：
 * - `md` = 18px：侧栏与会话行（桌面）；
 * - `sm` = 16px：移动端底栏那一颗（图标小一号，朵数也得跟着小）。
 *
 * 数字一律 `99+` 封顶（`max` 可改）；`label` 允许调用方给现成字符串（`AppShell` 那边已经算好了
 * `unreadLabel`，含它自己的封顶逻辑 —— 别在这里再造一遍它的规则）。
 */
export function UnreadBadge({
  count,
  label,
  max = 99,
  size = 'md',
  className,
}: {
  /** 未读数（`label` 给了就不看这个）。 */
  count?: number
  /** 现成的文字（调用方已算好封顶）。数字也行：`AppShell` 那边是 `string | number`。 */
  label?: string | number
  /** 封顶阈值，默认 99 ⇒ "99+"。 */
  max?: number
  size?: 'sm' | 'md'
  /** 只放定位（`absolute -top-… `）这类，**不放尺寸**。 */
  className?: string
}) {
  const text =
    label !== undefined && label !== null && label !== ''
      ? String(label)
      : typeof count === 'number'
        ? count > max
          ? `${max}+`
          : String(count)
        : ''
  if (!text) return null
  return (
    <span
      data-slot="unread-badge"
      className={cn(
        'flex items-center justify-center rounded-full bg-primary px-1 text-micro text-primary-foreground',
        size === 'sm' ? 'h-4 min-w-4' : 'h-4.5 min-w-4.5',
        className,
      )}
    >
      {text}
    </span>
  )
}
