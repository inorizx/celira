import { useEffect, useState } from 'react'
import { Loader2 } from 'lucide-react'
import { cn } from '@/lib/utils'

let katexModule: Promise<typeof import('katex')> | null = null

function loadKatex() {
  if (!katexModule) katexModule = import('katex')
  return katexModule
}

interface FormulaBlockProps {
  tex: string
  displayMode?: boolean
  streaming?: boolean
}

export function FormulaBlock({ tex, displayMode = false, streaming = false }: FormulaBlockProps) {
  const [html, setHtml] = useState<string | null>(null)
  const [failed, setFailed] = useState(false)

  useEffect(() => {
    if (streaming) return
    let cancelled = false
    void (async () => {
      try {
        const katex = await loadKatex()
        const rendered = katex.renderToString(tex, {
          throwOnError: false,
          displayMode,
          strict: false,
        })
        if (!cancelled) setHtml(rendered)
      } catch {
        if (!cancelled) setFailed(true)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [tex, displayMode, streaming])

  if (streaming) {
    return <code className="rounded bg-muted px-1 py-0.5 font-mono text-code">{`${displayMode ? '$$' : '$'}${tex}${displayMode ? '$$' : '$'}`}</code>
  }
  if (html) {
    return (
      <span
        className={cn('katex-host', displayMode && 'my-1.5 block overflow-x-auto')}
        dangerouslySetInnerHTML={{ __html: html }}
      />
    )
  }
  if (failed) {
    return <code className="rounded bg-muted px-1 py-0.5 font-mono text-code">{tex}</code>
  }
  return (
    <span className="inline-flex items-center gap-1 text-caption text-muted-foreground">
      <Loader2 className="size-3 animate-spin" />
      公式加载中…
    </span>
  )
}
