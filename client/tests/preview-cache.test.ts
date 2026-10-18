// 有上限的预览缓存（`src/lib/previewCache.ts`）的行为测试。
//
// 运行：cd client && npm test
//
// 这一刀要钉住的判据两条：**命中要刷新"最近用过"**（否则 LRU 会逐出错的东西），
// 以及**两头都有上限**（条数与字节数）—— 没有上限的预览缓存就是内存泄漏。

import assert from 'node:assert/strict'
import test from 'node:test'

import { createPreviewCache } from '../src/lib/previewCache.ts'

/** 门禁不许空跑（与 `render-failure.test.ts` 同款纪律）：最后一条断言本文件真的执行了用例。 */
let completed = 0
/** 总数**含最后那条**（与 `render-failure.test.ts` 同口径）。 */
const DECLARED_CASES = 7

function cacheCase(name: string, fn: () => void) {
  test(name, () => {
    fn()
    completed += 1
  })
}

/** 每次 dispose 都记下来 —— "逐出时有没有把 object URL 还回去"正是这一层要保证的事。 */
function makeCache<T>(maxEntries = 3, maxBytes = 1000) {
  const disposed: T[] = []
  const cache = createPreviewCache<T>({ maxEntries, maxBytes, dispose: (v) => disposed.push(v) })
  return { cache, disposed }
}

cacheCase('放进去、取出来；没有就是 null', () => {
  const { cache } = makeCache<string>()
  cache.set('a', 'A', 10)
  assert.equal(cache.get('a'), 'A')
  assert.equal(cache.get('没有这个键'), null)
})

cacheCase('★ 命中会刷新"最近用过"（所以 LRU 逐出的不是它）', () => {
  const { cache, disposed } = makeCache<string>(2, 1000)
  cache.set('a', 'A', 1)
  cache.set('b', 'B', 1)
  // 用一下 a ⇒ 现在"最久没用"的是 b
  assert.equal(cache.get('a'), 'A')
  cache.set('c', 'C', 1)
  assert.equal(cache.get('b'), null, 'b 应该被逐出')
  assert.deepEqual(disposed, ['B'], '被逐出的那个要 dispose（浏览器里 = 撤销 object URL）')
  assert.equal(cache.get('a'), 'A')
  assert.equal(cache.get('c'), 'C')
})

cacheCase('★ 条数上限：逐出最久没用过的那个', () => {
  const { cache, disposed } = makeCache<string>(3, 10_000)
  for (const key of ['a', 'b', 'c', 'd']) cache.set(key, key.toUpperCase(), 1)
  assert.deepEqual(cache.stats(), { entries: 3, bytes: 3 })
  assert.deepEqual(disposed, ['A'])
})

cacheCase('★ 字节上限：大文件进来时按字节逐出（条数没超也要逐）', () => {
  const { cache, disposed } = makeCache<string>(10, 100)
  cache.set('小', '小', 60)
  cache.set('大', '大', 60) // 60 + 60 > 100 ⇒ 得把"小"逐出去
  assert.deepEqual(cache.stats(), { entries: 1, bytes: 60 })
  assert.deepEqual(disposed, ['小'])
  assert.equal(cache.get('大'), '大', '刚放进来的那条不许被自己逐出去')
})

cacheCase('同键覆盖：旧的先还回去，字节数不许算重', () => {
  const { cache, disposed } = makeCache<string>(5, 1000)
  cache.set('a', '旧', 10)
  cache.set('a', '新', 30)
  assert.deepEqual(disposed, ['旧'])
  assert.deepEqual(cache.stats(), { entries: 1, bytes: 30 })
  assert.equal(cache.get('a'), '新')
})

cacheCase('删除与全清都要还回去（换世界/登出那条路）', () => {
  const { cache, disposed } = makeCache<string>(5, 1000)
  cache.set('a', 'A', 1)
  cache.set('b', 'B', 1)
  cache.delete('a')
  assert.deepEqual(disposed, ['A'])
  cache.clear()
  assert.deepEqual(disposed, ['A', 'B'])
  assert.deepEqual(cache.stats(), { entries: 0, bytes: 0 })
})

cacheCase('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(completed >= DECLARED_CASES - 1, `完成 ${completed} < 声明 ${DECLARED_CASES - 1}`)
})
