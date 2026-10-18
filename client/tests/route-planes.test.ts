// **边界守卫**（2026-10-11 立）：路由声明说的"这一页读哪几个平面"，与它**实际** import 的
// 数据面模块对表。
//
// ## 它治的是什么
//
// `routeRegistry.ts` 文件头那三条判据回答"这一页**归**哪一侧"，`route-admission.test.ts` 保证
// "有人回答过、且回答与实现一致（在不在守卫下）"。但**没有一条守卫能回答"这一页到底读了哪一侧"**
// —— 而那次事故正是这个形状：`/app/agents/:participantId` 曾声明成 `home`，实际**只读 core**
// （声明与事实相反），是**人**读出来的。
//
// 这一条把它变成机械的：声明表的 `planes`（读哪几个平面）＋ `page`（页面模块），顺着页面模块的
// import 闭包对表。
//
// ## 三条断言
//
// 1. **声明的面 ⊇ 实际 import 的面**：world 页不许读家面权威模块，反之亦然；
// 2. **主体那一侧必须在 `planes` 里**（`side` 是 `world` / `home` 时）；
// 3. **声明含 `home` 的页必须真的读到家面** —— 防的正是上面那次事故的形状。
//
// ## 射程（如实登记，不许当成"边界已经全覆盖"）
//
// - **脊柱不展开**：`api/store.ts` 这类两侧共用的机制按设计同时装两个平面（世界的事实与家侧的
//   读数都在里面）⇒ **经脊柱读对面平面的事实，这条守卫量不到**（例：机器页经 `store` 读
//   `ownedAgents` 来给住户起名字）。要机械化得先拆 store，那是另一刀。
// - **只量权威模块的直读**：两张表收的是"存在的全部理由就是某一侧"的模块；一个模块若两侧都用
//   （如 `api/participantDirectory.ts` 的名字投影），它**不进任何一张表** —— 名字标签是两侧
//   都允许的东西（`routeRegistry.ts` 原话：「可以有"上面跑着谁"的名字标签」）。
// - **`demo` 侧不在射程里**：那一侧读的是 fixtures，不属于任何一个平面（判据在 `planeCheckedRoutes`）。
// - **type-only import 不算读**（`import type` / `import { type X }`）：运行期被抹掉，不读东西。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { existsSync, readFileSync } from 'node:fs'
import { posix } from 'node:path'
import { fileURLToPath } from 'node:url'
import { ROUTES, planeCheckedRoutes, type RoutePlane } from '../src/api/routeRegistry.ts'

const SRC = new URL('../src/', import.meta.url)
const APP_SOURCE = readFileSync(fileURLToPath(new URL('../src/App.tsx', import.meta.url)), 'utf8')

/**
 * **家面权威模块**：存在的全部理由就是"那台机器"（状态口 / 配置门 / 家条目 / 家钥匙 / 安家）。
 *
 * 判据与 `routeRegistry.ts` 文件头那条同源：**把 im 服务端换成微信/QQ，这个模块还成立吗？**
 * 不成立（它问的是"这台机器"）⇒ 家面。
 */
const HOME_AUTHORITY: Record<string, string> = {
  'api/homes.ts': '家条目 + 家钥匙（那台机器怎么找、怎么进）',
  'api/homeStatus.ts': '状态口（那台机器在不在跑 / 醒过几次）',
  'api/homeConfig.ts': '模型配置门（那台机器上有哪些钥匙、这一支用哪条）',
  'api/shellLocal.ts': '壳的本机世界命令（起停 / 会话 / loopback 地址）',
  'api/anjia.ts': '接到这台机器上（壳那六步）',
  'api/agentMachine.ts': '归属裁决的人话（它住在哪台）',
  'api/agentHealth.ts': '宿主进程在不在跑（机器侧那一句）',
  'api/agentRowStatus.ts': '每行那三格机器读数',
  'components/celira/use-home-entries.ts': '家条目 + 逐条目读数（取数口）',
  'components/celira/use-home-reading.ts': '单条目的读数（取数口）',
}

