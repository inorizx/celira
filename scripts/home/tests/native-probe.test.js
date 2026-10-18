'use strict'
// **原生目录层**（2026-09-24）与它的机械判据。
//
// 这一层要钉住的六条：
//  1. **表驱动，加一家 = 加一条**：表里没有的家 ⇒ 走兼容端点，**行为一字不变**；
//  2. **绝不抛、绝不半读半猜**：原生端点是「锦上添花」，探不动 / 形状认不出 ⇒ `null` 退回兼容端点，
//     绝不因为「加料」没拿到就砸了整个试连；
//  3. **能力码认不出就不标**（宁可不标，也不给一个错的标签）；窗口**只收正整数**；
//  4. **翻页有上限**：上游把 total 报错也不许无限翻（MAX_PAGES）；
//  5. **候选是原生的、封顶 `NATIVE_LIST_LIMIT`**，且只在原生成功时才多出 `modelKinds` 一格；
//  6. **原生探到了就不问兼容端点**（省一枪），探不到就**一字不变**退回去。
//
// 运行： node --test "scripts/home/tests/*.test.js"

const test = require('node:test')
const assert = require('node:assert/strict')
const http = require('node:http')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const {
  createConfigServer,
  probeNative,
  originOf,
  kindsFromDashscope,
  NATIVE_PROBES,
  NATIVE_LIST_LIMIT,
  PROBE_PATH,
  modelsUrl,
  modelSources,
} = require('../config-server')
const { createWindowTable } = require('../model-windows')

const NO_WINDOW = { contextWindow: null, windowField: null }
const NO_CATALOG = { ...NO_WINDOW, models: [] }

let cases = 0
/** `await fn()` 而不是 `fn()`：漏掉 await 的用例断言会被静默跳过、还假装通过（那正是「测试空跑」）。 */
function nativeCase(name, fn) {
  test(name, async () => {
    await fn()
    cases += 1
  })
}

function scratch(label) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `celira-native-gate-${label}-`))
}

/** 清单文件路径（判「探一次有没有落盘」时用）。 */
function listFile(homeDir) {
  return modelSources.listPath(homeDir)
}

// ---------------------------------------------------------------------------
// 假网络：把「打了哪些 URL、打了几次、带什么头」变成可判据
// ---------------------------------------------------------------------------

/**
 * 一个假 fetch。`handler(nth, url, init)` 返回 `{ status, body }`；**抛** = 网络炸了。
 * 想断言「一次都不该打」时，给它一个会抛的 handler。
 */
function fakeFetch(handler) {
  const calls = []
  const impl = async (url, init) => {
    calls.push({ url: String(url), init })
    const res = await handler(calls.length, String(url), init ?? {})
    const status = res.status ?? 200
    // ★ 替身也得把 `ok` 算出来（`probeNative` 正是靠它判 2xx）—— 漏了会让每条用例静默走"没探到"。
    return { ok: status >= 200 && status < 300, status, json: async () => res.body }
  }
  return { impl, calls }
}

/** 一份百炼形状的原生响应。 */
function page(rows, total) {
  return { status: 200, body: { output: { models: rows, total } } }
}

/** 一个百炼条目（**逐字照抄实测字段**：`model` / `capabilities` / `model_info.context_window`）。 */
function row(model, overrides = {}) {
  return { model, capabilities: ['TG'], model_info: { context_window: 1000000 }, ...overrides }
}

/** 造 n 条**互不重名**的模型。 */
function rows(n, prefix = 'm') {
  return Array.from({ length: n }, (_, i) => row(`${prefix}-${i}`))
}

const BASE = 'https://dashscope.example.invalid'
const KEY = 'sk-native-secret'

/** 替身：`n` 条候选 + 按名给的品类/窗口；探不到时返回 `null`。 */
function nativeOk(overrides = {}) {
  return {
    provider: 'dashscope',
    url: `${BASE}/api/v1/models`,
    status: 200,
    ok: true,
    models: ['qwen3.8-max', 'qwen-vl-max', 'qwen-audio'],
    modelMeta: {
      'qwen3.8-max': { kinds: ['chat'], window: 1000000 },
      'qwen-vl-max': { kinds: ['chat'] },
      'qwen-audio': { window: 32768 }, // 只有窗口、没有品类 ⇒ 不该进 `modelKinds`
    },
    total: 3,
    ...overrides,
  }
}

/** 兜底表夹具：只放百炼一条，用来判「原生没给窗口时会不会回落社区表」。 */
const WINDOW_TABLE = createWindowTable({ dashscope: { 'qwen-vl-max': 262144 } })

