// 结构化提及（Mention）在客户端的**唯一裁决面**。
//
// 服务端契约 `docs/contracts/core-v1/CORE_V1_MENTION_TARGETS.md`：一条消息的 `mentions`
// 是**显式唤醒意图的公共事实**，Core **永不从正文推断 mention**；group 房内任意 active
// 成员（Human 或 Agent）都可是目标，`direct` / `human_direct` 必须为空。
//
// 收口前客户端有三个各写一遍的地方，各自都能单独漂移：
//   1. 候选名单的「排除自己 + 名字匹配 + agent 优先」筛选，在 `MessageInput`（两处：候选
//      计数与回车选择）与 `MentionPopover` 里**各写一遍**；
//   2. 送出的目标集合写死为「只取 agent 目标」（`agentMentionIds`）——服务端放开 Human
//      目标之后，这一条会让 @ 人**永远发不出去**；
//   3. 桌面通知判「有没有提到我」读的是**正文文本**（`body.includes('@' + 我的显示名)`），
//      正文里手打一句 `@我` 就误报；而同一条消息的**结构化 `mentions` 就在手上**（契约
//      明说它是权威事实）。渲染侧同病：结构化目标若不在成员名快照里，就**一律按 agent
//      上色**（把 Human 画成 Agent）。
//
// 本模块把这四件事各收成一个纯函数：无 React、无 zustand、无 fixtures、无副作用，
// `node --test` 直测（见 `tests/mention-targets.test.ts`）。**判据一律按 participant id**，
// 名字只用于「在正文里找到该高亮哪一个 token」这一件装饰性的事。

/** 提及目标与候选的统一形状：id 是判据，name 是投影，kind 只用于着色。 */
export interface MentionMember {
  id: string
  name: string
  kind: 'human' | 'agent'
}

/** `null` = **不声称种类**（中性样式）。猜不出来时给中性，绝不给一个假种类。 */
export type MentionTokenKind = 'human' | 'agent' | null

export interface MentionToken {
  /** 正文里出现的高亮片段（含 `@`），如 `@Inori`。 */
  token: string
  kind: MentionTokenKind
  /** true = 来自消息的结构化 `mentions`（权威事实）；false = 来自成员名快照（装饰）。 */
  structured: boolean
}

/**
 * @ 候选名单：房间成员里除自己之外、名字匹配 query 的成员，**agent 优先**（同组内保持
 * 传入顺序）。三个调用点（候选计数、回车选择、建议弹窗）共用这一份判据。
 */
export function mentionCandidates(
  members: readonly MentionMember[],
  opts: { meId?: string; query?: string } = {},
): MentionMember[] {
  const q = (opts.query ?? '').trim().toLowerCase()
  return members
    .filter((m) => m.id !== opts.meId)
    .filter((m) => m.name.toLowerCase().includes(q))
    .sort((a, b) => Number(b.kind === 'agent') - Number(a.kind === 'agent'))
}

/**
 * 送出的结构化 `mentions`：**只有 group 房才带**（契约 §2：`direct` / `human_direct`
 * 携带非空 mentions 一律 400 `invalid_mention`）⇒ 非 group 一律 fail-closed 返回
 * `undefined`，绝不把 ids 透传出去。空集合也是 `undefined`（不发空数组冒充事实）。
 *
 * 顺序去重：重复目标是非法的，且规范化目标集合参与服务端幂等比较。
 */
export function mentionsForSend(
  roomKind: string | undefined,
  mentionIds: readonly string[],
): string[] | undefined {
  if (roomKind !== 'group') return undefined
  const unique = [...new Set(mentionIds)].filter((id) => id.length > 0)
  return unique.length > 0 ? unique : undefined
}

/**
 * 这条消息是否**在结构化事实上**提到了我（按 participant id）。
 *
 * 契约原话：Core 永不从正文推断 mention ⇒ 客户端也不许。正文里手打的 `@我的名字`
 * 不构成提及；只有 `mentions` 含我的 id 才算。
 */
export function mentionedMe(message: { mentions?: readonly string[] }, meId?: string): boolean {
  if (!meId) return false
  return (message.mentions ?? []).includes(meId)
}

/**
 * 正文里该高亮的 `@token` 及其种类。
 *
 * 判据顺序（前者覆盖后者）：
 *   ① **结构化 `mentions`**（权威）：按 id 解析出目标与其种类；
 *   ② **成员名快照**（装饰）：正文里出现成员名就高亮，但**只在名字不歧义时**才声称种类
 *      —— 两个同名成员种类不同时给中性（不知道说的是谁，就不猜）。
 *
 * 解析不出名字（目标已不在成员表、且没有解析器）⇒ **不产生 token**：连高亮什么都不知道，
 * 更不能凭空给它一个种类。空名字同样跳过（否则 token 退化成 `@`，正文里每个 @ 都会被染色）。
 */
export function mentionTokens(args: {
  mentions?: readonly string[]
  members?: readonly MentionMember[]
  resolveMention?: (id: string) => MentionMember | undefined
}): MentionToken[] {
  const members = args.members ?? []
  const byId = new Map(members.map((m) => [m.id, m]))
  // 名字 → 种类：同名同 kind 忠实；同名不同 kind 一律中性。
  const kindByName = new Map<string, MentionTokenKind>()
  for (const m of members) {
    if (!m.name) continue
    const prev = kindByName.get(m.name)
    if (prev === undefined) kindByName.set(m.name, m.kind)
    else if (prev !== m.kind) kindByName.set(m.name, null)
  }

  const byToken = new Map<string, MentionToken>()
  // ① 结构化事实优先：目标种类只按 id 取。
  for (const id of new Set(args.mentions ?? [])) {
    const m = args.resolveMention?.(id) ?? byId.get(id)
    if (!m || !m.name) continue
    const token = `@${m.name}`
    const existing = byToken.get(token)
    if (existing) {
      // 两个目标解析出同一个 token 而种类不同 ⇒ 正文里这一段本就说不清是谁，给中性。
      if (existing.kind !== m.kind) byToken.set(token, { token, kind: null, structured: true })
      continue
    }
    byToken.set(token, { token, kind: m.kind, structured: true })
  }
  // ② 成员名快照补充装饰性高亮（不覆盖已有 token）。
  for (const m of members) {
    if (!m.name) continue
    const token = `@${m.name}`
    if (byToken.has(token)) continue
    byToken.set(token, { token, kind: kindByName.get(m.name) ?? null, structured: false })
  }

  // 长 token 在前：防止短名把长名前缀截断（渲染侧的既有纪律）。
  return [...byToken.values()].sort((a, b) => b.token.length - a.token.length)
}
