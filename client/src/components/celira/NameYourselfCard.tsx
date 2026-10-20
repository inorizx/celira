import { useState } from 'react'
import { Globe, Loader2, UserRound, XCircle } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { humanNameProblem } from '@/api/shellLocal'

/**
 * 「你在这个世界里叫什么」——首位 Human 的命名卡（2026-10-20 主人裁决：**必须要填**）。
 *
 * ## 为什么这张卡存在
 *
 * 从前的首位 Human 显示名是**这台机器的用户名**（`USERNAME`）。那是**机器事实**，
 * 而显示名是**公共事实**（那个世界的 Participant）—— 拿机器事实冒充公共事实，而且
 * 它常常不是你（`Administrator` / `user` / `DESKTOP-8K2LQ`）。判据与 2026-10-12
 * 给 Agent 定的那条同一条：显示名是公共呈现，**系统不替谁认定身份**。
 *
 * ## 纪律
 *
 * - **不预填任何东西**：预填一个机器字符串就是让那个 bug 借尸还魂；
 * - **不合格就不让走**（`humanNameProblem`，与 Core `DisplayName::parse`、壳的
 *   `normalize_human_name` 同一套三条）—— 页面这道只为"不让人白等一次往返"；
 * - **失败如实说**（壳回来的原因原样），不假装成功、不偷偷换一个名字；
 * - 唯一的另一个出口是「连别的世界」（与首屏那张卡同一个出口，语义不变）。
 *
 * 名字写进世界之后就归本人改（「我的资料」，2026-10-18 切片）—— 这张卡只负责第一次。
 */
export interface NameYourselfCardProps {
  /** 提交一个名字：由外层叫壳铸人并直连（`App.tsx` 的 `nameLocalHuman`）。 */
  onSubmit: (displayName: string) => Promise<{ ok: boolean; detail: string | null }>
  /** 「连别的世界」：与首屏那张卡的出口同一个语义（写下"这台机器上不跑世界"）。 */
  onConnectElsewhere: () => void | Promise<void>
}

export function NameYourselfCard({ onSubmit, onConnectElsewhere }: NameYourselfCardProps) {
  const [draft, setDraft] = useState('')
  const [busy, setBusy] = useState(false)
  const [problem, setProblem] = useState<string | null>(null)
  const [failure, setFailure] = useState<string | null>(null)

  const submit = async () => {
    const invalid = humanNameProblem(draft)
    if (invalid) {
      setProblem(invalid)
      return
    }
    setProblem(null)
    setFailure(null)
    setBusy(true)
    try {
      const done = await onSubmit(draft)
      // **不假装成功**：壳没铸成就把原因留在卡上，人还能改名再试或换个世界。
      if (!done.ok) setFailure(done.detail ?? '这个名字没能用上 —— 可以再试一次，或去连别的世界。')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="flex min-h-full flex-col items-center justify-center gap-6 bg-background px-6 py-10">
      <img src="/logo/logo-gradient.svg" alt="Celira" className="size-20 md:size-24" />
      <div className="max-w-sm space-y-2 text-center">
        <h1 className="text-title">你在这个世界里叫什么</h1>
        <p className="text-body text-muted-foreground">
          这个世界里别人看到的就是这个名字。以后可以在「我的资料」里改。
        </p>
      </div>

      <form
        className="w-full max-w-sm space-y-4 rounded-xl border border-border bg-card p-5"
        onSubmit={(event) => {
          event.preventDefault()
          void submit()
        }}
      >
        <div className="space-y-2">
          <label
            className="flex items-center gap-2 text-caption text-muted-foreground"
            htmlFor="name-yourself"
          >
            <UserRound className="size-3.5 shrink-0" aria-hidden />
            显示名
          </label>
          <Input
            id="name-yourself"
            name="display-name"
            value={draft}
            onChange={(event) => {
              setDraft(event.target.value)
              if (problem) setProblem(null)
            }}
            // ⚠ 占位必须是**一眼假的**（`user123` 这种）。一个"像真名"的例子会被读成
            //   "该填这样的名字"，而把某个具体的人名写进产品，就是拿一个人的身份当默认值
            //   （2026-10-20 真机被指出：占位里坐着的是主人自己的名字）。
            placeholder="比如：user123"
            autoComplete="off"
            autoFocus
            spellCheck={false}
            disabled={busy}
          />
          {problem && (
            <p className="flex items-start gap-1.5 text-caption text-presence-failed">
              <XCircle className="mt-0.5 size-3.5 shrink-0" aria-hidden />
              <span>{problem}</span>
            </p>
          )}
        </div>

        <Button type="submit" className="w-full cursor-pointer" disabled={busy}>
          {busy ? <Loader2 className="size-4 animate-spin" aria-hidden /> : null}
          {busy ? '正在铸出你…' : '进入'}
        </Button>

        <Button
          type="button"
          variant="ghost"
          className="w-full cursor-pointer text-muted-foreground"
          disabled={busy}
          onClick={() => void onConnectElsewhere()}
        >
          <Globe className="size-4" aria-hidden />
          连别的世界
        </Button>

        {failure && (
          <p className="flex items-start gap-1.5 text-caption text-presence-failed">
            <XCircle className="mt-0.5 size-3.5 shrink-0" aria-hidden />
            <span>{failure}</span>
          </p>
        )}
      </form>
    </div>
  )
}
