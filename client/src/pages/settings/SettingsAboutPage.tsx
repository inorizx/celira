import { useEffect, useState } from 'react'
import { Info } from 'lucide-react'

import { appVersionLine, readAppVersion, type AppVersionReadout } from '../../api/appVersion.ts'

/**
 * 关于页：产品身份说明（无操作）。
 *
 * ⚠ **版本号从壳里来**（`app_version_readout` 读 exe 旁边的 `VERSION`）。这一格从前是写死的
 * `v0.1.0` —— 发一版就撒一次谎；现在读不到就说"版本读不到"，**不编一个号**。
 */
export function SettingsAboutPage() {
  // `undefined` = 还没问到（首帧），`null` = 壳里读不到 ⇒ 如实说。
  const [version, setVersion] = useState<AppVersionReadout | null | undefined>(undefined)

  useEffect(() => {
    let alive = true
    void (async () => {
      const readout = await readAppVersion()
      if (alive) setVersion(readout)
    })()
    return () => {
      alive = false
    }
  }, [])

  return (
    <div className="flex items-center gap-3 rounded-xl border border-border bg-card px-5 py-4">
      <Info className="size-4 shrink-0 text-muted-foreground" />
      <div>
        <div className="text-body">Celira</div>
        <div className="text-caption text-muted-foreground">
          {version === undefined ? '版本…' : appVersionLine(version)} 视觉演示 · Human 与 Agent 长期共处的通信空间
        </div>
      </div>
    </div>
  )
}
