// 壳层导航分区（TASK_52）纯逻辑测试（node --test + 类型剥离，不引入新框架）。
//
// 产品语义（主人 2026-09-18/20 裁决，写进分区语义不许漂移）：
// Celira IM = 通讯产品（世界的事）；Agent 产品 = 用户侧系统（Agent 的事）。
// 一句话：**连线归世界，生活归执行端**（2026-09-30 口号统一：原句 2026-09-18「生活归她」的演化线，勿改「运行」）。
//
// 覆盖：两组的存在与顺序、**下组「Agent」收敛为单一入口**（刀2a：那三个平级入口切的是数据
// 来源不是 Agent，已收进 Agent 的主页）、两端（桌面图标栏 / 移动 TabBar）共享**同一份数组
// 派生**（有且只有一份入口清单，没有平行机制）、以及「我的服务」本刀不预建。

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import {
  NAV_GROUPS,
  flattenNavItems,
  isSecondaryMobileRoute,
  navGroupsFor,
} from '../src/components/celira/navGroups.ts'

/**
 * 去掉注释再扫。⚠ **这一条自己先假红过一次**：新写的注释里为了说清病根**引用了旧代码**
 * （`startsWith('/app/agents')`、`aria-label="返回我的"`）⇒ 按裸子串扫会把"引用"判成"违规"。
 * 判据找的是**代码**里还在不在，注释里提它是合法的（与 `home-status.test.ts` 那条同一脚）。
 */
const code = (source: string) =>
  source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '')

test('「我的」页不再给「运行」捷径（刀2b；2026-09-26 理由修订）：那一区归执行端导航', () => {
  // 为什么要一条机械判据：那个快捷入口曾指向 `/app/runtime`（如今只是重定向到
  // `/app/runtimes`）。「我的」页是**关于我**的地方 —— 机器读数与运行状态归壳层
  // 「执行端」组，不在这里再开第二条路。它不在 `navGroups.ts` 里（本页的捷径不算
  // 壳层导航），所以上面那圈"一级入口"的断言看不住它，得单独钉一条。
  //
  // ⚠ 本条的理由 2026-09-26 修订：原写「运行状态只在**某一个 Agent** 的主页里说」——
  // 那句在 2026-09-25 组织轴刀之后**方向反了**（现在主语是**机器**，运行状态在
  // `/app/runtimes/:entryId`）。**断言照旧成立，改的是理由**：不许**旧地址**回来。
  //
  // ⚠ 判据同日修准：原先是朴素的 `includes('/app/runtime')` —— 而
  // **`/app/runtimes` 正好包含它**（前缀），于是本页任何一次对**新**地址的合法引用
  // （哪怕只在注释里）都会把它判红。改成"旧地址后面**不接 `s`**"，才是它真正要拦的东西。
  const profile = readFileSync(
    fileURLToPath(new URL('../src/pages/ProfilePage.tsx', import.meta.url)),
    'utf8',
  )
  assert.ok(
    !/\/app\/runtime(?!s)/.test(profile),
    '「我的」里不该再有一条通往旧「运行状态」地址的路（那一区归壳层「执行端」组）',
  )
})

test('壳层导航分两组：上组「世界」（IM）、下组「执行端」（Agent 产品）', () => {
  assert.deepEqual(
    NAV_GROUPS.map((g) => g.id),
    ['world', 'agent'],
  )
  assert.deepEqual(
    NAV_GROUPS.map((g) => g.label),
    ['世界', '执行端'],
  )
  assert.deepEqual(
    NAV_GROUPS[0].items.map((i) => i.id),
    ['messages', 'contacts', 'discover'],
  )
  // 下组**两个入口**（2026-09-29 补第二个：`/app/agents`）。
  // ⚠ 2026-09-25 组织轴刀先把它收敛成一个（「我的执行端」），理由是**轴错了**：
  //    那一区读的全是机器级事实（在不在跑、醒过几次、用哪条模型），轴该是**机器**不是 Agent。
  //    那条判断今天仍然成立、也仍然由机器页承担。
  // ⚠ **2026-09-29 变更**（本次）：补回「我的 Agent」，理由是**变的是那一页**——
  //    `/app/agents` 现在是一张明说自己拼两件事的清单（`side: 'home'`：每行 = 世界侧
  //    所有权 + 家侧三格读数），不再冒充"Agent 的运行状态"；而代价是实测的：
  //    它原先**不在任何分区里**，唯一入口是「我的」页那张卡 —— 一级内容拿二级待遇。
  //    **轴没有变**（两个入口都在执行端组、都读家），变的只是"找得到"。
  assert.deepEqual(
    NAV_GROUPS[1].items.map((i) => i.id),
    ['runtimes', 'agents'],
  )
  assert.deepEqual(
    NAV_GROUPS[0].items.map((i) => i.to),
    ['/app/chat', '/app/contacts', '/app/discover'],
  )
  assert.deepEqual(
    NAV_GROUPS[1].items.map((i) => i.to),
    ['/app/runtimes', '/app/agents'],
    // ⚠ 路由**本体一个都没删**：`/app/runtime`、`/app/experience` 仍直达（重定向到执行端页），
    // `/settings/providers` 也没搬（从「模型」房间深链进去）—— 入口搬家，深链接不破。
  )
})

