// **服务端提醒档（attention）＋ 通知门控**的判据（2026-10-23 立）。
//
// 这一刀新加的三件事，各自都有一条"错了也看不出来"的路：
//
//  ① **映射**：`GET /v1/rooms` 每行的 `attention` → `Room.attention`。缺字段 / 旧服务端 /
//     `null` 都必须读成 `all`（契约 §9.1「缺行 = all」）。映射反了（缺省读成 `mentions`）
//     不会报错，只会**静默吞掉**本该提醒的群消息 —— 用户以为"没人说话"。
//  ② **store 动作**：乐观更新 + 服务端回读 + 失败回退。少任一半都会变成
//     「拨了没反应」或「自己弹回去还说没事」（这一格在置顶/免打扰上真出过一次）。
//     本仓 client 没有 jsdom（`node --test` + 类型剥离），所以形状判据按源码钉
//     （与 `no-silent-red-path.test.ts` 同规），可纯逻辑那一半直测。
//  ③ **通知门控**：`muted`（本机完全静音）压过 `attention=mentions`，而
//     `attention=mentions` 只在**结构化 mentions 含我**时放行。顺序错了就是"免打扰还响"。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { CoreApi, toRoomListEntry } from '../src/api/core.ts'
import type { RoomListEntryView } from '../src/api/contracts.ts'
import type { RoomAttention } from '../src/api/domain.ts'
import {
  DEFAULT_ROOM_ATTENTION,
  attentionForChecked,
  attentionSwitchChecked,
  isRoomAttention,
  parseRoomAttention,
  withRoomAttention,
} from '../src/api/roomAttention.ts'
import { shouldNotifyMessage, type NotificationFacts } from '../src/api/notificationGate.ts'
import { activeTransport, setTransport, type Transport, type TransportResponse } from '../src/api/transport.ts'

/** 门禁不许空跑：最后一条断言用例数与声明数一致（与 `local-state.test.ts` 同款纪律）。 */
let completed = 0
const DECLARED_CASES = 27
function attentionCase(name: string, fn: () => void | Promise<void>) {
  test(name, async () => {
    await fn()
    completed += 1
  })
}

const read = (relative: string) =>
  readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8')

// ── 1. 宽容解析：缺省 = all（不许猜成 mentions）────────────────────────────

attentionCase('解析：认契约的两个字面量', () => {
  assert.equal(parseRoomAttention('all'), 'all')
  assert.equal(parseRoomAttention('mentions'), 'mentions')
  assert.equal(DEFAULT_ROOM_ATTENTION, 'all')
})

attentionCase('★ 解析：缺字段 / null / 空串 / 大小写变体 / 别的类型一律归 `all`（缺行 = all）', () => {
  for (const raw of [undefined, null, '', 'Mentions', 'MENTIONS', 'every', 0, 42, {}, [], true]) {
    assert.equal(
      parseRoomAttention(raw),
      'all',
      `输入 ${JSON.stringify(raw)} 必须读成 all：猜成 mentions 会静默吞掉本该提醒的群消息`,
    )
  }
})

attentionCase('判定：只有契约的两个字面量算合法档', () => {
  assert.equal(isRoomAttention('all'), true)
  assert.equal(isRoomAttention('mentions'), true)
  for (const raw of ['All', 'MENTIONS', '', null, undefined, 1]) {
    assert.equal(isRoomAttention(raw), false, `输入 ${JSON.stringify(raw)} 不该算合法档`)
  }
})

// ── 2. 开关投影：checked 与目标值都只有一处判据 ─────────────────────────────

attentionCase('开关：`mentions` 才是开（缺省 / all / undefined 都是关）', () => {
  assert.equal(attentionSwitchChecked('mentions'), true)
  assert.equal(attentionSwitchChecked('all'), false)
  assert.equal(attentionSwitchChecked(undefined), false)
})

attentionCase('开关：开 = 写 mentions，关 = 写 all（不是"翻转当前值"）', () => {
  assert.equal(attentionForChecked(true), 'mentions')
  assert.equal(attentionForChecked(false), 'all')
})

// ── 3. 列表投影：纯函数、不改入参、不变就不造新对象 ─────────────────────────

