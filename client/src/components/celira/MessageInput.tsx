import { useEffect, useRef, useState } from 'react'
import { FileText, Image as ImageIcon, Keyboard, Mic, Paperclip, Plus, SendHorizonal, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Textarea } from '@/components/ui/textarea'
import type { Attachment, Message, Participant, Room } from '@/api/domain'
import { demoAttachments } from '@/fixtures/data'
import { roomMemberViews } from '@/api/roomMembers'
import { mentionCandidates, mentionsForSend, type MentionMember } from '@/api/mentionTargets'
import { getApi, useAppStore } from '@/api/store'
import { attachmentInputMode, neutralParticipant } from '@/api/uiPolicy'
import {
  createAttachmentUploadFence,
  type AttachmentUploadBatch,
} from '@/api/asyncOwnership'
import {
  audioExtension,
  classifyVoiceStartFailure,
  createVoiceRecorder,
  isVoiceRecordingSupported,
  VOICE_START_FAILURE_TEXT,
  type VoiceRecorder,
} from '@/lib/voice'
import { draftFor, setDraft } from '@/lib/messageDrafts'
import { MentionPopover } from './MentionPopover'
import { EmojiPicker } from './EmojiPicker'
import { cn } from '@/lib/utils'

const MAX_ATTACHMENTS = 8
const MAX_FILE_BYTES = 16 * 1024 * 1024

/**
 * 输入框长高的上限（像素）。见下面那个量高度的 `useEffect`：
 * 到这个高度就不再长，改为自己滚 —— 一屏留给对话，不留给草稿。
 * 约 7 行（行高 21 × 7 ＋ 内距）。
 */
const MAX_COMPOSER_PX = 160

/**
 * typing 上报节流（毫秒）。服务端 `TYPING_TTL` 是 6 秒，2 秒一次足够续期，
 * 也不会把世界问烦。**它只表示"我在写"**——没有正文、没有阶段。
 */
const TYPING_REPORT_MS = 2000

interface MessageInputProps {
  room: Room
  replyTo?: Message | null
  onCancelReply?: () => void
  /** 回复目标作者投影解析器：**由调用方注入真实解析器**（真实连接用真实
   * me + room.participants 投影；解析不出返回 undefined）。
   *
   * 组件**绝不查 fixtures、绝不抛异常**：解析不出时用中性「参与者」占位
   * （同 `MessageBubble` 的 fail-soft 形状）。原来的 `getParticipant(replyTo.authorId)`
   * 是一条 fail-hard
   * 地雷——真实群房里被回复的成员不在查找表里（查找表只有「我」+ 每房 peer，
   * 群房 peer 为空），解析抛异常直接进渲染路径 ⇒ 白屏。 */
  resolveReplyAuthor?: (authorId: string) => Participant | undefined
  /** 允许返回 Promise；当前实现不 await 它（fire-and-forget）。 */
  onSend?: (input: {
    body: string
    replyToId?: string
    attachments?: Attachment[]
    /** 显式提及（participant ID 列表；仅 group Room 非空，契约
     * `CORE_V1_MENTION_TARGETS.md` §2）。 */
    mentions?: string[]
  }) => void | Promise<void>
  editRequest?: { message: Message; seq: number } | null
}

interface MentionState {
  start: number
  query: string
}

function detectMention(value: string, caret: number): MentionState | null {
  const before = value.slice(0, caret)
  const match = /(?:^|\s)@([^\s@]*)$/.exec(before)
  if (!match) return null
  return { start: caret - match[1].length - 1, query: match[1] }
}

