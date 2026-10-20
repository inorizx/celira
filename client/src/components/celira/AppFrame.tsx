import type { ReactNode } from 'react'
import { TitleBar } from './TitleBar'

/**
 * **窗口框**：全应用**唯一的一层**「窗口最上面那一条」。
 *
 * ## 它治的是什么
 *
 * 主人真机两连问，同一个病：
 * ① 「为啥这个界面没有那个比如关闭啊什么按钮」（2026-10-20 上午：那时标题栏只挂在**进世界之后**
 *    那一层，于是启动 / 命名 / 正门几屏一个窗口按钮都没有）；
 * ② 「你觉得这个提示的位置说得过去吗。居然能在关闭窗口的上面的那种」（同日下午：换世界那条提示
 *    长在了**最小化/关闭那一排的上面**）。
 *
 * 两次的根都是同一件：**窗口栏挂在了某一屏上，而不是挂在一个框上**。因为框不在：
 * - 每一屏只好自己写 `h-screen` 把窗口撑满（`DESIGN_SYSTEM.md` 那条「页面根用 `h-full`」当时
 *   根本成立不了 —— 没有框给高度）；
 * - 于是任何「屏与屏之间」的东西（那条换世界提示）只能排在 `AppShell` **外面** ⇒ 它成了
 *   窗口最上面那一条，把三颗窗口按钮挤到第二行；
 * - 而且 `h-screen` ＋ 33px 的提示条 ＝ 100vh + 33px ⇒ 整页多出一根滚动条（真机截图里看得见）。
 *
 * ## 判据
 *
 * 1. **`<TitleBar />` 只准出现在这里**（`tests/shell-titlebar.test.ts` 钉着）。出现两处 = 两条栏；
 *    一处都不出现 = 那一屏没有关闭按钮。这一层在路由根上，所以**每一屏都有**，不用一屏一屏补；
 * 2. **提示条排在标题栏之后、内容之前**：窗口最上面那一条永远归窗口（品牌 + 三颗按钮）；
 *    「换了世界」「这条线断了」是**这一趟旅程的状态**，属于内容，长在标题栏**下面**；
 * 3. **高度由这一层给**：页面根用 `h-full`（`flex h-full min-h-0 flex-col`），不再自己写 `h-screen`
 *    —— 多出来的一条（提示条 / 以后别的条）从这里分走高度，页面自己缩，不再顶出一根全页滚动条；
 * 4. 浏览器里 `TitleBar` 自己返回 null（`isTauriRuntime`）⇒ web 版布局一行都不变（同一份产物两个环境）。
 */
export function AppFrame({ children, notice }: { children: ReactNode; notice?: ReactNode }) {
  return (
    <div className="flex h-screen flex-col bg-background">
      <TitleBar />
      {notice}
      {/* `min-h-0` 不可省：flex 子项默认 `min-height: auto` 会被内容撑高，提示条一出现就会把
          这一格顶出窗口（那正是修之前那根滚动条的来源）。 */}
      <div className="flex min-h-0 flex-1 flex-col">{children}</div>
    </div>
  )
}
