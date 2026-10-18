// **门判据的共用表**（2026-10-12 立）：两个生产者必须同形，而"同形"由**同一张表**保证。
//
// ## 它治的是什么
//
// 家那两个口有两份实现：源码形态 `node home.js doors`（`scripts/home/status-server.js` ＋
// `config-server.js`）与壳自己端（`app/src-tauri/src/home_doors.rs`）。两份**合不成一份**
// （CLI 得在没装 app 的机器上跑、壳得在自己进程里端那两个口）⇒ 判据只能落在**同形**上。
//
// ⚠ 2026-10-11 真撞过一次不同形：**来源那道与路由谁先判**两边反着 ⇒ 一个外域来源打
// `/home-binding`（只收 PUT）在壳那边得 403、在 JS 那边得 **405 ＋ `Allow: PUT`**
// —— 把"这个端点存在、它认哪些动词"告诉了一个不该知道的来源；而**谁答话取决于哪个生产者
// 碰巧占着那两个口**（那天产品形态门禁量的其实是源码形态的门，看着像代码坏了）。
//
// 那天是把 JS 改成与壳同形。但"再同步一次"还会再漂 ⇒ 判据收进**一张表**
// （`scripts/door-cases.mjs`），两个门禁都跑它：`verify-ui.mjs` 量源码形态、
// `verify-ui-app.mjs` 量壳 ⇒ **一边不同形就有一边红**。
//
// 判错即红的样子：把某一条判据只加在一边（另一个门禁不再覆盖它）、
// 或者哪个门禁又自己抄一份用例表（`https://evil.example` 这种字面量会重新出现在脚本里）。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import {
  DOOR_CASES,
  LOCAL_ORIGIN,
  SHELL_ORIGIN,
  doorCasesSayingWrongThing,
  originHeader,
} from '../scripts/door-cases.mjs'

const read = (relative: string) =>
  readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8')

test('门禁不许空跑：表里有那六条，含"来源先于路由"那一条', () => {
  assert.ok(DOOR_CASES.length >= 6, `表里只有 ${DOOR_CASES.length} 条 —— 少了？`)
  // ★ 那一条**必须**在：它是 2026-10-11 真撞出来的不同形（405 vs 403）。
  const ordering = DOOR_CASES.find(
    (item) => item.door === 'config' && item.path === '/home-binding' && item.origin === 'foreign',
  )
  assert.ok(ordering, '表里缺"外域来源打 PUT-only 端点"那一条 —— 那正是撞过的不同形')
  assert.equal(ordering.expect, 403, '它的期望值必须是 403（405 会说明守卫排在路由之后）')
  // 两条 405 的判据要**点名** `Allow`（只比状态码会漏掉"Allow 发给了外域"这类）。
  const allowCase = DOOR_CASES.find((item) => item.expect === 405)
  assert.ok(allowCase?.allow, '405 那条必须同时点名 `Allow`（否则"Allow 发给了不该给的人"看不出来')
})

test('★ 判据本体：期望与实得不同就报出来（正反例都过一遍）', () => {
  // 正例：照期望答 ⇒ 一条都不报。
  const ok = DOOR_CASES.map((item) => ({ case: item, status: item.expect, allow: item.allow ?? null }))
  assert.deepEqual(doorCasesSayingWrongThing(ok), [], '照期望答却被报了错 —— 判据本体有问题')
  // 反例：把那条"来源先于路由"改成 405（正是撞过的形状）⇒ 必须被抓到。
  const wrong = ok.map((row) =>
    row.case.path === '/home-binding' && row.case.origin === 'foreign'
      ? { ...row, status: 405, allow: 'PUT' }
      : row,
  )
  const complaints = doorCasesSayingWrongThing(wrong)
  assert.equal(complaints.length, 1, `应当恰好抓到那一条，实得 ${complaints.length} 条`)
  assert.match(complaints[0]!, /403/, '那句抱怨要说出期望值')
  // 反例之二：状态对了但 `Allow` 漏了 ⇒ 也要抓到。
  const noAllow = ok.map((row) => (row.case.expect === 405 ? { ...row, allow: null } : row))
  assert.equal(doorCasesSayingWrongThing(noAllow).length, 1, '405 那条少了 `Allow` 也要报')
})

test('★ 两个门禁跑的是**同一份表**，谁也没自己抄一份', () => {
  const web = read('../scripts/verify-ui.mjs')
  const app = read('../scripts/verify-ui-app.mjs')
  for (const [name, source] of [
    ['verify-ui.mjs（源码形态）', web],
    ['verify-ui-app.mjs（产品形态）', app],
  ] as const) {
    assert.ok(source.includes("from './door-cases.mjs'"), `${name} 没有引用那份共用表`)
    assert.ok(source.includes('runDoorCases('), `${name} 引了表却没跑它`)
    assert.ok(source.includes('doorCasesSayingWrongThing('), `${name} 跑了表却没比判据`)
    // 抄一份的样子：外域来源那个字面量又出现在脚本里（表里才有它）。
    assert.ok(
      !source.includes('https://evil.example'),
      `${name} 里又出现了外域来源的字面量 —— 说明它自己抄了一份用例表（表只能有一份）`,
    )
  }
})

test('两个形态的"自己的来源"各自点名（壳 `tauri.localhost`／源码形态走回环）', () => {
  assert.equal(originHeader('own', SHELL_ORIGIN), SHELL_ORIGIN)
  assert.equal(originHeader('own', LOCAL_ORIGIN), LOCAL_ORIGIN)
  assert.equal(originHeader('foreign', 'x'), 'https://evil.example')
  assert.equal(originHeader('foreign-scheme', 'x'), 'tauri://evil.example')
  // 源码形态那一份必须是**回环**（`home-key.js` 的 `localOriginAllowed` 认的就是回环）。
  assert.match(LOCAL_ORIGIN, /^http:\/\/(localhost|127\.0\.0\.1)/, '源码形态的 own 必须是回环来源')
})
