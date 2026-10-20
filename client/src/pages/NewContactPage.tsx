import { useState } from 'react'
import { useNavigate } from 'react-router'
import { ArrowLeft, Send, Undo2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ScrollArea } from '@/components/ui/scroll-area'
import { getApi, useAppStore } from '@/api/store'
import { errorText } from '@/api/contractErrors'
import {
  classifyContactRequest,
  looksLikeParticipantId,
  supportsContactRequest,
} from '@/api/uiPolicy'
import type { ContactRequestReceipt } from '@/api/domain'
import { ParticipantAvatar } from '@/components/celira/ParticipantAvatar'
import { AgentBadge } from '@/components/celira/AgentBadge'

/**
 * 新的朋友 = **发一段关系申请**。只有一件事，所以只有一个输入框、一个按钮。
 *
 * ## 为什么没有「添加真人 / 添加 Agent」两个页签
 *
 * 契约在批次三第二刀（2026-09-27）把目标从 `human_id` 放宽成 `participant_id`：
 * **任意 active 主体**都能加（`CORE_V1_CONTACT_REVERSIBILITY.md` §5.2）。两个页签等于
 * 客户端把世界刚拆掉的那道分叉又装回来 —— 而且 `kind` 本来就是**目标的属性**，
 * 不是用户要做的选择：它由回执带回来（`contact.peer.kind`），不由输入方式决定。
 *
 * ## 为什么这一页不再有「添加 Agent」（建一支 Agent）
 *
 * 那是**所有权**，不是关系 —— 与「发一段关系申请」是两件事，混在一页里会让人以为
 * "加 Agent"和"加人"是两种加法。创建的入口只有一个：`/app/agents` 的右上角「添加」
 * （`AgentsListPanel` 的就地创建面板），那条裁决写在那边的注释里。
 *
 * ## 这一页唯一不许简化掉的东西：回执
 *
 * 服务端在响应里已经回答了「这个 id 是谁」与「这条在等谁」。四种处境（刚发出 / 之前发过 /
 * **对方先发过、该我点头** / 早就是联系人）说成同一句「请求已发送」就是替系统撒谎
 * （判据：`api/uiPolicy.ts` 的 `classifyContactRequest`，有直测）。
 */

/** 四种处境各自那一句。**不许合并**：第三句的主语不是"对方"，是"我"。 */
const OUTCOME_LINE: Record<ReturnType<typeof classifyContactRequest>, string> = {
  sent: '申请已发出',
  'awaiting-them': '你之前已经发过这条申请，还在等',
  'needs-my-answer': '对方先发过给你 —— 这条在等你回答，不是等对方',
  'now-contact': '你们已经是联系人',
}

