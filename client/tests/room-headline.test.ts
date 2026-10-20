// 房间头部那一行（`src/api/roomHeadline.ts`）的行为测试。
//
// 两套判据，分两段：
// 1. **投契口径**（2026-10-14 真机读数逼出来的）：同一间房、无论成员表加载没加载，读数必须一样；
//    人数含我、在线不含我、读不到 ≠ 没有。
// 2. **不报账**（2026-10-20 主人真机点名"说了一大堆，是不是很逆天"）：句子里不许有
//    「（含你）」「（不含你）」这种对着读者解释口径的括注，也不许有 `Human` / `Agent` 分类计数
//    —— 那两样是仪表，不是人话。
import assert from 'node:assert/strict'
import test from 'node:test'

import type { RoomLiveSnapshot } from '../src/api/contracts.ts'
import {
  membersWithSelf,
  peerPresence,
  presenceLabel,
  roomHeadline,
  selfMember,
  type RollMember,
} from '../src/api/roomHeadline.ts'

const ME: RollMember = { id: 'human-me', kind: 'human', displayName: 'Inori' }
const PEER: RollMember = { id: 'agent-xiaoxin', kind: 'agent', displayName: '小欣' }
const OTHER: RollMember = { id: 'agent-other', kind: 'agent', displayName: '小测' }

/** 世界的成员表（含我）。 */
const authoritative: RollMember[] = [ME, PEER]
/** `direct` 房自带的 `participants`：**只有对端**（这就是那个洞）。 */
const peerOnly: RollMember[] = [PEER]

function snapshot(
  members: Array<Partial<RoomLiveSnapshot['members'][number]> & { participant_id: string }>,
): RoomLiveSnapshot {
  return {
    room_id: 'room-1',
    members: members.map((m) => ({
      display_name: m.participant_id,
      kind: 'human',
      online: false,
      typing: false,
      ...m,
    })) as RoomLiveSnapshot['members'],
  }
}

const onlinePeer = snapshot([{ participant_id: PEER.id, online: true }])

/** 每次都写全参数，省得每个用例各拼一遍。 */
const say = (input: {
  kind?: string
  members: readonly RollMember[]
  self?: RollMember | null
  authoritative?: boolean
  snapshot?: RoomLiveSnapshot | undefined
}) =>
  roomHeadline({
    kind: input.kind ?? 'group',
    members: input.members,
    self: input.self === undefined ? ME : input.self,
    authoritative: input.authoritative ?? true,
    snapshot: input.snapshot,
  })

test('★ 同一间 1:1 房：成员表加载与否，读数**必须一样**（那条真机缺陷的反例）', () => {
  // 加载过：成员表里就有我。
  const loaded = say({ kind: 'direct', members: authoritative, authoritative: true, snapshot: onlinePeer })
  // 没加载：房间自带的 `participants` 只给对端 —— 我由 `membersWithSelf` 补上。
  const fallback = say({ kind: 'direct', members: peerOnly, authoritative: false, snapshot: onlinePeer })

  assert.equal(loaded, '在线')
  assert.equal(fallback, '在线', '来源不同 ⇒ 同一句话（从前这里是「1 位参与者 · 0 Human · 1 Agent」）')
  assert.equal(membersWithSelf(peerOnly, ME).length, 2, '人数含我：只读到对端时也是 2 个人')
})

test('★ 2 人房（人 ＋ 1 个 Agent）不再报账：只说"小欣 · 在线"', () => {
  // 主人截图里那间房就是这一格：`group` 房 + 2 位参与者 + 对端在线。
  assert.equal(
    say({ kind: 'group', members: authoritative, snapshot: onlinePeer }),
    '小欣 · 在线',
    '★ 这是那条真机抱怨的原形状：从前是「2 位参与者（含你） · 1 Human · 1 Agent · 1 位在线（不含你）」',
  )
  // 组房与 1:1 房的差别只在"标题是不是已经是对方的名字"。
  assert.equal(say({ kind: 'direct', members: authoritative, snapshot: onlinePeer }), '在线')
  // 读不到进行态时，只说读不到（不写"不在线"）。
  assert.equal(say({ kind: 'group', members: authoritative }), '小欣 · 读不到在线状态')
})

