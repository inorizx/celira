import { useNavigate } from 'react-router'
import { Button } from '@/components/ui/button'
import { CeliraLogo } from '@/components/celira/CeliraLogo'

export function NotFoundPage() {
  const navigate = useNavigate()
  return (
    <div className="flex h-screen flex-col items-center justify-center gap-6 bg-background px-6">
      <CeliraLogo className="size-14" />
      <div className="space-y-2 text-center">
        <div className="text-display">404</div>
        <p className="text-body text-muted-foreground">这个地址还没有居民入住。</p>
      </div>
      <div className="flex gap-3">
        <Button onClick={() => navigate('/app/chat')}>回到消息</Button>
        <Button variant="outline" onClick={() => navigate(-1)}>
          返回上一页
        </Button>
      </div>
    </div>
  )
}
