import { Wrench } from 'lucide-react'
import { Outlet, useNavigate } from 'react-router'
import { Button } from '@/components/ui/button'
import { isRealWorldSession } from '@/api/prefs'
import { useConnection } from '@/api/store'

/**
 * **真实世界守卫**（TASK_56）——「这一页在真实世界里能不能看」的**唯一判据落点**。
 *
 * ---
 * ## 它修的是什么
 *
 * 这里原先叫 `HostRouteGuard`，判据写的是 `mode === 'host'`：
 *
 * ```tsx
 * if (mode === 'host') return <HostUnavailable />   // ← 只挡了 Host
 * ```
 *
 * 而**真实世界有两条入口**（本机 Host 会话、core 直连），这个判据只认其中一条。
 * 后果（实测）：core 直连的真实用户进「发现」，看到的是 fixtures 假居民，
 * 点「打招呼」跳进一个不存在的房间 `/app/chat/r-guide`。
 * **根因不是漏了某一个页面，是判据写错了概念**：把「这是不是真实世界」写成了
 * 「这是不是 Host 模式」。
 *
 * ## 为什么判据不在这里自己写
 *
 * 判据本体只有一份：`api/prefs.ts` 的 `isRealWorldSession`（附它为什么不能只看
 * `mode`：演示通道 `browseDemo()` 也写 `mode = 'core'`）。本组件只做一件事——
 * **真实世界 → 诚实缺省；演示通道 → 放行**。删 Host（C 刀）时这里一个字都没改。
 *
 * ## 演示通道为什么必须放行
 *
 * 登录页那句「不连接，先看演示界面」进的就是这条路（`/gallery`）。fixtures 是
 * 那个通道**唯一的内容来源**，拦掉它演示就成了空白页——**fixtures 只该活在演示
 * 通道里，而不是被删掉**。
 */
export function FixtureRouteGuard() {
  const { mode, token } = useConnection()
  if (isRealWorldSession({ mode, token })) return <FixturePageUnavailable />
  return <Outlet />
}

/**
 * fixture 驱动页在真实世界里的**诚实门**（原 `HostUnavailable` 的通用化）。
 *
 * 它说清三件事，缺一件都算不诚实：① 这一页现在没有真实数据；② **这不是故障**
 * （真实数据本来就还没有这一面，不是加载失败）；③ 给一个出口。
 *
 * 措辞纪律：**不承诺"即将提供"、不假装"正在加载"、不展示任何 fixtures 兜底**。
 *
 * ⚠ **标题那句文案有第二个读者**：仓外的正门验收工具
 * `D:/celira-front-door-accept/check.mjs` 把它抄成了 `FIXTURE_GATE_TEXT`
 * 做**正向断言**（S5）。**改这里的标题就要同步改那边**——否则那条断言会静默失效
 * （TASK_56 就吃过一次：门禁页改名的同时，那条断言还写着旧文案"不在场"）。
 */
export function FixturePageUnavailable() {
  const navigate = useNavigate()
  return (
    <div className="flex h-full min-h-0 flex-col items-center justify-center gap-4 bg-background px-6">
      <Wrench className="size-6 text-agent" aria-hidden />
      <div className="max-w-sm space-y-2 text-center">
        <h1 className="text-title">本页面尚未提供真实数据</h1>
        <p className="text-body text-muted-foreground">
          这一页目前只有演示内容。你已经连着真实的世界，所以这里不给演示。
        </p>
      </div>
      <Button variant="outline" onClick={() => navigate('/app/chat', { replace: true })}>
        返回消息
      </Button>
    </div>
  )
}
