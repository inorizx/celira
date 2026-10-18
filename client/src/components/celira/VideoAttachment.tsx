import { useCallback, useState } from 'react'
import { Film } from 'lucide-react'
import type { Attachment } from '@/api/domain'
import { useAttachmentSource } from '@/lib/attachmentSource'
import { cn } from '@/lib/utils'
import { FileRow } from './FileRow'

/**
 * 视频附件内联播放器：`mime` 为 `video/*` 时使用（判据在 `lib/attachmentMedia.ts`）。
 *
 * 与 `AudioAttachment` / `AttachmentImage` 走**同一条**鉴权 blob 源
 * （`useAttachmentSource` → `fetchAttachmentBlob`），所以凭据纪律与它俩一字不差：
 * **绝不** `<video src="/v1/...">` 直连（那会把 Bearer 挡在门外，还会把 URL 泄进 DOM）。
 *
 * ⚠ 代价如实登记（与图片、语音同款）：整份字节先进内存再交浏览器解码，所以
 * `preload="metadata"` 省的是**解码**而不是流量 —— 16 MiB 上限就是这条代价的上限。
 *
 * ⚠ 放不出来**必须说出口**（红路径不许静默）：源拿不到（401/404/断网）或浏览器解不了
 * 这个编码，都降级成**文件卡片 + 一句提示**（那张卡片自带下载，是一条能走的路），
 * 绝不留一块点不动的黑框。
 */
export function VideoAttachment({
  attachment,
  roomId,
}: {
  attachment: Attachment
  roomId: string
}) {
  const { src, failed } = useAttachmentSource(attachment, roomId)
  /**
   * **哪一个源**放不出来 —— 刻意不是布尔。
   *
   * 记「哪个源坏了」而不是「坏没坏」：源一变，判据自然不成立，于是**不需要**那条
   * 「源变了就清零」的 effect（它在真机上是竞态的一半，见下面 `attachVideo` 那段）。
   */
  const [failedSrc, setFailedSrc] = useState<string | null>(null)

  /**
   * ⚠ **不许用合成的 `onError`**（2026-10-08 真机实测，两处都验过）。
   *
   * 媒体元素的 `error` **可能在它还没插进 DOM 的时候就落下来** —— 那一刻它没有祖先，
   * 而 React 的合成事件挂在容器上，于是**收不到那一次**。实测现场：元素上
   * `error.code = 4` 明明在，`onError` 一次都没被调用，界面上就留一块点不动的黑框；
   * 从外面把同一个元素的 `src` 重新设一次，它才收得到（因为那时它已经在树里了）。
   *
   * ⇒ 一律**挂原生监听 ＋ 补读 `el.error` 终态**：事件可能在我们挂上之前就已经发生过了，
   *   读终态，别赌时序。`key={src}` 保证源一变就是新元素、重新走一遍这道判据。
   */
  const attachVideo = useCallback((el: HTMLVideoElement | null) => {
    if (!el) return
    const record = () => setFailedSrc(el.getAttribute('src'))
    el.addEventListener('error', record)
    if (el.error) record()
    return () => el.removeEventListener('error', record)
  }, [])

  if (failed || (src !== null && failedSrc === src)) {
    return (
      <div className="flex w-56 flex-col gap-1 self-start">
        <div className="flex items-center gap-2.5 rounded-xl border border-border bg-card px-3 py-2.5">
          <FileRow attachment={attachment} roomId={roomId} />
        </div>
        <p role="alert" className="text-micro text-muted-foreground">
          这个视频在这里放不出来，下载后可以看。
        </p>
      </div>
    )
  }

  if (!src) {
    return (
      <div
        aria-hidden
        className="flex aspect-video w-56 items-center justify-center rounded-xl border border-border bg-muted md:w-72"
      >
        <Film className="size-5 animate-pulse text-muted-foreground" />
      </div>
    )
  }

  return (
    <video
      key={src}
      ref={attachVideo}
      src={src}
      controls
      playsInline
      preload="metadata"
      aria-label={`播放视频 ${attachment.name}`}
      className={cn(
        'max-h-80 max-w-56 rounded-xl border border-border bg-muted md:max-w-72',
      )}
    />
  )
}