export function NewContactPage() {
  const navigate = useNavigate()
  const sendContactRequest = useAppStore((s) => s.sendContactRequest)
  const removeContact = useAppStore((s) => s.removeContact)

  const [raw, setRaw] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [receipt, setReceipt] = useState<ContactRequestReceipt | null>(null)
  const [withdrawn, setWithdrawn] = useState(false)

  // 世界的 id 是**规范小写连字符**的 UUID v4（服务端 `ParticipantId::parse_stable`）。
  // 这里只做规范化 + 形状判断；"这个人存不存在"只有服务端能答。
  const id = raw.trim().toLowerCase()
  const shaped = looksLikeParticipantId(id)
  const outcome = receipt ? classifyContactRequest(receipt) : null
  const peer = receipt?.peer

  const reset = () => {
    setReceipt(null)
    setWithdrawn(false)
    setError(null)
  }

  const submit = async () => {
    if (!shaped || busy) return
    // 能力探测**在点下去那一刻**做，不放在渲染里：`getApi()` 不是响应式的，而 store 的
    // 连接是在挂载之后才建立的 ⇒ 渲染期读它会把「晚一步建好」读成「这台后端没有这一面」，
    // 于是整页在真机上被一句话判死（2026-10-20 探针实测撞到过）。
    if (!supportsContactRequest(getApi())) {
      setError('当前连接不提供发起申请的接口，这一页用不了。')
      return
    }
    setBusy(true)
    setError(null)
    setWithdrawn(false)
    try {
      setReceipt(await sendContactRequest(id, undefined))
    } catch (e) {
      // 走文案族（`errorText`）：同样**绝不回显**服务端正文 / key / 路径（本表只吃
      // 稳定分类码），但不再把 `code (HTTP status)` 这种机器话直接打给用户。
      setReceipt(null)
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }

  // 撤回：`DELETE /v1/contacts/{id}` 对 **pending** 段同样成立（active 段以 `ended` 闭合）。
  // 它存在的理由是这一页的输入是"一串 id"：打错一个真实存在的 id 时，撤回是唯一的出口。
  const withdraw = async () => {
    if (!peer || busy) return
    setBusy(true)
    setError(null)
    try {
      await removeContact(peer.id)
      setWithdrawn(true)
      setReceipt(null)
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }

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
          <div className="space-y-3 rounded-xl border border-border bg-card p-5">
            <label htmlFor="contact-id" className="block text-caption text-muted-foreground">
              对方的身份 ID
            </label>
            <Input
              id="contact-id"
              value={raw}
              onChange={(e) => {
                setRaw(e.target.value)
                reset()
              }}
              placeholder="xxxxxxxx-xxxx-4xxx-xxxx-xxxxxxxxxxxx"
              spellCheck={false}
              autoComplete="off"
              autoCapitalize="off"
              aria-describedby="contact-id-hint"
              className="bg-muted font-mono text-caption"
            />
            {/* 同一句话里回答两个问题：为什么只有一个框（不按 kind 分叉）、这串 id 从哪来。 */}
            <p id="contact-id-hint" className="text-caption text-muted-foreground">
              Human 与 Agent 都用这一串。这里加的是「你与它的一段关系」，不按种类走两条路 ——
              对方是谁，发出去之后由回执告诉你。
            </p>
            {raw.trim() && !shaped && (
              <p className="text-caption text-muted-foreground">
                这不像一串身份 ID（36 位、形如 8-4-4-4-12）。这里只认 ID，不认名字 ——
                名字在这里查不到人。
              </p>
            )}
            <Button
              className="w-full gap-1.5"
              disabled={!shaped || busy}
              onClick={() => void submit()}
            >
              <Send className="size-4" />
              {busy ? '发送中…' : '发送申请'}
            </Button>
            {error && (
              <p role="alert" className="text-caption text-presence-failed">
                {error}
              </p>
            )}
          </div>

          {withdrawn && (
            <p className="px-1 text-caption text-muted-foreground">
              申请已撤回。对方不会被告知这件事，这段关系已经结束 —— 想再加，就是新的一段。
            </p>
          )}

          {receipt && peer && outcome && (
            <div className="space-y-3 rounded-xl border border-border bg-card p-5">
              <div className="flex items-center gap-3">
                <ParticipantAvatar participant={peer} size="md" />
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <span className="truncate text-body">{peer.displayName}</span>
                    {peer.kind === 'agent' && <AgentBadge />}
                  </div>
                  <div className="text-caption text-muted-foreground">{OUTCOME_LINE[outcome]}</div>
                </div>
              </div>

              {/* 「谁需要做什么」这句话按**处境**分，不按 kind 分：
                  这一页最容易撒的谎是让发送方以为对方被提醒了 —— 关系动作**不产生任何事件**
                  （契约 §5.2），对端只有自己拉列表才看得见。 */}
              {outcome === 'needs-my-answer' ? (
                <p className="text-caption text-muted-foreground">
                  这条申请在等你回答，回联系人里就能接受或拒绝。
                </p>
              ) : outcome === 'now-contact' ? (
                <p className="text-caption text-muted-foreground">关系已经成立，不用再发一次。</p>
              ) : (
                <p className="text-caption text-muted-foreground">
                  {peer.kind === 'agent'
                    ? '它不会被告知 —— 要等它下一次醒来、自己拉一遍关系列表才会看到这条申请。它没在跑的话，这条会一直躺着。'
                    : '对方不会被告知 —— 下次打开联系人时才会看到这条申请。'}
                </p>
              )}

              <div className="flex flex-wrap items-center gap-2">
                {outcome === 'needs-my-answer' && (
                  <Button size="sm" onClick={() => navigate('/app/contacts')}>
                    去联系人
                  </Button>
                )}
                {outcome === 'now-contact' && (
                  <Button size="sm" onClick={() => navigate(`/app/contacts/${peer.id}`)}>
                    看资料
                  </Button>
                )}
                {(outcome === 'sent' || outcome === 'awaiting-them') && (
                  <Button
                    size="sm"
                    variant="outline"
                    className="gap-1.5"
                    disabled={busy}
                    onClick={() => void withdraw()}
                  >
                    <Undo2 className="size-3.5" />
                    撤回这条申请
                  </Button>
                )}
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => {
                    setRaw('')
                    reset()
                  }}
                >
                  再发一条
                </Button>
              </div>
            </div>
          )}

          <p className="px-1 text-caption text-muted-foreground">
            这串 ID 由对方给你（个人资料页那格「身份 ID」可以复制）。这个世界没有目录，也没有「搜名字找人」。
          </p>
        </div>
      </ScrollArea>
    </div>
  )
}
