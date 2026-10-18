import { useEffect, useRef, useState } from 'react'
import { AudioLines, Pause, Play } from 'lucide-react'
import type { Attachment } from '@/api/domain'
import { useAttachmentSource } from '@/lib/attachmentSource'
import { cn } from '@/lib/utils'

/** 语音附件内联播放器：kind:'file' 且 mime 为 audio/* 时使用。 */
export function AudioAttachment({
  attachment,
  roomId,
  onSelfBubble,
}: {
  attachment: Attachment
  roomId: string
  onSelfBubble?: boolean
}) {
  const { src, failed } = useAttachmentSource(attachment, roomId)
  const audioRef = useRef<HTMLAudioElement | null>(null)
  const [playing, setPlaying] = useState(false)
  const [duration, setDuration] = useState(attachment.durationSeconds ?? 0)
  // 播放失败**要说出口**（TASK_72「红路径不许静默」）：源加载失败那条路有「语音不可用」回退，
  // 而**用户点了播放、浏览器拒绝/解码失败**这一条原先被 `.catch` 吞掉 ⇒ 点了没反应。
  // 文案**复用**同一句「语音不可用」（不新增词表条目）。
  const [playFailed, setPlayFailed] = useState(false)

  useEffect(() => {
    setPlaying(false)
    setDuration(attachment.durationSeconds ?? 0)
    setPlayFailed(false)
  }, [attachment.id, attachment.durationSeconds])

  const toggle = () => {
    const audio = audioRef.current
    if (!audio || !src) return
    if (playing) audio.pause()
    else {
      setPlayFailed(false)
      void audio.play().catch(() => setPlayFailed(true))
    }
  }

  if (failed) {
    return (
      <div className="flex items-center gap-2.5 text-muted-foreground">
        <AudioLines className="size-4" aria-hidden />
        <span className="text-caption">语音不可用</span>
      </div>
    )
  }

  return (
    <>
      <button
        type="button"
        onClick={toggle}
        disabled={!src}
        aria-label={playing ? `暂停语音 ${attachment.name}` : `播放语音 ${attachment.name}`}
        className={cn(
          'flex size-8 shrink-0 cursor-pointer items-center justify-center rounded-full transition-opacity hover:opacity-85 disabled:opacity-50',
          onSelfBubble
            ? 'bg-bubble-self-foreground/20 text-bubble-self-foreground'
            : 'bg-accent text-accent-foreground',
        )}
      >
        {playing ? <Pause className="size-4" /> : <Play className="size-4" />}
      </button>
      <span
        className={cn(
          'shrink-0 text-caption tabular-nums',
          onSelfBubble ? 'text-bubble-self-foreground' : 'text-foreground',
        )}
      >
        {duration > 0 ? `${duration}″` : '…'}
      </span>
      {playFailed && (
        <span role="alert" className="shrink-0 text-caption text-presence-failed">
          语音不可用
        </span>
      )}
      {src && (
        <audio
          ref={audioRef}
          src={src}
          preload="metadata"
          className="hidden"
          onPlay={() => setPlaying(true)}
          onPause={() => setPlaying(false)}
          onEnded={() => setPlaying(false)}
          onLoadedMetadata={(e) => {
            if (!attachment.durationSeconds) {
              const d = e.currentTarget.duration
              if (Number.isFinite(d) && d > 0) setDuration(Math.round(d))
            }
          }}
        />
      )}
    </>
  )
}
