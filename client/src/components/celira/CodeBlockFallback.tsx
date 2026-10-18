import { Copy, Check, AlertTriangle } from 'lucide-react'
import { useState } from 'react'

function CopyButton({ code }: { code: string }) {
  const [copied, setCopied] = useState(false)
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      // clipboard unavailable
    }
  }
  return (
    <button
      type="button"
      onClick={() => void copy()}
      aria-label="复制代码"
      className="flex shrink-0 cursor-pointer items-center gap-1 rounded-md px-1.5 py-0.5 text-caption text-muted-foreground transition-colors hover:bg-background hover:text-foreground"
    >
      {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
      {copied ? '已复制' : '复制'}
    </button>
  )
}

interface CodeBlockFallbackProps {
  code: string
  error?: string
  streaming?: boolean
}

export function CodeBlockFallback({ code, error, streaming = false }: CodeBlockFallbackProps) {
  return (
    <div className="group/code relative my-1.5 overflow-hidden rounded-lg bg-muted first:mt-0 last:mb-0">
      {error && (
        <div className="flex items-center gap-1.5 border-b border-border/60 bg-presence-failed/10 px-3 py-1.5 text-caption text-presence-failed">
          <AlertTriangle className="size-3" aria-hidden />
          {error}
        </div>
      )}
      <div className="flex items-center justify-between border-b border-border/60 px-3 py-1">
        <span className="text-micro text-muted-foreground">{streaming ? '生成中…' : 'mermaid'}</span>
        <CopyButton code={code} />
      </div>
      <pre className="overflow-x-auto p-3 text-caption">
        <code className="font-mono text-[0.9em]">{code}</code>
      </pre>
    </div>
  )
}
