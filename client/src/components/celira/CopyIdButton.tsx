import { useEffect, useRef, useState } from 'react'
import { Copy } from 'lucide-react'
import { cn } from '@/lib/utils'

/**
 * **复制一串身份 ID**（`/app/profile` 与联系人资料卡共用这一份）。
 *
 * 为什么必须是个真按钮：这一页的世界里没有目录，加一个人/一支 Agent 的唯一做法是
 * **对方把这串 ID 给你**。所以"复制我的 ID"是那条握手的另一半 —— 它从前是两颗
 * `Copy` 图标（`ProfilePage`、`ContactDetailCard` 各一颗），**没有 onClick**：看着能按，
 * 按了什么都不发生（"让用户做一个假的决定，比显示假事实更糟"同一条纪律）。
 *
 * 两条判据：
 * - **成功才说成功**：剪贴板写不进去（浏览器不给权限 / 不是安全上下文）时如实说
 *   「没能复制」并给出口（手动选中那串 code），不假装复制好了 —— 与聊天页批量复制同一句；
 * - **一句事实一个位置**：两处的图标与文案都在这一个组件里，不再各写一遍。
 */
export function CopyIdButton({ value, className }: { value: string; className?: string }) {
  const [state, setState] = useState<'idle' | 'copied' | 'failed'>('idle')
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    return () => {
      if (timer.current) clearTimeout(timer.current)
    }
  }, [])

  const copy = async () => {
    if (timer.current) clearTimeout(timer.current)
    try {
      if (!navigator.clipboard) throw new Error('no clipboard')
      await navigator.clipboard.writeText(value)
      setState('copied')
    } catch {
      setState('failed')
    }
    // 提示只亮一会儿：它是"刚才那一下的结果"，不是这一格的常驻读数。
    timer.current = setTimeout(() => setState('idle'), 2000)
  }

  // 还没有 id 的地方**不摆按钮**（身份没就绪时复制一串空串，是替一个不存在的事实做出口）。
  if (!value) return null

  return (
    <span className={cn('inline-flex items-center gap-1.5', className)}>
      <button
        type="button"
        aria-label="复制身份 ID"
        title="复制身份 ID"
        onClick={() => void copy()}
        className="cursor-pointer text-muted-foreground transition-colors hover:text-foreground"
      >
        <Copy className="size-3" aria-hidden />
      </button>
      {state !== 'idle' && (
        <span
          role="status"
          className={cn(
            'text-micro',
            state === 'failed' ? 'text-presence-failed' : 'text-muted-foreground',
          )}
        >
          {state === 'failed' ? '没能复制（浏览器没给剪贴板权限）' : '已复制'}
        </span>
      )}
    </span>
  )
}
