import React from 'react'
import ReactMarkdown, { type Components } from 'react-markdown'
import remarkGfm from 'remark-gfm'
import remarkMath from 'remark-math'
import rehypeHighlight from 'rehype-highlight'
import { Check, Copy } from 'lucide-react'
import javascript from 'highlight.js/lib/languages/javascript.js'
import typescript from 'highlight.js/lib/languages/typescript.js'
import python from 'highlight.js/lib/languages/python.js'
import rust from 'highlight.js/lib/languages/rust.js'
import bash from 'highlight.js/lib/languages/bash.js'
import json from 'highlight.js/lib/languages/json.js'
import css from 'highlight.js/lib/languages/css.js'
import xml from 'highlight.js/lib/languages/xml.js'
import 'katex/dist/katex.min.css'
import { MentionText } from './MentionText'
import type { MentionMember } from '@/api/mentionTargets'
import { MermaidBlock } from './MermaidBlock'
import { FormulaBlock } from './FormulaBlock'
import { cn } from '@/lib/utils'

const highlightLanguages = { javascript, typescript, python, rust, bash, json, css, xml }

/**
 * remark-math uses `$` delimiters, while Markdown consumes the backslash in
 * `\\(...\\)` before mdast is produced. Normalize only ordinary Markdown text
 * before parsing so the display layer supports escaped-parenthesis math without
 * touching inline/fenced code. Standalone `$$...$$` is expanded to flow-math
 * lines because remark-math treats a same-line pair as inline math.
 */
function normalizeMathDelimiters(source: string): string {
  const lines = source.split('\n')
  let fence: { marker: '`' | '~'; length: number } | null = null
  let mathBlock = false

  return lines
    .flatMap((line) => {
      const fenceMatch = /^\s{0,3}(`{3,}|~{3,})/.exec(line)
      if (fence) {
        if (fenceMatch && fenceMatch[1][0] === fence.marker && fenceMatch[1].length >= fence.length) {
          fence = null
        }
        return [line]
      }
      if (fenceMatch) {
        fence = { marker: fenceMatch[1][0] as '`' | '~', length: fenceMatch[1].length }
        return [line]
      }

      if (/^\s{0,3}\$\$\s*$/.test(line)) {
        mathBlock = !mathBlock
        return [line]
      }
      if (mathBlock) return [line]

      const standaloneMath = /^(\s{0,3})\$\$(?!\$)(.+?)\$\$\s*$/.exec(line)
      if (standaloneMath) {
        const indent = standaloneMath[1]
        return [`${indent}$$`, standaloneMath[2], `${indent}$$`]
      }

      let output = ''
      let cursor = 0
      while (cursor < line.length) {
        const open = line.indexOf('\\(', cursor)
        const tick = line.indexOf('`', cursor)

        if (tick >= 0 && (open < 0 || tick < open)) {
          output += line.slice(cursor, tick)
          let runEnd = tick
          while (line[runEnd] === '`') runEnd += 1
          const run = line.slice(tick, runEnd)
          const closeTick = line.indexOf(run, runEnd)
          if (closeTick < 0) {
            output += line.slice(tick)
            break
          }
          output += line.slice(tick, closeTick + run.length)
          cursor = closeTick + run.length
          continue
        }

        if (open < 0) {
          output += line.slice(cursor)
          break
        }
        output += line.slice(cursor, open)
        const close = line.indexOf('\\)', open + 2)
        if (close < 0) {
          output += line.slice(open)
          break
        }
        const value = line.slice(open + 2, close)
        output += value.trim() ? `$$${value}$$` : line.slice(open, close + 2)
        cursor = close + 2
      }
      return [output]
    })
    .join('\n')
}

interface MarkdownTextProps {
  text: string
  onSelf?: boolean
  className?: string
  streaming?: boolean
  /** 结构化提及目标（participant ID 列表）；与 `resolveMention` 一起把 @ 渲染交给
   * MentionText（真实世界传真实成员投影，种类按 **id** 取）。 */
  mentions?: string[]
  resolveMention?: (id: string) => MentionMember | undefined
  /** 房间成员名快照（agent/human 都要）：@ 到任意成员名都高亮。 */
  mentionMembers?: MentionMember[]
}

function CopyCodeButton({ code }: { code: string }) {
  const [copied, setCopied] = React.useState(false)
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
      className="flex cursor-pointer items-center gap-1 rounded-md px-1.5 py-0.5 text-caption text-muted-foreground transition-colors hover:bg-background hover:text-foreground"
    >
      {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
      {copied ? '已复制' : '复制'}
    </button>
  )
}

type MathComponents = {
  math: React.ComponentType<{ value?: string }>
  inlineMath: React.ComponentType<{ value?: string }>
}

