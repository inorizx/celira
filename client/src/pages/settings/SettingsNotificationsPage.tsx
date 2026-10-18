import { AtSign, BellRing } from 'lucide-react'
import { Switch } from '@/components/ui/switch'
import { isTauriRuntime } from '@/api/shell'
import { useNotificationSettings } from '@/hooks/use-notifications'

/**
 * 「通知权限被拒」那一句话**分形态**（2026-10-08）：
 *
 * - **浏览器里**：去浏览器设置里开（这句本来就对）；
 * - **壳里**：走的是**系统原生通知**（`api/notifications.ts` 的文件头写了为什么 Web 那一套在壳里
 *   用不了），所以该说的是**系统通知设置** —— 从前那句「请在浏览器设置中开启」在壳里是**误导**：
 *   壳里没有浏览器设置可开，而且它永远开不了。
 */
function deniedHint(): string {
  return isTauriRuntime()
    ? '系统把这个应用的通知关掉了，请在系统的通知设置里打开 Celira'
    : '浏览器已拒绝通知权限，请在浏览器设置中开启'
}

/** 通知设置：新消息通知与提及提醒（平台通知权限 + 本机开关）。 */
export function SettingsNotificationsPage() {
  const { enabled, mentions, permission, requestPermission, setEnabled, setMentions } =
    useNotificationSettings()

  const handleNotifyToggle = async (v: boolean) => {
    // 权限的读取与回写都在 hook 里（页面不该自己碰平台 API）；这里只按结果决定落盘值。
    if (v && !(await requestPermission())) {
      // 没授权：明确落回"关"，并把**实际落盘的值**拨回界面（不许显示成已开启）。
      await setEnabled(false)
      return
    }
    await setEnabled(v)
  }

  const handleMentionsToggle = async (v: boolean) => {
    if (v && !(await requestPermission())) {
      await setMentions(false)
      return
    }
    await setMentions(v)
  }

  return (
    <div className="divide-y divide-border rounded-xl border border-border bg-card">
      <label className="flex cursor-pointer items-center gap-3 px-5 py-4">
        <BellRing className="size-4 shrink-0 text-muted-foreground" />
        <div className="min-w-0">
          <div className="text-body">新消息通知</div>
          <div className="text-caption text-muted-foreground">
            {permission === 'unsupported'
              ? '这个浏览器没有系统通知'
              : permission === 'denied'
                ? deniedHint()
                : isTauriRuntime()
                  ? '窗口不在前台时提醒新消息（Agent 回复 / @提及）'
                  : '页面在后台时提醒新消息（Agent 回复 / @提及）'}
          </div>
        </div>
        <Switch
          className="ml-auto"
          aria-label="新消息通知"
          checked={enabled}
          onCheckedChange={(v) => void handleNotifyToggle(v)}
        />
      </label>
      <label className="flex cursor-pointer items-center gap-3 px-5 py-4">
        <AtSign className="size-4 shrink-0 text-muted-foreground" />
        <div className="min-w-0">
          <div className="text-body">提及提醒</div>
          <div className="text-caption text-muted-foreground">
            仅 @我的消息提醒（新消息通知关闭时也生效）
          </div>
        </div>
        <Switch
          className="ml-auto"
          aria-label="提及提醒"
          checked={mentions}
          onCheckedChange={(v) => void handleMentionsToggle(v)}
        />
      </label>
    </div>
  )
}
