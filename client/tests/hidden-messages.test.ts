// 「删除 · 仅为我」那份本地名单（`src/lib/hiddenMessages.ts`）的行为测试。
//
// 运行：cd client && npm test
//
// 这一刀要钉住的判据两条：**它是本地视图、不是公共事实**（这里只碰一串 id，绝不碰消息），
// 以及**宽容**（读不懂一律归空名单，绝不抛、绝不因为一份坏数据让整间房渲染不出来）。

import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import test from 'node:test'

import {
  addHidden,
  hiddenKey,
  isHidden,
  parseHidden,
  removeHidden,
  serializeHidden,
} from '../src/lib/hiddenMessages.ts'

const REPO = path.resolve(import.meta.dirname, '..')
const read = (relative: string) => fs.readFileSync(path.join(REPO, relative), 'utf8')

/** 门禁不许空跑（与 `render-failure.test.ts` 同款纪律）：最后一条断言本文件真的执行了用例。 */
let completed = 0
/** 总数**含最后那条**（与 `render-failure.test.ts` 同口径）。 */
const DECLARED_CASES = 11

function hiddenCase(name: string, fn: () => void) {
  test(name, () => {
    fn()
    completed += 1
  })
}

hiddenCase('键的形状：世界级 ＋ 房间级（与 roomPrefs 同族）', () => {
  assert.equal(hiddenKey('wk1', 'room-1'), 'celira.world:wk1:hidden:room-1')
  // 世界身份还拿不到时（空串）：键的形状变了，行为不变 —— 仍在同一命名空间下，
  // 所以"登出清整族"那条判据不会漏掉它。
  assert.equal(hiddenKey('', 'room-1'), 'celira.world::hidden:room-1')
})

hiddenCase('★ 宽容：读不懂一律归空名单（绝不抛）', () => {
  assert.deepEqual(parseHidden(null), [])
  assert.deepEqual(parseHidden(''), [])
  assert.deepEqual(parseHidden('不是 JSON'), [])
  assert.deepEqual(parseHidden('{"a":1}'), [])
  assert.deepEqual(parseHidden('["ok", 3, null, ""]'), ['ok'], '只收非空字符串')
})

hiddenCase('解析去重（同一份名单里不该有两条一样的）', () => {
  assert.deepEqual(parseHidden('["a","a","b"]'), ['a', 'b'])
})

hiddenCase('加一条：**幂等**，且不动原数组', () => {
  const before = ['a']
  const after = addHidden(before, 'b')
  assert.deepEqual(after, ['a', 'b'])
  assert.deepEqual(before, ['a'], '原数组不许被改（调用方拿它当 React state）')
  assert.deepEqual(addHidden(after, 'b'), ['a', 'b'], '再加同一条应该原样')
  assert.deepEqual(addHidden(after, ''), ['a', 'b'], '空 id 不入名单')
})

hiddenCase('去掉一条：幂等；去掉不在里面的也不报错', () => {
  assert.deepEqual(removeHidden(['a', 'b'], 'a'), ['b'])
  assert.deepEqual(removeHidden(['a'], 'zzz'), ['a'])
})

hiddenCase('判据只有一处：`isHidden`', () => {
  assert.equal(isHidden(['a'], 'a'), true)
  assert.equal(isHidden(['a'], 'b'), false)
  assert.equal(isHidden(['a'], ''), false, '空 id 永远不算被隐藏')
})

hiddenCase('序列化 ⇄ 解析 一圈回来是同一份（存储那层只做字符串搬运）', () => {
  const list = ['a', 'b']
  assert.deepEqual(parseHidden(serializeHidden(list)), list)
})

hiddenCase('★ 会话页真的按这份名单过滤（判据只有一处：`isHidden`）', () => {
  const page = read('src/pages/ChatPage.tsx')
  assert.match(page, /from '@\/lib\/hiddenMessages'/, '过滤判据要从那个模块来')
  assert.match(page, /roomMessages\.filter\(\(each\) => !isHidden\(hidden, each\.id\)\)/, '过滤就写在这一处')
  assert.match(page, /visibleMessages/, '时间线要用过滤后的那一份')
})

hiddenCase('★ 菜单项与撤回**分得开**：它是「删除 · 仅为我」，且是 destructive', () => {
  const menu = read('src/components/celira/MessageContextMenu.tsx')
  assert.match(menu, /删除 · 仅为我/, '名字里必须说清"仅为我"（否则与撤回读成一件）')
  assert.match(menu, /variant="destructive"[\s\S]{0,120}删除 · 仅为我|删除 · 仅为我[\s\S]{0,40}/, '它是销毁性动作')
  assert.match(menu, /typeof onHide === 'function'/, '不传回调就不出现（Gallery/Mock 保持原样）')
})

hiddenCase('★ 隐藏之后**说得出话**，而且说出真相：只在你这边 ＋ 能撤销 ＋ 存不住要报', () => {
  const page = read('src/pages/ChatPage.tsx')
  assert.match(page, /只在你这边/, '不许让用户以为"大家都看不见了"')
  assert.match(page, /撤销/, '给了撤销（这个仓库不用弹窗做确认）')
  assert.match(page, /没能保存（刷新之后它还会回来）/, '存不住要如实说，不许假装存住了')
  assert.match(page, /role="alert"/, '存不住那条要能被读屏念出来')
  assert.match(page, /都被你隐藏了/, '一条都被隐藏时不许说成「还没有消息」（那是假话）')
})

hiddenCase('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(completed >= DECLARED_CASES - 1, `完成 ${completed} < 声明 ${DECLARED_CASES - 1}`)
})
