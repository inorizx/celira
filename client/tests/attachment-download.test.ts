// 下载**收场**（`src/lib/downloadOutcome.ts`）的行为测试。
//
// 运行：cd client && npm test
//
// 这一刀要钉住的判据一条：**"什么都没说"、"说了存哪了"、"说了失败" 是三件事，不许互相冒充。**
// ★ 最要紧的是负向那条：浏览器形态**不许**替浏览器宣布"已保存到某处"（用户可能当场取消了）。

import assert from 'node:assert/strict'
import test from 'node:test'

import {
  directoryNameOf,
  downloadOutcomeText,
  type DownloadOutcome,
} from '../src/lib/downloadOutcome.ts'

/** 门禁不许空跑（与 `render-failure.test.ts` 同款纪律）：最后一条断言本文件真的执行了用例。 */
let completed = 0
/** 总数**含最后那条**（与 `render-failure.test.ts` 同口径）。 */
const DECLARED_CASES = 5

function outcomeCase(name: string, fn: () => void) {
  test(name, () => {
    fn()
    completed += 1
  })
}

outcomeCase('目录名认得 Windows 与 POSIX 两种写法', () => {
  assert.equal(directoryNameOf('C:\\Users\\x\\Downloads\\a.txt'), 'Downloads')
  assert.equal(directoryNameOf('/home/x/Downloads/a.txt'), 'Downloads')
  assert.equal(directoryNameOf('C:/Users/x/Downloads/a.txt'), 'Downloads')
})

outcomeCase('★ 认不出就说认不出（不许编一个位置）', () => {
  assert.equal(directoryNameOf('a.txt'), null)
  assert.equal(directoryNameOf(''), null)
  assert.equal(directoryNameOf('C:\\'), null)
})

outcomeCase('★ 浏览器形态的成功 ⇒ **什么都不说**（浏览器自己有下载 UI，页面不许替它宣布）', () => {
  // `{ok:true, readout:null}` 就是 `downloadAttachment` 在浏览器里回的那一档。
  assert.equal(downloadOutcomeText({ ok: true, readout: null }), null)
})

outcomeCase('壳里成功 ⇒ 说得出"已保存到 <目录>"；失败 ⇒ 说那句人话', () => {
  const saved: DownloadOutcome = { ok: true, readout: '已保存到 Downloads' }
  assert.equal(downloadOutcomeText(saved), '已保存到 Downloads')
  const failed: DownloadOutcome = { ok: false, text: '附件下载失败，请重试' }
  assert.equal(downloadOutcomeText(failed), '附件下载失败，请重试')
  assert.notEqual(downloadOutcomeText(failed), downloadOutcomeText(saved), '两档不许说成一句')
})

outcomeCase('门禁不许空跑：本文件真的执行了用例', () => {
  // 减一：本条自己也是用例。
  assert.ok(completed >= DECLARED_CASES - 1, `完成 ${completed} < 声明 ${DECLARED_CASES - 1}`)
})
