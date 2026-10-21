import { useEffect, useRef, useState } from 'react'
import { Check, Copy } from 'lucide-react'
import { cn } from '@/lib/utils'

/**
 * **一行要你在终端里跑的命令**（等宽字 ＋ 复制按钮）。
 *
 * 为什么要有它：`celira-im create-human --display-name "…"`、`node home.js up` 这类串从前
 * 混在正文句子里（与界面文案同一字重、同一行），读起来像文档，抄起来要手工选词。
 * 判据：**命令是"要执行的东西"，不是"要读的东西"** —— 给它自己的块、自己的字族、自己的出口。
 *
 * 两条与 `CopyIdButton` 同源的纪律：
 * - **成功才说成功**：剪贴板写不进去（不是安全上下文 / 没权限）时如实说「没能复制」并给出口
 *   （手动选中那串字），不假装复制好了；
 * - **空命令不摆按钮**：没有可执行的东西就不该有一个"看起来能按"的出口。
 */
export function CommandLine({
  command,
  note,
  className,
}: {
  /** 整行原样复制的那一串（不加提示符、不加换行）。 */
  command: string
  /** 这行命令是干什么的（一句话，写在块下面）。 */
  note?: string
  className?: string
}) {
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
      await navigator.clipboard.writeText(command)
      setState('copied')
    } catch {
      setState('failed')
    }
    // 提示只亮一会儿：它是"刚才那一下的结果"，不是这一格的常驻读数。
    timer.current = setTimeout(() => setState('idle'), 2000)
  }

  if (!command) return null

  return (
    <div className={cn('space-y-1', className)}>
      <div className="flex items-start gap-2 rounded-lg border border-border bg-muted px-3 py-2">
        {/* 换行两条都要：`break-keep`（`word-break: keep-all`）不许在 `"你的名字"` 这种
            CJK 中间断行（中文默认逐字可断）；`break-words` 兜住万一有超长无空格串。
            登录框只有 384px 宽，这一行一定会折，断点只能落在空格上。 */}
        <code className="min-w-0 flex-1 font-mono text-caption break-keep break-words">{command}</code>
        <button
          type="button"
          aria-label="复制这行命令"
          title="复制这行命令"
          onClick={() => void copy()}
          className="mt-0.5 shrink-0 cursor-pointer text-muted-foreground transition-colors hover:text-foreground"
        >
          {state === 'copied' ? (
            <Check className="size-3.5 text-presence-online" aria-hidden />
          ) : (
            <Copy className="size-3.5" aria-hidden />
          )}
        </button>
        <span role="status" className="sr-only">
          {state === 'copied' ? '已复制' : state === 'failed' ? '没能复制' : ''}
        </span>
      </div>
      {state === 'failed' && (
        <p className="text-micro text-presence-failed">没能复制（浏览器没给剪贴板权限），手动选中上面那串。</p>
      )}
      {note && <p className="text-micro text-muted-foreground">{note}</p>}
    </div>
  )
}
