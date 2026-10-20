// 「把它接到这台机器上」的四态与人话（`src/api/anjia.ts`）——纯逻辑直测。
//
// 这一组用例守四件事（都是踩过的形状）：
//  ① **词表对账**：壳侧九态（`app/src-tauri/src/anjia/commands.rs` 起的稳定词）逐条有归宿，
//     不认识的取值**不许当成功**（大小写/空串/中文词一律收敛成「读不到」）；
//  ② **四态不许压成两态**：读不到 ≠ 失败；「起了但没等到报到」不许冒充成功、也不许冒充失败；
//  ③ **拒绝要说清"什么都没动"**（判据 1/2/3 的护栏就是它：重试才是安全的）；
//  ④ **界面语言纪律**：人话里不许有设计词汇「安家」，也不许出现 `BANNED_FALSE_PATH`
//     那两句假路径（`client/scripts/verify-ui.mjs` 的判据，这里做**源码级**的前置守卫）。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

import {
  REFUSED_SUBS,
  SHELL_KINDS,
  anjiaReadout,
  hasAnjiaBridge,
  narrowKind,
  narrowRefusedSub,
  startAnjia,
  type AnjiaKind,
  type AnjiaUiKind,
} from '../src/api/anjia.ts'

/** 壳与页面合计的全部状态词（九壳 ＋ 两传输 ＋ 页面自己的「还在跑」）。 */
const ALL_KINDS: readonly AnjiaUiKind[] = [
  ...SHELL_KINDS,
  'bridge-missing',
  'bridge-error',
  'running',
]

test('壳侧十态逐条有归宿（词表对账；改壳那边就要改这里）', () => {
  assert.deepEqual(
    [...SHELL_KINDS],
    [
      'homed',
      // ★ 2026-10-15：接上了、但还没选模型 ⇒ 先没起它（第三种结局；不是失败也不是"跑起来了"）。
      'homed-not-started',
      'already-homed',
      'in-flight',
      'world-unreadable',
      'not-owned',
      'not-active',
      'refused',
      'failed',
      'started-no-report',
    ],
  )
  // 壳那边 `anjia::AnjiaOutcome::kind()` 的九个词 ＋ 本层新增的 `in-flight`。
  const source = readFileSync(
    fileURLToPath(new URL('../../app/src-tauri/src/anjia/commands.rs', import.meta.url)),
    'utf8',
  )
  for (const kind of SHELL_KINDS) {
    assert.ok(
      source.includes(`"${kind}"`),
      `壳侧源码里找不到这个稳定词：${kind}（词表两边必须逐字对得上）`,
    )
  }
})

test('★ 接上了但没起它：那句话必须说清"为什么"和"下一步"，且不冒充成功', () => {
  const readout = anjiaReadout('homed-not-started')
  assert.equal(readout.state, 'done', '接上这件事**做成了**（家建好了）——不许说成失败')
  assert.notEqual(readout.tone, 'ok', '但它还没跑起来 ⇒ 不许打绿')
  assert.match(readout.text, /还没选模型/, readout.text)
  assert.match(readout.text, /选/, `下一步要指得出：${readout.text}`)
  // 反面：它不许被说成"进程也起来了"（那是 `homed` 的话）。
  assert.ok(!readout.text.includes('进程也起来了'), readout.text)
})

test('每一个状态都有四态归宿与一句人话（没有落空的）', () => {
  for (const kind of ALL_KINDS) {
    const readout = anjiaReadout(kind)
    assert.ok(readout.text.length > 4, `${kind} 的人话太短：${readout.text}`)
    assert.ok(
      ['working', 'done', 'failed', 'unreadable'].includes(readout.state),
      `${kind} 的四态取值跑出了词表：${readout.state}`,
    )
  }
})

