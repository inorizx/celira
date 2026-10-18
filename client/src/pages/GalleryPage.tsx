import type { ReactNode } from 'react'
import { useState } from 'react'
import { Crown as CrownIcon, ShieldCheck as ShieldCheckIcon, UserMinus as UserMinusIcon } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Separator } from '@/components/ui/separator'
import { AgentBadge } from '@/components/celira/AgentBadge'
import { ParticipantAvatar } from '@/components/celira/ParticipantAvatar'
import { MessageBubble } from '@/components/celira/MessageBubble'
import { MarkdownText } from '@/components/celira/MarkdownText'
import { ConversationItem } from '@/components/celira/ConversationItem'
import { MessageInput } from '@/components/celira/MessageInput'
import { ThemeToggle } from '@/components/celira/ThemeToggle'
import { ThemePresetPreview } from '@/components/celira/ThemePresetCard'
import { THEME_PRESETS } from '@/lib/themes'
import { MentionText } from '@/components/celira/MentionText'
import { DateDivider } from '@/components/celira/DateDivider'
import { UnreadDivider } from '@/components/celira/UnreadDivider'
import { EmptyRoomsState } from '@/components/celira/EmptyRoomsState'
import { ConnectionBanner } from '@/components/celira/ConnectionBanner'
import { NewMessagePill } from '@/components/celira/NewMessagePill'
import { SystemMessage } from '@/components/celira/SystemMessage'
import { ImagePreviewLightbox } from '@/components/celira/ImagePreviewLightbox'
import { VideoAttachment } from '@/components/celira/VideoAttachment'
import { EmojiPicker } from '@/components/celira/EmojiPicker'
import { CeliraLogo } from '@/components/celira/CeliraLogo'
import { ChatSkeleton, MessageSkeleton } from '@/components/celira/Skeletons'
import { messages, participants, rooms } from '@/fixtures/data'
import { cn } from '@/lib/utils'

const colorTokens = [
  'background',
  'foreground',
  'card',
  'muted',
  'muted-foreground',
  'accent',
  'accent-foreground',
  'primary',
  'primary-foreground',
  'brand',
  'agent',
  'agent-surface',
  'bubble-self-from',
  'bubble-self-to',
  'bubble-peer',
  'border',
  'input',
  'ring',
  'overlay',
  'presence-online',
  'presence-thinking',
  'presence-silent',
  'presence-timeout',
  'presence-failed',
  'destructive',
]

function Swatch({ token }: { token: string }) {
  return (
    <div className="flex items-center gap-2">
      <span
        className="size-6 rounded-md border border-border"
        style={{ backgroundColor: `var(--color-${token}, var(--${token}))` }} // token-check:allow 动态令牌色板，值只能是 var()
      />
      <span className="font-mono text-caption text-muted-foreground">--{token}</span>
    </div>
  )
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="space-y-3">
      <h3 className="text-micro tracking-wider text-muted-foreground uppercase">
        {title}
      </h3>
      {children}
    </section>
  )
}

