// 本机**模型配置**：判定「这台机器上还没有模型配置吗」（`src/api/homeConfig.ts` 的
// `modelSourceAbsent`）—— 首次那条路用它决定要不要把人带去机器页的「模型」那一间
// （落点在 `App.tsx` 的 `entryLandingTarget`，两条路共用）。
//
// ## 为什么这一条要单独钉住（本刀的全部意义）
//
// 判据原先写在页面里、认的是**一个词**：`sourcesState === 'absent'`。而两个门对
// "清单文件不在"给的是**两个词**——这不是笔误，是两侧各自的处境：
//
// | 门 | 文件不在时给的读数 |
// |---|---|
// | web（`scripts/home/config-server.js` → `scripts/home/model-sources.js`） | `sourcesState: 'absent'` ＋ `entries: []` |
// | app（壳自己端 `/home-config`，`app/src-tauri/src/home_doors.rs`） | `sourcesState: 'loaded'` ＋ `entries: []` |
//
// ⇒ 认词的那一版在**壳里恒不触发**（而壳里正是用户真正在用的形态）、在 web 形态**假绿**。
// 判据只能落在**两个门共有**的那两格上：`entries` 空 ＋ **没有任何一支在用它**
// （2026-10-12 收：最后一格从"整机 `bound` 是 null"改成逐支口径 —— 多支的家这两句话不一样）。
//
// ## 五条（外加两条边界）
//
// 1. web 门那份载荷 ⇒ true；2. **壳门那份载荷 ⇒ true**（本刀的意义）；
// 3. `unreadable` ⇒ false（**"配置坏了"不许认成"没配"**：清单读不出来时门给的也是空表，
//    而那时该修的是那份文件，不是照着空表再配一条）；4. 清单非空 ⇒ false；
// 5. `bound` 非 null ⇒ false。
//
// 每条都先过一遍 `isHomeConfigView` —— 这份载荷必须是**门真会给的形状**，否则这条用例
// 在测空气（`home-config.test.ts` 那条纪律同款）。
//
// 末条是**同刀另一半的源级守卫**：那句拒绝的出口（「我的 Agent」列表行上那颗按钮）走的是壳
// 真拒了那一支才存在的读数，离线用例造得出来、真机门禁造不出来 —— 理由写在它自己那段注释里。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { isHomeConfigView, modelSourceAbsent, type HomeConfigView } from '../src/api/homeConfig.ts'

/** 一台**还没有任何模型配置**的机器（两台门共有的那几格；差异只在 `sourcesState` 那一个词）。 */
function view(over: Partial<HomeConfigView> = {}): HomeConfigView {
  return {
    readAt: '2026-10-08T09:00:00.000Z',
    state: 'absent',
    keyConfigured: false,
    endpoint: null,
    model: null,
    contextWindow: null,
    protocol: null,
    reason: null,
    sourcesState: 'absent',
    entries: [],
    bindingState: 'absent',
    bound: null,
    ...over,
  }
}

/** 清单里的**一条**（形状照门侧的 `entries[]` 抄：key 一律只回"配没配"）。 */
const ENTRY = {
  id: 'ms-1',
  name: '主钥匙',
  endpoint: 'https://api.example.invalid/chat/completions',
  model: 'some-model',
  contextWindow: 131072,
  protocol: null,
  keyConfigured: true,
}

/** 每条用例的前置：这份载荷必须是**门真会给的形状**（认不出 ⇒ 这一问在测空气）。 */
function doorPayload(over: Partial<HomeConfigView> = {}): HomeConfigView {
  const payload = view(over)
  assert.ok(isHomeConfigView(payload), '这份载荷不在 `isHomeConfigView` 认得的形状里')
  return payload
}

test('web 门：文件不在 ⇒ absent ＋ 空表 ＋ 没绑定 ⇒ 认成"还没有模型配置"', () => {
  const payload = doorPayload({ sourcesState: 'absent' })
  assert.equal(modelSourceAbsent(payload), true)
})

test('★ 壳门：文件不在 ⇒ loaded（空表）＋ 没绑定 ⇒ 同样认成"还没有模型配置"', () => {
  // `app/src-tauri/src/home_doors.rs` 的用例把这一格钉着（`sourcesState == "loaded"`，
  // 注释原话「文件不在就是空表」）——壳里用户真正在用的就是这一份载荷。
  const payload = doorPayload({ sourcesState: 'loaded' })
  assert.equal(modelSourceAbsent(payload), true)
})

test('★ 两台门给的词不同、答案必须相同（判据不认词，只认两门共有的那两格）', () => {
  const web = doorPayload({ sourcesState: 'absent' })
  const shell = doorPayload({ sourcesState: 'loaded' })
  // 词确实不同 —— 这就是本刀要修的裂口（认词的那一版在壳里恒不成立）。
  assert.notEqual(web.sourcesState, shell.sourcesState)
  // 而两门共有的那两格形状一致 ⇒ 同一个答案。
  assert.deepEqual(web.entries, shell.entries)
  assert.equal(web.bound, shell.bound)
  assert.equal(modelSourceAbsent(web), true)
  assert.equal(modelSourceAbsent(shell), true)
})

test('清单读不出来（unreadable）⇒ **不许**认成"还没配"（同样是空表，但那是"读不到"）', () => {
  const payload = doorPayload({ sourcesState: 'unreadable', state: 'unreadable', reason: 'not_json' })
  assert.equal(modelSourceAbsent(payload), false)
})

test('清单里已经有条目 ⇒ 不是"还没有模型配置"（哪怕这一支没绑）', () => {
  const payload = doorPayload({
    sourcesState: 'loaded',
    state: 'absent',
    entries: [ENTRY],
    bound: null,
  })
  assert.equal(modelSourceAbsent(payload), false)
})

