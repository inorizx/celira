import { useEffect, useState } from 'react'
import { Info, RefreshCw } from 'lucide-react'

import { appVersionLine, readAppVersion, type AppVersionReadout } from '../../api/appVersion.ts'
import {
  applyAppUpdate,
  checkAppUpdate,
  downloadAppUpdate,
  installBlocker,
  updateLine,
  type AppUpdateReadout,
} from '../../api/appUpdate.ts'
import { isTauriRuntime } from '../../api/shell.ts'

type Phase = 'idle' | 'checking' | 'downloading' | 'applying'

/**
 * 关于页：产品身份 + 「检查更新」/「下载并更新」。
 *
 * ⚠ **版本号从壳里来**（`app_version_readout` 读 exe 旁边的 `VERSION`）。这一格从前是写死的
 * `v0.1.0` —— 发一版就撒一次谎；现在读不到就说"版本读不到"，**不编一个号**。
 *
 * 更新只在壳里出现：浏览器形态没有"换掉这个程序"这件事（`isTauriRuntime()` 判，
 * 与 `LocalServices` 同款）。两颗按钮分两步：先问（只读），再换（下载/校验/替换）。
 * 换的那一步会让窗口退出——不是崩溃，是让位给替换脚本（见 `api/appUpdate.ts` 的
 * `applyAppUpdate` 注释）。
 */
export function SettingsAboutPage() {
  // `undefined` = 还没问到（首帧），`null` = 壳里读不到 ⇒ 如实说。
  const [version, setVersion] = useState<AppVersionReadout | null | undefined>(undefined)
  const [update, setUpdate] = useState<AppUpdateReadout | null>(null)
  const [phase, setPhase] = useState<Phase>('idle')
  const [failure, setFailure] = useState<string | null>(null)

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
    setPhase('checking')
    setFailure(null)
    try {
      const readout = await checkAppUpdate()
      if (readout) setUpdate(readout)
    } finally {
      setPhase('idle')
    }
  }

  const install = async () => {
    if (!update) return
    setFailure(null)
    setPhase('downloading')
    try {
      const prepared = await downloadAppUpdate(update)
      setPhase('applying')
      await applyAppUpdate(prepared)
    } catch (error) {
      // 失败把壳/渠道的原话显示出来，不翻译成"网络问题"。
      setFailure(error instanceof Error ? error.message : String(error))
      setPhase('idle')
    }
  }

  const canInstall = update !== null && installBlocker(update) === null
  const busy = phase !== 'idle'
  const label =
    phase === 'checking'
      ? '检查中…'
      : phase === 'downloading'
        ? '下载中…'
        : phase === 'applying'
          ? '正在重启…'
          : canInstall
            ? '下载并更新'
            : '检查更新'

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
            {failure && (
              <div className="mt-1 text-caption text-destructive" role="alert">
                {failure}
              </div>
            )}
          </div>
          <button
            type="button"
            onClick={canInstall ? install : check}
            disabled={busy}
            className="flex shrink-0 items-center gap-1.5 rounded-lg border border-border px-3 py-1.5 text-caption hover:bg-accent disabled:opacity-50"
          >
            <RefreshCw className={busy ? 'size-3.5 animate-spin' : 'size-3.5'} />
            {label}
          </button>
        </div>
      )}
    </div>
  )
}
