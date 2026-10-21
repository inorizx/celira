import { useRef, useState } from 'react'
import { useParams, useSearchParams } from 'react-router'
import { Check, ImagePlus, Trash2, X } from 'lucide-react'
import { cn } from '@/lib/utils'
import { Segmented } from '@/components/celira/Segmented'
import { lastRoomSnapshot, OFFICIAL_WALLPAPERS, validateWallpaperBlob, WALLPAPER_ACCEPT, WALLPAPER_MAX_BYTES, type WallpaperSelection } from '@/lib/wallpaper'
import { useWallpaper } from '@/hooks/wallpaper-context'

/**
 * 聊天页消息区壁纸层：样式 + 语义蒙版（bg-background 透明度档位），
 * 浮在页面底色之上、消息内容之下；pointer-events 穿透。
 * 按会话覆盖优先于全局选择。
 * 仅作为装饰性 CSS backgroundImage 呈现（官方目录 URL 或 useWallpaper 返回的
 * 浏览器 Blob object URL），不经任何网络/上传/Core 附件，不渲染 HTML/Markdown。
 */
const DIM_CLASS = {
  low: 'bg-background/45',
  medium: 'bg-background/60',
  high: 'bg-background/80',
} as const

export function ChatWallpaper() {
  const { roomId } = useParams()
  const { resolveUrl, pref } = useWallpaper()
  const url = resolveUrl(roomId)
  if (!url) return null
  return (
    <div
      aria-hidden
      className="pointer-events-none absolute inset-0 overflow-hidden"
      style={{
        backgroundImage: `url(${url})`,
        backgroundSize: 'cover',
        backgroundPosition: 'center',
      }}
    >
      <div className={cn('absolute inset-0', DIM_CLASS[pref.global.dim])} />
    </div>
  )
}

