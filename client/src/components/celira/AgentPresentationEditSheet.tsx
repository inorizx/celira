import { useCallback, useEffect, useState } from 'react'
import { Save } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { useAppStore } from '@/api/store'
import { roomPresentationErrorMessage } from '@/api/presentation'
import { presentationFieldError } from '@/api/uiPolicy'
import { AgentBadge } from './AgentBadge'
import { ParticipantAvatar } from './ParticipantAvatar'
import type { Participant } from '@/api/domain'

interface AgentPresentationEditSheetProps {
  roomId: string
  agent: Participant
  open: boolean
  onOpenChange: (open: boolean) => void
}

// 群内 Agent 成员介绍编辑（CORE_V1_PARTICIPANT_PRESENTATION.md §2.2/§5/§7）。
// 先 GET 详情（presentation_version 作为保存的 base_version，null => 0），保存
// PUT 的 body 是本地草稿，绝不持久化到浏览器；幂等 key 由 core 侧自生成。
//
// 409 stale：给稳定文案并触发权威重读最新版本，绝不静默覆盖。
// direct / human_direct Room 无编辑入口（由调用方保证），本组件不渲染异常。
export function AgentPresentationEditSheet({
  roomId,
  agent,
  open,
  onOpenChange,
}: AgentPresentationEditSheetProps) {
  const updateRoomPresentation = useAppStore((s) => s.updateRoomPresentation)
  const getRoomPresentation = useAppStore((s) => s.getRoomPresentation)
  const [baseVersion, setBaseVersion] = useState(0)
  const [displayName, setDisplayName] = useState('')
  const [roleName, setRoleName] = useState('')
  const [roleDescription, setRoleDescription] = useState('')
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [saved, setSaved] = useState(false)

  const loadDetail = useCallback(() => {
    setLoading(true)
    setError(null)
    setBaseVersion(0)
    setDisplayName('')
    setRoleName('')
    setRoleDescription('')
    void getRoomPresentation(roomId, agent.id)
      .then((detail) => {
        // 详情是版本裁决的唯一事实源；读取成功后才更新版本与表单。
        setBaseVersion(detail.presentation?.presentation_version ?? 0)
        setDisplayName(detail.presentation?.room_display_name ?? '')
        setRoleName(detail.presentation?.room_role_name ?? '')
        setRoleDescription(detail.presentation?.room_role_description ?? '')
      })
      .catch((e) => {
        // 读取失败时禁止保存，绝不把版本猜成 0。
        setError(roomPresentationErrorMessage(e))
      })
      .finally(() => setLoading(false))
  }, [roomId, agent.id, getRoomPresentation])

  useEffect(() => {
    if (!open) return
    setError(null)
    setSaved(false)
    loadDetail()
  }, [open, loadDetail])

  const save = async () => {
    if (saving) return
    const fieldError = presentationFieldError({
      roomDisplayName: displayName,
      roomRoleName: roleName,
      roomRoleDescription: roleDescription,
    })
    if (fieldError) {
      setError(fieldError)
      return
    }
    setSaving(true)
    setError(null)
    setSaved(false)
    try {
      await updateRoomPresentation(
        roomId,
        agent.id,
        {
          baseVersion,
          roomDisplayName: displayName.trim() || null,
          roomRoleName: roleName.trim() || null,
          roomRoleDescription: roleDescription.trim() || null,
        },
        undefined,
      )
      setSaved(true)
      setTimeout(() => onOpenChange(false), 600)
    } catch (e) {
      const code = e && typeof e === 'object' && 'code' in e ? String(e.code) : ''
      const text = e instanceof Error ? e.message : String(e)
      setError(text)
      if (code === 'room_presentation_stale') {
        // 过期 base_version：稳定文案已展示，权威重读最新版本再让用户保存。
        loadDetail()
      }
    } finally {
      setSaving(false)
    }
  }

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="w-full gap-0 p-0 sm:max-w-md">
        <SheetHeader className="border-b border-border px-5 py-4 text-left">
          <SheetTitle className="flex items-center gap-3 text-title">
            <ParticipantAvatar participant={agent} size="sm" />
            <span className="min-w-0 truncate">{agent.displayName}</span>
            <AgentBadge />
          </SheetTitle>
          <SheetDescription className="text-caption">
            当前群主编辑这位 Agent 在本群内的称呼与职责；仅群成员可见。
          </SheetDescription>
        </SheetHeader>
        <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-5 py-4">
          <label className="block space-y-1.5">
            <span className="text-caption text-muted-foreground">在本群的称呼</span>
            <Input
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
              placeholder="留空则使用默认名字"
              disabled={saving || loading}
              className="bg-muted"
            />
          </label>
          <label className="block space-y-1.5">
            <span className="text-caption text-muted-foreground">职责角色名</span>
            <Input
              value={roleName}
              onChange={(e) => setRoleName(e.target.value)}
              placeholder="例如：消息契约守门人"
              disabled={saving || loading}
              className="bg-muted"
            />
          </label>
          <label className="block space-y-1.5">
            <span className="text-caption text-muted-foreground">职责描述</span>
            <Textarea
              value={roleDescription}
              onChange={(e) => setRoleDescription(e.target.value)}
              placeholder="TA 在这个群里负责什么、适合怎样协作。"
              disabled={saving || loading}
              className="bg-muted"
              rows={4}
            />
          </label>
        </div>
        <div className="border-t border-border p-4">
          {loading && <p className="text-caption text-muted-foreground">正在读取详情…</p>}
          {error && <p className="text-caption text-presence-failed">{error}</p>}
          {saved && <p className="text-caption text-presence-online">已保存</p>}
          <div className="flex items-center justify-end gap-2">
            <Button variant="ghost" onClick={() => onOpenChange(false)} disabled={saving}>
              取消
            </Button>
            <Button onClick={() => void save()} disabled={saving || loading || saved || error !== null}>
              <Save className="size-4" />
              {saving ? '保存中…' : '保存介绍'}
            </Button>
          </div>
        </div>
      </SheetContent>
    </Sheet>
  )
}