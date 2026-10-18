// 未读角标的接线守卫（`src/api/unreadBadge.ts` ＋ `src/App.tsx`）。
//
// 运行：cd client && npm test
//
// 为什么要守卫：这条线的失败在界面上**看不出来** —— 角标不亮，用户只会觉得"它就是没有"。
// 而"命令名字对不对"由 `scripts/check-boundary.mjs` 那条既有接缝自动管（invoke ⊆ 壳注册），
// 所以这里只钉三件它管不到的事。

import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import test from 'node:test'

const REPO = path.resolve(import.meta.dirname, '..')
const read = (relative: string) => fs.readFileSync(path.join(REPO, relative), 'utf8')

/** 门禁不许空跑（与 `render-failure.test.ts` 同款纪律）：最后一条断言本文件真的执行了用例。 */
let completed = 0
/** 总数**含最后那条**（与 `render-failure.test.ts` 同口径）。 */
const DECLARED_CASES = 4

function badgeCase(name: string, fn: () => void) {
  test(name, () => {
    fn()
    completed += 1
  })
}

badgeCase('★ 只有壳里有这条：浏览器形态是空操作，且只在壳里才去拿桥', () => {
  const source = read('src/api/unreadBadge.ts')
  assert.match(source, /if \(!isTauriRuntime\(\)\) return/, '浏览器里直接返回（不是"还没实现"）')
  assert.match(source, /await import\('@tauri-apps\/api\/core'\)/, '桥要**动态**拿（浏览器里不许碰它）')
  assert.match(source, /invoke<string>\('set_unread_total'/, '报的就是这个命令')
})

badgeCase('★ 同一个数不重复过桥（房间列表每变一次都报会很吵）', () => {
  const source = read('src/api/unreadBadge.ts')
  assert.match(source, /if \(lastReported === count\) return/, '要有那道"报过了"的闸')
})

badgeCase('★ 失败不静默（角标不亮只能靠这条线索查）', () => {
  const source = read('src/api/unreadBadge.ts')
  assert.match(source, /catch \(error\) \{[\s\S]{0,200}console\.error/, 'catch 里必须留下痕迹')
  assert.ok(!/throw /.test(source.split('catch')[1] ?? ''), '不许把失败抛回调用方（角标是装饰，不该带崩别的）')
})

badgeCase('★ App 里那条 effect：求和成一个数，而不是订阅整个 rooms', () => {
  const app = read('src/App.tsx')
  assert.match(app, /s\.rooms\.reduce\(\(sum, room\) => sum \+ \(room\.unreadCount \?\? 0\), 0\)/, '在**选择器里**求和')
  assert.match(app, /syncUnreadBadge\(unreadTotal\)/, '变了就报')
  // 订阅整个数组会让 App 跟着每一次 presence/元数据变化重渲染 —— 这条判据就是防它。
  assert.ok(!/useAppStore\(\(s\) => s\.rooms\)/.test(app), '不许订阅整个 rooms 数组')
})

badgeCase('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(completed >= DECLARED_CASES - 1, `完成 ${completed} < 声明 ${DECLARED_CASES - 1}`)
})
