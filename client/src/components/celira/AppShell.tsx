import { Fragment, useEffect } from 'react'
import { Settings, UserRound } from 'lucide-react'
import { NavLink, Outlet, useLocation, useNavigate } from 'react-router'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { cn } from '@/lib/utils'
import { UnreadBadge } from '@/components/celira/UnreadBadge'
import { SELF_ID } from '@/fixtures/data'
import { getParticipant } from '@/api/participantDirectory'
import { toParticipant } from '@/api/participantProjection'
import { isRealWorldSession } from '@/api/prefs'
import { useAppStore, useConnection, useRooms } from '@/api/store'
import { useUnreadIndicators } from '@/hooks/use-unread'
import { isSecondaryMobileRoute, navGroupsFor } from './navGroups'
import { ParticipantAvatar } from './ParticipantAvatar'
import { ThemeToggle } from './ThemeToggle'

/**
 * 导航分区（TASK_52）：上组「世界」（IM：消息/联系人/发现）+ 下组「**执行端**」
 * （Agent 产品：**我的执行端 · 我的 Agent**，两个入口 —— 入口集合见 `navGroups.ts`）。
 * **唯一事实源 = `navGroups.ts`**，
 * 桌面图标栏与移动 TabBar 共用它，不建平行机制（设计系统「导航壳」纪律）。
 *
 * ⚠ 2026-09-26 修陈旧：本条注释原写「下组「家」（Agent：我的 Agent/模型配置/运行状态）」——
 * 那是 2026-09-22 刀2a 之前的形状。2026-09-25 组织轴刀已把它改成「执行端 / 我的执行端」，
 * 且那一区的主语是**机器**（`navGroups.ts` 头上有完整理由）。
 *
 * ★ **2026-10-11：「二级页」这条判据搬进 `navGroups.ts` 并从导航派生**。原先它是这里一段
 * `pathname.startsWith('/app/agents')` —— 把**一级入口本身**（`/app/agents` 清单）也判成了
 * 二级页 ✗：移动端 TabBar 让位、页面还顶着「返回我的」。判据现在只有一处，且
 * **一级入口永远不是二级**（见那个函数的注释）。
 */