test('两端共享同一数组派生：摊平结果只是两个分区的直接展开，无第二份清单', () => {
  const flat = flattenNavItems()
  assert.equal(flat.length, NAV_GROUPS[0].items.length + NAV_GROUPS[1].items.length)
  // 每个入口都必须来自某个分区的**同一引用**——第二份清单会在这里露馅。
  for (const item of flat) {
    assert.ok(
      NAV_GROUPS.some((g) => g.items.includes(item)),
      `${item.id} 必须来自分区数组本身（不是平行清单）`,
    )
  }
  // id 与目标路由各自唯一：重复入口就是「平行机制」的味道。
  const ids = flat.map((i) => i.id)
  assert.equal(new Set(ids).size, ids.length)
  const tos = flat.map((i) => i.to)
  assert.equal(new Set(tos).size, tos.length)
  // 显式传入分组时走同一条派生（两端渲染调用的是同一个函数）。
  assert.deepEqual(
    flattenNavItems(NAV_GROUPS).map((i) => i.id),
    ids,
  )
})

test('2026-09-25 组织轴刀：下组入口是「我的执行端」，且旧入口不许换个名字回来', () => {
  const flat = flattenNavItems()
  assert.equal(
    flat.find((i) => i.id === 'runtimes')?.to,
    '/app/runtimes',
    '「我的执行端」指向机器列表（点进去才是那一台机器的一页）',
  )
  assert.equal(flat.find((i) => i.id === 'runtimes')?.label, '我的执行端')
  for (const gone of ['providers', 'runtime', 'experience']) {
    assert.ok(
      !flat.some((i) => i.id === gone),
      `${gone} 不该还是「执行端」组的一级入口（内容住进那一台机器的页面里）`,
    )
  }
  // ⚠ 这一栏是**旧名黑名单**：入口换轴之后，任何"Agent 优先"的旧标签都不许回来当一级入口。
  //
  // ⚠ **2026-09-29 从这份名单里移除了 `'我的 Agent'`**（本次变更，理由必须留在这里）：
  //    它原被列在这里，注释写的是"它现在是 `/app/agents` 的页面标题，而**不是**这一组的
  //    入口"。那句注释描述的**正是本次要改的现状**，不是一条独立的理由 —— 于是它成了
  //    "用一个事实挡住那个事实的改变"。移除的依据是：黑名单真正要拦的是**用旧名冒充
  //    已换轴的东西**（`运行状态` = 一个以 Agent 为轴却读机器读数的页），而
  //    「我的 Agent」现在指的是一张**明说自己拼两件事**的清单（`side: 'home'`，
  //    每行 = 世界侧所有权 + 家侧三格读数）—— 名字与页面说的是同一件事，不构成冒充。
  //    改名的绕道**不被允许**（`AGENTS.md`：不得改名绕过真实违规），所以这里只做移除 ＋
  //    记录，不改用别的写法把同一件事偷偷放进来。
  for (const stale of [
    '运行状态',
    'Agent 的经历',
    '模型配置',
    '模型服务',
    'Agent 的状态',
    '家',
  ]) {
    assert.ok(
      !flat.some((i) => i.label === stale),
      `旧名不得残留为一级入口：${stale}`,
    )
  }
})

// 「分区不预建：『我的服务』本刀不进任何一组」那条断言**已随服务层退役删除**
// （2026-09-28）：它钉的是"归属已裁在世界侧、入口却未预建"这桩悬案，而
// `/app/services` 这个路由本身已经不存在了 —— 再留着就是一条恒真的装饰。
// 路由级的存在性由 `route-admission.test.ts`（「已删的 /app/services 不许回来」）负责。

test('每个入口都带图标（移动 TabBar 的图标位以此为据）', () => {
  for (const item of flattenNavItems()) {
    assert.ok(item.icon, `${item.id} 缺图标`)
  }
})