/**
 * **世界面权威模块**：**内容与关系**那一族（成员 / 进行态 / 提及 / 邀请 / 凭据 / 绑定 /
 * 世界作用域的会话偏好与内容视图）。
 *
 * ⚠ **不含名字投影**（`api/participantDirectory.ts` / `api/participantProjection.ts`）：
 * 那是"上面跑着谁"的名字标签，机器侧**正当**。
 * ⚠ **不含 `api/uiPolicy.ts`**：它是**能力探测与展示投影**（问的是"这个世界的 API 有没有这一面"），
 * 不读任何一侧的事实 ⇒ 两侧都能用（`roomCandidates` / `chatFeedState` / `relationActionAvailability`
 * 都住在它里面 —— 所以那三个名字**没有**自己的模块）。
 * ⚠ **不含 `api/core.ts`**：世界的传输层，而页面**不经它直读**（都走 `store`）—— 把它算进来
 * 等于把整棵树判成世界面。
 */
const WORLD_AUTHORITY: Record<string, string> = {
  'api/roomMembers.ts': '房间成员（关系）',
  'api/roomLive.ts': '主体进行态（在不在 / 正在写）',
  'api/mentionTargets.ts': '提及目标（关系）',
  'api/hiddenMessages.ts': '本机隐藏名单（世界作用域 + 房间级的内容视图）',
  'api/invite.ts': '兑换邀请（进这个世界）',
  'api/credentials.ts': '后端凭据（世界侧事实）',
  'api/agentBindings.ts': '后端绑定（世界侧事实）',
  'api/roomPrefs.ts': '置顶 / 免打扰（世界作用域的会话偏好）',
  'api/presentation.ts': '群内呈现的写路径（关系）',
  'api/profile.ts': 'Agent 公开资料的写路径（世界侧事实）',
}

/**
 * **脊柱**：两侧共用的机制 —— **记录但不展开**（它们按设计同时装两个平面）。
 *
 * 名单**故意只有两条**：每多一条就多一个"量不到"的角落。所以判据是"它是不是所有页面都从它
 * 身上过" —— 是 ⇒ 脊柱；不是 ⇒ 让它照常被走一遍（走出来的东西才算数）。
 */
const SPINE: Record<string, string> = {
  'api/store.ts': '两侧共用的状态层（世界的事实与家侧的读数都在里面）',
  'api/core.ts': '世界的传输层（页面经 store 用它，不直读）',
}

// ---------------------------------------------------------------------------
// 静态解析：值 import（type-only 不算）+ 闭包
// ---------------------------------------------------------------------------

/** `import <clause> from '<spec>'` 里的子句是不是**值** import（默认/命名空间/非 type 具名）。 */
function hasValueBinding(clause: string): boolean {
  const trimmed = clause.trim()
  if (/^type\s/.test(trimmed)) return false // import type { X } from …
  const brace = /\{([\s\S]*)\}/.exec(trimmed)
  if (!brace) return true // 默认导入 / 命名空间导入 ⇒ 值
  // 花括号**之前**还有东西（`Foo, { … }` / `* as ns, { … }`）⇒ 那本身就是一个值绑定。
  if (trimmed.slice(0, brace.index).replace(/[,\s]/g, '') !== '') return true
  const names = brace[1]
    .split(',')
    .map((name) => name.trim())
    .filter(Boolean)
  return names.some((name) => !/^type\s/.test(name))
}

