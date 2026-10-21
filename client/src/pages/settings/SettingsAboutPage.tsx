import { useEffect, useState } from 'react'
import { Info, RefreshCw } from 'lucide-react'

import { appVersionLine, readAppVersion, type AppVersionReadout } from '../../api/appVersion.ts'
import { checkAppUpdate, updateLine, type AppUpdateReadout } from '../../api/appUpdate.ts'
import { isTauriRuntime } from '../../api/shell.ts'

/**
 * 关于页：产品身份 + 「检查更新」。
 *
 * ⚠ **版本号从壳里来**（`app_version_readout` 读 exe 旁边的 `VERSION`）。这一格从前是写死的
 * `v0.1.0` —— 发一版就撒一次谎；现在读不到就说"版本读不到"，**不编一个号**。
 *
 * 「检查更新」只在壳里出现：浏览器形态没有"换掉这个程序"这件事（`isTauriRuntime()` 判，
 * 与 `LocalServices` 同款）。这一颗按钮**只问不换**——它回答"有没有新版"，
 * 下载与替换是后面的事（见 `api/appUpdate.ts` 文件头）。
 */
export function SettingsAboutPage() {
  // `undefined` = 还没问到（首帧），`null` = 壳里读不到 ⇒ 如实说。
  const [version, setVersion] = useState<AppVersionReadout | null | undefined>(undefined)
  const [update, setUpdate] = useState<AppUpdateReadout | null>(null)
  const [checking, setChecking] = useState(false)

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

  const check = async () => {
    setChecking(true)
    try {
      const readout = await checkAppUpdate()
      if (readout) setUpdate(readout)
    } finally {
      setChecking(false)
    }
  }

  return (
    <div className="space-y-3">
      <div className="flex items-center gap-3 rounded-xl border border-border bg-card px-5 py-4">
        <Info className="size-4 shrink-0 text-muted-foreground" />
        <div>
          <div className="text-body">Celira</div>
          <div className="text-caption text-muted-foreground">
            {version === undefined ? '版本…' : appVersionLine(version)} 视觉演示 · Human 与 Agent 长期共处的通信空间
          </div>
        </div>
      </div>

      {isTauriRuntime() && (
        <div className="flex items-center justify-between gap-3 rounded-xl border border-border bg-card px-5 py-4">
          <div className="min-w-0">
            <div className="text-body">更新</div>
            <div className="text-caption text-muted-foreground">
              {update ? updateLine(update) : '检查一下渠道上有没有新版本'}
            </div>
          </div>
          <button
            type="button"
            onClick={check}
            disabled={checking}
            className="flex shrink-0 items-center gap-1.5 rounded-lg border border-border px-3 py-1.5 text-caption hover:bg-accent disabled:opacity-50"
          >
            <RefreshCw className={checking ? 'size-3.5 animate-spin' : 'size-3.5'} />
            {checking ? '检查中…' : '检查更新'}
          </button>
        </div>
      )}
    </div>
  )
}
