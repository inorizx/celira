// **房间用户侧偏好（置顶 / 免打扰）**的纯逻辑测试（2026-09-25）。
//
// 这一层治过一次真实的静默失效，所以用例钉的不是"函数返回什么"，而是**几条不许松的判据**：
//
//  1. **它们是用户侧偏好，不是公共事实**：本模块**不许** import 任何世界侧模块
//     （`store` / `core` / `contracts` / `client`）—— 一旦 import 了，就说明有人又想
//     把它接回传输层（那正是原病灶：挂在两个零实现的方法上，拨了没反应）；
//     ⚠ 2026-10-08 存储收尾第二刀把这条边界的**允许集写实**了：它现在 import 存储窄口
//     （`./storage.ts`）—— 那是"东西存在哪"的缝，不是世界侧；除此之外一个都不许加；
//  2. **宽容解析**：读不懂、缺字段、类型不符一律归默认，绝不抛、绝不返回半个对象
//     （与 `prefs.ts` / `wallpaper.ts` 同款纪律）；
//  3. **按世界分键**：换一个 core 服务器就是换一个世界，偏好不该跟着串门；
//  4. **投影是纯函数**：不改入参；值没变时**返回同一个对象**（避免无意义的引用变化）；
//  5. **读写走窄口**（异步、失败如实返回）：写不进去不许静默成"已保存"。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import {
  EMPTY_ROOM_PREFS,
  applyRoomPrefs,
  loadRoomPrefs,
  parseRoomPrefs,
  roomPrefsKey,
  saveRoomPrefs,
  serializeRoomPrefs,
  toggleId,
} from '../src/api/roomPrefs.ts'
import { setStorage, type StoragePort } from '../src/api/storage.ts'

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(true)
})

// ── 1. 边界：只许依赖存储窄口，不许碰世界侧 ─────────────────────────────────

test('roomPrefs 只许 import 存储窄口（它是用户侧偏好，不是公共事实）', () => {
  const source = readFileSync(
    fileURLToPath(new URL('../src/api/roomPrefs.ts', import.meta.url)),
    'utf8',
  )
  // 只看真实的 import 语句，不看注释里提到的名字（注释里正是要讲这段历史）。
  const imports = [...source.matchAll(/^\s*import\b[^\n]*from\s*['"]([^'"]+)['"]/gm)].map((m) => m[1])
  assert.deepEqual(
    imports,
    ['./storage.ts'],
    `roomPrefs 的依赖集变了（现在是 ${imports.join(', ')}）：它只许依赖存储窄口 —— ` +
      '一旦 import 了世界侧模块（store/core/contracts/client），就说明它被接回了传输层',
  )
})

// ── 2. 宽容解析 ─────────────────────────────────────────────────────────────

test('解析：读不懂 / 空 / 非对象 一律归默认，绝不抛', () => {
  for (const raw of [null, undefined, '', 'not json', '[]', '42', '"str"', 'null']) {
    const parsed = parseRoomPrefs(raw)
    assert.equal(parsed.pinned.size, 0, `输入 ${String(raw)} 的 pinned 该为空`)
    assert.equal(parsed.muted.size, 0, `输入 ${String(raw)} 的 muted 该为空`)
  }
})

test('解析：只收非空字符串 id，别的（数字/空串/null/嵌套）一律丢掉', () => {
  const parsed = parseRoomPrefs(
    JSON.stringify({ pinned: ['r-1', '', 42, null, {}, 'r-2'], muted: 'not-an-array' }),
  )
  assert.deepEqual([...parsed.pinned].sort(), ['r-1', 'r-2'])
  assert.equal(parsed.muted.size, 0)
})

test('序列化 → 解析 往返一致（存的是数组，读回来是 Set）', () => {
  const prefs = { pinned: new Set(['a', 'b']), muted: new Set(['c']) }
  const back = parseRoomPrefs(serializeRoomPrefs(prefs))
  assert.deepEqual([...back.pinned].sort(), ['a', 'b'])
  assert.deepEqual([...back.muted], ['c'])
})

test('序列化产物是**两个数组**（JSON 没有 Set —— 别把 Set 直接塞进去）', () => {
  const raw = serializeRoomPrefs({ pinned: new Set(['a']), muted: new Set() })
  const shape = JSON.parse(raw) as { pinned: unknown; muted: unknown }
  assert.ok(Array.isArray(shape.pinned), 'pinned 必须是数组')
  assert.ok(Array.isArray(shape.muted), 'muted 必须是数组')
})

// ── 3. 按世界分键 ───────────────────────────────────────────────────────────

test('存储键按世界分：不同世界不同键，且都在 `celira.world` 命名空间下', () => {
  assert.equal(roomPrefsKey('home-a'), 'celira.world:home-a:room-prefs')
  assert.equal(roomPrefsKey('home-b'), 'celira.world:home-b:room-prefs')
  assert.notEqual(roomPrefsKey('home-a'), roomPrefsKey('home-b'))
  // 空串 / 空白 / 缺省 ⇒ 作用域那一段为空（与 worldIdentity「拿不到世界身份就退回按凭据分键」
  // 同一条纪律）。**注意它仍是 `celira.world:...` 下的键** —— 所以"登出清整族"不会漏掉它。
  assert.equal(roomPrefsKey(''), 'celira.world::room-prefs')
  assert.equal(roomPrefsKey('   '), 'celira.world::room-prefs')
  assert.equal(roomPrefsKey(undefined as unknown as string), 'celira.world::room-prefs')
})

