import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router'
import { AtSign, BellOff, ChevronRight, Images, LogOut, Pin, FileText, Search, ShieldCheck, UserMinus, UserPlus, Crown, PenLine } from 'lucide-react'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { Separator } from '@/components/ui/separator'
import { Switch } from '@/components/ui/switch'
import { Button } from '@/components/ui/button'
import { Segmented } from '@/components/celira/Segmented'
import { tryGetParticipant } from '@/api/participantDirectory'
import { toParticipant } from '@/api/participantProjection'
import { roomMemberViews } from '@/api/roomMembers'
import { resolveSelfId } from '@/api/selfIdentity'
import type { Participant, ParticipantKind, Room } from '@/api/domain'
import { getApi, useAppStore, useConnection, useRoomMessages } from '@/api/store'
import { supportsMemberPresentations } from '@/api/uiPolicy'
import { roomActionErrorText } from '@/api/contractErrors'
import { attentionForChecked, attentionSwitchChecked } from '@/api/roomAttention'
import { MemberAvatar } from './MemberAvatar'
import { isOnline, isTyping } from '@/api/roomLive'
import { membersWithSelf, roomHeadline, selfMember } from '@/api/roomHeadline'
import { AgentBadge } from './AgentBadge'
import { OfficialBadge } from './OfficialBadge'
import { FileRow } from './FileRow'
import { AttachmentImage } from './AttachmentImage'
import { InviteMemberSheet } from './InviteMemberSheet'
import { AgentPresentationEditSheet } from './AgentPresentationEditSheet'
import { RoomProfileEditSheet } from './RoomProfileEditSheet'
import { canEditRoomProfile } from '@/api/roomProfile'

type DrawerTab = 'members' | 'images' | 'files'

interface RoomInfoDrawerProps {
  room: Room
  open: boolean
  onOpenChange: (open: boolean) => void
  onPreviewImage?: (attachmentId: string) => void
}

const baseTabs: Array<{ key: DrawerTab; label: string }> = [
  { key: 'members', label: '参与者' },
  { key: 'images', label: '图片' },
  { key: 'files', label: '文件' },
]

