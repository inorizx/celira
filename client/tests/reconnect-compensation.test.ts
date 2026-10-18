// 「WS 断线重连 → 从持久化游标 REST 补拉」链的行为测试（2026-10-03 补封板缺口）。
//
// 契约判据（`docs/contracts/core-v1/CORE_V1_IM_CONTRACT.md` §10）：**唤醒可全丢、
// REST 恢复不能丢**。链路的形状是：
//
//   Core 提交公共事实 → 原子生成 Human Event（耐久，SQLite）
//     → WS 只发 `events.available`（易失提示；`client/src/api/core.ts` 的
//       `openWs`：掉线按 2s 起、翻倍、封顶 30s 的退避重连）
//     → 客户端从**持久化游标**（localStorage，`api/store.ts` 的 `pollEvents`）
//       走 `GET /v1/human/events?after=` 补拉。
//
// 本文件用**行为**证明这条链会收敛：判据是「事件最终一致」（不丢、不重、顺序对），
// 不是内部调用次数。四条场景：
//   ① WS 断开后按退避重连（2s → 4s → 8s；连上后计数复位）；
//   ② 重连成功后从**持久化游标**补拉（不丢、不重、顺序对；同一位置重拉稳定）；
//   ③ 断线窗口里产生的事件在恢复后补上；
//   ④ 补拉时 Core 还没起来 ⇒ 重试而不是把事件搁丢（a：整个 Core 不在；b：补拉端点失败）。
//
// ## 这个文件怎么把 store.ts 装进 `node --test`
//
// `api/store.ts` 的值导入**不带 `.ts` 扩展名**（它是给 Vite 打包的形状），而 Node 的
// ESM 解析不认无扩展名的相对路径（`core.ts` 头部记着同一条坑的另一面）⇒ 直接
// `import '../src/api/store.ts'` 会 `ERR_MODULE_NOT_FOUND: ./core`。
// 这里在文件顶部注册一层**解析钩子**（`node:module` 的同步钩子，Node 22.15+）：
// 相对路径补 `.ts`、`@/` 映射到 `src/`。**产品代码一行不改**；钩子只影响本进程。
//
// ## 环境（都在 import store.ts 之前摆好）
//
// - `localStorage`：内存版（游标就住在它里面 —— 「持久化」那一半的载体）；
// - `WebSocket`：可摆布的假连接（服务端行为由测试精确控制：开/认证/掉线/连不上）；
// - `fetch`：内存版 Core（只实现这条链用到的端点，事件分页贴 §10.3：
//   严格升序、`next_after` 空页原样回传实际采用的游标）；
// - `BroadcastChannel`：换成空壳（node 里也有真的，会让测试进程挂住）。
import assert from 'node:assert/strict'
import test, { type TestContext } from 'node:test'
import { registerHooks } from 'node:module'

// ─────────────────────────────────────────────────────────────────────────────
// 0. 环境（必须先于 store.ts 的 import：它在 import 期就把 prefs 读进内存）
// ─────────────────────────────────────────────────────────────────────────────

class MemStorage {
  private m = new Map<string, string>()
  getItem(k: string) {
    return this.m.has(k) ? (this.m.get(k) as string) : null
  }
  setItem(k: string, v: string) {
    this.m.set(k, String(v))
  }
  removeItem(k: string) {
    this.m.delete(k)
  }
  key(i: number) {
    return [...this.m.keys()][i] ?? null
  }
  get length() {
    return this.m.size
  }
  clear() {
    this.m.clear()
  }
}
const fakeStorage = new MemStorage()
Object.defineProperty(globalThis, 'localStorage', { value: fakeStorage, configurable: true })
// ⚠ prefs 必须在 import store.ts **之前**种上：store 在 import 期就把 prefs 读进内存了。
fakeStorage.setItem(
  'celira.connection.prefs',
  JSON.stringify({
    mode: 'core',
    token: 'clr_test_token',
    serverUrl: 'http://127.0.0.1:9',
    worldId: '',
  }),
)

class SilentChannel {
  postMessage() {}
  close() {}
}
Object.defineProperty(globalThis, 'BroadcastChannel', { value: SilentChannel, configurable: true })

