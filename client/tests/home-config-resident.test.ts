// 本机**模型配置**门那一格的**逐支**读法（`src/api/homeConfig.ts` 的 `residentModelReadout` /
// `inUseEntryIds` / `inUseEntryNames`）。
//
// ## 为什么要有它（2026-10-12 那一刀的全部意义）
//
// 门在"绑定"那一层给的是**两**份读数：
//
// | 那一格 | 答的是 | 门怎么算的（两个门逐字同形） |
// |---|---|---|
// | `bound` / `bindingState` | **这台机器**上的一条 —— **第一支**绑的那条 | 先看有没有哪一支读不出来，再取**第一个**绑着的 |
// | `boundPerResident[]` / `bindingDivergent` | **每一支**各绑了哪条 | 一支一行，照 `model_binding.json` 如实列 |
//
// 一个家住 N 支时（`PROJECT_SHAPE.md` §8.0：**隔离的事实单位是 Agent，不是机器**），第一格
// **只是其中一支的**：拿它答"**这一个** Agent 用哪条"，第二行起就是假话；拿它答"这台机器上
// **有没有人在用**"，只要有一支的绑定读不出来，它就被顶成 `null`（"还没配"——而它明明在用）。
//
// ⇒ 逐支那一格必须有消费者。本文件钉住**读法本身**（纯函数）；三处消费者的接线各自另有守卫
// （「我的 Agent」列表行走 `agentRowStatus` 的用例；模型配置页与机器页那一间走 `verify:ui`）。
//
// 每条都先过一遍 `isHomeConfigView` —— 这份载荷必须是**门真会给的形状**，否则这条用例在测空气
// （`home-config.test.ts` 那条纪律同款）。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import {
  inUseEntryIds,
  inUseEntryNames,
  isHomeConfigView,
  residentBindings,
  residentModelLine,
  residentModelReadout,
  unhomedModelLine,
  type HomeConfigView,
  type HomeResidentBinding,
} from '../src/api/homeConfig.ts'

/** 清单里的**两条**（形状照门侧的 `entries[]` 抄：key 一律只回"配没配"）。 */
const ENTRIES = [
  {
    id: 'ms-1',
    name: '主钥匙',
    endpoint: 'https://api.example.invalid/chat/completions',
    model: 'some-model',
    contextWindow: 131072,
    protocol: null,
    keyConfigured: true,
  },
  {
    id: 'ms-2',
    name: '',
    endpoint: 'https://api.example.invalid/v1/responses',
    model: 'other-model',
    contextWindow: 200000,
    protocol: 'responses',
    keyConfigured: true,
  },
]

function view(over: Partial<HomeConfigView> = {}): HomeConfigView {
  const payload: HomeConfigView = {
    readAt: '2026-10-12T12:00:00.000Z',
    state: 'configured',
    keyConfigured: true,
    endpoint: ENTRIES[0]!.endpoint,
    model: ENTRIES[0]!.model,
    contextWindow: ENTRIES[0]!.contextWindow,
    protocol: null,
    reason: null,
    sourcesState: 'loaded',
    entries: ENTRIES,
    bindingState: 'bound',
    bound: 'ms-1',
    ...over,
  }
  assert.ok(isHomeConfigView(payload), '这份载荷不在 `isHomeConfigView` 认得的形状里')
  return payload
}

/** 逐支那一行（门侧 `boundPerResident[]` 的形状）。 */
function row(agentId: string | null, boundId: string | null): HomeResidentBinding {
  return { agentId, state: boundId === null ? 'absent' : 'bound', bound: boundId }
}

/**
 * ★ **那句人话只有一处**（2026-10-12）：机器页那一间与 Agent 页那一段说的是同一件事 ——
 * 两处各拼一遍就是"两个答案"（`mindReadoutFor` 那条教训）。所以拼句收在 `residentModelLine` 里，
 * 这条用例钉住六态各自说什么（**每一态都要能说出一句**，不许留空）。
 */
