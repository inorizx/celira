/**
 * **这一支在这台机器上的手**（`api/agentHands.ts`）——纯逻辑用例（不需要壳在场）。
 *
 * 钉住三条判据：
 *
 * 1. **读数收敛**：壳回的形状不对 ⇒ `null`（调用方按"读不到"处理，不猜、不默认成关着）；
 * 2. **人话三档有顺序**：不住在这台 / 这台没有终端臂 / 开或关 —— 顺序错了就会对一台
 *    没有终端臂的机器说"关着"（那是假话：它根本开不了）；
 * 3. **没有壳不是"关着"**：`missing` 与 `error` 两态各归各的，页面据此不画开关。
 *
 * ⚠ 本文件不 import 任何 Tauri 模块（`agentHands.ts` 只在真要 invoke 时才动态 import），
 * 所以 `node --test` 的类型剥离模式下能直接跑。
 */
import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
  HANDS_APPLIES,
  HANDS_CONSEQUENCE,
  handsReadoutLine,
  hasHandsBridge,
  normalizeHands,
  readHands,
  setHands,
  type HandsView,
} from '../src/api/agentHands.ts'

const VIEW: HandsView = {
  agentId: 'a-1',
  homed: true,
  declared: null,
  granted: false,
  arm: true,
}

test('读数收敛：形状不对一律 null，不猜', () => {
  assert.deepEqual(normalizeHands(VIEW), VIEW)
  assert.deepEqual(normalizeHands({ ...VIEW, declared: 'full' }), { ...VIEW, declared: 'full' })
  // 三个布尔缺一不可（缺一个就等于"这一格没说清"）。
  for (const broken of [
    null,
    undefined,
    'x',
    {},
    { ...VIEW, homed: 'yes' },
    { ...VIEW, granted: 0 },
    { ...VIEW, arm: undefined },
  ]) {
    assert.equal(normalizeHands(broken), null, `这个形状不许被认成读数：${JSON.stringify(broken)}`)
  }
  // `declared` 不是字符串 ⇒ 收成 null；`agentId` 缺 ⇒ 空串（页面只用它做 key 之外的展示）。
  assert.deepEqual(normalizeHands({ ...VIEW, declared: 7, agentId: undefined }), {
    ...VIEW,
    declared: null,
    agentId: '',
  })
})

test('人话：不住在这台 → 这台机器给不了 → 允许/未允许（顺序就是判据）', () => {
  assert.equal(handsReadoutLine({ ...VIEW, homed: false }), '这一支不住在这台机器上')
  // ⚠ 住在、但本平台没有终端：**不许**说"未允许"（它开不了）。
  assert.equal(
    handsReadoutLine({ ...VIEW, arm: false }),
    '这台机器不提供终端权限（只有 Windows 与 Linux 有）',
  )
  // 屏幕上是权限的语言（2026-10-20 主人：不该叫"手"，该叫权限）。
  assert.equal(handsReadoutLine(VIEW), '未允许')
  assert.equal(handsReadoutLine({ ...VIEW, granted: true, declared: 'full' }), '已允许 · 整台机器')
})

test('没有壳：读/写都回 missing（不是"关着"）', async () => {
  assert.equal(hasHandsBridge({}), false, '默认不是壳')
  assert.equal(hasHandsBridge({ __TAURI_INTERNALS__: {} }), true, '内部注入在就是壳')
  assert.deepEqual(await readHands('a-1'), { kind: 'missing' })
  assert.deepEqual(await setHands('a-1', true), { kind: 'missing' })
})

test('两句话都在：后果说清、生效点说白', () => {
  assert.match(HANDS_CONSEQUENCE, /模型钥匙/, '后果里必须点名模型钥匙（那是真实后果）')
  assert.match(HANDS_APPLIES, /重开 Celira/, '生效点必须写成"重开 Celira"，不许暗示立刻生效')
})
