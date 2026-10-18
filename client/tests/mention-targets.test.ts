// 结构化提及裁决面（`src/api/mentionTargets.ts`）的行为测试。
//
// 判据全部对着**服务端已落地的契约**（`docs/contracts/core-v1/CORE_V1_MENTION_TARGETS.md`）：
// group 房内任意 active 成员（Human 或 Agent）都可是目标；`direct` / `human_direct` 必须为空；
// Core 永不从正文推断 mention。
//
// 关键用例都把**收口前的形状**作为对照写在测试里（`legacy*` 函数），并配一条能让旧形状
// **真红**的输入 —— 避免写出"恒真判据"（旧实现下也绿 = 没证明任何东西）。

import assert from 'node:assert/strict'
import test from 'node:test'

import {
  mentionCandidates,
  mentionedMe,
  mentionTokens,
  mentionsForSend,
  type MentionMember,
} from '../src/api/mentionTargets.ts'

const me: MentionMember = { id: 'p-me', name: 'Inori', kind: 'human' }
const alice: MentionMember = { id: 'p-alice', name: 'Alice', kind: 'human' }
const courier: MentionMember = { id: 'p-courier', name: 'Courier', kind: 'agent' }
const ariaAgent: MentionMember = { id: 'p-aria-a', name: 'Aria', kind: 'agent' }
const ariaHuman: MentionMember = { id: 'p-aria-h', name: 'Aria', kind: 'human' }

const roomMembers: MentionMember[] = [me, alice, courier]

// ── 收口前的形状（对照实现，逐字抄自收刀前的源码）─────────────────────────────

/** 收口前 `MessageInput` 的送出判据：**只把 agent 目标放进 mentions**。 */
function legacyMentionsForSend(mentionIds: string[], kindById: Map<string, 'human' | 'agent'>): string[] | undefined {
  const agentIds = mentionIds.filter((id) => kindById.get(id) === 'agent')
  return agentIds.length > 0 ? [...agentIds] : undefined
}

/** 收口前 `store.ts` 的提及判据：**读正文文本**（`maybeNotify` 原文）。 */
function legacyMentionedMe(body: string, myDisplayName: string): boolean {
  return body.includes(`@${myDisplayName}`)
}

/** 收口前 `MentionText` 的形状：结构化目标若不在名表里，**一律按 agent 上色**。 */
function legacyKindOfStructuredTarget(name: string, names: Array<{ name: string; kind: 'human' | 'agent' }>): string {
  const kindByName = new Map(names.map((n) => [n.name, n.kind]))
  if (name && !kindByName.has(name)) kindByName.set(name, 'agent')
  return kindByName.get(name) ?? ''
}

// ── 候选名单 ────────────────────────────────────────────────────────────────

test('候选含 Human 成员：@ 人现在是合法目标（旧形状只认 agent）', () => {
  const ids = mentionCandidates(roomMembers, { meId: me.id, query: '' }).map((m) => m.id)
  assert.ok(ids.includes(alice.id), 'Human 成员必须出现在候选里')
  assert.ok(ids.includes(courier.id), 'Agent 成员照旧在候选里')
})

test('候选排除自己', () => {
  const ids = mentionCandidates(roomMembers, { meId: me.id, query: '' }).map((m) => m.id)
  assert.equal(ids.includes(me.id), false)
})

test('候选按名字子串过滤，大小写不敏感；空 query 返回全部', () => {
  assert.deepEqual(
    mentionCandidates(roomMembers, { meId: me.id, query: 'ali' }).map((m) => m.id),
    [alice.id],
  )
  assert.deepEqual(
    mentionCandidates(roomMembers, { meId: me.id, query: 'COUR' }).map((m) => m.id),
    [courier.id],
  )
  assert.equal(mentionCandidates(roomMembers, { meId: me.id, query: '' }).length, 2)
})

test('候选 agent 优先，同组内保持传入顺序', () => {
  const ids = mentionCandidates([alice, courier, ariaAgent], { meId: me.id, query: '' }).map((m) => m.id)
  assert.deepEqual(ids, [courier.id, ariaAgent.id, alice.id])
})

test('候选不改动传入数组（纯函数，排序不许原地生效）', () => {
  const input = [alice, courier]
  mentionCandidates(input, { meId: me.id, query: '' })
  assert.deepEqual(input.map((m) => m.id), [alice.id, courier.id])
})

// ── 送出的结构化 mentions ────────────────────────────────────────────────────

test('group 房：Human 目标与 Agent 目标一起送出（旧形状会漏掉 Human）', () => {
  const kindById = new Map<string, 'human' | 'agent'>([
    [alice.id, 'human'],
    [courier.id, 'agent'],
  ])
  const mentionIds = [alice.id, courier.id]
  // 负对照：旧形状在这组输入下丢掉 Human —— 这条断言证明本用例不是恒真。
  assert.deepEqual(legacyMentionsForSend(mentionIds, kindById), [courier.id])
  // 新判据：两个目标都在，且保序。
  assert.deepEqual(mentionsForSend('group', mentionIds), [alice.id, courier.id])
})

