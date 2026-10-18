import type { ReactNode } from 'react'
import { ScrollArea } from '@/components/ui/scroll-area'

/**
 * **页面的两个中性布局件**：一个带锚点的分区壳，一条页面级滚动区。
 *
 * ## 为什么单独一个文件（2026-09-26）
 *
 * 它们原先住在 `components/celira/Runtimes.tsx` 里，叫 `RuntimeFrame` / `RuntimesScroll`。
 * 名字没错——它们当时只服务**执行端**那几页（机器页的三个房间）。问题是
 * **世界侧的页面也开始用它们**了：`/app/agents/:participantId`（Agent 主页，
 * `side: 'world'`）拿它们排「资料 / 服务」两间 ⇒ 一个**世界侧**的页面从**执行端**的
 * 组件文件里取布局件。这不是行为错误，是**边界说不清**：读代码的人会以为那一页
 * 也是执行端的。
 *
 * 归属判据（`api/routeRegistry.ts` 文件头：「把 im 换成微信，这一页还成立吗？」）
 * 量的是**页面主体**，而这两个件本身**没有任何一侧的事实** —— 它们只是 `<section>` 与
 * 滚动容器。所以正确的位置是**中性文件**，名字也去掉 `Runtime` 前缀。
 *
 * ⚠ 反过来**不成立**：不要因为"它们现在中立"就把执行端的事实性组件也搬进来 ——
 * `Runtimes.tsx` 里那些（`RunCard` / `MachineReading` / 三个房间）**读的是家**，
 * 必须留在执行端。
 */

/** 一个分区的外壳：锚点 + 分区名 + 一句**这一面说什么**。 */
export function SectionFrame({
  id,
  title,
  icon,
  subject,
  children,
}: {
  id: string
  title: string
  icon: ReactNode
  subject: ReactNode
  children: ReactNode
}) {
  return (
    <section id={id} className="scroll-mt-6 space-y-4">
      <div className="space-y-1">
        <div className="flex items-center gap-2">
          {icon}
          <h2 className="text-title">{title}</h2>
        </div>
        <p className="text-caption text-muted-foreground">{subject}</p>
      </div>
      {children}
    </section>
  )
}

/** 页面级滚动的公共壳（一页里几个分区共用一条滚动区）。 */
export function PageScroll({ children }: { children: ReactNode }) {
  return (
    <ScrollArea className="min-h-0 flex-1">
      <div className="mx-auto w-full max-w-4xl space-y-10 px-4 py-6 md:px-6">{children}</div>
    </ScrollArea>
  )
}
