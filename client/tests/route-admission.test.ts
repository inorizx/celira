// **路由准入判据**（2026-09-25 立）：每个路由都必须显式归入某一侧。
//
// ## 为什么需要一条机械判据，而不是"写进纪律就行"
//
// `DESIGN_SYSTEM.md` §导航壳早就写着「**新增路由时必须先决定它属于哪一侧**」——
// 但那是**一句话**。代价已经付过三次（三条都不是"忘了改"，是**没有一处能回答
// "这个路由归哪一侧"**）：
//
//   - `/app/services`：没人决定过 ⇒ 既不在任何导航分区，只靠「我的」页一条捷径够得着
//     （2026-09-28：随 Agent 服务层整体退役删除，见文件末尾那条断言）；
//   - `/identity`：决定成"fixture 页"，但没人问过它「创建身份」这个承诺是否属实
//     （实测：两个按钮都只是 `navigate('/app/chat')`）⇒ 已删；
//   - `/agent/:participantId`：同样挂进守卫，但没人问过"真实世界有 4 处入口链到它"。
//
// ## 这条测试怎么工作
//
// **读 `src/App.tsx` 的真实路由表**（不 import —— 它带 JSX 依赖），抽出所有 `path`
// 与它们是否在 `FixtureRouteGuard` 之下，然后与 `api/routeRegistry.ts` 的 `ROUTES`
// 逐条对表。四类失败各自有话：
//
//   1. 路由表里有、声明表里没有 ⇒ **新增路由没回答"归哪一侧"**；
//   2. 声明表里有、路由表里没有 ⇒ **陈旧条目**（路由删了，声明要跟着删）；
//   3. `side === 'demo'` 但不在守卫下 ⇒ 声明与实现矛盾（那一页真实世界会露出来）；
//   4. 不是 `demo' 却在守卫下 ⇒ 同上，反方向。
//
// ⚠ 它**不**断言"这个侧选得对不对"（那是产品取舍，机器判不了）；它断言的是
// **有人回答过、且回答与实现一致**。这正是元问题该有的形状。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { ROUTES, demoRoutePaths, reachableInRealWorld } from '../src/api/routeRegistry.ts'

const APP_SOURCE = readFileSync(fileURLToPath(new URL('../src/App.tsx', import.meta.url)), 'utf8')

test('门禁不许空跑：本文件真的执行了用例', () => {
  assert.ok(true)
})

/**
 * 去掉行注释与块注释（保留字符串与模板字面量里的内容）。
 *
 * ⚠ 两个坑都踩过：
 * 1. **块注释正文里出现"星号紧跟斜杠"这组字符**（为了讲清格式而把它写出来）——
 *    天真的实现会在那里**提前结束注释**，把后面的散文当成代码 ⇒ 幽灵路由。
 *    所以这里**先认字符串/模板字面量**：引号内的那组字符不算注释结束。
 * 2. **行注释标记出现在 URL 里**（`https:` 后面跟两个斜杠）—— 同理由"先认字符串"挡住。
 */
function stripComments(code: string): string {
  let out = ''
  let quote: string | null = null
  for (let i = 0; i < code.length; i++) {
    const ch = code[i]
    if (quote) {
      out += ch
      if (ch === '\\') {
        out += code[i + 1] ?? ''
        i++
      } else if (ch === quote) {
        quote = null
      }
      continue
    }
    if (ch === "'" || ch === '"' || ch === '`') {
      quote = ch
      out += ch
      continue
    }
    if (ch === '/' && code[i + 1] === '/') {
      while (i < code.length && code[i] !== '\n') i++
      out += '\n'
      continue
    }
    if (ch === '/' && code[i + 1] === '*') {
      i += 2
      while (i < code.length && !(code[i] === '*' && code[i + 1] === '/')) i++
      i++
      continue
    }
    out += ch
  }
  return out
}

/** 相对段拼到父路径上；已带前导 `/` 的按绝对路径算。 */
function resolvePath(base: string, segment: string): string {
  if (segment.startsWith('/')) return segment
  if (segment === '*') return base ? `${base}/*` : '*'
  // 根层（base 为空）的相对段就是它自己 —— 不加前导斜杠（加了会得到 `//*` 这种）。
  return base ? `${base}/${segment}` : segment
}

/**
 * 从 `App.tsx` 的源码里抽出**路由表**：每个 path 声明 + 它在不在 `FixtureRouteGuard` 下。
 *
 * 解析法：**单遍扫描 token 流 + 路径作用域栈**。去掉注释后，在
 * `createBrowserRouter([...])` 那一段里顺序找三类 token：
 *
 * - `path: '…'` ⇒ 用当前作用域前缀拼出完整路径，登记一条路由；
 * - `children: [ … ]` ⇒ 这一段子路由的作用域前缀 = 刚登记的那个 path；
 * - `element: <FixtureRouteGuard />` ⇒ 这个对象的 children 全部标 `guarded`。
 *
 * ⚠ 为什么不用"递归找配对括号再切片"（我先写错过两版，记在这里）：切出来的子串会
 * **包含自己的 children**，于是子对象在父的切片里被再看一遍、前缀被叠加两次 ——
 * 实测多出一条不存在的 `/app/discover/discover`，以及一条 `/` 与 `*` 拼成的 `//*`。
 * **按 token 顺序走、用栈管作用域**没有这个重复访问问题：每个 token 只处理一次。
 *
 * ⚠ 还有一条：切片的起点必须是 `createBrowserRouter(` 而**不是文件头** ——
 * 文件头的 `import { FixtureRouteGuard }` 也含那个字样，从文件头切会让"是不是守卫块"
 * 对整张表为真（实测：全部被判成 GUARDED，而那会让第 3/4 条判据永远通过）。
 */