test('★ 读不到 ≠ 失败：连不上世界 / 没有壳 / 壳没回话，一律只说「读不到」', () => {
  const unreadable: AnjiaKind[] = ['world-unreadable', 'bridge-missing', 'bridge-error']
  for (const kind of unreadable) {
    assert.equal(anjiaReadout(kind).state, 'unreadable', `${kind} 不许说成结论`)
  }
  // 而且措辞上不许混：读不到那几句话里不许出现"失败/成没成"的判断。
  assert.match(anjiaReadout('world-unreadable').text, /什么都没动/)
  assert.match(anjiaReadout('bridge-missing').text, /桌面版/)
})

test('★ 第三态不冒充结论：「起了、没等到报到」留在进行中并明说没等到', () => {
  const readout = anjiaReadout('started-no-report')
  assert.equal(readout.state, 'working', '它不是成功，也不是失败')
  assert.equal(readout.tone, 'warn', '不许给"绿"')
  assert.match(readout.text, /没等到/)
})

test('★ 四条拒绝之路都要说清「什么都没动」（用户才知道重试是安全的）', () => {
  for (const kind of ['not-owned', 'not-active', 'refused'] as const) {
    assert.match(
      anjiaReadout(kind).text,
      /什么都没动/,
      `${kind} 必须明说没动过任何东西（判据 1/2/3 的护栏）`,
    )
  }
})

test('★ 四态齐全（不压成两态）：进行中 / 成功 / 失败 / 读不到 各有归宿', () => {
  const states = new Set(ALL_KINDS.map((kind) => anjiaReadout(kind).state))
  for (const sub of REFUSED_SUBS) states.add(anjiaReadout('refused', sub).state)
  assert.deepEqual([...states].sort(), ['done', 'failed', 'unreadable', 'working'])
})

// ── 2026-10-02 刀：`refused` 的封闭子词（拒绝说得清）──────────────────────────

test('★ refused 子词表与壳侧源码逐字对账（封闭；改壳那边就要改这里）', () => {
  assert.deepEqual(
    [...REFUSED_SUBS],
    [
      'missing-repo',
      'missing-executable',
      'home-not-set-up',
      'bad-id-shape',
      'binding-unreadable',
      'missing-model-credential',
      'model-not-chosen',
      'credential-unreadable',
      'empty-id',
    ],
  )
  // 壳那边 `anjia::RefusalReason::sub()`（`mod.rs`）。
  const source = readFileSync(
    fileURLToPath(new URL('../../app/src-tauri/src/anjia/mod.rs', import.meta.url)),
    'utf8',
  )
  for (const sub of REFUSED_SUBS) {
    assert.ok(
      source.includes(`"${sub}"`),
      `壳侧源码里找不到这个子词：${sub}（词表两边必须逐字对得上）`,
    )
  }
})

test('子词收敛：不认识的取值一律 null（页面按整词 refused 回退，不瞎猜）', () => {
  for (const raw of [
    '',
    'MISSING-REPO',
    'missing_repo',
    'missing-model-credentials',
    '缺模型凭据',
    null,
    undefined,
    0,
    {},
  ]) {
    assert.equal(narrowRefusedSub(raw), null, `${String(raw)} 不该被认成子词`)
  }
  for (const sub of REFUSED_SUBS) {
    assert.equal(narrowRefusedSub(sub), sub, `${sub} 是词表里的，应当逐字通过`)
  }
})

test('★ 每个子词一句更细的人话 ＋ 具体下一步（都说清「什么都没动」）', () => {
  const texts = new Set<string>()
  for (const sub of REFUSED_SUBS) {
    const readout = anjiaReadout('refused', sub)
    assert.equal(readout.state, 'failed', `${sub} 是失败那一态`)
    assert.equal(readout.tone, 'warn', `${sub} 的色档与整词 refused 同档（不是红）`)
    assert.match(readout.text, /什么都没动/, `${sub} 必须明说没动过任何东西`)
    assert.ok(readout.text.length > 20, `${sub} 的人话太短：${readout.text}`)
    texts.add(readout.text)
  }
  // 八条各自不同（压成同一句就等于没拆）；且都与通用回退句不同。
  assert.equal(texts.size, REFUSED_SUBS.length, '子词的人话不许互相重复')
  assert.ok(!texts.has(anjiaReadout('refused').text), '子词句不许与通用句相同')
})