test('★ 六态各有一句人话（一处拼句，两页共用）', () => {
  const bound = view({ boundPerResident: [row('a-1', 'ms-1')] })
  assert.equal(residentModelLine(residentModelReadout(bound, 'a-1')), `正在用 ${ENTRIES[0]!.name}`)
  const absent = view({ boundPerResident: [row('a-1', null)] })
  assert.match(residentModelLine(residentModelReadout(absent, 'a-1')), /没指定/)
  const unreadable = view({
    boundPerResident: [{ agentId: 'a-1', state: 'unreadable', bound: null }],
  })
  assert.match(residentModelLine(residentModelReadout(unreadable, 'a-1')), /读不出来/)
  const dangling = view({ boundPerResident: [row('a-1', 'ms-9')] })
  assert.match(residentModelLine(residentModelReadout(dangling, 'a-1')), /ms-9/)
  const missing = view({ boundPerResident: [row('a-2', 'ms-1')] })
  assert.match(residentModelLine(residentModelReadout(missing, 'a-1')), /不在这份读数里/)
  assert.match(residentModelLine(residentModelReadout(view(), 'a-1')), /没给逐支那一格/)
})

// ── 一、`residentModelReadout`：这一支用哪一条（六态，一个不压）────────────────

/**
 * ★ **住户表为空 vs 门没给这一格**（2026-10-12）：两件事在页面上要做的事**相反** ——
 * 前者说"先让一支住进来"（并且**不摆**"换成这条"那颗按钮），后者是"不知道"（旧门，退回旧口径）。
 * 判据：`[]` 是**事实**，`undefined` 是**读不到** —— 不许把后者当"没有"。
 */
test('★ 住户表为空 ⇒ 空数组（是事实）；门没给这一格 ⇒ `null`（是不知道）', () => {
  assert.deepEqual(residentBindings(view({ boundPerResident: [] })), [])
  assert.deepEqual(residentBindings(view({ boundPerResident: [row('a-1', 'ms-1')] })).length, 1)
  assert.equal(residentBindings(view()), null, '老门没给逐支那一格 ⇒ 不许当成"没有住户"')
})

test('逐支：这一支绑着一条 ⇒ `bound`（名字与条目一起给出来）', () => {
  const payload = view({ boundPerResident: [row('a-1', 'ms-1'), row('a-2', 'ms-2')] })
  // ⚠ 2026-10-14 起 `bound` 那一支还带**支级覆盖那两格**（`null` = 没覆盖）—— 断言跟着加宽
  //   （判据变强：从前它连"具体用哪个型号"都读不出来）。
  assert.deepEqual(residentModelReadout(payload, 'a-2'), {
    kind: 'bound',
    entry: ENTRIES[1],
    model: null,
    reasoningEffort: null,
  })
})

// ★ **支级覆盖那两格**（2026-10-14）：条目里的型号与**实际在用的**可以是两个不同的东西
// （运行时里绑定赢过条目）⇒ 读数必须带出来、人话必须说出来，否则"页面上说的"与实际不符。
test('★ 逐支：这一支覆盖了型号/思考档 ⇒ 读数带出来，人话也说得出', () => {
  const payload = view({
    boundPerResident: [
      {
        agentId: 'a-1',
        state: 'bound',
        bound: 'ms-1',
        model: 'deepseek-v4.1-flash',
        reasoningEffort: 'max',
      },
    ],
  })
  const readout = residentModelReadout(payload, 'a-1')
  assert.deepEqual(readout, {
    kind: 'bound',
    entry: ENTRIES[0],
    model: 'deepseek-v4.1-flash',
    reasoningEffort: 'max',
  })
  const line = residentModelLine(readout)
  assert.match(line, /正在用/)
  assert.match(line, /型号：deepseek-v4\.1-flash/, `覆盖的型号必须上屏：${line}`)
  assert.match(line, /思考档：max/, `覆盖的档位必须上屏：${line}`)
  // 没覆盖时那句话**一个字都不多说**（不许凭空长出一截）。
  const plain = residentModelLine(residentModelReadout(view({ boundPerResident: [row('a-1', 'ms-1')] }), 'a-1'))
  assert.ok(!plain.includes('型号：'), plain)
  assert.ok(!plain.includes('思考档：'), plain)
  // 老门（不给那两格）⇒ 当"没覆盖"（它那一档的语义与"还没选过覆盖"一致，不会说反话）。
  const legacy = residentModelReadout(view({ boundPerResident: [row('a-1', 'ms-1')] }), 'a-1')
  assert.equal(legacy.kind === 'bound' && legacy.model, null)
})

