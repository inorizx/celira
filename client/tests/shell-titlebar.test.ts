// **壳自己的标题栏**的判据（2026-10-17 主人："为啥别人的 app 是类似自己的样式主题那种，
// 我们这个是系统默认的吗" —— 随后贴了一张网易云：品牌在左、窗口按钮也是自己的样式）。
//
// 这一条为什么值得钉：它是**窗口行为**的唯一入口 —— 判据松一格，坏的是"窗口拖不动 /
// 关不掉 / 关错了路"这种一眼就废的东西，而它们在浏览器里（`verify:ui` 跑的那条路）
// **一条都量不到**（浏览器里这个组件返回 null）。所以按源码判，与 `no-silent-red-path`
// 同一口径。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'

const read = (relative: string) =>
  readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8')

const TITLE_BAR = read('../src/components/celira/TitleBar.tsx')
const APP_SHELL = read('../src/components/celira/AppShell.tsx')
const TAURI_CONF = read('../../app/src-tauri/tauri.conf.json')

test('★ 标题栏：只在壳里渲染（浏览器里为 null）', () => {
  assert.match(
    TITLE_BAR,
    /isTauriRuntime\(\)/,
    '在场判据必须是 `isTauriRuntime`（壳的内部注入），不是平台名、不是构建变量',
  )
  assert.match(
    TITLE_BAR,
    /if \(!inShell\) return null/,
    '★ 浏览器里必须返回 null —— web 版的布局一行都不该变（同一份产物两个环境）',
  )
})

test('★ 标题栏：拖动区属性不许丢（丢了窗口就拖不动）', () => {
  // 属性**不继承** ⇒ 空白处与品牌那一块都得自己带（少一个就有一块拖不动）。
  const dragRegions = TITLE_BAR.match(/data-tauri-drag-region/g) ?? []
  assert.ok(
    dragRegions.length >= 3,
    `拖动区只有 ${dragRegions.length} 处：外层、品牌那一块、中间空白都要带（属性不继承）`,
  )
})

test('★ 标题栏：三个按钮走 Tauri 的窗口动作，关窗与系统那条路同一条', () => {
  assert.match(TITLE_BAR, /win\.minimize\(\)/, '最小化')
  assert.match(TITLE_BAR, /win\.toggleMaximize\(\)/, '最大化/还原')
  assert.match(
    TITLE_BAR,
    /win\.close\(\)/,
    '★ 关闭必须走 `close()`：壳里 `CloseRequested` 是"收进托盘"（lib.rs 里 prevent_close + hide）' +
      '—— 自己写隐藏逻辑会让两条路分叉',
  )
  assert.ok(
    !/win\.hide\(\)/.test(TITLE_BAR),
    '不许在标题栏里自己 hide：那会绕过壳的关窗语义',
  )
  // 无障碍名：最大/还原那一颗是**三元**（图标与名字都随状态变），所以按代码判，不按字面量判。
  assert.match(TITLE_BAR, /aria-label="最小化"/, '最小化要有无障碍名')
  assert.match(TITLE_BAR, /aria-label="关闭"/, '关闭要有无障碍名')
  assert.match(
    TITLE_BAR,
    /aria-label=\{maximized \? '还原' : '最大化'\}/,
    '最大/还原那一颗的名字要随状态变（读数是 isMaximized）',
  )
})

test('★ 壳：窗口无边框，且标题栏挂在根布局最上面', () => {
  assert.match(TAURI_CONF, /"decorations": false/, '窗口无边框（自绘标题栏的前提）')
  // ⚠ 这里**没有** `theme`：这一版窗口配置不收它（`theme: "dark"` 被 schema 拒了，实测），
  //   而应用的深色是自己的（`<html class="dark">` + 令牌），不靠窗口这一格。
  assert.match(APP_SHELL, /<TitleBar \/>/, '根布局要挂上标题栏')
  // 顺序判据：标题栏必须在导航/内容**之前**（它是窗口最上面那一条）。
  const barAt = APP_SHELL.indexOf('<TitleBar />')
  const navAt = APP_SHELL.indexOf('aria-label="主导航"')
  assert.ok(barAt > 0 && navAt > barAt, '标题栏要排在主导航之前（在最上面）')
  assert.ok(
    !APP_SHELL.includes('<CeliraLogo'),
    '★ 导航栏里不再放标记（标题栏已经有品牌 —— 同一个东西画两遍）',
  )
})

/**
 * ★★ **窗口动作的权限**（2026-10-17 真机当场撞到："好像不能拖动"）。
 *
 * `data-tauri-drag-region` 与三颗按钮**都不是 CSS 能办的事** —— 它们走 IPC
 * （`startDragging` / `minimize` / `toggleMaximize` / `close` / `isMaximized`），
 * 而 IPC 在 Tauri v2 里受 capabilities 的 ACL 管 ⇒ **不放行就静默没反应** ✗
 * （不报错、也不动，最难查的那种）。
 *
 * 判据：这五条**显式**在册；且**不许**图省事放 `core:window:default`
 * （那会把 `set-position`/`set-size`/`set-always-on-top` 一起给出去 ✗ —— 最小权限）。
 */
test('★★ 自绘标题栏要的五条窗口权限必须在册（少一条就"拖不动/点不动"）', () => {
  // ⚠ 判**权限数组本身**，不判整份文本：这一段的说明里就写着那个名字，
  //   按文本判会被自己的话绊倒（本文件已经踩过一次同类坑）。
  const caps = JSON.parse(read('../../app/src-tauri/capabilities/default.json')) as {
    permissions: (string | { identifier: string })[]
  }
  const granted = caps.permissions.map((each) =>
    typeof each === 'string' ? each : each.identifier,
  )
  for (const needed of [
    'core:window:allow-start-dragging',
    'core:window:allow-minimize',
    'core:window:allow-toggle-maximize',
    'core:window:allow-is-maximized',
    'core:window:allow-close',
  ]) {
    assert.ok(
      granted.includes(needed),
      `★ capabilities 里少了 ${needed} —— 自绘标题栏会静默失效（拖动/按钮没反应）`,
    )
  }
  assert.ok(
    !granted.includes('core:window:default'),
    '★ 不许放 `core:window:default`：它会连 set-position / set-size / set-always-on-top 一起给出去',
  )
})
