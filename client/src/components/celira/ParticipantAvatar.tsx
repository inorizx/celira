import { useEffect, useState } from 'react'
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar'
import { cn } from '@/lib/utils'
import type { Participant } from '@/api/domain'
import { CeliraLogo } from './CeliraLogo'

const sizeClasses = {
  sm: 'size-7 text-[10px]',
  md: 'size-9 text-xs',
  lg: 'size-10 text-sm',
  xl: 'size-14 text-base',
} as const

interface ParticipantAvatarProps {
  participant: Participant
  size?: keyof typeof sizeClasses
  /**
   * 真实头像字节的**同源 URL**（由 `useMemberAvatarUrl(roomId, participantId)`
   * 给出，见 `hooks/use-member-avatar.ts`）。
   *
   * 只有**房间上下文**里才给得出来：契约的头像读取路径（`CORE_V1_AVATAR_P0.md` §3.4）
   * 要求调用者与目标都是该 Room 的当前 active 成员。给不出来时（联系人页、
   * 还没有房间）就老实画 fallback —— 不猜、不造 URL、不发明第二条读取面。
   */
  avatarUrl?: string | null
  className?: string
}

export function ParticipantAvatar({
  participant,
  size = 'lg',
  avatarUrl = null,
  className,
}: ParticipantAvatarProps) {
  const isAgent = participant.kind === 'agent'
  // 加载失败与 404（没设头像 / 非成员 / 跨房间）走**同一条**降级路：画回 fallback。
  // 用组件内 state 记住"这一张失败过"，别让每次渲染都重试同一个坏 URL；
  // URL 变了（换房间 / 换人 / 对方换了头像）就重新给它一次机会。
  const [avatarFailed, setAvatarFailed] = useState(false)
  useEffect(() => {
    setAvatarFailed(false)
  }, [avatarUrl])
  const showImage = Boolean(avatarUrl) && !avatarFailed
  return (
    <div className={cn('relative shrink-0', className)}>
      <Avatar className={cn(sizeClasses[size], isAgent && 'ring-1 ring-agent/50')}>
        {showImage && (
          <AvatarImage
            src={avatarUrl ?? undefined}
            alt=""
            className="object-cover"
            onError={() => setAvatarFailed(true)}
          />
        )}
        <AvatarFallback
          className={cn(
            'font-medium',
            isAgent
              ? 'bg-agent-surface text-agent'
              : 'bg-accent text-accent-foreground',
          )}
        >
          {participant.official ? (
            <CeliraLogo className="size-[60%]" />
          ) : (
            participant.initials
          )}
        </AvatarFallback>
      </Avatar>
    </div>
  )
}