test('逐支：**各支各的** —— 同一个家两支绑不同，读出来就是不同的两条', () => {
  const payload = view({ boundPerResident: [row('a-1', 'ms-1'), row('a-2', 'ms-2')], bindingDivergent: true })
  const first = residentModelReadout(payload, 'a-1')
  const second = residentModelReadout(payload, 'a-2')
  assert.equal(first.kind === 'bound' && first.entry.id, 'ms-1')
  assert.equal(second.kind === 'bound' && second.entry.id, 'ms-2')
  assert.notEqual(
    first.kind === 'bound' ? first.entry.id : null,
    second.kind === 'bound' ? second.entry.id : null,
    '★★ 这一条就是本刀的全部意义：两支必须读出**各自**那一条（从前两个都读第一支的）',
  )
})

test('逐支：这一支没指定 ⇒ `absent`（这是一个事实，不是读不到）', () => {
  const payload = view({ boundPerResident: [row('a-1', 'ms-1'), row('a-2', null)] })
  assert.equal(residentModelReadout(payload, 'a-2').kind, 'absent')
})

/**
 * ★ **"没指定"那句话只说事实，不替对面断言形态**（2026-10-15）。
 *
 * 从前它写「没指定（走这台机器的环境变量）」—— 那在**命令行形态**里是真的，在 **app 形态**里
 * 是**假的**（`host_process::model_plan`：app 不继承模型来源，"没绑定"就是"还没配" ⇒ 起不来）。
 * 页面分不清对面是哪一种形态 ⇒ 它只说"还没指定用哪一条"；"起不起得来"由知道那一半的读数说
 * （机器页「运行」那一间、以及接上那一刻的结局 —— 见 `anjia.ts` 的 `homed-not-started`）。
 */
test('★ 逐支：没指定那句话不许再断言"走环境变量"（app 形态里那是假的）', () => {
  const line = residentModelLine(residentModelReadout(view({ boundPerResident: [row('a-1', null)] }), 'a-1'))
  assert.match(line, /还没指定/)
  assert.ok(!line.includes('环境变量'), `不许替对面断言形态：${line}`)
})

test('逐支：这一支的绑定读不出来 ⇒ `unreadable`（**不说成 absent**）', () => {
  const payload = view({
    boundPerResident: [{ agentId: 'a-1', state: 'unreadable', bound: null }],
  })
  assert.equal(residentModelReadout(payload, 'a-1').kind, 'unreadable')
})

test('逐支：绑的那条**不在清单里** ⇒ `dangling`（点名是哪一条）', () => {
  const payload = view({ boundPerResident: [row('a-1', 'ms-9')] })
  assert.deepEqual(residentModelReadout(payload, 'a-1'), { kind: 'dangling', id: 'ms-9' })
})

test('逐支：**这一支不在这份读数里** ⇒ `missing`（不许拿整机那格顶）', () => {
  const payload = view({ boundPerResident: [row('a-1', 'ms-1')] })
  assert.equal(residentModelReadout(payload, 'a-2').kind, 'missing')
  // 说不出是哪一支（没有 id）⇒ 同样是"读不到"，绝不替它认一条。
  assert.equal(residentModelReadout(payload, null).kind, 'missing')
  assert.equal(residentModelReadout(payload, '').kind, 'missing')
})

test('逐支：**老门没给逐支那一格** ⇒ `unlisted`（调用方退回整机 `bound`，旧口径不变）', () => {
  assert.equal(residentModelReadout(view(), 'a-1').kind, 'unlisted')
})

// ── 一·B、★ **还没住户化、但已经先选好了**那一格（2026-10-14「选择先于接上」）──────────
//
// 症状（主人当次撞上）：机器上有两条模型来源 ⇒ 接她时壳按"选哪一条 = 花谁的钱，不替她决定"
// 停下来；而"选"的那一格要求"某台机器已认领她" ⇒ 她还没被接上、那一格不摆 ⇒ 两边互等。
// 修法：选择**先落盘**（`<家>/agents/<id>/model_binding.json`），门把它读在**单独一格**
// `boundUnhomed` 里 —— **不许并进 `boundPerResident`**（那一格的主语是住户，混进去就是把她
// 画成住在这里）。
//
// ⚠ 这一格的人话与住户那一格**分开**（`unhomedModelLine`）：同一句话在两处会是假话 ——
// "正在用 X"对一支还没住进来的 Agent 不成立（她没在跑）。

