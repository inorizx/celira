import { MessageSquarePlus, UserPlus } from 'lucide-react'
import { useNavigate } from 'react-router'
import { Button } from '@/components/ui/button'

/**
 * 「还没选中一间房」那一块空状态（聊天主区）。
 *
 * ⚠ 2026-10-05（走用户视角走出来的第一条摩擦）：**新用户的第一屏就是这里**，而这时他
 * **一支 Agent 都还没有** —— 原来这一屏只说"从左侧列表挑一个房间"，可他左边**没有房间**，
 * 也没有任何地方告诉他"房间是从一支 Agent 开始的"。所以多一档：**一支 Agent 都没有**时，
 * 这一屏改成**指路**（去「我的 Agent」建一支），并把他直接送过去。
 *
 * 判据由调用方给（`noAgents`）——**页面手上的事实，组件不自己猜**；有 Agent 时照旧是原来那句。
 */
export function EmptyRoomsState({
  onCreateRoom,
  noAgents = false,
}: {
  onCreateRoom?: () => void
  noAgents?: boolean
}) {
  const navigate = useNavigate()

  if (noAgents) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-5 px-6 py-16 text-center">
        <div className="rounded-2xl bg-muted p-4">
          <UserPlus className="size-7 text-muted-foreground" />
        </div>
        <div className="space-y-2">
          <div className="text-title">先有一支 Agent，才有会话</div>
          <p className="max-w-sm text-body text-muted-foreground">
            这个 app 里说话的对象是你的 Agent。你还一支都没有 —— 去「我的 Agent」建一支，
            它就会出现在左边的列表里。
          </p>
        </div>
        <Button onClick={() => navigate('/app/agents')} className="gap-1.5">
          <UserPlus className="size-3.5" />
          去建一支 Agent
        </Button>
      </div>
    )
  }

  return (
    <div className="flex h-full flex-col items-center justify-center gap-5 px-6 py-16 text-center">
      <div className="rounded-2xl bg-muted p-4">
        <MessageSquarePlus className="size-7 text-muted-foreground" />
      </div>
      <div className="space-y-2">
        <div className="text-title">选择一个会话</div>
        <p className="max-w-xs text-body text-muted-foreground">
          从左侧列表挑一个房间。
        </p>
      </div>
      <Button onClick={onCreateRoom}>新建会话</Button>
    </div>
  )
}
