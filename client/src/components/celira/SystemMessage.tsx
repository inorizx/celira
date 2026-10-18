import { Info, Undo2 } from 'lucide-react'
import { tryGetParticipant } from '@/api/participantDirectory'
import type { Message } from '@/api/domain'
import { systemMessageIdentity } from '@/api/selfIdentity'

export function SystemMessage({
  message,
  selfId,
  resolveAuthorName,
}: {
  message: Message
  selfId?: string
  /** 真实作者名解析器（真实世界传入 me + room.participants 投影；缺省回退
   * fixtures Mock/Gallery）。未知作者显示中性「参与者」，绝不猜 human/agent。 */
  resolveAuthorName?: (authorId: string) => string | undefined
}) {
  const recalled = message.recalledAt != null
  // 作者名/「你撤回」归属 fail-closed（见 systemMessageIdentity）：传入真实
  // resolver 时只信 resolver 与真实 selfId，绝不回退 fixtures；resolver 缺省
  // （Mock/Gallery）才允许 fixtures 回退与 SELF_ID 兜底。
  const { authorName, isSelfAuthor } = systemMessageIdentity(
    message.authorId,
    selfId,
    resolveAuthorName,
    tryGetParticipant,
  )
  const body = recalled
    ? isSelfAuthor
      ? '你撤回了一条消息'
      : `${authorName} 撤回了一条消息`
    : message.body
  return (
    <div className="flex items-center justify-center gap-1.5 text-caption text-muted-foreground">
      {recalled ? <Undo2 className="size-3" aria-hidden /> : <Info className="size-3" aria-hidden />}
      <span>{body}</span>
      <span className="opacity-50">·</span>
      <span className="opacity-70">{message.sentAt}</span>
    </div>
  )
}