test('未知 / 缺席的子词：走通用句（fail-closed，不冒充某种具体原因）', () => {
  const generic = '这台机器上还缺一样东西才能接上它——这次什么都没动。'
  for (const sub of [undefined, null, '', 'unknown-sub', 'missing-model-credentials']) {
    assert.equal(anjiaReadout('refused', sub).text, generic, `${String(sub)} 应当回退到通用句`)
  }
  // 别的结局收下子词也不改变什么（子词只在 refused 上有意义）。
  assert.equal(anjiaReadout('homed', 'empty-id').state, 'done')
})

test('不认识的状态词一律收敛成「读不到」，绝不当成功', () => {
  for (const raw of ['HOMED', 'homed ', '', null, undefined, 0, '成功', 'refused-no-model']) {
    assert.equal(narrowKind(raw), null, `${String(raw)} 不该被认成词表里的态`)
  }
  // 线上来的态**逐个逐字**通过（页面自己那态 `running` 不在其列——它是本地的，
  // 不是壳会回的东西；壳要是回了它，那才是要当心的）。
  for (const kind of [...SHELL_KINDS, 'bridge-missing', 'bridge-error'] as const) {
    assert.equal(narrowKind(kind), kind, `${kind} 是词表里的，应当逐字通过`)
  }
  assert.equal(narrowKind('running'), null, '`running` 是页面自己的态，不许从线上收下')
})

test('没有壳时：判据说没有桥，叫一次也不假装成功', async () => {
  // 判据是「内部注入在不在」（承 shell.ts），不是平台名。
  assert.equal(hasAnjiaBridge({}), false)
  assert.equal(hasAnjiaBridge({ __TAURI_INTERNALS__: undefined }), false)
  assert.equal(hasAnjiaBridge({ __TAURI_INTERNALS__: {} }), true)

  const result = await startAnjia('agent-1')
  assert.equal(result.kind, 'bridge-missing', '浏览器里只能得到「没有壳」这一态')
  assert.equal(anjiaReadout(result.kind).state, 'unreadable')
})

test('★ 界面语言纪律：人话里不许有设计词汇「安家」，也不许有那两句假路径', () => {
  // 设计词汇给设计者，界面语言给用户（`src/lib/plain-language.ts` 的两套词典不许串门）。
  const banned = ['安家', '入籍', '在这台机器上安家', '设置 → 模型配置', 'Core', 'Runtime']
  const texts = [
    ...ALL_KINDS.map((kind) => anjiaReadout(kind).text),
    // 子词那八句是**用户会在「我的 Agent」列表页上看到**的 —— 同一批判据。
    ...REFUSED_SUBS.map((sub) => anjiaReadout('refused', sub).text),
  ]
  for (const text of texts) {
    for (const word of banned) {
      assert.ok(!text.includes(word), `人话里出现了不该上屏的词：${word}（${text}）`)
    }
  }
  // 正对照：词表本身要真会命中（防"写了却恒不命中"的装饰性守卫）。
  assert.ok(banned.some((word) => `在这台机器上安家`.includes(word)))
})

// ── 有桥那一半：装一只**假壳**把「壳回什么 → 页面怎么收」钉住 ──────────────
//
// `@tauri-apps/api` 的 `invoke` 读的就是 `window.__TAURI_INTERNALS__.invoke`
// （`node_modules/@tauri-apps/api/core.js` 第 327-328 行），所以这一半也能在
// `node --test` 里验——**不用真壳在场**（真壳那一支要 Windows 真机，见报告「没做」）。
function fakeShell(invokeImpl: (cmd: string) => Promise<unknown>): () => void {
  const scope = globalThis as unknown as Record<string, unknown>
  const before = { window: scope.window, internals: scope.__TAURI_INTERNALS__ }
  scope.window = globalThis
  scope.__TAURI_INTERNALS__ = { invoke: invokeImpl }
  return () => {
    scope.window = before.window
    scope.__TAURI_INTERNALS__ = before.internals
  }
}