// ─────────────────────────────────────────────────────────────────────────────
// 1. 假世界：内存版 Core（行为贴 §10.3 的事件分页契约）
// ─────────────────────────────────────────────────────────────────────────────

const ROOM = '11111111-1111-4111-8111-111111111111'
const ME = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa'
const PEER = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'
const START_CURSOR = 'c1.he.start'
const ISO = '2026-10-03T10:00:00.000Z'
const HOME_ID = 'home-reconnect-test'

interface WireEvent {
  id: string
  kind: string
  room_id: string
  snapshot: Record<string, unknown>
  created_at: string
}

const world = {
  /** Core 进程在不在（不在 ⇒ 所有请求连不上，WS 也连不上）。 */
  up: true,
  /** 「补拉」那个端点没就绪（Core 还没完全起来的最窄形状：WS 已能连、REST 还在 502）。 */
  eventsDown: false,
  /** 该 Human 的 delivery 序列（升序）。 */
  events: [] as WireEvent[],
}

/** 一条 `message.created` 事件（快照按 §10.1：固化 id/actor/body/…，不含 sequence）。 */
function messageEvent(
  eventId: string,
  messageId: string,
  body: string,
  actorId: string,
  actorName: string,
): WireEvent {
  return {
    id: eventId,
    kind: 'message.created',
    room_id: ROOM,
    created_at: ISO,
    snapshot: {
      id: messageId,
      room_id: ROOM,
      actor: { id: actorId, kind: 'human', display_name: actorName },
      body,
      mentions: [],
      attachments: [],
      reply_to_message_id: null,
      recalled_at: null,
    },
  }
}

/** §10.3 的分页：升序；非空页 `next_after` = 最后一条的游标；空页 = 实际采用的游标。 */
function eventsPage(after: string | undefined): { events: WireEvent[]; next_after: string } {
  let start = 0
  if (after && after !== START_CURSOR) {
    const id = after.replace('c1.he.', '')
    const idx = world.events.findIndex((e) => e.id === id)
    assert.ok(idx >= 0, `假世界收到未知游标：${after}`)
    start = idx + 1
  }
  const events = world.events.slice(start, start + 50)
  const nextAfter = events.length > 0 ? `c1.he.${events[events.length - 1].id}` : (after ?? START_CURSOR)
  return { events, next_after: nextAfter }
}

globalThis.fetch = (async (input: unknown) => {
  const url = new URL(String(input), 'http://world.test')
  const json = (status: number, body: unknown) =>
    new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } })
  if (!world.up) throw new TypeError('fetch failed: core 不在')
  switch (url.pathname) {
    case '/v1/me':
      return json(200, {
        participant: { id: ME, kind: 'human', display_name: '我', status: 'active', created_at: ISO },
        session: { id: 's-1', created_at: ISO },
        request_id: 'req',
      })
    case '/v1/home':
      return json(200, { home_id: HOME_ID, display_name: null, created_at: ISO })
    case '/v1/rooms':
      return json(200, {
        rooms: [
          {
            room: { id: ROOM, kind: 'group', status: 'active', name: '值班室', created_at: ISO },
            membership: { membership_id: 'm-1', participant_id: ME, role: 'owner', joined_at: ISO, left_at: null },
            peer: null,
            read_state: { last_read_message_id: null, unread_count: 0 },
          },
        ],
      })
    case '/v1/contacts':
      return json(200, { contacts: [] })
    case '/v1/blocks':
      return json(200, { blocks: [] })
    case '/v1/agents':
      return json(200, { agents: [] })
    case '/v1/human/events':
      if (world.eventsDown) return json(502, { error: { code: 'internal_error', message: '还没起来' } })
      return json(200, eventsPage(url.searchParams.get('after') ?? undefined))
    default:
      return json(404, { error: { code: 'not_found', message: url.pathname } })
  }
}) as typeof fetch

// ─────────────────────────────────────────────────────────────────────────────
// 2. 假 WebSocket：服务端行为可被测试精确摆布（开/认证/掉线/连不上）
// ─────────────────────────────────────────────────────────────────────────────

class FakeSocket {
  static made: FakeSocket[] = []
  readyState = 0
  sent: string[] = []
  url: string
  onopen: (() => void) | null = null
  onmessage: ((ev: { data: string }) => void) | null = null
  onclose: ((ev: { code: number }) => void) | null = null
  onerror: (() => void) | null = null

