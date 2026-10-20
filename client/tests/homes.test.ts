// 家条目与家族请求分流（家口远程访问刀）的纯逻辑测试。
//
// 四组判据：
//  ① **条目解析**：存着的记录 → 家条目（缺字段按默认值收、"读不懂"丢弃，绝不抛）；
//     本机那条**预置**（钥匙必空、id 固定）；
//  ② **分流构造**：本机 → 同源相对路径（与从前一字不差）；远程 → 绝对地址 + Bearer 头。
//     ⚠ 两条路的**路径形状不一样**是事实（代转改名 vs 直连用口子自己的路由名）；
//  ③ **错误分类**：钥匙不对 / 连不上 / 证书（+读不懂）各自成档、各有固定人话 ——
//     不许合并成一句"连接失败"；
//  ④ **认领**：按 `agent.agentId` 精确匹配（claimed / unknown-id / elsewhere / unreachable 四态）。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import {
  DEFAULT_HOME_PORTS,
  LOCAL_HOME_ID,
  claimEntry,
  claimedHostOf,
  classifyHomeFailure,
  configRequestUrl,
  homeAddressError,
  homeEntryDraftErrors,
  homeFailureText,
  homePortError,
  homeRequestHeaders,
  HomeRequestError,
  loadHomeEntries,
  localHomeEntry,
  mindEntryFor,
  mindReadoutFor,
  nextRemoteId,
  normalizeHomeAddress,
  parseStoredEntry,
  resolveAgentHome,
  saveHomeEntry,
  statusRequestUrl,
  type HomeEntry,
  type HomeReading,
} from '../src/api/homes.ts'
import { fetchHomeStatus } from '../src/api/homeStatus.ts'
import { HomeConfigError, HOME_CONFIG_ERROR_CODES, homeConfigErrorText } from '../src/api/homeConfig.ts'
import type { HomeStatusView } from '../src/api/homeStatus.ts'

/** 一条远程家（形状就是存进 IndexedDB 的样子）。 */
const REMOTE: HomeEntry = {
  id: 'home-1',
  name: '书房那台',
  baseUrl: 'https://192.168.1.20',
  statusPort: 5190,
  configPort: 5191,
  key: 'the-home-key',
  local: false,
}

// ── 一、条目解析与预置 ─────────────────────────────────────────────────

test('本机那条是**预置**：id 固定、钥匙为空、端口对带默认值', () => {
  const local = localHomeEntry()
  assert.equal(local.id, LOCAL_HOME_ID)
  assert.equal(local.local, true)
  assert.equal(local.key, '', '本机 = 同源代转 ⇒ 钥匙为空')
  assert.equal(local.baseUrl, '')
  assert.equal(local.statusPort, DEFAULT_HOME_PORTS.status)
  assert.equal(local.configPort, DEFAULT_HOME_PORTS.config)
})

test('解析存着的记录：缺字段按默认值收，端口缺就兜 5190/5191', () => {
  const parsed = parseStoredEntry({ id: 'home-2', baseUrl: 'https://10.0.0.7', key: 'k' })
  assert.ok(parsed)
  assert.equal(parsed.id, 'home-2')
  assert.equal(parsed.name, '未命名')
  assert.equal(parsed.statusPort, DEFAULT_HOME_PORTS.status)
  assert.equal(parsed.configPort, DEFAULT_HOME_PORTS.config)
  assert.equal(parsed.local, false)
  // 形状根本不对（不是对象 / 没有 id）⇒ 丢弃这一条（不猜）。
  assert.equal(parseStoredEntry(null), null)
  assert.equal(parseStoredEntry('nope'), null)
  assert.equal(parseStoredEntry({}), null)
})

test('新条目的 id 单调不撞（`home-N` 取最大 + 1）', () => {
  const local = localHomeEntry()
  assert.equal(nextRemoteId([local]), 'home-1')
  assert.equal(nextRemoteId([local, REMOTE, { ...REMOTE, id: 'home-3' }]), 'home-4')
  // 不认识的 id 形状不参与计数（认不出就不猜）。
  assert.equal(nextRemoteId([local, { ...REMOTE, id: '书房' }]), 'home-1')
})

