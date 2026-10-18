/**
 * §8.3 总开关那一格的**判据**（纯函数层：组件只渲染，判断都在这里）。
 *
 * 为什么这些要用例钉住：宪法那句「**默认值：不替用户做隐蔽的事**」落在这一格上 ——
 * 没表过态时必须写出"这是默认"，**不许**把它说成用户选过；读不到时也不许画成"没在跑"
 * （缺省不等于零）。
 */
import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
  canStopLocalServices,
  localServiceRows,
  quitBehaviourLabel,
  type LocalServicesReadout,
} from '../src/api/shellLocal.ts'

const readout = (over: Partial<LocalServicesReadout> = {}): LocalServicesReadout => ({
  worldRunning: true,
  residents: 1,
  residentsRunning: 1,
  legacyHome: false,
  summary: '世界在跑；1 支在跑',
  quitStopsServices: true,
  quitStopsServicesStated: true,
  ...over,
})

test('退出行为那一格：三态，且"默认"必须说出来', () => {
  assert.equal(quitBehaviourLabel(readout({ quitStopsServices: true })), '退出时：停止本机服务')
  assert.equal(quitBehaviourLabel(readout({ quitStopsServices: false })), '退出时：保持运行')
  // ★ 没表过态 ⇒ 生效值是默认（停），但**页面必须标明这是默认** ——
  //   不许让用户以为"我选过停"（§8.3：不替用户做隐蔽的事）。
  assert.equal(
    quitBehaviourLabel(readout({ quitStopsServices: true, quitStopsServicesStated: false })),
    '退出时：停止本机服务（默认）',
  )
  assert.equal(
    quitBehaviourLabel(readout({ quitStopsServices: false, quitStopsServicesStated: false })),
    '退出时：保持运行（默认）',
    '表过态与否只影响那句"默认"，不影响它现在是什么',
  )
  // 读不到就说读不到（**不画成一个具体行为**）。
  assert.equal(quitBehaviourLabel(null), '退出时：读不到')
})

test('读数行：世界与住户分开说；读不到不等于没在跑', () => {
  const rows = localServiceRows(readout())
  assert.deepEqual(
    rows.map((row) => `${row.label}=${row.value}`),
    ['世界=在跑', 'Agent=1 / 1 支在跑'],
  )
  assert.deepEqual(
    localServiceRows(readout({ worldRunning: false, residentsRunning: 0 })).map((row) => row.value),
    ['没在跑', '0 / 1 支在跑'],
    '世界停了、住户也停了 —— 两行各说各的',
  )
  assert.deepEqual(
    localServiceRows(readout({ residents: 0, residentsRunning: 0 })).map((row) => row.value),
    ['在跑', '这台机器上还没有 Agent'],
    '一支都没住 ⇒ 说"还没有"，不写 0 / 0',
  )
  // ★ 读不到：整格如实说读不到（不是"没在跑"，也不是空表）。
  // ⚠ 措辞 2026-10-11 改：从前那句写「读不到（**这一格只有 app 里才有**）」——而**这一格只可能在
  // app 里渲染**（浏览器形态整格不渲染）⇒ 那句话只会在 app 里出现，等于**把"读失败"说成了"形态不给"**
  // （"把某个失败说成另一个失败"是明文不许的）。走到这里的真实意思是"这一次没从壳里读到"。
  const absent = localServiceRows(null)
  assert.equal(absent.length, 1)
  assert.equal(absent[0]!.value, '读不到（这次没从壳里读到）')
  assert.equal(absent[0]!.tone, 'muted')
  assert.ok(
    !absent[0]!.value.includes('只有 app 里才有'),
    '不许再说"只有 app 里才有"——这句话只可能在 app 里出现，那是拿形态当理由掩盖读失败',
  )
})

// ★ 2026-10-05：**老形态 ≠ 没有 Agent**。
// 壳只认新布局，而老家（`<家>/env`）是 `up.js` 搬的（app 不做迁移）⇒ 零住户有两种来路。
// 同一屏上面那张卡（运行/活动）会说"这个家还是老形态"，这一格不许说成"还没有 Agent"——
// 判据：**认不出的形状不许说成不存在**。
test('读数行：零住户 + 老形态 ⇒ 说"老形态"（灰），不冒充"还没有 Agent"', () => {
  const rows = localServiceRows(readout({ residents: 0, residentsRunning: 0, legacyHome: true }))
  assert.deepEqual(
    rows.map((row) => `${row.label}=${row.value}`),
    ['世界=在跑', 'Agent=这个家还是老形态（住户化之前）'],
  )
  assert.equal(rows[1]!.tone, 'muted', '"认不出"不是"确实没有"（不给 off 那个灯）')
  // 真有住户时**先**说住户（那是主事实）——半迁移的家两边都在场时不许被"老形态"盖住。
  const half = localServiceRows(readout({ residents: 2, residentsRunning: 1, legacyHome: true }))
  assert.equal(half[1]!.value, '1 / 2 支在跑')
  assert.equal(half[1]!.tone, 'on')
  // 老形态而一支都没在跑：仍然是"认不出"那一档（不是 off）。
  const stopped = localServiceRows(readout({ residents: 0, residentsRunning: 0, legacyHome: false }))
  assert.equal(stopped[1]!.value, '这台机器上还没有 Agent')
  assert.equal(stopped[1]!.tone, 'off')
})

test('「全部停止」：有东西在跑才可按（空动作不骗点击）', () => {
  assert.equal(canStopLocalServices(readout()), true)
  assert.equal(canStopLocalServices(readout({ worldRunning: false })), true, '住户还在跑 ⇒ 可按')
  assert.equal(
    canStopLocalServices(readout({ worldRunning: false, residentsRunning: 0 })),
    false,
    '什么都没跑 ⇒ 不给一颗可按的空按钮',
  )
  assert.equal(canStopLocalServices(null), false, '读不到 ⇒ 不可按')
})
