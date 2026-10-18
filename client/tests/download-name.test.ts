// 下载**文件名**（`src/lib/downloadName.ts`）的行为测试。
//
// 运行：cd client && npm test
//
// 这一刀要钉住的判据两条：**名字是不可信输入**（消毒）、**重名绝不覆盖**。
// 落点本身不在这里 —— 它由 capability 的作用域管（`$DOWNLOAD/**`，见 `capabilities/default.json`）。
//
// ⚠ 这几条原本写在壳里（`app/src-tauri/src/downloads.rs` 的内联用例）。落盘机制最终走的是
//   fs 插件（Tauri 的下载钩子在本壳里不触发，账见 `app/README.md` §3.1）⇒ 名字这一格
//   跟着落盘那一格搬到页面这一侧，用例一并搬过来（那边模块已删）。

import assert from 'node:assert/strict'
import test from 'node:test'

import { nextFileName, sanitizeFileName } from '../src/lib/downloadName.ts'

/** 门禁不许空跑（与 `render-failure.test.ts` 同款纪律）：最后一条断言本文件真的执行了用例。 */
let completed = 0
/** 总数**含最后那条**（与 `render-failure.test.ts` 同口径）。 */
const DECLARED_CASES = 6

function nameCase(name: string, fn: () => void | Promise<void>) {
  test(name, async () => {
    await fn()
    completed += 1
  })
}

/** 一组"已经存在"的名字 ⇒ 冒充文件系统。 */
const taken = (...names: string[]) => async (candidate: string) => names.includes(candidate)

nameCase('非法字符与结尾的点、空格都被清掉', () => {
  assert.equal(sanitizeFileName('a<b>c:d"e|f?g*h.txt'), 'a_b_c_d_e_f_g_h.txt')
  assert.equal(sanitizeFileName('报表.xlsx.'), '报表.xlsx')
  assert.equal(sanitizeFileName('名字带空格 '), '名字带空格')
  assert.equal(sanitizeFileName('控\u0000制\u001f符.txt'), '控_制_符.txt')
})

nameCase('★ 带路径的名字只留最后一段（这是"名字"不是"路径"）', () => {
  assert.equal(sanitizeFileName('../../evil.exe'), 'evil.exe')
  assert.equal(sanitizeFileName('..\\..\\evil.exe'), 'evil.exe')
  assert.equal(sanitizeFileName('C:\\Windows\\System32\\evil.dll'), 'evil.dll')
  assert.equal(sanitizeFileName('/etc/passwd'), 'passwd')
})

nameCase('空名与保留名仍要落出一个打得开的名字', () => {
  assert.equal(sanitizeFileName('...'), '附件')
  assert.equal(sanitizeFileName(''), '附件')
  assert.equal(sanitizeFileName('NUL'), '_NUL')
  assert.equal(sanitizeFileName('com1.txt'), '_com1.txt')
  // 只是**以**保留名开头不算保留名（`COM10` / `console.log` 都是正常名字）。
  assert.equal(sanitizeFileName('console.log'), 'console.log')
  assert.equal(sanitizeFileName('COM10.txt'), 'COM10.txt')
})

nameCase('★ 重名加 ` (N)`，**绝不覆盖**', async () => {
  assert.equal(await nextFileName('报表.xlsx', taken()), '报表.xlsx')
  assert.equal(
    await nextFileName('报表.xlsx', taken('报表.xlsx')),
    '报表 (1).xlsx',
    '第一格被占 ⇒ 加 (1)',
  )
  assert.equal(
    await nextFileName('报表.xlsx', taken('报表.xlsx', '报表 (1).xlsx', '报表 (2).xlsx')),
    '报表 (3).xlsx',
  )
  // 没有扩展名的那种也要能加后缀。
  assert.equal(await nextFileName('README', taken('README')), 'README (1)')
})

nameCase('限长按**字节**算：不切多字节字符、保住扩展名', async () => {
  const long = `${'长'.repeat(200)}.webm`
  const name = await nextFileName(long, taken())
  assert.ok(new TextEncoder().encode(name).length <= 180, `名字没限住：${name.length} 字`)
  assert.ok(name.endsWith('.webm'), `扩展名丢了：${name}`)
  assert.ok(name.startsWith('长'), `把多字节字符切坏了：${name}`)
  assert.ok(!name.includes('\uFFFD'), `出现了替换字符：${name}`)
})

nameCase('门禁不许空跑：本文件真的执行了用例', () => {
  // 减一：本条自己也是用例。
  assert.ok(completed >= DECLARED_CASES - 1, `完成 ${completed} < 声明 ${DECLARED_CASES - 1}`)
})
