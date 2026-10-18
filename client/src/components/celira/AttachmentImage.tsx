import { ImageIcon } from 'lucide-react'
import type { Attachment } from '@/api/domain'
import { useAttachmentSource } from '@/lib/attachmentSource'
import { cn } from '@/lib/utils'

interface AttachmentImageProps {
  attachment: Attachment
  roomId: string
  alt: string
  className?: string
  imgLabel?: string
  onClick?: (e: React.MouseEvent<HTMLImageElement>) => void
  onTouchStart?: (e: React.TouchEvent<HTMLImageElement>) => void
  onTouchEnd?: (e: React.TouchEvent<HTMLImageElement>) => void
}

export function AttachmentImage({
  attachment,
  roomId,
  alt,
  className,
  imgLabel,
  onClick,
  onTouchStart,
  onTouchEnd,
}: AttachmentImageProps) {
  const { src, failed } = useAttachmentSource(attachment, roomId)

  if (failed) {
    return (
      <div className={cn('flex items-center justify-center bg-muted text-muted-foreground', className)}>
        <ImageIcon className="size-5" aria-hidden />
      </div>
    )
  }
  if (!src) {
    return <div className={cn('animate-pulse bg-muted', className)} />
  }
  return (
    <img
      src={src}
      alt={alt}
      aria-label={imgLabel}
      onClick={onClick}
      onTouchStart={onTouchStart}
      onTouchEnd={onTouchEnd}
      className={className}
    />
  )
}
