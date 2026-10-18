import { useState } from 'react'
import { useNavigate } from 'react-router'
import { Globe, House, KeyRound, Link2, Loader2, PlayCircle, XCircle } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useAppStore, useConnection } from '@/api/store'
import { normalizeServerUrl, serverUrlError } from '@/api/prefs'

/**
 * client 的**正门**：登录 = 「连哪个服务器」+「你是谁」，外加一格
 * 「这台机器上还没有世界」（`mode'run-local'`）。
 *
 * 触发条件（见 `api/entry.ts` 的 `decideFrontDoor`）：没有可直接复用的直连
 * 凭据、且落在**入口路径**（`/`、`/splash`）。此时第一屏就是这张卡——
 * 绝不把永久事实说成瞬时故障。
 *
 * ## 两个模式，一处分派
 *
 * - `'connect'`（默认，一字未变的今天）：服务器地址 + token + 「连接」；
 * - `'run-local'`（2026-10-03）：**桌面 app 的第一次启动**——壳说这个家还没表态，
 *   于是问一句「要不要在这台机器上跑一个世界」（`PROJECT_SHAPE.md` §8.6-1：它会写磁盘、
 *   占端口、常驻后台，所以问一次；问过之后由那次答复说了算）。
 *   两个出口都诚实：开 ⇒ 壳起世界、签一支会话、直接进；不开 ⇒ 回「连接」那一格，
 *   去连别的世界。**这不是第二条连接路径**：连哪个世界的分派点仍然只有
 *   `decideFrontDoor` 一处。
 *
 * 纪律：
 * - 地址留空 = 同源（家里 home_up / serve_client 托管的页面一个字符都不用改）；
 * - 提交时先落 prefs（模式 core + token + 归一化地址）再连，`connect()` 读的
 *   就是这份持久选择；连不上就停在本页显示真实错误，不假装成功、不静默回退；
 * - 凭据只进 `celira.connection.prefs`（既有 localStorage），不新增任何持久层。
 */
export interface LoginGateProps {
  mode?: 'connect' | 'run-local'
  /** `run-local` 那一格：用户选「开一个」之后由外层去叫壳（`App.tsx` 的 `startLocalWorld`）。 */
  onRunLocal?: () => Promise<void> | void
  /** `run-local` 那一格：用户选「连别的世界」之后由外层去叫壳（写下"不跑"的表态）。 */
  onConnectElsewhere?: () => Promise<void> | void
  /** `run-local` 那一格：壳这一次没把世界起起来时的原样原因（有 ⇒ 如实显示）。 */
  localFailure?: string | null
}

