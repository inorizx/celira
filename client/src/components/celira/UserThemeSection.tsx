import { useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { ThemeScopePreview } from '@/components/celira/ThemePresetCard'
import { useTheme } from '@/hooks/theme-context'
import { THEME_PRESETS } from '@/lib/themes'
import { auditTheme, type AuditFinding, type FindingLevel } from '@/lib/themeAudit'
import {
  asPreviewTheme,
  parseThemeFile,
  serializeThemeFile,
  PREVIEW_SCOPE_ID,
  THEME_FILE_ERROR_TEXT,
  type ParsedThemeFile,
} from '@/lib/themeFile'
import { auditBaseFor, officialThemeFile } from '@/lib/themeCatalog'
import { officialExportId, type UserThemeRecord } from '@/lib/userTheme'
import { cn } from '@/lib/utils'

/**
 * 「我的主题」：本机装载自己的配色与质感。
 *
 * 这一区是**外观这一层唯一的装载口**——它装的是**数据**（一份 CSS），不是代码：
 * 所以这里没有宿主 API、没有生命周期、没有版本协商，卸载就是删掉一条记录。
 * 判据见 `AGENTS.md` §前端边界的「扩展接缝」。
 *
 * ## 刀 2：装之前先看
 *
 * 分享来的一份主题，先前必须**先装上去**（＝立刻改变整个应用）才看得见它长什么样，
 * 看不中就得删掉。现在多一道小样：同一份主题、作用域换成**保留 id**
 * （`themeFile.ts` 的 `PREVIEW_SCOPE_ID`），靠**嵌套 `data-theme`** 把影响关在卡片里。
 * 两条安全性质：**小样绝不用文件自己那个 id**（预览正在生效中的主题也不会漏到全应用），
 * 而保留 id 挡住了"文件自己就叫它"那条撞车路。**预览不落任何持久面**——它是瞬时 DOM 状态。
 *
 * ## 刀 4：装完之后也看得到（列表给真样张）
 *
 * 刀 2 之前"没装上看不见"，刀 4 之前是**装完了也看不见**——已装那一列只有名字，
 * 想比较两套装哪个得逐个切过去。现在每条都带**真样张**（与官方预设同一处视觉）。
 * 它成立的前提是 `use-theme.tsx` 把**全部**已装主题都注入 `#celira-user-themes`
 * （不是只有生效那份）—— 没生效的那些在文档里但永不命中（作用域只带各自的 id）。
 *
 * ## 三个 `<style>` 各有唯一写者
 *
 * 官方包由启动那一处写 `#celira-theme-packages`（全量、一次性）；本机装的全部用户主题
 * 由 `use-theme.tsx` 写 `#celira-user-themes`（持久）；**小样**由本区写
 * `#celira-theme-preview`（**瞬时**，闸门一关整个移除）。三者作用域不同、生命周期不同，
 * 所以是三个元素而不是一个。
 */

/** 检查结论的档位 → 人话与颜色（`unknown` 与 `ok` **不合并**：没检查 ≠ 没问题）。 */
const LEVEL_TEXT: Record<FindingLevel, string> = {
  ok: '没问题',
  weak: '留意',
  fail: '不过关',
  unknown: '没检查到',
}

const LEVEL_CLASS: Record<FindingLevel, string> = {
  ok: 'text-muted-foreground',
  weak: 'text-presence-timeout',
  fail: 'text-destructive',
  unknown: 'text-muted-foreground',
}

const LEVEL_ORDER: FindingLevel[] = ['fail', 'weak', 'unknown', 'ok']

function summarize(findings: AuditFinding[]) {
  const counts: Record<FindingLevel, number> = { ok: 0, weak: 0, fail: 0, unknown: 0 }
  for (const f of findings) counts[f.level] += 1
  return counts
}

function downloadThemeFile(fileName: string, css: string) {
  const blob = new Blob([css], { type: 'text/css' })
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = fileName
  anchor.click()
  // ⚠ 不能紧接着 revoke：下载是异步取的，立刻回收会让某些浏览器**取消这次下载**。
  setTimeout(() => URL.revokeObjectURL(url), 0)
}

/** 文件名：只留安全字符，免得带出路径分隔符。 */
function safeFileName(name: string) {
  const slug = name.replace(/[^\w\u4e00-\u9fa5-]+/g, '-').replace(/^-+|-+$/g, '')
  return `${slug || 'theme'}.celira-theme.css`
}

const PREVIEW_STYLE_ID = 'celira-theme-preview'

/**
 * 这一区当前那道闸门。**一次只可能有一道**（用联合类型表达，不靠两个互相打架的 state）：
 * - `preview`：小样已渲染，等用户决定装不装；
 * - `installed`：刚装上，把检查结果留在眼前。
 */
type Gate =
  | {
      kind: 'preview'
      /** 被预览的**那一份文本**（装上装的是它，不是可能已被改动过的输入框）。 */
      raw: string
      theme: ParsedThemeFile
      findings: AuditFinding[]
      /** 同 id 的已装主题（装上会替换它）——照实说不猜。 */
      collides: UserThemeRecord | null
      /** 注入用的小样样式（作用域已是保留 id）。 */
      css: string
    }
  | { kind: 'installed'; name: string; findings: AuditFinding[] }

export function UserThemeSection() {
  const {
    theme,
    preset,
    userThemes,
    activeUserThemeId,
    activateUserTheme,
    installUserTheme,
    deleteUserTheme,
  } = useTheme()

  const [draft, setDraft] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [gate, setGate] = useState<Gate | null>(null)
  const [exportPreset, setExportPreset] = useState(preset)
  const fileRef = useRef<HTMLInputElement>(null)

  // 小样的样式注入：**这一个 `<style>` 是唯一写者**，且它是**瞬时**的
  // （闸门一关就整个移除——与生效主题那个"写空串、元素留着"不同，那个要跟到下一次）。
  useEffect(() => {
    const css = gate?.kind === 'preview' ? gate.css : ''
    if (css) {
      let el = document.getElementById(PREVIEW_STYLE_ID) as HTMLStyleElement | null
      if (!el) {
        el = document.createElement('style')
        el.id = PREVIEW_STYLE_ID
        document.head.appendChild(el)
      }
      el.textContent = css
    }
    return () => {
      document.getElementById(PREVIEW_STYLE_ID)?.remove()
    }
  }, [gate])

  const failWith = (message: string) => {
    setError(message)
    setGate(null)
  }

  /** 两道闸门共用的第一关：读文本、过格式（格式不合就拒收——那种文件装载不了）。 */
  const readTheme = (raw: string): ParsedThemeFile | null => {
    if (!raw.trim()) {
      failWith('先贴一份主题，或者选一个文件。')
      return null
    }
    const parsed = parseThemeFile(raw)
    if (!parsed.ok) {
      failWith(THEME_FILE_ERROR_TEXT[parsed.code])
      return null
    }
    return parsed.theme
  }

  /** 闸门一：**先看小样**（不动全局、不落任何持久面）。 */
  const runPreview = async (raw: string) => {
    const theme = readTheme(raw)
    if (!theme) return
    setBusy(true)
    setError(null)
    try {
      setGate({
        kind: 'preview',
        raw,
        theme,
        findings: auditTheme(theme, auditBaseFor(preset)),
        collides: userThemes.find((item) => item.id === theme.id) ?? null,
        css: serializeThemeFile(asPreviewTheme(theme)),
      })
    } finally {
      setBusy(false)
    }
  }

  /** 闸门二：真的装上——**只有这一条安装路径**（不看小样直接装也走它）。 */
  const doInstall = async (raw: string) => {
    setBusy(true)
    setError(null)
    try {
      const outcome = await installUserTheme(raw)
      if (outcome.ok) {
        setGate({ kind: 'installed', name: outcome.record.name, findings: outcome.findings })
        setDraft('')
      } else {
        failWith(outcome.text)
      }
    } finally {
      setBusy(false)
    }
  }

  const installRaw = async (raw: string) => {
    if (!readTheme(raw)) return
    await doInstall(raw)
  }

  /** 小样上那句「就装它」：装的是**被预览的那一份文本**，不是可能已被改动过的输入框。 */
  const confirmInstall = async () => {
    if (gate?.kind !== 'preview') return
    await doInstall(gate.raw)
  }

  const exportCurrentOfficial = async () => {
    const meta = THEME_PRESETS.find((p) => p.id === exportPreset)
    const file = officialThemeFile(exportPreset, meta?.name ?? exportPreset, officialExportId(exportPreset))
    if (!file) {
      setError('这一套官方主题的配色没能读出来，先换一套试试。')
      return
    }
    downloadThemeFile(safeFileName(`${meta?.name ?? exportPreset}-副本`), serializeThemeFile(file))
  }

  const counts = gate ? summarize(gate.findings) : null

  return (
    <div className="space-y-4 rounded-xl border border-border bg-card p-5">
      <div className="flex items-center gap-2">
        <span className="text-body">我的主题</span>
        <span className="ml-auto text-caption text-muted-foreground">
          {userThemes.length > 0 ? `装了 ${userThemes.length} 套` : '还没装'}
        </span>
      </div>
      <p className="text-caption text-muted-foreground">
        装一份自己的配色与质感。主题就是一份 CSS，只改变这台机器上的样子，不会上传，也不动世界里的任何东西。
        别人给你一份，先看小样再决定装不装。
      </p>

      {/* 从一套官方主题起步：用户不必从空白开始 */}
      <div className="space-y-2 rounded-lg border border-border bg-muted/40 p-3">
        <span className="text-caption text-muted-foreground">想改但不想从零写？先导出一套官方的当底稿：</span>
        <div className="flex flex-wrap items-center gap-2">
          <select
            value={exportPreset}
            onChange={(e) => setExportPreset(e.target.value as typeof exportPreset)}
            className="h-8 cursor-pointer rounded-md border border-input bg-transparent px-2 text-caption outline-none focus-visible:border-ring"
            aria-label="选一套官方主题当底稿"
          >
            {THEME_PRESETS.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
          <Button variant="outline" size="sm" onClick={exportCurrentOfficial}>
            导出这份底稿
          </Button>
          <span className="text-micro text-muted-foreground">
            导出的是配色那一份（官方主题的质感与字族不跟着走）。
          </span>
        </div>
      </div>

      {/* 贴进来 / 选文件 */}
      <div className="space-y-2">
        <span className="text-caption text-muted-foreground">把主题贴在这里，或者选一个文件：</span>
        <Textarea
          value={draft}
          onChange={(e) => {
            setDraft(e.target.value)
            // 输入框一改，先前那份小样就不是它了 —— 关掉闸门（留着就是拿旧样张冒充新文本）。
            if (gate?.kind === 'preview') setGate(null)
          }}
          rows={6}
          spellCheck={false}
          placeholder={"/* celira-theme: 我的深空 */\n[data-theme='user-mine'] {\n  --background: …\n}\n.dark[data-theme='user-mine'] {\n  --background: …\n}"}
          className="font-mono text-caption"
        />
        <div className="flex flex-wrap items-center gap-2">
          <Button size="sm" disabled={busy} onClick={() => void runPreview(draft)}>
            {busy ? '正在看…' : '先看小样'}
          </Button>
          <Button variant="outline" size="sm" disabled={busy} onClick={() => void installRaw(draft)}>
            装上
          </Button>
          <Button variant="outline" size="sm" onClick={() => fileRef.current?.click()}>
            选一个文件…
          </Button>
          <input
            ref={fileRef}
            type="file"
            accept=".css,text/css"
            className="hidden"
            onChange={(e) => {
              const file = e.target.files?.[0]
              e.target.value = ''
              if (file) void file.text().then((text) => runPreview(text))
            }}
          />
          <span className="text-micro text-muted-foreground">
            字体只能用这台机器上已有的，不能随主题带字体文件。
          </span>
        </div>
      </div>

      {error && (
        <p className="text-caption text-destructive" role="alert">
          {error}
        </p>
      )}

      {/* 闸门：小样（装之前）／检查结果（刚装上） */}
      {gate && counts && (
        <div className="space-y-3 rounded-lg border border-border bg-muted/40 p-3">
          {gate.kind === 'preview' ? (
            <div className="space-y-2">
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-caption">「{gate.theme.name}」的小样（按当前明暗）：</span>
                {gate.collides && (
                  <span className="text-micro text-presence-timeout">
                    它的 id 与已装的「{gate.collides.name}」相同 —— 装上会替换那一套。
                  </span>
                )}
              </div>
              <ThemeScopePreview scopeId={PREVIEW_SCOPE_ID} dark={theme === 'dark'} />
            </div>
          ) : (
            <span className="text-caption">「{gate.name}」已经装上了。</span>
          )}

          <div className="flex flex-wrap items-center gap-2">
            {LEVEL_ORDER.filter((level) => counts[level] > 0).map((level) => (
              <span key={level} className={cn('text-micro', LEVEL_CLASS[level])}>
                {LEVEL_TEXT[level]} {counts[level]}
              </span>
            ))}
          </div>
          <ul className="space-y-1">
            {gate.findings
              .filter((f) => f.level !== 'ok')
              .map((f, i) => (
                <li key={`${f.kind}-${f.mode}-${i}`} className={cn('text-micro', LEVEL_CLASS[f.level])}>
                  {f.detail}
                </li>
              ))}
          </ul>
          <p className="text-micro text-muted-foreground">
            这些是算出来的检查结果，不影响能不能用 —— 觉得没问题就留着。另一档明暗的结论也在上面。
          </p>

          {gate.kind === 'preview' && (
            <div className="flex flex-wrap items-center gap-2">
              <Button size="sm" disabled={busy} onClick={() => void confirmInstall()}>
                {busy ? '正在装…' : '就装它'}
              </Button>
              <Button variant="ghost" size="sm" onClick={() => setGate(null)}>
                算了
              </Button>
            </div>
          )}
        </div>
      )}

      {/* 已装的：每条给**真样张**（与官方预设同一处视觉）—— 切之前就看得到，
          不然只能逐个切过去比。样张靠嵌套 data-theme，前提是这些主题的样式
          **都已经在文档里**：`#celira-user-themes` 装的是**全部**已装主题，不是只有生效那份。 */}
      {userThemes.length > 0 && (
        <ul className="space-y-2">
          {userThemes.map((item) => {
            const active = item.id === activeUserThemeId
            return (
              <li
                key={item.id}
                className={cn(
                  'space-y-2 rounded-lg border p-2',
                  active ? 'border-primary bg-accent' : 'border-border',
                )}
              >
                <ThemeScopePreview scopeId={item.id} dark={theme === 'dark'} className="h-16" />
                <div className="flex flex-wrap items-center gap-2">
                  <span className="text-caption">{item.name}</span>
                  {active && <span className="text-micro text-primary">正在用</span>}
                  <div className="ml-auto flex items-center gap-1">
                    {active ? (
                      <Button variant="ghost" size="xs" onClick={() => activateUserTheme(null)}>
                        回到官方主题
                      </Button>
                    ) : (
                      <Button variant="outline" size="xs" onClick={() => activateUserTheme(item.id)}>
                        用这套
                      </Button>
                    )}
                    <Button
                      variant="ghost"
                      size="xs"
                      onClick={() => downloadThemeFile(safeFileName(item.name), item.css)}
                    >
                      导出
                    </Button>
                    <Button variant="ghost" size="xs" className="text-destructive" onClick={() => deleteUserTheme(item.id)}>
                      删掉
                    </Button>
                  </div>
                </div>
              </li>
            )
          })}
        </ul>
      )}

      {/* 贴进去的主题没写到的令牌会继承当前这套 —— 不说这句，用户会以为"我只改了背景，怎么字还是看不清" */}
      <p className="text-micro text-muted-foreground">
        没写到的颜色会跟着「{THEME_PRESETS.find((p) => p.id === preset)?.name ?? preset}」走 —— 所以只改一两行也能装。
      </p>
    </div>
  )
}
