import { Loader2 } from 'lucide-react'

/**
 * 启动探测加载态：**首屏归属定案之前**渲染，绝不提前展示 Splash 或 fixtures 内容。
 *
 * 措辞刻意**不含「本机 Host」**：定案前还不知道这台机器会给什么，替它说
 * 「正在连接本机 Host」就是又一次默认假设（TASK_49）。Host 全托管路径退役后
 * 这条纪律照样成立——现在要定的是"有没有可直接复用的直连凭据"。
 *
 * TASK_64 从 `components/celira/HostGate.tsx` 搬出来并改名：那个文件是 Host 线的
 * 门（`HostGate`），已随刀整份删除；只有这个加载态与宿主无关，所以在搬家名单里。
 */
export function StartupLoading() {
  return (
    <div className="flex h-screen flex-col items-center justify-center gap-6 bg-background px-6">
      <img src="/logo/logo-gradient.svg" alt="Celira" className="size-20 md:size-24" />
      <div className="flex items-center gap-2 text-muted-foreground">
        <Loader2 className="size-5 animate-spin" aria-hidden />
        <span className="text-body">正在准备连接…</span>
      </div>
    </div>
  )
}
