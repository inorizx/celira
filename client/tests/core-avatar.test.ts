// core 线头像传输层测试（TASK_54 接线刀；node --test + 类型剥离，不引入新框架）。
//
// 这一刀把头像接进 **core 直连**（Host 全托管路径已退役，那条线的端点无人可达）。
// 契约 `CORE_V1_AVATAR_P0.md` 早已批准、im 端点已黑盒验收，所以这里只钉**接线**：
// 路径拼得对不对、multipart 形状对不对、Bearer 带没带、错误码映得对不对。
//
// 三条容易走岔的地方（本文件逐条钉住）：
//   ① 读取走 fetch + blob（core 的认证是 Bearer 头，`<img>` 带不了它）；
//   ② 上传是 PUT 且 multipart 里**多一个 `base_version` 文本 part**（附件只有文件）；
//   ③ 幂等键由 core 侧每次生成（不做 Host 那套"按键复用"纪律），客户端不缓存版本。

import assert from 'node:assert/strict'
import test from 'node:test'
import { CoreApi } from '../src/api/core.ts'
import { ApiError } from '../src/api/client.ts'

const AGENT_ID = '11111111-1111-4111-8111-111111111111'
const ROOM_ID = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'
const PARTICIPANT_ID = '22222222-2222-4222-9222-222222222222'
const TOKEN = 'clr_hs_test_token'
const BASE = 'http://127.0.0.1:8080'

interface Seen {
  url: string
  method: string
  authorization: string | null
  idempotencyKey: string | null
  body: unknown
}

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  })
}

/**
 * 跑一次调用，捕获它真正发出去的那一发请求。
 *
 * 只允许**一发**（`seen.length === 1`）：头像的每一次调用都该是一次往返，
 * 多发一两发在这里就算错——"顺手多打一次"是这类接线最容易混进去的东西。
 */
async function captureFeed(
  responder: Response | (() => Response),
  run: (api: CoreApi) => Promise<unknown>,
): Promise<Seen> {
  const originalFetch = globalThis.fetch
  const seen: Seen[] = []
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const url =
      typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url
    const headers = (init?.headers ?? {}) as Record<string, string>
    seen.push({
      url,
      method: init?.method ?? 'GET',
      authorization: headers['Authorization'] ?? headers['authorization'] ?? null,
      idempotencyKey: headers['Idempotency-Key'] ?? headers['idempotency-key'] ?? null,
      body: init?.body,
    })
    return typeof responder === 'function' ? responder() : responder
  }) as typeof fetch
  try {
    await run(new CoreApi(TOKEN, BASE))
  } finally {
    globalThis.fetch = originalFetch
  }
  assert.equal(seen.length, 1, '头像的一次调用只该发一发请求')
  return seen[0]
}

const AVATAR_OK = {
  profile: {
    participant_id: AGENT_ID,
    profile_version: 4,
    short_description: null,
    public_description: null,
    avatar: { content_type: 'image/png', size_bytes: 3 },
    published_by_id: AGENT_ID,
    published_by_kind: 'agent',
    published_at: '2026-09-19T00:00:00Z',
  },
  idempotency_replayed: false,
  request_id: 'req-avatar-1',
}

function pngFile(): File {
  return new File([new Uint8Array([137, 80, 78, 71])], 'a.png', { type: 'image/png' })
}

test('putAgentAvatar：PUT 到 Owner 头像路径，multipart 带 base_version ＋ file，Bearer 认证', async () => {
  const seen = await captureFeed(jsonResponse(200, AVATAR_OK), (api) =>
    api.putAgentAvatar(AGENT_ID, { baseVersion: 3, file: pngFile() }),
  )

  assert.equal(seen.method, 'PUT', '契约是单步 PUT，不是 POST')
  assert.equal(seen.url, `${BASE}/v1/agents/${AGENT_ID}/profile/avatar`)
  assert.equal(seen.authorization, `Bearer ${TOKEN}`, 'core 线必须带 Bearer 头')
  // **这一条是真机踩出来的**：契约要求 Idempotency-Key，缺了 im 直接回
  // `idempotency_key_required`（400）。第一次接线时 helper 压根不发这个头，
  // 单元测试当时只断言了路径与 FormData，没断言头 —— 于是漏到了真机。
  assert.ok(seen.idempotencyKey, '上传必须带 Idempotency-Key（im 缺了就拒）')

  // multipart 形状：**恰好一个 base_version 文本 part ＋ 一个 file 文件 part**
  const form = seen.body
  assert.ok(form instanceof FormData, '上传体必须是 FormData')
  assert.equal(form.get('base_version'), '3', 'base_version 必须作为文本 part 下去')
  const file = form.get('file')
  assert.ok(file instanceof File, 'file part 必须是文件（带文件名与类型）')
  assert.equal(file.name, 'a.png')
  assert.equal(file.type, 'image/png')
})