// ── ★ 一级入口不是二级页（2026-10-11 立）───────────────────────────────────────//
// 病根：移动端外壳那条判据原先是 `pathname.startsWith('/app/agents')` —— 于是**清单本身**
// （导航里的一级入口）也被判成二级页 ✗：TabBar 让位，页面还顶着一颗「返回我的」⇒
// 人在一级入口上，却被告知自己在「我的」下面一层。
// ⚠ 讽刺的是这条判据**早就写在本仓**（`navGroups.ts` 里加 `/app/agents` 的理由正是
// "一级内容拿二级待遇，用户在自己的导航里找不到"）—— 判据写了，判据的**反面**留在外壳里。
//
// ⇒ 判据现在**从导航派生**（`isSecondaryMobileRoute`）：一级入口永远不是二级页。
//   下面的断言直接对着 `NAV_GROUPS` 跑 ⇒ **以后再加一个一级入口，也不用记得来改这里**。

test('★ 一级入口**永远**不是二级页（对着导航逐个跑，不写死是哪几条）', () => {
  for (const group of NAV_GROUPS) {
    for (const item of group.items) {
      assert.equal(
        isSecondaryMobileRoute(item.to),
        false,
        `「${item.label}」（${item.to}）是导航里的一级入口，不许被判成二级页`,
      )
    }
  }
  // 负向对照（否则上面那条可能只是"什么都没匹配上"）：一级入口**下面再深一层**才算二级。
  for (const path of [
    '/app/chat/room-1',
    '/app/contacts/agent-7',
    '/app/agents/agent-7',
    // ★ 2026-10-11 补：**某一台机器**与它的模型配置那一页也在这一层（同族的详情页）。
    '/app/runtimes/local',
    '/app/runtimes/local/config',
  ]) {
    assert.equal(isSecondaryMobileRoute(path), true, `${path} 是一级入口下面那一层，应当算二级`)
  }
  // 与"更深一层"无关的地址不许被误判。
  assert.equal(isSecondaryMobileRoute('/app/chat'), false)
  assert.equal(isSecondaryMobileRoute('/settings'), false)
})

test('★ 那条判据只有一处实现，且外壳里不许再留一份（旧写法会静默退化成"清单也是二级"）', () => {
  const shell = code(
    readFileSync(fileURLToPath(new URL('../src/components/celira/AppShell.tsx', import.meta.url)), 'utf8'),
  )
  assert.ok(shell.includes('isSecondaryMobileRoute'), '外壳没有用那份共用判据')
  assert.ok(
    !shell.includes("startsWith('/app/agents')"),
    "外壳里又出现了 `startsWith('/app/agents')` —— 那会把一级入口 `/app/agents` 判成二级页",
  )
})