export function MessageInput({
  room,
  replyTo,
  onCancelReply,
  resolveReplyAuthor,
  onSend,
  editRequest,
}: MessageInputProps) {
  // 草稿：**首帧快照**（`main.tsx` 在挂载之前已经补过水）——切回这个会话时，
  // 上次没发完的那段字就在框里，不闪、不覆盖。持久面归 `lib/messageDrafts.ts`。
  const [value, setValue] = useState(() => draftFor(room.id))
  const [mention, setMention] = useState<MentionState | null>(null)
  const [activeIndex, setActiveIndex] = useState(0)
  /** 当前正文中仍存在的显式提及（participant ID，按插入顺序去重）；插入时追加、
   * 正文删除对应 @名字 时移除，发送时随消息透传（group 房）。 */
  const [mentionIds, setMentionIds] = useState<string[]>([])
  const [pendingAttachments, setPendingAttachments] = useState<Attachment[]>([])
  const [uploading, setUploading] = useState(false)
  /**
   * **这一格最后一次没做成的事**（`null` = 没这回事）。
   *
   * ⚠ 2026-10-09 改名（原 `inputError`）：它装的不只是上传失败 —— 起录音失败也落这一格，
   * 而每一类都有自己的**具体那一句**（附件那几句 + `VOICE_START_FAILURE_TEXT` 那张封闭表）。
   * 名字跟着事实走，否则下一个人会以为"这里只有上传的错"。
   */
  const [inputError, setInputError] = useState<string | null>(null)
  const [dragging, setDragging] = useState(false)
  const fileInputRef = useRef<HTMLInputElement>(null)
  const meId = useAppStore((s) => s.me?.participant.id)
  const notifyTyping = useAppStore((s) => s.notifyTyping)
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  /** 上一次上报 typing 的时刻（节流用；0 = 从未报过）。 */
  const lastTypingRef = useRef(0)
  const uploadFenceRef = useRef(createAttachmentUploadFence<ReturnType<typeof getApi>>())
  // 语音消息：纯本地 MediaRecorder 采集，产出 kind:'file' 音频附件走既有管道。
  const [voiceMode, setVoiceMode] = useState(false)
  const [recording, setRecording] = useState(false)
  const [recordSeconds, setRecordSeconds] = useState(0)
  const voiceRecorderRef = useRef<VoiceRecorder | null>(null)
  const voiceSupported = isVoiceRecordingSupported()
  // 语音交互随断点分形态：移动端按住说话（拇指长按），桌面端点击开始/停止（鼠标无长按习惯）。
  const [isMobileUi, setIsMobileUi] = useState(
    () => typeof window !== 'undefined' && window.matchMedia('(max-width: 767px)').matches,
  )
  // 建议源：走 `roomMemberViews` 唯一裁决面（core 成员投影 → 房间自带成员 → 目录兜底），
  // 绝不查 fixtures（目录那一跳由 `fixtureFallbackAllowed()` 闸门决定）。解析不出的人
  // **跳过**，**绝不抛异常**——原 `participantIds.map(getParticipant)` 在真实世界查不到就抛，
  // 异常进渲染路径 ⇒ 白屏。
  const realMembers = useAppStore((s) => s.membersByRoom[room.id])
  const roomMembers: Participant[] = roomMemberViews({ core: realMembers, room })
  const memberById = new Map(roomMembers.map((p) => [p.id, p]))
  // 契约 `CORE_V1_MENTION_TARGETS.md` §2：结构化 mentions 只存在于 group Room
  //（direct / human_direct 必须空），目标是房内**任意 active 成员**（Human 或 Agent）。
  // ⇒ 判据是「group 房 + 有可 @ 的别人」；**不再要求房里必须有 Agent**（旧判据让一个
  // 纯人类群房完全 @ 不出人）。候选名单由 `mentionCandidates` 统一裁决（agent 优先）。
  const mentionTargets: MentionMember[] = roomMembers.map((p) => ({
    id: p.id,
    name: p.displayName,
    kind: p.kind,
  }))
  const canMention =
    room.kind === 'group' && mentionCandidates(mentionTargets, { meId }).length > 0

  // ── 附件入口判据（与「Agent 的经历」「我的 Agent」同一律：问**这台后端有没有这个能力**）──
  const currentApi = getApi()
  const attachmentMode = attachmentInputMode(currentApi)

  // 当前组件拥有的本地图片预览 URL；发送后消息记录只保留公开
  // metadata，URL 在预览生命周期结束时回收。
  const attachmentUrlsRef = useRef<Set<string>>(new Set())
  const currentRoomIdRef = useRef(room.id)
  currentRoomIdRef.current = room.id

  const isUploadBatchCurrent = (batch: AttachmentUploadBatch<ReturnType<typeof getApi>>): boolean =>
    uploadFenceRef.current.isCurrent(batch, currentRoomIdRef.current, getApi())

  useEffect(() => {
    for (const attachment of pendingAttachments) {
      if (attachment.localUrl) attachmentUrlsRef.current.add(attachment.localUrl)
    }
  }, [pendingAttachments])

  useEffect(() => {
    const urls = attachmentUrlsRef.current
    return () => {
      for (const url of urls) URL.revokeObjectURL(url)
    }
  }, [])

  useEffect(() => {
    const uploadFence = uploadFenceRef.current
    // 房间、模式或精确 api 实例变化都结束旧批次的所有权。旧批次的
    // await 结果仍会回来，但 success/failure/finally 均只能走 inert 分支。
    uploadFence.invalidate()
    for (const url of attachmentUrlsRef.current) URL.revokeObjectURL(url)
    attachmentUrlsRef.current.clear()
    setValue(draftFor(room.id))
    setMention(null)
    setMentionIds([])
    setPendingAttachments([])
    setInputError(null)
    setUploading(false)
    return () => uploadFence.invalidate()
  }, [room.id, currentApi])

  useEffect(() => {
    if (!recording) return
    const timer = setInterval(() => setRecordSeconds((s) => s + 1), 1000)
    return () => clearInterval(timer)
  }, [recording])

  // 换房/卸载时丢弃在录语音并释放麦克风。
  useEffect(() => () => voiceRecorderRef.current?.cancel(), [room.id])

  useEffect(() => {
    const mq = window.matchMedia('(max-width: 767px)')
    const onChange = () => setIsMobileUi(mq.matches)
    mq.addEventListener('change', onChange)
    return () => mq.removeEventListener('change', onChange)
  }, [])

  useEffect(() => {
    if (replyTo) textareaRef.current?.focus()
  }, [replyTo])

  /**
   * **输入框自己量高度**（2026-10-20 真机："这个高度好像容易出问题，有时候高有时候低，
   * 我切换窗口他就变那种"）。
   *
   * 病根是 CSS 的 `field-sizing: content`：高度由内容**隐式**推导，而它不会随着
   * "内容变少 / 布局重算（换窗口、缩放、切主题）"可靠地缩回去 —— 实测就是同一行草稿
   * 有时 36px、有时 130px。⇒ 这里改成**显式量**：先把高度归零、读 `scrollHeight`、
   * 写回一个确定值（上限 `MAX_COMPOSER_PX`，超过就自己滚）。
   * 判据：**同一段内容，任何一次重排后算出来的高度必须逐像素相同**。
   */
  useEffect(() => {
    const el = textareaRef.current
    if (!el) return
    el.style.height = 'auto'
    const next = Math.min(el.scrollHeight, MAX_COMPOSER_PX)
    el.style.height = `${next}px`
    el.style.overflowY = el.scrollHeight > MAX_COMPOSER_PX ? 'auto' : 'hidden'
  }, [value, voiceMode, isMobileUi])

  useEffect(() => {
    if (!editRequest) return
    const { message } = editRequest
    const next = message.body
    setValue(next)
    updateDraft(next)
    setMention(null)
    for (const url of attachmentUrlsRef.current) URL.revokeObjectURL(url)
    attachmentUrlsRef.current.clear()
    setPendingAttachments([])
    onCancelReply?.()
    textareaRef.current?.focus()
    textareaRef.current?.setSelectionRange(next.length, next.length)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editRequest])

  const updateDraft = (next: string) => {
    setDraft(room.id, next)
  }

  const addAttachments = (items: Attachment[]) => {
    setPendingAttachments((prev) => [...prev, ...items].slice(0, MAX_ATTACHMENTS))
  }

  const removeAttachment = (id: string) => {
    setPendingAttachments((prev) => {
      const removed = prev.find((a) => a.id === id)
      if (removed?.localUrl) URL.revokeObjectURL(removed.localUrl)
      return prev.filter((a) => a.id !== id)
    })
  }

  const cancelVoiceRecording = () => {
    voiceRecorderRef.current?.cancel()
    voiceRecorderRef.current = null
    setRecording(false)
  }

  const startVoiceRecording = async () => {
    if (recording || attachmentMode === 'unavailable') return
    const recorder = createVoiceRecorder()
    if (!recorder) {
      setInputError(VOICE_START_FAILURE_TEXT.unsupported)
      return
    }
    try {
      await recorder.start()
    } catch (error) {
      // ⚠ **逐类说清**（封闭表在 `voice.ts`）：从前这里只有一句"麦克风不可用或未授权"，
      //   把"没设备 / 被拒 / 壳里那条永远不答的路"三种处境压成了同一句；而壳里那条路
      //   从前连这一句都到不了（`getUserMedia` 永远不答 ⇒ 界面什么都不发生）。
      setInputError(VOICE_START_FAILURE_TEXT[classifyVoiceStartFailure(error)])
      return
    }
    setInputError(null)
    voiceRecorderRef.current = recorder
    setRecordSeconds(0)
    setRecording(true)
  }

  const finishVoiceRecording = async () => {
    const recorder = voiceRecorderRef.current
    if (!recorder) return
    voiceRecorderRef.current = null
    setRecording(false)
    const result = await recorder.stop()
    if (!result) {
      setInputError('说话时间太短')
      return
    }
    const now = new Date()
    const pad = (n: number) => String(n).padStart(2, '0')
    const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`
    const name = `语音-${stamp}.${audioExtension(result.mime)}`
    const file = new File([result.blob], name, { type: result.mime })
    void pickFiles([file], { durationSeconds: result.durationSeconds })
  }

  /**
   * **整窗拖放**（2026-10-08）：拖进窗口任意处，都能把文件加进**这条**消息。
   *
   * 为什么挂在 `window` 上：用户不会瞄着输入区拖（主流都是整窗接）。⚠ 但**判据只有这一处**
   * —— 原来那个容器上的 `onDragOver` / `onDragLeave` / `onDrop` 已一并删掉，否则同一次拖放
   * 会被接两遍，**文件会进两条**。
   *
   * 两条纪律：
   * 1. **只接"拖的是文件"**（`types` 里有 `Files`）—— 页面内拖文字/拖动排序之类不许被这张网拦下；
   * 2. **传不了附件就完全不管**（既不 `preventDefault`，也不亮提示）—— 别拦下浏览器自己的行为。
   */
  useEffect(() => {
    const takesFiles = (event: DragEvent) =>
      Array.from(event.dataTransfer?.types ?? []).includes('Files')
    const uploadable = () => attachmentInputMode(getApi()) !== 'unavailable'
    const onDragOver = (event: DragEvent) => {
      if (!takesFiles(event) || !uploadable()) return
      event.preventDefault()
      setDragging(true)
    }
    const onDragLeave = (event: DragEvent) => {
      // ⚠ 在元素之间移动**也会**发 `dragleave`；只有"离开窗口"（relatedTarget 为空）才算松手区外。
      if (!event.relatedTarget) setDragging(false)
    }
    const onDrop = (event: DragEvent) => {
      if (!takesFiles(event) || !uploadable()) return
      event.preventDefault()
      setDragging(false)
      const files = event.dataTransfer?.files
      if (files && files.length > 0) void pickFiles(files)
    }
    window.addEventListener('dragover', onDragOver)
    window.addEventListener('dragleave', onDragLeave)
    window.addEventListener('drop', onDrop)
    return () => {
      window.removeEventListener('dragover', onDragOver)
      window.removeEventListener('dragleave', onDragLeave)
      window.removeEventListener('drop', onDrop)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps -- pickFiles 每次渲染都是新的闭包，
    // 而它读的 api/房间都是当次渲染的 —— 重挂监听是这条判据的代价（很便宜），换来的是"绝不接错房"。
  })

  const pickFiles = async (files: FileList | File[] | null, meta?: { durationSeconds?: number }) => {
    if (!files || files.length === 0) return
    const api = getApi()
    const inputMode = attachmentInputMode(api)
    // 连接缺失/换代/非 connected（`inputMode === 'unavailable'`）时 fail closed：
    // 不能进入 Demo。api 引用在批次开始时精确捕获，后续
    // generation + identity fence 负责丢弃迟到结果。
    if (inputMode === 'unavailable') return
    const remaining = Math.max(0, MAX_ATTACHMENTS - pendingAttachments.length)
    if (remaining === 0) return

    // 一个 pick/drop/paste = 一个批次。新的批次立即使旧批次失效，避免旧批次
    // 的 success/failure/finally 释放或覆盖当前批次的 uploading/error/附件状态。
    const batch = uploadFenceRef.current.begin(room.id, api)
    const incoming: Attachment[] = []
    let committed = false
    const revokeIncoming = (): void => {
      for (const attachment of incoming) {
        if (attachment.localUrl) URL.revokeObjectURL(attachment.localUrl)
      }
      incoming.length = 0
    }
    const current = (): boolean => isUploadBatchCurrent(batch)

    setInputError(null)
    setUploading(true)
    try {
      for (const file of Array.from(files).slice(0, remaining)) {
        if (file.size > MAX_FILE_BYTES) {
          if (current()) setInputError(`${file.name} 超过 16 MiB，未上传`)
          continue
        }
        if (inputMode === 'real') {
          if (!api) return
          if (!api.uploadAttachment) {
            if (current()) setInputError('当前连接不支持附件上传')
            continue
          }
          try {
            const result = await api.uploadAttachment(room.id, file)
            // await 返回后先验 ownership；旧房/旧 api/旧批次不能创建或加入当前
            // 组件的附件状态。服务端 pending 不在这里主动删除。
            if (!current()) {
              if (result.attachment?.localUrl) URL.revokeObjectURL(result.attachment.localUrl)
              break
            }
            const previewable = file.type.startsWith('image/') || file.type.startsWith('audio/')
            const uploaded = result.attachment ?? {
              id: result.attachmentId,
              kind: file.type.startsWith('image/') ? 'image' : 'file',
              name: file.name,
              size: `${Math.max(1, Math.round(file.size / 1024))} KB`,
              state: 'pending' as const,
              mime: file.type,
              localUrl: previewable ? URL.createObjectURL(file) : undefined,
            }
            // 服务端返回公开元数据；预览使用一次性的本地 object URL，
            // 消息发送/移除/卸载时回收，不把 Blob 写进消息记录。
            const attachment = {
              ...(uploaded.localUrl || !previewable
                ? uploaded
                : { ...uploaded, localUrl: URL.createObjectURL(file) }),
              ...(meta?.durationSeconds ? { durationSeconds: meta.durationSeconds } : {}),
            }
            if (!current()) {
              if (attachment.localUrl) URL.revokeObjectURL(attachment.localUrl)
              break
            }
            incoming.push(attachment)
          } catch {
            if (!current()) break
            setInputError(`${file.name} 上传失败，请重试`)
          }
        } else {
          const previewable = file.type.startsWith('image/') || file.type.startsWith('audio/')
          const attachment: Attachment = {
            id: `demo-${file.name}-${Date.now()}`,
            kind: file.type.startsWith('image/') ? 'image' : 'file',
            name: file.name,
            size: `${Math.max(1, Math.round(file.size / 1024))} KB`,
            mime: file.type,
            localUrl: previewable ? URL.createObjectURL(file) : undefined,
            ...(meta?.durationSeconds ? { durationSeconds: meta.durationSeconds } : {}),
          }
          if (!current()) {
            if (attachment.localUrl) URL.revokeObjectURL(attachment.localUrl)
            break
          }
          incoming.push(attachment)
        }
      }
      if (!current()) {
        revokeIncoming()
        return
      }
      if (incoming.length > 0) {
        addAttachments(incoming)
        committed = true
      }
    } finally {
      // 旧批次 finally 只清理自己尚未提交的 object URL；绝不能把新批次的
      // uploading/error 或已提交附件收尾掉。
      if (current()) {
        uploadFenceRef.current.finish(batch)
        setUploading(false)
      } else if (!committed) {
        revokeIncoming()
      }
    }
  }

  const insertEmoji = (emoji: string) => {
    const el = textareaRef.current
    if (!el) return
    const start = el.selectionStart ?? value.length
    const end = el.selectionEnd ?? value.length
    const next = `${value.slice(0, start)}${emoji}${value.slice(end)}`
    updateValue(next, start + emoji.length)
    requestAnimationFrame(() => {
      el.focus()
      el.setSelectionRange(start + emoji.length, start + emoji.length)
    })
  }

  const addSticker = (attachment: Attachment) => {
    addAttachments([attachment])
  }

  // 候选名单：与建议弹窗、回车选择**同一份判据**（`mentionCandidates`）。
  const candidates = mention ? mentionCandidates(mentionTargets, { meId, query: mention.query }) : []
  const candidateCount = candidates.length

  const updateValue = (next: string, caret: number) => {
    setValue(next)
    updateDraft(next)
    // 主体进行态：「我正在这间房写」。**节流 + best-effort**：
    // - 只在真的有正文时上报（空输入不算在写）；
    // - 每 `TYPING_REPORT_MS` 至多一次（服务端 TTL 6s，2s 一次足够续期）；
    // - 失败**不提示**——进行态丢了不影响任何消息，服务端会按 TTL 让它消失。
    if (next.trim().length > 0) {
      const now = Date.now()
      if (now - lastTypingRef.current >= TYPING_REPORT_MS) {
        lastTypingRef.current = now
        void notifyTyping(room.id)
      }
    }
    // 移除已从正文删除的 @名字 对应 id（保留仍在文本中的提及）。名字从
    // 房间成员投影解析（真实成员投影优先，绝不查 fixtures）。
    setMentionIds((prev) =>
      prev.filter((id) => {
        const p = memberById.get(id)
        return p ? next.includes(`@${p.displayName}`) : false
      }),
    )
    // 契约 §2：只 group Room 识别 @ 提及；direct/human_direct 不弹建议。
    const m = canMention ? detectMention(next, caret) : null
    setMention(m)
    setActiveIndex(0)
  }

  const insertMention = (participantId: string) => {
    if (!mention) return
    const p = memberById.get(participantId)
    if (!p) return
    const name = p.displayName
    const next = `${value.slice(0, mention.start)}@${name} ${value.slice(mention.start + mention.query.length + 1)}`
    setValue(next)
    setMentionIds((prev) => (prev.includes(participantId) ? prev : [...prev, participantId]))
    updateDraft(next)
    setMention(null)
    textareaRef.current?.focus()
  }

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (!mention) {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault()
        handleSend()
      }
      return
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setActiveIndex((i) => (i + 1) % Math.max(candidateCount, 1))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setActiveIndex((i) => (i - 1 + Math.max(candidateCount, 1)) % Math.max(candidateCount, 1))
    } else if (e.key === 'Enter' || e.key === 'Tab') {
      e.preventDefault()
      // 候选 = 同一份 `mentionCandidates` 判据（排除自己、名字匹配、agent 优先）；
      // Human 与 Agent 都可选中，且**都进结构化 mentions**（服务端已放开 Human 目标）。
      const target = candidates[Math.min(activeIndex, candidates.length - 1)]
      if (target) insertMention(target.id)
    } else if (e.key === 'Escape') {
      setMention(null)
    }
  }

  const handleSend = () => {
    const body = value.trim()
    if (!body && !replyTo && pendingAttachments.length === 0) return
    onSend?.({
      body,
      replyToId: replyTo?.id,
      attachments: pendingAttachments.length > 0 ? pendingAttachments : undefined,
      mentions: mentionsForSend(room.kind, mentionIds),
    })
    setValue('')
    updateDraft('')
    setMention(null)
    setMentionIds([])
    setPendingAttachments([])
    onCancelReply?.()
    textareaRef.current?.focus()
  }

  // 回复目标作者投影：调用方注入的真实解析器优先（解析不出给中性「参与者」）。
  // **绝不查 fixtures、绝不抛异常**（同 `MessageBubble` 的 fail-soft 形状）；
  // 目标已撤回/未加载时仅显示占位。
  const replyAuthor: Participant | null = replyTo
    ? (resolveReplyAuthor?.(replyTo.authorId) ?? neutralParticipant(replyTo.authorId))
    : null
  // 撤回事实**只认权威历史那一格**（`recalledAt`）：它没有第二条来路。
  // （原先这里还 `|| isMessageRecalled(replyTo.id)`，而那条函数问的是零实现的方法
  // ⇒ 恒 false；2026-09-25 拆分刀把那条死路连同函数一起删了。）
  const replyRecalled = replyTo ? replyTo.recalledAt != null : false
  const replyPreview = replyTo
    ? replyRecalled
      ? '[该消息已撤回]'
      : replyTo.body.length > 40
        ? `${replyTo.body.slice(0, 40)}…`
        : replyTo.body || '[附件]'
    : ''

  return (
    <div
      className={cn(
        'px-4 pt-2 pb-5 transition-shadow md:px-6 max-md:border-t max-md:border-border max-md:bg-card max-md:px-2 max-md:pb-[max(0.5rem,env(safe-area-inset-bottom))]',
        dragging && 'rounded-xl ring-2 ring-ring',
      )}
      onPaste={(e) => {
        const files = e.clipboardData?.files
        if (
          files &&
          files.length > 0 &&
          attachmentInputMode(getApi()) !== 'unavailable'
        ) {
          e.preventDefault()
          void pickFiles(files)
        }
      }}
    >
      <div className="relative mx-auto w-full max-w-5xl">
        {mention && canMention && (
          <MentionPopover
            members={roomMembers}
            query={mention.query}
            activeIndex={activeIndex}
            onSelect={insertMention}
          />
        )}
        {replyTo && replyAuthor && (
          <div className="mb-2 flex items-center gap-2 rounded-lg border-l-2 border-primary/60 bg-muted px-2.5 py-1.5 text-caption text-muted-foreground">
            <span className="min-w-0 flex-1 truncate">
              <span className="font-medium">{replyAuthor.displayName}</span>
              <span className="mx-1.5 opacity-50">·</span>
              <span className="opacity-80">{replyPreview}</span>
            </span>
            <button
              type="button"
              onClick={onCancelReply}
              aria-label="取消回复"
              className="shrink-0 cursor-pointer rounded-full p-0.5 hover:bg-background"
            >
              <X className="size-3.5" />
            </button>
          </div>
        )}
        {pendingAttachments.length > 0 && (
          <div className="mb-2 flex flex-wrap items-center gap-2 rounded-lg bg-muted px-2.5 py-2">
            {pendingAttachments.map((a) => (
              <div
                key={a.id}
                className="flex items-center gap-2 rounded-md border border-border bg-card px-2 py-1 text-caption text-muted-foreground"
              >
                {a.kind === 'image' && (a.localUrl ?? a.url) ? (
                  <img
                    src={a.localUrl ?? a.url}
                    alt={a.name}
                    className="size-6 rounded-sm object-cover"
                  />
                ) : (
                  <span className="flex size-6 items-center justify-center rounded-sm bg-accent text-accent-foreground">
                    <FileText className="size-3.5" />
                  </span>
                )}
                <span className="max-w-32 truncate">{a.name}</span>
                <button
                  type="button"
                  onClick={() => removeAttachment(a.id)}
                  aria-label={`移除附件 ${a.name}`}
                  className="cursor-pointer rounded-full p-0.5 hover:bg-background"
                >
                  <X className="size-3" />
                </button>
              </div>
            ))}
          </div>
        )}
        {/* 移动端为微信式平铺栏：无外胶囊，[语音切换][文本域][表情][⊕/发送]；桌面保持胶囊 */}
        {/* 桌面：**一个表面 ＋ 下面一条工具行**（微信那种"工具在左、输入区高"的形状）。
           判据来自真机（2026-10-20）：工具放左边、文字又在同一行 ⇒ 草稿一长，文字被挤到右侧，
           左边工具**上方那一块永远打不进字**（主人："光标都在另一边了"）。⇒ 文字占满整行、
           工具条落到下面一行；移动端本来就是平铺一条，`max-md:` 那串把它拆回同一行。 */}
        <div className="flex flex-col gap-0.5 rounded-xl border border-input bg-card px-2 pt-1.5 pb-1 transition-colors focus-within:border-ring max-md:flex-row max-md:items-end max-md:gap-1 max-md:rounded-none max-md:border-0 max-md:bg-transparent max-md:px-1.5 max-md:py-1">
          {voiceMode ? (
            isMobileUi ? (
              <button
                type="button"
                aria-label={recording ? '松开发送，移出取消' : '按住说话'}
                disabled={uploading}
                onPointerDown={(e) => {
                  e.preventDefault()
                  void startVoiceRecording()
                }}
                onPointerUp={() => {
                  if (recording) void finishVoiceRecording()
                }}
                onPointerLeave={() => {
                  if (recording) cancelVoiceRecording()
                }}
                onPointerCancel={() => {
                  if (recording) cancelVoiceRecording()
                }}
                onContextMenu={(e) => e.preventDefault()}
                className={cn(
                  'min-h-9 w-full touch-none rounded-lg border px-3 py-2 text-center text-body transition-colors select-none max-md:order-1 max-md:flex-1',
                  recording
                    ? 'border-ring bg-accent text-accent-foreground'
                    : 'border-input bg-background text-muted-foreground',
                )}
              >
                {recording ? `松开 发送 · ${recordSeconds}″` : '按住 说话'}
              </button>
            ) : recording ? (
              <div className="flex min-h-9 w-full items-center justify-center gap-3 rounded-lg border border-ring bg-accent px-3 py-1.5 select-none">
                <span className="flex items-center gap-1.5 text-caption text-accent-foreground">
                  <span className="size-2 animate-pulse rounded-full bg-presence-failed" />
                  录音中 {recordSeconds}″
                </span>
                <Button
                  type="button"
                  size="sm"
                  aria-label="停止并发送语音"
                  onClick={() => void finishVoiceRecording()}
                >
                  停止并发送
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  aria-label="取消录音"
                  onClick={cancelVoiceRecording}
                >
                  取消
                </Button>
              </div>
            ) : (
              <button
                type="button"
                aria-label="点击开始录音"
                disabled={uploading}
                onClick={() => void startVoiceRecording()}
                className="min-h-9 w-full cursor-pointer rounded-lg border border-input bg-background px-3 py-2 text-center text-body text-muted-foreground transition-colors select-none hover:text-foreground"
              >
                点击 开始录音
              </button>
            )
          ) : (
            <Textarea
              ref={textareaRef}
              variant="bare"
              value={value}
              onChange={(e) => updateValue(e.target.value, e.target.selectionStart ?? e.target.value.length)}
              onKeyDown={onKeyDown}
              placeholder=""
              aria-label={`发送到 ${room.name}`}
              rows={1}
              // `variant="bare"`（不是在这串类名里逐条覆盖）：`dark:bg-input/30` 在原子件的
              // 基线上、与 `bg-transparent` 同权重 ⇒ 谁在 CSS 里靠后就谁赢，删不干净；
              // 而"容器就是表面"这件事该由**变体**说，判据见 `ui/textarea.tsx`。
              className="min-h-9 w-full resize-none px-1.5 py-1.5 max-md:order-1 max-md:min-h-6 max-md:flex-1"
            />
          )}
          {/* 工具条：桌面上是文字下面那一行；移动端 `contents` 把它拆回同一条平铺行。 */}
          <div className="flex items-center gap-0.5 max-md:contents">
          {voiceSupported && attachmentMode !== 'unavailable' && (
            <Button
              variant="ghost"
              size="icon"
              aria-label={voiceMode ? '切换到键盘输入' : '切换到语音输入'}
              aria-pressed={voiceMode}
              onClick={() => {
                if (recording) cancelVoiceRecording()
                setVoiceMode((v) => !v)
              }}
              className="shrink-0 max-md:order-0"
            >
              {voiceMode ? <Keyboard className="size-4" /> : <Mic className="size-4" />}
            </Button>
          )}
          {attachmentMode !== 'unavailable' && <div className={cn('flex shrink-0 gap-0.5 pb-0.5 max-md:order-3 max-md:pb-0', value.trim() && 'max-md:hidden')}>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                size="icon"
                aria-label="添加附件"
                disabled={pendingAttachments.length >= MAX_ATTACHMENTS || uploading}
              >
                  {uploading ? (
                    <FileText className="size-4 animate-pulse" />
                  ) : (
                    <>
                      <Paperclip className="size-4 max-md:hidden" />
                      <Plus className="size-5 md:hidden" />
                    </>
                  )}
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="center" side="top">
                {attachmentMode === 'real' ? (
                  <DropdownMenuItem onSelect={() => fileInputRef.current?.click()}>
                    <Paperclip className="size-3.5" />
                    选择文件（≤ 16 MiB）
                  </DropdownMenuItem>
                ) : (
                  <>
                    <DropdownMenuItem onSelect={() => addAttachments(demoAttachments.image)}>
                      <ImageIcon className="size-3.5" />
                      发送图片
                    </DropdownMenuItem>
                    <DropdownMenuItem onSelect={() => addAttachments(demoAttachments.file)}>
                      <FileText className="size-3.5" />
                      发送文件
                    </DropdownMenuItem>
                  </>
                )}
              </DropdownMenuContent>
            </DropdownMenu>
            <input
              ref={fileInputRef}
              type="file"
              multiple
              className="hidden"
              aria-hidden="true"
              tabIndex={-1}
              onChange={(e) => {
                void pickFiles(e.target.files)
                e.target.value = ''
              }}
            />
          </div>}
          {attachmentMode !== 'unavailable' && <div className="flex shrink-0 pb-0.5 max-md:order-2 max-md:pb-0">
            <EmojiPicker
              onInsertEmoji={insertEmoji}
              onSendSticker={addSticker}
              disabled={pendingAttachments.length >= MAX_ATTACHMENTS}
            />
          </div>}
          <div className="flex-1 max-md:hidden" />
          <Button
            size="icon"
            className={cn('shrink-0 max-md:order-3', !value.trim() && 'max-md:hidden')}
            aria-label="发送"
            disabled={(!value.trim() && pendingAttachments.length === 0) || uploading}
            onClick={handleSend}
          >
            <SendHorizonal className="size-4" />
          </Button>
          </div>
        </div>
          {inputError && (
            <p role="alert" className="mt-1.5 text-caption text-presence-failed">
              {inputError}
            </p>
          )}
      </div>
      {/*
        整窗拖放的提示（2026-10-08）：`pointer-events-none` 不是装饰 —— 它让这一次拖放
        **照旧落到 window 上**（否则这层会把 drop 吃掉，松手反而没反应）。
        文案说的是"加进这条消息"而不是"发送"：拖进来只是**加进待发条**，还要用户自己按发送。
      */}
      {dragging && (
        <div className="pointer-events-none fixed inset-0 z-50 flex items-center justify-center bg-overlay/40">
          <div className="rounded-xl border border-border bg-card px-4 py-3 text-body shadow-lg">
            松开就加到这条消息里
          </div>
        </div>
      )}
    </div>
  )
}
