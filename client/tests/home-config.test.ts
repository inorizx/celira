// 本机**模型配置门**读取层 / 写入层的纯逻辑测试（TASK_77；刀2b 起是"清单 + 绑定"）。
//
// 这一层要紧的判据四条：
//  1. **形状认不出就当读不到**（与状态口同一条纪律）——尤其 `state` 缺了就不能猜：
//     猜错的那个方向（把"读不出来"说成"没配"）会让人去重配一份本来在的文件；
//  2. **错误码是封闭表**：门说了我们不认识的码 ⇒ 页面必须**明确说不认识**，不许编一句
//     听起来合理的话（那是拿假话盖住一次契约漂移）；
//  3. **两侧对表**：那份词表在两个运行时里各有一份（浏览器的 TS / 家的 node 脚本），
//     没法共用代码 ⇒ 用一条对表守住"改一边忘另一边"；
//  4. **三合一缺一层也不接受**（刀2b）：清单与绑定是页面用来分辨"读不出来"与"没有钥匙"
//     的两层，只认半个三合一等于把两种处境混成一种。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { createRequire } from 'node:module'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import {
  HOME_CONFIG_ERROR_CODES,
  HOME_PROBE_URL,
  HomeConfigError,
  REASONING_EFFORTS,
  attachAgentOnMachine,
  bindHomeSource,
  createHomeSource,
  deleteHomeSource,
  fetchHomeCatalog,
  fetchHomeConfig,
  homeConfigErrorText,
  isHomeConfigView,
  modelChangeDraft,
  probeHomeSource,
  updateHomeSource,
} from '../src/api/homeConfig.ts'

const DRAFT = {
  name: '主钥匙',
  endpoint: 'https://api.example.invalid/chat/completions',
  apiKey: 'sk-secret',
  model: 'some-model',
  contextWindow: '131072',
  protocol: '',
}

/**
 * 把 `fetch` 换成替身，记下这次请求（真实调用路径，不是读源码猜）。
 *
 * 为什么值得：这一层的判据是**动作语义**（新建 / 改一条 / 换绑 / 删一条各打哪个方法、
 * 哪个路径、体里有什么），而"改成只动清单"这类错**跑起来才看得见**——页面照旧"成功了"，
 * 变的却是 Agent 正在用的那一条。
 */
