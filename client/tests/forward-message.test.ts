// 转发的**编排**（`src/lib/forwardMessage.ts`）的行为测试。
//
// 运行：cd client && npm test
//
// 这一刀要钉住的判据一条：**全有或全无** —— 一个附件没转成，那条消息就不许发出去
// （半转发比不转发更糟：用户以为转过去了，对面只收到一半）。
// ★ 最要紧的负向那条：`upload` 失败时 **`send` 一次都不许被调用**。

import assert from 'node:assert/strict'
import test from 'node:test'

import { forwardMessage, type ForwardDeps } from '../src/lib/forwardMessage.ts'
import type { Attachment } from '../src/api/domain.ts'

/** 门禁不许空跑（与 `render-failure.test.ts` 同款纪律）：最后一条断言本文件真的执行了用例。 */
let completed = 0
/** 总数**含最后那条**（与 `render-failure.test.ts` 同口径）。 */
const DECLARED_CASES = 7

function forwardCase(name: string, fn: () => void | Promise<void>) {
  test(name, async () => {
    await fn()
    completed += 1
  })
}

const attachment = (over: Partial<Attachment> = {}): Attachment => ({
  id: 'a1',
  kind: 'file',
  name: '报表.xlsx',
  mime: 'application/vnd.ms-excel',
  state: 'bound',
  ...over,
})

/** 记下每次调用，便于断言"谁被调了、调了几次、参数是什么"。 */
function deps(over: Partial<ForwardDeps> = {}) {
  const calls = { fetch: [] as string[], upload: [] as string[], send: [] as unknown[] }
  const base: ForwardDeps = {
    fetchBlob: async (id) => {
      calls.fetch.push(id)
      return new Blob(['bytes'], { type: 'application/octet-stream' })
    },
    upload: async (file) => {
      calls.upload.push(file.name)
      return `new-${file.name}`
    },
    send: async (input) => {
      calls.send.push(input)
    },
  }
  return { calls, deps: { ...base, ...over } }
}

forwardCase('没有附件：原样发过去，一条新消息', async () => {
  const { calls, deps: d } = deps()
  const outcome = await forwardMessage({ body: '看这个', attachments: [] }, d)
  assert.deepEqual(outcome, { ok: true, forwarded: 0 })
  assert.equal(calls.send.length, 1)
  assert.deepEqual(calls.send[0], { body: '看这个', attachments: [] })
  assert.equal(calls.upload.length, 0, '没有附件就不该碰上传')
})

forwardCase('★ 有附件：**重新上传一份**，新 id 绑进新消息（不是跨房引用）', async () => {
  const { calls, deps: d } = deps()
  const source = attachment()
  const outcome = await forwardMessage({ body: '转你', attachments: [source] }, d)
  assert.deepEqual(outcome, { ok: true, forwarded: 1 })
  assert.deepEqual(calls.fetch, ['a1'], '取的是原附件的字节')
  assert.deepEqual(calls.upload, ['报表.xlsx'], '在目标房重新上传，名字保留')
  const sent = calls.send[0] as { attachments: Attachment[] }
  assert.equal(sent.attachments.length, 1)
  assert.equal(sent.attachments[0].id, 'new-报表.xlsx', '绑的是**新**附件 id')
  assert.equal(sent.attachments[0].name, '报表.xlsx')
  assert.equal(sent.attachments[0].kind, 'file', '显示归类跟着原件走')
  assert.equal(sent.attachments[0].state, 'pending', '新上传的那份是 pending')
  assert.equal(sent.attachments[0].localUrl, undefined, '不许把源房的本地预览带过去')
})

forwardCase('多个附件：按顺序各转一份，全部成功才发', async () => {
  const { calls, deps: d } = deps()
  const outcome = await forwardMessage(
    { body: '两张图', attachments: [attachment({ id: 'a1', name: '一.png', kind: 'image' }), attachment({ id: 'a2', name: '二.png', kind: 'image' })] },
    d,
  )
  assert.deepEqual(outcome, { ok: true, forwarded: 2 })
  assert.deepEqual(calls.upload, ['一.png', '二.png'])
  assert.equal((calls.send[0] as { attachments: Attachment[] }).attachments.length, 2)
})

forwardCase('★ 上传失败 ⇒ **不发那条消息**，并说清是哪个文件（全有或全无）', async () => {
  const { calls, deps: d } = deps({
    upload: async (file) => {
      calls.upload.push(file.name)
      if (file.name === '二.png') throw new Error('boom')
      return `new-${file.name}`
    },
  })
  const outcome = await forwardMessage(
    { body: '两张图', attachments: [attachment({ id: 'a1', name: '一.png' }), attachment({ id: 'a2', name: '二.png' })] },
    d,
  )
  assert.equal(outcome.ok, false)
  assert.ok(!outcome.ok && outcome.text.includes('二.png'), `失败那句要点名文件：${!outcome.ok && outcome.text}`)
  assert.equal(calls.send.length, 0, '★ 一个没转成，就一条都不许发')
})

forwardCase('★ 下载失败 ⇒ 同样一条都不发（连上传都不该开始）', async () => {
  const { calls, deps: d } = deps({
    fetchBlob: async () => {
      throw new Error('404')
    },
  })
  const outcome = await forwardMessage({ body: 'x', attachments: [attachment()] }, d)
  assert.equal(outcome.ok, false)
  assert.equal(calls.upload.length, 0, '字节都没取到，不该去上传')
  assert.equal(calls.send.length, 0)
})

forwardCase('发送失败 ⇒ 如实说失败（不假装转发了）', async () => {
  const { deps: d } = deps({
    send: async () => {
      throw new Error('boom')
    },
  })
  const outcome = await forwardMessage({ body: 'x', attachments: [] }, d)
  assert.equal(outcome.ok, false)
  assert.ok(!outcome.ok && outcome.text.length > 0, '失败必须有一句话')
})

forwardCase('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(completed >= DECLARED_CASES - 1, `完成 ${completed} < 声明 ${DECLARED_CASES - 1}`)
})