test('地址归一化：没写 scheme 补 https、去尾斜杠（远程的家只开在 TLS 上）', () => {
  assert.equal(normalizeHomeAddress(' 192.168.1.20 '), 'https://192.168.1.20')
  assert.equal(normalizeHomeAddress('https://a.example/'), 'https://a.example')
  assert.equal(normalizeHomeAddress('http://a.example'), 'http://a.example', '写了的照写（校验再拦）')
  assert.equal(normalizeHomeAddress(''), '')
  assert.equal(homeAddressError(''), '机器地址不能为空')
  assert.equal(homeAddressError('192.168.1.20'), null, '裸主机名归一后是合法 https 地址')
  assert.match(homeAddressError('http://a.example') ?? '', /https/)
  // 读不懂的地址（URL 解析不了）：空格进不了主机名 ⇒ 明确报错，不猜。
  assert.match(homeAddressError('https://a b') ?? '', /读不懂/)
})

test('端口校验：留空合法（用默认）、越界与垃圾各自报一句', () => {
  assert.equal(homePortError(''), null)
  assert.equal(homePortError('5190'), null)
  assert.equal(homePortError('0'), '端口得在 1–65535 之间')
  assert.equal(homePortError('70000'), '端口得在 1–65535 之间')
  assert.equal(homePortError('abc'), '端口得是 1–65535 的数字')
})

test('添加表单的校验：地址 / 钥匙 / 两个端口各说各的（一处收口）', () => {
  const errors = homeEntryDraftErrors({
    name: 'x',
    baseUrl: 'http://a.example',
    statusPort: '0',
    configPort: 'abc',
    key: '   ',
  })
  assert.ok(errors.baseUrl, 'http 地址要被拦（远程必须 https）')
  assert.ok(errors.key, '钥匙必填')
  assert.ok(errors.statusPort)
  assert.ok(errors.configPort)
  assert.deepEqual(
    homeEntryDraftErrors({ name: 'x', baseUrl: '192.168.1.20', statusPort: '', configPort: '', key: 'k' }),
    {},
  )
})

test('读条目：没有本地存储（node 里没有 indexedDB）⇒ 抛出（由调用方如实说，不许静默）', async () => {
  await assert.rejects(() => loadHomeEntries())
  // 写一条也照抛（这条路只该在浏览器里走通）。
  await assert.rejects(() => saveHomeEntry(REMOTE))
})

// ── 二、分流构造：本机同源 vs 远程绝对 + 头 ────────────────────────────

test('本机：同源相对路径（与从前一字不差），不带任何头', () => {
  const local = localHomeEntry()
  assert.equal(statusRequestUrl(local, '/status'), '/home-status/status')
  assert.equal(statusRequestUrl(local, '/healthz'), '/home-status/healthz')
  assert.equal(configRequestUrl(local, '/home-config'), '/home-config')
  assert.equal(configRequestUrl(local, '/home-binding'), '/home-binding')
  assert.deepEqual(homeRequestHeaders(local), {})
})

test('远程：绝对地址 + 端口对，路径用**口子自己的路由名**，带 Bearer 家钥匙', () => {
  assert.equal(statusRequestUrl(REMOTE, '/status'), 'https://192.168.1.20:5190/status')
  assert.equal(statusRequestUrl(REMOTE, '/healthz'), 'https://192.168.1.20:5190/healthz')
  assert.equal(configRequestUrl(REMOTE, '/home-config'), 'https://192.168.1.20:5191/home-config')
  assert.equal(
    configRequestUrl(REMOTE, '/home-config/probe'),
    'https://192.168.1.20:5191/home-config/probe',
    '探测那条也在同一个配置门端面上',
  )
  assert.deepEqual(homeRequestHeaders(REMOTE), { authorization: 'Bearer the-home-key' })
  // 端口对可改（裁决：家条目带端口对，默认 5190/5191）。
  const moved = { ...REMOTE, statusPort: 6190, configPort: 6191 }
  assert.equal(statusRequestUrl(moved, '/status'), 'https://192.168.1.20:6190/status')
  assert.equal(configRequestUrl(moved, '/home-config'), 'https://192.168.1.20:6191/home-config')
})

