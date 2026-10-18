import { useEffect, useRef, useState } from 'react'
import { useNavigate } from 'react-router'
import { Bell, ChevronRight, Copy, ImagePlus, Info, Loader2, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Separator } from '@/components/ui/separator'
import { useMemberAvatarUrl } from '@/hooks/use-member-avatar'
import { myAgentIds, SELF_ID } from '@/fixtures/data'
import { getParticipant } from '@/api/participantDirectory'
import { toParticipant } from '@/api/participantProjection'
import { isRealWorldSession } from '@/api/prefs'
import { AgentBadge } from '@/components/celira/AgentBadge'
import { ParticipantAvatar } from '@/components/celira/ParticipantAvatar'
import { ThemeToggle } from '@/components/celira/ThemeToggle'
import { getApi, useAppStore, useConnection } from '@/api/store'
import { roomActionErrorText } from '@/api/contractErrors'
import type { Participant } from '@/api/domain'

export function ProfilePage() {
  const navigate = useNavigate()
  const { mode, token, me } = useConnection()
  const ownedAgents = useAppStore((s) => s.ownedAgents)
  // **真实世界**（core 直连）与**演示通道**（未连接）的分界，
  // 判据本体只有一份：`api/prefs.ts` 的 `isRealWorldSession`。
  // TASK_56：这一页原先有 8 处 `mode === 'host'` —— 把「真实世界」错写成
  // 「Host 模式」，于是 core 直连的真实用户在「我的」页看到的是：假名字、
  // 硬编码的假身份 ID（`hm_01J8XKM3T2`）、以及 fixtures 里的假 Agent 列表。
  const realWorld = isRealWorldSession({ mode, token })
  const realSelf: Participant | null = me ? toParticipant(me.participant) : null
  // 真身份 / 真 Agent 列表：真实世界一律用真的（读不到就是空，见 `me === null`）；
  // 只有演示通道才拿 fixtures 顶上。
  const self = realWorld ? realSelf : getParticipant(SELF_ID)
  const myAgents = realWorld ? ownedAgents : myAgentIds.map(getParticipant)

  // ── 人自己的资料（表达权第四刀，2026-10-17）──────────────────────────────
  //
  // 两条路**不是同一种写**，所以这里也是两套状态：
  // · **名字**：`display_name` 是既有 Participant 身份事实 ⇒ **状态写**
  //   （无版本号、无幂等键；同值重写天然幂等）；
  // · **头像**：呈现版本链上的一格 ⇒ 带 `base_version` ＋ 幂等键。
  //
  // ⚠ 自己那张脸**不能新开读取面**（Avatar P0 明文：读取只有房间成员那一条）。
  //   所以这里借**我所在的某一间房**读自己 —— 没有共同房间时照实画 fallback，
  //   不发明第二条路径、不造 URL。
  const applyMyName = useAppStore((s) => s.applyMyName)
  const rooms = useAppStore((s) => s.rooms)
  const [nameDraft, setNameDraft] = useState('')
  const [nameBusy, setNameBusy] = useState(false)
  const [nameNotice, setNameNotice] = useState<string | null>(null)
  const [avatarBusy, setAvatarBusy] = useState(false)
  const [avatarNotice, setAvatarNotice] = useState<string | null>(null)
  const [hasAvatar, setHasAvatar] = useState(false)
  const [profileVersion, setProfileVersion] = useState(0)
  const fileRef = useRef<HTMLInputElement | null>(null)

  const selfId = realWorld ? (me?.participant.id ?? null) : null
  // 预览用的房间：我所在的**第一间**（私人房优先，好让"我的脸"有地方可读）。
  const previewRoomId =
    rooms.find((r) => r.kind === 'direct')?.id ?? rooms[0]?.id ?? null
  const myFaceUrl = useMemberAvatarUrl(previewRoomId, selfId)

  // 读一次自己的呈现元数据（头像在不在、版本号多少）——**只读**，不改任何本地事实。
  useEffect(() => {
    if (!realWorld) return
    let cancelled = false
    void (async () => {
      const api = getApi()
      if (!api?.getMyProfile) return
      try {
        const view = await api.getMyProfile()
        if (cancelled) return
        setHasAvatar(!!view.profile?.avatar)
        setProfileVersion(view.profile?.profile_version ?? 0)
      } catch {
        // 读不到就不摆读数（头像那一行照常显示"还没有头像"）。
      }
    })()
    return () => {
      cancelled = true
    }
  }, [realWorld, me?.participant.id])

  const saveName = async () => {
    const api = getApi()
    const next = nameDraft.trim()
    if (!api?.renameMe || !next || nameBusy) return
    setNameBusy(true)
    setNameNotice(null)
    try {
      const res = await api.renameMe(next)
      // 落**世界的回读值**，不是把草稿写进本地（本地那一份不是权威）。
      applyMyName(res.display_name)
      setNameDraft('')
      setNameNotice(`改好了：现在叫「${res.display_name}」`)
    } catch (error) {
      setNameNotice(roomActionErrorText(error))
    } finally {
      setNameBusy(false)
    }
  }

  const uploadAvatar = async (file: File) => {
    const api = getApi()
    if (!api?.putMyAvatar || avatarBusy) return
    setAvatarBusy(true)
    setAvatarNotice(null)
    try {
      const res = await api.putMyAvatar({ baseVersion: profileVersion, file })
      setHasAvatar(!!res.profile.avatar)
      setProfileVersion(res.profile.profile_version)
      setAvatarNotice('换好了。别人在房间里看到的就是这一张。')
    } catch (error) {
      setAvatarNotice(roomActionErrorText(error))
    } finally {
      setAvatarBusy(false)
      if (fileRef.current) fileRef.current.value = ''
    }
  }

  const removeAvatar = async () => {
    const api = getApi()
    if (!api?.deleteMyAvatar || avatarBusy) return
    setAvatarBusy(true)
    setAvatarNotice(null)
    try {
      const res = await api.deleteMyAvatar({ baseVersion: profileVersion })
      setHasAvatar(!!res.profile.avatar)
      setProfileVersion(res.profile.profile_version)
      setAvatarNotice('摘掉了（历史那一张还在世界上，只是不再显示）。')
    } catch (error) {
      setAvatarNotice(roomActionErrorText(error))
    } finally {
      setAvatarBusy(false)
    }
  }

  return (
    <div className="flex h-full flex-col bg-background">
      <header className="border-b border-border bg-card px-4 py-4 md:px-6">
        <div className="text-title">我的</div>
      </header>
      <ScrollArea className="min-h-0 flex-1">
        <div className="mx-auto w-full max-w-3xl space-y-6 px-4 py-6">
          <section className="space-y-4 rounded-xl border border-border bg-card p-5">
            <div className="flex items-center gap-4">
              {self && <ParticipantAvatar participant={self} size="xl" avatarUrl={myFaceUrl} />}
              <div>
                <div className="text-title">{self ? self.displayName : '身份未就绪'}</div>
                <div className="text-caption text-muted-foreground">
                  {realWorld ? 'Human 身份 · 来自服务器' : 'Human 身份 · 本地演示身份'}
                </div>
              </div>
            </div>
            <div className="space-y-1.5 border-t border-border pt-4">
              <div className="flex items-center gap-2 text-caption text-muted-foreground">
                身份 ID
                <code className="rounded bg-muted px-1.5 py-0.5 font-mono">
                  {realWorld ? (me?.participant.id ?? '—') : 'hm_01J8XKM3T2'}
                </code>
                <Copy className="size-3 cursor-pointer transition-colors hover:text-foreground" />
              </div>
              <div className="text-caption text-muted-foreground">
                {realWorld
                  ? me
                    ? `创建于 ${me.participant.created_at.slice(0, 7)} · 服务器上的身份`
                    : '—'
                  : '创建于 2026-08 · 由本设备持有'}
              </div>
            </div>

            {/* ★ **人自己的名字与头像**（表达权第四刀，2026-10-17）：名字是身份事实
                （状态写），头像是呈现版本链上的一格。两个动作、两个入口 —— 不合成一个
                "保存资料"（那是把两件事说成一件事）。演示通道没有世界可写 ⇒ 不摆。 */}
            {realWorld && (
              <div className="space-y-3 border-t border-border pt-4">
                <div className="flex items-center gap-2">
                  <Input
                    value={nameDraft}
                    onChange={(e) => setNameDraft(e.target.value)}
                    placeholder={self ? self.displayName : '新名字'}
                    aria-label="新名字"
                    maxLength={80}
                    className="max-w-xs"
                  />
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={nameBusy || nameDraft.trim().length === 0}
                    onClick={() => void saveName()}
                  >
                    {nameBusy && <Loader2 className="size-3.5 animate-spin" aria-hidden />}
                    改名字
                  </Button>
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  <input
                    ref={fileRef}
                    type="file"
                    accept="image/png,image/jpeg,image/webp"
                    className="hidden"
                    onChange={(e) => {
                      const file = e.target.files?.[0]
                      if (file) void uploadAvatar(file)
                    }}
                  />
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={avatarBusy}
                    onClick={() => fileRef.current?.click()}
                  >
                    {avatarBusy ? (
                      <Loader2 className="size-3.5 animate-spin" aria-hidden />
                    ) : (
                      <ImagePlus className="size-3.5" aria-hidden />
                    )}
                    {hasAvatar ? '换头像' : '加头像'}
                  </Button>
                  {hasAvatar && (
                    <Button
                      variant="ghost"
                      size="sm"
                      disabled={avatarBusy}
                      onClick={() => void removeAvatar()}
                    >
                      <Trash2 className="size-3.5" aria-hidden />
                      摘掉头像
                    </Button>
                  )}
                  <span className="text-micro text-muted-foreground">
                    PNG / JPEG / WebP，≤ 2 MiB（服务端不裁切、不转码）
                  </span>
                </div>
                {nameNotice && (
                  <p className="text-caption text-muted-foreground" role="status">
                    {nameNotice}
                  </p>
                )}
                {avatarNotice && (
                  <p className="text-caption text-muted-foreground" role="status">
                    {avatarNotice}
                  </p>
                )}
                {/* 自己那张脸的读数：借**我所在的房间**读（契约只给这一条读取面）。 */}
                <p className="text-micro text-muted-foreground">
                  {myFaceUrl
                    ? '上面那张就是世界上存着的头像（经房间成员路径读回来的）。'
                    : previewRoomId
                      ? '还没有头像（或者刚换上还没读回来）。头像读取只有**房间成员**那一条路径 —— 换个房间再回来会重新读一次。'
                      : '你还没有任何房间，所以这里看不到自己的那张脸：头像读取只有**房间成员**那一条路径。'}
                </p>
              </div>
            )}
          </section>

          <section className="space-y-2">
            <div className="px-1 text-micro tracking-wider text-muted-foreground uppercase">
              我接入的 Agent
            </div>
            <button
              type="button"
              onClick={() => navigate('/app/agents')}
              className="flex w-full cursor-pointer items-center gap-3 rounded-xl border border-border bg-card px-5 py-4 text-left transition-colors hover:bg-muted"
            >
              <div className="flex -space-x-2">
                {myAgents.map((a) => (
                  <ParticipantAvatar
                    key={a.id}
                    participant={a}
                    size="sm"
                    className="rounded-full ring-2 ring-card"
                  />
                ))}
              </div>
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="text-body">
                    {myAgents.length > 0
                      ? myAgents.map((a) => a.displayName).join('、')
                      : realWorld
                        ? '还没有你的 Agent'
                        : '—'}
                  </span>
                  {myAgents.length > 0 && <AgentBadge />}
                </div>
                <div className="text-caption text-muted-foreground">
                  {`${myAgents.length} 个 · 由我创建，我承担责任`}
                </div>
              </div>
              <ChevronRight className="size-4 shrink-0 text-muted-foreground" />
            </button>
          </section>

          {/* ⚠ 刀2b 撤掉了这里原先的「运行」快捷入口。它指向那个**旧地址**
              （运行状态页的老路径，如今只是重定向到 `/app/runtimes` 机器列表）。
              2026-09-26 修订本条理由：原写「运行状态只在某一个 Agent 的主页里说」——
              那句在 2026-09-25 组织轴刀之后方向反了（现在主语是**机器**）。
              现在这条约束是：「我的」页是**关于我**的地方，机器读数归壳层「执行端」组，
              不在这里再开第二条路。路由本体照旧保留（`navGroups.ts` 是导航的唯一事实源）。
              ⚠ 别在本文件里写出那条**旧路径的字面量**：`tests/nav-groups.test.ts` 按源码扫它
              （连注释也算，与 `check-voice.mjs` 同一口径），写出来就红。 */}

          {/* ⚠ 2026-09-28：这里原先还有一张「服务」卡（「我的服务」⇒ `/app/services`）。
              那一页随 **Agent 服务层整体退役**（Publication / Access / 服务房）删除，
              卡与入口一并退场。 */}

          <section className="divide-y divide-border rounded-xl border border-border bg-card">
            <div className="flex items-center gap-3 px-5 py-4">
              <span className="text-body">外观</span>
              <span className="ml-auto text-caption text-muted-foreground">深色 / 浅色</span>
              <ThemeToggle />
            </div>
            <button
              type="button"
              onClick={() => navigate('/settings')}
              className="flex w-full cursor-pointer items-center gap-3 px-5 py-4 text-left transition-colors hover:bg-muted"
            >
              <Bell className="size-4 text-muted-foreground" />
              <span className="text-body">通知与设置</span>
              <ChevronRight className="ml-auto size-4 text-muted-foreground" />
            </button>
            <button
              type="button"
              // ★ **指对页**（2026-10-11）：从前它指 `/settings`（设置首页）⇒ 点「关于 Celira」
              // 落在**通用**那一页上 ✗。名字说的是哪一页，就去哪一页。
              onClick={() => navigate('/settings/about')}
              className="flex w-full cursor-pointer items-center gap-3 px-5 py-4 text-left transition-colors hover:bg-muted"
            >
              <Info className="size-4 text-muted-foreground" />
              <span className="text-body">关于 Celira</span>
              <ChevronRight className="ml-auto size-4 text-muted-foreground" />
            </button>
          </section>

          <Separator />
          <p className="text-center text-caption text-muted-foreground">
            你对自己的 Agent 承担创建、授权与撤销责任。
          </p>
        </div>
      </ScrollArea>
    </div>
  )
}
