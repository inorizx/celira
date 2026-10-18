// **通话那一面的传输判据**（2026-10-09 真机探针抓到缺陷之后补）。
//
// ## 为什么必须有这一层
//
// 2026-10-09 的真机探针（`scripts/home/probe-call.js`）第一次跑就红在**第一格**：
// A 点了「语音通话」，界面只回一句「请求不合法，请检查消息正文或附件」——
// `POST /v1/calls` 吃了 **400 `invalid_request`**。根因是 `core.ts` 里我把请求体写成了
// `JSON.stringify(body)`，而 `fetchJson` **自己**会 `JSON.stringify`（`client.ts`）⇒ **二次编码**：
// 服务端读到的是一个 JSON **字符串**，而不是带 `room_id`/`callee_id` 的对象。
//
// 这个缺陷**谁也看不见**：
//   · 纯逻辑用例（帧解析、媒体面、人话）都碰不到出网那一层；
//   · 边界守卫只对**路径与方法**，不看请求体形状；
//   · `verify:ui` 与 `verify:ui:app` 都不点通话按钮。
// ⇒ 所以这里按"**发出去的请求到底长什么样**"钉住它：截住 `fetch`，读真实请求体。
//
// 运行： cd client && npm test

import assert from 'node:assert/strict'
import test from 'node:test'

import { CoreApi } from '../src/api/core.ts'

interface Captured {
  url: string
  method: string
  body: string | undefined
  headers: Record<string, string>
}

/** 装一个假的 `fetch`（形状照 `transport.ts` 的 web 实现：`status/ok/headers.get/json/blob`）。 */
function stubFetch(reply: { status?: number; json?: unknown } = {}) {
  const captured: Captured[] = []
  const status = reply.status ?? 200
  const doFetch = async (url: string, init: Record<string, unknown> = {}) => {
    captured.push({
      url: String(url),
      method: String(init.method ?? 'GET'),
      body: typeof init.body === 'string' ? init.body : undefined,
      headers: (init.headers as Record<string, string>) ?? {},
    })
    const payload = reply.json === undefined ? {} : reply.json
    return {
      status,
      ok: status >= 200 && status < 300,
      headers: { get: () => null },
      json: async () => payload,
      blob: async () => new Blob([]),
    }
  }
  const had = 'fetch' in globalThis
  const before = (globalThis as Record<string, unknown>).fetch
  Object.defineProperty(globalThis, 'fetch', { value: doFetch, configurable: true, writable: true })
  return {
    captured,
    restore() {
      if (had) {
        Object.defineProperty(globalThis, 'fetch', { value: before, configurable: true, writable: true })
      } else {
        delete (globalThis as Record<string, unknown>).fetch
      }
    },
  }
}

const ROOM = '22222222-2222-4222-8222-222222222222'
const CALLEE = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'
const CALL = '11111111-1111-4111-8111-111111111111'

test('★ 发起：请求体是一个 **JSON 对象**（不是被二次编码的字符串）', async () => {
  const stub = stubFetch({ status: 201, json: { call: { call_id: CALL }, created: true, request_id: 'r1' } })
  try {
    const api = new CoreApi('tok', 'http://127.0.0.1:8080')
    const result = await api.initiateCall(ROOM, CALLEE)
    assert.equal(result.created, true)
    assert.equal(stub.captured.length, 1)
    const sent = stub.captured[0]
    assert.equal(sent.method, 'POST')
    assert.ok(sent.url.endsWith('/v1/calls'), `路径要对：${sent.url}`)
    const parsed = JSON.parse(String(sent.body))
    assert.equal(typeof parsed, 'object', '服务端读到的必须是对象（字符串 ⇒ 400 invalid_request）')
    assert.deepEqual(parsed, { room_id: ROOM, callee_id: CALLEE }, 'media 缺省时不带那一格（服务端默认 audio）')
  } finally {
    stub.restore()
  }
})

test('★ 发起：显式给 `video` 才带上 `media`（`deny_unknown_fields` 要求字段形状一致）', async () => {
  const stub = stubFetch({ status: 201, json: { call: { call_id: CALL }, created: false, request_id: 'r1' } })
  try {
    const api = new CoreApi('tok', 'http://127.0.0.1:8080')
    await api.initiateCall(ROOM, CALLEE, 'video')
    assert.deepEqual(JSON.parse(String(stub.captured[0].body)), {
      room_id: ROOM,
      callee_id: CALLEE,
      media: 'video',
    })
  } finally {
    stub.restore()
  }
})

test('★ 信令：请求体也是对象，且**只**带契约里那两格之一', async () => {
  const stub = stubFetch({ status: 204 })
  try {
    const api = new CoreApi('tok', 'http://127.0.0.1:8080')
    await api.signalCall(CALL, { kind: 'offer', sdp: 'v=0\r\n' })
    await api.signalCall(CALL, {
      kind: 'ice',
      candidates: [{ candidate: 'candidate:1 1 udp 1 10.0.0.1 1 typ host', sdp_mid: 'audio', sdp_mline_index: 0 }],
    })
    assert.equal(stub.captured.length, 2)
    const offer = JSON.parse(String(stub.captured[0].body))
    assert.deepEqual(offer, { kind: 'offer', sdp: 'v=0\r\n' })
    const ice = JSON.parse(String(stub.captured[1].body))
    assert.equal(ice.kind, 'ice')
    assert.equal(ice.candidates.length, 1)
    assert.equal(ice.sdp, undefined, 'ice 那一格不许夹带 sdp')
    assert.ok(stub.captured[1].url.endsWith(`/v1/calls/${CALL}/signal`))
    // 204 ⇒ `void`（不许假装拿到了 Call）
    assert.equal(await api.signalCall(CALL, { kind: 'ice', candidates: [{ candidate: 'c', sdp_mid: null, sdp_mline_index: null }] }), undefined)
  } finally {
    stub.restore()
  }
})

test('三个动作：路径与方法照契约（accept / decline / hangup 都是 POST、都不带体）', async () => {
  const stub = stubFetch({ status: 200, json: { call: { call_id: CALL }, request_id: 'r1' } })
  try {
    const api = new CoreApi('tok', 'http://127.0.0.1:8080')
    await api.acceptCall(CALL)
    await api.declineCall(CALL)
    await api.hangupCall(CALL)
    await api.getCall(CALL)
    assert.deepEqual(
      stub.captured.map((each) => `${each.method} ${each.url.replace('http://127.0.0.1:8080', '')}`),
      [
        `POST /v1/calls/${CALL}/accept`,
        `POST /v1/calls/${CALL}/decline`,
        `POST /v1/calls/${CALL}/hangup`,
        `GET /v1/calls/${CALL}`,
      ],
    )
    for (const each of stub.captured.slice(0, 3)) assert.equal(each.body, undefined, '动作不该带请求体')
  } finally {
    stub.restore()
  }
})

test('错误信封：稳定码原样抛出（上屏那一句由 `contractErrors` 给）', async () => {
  const stub = stubFetch({ status: 409, json: { error: { code: 'call_party_busy', request_id: 'r1' } } })
  try {
    const api = new CoreApi('tok', 'http://127.0.0.1:8080')
    await assert.rejects(
      () => api.initiateCall(ROOM, CALLEE),
      (error: unknown) => (error as { code?: string }).code === 'call_party_busy',
    )
  } finally {
    stub.restore()
  }
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(true)
})