test('★ 先选好的那一支：读出"已经给她选好"（**不说成"正在用"**）', () => {
  const payload = view({
    bindingState: 'absent',
    bound: null,
    boundPerResident: [],
    boundUnhomed: [row('a-9', 'ms-2')],
  })
  assert.deepEqual(residentModelReadout(payload, 'a-9', false), {
    kind: 'bound',
    entry: ENTRIES[1],
    model: null,
    reasoningEffort: null,
  })
  assert.match(unhomedModelLine(residentModelReadout(payload, 'a-9', false)), /已经给它选好/)
  assert.ok(
    !unhomedModelLine(residentModelReadout(payload, 'a-9', false)).includes('正在用'),
    '她还没被接到这台机器上 ⇒ 谈不上"正在用"',
  )
  // 同一份读数里，**住户那一档**问同一个 id 是"她不是住户"（两格分工不许互相顶）。
  assert.equal(residentModelReadout(payload, 'a-9').kind, 'missing')
})

test('★ 没先选过 ⇒ 「还没给她选」（`missing` 在这一格里就是"没选过"，不是"读不到"）', () => {
  const payload = view({ boundPerResident: [], boundUnhomed: [row('a-1', 'ms-1')] })
  assert.equal(residentModelReadout(payload, 'a-9', false).kind, 'missing')
  assert.match(unhomedModelLine(residentModelReadout(payload, 'a-9', false)), /还没给它选/)
})

test('★ 老门不给这一格 ⇒ `unlisted`（如实说"读不到"，**不许说成"没指定"**）', () => {
  const payload = view({ boundPerResident: [] })
  assert.equal(residentModelReadout(payload, 'a-9', false).kind, 'unlisted')
  assert.match(unhomedModelLine(residentModelReadout(payload, 'a-9', false)), /旧版/)
})

test('★ 先选好的那条**读不出来 / 不在清单里** ⇒ 两态各自如实说（不许压成"没选"）', () => {
  const unreadable = view({ boundUnhomed: [{ agentId: 'a-9', state: 'unreadable', bound: null }] })
  assert.match(unhomedModelLine(residentModelReadout(unreadable, 'a-9', false)), /读不出来/)
  const dangling = view({ boundUnhomed: [row('a-9', 'ms-9')] })
  assert.match(unhomedModelLine(residentModelReadout(dangling, 'a-9', false)), /ms-9/)
})

// ── 二、`inUseEntryIds` / `inUseEntryNames`：这台机器上有没有人在用 ────────────

test('在用：逐支在场 ⇒ 任一支绑着的那几条（去重、按住户顺序）', () => {
  const payload = view({
    boundPerResident: [row('a-1', 'ms-1'), row('a-2', 'ms-1'), row('a-3', 'ms-2')],
    bindingDivergent: true,
  })
  assert.deepEqual(inUseEntryIds(payload), ['ms-1', 'ms-2'])
  assert.deepEqual(inUseEntryNames(payload), ['主钥匙', 'other-model'], '名字空时拿模型名顶')
})

test('★ 在用：**一支读不出来、另一支绑着** ⇒ 仍然"有在用"（收掉的那一格）', () => {
  // 门那边这时给的整机 `bound` 是 `null`（`unreadable` 先赢）⇒ 拿它当"有没有人在用"就会说
  // "还没配"。逐支那一格说得出真相。
  const payload = view({
    bindingState: 'unreadable',
    bound: null,
    state: 'unreadable',
    reason: 'not_json',
    boundPerResident: [
      { agentId: 'a-1', state: 'unreadable', bound: null },
      row('a-2', 'ms-1'),
    ],
    bindingDivergent: true,
  })
  assert.equal(payload.bound, null, '前提：整机那一格是 null')
  assert.deepEqual(inUseEntryIds(payload), ['ms-1'])
})

test('在用：**悬空那条也算在用**（不过滤清单 —— 该修的是那一条，不是重配一份）', () => {
  const payload = view({ boundPerResident: [row('a-1', 'ms-9')] })
  assert.deepEqual(inUseEntryIds(payload), ['ms-9'])
  // 名字取自清单 ⇒ 清单里没有它就没有名字（不编一个）。
  assert.deepEqual(inUseEntryNames(payload), [])
})

test('在用：一支都没绑 ⇒ 空（不是"读不到"）', () => {
  const payload = view({ bindingState: 'absent', bound: null, boundPerResident: [row('a-1', null)] })
  assert.deepEqual(inUseEntryIds(payload), [])
})

test('在用：**老门**（无逐支那一格）⇒ 退回整机 `bound` 那一条', () => {
  assert.deepEqual(inUseEntryIds(view()), ['ms-1'])
  assert.deepEqual(inUseEntryIds(view({ bindingState: 'absent', bound: null })), [])
})