const components = (
  onSelf: boolean | undefined,
  streaming: boolean | undefined,
  mentions: string[] | undefined,
  resolveMention: ((id: string) => MentionMember | undefined) | undefined,
  mentionMembers: MentionMember[] | undefined,
): Components & MathComponents => ({
  text: ({ children }) => (
    <MentionText
      text={String(children)}
      mentions={mentions}
      resolveMention={resolveMention}
      members={mentionMembers}
    />
  ),  p: ({ children }) => <p className="my-1.5 first:mt-0 last:mb-0">{children}</p>,
  h1: ({ children }) => <h1 className="my-2 text-title font-semibold first:mt-0 last:mb-0">{children}</h1>,
  h2: ({ children }) => <h2 className="my-2 text-title font-semibold first:mt-0 last:mb-0">{children}</h2>,
  h3: ({ children }) => <h3 className="my-2 text-body font-semibold first:mt-0 last:mb-0">{children}</h3>,
  h4: ({ children }) => <h4 className="my-2 text-body font-semibold first:mt-0 last:mb-0">{children}</h4>,
  ul: ({ children }) => <ul className="my-1.5 list-disc pl-5 first:mt-0 last:mb-0">{children}</ul>,
  ol: ({ children }) => <ol className="my-1.5 list-decimal pl-5 first:mt-0 last:mb-0">{children}</ol>,
  li: ({ children }) => <li className="my-0.5">{children}</li>,
  a: ({ children, href }) => (
    <a href={href} target="_blank" rel="noreferrer" className="underline underline-offset-2">
      {children}
    </a>
  ),
  blockquote: ({ children }) => (
    <blockquote className="my-1.5 border-l-2 border-border pl-3 text-muted-foreground first:mt-0 last:mb-0">
      {children}
    </blockquote>
  ),
  pre: ({ children }) => <>{children}</>,
  code: ({ className, children }) => {
    const cls = className ?? ''
    const match = /language-(\w+)/.exec(cls)
    const lang = match?.[1]
    const text = String(children ?? '').replace(/\n$/, '')
    if (lang === 'mermaid') {
      return <MermaidBlock code={text} streaming={streaming} />
    }
    if (cls.includes('math-inline')) {
      return <FormulaBlock tex={text} streaming={streaming} />
    }
    if (lang === 'math') {
      return <FormulaBlock tex={text} displayMode streaming={streaming} />
    }
    const isBlock = !!lang || cls.includes('hljs') || text.includes('\n')
    if (isBlock) {
      return (
        <div className="group/code relative my-1.5 overflow-hidden rounded-lg bg-muted first:mt-0 last:mb-0">
          <div className="flex items-center justify-between border-b border-border/60 px-3 py-1">
            <span className="text-micro text-muted-foreground">{lang ?? 'code'}</span>
            <CopyCodeButton code={text} />
          </div>
          <pre className="overflow-x-auto p-3 text-caption">
            <code className="font-mono text-code">{children}</code>
          </pre>
        </div>
      )
    }
    return (
      <code
        className={cn(
          'rounded px-1 py-0.5 font-mono text-code',
          onSelf ? 'bg-bubble-self-foreground/20' : 'bg-muted',
        )}
      >
        {children}
      </code>
    )
  },
  math: ({ value }) => <FormulaBlock tex={value ?? ''} displayMode streaming={streaming} />,
  inlineMath: ({ value }) => <FormulaBlock tex={value ?? ''} streaming={streaming} />,
  table: ({ children }) => (
    <div className="my-1.5 overflow-x-auto first:mt-0 last:mb-0">
      <table className="w-full border-collapse text-caption">{children}</table>
    </div>
  ),
  th: ({ children }) => (
    <th className="border border-border bg-muted px-2 py-1 text-left font-medium">{children}</th>
  ),
  td: ({ children }) => <td className="border border-border px-2 py-1">{children}</td>,
  hr: () => <hr className="my-3 border-border" />,
  strong: ({ children }) => <strong className="font-semibold">{children}</strong>,
})

export function MarkdownText({
  text,
  onSelf,
  className,
  streaming = false,
  mentions,
  resolveMention,
  mentionMembers,
}: MarkdownTextProps) {
  return (
    <div className={cn('break-words text-left', className)}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm, [remarkMath, { singleDollarTextMath: false }]]}
        rehypePlugins={[
          [
            rehypeHighlight,
            { languages: highlightLanguages, detect: false, ignoreMissing: true },
          ],
        ]}
        components={components(onSelf, streaming, mentions, resolveMention, mentionMembers)}
      >
        {normalizeMathDelimiters(text)}
      </ReactMarkdown>
    </div>
  )
}