const ENDPOINT = 'https://dashscope.example.invalid/compatible-mode/v1/chat/completions'

// ---------------------------------------------------------------------------
// §1 originOf：读不懂就 null（调用方退回兼容端点，绝不拿一句垃圾去拼 URL）
// ---------------------------------------------------------------------------

nativeCase('originOf：只取 origin，读不懂的一律 null（不猜、不抛）', () => {
  assert.equal(originOf('https://dashscope.example.invalid/compatible-mode/v1/chat/completions'), BASE)
  assert.equal(originOf('  https://a.example/x?y=1  '), 'https://a.example')
  assert.equal(originOf('http://127.0.0.1:8080/v1'), 'http://127.0.0.1:8080')
  // 注：`ftp://` 这类**能被 URL 解析**的串会给出它自己的 origin —— 但端到不了那里：
  // 两条写路的端点都先过 `new URL()` + 只认 http/https 的形状预检 ⇒ 这里的"读不懂"就是"解析不了"。
  for (const bad of ['', 'not-a-url', '/relative/path', '//no-scheme', null, undefined, 42, {}]) {
    assert.equal(originOf(bad), null, `${String(bad)} 读不出 origin ⇒ null`)
  }
})

// ---------------------------------------------------------------------------
// §2 能力 → 类别：认不出的码**不产出**
// ---------------------------------------------------------------------------

nativeCase('能力→类别：认得的照直译，认不出的**不产出**（宁可不标，也不给错标签）', () => {
  // ★ VU（视觉理解）与 Reasoning 都归 chat —— 它们**能对话**；只有 VG 是真·别的东西。
  assert.deepEqual(kindsFromDashscope(['TG']), ['chat'])
  assert.deepEqual(kindsFromDashscope(['VU']), ['chat'])
  assert.deepEqual(kindsFromDashscope(['Reasoning']), ['chat'])
  assert.deepEqual(kindsFromDashscope(['VG']), ['video'])
  // 多码**去重**（TG 与 VU 都是 chat ⇒ 只留一个）
  assert.deepEqual(kindsFromDashscope(['TG', 'VU', 'Reasoning']), ['chat'])
  assert.deepEqual(kindsFromDashscope(['TG', 'VG']), ['chat', 'video'])
  // 认不出的码：自己不产出，但**不连坐**认得出的那些。
  assert.equal(kindsFromDashscope(['XX']), null)
  assert.equal(kindsFromDashscope(['TG', 'XX']).length, 1)
  assert.deepEqual(kindsFromDashscope(['TG', 'XX']), ['chat'])
  // 不是数组 ⇒ null（不半读半猜）
  for (const bad of [null, undefined, 'TG', 42, {}, []]) {
    assert.equal(kindsFromDashscope(bad), null, `${String(bad)} 不是能力数组 ⇒ null`)
  }
})

// ---------------------------------------------------------------------------
// §3 表的形状守卫（将来加行时守住「原生」二字）
// ---------------------------------------------------------------------------

nativeCase('★ 表的形状守卫：每行都得是「原生路径 + 每页条数」，不许把兼容端点抄进来', () => {
  const entries = Object.entries(NATIVE_PROBES)
  assert.ok(entries.length > 0, '表里至少得有一家（否则这一层是死的，判据也就没看头）')
  for (const [id, spec] of entries) {
    assert.ok(typeof id === 'string' && id.length > 0)
    assert.equal(typeof spec.path, 'string', `${id}.path 必须是字符串`)
    assert.ok(spec.path.startsWith('/'), `${id}.path 必须以 / 开头`)
    // 抄兼容端点 = 这一层白立（兼容端点本来就走 `probe`），而且会**重复打一枪**。
    assert.notEqual(spec.path, '/v1/models', `${id} 抄了兼容端点就没意义`)
    assert.notEqual(spec.path, '/models')
    assert.ok(Number.isInteger(spec.pageSize) && spec.pageSize > 0, `${id}.pageSize 必须是正整数`)
  }
  assert.ok(Number.isInteger(NATIVE_LIST_LIMIT) && NATIVE_LIST_LIMIT > 0)
})

// ---------------------------------------------------------------------------
// §4 probeNative 本体
// ---------------------------------------------------------------------------

