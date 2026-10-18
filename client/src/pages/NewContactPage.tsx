import { useState } from 'react'
import { useNavigate } from 'react-router'
import { ArrowLeft, Search, Send } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'
import { getApi, useAppStore } from '@/api/store'
import { errorText } from '@/api/contractErrors'
import { supportsContactRequestByHumanId } from '@/api/uiPolicy'
import { ParticipantAvatar } from '@/components/celira/ParticipantAvatar'
import { CREATE_WONT_REPLY } from '@/components/celira/AgentsListPanel'
import { hasAnjiaBridge } from '@/api/anjia'

type Tab = 'human' | 'agent'

function AddHumanPanel() {
  const [query, setQuery] = useState('')
  const [requested, setRequested] = useState(false)
  const [sending, setSending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const sendContactRequest = useAppStore((s) => s.sendContactRequest)
  // TASK_72 ⑥：那句提示（「直接输入对方 Human 的 UUID」）原本拿 `mode === 'core'` 当判据。
  // 演示通道也活在 `mode === 'core'` 里 ⇒ 演示态照样显示它，而那个通道不按 Human 身份收请求
  // ⇒ 那句话是误导。改成**能力探测**：这台后端真实现了 `sendContactRequest` 才说。
  const contactRequestSupported = supportsContactRequestByHumanId(getApi())
  const q = query.trim().toLowerCase()

  const submit = async () => {
    if (!q) return
    setSending(true)
    setError(null)
    try {
      await sendContactRequest(q, undefined)
      setRequested(true)
    } catch (e) {
      // 走文案族（`errorText`）：同样**绝不回显**服务端正文 / key / 路径（本表只吃
      // 稳定分类码），但不再把 `code (HTTP status)` 这种机器话直接打给用户。
      setError(errorText(e))
    } finally {
      setSending(false)
    }
  }

  return (
    <div className="space-y-4">
      <div className="relative">
        <Search className="absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          value={query}
          onChange={(e) => {
            setQuery(e.target.value)
            setRequested(false)
            setError(null)
          }}
          placeholder="输入对方 Human 的 UUID…"
          className="bg-card pl-9"
        />
      </div>
      {contactRequestSupported && (
        <p className="text-caption text-muted-foreground">
          请直接输入对方 Human 的 UUID。
        </p>
      )}
      {q ? (
        <div className="space-y-3 rounded-xl border border-border bg-card p-5">
          <div className="text-caption text-muted-foreground">目标 Human ID：{q}</div>
          <Button className="w-full gap-1.5" disabled={requested || sending} onClick={() => void submit()}>
            <Send className="size-4" />
            {requested
              ? '请求已发送，等待对方确认'
              : sending
                ? '发送中…'
                : '发送好友请求'}
          </Button>
          {error && <p className="text-caption text-presence-failed">{error}</p>}
        </div>
      ) : (
        <div className="rounded-xl border border-dashed border-border px-5 py-10 text-center text-caption text-muted-foreground">
          {/* ⚠ 2026-09-29 真机实测修：这里原先写「通过**名字**或身份 ID 找到…」，而这格
              只吃 UUID（名字会被服务端当 id 查，回 404）—— 一句会让人白试一次的引导。
              这个世界没有目录，如实说。 */}
          填上对方的身份 ID，就能向 TA 发一条好友申请
        </div>
      )}
    </div>
  )
}

function CreateAgentPanel() {
  const navigate = useNavigate()
  const [name, setName] = useState('')
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const createAgent = useAppStore((s) => s.createAgent)
  const refreshOwnedAgents = useAppStore((s) => s.refreshOwnedAgents)
  const trimmed = name.trim()

  const preview = {
    id: 'a-new',
    kind: 'agent' as const,
    displayName: trimmed || '新 Agent',
    initials: trimmed ? trimmed.slice(0, 1) : 'A',
  }

  const submit = async () => {
    if (!trimmed || creating) return
    setCreating(true)
    setError(null)
    try {
      const agentId = await createAgent(trimmed, undefined)
      // 权威重读列表：跳去列表页时**那一支已经在里面**（否则落地页可能差一轮，
      // 看着像没建成）。与「我的 Agent」内联创建面板同一手。
      await refreshOwnedAgents()
      // 把刚创建那一支的 id 交给落地页（`AgentsListPanel`）：四态读数要落在**它**
      // 那一行上——读数已经在 store 里（创建时壳报回来的），落地页不发新请求。
      navigate('/app/agents', { state: { createdAgentId: agentId || null } })
    } catch (e) {
      // 走文案族（`errorText`）：同样**绝不回显**服务端正文 / key / 路径（本表只吃
      // 稳定分类码），但不再把 `code (HTTP status)` 这种机器话直接打给用户。
      setError(errorText(e))
    } finally {
      setCreating(false)
    }
  }

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-4 rounded-xl border border-border bg-card p-5">
        <ParticipantAvatar participant={preview} size="xl" />
        <div className="min-w-0 flex-1 space-y-1.5">
          <div className="text-caption text-muted-foreground">名称</div>
          <Input
            value={name}
            onChange={(e) => {
              setName(e.target.value)
              setError(null)
            }}
            placeholder="给你的 Agent 起个名字…"
            maxLength={24}
            className="bg-muted"
          />
        </div>
      </div>

      {/*
        ⚠ 此处原有的一块「模型」按钮（点开 `ModelSelectSheet`）已删除：它读的是 fixtures 的
        演示供应商目录（本页不在任何 fixtures 守卫之下 ⇒ 真实世界会看到演示模型），而
        `createAgent(displayName, idempotencyKey?)` 根本没有模型这个位置 —— 选了什么都不进
        请求，却**卡着创建按钮**。一个卡住主要动作、却对结果毫无影响的控件，是让用户做一个
        假的决定，比显示假事实更糟。Agent"用哪条模型源"归**执行端**那一页的「模型」房间。

        ⚠ 此处原有的一块「描述 / 人格设定」文本域已删除（2026-09-26）：它的 placeholder 写着
        「这段会交给你的 Agent Backend 作为人格提示」，而**那段文字从来没有进过任何请求** ——
        `createAgent(displayName, idempotencyKey?)` 只送 `{ display_name }`（见 `api/core.ts`），
        人格文本被就地丢弃。用户认真写了一段，界面接受了，什么也没发生。

        这两块是**同一个类**：让用户做一个假的决定，比显示假事实更糟。要做真的人格设定，
        得先有一条把正文送出去的契约路径 —— 那是要主人裁的公共契约问题，不是前端能自己补的。

        ⚠ **2026-09-29 真机实测修**：删掉这两块时把同一句说明留了**两份**，页面上那句话
        真的渲染了两遍。现在只留下面那一份。
      */}
      <p className="px-1 text-caption text-muted-foreground">
        Agent「用哪条模型源」在执行端那一页的「模型」房间。
      </p>

      <Button className="w-full" disabled={!trimmed || creating} onClick={() => void submit()}>
        {creating ? '创建中…' : '创建'}
      </Button>
      {error && <p className="text-center text-caption text-presence-failed">{error}</p>}
      {/* ⚠ 这句与「我的 Agent」页创建面板**共用同一个字面量**（`CREATE_WONT_REPLY`）：
          它是 `client/README.md`「已知缺口」那条事实的**唯一权威位置**，两个创建入口
          都必须说出来 —— 2026-09-26 审计发现这里原先没说，按钮还写着「创建并接入」。
          ⚠ **在壳里**（首刀步骤 2 起）：这句话已经不是事实了 —— 壳里创建完会**自动**
          把它接到这台机器上，所以壳里改说那句预期（结果由读数如实给；本页创建完就跳去
          列表页，所以读数在那边看）。**浏览器里一个字不改**：那里没有壳，它就是事实。 */}
      <p className="text-center text-caption text-muted-foreground">
        {hasAnjiaBridge()
          ? '创建后会自动把它接到这台机器上。你对它的行为承担责任。'
          : `创建后${CREATE_WONT_REPLY}。你对它的行为承担责任。`}
      </p>
    </div>
  )
}

