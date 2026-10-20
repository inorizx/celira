import { useState } from 'react'
import { ArrowLeft, Phone, Search, Users, Video } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { roomMemberViews } from '@/api/roomMembers'
import { liveHeadline } from '@/api/roomLive'
import { groupHeadline, memberRoll, peerPresence, presenceLabel, selfMember } from '@/api/roomHeadline'
import { callEntryFor } from '@/api/uiPolicy'
import type { Participant, Room } from '@/api/domain'
import { useAppStore } from '@/api/store'
import { MemberAvatar } from './MemberAvatar'
interface ChatHeaderProps {
  room: Room
  onBack?: () => void
  onOpenInfo?: () => void
  onOpenMembers?: () => void
  // 放大镜入口：跳转「查找聊天记录」页（关键词/类型筛选 + 定位跳回）
  onOpenHistory?: () => void
}

type CallHint = 'audio' | 'video' | null

export function ChatHeader({
  room,
  onBack,
  onOpenInfo,
  onOpenMembers,
  onOpenHistory,
}: ChatHeaderProps) {
  const [callHint, setCallHint] = useState<CallHint>(null)
  const coreMembers = useAppStore((s) => s.membersByRoom[room.id])
  // 主体进行态（在不在 / 正在写）：**易失投影**，只用于这一行的显示。
  // 它不参与任何持久判断，也不代表"这个人/Agent 在不在家"——身体态归执行端
  // （`/app/runtimes`），这里只说 IM 里该说的那两句。
  const live = useAppStore((s) => s.roomLiveByRoom[room.id])
  const me = useAppStore((s) => s.me)
  const myId = me?.participant.id
  const startCall = useAppStore((s) => s.startCall)
  // 成员投影：走 `roomMemberViews` 唯一裁决面（core 成员投影 → 房间自带成员 → 目录兜底）。
  // **不写 presence** —— `ParticipantMinView.status` 是**户口状态**（account active/revoked），
  // 不是"在线"；拿它冒充在线会让每个活着的成员永远亮绿点。旧实现这里第三条来源用
  // `getParticipant`（查不到就抛）——收口后解析不出的人是**跳过**，异常不再有机会进渲染路径。
  const members: Participant[] = roomMemberViews({ core: coreMembers, room })
  // 成员读数**只从这一处算**（`api/roomHeadline.ts`：人数含我、在线不含我、读不到≠没有）。
  // ⚠ 从前这里自己数一遍 `members.length / human / agent`，而成员来源在"世界的成员表"与
  // "房间自带的对端"之间切换 ⇒ 同一间 1:1 房会先显示「1 位参与者 · 0 Human · 1 Agent」、
  // 加载后又显示「2 位参与者 · 1 Human · 1 Agent」（2026-10-14 真机读数）。
  const authoritativeMembers = coreMembers != null && coreMembers.length > 0
  const roll = memberRoll(members, selfMember(me), authoritativeMembers)
  // 1:1（`direct` / `human_direct`）：人数三连不承载信息（那间房只有你和对方），只报对端状态。
  const isOneToOne = room.kind === 'direct' || room.kind === 'human_direct'

  // 进行态的两个派生读数走**唯一裁决面**（`api/roomLive.ts`，有 `node --test`
  // 直测）：不含自己——自己的在线/进行态对自己没有信息量。
  const { onlineCount, typingNames } = liveHeadline(live, myId)

  const showCallHint = (kind: Exclude<CallHint, null>) => {
    setCallHint(kind)
    setTimeout(() => setCallHint(null), 1600)
  }

  /**
   * **通话入口**（`CORE_V1_CALL.md` 窄解冻切片，2026-10-09 接线）：媒体是两端之间的
   * WebRTC P2P、信令走 `/v1/call/stream`（与唤醒流**是两条流**），Core 永不接触媒体字节。
   *
   * ⚠ **给不给入口的判据只有一处**（`callEntryFor`，主人 2026-10-09 判断："电话只人跟人"）：
   * 对端是 Agent 或群房 ⇒ **一个入口都不给**（实时语音要求两端各有采集/播放那一面，
   * 而执行端今天没有 —— 给了就是"按下去对面永远没人接"的假入口）；对端是人 ⇒ 语音是真入口。
   * ⚠ 措辞纪律（与 `FixturePageUnavailable` 同源）：**不承诺"即将提供"** ——
   * "即将"是对未来的承诺，"尚未"才是**当下的事实**。
   */
  const callEntry = callEntryFor(room, myId ?? '')
  const startVoiceCall = () => {
    if (callEntry.audio !== 'call' || !room.peer) return
    void startCall(room.id, room.peer.id)
  }
  return (
    <header className="border-b border-border bg-card px-4 py-4 md:px-6">
      <div className="mx-auto flex w-full max-w-5xl 2xl:max-w-6xl items-center gap-3">
      {onBack && (
        <Button
          variant="ghost"
          size="icon"
          onClick={onBack}
          aria-label="返回会话列表"
          className="md:hidden"
        >
          <ArrowLeft className="size-4" />
        </Button>
      )}
      <button
        type="button"
        onClick={onOpenInfo}
        className="min-w-0 cursor-pointer rounded-lg px-2 py-1 text-left transition-colors hover:bg-muted"
      >
        <div className="truncate text-title">{room.name}</div>
        {/* 头部报**静态参与者统计** + **主体进行态**两件事，仅此而已：
            谁在线（连接在不在）、谁正在写。执行过程（在调工具 / 草稿写了
            什么）不进 IM——那是执行端的事，判据见 `client/DESIGN_SYSTEM.md`
            「导航壳」那条第一性原理与 2026-09-28 主人裁决。 */}
        <div className="text-caption text-muted-foreground" data-room-roll={isOneToOne ? 'one-to-one' : 'group'}>
          {isOneToOne
            ? presenceLabel(peerPresence(live, room.peer?.id))
            : groupHeadline(roll, onlineCount)}
        </div>
        {typingNames.length > 0 && (
          <div className="truncate text-caption text-muted-foreground" role="status">
            {typingNames.join('、')} 正在写…
          </div>
        )}
        {/* ⚠ 「对方读没读我最后那条」（`peer_read`，契约 `CORE_V1_READ_RECEIPT.md` §2）
            **不在这一格**（2026-10-20 主人真机点名后挪走）：那一格说的是"我这条消息
            有没有被接住"，主语是**我发的消息**，不是对方的状态 ⇒ 它挂在**我最后一条
            气泡下面**（`MessageBubble` 的 `peerRead`，由 `ChatPage` 的
            `myLastMessageId` 精确选中）。这里只留**对方的两件事**：在不在、正在写。 */}
      </button>
      <div className="ml-auto flex items-center gap-3">
        <div className="hidden -space-x-2 sm:flex">
          {members.map((m) => (
            <MemberAvatar key={m.id} roomId={room.id} member={m} size="sm" className="rounded-full ring-2 ring-card" />
          ))}
        </div>
        <Button variant="ghost" size="icon" onClick={onOpenMembers} aria-label="参与者列表">
          <Users className="size-4" />
        </Button>
        {/* 语音：对端是**人** ⇒ 真入口（`callEntryFor` 判）；否则这一个按钮根本不渲染
            （Agent 房与群房一个入口都不给 —— 那会是"按下去对面永远没人接"的假入口）。 */}
        {callEntry.audio !== 'none' && (
          <div className="relative">
            <Button
              variant="ghost"
              size="icon"
              onClick={() => (callEntry.audio === 'call' ? startVoiceCall() : showCallHint('audio'))}
              aria-label="语音通话"
            >
              <Phone className="size-4" />
            </Button>
            {callEntry.audio === 'hint' && callHint === 'audio' && (
              <span
                role="status"
                className="absolute right-0 -bottom-8 z-20 rounded-md border border-border bg-popover px-2.5 py-1 text-caption text-muted-foreground shadow-lg animate-in fade-in-0 zoom-in-95"
              >
                语音通话尚未开放
              </span>
            )}
          </div>
        )}
        {/* 视频：**今天一律只是提示**（壳只放行麦克风，Camera 一律拒 ⇒ 给了真入口就是假的）。 */}
        {callEntry.video !== 'none' && (
          <div className="relative">
            <Button
              variant="ghost"
              size="icon"
              onClick={() => showCallHint('video')}
              aria-label="视频通话"
            >
              <Video className="size-4" />
            </Button>
            {callHint === 'video' && (
              <span
                role="status"
                className="absolute right-0 -bottom-8 z-20 rounded-md border border-border bg-popover px-2.5 py-1 text-caption text-muted-foreground shadow-lg animate-in fade-in-0 zoom-in-95"
              >
                视频通话尚未开放
              </span>
            )}
          </div>
        )}
        <Button
          variant="ghost"
          size="icon"
          onClick={onOpenHistory}
          aria-label="查找聊天记录"
        >
          <Search className="size-4" />
        </Button>
      </div>
      </div>
    </header>
  )
}
