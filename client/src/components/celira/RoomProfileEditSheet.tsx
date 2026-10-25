import { useEffect, useState } from 'react'
import { Save } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { cn } from '@/lib/utils'
import { useAppStore } from '@/api/store'
import { errorText } from '@/api/contractErrors'
import {
  ROOM_PROFILE_MAX_BYTES,
  isProfileVersionConflict,
  roomProfileByteLength,
  roomProfileDraftError,
} from '@/api/roomProfile'
import type { Room } from '@/api/domain'

interface RoomProfileEditSheetProps {
  room: Room
  open: boolean
  onOpenChange: (open: boolean) => void
}

// 群简介编辑（契约 `CORE_V1_ROOM_PROFILE.md` §3；2026-10-25 客户端接入）。
//
// 三条行为是这一层的**全部**职责，其余裁决都在 Core：
// - `baseVersion` 取权威读面的 `room.profile?.version ?? 0`（首次写 0），本地绝不 +1；
// - 正文按**UTF-8 字节**计数（`roomProfileByteLength`），超 2000 或 trim 后为空时
//   禁用保存并给明确提示 —— 这只是早反馈，Core 仍会再裁决一次；
// - 409 `profile_version_conflict`：权威重读房间 + 稳定文案（「简介已被更新，请刷新后
//   重试」），并把 baseVersion 同步到最新版本 —— 草稿**不丢**，用户再点一次保存才算
//   明确同意覆盖。其余分类（403 / 400 / 网络）只给 `contractErrors` 的固定人话。
//
// 幂等 key 由 `core.ts` 每次生成；本组件不接触它，也不把草稿写进任何持久层。
export function RoomProfileEditSheet({ room, open, onOpenChange }: RoomProfileEditSheetProps) {
  const updateRoomProfile = useAppStore((s) => s.updateRoomProfile)
  const refreshRoom = useAppStore((s) => s.refreshRoom)
  const [description, setDescription] = useState('')
  const [baseVersion, setBaseVersion] = useState(0)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [conflict, setConflict] = useState(false)

  // 只在**打开时**播种：409 之后的 `refreshRoom` 会换掉 room 对象，若把
  // `room.profile` 放进依赖里，刚打的草稿会被权威值静默抹掉。
  useEffect(() => {
    if (!open) return
    setDescription(room.profile?.description ?? '')
    setBaseVersion(room.profile?.version ?? 0)
    setError(null)
    setConflict(false)
    setSaving(false)
    // eslint-disable-next-line react-hooks/exhaustive-deps -- 见上：打开那一刻的权威值才是草稿初值
  }, [open, room.id])

  const bytes = roomProfileByteLength(description)
  const overLimit = bytes > ROOM_PROFILE_MAX_BYTES
  const draftError = roomProfileDraftError(description)
  const canSave = !saving && draftError === null
  // 空正文也要说清为什么保存是灰的 —— 只把按钮变灰、不给理由是"静默失效"的另一种写法。
  const showDraftHint = draftError !== null

  const save = async () => {
    if (saving) return // 保存中防重复点击（双击只发一次）
    const invalid = roomProfileDraftError(description)
    if (invalid) {
      setError(invalid)
      return
    }
    setSaving(true)
    setError(null)
    setConflict(false)
    try {
      await updateRoomProfile(room.id, { baseVersion, description })
      onOpenChange(false)
    } catch (e) {
      // 稳定分类 → 固定人话；绝不回显 Core 正文 / key / 路径。
      setError(errorText(e))
      if (isProfileVersionConflict(e)) {
        setConflict(true)
        // 先权威重读（房间简介是公共事实，读面是唯一事实源），再同步 baseVersion；
        // 草稿保留，让用户自己决定是否覆盖刚写的那一版。
        await refreshRoom(room.id)
        const latest = useAppStore.getState().rooms.find((r) => r.id === room.id)
        setBaseVersion(latest?.profile?.version ?? 0)
      }
    } finally {
      setSaving(false)
    }
  }

  return (
    <Sheet
      open={open}
      onOpenChange={(next) => {
        // 保存中不许被遮罩/Esc 关掉：请求已经发出，关掉会让人以为没保存。
        if (!next && saving) return
        onOpenChange(next)
      }}
    >
      <SheetContent className="flex w-full flex-col gap-0 p-0 sm:max-w-md">
        <SheetHeader className="border-b border-border px-5 py-4 text-left">
          <SheetTitle className="text-title">编辑群简介</SheetTitle>
          <SheetDescription className="text-caption">
            这段简介对本群所有成员可见；只有群主可以修改。
          </SheetDescription>
        </SheetHeader>

        <div className="min-h-0 flex-1 space-y-2 overflow-y-auto px-5 py-4">
          <label htmlFor="room-profile-description" className="block text-caption text-muted-foreground">
            群简介
          </label>
          <Textarea
            id="room-profile-description"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="这个群在做什么、适合怎样协作。"
            disabled={saving}
            aria-invalid={draftError !== null || undefined}
            aria-describedby="room-profile-meta room-profile-hint"
            className="min-h-40 bg-muted"
            rows={8}
          />
          <div id="room-profile-meta" className="flex items-baseline justify-between gap-2">
            <span
              className={cn('text-caption tabular-nums', overLimit ? 'text-presence-failed' : 'text-muted-foreground')}
              aria-live="polite"
            >
              {bytes}/{ROOM_PROFILE_MAX_BYTES} 字节
            </span>
            <span className="text-caption text-muted-foreground">支持换行与 Emoji</span>
          </div>
          <p id="room-profile-hint" className="min-h-4 text-caption text-presence-failed" aria-live="polite">
            {showDraftHint ? draftError : ''}
          </p>
        </div>

        <div className="border-t border-border p-4">
          {error && (
            <p className="mb-2 text-caption text-presence-failed" role="alert">
              {error}
            </p>
          )}
          {conflict && (
            <p className="mb-2 text-caption text-muted-foreground">
              已读取最新版本；再次保存会覆盖刚更新的内容。
            </p>
          )}
          <div className="flex items-center justify-end gap-2">
            <Button variant="ghost" onClick={() => onOpenChange(false)} disabled={saving}>
              取消
            </Button>
            <Button onClick={() => void save()} disabled={!canSave}>
              <Save className="size-4" />
              {saving ? '保存中…' : '保存简介'}
            </Button>
          </div>
        </div>
      </SheetContent>
    </Sheet>
  )
}