test('★ 有桥时：壳回的读数原样带回来（成功路径不是空壳接线）', async () => {
  const restore = fakeShell(async () => ({
    agentId: 'agent-1',
    kind: 'homed',
    pid: 4242,
    envRoot: '/home/x/agents/agent-1',
    logFile: '/home/x/logs/host-agent-1.out',
    lines: ['[安家] 一行原样读数'],
  }))
  try {
    const result = await startAnjia('agent-1')
    assert.equal(result.kind, 'homed')
    assert.equal(result.pid, 4242, '进程号要带回来（它只是报告，页面上要显示）')
    assert.equal(result.envRoot, '/home/x/agents/agent-1')
    assert.equal(anjiaReadout(result.kind).state, 'done')
  } finally {
    restore()
  }
})

test('★ 壳回了一个词表外的态：按「读不到」收，绝不当成功', async () => {
  const restore = fakeShell(async () => ({ kind: '已经接好了' }))
  try {
    assert.equal((await startAnjia('agent-1')).kind, 'bridge-error')
  } finally {
    restore()
  }
})

test('★ 有桥时：refused 的子词原样带回来（页面按它说得更细）', async () => {
  const restore = fakeShell(async () => ({
    agentId: 'agent-1',
    kind: 'refused',
    sub: 'missing-model-credential',
    pid: null,
    envRoot: null,
    logFile: null,
    lines: ['[安家] 缺模型凭据……'],
  }))
  try {
    const result = await startAnjia('agent-1')
    assert.equal(result.kind, 'refused')
    assert.equal(result.sub, 'missing-model-credential')
    assert.match(anjiaReadout(result.kind, result.sub).text, /模型/)
  } finally {
    restore()
  }
})

test('★ 有桥时：词表外的子词收敛成 null（页面回退通用句，不瞎猜）', async () => {
  const restore = fakeShell(async () => ({ kind: 'refused', sub: 'refused-no-model' }))
  try {
    const result = await startAnjia('agent-1')
    assert.equal(result.kind, 'refused')
    assert.equal(result.sub, null)
    assert.equal(
      anjiaReadout(result.kind, result.sub).text,
      '这台机器上还缺一样东西才能接上它——这次什么都没动。',
    )
  } finally {
    restore()
  }
})

test('★ 壳没回话（命令不在 / 壳内部出错）：bridge-error —— 不假装成功，也不假装没接', async () => {
  const restore = fakeShell(async () => {
    throw new Error('command anjia_start not found')
  })
  try {
    const result = await startAnjia('agent-1')
    assert.equal(result.kind, 'bridge-error')
    assert.equal(anjiaReadout(result.kind).state, 'unreadable')
    assert.equal(result.pid, null, '这一支不许带任何"看起来成了"的读数')
  } finally {
    restore()
  }
})

test('★ 面板的接线还在（源级守卫）：读数在场的那一支渲染读数、且不许再挂那句横幅', () => {
  const panel = readFileSync(
    fileURLToPath(new URL('../src/components/celira/AgentsListPanel.tsx', import.meta.url)),
    'utf8',
  )
  const at = panel.indexOf('{hasShell && readout ? (')
  assert.ok(at > 0, '找不到读数分支——改名/删了？这条守卫要跟着改')
  const elseAt = panel.indexOf(') : (', at)
  assert.ok(elseAt > at, '读数分支后面找不到另一支（浏览器那一支）')
  const shellBranch = panel.slice(at, elseAt)
  assert.ok(shellBranch.includes('readout.text'), '读数分支必须真的渲染读数（空分支＝接线断了）')
  assert.ok(
    !shellBranch.includes('{CREATE_WONT_REPLY}'),
    '读数在场的那一支里不许再把那句横幅**渲染出来**（在壳里它已经不是事实了）',
  )
  // 另一支（浏览器）照旧须在：那条横幅在浏览器里仍是事实。
  assert.ok(
    panel.includes('<p className="text-caption text-muted-foreground">{CREATE_WONT_REPLY}</p>'),
    '浏览器那一支的诚实横幅不见了（浏览器里的话会变成假话）',
  )
})

