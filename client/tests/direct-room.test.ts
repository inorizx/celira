// **「跟它说话」那一间房**（`src/api/directRoom.ts`）——纯逻辑直测（2026-10-20 T4）。
//
// 这一条要钉的是**判据**（不是 UI）：两个人、正是这两个人 ⇒ 就是那间房；否则 null。
// 为什么值得单开一份用例：这条判据今天有两处入口（`/agent/:id` 与 Agent 主页首屏），
// 而契约里「同一 Human–Agent 对至多一间 `direct` 房」是硬约束 —— 判据写错一次，
// 用户点两次就撞 409（或者更糟：建出第二间）。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { directRoomOf, type DirectRoomView } from '../src/api/directRoom.ts'

function room(id: string, participantIds: string[]): DirectRoomView {
  return { id, participantIds }
}

test('两个人、正是这两个人 ⇒ 就是它（顺序无关）', () => {
  const rooms = [room('r-group', ['me', 'it', 'other']), room('r-direct', ['it', 'me'])]
  assert.equal(directRoomOf(rooms, 'me', 'it')?.id, 'r-direct')
})

test('★ 不看 `kind`：人↔人那间（契约里叫 `human_direct`）同一条判据要认得出', () => {
  // 这一格不是假设：`AgentPage` 原先那段就地判据就是"两个参与者、包含双方"，
  // 而人↔人的房 kind 是 `human_direct`。认 `kind === 'direct'` 会让人↔人那一支
  // 永远找不到房、每次点都去建一间。
  assert.equal(directRoomOf([room('r-human', ['me', 'someone'])], 'me', 'someone')?.id, 'r-human')
})

test('群房 / 只有我 / 只有它 ⇒ 都不是"我们的那间"', () => {
  assert.equal(directRoomOf([room('r-group', ['me', 'it', 'other'])], 'me', 'it'), null)
  assert.equal(directRoomOf([room('r-solo', ['me'])], 'me', 'it'), null)
  assert.equal(directRoomOf([room('r-other', ['someone', 'it'])], 'me', 'it'), null)
  assert.equal(directRoomOf([], 'me', 'it'), null)
})

test('★ 不知道"我是谁"（或地址里没有它）⇒ `null`，不拿列表里第一间两人房顶上', () => {
  const rooms = [room('r-with-someone-else', ['me', 'other'])]
  assert.equal(directRoomOf(rooms, '', 'it'), null)
  assert.equal(directRoomOf(rooms, 'me', ''), null)
  // 自己跟自己不是一种房（真发生就是上游数据错，这一层不替它编一间）。
  assert.equal(directRoomOf([room('r-self', ['me', 'me'])], 'me', 'me'), null)
})