export function RoomInfoDrawer({ room, open, onOpenChange, onPreviewImage }: RoomInfoDrawerProps) {
  const navigate = useNavigate()
  const [tab, setTab] = useState<DrawerTab>('members')
  const [inviteOpen, setInviteOpen] = useState(false)
  const [editingAgent, setEditingAgent] = useState<string | null>(null)
  const [profileEditOpen, setProfileEditOpen] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const togglePinned = useAppStore((s) => s.togglePinned)
  const toggleMuted = useAppStore((s) => s.toggleMuted)
  const setRoomAttention = useAppStore((s) => s.setRoomAttention)
  const leaveRoom = useAppStore((s) => s.leaveRoom)
  const removeMember = useAppStore((s) => s.removeMember)
  const transferOwnership = useAppStore((s) => s.transferOwnership)
  const refreshRoomMembers = useAppStore((s) => s.refreshRoomMembers)
  const refreshMemberPresentations = useAppStore((s) => s.refreshMemberPresentations)
  const refreshRoom = useAppStore((s) => s.refreshRoom)
  const membersByRoom = useAppStore((s) => s.membersByRoom)
  const presentationByRoom = useAppStore((s) => s.memberPresentationsByRoom[room.id])
  const { me } = useConnection()
  // 成员呈报能力（TASK_72 ⑥）：这台后端真提供它才去拉。
  const presentationsSupported = supportsMemberPresentations(getApi())
  // 提醒档能力探测：`RoomApi.setRoomAttention` 是可选的（与 `uiPolicy` 其余探测同规 ——
  // 按"实现里真的有没有这个方法"判）。没有就不画那个开关：本仓最恨的就是拨了没反应的按钮。
  const attentionSupported = typeof getApi()?.setRoomAttention === 'function'

  const coreMembers = room.id ? (membersByRoom[room.id] ?? null) : null
  const selfId = me?.participant.id ?? ''
  // 演示通道的成员投影里，「我」才是 owner。TASK_56：判据收敛到 `resolveSelfId`
  // —— 真实世界无身份时返空 ⇒ 不把任何 fixture 身份认成群主（真实角色另有来源：
  // 上面的 `coreMembers.membership.role`）。
  const fixtureSelfId = resolveSelfId(me?.participant?.id)
  // 成员**来源**走 `roomMemberViews` 同一份裁决（旧实现这里还有第四份三选一，且用
  // `room.participants ?? participantIds.map(getParticipant)`：空数组不穿透 + 查不到就抛）。
  // ⚠ 本组件产出的**成员行**（带 `role` / `official`）仍是**另一个事实**（角色与呈报），
  // 不在本刀范围内 —— 这里只统一"从哪儿取成员"。
  const fixtureMembers = roomMemberViews({ room }).map((p) => ({
    id: p.id,
    kind: p.kind,
    displayName: p.displayName,
    role: p.id === fixtureSelfId ? 'owner' : 'member',
    official: p.official,
  }))
  const members =
    coreMembers != null && coreMembers.length > 0
      ? coreMembers.map((m) => {
          const fixture = fixtureMembers.find((f) => f.id === m.participant.id)
          return {
            id: m.participant.id,
            kind: m.participant.kind as 'human' | 'agent',
            displayName: m.participant.display_name,
            role: m.membership.role as 'owner' | 'member',
            official: fixture?.official,
          }
        })
      : fixtureMembers
  const myRole = members.find((m) => m.id === selfId)?.role ?? (members[0]?.id === selfId ? 'owner' : 'member')
  const isOwner = myRole === 'owner'
  const isGroup = room.kind === 'group'
  // 群简介（`CORE_V1_ROOM_PROFILE.md`）：只有 group 房有这一格。
  // 写能力按"传输层真的实现了 putRoomProfile"探（与 uiPolicy 同规，不按 mode 判）；
  // 读只要成员就能读（`GET /v1/rooms/{id}` 的可见性裁决在 Core）。
  const profileSupported = typeof getApi()?.putRoomProfile === 'function'
  const profileEditable = canEditRoomProfile({ kind: room.kind, isOwner, supported: profileSupported })
  // ⚠ **成员列表含我**（2026-10-14 主人裁：人数一律含我 —— 「0 Human」从此不可能出现）。
  // 来源只给对端时（`direct` 房自带的 `participants`）我不在里面，这里补一行；
  // 角色给 `member`：**没有权威来源时不猜**（服务端给了成员表时我本来就在里面，走不到这条路）。
  // 合成的那一行也拿不到「转让 / 移除」按钮 —— 那些动作本来就写着 `m.id !== selfId`。
  const self = selfMember(me)
  const allMembers = membersWithSelf(
    members,
    self
      ? {
          ...self,
          // `kind` 在契约里是字符串、在呈现层是 `ParticipantKind`：这一处是**唯一的翻译点**
          // （服务端今天只会给 human / agent；给别的值就落到 `human` 那一侧，与本组件
          // 其余地方的窄化同款 —— 不猜第三种）。
          kind: (self.kind === 'agent' ? 'agent' : 'human') as ParticipantKind,
          role: 'member' as const,
          official: undefined,
        }
      : null,
  )
  const authoritativeMembers = coreMembers != null && coreMembers.length > 0
  // 主体进行态（在不在 / 正在写）：**易失投影**，只用于成员行上那个点与
  // 那三个字。判据走唯一裁决面 `api/roomLive.ts`（人与 Agent 同形）。
  const live = useAppStore((s) => s.roomLiveByRoom[room.id])
  // 标题下那句话与 `ChatHeader` **同一句**：唯一裁决面在 `api/roomHeadline.ts`。
  // 分解（几个 Agent / 几个人）不进句子 —— 它的家就是下面的成员列表
  // （「Agent · N」「人 · N」两组，每行一枚徽标）。
  const headline = roomHeadline({
    kind: room.kind,
    members: allMembers,
    self,
    authoritative: authoritativeMembers,
    snapshot: live,
  })
  const agents = allMembers.filter((m) => m.kind === 'agent')
  const humans = allMembers.filter((m) => m.kind === 'human')
  const presentationById = new Map(
    (presentationByRoom ?? []).map((p) => [p.participant.id, p]),
  )
  const presentNameOf = (memberId: string): string | null =>
    presentationById.get(memberId)?.room_display_name ?? null
  const editingMember = editingAgent ? agents.find((a) => a.id === editingAgent) : undefined
  const editingParticipant: Participant | undefined = editingMember
    ? toParticipant({ id: editingMember.id, kind: editingMember.kind, display_name: editingMember.displayName })
    : undefined
  const tabs = baseTabs
  const roomMessages = useRoomMessages(room.id)
  const images = roomMessages.flatMap((m) =>
    m.system ? [] : (m.attachments ?? []).filter((a) => a.kind === 'image' && a.url),
  )
  const files = roomMessages.flatMap((m) =>
    m.system ? [] : (m.attachments ?? []).filter((a) => a.kind === 'file'),
  )

  const runAction = async (fn: () => Promise<void>) => {
    setActionError(null)
    try {
      await fn()
    } catch (e) {
      setActionError(roomActionErrorText(e))
    }
  }

  const exitRoom = () => {
    // **两条路都走 Core 的 leave**（group 与 direct / human_direct 同一件事：
    // 「我离开这间房」）。owner 需先转让，409 由 Core 裁决后如实呈现；绝不本地删。
    //
    // ⚠ 2026-09-25：非 group 那一支原先调 `deleteRoom`，而那条 store 动作问的是
    // `ChatApi.deleteRoom`——**零实现** ⇒ 房间根本没被删，只是把用户导航走（看着像删了）。
    // **Core v1 没有房间删除端点**，架构红线也明写「不引入房间删除/状态/改名」
    // ⇒ 这里不该假装有那个能力：统一走 leave，房间进入「你已离开」的正常状态。
    // （会话右键菜单里那一项「删除会话」同因已移除 —— 它比这一支更坏：连导航都替用户做了。）
    void runAction(async () => {
      await leaveRoom(room.id)
      navigate('/app/chat')
    })
  }

  // Sheet 的 onOpenChange 只响应用户交互（遮罩/Esc），prop 打开不触发——用 effect 保证加载
  useEffect(() => {
    // TASK_72 ⑥ 收尾：原判据 `mode === 'core'` **恒真** ⇒ 演示通道也会去拉成员呈报
    // （store 那边会 no-op，但「要不要拉」这件事本来就该问能力）。
    if (open && presentationsSupported && isGroup) void refreshMemberPresentations(room.id)
  }, [open, presentationsSupported, isGroup, room.id, refreshMemberPresentations])

  const handleOpenChange = (next: boolean) => {
    onOpenChange(next)
    if (next) {
      void refreshRoom(room.id)
      void refreshRoomMembers(room.id)
    }
  }

  return (
    <Sheet open={open} onOpenChange={handleOpenChange}>
      <SheetContent className="flex w-80 flex-col gap-0 p-0">
        <SheetHeader className="border-b border-border px-5 py-4 text-left">
          <SheetTitle className="text-title">{room.name}</SheetTitle>
          <SheetDescription className="text-caption">
            {headline}
          </SheetDescription>
          <Segmented
            label="房间信息"
            value={tab}
            onChange={setTab}
            options={tabs.map((t) => ({ id: t.key, label: t.label }))}
            className="mt-2"
          />
        </SheetHeader>

        <button
          type="button"
          onClick={() => {
            onOpenChange(false)
            navigate(`/app/chat/${room.id}/history`)
          }}
          className="flex w-full cursor-pointer items-center gap-2 border-b border-border px-5 py-3 text-body transition-colors hover:bg-muted"
        >
          <Search className="size-4 text-muted-foreground" />
          查找聊天记录
          <ChevronRight className="ml-auto size-4 text-muted-foreground" />
        </button>

        <button
          type="button"
          onClick={() => {
            onOpenChange(false)
            navigate('/settings/appearance?wallpaper=room')
          }}
          className="flex w-full cursor-pointer items-center gap-2 border-b border-border px-5 py-3 text-body transition-colors hover:bg-muted"
        >
          <Images className="size-4 text-muted-foreground" />
          当前会话壁纸
          <ChevronRight className="ml-auto size-4 text-muted-foreground" />
        </button>

        <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-5 py-4">
          {/*
            **群简介**（契约 `CORE_V1_ROOM_PROFILE.md` §4.1）：只有 group 房有这一格，
            放在三个页签的内容之上 —— 不新开 tab，成员切到图片/文件时也仍看得到群是什么。
            它**不是**消息、不是成员资料，所以不参与任何列表、未读或通知。

            ⚠ `undefined`（还没读到）时**什么都不说**：`null` 才是 Core 明确答的
            「这间群没有简介」。把"还没读到"写成「还没有群简介」是这个仓库治过很多次的假话
            （`uiPolicy.chatFeedState` 同一纪律）。
          */}
          {isGroup && (
            <section
              aria-labelledby="room-profile-heading"
              className="space-y-2 rounded-lg border border-border bg-card px-3 py-3"
            >
              <div className="flex items-center justify-between gap-2">
                <h3
                  id="room-profile-heading"
                  className="text-micro tracking-wider text-muted-foreground uppercase"
                >
                  群简介
                </h3>
                {profileEditable && (
                  <Button
                    variant="ghost"
                    size="sm"
                    className="gap-1 px-2 text-caption"
                    onClick={() => setProfileEditOpen(true)}
                    aria-label="编辑群简介"
                  >
                    <PenLine className="size-3.5" />
                    编辑
                  </Button>
                )}
              </div>
              {room.profile ? (
                <p className="text-body break-words whitespace-pre-wrap text-muted-foreground">
                  {room.profile.description}
                </p>
              ) : room.profile === null ? (
                <p className="text-caption text-muted-foreground">还没有群简介</p>
              ) : null}
            </section>
          )}
          {tab === 'members' && (
            <>
              {isGroup && isOwner && (
                <Button
                  variant="outline"
                  size="sm"
                  className="w-full gap-1.5"
                  onClick={() => setInviteOpen(true)}
                >
                  <UserPlus className="size-4" />
                  邀请成员
                </Button>
              )}
              {agents.length > 0 && (
                <section>
                  {/* 这两行小标题是**用户可见面**：`Agent` 是本产品的词（头像徽标也这么写），
                      而 `Human` 是契约里 `kind` 的取值 —— 界面上说「人」。
                      （2026-10-20：与头部那一行同一刀，别让契约词漏到屏幕上。） */}
                  <div className="mb-2 text-micro tracking-wider text-muted-foreground uppercase">
                    Agent · {agents.length}
                  </div>
                  <ul className="space-y-2">
                    {agents.map((m) => {
                      const presentName = presentNameOf(m.id)
                      const pres = presentationById.get(m.id)
                      return (
                        <li key={m.id} className="flex flex-col gap-1">
                          <div className="flex items-center gap-2.5">
                            <MemberAvatar roomId={room.id} member={membersDetail(m.id) ?? toParticipant({ id: m.id, kind: m.kind, display_name: m.displayName })} />
                            <span className="min-w-0 flex-1 truncate text-body">
                              {presentName ?? m.displayName}
                            </span>
                            {/* 在不在：**连接在不在**（不是"在不在家"——那是执行端的事）。
                                点只对"在线"点亮；读不到进行态时**不画**任何点，
                                绝不猜、绝不用户口状态冒充。 */}
                            {isOnline(live, m.id) ? (
                              <span
                                className="size-2 shrink-0 rounded-full bg-presence-online"
                                role="img"
                                aria-label="在线"
                              />
                            ) : null}
                            {isTyping(live, m.id) ? (
                              <span className="shrink-0 text-micro text-muted-foreground">
                                正在写…
                              </span>
                            ) : null}
                            {m.role === 'owner' ? <OwnerBadge /> : null}
                            {isGroup && isOwner && (
                              <>
                                <Button
                                  variant="ghost"
                                  size="sm"
                                  className="size-7 p-0"
                                  onClick={() => setEditingAgent(m.id)}
                                  aria-label={`编辑 ${m.displayName} 的群介绍`}
                                  title={`编辑 ${m.displayName} 的群介绍`}
                                >
                                  <PenLine className="size-3.5" />
                                </Button>
                                <Button
                                  variant="ghost"
                                  size="sm"
                                  className="size-7 p-0"
                                  onClick={() =>
                                    void runAction(async () => {
                                      // 群主转让的判据是 role，不看 kind：人类群主
                                      // 可以把群交给房里的 Agent（与人类成员同一条路）。
                                      await transferOwnership(room.id, m.id, undefined)
                                    })
                                  }
                                  aria-label={`转让群主给 ${m.displayName}`}
                                  title={`转让群主给 ${m.displayName}`}
                                >
                                  <Crown className="size-3.5" />
                                </Button>
                              </>
                            )}
                            {m.official ? <OfficialBadge /> : <AgentBadge />}
                          </div>
                          {(pres?.short_description || pres?.room_role_name || pres?.room_role_description_preview) && (
                            <p className="pl-[calc(2.5rem+0.625rem)] text-caption text-muted-foreground">
                              {pres?.short_description ?? ''}
                              {pres?.short_description && (pres?.room_role_name || pres?.room_role_description_preview) ? ' · ' : null}
                              {pres?.room_role_name ? <span className="text-agent">{pres.room_role_name}</span> : null}
                              {pres?.room_role_name && pres?.room_role_description_preview ? ' · ' : null}
                              {pres?.room_role_description_preview ?? ''}
                            </p>
                          )}
                        </li>
                      )
                    })}
                  </ul>
                </section>
              )}
              <section>
                <div className="mb-2 text-micro tracking-wider text-muted-foreground uppercase">
                  人 · {humans.length}
                </div>
                <ul className="space-y-2">
                  {humans.map((m) => (
                    <li key={m.id} className="flex items-center gap-2.5">
                      <MemberAvatar roomId={room.id} member={membersDetail(m.id) ?? toParticipant({ id: m.id, kind: m.kind, display_name: m.displayName })} />
                      <span className="text-body">{m.displayName}</span>
                      {m.role === 'owner' ? <OwnerBadge /> : null}
                      {isGroup && isOwner && m.id !== selfId && (
                        <span className="ml-auto flex items-center gap-1">
                          {m.kind === 'human' && (
                            <Button
                              variant="ghost"
                              size="sm"
                              className="gap-1 px-2 text-caption"
                              onClick={() =>
                                void runAction(async () => {
                                  // 幂等 key 由 core 侧自生成（`RoomApi` 契约：可选参数，
                                  // core 忽略调用方传的）；Host 那套「按逻辑转让复用
                                  // 并轮换内存 key」的纪律已随 TASK_64 删除。
                                  await transferOwnership(room.id, m.id, undefined)
                                })
                              }
                            >
                              <Crown className="size-3.5" />
                              转让
                            </Button>
                          )}
                          <Button
                            variant="ghost"
                            size="sm"
                            className="gap-1 px-2 text-caption text-destructive"
                            onClick={() => void runAction(() => removeMember(room.id, m.id))}
                          >
                            <UserMinus className="size-3.5" />
                            移除
                          </Button>
                        </span>
                      )}
                    </li>
                  ))}
                </ul>
              </section>
              {actionError && <p className="text-caption text-presence-failed">{actionError}</p>}
            </>
          )}

          {tab === 'images' && (
            <section>
              {images.length > 0 ? (
                <div className="grid grid-cols-3 gap-1.5">
                  {images.map((a) => (
                    <button
                      key={a.id}
                      type="button"
                      onClick={() => onPreviewImage?.(a.id)}
                      aria-label={`预览图片 ${a.name}`}
                      className="cursor-pointer overflow-hidden rounded-lg border border-border transition-opacity hover:opacity-85"
                    >
                      <AttachmentImage
                        attachment={a}
                        roomId={room.id}
                        alt={a.name}
                        className="aspect-square w-full object-cover"
                      />
                    </button>
                  ))}
                </div>
              ) : (
                <div className="flex flex-col items-center gap-2 py-10 text-center">
                  <Images className="size-5 text-muted-foreground" aria-hidden />
                  <span className="text-caption text-muted-foreground">这个会话还没有图片</span>
                </div>
              )}
            </section>
          )}

          {tab === 'files' && (
            <section>
              {files.length > 0 ? (
                <ul className="space-y-1.5">
                  {files.map((a) => (
                    <li
                      key={a.id}
                      className="flex items-center gap-2.5 rounded-lg border border-border bg-card px-3 py-2.5"
                    >
                      <FileRow attachment={a} roomId={room.id} />
                    </li>
                  ))}
                </ul>
              ) : (
                <div className="flex flex-col items-center gap-2 py-10 text-center">
                  <FileText className="size-5 text-muted-foreground" aria-hidden />
                  <span className="text-caption text-muted-foreground">这个会话还没有文件</span>
                </div>
              )}
            </section>
          )}

          {/*
            **置顶 / 免打扰**（2026-09-25 归位）：用户侧偏好，只写本机存储（`api/roomPrefs.ts`
            经 `api/storage.ts` 窄口），不发任何请求 —— 见那个文件头部为什么它们**不该**问 Core。

            ⚠ 这里治过一次真实的静默失效：两个开关原先问
            `ChatApi.togglePinned` / `toggleMuted`——**两个零实现的方法** ⇒ 拨了没反应、
            自己弹回去。根因不是"Core 缺端点"，是**问错了地方**。
          */}
          {tab === 'members' && (
            <>
              <Separator />
              <section className="space-y-1">
                <label className="flex cursor-pointer items-center gap-3 rounded-lg px-2 py-2.5 transition-colors hover:bg-muted">
                  <Pin className="size-4 text-muted-foreground" />
                  <span className="text-body">置顶会话</span>
                  <Switch
                    className="ml-auto"
                    aria-label="置顶会话"
                    checked={room.pinned ?? false}
                    onCheckedChange={() => void togglePinned(room.id)}
                  />
                </label>
                <label className="flex cursor-pointer items-center gap-3 rounded-lg px-2 py-2.5 transition-colors hover:bg-muted">
                  <BellOff className="size-4 text-muted-foreground" />
                  <span className="text-body">消息免打扰</span>
                  <Switch
                    className="ml-auto"
                    aria-label="消息免打扰"
                    checked={room.muted ?? false}
                    onCheckedChange={() => void toggleMuted(room.id)}
                  />
                </label>
                {/*
                  **提醒档**（契约 `CORE_V1_IM_CONTRACT.md` §9.1）：与上面那一格**分属两层** ——
                  「消息免打扰」是本机的完全静音（`api/roomPrefs.ts`，压过一切），
                  而这一格是**世界侧**的个人事实（`PUT /v1/rooms/{id}/attention`，换设备跟着走）。
                  只有 group 房有这一档（两人房里没有「群消息」）。开关的 `checked` / 目标值
                  都由 `api/roomAttention.ts` 投影，组件不自己比较字符串。
                */}
                {isGroup && attentionSupported && (
                  <>
                    <label className="flex cursor-pointer items-center gap-3 rounded-lg px-2 py-2.5 transition-colors hover:bg-muted">
                      <AtSign className="size-4 text-muted-foreground" />
                      <span className="text-body">只接受 @ 我的消息</span>
                      <Switch
                        className="ml-auto"
                        aria-label="只接受 @ 我的消息"
                        checked={attentionSwitchChecked(room.attention)}
                        onCheckedChange={(checked) =>
                          void runAction(async () => {
                            // store 负责乐观更新 + 失败回退；失败的那句话由 `runAction` 显示。
                            await setRoomAttention(room.id, attentionForChecked(checked))
                          })
                        }
                      />
                    </label>
                    <p className="px-2 text-caption text-muted-foreground">
                      只有 @ 你的消息会提醒；「消息免打扰」仍会完全静音。
                    </p>
                  </>
                )}
              </section>
            </>
          )}
        </div>

        {tab === 'members' && (
          <div className="border-t border-border p-4">
            <Button
              variant={isGroup ? 'outline' : 'destructive'}
              className="w-full"
              onClick={exitRoom}
            >
              <LogOut className="size-4" />
              {isGroup
                ? isOwner
                  ? '转让群主后可退出'
                  : '退出群聊'
                : '离开房间'}
            </Button>
          </div>
        )}
      </SheetContent>
      {isGroup && (
        <InviteMemberSheet
          roomId={room.id}
          currentMemberIds={members.map((m) => m.id)}
          open={inviteOpen}
          onOpenChange={setInviteOpen}
        />
      )}
      {editingParticipant && (
        <AgentPresentationEditSheet
          roomId={room.id}
          agent={editingParticipant}
          open={editingAgent !== null}
          onOpenChange={(next) => !next && setEditingAgent(null)}
        />
      )}
      {isGroup && profileEditable && (
        <RoomProfileEditSheet
          room={room}
          open={profileEditOpen}
          onOpenChange={setProfileEditOpen}
        />
      )}
    </Sheet>
  )
}

function OwnerBadge() {
  return (
    <span className="inline-flex items-center gap-1 rounded-full border border-border bg-muted px-2 py-0.5 text-micro text-muted-foreground">
      <ShieldCheck className="size-3" />
      群主
    </span>
  )
}

// 目录查询走既有权威 `tryGetParticipant`（查不到返 null），不再手搓一层 try/catch。
function membersDetail(id: string) {
  return tryGetParticipant(id) ?? undefined
}
