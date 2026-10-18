// **世界身份**（`api/worldIdentity.ts`）与**键命名空间**的机械判据。
//
// 这一刀（2026-09-29）把持久键收进两个作用域命名空间，并**删掉迁移路径**
// （项目未上线，不做数据迁移）：
//   | 作用域 | 键的形状 |
//   |---|---|
//   | 世界级 | `celira.world:<世界身份或退回的凭据>:<名字>` |
//   | 机器级 | `celira.machine:<名字>` |
//
// 判据要钉住四件事：
//   1. 知道世界就用世界，**不知道就退回凭据**（老服务器/离线/演示：行为与从前一字不差）；
//   2. 回执读不懂 ⇒ `null`（**不猜**、不拿别的字段顶替）；
//   3. 空键名是**编程错误**（会让同一作用域下所有事实挤进一个键），必须明确拒绝；
//   4. **`roomPrefs.ts` 零 import 而自己重写了 `celira.world` 字面量** ——
//      两处必须逐字相同（那条纪律见 `room-prefs.test.ts` 的守卫）。
//
// 运行：cd client && npm test

import assert from 'node:assert/strict'
import test from 'node:test'
import {
  CURSOR_KEY_NAME,
  MACHINE_NAMESPACE,
  ROOM_PREFS_KEY_NAME,
  SYS_KEY_NAME,
  WORLD_NAMESPACE,
  isWorldKnown,
  machineScopedKey,
  parseWorldIdentity,
  worldKeyOf,
  worldScopedKey,
} from '../src/api/worldIdentity.ts'
import { roomPrefsKey } from '../src/api/roomPrefs.ts'

/** 门禁不许空跑（与仓库其余门禁同一条纪律）。 */
let completed = 0
const DECLARED_CASES = 9

function worldCase(name: string, fn: () => void) {
  test(name, () => {
    fn()
    completed += 1
  })
}

worldCase('键：知道世界身份就用它（世界级事实按世界归档）', () => {
  assert.equal(worldKeyOf('0f4c1a2e-1111-2222-3333-444455556666', 'clr_token'), '0f4c1a2e-1111-2222-3333-444455556666')
})

worldCase('键：不知道世界身份（空串/空值/非字符串）⇒ 退回凭据 —— 老库、离线、演示一字不差', () => {
  assert.equal(worldKeyOf('', 'clr_token'), 'clr_token')
  assert.equal(worldKeyOf('   ', 'clr_token'), 'clr_token')
  assert.equal(worldKeyOf(null, 'clr_token'), 'clr_token')
  assert.equal(worldKeyOf(undefined, 'clr_token'), 'clr_token')
  assert.equal(worldKeyOf(42 as unknown as string, 'clr_token'), 'clr_token')
})

worldCase('isWorldKnown：只有非空字符串才算"知道"', () => {
  assert.equal(isWorldKnown('abc'), true)
  assert.equal(isWorldKnown('  '), false)
  assert.equal(isWorldKnown(''), false)
  assert.equal(isWorldKnown(null), false)
  assert.equal(isWorldKnown(undefined), false)
})

worldCase('解析回执：`home_id` 是唯一来源，空白串不算', () => {
  assert.equal(parseWorldIdentity({ home_id: 'h-1', display_name: '我家', created_at: 'x' }), 'h-1')
  assert.equal(parseWorldIdentity({ home_id: '  h-2  ' }), 'h-2', '两侧空白去掉')
  assert.equal(parseWorldIdentity({ home_id: '' }), null)
  assert.equal(parseWorldIdentity({ home_id: '   ' }), null)
})

worldCase('解析回执：读不懂就是读不懂（**不猜**、不拿别的字段顶替）', () => {
  assert.equal(parseWorldIdentity(null), null)
  assert.equal(parseWorldIdentity(undefined), null)
  assert.equal(parseWorldIdentity('h-1'), null, '裸字符串不是回执')
  assert.equal(parseWorldIdentity({}), null)
  assert.equal(parseWorldIdentity({ home_id: 42 }), null)
  assert.equal(parseWorldIdentity({ display_name: '只有名字' }), null, '缺 home_id ⇒ null（不拿名字顶替）')
})

worldCase('世界级键：形状是 `celira.world:<作用域>:<名字>`，三个名字各占一个键', () => {
  assert.equal(worldScopedKey('h-1', CURSOR_KEY_NAME), 'celira.world:h-1:cursor')
  assert.equal(worldScopedKey('h-1', SYS_KEY_NAME), 'celira.world:h-1:sys')
  assert.equal(worldScopedKey('h-1', ROOM_PREFS_KEY_NAME), 'celira.world:h-1:room-prefs')
  // 同一作用域下不同名字**绝不撞键**；不同作用域下同名也不撞。
  const keys = [
    worldScopedKey('h-1', CURSOR_KEY_NAME),
    worldScopedKey('h-1', SYS_KEY_NAME),
    worldScopedKey('h-2', CURSOR_KEY_NAME),
  ]
  assert.equal(new Set(keys).size, keys.length, '三个键必须互不相同')
})

worldCase('机器级键：形状是 `celira.machine:<名字>`，**不带世界作用域**', () => {
  assert.equal(machineScopedKey('wallpaper'), 'celira.machine:wallpaper')
  assert.equal(machineScopedKey('notifications'), 'celira.machine:notifications')
  // 判据：机器级的东西换一个世界连，键**不该**跟着变 —— 形状里没有世界那一段。
  assert.ok(!machineScopedKey('wallpaper').includes('h-1'), '机器级键不含世界作用域')
})

worldCase('空键名是**编程错误**（会让整个作用域挤进一个键）⇒ 明确拒绝，不拼出看似正常的键', () => {
  assert.throws(() => worldScopedKey('h-1', ''), /键名不能为空/)
  assert.throws(() => worldScopedKey('h-1', '   '), /键名不能为空/)
  assert.throws(() => machineScopedKey(''), /键名不能为空/)
})

worldCase('交叉核对：`roomPrefs.ts` 不 import `worldIdentity`，自己重写的字面量必须与它逐字相同', () => {
  // roomPrefs 只许依赖存储窄口（那条守卫在 room-prefs.test.ts），不 import worldIdentity，
  // 所以它把 `celira.world` 又写了一遍。这里把两边拼出来的键**逐字对齐** —— 改了一边没改另一边，
  // 这条会红。（这也是"同一个事实只有一个权威位置"在"不许依赖世界侧"约束下的最小代价。）
  assert.equal(roomPrefsKey('h-1'), worldScopedKey('h-1', ROOM_PREFS_KEY_NAME))
  assert.equal(roomPrefsKey(''), worldScopedKey('', ROOM_PREFS_KEY_NAME))
  // 命名空间与键名的字面量也各钉一次（防止有人只改常量、不改测试）。
  assert.equal(WORLD_NAMESPACE, 'celira.world')
  assert.equal(MACHINE_NAMESPACE, 'celira.machine')
})

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.equal(completed, DECLARED_CASES, `用例数 ${completed} 与声明的 ${DECLARED_CASES} 不符`)
  assert.ok(completed > 0, '本文件一条用例都没跑（空跑不算过）')
})