attentionCase('投影：只改目标房间，**不改入参**', () => {
  const rooms = [{ id: 'r-1', attention: 'all' as RoomAttention }, { id: 'r-2' }]
  const out = withRoomAttention(rooms, 'r-1', 'mentions')
  assert.equal(out[0].attention, 'mentions')
  assert.equal(out[1].attention, undefined, '别的房间一个字都不许动')
  assert.equal(rooms[0].attention, 'all', '入参不许被改')
  assert.notEqual(out, rooms)
})

attentionCase('投影：值没变 ⇒ 返回同一个数组 / 同一个对象（不制造无谓重渲染）', () => {
  const room = { id: 'r-1', attention: 'mentions' as RoomAttention }
  const rooms = [room]
  const out = withRoomAttention(rooms, 'r-1', 'mentions')
  assert.equal(out, rooms, '一间都没变 ⇒ 原数组原样返回')
  assert.equal(out[0], room)
})

attentionCase('投影：房间不在列表里 ⇒ 原数组原样返回（不凭空造行）', () => {
  const rooms = [{ id: 'r-1', attention: 'all' as RoomAttention }]
  assert.equal(withRoomAttention(rooms, 'r-9', 'mentions'), rooms)
})

// ── 4. 映射：线协议行 → RoomListEntry（缺字段按 all）────────────────────────

/** 一条房间行的线协议形状（只给投影真正读的那几格）。 */
function row(over: Partial<RoomListEntryView> = {}): RoomListEntryView {
  return {
    room: {
      id: 'r-1',
      kind: 'group',
      status: 'active',
      name: '周末读书会',
      created_at: '2026-10-20T00:00:00.000Z',
    },
    membership: {
      membership_id: 'm-1',
      participant_id: 'p-1',
      role: 'member',
      joined_at: '2026-10-20T00:00:00.000Z',
      left_at: null,
      closed_by: null,
    },
    peer: null,
    read_state: { last_read_message_id: null, unread_count: 3 },
    peer_read: null,
    attention: 'all',
    ...over,
  }
}

attentionCase('映射：`mentions` 原样进 Room，其余字段不动', () => {
  const entry = toRoomListEntry(row({ attention: 'mentions' }))
  assert.equal(entry.room.attention, 'mentions')
  assert.equal(entry.room.kind, 'group')
  assert.equal(entry.room.unreadCount, 3)
  assert.deepEqual(entry.peer, [])
})

attentionCase('★ 映射：行里**没有** attention 字段 ⇒ `all`（旧服务端 / 中间层省略）', () => {
  const legacy = row()
  delete (legacy as { attention?: string | null }).attention
  assert.equal(toRoomListEntry(legacy).room.attention, 'all')
})

attentionCase('映射：`null` / 认不出的值 ⇒ `all`（不抛、不留 undefined）', () => {
  assert.equal(toRoomListEntry(row({ attention: null })).room.attention, 'all')
  assert.equal(toRoomListEntry(row({ attention: 'mentions_only' })).room.attention, 'all')
})

attentionCase('映射：两人房（peer 在场）照常投影，attention 仍按 `all` 读', () => {
  const entry = toRoomListEntry(
    row({
      attention: undefined,
      room: {
        id: 'r-2',
        kind: 'direct',
        status: 'active',
        name: null,
        created_at: '2026-10-20T00:00:00.000Z',
      },
      peer: { id: 'p-2', kind: 'agent', display_name: 'Inori', status: 'active' },
    }),
  )
  assert.equal(entry.room.attention, 'all')
  assert.equal(entry.room.name, 'Inori', '没有房名时用对端显示名（既有行为）')
  assert.equal(entry.peer.length, 1)
})

// ── 5. core 的写入路径：PUT + 只带 level + 以回读值为准 ─────────────────────

/** 一个记录请求的假出网口：按给定响应体应答。 */
function fakeTransport(payload: unknown): { transport: Transport; seen: Array<{ url: string; method: string; body?: string }> } {
  const seen: Array<{ url: string; method: string; body?: string }> = []
  const response: TransportResponse = {
    status: 200,
    ok: true,
    header: () => null,
    json: async () => payload,
    blob: async () => new Blob([]),
  }
  return {
    seen,
    transport: {
      async send(request) {
        seen.push({ url: request.url, method: request.method, body: request.body as string | undefined })
        return response
      },
    },
  }
}

