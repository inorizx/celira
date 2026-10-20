import { useNavigate } from 'react-router'
import { Button } from '@/components/ui/button'

/**
 * 品牌启动页。
 *
 * ⚠ 「进入」原先指向 `/identity`（一条**承诺创建 Human 身份、实际什么都不创建**的
 * 演示表单）。那条路由已于 2026-09-25 删除 —— 真实身份走 CLI 签发
 * （`celira-im create-human`，设置页也这么写），而演示身份不该假装成"创建"。
 *
 * 现在它回**正门**（`/`）：那里由 `RootLayout` 按 `api/entry.ts` 的判定分派 ——
 * 手里有可复用的直连凭据就自动连，否则给登录页（服务器地址 + token）。
 * 那才是"进入 Celira"的真实第一步。
 */
export function SplashPage() {
  const navigate = useNavigate()
  return (
    <div className="flex h-full min-h-0 flex-col items-center justify-center gap-8 bg-gradient-to-br from-brand/10 via-background to-agent/10 px-6">
      <img src="/logo/logo-gradient.svg" alt="Celira" className="size-24 md:size-28" />
      <div className="space-y-3 text-center">
        <h1 className="text-display">Celira</h1>
        <p className="max-w-sm text-body text-muted-foreground">
          Human 与 Agent 长期、平等共处的通信空间
        </p>
      </div>
      <Button size="lg" onClick={() => navigate('/')} className="px-10">
        进入
      </Button>
      <p className="text-caption text-muted-foreground">每个参与者，都有持续的身份</p>
    </div>
  )
}
