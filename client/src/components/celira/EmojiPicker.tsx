import { useEffect, useState } from 'react'
import { Smile, Sticker } from 'lucide-react'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Button } from '@/components/ui/button'
import type { Attachment } from '@/api/domain'
import { loadRecentEmoji, recordRecentEmoji } from '@/lib/emojiRecents'
import { cn } from '@/lib/utils'

const EMOJI_GROUPS: string[][] = [
  ['😀', '😄', '😁', '😂', '🤣', '😊', '😇', '🙂'],
  ['😉', '😌', '😍', '🥰', '😘', '😋', '🤔', '🤗'],
  ['🙃', '😅', '😆', '😮', '😴', '😪', '😢', '😭'],
  ['😤', '😠', '🤯', '🥳', '🤩', '😎', '🤓', '🥺'],
  ['👍', '👎', '👏', '🙏', '💪', '🤝', '👌', '✌️'],
  ['❤️', '🧡', '💛', '💚', '💙', '💜', '💯', '🔥'],
  ['✨', '🎉', '🎊', '🎁', '📌', '✅', '❌', '❓'],
]

export const STICKER_ATTACHMENTS: Attachment[] = [
  { id: 'st-hi', kind: 'image', name: 'hi.svg', url: '/mock/stickers/st-hi.svg' },
  { id: 'st-thumbs', kind: 'image', name: 'thumbs.svg', url: '/mock/stickers/st-thumbs.svg' },
  { id: 'st-heart', kind: 'image', name: 'heart.svg', url: '/mock/stickers/st-heart.svg' },
  { id: 'st-sad', kind: 'image', name: 'sad.svg', url: '/mock/stickers/st-sad.svg' },
  { id: 'st-ok', kind: 'image', name: 'ok.svg', url: '/mock/stickers/st-ok.svg' },
]

interface EmojiPickerProps {
  onInsertEmoji: (emoji: string) => void
  onSendSticker: (attachment: Attachment) => void
  disabled?: boolean
}

export function EmojiPicker({ onInsertEmoji, onSendSticker, disabled = false }: EmojiPickerProps) {
  const [tab, setTab] = useState<'emoji' | 'sticker'>('emoji')
  const [open, setOpen] = useState(false)
  // 存储口是**异步**的（`api/storage.ts`：localStorage 同步、app 的文件存储不同步），
  // 所以这里不能再用 `useState(loadRecentEmoji)` 那种"初始值即读"的写法 ——
  // 首帧先用空表，读到之后再补上（最近用过的表情晚一帧出现，比让整个面板等它值得）。
  const [recents, setRecents] = useState<string[]>([])

  useEffect(() => {
    let alive = true
    void loadRecentEmoji().then((list) => {
      if (alive) setRecents(list)
    })
    return () => {
      alive = false
    }
  }, [])

  const handleInsert = (emoji: string) => {
    // 顺序要紧：**先**把新列表算出来再交给界面，否则连点两个表情时，
    // 两次异步读可能先后到达，把"刚记的那一个"覆盖回去。
    void recordRecentEmoji(emoji).then(setRecents)
    onInsertEmoji(emoji)
  }

  const emojiButton = (e: string) => (
    <button
      key={e}
      type="button"
      onClick={() => handleInsert(e)}
      aria-label={`插入表情 ${e}`}
      className="cursor-pointer rounded-md p-1 text-lg leading-none transition-colors hover:bg-muted"
    >
      {e}
    </button>
  )

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button variant="ghost" size="icon" aria-label="表情与表情包" disabled={disabled}>
          <Smile className="size-4" />
        </Button>
      </PopoverTrigger>
      {/* forceMount + hidden：网格常驻 DOM，开关只做显隐——实测每次打开重新挂载
          Radix 层 + 56 个彩色 emoji 排版约 100ms 长任务（帧间隔 100ms+），常驻后
          打开延迟稳定在 ~55ms 且无长任务。对齐方式（start/center）与卡顿无关。 */}
      <PopoverContent
        forceMount
        align="center"
        side="top"
        className={cn(
          'w-72 p-0',
          !open && 'invisible pointer-events-none',
        )}
        onFocusOutside={(e) => e.preventDefault()}
      >
        {tab === 'emoji' ? (
          <ScrollArea className="h-56">
            <div className="p-2">
              {recents.length > 0 && (
                <>
                  <div className="px-1 pb-1 text-caption text-muted-foreground">最近使用</div>
                  <div className="mb-1 grid grid-cols-8 gap-0.5">{recents.map(emojiButton)}</div>
                </>
              )}
              {EMOJI_GROUPS.map((group, gi) => (
                <div key={gi} className="mb-1 grid grid-cols-8 gap-0.5">
                  {group.map(emojiButton)}
                </div>
              ))}
            </div>
          </ScrollArea>
        ) : (
          <ScrollArea className="h-56">
            <div className="grid grid-cols-5 content-start gap-1.5 p-2">
              {STICKER_ATTACHMENTS.map((s) => (
                <button
                  key={s.id}
                  type="button"
                  onClick={() => onSendSticker(s)}
                  aria-label={`发送表情包 ${s.name}`}
                  className="cursor-pointer rounded-lg p-1 transition-colors hover:bg-muted"
                >
                  <img src={s.url} alt={s.name} className="aspect-square w-full object-contain" />
                </button>
              ))}
            </div>
          </ScrollArea>
        )}
        {/* 微信式底部子导航栏：内容区在上、类别图标在下 */}
        <div className="flex gap-1 border-t border-border p-1.5">
          {(
            [
              { key: 'emoji', label: '表情', icon: Smile },
              { key: 'sticker', label: '表情包', icon: Sticker },
            ] as const
          ).map((t) => (
            <button
              key={t.key}
              type="button"
              onClick={() => setTab(t.key)}
              aria-label={t.label}
              className={cn(
                'flex size-9 cursor-pointer items-center justify-center rounded-md transition-colors',
                tab === t.key
                  ? 'bg-accent text-accent-foreground'
                  : 'text-muted-foreground hover:text-foreground',
              )}
            >
              <t.icon className="size-5" />
            </button>
          ))}
        </div>
      </PopoverContent>
    </Popover>
  )
}