/** 把 `fetch` 换成替身，记下这次请求的（url, headers）。 */
async function capture(call: () => Promise<unknown>) {
  const calls: { url: string; headers: Record<string, string> }[] = []
  const original = globalThis.fetch
  globalThis.fetch = (async (url: string | URL, init: RequestInit = {}) => {
    calls.push({ url: String(url), headers: (init.headers ?? {}) as Record<string, string> })
    return new Response(JSON.stringify(STATUS_OK), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })
  }) as typeof fetch
  try {
    await call()
  } finally {
    globalThis.fetch = original
  }
  return calls
}

const STATUS_OK = {
  readAt: '2026-09-24T04:00:00.000Z',
  stale: false,
  im: { health: 200 },
  agent: { state: 'loaded', agentId: 'agent-7', admission: 'ready' },
  lastWake: null,
}

test('fetchHomeStatus：本机打同源、不带头；远程打绝对地址、带家钥匙', async () => {
  const localCalls = await capture(() => fetchHomeStatus(localHomeEntry()))
  assert.equal(localCalls[0]!.url, '/home-status/status')
  assert.deepEqual(localCalls[0]!.headers, {})

  const remoteCalls = await capture(() => fetchHomeStatus(REMOTE))
  assert.equal(remoteCalls[0]!.url, 'https://192.168.1.20:5190/status')
  assert.equal(remoteCalls[0]!.headers['authorization'], 'Bearer the-home-key')
})

test('fetchHomeStatus：401 ⇒ HomeRequestError（unauthorized）；断网 ⇒ network', async () => {
  const original = globalThis.fetch
  try {
    globalThis.fetch = (async () => new Response('{}', { status: 401 })) as typeof fetch
    await assert.rejects(
      () => fetchHomeStatus(REMOTE),
      (error: unknown) => error instanceof HomeRequestError && error.failure === 'unauthorized',
    )
    globalThis.fetch = (async () => {
      throw new TypeError('failed to fetch')
    }) as typeof fetch
    await assert.rejects(
      () => fetchHomeStatus(REMOTE),
      (error: unknown) => error instanceof HomeRequestError && error.failure === 'network',
    )
  } finally {
    globalThis.fetch = original
  }
})

// ── 三、错误分类 → 固定人话（三档不许合并）─────────────────────────────

test('分类：钥匙不对 / 连不上 / 证书（https）各自成档', () => {
  const local = localHomeEntry()
  // 钥匙不对：远程错钥匙 / 本机口要钥匙（两句话说的事不一样）。
  assert.equal(classifyHomeFailure(REMOTE, new HomeRequestError('unauthorized')), 'key')
  assert.equal(classifyHomeFailure(local, new HomeRequestError('unauthorized')), 'local-key')
  // 断网：本机、远程 http、远程 https 三档。
  assert.equal(classifyHomeFailure(local, new TypeError('failed to fetch')), 'unreachable')
  assert.equal(
    classifyHomeFailure({ ...REMOTE, baseUrl: 'http://192.168.1.20' }, new TypeError('x')),
    'unreachable',
  )
  assert.equal(classifyHomeFailure(REMOTE, new TypeError('x')), 'tls')
  // 形状 / 别的。
  assert.equal(classifyHomeFailure(REMOTE, new HomeRequestError('shape')), 'shape')
})