test('★ 一级入口的头部**不带**返回箭头（二级页那颗照旧在）', () => {
  const listPages = [
    ['我的执行端', '../src/pages/RuntimesPage.tsx'],
    ['我的 Agent', '../src/components/celira/AgentsListPanel.tsx'],
  ] as const
  for (const [label, relative] of listPages) {
    const source = code(readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8'))
    assert.ok(
      !source.includes('aria-label="返回我的"'),
      `「${label}」是一级入口，头部不许再顶着「返回我的」（那等于说它在「我的」下面一层）`,
    )
  }
  // 正对照：**二级页那颗照旧在**（否则上面那条会退化成"把返回都删了"也没人知道"）。
  // ⚠ 它们在**各自的文件**里 —— 一级入口那一份只是不该有「返回我的」，
  //   而 `AgentsListPanel` 里根本没有二级出口（那一支的详情页是 `MyAgentsPage`）。
  const detailArrows = [
    ['../src/pages/RuntimesPage.tsx', '返回我的执行端'],
    ['../src/pages/MyAgentsPage.tsx', '返回我的 Agent'],
  ] as const
  for (const [relative, arrow] of detailArrows) {
    const source = code(readFileSync(fileURLToPath(new URL(relative, import.meta.url)), 'utf8'))
    assert.ok(source.includes(arrow), `二级页的返回出口「${arrow}」不见了（${relative}）`)
  }
})

test('★ 「关于 Celira」指到**关于那一页**，不是设置首页（从前落在「通用」上）', () => {
  const profile = code(
    readFileSync(fileURLToPath(new URL('../src/pages/ProfilePage.tsx', import.meta.url)), 'utf8'),
  )
  const entry = profile.indexOf('关于 Celira')
  assert.ok(entry > 0, '「我的」页里找不到「关于 Celira」那条入口 —— 改名了？这条守卫要跟着改')
  // ⚠ 取**它前面最近的一次 `navigate(`**（不按字数切窗口：窗口会随注释长短失效）。
  const before = profile.slice(0, entry)
  const call = before.lastIndexOf('navigate(')
  assert.ok(call > 0, '「关于 Celira」那条入口前面没有 `navigate(` —— 这条守卫要跟着改')
  assert.ok(
    before.slice(call, call + 80).includes("'/settings/about'"),
    '「关于 Celira」没有指到 `/settings/about` —— 名字说的是哪一页，就去哪一页',
  )
})

test('真实世界下不摆"点进去必然撞门"的入口（2026-10-03 刀）：`发现` 被摘掉，其余不动', () => {
  // 判据不在导航层，在路由表：`demo` 是唯一"真实世界进不去"的侧
  // （`routeRegistry.reachableInRealWorld`）。真机上「发现」挂在一级导航里，
  // 而它 `side: 'demo'` ⇒ 真实用户点进去只有一句"尚未提供" —— 那正是
  // routeRegistry 自己写的那条纪律要拦的东西（"真实用户点得到、点进去却撞门"）。
  const real = navGroupsFor(true)
  assert.deepEqual(
    real.map((g) => g.id),
    ['world', 'agent'],
    '分区本身不因过滤而消失',
  )
  assert.deepEqual(
    real[0].items.map((i) => i.id),
    ['messages', 'contacts'],
    '「世界」组摘掉发现（其余两条照旧）',
  )
  assert.deepEqual(
    real[1].items.map((i) => i.id),
    ['runtimes', 'agents'],
    '「执行端」组一个字都不动',
  )
  // 演示通道（未连接）照旧全摆：那条通道里发现页是**说得出来**的（fixtures 撑着）。
  assert.deepEqual(
    navGroupsFor(false).map((g) => g.items.map((i) => i.id)),
    NAV_GROUPS.map((g) => g.items.map((i) => i.id)),
  )
  // **这是派生不是第二份清单**：世界契约落地后把那条路由的 side 一改，它自动回来。
  // 这条断言就是那把锁 —— 手写的过滤清单（按 id 或按标签）会在这里露馅。
  const synthetic = NAV_GROUPS.map((g) => ({
    ...g,
    items: g.items.map((i) =>
      // 把发现的**目标**换成一个非 demo 侧的地址（合成用例：只动目标，不动别的）。
      i.id === 'discover' ? { ...i, to: '/app/world-discover' } : i,
    ),
  }))
  assert.deepEqual(
    navGroupsFor(true, synthetic)[0].items.map((i) => i.id),
    ['messages', 'contacts', 'discover'],
    '目标路由不再是 demo 侧 ⇒ 它回到导航里（判据跟着路由表走，不跟人走）',
  )
})

// `scripts/verify-ui.mjs`（页面级真机断言）自己带一份期望入口清单——它跑在
// Node 里、走真实浏览器，不能 import 这份带 JSX 依赖的模块。两份清单若不锁在一起，
// 迟早各自漂移（"一条规则只留一个权威位置"）。这条测试就是那把锁：
// **改了 navGroups 而忘了改脚本 ⇒ 这里红**。
//
// ⚠ 2026-10-03 修准：这条锁从前拿 `flattenNavItems()`（**全量**）去对脚本的清单，
// 而脚本跑的是**真实世界**（要一个跑着的 core + 一支 Human token）——真实世界里
// 发现已经被摘掉。⇒ 锁的两头必须量同一件事：脚本的期望 = **真实世界那份派生**。
test('verify-ui 脚本的期望入口与 navGroups 对齐（防两处漂移）', async () => {
  const { readFileSync } = await import('node:fs')
  const { fileURLToPath } = await import('node:url')
  const scriptPath = fileURLToPath(new URL('../scripts/verify-ui.mjs', import.meta.url))
  const source = readFileSync(scriptPath, 'utf8')

  const arrayOf = (name: string): string[] => {
    const match = source.match(new RegExp(`const ${name} = \\[([^\\]]*)\\]`))
    assert.ok(match, `脚本里找不到 ${name}（改过名字？这条测试要跟着改）`)
    return [...match[1].matchAll(/'([^']+)'/g)].map((m) => m[1])
  }

  assert.deepEqual(
    arrayOf('EXPECTED_NAV'),
    flattenNavItems(navGroupsFor(true)).map((item) => item.label),
    '脚本的 EXPECTED_NAV 与「真实世界」那份入口清单不一致',
  )
  assert.deepEqual(
    arrayOf('EXPECTED_GROUPS'),
    navGroupsFor(true).map((group) => group.label),
    '脚本的 EXPECTED_GROUPS 与 navGroups 的分区标签不一致',
  )
})
