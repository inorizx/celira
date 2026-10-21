import { useState } from 'react'
import { Outlet, useLocation, useNavigate, Navigate } from 'react-router'
import {
  ArrowLeft,
  BellRing,
  ChevronRight,
  Info,
  Palette,
  Server,
  Settings2,
} from 'lucide-react'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'

// 设置信息架构：分组目录（Discord / VS Code 式），扩展 = 往分组里加条目。
//
// **语义边界（TASK_52 壳层分区；2026-09-23 骨架刀修订；2026-10-03 轴修；2026-10-11 收口）**：
// 设置区收敛为「关于我和这台设备」，分组**与主导航同一条轴**（`components/celira/navGroups.ts`）：
// **这个世界**（你连着的 core：服务器地址与状态）／**你的机器**（机器级事实：本机服务、家钥匙）
// —— 前者换成微信/QQ 仍成立，后者不成立（`routeRegistry` 的那条判据）。
// ⚠ **判据是那条，而不是"分组一定要有两个"**：世界侧的东西现在**都在「通用」那一页里**
// （地址与 token 在那里可编辑）⇒ 目录里就不再有「这个世界」这一组（**空组不留**）。
// 这一条由 `tests/settings-axis.test.ts` 顺着 `routeRegistry` 的 `planes` 机械钉住。
//
// ★ **「模型配置」2026-10-11 搬出设置区**：它 2026-10-03 曾从「Agent 与服务器」搬进「你的机器」
// （判据正确：它管的是**某台机器**的钥匙清单）。但那一次只搬了**分组**、没搬**地址** ——
// 而一个"配某台机器"的页面住在没有机器上下文的 `/settings/providers` 里，就只能自己长一行
// "配哪台机器"的选择器，等于**同一个问题有两个答案**。
// 现在它**按机器寻址**（`/app/runtimes/:entryId/config`）：配置跟着它的对象走，那一行随之退场。
// 旧地址保留成**别名**（带 `?home=<id>` 落那台；没有上下文落执行端列表，**不猜**）。
// ⚠ 这一页**不再出现在本目录里**：入口归**机器页的「模型」间**（那里本来就有"去配置模型"）。
//
// **「家」**（`/settings/homes`）：**2026-10-12 退役** —— 它是**同一份机器清单的第二个门** ✗
// （与一级入口 `/app/runtimes` 列同一批条目、各说一半）。清单现在只有一处，见下面 `NAV_GROUPS`
// 那一项的注释；地址留成别名。
type NavItem = {
  to: string
  label: string
  icon: typeof Settings2
}

const NAV_GROUPS: { title: string; items: NavItem[] }[] = [
  {
    title: '账号与通用',
    items: [
      { to: '/settings/general', label: '通用', icon: Settings2 },
      { to: '/settings/notifications', label: '通知', icon: BellRing },
      { to: '/settings/appearance', label: '外观', icon: Palette },
    ],
  },
  {
    // **「你的机器」**（机器级事实一族）：这一组里每一项的主语都是**这台机器** ——
    // 「本机服务」是这台机器上的进程与本机记录，「家」是这台浏览器记得哪些执行端。
    //
    // ★ **2026-10-11 第三刀：那一项从「这个世界」挪进来了**（判据与搬走「模型配置」**同一条**）：
    //   本机服务端读数 = **这台机器上的进程**（把 im 换成微信/QQ，这一页就没有了）
    //   ⇒ `routeRegistry` 里它本来就写着 `planes: ['home']`，而**组标题说的是「这个世界」** ✗。
    //   上一次只搬了模型的**地址**、没管这一项 ⇒ 同一条推理在这一格**没走完**。
    // ⚠ 名字也换了（撞车）：「服务器」这三个字在设置区指两件事 —— `/settings/general` 里那个
    //   "服务器"是**你连的世界地址**，这个却是**这台机器上的进程**。现在叫「**本机服务**」，
    //   与这一页自己那一行、以及机器页「运行」那一间**用同一个词**。
    // ⚠ 空了的那一组（「这个世界」）随之**删掉**：**空的分组不该留在目录里**。设置区里
    //   世界侧的东西只有"你连哪儿"（服务器地址 / token），它本来就在「通用」里、那里可编辑，
    //   **权威也只在那里**。
    title: '你的机器',
    items: [
      { to: '/settings/server', label: '本机服务', icon: Server },
      // ★ **2026-10-12：「家」这一项退役**（机器清单**只留一个门**）：清单、加一台、管一台
      //   现在都在一级入口 `/app/runtimes` 那一侧（加：清单页那颗「添加」就地开；
      //   管：机器那一页的「这台机器的设置」就地展开）。这一页从前是**同一份清单的第二个门** ✗
      //   —— 两页列同一批条目、各说一半，人要记得"加一台去设置、看状态回执行端"。
      //   ⚠ 地址留成**别名**（`/settings/homes` ⇒ `/app/runtimes`，见 `App.tsx`）：旧书签照旧到得了。
    ],
  },
  {
    title: '系统',
    items: [{ to: '/settings/about', label: '关于', icon: Info }],
  },
]

/**
 * 设置布局：
 * - 桌面端「左侧分组目录 + 右侧内容」双栏（对齐 Discord / VS Code / 微信 PC）；
 * - 移动端 /settings 是分组卡片列表首页（iOS 设置式组簇），子页为带返回的二级页。
 * 两端共享同一份 NAV_GROUPS，保证信息架构一致；差异只在呈现形态。
 */
