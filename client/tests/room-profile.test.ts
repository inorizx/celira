// **群简介接入**（契约 `CORE_V1_ROOM_PROFILE.md`；2026-10-25 客户端切片）。
//
// 这一刀只接客户端，Core 的 wire 早已实现。本文件钉三件容易走岔的事：
//   ① **字节口径**：2000 字节是 UTF-8 字节，不是 `String.length`（中文 3、Emoji 4）；
//      用后者会把「666 个汉字 + 2 个 ASCII」判成合法，而 Core 会回 400。
//   ② **请求形状**：`PUT .../profile` + `base_version`（snake_case）+ 每次一支
//      Idempotency-Key。写错字段名或漏 key 是这条线最典型的静默失败。
//   ③ **产品面**：群简介只在 group 房出现、群主才有编辑入口、空态有那句「还没有群简介」——
//      direct / human_direct 连区块都不该有（契约 §3.2 / §8）。
//
// 真机那半由 Core 的黑盒测试证明（`cargo test -p celira-im --test room_profile_blackbox`）；
// 这里能证明的是**客户端这一侧**的形状与判据。

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

import { CoreApi } from '../src/api/core.ts'
import { apiErrorText } from '../src/api/contractErrors.ts'
import {
  ROOM_PROFILE_MAX_BYTES,
  applyRoomProfile,
  buildRoomProfilePutRequest,
  canEditRoomProfile,
  isProfileVersionConflict,
  roomProfileByteLength,
  roomProfileDraftError,
  roomProfileFromView,
  roomProfilePath,
} from '../src/api/roomProfile.ts'
import type { Room } from '../src/api/domain.ts'
import type { RoomProfileView } from '../src/api/contracts.ts'

const ROOM_ID = '22222222-2222-4222-8222-222222222222'
const TOKEN = 'clr_hs_test_token'
const BASE = 'http://127.0.0.1:8080'

const DRAWER = readFileSync(
  fileURLToPath(new URL('../src/components/celira/RoomInfoDrawer.tsx', import.meta.url)),
  'utf8',
)
const SHEET = readFileSync(
  fileURLToPath(new URL('../src/components/celira/RoomProfileEditSheet.tsx', import.meta.url)),
  'utf8',
)
const MOCK = readFileSync(
  fileURLToPath(new URL('../src/api/mock.ts', import.meta.url)),
  'utf8',
)

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(true)
})

// ── ① 字节口径与正文规则（与 Core §3.1 同一条）────────────────────────────

test('★ 计数按 **UTF-8 字节**：中文 3、Emoji 4、换行/Tab 1（不是 String.length）', () => {
  assert.equal(roomProfileByteLength(''), 0)
  assert.equal(roomProfileByteLength('abc'), 3)
  assert.equal(roomProfileByteLength('群简介'), 9, '一个汉字 3 字节 —— 不是 1')
  assert.equal(roomProfileByteLength('😀'), 4, 'BMP 外的 Emoji 是 4 字节')
  assert.equal(roomProfileByteLength('a\nb\tc'), 5, '换行与 Tab 都允许，各 1 字节')

  const chinese = '好'.repeat(666) // 1998 字节 / 666 个 UTF-16 码元
  assert.equal(chinese.length, 666)
  assert.equal(roomProfileByteLength(chinese), 1998)
})

test('★ 2000 字节是**含边界**的：2000 合法、2001 报超限（按字节而不是字数）', () => {
  const atLimit = `${'好'.repeat(666)}ab` // 1998 + 2
  assert.equal(roomProfileByteLength(atLimit), ROOM_PROFILE_MAX_BYTES)
  assert.equal(roomProfileDraftError(atLimit), null)

  const over = `${atLimit}c`
  assert.equal(roomProfileByteLength(over), ROOM_PROFILE_MAX_BYTES + 1)
  const hint = roomProfileDraftError(over)
  assert.ok(hint && hint.includes('2001'), `超限提示要说清当前字节数：${hint}`)
})

test('★ trim 只用于「是否为空」：纯空白非法，正文原样（含首尾空白）合法', () => {
  assert.ok(roomProfileDraftError(''))
  assert.ok(roomProfileDraftError('   \n\t  '), 'trim 后为空 ⇒ 非法（Core 同一裁决）')
  assert.equal(roomProfileDraftError(' 这个群在做什么 \n'), null, '首尾空白是正文的一部分，不许 trim 掉')
})

test('★ U+0000 一律拒绝（服务端 400 的三条正文规则之一）', () => {
  const hint = roomProfileDraftError(`正常${'\u0000'}正文`)
  assert.ok(hint && hint.includes('U+0000'))
})

// ── ② 写请求形状（真发出去的那一发）──────────────────────────────────────

interface Captured {
  url: string
  method: string
  body: string | undefined
  headers: Record<string, string>
}

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

test('★ 路径：`/v1/rooms/{id}/profile`，room id 走 encodeURIComponent', () => {
  assert.equal(roomProfilePath('abc'), '/v1/rooms/abc/profile')
  assert.equal(roomProfilePath('a/b c'), '/v1/rooms/a%2Fb%20c/profile')
})

