// **壳自己的标题栏**的判据（2026-10-17 主人："为啥别人的 app 是类似自己的样式主题那种，
// 我们这个是系统默认的吗" —— 随后贴了一张网易云：品牌在左、窗口按钮也是自己的样式）。
//
// 这一条为什么值得钉：它是**窗口行为**的唯一入口 —— 判据松一格，坏的是"窗口拖不动 /
// 关不掉 / 关错了路"这种一眼就废的东西，而它们在浏览器里（`verify:ui` 跑的那条路）
// **一条都量不到**（浏览器里这个组件返回 null）。所以按源码判，与 `no-silent-red-path`
// 同一口径。
//
// ★ **2026-10-20 下午：栏搬进了 `AppFrame`（路由根）。** 同一天两次真机抱怨（"这屏没有
// 关闭按钮" / "提示居然在关闭按钮上面"）是同一个病：**按屏补栏**。现在判据从"数外壳"
// 换成"唯一写者 + 顺序 + 高度只有一个来源"，见下面那三条。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const read = (relative: string) =>
  readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8')

const TITLE_BAR = read('../src/components/celira/TitleBar.tsx')
const APP_SHELL = read('../src/components/celira/AppShell.tsx')
const APP_FRAME = read('../src/components/celira/AppFrame.tsx')
const APP = read('../src/App.tsx')
const TAURI_CONF = read('../../app/src-tauri/tauri.conf.json')

const SRC = fileURLToPath(new URL('../src', import.meta.url))

/** `src/` 下全部 `.tsx`（判"谁渲染了窗口栏 / 谁自己撑了窗口"只能按文件扫）。 */
function tsxFiles(dir = SRC): string[] {
  const out: string[] = []
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name)
    if (entry.isDirectory()) out.push(...tsxFiles(full))
    else if (entry.name.endsWith('.tsx')) out.push(full)
  }
  return out
}

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

test('★ 壳：窗口无边框，且窗口栏只有一层（挂在路由根的 `AppFrame` 上）', () => {
  assert.match(TAURI_CONF, /"decorations": false/, '窗口无边框（自绘标题栏的前提）')
  // ⚠ 这里**没有** `theme`：这一版窗口配置不收它（`theme: "dark"` 被 schema 拒了，实测），
  //   而应用的深色是自己的（`<html class="dark">` + 令牌），不靠窗口这一格。
  assert.match(APP_FRAME, /<TitleBar \/>/, 'AppFrame 里要真的渲染标题栏')
  assert.ok(
    !APP_SHELL.includes('<TitleBar'),
    '★ `AppShell` 不许再画一条：两条栏就是两个写者（2026-10-20 之前正是"按屏补栏"）',
  )
  // ★ 唯一写者：全仓只有 `AppFrame.tsx` 渲染窗口栏。第二个文件一出现，就是又一条栏。
  const barWriters = tsxFiles()
    .filter((file) => /<TitleBar\b/.test(readFileSync(file, 'utf8')))
    .map((file) => file.slice(SRC.length + 1))
  assert.deepEqual(
    barWriters,
    [join('components', 'celira', 'AppFrame.tsx')],
    `渲染窗口栏的文件只该有一个（AppFrame）；现在有：${barWriters.join(' / ') || '（没有）'}`,
  )
  assert.match(APP, /<AppFrame\b/, '★ 路由根必须经过 `AppFrame` —— 不然那一屏没有窗口按钮')
  // 顺序判据：在框里，栏永远是第一件事（内容之前）。
  assert.ok(
    APP_FRAME.indexOf('<TitleBar />') < APP_FRAME.indexOf('{children}'),
    '标题栏要排在内容之前（它是窗口最上面那一条）',
  )
  assert.ok(
    !APP_SHELL.includes('<CeliraLogo'),
    '★ 导航栏里不再放标记（标题栏已经有品牌 —— 同一个东西画两遍）',
  )
})

/**
 * ★★ **提示条不许长在关闭按钮上面**（2026-10-20 主人："你觉得这个提示的位置说得过去吗。
 * 居然能在关闭窗口的上面的那种"）。
 *
 * 病：换世界那条提示排在 `AppShell` **外面**，而窗口是无边框自绘的 ⇒ 它成了窗口最上面
 * 那一条，把最小化/最大化/关闭挤到第二行；而且 `h-screen` 的壳 ＋ 33px 的条 ＝ 整页多一根
 * 滚动条（真机截图里看得见）。
 *
 * 判据：**栏 → 提示条 → 内容** 这个顺序只能由 `AppFrame` 写；页面根不再自己撑窗口。
 */