function parseRouteTable(source: string): Map<string, boolean> {
  const start = source.indexOf('createBrowserRouter([')
  assert.ok(start >= 0, 'App.tsx 里找不到 createBrowserRouter([ —— 这条测试要跟着改')
  const body = stripComments(source.slice(start))
  const inner = body.slice(body.indexOf('[') + 1, body.lastIndexOf(']'))

  const routes = new Map<string, boolean>()
  /** 路径作用域栈：每个 `children:` 数组一层（根层是空串）。 */
  const scopes: string[] = ['']
  /** 守卫作用域栈：与 scopes 同步，进入 `FixtureRouteGuard` 的 children 后为 true。 */
  const guards: boolean[] = [false]
  /** 最近登记的 path（供紧随其后的 `children:` 当作用域用）。 */
  let lastPath: string | null = null
  /** 当前对象自己的 element 是不是守卫（决定它的 children 要不要标 guarded）。 */
  let objectIsGuard = false

  const tokenRe = /\bpath:\s*'([^']*)'|\bchildren:\s*\[|\belement:\s*([^,}]*)|[{}[\]]/g
  let m: RegExpExecArray | null
  while ((m = tokenRe.exec(inner)) !== null) {
    const token = m[0]

    if (token === '{' || token === '}') {
      objectIsGuard = false
      lastPath = null
      continue
    }
    if (m[1] !== undefined) {
      const full = resolvePath(scopes[scopes.length - 1] ?? '', m[1])
      routes.set(full, guards[guards.length - 1] ?? false)
      lastPath = full
      continue
    }
    if (m[2] !== undefined) {
      if (/<FixtureRouteGuard\s*\/>/.test(m[2])) objectIsGuard = true
      continue
    }
    if (token.startsWith('children:')) {
      scopes.push(lastPath ?? scopes[scopes.length - 1] ?? '')
      guards.push((guards[guards.length - 1] ?? false) || objectIsGuard)
      lastPath = null
      continue
    }
    if (token === ']') {
      // 只弹出由 `children:` 压入的层；根层留着。
      if (scopes.length > 1) {
        scopes.pop()
        guards.pop()
      }
      lastPath = null
      continue
    }
  }
  return routes
}

const ACTUAL = parseRouteTable(APP_SOURCE)
const DECLARED = new Map(ROUTES.map((r) => [r.path, r]))

/**
 * 把解析出来的整张表渲染成可读文本 —— **失败信息里要带上它**。
 *
 * 为什么值得单独一个函数：这条测试的判据是"两张表对得上"，一旦对不上，最需要知道的
 * 就是**解析器到底看见了什么**。只报"多了/少了哪条"时，解析器自身的毛病
 * （比如把 `/app/discover` 拼成 `/app/discover/discover`）会伪装成"声明表有问题"，
 * 让人去改声明表 —— 修错东西。
 */
function renderActual(): string {
  return [...ACTUAL.entries()].map(([p, g]) => `  ${g ? 'GUARDED' : '       '} ${p}`).join('\n')
}

test('路由表真的被解析出来了（防空跑：解析失败会让下面每条都变成假绿）', () => {
  assert.ok(ACTUAL.size >= 20, `只解析出 ${ACTUAL.size} 条路由 —— 解析器坏了？\n${renderActual()}`)
  assert.ok(ACTUAL.has('/app/chat'), `解析结果里没有 /app/chat —— 解析器坏了\n${renderActual()}`)
  assert.ok(ACTUAL.has('/splash'), `解析结果里没有 /splash —— 解析器坏了\n${renderActual()}`)
  // 守卫块必须真的被认出来（否则第 3/4 条判据永远为真）。
  assert.ok(
    [...ACTUAL.values()].some((guarded) => guarded),
    `没有任何路由被判为"在守卫下" —— 守卫块没被解析出来，第 3/4 条判据失效\n${renderActual()}`,
  )
  // 反过来也要防：**不能全是** guarded（那说明守卫判据对整张表为真）。
  assert.ok(
    [...ACTUAL.values()].some((guarded) => !guarded),
    `所有路由都被判成"在守卫下" —— 守卫判据太宽，第 3/4 条判据失效\n${renderActual()}`,
  )
})

