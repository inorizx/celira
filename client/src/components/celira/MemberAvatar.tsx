import { useMemberAvatarUrl } from '@/hooks/use-member-avatar'
import type { Participant } from '@/api/domain'
import { ParticipantAvatar } from './ParticipantAvatar'

/**
 * 房间内的成员头像：在 `ParticipantAvatar` 之上补一件它给不了的事 —— **取 URL**。
 *
 * 分工：
 * - `ParticipantAvatar` 是纯展示：给 URL 就画图，没有或加载失败就画首字母；
 * - "这个人在**这一间房**里有没有头像、URL 是什么"需要房间上下文（契约的读取路径
 *   要求调用者是该 Room 的当前 active 成员），由这里用 hook 取。
 *
 * 为什么必须是组件而不是一个工具函数：**hook 不能在 `.map()` 里调** —— 每个成员
 * 要各自取自己那条 URL，不能共用一次调用。
 */
export function MemberAvatar({
  roomId,
  member,
  size = 'md',
  className,
}: {
  roomId: string
  member: Participant
  size?: 'sm' | 'md' | 'lg' | 'xl'
  className?: string
}) {
  const avatarUrl = useMemberAvatarUrl(roomId, member.id)
  return (
    <ParticipantAvatar
      participant={member}
      size={size}
      avatarUrl={avatarUrl}
      className={className}
    />
  )
}