function ThemePanel({
  dark,
  title,
  previewOpen,
  onTogglePreview,
}: {
  dark?: boolean
  title: string
  previewOpen: boolean
  onTogglePreview: () => void
}) {
  return (
    <div className={cn('rounded-xl border border-border bg-background p-6 text-foreground', dark && 'dark')}>
      <div className="mb-5 flex items-center gap-2 text-title">
        <CeliraLogo className="size-5" />
        {title}
      </div>
      <div className="space-y-8">
        <Section title="Logo">
          <div className="flex items-end gap-6">
            <CeliraLogo className="size-16" />
            <CeliraLogo variant="mono" className="size-16" />
            <img src="/logo/logo-gradient.svg" alt="Celira 渐变版" className="size-16" />
            <img src="/logo/logo-mark.svg" alt="Celira 小尺寸版" className="size-8" />
            <img src="/logo/logo-mark.svg" alt="Celira favicon" className="size-4" />
          </div>
          <p className="text-caption text-muted-foreground">
            标准 · 单色 · 渐变 · 小尺寸 32px / 16px（统一品牌紫）
          </p>
        </Section>
        <Separator />
        <Section title="Type Scale">
          <div className="space-y-2">
            <div className="text-display">页面标题 · text-display</div>
            <div className="text-title">组件标题 / 参与者名 · text-title</div>
            <div className="text-body">正文与消息气泡 · text-body</div>
            <div className="text-caption text-muted-foreground">辅助说明 / 时间戳 · text-caption</div>
            <div className="text-micro tracking-wider text-muted-foreground uppercase">
              徽章与分组标签 · text-micro
            </div>
          </div>
        </Section>
        <Separator />
        <Section title="Tokens">
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-4">
            {colorTokens.map((t) => (
              <Swatch key={t} token={t} />
            ))}
          </div>
        </Section>
        <Separator />
        <Section title="Atoms">
          <div className="flex flex-wrap items-center gap-3">
            <Button size="sm">主要按钮</Button>
            <Button size="sm" variant="secondary">
              次要按钮
            </Button>
            <Button size="sm" variant="outline">
              描边按钮
            </Button>
            <Button size="sm" variant="ghost">
              幽灵按钮
            </Button>
            <Button size="sm" variant="destructive">
              危险按钮
            </Button>
            <Badge>默认徽章</Badge>
            <AgentBadge />
            <ThemeToggle />
          </div>
          <Input placeholder="输入框占位" className="max-w-xs" />
        </Section>
        <Separator />
        <Section title="Avatars">
          <div className="flex items-center gap-3">
            {participants.map((p) => (
              <ParticipantAvatar key={p.id} participant={p} />
            ))}
          </div>
        </Section>
        <Separator />
        <Section title="Conversation Item">
          <div className="max-w-xs space-y-1">
            <ConversationItem room={rooms[0]} active />
            <ConversationItem room={rooms[1]} />
            <ConversationItem room={rooms[3]} />
          </div>
        </Section>
        <Separator />
        <Section title="Message Bubble">
          <div className="space-y-6">
            <MessageBubble message={messages[0]} />
            <MessageBubble message={messages[1]} />
            <MessageBubble message={messages[2]} />
            <MessageBubble message={messages[3]} />
          </div>
        </Section>
        <Separator />
        <Section title="Stream Elements">
          <div className="space-y-6">
            <DateDivider label="今天" />
            <UnreadDivider />
          </div>
          <div className="text-body">
            正文里的提及：<MentionText text="@Courier 和 @Aria 会被自动高亮，普通文本不变。" />
          </div>
        </Section>
        <Separator />
        <Section title="Edge Cases">
          <div className="max-w-xs space-y-1">
            <ConversationItem room={rooms[4]} />
          </div>
          <div className="space-y-6">
            <MessageBubble message={messages[4]} />
            <MessageBubble message={messages[5]} />
          </div>
          <div className="rounded-xl border border-dashed border-border px-6 py-10 text-center text-body text-muted-foreground">
            这个会话还没有消息，说一句开始吧。
          </div>
          <div className="rounded-xl border border-border">
            <EmptyRoomsState />
          </div>
          {/* 新用户的第一屏（2026-10-05）：一支 Agent 都没有时，这一块改成**指路**而不是
              "从左侧列表挑一个房间"（他左边是空的）。两档都摆出来，改文案的人一眼看到差别。 */}
          <div className="rounded-xl border border-border">
            <EmptyRoomsState noAgents />
          </div>
        </Section>
        <Separator />
        <Section title="Connection & Pill">
          <div className="space-y-2 overflow-hidden rounded-xl border border-border">
            <ConnectionBanner state="connecting" />
            <ConnectionBanner state="offline" />
            <ConnectionBanner state="backend-offline" target="Courier" />
          </div>
          <div>
            <NewMessagePill count={2} />
          </div>
        </Section>
        <Separator />
        <Section title="Message Grouping">
          <div className="space-y-6">
            <MessageBubble message={messages[4]} />
            <div className="-mt-5">
              <MessageBubble message={messages[5]} compact />
            </div>
          </div>
          <p className="text-caption text-muted-foreground">
            同作者连续消息：隐藏头像与名字、间距收紧 4px、气泡圆角收敛（rounded-xl），形成成组视觉。
          </p>
        </Section>
        <Separator />
        <Section title="Loading States">
          <div className="grid gap-4 md:grid-cols-2">
            <div className="overflow-hidden rounded-xl border border-border">
              <ChatSkeleton />
            </div>
            <div className="overflow-hidden rounded-xl border border-border">
              <MessageSkeleton />
            </div>
          </div>
          <p className="text-caption text-muted-foreground">
            异步列表一律用骨架屏（bg-muted + animate-pulse），禁止 spinner。
          </p>
        </Section>
        <Separator />
        <Section title="Recall">
          <div className="space-y-6">
            <SystemMessage message={messages[messages.length - 1]} />
            <p className="text-caption text-muted-foreground">
              撤回是终态 tombstone：消息行仍在历史中，投影为居中系统行「X 撤回了一条消息」，无气泡、无操作菜单、不可回复；引用已撤回消息显示「[该消息已撤回]」占位。
            </p>
          </div>
        </Section>
        <Separator />
        <Section title="Image Preview Lightbox">
          <div>
            <Button size="sm" onClick={onTogglePreview} aria-label="打开图片预览演示">
              {previewOpen ? '关闭预览' : '打开图片预览'}
            </Button>
          </div>
          <p className="text-caption text-muted-foreground">
            点击聊天中的图片打开全屏预览：overlay 遮罩、序号/文件名、← / → / Esc 切换与关闭。
          </p>
        </Section>
        <Separator />
        <Section title="Video Attachment">
          {/* 左边那段演示用的是仓库自己录的视频（`public/mock/video-1.webm`，320x180 · 约 1.5s ·
              约 11 KB）：本机没有 ffmpeg，是浏览器 MediaRecorder + canvas 录出来的 ——
              自己生成，没有第三方素材。右边那格刻意指向一个不存在的地址，演示降级。 */}
          <div className="flex flex-wrap items-start gap-4">
            <VideoAttachment
              attachment={{
                id: 'demo-video',
                kind: 'file',
                name: 'video-1.webm',
                mime: 'video/webm',
                url: '/mock/video-1.webm',
              }}
              roomId={rooms[0].id}
            />
            <VideoAttachment
              attachment={{
                id: 'demo-video-unreachable',
                kind: 'file',
                name: '读不到的这一段.webm',
                mime: 'video/webm',
                url: '/mock/not-here.webm',
              }}
              roomId={rooms[0].id}
            />
          </div>
          <p className="text-caption text-muted-foreground">
            视频附件（服务端 content_type 以 video/ 开头）内联播放，与图片、语音走同一条鉴权 blob 源，绝不直连附件地址；
            右边那一个是「源读不到时的样子」——降级成文件卡片（自带下载）加一句提示，不留一块点不动的黑框。          </p>
        </Section>
        <Separator />
        <Section title="Emoji & Sticker">
          <div className="w-fit">
            <EmojiPicker onInsertEmoji={() => {}} onSendSticker={() => {}} />
          </div>
          <p className="text-caption text-muted-foreground">
            表情 tab 把 emoji 字符插入输入框正文（纯文本，对齐 Core body 契约）；表情包 tab 把 sticker 作为图片附件加入待发条（走附件路径）。
          </p>
        </Section>
        <Separator />
        <Section title="Room Members">
          <div className="space-y-2">
            {participants.slice(0, 4).map((p, i) => (
              <div key={p.id} className="flex items-center gap-2.5">
                <ParticipantAvatar participant={p} size="md" />
                <span className="text-body">{p.displayName}</span>
                {i === 0 ? (
                  <span className="inline-flex items-center gap-1 rounded-full border border-border bg-muted px-2 py-0.5 text-micro text-muted-foreground">
                    <ShieldCheckIcon className="size-3" />
                    群主
                  </span>
                ) : (
                  <AgentBadge className="ml-auto" />
                )}
                {i !== 0 && (
                  <Button variant="ghost" size="sm" className="gap-1 px-2 text-caption">
                    <CrownIcon className="size-3.5" />
                    转让
                  </Button>
                )}
                {i !== 0 && (
                  <Button variant="ghost" size="sm" className="gap-1 px-2 text-caption text-destructive">
                    <UserMinusIcon className="size-3.5" />
                    移除
                  </Button>
                )}
              </div>
            ))}
          </div>
          <p className="text-caption text-muted-foreground">
            参与者列表：群主徽标标识 role=owner；管理动作只对 group owner 可见（转让仅 Human 成员，移除不含自己）。
          </p>
        </Section>
        <Separator />
        <Section title="Markdown">
          <div className="rounded-xl border border-border bg-bubble-peer p-4 text-bubble-peer-foreground">
            <MarkdownText
              text={[
                '## 消息正文支持 Markdown',
                '',
                '**粗体**、*斜体*、`行内代码` 与 @Courier 提及共存；',
                '',
                '- 列表项一',
                '- 列表项二',
                '',
                '```ts',
                'const stick = (near: boolean) => near && scroll()',
                '```',
                '',
                '| 能力 | 状态 |',
                '| --- | --- |',
                '| GFM 表格 | 支持 |',
                '| 代码高亮 | 支持 |',
                '',
                '> 引用块：`body` 契约保持纯文本，渲染语义由客户端决定。',
              ].join('\n')}
            />
          </div>
          <p className="text-caption text-muted-foreground">
            body 契约保持「原样保存的 UTF-8 文本」，Markdown 只在显示层渲染；提及高亮经 text component 注入，无需 rehype-raw（无 XSS 面）。
          </p>
        </Section>
        <Separator />
        <Section title="Diagrams & Math">
          <div className="rounded-xl border border-border bg-bubble-peer p-4 text-bubble-peer-foreground">
            <MarkdownText
              text={[
                '```mermaid',
                'flowchart LR',
                '  A[Human] --> B{Agent Backend}',
                '  B --> C[Core]',
                '  B --> D[Model]',
                '```',
                '',
                '块级公式：',
                '',
                '$$\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}$$',
                '',
                '行内公式 \\(E = mc^2\\) 与金额 $5 and $10（单美元不误伤）。',
              ].join('\n')}
            />
          </div>
          <div className="rounded-xl border border-border bg-bubble-peer p-4 text-bubble-peer-foreground">
            <MarkdownText
              text={[
                '```mermaid',
                'graph TD',
                '  A[未闭合的节点',
                '```',
              ].join('\n')}
            />
          </div>
          <p className="text-caption text-muted-foreground">
            Mermaid 动态加载（不进主包）、按内容与主题缓存、解析失败降级为代码原文；公式用 KaTeX，`$$..$$` 块级与 `\(..\)` 行内（单美元 `$..$` 禁用，避免金额误伤）。
          </p>
        </Section>
        <Separator />
        <Section title="Message Input">
          <MessageInput room={rooms[0]} />
        </Section>
      </div>
    </div>
  )
}