export function LoginGate({
  mode = 'connect',
  onRunLocal,
  onConnectElsewhere,
  localFailure = null,
}: LoginGateProps) {
  const navigate = useNavigate()
  const { token, serverUrl } = useConnection()
  const setPrefs = useAppStore((s) => s.setPrefs)
  const connect = useAppStore((s) => s.connect)
  const setNeedsLogin = useAppStore((s) => s.setNeedsLogin)
  const [addressDraft, setAddressDraft] = useState(serverUrl)
  const [tokenDraft, setTokenDraft] = useState(token)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const submit = async () => {
    const badAddress = serverUrlError(addressDraft)
    if (badAddress) {
      setError(badAddress)
      return
    }
    if (!tokenDraft.trim()) {
      setError('请先粘贴服务端签发的 token')
      return
    }
    setBusy(true)
    setError(null)
    setPrefs('core', tokenDraft.trim(), normalizeServerUrl(addressDraft))
    await connect()
    setBusy(false)
    const next = useAppStore.getState()
    if (next.connection === 'connected') {
      // 进值班室（聊天首页）：core 模式既有的默认房间行为 —— 不替主体猜房间，
      // 由聊天页自己挑。**只有真的连上了才收起正门**（连不上就留在本页，
      // 把失败原因摆在刚才那个表单上）。
      setNeedsLogin(false)
      navigate('/app/chat', { replace: true })
      return
    }
    setError(next.connectionError ?? '连接失败')
  }

  const browseDemo = () => {
    // 「只重排入口，不删能力」：不连任何服务器也能看演示界面（fixtures 路径，
    // 经 DemoSwitcher 出入）。这是**显式用户动作**，不是 host 模式静默回退。
    // 演示页是非入口路径，正门不会拦它；回到入口路径（`/`）时正门照旧问
    // 「连哪个服务器 + 你是谁」——不在这里替用户把门关掉。
    setPrefs('core', '')
    navigate('/gallery')
  }

  // ── 这一台机器上还没有世界（§8.6-1 问一次）─────────────────────────────────
  if (mode === 'run-local') {
    const runIt = async () => {
      setBusy(true)
      setError(null)
      try {
        await onRunLocal?.()
      } finally {
        setBusy(false)
      }
    }
    const elsewhere = async () => {
      setBusy(true)
      await onConnectElsewhere?.()
      setBusy(false)
    }
    return (
      <div className="flex min-h-screen flex-col items-center justify-center gap-6 bg-background px-6 py-10">
        <img src="/logo/logo-gradient.svg" alt="Celira" className="size-20 md:size-24" />
        <div className="max-w-sm space-y-2 text-center">
          <h1 className="text-title">你自己的世界</h1>
          <p className="text-body text-muted-foreground">
            这台机器上还没有世界。要不要在这里开一个？你的 Agent 会住在里面。
          </p>
        </div>

        <div className="w-full max-w-sm space-y-4 rounded-xl border border-border bg-card p-5">
          <p className="flex items-start gap-2 text-caption text-muted-foreground">
            <House className="mt-0.5 size-3.5 shrink-0" aria-hidden />
            <span>它会把数据留在这台机器上，占一个本机端口，并在后台一直开着。</span>
          </p>
          <Button type="button" className="w-full cursor-pointer" disabled={busy} onClick={() => void runIt()}>
            {busy ? <Loader2 className="size-4 animate-spin" aria-hidden /> : <House className="size-4" aria-hidden />}
            {busy ? '正在准备…' : '在这台机器上开一个'}
          </Button>
          <Button
            type="button"
            variant="ghost"
            className="w-full cursor-pointer text-muted-foreground"
            disabled={busy}
            onClick={() => void elsewhere()}
          >
            <Globe className="size-4" aria-hidden />
            连别的世界
          </Button>
          {(error || localFailure) && (
            <p className="flex items-start gap-1.5 text-caption text-presence-failed">
              <XCircle className="mt-0.5 size-3.5 shrink-0" aria-hidden />
              <span>{error ?? localFailure}</span>
            </p>
          )}
        </div>
      </div>
    )
  }

  return (
    <div className="flex min-h-screen flex-col items-center justify-center gap-6 bg-background px-6 py-10">
      <img src="/logo/logo-gradient.svg" alt="Celira" className="size-20 md:size-24" />
      <div className="max-w-sm space-y-2 text-center">
        <h1 className="text-title">连接你的世界</h1>
        <p className="text-body text-muted-foreground">
          这里直接问：连哪个服务器，你是谁。
        </p>
      </div>

      <form
        className="w-full max-w-sm space-y-4 rounded-xl border border-border bg-card p-5"
        onSubmit={(e) => {
          e.preventDefault()
          void submit()
        }}
      >
        <div className="space-y-2">
          <label className="flex items-center gap-2 text-caption text-muted-foreground" htmlFor="login-server">
            <Link2 className="size-3.5 shrink-0" aria-hidden />
            服务器地址（留空 = 本页同源）
          </label>
          <Input
            id="login-server"
            name="server-url"
            value={addressDraft}
            onChange={(e) => setAddressDraft(e.target.value)}
            placeholder="http://127.0.0.1:8080"
            autoComplete="off"
            spellCheck={false}
          />
        </div>

        <div className="space-y-2">
          <label className="flex items-center gap-2 text-caption text-muted-foreground" htmlFor="login-token">
            <KeyRound className="size-3.5 shrink-0" aria-hidden />
            Bearer token
          </label>
          <Input
            id="login-token"
            name="core-token"
            type="password"
            value={tokenDraft}
            onChange={(e) => setTokenDraft(e.target.value)}
            placeholder="服务端签发的那一支"
            autoComplete="off"
          />
        </div>

        <p className="text-caption text-muted-foreground">
          本机服务端签发：<code className="font-mono">celira-im create-human --display-name "你的名字"</code>
          ，把输出的 token 粘到上面。token 只存在这个浏览器里。
        </p>

        <Button type="submit" className="w-full" disabled={busy}>
          {busy ? <Loader2 className="size-4 animate-spin" aria-hidden /> : <Link2 className="size-4" aria-hidden />}
          {busy ? '连接中…' : '连接'}
        </Button>

        {error && (
          <p className="flex items-start gap-1.5 text-caption text-presence-failed">
            <XCircle className="mt-0.5 size-3.5 shrink-0" aria-hidden />
            <span>{error}</span>
          </p>
        )}
      </form>

      <Button variant="ghost" className="text-muted-foreground" onClick={browseDemo}>
        <PlayCircle className="size-4" aria-hidden />
        不连接，先看演示界面
      </Button>
    </div>
  )
}