nativeCase('表里没有的家 ⇒ null，且**一次网络都不打**（行为一字不变）', async () => {
  const net = fakeFetch(() => {
    throw new Error('表外的一家不该去探原生端点')
  })
  const out = await probeNative('deepseek', BASE, KEY, { fetch: net.impl })
  assert.equal(out, null)
  assert.equal(net.calls.length, 0)
})

nativeCase('缺 baseUrl / 缺钥匙 ⇒ null，也不打网络（不拿空凭据去撞门）', async () => {
  const net = fakeFetch(() => {
    throw new Error('参数不全时不该打网络')
  })
  assert.equal(await probeNative('dashscope', null, KEY, { fetch: net.impl }), null)
  assert.equal(await probeNative('dashscope', '', KEY, { fetch: net.impl }), null)
  assert.equal(await probeNative('dashscope', BASE, '', { fetch: net.impl }), null)
  assert.equal(await probeNative('dashscope', BASE, null, { fetch: net.impl }), null)
  assert.equal(await probeNative('', BASE, KEY, { fetch: net.impl }), null)
  assert.equal(net.calls.length, 0)
})

nativeCase('★ 形状认不出 ⇒ null：绝不半读半猜（响应里有别的东西也不认）', async () => {
  for (const body of [
    {}, // 空的
    { output: {} }, // 有 output、没有 models
    { output: { models: 'not-an-array' } }, // models 不是数组
    { output: { models: [null, 42, 'x'] } }, // 全是垃圾条目 ⇒ 一条都认不出
    { models: [row('m-0')] }, // 少一层 output：形状不对就是不对
    null,
  ]) {
    const net = fakeFetch(() => ({ status: 200, body }))
    const out = await probeNative('dashscope', BASE, KEY, { fetch: net.impl })
    assert.equal(out, null, `${JSON.stringify(body)} 认不出 ⇒ null（不猜）`)
    assert.equal(net.calls.length, 1, '每一档都要真去问一次（不是靠短路过的判据）')
  }
})

nativeCase('★ 非 2xx / 网络炸 ⇒ null 且**不抛**（原生层坏了最多损失「加料」）', async () => {
  for (const [label, handler] of [
    ['401', () => ({ status: 401, body: { code: 10110400 } })],
    ['403', () => ({ status: 403, body: {} })],
    ['500', () => ({ status: 500, body: {} })],
    ['fetch 抛', () => {
      throw new Error('ECONNRESET')
    }],
    ['json 不是对象', () => ({ status: 200, body: 'plain text' })],
  ]) {
    const net = fakeFetch(handler)
    const out = await probeNative('dashscope', BASE, KEY, { fetch: net.impl })
    assert.equal(out, null, `${label} ⇒ null（不抛）`)
  }
})

nativeCase('单页：候选保序去重，窗口/品类按名入档', async () => {
  const net = fakeFetch(() =>
    page([row('a-1'), row('a-2'), row('a-1'), row('a-3', { capabilities: ['VG'], model_info: { context_window: 4096 } })], 4),
  )
  const out = await probeNative('dashscope', BASE, KEY, { fetch: net.impl })
  assert.deepEqual(out.models, ['a-1', 'a-2', 'a-3'], '重名只留一次、顺序照对方给的')
  assert.deepEqual(out.modelMeta['a-3'], { kinds: ['video'], window: 4096 })
  assert.equal(out.total, 4)
  assert.equal(out.ok, true)
  assert.equal(out.url, `${BASE}/api/v1/models`)
  // 请求形状：GET + Bearer + 原生路径 + 分页参数
  const { url, init } = net.calls[0]
  assert.ok(url.startsWith(`${BASE}/api/v1/models?`), url)
  assert.match(url, /page_no=1/)
  assert.match(url, /page_size=100/)
  assert.equal(init.method, 'GET')
  assert.equal(init.headers.authorization, `Bearer ${KEY}`)
})

nativeCase('★ 窗口只收正整数：0 / 负 / 小数 / 字符串数字一律不收（不拿错的数去骗人）', async () => {
  const bad = [0, -1, 1.5, '131072', null, undefined, {}, [], Number.NaN]
  const net = fakeFetch(() =>
    page(
      bad.map((value, i) => row(`bad-${i}`, { model_info: { context_window: value } })).concat([
        row('good', { model_info: { context_window: 131072 } }),
        row('no-info', { model_info: null }),
      ]),
      bad.length + 2,
    ),
  )
  const out = await probeNative('dashscope', BASE, KEY, { fetch: net.impl })
  for (let i = 0; i < bad.length; i += 1) {
    assert.equal(out.modelMeta[`bad-${i}`].window, undefined, `${String(bad[i])} 不该被当成窗口`)
  }
  assert.equal(out.modelMeta.good.window, 131072)
  assert.equal(out.modelMeta['no-info'].window, undefined)
  assert.equal(out.models.includes('bad-0'), true, '窗口不合规不影响它进候选（两件事）')
})