test('★ 请求体是 snake_case 的 `base_version` + 原文 `description`（不 trim、不驼峰）', () => {
  const request = buildRoomProfilePutRequest(ROOM_ID, {
    baseVersion: 3,
    description: '  两行\n正文 😀  ',
  })
  assert.equal(request.method, 'PUT')
  assert.equal(request.path, `/v1/rooms/${ROOM_ID}/profile`)
  assert.deepEqual(Object.keys(request.body).sort(), ['base_version', 'description'])
  assert.equal(request.body.base_version, 3)
  assert.equal(request.body.description, '  两行\n正文 😀  ')
})

test('★ 真发出去的一发：PUT + Idempotency-Key + `{base_version, description}`，且两次 key 不同', async () => {
  const stub = stubFetch({
    json: {
      profile: { description: '新的简介', version: 4, updated_at: '2026-10-25T12:00:00+00:00' },
      idempotency_replayed: false,
      request_id: 'r1',
    },
  })
  try {
    const api = new CoreApi(TOKEN, BASE)
    const result = await api.putRoomProfile(ROOM_ID, { baseVersion: 3, description: '新的简介' })
    assert.equal(result.profile.version, 4, '版本号只认 Core 回执，本地不 +1')
    await api.putRoomProfile(ROOM_ID, { baseVersion: 4, description: '再一版' })

    assert.equal(stub.captured.length, 2)
    const [first, second] = stub.captured
    assert.equal(first.method, 'PUT')
    assert.ok(first.url.endsWith(`/v1/rooms/${ROOM_ID}/profile`), `路径要对：${first.url}`)
    assert.equal(first.headers.Authorization ?? first.headers.authorization, `Bearer ${TOKEN}`, 'Human Session 必须带上')

    const key = first.headers['Idempotency-Key'] ?? first.headers['idempotency-key']
    assert.ok(key && key.length > 0, '缺 Idempotency-Key ⇒ Core 回 400 idempotency_key_required')
    assert.ok(key.length <= 128 && !key.includes(',') && !/^\s|\s$/.test(key), 'key 形状：1–128 字节、无逗号、无首尾空白')
    const secondKey = second.headers['Idempotency-Key'] ?? second.headers['idempotency-key']
    assert.notEqual(secondKey, key, '每次调用一支新 key（同 key 会被 Core 当成重放）')

    const body = JSON.parse(String(first.body))
    assert.deepEqual(body, { base_version: 3, description: '新的简介' }, '字段名必须是 snake_case（服务端 deny_unknown_fields）')
    assert.ok(!('baseVersion' in body), '驼峰字段会被 400 invalid_request 整发拒掉')
  } finally {
    stub.restore()
  }
})

// ── 读面映射与「只改目标房」────────────────────────────────────────────────

const PROFILE: RoomProfileView = {
  description: '这个群在做什么',
  version: 2,
  updated_at: '2026-10-25T12:34:56.789+00:00',
}

test('★ `updated_at` → `updatedAt`：线协议与视图模型的唯一翻译点', () => {
  assert.deepEqual(roomProfileFromView(PROFILE), {
    description: '这个群在做什么',
    version: 2,
    updatedAt: '2026-10-25T12:34:56.789+00:00',
  })
  assert.equal(roomProfileFromView(null), null, 'null = 没有简介（不是错误）')
  assert.equal(roomProfileFromView(undefined), null)
})

function room(id: string, extra: Partial<Room> = {}): Room {
  return {
    id,
    name: id,
    participantIds: [],
    lastMessagePreview: '',
    lastActivityAt: '',
    unreadCount: 0,
    ...extra,
  }
}

test('★ 刷新一间房**不清空别的房**的 profile（列表端点本来就不带这一格）', () => {
  const other = { description: '别的群的简介', version: 7, updatedAt: '2026-10-01T00:00:00Z' }
  const rooms = [room('a'), room('b', { kind: 'group', profile: other })]
  const next = applyRoomProfile(rooms, 'a', PROFILE)

  assert.deepEqual(next[0].profile, roomProfileFromView(PROFILE), '目标房换成权威值')
  assert.deepEqual(next[1].profile, other, '★ 其它房间的 profile 原样保留，绝不被顺手清掉')
  assert.equal(rooms[0].profile, undefined, '纯函数不改入参')
  assert.equal(next[1], rooms[1], '非目标房连对象都不重建')
})

test('★ 目标房真的没有简介时，权威 `null` 会落成「还没有」', () => {
  const rooms = [room('a', { kind: 'group', profile: { description: '旧', version: 1, updatedAt: 'x' } })]
  assert.equal(applyRoomProfile(rooms, 'a', null)[0].profile, null)
})

// ── 可编辑判据（group + owner + 传输层真的实现了写面）──────────────────────

