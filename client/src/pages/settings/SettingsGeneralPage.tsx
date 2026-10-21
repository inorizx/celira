import { useEffect, useState } from 'react'
import { CheckCircle2, Copy, KeyRound, Link2, Loader2, LogOut, Ticket, UserPlus, UserRound, XCircle } from 'lucide-react'
import { useNavigate } from 'react-router'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { CommandLine } from '@/components/celira/CommandLine'
import { getApi, useAppStore, useConnection } from '@/api/store'
import { normalizeServerUrl, serverUrlError } from '@/api/prefs'
// 「现在连的是哪儿」的读数：**权威在这一页**（地址在这里可编辑）。2026-10-11 之前它被
// 复制在「本机服务」那一页上（那页归这台机器，不归世界）⇒ 一份事实两个位置。
import { serverAddressReading } from '@/lib/server-address'
import {
  exchangeInvite,
  inviteCodeError,
  inviteExchangeErrorText,
  inviteNameError,
} from '@/api/invite'
import type { PairingGrantView } from '@/api/contracts'
import { errorText } from '@/api/contractErrors'
import { cn } from '@/lib/utils'
import { clearAttachmentPreviewCache } from '@/lib/attachmentSource'

const connectionStatusText = {
  idle: '未连接',
  connecting: '连接中…',
  connected: '已连接',
  error: '连接失败',
  reconnecting: '已断开，自动重连中…',
} as const

/**
 * 通用设置：连接状态、**连接设置**（直连 Core：服务器地址 + token）与 Core
 * 会话退出。
 *
 * 直连 Core 是所有自部署用户的正常登录方式（不是「开发者选项」）；地址留空
 * 即同源，家里由 home_up / serve_client 托管的页面一个字符都不用改。
 */
/** 拿不到传输层（还没连上）就当场说清——邀请只对已连接的世界有意义。 */
function getApiOrThrow() {
  const api = getApi()
  if (!api) throw new Error('not connected')
  return api
}

