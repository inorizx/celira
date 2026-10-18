// 建房 / 邀请的**候选与走哪条路**（`src/api/uiPolicy.ts` 的两条纯规则）的行为测试。
//
// 这两条规则是 2026-09-29 真机端到端测试里两个**真实故障**的修复点，它们原先各自内联在
// 两个组件里（`NewRoomSheet` 与 `InviteMemberSheet`），既重复、又不可直测：
//
//   1. `roomCandidates` —— 候选 = accepted 联系人 ∪ 自有 Agent。**同一位 Agent 可以同时在
//      两个名单里**（屋主给自己的 Agent 发过好友申请并接受过，那是批次三允许的常态），
//      直接拼数组会让它列出两行同一个 id（React key 也重复）。真机实测：小满出现两次。
//
//   2. `directRoomRouteFor` —— 单选一个候选人建房时走哪条路，判据只能是**它是什么**。
//      真机实测：原先先 `contacts.find(...)` 再 `ownedAgents.find(...)`，于是"既是联系人
//      又是我的 Agent"的那一支**永远**被送进 `POST /v1/human-direct-rooms`（只收 Human）
//      ⇒ 404、界面红字「找不到这位用户」，而"回到你们那间已有的私人房"那条路永远到不了。

import assert from 'node:assert/strict'
import test from 'node:test'

import { directRoomRouteFor, roomCandidates } from '../src/api/uiPolicy.ts'

interface Cand {
  id: string
  kind: 'human' | 'agent'
  displayName: string
}

const 小满 = (): Cand => ({ id: 'ag-1', kind: 'agent', displayName: '小满' })
const 阿元 = (): Cand => ({ id: 'hm-1', kind: 'human', displayName: '阿元' })

test('roomCandidates: 同一位 Agent 同时在联系人与自有 Agent 里 ⇒ 只留一行（真机实测那条）', () => {
  const contacts = [阿元(), 小满()]
  const owned = [小满()]
  const out = roomCandidates(contacts, owned)
  assert.equal(out.length, 2, `去重后应当只剩两行，实际 ${out.length}`)
  assert.deepEqual(
    out.map((p) => p.id),
    ['hm-1', 'ag-1'],
  )
  assert.equal(new Set(out.map((p) => p.id)).size, out.length, 'id 不许重复（否则 React key 也重复）')
})

test('roomCandidates: 先到先得、顺序不变（contacts 在前）', () => {
  const first = { id: 'x', kind: 'agent' as const, displayName: '先来的' }
  const second = { id: 'x', kind: 'agent' as const, displayName: '后来的' }
  const out = roomCandidates([first], [second])
  assert.equal(out.length, 1)
  assert.equal(out[0].displayName, '先来的', '同一个 id 保留先出现的那个，不覆盖')
})

test('roomCandidates: 三个名单也能拼（接口是变参）', () => {
  const a = { id: 'a', kind: 'human' as const, displayName: 'A' }
  const b = { id: 'b', kind: 'human' as const, displayName: 'B' }
  const c = { id: 'c', kind: 'agent' as const, displayName: 'C' }
  assert.deepEqual(
    roomCandidates([a], [b, c], [a, c]).map((p) => p.id),
    ['a', 'b', 'c'],
  )
})

test('roomCandidates: 空输入得空（不伪造候选）', () => {
  assert.deepEqual(roomCandidates([], []), [])
  assert.deepEqual(roomCandidates(), [])
})

test('roomCandidates: 入参不被变异', () => {
  const contacts = [阿元(), 小满()]
  const owned = [小满()]
  const before = JSON.stringify([contacts, owned])
  roomCandidates(contacts, owned)
  assert.equal(JSON.stringify([contacts, owned]), before, '纯函数不许改动入参')
})

test('directRoomRouteFor: 判据是「它是什么」，不是「它从哪个名单来」', () => {
  // 这一条就是真机那个 404 的形状：小满**既是**联系人（走 contacts.find 会命中）**又是**
  // 自有 Agent。按 kind 判 ⇒ agent；按名单判 ⇒ human（错）。
  assert.equal(directRoomRouteFor(小满()), 'agent')
  assert.equal(directRoomRouteFor(阿元()), 'human')
})

test('directRoomRouteFor: 认不出目标时按 human 走（调用方自己会先判 target 在不在）', () => {
  assert.equal(directRoomRouteFor(undefined), 'human')
})

test('roomCandidates: **已撤销的 Agent 不进候选**（它是一条必然失败的死选项）', () => {
  // 契约：软撤销后参与者行仍在（`GET /v1/agents` 是"owner-scoped 管理视图，含 revoked 历史"），
  // 但它不再接收事件、房间成员关系与凭证都已作废。真机实测：已撤销的那支照样列在候选里。
  const live = { id: 'ag-live', kind: 'agent' as const, displayName: '活着的' }
  const dead = { id: 'ag-dead', kind: 'agent' as const, displayName: '已撤销的', revoked: true }
  const out = roomCandidates([], [live, dead])
  assert.deepEqual(
    out.map((p) => p.id),
    ['ag-live'],
  )
  assert.equal(
    roomCandidates([dead]).length,
    0,
    '全是已撤销的 ⇒ 候选为空（页面据此说"没有可邀请的联系人或 Agent"）',
  )
})

test('roomCandidates: 撤销标记不影响联系人（contacts 那份没有这个字段）', () => {
  const human = { id: 'hm-1', kind: 'human' as const, displayName: '阿元' }
  assert.deepEqual(
    roomCandidates([human]).map((p) => p.id),
    ['hm-1'],
  )
})
