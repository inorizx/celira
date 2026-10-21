// 「设置」这扇门能不能回去（2026-10-21）。
//
// 病根不是"少写一个返回"，是**两个壳**：`/settings` 与 `/app` 在路由表里是并列的
// （`App.tsx`），点进设置那条图标栏就没了；而「返回」写死 `/app/profile`
// ⇒ 从某间房进设置，回来落在「我的」，房间要再点两下 —— 一扇单向门。
//
// ⚠ **这里守的是接线形状**：谁把来路交给设置页、那条 `replace` 重定向有没有转发。
// 它**证明不了**运行期真的回得去（那是另一回事，见下面最后一条注释）——
// 真机验证：起 dev server，从 `/app/chat/<room>` 点「设置」再点「返回」，看是否落回那间房。

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const read = (relative: string) =>
  readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8')

/**
 * 去掉注释再扫。⚠ 与 `nav-groups.test.ts` 那条同一脚：注释里为了说清病根**引用了旧代码**
 * （`startsWith('/settings')`、`navigate('/app/profile')`）⇒ 按裸子串扫会把"引用"判成"违规"。
 * 判据找的是**代码**里还在不在。
 */
const code = (source: string) =>
  source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '')

test('图标栏的「设置」不再声称自己是当前页（那条判断永远为假）', () => {
  const shell = code(read('../src/components/celira/AppShell.tsx'))
  assert.ok(
    !shell.includes("startsWith('/settings')"),
    "`AppShell` 里又出现了 `startsWith('/settings')` —— `/settings` 不在 `/app` 之下，" +
      '这条激活态永远不成立，只会让读代码的人以为设置页长在图标栏里',
  )
  // 正对照：来路得有人交出去，否则删干净了就变成"既没有激活态、也回不去"。
  assert.ok(
    shell.includes('state: { from: location.pathname }'),
    '图标栏点「设置」时必须把来路交出去（`state.from`），否则返回只能靠猜',
  )
})

test('设置页的「返回」从来路回来，不写死「我的」', () => {
  const layout = code(read('../src/pages/settings/SettingsLayout.tsx'))
  assert.ok(
    layout.includes("from ?? '/app/profile'"),
    '「返回」必须是「来路 ?? 我的」（深链接/直接打开时缺省落「我的」）',
  )
  assert.ok(
    !layout.includes("navigate('/app/profile')"),
    '「返回」又写死成 `/app/profile` 了 —— 那就是一扇单向门（从房间里进来，回来落在「我的」）',
  )
})

test('桌面端那条 replace 重定向必须把来路转发下去', () => {
  const layout = code(read('../src/pages/settings/SettingsLayout.tsx'))
  // `Navigate` 不带 state：`/settings` → `/settings/general` 这一跳如果不转发，
  // `AppShell` 传的 `from` 正好在这里蒸发（桌面端每次从图标栏进来都走这条路）。
  assert.match(
    layout,
    /<Navigate to="\/settings\/general" replace state=\{location\.state\} \/>/,
    '`SettingsIndex` 的 `replace` 重定向没有转发 `state` —— 桌面端来路会在这里丢掉',
  )
})

test('设置区自己的子导航也带着来路（点两个来回不该丢）', () => {
  const layout = code(read('../src/pages/settings/SettingsLayout.tsx'))
  const carried = layout.match(/state: originState/g)?.length ?? 0
  assert.equal(
    carried,
    2,
    '目录点条目、移动端「返回设置」这两处都要带 `state: originState`（少了任一处，' +
      '在设置区内部走两步再来路就没了）',
  )
})

// ⚠ **这条测试覆盖不到什么**：它只看源码里那几处接线。
//   ① 运行期真回不回得去（真机走一遍）；
//   ② 深链接进来时「我的」这个缺省是否合适（那是产品判断，不是接线）；
//   ③ 浏览器前进/后退与设置区的关系。
