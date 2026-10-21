import { Bot, Compass, MessageSquare, Server, Users, type LucideIcon } from 'lucide-react'
// ⚠ 相对路径（不是 `@/api/...`）：本模块被 `node --test` 直接跑（类型剥离、无打包器），
// 别名在那里解析不了 —— 与 `api/shellLocal.ts` 那条线的写法一致。
import { ROUTES, reachableInRealWorld } from '../../api/routeRegistry.ts'

/**
 * 壳层导航分区（TASK_52）：**唯一事实源**。
 *
 * 产品语义（主人 2026-09-18/20 裁决）：Celira IM = 通讯产品（世界的事），
 * Agent 产品 = 用户侧系统（执行端的事）。一句话：**连线归世界，生活归执行端**。
 * 上组「世界」= 房间/消息/联系人/发现；下组「执行端」= **我的执行端 · 我的 Agent**
 * （2026-09-29 起是两个入口 —— 为什么补第二个见下面 `NAV_GROUPS` 里那一段）。
 *
 * ⚠ **2026-09-25 组织轴刀**：下组的标签与入口从「家 / 我的 Agent」改成
 * **「执行端 / 我的执行端」**。理由不是审美，是**轴错了**：
 *
 * - 那一区读的**全是机器级事实**（Agent 在不在跑、醒过几次、用哪条模型），
 *   而它当时以 **Agent** 为轴（"我的 Agent"是列表、点一个 Agent 进主页）⇒
 *   页面必须先"认领"（遍历机器找哪台报的是这个 Agent）才说得出话，
 *   "运行状态代表谁"页面自己答不出来；
 * - 现在主语是**执行端**：`/app/runtimes` 一台机器一行、`/app/runtimes/:id` 一台机器一页；
 *   「这台机器上跑着谁」是机器的一个属性，由那一页说一次。
 *
 * 三个词各归其位（`PRODUCT_VISION.md` 的「一词一主」是唯一解释处）：
 * **世界**（契约专名 `Home`）= im 服务端与它裁决的公共事实；
 * **执行端** = 跑 Agent 的机器（用户可见的说法）；
 * **家** = `home.js` 管的那套本机环境（本机工具语境，不上导航）。
 *
 * ⚠ **2026-09-23 称谓刀**：页面内句子里泛指一个 Agent 时用 `Agent`（大写 A），
 * 页面在手知道名字时用名字；导航是**铬件**（短标签），不带主语。
 *
 * 纪律（`DESIGN_SYSTEM.md`「导航壳」）：
 * - 桌面图标栏与移动 TabBar **共用这一份分组数据**，不建平行机制、不建第二套
 *   导航范式；新增入口只往这里加一项并配好路由。
 * - 图标是**入口的形状**不是数据：未接通的页面（如发现）由
 *   `FixtureRouteGuard` 渲染诚实「尚未提供」兜底，分区不给任何页面造假通行证。
 * - 「我的服务」（`/app/services`）**已随 Agent 服务层整体退役删除**（2026-09-28）：
 *   那条"归属已裁在世界侧、入口未预建"的悬案随之结案，不再有复审项。
 *
 * 放在独立 `.ts`（无 JSX）里，是为了让纯逻辑测试能直接断言分组结构本身。
 */
export interface NavItem {
  id: string
  label: string
  icon: LucideIcon
  to: string
}

export interface NavGroup {
  /** 分区的机器可读身份（测试与 aria 用）与人类标签。 */
  id: 'world' | 'agent'
  label: string
  items: NavItem[]
}