test('固定人话：错钥匙那句逐字在（任务书里的原话），别的三类各说各的', () => {
  assert.equal(homeFailureText(REMOTE, new HomeRequestError('unauthorized')), '钥匙不对，去那台机器上重新拿一把')
  const tls = homeFailureText(REMOTE, new TypeError('x'))
  assert.match(tls, /证书/, 'https 连不上要把"证书"这个可能说出来（不假装分辨得出）')
  assert.match(homeFailureText(localHomeEntry(), new TypeError('x')), /本机服务/)
  assert.match(homeFailureText(REMOTE, new HomeRequestError('shape')), /读不懂/)
  // 模型配置门的拒绝（封闭表那句话）照原样用它的人话 —— 那是表里给的，不从服务端回显。
  assert.match(homeFailureText(REMOTE, new HomeConfigError('entry_bound')), /正在被这个 Agent 使用/)
})

test('★ http 那一档自己有人话 —— 不许把开发向英文漏给用户（2026-09-25 实测踩过）', () => {
  // 实测现场：本机服务没起（口子回了 502）时，执行端那一行的处境写着
  // `home request failed: http (502)` —— 那是 `HomeRequestError` 的**开发向 message**。
  // 根因是这一档从前落进 `other`，而 `other` 取 `error.message`。
  const err = new HomeRequestError('http', 502)
  assert.equal(classifyHomeFailure(localHomeEntry(), err), 'http', '502 必须自成一档，不落 other')
  const text = homeFailureText(localHomeEntry(), err)
  assert.ok(!/home request failed|http \(|502/.test(text), `用户可见文案里不许出现开发向英文或状态码：${text}`)
  // 与"连不上"必须分开说：服务答了 ≠ 服务没起。
  assert.notEqual(text, homeFailureText(localHomeEntry(), new TypeError('failed to fetch')))
})

test('封闭表：家钥匙那个码两侧对齐，且有自己的固定人话', () => {
  assert.ok(HOME_CONFIG_ERROR_CODES.includes('unauthorized'), '客户端解码端必须收下这个码')
  assert.equal(homeConfigErrorText('unauthorized'), '钥匙不对，去那台机器上重新拿一把')
})

// ── 四、认领（按 agent.agentId 精确匹配）────────────────────────────────

const view = (agentId?: string): HomeStatusView => ({
  readAt: 'x',
  stale: false,
  im: { health: 200 },
  agent: { state: 'loaded', ...(agentId === undefined ? {} : { agentId }), admission: 'ready' },
  lastWake: null,
})

const readingOf = (entry: HomeEntry, agentId?: string, failure: string | null = null): HomeReading<HomeStatusView> => ({
  entry,
  view: failure ? null : view(agentId),
  failure,
})

test('认领：读数的 id 与世界的 Agent id 精确相等才算（没有 ⇒ 不猜）', () => {
  const readings = [readingOf(localHomeEntry(), 'agent-7')]
  assert.equal(claimEntry(readings, 'agent-7')?.entry.id, LOCAL_HOME_ID)
  assert.equal(claimEntry(readings, 'agent-9'), null)
  assert.equal(claimEntry(readings, ''), null, '不知道我是谁 ⇒ 不认领')
})

test('★ 第 2 支起：认领要看**住户清单**（从前只比顶层那一格 ⇒ 它们被判成"没有机器在跑"）', () => {
  // 现场（2026-10-08）：一个家住多支时，顶层那几格是**第一支**的（口子把第一支抬到顶层，
  // 为的是不破坏冻结前端）⇒ 只比顶层那一格的话，第 2、3 支的 Agent 主页会写
  // 「还没有机器在跑它」——**假话**（它们就在这台机器上跑着，只是各住一个环境根）。
  // 清单那一格（`residents[]`）**本来就在载荷里**，只是客户端契约从来没声明过它。
  const local = localHomeEntry()
  const multi: HomeStatusView = {
    ...view('agent-1'),
    host: { state: 'running' },
    residents: [
      { agentId: 'agent-1', slot: 1, host: { state: 'running' } },
      { agentId: 'agent-7', slot: 2, host: { state: 'stopped' } },
    ],
  }
  const readings: HomeReading<HomeStatusView>[] = [{ entry: local, view: multi, failure: null }]

  // 第一支：认领靠顶层那一格 ⇒ 读数**就是它的**（与从前一字不变）。
  const first = resolveAgentHome(readings, 'agent-1')
  assert.equal(first.kind, 'claimed')
  assert.equal(first.kind === 'claimed' ? first.ownReadings : null, true)
  assert.equal(claimedHostOf(first, 'agent-1')?.state, 'running')

  // 第 2 支：认领靠**清单** ⇒ 住址对（不再说"没在跑"），但**读数不是它的**。
  const second = resolveAgentHome(readings, 'agent-7')
  assert.equal(second.kind, 'claimed', '第 2 支必须被认领 —— 不许再写成「还没有机器在跑它」')
  assert.equal(second.kind === 'claimed' ? second.ownReadings : null, false)
  assert.equal(second.kind === 'claimed' ? second.entry.id : '', LOCAL_HOME_ID)
  // ⚠ 最尖的一条：宿主那一格必须读**它自己那一项**的（`stopped`），
  //   **不许**退回顶层那一格（那是第一支的 `running`）—— 那正是张冠李戴。
  assert.equal(claimedHostOf(second, 'agent-7')?.state, 'stopped')

  // 名单里没有的 id 照旧不认领（不匹配不许说成匹配）。
  assert.equal(resolveAgentHome(readings, 'agent-9').kind, 'elsewhere')
})

test('裁决四态：claimed / unknown-id / elsewhere / unreachable', () => {  const local = localHomeEntry()
  const claimed = resolveAgentHome([readingOf(local, 'agent-7')], 'agent-7')
  assert.equal(claimed.kind, 'claimed')

  const remoteClaim = resolveAgentHome([readingOf(local, 'other-agent'), readingOf(REMOTE, 'agent-7')], 'agent-7')
  assert.equal(remoteClaim.kind, 'claimed')
  assert.equal(remoteClaim.kind === 'claimed' ? remoteClaim.entry.name : '', REMOTE.name, '认领到的是那台远程机器')

  const unknownId = resolveAgentHome([readingOf(local)], 'agent-7')
  assert.equal(unknownId.kind, 'unknown-id', '读数没报 id ⇒ 照旧显示，绝不冒充"未接入"')

  const elsewhere = resolveAgentHome([readingOf(local, 'other-agent')], 'agent-7')
  assert.equal(elsewhere.kind, 'elsewhere')

  const unreachable = resolveAgentHome([readingOf(local, undefined, '连不上本机服务')], 'agent-7')
  assert.equal(unreachable.kind, 'unreachable')
  assert.equal(unreachable.kind === 'unreachable' ? unreachable.failure : '', '连不上本机服务')
})

test('模型那一面问谁：认领到就跟认领；一份没读到且**只有一个家**时用它；否则不猜', () => {
  const local = localHomeEntry()
  const claimed = resolveAgentHome([readingOf(local, 'agent-7')], 'agent-7')
  assert.equal(mindEntryFor(claimed, [local])?.id, LOCAL_HOME_ID)

  const unreachable = resolveAgentHome([readingOf(local, undefined, 'x')], 'agent-7')
  assert.equal(mindEntryFor(unreachable, [local])?.id, LOCAL_HOME_ID, '只有一个家 ⇒ 没有歧义')
  assert.equal(mindEntryFor(unreachable, [local, REMOTE]), null, '多个家读不回来 ⇒ 不拿某台顶上')

  const elsewhere = resolveAgentHome([readingOf(local, 'other')], 'agent-7')
  assert.equal(mindEntryFor(elsewhere, [local]), null, '谁都不是 ⇒ 这一面不该问本机')
})

// ── 「Agent 的模型」这一面的读数：问哪一条 + 那条读到了什么（一次问完）──────────────
//
// 为什么要有这一段：这条链原先在**两个调用点各拼一遍**，而两处的手不一样 ——
// Agent 的主页用 `mindEntryFor`（带"只配了一个家"那一手），「我的 Agent」列表行自己拼了一个
// 少了那一手的局部判据。同一个输入下两处说不同的话，而门禁全绿。下面第一条就是那个输入。

/** 配置读数的最小形状（本函数对 C 是泛型，这里只要能被逐字段比出来即可）。 */
interface FakeConfig {
  model: string
}

const conf = (entry: HomeEntry, view: FakeConfig | null, failure: string | null = null) => ({
  entry,
  config: { view, failure },
})

test('★ 一份都没读回来 + 只配了一个家 ⇒ 模型那一面用那一条（列表行与主页曾经在这里说不同的话）', () => {
  const local = localHomeEntry()
  const unreachable = resolveAgentHome([readingOf(local, undefined, '连不上本机服务')], 'agent-7')
  const readout = mindReadoutFor<HomeStatusView, FakeConfig>({
    resolution: unreachable,
    entries: [local],
    readings: [conf(local, { model: 'agent-model' })],
  })
  assert.equal(readout?.entry.id, LOCAL_HOME_ID, '只配了一个家 ⇒ 没有歧义')
  assert.deepEqual(readout?.view, { model: 'agent-model' }, '那条机器的读数照旧读得到')
  assert.equal(readout?.failure, null)
})

test('模型面：认领到就跟认领（本机与远程各一例）', () => {
  const local = localHomeEntry()
  const claimedLocal = resolveAgentHome([readingOf(local, 'agent-7')], 'agent-7')
  assert.equal(
    mindReadoutFor<HomeStatusView, FakeConfig>({
      resolution: claimedLocal,
      entries: [local],
      readings: [conf(local, { model: 'local-model' })],
    })?.view?.model,
    'local-model',
  )

  const claimedRemote = resolveAgentHome([readingOf(local, 'other'), readingOf(REMOTE, 'agent-7')], 'agent-7')
  const readout = mindReadoutFor<HomeStatusView, FakeConfig>({
    resolution: claimedRemote,
    entries: [local, REMOTE],
    readings: [conf(local, { model: 'local-model' }), conf(REMOTE, { model: 'remote-model' })],
  })
  assert.equal(readout?.entry.id, REMOTE.id, '认领到的是那台远程机器')
  assert.equal(readout?.view?.model, 'remote-model', '读的是**Agent 那台**的配置，不是本机的')
})

test('模型面：读数缺了那一条 / 读取失败 ⇒ 照实说，不抛也不猜', () => {
  const local = localHomeEntry()
  const claimed = resolveAgentHome([readingOf(local, 'agent-7')], 'agent-7')
  // 条目在、但这一轮读数里没有它那一份 ⇒ 两个字段都为 null（不是"没配"）。
  assert.deepEqual(
    mindReadoutFor<HomeStatusView, FakeConfig>({ resolution: claimed, entries: [local], readings: [] }),
    { entry: local, view: null, failure: null },
  )
  // 读取失败：人话原样带出去（分类与会话成型都在上层一处）。
  const failed = mindReadoutFor<HomeStatusView, FakeConfig>({
    resolution: claimed,
    entries: [local],
    readings: [conf(local, null, '连不上本机服务')],
  })
  assert.equal(failed?.view, null)
  assert.equal(failed?.failure, '连不上本机服务')
})

test('模型面：谁都不是 / 多个家读不回来 ⇒ `null`（这一面不知道该问谁，不拿某台顶上）', () => {
  const local = localHomeEntry()
  const elsewhere = resolveAgentHome([readingOf(local, 'other')], 'agent-7')
  assert.equal(
    mindReadoutFor<HomeStatusView, FakeConfig>({
      resolution: elsewhere,
      entries: [local],
      readings: [conf(local, { model: 'x' })],
    }),
    null,
  )

  const unreachable = resolveAgentHome([readingOf(local, undefined, 'x')], 'agent-7')
  assert.equal(
    mindReadoutFor<HomeStatusView, FakeConfig>({
      resolution: unreachable,
      entries: [local, REMOTE],
      readings: [conf(local, { model: 'x' }), conf(REMOTE, { model: 'y' })],
    }),
    null,
    '多个家读不回来 ⇒ 不拿某一台顶上去',
  )
})