import { useEffect, useState } from 'react'
import { Download, FileText } from 'lucide-react'
import type { Attachment } from '@/api/domain'
import { cn } from '@/lib/utils'
import {
  canDownloadAttachment,
  downloadAttachment,
  downloadOutcomeText,
} from '@/lib/attachmentDownload'

/**
 * 文件卡片（也用在**视频放不出来**那条降级路上）。
 *
 * 下载的**收场要说出口**（TASK_72 追补：失败原先被 `void` 吞掉、点了没反应）。
 * 2026-10-08 起还多一格：**壳里成功也要说** —— 那边没有浏览器那套下载 UI，不说一句用户
 * 就不知道东西存没存下来、存哪了（壳里的落点见 `app/src-tauri/src/downloads.rs`）。
 */
export function FileRow({
  attachment,
  roomId,
  onSelfBubble,
}: {
  attachment: Attachment
  roomId: string
  onSelfBubble?: boolean
}) {
  const [note, setNote] = useState<{ text: string; bad: boolean } | null>(null)

  // 成功那句是**看一眼就够**的（几秒后自己收）；失败那句留着 —— 它要求用户动手。
  useEffect(() => {
    if (!note || note.bad) return
    const timer = setTimeout(() => setNote(null), 4000)
    return () => clearTimeout(timer)
  }, [note])

  return (
    <>
      <div
        className={cn(
          'flex size-8 shrink-0 items-center justify-center rounded-md',
          onSelfBubble ? 'bg-bubble-self-foreground/20 text-bubble-self-foreground' : 'bg-accent text-accent-foreground',
        )}
      >
        <FileText className="size-4" />
      </div>
      <div className="min-w-0 text-left">
        <div className={cn('truncate text-caption font-medium', onSelfBubble ? 'text-bubble-self-foreground' : 'text-foreground')}>
          {attachment.name}
        </div>
        {attachment.size && (
          <div className={cn('text-micro', onSelfBubble ? 'text-bubble-self-foreground/70' : 'text-muted-foreground')}>
            {attachment.size}
          </div>
        )}
        {note && (
          <p
            role={note.bad ? 'alert' : 'status'}
            className={cn('text-micro', note.bad ? 'text-presence-failed' : 'text-muted-foreground')}
          >
            {note.text}
          </p>
        )}
      </div>
      {canDownloadAttachment(attachment) && (
        <button
          type="button"
          onClick={() => {
            void (async () => {
              const outcome = await downloadAttachment(roomId, attachment)
              const text = downloadOutcomeText(outcome)
              setNote(text ? { text, bad: !outcome.ok } : null)
            })()
          }}
          aria-label={`下载 ${attachment.name}`}
          className={cn(
            'ml-auto shrink-0 cursor-pointer rounded-full p-1.5 transition-opacity hover:opacity-75',
            onSelfBubble ? 'text-bubble-self-foreground/80' : 'text-muted-foreground',
          )}
        >
          <Download className="size-3.5" />
        </button>
      )}
    </>
  )
}