attentionCase('★ core：`setRoomAttention` 打 `PUT /v1/rooms/{id}/attention`，body 只有 level', async () => {
  const before = activeTransport()
  const fake = fakeTransport({ attention: 'mentions', request_id: 'req-1' })
  setTransport(fake.transport)
  try {
    const api = new CoreApi('tok', 'http://core.test')
    const applied = await api.setRoomAttention('r-1', 'mentions')
    assert.equal(applied, 'mentions', '返回值是服务端回读的权威取值')
    assert.equal(fake.seen.length, 1)
    assert.equal(fake.seen[0].url, 'http://core.test/v1/rooms/r-1/attention')
    assert.equal(fake.seen[0].method, 'PUT', '状态写；GET 会让整个开关变成只读')
    assert.deepEqual(JSON.parse(fake.seen[0].body ?? '{}'), { level: 'mentions' }, '请求体只有 level（deny_unknown_fields）')
  } finally {
    setTransport(before)
  }
})

attentionCase('core：回读值认不出时按 `all` 收（请求已成功，不许在投影处抛）', async () => {
  const before = activeTransport()
  const fake = fakeTransport({ attention: 'something_new', request_id: 'req-2' })
  setTransport(fake.transport)
  try {
    const api = new CoreApi('tok', 'http://core.test')
    assert.equal(await api.setRoomAttention('r-1', 'all'), 'all')
  } finally {
    setTransport(before)
  }
})

// ── 6. 通知门控：顺序 = 免打扰 > 提醒档 > 偏好 ──────────────────────────────

/** 一条"该提醒"的基线事实：三个偏好全开、群消息、别人发的、提到了我。 */
function facts(over: Partial<NotificationFacts> = {}): NotificationFacts {
  return {
    notificationsOn: true,
    mentionsOnly: false,
    muted: false,
    attention: 'all',
    system: false,
    fromSelf: false,
    mentionedMe: false,
    ...over,
  }
}

attentionCase('门控：总开关开、`all` ⇒ 普通群消息也提醒', () => {
  assert.equal(shouldNotifyMessage(facts()), true)
})

attentionCase('★ 门控：`mentions` + 没提到我 ⇒ **不弹**（这一档的全部意义）', () => {
  assert.equal(shouldNotifyMessage(facts({ attention: 'mentions' })), false)
})

attentionCase('★ 门控：`mentions` + mentions 含我 ⇒ 弹（并且不必等总开关）', () => {
  assert.equal(shouldNotifyMessage(facts({ attention: 'mentions', mentionedMe: true })), true)
  assert.equal(
    shouldNotifyMessage(
      facts({ attention: 'mentions', mentionedMe: true, notificationsOn: false, mentionsOnly: true }),
    ),
    true,
    '只开了「提及提醒」时，@ 我照常弹',
  )
})

attentionCase('★ 门控：本机免打扰压过一切（含 attention=all、含 @ 我）', () => {
  assert.equal(shouldNotifyMessage(facts({ muted: true })), false)
  assert.equal(shouldNotifyMessage(facts({ muted: true, attention: 'all', mentionedMe: true })), false)
  assert.equal(shouldNotifyMessage(facts({ muted: true, attention: 'mentions', mentionedMe: true })), false)
})

attentionCase('门控：attention 缺省（undefined）= `all`，不静默吞消息', () => {
  assert.equal(shouldNotifyMessage(facts({ attention: undefined })), true)
})

attentionCase('门控：两个开关都关 ⇒ 一律不弹', () => {
  assert.equal(shouldNotifyMessage(facts({ notificationsOn: false, mentionsOnly: false })), false)
  assert.equal(
    shouldNotifyMessage(facts({ notificationsOn: false, mentionsOnly: false, mentionedMe: true })),
    false,
  )
})

attentionCase('门控：只开「提及提醒」⇒ 只有 @ 我才弹', () => {
  const base = { notificationsOn: false, mentionsOnly: true }
  assert.equal(shouldNotifyMessage(facts({ ...base, mentionedMe: true })), true)
  assert.equal(shouldNotifyMessage(facts({ ...base, mentionedMe: false })), false)
})

