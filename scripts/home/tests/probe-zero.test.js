'use strict'
/**
 * `probe-zero.js` 的**判据**用例（真机部分不在这里跑 —— 那要真世界、真宿主、真页面、真模型调用）。
 *
 * 为什么这几条值得单独钉住：这条门禁 2026-10-09 手工跑出来的两个缺陷都长在"**什么时候生效**"
 * 上（配完不重启吃不到 / 差一拍）。判决函数因此必须与进程无关地钉死：三段读数 ⇒ 一个结论，
 * 而且"配之前就回了"那种环境不对的场合**不许**被当成通过。
 *
 * ⚠ **2026-10-10（在场那一刀）之后判决逻辑一个字没改**：那一刀让**一段在场跨多声铃**，而
 * "配完之后的第一段清醒期"＝"她**下一次开新段**"——本探针的场景里她两次发消息之间**不在场**
 * （`配之前` 那一段以 `system:model-failed` 收场），因此第 1 条必然开新段、必然用新装配。
 * 射程与它量不到的那一格写在 `probe-zero.js` 的文件头。
 */

const test = require('node:test')
const assert = require('node:assert/strict')

const { parseArguments, zeroVerdict } = require('../probe-zero')

let cases = 0
function check(name, fn) {
  test(name, () => {
    cases += 1
    fn()
  })
}

check('参数：缺省不保留、90 秒、清单取默认家、档位 max；不认识的一律明确失败', () => {
  assert.deepEqual(parseArguments([]), {
    keep: false,
    timeoutSeconds: 90,
    modelSources: null,
    modelSourceId: null,
    effort: 'max',
  })
  assert.equal(parseArguments(['--keep']).keep, true)
  assert.equal(parseArguments(['--timeout', '30']).timeoutSeconds, 30)
  assert.equal(parseArguments(['--model-sources', 'D:/x.json']).modelSources, 'D:/x.json')
  assert.equal(parseArguments(['--model-source-id', 'ms-2']).modelSourceId, 'ms-2')
  assert.equal(parseArguments(['--effort', 'high']).effort, 'high')
  assert.equal(parseArguments(['--effort', 'none']).effort, null, 'none = 不写这一格')
  // 静默忽略未知参数 = 让人以为门禁按他想的那样跑了。
  assert.throws(() => parseArguments(['--timeout']), /非空取值/)
  assert.throws(() => parseArguments(['--timeout', '0']), /正整数/)
  assert.throws(() => parseArguments(['--timeout', 'abc']), /正整数/)
  assert.throws(() => parseArguments(['--model-sources', '--keep']), /非空取值/)
  assert.throws(() => parseArguments(['--all']), /不认识的参数/)
})

check('★ 判决：配完**第 1 条**就回 = 0（完全兑现）；只有第 2 条回 = 3（差一拍）', () => {
  const full = zeroVerdict({ before: 0, first: 1, second: 1 })
  assert.equal(full.code, 0)
  assert.match(full.reading, /第 1 条就回/)
  // ⚠ 第 1 条回就够了：门铃是合并的，她一次答掉两条时第 2 条那格是 0 —— 那**不许**影响判决。
  assert.equal(zeroVerdict({ before: 0, first: 1, second: 0 }).code, 0, '第 2 条 0 条不影响')

  // 差一拍：这是缺陷的形状，不许被当成通过。
  const lagged = zeroVerdict({ before: 0, first: 0, second: 1 })
  assert.equal(lagged.code, 3)
  assert.match(lagged.reading, /差一拍/)

  // 两段都不回：如实报，不是通过。
  assert.equal(zeroVerdict({ before: 0, first: 0, second: 0 }).code, 3)
})

check('★ 判决：配**之前**她就回了 ⇒ 2（环境不对，这一轮测不出"配置生效"）', () => {
  const wrongEnvironment = zeroVerdict({ before: 1, first: 1, second: 1 })
  assert.equal(wrongEnvironment.code, 2, '这一轮的前提不成立 ⇒ 不许报通过')
  assert.match(wrongEnvironment.reading, /配\*\*之前\*\*她就回了/)
})

check('用例数 > 0（空跑守卫）', () => {
  assert.ok(cases > 0, '这一份必须真的跑过用例')
})