test('direct / human_direct：一律 fail-closed 返回 undefined（契约 §2）', () => {
  assert.equal(mentionsForSend('direct', [alice.id]), undefined)
  assert.equal(mentionsForSend('human_direct', [alice.id, courier.id]), undefined)
  // 房间类型未知（未加载/形状不认识）同样不许透传。
  assert.equal(mentionsForSend(undefined, [alice.id]), undefined)
})

test('空集合与空字符串：不送空数组冒充事实', () => {
  assert.equal(mentionsForSend('group', []), undefined)
  assert.equal(mentionsForSend('group', ['', '']), undefined)
})

test('重复目标去重且保序（重复目标服务端一律 400）', () => {
  assert.deepEqual(mentionsForSend('group', [courier.id, alice.id, courier.id]), [courier.id, alice.id])
})

// ── 通知判据 ────────────────────────────────────────────────────────────────

test('结构化 mentions 含我 ⇒ 提到我了', () => {
  assert.equal(mentionedMe({ mentions: [alice.id, me.id] }, me.id), true)
})

test('正文里手打的 @我的名字、mentions 为空 ⇒ 不算提及（旧形状会误报）', () => {
  const body = '这个 @Inori 只是手打的文本'
  // 负对照：旧形状读正文 ⇒ 误报。
  assert.equal(legacyMentionedMe(body, me.name), true)
  // 新判据：结构化事实才是判据。
  assert.equal(mentionedMe({ mentions: [] }, me.id), false)
  assert.equal(mentionedMe({ mentions: undefined }, me.id), false)
})

test('无 meId 或 mentions 为他人 ⇒ 不算提及', () => {
  assert.equal(mentionedMe({ mentions: [me.id] }, undefined), false)
  assert.equal(mentionedMe({ mentions: [alice.id] }, me.id), false)
})

// ── 渲染高亮：按 id 取种类 ──────────────────────────────────────────────────

test('结构化 Human 目标不在成员快照里 ⇒ 按 Human 上色（旧形状硬染 agent）', () => {
  const members: MentionMember[] = [me, courier] // 名表里没有 Alice
  // 负对照：旧形状在这条路径上必然给 agent。
  assert.equal(legacyKindOfStructuredTarget(alice.name, members), 'agent')
  const tokens = mentionTokens({
    mentions: [alice.id],
    members,
    resolveMention: (id) => (id === alice.id ? alice : undefined),
  })
  // 名表成员照旧装饰性高亮，这里只钉结构化那一条。
  assert.deepEqual(
    tokens.find((t) => t.token === '@Alice'),
    { token: '@Alice', kind: 'human', structured: true },
  )
})

test('目标解析不出名字 ⇒ 不产生 token（不伪造高亮，更不伪造种类）', () => {
  assert.deepEqual(mentionTokens({ mentions: ['p-gone'], members: [] }), [])
})

test('成员名快照的装饰性高亮：种类按成员', () => {
  const tokens = mentionTokens({ members: [me, alice, courier] })
  const byToken = new Map(tokens.map((t) => [t.token, t]))
  assert.equal(byToken.get('@Alice')?.kind, 'human')
  assert.equal(byToken.get('@Courier')?.kind, 'agent')
  assert.equal(byToken.get('@Courier')?.structured, false)
})

test('同名不同种类且无结构化事实 ⇒ 中性（不猜是谁）', () => {
  const tokens = mentionTokens({ members: [ariaAgent, ariaHuman] })
  assert.deepEqual(tokens, [{ token: '@Aria', kind: null, structured: false }])
})

test('结构化事实优先：同名冲突时按结构化目标的种类', () => {
  const tokens = mentionTokens({
    mentions: [ariaHuman.id],
    members: [ariaAgent, ariaHuman],
    resolveMention: (id) => (id === ariaHuman.id ? ariaHuman : undefined),
  })
  assert.deepEqual(tokens, [{ token: '@Aria', kind: 'human', structured: true }])
})

test('两个目标解析出同名的不同种类 ⇒ 中性（正文那一段本就说不清）', () => {
  const tokens = mentionTokens({
    mentions: [ariaAgent.id, ariaHuman.id],
    members: [ariaAgent, ariaHuman],
  })
  assert.deepEqual(tokens, [{ token: '@Aria', kind: null, structured: true }])
})

test('空名字不进名表（否则 token 退化成 @，正文里每个 @ 都被染色）', () => {
  const nameless: MentionMember = { id: 'p-x', name: '', kind: 'agent' }
  assert.deepEqual(mentionTokens({ members: [nameless] }), [])
  assert.deepEqual(mentionTokens({ mentions: [nameless.id], members: [nameless] }), [])
})

test('长 token 在前（防止短名截断长名前缀）', () => {
  const short: MentionMember = { id: 'p-1', name: 'Ann', kind: 'human' }
  const long: MentionMember = { id: 'p-2', name: 'Anna', kind: 'agent' }
  const tokens = mentionTokens({ members: [short, long] })
  assert.deepEqual(tokens.map((t) => t.token), ['@Anna', '@Ann'])
})

test('成员表按 id 命中优先于解析器（同一目标两条来源必须同解）', () => {
  const tokens = mentionTokens({ mentions: [courier.id], members: [courier] })
  assert.deepEqual(tokens, [{ token: '@Courier', kind: 'agent', structured: true }])
})
