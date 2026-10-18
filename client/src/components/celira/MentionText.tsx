import { participants } from '@/fixtures/data'
import { fixtureFallbackAllowed } from '@/api/participantDirectory'
import { mentionTokens, type MentionMember } from '@/api/mentionTargets'
import { cn } from '@/lib/utils'

interface MentionTextProps {
  text: string
  /** 结构化提及目标（participant ID 列表；来自消息公共视图的 `mentions` 字段，契约
   * `CORE_V1_MENTION_TARGETS.md`）。**判据是 id 不是名字**：目标种类只按 id 取，
   * 目标不在成员快照里也照样按它自己的种类高亮（core 权威事实）。 */
  mentions?: string[]
  /** 目标 ID → 成员（名字 + 种类）解析器。真实世界必须传真实成员投影；绝不查 fixtures。
   * 仅当 `mentions` 非空时使用。 */
  resolveMention?: (id: string) => MentionMember | undefined
  /** 房间成员名快照（真实成员投影）：@ 到任意成员名都高亮——agent 用 agent 系样式、
   * human 用 accent 样式、**名字有歧义（同名不同种类）时给中性**。缺省只在**演示通道**
   * 回退 fixtures 参与者名；真实世界下 `members` 缺失就**不**回退演示名（保持中性）。 */
  members?: MentionMember[]
  className?: string
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

function mentionMemberOf(p: { id: string; displayName: string; kind: 'human' | 'agent' }): MentionMember {
  return { id: p.id, name: p.displayName, kind: p.kind }
}

export function MentionText({ text, mentions, resolveMention, members, className }: MentionTextProps) {
  // 成员表：优先真实成员快照（真实世界必须由调用方注入）；缺省**只在不属于真实世界时**
  // （`fixtureFallbackAllowed()` 闸门，与 `selfIdentity.ts` 同款用法）才回退 fixtures
  // （Gallery/Mock 展示路径）。真实世界 `members` 缺失 ⇒ 空名表 ⇒ 中性渲染，绝不把
  // 演示名当成真实成员。
  const resolvedMembers =
    members ?? (fixtureFallbackAllowed() ? participants.map(mentionMemberOf) : [])
  const tokens = mentionTokens({ mentions, members: resolvedMembers, resolveMention })
  if (tokens.length === 0) return <span className={className}>{text}</span>

  // 名字可能含正则特殊字符，必须转义；token 已按长度降序（防止前缀名截断匹配）。
  const pattern = new RegExp(`(${tokens.map((t) => escapeRegExp(t.token)).join('|')})`, 'g')
  const segments = text.split(pattern)
  const kindByToken = new Map(tokens.map((t) => [t.token, t.kind]))

  return (
    <span className={className}>
      {segments.map((segment, i) => {
        if (!segment.startsWith('@')) return <span key={i}>{segment}</span>
        if (!kindByToken.has(segment)) return <span key={i}>{segment}</span>
        const kind = kindByToken.get(segment)
        return (
          <span
            key={i}
            className={cn(
              'rounded px-0.5 font-medium',
              kind === 'agent'
                ? 'bg-agent-surface text-agent'
                : kind === 'human'
                  ? 'bg-accent text-accent-foreground'
                  : // 种类不知道（目标已离开房间 / 同名不同种类）：高亮这个 @ 事实，
                    // 但**不声称**它是人还是 Agent。
                    'bg-muted text-foreground',
            )}
          >
            {segment}
          </span>
        )
      })}
    </span>
  )
}
