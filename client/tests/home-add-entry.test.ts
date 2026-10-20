// **「加一台机器」只有一处权威**（2026-10-11 立，2026-10-12 改形状）。
//
// ## 它治的是什么
//
// 机器清单是**一级入口**（`/app/runtimes`）⇒ **正门得能加**：人从清单页点「添加」就该能加，
// 不该被送去另一页再找一遍表单。
//
// ⚠ **2026-10-11 那版是"带意图过去"**（`navigate('/settings/homes?add=1')` + 目标页消费
// `?add`）—— 那是**当时**的合理解（表单那时住在设置里）。**2026-10-12 形状变了**：
// 机器清单**只留一个门** ⇒ 表单**就地开**（清单页那颗「添加」），不再有第二个门、也就不再需要
// 那条意图通道。这一版量的是新形状：
//  1. 清单页那颗「添加」**就地**开表单（不是 navigate 走人）；
//  2. 表单与落盘**只有一处**（`AddMachineForm.tsx` 的 `saveHomeEntry`）—— 清单页的**代码**里
//     不许出现字段名或落盘调用（复制一份权威一定会漂）；
//  3. 旧地址留成**别名**（`/settings/homes` ⇒ `/app/runtimes`）：旧书签照旧到得了。
//
// 判错即红的样子：把表单字段又抄回清单页；把「添加」改回 navigate 走人；
// 或者删了别名（旧链接会落到 404）。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const read = (relative: string) =>
  readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8')

/** 去掉注释再扫（守卫找的是**代码**里的重复与接线；注释里可以提这些词）。 */
const code = (source: string) =>
  source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '')

const LIST = read('../src/pages/RuntimesPage.tsx')
const FORM = read('../src/components/celira/AddMachineForm.tsx')
const APP = read('../src/App.tsx')

test('门禁不许空跑：三处都读到了、那条链还在', () => {
  assert.ok(LIST.length > 5000, `清单页只读到 ${LIST.length} 字节 —— 读错文件了？`)
  assert.ok(FORM.length > 2000, `表单只读到 ${FORM.length} 字节 —— 读错文件了？`)
  assert.ok(FORM.includes('saveHomeEntry('), '表单里没有"加"那条链 —— 这条守卫量不到东西')
})

test('★ 清单页那颗「添加」**就地**开表单（不是 navigate 走人）', () => {
  const listCode = code(LIST)
  assert.ok(
    listCode.includes('setAdding('),
    '清单页那颗「添加」没有就地开表单 —— 人会被送去另一页再找一遍',
  )
  assert.ok(
    !/navigate\(\s*['"`]\/settings\/homes/.test(listCode),
    '清单页又把人 navigate 到 /settings/homes 了 —— 清单只能有一个门（那个门就是这一页）',
  )
  // 正对照：表单确实**挂在这一页上**（否则上面两条可能只是"它没写"）。
  assert.ok(listCode.includes('<AddMachineForm'), '清单页没有挂那块表单 —— 上面那条断言会变成空跑')
})

test('★ 加机器这件事**只有一处权威**：清单页里不许出现表单字段或落盘调用', () => {
  const listCode = code(LIST)
  for (const forbidden of ['saveHomeEntry(', '机器钥匙', '状态口端口', '配置门端口']) {
    assert.ok(
      !listCode.includes(forbidden),
      `清单页的**代码**里出现了「${forbidden}」 —— 加机器那一套被复制成第二份了（两份权威一定会漂）`,
    )
  }
  // 正对照：那一套确实在表单那一个文件里（否则上面的断言是空跑）。
  assert.ok(
    FORM.includes('状态口端口') && FORM.includes('配置门端口') && FORM.includes('机器钥匙'),
    '表单字段不见了 —— 上面那条"清单页不许有"的断言会变成空跑',
  )
})

test('★ 旧地址留成**别名**（`/settings/homes` ⇒ 机器列表）：旧书签照旧到得了', () => {
  const appCode = code(APP)
  assert.ok(
    /path:\s*'homes'[\s\S]{0,120}?<Navigate to="\/app\/runtimes" replace \/>/.test(appCode),
    '别名没了 —— 旧书签（`/settings/homes`）会落到 404',
  )
  // 正对照：设置目录里**不再有**那一项（它退役了；留着就是第二个门又回来了）。
  const layout = code(read('../src/pages/settings/SettingsLayout.tsx'))
  assert.ok(
    !layout.includes("'/settings/homes'"),
    '设置目录里又出现了 /settings/homes —— 同一份清单的第二个门又回来了',
  )
})
