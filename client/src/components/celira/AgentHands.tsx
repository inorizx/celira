/**
 * **这一支在这台机器上的终端权限** —— 机器那一段里的一行。
 *
 * 屏幕上的词是**权限的语言**（2026-10-20 主人：「啥叫给它那只手……应该叫类似权限什么的」）：
 * 一句「终端权限」就说清了"这是什么、"给/不给"意味着什么——而"手"只说了个形状，
 * 人得自己去猜它能干什么。（内部标识符仍叫 `hands`，改名欠账见 `api/agentHands.ts` 文件头。）
 *
 * 三条判据都在 `api/agentHands.ts` 里（读数收敛、人话、三态），这里只做组合：
 *
 * - 没有壳 / 不住在这台机器上 ⇒ **整行不出场**（不是显示一个关着的开关）；
 * - 读不到 ⇒ 只印那句"读不到"，**不许画成"关着"**；
 * - 开关 = 一个布尔，旁边常驻后果那一句与生效那半句（不弹确认框：它不是不可逆动作，
 *   关回去就是关回去）。
 */

import { useEffect, useState } from 'react'

import { Switch } from '@/components/ui/switch'
import {
  HANDS_APPLIES,
  HANDS_CONSEQUENCE,
  handsReadoutLine,
  hasHandsBridge,
  readHands,
  setHands,
  type HandsView,
} from '@/api/agentHands'

export function AgentHandsRow({ agentId }: { agentId: string }) {
  const [view, setView] = useState<HandsView | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [read, setRead] = useState(false)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    if (!hasHandsBridge()) {
      setRead(true)
      return
    }
    let alive = true
    void readHands(agentId).then((next) => {
      if (!alive) return
      setRead(true)
      if (next.kind === 'ready') setView(next.view)
      else if (next.kind === 'error') setError(next.message)
    })
    return () => {
      alive = false
    }
  }, [agentId])

  if (!read) return null
  if (view === null) {
    return error === null ? null : (
      <p className="border-t border-border pt-3 text-caption text-muted-foreground">{error}</p>
    )
  }

  const toggle = async (next: boolean) => {
    if (busy) return
    setBusy(true)
    setError(null)
    const result = await setHands(agentId, next)
    if (result.kind === 'ready') setView(result.view)
    else if (result.kind === 'error') setError(result.message)
    setBusy(false)
  }

  return (
    <div className="space-y-1.5 border-t border-border pt-3">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-caption text-muted-foreground">终端权限</span>
        <span className="text-body">{handsReadoutLine(view)}</span>
        {view.arm && (
          <Switch
            className="ml-auto"
            checked={view.granted}
            disabled={busy}
            onCheckedChange={(next) => void toggle(next)}
            aria-label="允许这一支在这台机器上使用终端"
          />
        )}
      </div>
      <p className="text-micro text-muted-foreground">{HANDS_CONSEQUENCE}</p>
      <p className="text-micro text-muted-foreground">{HANDS_APPLIES}</p>
      {error !== null && <p className="text-caption text-destructive">{error}</p>}
    </div>
  )
}