test('★ 认领出口不许被读数挡住（源级守卫）：配好模型回来之后，那一行仍然点得动', () => {
  const panel = readFileSync(
    fileURLToPath(new URL('../src/components/celira/AgentsListPanel.tsx', import.meta.url)),
    'utf8',
  )
  assert.ok(
    panel.includes('hasShell && shouldOfferAttach(resolution)'),
    '认领出口的接线不见了（`attachAgent` 那一处）—— 改名/删了？这条守卫要跟着改',
  )
  // ⚠ 第一版写成 `!readout && hasShell && shouldOfferAttach(...)` ⇒ 第一次用的路径是
  // 「创建被拒（缺模型源）⇒ 按出口去配好 ⇒ 回到这一页」，而那份拒绝读数是**易失 store 里的
  // 旧话**：配好了它照旧挂着 ⇒ 出口被它挡住 ⇒ 用户**还是没得点**（只能重启 app 才恢复）。
  // 读数说"发生过什么"，出口说"现在能做什么" —— 两件事不互相挡。
  assert.ok(
    !panel.includes('!readout && hasShell && shouldOfferAttach'),
    '认领出口又被 readout 挡住了：配好模型回来还是点不动（只能重启 app）',
  )
})

test('★ 落地页接线（源级守卫）：列表行渲染四态读数；创建读数落在**就地面板**里', () => {
  const panel = readFileSync(
    fileURLToPath(new URL('../src/components/celira/AgentsListPanel.tsx', import.meta.url)),
    'utf8',
  )
  // 数据来自 store 里那份**易失**读数（创建时壳报回来的）——落地页不发新请求。
  assert.ok(panel.includes('s.anjiaByAgent'), '列表要订阅 store 里的读数（数据已在 store）')
  assert.ok(
    panel.includes('anjiaReadout(anjia.kind, anjia.sub)'),
    '列表行必须渲染 anjiaReadout 的人话（含 refused 子词）',
  )
  // ⚠ **2026-10-20**：`createdAgentId`（「新的朋友 → 添加 Agent」创建完跳过来、读数落在那一行）
  // 随那个入口一起删除 —— 创建是**所有权**，不是关系，只发生在这一页的就地面板里，
  // 而读数由面板自己那张卡说。**别把它当"反正没人用"留着**：那是一句永远不成立的判断。
  // 判据落在**代码形状**上（`const justCreated` / `useLocation()`），不落在注释里提没提到这个名字：
  // 上面两段注释是**记这件事为什么被删**，它们必须留着。
  assert.ok(
    !panel.includes('const justCreated') && !panel.includes('useLocation()'),
    '创建入口只剩这一页：不该再有"从别处创建完跳过来、按地址状态认领"那条路（已删）',
  )
  // 无壳那一支照旧说 CREATE_WONT_REPLY（不许改成"看起来成功"）。
  assert.ok(panel.includes('{CREATE_WONT_REPLY}'), '没有壳时必须渲染那句诚实横幅（常量，不内联字符串）')

  const page = readFileSync(
    fileURLToPath(new URL('../src/pages/NewContactPage.tsx', import.meta.url)),
    'utf8',
  )
  // 「新的朋友」只发关系申请：**不许**再创建 Agent（那是所有权，归 /app/agents）。
  assert.ok(!page.includes('createAgent('), '「新的朋友」不该再创建 Agent')
  // 回执必须被用上：四种处境（刚发出 / 之前发过 / **对方先发过、该我点头** / 早就是联系人）
  // 说成同一句，就是替系统撒谎 —— 判据是 `api/uiPolicy.ts` 的 `classifyContactRequest`。
  assert.ok(
    page.includes('classifyContactRequest'),
    '「新的朋友」必须按回执分处境，不许只说一句"已发送"',
  )
  assert.ok(
    !page.includes('请求已发送，等待对方确认'),
    '那句把四种处境压成一种的话不许回来（它还会让"对方先发给我"变成"等对方"）',
  )
})