async function capture(call: () => Promise<unknown>, response: unknown = GOOD) {
  const calls: { url: string; method: string; body: unknown }[] = []
  const original = globalThis.fetch
  globalThis.fetch = (async (url: string | URL, init: RequestInit = {}) => {
    calls.push({
      url: String(url),
      method: String(init.method ?? 'GET'),
      body: init.body === undefined ? undefined : JSON.parse(String(init.body)),
    })
    return new Response(JSON.stringify(response), {
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

/** 一份**真实形状**的读数（照门侧 `GET /home-config` 的载荷抄的）。 */
const GOOD = {
  readAt: '2026-09-22T13:00:00.000Z',
  state: 'configured',
  keyConfigured: true,
  endpoint: 'https://api.example.invalid/chat/completions',
  model: 'some-model',
  contextWindow: 131072,
  protocol: null,
  reason: null,
  // 刀2b 的三合一：清单（机器级）+ 绑定（Agent 级）+ 派生（上面那六格就是从绑定算的）。
  sourcesState: 'loaded',
  entries: [
    {
      id: 'ms-1',
      name: '主钥匙',
      endpoint: 'https://api.example.invalid/chat/completions',
      model: 'some-model',
      contextWindow: 131072,
      protocol: null,
      keyConfigured: true,
    },
  ],
  bindingState: 'bound',
  bound: 'ms-1',
}

/** 一份**试连读数**（形状：窗口那两格与模型目录那一格**必须给**，可以是 null / 空数组）。 */
const PROBE = {
  url: 'https://x.invalid/models',
  ok: false,
  status: 401,
  reason: null,
  contextWindow: null,
  windowField: null,
  models: [],
}

test('形状守卫：真实形状认得出来（含 `probe` 那一格）', () => {
  assert.equal(isHomeConfigView(GOOD), true)
  assert.equal(isHomeConfigView({ ...GOOD, state: 'absent', keyConfigured: false }), true)
  assert.equal(isHomeConfigView({ ...GOOD, probe: PROBE }), true)
  assert.equal(isHomeConfigView({ ...GOOD, probe: { ...PROBE, status: null, reason: 'timeout' } }), true)
})

// ── 逐支那一格（2026-10-12：一个家住 N 支 ⇒ 门给的是"每一支各绑了哪条"）──────────
//
// 它是**可选**的（老门不给 ⇒ 调用方退回整机 `bound`，单支的家两者等价），但**给了就整格校验**
// —— 半读的逐支表会把"这一支用哪条"指到**别的支**的配置上，而那比读不到更坏。
test('形状守卫：逐支那一格 —— 在场要认得出，缺场照旧接受（老门）', () => {
  const rows = [
    { agentId: 'a-1', state: 'bound', bound: 'ms-1' },
    { agentId: 'a-2', state: 'absent', bound: null },
    // `agentId` 允许 null：门读不到那一支的 id 时就给 null（那一行归不到任何一支）。
    { agentId: null, state: 'unreadable', bound: null },
  ]
  assert.equal(isHomeConfigView({ ...GOOD, boundPerResident: rows, bindingDivergent: true }), true)
  assert.equal(isHomeConfigView({ ...GOOD, boundPerResident: [], bindingDivergent: false }), true)
  // 缺场（老门 / 远程旧版）⇒ 照旧接受：调用方走整机那一格。
  assert.equal(isHomeConfigView(GOOD), true)
})

test('形状守卫：逐支那一格给坏形状 ⇒ **整份读数拒收**（半读的它比读不到更坏）', () => {
  assert.equal(isHomeConfigView({ ...GOOD, boundPerResident: 'nope' }), false, '不是数组')
  assert.equal(
    isHomeConfigView({ ...GOOD, boundPerResident: [{ agentId: 'a-1', state: 'weird', bound: null }] }),
    false,
    'state 不在封闭表里',
  )
  assert.equal(
    isHomeConfigView({ ...GOOD, boundPerResident: [{ agentId: 7, state: 'bound', bound: 'ms-1' }] }),
    false,
    'agentId 既不是字符串也不是 null',
  )
  assert.equal(
    isHomeConfigView({ ...GOOD, boundPerResident: [{ agentId: 'a-1', state: 'bound' }] }),
    false,
    '缺 `bound` 那一格',
  )
  assert.equal(
    isHomeConfigView({ ...GOOD, bindingDivergent: 'yes' }),
    false,
    '`bindingDivergent` 不是布尔',
  )
})

// ── ★ **这两份配置最后是谁改的**（2026-10-18：见证那一格）────────────────────────
//
// 与逐支那一格同一条纪律：**可选、给了就整格校验**。为什么不能"多一个字段不管"——
// 读它的那一处要拿它说"页面在 X 改的 / 有人动过它"，而**半读的见证会把"判不了"说成
// "没改过"**（这一格最坏的一种假话）。
const WITNESS = { state: 'changed', atMs: 1760000000000, detail: 'content', deltaBytes: -7 }

test('形状守卫：见证那一格 —— 在场要认得出，缺场照旧接受（老门）', () => {
  assert.equal(isHomeConfigView({ ...GOOD, listWitness: WITNESS }), true)
  assert.equal(isHomeConfigView({ ...GOOD, witnessChainBreakAt: null }), true)
  assert.equal(isHomeConfigView({ ...GOOD, witnessChainBreakAt: 0 }), true)
  assert.equal(
    isHomeConfigView({
      ...GOOD,
      listWitness: { state: 'same', atMs: 1, detail: null, deltaBytes: null },
      witnessChainBreakAt: 3,
      boundPerResident: [{ agentId: 'a-1', state: 'bound', bound: 'ms-1', witness: WITNESS }],
    }),
    true,
    '逐支那一行的见证与顶层那一格同一个形状',
  )
  // 缺场 = 老门 ⇒ 照旧接受（那一档读成"读不到这一格"，**不许**当成"没改过"）。
  assert.equal(isHomeConfigView(GOOD), true)
  assert.equal(
    isHomeConfigView({ ...GOOD, boundPerResident: [{ agentId: 'a-1', state: 'bound', bound: 'ms-1' }] }),
    true,
    '逐支那一行的见证也可以缺场',
  )
})

test('形状守卫：见证那一格给坏形状 ⇒ **整份读数拒收**（半读的它比读不到更坏）', () => {
  for (const bad of [
    { ...WITNESS, state: 'maybe' },
    { ...WITNESS, atMs: 'now' },
    { ...WITNESS, atMs: Number.NaN },
    { ...WITNESS, detail: 7 },
    { ...WITNESS, deltaBytes: 'lots' },
    { ...WITNESS, deltaBytes: Number.POSITIVE_INFINITY },
    'nope',
  ]) {
    assert.equal(isHomeConfigView({ ...GOOD, listWitness: bad }), false, JSON.stringify(bad))
    assert.equal(
      isHomeConfigView({
        ...GOOD,
        boundPerResident: [{ agentId: 'a-1', state: 'bound', bound: 'ms-1', witness: bad }],
      }),
      false,
      `逐支那一行的见证同一条判据：${JSON.stringify(bad)}`,
    )
  }
  // 链条断点那一格：非负整数或 null，别的都不认（`-1` / 小数 / 字符串各堵一个方向）。
  for (const bad of [-1, 1.5, 'x']) {
    assert.equal(isHomeConfigView({ ...GOOD, witnessChainBreakAt: bad }), false, JSON.stringify(bad))
  }
})

// ── ★ **还没住户化、但已经先选好了**那一格（2026-10-14「选择先于接上」）──────────
//
// 与逐支那一格同一条纪律：**可选、给了就整格校验**。为什么不能"多一个字段不管"——
// 读它的那一处要拿它说"先给她选好的那条是 X"，半读的这一格会把"选过了"说成"没指定"
// （同族的假话：把"读不到"说成"没有"）。
test('形状守卫：先选好那一格 —— 在场要认得出，缺场照旧接受（老门）', () => {
  assert.equal(
    isHomeConfigView({
      ...GOOD,
      boundPerResident: [],
      boundUnhomed: [
        { agentId: 'a-9', state: 'bound', bound: 'ms-2' },
        { agentId: 'a-8', state: 'unreadable', bound: null },
      ],
    }),
    true,
  )
  assert.equal(isHomeConfigView({ ...GOOD, boundUnhomed: [] }), true, '空是一个真答案')
  assert.equal(isHomeConfigView(GOOD), true, '缺场 = 老门 ⇒ 照旧接受（那一档读成"读不到"）')
  // 坏形状 ⇒ 整份拒收（四格各自堵一个方向）。
  assert.equal(isHomeConfigView({ ...GOOD, boundUnhomed: 'nope' }), false, '不是数组')
  assert.equal(
    isHomeConfigView({ ...GOOD, boundUnhomed: [{ agentId: 'a-9', state: 'weird', bound: null }] }),
    false,
    'state 不在封闭表里',
  )
  assert.equal(
    isHomeConfigView({ ...GOOD, boundUnhomed: [{ agentId: 7, state: 'bound', bound: 'ms-2' }] }),
    false,
    'agentId 既不是字符串也不是 null',
  )
  assert.equal(
    isHomeConfigView({ ...GOOD, boundUnhomed: [{ agentId: 'a-9', state: 'bound' }] }),
    false,
    '缺 `bound` 那一格',
  )
})

test('形状守卫：窗口探测那两格（探到 / 探不到 / 窗口那一格是哪儿来的）', () => {
  // 探到 ⇒ 值 + **字段名**一起给（字段名是"如实点名"的那一格，页面会画出来）。
  assert.equal(
    isHomeConfigView({
      ...GOOD,
      windowSource: 'probed',
      probe: {
        ...PROBE,
        ok: true,
        status: 200,
        contextWindow: 131072,
        windowField: 'context_length',
      },
    }),
    true,
  )
  // ⚠ **缺这两格 ⇒ 整份读数判"读不到"**：页面会照着它往输入框里填，而"没探到"与
  // "探到了但没值"是两件不同的事，猜错会让人手填一个假的窗口。
  const { contextWindow: _window, ...withoutWindow } = PROBE
  assert.equal(isHomeConfigView({ ...GOOD, probe: withoutWindow }), false, '缺 contextWindow 要拒')
  const { windowField: _field, ...withoutField } = PROBE
  assert.equal(isHomeConfigView({ ...GOOD, probe: withoutField }), false, '缺 windowField 要拒')
  // 形状不对也拒。
  assert.equal(isHomeConfigView({ ...GOOD, probe: { ...PROBE, contextWindow: '131072' } }), false)
  assert.equal(isHomeConfigView({ ...GOOD, probe: { ...PROBE, windowField: 7 } }), false)
  // `windowSource` 只认那三条取值（人填的 / 试连问来的 / 随包对照表查来的）。
  assert.equal(isHomeConfigView({ ...GOOD, windowSource: 'given' }), true)
  assert.equal(isHomeConfigView({ ...GOOD, windowSource: 'probed' }), true)
  assert.equal(isHomeConfigView({ ...GOOD, windowSource: 'table' }), true, '查表那条路也是合法来路')
  assert.equal(isHomeConfigView({ ...GOOD, windowSource: null }), true)
  assert.equal(isHomeConfigView({ ...GOOD, windowSource: 'magic' }), false)
})

test('形状守卫 fail-closed：缺 `state` / 未知 `state` 一律"读不到"', () => {
  const { state: _state, ...withoutState } = GOOD
  assert.equal(isHomeConfigView(withoutState), false, '缺 state 就只能猜，而猜错会让人重配一份在的文件')
  assert.equal(isHomeConfigView({ ...GOOD, state: 'weird' }), false)
  assert.equal(isHomeConfigView({ ...GOOD, keyConfigured: 'yes' }), false)
  assert.equal(isHomeConfigView({ ...GOOD, readAt: 123 }), false)
  assert.equal(isHomeConfigView({ ...GOOD, contextWindow: '131072' }), false, '窗口是数字，不是字符串')
  assert.equal(isHomeConfigView(null), false)
  assert.equal(isHomeConfigView('configured'), false)
})

test('形状守卫：`probe` 只认那两种结局（拿到码 / 没拿到码 + 原因）', () => {
  assert.equal(isHomeConfigView({ ...GOOD, probe: { ...PROBE, status: '200' } }), false)
  assert.equal(isHomeConfigView({ ...GOOD, probe: { ...PROBE, status: 200, reason: 'boom' } }), false)
  const { url: _url, ...withoutUrl } = PROBE
  assert.equal(isHomeConfigView({ ...GOOD, probe: withoutUrl }), false)
  // **模型目录那一格必须在场**（可以是空数组）：缺了它就是"不知道这家有哪些模型"，
  // 而页面会拿它画"照着挑一个" —— 猜错的方向正好是"这家一个模型都没有"（假的）。
  const { models: _models, ...withoutModels } = PROBE
  assert.equal(isHomeConfigView({ ...GOOD, probe: withoutModels }), false, '缺 models 要拒')
  assert.equal(isHomeConfigView({ ...GOOD, probe: { ...PROBE, models: 'x' } }), false)
  assert.equal(isHomeConfigView({ ...GOOD, probe: { ...PROBE, models: [7] } }), false)
  assert.equal(isHomeConfigView({ ...GOOD, probe: { ...PROBE, models: ['a', 'b'] } }), true)
})

// ---------------------------------------------------------------------------
// 三合一（刀2b）：清单 + 绑定 + 派生 —— 缺一层就不接受
// ---------------------------------------------------------------------------

test('三合一：缺清单 / 绑定那一层 ⇒ 整份读数"读不到"（不许只认半个三合一）', () => {
  const { entries: _entries, ...withoutEntries } = GOOD
  assert.equal(isHomeConfigView(withoutEntries), false, '缺 entries 就分不清"读不出来"与"没有钥匙"')
  const { sourcesState: _sources, ...withoutSources } = GOOD
  assert.equal(isHomeConfigView(withoutSources), false, '缺 sourcesState 同上')
  const { bindingState: _binding, ...withoutBinding } = GOOD
  assert.equal(isHomeConfigView(withoutBinding), false, '缺 bindingState 同上')
  assert.equal(isHomeConfigView({ ...GOOD, sourcesState: 'weird' }), false)
  assert.equal(isHomeConfigView({ ...GOOD, bindingState: 'weird' }), false)
  assert.equal(isHomeConfigView({ ...GOOD, bound: 7 }), false)
  // 负向对照：**合法**的两种"没东西"照常认得出来（空清单 + 没绑定）。
  assert.equal(
    isHomeConfigView({
      ...GOOD,
      state: 'absent',
      keyConfigured: false,
      endpoint: null,
      model: null,
      contextWindow: null,
      sourcesState: 'absent',
      entries: [],
      bindingState: 'absent',
      bound: null,
    }),
    true,
  )
})

test('三合一：条目缺一格 / 类型不对 ⇒ 不许半读（人照着半读的清单会重填一份本来在的）', () => {
  const entry = GOOD.entries[0]!
  assert.equal(isHomeConfigView({ ...GOOD, entries: [entry] }), true)
  for (const key of ['id', 'name', 'endpoint', 'model', 'keyConfigured'] as const) {
    const { [key]: _dropped, ...without } = entry
    assert.equal(
      isHomeConfigView({ ...GOOD, entries: [without] }),
      false,
      `条目缺 ${key} 要拒`,
    )
  }
  assert.equal(isHomeConfigView({ ...GOOD, entries: [{ ...entry, contextWindow: '1' }] }), false)
  assert.equal(isHomeConfigView({ ...GOOD, entries: [{ ...entry, protocol: 7 }] }), false)
  assert.equal(isHomeConfigView({ ...GOOD, entries: 'none' }), false)
})

test('三合一：**悬空绑定**（bound 指的条目不在清单里）形状仍然合法 —— 那是"读不出来"，不是认不出', () => {
  // 这一条的存在意义：门回 `binding_dangling`（state=unreadable、bound 仍在场），
  // 页面要能分辨"Agent 指的那条没了"与"这份响应是坏的"。若守卫在这里判 false，
  // 页面只会说一句"读不到"，人不知道去重绑。
  assert.equal(isHomeConfigView({ ...GOOD, state: 'unreadable', reason: 'binding_dangling' }), true)
})

// ---------------------------------------------------------------------------
// 动作语义（刀2b）：清单归机器，绑定归 Agent —— 四个动作走四条不同的路
// ---------------------------------------------------------------------------

test('新建一条 = `POST /home-config`（**保存即启用**：体里不发 `bind: false`）', async () => {
  const calls = await capture(() => createHomeSource(DRAFT))
  assert.equal(calls.length, 1)
  assert.equal(calls[0]!.method, 'POST')
  assert.equal(calls[0]!.url, '/home-config')
  assert.deepEqual(calls[0]!.body, {
    endpoint: DRAFT.endpoint,
    apiKey: 'sk-secret',
    model: DRAFT.model,
    contextWindow: '131072',
    name: '主钥匙',
  })
  assert.ok(!('bind' in (calls[0]!.body as object)), '新建不发 bind ⇒ 门的缺省 true = 存了就用它')
})

test('改一条 = `PUT /home-config`，体里点名 id 且 `bind: false`（**只动清单，不换 Agent 用的那条**）', async () => {
  const calls = await capture(() => updateHomeSource('ms-2', { ...DRAFT, apiKey: '' }))
  assert.equal(calls[0]!.method, 'PUT')
  assert.equal(calls[0]!.url, '/home-config')
  assert.deepEqual(calls[0]!.body, {
    id: 'ms-2',
    bind: false,
    endpoint: DRAFT.endpoint,
    apiKey: '',
    model: DRAFT.model,
    contextWindow: '131072',
    name: '主钥匙',
  })
})

/**
 * ★ **`vision` 那一格是三态**（2026-10-17）：**没说** ⇒ 体里**没有这一格**（门从旧条目
 * **原样带过来**，老页面保存不许把它删掉）；**`true` / `false`** ⇒ 以它为准。
 *
 * ⚠ 为什么这一条值得一条用例：写成那个随处可见的惯例 `...(draft.vision ? { vision } : {})`
 * 时，`false` 是 **falsy** ⇒ "明确关掉"那一态**整个消失**，门把它读成"没说"、再从旧条目带
 * 回来 ⇒ **这一格永远关不掉**，而且**没有任何报错**（用户以为配好了，实际静默地还是开着）。
 *
 * 判据与门那一侧同一句：`scripts/home/tests/config-server.test.js` 的 `vision-tri-state`。
 * 两侧各有一条 —— 那一份管"门收到没说时怎么办"，这一份管"页面说不说得出那个 false"。
 */
test('★ `vision` 三态：没说 ⇒ 体里没有这一格；`false` 必须**原样发出去**（不许被 falsy 吞掉）', async () => {
  const bodyOf = (call: { body: unknown }) => call.body as Record<string, unknown>
  const off = await capture(() => updateHomeSource('ms-2', { ...DRAFT, apiKey: '', vision: false }))
  assert.equal(
    bodyOf(off[0]!).vision,
    false,
    '明确关掉必须发出去 —— 被吞掉就等于"没说"，这一格就永远关不掉',
  )
  const on = await capture(() => updateHomeSource('ms-2', { ...DRAFT, apiKey: '', vision: true }))
  assert.equal(bodyOf(on[0]!).vision, true)
  // 没说那一态（老页面 / 没碰过这一格）：体里**一个字节都不多**。
  const silent = await capture(() => updateHomeSource('ms-2', { ...DRAFT, apiKey: '' }))
  assert.ok(!('vision' in bodyOf(silent[0]!)), '没说 ⇒ 体里没有这一格（门从旧条目带过来）')
})

/**
 * ★ **换型号那一次的声明**（2026-10-17）：窗口重问 ＋ **「这个模型能看图」不沿用旧值**。
 *
 * 判据一句话：**随模型走的声明，不许在换模型时沿用**。窗口那一半早就成立（留空 ⇒ 门重新问
 * 或查表）；`vision` 没有可重问的来源（`GET /models` 不报模态）⇒ 只能**显式**落到"不支持"，
 * 再由回执请人重新勾。
 *
 * ⚠ 靠"没说"是**这个病本身**：线上那一格的 `undefined` 语义是"从旧条目原样带过来"（门侧的
 * 保险）⇒ 换成一个收不下图像块的模型，工具面照旧说"读得动图片"，直到真去读图那一次才炸在
 * 对方那里。所以这一条**必须**钉住那个 `false` 真的发出去了。
 */
test('★ 换型号那一次：窗口留空重问 ＋ 看图那一格**显式**说不支持（不靠"没说"沿用旧值）', async () => {
  const bodyOf = (call: { body: unknown }) => call.body as Record<string, unknown>
  const entry = { ...GOOD.entries[0]!, vision: true }
  const calls = await capture(() => updateHomeSource('ms-1', modelChangeDraft(entry, 'another-model')))
  const body = bodyOf(calls[0]!)
  assert.equal(body.model, 'another-model')
  assert.equal(body.contextWindow, '', '窗口留空 ⇒ 门重新问一次（它随模型走）')
  assert.equal(body.apiKey, '', '密钥留空 = 沿用那一条已存的那把')
  assert.equal(
    body.vision,
    false,
    '换型号 ⇒ 这一格必须**显式**表态：靠"没说"会被门从旧条目原样带回来（"换了读不了图的模型、工具面照旧说读得动"）',
  )
})

test('换绑 / 解除指定 = `PUT /home-binding`（解绑发空 source，**不是**去删条目）', async () => {
  const bound = await capture(() => bindHomeSource('ms-2'))
  assert.equal(bound[0]!.method, 'PUT')
  assert.equal(bound[0]!.url, '/home-binding')
  assert.deepEqual(bound[0]!.body, { source: 'ms-2' })
  const unbound = await capture(() => bindHomeSource(null))
  assert.equal(unbound[0]!.url, '/home-binding')
  assert.deepEqual(unbound[0]!.body, { source: '' })
})

/**
 * ★ **住户级覆盖**（2026-10-12）：给了 `agentId` 就把它带上（门那一侧只写那一支）；
 * **不给就不带那一格** —— 老门（不认这一格）收到的体与从前**逐字节相同**，不会因为多了个
 * `agentId: null` 而走岔路。
 */
test('★ 给某一支换 = 同一个端点 ＋ 多一格 `agentId`；不给就一个字节都不多', async () => {
  const one = await capture(() => bindHomeSource('ms-2', undefined, 'a-2'))
  assert.equal(one[0]!.method, 'PUT')
  assert.equal(one[0]!.url, '/home-binding')
  assert.deepEqual(one[0]!.body, { source: 'ms-2', agentId: 'a-2' })
  const cleared = await capture(() => bindHomeSource(null, undefined, 'a-2'))
  assert.deepEqual(cleared[0]!.body, { source: '', agentId: 'a-2' })
  const machineWide = await capture(() => bindHomeSource('ms-2'))
  assert.deepEqual(machineWide[0]!.body, { source: 'ms-2' }, '不给 agentId ⇒ 体里没有这一格')
})

/**
 * ★ **支级覆盖那两格**（2026-10-14）："这一支具体用哪个型号 / 什么思考档"。
 *
 * 三态一个都不能含糊：**不给** = 那两格不动（换来源时不抹掉覆盖）；**`null`** = 清掉那一格；
 * **字符串** = 写成它。⚠ 而且**只在点名 `agentId` 时能带** —— 那两格是支级事实，家级默认带它
 * 就是"每一支同一个型号"（门会回 `bad_request`），所以这一层连着 `agentId` 一起判，
 * 不把一笔注定被拒的请求发出去。
 */
test('★ 支级覆盖：给了就带上（含 `null` = 清掉）；不给 agentId 时**一个字节都不多发**', async () => {
  const both = await capture(() =>
    bindHomeSource('ms-1', undefined, 'a-2', { model: 'deepseek-v4.1-flash', reasoningEffort: 'max' }),
  )
  assert.deepEqual(both[0]!.body, {
    source: 'ms-1',
    agentId: 'a-2',
    model: 'deepseek-v4.1-flash',
    reasoningEffort: 'max',
  })
  // 清掉一格：`null` **要发出去**（"不动"是"不给"，两件事不许混）。
  const cleared = await capture(() => bindHomeSource('ms-1', undefined, 'a-2', { model: null }))
  assert.deepEqual(cleared[0]!.body, { source: 'ms-1', agentId: 'a-2', model: null })
  assert.ok(!('reasoningEffort' in cleared[0]!.body), '没点名要动的那一格不许出现在体里')
  // 家级默认（不给 agentId）⇒ 那两格一个都不带（门会拒；不由页面去撞那一下）。
  const machineWide = await capture(() =>
    bindHomeSource('ms-1', undefined, null, { model: 'deepseek-v4.1-flash' }),
  )
  assert.deepEqual(machineWide[0]!.body, { source: 'ms-1' })
})

test('只读目录 = `POST /home-config/catalog`（体里只有条目 id，钥匙由门从清单里读）', async () => {
  let returned: unknown = null
  const calls = await capture(async () => {
    returned = await fetchHomeCatalog('ms-2')
  }, { probe: PROBE })
  assert.equal(calls[0]!.method, 'POST')
  assert.equal(calls[0]!.url, '/home-config/catalog')
  assert.deepEqual(calls[0]!.body, { id: 'ms-2' })
  assert.ok(
    !JSON.stringify(calls[0]!.body).includes('sk-'),
    '体里不许出现任何钥匙（它由门从清单里读）',
  )
  // 回来的就是那份**试连读数**（与探测同一份形状：页面拿它列型号）。
  assert.deepEqual(returned, PROBE)
})

/**
 * ★ **接上一支**（2026-10-15 远程接上那一刀）：体里只有 `agentId`。
 *
 * "哪台机器"**不进请求体** —— 它由"发给谁"表达（本机＝同源代转；外地＝绝对地址 ＋ Bearer
 * 家钥匙，分流在 `homes.ts` 一处）。所以这一层既不许带主机名、也不许带任何钥匙。
 */
test('★ 接上 = `POST /home-control/attach`（体里只有 agentId；结局按封闭表收）', async () => {
  let returned: unknown = null
  const calls = await capture(async () => {
    returned = await attachAgentOnMachine('a-1')
  }, { outcome: 'homed-not-started', agentId: 'a-1', envRoot: '/x', pid: null, lines: ['一行'] })
  assert.equal(calls[0]!.method, 'POST')
  assert.equal(calls[0]!.url, '/home-control/attach')
  assert.deepEqual(calls[0]!.body, { agentId: 'a-1' })
  assert.deepEqual(returned, {
    outcome: 'homed-not-started',
    agentId: 'a-1',
    envRoot: '/x',
    pid: null,
    lines: ['一行'],
  })
})

test('★ 接上：不认识的结局**不许当成功**（那一类错就是把拒绝说成成功）', async () => {
  await assert.rejects(
    () => capture(() => attachAgentOnMachine('a-1'), { outcome: 'weird-new-word' }),
    /不认识的结局/,
  )
})

test('★ 接上：那台机器拒绝时，**它自己说的话**要跟着出来（`lines`）', async () => {
  const original = globalThis.fetch
  globalThis.fetch = (async () =>
    new Response(JSON.stringify({ error: 'attach_failed', lines: ['[home_up] 缺凭据'] }), {
      status: 500,
      headers: { 'content-type': 'application/json' },
    })) as typeof fetch
  try {
    await assert.rejects(
      () => attachAgentOnMachine('a-1'),
      (error: unknown) => {
        assert.ok(error instanceof HomeConfigError)
        assert.equal(error.code, 'attach_failed')
        assert.deepEqual(error.lines, ['[home_up] 缺凭据'])
        return true
      },
    )
  } finally {
    globalThis.fetch = original
  }
})

test('★ 视图那一格：`remoteControl` 三态收下；别的取值**整份拒收**（指错方向比读不到更坏）', () => {
  for (const value of ['on', 'off', null, undefined]) {
    assert.equal(isHomeConfigView({ ...GOOD, remoteControl: value }), true, String(value))
  }
  for (const bad of ['ON', 'yes', true, 1, 'enabled']) {
    assert.equal(isHomeConfigView({ ...GOOD, remoteControl: bad }), false, String(bad))
  }
})

test('删一条 = `DELETE /home-config?id=`（销毁式：删的是**那一条**，不是"解除指定"）', async () => {
  const calls = await capture(() => deleteHomeSource('ms-3'))
  assert.equal(calls[0]!.method, 'DELETE')
  assert.equal(calls[0]!.url, '/home-config?id=ms-3')
  assert.equal(calls[0]!.body, undefined)
})

test('读 = `GET /home-config`，不带体', async () => {
  const calls = await capture(() => fetchHomeConfig())
  assert.equal(calls[0]!.method, 'GET')
  assert.equal(calls[0]!.url, '/home-config')
  assert.equal(calls[0]!.body, undefined)
})

test('词表里的人话：每一条都能说出一句（没有留空的格）', () => {
  for (const code of HOME_CONFIG_ERROR_CODES) {
    const text = homeConfigErrorText(code)
    assert.ok(text.length > 2, `${code} 缺人话`)
    assert.notEqual(text, `模型门回了一个我不认识的错误码：${code}`, `${code} 必须在词表里`)
  }
})

test('不认识的错误码**明确说不认识**，并把原话带出来（不许编一句合理的）', () => {
  const text = homeConfigErrorText('some_new_code_from_a_newer_door')
  assert.match(text, /不认识的错误码/)
  assert.match(text, /some_new_code_from_a_newer_door/)
  const error = new HomeConfigError('some_new_code_from_a_newer_door')
  assert.equal(error.known, false)
  assert.equal(error.code, 'some_new_code_from_a_newer_door')
  // 正对照：表里的码是 known 的。
  assert.equal(new HomeConfigError('invalid_window').known, true)
})

test('门的拒绝可以顺手带回一份试连读数（`window_unavailable` 用它说清"为什么没问到"）', () => {
  // 没有读数时就是没有（不编）。
  assert.equal(new HomeConfigError('window_unavailable').probe, null)
  // 有读数时照原样带出来：页面才能把"连不上 / 接口没给"这件事摆在人眼前，
  // 而不是只丢一句"请手填"。
  const carried = new HomeConfigError('window_unavailable', {
    url: 'https://x.invalid/models',
    ok: false,
    status: null,
    reason: 'unreachable',
    contextWindow: null,
    windowField: null,
    models: [],
  })
  assert.equal(carried.code, 'window_unavailable')
  assert.equal(carried.known, true)
  assert.equal(carried.probe?.reason, 'unreachable')
  assert.match(carried.message, /手填/)
})

test('两侧词表**逐字相同**（防漂移：客户端那份 vs 门那份）', () => {
  // 两份住在两个运行时里（浏览器 TS / 家的 node 脚本），没法共用一份代码；
  // 于是用这条**对表**守住"改一边忘另一边"——那正是契约最容易钻进来的缝。
  const require = createRequire(import.meta.url)
  const door = require('../../scripts/home/config-server.js') as {
    ERRORS: Record<string, string>
    modelSources: { REASONING_EFFORTS: readonly string[] }
  }
  assert.deepEqual(
    [...HOME_CONFIG_ERROR_CODES].sort(),
    [...Object.values(door.ERRORS)].sort(),
    '客户端与门的错误码词表必须一模一样',
  )
  // ★ **思考档那张表**（2026-10-14）：页面拿它画选项、门拿它做写入口预检、运行时拿它做终审
  //   —— 三处各一份。这里比"客户端 vs 门"这一对；门 vs 运行时那一对在
  //   `scripts/home/tests/model-sources-binding.test.js`（那一侧读的是源文件文本）。
  assert.deepEqual(
    [...REASONING_EFFORTS],
    [...door.modelSources.REASONING_EFFORTS],
    '客户端与门的思考档词表必须一模一样',
  )
})

/**
 * **页面接线**（源级判据，与 `no-silent-red-path.test.ts` 同一档）。
 *
 * 这个仓库的 client 测试是 `node --test` + TS 类型剥离，**没有 jsdom / 组件渲染** ⇒
 * "探到之后到底有没有填进输入框、探不到之后降级文案在不在"只能按源码判。判据要能红：
 * 把 `view.windowSource === 'probed'` 那一段删掉、把「已自动获取」或那段删除警示删掉，
 * 对应断言立刻红。
 */
test('页面接线：自动填窗口 / 降级文案 / 摘除警示——三件事都在源码里', () => {
  const page = [
    // ⚠ **2026-10-12 第三刀**：这一族断言的锚点**横跨两个文件**了 —— "窗口的来路"
    //   在页面里算（读门的 `windowSource`），而它在**面板**里被渲染出来。
    //   ⇒ 读两份、拼起来断言（判据一字未改，强度不变：锚点缺了在哪一份里都找不到）。
    readFileSync(fileURLToPath(new URL('../src/pages/ModelProvidersPage.tsx', import.meta.url)), 'utf8'),
    readFileSync(fileURLToPath(new URL('../src/components/celira/ProviderPanel.tsx', import.meta.url)), 'utf8'),
  ].join('\n')
  /** ★ 解除那一族动作现在住在**那一支自己的页面**上（2026-10-15 从配置页搬过去）。 */
  const agentPage = readFileSync(
    fileURLToPath(new URL('../src/pages/MyAgentsPage.tsx', import.meta.url)),
    'utf8',
  )
  // ③ 探到 ⇒ 窗口那一格旁注「已自动获取」（读数来自门的 `windowSource`，不是猜的）。
  assert.match(page, /view\.windowSource === 'probed'/, '窗口的来路要读门的读数')
  assert.match(page, /已自动获取/, '探到之后要旁注「已自动获取」')
  // ③ ⚠ 2026-09-23 兜底表刀：**自动来路从一种变成两种**（试连问来的 / 随包对照表查来的）⇒
  //    判据跟着搬，**约束不变、强度更高**：两条来路都必须**读门的读数**，且**各自如实标注**。
  assert.match(page, /view\.windowSource === 'table'/, '查表那条来路同样要读门的读数（不许页面自己猜）')
  assert.match(page, /来自对照表/, '查表来的值要标「来自对照表」，不许冒充「服务方给的」')
  // ③ 探不到 ⇒ 输入框 placeholder 降级到"这时才需要你填"。
  // ⚠ 2026-09-23 文案减重刀：这一格底下的 helper **整段删了**（安静时刻不先讲机制）⇒
  //    这句降级文案**只剩 placeholder 这一个落点**，判据跟着搬过去（约束不变，只换承载者）。
  assert.match(page, /windowProbeFailed/, '降级状态要有')
  assert.match(page, /自动获取失败，请手填/, '降级文案要逐字在（现在挂在 placeholder 上）')
  // ③ 窗口是"体检"：输入框必须允许留空（留空 = 保存时自动获取）。
  assert.match(page, /留空 = 保存时自动获取|留空就行/, '不许再要求人必填窗口')
  // ① 删除的**确认门口** + 那三件事（没指定就是没配 / 它会停下 / 怎么让它恢复）。
  // ⚠ 人话刀（2026-09-22）把这些词换成人话了：摘除→删除、供应商→服务方、窗口→上下文长度。
  //    判据本身一条没变（还是那三件事在不在源码里）。
  // ⚠ **2026-10-07 判据变了**（主人裁：**app 形态不继承模型来源**，见
  //    `app/src-tauri/src/anjia/host_process.rs` 的 `model_plan`）：从前第一件写的是"环境变量里
  //    也没有模型来源"，而 app 里那条路**已经不存在**（没绑定就是没配、她起不来）⇒ 断言换成新
  //    判据的话，**约束不变、强度更高**：后果（缺模型就停下）与恢复出口（重新指定一条）都还要在。
  // ⚠ **2026-10-15 换了承载者**（主人裁：**模型配置归机器侧**）：解除那一族动作从配置页搬到
  //    **那一支自己的页面**（`MyAgentsPage.tsx`）—— 这三件事**跟着搬**，判据一字未改
  //    （**约束不变**：动作在哪，"先亮警示再动手"就在哪）。
  assert.match(agentPage, /确认解除/, '解除要先亮警示再动手')
  assert.match(agentPage, /不继承/, '第一件：app 不继承这台机器的环境变量（没指定就是没配）')
  assert.match(agentPage, /没有模型来源而停下/, '第二件：它会停下（并且说清是缺模型）')
  assert.match(agentPage, /重新给它指定一条/, '第三件：怎么让它恢复')
  // ④ 原生目录层（2026-09-24）：探的时候要**告诉门是哪一家**，探回来的**权威品类要喂给候选列表**。
  //    两处都是"接上了才有意义"的接线 —— 断掉任何一处，功能静默退化成按名字猜（页面照样不报错），
  //    所以只能按源码钉（本仓库没有组件渲染，见本文件上方那条说明）。
  assert.match(
    page,
    /providerId !== 'custom' \? \{ provider: providerId \}/,
    '★ 探测要带上"是哪一家"（原生目录的查找键；`custom` 不知道是哪一家 ⇒ 不带）',
  )
  // ★★ **2026-10-16「读的顺序」那两条**（主人当次："还是很混乱，不够符合直觉，太晦涩了"）。
  //
  // 它们**只**能按源码钉（本仓库没有组件渲染）：一条是**默认值**（目录收起），一条是**次序**
  // （回执排在主体之后）。两条都是"用户第一眼看到什么"的判据 —— 上一版的病是
  // **一坨 37 个模型芯片压在主体之上**，而主体（地址/密钥/型号/窗口）要滚很久才看见。
  const catalog = readFileSync(
    fileURLToPath(new URL('../src/components/celira/ModelCatalog.tsx', import.meta.url)),
    'utf8',
  )
  assert.match(
    catalog,
    /const \[open, setOpen\] = useState\(false\)/,
    '★ 型号目录**默认收起**（它是试连的产物，不许自动铺满首屏）',
  )
  assert.match(catalog, /这条能调哪些型号（/, '★ 收起时那行字要说清"这是什么"（不是"37 个模型"）')
  // 次序：主体（`<ProviderPanel`）必须排在回执（`{notice &&`）**之前**。
  const panelAt = page.indexOf('<ProviderPanel')
  const noticeAt = page.indexOf('{notice && (')
  assert.ok(panelAt > 0 && noticeAt > 0, '两处都该在页面里（主体与回执）')
  assert.ok(
    panelAt < noticeAt,
    '★ 回执（已保存 / 连接测试 / 型号目录）必须排在**主体之后** —— 先说这一条是什么，再说刚才那一下的结果',
  )
  assert.match(page, /kinds=\{notice\.probe\.modelKinds\}/, '★ 服务方给的品类要喂给候选列表（不喂就等于白探）')
})

test('门的默认端口与家的三个端口同一个家（老 home.conf 没有这一键也能起）', () => {
  const require = createRequire(import.meta.url)
  const paths = require('../../scripts/home/paths.js') as {
    DEFAULT_CONFIG_PORT: string
    DEFAULT_STATUS_PORT: string
    DEFAULT_CLIENT_PORT: string
  }
  assert.equal(paths.DEFAULT_CONFIG_PORT, '5191')
  // 不得与既有端口冲突（三个端口两类都不同）。
  const taken = [paths.DEFAULT_STATUS_PORT, paths.DEFAULT_CLIENT_PORT, '8080']
  assert.ok(!taken.includes(paths.DEFAULT_CONFIG_PORT), `config_port 撞了既有端口：${taken.join(', ')}`)
})

// ---------------------------------------------------------------------------
// 只读探测（`POST /home-config/probe`，2026-09-23 预设供应商层）
//
// 客户端这一层的判据只有三件：**打哪条路**、**体里有什么**、**回执认不出怎么办**。
// "不落盘"是**门侧**的事（`scripts/home/tests/config-server.test.js` 拿文件指纹判），
// 这里不替它背书 —— 两侧各判各的那一半。
// ---------------------------------------------------------------------------

/** 一份**合法**的探测回执（形状照门侧 `probeModels` 的返回）。 */
const PROBE_READING = {
  url: 'https://api.example.invalid/v1/models',
  ok: true,
  status: 200,
  reason: null,
  contextWindow: null,
  windowField: null,
  models: ['model-a', 'model-b'],
}

/** 同 `capture`，但**回执由用例给**（探测那条路的回执是 `{probe}`，不是整份读数）。 */
async function captureReply(reply: unknown, status: number, call: () => Promise<unknown>) {
  const calls: { url: string; method: string; body: unknown }[] = []
  const original = globalThis.fetch
  globalThis.fetch = (async (url: string | URL, init: RequestInit = {}) => {
    calls.push({
      url: String(url),
      method: String(init.method ?? 'GET'),
      body: init.body === undefined ? undefined : JSON.parse(String(init.body)),
    })
    return new Response(JSON.stringify(reply), {
      status,
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

const PROBE_INPUT = {
  endpoint: 'https://api.example.invalid/v1/chat/completions',
  apiKey: 'sk-probe-canary',
}

test('探测 = `POST /home-config/probe`，体里的键名是 `api_key`（**门侧定的形状**）', async () => {
  assert.equal(HOME_PROBE_URL, '/home-config/probe')
  const calls = await captureReply({ probe: PROBE_READING }, 200, () => probeHomeSource(PROBE_INPUT))
  assert.equal(calls.length, 1)
  assert.equal(calls[0]!.method, 'POST')
  assert.equal(calls[0]!.url, HOME_PROBE_URL)
  assert.deepEqual(calls[0]!.body, { endpoint: PROBE_INPUT.endpoint, api_key: PROBE_INPUT.apiKey })
  // ⚠ 键名与保存那条**有意不同**（保存走 `apiKey`）：`validateProbe` 只认 `api_key`。
  //    这里连"体里恰好只有这两个键"一起钉住 —— 多塞一个字段就多一份两侧漂移面。
  assert.deepEqual(Object.keys(calls[0]!.body as object).sort(), ['api_key', 'endpoint'])
})

test('探测：可选的协议 / 模型名**只在给了的时候**才进体（不给就不占位置）', async () => {
  const optional = await captureReply({ probe: PROBE_READING }, 200, () =>
    probeHomeSource({ ...PROBE_INPUT, protocol: 'responses', model: 'model-a' }),
  )
  assert.deepEqual(optional[0]!.body, {
    endpoint: PROBE_INPUT.endpoint,
    api_key: PROBE_INPUT.apiKey,
    protocol: 'responses',
    model: 'model-a',
  })
})

test('探测：**可选的 provider**（原生目录的查找键）只在给了的时候才进体', async () => {
  const withProvider = await captureReply({ probe: PROBE_READING }, 200, () =>
    probeHomeSource({ ...PROBE_INPUT, provider: 'dashscope' }),
  )
  assert.deepEqual(withProvider[0]!.body, {
    endpoint: PROBE_INPUT.endpoint,
    api_key: PROBE_INPUT.apiKey,
    provider: 'dashscope',
  })
  // ★ 负对照：不给（「自定义」那条路）⇒ 一个字节都不多带 —— 门那边"查不中就退兼容端点"，
  //   带一个我们自己猜的 provider 反而会让它去敲错的原生端点。
  const without = await captureReply({ probe: PROBE_READING }, 200, () => probeHomeSource(PROBE_INPUT))
  assert.equal('provider' in (without[0]!.body as object), false)
})

test('探测：回执里的 `modelKinds`（原生目录给的权威品类）原样交回来；没有就没有这一格', async () => {
  const kinds = { 'model-a': ['chat'], 'model-b': ['embedding'] }
  await captureReply({ probe: { ...PROBE_READING, modelKinds: kinds } }, 200, async () => {
    const reading = await probeHomeSource(PROBE_INPUT)
    assert.deepEqual(reading.modelKinds, kinds, '★ 供应商给了品类 ⇒ 页面按它打标签（不退回按名字猜）')
    return reading
  })
  await captureReply({ probe: PROBE_READING }, 200, async () => {
    const reading = await probeHomeSource(PROBE_INPUT)
    assert.equal('modelKinds' in reading, false, '★ 兼容端点没有这一格 ⇒ 不编一个空对象假装拿到了')
    return reading
  })
})

test('探测：回执交回来的就是那份试连读数（含模型目录；**空目录不是错误**）', async () => {
  const empty = { ...PROBE_READING, models: [] }
  const reply = await captureReply({ probe: empty }, 200, async () => {
    const reading = await probeHomeSource(PROBE_INPUT)
    assert.deepEqual(reading, empty)
    return reading
  })
  assert.equal(reply.length, 1)
})

test('探测：回执形状认不出 ⇒ **抛**（不半读半猜）', async () => {
  // 缺 `models` 那一格：`isProbe` 要求它在场（缺了就是"不知道这家有哪些模型"）。
  const { models: _omitted, ...withoutModels } = PROBE_READING
  await assert.rejects(
    () => captureReply({ probe: withoutModels }, 200, () => probeHomeSource(PROBE_INPUT)),
    /认不出的探测读数/,
  )
  await assert.rejects(
    () => captureReply({ probe: { url: 1 } }, 200, () => probeHomeSource(PROBE_INPUT)),
    /认不出的探测读数/,
  )
  // 连 `probe` 那一格都没有（比如打错了路径、回了一份清单读数）—— 同样认不出。
  await assert.rejects(
    () => captureReply(GOOD, 200, () => probeHomeSource(PROBE_INPUT)),
    /认不出的探测读数/,
  )
})

test('探测：门的拒绝接成人话，且**没有引入未入表的码**（`key_required` 在封闭表里）', async () => {
  const original = globalThis.fetch
  globalThis.fetch = (async () =>
    new Response(JSON.stringify({ error: 'key_required' }), {
      status: 400,
      headers: { 'content-type': 'application/json' },
    })) as typeof fetch
  try {
    await assert.rejects(
      () => probeHomeSource({ ...PROBE_INPUT, apiKey: '' }),
      (error: unknown) => {
        assert.ok(error instanceof HomeConfigError, '拒绝要抛 HomeConfigError')
        assert.equal(error.code, 'key_required')
        assert.equal(error.known, true, '探测那条路的码也必须落在封闭表里')
        // 页面直接把这句话摆出来（`role="alert"`），所以它与词表必须逐字一致。
        assert.equal(error.message, homeConfigErrorText('key_required'))
        return true
      },
    )
  } finally {
    globalThis.fetch = original
  }
})