export function GalleryPage() {
  const [previewOpen, setPreviewOpen] = useState(false)
  return (
    <div className="min-h-screen bg-background px-6 py-10">
      <div className="mx-auto max-w-5xl space-y-6">
        <header className="space-y-1">
          <h1 className="text-display">Celira 组件陈列室</h1>
          <p className="text-body text-muted-foreground">
            所有领域组件 × 双主题的正确用法参照。新样式先查这里，不要自由发挥。
          </p>
        </header>
        <section className="space-y-3 rounded-xl border border-border bg-card p-5">
          <h3 className="text-micro tracking-wider text-muted-foreground uppercase">主题预设（日 / 夜）</h3>
          <div className="grid gap-4 sm:grid-cols-2">
            {THEME_PRESETS.map((p) => (
              <div key={p.id} className="space-y-1.5">
                <div className="grid grid-cols-2 gap-2">
                  <ThemePresetPreview presetId={p.id} />
                  <ThemePresetPreview presetId={p.id} dark />
                </div>
                <p className="text-caption text-muted-foreground">
                  {p.name} · {p.description}
                </p>
              </div>
            ))}
          </div>
          <p className="text-caption text-muted-foreground">
            预设 = 一套语义令牌（tokens.css 按 [data-theme] 登记）+ 主题质感层；新增预设先在 tokens.css 与
            src/lib/themes.ts 登记，再补进本节做双模式验收。
          </p>
          <Separator />
          <h3 className="text-micro tracking-wider text-muted-foreground uppercase">配色变体（每主题 · 日 / 夜）</h3>
          <div className="space-y-3">
            {THEME_PRESETS.map((p) => (
              <div key={p.id} className="flex flex-wrap items-center gap-x-4 gap-y-2">
                <span className="w-16 shrink-0 text-caption text-muted-foreground">{p.name}</span>
                {p.variants.map((v) => (
                  <div key={v.id} className="flex flex-col items-center gap-1">
                    <div className="flex gap-1.5">
                      <span
                        data-theme={p.id}
                        data-variant={v.id === 'standard' ? undefined : v.id}
                        className="size-6 rounded-full bg-gradient-to-br from-brand to-bubble-self-to"
                      />
                      <span
                        data-theme={p.id}
                        data-variant={v.id === 'standard' ? undefined : v.id}
                        className="dark size-6 rounded-full bg-gradient-to-br from-brand to-bubble-self-to"
                      />
                    </div>
                    <span className="text-caption text-muted-foreground">{v.name}</span>
                  </div>
                ))}
              </div>
            ))}
          </div>
          <p className="text-caption text-muted-foreground">
            配色变体 = 每套主题手工策展的色板（tokens.css 配色变体层按 [data-theme][data-variant] 登记，
            只覆盖主行动/品牌/焦点环/自己气泡渐变等「色彩声音」令牌），组件样式与布局不变；
            永不触碰 agent 系、presence 五态与 destructive/code 语义色。新增变体须在本节目检日/夜。
          </p>
        </section>
        <ThemePanel title="浅色 · 晨雾浅紫" previewOpen={previewOpen} onTogglePreview={() => setPreviewOpen((v) => !v)} />
        <ThemePanel dark title="深色 · 默认主题" previewOpen={previewOpen} onTogglePreview={() => setPreviewOpen((v) => !v)} />
        <ImagePreviewLightbox
          open={previewOpen}
          images={[
            {
              id: 'g-img-1',
              attachment: { id: 'g-img-1', kind: 'image', name: 'concept-1.png', url: '/mock/img-1.svg' },
              roomId: 'r-core',
              name: 'concept-1.png',
            },
            {
              id: 'g-img-2',
              attachment: { id: 'g-img-2', kind: 'image', name: 'palette-dark.png', url: '/mock/img-2.svg' },
              roomId: 'r-core',
              name: 'palette-dark.png',
            },
            {
              id: 'g-img-3',
              attachment: { id: 'g-img-3', kind: 'image', name: 'palette-light.png', url: '/mock/img-3.svg' },
              roomId: 'r-core',
              name: 'palette-light.png',
            },
          ]}
          onClose={() => setPreviewOpen(false)}
        />
      </div>
    </div>
  )
}