// ── 4. 翻转与投影 ───────────────────────────────────────────────────────────

test('toggleId：加上 / 去掉，且**不改入参**（返回新集合）', () => {
  const original = new Set(['a'])
  const added = toggleId(original, 'b')
  assert.deepEqual([...added].sort(), ['a', 'b'])
  assert.deepEqual([...original], ['a'], '入参不许被改')
  const removed = toggleId(original, 'a')
  assert.equal(removed.size, 0)
  assert.deepEqual([...original], ['a'], '入参不许被改')
})

test('applyRoomPrefs：投影出 pinned / muted，且**不改入参**', () => {
  const rooms = [{ id: 'r-1' }, { id: 'r-2' }]
  const out = applyRoomPrefs(rooms, { pinned: new Set(['r-1']), muted: new Set(['r-2']) })
  assert.equal(out[0].pinned, true)
  assert.equal(out[0].muted, undefined, '没被免打扰的房间不该有 muted')
  assert.equal(out[1].pinned, undefined)
  assert.equal(out[1].muted, true)
  assert.equal(rooms[0].pinned, undefined, '入参不许被改')
})

test('applyRoomPrefs：值没变时**返回同一个对象**（不制造无意义的引用变化）', () => {
  const room = { id: 'r-1', pinned: true as boolean | undefined, muted: undefined }
  const out = applyRoomPrefs([room], { pinned: new Set(['r-1']), muted: new Set() })
  assert.equal(out[0], room, '值相同时该原样返回同一个对象')
})

test('applyRoomPrefs：**偏好为空**时一个字段都不写（演示通道的 fixtures 值原样保留）', () => {
  // 演示通道的 rooms 自带 pinned / muted（fixtures 数据），而用户还没表达过任何偏好
  // ⇒ 不许把 fixtures 那两个值抹掉，也不许制造新的对象引用。
  const room = { id: 'r-1', pinned: true, muted: true }
  const out = applyRoomPrefs([room], EMPTY_ROOM_PREFS)
  assert.equal(out[0], room)
  assert.equal(out[0].pinned, true)
  assert.equal(out[0].muted, true)
})

test('applyRoomPrefs：**偏好非空**时它就是权威（用户一旦表过态，fixtures 的初始值让位）', () => {
  // 判据只有一条：`pinned` 集合里有没有这个 id。一旦用户置顶过任何房间，
  // 集合就成了这一格的唯一权威 —— 不在集合里就是"没置顶"（写 undefined），
  // 而不是"沿用 fixtures 的初始值"。这与传统 IM 一致：置顶是我的看法，我说了算。
  const room = { id: 'r-1', pinned: true as boolean | undefined }
  const out = applyRoomPrefs([room], { pinned: new Set(['r-9']), muted: new Set() })
  assert.equal(out[0].pinned, undefined, '不在置顶集合里 ⇒ 没置顶')
  assert.notEqual(out[0], room, '值变了就该是新对象')
})

// ── 5. 存取（走窄口：读不到归默认、写失败如实返回，都不抛）──────────────────
//
// ⚠ 2026-10-08 存储收尾第二刀：这两条从前同步读 `globalThis.localStorage`，现在经窄口 ——
//   于是判据也跟着挪到"窄口回什么"上（真机那一半——壳里真的落进文件——由 `verify:ui:app` 量）。

/** 一个假窄口：读回给定原文，写按 `canWrite` 如实返回。 */
function fakePort(raw: string | null, canWrite: boolean): StoragePort {
  return {
    async read() {
      return raw
    },
    async write() {
      return canWrite
    },
    async remove() {},
    async keys() {
      return []
    },
  }
}

test('loadRoomPrefs：窄口读不到（Node 环境 / 隐私模式）时归默认，绝不抛', async () => {
  const prefs = await loadRoomPrefs('some-world')
  assert.equal(prefs.pinned.size, 0)
  assert.equal(prefs.muted.size, 0)
})

test('loadRoomPrefs：窄口里那份读得回来（按世界那一格）', async () => {
  const before = (await import('../src/api/storage.ts')).currentStorage()
  setStorage(fakePort(serializeRoomPrefs({ pinned: new Set(['r-1']), muted: new Set(['r-2']) }), true))
  try {
    const prefs = await loadRoomPrefs('some-world')
    assert.deepEqual([...prefs.pinned], ['r-1'])
    assert.deepEqual([...prefs.muted], ['r-2'])
  } finally {
    setStorage(before)
  }
})

test('saveRoomPrefs：窄口写不进去时返回 **false**（不许把失败静默成"已保存"）', async () => {
  const before = (await import('../src/api/storage.ts')).currentStorage()
  setStorage(fakePort(null, false))
  try {
    assert.equal(await saveRoomPrefs('some-world', EMPTY_ROOM_PREFS), false)
  } finally {
    setStorage(before)
  }
  // 没有 localStorage 的 Node 环境里（缺省实现 = 浏览器实现，而这里没有 `globalThis.localStorage`）
  // 也是同一条下场：写不进去 ⇒ `false`，不抛。
  assert.equal(await saveRoomPrefs('some-world', EMPTY_ROOM_PREFS), false)
})