nativeCase('条目没有 `model` / 是空串 ⇒ 跳过（不进候选、不进 meta）', async () => {
  const net = fakeFetch(() =>
    page([{ capabilities: ['TG'] }, { model: '   ' }, { model: 42 }, row('real')], 4),
  )
  const out = await probeNative('dashscope', BASE, KEY, { fetch: net.impl })
  assert.deepEqual(out.models, ['real'])
  assert.deepEqual(Object.keys(out.modelMeta), ['real'])
})

nativeCase('★ 翻页：满页就翻下一页，total 到了就停（不多打一枪）', async () => {
  // 第一页满 100 ⇒ 翻；第二页 50 条（不满页）⇒ 停。
  const net = fakeFetch((nth) => (nth === 1 ? page(rows(100, 'p1'), 150) : page(rows(50, 'p2'), 150)))
  const out = await probeNative('dashscope', BASE, KEY, { fetch: net.impl })
  assert.equal(net.calls.length, 2)
  assert.equal(out.models.length, 150)
  assert.ok(net.calls[1].url.includes('page_no=2'))
})

nativeCase('★ 翻页的第二道闸：rows 满页但 total 已够 ⇒ 停（不让上游的 total 拖着无限翻）', async () => {
  const net = fakeFetch(() => page(rows(100, 'p1'), 100))
  const out = await probeNative('dashscope', BASE, KEY, { fetch: net.impl })
  assert.equal(net.calls.length, 1, 'total=100 且已收满 ⇒ 打一次就够')
  assert.equal(out.models.length, 100)
})

nativeCase('★ 翻页上限：total 报错（超大）也不许无限翻 —— 封顶 MAX_PAGES=12', async () => {
  const net = fakeFetch((nth) => page(rows(100, `p${nth}`), 999999))
  const out = await probeNative('dashscope', BASE, KEY, { fetch: net.impl })
  assert.equal(net.calls.length, 12, '上限就是 12 页（不死循环、不越翻越多）')
  assert.equal(out.models.length, 1200)
  assert.ok(out.models.length < 999999)
})

nativeCase('空目录 ⇒ null（探到了但一条都没有 ⇒ 当没探到，退回兼容端点）', async () => {
  const net = fakeFetch(() => page([], 0))
  assert.equal(await probeNative('dashscope', BASE, KEY, { fetch: net.impl }), null)
})

nativeCase('★ 跨页重名只留一次（去重在**整份**上做，不是每页各做一次）', async () => {
  const net = fakeFetch((nth) => (nth === 1 ? page(rows(100, 'same'), 150) : page([...rows(50, 'same')], 150)))
  const out = await probeNative('dashscope', BASE, KEY, { fetch: net.impl })
  assert.equal(out.models.length, 100, '第二页那 50 条是第一页的前 50 个重名 ⇒ 不重复入列')
})

// ---------------------------------------------------------------------------
// §5 组合口：原生 ↔ 兼容端点（经真 server）
// ---------------------------------------------------------------------------

