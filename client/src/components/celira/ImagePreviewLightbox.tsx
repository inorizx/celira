import { useEffect, useRef, useState } from 'react'
import { ChevronLeft, ChevronRight, Download, X } from 'lucide-react'
import { Dialog as LightboxDialog } from 'radix-ui'
import { Button } from '@/components/ui/button'
import type { Attachment } from '@/api/domain'
import { canDownloadAttachment, downloadAttachment, downloadOutcomeText } from '@/lib/attachmentDownload'
import { cn } from '@/lib/utils'
import { AttachmentImage } from './AttachmentImage'

export interface PreviewImage {
  id: string
  attachment: Attachment
  roomId: string
  name: string
}

interface ImagePreviewLightboxProps {
  open: boolean
  images: PreviewImage[]
  initialIndex?: number
  onClose: () => void
}

export function ImagePreviewLightbox({
  open,
  images,
  initialIndex = 0,
  onClose,
}: ImagePreviewLightboxProps) {
  const [index, setIndex] = useState(initialIndex)
  // 下载的**收场要说出口**（TASK_72 追补；2026-10-08 起壳里成功也要说 —— 那边没有浏览器
  // 那套下载 UI）。判别式与两档的差别见 `lib/downloadOutcome.ts` 的文件头。
  const [downloadNote, setDownloadNote] = useState<{ text: string; bad: boolean } | null>(null)
  const touchStart = useRef<{ x: number; y: number } | null>(null)

  const step = (delta: number) => {
    if (images.length === 0) return
    setIndex((i) => (i + delta + images.length) % images.length)
  }

  useEffect(() => {
    if (open) setIndex(Math.min(initialIndex, Math.max(images.length - 1, 0)))
  }, [open, initialIndex, images.length])

  useEffect(() => {
    if (!open) return
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
      if (e.key === 'ArrowLeft') step(-1)
      if (e.key === 'ArrowRight') step(1)
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, images.length, onClose])

  if (!open || images.length === 0) return null
  const current = images[index]

  return (
    // 灯箱 = **真模态层**，与房间抽屉同一个 Radix Dialog primitive 家族（抽屉的 Sheet 就是它）。
    // 判据：两个覆盖物不能靠「谁后插进 DOM」决定层叠——那是不可维持的隐式契约。做法：
    //   · portal 到 body ⇒ 不再与抽屉挤在页面子树里比插入顺序；
    //   · 层梯 z-[60]（Sheet/覆盖物是 z-50；主题的装饰扫描线同为 z-60，由 body::after 最后绘制，仍在最上）；
    //   · 自己接管 aria-hidden 与焦点 ⇒ 抽屉开着时从「图片」页签点开灯箱：
    //     灯箱是顶层（抽屉那层被标 aria-hidden），「下载/关闭预览」可点、Tab 进得来、
    //     Esc 只关灯箱（抽屉留着）——不再出现「灯箱被抽屉压在下面」。
    <LightboxDialog.Root open onOpenChange={(next) => !next && onClose()}>
      <LightboxDialog.Portal>
        <LightboxDialog.Overlay className="fixed inset-0 z-[60] bg-overlay" />
        <LightboxDialog.Content
          aria-label={`图片预览 ${current.name}`}
          className="fixed inset-0 z-[60] flex flex-col outline-none"
          onClick={onClose}
        >
          <LightboxDialog.Title className="sr-only">图片预览 {current.name}</LightboxDialog.Title>
          <div className="flex items-center justify-between px-4 py-3 text-caption text-background/80 md:px-6">
            <span className="min-w-0 flex-1 truncate">
              {current.name} · {index + 1}/{images.length}
            </span>
            {canDownloadAttachment(current.attachment) && (
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation()
                  void (async () => {
                    const outcome = await downloadAttachment(current.roomId, current.attachment)
                    const text = downloadOutcomeText(outcome)
                    setDownloadNote(text ? { text, bad: !outcome.ok } : null)
                  })()
                }}
                aria-label={`下载 ${current.name}`}
                className="mr-2 cursor-pointer rounded-full p-2 text-background transition-colors hover:bg-background/15"
              >
                <Download className="size-5" />
              </button>
            )}
            <Button
              variant="ghost"
              size="icon"
              aria-label="关闭预览"
              autoFocus
              className="text-background hover:bg-background/15 hover:text-background"
              onClick={onClose}
            >
              <X className="size-5" />
            </Button>
          </div>
          {downloadNote && (
            <p
              role={downloadNote.bad ? 'alert' : 'status'}
              className={cn(
                'px-4 pb-2 text-center text-caption md:px-6',
                downloadNote.bad ? 'text-presence-failed' : 'text-background/80',
              )}
            >
              {downloadNote.text}
            </p>
          )}
          <div className="flex min-h-0 flex-1 items-center justify-center gap-2 px-2 pb-4 md:gap-4 md:px-6">
            <Button
              variant="ghost"
              size="icon"
              aria-label="上一张"
              disabled={images.length === 1}
              className="shrink-0 text-background hover:bg-background/15 hover:text-background"
              onClick={(e) => {
                e.stopPropagation()
                step(-1)
              }}
            >
              <ChevronLeft className="size-6" />
            </Button>
            <AttachmentImage
              attachment={current.attachment}
              roomId={current.roomId}
              alt={current.name}
              onClick={(e) => e?.stopPropagation()}
              onTouchStart={(e) => {
                const t = e.touches[0]
                touchStart.current = { x: t.clientX, y: t.clientY }
              }}
              onTouchEnd={(e) => {
                const start = touchStart.current
                touchStart.current = null
                if (!start) return
                const t = e.changedTouches[0]
                const dx = t.clientX - start.x
                const dy = t.clientY - start.y
                if (Math.abs(dx) > 48 && Math.abs(dx) > Math.abs(dy)) step(dx < 0 ? 1 : -1)
              }}
              className="max-h-[80vh] max-w-[80vw] touch-pan-y rounded-lg object-contain select-none md:max-w-[70vw]"
            />
            <Button
              variant="ghost"
              size="icon"
              aria-label="下一张"
              disabled={images.length === 1}
              className="shrink-0 text-background hover:bg-background/15 hover:text-background"
              onClick={(e) => {
                e.stopPropagation()
                step(1)
              }}
            >
              <ChevronRight className="size-6" />
            </Button>
          </div>
        </LightboxDialog.Content>
      </LightboxDialog.Portal>
    </LightboxDialog.Root>
  )
}