export function SettingsLayout() {
  const location = useLocation()
  const navigate = useNavigate()
  // ★ **2026-10-21：设置页记住来路**。`/settings` 与 `/app` 是并列的两个壳（见 `App.tsx`），
  // 进来之后那条图标栏就没了 ⇒ 「返回」若写死一个地址，这里就是一扇**单向门**：
  // 从某间房点设置，回来落在「我的」，房间要再点两下。
  // ⇒ 来源由入口传（`state.from`，`AppShell` 的「设置」那颗），深链接/直接打开时缺省落「我的」。
  // ⚠ 它必须在**设置区自己的子导航**里一路带着走（目录点条目、移动端「返回设置」），
  //   否则点两个来回就丢；桌面端还有一条 `replace` 重定向（`SettingsIndex`）同样要转发。
  const from = (location.state as { from?: string } | null)?.from
  const originState = from ? { from } : undefined
  const groups = NAV_GROUPS
  const isIndex = location.pathname === '/settings'
  const current = groups.flatMap((g) => g.items).find((item) => location.pathname.startsWith(item.to))

  return (
    // 高度由窗口框给（`components/celira/AppFrame.tsx` 的 `flex-1` 那一格）；本页只填满
    // ⇒ `h-full min-h-0`，不写 `h-screen`（写了就会把标题栏那 36px 加成一整页的溢出）。
    <div className="flex h-full min-h-0 bg-background">
      {/* 目录栏：桌面常显为窄栏；移动端仅在 /settings 首页全宽显示 */}
      <div
        className={cn(
          'h-full flex-col border-border md:flex md:w-60 md:shrink-0 md:border-r md:bg-card',
          isIndex ? 'flex w-full' : 'hidden',
        )}
      >
        <header className="flex items-center gap-2 border-b border-border bg-card px-4 py-4 md:border-b-0">
          <Button
            variant="ghost"
            size="icon"
            onClick={() => navigate(from ?? '/app/profile')}
            aria-label="返回"
          >
            <ArrowLeft className="size-4" />
          </Button>
          <div className="text-title">设置</div>
        </header>
        {/* min-h-0：flex 子项默认 min-height:auto 会被内容撑高，导致整页滚动、
            左栏只覆盖首屏——必须显式允许收缩，滚动才发生在 ScrollArea 内部 */}
        <ScrollArea className="min-h-0 flex-1">
          <nav className="flex flex-col gap-5 p-3" aria-label="设置目录">
            {groups.map((group) => (
              <div key={group.title}>
                <div className="px-3 pb-1.5 text-micro tracking-wider text-muted-foreground uppercase">
                  {group.title}
                </div>
                {/* 移动端：组内条目聚合成一张圆角卡片（iOS 设置式组簇）；
                    桌面端：透明背景按钮列表（Discord 式目录） */}
                <div className="overflow-hidden rounded-xl border border-border bg-card md:rounded-none md:border-0 md:bg-transparent">
                  <div className="flex flex-col divide-y divide-border md:gap-1 md:divide-y-0">
                    {group.items.map((item) => {
                      const active = location.pathname.startsWith(item.to)
                      return (
                        <button
                          key={item.to}
                          type="button"
                          onClick={() => navigate(item.to, { state: originState })}
                          className={cn(
                            'flex w-full cursor-pointer items-center gap-3 px-4 py-3 text-left transition-colors md:rounded-lg md:px-3 md:py-2.5',
                            active
                              ? 'bg-accent font-medium text-accent-foreground'
                              : 'text-foreground hover:bg-muted/60 md:text-muted-foreground md:hover:text-foreground',
                          )}
                        >
                          <item.icon
                            className={cn(
                              'size-4 shrink-0',
                              active ? 'text-accent-foreground' : 'text-muted-foreground',
                            )}
                          />
                          <span className="text-body">{item.label}</span>
                          <ChevronRight className="ml-auto size-4 shrink-0 text-muted-foreground md:hidden" />
                        </button>
                      )
                    })}
                  </div>
                </div>
              </div>
            ))}
          </nav>
        </ScrollArea>
      </div>

      {/* 内容栏：桌面常显；移动端仅在子页显示 */}
      <div className={cn('h-full min-w-0 flex-1 flex-col', isIndex ? 'hidden md:flex' : 'flex')}>
        <header className="flex items-center gap-2 border-b border-border bg-card px-4 py-4 md:hidden">
          <Button
            variant="ghost"
            size="icon"
            onClick={() => navigate('/settings', { state: originState })}
            aria-label="返回设置"
          >
            <ArrowLeft className="size-4" />
          </Button>
          <div className="text-title">{current?.label ?? '设置'}</div>
        </header>
        <ScrollArea className="min-h-0 flex-1">
          <div className="mx-auto w-full max-w-3xl space-y-6 px-4 py-6">
            <Outlet />
          </div>
        </ScrollArea>
      </div>
    </div>
  )
}

/** /settings 索引：桌面端默认落到「通用」；移动端列表首页由布局目录栏渲染，内容区为空。 */
export function SettingsIndex() {
  const [isDesktop] = useState(() => window.matchMedia('(min-width: 768px)').matches)
  // ⚠ **这条 `replace` 会把 `location.state` 一起吞掉**（`Navigate` 不带 state）。
  // 桌面端每次从图标栏点「设置」都走这条路 ⇒ 不转发的话，`AppShell` 传的 `from`
  // 正好在这里蒸发，「返回」永远退回「我的」（这正是它原先的表现）。
  const location = useLocation()
  if (isDesktop) return <Navigate to="/settings/general" replace state={location.state} />
  return null
}