/** 一个模块里所有**值** import 的说明符（含副作用 import 与动态 import）。 */
function valueSpecifiers(source: string): string[] {
  const specs: string[] = []
  for (const match of source.matchAll(/\bimport\s+([^'";]+?)\s+from\s*['"]([^'"]+)['"]/g)) {
    if (hasValueBinding(match[1])) specs.push(match[2])
  }
  // 副作用 import（`import 'x'`）与动态 import（`import('x')`）都按值算。
  for (const match of source.matchAll(/\bimport\s*\(\s*['"]([^'"]+)['"]\s*\)/g)) specs.push(match[1])
  for (const match of source.matchAll(/^\s*import\s*['"]([^'"]+)['"]/gm)) specs.push(match[1])
  return specs
}

/** 说明符 → `src/` 下的相对路径（解析不出 ⇒ `null`：外部包 / 认不出的写法）。 */
function resolveModule(fromRel: string, spec: string): string | null {
  let base: string
  if (spec.startsWith('@/')) base = spec.slice(2)
  else if (spec.startsWith('./') || spec.startsWith('../')) {
    base = posix.normalize(posix.join(posix.dirname(fromRel), spec))
  } else return null
  for (const candidate of [base, `${base}.ts`, `${base}.tsx`, `${base}/index.ts`, `${base}/index.tsx`]) {
    if (existsSync(new URL(candidate, SRC))) return candidate
  }
  return null
}

/** 一个模块的 import 闭包（`src/` 内；**脊柱记录但不展开**）。 */
function closureOf(entry: string): Set<string> {
  const seen = new Set<string>()
  const queue = [entry]
  while (queue.length > 0) {
    const rel = queue.shift() as string
    if (seen.has(rel)) continue
    seen.add(rel)
    if (rel in SPINE) continue
    const source = readFileSync(fileURLToPath(new URL(rel, SRC)), 'utf8')
    for (const spec of valueSpecifiers(source)) {
      const resolved = resolveModule(rel, spec)
      if (resolved !== null && !seen.has(resolved)) queue.push(resolved)
    }
  }
  return seen
}

/** 按**页面模块**归并：同一个模块可能服务多条路由（`MyAgentsPage` 服务列表与主页）。 */
function byPage(): Map<string, { paths: string[]; allowed: Set<RoutePlane> }> {
  const grouped = new Map<string, { paths: string[]; allowed: Set<RoutePlane> }>()
  for (const route of planeCheckedRoutes()) {
    if (!route.page) continue
    const entry = grouped.get(route.page) ?? { paths: [], allowed: new Set<RoutePlane>() }
    entry.paths.push(route.path)
    for (const plane of route.planes) entry.allowed.add(plane)
    grouped.set(route.page, entry)
  }
  return grouped
}

function authorityOf(rel: string): { plane: RoutePlane; why: string } | null {
  if (rel in HOME_AUTHORITY) return { plane: 'home', why: HOME_AUTHORITY[rel] }
  if (rel in WORLD_AUTHORITY) return { plane: 'world', why: WORLD_AUTHORITY[rel] }
  return null
}

// ---------------------------------------------------------------------------
// 门禁自己不许空跑
// ---------------------------------------------------------------------------

test('门禁不许空跑：清单在、模块在、解析器真的在走', () => {
  assert.ok(Object.keys(HOME_AUTHORITY).length >= 5, '家面权威表太小 —— 这条守卫会静默失效')
  assert.ok(Object.keys(WORLD_AUTHORITY).length >= 5, '世界面权威表太小 —— 这条守卫会静默失效')
  for (const rel of [...Object.keys(HOME_AUTHORITY), ...Object.keys(WORLD_AUTHORITY), ...Object.keys(SPINE)]) {
    assert.ok(existsSync(new URL(rel, SRC)), `名单里的模块不存在（写错名字 = 守卫静默失效）：${rel}`)
  }

  // 正对照：聊天页那一棵树必须解析出一大堆模块 —— 解析器坏了与"边界干净"长得一模一样。
  const chat = closureOf('pages/ChatPage.tsx')
  assert.ok(chat.size >= 10, `ChatPage 只解析出 ${chat.size} 个模块 —— 解析器坏了`)
  assert.ok(chat.has('api/roomMembers.ts'), 'ChatPage 的闭包里应该有 api/roomMembers.ts')

  // 反例：**type-only import 不算读**（`status-tone.ts` 只从 `agentHealth` 取一个类型）。
  const tone = closureOf('components/celira/status-tone.ts')
  assert.ok(
    !tone.has('api/agentHealth.ts'),
    'type-only import 被算成了"读" —— 那会让每个只取类型的模块都变成越界',
  )
})

test('每条声明都回答了"读哪几个平面"与"页面模块是哪个"', () => {
  for (const route of ROUTES) {
    assert.ok(Array.isArray(route.planes), `${route.path} 没有 planes（新增路由必须回答它读哪几个平面）`)
    for (const plane of route.planes) {
      assert.ok(plane === 'world' || plane === 'home', `${route.path} 的 planes 里有认不出的取值：${plane}`)
    }
    assert.ok(route.page === null || typeof route.page === 'string', `${route.path} 的 page 形状不对`)
    if (route.page !== null) {
      assert.ok(existsSync(new URL(route.page, SRC)), `${route.path} 的页面模块不存在：${route.page}`)
    }
  }
})

test('★ 主体那一侧必须在 planes 里（`side` 是 world / home 时）', () => {
  for (const route of ROUTES) {
    if (route.side !== 'world' && route.side !== 'home') continue
    // 重定向没有数据面（它一个字节都不读）⇒ 下一段单独管它，这里只管有页面的。
    if (route.page === null) continue
    assert.ok(
      route.planes.includes(route.side),
      `${route.path}：side 是 ${route.side}，planes 里却没有它（${JSON.stringify(route.planes)}）`,
    )
  }
  // 反方向：**没有页面模块的条目不许声明平面**（它连数据面都没有，声明了就是凭空多一个答案）。
  for (const route of ROUTES) {
    if (route.page !== null) continue
    assert.deepEqual(
      [...route.planes],
      [],
      `${route.path} 没有页面模块，却声明了平面（${JSON.stringify(route.planes)}）—— 它一个字节都不读`,
    )
  }
})

test('★ 新增页面模块必须被某条声明认领（App.tsx 的懒加载 ⊆ 声明表）', () => {
  const lazy = [...APP_SOURCE.matchAll(/\bimport\s*\(\s*['"]([^'"]+)['"]\s*\)/g)].map((m) => m[1])
  // ⚠ 懒加载的说明符**不带扩展名**（`@/pages/AgentPage`），而声明的 `page` 带（`….tsx`）
  //   ⇒ 两边都要先过一遍**同一个**解析器，否则这条断言量的是"写法一致"而不是"页面被认领"。
  const pages = new Set(
    lazy
      .map((spec) => resolveModule('App.tsx', spec))
      .filter((rel): rel is string => rel !== null),
  )
  assert.ok(pages.size >= 15, `只解析出 ${pages.size} 个懒加载页面模块 —— 解析器坏了`)

  const declared = new Set(ROUTES.map((route) => route.page).filter((page): page is string => page !== null))
  const unclaimed = [...pages].filter((page) => !declared.has(page)).sort()
  assert.deepEqual(
    unclaimed,
    [],
    '这些页面模块被 App.tsx 懒加载，但没有任何一条路由声明认领它们（新增页面必须回答它读哪几个平面）：\n' +
      unclaimed.map((page) => `  ＋ ${page}`).join('\n'),
  )

  const stale = [...declared].filter((page) => !APP_SOURCE.includes(`'@/${page.replace(/\.tsx?$/, '')}'`)).sort()
  assert.deepEqual(
    stale,
    [],
    '这些页面模块在声明表里，但 App.tsx 已经不引用它们了（陈旧条目）：\n' +
      stale.map((page) => `  － ${page}`).join('\n'),
  )
})

// ---------------------------------------------------------------------------
// 判据本体
// ---------------------------------------------------------------------------

test('★ 声明的面 ⊇ 实际读的面（world 页不许读家面，反之亦然）', () => {
  const violations: string[] = []
  for (const [page, { paths, allowed }] of byPage()) {
    const declared = [...allowed].join(' + ') || '两侧都不读'
    for (const rel of closureOf(page)) {
      const authority = authorityOf(rel)
      if (authority === null) continue
      if (allowed.has(authority.plane)) continue
      violations.push(
        `  ✗ ${paths.join(' / ')}（声明：${declared}）读了 ${authority.plane} 面的 ${rel}` +
          `\n      —— ${authority.why}`,
      )
    }
  }
  assert.deepEqual(
    violations,
    [],
    '声明与事实各说各话：\n' +
      violations.join('\n') +
      '\n\n改法只有两条：**把那一面写进 `planes`**，或者**把那一页的读取去掉**。' +
      '\n⚠ 不许把模块从权威表里挪走 —— 那是把判据改成能过，不是把边界理清。',
  )
})

test('★ 声明含 home 的页必须真的读到家面（防"声明与事实相反"）', () => {
  const silent: string[] = []
  for (const [page, { paths, allowed }] of byPage()) {
    if (!allowed.has('home')) continue
    const reads = [...closureOf(page)].some((rel) => rel in HOME_AUTHORITY)
    if (!reads) {
      silent.push(`  ✗ ${paths.join(' / ')}（声明含家面）整棵 import 闭包里一个家面权威模块都没有`)
    }
  }
  assert.deepEqual(
    silent,
    [],
    '声明与事实相反（那次事故的形状：`/app/agents/:participantId` 声明 home 而只读 core）：\n' +
      silent.join('\n') +
      '\n\n要么它本来就不读家（那 `planes` 该去掉 home），要么它的读取走了**脊柱**（`api/store.ts`）' +
      '—— 后者要在这里说明白，别让它看起来像"读家"。',
  )
})
