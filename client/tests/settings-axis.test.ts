// **设置目录的轴：每一项归哪一组，由 `routeRegistry` 的 `planes` 说了算**（2026-10-11 立）。
//
// ## 它治的是什么
//
// 2026-10-03 立过一条"轴"判据（现在还在真机脚本里：`scripts/verify-ui.mjs` 的 `SETTINGS_AXIS`），
// 但它量的是**组标题与条目标签这两个字符串** ⇒ 两个漏洞：
// ① **改名即失效**（仓库点过名的那类事故：*守卫写死了被守卫对象的取舍*）；
// ② 它管不到"组标题与那一条**实质**是不是同一个轴"。2026-10-11 真撞上了：
//    `/settings/server` 在注册表里声明的**一直是** `planes: ['home']`（这台机器），
//    却被挂在「这个世界」组下 ✗ —— 而那条断言照样绿 ✓。
//
// ## 现在怎么判
//
// **事实**取自 `routeRegistry`（每一页自己声明读哪几个平面），**判断**写在下面那张表里
// （"这一组的主语该是什么"）⇒ 两边对不上就红。以后再有人把 home 页挪进 world 组、
// 或把 world 页挪进 home 组，**一个字都不用改就会红**。
//
// ⚠ 三个平面值，不是一个二选一：`world`（你连的世界）／`home`（这台机器）／
// **两侧都不读**（这台设备上的客户端状态：外观、通知开关、关于那一页）——
// 后一档在注册表里写作 `planes: []`，**不许**当成"机器级"直接归进 `home`
// （注册表那一节写着为什么：家面是**读那台机器的门**，而这一档只读浏览器自己的库）。
//
// 判错即红的样子：把 `/settings/server` 挪回「这个世界」；或新增一个空组；或新增一条
// 目录项却忘了在注册表里声明它；或给某一组写下"什么都能放"的轴。
//
// 运行： node --test --experimental-strip-types "tests/*.test.ts"

import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { ROUTES } from '../src/api/routeRegistry.ts'

const LAYOUT = readFileSync(
  fileURLToPath(new URL('../src/pages/settings/SettingsLayout.tsx', import.meta.url)),
  'utf8',
)

/**
 * **判断**：每一组的主语该是什么（键 = 组标题；值是允许出现在这一组里的平面读数）。
 *
 * ⚠ 加一个组就要在这里写清它的轴 —— 不写就红（这正是这条守卫的用处）。
 */
const GROUP_PLANES: Record<string, { allow: string[]; why: string }> = {
  账号与通用: {
    // 「通用」读的是**你连的世界**（地址与 token）；「外观」「通知」读的是**这台设备上的
    // 客户端状态**（两侧都不读）⇒ 这一组**有意**混着这两档，但每一档都必须写对。
    allow: ['world', ''],
    why: '「通用」=世界侧；「外观」「通知」=这台设备的客户端状态',
  },
  你的机器: {
    // 2026-10-11 收口：这一组的主语**全是这台机器**（本机服务 / 家）。
    allow: ['home'],
    why: '这一组里每一项读的都是**这台机器**（家面）',
  },
  系统: {
    allow: [''],
    why: '「关于」读的是这台设备上的客户端状态',
  },
}

/** 从 `SettingsLayout.tsx` 的 `NAV_GROUPS` 里切出「组 → 条目地址」。 */
function navGroups(): { title: string; items: string[] }[] {
  const start = LAYOUT.indexOf('const NAV_GROUPS')
  assert.ok(start > 0, '找不到 `NAV_GROUPS` —— 改名了？这条守卫要跟着改')
  const end = LAYOUT.indexOf('\n]', start)
  assert.ok(end > start, '`NAV_GROUPS` 的正文切不出来 —— 守卫要跟着改')
  return LAYOUT.slice(start, end)
    .split(/\btitle:\s*'/)
    .slice(1)
    .map((chunk) => {
      const close = chunk.indexOf("'")
      assert.ok(close > 0, '`title:` 那一行读不出组名')
      return {
        title: chunk.slice(0, close),
        items: [...chunk.matchAll(/to:\s*'([^']+)'/g)].map((match) => match[1]!),
      }
    })
}

/** 一条路由声明的平面读数（`''` = 两侧都不读）。 */
const planeOf = (path: string): string | null => {
  const route = ROUTES.find((entry) => entry.path === path)
  return route ? route.planes.join('+') : null
}

test('门禁不许空跑：目录切出来了、每一条都在注册表里、没有空组', () => {
  const groups = navGroups()
  assert.ok(groups.length >= 3, `只切出 ${groups.length} 组 —— 切错了？`)
  const items = groups.flatMap((group) => group.items)
  // ⚠ 这是**防空跑**的下限，不是"应该有这么多"：2026-10-12 退役了「家」那一项
  //   （机器清单只留一个门）⇒ 5 条。**加/删目录项时跟着改这一行**，别把它当判据。
  assert.ok(items.length >= 5, `只切出 ${items.length} 条目录项 —— 切错了？`)
  for (const to of items) {
    assert.equal(typeof planeOf(to), 'string', `目录里有注册表**没声明**的地址：${to}`)
  }
  for (const group of groups) {
    assert.ok(group.items.length > 0, `「${group.title}」是**空组** —— 空的分组不该留在目录里`)
  }
})

test('★ 每一项目录项所在的组，必须与它在注册表里声明的平面同一个轴', () => {
  const wrong: string[] = []
  for (const group of navGroups()) {
    const axis = GROUP_PLANES[group.title]
    assert.ok(
      axis,
      `目录里出现了没登记的组「${group.title}」—— 加组就要在 GROUP_PLANES 里写清它的轴`,
    )
    for (const to of group.items) {
      const plane = planeOf(to) ?? '(没声明)'
      if (!axis.allow.includes(plane)) {
        wrong.push(`${to}（读的是 ${plane || '两侧都不读'}）∈「${group.title}」`)
      }
    }
  }
  assert.deepEqual(
    wrong,
    [],
    `这些条目的平面与它所在的组不是同一个轴：${wrong.join(' · ')}` +
      `（每组的轴：${Object.entries(GROUP_PLANES)
        .map(([title, axis]) => `${title}=${axis.why}`)
        .join('；')}）`,
  )
})

test('★ 世界侧那两样（地址 / 账号）的权威页在「通用」，且它确实读世界面', () => {
  // 这一条把"搬家的方向"也钉住：`/settings/server` 收成一个主语之后，世界侧读数只剩
  // `general` 一处 —— 它必须是 `world`（否则等于**没人读**）。
  assert.equal(planeOf('/settings/general'), 'world', '「通用」不再声明读世界面了？')
  assert.equal(planeOf('/settings/server'), 'home', '「本机服务」又跑回世界面了？')
})