/** 设置页壁纸选择器：默认 / 官方策展 / 本地上传 + 作用范围 + 蒙版档位。 */
export function WallpaperPicker() {
  const {
    pref,
    customUrl,
    resolveSelection,
    selectNone,
    selectOfficial,
    selectCustom,
    clearRoomOverride,
    setDim,
    uploadCustom,
    removeCustom,
  } = useWallpaper()
  const fileRef = useRef<HTMLInputElement>(null)
  const [uploading, setUploading] = useState(false)
  const [uploadError, setUploadError] = useState<string | null>(null)
  // 「仅此会话」作用范围：基于用户最后停留的会话（ChatPage 记录）；
  // 会话详情抽屉的「当前会话壁纸」入口带 ?wallpaper=room，直达该会话范围。
  // ⚠ 读的是**首帧快照**（`main.tsx` 在挂载之前补过水），不是模块求值期那次同步读。
  const [lastRoom] = useState(lastRoomSnapshot)
  const [searchParams] = useSearchParams()
  const [scope, setScope] = useState<'global' | 'room'>(() =>
    (searchParams.get('wallpaper') === 'room' || (lastRoom && pref.rooms[lastRoom.id])) && lastRoom
      ? 'room'
      : 'global',
  )
  const scopeRoomId = scope === 'room' ? lastRoom?.id : undefined
  const current: WallpaperSelection = resolveSelection(scopeRoomId)
  const hasOverride = Boolean(scopeRoomId && pref.rooms[scopeRoomId])

  const handleFile = async (file: File | undefined) => {
    if (!file) return
    const check = validateWallpaperBlob(file)
    if (!check.ok) {
      setUploadError(
        check.error === 'file_too_large'
          ? `图片太大，不能超过 ${Math.round(WALLPAPER_MAX_BYTES / (1024 * 1024))} MiB`
          : '不支持的图片类型',
      )
      return
    }
    setUploadError(null)
    setUploading(true)
    try {
      await uploadCustom(file, scopeRoomId)
    } catch {
      setUploadError('保存失败，请重试')
    } finally {
      setUploading(false)
    }
  }

  const thumbClass = (active: boolean) =>
    cn(
      'relative h-14 w-full cursor-pointer overflow-hidden rounded-lg border transition-colors',
      active ? 'border-primary ring-2 ring-ring' : 'border-border hover:border-input',
    )

  return (
    <div className="space-y-3">
      {lastRoom && (
        <div className="flex items-center gap-2">
          <span className="text-caption text-muted-foreground">作用于</span>
          <Segmented
            dense
            fill={false}
            label="壁纸作用范围"
            value={scope}
            onChange={setScope}
            options={[
              { id: 'global', label: '所有会话' },
              { id: 'room', label: `仅此会话（${lastRoom.name}）` },
            ] as const}
          />
        </div>
      )}
      <div className="grid grid-cols-3 gap-2 sm:grid-cols-5">
        <button type="button" onClick={() => selectNone(scopeRoomId)} aria-pressed={current.kind === 'none'} className={thumbClass(current.kind === 'none')}>
          <span className="flex size-full items-center justify-center bg-muted text-caption text-muted-foreground">
            默认
          </span>
          {current.kind === 'none' && <Check className="absolute right-1 top-1 size-3.5 text-primary" />}
        </button>
        {OFFICIAL_WALLPAPERS.map((w) => (
          <button
            key={w.id}
            type="button"
            onClick={() => selectOfficial(w.id, scopeRoomId)}
            aria-pressed={current.kind === 'official' && current.assetId === w.id}
            aria-label={`壁纸：${w.name}`}
            className={thumbClass(current.kind === 'official' && current.assetId === w.id)}
          >
            <img src={w.url} alt={w.name} className="size-full object-cover" />
            <span className="absolute inset-x-0 bottom-0 bg-overlay px-1 py-0.5 text-center text-micro text-primary-foreground">
              {w.name}
            </span>
            {current.kind === 'official' && current.assetId === w.id && (
              <Check className="absolute right-1 top-1 size-3.5 text-primary" />
            )}
          </button>
        ))}
        {customUrl ? (
          <div className={thumbClass(current.kind === 'custom')}>
            <button
              type="button"
              onClick={() => selectCustom(scopeRoomId)}
              aria-pressed={current.kind === 'custom'}
              aria-label="壁纸：自定义图片"
              className="size-full cursor-pointer"
            >
              <img src={customUrl} alt="自定义壁纸" className="size-full object-cover" />
              {current.kind === 'custom' && <Check className="absolute right-1 top-1 size-3.5 text-primary" />}
            </button>
            <button
              type="button"
              onClick={() => {
                setUploadError(null)
                void removeCustom().catch(() => setUploadError('删除失败，请重试'))
              }}
              aria-label="删除自定义壁纸"
              className="absolute bottom-1 right-1 cursor-pointer rounded-md bg-overlay p-1 text-primary-foreground transition-opacity hover:opacity-85"
            >
              <Trash2 className="size-3" />
            </button>
          </div>
        ) : (
          <button
            type="button"
            onClick={() => fileRef.current?.click()}
            disabled={uploading}
            className={cn(thumbClass(false), 'flex items-center justify-center gap-1 bg-muted text-caption text-muted-foreground')}
          >
            <ImagePlus className="size-4" />
            {uploading ? '保存中…' : '上传'}
          </button>
        )}
      </div>
      {hasOverride && scopeRoomId && (
        <button
          type="button"
          onClick={() => clearRoomOverride(scopeRoomId)}
          className="flex cursor-pointer items-center gap-1 text-caption text-muted-foreground transition-colors hover:text-foreground"
        >
          <X className="size-3" />
          清除此会话的专属壁纸（回落到全局设置）
        </button>
      )}
      <input
        ref={fileRef}
        type="file"
        accept={WALLPAPER_ACCEPT}
        className="hidden"
        onChange={(e) => {
          void handleFile(e.target.files?.[0])
          e.target.value = ''
        }}
      />
      {uploadError && <p className="text-caption text-destructive">{uploadError}</p>}
      <div className="flex items-center gap-2">
        <span className="text-caption text-muted-foreground">蒙版</span>
        <Segmented
          dense
          fill={false}
          label="蒙版强度"
          value={pref.global.dim}
          onChange={setDim}
          options={[
            { id: 'low', label: '弱' },
            { id: 'medium', label: '中' },
            { id: 'high', label: '强' },
          ] as const}
        />
      </div>
    </div>
  )
}