test('★★ 窗口最上面那一条只归窗口：提示条排在标题栏之后、内容之前', () => {
  const barAt = APP_FRAME.indexOf('<TitleBar />')
  const noticeAt = APP_FRAME.indexOf('{notice}')
  const slotAt = APP_FRAME.indexOf('{children}')
  assert.ok(barAt >= 0, '框里要有标题栏')
  assert.ok(
    noticeAt > barAt,
    '★ 提示条必须在标题栏**之后**：这一趟旅程的状态属于内容，不属于窗口',
  )
  assert.ok(slotAt > noticeAt, '内容排在提示条之后')
  // 高度：框自己撑满窗口，内容那格 `min-h-0 flex-1` ⇒ 多出来一条时是**页面缩**，不是整页溢出。
  assert.match(APP_FRAME, /className="flex h-screen flex-col/, '框要撑满窗口')
  assert.match(
    APP_FRAME,
    /className="flex min-h-0 flex-1 flex-col"/,
    '★ 内容那一格必须 `min-h-0 flex-1`：少了它，提示条一出现就把内容顶出窗口（那根滚动条就是这么来的）',
  )
})

/**
 * ★ **屏幕根不再自己撑窗口**：`h-screen` / `min-h-screen` 只准出现在 `AppFrame`。
 *
 * 判据一句话：高度只有一个来源。页面自己写 `min-h-screen`，一旦框里多出任何一条
 * （提示条、以后的别的条）就是 100vh + 那一条的高度 ⇒ 整页滚动条；写 `h-screen` 则会把
 * 框给的高度整个忽略掉。这条是上面那个 bug 的**机械复现条件**，所以钉在源码上。
 */
test('★ 只有 `AppFrame` 可以写 `h-screen` / `min-h-screen`（高度只有一个来源）', () => {
  const offenders: string[] = []
  for (const file of tsxFiles()) {
    if (file === join(SRC, 'components', 'celira', 'AppFrame.tsx')) continue
    const text = readFileSync(file, 'utf8')
    // 只看 `className="..."` 里的字面量：注释里写着这些词不算（本仓注释天天在讲历史）。
    for (const match of text.matchAll(/className="([^"]*)"/g)) {
      if (/\b(min-h-screen|h-screen)\b/.test(match[1])) {
        offenders.push(`${file.slice(SRC.length + 1)}: className="${match[1]}"`)
      }
    }
  }
  assert.deepEqual(offenders, [], `窗口高度只能由 AppFrame 给，这些地方自己撑了：\n${offenders.join('\n')}`)
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

/**
 * ★ **每一屏都带窗口按钮，而这件事不再靠"数外壳"**（2026-10-20）。
 *
 * 上一版（上午）的写法是「几屏就要几个 `FirstScreen` 外壳」—— 那是**按屏补栏**：
 * 判据靠数数成立，漏一屏就没按钮；而栏一旦有两个写者，就出了下午那件事
 * （提示条长在关闭按钮上面）。现在栏挂在路由根的 `AppFrame` 上 ⇒ 这条判据变成
 * **"根上那一层在"**：只要 `RootLayout` 的出口是 `AppFrame`，任何一屏都在它里面。
 *
 * 反例留着：`RootLayout` 里再出现按屏的 `return <FirstScreen>` / 任何 `<TitleBar />`，
 * 就是又长出第二个写者 —— 上面那条 `!APP.includes('FirstScreen')` 与
 * `!APP_SHELL.includes('<TitleBar')` 各自钉着一半。
 */
test('★ 屏幕不必各自带栏：入口那几屏都在 AppFrame 里面', () => {
  // `RootLayout` 只有一个出口，且那个出口是框 —— 于是"漏一屏"这种失败模式不可能发生。
  const rootBody = APP.slice(APP.indexOf('function RootLayout('))
  assert.match(rootBody, /<AppFrame\b/, 'RootLayout 的出口必须是 AppFrame')
  const frames = (APP.match(/<AppFrame\b/g) ?? []).length
  assert.equal(frames, 1, `App.tsx 里有 ${frames} 个 AppFrame —— 只该有一个（多半在 RootLayout）`)
  // 第一屏那张卡仍然在（它们是被框住的内容，不是自己撑窗口的外壳）。
  for (const screen of ['StartupLoading', 'NameYourselfCard', 'LoginGate']) {
    assert.match(APP, new RegExp(`<${screen}\\b`), `第一屏少了一张：${screen}`)
  }
})