export function NewContactPage() {
  const navigate = useNavigate()
  const [tab, setTab] = useState<Tab>('human')

  return (
    <div className="flex h-full flex-col bg-background">
      <header className="flex items-center gap-2 border-b border-border bg-card px-4 py-4">
        <Button variant="ghost" size="icon" onClick={() => navigate('/app/contacts')} aria-label="返回联系人">
          <ArrowLeft className="size-4" />
        </Button>
        <div className="text-title">新的朋友</div>
      </header>
      <ScrollArea className="min-h-0 flex-1">
        <div className="mx-auto w-full max-w-xl space-y-6 px-4 py-6">
          <div className="flex rounded-xl border border-border bg-card p-1">
            {(
              [
                { id: 'human', label: '添加真人' },
                { id: 'agent', label: '添加 Agent' },
              ] as const
            ).map((t) => (
              <button
                key={t.id}
                type="button"
                onClick={() => setTab(t.id)}
                aria-pressed={tab === t.id}
                className={cn(
                  'flex-1 rounded-lg py-2 text-body transition-colors',
                  'cursor-pointer',
                  tab === t.id
                    ? 'bg-accent text-accent-foreground'
                    : 'text-muted-foreground hover:text-foreground',
                )}
              >
                {t.label}
              </button>
            ))}
          </div>
          {tab === 'human' ? <AddHumanPanel /> : <CreateAgentPanel />}
        </div>
      </ScrollArea>
    </div>
  )
}