test('★ 编辑入口只在「group + 群主 + 有写能力」三者同时成立时出现', () => {
  assert.equal(canEditRoomProfile({ kind: 'group', isOwner: true, supported: true }), true)
  assert.equal(canEditRoomProfile({ kind: 'group', isOwner: false, supported: true }), false, '成员只读')
  assert.equal(canEditRoomProfile({ kind: 'group', isOwner: true, supported: false }), false, '写面没实现 ⇒ 不给假按钮')
  assert.equal(canEditRoomProfile({ kind: 'direct', isOwner: true, supported: true }), false, 'direct 没有这一格')
  assert.equal(canEditRoomProfile({ kind: 'human_direct', isOwner: true, supported: true }), false, 'human_direct 没有这一格')
  assert.equal(canEditRoomProfile({ kind: undefined, isOwner: true, supported: true }), false, 'kind 未知时不猜')
})

// ── 错误码 → 人话（单一事实源在 contractErrors）────────────────────────────

test('★ 409 的文案是产品原话；403 / 400 / 413 都有人话（绝不上屏 `code (HTTP n)`）', () => {
  assert.equal(apiErrorText('profile_version_conflict'), '简介已被更新，请刷新后重试')
  assert.equal(apiErrorText('not_room_owner'), '只有群主可以这样操作')
  assert.equal(apiErrorText('invalid_room_kind'), '这个房间类型不支持这项操作')
  assert.match(apiErrorText('invalid_profile_description') ?? '', /1–2000 字节/)
  assert.match(apiErrorText('payload_too_large') ?? '', /过长/)
  for (const code of [
    'profile_version_conflict',
    'not_room_owner',
    'invalid_room_kind',
    'invalid_profile_description',
  ]) {
    assert.ok(!apiErrorText(code)?.includes(code), `${code} 的分类码不许直接上屏`)
  }
})

test('★ 只有 409 `profile_version_conflict` 触发权威重读（其余失败不自动改写任何东西）', () => {
  assert.equal(isProfileVersionConflict({ code: 'profile_version_conflict' }), true)
  assert.equal(isProfileVersionConflict({ code: 'not_room_owner' }), false)
  assert.equal(isProfileVersionConflict(new Error('Failed to fetch')), false)
  assert.equal(isProfileVersionConflict(null), false)
})

// ── ③ 组件源码守卫（行为在真机由探针量；这里防"改回去"）────────────────────

test('★ 抽屉：群简介区块**只在 group 房**出现，且空态是「还没有群简介」', () => {
  assert.match(
    DRAWER,
    /\{isGroup && \(\s*<section[\s\S]{0,400}aria-labelledby="room-profile-heading"/,
    '群简介区块必须被 isGroup 包着（direct / human_direct 完全不显示）',
  )
  assert.ok(DRAWER.includes('还没有群简介'), '空态文案是约定的那一句')
  assert.equal(
    DRAWER.match(/aria-labelledby="room-profile-heading"/g)?.length,
    1,
    '只有一个群简介区块（复制第二份迟早与判据打架）',
  )
  assert.ok(
    DRAWER.indexOf('aria-labelledby="room-profile-heading"') <
      DRAWER.indexOf('{tab === \'members\' && ('),
    '区块在三个页签的内容之上，不新开 tab',
  )
})

test('★ 抽屉：编辑入口只给群主（isOwner）+ 写能力探测，非群主只读', () => {
  assert.match(DRAWER, /const profileEditable = canEditRoomProfile\(\{ kind: room\.kind, isOwner, supported: profileSupported \}\)/)
  assert.match(DRAWER, /\{profileEditable && \(\s*<Button[\s\S]{0,300}aria-label="编辑群简介"/)
  assert.match(DRAWER, /\{isGroup && profileEditable && \(\s*<RoomProfileEditSheet/)
})

test('★ 编辑 sheet：字节计数 / 2000、超限或空白禁用保存、保存中防重复、409 重读', () => {
  assert.ok(SHEET.includes('{bytes}/{ROOM_PROFILE_MAX_BYTES} 字节'), '屏幕上要有 `n/2000` 字节计数')
  assert.ok(SHEET.includes('roomProfileByteLength'), '计数必须按 UTF-8 字节，不许 String.length')
  assert.match(SHEET, /disabled=\{!canSave\}/, '超限 / trim 后为空时禁用保存')
  assert.match(SHEET, /if \(saving\) return/, '保存中防重复点击')
  assert.match(SHEET, /baseVersion/, '提交必须带权威读面的 baseVersion')
  assert.match(SHEET, /isProfileVersionConflict\(e\)[\s\S]{0,400}await refreshRoom\(room\.id\)/, '409 先权威重读')
  assert.match(SHEET, /errorText\(e\)/, '错误走 contractErrors 的人话映射（不许静默、不许回显服务端正文）')
})

test('★ 演示通道：内存实现真的在（getRoom 读 profile / putRoomProfile 写），演示不崩', () => {
  assert.match(MOCK, /private roomProfiles = new Map<string, RoomProfileView>\(\)/)
  assert.match(MOCK, /profile: room\?\.kind === 'group' \? \(this\.roomProfiles\.get\(roomId\) \?\? null\) : null/)
  assert.match(MOCK, /async putRoomProfile\([\s\S]{0,200}Promise<PutRoomProfileResponse>/)
})