test('★ 句子里**没有**括注与分类计数（这就是"说了一大堆"的机械复现条件）', () => {
  const samples = [
    say({ kind: 'group', members: authoritative, snapshot: onlinePeer }),
    say({ kind: 'group', members: [PEER, OTHER], snapshot: onlinePeer }),
    say({ kind: 'group', members: peerOnly, authoritative: false, snapshot: onlinePeer }),
    say({ kind: 'group', members: [], snapshot: undefined }),
    say({ kind: 'direct', members: peerOnly, authoritative: false, snapshot: onlinePeer }),
  ]
  for (const line of samples) {
    for (const banned of ['含你', '不含你', 'Human', 'Agent', '参与者', '读数和']) {
      assert.ok(
        !line.includes(banned),
        `★ 这一行里出现了「${banned}」：${line} —— 给读者解释口径的话、以及 Human/Agent 分类计数都不属于这一行`,
      )
    }
  }
})

test('★ 不止一人：人数含我、在线不含我（口径写进计算，不写进句子）', () => {
  // 我 ＋ 两个 Agent ＝ 3 位成员；其中 1 位在线（不含我）。
  assert.equal(
    say({ kind: 'group', members: [PEER, OTHER], snapshot: onlinePeer }),
    '3 位成员 · 1 位在线',
  )
  // 没有人（或读不到）在线 ⇒ 那一格整个不出现（不写"0 位在线"）。
  assert.equal(say({ kind: 'group', members: [PEER, OTHER], snapshot: snapshot([]) }), '3 位成员')
  // 来源不全 ⇒ 说"已知"，不说"就这么多"。
  assert.equal(
    say({ kind: 'group', members: [PEER, OTHER], authoritative: false, snapshot: onlinePeer }),
    '已知 3 位成员 · 1 位在线',
  )
})

test('只有我：成员表在场 ⇒「只有你」；不在场 ⇒ 说"还没读到"，不假装"就我一个"', () => {
  assert.equal(say({ kind: 'group', members: [], self: ME, authoritative: true }), '只有你')
  assert.equal(say({ kind: 'group', members: [], self: ME, authoritative: false }), '成员还没读到')
  assert.equal(say({ kind: 'group', members: [], self: null, authoritative: false }), '成员还没读到')
})

test('含我：不在列表里就补上，已经在里面**不重复**（同一份事实只有一个数）', () => {
  assert.deepEqual(
    membersWithSelf(authoritative, ME).map((m) => m.id),
    ['human-me', 'agent-xiaoxin'],
  )
  assert.deepEqual(membersWithSelf(peerOnly, ME).map((m) => m.id), ['agent-xiaoxin', 'human-me'])
  assert.deepEqual(membersWithSelf(peerOnly, null).map((m) => m.id), ['agent-xiaoxin'])
  assert.deepEqual(membersWithSelf([], null), [])
})

test('我这一行从 `GET /v1/home` 的 participant 来（snake_case 只在这一处翻译）', () => {
  assert.deepEqual(
    selfMember({ participant: { id: 'p-1', kind: 'human', display_name: 'Inori' } }),
    { id: 'p-1', kind: 'human', displayName: 'Inori' },
  )
  assert.equal(selfMember(null), null, '没登录 ⇒ 没有我这一行（数字按来源算）')
  assert.equal(selfMember({ participant: null }), null)
  assert.equal(selfMember({ participant: { id: '', kind: 'human', display_name: 'x' } }), null)
})

test('对端在不在：三态**不许压成两态**（读不到 ≠ 不在线）', () => {
  assert.equal(peerPresence(undefined, PEER.id), 'unknown', '快照没到 ⇒ 读不到')
  assert.equal(peerPresence(snapshot([]), PEER.id), 'unknown', '快照里没有她那一行 ⇒ 读不到')
  assert.equal(peerPresence(snapshot([{ participant_id: PEER.id, online: true }]), PEER.id), 'online')
  assert.equal(
    peerPresence(snapshot([{ participant_id: PEER.id, online: false }]), PEER.id),
    'offline',
    '快照里真有那一行、写着 false ⇒ 才准说"不在线"',
  )
  assert.equal(peerPresence(snapshot([{ participant_id: PEER.id, online: true }]), null), 'unknown')
  assert.equal(presenceLabel('online'), '在线')
  assert.equal(presenceLabel('offline'), '不在线')
  assert.equal(presenceLabel('unknown'), '读不到在线状态')
})
