// 多选的纯逻辑（`src/lib/messageSelection.ts`）的行为测试。
//
// 运行：cd client && npm test
//
// 这一刀要钉住的判据一条：**批量动作按时间线走，不按你点的先后**。
// ★ 那是一条"错了也没人当场发现"的判据 —— 按点击顺序转发出去，对面读到的就是一段
//   被打乱的对话，所以必须有用例钉着。

import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import test from 'node:test'

import {
  copyTextOf,
  isSelected,
  selectedInOrder,
  toggleSelected,
} from '../src/lib/messageSelection.ts'
import type { Message } from '../src/api/domain.ts'

const REPO = path.resolve(import.meta.dirname, '..')
const read = (relative: string) => fs.readFileSync(path.join(REPO, relative), 'utf8')

/** 门禁不许空跑（与 `render-failure.test.ts` 同款纪律）：最后一条断言本文件真的执行了用例。 */
let completed = 0
/** 总数**含最后那条**（与 `render-failure.test.ts` 同口径）。 */
const DECLARED_CASES = 11

function selectionCase(name: string, fn: () => void) {
  test(name, () => {
    fn()
    completed += 1
  })
}

const message = (id: string, body = `第 ${id} 条`): Message =>
  ({ id, roomId: 'r1', authorId: 'a1', body, day: '今天', sentAt: '刚刚' }) as Message

/** 时间线：1 → 2 → 3（id 就是顺序）。 */
const timeline = [message('1'), message('2'), message('3')]

selectionCase('点一下：没选 ⇒ 选上；已选 ⇒ 取消（且不动原数组）', () => {
  const before: string[] = []
  const one = toggleSelected(before, '1')
  assert.deepEqual(one, ['1'])
  assert.deepEqual(before, [], '原数组不许被改（调用方拿它当 React state）')
  assert.deepEqual(toggleSelected(one, '1'), [], '再点一下就是取消')
  assert.deepEqual(toggleSelected(one, ''), ['1'], '空 id 不入集合')
})

selectionCase('判据只有一处：`isSelected`', () => {
  assert.equal(isSelected(['1'], '1'), true)
  assert.equal(isSelected(['1'], '2'), false)
  assert.equal(isSelected(['1'], ''), false)
})

selectionCase('★ 取出来的顺序是**时间线**，不是你点的先后（先点 3、再点 1）', () => {
  const clicked = toggleSelected(toggleSelected([], '3'), '1')
  assert.deepEqual(clicked, ['3', '1'], '（选择状态本身记的是点击顺序，那没关系）')
  assert.deepEqual(
    selectedInOrder(timeline, clicked).map((each) => each.id),
    ['1', '3'],
    '★ 真要用这批消息时必须回时间线里取：1 在前、3 在后',
  )
})

selectionCase('时间线里没有的 id 自然丢掉（不报错、也不凭空造一条）', () => {
  assert.deepEqual(selectedInOrder(timeline, ['2', '已经不在时间线里了']).map((each) => each.id), ['2'])
  assert.deepEqual(selectedInOrder(timeline, []), [])
})

selectionCase('★ 复制的文本也按时间线，且**跳过空正文**（纯附件消息没有正文）', () => {
  const withEmpty = [message('1', '第一句'), message('2', ''), message('3', '第三句')]
  assert.equal(copyTextOf(selectedInOrder(withEmpty, ['3', '1', '2'])), '第一句\n第三句')
  assert.equal(copyTextOf([]), '', '什么都没选时是空串')
  assert.equal(copyTextOf([message('9', '')]), '', '全是没有正文的那几条 ⇒ 空串（不是一串换行）')
})

selectionCase('单条也走同一条路（不给自己留一条"特例分支"）', () => {
  assert.deepEqual(selectedInOrder(timeline, ['2']).map((each) => each.id), ['2'])
  assert.equal(copyTextOf(selectedInOrder(timeline, ['2'])), '第 2 条')
})

// ── 接线守卫：纯逻辑对了不等于**页面上真的用了它** ─────────────────────────────

selectionCase('★ 会话页那三个批量动作都从 `selectedInOrder` 取（不许自己 map 一遍）', () => {
  const page = read('src/pages/ChatPage.tsx')
  assert.match(page, /const selectedMessages = selectedInOrder\(roomMessages, selected\)/, '取法只有这一处')
  assert.match(page, /setForwardTargets\(selectedMessages\)/, '批量转发用它')
  assert.match(page, /copyTextOf\(selectedMessages\)/, '批量复制用它')
  assert.match(page, /selectedMessages\.map\(\(each\) => each\.id\)/, '批量删除用它')
})

selectionCase('★ 多选态把输入区换掉（此刻底下那一格不是"再写一条"）', () => {
  const page = read('src/pages/ChatPage.tsx')
  assert.match(page, /\{selectMode \? \(/, '多选态要分支渲染')
  assert.match(page, /已选 \{selected\.length\} 条/, '要说清选了几条')
  assert.match(page, /删除 · 仅为我/, '批量里那个销毁性动作名字与单条一致')
  assert.match(page, /<MessageInput/, '平时仍然是输入区（不许把它删了）')
})

selectionCase('★ 批量转发交给面板时是**一批**，且失败要说清"前几条已经过去了"', () => {
  const sheet = read('src/components/celira/ForwardSheet.tsx')
  assert.match(sheet, /messages: Message\[\]/, '面板收的是一批（按时间线排好的）')
  assert.match(sheet, /for \(const each of messages\)/, '一条一条转（顺序就是对面读到的顺序）')
  assert.match(sheet, /前 \$\{done\} 条已经转过去了/, '半途失败要把"已经发出去的"如实说出来')
  assert.match(
    sheet,
    /按时间顺序/,
    '多条时要说清"按时间顺序"（用户点的先后不是顺序）',
  )
})

selectionCase('★ 菜单里进多选那一项，与气泡上的勾选是两条路', () => {
  const menu = read('src/components/celira/MessageContextMenu.tsx')
  assert.match(menu, /onSelectMode/, '菜单里要有「选择」')
  assert.match(menu, /selectionMode[\s\S]{0,400}onToggleSelect/, '多选态里点气泡 = 勾选/取消')
  assert.match(menu, /data-selected/, '勾选状态要落在 DOM 上（可测、也能被样式看见）')
})

selectionCase('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(completed >= DECLARED_CASES - 1, `完成 ${completed} < 声明 ${DECLARED_CASES - 1}`)
})