export function SettingsGeneralPage() {
  const navigate = useNavigate()
  const { token, serverUrl, connection, connectionError, me } = useConnection()
  // 同源时"本机不本机"只能看**页面自己**的主机名 —— 判据在 `lib/server-address.ts`
  // （纯函数、有 `node --test` 用例），本页只负责把 origin 递进去。
  const origin = typeof window === 'undefined' ? '' : window.location.origin
  const addressReading = serverAddressReading(serverUrl, origin)
  /** 已经连着（含自动重连）才配说"现在连的是"；其余状态说的是"将连到"。 */
  const linked = connection === 'connected' || connection === 'reconnecting'
  const setPrefs = useAppStore((s) => s.setPrefs)
  const connect = useAppStore((s) => s.connect)
  const revokeSession = useAppStore((s) => s.revokeSession)
  const [modeDraft] = useState<'core'>('core')
  const [addressDraft, setAddressDraft] = useState(serverUrl)
  const [tokenDraft, setTokenDraft] = useState(token)
  const [formError, setFormError] = useState<string | null>(null)
  const [logoutArmed, setLogoutArmed] = useState(false)
  const [logoutError, setLogoutError] = useState<string | null>(null)
  // 进门（被邀请者）：粘码 + 自己命名 ⇒ 兑换出一支属于**自己**的会话。
  const [inviteCode, setInviteCode] = useState('')
  const [inviteName, setInviteName] = useState('')
  const [inviteBusy, setInviteBusy] = useState(false)
  const [inviteError, setInviteError] = useState<string | null>(null)
  // 邀请（屋主/任何成员）：当场生成一张一次性邀请码，只显示这一次。
  const [issuedCode, setIssuedCode] = useState<string | null>(null)
  // 上面那一串是哪一张 Grant —— 撤销它的时候要把它一起收走（否则屏幕上还挂着一串死码 + 复制按钮）。
  const [issuedGrantId, setIssuedGrantId] = useState<string | null>(null)
  const [issuedError, setIssuedError] = useState<string | null>(null)
  const [myGrants, setMyGrants] = useState<PairingGrantView[]>([])

  const joinWithInvite = async () => {
    const badAddress = serverUrlError(addressDraft)
    if (badAddress) return setInviteError(badAddress)
    const badCode = inviteCodeError(inviteCode)
    if (badCode) return setInviteError(badCode)
    const badName = inviteNameError(inviteName)
    if (badName) return setInviteError(badName)
    setInviteError(null)
    setInviteBusy(true)
    try {
      const result = await exchangeInvite(normalizeServerUrl(addressDraft), inviteCode, inviteName)
      // 拿到的是**自己的** Human 会话：与手工粘 token 走同一条存储路径。
      setPrefs('core', result.token, normalizeServerUrl(addressDraft))
      setTokenDraft(result.token)
      setInviteCode('')
      await connect()
    } catch (e) {
      const code = (e as { code?: string }).code
      setInviteError(inviteExchangeErrorText(code))
    } finally {
      setInviteBusy(false)
    }
  }

  const createInvite = async () => {
    setIssuedError(null)
    try {
      const issued = await getApiOrThrow().issueInvite()
      setIssuedCode(issued.token)
      setIssuedGrantId(issued.grant_id)
      setMyGrants((await getApiOrThrow().listGrants()).grants)
    } catch {
      setIssuedError('没能生成邀请码：先确认已经连上这台服务器。')
    }
  }

  const refreshGrants = async () => {
    try {
      setMyGrants((await getApiOrThrow().listGrants()).grants)
    } catch {
      setIssuedError('没能读到已发出的邀请。')
    }
  }

  /**
   * **进页面就把「已发出的邀请」拉出来**（2026-09-29 真机实测修）。
   *
   * 原先 `myGrants` 只在"生成一张"或"撤销一张"之后才赋值 ⇒ 刷新一次页面，那一整块就消失，
   * 而服务端明明还存着（`GET /v1/home/pairing-grants`）。后果是屋主**看不见也撤销不了**
   * 自己发出去的邀请码 —— 页面上唯一能让列表出现的动作居然是"再生成一张"。
   *
   * 只在真的连上时才拉（`getApiOrThrow` 拿不到传输层就抛，这里静默不显示即可：
   * 「邀请一个人」那一块本来也只在真实世界才渲染）。
   */
  useEffect(() => {
    if (connection !== 'connected') return
    void refreshGrants()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connection])

  const revokeGrant = async (grantId: string) => {
    try {
      await getApiOrThrow().revokeGrant(grantId)
      // 撤掉的正好是屏幕上挂着的那一串 ⇒ 连它一起收走（否则会留一个"复制死码"的按钮）。
      if (grantId === issuedGrantId) {
        setIssuedCode(null)
        setIssuedGrantId(null)
      }
      await refreshGrants()
    } catch {
      setIssuedError('没能撤销这一张，稍后再试。')
    }
  }

  const saveAndConnect = () => {
    const badAddress = serverUrlError(addressDraft)
    if (badAddress) {
      setFormError(badAddress)
      return
    }
    setFormError(null)
    setPrefs(modeDraft, tokenDraft, normalizeServerUrl(addressDraft))
    // 换世界 ⇒ 预览缓存里那些字节不再属于眼前这个世界，清掉（下次照旧按需再取）。
    clearAttachmentPreviewCache()
    void connect()
  }

  const handleLogout = async () => {
    if (!logoutArmed) {
      setLogoutArmed(true)
      setTimeout(() => setLogoutArmed((a) => a && false), 4000)
      return
    }
    setLogoutError(null)
    try {
      await revokeSession()
      // 预览缓存里那些字节属于**上一个身份**看得见的东西 ⇒ 会话撤销后立刻清掉
      // （清的同时撤销 object URL，把内存还回去）。
      clearAttachmentPreviewCache()
      setTokenDraft('')
      setLogoutArmed(false)
      navigate('/app/profile')
    } catch (e) {
      // 同一处权威（`contractErrors` 的 `errorText`）：登出要打世界，而世界够不着时
      // `e.message` 是库的原文（`error sending request for url (…)`）—— 那是内行话，不上屏。
      setLogoutError(errorText(e))
      setLogoutArmed(false)
    }
  }

  const addressField = (
    <div className="space-y-2">
      {/* 常驻 label（照 `LoginGate` 那一套）：占位符只给例子，一打字不会把"这一格是什么"带走。 */}
      <label className="flex items-center gap-2 text-caption text-muted-foreground" htmlFor="settings-server-url">
        <Link2 className="size-3.5 shrink-0" aria-hidden />
        服务器地址（留空 = 本页同源）
      </label>
      <Input
        id="settings-server-url"
        value={addressDraft}
        onChange={(e) => setAddressDraft(e.target.value)}
        placeholder="http://127.0.0.1:8080"
        autoComplete="off"
        spellCheck={false}
      />
      <p className="text-caption text-muted-foreground">
        留空即用本页同源（<code className="font-mono">/v1</code>）；跨机或跨端口时填
        <code className="font-mono"> http://主机:端口</code>。
      </p>
      {/* ★ **连到哪儿**（2026-10-11 从「本机服务」那一页搬回来的）：那一档
          "就是这一页"（空值）**只有这个读数说得出** —— 输入框空着看不出它到底连哪儿。
          ⚠ 空闲时不许写"现在连的是"：那时还没连上，说的是**将要去的地方**。 */}
      <p className="text-caption text-muted-foreground">
        {linked ? '现在连的是' : '将连到'}：{addressReading.address}
      </p>
    </div>
  )

  const tokenField = (
    <div className="space-y-2">
      <label className="flex items-center gap-2 text-caption text-muted-foreground" htmlFor="settings-token">
        <KeyRound className="size-3.5 shrink-0" aria-hidden />
        访问令牌
      </label>
      <Input
        id="settings-token"
        type="password"
        value={tokenDraft}
        onChange={(e) => setTokenDraft(e.target.value)}
        placeholder="粘贴上面那行命令输出的那一串"
        autoComplete="off"
      />
    </div>
  )

  return (
    <>
      <div className="space-y-3 rounded-xl border border-border bg-card p-5">
        <div className="flex items-center gap-2">
          <span className="text-body">连接设置</span>
          <span className="ml-auto inline-flex items-center gap-1.5 text-caption">
            {connection === 'connected' && <CheckCircle2 className="size-3.5 text-presence-online" />}
            {connection === 'connecting' && <Loader2 className="size-3.5 animate-spin text-muted-foreground" />}
            {connection === 'error' && <XCircle className="size-3.5 text-presence-failed" />}
            <span
              className={cn(
                connection === 'connected' && 'text-presence-online',
                connection === 'error' && 'text-presence-failed',
              )}
            >
              {connectionStatusText[connection]}
            </span>
            {me && <span className="text-muted-foreground">· {me.participant.display_name}</span>}
          </span>
        </div>
        {addressField}
        {tokenField}
        {/* 命令是"要执行的东西"，不是"要读的东西" —— 给它自己的块和复制按钮。 */}
        <CommandLine
          command='celira-im create-human --display-name "你的名字"'
          note="在本机终端里跑（仓根），把输出的那一串粘到上面「访问令牌」里。"
        />
        <Button className="w-full" onClick={saveAndConnect}>
          保存并连接
        </Button>
        {formError && <p className="text-caption text-presence-failed">{formError}</p>}
        {connection === 'error' && connectionError && (
          <p className="text-caption text-presence-failed">{connectionError}</p>
        )}
        {connection === 'connected' && (
          <div className="space-y-2 border-t border-border pt-4">
            <Button variant="destructive" className="w-full" onClick={() => void handleLogout()}>
              <LogOut className="size-4" />
              {logoutArmed ? '再次点击确认退出并撤销会话' : '退出登录（撤销当前会话）'}
            </Button>
            {logoutError && <p className="text-caption text-presence-failed">{logoutError}</p>}
          </div>
        )}
      </div>

      <div className="space-y-3 rounded-xl border border-border bg-card p-5">
        <div className="flex items-center gap-2">
          <UserPlus className="size-4" />
          <span className="text-body">用邀请码进来</span>
        </div>
        <p className="text-caption text-muted-foreground">
          这个世界没有注册：要么你自己部署一个，要么有人请你进来。把对方给你的邀请码
          粘到这里，名字由你自己填——世界里的其他人会看到它。
        </p>
        <div className="space-y-2">
          <label className="flex items-center gap-2 text-caption text-muted-foreground" htmlFor="settings-invite-code">
            <Ticket className="size-3.5 shrink-0" aria-hidden />
            邀请码
          </label>
          <Input
            id="settings-invite-code"
            value={inviteCode}
            onChange={(e) => setInviteCode(e.target.value)}
            placeholder="clr_pg_ 开头的一整串"
            autoComplete="off"
            spellCheck={false}
          />
        </div>
        <div className="space-y-2">
          <label className="flex items-center gap-2 text-caption text-muted-foreground" htmlFor="settings-invite-name">
            <UserRound className="size-3.5 shrink-0" aria-hidden />
            我的名字
          </label>
          <Input
            id="settings-invite-name"
            value={inviteName}
            onChange={(e) => setInviteName(e.target.value)}
            placeholder="世界里的其他人会看到它"
            autoComplete="off"
          />
        </div>
        {/* 一屏只有一个主行动：上面那张卡的「保存并连接」是它。
            这一颗描边仍然可点、仍然是这一张卡的动作，只是不与它并列抢主角。 */}
        <Button variant="outline" className="w-full" disabled={inviteBusy} onClick={() => void joinWithInvite()}>
          {inviteBusy ? '正在进来…' : '用这个邀请码进来'}
        </Button>
        {inviteError && <p className="text-caption text-presence-failed">{inviteError}</p>}
        <p className="text-caption text-muted-foreground">
          服务器地址用上面那一格（留空 = 本页同源）。
        </p>
      </div>

      {connection === 'connected' && (
        <div className="space-y-3 rounded-xl border border-border bg-card p-5">
          <div className="flex items-center gap-2">
            <UserPlus className="size-4" />
            <span className="text-body">邀请一个人</span>
            <Button
              variant="ghost"
              size="sm"
              className="ml-auto"
              onClick={() => void createInvite()}
            >
              生成邀请码
            </Button>
          </div>
          <p className="text-caption text-muted-foreground">
            邀请码一次性、72 小时内有效、随时可撤销；只在生成这一刻显示，请当场复制给对方。
            对方用它进来时自己填名字，进来后会看到你的一条好友申请——由他自己决定加不加你。
          </p>
          {issuedCode && (
            <div className="space-y-2 rounded-lg border border-border bg-muted/40 p-3">
              <code className="block break-all font-mono text-caption">{issuedCode}</code>
              <Button
                variant="outline"
                size="sm"
                className="w-full"
                onClick={() => {
                  void navigator.clipboard?.writeText(issuedCode)
                }}
              >
                <Copy className="size-3.5" />
                复制邀请码
              </Button>
            </div>
          )}
          {issuedError && <p className="text-caption text-presence-failed">{issuedError}</p>}
          {myGrants.filter((g) => g.grants === 'new_human').length > 0 && (
            <div className="space-y-2">
              {myGrants
                .filter((g) => g.grants === 'new_human')
                .map((g) => (
                  <div key={g.grant_id} className="flex items-center gap-2 text-caption">
                    <span className="text-muted-foreground">
                      {g.status === 'active' ? '还没人用' : g.status === 'expired' ? '已过期' : '已撤销'}
                      {' · '}
                      {new Date(g.expires_at).toLocaleString()}
                    </span>
                    {g.status === 'active' && (
                      <Button
                        variant="ghost"
                        size="sm"
                        className="ml-auto"
                        onClick={() => void revokeGrant(g.grant_id)}
                      >
                        撤销
                      </Button>
                    )}
                  </div>
                ))}
            </div>
          )}
        </div>
      )}

      <p className="text-center text-caption text-muted-foreground">
        直连服务器只影响当前浏览器会话与这份连接设置。
      </p>
    </>
  )
}