export function AppShell() {
  const location = useLocation()
  // 移动端二级页面（一间房 / 一个联系人 / 某一支 Agent）沉浸全屏，底部 TabBar 让位；
  // 这些页面必须自带移动端返回出口。**一级入口不在此列**（判据在 `navGroups.ts`）。
  const hideMobileTabBar = isSecondaryMobileRoute(location.pathname)
  const rooms = useRooms()
  const { mode, token } = useConnection()
  // dock 完整呈现全部主入口（世界：消息/联系人/发现；执行端：我的执行端）：
  // 图标不是数据，未接通的页面（如发现）由
  // FixtureRouteGuard 渲染诚实「尚未提供」页兜底，绝不因入口存在而混入 fixtures
  // 假数据。分组只为呈现——两端的入口集合来自同一份 `navGroupsFor(...)` 派生
  // （⚠ 2026-10-03：真实世界下它会把"点进去必然撞门"的入口摘掉 —— 见 `navGroups.ts`）。
  const connect = useAppStore((s) => s.connect)
  const me = useAppStore((s) => s.me)
  // 真实世界（core 直连）：只用真实身份；`me` 暂缺时渲染中性
  // **非 fixture** 占位。演示通道（未连接）才回退 fixtures 的自己。
  // TASK_56：判据原先写的是 `mode === 'host'` —— 于是 core 直连的真实用户在
  // 身份还没读出来时，会看到一个**假自己**（`p-inori`）。
  const self = me
    ? toParticipant(me.participant)
    : isRealWorldSession({ mode, token })
      ? null
      : getParticipant(SELF_ID)
  const totalUnread = rooms.reduce((sum, r) => (r.muted ? sum : sum + r.unreadCount), 0)
  const unreadLabel = totalUnread > 99 ? '99+' : totalUnread
  useUnreadIndicators(totalUnread)

  useEffect(() => {
    void connect()
  }, [connect])

  const navigate = useNavigate()
  // **真实世界里进得去的入口**：判据在路由表（`reachableInRealWorld`），这一层只过滤 ——
  // 真实世界下不摆"点进去必然撞门"的入口（如 `发现`，它 `side: 'demo'`）。
  const groups = navGroupsFor(isRealWorldSession({ mode, token }))
  // 真实世界下 me 暂缺时的中性非 fixture 占位头像。
  const selfNode = self ? (
    <ParticipantAvatar participant={self} size="lg" />
  ) : (
    <div className="flex size-9 items-center justify-center rounded-full bg-muted text-muted-foreground">
      <UserRound className="size-4" aria-hidden />
    </div>
  )

  useEffect(() => {
    const onFocusRoom = (ev: Event) => {
      const roomId = (ev as CustomEvent<{ roomId: string }>).detail?.roomId
      if (roomId) navigate(`/app/chat/${roomId}`)
    }
    window.addEventListener('celira:focus-room', onFocusRoom)
    return () => window.removeEventListener('celira:focus-room', onFocusRoom)
  }, [navigate])

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden bg-background">
      {/* ⚠ **这里不再画标题栏**（2026-10-20 下午）：窗口栏只有一层，在路由根的
          `AppFrame`（栏 + 提示条 + 内容，高度也由它给）。壳这一层只负责"进世界之后"
          的形状：导航 + 内容 + 移动端 TabBar，根用 `h-full` 填满框给的高度。 */}
      <div className="flex min-h-0 flex-1 flex-col overflow-hidden md:flex-row">
        {/* 桌面图标栏：与移动 TabBar 同源（`navGroupsFor`），上组「世界」、
            下组「家」之间用分隔线呈现分区（设计系统「导航壳」）。
            ⚠ 标记**不在这里**了（2026-10-17）：标题栏已经有品牌，导航栏再放一枚
            就是同一个东西画两遍 ✗（网易云那种形状也是品牌只在标题栏）。 */}
        {/* 轨道与聊天区**同一族表面**（`bg-background`），只有会话列表是 `bg-card`：
            少一处靠 1px 线分层的地方，多一处靠表面 —— 暗色下那几条竖线是最像"仪表盘"的东西
            （判据与量出来的数在 `tokens.css` 的 `.dark` 注释里）。 */}
        <nav
          aria-label="主导航"
          className="hidden w-14 shrink-0 flex-col items-center gap-2 border-r border-border bg-background py-3 md:flex"
        >
        {groups.map((group, groupIndex) => (
          <div
            key={group.id}
            role="group"
            aria-label={group.label}
            className="flex w-full flex-col items-center gap-2"
          >
            {groupIndex > 0 && (
              <div aria-hidden className="my-1 h-px w-6 bg-border" />
            )}
            {group.items.map(({ id, label, icon: Icon, to }) => (
              <Tooltip key={id}>
                <TooltipTrigger asChild>
                  <NavLink
                    to={to}
                    aria-label={label}
                    className={cn(
                      'relative cursor-pointer rounded-xl p-2.5 transition-colors',
                      location.pathname.startsWith(to)
                        ? 'bg-accent text-accent-foreground'
                        : 'text-muted-foreground hover:bg-muted hover:text-foreground',
                    )}
                  >
                    <Icon className="size-5" />
                    {id === 'messages' && totalUnread > 0 && (
                      <UnreadBadge label={unreadLabel} className="absolute -top-0.5 -right-0.5" />
                    )}
                  </NavLink>
                </TooltipTrigger>
                <TooltipContent side="right">{label}</TooltipContent>
              </Tooltip>
            ))}
          </div>
        ))}
        {/* 底部用户区（Discord 式）：主题 / 设置直达 / 用户头像。
            设置与用户身份同区——设置本质即「关于我和这台设备」。
            ★ **2026-10-21 尺寸对齐**：三颗一律 40 命中面 / 20 图标（主题那颗的 `size-5`
            由 `ThemeToggle` 自己按 `icon-lg` 给），与上面导航项的 40/20 同一个尺度与 48 节奏。
            判据不是"看起来齐"，是**层级自洽**：移动端「我的」是 TabBar 的一等 tab，
            桌面上它就不该缩成次级按钮（实测原先 36/16 ⇒ 描边细 25%、节奏 44 对 48）。
            圆角也一并对齐：导航项是 `rounded-xl`（14px），按钮默认 `rounded-md`（8px）——
            40 的面上 6px 的差看得出来。 */}
        <div className="mt-auto flex flex-col items-center gap-2">
          <ThemeToggle size="icon-lg" className="rounded-xl" />
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-lg"
                aria-label="设置"
                className="cursor-pointer rounded-xl"
                // ★ **2026-10-21：这里没有「当前页」样式**（那一行 `startsWith('/settings')` 是死判断，
                // 已删）。判据在 `App.tsx` 的路由表：`/settings` 与 `/app` 是**并列的两个壳**，
                // 设置页不经过这条图标栏 ⇒ 它永远不可能"亮在栏里"。给一个永不成立的激活态
                // 只会让下一个读代码的人以为设置长在栏内。
                //
                // 代价要一起还：点进去整条栏就没了。所以**把来路交给设置页**（`state.from`），
                // 让它的「返回」回得来你刚才那间房 —— 否则这就是一扇单向门
                // （来源那个坑：`/settings` 会 `replace` 到 `/settings/general`，
                //   `SettingsIndex` 必须把 state 转发过去，不然这里传的 `from` 会丢）。
                onClick={() => navigate('/settings', { state: { from: location.pathname } })}
              >
                <Settings className="size-5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="right">设置</TooltipContent>
          </Tooltip>
          <NavLink
            to="/app/profile"
            aria-label="我的"
            className={({ isActive }) =>
              cn('cursor-pointer rounded-full transition-shadow', isActive && 'ring-2 ring-ring')
            }
          >
            {selfNode}
          </NavLink>
        </div>
      </nav>

      <main className="flex min-h-0 min-w-0 flex-1 flex-col">
        <div className="min-h-0 flex-1">
          <Outlet />
        </div>
      </main>

      {/* 移动端底部 TabBar：只在一级 tab 页出现，二级沉浸页隐藏。
          与桌面图标栏**同源**（`navGroupsFor`）：两组按顺序排出，组间一条
          竖分隔线；「我的」沿用既有底部入口，排在两组之后。窄屏 7 个入口时
          标签允许截断（`min-w-0` + `truncate`），绝不把栏撑破。 */}
      <nav
        aria-label="主导航"
        className={cn(
          'h-14 shrink-0 items-stretch border-t border-border bg-card',
          hideMobileTabBar ? 'hidden' : 'flex md:hidden',
        )}
      >
        {groups.map((group, groupIndex) => (
          <Fragment key={group.id}>
            {groupIndex > 0 && (
              <div aria-hidden className="my-2 w-px self-stretch bg-border" />
            )}
            {group.items.map((tab) => (
              <NavLink
                key={tab.id}
                to={tab.to}
                aria-label={tab.label}
                className={({ isActive }) =>
                  cn(
                    'flex min-w-0 flex-1 cursor-pointer flex-col items-center justify-center gap-0.5 transition-colors',
                    isActive ? 'text-primary' : 'text-muted-foreground',
                  )
                }
              >
                <span className="relative">
                  <tab.icon className="size-5" />
                  {tab.id === 'messages' && totalUnread > 0 && (
                    <UnreadBadge
                      label={unreadLabel}
                      size="sm"
                      className="absolute -top-1.5 -right-2"
                    />
                  )}
                </span>
                <span className="w-full truncate px-0.5 text-center text-micro">{tab.label}</span>
              </NavLink>
            ))}
          </Fragment>
        ))}
        <NavLink
          to="/app/profile"
          aria-label="我的"
          className={({ isActive }) =>
            cn(
              'flex min-w-0 flex-1 cursor-pointer flex-col items-center justify-center gap-0.5 transition-colors',
              isActive ? 'text-primary' : 'text-muted-foreground',
            )
          }
        >
          {self ? (
            <ParticipantAvatar participant={self} size="sm" />
          ) : (
            <UserRound className="size-5" aria-hidden />
          )}
          <span className="w-full truncate px-0.5 text-center text-micro">我的</span>
        </NavLink>
      </nav>
      </div>
    </div>
  )
}
