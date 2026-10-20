import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  DEFAULT_HOME_PORTS,
  homeEntryDraftErrors,
  nextRemoteId,
  normalizeHomeAddress,
  saveHomeEntry,
  type HomeEntry,
  type HomeEntryDraft,
  type HomeEntryDraftError,
} from '@/api/homes'

/**
 * **加一台机器**（名字 + 机器地址 + 端口对 + 机器钥匙）——从 `/settings/homes` 那一页搬进
 * 机器清单（2026-10-12：清单**只留一个门**，所以"加"跟着清单走，不在设置里另开一处）。
 *
 * ⚠ 落盘与校验**只有这一处**（`saveHomeEntry` ＋ `homeEntryDraftErrors`）：别处只带意图过来
 * （清单页那颗「添加」就地开这块表单），不复制流程。
 */
export function AddMachineForm({
  entries,
  onSaved,
  onCancel,
}: {
  /** 现有的条目（只为算下一个 id）。 */
  entries: HomeEntry[]
  onSaved: () => void
  onCancel: () => void
}) {
  const [draft, setDraft] = useState<HomeEntryDraft>({
    name: '',
    baseUrl: '',
    statusPort: '',
    configPort: '',
    key: '',
  })
  const [errors, setErrors] = useState<HomeEntryDraftError>({})
  const [busy, setBusy] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)

  const save = async () => {
    const draftErrors = homeEntryDraftErrors(draft)
    if (Object.keys(draftErrors).length > 0) {
      setErrors(draftErrors)
      return
    }
    setBusy(true)
    setSaveError(null)
    try {
      await saveHomeEntry({
        id: nextRemoteId(entries),
        name: draft.name.trim() || '未命名',
        baseUrl: normalizeHomeAddress(draft.baseUrl),
        // 端口留空 = 用默认（5190/5191；与 `home.conf` 那份默认值同一条纪律）。
        statusPort: draft.statusPort.trim()
          ? Number.parseInt(draft.statusPort, 10)
          : DEFAULT_HOME_PORTS.status,
        configPort: draft.configPort.trim()
          ? Number.parseInt(draft.configPort, 10)
          : DEFAULT_HOME_PORTS.config,
        key: draft.key.trim(),
        local: false,
      })
      onSaved()
    } catch {
      setSaveError('存不进去（这台浏览器写不了本地存储）—— 换个浏览器试试')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-3 rounded-xl border border-border bg-card p-4">
      <div className="text-body">添加一台机器</div>
      <label className="block space-y-1.5">
        <span className="text-caption text-muted-foreground">名字</span>
        <Input
          value={draft.name}
          onChange={(event) => setDraft({ ...draft, name: event.target.value })}
          placeholder="Agent 住的那台机器"
          className="bg-muted"
        />
      </label>
      <label className="block space-y-1.5">
        <span className="text-caption text-muted-foreground">机器地址</span>
        <Input
          value={draft.baseUrl}
          onChange={(event) => {
            setDraft({ ...draft, baseUrl: event.target.value })
            setErrors({ ...errors, baseUrl: '' })
          }}
          placeholder="https://192.168.1.20"
          className="bg-muted"
        />
        {errors.baseUrl && <span className="text-caption text-presence-failed">{errors.baseUrl}</span>}
      </label>
      <div className="grid grid-cols-2 gap-3">
        <label className="block space-y-1.5">
          <span className="text-caption text-muted-foreground">状态口端口</span>
          <Input
            value={draft.statusPort}
            onChange={(event) => setDraft({ ...draft, statusPort: event.target.value })}
            placeholder="5190"
            className="bg-muted"
          />
          {errors.statusPort && (
            <span className="text-caption text-presence-failed">{errors.statusPort}</span>
          )}
        </label>
        <label className="block space-y-1.5">
          <span className="text-caption text-muted-foreground">配置门端口</span>
          <Input
            value={draft.configPort}
            onChange={(event) => setDraft({ ...draft, configPort: event.target.value })}
            placeholder="5191"
            className="bg-muted"
          />
          {errors.configPort && (
            <span className="text-caption text-presence-failed">{errors.configPort}</span>
          )}
        </label>
      </div>
      <label className="block space-y-1.5">
        <span className="text-caption text-muted-foreground">机器钥匙</span>
        <Input
          type="password"
          value={draft.key}
          onChange={(event) => {
            setDraft({ ...draft, key: event.target.value })
            setErrors({ ...errors, key: '' })
          }}
          placeholder="在那台机器上跑 node home.js issue-home-key 拿"
          className="bg-muted"
        />
        {errors.key && <span className="text-caption text-presence-failed">{errors.key}</span>}
      </label>
      {/* **红路径不许静默**：存不进去也要说出口（这块表单是它唯一的现场）。 */}
      {saveError && <p className="text-caption text-presence-failed">{saveError}</p>}
      <div className="flex items-center gap-2">
        <Button size="sm" disabled={busy} onClick={() => void save()}>
          {busy ? '保存中…' : '保存'}
        </Button>
        <Button size="sm" variant="ghost" onClick={onCancel}>
          取消
        </Button>
      </div>
    </div>
  )
}