attentionCase('门控：系统消息 / 我自己发的不是"新消息"（哪怕 @ 我也一样）', () => {
  assert.equal(shouldNotifyMessage(facts({ system: true, mentionedMe: true })), false)
  assert.equal(shouldNotifyMessage(facts({ fromSelf: true })), false)
})

attentionCase('门控：`mentions` 档 + 只开「提及提醒」+ 没 @ 我 ⇒ 不弹（两条都要求 @）', () => {
  assert.equal(
    shouldNotifyMessage(facts({ attention: 'mentions', notificationsOn: false, mentionsOnly: true })),
    false,
  )
})

// ── 7. 接线守卫（client 没有 jsdom：形状按源码钉）─────────────────────────

attentionCase('★ store 门控只走唯一裁决面：`maybeNotify` 把事实交给 `shouldNotifyMessage`', () => {
  const store = read('../src/api/store.ts')
  const start = store.indexOf('async function maybeNotify(')
  assert.ok(start >= 0, '`maybeNotify` 不见了：门控的调用点被删/改名')
  const body = store.slice(start, store.indexOf('\n}', start))
  assert.match(body, /shouldNotifyMessage\(\{/)
  assert.match(body, /attention: room\?\.attention/, '提醒档必须从房间列表读（乐观更新就落在那里）')
  assert.match(body, /muted: room\?\.muted === true/, '本机免打扰仍必须参与裁决')
  // 反例守卫：自己再写一套 `if (attention...)` 就是第二份判据。
  assert.ok(
    !/attention\s*===\s*'mentions'/.test(body),
    '`maybeNotify` 里不许自己判 attention —— 判据只有 `api/notificationGate.ts` 一处',
  )
})

attentionCase('★ store 动作：乐观先落、失败回退并重抛（不许静默成"已保存"）', () => {
  const store = read('../src/api/store.ts')
  const start = store.indexOf('setRoomAttention: async (roomId, level)')
  assert.ok(start >= 0, '`setRoomAttention` 动作不见了')
  const body = store.slice(start, store.indexOf('\n    },', start))
  const optimistic = body.indexOf('withRoomAttention(s.rooms, roomId, level)')
  const request = body.indexOf('await apiNow.setRoomAttention')
  const rollback = body.indexOf('withRoomAttention(s.rooms, roomId, current)')
  const rethrow = body.indexOf('throw error')
  assert.ok(optimistic >= 0, '乐观更新丢了：开关会等一个来回才动')
  assert.ok(request > optimistic, '顺序必须"先乐观、再发请求"')
  assert.ok(rollback > request, '失败回退丢了：开关会自己弹回去')
  assert.ok(rethrow > rollback, '失败必须重抛（由界面说那句话），不许吞')
  assert.match(body, /if \(!apiNow\?\.setRoomAttention\) return/, '没有这一档的实现不许假装写成功')
})

attentionCase('★ 抽屉：开关只在 group 房 + 有能力时出现，checked / 目标值都走投影函数', () => {
  const drawer = read('../src/components/celira/RoomInfoDrawer.tsx')
  assert.match(
    drawer,
    /isGroup && attentionSupported/,
    '两人房不许画这一档（Core 对 direct 房回 404）',
  )
  assert.match(drawer, /checked=\{attentionSwitchChecked\(room\.attention\)\}/)
  assert.match(drawer, /setRoomAttention\(room\.id, attentionForChecked\(checked\)\)/)
  assert.match(drawer, /typeof getApi\(\)\?\.setRoomAttention === 'function'/, '能力探测按"方法在不在"判')
  assert.match(drawer, /消息免打扰/, '本机完全静音那一格必须还在（attention 不许取代它）')
  assert.match(drawer, /仍会完全静音/, '文案必须说清 muted 压过这一档（否则用户以为开了它就一定安静）')
})

// ── 8. 门禁不许空跑 ─────────────────────────────────────────────────────────

test('门禁不许空跑：本文件真的执行了全部用例', () => {
  assert.equal(completed, DECLARED_CASES - 1, `实际执行 ${completed} 条，声明 ${DECLARED_CASES - 1} 条`)
})