test('putAgentAvatar：成功响应按契约投影（profile 里的 avatar 元数据读得出来）', async () => {
  const originalFetch = globalThis.fetch
  globalThis.fetch = (async () => jsonResponse(200, AVATAR_OK)) as typeof fetch
  try {
    const result = await new CoreApi(TOKEN, BASE).putAgentAvatar(AGENT_ID, {
      baseVersion: 3,
      file: pngFile(),
    })
    assert.equal(result.profile.profile_version, 4)
    assert.deepEqual(result.profile.avatar, { content_type: 'image/png', size_bytes: 3 })
    assert.equal(result.idempotency_replayed, false)
  } finally {
    globalThis.fetch = originalFetch
  }
})

test('deleteAgentAvatar：DELETE 同路径，JSON body 带 base_version，且带幂等键头', async () => {
  const seen = await captureFeed(jsonResponse(200, { ...AVATAR_OK, idempotency_replayed: true }), (api) =>
    api.deleteAgentAvatar(AGENT_ID, { baseVersion: 4 }),
  )

  assert.equal(seen.method, 'DELETE')
  assert.equal(seen.url, `${BASE}/v1/agents/${AGENT_ID}/profile/avatar`)
  assert.equal(seen.authorization, `Bearer ${TOKEN}`)
  assert.deepEqual(
    JSON.parse(String(seen.body)),
    { base_version: 4 },
    '摘除是 JSON body ＋ base_version（不是 multipart）',
  )
  // 契约要求 Idempotency-Key（`idempotency_key_required` 是契约列出的错误码）；
  // 但 core 线每次生成一支即可——不缓存、不复用、不预判结果。
  assert.ok(seen.idempotencyKey, 'core 线也要带幂等键（契约要求）')
})

test('fetchMemberAvatarBlob：GET Room 成员路径 ＋ Bearer，拿到字节', async () => {
  const bytes = new Uint8Array([137, 80, 78, 71])
  const seen = await captureFeed(
    new Response(bytes, { status: 200, headers: { 'content-type': 'image/png' } }),
    (api) => api.fetchMemberAvatarBlob(ROOM_ID, PARTICIPANT_ID),
  )

  assert.equal(seen.method, 'GET')
  assert.equal(
    seen.url,
    `${BASE}/v1/rooms/${ROOM_ID}/members/${PARTICIPANT_ID}/avatar`,
    'Room 成员路径是契约给的唯一读取面',
  )
  assert.equal(seen.authorization, `Bearer ${TOKEN}`, 'core 线的读取也要 Bearer（所以不能走 <img>）')
  assert.equal(seen.body, undefined, '读取不带 body')
})

test('同源部署（serverUrl 为空）时路径不带 origin', async () => {
  const originalFetch = globalThis.fetch
  let url = ''
  globalThis.fetch = (async (input: RequestInfo | URL) => {
    url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url
    return jsonResponse(200, AVATAR_OK)
  }) as typeof fetch
  try {
    await new CoreApi(TOKEN).putAgentAvatar(AGENT_ID, { baseVersion: 0, file: pngFile() })
  } finally {
    globalThis.fetch = originalFetch
  }
  assert.equal(url, `/v1/agents/${AGENT_ID}/profile/avatar`)
})

test('写入失败：契约错误码原样映成 ApiError.code（正文不透传）', async () => {
  const cases: Array<[number, string]> = [
    [400, 'avatar_invalid'],
    [413, 'payload_too_large'],
    [409, 'profile_stale'],
    [404, 'agent_not_found'],
  ]
  for (const [status, code] of cases) {
    await assert.rejects(
      () =>
        captureFeed(jsonResponse(status, { error: { code, message: 'sk-secret-正文不该出现' } }), (api) =>
          api.putAgentAvatar(AGENT_ID, { baseVersion: 0, file: pngFile() }),
        ),
      (error: unknown) => {
        assert.ok(error instanceof ApiError, `${code} 应抛 ApiError`)
        assert.equal(error.status, status)
        assert.equal(error.code, code)
        assert.ok(!error.message.includes('sk-secret'), '错误正文绝不进 ApiError.message')
        return true
      },
      `${status} ${code} 应被如实映出来`,
    )
  }
})

test('读取失败：404 的分类带 avatar 前缀（不冒充附件下载）', async () => {
  await assert.rejects(
    () =>
      captureFeed(new Response('', { status: 404 }), (api) =>
        api.fetchMemberAvatarBlob(ROOM_ID, PARTICIPANT_ID),
      ),
    (error: unknown) => {
      assert.ok(error instanceof ApiError)
      assert.equal(error.status, 404)
      assert.equal(
        error.code,
        'avatar_download_404',
        '同一个 fetchBlob helper，头像要用自己的分类前缀',
      )
      return true
    },
  )
})