export const NAV_GROUPS: NavGroup[] = [
  {
    id: 'world',
    label: '世界',
    items: [
      { id: 'messages', label: '消息', icon: MessageSquare, to: '/app/chat' },
      { id: 'contacts', label: '联系人', icon: Users, to: '/app/contacts' },
      { id: 'discover', label: '发现', icon: Compass, to: '/app/discover' },
    ],
  },
  {
    // 「执行端」组：Agent 产品的一级入口。**两个**（2026-09-29 补第二个）：
    // 「我的执行端」= 机器列表（点进去是那一台机器那一页）、「我的 Agent」= Agent 清单
    // （每行拼世界侧所有权 + 家侧三格读数）。
    //
    // ⚠ **2026-09-29 为什么补回来**：2026-09-25 那一刀把它去掉的理由是**轴错了**
    //   （Agent 优先的页面读机器读数 ⇒「运行状态代表谁」页面自己答不出来），那条判断
    //   今天仍然成立、也仍然由机器页承担。**变的是这一页**：`/app/agents` 现在是一张
    //   **明说自己拼两件事**的清单（`routeRegistry.ts`：`side: 'home'`，每行 = 世界侧
    //   所有权 + 家侧三格读数），它不再冒充"Agent 的运行状态"。
    //   而代价是实测出来的：**这一页原先不在任何分区里**，唯一入口是「我的」页里那张卡
    //   —— 一个一级内容拿二级待遇，用户在自己的导航里找不到自己的 Agent 清单。
    //   ⚠ 轴**没有变**：两个入口都在执行端组，都读家；变的只是"找得到"。
    //
    // 看一个 Agent 的运行/模型：先有机器（`/app/runtimes`），或者从 Agent 那一页的
    // 「机器」那一段跳过去（`/app/agents/:id#machine`）—— **两个方向都通**。
    // Agent 自己的公开资料与接入方式在世界侧（`/app/contacts` → 资料卡 → Agent 页）。
    //
    // ★ **2026-10-20（T5 命名统一）**：组标与入口名从「执行端 / 我的执行端」改成
    //   「机器 / 我的机器」。判据在 `AGENTS.md` 与 `docs/runtime/AGENT_VOCABULARY.md`：
    //   **`执行端` 是平面名**（"本节的名字全部属于执行端平面"），而"承载执行端的机器或部署
    //   位置"那个词是**执行点**；把平面名当机器名用，一句跨平面的话就分不清主语是哪一个。
    //   ⇒ 屏幕上的产品词是**机器**（用户操作的对象），`执行端` 只在机器页那句**定义**里
    //   出现一次（说清"这台机器上跑着 Agent 的执行端"）。
    id: 'agent',
    label: '机器',
    // ★ **2026-10-21 图标对调**（判据：**图标画的名词必须与这一项的主语同轴**）：
    // 「我的机器」=`Server`、「我的 Agent」=`Bot`，`Sparkles` 退出导航。
    //   - 原先正好反着：机器人头挂在**机器**上。而这区的主语是机器（见上面 2026-09-25 那刀），
    //     `Bot` 在所有产品里都读作「一个 AI」⇒ 屏幕把拆掉的那根轴又画了回来；
    //   - `Sparkles` 在本仓已经指 Agent（`RuntimesPage` 那颗「我的 Agent（在那里建一支）」），
    //     于是一个字形同时指两个入口 ⇒ **所指交叉**；而这一对恰是本产品最该被分开的一对
    //     （基础设施 / 住户）；
    //   - `Server` 本来就是本仓「机器」的形（机器页头、`AgentsListPanel` 的机器行用它指
    //     「这台机器」）；`Bot` 与「联系人」的 `Users` 同层：都是**身份**。
    // ⚠ 别因为"AI 感"换回去：常驻 chrome 最不该放万能贴纸（星星同时表示 新/AI/推荐/智能，
    //   本身不携带信息），且它 20px 低对比下三颗星糊成一团。
    items: [
      { id: 'runtimes', label: '我的机器', icon: Server, to: '/app/runtimes' },
      { id: 'agents', label: '我的 Agent', icon: Bot, to: '/app/agents' },
    ],
  },
]

/**
 * 两端的共享派生：按分区顺序摊平成一维入口列表。桌面与移动只需要「怎么分组
 * 呈现」的差异，绝不允许各自维护第二份入口清单（那正是漂移的入口）。
 */
