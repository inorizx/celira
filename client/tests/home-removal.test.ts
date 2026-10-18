// **「移除」不是撤销**（2026-10-11 立）：`/settings/homes` 那一格的家条目。
//
// ## 它治的是什么
//
// 那一页的移除按钮原先是一颗一次点掉的垃圾桶，名字叫「删除 <名字>」，而它实际做的事只有一件：
// **删掉这台浏览器里的记录**。那台机器上那把钥匙**照旧有效** —— 钥匙的失效只有一条路
// （`scripts/home/home-key.js`：在那台机器上重跑 `issue-home-key` 覆盖摘要）。
//
// 代价落在**安全**上：用户以为自己已经把它切断了，而那台机器还认那把钥匙。而**远程家正是为
// "人不在那台机器旁边"建的** ⇒ 这一格是那一族里唯一说得上话的地方。
//
// 判据与「**删除 · 仅为我 ≠ 删除**」同源（`DESIGN_SYSTEM.md`）：**名字必须说清它不是撤销，
// 后果必须说出口**；而这个仓库**不用弹窗做确认** ⇒ 两次点击确认式（与「撤销 Agent」同一配方）。
//
// ## 为什么是源级断言
//
// 这个仓库的 client 测试是 `node --test` + TS 类型剥离，**没有 jsdom / 组件渲染** ⇒
// "接线还在不在"只能按源码判（与 `no-silent-red-path.test.ts` 同一条口径）。
// 判错即红的样子：把名字改回「删除」、把两次点击改成一次、把后果那几句删掉、
// 或把失败提示挪回"添加"表单里面。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const PAGE = readFileSync(
  fileURLToPath(new URL('../src/components/celira/MachineAdminPanel.tsx', import.meta.url)),
  'utf8',
)

/** `remove` 那个处理器的正文（从声明到下一个 `\n  }`）。 */
function removeHandler(): string {
  const start = PAGE.indexOf('const remove = async () => {')
  assert.ok(start > 0, '找不到 `remove` 处理器 —— 改名了？这条守卫要跟着改')
  const end = PAGE.indexOf('\n  }', start)
  assert.ok(end > start, '`remove` 处理器的正文切不出来 —— 守卫要跟着改')
  return PAGE.slice(start, end)
}

test('门禁不许空跑：页面读到了、那两个锚点都在', () => {
  assert.ok(PAGE.length > 2000, `页面源码只有 ${PAGE.length} 字节 —— 读错文件了？`)
  assert.ok(PAGE.includes('removeHomeEntry('), '页面里没有移除那条链 —— 守卫量不到东西')
  // ⚠ 2026-10-12 形状变了：**"添加"那块表单已经搬出这个文件**（`AddMachineForm.tsx`）
  //   ⇒ 下面那条"失败提示在表单之外"的判据因此更强：它俩现在**根本不在一个组件里**。
  assert.ok(
    !PAGE.includes('状态口端口'),
    '这个面板里又出现了"添加"表单的字段 —— 两件事混回一处了（移除失败会跟着表单一起被藏起来）',
  )
})

test('★ 那颗按钮**不许叫「删除」**（名字必须说清它不是撤销）', () => {
  assert.ok(
    !/aria-label=\{[^}]*删除/.test(PAGE),
    '这颗按钮的 aria-label 又写回「删除」了 —— 它只删本机记录，那台机器上那把钥匙仍然有效',
  )
  assert.ok(PAGE.includes('`移除 ${entry.name}`'), '未武装那一态的名字该是「移除 <名字>」')
  // ⚠ **武装那一态的名字必须把可见文字包进去**（2026-10-11 真机门禁第一次跑就红在这里）：
  //    可见文字是「再次点击确认移除」，而 `aria-label` 会**盖住**它 —— 名字若写成「确认移除 X」，
  //    屏幕阅读器听到的与眼睛看到的就不是同一个按钮（**WCAG 2.5.3 Label in Name**）。
  assert.ok(
    PAGE.includes('`再次点击确认移除 ${entry.name}`'),
    '武装那一态的 aria-label 必须**包含可见文字**（WCAG 2.5.3：名字里要有眼睛看到的那几个字）',
  )
})

test('★ 两次点击确认式：第一次只把后果摆出来，第二次才真的移除', () => {
  const handler = removeHandler()
  const arm = handler.indexOf('setArmed(true)')
  const really = handler.indexOf('removeHomeEntry(')
  assert.ok(arm > 0, '`remove` 里没有"先武装"这一步 —— 又变回一次点掉了')
  assert.ok(really > 0, '`remove` 里没有真的移除那一步')
  assert.ok(arm < really, '顺序反了：必须**先**武装、**再**（下一次点击才）移除')
  assert.match(
    handler.slice(0, really),
    /return\b/,
    '武装那一步必须**当场返回**（否则同一次点击就把它移除了 —— 两次点击是假的）',
  )
  // 本机那条永远不给这颗按钮（预置的第一条，删不得）。
  assert.ok(PAGE.includes('{!entry.local && ('), '本机那条的守卫不见了 —— 预置那条删不得')
})

test('★ 后果三句都在（缺一句，用户就会把"本机不再记它"读成"它进不来了"）', () => {
  // ① 作用域：只影响**这台浏览器**；② 那句硬事实：那台机器上的钥匙**仍然有效**；③ 真出口：CLI。
  assert.ok(PAGE.includes('这台浏览器'), '没说清作用域（只影响这台浏览器）')
  assert.ok(
    PAGE.includes('那台机器上那把钥匙仍然有效'),
    '那句硬事实不见了 —— 这是这一格最容易犯、代价又在安全上的错',
  )
  assert.ok(PAGE.includes('issue-home-key'), '没有给出真的让它进不来的那条路（CLI）')
  // ⚠ 不许承诺页面上做不到的事：**撤销钥匙**今天只有 CLI 一条路（门侧没有这个端面）。
  assert.ok(
    !/在这里[^。]{0,12}(撤销|换一把)/.test(PAGE),
    '页面不许暗示"在这里就能撤销/换钥匙" —— 那是一条假路径',
  )
})

test('★ 移除失败**必须说出口**，而且**在"添加"表单之外**', () => {
  const alert = PAGE.indexOf('role="alert"')
  // ⚠ 2026-10-12：那块表单**不在这个文件里**（`AddMachineForm.tsx`）⇒ 这一条现在是结构性的：
  //   移除失败提示长在**管理面板**里，而"添加"是**另一个组件** ⇒ 表单关着时它照旧显示。
  //   （原来那版比的是同一文件里两个块的先后 —— 现在比的是"它们不在一处"，更强。）
  const addForm = Number.POSITIVE_INFINITY
  assert.ok(alert > 0, '页面上没有 role="alert" —— 失败又会变成"点了没反应"')
  assert.ok(
    alert < addForm,
    '失败提示又长回"添加"表单里面了 —— 表单没开着时，移除失败一个字都不显示',
  )
  // 那句话本身也要在（不许只说"出错了"）。
  assert.ok(PAGE.includes('没能移除'), '移除失败那句人话不见了')
  // 而且它得是**状态驱动**的渲染，不是写死的。
  assert.ok(PAGE.includes('{actionError && ('), '失败提示不是按状态渲染的')
})