/** 起一个**真的** server，两个探针都可注入；跑完一定关掉。 */
async function withServer(fn, ctx = {}) {
  const probes = []
  const natives = []
  const injectedProbe = ctx.probe
  const injectedNative = ctx.probeNative
  // ★ 两个探针都在**这里**记账、再转交注入的替身 —— 否则"注入了替身"就等于"丢了计数"，
  //   「打了几枪」这条判据会静默变成 `0`，而 `0` 恰好等于"原生探到了"那一档的期望值（假绿）。
  const server = createConfigServer({
    ...ctx,
    probe: async (endpoint, credential, model) => {
      probes.push({ endpoint, credential, model })
      if (injectedProbe) return await injectedProbe(endpoint, credential, model)
      return { url: modelsUrl(endpoint), ok: true, status: 200, reason: null, ...NO_CATALOG }
    },
    // ★ 缺省返回 null ⇒ 与「表里没有这一家」等价（行为一字不变）。
    probeNative: async (provider, baseUrl, credential) => {
      natives.push({ provider, baseUrl, credential })
      return injectedNative ? await injectedNative(provider, baseUrl, credential) : null
    },
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const port = server.address().port
  try {
    await fn({ port, probes, natives })
  } finally {
    await new Promise((resolve) => server.close(resolve))
  }
}

function request(port, method, target, { body, headers } = {}) {
  return new Promise((resolve, reject) => {
    const payload = body === undefined ? null : Buffer.from(JSON.stringify(body), 'utf8')
    const requestHeaders = { 'content-type': 'application/json', ...headers }
    if (payload) requestHeaders['content-length'] = payload.length
    const req = http.request(
      { host: '127.0.0.1', port, method, path: target, headers: requestHeaders },
      (res) => {
        let text = ''
        res.setEncoding('utf8')
        res.on('data', (chunk) => {
          text += chunk
        })
        res.on('end', () => {
          let json = null
          try {
            json = JSON.parse(text)
          } catch {
            json = null
          }
          resolve({ status: res.statusCode, headers: res.headers, text, json })
        })
      },
    )
    req.on('error', reject)
    if (payload) req.write(payload)
    req.end()
  })
}

/** 写路径的合法体（`contextWindow: ''` = 留空 ⇒ 走「先试连」那条路）。 */
function saveBody(overrides = {}) {
  return {
    endpoint: ENDPOINT,
    apiKey: KEY,
    model: 'qwen3.8-max',
    contextWindow: '',
    provider: 'dashscope',
    ...overrides,
  }
}

nativeCase('★ 原生探到了 ⇒ 用它的候选与窗口，且**兼容端点一次都不打**（省一枪）', async () => {
  const root = scratch('native-hit')
  try {
    await withServer(
      async ({ port, probes, natives }) => {
        const res = await request(port, 'POST', '/home-config', { body: saveBody() })
        assert.equal(res.status, 200, res.text)
        // 「对方自己说的」赢过「社区说的」⇒ 来源照标 probed
        assert.equal(res.json.contextWindow, 1000000)
        assert.equal(res.json.windowSource, 'probed')
        assert.equal(res.json.probe.windowField, 'model_info.context_window', '字段名如实（不冒充兼容端点）')
        // 候选是**原生**那份（3 条），不是兼容端点的空数组
        assert.deepEqual(res.json.probe.models, ['qwen3.8-max', 'qwen-vl-max', 'qwen-audio'])
        // 品类：只有**有 kinds 的那两条**进来；只有窗口的 `qwen-audio` 不进。
        assert.deepEqual(res.json.probe.modelKinds, {
          'qwen3.8-max': ['chat'],
          'qwen-vl-max': ['chat'],
        })
        assert.equal(probes.length, 0, '原生拿到了还去问兼容端点 ⇒ 白打一枪')
        assert.equal(natives.length, 1)
        assert.equal(natives[0].provider, 'dashscope')
        assert.equal(natives[0].baseUrl, BASE, '原生端点按 endpoint 的 origin 拼，不是整条 URL')
        assert.equal(natives[0].credential, KEY)
      },
      {
        homeDir: root,
        windowTable: WINDOW_TABLE,
        probeNative: async () => nativeOk(),
      },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

nativeCase('★ 原生探到了但**没给这个模型的窗口** ⇒ 仍回落兜底表（对方没说 ⇒ 才轮到社区说）', async () => {
  const root = scratch('native-no-window')
  try {
    await withServer(
      async ({ port, probes }) => {
        const res = await request(port, 'POST', '/home-config', { body: saveBody({ model: 'qwen-vl-max' }) })
        assert.equal(res.status, 200, res.text)
        assert.equal(res.json.contextWindow, 262144, '表里那一条顶上来（原生只给了品类，没给窗口）')
        assert.equal(res.json.windowSource, 'table', '★ 来源要如实 —— 帮填 ≠ 权威值')
        // ★ 窗口来自表，但**候选仍是原生的**（两件事各自独立，别混成一个）
        assert.deepEqual(res.json.probe.models, ['qwen3.8-max', 'qwen-vl-max', 'qwen-audio'])
        assert.equal(probes.length, 0)
      },
      { homeDir: root, windowTable: WINDOW_TABLE, probeNative: async () => nativeOk() },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

nativeCase('★ 原生探不动 ⇒ **一字不变**退回兼容端点（回执里连 modelKinds 这一格都没有）', async () => {
  const root = scratch('native-miss')
  try {
    await withServer(
      async ({ port, probes, natives }) => {
        const res = await request(port, 'POST', '/home-config', {
          body: saveBody({ model: 'deepseek-flash', contextWindow: 131072 }),
        })
        assert.equal(res.status, 200, res.text)
        assert.equal(natives.length, 1, '表内有这一家 ⇒ 还是要问一句（不问就不知道它探不探得动）')
        assert.equal(probes.length, 1, '原生 null ⇒ 兼容端点必须照旧打')
        assert.equal(res.json.probe.url, modelsUrl(ENDPOINT), '读数就是兼容端点那一份')
        assert.equal('modelKinds' in res.json.probe, false, '★ 原生没成功 ⇒ 整格不产出（不给空对象假装成功）')
      },
      {
        homeDir: root,
        windowTable: WINDOW_TABLE,
        probeNative: async () => null,
      },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

nativeCase('★ 负对照：没给 provider ⇒ 原生层**一次都不问**（表外的家/自定义不该被硬套一个原生端点）', async () => {
  const root = scratch('no-provider')
  try {
    await withServer(
      async ({ port, probes, natives }) => {
        const res = await request(port, 'POST', '/home-config', {
          body: saveBody({ provider: undefined, contextWindow: 131072 }),
        })
        assert.equal(res.status, 200, res.text)
        assert.equal(natives.length, 0, '★ provider 是查找键，没有键就不查（这一条要是红了，说明有人在猜）')
        assert.equal(probes.length, 1)
      },
      { homeDir: root, probeNative: async () => nativeOk() },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

nativeCase('★ 只读探测：带 provider 走原生，一个字节都不落盘、响应里没有钥匙', async () => {
  const root = scratch('probe-readonly')
  try {
    await withServer(
      async ({ port, probes }) => {
        const res = await request(port, 'POST', PROBE_PATH, {
          body: { endpoint: ENDPOINT, api_key: KEY, model: 'qwen3.8-max', provider: 'dashscope' },
        })
        assert.equal(res.status, 200, res.text)
        assert.equal(res.json.probe.contextWindow, 1000000)
        assert.deepEqual(res.json.probe.models, ['qwen3.8-max', 'qwen-vl-max', 'qwen-audio'])
        assert.deepEqual(res.json.probe.modelKinds, { 'qwen3.8-max': ['chat'], 'qwen-vl-max': ['chat'] })
        assert.equal(probes.length, 0)
        // ★ 钥匙一个字节都不回浏览器（整份响应里搜）
        assert.equal(res.text.includes(KEY), false, '试连是读数，但钥匙不能跟着回执出去')
        // ★ 探测不写盘（清单文件根本不该被造出来）
        assert.equal(fs.existsSync(listFile(root)), false, '探测是只读的 —— 落盘就是违约')
      },
      { homeDir: root, probeNative: async () => nativeOk() },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

nativeCase('★ 候选封顶：原生给 700 条 ⇒ 只回 `NATIVE_LIST_LIMIT` 条（且短列表不被截断）', async () => {
  const root = scratch('native-cap')
  try {
    await withServer(
      async ({ port }) => {
        const big = await request(port, 'POST', PROBE_PATH, {
          body: { endpoint: ENDPOINT, api_key: KEY, provider: 'dashscope' },
        })
        assert.equal(big.status, 200, big.text)
        assert.equal(big.json.probe.models.length, NATIVE_LIST_LIMIT)
        assert.ok(NATIVE_LIST_LIMIT < 700, '封顶要真的切掉一截（否则这条判据是装饰）')
        assert.equal(
          'modelKinds' in big.json.probe,
          false,
          '一条品类都没有 ⇒ 整格不产出（空对象会让页面以为「原生说了，这些都不是 chat」）',
        )
      },
      {
        homeDir: root,
        probeNative: async () => nativeOk({ models: rows(700, 'big').map((each) => each.model), modelMeta: {} }),
      },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

nativeCase('★ provider 只是**查找键**：不在闭集里也不该 4xx（「自定义」那格传什么都不许炸）', async () => {
  const root = scratch('provider-open')
  try {
    await withServer(
      async ({ port }) => {
        for (const provider of ['', '  ', 'whatever', 'dashscope']) {
          const res = await request(port, 'POST', PROBE_PATH, {
            body: { endpoint: ENDPOINT, api_key: KEY, provider },
          })
          assert.equal(res.status, 200, `${provider} 不该 4xx：${res.text}`)
        }
      },
      { homeDir: root, probeNative: async () => null },
    )
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

test('（用例计数）', () => {
  assert.ok(cases > 0, '原生目录层用例数必须大于零')
  console.log(`      native-probe 用例数：${cases}`)
})
