import { useEffect, useState } from 'react'
import { Copy, Minus, Square, X } from 'lucide-react'
import { CeliraLogo } from '@/components/celira/CeliraLogo'
import { isTauriRuntime } from '@/api/shell'
import { cn } from '@/lib/utils'

/**
 * **壳自己的标题栏**（2026-10-17 主人："为啥别人的 app 是类似自己的样式主题那种，
 * 我们这个是系统默认的吗" —— 然后贴了一张网易云：品牌在左、窗口按钮也是自己的样式）。
 *
 * ## 为什么是自绘而不是"折中"
 *
 * 先查过原文：这版 Tauri（2.10 的 `tauri-utils`）里 `TitleBarStyle` 的文档头一句就是
 * **"How the window title bar should be displayed on macOS"** —— 那个 overlay 折中
 * **只对 macOS 生效** ✗ ⇒ Windows 上只有两条路：原生边框（系统默认长相 ✗）或
 * `decorations: false` ＋ 自己画一条 ✓。选了后者。
 *
 * ## 三条纪律
 *
 * 1. **只在壳里渲染**（`isTauriRuntime()`，判据是壳的内部注入）：浏览器里的 web 版
 *    **一行布局都不变**（这里返回 null）✓ —— 同一份产物两个环境，不为谁单独构建；
 * 2. **拖动区必须有 `data-tauri-drag-region`**：去掉它窗口就拖不动了 ✗（属性**不继承**
 *    ⇒ 空白处、品牌那一块都要各自带上）；双击该区域 = 最大化/还原（Tauri 自己处理 ✓）；
 * 3. **关窗按钮走 `close()`**：与系统那条路同一条 —— 壳里 `CloseRequested` 是
 *    "收进托盘"（`lib.rs` 里 `prevent_close` + `hide`）⇒ 这里**不自己写隐藏逻辑**，
 *    否则两条路会分叉 ✗。
 *
 * ⚠ 如实记：自绘之后**右击标题栏弹系统菜单**没有了（原生边框才有）；贴边吸附/拖边缘改大小
 * 依赖 Tauri 对无边框窗口的处理，需要真机拖一下才算验过（我在报告里点名让人试）。
 */
export function TitleBar() {
  const [inShell] = useState(() => isTauriRuntime())
  const [maximized, setMaximized] = useState(false)

  useEffect(() => {
    if (!inShell) return
    let alive = true
    let unlisten: (() => void) | null = null
    void (async () => {
      const { getCurrentWindow } = await import('@tauri-apps/api/window')
      const win = getCurrentWindow()
      try {
        const now = await win.isMaximized()
        if (alive) setMaximized(now)
      } catch {
        /* 读不到就当"没最大化"（图标是读数，不是判据） */
      }
      const stop = await win.onResized(async () => {
        try {
          const now = await win.isMaximized()
          if (alive) setMaximized(now)
        } catch {
          /* 同上 */
        }
      })
      if (!alive) stop()
      else unlisten = stop
    })()
    return () => {
      alive = false
      unlisten?.()
    }
  }, [inShell])

  if (!inShell) return null

  const withWindow = async (action: (win: Awaited<ReturnType<typeof import('@tauri-apps/api/window')['getCurrentWindow']>>) => Promise<void>) => {
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    await action(getCurrentWindow()).catch(() => {})
  }

  const buttonClass =
    'flex size-8 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground'

  return (
    <header
      data-tauri-drag-region
      className="flex h-9 shrink-0 items-center gap-2 border-b border-border bg-card pr-1 pl-3 select-none"
    >
      <div data-tauri-drag-region className="flex items-center gap-2">
        <CeliraLogo className="size-5" />
        <span className="text-caption font-medium text-foreground">Celira</span>
      </div>
      {/* 中间这一整块也是拖动区（属性不继承 ⇒ 得自己带）。 */}
      <div data-tauri-drag-region className="h-full flex-1" />
      <div className="flex items-center gap-0.5">
        <button
          type="button"
          aria-label="最小化"
          className={buttonClass}
          onClick={() => void withWindow((win) => win.minimize())}
        >
          <Minus className="size-4" />
        </button>
        <button
          type="button"
          aria-label={maximized ? '还原' : '最大化'}
          className={buttonClass}
          onClick={() => void withWindow((win) => win.toggleMaximize())}
        >
          {maximized ? <Copy className="size-3.5" /> : <Square className="size-3.5" />}
        </button>
        <button
          type="button"
          aria-label="关闭"
          // hover 泛红用 **`destructive`**（语义令牌），不是 `presence-*` ——
          // 那几只是**读数**（在线/沉默/超时），拿来当强调色是误用。
          className={cn(buttonClass, 'hover:bg-destructive hover:text-destructive-foreground')}
          onClick={() => void withWindow((win) => win.close())}
        >
          <X className="size-4" />
        </button>
      </div>
    </header>
  )
}
