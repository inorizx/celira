// 整窗拖放的接线守卫（`src/components/celira/MessageInput.tsx`）。
//
// 运行：cd client && npm test
//
// 为什么要守卫：这三条错了在界面上都**看不出来**，而且后果都不是"少个功能"：
//   ① 容器上那条 `onDrop` 忘了删 ⇒ 同一次拖放被接**两遍** ⇒ 文件进两条（用户会以为是自己手抖）；
//   ② 不判 `Files` ⇒ 页面内拖文字之类也被这张网拦下（`preventDefault` 一按，别人的拖拽就废了）；
//   ③ 提示层忘了 `pointer-events-none` ⇒ **它自己把 drop 吃掉**，松手反而没反应。

import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import test from 'node:test'

const REPO = path.resolve(import.meta.dirname, '..')
const SOURCE = fs.readFileSync(path.join(REPO, 'src/components/celira/MessageInput.tsx'), 'utf8')

/** 门禁不许空跑（与 `render-failure.test.ts` 同款纪律）：最后一条断言本文件真的执行了用例。 */
let completed = 0
/** 总数**含最后那条**（与 `render-failure.test.ts` 同口径）。 */
const DECLARED_CASES = 4

function dropCase(name: string, fn: () => void) {
  test(name, () => {
    fn()
    completed += 1
  })
}

dropCase('★ 判据只有一处：容器上不许再有 onDrop / onDragOver（否则一次拖放接两遍）', () => {
  assert.match(SOURCE, /window\.addEventListener\('drop'/, '整窗那条要在')
  assert.ok(
    !/\sonDrop=\{/.test(SOURCE),
    '容器上的 onDrop 必须已删 —— 留着它同一次拖放会被接两遍，文件进两条',
  )
  assert.ok(!/\sonDragOver=\{/.test(SOURCE), '容器上的 onDragOver 同理（它只负责亮那圈 ring）')
})

dropCase('★ 只接"拖的是文件"，而且传不了附件时**完全不拦**', () => {
  assert.match(SOURCE, /includes\('Files'\)/, '要先看 types 里有没有 Files')
  assert.match(
    SOURCE,
    /if \(!takesFiles\(event\) \|\| !uploadable\(\)\) return/,
    '不满足就原样放行（既不 preventDefault，也不亮提示）',
  )
  assert.match(SOURCE, /attachmentInputMode\(getApi\(\)\) !== 'unavailable'/, '判据仍是那一条能力探测')
})

dropCase('★ 提示层必须 pointer-events-none，且文案说的是"加进这条消息"（不是"发送"）', () => {
  assert.match(SOURCE, /pointer-events-none fixed inset-0/, '忘了它 ⇒ 提示层会把 drop 吃掉')
  assert.match(SOURCE, /松开就加到这条消息里/, '拖进来只是加进待发条，还要用户自己按发送')
})

dropCase('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(completed >= DECLARED_CASES - 1, `完成 ${completed} < 声明 ${DECLARED_CASES - 1}`)
})
