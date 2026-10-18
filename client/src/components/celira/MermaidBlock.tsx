import { useEffect, useRef, useState } from 'react'
import { Loader2 } from 'lucide-react'
import { cn } from '@/lib/utils'
import { CodeBlockFallback } from './CodeBlockFallback'

type MermaidApi = typeof import('mermaid')['default']

let mermaidModule: Promise<MermaidApi> | null = null
let initializedTheme: string | null = null
let queue: Promise<unknown> = Promise.resolve()
let idCounter = 0

const svgCache = new Map<string, string>()
const failedCache = new Set<string>()

function loadMermaid(): Promise<MermaidApi> {
  if (!mermaidModule) mermaidModule = import('mermaid').then((m) => m.default)
  return mermaidModule
}

function themeKey(): string {
  return document.documentElement.classList.contains('dark') ? 'dark' : 'light'
}

function cacheKey(code: string): string {
  return `${themeKey()}:${code}`
}

async function initializeMermaid(mermaid: MermaidApi) {
  const theme = themeKey()
  if (initializedTheme === theme) return
  const styles = getComputedStyle(document.documentElement)
  const v = (name: string) => styles.getPropertyValue(name).trim() || `var(${name})`
  mermaid.initialize({
    startOnLoad: false,
    securityLevel: 'strict',
    theme: 'base',
    themeVariables: {
      primaryColor: v('--secondary'),
      primaryTextColor: v('--foreground'),
      primaryBorderColor: v('--border'),
      lineColor: v('--muted-foreground'),
      textColor: v('--foreground'),
      fontSize: '14px',
      fontFamily: v('--font-sans'),
      mainBkg: v('--card'),
      nodeBorder: v('--border'),
      clusterBkg: v('--muted'),
      clusterBorder: v('--border'),
      edgeLabelBackground: v('--card'),
      actorBkg: v('--card'),
      actorBorder: v('--border'),
      actorTextColor: v('--foreground'),
      labelBackground: v('--card'),
      noteBkgColor: v('--secondary'),
      noteBorderColor: v('--border'),
    },
  })
  initializedTheme = theme
}

async function renderMermaid(code: string): Promise<string> {
  const key = cacheKey(code)
  const cached = svgCache.get(key)
  if (cached) return cached
  const run = queue.then(async () => {
    const mermaid = await loadMermaid()
    await initializeMermaid(mermaid)
    await mermaid.parse(code)
    const id = `celira-mermaid-${++idCounter}`
    const { svg } = await mermaid.render(id, code)
    svgCache.set(key, svg)
    return svg
  })
  queue = run.catch(() => {})
  return run
}

interface MermaidBlockProps {
  code: string
  streaming?: boolean
}

export function MermaidBlock({ code, streaming = false }: MermaidBlockProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const mountedRef = useRef(true)
  const [state, setState] = useState<'pending' | 'rendered' | 'failed'>('pending')
  const [svg, setSvg] = useState('')

  useEffect(() => {
    mountedRef.current = true
    if (streaming) return
    const key = cacheKey(code)
    if (failedCache.has(key)) {
      setState('failed')
      return
    }
    const cached = svgCache.get(key)
    if (cached) {
      setSvg(cached)
      setState('rendered')
      return
    }
    const doRender = async () => {
      try {
        const out = await renderMermaid(code)
        if (mountedRef.current) {
          setSvg(out)
          setState('rendered')
        }
      } catch {
        failedCache.add(key)
        if (mountedRef.current) setState('failed')
      }
    }
    const el = containerRef.current
    if (!el || typeof IntersectionObserver === 'undefined') {
      void doRender()
      return
    }
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          io.disconnect()
          void doRender()
        }
      },
      { rootMargin: '200px' },
    )
    io.observe(el)
    return () => {
      io.disconnect()
      mountedRef.current = false
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [code, streaming])

  if (streaming) {
    return <CodeBlockFallback code={code} streaming />
  }
  if (state === 'rendered') {
    return (
      <div
        ref={containerRef}
        className="overflow-x-auto rounded-lg bg-card p-3"
        dangerouslySetInnerHTML={{ __html: svg }}
      />
    )
  }
  if (state === 'failed') {
    return <CodeBlockFallback code={code} error="图表解析失败，已显示原文" />
  }
  return (
    <div
      ref={containerRef}
      className={cn(
        'flex items-center gap-2 rounded-lg bg-card p-3 text-caption text-muted-foreground',
        state === 'pending' && 'min-h-12',
      )}
    >
      <Loader2 className="size-3.5 animate-spin" />
      图表加载中…
    </div>
  )
}