test('★ 新增路由必须先回答"它归哪一侧"（路由表 ⊆ 声明表）', () => {
  const undeclared = [...ACTUAL.keys()].filter((p) => !DECLARED.has(p)).sort()
  assert.deepEqual(
    undeclared,
    [],
    `这些路由在 App.tsx 里存在，但没在 api/routeRegistry.ts 的 ROUTES 里声明归属侧：\n` +
      undeclared.map((p) => `  ＋ ${p}`).join('\n') +
      '\n\n请回答：它在真实世界里说得出话吗？' +
      '\n  entry    = 连接之前的首屏（启动页 / 登录页）' +
      '\n  world    = 读真实 im 服务端事实' +
      '\n  home     = 读"家"（那台机器的状态口 / 配置门）' +
      '\n  settings = 关于我和这台设备' +
      '\n  shared   = 两侧都能开（重定向 / 错误页这类没有数据面的路由）' +
      '\n  demo     = **真实世界被诚实门拦下**（必须同时挂进 FixtureRouteGuard）' +
      `\n\n解析器实际读到的整张表（${ACTUAL.size} 条）：\n${renderActual()}`,
  )
})

test('★ 声明表不许留陈旧条目（声明表 ⊆ 路由表）', () => {
  const stale = ROUTES.map((r) => r.path).filter((p) => !ACTUAL.has(p)).sort()
  assert.deepEqual(
    stale,
    [],
    `这些路由已在 App.tsx 里不存在，但 ROUTES 里还声明着（路由删了，声明要跟着删）：\n` +
      stale.map((p) => `  － ${p}`).join('\n') +
      `\n\n解析器实际读到的整张表（${ACTUAL.size} 条）：\n${renderActual()}`,
  )
})

test('★ 声明与实现一致：demo ⇔ 在 FixtureRouteGuard 之下（两个方向都查）', () => {
  const mismatched: string[] = []
  for (const route of ROUTES) {
    const guarded = ACTUAL.get(route.path)
    if (guarded === undefined) continue // 陈旧条目由上一条负责报
    if (route.side === 'demo' && !guarded) {
      mismatched.push(`  ✗ ${route.path}：声明是 demo（真实世界该被拦），实际**不在**守卫下 ⇒ 真实用户会看到这一页`)
    }
    if (route.side !== 'demo' && guarded) {
      mismatched.push(`  ✗ ${route.path}：声明是 ${route.side}（真实世界该能开），实际**在**守卫下 ⇒ 真实用户只会撞诚实门`)
    }
  }
  assert.deepEqual(mismatched, [], `声明与实现各说各话：\n${mismatched.join('\n')}`)
})

test('demo 是唯一"真实世界进不去"的侧 —— 它的路径集合只有一个来源', () => {
  // 这条钉的是**判据本身**：`reachableInRealWorld` 与 `side` 必须一致，
  // 免得哪天有人往 demo 里塞一个"其实真实世界也能开"的页。
  for (const route of ROUTES) {
    assert.equal(
      reachableInRealWorld(route),
      route.side !== 'demo',
      `${route.path} 的 reachableInRealWorld 与 side 不一致`,
    )
  }
  assert.deepEqual(
    demoRoutePaths().sort(),
    ROUTES.filter((r) => r.side === 'demo')
      .map((r) => r.path)
      .sort(),
  )
})

test('每个声明都写得出"这一页说什么"（what 不许是占位符）', () => {
  for (const route of ROUTES) {
    assert.ok(route.what.trim().length >= 8, `${route.path} 的 what 太短或没写：${JSON.stringify(route.what)}`)
    assert.ok(!/TODO|待填|占位/.test(route.what), `${route.path} 的 what 是占位符：${route.what}`)
  }
})

test('已删的 /identity 不许以任何形式回来', () => {
  // 那条路由与页面的删除理由见 routeRegistry.ts 头部：**承诺不实**
  // （标题「先创建你的 Human 身份」、按钮「创建身份并进入」，实际只 navigate 一下）。
  // 这条断言防的是"换个名字回来"。
  assert.ok(!ACTUAL.has('/identity'), 'App.tsx 里又出现了 /identity')
  assert.ok(!ROUTES.some((r) => r.path === '/identity'), 'routeRegistry 里又声明了 /identity')
  assert.ok(!APP_SOURCE.includes('IdentityPage'), 'App.tsx 里又 import 了 IdentityPage')
})

test('已删的 /app/services 不许以任何形式回来', () => {
  // 2026-09-28：**Agent 服务层整体退役**（Publication / Access / 服务房）——
  // 那一层是工作态（"我能提供什么服务"）而不是 IM 事实，违反了 2026-09-26 那条
  // 红线「执行过程不经过 IM」。页面、`ServiceApi` 与协议面同时下线。
  // 与 `/identity` 同款：防的是"换个名字回来"。
  assert.ok(!ACTUAL.has('/app/services'), 'App.tsx 里又出现了 /app/services')
  assert.ok(!ROUTES.some((r) => r.path === '/app/services'), 'routeRegistry 里又声明了 /app/services')
  assert.ok(!APP_SOURCE.includes('MyServicesPage'), 'App.tsx 里又 import 了 MyServicesPage')
})
