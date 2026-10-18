// **消息投影**（线协议 `MessageView` → 视图模型 `Message`）的纯逻辑测试（2026-09-26 立）。
//
// ## 为什么需要它
//
// 2026-09-26 的前端审计抓到一条主链缺陷：**权威历史里的撤回消息仍渲染成气泡**——
// 有气泡、头像可点、右键可回复、还能再撤回一次。根因是 `core.ts` 的 `toMessage`
// **从来不置 `system`**，而渲染分支（`ChatPage`）只按 `message.system` 二选一；
// 事件路径置上的 `system: true`（`store.ts` 的 `message.recalled` 那一支）
// 又会被下一次唤醒的**权威重读整体覆盖** ⇒ 刷新之后撤回又变回气泡。
//
// 这一格**当时一条测试都没有**。本文件就是那条判据。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { toMessage } from '../src/api/core.ts'
import type { MessageView } from '../src/api/contracts.ts'

/** 一条普通消息的线协议形状（只给投影真正读的那几格）。 */
function view(over: Partial<MessageView> = {}): MessageView {
  return {
    id: 'm-1',
    room_id: 'r-1',
    actor: { id: 'p-1', kind: 'human', display_name: '某人' } as MessageView['actor'],
    body: '在的',
    mentions: [],
    attachments: [],
    reply_to_message_id: null,
    created_at: '2026-09-26T04:00:00.000Z',
    recalled_at: null,
    ...over,
  }
}

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(true)
})

test('★ 撤回是终态 tombstone：`recalled_at` 在场 ⇒ `system` 必须置上（不是气泡）', () => {
  const recalled = toMessage(view({ recalled_at: '2026-09-26T05:00:00.000Z' }))
  assert.equal(recalled.system, true, '撤回消息必须是系统行——否则它会进 MessageBubble')
  assert.equal(recalled.recalledAt, '2026-09-26T05:00:00.000Z')

  // 正向锚点：**没撤回的**不许被误标成系统行（否则整条时间线都变系统消息）。
  const normal = toMessage(view())
  assert.equal(normal.system, undefined)
  assert.equal(normal.recalledAt, undefined)
})

test('★ 撤回后的正文按契约是 null ⇒ 投影成空串，且**不许**把撤回前的正文带出来', () => {
  // 契约：撤回后 `body = null`、`mentions = []`、`attachments = []`。
  const recalled = toMessage(
    view({ recalled_at: '2026-09-26T05:00:00.000Z', body: null, mentions: [], attachments: [] }),
  )
  assert.equal(recalled.body, '')
  assert.deepEqual(recalled.mentions, [])
  assert.deepEqual(recalled.attachments, [])
})

test('投影的其余几格照旧：id / 房间 / 作者 / 回复目标 / 时间', () => {
  const m = toMessage(
    view({ reply_to_message_id: 'm-0', mentions: ['p-2'], created_at: '2026-09-26T04:00:00.000Z' }),
  )
  assert.equal(m.id, 'm-1')
  assert.equal(m.roomId, 'r-1')
  assert.equal(m.authorId, 'p-1')
  assert.equal(m.replyToId, 'm-0')
  assert.deepEqual(m.mentions, ['p-2'])
  assert.equal(m.delivery, 'delivered')
  assert.ok(m.sentAt.length > 0 && m.day.length > 0)
})