  constructor(url: string) {
    this.url = String(url)
    FakeSocket.made.push(this)
    // 服务端行为：Core 在 ⇒ 连上、收到认证帧后回 `authenticated`；
    // Core 不在 ⇒ 像真浏览器那样以 1006 收场（onopen 不触发）。
    queueMicrotask(() => {
      if (this.readyState !== 0) return
      if (!world.up) {
        this.serverClose(1006)
        return
      }
      this.readyState = 1
      this.onopen?.()
      queueMicrotask(() => {
        if (this.readyState !== 1) return
        if (this.sent.some((s) => s.includes('authenticate'))) this.serverFrame({ type: 'authenticated' })
      })
    })
  }
  send(data: string) {
    this.sent.push(String(data))
  }
  close(code = 1000) {
    this.serverClose(code)
  }
  /** 服务端推一帧（唤醒只表达「可能有新事件」）。 */
  serverFrame(obj: unknown) {
    this.onmessage?.({ data: JSON.stringify(obj) })
  }
  /** 服务端侧掉线（网络断 / Core 停）。 */
  serverClose(code: number) {
    if (this.readyState === 3) return
    this.readyState = 3
    this.onclose?.({ code })
  }
  get open() {
    return this.readyState === 1
  }
}
Object.defineProperty(globalThis, 'WebSocket', { value: FakeSocket, configurable: true })

// ─────────────────────────────────────────────────────────────────────────────
// 3. 让 node 直载 store.ts（见文件头；产品代码一行不改）
// ─────────────────────────────────────────────────────────────────────────────

const SRC = new URL('../src/', import.meta.url)
registerHooks({
  resolve(spec, ctx, next) {
    if ((spec.startsWith('./') || spec.startsWith('../')) && !/\.[a-z]+$/i.test(spec)) {
      try {
        return next(spec + '.ts', ctx)
      } catch {
        /* 落回原样 */
      }
    }
    if (spec.startsWith('@/')) return next(new URL(spec.slice(2) + '.ts', SRC).href, ctx)
    return next(spec, ctx)
  },
})

const { useAppStore, hydrateLocalState } = await import(
  new URL('../src/api/store.ts', import.meta.url).href
)

/**
 * **补水**（2026-10-08 两段式启动之后新增的启动契约）。
 *
 * 从前 store 在 **import 期**就同步把 prefs 读进内存了（上面第 65 行那句注释说的就是它），
 * 所以种完 `localStorage` 直接 import 就够。现在**模块求值期只有缺省值**：真值由
 * `hydrateLocalState()` 经过存储窄口读进来（正门是 `main.tsx`，在 `createRoot` 之前调）。
 *
 * ⇒ 这个文件模拟"启动"就得**照做**：种好 `localStorage` → import → **补一次水**。
 * 不补的话 `worldKey()` 会是缺省值（空 token/worldId），落在盘上的游标就找不到 —— 表现为
 * "补拉回来的是空数组"。**这不是把断言改松**，是启动契约变了、模拟启动的那一步要跟上。
 */
await hydrateLocalState()

// ─────────────────────────────────────────────────────────────────────────────
// 4. 夹具
// ─────────────────────────────────────────────────────────────────────────────

const PREFS_KEY = 'celira.connection.prefs'
const CURSOR_KEY = `celira.world:${HOME_ID}:cursor`

function seedPrefs() {
  fakeStorage.setItem(
    PREFS_KEY,
    JSON.stringify({
      mode: 'core',
      token: 'clr_test_token',
      serverUrl: 'http://127.0.0.1:9',
      worldId: '',
    }),
  )
}

/** 让所有在途微任务跑完（假定时器只管 setTimeout；setImmediate 仍是真实的）。 */
async function settle(turns = 4) {
  for (let i = 0; i < turns; i += 1) await new Promise((r) => setImmediate(r))
}

/**
 * **唤醒流**那一条（`/v1/human/events/stream`）。
 *
 * ⚠ 2026-10-09 起客户端有**两条** WS：唤醒流与**通话流**（`/v1/call/stream`，带事实、
 * 独立退避）。本文件量的是唤醒这条链 ⇒ 计数一律**按 URL 认流**，不按"所有 socket"数
 * （那会把通话流的连接算进来，读成"多了一次重连"）。
 */