export function flattenNavItems(groups: NavGroup[] = NAV_GROUPS): NavItem[] {
  return groups.flatMap((group) => group.items)
}

/**
 * **真实世界里进得去的入口集合**（2026-10-03 刀）。
 *
 * 判据**不在这一层**，在路由表自己手里：`routeRegistry.reachableInRealWorld`
 * ——`demo` 是唯一一个"真实世界进不去"的侧（守卫直接换成诚实门禁页）。
 * 这一层只做一件事：**把真实世界里必然撞门的入口从导航里摘掉**。
 *
 * 为什么必须有它（真机读数）：`发现`（`/app/discover`）挂在「世界」组一级，
 * 而它 `side: 'demo'` ⇒ 真实用户点进去只有一句"尚未提供"。routeRegistry 自己写着
 * 这条纪律：*任何"真实用户点得到、点进去却撞门"的入口都是缺陷*。
 *
 * ⚠ 这是**派生**不是第二份清单：世界契约（Directory）落地后，那条路由的 `side`
 * 一改，它**自动**回到导航里 —— 这一层一个字都不用动。分区空了整组也不渲染。
 */
export function navGroupsFor(realWorld: boolean, groups: NavGroup[] = NAV_GROUPS): NavGroup[] {
  if (!realWorld) return groups
  const unreachable = new Set(
    ROUTES.filter((route) => !reachableInRealWorld(route)).map((route) => route.path),
  )
  return groups
    .map((group) => ({
      ...group,
      items: group.items.filter((item) => !unreachable.has(item.to)),
    }))
    .filter((group) => group.items.length > 0)
}

/**
 * ★ **一级入口不是二级页**（2026-10-11 立）：移动端外壳据此决定"沉浸全屏、TabBar 让位"
 * 与"这一页得自带返回出口"（`AppShell.tsx` 唯一的调用点）。
 *
 * 病根：那条判据原先是 `pathname.startsWith('/app/agents')` —— 于是**清单本身**
 * （`/app/agents`，导航里的一级入口）也被判成二级 ✗：移动端 TabBar 让位，页面还顶着
 * 一颗「返回我的」⇒ 人在**一级入口**上，却被告知自己在「我的」下面一层。
 * ⚠ 而这个仓库**早就写过**这条判据（本文件上面那段：把 `/app/agents` 加回导航，理由正是
 * "**一个一级内容拿二级待遇**，用户在自己的导航里找不到自己的 Agent 清单"）——
 * **判据写了，判据的反面却留在外壳里**。
 *
 * ⇒ 现在把它**从导航派生**：一级入口的地址**永远**不是二级页 ⇒ 以后再加一个一级入口，
 * 也不会重犯（不用记得来改这里）。
 *
 * ⚠ **二级 = 一级入口下面再深一层**：一间房、一个联系人、某一支 Agent、**某一台机器**
 * （以及它的模型配置那一页）—— 它们都自带返回出口。
 *
 * ★ **2026-10-11 补上"某一台机器"**：`/app/runtimes/:entryId` 与
 * `/app/runtimes/:entryId/config` 从前**不算二级**（自带「返回我的执行端」却保留 TabBar ✗）——
 * 那不是"说错自己在哪一层"，而是**不一致**：同一族的详情页（房 / 联系人 / Agent）都沉浸全屏。
 * 判据一句话：**一级入口下面那一层就是二级页**，与它是哪种东西无关。
 */
export function isSecondaryMobileRoute(pathname: string, groups: NavGroup[] = NAV_GROUPS): boolean {
  if (flattenNavItems(groups).some((item) => item.to === pathname)) return false
  return (
    /^\/app\/chat\/[^/]+/.test(pathname) ||
    /^\/app\/contacts\/[^/]+/.test(pathname) ||
    /^\/app\/agents\/[^/]+/.test(pathname) ||
    /^\/app\/runtimes\/[^/]+/.test(pathname)
  )
}