test('绑着一条 ⇒ 不是"还没有模型配置"（清单空、绑定却在：悬空绑定的那份载荷）', () => {
  const payload = doorPayload({
    sourcesState: 'loaded',
    state: 'unreadable',
    reason: 'binding_dangling',
    bindingState: 'bound',
    bound: 'ms-1',
  })
  assert.equal(modelSourceAbsent(payload), false)
})

test('两格都占（清单非空 ＋ 绑着一条）⇒ 同上，判据不看派生那一格', () => {
  const payload = doorPayload({
    state: 'configured',
    keyConfigured: true,
    endpoint: ENTRY.endpoint,
    model: ENTRY.model,
    contextWindow: ENTRY.contextWindow,
    sourcesState: 'loaded',
    entries: [ENTRY],
    bindingState: 'bound',
    bound: 'ms-1',
  })
  assert.equal(modelSourceAbsent(payload), false)
})

// ── 六、多支（2026-10-12 收）：判据的最后一格从"第一支绑了吗"改成"**有没有人在用**" ──
//
// 起因：`bound` 那一格是门取的"**第一支**绑的那条"，而**任一支读不出来就会把它顶成 `null`**
// ⇒ 拿它当"这台机器上有没有模型配置"，一个家住两支时会说反（一支在用、另一支的文件坏了）。
test('★ 多支：一支绑着、另一支读不出来 ⇒ **不是"还没配"**（整机那一格这时是 null）', () => {
  const payload = doorPayload({
    sourcesState: 'loaded',
    state: 'unreadable',
    reason: 'not_json',
    bindingState: 'unreadable',
    bound: null,
    bindingDivergent: true,
    boundPerResident: [
      { agentId: 'a-1', state: 'unreadable', bound: null },
      { agentId: 'a-2', state: 'bound', bound: 'ms-1' },
    ],
  })
  assert.equal(payload.bound, null, '前提：整机那一格被"读不出来"顶成 null')
  assert.equal(modelSourceAbsent(payload), false, '有人真在用 ⇒ 不许把人带去"先配一条"')
})

test('多支：一支都没绑、清单也空 ⇒ 仍然是"还没有模型配置"', () => {
  const payload = doorPayload({
    sourcesState: 'loaded',
    entries: [],
    bindingState: 'absent',
    bound: null,
    bindingDivergent: false,
    boundPerResident: [
      { agentId: 'a-1', state: 'absent', bound: null },
      { agentId: 'a-2', state: 'absent', bound: null },
    ],
  })
  assert.equal(modelSourceAbsent(payload), true)
})

test('★ 老门（没有逐支那一格）⇒ 判据与从前**一字不变**', () => {
  const payload = doorPayload({
    sourcesState: 'loaded',
    entries: [ENTRY],
    bindingState: 'bound',
    bound: 'ms-1',
  })
  assert.equal('boundPerResident' in payload, false)
  assert.equal(modelSourceAbsent(payload), false)
})

// ── 同一刀的第二半（源级守卫）───────────────────────────────────────────────
//
// **为什么是源级守卫**：那句拒绝的出口（「我的 Agent」列表行上那颗按钮）走的是壳真的拒了
// 那一支才存在的读数（`store.anjiaByAgent[id].sub === 'missing-model-credential'`）——
// 离线用例造得出来、真机门禁造不出来（`verify:ui` 跑的是 web 形态，壳的命令根本不在）。
// 而"诊断句没有下一步"正是本刀要修的病 ⇒ 接线被谁顺手删掉时，必须有人喊。
// 本仓对这类"测试够不到的渲染支"的成例就是源级守卫（`tests/anjia.test.ts` 那两条）。
//
// ⚠ 它住在这个文件里，是因为本刀只许新建这一个用例文件（`anjia.test.ts` 不在点名清单里）；
// 要挪进 `anjia.test.ts` 请连同这一段的理由一起挪。
test('★ 出口接线（源级守卫）：那句拒绝旁边真的有出口，且只在它那一条上出现', () => {
  const panel = readFileSync(
    fileURLToPath(new URL('../src/components/celira/AgentsListPanel.tsx', import.meta.url)),
    'utf8',
  )
  // 判据认的是**封闭子词**（`api/anjia.ts` 那张表里的一个词），不是"任何一条拒绝"。
  assert.equal(
    (panel.match(/=== 'missing-model-credential'/g) ?? []).length,
    1,
    '「缺模型凭据」这个子词应当只在一处被点名（判据只有一个权威位置）',
  )
  // 去处是**机器页的「模型」那一间**（这一整条路说的就是这台机器）。
  assert.ok(
    panel.includes("navigate('/app/runtimes/local#model')"),
    '出口没有指向机器页的「模型」那一间',
  )
  // 那句话在这一页**两处渲染**（列表行 ／ 创建成功横幅）⇒ 出口两处都要在，且共用同一个组件。
  const exits = panel.match(/<ModelSourceExit \/>/g) ?? []
  assert.equal(exits.length, 2, `出口应当出现 2 次（列表行 ＋ 创建横幅），实际 ${exits.length} 次`)
  // **不许无条件渲染**：每一次出现都得挂在那条判据后面（否则任何一条拒绝/成功都会长出一颗
  // 指错门的按钮 —— 指错门与假路径同族）。
  const ungated = panel
    .split('\n')
    .filter((line) => line.includes('<ModelSourceExit />') && !line.includes('refusedForMissingModelCredential'))
  assert.deepEqual(ungated, [], '出口必须在判据之后（每个渲染点都要挂 `refusedForMissingModelCredential`）')
})