function wakeupSockets() {
  return FakeSocket.made.filter((s) => s.url.includes('/v1/human/events/stream'))
}

function socketsSince(n: number) {
  return wakeupSockets().slice(n)
}

async function connectAndSettle() {
  await useAppStore.getState().connect()
  await settle()
}

function messages() {
  return (useAppStore.getState().messagesByRoom[ROOM] ?? []).map((m) => m.body)
}

function resetWorld() {
  world.up = true
  world.eventsDown = false
  world.events = []
  FakeSocket.made = []
  fakeStorage.clear()
  seedPrefs()
  useAppStore.setState({
    connection: 'idle',
    me: null,
    rooms: [],
    messagesByRoom: {},
    nextBeforeByRoom: {},
    contacts: [],
    pendingContacts: [],
    blocks: [],
    ownedAgents: [],
    ownedAgentsLoading: false,
    ownedAgentsError: undefined,
    anjiaByAgent: {},
  })
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. 用例
// ─────────────────────────────────────────────────────────────────────────────

test('对照组：连上 → 世界侧来事件 → 唤醒 → 从游标补上', async (t: TestContext) => {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  resetWorld()
  world.events.push(messageEvent('e1', 'm1', '第一条', PEER, '阿元'))
  await connectAndSettle()

  assert.equal(useAppStore.getState().connection, 'connected')
  assert.deepEqual(messages(), ['第一条'])
  assert.equal(fakeStorage.getItem(CURSOR_KEY), 'c1.he.e1')

  // 世界侧又来一条 ⇒ WS 只发「可能有新事件」 ⇒ 客户端 REST 补拉
  world.events.push(messageEvent('e2', 'm2', '第二条', PEER, '阿元'))
  const sock = wakeupSockets()[wakeupSockets().length - 1]
  sock.serverFrame({ type: 'events.available' })
  await settle()
  assert.deepEqual(messages(), ['第一条', '第二条'])
  assert.equal(fakeStorage.getItem(CURSOR_KEY), 'c1.he.e2')
})

test('② 补拉从**持久化**游标开始（跨会话：盘上已有游标，不重放旧事件）', async (t: TestContext) => {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  resetWorld()
  world.events.push(messageEvent('e1', 'm1', '很久以前', PEER, '阿元'))
  world.events.push(messageEvent('e2', 'm2', '上次读到这', PEER, '阿元'))
  world.events.push(messageEvent('e3', 'm3', '这次开机之后', PEER, '阿元'))
  // 「上一次会话」留下的游标（e2 已读）——补拉必须从它开始，e1/e2 不许重放。
  fakeStorage.setItem(CURSOR_KEY, 'c1.he.e2')
  await connectAndSettle()
  assert.deepEqual(messages(), ['这次开机之后'], '只补游标之后的事件，不重放旧事件')
  assert.equal(fakeStorage.getItem(CURSOR_KEY), 'c1.he.e3')
})

test('① 断线后按退避重连：2s → 4s → 8s，连上后计数复位', async (t: TestContext) => {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  resetWorld()
  await connectAndSettle()
  const base = wakeupSockets().length
  assert.equal(base, 1)
  assert.equal(useAppStore.getState().connection, 'connected')

  // 掉线，且 Core 不在 ⇒ 每一次重连尝试都会失败（onclose 1006）
  world.up = false
  wakeupSockets()[0].serverClose(1006)
  await settle()
  assert.equal(useAppStore.getState().connection, 'reconnecting')

  // 第一次重连不能早于 2s
  t.mock.timers.tick(1999)
  await settle()
  assert.equal(socketsSince(base).length, 0, '1999ms 时不许重连')
  t.mock.timers.tick(1)
  await settle()
  assert.equal(socketsSince(base).length, 1, '2000ms 应重连一次')
  assert.equal(socketsSince(base)[0].open, false, 'Core 不在 ⇒ 这次连不上')

  // 失败之后退避到 4s
  t.mock.timers.tick(3999)
  await settle()
  assert.equal(socketsSince(base).length, 1, '第二次重连不许早于 4s')
  t.mock.timers.tick(1)
  await settle()
  assert.equal(socketsSince(base).length, 2, '4000ms 应第二次重连')

  // 再失败 ⇒ 退避到 8s
  t.mock.timers.tick(7999)
  await settle()
  assert.equal(socketsSince(base).length, 2, '第三次重连不许早于 8s')
  t.mock.timers.tick(1)
  await settle()
  assert.equal(socketsSince(base).length, 3, '8000ms 应第三次重连')

  // Core 回来了 ⇒ 下一次重试连上
  world.up = true
  t.mock.timers.tick(16_000)
  await settle()
  assert.equal(useAppStore.getState().connection, 'connected')

  // 连上之后退避计数复位：再掉线，等待又是 2s 起
  const base2 = wakeupSockets().length
  wakeupSockets()[base2 - 1].serverClose(1006)
  await settle()
  t.mock.timers.tick(1999)
  await settle()
  assert.equal(socketsSince(base2).length, 0)
  t.mock.timers.tick(1)
  await settle()
  assert.equal(socketsSince(base2).length, 1, '连上过之后退避应从 2s 重新起算')
})

test('②③ 断线窗口里的事件，恢复后从持久化游标补上：不丢、不重、顺序对', async (t: TestContext) => {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  resetWorld()
  world.events.push(messageEvent('e1', 'm1', '断线前', PEER, '阿元'))
  await connectAndSettle()
  assert.equal(fakeStorage.getItem(CURSOR_KEY), 'c1.he.e1')

  // 掉线；断线窗口里世界侧产生两条（另一条路：Agent 回话 / 别的会话）
  wakeupSockets()[0].serverClose(1006)
  await settle()
  world.events.push(messageEvent('e2', 'm2', '窗口里第一条', PEER, '阿元'))
  world.events.push(messageEvent('e3', 'm3', '窗口里第二条', ME, '我'))
  // 窗口里游标不许动（动了就是丢事件）
  assert.equal(fakeStorage.getItem(CURSOR_KEY), 'c1.he.e1')

  // 退避到点 ⇒ 重连 ⇒ 从**盘上的游标**补拉
  t.mock.timers.tick(2000)
  await settle()
  assert.deepEqual(messages(), ['断线前', '窗口里第一条', '窗口里第二条'], '不丢、顺序对')
  assert.equal(new Set(messages()).size, messages().length, '不重')
  assert.equal(fakeStorage.getItem(CURSOR_KEY), 'c1.he.e3')
  assert.equal(useAppStore.getState().connection, 'connected')

  // 同一位置重拉稳定：再唤醒一次 ⇒ 空页 ⇒ 一个字都不动（§10.3）
  const sock = wakeupSockets()[wakeupSockets().length - 1]
  sock.serverFrame({ type: 'events.available' })
  await settle()
  assert.deepEqual(messages(), ['断线前', '窗口里第一条', '窗口里第二条'])
  assert.equal(fakeStorage.getItem(CURSOR_KEY), 'c1.he.e3')
})

test('④a Core 不在时重连失败会继续重试；Core 回来 + 有窗口事件 ⇒ 补上', async (t: TestContext) => {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  resetWorld()
  world.events.push(messageEvent('e1', 'm1', '断线前', PEER, '阿元'))
  await connectAndSettle()

  wakeupSockets()[0].serverClose(1006)
  await settle()
  world.up = false // Core 停机
  const base = wakeupSockets().length

  t.mock.timers.tick(2000)
  await settle()
  assert.equal(socketsSince(base).length, 1, '第一次重试要发生')
  assert.equal(socketsSince(base)[0].open, false, 'Core 不在 ⇒ 连不上')
  assert.equal(useAppStore.getState().connection, 'reconnecting')
  t.mock.timers.tick(4000)
  await settle()
  assert.equal(socketsSince(base).length, 2, '失败之后还要重试（4s 档）')
  // 连不上期间游标没动、事件没丢
  assert.equal(fakeStorage.getItem(CURSOR_KEY), 'c1.he.e1')

  // Core 回来了，并且耐久账本里多了断线窗口里产生的事件
  world.up = true
  world.events.push(messageEvent('e2', 'm2', '窗口里', PEER, '阿元'))
  t.mock.timers.tick(8000)
  await settle()
  assert.deepEqual(messages(), ['断线前', '窗口里'])
  assert.equal(fakeStorage.getItem(CURSOR_KEY), 'c1.he.e2')
  assert.equal(useAppStore.getState().connection, 'connected')
})

test('④b 补拉请求失败（Core 还没完全起来）⇒ 必须重试，不许把事件搁丢', async (t: TestContext) => {
  t.mock.timers.enable({ apis: ['setTimeout'] })
  resetWorld()
  world.events.push(messageEvent('e1', 'm1', '断线前', PEER, '阿元'))
  await connectAndSettle()

  wakeupSockets()[0].serverClose(1006)
  await settle()
  world.eventsDown = true // WS 已能连上，但补拉端点还没就绪（502）
  world.events.push(messageEvent('e2', 'm2', '窗口里', PEER, '阿元'))
  t.mock.timers.tick(2000)
  await settle()
  // 补拉全失败：事件还没上屏、游标没动（不丢的前提）
  assert.deepEqual(messages(), ['断线前'])
  assert.equal(fakeStorage.getItem(CURSOR_KEY), 'c1.he.e1')

  // Core 起来了；此后**没有任何新的唤醒/事件** —— 恢复必须自己收敛（§10：
  // 没有新事件就没有下一次唤醒，一次瞬时失败不能把窗口里的事件永久搁下）。
  world.eventsDown = false
  t.mock.timers.tick(30_000)
  await settle(6)
  assert.deepEqual(messages(), ['断线前', '窗口里'], '补拉失败后必须重试，直到补上')
  assert.equal(fakeStorage.getItem(CURSOR_KEY), 'c1.he.e2')
})

test('★ 两条流各走各的：通话流上的帧真的落进 store，而唤醒那条链一个字都不动', async (t: TestContext) => {
  // 2026-10-09 接线通话时立：`/v1/call/stream` 与唤醒流**是两条独立流**
  // （契约 §4.1：独立 notifier、不污染 `events.available` 语义）。这条用例量两件事：
  // ① 真的是两条连接；② 通话的帧只驱动通话那一格（游标/消息一个字都不动）。
  t.mock.timers.enable({ apis: ['setTimeout'] })
  resetWorld()
  world.events.push(messageEvent('e1', 'm1', '第一条', PEER, '阿元'))
  await connectAndSettle()

  assert.equal(wakeupSockets().length, 1)
  const callSockets = FakeSocket.made.filter((s) => s.url.includes('/v1/call/stream'))
  assert.equal(callSockets.length, 1, '通话是**另一条**流，不是搭在唤醒流上')
  assert.equal(useAppStore.getState().call, null, '一开始没有通话')

  const callView = {
    call_id: 'c-1',
    room_id: 'r-1',
    caller_id: PEER,
    caller_display: '阿元',
    callee_id: ME,
    callee_display: '我',
    media: 'audio',
    state: 'ringing',
    ended_reason: null,
    created_at: '2026-10-09T00:00:00Z',
    accepted_at: null,
    ended_at: null,
  }
  callSockets[0].serverFrame({ type: 'call.ring', call: callView })
  await settle()
  assert.equal(useAppStore.getState().call?.call_id, 'c-1', '来电要落进 store')
  assert.equal(useAppStore.getState().callRole, 'callee')
  assert.equal(fakeStorage.getItem(CURSOR_KEY), 'c1.he.e1', '通话流不该让事件游标动')
  assert.deepEqual(messages(), ['第一条'], '通话不是消息：时间线一个字都不该变')

  // 读不懂的帧（这里 `call` 那一格不合法）⇒ 丢掉：状态**不许**被它改。
  callSockets[0].serverFrame({ type: 'call.ring' })
  callSockets[0].serverFrame({ type: 'call.video.upgraded', call: callView })
  await settle()
  assert.equal(useAppStore.getState().call?.call_id, 'c-1')

  // 对方挂了 ⇒ 收尾：`call` 清空，但"对方挂断了"那句话留下。
  callSockets[0].serverFrame({
    type: 'call.state',
    call: { ...callView, state: 'ended', ended_reason: 'hangup_caller' },
  })
  await settle()
  assert.equal(useAppStore.getState().call, null)
  assert.equal(useAppStore.getState().callNotice, '对方挂断了')
})

test.after(() => {
  useAppStore.getState().disconnect()
})
